//! What `zo build` decides as it runs a project's builds, and what the window
//! asks before it starts one: which builds run and in what order, whether a
//! person has allowed their commands, whether a runner already holds the
//! project, and how long a running step may stay quiet.
//!
//! Nothing here runs a command. `zo build` spawns the steps and watches them;
//! the window only starts `zo build`. The files these answers keep sit beside
//! the build records under the repository's shared git folder
//! ([`crate::builds::folder`]), where no commit reaches, and their names are
//! never build ids.

use std::collections::BTreeMap;
use std::fs::{File, OpenOptions, TryLockError};
use std::io;
use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::atomic_file::write_atomically;
use crate::build_file::{BuildFile, BuildSpec, PLACEHOLDERS, StepSpec, placeholders};
use crate::builds::{PlanStep, folder, read_bounded};

/// The file a runner holds for as long as it runs a project's builds.
const HOLD_FILE: &str = ".runner.lock";
/// The file `zo build stop` leaves for the runner that holds the project.
const STOP_FILE: &str = ".stop";
/// The commands a person has allowed to run in the repository.
const APPROVED_FILE: &str = ".approved.json";
/// The most allowed commands kept; the oldest are forgotten first.
const APPROVED_MOST: usize = 512;
/// How much of a commit's id `${sha8}` is.
const SHORT_SHA_CHARS: usize = 8;
/// How many times a hold's lock found taken is looked at, this far apart,
/// before it counts as a runner's: another process looking has it for an
/// instant, and a child the holder's process spawns has a copy of its file
/// until it execs.
const SETTLE_LOOKS: u32 = 10;
const SETTLE_EVERY: Duration = Duration::from_millis(25);
/// The marks a version may hold besides ASCII letters and digits.
const VERSION_MARKS: [char; 4] = ['.', '_', '+', '-'];

/// How long a running step may write nothing. It is said stalled at a
/// multiple of its usual time and ended as failed at a larger one, each never
/// sooner than its floor. A step that has never passed is said at the floor
/// and never ended, since nothing says what is long for it. A step with a
/// `cap` of its own is ended by that cap alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuietRule {
    pub say_factor: u32,
    pub end_factor: u32,
    pub say_floor_ms: u64,
    pub end_floor_ms: u64,
}

impl QuietRule {
    /// The rule a step of `.zo/build.toml` runs under: said at twice its usual
    /// time and at least ten minutes, ended at three times and at least twenty
    /// — a quick step waiting on a lock another build holds is not stalled.
    pub const DEFAULT: Self = Self {
        say_factor: 2,
        end_factor: 3,
        say_floor_ms: 10 * 60 * 1000,
        end_floor_ms: 20 * 60 * 1000,
    };

    /// When a step whose passing runs usually took `usual_secs` is said
    /// stalled and when it is ended, in milliseconds of quiet.
    #[must_use]
    pub fn limits(&self, usual_secs: Option<u32>, capped: bool) -> Quiet {
        let usual_ms = usual_secs
            .filter(|secs| *secs > 0)
            .map(|secs| u64::from(secs) * 1000);
        Quiet {
            say_ms: usual_ms.map_or(self.say_floor_ms, |usual| {
                usual
                    .saturating_mul(u64::from(self.say_factor))
                    .max(self.say_floor_ms)
            }),
            end_ms: usual_ms.filter(|_| !capped).map(|usual| {
                usual
                    .saturating_mul(u64::from(self.end_factor))
                    .max(self.end_floor_ms)
            }),
        }
    }
}

/// One step's quiet limits, in milliseconds of quiet ([`QuietRule::limits`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Quiet {
    pub say_ms: u64,
    /// `None`: the step is not ended for being quiet.
    pub end_ms: Option<u64>,
}

