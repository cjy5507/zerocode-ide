//! What an automation run leaves behind, written by the window itself.
//!
//! An agent told "leave screenshots" may or may not. The surfaces it drives —
//! `zerocode-emulator`, `zerocode-browser`, `zerocode-computer` — are this
//! window's, so when a run asks for evidence the window writes a step line
//! and the frame after every state-changing action into the run's folder,
//! and "성공했습니다" can be checked against files rather than taken at its
//! word. A run that did not ask has no folder, and nothing here runs.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock, PoisonError};
use tauri::Manager as _;

use serde::{Deserialize, Serialize};
use zerocode_core::computer_use_protocol::frame::ShotFrame;

/// The step log: one JSON object per line, in the order things happened.
pub const STEPS_FILE: &str = "steps.jsonl";
/// A walk's own record beside the steps it walked — `walk-001.json`,
/// numbered per folder in the order the walks ended (plan D4).
pub const WALK_FILE_PREFIX: &str = "walk-";
pub const WALK_FILE_EXTENSION: &str = "json";
/// The width step and walk numbers are written at, so names sort as they
/// happened — and the report names its frames by.
pub const NUMBER_WIDTH: usize = 3;
/// The frames a step keeps are PNGs; anything else a backend hands back is
/// not written under a `.png` name.
const PNG_SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
/// How much of a refusal a step line keeps.
const ERROR_CHARS: usize = 400;

/// One line of the step log, as written and as read back.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Step {
    pub n: usize,
    pub at_epoch_ms: i64,
    pub tool: String,
    pub verb: String,
    pub argv: Vec<String>,
    pub ok: bool,
    /// Measurements and recipe origin supplied by the actual step call.
    /// Absent in old logs; neither timing nor retry origin is inferred by UI.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observation: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// A refused step's code, read from the whole refusal before `error` is
    /// cut to `ERROR_CHARS` — what a tally counts by, never re-parsed text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    /// The step acts on the desktop (core `ComputerMethod::acts`): what "the
    /// first act" of a run means, without a copy of the verb table.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub acts: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shot: Option<String>,
    /// Where the frame sits on the screen and how large it is: what a
    /// report needs to draw a recorded rectangle over it (plan D11).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame: Option<FrameMeta>,
    /// Why a step whose verb frames has no frame — a step without one is
    /// never shown as if it had (review 19).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame_skipped: Option<FrameSkipped>,
}

/// Why a step has no frame though its verb frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FrameSkipped {
    /// A later step on the same screen would have been in the picture; that
    /// step's frame shows this one.
    Superseded,
    /// The writer was behind by more than its table: lines first.
    Backlog,
    /// The walk's Flow asked for no frames (`verdict-only`, `off`).
    Off,
    /// The session folder holds as many frames as the table allows.
    Capped,
    /// The capture failed, timed out, or was not a PNG.
    CaptureFailed,
}

/// A frame's place on the screen (`ShotFrame`: picture pixels per point and
/// the top-left in points) and its size in pixels.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FrameMeta {
    pub scale: f64,
    pub origin: [f64; 2],
    pub width: u32,
    pub height: u32,
}

/// The frame a step is recorded with: a picture and where it sits, a reason
/// there is none, or nothing to frame at all (a stop, a wait, a refusal).
pub enum Framing<'a> {
    Picture { png: &'a [u8], placed: ShotFrame },
    Skipped(FrameSkipped),
    None,
}

/// The folder a shim presented, admitted only when it is one of ours: under
/// this window's automations tree, and already made by the run's start. A
/// header is a claim; the tree is the fence.
pub fn fenced_dir(local_data_root: &Path, presented: Option<&str>) -> Option<PathBuf> {
    let dir = PathBuf::from(presented?);
    (dir.starts_with(local_data_root.join("automations")) && dir.is_dir()).then_some(dir)
}

