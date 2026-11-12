//! A schedule an agent asks for (`recipe-schedule`).
//!
//! An agent may ask the window to walk a saved Flow on a schedule with nobody
//! watching. Only the person's answer on the window's card writes the job, and
//! everything that decides what such a request may become is in this file:
//!
//! - the card's sentence is the window's; what the agent gave — a label — and
//!   what a document it wrote says — the Flow's title — are shown as data,
//!   by the one rule every such card shows data by
//!   (`zerocode_core::card_words`), and the card says how many letters
//!   that draw nothing the rule left out of them;
//! - the job is built before the card and the card is drawn from it
//!   ([`Request`]), so what the person allowed is what is saved;
//! - the card says which document it is about, beyond the title the document
//!   gives itself ([`About`]): the file's own name, how many steps it walks,
//!   the apps and hosts it names and when it was last written — a Flow an
//!   agent saved a minute ago under a familiar title does not read as the one
//!   the person knows;
//! - the one job this road writes is a Flow the window walks itself: no
//!   agent, no command and no precheck (`Automation::asked_flow`), which a
//!   request has no word for;
//! - a Flow that may spend — policy `guarded`, its money step pressed past
//!   the gate alone — may be asked for too (the person's answer, 2026-11-12),
//!   and its card says so in plain words, not by the policy's name alone;
//! - it fires no closer together than [`ASKED_SCHEDULE_FLOOR_MINUTES`];
//! - the folder it would run in is the caller's to name and cannot be cut or
//!   cleaned and still be that folder, so a path the card's rule would show
//!   differently is refused before any card;
//! - a no and a silence write nothing, and a Flow document that changed while
//!   the card stood is not the one that was allowed ([`settle`]);
//! - after two answers in a row that were not a yes, requests rest: for
//!   [`ASKED_REST_MINUTES`] every one is refused before any card ([`Rest`]),
//!   so a card that keeps coming back does not get pressed to make it stop.
//!
//! The window cannot tell a job somebody wrote straight into its saved file
//! from one it was asked for; this road closes the asking, not the file.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use serde::Serialize;
use serde_json::{Map, Value, json};
use zerocode_core::automation::{ASKED_SCHEDULE_FLOOR_MINUTES, Asked, flow_document_sha256};
use zerocode_core::card_words::{self, WORD_MAX_CHARS};
use zerocode_core::computer_flow::{FlowSpec, Policy, parse_flow};
use zerocode_core::computer_recipe::recipe_lines;
use zerocode_core::computer_use::{COMPUTER_CLI, recipe_slug};
use zerocode_core::computer_use_protocol::error_code;
use zerocode_core::{Automation, Cron, LocalMinute, Schedule};

use super::super::ComputerUseError;
use super::super::confirm::Decision;
use super::{recipe_title, show};

/// The page's words for the card (`COMPUTER_HANDOFF_WORDS`, ui/shell-computer.js).
pub(crate) const ASK_KEY: &str = "auto.asked.card";
/// How many of the coming runs the card names.
const CARD_RUNS: usize = 3;
/// How many of the sites, and how many of the apps, a document names the card
/// repeats before it only counts the rest.
const ABOUT_NAMES: usize = 3;

/// One kind of name a document's fingerprint holds — its sites, or its apps:
/// the first [`ABOUT_NAMES`] as the card's rule shows them, and how many it
/// names in all, so what is not shown is still counted, each kind apart.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub(crate) struct Named {
    pub shown: Vec<String>,
    pub count: usize,
}

/// Which document a card or a question is about, beyond the title the document
/// gives itself — all of it off the one text the hash is of. What a document
/// says of itself is data an agent may have written: its names are shown by
/// the card's rule (`card_words`), `hidden` counts the letters that rule left
/// out, and the page says they are the document's own words.
///
/// When the file was last written is not here: a process of the same user sets
/// that time to what it likes, and a card must not offer it as a ground.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct About {
    /// The file's own name without `.md`: what the recipe store calls it, as
    /// the card's rule shows it.
    pub slug: String,
    /// How many steps it walks.
    pub steps: usize,
    /// The sites its fingerprint names — said first: a walk is refused a page
    /// action on any other.
    pub hosts: Named,
    /// The apps its fingerprint names.
    pub apps: Named,
    /// Letters that draw nothing, left out of the slug and the names.
    pub hidden: usize,
}

impl About {
    /// What `spec` and `text` — one reading of the document kept under `slug`
    /// — say it is.
    ///
    /// # Errors
    ///
    /// When the steps do not read: a document that cannot be walked is not
    /// one to ask about.
    fn read(slug: &str, text: &str, spec: &FlowSpec) -> Result<Self, String> {
        let steps = recipe_lines(text)?.len();
        let mut hidden = 0;
        let mut named = |names: &BTreeSet<String>| Named {
            shown: names
                .iter()
                .take(ABOUT_NAMES)
                .map(|name| {
                    let shown = card_words::shown_counting(name, WORD_MAX_CHARS);
                    hidden += shown.hidden;
                    shown.text
                })
                .collect(),
            count: names.len(),
        };
        let hosts = named(&spec.fingerprint.hosts);
        let apps = named(&spec.fingerprint.apps);
        let slug = card_words::shown_counting(slug, WORD_MAX_CHARS);
        Ok(Self {
            slug: slug.text,
            steps,
            hosts,
            apps,
            hidden: hidden + slug.hidden,
        })
    }

    /// The window's own words for it, for a page that does not know the card.
    fn said(&self) -> String {
        let named = |what: &str, names: &Named| {
            if names.count == 0 {
                return String::new();
            }
            let more = names.count.saturating_sub(names.shown.len());
            let rest = if more == 0 {
                String::new()
            } else {
                format!(" and {more} more")
            };
            format!(
                " It names {} {what}: {}{rest}.",
                names.count,
                names.shown.join(", ")
            )
        };
        format!(
            " The document is {}.md: {} step(s).{}{}",
            self.slug,
            self.steps,
            named("site(s)", &self.hosts),
            named("app(s)", &self.apps)
        )
    }
}

/// The turn to ask: held while a schedule's card stands, so a second request
/// is refused instead of queued behind it — the person answers one such
/// question at a time, about one job.
#[derive(Debug)]
pub(crate) struct Turn(());

static ASKING: AtomicBool = AtomicBool::new(false);

impl Turn {
    /// `None` while another request holds it.
    #[must_use]
    pub(crate) fn take() -> Option<Self> {
        ASKING
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
            .then_some(Self(()))
    }
}

impl Drop for Turn {
    fn drop(&mut self) {
        ASKING.store(false, Ordering::SeqCst);
    }
}

