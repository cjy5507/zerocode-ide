//! t-121651 (I-t119309-7): the build line's end notice reaches the worker
//! that put the job in.
//!
//! 2026-11-20, as it happened: `zo line add --notify worker:w-119748` told the
//! job's end through `zerocode-orc send` in the environment of the pane that
//! put the job in, so the letter (m-120865) was the worker's own words to
//! itself. The ledger filed it and never handed it over — `check --peek`
//! counted one letter, `check` none, and no pointer came — because a seat is
//! not handed back what it wrote to the address it reads. That rule is for
//! `run:<id>`, which more than one seat reads. An address one seat alone reads
//! (`worker:` · `pane:`) now hands that seat a root letter it signed as that
//! address and sent there; a reply, a line the ledger writes in the seat's
//! name and every `run:` letter are held back from it as before.

use super::letter_answers::{as_leader_of, ask_another_run, run_of};
use super::*;

/// A status as `zo line` tells a job's end (its `send_args`): to `to`, under
/// the job's own retry name, in the run of the pane that put the job in — sent
/// from `pane`. Answers the letter's id.
fn line_notice(bench: &mut Bench, pane: &str, run_id: &str, to: &str, job: &str) -> String {
    let body = format!("zo line {job}: cargo test -p zerocode-core --lib — passed (rc 0, 41s)");
    let retry = format!("zo-line-{job}");
    let planned = bench.at_argv(
        pane,
        [
            "send",
            "--to",
            to,
            "--type",
            "status",
            "--body",
            body.as_str(),
            "--retry-request",
            retry.as_str(),
            "--run",
            run_id,
        ]
        .map(str::to_string)
        .to_vec(),
    );
    assert_eq!(planned.reply.exit_code, 0, "{}", planned.reply.stderr);
    let said: serde_json::Value =
        serde_json::from_str(&planned.reply.stdout).expect("send answers JSON");
    message_id(&said)
}

/// The id a verb's answer names.
fn message_id(said: &serde_json::Value) -> String {
    said["messageId"]
        .as_str()
        .expect("a message id")
        .to_string()
}

