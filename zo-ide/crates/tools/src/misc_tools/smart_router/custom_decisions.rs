//! A person's own decisions (t-62170, stage 2), asked at zo's four points: a
//! turn's start, its end, right before a shell command, and an agent asking
//! one by name. Each point finds the decisions the person's store keeps for
//! it, asks each through the door inside the hours that bound it, writes its
//! row, and hands the one line an acting decision earned.
//!
//! The store is read from zo's config home alone ([`store::asking_in`]): a
//! project's own files never define a decision. Nothing past the switch over
//! every decision is read while it is off, and nothing is written until a
//! decision is asked, so a person with none — or with every one off — finds
//! the product as it was, to the byte. Each point asks outside the seat that
//! already asks there, so a decision works with that seat off. A decision
//! never stops a command or changes what a turn says: all it may do is hand
//! one line ([`custom::line`]), and only while it is on and as sure as its
//! act-from line.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use api::{SystemOneClient, SystemOneConfig, SystemOneFailure};
use futures_util::future::{BoxFuture, FutureExt, Shared};
use runtime::CommandAsk;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use zerocode_core::jev::custom::{self, deadline_at, standing, store, Definition, Lent, Point};
use zerocode_core::jev::door::{self, Refused};
use zerocode_core::jev::custom::Input;
use zerocode_core::jev::tool_guard::asks_about;
use zerocode_core::jev::{count, JevMode, CUSTOM_DECISIONS};
use zerocode_core::AgentKind;

use super::jev_gate::{self, JevDoor};
use super::patch_review::detach;
use super::settings::merged_settings_root_from;
use super::shadow_ledger::{append_shadow_row, shadow_ledger_path, SHADOW_LEDGER_MAX_BYTES};

/// The switch over every decision (`smart.customDecisions`) in the settings
/// `cwd` reads; off when they cannot be read.
fn switch_in(cwd: &Path) -> JevMode {
    merged_settings_root_from(&runtime::ConfigLoader::default_for(cwd))
        .map_or(JevMode::Off, |root| CUSTOM_DECISIONS.mode_in(&root))
}

/// One decision about to be asked: its row in the index, its definition, and
/// where it stands.
#[derive(Debug, Clone)]
struct Asked {
    entry: store::Entry,
    definition: Definition,
    mode: JevMode,
}

/// The decisions that ask at `point` in `cwd`, each where it stands. Nothing
/// past the switch is read while it is off, and a store that cannot be read
/// asks nothing.
fn asking_at(cwd: &Path, point: Point) -> Vec<Asked> {
    let switch = switch_in(cwd);
    if !switch.asks() {
        return Vec::new();
    }
    let workspace = PathBuf::from(door::resolved_path(cwd));
    store::asking_in(&runtime::default_config_home(), &workspace)
        .unwrap_or_default()
        .into_iter()
        .filter(|(_, definition)| definition.at == point)
        .map(|(entry, definition)| Asked { mode: standing(switch, entry.mode), entry, definition })
        .filter(|asked| asked.mode.asks())
        .collect()
}

/// One request of a decision's, as its ledger keeps it ([`custom::DecisionRow`]),
/// with what the server said beside it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct DecisionRow {
    #[serde(flatten)]
    row: custom::DecisionRow,
    #[serde(flatten)]
    server_said: jev_gate::ServerSaid,
}

impl DecisionRow {
    fn of(asked: &Asked, outcome: &str) -> Self {
        Self {
            row: custom::DecisionRow::of(
                super::decision_shadow::unix_millis(),
                AgentKind::Zo.slug(),
                &asked.entry,
                &asked.definition,
                asked.mode,
                outcome,
            ),
            server_said: jev_gate::ServerSaid::default(),
        }
    }
}

/// The decision and version a row of the ledger names: how the summary tells
/// the one ledger's rows apart by decision and by version.
#[derive(Deserialize)]
struct Named<'row> {
    decision: &'row str,
    definition: &'row str,
}

/// The decision's id and version `row` names, when it is one of a decision's.
pub(super) fn named_by(row: &Value) -> Option<(&str, &str)> {
    Named::deserialize(row).ok().map(|named| (named.decision, named.definition))
}

fn write(cwd: &Path, row: &DecisionRow) {
    let _ = append_shadow_row(&shadow_ledger_path(cwd, CUSTOM_DECISIONS.ledger), row, SHADOW_LEDGER_MAX_BYTES);
}