/// The builds `zo build` runs when asked for `asked`, in the file's order:
/// every build it runs itself when none is asked.
///
/// # Errors
///
/// When `asked` names a build the file does not have or one another script
/// reports, or the file has no build `zo build` runs.
pub fn queue<'file>(
    file: &'file BuildFile,
    asked: &[String],
) -> Result<Vec<&'file BuildSpec>, String> {
    for id in asked {
        match file.builds.iter().find(|build| build.id == *id) {
            None => return Err(format!("the order file has no build `{id}`")),
            Some(build) if !build.source.runs_here() => {
                return Err(format!(
                    "the build `{id}` is reported by another script; `zo build` does not run it"
                ));
            }
            Some(_) => {}
        }
    }
    let builds: Vec<&BuildSpec> = file
        .builds
        .iter()
        .filter(|build| build.source.runs_here())
        .filter(|build| asked.is_empty() || asked.contains(&build.id))
        .collect();
    if builds.is_empty() {
        return Err("the order file has no build `zo build` runs".to_owned());
    }
    Ok(builds)
}

/// The command `step` runs on this platform: on Windows its `windows` one,
/// when it has one.
#[must_use]
pub fn command_of(step: &StepSpec) -> &str {
    let windows = if cfg!(windows) {
        step.windows.as_deref()
    } else {
        None
    };
    windows.or(step.run.as_deref()).unwrap_or_default()
}

/// Every command `builds` run here, once each, in the order they run.
#[must_use]
pub fn commands_of(builds: &[&BuildSpec]) -> Vec<String> {
    let mut commands: Vec<String> = Vec::new();
    for command in builds.iter().flat_map(|build| &build.steps).map(command_of) {
        if !commands.iter().any(|held| held == command) {
            commands.push(command.to_owned());
        }
    }
    commands
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Approved {
    #[serde(default)]
    commands: Vec<String>,
}

fn approved_of(shared_git_dir: &Path) -> Approved {
    read_bounded(&folder(shared_git_dir).join(APPROVED_FILE))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// The commands among `commands` a person has not allowed in the repository
/// whose shared git folder is `shared_git_dir`: a file someone else committed
/// runs nothing until its person has seen them.
#[must_use]
pub fn unapproved(shared_git_dir: &Path, commands: &[String]) -> Vec<String> {
    let approved = approved_of(shared_git_dir);
    commands
        .iter()
        .filter(|command| !approved.commands.contains(command))
        .cloned()
        .collect()
}

/// Records that a person allowed `commands`, after the ones allowed before.
///
/// # Errors
///
/// When the record cannot be written.
pub fn approve(shared_git_dir: &Path, commands: &[String]) -> Result<(), String> {
    let mut approved = approved_of(shared_git_dir);
    approved.commands.retain(|held| !commands.contains(held));
    approved.commands.extend(commands.iter().cloned());
    let forgotten = approved.commands.len().saturating_sub(APPROVED_MOST);
    approved.commands.drain(..forgotten);
    let dir = folder(shared_git_dir);
    std::fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
    let mut bytes = serde_json::to_vec_pretty(&approved).map_err(|error| error.to_string())?;
    bytes.push(b'\n');
    write_atomically(&dir.join(APPROVED_FILE), &bytes).map_err(|error| error.to_string())
}

/// One runner's hold on a project. Dropping it, or the runner's end, frees
/// the project.
#[derive(Debug)]
pub struct Hold {
    _file: File,
}

/// Takes the project of `shared_git_dir` for one runner: none while another
/// runner holds it. A lock taken only for a moment — another process looking
/// ([`held`]), or a copy a child keeps until it execs — is waited out first.
///
/// # Errors
///
/// When the hold's file cannot be made or locked.
pub fn hold(shared_git_dir: &Path) -> io::Result<Option<Hold>> {
    let dir = folder(shared_git_dir);
    std::fs::create_dir_all(&dir)?;
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join(HOLD_FILE))?;
    match settled(|| file.try_lock()) {
        Ok(()) => Ok(Some(Hold { _file: file })),
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Error(error)) => Err(error),
    }
}

