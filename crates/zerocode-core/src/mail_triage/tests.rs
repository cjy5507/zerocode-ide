use std::collections::{BTreeMap, HashMap};

use serde_json::{Value, json};

use super::*;
use crate::jev::{MAIL_TRIAGE, rubric_fingerprint};
use crate::orchestration::{
    CheckV1, Draft, LedgerProjectionV1, Priority, ServedAnswer, ServedRow, Text, worker_address,
};

/// The coordinator's session, as a receipt names it.
const COORDINATOR: &str = "actor-v1:coordinator";
/// A worker's session.
const WORKER_ACTOR: &str = "actor-v1:worker";
/// The run's address in the hand-built rooms below.
const ADDRESS: &str = "run:run-1";

/// One message, as the ledger holds it — with no words, which the label
/// never reads.
fn posted(
    id: &str,
    from: &str,
    to: &str,
    kind: MessageKind,
    at: i64,
    thread: Option<&str>,
    task: Option<&str>,
    dispatch: Option<&str>,
) -> Message {
    Message {
        id: id.to_string(),
        from: from.to_string(),
        to: to.to_string(),
        kind,
        body: Text::default(),
        subject: Text::default(),
        priority: Priority::Normal,
        payload: Text::default(),
        thread: thread.map(str::to_string),
        task: task.map(str::to_string),
        dispatch: dispatch.map(str::to_string),
        author_seat: None,
        created_ms: at,
    }
}

/// A worker's letter to the coordinator.
fn from_worker(id: &str, worker: &str, kind: MessageKind, at: i64, task: Option<&str>) -> Message {
    posted(
        id,
        &worker_address(worker),
        ADDRESS,
        kind,
        at,
        None,
        task,
        None,
    )
}

/// The coordinator's own message.
fn from_coordinator(
    id: &str,
    to: &str,
    at: i64,
    thread: Option<&str>,
    task: Option<&str>,
) -> Message {
    posted(id, ADDRESS, to, MessageKind::Status, at, thread, task, None)
}

/// One attempt.
fn attempt(id: &str, task: &str, worker: &str, started: i64) -> Dispatch {
    Dispatch {
        id: id.to_string(),
        task: task.to_string(),
        worker: worker.to_string(),
        started_ms: started,
        ended_ms: None,
        succeeded: None,
        retry_of: None,
        remote: None,
        source: None,
    }
}

/// One receipt, as the label reads it.
fn filed(caller: &str, verb: &str, at: i64, answer: Value) -> Filed {
    Filed::of_parts(
        Some(caller.to_string()),
        Some(verb.to_string()),
        Some(at),
        &answer,
        None,
    )
}

/// A `check` of the coordinator's inbox — its acknowledgement — that handed
/// `messages` over.
fn handing(at: i64, messages: &[&str]) -> Filed {
    Filed::of_parts(
        Some(COORDINATOR.to_string()),
        Some(inbox_verb().to_string()),
        Some(at),
        &Value::Null,
        Some(Looked {
            address: ADDRESS.to_string(),
            messages: messages.iter().map(|id| (*id).to_string()).collect(),
        }),
    )
}

/// A quiet `check` of the coordinator's inbox.
fn looked(at: i64) -> Filed {
    handing(at, &[])
}

/// The verb the table classes as the inbox's own, read off the table.
fn inbox_verb() -> &'static str {
    VERBS
        .iter()
        .find(|(_, _, doing)| *doing == Doing::Inbox)
        .map(|(verb, _, _)| *verb)
        .expect("the table has an inbox verb")
}

/// A coordinator act that names nothing any letter below is about: a task
/// written down.
fn busywork(at: i64, n: usize) -> Filed {
    filed(
        COORDINATOR,
        "task-create",
        at,
        json!({ "taskId": format!("t-new-{n}"), "status": "pending", "title": "기록하지 않는 제목" }),
    )
}

fn room<'a>(
    messages: &'a [Message],
    dispatches: &'a [Dispatch],
    receipts: &'a [Filed],
) -> Mailroom<'a> {
    Mailroom::new(
        ADDRESS.to_string(),
        Some(COORDINATOR),
        messages,
        dispatches,
        None,
        &[],
        receipts,
    )
}

/// The version is pinned to the words: a wording change without a bump is a
/// red test, not a quiet drift.
#[test]
fn the_version_is_pinned_to_the_words() {
    assert_eq!(MAIL_TRIAGE_RUBRIC_VERSION, 1);
    assert_eq!(rubric_fingerprint(rubric_words), "5a4f2984acb4ba2e");
}

/// The answers are three words in the question's order, each reads back, and
/// a word the question never offered is none of them.
#[test]
fn the_answers_are_three_words_in_the_questions_order() {
    let words: Vec<&str> = Triage::ALL.iter().map(|triage| triage.word()).collect();
    assert_eq!(words, ["answer_now", "can_wait", "no_need"]);
    for triage in Triage::ALL {
        assert_eq!(Triage::from_word(triage.word()), Some(triage));
    }
    assert_eq!(Triage::from_word("urgent"), None);
    assert_eq!(
        NotCompared::ALL.map(NotCompared::word),
        ["superseded", "unhandled"]
    );
}

/// The kind rule is the brief's four — a question and a finished task now, a
/// status later, a silence never — and a heartbeat reads as the idle it is.
#[test]
fn the_kind_rule_reads_the_kind_alone() {
    assert_eq!(kind_rule(MessageKind::Question), Triage::AnswerNow);
    assert_eq!(kind_rule(MessageKind::WorkerDone), Triage::AnswerNow);
    assert_eq!(kind_rule(MessageKind::Status), Triage::CanWait);
    assert_eq!(kind_rule(MessageKind::WentQuiet), Triage::NoNeed);
    assert_eq!(kind_rule(MessageKind::Heartbeat), Triage::NoNeed);
    assert_eq!(kind_rule(MessageKind::WorkerDied), Triage::AnswerNow);
    assert_eq!(kind_rule(MessageKind::Handover), Triage::CanWait);
}

fn look<'a>() -> MailLook<'a> {
    MailLook {
        kind: MessageKind::Question,
        from: "worker",
        worker: Some("w-7"),
        task: Some("t-3"),
        task_status: Some("dispatched"),
        priority: "normal",
        awaits_answer: true,
        thread_depth: 0,
        age_ms: 42_500,
        delivered: false,
        repeats: 2,
        coordinator_busy: None,
        open_questions: 3,
    }
}

