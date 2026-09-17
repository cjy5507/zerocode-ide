//! A provider that lives on its own thread, spoken to over a channel.
//!
//! The Windows provider is COM: its objects belong to the thread that made
//! them, some of its calls block on the target app, and a hung target must
//! not hang the window. So the provider runs on one dedicated thread and the
//! session talks to it through a request channel with a deadline. A request
//! that outlives its deadline is a transport failure — the session is
//! dropped, this thread is left to finish its call and exit on its own, and
//! the next request stands a fresh one. That is the in-process spelling of
//! "the helper stopped answering, launch another".
//!
//! Nothing here is Windows-specific: the handler is a trait, so the thread's
//! lifecycle — start, answer, time out, crash, end — is tested on every
//! platform with a fake handler.

use std::sync::mpsc;
use std::time::Duration;

use serde_json::Value;
use zerocode_core::computer_use_protocol::error_code;

use super::ComputerUseError;
use super::session::SessionFailure;

/// How long a provider may take to stand up before the session gives up.
const START_TIMEOUT: Duration = Duration::from_secs(10);

/// What the thread runs.
pub(super) trait Handler: 'static {
    fn handle(&mut self, method: &str, params: Value) -> Result<Value, ComputerUseError>;
}

enum Answer {
    Provider(Result<Value, ComputerUseError>),
    /// The handler panicked; the thread is ending.
    Crashed(String),
}

struct Request {
    method: String,
    params: Value,
    reply: mpsc::Sender<Answer>,
}

/// One provider thread. Dropping it closes the channel; the thread returns
/// from its next `recv` and drops the handler on its own thread — which is
/// the only thread a COM object may be released on.
pub(super) struct ProviderThread {
    requests: mpsc::Sender<Request>,
}

