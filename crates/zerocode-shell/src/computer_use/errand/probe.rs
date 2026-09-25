//! A goal walk timed on a page of our own (t-6720, `tools/walk-judgment-probe`):
//! the walk the window runs — the question, the Jev wire, the goal world and,
//! in a build that has one, the value seat — with the one road it drives
//! handed the window's own CLI instead of the door inside the window, so one
//! harness times the build before a change and the build after it on the same
//! pane, turn about.
//!
//! What is not the product's is said so on its own line of the row: the
//! page's snapshot a look will carry once the browser door reads one (t-6721
//! U4) is stood in for by one more `eval` after `marks`, timed apart
//! (`standInMs`), and a CLI call is a process where the window's own road is a
//! call. A typed value goes to the CLI on stdin (`type … --value-stdin`), the
//! road the shim already has, and never on a process's argv.
//!
//! Lines between `// after-only {` and `// after-only }` need what only the
//! build after the change has; the driver drops them for the build before.

use std::io::Write as _;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Instant;

use serde_json::{Value, json};
use zerocode_core::agent_browser::{TYPE_VALUE_FLAG, TYPE_VALUE_STDIN_FLAG};
use zerocode_core::computer_recipe::RecipeTool;
use zerocode_core::jev::{BROWSER, JUDGMENT_CACHE, JevMode, Run};
use zerocode_hookd::TeamAnswer;

use super::desk::{Aim, GoalWorld};
use super::live::{Doorway, LiveJudge};
use super::{Branching, Errand, Options, Seen, Why, run_with};

/// The window's browser CLI — the shim every agent in the window drives.
const CLI: &str = "zerocode-browser";

fn knob(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
}

/// The process one road call becomes: the walk's argv, except that a `type …
/// --value <v>` goes as `type … --value-stdin` with `<v>` on stdin — the value
/// never on a process's argv.
pub(super) fn cli_call(argv: &[String]) -> (Vec<String>, Option<String>) {
    if argv.first().map(String::as_str) == Some("type")
        && argv.len() == 5
        && argv[3] == TYPE_VALUE_FLAG
    {
        let mut words = argv[..3].to_vec();
        words.push(TYPE_VALUE_STDIN_FLAG.to_string());
        return (words, Some(argv[4].clone()));
    }
    (argv.to_vec(), None)
}

/// One CLI call, and how long it took whole.
fn drive(argv: &[String]) -> (TeamAnswer, f64) {
    let (words, stdin) = cli_call(argv);
    let began = Instant::now();
    let mut child = Command::new(CLI)
        .args(&words)
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the window's browser CLI on PATH");
    if let (Some(value), Some(mut pipe)) = (stdin, child.stdin.take()) {
        pipe.write_all(value.as_bytes())
            .expect("the value on stdin");
    }
    let out = child.wait_with_output().expect("an answer");
    (
        TeamAnswer {
            exit_code: out.status.code().unwrap_or(1),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        },
        began.elapsed().as_secs_f64() * 1_000.0,
    )
}

/// What an `eval` answered, out of the fence the CLI puts page words in —
/// JSON, or JSON the page wrote as a string.
fn evaled(stdout: &str) -> Option<Value> {
    let inner: Vec<&str> = stdout
        .lines()
        .filter(|line| !line.starts_with("<<<"))
        .collect();
    match serde_json::from_str::<Value>(inner.join("\n").trim()).ok()? {
        Value::String(text) => serde_json::from_str(&text).ok(),
        other => Some(other),
    }
}

/// The page's snapshot, stood in for (U4 not landed): the stand-in script
/// applied to the marks answer's own numbers and selectors, and what it read
/// merged beside the answer's `items` — `None` when it read nothing.
fn stand_in(pane: &str, script: &str, marks: &str) -> Option<(String, f64)> {
    let mut said: Value = serde_json::from_str(marks.trim()).ok()?;
    let pairs: Vec<Value> = said
        .get("items")?
        .as_array()?
        .iter()
        .map(|item| json!([item["mark"], item["selector"]]))
        .collect();
    let expression = format!("({script})({})", Value::Array(pairs));
    let (answer, ms) = drive(&["eval".to_string(), pane.to_string(), expression]);
    let read = evaled(&answer.stdout)?;
    for (key, value) in read.as_object()? {
        said[key] = value.clone();
    }
    Some((said.to_string(), ms))
}