/// Whether one verb of one tool is a step — and whether a frame follows it.
///
/// `None`: not evidence of anything happening (a listing, a tree, a read).
/// `Some(false)`: worth a line, but nothing to frame yet — the browser's
/// `open` has no label until the tab exists. `Some(true)`: the frame after
/// it is the proof. An explicit `screenshot` counts, so the agent's own
/// captures sit in the same log as the window's.
#[must_use]
pub fn captures(tool: &str, verb: &str) -> Option<bool> {
    match (tool, verb) {
        ("emulator", verb) if zerocode_core::agent_emulator::acts(verb) => Some(true),
        // Deterministic AX checks leave a line; their verdict needs no picture.
        ("emulator", verb) if zerocode_core::agent_emulator::is_check(verb) => Some(false),
        ("emulator", "screenshot")
        | (
            "browser",
            "goto" | "click" | "type" | "eval" | "wait" | "screenshot" | "viewport" | "scroll"
            | "find",
        )
        | (
            "computer",
            "click"
            | "perform-secondary-action"
            | "scroll"
            | "drag"
            | "type-text"
            | "press-key"
            | "hotkey"
            | "paste-text"
            | "set-value"
            // The desktop, the apps, the windows and the system (docs/design/
            // computer-use-full-operator.md §1.4): every action leaves a line
            // and the frame after it; an explicit screenshot counts as before.
            | "screenshot"
            | "zoom"
            | "mouse-click"
            | "mouse-drag"
            | "mouse-scroll"
            | "key"
            | "hold-key"
            | "type"
            | "launch"
            | "quit"
            | "activate"
            | "open"
            | "run"
            | "window-focus"
            | "window-move"
            | "window-resize"
            | "window-minimize"
            | "window-zoom"
            | "window-close"
            | "clipboard-write",
        ) => Some(true),
        // `marks` numbers the controls; the line stays in the log, but the
        // picture with the numbers on it is `screenshot --marks`, so nothing
        // is framed after the marks line itself.
        ("emulator" | "browser", "open" | "marks") | ("browser", "close") => Some(false),
        // The one hand's own words are worth a line — a stop in the log says
        // why the actions end — and so are the checks, the pauses, the
        // person's turn, the ears and a hover (the menu a later click needs):
        // a recipe saved from the log keeps them, and the walk that replays
        // it judges its checks. None frames anything.
        (
            "computer",
            "stop" | "resume" | "wait-for" | "sound-wait" | "handoff" | "wait" | "listen-start"
            | "listen-stop" | "mouse-move",
        ) => Some(false),
        _ => None,
    }
}

/// The argv as the log keeps it: what was typed is replaced by its length.
/// A QA run types passwords and one-time codes, and a step log that kept
/// them would be the leak the evidence folder was never meant to be. What a
/// check looks for on the screen (`find`/`wait-for --text`) and what a click
/// is aimed at (`click --text`) is kept: it is the screen's text, which the
/// frames already hold, and a recipe replays it.
#[must_use]
pub fn redacted(tool: &str, argv: &[String]) -> Vec<String> {
    let typing = tool == "browser" && argv.first().is_some_and(|verb| verb == "type");
    let needle = tool == "computer"
        && argv
            .first()
            .and_then(|verb| zerocode_core::computer_use::verb_method(verb))
            .is_some_and(|method| COMPUTER_NEEDLE_VERBS.contains(&method));
    let mut hide_next = false;
    argv.iter()
        .enumerate()
        .map(|(index, word)| {
            // A recipe's own words — a word holding its placeholder — are the
            // document's, not what was typed. A browser `type` carries its text
            // last, as one word or after `--value` (the stdin road's shape).
            let hidden =
                (hide_next || (typing && index + 1 == argv.len())) && !names_a_placeholder(word);
            hide_next = !needle && matches!(word.as_str(), "--text" | "--value");
            if hidden {
                redaction(word.chars().count())
            } else {
                word.clone()
            }
        })
        .collect()
}

/// Whether a word holds a recipe's placeholder (`{{name}}`): a line a recipe
/// walk logs in its own words — the document's text, with its names.
fn names_a_placeholder(word: &str) -> bool {
    !zerocode_core::computer_recipe::recipe_placeholders(word).is_empty()
}

/// The verbs whose `--text` is what the screen should show, not what the
/// hand types: a check's needle, and a click's target — the control named
/// by what it reads, kept so a saved walk replays the press (t-4227).
const COMPUTER_NEEDLE_VERBS: &[zerocode_core::computer_use::ComputerMethod] = &[
    zerocode_core::computer_use::ComputerMethod::Click,
    zerocode_core::computer_use::ComputerMethod::Find,
    zerocode_core::computer_use::ComputerMethod::WaitFor,
];
/// How a redacted word reads in the log — written and read back here only.
const REDACTED_OPEN: &str = "[";
const REDACTED_CLOSE: &str = " chars]";

fn redaction(chars: usize) -> String {
    format!("{REDACTED_OPEN}{chars}{REDACTED_CLOSE}")
}

/// The length a redacted word stands for, if it is one.
#[must_use]
pub fn redacted_chars(word: &str) -> Option<usize> {
    word.strip_prefix(REDACTED_OPEN)?
        .strip_suffix(REDACTED_CLOSE)?
        .parse()
        .ok()
}

/// One writer at a time, so two panes of one run number their steps in
/// sequence instead of both taking the same next number.
static WRITING: Mutex<()> = Mutex::new(());

static WINDOW: OnceLock<tauri::AppHandle> = OnceLock::new();

/// Bind the existing writer to this window. Emission is best effort and
/// happens only after a complete append, under the writer's ordering lock.
pub fn install_window(app: tauri::AppHandle) {
    let _ = WINDOW.set(app);
}

