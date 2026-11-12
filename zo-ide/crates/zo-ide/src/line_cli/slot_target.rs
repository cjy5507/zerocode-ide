//! The folder a line job's cargo builds in (`CARGO_TARGET_DIR`): one a slot, under zo's home beside the
//! slots' locks, kept from job to job, so a registry crate is compiled once a slot rather than once a
//! checkout, and the machine holds as many build folders as it has slots, not one a worktree (2026-11-12:
//! from-scratch builds in a dozen worktrees took the disk under 20 GiB four times in one day).
//!
//! What it keeps from one job to the next is only what two checkouts cannot differ in. A workspace's
//! own crates are named by their path inside the workspace, so the same crate of two worktrees lands on
//! the same files, and cargo calls them fresh by mtime alone: a worktree whose files were older than the
//! last build linked the other worktree's code (2026-11-12, a zo built against another tree's
//! core-types). A build script's watch list is kept as absolute paths, so touching a crate root does not
//! run it again either: zo's build stamp named the worktree built before. So before a job and after it,
//! the line has cargo remove the workspace crates of the job's checkout from the folder
//! (`cargo clean --workspace`, for each profile and target the folder holds), and every one of them is
//! built from the job's own files; a profile its checkout cannot name is removed whole.
//!
//! A job whose environment names its own `CARGO_TARGET_DIR` — the release lane's gates, lane-env, a
//! script that wants its build afterwards — builds there, untouched; so does a job put in from a folder
//! with no cargo workspace, which gets no folder at all. What a job built stays in its slot's folder only
//! while the job runs: whoever wants a binary afterwards names their own folder.

use std::ffi::OsString;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The slots' build folders, beside their locks.
const FOLDER: &str = "targets";
/// A folder the line let go of: renamed under the line's lock, removed outside it.
const GONE: &str = ".gone-";
/// How far above its disk floor the line empties idle slots' folders before a job starts: the
/// coordinator's own alarm sat 4 GiB above the 16 GiB floor on 2026-11-12, when four of them came.
pub(super) const ROOM_ABOVE_FLOOR_GIB: u64 = 4;
/// The variables the line sets on a job building in a slot's folder.
const TARGET_DIR: &str = "CARGO_TARGET_DIR";
const INCREMENTAL: &str = "CARGO_INCREMENTAL";
/// Folders a workspace manifest is never looked for in.
const NOT_SOURCE: [&str; 2] = ["target", "node_modules"];

/// Slot `slot`'s build folder under the slots' folder `slots`.
pub(super) fn folder(slots: &Path, slot: u32) -> PathBuf {
    slots.join(FOLDER).join(slot.to_string())
}

/// What the line sets on a job building in `folder`, read against the job's own environment `own`:
/// nothing when the job names its own `CARGO_TARGET_DIR`. `CARGO_INCREMENTAL` is 0 unless the job names
/// it: the incremental state of crates removed after every job is never read again.
pub(super) fn env_for(own: &dyn Fn(&str) -> Option<OsString>, folder: &Path) -> Option<Vec<(&'static str, OsString)>> {
    if own(TARGET_DIR).is_some() {
        return None;
    }
    let mut set = vec![(TARGET_DIR, folder.as_os_str().to_owned())];
    if own(INCREMENTAL).is_none() {
        set.push((INCREMENTAL, OsString::from("0")));
    }
    Some(set)
}

/// The cargo workspaces a job put in from `checkout` builds: each `Cargo.toml` with a `[workspace]` at
/// the root of the checkout's repository (the checkout itself outside one) and in the folders right
/// under it, as this repository's `zo-ide/` is.
pub(super) fn workspaces(checkout: &Path) -> Vec<PathBuf> {
    let root = checkout.ancestors().find(|folder| folder.join(".git").exists()).unwrap_or(checkout);
    let mut found: Vec<PathBuf> = std::iter::once(root.join("Cargo.toml"))
        .chain(
            std::fs::read_dir(root)
                .into_iter()
                .flatten()
                .filter_map(Result::ok)
                .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
                .filter(|entry| {
                    let name = entry.file_name();
                    let name = name.to_string_lossy();
                    !name.starts_with('.') && !NOT_SOURCE.contains(&name.as_ref())
                })
                .map(|entry| entry.path().join("Cargo.toml")),
        )
        .filter(|manifest| names_a_workspace(manifest))
        .collect();
    found.sort();
    found
}

