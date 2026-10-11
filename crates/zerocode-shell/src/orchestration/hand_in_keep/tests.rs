use std::path::{Path, PathBuf};

use zerocode_core::agent::ALL_AGENTS;
use zerocode_core::artifact::{ArtifactKind, Limits, Origin, Source};
use zerocode_core::hand_in::{
    Evidence, Expect, Facts, HAND_IN_BYTES_MAX, HandIn, Named, Outcome, ReportKind, Role, Standing,
    State, Why,
};
use zerocode_core::private_data::{HOME_NAME, MAILBOX};

use super::*;
use crate::orchestration::desk::{DeskSnapshot, DeskTask};

// Fictional values that belong to nobody; each carries its own waiver so this
// file does not trip the gate whose table the mask reads.
// pii-scan: allow home-path — a fictional account the keeping is tested with
const HOME: &str = "/Users/mallory";
// pii-scan: allow email — a fictional mailbox the keeping is tested with
const PERSON: &str = "chief@northwind-holdings.co.kr";
// pii-scan: allow credential — a fictional key id the keeping is tested with
const KEY_ID: &str = "AKIAQ7RVBNMLKJHGFDSZ";

/// A PNG's eight signature bytes: a screenshot as far as anything here looks.
const PNG: &[u8] = b"\x89PNG\r\n\x1a\n";

/// A scratch directory holding a store, a worker's checkout and a folder
/// outside both.
struct Bench {
    dir: tempfile::TempDir,
    store: Store,
}

impl Bench {
    fn open() -> Self {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let store = Store::open(&dir.path().join("data"), Limits::default());
        std::fs::create_dir_all(dir.path().join("checkout")).expect("the checkout");
        std::fs::create_dir_all(dir.path().join("elsewhere")).expect("a folder outside");
        Self { dir, store }
    }

    fn checkout(&self) -> PathBuf {
        self.dir.path().join("checkout")
    }

    fn elsewhere(&self) -> PathBuf {
        self.dir.path().join("elsewhere")
    }

    /// A file in the worker's checkout.
    fn write(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.checkout().join(name);
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("a folder");
        std::fs::write(&path, bytes).expect("a file");
        path
    }

    /// The job a `worker_done` naming `report` and `evidence` makes. The roots
    /// are the checkout alone, so a file in `elsewhere` is outside them. The run
    /// and the task are the message's own: the table of keepings in flight is
    /// the window's, and tests run side by side.
    fn job(&self, message: &str, report: Option<&Path>, evidence: &[&Path]) -> Job {
        Job {
            hand_in: HandIn {
                run: run_of(message),
                message: message.into(),
                task: Some(task_of(message)),
                dispatch: Some("dp-1".into()),
                worker: "w-1".into(),
                at_ms: 5,
                commit: None,
                named: Named {
                    report: report.map(Path::to_path_buf),
                    report_kind: None,
                    evidence: evidence.iter().map(|path| Evidence::at(*path)).collect(),
                    beyond: 0,
                },
            },
            origin: Origin {
                run: Some(run_of(message)),
                task: Some(task_of(message)),
                worker: Some("w-1".into()),
                agent: Some("claude".into()),
                ..Origin::default()
            },
            roots: Roots {
                checkout: Some(self.checkout().canonicalize().expect("canonical")),
                temps: Vec::new(),
            },
        }
    }
}

fn run_of(message: &str) -> String {
    format!("run-{message}")
}

fn task_of(message: &str) -> String {
    format!("t-{message}")
}

/// A desk row for the task a message belongs to.
fn desk_row(message: &str) -> DeskTask {
    DeskTask {
        run: run_of(message),
        id: task_of(message),
        title: "keep it".into(),
        stage: "reported",
        gate: None,
        blocked_by: Vec::new(),
        closed: None,
        created_ms: 1,
        cost: None,
        writing: None,
        kept: None,
        hand_in: Default::default(),
    }
}

/// A report of about `bytes` that carries one of each private value.
fn report_of(bytes: usize) -> String {
    let mut text = format!(
        "# Report\nbuilt in {HOME}/work/t-1 for {PERSON} with {KEY_ID}\nAPI_TOKEN=hunter2\n"
    );
    while text.len() < bytes {
        text.push_str(
            "a line of an ordinary report, with the disk-guard job and no secret in it\n",
        );
    }
    text
}

#[test]
fn a_report_is_kept_masked_and_the_board_still_opens_it_after_the_worktree_is_deleted() {
    let bench = Bench::open();
    let report = bench.write("report.md", report_of(20 * 1024).as_bytes());
    let job = bench.job("m-1", Some(&report), &[]);

    let manifest = ensure(&bench.store, &job, 10);
    assert_eq!(manifest.state(), State::Kept, "{manifest:?}");
    assert!(manifest.masked.total() >= 4, "{:?}", manifest.masked);

    // The worker's checkout is cleaned.
    std::fs::remove_dir_all(bench.checkout()).expect("the cleanup");
    assert!(!report.exists());

    // The board's row still points at the report, and the catalog opens it.
    let mut desk = DeskSnapshot {
        tasks: vec![desk_row("m-1")],
        ..DeskSnapshot::default()
    };
    dress_desk_with(&bench.store, &mut desk);
    let facts = desk.tasks[0]
        .kept
        .as_ref()
        .expect("the task row points at what was kept");
    assert_eq!(facts.state, "kept");
    let id = facts.report.as_deref().expect("the row names the report");
    let row = bench
        .store
        .get(id)
        .expect("the catalog holds the kept report");
    assert_eq!(row.kind, ArtifactKind::Report);
    assert_eq!(row.source, Source::WorkerReport);
    assert_eq!(row.origin.task.as_deref(), Some("t-m-1"));
    let text = std::fs::read_to_string(&row.path).expect("the kept copy is still there");
    assert!(text.starts_with("# Report\n"), "{text}");
    assert!(
        text.contains(&format!(
            "built in /Users/{HOME_NAME}/work/t-1 for {MAILBOX}"
        )),
        "{text}"
    );
    for secret in ["mallory", PERSON, KEY_ID, "hunter2"] {
        assert!(!text.contains(secret), "{secret} was kept: {text}");
    }
    assert!(
        text.contains("disk-guard"),
        "prose is not read as a key: {text}"
    );
    assert!(text.len() > 20 * 1024 - 200, "the report was kept whole");
}

