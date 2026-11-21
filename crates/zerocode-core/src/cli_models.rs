//! What an agent's own CLI says about the models it can run (t-124593).
//!
//! The person's words: 「거기안에 모델들도 보고 devin 에 모델들도보고」 — a
//! coordinator naming `--model` for a CLI should see the models THAT CLI
//! offers this account, read off the CLI rather than typed into a table. So
//! the window asks it: the read-only list command its row names
//! ([`ModelSource::OwnList`]), run on the binary the catalog found, never a
//! session and never a model call. Its answer becomes the agent's lineup —
//! the same rows zo's catalog gives the agents it serves
//! (`summon_difficulty::lineup::Lineup`) — so a summons, the model seat and
//! `agent-list` read one source.
//!
//! Every fact about a CLI lives on its catalog row: the list command's words,
//! read off its `--help` on this machine, and the SHAPE of its answer as
//! values ([`ListShape`]), read off a real run's output. A road whose answer
//! nobody has seen yet is [`ListShape::Unmeasured`]: the window still asks,
//! keeps how much was said and when, and reads no model out of it. This file
//! holds no agent's name and no reader of its own per CLI: a CLI measured
//! later fills its row.
//!
//! The ids stay opaque, as in the launch tuning: a summons naming a model the
//! list does not hold is the CLI's to refuse, and the list is not a gate.

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::capabilities::{ListRoad, ListShape, ModelSource};
pub use crate::process_tree::Ran;

/// How long one ask may run. A list that needs the account's server answers
/// in seconds; one that has not answered in this long is ended, its whole
/// process tree with it.
pub const ASK_LIMIT: Duration = Duration::from_secs(30);

/// The most of a failure's first line kept for a reader — enough to name
/// the cause, short of carrying a page of somebody's output around.
const SAID_CAP: usize = 160;

/// `models.why` for an agent whose row names no source.
const NO_SOURCE: &str =
    "its CLI's help names no command that lists its models, and zo's catalog serves none of them";

/// `models.why` for a list nobody reads: what a list would be for is a
/// `--model`, and the CLI takes none (the catalog's own sentence for that).
const PICKS_NOTHING: &str = crate::orchestration::SUMMON_NO_MODEL_FLAG;

/// The program that answers for zo's catalog, as a reader is shown it.
pub const ZO_CATALOG_PROGRAM: &str = "zo";

/// The words the window asks zo's catalog with — the one spelling the lineup
/// read and the composer's model menu share (`slash_catalog`).
pub const ZO_CATALOG_WORDS: [&str; 2] = ["models", "--json"];

/// One model as its CLI listed it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Listed {
    /// The id the CLI's own model flag takes.
    pub id: String,
    /// The name it shows, when it is not the id itself.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// What came of one ask.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    /// It listed these, in its own order, and named this one its default.
    Listed {
        models: Vec<Listed>,
        default: Option<String>,
    },
    /// It ended well and said this many bytes, which are not read: the road's
    /// shape is unmeasured, or the answer did not hold the measured shape.
    Unread { bytes: usize, why: UnreadWhy },
    /// It ended with this code (`None`: by a signal), and the first line it
    /// said, private values masked.
    Failed { code: Option<i32>, said: String },
    /// It did not end within [`ASK_LIMIT`] and was ended.
    NoAnswer,
    /// It could not be started.
    NotStarted(String),
}

impl Answer {
    /// The models it listed; none for any other answer.
    #[must_use]
    pub fn listed(&self) -> &[Listed] {
        match self {
            Self::Listed { models, .. } => models,
            _ => &[],
        }
    }
}

/// Why an answer that ended well was not read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnreadWhy {
    /// Nobody has measured this road's shape.
    Unmeasured,
    /// The road's measured shape was not what it said.
    NotTheShape,
}

impl UnreadWhy {
    const fn says(self) -> &'static str {
        match self {
            Self::Unmeasured => "the shape of this CLI's answer has not been measured",
            Self::NotTheShape => "the answer was not in the shape measured for this CLI",
        }
    }
}

/// One read of an agent's models: its own CLI's answer, or — for an agent
/// whose models zo's catalog serves — the lineup that catalog gave, said the
/// same way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub agent: String,
    /// The command as it was asked: the program's name and its words.
    pub asked: String,
    pub answer: Answer,
    pub read_at_ms: i64,
    pub took_ms: u64,
}

