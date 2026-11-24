//! The evidence of a step, off the caller's road (docs/design/
//! computer-use-full-operator.md §1.4).
//!
//! A step's line and the frame after it are proof for a person reading later,
//! not an answer the agent is waiting on. They used to be taken on the reply
//! path — a settle and a second full capture before every action was
//! answered, which made the evidence recorder the ceiling of the operator's
//! hand. Now the caller hands the step to one writer and answers at once. The
//! writer keeps the steps in the order they happened, gives a surface its
//! settle (counted from the step, not from when the writer got to it) before
//! framing it, and frames only the last of a burst on one screen — or in one
//! window pictured alone, which is a surface of its own: a frame is taken
//! when the writer gets to it, so a step that a later action on the same
//! surface followed before then — anywhere in the queue, past waits and
//! looks — would be framed showing that later action, and is shown by that
//! later frame instead. A step that answered a picture — a look, or an app
//! act in a walk that frames its steps, whose answer carries the helper's
//! look after the act — is its own frame, kept at once (t-4229); an act's
//! only while that look is of the window the act went to, drawn by that
//! window alone, since the look lands where the app's focus leads (review
//! t-128939 §1) — else that window is taken again alone, wherever the act
//! went (review t-128987).
//!
//! Three rules keep the writer honest. A verb that reads or writes the step
//! log (`verdict`, `evidence`, `recipe-save`) waits for it to be written
//! first ([`written`]). A desktop frame is a picture the helper takes — the
//! window an act went to, alone, after an act that went no further than it,
//! so a person's own window over it never lands in the folder
//! (I-t123731-rc1); the window's region of the screen, or the whole display,
//! after one that took the desk and answered no picture — never a
//! `getAppState`, which would replace the element indices the agent was
//! just given. What the act did is heard as the helper answers it
//! (`computer_use::acted`), not read again from the step's words. And a
//! writer that falls behind drops frames before lines, bounds every capture
//! by a timeout, and is started again if it ever dies.
//!
//! A walk (a batch, a recipe) speaks to the same writer (plan D4·D10): it
//! says when it begins in a folder and whether its steps are framed — its
//! Flow's evidence level — and hands over its own record at the end, which
//! lands after the lines it reports as the folder's next `walk-NNN.json`.
//! A step that has no frame says why on its line (review 19), and a walk's
//! record that could not be written is said on stderr, not swallowed
//! (review 7).

use super::*;

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use zerocode_core::computer_use::ComputerMethod;
use zerocode_core::computer_use_protocol::execution::Acted;
use zerocode_core::computer_use_protocol::frame::ShotFrame;

use run_evidence::{FrameSkipped, Framing};

/// Past this many steps waiting behind the one being framed, the writer
/// writes lines without frames until it has caught up.
const BACKLOG_FRAMES: usize = 8;
/// The longest one frame may take before its step is written without it.
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(5);
/// The longest a log reader waits for the writer to catch up.
const WRITTEN_TIMEOUT: Duration = Duration::from_secs(10);
/// What the helper's `screenshotDesktop` is asked with to picture one window
/// alone, by its number: the frame of a step whose act went no further than
/// its window.
const WINDOW_ONLY: &str = "windowOnly";
/// The process that window is held to — the one the act found owning it —
/// so a number the window server gave out again pictures no other window.
const WINDOW_OWNER: &str = "windowOwner";

/// A frame taken: the picture, and where it sits on the screen.
struct Picture {
    png: Vec<u8>,
    placed: ShotFrame,
}

impl Picture {
    /// A picture whose pixels are its own space — an emulator's screen, a
    /// browser pane's — placed as the unit frame.
    fn unplaced(png: Vec<u8>) -> Self {
        Self {
            png,
            placed: ShotFrame::UNIT,
        }
    }
}

/// A surface a step can be framed on once it has settled.
trait Surface: Send + 'static {
    /// Which screen the frame is a picture of: a later action on the same
    /// screen is in this frame too, whatever part of it either one framed.
    fn key(&self) -> String;
    fn take(self) -> impl std::future::Future<Output = Result<Picture, FrameSkipped>> + Send;
}

/// The frame a step leaves.
enum Frame<S> {
    /// The step answered a picture itself: a look, or an app act whose walk
    /// keeps its picture. Kept as it is — no settle, nothing supersedes it.
    Own(Picture),
    /// The surface is framed once it has settled after the step.
    After(S),
    /// A verb that frames, left without its frame — and why.
    Skipped(FrameSkipped),
    /// A line without a frame: the verb frames nothing, or it was refused.
    None,
}

/// How a settled surface is framed.
enum Capture {
    /// A picture through the helper's `screenshotDesktop`, asked with
    /// `params`: the acted window alone (`windowOnly`, `windowOwner`) after
    /// an act that went no further than it; the whole display, or the acted
    /// window's region of it, after one that took the desk.
    Desktop { params: serde_json::Value },
    Emulator {
        platform: zerocode_core::computer_use::EmulatorPlatform,
        device: String,
    },
    Browser {
        app: AppHandle,
        label: String,
        control: Option<crate::browser_workspace::StepControl>,
    },
}

impl Surface for Capture {
    fn key(&self) -> String {
        match self {
            // A window pictured alone is its own surface (I-t123731-rc1): an
            // action in another window, or on the screen around it, is in
            // no picture of it, and a picture of either shows nothing of its
            // step — so only a later action in that window supersedes it.
            Self::Desktop { params } if params.get(WINDOW_ONLY).is_some() => format!(
                "computer:window:{}:{}",
                params[WINDOW_ONLY], params[WINDOW_OWNER]
            ),
            // The acted window's region and the whole display are one
            // screen: an action in one is in a picture of the other.
            Self::Desktop { .. } => "computer".to_string(),
            Self::Emulator { platform, device } => format!("emulator:{platform:?}:{device}"),
            Self::Browser { label, .. } => format!("browser:{label}"),
        }
    }

    async fn take(self) -> Result<Picture, FrameSkipped> {
        match self {
            Self::Desktop { params } => {
                // A window asked for alone that cannot be pictured says so:
                // nothing of the screen is taken in its place.
                let alone = params.get(WINDOW_ONLY).cloned();
                let failed = if alone.is_some() {
                    FrameSkipped::WindowUnavailable
                } else {
                    FrameSkipped::CaptureFailed
                };
                tauri::async_runtime::spawn_blocking(move || {
                    let answer = computer_use::call("screenshotDesktop", params).ok()?;
                    desktop_picture(alone.as_ref(), &answer)
                })
                .await
                .ok()
                .flatten()
                .ok_or(failed)
            }
            Self::Emulator { platform, device } => emulator_screenshot_bytes(platform, &device)
                .await
                .ok()
                .map(Picture::unplaced)
                .ok_or(FrameSkipped::CaptureFailed),
            Self::Browser {
                app,
                label,
                control,
            } => {
                let state = app.state::<AppState>();
                match cmd::fs::browser_evidence_png(&app, &state, &label, control.as_ref()).await {
                    Ok(Some(png)) => Ok(Picture::unplaced(png)),
                    Ok(None) => Err(FrameSkipped::ControlChanged),
                    Err(_) => Err(FrameSkipped::CaptureFailed),
                }
            }
        }
    }
}

/// The picture a `screenshotDesktop` answer carries, placed where the answer
/// says it sits. A window asked for `alone` is kept only when the answer says
/// that window was pictured alone (`windowOnly`): a helper that does not know
/// the request answers with the screen around it, which is never kept in its
/// place (I-t123731-rc1).
fn desktop_picture(
    alone: Option<&serde_json::Value>,
    answer: &serde_json::Value,
) -> Option<Picture> {
    if alone.is_some() && answer.get(WINDOW_ONLY) != alone {
        return None;
    }
    Some(Picture {
        png: computer_use::screenshot_png(answer)?,
        placed: ShotFrame::from_answer(answer).unwrap_or(ShotFrame::UNIT),
    })
}

/// One step, as the caller saw it happen.
struct Job<S> {
    dir: PathBuf,
    tool: &'static str,
    argv: Vec<String>,
    refusal: Option<String>,
    at_epoch_ms: i64,
    observation: Option<serde_json::Value>,
    at: tokio::time::Instant,
    frame: Frame<S>,
}

impl<S: Surface> Job<S> {
    fn new(
        dir: &Path,
        tool: &'static str,
        argv: &[String],
        answer: &zerocode_hookd::TeamAnswer,
        frame: Frame<S>,
        observation: Option<serde_json::Value>,
    ) -> Self {
        Self {
            dir: dir.to_path_buf(),
            tool,
            argv: argv.to_vec(),
            refusal: (answer.exit_code != 0).then(|| answer.stderr.clone()),
            at_epoch_ms: now_epoch_ms(),
            observation,
            at: tokio::time::Instant::now(),
            frame,
        }
    }

    /// The folder and screen a later frame of this step would show.
    fn framed_surface(&self) -> Option<(&Path, String)> {
        match &self.frame {
            Frame::After(surface) => Some((self.dir.as_path(), surface.key())),
            Frame::Own(_) | Frame::Skipped(_) | Frame::None => None,
        }
    }
}

