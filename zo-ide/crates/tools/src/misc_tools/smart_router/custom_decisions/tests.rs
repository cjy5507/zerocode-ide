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
        assert_eq!(mock.requests().len(), Point::ALL.len());
        assert_eq!(pending_project_rule_advice(cwd, "session", project_rules::ADVICE_CHAR_CAP), None, "the next turn");
    });
}
