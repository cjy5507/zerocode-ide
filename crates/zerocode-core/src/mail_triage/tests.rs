use std::collections::{BTreeMap, HashMap};

use serde_json::{Value, json};

use super::*;
use crate::jev::batch::{self, Request, question_name};
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
/// never reads. A letter about one attempt names it after
/// (`the_ledgers_notice`).
fn posted(
    id: &str,
    from: &str,
    to: &str,
    kind: MessageKind,
    at: i64,
    thread: Option<&str>,
    task: Option<&str>,
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
        dispatch: None,
        author_seat: None,
        created_ms: at,
    }
}

/// A worker's letter to the coordinator.
fn from_worker(id: &str, worker: &str, kind: MessageKind, at: i64, task: Option<&str>) -> Message {
    posted(id, &worker_address(worker), ADDRESS, kind, at, None, task)
}

/// The coordinator's own message.
fn from_coordinator(
    id: &str,
    to: &str,
    at: i64,
    thread: Option<&str>,
    task: Option<&str>,
) -> Message {
    posted(id, ADDRESS, to, MessageKind::Status, at, thread, task)
}

/// The ledger's notice that attempt `dp-1` of task `t-1` went quiet.
fn the_ledgers_notice(id: &str, at: i64) -> Message {
    Message {
        dispatch: Some("dp-1".to_string()),
        ..posted(
            id,
            "ledger",
            ADDRESS,
            MessageKind::WentQuiet,
            at,
            None,
            Some("t-1"),
        )
    }
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
        session_history: None,
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
    assert_eq!(MAIL_TRIAGE_RUBRIC_VERSION, 2);
    assert_eq!(rubric_fingerprint(rubric_words), "082a013917c98431");
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
    }
}

/* ---- the batch road (t-32796) ------------------------------------------- */

/// What holds for every letter of the batches below.
fn situation() -> Situation {
    Situation {
        coordinator_busy: Some(true),
        open_questions: 3,
    }
}

/// Three letters of three kinds, as one run's coordinator is handed them.
fn three_letters<'a>() -> Vec<MailLook<'a>> {
    vec![
        MailLook {
            kind: MessageKind::Question,
            ..look()
        },
        MailLook {
            kind: MessageKind::WorkerDone,
            awaits_answer: false,
            repeats: 0,
            ..look()
        },
        MailLook {
            kind: MessageKind::WentQuiet,
            from: "ledger",
            worker: Some("w-9"),
            awaits_answer: false,
            repeats: 4,
            ..look()
        },
    ]
}