#[test]
fn evidence_files_and_folders_are_kept_beside_the_report_and_open_as_what_they_are() {
    let bench = Bench::open();
    let report = bench.write("report.md", b"# done\n");
    let shot = bench.write("output/dark.png", PNG);
    let log = bench.write("output/run.log", b"built\n");
    let frames = bench.checkout().join("output/frames");
    bench.write("output/frames/light.png", PNG);
    bench.write("output/frames/deep/notes.txt", b"nested\n");
    let job = bench.job("m-2", Some(&report), &[&shot, &log, &frames]);

    let manifest = ensure(&bench.store, &job, 10);
    assert_eq!(manifest.state(), State::Kept, "{manifest:?}");
    assert_eq!(manifest.kept, 5, "{manifest:?}");
    let kept: Vec<(&str, Role)> = manifest
        .entries
        .iter()
        .map(|entry| (entry.name.as_str(), entry.role))
        .collect();
    assert_eq!(
        kept,
        [
            ("report.md", Role::Report),
            ("dark.png", Role::Evidence),
            ("run.log", Role::Evidence),
            ("frames/light.png", Role::Evidence),
            ("frames/deep/notes.txt", Role::Evidence),
        ]
    );
    // A file keeps its own name and extension in the store, whatever folder it
    // came from — a log summary reads `run.log` and `light.png` as they were.
    let stored: Vec<String> = manifest
        .entries
        .iter()
        .map(|entry| {
            let row = bench
                .store
                .get(entry.artifact.as_deref().expect("a kept row"))
                .expect("row");
            row.path
                .file_name()
                .expect("a name")
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert_eq!(
        stored,
        ["report.md", "dark.png", "run.log", "light.png", "notes.txt"]
    );
    let kinds: Vec<ArtifactKind> = manifest
        .entries
        .iter()
        .map(|entry| {
            bench
                .store
                .get(entry.artifact.as_deref().expect("a kept row"))
                .expect("in the catalog")
                .kind
        })
        .collect();
    assert_eq!(
        kinds,
        [
            ArtifactKind::Report,
            ArtifactKind::Screenshot,
            ArtifactKind::Evidence,
            ArtifactKind::Screenshot,
            ArtifactKind::Evidence,
        ]
    );
    assert_eq!(bench.store.counts().by_task.get("t-m-2"), Some(&5));
}

#[test]
fn a_file_over_the_cap_keeps_its_head_and_says_what_was_left_out() {
    let bench = Bench::open();
    // Lines of 64 bytes: the cap is a whole number of them, so the head ends exactly on it.
    let cap = usize::try_from(zerocode_core::hand_in::FILE_BYTES_MAX).expect("small");
    let big = format!("{}\n", "x".repeat(63))
        .repeat(cap / 64 + 65)
        .into_bytes();
    let log = bench.write("big.log", &big);
    let mut picture = PNG.to_vec();
    picture.extend_from_slice(&big);
    let huge = bench.write("big.png", &picture);
    let job = bench.job("m-3", None, &[&log, &huge]);

    let manifest = ensure(&bench.store, &job, 10);
    assert_eq!(manifest.state(), State::Partial, "{manifest:?}");
    let head = &manifest.entries[0];
    assert_eq!(
        (head.outcome, head.why),
        (Outcome::Trimmed, Some(Why::OverFileCap))
    );
    assert_eq!(head.source_bytes, big.len() as u64);
    let kept = std::fs::read_to_string(
        &bench
            .store
            .get(head.artifact.as_deref().expect("kept"))
            .expect("row")
            .path,
    )
    .expect("text");
    let said = format!(
        "[trimmed: the first {} of {} bytes are kept",
        zerocode_core::hand_in::FILE_BYTES_MAX,
        big.len()
    );
    assert!(
        kept.starts_with("xxxx") && kept.contains(&said),
        "{}",
        &kept[kept.len() - 120..]
    );
    let picture = &manifest.entries[1];
    assert_eq!(
        (picture.outcome, picture.why),
        (Outcome::Left, Some(Why::OverFileCap))
    );
    assert!(picture.artifact.is_none());
    let facts = manifest.facts();
    assert_eq!(facts.reasons, ["over_file_cap"]);
    assert_eq!(facts.left_out, 1);
    assert!(facts.left_out_bytes >= big.len() as u64, "{facts:?}");
    assert_eq!(facts.cap_bytes, HAND_IN_BYTES_MAX);
}

#[test]
fn a_head_ends_at_a_whole_line_so_the_cap_never_splits_a_value_the_mask_must_see() {
    let bench = Bench::open();
    let cap = usize::try_from(zerocode_core::hand_in::FILE_BYTES_MAX).expect("small");
    // The cap falls ten bytes into the line that carries a key id: a head cut
    // there would end in `token AKIA`, half a key that no mask could know.
    let mut text = String::new();
    while text.len() + 64 <= cap - 10 {
        text.push_str(&format!("{:0>63}\n", text.len() / 64));
    }
    text.push_str(&format!("{}\n", "x".repeat(cap - 10 - text.len() - 1)));
    assert_eq!(text.len(), cap - 10);
    text.push_str(&format!("token {KEY_ID} end\n"));
    text.push_str(&"z".repeat(4096));
    let log = bench.write("long.log", text.as_bytes());
    let job = bench.job("m-12", None, &[&log]);

    let manifest = ensure(&bench.store, &job, 10);
    assert_eq!(manifest.state(), State::Partial, "{manifest:?}");
    let head = &manifest.entries[0];
    assert_eq!(
        (head.outcome, head.why),
        (Outcome::Trimmed, Some(Why::OverFileCap)),
        "{manifest:?}"
    );
    let kept = std::fs::read_to_string(
        &bench
            .store
            .get(head.artifact.as_deref().expect("kept"))
            .expect("row")
            .path,
    )
    .expect("text");
    assert!(
        !kept.contains("AKIA"),
        "the head holds the half of a key: {}",
        &kept[kept.len().saturating_sub(160)..]
    );
    let said = format!(
        "x\n\n\n[trimmed: the first {} of {} bytes are kept",
        cap - 10,
        text.len()
    );
    assert!(
        kept.contains(&said),
        "the head does not end at the line before the key: {}",
        &kept[kept.len().saturating_sub(160)..]
    );
}

#[test]
fn a_head_with_no_line_and_no_space_to_end_at_is_left_out_not_kept_half_read() {
    let bench = Bench::open();
    let cap = usize::try_from(zerocode_core::hand_in::FILE_BYTES_MAX).expect("small");
    let one_word = bench.write("minified.json", "y".repeat(cap + 4096).as_bytes());
    let job = bench.job("m-13", None, &[&one_word]);

    let manifest = ensure(&bench.store, &job, 10);
    assert_eq!(manifest.entries.len(), 1, "{manifest:?}");
    let only = &manifest.entries[0];
    assert_eq!(
        (only.outcome, only.why, only.artifact.as_deref()),
        (Outcome::Left, Some(Why::OverFileCap), None),
        "{manifest:?}"
    );
    assert_eq!(manifest.state(), State::Withheld);
}

#[test]
fn what_cannot_be_checked_or_must_not_be_read_is_refused_and_says_so() {
    let bench = Bench::open();
    let report = bench.write("report.md", b"# fine\n");
    let dotenv = bench.write(".env", b"API_TOKEN=hunter2\n");
    let key = bench.write("keys/id_rsa", b"-----BEGIN-----\nsecret\n");
    let blob = bench.write("blob.bin", &[0, 159, 146, 150, 0, 1, 2]);
    let latin = bench.write("latin.txt", &[b'c', b'a', b'f', 0xE9, b'\n']);
    let outside = bench.elsewhere().join("notes.md");
    std::fs::write(&outside, "# not the worker's\n").expect("a file outside");
    let gone = bench.checkout().join("never-written.png");
    let job = bench.job(
        "m-4",
        Some(&report),
        &[&dotenv, &key, &blob, &latin, &outside, &gone],
    );

    let manifest = ensure(&bench.store, &job, 10);
    let said: Vec<(&str, Outcome, Option<Why>)> = manifest
        .entries
        .iter()
        .map(|entry| (entry.name.as_str(), entry.outcome, entry.why))
        .collect();
    assert_eq!(
        said,
        [
            ("report.md", Outcome::Kept, None),
            (".env", Outcome::Left, Some(Why::SecretName)),
            ("id_rsa", Outcome::Left, Some(Why::SecretName)),
            ("blob.bin", Outcome::Left, Some(Why::NotKept)),
            ("latin.txt", Outcome::Left, Some(Why::NotKept)),
            ("notes.md", Outcome::Left, Some(Why::OutsideRoots)),
            ("never-written.png", Outcome::Left, Some(Why::Gone)),
        ]
    );
    assert_eq!(manifest.state(), State::Partial);
    assert_eq!(
        manifest.refused, 5,
        "five refusals; a file that was already gone is not one"
    );
    // Nothing of what was refused reached the store.
    let store_text = walk_text(bench.store.root());
    for secret in ["hunter2", "secret\n", "not the worker's"] {
        assert!(!store_text.contains(secret), "{secret} reached the store");
    }
    assert!(
        Standing::of(Some(&manifest)).lets_a_cleanup_go_on(),
        "a refusal is a finished keeping"
    );
}

/// Every byte under `root` that reads as text, joined.
fn walk_text(root: &Path) -> String {
    let mut out = String::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let Ok(read) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in read.flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if let Ok(text) = std::fs::read_to_string(&path) {
                out.push_str(&text);
            }
        }
    }
    out
}

