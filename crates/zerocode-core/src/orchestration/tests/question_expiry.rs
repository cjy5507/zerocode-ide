//! t-59383 r2: a question nobody waits on any more is closed for every count.
//!
//! The task board's 「답할 우편」 stood at 91 on 2026-10-16 with questions
//! fifteen, thirteen and twelve days old on it: a coordinator's question rides
//! no attempt, so nothing ever closed it ([`question_closed`]). Two clocks
//! close one now, read off `now_ms` and never stored — three days with no
//! answer, and a day with the coordinator's chair it was asked from standing
//! empty since — for the seat's count ([`open_questions_to`]), the run's
//! ([`open_questions`]) and the rings of waiting read off it. A worker's
//! question rides its attempt and closes with it, as before. The audit
//! (`inbox`) still shows every one, and `reply` refuses an expired one as it
//! refuses a closed one.

use super::letter_answers::{answered_as_leader_of, ask_another_run, ask_at, letter, run_of};
use super::*;

const HOUR_MS: i64 = 3_600_000;

/// A root question from a pane that is nobody's chair, filed straight into
/// `run_id` at `at`: no attempt under it, so only the clock closes it.
fn a_strangers_question(bench: &mut Bench, run_id: &str, to: &str, at: i64) -> String {
    bench
        .ledger
        .post(
            run_id,
            Draft {
                kind: MessageKind::Question,
                ..letter("pane:fixture/%0", to, "which-way?")
            },
            at,
        )
        .expect("the question")
}

/// How many lines the ledger wrote into `question`'s thread — what it tells
/// an asker about the seat it is waiting on.
fn told(bench: &Bench, run_id: &str, question: &str) -> usize {
    bench
        .ledger
        .run(run_id)
        .expect("the run")
        .messages()
        .iter()
        .filter(|held| held.from == LEDGER_ITSELF && held.thread.as_deref() == Some(question))
        .count()
}

fn run_id_of(said: &serde_json::Value) -> String {
    said["runId"].as_str().expect("a run id").to_string()
}

/// A7, the seat's count ([`open_questions_to`]): 71 hours after it was asked,
/// a question nobody answered is still waited on, and its asker is told when
/// the chair it asked is vacated; at 73 hours it has expired, and nobody is.
#[test]
fn a_question_unanswered_for_three_days_is_waited_on_no_more() {
    let told_after = |hours: i64| {
        let mut bench = Bench::new();
        let run_id = run_id_of(&bench.json("run-create --name expiry"));
        bench.clock += 1;
        let asked_at = bench.clock;
        let asked = a_strangers_question(&mut bench, &run_id, &format!("run:{run_id}"), asked_at);
        bench.ledger.window_restarted(asked_at + hours * HOUR_MS);
        told(&bench, &run_id, &asked)
    };
    assert_eq!(
        told_after(71),
        1,
        "a question 71 hours old was not told its chair was vacated"
    );
    assert_eq!(
        told_after(73),
        0,
        "a question 73 hours old is still waited on"
    );
}

/// The `deadlocked` notices a run's chair holds after one worker asks
/// another and the other asks back `hours` later. With `on_tasks` each worker
/// carries an attempt, and its question rides it.
fn deadlocks(hours: i64, on_tasks: bool) -> serde_json::Value {
    let mut bench = Bench::new();
    bench.json("run-create --name expiry-ring");
    let summon = |bench: &mut Bench, agent: &str| {
        let line = match on_tasks {
            true => {
                let task = bench.json("task-create --spec ring")["taskId"]
                    .as_str()
                    .expect("a task id")
                    .to_string();
                format!("worker-start --agent {agent} --task {task}")
            }
            false => format!("worker-start --agent {agent}"),
        };
        bench.seat(&line)
    };
    let (first, first_pane) = summon(&mut bench, "claude");
    let (second, second_pane) = summon(&mut bench, "codex");
    let (to_first, to_second) = (format!("worker:{first}"), format!("worker:{second}"));
    ask_at(&mut bench, &first_pane, &to_second, "yours?");
    bench.clock += hours * HOUR_MS;
    ask_at(&mut bench, &second_pane, &to_first, "and-yours?");
    bench.json("check --peek --types deadlocked")["count"].clone()
}