/// The requests `looks` are asked in.
fn requests_for(looks: &[MailLook<'_>]) -> Vec<Request> {
    batch::requests(
        &MailTriage::new(situation()),
        looks.iter().map(MailLook::facts).collect(),
    )
}

/// A reply's answers about the letters of a batch: for each `(item, chosen,
/// urgent)`, a choice and a Noul under the names the batch road gave that
/// letter's questions.
fn replies(about: &[(usize, &str, Value)]) -> Value {
    let mut answers = Map::new();
    for (item, chosen, urgent) in about {
        answers.insert(
            question_name(*item, TRIAGE_SUFFIX),
            json!({
                "type": "choice",
                "choice": chosen,
                "probabilities": { "answer_now": 0.7, "can_wait": 0.2, "no_need": 0.1 },
                "confidence": 0.6,
            }),
        );
        answers.insert(
            question_name(*item, URGENT_SUFFIX),
            json!({ "type": "noul", "noul": urgent }),
        );
    }
    Value::Object(answers)
}

/// A batch of letters is ONE request: the rubric and the coordinator's
/// situation said once, each letter in an entry of its own.
#[test]
fn a_batch_is_one_request_that_says_the_rubric_and_the_situation_once() {
    let looks = three_letters();
    let asked = requests_for(&looks);
    assert_eq!(asked.len(), 1, "three letters are one request");
    let request = &asked[0];
    assert_eq!(request.items(), 0..3);
    let state = request.state.as_object().expect("a state");
    let mut keys: Vec<&str> = state.keys().map(String::as_str).collect();
    keys.sort_unstable();
    let mut expected = REQUEST_KEYS.to_vec();
    expected.sort_unstable();
    assert_eq!(keys, expected);
    assert_eq!(state["rubric"]["about"], INSTRUCTIONS);
    for triage in Triage::ALL {
        assert_eq!(state["rubric"]["answers"][triage.word()], triage.means());
    }
    assert_eq!(state["coordinator"]["busy"], true);
    assert_eq!(state["coordinator"]["openQuestions"], 3);
    let letters = state["letters"].as_array().expect("the letters");
    assert_eq!(letters.len(), 3);
    for (entry, look) in letters.iter().zip(&looks) {
        assert_eq!(*entry, look.facts());
    }
}

/// A letter's entry carries its structure under the table's keys — the
/// texts only where the table declares them, numbers and flags elsewhere —
/// and never a word the letter says.
#[test]
fn a_letters_entry_carries_its_structure_and_never_its_words() {
    let entry = look().facts();
    let fields = entry.as_object().expect("an entry");
    let mut keys: Vec<&str> = fields.keys().map(String::as_str).collect();
    keys.sort_unstable();
    let mut expected = LETTER_KEYS.to_vec();
    expected.sort_unstable();
    assert_eq!(keys, expected);
    assert_eq!(entry["kind"], "question");
    assert_eq!(entry["from"], "worker");
    assert_eq!(entry["worker"], "w-7");
    assert_eq!(entry["task"], "t-3");
    assert_eq!(entry["taskStatus"], "dispatched");
    assert_eq!(entry["priority"], "normal");
    assert_eq!(entry["awaitsAnswer"], true);
    assert_eq!(entry["threadDepth"], 0);
    assert_eq!(entry["ageSeconds"], 42, "seconds a person would say");
    assert_eq!(entry["delivered"], false);
    assert_eq!(entry["repeats"], 2);
    // Every text in the entry is one the table declares; the rest are not
    // texts at all.
    let declared: Vec<&str> = MAIL_TRIAGE
        .sends
        .iter()
        .filter_map(|sent| sent.at.strip_prefix("/state/letters/*/"))
        .collect();
    for (key, value) in fields {
        assert_eq!(
            value.is_string(),
            declared.contains(&key.as_str()),
            "{key}: {value}"
        );
    }
    // A negative age is none.
    let early = MailLook {
        age_ms: -5,
        ..look()
    };
    assert_eq!(early.facts()["ageSeconds"], 0);
}

/// Each letter is asked one closed choice and one Noul, by its place in the
/// list; the options say what they mean in a line, and the rubric — said
/// once, in the state — is not said again in them.
#[test]
fn each_letter_is_asked_a_choice_and_a_noul_by_its_place_and_the_rubric_is_not_repeated() {
    let looks = three_letters();
    let asked = requests_for(&looks);
    assert_eq!(asked.len(), 1);
    let request = &asked[0];
    let questions = request.questions.as_object().expect("questions");
    assert_eq!(questions.len(), 6, "two about each of three letters");
    let mut words: Vec<&str> = Triage::ALL.map(Triage::word).to_vec();
    words.sort_unstable();
    for (at, item) in request.items().enumerate() {
        let choice = &questions[&question_name(item, TRIAGE_SUFFIX)];
        assert_eq!(choice["type"], "choice");
        let said = choice["instructions"].as_str().expect("words");
        assert!(said.contains(&format!("letters[{at}]")), "{said}");
        let criteria = choice["criteria"].as_object().expect("the options");
        let mut offered: Vec<&str> = criteria.keys().map(String::as_str).collect();
        offered.sort_unstable();
        assert_eq!(offered, words);
        for triage in Triage::ALL {
            assert_eq!(
                criteria[triage.word()],
                triage.gist(),
                "an option says what it means in a line"
            );
        }
        let urgent = &questions[&question_name(item, URGENT_SUFFIX)];
        assert_eq!(urgent["type"], "noul");
        let said = urgent["instructions"].as_str().expect("words");
        assert!(said.contains(&format!("letters[{at}]")), "{said}");
    }
    let whole = json!({ "state": request.state, "questions": request.questions }).to_string();
    assert_eq!(whole.matches(INSTRUCTIONS).count(), 1, "the rubric, once");
    for triage in Triage::ALL {
        assert_eq!(
            whole.matches(triage.means()).count(),
            1,
            "{}: what the option means, once",
            triage.word()
        );
    }
}

/// A batch above the cap is cut evenly, and every request of it says the
/// rubric once.
#[test]
fn a_batch_above_the_cap_is_cut_evenly_and_every_request_says_the_rubric_once() {
    let letters = MAIL_TRIAGE_BATCH_CAP * 2 + 1;
    let looks: Vec<MailLook<'_>> = (0..letters).map(|_| look()).collect();
    let asked = requests_for(&looks);
    assert_eq!(
        asked.len(),
        3,
        "{letters} letters at a cap of {MAIL_TRIAGE_BATCH_CAP}"
    );
    let sizes: Vec<usize> = asked.iter().map(|request| request.items().len()).collect();
    assert_eq!(sizes.iter().sum::<usize>(), letters, "{sizes:?}");
    let widest = sizes.iter().max().copied().unwrap_or(0);
    let narrowest = sizes.iter().min().copied().unwrap_or(0);
    assert!(widest <= MAIL_TRIAGE_BATCH_CAP, "{sizes:?}");
    assert!(widest - narrowest <= 1, "{sizes:?}");
    for request in &asked {
        let whole = json!({ "state": request.state, "questions": request.questions }).to_string();
        assert_eq!(whole.matches(INSTRUCTIONS).count(), 1);
        assert_eq!(
            request.state["letters"].as_array().expect("letters").len(),
            request.items().len()
        );
    }
}

/// A letter's answer needs both heads in shape; one broken rule in either
/// discards that answer whole, and says which.
#[test]
fn a_letters_answer_needs_both_heads_or_is_refused_whole() {
    let looks = three_letters();
    let asked = requests_for(&looks);
    assert_eq!(asked.len(), 1);
    let judgment = MailTriage::new(situation());
    let read = asked[0].read(
        &judgment,
        &replies(&[
            (0, "answer_now", json!(0.8)),
            (1, "can_wait", json!(0.3)),
            (2, "no_need", json!(0.1)),
        ]),
    );
    assert_eq!(read.len(), 3);
    let first = read[0].as_ref().expect("in shape");
    assert_eq!(first.triage, Triage::AnswerNow);
    assert_eq!(first.confidence, 0.6);
    assert_eq!(first.urgent, 0.8);
    assert_eq!(first.probabilities.len(), 3);
    assert_eq!(read[1].as_ref().expect("in shape").triage, Triage::CanWait);
    assert_eq!(read[2].as_ref().expect("in shape").triage, Triage::NoNeed);
    assert_eq!(
        MailRefusal::Urgent(NoulRefusal::NoAnswer).token(),
        "schema_no_noul"
    );
    assert_eq!(
        MailRefusal::Triage(ChoiceRefusal::NotOne).token(),
        "schema_not_one"
    );
}

/// Stage one's rule, held for the mail: a letter whose answer breaks a rule
/// is refused alone, by the rule it broke, and the letters beside it are
/// read as they would have been had nothing broken.
#[test]
fn a_letters_broken_answer_discards_that_letter_alone() {
    let looks = vec![look(); 4];
    let asked = requests_for(&looks);
    assert_eq!(asked.len(), 1);
    let judgment = MailTriage::new(situation());
    let mut about = replies(&[
        (0, "later", json!(0.8)),
        (1, "answer_now", json!(1.5)),
        (2, "answer_now", json!(0.8)),
        (3, "can_wait", json!(0.2)),
    ]);
    about
        .as_object_mut()
        .expect("answers")
        .remove(&question_name(2, URGENT_SUFFIX));
    let read = asked[0].read(&judgment, &about);
    assert_eq!(read.len(), 4);
    assert_eq!(
        read[0],
        Err(MailRefusal::Triage(ChoiceRefusal::UnknownOption))
    );
    assert_eq!(read[1], Err(MailRefusal::Urgent(NoulRefusal::OutOfRange)));
    assert_eq!(read[2], Err(MailRefusal::Urgent(NoulRefusal::NoAnswer)));
    let kept = read[3].as_ref().expect("the letter beside them is read");
    assert_eq!(kept.triage, Triage::CanWait);
    assert_eq!(kept.urgent, 0.2);
}

/// The door cuts a batch to the table's cap and never sends it whole: the
/// road itself never builds a request over the cap, which is what makes the
/// door's cut a belt beside its braces.
#[test]
fn the_door_cuts_a_batch_to_the_cap_and_never_sends_it_whole() {
    use crate::jev::door::{Asking, JevSettings, may_send};

    let letters = MAIL_TRIAGE_BATCH_CAP + 5;
    let facts: Vec<Value> = (0..letters).map(|_| look().facts()).collect();
    let body = json!({ "state": { "letters": facts }, "questions": {} });
    let settings = JevSettings::from_root(
        &json!({ "smart": { "jev": { "enabled": true, "workspaces": ["*"] } } }),
    );
    let cleared = may_send(
        &MAIL_TRIAGE,
        &Asking {
            key: true,
            settings: &settings,
            workspace: Some("/work/checkout"),
            sent_today: 0,
        },
        body,
    )
    .expect("the door lets it through");
    let sent: Value = serde_json::from_slice(cleared.bytes()).expect("what leaves");
    assert_eq!(
        sent["state"]["letters"].as_array().expect("letters").len(),
        MAIL_TRIAGE_BATCH_CAP
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
    let notice = the_ledgers_notice("m-1", 100);
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
    let messages = vec![
        the_ledgers_notice("m-1", 100),
        the_ledgers_notice("m-2", 400),
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
/// its worker and task and where the task stands, its depth and how many
/// letters about the same thing came before it — and, once for the run, how
/// many questions wait on the coordinator.
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
    let situation = room.situation(run, Some(false));
    assert_eq!(
        situation,
        Situation {
            coordinator_busy: Some(false),
            open_questions: 1,
        }
    );
    let look = room.look(run, letter, 20_010);
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
    let asked = requests_for(&[look]);
    assert!(
        !asked[0].state.to_string().contains("읽지 않는다"),
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