/// Deliver only into the trusted main webview. Tauri's plain `listen()` uses
/// an Any target, which can defeat emit filters (see `dirty_event_for`).
/// JSON is parsed as data in that one webview, never broadcast to guests.
pub fn emit<T: Serialize>(event: &str, value: T) {
    let Some(webview) = WINDOW
        .get()
        .and_then(|app| app.get_webview(crate::MAIN_WINDOW_LABEL))
    else {
        return;
    };
    let Ok(payload) = serde_json::to_string(&(event, value)) else {
        return;
    };
    let Ok(quoted) = serde_json::to_string(&payload) else {
        return;
    };
    let _ = webview.eval(format!(
        "(()=>{{const [name,payload]=JSON.parse({quoted});window.dispatchEvent(new CustomEvent(name,{{detail:payload}}));}})()"
    ));
}

thread_local! {
    static OBSERVATION: std::cell::RefCell<Option<serde_json::Value>> = const { std::cell::RefCell::new(None) };
}

/// The synchronous recipe/goal road's measured context. A road entering an
/// async operation copies it before awaiting, so another task cannot own it.
pub fn observation() -> Option<serde_json::Value> {
    OBSERVATION.with(|held| held.borrow().clone())
}

/// Scope a measured look/judgment to the press it caused. Unwinding restores
/// the previous context as well, so a failed press cannot label a later one.
pub fn observing<T>(value: serde_json::Value, run: impl FnOnce() -> T) -> T {
    struct Restore(Option<serde_json::Value>);
    impl Drop for Restore {
        fn drop(&mut self) {
            OBSERVATION.with(|held| *held.borrow_mut() = self.0.take());
        }
    }
    let _restore = Restore(OBSERVATION.with(|held| held.replace(Some(value))));
    run()
}

/// Add the time measured around the actual door call. Verb classification
/// stays with the existing core tables; a read is never labelled a press.
pub fn measured(
    mut observed: Option<serde_json::Value>,
    tool: &str,
    argv: &[String],
    elapsed: std::time::Duration,
) -> Option<serde_json::Value> {
    let verb = argv.first().map_or("", String::as_str);
    let acts = match tool {
        "browser" => zerocode_core::agent_browser::acts(verb),
        "emulator" => zerocode_core::agent_emulator::acts(verb),
        _ => zerocode_core::computer_use::parse_command(argv)
            .is_ok_and(|command| command.method.acts()),
    };
    let key = if acts { "act_ms" } else { "elapsed_ms" };
    let value = observed.get_or_insert_with(|| serde_json::json!({}));
    value[key] = serde_json::json!(u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX));
    observed
}

/// Write one step: its line, and its frame when there is one — or why there
/// is none. Answers the frame's file name. Best effort throughout —
/// evidence that could not be written is a gap in the log, never a refused
/// action.
pub fn record(
    dir: &Path,
    at_epoch_ms: i64,
    tool: &str,
    argv: &[String],
    outcome: Result<(), &str>,
    frame: Framing<'_>,
) -> Option<String> {
    record_measured(dir, (at_epoch_ms, None), tool, argv, outcome, frame)
}

/// The same recorder, with the actual road's observation attached.
pub fn record_measured(
    dir: &Path,
    observed_at: (i64, Option<serde_json::Value>),
    tool: &str,
    argv: &[String],
    outcome: Result<(), &str>,
    frame: Framing<'_>,
) -> Option<String> {
    record_with(dir, observed_at, tool, argv, outcome, frame, |dir, step| {
        emit("flow:step", (dir, step));
    })
}

