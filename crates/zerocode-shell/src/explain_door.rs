//! The door a request for an explanation page passes (t-32787).
//!
//! The rules are core's: [`zerocode_core::explain`] says what a request
//! carries, what is taken out of it and what the model is told, and
//! [`zerocode_core::explain_desk`] says what each event of a pane's life does
//! to a request in flight. This is the part that owns the panes, the one-shot
//! process and the window's events.
//!
//! **A pane's conversation goes through the window's one typing door**
//! ([`crate::cmd::terminal::type_prompt_at_term`]), asked not to type over a
//! person: the guard refuses while the line holds words the person typed,
//! while a question is parked, and when a hand reaches the line as the words
//! settle. The common refusals are decided before anything is registered
//! ([`zerocode_core::explain_desk::decide`]), so a refused request leaves no
//! delivery behind — and no generated prompt on the person's clipboard, which
//! is where the window puts words a delivery handed back. A pane in a turn
//! holds the request ([`Desk::release_waiting`]); the event that says it is
//! between turns ([`note_pane_state`]) sends it, once.
//!
//! **A one-shot is the vendor's own CLI, run once** under the login the panes
//! use ([`zerocode_core::capabilities::OneShotRoad`]), through the window's one
//! launch ledger. Its answer is the page; the window publishes it itself.
//!
//! **Nothing here polls.** Three events end a request — the delivery's one
//! answer (a thread that waits for it and nothing else), the pane's state moving
//! ([`note_pane_state`]), and a publication that names the pane
//! ([`note_published`]) — plus the pane going away ([`note_pane_gone`]) and a
//! person's cancel. When no request stands, each of them costs one atomic load.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::RecvTimeoutError;
use std::sync::{Mutex, OnceLock, PoisonError};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter as _, Manager as _};
use zerocode_core::artifact::{Artifact, ArtifactKind};
use zerocode_core::capabilities::OneShotRoad;
use zerocode_core::explain::{self, Ask, Kind, OneShotPrompt, why};
use zerocode_core::explain_desk::{
    Decision, Desk, Heard, Held, PaneFacts, State, Term, decide, heard,
};
use zerocode_core::hook::HookState;
use zerocode_core::launch_budget::Refusal as BudgetRefusal;
use zerocode_pty::DeliveryOutcome;

use crate::hooks::PaneHookReport;
use crate::launch_budget_door::{Budgeted, Launch, run_budgeted};
use crate::quota_wall::{StallCause, one_shot_cause};
use crate::scm_runtime::{Once, OnceFailure};
use crate::{AppState, ShellStateExt as _};

/// What the window hears about a request.
const STATE_EVENT: &str = "explain:state";

/// Where a request goes — the card's choice.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "via", rename_all = "snake_case")]
pub(crate) enum Route {
    /// The conversation of the agent that sits in this pane.
    Conversation { term: Term },
    /// The vendor's one-shot mode, run on its own.
    OneShot { agent: String },
}

/// One request, as the window sends it. Exactly one of `text` and `report`
/// carries the material: the chosen text, or the id of a worker's report whose
/// file the door reads itself.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct Start {
    pub(crate) id: String,
    pub(crate) kind: String,
    pub(crate) title: String,
    #[serde(default)]
    pub(crate) text: Option<String>,
    #[serde(default)]
    pub(crate) report: Option<String>,
    pub(crate) language: String,
    pub(crate) headline: String,
    pub(crate) route: Route,
    /// Where the person was when they pressed. An older window sends none.
    #[serde(default)]
    pub(crate) from: Option<Source>,
}

/// What the window knows of where a request came from: the pane the person was
/// reading in and the folder of the checkout it belongs to. Each is absent when
/// the window does not hold it — a diff belongs to a checkout and to no pane, and
/// a report's folder is the report's own — so that a page's origin never says
/// what nobody vouched for.
#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct Source {
    #[serde(default)]
    pub(crate) term: Option<Term>,
    #[serde(default)]
    pub(crate) cwd: Option<String>,
}

