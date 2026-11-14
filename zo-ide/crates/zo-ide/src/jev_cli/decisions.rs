//! `zo jev decisions` (t-107869): the person's own decisions, listed, and
//! the three things this verb changes — when one asks (the store's file
//! beside the index, `custom::ask_when`), how many requests one may send in
//! an hour (a new version of its definition, `store::set_max_per_hour`), and the hourly
//! bound of every decision together (`smart.jev.customHourlyRequests`).
//!
//! Each change answers what it said before, what it says now, and the
//! command that puts it back. The words and the bounds go through the store's
//! own writers and the settings lock every other writer takes: nothing here
//! edits a definition's file or the settings by hand.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{Map, Value, json};
use zerocode_core::jev::custom::{AskWhen, DECISIONS_MAX, HOURLY_REQUESTS, store};
use zerocode_core::jev::door::{self, CUSTOM_HOURLY_REQUESTS_SETTING, JEV_SETTINGS_KEY};
use zerocode_core::jev::SMART_SETTINGS_KEY;
use zerocode_core::settings_file_lock::SettingsFileLock;

use super::{Refused, Report};

/// The settings file under zo's config home.
const SETTINGS_FILE: &str = "settings.json";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Request {
    List,
    AskWhen { decision: String, said: AskWhen },
    PerHour { decision: String, most: u64 },
    AllPerHour { most: u64 },
}

fn refused(message: impl Into<String>) -> Refused {
    Refused { message: message.into(), exit: 2 }
}

/// The most every decision together may send in an hour: each of the most
/// decisions a store keeps at its own highest bound.
fn all_most() -> u64 {
    u64::try_from(DECISIONS_MAX).unwrap_or(u64::MAX).saturating_mul(*HOURLY_REQUESTS.end())
}

fn number(word: Option<&String>, flag: &str) -> Result<u64, Refused> {
    word.and_then(|word| word.parse::<u64>().ok()).ok_or_else(|| refused(format!("{flag} needs a whole number")))
}

pub(super) fn parse(args: &[String]) -> Result<Request, Refused> {
    let words: Vec<&String> = args.iter().filter(|word| *word != "--json").collect();
    let mut rest = words.into_iter();
    match rest.next().map(String::as_str) {
        Some("list") => match rest.next() {
            None => Ok(Request::List),
            Some(extra) => Err(refused(format!("unknown decisions list argument '{extra}'"))),
        },
        Some("ask-when") => {
            let decision = rest
                .next()
                .filter(|word| !word.starts_with('-'))
                .ok_or_else(|| refused("decisions ask-when needs a decision's id or name"))?
                .clone();
            let (mut said, mut always) = (AskWhen::default(), false);
            while let Some(flag) = rest.next() {
                match flag.as_str() {
                    "--word" => said.words.push(rest.next().ok_or_else(|| refused("--word needs a word"))?.clone()),
                    "--edited" => said.edited = true,
                    "--always" => always = true,
                    _ => return Err(refused(format!("unknown decisions ask-when argument '{flag}'"))),
                }
            }
            match (always, said.always()) {
                (true, true) | (false, false) => Ok(Request::AskWhen { decision, said }),
                (true, false) => Err(refused("--always says it asks every time: give it alone")),
                (false, true) => Err(refused("decisions ask-when needs --word <word>..., --edited or --always")),
            }
        }
        Some("limit") => match (rest.next().map(String::as_str), rest.next(), rest.next(), rest.next()) {
            (Some("--all"), most, None, None) => Ok(Request::AllPerHour { most: number(most, "--all")? }),
            (Some(decision), Some(flag), most, None) if !decision.starts_with('-') && flag == "--per-hour" => {
                Ok(Request::PerHour { decision: decision.to_string(), most: number(most, "--per-hour")? })
            }
            _ => Err(refused("decisions limit needs <decision> --per-hour <n>, or --all <n>")),
        },
        _ => Err(refused("decisions needs list, ask-when or limit")),
    }
}

