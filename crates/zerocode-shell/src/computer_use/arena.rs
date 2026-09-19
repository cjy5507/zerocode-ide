//! `recipe-run --arena <evidence dir>` (docs/design/flow-engine-operator-and-qa.md
//! §3 — the arena): the one walk over a recorded folder instead of a desk.
//! Every step the walk asks is answered from the recording — the next
//! recorded line, when the words match in order, with the ok or the refusal
//! it recorded; a Flow's check line from the recorded baseline and
//! observations; a look (a verb the log keeps no line of) with an empty ok —
//! and a step the recording never made is refused by name
//! (`arena_off_script`), so a rehearsal never invents an outcome. The hand,
//! the pointer and the pages are the recording's: nobody's hand on the
//! pointer, no pictures, the pages the recorded fingerprint names. What the
//! walk leaves is its own — lines without frames and a walk record of kind
//! `arena`, in a folder of its own — so a guarded document's gates, ledger
//! and stops are rehearsed against a service nobody touches.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use zerocode_core::agent_browser;
use zerocode_core::computer_flow::{Check, FlowRecord, Observed, Presence};
use zerocode_core::computer_recipe::{RECIPE_CHECKS, RecipeTool, recipe_saved_words};
use zerocode_core::computer_use::{ComputerCommand, hold_of, parse_command, verb_method};
use zerocode_core::computer_use_protocol::error_code;
use zerocode_core::computer_use_protocol::frame::ShotFrame;
use zerocode_hookd::TeamAnswer;

use super::recipe_run::Desk;
use crate::agent_tools_runtime::{browser_refused, browser_said, computer_refused, computer_said};
use crate::cmd::browser::WAIT_TIMED_OUT;
use crate::run_evidence::{captures, redacted, steps_in, walks_in};

/// The word an arena walk's record carries as its `kind` — and the suffix of
/// the folder its evidence lands in (`evidence::arena_dir`).
pub const WALK_KIND: &str = "arena";
/// The scheme a recorded host is shown under as a page: the recording keeps
/// hosts (`Fingerprint`), a desk shows pages by URL.
const PAGE_SCHEME: &str = "https://";

/// One recorded step of the walk being rehearsed, in the recording's order:
/// its door, its words as the log kept them, and the refusal it recorded
/// (`(code, message)`) when it was refused.
struct Recorded {
    tool: RecipeTool,
    argv: Vec<String>,
    refusal: Option<(String, String)>,
}

/// Where a check ask stands in the walk: before its first step, a check is
/// the baseline's look; after, the oracle's.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Baseline,
    Oracle,
}

/// A recorded folder standing in for a desk: the recording's steps in
/// order, its Flow's stand and observations, and the pages its fingerprint
/// names.
pub struct Arena {
    from: PathBuf,
    script: Vec<Recorded>,
    at: usize,
    phase: Phase,
    checks: Vec<Check>,
    baseline: BTreeMap<usize, Presence>,
    observed: BTreeMap<usize, Observed>,
    pages: Vec<(String, String)>,
}

impl Arena {
    /// The arena `command` asks for, if it asks for one (`--arena <dir>`):
    /// read from that folder, its script from the command's `--start`.
    pub fn of(command: &ComputerCommand) -> Result<Option<Self>, String> {
        let Some(dir) = command.params.get("arena").and_then(Value::as_str) else {
            return Ok(None);
        };
        let start = command
            .params
            .get("start")
            .and_then(Value::as_u64)
            .and_then(|start| usize::try_from(start).ok())
            .unwrap_or(1);
        Self::read(Path::new(dir), start).map(Some)
    }

