//! t-65286: which project a run belongs to — written by the pane that made
//! it, read off a run made before, confirmed only by that run's live
//! coordinator — and the run a verb answers in, read the way the plan reads
//! it, so the window can hold a pane of one project out of another's run.

use super::*;
use crate::orchestration::project_scope::{
    PROJECT_ROOT_MAX, ProjectSource, RunProject, WINDOW_VERBS, acting_run, label, reach, refusal,
};

const A: &str = "/Users/dev/a/.git";
const B: &str = "/Users/dev/b/.git";

/// The leader seat every bench verb is typed from.
const SEAT: &str = "team-1/%1";

fn run_of(said: &serde_json::Value) -> String {
    said["runId"].as_str().expect("a run id").to_string()
}

/// `run-create` writes the project of the pane that made the run — the root,
/// `created`, when, and from which seat — says the folder in its answer, and
/// `run-show` says all of it. A pane in no known folder makes a run with no
/// project, and an answer in the shape it always had.
#[test]
fn run_create_writes_the_project_of_the_pane_that_made_the_run() {
    let mut bench = Bench::new();
    bench.clock += 1;
    let actor = bench_actor(&bench.team.id, agent_teams::LEADER_PANE);
    let planned = plan_in_project(
        &mut bench.ledger,
        &mut bench.team,
        &bench.launcher,
        &words("run-create --name a --retry-request made-a"),
        agent_teams::LEADER_PANE,
        bench.clock,
        Some(&actor),
        &crate::orchestration::evidence::ReceiptFiles::default(),
        None,
        Some(A),
    );
    assert_eq!(planned.reply.exit_code, 0, "{}", planned.reply.stderr);
    let said: serde_json::Value = serde_json::from_str(&planned.reply.stdout).expect("JSON");
    assert_eq!(said["project"], "/Users/dev/a");
    let made = run_of(&said);
    assert_eq!(
        bench.ledger.run(&made).and_then(|run| run.project.clone()),
        Some(RunProject {
            root: A.to_string(),
            source: ProjectSource::Created,
            at_ms: bench.clock,
            seat: Some(SEAT.to_string()),
        })
    );
    bench.clock += 1;
    let shown = plan_in_project(
        &mut bench.ledger,
        &mut bench.team,
        &bench.launcher,
        &words(&format!("run-show --run {made}")),
        agent_teams::LEADER_PANE,
        bench.clock,
        Some(&actor),
        &crate::orchestration::evidence::ReceiptFiles::default(),
        None,
        Some(A),
    );
    assert_eq!(shown.reply.exit_code, 0, "{}", shown.reply.stderr);
    let shown: serde_json::Value = serde_json::from_str(&shown.reply.stdout).expect("JSON");
    assert_eq!(
        shown["project"],
        serde_json::json!({
            "folder": "/Users/dev/a",
            "root": A,
            "source": "created",
            "atMs": bench.clock - 1,
            "seat": SEAT,
        })
    );

    let plain = bench.json("run-create --name b");
    assert!(plain.get("project").is_none(), "{plain}");
    assert!(
        bench
            .ledger
            .run(&run_of(&plain))
            .is_some_and(|run| run.project.is_none())
    );
    assert!(bench.json(&format!("run-show --run {}", run_of(&plain)))["project"].is_null());
}

#[test]
fn a_project_learned_after_preflight_is_checked_before_mutation() {
    let mut bench = Bench::new();
    let run = run_of(&bench.json("run-create --name legacy"));
    let actor = bench_actor(&bench.team.id, agent_teams::LEADER_PANE);
    let command = words(&format!(
        "task-create --run {run} --spec own-work --retry-request scope-race"
    ));
    let before = reach(
        &bench.ledger,
        &command,
        &bench.team.id,
        agent_teams::LEADER_PANE,
        Some(&actor),
        A,
    );
    assert_eq!(before.run, Some((run.clone(), None)));
    assert!(bench.ledger.state_run_project(&run, B, SEAT, bench.clock));
    let planned = plan_in_project(
        &mut bench.ledger,
        &mut bench.team,
        &bench.launcher,
        &command,
        agent_teams::LEADER_PANE,
        bench.clock + 1,
        Some(&actor),
        &crate::orchestration::evidence::ReceiptFiles::default(),
        None,
        Some(A),
    );
    assert_eq!(
        planned.reply.exit_code, 1,
        "a changed project was not checked atomically"
    );
    assert!(
        bench.ledger.run(&run).expect("run").tasks.is_empty(),
        "a refused command created a task"
    );
}

