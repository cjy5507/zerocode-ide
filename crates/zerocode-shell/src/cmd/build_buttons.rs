//! The build card's buttons (the build card's plan, step 6): 「빌드」, 「다시」
//! (from the start, or from the step a run failed or was stopped at — 11-08)
//! and a step that waits for a person start `zo build` outside the window, and
//! 「멈추기」 asks it to stop. 「기록 지우기」 and its 「되돌리기」 are
//! `zo build forget`/`restore`, and the retry and stop of a build another
//! script reports are `zo build act`. A build goes on when the window closes, and the
//! window writes no record. Before a first run the person is shown the commands
//! no one has allowed to run in the repository, and nothing starts until they
//! say yes (`zerocode_core::build_run`).

use std::io::{Read as _, Seek as _, SeekFrom};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use zerocode_core::build_file::{self, Act};
use zerocode_core::build_run;

use crate::*;

/// How long a press waits for `zo build` to take the project — so the board's
/// next look already shows its runs — or to end at once and say why.
const START_WAIT: Duration = Duration::from_secs(3);
const START_LOOK_EVERY: Duration = Duration::from_millis(50);
/// What `zo` puts before a refusal on stderr; the window says the rest.
const REFUSAL_LEAD: &str = "zo build: ";
/// The line `zo` writes on stderr as it exits, whatever it ran
/// (`session::process_lifecycle` in zo): not part of what it said.
const EXIT_LINE_LEAD: &str = "zo: process exit ";

/// What a press did.
#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case")]
pub(crate) enum Pressed {
    /// `zo build` runs, outside the window.
    Started,
    /// Nothing ran: these commands wait for the person's yes.
    Approve { commands: Vec<String> },
}

/// What a press goes on to do: the words after `zo build`, or the commands to
/// ask about first.
#[derive(Debug, PartialEq, Eq)]
enum Next {
    Start(Vec<String>),
    Approve(Vec<String>),
}

/// Each build is named with `--build`, even when every build is meant: an
/// older zo with no runner refuses the flag before any session starts.
fn build_words<'id>(builds: impl IntoIterator<Item = &'id String>) -> Vec<String> {
    builds
        .into_iter()
        .flat_map(|id| ["--build".to_owned(), id.clone()])
        .collect()
}

/// The press, up to the launch: the commands the person allowed in `allowed`
/// are recorded as allowed, and the builds start only when no command of
/// theirs is left unallowed and no runner holds the project.
fn press(
    checkout: &Path,
    shared: &Path,
    builds: &[String],
    step: Option<&str>,
    from: Option<&str>,
    allowed: &[String],
) -> Result<Next, String> {
    let file = build_file::read(checkout)?;
    let queue = build_run::queue(&file, builds)?;
    let commands = build_run::commands_of(&queue);
    let yes: Vec<String> = commands
        .iter()
        .filter(|command| allowed.contains(command))
        .cloned()
        .collect();
    if !yes.is_empty() {
        build_run::approve(shared, &yes)?;
    }
    let unapproved = build_run::unapproved(shared, &commands);
    if !unapproved.is_empty() {
        return Ok(Next::Approve(unapproved));
    }
    if build_run::held(shared) {
        return Err("builds are already running in this project".to_owned());
    }
    let mut words = build_words(queue.iter().map(|build| &build.id));
    if let Some(step) = step {
        words.extend(["--step".to_owned(), step.to_owned()]);
    } else if let Some(from) = from {
        words.extend(["--from".to_owned(), from.to_owned()]);
    }
    Ok(Next::Start(words))
}

/// A reported build's act, up to the launch: the commands the person allowed in
/// `allowed` are recorded as allowed, and the act starts only when its command
/// is allowed.
fn press_act(
    checkout: &Path,
    shared: &Path,
    id: &str,
    act: Act,
    allowed: &[String],
) -> Result<Next, String> {
    let file = build_file::read(checkout)?;
    let build = file
        .builds
        .iter()
        .find(|build| build.id == id)
        .ok_or_else(|| format!("the order file names no build `{id}`"))?;
    let command = build
        .act_command(act)
        .ok_or_else(|| format!("the build `{id}` names no `{}` command", act.word()))?
        .to_owned();
    if allowed.contains(&command) {
        build_run::approve(shared, std::slice::from_ref(&command))?;
    }
    let unapproved = build_run::unapproved(shared, std::slice::from_ref(&command));
    if !unapproved.is_empty() {
        return Ok(Next::Approve(unapproved));
    }
    Ok(Next::Start(vec![
        "act".to_owned(),
        "--build".to_owned(),
        id.to_owned(),
        "--act".to_owned(),
        act.word().to_owned(),
    ]))
}

