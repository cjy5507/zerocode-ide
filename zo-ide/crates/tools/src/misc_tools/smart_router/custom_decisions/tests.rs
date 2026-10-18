//! A person's own decisions held to their contract on a fake wire, at each of
//! zo's four points. One that records asks, keeps its row and hands nothing;
//! one that acts hands its one line only when it is as sure as its act-from
//! line and alone at the top; a command it is asked before runs whatever it
//! answers. Nothing leaves outside the door's consent, a decision's folders or
//! its hour, and no secret line leaves at all. With no decision, or with
//! every one off, nothing is asked or written and nothing a turn says
//! changes, and a folder's own store or a damaged one asks nothing.

use std::collections::BTreeMap;
use std::fs;

use api::sync_bridge::run_blocking;
use runtime::skill_rank::SkillSuggestionSeat;
use runtime::tool_guard::SHELL_TOOL;
use runtime::{
    ApiRequest, AssistantEvent, CommandRan, ContentBlock, ConversationMessage, ConversationRuntime, MessageRole,
    PermissionMode, PermissionPolicy, RuntimeError, Session, StaticToolExecutor, ToolGuardSeat,
};
use zerocode_core::jev::custom::{Field, Folders, Refusal};
use zerocode_core::jev::door::{CUSTOM_HOURLY_REQUESTS_SETTING, JEV_SETTINGS_KEY};
use zerocode_core::jev::{COMMAND_GUARD, PROJECT_RULES, SKILL_SUGGESTION, SMART_SETTINGS_KEY, TOOL_TEXT_GUARD};

use super::super::jev_mock::{machine_words, Mock};
use super::super::project_rules::{note_project_rule_turn, pending_project_rule_advice, project_rule_advice_delivered};
use super::super::shadow_ledger::read_shadow_rows;
use super::super::skill_search::SkillSuggestionJudge;
use super::super::tool_guard::ToolGuardJudge;
use super::*;

/// What the person asked for, as a turn's start lends it.
pub(crate) const TASK: &str = "write the release notes";

/// A credential, as one line of what a point lends or a definition holds.
const SECRET: &str = "sk-live-4242";

/// An option's name the door's detector reads as a credential.
const SECRET_OPTION: &str = "password";

/// How long a test that says nothing was asked waits before it looks: long
/// enough for a request a point sent off on a worker to reach a mock on this
/// machine's own loopback, so that a request sent late still counts.
const LATE: Duration = Duration::from_millis(500);

/// How long a test waits for what a worker writes.
const WORKER_WAIT: Duration = Duration::from_secs(10);

/// The words a test's machine holds: the switch over every decision as
/// `switch` says, when it says anything, and the uses already asking at a
/// decision's points off, so that all a point does is the decisions'.
pub(crate) fn words(switch: Option<&'static str>) -> Vec<(&'static str, &'static str)> {
    let off = JevMode::Off.key();
    let mut words = vec![
        (SKILL_SUGGESTION.setting, off),
        (COMMAND_GUARD.setting, off),
        (TOOL_TEXT_GUARD.setting, off),
        (PROJECT_RULES.setting, off),
    ];
    words.extend(switch.map(|word| (CUSTOM_DECISIONS.setting, word)));
    words
}

/// The reply to `request`: `answer` of each question it asked, by its id.
pub(crate) fn reply_to(request: &str, answer: &impl Fn(&str) -> Value) -> String {
    let body: Value = serde_json::from_str(request).unwrap_or_default();
    let answers: Map<String, Value> = body["questions"]
        .as_object()
        .into_iter()
        .flat_map(Map::keys)
        .map(|id| (id.clone(), answer(id)))
        .collect();
    json!({"model": "jev-test", "answers": answers, "usage": {"input_tokens": 20, "output_tokens": 0}}).to_string()
}

/// A mock that answers every question with `answer` of its id.
pub(crate) fn replying(answer: impl Fn(&str) -> Value + Send + Sync + 'static) -> Mock {
    Mock::answering(move |request| (200, reply_to(request, &answer)))
}

/// A yes-or-no answer: its probability of yes.
pub(crate) fn a_noul(yes: f64) -> Value {
    json!({"type": "noul", "noul": yes})
}

/// A choice of `chosen`, with every option's probability and how sure.
fn a_choice(chosen: &str, spread: &[(&str, f64)], confidence: f64) -> Value {
    let probabilities: Map<String, Value> =
        spread.iter().map(|(option, probability)| ((*option).to_string(), json!(probability))).collect();
    json!({"type": "choice", "choice": chosen, "confidence": confidence, "probabilities": probabilities})
}

/// A score whose levels, lowest first, are as likely as `spread` says, its
/// score their weighted mean as the wire writes it.
fn a_score(spread: &[f64], confidence: f64) -> Value {
    let (mut score, mut number) = (0.0, 0.0);
    let (mut probabilities, mut legend) = (Map::new(), Map::new());
    for (level, probability) in spread.iter().enumerate() {
        score += probability * number;
        number += 1.0;
        probabilities.insert(level.to_string(), json!(probability));
        legend.insert(level.to_string(), json!(format!("level {level}")));
    }
    json!({"type": "score", "score": score, "confidence": confidence, "legend": legend, "probabilities": probabilities})
}

/// A yes-or-no decision named `name` asking at `at`, as a new one starts
/// ([`Definition::new`]).
pub(crate) fn yes_no(name: &str, at: Point) -> Definition {
    Definition::new(
        custom::new_id(),
        name.to_string(),
        "Does this change what a person sees?".to_string(),
        Answer::YesNo { yes: "A person sees it change.".to_string(), no: "Nothing a person sees moves.".to_string() },
        at,
    )
}

/// A decision named `name` at `at` that chooses one of `options`.
fn choosing(name: &str, at: Point, options: &[&str]) -> Definition {
    let mut definition = yes_no(name, at);
    definition.answer =
        Answer::Choice(options.iter().map(|option| ((*option).to_string(), format!("The change is {option}."))).collect());
    definition
}