/// How many answers in a row that were not a yes — a no, or a silence — it
/// takes before requests rest. One is let pass: a no pressed by mistake is
/// mended by asking once more.
pub(crate) const ASKED_NOS_BEFORE_REST: u32 = 2;
/// How long requests rest after that many (the person's answer, 2026-11-12).
pub(crate) const ASKED_REST_MINUTES: i64 = 10;
/// The milliseconds in a minute: the rest is kept in the first and said in
/// the second.
const MS_PER_MINUTE: i64 = 60_000;

/// The answers that were not a yes, in a row, and the rest they bring. Kept
/// for the whole window and not per pane: the pane a request names is the
/// caller's own word (`run_evidence::actor_pane`), and a rest kept by that
/// name is stepped around by naming another.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Rest {
    /// Nos and silences since the last yes or the last rest.
    nos: u32,
    /// Until when requests are refused before any card, by the clock the
    /// rest is kept by ([`rest_clock_ms`]).
    until_ms: i64,
}

impl Rest {
    /// How long a request made at `now_ms` still has to wait, when requests
    /// rest: it is refused before any card.
    #[must_use]
    pub(crate) fn left_ms(self, now_ms: i64) -> Option<i64> {
        (now_ms < self.until_ms).then_some(self.until_ms - now_ms)
    }

    /// What an answer given at `now_ms` makes of it: a yes forgets every no;
    /// the no or the silence that makes [`ASKED_NOS_BEFORE_REST`] in a row
    /// starts the rest, and the count starts over after it.
    #[must_use]
    pub(crate) fn after(self, decision: Decision, now_ms: i64) -> Self {
        match decision {
            Decision::Allowed => Self::default(),
            Decision::Refused | Decision::TimedOut => {
                let nos = self.nos + 1;
                if nos >= ASKED_NOS_BEFORE_REST {
                    Self {
                        nos: 0,
                        until_ms: now_ms + ASKED_REST_MINUTES * MS_PER_MINUTE,
                    }
                } else {
                    Self { nos, ..self }
                }
            }
        }
    }
}

static REST: Mutex<Rest> = Mutex::new(Rest {
    nos: 0,
    until_ms: 0,
});

/// The clock the rest is kept by: the milliseconds this window has run, off a
/// clock that only runs forward. Kept by the wall clock, a rest grew by
/// however far that clock was set back while it stood.
fn rest_clock_ms() -> i64 {
    static STARTED: OnceLock<Instant> = OnceLock::new();
    i64::try_from(STARTED.get_or_init(Instant::now).elapsed().as_millis()).unwrap_or(i64::MAX)
}

/// Refuse a request made while requests rest ([`Rest`]) — before the request
/// is read, so nothing is shown and nothing about it is answered.
///
/// # Errors
///
/// When the person's last two answers were not a yes and the rest they
/// brought is not over.
pub(crate) fn rested() -> Result<(), ComputerUseError> {
    let rest = *REST
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    match rest.left_ms(rest_clock_ms()) {
        None => Ok(()),
        Some(left_ms) => Err(ComputerUseError::new(
            error_code::CONFIRMATION_REFUSED,
            resting_said(left_ms),
        )),
    }
}

/// What a request refused for the rest is told: how long is left, and what to
/// do instead of asking again.
fn resting_said(left_ms: i64) -> String {
    // A started minute is said as a whole one.
    let minutes = (left_ms + MS_PER_MINUTE - 1) / MS_PER_MINUTE;
    format!(
        "the person's last {ASKED_NOS_BEFORE_REST} answers to a schedule request were a no or a silence, so no schedule is asked for during the next {minutes} minute(s). Do not ask again: tell the person what you wanted to schedule and why — they can add it themselves in the automation form"
    )
}

/// Keep the person's answer to a request's card ([`Rest::after`]).
pub(crate) fn answered(decision: Decision) {
    let mut rest = REST
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    *rest = rest.after(decision, rest_clock_ms());
}

/// A request read and checked: the job as it will be saved — which is what the
/// card is drawn from — and what the card says beside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Request {
    pub job: Automation,
    /// The Flow's own title, as its document says it.
    pub flow_name: String,
    /// The Flow's policy, in its document's word.
    pub policy: &'static str,
    /// Whether walking the Flow may spend money ([`spends`]): the card says
    /// so in the person's words.
    pub money: bool,
    /// The coming runs the card names, soonest first.
    pub next: Vec<LocalMinute>,
    /// How many letters that draw nothing were left out of the label, the
    /// Flow's title and the names its document gives
    /// (`card_words::shown_counting`): the card says so.
    pub hidden: usize,
    /// Which document this is, beyond its title.
    pub about: About,
    /// The Flow's document, read again once the person has answered.
    document: PathBuf,
}