fn names_a_workspace(manifest: &Path) -> bool {
    std::fs::read_to_string(manifest).is_ok_and(|text| {
        text.lines().map(str::trim).any(|line| line == "[workspace]" || line.starts_with("[workspace."))
    })
}

/// One build the folder holds: a target triple's (none for the host) and a profile's folder.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Build {
    triple: Option<String>,
    profile: String,
}

impl Build {
    fn path(&self, folder: &Path) -> PathBuf {
        self.triple.as_ref().map_or_else(|| folder.to_path_buf(), |triple| folder.join(triple)).join(&self.profile)
    }

    /// The words that name it to `cargo clean`: the dev profile builds in `debug`.
    fn words(&self) -> Vec<String> {
        let mut words = Vec::new();
        if let Some(triple) = &self.triple {
            words.extend(["--target".to_owned(), triple.clone()]);
        }
        match self.profile.as_str() {
            "debug" => {}
            "release" => words.push("--release".to_owned()),
            other => words.extend(["--profile".to_owned(), other.to_owned()]),
        }
        words
    }
}

/// A profile's folder is one cargo keeps its freshness records in.
fn is_profile(path: &Path) -> bool {
    path.join(".fingerprint").is_dir()
}

fn folders_in(path: &Path) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(path)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .map(|entry| entry.path())
        .collect();
    found.sort();
    found
}

fn name_of(path: &Path) -> String {
    path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default()
}

/// The builds `folder` holds: `debug`, `release`, a named profile's folder, and the same under a
/// target triple's folder.
fn builds(folder: &Path) -> Vec<Build> {
    let mut found = Vec::new();
    for path in folders_in(folder) {
        if is_profile(&path) {
            found.push(Build { triple: None, profile: name_of(&path) });
            continue;
        }
        for inner in folders_in(&path).into_iter().filter(|inner| is_profile(inner)) {
            found.push(Build { triple: Some(name_of(&path)), profile: name_of(&inner) });
        }
    }
    found
}

/// Has cargo remove the workspace crates of `manifests` from `folder`, for each build it holds; a build
/// one of them cannot name is removed whole. Says how many builds went whole; Err when cargo could not
/// be started at all.
pub(super) fn clean(folder: &Path, manifests: &[PathBuf]) -> Result<usize, String> {
    let mut whole = 0;
    for build in builds(folder) {
        let mut named = true;
        for manifest in manifests {
            let status = Command::new("cargo")
                .args(["clean", "--workspace", "--offline", "--quiet", "--manifest-path"])
                .arg(manifest)
                .arg("--target-dir")
                .arg(folder)
                .args(build.words())
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .map_err(|error| format!("cargo did not start: {error}"))?;
            named &= status.success();
        }
        if !named {
            let _ = std::fs::remove_dir_all(build.path(folder));
            whole += 1;
        }
    }
    Ok(whole)
}

/// Readies slot `slot`'s folder for a job put in from `checkout`, read against the job's environment
/// `own`: its workspace crates removed, and what to set on the job. None when the job builds where it
/// would without the line — it names its own folder, its checkout holds no workspace, or cargo cannot
/// be started. What it did is said on `log`.
pub(super) fn ready(
    slots: &Path,
    slot: u32,
    checkout: &Path,
    own: &dyn Fn(&str) -> Option<OsString>,
    mut log: &std::fs::File,
) -> Option<Ready> {
    let folder = folder(slots, slot);
    let set = env_for(own, &folder)?;
    let manifests = workspaces(checkout);
    if manifests.is_empty() {
        return None;
    }
    std::fs::create_dir_all(&folder).ok()?;
    let note = match clean(&folder, &manifests) {
        Ok(0) => format!("zo line: cargo builds in {} — the workspace crates of {} removed from it first", folder.display(), said(&manifests)),
        Ok(whole) => format!(
            "zo line: cargo builds in {} — the workspace crates of {} removed from it first, {whole} build(s) they cannot name removed whole",
            folder.display(),
            said(&manifests)
        ),
        Err(error) => {
            let _ = writeln!(log, "zo line: cargo builds where the job says, not in slot {slot}'s folder — {error}");
            return None;
        }
    };
    let _ = writeln!(log, "{note}");
    Some(Ready { folder, manifests, set })
}

