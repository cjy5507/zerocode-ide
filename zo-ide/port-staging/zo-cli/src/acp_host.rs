//! Zo runtime adapter for the ACP state machine.

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use acp::{
    AcpServer, BoxFuture, PermissionRequester, RuntimeError, RuntimeFactory, RuntimeSession,
    TurnCancellation,
};
use runtime::permission::{
    PermissionDecision, PermissionError, PermissionPrompter, PermissionRequest,
};
use runtime::{HookAbortSignal, PermissionMode};

use crate::cli_args::AllowedToolSet;
use crate::session::{LiveCli, StreamPrompter};
use crate::session_registry::SessionScope;

pub(crate) fn run_acp(
    model: String,
    allowed_tools: Option<AllowedToolSet>,
    permission_mode: PermissionMode,
) -> Result<(), Box<dyn std::error::Error>> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .max_blocking_threads(512)
        .thread_name("zo-acp")
        .enable_all()
        .build()?;
    let factory = Arc::new(ZoRuntimeFactory {
        model,
        allowed_tools,
        permission_mode,
    });
    runtime.block_on(AcpServer::new(factory).serve_stdio())?;
    Ok(())
}

struct ZoRuntimeFactory {
    model: String,
    allowed_tools: Option<AllowedToolSet>,
    permission_mode: PermissionMode,
}

impl RuntimeFactory for ZoRuntimeFactory {
    fn create_session(
        &self,
        cwd: PathBuf,
    ) -> BoxFuture<'_, Result<Arc<dyn RuntimeSession>, RuntimeError>> {
        let model = self.model.clone();
        let allowed_tools = self.allowed_tools.clone();
        let permission_mode = self.permission_mode;
        Box::pin(async move {
            let built = tokio::task::spawn_blocking(move || {
                LiveCli::new_scoped_at(
                    model,
                    true,
                    allowed_tools,
                    permission_mode,
                    SessionScope::Project,
                    cwd,
                )
                .map_err(|error| error.to_string())
            })
            .await
            .map_err(|error| RuntimeError::new(format!("session construction panicked: {error}")))?
            .map_err(RuntimeError::new)?;
            let id = built.session.id.clone();
            let runtime: Arc<dyn RuntimeSession> = Arc::new(ZoRuntimeSession {
                id,
                permission_mode,
                cli: tokio::sync::Mutex::new(built),
            });
            Ok(runtime)
        })
    }
}

struct ZoRuntimeSession {
    id: String,
    permission_mode: PermissionMode,
    cli: tokio::sync::Mutex<LiveCli>,
}

impl RuntimeSession for ZoRuntimeSession {
    fn id(&self) -> &str {
        &self.id
    }

    fn permission_mode(&self) -> PermissionMode {
        self.permission_mode
    }

    fn run_turn(
        &self,
        prompt: String,
        events: tokio::sync::mpsc::Sender<runtime::message_stream::RenderBlock>,
        permissions: Arc<dyn PermissionRequester>,
        cancellation: TurnCancellation,
    ) -> BoxFuture<'_, Result<(), RuntimeError>> {
        Box::pin(async move {
            let mut cli = self.cli.lock().await;
            let turn_abort = HookAbortSignal::new();
            let user_cancel_requested = Arc::new(AtomicBool::new(false));
            let prompter = StreamPrompter::External(Arc::new(AcpPermissionPrompter {
                requester: permissions,
            }));
            let turn = cli.run_turn_streaming_to_channel_with_prompter(
                &prompt,
                events,
                prompter,
                turn_abort.clone(),
                Arc::clone(&user_cancel_requested),
            );
            tokio::pin!(turn);
            let result = tokio::select! {
                result = &mut turn => result,
                () = cancellation.cancelled() => {
                    user_cancel_requested.store(true, Ordering::SeqCst);
                    turn_abort.abort();
                    turn.await
                }
            };
            result.map(|_| ()).map_err(RuntimeError::new)
        })
    }
}

struct AcpPermissionPrompter {
    requester: Arc<dyn PermissionRequester>,
}

impl PermissionPrompter for AcpPermissionPrompter {
    fn decide<'a>(
        &'a self,
        request: PermissionRequest,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<PermissionDecision, PermissionError>> + Send + 'a,
        >,
    > {
        self.requester.request(request)
    }
}
