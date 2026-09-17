//! Bridge from the synchronous `AskUserQuestion` tool to the TUI modal
//! surface — and, since t-2943, from `PushNotification` to whichever surface
//! can reach the person.

use std::io::IsTerminal as _;
use std::sync::Arc;

use runtime::message_stream::{BlockIdGen, RenderBlock, SystemLevel, UserQuestionPrompt};
use tokio::sync::mpsc;
use tools::{
    push_road, GlobalToolRegistry, KeyboardPresence, PushLimits, PushNotice, PushRoad,
    PushSurface, ToolError, UserQuestionChannel,
};

use crate::ide::events::{self, EventsChannel};

/// Where this process can reach a person, asked at push time because both
/// facts move: a window subscribes after the pane is born, and a test states
/// a surface the process does not have.
pub(crate) struct SurfaceProbe {
    /// Somebody is subscribed to this session's events channel — a window.
    pub(crate) window: Box<dyn Fn() -> bool + Send + Sync>,
    /// Stdout is a terminal that can carry a bell.
    pub(crate) terminal: Box<dyn Fn() -> bool + Send + Sync>,
}

impl SurfaceProbe {
    /// The process's real surfaces.
    fn live() -> Self {
        Self {
            window: Box::new(|| events::channel().is_some_and(EventsChannel::has_subscribers)),
            terminal: Box::new(|| std::io::stdout().is_terminal()),
        }
    }
}

/// Synchronous question channel used by `AskUserQuestion` during TUI turns,
/// and the surface `PushNotification` judges and delivers through.
pub(crate) struct TuiUserQuestionChannel {
    render_tx: mpsc::Sender<RenderBlock>,
    ids: BlockIdGen,
    /// When a key was last pressed at this keyboard — the attended fact.
    presence: KeyboardPresence,
    surface: SurfaceProbe,
    limits: PushLimits,
}

impl TuiUserQuestionChannel {
    /// Create a channel that emits user-question prompts into the TUI render
    /// stream, reading the process's own keyboard and surfaces.
    #[must_use]
    pub(crate) fn new(render_tx: mpsc::Sender<RenderBlock>, ids: BlockIdGen) -> Self {
        Self {
            render_tx,
            ids,
            presence: KeyboardPresence::process().clone(),
            surface: SurfaceProbe::live(),
            limits: PushLimits::from_env(),
        }
    }

    /// State the keyboard, the surfaces and the limits — a test's seam.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn with_surface(
        mut self,
        presence: KeyboardPresence,
        surface: SurfaceProbe,
        limits: PushLimits,
    ) -> Self {
        self.presence = presence;
        self.surface = surface;
        self.limits = limits;
        self
    }
}

impl UserQuestionChannel for TuiUserQuestionChannel {
    fn ask(
        &self,
        question: &str,
        header: Option<&str>,
        options: &[runtime::message_stream::QuestionOption],
        multi_select: bool,
    ) -> Result<Vec<String>, ToolError> {
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            if matches!(
                handle.runtime_flavor(),
                tokio::runtime::RuntimeFlavor::CurrentThread
            ) && crate::tui_active()
            {
                return Err(ToolError::Execution(
                    "AskUserQuestion cannot block the single-threaded TUI runtime".to_string(),
                ));
            }
        }

        let (responder, response) = tokio::sync::oneshot::channel();
        let prompt = UserQuestionPrompt {
            id: self.ids.next(),
            question: question.to_string(),
            header: header.map(str::to_string),
            options: options.to_vec(),
            multi_select,
            responder,
        };

        self.render_tx
            .blocking_send(RenderBlock::UserQuestionPrompt(prompt))
            .map_err(|_| ToolError::Execution("TUI question channel is closed".to_string()))?;