fn said(manifests: &[PathBuf]) -> String {
    manifests.iter().map(|manifest| manifest.display().to_string()).collect::<Vec<_>>().join(", ")
}

/// A slot's folder readied for one job.
pub(super) struct Ready {
    folder: PathBuf,
    manifests: Vec<PathBuf>,
    set: Vec<(&'static str, OsString)>,
}

impl Ready {
    /// Sets the job's build folder on its command.
    pub(super) fn apply(&self, command: &mut Command) {
        for (name, value) in &self.set {
            command.env(name, value);
        }
    }

    /// After the job: its workspace crates leave the folder, which keeps only what the next job can use.
    pub(super) fn done(self) {
        let _ = clean(&self.folder, &self.manifests);
    }
}

/// Lets the folders of idle slots `idle` go: renamed now (the caller holds the line's lock and these
/// slots), removed by `sweep`. Says how many went.
pub(super) fn let_idle_go(slots: &Path, idle: &[u32], now_ms: i64) -> usize {
    let mut went = 0;
    for slot in idle {
        let from = folder(slots, *slot);
        if from.is_dir() && std::fs::rename(&from, slots.join(FOLDER).join(format!("{GONE}{slot}-{now_ms}"))).is_ok() {
            went += 1;
        }
    }
    went
}

/// Removes the folders the line let go of.
pub(super) fn sweep(slots: &Path) {
    for path in folders_in(&slots.join(FOLDER)).into_iter().filter(|path| name_of(path).starts_with(GONE)) {
        let _ = std::fs::remove_dir_all(path);
    }
}

/// Whether `free` bytes stand within the room above a `floor_gib` floor where idle slots' folders go.
pub(super) fn near_floor(free: Option<u64>, floor_gib: u64) -> bool {
    free.is_some_and(|free| free < floor_gib.saturating_add(ROOM_ABOVE_FLOOR_GIB).saturating_mul(1 << 30))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn none(_: &str) -> Option<OsString> {
        None
    }

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("folders");
        std::fs::write(path, text).expect("written");
    }

    /// A job that names its own build folder gets nothing from the line; one that does not builds in its
    /// slot's folder, without incremental state unless it asks for it.
    #[test]
    fn a_job_that_names_its_own_folder_keeps_it_and_one_that_does_not_builds_in_its_slots() {
        let slot = Path::new("/zo/build-slots/targets/1");
        assert_eq!(env_for(&|name| (name == TARGET_DIR).then(|| OsString::from("/lane/target")), slot), None);
        assert_eq!(
            env_for(&none, slot),
            Some(vec![(TARGET_DIR, OsString::from("/zo/build-slots/targets/1")), (INCREMENTAL, OsString::from("0"))])
        );
        assert_eq!(
            env_for(&|name| (name == INCREMENTAL).then(|| OsString::from("1")), slot),
            Some(vec![(TARGET_DIR, OsString::from("/zo/build-slots/targets/1"))])
        );
        assert_eq!(folder(Path::new("/zo/build-slots"), 1), slot);
    }

    /// The workspaces are read from the repository's root and the folders right under it, whatever
    /// folder of it the job was put in from; a crate's own manifest and one under `node_modules` are not.
    #[test]
    fn the_workspaces_are_the_repositorys_root_and_the_folders_under_it() {
        let repo = tempfile::tempdir().expect("a checkout");
        let root = repo.path();
        std::fs::create_dir_all(root.join(".git")).expect("a repository");
        write(&root.join("Cargo.toml"), "[workspace]\nmembers = [\"crates/a\"]\n");
        write(&root.join("crates/a/Cargo.toml"), "[package]\nname = \"a\"\n");
        write(&root.join("zo-ide/Cargo.toml"), "[workspace.package]\nedition = \"2024\"\n");
        write(&root.join("node_modules/x/Cargo.toml"), "[workspace]\n");
        write(&root.join("tools/Cargo.toml"), "[package]\nname = \"tools\"\n");
        let expected = vec![root.join("Cargo.toml"), root.join("zo-ide/Cargo.toml")];
        assert_eq!(workspaces(root), expected);
        assert_eq!(workspaces(&root.join("crates/a")), expected, "put in from a folder inside it");
        let bare = tempfile::tempdir().expect("a folder");
        assert!(workspaces(bare.path()).is_empty());
    }