/// `asked` while its definition still keeps every rule ([`custom::check`]);
/// else the word of the first rule it breaks, on the one row it writes. It is
/// not asked: a definition the store would refuse today sends nothing.
fn kept(cwd: &Path, asked: Asked) -> Result<Asked, &'static str> {
    match custom::check(&asked.definition).first() {
        Some(refusal) => {
            write(cwd, &DecisionRow::of(&asked, refusal.word()));
            Err(refusal.word())
        }
        None => Ok(asked),
    }
}

/// The line one decision handed, with what earned it.
#[derive(Debug, Clone, PartialEq)]
struct Handed {
    decision: String,
    version: String,
    line: String,
    chosen: Value,
    confidence: f64,
}

/// Ask one decision through the door, inside its point's wall, and write its
/// row: the row's outcome, and the line it hands when it earns one.
async fn ask_one(
    cwd: &Path,
    door: &JevDoor,
    client: Option<&SystemOneClient>,
    asked: &Asked,
    lent: &Lent,
) -> (String, Option<Handed>) {
    let mut row = DecisionRow::of(asked, door::ANSWERED_OUTCOME);
    let handed = judged(cwd, door, client, asked, lent, &mut row).await;
    row.row.applied = handed.is_some();
    let outcome = row.row.outcome.clone();
    let ledger = shadow_ledger_path(cwd, CUSTOM_DECISIONS.ledger);
    let _ = tokio::task::spawn_blocking(move || append_shadow_row(&ledger, &row, SHADOW_LEDGER_MAX_BYTES)).await;
    (outcome, handed)
}

async fn judged(
    cwd: &Path,
    door: &JevDoor,
    client: Option<&SystemOneClient>,
    asked: &Asked,
    lent: &Lent,
    row: &mut DecisionRow,
) -> Option<Handed> {
    let home = runtime::default_config_home();
    let offset_minutes = i32::try_from(core_types::date::local_utc_offset_secs() / 60).unwrap_or(0);
    let hour = count::hour_of(super::decision_shadow::now_ms(), offset_minutes);
    let (decision_hour, every_hour) =
        (count::hour_path(&home, &asked.entry.id, &hour), count::hour_path(&home, custom::EVERY_DECISION, &hour));
    let body = custom::body_of(&asked.definition, lent);
    let passed = door.pass_within_the_hour(
        &CUSTOM_DECISIONS,
        client.is_some(),
        body,
        &decision_hour,
        asked.definition.max_per_hour,
        &every_hour,
    );
    let (cleared, client) = match (passed, client) {
        (Ok(cleared), Some(client)) => (cleared, client),
        (passed, _) => {
            row.row.outcome = passed.err().unwrap_or(Refused::NoKey).token().to_string();
            return None;
        }
    };
    row.row.redacted_lines = u32::try_from(cleared.withheld_lines()).unwrap_or(u32::MAX);
    let call = jev_gate::send(client, cleared, deadline_at(asked.definition.at), None).await;
    row.row.requests = call.requests;
    row.row.retries = call.retries;
    row.row.elapsed_ms = jev_gate::millis(call.elapsed);
    row.server_said = jev_gate::ServerSaid::of(&call);
    let response = match call.outcome {
        Ok(response) => response,
        Err(failure) => {
            row.row.outcome = failure.ledger_token();
            return None;
        }
    };
    row.row.model = Some(response.model.clone());
    row.row.input_tokens = Some(response.usage.input_tokens);
    let reading = match custom::read(&asked.definition, response.answers.get(&asked.definition.id)) {
        Ok(reading) => reading,
        Err(rule) => {
            row.row.outcome = SystemOneFailure::Schema.ledger_token();
            row.row.rejected = Some(rule);
            return None;
        }
    };
    row.row.chosen = Some(reading.chosen.clone());
    row.row.confidence = Some(reading.confidence);
    let hands = asked.mode.applies()
        && custom::earns_its_line(&asked.definition, &reading)
        && switch_in(cwd) == JevMode::On
        && door.permits_application_now();
    hands.then(|| Handed {
        decision: asked.entry.id.clone(),
        version: asked.entry.definition.clone(),
        line: custom::line(&asked.definition.name, &custom::said(&asked.definition, &reading.shown), reading.confidence),
        chosen: reading.chosen,
        confidence: reading.confidence,
    })
}

/// Ask every decision in `asking` through one door: what each row's outcome
/// was, and the line each hands.
async fn ask_each(cwd: PathBuf, asking: Vec<Asked>, lent: Arc<Lent>) -> Vec<(String, Option<Handed>)> {
    let opened = cwd.clone();
    let Ok(door) = tokio::task::spawn_blocking(move || JevDoor::open(&opened)).await else {
        return Vec::new();
    };
    let client = SystemOneConfig::from_env().ok().map(SystemOneConfig::into_client);
    let asks = asking.iter().map(|asked| ask_one(&cwd, &door, client.as_ref(), asked, &lent));
    futures_util::future::join_all(asks).await
}

