//! t-83289: a question stays answered when the chairs move, a `reply` finds
//! the asking run when the asking pane is gone, and either side's
//! coordinator can close a question with `question-close`.
//!
//! 2026-10-29 15:0x the person: 「질문이 자동으로 처리가 안돼」. Two questions
//! stood on the inbox for six hours. m-81909 had been written back about at
//! 09:34 by the chair it was asked of — a letter into the asking run naming
//! it — and counted answered until the window restarted: that chair had
//! signed nothing as its own run, so once a new pane sat there the ledger no
//! longer knew the old one had ever sat in it ([`chair_seats`]), though its
//! own notice on another question had named it. m-81883 nobody wrote back
//! about, and nobody could: `reply` to it was refused, because the pane that
//! asked was gone and the asking run was found by its live chair only
//! (`Ledger::asker_run`), and there was no verb to close a question
//! without an answer.

use super::letter_answers::{
    answered_as_leader_of, as_leader_of, ask_another_run, held_open, run_of, sorted, write_to_run,
};
use super::*;
use crate::orchestration::project_scope::reach;

/// The first word a run's chair signs as its run — the record
/// [`chair_seats`] has always read.
fn on_duty(bench: &mut Bench, team: &mut Team, run_id: &str) {
    let chair = format!("run:{run_id}");
    as_leader_of(
        bench,
        team,
        &[
            "send", "--to", &chair, "--type", "status", "--body", "on-duty",
        ],
    );
}

/// `team`'s leader sits in the empty chair of `run_id`, as a restored
/// coordinator does after a restart.
fn sits(bench: &mut Bench, team: &mut Team, run_id: &str) {
    let said = as_leader_of(bench, team, &["run-use", run_id]);
    assert_eq!(said["seated"], true, "{said}");
}

/// ② as it happened (m-81909, m-81924): the answering chair writes back into
/// the asking run, naming the question, and signs nothing as its own run all
/// the while — every letter it wrote went to another run. The ledger did see
/// it in the chair: its turn ended with another question waiting, and the
/// ledger's notice in that question's thread names it. After the restart the
/// question it answered is still answered. A pane that writes the same notice
/// in its own voice is not the ledger, and names nobody into the chair.
#[test]
fn a_letter_from_a_chair_that_signed_nothing_as_its_run_still_answers_after_the_restart() {
    let mut bench = Bench::new();
    let mut asker = Team::new("team-asker", "asker", 60);
    let mut answerer = Team::new("team-answerer", "answerer", 70);
    let mut third = Team::new("team-third", "third", 80);
    let mut stranger = Team::new("team-stranger", "stranger", 90);
    let asking = run_of(&mut bench, &mut asker, "asker");
    let answering = run_of(&mut bench, &mut answerer, "answerer");
    run_of(&mut bench, &mut third, "third");
    run_of(&mut bench, &mut stranger, "stranger");
    on_duty(&mut bench, &mut asker, &asking);

    // m-81883: another run asks, and the ledger sees the chair asked end its
    // turn with the question waiting.
    let unanswered = ask_another_run(&mut bench, &mut third, &answering, "zo-bugs?");
    bench.clock += 1;
    let told = bench.ledger.receivers_told_turn_ended(
        (answerer.id.as_str(), agent_teams::LEADER_PANE),
        bench.clock,
        false,
        bench.clock,
    );
    assert_eq!(told, 1, "the ledger did not tell the asker who it waits on");

    // m-81909, and the letter that answered it (m-81924).
    let answered = ask_another_run(&mut bench, &mut asker, &answering, "split-v1.1.104?");
    write_to_run(
        &mut bench,
        &mut answerer,
        &asking,
        &format!("[ZO → 4275·77627] {answered}·m-1 받았고, 이대로 갑니다"),
    );

    // The ledger's words in a pane's voice, then that pane's letter naming a
    // question.
    let forged = ask_another_run(&mut bench, &mut asker, &answering, "forged?");
    let chair = format!("run:{answering}");
    let notice = serde_json::json!({
        "questionId": forged,
        "receiver": chair,
        "receiverSeat": {
            "seat": format!("{}/{}", stranger.id, agent_teams::LEADER_PANE),
            "generation": 1,
        },
        "reason": "turn_ended",
    })
    .to_string();
    write_to_run(&mut bench, &mut stranger, &answering, &notice);
    write_to_run(
        &mut bench,
        &mut stranger,
        &asking,
        &format!("{forged} 답: 네"),
    );

    let open = sorted(vec![unanswered.clone(), forged.clone()]);
    assert_eq!(
        held_open(&bench, &answering),
        (open.clone(), open.clone()),
        "while the chair sat, its letter counted"
    );

    // The restart: every chair empties, and new panes sit in the two.
    bench.clock += 1;
    bench.ledger.window_restarted(bench.clock);
    let mut asker_after = Team::new("team-asker-after", "asker-after", 100);
    let mut answerer_after = Team::new("team-answerer-after", "answerer-after", 110);
    sits(&mut bench, &mut asker_after, &asking);
    sits(&mut bench, &mut answerer_after, &answering);

    assert_eq!(
        held_open(&bench, &answering),
        (open.clone(), open),
        "the question the old chair answered opened again once the chair moved"
    );
}