/// What the writer is handed: a step, a reader asking to be told when every
/// step before it is written, or a walk's word — that it begins in a folder
/// (and whether its steps are framed), and its record when it ends.
enum Message<S> {
    Step(Job<S>),
    Written(tokio::sync::oneshot::Sender<()>),
    /// A walk begins in `dir`: `framed` is its Flow's evidence level. Until
    /// the walk's record arrives, the folder's steps are framed only if so;
    /// a lone command outside a walk is framed as before.
    WalkBegin {
        dir: PathBuf,
        framed: bool,
        identity: serde_json::Value,
    },
    /// A walk ended: its record, written after the lines before it in the
    /// queue as the folder's next `walk-NNN.json`.
    Walk {
        dir: PathBuf,
        report: serde_json::Value,
    },
}

static WRITER: Mutex<Option<UnboundedSender<Message<Capture>>>> = Mutex::new(None);

/// Hand a message to the one writer — starting it on first use, and again
/// if it ever stopped.
fn send(message: Message<Capture>) {
    let mut writer = WRITER
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let message = match writer.as_ref() {
        Some(sender) => match sender.send(message) {
            Ok(()) => return,
            Err(returned) => returned.0,
        },
        None => message,
    };
    let (sender, messages) = unbounded_channel();
    tauri::async_runtime::spawn(write_in_order(messages, EVIDENCE_SETTLE));
    let _ = sender.send(message);
    *writer = Some(sender);
}

/// Whether a verb reads or writes the step log — and so waits for the steps
/// already answered to be written first. Recipe evidence is linked by the
/// identity carried with each actual step, never by a predicted row count.
pub(super) fn reads_the_log(method: ComputerMethod) -> bool {
    matches!(
        method,
        ComputerMethod::Verdict
            | ComputerMethod::Evidence
            | ComputerMethod::RecipeSave
            | ComputerMethod::RecipeRun
    )
}

/// Wait until every step handed over before this call is written (bounded).
pub(super) async fn written() {
    let (done, wait) = tokio::sync::oneshot::channel();
    send(Message::Written(done));
    let _ = tokio::time::timeout(WRITTEN_TIMEOUT, wait).await;
}

/// The writer: steps in the order they happened, each settled screen framed
/// once, a burst framed by its last action; a walk's record after the lines
/// it reports.
async fn write_in_order<S: Surface>(mut messages: UnboundedReceiver<Message<S>>, settle: Duration) {
    let mut held: std::collections::VecDeque<Message<S>> = std::collections::VecDeque::new();
    // The folders a walk is going through whose Flow keeps no frames.
    let mut unframed: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    loop {
        let message = match held.pop_front() {
            Some(message) => message,
            None => match messages.recv().await {
                Some(message) => message,
                None => return,
            },
        };
        let job = match message {
            Message::Written(done) => {
                let _ = done.send(());
                continue;
            }
            Message::WalkBegin {
                dir,
                framed,
                identity,
            } => {
                run_evidence::emit("flow:begin", identity);
                if framed {
                    unframed.remove(&dir);
                } else {
                    unframed.insert(dir);
                }
                continue;
            }
            Message::Walk { dir, report } => {
                unframed.remove(&dir);
                let _ = tauri::async_runtime::spawn_blocking(move || {
                    if let Err(why) = run_evidence::record_walk(&dir, &report) {
                        eprintln!("evidence: a walk's record was not written: {why}");
                    }
                })
                .await;
                continue;
            }
            Message::Step(job) => job,
        };
        let Job {
            dir,
            tool,
            argv,
            refusal,
            at_epoch_ms,
            observation,
            at,
            frame,
        } = job;
        // The frame the writer settled on: a picture, or why there is none
        // (`None`: the step never frames).
        let frame: Result<Picture, Option<FrameSkipped>> = match frame {
            Frame::None => Err(None),
            _ if unframed.contains(&dir) => Err(Some(FrameSkipped::Off)),
            Frame::Skipped(why) => Err(Some(why)),
            Frame::Own(picture) => Ok(picture),
            Frame::After(capture) => {
                tokio::time::sleep_until(at + settle).await;
                // Everything handed over by now happened before this frame
                // would be taken.
                while let Ok(later) = messages.try_recv() {
                    held.push_back(later);
                }
                let screen = capture.key();
                let superseded = held.iter().any(|later| {
                    matches!(later, Message::Step(next)
                    if next.framed_surface().is_some_and(|(next_dir, next_screen)| {
                        next_dir == dir && next_screen == screen
                    }))
                });
                if superseded {
                    Err(Some(FrameSkipped::Superseded))
                } else if held.len() >= BACKLOG_FRAMES {
                    // Behind by more than the table: lines first, frames later.
                    Err(Some(FrameSkipped::Backlog))
                } else {
                    tokio::time::timeout(CAPTURE_TIMEOUT, capture.take())
                        .await
                        .unwrap_or(Err(FrameSkipped::CaptureFailed))
                        .map_err(Some)
                }
            }
        };
        let _ = tauri::async_runtime::spawn_blocking(move || {
            let outcome = refusal.as_deref().map_or(Ok(()), Err);
            let framing = match &frame {
                Ok(picture) => Framing::Picture {
                    png: &picture.png,
                    placed: picture.placed,
                },
                Err(Some(why)) => Framing::Skipped(*why),
                Err(None) => Framing::None,
            };
            run_evidence::record_measured(
                &dir,
                (at_epoch_ms, observation),
                tool,
                &argv,
                outcome,
                framing,
            );
        })
        .await;
    }
}

/// The answer's envelope, if it printed one: on stdout when it answered, on
/// stderr when it refused.
pub(super) fn envelope(answer: &zerocode_hookd::TeamAnswer) -> Option<serde_json::Value> {
    zerocode_core::computer_use_protocol::answer_envelope(&answer.stdout, &answer.stderr)
}

/// The picture a step already answered: the exported PNG its envelope names
/// — an explicit look's, or the helper's look after an app act, which a walk
/// that frames its steps leaves on the answer (`walked_step_flags`) — placed
/// where the answer says it sits (an app's at its window). Read before the
/// caller answers — the agent may take the file away after.
fn own_frame(result: Option<&serde_json::Value>) -> Option<Picture> {
    let result = result?;
    let png = std::fs::read(picture_path(result)?).ok()?;
    Some(Picture {
        png,
        placed: ShotFrame::from_answer(result).unwrap_or(ShotFrame::UNIT),
    })
}

/// The file an answer's picture was exported to, when it carried one.
fn picture_path(result: &serde_json::Value) -> Option<&str> {
    result.pointer("/screenshot/path")?.as_str()
}

/// Whether the picture an answer carries is of what its step did (review
/// t-128939 §1, t-128987 §2): a look's, of which no act was heard, is; an
/// act's only when its look is of the window the act went to — the same
/// number, the same owner (`acted`) — drawn by that window alone. The look
/// after an act is of the window the app's focus and window order lead to,
/// which may be another of its windows, where a person's other document
/// stands; an act that named no window has none to match it with; and a
/// window that would not paint itself is copied off the screen instead
/// (`SCREEN_COPY_ENGINE`, Windows' road), with whatever stands over it.
fn pictures_its_act(result: &serde_json::Value, acted: Option<Acted>) -> bool {
    let Some(acted) = acted else {
        return true;
    };
    let shows = |pointer: &str| result.pointer(pointer).and_then(serde_json::Value::as_u64);
    let copied_off_the_screen = result
        .pointer("/screenshotStatus/metadata/engine")
        .and_then(serde_json::Value::as_str)
        == Some(zerocode_core::computer_use_protocol::SCREEN_COPY_ENGINE);
    !copied_off_the_screen
        && acted.window.is_some_and(|window| {
            shows("/snapshot/window/id") == Some(window.id)
                && shows("/snapshot/app/pid") == Some(window.pid)
        })
}

/// Where a desktop step that took the desk is framed: the region of the
/// window an app action answered in its result (the screen as a person would
/// see it there), or the whole display. Never `getAppState`: that would
/// replace the element indices the agent was just handed, from a writer
/// running behind its answers.
fn desktop_capture(result: Option<&serde_json::Value>) -> Capture {
    let window = result.and_then(|result| {
        let window = result.pointer("/snapshot/window")?;
        let side = |key: &str| window.get(key).and_then(serde_json::Value::as_f64);
        Some(serde_json::json!({
            "x": side("x")?,
            "y": side("y")?,
            "width": side("width").filter(|width| *width > 0.0)?,
            "height": side("height").filter(|height| *height > 0.0)?,
        }))
    });
    Capture::Desktop {
        params: match window {
            Some(region) => serde_json::json!({ "region": region }),
            None => serde_json::json!({}),
        },
    }
}

/// Where a desktop step is framed, by what its act did (I-t123731-rc1).
/// An act that went no further than its window — an app's act the helper
/// ran in the background, a window moved or sized by its number, an app
/// opened behind — is framed as that window alone, by its number and the
/// process the act found owning it: what another window covers drawn as the
/// window itself draws it, never the screen around it, where a person's own
/// window may stand. `acted` is what the act did as the helper answered it,
/// and names the window the act went to, not the one a look after it lands
/// on; with no window named there is no picture, and the line says why. A
/// step that took the desk is framed as before (`desktop_capture`). When no
/// act of the step was heard — ZeroCode's own page pressed it, or it pressed
/// nothing — its words say whether it was for one window (`argv`, the words
/// that ran), whose number nobody gave.
fn desktop_frame(
    argv: &[String],
    result: Option<&serde_json::Value>,
    acted: Option<Acted>,
) -> Frame<Capture> {
    let in_its_window = acted.map_or_else(
        || {
            zerocode_core::computer_use::parse_command(argv).is_ok_and(|ran| {
                zerocode_core::computer_use_protocol::execution::went_in_the_background(&ran)
            })
        },
        |acted| acted.in_its_window,
    );
    if !in_its_window {
        return Frame::After(desktop_capture(result));
    }
    acted_window_alone(acted)
}

