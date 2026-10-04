//! The hand-in half of the artifact store (t-32798): the kept copy of what a
//! worker's `worker_done` names, and the manifest that says what became of every
//! file.
//!
//! Today the store keeps no hand-in: a report is copied verbatim by
//! `register_report` and nothing records what was named. This is the shape the
//! keeping asks of the store, with a book that holds nothing and a store that
//! keeps no file. The tests below fail on that.

use zerocode_core::hand_in::{Facts, Manifest, Role};

use super::*;

/// The manifests, beside the catalog.
const HAND_INS_FILE: &str = "hand-ins.jsonl";

/// How many manifests the store holds. A thousand hand-ins is about three weeks
/// of the busiest ledger this machine has run; each is a few hundred bytes to a
/// few kilobytes, so the book never holds more than a few megabytes however long
/// the window stays open. The oldest give their seats away.
const HAND_INS_MAX: usize = 1000;

/// How many superseded or unread lines the file may carry before it is rewritten
/// whole: a keeping that is redone appends a line, and the file is compacted when
/// it holds this many more lines than manifests.
const HAND_INS_SLACK: usize = 256;

/// A manifest, and the facts counted from it once.
struct Held {
    manifest: Manifest,
    facts: Facts,
}

/// Every manifest the store holds, with the two ways a board asks for one: by the
/// task, and by the worker. Pointers, not copies, and both by the newest manifest.
#[derive(Default)]
pub(crate) struct HandIns {
    by_message: HashMap<String, Held>,
    /// run → task → the message of its newest manifest.
    by_task: HashMap<String, HashMap<String, String>>,
    /// run → worker → the message of its newest manifest.
    by_worker: HashMap<String, HashMap<String, String>>,
    /// Lines the file holds: live ones, superseded ones and ones nobody read.
    lines: usize,
}

impl HandIns {
    /// Nothing is read.
    pub(super) fn load(_root: &Path) -> Self {
        Self::default()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.by_message.is_empty()
    }

    /// How many manifests the book holds.
    pub(crate) fn len(&self) -> usize {
        self.by_message.len()
    }

    /// No task has a keeping.
    pub(crate) fn facts_of_task(&self, _run: &str, _task: &str) -> Option<&Facts> {
        None
    }

    /// No task has a record.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "the per-task view (t-36910) is its reader")
    )]
    pub(crate) fn of_task(&self, _run: &str, _task: &str) -> Vec<&Manifest> {
        Vec::new()
    }

    /// No worker has a keeping.
    pub(crate) fn facts_of_worker(&self, _run: &str, _worker: &str) -> Option<&Facts> {
        None
    }

    /// A manifest is not held.
    fn insert(&mut self, _manifest: Manifest) {}

    /// The book never gives a seat away.
    fn cap_to(&mut self, _max: usize) -> bool {
        false
    }
}

/// What a keeping hands the store for one file: its name, whose it is, and its
/// bytes — the masked text, or a picture as it was.
pub(crate) struct KeptFile<'a> {
    /// One path component, as [`zerocode_core::hand_in::kept_name`] makes it.
    pub(crate) name: &'a str,
    pub(crate) role: Role,
    pub(crate) bytes: &'a [u8],
    /// The path the payload named it by — what the row's id is made from, so the
    /// same file kept again is the same row.
    pub(crate) identity: &'a Path,
    pub(crate) origin: &'a Origin,
    /// What the worker said to expect of the file — the row's description.
    pub(crate) note: Option<&'a str>,
}

impl Store {
    /// The manifests, for a reader that asks many rows at once under one lock.
    pub(crate) fn hand_ins(&self) -> std::sync::MutexGuard<'_, HandIns> {
        self.hand_ins.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// No manifest is kept.
    pub(crate) fn hand_in(&self, _message: &str) -> Option<Manifest> {
        None
    }

    /// A manifest is not written.
    pub(crate) fn note_hand_in(&self, _manifest: Manifest) -> Result<(), String> {
        Ok(())
    }

    /// No manifest goes on its own age.
    pub(super) fn prune_hand_ins(&self, _horizon_ms: i64) -> usize {
        0
    }