    /// The recording in `dir`: its last walk's record — the steps it ran,
    /// by the lines they left in the log, and its Flow's stand and
    /// observations — and the pages its fingerprint's host names, one per
    /// pane the recorded browser steps aimed at. `start` is the recipe step
    /// the rehearsal begins at: the script begins there too.
    pub fn read(dir: &Path, start: usize) -> Result<Self, String> {
        if !dir.is_dir() {
            return Err(format!("{} is not a folder", dir.display()));
        }
        let lines = steps_in(dir);
        let (_, walk) = walks_in(dir)
            .into_iter()
            .rev()
            .find(|(_, walk)| walk.get("ran").is_some_and(Value::is_array))
            .ok_or_else(|| {
                format!(
                    "no walk record in {}: an arena rehearses a recipe-run's own recording",
                    dir.display()
                )
            })?;
        let script: Vec<Recorded> = walk["ran"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|ran| {
                ran.get("step")
                    .and_then(Value::as_u64)
                    .and_then(|step| usize::try_from(step).ok())
                    .is_some_and(|step| step >= start)
            })
            .filter(|ran| {
                captures(
                    ran["tool"].as_str().unwrap_or_default(),
                    ran["verb"].as_str().unwrap_or_default(),
                )
                .is_some()
            })
            .map(|ran| {
                let line = crate::run_evidence::step_with_id(
                    &lines,
                    ran.get("evidence_id").and_then(Value::as_str),
                )
                .ok_or_else(|| {
                    format!(
                        "no unique recorded evidence for recipe step {}",
                        ran["step"]
                    )
                })?;
                let tool = RecipeTool::ALL
                    .into_iter()
                    .find(|tool| tool.as_str() == line.tool)
                    .ok_or_else(|| format!("unknown recorded tool: {}", line.tool))?;
                Ok(Recorded {
                    tool,
                    argv: line.argv.clone(),
                    refusal: (!line.ok).then(|| {
                        (
                            line.code
                                .clone()
                                .unwrap_or_else(|| error_code::UNANSWERED.to_string()),
                            line.error.clone().unwrap_or_default(),
                        )
                    }),
                })
            })
            .collect::<Result<_, String>>()?;
        let flow = walk
            .get("flow")
            .cloned()
            .and_then(|flow| serde_json::from_value::<FlowRecord>(flow).ok());
        let (checks, baseline, observed, hosts) = match flow {
            Some(flow) => (
                flow.spec.checks,
                flow.baseline,
                flow.observed,
                flow.spec.fingerprint.hosts,
            ),
            None => Default::default(),
        };
        let pages = hosts
            .iter()
            .next()
            .map(|host| {
                script
                    .iter()
                    .filter(|line| line.tool == RecipeTool::Browser)
                    .filter_map(|line| page_label(&line.argv))
                    .collect::<BTreeSet<String>>()
                    .into_iter()
                    .map(|label| (label, format!("{PAGE_SCHEME}{host}/")))
                    .collect()
            })
            .unwrap_or_default();
        Ok(Self {
            from: dir.to_path_buf(),
            script,
            at: 0,
            phase: Phase::Baseline,
            checks,
            baseline,
            observed,
            pages,
        })
    }

    /// The folder the recording was read from.
    #[must_use]
    pub fn from(&self) -> &Path {
        &self.from
    }

    /// The pages the recording's fingerprint names, `(label, url)`.
    #[must_use]
    pub fn pages(&self) -> Vec<(String, String)> {
        self.pages.clone()
    }

    /// The road a walk's steps take through this arena: the recording's
    /// answer, and — given the walk's folder — its line there.
    pub fn road<'a>(
        &'a mut self,
        dir: Option<&'a Path>,
    ) -> impl FnMut(RecipeTool, &[String], &[String]) -> TeamAnswer + 'a {
        move |tool, argv, logged| {
            let answer = self.answer(tool, argv, logged);
            if let Some(dir) = dir {
                crate::evidence_runtime::leave_arena_evidence(dir, tool.as_str(), logged, &answer);
            }
            answer
        }
    }

    /// What the recording answers a step `argv` (logged as `logged`): a
    /// verb the log keeps no line of, an empty ok — a look is nobody's
    /// record; the next recorded line, when its words match, as it
    /// answered; a Flow check line (matched by its words) from the recorded
    /// stand before the first step and from the observations after; and
    /// anything else off script, refused by name.
    pub fn answer(&mut self, tool: RecipeTool, argv: &[String], logged: &[String]) -> TeamAnswer {
        let verb = argv.first().map_or("", String::as_str);
        if captures(tool.as_str(), verb).is_none() {
            self.phase = Phase::Oracle;
            return recorded_answer(tool, None);
        }
        if self.next_is(tool, logged) {
            let line = &self.script[self.at];
            self.at += 1;
            self.phase = Phase::Oracle;
            return recorded_answer(tool, line.refusal.as_ref());
        }
        if check_verb(tool, verb)
            && let Some(check) = self
                .checks
                .iter()
                .find(|check| check.tool == tool && same_check(tool, argv, &check.argv))
        {
            let seen = match self.phase {
                Phase::Baseline => self
                    .baseline
                    .get(&check.id)
                    .map(|stand| Observed::Seen(*stand))
                    .or_else(|| self.observed.get(&check.id).cloned()),
                Phase::Oracle => self.observed.get(&check.id).cloned(),
            };
            if let Some(seen) = seen {
                return presence_answer(tool, verb, &seen);
            }
        }
        let why = self.off_script(logged);
        match tool {
            RecipeTool::Computer | RecipeTool::Emulator => {
                refused_answer(error_code::ARENA_OFF_SCRIPT, &why)
            }
            RecipeTool::Browser => browser_refused_answer(&why),
        }
    }

    /// Whether the next recorded line is `logged`'s: the same door, the same
    /// recipe words — the log's redaction and a walk's own flags left out,
    /// so a recording from another build's walk still reads.
    fn next_is(&self, tool: RecipeTool, logged: &[String]) -> bool {
        self.script.get(self.at).is_some_and(|line| {
            line.tool == tool
                && recipe_saved_words(&line.argv)
                    == recipe_saved_words(&redacted(tool.as_str(), logged))
        })
    }

    /// The refusal's sentence: the step asked, and the line the recording
    /// holds instead.
    fn off_script(&self, logged: &[String]) -> String {
        let held = self.script.get(self.at).map_or_else(
            || "its lines are all walked".to_string(),
            |line| format!("line {} is `{}`", self.at + 1, line.argv.join(" ")),
        );
        format!(
            "{}: `{}` is not what the recording holds next — {held}",
            error_code::ARENA_OFF_SCRIPT,
            logged.join(" ")
        )
    }
}

