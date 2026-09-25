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
//! The running window's door may predate the one a build expects (t-9712): a
//! v1.1.25 window's `click --mark` answers without settling, and knows no
//! `--settle-later`. So the probe stands the door a build expects in front of
//! the window's: a press by number settles the page before it answers, by the
//! product's own loop and verdict (`settle_with`) with each poll one `eval` of
//! the door's own settle script; `--settle-later` presses, answers the page as
//! the press left it (`marks --json`), and holds the settle for the pane's next
//! `marks`, which finishes it the same way and says how. Each call's row says
//! what the stand-in door did inside it (`settleMs`, `previewMs`), apart.
//!
//! Lines between `// after-only {` and `// after-only }` need what only the
//! build after the change has; the driver drops them for the build before.

use std::io::Write as _;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Instant;

use serde_json::{Value, json};
use zerocode_core::agent_browser::{BROWSER_SETTLE_BUSY, TYPE_VALUE_FLAG, TYPE_VALUE_STDIN_FLAG};
use zerocode_core::branching::BranchAsk;
use zerocode_core::computer_recipe::RecipeTool;
use zerocode_core::jev::{BROWSER, JUDGMENT_CACHE, JevMode, Run};
use zerocode_core::screen_action::ActionAsk;
use zerocode_hookd::TeamAnswer;

use crate::cmd::browser::{
    BROWSER_OBSERVE_HELPERS, BROWSER_SETTLE_BODY, SettleReport, automation_script, settle_with,
};

use super::desk::{Aim, GoalWorld};
use super::live::{Doorway, LiveJudge};
use super::{
    ActionJudge, Branching, Compared, Done, Errand, Judged, Options, Pending, Seen, Spent, Why,
    run_with,
};

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
    let mut child = crate::proc::quiet_command(CLI)
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

/// A road call as the row keeps it, with what the stand-in door did inside it.
struct Call {
    verb: String,
    start_ms: f64,
    ms: f64,
    exit: i32,
    inside: Value,
}

/// The name the door keeps a settle's watch under on a page — the guest's
/// own slot name, read from the one file the window reads it from.
const WATCH: &str = include_str!("../../../../../ui/browser-guest-key.txt");

/// The epoch the stand-in settle names the press's document by: the first
/// document its polls saw — a later one is another document.
const PRESS_EPOCH: &str = "the-press";

/// Settle `pane` as the door has since t-6721 — the product's own loop and
/// verdict, each poll one `eval` of the door's own settle script. Stillness
/// counts from the first poll: the watch the press would have set before its
/// first event is set by that poll here, one CLI round later.
fn settle_by_eval(pane: &str) -> SettleReport {
    let script = automation_script(
        &json!({ "watch": WATCH.trim(), "busy": BROWSER_SETTLE_BUSY.join(",") }),
        &format!("{BROWSER_OBSERVE_HELPERS}\n{BROWSER_SETTLE_BODY}"),
    );
    let first: std::cell::RefCell<Option<String>> = std::cell::RefCell::new(None);
    let poll = |_left: std::time::Duration| {
        let (answer, _) = drive(&["eval".to_string(), pane.to_string(), script.clone()]);
        let said = if answer.exit_code == 0 {
            evaled(&answer.stdout).ok_or_else(|| "unreadable".to_string())
        } else {
            Err(answer.stderr)
        };
        let said = said.map(|mut said| {
            let seen = said
                .pointer("/value/documentEpoch")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let press = first
                .borrow_mut()
                .get_or_insert_with(|| seen.clone())
                .clone();
            if seen == press {
                said["value"]["documentEpoch"] = json!(PRESS_EPOCH);
            }
            said
        });
        std::future::ready(said)
    };
    tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("a runtime for the settle")
        .block_on(settle_with(PRESS_EPOCH, f64::NEG_INFINITY, poll))
}