pub(super) fn run(request: &Request, home: &Path, now_ms: i64) -> Result<Report, Refused> {
    let said = match request {
        Request::List => listed(home)?,
        Request::AskWhen { decision, said } => {
            let entry = named(home, decision)?;
            let (before, now) = store::set_ask_when(home, &entry.id, said).map_err(|error| match error {
                store::AskWhenError::Missing => refused(format!("no decision is named '{decision}'")),
                store::AskWhenError::Unfit(unfit) => refused(format!("'{}' cannot be told that: {}", entry.name, unfit.word())),
                store::AskWhenError::Io(error) => refused(format!("the decisions store: {error}")),
            })?;
            json!({
                "id": entry.id, "name": entry.name,
                "before": before, "now": now,
                "undo": undo_ask_when(&entry.id, &before),
            })
        }
        Request::PerHour { decision, most } => {
            let entry = named(home, decision)?;
            if !HOURLY_REQUESTS.contains(most) {
                return Err(refused(format!(
                    "a decision's hourly bound is {} to {}",
                    HOURLY_REQUESTS.start(),
                    HOURLY_REQUESTS.end()
                )));
            }
            // Read and written under the store's lock: an edit saved meanwhile stays.
            let (before, version) = store::set_max_per_hour(home, &entry.id, *most, now_ms)
                .map_err(|error| refused(format!("the store kept nothing: {error:?}")))?;
            json!({
                "id": entry.id, "name": entry.name,
                "before": before, "now": most, "version": version,
                "undo": format!("zo jev decisions limit {} --per-hour {before}", entry.id),
            })
        }
        Request::AllPerHour { most } => {
            if *most == 0 || *most > all_most() {
                return Err(refused(format!("every decision's hourly bound is 1 to {}", all_most())));
            }
            let before = set_all_per_hour(&home.join(SETTINGS_FILE), *most)?;
            json!({
                "setting": format!("{SMART_SETTINGS_KEY}.{JEV_SETTINGS_KEY}.{CUSTOM_HOURLY_REQUESTS_SETTING}"),
                "before": before, "now": most,
                "undo": format!("zo jev decisions limit --all {before}"),
            })
        }
    };
    Ok(Report { text: said.to_string(), exit: 0 })
}

/// Every decision: its id, name, mode, point, hourly bound and when it asks,
/// and the bound of every decision together. A file of when they ask that
/// does not read is said beside them, with how to get past it: every reader
/// takes it as saying nothing, so each decision asks every time.
fn listed(home: &Path) -> Result<Value, Refused> {
    let entries = store::list(home).map_err(|error| refused(format!("the decisions store: {error}")))?;
    let (mut asks_when, unreadable) = match store::ask_when(home) {
        Ok(said) => (said, None),
        Err(error) => (BTreeMap::new(), Some(error.to_string())),
    };
    let decisions: Vec<Value> = entries
        .into_iter()
        .map(|entry| {
            let definition = store::definition(home, &entry.definition).ok();
            json!({
                "id": entry.id,
                "name": entry.name,
                "mode": entry.mode.key(),
                "at": definition.as_ref().map(|one| one.at.key()),
                "maxPerHour": definition.as_ref().map(|one| one.max_per_hour),
                "askWhen": asks_when
                    .remove(&entry.id)
                    .zip(definition.as_ref())
                    .map(|(said, one)| said.kept_at(one.at))
                    .unwrap_or_default(),
            })
        })
        .collect();
    let settings = read_settings(&home.join(SETTINGS_FILE))?;
    let mut listed = json!({ "decisions": decisions, "allPerHour": door::custom_hourly_requests(&settings) });
    if let Some(error) = unreadable {
        listed["askWhenUnreadable"] = Value::String(error);
    }
    Ok(listed)
}

