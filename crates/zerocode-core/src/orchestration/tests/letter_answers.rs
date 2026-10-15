//! t-59383: a question the seat it was asked of has written back about is
//! not counted open.
//!
//! The coordinator card's 「질문 N」 stood at 50, 13 and 26 on 2026-10-16
//! because coordinators answered with `send` — a letter that starts no thread
//! — and the count read only the thread. A root letter from the asked seat to
//! the asker, written after the question and naming its id, now closes the
//! question for every count: [`open_questions_to`], [`open_questions`] and the
//! rings of waiting read off it. The answer an `ask` reads back is untouched:
//! it is still the word in the thread ([`thread_answer`]).

use super::*;

/// One status letter from `pane`, with a body that may hold spaces.
fn send_at(bench: &mut Bench, pane: &str, to: &str, body: &str) -> String {
    let planned = bench.at_argv(
        pane,
        ["send", "--to", to, "--type", "status", "--body", body]
            .map(str::to_string)
            .to_vec(),
    );
    assert_eq!(planned.reply.exit_code, 0, "{}", planned.reply.stderr);
    serde_json::from_str::<serde_json::Value>(&planned.reply.stdout)
        .expect("send answers JSON")["messageId"]
        .as_str()
        .expect("a message id")
        .to_string()
}

/// A question `pane` puts to whoever `to` names.
fn ask_at(bench: &mut Bench, pane: &str, to: &str, body: &str) -> String {
    bench.json_at(pane, &format!("ask --to {to} --body {body}"))["questionId"]
        .as_str()
        .expect("a question id")
        .to_string()
}

/// A root status letter, for a fixture that posts straight to the ledger.
fn letter(from: &str, to: &str, body: &str) -> Draft {
    Draft {
        from: from.to_string(),
        to: to.to_string(),
        kind: MessageKind::Status,
        body: body.into(),
        subject: Text::default(),
        priority: Priority::Normal,
        payload: Text::default(),
        thread: None,
        task: None,
        dispatch: None,
    }
}

/// The questions the run's own seat is counted as owing, as both counts read
/// them: [`open_questions_to`] (the receiver notices) and [`open_questions`]
/// (the rings of waiting), each sorted.
fn held_open(bench: &Bench, run_id: &str) -> (Vec<String>, Vec<String>) {
    let run = bench.ledger.run(run_id).expect("the run");
    let mut to_seat: Vec<String> = open_questions_to(run, &run.address())
        .into_iter()
        .map(|question| question.id.clone())
        .collect();
    let mut every: Vec<String> = open_questions(run)
        .into_iter()
        .filter(|(_, to, _)| *to == run.address())
        .map(|(_, _, id)| id.to_string())
        .collect();
    to_seat.sort();
    every.sort();
    (to_seat, every)
}

fn sorted(mut ids: Vec<String>) -> Vec<String> {
    ids.sort();
    ids
}

