//! t-65286: every project a window opens shares one ledger, and a run is one
//! project's. A pane of another project is not pointed at the run's mail and
//! opens none of its workers' checkouts; a verb it sends into the run is
//! written down as one the boundary would refuse, and refused once the person
//! turns the boundary on. Two repositories stand in for two projects, and
//! three checkouts of the first — the main checkout, a worktree beside it and
//! one under the workspaces folder — for one project in three folders.
//!
//! Every scene here is driven through the production doors and read through
//! what a person reads: the answers, the keystrokes, the window's black box.

use super::*;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// The file the window reads the boundary's switch from, under its data root,
/// and the word in it that turns the boundary on.
const SWITCH: &str = "orchestration-project-scope.json";
const ENFORCE: &str = "\"enforce\"";

/// Two projects on disk, laid out the way git lays them out: a main checkout
/// whose `.git` is the shared directory, linked worktrees whose `.git` file
/// points into `worktrees/<name>` under it, and a second repository.
struct Projects {
    _root: tempfile::TempDir,
    /// Project A's main checkout.
    a: PathBuf,
    /// A worktree of A beside it, the way `<repository>-worktrees/<name>` is cut.
    a_beside: PathBuf,
    /// A worktree of A under the workspaces folder, the way a task's is cut.
    a_task: PathBuf,
    /// Project B's checkout.
    b: PathBuf,
}

impl Projects {
    fn lay_out() -> Self {
        let root = tempfile::tempdir().expect("a folder for two projects");
        let a = root.path().join("2026").join("zerocode");
        let shared = a.join(".git");
        std::fs::create_dir_all(&shared).expect("project A's repository");
        let linked = |name: &str, at: &Path| {
            let own = shared.join("worktrees").join(name);
            std::fs::create_dir_all(&own).expect("the worktree's own git directory");
            std::fs::write(own.join("commondir"), "../..\n").expect("its common pointer");
            std::fs::create_dir_all(at).expect("the worktree");
            std::fs::write(at.join(".git"), format!("gitdir: {}\n", own.display()))
                .expect("its .git pointer");
        };
        let a_beside = root
            .path()
            .join("2026")
            .join("zerocode-worktrees")
            .join("zo-l32");
        linked("zo-l32", &a_beside);
        let a_task = root
            .path()
            .join("zerocode")
            .join("workspaces")
            .join("zerocode")
            .join("t-65286");
        linked("t-65286", &a_task);
        let b = root.path().join("acme");
        std::fs::create_dir_all(b.join(".git")).expect("project B's repository");
        Self {
            _root: root,
            a,
            a_beside,
            a_task,
            b,
        }
    }

    /// The root a run of project A is written down under: the shared git
    /// directory, canonical.
    fn a_root(&self) -> String {
        canonical(&self.a.join(".git"))
    }

    fn b_root(&self) -> String {
        canonical(&self.b.join(".git"))
    }
}

fn canonical(path: &Path) -> String {
    std::fs::canonicalize(path)
        .expect("a folder that is there")
        .to_string_lossy()
        .into_owned()
}

/// A window whose panes sit in the folders a test puts them in. Every
/// keystroke is recorded, and a split opens the next worker terminal.
struct Placed {
    folders: Mutex<HashMap<u32, PathBuf>>,
    sends: Mutex<Vec<(u32, String)>>,
    next: std::sync::atomic::AtomicU32,
    spawned: std::sync::atomic::AtomicUsize,
}