/// What a one-shot's page is published as, and where its request came from.
struct Page {
    kind: Kind,
    title: String,
    headline: String,
    from: Source,
    /// The artifact id of the report, when a task's report is what was explained.
    report: Option<String>,
}

/// What the card says about the material before it is sent.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct Facts {
    pub(crate) lines: usize,
    pub(crate) masked: usize,
    pub(crate) clipped: bool,
}

/// An agent the window can run once.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct Road {
    pub(crate) agent: &'static str,
}

/// One word about a request, sent to the window.
#[derive(Debug, Clone, Serialize)]
struct Told {
    id: String,
    state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    why: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    term: Option<Term>,
    #[serde(skip_serializing_if = "Option::is_none")]
    agent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    artifact: Option<Artifact>,
}

impl Told {
    fn new(id: &str, state: State) -> Self {
        Self {
            id: id.to_string(),
            state: state.word(),
            why: None,
            term: None,
            agent: None,
            artifact: None,
        }
    }
}

/// The requests in flight, and whether any stands — the one thing the
/// pane-state road asks on every event.
struct Door {
    desk: Mutex<Desk>,
    standing: AtomicBool,
}

fn door() -> &'static Door {
    static DOOR: OnceLock<Door> = OnceLock::new();
    DOOR.get_or_init(|| Door {
        desk: Mutex::new(Desk::default()),
        standing: AtomicBool::new(false),
    })
}

/// Run `then` on the desk, and keep the "something stands" flag true to it.
fn with_desk<R>(then: impl FnOnce(&mut Desk) -> R) -> R {
    let door = door();
    let mut desk = door.desk.lock().unwrap_or_else(PoisonError::into_inner);
    let out = then(&mut desk);
    door.standing.store(!desk.is_empty(), Ordering::Release);
    out
}

fn anything_stands() -> bool {
    door().standing.load(Ordering::Acquire)
}

fn tell(app: &AppHandle, told: Told) {
    let _ = app.emit(STATE_EVENT, told);
}

/// Say a request failed, whether or not the desk ever held it — a request
/// refused at the door was never admitted.
fn tell_failed(app: &AppHandle, id: &str, reason: &'static str, term: Option<Term>) {
    tell(
        app,
        Told {
            why: Some(reason),
            term,
            ..Told::new(id, State::Failed)
        },
    );
}

/// A standing request ends in failure: it leaves the desk and the window is
/// told — once, and only if it was still standing (a cancelled one is not told).
fn fail(app: &AppHandle, id: &str, reason: &'static str, term: Option<Term>) {
    if with_desk(|desk| desk.finish(id)).is_some() {
        tell_failed(app, id, reason, term);
    }
}

/// What the window knows about a pane. `reported` is the state the pane has
/// just reported, which the state map does not hold yet while the report is
/// still being merged into it.
fn facts_of(state: &AppState, term: Term, reported: Option<HookState>) -> PaneFacts<'static> {
    let agent = state.agent_terms().get(&term).copied();
    let said = reported.or_else(|| state.pane_states().get(&term).map(|held| held.state));
    let present = state.terminals().contains_key(&term);
    PaneFacts {
        present,
        agent,
        busy: said == Some(HookState::Working),
        parked: said == Some(HookState::NeedsAttention),
        draft: crate::human_input::line_of(term).0,
    }
}

/// The material of a request: the text it carries, or a worker's report read
/// from the catalog. A report is read to the bytes a request may receive and no
/// further; only a report or a document is read this way.
fn material_of(text: Option<&str>, report: Option<&str>) -> Result<String, String> {
    let Some(id) = report else {
        return text
            .map(str::to_string)
            .ok_or_else(|| "an explanation needs text or a report".to_string());
    };
    let store = crate::artifact_runtime::store().ok_or("the artifact store is not ready")?;
    let row = store
        .get(id)
        .ok_or_else(|| format!("no such artifact: {id}"))?;
    if !matches!(row.kind, ArtifactKind::Report | ArtifactKind::Document) {
        return Err(format!("not a report: {id}"));
    }
    read_bounded_text(&row.path, explain::RECEIVE_BYTES_MAX)
}