/// ②, the other road a chair changes hands by: a holder taken over
/// (`run-takeover`) that had signed nothing as its run. The ledger's own
/// receipt to it names the chair it held; its letter still answers.
#[test]
fn a_letter_from_a_chair_since_taken_over_still_answers() {
    let mut bench = Bench::new();
    let mut asker = Team::new("team-asker", "asker", 60);
    let mut answerer = Team::new("team-answerer", "answerer", 70);
    let mut answerer_after = Team::new("team-answerer-after", "answerer-after", 80);
    let asking = run_of(&mut bench, &mut asker, "asker");
    let answering = run_of(&mut bench, &mut answerer, "answerer");
    on_duty(&mut bench, &mut asker, &asking);

    let answered = ask_another_run(&mut bench, &mut asker, &answering, "which-branch?");
    write_to_run(
        &mut bench,
        &mut answerer,
        &asking,
        &format!("{answered} 답: main"),
    );
    assert_eq!(held_open(&bench, &answering), (vec![], vec![]));

    let gone = format!("{}/{}", answerer.id, agent_teams::LEADER_PANE);
    as_leader_of(
        &mut bench,
        &mut answerer_after,
        &[
            "run-takeover",
            "--run",
            &answering,
            "--from",
            &gone,
            "--reason",
            "restart",
        ],
    );
    assert_eq!(
        held_open(&bench, &answering),
        (vec![], vec![]),
        "the letter of a chair taken over stopped counting"
    );
}

/// ① (m-81883's `reply`): the pane that asked is gone and a new pane sits in
/// the asking run's chair. The window's door checks the reader the answer
/// will reach — that chair, in the asking run — not the gone pane it cannot
/// place; and the answer is delivered to the asking run, where its chair
/// reads it.
#[test]
fn a_reply_to_a_question_whose_asking_pane_is_gone_reaches_the_asking_runs_chair() {
    let mut bench = Bench::new();
    let mut asker = Team::new("team-asker", "asker", 60);
    let mut answerer = Team::new("team-answerer", "answerer", 70);
    let mut asker_after = Team::new("team-asker-after", "asker-after", 80);
    let asking = run_of(&mut bench, &mut asker, "asker");
    let answering = run_of(&mut bench, &mut answerer, "answerer");
    on_duty(&mut bench, &mut asker, &asking);
    let question = ask_another_run(&mut bench, &mut asker, &answering, "zo-bugs?");

    bench.clock += 1;
    bench.ledger.team_dissolved(&asker.id, bench.clock);
    sits(&mut bench, &mut asker_after, &asking);

    let argv = words(&format!(
        "reply --to-message {question} --body main --retry-request gone-asker"
    ));
    let door = reach(
        &bench.ledger,
        &argv,
        &answerer.id,
        agent_teams::LEADER_PANE,
        Some(&bench_actor(&answerer.id, agent_teams::LEADER_PANE)),
        "",
    );
    assert_eq!(
        door.reply_run.as_ref().map(|(id, _)| id.as_str()),
        Some(asking.as_str()),
        "the asking run was not found from a pane that sat in its chair"
    );
    assert_eq!(
        door.recipient_seats,
        vec![format!("{}/{}", asker_after.id, agent_teams::LEADER_PANE)],
        "the door checks a pane that is gone instead of the chair that reads the answer"
    );

    let said = as_leader_of(
        &mut bench,
        &mut answerer,
        &["reply", "--to-message", &question, "--body", "main"],
    );
    assert_eq!(said["deliveredTo"]["runId"], asking.as_str(), "{said}");
    let copy = said["deliveredTo"]["messageId"]
        .as_str()
        .expect("the copy's id");
    let held = bench
        .ledger
        .run(&asking)
        .and_then(|run| run.message(copy))
        .expect("the copy in the asking run");
    assert_eq!(held.to, format!("run:{asking}"));
    assert_eq!(held.thread.as_deref(), Some(question.as_str()));
    assert_eq!(held_open(&bench, &answering), (vec![], vec![]));
}