impl ProviderThread {
    /// Stand the thread up and wait for its handler to be ready. `factory`
    /// runs ON the new thread, so thread-affine initialisation (COM) happens
    /// where the handler will live.
    pub(super) fn spawn<H: Handler>(
        name: &'static str,
        factory: impl FnOnce() -> Result<H, ComputerUseError> + Send + 'static,
    ) -> Result<Self, ComputerUseError> {
        let (requests, inbox) = mpsc::channel::<Request>();
        let (ready, readiness) = mpsc::channel::<Result<(), ComputerUseError>>();
        std::thread::Builder::new()
            .name(name.to_string())
            .spawn(move || {
                let mut handler = match factory() {
                    Ok(handler) => {
                        let _ = ready.send(Ok(()));
                        handler
                    }
                    Err(error) => {
                        let _ = ready.send(Err(error));
                        return;
                    }
                };
                while let Ok(request) = inbox.recv() {
                    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        handler.handle(&request.method, request.params)
                    }));
                    match outcome {
                        Ok(answer) => {
                            let _ = request.reply.send(Answer::Provider(answer));
                        }
                        Err(panic) => {
                            let why = crate::system_runtime::panic_payload(panic.as_ref());
                            let _ = request.reply.send(Answer::Crashed(why));
                            break;
                        }
                    }
                }
                drop(handler);
            })
            .map_err(|error| {
                ComputerUseError::new(
                    error_code::ACCESSIBILITY_ERROR,
                    format!("could not start the Computer Use provider thread: {error}"),
                )
            })?;
        match readiness.recv_timeout(START_TIMEOUT) {
            Ok(Ok(())) => Ok(Self { requests }),
            Ok(Err(error)) => Err(error),
            Err(_) => Err(ComputerUseError::new(
                error_code::ACTION_TIMEOUT,
                "timed out starting the Computer Use provider",
            )),
        }
    }

    /// One request, answered within `deadline` or not at all.
    pub(super) fn request(
        &self,
        method: &str,
        params: Value,
        deadline: Duration,
    ) -> Result<Value, SessionFailure> {
        let (reply, answer) = mpsc::channel();
        self.requests
            .send(Request {
                method: method.to_string(),
                params,
                reply,
            })
            .map_err(|_| {
                SessionFailure::Transport(ComputerUseError::new(
                    error_code::ACCESSIBILITY_ERROR,
                    "the Computer Use provider thread has ended",
                ))
            })?;
        match answer.recv_timeout(deadline) {
            Ok(Answer::Provider(Ok(value))) => Ok(value),
            Ok(Answer::Provider(Err(error))) => Err(SessionFailure::Provider(error)),
            Ok(Answer::Crashed(why)) => Err(SessionFailure::Transport(ComputerUseError::new(
                error_code::ACCESSIBILITY_ERROR,
                format!("the Computer Use provider crashed: {why}"),
            ))),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                Err(SessionFailure::Transport(ComputerUseError::new(
                    error_code::ACTION_TIMEOUT,
                    format!(
                        "timed out after {}s waiting for the Computer Use provider to answer {method}",
                        deadline.as_secs()
                    ),
                )))
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                Err(SessionFailure::Transport(ComputerUseError::new(
                    error_code::ACCESSIBILITY_ERROR,
                    "the Computer Use provider ended without answering",
                )))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    struct Fake {
        alive: Arc<AtomicBool>,
        answered: Arc<AtomicUsize>,
    }

    impl Handler for Fake {
        fn handle(&mut self, method: &str, params: Value) -> Result<Value, ComputerUseError> {
            self.answered.fetch_add(1, Ordering::SeqCst);
            match method {
                "echo" => Ok(params),
                "slow" => {
                    std::thread::sleep(Duration::from_millis(300));
                    Ok(json!("late"))
                }
                "refuse" => Err(ComputerUseError::new("app_not_found", "no such app")),
                "panic" => panic!("boom"),
                _ => Err(ComputerUseError::invalid_argument(format!(
                    "unknown {method}"
                ))),
            }
        }
    }

    impl Drop for Fake {
        fn drop(&mut self) {
            self.alive.store(false, Ordering::SeqCst);
        }
    }

    fn stand() -> (ProviderThread, Arc<AtomicBool>, Arc<AtomicUsize>) {
        let alive = Arc::new(AtomicBool::new(true));
        let answered = Arc::new(AtomicUsize::new(0));
        let (alive_in, answered_in) = (alive.clone(), answered.clone());
        let thread = ProviderThread::spawn("computer-use-fake", move || {
            Ok(Fake {
                alive: alive_in,
                answered: answered_in,
            })
        })
        .expect("spawn");
        (thread, alive, answered)
    }

    fn wait_until(check: impl Fn() -> bool) {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !check() {
            assert!(std::time::Instant::now() < deadline, "condition never held");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn answers_are_the_handlers_and_refusals_keep_the_session() {
        let (thread, _, answered) = stand();
        let echoed = thread
            .request("echo", json!({ "a": 1 }), Duration::from_secs(5))
            .expect("echo");
        assert_eq!(echoed, json!({ "a": 1 }));
        match thread.request("refuse", json!({}), Duration::from_secs(5)) {
            Err(SessionFailure::Provider(error)) => assert_eq!(error.code, "app_not_found"),
            other => panic!("expected a provider refusal, got {other:?}"),
        }
        assert_eq!(answered.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn a_deadline_is_a_transport_failure_that_names_the_method() {
        let (thread, _, _) = stand();
        match thread.request("slow", json!({}), Duration::from_millis(50)) {
            Err(SessionFailure::Transport(error)) => {
                assert_eq!(error.code, "action_timeout");
                assert!(error.message.contains("slow"), "{}", error.message);
            }
            other => panic!("expected a timeout, got {other:?}"),
        }
    }

    #[test]
    fn a_crash_is_a_transport_failure_and_the_thread_ends() {
        let (thread, alive, _) = stand();
        match thread.request("panic", json!({}), Duration::from_secs(5)) {
            Err(SessionFailure::Transport(error)) => {
                assert!(error.message.contains("crashed: boom"), "{}", error.message);
            }
            other => panic!("expected a crash report, got {other:?}"),
        }
        wait_until(|| !alive.load(Ordering::SeqCst));
        assert!(matches!(
            thread.request("echo", json!(1), Duration::from_secs(1)),
            Err(SessionFailure::Transport(_))
        ));
    }

    #[test]
    fn dropping_the_session_drops_the_handler_on_its_own_thread() {
        let (thread, alive, _) = stand();
        assert!(alive.load(Ordering::SeqCst));
        drop(thread);
        wait_until(|| !alive.load(Ordering::SeqCst));
    }

    #[test]
    fn a_handler_that_cannot_start_is_the_sessions_error() {
        let failed = ProviderThread::spawn("computer-use-fake", || {
            Err::<Fake, _>(ComputerUseError::new("accessibility_error", "no COM today"))
        });
        assert_eq!(
            failed.err().map(|error| error.message).as_deref(),
            Some("no COM today")
        );
    }
}