impl Placed {
    fn new(first_worker: u32) -> Self {
        Self {
            folders: Mutex::new(HashMap::new()),
            sends: Mutex::new(Vec::new()),
            next: std::sync::atomic::AtomicU32::new(first_worker),
            spawned: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    /// The pane at `term` works in `folder` from now on.
    fn place(&self, term: u32, folder: &Path) {
        self.folders
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .insert(term, folder.to_path_buf());
    }

    /// How many mail pointers were typed at `term`.
    fn pointed_at(&self, term: u32) -> usize {
        self.sends
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .iter()
            .filter(|(at, text)| *at == term && zerocode_core::orchestration::is_pointer_text(text))
            .count()
    }

    fn spawned(&self) -> usize {
        self.spawned.load(std::sync::atomic::Ordering::SeqCst)
    }
}

impl Host for Placed {
    fn split(
        &self,
        _team: &str,
        _leader_term: u32,
        _from_term: u32,
        _pane: &str,
        _direction: zerocode_core::agent_teams::Direction,
        _command: &str,
        _token: &str,
    ) -> Option<u32> {
        self.spawned
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Some(self.next.fetch_add(1, std::sync::atomic::Ordering::SeqCst))
    }
    fn send(&self, term: u32, text: &str) -> bool {
        self.sends
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .push((term, text.to_string()));
        true
    }
    fn capture(&self, _term: u32) -> Option<String> {
        Some(String::new())
    }
    fn focus(&self, _term: u32) -> bool {
        true
    }
    fn close(&self, _term: u32) {}
    fn actor_for(&self, term: u32) -> Option<String> {
        Some(test_actor(term))
    }
    fn worktree_of(&self, term: u32) -> Option<PathBuf> {
        self.folders
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .get(&term)
            .cloned()
    }
}

/// A leader pane at `term`, of a team of its own, working in `folder` — or
/// in no folder the window recorded.
fn a_leader(host: &Placed, team: &str, term: u32, folder: Option<&Path>) {
    seat_a_team(team, term);
    if let Some(folder) = folder {
        host.place(term, folder);
    }
}

/// One verb from the leader pane of `team`, as the shim sends it.
fn from_leader(host: &Placed, team: &str, line: &str) -> zerocode_hookd::TeamAnswer {
    run(
        host,
        Vec::new(),
        team,
        zerocode_core::agent_teams::LEADER_PANE,
        TEST_CAPABILITY,
        &words(line),
        clock(),
    )
}

/// One verb from a worker's own pane, with the capability its split minted.
fn from_worker(host: &Placed, team: &str, pane: &str, line: &str) -> zerocode_hookd::TeamAnswer {
    let held = crate::agent_teams::current_pane_capability(team, pane)
        .expect("the split minted the worker a capability");
    run(host, Vec::new(), team, pane, &held, &words(line), clock())
}

/// The JSON a verb from the leader pane of `team` answered.
fn json_from_leader(host: &Placed, team: &str, line: &str) -> serde_json::Value {
    let answered = from_leader(host, team, line);
    assert_eq!(answered.exit_code, 0, "`{line}`: {}", answered.stderr);
    serde_json::from_str(&answered.stdout).expect("a JSON answer")
}

/// A run opened from the leader pane of `team`.
fn a_run_from(host: &Placed, team: &str, name: &str) -> String {
    json_from_leader(host, team, &format!("run-create --name {name}"))["runId"]
        .as_str()
        .expect("a run id")
        .to_string()
}

/// The project `run-show` says a run belongs to.
fn shown_project(host: &Placed, team: &str, run_id: &str) -> serde_json::Value {
    json_from_leader(host, team, &format!("run-show --run {run_id}"))["project"].clone()
}

/// The window log's lines that carry every one of `needles`.
fn logged(needles: &[&str]) -> Vec<String> {
    let blackbox = super::super::BLACKBOX
        .get()
        .expect("the bench window's black box")
        .join("window-errors.log");
    std::fs::read_to_string(blackbox)
        .unwrap_or_default()
        .lines()
        .filter(|line| needles.iter().all(|needle| line.contains(needle)))
        .map(str::to_string)
        .collect()
}

/// The boundary turned on for as long as this lives, and off again after —
/// written where the window reads it, the way a person turns it on.
struct BoundaryOn;

impl BoundaryOn {
    fn turn() -> Self {
        let root = super::super::BLACKBOX
            .get()
            .expect("the bench window's data root");
        std::fs::write(root.join(SWITCH), ENFORCE).expect("the switch");
        Self
    }
}

impl Drop for BoundaryOn {
    fn drop(&mut self) {
        if let Some(root) = super::super::BLACKBOX.get() {
            let _ = std::fs::remove_file(root.join(SWITCH));
        }
    }
}

/// A run is written down as the project of the pane that made it: the shared
/// git directory, so a run made in the main checkout, in a worktree beside it
/// and in one under the workspaces folder are one project, and a run made in
/// another repository is another. `run-show` says how it was learned, when
/// and from which seat.
#[test]
fn a_run_is_the_project_of_the_pane_that_made_it() {
    const A: u32 = 65_200;
    const A_BESIDE: u32 = 65_201;
    const A_TASK: u32 = 65_202;
    const B: u32 = 65_203;
    let _window = the_window();
    let disk = Projects::lay_out();
    let host = Placed::new(65_210);
    let leader = zerocode_core::agent_teams::LEADER_PANE;
    let teams: Vec<(String, u32, &Path)> = vec![
        (format!("team-project-made-a-{A}"), A, disk.a.as_path()),
        (
            format!("team-project-made-beside-{A_BESIDE}"),
            A_BESIDE,
            disk.a_beside.as_path(),
        ),
        (
            format!("team-project-made-task-{A_TASK}"),
            A_TASK,
            disk.a_task.as_path(),
        ),
        (format!("team-project-made-b-{B}"), B, disk.b.as_path()),
    ];
    let mut roots = Vec::new();
    for (team, term, folder) in &teams {
        a_leader(&host, team, *term, Some(*folder));
        let opened = json_from_leader(&host, team, "run-create --name made");
        let run_id = opened["runId"].as_str().expect("a run id").to_string();
        let shown = shown_project(&host, team, &run_id);
        assert_eq!(
            shown["source"], "created",
            "run-create did not write the project of the pane that made {run_id}: {shown}"
        );
        assert_eq!(shown["seat"], format!("{team}/{leader}"), "{shown}");
        roots.push(shown["root"].as_str().expect("a root").to_string());
        assert_eq!(
            opened["project"], shown["folder"],
            "run-create's answer names another folder than run-show: {opened}"
        );
    }
    assert_eq!(
        roots,
        vec![disk.a_root(), disk.a_root(), disk.a_root(), disk.b_root()],
        "the main checkout, its worktrees and another repository were not read as two projects"
    );
    assert_eq!(
        shown_project(&host, &teams[0].0, &a_run_from(&host, &teams[0].0, "again"))["folder"],
        canonical(&disk.a),
        "a project is named by its repository's folder"
    );

    for (_, term, _) in &teams {
        crate::agent_teams::forget_term(*term);
    }
}

/// The case the person saw: a pane of project B holds the seat of a run that
/// project A made, and mail for the run's coordinator lands. Before t-65286
/// the pointer was typed into B's pane, where another project's agent read
/// it. Now nothing is typed there, the black box says why once, and the mail
/// is still `check`'s to read.
#[test]
fn a_runs_mail_is_not_pointed_at_a_pane_of_another_project_holding_its_seat() {
    const A: u32 = 65_220;
    const B: u32 = 65_221;
    let _window = the_window();
    let _turn = one_beat_at_a_time();
    let disk = Projects::lay_out();
    let host = Placed::new(65_230);
    let leader = zerocode_core::agent_teams::LEADER_PANE;
    let team_a = format!("team-project-pointer-a-{A}");
    let team_b = format!("team-project-pointer-b-{B}");
    a_leader(&host, &team_a, A, Some(&disk.a));
    a_leader(&host, &team_b, B, Some(&disk.b));
    let run_id = a_run_from(&host, &team_a, "pointed");

    // B takes the seat: a verb into another project's run, carried out while
    // the boundary only writes such verbs down.
    let took = from_leader(
        &host,
        &team_b,
        &format!("run-takeover --run {run_id} --from {team_a}/{leader} --reason scene"),
    );
    assert_eq!(took.exit_code, 0, "{}", took.stderr);

    // A writes to the run's coordinator, and B's pane is at rest.
    let sent = from_leader(
        &host,
        &team_a,
        &format!("send --run {run_id} --to run:{run_id} --type status --body landed"),
    );
    assert_eq!(sent.exit_code, 0, "{}", sent.stderr);
    super::super::pane_turn_ended(B, 50, false, clock());
    super::super::tick(&host, &[], clock());
    super::super::tick(&host, &[], clock());

    assert_eq!(
        host.pointed_at(B),
        0,
        "{run_id}'s mail was pointed at terminal {B}, a pane of project B"
    );
    assert_eq!(
        logged(&[&run_id, &format!("not pointed at terminal {B}")]).len(),
        1,
        "the black box did not say once why {run_id}'s mail was not pointed at terminal {B}"
    );
    let read = json_from_leader(&host, &team_b, &format!("check --run {run_id} --peek"));
    assert!(
        read["messages"]
            .as_array()
            .expect("a delivery")
            .iter()
            .any(|message| message["body"] == "landed"),
        "the seat's mail is no longer check's: {read}"
    );

    super::super::pane_turn_began(B, clock());
    crate::agent_teams::forget_term(A);
    crate::agent_teams::forget_term(B);
}

/// A pane of project B reads and writes a run of project A with `--run`.
/// While the boundary only writes such verbs down, each is carried out and
/// the black box names the run, the pane, both projects and the verb. Once
/// the person turns the boundary on, the same verbs are refused in words
/// that say whose run it is, and B's `inbox` and `run-list` no longer show
/// A's run.
#[test]
fn a_verb_from_another_project_is_written_down_and_refused_once_the_boundary_is_on() {
    const A: u32 = 65_240;
    const B: u32 = 65_241;
    let _window = the_window();
    let disk = Projects::lay_out();
    let host = Placed::new(65_250);
    let team_a = format!("team-project-verb-a-{A}");
    let team_b = format!("team-project-verb-b-{B}");
    a_leader(&host, &team_a, A, Some(&disk.a));
    a_leader(&host, &team_b, B, Some(&disk.b));
    let run_id = a_run_from(&host, &team_a, "guarded");

    let read = from_leader(&host, &team_b, &format!("check --run {run_id}"));
    assert_eq!(read.exit_code, 0, "{}", read.stderr);
    assert_eq!(
        logged(&[
            &format!("`check` from terminal {B}"),
            &format!("into {run_id} of"),
            &canonical(&disk.a),
            &canonical(&disk.b),
            "would be refused once",
        ])
        .len(),
        1,
        "B's `check --run {run_id}` was not written down as a verb the boundary would refuse"
    );
    let wrote = from_leader(
        &host,
        &team_b,
        &format!("send --run {run_id} --to run:{run_id} --type status --body across"),
    );
    assert_eq!(wrote.exit_code, 0, "{}", wrote.stderr);

    let on = BoundaryOn::turn();
    let refused = from_leader(&host, &team_b, &format!("check --run {run_id}"));
    assert_eq!(
        refused.exit_code, 1,
        "B's `check --run {run_id}` was answered with the boundary on: {}",
        refused.stdout
    );
    assert!(
        refused.stderr.contains(&format!(
            "{run_id} belongs to the project at {}",
            canonical(&disk.a)
        )) && refused.stderr.contains("`check`"),
        "the refusal does not say whose run it is: {}",
        refused.stderr
    );
    for line in [
        format!("task-create --run {run_id} --spec across"),
        format!("send --run {run_id} --to run:{run_id} --type status --body across"),
        format!("task-list --run {run_id}"),
    ] {
        let answered = from_leader(&host, &team_b, &line);
        assert_eq!(
            answered.exit_code, 1,
            "`{line}` from project B was answered with the boundary on"
        );
    }
    let inbox = json_from_leader(&host, &team_b, "inbox --limit 100");
    assert!(
        inbox["messages"]
            .as_array()
            .expect("an inbox")
            .iter()
            .all(|message| message["runId"] != run_id.as_str()),
        "B's inbox shows {run_id}'s mail with the boundary on: {inbox}"
    );
    let runs = json_from_leader(&host, &team_b, "run-list");
    assert!(
        runs["runs"]
            .as_array()
            .expect("a run list")
            .iter()
            .all(|listed| listed["runId"] != run_id.as_str()),
        "B's run-list shows {run_id} with the boundary on: {runs}"
    );
    // The run's own project is untouched by the boundary.
    let own = from_leader(&host, &team_a, &format!("check --run {run_id}"));
    assert_eq!(own.exit_code, 0, "{}", own.stderr);
    drop(on);

    crate::agent_teams::forget_term(A);
    crate::agent_teams::forget_term(B);
}

/// A worker works in its run's project. A leader pane of project B that
/// summons a worker into a run project A made would open it in B's checkout:
/// the summons is refused, nothing is split, and the black box says why.
#[test]
fn a_worker_is_not_opened_in_another_projects_checkout() {
    const A: u32 = 65_260;
    const B: u32 = 65_261;
    let _window = the_window();
    let disk = Projects::lay_out();
    let host = Placed::new(65_270);
    let team_a = format!("team-project-worker-a-{A}");
    let team_b = format!("team-project-worker-b-{B}");
    a_leader(&host, &team_a, A, Some(&disk.a));
    a_leader(&host, &team_b, B, Some(&disk.b));
    let run_id = a_run_from(&host, &team_a, "summoned");
    let task_of = |spec: &str| {
        json_from_leader(&host, &team_a, &format!("task-create --spec {spec}"))["taskId"]
            .as_str()
            .expect("a task")
            .to_string()
    };
    let task = task_of("across");
    let own_task = task_of("own");

    let summoned = from_leader(
        &host,
        &team_b,
        &format!("worker-start --run {run_id} --agent claude --task {task}"),
    );
    assert_eq!(
        (summoned.exit_code, host.spawned()),
        (1, 0),
        "a worker of {run_id} was opened in project B's checkout: {}",
        summoned.stdout
    );
    assert!(
        summoned
            .stderr
            .contains("a worker's checkout is opened only in its run's project"),
        "{}",
        summoned.stderr
    );
    assert_eq!(
        logged(&[&run_id, "was not opened in", &canonical(&disk.b)]).len(),
        1,
        "the black box did not say why the worker was not opened"
    );

    // The run's own leader summons as before.
    let own = from_leader(
        &host,
        &team_a,
        &format!("worker-start --agent claude --task {own_task}"),
    );
    assert_eq!(own.exit_code, 0, "{}", own.stderr);
    assert_eq!(host.spawned(), 1);

    crate::agent_teams::forget_term(A);
    crate::agent_teams::forget_term(B);
}

/// One project in three folders: a pane in a worktree of A beside its main
/// checkout holds the seat of a run the main checkout made, and is pointed at
/// its mail as before; its verbs into that run are answered with the boundary
/// on, and nothing is written down about them.
#[test]
fn a_pane_in_another_worktree_of_the_same_repository_is_the_same_project() {
    const A: u32 = 65_280;
    const A_BESIDE: u32 = 65_281;
    let _window = the_window();
    let _turn = one_beat_at_a_time();
    let disk = Projects::lay_out();
    let host = Placed::new(65_290);
    let leader = zerocode_core::agent_teams::LEADER_PANE;
    let team_a = format!("team-project-same-a-{A}");
    let team_beside = format!("team-project-same-beside-{A_BESIDE}");
    a_leader(&host, &team_a, A, Some(&disk.a));
    a_leader(&host, &team_beside, A_BESIDE, Some(&disk.a_beside));
    let run_id = a_run_from(&host, &team_a, "one-project");
    let _on = BoundaryOn::turn();

    let took = from_leader(
        &host,
        &team_beside,
        &format!("run-takeover --run {run_id} --from {team_a}/{leader} --reason scene"),
    );
    assert_eq!(took.exit_code, 0, "{}", took.stderr);
    let sent = from_leader(
        &host,
        &team_a,
        &format!("send --run {run_id} --to run:{run_id} --type status --body landed"),
    );
    assert_eq!(sent.exit_code, 0, "{}", sent.stderr);
    super::super::pane_turn_ended(A_BESIDE, 50, false, clock());
    super::super::tick(&host, &[], clock());
    super::super::tick(&host, &[], clock());
    assert_eq!(
        host.pointed_at(A_BESIDE),
        1,
        "a worktree of the run's own repository was not pointed at its mail"
    );
    let read = from_leader(&host, &team_beside, &format!("check --run {run_id}"));
    assert_eq!(read.exit_code, 0, "{}", read.stderr);
    assert!(
        logged(&[&format!("into {run_id} of")]).is_empty(),
        "a verb within one project was written down as a crossing"
    );

    super::super::pane_turn_began(A_BESIDE, clock());
    crate::agent_teams::forget_term(A);
    crate::agent_teams::forget_term(A_BESIDE);
}

/// Two runs of one project still write to each other with the boundary on:
/// a question sent across with `--run`, read and answered in the other run,
/// and the answer read back where it was asked.
#[test]
fn two_runs_of_one_project_still_send_and_reply_with_the_boundary_on() {
    const A: u32 = 65_300;
    const A_TASK: u32 = 65_301;
    let _window = the_window();
    let disk = Projects::lay_out();
    let host = Placed::new(65_310);
    let team_one = format!("team-project-two-runs-one-{A}");
    let team_two = format!("team-project-two-runs-two-{A_TASK}");
    a_leader(&host, &team_one, A, Some(&disk.a));
    a_leader(&host, &team_two, A_TASK, Some(&disk.a_task));
    let one = a_run_from(&host, &team_one, "first");
    let two = a_run_from(&host, &team_two, "second");
    let _on = BoundaryOn::turn();

    let asked = json_from_leader(
        &host,
        &team_one,
        &format!("send --run {two} --to run:{two} --type question --body which"),
    );
    let question = asked["messageId"]
        .as_str()
        .expect("a message id")
        .to_string();
    let read = json_from_leader(&host, &team_two, "check");
    assert!(
        read["messages"]
            .as_array()
            .expect("a delivery")
            .iter()
            .any(|message| message["messageId"] == question.as_str()),
        "{two}'s coordinator did not read the question from {one}: {read}"
    );
    let answered = from_leader(
        &host,
        &team_two,
        &format!("reply --to-message {question} --body this-one"),
    );
    assert_eq!(answered.exit_code, 0, "{}", answered.stderr);
    // The answer reaches the asker in its own run's inbox, or in the inbox
    // its pane has in the run that answered.
    let answered_in = |line: &str| {
        json_from_leader(&host, &team_one, line)["messages"]
            .as_array()
            .is_some_and(|messages| messages.iter().any(|message| message["body"] == "this-one"))
    };
    assert!(
        answered_in("check") || answered_in(&format!("check --run {two}")),
        "the answer from {two} did not come back to {one}'s coordinator"
    );
    assert!(
        logged(&[&format!("into {one} of")]).is_empty()
            && logged(&[&format!("into {two} of")]).is_empty(),
        "a verb between two runs of one project was written down as a crossing"
    );

    crate::agent_teams::forget_term(A);
    crate::agent_teams::forget_term(A_TASK);
}

/// A run made before projects were recorded — here, made from a pane the
/// window recorded no folder for — is read off its workers' checkouts on the
/// beat: the project most of them belong to, every worktree of one
/// repository counted as one. It is written down as inferred, a worker's
/// verb does not confirm it, and the run's own coordinator does, by a verb
/// typed in its pane in that project — when, and from which seat, written
/// down beside it.
#[test]
fn an_older_run_is_read_off_its_workers_and_confirmed_by_its_coordinator() {
    const LEADER: u32 = 65_320;
    let _window = the_window();
    let _turn = one_beat_at_a_time();
    let disk = Projects::lay_out();
    let host = Placed::new(65_330);
    let leader = zerocode_core::agent_teams::LEADER_PANE;
    let team = format!("team-project-older-{LEADER}");
    a_leader(&host, &team, LEADER, None);
    let run_id = a_run_from(&host, &team, "older");
    assert!(
        shown_project(&host, &team, &run_id).is_null(),
        "a run made in no known folder was given a project"
    );

    // Four workers, seated in A's three folders and in B's.
    let held = super::super::runtime().expect("the bench window's runtime");
    let mut workers = Vec::new();
    for (at, folder) in [&disk.a, &disk.a_beside, &disk.a_task, &disk.b]
        .into_iter()
        .enumerate()
    {
        let task =
            json_from_leader(&host, &team, &format!("task-create --spec part-{at}"))["taskId"]
                .as_str()
                .expect("a task")
                .to_string();
        let started = json_from_leader(
            &host,
            &team,
            &format!("worker-start --agent claude --task {task}"),
        );
        let pane = started["pane"].as_str().expect("a pane").to_string();
        held.actor
            .worker_seated(&team, &pane, folder.to_string_lossy().into_owned(), clock())
            .expect("the seat report lands");
        // Read here, with nothing else held: the table's lock is not taken
        // twice.
        let term = crate::agent_teams::teams()
            .get(&team)
            .and_then(|held| held.term_of(&pane))
            .expect("the worker's terminal");
        workers.push((pane, term));
    }

    super::super::tick(&host, &[], clock());
    let read = shown_project(&host, &team, &run_id);
    assert_eq!(
        (read["source"].clone(), read["root"].clone()),
        (
            serde_json::Value::from("workers"),
            serde_json::Value::from(disk.a_root())
        ),
        "{run_id} was not read off the project most of its workers' checkouts belong to: {read}"
    );

    // A worker's own verb confirms nothing, whatever folder its pane is in.
    let (pane, worker_term) = &workers[2];
    host.place(*worker_term, &disk.a_task);
    let spoke = from_worker(&host, &team, pane, "task-list");
    assert_eq!(spoke.exit_code, 0, "{}", spoke.stderr);
    assert_eq!(
        shown_project(&host, &team, &run_id)["source"],
        "workers",
        "a worker's verb confirmed its run's project"
    );

    // The run's coordinator, in a pane of that project, confirms it.
    host.place(LEADER, &disk.a_beside);
    let confirmed = shown_project(&host, &team, &run_id);
    let now = shown_project(&host, &team, &run_id);
    assert_eq!(
        (
            now["source"].clone(),
            now["seat"].clone(),
            now["root"].clone()
        ),
        (
            serde_json::Value::from("confirmed"),
            serde_json::Value::from(format!("{team}/{leader}")),
            serde_json::Value::from(disk.a_root())
        ),
        "the coordinator's own verb did not confirm {run_id}'s project: {confirmed} then {now}"
    );
    assert!(
        now["atMs"].as_i64().is_some_and(|at| at > 0),
        "the confirmation says no time: {now}"
    );

    crate::agent_teams::forget_term(LEADER);
    for (_, term) in workers {
        crate::agent_teams::forget_term(term);
    }
}