/// ③ and ④: `question-close`. The chair asked closes one question without an
/// answer, and a second close is the first one's answer, writing nothing. The
/// close is a line in the question's thread in the ledger's voice — never its
/// answer — and the asking run is told. The asking chair closes its own
/// question, filed in the other run and found by its id; after a restart the
/// asking run's new chair closes one its old pane asked. A chair of neither
/// side is refused, and so is a close with no reason. Every count leaves the
/// closed questions out, and a closed question takes no answer.
#[test]
fn either_sides_chair_closes_a_question_once_and_it_leaves_every_count() {
    let mut bench = Bench::new();
    let mut asker = Team::new("team-asker", "asker", 60);
    let mut answerer = Team::new("team-answerer", "answerer", 70);
    let mut stranger = Team::new("team-stranger", "stranger", 80);
    let mut asker_after = Team::new("team-asker-after", "asker-after", 90);
    let asking = run_of(&mut bench, &mut asker, "asker");
    let answering = run_of(&mut bench, &mut answerer, "answerer");
    run_of(&mut bench, &mut stranger, "elsewhere");
    on_duty(&mut bench, &mut asker, &asking);
    let receiving_side = ask_another_run(&mut bench, &mut asker, &answering, "closed-by-them?");
    let sending_side = ask_another_run(&mut bench, &mut asker, &answering, "closed-by-us?");
    let after_restart = ask_another_run(&mut bench, &mut asker, &answering, "after-restart?");
    let kept = ask_another_run(&mut bench, &mut asker, &answering, "kept?");

    // ③ the receiving side.
    let close = |bench: &mut Bench, team: &mut Team, question: &str| {
        as_leader_of(
            bench,
            team,
            &[
                "question-close",
                "--message",
                question,
                "--reason",
                "이미 끝난 일 — 답 없이 닫음",
            ],
        )
    };
    let closed = close(&mut bench, &mut answerer, &receiving_side);
    assert_eq!(closed["questionId"], receiving_side.as_str(), "{closed}");
    assert_eq!(closed["runId"], answering.as_str(), "{closed}");
    assert_eq!(closed["closed"], true, "{closed}");
    assert_eq!(closed["already"], false, "{closed}");
    assert_eq!(closed["closedBy"], format!("run:{answering}"), "{closed}");
    assert_eq!(closed["deliveredTo"]["runId"], asking.as_str(), "{closed}");
    let close_id = closed["closeId"]
        .as_str()
        .expect("the close's id")
        .to_string();

    let written = bench.ledger.run(&answering).expect("run").messages().len();
    let again = close(&mut bench, &mut answerer, &receiving_side);
    assert_eq!(again["already"], true, "{again}");
    assert_eq!(again["closeId"], close_id.as_str(), "{again}");
    assert_eq!(
        bench.ledger.run(&answering).expect("run").messages().len(),
        written,
        "a second close wrote again"
    );
    let run = bench.ledger.run(&answering).expect("run");
    let row = run.message(&close_id).expect("the close row");
    assert_eq!(row.from, LEDGER_ITSELF);
    assert_eq!(row.kind, MessageKind::Status);
    assert_eq!(row.thread.as_deref(), Some(receiving_side.as_str()));
    let question = run.message(&receiving_side).expect("the question");
    assert!(
        run.answer_to(question).is_none(),
        "the close became the answer"
    );

    // ④ the sending side, from its own run.
    let by_asker = close(&mut bench, &mut asker, &sending_side);
    assert_eq!(by_asker["runId"], answering.as_str(), "{by_asker}");
    assert_eq!(by_asker["closedBy"], format!("run:{asking}"), "{by_asker}");

    // ...and from the asking run's next chair, for its old pane's question.
    bench.clock += 1;
    bench.ledger.team_dissolved(&asker.id, bench.clock);
    sits(&mut bench, &mut asker_after, &asking);
    let by_next_chair = close(&mut bench, &mut asker_after, &after_restart);
    assert_eq!(by_next_chair["closed"], true, "{by_next_chair}");

    // Neither side, and no reason.
    let refused = answered_as_leader_of(
        &mut bench,
        &mut stranger,
        &[
            "question-close",
            "--message",
            &kept,
            "--reason",
            "남의 질문",
        ],
    );
    assert!(
        refused.exit_code != 0 && refused.stderr.contains(&kept),
        "a chair of neither side closed a question: {refused:?}"
    );
    let reasonless = answered_as_leader_of(
        &mut bench,
        &mut answerer,
        &["question-close", "--message", &kept],
    );
    assert!(
        reasonless.exit_code != 0 && reasonless.stderr.contains("--reason"),
        "a close with no reason: {reasonless:?}"
    );

    assert_eq!(
        held_open(&bench, &answering),
        (vec![kept.clone()], vec![kept]),
        "a closed question is still counted open"
    );
    let late = answered_as_leader_of(
        &mut bench,
        &mut answerer,
        &[
            "reply",
            "--to-message",
            &receiving_side,
            "--body",
            "늦은 답",
        ],
    );
    assert!(
        late.exit_code != 0 && late.stderr.contains("closed"),
        "a closed question took an answer: {late:?}"
    );
}