fn record_with(
    dir: &Path,
    (at_epoch_ms, observation): (i64, Option<serde_json::Value>),
    tool: &str,
    argv: &[String],
    outcome: Result<(), &str>,
    frame: Framing<'_>,
    emitted: impl FnOnce(&Path, &Step),
) -> Option<String> {
    let verb = argv.first().map_or("?", String::as_str);
    let _writing = WRITING.lock().unwrap_or_else(PoisonError::into_inner);
    let steps = dir.join(STEPS_FILE);
    let (n, separated) = match std::fs::read(&steps) {
        Ok(held) => (
            held.split(|byte| *byte == b'\n')
                .filter(|line| !line.iter().all(u8::is_ascii_whitespace))
                .count()
                + 1,
            held.is_empty() || held.last() == Some(&b'\n'),
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (1, true),
        // A log we cannot read cannot be numbered honestly. Evidence may
        // be missing; it must not overwrite a previous step's frame.
        Err(_) => return None,
    };
    let (shot, meta, skipped) = match frame {
        Framing::Picture { png, placed } if png.starts_with(PNG_SIGNATURE) => {
            let name = format!("{n:0width$}-{tool}-{verb}.png", width = NUMBER_WIDTH);
            match std::fs::write(dir.join(&name), png) {
                Ok(()) => {
                    let (x, y) = placed.origin();
                    let meta =
                        crate::computer_use::compare::png_size(png).map(|(width, height)| {
                            FrameMeta {
                                scale: placed.scale(),
                                origin: [x, y],
                                width,
                                height,
                            }
                        });
                    (Some(name), meta, None)
                }
                Err(_) => (None, None, Some(FrameSkipped::CaptureFailed)),
            }
        }
        Framing::Picture { .. } => (None, None, Some(FrameSkipped::CaptureFailed)),
        Framing::Skipped(why) => (None, None, Some(why)),
        Framing::None => (None, None, None),
    };
    let line = Step {
        n,
        at_epoch_ms,
        tool: tool.to_string(),
        verb: verb.to_string(),
        argv: redacted(tool, argv),
        ok: outcome.is_ok(),
        observation,
        error: outcome
            .err()
            .map(|error| error.trim().chars().take(ERROR_CHARS).collect()),
        code: outcome
            .err()
            .and_then(zerocode_core::computer_use_protocol::refusal_code),
        acts: tool == "computer"
            && zerocode_core::computer_use::parse_command(argv)
                .is_ok_and(|command| command.method.acts()),
        shot: shot.clone(),
        frame: meta,
        frame_skipped: skipped,
    };
    if let Ok(mut json) = serde_json::to_string(&line) {
        use std::io::Write as _;
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&steps)
        {
            // A previous best-effort append may have stopped mid-line or
            // mid-codepoint. Start a fresh line before publishing this one.
            if !separated {
                json.insert(0, '\n');
            }
            json.push('\n');
            if file.write_all(json.as_bytes()).is_ok() {
                emitted(dir, &line);
            }
        }
    }
    shot
}

/// The step log read back, in order; a line that does not parse is skipped.
#[must_use]
pub fn steps_in(dir: &Path) -> Vec<Step> {
    std::fs::read(dir.join(STEPS_FILE))
        .map(|held| {
            held.split(|byte| *byte == b'\n')
                .filter_map(|line| serde_json::from_slice(line).ok())
                .collect()
        })
        .unwrap_or_default()
}

/// The one actual row carrying this call's identity. Missing or duplicate
/// provenance cannot borrow a neighbouring row, even if the commands match.
pub fn step_with_id<'a>(steps: &'a [Step], id: Option<&str>) -> Option<&'a Step> {
    let id = id.filter(|id| !id.is_empty())?;
    let mut matches = steps.iter().filter(|step| {
        step.observation
            .as_ref()
            .and_then(|value| value.get("evidence_id"))
            .and_then(serde_json::Value::as_str)
            == Some(id)
    });
    let step = matches.next()?;
    (matches.next().is_none() && steps.iter().filter(|other| other.n == step.n).count() == 1)
        .then_some(step)
}

/// Write a walk's record beside its steps as the folder's next
/// `walk-NNN.json` — under the one writer's lock, so two walks never take
/// one number. Answers the file written.
pub fn record_walk(dir: &Path, report: &serde_json::Value) -> Result<PathBuf, String> {
    let _writing = WRITING.lock().unwrap_or_else(PoisonError::into_inner);
    let n = walk_files(dir).len() + 1;
    let file = dir.join(format!(
        "{WALK_FILE_PREFIX}{n:0width$}.{WALK_FILE_EXTENSION}",
        width = NUMBER_WIDTH
    ));
    // The existing queue places this record after its steps. Resolve only
    // identities the writer actually persisted; failed writes stay unlinked.
    let mut report = report.clone();
    let steps = steps_in(dir);
    if report.get("evidence_ids") == Some(&serde_json::Value::Bool(true))
        && let Some(ran) = report
            .get_mut("ran")
            .and_then(serde_json::Value::as_array_mut)
    {
        for row in ran {
            let n = step_with_id(
                &steps,
                row.get("evidence_id").and_then(serde_json::Value::as_str),
            )
            .map(|step| step.n);
            if let Some(row) = row.as_object_mut() {
                row.remove("evidence_n");
                if let Some(n) = n {
                    row.insert("evidence_n".into(), serde_json::json!(n));
                }
            }
        }
    }
    let body = serde_json::to_vec_pretty(&report).map_err(|error| error.to_string())?;
    std::fs::write(&file, body)
        .map_err(|error| format!("could not write {}: {error}", file.display()))?;
    emit("flow:walk", (dir, report));
    Ok(file)
}