/// What a settle came to, as the call's row keeps it.
fn settled_note(settle: &SettleReport) -> Value {
    json!({
        "settleMs": settle.ms,
        "settle": settle.state.word(),
        "settleWhy": settle.why.word(),
        "polls": settle.polls,
    })
}

/// The door a build expects, stood in front of the running window's: a
/// look, with the stand-in snapshot when the scenario needs it; a press by
/// number that settles before it answers; and — for a build that asks for
/// it — a press that leaves its settle for the pane's next look.
struct Door<'a> {
    pane: &'a str,
    script: Option<&'a str>,
    // after-only {
    /// Whether the last press left its settle for the next look.
    held: bool,
    // after-only }
}

impl Door<'_> {
    /// One look, with the stand-in snapshot merged in when asked for.
    fn look(&self, argv: &[String], inside: &mut Value) -> TeamAnswer {
        let (mut answer, _) = drive(argv);
        if let (Some(script), 0) = (self.script, answer.exit_code)
            && let Some((said, took)) = stand_in(self.pane, script, &answer.stdout)
        {
            answer.stdout = said;
            inside["standInMs"] = json!(took);
        }
        answer
    }

    /// One road call through the stand-in door, and what it did inside it.
    fn drive(&mut self, argv: &[String]) -> (TeamAnswer, Value) {
        let mut inside = json!({});
        let verb = argv.first().map(String::as_str);
        // after-only {
        if verb == Some("click")
            && argv.last().map(String::as_str)
                == Some(zerocode_core::agent_browser::BROWSER_SETTLE_LATER_FLAG)
        {
            let (pressed, _) = drive(&argv[..argv.len() - 1]);
            if pressed.exit_code != 0 {
                return (pressed, inside);
            }
            self.held = true;
            let began = Instant::now();
            let look = self.look(
                &[
                    "marks".to_string(),
                    self.pane.to_string(),
                    "--json".to_string(),
                ],
                &mut inside,
            );
            inside["previewMs"] = json!(began.elapsed().as_secs_f64() * 1_000.0);
            let mut said: Value = serde_json::from_str(look.stdout.trim()).unwrap_or(json!({}));
            said[crate::cmd::browser::CLICK_SAID_KEY] = json!(pressed.stdout.trim());
            return (
                TeamAnswer {
                    exit_code: 0,
                    stdout: said.to_string(),
                    stderr: String::new(),
                },
                inside,
            );
        }
        if verb == Some("marks") && std::mem::take(&mut self.held) {
            let settle = settle_by_eval(self.pane);
            inside = settled_note(&settle);
            let mut look = self.look(argv, &mut inside);
            if look.exit_code == 0
                && let Ok(mut said) = serde_json::from_str::<Value>(look.stdout.trim())
            {
                said[zerocode_core::agent_browser::BROWSER_SETTLE_KEY] =
                    zerocode_core::agent_browser::settle_said(settle.state, settle.why, settle.ms);
                look.stdout = said.to_string();
            }
            return (look, inside);
        }
        // after-only }
        match verb {
            Some("marks") => {
                let look = self.look(argv, &mut inside);
                (look, inside)
            }
            Some("click") if argv.get(2).map(String::as_str) == Some("--mark") => {
                let (pressed, _) = drive(argv);
                if pressed.exit_code == 0 {
                    inside = settled_note(&settle_by_eval(self.pane));
                }
                (pressed, inside)
            }
            _ => (drive(argv).0, inside),
        }
    }
}

/// A judge that counts the questions it was asked, in turn and ahead —
/// every request a walk spent, whatever became of its answer.
struct Counting<'a> {
    judge: &'a mut LiveJudge,
    asked: usize,
    begun: usize,
}