/// Whether a runner holds the project of `shared_git_dir` now. Asking makes
/// no file: it has the lock as a reader for an instant, which [`hold`] waits
/// out, and a lock taken only for a moment does not read as a runner's.
#[must_use]
pub fn held(shared_git_dir: &Path) -> bool {
    File::open(folder(shared_git_dir).join(HOLD_FILE)).is_ok_and(|file| {
        matches!(
            settled(|| file.try_lock_shared()),
            Err(TryLockError::WouldBlock)
        )
    })
}

/// `try_lock` again while it finds the lock taken, up to [`SETTLE_LOOKS`]
/// times: what it says then.
fn settled(try_lock: impl Fn() -> Result<(), TryLockError>) -> Result<(), TryLockError> {
    for _ in 1..SETTLE_LOOKS {
        match try_lock() {
            Err(TryLockError::WouldBlock) => std::thread::sleep(SETTLE_EVERY),
            said => return said,
        }
    }
    try_lock()
}

/// Asks the runner that holds the project of `shared_git_dir` to stop.
///
/// # Errors
///
/// When the ask cannot be written.
pub fn ask_stop(shared_git_dir: &Path) -> io::Result<()> {
    std::fs::write(folder(shared_git_dir).join(STOP_FILE), b"")
}

/// Whether a stop has been asked since the last look; the ask is taken.
#[must_use]
pub fn stop_asked(shared_git_dir: &Path) -> bool {
    std::fs::remove_file(folder(shared_git_dir).join(STOP_FILE)).is_ok()
}

/// A build's steps as its record plans them, named as the file names them.
#[must_use]
pub fn plan_of(build: &BuildSpec) -> Vec<PlanStep> {
    build
        .steps
        .iter()
        .map(|step| PlanStep {
            name: step.id.clone(),
            words: step.name.clone(),
        })
        .collect()
}

/// What a run fills into its commands ([`PLACEHOLDERS`]): the commit `sha`,
/// whole and its first characters, and the `version` it is labelled with.
///
/// A step's command runs in a shell, and a person allowed it before these
/// were filled in, so neither may hold what a shell reads as more words: the
/// commit holds only hexadecimal digits, the version only ASCII letters,
/// digits and the marks of `VERSION_MARKS`. Nor may the version start with
/// `-`, which the step's program would read as a flag. The version is often
/// the nearest tag, which anyone who can push to the repository names.
///
/// # Errors
///
/// When `sha` or `version` holds anything else, or `version` starts with `-`,
/// in a sentence that names it.
pub fn values(sha: &str, version: &str) -> Result<BTreeMap<&'static str, String>, String> {
    if !sha.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!(
            "the commit `{sha}` is not hexadecimal, so no step is given it"
        ));
    }
    if let Some(c) = version
        .chars()
        .find(|c| !(c.is_ascii_alphanumeric() || VERSION_MARKS.contains(c)))
    {
        let marks = VERSION_MARKS.map(|mark| format!("`{mark}`")).join(" ");
        return Err(format!(
            "the version `{version}` holds {c:?}, and a step is given only letters, digits and {marks}; name the version with `--label`"
        ));
    }
    if version.starts_with('-') {
        return Err(format!(
            "the version `{version}` starts with `-`, which the step's program would read as a flag; name the version with `--label`"
        ));
    }
    let [whole, short, labelled] = PLACEHOLDERS;
    Ok(BTreeMap::from([
        (whole, sha.to_owned()),
        (short, sha.chars().take(SHORT_SHA_CHARS).collect()),
        (labelled, version.to_owned()),
    ]))
}

/// The placeholder `command` uses that `values` leaves empty, if any.
#[must_use]
pub fn unfilled<'command>(
    command: &'command str,
    values: &BTreeMap<&str, String>,
) -> Option<&'command str> {
    placeholders(command)
        .ok()?
        .into_iter()
        .find(|name| values.get(name).is_none_or(String::is_empty))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build_file::parse;

    const ORDER: &str = r#"
[[build]]
id = "app"
step = [
  { id = "compile", run = "./gradlew compileJava" },
  { id = "test", run = "./gradlew test", cap = "20m" },
]