/// The line a person's step leaves in an arena's folder (`run`'s `leave`):
/// the stop as the walk answered it, no frame.
pub fn leave(dir: Option<&Path>, logged: &[String], answer: &TeamAnswer) {
    if let Some(dir) = dir {
        crate::evidence_runtime::leave_arena_evidence(
            dir,
            RecipeTool::Computer.as_str(),
            logged,
            answer,
        );
    }
}

/// The pane a recorded browser line aimed at, when it named one: the word
/// after the verb, for every verb but the one that opens a pane by URL.
fn page_label(argv: &[String]) -> Option<String> {
    let verb = argv.first()?;
    (verb != "open" && agent_browser::browser_verb(verb).is_some_and(|row| row.arity.min >= 1))
        .then(|| argv.get(1).cloned())
        .flatten()
}

/// Whether a verb's answer is a check a Flow judges, by its door's table.
fn check_verb(tool: RecipeTool, verb: &str) -> bool {
    match tool {
        RecipeTool::Computer => {
            verb_method(verb).is_some_and(|method| RECIPE_CHECKS.contains(&method))
        }
        RecipeTool::Browser => agent_browser::is_check(verb),
        RecipeTool::Emulator => zerocode_core::agent_emulator::is_check(verb),
    }
}

/// Whether an asked check line is a recorded one: the same command, its
/// wait left out — the walk asks a check with its own budget (the stand
/// with none, the oracle with the verify budget), the document wrote it
/// with another or none. A browser check keeps its verb, pane and subject.
fn same_check(tool: RecipeTool, asked: &[String], recorded: &[String]) -> bool {
    match tool {
        RecipeTool::Emulator => {
            let normalized = |argv: &[String]| {
                let mut command = zerocode_core::computer_use::parse_emulator_command(argv).ok()?;
                command.json = false;
                Some(command)
            };
            normalized(asked).is_some_and(|asked| Some(asked) == normalized(recorded))
        }
        RecipeTool::Computer => {
            let without_wait = |argv: &[String]| {
                let command = parse_command(argv).ok()?;
                let mut params = command.params;
                if let Some(param) = hold_of(command.method)
                    .and_then(|hold| hold.asked_by)
                    .map(|asked_by| asked_by.param)
                    && let Some(params) = params.as_object_mut()
                {
                    params.remove(param);
                }
                Some((command.method, params))
            };
            without_wait(asked).is_some_and(|asked| Some(asked) == without_wait(recorded))
        }
        RecipeTool::Browser => {
            !asked.is_empty()
                && !recorded.is_empty()
                && check_subject(asked) == check_subject(recorded)
        }
    }
}

/// A browser check's verb, pane and subject — the words its table counts
/// as the least it takes, its wait left out.
fn check_subject(argv: &[String]) -> &[String] {
    let words = agent_browser::browser_verb(&argv[0]).map_or(argv.len(), |row| row.arity.min + 1);
    &argv[..words.min(argv.len())]
}

/// The desktop envelope and the browser sentence a recorded step answered
/// with, or the refusal it recorded.
fn recorded_answer(tool: RecipeTool, refusal: Option<&(String, String)>) -> TeamAnswer {
    match (tool, refusal) {
        (RecipeTool::Computer | RecipeTool::Emulator, None) => said_answer(json!({})),
        (RecipeTool::Computer | RecipeTool::Emulator, Some((code, message))) => {
            refused_answer(code, message)
        }
        (RecipeTool::Browser, None) => browser_said_answer("ok"),
        (RecipeTool::Browser, Some((_, message))) => browser_refused_answer(message),
    }
}