impl ActionJudge for Counting<'_> {
    fn choose(&mut self, ask: &ActionAsk) -> Judged {
        self.asked += 1;
        self.judge.choose(ask)
    }
    fn compare(&mut self, ask: &BranchAsk) -> Compared {
        self.judge.compare(ask)
    }
    fn begin(&mut self, ask: &ActionAsk) -> Option<Pending> {
        let begun = self.judge.begin(ask);
        self.begun += usize::from(begun.is_some());
        begun
    }
    fn finish(&mut self, done: Done) -> Judged {
        self.judge.finish(done)
    }
    fn choose_within(&mut self, ask: &ActionAsk, left: std::time::Duration) -> Judged {
        self.asked += 1;
        self.judge.choose_within(ask, left)
    }
    fn spent(&self) -> Option<Spent> {
        self.judge.spent()
    }
    fn cached(&self) -> bool {
        self.judge.cached()
    }
}

// after-only {
/// The value seat as a person who set its key has it: the key handed to this
/// command's environment, held in a store of the probe's own — never a
/// subscription login, never a keychain, never printed.
fn value_writer(key: &str) -> super::value::LiveWriter {
    use crate::api_routers::RouterKeys as _;
    let keys = crate::api_routers::HeldKeys::default();
    let row = zerocode_core::type_value::chosen().expect("a chosen row");
    let service = super::value::key_service(row).expect("its key's name");
    keys.write(&service, key).expect("the key held");
    super::value::LiveWriter::at(
        zerocode_core::type_value::ANTHROPIC_WIRE.url,
        Box::new(keys),
    )
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
    // `walk --overlap`: the walk asks ahead, and a page's world hands back
    // the screen its press left.
    let overlap = knob("ZEROCODE_WALK_PROBE_OVERLAP").is_some_and(|flag| flag == "1");
    // What says the page has loaded: a control of its own.
    let ready = knob("ZEROCODE_WALK_PROBE_READY").unwrap_or_else(|| "#search".to_string());

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
        let (waited, _) = drive(&["wait".to_string(), pane.clone(), ready.clone()]);
        assert_eq!(waited.exit_code, 0, "{}", waited.stderr);

        let began = Instant::now();
        let calls = std::cell::RefCell::new(Vec::<Call>::new());
        let mut door = Door {
            pane: &pane,
            script: script.as_deref(),
            // after-only {
            held: false,
            // after-only }
        };
        let mut road = |_: RecipeTool, argv: &[String], _: &[String]| {
            let start_ms = began.elapsed().as_secs_f64() * 1_000.0;
            let (answer, inside) = door.drive(argv);
            calls.borrow_mut().push(Call {
                verb: argv.first().cloned().unwrap_or_default(),
                start_ms,
                ms: began.elapsed().as_secs_f64() * 1_000.0 - start_ms,
                exit: answer.exit_code,
                inside,
            });
            answer
        };
        let at = Errand {
            goal: &goal,
            why: Why::Goal { steps },
            flow: None,
            moves_money: false,
        };
        let mut counting = Counting {
            judge: &mut judge,
            asked: 0,
            begun: 0,
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
            )
            .previewing(overlap);
            // after-only {
            let world = match knob("ZEROCODE_WALK_PROBE_VALUE_KEY") {
                Some(key) => world.writing(Box::new(value_writer(&key))),
                None => world,
            };
            // after-only }
            let mut world = world;
            run_with(
                JevMode::On,
                true,
                Branching::OFF,
                &at,
                &mut counting,
                &mut world,
                Options {
                    overlap,
                    rescue: false,
                    act_line: None,
                },
                None,
            )
        };
        let walk_ms = began.elapsed().as_secs_f64() * 1_000.0;
        let (asked, begun) = (counting.asked, counting.begun);
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
            "overlap": overlap,
            "asked": asked,
            "begun": begun,
            "overlapped": walked.overlapped,
            "discarded": walked.discarded,
            // after-only {
            "cancelled": walked.cancelled,
            // after-only }
            "calls": calls.into_inner().iter().map(|call| json!({
                "verb": call.verb, "startMs": call.start_ms, "ms": call.ms, "exit": call.exit,
                "inside": call.inside,
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
