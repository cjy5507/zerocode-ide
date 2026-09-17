//! Running the project's own setup and archive scripts — and an automation's
//! precheck, which is the same machine asked a different question.
//!
//! The schema, the effective-script rules and the variable names are in
//! `zerocode-core::project`; what lives here is the part that touches the
//! machine — reading the file off disk, spawning a shell, and giving up after
//! the measured two minutes.
//!
//! Three decisions this file makes, each of which a plain `Command::new`
//! would get wrong:
//!
//!   1. **The script is stdin, not an argument.** `bash -c "$script"` puts the
//!      whole thing on a command line, where a long setup script can exceed
//!      the platform's argument limit and where quoting is one mistake away
//!      from running half of it. Orca uses a shell reading a script
//!      (`getHookShell` + a spawn); piping is the version with no length limit
//!      and no quoting at all.
//!   2. **It runs in the WORKTREE, not the repository.** A setup script exists
//!      to prepare the checkout that was just made, and `npm install` in the
//!      wrong directory is a slow, silent no-op.
//!   3. **A timeout must kill the child, not just stop waiting for it.** A
//!      script that hangs holds a shell, and a window that stopped waiting
//!      while the process lives has leaked it.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use serde::Serialize;
use zerocode_core::PrecheckOutcome;
use zerocode_core::commit_failure::truncate_prompt_text;
use zerocode_core::project::{
    PROJECT_FILE, ProjectFile, SCRIPT_TIMEOUT, parse_project_file, script_env,
};

/// How a script run ended. The window shows all four; none is a log line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ScriptOutcome {
    /// Nothing to run — no file, or no script of this kind in it.
    Nothing,
    Ok {
        /// Trailing output, for the panel that reports it.
        tail: String,
    },
    Failed {
        code: Option<i32>,
        tail: String,
    },
    /// Still running when the two minutes were up, and killed.
    TimedOut,
    /// The shell could not be started at all.
    Unstartable {
        why: String,
    },
}

/// Read the project's file from a checkout, or `None` when there is none.
///
/// Reads from the WORKTREE rather than the repository root, which is Orca's own
/// `getEffectiveHooks(repo, worktreePath)`: a branch may add or change its own
/// setup, and the checkout being prepared is the one whose instructions apply.
pub fn read_project_file(root: &Path) -> Option<ProjectFile> {
    let text = std::fs::read_to_string(project_file_path(root)).ok()?;
    let file = parse_project_file(&text);
    // A file that exists but says nothing is the same as no file — except when
    // it could not be READ, which is something the window has to be able to
    // say.
    (!file.is_silent() || file.unreadable).then_some(file)
}

pub fn project_file_path(root: &Path) -> PathBuf {
    root.join(PROJECT_FILE)
}

/// The shell a script is handed to. Orca's `getHookShell`.
fn shell() -> &'static str {
    #[cfg(windows)]
    {
        "cmd.exe"
    }
    #[cfg(not(windows))]
    {
        "/bin/bash"
    }
}

/// The same interpreter as blocking setup runs, but kept interactive so its
/// command can live in a visible terminal pane and leave the shell available
/// afterwards. The renderer never chooses this program or supplies argv.
pub(crate) fn terminal_shell() -> &'static str {
    shell()
}

pub(crate) fn terminal_shell_args() -> Vec<String> {
    #[cfg(windows)]
    {
        vec!["/Q".to_string()]
    }
    #[cfg(not(windows))]
    {
        Vec::new()
    }
}

pub(crate) fn setup_environment(repo_root: &Path, worktree: &Path) -> Vec<(String, String)> {
    script_env(&repo_root.to_string_lossy(), &worktree.to_string_lossy())
}

/// How much of the output travels back. A setup script can print a megabyte of
/// npm noise, and what a person needs is the end of it — where the error is.
const TAIL_BYTES: usize = 8 * 1024;

fn tail_of(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    if text.len() <= TAIL_BYTES {
        return text.into_owned();
    }
    // Cut on a char boundary; a lossy string can still be sliced wrongly.
    let from = text
        .char_indices()
        .rev()
        .map(|(at, _)| at)
        .find(|at| text.len() - at >= TAIL_BYTES)
        .unwrap_or(0);
    text[from..].to_string()
}