#[cfg(unix)]
#[test]
fn a_link_is_not_followed_out_of_a_folder_and_a_named_link_is_judged_where_it_points() {
    let bench = Bench::open();
    let outside = bench.elsewhere().join("outside.md");
    std::fs::write(&outside, "# outside\n").expect("outside");
    let inside = bench.write("frames/real.md", b"# inside\n");
    std::os::unix::fs::symlink(&outside, bench.checkout().join("frames/escape.md")).expect("link");
    let named_link = bench.checkout().join("report-link.md");
    std::os::unix::fs::symlink(&outside, &named_link).expect("link");
    let named_inside = bench.checkout().join("inside-link.md");
    std::os::unix::fs::symlink(&inside, &named_inside).expect("link");
    let job = bench.job(
        "m-5",
        Some(&named_link),
        &[&bench.checkout().join("frames"), &named_inside],
    );

    let manifest = ensure(&bench.store, &job, 10);
    let said: Vec<(&str, Option<Why>)> = manifest
        .entries
        .iter()
        .map(|entry| (entry.name.as_str(), entry.why))
        .collect();
    // What was kept is listed first, then what was left out, each in the order named.
    assert_eq!(
        said,
        [
            ("frames/real.md", None),
            ("report-link.md", Some(Why::OutsideRoots)),
            ("frames/escape.md", Some(Why::Link)),
        ],
        "{manifest:?}"
    );
    assert!(
        !walk_text(bench.store.root()).contains("# outside"),
        "a link led out of the checkout"
    );
}