/// Ask every decision in `asking` with what its point lent: the recording
/// ones on a worker, so they hold up nobody, and the acting ones awaited for
/// the lines they hand.
async fn ask_all(cwd: PathBuf, asking: Vec<Asked>, lent: Lent) -> Vec<Handed> {
    let (acting, recording): (Vec<Asked>, Vec<Asked>) =
        asking.into_iter().filter_map(|asked| kept(&cwd, asked).ok()).partition(|asked| asked.mode.applies());
    let lent = Arc::new(lent);
    if !recording.is_empty() {
        let (cwd, lent) = (cwd.clone(), Arc::clone(&lent));
        detach(async move {
            drop(ask_each(cwd, recording, lent).await);
        });
    }
    if acting.is_empty() {
        return Vec::new();
    }
    ask_each(cwd, acting, lent).await.into_iter().filter_map(|(_, handed)| handed).collect()
}

/// `note` with `lines` under it, one to a line, after the one line saying
/// what they are ([`custom::introduced`]).
pub(super) fn beside(note: Option<String>, lines: &[String]) -> Option<String> {
    let Some(lines) = custom::introduced(lines) else {
        return note;
    };
    Some(match note {
        Some(note) => format!("{note}\n{lines}"),
        None => lines,
    })
}

/* ---- a turn's start ----------------------------------------------------------- */

/// The lines the decisions asking at a turn's start hand that turn, for a
/// request whose words are `task`; none for a request with no words.
pub(super) async fn at_turn_start(cwd: PathBuf, task: String) -> Vec<String> {
    if task.trim().is_empty() {
        return Vec::new();
    }
    let asking = asking_at(&cwd, Point::TurnStart);
    if asking.is_empty() {
        return Vec::new();
    }
    let lent: Lent = [(Input::Task.key().to_string(), Value::from(task))].into_iter().collect();
    ask_all(cwd, asking, lent).await.into_iter().map(|handed| handed.line).collect()
}

/* ---- a turn's end ------------------------------------------------------------- */

/// What a turn's end lends the decisions asking there: the turn as the
/// project-rules check reads it.
pub(super) struct TurnEnd {
    pub(super) task: String,
    pub(super) final_text: String,
    pub(super) evidence: String,
    pub(super) edited_paths: Vec<String>,
}

/// Lines a turn's end offered the next turn, under the key their advice goes
/// out with: the turn that ended at `at`.
#[derive(Debug, Clone)]
struct Offered {
    at: u64,
    key: String,
    handed: Vec<Handed>,
}

type Offers = HashMap<(PathBuf, String), Offered>;

fn offers() -> &'static Mutex<Offers> {
    static OFFERS: OnceLock<Mutex<Offers>> = OnceLock::new();
    OFFERS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// A turn of `recipient`'s in `workspace` has ended: ask the decisions that
/// ask at a turn's end on a worker, and offer the lines the acting ones hand
/// to the next turn ([`pending_lines`]). `turn` is read only when a decision
/// asks.
pub(super) fn at_turn_end(workspace: &Path, recipient: &str, turn: impl FnOnce() -> TurnEnd) {
    let asking = asking_at(workspace, Point::TurnEnd);
    if asking.is_empty() {
        return;
    }
    let TurnEnd { task, final_text, evidence, edited_paths } = turn();
    let lent = custom::lent_at_turn_end(task, final_text, evidence, edited_paths);
    let (cwd, recipient) = (workspace.to_path_buf(), recipient.to_string());
    let at = super::decision_shadow::unix_millis();
    detach(async move {
        let handed = ask_all(cwd.clone(), asking, lent).await;
        offer(cwd, recipient, at, handed);
    });
}

/// Offer what the turn that ended at `at` handed in place of what an earlier
/// turn offered: a later turn's end speaks for the turn after it, even when
/// it hands nothing, and an earlier turn's late answer never replaces it.
fn offer(workspace: PathBuf, recipient: String, at: u64, handed: Vec<Handed>) {
    let Ok(mut offered) = offers().lock() else {
        return;
    };
    let named = (workspace, recipient);
    if offered.get(&named).is_some_and(|later| later.at > at) {
        return;
    }
    if handed.is_empty() {
        offered.remove(&named);
        return;
    }
    let lines: Vec<&str> = handed.iter().map(|one| one.line.as_str()).collect();
    let key = custom::version_of(format!("{at}\n{}", lines.join("\n")).as_bytes());
    offered.insert(named, Offered { at, key, handed });
}