    /// No file is kept.
    pub(crate) fn register_kept(
        &self,
        file: &KeptFile<'_>,
        _now_ms: i64,
    ) -> Result<Artifact, String> {
        Err(format!("a hand-in is not kept: {}", file.name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zerocode_core::hand_in::{Entry, HandIn, Named, Outcome};

    fn manifest(message: &str, task: Option<&str>, worker: &str, at_ms: i64) -> Manifest {
        let hand_in = HandIn {
            run: "run-1".into(),
            message: message.into(),
            task: task.map(str::to_string),
            dispatch: None,
            worker: worker.into(),
            at_ms,
            commit: None,
            named: Named::default(),
        };
        let mut manifest = Manifest::begin(&hand_in, format!("print-{message}"), at_ms);
        manifest.push(Entry {
            name: "report.md".into(),
            role: Role::Report,
            outcome: Outcome::Kept,
            kept_bytes: 3,
            source_bytes: 3,
            artifact: Some(format!("a-{message}")),
            ..Entry::default()
        });
        manifest
    }

    fn origin() -> Origin {
        Origin {
            run: Some("run-1".into()),
            task: Some("t-1".into()),
            worker: Some("w-1".into()),
            agent: Some("claude".into()),
            ..Origin::default()
        }
    }

    #[test]
    fn a_kept_file_is_a_row_the_catalog_opens_and_keeping_it_again_is_the_same_row() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let store = Store::open(dir.path(), Limits::default());
        let origin = origin();
        let kept = |role, name: &str, bytes: &[u8], identity: &str| {
            let said = store.register_kept(
                &KeptFile {
                    name,
                    role,
                    bytes,
                    identity: Path::new(identity),
                    origin: &origin,
                    note: None,
                },
                10,
            );
            assert!(said.is_ok(), "{name} was not kept: {said:?}");
            said.unwrap_or_else(|why| panic!("{name} was not kept: {why}"))
        };
        let report = kept(Role::Report, "report.md", b"# one\n", "/w/report.md");
        let shot = kept(
            Role::Evidence,
            "dark.png",
            b"\x89PNG\r\n\x1a\n",
            "/w/dark.png",
        );
        let note = kept(Role::Evidence, "run.log", b"line\n", "/w/run.log");
        assert_eq!(report.kind, ArtifactKind::Report);
        assert_eq!(shot.kind, ArtifactKind::Screenshot);
        assert_eq!(
            note.kind,
            ArtifactKind::Evidence,
            "evidence is evidence, not a report"
        );
        assert_eq!(report.source, Source::WorkerReport);
        assert_eq!(shot.source, Source::WorkerEvidence);
        assert_eq!(std::fs::read(&report.path).expect("the copy"), b"# one\n");
        assert!(report.path.starts_with(store.root()), "{:?}", report.path);
        assert_eq!(report.origin.task.as_deref(), Some("t-1"));

        let again = kept(Role::Report, "report.md", b"# two\n", "/w/report.md");
        assert_eq!(again.id, report.id, "the same file kept again is one row");
        assert_eq!(std::fs::read(&again.path).expect("the copy"), b"# two\n");
        assert_eq!(store.counts().by_task.get("t-1"), Some(&3));
    }

    #[test]
    fn a_manifest_is_found_by_its_message_its_task_and_its_worker_and_the_newest_wins() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let store = Store::open(dir.path(), Limits::default());
        store
            .note_hand_in(manifest("m-1", Some("t-1"), "w-1", 10))
            .expect("noted");
        store
            .note_hand_in(manifest("m-2", Some("t-1"), "w-2", 20))
            .expect("noted");
        store
            .note_hand_in(manifest("m-0", Some("t-1"), "w-1", 5))
            .expect("noted");
        assert_eq!(store.hand_in("m-2").map(|held| held.at_ms), Some(20));
        assert_eq!(store.hand_in("m-9"), None);
        let book = store.hand_ins();
        assert_eq!(
            book.facts_of_task("run-1", "t-1")
                .and_then(|facts| facts.report.clone())
                .as_deref(),
            Some("a-m-2"),
            "the task's newest hand-in answers for it, whatever order they were written"
        );
        assert_eq!(
            book.facts_of_worker("run-1", "w-1")
                .and_then(|facts| facts.report.clone())
                .as_deref(),
            Some("a-m-1")
        );
        assert!(book.facts_of_task("run-1", "t-nope").is_none());
        assert!(book.facts_of_worker("run-2", "w-1").is_none());
    }

    #[test]
    fn every_hand_in_record_of_a_task_is_read_newest_first_and_only_its_own() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let store = Store::open(dir.path(), Limits::default());
        store
            .note_hand_in(manifest("m-1", Some("t-1"), "w-1", 10))
            .expect("noted");
        store
            .note_hand_in(manifest("m-3", Some("t-1"), "w-2", 30))
            .expect("noted");
        store
            .note_hand_in(manifest("m-2", Some("t-1"), "w-1", 20))
            .expect("noted");
        store
            .note_hand_in(manifest("m-4", Some("t-2"), "w-1", 40))
            .expect("noted");
        store
            .note_hand_in(manifest("m-5", None, "w-1", 50))
            .expect("noted");
        let book = store.hand_ins();
        let messages = |run: &str, task: &str| -> Vec<String> {
            book.of_task(run, task)
                .iter()
                .map(|held| held.message.clone())
                .collect()
        };
        assert_eq!(messages("run-1", "t-1"), ["m-3", "m-2", "m-1"]);
        assert_eq!(messages("run-1", "t-2"), ["m-4"]);
        assert!(messages("run-1", "t-nope").is_empty());
        assert!(
            messages("run-2", "t-1").is_empty(),
            "another run's task of the same name is not this one's"
        );
    }