/// Run one script in `worktree`, with `repo_root` as the repository it belongs
/// to.
///
/// Blocking: the caller puts it on a pool. Two minutes is a long time to hold
/// a thread and much longer than a window may hold its own.
pub fn run_script(script: &str, repo_root: &Path, worktree: &Path) -> ScriptOutcome {
    if script.trim().is_empty() {
        return ScriptOutcome::Nothing;
    }
    let env = script_env(&repo_root.to_string_lossy(), &worktree.to_string_lossy());
    let ran = run_shell(script, worktree, &env, SCRIPT_TIMEOUT);
    if let Some(why) = ran.unstartable {
        return ScriptOutcome::Unstartable { why };
    }
    if ran.timed_out {
        return ScriptOutcome::TimedOut;
    }
    let tail = tail_of(&ran.said);
    if ran.code == Some(0) {
        ScriptOutcome::Ok { tail }
    } else {
        ScriptOutcome::Failed {
            code: ran.code,
            tail,
        }
    }
}

/// One shell run's ending, before anybody decides what it means.
///
/// Two callers read this differently — a setup script's ending is shown to a
/// person, a precheck's decides whether an agent is woken — and both readings
/// need the same four facts. Keeping the SPAWN in one place is the point: the
/// timeout that kills rather than merely stops waiting, and the stdin that is
/// closed before the wait begins, are each one mistake away from a leaked
/// process or a deadlock, and neither should be written twice.
struct Ran {
    /// `None` when the shell was killed or never reported one — which is why
    /// a missing code can never be read as zero.
    code: Option<i32>,
    timed_out: bool,
    /// Why the shell could not be started, or could not be waited on.
    unstartable: Option<String>,
    /// Both pipes, in the order they were drained.
    said: Vec<u8>,
    took: Duration,
}

/// Spawn `shell()` on `script`, in `cwd`, and give up after `budget`.
fn run_shell(script: &str, cwd: &Path, env: &[(String, String)], budget: Duration) -> Ran {
    let began = Instant::now();
    let unstartable = |why: String, began: Instant| Ran {
        code: None,
        timed_out: false,
        unstartable: Some(why),
        said: Vec::new(),
        took: began.elapsed(),
    };
    let mut command = crate::proc::quiet_command(shell());
    #[cfg(windows)]
    command.arg("/Q");
    #[cfg(not(windows))]
    command.arg("-s");
    command
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (name, value) in env {
        command.env(name, value);
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => return unstartable(error.to_string(), began),
    };
    // Written and closed before waiting: a shell reading from stdin does not
    // start until the pipe ends, so holding it open is a deadlock rather than
    // a slow script.
    if let Some(mut pipe) = child.stdin.take() {
        let _ = pipe.write_all(script.as_bytes());
    }

    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let mut said = Vec::new();
                if let Some(mut out) = child.stdout.take() {
                    let _ = std::io::Read::read_to_end(&mut out, &mut said);
                }
                if let Some(mut err) = child.stderr.take() {
                    let _ = std::io::Read::read_to_end(&mut err, &mut said);
                }
                return Ran {
                    code: status.code(),
                    timed_out: false,
                    unstartable: None,
                    said,
                    took: began.elapsed(),
                };
            }
            Ok(None) => {
                if began.elapsed() >= budget {
                    // Killed, not merely abandoned: a hung script holds a
                    // shell for as long as the window lives otherwise.
                    let _ = child.kill();
                    let _ = child.wait();
                    return Ran {
                        code: None,
                        timed_out: true,
                        unstartable: None,
                        said: Vec::new(),
                        took: began.elapsed(),
                    };
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(error) => return unstartable(error.to_string(), began),
        }
    }
}

/// How much of a precheck's output the ledger keeps.
///
/// Smaller than a setup script's tail on purpose: this one is not a panel
/// somebody is watching, it is a line in a history read weeks later, and it is
/// written to a file that holds fifty runs of every job.
const PRECHECK_OUTPUT_LIMIT: usize = 2_000;

/// A precheck that ran, and everything the history keeps about it.
pub struct PrecheckRun {
    /// The three facts the verdict is made of. Ask
    /// [`PrecheckOutcome::passed`] — never these fields separately.
    pub outcome: PrecheckOutcome,
    pub duration_ms: i64,
    /// Both pipes, head and tail.
    pub output: String,
}

