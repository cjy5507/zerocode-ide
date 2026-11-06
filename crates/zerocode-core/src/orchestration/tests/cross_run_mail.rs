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

use super::letter_answers::{answered_as_leader_of, as_leader_of, run_of, write_to_run};
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