        response.blocking_recv().map_err(|_| {
            ToolError::Execution("TUI question prompt closed without an answer".to_string())
        })
    }

    fn send_to_user(&self, message: &str) -> Result<(), ToolError> {
        // Fire-and-forget: a `UserNotice` block carries no responder, so unlike
        // `ask` there is nothing to wait on. Tools run in `spawn_blocking`, so
        // `blocking_send` cannot stall the render loop.
        self.render_tx
            .blocking_send(RenderBlock::UserNotice {
                id: self.ids.next(),
                message: message.to_string(),
            })
            .map_err(|_| ToolError::Execution("TUI user channel is closed".to_string()))
    }

    /// Judge the road from the facts only this surface holds, then deliver:
    /// a `Notification` block goes down the render stream — the TUI draws one
    /// line and rings the tty on the terminal road, and the turn loop's
    /// publish turns it into the window's `notify` frame. A skip sends
    /// nothing; the road word is the whole receipt.
    fn push_notification(&self, notice: &PushNotice) -> Result<PushRoad, ToolError> {
        let road = push_road(PushSurface {
            attended: self.presence.attended_within(self.limits.attended_window),
            window: (self.surface.window)(),
            terminal: (self.surface.terminal)(),
        });
        if road.delivered() {
            self.render_tx
                .blocking_send(RenderBlock::Notification {
                    id: self.ids.next(),
                    title: notice.title.clone(),
                    body: notice.body.clone(),
                    level: SystemLevel::Info,
                    road,
                })
                .map_err(|_| ToolError::Execution("TUI user channel is closed".to_string()))?;
        }
        Ok(road)
    }
}

/// Attach the TUI user-question channel to a tool registry.
pub(crate) fn install_tui_user_question_channel(
    registry: &mut GlobalToolRegistry,
    render_tx: mpsc::Sender<RenderBlock>,
    ids: BlockIdGen,
) {
    let channel: Arc<dyn UserQuestionChannel> =
        Arc::new(TuiUserQuestionChannel::new(render_tx, ids));
    // Writes through the context's shared channel cell, so the boot-time
    // registry clones (concurrent-dispatch closure, API client) see the
    // install too — not just this executor's clone.
    registry.context_mut().set_user_question_channel(Some(channel));
}

#[cfg(test)]
mod tests {
    use super::{SurfaceProbe, TuiUserQuestionChannel};
    use runtime::message_stream::{BlockIdGen, QuestionOption, RenderBlock};
    use tokio::sync::mpsc;
    use tools::{KeyboardPresence, PushLimits, PushNotice, PushRoad, UserQuestionChannel};

    #[test]
    fn channel_sends_render_prompt_and_waits_for_modal_answer() {
        let (render_tx, mut render_rx) = mpsc::channel::<RenderBlock>(1);
        let channel = TuiUserQuestionChannel::new(render_tx, BlockIdGen::default());

        let handle = std::thread::spawn(move || {
            channel
                .ask(
                    "Pick one",
                    Some("Choice"),
                    &[
                        QuestionOption {
                            label: "alpha".to_string(),
                            description: Some("first option".to_string()),
                            preview: None,
                        },
                        QuestionOption::plain("beta"),
                    ],
                    false,
                )
                .expect("question answered")
        });

        let block = render_rx.blocking_recv().expect("prompt arrives");
        match block {
            RenderBlock::UserQuestionPrompt(prompt) => {
                assert_eq!(prompt.question, "Pick one");
                assert_eq!(prompt.header.as_deref(), Some("Choice"));
                assert_eq!(prompt.options[0].label, "alpha");
                assert_eq!(
                    prompt.options[0].description.as_deref(),
                    Some("first option")
                );
                assert_eq!(prompt.options[1], QuestionOption::plain("beta"));
                assert!(!prompt.multi_select, "single-select prompt by default");
                prompt
                    .responder
                    .send(vec!["beta".to_string()])
                    .expect("responder live");
            }
            other => panic!("unexpected render block: {other:?}"),
        }

        assert_eq!(handle.join().expect("thread join"), vec!["beta".to_string()]);
    }

    #[test]
    fn channel_round_trips_freeform_text_for_an_option_question() {
        let (render_tx, mut render_rx) = mpsc::channel::<RenderBlock>(1);
        let channel = TuiUserQuestionChannel::new(render_tx, BlockIdGen::default());

        let handle = std::thread::spawn(move || {
            channel
                .ask(
                    "Pick one",
                    None,
                    &[QuestionOption::plain("alpha"), QuestionOption::plain("beta")],
                    false,
                )
                .expect("question answered")
        });

        match render_rx.blocking_recv().expect("prompt arrives") {
            RenderBlock::UserQuestionPrompt(prompt) => prompt
                .responder
                .send(vec!["something else".to_string()])
                .expect("responder live"),
            other => panic!("unexpected render block: {other:?}"),
        }

        assert_eq!(
            handle.join().expect("thread join"),
            vec!["something else".to_string()]
        );
    }