/// The run a verb answers in, by the plan's own rule: `--run`, the id
/// `run-use` and `run-takeover` name, or the caller's bound run — and none for
/// the verbs that answer for the whole window, for the bare evidence read, and
/// for a run the ledger does not hold.
#[test]
fn the_run_a_verb_answers_in_is_read_the_way_the_plan_reads_it() {
    let mut bench = Bench::new();
    let one = run_of(&bench.json("run-create --name one"));
    let two = run_of(&bench.json("run-create --name two"));
    let actor = bench_actor(&bench.team.id, agent_teams::LEADER_PANE);
    let acting = |line: &str| {
        acting_run(
            &bench.ledger,
            &words(line),
            "team-1",
            agent_teams::LEADER_PANE,
            Some(&actor),
        )
    };
    assert_eq!(acting("task-list"), Some(two.clone()), "the bound run");
    assert_eq!(acting(&format!("task-list --run {one}")), Some(one.clone()));
    assert_eq!(acting(&format!("run-use {one}")), Some(one.clone()));
    assert_eq!(acting(&format!("run-use --run {one}")), Some(one.clone()));
    assert_eq!(acting("run-use"), None, "run-use names its run or none");
    assert_eq!(
        acting(&format!("run-takeover {one} --from x/%1")),
        Some(one.clone())
    );
    assert_eq!(acting("run-takeover --from x/%1"), Some(two.clone()));
    assert_eq!(acting("task-list --run run-404"), None);
    assert_eq!(acting(WORKTREE_EVIDENCE_VERB), None);
    assert_eq!(
        acting(&format!("{WORKTREE_EVIDENCE_VERB} --worker w-1")),
        Some(two.clone())
    );
    for verb in WINDOW_VERBS {
        assert_eq!(acting(&format!("{verb} --run {one}")), None, "{verb}");
    }
    assert_eq!(
        acting_run(
            &bench.ledger,
            &words("task-list"),
            "team-9",
            "%1",
            Some("nobody")
        ),
        None,
        "a pane bound to nothing answers in no run"
    );
}

/// Every verb but the window's own answers in the run its `--run` names — so
/// a verb added to the table is held to its run's project by default, and a
/// window verb is a verb the table holds.
#[test]
fn every_verb_but_the_windows_answers_in_the_run_its_run_flag_names() {
    let mut bench = Bench::new();
    let one = run_of(&bench.json("run-create --name one"));
    bench.json("run-create --name two");
    for (verb, _, _) in VERBS {
        if WINDOW_VERBS.contains(verb) {
            continue;
        }
        let mut argv = vec![verb.to_string(), "--run".to_string(), one.clone()];
        if *verb == WORKTREE_EVIDENCE_VERB {
            argv.extend(["--worker".to_string(), "w-1".to_string()]);
        }
        assert_eq!(
            acting_run(&bench.ledger, &argv, "team-1", "%1", None),
            Some(one.clone()),
            "{verb}"
        );
    }
    for verb in WINDOW_VERBS {
        assert!(
            VERBS.iter().any(|(name, _, _)| name == verb),
            "{verb} is not a verb"
        );
    }
}