/// The question carries a letter's structure under the table's keys — the
/// texts only where the table declares them, numbers and flags elsewhere —
/// and asks one closed choice and one Noul in one request.
#[test]
fn the_question_carries_the_letters_structure_and_never_its_words() {
    let asked = ask(&look());
    let state = asked.state.as_object().expect("a state");
    let keys: Vec<&str> = state.keys().map(String::as_str).collect();
    let mut expected = STATE_KEYS.to_vec();
    expected.sort_unstable();
    let mut keys_sorted = keys.clone();
    keys_sorted.sort_unstable();
    assert_eq!(keys_sorted, expected);
    assert_eq!(asked.state["kind"], "question");
    assert_eq!(asked.state["from"], "worker");
    assert_eq!(asked.state["worker"], "w-7");
    assert_eq!(asked.state["task"], "t-3");
    assert_eq!(asked.state["taskStatus"], "dispatched");
    assert_eq!(asked.state["priority"], "normal");
    assert_eq!(asked.state["awaitsAnswer"], true);
    assert_eq!(asked.state["threadDepth"], 0);
    assert_eq!(asked.state["ageSeconds"], 42, "seconds a person would say");
    assert_eq!(asked.state["delivered"], false);
    assert_eq!(asked.state["repeats"], 2);
    assert_eq!(
        asked.state["coordinatorBusy"],
        Value::Null,
        "unknown is unknown"
    );
    assert_eq!(asked.state["openQuestions"], 3);
    // Every text in the state is one the table declares; the rest are not
    // texts at all.
    let declared: Vec<&str> = MAIL_TRIAGE
        .sends
        .iter()
        .map(|sent| sent.at.trim_start_matches("/state/"))
        .collect();
    for (key, value) in state {
        assert_eq!(
            value.is_string(),
            declared.contains(&key.as_str()),
            "{key}: {value}"
        );
    }
    let questions = asked.questions.as_object().expect("questions");
    assert_eq!(questions.len(), 2);
    assert_eq!(asked.questions["triage"]["type"], "choice");
    let criteria = asked.questions["triage"]["criteria"]
        .as_object()
        .expect("criteria");
    let offered: Vec<&str> = criteria.keys().map(String::as_str).collect();
    let mut words: Vec<&str> = Triage::ALL.map(Triage::word).to_vec();
    words.sort_unstable();
    let mut offered_sorted = offered.clone();
    offered_sorted.sort_unstable();
    assert_eq!(offered_sorted, words);
    assert_eq!(asked.questions["urgent"]["type"], "noul");

    // A busy coordinator is said as a flag, and a negative age as none.
    let busy = ask(&MailLook {
        coordinator_busy: Some(true),
        age_ms: -5,
        ..look()
    });
    assert_eq!(busy.state["coordinatorBusy"], true);
    assert_eq!(busy.state["ageSeconds"], 0);
}

fn answer(chosen: &str, urgent: Value) -> Value {
    json!({
        "triage": {
            "type": "choice",
            "choice": chosen,
            "probabilities": { "answer_now": 0.7, "can_wait": 0.2, "no_need": 0.1 },
            "confidence": 0.6,
        },
        "urgent": { "type": "noul", "noul": urgent },
    })
}

/// An answer needs both heads in shape; one broken rule in either discards
/// it whole, and says which.
#[test]
fn an_answer_needs_both_heads_or_is_refused_whole() {
    let asked = ask(&look());
    let read = asked
        .read(&answer("answer_now", json!(0.8)))
        .expect("in shape");
    assert_eq!(read.triage, Triage::AnswerNow);
    assert_eq!(read.confidence, 0.6);
    assert_eq!(read.urgent, 0.8);
    assert_eq!(read.probabilities.len(), 3);
    assert_eq!(
        asked.read(&answer("later", json!(0.8))),
        Err(MailRefusal::Triage(ChoiceRefusal::UnknownOption))
    );
    assert_eq!(
        asked.read(&answer("answer_now", json!(1.5))),
        Err(MailRefusal::Urgent(NoulRefusal::OutOfRange))
    );
    let mut lacking = answer("answer_now", json!(0.8));
    lacking.as_object_mut().expect("answers").remove("urgent");
    assert_eq!(
        asked.read(&lacking),
        Err(MailRefusal::Urgent(NoulRefusal::NoAnswer))
    );
    assert_eq!(
        MailRefusal::Urgent(NoulRefusal::NoAnswer).token(),
        "schema_no_noul"
    );
    assert_eq!(
        MailRefusal::Triage(ChoiceRefusal::NotOne).token(),
        "schema_not_one"
    );
}

/// A receipt's answer names the ledger's ids under the verbs' own keys — a
/// question an `ask` posted among them — and nothing else of it is read: a
/// status word, a title, an answer's prose.
#[test]
fn a_receipts_answer_names_ids_and_nothing_else() {
    assert_eq!(
        Names::of_answer(&json!({ "messageId": "m-9" })),
        Names {
            message: Some("m-9".into()),
            ..Names::default()
        }
    );
    assert_eq!(
        Names::of_answer(
            &json!({ "taskId": "t-1", "status": "completed", "author": "coordinator" })
        ),
        Names {
            task: Some("t-1".into()),
            ..Names::default()
        }
    );
    assert_eq!(
        Names::of_answer(&json!({
            "dispatchId": "dp-4", "taskId": "t-1", "workerId": "w-5", "retryOf": "dp-2",
            "pane": "%7", "model": "opus",
        })),
        Names {
            task: Some("t-1".into()),
            worker: Some("w-5".into()),
            dispatch: Some("dp-4".into()),
            retry_of: Some("dp-2".into()),
            ..Names::default()
        }
    );
    assert_eq!(
        Names::of_answer(&json!({
            "questionId": "m-3", "answered": true,
            "answer": { "messageId": "m-4", "body": "비밀 문장" },
        })),
        Names {
            message: Some("m-3".into()),
            ..Names::default()
        },
        "the question an ask posted, not the answer it was told"
    );
    assert_eq!(Names::of_answer(&json!("printed prose")), Names::default());
    assert_eq!(
        Names::of_answer(&json!({ "taskId": 7 })),
        Names::default(),
        "an id is a text"
    );
}