fn read_bounded_text(path: &Path, cap: usize) -> Result<String, String> {
    use std::io::Read as _;
    let mut held = Vec::new();
    std::fs::File::open(path)
        .and_then(|file| file.take(cap as u64).read_to_end(&mut held))
        .map_err(|error| error.to_string())?;
    Ok(String::from_utf8_lossy(&held).into_owned())
}

/// What the card says about the material, before anything is sent.
pub(crate) fn preview(
    kind: &str,
    text: Option<String>,
    report: Option<String>,
) -> Result<Facts, String> {
    let kind = Kind::parse(kind).ok_or_else(|| format!("unknown kind: {kind}"))?;
    let raw = material_of(text.as_deref(), report.as_deref())?;
    let prepared = explain::prepare(kind, &raw);
    Ok(Facts {
        lines: prepared.lines,
        masked: prepared.masked,
        clipped: prepared.clipped,
    })
}

/// The agents the window can run once on this machine: the rows that have a
/// one-shot road, whose program is installed.
pub(crate) fn roads() -> Vec<Road> {
    // One look at the PATH for every agent, not one per row.
    let present = crate::scm_runtime::detected_agents(false);
    zerocode_core::agent::AGENT_SPECS
        .iter()
        .filter(|spec| spec.capabilities().one_shot.is_some())
        .filter(|spec| present.iter().any(|row| row.id == spec.id && row.installed))
        .map(|spec| Road { agent: spec.id })
        .collect()
}

/// Take a request. Everything that happens to it after this is told as
/// `explain:state`; the only error is a request the door cannot read at all.
pub(crate) fn start(app: &AppHandle, request: Start) -> Result<(), String> {
    let kind =
        Kind::parse(&request.kind).ok_or_else(|| format!("unknown kind: {}", request.kind))?;
    // The title is shown to a model and kept in the catalog: it is masked like the material.
    let title = explain::scrub_title(&request.title);
    let prepared = explain::prepare(
        kind,
        &material_of(request.text.as_deref(), request.report.as_deref())?,
    );
    let ask = Ask {
        kind,
        title: &title,
        language: &request.language,
        headline: &request.headline,
        material: &prepared,
    };
    match &request.route {
        Route::Conversation { term } => {
            start_in_pane(app, &request.id, *term, explain::conversation_prompt(&ask));
        }
        Route::OneShot { agent } => start_one_shot(
            app,
            &request.id,
            agent,
            explain::one_shot_prompt(&ask),
            Page {
                kind,
                title: title.clone(),
                headline: request.headline.clone(),
                from: request.from.clone().unwrap_or_default(),
                report: request.report.clone(),
            },
        ),
    }
    Ok(())
}

/// A person's cancel: the request is let go. What already went into a pane, or
/// a one-shot already running, is not taken back — what is let go is the
/// opening of whatever comes of it.
pub(crate) fn cancel(id: &str) -> bool {
    with_desk(|desk| desk.finish(id)).is_some()
}

// ---- a pane's conversation ----

fn start_in_pane(app: &AppHandle, id: &str, term: Term, prompt: String) {
    let state = app.state::<AppState>();
    let facts = facts_of(&state, term, None);
    let Some(agent) = facts.agent else {
        return tell_failed(
            app,
            id,
            if facts.present {
                why::NO_AGENT
            } else {
                why::NO_PANE
            },
            Some(term),
        );
    };
    let waits = match decide(&facts) {
        Decision::Refuse(reason) => return tell_failed(app, id, reason, Some(term)),
        Decision::Wait => true,
        Decision::Send => false,
    };
    let held = Held {
        id: id.to_string(),
        term: Some(term),
        agent: agent.to_string(),
        state: if waits { State::Waiting } else { State::Sent },
        made_ms: crate::now_epoch_ms(),
        prompt: waits.then(|| prompt.clone()),
    };
    if let Err(reason) = with_desk(|desk| desk.admit(held)) {
        return tell_failed(app, id, reason, Some(term));
    }
    if waits {
        tell(
            app,
            Told {
                term: Some(term),
                agent: Some(agent.to_string()),
                ..Told::new(id, State::Waiting)
            },
        );
    } else {
        type_into(app, id, term, agent, prompt);
    }
}