/// What a check's door answers for a recorded presence — the reading
/// `presence_of` makes, inverted: a desktop check ok when its subject was
/// there and `timeout` when not, a browser `wait` likewise by its exit and
/// its named refusal, a `find` the count.
fn presence_answer(tool: RecipeTool, verb: &str, seen: &Observed) -> TeamAnswer {
    match (tool, seen) {
        (RecipeTool::Emulator, Observed::Seen(presence)) => said_answer(json!({
            "count": presence.count.unwrap_or(usize::from(presence.present))
        })),
        (RecipeTool::Computer, Observed::Seen(presence)) if presence.present => {
            said_answer(json!({}))
        }
        (RecipeTool::Computer, Observed::Seen(_)) => {
            refused_answer(error_code::TIMEOUT, "the recording saw nothing there")
        }
        (RecipeTool::Computer | RecipeTool::Emulator, Observed::NotEvaluable(why)) => {
            refused_answer(error_code::UNANSWERED, why)
        }
        (RecipeTool::Browser, Observed::Seen(presence)) if verb == "find" => browser_said_answer(
            &json!({ "count": presence.count.unwrap_or(usize::from(presence.present)) })
                .to_string(),
        ),
        (RecipeTool::Browser, Observed::Seen(presence)) if presence.present => {
            browser_said_answer("ok")
        }
        (RecipeTool::Browser, Observed::Seen(_)) => browser_refused_answer(WAIT_TIMED_OUT),
        (RecipeTool::Browser, Observed::NotEvaluable(why)) => browser_refused_answer(why),
    }
}

/// A desktop envelope answered, as the shim prints one.
fn said_answer(result: Value) -> TeamAnswer {
    computer_said(format!("{}\n", json!({ "ok": true, "result": result })))
}

/// A desktop refusal's envelope, as the shim prints one.
fn refused_answer(code: &str, message: &str) -> TeamAnswer {
    computer_refused(
        json!({ "ok": false, "error": { "code": code, "message": message } }).to_string(),
    )
}

/// The browser door's sentence, and its refusal in the door's own words.
fn browser_said_answer(said: &str) -> TeamAnswer {
    browser_said(format!("{said}\n"))
}

fn browser_refused_answer(why: &str) -> TeamAnswer {
    browser_refused(format!("{}: {why}\n", RecipeTool::Browser.command_word()))
}

/// The desk an arena stands in for: nobody's hand on the pointer, no
/// pictures (nothing landed anywhere), no pause (nothing paints), the
/// clock's own time, and the pages the recording names.
pub struct ArenaDesk {
    began: Instant,
    pages: Vec<(String, String)>,
}

impl ArenaDesk {
    #[must_use]
    pub fn new(pages: Vec<(String, String)>) -> Self {
        Self {
            began: Instant::now(),
            pages,
        }
    }
}

impl Desk for ArenaDesk {
    fn pointer(&mut self) -> Option<(f64, f64)> {
        None
    }

    fn picture(&mut self, _: [f64; 4]) -> Option<(Vec<u8>, ShotFrame)> {
        None
    }

    fn changed(&self, _: &[u8], _: &[u8], _: ShotFrame) -> Option<bool> {
        None
    }