#[test]
fn a_workers_roots_are_its_checkout_and_the_temporary_folders_and_nowhere_else() {
    let bench = Bench::open();
    let scratch = bench
        .write("scratch/notes.md", b"# notes\n")
        .canonicalize()
        .expect("a real path");
    // The crate's own folder stands for a checkout the temporary folders do not hold.
    let checkout = Path::new(env!("CARGO_MANIFEST_DIR"))
        .canonicalize()
        .expect("a real folder");
    let roots = Roots::of(checkout.to_str());
    assert!(
        roots.holds(&checkout.join("Cargo.toml")),
        "a file in the worker's checkout is outside its roots"
    );
    assert!(
        roots.holds(&scratch),
        "a file in the temporary folder is outside its roots"
    );
    let everywhere = Path::new(if cfg!(windows) { "C:\\" } else { "/" });
    assert!(
        !roots.holds(everywhere),
        "the whole machine is inside a worker's roots"
    );
    // A worker whose checkout is already gone is left with the temporary folders.
    let gone = Roots::of(Some("/nowhere/t-0"));
    assert!(
        gone.holds(&scratch),
        "the temporary folders went with the checkout"
    );
}

#[cfg(unix)]
#[test]
fn a_checkout_is_the_same_one_however_it_is_spelled() {
    let bench = Bench::open();
    let real = bench.checkout();
    // A link named like the checkout and pointing at it: the other spelling of one folder.
    let spelled = bench.elsewhere().join("checkout");
    std::os::unix::fs::symlink(&real, &spelled).expect("link");
    assert!(is_the_checkout(spelled.to_str().expect("utf-8"), &real));
    assert!(is_the_checkout(real.to_str().expect("utf-8"), &real));
    // The same last component in another folder is another checkout.
    let other = bench.elsewhere().join("other").join("checkout");
    std::fs::create_dir_all(&other).expect("another folder");
    assert!(!is_the_checkout(other.to_str().expect("utf-8"), &real));
    // And a row of another name is refused by its name alone.
    assert!(!is_the_checkout("/nowhere/t-9", &real));
}

#[test]
fn a_write_the_store_refuses_fails_the_keeping_and_a_cleanup_waits_with_the_reason() {
    let bench = Bench::open();
    let report = bench.write("report.md", b"# done\n");
    let job = bench.job("m-6", Some(&report), &[]);
    // The store's `run` bucket is a file, so no folder can be made under it.
    std::fs::create_dir_all(bench.store.root()).expect("the store");
    std::fs::write(bench.store.root().join("run"), "in the way").expect("a file in the way");

    let manifest = ensure(&bench.store, &job, 1_000);
    assert_eq!(manifest.state(), State::Failed, "{manifest:?}");
    let failure = manifest.failed.as_ref().expect("a failure");
    assert_eq!(failure.fault, zerocode_core::hand_in::Fault::CopyFailed);
    assert!(!Standing::of(Some(&manifest)).lets_a_cleanup_go_on());

    // The beat's sweep waits and says why; within the retry window it does not
    // even start another try.
    let jobs = [job.clone()];
    let (waiting, started) = clearance(Some(&bench.store), &jobs, Mode::Wait, 1_000 + 1_000);
    let Clearance::Held(reason) = waiting else {
        panic!("a failed keeping did not hold the cleanup: {waiting:?}");
    };
    assert!(
        reason.contains("m-6") && reason.contains("t-m-6") && reason.contains("copy_failed"),
        "{reason}"
    );
    assert!(started.is_empty(), "the sweep retried inside the window");
    // The person's removal retries on the spot, and holds again with the reason.
    let (asked, started) = clearance(Some(&bench.store), &jobs, Mode::Now, 1_000 + 1_000);
    assert!(matches!(asked, Clearance::Held(_)), "{asked:?}");
    assert!(started.is_empty());

    // The disk is mended: the person's removal keeps what was owed and goes on.
    std::fs::remove_file(bench.store.root().join("run")).expect("mended");
    let (cleared, _) = clearance(Some(&bench.store), &jobs, Mode::Now, 1_000 + 2_000);
    assert_eq!(cleared, Clearance::Clear);
    let kept = bench.store.hand_in("m-6").expect("a manifest");
    assert_eq!(kept.state(), State::Kept);
    // And the sweep, past the window, retries by starting a keeping.
    let mut failing = kept.clone();
    failing.fail(
        zerocode_core::hand_in::Fault::CopyFailed,
        "No space left on device",
    );
    failing.at_ms = 1_000;
    bench.store.note_hand_in(failing).expect("noted");
    let later = 1_000 + zerocode_core::hand_in::RETRY_FAILED_AFTER_MS;
    let (waiting, started) = clearance(Some(&bench.store), &jobs, Mode::Wait, later);
    assert!(matches!(waiting, Clearance::Keeping(_)), "{waiting:?}");
    assert_eq!(started.len(), 1, "the sweep did not start the retry");
}