/// `zo build`, for `checkout`: the zo this window keeps at `~/.local/bin/zo`,
/// on the person's own `PATH`, so that a step finds the tools a terminal would.
fn zo_build(checkout: &Path) -> Result<Command, String> {
    let home = dirs::home_dir().ok_or_else(|| "no home directory".to_owned())?;
    let bin = crate::zo_companion::zo_path_under(&home);
    if !bin.exists() {
        return Err(format!("zo is not installed at {}", bin.display()));
    }
    let mut command = crate::proc::quiet_command_on_path(&bin, crate::shell_path::hydrated());
    command
        .arg("build")
        .current_dir(checkout)
        .stdin(Stdio::null())
        .stdout(Stdio::null());
    Ok(command)
}

/// What `zo` said as it refused: its first paragraph, without its exit line
/// or the usage that follows a refusal after a blank line.
fn refusal(said: &[u8]) -> String {
    let said = String::from_utf8_lossy(said);
    let first = said
        .lines()
        .filter(|line| !line.starts_with(EXIT_LINE_LEAD))
        .skip_while(|line| line.trim().is_empty())
        .take_while(|line| !line.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    first
        .strip_prefix(REFUSAL_LEAD)
        .unwrap_or(&first)
        .to_owned()
}

/// Starts `command` as its own process group, outside the window's, and waits
/// until it holds the project of `shared` or ends. What it says goes to a file
/// no one names, read only when it ends at once — never to a pipe, which would
/// break on its last word once the window closed while the build went on.
fn start(command: Command, shared: &Path) -> Result<(), String> {
    start_until(command, || build_run::held(shared))
}

/// [`start`], waiting until `holds` says the child took what it was started
/// for — an act holds nothing, so it waits only for an end at once.
fn start_until(mut command: Command, holds: impl Fn() -> bool) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.process_group(0);
    }
    let mut said = tempfile::tempfile().map_err(|error| error.to_string())?;
    let stderr = said.try_clone().map_err(|error| error.to_string())?;
    let mut child = command
        .stderr(stderr)
        .spawn()
        .map_err(|error| error.to_string())?;
    let until = Instant::now() + START_WAIT;
    while Instant::now() < until && !holds() {
        if let Some(status) = child.try_wait().map_err(|error| error.to_string())? {
            if status.success() {
                return Ok(());
            }
            let mut words = Vec::new();
            let _ = said
                .seek(SeekFrom::Start(0))
                .and_then(|_| said.read_to_end(&mut words));
            return Err(refusal(&words));
        }
        std::thread::sleep(START_LOOK_EVERY);
    }
    // The runner goes on alone; its exit is collected whenever it comes.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

fn shared_of(checkout: &Path) -> Result<PathBuf, String> {
    zerocode_core::git_dir::shared_of(checkout)
        .ok_or_else(|| format!("{} is not inside a git checkout", checkout.display()))
}

/// 「빌드」, 「다시」 or a waiting step, for `builds` of `worktree`'s project —
/// the checkout on screen, the open one when none is named. `allowed` holds the
/// commands the person just said yes to.
#[tauri::command]
pub(crate) async fn build_start(
    state: State<'_, AppState>,
    worktree: Option<String>,
    builds: Vec<String>,
    step: Option<String>,
    from: Option<String>,
    allowed: Option<Vec<String>>,
) -> Result<Pressed, String> {
    let checkout = worktree.map_or_else(|| state.active_root(), PathBuf::from);
    tauri::async_runtime::spawn_blocking(move || {
        let shared = shared_of(&checkout)?;
        match press(
            &checkout,
            &shared,
            &builds,
            step.as_deref(),
            from.as_deref(),
            &allowed.unwrap_or_default(),
        )? {
            Next::Approve(commands) => Ok(Pressed::Approve { commands }),
            Next::Start(words) => {
                let mut command = zo_build(&checkout)?;
                command.args(words);
                start(command, &shared).map(|()| Pressed::Started)
            }
        }
    })
    .await
    .map_err(|error| error.to_string())?
}