/// The decision `word` names — its id, or else its name.
fn named(home: &Path, word: &str) -> Result<store::Entry, Refused> {
    let entries = store::list(home).map_err(|error| refused(format!("the decisions store: {error}")))?;
    entries
        .iter()
        .find(|entry| entry.id == word)
        .or_else(|| entries.iter().find(|entry| entry.name.trim() == word.trim()))
        .cloned()
        .ok_or_else(|| refused(format!("no decision is named '{word}'")))
}

/// The command that says `before` again.
fn undo_ask_when(id: &str, before: &AskWhen) -> String {
    if before.always() {
        return format!("zo jev decisions ask-when {id} --always");
    }
    let mut command = format!("zo jev decisions ask-when {id}");
    for word in &before.words {
        command.push_str(" --word ");
        command.push_str(&quoted(word));
    }
    if before.edited {
        command.push_str(" --edited");
    }
    command
}

/// `word` as one shell word.
fn quoted(word: &str) -> String {
    if !word.is_empty() && word.chars().all(|character| character.is_ascii_alphanumeric() || "-_./:".contains(character)) {
        word.to_string()
    } else {
        format!("'{}'", word.replace('\'', r"'\''"))
    }
}

fn read_settings(path: &Path) -> Result<Value, Refused> {
    match std::fs::read_to_string(path) {
        Ok(text) if !text.trim().is_empty() => {
            serde_json::from_str(&text).map_err(|error| refused(format!("{} is not JSON: {error}", path.display())))
        }
        Ok(_) => Ok(Value::Object(Map::new())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Value::Object(Map::new())),
        Err(error) => Err(refused(format!("{}: {error}", path.display()))),
    }
}

/// Set `smart.jev.customHourlyRequests` to `most` under the settings lock,
/// leaving every other key as it stood; the bound it read before. The
/// person's file is replaced as `runtime::replace_file_atomic` replaces one:
/// with the permissions it had (an owner-only file may still hold an old
/// secret) and synced before the rename.
fn set_all_per_hour(path: &Path, most: u64) -> Result<u64, Refused> {
    let _guard = SettingsFileLock::acquire(path).map_err(|error| refused(format!("the settings lock: {error}")))?;
    let mut root = read_settings(path)?;
    let before = door::custom_hourly_requests(&root);
    let object = root.as_object_mut().ok_or_else(|| refused("settings.json is not a JSON object"))?;
    let smart = object.entry(SMART_SETTINGS_KEY).or_insert_with(|| Value::Object(Map::new()));
    let smart = smart.as_object_mut().ok_or_else(|| refused(format!("settings.json's {SMART_SETTINGS_KEY} is not an object")))?;
    let jev = smart.entry(JEV_SETTINGS_KEY).or_insert_with(|| Value::Object(Map::new()));
    let jev = jev
        .as_object_mut()
        .ok_or_else(|| refused(format!("settings.json's {SMART_SETTINGS_KEY}.{JEV_SETTINGS_KEY} is not an object")))?;
    jev.insert(CUSTOM_HOURLY_REQUESTS_SETTING.to_string(), Value::from(most));
    let bytes = serde_json::to_vec_pretty(&root).map_err(|error| refused(error.to_string()))?;
    runtime::replace_file_atomic(path, &bytes).map_err(|error| refused(format!("{}: {error}", path.display())))?;
    Ok(before)
}

#[cfg(test)]
mod tests {
    use zerocode_core::jev::custom::{self, Answer, Definition, Point};
    use zerocode_core::jev::JevMode;

    use super::*;

    fn words(values: &[&str]) -> Vec<String> {
        values.iter().map(|word| (*word).to_string()).collect()
    }

    fn keep(home: &Path, name: &str, at: Point) -> String {
        let definition = Definition::new(
            custom::new_id(),
            name.to_string(),
            "Did the agent look first?".to_string(),
            Answer::YesNo { yes: "It did not look.".to_string(), no: "It looked.".to_string() },
            at,
        );
        store::create(home, &definition, JevMode::Shadow, 1).expect("kept");
        definition.id
    }