#[test]
fn a_hand_in_not_yet_kept_is_started_by_the_sweep_and_kept_on_the_spot_for_a_person() {
    let bench = Bench::open();
    let report = bench.write("report.md", b"# done\n");
    let job = bench.job("m-7", Some(&report), &[]);
    let jobs = [job.clone()];

    // Nothing owed on a checkout that handed in nothing.
    assert_eq!(
        clearance(Some(&bench.store), &[], Mode::Wait, 10).0,
        Clearance::Clear
    );
    // The sweep reads nothing and writes nothing: it waits and starts the keeping.
    let (waiting, started) = clearance(Some(&bench.store), &jobs, Mode::Wait, 10);
    assert!(matches!(waiting, Clearance::Keeping(_)), "{waiting:?}");
    assert_eq!(started.len(), 1);
    assert!(
        bench.store.hand_in("m-7").is_none(),
        "the sweep wrote a manifest on the beat's thread"
    );
    // No store open yet: the cleanup waits, never goes on.
    assert!(matches!(
        clearance(None, &jobs, Mode::Wait, 10).0,
        Clearance::Keeping(_)
    ));
    // A hand-in already being kept is left to the thread that has it.
    let flight = Flight::enter(&job, 10).expect("the first to enter");
    assert!(
        Flight::enter(&job, 11).is_none(),
        "a second keeping of one hand-in began"
    );
    let (waiting, started) = clearance(Some(&bench.store), &jobs, Mode::Wait, 12);
    assert!(
        matches!(waiting, Clearance::Keeping(_)) && started.is_empty(),
        "{waiting:?}"
    );
    drop(flight);
    // A person asking for the removal keeps it on the spot.
    let (cleared, started) = clearance(Some(&bench.store), &jobs, Mode::Now, 20);
    assert_eq!(cleared, Clearance::Clear);
    assert!(started.is_empty());
    assert_eq!(
        bench.store.hand_in("m-7").map(|held| held.state()),
        Some(State::Kept)
    );
    // Kept and unchanged: nothing to start, nothing read again.
    let (cleared, started) = clearance(Some(&bench.store), &jobs, Mode::Wait, 30);
    assert_eq!(cleared, Clearance::Clear);
    assert!(started.is_empty());
}

#[test]
fn an_old_hand_in_is_not_owed_and_a_crowd_is_started_a_few_at_a_time() {
    let bench = Bench::open();
    let report = bench.write("report.md", b"# done\n");
    let jobs: Vec<Job> = (0..12)
        .map(|at| {
            let mut job = bench.job(&format!("m-crowd{at}"), Some(&report), &[]);
            job.hand_in.at_ms = 1_000_000 + at;
            job
        })
        .collect();
    let day = 24 * 60 * 60 * 1000;
    let now = 1_000_000 + 12;
    // The sweep starts a few, newest first, and the cleanup waits for the rest.
    let (waiting, started) = clearance(Some(&bench.store), &jobs, Mode::Wait, now);
    assert!(matches!(waiting, Clearance::Keeping(_)), "{waiting:?}");
    let order: Vec<i64> = started.iter().map(|job| job.hand_in.at_ms).collect();
    assert_eq!(
        order,
        [
            1_000_011, 1_000_010, 1_000_009, 1_000_008, 1_000_007, 1_000_006, 1_000_005, 1_000_004
        ]
    );
    // A hand-in older than the days the store keeps what it keeps is not owed a keeping.
    let mut old = bench.job("m-old", Some(&report), &[]);
    old.hand_in.at_ms = 0;
    let (cleared, started) = clearance(Some(&bench.store), &[old], Mode::Wait, 60 * day);
    assert_eq!(cleared, Clearance::Clear);
    assert!(
        started.is_empty(),
        "a hand-in past the store's days was started"
    );
    // A person asking for the removal keeps the whole crowd on the spot.
    let (cleared, started) = clearance(Some(&bench.store), &jobs, Mode::Now, now);
    assert_eq!(cleared, Clearance::Clear);
    assert!(started.is_empty());
    assert!(
        jobs.iter()
            .all(|job| bench.store.hand_in(&job.hand_in.message).is_some())
    );
}