/// Read a request: the Flow it names, when it would run, where, and from which
/// pane. What is refused here is refused before any card is shown.
///
/// # Errors
///
/// When the name is no Flow, the schedule does not read or fires closer
/// together than the floor, or the request came from no pane or no folder.
pub(crate) fn read(
    params: &Map<String, Value>,
    recipes_root: &Path,
    workspace: Option<&Path>,
    pane: Option<String>,
    now: LocalMinute,
    id: String,
) -> Result<Request, ComputerUseError> {
    let word = |key: &str| params.get(key).and_then(Value::as_str);
    let named = word("name").unwrap_or_default();
    let (document, text) = show(recipes_root, named)?;
    let slug = recipe_slug(named);
    // The reading every road that asks about a Flow shares ([`Standing::of`]):
    // what the card says of the document is what the list and 「다시 승인」
    // would say of it.
    let stands = Standing::of(&slug, &text).map_err(|why| {
        ComputerUseError::invalid_argument(match why {
            NotStanding::Recipe => format!(
                "'{named}' is a recipe, not a Flow — only a Flow is judged after it runs. Stand it up first: `{COMPUTER_CLI} recipe-save --name <name> --flow-check <text the screen shows when it worked>`"
            ),
            NotStanding::Flow(said) => {
                format!("the Flow section of '{named}' does not read: {said}")
            }
            NotStanding::Steps(said) => format!("the steps of '{named}' do not read: {said}"),
        })
    })?;
    let workspace = workspace.filter(|folder| folder.is_dir()).ok_or_else(|| {
        ComputerUseError::invalid_argument(
            "a schedule runs in the folder it was asked from, and this request came from none: ask from the pane's own shell",
        )
    })?;
    // The card shows the folder as it is: one whose path the card's rule would
    // show differently could make the card read as something it does not say.
    let folder = workspace.display().to_string();
    if card_words::shown(&folder, usize::MAX) != folder {
        return Err(ComputerUseError::invalid_argument(
            "a schedule is not asked from this folder: its path holds what a card cannot show as it is — a letter that draws nothing (a control character, a mark that turns the reading direction, a zero-width or variation mark) or white space other than single spaces. The person can schedule the Flow there themselves, in the automation form",
        ));
    }
    let pane = pane.ok_or_else(|| {
        ComputerUseError::invalid_argument(
            "a schedule is asked from a pane of this window, so the person is shown who asked",
        )
    })?;
    let schedule = Schedule::from_words(word("every"), word("at"), word("day"), word("cron"))
        .map_err(ComputerUseError::invalid_argument)?;
    let cron = Cron::parse(&schedule.to_cron())
        .ok_or_else(|| ComputerUseError::invalid_argument("the schedule does not read"))?;
    if let Some(gap) = cron
        .shortest_gap_minutes(now)
        .filter(|gap| *gap < ASKED_SCHEDULE_FLOOR_MINUTES)
    {
        return Err(ComputerUseError::invalid_argument(format!(
            "a schedule that is asked for fires no closer together than {ASKED_SCHEDULE_FLOOR_MINUTES} minutes; this one fires {gap} apart. The person can set a tighter one themselves, in the automation form"
        )));
    }
    let next = coming(&cron, now);
    if next.is_empty() {
        return Err(ComputerUseError::invalid_argument(
            "this schedule names no minute in the year ahead",
        ));
    }
    let label = word("label").map(|label| card_words::shown_counting(label, WORD_MAX_CHARS));
    let hidden = stands.hidden + label.as_ref().map_or(0, |label| label.hidden);
    let name = label
        .map(|label| label.text)
        .filter(|label| !label.is_empty())
        .unwrap_or_else(|| stands.title.clone());
    let mut job = Automation::asked_flow(
        id,
        name,
        folder,
        slug,
        schedule,
        Asked {
            pane,
            agent: None,
            // Written when the person answers ([`settle`]).
            approved_at_ms: 0,
            flow_sha256: stands.sha256,
        },
    );
    // Checked as the save door will check it, so no card is shown for a job
    // that could not be saved.
    job.bind_flow()
        .and_then(|()| job.validate_for_save())
        .map_err(ComputerUseError::invalid_argument)?;
    Ok(Request {
        job,
        flow_name: stands.title,
        policy: stands.policy,
        money: stands.money,
        next,
        hidden,
        about: stands.about,
        document,
    })
}

/// Whether a Flow of `policy` may spend money when it is walked: its money
/// step is pressed past the gate alone. Such a Flow may be scheduled by
/// asking, and the card of the request says it. Every policy is answered
/// here, so a new one is decided before a card can be silent about it.
const fn spends(policy: Policy) -> bool {
    match policy {
        Policy::Dry => false,
        Policy::Guarded => true,
    }
}

/// The hash of a Flow's document as it stands under `recipes_root`, or `None`
/// when it cannot be read: what an approval is held against before a walk and
/// on the list.
#[must_use]
pub(crate) fn document_sha256(recipes_root: &Path, slug: &str) -> Option<String> {
    show(recipes_root, slug)
        .ok()
        .map(|(_, text)| flow_document_sha256(&text))
}

/// A Flow's document as one reading of it stands: the hash an approval is held
/// against, and what a question about allowing it has to say — both off the
/// same text, so the person is not asked about one document and answered for
/// another.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Standing {
    /// The hash of the text that was read (`flow_document_sha256`).
    pub sha256: String,
    /// The title the document gives itself — its file's own name when it
    /// gives none — as the card's rule shows it. What is asked names the Flow
    /// by the reading it is held to, not by a list read at another time.
    pub title: String,
    /// The Flow's policy, in its document's word.
    pub policy: &'static str,
    /// Whether walking it may spend money ([`spends`]): the question says so.
    pub money: bool,
    /// Which document it is, beyond its title: the question says that too.
    pub about: About,
    /// Letters that draw nothing, left out of the title, the file's name and
    /// the names the document gives (`card_words::shown_counting`): what is
    /// asked says how many.
    pub hidden: usize,
}

/// Why a document does not stand as a Flow to ask about.
#[derive(Debug, Clone, PartialEq, Eq)]
enum NotStanding {
    /// A recipe that was never stood up as a Flow.
    Recipe,
    /// Its Flow section does not read.
    Flow(String),
    /// Its steps do not read.
    Steps(String),
}

impl Standing {
    /// What `text` — one reading of the document kept under `slug` — stands
    /// as. The card of a request, the list, 「다시 승인」 and a form's save
    /// all read a Flow here, so none of them says of a document what another
    /// would not.
    fn of(slug: &str, text: &str) -> Result<Self, NotStanding> {
        let spec = match parse_flow(text) {
            Ok(Some(spec)) => spec,
            Ok(None) => return Err(NotStanding::Recipe),
            Err(why) => return Err(NotStanding::Flow(why)),
        };
        let about = About::read(slug, text, &spec).map_err(NotStanding::Steps)?;
        let title = card_words::shown_counting(
            &recipe_title(text).unwrap_or_else(|| slug.to_string()),
            WORD_MAX_CHARS,
        );
        // A title of nothing but letters that draw nothing shows as no title
        // at all: the Flow is then named by its file, as one that gives none
        // is, and the letters left out are still counted.
        let shown = if title.text.is_empty() {
            card_words::shown(slug, WORD_MAX_CHARS)
        } else {
            title.text
        };
        Ok(Self {
            sha256: flow_document_sha256(text),
            title: shown,
            policy: spec.policy.as_str(),
            money: spends(spec.policy),
            hidden: title.hidden + about.hidden,
            about,
        })
    }
}

/// The document of `slug` under `recipes_root` as it stands, read once.
/// `None` when it cannot be read or no longer reads as a Flow with steps that
/// read: there is then nothing to allow — a walk would refuse it too.
#[must_use]
pub(crate) fn standing(recipes_root: &Path, slug: &str) -> Option<Standing> {
    let (_, text) = show(recipes_root, slug).ok()?;
    Standing::of(&recipe_slug(slug), &text).ok()
}

/// What 「다시 승인」 is refused with when the document is no longer the one
/// the question was asked about ([`allowed_again`]).
pub(crate) const FLOW_CHANGED_UNDER_QUESTION: &str =
    "묻는 동안 Flow 문서가 다시 바뀌었습니다 — 문서를 확인한 뒤 다시 승인해 주세요";

/// What 「다시 승인」 is refused with when there is no Flow to allow.
pub(crate) const FLOW_NOT_A_FLOW: &str =
    "Flow 문서를 읽을 수 없거나 더는 Flow가 아닙니다 — Flow 탭에서 문서를 확인해 주세요";