/// The window an act went to, framed alone — by its number and the process
/// the act found owning it, what another window covers drawn as the window
/// itself draws it — or, when the act named no window, no picture and the
/// line says why: never the screen around it, where a person's own window
/// may stand.
fn acted_window_alone(acted: Option<Acted>) -> Frame<Capture> {
    match acted.and_then(|acted| acted.window) {
        Some(window) => Frame::After(Capture::Desktop {
            params: serde_json::json!({ WINDOW_ONLY: window.id, WINDOW_OWNER: window.pid }),
        }),
        None => Frame::Skipped(FrameSkipped::WindowUnavailable),
    }
}

fn emulator_capture(argv: &[String]) -> Option<Capture> {
    let command = zerocode_core::computer_use::parse_emulator_command(argv).ok()?;
    Some(Capture::Emulator {
        platform: command.platform?,
        device: command.device?,
    })
}

/// A run's step on the browser road: the line, and the frame of the pane it
/// acted on once the page has had a moment to paint. `open` has no label to
/// frame yet; a refused action is a line without a frame.
pub(super) fn leave_browser_evidence(
    app: &AppHandle,
    dir: &Path,
    argv: &[String],
    answer: &zerocode_hookd::TeamAnswer,
    observation: Option<serde_json::Value>,
    control: Option<crate::browser_workspace::StepControl>,
) {
    let observation = run_evidence::browser_open_observation(argv, answer, observation);
    let verb = argv.first().map_or("", String::as_str);
    let Some(framed) = run_evidence::captures("browser", verb) else {
        return;
    };
    let label = if verb == "screenshot" {
        zerocode_core::agent_browser::parse_screenshot(argv)
            .ok()
            .map(|command| command.label)
    } else {
        argv.get(1).cloned()
    };
    let frame = match label {
        Some(label) if answer.exit_code == 0 && framed => Frame::After(Capture::Browser {
            app: app.clone(),
            label,
            control,
        }),
        _ => Frame::None,
    };
    send(Message::Step(Job::new(
        dir,
        "browser",
        argv,
        answer,
        frame,
        observation,
    )));
}

/// The frame a step on the desktop or emulator road leaves, from its answer
/// (§1.4): none for a refusal or a verb that frames nothing (`frames`); a
/// session folder past its table says `capped`; a picture the answer
/// exported is the step's own — kept at once, whatever follows it — when it
/// is of what the step did (`pictures_its_act`, by `acted`: what its act did,
/// as the helper answered it); else the frame `after` names from the
/// answer's result: a surface framed once it has settled, or why there is
/// none. The envelope is read once, for both.
fn frame_of<S>(
    dir: &Path,
    frames: bool,
    capped: bool,
    answer: &zerocode_hookd::TeamAnswer,
    acted: Option<Acted>,
    after: impl FnOnce(Option<&serde_json::Value>) -> Frame<S>,
) -> Frame<S> {
    if answer.exit_code != 0 || !frames {
        return Frame::None;
    }
    if capped && !computer_use::evidence::frame_allowed(dir) {
        // A session folder frames up to the table; the log keeps counting.
        return Frame::Skipped(FrameSkipped::Capped);
    }
    let envelope = envelope(answer);
    let result = envelope
        .as_ref()
        .and_then(|envelope| envelope.get("result"));
    match own_frame(result.filter(|result| pictures_its_act(result, acted))) {
        Some(picture) => Frame::Own(picture),
        None => after(result),
    }
}

/// A run's step on the emulator road: the line, and the frame of the device
/// the command named once it has painted (`frame_of`) — `open` has no device
/// to frame yet, a refused action is a line without a frame, an explicit
/// screenshot is its own frame. `argv` is the words after the door (no
/// `emulator`), the shape the log keeps. The one function the lone
/// `zerocode-emulator` command and every emulator step of a walk take
/// (`emulator_step`), the browser door's twin.
pub(super) fn leave_emulator_evidence(
    dir: &Path,
    argv: &[String],
    answer: &zerocode_hookd::TeamAnswer,
    observation: Option<serde_json::Value>,
) {
    let verb = argv.first().map_or("", String::as_str);
    let Some(frames) = run_evidence::captures("emulator", verb) else {
        return;
    };
    // No helper hears an emulator's act: a picture its answer exported is
    // an explicit screenshot's.
    let frame = frame_of(dir, frames, false, answer, None, |_| {
        emulator_capture(argv).map_or(Frame::None, Frame::After)
    });
    send(Message::Step(Job::new(
        dir,
        "emulator",
        argv,
        answer,
        frame,
        observation,
    )));
}

/// A run's step on the desktop road: the line, and the frame of the surface
/// it acted on (`computer_frame`) — a picture the answer exported of the
/// window its act went to is the step's own frame, and a picture of any
/// other is taken again as that window alone; else, by what its act did
/// (`desktop_frame`), the window it went to alone after an act that went no
/// further, or the acted window's region or the whole screen after one that
/// took the desk. `argv` is the words that ran; `logged` the words its line
/// keeps; `acted` what its act did, as the helper answered it. An `ssh`
/// command is not a surface, and the emulator has its own road
/// (`leave_emulator_evidence`).
pub(super) fn leave_computer_evidence(
    dir: &Path,
    argv: &[String],
    logged: &[String],
    answer: &zerocode_hookd::TeamAnswer,
    acted: Option<Acted>,
    capped: bool,
    observation: Option<serde_json::Value>,
) {
    let verb = match logged.first().map(String::as_str) {
        Some("ssh") | Some("emulator") | None => return,
        Some(verb) => verb,
    };
    let Some(frames) = run_evidence::captures("computer", verb) else {
        return;
    };
    let frame = computer_frame(dir, frames, capped, argv, answer, acted);
    send(Message::Step(Job::new(
        dir,
        "computer",
        logged,
        answer,
        frame,
        observation,
    )));
}

/// The frame a step on the desktop road leaves (`frame_of`): a picture its
/// answer exported, kept at once, when it is of the window its act went to,
/// drawn by it (review t-128939 §1, t-128987 §2). A picture of anything
/// else is taken again as that window alone, wherever the act went — after
/// one that took the desk too, where by the time of the frame a person's
/// own window or document may stand over the region the picture's window
/// held (review t-128987 §3) — or, when the act named no window, not at
/// all. A step whose answer carried no picture is framed by the surface its
/// act's road frames (`desktop_frame`). `argv` is the words that ran;
/// `acted` what its act did, as the helper answered it.
fn computer_frame(
    dir: &Path,
    frames: bool,
    capped: bool,
    argv: &[String],
    answer: &zerocode_hookd::TeamAnswer,
    acted: Option<Acted>,
) -> Frame<Capture> {
    frame_of(dir, frames, capped, answer, acted, |result| {
        let pictured_elsewhere = result.is_some_and(|result| {
            picture_path(result).is_some() && !pictures_its_act(result, acted)
        });
        if pictured_elsewhere {
            acted_window_alone(acted)
        } else {
            desktop_frame(argv, result, acted)
        }
    })
}

/// A walk begins in `dir` (plan D10): whether the steps that follow are
/// framed is its Flow's evidence level — `full` frames, `verdict-only` and
/// `off` do not. The walk's record and the verdict are written whatever the
/// level. A lone command outside a walk is framed as before.
pub(super) fn leave_walk_begin(
    dir: &Path,
    level: zerocode_core::computer_flow::EvidenceLevel,
    identity: serde_json::Value,
) {
    send(Message::WalkBegin {
        dir: dir.to_path_buf(),
        framed: level.frames(),
        identity,
    });
}

/// A walk's own record (plan D4): handed to the one writer, which writes it
/// after the lines it reports as the folder's next `walk-NNN.json`, and
/// frames the folder's later lone commands again.
pub(super) fn leave_walk_report(dir: &Path, report: serde_json::Value) {
    send(Message::Walk {
        dir: dir.to_path_buf(),
        report,
    });
}

/// An arena walk's step (`computer_use::arena`): the line as the live road
/// would leave it — the recipe's own words, the answer the recording gave —
/// and never a frame, since nothing was on a screen. A verb the log keeps
/// no line of leaves none here either.
pub(super) fn leave_arena_evidence(
    dir: &Path,
    tool: &'static str,
    argv: &[String],
    answer: &zerocode_hookd::TeamAnswer,
) {
    let verb = argv.first().map_or("", String::as_str);
    if run_evidence::captures(tool, verb).is_none() {
        return;
    }
    send(Message::Step(Job::new(
        dir,
        tool,
        argv,
        answer,
        Frame::None,
        run_evidence::observation(),
    )));
}

#[cfg(test)]
mod tests {
    use super::*;
    use zerocode_core::computer_use_protocol::execution::ActedWindow;