/// A decision named `name` at `at` that scores on `levels` levels.
fn scoring(name: &str, at: Point, levels: usize) -> Definition {
    let mut definition = yes_no(name, at);
    definition.answer = Answer::Score((0..levels).map(|level| format!("Level {level} of the effort.")).collect());
    definition
}

/// `definition` kept in zo's store in `mode`: its entry, as the index lists
/// it.
pub(crate) fn keep(definition: &Definition, mode: JevMode) -> store::Entry {
    let home = runtime::default_config_home();
    store::create(&home, definition, mode, super::super::decision_shadow::now_ms()).expect("the store keeps the decision");
    store::list(&home).expect("the index reads").into_iter().find(|entry| entry.id == definition.id).expect("its entry")
}

/// `definition` put in the store in place of the version `entry` names, as
/// it is — past every rule the store checks on a write, as a hand or another
/// writer could leave it.
fn written_over(entry: &store::Entry, definition: &Definition) {
    put_over(entry, &definition.to_bytes().expect("a definition's bytes"));
}

/// `bytes` put in the store as the definition `entry` names, under the name
/// they hash to, and the index pointed at them.
fn put_over(entry: &store::Entry, bytes: &[u8]) {
    let root = store::root(&runtime::default_config_home());
    let stored = file_named(&root, &entry.definition);
    let version = custom::version_of(bytes);
    let mut path = stored.with_file_name(&version);
    if let Some(extension) = stored.extension() {
        path.set_extension(extension);
    }
    fs::write(path, bytes).expect("the definition written over");
    let index = index_of(&root);
    let mut listed: Value = serde_json::from_slice(&fs::read(&index).expect("the index")).expect("an index");
    let rows = listed.as_object_mut().into_iter().flat_map(Map::values_mut).filter_map(Value::as_array_mut);
    for row in rows.flatten() {
        if let Ok(mut one) = serde_json::from_value::<store::Entry>(row.clone()) {
            if one.id == entry.id {
                one.definition.clone_from(&version);
                *row = serde_json::to_value(&one).expect("an entry");
            }
        }
    }
    fs::write(index, listed.to_string()).expect("the index written over");
}

/// Every folder and file under `root`, each file with its bytes.
fn files_under(root: &Path) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
    let mut found = BTreeMap::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(folder) = pending.pop() {
        for entry in fs::read_dir(folder).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                found.insert(path.clone(), None);
                pending.push(path);
            } else {
                let bytes = fs::read(&path).ok();
                found.insert(path, bytes);
            }
        }
    }
    found
}

/// The file under `root` whose name, its extension aside, is `stem`: how a
/// test finds a store's file without spelling the store's layout.
fn file_named(root: &Path, stem: &str) -> PathBuf {
    files_under(root)
        .into_keys()
        .find(|path| path.file_stem().is_some_and(|name| name == stem))
        .expect("the store's file")
}

/// The store's index: the one file at its root that reads as JSON.
fn index_of(root: &Path) -> PathBuf {
    fs::read_dir(root)
        .expect("the store's folder")
        .flatten()
        .map(|entry| entry.path())
        .find(|path| path.is_file() && fs::read(path).is_ok_and(|bytes| serde_json::from_slice::<Value>(&bytes).is_ok()))
        .expect("the store's index")
}