/// A road call as the row keeps it.
struct Call {
    verb: String,
    start_ms: f64,
    ms: f64,
    exit: i32,
}

// after-only {
/// The login the value seat is asked with in a measurement: the one zo keeps
/// in its own store, read here and handed in — never printed, never written.
fn value_login(path: &str) -> String {
    let text = std::fs::read_to_string(path).expect("zo's credential store");
    let parsed: Value = serde_json::from_str(&text).expect("a credential document");
    parsed
        .pointer("/oauth/accessToken")
        .and_then(Value::as_str)
        .expect("a subscription login")
        .to_string()
}
// after-only }

#[test]
#[ignore = "drives a pane of the caller's own in the running window and spends a real key; a measurement, written to a file"]
fn a_goal_walk_timed_on_a_page_of_our_own() {
    let pane = knob("ZEROCODE_WALK_PROBE_PANE").expect("a pane of the caller's own");
    let url = knob("ZEROCODE_WALK_PROBE_URL").expect("the page to walk");
    let key = knob("ZEROCODE_WALK_PROBE_KEY").expect("a TypeSafe key");
    let out = PathBuf::from(knob("ZEROCODE_WALK_PROBE_OUT").expect("where the rows go"));
    let home =
        PathBuf::from(knob("ZEROCODE_WALK_PROBE_HOME").expect("a zo home of the probe's own"));
    let goal = knob("ZEROCODE_WALK_PROBE_GOAL").expect("a goal");
    let until = knob("ZEROCODE_WALK_PROBE_UNTIL");
    let steps: usize = knob("ZEROCODE_WALK_PROBE_STEPS").map_or(4, |n| n.parse().expect("steps"));
    let walks: usize = knob("ZEROCODE_WALK_PROBE_WALKS").map_or(1, |n| n.parse().expect("walks"));
    let label = knob("ZEROCODE_WALK_PROBE_LABEL").unwrap_or_default();
    let scenario = knob("ZEROCODE_WALK_PROBE_SCENARIO").unwrap_or_default();
    let script = knob("ZEROCODE_WALK_PROBE_SNAPSHOT_JS")
        .map(|path| std::fs::read_to_string(path).expect("the stand-in script"));
    let repeated = knob("ZEROCODE_WALK_PROBE_REPLAY").is_some_and(|flag| flag == "1");
    let cache = knob("ZEROCODE_WALK_PROBE_CACHE").unwrap_or_else(|| JevMode::Off.key().to_string());
    let base = knob("ZO_SYSTEMONE_BASE_URL")
        .unwrap_or_else(|| crate::systemone::SYSTEMONE_BASE_URL.to_string());

    // A zo home of the probe's own: its settings consent one workspace and
    // switch the browser seat on; its ledgers are the only ones written.
    let work = home.join("work");
    std::fs::create_dir_all(&work).expect("a workspace");
    let settings = home.join("settings.json");
    std::fs::write(
        &settings,
        json!({ "smart": {
            "enabled": true,
            "jev": { "workspaces": [work.display().to_string()] },
            (BROWSER.setting): JevMode::On.key(),
            (JUDGMENT_CACHE.setting): cache,
        } })
        .to_string(),
    )
    .expect("the probe's settings");

    for walk in 0..walks {
        // The judge the window builds for a walk opens its wire as it is
        // built (`LiveJudge::new`, and the window's own doors before that);
        // this one is handed its key, so it is opened here, before the page
        // loads, as the window's would be by the time a walk looks.
        let mut judge = LiveJudge::at(
            &base,
            &key,
            Doorway {
                settings: Some(settings.clone()),
                workspace: Some(work.clone()),
                seat: &BROWSER,
            },
        )
        .in_run(if repeated { Run::Repeated } else { Run::Fresh });
        judge.wire().warm();
        // The page as it loads, every walk: a fresh document.
        let (went, _) = drive(&["goto".to_string(), pane.clone(), url.clone()]);
        assert_eq!(went.exit_code, 0, "{}", went.stderr);
        let (waited, _) = drive(&["wait".to_string(), pane.clone(), "#search".to_string()]);
        assert_eq!(waited.exit_code, 0, "{}", waited.stderr);

        let began = Instant::now();
        let calls = std::cell::RefCell::new(Vec::<Call>::new());
        let stand_ins = std::cell::RefCell::new(Vec::<f64>::new());
        let mut road = |_: RecipeTool, argv: &[String], _: &[String]| {
            let start_ms = began.elapsed().as_secs_f64() * 1_000.0;
            let (mut answer, ms) = drive(argv);
            calls.borrow_mut().push(Call {
                verb: argv.first().cloned().unwrap_or_default(),
                start_ms,
                ms,
                exit: answer.exit_code,
            });
            if let (Some(script), Some("marks"), 0) = (
                script.as_deref(),
                argv.first().map(String::as_str),
                answer.exit_code,
            ) && let Some((said, took)) = stand_in(&pane, script, &answer.stdout)
            {
                answer.stdout = said;
                stand_ins.borrow_mut().push(took);
            }
            answer
        };
        let at = Errand {
            goal: &goal,
            why: Why::Goal { steps },
            flow: None,
            moves_money: false,
        };
        let walked = {
            let world = GoalWorld::new(
                &mut road,
                Aim::Pane {
                    label: pane.clone(),
                },
                Seen::Page {
                    host: String::new(),
                    path: "/walk-judgment-probe".to_string(),
                },
                until.clone(),
                120_000,
                0,
            );
            // after-only {
            let world = match knob("ZEROCODE_WALK_PROBE_LOGIN_FILE") {
                Some(path) => world.writing(Box::new(super::value::LiveWriter::at(
                    zerocode_core::type_value::ANTHROPIC_WIRE.url,
                    &value_login(&path),
                ))),
                None => world,
            };
            // after-only }
            let mut world = world;
            run_with(
                JevMode::On,
                true,
                Branching::OFF,
                &at,
                &mut judge,
                &mut world,
                Options {
                    overlap: false,
                    rescue: false,
                },
                None,
            )
        };
        let walk_ms = began.elapsed().as_secs_f64() * 1_000.0;
        let now = crate::project_runtime::now_epoch_ms();
        super::write_rows(&BROWSER, judge.wire(), None, &walked.rows, now);
        judge.write_memo_rows(None, now);

        let (oracle, _) = drive(&[
            "eval".to_string(),
            pane.clone(),
            "JSON.stringify(window.probe)".to_string(),
        ]);
        let row = json!({
            "label": label,
            "scenario": scenario,
            "walk": walk,
            "walkMs": walk_ms,
            "pressed": walked.pressed,
            "reached": walked.reached,
            "oracle": evaled(&oracle.stdout),
            "standInMs": stand_ins.into_inner(),
            "calls": calls.into_inner().iter().map(|call| json!({
                "verb": call.verb, "startMs": call.start_ms, "ms": call.ms, "exit": call.exit,
            })).collect::<Vec<_>>(),
            "rows": walked.rows,
            "load": crate::orchestration::desk::load_average().map(|reading| reading.one_minute),
        });
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&out)
            .expect("the rows' file");
        writeln!(file, "{row}").expect("a row");
        println!(
            "{label} {scenario} walk {walk}: {walk_ms:.0} ms, pressed {}, reached {:?}",
            walked.pressed, walked.reached
        );
    }
}

/// The road a measurement drives puts a typed value on the CLI's stdin and
/// nowhere on its argv; every other call is the walk's own argv.
#[test]
fn a_typed_value_goes_to_the_cli_on_stdin_and_never_on_its_argv() {
    let words =
        |list: &[&str]| -> Vec<String> { list.iter().map(|word| (*word).to_string()).collect() };
    let (argv, stdin) = cli_call(&words(&[
        "type",
        "browser-9",
        "#destination",
        TYPE_VALUE_FLAG,
        "London",
    ]));
    assert_eq!(
        argv,
        ["type", "browser-9", "#destination", TYPE_VALUE_STDIN_FLAG]
    );
    assert_eq!(stdin.as_deref(), Some("London"));
    assert!(!argv.iter().any(|word| word.contains("London")));
    let marks = words(&["marks", "browser-9", "--json"]);
    assert_eq!(cli_call(&marks), (marks.clone(), None));
}
