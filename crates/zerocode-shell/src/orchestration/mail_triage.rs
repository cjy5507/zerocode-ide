//! A coordinator's mail, triaged by Jev as it arrives (t-9471,
//! `zerocode_core::jev::MAIL_TRIAGE`, `smart.jevMailTriage`).
//!
//! On the beat, for every run whose coordinator seat this window holds, each
//! letter to the coordinator that it can still be handed — waiting in its
//! inbox, or in the batch it holds open — is put to Jev once: its structure,
//! never its words (`zerocode_core::mail_triage::ask`), through the Jev door
//! with the coordinator's own checkout as the workspace consented to. The
//! answer is one row in `mail-triage.jsonl`. It changes nothing the beat
//! does: the seat only records, and nothing on the desk moves.
//!
//! Later beats read what the coordinator did next off the ledger
//! (`zerocode_core::mail_triage::Mailroom::label`) and write it as the row's
//! label — when the window saw the letter handed over, the batch's own stamp
//! is the label's start. A question waits up to [`MAIL_TRIAGE_DEADLINE`] for
//! its answer, so every letter a beat found is asked in one job off the beat
//! ([`Host::off_the_beat`]); the beat never waits on a socket, and a ledger
//! that has not moved since the last beat costs a comparison.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::Value;
use zerocode_core::jev::MAIL_TRIAGE;
use zerocode_core::mail_triage::{Labeled, Mailroom, NotCompared, Start, Triage};
use zerocode_core::orchestration::{Ledger, Message, Run};

use crate::agent_teams::Host;

/// How long one question may wait for its answer — the table's wire wall
/// (`MAIL_TRIAGE_DEADLINE_MS`), read from there so the wait and its reason
/// are one number.
pub(crate) const MAIL_TRIAGE_DEADLINE: Duration =
    Duration::from_millis(zerocode_core::jev::MAIL_TRIAGE_DEADLINE_MS);

/// The key a request row names its letter under, and a label row names it
/// back — the table's own name for the seat's requests.
const KEY: &str = MAIL_TRIAGE.request_name[0];

/// What this window remembers about the letters it asked about.
#[derive(Default)]
pub(super) struct MailBook {
    /// The ledger revision the last sweep read: an unchanged ledger has no new
    /// letter, no new hand-over and no new act, and costs a comparison.
    revision: Option<u64>,
    /// Every letter asked about, by id — once, whatever became of it. Read
    /// back off the rows' tail by the first sweep, so a restart asks about
    /// none of them again.
    asked: HashSet<String>,
    /// When this window saw each letter in the batch its coordinator held
    /// open — the batch's own stamp — by letter, until its label is written.
    opened: HashMap<String, i64>,
    /// The ledger the rows go to, and the answered rows without a label yet.
    /// `None` until a sweep has read that ledger's tail.
    waiting: Option<(PathBuf, Vec<Waiting>)>,
}

/// An answered question waiting for what the coordinator does next.
#[derive(Debug, Clone, PartialEq)]
struct Waiting {
    key: String,
    run: String,
    asked_ms: i64,
    triage: Triage,
    /// The batch's stamp, when the letter was already in it when asked.
    delivered_ms: Option<i64>,
}

/// Put every letter the coordinators this window seats can still be handed,
/// and nobody has asked about, to Jev — and write the label of every
/// answered one the ledger now says something about.
pub(super) fn sweep(_host: &dyn Host, _book: &Arc<Mutex<MailBook>>, _now_ms: i64) {
    todo!("t-9471")
}

/// The letters a question may be asked about: those the coordinator can
/// still be handed — waiting in its inbox, or in the batch it holds open —
/// that nobody has asked about yet, oldest first.
fn fresh_letters<'a>(_room: &Mailroom<'a>, _asked: &HashSet<String>) -> Vec<&'a Message> {
    todo!("t-9471")
}

/// Every letter the rows at `ledger`'s tail asked about, and the answered
/// ones no label row names yet.
fn read_tail(_ledger: &Path) -> (HashSet<String>, Vec<Waiting>) {
    todo!("t-9471")
}

/// The label rows `ledger` can write now for the answered rows waiting in
/// `book` — each once, and dropped from the book as it is written. A run the
/// ledger no longer holds cannot say what followed, and its rows stay
/// unlabeled rather than guessed at; a letter not handed over yet waits.
fn label_waiting(_book: &mut MailBook, _ledger: &Ledger, _now_ms: i64) -> Vec<Value> {
    todo!("t-9471")
}

/// The label row of one answered letter: what the coordinator did, when and
/// how the label knows the hand-over, and the marks — the answer's and the
/// kind rule's — or why there are none.
fn label_row(
    _one: &Waiting,
    _letter: &Message,
    _start: Start,
    _outcome: &Result<Labeled<'_>, NotCompared>,
    _now_ms: i64,
) -> Value {
    todo!("t-9471")
}