    /// The builds a folder holds, named as `cargo clean` names them.
    #[test]
    fn the_builds_of_a_folder_are_named_as_cargo_clean_names_them() {
        let made = tempfile::tempdir().expect("a folder");
        for build in ["debug", "release", "fast-release", "x86_64-pc-windows-msvc/debug"] {
            std::fs::create_dir_all(made.path().join(build).join(".fingerprint")).expect("a build");
        }
        for other in ["doc/foo_bar", "tmp/x"] {
            std::fs::create_dir_all(made.path().join(other)).expect("a folder that is no build");
        }
        let words: Vec<Vec<String>> = builds(made.path()).iter().map(Build::words).collect();
        assert_eq!(
            words,
            vec![
                vec![],
                vec!["--profile".to_owned(), "fast-release".to_owned()],
                vec!["--release".to_owned()],
                vec!["--target".to_owned(), "x86_64-pc-windows-msvc".to_owned()],
            ]
        );
    }

    /// Cargo itself removes the workspace's crates — their freshness records (what makes cargo build them
    /// again), libraries and incremental state, every hash of them, in every build — and leaves a
    /// registry crate's; a build the workspace cannot name goes whole. A cargo whose `clean` stopped
    /// doing so turns this red.
    #[test]
    fn cargo_removes_the_workspace_crates_from_every_build_and_keeps_the_registrys() {
        let repo = tempfile::tempdir().expect("a checkout");
        let root = repo.path();
        write(&root.join("Cargo.toml"), "[workspace]\nmembers = [\"crates/foo-bar\"]\nresolver = \"2\"\n");
        write(&root.join("crates/foo-bar/Cargo.toml"), "[package]\nname = \"foo-bar\"\nversion = \"0.1.0\"\nedition = \"2021\"\n");
        write(&root.join("crates/foo-bar/src/lib.rs"), "pub fn one() {}\n");
        let slot = tempfile::tempdir().expect("a slot's folder");
        let builds = ["debug", "release", "x86_64-pc-windows-msvc/debug", "fast-release"];
        let ours = [".fingerprint/foo-bar-1111/lib", ".fingerprint/foo-bar-2222/run", "deps/libfoo_bar-1111.rlib", "incremental/foo_bar-5555/s"];
        let registry = [".fingerprint/serde-3333/lib", "deps/libserde-3333.rlib"];
        for build in builds {
            for file in ours.iter().chain(&registry) {
                write(&slot.path().join(build).join(file), "");
            }
        }
        assert_eq!(clean(slot.path(), &[root.join("Cargo.toml")]), Ok(1), "fast-release is no profile of this workspace");
        assert!(!slot.path().join("fast-release").exists(), "a build the workspace cannot name goes whole");
        for build in &builds[..3] {
            for file in ours {
                assert!(!slot.path().join(build).join(file).exists(), "{build}/{file} is removed");
            }
            for file in registry {
                assert!(slot.path().join(build).join(file).exists(), "{build}/{file} stays");
            }
        }
    }

    /// Near its floor the line lets idle slots' folders go, and only theirs.
    #[test]
    fn near_the_floor_idle_slots_folders_go_and_a_busy_ones_stays() {
        let gib = 1_u64 << 30;
        assert!(near_floor(Some(19 * gib), 16));
        assert!(!near_floor(Some(20 * gib), 16));
        assert!(!near_floor(None, 16), "a disk the line cannot read lets nothing go");
        let slots = tempfile::tempdir().expect("the slots' folder");
        for slot in [0, 1] {
            write(&folder(slots.path(), slot).join("debug/deps/libserde.rlib"), "");
        }
        assert_eq!(let_idle_go(slots.path(), &[1, 2], 7), 1, "slot 2 has no folder");
        sweep(slots.path());
        assert!(folder(slots.path(), 0).join("debug/deps/libserde.rlib").exists(), "the busy slot's folder stays");
        assert!(!folder(slots.path(), 1).exists());
        assert_eq!(folders_in(&slots.path().join(FOLDER)), vec![folder(slots.path(), 0)], "nothing let go is left");
    }
}