/// What a verb reaches is read off the ledger in one place: the run it answers
/// in with that run's project, whether the caller holds the run's live seat,
/// and — for the verbs that answer for the whole window — every run of
/// another project than the caller's.
#[test]
fn what_a_verb_reaches_is_read_in_one_place() {
    let mut bench = Bench::new();
    let actor = bench_actor(&bench.team.id, agent_teams::LEADER_PANE);
    bench.clock += 1;
    let made = plan_in_project(
        &mut bench.ledger,
        &mut bench.team,
        &bench.launcher,
        &words("run-create --name a --retry-request reach-a"),
        agent_teams::LEADER_PANE,
        bench.clock,
        Some(&actor),
        &crate::orchestration::evidence::ReceiptFiles::default(),
        None,
        Some(A),
    );
    let made: serde_json::Value = serde_json::from_str(&made.reply.stdout).expect("JSON");
    let of_a = run_of(&made);
    let plain = run_of(&bench.json("run-create --name plain"));
    let project = bench
        .ledger
        .run(&of_a)
        .and_then(|run| run.project.clone())
        .expect("the run's project");
    let from = |line: &str, pane: &str, root: &str| {
        reach(
            &bench.ledger,
            &words(line),
            "team-1",
            pane,
            Some(&actor),
            root,
        )
    };
    let leader = agent_teams::LEADER_PANE;

    let crossing = from(&format!("check --run {of_a}"), leader, B);
    assert_eq!(crossing.run, Some((of_a.clone(), Some(project.clone()))));
    assert!(
        crossing.coordinator,
        "the run's own seat was not read as its seat"
    );
    assert!(
        crossing.elsewhere.is_empty(),
        "a run verb reached other runs"
    );
    assert!(!from(&format!("check --run {of_a}"), "%2", B).coordinator);
    assert_eq!(
        from("task-list", leader, B).run,
        Some((plain.clone(), None))
    );
    for verb in ["inbox", "run-list", "reset --all"] {
        let window = from(verb, leader, B);
        assert_eq!(window.run, None, "{verb}");
        assert_eq!(
            window.elsewhere,
            vec![(of_a.clone(), project.clone())],
            "{verb}"
        );
        assert!(from(verb, leader, A).elsewhere.is_empty(), "{verb}");
    }
}

/// A run stands to a pane as the same project, another, or unknown — and only
/// a crossing of a project that was not merely inferred holds the boundary.
#[test]
fn a_run_stands_to_a_pane_as_the_same_project_another_or_unknown() {
    let mut ledger = Ledger::new();
    let run = ledger.create_run("scoped", 1);
    assert_eq!(
        ledger.run(&run).expect("the run").scope(Some(A)),
        Scope::Unknown
    );
    assert!(ledger.state_run_project(&run, A, SEAT, 2));
    let held = ledger.run(&run).expect("the run");
    assert_eq!(held.scope(None), Scope::Unknown);
    assert_eq!(held.scope(Some(A)), Scope::Same);
    assert!(matches!(held.scope(Some(B)), Scope::Crossed(project) if project.root == A));
    assert!(held.scope(Some(B)).holds());
    assert!(!held.scope(Some(A)).holds());
    assert!(!held.scope(None).holds());
}