/// The person's yes to allowing a job's Flow again, held to the document the
/// question was asked about: `seen` is the hash the list read and the question
/// was drawn from, `stands` the document as it reads at the press. The hash an
/// approval may move to is answered only when the two are one document — the
/// one that was compared is the one that is stored.
///
/// # Errors
///
/// When there is no Flow to allow, or the document changed under the question.
pub(crate) fn allowed_again<'a>(
    stands: Option<&'a Standing>,
    seen: &str,
) -> Result<&'a str, &'static str> {
    let stands = stands.ok_or(FLOW_NOT_A_FLOW)?;
    if stands.sha256 == seen {
        Ok(stands.sha256.as_str())
    } else {
        Err(FLOW_CHANGED_UNDER_QUESTION)
    }
}

/// What a job says when the Flow it was allowed for reads differently: it
/// waits for the person instead of walking it.
pub(crate) const FLOW_CHANGED: &str =
    "승인한 뒤 Flow 문서가 바뀌었습니다 — 자동화 화면에서 다시 승인해 주세요";

/// Whether `text` — a Flow's document as a walk read it, the very text it is
/// about to walk — is the one the walk is held to: `held` is the SHA-256 the
/// window says with the walk it starts for a schedule an agent asked for
/// (`FLOW_SHA256_FLAG`), and `None` for every other walk. Judged here, on the
/// walk's own reading, so no check stands a moment before the walk for the
/// document to change in.
///
/// # Errors
///
/// When the walk is held to another document than the one it read.
pub(crate) fn held_to(text: &str, held: Option<&str>) -> Result<(), ComputerUseError> {
    match held {
        Some(sha256) if flow_document_sha256(text) != sha256 => {
            Err(ComputerUseError::invalid_argument(FLOW_CHANGED))
        }
        _ => Ok(()),
    }
}

/// The next [`CARD_RUNS`] minutes the expression names after `now`.
fn coming(cron: &Cron, now: LocalMinute) -> Vec<LocalMinute> {
    let mut from = now.epoch_minutes + 1;
    let mut found = Vec::with_capacity(CARD_RUNS);
    while found.len() < CARD_RUNS {
        let Some(at) = cron.next_after(LocalMinute {
            epoch_minutes: from,
        }) else {
            break;
        };
        from = at.epoch_minutes + 1;
        found.push(at);
    }
    found
}

/// The card for a request: the window's sentence, for a page that does not
/// know [`ASK_KEY`], and what fills the page's own — every value read off the
/// job that will be saved.
#[must_use]
pub(crate) fn card(request: &Request) -> (String, Value) {
    let job = &request.job;
    let pane = job.asked.as_ref().map_or("", |asked| asked.pane.as_str());
    let cron = job.schedule.to_cron();
    let args = json!({
        "flow": request.flow_name,
        "name": job.name,
        "policy": request.policy,
        "schedule": job.schedule,
        "cron": cron,
        "next": minutes(&request.next),
        "workspace": job.workspace,
        "pane": pane,
        "hidden": request.hidden,
        "money": request.money,
        "about": request.about,
    });
    let sentence = format!(
        "An agent in {pane} asks to schedule the Flow \"{flow}\" (policy {policy}) as \"{name}\": cron `{cron}`, in \"{workspace}\". The window would walk it by itself, with nobody watching. Allow it only if that is what you want; cancel it, or leave it, and nothing is scheduled.{spending}{about}{left_out}",
        about = request.about.said(),
        flow = request.flow_name,
        policy = request.policy,
        name = job.name,
        workspace = job.workspace,
        spending = if request.money {
            " This Flow may spend money: its money step is pressed with nobody there to stop it."
        } else {
            ""
        },
        left_out = card_words::hidden_said(request.hidden)
            .map(|said| format!(" ({said}.)"))
            .unwrap_or_default(),
    );
    (sentence, args)
}

fn minutes(runs: &[LocalMinute]) -> Vec<i64> {
    runs.iter().map(|at| at.epoch_minutes).collect()
}