    fn said(report: &Report) -> Value {
        serde_json::from_str(&report.text).expect("JSON")
    }

    #[test]
    fn the_verbs_refuse_what_they_cannot_read() {
        for args in [
            vec![],
            vec!["list", "extra"],
            vec!["ask-when"],
            vec!["ask-when", "d-00000001"],
            vec!["ask-when", "d-00000001", "--word"],
            vec!["ask-when", "d-00000001", "--always", "--word", "rm"],
            vec!["limit"],
            vec!["limit", "d-00000001"],
            vec!["limit", "d-00000001", "--per-hour"],
            vec!["limit", "d-00000001", "--per-hour", "many"],
            vec!["limit", "--all"],
            vec!["unknown"],
        ] {
            assert!(parse(&words(&args)).is_err(), "{args:?}");
        }
        assert_eq!(
            parse(&words(&["ask-when", "Deleting", "--word", "rm", "--word", "reset --hard", "--json"])),
            Ok(Request::AskWhen {
                decision: "Deleting".to_string(),
                said: AskWhen { words: words(&["rm", "reset --hard"]), edited: false },
            })
        );
        assert_eq!(parse(&words(&["limit", "--all", "240"])), Ok(Request::AllPerHour { most: 240 }));
    }

    #[test]
    fn asking_when_is_set_by_name_and_answers_how_to_put_it_back() {
        let home = tempfile::tempdir().unwrap();
        let id = keep(home.path(), "Deleting", Point::BeforeCommand);
        let report = run(
            &parse(&words(&["ask-when", "Deleting", "--word", "rm", "--word", "reset --hard"])).unwrap(),
            home.path(),
            2,
        )
        .unwrap();
        let said_now = said(&report);
        assert_eq!(said_now["before"], json!({}));
        assert_eq!(said_now["now"], json!({ "words": ["rm", "reset --hard"] }));
        assert_eq!(said_now["undo"], format!("zo jev decisions ask-when {id} --always"));
        let again = run(&parse(&words(&["ask-when", &id, "--always"])).unwrap(), home.path(), 3).unwrap();
        assert_eq!(said(&again)["undo"], format!("zo jev decisions ask-when {id} --word rm --word 'reset --hard'"));
        assert!(store::ask_when(home.path()).unwrap().is_empty());

        let asked = keep(home.path(), "Asked", Point::AgentAsks);
        let unfit = run(&parse(&words(&["ask-when", &asked, "--word", "rm"])).unwrap(), home.path(), 4);
        assert!(unfit.is_err(), "an agent's decision is asked on purpose");
    }

    #[test]
    fn a_decisions_bound_is_a_new_version_and_the_same_question() {
        let home = tempfile::tempdir().unwrap();
        let id = keep(home.path(), "Build line", Point::BeforeCommand);
        let before = store::list(home.path()).unwrap()[0].definition.clone();
        let report = run(&parse(&words(&["limit", "Build line", "--per-hour", "60"])).unwrap(), home.path(), 5).unwrap();
        let said_now = said(&report);
        assert_eq!((said_now["before"].as_u64(), said_now["now"].as_u64()), (Some(custom::HOURLY_DEFAULT), Some(60)));
        assert_eq!(said_now["undo"], format!("zo jev decisions limit {id} --per-hour {}", custom::HOURLY_DEFAULT));
        let after = store::list(home.path()).unwrap()[0].definition.clone();
        assert_ne!(after, before);
        let (old, new) = (store::definition(home.path(), &before).unwrap(), store::definition(home.path(), &after).unwrap());
        assert_eq!(new.max_per_hour, 60);
        assert_eq!(old.meaning().unwrap(), new.meaning().unwrap(), "a bound raised asks the same question");
        assert!(run(&parse(&words(&["limit", &id, "--per-hour", "121"])).unwrap(), home.path(), 6).is_err());
    }