/// A7, the run's count ([`open_questions`]) and the walk that finds a ring
/// ([`ring_closed_by`]): the first worker asks the second, and the second's
/// question back 71 hours later closes a ring — 73 hours later it closes
/// none, because the first wait has expired. Two workers whose questions
/// ride open attempts close the ring at 73 hours all the same (A11).
#[test]
fn a_wait_older_than_three_days_closes_no_ring() {
    assert_eq!(deadlocks(71, false), 1, "a 71-hour wait closed no ring");
    assert_eq!(deadlocks(73, false), 0, "a wait 73 hours old closed a ring");
    assert_eq!(
        deadlocks(73, true),
        1,
        "a wait riding an open attempt expired"
    );
}

/// A7, a ring told once ([`already_told`]): it stands while its questions are
/// waited on, and the same two seats holding each other again is news once
/// the first ring's questions have expired. The workers ask each other, and
/// again 71 (or 73) hours on: one notice at 71 hours, two at 73.
#[test]
fn a_ring_whose_questions_expired_is_news_again() {
    let again = |hours: i64| {
        let mut bench = Bench::new();
        bench.json("run-create --name expiry-ring-again");
        let (first, first_pane) = bench.seat("worker-start --agent claude");
        let (second, second_pane) = bench.seat("worker-start --agent codex");
        let (to_first, to_second) = (format!("worker:{first}"), format!("worker:{second}"));
        for round in 0..2 {
            if round > 0 {
                bench.clock += hours * HOUR_MS;
            }
            ask_at(&mut bench, &first_pane, &to_second, "yours?");
            ask_at(&mut bench, &second_pane, &to_first, "and-yours?");
        }
        bench.json("check --peek --types deadlocked")["count"].clone()
    };
    assert_eq!(again(71), 1, "a ring still standing was told twice");
    assert_eq!(again(73), 2, "a ring whose questions expired still stood");
}

/// A8, the run's own address: questions the chair asked as `run:<id>`, and
/// the chair empties — its leader exited. 23 hours later the seat it asked
/// can still answer one; 25 hours later `reply` is refused as for a closed
/// question, saying it expired. The run's address still routes — to whoever
/// sits next — so nothing but the clock stops that answer.
#[test]
fn a_question_asked_as_the_run_closes_a_day_after_its_chair_emptied() {
    let mut bench = Bench::new();
    let mut gone = Team::new("team-gone", "gone", 60);
    let asking = run_of(&mut bench, &mut gone, "asker-gone");
    let chair = format!("run:{asking}");
    let asked_seat = format!("pane:{}/%9", bench.team.id);
    bench.clock += 1;
    bench
        .ledger
        .post(&asking, letter(&asked_seat, &chair, "on-duty"), bench.clock)
        .expect("the seat it asks speaks first");
    let ask = |bench: &mut Bench, body: &str| {
        bench.clock += 1;
        bench
            .ledger
            .post(
                &asking,
                Draft {
                    kind: MessageKind::Question,
                    ..letter(&chair, &asked_seat, body)
                },
                bench.clock,
            )
            .expect("the chair's question")
    };
    let at_23 = ask(&mut bench, "which-branch?");
    let at_25 = ask(&mut bench, "which-port?");

    bench.clock += 1;
    let emptied = bench.clock;
    bench.ledger.team_dissolved("team-gone", emptied);
    let reply_at = |bench: &mut Bench, hours: i64, question: &str| {
        bench.clock = emptied + hours * HOUR_MS;
        bench
            .at_argv(
                "%9",
                [
                    "reply",
                    "--run",
                    &asking,
                    "--to-message",
                    question,
                    "--body",
                    "main",
                ]
                .map(str::to_string)
                .to_vec(),
            )
            .reply
    };
    let early = reply_at(&mut bench, 23, &at_23);
    assert_eq!(
        early.exit_code, 0,
        "23 hours after the chair emptied: {}",
        early.stderr
    );
    let late = reply_at(&mut bench, 25, &at_25);
    assert!(
        late.exit_code != 0 && late.stderr.contains("closed") && late.stderr.contains("expired"),
        "25 hours after the chair emptied: {late:?}"
    );
}