/// Register the words with the typing door and wait — on a thread that does
/// nothing else, for the delivery's one answer — to say how it ended.
fn type_into(app: &AppHandle, id: &str, term: Term, agent: &'static str, prompt: String) {
    use crate::cmd::terminal::{PromptReadiness, type_prompt_at_term};
    let readiness = PromptReadiness::RestingBesideADraft;
    let state = app.state::<AppState>();
    let Ok(waiting) = type_prompt_at_term(&state, term, prompt, true, Some(agent), readiness)
    else {
        return fail(app, id, why::NO_PANE, Some(term));
    };
    // The door's own deadline and the pane's receipt after it — and no less,
    // or this gives up on a delivery that is still waiting for its program.
    let patience =
        readiness.door_deadline(Some(agent)) + zerocode_pty::ready::SUBMIT_RECEIPT_PATIENCE;
    let app = app.clone();
    let id = id.to_string();
    std::thread::spawn(move || settle(&app, &id, term, waiting.recv_timeout(patience)));
}

/// How a delivery ended: taken, or withheld for a reason the guard names.
fn settle(
    app: &AppHandle,
    id: &str,
    term: Term,
    outcome: Result<DeliveryOutcome, RecvTimeoutError>,
) {
    match outcome {
        Ok(DeliveryOutcome::Delivered) => {
            if with_desk(|desk| desk.move_to(id, State::Asked)) {
                tell(
                    app,
                    Told {
                        term: Some(term),
                        ..Told::new(id, State::Asked)
                    },
                );
            }
        }
        Ok(DeliveryOutcome::Unsubmitted(refusal) | DeliveryOutcome::Refused(refusal)) => {
            fail(app, id, refusal.token(), Some(term));
        }
        Ok(DeliveryOutcome::TimedOut) | Err(_) => fail(app, id, why::NOT_READY, Some(term)),
    }
}

/// The pane reported its state. A turn that ended releases the request that
/// waited for it, and ends one that was asked and got no page; an agent that
/// left takes its requests with it. A pane in a turn changes nothing — and
/// neither does a session starting ([`heard`] says which reports those are).
pub(crate) fn note_pane_state(app: &AppHandle, report: &PaneHookReport) {
    if !anything_stands() {
        return;
    }
    let term = report.term;
    match heard(report.state, report.session_boundary) {
        Heard::Nothing => {}
        Heard::TurnEnded => {
            if let Some(id) = with_desk(|desk| desk.turn_ended(term)) {
                tell_failed(app, &id, why::NO_PAGE, Some(term));
            }
            release(app, term, report.state);
        }
        Heard::AgentGone => {
            for id in with_desk(|desk| desk.pane_gone(term)) {
                tell_failed(app, &id, why::NO_AGENT, Some(term));
            }
        }
    }
}

/// The pane is between turns: send what waited for it, if the line is still
/// the agent's to write on.
fn release(app: &AppHandle, term: Term, reported: HookState) {
    let Some(held) = with_desk(|desk| desk.release_waiting(term)) else {
        return;
    };
    let state = app.state::<AppState>();
    let facts = facts_of(&state, term, Some(reported));
    match (decide(&facts), facts.agent, held.prompt) {
        (Decision::Refuse(reason), _, _) => fail(app, &held.id, reason, Some(term)),
        (_, Some(agent), Some(prompt)) => type_into(app, &held.id, term, agent, prompt),
        _ => fail(app, &held.id, why::NO_AGENT, Some(term)),
    }
}

