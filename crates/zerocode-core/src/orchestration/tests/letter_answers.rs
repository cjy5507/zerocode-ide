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
pub(super) fn ask_at(bench: &mut Bench, pane: &str, to: &str, body: &str) -> String {
    bench.json_at(pane, &format!("ask --to {to} --body {body}"))["questionId"]
        .as_str()
        .expect("a question id")
        .to_string()
}

/// A root status letter, for a fixture that posts straight to the ledger.
pub(super) fn letter(from: &str, to: &str, body: &str) -> Draft {
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
/// them at the bench's clock: [`open_questions_to`] (the receiver notices)
/// and [`open_questions`] (the rings of waiting), each sorted.
pub(super) fn held_open(bench: &Bench, run_id: &str) -> (Vec<String>, Vec<String>) {
    let run = bench.ledger.run(run_id).expect("the run");
    let mut to_seat: Vec<String> =
        open_questions_to(&bench.ledger, run, &run.address(), bench.clock)
            .into_iter()
            .map(|question| question.id.clone())
            .collect();
    let mut every: Vec<String> = open_questions(&bench.ledger, run, bench.clock)
        .into_iter()
        .filter(|(_, to, _)| *to == run.address())
        .map(|(_, _, id)| id.to_string())
        .collect();
    to_seat.sort();
    every.sort();
    (to_seat, every)
}

pub(super) fn sorted(mut ids: Vec<String>) -> Vec<String> {
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
    send_at(
        &mut bench,
        lead,
        &format!("worker:{second}"),
        "답: 네, 올리세요",
    );
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
            letter(
                &seat,
                &format!("worker:{first}"),
                &format!("{coming} 답: main"),
            ),
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
    assert_eq!(
        early, coming,
        "the letter does not name the question after it"
    );

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
            run.answer_to(run.message(id).expect("the question"))
                .is_none(),
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

/// Run `argv` from the leader pane of `team`, swapped in for the bench's own
/// team and back — another window's coordinator. Answers what the verb
/// answered, a refusal included.
pub(super) fn answered_as_leader_of(bench: &mut Bench, team: &mut Team, argv: &[&str]) -> Reply {
    std::mem::swap(&mut bench.team, team);
    let planned = bench.at_argv(
        agent_teams::LEADER_PANE,
        argv.iter().map(|word| word.to_string()).collect(),
    );
    std::mem::swap(&mut bench.team, team);
    planned.reply
}

/// The same, for a verb that must be taken: answers its JSON.
pub(super) fn as_leader_of(bench: &mut Bench, team: &mut Team, argv: &[&str]) -> serde_json::Value {
    let reply = answered_as_leader_of(bench, team, argv);
    assert_eq!(reply.exit_code, 0, "{argv:?}: {}", reply.stderr);
    serde_json::from_str(&reply.stdout).expect("the verb answers JSON")
}

/// A run `team`'s leader creates, and so sits in the chair of.
pub(super) fn run_of(bench: &mut Bench, team: &mut Team, name: &str) -> String {
    as_leader_of(bench, team, &["run-create", "--name", name])["runId"]
        .as_str()
        .expect("a run id")
        .to_string()
}

/// A question `team`'s leader puts to another run's chair — filed in that
/// run under the leader's pane address, as `send --run` files it.
pub(super) fn ask_another_run(
    bench: &mut Bench,
    team: &mut Team,
    run_id: &str,
    body: &str,
) -> String {
    let chair = format!("run:{run_id}");
    as_leader_of(
        bench,
        team,
        &[
            "send", "--run", run_id, "--to", &chair, "--type", "question", "--body", body,
        ],
    )["messageId"]
        .as_str()
        .expect("a question id")
        .to_string()
}

/// A letter `team`'s leader files in `run_id`, to that run's chair: the road
/// a coordinator answers another run's question by.
pub(super) fn write_to_run(bench: &mut Bench, team: &mut Team, run_id: &str, body: &str) -> String {
    let chair = format!("run:{run_id}");
    as_leader_of(
        bench,
        team,
        &[
            "send", "--run", run_id, "--to", &chair, "--type", "status", "--body", body,
        ],
    )["messageId"]
        .as_str()
        .expect("a letter id")
        .to_string()
}

/// One cross-run case, seats as they were (A1, the asker's run): the
/// answering chair's letter in the asker's run closes the question it names.
/// The words without the id, the same words from a pane that never sat in the
/// chair asked, and the same words in a run whose chair the asker never sat
/// in all leave theirs open.
#[test]
fn a_letter_in_the_askers_run_from_the_chair_it_asked_closes_the_question() {
    let mut bench = Bench::new();
    let mut asker = Team::new("team-asker", "asker", 60);
    let mut answerer = Team::new("team-answerer", "answerer", 70);
    let mut stranger = Team::new("team-stranger", "stranger", 80);
    let asking = run_of(&mut bench, &mut asker, "asker");
    let answering = run_of(&mut bench, &mut answerer, "answerer");
    let elsewhere = run_of(&mut bench, &mut stranger, "elsewhere");

    let named = ask_another_run(&mut bench, &mut asker, &answering, "which-branch?");
    let unnamed = ask_another_run(&mut bench, &mut asker, &answering, "which-port?");
    let by_a_stranger = ask_another_run(&mut bench, &mut asker, &answering, "which-host?");
    let filed_astray = ask_another_run(&mut bench, &mut asker, &answering, "which-tag?");
    let before: Vec<String> = sorted(vec![
        named.clone(),
        unnamed.clone(),
        by_a_stranger.clone(),
        filed_astray.clone(),
    ]);
    assert_eq!(held_open(&bench, &answering), (before.clone(), before));

    write_to_run(
        &mut bench,
        &mut answerer,
        &asking,
        &format!("[{answering}] {named} 답: main"),
    );
    write_to_run(&mut bench, &mut answerer, &asking, "답: 8080");
    write_to_run(
        &mut bench,
        &mut stranger,
        &asking,
        &format!("{by_a_stranger} 답: example.test"),
    );
    write_to_run(
        &mut bench,
        &mut answerer,
        &elsewhere,
        &format!("{filed_astray} 답: v1"),
    );

    let open = sorted(vec![unnamed, by_a_stranger, filed_astray]);
    assert_eq!(
        held_open(&bench, &answering),
        (open.clone(), open),
        "the asker's run was not read, or was read too widely"
    );
}

/// The other cross-run case: the chairs moved between the question and its
/// answer (A1, seats that have sat). A window restart seats every
/// coordinator in a new pane — twice on the night of 2026-10-16 — so the
/// asker's pane is no run's chair any more, and the answers come from the old
/// answering pane before the move and from the new one after it. Each chair
/// signed one letter as its run first, which is how a run remembers who sat in
/// it.
#[test]
fn a_question_asked_before_the_chairs_moved_is_closed_by_letters_from_either_side_of_it() {
    let mut bench = Bench::new();
    let mut asker = Team::new("team-asker", "asker", 60);
    let mut answerer = Team::new("team-answerer", "answerer", 70);
    let mut asker_after = Team::new("team-asker-after", "asker-after", 80);
    let mut answerer_after = Team::new("team-answerer-after", "answerer-after", 90);
    let asking = run_of(&mut bench, &mut asker, "asker");
    let answering = run_of(&mut bench, &mut answerer, "answerer");
    for (team, run_id) in [(&mut asker, &asking), (&mut answerer, &answering)] {
        let chair = format!("run:{run_id}");
        as_leader_of(
            &mut bench,
            team,
            &[
                "send", "--to", &chair, "--type", "status", "--body", "on-duty",
            ],
        );
    }

    let answered_before = ask_another_run(&mut bench, &mut asker, &answering, "which-branch?");
    let answered_after = ask_another_run(&mut bench, &mut asker, &answering, "which-port?");
    write_to_run(
        &mut bench,
        &mut answerer,
        &asking,
        &format!("{answered_before} 답: main"),
    );

    // The restart: both chairs are taken from panes that are gone.
    let gone = format!("{}/%1", asker.id);
    as_leader_of(
        &mut bench,
        &mut asker_after,
        &[
            "run-takeover",
            "--run",
            &asking,
            "--from",
            &gone,
            "--reason",
            "restart",
        ],
    );
    let gone = format!("{}/%1", answerer.id);
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
        (vec![answered_after.clone()], vec![answered_after.clone()]),
        "the old chairs' letter stopped counting once the chairs moved"
    );

    write_to_run(
        &mut bench,
        &mut answerer_after,
        &asking,
        &format!("{answered_after} 답: 8080"),
    );
    assert_eq!(
        held_open(&bench, &answering),
        (vec![], vec![]),
        "a question asked from a pane that is no chair now stayed open"
    );
}

/// A6: the pile as one desk saw it, in small — two coordinators, five
/// questions from one to the other, three answered by root letters (two
/// filed in the asker's run, one in the answering run to the asker's pane),
/// one written back about without its id and one not at all: five open
/// before the letters, two after.
#[test]
fn two_coordinators_five_questions_three_letters_leave_two_open() {
    let mut bench = Bench::new();
    let mut asker = Team::new("team-asker", "asker", 60);
    let mut answerer = Team::new("team-answerer", "answerer", 70);
    let asking = run_of(&mut bench, &mut asker, "asker");
    let answering = run_of(&mut bench, &mut answerer, "answerer");

    let asked: Vec<String> = (1..=5)
        .map(|at| ask_another_run(&mut bench, &mut asker, &answering, &format!("q{at}?")))
        .collect();
    assert_eq!(held_open(&bench, &answering).0.len(), 5);

    write_to_run(
        &mut bench,
        &mut answerer,
        &asking,
        &format!("{} 답: 네", asked[0]),
    );
    write_to_run(
        &mut bench,
        &mut answerer,
        &asking,
        &format!("{}에 답: 아니요", asked[1]),
    );
    let asker_pane = format!("pane:{}/%1", asker.id);
    as_leader_of(
        &mut bench,
        &mut answerer,
        &[
            "send",
            "--to",
            &asker_pane,
            "--type",
            "status",
            "--body",
            &format!("{} 답: 나중에", asked[2]),
        ],
    );
    write_to_run(&mut bench, &mut answerer, &asking, "답: 넷째는 내일");

    let open = sorted(vec![asked[3].clone(), asked[4].clone()]);
    assert_eq!(
        held_open(&bench, &answering),
        (open.clone(), open),
        "five questions, three letters: two should stay open"
    );
}