/// 「다시」·「멈추기」 of a build another script reports: `zo build act` runs the
/// command the order file names for it, outside the window — once the person
/// allowed it (`allowed`), as with a step's command.
#[tauri::command]
pub(crate) async fn build_act(
    state: State<'_, AppState>,
    worktree: Option<String>,
    build: String,
    act: String,
    allowed: Option<Vec<String>>,
) -> Result<Pressed, String> {
    let checkout = worktree.map_or_else(|| state.active_root(), PathBuf::from);
    let act = Act::of(&act).ok_or_else(|| format!("no act `{act}`: retry or stop"))?;
    tauri::async_runtime::spawn_blocking(move || {
        let shared = shared_of(&checkout)?;
        match press_act(
            &checkout,
            &shared,
            &build,
            act,
            &allowed.unwrap_or_default(),
        )? {
            Next::Approve(commands) => Ok(Pressed::Approve { commands }),
            Next::Start(words) => {
                let mut command = zo_build(&checkout)?;
                command.args(words);
                start_until(command, || false).map(|()| Pressed::Started)
            }
        }
    })
    .await
    .map_err(|error| error.to_string())?
}

/// 「기록 지우기」: `zo build forget` moves the ended run of `build` that began
/// at `started_ms` to the trash it can be brought back from for a day.
#[tauri::command]
pub(crate) async fn build_forget(
    state: State<'_, AppState>,
    worktree: Option<String>,
    build: String,
    started_ms: i64,
) -> Result<(), String> {
    zo_build_says(
        &state,
        worktree,
        &[
            "forget",
            "--build",
            &build,
            "--started",
            &started_ms.to_string(),
        ],
    )
    .await
}

/// 「되돌리기」 after 「기록 지우기」: `zo build restore` brings the run back.
#[tauri::command]
pub(crate) async fn build_restore(
    state: State<'_, AppState>,
    worktree: Option<String>,
    build: String,
    started_ms: i64,
) -> Result<(), String> {
    zo_build_says(
        &state,
        worktree,
        &[
            "restore",
            "--build",
            &build,
            "--started",
            &started_ms.to_string(),
        ],
    )
    .await
}

/// Runs `zo build <words>` for `worktree`'s project to its end, and says why
/// when it refused.
async fn zo_build_says(
    state: &State<'_, AppState>,
    worktree: Option<String>,
    words: &[&str],
) -> Result<(), String> {
    let checkout = worktree.map_or_else(|| state.active_root(), PathBuf::from);
    let words: Vec<String> = words.iter().map(|word| (*word).to_owned()).collect();
    tauri::async_runtime::spawn_blocking(move || {
        let mut command = zo_build(&checkout)?;
        let output = command
            .args(&words)
            .stdout(Stdio::null())
            .output()
            .map_err(|error| error.to_string())?;
        if output.status.success() {
            Ok(())
        } else {
            Err(refusal(&output.stderr))
        }
    })
    .await
    .map_err(|error| error.to_string())?
}

/// The verbs the board's build line presses on one job (`zo line <verb> <id>`).
const LINE_VERBS: [&str; 6] = ["retry", "withdraw", "front", "stop", "forget", "restore"];

/// `zo line …`: the machine's line, run from the person's home — the line is
/// the machine's, not a project's.
fn zo_line() -> Result<Command, String> {
    let home = dirs::home_dir().ok_or_else(|| "no home directory".to_owned())?;
    let bin = crate::zo_companion::zo_path_under(&home);
    if !bin.exists() {
        return Err(format!("zo is not installed at {}", bin.display()));
    }
    let mut command = crate::proc::quiet_command_on_path(&bin, crate::shell_path::hydrated());
    command
        .arg("line")
        .current_dir(&home)
        .stdin(Stdio::null())
        .stdout(Stdio::null());
    Ok(command)
}

async fn zo_line_says(words: Vec<String>) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let output = zo_line()?
            .args(&words)
            .output()
            .map_err(|error| error.to_string())?;
        if output.status.success() {
            Ok(())
        } else {
            Err(refusal(&output.stderr))
        }
    })
    .await
    .map_err(|error| error.to_string())?
}