    #[test]
    fn manifests_survive_a_reopen_and_a_torn_last_line_is_mended() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        {
            let store = Store::open(dir.path(), Limits::default());
            store
                .note_hand_in(manifest("m-1", Some("t-1"), "w-1", 10))
                .expect("noted");
            store
                .note_hand_in(manifest("m-2", Some("t-2"), "w-1", 20))
                .expect("noted");
        }
        let file = dir.path().join(STORE_DIR_NAME).join(HAND_INS_FILE);
        assert!(file.is_file(), "no manifest was written to {file:?}");
        let whole = std::fs::read_to_string(&file).expect("the file");
        assert_eq!(whole.lines().count(), 2);
        std::fs::write(&file, format!("{whole}{{\"message\":\"m-torn\",\"run")).expect("torn");
        let reopened = Store::open(dir.path(), Limits::default());
        assert!(reopened.hand_in("m-1").is_some() && reopened.hand_in("m-2").is_some());
        assert!(reopened.hand_in("m-torn").is_none());
        let mended = std::fs::read_to_string(&file).expect("the file");
        assert!(
            mended.ends_with('\n'),
            "the torn tail was left for the next append to glue to"
        );
        assert_eq!(mended.lines().count(), 2, "{mended}");
        reopened
            .note_hand_in(manifest("m-3", None, "w-3", 30))
            .expect("noted");
        assert_eq!(
            std::fs::read_to_string(&file)
                .expect("the file")
                .lines()
                .count(),
            3
        );
    }

    #[test]
    fn the_book_holds_so_many_and_the_oldest_give_way() {
        let mut book = HandIns::default();
        for at in 0..12 {
            book.insert(manifest(
                &format!("m-{at}"),
                Some(&format!("t-{at}")),
                "w-1",
                i64::from(at),
            ));
        }
        assert!(book.cap_to(10), "two were over");
        assert_eq!(book.by_message.len(), 10);
        assert!(
            book.facts_of_task("run-1", "t-0").is_none(),
            "the oldest kept its seat"
        );
        assert!(book.facts_of_task("run-1", "t-1").is_none());
        assert!(book.facts_of_task("run-1", "t-11").is_some());
        assert_eq!(
            book.facts_of_worker("run-1", "w-1")
                .and_then(|facts| facts.report.clone())
                .as_deref(),
            Some("a-m-11"),
            "the pointers follow the manifests that stayed"
        );
        assert!(!book.cap_to(10), "nothing more to give");
    }

    #[test]
    fn a_file_holding_more_than_the_book_is_cut_to_it_at_boot() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let root = dir.path().join(STORE_DIR_NAME);
        std::fs::create_dir_all(&root).expect("the store folder");
        let mut text = String::new();
        for at in 0..HAND_INS_MAX + 5 {
            let said = serde_json::to_string(&manifest(
                &format!("m-{at}"),
                Some(&format!("t-{at}")),
                "w-1",
                i64::try_from(at).expect("small"),
            ))
            .expect("serializes");
            text.push_str(&said);
            text.push('\n');
        }
        std::fs::write(root.join(HAND_INS_FILE), text).expect("the file");
        let store = Store::open(dir.path(), Limits::default());
        assert_eq!(store.hand_ins().by_message.len(), HAND_INS_MAX);
        assert!(store.hand_in("m-0").is_none() && store.hand_in("m-1004").is_some());
        let lines = std::fs::read_to_string(root.join(HAND_INS_FILE))
            .expect("the file")
            .lines()
            .count();
        assert_eq!(lines, HAND_INS_MAX, "the file was cut to the book at boot");
    }

    #[test]
    fn the_retention_sweep_takes_old_manifests_and_leaves_new_ones() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let store = Store::open(dir.path(), Limits::default());
        store
            .note_hand_in(manifest("m-old", Some("t-1"), "w-1", 1_000))
            .expect("noted");
        store
            .note_hand_in(manifest("m-new", Some("t-2"), "w-2", 9_000_000_000))
            .expect("noted");
        let day = 24 * 60 * 60 * 1000;
        let pruned = store.prune(9_000_000_000 + 40 * day, 30);
        assert_eq!(
            pruned.rows, 0,
            "no row was old; the manifest went on its own age"
        );
        assert!(store.hand_in("m-old").is_none());
        assert!(
            store.hand_in("m-new").is_none(),
            "forty days on, both are old"
        );
        store
            .note_hand_in(manifest(
                "m-fresh",
                Some("t-3"),
                "w-3",
                9_000_000_000 + 39 * day,
            ))
            .expect("noted");
        store.prune(9_000_000_000 + 40 * day, 30);
        assert!(
            store.hand_in("m-fresh").is_some(),
            "a hand-in inside the days stays"
        );
        let reopened = Store::open(dir.path(), Limits::default());
        assert!(reopened.hand_in("m-fresh").is_some() && reopened.hand_in("m-old").is_none());
    }
}
