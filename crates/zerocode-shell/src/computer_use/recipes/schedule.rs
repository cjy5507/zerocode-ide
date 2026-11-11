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
//!   the card stood is not the one that was allowed ([`settle`]).
//!
//! The window cannot tell a job somebody wrote straight into its saved file
//! from one it was asked for; this road closes the asking, not the file.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::{Map, Value, json};
use zerocode_core::automation::{ASKED_SCHEDULE_FLOOR_MINUTES, Asked, flow_document_sha256};
use zerocode_core::card_words::{self, WORD_MAX_CHARS};
use zerocode_core::computer_flow::{Policy, parse_flow};
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
    /// How many letters that draw nothing were left out of the label and the
    /// Flow's title (`card_words::shown_counting`): the card says so.
    pub hidden: usize,
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
    let spec = match parse_flow(&text) {
        Ok(Some(spec)) => spec,
        Ok(None) => {
            return Err(ComputerUseError::invalid_argument(format!(
                "'{named}' is a recipe, not a Flow — only a Flow is judged after it runs. Stand it up first: `{COMPUTER_CLI} recipe-save --name <name> --flow-check <text the screen shows when it worked>`"
            )));
        }
        Err(why) => {
            return Err(ComputerUseError::invalid_argument(format!(
                "the Flow section of '{named}' does not read: {why}"
            )));
        }
    };
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
    let slug = recipe_slug(named);
    let title = card_words::shown_counting(
        &recipe_title(&text).unwrap_or_else(|| slug.clone()),
        WORD_MAX_CHARS,
    );
    let label = word("label").map(|label| card_words::shown_counting(label, WORD_MAX_CHARS));
    let hidden = title.hidden + label.as_ref().map_or(0, |label| label.hidden);
    let flow_name = title.text;
    let name = label
        .map(|label| label.text)
        .filter(|label| !label.is_empty())
        .unwrap_or_else(|| flow_name.clone());
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
            flow_sha256: flow_document_sha256(&text),
        },
    );
    // Checked as the save door will check it, so no card is shown for a job
    // that could not be saved.
    job.bind_flow()
        .and_then(|()| job.validate_for_save())
        .map_err(ComputerUseError::invalid_argument)?;
    Ok(Request {
        job,
        flow_name,
        policy: spec.policy.as_str(),
        money: spends(spec.policy),
        next,
        hidden,
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
    });
    let sentence = format!(
        "An agent in {pane} asks to schedule the Flow \"{flow}\" (policy {policy}) as \"{name}\": cron `{cron}`, in \"{workspace}\". The window would walk it by itself, with nobody watching. Allow it only if that is what you want; cancel it, or leave it, and nothing is scheduled.{spending}{left_out}",
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

    use zerocode_core::computer_flow::{FLOW_HEADING, FLOW_HEADING_CHECKS};
    use zerocode_core::computer_recipe::RECIPE_HEADING_STEPS;
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
