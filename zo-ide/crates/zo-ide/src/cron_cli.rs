//! `zo cron` — a workspace's cron registry, from outside a session.
//!
//! The registry the `CronCreate` tool writes is per working directory
//! (`<cwd>/.zo/registries/crons.json`) and is read by whichever zo session is
//! idle there. Something that is not a session — the `ZeroCode` window seeding
//! the second brain's weekly review — needs the same file written by the same
//! writer, with the same merge, lock and tombstone rules, rather than a second
//! spelling of the format. This is that door: three verbs, JSON receipts, and
//! every receipt names the scheduler that will fire the record.
//!
//! ```text
//! zo cron ensure --description <name> --schedule <expr> (--prompt <text> | --prompt-file <path|->) [--cwd <dir>]
//! zo cron show   --description <name> [--cwd <dir>]
//! zo cron remove --description <name> [--cwd <dir>]
//! ```
//!
//! `ensure` is idempotent ([`CronRegistry::ensure_named`]): the same words
//! twice leave one record; other words replace it; the receipt says which.

use std::io::Read as _;
use std::path::{Path, PathBuf};

use runtime::team_cron_registry::{
    cron_zone, AUTOMATIC_SCHEDULER_NOTE, AUTOMATIC_SCHEDULER_STATUS, CronEntry, CronRegistry,
};
use serde_json::{Value, json};

pub const USAGE: &str = "\
zo cron ensure --description <name> --schedule <expr> (--prompt <text> | --prompt-file <path|->) [--cwd <dir>]
zo cron show   --description <name> [--cwd <dir>]
zo cron remove --description <name> [--cwd <dir>]

  The workspace's cron registry (<cwd>/.zo/registries/crons.json), from
  outside a session. `ensure` keeps exactly one enabled record under the
  description; `show` answers whether one stands; `remove` takes it out.
  Receipts are JSON and name the scheduler that fires the record: the idle
  zo session whose working directory owns the registry.
";