[[build]]
id = "release"
source = "report"
step = [{ id = "publish" }]

[[build]]
id = "image"
after = "app"
step = [
  { id = "build", run = "docker build -t shop:${sha8} ." },
  { id = "push", run = "docker push shop:${sha8}", when = "manual" },
  { id = "again", run = "./gradlew test" },
]
"#;

    fn ids(builds: &[&BuildSpec]) -> Vec<String> {
        builds.iter().map(|build| build.id.clone()).collect()
    }

    fn asked(list: &[&str]) -> Vec<String> {
        list.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn a_quiet_step_is_said_and_ended_at_the_multiples_of_its_usual_time() {
        let rule = QuietRule::DEFAULT;

        // A test that usually takes 955 s is said at 31 minutes and ended at 47.
        assert_eq!(
            rule.limits(Some(955), false),
            Quiet {
                say_ms: 1_910_000,
                end_ms: Some(2_865_000)
            }
        );
        // A quick step is said and ended no sooner than the floors.
        assert_eq!(
            rule.limits(Some(30), false),
            Quiet {
                say_ms: 600_000,
                end_ms: Some(1_200_000)
            }
        );
        // No passing past, or a cap of its own: said, never ended for quiet.
        for (usual, capped) in [(None, false), (Some(0), false)] {
            assert_eq!(
                rule.limits(usual, capped),
                Quiet {
                    say_ms: 600_000,
                    end_ms: None
                }
            );
        }
        assert_eq!(rule.limits(Some(955), true).end_ms, None);
    }

    #[test]
    fn a_queue_is_the_files_order_of_the_builds_zo_build_runs() {
        let file = parse(ORDER).expect("the order reads");

        assert_eq!(ids(&queue(&file, &[]).expect("all")), ["app", "image"]);
        assert_eq!(
            ids(&queue(&file, &asked(&["image", "app"])).expect("both")),
            ["app", "image"]
        );
        let reported = queue(&file, &asked(&["release"])).expect_err("reported");
        let missing = queue(&file, &asked(&["nope"])).expect_err("not in the file");
        let none = queue(&BuildFile::default(), &[]).expect_err("nothing to run");

        assert!(
            reported.contains("reported by another script"),
            "{reported}"
        );
        assert!(missing.contains("no build `nope`"), "{missing}");
        assert!(none.contains("no build `zo build` runs"), "{none}");
    }

    #[test]
    fn each_command_is_listed_once_and_asked_for_until_a_person_allows_it() {
        let shared = tempfile::tempdir().expect("a shared git folder");
        let file = parse(ORDER).expect("the order reads");
        let commands = commands_of(&queue(&file, &[]).expect("all"));

        assert_eq!(
            commands,
            [
                "./gradlew compileJava",
                "./gradlew test",
                "docker build -t shop:${sha8} .",
                "docker push shop:${sha8}",
            ]
        );
        assert_eq!(unapproved(shared.path(), &commands), commands);
        approve(shared.path(), &commands[..2]).expect("allowed");
        assert_eq!(unapproved(shared.path(), &commands), commands[2..]);
        approve(shared.path(), &commands).expect("allowed");
        assert!(unapproved(shared.path(), &commands).is_empty());

        let many: Vec<String> = (0..APPROVED_MOST + 3)
            .map(|n| format!("echo {n}"))
            .collect();
        approve(shared.path(), &many).expect("allowed");
        assert_eq!(unapproved(shared.path(), &many), many[..3]);
        assert_eq!(unapproved(shared.path(), &commands), commands);
    }

    #[test]
    fn one_runner_holds_a_project_and_a_stop_is_asked_once() {
        let shared = tempfile::tempdir().expect("a shared git folder");

        assert!(!held(shared.path()), "nothing held before any runner");
        let first = hold(shared.path()).expect("held").expect("free");
        assert!(hold(shared.path()).expect("asked").is_none(), "one runner");
        assert!(held(shared.path()));
        drop(first);
        // A child another test spawns keeps the lock file open until it execs,
        // so the hold can outlive its holder for a moment.
        let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while held(shared.path()) {
            assert!(
                std::time::Instant::now() < until,
                "the hold outlived its holder"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }

        assert!(!stop_asked(shared.path()));
        ask_stop(shared.path()).expect("asked");
        assert!(stop_asked(shared.path()));
        assert!(!stop_asked(shared.path()), "an ask is taken once");
    }

    #[test]
    fn a_lock_taken_only_for_a_moment_neither_refuses_a_runner_nor_reads_as_one() {
        // Each lock below is let go this long after it is taken: far less than a
        // hold looks again for, far more than a look takes.
        const MOMENT: std::time::Duration = std::time::Duration::from_millis(20);
        let shared = tempfile::tempdir().expect("a shared git folder");
        let let_go = |file: File| {
            std::thread::spawn(move || {
                std::thread::sleep(MOMENT);
                drop(file);
            })
        };
        std::fs::create_dir_all(folder(shared.path())).expect("the builds folder");
        let look = File::create(folder(shared.path()).join(HOLD_FILE)).expect("the hold's file");

        // Another process looks at the hold, and has the lock for that moment.
        look.try_lock().expect("the look has the lock");
        let looked = let_go(look);
        let runner = hold(shared.path()).expect("asked");
        looked.join().expect("the look ends");
        let runner = runner.expect("a look's moment does not refuse a runner");

        // A child the runner's process spawned has a copy of its file until it execs.
        let copy = runner._file.try_clone().expect("the child's copy");
        drop(runner);
        let copied = let_go(copy);
        let said = held(shared.path());
        copied.join().expect("the child execs");

        assert!(!said, "a copy's moment does not read as a runner");
    }

    #[test]
    fn a_run_fills_the_commit_and_version_and_names_what_it_cannot() {
        let values = values("3e1c9a0d5b7f", "v1.2").expect("a plain commit and version");
        let file = parse(ORDER).expect("the order reads");
        let image = &file.builds[2];

        assert_eq!(
            crate::build_file::fill(command_of(&image.steps[0]), &values),
            "docker build -t shop:3e1c9a0d ."
        );
        assert_eq!(
            plan_of(image)
                .iter()
                .map(|step| step.name.as_str())
                .collect::<Vec<_>>(),
            ["build", "push", "again"]
        );
        assert_eq!(unfilled("echo ${version}", &values), None);
        assert_eq!(
            unfilled(
                "docker push shop:${sha8}",
                &self::values("", "v1").expect("a checkout with no commit yet")
            ),
            Some("sha8")
        );
    }

    #[test]
    fn a_commit_or_version_the_shell_would_read_as_more_words_is_refused() {
        for tag in [
            "v1;touch${IFS}x",
            "v1$(id)",
            "v1`id`",
            "v1|id",
            "v1&&id",
            "v1'x",
            "v1>out",
            "v1 x",
            "v1\nx",
        ] {
            let refused = values("3e1c9a0d5b7f", tag).expect_err(tag);
            assert!(
                refused.contains(tag),
                "the refusal names the version: {refused}"
            );
        }
        for version in [
            "v1.2",
            "1.2.3-rc.1+build.5",
            "v1.1.127-3-g3e1c9a0",
            "3e1c9a0",
            "release_2",
            "",
        ] {
            assert!(
                values("3e1c9a0d5b7f", version).is_ok(),
                "{version} is plain"
            );
        }
        assert!(
            values("3e1c9a0d;id", "v1").is_err(),
            "a commit is hexadecimal"
        );
    }

    #[test]
    fn a_version_the_steps_program_would_read_as_a_flag_is_refused() {
        for tag in ["--force", "-x"] {
            let refused = values("3e1c9a0d5b7f", tag).expect_err(tag);
            assert!(
                refused.contains(tag),
                "the refusal names the version: {refused}"
            );
        }
    }
}