/// A8, the pane a chair sits in: another run's chair asks this one
/// (`send --run`, filed here under the asker's pane), and the asker's chair
/// empties. 25 hours later, with nothing said from that pane since, `reply`
/// is refused as for a closed question, saying it expired; a question from a
/// chair still held takes its answer. Once the emptied chair's pane speaks
/// again somebody is there, as the ledger's routing already reads it: its
/// questions, the one asked before and the one it just asked, take answers.
#[test]
fn a_question_from_a_chair_empty_for_a_day_is_closed_until_its_pane_speaks() {
    let mut bench = Bench::new();
    let mut gone = Team::new("team-gone", "gone", 60);
    let mut held = Team::new("team-held", "held", 70);
    let mut answerer = Team::new("team-answerer", "answerer", 80);
    run_of(&mut bench, &mut gone, "asker-gone");
    run_of(&mut bench, &mut held, "asker-held");
    let answering = run_of(&mut bench, &mut answerer, "answerer");
    let before = ask_another_run(&mut bench, &mut gone, &answering, "which-branch?");
    let still_held = ask_another_run(&mut bench, &mut held, &answering, "which-tag?");

    bench.clock += 1;
    let emptied = bench.clock;
    bench.ledger.team_dissolved("team-gone", emptied);
    bench.clock = emptied + 25 * HOUR_MS;
    let mut reply = |bench: &mut Bench, question: &str| {
        answered_as_leader_of(
            bench,
            &mut answerer,
            &["reply", "--to-message", question, "--body", "main"],
        )
    };
    let late = reply(&mut bench, &before);
    assert!(
        late.exit_code != 0 && late.stderr.contains("closed") && late.stderr.contains("expired"),
        "a question from a chair empty for 25 hours: {late:?}"
    );
    let taken = reply(&mut bench, &still_held);
    assert_eq!(
        taken.exit_code, 0,
        "a held chair's question: {}",
        taken.stderr
    );

    let after = ask_another_run(&mut bench, &mut gone, &answering, "which-port?");
    for question in [&before, &after] {
        let taken = reply(&mut bench, question);
        assert_eq!(
            taken.exit_code, 0,
            "{question} after the pane spoke again: {}",
            taken.stderr
        );
    }
}

/// A11, and A9's two other doors. A worker's question rides its attempt:
/// unanswered for 73 hours, it is still waited on while the attempt is open —
/// the coordinator's late answer is taken — and it closes with the attempt,
/// as it always did. A question that rides no attempt has expired by then:
/// `reply` refuses it, saying so, and the audit (`inbox`) still shows it.
#[test]
fn a_workers_question_stays_open_while_its_attempt_is_open() {
    let mut bench = Bench::new();
    let run_id = run_id_of(&bench.json("run-create --name expiry-attempt"));
    let chair = format!("run:{run_id}");
    let task = bench.json("task-create --spec work")["taskId"]
        .as_str()
        .expect("a task id")
        .to_string();
    let (worker, pane) = bench.seat(&format!("worker-start --agent claude --task {task}"));
    let answered = ask_at(&mut bench, &pane, &chair, "which-branch?");
    let left = ask_at(&mut bench, &pane, &chair, "which-port?");
    bench.clock += 1;
    let asked_at = bench.clock;
    let stranger = a_strangers_question(&mut bench, &run_id, &chair, asked_at);
    bench.clock += 73 * HOUR_MS;

    let late = bench.run(&format!("reply --to-message {answered} --body main"));
    assert_eq!(
        late.reply.exit_code, 0,
        "a worker's question on an open attempt expired: {}",
        late.reply.stderr
    );
    let expired = bench.run(&format!("reply --to-message {stranger} --body main"));
    assert!(
        expired.reply.exit_code != 0
            && expired.reply.stderr.contains("closed")
            && expired.reply.stderr.contains("expired"),
        "a question 73 hours old with no attempt under it: {:?}",
        expired.reply
    );
    let audit = bench.json("inbox --address pane:fixture/%0");
    assert!(
        audit["messages"]
            .as_array()
            .expect("the audit's rows")
            .iter()
            .any(|row| row["messageId"] == stranger.as_str()),
        "the audit lost the expired question: {audit}"
    );

    bench.json(&format!("worker-stop --worker {worker}"));
    let closed = bench.run(&format!("reply --to-message {left} --body main"));
    assert!(
        closed.reply.exit_code != 0 && closed.reply.stderr.contains("its dispatch ended"),
        "a worker's question did not close with its attempt: {:?}",
        closed.reply
    );
}