#[derive(Debug, Clone, PartialEq, Eq)]
enum Verb {
    Ensure,
    Show,
    Remove,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Request {
    verb: Verb,
    description: String,
    schedule: Option<String>,
    prompt: Option<PromptSource>,
    cwd: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PromptSource {
    Text(String),
    File(PathBuf),
    Stdin,
}

fn parse(args: &[String]) -> Result<Request, String> {
    let verb = match args.first().map(String::as_str) {
        Some("ensure") => Verb::Ensure,
        Some("show") => Verb::Show,
        Some("remove") => Verb::Remove,
        Some("-h" | "--help") | None => return Err(USAGE.to_string()),
        Some(other) => return Err(format!("unknown `zo cron` verb `{other}`\n\n{USAGE}")),
    };
    let mut description = None;
    let mut schedule = None;
    let mut prompt = None;
    let mut cwd = None;
    let mut iter = args[1..].iter();
    while let Some(flag) = iter.next() {
        let mut value = || {
            iter.next()
                .cloned()
                .ok_or_else(|| format!("{flag} requires a value"))
        };
        match flag.as_str() {
            "--description" => description = Some(value()?),
            "--schedule" => schedule = Some(value()?),
            "--prompt" => prompt = Some(PromptSource::Text(value()?)),
            "--prompt-file" => {
                let path = value()?;
                prompt = Some(if path == "-" {
                    PromptSource::Stdin
                } else {
                    PromptSource::File(PathBuf::from(path))
                });
            }
            "--cwd" => cwd = Some(PathBuf::from(value()?)),
            "--json" => {}
            other => return Err(format!("unknown argument `{other}`\n\n{USAGE}")),
        }
    }
    let description = description
        .filter(|held| !held.trim().is_empty())
        .ok_or("--description is required")?;
    if verb == Verb::Ensure {
        if schedule.is_none() {
            return Err("`zo cron ensure` needs --schedule".to_string());
        }
        if prompt.is_none() {
            return Err("`zo cron ensure` needs --prompt or --prompt-file".to_string());
        }
    }
    Ok(Request {
        verb,
        description,
        schedule,
        prompt,
        cwd,
    })
}

fn read_prompt(source: &PromptSource) -> Result<String, String> {
    match source {
        PromptSource::Text(text) => Ok(text.clone()),
        PromptSource::File(path) => std::fs::read_to_string(path)
            .map_err(|error| format!("could not read {}: {error}", path.display())),
        PromptSource::Stdin => {
            let mut held = String::new();
            std::io::stdin()
                .read_to_string(&mut held)
                .map_err(|error| format!("could not read the prompt from stdin: {error}"))?;
            Ok(held)
        }
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn entry_json(registry: &CronRegistry, entry: &CronEntry) -> Value {
    json!({
        "cron_id": entry.cron_id,
        "schedule": entry.schedule,
        "prompt": entry.prompt,
        "description": entry.description,
        "enabled": entry.enabled,
        "created_at": entry.created_at,
        "updated_at": entry.updated_at,
        "last_run_at": entry.last_run_at,
        "run_count": entry.run_count,
        "next_due_at": registry.next_due_at(&entry.cron_id, now_secs()).ok().flatten(),
    })
}

fn receipt(action: &str, workspace: &Path, registry: &CronRegistry, body: &Value) -> Value {
    let zone = cron_zone();
    let mut out = json!({
        "action": action,
        "workspace": workspace.display().to_string(),
        "registry": registry.persistence_path().map(|path| path.display().to_string()),
        "time_zone": zone.label,
        "time_zone_utc_offset_secs": zone.offset_secs,
        "automatic_scheduler_status": AUTOMATIC_SCHEDULER_STATUS,
        "automatic_scheduler_note": AUTOMATIC_SCHEDULER_NOTE,
    });
    if let (Some(out), Some(body)) = (out.as_object_mut(), body.as_object()) {
        for (key, value) in body {
            out.insert(key.clone(), value.clone());
        }
    }
    out
}

/// Run one verb against the registry of `cwd` (or `--cwd`) and answer the
/// receipt as pretty JSON. `Err` is the sentence for stderr.
pub fn run(args: &[String], cwd: &Path) -> Result<String, String> {
    let request = parse(args)?;
    let workspace = request
        .cwd
        .as_deref()
        .unwrap_or(cwd)
        .canonicalize()
        .map_err(|error| format!("--cwd: {error}"))?;
    if !workspace.is_dir() {
        return Err(format!("{} is not a directory", workspace.display()));
    }
    let registry = CronRegistry::for_workspace(&workspace);
    let answer = match request.verb {
        Verb::Ensure => {
            let schedule = request.schedule.as_deref().unwrap_or_default();
            let prompt = read_prompt(request.prompt.as_ref().ok_or("a prompt is required")?)?;
            let (entry, outcome) = registry.ensure_named(&request.description, schedule, &prompt)?;
            let mut body = entry_json(&registry, &entry);
            if let Some(body) = body.as_object_mut() {
                body.insert("outcome".into(), json!(outcome.as_str()));
                body.insert("registered".into(), json!(true));
            }
            receipt("ensure", &workspace, &registry, &body)
        }
        Verb::Show => {
            let held = registry.find_named(&request.description);
            let body = match held.first() {
                Some(entry) => {
                    let mut body = entry_json(&registry, entry);
                    if let Some(body) = body.as_object_mut() {
                        body.insert("registered".into(), json!(entry.enabled));
                        body.insert("twins".into(), json!(held.len().saturating_sub(1)));
                    }
                    body
                }
                None => json!({ "registered": false, "description": request.description }),
            };
            receipt("show", &workspace, &registry, &body)
        }
        Verb::Remove => {
            let removed = registry.remove_named(&request.description)?;
            receipt(
                "remove",
                &workspace,
                &registry,
                &json!({ "registered": false, "description": request.description, "removed": removed }),
            )
        }
    };
    serde_json::to_string_pretty(&answer).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn the_verbs_parse_and_ensure_insists_on_its_two_inputs() {
        let ensure = parse(&args(&[
            "ensure",
            "--description",
            "x",
            "--schedule",
            "0 21 * * 6",
            "--prompt-file",
            "-",
            "--cwd",
            "/v",
        ]))
        .expect("ensure parses");
        assert_eq!(ensure.verb, Verb::Ensure);
        assert_eq!(ensure.prompt, Some(PromptSource::Stdin));
        assert_eq!(ensure.cwd, Some(PathBuf::from("/v")));
        assert!(parse(&args(&["ensure", "--description", "x", "--schedule", "* * * * *"]))
            .unwrap_err()
            .contains("--prompt"));
        assert!(parse(&args(&["ensure", "--description", "x", "--prompt", "p"]))
            .unwrap_err()
            .contains("--schedule"));
        assert!(parse(&args(&["show"])).unwrap_err().contains("--description"));
        assert!(parse(&args(&["dance"])).unwrap_err().contains("unknown"));
        assert!(parse(&args(&[])).unwrap_err().contains("zo cron ensure"));
    }

    #[test]
    fn ensure_show_remove_round_trip_on_a_workspace_and_name_the_scheduler() {
        let root = tempfile::tempdir().expect("tempdir");
        let name = "zerocode:second-brain:weekly-review";
        let ensure = |schedule: &str| {
            serde_json::from_str::<Value>(
                &run(
                    &args(&[
                        "ensure",
                        "--description",
                        name,
                        "--schedule",
                        schedule,
                        "--prompt",
                        "review the week",
                    ]),
                    root.path(),
                )
                .expect("ensure"),
            )
            .expect("json")
        };
        let first = ensure("0 21 * * 6");
        assert_eq!(first["outcome"], "created");
        assert_eq!(first["registered"], true);
        assert_eq!(first["automatic_scheduler_status"], AUTOMATIC_SCHEDULER_STATUS);
        assert!(first["next_due_at"].is_u64(), "{first}");
        assert!(
            first["registry"]
                .as_str()
                .is_some_and(|path| path.ends_with(".zo/registries/crons.json")),
            "{first}"
        );
        let again = ensure("0 21 * * 6");
        assert_eq!(again["outcome"], "unchanged");
        assert_eq!(again["cron_id"], first["cron_id"]);
        let moved = ensure("0 22 * * 6");
        assert_eq!(moved["outcome"], "replaced");
        assert_ne!(moved["cron_id"], first["cron_id"]);

        let shown: Value = serde_json::from_str(
            &run(&args(&["show", "--description", name]), root.path()).expect("show"),
        )
        .expect("json");
        assert_eq!(shown["registered"], true);
        assert_eq!(shown["schedule"], "0 22 * * 6");
        assert_eq!(shown["twins"], 0);

        let removed: Value = serde_json::from_str(
            &run(&args(&["remove", "--description", name]), root.path()).expect("remove"),
        )
        .expect("json");
        assert_eq!(removed["removed"], 1);
        let gone: Value = serde_json::from_str(
            &run(&args(&["show", "--description", name]), root.path()).expect("show"),
        )
        .expect("json");
        assert_eq!(gone["registered"], false);
        assert_eq!(gone["automatic_scheduler_status"], AUTOMATIC_SCHEDULER_STATUS);

        // A bad schedule is refused before anything is written.
        assert!(
            run(
                &args(&["ensure", "--description", name, "--schedule", "nope", "--prompt", "p"]),
                root.path()
            )
            .is_err()
        );
    }
}