/// The inbox's own verb — a look, or the acknowledgement it carries — is not
/// one of the coordinator's acts, and neither is anything another session
/// filed; a session the ledger has seen write as the coordinator is.
#[test]
fn the_coordinators_acts_are_its_own_receipts_less_the_inbox() {
    let messages = vec![
        from_coordinator("m-2", "worker:w-1", 5, None, None),
        from_coordinator("m-3", "worker:w-1", 6, None, None),
    ];
    let receipts = vec![
        looked(10),
        filed(WORKER_ACTOR, "send", 11, json!({ "messageId": "m-1" })),
        filed(COORDINATOR, "send", 12, json!({ "messageId": "m-2" })),
        // A second session of the coordinator's — its restart — known by the
        // letter it wrote as the coordinator.
        filed(
            "actor-v1:restarted",
            "send",
            13,
            json!({ "messageId": "m-3" }),
        ),
        filed(
            "actor-v1:restarted",
            "task-update",
            14,
            json!({ "taskId": "t-1" }),
        ),
    ];
    assert!(receipts[0].reads_the_inbox());
    assert!(!receipts[2].reads_the_inbox());
    let room = room(&messages, &[], &receipts);
    let acts: Vec<(i64, Option<&str>)> = room
        .acts()
        .iter()
        .map(|act| (act.at_ms, act.verb))
        .collect();
    assert_eq!(
        acts,
        [
            (12, Some("send")),
            (13, Some("send")),
            (14, Some("task-update"))
        ]
    );
    assert_eq!(
        room.acts()[0].message.map(|one| one.id.as_str()),
        Some("m-2"),
        "an act that wrote a message carries it"
    );
}

/// A receipt an older window filed with no verb is still the inbox's when
/// its answer is one only a look gives — a batch, or a peek's or a history's
/// page — and an act otherwise.
#[test]
fn a_receipt_with_no_verb_is_the_inboxs_by_its_answer() {
    let bare = |answer: Value, check: Option<Looked>| {
        Filed::of_parts(Some(COORDINATOR.to_string()), None, Some(1), &answer, check)
    };
    let batch = Looked {
        address: ADDRESS.to_string(),
        messages: vec!["m-1".into()],
    };
    assert!(bare(Value::Null, Some(batch)).reads_the_inbox());
    for mode in [
        crate::orchestration::PEEK_MODE,
        crate::orchestration::HISTORY_MODE,
    ] {
        let page =
            json!({ (crate::orchestration::LOOK_MODE_KEY): mode, "count": 0, "messages": [] });
        assert!(bare(page, None).reads_the_inbox(), "{mode}");
    }
    assert!(!bare(json!({ "messageId": "m-9" }), None).reads_the_inbox());
    assert!(
        !filed(
            COORDINATOR,
            "send",
            1,
            json!({ "mode": crate::orchestration::PEEK_MODE })
        )
        .reads_the_inbox(),
        "a verb says what it was, whatever its answer carries"
    );
}

/// A question the coordinator answered with its very next act wanted it now,
/// and was urgent.
#[test]
fn a_question_answered_at_the_first_act_is_answer_now_and_urgent() {
    let messages = vec![
        from_worker("m-1", "w-1", MessageKind::Question, 100, Some("t-1")),
        from_coordinator("m-2", "worker:w-1", 200, Some("m-1"), None),
    ];
    let receipts = vec![
        looked(150),
        filed(COORDINATOR, "reply", 200, json!({ "messageId": "m-2" })),
    ];
    let room = room(&messages, &[], &receipts);
    let letter = &messages[0];
    let start = room.start_of(letter, None);
    assert_eq!(start, Start::Created(100));
    let labeled = room
        .label(letter, start, 300)
        .expect("decided")
        .expect("compared");
    assert_eq!(
        labeled,
        Labeled {
            truth: Triage::AnswerNow,
            urgent: true,
            after_actions: 1,
            after_ms: 100,
            handled_by: Some("reply"),
        }
    );
}

/// Answered after more acts than "now" takes, it could wait — and the acts
/// that named something else are counted, the inbox's are not.
#[test]
fn a_question_answered_after_the_line_could_wait() {
    let mut messages = vec![from_worker("m-1", "w-1", MessageKind::Question, 100, None)];
    let mut receipts = Vec::new();
    for n in 0..ANSWER_NOW_WITHIN_ACTS {
        let at = 110 + i64::try_from(n).expect("small") * 10;
        receipts.push(looked(at - 1));
        receipts.push(busywork(at, n));
    }
    messages.push(from_coordinator(
        "m-2",
        "worker:w-1",
        500,
        Some("m-1"),
        None,
    ));
    receipts.push(filed(
        COORDINATOR,
        "reply",
        500,
        json!({ "messageId": "m-2" }),
    ));
    let room = room(&messages, &[], &receipts);
    let letter = &messages[0];
    let labeled = room
        .label(letter, room.start_of(letter, None), 600)
        .expect("decided")
        .expect("compared");
    assert_eq!(labeled.truth, Triage::CanWait);
    assert_eq!(labeled.after_actions, ANSWER_NOW_WITHIN_ACTS + 1);
    assert!(!labeled.urgent);
}

/// A question is handled by its answer and nothing else: a word to its asker
/// outside its thread is not the answer.
#[test]
fn a_question_is_handled_only_by_its_answer() {
    let messages = vec![
        from_worker("m-1", "w-1", MessageKind::Question, 100, Some("t-1")),
        from_coordinator("m-2", "worker:w-1", 150, None, Some("t-1")),
    ];
    let receipts = vec![filed(
        COORDINATOR,
        "send",
        150,
        json!({ "messageId": "m-2" }),
    )];
    let room = room(&messages, &[], &receipts);
    let letter = &messages[0];
    assert!(!room.handles(letter, &room.acts()[0]));
    assert_eq!(room.label(letter, room.start_of(letter, None), 200), None);
}

/// A finished task is handled only by an act naming that task — its review's
/// `task-update` — and not by a word to the worker that finished it.
#[test]
fn a_finished_task_is_handled_only_by_an_act_naming_its_task() {
    let messages = vec![
        from_worker("m-1", "w-1", MessageKind::WorkerDone, 100, Some("t-1")),
        from_coordinator("m-2", "worker:w-1", 150, None, None),
    ];
    let receipts = vec![
        filed(COORDINATOR, "send", 150, json!({ "messageId": "m-2" })),
        filed(
            COORDINATOR,
            "worker-retain",
            160,
            json!({ "workerId": "w-1", "state": "active" }),
        ),
        filed(
            COORDINATOR,
            "task-update",
            170,
            json!({ "taskId": "t-1", "status": "completed", "author": "coordinator" }),
        ),
    ];
    let room = room(&messages, &[], &receipts);
    let letter = &messages[0];
    let labeled = room
        .label(letter, room.start_of(letter, None), 200)
        .expect("decided")
        .expect("compared");
    assert_eq!(labeled.after_actions, 3);
    assert_eq!(labeled.truth, Triage::AnswerNow);
    assert_eq!(labeled.handled_by, Some("task-update"));
}