/// A press on one job of the board's build line (11-08): run it again, take it
/// out of the queue, move it to the front, stop it, forget it (with its undo).
/// Only the verbs the board draws, on an id the line could have made.
#[tauri::command]
pub(crate) async fn line_act(job: String, verb: String) -> Result<(), String> {
    if !LINE_VERBS.contains(&verb.as_str()) {
        return Err(format!("`{verb}` is not a verb of the board's line"));
    }
    if !zerocode_core::line::is_job_id(&job) {
        return Err(format!("`{job}` is no job of the line"));
    }
    zo_line_says(vec![verb, job]).await
}

/// The board's 「줄 멈추기」 and 「줄 다시 열기」: `zo line pause --why …` or `zo line resume`.
#[tauri::command]
pub(crate) async fn line_pause(pause: bool, why: Option<String>) -> Result<(), String> {
    let words = if pause {
        let mut words = vec!["pause".to_owned()];
        if let Some(why) = why.filter(|why| !why.trim().is_empty()) {
            words.extend(["--why".to_owned(), why]);
        }
        words
    } else {
        vec!["resume".to_owned()]
    };
    zo_line_says(words).await
}

/// 「멈추기」: `zo build stop` for `builds` of `worktree`'s project. The runner
/// that holds the project is asked to stop; a run that waits for a person is
/// ended.
#[tauri::command]
pub(crate) async fn build_stop(
    state: State<'_, AppState>,
    worktree: Option<String>,
    builds: Vec<String>,
) -> Result<(), String> {
    let checkout = worktree.map_or_else(|| state.active_root(), PathBuf::from);
    tauri::async_runtime::spawn_blocking(move || {
        let file = build_file::read(&checkout)?;
        let ids: Vec<String> = build_run::queue(&file, &builds)?
            .iter()
            .map(|build| build.id.clone())
            .collect();
        let mut command = zo_build(&checkout)?;
        let output = command
            .arg("stop")
            .args(build_words(&ids))
            .stdout(Stdio::null())
            .output()
            .map_err(|error| error.to_string())?;
        if output.status.success() {
            Ok(())
        } else {
            Err(refusal(&output.stderr))
        }
    })
    .await
    .map_err(|error| error.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    const ORDER: &str = "[[build]]\nid = \"app\"\nstep = [{ id = \"make\", run = \"make\" }]\n\n\
        [[build]]\nid = \"release\"\nsource = \"report\"\n\n\
        [[build]]\nid = \"image\"\nstep = [\n  { id = \"build\", run = \"docker build .\" },\n  \
        { id = \"push\", run = \"docker push\", when = \"manual\" },\n]\n";

    fn checkout() -> (tempfile::TempDir, PathBuf) {
        let checkout = tempfile::tempdir().expect("a checkout");
        std::fs::create_dir_all(checkout.path().join(".git")).expect("its git folder");
        std::fs::create_dir_all(checkout.path().join(".zo")).expect("its .zo folder");
        std::fs::write(checkout.path().join(".zo/build.toml"), ORDER).expect("its order");
        let shared = shared_of(checkout.path()).expect("a repository");
        (checkout, shared)
    }

    fn words(list: &[&str]) -> Vec<String> {
        list.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn a_press_asks_for_the_commands_no_one_allowed_then_names_every_build() {
        let (checkout, shared) = checkout();

        let asked = press(checkout.path(), &shared, &[], None, None, &[]).expect("asked");
        let half = press(
            checkout.path(),
            &shared,
            &[],
            None,
            None,
            &words(&["make", "rm -rf /"]),
        )
        .expect("asked again");
        let all = words(&["docker build .", "docker push"]);
        let started = press(checkout.path(), &shared, &[], None, None, &all).expect("started");
        let pressed = press(
            checkout.path(),
            &shared,
            &words(&["image"]),
            Some("push"),
            None,
            &[],
        )
        .expect("the waiting step");
        let again = press(
            checkout.path(),
            &shared,
            &words(&["image"]),
            None,
            Some("push"),
            &[],
        )
        .expect("again from a step");

        assert_eq!(
            asked,
            Next::Approve(words(&["make", "docker build .", "docker push"]))
        );
        assert_eq!(
            half,
            Next::Approve(all.clone()),
            "only the file's commands are allowed"
        );
        assert_eq!(
            started,
            Next::Start(words(&["--build", "app", "--build", "image"]))
        );
        assert_eq!(
            pressed,
            Next::Start(words(&["--build", "image", "--step", "push"]))
        );
        assert_eq!(
            again,
            Next::Start(words(&["--build", "image", "--from", "push"])),
            "「…부터 다시」 starts a new run from that step"
        );
    }

    /// A reported build's retry (11-08): asked about until the person allows its
    /// command, then `zo build act`; a build naming no such act is refused.
    #[test]
    fn a_reported_builds_act_asks_first_then_names_the_act() {
        let (checkout, shared) = checkout();
        std::fs::write(
            checkout.path().join(".zo/build.toml"),
            "[[build]]\nid = \"release\"\nsource = \"report\"\nretry = \"tools/lane.sh ${sha}\"\n",
        )
        .expect("an order with a retry");
        let asked = press_act(checkout.path(), &shared, "release", Act::Retry, &[]).expect("asked");
        let started = press_act(
            checkout.path(),
            &shared,
            "release",
            Act::Retry,
            &words(&["tools/lane.sh ${sha}"]),
        )
        .expect("allowed and started");
        let none = press_act(checkout.path(), &shared, "release", Act::Stop, &[])
            .expect_err("no stop named");
        assert_eq!(asked, Next::Approve(words(&["tools/lane.sh ${sha}"])));
        assert_eq!(
            started,
            Next::Start(words(&["act", "--build", "release", "--act", "retry"]))
        );
        assert!(none.contains("names no `stop` command"), "{none}");
    }

    #[test]
    fn a_press_is_refused_while_a_runner_holds_the_project_or_without_an_order() {
        let (checkout, shared) = checkout();
        build_run::approve(&shared, &words(&["make", "docker build .", "docker push"]))
            .expect("allowed");
        let held = build_run::hold(&shared).expect("held").expect("free");

        let running = press(checkout.path(), &shared, &[], None, None, &[]).expect_err("held");
        drop(held);
        let reported = press(
            checkout.path(),
            &shared,
            &words(&["release"]),
            None,
            None,
            &[],
        )
        .expect_err("another script reports it");
        std::fs::remove_file(checkout.path().join(".zo/build.toml")).expect("no order");
        let none =
            press(checkout.path(), &shared, &[], None, None, &[]).expect_err("no order file");

        assert!(running.contains("already running"), "{running}");
        assert!(
            reported.contains("reported by another script"),
            "{reported}"
        );
        assert!(none.contains("has no .zo/build.toml"), "{none}");
    }

    #[test]
    fn a_refusal_is_said_without_zos_lead_its_exit_line_or_the_usage() {
        assert_eq!(
            refusal(
                b"zo build: builds are already running in this project\n\
                  zo: process exit reason=build-refused\n"
            ),
            "builds are already running in this project"
        );
        assert_eq!(
            refusal(
                b"zo build: these commands have not been allowed to run here:\n  make\n\
                  allow them with `zo build approve`\nzo: process exit reason=build-refused\n"
            ),
            "these commands have not been allowed to run here:\n  make\nallow them with `zo build approve`"
        );
        // A zo from before `zo build` refuses its flag, then prints its whole usage.
        assert_eq!(
            refusal(
                b"zo: process exit reason=error detail=unknown argument '--build'  zo [<prompt>...]\n\
                  unknown argument '--build'\n\nzo [<prompt>...] [--model <alias>]\nzo models [--json]\n"
            ),
            "unknown argument '--build'"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_start_says_why_zo_ended_at_once_and_leaves_a_runner_to_go_on_alone() {
        let shared = tempfile::tempdir().expect("a shared git folder");
        let sh = |script: &str| {
            let mut command = crate::proc::quiet_command("/bin/sh");
            command
                .arg("-c")
                .arg(script)
                .stdin(Stdio::null())
                .stdout(Stdio::null());
            command
        };

        let refused = start(
            sh("echo 'zo build: the order file has no build `nope`' >&2; exit 1"),
            shared.path(),
        );
        let passed = start(sh("exit 0"), shared.path());
        let runner = build_run::hold(shared.path()).expect("held").expect("free");
        let began = Instant::now();
        let alone = start(sh("sleep 2"), shared.path());

        assert_eq!(
            refused,
            Err("the order file has no build `nope`".to_owned())
        );
        assert_eq!(passed, Ok(()));
        assert_eq!(alone, Ok(()));
        assert!(
            began.elapsed() < START_WAIT,
            "a press answers once the runner holds the project"
        );
        drop(runner);
    }
}
