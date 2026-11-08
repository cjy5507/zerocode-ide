//! t-97078: a letter reaches the `check` of the run its reader sits in, and a
//! verification of a source that is the tested head rebased is taken.
//!
//! 2026-11-05 and 11-06, as it happened: letters to a coordinator's pane
//! (m-96346, m-96373) stood in the sending run's inbox and the pane's own
//! run's `check` never showed them; a `reply` to a status letter (m-96401,
//! m-96470) stayed in the answering run; `send --to run:<another run>` was
//! 「unroutable」 (m-96541); and t-95969's verification was refused because
//! the worker had rebased the commit it tested onto `main` before handing it
//! in, which the coordinator then proved the same change by hand.

use super::letter_answers::{
    answered_as_leader_of, as_leader_of, ask_another_run, run_of, write_to_run,
};
use super::*;
use crate::orchestration::coordinator_take::{CommitSeen, CommitsSeen, PatchSeen, REBASED_KEY};
use crate::orchestration::evidence::ReceiptFiles;
use crate::orchestration::project_scope::reach;

/// Every body a `check` answered.
fn bodies(read: &serde_json::Value) -> Vec<String> {
    read["messages"]
        .as_array()
        .map(|messages| {
            messages
                .iter()
                .filter_map(|message| message["body"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// What `team`'s leader reads with a plain `check` in the run it is bound to,
/// acknowledged — the next reader of that chair is handed only what came after.
fn read_by(bench: &mut Bench, team: &mut Team) -> Vec<String> {
    let read = as_leader_of(bench, team, &["check"]);
    if let Some(delivery) = read["deliveryId"].as_str() {
        as_leader_of(bench, team, &["check", "--ack", delivery]);
    }
    bodies(&read)
}

/// `team`'s leader pane, as an address.
fn pane_of(team: &Team) -> String {
    format!("pane:{}/{}", team.id, agent_teams::LEADER_PANE)
}

/// (a) A letter to a pane is filed where it was sent, and a copy where the
/// pane reads: the `check` of the run whose chair it sits in.
#[test]
fn a_letter_to_a_pane_reaches_the_check_of_the_run_that_pane_sits_in() {
    let mut bench = Bench::new();
    let mut here = Team::new("team-here", "here", 60);
    let mut there = Team::new("team-there", "there", 70);
    let home = run_of(&mut bench, &mut here, "home");
    let away = run_of(&mut bench, &mut there, "away");
    // The other run's coordinator writes into this run, so its pane is a
    // reader here — the shape 77627 wrote to 94975's pane from.
    write_to_run(&mut bench, &mut there, &home, "hello-from-away");

    let sent = as_leader_of(
        &mut bench,
        &mut here,
        &[
            "send",
            "--to",
            &pane_of(&there),
            "--type",
            "status",
            "--body",
            "over-to-you",
        ],
    );
    assert_eq!(sent["deliveredTo"]["runId"], away.as_str(), "{sent}");
    assert!(
        read_by(&mut bench, &mut there).contains(&"over-to-you".to_string()),
        "the pane's own run's check did not show the letter to its pane"
    );
}

/// (b) A reply to any letter another run's coordinator wrote — not only a
/// question — reaches the run that coordinator reads.
#[test]
fn a_reply_to_a_status_letter_reaches_the_run_of_the_pane_that_wrote_it() {
    let mut bench = Bench::new();
    let mut here = Team::new("team-here", "here", 60);
    let mut there = Team::new("team-there", "there", 70);
    let home = run_of(&mut bench, &mut here, "home");
    let away = run_of(&mut bench, &mut there, "away");
    let letter = write_to_run(&mut bench, &mut there, &home, "status-from-away");

    let answered = as_leader_of(
        &mut bench,
        &mut here,
        &["reply", "--to-message", &letter, "--body", "noted"],
    );
    assert_eq!(
        answered["deliveredTo"]["runId"],
        away.as_str(),
        "{answered}"
    );
    assert!(
        read_by(&mut bench, &mut there).contains(&"noted".to_string()),
        "the writer's run did not read the reply"
    );
}

/// r2 (review t-97475 P3-1): the copy of a reply to a status letter goes to
/// the run its writer reads now. The writer wrote from one run's chair; a
/// restart emptied that chair, and the same pane went on in a new run. The
/// reply reaches the new run — not the empty chair it wrote from, where a
/// copy is refused and took the reply down with it — on the window's half
/// and in the ledger alike.
#[test]
fn a_reply_to_a_status_letter_goes_to_the_run_its_writer_sits_in_now() {
    let mut bench = Bench::new();
    let mut here = Team::new("team-here", "here", 60);
    let mut there = Team::new("team-there", "there", 70);
    let mut here_after = Team::new("team-here-after", "here-after", 80);
    let home = run_of(&mut bench, &mut here, "home");
    let away = run_of(&mut bench, &mut there, "away");
    let letter = write_to_run(&mut bench, &mut there, &home, "status-from-away");

    bench.clock += 1;
    bench.ledger.window_restarted(bench.clock);
    let seated = as_leader_of(&mut bench, &mut here_after, &["run-use", &home]);
    assert_eq!(seated["seated"], true, "{seated}");
    let later = run_of(&mut bench, &mut there, "later");

    let argv = words(&format!("reply --to-message {letter} --body noted"));
    let door = reach(
        &bench.ledger,
        &argv,
        &here_after.id,
        agent_teams::LEADER_PANE,
        Some(&bench_actor(&here_after.id, agent_teams::LEADER_PANE)),
        "",
    );
    assert_eq!(
        door.carried_run.as_ref().map(|(id, _)| id.as_str()),
        Some(later.as_str()),
        "carried to {away}, whose chair nobody sits in"
    );

    let answered = answered_as_leader_of(
        &mut bench,
        &mut here_after,
        &["reply", "--to-message", &letter, "--body", "noted"],
    );
    assert_eq!(answered.exit_code, 0, "{answered:?}");
    let said: serde_json::Value = serde_json::from_str(&answered.stdout).expect("JSON");
    assert_eq!(said["deliveredTo"]["runId"], later.as_str(), "{said}");
    assert!(
        read_by(&mut bench, &mut there).contains(&"noted".to_string()),
        "the writer's run now did not read the reply"
    );
}

/// r3 (review t-97548 P2): the writer sat in an older run's chair before the
/// one it wrote from, and another pane sits in that older chair now. A restart
/// empties the chair it wrote from and the writer goes on in a new run. The
/// copy of the reply goes to the run the writer sits in now — never to the
/// older run, where another pane would read it, and whose project could
/// refuse the reply whole at the window's door.
#[test]
fn a_reply_to_a_status_letter_skips_an_older_chair_another_pane_sits_in_now() {
    let mut bench = Bench::new();
    let mut here = Team::new("team-here", "here", 60);
    let mut there = Team::new("team-there", "there", 70);
    let mut here_after = Team::new("team-here-after", "here-after", 80);
    let mut first_after = Team::new("team-first-after", "first-after", 90);
    let home = run_of(&mut bench, &mut here, "home");
    let first = run_of(&mut bench, &mut there, "first");
    // The writer signs as the first run's chair, then moves on.
    write_to_run(&mut bench, &mut there, &first, "on-duty-first");
    let away = run_of(&mut bench, &mut there, "away");
    write_to_run(&mut bench, &mut there, &away, "on-duty-away");
    let letter = write_to_run(&mut bench, &mut there, &home, "status-from-away");

    bench.clock += 1;
    bench.ledger.window_restarted(bench.clock);
    for (team, run) in [(&mut here_after, &home), (&mut first_after, &first)] {
        let seated = as_leader_of(&mut bench, team, &["run-use", run]);
        assert_eq!(seated["seated"], true, "{seated}");
    }
    let later = run_of(&mut bench, &mut there, "later");

    let argv = words(&format!("reply --to-message {letter} --body noted"));
    let door = reach(
        &bench.ledger,
        &argv,
        &here_after.id,
        agent_teams::LEADER_PANE,
        Some(&bench_actor(&here_after.id, agent_teams::LEADER_PANE)),
        "",
    );
    assert_eq!(
        door.carried_run.as_ref().map(|(id, _)| id.as_str()),
        Some(later.as_str()),
        "carried to {first}, whose chair another pane sits in now"
    );

    let answered = answered_as_leader_of(
        &mut bench,
        &mut here_after,
        &["reply", "--to-message", &letter, "--body", "noted"],
    );
    assert_eq!(answered.exit_code, 0, "{answered:?}");
    let said: serde_json::Value = serde_json::from_str(&answered.stdout).expect("JSON");
    assert_eq!(said["deliveredTo"]["runId"], later.as_str(), "{said}");
    assert!(
        !read_by(&mut bench, &mut first_after).contains(&"noted".to_string()),
        "the pane in the older chair read a reply meant for the writer"
    );
    assert!(
        read_by(&mut bench, &mut there).contains(&"noted".to_string()),
        "the writer's run now did not read the reply"
    );
}

/// (c) Another run's `run:` address is that run's chair: filed there and read
/// by whoever sits in it, a new pane after a restart included. While nobody
/// sits there the sender is told at once, and nothing is filed.
#[test]
fn another_runs_address_reaches_its_chair_and_an_empty_chair_is_refused_at_once() {
    let mut bench = Bench::new();
    let mut here = Team::new("team-here", "here", 60);
    let mut there = Team::new("team-there", "there", 70);
    let mut after = Team::new("team-after", "after", 80);
    run_of(&mut bench, &mut here, "home");
    let away = run_of(&mut bench, &mut there, "away");
    let chair = format!("run:{away}");
    let mut to_chair = |bench: &mut Bench, body: &str| {
        answered_as_leader_of(
            bench,
            &mut here,
            &["send", "--to", &chair, "--type", "status", "--body", body],
        )
    };

    let sent = to_chair(&mut bench, "to-the-chair");
    assert_eq!(sent.exit_code, 0, "{}", sent.stderr);
    let sent: serde_json::Value = serde_json::from_str(&sent.stdout).expect("JSON");
    assert_eq!(sent["runId"], away.as_str(), "{sent}");
    assert!(read_by(&mut bench, &mut there).contains(&"to-the-chair".to_string()));

    bench.clock += 1;
    bench.ledger.window_restarted(bench.clock);
    let filed_before = bench.ledger.run(&away).expect("the run").messages().len();
    let refused = to_chair(&mut bench, "nobody-home");
    assert_ne!(refused.exit_code, 0, "{refused:?}");
    assert!(
        refused.stderr.contains(&format!("nobody reads {chair}")),
        "{}",
        refused.stderr
    );
    assert_eq!(
        bench.ledger.run(&away).expect("the run").messages().len(),
        filed_before,
        "a letter to an empty chair was filed"
    );

    let seated = as_leader_of(&mut bench, &mut after, &["run-use", &away]);
    assert_eq!(seated["seated"], true, "{seated}");
    let sent = to_chair(&mut bench, "after-the-restart");
    assert_eq!(sent.exit_code, 0, "{}", sent.stderr);
    assert!(
        read_by(&mut bench, &mut after).contains(&"after-the-restart".to_string()),
        "the chair's new pane did not read the letter"
    );
}

/// 2026-11-08 22:50: run-94975's coordinator wrote into run-77627 with
/// `--run`, after the person had closed 77627's pane. The letter went out
/// signed `run:run-77627` — the empty chair's own voice — and the ledger's
/// "filed, not pointed" word went back to that empty chair, so the writer never
/// heard it. A vacated chair speaks and reads only from the pane that held it,
/// and the writer is answered where it reads.
#[test]
fn a_letter_into_an_empty_chair_is_signed_by_its_pane_and_heard_back_where_it_reads() {
    let mut bench = Bench::new();
    let mut here = Team::new("team-here", "here", 60);
    let mut there = Team::new("team-there", "there", 70);
    let mut stranger = Team::new("team-stranger", "stranger", 80);
    let home = run_of(&mut bench, &mut here, "home");
    let away = run_of(&mut bench, &mut there, "away");
    bench.clock += 1;
    bench.ledger.window_restarted(bench.clock);
    let seated = as_leader_of(&mut bench, &mut here, &["run-use", &home]);
    assert_eq!(seated["seated"], true, "{seated}");

    let letter = write_to_run(&mut bench, &mut here, &away, "into-the-empty-chair");
    let filed = bench
        .ledger
        .run(&away)
        .and_then(|run| run.message(&letter))
        .expect("the letter is filed in the run it names");
    assert_eq!(
        filed.from,
        pane_of(&here),
        "a letter into an empty chair spoke in that chair's voice"
    );
    assert_eq!(
        bench.ledger.writer_inbox(&away, &letter),
        Some((home.clone(), format!("run:{home}"))),
        "the writer is not answered where it reads"
    );

    // Nor does a stranger naming the run read the empty chair's inbox.
    write_to_run(&mut bench, &mut stranger, &away, "for-the-chair-only");
    let read = as_leader_of(&mut bench, &mut here, &["check", "--run", &away]);
    assert!(
        !bodies(&read).contains(&"for-the-chair-only".to_string()),
        "a pane naming the run read its empty chair's inbox: {read}"
    );
    // The run's own coordinator, back in a new pane before its chair is sat
    // again, is told where its mail waits and the verb that sits the chair.
    let mut there_again = Team::new("team-there-again", "there-again", 75);
    bench.actor = Some(bench_actor(&there.id, agent_teams::LEADER_PANE));
    let read = as_leader_of(&mut bench, &mut there_again, &["check"]);
    assert!(
        read["seatElsewhere"]["sit"]
            .as_str()
            .is_some_and(|sit| sit.starts_with(&format!("run-use {away}"))),
        "a bound pane reading its own inbox was not told the chair is empty: {read}"
    );
    let seated = as_leader_of(&mut bench, &mut there_again, &["run-use", &away]);
    assert_eq!(seated["seated"], true, "{seated}");
    assert!(
        read_by(&mut bench, &mut there_again).contains(&"for-the-chair-only".to_string()),
        "the chair's own coordinator, back in its seat, did not read its mail"
    );
    bench.actor = None;
}

/// The id of the letter `team`'s leader wrote last in `run`.
fn last_letter_from(bench: &Bench, run: &str, team: &Team) -> String {
    bench
        .ledger
        .run(run)
        .and_then(|run| {
            run.messages()
                .iter()
                .rev()
                .find(|message| message.from == pane_of(team))
        })
        .map(|message| message.id.clone())
        .expect("a letter from the pane")
}

/// 2026-11-08: a coordinator and the release-lane pane — a pane no run
/// seats — wrote to each other's pane addresses, and the window never told
/// either side a letter was there: its pointer says `zerocode-orc check`, and
/// for a pane bound to no run that was a refusal naming nothing. `check` now
/// names where letters to the pane's own address wait — in its refusal when
/// it is bound to nothing, beside its read when it is bound elsewhere — and a
/// pane that sits in a chair, which reads a copy where it sits, is told
/// nothing.
#[test]
fn check_names_the_runs_where_letters_to_this_panes_own_address_wait() {
    let mut bench = Bench::new();
    let mut chair = Team::new("team-chair", "chair", 60);
    let mut free = Team::new("team-free", "free", 70);
    let mut other = Team::new("team-other", "other", 80);
    let home = run_of(&mut bench, &mut chair, "home");
    write_to_run(&mut bench, &mut free, &home, "hello");
    let letter = last_letter_from(&bench, &home, &free);
    as_leader_of(
        &mut bench,
        &mut chair,
        &["reply", "--to-message", &letter, "--body", "back"],
    );

    let refused = answered_as_leader_of(&mut bench, &mut free, &["check"]);
    assert_ne!(refused.exit_code, 0, "{refused:?}");
    assert!(
        refused
            .stderr
            .contains(&format!("1 in {home}: `zerocode-orc check --run {home}`")),
        "a pane bound to no run was not told where its letter waits: {}",
        refused.stderr
    );
    let read = as_leader_of(&mut bench, &mut free, &["check", "--run", &home]);
    assert!(bodies(&read).contains(&"back".to_string()), "{read}");
    let delivery = read["deliveryId"]
        .as_str()
        .expect("a delivery to acknowledge");
    as_leader_of(
        &mut bench,
        &mut free,
        &["check", "--run", &home, "--ack", delivery],
    );

    // Bound to another run, it is told beside its own read.
    let away = run_of(&mut bench, &mut other, "away");
    let bound = as_leader_of(&mut bench, &mut free, &["run-use", &away]);
    assert_eq!(bound["seated"], false, "{bound}");
    as_leader_of(
        &mut bench,
        &mut chair,
        &[
            "send",
            "--to",
            &pane_of(&free),
            "--type",
            "status",
            "--body",
            "again",
        ],
    );
    let read = as_leader_of(&mut bench, &mut free, &["check"]);
    assert_eq!(read["mailElsewhere"][0]["run"], home.as_str(), "{read}");
    assert_eq!(read["mailElsewhere"][0]["waiting"], 1, "{read}");

    // A pane that sits in a chair reads a copy where it sits: nothing to tell.
    write_to_run(&mut bench, &mut chair, &away, "from-a-chair");
    let letter = last_letter_from(&bench, &away, &chair);
    as_leader_of(
        &mut bench,
        &mut other,
        &["reply", "--to-message", &letter, "--body", "to-the-chair"],
    );
    let read = as_leader_of(&mut bench, &mut chair, &["check", "--peek"]);
    assert!(
        read.get("mailElsewhere").is_none(),
        "a pane that sits in a chair was pointed at its pane address too: {read}"
    );
}

/// After a restart a conversation goes on in a pane with a new address, and
/// letters to the old one name it (a99e24911) — but a pane address answers
/// only once it has written in the run, so the conversation could not be
/// written to until it spoke again. The window's word that the same
/// conversation goes on makes the new pane answerable where the old one spoke,
/// and tells it how many letters wait at the old address. Once per run.
#[test]
fn a_pane_that_goes_on_for_another_is_told_so_and_answers_where_the_old_one_spoke() {
    let mut bench = Bench::new();
    let mut chair = Team::new("team-chair", "chair", 60);
    let mut old = Team::new("team-old", "old", 70);
    let new = Team::new("team-new", "new", 75);
    let mut elsewhere = Team::new("team-elsewhere", "elsewhere", 80);
    let home = run_of(&mut bench, &mut chair, "home");
    let quiet = run_of(&mut bench, &mut elsewhere, "quiet");
    write_to_run(&mut bench, &mut old, &home, "before-the-restart");
    let letter = last_letter_from(&bench, &home, &old);
    as_leader_of(
        &mut bench,
        &mut chair,
        &[
            "reply",
            "--to-message",
            &letter,
            "--body",
            "waits-at-the-old-address",
        ],
    );
    let to_new = |bench: &mut Bench, chair: &mut Team| {
        answered_as_leader_of(
            bench,
            chair,
            &[
                "send",
                "--to",
                &pane_of(&new),
                "--type",
                "status",
                "--body",
                "to-the-new-pane",
            ],
        )
    };
    let refused = to_new(&mut bench, &mut chair);
    assert_ne!(
        refused.exit_code, 0,
        "a pane that never spoke was answerable"
    );

    let (old_seat, new_seat) = (
        format!("{}/{}", old.id, agent_teams::LEADER_PANE),
        format!("{}/{}", new.id, agent_teams::LEADER_PANE),
    );
    bench.clock += 1;
    assert!(
        bench
            .ledger
            .seat_continued(&old_seat, &new_seat, bench.clock),
        "nothing was told to the pane the conversation goes on in"
    );
    assert!(
        !bench
            .ledger
            .seat_continued(&old_seat, &new_seat, bench.clock + 1),
        "the same word was filed twice"
    );
    let told: Vec<&Message> = bench
        .ledger
        .run(&home)
        .expect("the run")
        .messages()
        .iter()
        .filter(|message| {
            message.from == LEDGER_ITSELF
                && message.to == pane_of(&new)
                && message.subject.as_str().starts_with(SEAT_CONTINUES)
        })
        .collect();
    assert_eq!(told.len(), 1, "{told:?}");
    assert!(
        told[0]
            .body
            .as_str()
            .contains(&format!("1 letter(s) to {} wait unread", pane_of(&old))),
        "{}",
        told[0].body.as_str()
    );
    assert!(
        !bench
            .ledger
            .run(&quiet)
            .expect("the run")
            .messages()
            .iter()
            .any(|message| message.to == pane_of(&new)),
        "a run the old pane never spoke in was told"
    );
    let sent = to_new(&mut bench, &mut chair);
    assert_eq!(
        sent.exit_code, 0,
        "the pane the conversation goes on in is not answerable: {}",
        sent.stderr
    );
}

/// (c, d) The window's half asks the ledger where such a letter goes: the run
/// the `run:` address names, and the run a `pane:` recipient sits in, each
/// with its project — and the project of the run each recipient sits in.
#[test]
fn the_reach_of_a_letter_names_the_run_it_is_filed_or_copied_in() {
    let mut bench = Bench::new();
    let mut here = Team::new("team-here", "here", 60);
    let mut there = Team::new("team-there", "there", 70);
    let home = run_of(&mut bench, &mut here, "home");
    let away = run_of(&mut bench, &mut there, "away");
    write_to_run(&mut bench, &mut there, &home, "hello-from-away");
    let root = "/Users/dev/repo/.git";
    for run in [&home, &away] {
        assert!(bench.ledger.state_run_project(run, root, "team-x/%1", 1));
    }
    let seat = format!("{}/{}", there.id, agent_teams::LEADER_PANE);
    for to in [format!("run:{away}"), pane_of(&there)] {
        let argv: Vec<String> = ["send", "--to", &to, "--type", "status"]
            .map(str::to_string)
            .to_vec();
        let door = crate::orchestration::project_scope::reach(
            &bench.ledger,
            &argv,
            &here.id,
            agent_teams::LEADER_PANE,
            Some(&bench_actor(&here.id, agent_teams::LEADER_PANE)),
            root,
        );
        assert_eq!(
            door.carried_run.as_ref().map(|(id, _)| id.as_str()),
            Some(away.as_str()),
            "{to}"
        );
        assert!(door.recipient_seats.contains(&seat), "{to}: {door:?}");
        assert_eq!(door.seated_root(&seat), Some(root), "{to}");
    }
}

/// A task handed in at `source` by a worker of the bench's own run.
fn handed_in(bench: &mut Bench, source: &str) -> (String, String) {
    bench.json("run-create --name rebased");
    let task = bench.json("task-create --spec rebased-head")["taskId"]
        .as_str()
        .expect("a task id")
        .to_string();
    let (_, pane) = bench.seat(&format!("worker-start --agent claude --task {task}"));
    let attempt = bench.ledger.runs()[0].dispatches[0].id.clone();
    bench.json_at(
        &pane,
        &format!("send --type worker_done --body {{\"ok\":true,\"head\":\"{source}\"}}"),
    );
    (task, attempt)
}

/// One change, file by file.
fn change(commits: u32, files: &[(&str, &str)]) -> PatchSeen {
    PatchSeen {
        commits,
        files: files
            .iter()
            .map(|(file, digest)| ((*file).to_string(), (*digest).to_string()))
            .collect(),
    }
}

/// What git said the two heads change.
fn git_said(tested: &str, was: PatchSeen, source: &str, now: PatchSeen) -> ReceiptFiles {
    let mut seen = CommitsSeen::default();
    seen.insert_patch(tested, was);
    seen.insert_patch(source, now);
    ReceiptFiles::default().with_commits(Some(seen))
}

/// (e) A verification whose tested head is the source rebased — the same
/// change, file by file — is taken and says so; another change is refused
/// naming the files that differ; and without git's word it is refused as
/// before, saying git was not asked.
#[test]
fn a_verification_of_the_tested_head_rebased_is_taken_and_another_change_is_refused() {
    const TESTED: &str = "abc1234";
    const SOURCE: &str = "def5678";
    let verify = |bench: &mut Bench, task: &str, attempt: &str, git: &ReceiptFiles| {
        bench.at_argv_with(
            agent_teams::LEADER_PANE,
            [
                "task-update",
                "--task",
                task,
                "--result",
                &format!(r#"{{"verified":true,"testedHead":"{TESTED}"}}"#),
                "--attempt",
                attempt,
                "--source",
                SOURCE,
            ]
            .map(str::to_string)
            .to_vec(),
            git,
        )
    };
    let same = change(2, &[("src/a.rs", "1111"), ("src/b.rs", "2222")]);

    let mut bench = Bench::new();
    let (task, attempt) = handed_in(&mut bench, SOURCE);
    for (git, says) in [
        (ReceiptFiles::default(), "git was not asked"),
        (
            git_said(
                TESTED,
                same.clone(),
                SOURCE,
                change(2, &[("src/a.rs", "1111"), ("src/c.rs", "3333")]),
            ),
            "src/b.rs, src/c.rs",
        ),
        (
            git_said(
                TESTED,
                change(0, &[]),
                SOURCE,
                change(2, &[("src/a.rs", "1111")]),
            ),
            "changes nothing",
        ),
    ] {
        let refused = verify(&mut bench, &task, &attempt, &git);
        assert_ne!(refused.reply.exit_code, 0, "{says}: {:?}", refused.reply);
        assert!(
            refused.reply.stderr.contains("testedHead") && refused.reply.stderr.contains(says),
            "the refusal does not say {says:?}: {}",
            refused.reply.stderr
        );
    }

    let taken = verify(
        &mut bench,
        &task,
        &attempt,
        &git_said(TESTED, same.clone(), SOURCE, same),
    );
    assert_eq!(taken.reply.exit_code, 0, "{:?}", taken.reply);
    let result: serde_json::Value = serde_json::from_str(
        bench.ledger.runs()[0]
            .task(&task)
            .expect("the task")
            .result
            .as_str(),
    )
    .expect("a JSON result");
    assert_eq!(result["verified"], true, "{result}");
    assert_eq!(result[REBASED_KEY]["testedHead"], TESTED, "{result}");
    assert_eq!(result[REBASED_KEY]["source"], SOURCE, "{result}");
    assert_eq!(result[REBASED_KEY]["samePatchId"], true, "{result}");
}

/// r2 (review t-97475 P3-3): a tested head written short and a source written
/// in full that git resolves to one commit are the source itself — taken, with
/// no note of a rebase — not a change that adds nothing on top of itself.
#[test]
fn a_tested_head_named_short_is_the_source_named_in_full() {
    const FULL: &str = "abc1234abc1234abc1234abc1234abc1234abc12";
    const SHORT: &str = "abc1234ab";
    let mut bench = Bench::new();
    let (task, attempt) = handed_in(&mut bench, FULL);
    let mut seen = CommitsSeen::default();
    for named in [SHORT, FULL] {
        seen.insert(
            named,
            CommitSeen {
                full: Some(FULL.to_string()),
                ..CommitSeen::default()
            },
        );
    }
    let taken = bench.at_argv_with(
        agent_teams::LEADER_PANE,
        [
            "task-update",
            "--task",
            &task,
            "--result",
            &format!(r#"{{"verified":true,"testedHead":"{SHORT}"}}"#),
            "--attempt",
            &attempt,
            "--source",
            FULL,
        ]
        .map(str::to_string)
        .to_vec(),
        &ReceiptFiles::default().with_commits(Some(seen)),
    );
    assert_eq!(taken.reply.exit_code, 0, "{:?}", taken.reply);
    let result: serde_json::Value = serde_json::from_str(
        bench.ledger.runs()[0]
            .task(&task)
            .expect("the task")
            .result
            .as_str(),
    )
    .expect("a JSON result");
    assert_eq!(result["verified"], true, "{result}");
    assert_eq!(result.get(REBASED_KEY), None, "{result}");
}

/// t-98182 L1 (review t-97475 P3-2): a letter to another run's `run:` address
/// carries `--task` and `--dispatch` only when they name that run's own task
/// and attempt. The sending run's task, or an attempt nobody there holds, is
/// refused in the sender's hearing and nothing is filed; the receiving run's
/// own task rides along as it always did.
#[test]
fn a_letter_to_another_run_carries_only_that_runs_own_task_and_attempt() {
    let mut bench = Bench::new();
    let mut here = Team::new("team-here", "here", 60);
    let mut there = Team::new("team-there", "there", 70);
    run_of(&mut bench, &mut here, "home");
    let away = run_of(&mut bench, &mut there, "away");
    let task_of = |bench: &mut Bench, team: &mut Team| {
        as_leader_of(bench, team, &["task-create", "--spec", "own-work"])["taskId"]
            .as_str()
            .expect("a task id")
            .to_string()
    };
    let mine = task_of(&mut bench, &mut here);
    let theirs = task_of(&mut bench, &mut there);
    let chair = format!("run:{away}");
    let filed = |bench: &Bench| bench.ledger.run(&away).expect("the run").messages().len();

    let before = filed(&bench);
    for (flag, named) in [("--task", mine.as_str()), ("--dispatch", "dp-nobody")] {
        let refused = answered_as_leader_of(
            &mut bench,
            &mut here,
            &[
                "send", "--to", &chair, "--type", "status", flag, named, "--body", "carried",
            ],
        );
        assert_ne!(refused.exit_code, 0, "{flag} {named}: {refused:?}");
        assert!(
            refused.stderr.contains(named) && refused.stderr.contains(&away),
            "{flag}: the refusal does not name {named} and {away}: {}",
            refused.stderr
        );
        assert_eq!(filed(&bench), before, "{flag} {named} was filed in {away}");
    }

    let sent = as_leader_of(
        &mut bench,
        &mut here,
        &[
            "send", "--to", &chair, "--type", "status", "--task", &theirs, "--body", "theirs",
        ],
    );
    let id = sent["messageId"].as_str().expect("a letter id");
    let held = bench
        .ledger
        .run(&away)
        .and_then(|run| run.message(id))
        .expect("filed in the run it names");
    assert_eq!(held.task.as_deref(), Some(theirs.as_str()));
}

/// t-98182 L2 (review t-97475 P3-4): the window's look at where a letter
/// goes and the ledger's write are two moments, and the run the letter is
/// carried into can learn another project between them. The write asks
/// again, in the one command that files the letter: a letter to another
/// run's chair that is now another project's is refused and nothing is
/// filed — as the window would have refused it, had it looked a moment later.
#[test]
fn a_run_that_became_another_projects_after_the_look_is_refused_at_the_write() {
    const HOME: &str = "/Users/dev/home/.git";
    const ELSEWHERE: &str = "/Users/dev/elsewhere/.git";
    let mut bench = Bench::new();
    let mut here = Team::new("team-here", "here", 60);
    let mut there = Team::new("team-there", "there", 70);
    let home = run_of(&mut bench, &mut here, "home");
    let away = run_of(&mut bench, &mut there, "away");
    assert!(
        bench
            .ledger
            .state_run_project(&home, HOME, "team-here/%1", bench.clock)
    );
    let actor = bench_actor(&here.id, agent_teams::LEADER_PANE);
    let argv = words(&format!(
        "send --to run:{away} --type status --body crossed --retry-request l2-race"
    ));
    let looked = reach(
        &bench.ledger,
        &argv,
        &here.id,
        agent_teams::LEADER_PANE,
        Some(&actor),
        HOME,
    );
    assert_eq!(looked.carried_run, Some((away.clone(), None)), "{looked:?}");

    assert!(
        bench
            .ledger
            .state_run_project(&away, ELSEWHERE, "team-there/%1", bench.clock)
    );
    let filed = bench.ledger.run(&away).expect("the run").messages().len();
    bench.clock += 1;
    let planned = plan_in_project(
        &mut bench.ledger,
        &mut here,
        &bench.launcher,
        &argv,
        agent_teams::LEADER_PANE,
        bench.clock,
        Some(&actor),
        &ReceiptFiles::default(),
        None,
        Some(HOME),
    );
    assert_ne!(planned.reply.exit_code, 0, "{:?}", planned.reply);
    assert!(
        planned.reply.stderr.contains(&away),
        "the refusal does not name {away}: {}",
        planned.reply.stderr
    );
    assert_eq!(
        bench.ledger.run(&away).expect("the run").messages().len(),
        filed,
        "a letter was filed in a run of another project"
    );
}

/// t-98182 L6 (review t-97548 r3 P3-2): a letter says the run its writer
/// wrote it from. The writer signed as an older run's chair, then wrote a
/// status letter from a newer run's chair, signing nothing there. A restart
/// emptied both chairs and two other panes sat down in them with `run-use`,
/// which wrote over the newer chair's record. The copy of the reply goes to
/// the run the letter was written from — whoever sits in its chair now — and
/// never to the older run, whose chair another pane reads.
#[test]
fn a_reply_goes_to_the_run_its_letter_was_written_from_after_run_use_wrote_over_that_chair() {
    let mut bench = Bench::new();
    let mut here = Team::new("team-here", "here", 60);
    let mut there = Team::new("team-there", "there", 70);
    let mut here_after = Team::new("team-here-after", "here-after", 80);
    let mut first_after = Team::new("team-first-after", "first-after", 90);
    let mut away_after = Team::new("team-away-after", "away-after", 100);
    let home = run_of(&mut bench, &mut here, "home");
    let first = run_of(&mut bench, &mut there, "first");
    write_to_run(&mut bench, &mut there, &first, "on-duty-first");
    let away = run_of(&mut bench, &mut there, "away");
    let letter = write_to_run(&mut bench, &mut there, &home, "status-from-away");

    bench.clock += 1;
    bench.ledger.window_restarted(bench.clock);
    for (team, run) in [
        (&mut here_after, &home),
        (&mut first_after, &first),
        (&mut away_after, &away),
    ] {
        let seated = as_leader_of(&mut bench, team, &["run-use", run]);
        assert_eq!(seated["seated"], true, "{seated}");
    }

    let argv = words(&format!("reply --to-message {letter} --body noted"));
    let door = reach(
        &bench.ledger,
        &argv,
        &here_after.id,
        agent_teams::LEADER_PANE,
        Some(&bench_actor(&here_after.id, agent_teams::LEADER_PANE)),
        "",
    );
    assert_eq!(
        door.carried_run.as_ref().map(|(id, _)| id.as_str()),
        Some(away.as_str()),
        "carried to {first}, a run the letter was not written from"
    );
    let answered = as_leader_of(
        &mut bench,
        &mut here_after,
        &["reply", "--to-message", &letter, "--body", "noted"],
    );
    assert_eq!(
        answered["deliveredTo"]["runId"],
        away.as_str(),
        "{answered}"
    );
    assert!(
        !read_by(&mut bench, &mut first_after).contains(&"noted".to_string()),
        "the pane in the older chair read a reply to a letter written from {away}"
    );
    assert!(
        read_by(&mut bench, &mut away_after).contains(&"noted".to_string()),
        "the chair of the run the letter was written from did not read the reply"
    );
}

/// t-98182 r3 (review t-99153 P3-1): `question-close` copies its line — the
/// question's id, the chair that closed it and the closer's own reason — into
/// the run that asked. That run learned another project after it asked, and
/// the close is refused where the copy would cross into it: by the window's
/// look and again at the write, as a `reply` is, and nothing is filed there.
#[test]
fn a_close_copied_into_a_run_of_another_project_is_refused() {
    const HOME: &str = "/Users/dev/home/.git";
    const ELSEWHERE: &str = "/Users/dev/elsewhere/.git";
    let mut bench = Bench::new();
    let mut asker = Team::new("team-asker", "asker", 60);
    let mut answerer = Team::new("team-answerer", "answerer", 70);
    let asking = run_of(&mut bench, &mut asker, "asker");
    let answering = run_of(&mut bench, &mut answerer, "answerer");
    let question = ask_another_run(&mut bench, &mut asker, &answering, "anyone?");
    for (run, root, team) in [(&answering, HOME, &answerer), (&asking, ELSEWHERE, &asker)] {
        let seat = format!("{}/{}", team.id, agent_teams::LEADER_PANE);
        assert!(
            bench
                .ledger
                .state_run_project(run, root, &seat, bench.clock)
        );
    }
    let actor = bench_actor(&answerer.id, agent_teams::LEADER_PANE);
    let argv = words(&format!(
        "question-close --message {question} --reason 끝남 --retry-request p3-1"
    ));
    let looked = reach(
        &bench.ledger,
        &argv,
        &answerer.id,
        agent_teams::LEADER_PANE,
        Some(&actor),
        HOME,
    );
    assert_eq!(
        looked.carried_run,
        Some((
            asking.clone(),
            bench
                .ledger
                .run(&asking)
                .and_then(|run| run.project.clone())
        )),
        "the window's look does not see the close's copy: {looked:?}"
    );

    let filed = bench.ledger.run(&asking).expect("the run").messages().len();
    bench.clock += 1;
    let planned = plan_in_project(
        &mut bench.ledger,
        &mut answerer,
        &bench.launcher,
        &argv,
        agent_teams::LEADER_PANE,
        bench.clock,
        Some(&actor),
        &ReceiptFiles::default(),
        None,
        Some(HOME),
    );
    assert_ne!(planned.reply.exit_code, 0, "{:?}", planned.reply);
    assert!(
        planned.reply.stderr.contains(&asking),
        "the refusal does not name {asking}: {}",
        planned.reply.stderr
    );
    assert_eq!(
        bench.ledger.run(&asking).expect("the run").messages().len(),
        filed,
        "a close's line was filed in a run of another project"
    );
}

/// t-98182 r3 (review t-99153 P3-2): a letter to a pane is filed in the run
/// it was sent in, and its copy where that pane reads its mail. The copy is
/// a courtesy to the reader, so it carries `--task` and `--dispatch` only
/// when they are that run's own — the sending run's task does not ride into
/// another run — and the letter itself keeps them.
#[test]
fn a_copy_to_a_pane_in_another_run_carries_only_that_runs_own_task() {
    let mut bench = Bench::new();
    let mut here = Team::new("team-here", "here", 60);
    let mut there = Team::new("team-there", "there", 70);
    let home = run_of(&mut bench, &mut here, "home");
    let away = run_of(&mut bench, &mut there, "away");
    let mine = as_leader_of(
        &mut bench,
        &mut here,
        &["task-create", "--spec", "own-work"],
    )["taskId"]
        .as_str()
        .expect("a task id")
        .to_string();
    // The other run's coordinator writes into this run, so its pane is a
    // reader here.
    write_to_run(&mut bench, &mut there, &home, "hello-from-away");
    let pane = pane_of(&there);

    let sent = as_leader_of(
        &mut bench,
        &mut here,
        &[
            "send", "--to", &pane, "--type", "status", "--task", &mine, "--body", "copied",
        ],
    );
    assert_eq!(sent["deliveredTo"]["runId"], away.as_str(), "{sent}");
    let held = |run: &str, key: &str| {
        let id = sent[key]["messageId"]
            .as_str()
            .or_else(|| sent[key].as_str())
            .expect("a letter id");
        bench
            .ledger
            .run(run)
            .and_then(|run| run.message(id))
            .map(|letter| (letter.task.clone(), letter.dispatch.clone()))
            .expect("filed")
    };
    assert_eq!(held(&home, "messageId"), (Some(mine.clone()), None));
    assert_eq!(
        held(&away, "deliveredTo"),
        (None, None),
        "{home}'s task rode into {away} on the copy"
    );
}

/// t-98182 r3 (review t-99153 P3-3): a pane can hold two live chairs at once.
/// A letter it writes as the first run's chair (`send --run <first>`) says it
/// was written from that run — the run the verb acted in — and not from the
/// chair the pane sat down in later, so the reply's copy goes back there.
#[test]
fn a_letter_says_the_run_its_verb_acted_in_among_two_chairs_the_pane_holds() {
    let mut bench = Bench::new();
    let mut here = Team::new("team-here", "here", 60);
    let mut there = Team::new("team-there", "there", 70);
    let home = run_of(&mut bench, &mut here, "home");
    let first = run_of(&mut bench, &mut there, "first");
    let later = run_of(&mut bench, &mut there, "later");
    let chair = format!("run:{home}");
    let sent = as_leader_of(
        &mut bench,
        &mut there,
        &[
            "send",
            "--run",
            &first,
            "--to",
            &chair,
            "--type",
            "status",
            "--body",
            "from-first",
        ],
    );
    let id = sent["messageId"].as_str().expect("a letter id").to_string();
    let held = bench
        .ledger
        .run(&home)
        .and_then(|run| run.message(&id))
        .expect("filed in the run it names");
    assert_eq!(
        held.from_run.as_deref(),
        Some(first.as_str()),
        "written as {first}'s chair, and said to be from {later}"
    );

    let answered = as_leader_of(
        &mut bench,
        &mut here,
        &["reply", "--to-message", &id, "--body", "noted"],
    );
    assert_eq!(
        answered["deliveredTo"]["runId"],
        first.as_str(),
        "{answered}"
    );
}

/// t-98182 r4 (review t-99233 P3-D): `question-close` copies its line into
/// the run that asked, and the copy carries `--task` and `--dispatch` only
/// when that run holds them, as a `pane:` letter's copy does (r3 P3-2). The
/// question was asked of another run under that run's own task; the line
/// filed where it was asked keeps it.
#[test]
fn a_close_copied_into_the_asking_run_carries_only_that_runs_own_task() {
    let mut bench = Bench::new();
    let mut asker = Team::new("team-asker", "asker", 60);
    let mut answerer = Team::new("team-answerer", "answerer", 70);
    let asking = run_of(&mut bench, &mut asker, "asker");
    let answering = run_of(&mut bench, &mut answerer, "answerer");
    let theirs = as_leader_of(
        &mut bench,
        &mut answerer,
        &["task-create", "--spec", "own-work"],
    )["taskId"]
        .as_str()
        .expect("a task id")
        .to_string();
    let chair = format!("run:{answering}");
    let question = as_leader_of(
        &mut bench,
        &mut asker,
        &[
            "send", "--run", &answering, "--to", &chair, "--type", "question", "--task", &theirs,
            "--body", "anyone?",
        ],
    )["messageId"]
        .as_str()
        .expect("a question id")
        .to_string();

    let closed = as_leader_of(
        &mut bench,
        &mut answerer,
        &[
            "question-close",
            "--message",
            &question,
            "--reason",
            "끝남",
            "--retry-request",
            "p3-d",
        ],
    );
    assert_eq!(closed["deliveredTo"]["runId"], asking.as_str(), "{closed}");
    let held = |run: &str, id: &serde_json::Value| {
        let id = id.as_str().expect("a letter id");
        bench
            .ledger
            .run(run)
            .and_then(|run| run.message(id))
            .map(|letter| (letter.task.clone(), letter.dispatch.clone()))
            .expect("filed")
    };
    assert_eq!(
        held(&answering, &closed["closeId"]),
        (Some(theirs.clone()), None)
    );
    assert_eq!(
        held(&asking, &closed["deliveredTo"]["messageId"]),
        (None, None),
        "{answering}'s task rode into {asking} on the close's copy"
    );
}

/// t-98182 r4 (review t-99233 P3-E): a letter written from the run it is
/// answered in has no other run to be copied to. The pane holds two chairs;
/// a letter it signed as its pane in the first run says it was written from
/// there, and the answer in that run goes nowhere else — not to the other
/// chair the pane sits in.
#[test]
fn a_letter_answered_in_the_run_it_was_written_from_is_copied_nowhere() {
    let mut bench = Bench::new();
    let mut there = Team::new("team-there", "there", 70);
    let first = run_of(&mut bench, &mut there, "first");
    let later = run_of(&mut bench, &mut there, "later");
    let seat = format!("{}/{}", there.id, agent_teams::LEADER_PANE);
    let id = bench
        .ledger
        .post_acting(
            &first,
            super::letter_answers::letter(&pane_of(&there), &format!("run:{first}"), "own-run"),
            &seat,
            &first,
            bench.clock,
        )
        .expect("filed");
    let written = bench
        .ledger
        .run(&first)
        .and_then(|run| run.message(&id))
        .expect("filed")
        .clone();
    assert_eq!(written.from_run.as_deref(), Some(first.as_str()));

    assert_eq!(
        bench.ledger.asker_run(&written, &first),
        None,
        "a letter answered where it was written from was copied to {later}"
    );
}