/// A notice the ledger wrote about a worker is handled by an act naming its
/// worker — read through the attempt the notice names — its task, its
/// attempt, or its thread.
#[test]
fn a_notice_is_handled_by_an_act_naming_what_it_is_about() {
    let dispatches = vec![attempt("dp-1", "t-1", "w-1", 1)];
    let notice = posted(
        "m-1",
        "ledger",
        ADDRESS,
        MessageKind::WentQuiet,
        100,
        None,
        Some("t-1"),
        Some("dp-1"),
    );
    for (verb, answer) in [
        (
            "worker-stop",
            json!({ "workerId": "w-1", "state": "released" }),
        ),
        ("task-update", json!({ "taskId": "t-1" })),
        (
            "worker-start",
            json!({ "dispatchId": "dp-2", "taskId": "t-9", "workerId": "w-2", "retryOf": "dp-1" }),
        ),
    ] {
        let messages = vec![notice.clone()];
        let receipts = vec![filed(COORDINATOR, verb, 150, answer)];
        let room = room(&messages, &dispatches, &receipts);
        assert_eq!(room.worker_of(&messages[0]), Some("w-1"));
        let labeled = room
            .label(&messages[0], Start::Created(100), 200)
            .expect("decided")
            .expect("compared");
        assert_eq!(labeled.handled_by, Some(verb), "{verb}");
        assert!(labeled.urgent);
    }
    // Mail to the worker handles it too.
    let messages = vec![
        notice.clone(),
        from_coordinator("m-2", "worker:w-1", 150, None, None),
    ];
    let receipts = vec![filed(
        COORDINATOR,
        "send",
        150,
        json!({ "messageId": "m-2" }),
    )];
    let room = room(&messages, &dispatches, &receipts);
    assert!(room.handles(&messages[0], &room.acts()[0]));
}

/// A letter nothing named for as many acts as the line allows needed no look.
#[test]
fn a_letter_nothing_named_for_the_line_needed_no_look() {
    let messages = vec![from_worker(
        "m-1",
        "w-1",
        MessageKind::Status,
        100,
        Some("t-1"),
    )];
    let receipts: Vec<Filed> = (0..NO_NEED_AFTER_ACTS + 3)
        .map(|n| busywork(200 + i64::try_from(n).expect("small"), n))
        .collect();
    let room = room(&messages, &[], &receipts);
    let letter = &messages[0];
    let labeled = room
        .label(letter, room.start_of(letter, None), 10_000)
        .expect("decided")
        .expect("compared");
    assert_eq!(labeled.truth, Triage::NoNeed);
    assert_eq!(labeled.after_actions, NO_NEED_AFTER_ACTS);
    assert_eq!(labeled.handled_by, None);
    assert!(!labeled.urgent);
    // One act short of the line, it is still waiting.
    let short = &receipts[..NO_NEED_AFTER_ACTS - 1];
    let waiting = room_with(&messages, short);
    assert_eq!(waiting.label(letter, Start::Created(100), 10_000), None);
}

fn room_with<'a>(messages: &'a [Message], receipts: &'a [Filed]) -> Mailroom<'a> {
    room(messages, &[], receipts)
}

/// The acts before the hand-over are not the coordinator's answer to a letter
/// it had not been handed.
#[test]
fn acts_before_the_hand_over_are_not_counted() {
    let messages = vec![
        from_worker("m-1", "w-1", MessageKind::Question, 100, None),
        from_coordinator("m-2", "worker:w-1", 400, Some("m-1"), None),
    ];
    let mut receipts: Vec<Filed> = (0..3)
        .map(|n| busywork(110 + i64::try_from(n).expect("small"), n))
        .collect();
    receipts.push(filed(
        COORDINATOR,
        "reply",
        400,
        json!({ "messageId": "m-2" }),
    ));
    let room = room(&messages, &[], &receipts);
    let letter = &messages[0];
    let late = room
        .label(letter, Start::Opened(300), 500)
        .expect("decided")
        .expect("compared");
    assert_eq!(late.after_actions, 1);
    assert_eq!(late.after_ms, 100);
    let early = room
        .label(letter, Start::Created(100), 500)
        .expect("decided")
        .expect("compared");
    assert_eq!(early.after_actions, 4);
}

/// When the coordinator was handed a letter: the open batch's stamp first
/// (the one the window saw, else the one it stands in now), else the first
/// `check` that handed it over, else when it was written.
#[test]
fn the_start_is_the_open_batch_then_a_check_then_the_writing() {
    let messages = vec![
        from_worker("m-1", "w-1", MessageKind::Status, 100, None),
        from_worker("m-2", "w-1", MessageKind::Status, 110, None),
        from_worker("m-3", "w-1", MessageKind::Status, 120, None),
    ];
    let receipts = vec![handing(130, &["m-2"]), handing(140, &["m-2"])];
    let open = Delivery {
        id: "d-9".into(),
        messages: vec!["m-3".into()],
        holder: Some("team/%1".into()),
        opened_ms: Some(125),
    };
    let pending = ["m-1"];
    let room = Mailroom::new(
        ADDRESS.to_string(),
        Some(COORDINATOR),
        &messages,
        &[],
        Some(&open),
        &pending,
        &receipts,
    );
    assert_eq!(room.start_of(&messages[0], None), Start::Created(100));
    assert_eq!(room.start_of(&messages[0], Some(105)), Start::Opened(105));
    assert_eq!(room.start_of(&messages[1], None), Start::Checked(130));
    assert_eq!(room.start_of(&messages[2], None), Start::Opened(125));
    assert!(room.is_pending(&messages[0]));
    assert!(room.is_open(&messages[2]));
    assert!(!room.is_open(&messages[1]));
    assert_eq!(
        [Start::Opened(1), Start::Checked(1), Start::Created(1)].map(Start::word),
        ["opened", "checked", "created"]
    );
}