#[test]
fn keeping_again_refreshes_what_changed_and_never_forgets_what_was_kept() {
    let bench = Bench::open();
    let report = bench.write("report.md", b"# first\n");
    let shot = bench.write("dark.png", PNG);
    let job = bench.job("m-8", Some(&report), &[&shot]);

    let first = ensure(&bench.store, &job, 10);
    assert_eq!(first.kept, 2);
    // Nothing changed: the manifest is the one written, not a new one.
    let same = ensure(&bench.store, &job, 99);
    assert_eq!((same.at_ms, &same.fingerprint), (10, &first.fingerprint));

    // The worker amends its report after it handed in: the copy follows.
    std::fs::write(&report, "# second, and longer\n").expect("amended");
    let (waiting, started) = clearance(
        Some(&bench.store),
        std::slice::from_ref(&job),
        Mode::Wait,
        100,
    );
    assert!(
        matches!(waiting, Clearance::Keeping(_)) && started.len() == 1,
        "{waiting:?}"
    );
    let second = ensure(&bench.store, &job, 100);
    assert_eq!(second.at_ms, 100);
    let id = second.facts().report.expect("a report");
    let text = std::fs::read_to_string(&bench.store.get(&id).expect("row").path).expect("text");
    assert!(text.starts_with("# second"), "{text}");

    // The checkout goes, picture first: what was kept stays kept.
    std::fs::remove_file(&shot).expect("the picture goes");
    let third = ensure(&bench.store, &job, 200);
    assert_eq!(
        third.kept, 2,
        "a kept file that is gone from its place was forgotten: {third:?}"
    );
    std::fs::remove_dir_all(bench.checkout()).expect("the cleanup");
    let fourth = ensure(&bench.store, &job, 300);
    assert_eq!(
        (fourth.at_ms, fourth.kept),
        (third.at_ms, 2),
        "nothing left to read leaves the keeping as it was"
    );
    assert!(
        bench.store.get(&id).is_some_and(|row| row.path.is_file()),
        "the kept report went with the checkout"
    );
}

#[test]
fn the_same_road_keeps_a_hand_in_whatever_agent_wrote_it() {
    for agent in ALL_AGENTS {
        let bench = Bench::open();
        let report = bench.write("report.md", report_of(256).as_bytes());
        let mut job = bench.job("m-9", Some(&report), &[]);
        job.origin.agent = Some(agent.slug().to_string());
        let manifest = ensure(&bench.store, &job, 10);
        assert_eq!(
            manifest.state(),
            State::Kept,
            "{}: {manifest:?}",
            agent.slug()
        );
        let id = manifest.facts().report.expect("a report");
        let row = bench.store.get(&id).expect("in the catalog");
        assert_eq!(row.origin.agent.as_deref(), Some(agent.slug()));
        assert_eq!(row.kind, ArtifactKind::Report, "{}", agent.slug());
    }
}

#[test]
fn the_board_shows_a_keeping_in_flight_and_then_what_it_kept() {
    let bench = Bench::open();
    let report = bench.write("report.md", b"# done\n");
    let job = bench.job("m-10", Some(&report), &[]);
    let dressed = |store: &Store| {
        let mut desk = DeskSnapshot {
            tasks: vec![desk_row("m-10")],
            ..DeskSnapshot::default()
        };
        dress_desk_with(store, &mut desk);
        desk.tasks.remove(0).kept
    };
    assert_eq!(
        dressed(&bench.store),
        None,
        "a task nobody handed anything in on carries nothing"
    );

    let flight = Flight::enter(&job, 77).expect("in flight");
    assert_eq!(dressed(&bench.store), Some(Facts::keeping(77)));
    let mut agents = Vec::new();
    dress_agents_with(&bench.store, &mut agents);
    drop(flight);

    ensure(&bench.store, &job, 80);
    let kept = dressed(&bench.store).expect("what was kept");
    assert_eq!((kept.state, kept.kept), ("kept", 1));
}

#[test]
fn an_evidence_row_keeps_the_workers_note_its_files_name_and_the_commit_it_handed_in() {
    let bench = Bench::open();
    let report = bench.write("report.md", b"# done\n");
    let red = bench.write(
        "1791083854-w-1-red-build.jsonl",
        b"{\"goal\":\"a first line that is not the name\"}\n",
    );
    let rc = bench.write("1791083854-w-1-red-build.rc", b"1\n");
    let mut job = bench.job("m-11", Some(&report), &[&red, &rc]);
    job.hand_in.commit = Some("0123abcd4567ef890123abcd4567ef890123abcd".into());
    job.origin.commit.clone_from(&job.hand_in.commit);
    job.hand_in.named.evidence[0].expected =
        Some(format!("fails at the assertion on main, for {PERSON}"));
    job.hand_in.named.evidence[0].expect = Some(Expect::Fail);
    job.hand_in.named.report_kind = Some(ReportKind::Review);

    let manifest = ensure(&bench.store, &job, 10);
    assert_eq!(manifest.state(), State::Kept, "{manifest:?}");
    let rows: Vec<_> = manifest
        .entries
        .iter()
        .map(|entry| {
            bench
                .store
                .get(entry.artifact.as_deref().expect("kept"))
                .expect("row")
        })
        .collect();
    // The log is named by its file, not by what its first line says, and the
    // worker's note rides on the row — masked like the file.
    assert_eq!(rows[1].title, "1791083854-w-1-red-build.jsonl");
    assert_eq!(rows[2].title, "1791083854-w-1-red-build.rc");
    assert_eq!(
        rows[1].description.as_deref(),
        Some(format!("fails at the assertion on main, for {MAILBOX}").as_str())
    );
    assert_eq!(manifest.entries[1].expected, rows[1].description);
    // The closed words ride in the record as the worker said them — the report's
    // kind on the report's entry, the expectation on the evidence's — and on no other.
    assert_eq!(manifest.entries[0].report_kind, Some(ReportKind::Review));
    assert_eq!(manifest.entries[1].expect, Some(Expect::Fail));
    assert!(
        manifest.entries[1..]
            .iter()
            .all(|entry| entry.report_kind.is_none())
    );
    assert_eq!(
        (manifest.entries[0].expect, manifest.entries[2].expect),
        (None, None)
    );
    assert_eq!(rows[2].description, None);
    // Every row wears what the ledger vouches for, the commit among it.
    for row in &rows {
        assert_eq!(
            row.origin.commit.as_deref(),
            Some("0123abcd4567ef890123abcd4567ef890123abcd")
        );
        assert_eq!(row.origin.agent.as_deref(), Some("claude"));
    }
    assert!(
        !walk_text(bench.store.root()).contains(PERSON),
        "the note was kept as written"
    );
}