/// A page was published. When it came from the pane a request asked, that
/// request is finished: the window opens the page the agent made.
pub(crate) fn note_published(app: &AppHandle, row: &Artifact) {
    if !anything_stands() {
        return;
    }
    let Some(term) = row
        .origin
        .pane
        .as_deref()
        .and_then(crate::hooks::term_of_pane_key)
    else {
        return;
    };
    let Some(id) = with_desk(|desk| desk.page_published(term, row.modified_ms)) else {
        return;
    };
    tell(
        app,
        Told {
            term: Some(term),
            agent: row.origin.agent.clone(),
            artifact: Some(row.clone()),
            ..Told::new(&id, State::Ready)
        },
    );
}

/// A terminal is gone — closed, or its shell ended: its requests go with it.
pub(crate) fn note_pane_gone(term: Term) {
    if !anything_stands() {
        return;
    }
    let ids = with_desk(|desk| desk.pane_gone(term));
    let Some(app) = crate::artifact_runtime::window_handle() else {
        return;
    };
    for id in ids {
        tell_failed(&app, &id, why::NO_PANE, Some(term));
    }
}

// ---- a one-shot ----

fn start_one_shot(app: &AppHandle, id: &str, agent: &str, prompt: OneShotPrompt, page: Page) {
    let Some(road) = zerocode_core::agent_capabilities(agent).and_then(|caps| caps.one_shot) else {
        return tell_failed(app, id, why::NO_AGENT, None);
    };
    let Some(program) = crate::usage_runtime::agent_program(agent) else {
        return tell_failed(app, id, why::CLI_MISSING, None);
    };
    let held = Held {
        id: id.to_string(),
        term: None,
        agent: agent.to_string(),
        state: State::Running,
        made_ms: crate::now_epoch_ms(),
        prompt: None,
    };
    if let Err(reason) = with_desk(|desk| desk.admit(held)) {
        return tell_failed(app, id, reason, None);
    }
    tell(
        app,
        Told {
            agent: Some(agent.to_string()),
            ..Told::new(id, State::Running)
        },
    );
    let app = app.clone();
    let (id, agent) = (id.to_string(), agent.to_string());
    std::thread::spawn(move || {
        let made = run_once_for(&app, &agent, road, &program, &prompt);
        match made.and_then(|html| publish_made(&app, &id, &agent, &page, &html)) {
            Ok(row) => finish_ready(&app, &id, &agent, row),
            Err(reason) => fail(&app, &id, reason, None),
        }
    });
}

/// The environment the CLI runs under: the account the window's panes run as.
fn one_shot_env(state: &AppState, road: OneShotRoad) -> Result<Vec<(String, String)>, String> {
    match road {
        OneShotRoad::ClaudePrint => crate::scm_runtime::claude_reading_env(state.config_root()),
        OneShotRoad::CodexExec => {
            let mut env = crate::usage_runtime::account_env_for(state.config_root(), "codex")?;
            env.extend(crate::hooks::agent_launch_env(
                state.local_data_root(),
                "codex",
            ));
            Ok(env)
        }
    }
}

/// Run the CLI once and read the page out of what it answered. The reason a
/// run did not end in a page is a [`why`] token.
fn run_once_for(
    app: &AppHandle,
    agent: &str,
    road: OneShotRoad,
    program: &str,
    prompt: &OneShotPrompt,
) -> Result<String, &'static str> {
    let state = app.state::<AppState>();
    let env = one_shot_env(&state, road).map_err(|_| why::CLI_REFUSED)?;
    let argv = explain::one_shot_argv(road, &prompt.system);
    let stdin = explain::one_shot_stdin(road, prompt);
    // The same question asked twice while the first still runs is one job.
    let job = zerocode_core::launch_budget::job_key("", "", "explain", &stdin);
    let launch = Launch {
        provider: agent,
        job: Some(&job),
        fresh_ms: None,
        requested: true,
    };
    let cwd = crate::computer_use::errand::value::one_shot_dir();
    let once = match run_budgeted(
        &launch,
        program,
        cwd.as_deref(),
        &argv,
        &env,
        &stdin,
        explain::ONE_SHOT_WALL,
    ) {
        Budgeted::Refused(refusal) => return Err(budget_token(&refusal)),
        Budgeted::Ran(Err(OnceFailure::Spawn(_))) => return Err(why::CLI_MISSING),
        Budgeted::Ran(Err(OnceFailure::TimedOut)) => return Err(why::TIMED_OUT),
        Budgeted::Ran(Ok(once)) => once,
    };
    let said = explain::read_one_shot(road, &once.stdout, &once.stderr_tail, once.success)
        .map_err(|words| refused_token(agent, &words, &once))?;
    explain::page_from_output(&said).map_err(|_| why::NOT_HTML)
}