/// A silence told again and again is one thing: the newer notice of the same
/// worker and task, arriving before anything handled the older one, takes it
/// over — and the act that then handles the silence labels the newer notice.
#[test]
fn a_newer_letter_about_the_same_thing_supersedes_an_unhandled_one() {
    let dispatches = vec![attempt("dp-1", "t-1", "w-1", 1)];
    let quiet = |id: &str, at: i64| {
        posted(
            id,
            "ledger",
            ADDRESS,
            MessageKind::WentQuiet,
            at,
            None,
            Some("t-1"),
            Some("dp-1"),
        )
    };
    let messages = vec![
        quiet("m-1", 100),
        quiet("m-2", 400),
        from_coordinator("m-3", "worker:w-1", 500, None, None),
    ];
    let receipts = vec![
        busywork(200, 0),
        filed(COORDINATOR, "send", 500, json!({ "messageId": "m-3" })),
    ];
    let room = room(&messages, &dispatches, &receipts);
    assert_eq!(room.newer_at(&messages[0]), Some(400));
    assert_eq!(room.newer_at(&messages[1]), None);
    assert_eq!(
        room.label(&messages[0], Start::Created(100), 600),
        Some(Err(NotCompared::Superseded))
    );
    let newer = room
        .label(&messages[1], Start::Created(400), 600)
        .expect("decided")
        .expect("compared");
    assert_eq!(newer.after_actions, 1);
    assert_eq!(room.repeats(&messages[1]), 1);
    assert_eq!(room.repeats(&messages[0]), 0);
    // A newer letter with nothing after it yet still takes the older over.
    let early = room_with(&messages[..2], &receipts[..1]);
    assert_eq!(
        early.label(&messages[0], Start::Created(100), 450),
        Some(Err(NotCompared::Superseded))
    );
}

/// A question has its own answer, so a second question from the same asker
/// never takes the first one over.
#[test]
fn a_question_is_never_superseded() {
    let messages = vec![
        from_worker("m-1", "w-1", MessageKind::Question, 100, Some("t-1")),
        from_worker("m-2", "w-1", MessageKind::Question, 110, Some("t-1")),
    ];
    let room = room(&messages, &[], &[]);
    assert_eq!(room.newer_at(&messages[0]), None);
}

/// A day with too few acts to say — the coordinator stopped — is written as
/// unhandled; until the day is out, the letter waits.
#[test]
fn a_day_without_enough_acts_is_unhandled() {
    let messages = vec![from_worker("m-1", "w-1", MessageKind::Status, 100, None)];
    let receipts: Vec<Filed> = (0..3)
        .map(|n| busywork(200 + i64::try_from(n).expect("small"), n))
        .collect();
    let room = room(&messages, &[], &receipts);
    let letter = &messages[0];
    assert_eq!(
        room.label(letter, Start::Created(100), 100 + LABEL_HORIZON_MS),
        None
    );
    assert_eq!(
        room.label(letter, Start::Created(100), 101 + LABEL_HORIZON_MS),
        Some(Err(NotCompared::Unhandled))
    );
}

/// One engine for the shipped rule and any a replay puts beside it: the
/// act the caller's rule names, by its place, decides — here the second.
#[test]
fn the_engine_takes_the_rule_it_is_handed() {
    let names = Names::default();
    let acts: Vec<Act<'_>> = [110, 120, 130]
        .into_iter()
        .map(|at| Act {
            at_ms: at,
            verb: Some("task-create"),
            names: &names,
            message: None,
        })
        .collect();
    let labeled = label_over(&acts, Start::Created(100), None, 100, 200, |at, _| at == 1)
        .expect("decided")
        .expect("compared");
    assert_eq!(labeled.after_actions, 2);
    assert_eq!(labeled.after_ms, 20);
    assert_eq!(
        label_over(&acts, Start::Created(115), None, 100, 200, |at, _| at == 0),
        None,
        "an act before the start is none of the letter's"
    );
}

/// What the question reads about a letter off a live run: its sender's head,
/// its worker and task and where the task stands, its depth, how many
/// letters about the same thing came before it and how many questions wait.
#[test]
fn the_look_reads_the_letters_facts_off_the_run() {
    let mut ledger = Ledger::new();
    let run_id = ledger.create_run("mail", 1);
    let task = ledger
        .create_task(&run_id, "spec".into(), "title".into(), vec![], None, 2)
        .expect("a task");
    let started = ledger
        .start_worker(&run_id, "claude", ("team", "%2"), Some(&task), 3)
        .expect("a worker");
    let address = format!("run:{run_id}");
    let question = ledger
        .post(
            &run_id,
            Draft {
                from: worker_address(&started.worker),
                to: address.clone(),
                kind: MessageKind::Question,
                body: Text::from("이 편지의 글은 읽지 않는다"),
                subject: Text::default(),
                priority: Priority::High,
                payload: Text::default(),
                thread: None,
                task: Some(task.clone()),
                dispatch: started.dispatch.clone(),
            },
            10,
        )
        .expect("a question");
    let run = ledger.run(&run_id).expect("the run");
    let receipts = Vec::new();
    let room = Mailroom::of_run(run, &receipts);
    let letter = room.message(&question).expect("the letter");
    let open_questions = room.open_questions(run);
    assert_eq!(open_questions, 1);
    let look = room.look(run, letter, 20_010, Some(false), open_questions);
    assert_eq!(look.kind, MessageKind::Question);
    assert_eq!(look.from, "worker");
    assert_eq!(look.worker, Some(started.worker.as_str()));
    assert_eq!(look.task, Some(task.as_str()));
    assert_eq!(look.task_status, Some("dispatched"));
    assert_eq!(look.priority, "high");
    assert!(look.awaits_answer);
    assert_eq!(look.thread_depth, 0);
    assert_eq!(look.age_ms, 20_000);
    assert!(!look.delivered);
    assert_eq!(look.repeats, 0);
    assert_eq!(look.coordinator_busy, Some(false));
    let asked = ask(&look);
    assert!(
        !asked.state.to_string().contains("읽지 않는다"),
        "a letter's words never reach the question"
    );
}