/// This process's resident memory in KiB, from `ps` — a measurement helper, never
/// part of the shipped code. The child starts through the window's one door
/// ([`crate::proc::quiet_command`]), as every child in this crate does.
fn resident_kib() -> u64 {
    crate::proc::quiet_command("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .ok()
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .and_then(|text| text.trim().parse().ok())
        .unwrap_or(0)
}

/// A report of about `bytes` the way a real one reads: Korean and English, job
/// names, paths and numbers, and a line with each private value every forty lines.
fn realistic_report(bytes: usize) -> String {
    let mut text = String::with_capacity(bytes + 512);
    let mut line = 0usize;
    while text.len() < bytes {
        line += 1;
        match line % 4 {
            0 => text.push_str("- 빌드 줄 일감 `1791083855-w-35393-t32796-red-run` rc=0 (581s) — 헤드 8d042634d, disk-guard 통과\n"),
            1 => text.push_str("crates/zerocode-core/src/orchestration/tests.rs:6430 — the ask-wait replay seed counts the gate's receipt\n"),
            2 => text.push_str("측정: 기본 사양 429 → 578 ms (+149), 효율 코어 4,258 → 5,033 ms; RSS 평탄 7,056 → 7,104 KiB\n"),
            _ => text.push_str("Reported a clean, landed checkout holding only its build; nothing else was kept in it.\n"),
        }
        if line.is_multiple_of(40) {
            text.push_str(&format!(
                "worktree {HOME}/work/t-1 mailed {PERSON} with {KEY_ID}\n"
            ));
        }
    }
    text
}

/// Every byte under `root`, files only.
fn bytes_under(root: &Path) -> u64 {
    let mut total = 0;
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let Ok(read) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in read.flatten() {
            match entry.metadata() {
                Ok(meta) if meta.is_dir() => pending.push(entry.path()),
                Ok(meta) => total += meta.len(),
                Err(_) => {}
            }
        }
    }
    total
}

/// What one keeping costs, on a 20 KB and on a 5 MB report with a 300 KB picture beside it:
/// the wall time of the whole of it (stat, read, mask, write and sync, manifest), of the
/// mask alone and of a cleanup's stat question, and the bytes it put on disk. A measurement, not
/// a check: run on purpose, normally and under `taskpolicy -b` (set `KEEP_PROFILE` to say which):
/// `cargo test -p zerocode-shell --bin zerocode-shell hand_in_keep::tests::measure -- --ignored --nocapture`.
/// A debug build, so every number is an upper bound of what the shipped build spends.
#[test]
#[ignore = "a measurement, run on purpose: -- --ignored --nocapture"]
fn measure_one_keeping_of_a_small_and_a_large_report() {
    use std::time::Instant;
    const ROUNDS: usize = 7;
    let profile = std::env::var("KEEP_PROFILE").unwrap_or_else(|_| "normal".to_string());
    let median = |mut times: Vec<f64>| {
        times.sort_by(f64::total_cmp);
        times[times.len() / 2]
    };
    let millis = |began: Instant| began.elapsed().as_secs_f64() * 1_000.0;
    for (label, size) in [("20KB", 20 * 1024), ("5MB", 5 * 1024 * 1024)] {
        let (mut whole, mut mask_alone, mut stat_ask) = (Vec::new(), Vec::new(), Vec::new());
        let (mut kept_bytes, mut disk_bytes, mut manifest_line) = (0u64, 0u64, 0usize);
        let mut values = 0;
        for round in 0..ROUNDS {
            let bench = Bench::open();
            let text = realistic_report(size);
            let report = bench.write("report.md", text.as_bytes());
            let mut picture = PNG.to_vec();
            picture.extend(std::iter::repeat_n(7u8, 300 * 1024));
            let shot = bench.write("dark.png", &picture);
            let job = bench.job(&format!("m-m{round}"), Some(&report), &[&shot]);
            mask_alone.push({
                let from = Instant::now();
                std::hint::black_box(zerocode_core::private_data::mask(&text));
                millis(from)
            });
            let began = Instant::now();
            let manifest = ensure(&bench.store, &job, 10);
            whole.push(millis(began));
            assert_eq!(manifest.state(), State::Kept, "{manifest:?}");
            values = manifest.masked.total();
            kept_bytes = manifest.kept_bytes;
            manifest_line = serde_json::to_string(&manifest).expect("serializes").len();
            disk_bytes = bytes_under(bench.store.root());
            let began = Instant::now();
            std::hint::black_box(clearance(
                Some(&bench.store),
                std::slice::from_ref(&job),
                Mode::Wait,
                20,
            ));
            stat_ask.push(millis(began));
        }
        eprintln!(
            "HAND_IN_KEEP_NUMBERS profile={profile} report={label} rounds={ROUNDS} ensure_ms_p50={:.2} ensure_ms_max={:.2} mask_ms_p50={:.2} cleanup_stat_question_ms_p50={:.3} values_masked={values} kept_bytes={kept_bytes} disk_bytes={disk_bytes} manifest_line_bytes={manifest_line}",
            median(whole.clone()),
            whole.iter().copied().fold(0.0, f64::max),
            median(mask_alone),
            median(stat_ask),
        );
    }
}