/// A run made before projects were recorded is read once, and only as a
/// reading; it holds nothing until its live coordinator confirms it from a
/// pane of that project — a pane that is not the seat, or another project,
/// confirms nothing — and the confirmation is written down with when and
/// from which seat. A stated project is never rewritten.
#[test]
fn an_inferred_project_is_confirmed_only_by_the_runs_live_coordinator_in_that_project() {
    let mut ledger = Ledger::new();
    let run = ledger.create_run("older", 1);
    ledger
        .seat_coordinator(&run, SEAT, None, 2)
        .expect("the seat takes its author");

    assert!(
        !ledger.infer_run_project(&run, A, ProjectSource::Created, None, 3),
        "a project was inferred as made"
    );
    assert!(
        !ledger.infer_run_project(&run, A, ProjectSource::Confirmed, Some(SEAT), 3),
        "a project was inferred as confirmed"
    );
    assert!(ledger.infer_run_project(&run, A, ProjectSource::Workers, None, 3));
    assert!(
        !ledger.infer_run_project(&run, B, ProjectSource::Seat, Some(SEAT), 4),
        "a second reading replaced the first"
    );
    let read = ledger.run(&run).expect("the run");
    assert!(read.project.as_ref().is_some_and(RunProject::inferred));
    assert!(
        matches!(read.scope(Some(B)), Scope::Crossed(_)) && read.scope(Some(B)).holds(),
        "an inferred project failed to hold the boundary"
    );

    assert!(
        !ledger.confirm_run_project(&run, A, "team-1/%2", 5),
        "a pane that is not the live seat confirmed"
    );
    assert!(
        !ledger.confirm_run_project(&run, B, SEAT, 5),
        "the seat confirmed a project the run was not read off as"
    );
    assert!(ledger.confirm_run_project(&run, A, SEAT, 6));
    assert_eq!(
        ledger.run(&run).and_then(|run| run.project.clone()),
        Some(RunProject {
            root: A.to_string(),
            source: ProjectSource::Confirmed,
            at_ms: 6,
            seat: Some(SEAT.to_string()),
        })
    );
    assert!(ledger.run(&run).expect("the run").scope(Some(B)).holds());

    assert!(!ledger.confirm_run_project(&run, A, SEAT, 7));
    assert!(!ledger.state_run_project(&run, B, "team-2/%1", 7));
    assert!(!ledger.infer_run_project(&run, B, ProjectSource::Workers, None, 7));

    // A seat that was vacated confirms nothing.
    let vacated = ledger.create_run("vacated", 8);
    assert!(ledger.infer_run_project(&vacated, A, ProjectSource::Seat, Some(SEAT), 9));
    assert!(
        !ledger.confirm_run_project(&vacated, A, SEAT, 10),
        "a run with no live seat was confirmed"
    );
    // And a root no run could hold is no root.
    assert!(!ledger.state_run_project(&vacated, "", SEAT, 10));
    let long = ledger.create_run("long", 11);
    assert!(!ledger.infer_run_project(
        &long,
        &"/".repeat(PROJECT_ROOT_MAX + 1),
        ProjectSource::Workers,
        None,
        11
    ));
}

/// The project rides the run through the ledger's projection and its own
/// serialized shape; a run written before projects reads back with none.
#[test]
fn a_runs_project_survives_the_projection_and_a_run_without_one_reads_back_without() {
    let mut ledger = Ledger::new();
    let made = ledger.create_run("made", 1);
    let older = ledger.create_run("older", 2);
    assert!(ledger.state_run_project(&made, A, SEAT, 3));
    let rebuilt = Ledger::rebuild(ledger.export()).expect("the projection rebuilds");
    assert_eq!(
        rebuilt.run(&made).and_then(|run| run.project.clone()),
        ledger.run(&made).and_then(|run| run.project.clone())
    );
    assert!(rebuilt.run(&older).is_some_and(|run| run.project.is_none()));

    let projected = rebuilt.export();
    let written = serde_json::to_value(&projected.runs[0]).expect("a row");
    assert_eq!(
        written["project"],
        serde_json::json!({ "root": A, "source": "created", "at_ms": 3, "seat": SEAT })
    );
    let mut before = written.clone();
    before
        .as_object_mut()
        .expect("a row object")
        .remove("project");
    let read: RunRow = serde_json::from_value(before).expect("a row written before projects");
    assert_eq!(read.project, None);
    let mut stranger = written;
    stranger["project"]["kind"] = serde_json::json!("folder");
    assert!(
        serde_json::from_value::<RunRow>(stranger).is_err(),
        "a project with a field this window does not know was read"
    );
}

/// The words a person reads: the repository's folder for a shared git
/// directory, whose run it is and where the pane works in a refusal, and no
/// path in a `dbg!`.
#[test]
fn a_project_is_said_by_its_repositorys_folder_and_never_printed_by_debug() {
    assert_eq!(label(A), "/Users/dev/a");
    assert_eq!(label("/Users/dev/notes"), "/Users/dev/notes");
    let project = RunProject {
        root: A.to_string(),
        source: ProjectSource::Created,
        at_ms: 1,
        seat: Some(SEAT.to_string()),
    };
    let said = refusal("check", "run-7", &project, B);
    assert!(
        said.contains(
            "run-7 belongs to the project at /Users/dev/a and this pane works in /Users/dev/b"
        ) && said.contains("`check`"),
        "{said}"
    );
    let printed = format!("{project:?}");
    assert!(
        !printed.contains("/Users/dev") && !printed.contains("team-1"),
        "{printed}"
    );
}