/// The four cases of A1, and the letter that names two questions at once.
///
/// ① the asked seat writes to the asker, after the question, naming its id —
///   a Korean particle right after the id is still the id: closed;
/// ② it writes to the asker without the id, or with a longer number that is
///   another id: still open;
/// ③ another seat names the id to the asker, or the asked seat names it to
///   somebody else: still open;
/// ④ the letter naming the id went out before the question: still open.
#[test]
fn a_root_letter_from_the_asked_seat_naming_the_question_closes_it_for_the_count() {
    let mut bench = Bench::new();
    let run_id = bench.json("run-create --name question-pile")["runId"]
        .as_str()
        .expect("a run id")
        .to_string();
    let seat = format!("run:{run_id}");
    let (first, first_pane) = bench.seat("worker-start --agent claude");
    let (second, second_pane) = bench.seat("worker-start --agent codex");
    let (third, third_pane) = bench.seat("worker-start --agent claude");
    let lead = agent_teams::LEADER_PANE;

    // ① answered by a letter.
    let answered = ask_at(&mut bench, &first_pane, &seat, "which-branch?");
    send_at(
        &mut bench,
        lead,
        &format!("worker:{first}"),
        &format!("{answered}에 답: main으로 가세요"),
    );

    // ② a letter without the id, and one whose number only starts like it.
    let unnamed = ask_at(&mut bench, &second_pane, &seat, "may-i-merge?");
    send_at(&mut bench, lead, &format!("worker:{second}"), "답: 네, 올리세요");
    send_at(
        &mut bench,
        lead,
        &format!("worker:{second}"),
        &format!("{unnamed}7 일감은 따로 봅니다"),
    );

    // ③ the right words from the wrong seat, and to the wrong reader.
    let elsewhere = ask_at(&mut bench, &third_pane, &seat, "which-port?");
    send_at(
        &mut bench,
        &first_pane,
        &format!("worker:{third}"),
        &format!("{elsewhere} 답: 8080"),
    );
    send_at(
        &mut bench,
        lead,
        &format!("worker:{first}"),
        &format!("{elsewhere}은 셋째의 질문입니다"),
    );

    // ④ the letter came first and names the id the question is minted under.
    let coming = format!("m-{}", bench.ledger.next_id + 2);
    bench.clock += 1;
    bench
        .ledger
        .post(
            &run_id,
            letter(&seat, &format!("worker:{first}"), &format!("{coming} 답: main")),
            bench.clock,
        )
        .expect("the early letter");
    bench.clock += 1;
    let early = bench
        .ledger
        .post(
            &run_id,
            Draft {
                kind: MessageKind::Question,
                ..letter(&format!("worker:{first}"), &seat, "and-now?")
            },
            bench.clock,
        )
        .expect("the question after it");
    assert_eq!(early, coming, "the letter does not name the question after it");

    // One letter, two questions.
    let both_one = ask_at(&mut bench, &second_pane, &seat, "first-half?");
    let both_two = ask_at(&mut bench, &second_pane, &seat, "second-half?");
    send_at(
        &mut bench,
        lead,
        &format!("worker:{second}"),
        &format!("{both_one}·{both_two} 답: 둘 다 main"),
    );

    let open = sorted(vec![unnamed.clone(), elsewhere.clone(), early.clone()]);
    assert_eq!(
        held_open(&bench, &run_id),
        (open.clone(), open),
        "the count is not what the letters answered"
    );

    // The answer an `ask` reads back is still the word in the thread.
    let run = bench.ledger.run(&run_id).expect("the run");
    for id in [&answered, &both_one, &both_two] {
        assert!(
            run.answer_to(run.message(id).expect("the question")).is_none(),
            "a letter became the thread's answer to {id}"
        );
    }
    let resumed = bench.json_at(&first_pane, &format!("ask --resume {answered}"));
    assert_eq!(resumed["answered"], false, "{resumed}");
}

/// A5: a wait the asked seat answered by letter is no edge of a ring. The
/// first worker asks the second, the second writes back naming the
/// question, and the second's own question back closes nothing.
#[test]
fn a_question_answered_by_a_letter_closes_no_ring_of_waiting() {
    let mut bench = Bench::new();
    bench.json("run-create --name answered-by-letter");
    let (first, first_pane) = bench.seat("worker-start --agent claude");
    let (second, second_pane) = bench.seat("worker-start --agent codex");

    let asked = ask_at(
        &mut bench,
        &first_pane,
        &format!("worker:{second}"),
        "yours?",
    );
    send_at(
        &mut bench,
        &second_pane,
        &format!("worker:{first}"),
        &format!("{asked} 답: 제 것입니다"),
    );
    ask_at(
        &mut bench,
        &second_pane,
        &format!("worker:{first}"),
        "and-yours?",
    );
    assert_eq!(
        bench.json("check --peek --types deadlocked")["count"],
        0,
        "a question its seat answered by letter closed a ring"
    );
}
