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
//! framing it, and frames only the last of a burst on one screen: a frame is
//! taken when the writer gets to it, so a step that a later action on the
//! same screen followed before then — anywhere in the queue, past waits and
//! looks — would be framed showing that later action, and is shown by that
//! later frame instead. A step that answered a picture — a look, or an app
//! act in a walk that frames its steps, whose answer carries the helper's
//! look after the act — is its own frame, kept at once (t-4229).
//!
//! Three rules keep the writer honest. A verb that reads or writes the step
//! log (`verdict`, `evidence`, `recipe-save`) waits for it to be written
//! first ([`written`]). A desktop frame is a picture of the screen — the
//! acted app's window region, or the whole display — never a `getAppState`,
//! which would replace the element indices the agent was just given. And a
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
use zerocode_core::computer_use_protocol::frame::ShotFrame;

use run_evidence::{FrameSkipped, Framing};

/// Past this many steps waiting behind the one being framed, the writer
/// writes lines without frames until it has caught up.
const BACKLOG_FRAMES: usize = 8;
/// The longest one frame may take before its step is written without it.
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(5);
/// The longest a log reader waits for the writer to catch up.
const WRITTEN_TIMEOUT: Duration = Duration::from_secs(10);

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
    fn take(self) -> impl std::future::Future<Output = Option<Picture>> + Send;
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
    /// A picture of the screen through the helper's `screenshotDesktop`: the
    /// whole display, or the acted window's region of it.
    Desktop {
        params: serde_json::Value,
    },
    Emulator {
        platform: zerocode_core::computer_use::EmulatorPlatform,
        device: String,
    },
    Browser {
        app: AppHandle,
        label: String,
    },
}

impl Surface for Capture {
    fn key(&self) -> String {
        match self {
            // The acted window's region and the whole display are one
            // screen: an action in one is in a picture of the other.
            Self::Desktop { .. } => "computer".to_string(),
            Self::Emulator { platform, device } => format!("emulator:{platform:?}:{device}"),
            Self::Browser { label, .. } => format!("browser:{label}"),
        }
    }

    async fn take(self) -> Option<Picture> {
        match self {
            Self::Desktop { params } => tauri::async_runtime::spawn_blocking(move || {
                let answer = computer_use::call("screenshotDesktop", params).ok()?;
                Some(Picture {
                    png: computer_use::screenshot_png(&answer)?,
                    placed: ShotFrame::from_answer(&answer).unwrap_or(ShotFrame::UNIT),
                })
            })
            .await
            .ok()
            .flatten(),
            Self::Emulator { platform, device } => emulator_screenshot_bytes(platform, &device)
                .await
                .ok()
                .map(Picture::unplaced),
            Self::Browser { app, label } => {
                let state = app.state::<AppState>();
                cmd::fs::browser_snapshot_png(&app, &state, &label)
                    .await
                    .ok()
                    .map(Picture::unplaced)
            }
        }
    }
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
                        .ok()
                        .flatten()
                        .ok_or(Some(FrameSkipped::CaptureFailed))
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
    let png = std::fs::read(result.pointer("/screenshot/path")?.as_str()?).ok()?;
    Some(Picture {
        png,
        placed: ShotFrame::from_answer(result).unwrap_or(ShotFrame::UNIT),
    })
}

/// Where a desktop step is framed: the region of the window an app action
/// answered in its result (the screen as a person would see it there), or
/// the whole display. Never `getAppState`: that would replace the element
/// indices the agent was just handed, from a writer running behind its
/// answers.
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
) {
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
/// exported is the step's own — kept at once, whatever follows it; else the
/// surface `after` names from the answer's result is framed once it has
/// settled. The envelope is read once, for both.
fn frame_of<S>(
    dir: &Path,
    frames: bool,
    capped: bool,
    answer: &zerocode_hookd::TeamAnswer,
    after: impl FnOnce(Option<&serde_json::Value>) -> Option<S>,
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
    match own_frame(result) {
        Some(picture) => Frame::Own(picture),
        None => after(result).map_or(Frame::None, Frame::After),
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
    let frame = frame_of(dir, frames, false, answer, |_| emulator_capture(argv));
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
/// it acted on (`frame_of`) — a picture the answer exported is the step's own
/// frame; else the acted window's region, or the whole screen after a
/// desktop action. An `ssh` command is not a surface, and the emulator has
/// its own road (`leave_emulator_evidence`).
pub(super) fn leave_computer_evidence(
    dir: &Path,
    argv: &[String],
    answer: &zerocode_hookd::TeamAnswer,
    capped: bool,
    observation: Option<serde_json::Value>,
) {
    let verb = match argv.first().map(String::as_str) {
        Some("ssh") | Some("emulator") | None => return,
        Some(verb) => verb,
    };
    let Some(frames) = run_evidence::captures("computer", verb) else {
        return;
    };
    let frame = frame_of(dir, frames, capped, answer, |result| {
        Some(desktop_capture(result))
    });
    send(Message::Step(Job::new(
        dir,
        "computer",
        argv,
        answer,
        frame,
        observation,
    )));
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

        async fn take(self) -> Option<Picture> {
            Some(Picture::unplaced(PNG.to_vec()))
        }
    }

    fn step(dir: &Path, verb: &str, frame: Frame<Fixed>) -> Message<Fixed> {
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
    #[tokio::test(flavor = "multi_thread")]
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
        assert!(
            started.elapsed() < settle * 2,
            "the settle is counted from the step, not added per framed step: {:?}",
            started.elapsed()
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
    /// look after an app act, placed at the window it shows. Twenty acts at
    /// a replay's pace all keep their frame, where the same twenty framed
    /// after a settle keep one (the desk's 1 of 20); a walk that frames
    /// nothing keeps none, as before. No picture: framed after the settle.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_step_that_answered_its_own_picture_is_framed_by_it_not_by_a_later_capture() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("shot.png");
        std::fs::write(&file, PNG).unwrap();
        let window = serde_json::json!({ "window": { "id": 7, "x": 10, "y": 20, "width": 300, "height": 200 } });
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
            frame_of(dir.path(), frames, false, answer, |_| {
                Some(Fixed("computer"))
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
                let frame = frame_of(folder.path(), true, false, answer, |_| {
                    Some(Fixed("computer"))
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
