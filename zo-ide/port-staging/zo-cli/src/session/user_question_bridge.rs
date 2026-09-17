//! Bridge from the synchronous `AskUserQuestion` tool to the TUI modal surface.

use std::sync::Arc;

use runtime::message_stream::{BlockIdGen, RenderBlock, UserQuestionPrompt};
use tokio::sync::mpsc;
use tools::{GlobalToolRegistry, ToolError, UserQuestionChannel};

/// Synchronous question channel used by `AskUserQuestion` during TUI turns.
pub(crate) struct TuiUserQuestionChannel {
    render_tx: mpsc::Sender<RenderBlock>,
    ids: BlockIdGen,
}

impl TuiUserQuestionChannel {
    /// Create a channel that emits user-question prompts into the TUI render stream.
    #[must_use]
    pub(crate) fn new(render_tx: mpsc::Sender<RenderBlock>, ids: BlockIdGen) -> Self {
        Self { render_tx, ids }
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
    use super::TuiUserQuestionChannel;
    use runtime::message_stream::{BlockIdGen, QuestionOption, RenderBlock};
    use tokio::sync::mpsc;
    use tools::UserQuestionChannel;

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