/// The run's coordinator seat as `team/pane` and the term this window holds
/// it in, when it holds it.
fn seat_term(_run: &Run, _seats: &super::TeamSeatIndex) -> Option<u32> {
    todo!("t-9471")
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use serde_json::json;
    use zerocode_core::jev::summary::{AGREED, BASELINE_AGREED, LABEL, NOT_COMPARED, REQUEST_AT};
    use zerocode_core::mail_triage::{ANSWER_NOW_WITHIN_ACTS, kind_rule};
    use zerocode_core::orchestration::{
        Draft, MessageKind, Priority, ServedAnswer, ServedRow, Text, worker_address,
    };

    use super::*;

    const COORDINATOR: &str = "actor-v1:coordinator";

    /// A window that restarts reads back the letters its rows asked about —
    /// every one, so none is asked again — and the answered ones no label
    /// names yet, with what they answered.
    #[test]
    fn a_restart_finds_the_letters_it_asked_about_and_the_answers_waiting() {
        let dir = tempfile::tempdir().expect("a zo home");
        let ledger = dir.path().join(MAIL_TRIAGE.ledger);
        let row = |key: &str, outcome: &str, triage: &str| {
            json!({ "at": 5, KEY: key, "run": "run-1", "outcome": outcome,
                    "triage": triage, "urgent": 0.4, "deliveredMs": 3 })
        };
        crate::systemone::append_rows(
            &ledger,
            &[
                row("m-1", "answered", "answer_now"),
                row("m-2", "not_consented", "answer_now"),
                row("m-3", "answered", "can_wait"),
                json!({ "at": 9, (LABEL.canonical): "m-1", "truth": "answer_now" }),
            ],
        );
        std::fs::OpenOptions::new()
            .append(true)
            .open(&ledger)
            .and_then(|mut file| file.write_all(b"{\"torn\": "))
            .expect("a torn last line");

        let (asked, waiting) = read_tail(&ledger);
        let mut asked: Vec<&str> = asked.iter().map(String::as_str).collect();
        asked.sort_unstable();
        assert_eq!(asked, ["m-1", "m-2", "m-3"], "every letter asked about");
        assert_eq!(
            waiting,
            vec![Waiting {
                key: "m-3".into(),
                run: "run-1".into(),
                asked_ms: 5,
                triage: Triage::CanWait,
                delivered_ms: Some(3),
            }]
        );
        let (none, nothing) = read_tail(&dir.path().join("gone.jsonl"));
        assert!(none.is_empty() && nothing.is_empty());
    }

    /// A ledger with one seated coordinator, a worker carrying a task, and
    /// that worker's question to the coordinator, handed over in a batch.
    fn a_run_with_a_question() -> (Ledger, String, String, String) {
        let mut ledger = Ledger::new();
        let run_id = ledger.create_run("mail", 1);
        ledger
            .seat_coordinator(&run_id, "team/%1", Some(COORDINATOR), 2)
            .expect("seated");
        let task = ledger
            .create_task(&run_id, "spec".into(), "title".into(), vec![], None, 3)
            .expect("a task");
        let started = ledger
            .start_worker(&run_id, "claude", ("team", "%2"), Some(&task), 4)
            .expect("a worker");
        let question = ledger
            .post(
                &run_id,
                Draft {
                    from: worker_address(&started.worker),
                    to: format!("run:{run_id}"),
                    kind: MessageKind::Question,
                    body: Text::from("words the label never reads"),
                    subject: Text::default(),
                    priority: Priority::Normal,
                    payload: Text::default(),
                    thread: None,
                    task: Some(task.clone()),
                    dispatch: started.dispatch.clone(),
                },
                10,
            )
            .expect("a question");
        (ledger, run_id, question, started.worker)
    }

    /// The coordinator's answer to `question`, and its receipt.
    fn answered(ledger: &Ledger, run_id: &str, question: &str, worker: &str, at: i64) -> Ledger {
        let mut ledger = Ledger::rebuild(ledger.export()).expect("a copy");
        let reply = ledger
            .post(
                run_id,
                Draft {
                    from: format!("run:{run_id}"),
                    to: worker_address(worker),
                    kind: MessageKind::Question,
                    body: Text::from("yes"),
                    subject: Text::default(),
                    priority: Priority::Normal,
                    payload: Text::default(),
                    thread: Some(question.to_string()),
                    task: None,
                    dispatch: None,
                },
                at,
            )
            .expect("an answer");
        let mut projected = ledger.export();
        projected.served.push(ServedRow {
            caller: Some(Text::from(COORDINATOR)),
            request: Text::from("r-reply"),
            answer: ServedAnswer::Inline(json!({ "messageId": reply }).to_string()),
            fingerprint: None,
            verb: Some("reply".to_string()),
            filed_ms: Some(at),
            expired: false,
        });
        Ledger::rebuild(projected).expect("the ledger with its receipt")
    }

    /// An answered question's label is written once the coordinator acts on
    /// it — the answer's own mark and the kind rule's beside it — and not
    /// before; the letter leaves the book as its label is written.
    #[test]
    fn the_label_is_written_off_the_ledger_once_the_coordinator_acts() {
        let (ledger, run_id, question, worker) = a_run_with_a_question();
        let mut book = MailBook {
            waiting: Some((
                PathBuf::from("mail-triage.jsonl"),
                vec![Waiting {
                    key: question.clone(),
                    run: run_id.clone(),
                    asked_ms: 12,
                    triage: Triage::CanWait,
                    delivered_ms: None,
                }],
            )),
            ..MailBook::default()
        };
        // Still waiting in the coordinator's inbox: nothing to say yet.
        assert!(label_waiting(&mut book, &ledger, 20).is_empty());
        let mut projected = ledger.export();
        let inbox = projected
            .inboxes
            .iter_mut()
            .find(|row| row.run == run_id && row.address == format!("run:{run_id}"))
            .expect("the coordinator's inbox");
        inbox.pending.retain(|id| *id != question);
        inbox.open = Some(zerocode_core::orchestration::Delivery {
            id: "d-1".into(),
            messages: vec![question.clone()],
            holder: Some("team/%1".into()),
            opened_ms: Some(15),
        });
        let handed = Ledger::rebuild(projected).expect("handed over");
        book.opened.insert(question.clone(), 15);
        let answered = answered(&handed, &run_id, &question, &worker, 30);
        let labels = label_waiting(&mut book, &answered, 40);
        assert_eq!(labels.len(), 1, "{labels:?}");
        let label = &labels[0];
        assert_eq!(label[LABEL.canonical], question.as_str());
        assert_eq!(label[REQUEST_AT.canonical], 12);
        assert_eq!(label["truth"], "answer_now");
        assert_eq!(label["afterActions"], 1);
        assert_eq!(
            label["afterMs"], 15,
            "from the batch's stamp the window saw"
        );
        assert_eq!(label["start"], "opened");
        assert_eq!(
            label[AGREED.canonical], false,
            "it said the question could wait"
        );
        assert_eq!(
            label[BASELINE_AGREED.canonical],
            kind_rule(MessageKind::Question) == Triage::AnswerNow
        );
        assert!(!label.to_string().contains("words the label"), "{label}");
        assert!(
            book.waiting
                .as_ref()
                .is_some_and(|(_, rows)| rows.is_empty()),
            "a label is written once"
        );
        assert!(book.opened.is_empty(), "and the stamp goes with it");
        assert!(label_waiting(&mut book, &answered, 50).is_empty());
    }

    /// A label that cannot mark says why, in the words the dashboard names.
    #[test]
    fn a_label_that_cannot_mark_says_why() {
        let (ledger, run_id, question, _) = a_run_with_a_question();
        let run = ledger.run(&run_id).expect("the run");
        let letter = run.message(&question).expect("the letter");
        let one = Waiting {
            key: question.clone(),
            run: run_id.clone(),
            asked_ms: 12,
            triage: Triage::AnswerNow,
            delivered_ms: None,
        };
        let row = label_row(
            &one,
            letter,
            Start::Created(10),
            &Err(NotCompared::Superseded),
            99,
        );
        assert_eq!(row[NOT_COMPARED.canonical], "superseded");
        assert!(row.get(AGREED.canonical).is_none());
        assert!(row.get(BASELINE_AGREED.canonical).is_none());
        let handled = label_row(
            &one,
            letter,
            Start::Created(10),
            &Ok(Labeled {
                truth: Triage::AnswerNow,
                urgent: true,
                after_actions: ANSWER_NOW_WITHIN_ACTS,
                after_ms: 5,
                handled_by: Some("reply"),
            }),
            99,
        );
        assert_eq!(handled[AGREED.canonical], true);
        assert_eq!(handled["urgentTruth"], true);
        assert_eq!(handled["handledBy"], "reply");
        assert!(handled.get(NOT_COMPARED.canonical).is_none());
    }

    /// A question is asked about a letter the coordinator can still be
    /// handed — waiting, or in its open batch — and once; one it has been
    /// handed and put away is history, and its own words are not letters.
    #[test]
    fn only_letters_the_coordinator_can_still_be_handed_are_asked_about_once() {
        let (ledger, run_id, question, _) = a_run_with_a_question();
        let run = ledger.run(&run_id).expect("the run");
        let receipts = Vec::new();
        let room = Mailroom::of_run(run, &receipts);
        let fresh: Vec<&str> = fresh_letters(&room, &HashSet::new())
            .iter()
            .map(|one| one.id.as_str())
            .collect();
        assert_eq!(fresh, [question.as_str()]);
        let asked: HashSet<String> = [question.clone()].into_iter().collect();
        assert!(fresh_letters(&room, &asked).is_empty(), "asked once");

        let mut projected = ledger.export();
        let inbox = projected
            .inboxes
            .iter_mut()
            .find(|row| row.run == run_id && row.address == format!("run:{run_id}"))
            .expect("the coordinator's inbox");
        inbox.pending.clear();
        let history = Ledger::rebuild(projected).expect("put away");
        let run = history.run(&run_id).expect("the run");
        let room = Mailroom::of_run(run, &receipts);
        assert!(
            fresh_letters(&room, &HashSet::new()).is_empty(),
            "a letter handed over and put away is history"
        );
    }
}