/// The list command of `agent`'s CLI, where its row names one.
#[must_use]
pub fn road_of(agent: &str) -> Option<ListRoad> {
    crate::agent::agent_spec(agent).and_then(|spec| spec.harness.models.own_list())
}

/// Run `road` on `command` — a command whose program the caller has set
/// (the window's own spawn door, so a Windows window opens no console) —
/// within `limit`, both outputs read: the one bounded question
/// ([`crate::process_tree::run_bounded`]).
pub fn run(mut command: Command, road: &ListRoad, limit: Duration) -> Ran {
    crate::process_tree::as_a_question(
        command
            .args(road.words)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped()),
    );
    crate::process_tree::run_bounded(&mut command, limit)
}

/// What `ran` says, read by `shape`.
#[must_use]
pub fn read(shape: ListShape, ran: &Ran) -> Answer {
    match ran {
        Ran::NotStarted(why) => Answer::NotStarted(why.clone()),
        // An answer that never reached this side is no answer.
        Ran::TimedOut
        | Ran::Ended {
            code: Some(0),
            stdout: None,
            ..
        } => Answer::NoAnswer,
        Ran::Ended {
            code: Some(0),
            stdout: Some(stdout),
            ..
        } => {
            let unread = |why| Answer::Unread {
                bytes: stdout.len(),
                why,
            };
            match shape {
                ListShape::Unmeasured => unread(UnreadWhy::Unmeasured),
                ListShape::Json {
                    list,
                    id,
                    name,
                    default,
                } => json_list(stdout, list, id, name, default).map_or_else(
                    || unread(UnreadWhy::NotTheShape),
                    |(models, default)| Answer::Listed { models, default },
                ),
            }
        }
        Ran::Ended {
            code,
            stdout,
            stderr,
        } => Answer::Failed {
            code: *code,
            said: first_line(
                stderr
                    .as_deref()
                    .filter(|said| !said.trim().is_empty())
                    .or(stdout.as_deref())
                    .unwrap_or_default(),
            ),
        },
    }
}

/// Ask `agent`'s `road` on `command` and keep the answer with when it was
/// read.
pub fn ask(
    command: Command,
    agent: &str,
    road: &ListRoad,
    limit: Duration,
    now_ms: i64,
) -> Snapshot {
    let program = std::path::Path::new(command.get_program())
        .file_name()
        .map_or_else(
            || command.get_program().to_string_lossy().into_owned(),
            |name| name.to_string_lossy().into_owned(),
        );
    let began = Instant::now();
    let ran = run(command, road, limit);
    Snapshot {
        agent: agent.to_string(),
        asked: said_as(&program, road.words),
        answer: read(road.shape, &ran),
        read_at_ms: now_ms,
        took_ms: u64::try_from(began.elapsed().as_millis()).unwrap_or(u64::MAX),
    }
}

/// A command as a reader is shown it: the program, then its words.
#[must_use]
pub fn said_as(program: &str, words: &[&str]) -> String {
    std::iter::once(program)
        .chain(words.iter().copied())
        .collect::<Vec<_>>()
        .join(" ")
}

/// A JSON answer read by the row's pointers: every entry of the list needs
/// its id; the name rides only where it differs from the id; the default is
/// the CLI's own. An entry without an id makes the whole answer not the
/// measured shape — half a list is not the CLI's list.
fn json_list(
    text: &str,
    list: &str,
    id: &str,
    name: Option<&str>,
    default: Option<&str>,
) -> Option<(Vec<Listed>, Option<String>)> {
    let value: serde_json::Value = serde_json::from_str(text.trim()).ok()?;
    let text_at = |value: &serde_json::Value, pointer: &str| {
        value
            .pointer(pointer)
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|said| !said.is_empty())
            .map(str::to_string)
    };
    let models = value
        .pointer(list)?
        .as_array()?
        .iter()
        .map(|entry| {
            let id = text_at(entry, id)?;
            let name = name
                .and_then(|pointer| text_at(entry, pointer))
                .filter(|name| *name != id);
            Some(Listed { id, name })
        })
        .collect::<Option<Vec<_>>>()?;
    Some((models, default.and_then(|pointer| text_at(&value, pointer))))
}