/// What `look` finds once it finds anything, waited for as long as a worker
/// may take.
fn waited<T>(mut look: impl FnMut() -> Option<T>) -> Option<T> {
    let started = Instant::now();
    loop {
        if let Some(found) = look() {
            return Some(found);
        }
        if started.elapsed() > WORKER_WAIT {
            return None;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// The rows of the decisions' ledger for `cwd`, waited for until `count` are
/// there: a recording decision writes its row after its point has gone on.
pub(crate) fn rows_in(cwd: &Path, count: usize) -> Vec<Value> {
    waited(|| Some(rows_now(cwd)).filter(|rows| rows.len() >= count)).unwrap_or_else(|| rows_now(cwd))
}

/// The rows of the decisions' ledger for `cwd` as they are now.
fn rows_now(cwd: &Path) -> Vec<Value> {
    read_shadow_rows(&shadow_ledger_path(cwd, CUSTOM_DECISIONS.ledger))
}

/// The rows `rows` holds of the decision `id`.
fn rows_of<'rows>(rows: &'rows [Value], id: &str) -> Vec<&'rows Value> {
    rows.iter().filter(|row| row["decision"] == id).collect()
}

/// Whether `mock` is still unasked once [`LATE`] has passed.
fn unasked(mock: &Mock) -> bool {
    std::thread::sleep(LATE);
    mock.requests().is_empty()
}

/// What the runtime hands the command seat for the call `id` of `command`.
pub(crate) fn command_ask(cwd: &Path, id: &str, command: &str) -> CommandAsk {
    CommandAsk {
        attempt: "turn-1".to_string(),
        owner: "turn-1".to_string(),
        tool_use_id: id.to_string(),
        command: command.to_string(),
        cwd: cwd.to_path_buf(),
        task: "clean the build folder".to_string(),
    }
}

/// The call `id` has run.
pub(crate) fn finished(id: &str) -> CommandRan {
    CommandRan { owner: "turn-1".to_string(), tool_use_id: id.to_string(), failed: false, cancelled: false }
}

/// `definition` asked by name for an agent in `cwd`: what it answered, and
/// the row it wrote.
fn decided(cwd: &Path, definition: &Definition) -> (CustomVerdict, Value) {
    let verdict = decide(cwd, &definition.name, Some("the diff adds one test"), &[]).expect("a verdict");
    let row = rows_now(cwd).into_iter().rev().find(|row| row["decision"] == definition.id.as_str()).expect("its row");
    (verdict, row)
}

/// A model that runs `command` in the shell once and then says it is done,
/// keeping the newest tool result each request carried.
struct RunsOnce {
    command: String,
    asked: bool,
    seen: Arc<Mutex<Vec<String>>>,
}

impl runtime::ApiClient for RunsOnce {
    fn stream(&mut self, request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
        let newest = request.messages.iter().rev().flat_map(|message| message.blocks.iter().rev()).find_map(|block| {
            match block {
                ContentBlock::ToolResult { output, .. } => Some(output.clone()),
                _ => None,
            }
        });
        if let Some(output) = newest {
            self.seen.lock().expect("what the model read").push(output);
        }
        if self.asked {
            return Ok(vec![AssistantEvent::TextDelta("done".to_string()), AssistantEvent::MessageStop]);
        }
        self.asked = true;
        Ok(vec![
            AssistantEvent::ToolUse {
                id: "shell-1".to_string(),
                name: SHELL_TOOL.to_string(),
                input: json!({"command": self.command}).to_string(),
            },
            AssistantEvent::MessageStop,
        ])
    }
}

/// A model that answers every request at once, keeping whether the reminders
/// after the newest request carried `line`.
struct Answers {
    line: String,
    carried: Arc<Mutex<Vec<bool>>>,
}

impl runtime::ApiClient for Answers {
    fn stream(&mut self, request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
        let newest =
            request.messages.iter().rposition(|message| message.role == MessageRole::User).map_or(0, |at| at + 1);
        let carried = request.messages.iter().skip(newest).any(|message| {
            message.role == MessageRole::System
                && message.blocks.iter().any(|block| matches!(block, ContentBlock::Text { text } if text.contains(&self.line)))
        });
        self.carried.lock().expect("what the model read").push(carried);
        Ok(vec![AssistantEvent::TextDelta("done".to_string()), AssistantEvent::MessageStop])
    }
}

/// A recording decision is asked at every point and keeps its row, and what
/// the agent reads — a turn's notes, a command's result, the next turn's
/// advice, an agent's own answer — carries nothing of it.
#[test]
fn shadow_asks_and_writes_but_hands_nothing() {
    let mock = replying(|_| a_noul(0.95));
    machine_words(&words(Some(JevMode::On.key())), &mock.base_url, |cwd| {
        for point in Point::ALL {
            keep(&yes_no(point.key(), point), JevMode::Shadow);
        }
        assert_eq!(run_blocking(SkillSuggestionJudge::at(cwd).suggest(TASK.to_string())), None, "the turn's notes");
        let guard = ToolGuardJudge::at(cwd);
        guard.command(command_ask(cwd, "clean", "rm -rf build"));
        assert_eq!(run_blocking(guard.command_ran(finished("clean"))), None, "the command's result");
        note_project_rule_turn(cwd, "session", "turn-1", &[ConversationMessage::user_text(TASK)]);
        let verdict = decide(cwd, Point::AgentAsks.key(), Some("the diff adds one test"), &[]).expect("a verdict");
        assert_eq!((verdict.line.as_deref(), verdict.mode.as_str()), (None, JevMode::Shadow.key()), "an agent's answer");
        let rows = rows_in(cwd, Point::ALL.len());
        let points: BTreeSet<&str> = rows.iter().filter_map(|row| row["point"].as_str()).collect();
        assert_eq!(points, Point::ALL.into_iter().map(Point::key).collect::<BTreeSet<&str>>(), "{rows:?}");
        for row in &rows {
            assert_eq!(row["mode"], JevMode::Shadow.key(), "{row}");
            assert_eq!(row["outcome"], door::ANSWERED_OUTCOME, "{row}");
            assert_eq!(row["chosen"], AGENT_TOOL_ASK_OPTIONS[0], "{row}");
            assert_eq!(row["requests"], 1, "{row}");
            assert_eq!(row["applied"], false, "{row}");
        }
        assert_eq!(rows.len(), Point::ALL.len(), "{rows:?}");
        let ledger = fs::read_to_string(shadow_ledger_path(cwd, CUSTOM_DECISIONS.ledger)).expect("the ledger");
        print!("{ledger}");
        assert_eq!(mock.requests().len(), Point::ALL.len());
        assert_eq!(pending_project_rule_advice(cwd, "session", project_rules::ADVICE_CHAR_CAP), None, "the next turn");
    });
}

/// A decision that is on hands its one line, in DESIGN 2.4's shape and under
/// its cap, for each kind of answer as sure as its act-from line; one below
/// it, or one that answers it does not apply, hands none and still keeps its
/// row — with a score's level counted from zero there.
#[test]
fn on_above_the_bar_hands_one_line_and_below_hands_none() {
    let (docs, tests, unsure) =
        (yes_no("Docs", Point::TurnStart), yes_no("Tests", Point::TurnStart), yes_no("Unsure", Point::TurnStart));
    let size = choosing("Size", Point::TurnStart, &["small", "large"]);
    let effort = scoring("Effort", Point::TurnStart, 4);
    let scope = choosing("Scope", Point::TurnStart, &["docs", "code"]);
    let answers = BTreeMap::from([
        (docs.id.clone(), a_noul(0.9)),
        (tests.id.clone(), a_noul(0.1)),
        (unsure.id.clone(), a_noul(0.7)),
        (size.id.clone(), a_choice("large", &[("small", 0.1), ("large", 0.85), (custom::NOT_APPLICABLE, 0.05)], 0.85)),
        (effort.id.clone(), a_score(&[0.0, 0.05, 0.9, 0.05], 0.85)),
        (scope.id.clone(), a_choice(custom::NOT_APPLICABLE, &[("docs", 0.05), ("code", 0.05), (custom::NOT_APPLICABLE, 0.9)], 0.9)),
    ]);
    let mock = replying(move |id| answers.get(id).cloned().unwrap_or_default());
    machine_words(&words(Some(JevMode::On.key())), &mock.base_url, |cwd| {
        for definition in [&docs, &tests, &unsure, &size, &effort, &scope] {
            keep(definition, JevMode::On);
        }
        let lines = run_blocking(at_turn_start(cwd.to_path_buf(), TASK.to_string()));
        let (yes, no) = (AGENT_TOOL_ASK_OPTIONS[0], AGENT_TOOL_ASK_OPTIONS[1]);
        let handed = [
            custom::line("Docs", yes, 0.9),
            custom::line("Tests", no, 1.0 - 0.1),
            custom::line("Size", "large", 0.85),
            custom::line("Effort", &custom::shown_level(2, 4), 0.85),
        ];
        assert_eq!(lines, handed);
        assert!(lines.iter().all(|line| line.chars().count() <= custom::LINE_MAX_CHARS), "{lines:?}");
        let rows = rows_now(cwd);
        let applied = |definition: &Definition| rows_of(&rows, &definition.id).first().map(|row| row["applied"].clone());
        for (definition, was) in [(&docs, true), (&tests, true), (&unsure, false), (&size, true), (&effort, true), (&scope, false)] {
            assert_eq!(applied(definition), Some(Value::from(was)), "{}", definition.name);
        }
        let unsure_row = rows_of(&rows, &unsure.id)[0];
        assert_eq!((&unsure_row["chosen"], &unsure_row["confidence"]), (&Value::from(yes), &Value::from(0.7)));
        assert_eq!(rows_of(&rows, &effort.id)[0]["chosen"], 2, "the row keeps the level counted from zero");
        assert_eq!(rows_of(&rows, &scope.id)[0]["chosen"], custom::NOT_APPLICABLE);
    });
}

/// Whatever a decision asked before a command answers, the command runs —
/// once, and as the model wrote it. A decision that is on adds its line to
/// what the model reads of the result; one that records adds nothing.
#[test]
fn before_a_command_never_stops_it() {
    let (yes, no) = (AGENT_TOOL_ASK_OPTIONS[0], AGENT_TOOL_ASK_OPTIONS[1]);
    for (mode, answer, handed) in [(JevMode::On, 0.95, Some(yes)), (JevMode::On, 0.05, Some(no)), (JevMode::Shadow, 0.05, None)] {
        let mock = replying(move |_| a_noul(answer));
        machine_words(&words(Some(JevMode::On.key())), &mock.base_url, |cwd| {
            keep(&yes_no("Cleanup", Point::BeforeCommand), mode);
            let (shell_log, seen) = (Arc::new(Mutex::new(Vec::new())), Arc::new(Mutex::new(Vec::new())));
            let shell = Arc::clone(&shell_log);
            let mut turns = ConversationRuntime::new(
                Session::new(),
                RunsOnce { command: "rm -rf build".to_string(), asked: false, seen: Arc::clone(&seen) },
                StaticToolExecutor::new().register(SHELL_TOOL, move |input| {
                    shell.lock().expect("the shell's log").push(input.to_string());
                    Ok("removed".to_string())
                }),
                PermissionPolicy::new(PermissionMode::DangerFullAccess),
                vec!["system".to_string()],
            );
            turns.set_tool_guard_seat(Some(Arc::new(ToolGuardJudge::at(cwd)) as Arc<dyn ToolGuardSeat>));
            turns.run_turn("clean the build folder", None).expect("the turn");
            let ran = shell_log.lock().expect("the shell's log").clone();
            assert_eq!(ran.len(), 1, "{mode:?} {answer}: the command ran once");
            let input: Value = serde_json::from_str(&ran[0]).expect("the shell's input");
            assert_eq!(input["command"], "rm -rf build", "{mode:?} {answer}: as the model wrote it");
            let read = seen.lock().expect("what the model read").first().cloned().unwrap_or_default();
            assert!(read.contains("removed"), "{mode:?} {answer}: {read}");
            let sure = if answer < 0.5 { 1.0 - answer } else { answer };
            for option in [yes, no] {
                let line = custom::line("Cleanup", option, sure);
                assert_eq!(read.contains(&line), handed == Some(option), "{mode:?} {answer}: {read}");
            }
            let rows = rows_in(cwd, 1);
            assert_eq!(rows.len(), 1, "{rows:?}");
            assert_eq!(rows[0]["outcome"], door::ANSWERED_OUTCOME);
        });
    }
}

/// A folder the person never consented to asks nothing, and its row says
/// the door refused; a decision whose folders hold neither the folder asked
/// in nor one above it asks nothing and writes nothing.
#[test]
fn outside_consent_or_scope_asks_nothing() {
    let mock = replying(|_| a_noul(0.9));
    machine_words(&words(Some(JevMode::On.key())), &mock.base_url, |_| {
        let unconsented = tempfile::tempdir().expect("a folder nobody consented to");
        let elsewhere = fs::canonicalize(unconsented.path()).expect("the folder resolved");
        keep(&yes_no("Docs", Point::TurnStart), JevMode::On);
        assert!(run_blocking(at_turn_start(elsewhere.clone(), TASK.to_string())).is_empty());
        let refused = rows_now(&elsewhere);
        assert_eq!(refused.len(), 1, "{refused:?}");
        assert_eq!(refused[0]["outcome"], Refused::NotConsented.token());
        assert_eq!(refused[0]["requests"], 0);
        assert!(unasked(&mock), "nothing left for a folder nobody consented to");
    });
    machine_words(&words(Some(JevMode::On.key())), &mock.base_url, |cwd| {
        let chosen = tempfile::tempdir().expect("the one folder the decisions ask in");
        let folders = BTreeSet::from([fs::canonicalize(chosen.path()).expect("resolved").to_string_lossy().into_owned()]);
        for point in [Point::TurnStart, Point::AgentAsks] {
            let mut definition = yes_no(point.key(), point);
            definition.folders = Folders::Chosen(folders.clone());
            keep(&definition, JevMode::On);
        }
        assert!(run_blocking(at_turn_start(cwd.to_path_buf(), TASK.to_string())).is_empty());
        let verdict = decide(cwd, Point::AgentAsks.key(), Some("the diff adds one test"), &[]).expect("a verdict");
        assert_eq!((verdict.line.as_deref(), verdict.outcome.as_str()), (None, JevMode::Off.key()));
        assert!(unasked(&mock), "nothing left outside the decisions' folders");
        assert!(rows_now(cwd).is_empty(), "and nothing was written");
    });
}

/// No line that may carry a credential leaves zo: one in what a point lends
/// is withheld from its request, and a definition holding one in its
/// question or an option's name is not asked at all — its row says why,
/// and holds none of it.
#[test]
fn secret_lines_never_leave() {
    let mock = replying(|_| a_noul(0.9));
    machine_words(&words(Some(JevMode::On.key())), &mock.base_url, |cwd| {
        keep(&yes_no("Docs", Point::TurnStart), JevMode::On);
        let task = format!("{TASK}\nAuthorization: Bearer {SECRET}\nthen say so");
        assert_eq!(run_blocking(at_turn_start(cwd.to_path_buf(), task)).len(), 1, "the rest of the task was asked");
        keep(&yes_no("Fetch", Point::BeforeCommand), JevMode::On);
        let guard = ToolGuardJudge::at(cwd);
        guard.command(command_ask(cwd, "fetch", &format!("curl -H 'Authorization: Bearer {SECRET}' https://example.invalid/a")));
        assert!(run_blocking(guard.command_ran(finished("fetch"))).is_some(), "the rest of the command was asked");
        let lent = rows_now(cwd);
        assert!(lent.len() == 2 && lent.iter().all(|row| row["redactedLines"].as_u64() >= Some(1)), "{lent:?}");
        let mut leaking = yes_no("Rotate", Point::TurnStart);
        let rotate = keep(&leaking, JevMode::On);
        leaking.question = format!("Rotate it?\nAuthorization: Bearer {SECRET}");
        written_over(&rotate, &leaking);
        let mut naming = choosing("Pick", Point::TurnStart, &["ship", "wait"]);
        let pick = keep(&naming, JevMode::On);
        naming.answer = Answer::Choice(BTreeMap::from([
            ("ship".to_string(), "Ship it.".to_string()),
            (SECRET_OPTION.to_string(), "Wait.".to_string()),
        ]));
        written_over(&pick, &naming);
        let asked = mock.requests().len();
        run_blocking(at_turn_start(cwd.to_path_buf(), TASK.to_string()));
        assert_eq!(mock.requests().len(), asked + 1, "only the decision that keeps every rule was asked");
        let rows = rows_now(cwd);
        let word = Refusal::SecretLine { field: Field::Question, line: 1 }.word();
        for entry in [&rotate, &pick] {
            let written = rows_of(&rows, &entry.id);
            assert_eq!(written.len(), 1, "{written:?}");
            assert_eq!(written[0]["outcome"], word);
        }
        let ledger = fs::read_to_string(shadow_ledger_path(cwd, CUSTOM_DECISIONS.ledger)).expect("the ledger");
        for said in mock.requests().iter().chain([&ledger]) {
            assert!(!said.contains(SECRET) && !said.contains(SECRET_OPTION), "{said}");
        }
    });
}

/// A decision past its own bound in the hour, or past the bound every
/// decision shares, sends nothing and writes `hourly_budget`.
#[test]
fn hourly_bound_writes_hourly_budget_and_sends_nothing() {
    let mock = replying(|_| a_noul(0.9));
    machine_words(&words(Some(JevMode::On.key())), &mock.base_url, |cwd| {
        let mut once = yes_no("Once", Point::AgentAsks);
        once.max_per_hour = *custom::HOURLY_REQUESTS.start();
        keep(&once, JevMode::On);
        let ask = |name: &str| decide(cwd, name, Some("the diff adds one test"), &[]).expect("a verdict");
        assert_eq!(ask("Once").outcome, door::ANSWERED_OUTCOME);
        let bounded = ask("Once");
        assert_eq!((bounded.outcome.as_str(), bounded.line.as_deref()), (Refused::HourlyBudget.token(), None));
        assert_eq!(mock.requests().len(), 1, "its own hour sent nothing more");
        let path = runtime::default_config_home().join("settings.json");
        let mut settings: Value = serde_json::from_str(&fs::read_to_string(&path).expect("the settings")).expect("JSON");
        settings[SMART_SETTINGS_KEY][JEV_SETTINGS_KEY][CUSTOM_HOURLY_REQUESTS_SETTING] = Value::from(2);
        fs::write(path, settings.to_string()).expect("the settings written");
        for name in ["First", "Second"] {
            keep(&yes_no(name, Point::AgentAsks), JevMode::On);
        }
        assert_eq!(ask("First").outcome, door::ANSWERED_OUTCOME);
        let shared = ask("Second");
        assert_eq!((shared.outcome.as_str(), shared.line.as_deref()), (Refused::HourlyBudget.token(), None));
        assert_eq!(mock.requests().len(), 2, "the hour every decision shares sent nothing more");
        let rows = rows_now(cwd);
        let outcomes: Vec<&str> = rows.iter().filter_map(|row| row["outcome"].as_str()).collect();
        let (answered, bound) = (door::ANSWERED_OUTCOME, Refused::HourlyBudget.token());
        assert_eq!(outcomes, [answered, bound, answered, bound]);
        assert!(rows.iter().filter(|row| row["outcome"] == bound).all(|row| row["requests"] == 0), "{rows:?}");
    });
}

/// What a command lends its decisions of the person's words is the first
/// line with words, cut to the command guard's cap in characters.
#[test]
fn a_command_lends_the_first_line_of_the_task_cut_to_its_cap() {
    let mock = replying(|_| a_noul(0.9));
    machine_words(&words(Some(JevMode::On.key())), &mock.base_url, |cwd| {
        let mut decision = yes_no("Task", Point::BeforeCommand);
        decision.reads = BTreeSet::from([Input::Task]);
        keep(&decision, JevMode::On);
        let guard = ToolGuardJudge::at(cwd);
        let long = "가".repeat(COMMAND_GUARD_TASK_CHAR_CAP * 2);
        let cases = [
            ("long", format!("{long}\nthe second line"), "가".repeat(COMMAND_GUARD_TASK_CHAR_CAP)),
            ("short", "  \nShip the docs\nthe second line".to_string(), "Ship the docs".to_string()),
        ];
        for (id, task, lent) in cases {
            let mut ask = command_ask(cwd, id, "rm -rf build");
            ask.task = task;
            guard.command(ask);
            assert!(run_blocking(guard.command_ran(finished(id))).is_some(), "{id}: the decision answered");
            let requests = mock.requests();
            let body: Value = serde_json::from_str(requests.last().expect("a request")).expect("a JSON body");
            assert_eq!(body["state"][Input::Task.key()], Value::from(lent), "{id}");
            assert_eq!(body["state"].as_object().map(Map::len), Some(1), "{id}: it reads the task alone");
        }
    });
}

/// With no decision kept — Jev on and no word for the switch, or the switch
/// on — or with every decision off, through the switch or its own mode,
/// nothing is asked at any point, nothing under zo's config home changes, a
/// turn's end is not even read, and what each point hands is what it hands
/// with no decision at all.
#[test]
fn with_no_decision_or_every_one_off_nothing_changes() {
    let (on, off, shadow) = (JevMode::On.key(), JevMode::Off.key(), JevMode::Shadow.key());
    let cases =
        [(None, None), (Some(on), None), (Some(on), Some(JevMode::Off)), (Some(off), Some(JevMode::On)), (Some(shadow), Some(JevMode::Off))];
    for (switch, kept) in cases {
        let mock = replying(|_| a_noul(0.9));
        machine_words(&words(switch), &mock.base_url, |cwd| {
            if let Some(mode) = kept {
                for point in Point::ALL {
                    keep(&yes_no(point.key(), point), mode);
                }
            }
            let home = runtime::default_config_home();
            let before = files_under(&home);
            assert_eq!(run_blocking(SkillSuggestionJudge::at(cwd).suggest(TASK.to_string())), None);
            let guard = ToolGuardJudge::at(cwd);
            guard.command(command_ask(cwd, "clean", "rm -rf build"));
            assert_eq!(run_blocking(guard.command_ran(finished("clean"))), None);
            note_project_rule_turn(cwd, "session", "turn-1", &[ConversationMessage::user_text(TASK)]);
            let mut read = false;
            at_turn_end(cwd, "session", || {
                read = true;
                TurnEnd { task: TASK.to_string(), final_text: String::new(), evidence: String::new(), edited_paths: Vec::new() }
            });
            assert!(!read, "{switch:?} {kept:?}: the turn is not read");
            assert_eq!(pending_project_rule_advice(cwd, "session", project_rules::ADVICE_CHAR_CAP), None);
            match decide(cwd, Point::AgentAsks.key(), Some("the diff adds one test"), &[]) {
                Ok(verdict) => assert_eq!((verdict.line, verdict.outcome.as_str()), (None, off), "{switch:?} {kept:?}"),
                Err(error) => assert!(kept.is_none(), "{switch:?} {kept:?}: {error}"),
            }
            assert!(unasked(&mock), "{switch:?} {kept:?}: nothing was asked");
            assert_eq!(files_under(&home), before, "{switch:?} {kept:?}: nothing under the config home changed");
        });
    }
}

/// A store in the folder a turn runs in is never read: only zo's config
/// home keeps the person's decisions.
#[test]
fn a_folders_own_store_is_never_read() {
    let mock = replying(|_| a_noul(0.9));
    machine_words(&words(Some(JevMode::On.key())), &mock.base_url, |cwd| {
        let own = cwd.join(".zo");
        for point in Point::ALL {
            store::create(&own, &yes_no(point.key(), point), JevMode::On, 0).expect("a store in the folder");
        }
        assert_eq!(store::list(&own).expect("the folder's store reads").len(), Point::ALL.len());
        assert!(store::list(&runtime::default_config_home()).expect("zo's store reads").is_empty());
        assert_eq!(run_blocking(SkillSuggestionJudge::at(cwd).suggest(TASK.to_string())), None);
        assert!(run_blocking(at_turn_start(cwd.to_path_buf(), TASK.to_string())).is_empty());
        let guard = ToolGuardJudge::at(cwd);
        guard.command(command_ask(cwd, "clean", "rm -rf build"));
        assert_eq!(run_blocking(guard.command_ran(finished("clean"))), None);
        note_project_rule_turn(cwd, "session", "turn-1", &[ConversationMessage::user_text(TASK)]);
        assert!(decide(cwd, Point::AgentAsks.key(), None, &[]).is_err(), "no decision of zo's has the name");
        assert!(unasked(&mock), "the folder's store asked nothing");
        assert!(rows_now(cwd).is_empty());
    });
}

/// How a test damages the store in zo's config home.
#[derive(Debug, Clone, Copy)]
enum Damage {
    /// The index is cut off mid-write.
    TornIndex,
    /// The definition's file holds another version's bytes, which no longer
    /// hash to its name.
    ChangedBytes,
    /// The index names a definition one byte past the size the store reads.
    TooLarge,
}

/// A store damaged as `damage` says asks nothing at a turn's start, writes
/// nothing, and leaves the turn as it was.
fn a_damaged_store_asks_nothing(damage: Damage) {
    let mock = replying(|_| a_noul(0.9));
    machine_words(&words(Some(JevMode::On.key())), &mock.base_url, |cwd| {
        let decision = yes_no("Docs", Point::TurnStart);
        let entry = keep(&decision, JevMode::On);
        let root = store::root(&runtime::default_config_home());
        match damage {
            Damage::TornIndex => fs::write(index_of(&root), b"{").expect("a torn index"),
            Damage::ChangedBytes => {
                let mut changed = decision.clone();
                changed.question = "Does this change what a person reads?".to_string();
                let bytes = changed.to_bytes().expect("its bytes");
                fs::write(file_named(&root, &entry.definition), bytes).expect("bytes of another version");
            }
            Damage::TooLarge => {
                let mut bytes = decision.to_bytes().expect("its bytes");
                bytes.resize(store::DEFINITION_MAX_BYTES + 1, b' ');
                put_over(&entry, &bytes);
            }
        }
        assert_eq!(run_blocking(SkillSuggestionJudge::at(cwd).suggest(TASK.to_string())), None, "{damage:?}");
        assert!(run_blocking(at_turn_start(cwd.to_path_buf(), TASK.to_string())).is_empty(), "{damage:?}");
        assert!(unasked(&mock), "{damage:?}: nothing was asked");
        assert!(rows_now(cwd).is_empty(), "{damage:?}: nothing was written");
    });
}

/// A torn index asks nothing, writes nothing, and leaves the turn as it was.
#[test]
fn a_torn_index_asks_nothing_and_leaves_the_turn_as_it_was() {
    a_damaged_store_asks_nothing(Damage::TornIndex);
}

/// A definition whose bytes no longer hash to its name asks nothing, writes
/// nothing, and leaves the turn as it was.
#[test]
fn a_definition_that_no_longer_hashes_to_its_name_asks_nothing() {
    a_damaged_store_asks_nothing(Damage::ChangedBytes);
}

/// A definition past the size the store reads back asks nothing, writes
/// nothing, and leaves the turn as it was.
#[test]
fn a_definition_past_the_size_the_store_reads_asks_nothing() {
    a_damaged_store_asks_nothing(Damage::TooLarge);
}

/// A definition the store would refuse today — here an option's name past
/// its length — is not asked: it writes one row whose outcome is the word of
/// the rule it breaks, and none of its words.
#[test]
fn a_definition_that_breaks_a_rule_writes_its_word_and_asks_nothing() {
    let long_option = "release-every-change-to-every-customer-at-once";
    let mock = replying(|_| a_noul(0.9));
    machine_words(&words(Some(JevMode::On.key())), &mock.base_url, |cwd| {
        for point in [Point::AgentAsks, Point::TurnStart] {
            let mut definition = choosing(point.key(), point, &["ship", "wait"]);
            let entry = keep(&definition, JevMode::On);
            definition.answer = Answer::Choice(BTreeMap::from([
                (long_option.to_string(), "Ship it all.".to_string()),
                ("wait".to_string(), "Wait.".to_string()),
            ]));
            let broken = custom::check(&definition);
            assert_eq!(broken, [Refusal::OptionName(long_option.to_string())], "the store would refuse it today");
            written_over(&entry, &definition);
        }
        let verdict = decide(cwd, Point::AgentAsks.key(), Some("the diff adds one test"), &[]).expect("a verdict");
        let word = Refusal::OptionName(String::new()).word();
        assert_eq!((verdict.outcome.as_str(), verdict.line.as_deref()), (word, None));
        assert!(run_blocking(at_turn_start(cwd.to_path_buf(), TASK.to_string())).is_empty());
        let rows = rows_now(cwd);
        assert_eq!(rows.len(), 2, "one row each: {rows:?}");
        for row in &rows {
            assert_eq!(row["outcome"], word, "{row}");
            assert_eq!((row.get("chosen"), &row["requests"]), (None, &Value::from(0)), "{row}");
            assert!(!row.to_string().contains(long_option), "the row keeps the rule's word, never the option: {row}");
        }
        assert!(unasked(&mock), "nothing was asked");
    });
}

/// A command today's read-only rule clears asks no decision, as the runtime
/// asks the command seat about none, and writes no row.
#[test]
fn a_read_only_command_asks_nothing_and_writes_no_row() {
    let mock = replying(|_| a_noul(0.9));
    machine_words(&words(Some(JevMode::On.key())), &mock.base_url, |cwd| {
        keep(&yes_no("Cleanup", Point::BeforeCommand), JevMode::On);
        let guard = ToolGuardJudge::at(cwd);
        let commands = ["git status", "ls -la crates && git log --oneline -3", "grep -rn guard zo-ide | head"];
        for (id, command) in commands.iter().enumerate() {
            assert!(!asks_about(command), "{command}: today's rule proves it read-only");
            let id = id.to_string();
            guard.command(command_ask(cwd, &id, command));
            assert_eq!(run_blocking(guard.command_ran(finished(&id))), None, "{command}");
        }
        assert!(unasked(&mock), "a read-only command asked nothing");
        assert!(rows_now(cwd).is_empty(), "and wrote no row");
    });
}

/// Each point asks its decisions with the use already asking there off — the
/// skill suggestion, the command guard, the project-rules check — and hands
/// the line all the same: at a turn's start alone under the skill note's
/// prefix, beside a command's result, and to the next turn as an advice of
/// its own, once, that names no rule and leaves no rule's receipt.
#[test]
fn each_point_asks_with_the_use_already_there_off() {
    let mock = replying(|_| a_noul(0.9));
    machine_words(&words(Some(JevMode::On.key())), &mock.base_url, |cwd| {
        for point in [Point::TurnStart, Point::BeforeCommand, Point::TurnEnd] {
            keep(&yes_no(point.key(), point), JevMode::On);
        }
        let line = |point: Point| custom::line(point.key(), AGENT_TOOL_ASK_OPTIONS[0], 0.9);
        let note = run_blocking(SkillSuggestionJudge::at(cwd).suggest(TASK.to_string()));
        assert_eq!(note, Some(format!("{}\n{}", runtime::SKILL_RECOMMENDATION_REMINDER_PREFIX, line(Point::TurnStart))));
        let guard = ToolGuardJudge::at(cwd);
        guard.command(command_ask(cwd, "clean", "rm -rf build"));
        assert_eq!(run_blocking(guard.command_ran(finished("clean"))), Some(line(Point::BeforeCommand)));
        note_project_rule_turn(cwd, "session", "turn-1", &[ConversationMessage::user_text(TASK)]);
        let advice =
            waited(|| pending_project_rule_advice(cwd, "session", project_rules::ADVICE_CHAR_CAP)).expect("the next turn's advice");
        let mut said = advice.text.lines();
        assert!(said.next().is_some_and(|head| head.contains(&advice.key)), "{}", advice.text);
        assert_eq!(said.collect::<Vec<&str>>(), [line(Point::TurnEnd).as_str()]);
        assert!(advice.rules.is_empty(), "it names no rule");
        project_rule_advice_delivered(cwd, "session", &advice);
        assert_eq!(pending_project_rule_advice(cwd, "session", project_rules::ADVICE_CHAR_CAP), None, "it is handed once");
        assert!(!shadow_ledger_path(cwd, PROJECT_RULES.ledger).exists(), "and leaves no rule's receipt");
        let rows = rows_in(cwd, 3);
        assert_eq!(rows.iter().filter(|row| row["applied"] == true).count(), 3, "{rows:?}");
        assert_eq!(mock.requests().len(), 3);
    });
}

/// A turn whose start has a decision's line and no skill suggestion carries
/// the line under the skill note's prefix alone, so the next turn's start
/// clears it like any skill note: the model reads it in its own turn only.
#[test]
fn a_lone_decision_line_opens_with_the_skill_prefix_and_is_gone_next_turn() {
    let mock = replying(|_| a_noul(0.9));
    machine_words(&words(Some(JevMode::On.key())), &mock.base_url, |cwd| {
        let entry = keep(&yes_no("Docs", Point::TurnStart), JevMode::On);
        let line = custom::line("Docs", AGENT_TOOL_ASK_OPTIONS[0], 0.9);
        let carried = Arc::new(Mutex::new(Vec::new()));
        let mut turns = ConversationRuntime::new(
            Session::new(),
            Answers { line: line.clone(), carried: Arc::clone(&carried) },
            StaticToolExecutor::new(),
            PermissionPolicy::new(PermissionMode::DangerFullAccess),
            vec!["system".to_string()],
        );
        turns.set_skill_suggestion_seat(Some(Arc::new(SkillSuggestionJudge::at(cwd)) as Arc<dyn SkillSuggestionSeat>));
        let prefix = runtime::SKILL_RECOMMENDATION_REMINDER_PREFIX;
        turns.run_turn(TASK, None).expect("the first turn");
        let noted: Vec<&String> = turns.transient_reminders().iter().filter(|reminder| reminder.starts_with(prefix)).collect();
        assert_eq!(noted, [&format!("{prefix}\n{line}")], "the line rides the skill note's prefix alone");
        let first = carried.lock().expect("what the model read").clone();
        assert!(!first.is_empty() && first.iter().all(|seen| *seen), "the first turn's model read the line: {first:?}");
        store::set_mode(&runtime::default_config_home(), &entry.id, JevMode::Off, 1).expect("the decision turned off");
        turns.run_turn("and the changelog", None).expect("the second turn");
        assert!(turns.transient_reminders().iter().all(|reminder| !reminder.starts_with(prefix)), "the note is gone");
        let second = carried.lock().expect("what the model read")[first.len()..].to_vec();
        assert!(!second.is_empty() && second.iter().all(|seen| !*seen), "the next turn's model never read it: {second:?}");
        assert_eq!(mock.requests().len(), 1, "the decision was asked in the first turn alone");
    });
}

/// A yes as likely as a no reaches a line of a half and hands nothing: its
/// yes is the order a tie breaks in, which nothing decided. The row keeps the
/// answer all the same.
#[test]
fn a_yes_or_no_tie_at_a_line_of_a_half_hands_nothing() {
    let mock = replying(|_| a_noul(0.5));
    machine_words(&words(Some(JevMode::On.key())), &mock.base_url, |cwd| {
        let mut definition = yes_no("Even", Point::AgentAsks);
        definition.act_from = *custom::ACT_FROM.start();
        keep(&definition, JevMode::On);
        let (verdict, row) = decided(cwd, &definition);
        assert_eq!(verdict.line, None);
        assert_eq!((&row["outcome"], &row["chosen"]), (&Value::from(door::ANSWERED_OUTCOME), &Value::from(AGENT_TOOL_ASK_OPTIONS[0])));
        assert_eq!((&row["confidence"], &row["applied"]), (&Value::from(0.5), &Value::from(false)));
    });
}

/// An option alone at the top of three reaches a line of a half and hands its
/// line, and its row says it was handed.
#[test]
fn a_choice_alone_at_the_top_at_a_line_of_a_half_hands_its_line() {
    let spread = [("small", 0.5), ("medium", 0.3), ("large", 0.2), (custom::NOT_APPLICABLE, 0.0)];
    let mock = replying(move |_| a_choice("small", &spread, 0.5));
    machine_words(&words(Some(JevMode::On.key())), &mock.base_url, |cwd| {
        let mut definition = choosing("Size", Point::AgentAsks, &["small", "medium", "large"]);
        definition.act_from = *custom::ACT_FROM.start();
        keep(&definition, JevMode::On);
        let (verdict, row) = decided(cwd, &definition);
        assert_eq!(verdict.line, Some(custom::line("Size", "small", 0.5)));
        assert_eq!((&row["chosen"], &row["applied"]), (&Value::from("small"), &Value::from(true)));
    });
}

/// Two options as likely as each other hand nothing, whichever one the
/// answer names; the row keeps the answer all the same.
#[test]
fn a_two_option_tie_hands_nothing() {
    let spread = [("ship", 0.5), ("wait", 0.5), (custom::NOT_APPLICABLE, 0.0)];
    let mock = replying(move |_| a_choice("ship", &spread, 0.5));
    machine_words(&words(Some(JevMode::On.key())), &mock.base_url, |cwd| {
        let mut definition = choosing("Ship", Point::AgentAsks, &["ship", "wait"]);
        definition.act_from = *custom::ACT_FROM.start();
        keep(&definition, JevMode::On);
        let (verdict, row) = decided(cwd, &definition);
        assert_eq!(verdict.line, None);
        assert_eq!((&row["chosen"], &row["applied"]), (&Value::from("ship"), &Value::from(false)));
    });
}