    #[test]
    fn every_decisions_bound_is_written_beside_every_other_setting() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join(SETTINGS_FILE);
        std::fs::write(&path, r#"{"model":"keep","smart":{"enabled":true,"jev":{"enabled":true}}}"#).unwrap();
        let report = run(&parse(&words(&["limit", "--all", "240"])).unwrap(), home.path(), 7).unwrap();
        let said_now = said(&report);
        assert_eq!((said_now["before"].as_u64(), said_now["now"].as_u64()), (Some(custom::ALL_HOURLY_DEFAULT), Some(240)));
        assert_eq!(said_now["undo"], format!("zo jev decisions limit --all {}", custom::ALL_HOURLY_DEFAULT));
        let root: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(root["model"], "keep");
        assert_eq!(root["smart"]["enabled"], true);
        assert_eq!(root["smart"]["jev"]["enabled"], true);
        assert_eq!(door::custom_hourly_requests(&root), 240);
        assert!(run(&parse(&words(&["limit", "--all", "0"])).unwrap(), home.path(), 8).is_err());
        let listed = said(&run(&Request::List, home.path(), 9).unwrap());
        assert_eq!(listed["allPerHour"], 240);
    }

    /// A file of when they ask that does not read is said beside the list,
    /// with how to get past it — every reader already takes it as saying
    /// nothing — and a change of words refuses with the same (t-107869).
    #[test]
    fn a_broken_file_of_when_they_ask_says_how_to_get_past_it() {
        let home = tempfile::tempdir().unwrap();
        let id = keep(home.path(), "Deleting", Point::BeforeCommand);
        let listed = said(&run(&Request::List, home.path(), 1).unwrap());
        assert!(listed.get("askWhenUnreadable").is_none(), "{listed}");
        std::fs::write(store::ask_when_path(home.path()), "{ not json").unwrap();
        let listed = said(&run(&Request::List, home.path(), 2).unwrap());
        let unreadable = listed["askWhenUnreadable"].as_str().unwrap_or_default();
        assert!(unreadable.contains("removing it makes every decision ask every time"), "{listed}");
        assert_eq!(listed["decisions"][0]["askWhen"], json!({}), "read as saying nothing");
        let refused = run(&parse(&words(&["ask-when", &id, "--always"])).unwrap(), home.path(), 3).unwrap_err();
        assert!(refused.message.contains(unreadable), "{}", refused.message);
    }

    /// A bound is set on the newest version, and a bound it already has
    /// writes nothing new.
    #[test]
    fn a_bound_it_already_has_writes_no_version() {
        let home = tempfile::tempdir().unwrap();
        let id = keep(home.path(), "Deleting", Point::BeforeCommand);
        let first = said(&run(&parse(&words(&["limit", &id, "--per-hour", "60"])).unwrap(), home.path(), 1).unwrap());
        let again = said(&run(&parse(&words(&["limit", &id, "--per-hour", "60"])).unwrap(), home.path(), 2).unwrap());
        assert_eq!((again["before"].as_u64(), again["now"].as_u64()), (Some(60), Some(60)));
        assert_eq!(again["version"], first["version"]);
    }

    /// The person's settings file keeps the permissions it had (t-107869):
    /// a file kept owner-only stays owner-only, and so does any other mode it
    /// was given, whatever this process's umask; every other key stays.
    #[cfg(unix)]
    #[test]
    fn the_shared_bound_keeps_the_settings_files_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let home = tempfile::tempdir().unwrap();
        let path = home.path().join("settings.json");
        for mode in [0o600, 0o640] {
            let held = json!({ "model": "kept", SMART_SETTINGS_KEY: { JEV_SETTINGS_KEY: { CUSTOM_HOURLY_REQUESTS_SETTING: 60 } } });
            std::fs::write(&path, held.to_string()).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
            assert_eq!(set_all_per_hour(&path, 240), Ok(60));
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, mode, "a file at {mode:o}");
            let read: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            assert_eq!((read["model"].as_str(), door::custom_hourly_requests(&read)), (Some("kept"), 240));
        }
    }
}