/// The first line that says something, private values masked, cut to
/// [`SAID_CAP`] characters.
fn first_line(text: &str) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default();
    crate::private_data::mask(line)
        .text
        .chars()
        .take(SAID_CAP)
        .collect()
}

/// `models` in an `agent-list` row: what the agent's models were last read
/// from — its own CLI's list, or zo's catalog — with when, or why there is
/// nothing to say:
///
/// - `unknown`: its row names no source;
/// - `not-read`: its CLI lists models and takes no measured model flag
///   (`PICKS_NOTHING`), so a list would pick nothing for a summons, and
///   none is read;
/// - `not-asked`: a source nobody has read yet, with its command;
/// - a read: its state (`listed`, `unread`, `failed`, `no-answer`,
///   `not-started`), the command, and its age.
#[must_use]
pub fn json(agent: &str, snapshot: Option<&Snapshot>, now_ms: i64) -> serde_json::Value {
    let spec = crate::agent::agent_spec(agent);
    let source = spec.map_or(ModelSource::Unknown, |spec| spec.harness.models);
    let program = spec.map_or(agent, |spec| spec.detect);
    let from = match source {
        ModelSource::Unknown => {
            return serde_json::json!({"state": "unknown", "why": NO_SOURCE});
        }
        ModelSource::OwnList(road) => said_as(program, road.words),
        ModelSource::ZoCatalog(_) | ModelSource::ZoCatalogAll | ModelSource::ZoOtherLogin(_) => {
            said_as(ZO_CATALOG_PROGRAM, &ZO_CATALOG_WORDS)
        }
    };
    if spec.is_some_and(|spec| spec.harness.tuning.is_none()) {
        return serde_json::json!({"state": "not-read", "from": from, "why": PICKS_NOTHING});
    }
    let Some(held) = snapshot else {
        return serde_json::json!({"state": "not-asked", "from": from});
    };
    let mut row = serde_json::json!({
        "from": held.asked,
        "readAtMs": held.read_at_ms,
        "ageMs": now_ms.saturating_sub(held.read_at_ms).max(0),
        "tookMs": held.took_ms,
    });
    match &held.answer {
        Answer::Listed { models, default } => {
            row["state"] = serde_json::json!("listed");
            row["ids"] = serde_json::json!(models.iter().map(|one| &one.id).collect::<Vec<_>>());
            row["models"] = serde_json::json!(models);
            row["default"] = serde_json::json!(default);
        }
        Answer::Unread { bytes, why } => {
            row["state"] = serde_json::json!("unread");
            row["bytes"] = serde_json::json!(bytes);
            row["why"] = serde_json::json!(why.says());
        }
        Answer::Failed { code, said } => {
            row["state"] = serde_json::json!("failed");
            row["code"] = serde_json::json!(code);
            row["said"] = serde_json::json!(said);
        }
        Answer::NoAnswer => {
            row["state"] = serde_json::json!("no-answer");
            row["why"] = serde_json::json!(format!(
                "it did not answer within {} s and was ended",
                ASK_LIMIT.as_secs()
            ));
        }
        Answer::NotStarted(why) => {
            row["state"] = serde_json::json!("not-started");
            row["why"] = serde_json::json!(why);
        }
    }
    row
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shape kiro-cli 2.34.0's answer has, with models made up: the
    /// reader is tested on the shape, never on anybody's account.
    const KIRO_SHAPED: &str = r#"{"models":[
        {"model_name":"auto","description":"picks","model_id":"auto","context_window_tokens":1000000,"rate_multiplier":1.0,"rate_unit":"credit"},
        {"model_name":"Example Model","description":"a model","model_id":"example-model","context_window_tokens":200000,"rate_multiplier":1.3,"rate_unit":"credit"}
    ],"default_model":"auto"}"#;

    fn ended(code: i32, stdout: &str, stderr: &str) -> Ran {
        Ran::Ended {
            code: Some(code),
            stdout: Some(stdout.to_string()),
            stderr: Some(stderr.to_string()),
        }
    }

    fn kiro_road() -> ListRoad {
        road_of("kiro").expect("kiro's row names its list")
    }

    #[test]
    fn a_measured_answer_is_read_by_its_rows_pointers() {
        assert_eq!(
            read(kiro_road().shape, &ended(0, KIRO_SHAPED, "")),
            Answer::Listed {
                models: vec![
                    Listed {
                        id: "auto".into(),
                        name: None,
                    },
                    Listed {
                        id: "example-model".into(),
                        name: Some("Example Model".into()),
                    },
                ],
                default: Some("auto".into()),
            }
        );
        // The same reader, other pointers: a bare list of ids.
        let bare = ListShape::Json {
            list: "",
            id: "",
            name: None,
            default: None,
        };
        assert_eq!(
            read(bare, &ended(0, r#"["model-a", "model-b"]"#, "")).listed(),
            [
                Listed {
                    id: "model-a".into(),
                    name: None,
                },
                Listed {
                    id: "model-b".into(),
                    name: None,
                },
            ]
        );
    }

    /// An answer nobody measured is kept as a size, never read as a list —
    /// and a measured road whose answer changed shape says so instead of
    /// listing half of it.
    #[test]
    fn an_answer_whose_shape_is_not_known_lists_nothing() {
        assert_eq!(
            read(ListShape::Unmeasured, &ended(0, KIRO_SHAPED, "")),
            Answer::Unread {
                bytes: KIRO_SHAPED.len(),
                why: UnreadWhy::Unmeasured,
            }
        );
        for changed in [
            "example-model\nother-model\n",
            r#"{"models":[{"model_name":"no id"}]}"#,
            r#"{"list":[]}"#,
        ] {
            assert_eq!(
                read(kiro_road().shape, &ended(0, changed, "")),
                Answer::Unread {
                    bytes: changed.len(),
                    why: UnreadWhy::NotTheShape,
                },
                "{changed}"
            );
        }
    }

    /// A failure keeps its code and the first thing it said — masked, cut
    /// short — so a row can say why without carrying the CLI's whole output.
    #[test]
    fn a_failed_ask_names_its_code_and_its_first_words() {
        let long = "x".repeat(SAID_CAP * 2);
        assert_eq!(
            read(
                kiro_road().shape,
                &ended(1, "", &format!("\n  Error: not signed in {long}\nmore"))
            ),
            Answer::Failed {
                code: Some(1),
                said: format!("Error: not signed in {long}")
                    .chars()
                    .take(SAID_CAP)
                    .collect(),
            }
        );
        // Nothing on stderr: the first words are stdout's.
        assert_eq!(
            read(ListShape::Unmeasured, &ended(3, "usage: no such list", "")),
            Answer::Failed {
                code: Some(3),
                said: "usage: no such list".into(),
            }
        );
    }

    /// A row's list command is a list of words its own row reads back.
    #[test]
    fn every_list_road_on_the_catalog_is_words_its_row_reads() {
        for spec in &crate::agent::AGENT_SPECS {
            let Some(road) = spec.harness.models.own_list() else {
                continue;
            };
            assert!(!road.words.is_empty(), "{} asks nothing", spec.id);
            assert_eq!(road_of(spec.id), Some(road), "{}", spec.id);
        }
    }

    #[test]
    fn a_row_says_unknown_not_read_not_asked_or_what_was_heard_with_its_age() {
        assert_eq!(json("qwen-code", None, 0)["state"], "unknown");
        assert_eq!(json("opencode", None, 0)["state"], "not-read");
        let unasked = json("kiro", None, 0);
        assert_eq!(unasked["state"], "not-asked");
        assert_eq!(unasked["from"], "kiro-cli chat --list-models --format json");
        assert_eq!(json("claude", None, 0)["from"], "zo models --json");

        let heard = Snapshot {
            agent: "kiro".into(),
            asked: "kiro-cli chat --list-models --format json".into(),
            answer: read(kiro_road().shape, &ended(0, KIRO_SHAPED, "")),
            read_at_ms: 1_000,
            took_ms: 420,
        };
        let row = json("kiro", Some(&heard), 61_000);
        assert_eq!(row["state"], "listed");
        assert_eq!(row["ids"], serde_json::json!(["auto", "example-model"]));
        assert_eq!(row["default"], "auto");
        assert_eq!(row["readAtMs"], 1_000);
        assert_eq!(row["ageMs"], 60_000);
        assert_eq!(row["from"], "kiro-cli chat --list-models --format json");
        assert_eq!(row["models"][1]["name"], "Example Model");
    }

    /// The roads as the window runs them, against the real CLIs of this
    /// machine — the samples an unmeasured shape is read from (t-124593).
    /// Runs only by hand: `ZEROCODE_CLI_MODEL_SAMPLES` names an existing
    /// folder for the answers and `ZEROCODE_CLI_MODEL_BINS` the binaries the
    /// catalog resolved, as `agent=/path,…`. Each road is asked once; none
    /// opens a session or calls a model. The pane's own coordinates are taken
    /// out of the child, as the window's own process holds none.
    #[test]
    #[ignore = "asks the real CLIs of this machine; run by hand with its two variables"]
    fn the_real_clis_answer_their_lists_once_each() {
        let (Ok(folder), Ok(bins)) = (
            std::env::var("ZEROCODE_CLI_MODEL_SAMPLES"),
            std::env::var("ZEROCODE_CLI_MODEL_BINS"),
        ) else {
            return;
        };
        let folder = std::path::Path::new(&folder);
        assert!(folder.is_dir(), "{} is not a folder", folder.display());
        for pair in bins.split(',').map(str::trim).filter(|pair| !pair.is_empty()) {
            let (agent, path) = pair
                .split_once('=')
                .unwrap_or_else(|| panic!("{pair}: expected agent=/path"));
            let road = road_of(agent).unwrap_or_else(|| panic!("{agent} names no list"));
            let mut command = Command::new(path);
            for (name, _) in std::env::vars_os() {
                let text = name.to_string_lossy();
                if text.starts_with("ZEROCODE_") || text.starts_with("CLAUDE") {
                    command.env_remove(&name);
                }
            }
            let began = Instant::now();
            let ran = run(command, &road, ASK_LIMIT);
            let took = u64::try_from(began.elapsed().as_millis()).unwrap_or(u64::MAX);
            let (code, stdout, stderr) = match &ran {
                Ran::Ended {
                    code,
                    stdout,
                    stderr,
                } => (
                    *code,
                    stdout.clone().unwrap_or_default(),
                    stderr.clone().unwrap_or_default(),
                ),
                Ran::TimedOut => (None, String::new(), "timed out".to_string()),
                Ran::NotStarted(why) => (None, String::new(), why.clone()),
            };
            let keep = |name: &str, text: &str| {
                std::fs::write(
                    folder.join(format!("{agent}-models.{name}")),
                    crate::private_data::mask(text).text,
                )
                .expect("a sample is written");
            };
            keep("stdout.txt", &stdout);
            keep("stderr.txt", &stderr);
            keep(
                "meta.json",
                &serde_json::to_string_pretty(&serde_json::json!({
                    "asked": said_as(path, road.words),
                    "code": code,
                    "tookMs": took,
                    "read": format!("{:?}", read(road.shape, &ran)),
                }))
                .expect("json"),
            );
        }
    }

    /// The run itself, against a fake CLI: what it printed comes back with
    /// its code, and one that will not end is ended within the limit.
    #[cfg(unix)]
    #[test]
    fn a_fake_cli_is_asked_and_one_that_hangs_is_ended_in_time() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().expect("a scratch folder");
        let fake = dir.path().join("fake-cli");
        std::fs::write(
            &fake,
            format!(
                "#!/bin/sh\nif [ \"$1\" = hang ]; then exec sleep 30; fi\nprintf '%s' '{}'\n",
                KIRO_SHAPED.replace('\n', " ")
            ),
        )
        .expect("the fake is written");
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755))
            .expect("the fake runs");
        let road = kiro_road();
        let heard = ask(Command::new(&fake), "kiro", &road, ASK_LIMIT, 7);
        assert_eq!(heard.asked, "fake-cli chat --list-models --format json");
        assert_eq!(heard.read_at_ms, 7);
        assert!(
            matches!(&heard.answer, Answer::Listed { models, default }
                if models.len() == 2 && default.as_deref() == Some("auto")),
            "{heard:?}"
        );

        let hangs = ListRoad {
            words: &["hang"],
            shape: road.shape,
        };
        let began = Instant::now();
        assert_eq!(
            run(Command::new(&fake), &hangs, Duration::from_millis(300)),
            Ran::TimedOut
        );
        assert!(
            began.elapsed() < Duration::from_secs(10),
            "a hung CLI held the ask for {:?}",
            began.elapsed()
        );
        assert!(matches!(
            run(Command::new(dir.path().join("nobody")), &road, ASK_LIMIT),
            Ran::NotStarted(_)
        ));
    }
}