    #[test]
    fn send_to_user_emits_a_user_notice_block() {
        let (render_tx, mut render_rx) = mpsc::channel::<RenderBlock>(1);
        let channel = TuiUserQuestionChannel::new(render_tx, BlockIdGen::default());

        channel
            .send_to_user("verbatim finding")
            .expect("push succeeds");

        match render_rx.blocking_recv().expect("notice arrives") {
            RenderBlock::UserNotice { message, .. } => {
                assert_eq!(message, "verbatim finding");
            }
            other => panic!("unexpected render block: {other:?}"),
        }
    }

    /* ---- PushNotification (t-2943): the surface judges, then delivers ---- */

    fn probe(window: bool, terminal: bool) -> SurfaceProbe {
        SurfaceProbe {
            window: Box::new(move || window),
            terminal: Box::new(move || terminal),
        }
    }

    fn push_channel(
        render_tx: mpsc::Sender<RenderBlock>,
        presence: &KeyboardPresence,
        window: bool,
        terminal: bool,
    ) -> TuiUserQuestionChannel {
        TuiUserQuestionChannel::new(render_tx, BlockIdGen::default()).with_surface(
            presence.clone(),
            probe(window, terminal),
            PushLimits::default(),
        )
    }

    fn notice() -> PushNotice {
        PushNotice {
            title: "zo · api".to_string(),
            body: "Build is green; merge when you are back".to_string(),
        }
    }

    #[test]
    fn a_push_with_nobody_typing_and_no_window_rings_the_terminal_and_says_so() {
        let (render_tx, mut render_rx) = mpsc::channel::<RenderBlock>(1);
        let channel = push_channel(render_tx, &KeyboardPresence::default(), false, true);

        let road = channel.push_notification(&notice()).expect("push judged");
        assert_eq!(road, PushRoad::Terminal);
        match render_rx.blocking_recv().expect("notification block arrives") {
            RenderBlock::Notification {
                title, body, road, ..
            } => {
                assert_eq!(title, "zo · api");
                assert_eq!(body, "Build is green; merge when you are back");
                assert_eq!(road, PushRoad::Terminal);
            }
            other => panic!("unexpected render block: {other:?}"),
        }
    }

    #[test]
    fn a_subscribed_window_takes_the_push_before_the_terminal() {
        let (render_tx, mut render_rx) = mpsc::channel::<RenderBlock>(1);
        let channel = push_channel(render_tx, &KeyboardPresence::default(), true, true);

        assert_eq!(
            channel.push_notification(&notice()).expect("push judged"),
            PushRoad::Window
        );
        match render_rx.blocking_recv().expect("notification block arrives") {
            RenderBlock::Notification { road, .. } => assert_eq!(road, PushRoad::Window),
            other => panic!("unexpected render block: {other:?}"),
        }
    }

    #[test]
    fn a_push_while_the_person_is_typing_is_skipped_and_sends_no_block() {
        let (render_tx, mut render_rx) = mpsc::channel::<RenderBlock>(1);
        let presence = KeyboardPresence::default();
        presence.note_input();
        let channel = push_channel(render_tx, &presence, true, true);

        assert_eq!(
            channel.push_notification(&notice()).expect("push judged"),
            PushRoad::SkippedAttended
        );
        assert!(
            matches!(render_rx.try_recv(), Err(mpsc::error::TryRecvError::Empty)),
            "a skipped push leaves nothing on the screen"
        );
    }

    #[test]
    fn a_push_with_no_surface_at_all_is_skipped_nowhere() {
        let (render_tx, _render_rx) = mpsc::channel::<RenderBlock>(1);
        let channel = push_channel(render_tx, &KeyboardPresence::default(), false, false);
        assert_eq!(
            channel.push_notification(&notice()).expect("push judged"),
            PushRoad::SkippedNowhere
        );
    }

    #[test]
    fn a_push_errors_when_the_render_stream_is_closed() {
        let (render_tx, render_rx) = mpsc::channel::<RenderBlock>(1);
        drop(render_rx);
        let channel = push_channel(render_tx, &KeyboardPresence::default(), false, true);
        assert!(channel.push_notification(&notice()).is_err());
    }

    #[test]
    fn send_to_user_errors_when_channel_closed() {
        let (render_tx, render_rx) = mpsc::channel::<RenderBlock>(1);
        drop(render_rx);
        let channel = TuiUserQuestionChannel::new(render_tx, BlockIdGen::default());

        // A closed render stream must surface as an error so the runner can
        // fall back to an inline echo instead of dropping the content.
        assert!(channel.send_to_user("lost?").is_err());
    }
}