    fn elapsed_ms(&self) -> u64 {
        u64::try_from(self.began.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    fn now_epoch_ms(&self) -> i64 {
        crate::project_runtime::now_epoch_ms()
    }

    fn pause(&mut self, _: Duration) {}

    fn pages(&mut self) -> Vec<(String, String)> {
        self.pages.clone()
    }
}

/// Where a walk's desks come from: one per round, fresh — a closure that
/// makes one. Named so a caller's signature need not name the desk.
pub trait DeskOf {
    type Desk: Desk;
    fn desk(&mut self) -> Self::Desk;
}

impl<D: Desk, F: FnMut() -> D> DeskOf for F {
    type Desk = D;
    fn desk(&mut self) -> D {
        self()
    }
}

/// The desk a recipe walk stands on: the live one, or an arena's.
pub enum Stage<D> {
    Live(D),
    Arena(ArenaDesk),
}

impl<D: Desk> Desk for Stage<D> {
    fn pointer(&mut self) -> Option<(f64, f64)> {
        match self {
            Self::Live(desk) => desk.pointer(),
            Self::Arena(desk) => desk.pointer(),
        }
    }

    fn picture(&mut self, region: [f64; 4]) -> Option<(Vec<u8>, ShotFrame)> {
        match self {
            Self::Live(desk) => desk.picture(region),
            Self::Arena(desk) => desk.picture(region),
        }
    }

    fn changed(&self, before: &[u8], after: &[u8], frame: ShotFrame) -> Option<bool> {
        match self {
            Self::Live(desk) => desk.changed(before, after, frame),
            Self::Arena(desk) => desk.changed(before, after, frame),
        }
    }

    fn elapsed_ms(&self) -> u64 {
        match self {
            Self::Live(desk) => desk.elapsed_ms(),
            Self::Arena(desk) => desk.elapsed_ms(),
        }
    }

    fn now_epoch_ms(&self) -> i64 {
        match self {
            Self::Live(desk) => desk.now_epoch_ms(),
            Self::Arena(desk) => desk.now_epoch_ms(),
        }
    }

    fn pause(&mut self, pause: Duration) {
        match self {
            Self::Live(desk) => desk.pause(pause),
            Self::Arena(desk) => desk.pause(pause),
        }
    }

    fn pages(&mut self) -> Vec<(String, String)> {
        match self {
            Self::Live(desk) => desk.pages(),
            Self::Arena(desk) => desk.pages(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::recipe_run::bench::*;
    use super::super::recipe_run::{Desk, run};
    use super::*;
    use serde_json::{Value, json};
    use std::collections::{BTreeMap, BTreeSet};
    use zerocode_core::computer_flow::{
        Check, CheckKind, EvidenceLevel, Fingerprint, FlowRecord, FlowSpec, Observed, Policy,
        Presence, judge,
    };
    use zerocode_core::computer_recipe::RecipeTool;
    use zerocode_core::computer_use::COMPUTER_USE_PROTOCOL_VERSION;
    use zerocode_core::computer_use_protocol::error_code;

    /// A recorded line: its door, its words as the document says them, and
    /// whether it was answered or refused (by code).
    type Line<'a> = (RecipeTool, Vec<String>, Result<(), &'a str>);

    fn refusal(code: &str) -> String {
        json!({ "ok": false, "error": { "code": code, "message": "no" } }).to_string()
    }

    /// A recorded folder: the lines as the live road left them (no frames),
    /// and the walk's record over them — every line a step of the walk, in
    /// order — with its Flow when it had one.
    fn recorded(dir: &std::path::Path, lines: &[Line<'_>], flow: Option<&FlowRecord>) {
        let mut ran = Vec::new();
        for (at, (tool, argv, outcome)) in lines.iter().enumerate() {
            let refused = outcome.err().map(refusal);
            crate::run_evidence::record_measured(
                dir,
                (
                    1_000 + at as i64,
                    Some(json!({"evidence_id": format!("fixture-{at}")})),
                ),
                tool.as_str(),
                argv,
                refused.as_deref().map_or(Ok(()), Err),
                crate::run_evidence::Framing::None,
            );
            ran.push(json!({
                "step": at + 1, "shown": at + 1, "tool": tool.as_str(), "verb": argv[0],
                "ok": outcome.is_ok(), "ms": 1, "evidence_id": format!("fixture-{at}"),
            }));
        }
        let mut walk = json!({
            "kind": "recipe-run", "name": "test", "file": "f.md", "start": 1,
            "steps": lines.len(), "at_epoch_ms": 1_000, "elapsedMs": 9,
            "done": lines.iter().all(|(_, _, outcome)| outcome.is_ok()), "ran": ran,
        });
        if let Some(flow) = flow {
            walk["flow"] = serde_json::to_value(flow).unwrap();
        }
        crate::run_evidence::record_walk(dir, &walk).unwrap();
    }

    #[test]
    fn flow_retry_arena_refuses_a_missing_record_instead_of_shifting_identical_commands() {
        let dir = tempfile::tempdir().unwrap();
        let lines = vec![
            (
                RecipeTool::Computer,
                words(&["key", "--key", "tab"]),
                Ok(())
            );
            2
        ];
        recorded(dir.path(), &lines, None);
        let steps = steps_in(dir.path());
        // Only the second of two identical calls survived its append.
        std::fs::write(
            dir.path().join(crate::run_evidence::STEPS_FILE),
            format!("{}\n", serde_json::to_string(&steps[1]).unwrap()),
        )
        .unwrap();
        assert!(Arena::read(dir.path(), 1).is_err());
        assert!(Arena::read(dir.path(), 2).is_ok());
    }

    fn present(count: Option<usize>) -> Presence {
        Presence {
            present: true,
            count,
        }
    }

    fn absent() -> Presence {
        Presence {
            present: false,
            count: None,
        }
    }

    fn check(id: usize, kind: CheckKind, tool: RecipeTool, line: &[&str]) -> Check {
        Check {
            id,
            kind,
            required: true,
            tool,
            argv: words(line),
        }
    }

    /// A Flow bound to one app and one host, its checks as given, judged as
    /// the recording observed them.
    fn flow_record(
        checks: Vec<Check>,
        baseline: BTreeMap<usize, Presence>,
        observed: BTreeMap<usize, Observed>,
    ) -> FlowRecord {
        let verdict = judge(&checks, &baseline, &observed);
        FlowRecord {
            spec: FlowSpec {
                money: None,
                confirm: Default::default(),
                policy: Policy::Dry,
                evidence: EvidenceLevel::Off,
                fingerprint: Fingerprint {
                    apps: ["X".to_string()].into_iter().collect(),
                    hosts: ["stg.example".to_string()].into_iter().collect(),
                    protocol: COMPUTER_USE_PROTOCOL_VERSION,
                    assets: BTreeSet::new(),
                },
                checks,
                trigger: None,
            },
            baseline,
            observed,
            verdict,
        }
    }

    #[test]
    fn emulator_checks_arena_answers_the_recorded_baseline_and_final_count() {
        let dir = tempfile::tempdir().unwrap();
        let query = [
            "find",
            "--platform",
            "ios",
            "--device",
            "phone",
            "--text",
            "Done",
        ];
        let flow = flow_record(
            vec![check(1, CheckKind::Event, RecipeTool::Emulator, &query)],
            BTreeMap::from([(
                1,
                Presence {
                    present: false,
                    count: Some(0),
                },
            )]),
            BTreeMap::from([(1, Observed::Seen(present(Some(1))))]),
        );
        let step = [
            "button",
            "--platform",
            "ios",
            "--device",
            "phone",
            "--name",
            "home",
        ];
        recorded(
            dir.path(),
            &[(RecipeTool::Emulator, words(&step), Ok(()))],
            Some(&flow),
        );
        let mut arena = Arena::read(dir.path(), 1).unwrap();
        let mut asked = words(&query);
        asked.push("--json".into());
        let before = arena.answer(RecipeTool::Emulator, &asked, &asked);
        assert_eq!(
            serde_json::from_str::<Value>(&before.stdout).unwrap()["result"]["count"],
            0
        );
        arena.answer(RecipeTool::Emulator, &words(&step), &words(&step));
        let after = arena.answer(RecipeTool::Emulator, &asked, &asked);
        assert_eq!(
            serde_json::from_str::<Value>(&after.stdout).unwrap()["result"]["count"],
            1
        );
    }

    #[test]
    fn emulator_checks_replay_counts_and_match_the_json_transport() {
        for (verb, flag, subject) in [
            ("find", "--text", "Done"),
            ("foreground", "--app", "com.example.wallet"),
        ] {
            let recorded = words(&[
                verb,
                "--platform",
                "android",
                "--device",
                "phone",
                flag,
                subject,
            ]);
            let mut asked = recorded.clone();
            asked.push("--json".into());
            assert!(check_verb(RecipeTool::Emulator, verb));
            assert!(same_check(RecipeTool::Emulator, &asked, &recorded));
            asked[6] = "Another".into();
            assert!(!same_check(RecipeTool::Emulator, &asked, &recorded));
            for count in [0, 2] {
                let answer = presence_answer(
                    RecipeTool::Emulator,
                    verb,
                    &Observed::Seen(Presence {
                        present: count > 0,
                        count: Some(count),
                    }),
                );
                assert_eq!(answer.exit_code, 0);
                assert_eq!(
                    serde_json::from_str::<Value>(&answer.stdout).unwrap()["result"]["count"],
                    count
                );
            }
        }
    }

    const KEY: [&str; 3] = ["key", "--key", "tab"];
    const TYPE: [&str; 3] = ["type", "--text", "{{name}}"];
    const SEND: [&str; 3] = ["click", "browser-1", "#send"];
    const MISSING: [&str; 5] = ["click", "--app", "X", "--text", "Send"];
    const DONE: [&str; 5] = ["wait-for", "--app", "X", "--text", "Done"];
    const RECEIPT: [&str; 3] = ["find", "browser-1", "RCPT-"];

    /// The recording every test here rehearses: three acts answered, a
    /// fourth refused, an event line that flipped and a state line seen.
    fn recording(dir: &std::path::Path) -> FlowRecord {
        let flow = flow_record(
            vec![
                check(1, CheckKind::Event, RecipeTool::Computer, &DONE),
                check(2, CheckKind::State, RecipeTool::Browser, &RECEIPT),
            ],
            BTreeMap::from([(1, absent())]),
            BTreeMap::from([
                (1, Observed::Seen(present(None))),
                (2, Observed::Seen(present(Some(1)))),
            ]),
        );
        recorded(
            dir,
            &[
                (RecipeTool::Computer, words(&KEY), Ok(())),
                (RecipeTool::Computer, words(&TYPE), Ok(())),
                (RecipeTool::Browser, words(&SEND), Ok(())),
                (
                    RecipeTool::Computer,
                    words(&MISSING),
                    Err(error_code::ELEMENT_NOT_FOUND),
                ),
            ],
            Some(&flow),
        );
        flow
    }

    fn line(tool: RecipeTool, argv: &[&str]) -> (RecipeTool, String) {
        (tool, argv.join(" "))
    }

    /// The walk of `text` from `command` over `arena`, with no live road.
    fn rehearse(arena: &mut Arena, command: &ComputerCommandOf, text: &str) -> Value {
        let desk = ArenaDesk::new(arena.pages());
        run(
            &walk_of(command, text),
            |tool, argv, logged| arena.answer(tool, argv, logged),
            |_, _, _| {},
            |_| {},
            desk,
            || true,
        )
        .expect("walked")
    }

    type ComputerCommandOf = zerocode_core::computer_use::ComputerCommand;

    #[test]
    fn an_arena_answers_each_step_from_the_recorded_folder_and_refuses_off_script() {
        let dir = tempfile::tempdir().unwrap();
        let flow = recording(dir.path());
        let lines = [
            line(RecipeTool::Computer, &KEY),
            line(RecipeTool::Computer, &TYPE),
            line(RecipeTool::Browser, &SEND),
            line(RecipeTool::Computer, &MISSING),
        ];
        let lines: Vec<(RecipeTool, &str)> = lines
            .iter()
            .map(|(tool, text)| (*tool, text.as_str()))
            .collect();
        let text = flow_doc(&lines, &flow.spec);
        let walk_command = command(&["--params", r#"{"name":"joe"}"#]);
        let mut arena = Arena::read(dir.path(), 1).expect("a recording");
        assert_eq!(arena.from(), dir.path());
        let report = rehearse(&mut arena, &walk_command, &text);
        // Each act answered as recorded: three ok, the fourth refused by the
        // code it recorded, and the walk stopped there as it did.
        let ran = report["ran"].as_array().unwrap();
        assert_eq!(ran.len(), 4, "{report}");
        assert!(
            ran[..3].iter().all(|step| step["ok"] == json!(true)),
            "{report}"
        );
        assert_eq!(report["stop"]["code"], json!(error_code::ELEMENT_NOT_FOUND));
        assert_eq!(report["stoppedAt"], json!(4));
        // Each check answered from the record: the event's stand, then what
        // was observed — and judged afresh to the recorded verdict.
        assert_eq!(report["flow"]["baseline"]["1"]["present"], json!(false));
        assert_eq!(
            report["flow"]["observed"]["1"],
            json!({ "seen": { "present": true, "count": null } })
        );
        assert_eq!(
            report["flow"]["observed"]["2"],
            json!({ "seen": { "present": true, "count": 1 } })
        );
        assert_eq!(report["flow"]["verdict"]["pass"], json!(true));
        assert_eq!(report["handWatched"], json!(false), "nobody's hand");
        assert!(
            ran.iter().all(|step| step["landed"].is_null()),
            "nothing is looked at in pixels: {report}"
        );

        // Off script: a step the recording never made is refused by name,
        // and the refusal names the recorded line it was not.
        let mut swapped = lines.clone();
        swapped[1] = (RecipeTool::Computer, "key --key enter");
        let text = flow_doc(&swapped, &flow.spec);
        let mut arena = Arena::read(dir.path(), 1).unwrap();
        let report = rehearse(&mut arena, &walk_command, &text);
        assert_eq!(
            report["stop"]["code"],
            json!(error_code::ARENA_OFF_SCRIPT),
            "{report}"
        );
        assert_eq!(report["stoppedAt"], json!(2));
        let said = report["stop"]["message"].as_str().unwrap_or_default();
        assert!(said.contains("type --text {{name}}"), "{said}");

        // A look the log keeps no line of is answered empty and the script
        // stands; a check the record never asked is not evaluable.
        let mut looked = lines.clone();
        looked.insert(0, (RecipeTool::Computer, "observe --app X"));
        let mut other = flow.spec.clone();
        other.checks[0].argv = words(&["wait-for", "--app", "X", "--text", "Other"]);
        let text = flow_doc(&looked, &other);
        let mut arena = Arena::read(dir.path(), 1).unwrap();
        let report = rehearse(&mut arena, &walk_command, &text);
        assert_eq!(report["ran"][0]["ok"], json!(true), "{report}");
        assert_eq!(report["ran"][1]["ok"], json!(true));
        assert_eq!(report["stoppedAt"], json!(5));
        let unasked = report["flow"]["observed"]["1"]["not-evaluable"]
            .as_str()
            .unwrap_or_default();
        assert!(unasked.contains(error_code::ARENA_OFF_SCRIPT), "{report}");

        // `--start` aligns the script: the walk from step three asks the
        // browser click first, and gets its line.
        let text = flow_doc(&lines, &flow.spec);
        let from_three = command(&["--start", "3", "--params", r#"{"name":"joe"}"#]);
        let mut arena = Arena::read(dir.path(), 3).unwrap();
        let report = rehearse(&mut arena, &from_three, &text);
        assert_eq!(report["ran"][0]["verb"], json!("click"));
        assert_eq!(report["ran"][0]["ok"], json!(true), "{report}");
        assert_eq!(report["stop"]["code"], json!(error_code::ELEMENT_NOT_FOUND));

        // The command names the arena and where to start.
        let asked = command(&["--arena", &dir.path().display().to_string(), "--start", "3"]);
        let arena = Arena::of(&asked).expect("readable").expect("an arena");
        assert_eq!(arena.from(), dir.path());
        assert!(Arena::of(&command(&[])).unwrap().is_none());
        assert!(Arena::of(&command(&["--arena", "/nowhere/at/all"])).is_err());
        let empty = tempfile::tempdir().unwrap();
        assert!(
            Arena::read(empty.path(), 1).is_err(),
            "a folder without a walk's record is no arena"
        );
    }

    /// A guarded document — a recipient typed, an amount typed, the send
    /// pressed, the completion seen — rehearsed from its recording with no
    /// desk at all: no pointer, no pictures, the pages the recording's
    /// fingerprint names; the verdict is the recording's, every time. (Under
    /// `policy: dry` until the guarded contracts land.)
    #[test]
    fn an_arena_rehearses_a_guarded_document_without_touching_a_desk() {
        const RECIPIENT: [&str; 4] = ["type", "browser-1", "#recipient", "{{recipient}}"];
        const AMOUNT: [&str; 4] = ["type", "browser-1", "#amount", "{{amount}}"];
        const COMPLETE: [&str; 3] = ["find", "browser-1", "송금이 완료되었습니다"];
        let dir = tempfile::tempdir().unwrap();
        let flow = flow_record(
            vec![check(1, CheckKind::Event, RecipeTool::Browser, &COMPLETE)],
            BTreeMap::from([(1, present(Some(0)))]),
            BTreeMap::from([(1, Observed::Seen(present(Some(1))))]),
        );
        recorded(
            dir.path(),
            &[
                (RecipeTool::Browser, words(&RECIPIENT), Ok(())),
                (RecipeTool::Browser, words(&AMOUNT), Ok(())),
                (RecipeTool::Browser, words(&SEND), Ok(())),
            ],
            Some(&flow),
        );
        let lines = [
            line(RecipeTool::Browser, &RECIPIENT),
            line(RecipeTool::Browser, &AMOUNT),
            line(RecipeTool::Browser, &SEND),
        ];
        let lines: Vec<(RecipeTool, &str)> = lines
            .iter()
            .map(|(tool, text)| (*tool, text.as_str()))
            .collect();
        let text = flow_doc(&lines, &flow.spec);
        let walk_command = command(&["--params", r#"{"recipient":"ACME","amount":"12000"}"#]);
        for _ in 0..2 {
            let mut arena = Arena::read(dir.path(), 1).unwrap();
            let mut desk = ArenaDesk::new(arena.pages());
            assert_eq!(desk.pointer(), None, "nobody's hand");
            assert!(desk.picture([0.0, 0.0, 10.0, 10.0]).is_none(), "no screen");
            assert_eq!(
                desk.pages(),
                vec![("browser-1".to_string(), "https://stg.example/".to_string())],
                "the pages the recording's fingerprint names"
            );
            let report = rehearse(&mut arena, &walk_command, &text);
            assert_eq!(report["done"], json!(true), "{report}");
            assert_eq!(report["flow"]["verdict"]["pass"], json!(true), "{report}");
            assert_eq!(report["flow"]["baseline"]["1"]["count"], json!(0));
            assert_eq!(report["handWatched"], json!(false));
            for step in report["ran"].as_array().unwrap() {
                assert_eq!(step["ok"], json!(true), "{step}");
                assert_eq!(step["phases"]["settle"], json!(0), "{step}");
                assert_eq!(step["phases"]["pointer"], json!(0), "{step}");
            }
        }
        assert_eq!(WALK_KIND, "arena");
    }

    /// Twenty recorded steps rehearsed through the one walk, nobody's hand
    /// on the pointer — printed, not checked.
    #[test]
    #[ignore = "a measurement, printed; not a check"]
    fn measure_arena_twenty_steps_ms() {
        const STEPS: usize = 20;
        const RUNS: usize = 20;
        let dir = tempfile::tempdir().unwrap();
        let steps: Vec<Vec<String>> = (0..STEPS)
            .map(|at| words(&["key", "--key", &format!("f{}", at + 1)]))
            .collect();
        let lines: Vec<Line<'_>> = steps
            .iter()
            .map(|argv| (RecipeTool::Computer, argv.clone(), Ok(())))
            .collect();
        recorded(dir.path(), &lines, None);
        let text = doc(&steps
            .iter()
            .map(|argv| argv.join(" "))
            .collect::<Vec<_>>()
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>());
        let walk_command = command(&[]);
        let mut runs: Vec<f64> = (0..RUNS)
            .map(|_| {
                let mut arena = Arena::read(dir.path(), 1).unwrap();
                let started = std::time::Instant::now();
                let report = rehearse(&mut arena, &walk_command, &text);
                let took = started.elapsed().as_secs_f64() * 1_000.0;
                assert_eq!(report["done"], json!(true));
                took
            })
            .collect();
        runs.sort_by(|a, b| a.partial_cmp(b).unwrap());
        println!(
            "measure: arena steps={STEPS} walk_ms_median={:.3} walk_ms_min={:.3} ({})",
            runs[runs.len() / 2],
            runs[0],
            if cfg!(debug_assertions) {
                "debug"
            } else {
                "release"
            }
        );
    }
}
