//! t-131644 (F1 of the review t-131831): `line-notify` — `zo line add` binds
//! a build line job's end to the pane that puts the job in, before the job
//! runs, so the ledger can tell that end itself when a window restart leaves
//! the line no pane to tell it from. The ledger answers only for a job id, an
//! address the run reads at — a worker of the run, or its chair — and a pane
//! and a run of one known project; the answer is filed under the job's own
//! name, and nothing else is written.

use super::*;

const A: &str = "/Users/dev/a/.git";
const B: &str = "/Users/dev/b/.git";
/// Job ids as `zo line` makes them.
const JOB: &str = "j-1795663698796-00a1";
const OTHER_JOB: &str = "j-1795663698796-00a2";

/// One verb from `pane` of the bench's team, working in `project` — the
/// window's record of the pane's folder, `""` for a pane in none it knows —
/// with the receipt filed as the window files it.
fn from_project(bench: &mut Bench, pane: &str, project: &str, argv: &[&str]) -> Decided {
    bench.clock += 1;
    let actor = bench_actor(&bench.team.id, pane);
    let argv: Vec<String> = argv.iter().map(|word| (*word).to_string()).collect();
    let planned = plan_in_project(
        &mut bench.ledger,
        &mut bench.team,
        &bench.launcher,
        &argv,
        pane,
        bench.clock,
        Some(&actor),
        &crate::orchestration::evidence::ReceiptFiles::default(),
        None,
        Some(project),
    );
    bench.ledger.file_receipt(&planned, bench.clock);
    planned
}

/// `line-notify` for `job`'s end at `to` — in `run` when it names one — as
/// `zo line add` asks it, from `pane` working in `project`.
fn bind(
    bench: &mut Bench,
    pane: &str,
    project: &str,
    job: &str,
    to: &str,
    run: Option<&str>,
) -> Decided {
    let name = format!("zo-line-notify-{job}");
    let mut argv = vec![
        "line-notify",
        "--job",
        job,
        "--to",
        to,
        "--retry-request",
        name.as_str(),
    ];
    if let Some(run) = run {
        argv.extend(["--run", run]);
    }
    from_project(bench, pane, project, &argv)
}

/// A run its leader pane made working in `project` (`""`: in no folder the
/// window knows), and one worker summoned into it: the run, the worker and
/// the worker's pane.
fn a_run_of(bench: &mut Bench, project: &str, name: &str) -> (String, String, String) {
    let made = from_project(
        bench,
        agent_teams::LEADER_PANE,
        project,
        &["run-create", "--name", name, "--retry-request", name],
    );
    assert_eq!(made.reply.exit_code, 0, "{}", made.reply.stderr);
    let made: serde_json::Value = serde_json::from_str(&made.reply.stdout).expect("JSON");
    let run = made["runId"].as_str().expect("a run id").to_string();
    let (worker, pane) = bench.seat("worker-start --agent claude");
    (run, worker, pane)
}

/// How many letters `run` holds.
fn letters(bench: &Bench, run: &str) -> usize {
    bench.ledger.run(run).expect("the run").messages().len()
}

/// A worker's pane binds its job's end to itself, and the run's leader pane
/// binds another's to the chair, naming the run: each answer is the binding
/// — the job, the address, the run and the pane's project — filed under the
/// job's own name as the answer to `line-notify`, and no letter is written.
#[test]
fn an_end_is_bound_to_a_worker_or_the_chair_of_a_run_of_the_panes_own_project() {
    let mut bench = Bench::new();
    let (run, worker, pane) = a_run_of(&mut bench, A, "lines");
    let before = letters(&bench, &run);
    let to_worker = format!("worker:{worker}");
    let to_chair = format!("run:{run}");

    let bound = bind(&mut bench, &pane, A, JOB, &to_worker, None);
    assert_eq!(
        bound.reply.exit_code, 0,
        "the bind was refused: {}",
        bound.reply.stderr
    );
    let said: serde_json::Value = serde_json::from_str(&bound.reply.stdout).expect("JSON");
    assert_eq!(
        said,
        serde_json::json!({ "job": JOB, "to": to_worker, "run": run, "project": A })
    );
    let chaired = bind(
        &mut bench,
        agent_teams::LEADER_PANE,
        A,
        OTHER_JOB,
        &to_chair,
        Some(&run),
    );
    assert_eq!(
        chaired.reply.exit_code, 0,
        "the bind was refused: {}",
        chaired.reply.stderr
    );
    let said: serde_json::Value = serde_json::from_str(&chaired.reply.stdout).expect("JSON");
    assert_eq!(
        said,
        serde_json::json!({ "job": OTHER_JOB, "to": to_chair, "run": run, "project": A })
    );

    let actor = bench_actor(&bench.team.id, &pane);
    let filed = bench
        .ledger
        .already_served(&actor, &format!("zo-line-notify-{JOB}"))
        .expect("the answer is filed under the job's own name");
    assert_eq!(filed.verb.as_deref(), Some("line-notify"));
    assert_eq!(letters(&bench, &run), before, "a bind wrote a letter");
}