/// Run one automation's precheck and say how it ended.
///
/// The shell, the stdin and the killing timeout are the setup script's, for
/// the reasons at the top of this file — a precheck is a command somebody
/// typed into a form, so it can be a pipeline, a `&&` chain or a here-doc, and
/// splitting it on whitespace would run the first word with the rest as
/// arguments.
///
/// Runs in the checkout the job names, and BEFORE anything is made: a
/// `NewPerRun` job whose precheck fails must not leave a worktree behind that
/// nothing ever ran in.
///
/// No `ZEROCODE_*` variables. A setup script is handed the checkout it is
/// preparing because it could not otherwise know; a precheck already runs in
/// the one directory it is about, and a variable this window promises is a
/// variable it has to keep promising.
pub fn run_precheck(command: &str, cwd: &Path, budget: Duration) -> PrecheckRun {
    let ran = run_shell(command, cwd, &[], budget);
    PrecheckRun {
        outcome: PrecheckOutcome {
            exit_code: ran.code,
            timed_out: ran.timed_out,
            errored: ran.unstartable.is_some(),
        },
        duration_ms: i64::try_from(ran.took.as_millis()).unwrap_or(i64::MAX),
        // The shell's complaint IS the output when there was no shell — a
        // history row reading "skipped" with nothing beside it is one nobody
        // can act on.
        output: truncate_prompt_text(
            ran.unstartable
                .as_deref()
                .unwrap_or(&String::from_utf8_lossy(&ran.said)),
            PRECHECK_OUTPUT_LIMIT,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zerocode_core::project::{ProjectScript, ScriptSource, effective_script};

    fn scratch() -> tempfile::TempDir {
        tempfile::tempdir().expect("no temp dir")
    }

    #[test]
    fn a_checkout_with_no_file_asks_for_nothing() {
        let dir = scratch();
        assert_eq!(read_project_file(dir.path()), None);
        // And a file that says nothing reads the same way.
        std::fs::write(project_file_path(dir.path()), "# hello\n").expect("write");
        assert_eq!(read_project_file(dir.path()), None);
    }

    #[test]
    fn a_file_that_cannot_be_read_is_reported_rather_than_ignored() {
        let dir = scratch();
        std::fs::write(project_file_path(dir.path()), "whatIsThis: yes\n").expect("write");
        let file = read_project_file(dir.path()).expect("an unreadable file was swallowed");
        assert!(file.unreadable);
    }

    #[cfg(unix)]
    #[test]
    fn a_script_runs_in_the_worktree_and_is_told_where_it_is() {
        let repo = scratch();
        let tree = repo.path().join("wt");
        std::fs::create_dir(&tree).expect("mkdir");
        // Both facts in one script: the working directory it landed in, and
        // the variables it was handed.
        let outcome = run_script(
            "pwd > where.txt\nprintf '%s|%s' \"$ZEROCODE_ROOT_PATH\" \"$ZEROCODE_WORKSPACE_NAME\" > vars.txt",
            repo.path(),
            &tree,
        );
        assert!(matches!(outcome, ScriptOutcome::Ok { .. }), "{outcome:?}");
        let landed = std::fs::read_to_string(tree.join("where.txt")).expect("no where.txt");
        assert!(
            landed.trim().ends_with("wt"),
            "the script ran somewhere else: {landed}"
        );
        let vars = std::fs::read_to_string(tree.join("vars.txt")).expect("no vars.txt");
        let (root, name) = vars.split_once('|').expect("both variables");
        assert_eq!(root, repo.path().to_string_lossy());
        assert_eq!(name, "wt");
    }

    #[cfg(unix)]
    #[test]
    fn a_failing_script_reports_its_code_and_the_end_of_its_output() {
        let dir = scratch();
        let outcome = run_script("echo trouble >&2\nexit 3", dir.path(), dir.path());
        match outcome {
            ScriptOutcome::Failed { code, tail } => {
                assert_eq!(code, Some(3));
                assert!(tail.contains("trouble"), "the output was lost: {tail}");
            }
            other => panic!("a failing script reported {other:?}"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_long_script_is_not_a_command_line() {
        // The reason the script goes in on stdin. A megabyte of comments is
        // well past the platform's argument limit and must still run.
        let dir = scratch();
        let mut script = String::from("touch ran.txt\n");
        script.push_str(&"# padding padding padding\n".repeat(40_000));
        assert!(script.len() > 1_000_000);
        let outcome = run_script(&script, dir.path(), dir.path());
        assert!(matches!(outcome, ScriptOutcome::Ok { .. }), "{outcome:?}");
        assert!(dir.path().join("ran.txt").exists());
    }

    #[cfg(unix)]
    #[test]
    fn an_empty_script_is_nothing_rather_than_a_successful_shell() {
        // `bash -s` with no input exits 0, which would report a setup that
        // "worked" when nothing was configured.
        let dir = scratch();
        assert_eq!(
            run_script("   \n", dir.path(), dir.path()),
            ScriptOutcome::Nothing
        );
    }

    #[cfg(unix)]
    #[test]
    fn the_file_and_the_runner_meet_where_they_are_supposed_to() {
        // The seam between the two modules, exercised end to end: a real file
        // on disk becomes a real process.
        let repo = scratch();
        std::fs::write(
            project_file_path(repo.path()),
            "scripts:\n  setup: |\n    printf ok > proof.txt\n",
        )
        .expect("write");
        let file = read_project_file(repo.path()).expect("no file read");
        let script = effective_script(&file, None, ProjectScript::Setup, ScriptSource::default())
            .expect("no setup script");
        let outcome = run_script(&script, repo.path(), repo.path());
        assert!(matches!(outcome, ScriptOutcome::Ok { .. }), "{outcome:?}");
        assert_eq!(
            std::fs::read_to_string(repo.path().join("proof.txt")).expect("no proof"),
            "ok"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_precheck_answers_from_the_checkout_it_guards() {
        // A real command, in the directory the job named, whose exit code is
        // the answer. A pipeline rather than one word, because a precheck is
        // typed into a form: anything that splits on whitespace would run
        // `test` with `-f` and `marker` as arguments to something else.
        let dir = scratch();
        std::fs::write(dir.path().join("marker"), "here").expect("write");
        let ran = run_precheck("test -f marker && echo found", dir.path(), SCRIPT_TIMEOUT);
        assert!(ran.outcome.passed(), "{:?}", ran.outcome);
        assert!(ran.output.contains("found"), "the output was lost");

        // The same command one directory over is the same command failing —
        // which is the whole reason the cwd is the job's checkout.
        let elsewhere = scratch();
        let ran = run_precheck(
            "test -f marker && echo found",
            elsewhere.path(),
            SCRIPT_TIMEOUT,
        );
        assert!(!ran.outcome.passed());
        assert_eq!(ran.outcome.exit_code, Some(1));
    }

    #[cfg(unix)]
    #[test]
    fn a_failing_precheck_keeps_its_code_and_both_pipes() {
        let dir = scratch();
        let ran = run_precheck("echo out; echo err >&2; exit 3", dir.path(), SCRIPT_TIMEOUT);
        assert!(!ran.outcome.passed());
        assert_eq!(ran.outcome.exit_code, Some(3));
        assert!(!ran.outcome.timed_out && !ran.outcome.errored);
        // Both pipes: a command that states its complaint on stderr is the
        // usual case, and one that states it on stdout is the other usual
        // case.
        assert!(
            ran.output.contains("out") && ran.output.contains("err"),
            "a pipe was dropped: {}",
            ran.output
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_precheck_that_hangs_is_killed_and_never_passes() {
        // The budget is the job's own `precheck_timeout_seconds`, so this one
        // is measured in milliseconds rather than the two minutes a setup
        // script gets — nobody should wait a real minute to prove a kill.
        let dir = scratch();
        let began = Instant::now();
        let ran = run_precheck("sleep 30", dir.path(), Duration::from_millis(300));
        assert!(ran.outcome.timed_out, "the sleep was waited out");
        assert!(
            !ran.outcome.passed(),
            "a precheck that never answered read as a pass"
        );
        assert!(
            began.elapsed() < Duration::from_secs(5),
            "the budget was not enforced at all"
        );
        // And the child is dead rather than merely abandoned: `wait` after
        // `kill` is what makes this a process the machine has finished with.
        assert!(ran.duration_ms >= 300, "the clock was not read");
    }

    #[cfg(unix)]
    #[test]
    fn a_precheck_that_cannot_run_says_so_instead_of_passing() {
        // A command that is not there exits non-zero through the shell, which
        // is the ordinary refusal. The failure mode this guards is the other
        // one — a working directory that is gone, so no shell starts at all.
        let dir = scratch();
        let missing = dir.path().join("was-deleted");
        let ran = run_precheck("true", &missing, SCRIPT_TIMEOUT);
        assert!(
            ran.outcome.errored,
            "a shell that never ran was not noticed"
        );
        assert!(
            !ran.outcome.passed(),
            "a checkout that is gone read as permission to run"
        );
        assert!(
            !ran.output.is_empty(),
            "the history would show a skip with no reason beside it"
        );
    }
}
