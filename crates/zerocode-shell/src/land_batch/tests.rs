//! t-45315 (T3 stage 6): the integration queue's window half, run against real temporary
//! repositories.
//!
//! Every road below runs `git` for real in a temporary repository and an app-data folder of its
//! own. The ledger's records are read from the channel the window's roads feed, so no ledger is
//! needed; land-check's own roads run against a store in the same app-data folder.

use super::*;
use std::sync::mpsc::Receiver;
use zerocode_core::orchestration::land_batch::{
    Culprit, PreparedGate, Promotion, Search, Stack, Stacking, Standing, TipGate, person_press,
};

const RUN: &str = "run-1";

/// The task the ledger opened for a batch; the window writes its gate letters against it.
const BATCH_TASK: &str = "t-90";

/// The time of every press below, in the ledger's milliseconds.
const NOW: i64 = 1_791_940_000_000;

/// How long a test waits for a record the window writes in the background.
const RECORD_WAIT: Duration = Duration::from_secs(60);

fn git(dir: &Path, args: &[&str]) -> String {
    let output = lock_free_git(crate::proc::quiet_command("git"))
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .expect("git");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// Whether git says yes: `merge-base --is-ancestor` answers with its exit code alone.
fn git_says(dir: &Path, args: &[&str]) -> bool {
    lock_free_git(crate::proc::quiet_command("git"))
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .expect("git")
        .status
        .success()
}

fn commit_file(repo: &Path, name: &str, text: &str, message: &str) -> String {
    fs::write(repo.join(name), text).expect("a file");
    git(repo, &["add", "."]);
    git(repo, &["commit", "-q", "-m", message]);
    git(repo, &["rev-parse", "HEAD"])
}

/// A repository whose `main` holds `a.txt`, with one branch per member cut from it: member k adds
/// `f<k>.txt`, and each member named in `clashing` also writes `shared.txt` its own way. Every
/// member's landing message waits in `messages/<task>.txt`.
struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    repo: PathBuf,
    data: PathBuf,
    messages: PathBuf,
    main: String,
    members: Vec<Member>,
}

impl Fixture {
    fn new(size: usize, clashing: &[usize]) -> Self {
        let temp = tempfile::tempdir().expect("a temporary folder");
        let root = fs::canonicalize(temp.path()).expect("a canonical folder");
        let repo = root.join("repo");
        fs::create_dir_all(&repo).expect("the repository folder");
        git(&repo, &["init", "-q", "-b", "main"]);
        git(&repo, &["config", "user.name", "ZeroCode Test"]);
        git(&repo, &["config", "user.email", "test@zerocode"]);
        let main = commit_file(&repo, "a.txt", "one\n", "first");
        let messages = root.join("messages");
        fs::create_dir_all(&messages).expect("the message folder");
        let mut members = Vec::new();
        for k in 1..=size {
            let task = format!("t-{k}");
            git(&repo, &["checkout", "-q", "-b", &task, &main]);
            let mut head = commit_file(&repo, &format!("f{k}.txt"), "member\n", &task);
            if clashing.contains(&k) {
                head = commit_file(&repo, "shared.txt", &format!("member {k}\n"), "shared");
            }
            git(&repo, &["checkout", "-q", "main"]);
            fs::write(
                messages.join(format!("{task}.txt")),
                format!("Land {task}: add f{k}.txt\n\nWhy {task} lands.\n"),
            )
            .expect("a landing message");
            members.push(Member { task, head });
        }
        Self {
            _temp: temp,
            data: root.join("app-data"),
            root,
            repo,
            messages,
            main,
            members,
        }
    }

    fn rev(&self, word: &str) -> String {
        git(&self.repo, &["rev-parse", word])
    }

    /// Whether a ref exists at all.
    fn has(&self, name: &str) -> bool {
        git_says(&self.repo, &["rev-parse", "--verify", "--quiet", name])
    }