/// Why the launch ledger would not start a run.
fn budget_token(refusal: &BudgetRefusal) -> &'static str {
    match refusal {
        BudgetRefusal::Resting { .. } => why::QUOTA_WALL,
        BudgetRefusal::Running => why::IN_FLIGHT,
        _ => why::BUDGET,
    }
}

/// Why a CLI that ran did not answer: the quota wall and the login wall are
/// read by the same words the window reads a pane's by; anything else is the
/// CLI's own refusal.
fn refused_token(agent: &str, words: &str, once: &Once) -> &'static str {
    let said = format!("{}\n{words}", once.stderr_tail);
    match one_shot_cause(agent, &said) {
        Some(StallCause::QuotaWall) => why::QUOTA_WALL,
        Some(StallCause::LoginWall) => why::LOGIN_WALL,
        _ => why::CLI_REFUSED,
    }
}

/// A name for the file the page is written to, from the request's name: the
/// name came from the window, and a path is made of nothing else.
fn file_stem(id: &str) -> String {
    id.chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || *ch == '-')
        .collect()
}

/// The origin of the page a one-shot made: what the window and the ledger vouch
/// for about where the request came from ([`explain::page_origin`] says how they
/// are put together), asked as late as possible — the pane may have changed what it
/// sits over while the CLI ran, and the report's row is read as it stands.
fn origin_of_page(app: &AppHandle, agent: &str, page: &Page) -> zerocode_core::artifact::Origin {
    let base = crate::artifact_runtime::origin_of_source(
        app,
        page.from.term,
        page.from.cwd.as_deref().map(Path::new),
    );
    let report = page
        .report
        .as_deref()
        .and_then(|id| crate::artifact_runtime::store()?.get(id))
        .map(|row| row.origin);
    explain::page_origin(base, agent, report.as_ref())
}

/// Publish the page a one-shot made, through the road the hook door uses. The
/// file the store reads it from is the store's own folder and is removed
/// as soon as the store has its copy.
fn publish_made(
    app: &AppHandle,
    id: &str,
    agent: &str,
    page: &Page,
    html: &str,
) -> Result<Artifact, &'static str> {
    if with_desk(|desk| desk.get(id).is_none()) {
        // Cancelled while it ran: nothing opens.
        return Err(why::NOT_PUBLISHED);
    }
    let store = crate::artifact_runtime::store().ok_or(why::NOT_PUBLISHED)?;
    let dir: PathBuf = store.root().join("explain");
    let file = dir.join(format!("{}.html", file_stem(id)));
    std::fs::create_dir_all(&dir).map_err(|_| why::NOT_PUBLISHED)?;
    std::fs::write(&file, html).map_err(|_| why::NOT_PUBLISHED)?;
    let input = explain::publish_input(page.kind, &page.title, &page.headline, file.clone());
    let origin = origin_of_page(app, agent, page);
    let published = crate::artifact_runtime::publish_and_tell(&store, &input, origin);
    let _ = std::fs::remove_file(&file);
    published
        .map(|(_, row)| row)
        .map_err(|_| why::NOT_PUBLISHED)
}

fn finish_ready(app: &AppHandle, id: &str, agent: &str, row: Artifact) {
    if with_desk(|desk| desk.finish(id)).is_none() {
        return;
    }
    tell(
        app,
        Told {
            agent: Some(agent.to_string()),
            artifact: Some(row),
            ..Told::new(id, State::Ready)
        },
    );
}

#[cfg(test)]
mod tests;