    const PNG: &[u8] = &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0];

    fn answered(ok: bool) -> zerocode_hookd::TeamAnswer {
        zerocode_hookd::TeamAnswer {
            stdout: String::new(),
            stderr: if ok { String::new() } else { "refused".into() },
            exit_code: i32::from(!ok),
        }
    }

    /// A surface whose frame is always the same picture.
    struct Fixed(&'static str);

    impl Surface for Fixed {
        fn key(&self) -> String {
            self.0.to_string()
        }

        async fn take(self) -> Result<Picture, FrameSkipped> {
            Ok(Picture::unplaced(PNG.to_vec()))
        }
    }

    struct Moved;

    impl Surface for Moved {
        fn key(&self) -> String {
            "browser:browser-1".to_string()
        }

        async fn take(self) -> Result<Picture, FrameSkipped> {
            Err(FrameSkipped::ControlChanged)
        }
    }

    fn step<S: Surface>(dir: &Path, verb: &str, frame: Frame<S>) -> Message<S> {
        Message::Step(Job::new(
            dir,
            "computer",
            &[verb.to_string()],
            &answered(true),
            frame,
            None,
        ))
    }

    fn fixed(surface: &'static str) -> Frame<Fixed> {
        Frame::After(Fixed(surface))
    }

    /// The answer's result, as the writer reads it once.
    fn result_of(answer: &zerocode_hookd::TeamAnswer) -> Option<serde_json::Value> {
        envelope(answer).and_then(|envelope| envelope.get("result").cloned())
    }

    /// Steps land in the order they happened; a burst on one surface is
    /// framed once, by its last step; another surface keeps its own frame;
    /// a look is its own frame and a refusal is a line; and a reader asking
    /// to be told is told only once every step before it is written.
    ///
    /// On tokio's paused clock (t-8938). The settle is time the writer
    /// COUNTS, and on the wall clock it was measured together with six
    /// lines and three pictures written to disk through the blocking pool —
    /// 651 ms against a 400 ms bound in a loaded gate, the settle itself
    /// never the slow part. Paused, the clock moves only by the timers the
    /// writer waits on, so the elapsed time IS the settles it slept: one
    /// for the burst when it is counted from the step, four when it is
    /// added per framed step.
    #[tokio::test(start_paused = true)]
    async fn the_writer_keeps_order_frames_a_burst_once_and_answers_a_reader_after_it() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (writer, messages) = unbounded_channel();
        let settle = Duration::from_millis(200);
        let written = tokio::spawn(write_in_order(messages, settle));
        let started = tokio::time::Instant::now();
        for (verb, frame) in [
            ("click", fixed("desktop")),
            ("type", fixed("desktop")),
            ("key", fixed("desktop")),
            ("tap", fixed("phone")),
            ("screenshot", Frame::Own(Picture::unplaced(PNG.to_vec()))),
        ] {
            writer.send(step(dir.path(), verb, frame)).unwrap();
        }
        let refused = Message::Step(Job::new(
            dir.path(),
            "computer",
            &["click".to_string()],
            &answered(false),
            Frame::None,
            None,
        ));
        writer.send(refused).unwrap();
        let (done, told) = tokio::sync::oneshot::channel();
        writer.send(Message::Written(done)).unwrap();
        told.await.expect("the reader is told");
        let steps = run_evidence::steps_in(dir.path());
        let verbs: Vec<&str> = steps.iter().map(|step| step.verb.as_str()).collect();
        assert_eq!(
            verbs,
            ["click", "type", "key", "tap", "screenshot", "click"],
            "every step before the reader is written, in order"
        );
        let shot: Vec<bool> = steps.iter().map(|step| step.shot.is_some()).collect();
        assert_eq!(
            shot,
            [false, false, true, true, true, false],
            "the burst's last desktop step, the phone and the look are framed"
        );
        assert!(!steps[5].ok, "a refusal is a line");
        let slept = started.elapsed();
        assert!(
            slept >= settle && slept < settle * 2,
            "the settle is counted from the step, not added per framed step: {slept:?}"
        );
        drop(writer);
        written.await.unwrap();
    }

    /// A frame is taken when the writer gets to it: a step a later action on
    /// the same screen followed — past a wait that frames nothing — is shown
    /// by that later frame, not by a picture of the batch's end labelled
    /// with its own name.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_step_is_not_framed_showing_a_later_action_on_its_screen() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (writer, messages) = unbounded_channel();
        for (verb, frame) in [
            ("click", fixed("computer")),
            ("wait", Frame::None),
            ("key", fixed("computer")),
            ("tap", fixed("phone")),
            ("wait", Frame::None),
        ] {
            writer.send(step(dir.path(), verb, frame)).unwrap();
        }
        drop(writer);
        write_in_order(messages, Duration::from_millis(50)).await;
        let shot: Vec<bool> = run_evidence::steps_in(dir.path())
            .iter()
            .map(|step| step.shot.is_some())
            .collect();
        assert_eq!(
            shot,
            [false, false, true, true, false],
            "the click is shown by the key's frame; the phone keeps its own"
        );
        let region = serde_json::json!({ "region": { "x": 1, "y": 2, "width": 3, "height": 4 } });
        assert_eq!(
            Capture::Desktop { params: region }.key(),
            Capture::Desktop {
                params: serde_json::json!({})
            }
            .key(),
            "a window's region and the whole display are one screen"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_step_frame_whose_control_moved_is_a_line_without_a_picture() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (writer, messages) = unbounded_channel();
        let click = ["click", "browser-1", "#send"].map(String::from);
        writer
            .send(Message::Step(Job::new(
                dir.path(),
                "browser",
                &click,
                &answered(true),
                Frame::After(Moved),
                None,
            )))
            .unwrap();
        drop(writer);
        write_in_order(messages, Duration::ZERO).await;
        let steps = run_evidence::steps_in(dir.path());
        assert_eq!(steps.len(), 1);
        assert!(steps[0].shot.is_none());
        assert_eq!(steps[0].frame_skipped, Some(FrameSkipped::ControlChanged));
        let pictures = std::fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "png"))
            .count();
        assert_eq!(pictures, 0);
    }

    /// Past the backlog table the writer writes lines without frames.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_writer_behind_writes_lines_before_frames() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (writer, messages) = unbounded_channel();
        let surfaces = ["a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l"];
        for surface in surfaces {
            writer
                .send(step(dir.path(), "click", fixed(surface)))
                .unwrap();
        }
        drop(writer);
        write_in_order(messages, Duration::ZERO).await;
        let shot: Vec<bool> = run_evidence::steps_in(dir.path())
            .iter()
            .map(|step| step.shot.is_some())
            .collect();
        assert_eq!(shot.len(), surfaces.len(), "every line is written");
        assert!(
            !shot[0] && *shot.last().unwrap(),
            "the first steps, far behind, are lines; the last ones are framed: {shot:?}"
        );
    }

    /// A step whose answer exported a picture is framed by it, at once and
    /// whatever follows (t-4229): an explicit look's, or — in a walk that
    /// frames its steps, which leaves the picture flag on — the helper's
    /// look after an app act, of the window the act went to, placed at the
    /// window it shows. Twenty acts at a replay's pace all keep their frame,
    /// where the same twenty framed after a settle keep one (the desk's 1 of
    /// 20); a walk that frames nothing keeps none, as before. No picture:
    /// framed after the settle.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_step_that_answered_its_own_picture_is_framed_by_it_not_by_a_later_capture() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("shot.png");
        std::fs::write(&file, PNG).unwrap();
        let window = serde_json::json!({ "app": { "pid": 501 },
            "window": { "id": 7, "x": 10, "y": 20, "width": 300, "height": 200 } });
        // The act went to the window its look shows (review t-128939 §1).
        let acted = heard(true, Some((7, 501)));
        let mut pictured = answered(true);
        pictured.stdout = format!(
            "{}\n",
            serde_json::json!({ "ok": true, "result": {
                "snapshot": window, "screenshot": { "path": file, "scale": 2.0 } } })
        );
        let mut unpictured = answered(true);
        unpictured.stdout = format!(
            "{}\n",
            serde_json::json!({ "ok": true, "result": { "snapshot": window } })
        );
        let own = own_frame(result_of(&pictured).as_ref()).expect("the exported picture");
        assert_eq!(own.png, PNG);
        assert_eq!(
            own.placed,
            ShotFrame::new((10.0, 20.0), 2.0).unwrap(),
            "an app's picture sits at its window"
        );
        assert!(own_frame(result_of(&unpictured).as_ref()).is_none());
        let framed_by = |answer: &zerocode_hookd::TeamAnswer, frames: bool| {
            frame_of(dir.path(), frames, false, answer, acted, |_| {
                Frame::After(Fixed("computer"))
            })
        };
        assert!(matches!(framed_by(&pictured, true), Frame::Own(_)));
        assert!(
            matches!(framed_by(&unpictured, true), Frame::After(_)),
            "no picture: framed once the screen settles, as before"
        );
        assert!(
            matches!(framed_by(&pictured, false), Frame::None),
            "a verb that frames nothing keeps no picture"
        );
        assert!(
            matches!(framed_by(&answered(false), true), Frame::None),
            "a refusal is a line"
        );
        let click = ["click", "--app", "Mail", "--text", "Amber"].map(String::from);
        let mut kept = std::collections::BTreeMap::new();
        for (name, answer, framed) in [
            ("own", &pictured, true),
            ("after", &unpictured, true),
            ("off", &pictured, false),
        ] {
            let folder = tempfile::tempdir().expect("tempdir");
            let (writer, messages) = unbounded_channel();
            writer
                .send(Message::WalkBegin {
                    dir: folder.path().to_path_buf(),
                    framed,
                    identity: serde_json::Value::Null,
                })
                .unwrap();
            for _ in 0..20 {
                let frame = frame_of(folder.path(), true, false, answer, acted, |_| {
                    Frame::After(Fixed("computer"))
                });
                writer
                    .send(Message::Step(Job::new(
                        folder.path(),
                        "computer",
                        &click,
                        answer,
                        frame,
                        None,
                    )))
                    .unwrap();
            }
            drop(writer);
            write_in_order(messages, Duration::ZERO).await;
            let steps = run_evidence::steps_in(folder.path());
            assert_eq!(steps.len(), 20, "{name}: every line is written");
            let skipped = |why: FrameSkipped| {
                steps
                    .iter()
                    .filter(|step| step.frame_skipped == Some(why))
                    .count()
            };
            kept.insert(
                name,
                (
                    steps.iter().filter(|step| step.shot.is_some()).count(),
                    skipped(FrameSkipped::Superseded),
                    skipped(FrameSkipped::Off),
                ),
            );
        }
        assert_eq!(
            kept["own"],
            (20, 0, 0),
            "every act keeps its own frame: {kept:?}"
        );
        assert_eq!(
            kept["after"],
            (1, 19, 0),
            "framed after the settle, a replay's pace keeps one in twenty: {kept:?}"
        );
        assert_eq!(
            kept["off"],
            (0, 0, 20),
            "a walk that frames nothing keeps none: {kept:?}"
        );
    }

    /// An app action is framed by its window's region of the screen; a
    /// desktop action by the whole display — and neither asks `getAppState`.
    #[test]
    fn a_desktop_frame_is_a_picture_of_the_screen_never_the_apps_tree() {
        let mut answer = answered(true);
        answer.stdout = format!(
            "{}\n",
            serde_json::json!({ "ok": true, "result": { "snapshot": {
                "window": { "id": 7, "x": 10, "y": 20, "width": 800, "height": 600 } } } })
        );
        let Capture::Desktop { params } = desktop_capture(result_of(&answer).as_ref()) else {
            panic!("a desktop capture");
        };
        assert_eq!(
            params,
            serde_json::json!({ "region": { "x": 10.0, "y": 20.0, "width": 800.0, "height": 600.0 } })
        );
        let Capture::Desktop { params } = desktop_capture(result_of(&answered(true)).as_ref())
        else {
            panic!("a desktop capture");
        };
        assert_eq!(
            params,
            serde_json::json!({}),
            "no window answered: the whole display"
        );
        assert!(reads_the_log(ComputerMethod::Verdict));
        assert!(
            reads_the_log(ComputerMethod::Evidence)
                && reads_the_log(ComputerMethod::RecipeSave)
                && reads_the_log(ComputerMethod::RecipeRun)
        );
        assert!(!reads_the_log(ComputerMethod::Screenshot));
    }

    /// What a desktop step's frame asks the helper's `screenshotDesktop` for,
    /// or why it asks nothing.
    fn asked(frame: Frame<Capture>) -> Result<serde_json::Value, Option<FrameSkipped>> {
        match frame {
            Frame::After(Capture::Desktop { params }) => Ok(params),
            Frame::Skipped(why) => Err(Some(why)),
            Frame::After(_) | Frame::Own(_) | Frame::None => Err(None),
        }
    }

    /// A step no act of which was heard — ZeroCode's own page pressed it, or
    /// it pressed nothing — is framed by its words (I-t123731-rc1): one that
    /// took the desk (`--foreground`) by its window's region of the screen,
    /// as before, and a desktop action by the whole display; one its words
    /// send to its window leaves no picture, since no act named the window it
    /// went to — not the look its answer carries, not the window its words
    /// name — and never the screen around it. The words are the ones that
    /// ran: a walk sends its recipe's desktop `key` as the app's `press-key`.
    #[test]
    fn a_step_no_act_of_which_was_heard_is_framed_by_its_words_and_names_no_window() {
        let looked = answered_with(serde_json::json!({
            "action": { "targetWindowId": 7, "background": { "requested": true } },
            "snapshot": { "window": { "id": 7, "x": 10, "y": 20, "width": 800, "height": 600 } },
        }));
        let region = serde_json::json!({
            "region": { "x": 10.0, "y": 20.0, "width": 800.0, "height": 600.0 }
        });
        let click = words(&["click", "--app", "Notes", "--element-index", "3", "--json"]);
        let mut foreground = click.clone();
        foreground.push("--foreground".into());
        assert_eq!(
            asked(desktop_frame(&foreground, looked.as_ref(), None)),
            Ok(region.clone()),
            "a step that took the desk: its window's region of the screen, as before"
        );
        assert_eq!(
            asked(desktop_frame(
                &words(&["mouse-click", "--x", "1", "--y", "1", "--foreground"]),
                None,
                None
            )),
            Ok(serde_json::json!({})),
            "a desktop action: the whole display, as before"
        );
        if !cfg!(target_os = "macos") {
            // No background road here: no step goes down one.
            assert_eq!(
                asked(desktop_frame(&click, looked.as_ref(), None)),
                Ok(region)
            );
            return;
        }
        for (step, answer) in [
            (click.clone(), looked.clone()),
            (
                words(&["press-key", "--app", "TextEdit", "--key", "cmd+n", "--json"]),
                looked.clone(),
            ),
            (
                words(&[
                    "click",
                    "--app",
                    "Notes",
                    "--window-id",
                    "5",
                    "--element-index",
                    "3",
                ]),
                None,
            ),
            (
                words(&["launch", "--app", "Notes"]),
                answered_with(serde_json::json!({ "windows": 1 })),
            ),
        ] {
            assert_eq!(
                asked(desktop_frame(&step, answer.as_ref(), None)),
                Err(Some(FrameSkipped::WindowUnavailable)),
                "{step:?}: no act named its window, so no picture — never the screen"
            );
        }
    }

    fn words(list: &[&str]) -> Vec<String> {
        list.iter().map(|word| (*word).to_string()).collect()
    }

    /// A step's result, as its envelope carries it to the writer.
    fn answered_with(result: serde_json::Value) -> Option<serde_json::Value> {
        let mut answer = answered(true);
        answer.stdout = format!("{}\n", serde_json::json!({ "ok": true, "result": result }));
        result_of(&answer)
    }

    /// What a step's act did, as the helper answered it: whether it went no
    /// further than its window, and that window (number, owner) if named.
    fn heard(in_its_window: bool, window: Option<(u64, u64)>) -> Option<Acted> {
        Some(Acted {
            in_its_window,
            window: window.map(|(id, pid)| ActedWindow { id, pid }),
        })
    }

    /// A step is framed by the road its act ran, as the helper answered it —
    /// not by its words read again (I-t123731-rc1, review t-128744 §1). The
    /// shared desktop sends a mark click asked `--foreground` to the
    /// background (the mark's press carries no `--foreground`), and a click
    /// asked `--background --foreground` goes there too: each is framed as
    /// the window it went to, alone — never the screen around it. An act the
    /// helper ran on the desk instead, by borrowing the desktop, is framed by
    /// its window's region of the screen, as before.
    #[test]
    fn a_step_is_framed_by_the_road_its_act_ran_not_by_its_words() {
        let looked = answered_with(serde_json::json!({
            "action": { "targetWindowId": 7, "background": { "requested": true } },
            "snapshot": { "window": { "id": 7, "x": 10, "y": 20, "width": 800, "height": 600 } },
        }));
        for asked_for in [
            words(&[
                "click",
                "--mark",
                "3",
                "--look",
                "L1",
                "--foreground",
                "--json",
            ]),
            words(&[
                "click",
                "--app",
                "Notes",
                "--element-index",
                "3",
                "--background",
                "--foreground",
                "--json",
            ]),
        ] {
            assert_eq!(
                asked(desktop_frame(
                    &asked_for,
                    looked.as_ref(),
                    heard(true, Some((7, 501)))
                )),
                Ok(serde_json::json!({ "windowOnly": 7, "windowOwner": 501 })),
                "{asked_for:?} went in the background: its window alone, never the screen"
            );
        }
        let borrowed = answered_with(serde_json::json!({
            "action": { "targetWindowId": 7, "borrowedDesktop": true },
            "snapshot": { "window": { "id": 7, "x": 10, "y": 20, "width": 800, "height": 600 } },
        }));
        assert_eq!(
            asked(desktop_frame(
                &words(&["click", "--app", "Notes", "--element-index", "3", "--json"]),
                borrowed.as_ref(),
                heard(false, Some((7, 501)))
            )),
            Ok(serde_json::json!({
                "region": { "x": 10.0, "y": 20.0, "width": 800.0, "height": 600.0 }
            })),
            "the helper borrowed the desktop: the window's region of the screen, as before"
        );
    }

    /// A window moved, sized, minimized, zoomed or closed by its number is
    /// framed as that window alone — the act touched nothing else, with or
    /// without `--foreground` — never as the whole display, whose other
    /// windows may be a person's (I-t123731-rc1, review t-128744 §1). Its
    /// answer has no look to give a region by. Focusing a window brings its
    /// app in front of the person: the screen, as before.
    #[test]
    fn a_window_moved_by_its_number_is_framed_as_that_window_alone() {
        let window = |action: &str| {
            answered_with(serde_json::json!({
                "windowId": 17,
                "pid": 501,
                "action": action,
                "frame": { "x": 10, "y": 20, "width": 320, "height": 200 },
            }))
        };
        assert_eq!(
            asked(desktop_frame(
                &words(&[
                    "window-move",
                    "--id",
                    "17",
                    "--x",
                    "10",
                    "--y",
                    "20",
                    "--json"
                ]),
                window("move").as_ref(),
                heard(true, Some((17, 501)))
            )),
            Ok(serde_json::json!({ "windowOnly": 17, "windowOwner": 501 })),
            "the moved window alone, by its number and its owner"
        );
        assert_eq!(
            asked(desktop_frame(
                &words(&["window-focus", "--id", "17", "--json"]),
                window("focus").as_ref(),
                heard(false, Some((17, 501)))
            )),
            Ok(serde_json::json!({})),
            "a window brought in front: the screen, as before"
        );
    }

    /// A step is framed by the window its act went to, as the helper found
    /// it then — its number and its owner — not by the window a look after
    /// the act lands on, which may be another window of the same app once
    /// the focus moved (I-t123731-rc1, review t-128744 §1). An act that went
    /// in the background and named no window leaves no picture.
    #[test]
    fn a_step_is_framed_by_the_window_its_act_went_to_not_the_one_a_later_look_shows() {
        let later = answered_with(serde_json::json!({
            "action": {
                "targetWindowId": 19,
                "actedWindow": { "id": 7, "pid": 501 },
                "background": { "requested": true },
            },
            "snapshot": { "window": { "id": 19, "x": 10, "y": 20, "width": 800, "height": 600 } },
        }));
        assert_eq!(
            asked(desktop_frame(
                &words(&["click", "--app", "Notes", "--element-index", "3", "--json"]),
                later.as_ref(),
                heard(true, Some((7, 501)))
            )),
            Ok(serde_json::json!({ "windowOnly": 7, "windowOwner": 501 })),
            "the window the act went to, never the one the look after it shows"
        );
        assert_eq!(
            asked(desktop_frame(
                &words(&["launch", "--app", "Notes"]),
                answered_with(serde_json::json!({ "windows": 1 })).as_ref(),
                heard(true, None)
            )),
            Err(Some(FrameSkipped::WindowUnavailable)),
            "an app opened behind names no window: no picture, never the whole display"
        );
    }

    /// The picture an act's answer carries frames its step only when it is
    /// of the window the act went to — the same number, the same owner
    /// (I-t123731-rc1, review t-128939 §1). A walk asks each app act for the
    /// look after it (`--full-state`), and with no window held to, that look
    /// lands where the app's focus and window order lead: another of the
    /// app's windows once the focus moved, where a person's other document
    /// may stand. A picture that is not of the acted window, or that no act
    /// names a window to match, frames nothing: the step is framed by the
    /// window its act went to alone, wherever the act went — after one that
    /// took the desk too, never the screen where the other window stood
    /// (review t-128987 §3) — or, when the act named none, by no picture,
    /// its line saying why. A look, of which no act was heard, keeps its own
    /// picture, as before. Every case is framed before the one assertion
    /// reads them all.
    #[test]
    fn an_acts_own_picture_frames_its_step_only_when_it_is_of_the_window_the_act_went_to() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("look.png");
        std::fs::write(&file, PNG).unwrap();
        // An answer whose look, and the picture it carries, is of window
        // `id` of process `pid`; an act's names the window it acted in.
        let looked_at = |id: u64, pid: u64, act: bool| {
            let mut result = serde_json::json!({
                "snapshot": {
                    "app": { "name": "Notes", "pid": pid },
                    "window": { "id": id, "x": 10, "y": 20, "width": 800, "height": 600 },
                },
                "screenshot": { "path": file, "scale": 2.0 },
            });
            if act {
                result["action"] = serde_json::json!({
                    "targetWindowId": id,
                    "actedWindow": { "id": 7, "pid": 501 },
                    "background": { "requested": true },
                });
            }
            let mut answer = answered(true);
            answer.stdout = format!("{}\n", serde_json::json!({ "ok": true, "result": result }));
            answer
        };
        let click = words(&[
            "click",
            "--app",
            "Notes",
            "--element-index",
            "3",
            "--full-state",
            "--json",
        ]);
        let own = || -> Result<serde_json::Value, Option<FrameSkipped>> {
            Ok(serde_json::json!("the picture its answer carries"))
        };
        let framed = |answer: &zerocode_hookd::TeamAnswer, acted: Option<Acted>| {
            let frame = computer_frame(dir.path(), true, false, &click, answer, acted);
            if matches!(frame, Frame::Own(_)) {
                own()
            } else {
                asked(frame)
            }
        };
        // The act went to window 7 of process 501.
        let in_seven = heard(true, Some((7, 501)));
        let took_the_desk = heard(false, Some((7, 501)));
        let elsewhere = looked_at(19, 501, true);
        let seen = [
            framed(&looked_at(7, 501, true), in_seven),
            framed(&elsewhere, in_seven),
            framed(&looked_at(7, 777, true), in_seven),
            framed(&elsewhere, heard(true, None)),
            framed(&elsewhere, took_the_desk),
            framed(&looked_at(19, 501, false), None),
        ];
        let alone = Ok(serde_json::json!({ "windowOnly": 7, "windowOwner": 501 }));
        assert_eq!(
            seen,
            [
                // Of the window the act went to: its frame, at once, as before.
                own(),
                // Of another window the focus moved to: the acted window alone.
                alone.clone(),
                // Of its number, now another process's: the acted window alone.
                alone.clone(),
                // An act that named no window (an older helper): nothing to
                // match the picture with — none, and the line says why.
                Err(Some(FrameSkipped::WindowUnavailable)),
                // Of another window, after an act that took the desk: the
                // acted window alone too, never the screen where the other
                // window stood (review t-128987 §3).
                alone,
                // A look, of which no act was heard: its own picture, as before.
                own(),
            ],
            "an act's own picture frames its step only when it is of the window the act went to"
        );
    }

    /// After an act that took the desk, a picture its answer carried of
    /// another window is taken again as the window the act went to, alone —
    /// by its number and its owner — never as the region where the other
    /// window stood, nor as the whole display when the answer gave no place
    /// (I-t123731-rc1, review t-128987 §3): by the time the frame is taken a
    /// person's own window or document may stand over that region. An act
    /// that named no window (an older helper) leaves no picture, its line
    /// saying why. A step whose answer carried no picture is framed as it
    /// was: its window's region of the screen, or the whole display. Every
    /// case is framed before the one assertion reads them all.
    #[test]
    fn a_picture_of_another_window_after_a_desk_act_is_taken_again_as_the_acted_window_alone() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("look.png");
        std::fs::write(&file, PNG).unwrap();
        // The answer to an act the helper ran on the desk (it borrowed the
        // desktop) in window 7, whose look is of window 19 of process 501:
        // with the picture it carries or none, its place on the screen or none.
        let answered_after = |pictured: bool, placed: bool| {
            let window = if placed {
                serde_json::json!({ "id": 19, "x": 10, "y": 20, "width": 800, "height": 600 })
            } else {
                serde_json::json!({ "id": 19 })
            };
            let mut result = serde_json::json!({
                "action": {
                    "targetWindowId": 19,
                    "actedWindow": { "id": 7, "pid": 501 },
                    "borrowedDesktop": true,
                },
                "snapshot": { "app": { "name": "Notes", "pid": 501 }, "window": window },
            });
            if pictured {
                result["screenshot"] = serde_json::json!({ "path": file, "scale": 2.0 });
            }
            let mut answer = answered(true);
            answer.stdout = format!("{}\n", serde_json::json!({ "ok": true, "result": result }));
            answer
        };
        let click = words(&[
            "click",
            "--app",
            "Notes",
            "--element-index",
            "3",
            "--full-state",
            "--json",
        ]);
        let framed = |answer: &zerocode_hookd::TeamAnswer, acted: Option<Acted>| {
            asked(computer_frame(
                dir.path(),
                true,
                false,
                &click,
                answer,
                acted,
            ))
        };
        let on_the_desk = heard(false, Some((7, 501)));
        let seen = [
            framed(&answered_after(true, false), on_the_desk),
            framed(&answered_after(true, true), heard(false, None)),
            framed(&answered_after(false, true), on_the_desk),
            framed(&answered_after(false, false), on_the_desk),
        ];
        assert_eq!(
            seen,
            [
                // Of another window, its place not given: the acted window
                // alone, never the whole display.
                Ok(serde_json::json!({ "windowOnly": 7, "windowOwner": 501 })),
                // An act that named no window: no picture, the line says why.
                Err(Some(FrameSkipped::WindowUnavailable)),
                // No picture in the answer: its window's region, as before.
                Ok(serde_json::json!({
                    "region": { "x": 10.0, "y": 20.0, "width": 800.0, "height": 600.0 }
                })),
                // No picture and no place: the whole display, as before.
                Ok(serde_json::json!({})),
            ],
            "a desk act's picture of another window is taken again as the acted window alone"
        );
    }

    /// A Windows app act's answer, as the Windows provider builds it
    /// (`ActionMetadata`, `screenshotStatus`), frames its step by the picture
    /// it carries when that picture is of the window the act went to, drawn
    /// by the window itself (`printWindow`): the provider names that window
    /// on its answer, as the macOS helper does (review t-128987 §2), so a
    /// recipe's steps on Windows keep their frames. A picture the provider
    /// copied off the screen instead (`screenBlt`), overlaps and all, or a
    /// look of another window than the act's, is not the step's: the acted
    /// window alone is asked for — which Windows refuses, so the line says
    /// why — and a person's window over the acted one never lands in the
    /// folder. Every case is framed before the one assertion reads them all.
    #[test]
    fn a_windows_acts_own_picture_frames_its_step_when_its_window_drew_it_alone() {
        use zerocode_core::computer_use_protocol::{
            ActionMetadata, ActionPath, SCREEN_COPY_ENGINE, ScreenshotMetadata, ScreenshotStatus,
        };
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("look.png");
        std::fs::write(&file, PNG).unwrap();
        // The provider's answer to an act it checked against window `acted`
        // of process 501, whose look after it is of window 19, drawn by
        // `engine` — and what the shell hears of the act: Windows has no
        // background road.
        let answered_on_windows = |acted: u64, engine: &str| {
            let mut action = ActionMetadata::new(ActionPath::Accessibility).named("Invoke");
            action.target_window_id = 19;
            action.acted_window = Some(ActedWindow {
                id: acted,
                pid: 501,
            });
            let result = serde_json::json!({
                "snapshot": {
                    "app": { "name": "Notepad", "pid": 501 },
                    "window": { "id": 19, "x": 10, "y": 20, "width": 800, "height": 600 },
                },
                "screenshot": { "path": file, "scale": 1.0 },
                "screenshotStatus": ScreenshotStatus::Captured(ScreenshotMetadata {
                    engine: engine.to_string(),
                    window_id: 19,
                }),
                "action": action,
            });
            let act =
                zerocode_core::computer_use_protocol::execution::acted("click", false, &result);
            let mut answer = answered(true);
            answer.stdout = format!("{}\n", serde_json::json!({ "ok": true, "result": result }));
            (answer, act)
        };
        let click = words(&[
            "click",
            "--app",
            "Notepad",
            "--element-index",
            "3",
            "--full-state",
            "--json",
        ]);
        let own = || -> Result<serde_json::Value, Option<FrameSkipped>> {
            Ok(serde_json::json!("the picture its answer carries"))
        };
        let framed = |(answer, acted): (zerocode_hookd::TeamAnswer, Option<Acted>)| {
            let frame = computer_frame(dir.path(), true, false, &click, &answer, acted);
            if matches!(frame, Frame::Own(_)) {
                own()
            } else {
                asked(frame)
            }
        };
        let seen = [
            framed(answered_on_windows(19, "printWindow")),
            framed(answered_on_windows(19, SCREEN_COPY_ENGINE)),
            framed(answered_on_windows(7, "printWindow")),
        ];
        assert_eq!(
            seen,
            [
                // Drawn by the window the act went to: its frame, at once.
                own(),
                // Copied off the screen: that window alone is asked for.
                Ok(serde_json::json!({ "windowOnly": 19, "windowOwner": 501 })),
                // A look of another window: the acted window alone.
                Ok(serde_json::json!({ "windowOnly": 7, "windowOwner": 501 })),
            ],
            "a Windows act's picture of its own window, drawn by it, frames its step"
        );
    }

    /// A Windows act's answer names its window only when the provider saw
    /// that window take the act's input (review t-129021 §3), so its look of
    /// that window frames its step only then. An element of the window's own
    /// tree worked, a press the click fence let through only over it, keys
    /// sent with the window itself in front: the look, drawn by the window,
    /// is the step's frame. A wheel or a drag sent at a point of the screen,
    /// which whatever window stands there takes; keys sent while another
    /// window of the app stood in front over it, which the focus check lets
    /// pass; text written into the focused element that only its process
    /// ties to the window: the look shows the window the act was aimed at,
    /// not one its input is known to have reached, so no picture is kept and
    /// the line says why. Every case is framed before the one assertion reads
    /// them all.
    #[test]
    fn a_windows_act_frames_its_window_only_when_the_window_was_seen_to_take_its_input() {
        use zerocode_core::computer_use_protocol::{
            ActionMetadata, ActionPath, ScreenshotMetadata, ScreenshotStatus,
        };
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("look.png");
        std::fs::write(&file, PNG).unwrap();
        // The provider's answer to `method`, aimed at window 19 of process
        // 501 (`current_snapshot`), whose road (`path`) saw window `took`
        // take its input — and whose look after it is of window 19, drawn
        // by it.
        let answered_on_windows = |method: &str, path: ActionPath, took: Option<u64>| {
            let mut action = ActionMetadata::new(path);
            action.target_window_id = 19;
            action.acted_window = ActedWindow { id: 19, pid: 501 }.if_it_took(took);
            let result = serde_json::json!({
                "snapshot": {
                    "app": { "name": "Notepad", "pid": 501 },
                    "window": { "id": 19, "x": 10, "y": 20, "width": 800, "height": 600 },
                },
                "screenshot": { "path": file, "scale": 1.0 },
                "screenshotStatus": ScreenshotStatus::Captured(ScreenshotMetadata {
                    engine: "printWindow".to_string(),
                    window_id: 19,
                }),
                "action": action,
            });
            let act =
                zerocode_core::computer_use_protocol::execution::acted(method, false, &result);
            let mut answer = answered(true);
            answer.stdout = format!("{}\n", serde_json::json!({ "ok": true, "result": result }));
            (answer, act)
        };
        // `step` is the words that ran; the frame goes by the act its answer
        // names.
        let framed = |step: &[&str], method: &str, path: ActionPath, took: Option<u64>| {
            let (answer, acted) = answered_on_windows(method, path, took);
            match computer_frame(dir.path(), true, false, &words(step), &answer, acted) {
                Frame::Own(_) => Ok(serde_json::json!("the picture its answer carries")),
                frame => asked(frame),
            }
        };
        let press_return = ["press-key", "--app", "Notepad", "--key", "Return"];
        let seen = [
            // The click fence let the press through over window 19.
            framed(
                &["click", "--app", "Notepad", "--x", "400", "--y", "300"],
                "click",
                ActionPath::Synthetic,
                Some(19),
            ),
            // Window 19 itself was in front when the keys went.
            framed(&press_return, "pressKey", ActionPath::Synthetic, Some(19)),
            // The wheel at a point of the screen: nobody looked who took it.
            framed(
                &[
                    "scroll",
                    "--app",
                    "Notepad",
                    "--x",
                    "400",
                    "--y",
                    "300",
                    "--direction",
                    "down",
                ],
                "scroll",
                ActionPath::Synthetic,
                None,
            ),
            // A drag between points of the screen: the same.
            framed(
                &[
                    "drag", "--app", "Notepad", "--from-x", "100", "--from-y", "100", "--to-x",
                    "400", "--to-y", "300",
                ],
                "drag",
                ActionPath::Synthetic,
                None,
            ),
            // Window 23 of the same app stood in front over window 19.
            framed(&press_return, "pressKey", ActionPath::Synthetic, Some(23)),
            // The focused element took the text through its Value pattern,
            // and named no window of its own: only its process, 501.
            framed(
                &["type-text", "--app", "Notepad", "--text", "a"],
                "typeText",
                ActionPath::Accessibility,
                None,
            ),
        ];
        let own = || -> Result<serde_json::Value, Option<FrameSkipped>> {
            Ok(serde_json::json!("the picture its answer carries"))
        };
        let unseen = || -> Result<serde_json::Value, Option<FrameSkipped>> {
            Err(Some(FrameSkipped::WindowUnavailable))
        };
        assert_eq!(
            seen,
            [own(), own(), unseen(), unseen(), unseen(), unseen()],
            "a Windows act's look frames its step only when its window was seen to take the input"
        );
    }

    /// A surface framed the way a desktop step's capture is keyed, whose
    /// frame is always the same picture.
    struct Pictured(Capture);

    impl Surface for Pictured {
        fn key(&self) -> String {
            self.0.key()
        }

        async fn take(self) -> Result<Picture, FrameSkipped> {
            Ok(Picture::unplaced(PNG.to_vec()))
        }
    }

    /// A frame of one window alone is its own surface (I-t123731-rc1, review
    /// t-128744 §6): an action in another window, or on the screen around
    /// it, is in no picture of that window, and a picture of either shows
    /// nothing of its step — so neither takes its frame away. A burst in one
    /// window is framed once, by its last step, as a burst on the screen is.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_frame_of_one_window_is_kept_past_acts_in_other_windows_or_on_the_screen() {
        let alone = |id: u64| {
            Frame::After(Pictured(Capture::Desktop {
                params: serde_json::json!({ "windowOnly": id, "windowOwner": 501 }),
            }))
        };
        let screen = || {
            Frame::After(Pictured(Capture::Desktop {
                params: serde_json::json!({}),
            }))
        };
        let dir = tempfile::tempdir().expect("tempdir");
        let (writer, messages) = unbounded_channel();
        for (verb, frame) in [
            ("click", alone(7)),
            ("type-text", alone(19)),
            ("press-key", alone(7)),
            ("mouse-click", screen()),
            ("click", alone(7)),
        ] {
            writer.send(step(dir.path(), verb, frame)).unwrap();
        }
        drop(writer);
        write_in_order(messages, Duration::from_millis(50)).await;
        let shot: Vec<bool> = run_evidence::steps_in(dir.path())
            .iter()
            .map(|step| step.shot.is_some())
            .collect();
        assert_eq!(
            shot,
            [false, true, false, true, true],
            "window 7's burst is framed by its last step; window 19 and the screen keep their own"
        );
    }

    /// A window asked for alone is kept only when the helper's answer says
    /// it pictured that window alone: an answer that does not say so — a
    /// helper that does not know the request pictures the screen instead —
    /// or that names another window keeps nothing (I-t123731-rc1). A picture
    /// of the screen asked for as such is kept as before.
    #[test]
    fn a_window_asked_for_alone_is_kept_only_when_the_helper_says_it_pictured_it_alone() {
        use base64::Engine as _;
        let data = base64::engine::general_purpose::STANDARD.encode(PNG);
        let helper_said = |alone: Option<u64>| {
            let mut said = serde_json::json!({
                "screenshot": { "data": data, "width": 1, "height": 1, "scale": 2.0 },
                "origin": { "x": 10.0, "y": 20.0 },
            });
            if let Some(id) = alone {
                said[WINDOW_ONLY] = id.into();
            }
            said
        };
        let seven = serde_json::json!(7);
        let kept = desktop_picture(Some(&seven), &helper_said(Some(7))).expect("the window alone");
        assert_eq!(kept.png, PNG);
        assert_eq!(
            kept.placed,
            ShotFrame::new((10.0, 20.0), 2.0).unwrap(),
            "placed where the window stands"
        );
        assert!(
            desktop_picture(Some(&seven), &helper_said(None)).is_none(),
            "an answer that does not say so is of the screen: not kept"
        );
        assert!(
            desktop_picture(Some(&seven), &helper_said(Some(8))).is_none(),
            "a picture of another window: not kept"
        );
        assert!(
            desktop_picture(None, &helper_said(None)).is_some(),
            "the screen, asked for as such, is kept as before"
        );
    }

    /// The walk's own record is the writer's to place: it lands after the
    /// lines it reports, when its turn in the queue comes — never before —
    /// and each walk of a folder takes the next number. A step a later step
    /// on its screen superseded says so on its line.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_walk_report_is_written_after_its_steps_by_the_one_writer() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (writer, messages) = unbounded_channel();
        let written = tokio::spawn(write_in_order(messages, Duration::ZERO));
        let told = |writer: &UnboundedSender<Message<Fixed>>| {
            let (done, told) = tokio::sync::oneshot::channel();
            writer.send(Message::Written(done)).unwrap();
            told
        };
        writer
            .send(step(dir.path(), "click", fixed("desktop")))
            .unwrap();
        writer
            .send(step(dir.path(), "key", fixed("desktop")))
            .unwrap();
        told(&writer).await.expect("the reader is told");
        assert_eq!(run_evidence::steps_in(dir.path()).len(), 2);
        assert!(
            run_evidence::walks_in(dir.path()).is_empty(),
            "no walk has been handed over yet"
        );
        writer
            .send(Message::Walk {
                dir: dir.path().to_path_buf(),
                report: serde_json::json!({ "kind": "batch", "start": 1, "steps": 2 }),
            })
            .unwrap();
        told(&writer).await.expect("the reader is told");
        let walks = run_evidence::walks_in(dir.path());
        assert_eq!(walks.len(), 1);
        assert_eq!(walks[0].0, "walk-001.json");
        assert_eq!(walks[0].1["steps"], 2);
        let steps = run_evidence::steps_in(dir.path());
        assert_eq!(
            steps[0].frame_skipped,
            Some(run_evidence::FrameSkipped::Superseded),
            "the click's frame would have shown the key: its line says so"
        );
        assert!(steps[0].shot.is_none() && steps[1].shot.is_some());
        assert!(
            steps[1].frame.is_some() == steps[1].shot.is_some() || steps[1].frame.is_none(),
            "a fake PNG has no readable size; a real one keeps its placement"
        );
        writer
            .send(step(dir.path(), "type", fixed("desktop")))
            .unwrap();
        writer
            .send(Message::Walk {
                dir: dir.path().to_path_buf(),
                report: serde_json::json!({ "kind": "recipe-run", "start": 3, "steps": 1 }),
            })
            .unwrap();
        told(&writer).await.expect("the reader is told");
        let walks = run_evidence::walks_in(dir.path());
        assert_eq!(walks.len(), 2);
        assert_eq!(
            (walks[1].0.as_str(), &walks[1].1["start"]),
            ("walk-002.json", &serde_json::json!(3))
        );
        assert_eq!(run_evidence::steps_in(dir.path()).len(), 3);
        drop(writer);
        written.await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn flow_retry_identity_follows_the_existing_queue_and_sealed_report() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(run_evidence::STEPS_FILE), b"{\xff").unwrap();
        let (writer, messages) = unbounded_channel::<Message<Fixed>>();
        for (id, origin) in [
            (Some("first"), Some(1)),
            (None, None),
            (Some("second"), Some(3)),
        ] {
            let observation = id.map(|id| {
                serde_json::json!({"evidence_id": id,
                "retry": {"name": "recipe", "step": origin, "cwd": "/original"}})
            });
            writer
                .send(Message::Step(Job::new(
                    dir.path(),
                    "computer",
                    &["key", "--key", "tab"].map(String::from),
                    &answered(true),
                    Frame::None,
                    observation,
                )))
                .unwrap();
        }
        writer.send(Message::Walk { dir: dir.path().to_path_buf(), report: serde_json::json!({
            "kind": "recipe-run", "name": "recipe", "evidence_ids": true, "ran": [
                {"step": 1, "evidence_id": "first"}, {"step": 2, "evidence_id": "unwritten"},
                {"step": 3, "evidence_id": "second"},
            ]
        })}).unwrap();
        drop(writer);
        write_in_order(messages, Duration::ZERO).await;
        let walk = &run_evidence::walks_in(dir.path())[0].1;
        assert_eq!(walk["ran"][0]["evidence_n"], 2);
        assert!(walk["ran"][1].get("evidence_n").is_none());
        assert_eq!(walk["ran"][2]["evidence_n"], 4);
        computer_use::report::write(dir.path()).unwrap().unwrap();
        assert!(computer_use::report::verify(dir.path()).reproduced);
    }

    /// A walk whose Flow says `evidence: off` (or `verdict-only`) frames
    /// nothing in its folder — every line says `off` — while the walk's
    /// record and the verdict are written as always; a lone command after
    /// the walk is framed as before, and no report is written for `off`.
    #[tokio::test(flavor = "multi_thread")]
    async fn an_evidence_level_of_off_keeps_the_verdict_and_the_walk_json() {
        use zerocode_core::computer_flow::EvidenceLevel;
        let dir = tempfile::tempdir().expect("tempdir");
        let (writer, messages) = unbounded_channel();
        writer
            .send(Message::WalkBegin {
                dir: dir.path().to_path_buf(),
                framed: EvidenceLevel::Off.frames(),
                identity: serde_json::Value::Null,
            })
            .unwrap();
        for (verb, frame) in [
            ("click", fixed("desktop")),
            ("key", fixed("desktop")),
            ("screenshot", Frame::Own(Picture::unplaced(PNG.to_vec()))),
        ] {
            writer.send(step(dir.path(), verb, frame)).unwrap();
        }
        writer
            .send(Message::Walk {
                dir: dir.path().to_path_buf(),
                report: serde_json::json!({
                    "kind": "recipe-run", "start": 1, "steps": 3,
                    "flow": { "policy": "dry", "evidence": "off" },
                }),
            })
            .unwrap();
        writer
            .send(step(dir.path(), "click", fixed("desktop")))
            .unwrap();
        drop(writer);
        write_in_order(messages, Duration::ZERO).await;
        let steps = run_evidence::steps_in(dir.path());
        assert_eq!(steps.len(), 4, "every line is written");
        for step in &steps[..3] {
            assert!(step.shot.is_none(), "{}: no frame at this level", step.verb);
            assert_eq!(
                step.frame_skipped,
                Some(run_evidence::FrameSkipped::Off),
                "{}: the line says why",
                step.verb
            );
        }
        assert!(
            steps[3].shot.is_some() && steps[3].frame_skipped.is_none(),
            "a lone command after the walk is framed as before"
        );
        let frames = std::fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "png"))
            .count();
        assert_eq!(frames, 1, "only the lone command's frame is on disk");
        assert_eq!(
            run_evidence::walks_in(dir.path()).len(),
            1,
            "the walk's record is kept"
        );
        computer_use::evidence::write_verdict(
            dir.path(),
            &["verdict".to_string(), "--pass".to_string()],
            true,
            None,
            5_000,
        )
        .expect("the verdict is written whatever the level");
        assert_eq!(
            computer_use::evidence::read_verdict(dir.path()),
            Some((true, None))
        );
        assert_eq!(
            computer_use::report::write(dir.path()),
            Ok(None),
            "no report for `off`"
        );
        assert!(!dir.path().join(computer_use::report::REPORT_FILE).exists());
        assert!(!EvidenceLevel::VerdictOnly.frames() && EvidenceLevel::Full.frames());
    }
}
