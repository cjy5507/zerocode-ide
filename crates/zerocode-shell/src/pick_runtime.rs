use super::*;

use tokio::io::AsyncReadExt as _;
use zerocode_core::pick::{HELPER_NAME, PickRequest, parse_answer};

/* ---- the native panel, out of process (t-2982) -----------------------------
 *
 * The window spawns `zerocode-pick` and waits for its stdout on the async
 * runtime. Nothing here touches AppKit on the main thread: the helper's own
 * process pays the remote-view wait that used to be the window's, and what
 * the window keeps is what a parent keeps of a child — its pid, to bring it
 * forward; a cancel, to kill it; a bound, past which it is killed anyway.
 * The seat that owns the desk and the clock is `pick_paths` in
 * `project_runtime.rs`; this file is the road under it, testable with a
 * shell script standing in for the helper. */

/// What the road came back with.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum PickOutcome {
    Chosen(Vec<PathBuf>),
    /// The person dismissed the panel — empty stdout, exit 0.
    Dismissed,
    /// The wait ran out and the helper was killed.
    TimedOut,
    /// The window cancelled it and the helper was killed.
    Cancelled,
    /// The helper could not be spawned, or said it could not open the panel.
    Failed(String),
}

/// The first reply keeps its value: a completed picker must never be polled again.
pub(super) enum PickStart {
    MainThread(Option<Duration>),
    Answered(PickOutcome),
}

pub(super) async fn wait_for_pick_start(
    asked_at: Instant,
    budget: Duration,
    marked: tokio::sync::oneshot::Receiver<Instant>,
    helper: &mut (impl std::future::Future<Output = PickOutcome> + Unpin),
) -> PickStart {
    tokio::select! {
        marked = tokio::time::timeout(budget, marked) => PickStart::MainThread(match marked {
            Ok(Ok(reached)) => Some(reached.saturating_duration_since(asked_at)),
            _ => None,
        }),
        outcome = helper => PickStart::Answered(outcome),
    }
}

/// Where the helper lives: beside this executable, always — the bundle's
/// own helper, the one this build was tested with, and in a development
/// tree the one cargo built into the same `target/` directory. Not `PATH`:
/// a Finder-launched window has launchd's, and the window's rule is that
/// nothing reads that directly (`agents_are_found_on_the_shells_path_not_the_apps`).
pub(super) fn pick_helper_program() -> Option<PathBuf> {
    let beside = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf));
    pick_helper_program_in(beside.as_deref())
}

/// The lookup with its input handed in, so it can be tested without moving
/// this test binary.
pub(super) fn pick_helper_program_in(beside: Option<&Path>) -> Option<PathBuf> {
    let name = format!("{HELPER_NAME}{}", std::env::consts::EXE_SUFFIX);
    beside
        .map(|dir| dir.join(&name))
        .filter(|candidate| candidate.is_file())
}

/// Spawn the helper and wait for its answer.
///
/// `on_spawn` hears the pid the moment the child exists — before any wait —
/// so the desk can hold it for a recall or a cancel. `cancel` resolving (or
/// its sender being dropped) kills the helper; so does `wait` running out.
/// The child is spawned with `kill_on_drop`, so a road dropped mid-wait —
/// the runtime shutting down — takes the helper with it rather than leaving
/// a panel standing for a window that is gone.
pub(super) async fn run_pick_helper(
    program: PathBuf,
    request: PickRequest,
    wait: Duration,
    cancel: tokio::sync::oneshot::Receiver<()>,
    on_spawn: impl FnOnce(u32),
) -> PickOutcome {
    let mut command = crate::proc::quiet_tokio_command(&program);
    command
        .args(request.argv())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => return PickOutcome::Failed(format!("{}: {error}", program.display())),
    };
    if let Some(pid) = child.id() {
        on_spawn(pid);
    }
    let Some(mut stdout) = child.stdout.take() else {
        return PickOutcome::Failed("the helper's stdout was not piped".to_string());
    };
    let mut stderr = child.stderr.take();

    enum Step {
        Answered(std::io::Result<(Vec<u8>, std::process::ExitStatus)>),
        TimedOut,
        Cancelled,
    }
    let read = async {
        let mut bytes = Vec::new();
        stdout.read_to_end(&mut bytes).await?;
        let status = child.wait().await?;
        Ok((bytes, status))
    };
    let step = tokio::select! {
        answered = tokio::time::timeout(wait, read) => match answered {
            Ok(answer) => Step::Answered(answer),
            Err(_) => Step::TimedOut,
        },
        _ = cancel => Step::Cancelled,
    };
    match step {
        Step::Answered(Ok((bytes, status))) => {
            if !status.success() {
                let mut said = String::new();
                if let Some(mut stderr) = stderr.take() {
                    let _ = stderr.read_to_string(&mut said).await;
                }
                let said = said.trim();
                return PickOutcome::Failed(if said.is_empty() {
                    format!("{HELPER_NAME}: {status}")
                } else {
                    said.to_string()
                });
            }
            let paths = parse_answer(&bytes);
            if paths.is_empty() {
                PickOutcome::Dismissed
            } else {
                PickOutcome::Chosen(paths)
            }
        }
        Step::Answered(Err(error)) => {
            let _ = child.kill().await;
            PickOutcome::Failed(format!("{HELPER_NAME}: {error}"))
        }
        Step::TimedOut => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            PickOutcome::TimedOut
        }
        Step::Cancelled => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            PickOutcome::Cancelled
        }
    }
}

/// Bring the helper's panel forward — it is another application's window
/// now, so the window activates that application by pid. Answers whether
/// the system knew the pid. Only macOS can lose a panel behind other apps
/// this way; elsewhere the answer is `false` and the window comes forward
/// instead.
pub(super) fn bring_helper_forward(pid: u32) -> bool {
    #[cfg(target_os = "macos")]
    {
        use objc2_app_kit::{NSApplicationActivationOptions, NSRunningApplication};
        let Ok(pid) = libc::pid_t::try_from(pid) else {
            return false;
        };
        NSRunningApplication::runningApplicationWithProcessIdentifier(pid).is_some_and(|helper| {
            // macOS 14 activates another application only cooperatively — the
            // active one (this window) names itself as yielding; the older
            // call is what there is before 14, and is kept as the second try.
            use objc2::runtime::NSObjectProtocol as _;
            let cooperative = helper
                .respondsToSelector(objc2::sel!(activateFromApplication:options:))
                && helper.activateFromApplication_options(
                    &NSRunningApplication::currentApplication(),
                    NSApplicationActivationOptions::empty(),
                );
            #[allow(deprecated)]
            {
                cooperative
                    || helper.activateWithOptions(
                        NSApplicationActivationOptions::ActivateIgnoringOtherApps,
                    )
            }
        })
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = pid;
        false
    }
}