/// What a window left open for a long run holds: twelve hundred hand-ins kept one after
/// another — more than the book's thousand — with the resident size after every four hundred,
/// the book's size, and the time one keeping takes at the start and at the end. The numbers
/// should stop rising after the first step, and the book hold at its cap.
#[test]
#[ignore = "a measurement, run on purpose: -- --ignored --nocapture"]
fn measure_a_long_run_of_keepings() {
    use std::time::Instant;
    const HAND_INS: usize = 1_200;
    let profile = std::env::var("KEEP_PROFILE").unwrap_or_else(|_| "normal".to_string());
    let bench = Bench::open();
    let text = realistic_report(2 * 1024);
    let started_rss = resident_kib();
    let mut slice_ms = Vec::new();
    let mut rss = vec![("start", started_rss)];
    let mut began = Instant::now();
    for at in 0..HAND_INS {
        let report = bench.write(&format!("r{at}.md"), text.as_bytes());
        let job = bench.job(&format!("m-long{at}"), Some(&report), &[]);
        assert_eq!(
            ensure(&bench.store, &job, 10 + at as i64).state(),
            State::Kept
        );
        if (at + 1) % 400 == 0 {
            slice_ms.push(began.elapsed().as_secs_f64() * 1_000.0 / 400.0);
            rss.push((["400", "800", "1200"][(at + 1) / 400 - 1], resident_kib()));
            began = Instant::now();
        }
    }
    let held = bench.store.hand_ins().len();
    eprintln!(
        "HAND_IN_KEEP_LONG profile={profile} hand_ins={HAND_INS} book_held={held} rss_kib={rss:?} ms_per_keeping_by_400={slice_ms:.2?} store_disk_bytes={}",
        bytes_under(bench.store.root())
    );
}

/// What the board's beat pays to dress its rows: a desk of two hundred and forty rows
/// against a book of a thousand manifests, p50 and p95 over a thousand beats, and how many
/// bytes the facts add to the desk the window serializes.
#[test]
#[ignore = "a measurement, run on purpose: -- --ignored --nocapture"]
fn measure_dressing_the_desk_each_beat() {
    use std::time::Instant;
    let profile = std::env::var("KEEP_PROFILE").unwrap_or_else(|_| "normal".to_string());
    let dir = tempfile::tempdir().expect("a scratch directory");
    let store = Store::open(dir.path(), Limits::default());
    for at in 0..1_000 {
        let mut manifest = zerocode_core::hand_in::Manifest::begin(
            &HandIn {
                run: "run-m".into(),
                message: format!("m-{at}"),
                task: Some(format!("t-{at}")),
                dispatch: None,
                worker: format!("w-{}", at % 7),
                at_ms: at,
                commit: None,
                named: Named::default(),
            },
            "p".into(),
            at,
        );
        manifest.push(zerocode_core::hand_in::Entry {
            name: "report.md".into(),
            role: Role::Report,
            outcome: Outcome::Kept,
            kept_bytes: 3,
            source_bytes: 3,
            artifact: Some(format!("a-{at}")),
            ..zerocode_core::hand_in::Entry::default()
        });
        store.note_hand_in(manifest).expect("noted");
    }
    let rows = |dressed: bool| {
        let mut desk = DeskSnapshot {
            tasks: (0..240)
                .map(|at| DeskTask {
                    run: "run-m".into(),
                    id: format!("t-{}", 1_000 - 1 - at),
                    ..desk_row("m-0")
                })
                .collect(),
            ..DeskSnapshot::default()
        };
        if dressed {
            dress_desk_with(&store, &mut desk);
        }
        desk
    };
    let (mut beats, mut bytes_with, mut bytes_without) = (Vec::new(), 0, 0);
    for _ in 0..1_000 {
        let mut desk = rows(false);
        let began = Instant::now();
        dress_desk_with(&store, &mut desk);
        beats.push(began.elapsed().as_secs_f64() * 1_000_000.0);
        bytes_with = serde_json::to_string(&desk).expect("serializes").len();
        bytes_without = serde_json::to_string(&rows(false))
            .expect("serializes")
            .len();
    }
    beats.sort_by(f64::total_cmp);
    eprintln!(
        "HAND_IN_KEEP_DRESS profile={profile} rows=240 book=1000 dress_us_p50={:.1} dress_us_p95={:.1} desk_json_bytes_without={bytes_without} desk_json_bytes_with={bytes_with}",
        beats[beats.len() / 2],
        beats[beats.len() * 95 / 100],
    );
}