    /// The names under one of land-check's folders.
    fn land_check(&self, folder: &str) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(self.data.join("land-check").join(folder))
            .map(|entries| {
                entries
                    .flatten()
                    .map(|entry| entry.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }
}

/// The window in a test: the ledger's records and land-check's letters land in channels, in the
/// order they were written, and land-check's roads keep their store under the fixture's app data.
struct Window {
    roads: Roads,
    records: Receiver<LandBatchReceipt>,
    letters: Receiver<LandCheckReceipt>,
}

fn window(fixture: &Fixture) -> Window {
    let (record_tx, records) = mpsc::channel();
    let (letter_tx, letters) = mpsc::channel();
    let store = crate::land_check::Store::at(&fixture.data).expect("the land-check store");
    let quiet_store = store.clone();
    let letter_sink: crate::land_check::Sink = Arc::new(move |receipt: LandCheckReceipt| {
        let _ = letter_tx.send(receipt);
    });
    let no_letter: crate::land_check::Sink = Arc::new(|_: LandCheckReceipt| {});
    let check_policy = |_: &Path| crate::land_check::Policy::from_settings(None, None, None);
    Window {
        roads: Roads {
            ledger: Arc::new(move |receipt: LandBatchReceipt| {
                let _ = record_tx.send(receipt);
                Ok(())
            }),
            check: Arc::new(move |ask: &LandCheckAsk, now_ms: i64| {
                crate::land_check::run(&store, &check_policy, ask, now_ms, &letter_sink)
            }),
            quiet: Arc::new(move |ask: &LandCheckAsk, now_ms: i64| {
                crate::land_check::run(&quiet_store, &check_policy, ask, now_ms, &no_letter)
            }),
            folder: fixture.data.join("land-batch"),
        },
        records,
        letters,
    }
}

impl Window {
    fn next(&self) -> LandBatchReceipt {
        self.records
            .recv_timeout(RECORD_WAIT)
            .expect("the window wrote a record")
    }

    fn quiet(&self) -> bool {
        self.records.try_recv().is_err()
    }
}

fn on() -> Policy {
    Policy {
        enabled: true,
        pinned_base: None,
        git_budget: GIT_BUDGET,
        promote_budget: PROMOTE_GIT_BUDGET,
        git: PathBuf::from("git"),
    }
}

fn coordinator() -> PressedBy {
    PressedBy::Coordinator {
        run: RUN.to_string(),
        seat: "seat-1".to_string(),
        generation: 3,
    }
}

fn ask(by: PressedBy, action: Action) -> LandBatchAsk {
    LandBatchAsk {
        run: RUN.to_string(),
        by,
        action,
    }
}

fn choose(fixture: &Fixture) -> Action {
    Action::Choose {
        members: fixture.members.clone(),
        checkout: fixture.repo.display().to_string(),
        message_dir: fixture.messages.display().to_string(),
    }
}

/// The answer the verb printed, read as JSON, or a panic that says what was refused.
fn said(answer: Result<String, String>) -> serde_json::Value {
    let text = answer.unwrap_or_else(|why| panic!("the press was refused: {why}"));
    serde_json::from_str(&text).unwrap_or_else(|_| panic!("the answer is not JSON: {text}"))
}

fn press(window: &Window, by: PressedBy, action: Action) -> Result<String, String> {
    run(&|_| on(), &ask(by, action), &[], NOW, &window.roads)
}

fn phases(records: &[LandBatchReceipt]) -> Vec<&'static str> {
    records.iter().map(|one| one.record.phase()).collect()
}

/// The batch the ledger would hold after these records: the window's input for the next press.
/// Only the phases these tests write before a press are read.
fn state_of(records: &[LandBatchReceipt]) -> BatchState {
    let mut state: Option<BatchState> = None;
    for receipt in records {
        if let Record::Chosen {
            members,
            base_branch,
            base,
            repository,
        } = &receipt.record
        {
            state = Some(BatchState {
                batch: receipt.batch.clone(),
                task: BATCH_TASK.to_string(),
                generation: receipt.generation,
                members: members.clone(),
                base_branch: base_branch.clone(),
                base: base.clone(),
                repository: repository.clone(),
                chosen_by: receipt.by.clone(),
                chosen_at: NOW,
                operations: Vec::new(),
                dropped: Vec::new(),
                stacking: None,
                stacked: None,
                stack_halt: None,
                stack_abandoned: false,
                prepared: None,
                tip_gate: None,
                search: None,
                culprit: None,
                promoting: None,
                promote_halt: None,
                promotion: None,
                cancelled: None,
                merged_recorded: false,
                swept: false,
                standing: Standing::Chosen,
                last_at: NOW,
            });
            continue;
        }
        let held = state.as_mut().expect("a batch is chosen first");
        match &receipt.record {
            Record::Stacking {
                operation,
                lease_until_ms,
            } => {
                held.operations.push(operation.clone());
                held.stacking = Some(Stacking {
                    operation: operation.clone(),
                    lease_until_ms: *lease_until_ms,
                    by: receipt.by.clone(),
                });
                held.standing = Standing::Stacking;
            }
            Record::Stacked {
                steps,
                tip,
                tree,
                took_ms,
                overlaps,
            } => {
                held.stacked = Some(Stack {
                    steps: steps.clone(),
                    tip: tip.clone(),
                    tree: tree.clone(),
                    took_ms: *took_ms,
                    overlaps: *overlaps,
                });
                held.standing = Standing::Stacked;
            }
            Record::GatePrepared {
                check,
                step,
                target,
                tree,
                folder,
                log,
            } => {
                held.prepared = Some(PreparedGate {
                    check: check.clone(),
                    step: *step,
                    target: target.clone(),
                    tree: tree.clone(),
                    folder: folder.clone(),
                    log: log.clone(),
                });
                held.standing = Standing::Gating;
            }
            Record::Gated {
                check,
                step,
                tree,
                verdict,
                rc,
                took_ms,
                log,
                ..
            } => {
                held.prepared = None;
                if *step == held.size() {
                    held.tip_gate = Some(TipGate {
                        check: check.clone(),
                        tree: tree.clone(),
                        verdict: *verdict,
                        rc: *rc,
                        took_ms: *took_ms,
                        log: log.clone(),
                    });
                    held.standing = match verdict {
                        Verdict::Green => Standing::Green,
                        Verdict::Red => Standing::Red,
                    };
                }
            }
            other => panic!("the test's fold does not read {}", other.phase()),
        }
    }
    state.expect("a chosen record")
}

/// Choose the fixture's members and stack them: the batch as the ledger holds it after `stacked`.
fn stacked(fixture: &Fixture, window: &Window) -> (BatchState, Vec<LandBatchReceipt>) {
    let answer = said(press(window, coordinator(), choose(fixture)));
    assert_eq!(answer["state"], "stacked", "{answer}");
    let records: Vec<LandBatchReceipt> = (0..3).map(|_| window.next()).collect();
    assert_eq!(phases(&records), ["chosen", "stacking", "stacked"]);
    (state_of(&records), records)
}

/// The tip as the ledger holds it after a green gate of its exact tree.
fn green(state: &mut BatchState) {
    let stack = state.stacked.clone().expect("a stack");
    state.tip_gate = Some(TipGate {
        check: "lc-1791940000000-1".to_string(),
        tree: stack.tree,
        verdict: Verdict::Green,
        rc: 0,
        took_ms: Some(1_834_200),
        log: "/gate/lc-1791940000000-1.log".to_string(),
    });
    state.standing = Standing::Green;
}

/// The ledger's `promoting`, as a press wrote it before a crash.
fn promoting(state: &BatchState, by: PressedBy, lease_until_ms: i64) -> Promoting {
    Promoting {
        by,
        at: NOW,
        target: state.stacked.as_ref().expect("a stack").tip.clone(),
        expect: state.base.clone(),
        base_branch: state.base_branch.clone(),
        lease_until_ms,
    }
}

/// One generation's stacking as the window runs it, for the roads that stop half way.
fn job(fixture: &Fixture, batch: &str) -> StackJob {
    StackJob {
        run: RUN.to_string(),
        batch: batch.to_string(),
        generation: 1,
        members: fixture.members.clone(),
        messages: fixture
            .members
            .iter()
            .map(|member| {
                fs::read_to_string(fixture.messages.join(format!("{}.txt", member.task)))
                    .expect("a landing message")
            })
            .collect(),
        base_branch: "refs/heads/main".to_string(),
        base: fixture.main.clone(),
        repo: fixture.repo.clone(),
        by: coordinator(),
        policy: on(),
    }
}

/// The batch as the ledger holds it while `job` stacks: chosen, and stacking under `lease`.
fn stacking(fixture: &Fixture, batch: &str, lease_until_ms: i64) -> BatchState {
    let operation = operation_id(RUN, batch, 1);
    let mut state = state_of(&[LandBatchReceipt {
        run: RUN.to_string(),
        batch: batch.to_string(),
        generation: 1,
        by: coordinator(),
        record: Record::Chosen {
            members: fixture.members.clone(),
            base_branch: "refs/heads/main".to_string(),
            base: fixture.main.clone(),
            repository: fixture.repo.display().to_string(),
        },
    }]);
    state.operations.push(operation.clone());
    state.stacking = Some(Stacking {
        operation,
        lease_until_ms,
        by: coordinator(),
    });
    state.standing = Standing::Stacking;
    state
}

/// Prepare the gate the batch asks for next, and the batch as the ledger holds it after.
fn prepare_gate(window: &Window, state: &mut BatchState, step: u32) -> (String, PathBuf, PathBuf) {
    let target = state.step_commit(step).expect("a step").to_string();
    let answer = said(press(
        window,
        coordinator(),
        Action::Prepare {
            batch: state.clone(),
            step,
            target,
        },
    ));
    assert_eq!(answer["state"], "prepared", "{answer}");
    let prepared = window.next();
    *state = fold(state, &prepared);
    let Some(gate) = &state.prepared else {
        panic!("no gate_prepared: {:?}", prepared.record);
    };
    (
        gate.check.clone(),
        PathBuf::from(&gate.log),
        PathBuf::from(&gate.folder),
    )
}

/// One more record on a batch the test already holds.
fn fold(state: &BatchState, receipt: &LandBatchReceipt) -> BatchState {
    let chosen = LandBatchReceipt {
        run: RUN.to_string(),
        batch: state.batch.clone(),
        generation: state.generation,
        by: state.chosen_by.clone(),
        record: Record::Chosen {
            members: state.members.clone(),
            base_branch: state.base_branch.clone(),
            base: state.base.clone(),
            repository: state.repository.clone(),
        },
    };
    let mut held = state_of(&[chosen, receipt.clone()]);
    held.stacking = state.stacking.clone();
    held.operations = state.operations.clone();
    held.stacked = state.stacked.clone();
    if held.tip_gate.is_none() {
        held.tip_gate = state.tip_gate.clone();
    }
    held
}

#[cfg(unix)]
fn script(path: &Path, text: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt as _;
    fs::write(path, text).expect("the stand-in git");
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("an executable");
    path.to_path_buf()
}

/// 1 (the window's half): a repository that never turned the queue on refuses the press before
/// anything is written — no record, no ref, no folder — and the switch is the project's own
/// setting, read per repository.
#[test]
fn default_off_refuses_and_writes_nothing() {
    let fixture = Fixture::new(2, &[]);
    let window = window(&fixture);
    let settings = settings::SettingsRepository::new(fixture.root.join("settings"));
    assert!(
        !policy_in(&settings, &fixture.repo).enabled,
        "a repository nobody turned on runs the queue"
    );
    let refused = run(
        &|repo| policy_in(&settings, repo),
        &ask(coordinator(), choose(&fixture)),
        &[],
        NOW,
        &window.roads,
    );
    let why = refused.expect_err("an off repository stacked");
    assert!(why.contains("off in this repository"), "{why}");
    assert!(window.quiet(), "a record was written");
    assert_eq!(
        git(&fixture.repo, &["for-each-ref", "refs/zerocode/"]),
        "",
        "a ref was made"
    );
    assert!(
        !fixture.data.join("land-batch").exists(),
        "a folder was made"
    );
    assert!(fixture.land_check("work").is_empty());
    assert_eq!(fixture.rev("refs/heads/main"), fixture.main);

    // The switch is the repository's own: on, it reads on wherever this machine runs the queue,
    // and off again, off.
    let key = project_runtime::project_settings_key(&fixture.repo);
    for (said, reads) in [
        (Some(true), integration_queue_supported()),
        (Some(false), false),
    ] {
        project_runtime::mutate_project_settings(&settings, |all| {
            all.entry(key.clone())
                .or_default()
                .integration_queue_enabled = said;
            Ok(())
        })
        .expect("the switch is saved");
        assert_eq!(
            policy_in(&settings, &fixture.repo).enabled,
            reads,
            "{said:?}"
        );
    }
}

/// 5: one merge commit per member on the base, each with the step before and the member's head
/// as its parents and the member's landing message under a line that names it; the integration
/// ref is published at the tip with the operation's receipt, and the base branch does not move.
#[test]
fn stacking_builds_one_merge_per_member_on_main() {
    let fixture = Fixture::new(3, &[]);
    let window = window(&fixture);
    let (state, records) = stacked(&fixture, &window);
    let Record::Chosen {
        members,
        base_branch,
        base,
        repository,
    } = &records[0].record
    else {
        panic!("{:?}", records[0].record);
    };
    assert_eq!(
        members, &fixture.members,
        "the members or their order changed"
    );
    assert_eq!(base_branch, "refs/heads/main");
    assert_eq!(base, &fixture.main);
    assert_eq!(repository, &fixture.repo.display().to_string());
    assert_eq!(records[0].by, coordinator());
    assert_eq!(records[0].generation, 1);
    assert!(is_batch_id(&state.batch), "{}", state.batch);
    let Record::Stacking {
        operation,
        lease_until_ms,
    } = &records[1].record
    else {
        panic!("{:?}", records[1].record);
    };
    assert_eq!(operation, &operation_id(RUN, &state.batch, 1));
    assert!(*lease_until_ms > NOW);

    let stack = state.stacked.clone().expect("a stack");
    assert_eq!(stack.steps.len(), 3);
    assert_eq!(stack.tip, stack.steps[2]);
    let mut previous = fixture.main.clone();
    for (k, (step, member)) in stack.steps.iter().zip(&fixture.members).enumerate() {
        assert_eq!(
            git(&fixture.repo, &["rev-list", "--parents", "-n", "1", step]),
            format!("{step} {previous} {}", member.head),
            "step {} is not a merge of the step before and its member",
            k + 1
        );
        let message = git(&fixture.repo, &["log", "-1", "--format=%B", step]);
        assert!(
            message.starts_with(&format!(
                "Merge {} at {} — Land {}: add f{}.txt",
                member.task,
                &member.head[..9],
                member.task,
                k + 1
            )),
            "{message}"
        );
        assert!(
            message.contains(&format!("Why {} lands.", member.task)),
            "{message}"
        );
        assert!(
            message.contains(&format!(
                "Stacked by the integration queue: batch {}, generation 1,\nstep {} of 3, on main {}.",
                state.batch,
                k + 1,
                &fixture.main[..9]
            )),
            "{message}"
        );
        previous = step.clone();
    }
    assert_eq!(fixture.rev(&format!("{}^{{tree}}", stack.tip)), stack.tree);
    let files = git(&fixture.repo, &["ls-tree", "--name-only", &stack.tip]);
    for file in ["a.txt", "f1.txt", "f2.txt", "f3.txt"] {
        assert!(
            files.lines().any(|line| line == file),
            "{file} is not in the tip: {files}"
        );
    }
    assert_eq!(fixture.rev(&integration_ref(&state.batch, 1)), stack.tip);
    assert_eq!(fixture.rev(&stack_ref(&state.batch, 1)), stack.tip);
    assert_eq!(
        fixture.rev(&format!(
            "{APPLIED_REF_PREFIX}{}",
            operation_id(RUN, &state.batch, 1)
        )),
        stack.tip,
        "the publish left no receipt"
    );
    assert_eq!(fixture.rev("refs/heads/main"), fixture.main, "main moved");
}

/// 6: a member that clashes stops the stacking at its own step and names the files; the steps
/// before it stay on the stack ref, and the integration ref is never published past the base.
#[test]
fn stacking_stops_at_the_first_conflict_and_names_files() {
    let fixture = Fixture::new(4, &[1, 3]);
    let window = window(&fixture);
    let answer = said(press(&window, coordinator(), choose(&fixture)));
    assert_eq!(answer["state"], "stack_stopped", "{answer}");
    assert_eq!(answer["step"], 3, "{answer}");
    assert_eq!(answer["task"], "t-3", "{answer}");
    assert_eq!(answer["why"], "conflict", "{answer}");
    assert_eq!(answer["files"], json!(["shared.txt"]), "{answer}");
    assert_eq!(answer["total"], 1, "{answer}");
    let records: Vec<LandBatchReceipt> = (0..3).map(|_| window.next()).collect();
    assert_eq!(phases(&records), ["chosen", "stacking", "stack_stopped"]);
    assert_eq!(
        records[2].record,
        Record::StackStopped {
            step: 3,
            task: "t-3".to_string(),
            why: StackStop::Conflict,
            files: vec!["shared.txt".to_string()],
            total: 1,
        }
    );
    assert!(window.quiet(), "a record followed the stop");
    let batch = &records[0].batch;
    let held = fixture.rev(&stack_ref(batch, 1));
    assert_eq!(
        git(
            &fixture.repo,
            &[
                "rev-list",
                "--first-parent",
                "--count",
                &format!("{}..{held}", fixture.main)
            ]
        ),
        "2",
        "the stack ref does not hold the two steps before the clash"
    );
    assert_eq!(
        fixture.rev(&integration_ref(batch, 1)),
        fixture.main,
        "the integration ref was published past the base"
    );
    assert_eq!(
        git(&fixture.repo, &["for-each-ref", "refs/zerocode/publish/"]),
        "",
        "a publish receipt was written"
    );
    assert_eq!(fixture.rev("refs/heads/main"), fixture.main);
}

/// 7: stacking touches no checkout: the main checkout and a worker's keep their HEAD, their index
/// bytes and their status, and the branch list is the same — only refs under `refs/zerocode/`
/// are new.
#[test]
fn stacking_touches_no_worktree_index_or_branch() {
    let fixture = Fixture::new(2, &[]);
    let worker = fixture.root.join("worker");
    git(
        &fixture.repo,
        &[
            "worktree",
            "add",
            "-q",
            &worker.display().to_string(),
            "t-1",
        ],
    );
    let index_of = |dir: &Path| -> Vec<u8> {
        let named = PathBuf::from(git(dir, &["rev-parse", "--git-path", "index"]));
        fs::read(if named.is_absolute() {
            named
        } else {
            dir.join(named)
        })
        .expect("an index")
    };
    let seen = || -> Vec<String> {
        vec![
            git(&fixture.repo, &["rev-parse", "HEAD"]),
            git(&fixture.repo, &["symbolic-ref", "HEAD"]),
            git(&fixture.repo, &["status", "--porcelain"]),
            git(&worker, &["rev-parse", "HEAD"]),
            git(&worker, &["symbolic-ref", "HEAD"]),
            git(&worker, &["status", "--porcelain"]),
            git(&fixture.repo, &["for-each-ref", "refs/heads/"]),
            git(&fixture.repo, &["worktree", "list", "--porcelain"]),
        ]
    };
    let (main_index, worker_index) = (index_of(&fixture.repo), index_of(&worker));
    let before = seen();

    let window = window(&fixture);
    let (state, _) = stacked(&fixture, &window);

    assert_eq!(
        index_of(&fixture.repo),
        main_index,
        "the main checkout's index changed"
    );
    assert_eq!(
        index_of(&worker),
        worker_index,
        "the worker's index changed"
    );
    assert_eq!(seen(), before);
    let made = git(
        &fixture.repo,
        &["for-each-ref", "--format=%(refname)", "refs/zerocode/"],
    );
    let mut made: Vec<&str> = made.lines().collect();
    made.sort_unstable();
    let mut want = vec![
        format!("{APPLIED_REF_PREFIX}{}", operation_id(RUN, &state.batch, 1)),
        integration_ref(&state.batch, 1),
        stack_ref(&state.batch, 1),
    ];
    want.sort();
    assert_eq!(made, want);
}

/// 9: the gate is prepared on land-check's road for the tip, and its folder holds the tip's exact
/// tree. Once the compare ref is not an ancestor of the tip, the merged tree is another, and the
/// window takes that folder down and records nothing.
#[test]
fn gate_prepares_the_tip_tree_and_refuses_another_tree() {
    let fixture = Fixture::new(2, &[]);
    let window = window(&fixture);
    let (state, _) = stacked(&fixture, &window);
    let stack = state.stacked.clone().expect("a stack");
    let answer = said(press(
        &window,
        coordinator(),
        Action::Prepare {
            batch: state.clone(),
            step: 2,
            target: stack.tip.clone(),
        },
    ));
    assert_eq!(answer["state"], "prepared", "{answer}");
    assert_eq!(answer["tree"], stack.tree.as_str(), "{answer}");
    let folder = PathBuf::from(answer["path"].as_str().expect("the folder"));
    assert!(
        folder.starts_with(fixture.data.join("land-check").join("work")),
        "{folder:?}"
    );
    assert!(
        folder.join("f2.txt").exists(),
        "the folder does not hold the tip"
    );
    let check = answer["check"].as_str().expect("the check").to_string();
    assert_eq!(
        window.next().record,
        Record::GatePrepared {
            check: check.clone(),
            step: 2,
            target: stack.tip.clone(),
            tree: stack.tree.clone(),
            folder: folder.display().to_string(),
            log: answer["log"].as_str().expect("the log").to_string(),
        }
    );

    commit_file(
        &fixture.repo,
        "late.txt",
        "late\n",
        "main moves on without the batch",
    );
    let refused = press(
        &window,
        coordinator(),
        Action::Prepare {
            batch: state.clone(),
            step: 2,
            target: stack.tip.clone(),
        },
    );
    let why = refused.expect_err("another tree was prepared");
    assert!(why.contains("not step 2's tree"), "{why}");
    assert!(why.contains("taken down"), "{why}");
    assert!(window.quiet(), "a gate was recorded for another tree");
    assert_eq!(
        fixture.land_check("work"),
        vec![check.clone()],
        "the refused folder is still there"
    );
    assert_eq!(fixture.land_check("rows"), vec![format!("{check}.json")]);
    let listed = git(&fixture.repo, &["worktree", "list", "--porcelain"]);
    assert_eq!(listed.matches("worktree ").count(), 2, "{listed}");
    let logs = fixture.land_check("logs");
    let taken = logs
        .iter()
        .filter(|name| **name != format!("{check}.log"))
        .map(|name| {
            fs::read_to_string(fixture.data.join("land-check").join("logs").join(name))
                .unwrap_or_default()
        })
        .collect::<Vec<_>>();
    assert!(
        taken.iter().any(|log| log.contains("taken down unrun")),
        "the taken-down gate's log does not say why: {taken:?}"
    );
}

/// 10 (the window's half): the end of the gate's jobs goes through land-check's record road —
/// its letter, its folder taken down — and then the batch's `gated`: green only for rc 0.
#[test]
fn record_writes_green_or_red_for_the_tip() {
    let fixture = Fixture::new(2, &[]);
    let window = window(&fixture);
    let (mut state, _) = stacked(&fixture, &window);
    let stack = state.stacked.clone().expect("a stack");
    for (rc, word, letter_state) in [(0, "green", "passed"), (3, "red", "failed")] {
        let (check, log, folder) = prepare_gate(&window, &mut state, 2);
        fs::write(&log, format!("core rc={rc}\nshell rc=0\n")).expect("the RC summary");
        let answer = said(press(
            &window,
            coordinator(),
            Action::Record {
                batch: state.clone(),
                check: check.clone(),
                rc,
                log: log.display().to_string(),
                took_ms: Some(1_834_200),
            },
        ));
        assert_eq!(answer["state"], word, "{answer}");
        let letter = window
            .letters
            .recv_timeout(RECORD_WAIT)
            .expect("land-check's letter");
        assert_eq!(letter.task, BATCH_TASK);
        assert_eq!(letter.check, check);
        let evidence: serde_json::Value =
            serde_json::from_str(&letter.evidence).expect("the evidence is JSON");
        assert_eq!(evidence["state"], letter_state, "{evidence}");
        assert_eq!(evidence["rc"], rc, "{evidence}");
        let gated = window.next();
        assert_eq!(
            gated.record,
            Record::Gated {
                check: check.clone(),
                step: 2,
                target: stack.tip.clone(),
                tree: stack.tree.clone(),
                verdict: Verdict::of(rc),
                rc,
                took_ms: Some(1_834_200),
                log: log.display().to_string(),
            }
        );
        assert!(!folder.exists(), "the gate folder outlived its record");
        state = fold(&state, &gated);
    }
}

/// 11: a green tip promotes by fast-forward: the base branch and its checkout stand at the tip,
/// whose tree is the one the gate passed, with nothing left unfinished and no commit made.
#[test]
fn promote_fast_forwards_main_and_its_checkout_when_green() {
    let fixture = Fixture::new(2, &[]);
    let window = window(&fixture);
    let (mut state, _) = stacked(&fixture, &window);
    green(&mut state);
    let stack = state.stacked.clone().expect("a stack");
    let answer = said(press(
        &window,
        person_press(),
        Action::Promote {
            batch: state.clone(),
        },
    ));
    assert_eq!(answer["state"], "promoted", "{answer}");
    let first = window.next();
    let Record::Promoting {
        target,
        expect,
        base_branch,
        lease_until_ms,
    } = &first.record
    else {
        panic!(
            "the promote did not write promoting first: {:?}",
            first.record
        );
    };
    assert_eq!((target, expect), (&stack.tip, &fixture.main));
    assert_eq!(base_branch, "refs/heads/main");
    assert!(*lease_until_ms > NOW);
    assert_eq!(first.by, PressedBy::Person);
    let ended = window.next();
    assert_eq!(ended.by, PressedBy::Person);
    assert_eq!(
        ended.record,
        Record::Promoted {
            from: fixture.main.clone(),
            to: stack.tip.clone(),
            members: fixture
                .members
                .iter()
                .zip(&stack.steps)
                .map(|(member, step)| Landed {
                    task: member.task.clone(),
                    head: member.head.clone(),
                    merged_as: step.clone(),
                })
                .collect(),
            settled_later: false,
        }
    );
    assert_eq!(fixture.rev("refs/heads/main"), stack.tip);
    assert_eq!(fixture.rev("HEAD"), stack.tip);
    assert_eq!(
        fixture.rev("HEAD^{tree}"),
        stack.tree,
        "main's tree is not the gated tree"
    );
    assert_eq!(git(&fixture.repo, &["status", "--porcelain"]), "");
    assert!(fixture.repo.join("f1.txt").exists() && fixture.repo.join("f2.txt").exists());
}

/// 12: a base branch that moved stops the promote and nothing changes — whether it moved before
/// the press, or somebody moved it between the window's read and the fast-forward.
#[test]
fn promote_stops_when_main_moved_before_the_fast_forward() {
    let fixture = Fixture::new(2, &[]);
    let window = window(&fixture);
    let (mut state, _) = stacked(&fixture, &window);
    green(&mut state);
    let tip = state.stacked.clone().expect("a stack").tip;
    let moved = commit_file(&fixture.repo, "late.txt", "late\n", "somebody lands first");
    let answer = said(press(
        &window,
        person_press(),
        Action::Promote {
            batch: state.clone(),
        },
    ));
    assert_eq!(answer["state"], "promote_stopped", "{answer}");
    assert_eq!(answer["why"], "main_moved", "{answer}");
    assert_eq!(window.next().record.phase(), "promoting");
    assert_eq!(
        window.next().record,
        Record::PromoteStopped {
            why: PromoteStop::MainMoved,
            files: Vec::new(),
            total: 0,
            seen: Some(moved.clone()),
        }
    );
    assert_eq!(fixture.rev("refs/heads/main"), moved);
    assert_eq!(fixture.rev("HEAD"), moved);
    assert!(!git_says(
        &fixture.repo,
        &["merge-base", "--is-ancestor", &tip, "main"]
    ));

    #[cfg(unix)]
    {
        let fixture = Fixture::new(2, &[]);
        let window = self::window(&fixture);
        let (mut state, _) = stacked(&fixture, &window);
        green(&mut state);
        let tip = state.stacked.clone().expect("a stack").tip;
        let tree = format!("{}^{{tree}}", fixture.main);
        let sibling = git(
            &fixture.repo,
            &[
                "commit-tree",
                &tree,
                "-p",
                &fixture.main,
                "-m",
                "a sibling of the base",
            ],
        );
        let meddling = script(
            &fixture.root.join("meddling-git"),
            &format!(
                "#!/bin/sh\ncase \" $* \" in *\" merge \"*) git -C '{}' update-ref refs/heads/main {sibling} ;; esac\nexec git \"$@\"\n",
                fixture.repo.display()
            ),
        );
        let policy = Policy {
            git: meddling,
            ..on()
        };
        let answer = said(run(
            &|_| policy.clone(),
            &ask(
                person_press(),
                Action::Promote {
                    batch: state.clone(),
                },
            ),
            &[],
            NOW,
            &window.roads,
        ));
        assert_eq!(answer["state"], "promote_stopped", "{answer}");
        assert_eq!(answer["why"], "main_moved", "{answer}");
        assert_eq!(window.next().record.phase(), "promoting");
        let stopped = window.next();
        let Record::PromoteStopped { why, seen, .. } = &stopped.record else {
            panic!("{:?}", stopped.record);
        };
        assert_eq!(
            (*why, seen.as_deref()),
            (PromoteStop::MainMoved, Some(sibling.as_str()))
        );
        assert_eq!(
            fixture.rev("refs/heads/main"),
            sibling,
            "the window moved main itself"
        );
        assert!(!git_says(
            &fixture.repo,
            &["merge-base", "--is-ancestor", &tip, "main"]
        ));
    }
}

/// 13: the base branch's checkout with unfinished work stops the promote and names it — a
/// changed file, a staged one, a merge in progress, a held index — and the base does not move.
#[test]
fn promote_stops_on_unfinished_files_in_the_main_checkout() {
    fn modified(repo: &Path) {
        fs::write(repo.join("a.txt"), "changed\n").expect("a change");
    }
    fn staged(repo: &Path) {
        fs::write(repo.join("a.txt"), "staged\n").expect("a change");
        git(repo, &["add", "a.txt"]);
    }
    fn merging(repo: &Path) {
        git(repo, &["checkout", "-q", "-b", "side"]);
        commit_file(repo, "side.txt", "side\n", "a side branch");
        git(repo, &["checkout", "-q", "main"]);
        git(repo, &["merge", "-q", "--no-ff", "--no-commit", "side"]);
    }
    fn locked(repo: &Path) {
        fs::write(repo.join(".git").join("index.lock"), "").expect("a held index");
    }
    type Case = (&'static str, fn(&Path), &'static str);
    let cases: [Case; 4] = [
        ("a changed file", modified, "a.txt"),
        ("a staged file", staged, "a.txt"),
        ("a merge in progress", merging, "MERGE_HEAD"),
        ("a held index", locked, "index.lock"),
    ];
    for (case, make, named) in cases {
        let fixture = Fixture::new(1, &[]);
        let window = window(&fixture);
        let (mut state, _) = stacked(&fixture, &window);
        green(&mut state);
        make(&fixture.repo);
        let answer = said(press(
            &window,
            person_press(),
            Action::Promote {
                batch: state.clone(),
            },
        ));
        assert_eq!(answer["state"], "promote_stopped", "{case}: {answer}");
        assert_eq!(answer["why"], "unfinished_files", "{case}: {answer}");
        assert_eq!(window.next().record.phase(), "promoting", "{case}");
        let stopped = window.next();
        let Record::PromoteStopped {
            why, files, total, ..
        } = &stopped.record
        else {
            panic!("{case}: {:?}", stopped.record);
        };
        assert_eq!(*why, PromoteStop::UnfinishedFiles, "{case}");
        assert!(files.iter().any(|file| file == named), "{case}: {files:?}");
        assert!(*total >= 1, "{case}");
        assert_eq!(
            fixture.rev("refs/heads/main"),
            fixture.main,
            "{case}: main moved"
        );
    }
}

/// 19: a restack without the culprit makes generation two on the base branch as it stands now,
/// leaving out the culprit and the member that reached main meanwhile, under a new operation; the
/// first generation's refs stay as they were.
#[test]
fn restack_without_the_culprit_makes_generation_two() {
    let fixture = Fixture::new(3, &[]);
    let window = window(&fixture);
    let (mut state, _) = stacked(&fixture, &window);
    let first = state.stacked.clone().expect("a stack");
    state.tip_gate = Some(TipGate {
        check: "lc-1791940000000-2".to_string(),
        tree: first.tree.clone(),
        verdict: Verdict::Red,
        rc: 1,
        took_ms: None,
        log: "/gate/lc-1791940000000-2.log".to_string(),
    });
    state.search = Some(Search {
        lo: 1,
        hi: 2,
        next: None,
        steps: Vec::new(),
        by: coordinator(),
    });
    state.culprit = Some(Culprit {
        task: "t-2".to_string(),
        step: 2,
        green: Some("lc-1791940000000-3".to_string()),
        red: "lc-1791940000000-4".to_string(),
        main_first: false,
    });
    state.standing = Standing::CulpritFound;
    git(
        &fixture.repo,
        &[
            "merge",
            "-q",
            "--no-ff",
            "-m",
            "t-3 lands by hand",
            &fixture.members[2].head,
        ],
    );
    let base = fixture.rev("refs/heads/main");

    let answer = said(press(
        &window,
        coordinator(),
        Action::Restack {
            batch: state.clone(),
            drop: vec!["t-2".to_string()],
        },
    ));
    assert_eq!(answer["state"], "stacked", "{answer}");
    assert_eq!(answer["generation"], 2, "{answer}");
    let records: Vec<LandBatchReceipt> = (0..3).map(|_| window.next()).collect();
    assert_eq!(phases(&records), ["restacked", "stacking", "stacked"]);
    assert!(records.iter().all(|one| one.generation == 2));
    assert_eq!(
        records[0].record,
        Record::Restacked {
            members: vec![fixture.members[0].clone()],
            dropped: vec![
                Dropped {
                    task: "t-2".to_string(),
                    why: DropWhy::Culprit,
                },
                Dropped {
                    task: "t-3".to_string(),
                    why: DropWhy::OnMain,
                },
            ],
            base: base.clone(),
        }
    );
    let second_operation = operation_id(RUN, &state.batch, 2);
    assert_ne!(second_operation, operation_id(RUN, &state.batch, 1));
    let Record::Stacking { operation, .. } = &records[1].record else {
        panic!("{:?}", records[1].record);
    };
    assert_eq!(
        operation, &second_operation,
        "the restack reused the old operation"
    );
    let Record::Stacked { steps, tip, .. } = &records[2].record else {
        panic!("{:?}", records[2].record);
    };
    assert_eq!(steps.len(), 1);
    assert_eq!(
        git(&fixture.repo, &["rev-list", "--parents", "-n", "1", tip]),
        format!("{tip} {base} {}", fixture.members[0].head)
    );
    assert_eq!(fixture.rev(&stack_ref(&state.batch, 2)), *tip);
    assert_eq!(fixture.rev(&integration_ref(&state.batch, 2)), *tip);
    assert_eq!(
        fixture.rev(&format!("{APPLIED_REF_PREFIX}{second_operation}")),
        *tip
    );
    assert_eq!(
        fixture.rev(&stack_ref(&state.batch, 1)),
        first.tip,
        "generation 1 moved"
    );
    assert_eq!(fixture.rev(&integration_ref(&state.batch, 1)), first.tip);
    assert_eq!(
        fixture.rev(&format!(
            "{APPLIED_REF_PREFIX}{}",
            operation_id(RUN, &state.batch, 1)
        )),
        first.tip
    );
}

/// 20 (the window's half): a cancel writes `cancelled` first and tombstones the generation's
/// publish, so a stacker that built its merges before the cancel cannot publish them after it:
/// the integration ref stays at the base.
#[test]
fn cancel_tombstones_the_generation_and_a_late_stacker_cannot_publish() {
    let fixture = Fixture::new(2, &[]);
    let window = window(&fixture);
    let batch = "lb-1791940000000-7";
    let late = job(&fixture, batch);
    let host = Host::for_workspace(&fixture.repo);
    let built = build(&host, &late).expect("the merges are built");
    assert_eq!(built.steps.len(), 2);
    let state = stacking(&fixture, batch, NOW + 60_000);

    let answer = said(press(
        &window,
        coordinator(),
        Action::Cancel {
            batch: state.clone(),
        },
    ));
    assert_eq!(answer["state"], "cancelled", "{answer}");
    assert_eq!(answer["tombstoned"], true, "{answer}");
    let cancelled = window.next();
    assert_eq!(cancelled.record, Record::Cancelled {});
    assert_eq!(cancelled.by, coordinator());
    let operation = operation_id(RUN, batch, 1);
    assert!(
        fixture.has(&format!("{CANCELLED_REF_PREFIX}{operation}")),
        "no tombstone"
    );

    assert_eq!(
        publish(&late, &built),
        Err(Halted::Stop(Halt {
            step: 2,
            task: "t-2".to_string(),
            why: StackStop::PublishRefused,
            files: Vec::new(),
            total: 0,
        })),
        "a late stacker published after the cancel"
    );
    assert_eq!(fixture.rev(&integration_ref(batch, 1)), fixture.main);
    assert!(!fixture.has(&format!("{APPLIED_REF_PREFIX}{operation}")));
}

/// 21: a stacking whose lease ended without its record is settled from what git holds: a publish
/// that applied is the stack, written late; one that never applied is tombstoned and abandoned. A
/// lease still running is left alone.
#[test]
fn next_run_settles_a_crash_mid_publish() {
    let fixture = Fixture::new(2, &[]);
    let window = window(&fixture);
    let applied = "lb-1791940000000-21";
    let job_applied = job(&fixture, applied);
    let host = Host::for_workspace(&fixture.repo);
    let built = build(&host, &job_applied).expect("the merges are built");
    publish(&job_applied, &built).expect("the publish applied");
    settle(
        &|_| on(),
        &[stacking(&fixture, applied, NOW - 1)],
        RUN,
        None,
        NOW,
        &window.roads,
    );
    let late = window.next();
    assert_eq!(late.batch, applied);
    assert_eq!(late.by, PressedBy::Ledger);
    let Record::Stacked {
        steps, tip, tree, ..
    } = &late.record
    else {
        panic!("{:?}", late.record);
    };
    assert_eq!(steps, &built.steps);
    assert_eq!(Some(tip), built.steps.last());
    assert_eq!(tree, &built.tree);

    let never = "lb-1791940000000-22";
    let job_never = job(&fixture, never);
    build(&host, &job_never).expect("the merges are built");
    let running = stacking(&fixture, never, NOW + 60_000);
    settle(&|_| on(), &[running], RUN, None, NOW, &window.roads);
    assert!(window.quiet(), "a running stacking was settled");
    settle(
        &|_| on(),
        &[stacking(&fixture, never, NOW - 1)],
        RUN,
        None,
        NOW,
        &window.roads,
    );
    let abandoned = window.next();
    assert_eq!(abandoned.by, PressedBy::Ledger);
    let operation = operation_id(RUN, never, 1);
    assert_eq!(
        abandoned.record,
        Record::StackAbandoned {
            operation: operation.clone()
        }
    );
    assert!(
        fixture.has(&format!("{CANCELLED_REF_PREFIX}{operation}")),
        "no tombstone"
    );
    assert_eq!(fixture.rev(&integration_ref(never, 1)), fixture.main);
}

/// 22: a promote whose lease ended without its record is settled from what git holds, and nothing
/// is reset: main at the tip is `promoted` in the presser's name; main at the base, clean, is
/// `promote_abandoned`; anything else is `promote_stopped` as `unknown_state`.
#[test]
fn next_run_settles_a_crash_mid_promote() {
    for case in ["at the tip", "at the base", "elsewhere"] {
        let fixture = Fixture::new(2, &[]);
        let window = window(&fixture);
        let (mut state, _) = stacked(&fixture, &window);
        green(&mut state);
        let tip = state.stacked.clone().expect("a stack").tip;
        state.promoting = Some(promoting(&state, person_press(), NOW - 1));
        state.standing = Standing::Promoting;
        let moved = match case {
            "at the tip" => {
                git(&fixture.repo, &["merge", "-q", "--ff-only", &tip]);
                tip.clone()
            }
            "at the base" => fixture.main.clone(),
            _ => commit_file(&fixture.repo, "late.txt", "late\n", "somebody else"),
        };
        settle(&|_| on(), &[state.clone()], RUN, None, NOW, &window.roads);
        let settled = window.next();
        match case {
            "at the tip" => {
                assert_eq!(settled.by, PressedBy::Person, "the presser was not kept");
                let Record::Promoted {
                    from,
                    to,
                    settled_later,
                    ..
                } = &settled.record
                else {
                    panic!("{case}: {:?}", settled.record);
                };
                assert_eq!((from, to, *settled_later), (&fixture.main, &tip, true));
            }
            "at the base" => {
                assert_eq!(settled.by, PressedBy::Ledger);
                assert_eq!(
                    settled.record,
                    Record::PromoteAbandoned {
                        seen: fixture.main.clone()
                    }
                );
            }
            _ => {
                assert_eq!(settled.by, PressedBy::Ledger);
                let Record::PromoteStopped { why, seen, .. } = &settled.record else {
                    panic!("{case}: {:?}", settled.record);
                };
                assert_eq!(
                    (*why, seen.as_deref()),
                    (PromoteStop::UnknownState, Some(moved.as_str()))
                );
            }
        }
        assert_eq!(
            fixture.rev("refs/heads/main"),
            moved,
            "{case}: the settle moved main"
        );
        assert!(window.quiet(), "{case}: a second record");
    }
}

/// 23: the sweep deletes the refs of ended batches past [`INTEGRATION_KEEP_MS`] and nothing else:
/// a batch that ended a moment ago and an open one from long ago keep theirs.
#[test]
fn sweep_removes_only_ended_batches_older_than_keep() {
    let fixture = Fixture::new(1, &[]);
    let window = window(&fixture);
    let (mut old_end, _) = stacked(&fixture, &window);
    let (mut new_end, _) = stacked(&fixture, &window);
    let (mut open, _) = stacked(&fixture, &window);
    let long_ago = NOW - INTEGRATION_KEEP_MS - 1;
    let tip = old_end.stacked.clone().expect("a stack").tip;
    old_end.promotion = Some(Promotion {
        by: person_press(),
        at: long_ago,
        from: fixture.main.clone(),
        to: tip,
        members: Vec::new(),
        settled_later: false,
    });
    old_end.standing = Standing::Promoted;
    old_end.last_at = long_ago;
    new_end.cancelled = Some(person_press());
    new_end.standing = Standing::Cancelled;
    new_end.last_at = NOW - 1_000;
    open.last_at = long_ago;
    let refs_of = |batch: &str| {
        [
            stack_ref(batch, 1),
            integration_ref(batch, 1),
            format!("{APPLIED_REF_PREFIX}{}", operation_id(RUN, batch, 1)),
        ]
    };
    for state in [&old_end, &new_end, &open] {
        assert!(refs_of(&state.batch).iter().all(|name| fixture.has(name)));
    }

    settle(
        &|_| on(),
        &[old_end.clone(), new_end.clone(), open.clone()],
        RUN,
        None,
        NOW,
        &window.roads,
    );
    let swept = window.next();
    assert_eq!(swept.batch, old_end.batch);
    assert_eq!(swept.by, PressedBy::Ledger);
    assert_eq!(swept.record, Record::Swept { refs: 3 });
    assert!(window.quiet(), "another batch was swept");
    assert!(
        refs_of(&old_end.batch)
            .iter()
            .all(|name| !fixture.has(name))
    );
    for state in [&new_end, &open] {
        assert!(
            refs_of(&state.batch).iter().all(|name| fixture.has(name)),
            "{} lost a ref",
            state.batch
        );
    }
    let kept = |batch: &str| fixture.data.join("land-batch").join("messages").join(batch);
    assert!(
        !kept(&old_end.batch).exists(),
        "the swept batch kept its messages"
    );
    assert!(
        kept(&open.batch).exists(),
        "the open batch lost its messages"
    );
}

/// The approval's first change: a fast-forward that outlives [`Policy::promote_budget`] is not
/// killed. The press answers "still promoting", git runs to its end, and the settle writes
/// `promoted` in the presser's name — the watcher when git ends, and the next run the same.
#[cfg(unix)]
#[test]
fn promote_outlives_its_budget_is_not_killed_and_the_next_settle_records_promoted() {
    const SLOW_SECS: u64 = 2;
    let fixture = Fixture::new(1, &[]);
    let window = window(&fixture);
    let (mut state, _) = stacked(&fixture, &window);
    green(&mut state);
    let tip = state.stacked.clone().expect("a stack").tip;
    let done = fixture.root.join("fast-forward.done");
    let slow = script(
        &fixture.root.join("slow-git"),
        &format!(
            "#!/bin/sh\nsleep {SLOW_SECS}\ngit \"$@\"\nrc=$?\necho $rc > '{}'\nexit $rc\n",
            done.display()
        ),
    );
    let policy = Policy {
        promote_budget: Duration::from_millis(300),
        git: slow,
        ..on()
    };
    let started = Instant::now();
    let answer = said(run(
        &|_| policy.clone(),
        &ask(
            person_press(),
            Action::Promote {
                batch: state.clone(),
            },
        ),
        &[],
        NOW,
        &window.roads,
    ));
    assert!(
        started.elapsed() < Duration::from_secs(SLOW_SECS),
        "the press waited for git past its budget"
    );
    assert_eq!(answer["state"], "promoting", "{answer}");
    assert_eq!(answer["still"], true, "{answer}");
    assert_eq!(window.next().record.phase(), "promoting");
    assert!(
        !done.exists(),
        "git ended inside the budget, so nothing outlived it"
    );
    assert!(
        window.quiet(),
        "the promote's end was written while git ran"
    );

    let waited = Instant::now();
    while !done.exists() {
        assert!(
            waited.elapsed() < RECORD_WAIT,
            "the fast-forward never ended"
        );
        std::thread::sleep(POLL);
    }
    assert_eq!(
        fs::read_to_string(&done).expect("git's exit").trim(),
        "0",
        "git was stopped instead of finishing"
    );
    let settled = window.next();
    assert_eq!(settled.by, PressedBy::Person, "the presser was not kept");
    let Record::Promoted {
        from,
        to,
        settled_later,
        ..
    } = &settled.record
    else {
        panic!("{:?}", settled.record);
    };
    assert_eq!((from, to, *settled_later), (&fixture.main, &tip, true));
    assert_eq!(fixture.rev("refs/heads/main"), tip);
    assert_eq!(fixture.rev("HEAD"), tip);

    while live(RUN, &state.batch) {
        assert!(waited.elapsed() < RECORD_WAIT, "the watcher never let go");
        std::thread::sleep(POLL);
    }
    let mut held = state.clone();
    held.promoting = Some(promoting(&state, person_press(), NOW - 1));
    held.standing = Standing::Promoting;
    settle(&|_| on(), &[held], RUN, None, NOW, &window.roads);
    let again = window.next();
    assert_eq!(again.by, PressedBy::Person);
    assert!(
        matches!(
            again.record,
            Record::Promoted {
                settled_later: true,
                ..
            }
        ),
        "{:?}",
        again.record
    );
}