/// What the person's answer makes of a request. Allowed, the job is saved
/// through `save` — the door every writer of a job uses — unless the Flow's
/// document no longer reads as it did when the card was drawn. Refused or
/// unanswered, nothing is written.
///
/// # Errors
///
/// The person's no, their silence, a document that changed under the card, or
/// the save door's own refusal.
pub(crate) fn settle(
    request: Request,
    decision: Decision,
    now_epoch_ms: i64,
    save: impl FnOnce(Automation) -> Result<(), String>,
) -> Result<Value, ComputerUseError> {
    match decision {
        Decision::Allowed => {}
        Decision::Refused => {
            return Err(ComputerUseError::new(
                error_code::CONFIRMATION_REFUSED,
                "the person said no; nothing was scheduled. Do not ask again.",
            ));
        }
        Decision::TimedOut => {
            return Err(ComputerUseError::new(
                error_code::CONFIRMATION_TIMEOUT,
                "nobody answered the card; nothing was scheduled. Tell the person what you wanted to schedule and why.",
            ));
        }
    }
    let Request {
        mut job,
        next,
        document,
        ..
    } = request;
    let stands = std::fs::read_to_string(&document)
        .ok()
        .map(|text| flow_document_sha256(&text));
    if job.flow_changed_since_approval(stands.as_deref()) {
        return Err(ComputerUseError::invalid_argument(
            "the Flow's document changed while the card stood, so what the person allowed is not what is there now. Nothing was scheduled; ask again.",
        ));
    }
    if let Some(asked) = job.asked.as_mut() {
        asked.approved_at_ms = u64::try_from(now_epoch_ms).unwrap_or_default();
    }
    let answer = json!({
        "scheduled": true,
        "id": job.id,
        "name": job.name,
        "flow": job.flow,
        "cron": job.schedule.to_cron(),
        "next": minutes(&next),
    });
    save(job).map_err(ComputerUseError::invalid_argument)?;
    Ok(answer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    use zerocode_core::computer_flow::{
        FLOW_HEADING, FLOW_HEADING_CHECKS, FLOW_KEY_MONEY, FLOW_MONEY_AMOUNT, FLOW_MONEY_ID,
        FLOW_MONEY_RECIPIENT,
    };
    use zerocode_core::computer_recipe::{
        RECIPE_HEADING_STEPS, RECIPE_MARK_MONEY, RECIPE_MARK_SEPARATOR,
    };
    use zerocode_core::computer_use::COMPUTER_USE_PROTOCOL_VERSION;

    /// 2026-11-11 00:00, local: a Wednesday.
    const NOW: LocalMinute = LocalMinute {
        epoch_minutes: 29_905_920,
    };

    /// A Flow as `recipe-save --flow-check` leaves one: steps, the Flow
    /// section, one required check.
    fn flow_document() -> String {
        format!(
            "# Wallet QA\n\n{RECIPE_HEADING_STEPS}\n\n\
             1. `zerocode-computer launch --app \"Sample Wallet\"`\n\n\
             {FLOW_HEADING}\n\n\
             - policy: dry\n\
             - evidence: full\n\
             - fingerprint: apps=com.example.wallet hosts=shop.example.com protocol={COMPUTER_USE_PROTOCOL_VERSION}\n\n\
             {FLOW_HEADING_CHECKS}\n\n\
             1. `zerocode-computer wait-for --app \"Sample Wallet\" --text Balance` — event, required\n"
        )
    }

    /// A Flow that may spend: policy `guarded`, a money line, and the one step
    /// that carries the money mark.
    fn spending_document() -> String {
        format!(
            "# Wallet top-up\n\n{RECIPE_HEADING_STEPS}\n\n\
             1. `zerocode-browser type browser-1 #recipient {{{{recipient}}}}`\n\
             2. `zerocode-browser type browser-1 #amount {{{{amount}}}}`\n\
             3. `zerocode-browser click browser-1 #send`{RECIPE_MARK_SEPARATOR}{RECIPE_MARK_MONEY}\n\n\
             {FLOW_HEADING}\n\n\
             - policy: guarded\n\
             - evidence: full\n\
             - fingerprint: hosts=shop.example.com protocol={COMPUTER_USE_PROTOCOL_VERSION}\n\
             - {FLOW_KEY_MONEY}: {FLOW_MONEY_ID}=txn {FLOW_MONEY_AMOUNT}=amount {FLOW_MONEY_RECIPIENT}=recipient\n\n\
             {FLOW_HEADING_CHECKS}\n\n\
             1. `zerocode-browser find browser-1 \"Sent\"` — event, required\n"
        )
    }

    /// What a card says a document is: its sites, then its apps, every one shown.
    fn about(slug: &str, steps: usize, hosts: &[&str], apps: &[&str]) -> About {
        let named = |names: &[&str]| Named {
            shown: names.iter().map(ToString::to_string).collect(),
            count: names.len(),
        };
        About {
            slug: slug.to_string(),
            steps,
            hosts: named(hosts),
            apps: named(apps),
            hidden: 0,
        }
    }

    /// A recipe that was never stood up as a Flow.
    fn plain_document() -> String {
        format!(
            "# Plain steps\n\n{RECIPE_HEADING_STEPS}\n\n\
             1. `zerocode-computer launch --app \"Sample Wallet\"`\n"
        )
    }

    fn stand(documents: &[(&str, &str)]) -> (tempfile::TempDir, PathBuf) {
        let root = tempfile::tempdir().expect("a root");
        let recipes = root.path().join(super::super::RECIPES_DIR);
        std::fs::create_dir_all(&recipes).expect("the recipes folder");
        for (slug, text) in documents {
            std::fs::write(recipes.join(format!("{slug}.md")), text).expect("a document");
        }
        let workspace = root.path().join("repo");
        std::fs::create_dir_all(&workspace).expect("a workspace");
        (root, workspace)
    }

    fn params(pairs: &[(&str, &str)]) -> Map<String, Value> {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), Value::String((*value).to_string())))
            .collect()
    }

    fn asked(
        root: &Path,
        workspace: Option<&Path>,
        pairs: &[(&str, &str)],
    ) -> Result<Request, ComputerUseError> {
        read(
            &params(pairs),
            root,
            workspace,
            Some("term:7".to_string()),
            NOW,
            "job-1".to_string(),
        )
    }

    /// The fixture is what it says: a Flow, and a recipe that is not one.
    #[test]
    fn the_fixture_documents_read_as_a_flow_and_as_a_plain_recipe() {
        let (flow, plain) = (flow_document(), plain_document());
        assert!(
            matches!(parse_flow(&flow), Ok(Some(_))),
            "{:?}",
            parse_flow(&flow)
        );
        assert!(
            matches!(parse_flow(&plain), Ok(None)),
            "{:?}",
            parse_flow(&plain)
        );
        assert_eq!(recipe_title(&flow).as_deref(), Some("Wallet QA"));
        let spending = spending_document();
        assert!(
            matches!(parse_flow(&spending), Ok(Some(ref spec)) if spec.policy == Policy::Guarded),
            "{:?}",
            parse_flow(&spending)
        );
    }

    /// One no is let pass — a no pressed by mistake is mended by asking once
    /// more. The second answer in a row that is not a yes starts the rest:
    /// for ten minutes a request is refused before any card, and told how long
    /// is left. A yes in between forgets the no before it.
    #[test]
    fn requests_rest_after_two_answers_in_a_row_that_were_not_a_yes() {
        let start = 1_794_000_000_000_i64;
        let rest = Rest::default();
        assert_eq!(rest.left_ms(start), None);

        let once = rest.after(Decision::Refused, start);
        assert_eq!(once.left_ms(start), None, "one no already rested requests");

        // A silence counts as a no does.
        let twice = once.after(Decision::TimedOut, start + 1_000);
        let whole = ASKED_REST_MINUTES * MS_PER_MINUTE;
        assert_eq!(twice.left_ms(start + 1_000), Some(whole));
        assert_eq!(twice.left_ms(start + 1_000 + whole - 1), Some(1));
        assert_eq!(twice.left_ms(start + 1_000 + whole), None);
        // After the rest the count starts over: one more no does not rest again.
        let later = start + 1_000 + whole;
        assert_eq!(twice.after(Decision::Refused, later).left_ms(later), None);

        // A yes forgets the no before it.
        let mended = once.after(Decision::Allowed, start + 2_000);
        assert_eq!(mended, Rest::default());
        assert_eq!(
            mended
                .after(Decision::Refused, start + 3_000)
                .left_ms(start + 3_000),
            None
        );

        // What a rested request is told: the minutes left, rounded up, and
        // what to do instead.
        let said = resting_said(whole - 30_000);
        assert!(
            said.contains("next 10 minute(s)") && said.contains("Do not ask again"),
            "{said}"
        );
        assert!(resting_said(1).contains("next 1 minute(s)"));
    }

    /// One reading of a Flow's document gives the hash an approval is held
    /// against and what a question about it says. A document that cannot be
    /// read, or is no Flow, stands as nothing: there is nothing to allow.
    #[test]
    fn a_flows_document_stands_as_its_hash_and_whether_it_may_spend() {
        let (flow, spending, plain) = (flow_document(), spending_document(), plain_document());
        let (root, _) = stand(&[
            ("wallet-qa", &flow),
            ("wallet-top-up", &spending),
            ("plain-steps", &plain),
        ]);
        assert_eq!(
            standing(root.path(), "wallet-qa"),
            Some(Standing {
                sha256: flow_document_sha256(&flow),
                title: "Wallet QA".to_string(),
                policy: "dry",
                money: false,
                about: about(
                    "wallet-qa",
                    1,
                    &["shop.example.com"],
                    &["com.example.wallet"]
                ),
                hidden: 0,
            })
        );
        assert_eq!(
            standing(root.path(), "wallet-top-up"),
            Some(Standing {
                sha256: flow_document_sha256(&spending),
                title: "Wallet top-up".to_string(),
                policy: "guarded",
                money: true,
                about: about("wallet-top-up", 3, &["shop.example.com"], &[]),
                hidden: 0,
            })
        );
        assert_eq!(standing(root.path(), "plain-steps"), None);
        assert_eq!(standing(root.path(), "nothing"), None);
    }

    /// The card of a request and the list read a document the same way: one
    /// reading ([`Standing::of`]) gives the hash, the title, whether it may
    /// spend and which document it is, to both. The title is the document's
    /// own word — shown by the card's rule, its letters that draw nothing
    /// counted — and a document that gives none is named by its file.
    #[test]
    fn a_request_and_the_list_say_the_same_of_one_document() {
        let spending = spending_document();
        let marked = flow_document().replace("# Wallet QA", "# Wallet\u{200B}\u{202E} QA");
        let untitled = flow_document().replace("# Wallet QA\n\n", "");
        // A title that is one letter that draws nothing (t-105889, T1).
        let blank = flow_document().replace("# Wallet QA", "# \u{200B}");
        let (root, workspace) = stand(&[
            ("wallet-top-up", &spending),
            ("marked", &marked),
            ("untitled", &untitled),
            ("blank-title", &blank),
        ]);
        for slug in ["wallet-top-up", "marked", "untitled", "blank-title"] {
            let stands = standing(root.path(), slug).expect("a Flow");
            let request = asked(
                root.path(),
                Some(&workspace),
                &[("name", slug), ("every", "daily")],
            )
            .expect("read");
            let held = request.job.asked.as_ref().expect("who asked");
            assert_eq!(
                (
                    held.flow_sha256.as_str(),
                    request.flow_name.as_str(),
                    request.policy,
                    request.money,
                    &request.about,
                    request.hidden,
                ),
                (
                    stands.sha256.as_str(),
                    stands.title.as_str(),
                    stands.policy,
                    stands.money,
                    &stands.about,
                    stands.hidden,
                ),
                "{slug}"
            );
        }
        let marked = standing(root.path(), "marked").expect("a Flow");
        assert_eq!((marked.title.as_str(), marked.hidden), ("Wallet QA", 2));
        assert_eq!(
            standing(root.path(), "untitled").expect("a Flow").title,
            "untitled"
        );
        let blank = standing(root.path(), "blank-title").expect("a Flow");
        assert_eq!(
            (blank.title.as_str(), blank.hidden),
            ("blank-title", 1),
            "a title that draws nothing left the Flow unnamed"
        );
        // One reading, in the source too: the card's road parses no Flow and
        // reads no title of its own.
        let source = include_str!("schedule.rs");
        let road = source
            .split_once("pub(crate) fn read(")
            .and_then(|(_, rest)| rest.split_once("\nconst fn spends("))
            .map(|(road, _)| road)
            .expect("the card's road");
        assert!(
            road.contains("Standing::of(&slug, &text)")
                && !road.contains("parse_flow(")
                && !road.contains("recipe_title(")
                && !road.contains("About::read("),
            "the card's road reads the document a second way"
        );
    }

    /// Which document a card is about is read off the document itself: its
    /// file's name, its steps, and the sites and the apps it names — sites
    /// first, each kind counted apart, the first few by name — shown by the
    /// card's rule. When the file was written is not said: a process of the
    /// same user sets that. And what may spend is said before any of it: the
    /// document's own words, however long it makes them, do not push that
    /// sentence down (t-105495, 막는 문제 1).
    #[test]
    fn a_card_says_which_document_it_is_about() {
        let crowded = flow_document().replace(
            "apps=com.example.wallet hosts=shop.example.com",
            "apps=com.example.a,com.example.b,com.example.c,com.example.d hosts=one.example.com,two.example.com,three.example.com,four.example.com,five.example.com",
        );
        let (root, workspace) = stand(&[("wallet-qa", &flow_document()), ("crowded", &crowded)]);
        let plain = asked(
            root.path(),
            Some(&workspace),
            &[("name", "wallet-qa"), ("every", "daily")],
        )
        .expect("read");
        assert_eq!(
            plain.about,
            about(
                "wallet-qa",
                1,
                &["shop.example.com"],
                &["com.example.wallet"]
            )
        );
        let (sentence, args) = card(&plain);
        assert!(
            sentence.contains(
                "The document is wallet-qa.md: 1 step(s). It names 1 site(s): shop.example.com. It names 1 app(s): com.example.wallet."
            ),
            "{sentence}"
        );
        assert_eq!(
            args["about"],
            json!({
                "slug": "wallet-qa",
                "steps": 1,
                "hosts": { "shown": ["shop.example.com"], "count": 1 },
                "apps": { "shown": ["com.example.wallet"], "count": 1 },
                "hidden": 0,
            }),
            "the card is told a file time again, or the names in another shape"
        );

        // More than the card repeats: the first few of each kind, in the
        // document's own order, and each kind's count — apps do not crowd the
        // sites out.
        let many = asked(
            root.path(),
            Some(&workspace),
            &[("name", "crowded"), ("every", "daily")],
        )
        .expect("read");
        assert_eq!(
            many.about.hosts,
            Named {
                shown: vec![
                    "five.example.com".to_string(),
                    "four.example.com".to_string(),
                    "one.example.com".to_string(),
                ],
                count: 5,
            }
        );
        assert_eq!(
            (many.about.apps.shown.len(), many.about.apps.count),
            (ABOUT_NAMES, 4)
        );
        let (sentence, _) = card(&many);
        assert!(
            sentence.contains("It names 5 site(s): five.example.com, four.example.com, one.example.com and 2 more.")
                && sentence.contains("It names 4 app(s): com.example.a, com.example.b, com.example.c and 1 more."),
            "{sentence}"
        );

        // What may spend comes before what the document says of itself.
        let spending = Request {
            money: spends(Policy::Guarded),
            policy: Policy::Guarded.as_str(),
            ..many.clone()
        };
        let (sentence, _) = card(&spending);
        let money = sentence
            .find("may spend money")
            .expect("the money sentence");
        let document = sentence.find("The document is").expect("the document");
        assert!(
            money < document,
            "the document's own words stand before the sentence that says it may spend: {sentence}"
        );

        // The file's name is shown by the card's rule too: a letter that draws
        // nothing is left out of it and counted with the others.
        let spec = parse_flow(&flow_document())
            .expect("reads")
            .expect("a Flow");
        let named = About::read("wallet\u{3164}qa", &flow_document(), &spec).expect("reads");
        assert_eq!((named.slug.as_str(), named.hidden), ("walletqa", 1));
    }

    /// 「다시 승인」 is the person's yes to the document the question was drawn
    /// from: the approval may move only to that one. A document that changed
    /// again while the question stood is refused — what the person read about
    /// is not what is there — and so is one that is no longer a Flow.
    #[test]
    fn allowing_again_is_held_to_the_document_the_question_was_asked_about() {
        let asked_about = Standing {
            sha256: flow_document_sha256(&flow_document()),
            title: "Wallet QA".to_string(),
            policy: "dry",
            money: false,
            about: about(
                "wallet-qa",
                1,
                &["shop.example.com"],
                &["com.example.wallet"],
            ),
            hidden: 0,
        };
        let changed_again = Standing {
            sha256: flow_document_sha256(&spending_document()),
            title: "Wallet top-up".to_string(),
            policy: "guarded",
            money: true,
            about: about("wallet-qa", 3, &["shop.example.com"], &[]),
            hidden: 0,
        };
        assert_eq!(
            allowed_again(Some(&asked_about), &asked_about.sha256),
            Ok(asked_about.sha256.as_str())
        );
        assert_eq!(
            allowed_again(Some(&changed_again), &asked_about.sha256),
            Err(FLOW_CHANGED_UNDER_QUESTION),
            "a document that changed under the question was allowed as if it had been read"
        );
        assert_eq!(
            allowed_again(None, &asked_about.sha256),
            Err(FLOW_NOT_A_FLOW)
        );
        // A page that sent no hash — an older one — allows nothing.
        assert_eq!(
            allowed_again(Some(&asked_about), ""),
            Err(FLOW_CHANGED_UNDER_QUESTION)
        );
    }

    /// The job is built before the card, and the card is drawn from it: a Flow
    /// the window walks itself, in the folder that asked, naming the pane.
    #[test]
    fn a_request_becomes_the_job_the_card_is_drawn_from() {
        let flow = flow_document();
        let (root, workspace) = stand(&[("wallet-qa", &flow)]);
        let request = asked(
            root.path(),
            Some(&workspace),
            &[
                ("name", "Wallet QA"),
                ("every", "daily"),
                ("at", "09:30"),
                ("label", "Morning wallet"),
            ],
        )
        .expect("read");
        let job = &request.job;
        assert!(job.runs_flow_directly() && job.enabled);
        assert_eq!(
            (
                job.name.as_str(),
                job.flow.as_deref(),
                job.schedule.to_cron().as_str()
            ),
            ("Morning wallet", Some("wallet-qa"), "30 9 * * *")
        );
        assert_eq!(job.workspace, workspace.display().to_string());
        assert!(job.precheck.is_empty() && job.command.is_empty() && job.agent.is_none());
        let held = job.asked.as_ref().expect("who asked");
        assert_eq!(
            (
                held.pane.as_str(),
                held.approved_at_ms,
                held.flow_sha256.as_str()
            ),
            ("term:7", 0, flow_document_sha256(&flow).as_str())
        );
        assert_eq!(
            (request.flow_name.as_str(), request.policy),
            ("Wallet QA", "dry")
        );
        // Three runs, a day apart, the first this morning.
        assert_eq!(
            minutes(&request.next),
            [
                NOW.epoch_minutes + 570,
                NOW.epoch_minutes + 2010,
                NOW.epoch_minutes + 3450
            ]
        );

        let (sentence, args) = card(&request);
        for shown in [
            "Wallet QA",
            "Morning wallet",
            "30 9 * * *",
            "term:7",
            "nobody watching",
        ] {
            assert!(sentence.contains(shown), "{shown}: {sentence}");
        }
        assert_eq!(args["name"], job.name);
        assert_eq!(args["flow"], "Wallet QA");
        assert_eq!(args["policy"], "dry");
        assert_eq!(args["cron"], "30 9 * * *");
        assert_eq!(args["workspace"], job.workspace);
        assert_eq!(args["pane"], "term:7");
        assert_eq!(
            args["schedule"],
            serde_json::to_value(&job.schedule).expect("schedule")
        );
        assert_eq!(args["next"], json!(minutes(&request.next)));
        // A Flow that may spend is said so on the card — in the window's
        // sentence and to the page — and a dry one is not.
        assert!(!request.money && args["money"] == false);
        assert!(!sentence.contains("may spend money"), "{sentence}");
        let spending = Request {
            money: spends(Policy::Guarded),
            policy: Policy::Guarded.as_str(),
            ..request.clone()
        };
        let (sentence, args) = card(&spending);
        assert_eq!(args["money"], true);
        assert!(
            sentence.contains("policy guarded") && sentence.contains("may spend money"),
            "{sentence}"
        );
        assert!(!spends(Policy::Dry));
        // Without a label the job is listed under the Flow's own title.
        let unnamed = asked(
            root.path(),
            Some(&workspace),
            &[("name", "wallet-qa"), ("every", "hourly")],
        )
        .expect("read");
        assert_eq!(unnamed.job.name, "Wallet QA");
    }

    /// Refused before any card: a recipe that is no Flow, a name that is
    /// nothing, a schedule below the floor, and a request from nowhere.
    #[test]
    fn what_cannot_be_scheduled_is_refused_before_a_card() {
        let (flow, plain) = (flow_document(), plain_document());
        let (root, workspace) = stand(&[("wallet-qa", &flow), ("plain-steps", &plain)]);
        let refused = |workspace: Option<&Path>, pairs: &[(&str, &str)]| {
            asked(root.path(), workspace, pairs)
                .expect_err("refused")
                .message
        };
        assert!(
            refused(
                Some(&workspace),
                &[("name", "plain-steps"), ("every", "daily")]
            )
            .contains("not a Flow")
        );
        assert!(
            refused(Some(&workspace), &[("name", "nothing"), ("every", "daily")])
                .contains("no recipe")
        );
        assert!(
            refused(
                Some(&workspace),
                &[("name", "wallet-qa"), ("cron", "*/5 * * * *")]
            )
            .contains("no closer together than")
        );
        assert!(
            refused(
                Some(&workspace),
                &[("name", "wallet-qa"), ("cron", "0,1 9 * * *")]
            )
            .contains("fires 1 apart")
        );
        assert!(
            asked(
                root.path(),
                Some(&workspace),
                &[("name", "wallet-qa"), ("cron", "*/15 * * * *")]
            )
            .is_ok()
        );
        assert!(refused(None, &[("name", "wallet-qa"), ("every", "daily")]).contains("folder"));
        assert!(
            refused(
                Some(&root.path().join("gone")),
                &[("name", "wallet-qa"), ("every", "daily")]
            )
            .contains("folder")
        );
        // A folder the card could not show as it is: asked from there, refused.
        for odd in ["re\u{202E}po", "two  spaces"] {
            let folder = root.path().join(odd);
            std::fs::create_dir_all(&folder).expect("an oddly named folder");
            assert!(
                refused(Some(&folder), &[("name", "wallet-qa"), ("every", "daily")])
                    .contains("cannot show as it is"),
                "{odd:?}"
            );
        }
        let no_pane = read(
            &params(&[("name", "wallet-qa"), ("every", "daily")]),
            root.path(),
            Some(&workspace),
            None,
            NOW,
            "job-1".to_string(),
        )
        .expect_err("no pane");
        assert!(no_pane.message.contains("pane"));
    }

    /// One schedule is asked about at a time: the turn is held while a card
    /// stands and given back when it is answered.
    #[test]
    fn a_second_request_is_refused_while_the_first_waits_for_the_person() {
        let first = Turn::take().expect("the first asks");
        assert!(Turn::take().is_none());
        drop(first);
        assert!(Turn::take().is_some());
    }

    /// What an agent gave is data: cut, and without the characters that could
    /// make a card read as something else.
    #[test]
    fn a_word_that_is_not_the_windows_is_cut_and_cleaned() {
        // The rule itself is `card_words`' and tested there; here, that a
        // name is held to it at a name's length.
        let said = |text: &str| card_words::shown(text, WORD_MAX_CHARS);
        assert_eq!(said("  Morning\twallet\n check "), "Morning wallet check");
        assert_eq!(said("pay\u{202E}dleh\u{2066}x\u{2069}\u{200F}"), "paydlehx");
        assert_eq!(
            said(&"가".repeat(WORD_MAX_CHARS + 20)).chars().count(),
            WORD_MAX_CHARS
        );
        let flow = flow_document();
        let (root, workspace) = stand(&[("wallet-qa", &flow)]);
        let request = asked(
            root.path(),
            Some(&workspace),
            &[
                ("name", "wallet-qa"),
                ("every", "daily"),
                ("label", "\u{202E}\u{0007}"),
            ],
        )
        .expect("read");
        assert_eq!(
            request.job.name, "Wallet QA",
            "a label of nothing is no label"
        );
        // Letters that draw nothing are left out of the name, and the card
        // says how many — in its sentence and in what fills the page's own.
        assert_eq!(request.hidden, 2);
        let (sentence, args) = card(&request);
        assert_eq!(args["hidden"], 2);
        assert!(
            sentence.contains("2 characters that draw nothing were left out"),
            "{sentence}"
        );
        let spelled = asked(
            root.path(),
            Some(&workspace),
            &[
                ("name", "wallet-qa"),
                ("every", "daily"),
                ("label", "Morning\u{200B}\u{E0041}\u{E0042} wallet"),
            ],
        )
        .expect("read");
        assert_eq!(
            (spelled.job.name.as_str(), spelled.hidden),
            ("Morning wallet", 3)
        );
        let plain = asked(
            root.path(),
            Some(&workspace),
            &[
                ("name", "wallet-qa"),
                ("every", "daily"),
                ("label", "Morning wallet"),
            ],
        )
        .expect("read");
        let (sentence, args) = card(&plain);
        assert_eq!(plain.hidden, 0);
        assert_eq!(args["hidden"], 0);
        assert!(!sentence.contains("left out"), "{sentence}");
    }

    /// Only the person's yes writes the job, through the one save door, with
    /// the moment they said it; a no, a silence and a document changed under
    /// the card write nothing.
    #[test]
    fn only_an_allowed_request_is_saved_and_only_the_flow_that_was_shown() {
        let flow = flow_document();
        let (root, workspace) = stand(&[("wallet-qa", &flow)]);
        let request = || {
            asked(
                root.path(),
                Some(&workspace),
                &[
                    ("name", "wallet-qa"),
                    ("every", "weekly"),
                    ("day", "fri"),
                    ("at", "18:30"),
                ],
            )
            .expect("read")
        };
        let saved: RefCell<Vec<Automation>> = RefCell::new(Vec::new());
        let save = |job: Automation| {
            saved.borrow_mut().push(job);
            Ok(())
        };

        for (decision, code) in [
            (Decision::Refused, error_code::CONFIRMATION_REFUSED),
            (Decision::TimedOut, error_code::CONFIRMATION_TIMEOUT),
        ] {
            let refused =
                settle(request(), decision, 1_794_000_000_000, save).expect_err("nothing");
            assert_eq!(refused.code, code);
        }
        assert!(saved.borrow().is_empty());

        let answer = settle(request(), Decision::Allowed, 1_794_000_000_000, save).expect("saved");
        assert_eq!(answer["scheduled"], true);
        assert_eq!(answer["cron"], "30 18 * * 5");
        {
            let held = saved.borrow();
            assert_eq!(held.len(), 1);
            let stored = held[0].asked.as_ref().expect("who asked");
            assert_eq!(stored.approved_at_ms, 1_794_000_000_000);
            assert_eq!(stored.flow_sha256, flow_document_sha256(&flow));
            assert_eq!(answer["id"], held[0].id);
        }

        // The document is rewritten while the card stands.
        let waiting = request();
        std::fs::write(
            root.path()
                .join(super::super::RECIPES_DIR)
                .join("wallet-qa.md"),
            flow.replace("com.example.wallet", "com.example.other"),
        )
        .expect("rewritten");
        let changed =
            settle(waiting, Decision::Allowed, 1_794_000_000_000, save).expect_err("changed");
        assert!(changed.message.contains("changed while the card stood"));
        assert_eq!(saved.borrow().len(), 1);

        // A walk is held to the document the person allowed by the text it
        // read: the same text walks, another is refused, and a walk nobody
        // held to a document walks what it reads.
        let allowed = flow_document_sha256(&flow);
        let rewritten = flow.replace("com.example.wallet", "com.example.other");
        assert!(held_to(&flow, Some(&allowed)).is_ok());
        assert_eq!(
            held_to(&rewritten, Some(&allowed))
                .expect_err("another document")
                .message,
            FLOW_CHANGED
        );
        assert!(held_to(&rewritten, None).is_ok());

        // The same hash is what a walk and the list hold the approval against.
        assert_eq!(
            document_sha256(root.path(), "wallet-qa"),
            Some(flow_document_sha256(
                &flow.replace("com.example.wallet", "com.example.other")
            ))
        );
        assert_eq!(document_sha256(root.path(), "nothing"), None);

        // The save door's own refusal is the answer.
        let refused = settle(request(), Decision::Allowed, 1, |_| {
            Err("일정을 읽을 수 없습니다".to_string())
        })
        .expect_err("refused by the door");
        assert_eq!(refused.message, "일정을 읽을 수 없습니다");
    }
}
