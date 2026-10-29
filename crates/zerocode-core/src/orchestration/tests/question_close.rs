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
            "send", "--run", run_id, "--to", &chair, "--type", "status", "--body", "on-duty",
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
    // The count Jev reads of the coordinator's mail is the same count.
    let run = bench.ledger.run(&answering).expect("the run");
    let receipts = Vec::new();
    assert_eq!(
        crate::mail_triage::Mailroom::of_run(run, &receipts).open_questions(
            &bench.ledger,
            run,
            bench.clock
        ),
        2,
        "Jev's count of open questions reads another rule"
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

/// P2-1 (r2 review): a pane that sat in two runs' chairs asked from each in
/// turn. Its first question was asked from the first run's chair, its later
/// ones from the second's — after it sat there and signed as that run. The
/// answer and the close go to the run it asked FROM, read off the times of
/// its chairs: while it still holds both, and after a restart has sat new
/// panes in both. Before this the first run it had ever sat in took them
/// all — its new chair read answers meant for the other, and was the one
/// let close them.
#[test]
fn a_question_from_a_pane_that_sat_in_two_chairs_goes_to_the_run_it_asked_from() {
    let mut bench = Bench::new();
    let mut asker = Team::new("team-asker", "asker", 60);
    let mut answerer = Team::new("team-answerer", "answerer", 70);
    let mut first_after = Team::new("team-first-after", "first-after", 80);
    let mut second_after = Team::new("team-second-after", "second-after", 90);
    let first = run_of(&mut bench, &mut asker, "first");
    on_duty(&mut bench, &mut asker, &first);
    let answering = run_of(&mut bench, &mut answerer, "answerer");
    let asked_from_first = ask_another_run(&mut bench, &mut asker, &answering, "from-first?");

    // The same pane sits in a second run's chair and works there.
    let second = run_of(&mut bench, &mut asker, "second");
    on_duty(&mut bench, &mut asker, &second);
    let asked_from_second = ask_another_run(&mut bench, &mut asker, &answering, "from-second?");
    let closed_from_second = ask_another_run(&mut bench, &mut asker, &answering, "close-me?");

    let reply = |bench: &mut Bench, answerer: &mut Team, question: &str| {
        as_leader_of(
            bench,
            answerer,
            &["reply", "--to-message", question, "--body", "main"],
        )
    };
    // While the pane holds both chairs.
    let live = reply(&mut bench, &mut answerer, &asked_from_second);
    assert_eq!(
        live["deliveredTo"]["runId"],
        second.as_str(),
        "the answer went to the first chair the pane sat in, not the one it asked from: {live}"
    );

    // The restart: new panes sit in both chairs.
    bench.clock += 1;
    bench.ledger.window_restarted(bench.clock);
    sits(&mut bench, &mut first_after, &first);
    sits(&mut bench, &mut second_after, &second);

    let early = reply(&mut bench, &mut answerer, &asked_from_first);
    assert_eq!(
        early["deliveredTo"]["runId"],
        first.as_str(),
        "the question asked before the pane sat in the second chair: {early}"
    );
    let argv = words(&format!(
        "reply --to-message {asked_from_first} --body main --retry-request door"
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
        Some(first.as_str()),
        "the window's door places another run than the ledger delivers to"
    );

    let close = |bench: &mut Bench, team: &mut Team| {
        answered_as_leader_of(
            bench,
            team,
            &[
                "question-close",
                "--message",
                &closed_from_second,
                "--reason",
                "다른 런에서 끝남",
            ],
        )
    };
    let wrong_side = close(&mut bench, &mut first_after);
    assert!(
        wrong_side.exit_code != 0,
        "the chair of a run the question was not asked from closed it: {wrong_side:?}"
    );
    let right_side = close(&mut bench, &mut second_after);
    assert_eq!(right_side.exit_code, 0, "{right_side:?}");
    let right_side: serde_json::Value = serde_json::from_str(&right_side.stdout).expect("JSON");
    assert_eq!(
        right_side["closedBy"],
        format!("run:{second}"),
        "{right_side}"
    );
    // The asking side closes it where it was filed, and tells nobody else.
    assert_eq!(right_side["runId"], answering.as_str(), "{right_side}");
    assert!(right_side["deliveredTo"].is_null(), "{right_side}");
}

/// P3-3 (r2 review): the roads `question-close` was asked to take that the
/// first round read in the code and never pinned. A question answered
/// already closes as `already` with its answer named and nothing written;
/// an id nobody minted, and a letter that is no question, are refused by
/// name; `--run` names the run that holds the question, and a run that
/// does not hold it finds nothing.
#[test]
fn a_close_of_an_answered_question_an_unknown_id_a_letter_or_another_run() {
    let mut bench = Bench::new();
    let mut asker = Team::new("team-asker", "asker", 60);
    let mut answerer = Team::new("team-answerer", "answerer", 70);
    let asking = run_of(&mut bench, &mut asker, "asker");
    on_duty(&mut bench, &mut asker, &asking);
    let answering = run_of(&mut bench, &mut answerer, "answerer");
    let answered = ask_another_run(&mut bench, &mut asker, &answering, "answered?");
    let open = ask_another_run(&mut bench, &mut asker, &answering, "open?");
    let reply = as_leader_of(
        &mut bench,
        &mut answerer,
        &["reply", "--to-message", &answered, "--body", "main"],
    );
    let answer = reply["messageId"].as_str().expect("the answer's id");
    let letter = write_to_run(&mut bench, &mut asker, &answering, "그냥 편지");

    let close = |bench: &mut Bench, team: &mut Team, extra: &[&str]| {
        let mut argv = vec!["question-close", "--reason", "끝"];
        argv.extend_from_slice(extra);
        answered_as_leader_of(bench, team, &argv)
    };
    let written = |bench: &Bench| {
        bench
            .ledger
            .runs()
            .iter()
            .map(|run| run.messages().len())
            .sum::<usize>()
    };

    let before = written(&bench);
    let standing = close(&mut bench, &mut answerer, &["--message", &answered]);
    assert_eq!(standing.exit_code, 0, "{standing:?}");
    let standing: serde_json::Value = serde_json::from_str(&standing.stdout).expect("JSON");
    assert_eq!(standing["closed"], true, "{standing}");
    assert_eq!(standing["already"], true, "{standing}");
    assert_eq!(standing["answerId"], answer, "{standing}");
    assert!(standing["closeId"].is_null(), "{standing}");
    assert_eq!(
        written(&bench),
        before,
        "closing an answered question wrote"
    );

    let unknown = close(&mut bench, &mut answerer, &["--message", "m-999999"]);
    assert!(
        unknown.exit_code != 0 && unknown.stderr.contains("unknown message: m-999999"),
        "{unknown:?}"
    );
    let not_a_question = close(&mut bench, &mut answerer, &["--message", &letter]);
    assert!(
        not_a_question.exit_code != 0 && not_a_question.stderr.contains("is not a question"),
        "{not_a_question:?}"
    );

    // `--run` names the run the question was filed in, from the asking side…
    let named = close(
        &mut bench,
        &mut asker,
        &["--message", &open, "--run", &answering],
    );
    assert_eq!(named.exit_code, 0, "{named:?}");
    let named: serde_json::Value = serde_json::from_str(&named.stdout).expect("JSON");
    assert_eq!(named["runId"], answering.as_str(), "{named}");
    assert_eq!(named["already"], false, "{named}");
    // …and a run that does not hold it holds nothing to close.
    let elsewhere = close(
        &mut bench,
        &mut asker,
        &["--message", &open, "--run", &asking],
    );
    assert!(
        elsewhere.exit_code != 0 && elsewhere.stderr.contains("unknown message"),
        "{elsewhere:?}"
    );
}

/// P3-2 and P3-3 (r2 review): a worker sleeping on `ask` wakes when its
/// coordinator closes the question, and is told who closed it and why —
/// the same words the close left in the thread — by the woken look and by
/// the deadline alike.
#[test]
fn a_worker_waiting_on_ask_wakes_on_a_close_and_is_told_who_and_why() {
    let mut bench = Bench::new();
    let run_id = bench.json("run-create --name closes")["runId"]
        .as_str()
        .expect("a run id")
        .to_string();
    let (_worker, pane) = bench.seat("worker-start --agent codex");
    let (question, waiting) = asked_of(&mut bench, &pane, &format!("run:{run_id}"));
    assert!(look_again(&mut bench.ledger, &waiting).is_none());

    let closed = bench.at_argv(
        agent_teams::LEADER_PANE,
        vec![
            "question-close".to_string(),
            "--message".to_string(),
            question.clone(),
            "--reason".to_string(),
            "브리핑에 이미 답이 있음".to_string(),
        ],
    );
    assert_eq!(closed.reply.exit_code, 0, "{}", closed.reply.stderr);
    let closed: serde_json::Value = serde_json::from_str(&closed.reply.stdout).expect("JSON");
    let close_id = closed["closeId"].as_str().expect("the close's id");

    let woken = look_again(&mut bench.ledger, &waiting).expect("the close wakes the ask");
    let deadline = deadline_look(&bench.ledger, &waiting).expect("the deadline answers");
    for (road, settled) in [("woken look", woken), ("deadline", deadline)] {
        let said: serde_json::Value = serde_json::from_str(&settled.reply.stdout).expect("JSON");
        assert_eq!(said["answered"], false, "{road}: {said}");
        assert_eq!(said["closed"], true, "{road}: {said}");
        assert_eq!(said["closeId"], close_id, "{road}: {said}");
        assert_eq!(said["closedBy"], format!("run:{run_id}"), "{road}: {said}");
        assert_eq!(said["note"], "브리핑에 이미 답이 있음", "{road}: {said}");
        assert!(said["timedOut"].is_null(), "{road}: {said}");
    }
}

/// P3-3 and P3-4 (r2 review): the asking pane is gone and nobody sits in
/// the asking run's chair yet. A `reply` reaches nobody and is refused — and
/// the refusal names both ways out: `reply` again once the run's next
/// coordinator sits, or `question-close`. The close works from the side
/// asked, with nobody in the asking chair to tell.
#[test]
fn a_reply_while_the_asking_chair_is_empty_says_the_ways_out_and_the_close_works() {
    let mut bench = Bench::new();
    let mut asker = Team::new("team-asker", "asker", 60);
    let mut answerer = Team::new("team-answerer", "answerer", 70);
    let mut answerer_after = Team::new("team-answerer-after", "answerer-after", 80);
    let asking = run_of(&mut bench, &mut asker, "asker");
    on_duty(&mut bench, &mut asker, &asking);
    let answering = run_of(&mut bench, &mut answerer, "answerer");
    let question = ask_another_run(&mut bench, &mut asker, &answering, "anyone?");

    bench.clock += 1;
    bench.ledger.window_restarted(bench.clock);
    sits(&mut bench, &mut answerer_after, &answering);

    let refused = answered_as_leader_of(
        &mut bench,
        &mut answerer_after,
        &["reply", "--to-message", &question, "--body", "main"],
    );
    assert_ne!(refused.exit_code, 0, "{refused:?}");
    for way_out in [
        "unroutable address",
        &format!("run:{asking}"),
        "reply",
        &format!("question-close --message {question}"),
    ] {
        assert!(
            refused.stderr.contains(way_out),
            "the refusal does not say {way_out:?}: {}",
            refused.stderr
        );
    }

    let closed = as_leader_of(
        &mut bench,
        &mut answerer_after,
        &[
            "question-close",
            "--message",
            &question,
            "--reason",
            "묻는 쪽 자리가 비어 있음",
        ],
    );
    assert_eq!(closed["closed"], true, "{closed}");
    assert_eq!(closed["closedBy"], format!("run:{answering}"), "{closed}");
    assert_eq!(held_open(&bench, &answering), (vec![], vec![]));
}