/// The folder's walk records by name — and so in the order the walks ended
/// — as `(file name, record)`; a file that does not parse is skipped, like
/// a step line.
#[must_use]
pub fn walks_in(dir: &Path) -> Vec<(String, serde_json::Value)> {
    walk_files(dir)
        .into_iter()
        .filter_map(|name| {
            let record = serde_json::from_slice(&std::fs::read(dir.join(&name)).ok()?).ok()?;
            Some((name, record))
        })
        .collect()
}

/// The walk records' file names, sorted.
fn walk_files(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|entry| {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    let walk = name.starts_with(WALK_FILE_PREFIX)
                        && Path::new(&name)
                            .extension()
                            .is_some_and(|ext| ext == WALK_FILE_EXTENSION);
                    walk.then_some(name)
                })
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emulator_checks_leave_evidence_without_capturing_a_picture() {
        for row in zerocode_core::agent_emulator::EMULATOR_VERBS
            .iter()
            .filter(|row| row.check)
        {
            assert_eq!(captures("emulator", row.word), Some(false));
        }
    }

    #[test]
    fn flow_retry_links_refuse_missing_duplicate_and_legacy_provenance() {
        let dir = tempfile::tempdir().unwrap();
        let put = |id: Option<&str>| {
            record_measured(
                dir.path(),
                (1, id.map(|id| serde_json::json!({"evidence_id": id}))),
                "computer",
                &words(&["key", "--key", "tab"]),
                Ok(()),
                Framing::None,
            )
        };
        put(None);
        put(Some("actual"));
        let steps = steps_in(dir.path());
        assert_eq!(step_with_id(&steps, Some("actual")).unwrap().n, 2);
        assert!(step_with_id(&steps, None).is_none());
        assert!(step_with_id(&steps, Some("missing")).is_none());
        put(Some("actual"));
        assert!(step_with_id(&steps_in(dir.path()), Some("actual")).is_none());
        let report = serde_json::json!({"evidence_ids": true, "ran": [
            {"step": 1, "evidence_id": "actual", "evidence_n": 2},
            {"step": 2, "evidence_n": 1},
        ]});
        record_walk(dir.path(), &report).unwrap();
        let written = walks_in(dir.path());
        assert!(
            written[0].1["ran"]
                .as_array()
                .unwrap()
                .iter()
                .all(|row| row.get("evidence_n").is_none())
        );
    }

    #[test]
    fn every_emulator_action_is_evidence_and_marks_leave_a_line_without_a_picture() {
        for row in zerocode_core::agent_emulator::EMULATOR_VERBS {
            if zerocode_core::agent_emulator::acts(row.word) {
                assert_eq!(captures("emulator", row.word), Some(true), "{}", row.word);
            }
        }
        assert_eq!(captures("emulator", "marks"), Some(false));
    }

    #[test]
    fn live_steps_are_the_written_redacted_values_and_preserve_order() {
        let dir = tempfile::tempdir().unwrap();
        let emitted = Mutex::new(Vec::new());
        std::thread::scope(|scope| {
            for at in 0..12 {
                let dir = dir.path();
                let emitted = &emitted;
                scope.spawn(move || {
                    record_with(
                        dir,
                        (
                            at,
                            Some(
                                serde_json::json!({ "act_ms": 7, "judgment": { "asked": false } }),
                            ),
                        ),
                        "computer",
                        &words(&["type", "--text", "secret"]),
                        Ok(()),
                        Framing::None,
                        |folder, step| {
                            let written = steps_in(folder);
                            assert_eq!(
                                serde_json::to_value(written.last().unwrap()).unwrap(),
                                serde_json::to_value(step).unwrap()
                            );
                            emitted
                                .lock()
                                .unwrap()
                                .push(serde_json::to_value(step).unwrap());
                        },
                    );
                });
            }
        });
        let emitted = emitted.into_inner().unwrap();
        assert_eq!(
            emitted,
            serde_json::to_value(steps_in(dir.path()))
                .unwrap()
                .as_array()
                .unwrap()
                .clone()
        );
        assert!(!serde_json::to_string(&emitted).unwrap().contains("secret"));
        assert_eq!(emitted[0]["observation"]["act_ms"], 7);
        crate::computer_use::report::write(dir.path()).unwrap();
        assert!(crate::computer_use::report::verify(dir.path()).reproduced);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 3);
    }

    #[test]
    fn a_judged_press_keeps_its_measurements_and_a_later_read_is_not_a_press() {
        let dir = tempfile::tempdir().unwrap();
        observing(
            serde_json::json!({"look_ms": 12, "judgment": {"asked": true, "ms": 34, "confidence": 0.7}}),
            || {
                let observed = measured(
                    observation(),
                    "browser",
                    &words(&["click", "page", "#ok"]),
                    std::time::Duration::from_millis(8),
                );
                record_with(
                    dir.path(),
                    (1, observed),
                    "browser",
                    &words(&["click", "page", "#ok"]),
                    Ok(()),
                    Framing::None,
                    |_, step| {
                        assert_eq!(
                            step.observation.as_ref().unwrap()["judgment"]["confidence"],
                            0.7
                        )
                    },
                );
            },
        );
        assert!(observation().is_none());
        let read = measured(
            None,
            "browser",
            &words(&["marks", "page"]),
            std::time::Duration::from_millis(5),
        )
        .unwrap();
        assert_eq!(read["elapsed_ms"], 5);
        assert!(read.get("act_ms").is_none());
        assert!(read.get("judgment").is_none());
    }

    #[test]
    fn live_steps_stay_readable_after_a_partial_utf8_tail() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(STEPS_FILE), b"{\"n\":1,\"verb\":\"\xe2").unwrap();
        record_with(
            dir.path(),
            (2, None),
            "browser",
            &words(&["click", "page", "#ok"]),
            Ok(()),
            Framing::None,
            |folder, emitted| {
                let read = steps_in(folder);
                assert_eq!(
                    read.len(),
                    1,
                    "the incomplete line must not hide a later complete one"
                );
                assert_eq!(
                    serde_json::to_value(&read[0]).unwrap(),
                    serde_json::to_value(emitted).unwrap()
                );
                assert_eq!(emitted.n, 2);
            },
        );
    }

    #[test]
    fn live_steps_never_emit_an_unwritten_line() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(STEPS_FILE)).unwrap();
        let shot = record_with(
            dir.path(),
            (1, None),
            "computer",
            &words(&["key", "--key", "a"]),
            Ok(()),
            Framing::None,
            |_, _| panic!("a failed append must not emit"),
        );
        assert!(shot.is_none());
    }

    /// A refused step keeps its code even when its refusal is longer than the
    /// line keeps; a plain sentence and an answered step carry none, and the
    /// line then has no `code` key at all.
    #[test]
    fn a_refused_step_keeps_its_code_past_the_cut() {
        let dir = tempfile::tempdir().expect("tempdir");
        let long = serde_json::json!({
            "ok": false,
            "error": { "code": "confirmation_timeout", "message": "x".repeat(600) },
        })
        .to_string();
        record(
            dir.path(),
            1,
            "computer",
            &words(&["mouse-click", "--x", "1", "--y", "1"]),
            Err(&long),
            Framing::None,
        );
        record(
            dir.path(),
            2,
            "computer",
            &words(&["key", "--key", "a"]),
            Err("zerocode-computer: no answer"),
            Framing::None,
        );
        record(
            dir.path(),
            3,
            "computer",
            &words(&["screenshot"]),
            Ok(()),
            Framing::None,
        );
        let steps = steps_in(dir.path());
        assert_eq!(steps[0].code.as_deref(), Some("confirmation_timeout"));
        assert_eq!(
            steps[0].error.as_ref().map(|error| error.chars().count()),
            Some(ERROR_CHARS)
        );
        assert_eq!(
            (steps[1].code.as_deref(), steps[2].code.as_deref()),
            (None, None)
        );
        let log = std::fs::read_to_string(dir.path().join(STEPS_FILE)).unwrap();
        assert_eq!(log.matches("\"code\":").count(), 1, "{log}");
    }

    /// A step says whether it acted, by core's own table — what a run's first
    /// act is measured from; a look does not, and the key is left out.
    #[test]
    fn a_step_says_whether_it_acted() {
        let dir = tempfile::tempdir().expect("tempdir");
        record(
            dir.path(),
            1,
            "computer",
            &words(&["screenshot"]),
            Ok(()),
            Framing::None,
        );
        record(
            dir.path(),
            2,
            "computer",
            &words(&["mouse-click", "--x", "1", "--y", "1"]),
            Ok(()),
            Framing::None,
        );
        record(
            dir.path(),
            3,
            "browser",
            &words(&["click", "--selector", "a"]),
            Ok(()),
            Framing::None,
        );
        let acted: Vec<bool> = steps_in(dir.path()).iter().map(|step| step.acts).collect();
        assert_eq!(acted, [false, true, false]);
        let log = std::fs::read_to_string(dir.path().join(STEPS_FILE)).unwrap();
        assert_eq!(log.matches("\"acts\":true").count(), 1, "{log}");
    }

    fn words(list: &[&str]) -> Vec<String> {
        list.iter().map(ToString::to_string).collect()
    }

    /// Every changing computer verb is a framed step, a stop is a line
    /// without a frame, and a look is no step at all.
    #[test]
    fn every_computer_action_is_a_framed_step_and_a_look_is_none() {
        for verb in [
            "mouse-click",
            "mouse-drag",
            "key",
            "type",
            "launch",
            "quit",
            "open",
            "run",
            "window-close",
            "clipboard-write",
            "screenshot",
            "click",
            "type-text",
        ] {
            assert_eq!(captures("computer", verb), Some(true), "{verb}");
        }
        for verb in [
            "stop",
            "resume",
            "wait-for",
            "sound-wait",
            "handoff",
            "wait",
            "listen-start",
            "listen-stop",
            "mouse-move",
        ] {
            assert_eq!(
                captures("computer", verb),
                Some(false),
                "{verb}: a line, no frame"
            );
        }
        for verb in [
            "find",
            "read",
            "status",
            "displays",
            "list-all-windows",
            "clipboard-read",
        ] {
            assert_eq!(captures("computer", verb), None, "{verb}");
        }
        // Every desktop act the helper makes, and every check a recipe
        // judges, is a step: the log a recipe is saved from cannot drop one.
        for &method in zerocode_core::computer_use::ComputerMethod::ALL {
            let checks = zerocode_core::computer_recipe::RECIPE_CHECKS.contains(&method);
            if (method.acts() && method.provider_name().is_some()) || checks {
                assert!(
                    captures("computer", method.verb_name()).is_some(),
                    "{method:?} acts but leaves no line"
                );
            }
        }
        // A recipe walk logs its own words: a placeholder is a name, kept.
        assert_eq!(
            redacted("computer", &words(&["type", "--text", "{{text-3}}"])),
            words(&["type", "--text", "{{text-3}}"])
        );
        assert_eq!(
            redacted("computer", &words(&["type", "--text", "Dear {{name}}"]))[2],
            "Dear {{name}}",
            "a recipe's own words keep the names a re-save needs"
        );
        assert_eq!(
            redacted("computer", &words(&["type", "--text", "Dear Kim"]))[2],
            "[8 chars]",
            "typed text is hidden"
        );
        assert_eq!(
            redacted(
                "computer",
                &words(&["clipboard-write", "--text", "hunter2"])
            ),
            words(&["clipboard-write", "--text", "[7 chars]"])
        );
        // What a check looks for is the screen's text, kept for the replay;
        // what the hand types never is.
        assert_eq!(
            redacted(
                "computer",
                &words(&["wait-for", "--app", "X", "--text", "Checkout"])
            ),
            words(&["wait-for", "--app", "X", "--text", "Checkout"])
        );
        assert_eq!(
            redacted("computer", &words(&["find", "--text", "Total"])),
            words(&["find", "--text", "Total"])
        );
        for typing in [
            &["type", "--text", "hunter2"][..],
            &["type-text", "--app", "X", "--text", "hunter2"],
            &["paste-text", "--app", "X", "--text", "hunter2"],
            &[
                "set-value",
                "--app",
                "X",
                "--element-index",
                "1",
                "--value",
                "hunter2",
            ],
        ] {
            assert!(
                !redacted("computer", &words(typing)).contains(&"hunter2".to_string()),
                "{typing:?}"
            );
        }
        assert_eq!(
            redacted("emulator", &words(&["text", "--text", "hunter2"]))[2],
            "[7 chars]",
            "a needle is the desktop's word only"
        );
        assert_eq!(redacted_chars(&redaction(6)), Some(6));
        assert_eq!(redacted_chars("[six chars]"), None);
        assert_eq!(redacted_chars("6 chars"), None);
    }

    /// A click's `--text` names the control by what it reads — the screen's
    /// word, like a check's needle — so the log keeps it and a recipe replays
    /// the press without a parameter; what the hand types stays a length.
    #[test]
    fn a_clicks_target_text_is_the_screens_word_not_the_hands() {
        let press = words(&["click", "--app", "X", "--role", "button", "--text", "Amber"]);
        assert_eq!(redacted("computer", &press), press);
        assert_eq!(
            redacted(
                "computer",
                &words(&["type-text", "--app", "X", "--text", "Amber"])
            )[4],
            "[5 chars]"
        );
    }

    /// Steps are numbered in order across calls, a frame is written under
    /// the step's number, typed text is kept only as a length, and a refusal
    /// is a line without a frame.
    #[test]
    fn steps_are_numbered_framed_and_redacted() {
        let dir = tempfile::tempdir().expect("tempdir");
        let png = b"\x89PNG\r\n\x1a\n0000".to_vec();
        let shot = record(
            dir.path(),
            1,
            "emulator",
            &words(&["tap", "--platform", "android", "--x", "0.5", "--y", "0.7"]),
            Ok(()),
            Framing::Picture {
                png: &png,
                placed: ShotFrame::UNIT,
            },
        );
        assert_eq!(shot.as_deref(), Some("001-emulator-tap.png"));
        assert!(dir.path().join("001-emulator-tap.png").is_file());
        let none = record(
            dir.path(),
            2,
            "emulator",
            &words(&["text", "--text", "s3cret"]),
            Err("no focused field\n"),
            Framing::Skipped(FrameSkipped::Backlog),
        );
        assert_eq!(none, None);
        // A frame that is not a PNG is not written under a .png name.
        let refused = record(
            dir.path(),
            3,
            "browser",
            &words(&["click", "b1", "#go"]),
            Ok(()),
            Framing::Picture {
                png: b"nope",
                placed: ShotFrame::UNIT,
            },
        );
        assert_eq!(refused, None);
        let steps = steps_in(dir.path());
        assert_eq!(
            steps.iter().map(|step| step.n).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        assert_eq!(steps[1].argv, words(&["text", "--text", "[6 chars]"]));
        assert_eq!(steps[1].error.as_deref(), Some("no focused field"));
        assert!(!steps[1].ok && steps[1].shot.is_none());
        assert_eq!(
            steps[1].frame_skipped,
            Some(FrameSkipped::Backlog),
            "a line without its frame says why"
        );
        assert!(steps[2].ok && steps[2].shot.is_none());
        assert_eq!(
            steps[2].frame_skipped,
            Some(FrameSkipped::CaptureFailed),
            "bytes that are no PNG are no frame"
        );
        assert!(
            steps[0].frame.is_none() && steps[0].frame_skipped.is_none(),
            "a signature alone has no readable size: the placement is left out, not made up"
        );
        let log = std::fs::read_to_string(dir.path().join(STEPS_FILE)).unwrap();
        assert!(
            log.contains("\"frame_skipped\":\"backlog\"") && !log.contains("\"frame\":null"),
            "{log}"
        );
    }

    /// A walk's record takes the folder's next number and reads back in
    /// order; a folder without one reads back empty.
    #[test]
    fn walk_records_are_numbered_per_folder_and_read_back_in_order() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(walks_in(dir.path()).is_empty());
        let first = record_walk(dir.path(), &serde_json::json!({ "start": 1 })).unwrap();
        assert!(first.ends_with("walk-001.json"), "{}", first.display());
        record_walk(dir.path(), &serde_json::json!({ "start": 4 })).unwrap();
        std::fs::write(dir.path().join("walk-notes.txt"), "x").unwrap();
        let walks = walks_in(dir.path());
        assert_eq!(
            walks
                .iter()
                .map(|(name, record)| (name.as_str(), record["start"].as_u64()))
                .collect::<Vec<_>>(),
            [("walk-001.json", Some(1)), ("walk-002.json", Some(4))]
        );
        assert!(
            record_walk(&dir.path().join("nowhere"), &serde_json::json!({})).is_err(),
            "a folder that is not there is said, not swallowed"
        );
    }

    /// The browser's `type` hides its fourth word; flags hide the next.
    #[test]
    fn what_was_typed_is_kept_only_as_a_length() {
        assert_eq!(
            redacted("browser", &words(&["type", "b1", "#pw", "hunter2"])),
            words(&["type", "b1", "#pw", "[7 chars]"])
        );
        assert_eq!(
            redacted(
                "computer",
                &words(&["type-text", "--app", "Wallet", "--text", "1234"])
            ),
            words(&["type-text", "--app", "Wallet", "--text", "[4 chars]"])
        );
    }

    /// Only a folder inside this window's automations tree is believed.
    #[test]
    fn a_presented_folder_is_fenced_to_the_automations_tree() {
        let root = tempfile::tempdir().expect("tempdir");
        let inside = root
            .path()
            .join("automations")
            .join("j")
            .join("runs")
            .join("1");
        std::fs::create_dir_all(&inside).expect("mkdir");
        let shown = inside.display().to_string();
        assert_eq!(fenced_dir(root.path(), Some(&shown)), Some(inside));
        assert_eq!(fenced_dir(root.path(), Some("/tmp")), None);
        assert_eq!(fenced_dir(root.path(), None), None);
        let missing = root
            .path()
            .join("automations")
            .join("gone")
            .display()
            .to_string();
        assert_eq!(
            fenced_dir(root.path(), Some(&missing)),
            None,
            "a folder the start never made"
        );
    }

    /// Reads are not steps; the browser's open is a step without a frame.
    #[test]
    fn only_actions_are_steps() {
        assert_eq!(captures("emulator", "tap"), Some(true));
        assert_eq!(captures("emulator", "tree"), None);
        assert_eq!(captures("browser", "open"), Some(false));
        assert_eq!(captures("browser", "close"), Some(false));
        assert_eq!(captures("browser", "marks"), Some(false));
        assert_eq!(captures("browser", "find"), Some(true));
        assert_eq!(captures("browser", "read"), None);
        assert_eq!(captures("browser", "tabs"), None);
        assert_eq!(captures("computer", "get-app-state"), None);
        assert_eq!(captures("computer", "click"), Some(true));
    }
}