/// The ledger binds no end it could not tell, and files nothing for one: a
/// job that is no job id, a name that is not the job's own, an address no
/// worker or chair of the run reads at, a worker the run does not have, a
/// pane or a run whose project is not known, and a pane of another project.
#[test]
fn the_ledger_binds_no_end_it_could_not_tell_and_files_nothing_for_one() {
    let mut bench = Bench::new();
    let (run, worker, pane) = a_run_of(&mut bench, A, "lines");
    let (unplaced, _, _) = a_run_of(&mut bench, "", "unplaced");
    let to_worker = format!("worker:{worker}");
    let refused = |planned: &Decided, why: &str, what: &str| {
        assert_ne!(planned.reply.exit_code, 0, "{what} was bound");
        assert!(
            planned.reply.stderr.contains(why),
            "{what} was refused for another reason: {}",
            planned.reply.stderr
        );
    };

    let nameless = bind(&mut bench, &pane, A, "x-1", &to_worker, None);
    refused(&nameless, "job id", "a job that is no job id");

    let other_name = from_project(
        &mut bench,
        &pane,
        A,
        &[
            "line-notify",
            "--job",
            JOB,
            "--to",
            &to_worker,
            "--retry-request",
            "my-own-name",
        ],
    );
    refused(
        &other_name,
        &format!("zo-line-notify-{JOB}"),
        "a bind under a name that is not the job's",
    );

    let a_pane = bind(&mut bench, &pane, A, JOB, "pane:team-1/%9", None);
    refused(&a_pane, "a worker or a run's chair", "a pane's address");

    let another_chair = bind(
        &mut bench,
        agent_teams::LEADER_PANE,
        A,
        JOB,
        &format!("run:{unplaced}"),
        Some(&run),
    );
    refused(&another_chair, "another run", "another run's chair");

    let nobody = bind(&mut bench, &pane, A, JOB, "worker:w-404", None);
    refused(&nobody, "unknown worker", "a worker the run does not have");

    let unknown_pane = bind(&mut bench, &pane, "", JOB, &to_worker, None);
    refused(
        &unknown_pane,
        "project",
        "a pane in no folder the window knows",
    );

    let unknown_run = bind(
        &mut bench,
        agent_teams::LEADER_PANE,
        A,
        JOB,
        &format!("run:{unplaced}"),
        Some(&unplaced),
    );
    refused(
        &unknown_run,
        "project",
        "a run whose project is not written down",
    );

    let crossing = bind(&mut bench, &pane, B, JOB, &to_worker, None);
    refused(&crossing, "project", "a pane of another project");

    let actor = bench_actor(&bench.team.id, &pane);
    let leader = bench_actor(&bench.team.id, agent_teams::LEADER_PANE);
    for (who, name) in [
        (&actor, format!("zo-line-notify-{JOB}")),
        (&leader, format!("zo-line-notify-{JOB}")),
        (&actor, "zo-line-notify-x-1".to_string()),
        (&actor, "my-own-name".to_string()),
    ] {
        assert!(
            bench.ledger.already_served(who, &name).is_none(),
            "a refused bind was filed under {name}"
        );
    }
}