/// The lines a turn's end offered `recipient`'s next turn in `workspace`, with
/// the key their advice goes out under: those whose decision still stands on
/// at the version that answered, while the switch is on and the door lets
/// `workspace` act.
pub(super) fn pending_lines(workspace: &Path, recipient: &str) -> Option<(String, Vec<String>)> {
    let offered = offers().lock().ok()?.get(&(workspace.to_path_buf(), recipient.to_string()))?.clone();
    if switch_in(workspace) != JevMode::On || !jev_gate::workspace_permitted_now(workspace) {
        return None;
    }
    let listed = store::list(&runtime::default_config_home()).ok()?;
    let lines: Vec<String> = offered
        .handed
        .into_iter()
        .filter(|one| {
            listed.iter().any(|entry| entry.id == one.decision && entry.definition == one.version && entry.mode.applies())
        })
        .map(|one| one.line)
        .collect();
    (!lines.is_empty()).then_some((offered.key, lines))
}

/// Whether `key` names the lines offered `recipient` in `workspace` — taken
/// when it does, so they are handed once.
pub(super) fn delivered(workspace: &Path, recipient: &str, key: &str) -> bool {
    let Ok(mut offered) = offers().lock() else {
        return false;
    };
    let named = (workspace.to_path_buf(), recipient.to_string());
    let ours = offered.get(&named).is_some_and(|one| one.key == key);
    if ours {
        offered.remove(&named);
    }
    ours
}

/* ---- before a shell command --------------------------------------------------- */

type Judged = Shared<BoxFuture<'static, Vec<String>>>;
type Commands = HashMap<(PathBuf, String, String), (Instant, Judged)>;

/// The lines acting decisions will hand a command once it has run, by project
/// and call.
fn commands() -> &'static Mutex<Commands> {
    static COMMANDS: OnceLock<Mutex<Commands>> = OnceLock::new();
    COMMANDS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// A shell command is about to run in `project`: ask the decisions that ask
/// before one on a worker, and keep what the acting ones will hand for
/// [`after_command`]. Returns before anything is asked. A command the
/// read-only rule clears asks nothing ([`asks_about`]), as the runtime hands
/// the seat none. The task lent is the first line of the person's words, cut
/// to the command guard's cap ([`custom::lent_before_command`]).
pub(super) fn before_command(project: &Path, ask: &CommandAsk) {
    if !asks_about(&ask.command) {
        return;
    }
    let asking = asking_at(project, Point::BeforeCommand);
    if asking.is_empty() {
        return;
    }
    let lent = custom::lent_before_command(&ask.command, &ask.cwd, &ask.task);
    let acting = asking.iter().any(|asked| asked.mode.applies());
    let judged: Judged = ask_all(project.to_path_buf(), asking, lent)
        .map(|handed| handed.into_iter().map(|one| one.line).collect())
        .boxed()
        .shared();
    if acting {
        if let Ok(mut waiting) = commands().lock() {
            waiting.insert(
                (project.to_path_buf(), ask.owner.clone(), ask.tool_use_id.clone()),
                (Instant::now(), judged.clone()),
            );
        }
    }
    detach(async move {
        drop(judged.await);
    });
}

/// The call `tool_use_id` of `owner`'s has run in `project`: the lines its
/// acting decisions hand, waited for no longer than the rest of the wall.
pub(super) async fn after_command(project: PathBuf, owner: String, tool_use_id: String) -> Vec<String> {
    let waiting = commands().lock().ok().and_then(|mut waiting| waiting.remove(&(project, owner, tool_use_id)));
    let Some((asked_at, judged)) = waiting else {
        return Vec::new();
    };
    let wall = deadline_at(Point::BeforeCommand).saturating_sub(asked_at.elapsed());
    tokio::time::timeout(wall, judged).await.unwrap_or_default()
}

/* ---- an agent asks by name ---------------------------------------------------- */

/// What `zo jev decide <name>` answers (t-62170): the line a decision that is
/// on hands an agent, or why there is none.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomVerdict {
    /// The decision's name, as it was asked for.
    pub name: String,
    /// Where it stood: the lower of the switch and its own mode.
    pub mode: String,
    /// What became of the request: its row's outcome, or `off` when nothing
    /// was asked.
    pub outcome: String,
    /// The one line ([`custom::line`]), only from a decision that is on and
    /// as sure as its act-from line.
    pub line: Option<String>,
    /// What it chose beside the line: `yes` or `no`, a choice's option, or a
    /// score's level counted from the lowest at zero.
    pub chosen: Option<Value>,
    /// How sure it was, from zero to one.
    pub confidence: Option<f64>,
    /// Why there is no line, or what the line is.
    pub note: String,
}