/// The ids of the letters one look handed over, in the order it handed them.
fn ids(read: &serde_json::Value) -> Vec<String> {
    read["messages"]
        .as_array()
        .map(|messages| {
            messages
                .iter()
                .filter_map(|message| message["messageId"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// A run the bench's own leader opens, and so sits in the chair of.
fn opened(bench: &mut Bench, name: &str) -> String {
    bench.json(&format!("run-create --name {name}"))["runId"]
        .as_str()
        .expect("a run id")
        .to_string()
}

/// The seat behind one of the bench's own panes, as the ledger signs it.
fn seat_of(bench: &Bench, pane: &str) -> String {
    format!("{}/{pane}", bench.team.id)
}

/// The case as it happened: a worker carrying a task puts a job in with
/// `--notify worker:<itself>`, and the line tells the job's end as that
/// worker. Its pointer counts the notice and its `check` hands it over, once:
/// after the ack nothing comes back, and reading it wrote nothing.
#[test]
fn the_build_lines_end_notice_reaches_the_worker_that_put_the_job_in() {
    let mut bench = Bench::new();
    let run_id = opened(&mut bench, "line-notice");
    let task = bench.json("task-create --spec build-and-test")["taskId"]
        .as_str()
        .expect("a task id")
        .to_string();
    let (worker, pane) = bench.seat(&format!("worker-start --agent codex --task {task}"));
    let address = worker_address(&worker);
    let seat = seat_of(&bench, &pane);

    let notice = line_notice(&mut bench, &pane, &run_id, &address, "j-1");
    let run = bench.ledger.run(&run_id).expect("the run");
    let filed = run.message(&notice).expect("the notice is filed");
    assert_eq!(
        (
            filed.from.as_str(),
            filed.to.as_str(),
            filed.author_seat.as_deref(),
            filed.thread.as_deref(),
        ),
        (
            address.as_str(),
            address.as_str(),
            Some(seat.as_str()),
            None
        ),
        "the notice is not the worker's own root letter to itself"
    );
    assert_eq!(
        run.pointer_wanted(&address, Some(&seat)),
        Some(1),
        "the worker's pointer did not count its notice"
    );
    assert_eq!(run.unread_wanted(&address, Some(&seat)), Some(1));

    let read = bench.json_at(&pane, "check");
    assert_eq!(
        ids(&read),
        [notice],
        "the worker's check did not hand over its notice: {read}"
    );
    let batch = read["deliveryId"].as_str().expect("a batch").to_string();
    let written = bench.ledger.run(&run_id).expect("the run").messages().len();
    bench.json_at(&pane, &format!("check --ack {batch} --peek"));
    assert_eq!(
        bench.json_at(&pane, "check")["count"],
        0,
        "the notice came back after its ack"
    );
    let run = bench.ledger.run(&run_id).expect("the run");
    assert_eq!(
        run.messages().len(),
        written,
        "reading the notice wrote a letter"
    );
    assert_eq!(run.pointer_wanted(&address, Some(&seat)), None);
    assert_eq!(run.unread_wanted(&address, Some(&seat)), None);
}

/// Beside what other seats write to the worker — its coordinator, and a pane
/// of no run — the note is handed over in the place it was filed, and the
/// pointer counts all three.
#[test]
fn a_note_to_self_is_handed_over_in_its_place_among_other_seats_letters() {
    let mut bench = Bench::new();
    let run_id = opened(&mut bench, "line-notice-among");
    let (worker, pane) = bench.seat("worker-start --agent codex");
    let address = worker_address(&worker);
    let seat = seat_of(&bench, &pane);

    let first = message_id(&bench.json(&format!(
        "send --to {address} --type status --body from-the-coordinator"
    )));
    let note = line_notice(&mut bench, &pane, &run_id, &address, "j-2");
    let last = bench.peer_message_to(
        &address,
        MessageKind::Status,
        "from-a-pane-of-no-run",
        "",
        Priority::Normal,
        "",
    );

    assert_eq!(
        bench
            .ledger
            .run(&run_id)
            .expect("the run")
            .pointer_wanted(&address, Some(&seat)),
        Some(3),
        "the pointer left the worker's note out of its count"
    );
    let read = bench.json_at(&pane, "check");
    assert_eq!(ids(&read), [first, note, last], "{read}");
}

/// The line tells a job's end once, under the job's own retry name. Sent
/// again under that name it is the same letter — nothing more is filed — and
/// the worker is handed it once: a second look before the ack is the same
/// batch, and after the ack nothing comes back.
#[test]
fn a_notice_sent_again_under_its_retry_name_is_one_letter_handed_over_once() {
    let mut bench = Bench::new();
    let run_id = opened(&mut bench, "line-notice-retried");
    let (worker, pane) = bench.seat("worker-start --agent codex");
    let address = worker_address(&worker);

    let notice = line_notice(&mut bench, &pane, &run_id, &address, "j-3");
    let written = bench.ledger.run(&run_id).expect("the run").messages().len();
    let again = line_notice(&mut bench, &pane, &run_id, &address, "j-3");
    assert_eq!(
        again, notice,
        "the retried notice was filed as a second letter"
    );
    assert_eq!(
        bench.ledger.run(&run_id).expect("the run").messages().len(),
        written,
        "the retried notice wrote a row"
    );

    let read = bench.json_at(&pane, "check");
    assert_eq!(ids(&read), [notice.clone()], "{read}");
    let batch = read["deliveryId"].as_str().expect("a batch").to_string();
    let replayed = bench.json_at(&pane, "check");
    assert_eq!(replayed["deliveryId"], batch.as_str(), "{replayed}");
    assert_eq!(ids(&replayed), [notice], "{replayed}");
    bench.json_at(&pane, &format!("check --ack {batch} --peek"));
    assert_eq!(
        bench.json_at(&pane, "check")["count"],
        0,
        "the notice was handed over twice"
    );
}

/// A pane that is nobody's worker reads its own `pane:` address, and nobody
/// else does: a note it writes there is counted by its pointer and handed
/// back by its `check --run`.
#[test]
fn a_panes_note_to_its_own_pane_address_reaches_its_check() {
    let mut bench = Bench::new();
    let run_id = opened(&mut bench, "pane-note");
    let mut loose = Team::new("team-loose", "loose", 70);
    let seat = format!("{}/{}", loose.id, agent_teams::LEADER_PANE);
    let address = format!("{PANE_ADDRESS_PREFIX}{seat}");
    // A pane is written to once it has spoken in the run.
    as_leader_of(
        &mut bench,
        &mut loose,
        &[
            "send", "--run", &run_id, "--type", "status", "--body", "here",
        ],
    );
    let note = message_id(&as_leader_of(
        &mut bench,
        &mut loose,
        &[
            "send",
            "--run",
            &run_id,
            "--to",
            &address,
            "--type",
            "status",
            "--body",
            "a-note-to-myself",
        ],
    ));
    let run = bench.ledger.run(&run_id).expect("the run");
    let filed = run.message(&note).expect("the note is filed");
    assert_eq!(
        (filed.from.as_str(), filed.author_seat.as_deref()),
        (address.as_str(), Some(seat.as_str())),
        "the note is not the pane's own letter"
    );
    assert_eq!(
        run.pointer_wanted(&address, Some(&seat)),
        Some(1),
        "the pane's pointer did not count its note"
    );

    let read = as_leader_of(&mut bench, &mut loose, &["check", "--run", &run_id]);
    assert_eq!(ids(&read), [note], "{read}");
}

/// `run:<id>` has a reader for every leader pane bound to the run, so the rule
/// there is as it was: a coordinator that puts a job in with
/// `--notify run:<its run>` is not handed its own notice — the seat beside it
/// would be — and its `check` stays quiet.
#[test]
fn a_coordinators_own_notice_to_its_run_is_held_back_from_it_as_before() {
    let mut bench = Bench::new();
    let run_id = opened(&mut bench, "run-notice");
    let address = format!("{RUN_ADDRESS_PREFIX}{run_id}");
    let seat = seat_of(&bench, agent_teams::LEADER_PANE);

    let notice = line_notice(
        &mut bench,
        agent_teams::LEADER_PANE,
        &run_id,
        &address,
        "j-4",
    );
    let run = bench.ledger.run(&run_id).expect("the run");
    assert_eq!(
        run.message(&notice).expect("the notice is filed").from,
        address,
        "the coordinator did not sign as its run"
    );
    assert_eq!(
        run.pending_messages(&address, &[]).len(),
        1,
        "the notice is not queued for the run's other readers"
    );
    assert_eq!(
        run.pointer_wanted(&address, Some(&seat)),
        None,
        "the coordinator was pointed at its own notice"
    );
    assert_eq!(
        run.pointer_wanted(&address, Some("team-2/%1")),
        Some(1),
        "the seat beside the coordinator was told nothing"
    );
    assert_eq!(
        bench.json("check")["count"],
        0,
        "the coordinator was handed its own notice"
    );
}

/// Nor does `--notify pane:<its own pane>` reach a coordinator (I-t121651-1).
/// The chair signs as its run, so in its own run its pane address has never
/// spoken, and the line's letter there is refused rather than filed — the
/// chair reads `run:<id>`, where its own words are held back. Test 4's pane
/// is nobody's worker and sits in no chair.
#[test]
fn a_coordinators_notice_to_its_own_pane_address_is_refused_in_its_run() {
    let mut bench = Bench::new();
    let run_id = opened(&mut bench, "chair-pane-notice");
    let address = format!(
        "{PANE_ADDRESS_PREFIX}{}",
        seat_of(&bench, agent_teams::LEADER_PANE)
    );
    let written = bench.ledger.run(&run_id).expect("the run").messages().len();

    let planned = bench.at_argv(
        agent_teams::LEADER_PANE,
        [
            "send",
            "--to",
            address.as_str(),
            "--type",
            "status",
            "--body",
            "zo line j-6: gate — passed (rc 0, 9s)",
            "--retry-request",
            "zo-line-j-6",
            "--run",
            run_id.as_str(),
        ]
        .map(str::to_string)
        .to_vec(),
    );
    assert_ne!(
        planned.reply.exit_code, 0,
        "the chair's pane notice was filed: {}",
        planned.reply.stdout
    );
    assert!(
        planned
            .reply
            .stderr
            .contains("a seat is answerable once it has spoken"),
        "{}",
        planned.reply.stderr
    );
    assert_eq!(
        bench.ledger.run(&run_id).expect("the run").messages().len(),
        written,
        "the refused notice wrote a row"
    );
    assert_eq!(bench.json("check")["count"], 0);
}

/// A word in a thread is no note. A worker that answers its own notice files
/// the answer at its own address, and the answer is not handed back to it: a
/// note does not start a round of a worker answering itself.
#[test]
fn a_reply_to_a_note_to_self_is_not_handed_back_to_be_answered_again() {
    let mut bench = Bench::new();
    let run_id = opened(&mut bench, "note-reply");
    let (worker, pane) = bench.seat("worker-start --agent codex");
    let address = worker_address(&worker);
    let seat = seat_of(&bench, &pane);

    let note = line_notice(&mut bench, &pane, &run_id, &address, "j-5");
    let reply =
        message_id(&bench.json_at(&pane, &format!("reply --to-message {note} --body got-it")));
    let run = bench.ledger.run(&run_id).expect("the run");
    let answer = run.message(&reply).expect("the reply is filed");
    assert_eq!(
        (answer.to.as_str(), answer.thread.as_deref()),
        (address.as_str(), Some(note.as_str())),
        "the reply is not the worker's answer to its note, at its own address"
    );

    let read = bench.json_at(&pane, "check");
    assert!(
        !ids(&read).contains(&reply),
        "the worker was handed its own reply: {read}"
    );
    if let Some(batch) = read["deliveryId"].as_str() {
        bench.json_at(&pane, &format!("check --ack {batch} --peek"));
    }
    assert_eq!(
        bench.json_at(&pane, "check")["count"],
        0,
        "the reply came back to be answered"
    );
    assert_eq!(
        bench
            .ledger
            .run(&run_id)
            .expect("the run")
            .pointer_wanted(&address, Some(&seat)),
        None,
        "the worker was pointed at its own reply"
    );
}

/// What other seats write to a worker reaches it as it did, and what the
/// worker reports to its run goes to the run: its own report is not in its
/// `check`, and its coordinator is handed it.
#[test]
fn letters_other_seats_write_to_a_worker_reach_it_as_they_did() {
    let mut bench = Bench::new();
    let run_id = opened(&mut bench, "others-letters");
    let (worker, pane) = bench.seat("worker-start --agent codex");
    let address = worker_address(&worker);
    let seat = seat_of(&bench, &pane);

    let from_coordinator = message_id(&bench.json(&format!(
        "send --to {address} --type status --body go-ahead"
    )));
    let from_a_pane = bench.peer_message_to(
        &address,
        MessageKind::Status,
        "from-a-pane-of-no-run",
        "",
        Priority::Normal,
        "",
    );
    let report = message_id(&bench.json_at(&pane, "send --type status --body working-on-it"));

    assert_eq!(
        bench
            .ledger
            .run(&run_id)
            .expect("the run")
            .pointer_wanted(&address, Some(&seat)),
        Some(2)
    );
    let read = bench.json_at(&pane, "check");
    assert_eq!(ids(&read), [from_coordinator, from_a_pane], "{read}");
    let coordinator = bench.json("check");
    assert!(
        ids(&coordinator).contains(&report),
        "the coordinator was not handed the worker's report: {coordinator}"
    );
}

/// A line the ledger writes with a seat's name on it is no note of that
/// seat's: a chair that closes the question it asked another run is not
/// handed the close the ledger files at its pane address there, as before.
#[test]
fn the_close_of_a_question_its_own_asker_closed_is_held_back_from_it_as_before() {
    let mut bench = Bench::new();
    let mut asker = Team::new("team-asker", "asker", 60);
    let mut answerer = Team::new("team-answerer", "answerer", 70);
    let asking = run_of(&mut bench, &mut asker, "asker");
    let answering = run_of(&mut bench, &mut answerer, "answerer");
    let chair = format!("{RUN_ADDRESS_PREFIX}{asking}");
    as_leader_of(
        &mut bench,
        &mut asker,
        &[
            "send", "--run", &asking, "--to", &chair, "--type", "status", "--body", "on-duty",
        ],
    );
    let question = ask_another_run(&mut bench, &mut asker, &answering, "still-needed?");
    let closed = as_leader_of(
        &mut bench,
        &mut asker,
        &[
            "question-close",
            "--message",
            &question,
            "--reason",
            "no-longer-needed",
        ],
    );
    assert_eq!(closed["closed"], true, "{closed}");
    let close = message_of(&closed, "closeId");
    let seat = format!("{}/{}", asker.id, agent_teams::LEADER_PANE);
    let address = format!("{PANE_ADDRESS_PREFIX}{seat}");

    let run = bench.ledger.run(&answering).expect("the run");
    let line = run.message(&close).expect("the close is filed");
    assert_eq!(
        (
            line.from.as_str(),
            line.to.as_str(),
            line.author_seat.as_deref(),
            line.thread.as_deref(),
        ),
        (
            LEDGER_ITSELF,
            address.as_str(),
            Some(seat.as_str()),
            Some(question.as_str()),
        ),
        "the close is not the ledger's line at the asker's pane in the asker's name"
    );
    assert_eq!(
        run.pointer_wanted(&address, Some(&seat)),
        None,
        "the asker was pointed at the close it made"
    );
    let read = as_leader_of(&mut bench, &mut asker, &["check", "--run", &answering]);
    assert!(
        !ids(&read).contains(&close),
        "the asker was handed the close it made: {read}"
    );
}

/// The id an answer names under `key`.
fn message_of(said: &serde_json::Value, key: &str) -> String {
    said[key].as_str().expect("an id").to_string()
}