/// The receipts a live ledger holds reach the label as the facts it reads:
/// the coordinator's own — its seat's session — less the inbox's, and a
/// worker's never.
#[test]
fn a_live_ledgers_receipts_reach_the_label() {
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
    let address = format!("run:{run_id}");
    let done = ledger
        .post(
            &run_id,
            Draft {
                from: worker_address(&started.worker),
                to: address.clone(),
                kind: MessageKind::WorkerDone,
                body: Text::from(r#"{"ok":true,"summary":"reviewed words the label never reads"}"#),
                subject: Text::default(),
                priority: Priority::Normal,
                payload: Text::default(),
                thread: None,
                task: Some(task.clone()),
                dispatch: started.dispatch.clone(),
            },
            10,
        )
        .expect("a report");
    let mut projected: LedgerProjectionV1 = ledger.export();
    let receipt = |caller: &str, verb: &str, at: i64, answer: ServedAnswer| ServedRow {
        caller: Some(Text::from(caller)),
        request: Text::from(format!("r-{verb}-{at}")),
        answer,
        fingerprint: None,
        verb: Some(verb.to_string()),
        filed_ms: Some(at),
        expired: false,
    };
    projected.served.push(receipt(
        WORKER_ACTOR,
        "send",
        10,
        ServedAnswer::Inline(json!({ "messageId": done }).to_string()),
    ));
    projected.served.push(receipt(
        COORDINATOR,
        inbox_verb(),
        20,
        ServedAnswer::Check(CheckV1 {
            run: run_id.clone(),
            address: address.clone(),
            delivery: None,
            messages: Vec::new(),
        }),
    ));
    projected.served.push(receipt(
        COORDINATOR,
        "task-update",
        30,
        ServedAnswer::Inline(
            json!({ "taskId": task, "status": "completed", "author": "coordinator" }).to_string(),
        ),
    ));
    let ledger = Ledger::rebuild(projected).expect("the ledger with its receipts");
    let receipts = Filed::of_ledger(&ledger, 5);
    assert_eq!(receipts.len(), 3);
    assert_eq!(
        receipts[1].check.as_ref().map(|one| one.address.as_str()),
        Some(address.as_str())
    );
    assert_eq!(
        Filed::of_ledger(&ledger, 25).len(),
        1,
        "filed after the start only"
    );
    let run = ledger.run(&run_id).expect("the run");
    let room = Mailroom::of_run(run, &receipts);
    assert_eq!(room.address(), address);
    assert_eq!(room.acts().len(), 1, "the coordinator's review alone");
    let letter = room.message(&done).expect("the report");
    let labeled = room
        .label(letter, room.start_of(letter, None), 100)
        .expect("decided")
        .expect("compared");
    assert_eq!(labeled.handled_by, Some("task-update"));
    assert_eq!(labeled.truth, Triage::AnswerNow);
    assert!(labeled.urgent);
}

/* ---- the replay: this machine's mail, read and never asked --------------- */

/// A replay's seed (`tools/mail-triage-replay/seed.py`): one ledger's runs —
/// their messages without a word, their attempts, the batch open and the ids
/// pending — and every receipt with its answer cut to ids.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Seed {
    schema: u32,
    ledger_id: String,
    read_at_ms: i64,
    from_ms: i64,
    runs: Vec<SeedRun>,
    receipts: Vec<SeedReceipt>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SeedRun {
    id: String,
    address: String,
    seat_actor: Option<String>,
    messages: Vec<Message>,
    dispatches: Vec<Dispatch>,
    open: Option<Delivery>,
    pending: Vec<String>,
    acked: Vec<SeedBatch>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SeedBatch {
    delivery: String,
    messages: Vec<String>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SeedReceipt {
    caller: Option<String>,
    verb: Option<String>,
    filed_ms: Option<i64>,
    answer: Value,
    check: Option<SeedCheck>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SeedCheck {
    address: String,
    delivery: Option<String>,
    messages: Vec<String>,
}

/// The seed's schema this replay reads.
const SEED_SCHEMA: u32 = 1;

/// What the replay counts: per kind, what the label said; how each start was
/// known; the kind rule beside the label; the acts it took to handle what was
/// handled; and the rule the coordinator turned down — an acknowledgement as
/// the handling of a status or a notice — on the same letters.
#[derive(Debug, Default, PartialEq)]
struct Tally {
    letters: usize,
    by_kind: BTreeMap<String, BTreeMap<&'static str, usize>>,
    starts: BTreeMap<&'static str, usize>,
    kind_rule: Confusion,
    acked_rule: Confusion,
    question_acts: Vec<usize>,
    handled_acts: Vec<usize>,
    /// The act that first named each letter's subject, however late and
    /// whatever came between — the uncut spread the two lines are drawn
    /// from, which the labels themselves cannot show past the no-look line.
    first_named_acts: Vec<usize>,
}

/// The kind rule's answers beside the label's, on the letters compared.
#[derive(Debug, Default, PartialEq)]
struct Confusion {
    compared: usize,
    agreed: usize,
    /// Of the letters the rule said `answer_now`, how many the label did.
    said_now: usize,
    said_now_right: usize,
    /// Of the letters the label said `answer_now`.
    were_now: usize,
    /// What the label said, counted.
    truths: BTreeMap<&'static str, usize>,
}

impl Confusion {
    fn count(&mut self, rule: Triage, truth: Triage) {
        self.compared += 1;
        self.agreed += usize::from(rule == truth);
        self.said_now += usize::from(rule == Triage::AnswerNow);
        self.said_now_right += usize::from(rule == Triage::AnswerNow && truth == Triage::AnswerNow);
        self.were_now += usize::from(truth == Triage::AnswerNow);
        *self.truths.entry(truth.word()).or_default() += 1;
    }

    fn json(&self) -> Value {
        let share = |part: usize, whole: usize| {
            (whole > 0).then(|| {
                let part = u32::try_from(part).unwrap_or(u32::MAX);
                let whole = u32::try_from(whole).unwrap_or(u32::MAX);
                (f64::from(part) / f64::from(whole) * 1_000.0).round() / 1_000.0
            })
        };
        json!({
            "compared": self.compared,
            "agreed": self.agreed,
            "agreement": share(self.agreed, self.compared),
            "agreementLowerBound": (self.compared > 0).then(|| {
                (crate::jev::summary::wilson_lower(self.agreed, self.compared, crate::jev::summary::WILSON_Z_95) * 1_000.0).round() / 1_000.0
            }),
            "answerNowPrecision": share(self.said_now_right, self.said_now),
            "answerNowRecall": share(self.said_now_right, self.were_now),
            "truths": self.truths,
        })
    }
}

/// The receipts of a seed, as the label reads them.
fn filed_of(seed: &Seed) -> Vec<Filed> {
    seed.receipts
        .iter()
        .map(|one| {
            Filed::of_parts(
                one.caller.clone(),
                one.verb.clone(),
                one.filed_ms,
                &one.answer,
                one.check.as_ref().map(|looked| Looked {
                    address: looked.address.clone(),
                    messages: looked.messages.clone(),
                }),
            )
        })
        .collect()
}

/// When each acknowledged batch of `run`'s was spent, as the turned-down rule
/// reads it, by delivery: the first `check` of the run's inbox after the
/// batch's newest letter that no longer held it open.
fn acknowledged_at(run: &SeedRun, seed: &Seed) -> HashMap<String, i64> {
    let written: HashMap<&str, i64> = run
        .messages
        .iter()
        .map(|one| (one.id.as_str(), one.created_ms))
        .collect();
    let mut looks: Vec<(i64, Option<&str>)> = seed
        .receipts
        .iter()
        .filter_map(|one| {
            let looked = one.check.as_ref()?;
            (looked.address == run.address).then_some((one.filed_ms?, looked.delivery.as_deref()))
        })
        .collect();
    looks.sort_unstable();
    let mut acked = HashMap::new();
    for batch in &run.acked {
        let newest = batch
            .messages
            .iter()
            .filter_map(|id| written.get(id.as_str()))
            .max();
        let Some(newest) = newest else { continue };
        if let Some((at, _)) = looks
            .iter()
            .find(|(at, open)| at > newest && *open != Some(batch.delivery.as_str()))
        {
            acked.insert(batch.delivery.clone(), *at);
        }
    }
    acked
}

/// Label every letter of `seed` written at or after its `fromMs`, and count.
fn replay(seed: &Seed) -> Tally {
    assert_eq!(seed.schema, SEED_SCHEMA, "a seed this replay cannot read");
    let receipts = filed_of(seed);
    let mut tally = Tally::default();
    let mut seen = std::collections::HashSet::new();
    for run in &seed.runs {
        assert!(
            seen.insert(run.id.as_str()),
            "the seed holds run {} twice",
            run.id
        );
        let pending: Vec<&str> = run.pending.iter().map(String::as_str).collect();
        let room = Mailroom::new(
            run.address.clone(),
            run.seat_actor.as_deref(),
            &run.messages,
            &run.dispatches,
            run.open.as_ref(),
            &pending,
            &receipts,
        );
        // The turned-down rule's acts: the coordinator's own, and one more for
        // each acknowledgement, which spends its whole batch at once.
        let acked = acknowledged_at(run, seed);
        let empty = Names::default();
        let mut with_acks: Vec<(Act<'_>, Option<&SeedBatch>)> =
            room.acts().iter().map(|act| (*act, None)).collect();
        for batch in &run.acked {
            if let Some(at) = acked.get(&batch.delivery) {
                with_acks.push((
                    Act {
                        at_ms: *at,
                        verb: Some(inbox_verb()),
                        names: &empty,
                        message: None,
                    },
                    Some(batch),
                ));
            }
        }
        with_acks.sort_by_key(|(act, _)| act.at_ms);
        let acts_a: Vec<Act<'_>> = with_acks.iter().map(|(act, _)| *act).collect();
        for letter in room.letters().filter(|one| one.created_ms >= seed.from_ms) {
            tally.letters += 1;
            let start = room.start_of(letter, None);
            *tally.starts.entry(start.word()).or_default() += 1;
            if let Some(at) = room
                .acts()
                .iter()
                .filter(|act| act.at_ms > start.at())
                .position(|act| room.handles(letter, act))
            {
                tally.first_named_acts.push(at + 1);
            }
            let outcome = room.label(letter, start, seed.read_at_ms);
            let said = match &outcome {
                None => "open",
                Some(Err(why)) => why.word(),
                Some(Ok(labeled)) => labeled.truth.word(),
            };
            *tally
                .by_kind
                .entry(letter.kind.as_str().to_string())
                .or_default()
                .entry(said)
                .or_default() += 1;
            if let Some(Ok(labeled)) = &outcome {
                tally.kind_rule.count(kind_rule(letter.kind), labeled.truth);
                // Handled is every truth but "no look": an act an older
                // window filed with no verb handles a letter too.
                if labeled.truth != Triage::NoNeed {
                    tally.handled_acts.push(labeled.after_actions);
                    if letter.kind == MessageKind::Question {
                        tally.question_acts.push(labeled.after_actions);
                    }
                }
            }
            // The turned-down rule: a status or a notice is handled by the
            // acknowledgement of its batch, and acknowledgements are acts.
            let by_ack = handled_by(letter.kind) == Subject::ALL.as_slice();
            let turned_down = label_over(
                &acts_a,
                start,
                room.newer_at(letter),
                letter.created_ms,
                seed.read_at_ms,
                |at, act| {
                    room.handles(letter, act)
                        || (by_ack
                            && with_acks[at]
                                .1
                                .is_some_and(|batch| batch.messages.contains(&letter.id)))
                },
            );
            if let Some(Ok(labeled)) = turned_down {
                tally
                    .acked_rule
                    .count(kind_rule(letter.kind), labeled.truth);
            }
        }
    }
    tally
}

fn percentile(values: &[usize], share: f64) -> Option<usize> {
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    let last = sorted.len().checked_sub(1)?;
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss
    )]
    let at = ((last as f64) * share).round() as usize;
    sorted.get(at.min(last)).copied()
}

fn tally_json(seed: &Seed, tally: &Tally) -> Value {
    let spread = |values: &[usize]| {
        json!({
            "n": values.len(),
            "p50": percentile(values, 0.5),
            "p90": percentile(values, 0.9),
            "p95": percentile(values, 0.95),
            "p99": percentile(values, 0.99),
            "first": values.iter().filter(|n| **n == 1).count(),
            "withinAnswerNow": values.iter().filter(|n| **n <= ANSWER_NOW_WITHIN_ACTS).count(),
            "withinNoNeedLine": values.iter().filter(|n| **n <= NO_NEED_AFTER_ACTS).count(),
        })
    };
    json!({
        "ledger": seed.ledger_id,
        "fromMs": seed.from_ms,
        "readAtMs": seed.read_at_ms,
        "letters": tally.letters,
        "byKind": tally.by_kind,
        "starts": tally.starts,
        "kindRule": tally.kind_rule.json(),
        "ackedRule": tally.acked_rule.json(),
        "questionActs": spread(&tally.question_acts),
        "handledActs": spread(&tally.handled_acts),
        "firstNamedActs": spread(&tally.first_named_acts),
        "answerNowWithinActs": ANSWER_NOW_WITHIN_ACTS,
        "noNeedAfterActs": NO_NEED_AFTER_ACTS,
        "jevRequests": 0,
    })
}

/// A small seed in the shape the collector writes, replayed: a question
/// answered at once, a finished task reviewed after five acts, a silence
/// told twice and a status nothing named — and the turned-down rule beside
/// it, under which the acknowledgements alone handle the status and the
/// silence at once.
#[test]
fn a_seed_replays_into_the_labels_and_the_kind_rule_beside_them() {
    let seed: Seed = serde_json::from_value(json!({
        "schema": SEED_SCHEMA,
        "ledgerId": "main-ledger",
        "readAtMs": 100_000,
        "fromMs": 0,
        "runs": [{
            "id": "run-1",
            "address": ADDRESS,
            "seatActor": COORDINATOR,
            "messages": [
                { "id": "m-1", "from": "worker:w-1", "to": ADDRESS, "kind": "question", "body": "",
                  "thread": null, "task": "t-1", "dispatch": "dp-1", "created_ms": 100 },
                { "id": "m-2", "from": ADDRESS, "to": "worker:w-1", "kind": "question", "body": "",
                  "thread": "m-1", "task": null, "dispatch": null, "created_ms": 200 },
                { "id": "m-3", "from": "worker:w-2", "to": ADDRESS, "kind": "worker_done", "body": "",
                  "thread": null, "task": "t-2", "dispatch": "dp-2", "created_ms": 300 },
                { "id": "m-4", "from": "ledger", "to": ADDRESS, "kind": "went_quiet", "body": "",
                  "thread": null, "task": "t-3", "dispatch": "dp-3", "created_ms": 310 },
                { "id": "m-5", "from": "ledger", "to": ADDRESS, "kind": "went_quiet", "body": "",
                  "thread": null, "task": "t-3", "dispatch": "dp-3", "created_ms": 320 },
                { "id": "m-6", "from": "worker:w-4", "to": ADDRESS, "kind": "status", "body": "",
                  "thread": null, "task": "t-4", "dispatch": "dp-4", "created_ms": 330 }
            ],
            "dispatches": [
                { "id": "dp-1", "task": "t-1", "worker": "w-1", "started_ms": 1, "ended_ms": null, "succeeded": null },
                { "id": "dp-2", "task": "t-2", "worker": "w-2", "started_ms": 1, "ended_ms": 300, "succeeded": true },
                { "id": "dp-3", "task": "t-3", "worker": "w-3", "started_ms": 1, "ended_ms": null, "succeeded": null },
                { "id": "dp-4", "task": "t-4", "worker": "w-4", "started_ms": 1, "ended_ms": null, "succeeded": null }
            ],
            "open": null,
            "pending": [],
            "acked": [
                { "delivery": "d-7", "messages": ["m-1"] },
                { "delivery": "d-8", "messages": ["m-3", "m-4", "m-5", "m-6"] }
            ]
        }],
        "receipts": [
            { "caller": COORDINATOR, "verb": inbox_verb(), "filedMs": 150, "answer": null,
              "check": { "address": ADDRESS, "delivery": null, "messages": [] } },
            { "caller": COORDINATOR, "verb": "reply", "filedMs": 200, "answer": { "messageId": "m-2" }, "check": null },
            { "caller": COORDINATOR, "verb": inbox_verb(), "filedMs": 340, "answer": null,
              "check": { "address": ADDRESS, "delivery": null, "messages": [] } },
            { "caller": COORDINATOR, "verb": "task-create", "filedMs": 350, "answer": { "taskId": "t-8" }, "check": null },
            { "caller": COORDINATOR, "verb": "task-create", "filedMs": 360, "answer": { "taskId": "t-9" }, "check": null },
            { "caller": COORDINATOR, "verb": "task-create", "filedMs": 370, "answer": { "taskId": "t-10" }, "check": null },
            { "caller": COORDINATOR, "verb": "task-create", "filedMs": 380, "answer": { "taskId": "t-11" }, "check": null },
            { "caller": COORDINATOR, "verb": "task-update", "filedMs": 390, "answer": { "taskId": "t-2", "status": "completed" }, "check": null },
            { "caller": COORDINATOR, "verb": "worker-stop", "filedMs": 400, "answer": { "workerId": "w-3" }, "check": null }
        ]
    }))
    .expect("the seed reads");
    let tally = replay(&seed);
    assert_eq!(
        tally.letters, 5,
        "the coordinator's own reply is not a letter"
    );
    let said = |kind: &str, word: &str| {
        tally
            .by_kind
            .get(kind)
            .and_then(|words| words.get(word))
            .copied()
            .unwrap_or(0)
    };
    assert_eq!(said("question", "answer_now"), 1);
    assert_eq!(
        said("worker_done", "can_wait"),
        1,
        "reviewed at its fifth act"
    );
    assert_eq!(
        said("went_quiet", "superseded"),
        1,
        "the older notice, told again"
    );
    assert_eq!(
        said("went_quiet", "can_wait"),
        1,
        "its worker stopped at the sixth act"
    );
    assert_eq!(
        said("status", "open"),
        1,
        "nothing named it yet, and the line is far off"
    );
    assert_eq!(tally.starts.get("created"), Some(&5));
    assert_eq!(tally.question_acts, vec![1]);
    // The kind rule beside the label: right on the question, wrong on the
    // finished task (it could wait), wrong on the silence (it was acted on).
    assert_eq!(tally.kind_rule.compared, 3);
    assert_eq!(tally.kind_rule.agreed, 1);
    // Under the turned-down rule every acknowledged status and notice is
    // handled at once: the status and the newer silence become `answer_now`
    // beside the question, and the status is compared at all.
    assert_eq!(tally.acked_rule.compared, 4);
    assert_eq!(tally.acked_rule.truths.get("answer_now"), Some(&3));
    let printed = tally_json(&seed, &tally);
    assert_eq!(printed["jevRequests"], 0);
    assert!(
        !printed.to_string().contains("w-"),
        "the replay prints numbers, not ids"
    );
}

/// The replay over this machine's own mail (t-9471 §0): the seed the
/// collector read, labeled by the rule the seat is graded by, the kind rule
/// beside it and the turned-down rule beside that — numbers only, and not
/// one Jev request.
///
/// ```sh
/// python3 tools/mail-triage-replay/seed.py --out /tmp/mail-triage/seed.json
/// ZEROCODE_MAIL_TRIAGE_REPLAY_SEED=/tmp/mail-triage/seed.json \
///   cargo test -p zerocode-core --lib \
///   mail_triage::tests::the_mail_this_machine_would_have_labeled \
///   -- --ignored --nocapture
/// ```
#[test]
#[ignore = "a measurement over the person's own ledger, printed; not a check"]
fn the_mail_this_machine_would_have_labeled() {
    let path = std::env::var("ZEROCODE_MAIL_TRIAGE_REPLAY_SEED")
        .expect("ZEROCODE_MAIL_TRIAGE_REPLAY_SEED names the collector's seed");
    let text = std::fs::read_to_string(&path).expect("the seed reads");
    let seed: Seed = serde_json::from_str(&text).expect("the seed parses");
    let tally = replay(&seed);
    println!("{}", tally_json(&seed, &tally));
}