impl CustomVerdict {
    fn nothing(name: &str, mode: JevMode, outcome: &str, note: String) -> Self {
        Self {
            name: name.to_string(),
            mode: mode.key().to_string(),
            outcome: outcome.to_string(),
            line: None,
            chosen: None,
            confidence: None,
            note,
        }
    }
}

/// Ask the decision named `name` for an agent in `cwd`, with the `context`
/// and `items` it passed. Only a decision that is on and as sure as its
/// act-from line answers; one that records keeps its answer for the person.
/// The row is written before this returns.
///
/// # Errors
/// No decision has `name`, the store or its definition cannot be read, or the
/// decision asks at another point.
pub fn decide(cwd: &Path, name: &str, context: Option<&str>, items: &[String]) -> Result<CustomVerdict, String> {
    let off = JevMode::Off.key();
    let switch = switch_in(cwd);
    if !switch.asks() {
        let note = format!(
            "smart.{} is off, so nothing was asked; decide this yourself, or ask the person to turn it on in the \
             ZeroCode settings.",
            CUSTOM_DECISIONS.setting
        );
        return Ok(CustomVerdict::nothing(name, JevMode::Off, off, note));
    }
    let home = runtime::default_config_home();
    let entry = store::list(&home)
        .map_err(|error| format!("the decisions store cannot be read: {error}"))?
        .into_iter()
        .find(|entry| entry.name == name)
        .ok_or_else(|| format!("no decision is named '{name}'"))?;
    let definition = store::definition(&home, &entry.definition)
        .ok()
        .filter(|definition| definition.id == entry.id)
        .ok_or_else(|| format!("'{name}' cannot be read from the decisions store"))?;
    if definition.at != Point::AgentAsks {
        return Err(format!("'{name}' asks at {}, not when an agent asks", definition.at.key()));
    }
    let mode = standing(switch, entry.mode);
    if !mode.asks() {
        return Ok(CustomVerdict::nothing(name, mode, off, format!("'{name}' is off, so nothing was asked; decide this yourself.")));
    }
    if !custom::asks_in(&definition, Path::new(&door::resolved_path(cwd))) {
        let note = format!("'{name}' does not ask in this folder, so nothing was asked; decide this yourself.");
        return Ok(CustomVerdict::nothing(name, mode, off, note));
    }
    let asked = match kept(cwd, Asked { entry, definition, mode }) {
        Ok(asked) => asked,
        Err(word) => {
            let note = format!("'{name}' breaks a rule it was kept under ({word}), so nothing was asked; decide this yourself.");
            return Ok(CustomVerdict::nothing(name, mode, word, note));
        }
    };
    let mut lent = Lent::new();
    if let Some(context) = context {
        lent.insert(Input::Context.key().to_string(), Value::from(context));
    }
    if !items.is_empty() {
        lent.insert(Input::Items.key().to_string(), Value::from(items.to_vec()));
    }
    let door = JevDoor::open(cwd);
    let client = SystemOneConfig::from_env().ok().map(SystemOneConfig::into_client);
    let (outcome, handed) = api::sync_bridge::run_blocking(ask_one(cwd, &door, client.as_ref(), &asked, &lent));
    let mut verdict = CustomVerdict::nothing(name, mode, &outcome, String::new());
    match handed {
        Some(handed) => {
            verdict.note = "Jev answered; the line is the whole of its reasoning.".to_string();
            verdict.line = Some(handed.line);
            verdict.chosen = Some(handed.chosen);
            verdict.confidence = Some(handed.confidence);
        }
        None if outcome != door::ANSWERED_OUTCOME => {
            verdict.note = format!("No answer ({outcome}); decide this yourself.");
        }
        None if mode.applies() => {
            verdict.note = format!(
                "'{name}' answered below its act-from line, in a tie, or that it does not apply, so it hands \
                 nothing; decide this yourself."
            );
        }
        None => {
            verdict.note = format!(
                "'{name}' is {}, so the answer was recorded for the person and withheld from you; decide this \
                 yourself.",
                mode.key()
            );
        }
    }
    Ok(verdict)
}

#[cfg(test)]
pub(super) mod tests;
