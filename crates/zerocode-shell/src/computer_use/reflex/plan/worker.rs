//! A re-plan's writer, off the collector's thread (t-21494): the collector
//! asks for a later plan and polls for it each collect, so the valid run it
//! stands on keeps being read and judged while the model thinks. The writer
//! is made on its own thread — a key store is read where it is asked from —
//! and a pending answer dropped is a request withdrawn: no further request
//! of it reaches the model, and a wire already answering is not waited on.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};

use super::*;
use crate::computer_use::errand::value::Setup;

/// How a writer is made, on the thread that asks it.
type Factory = Box<dyn FnOnce() -> Box<dyn Generator> + Send>;

/// The name the writer's thread carries.
const WRITER_THREAD: &str = "reflex-plan";

/// A plan asked for and not yet answered: polled by the collector, never
/// waited on; dropped, it is withdrawn.
pub(crate) struct PendingPlan {
    answer: Receiver<Written>,
    cancelled: Arc<AtomicBool>,
}

impl PendingPlan {
    /// A plan already written — a scripted writer's, answered at once.
    pub(super) fn ready(written: Written) -> Self {
        let (answer, received) = mpsc::channel();
        let _ = answer.send(written);
        Self {
            answer: received,
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }

    /// The plan once it is written; `None` while the writer works.
    ///
    /// # Errors
    ///
    /// [`NO_GENERATOR`]: the writer went without answering — its thread
    /// ended, or never began.
    pub(crate) fn poll(&self) -> Result<Option<Written>, String> {
        match self.answer.try_recv() {
            Ok(written) => Ok(Some(written)),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => Err(NO_GENERATOR.to_string()),
        }
    }
}

impl Drop for PendingPlan {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }
}

/// One plan asked of the writer's thread: the ask, owned; the wall it has
/// from the moment it was asked; its withdrawal; and where its answer goes.
struct Request {
    goal: String,
    scope: Scope,
    stage: Stage,
    palette: Palette,
    previous: Option<(ReflexPlan, Value)>,
    began: Instant,
    left: Duration,
    cancelled: Arc<AtomicBool>,
    answer: Sender<Written>,
}

/// The window's writer for an autopilot: the first plan is written here, on
/// the collector's thread, before any hand moves; every later plan goes to a
/// writer of the same making on a thread of its own, started at the first
/// later plan and kept until this generator is dropped. Requests queue in
/// the order they were asked, a withdrawn one skipped.
pub(crate) struct Background {
    first: Box<dyn Generator>,
    factory: Option<Factory>,
    requests: Option<Sender<Request>>,
}

impl Background {
    /// The window's own writers, on the road the person set up.
    pub(crate) fn window(setup: Setup) -> Self {
        Self::new(
            Box::new(LiveWriter::window(setup.clone())),
            Box::new(move || Box::new(LiveWriter::window(setup)) as Box<dyn Generator>),
        )
    }

    pub(crate) fn new(first: Box<dyn Generator>, factory: Factory) -> Self {
        Self {
            first,
            factory: Some(factory),
            requests: None,
        }
    }

    /// The writer's thread, started at the first later plan. `None` once it
    /// could not be started: a request then goes unanswered, and its poll
    /// says so.
    fn writer(&mut self) -> Option<&Sender<Request>> {
        if let Some(factory) = self.factory.take() {
            let (send, requests) = mpsc::channel::<Request>();
            let started = std::thread::Builder::new()
                .name(WRITER_THREAD.into())
                .spawn(move || {
                    let mut generator = factory();
                    while let Ok(request) = requests.recv() {
                        if request.cancelled.load(Ordering::SeqCst) {
                            continue;
                        }
                        let ask = Ask {
                            goal: &request.goal,
                            scope: &request.scope,
                            stage: &request.stage,
                            palette: &request.palette,
                            previous: request
                                .previous
                                .as_ref()
                                .map(|(plan, outcomes)| Previous { plan, outcomes }),
                        };
                        let mut bounded = Bounded {
                            generator: generator.as_mut(),
                            began: request.began,
                            left: request.left,
                            cancelled: &request.cancelled,
                        };
                        let written = write_plan(&mut bounded, &ask);
                        let _ = request.answer.send(written);
                    }
                });
            self.requests = started.ok().map(|_| send);
        }
        self.requests.as_ref()
    }
}

impl Generator for Background {
    fn unready(&self) -> Option<String> {
        self.first.unready()
    }

    fn model(&self) -> Option<String> {
        self.first.model()
    }

    fn source(&self) -> &'static str {
        self.first.source()
    }

    fn ask(&mut self, system: &str, user: &str, left: Duration) -> Result<Said, String> {
        self.first.ask(system, user, left)
    }

    fn pass_over(&mut self, why: &str) -> bool {
        self.first.pass_over(why)
    }

    fn plan_later(&mut self, ask: &Ask<'_>, left: Duration) -> PendingPlan {
        let (answer, received) = mpsc::channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        let request = Request {
            goal: ask.goal.to_string(),
            scope: ask.scope.clone(),
            stage: *ask.stage,
            palette: ask.palette.clone(),
            previous: ask
                .previous
                .as_ref()
                .map(|previous| (previous.plan.clone(), previous.outcomes.clone())),
            began: Instant::now(),
            left,
            cancelled: Arc::clone(&cancelled),
            answer,
        };
        // A writer that went, or never began, drops the request with the
        // sender of its answer: the poll says so.
        if let Some(writer) = self.writer() {
            let _ = writer.send(request);
        }
        PendingPlan {
            answer: received,
            cancelled,
        }
    }
}

/// A writer held to a request's wall and its withdrawal: a request asked
/// after either is refused before it reaches the model, and no further road
/// is asked for it.
struct Bounded<'a> {
    generator: &'a mut dyn Generator,
    began: Instant,
    left: Duration,
    cancelled: &'a AtomicBool,
}

impl Generator for Bounded<'_> {
    fn unready(&self) -> Option<String> {
        self.generator.unready()
    }

    fn model(&self) -> Option<String> {
        self.generator.model()
    }

    fn source(&self) -> &'static str {
        self.generator.source()
    }

    fn ask(&mut self, system: &str, user: &str, left: Duration) -> Result<Said, String> {
        if self.cancelled.load(Ordering::SeqCst) {
            return Err(PLAN_REFUSED.to_string());
        }
        let left = left.min(self.left.saturating_sub(self.began.elapsed()));
        if left.is_zero() {
            return Err(crate::systemone::TIMEOUT.to_string());
        }
        self.generator.ask(system, user, left)
    }

    fn pass_over(&mut self, why: &str) -> bool {
        !self.cancelled.load(Ordering::SeqCst)
            && self.began.elapsed() < self.left
            && self.generator.pass_over(why)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::sync::atomic::AtomicUsize;

    use super::super::tests::{Scripted, answer_for, capture, scope};
    use super::*;

    #[test]
    fn polling_never_waits_and_drop_cancels_a_disconnected_writer() {
        let (send, received) = mpsc::channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        let pending = PendingPlan {
            answer: received,
            cancelled: Arc::clone(&cancelled),
        };
        assert!(pending.poll().is_ok_and(|answer| answer.is_none()));
        drop(send);
        assert_eq!(pending.poll().err().as_deref(), Some(NO_GENERATOR));
        drop(pending);
        assert!(cancelled.load(Ordering::SeqCst));
    }

    #[test]
    fn expired_or_cancelled_requests_never_reach_the_generator() {
        let mut generator = Scripted {
            answers: Vec::new().into(),
            asked: Vec::new(),
        };
        let cancelled = AtomicBool::new(false);
        let mut bounded = Bounded {
            generator: &mut generator,
            began: Instant::now(),
            left: Duration::ZERO,
            cancelled: &cancelled,
        };
        assert_eq!(
            bounded.ask("", "", Duration::from_secs(1)).err().as_deref(),
            Some(crate::systemone::TIMEOUT)
        );
        assert!(!bounded.pass_over(PLAN_REFUSED));
        cancelled.store(true, Ordering::SeqCst);
        bounded.left = Duration::from_secs(1);
        assert_eq!(
            bounded.ask("", "", Duration::from_secs(1)).err().as_deref(),
            Some(PLAN_REFUSED)
        );
        assert!(!bounded.pass_over(PLAN_REFUSED));
        assert!(generator.asked.is_empty());
    }

    /// A writer whose first request waits on the test's gate, answering a
    /// plan to every request after it at once.
    struct Gated {
        gate: Arc<Mutex<mpsc::Receiver<()>>>,
        asked: Arc<AtomicUsize>,
    }

    impl Generator for Gated {
        fn unready(&self) -> Option<String> {
            None
        }

        fn model(&self) -> Option<String> {
            Some("gated-test".into())
        }

        fn ask(&mut self, system: &str, user: &str, left: Duration) -> Result<Said, String> {
            if self.asked.fetch_add(1, Ordering::SeqCst) == 0 {
                self.gate
                    .lock()
                    .expect("one request at a time")
                    .recv_timeout(Duration::from_secs(10))
                    .expect("the test opens its gate");
            }
            Scripted {
                answers: vec![Ok(answer_for(&scope()).to_string())].into(),
                asked: Vec::new(),
            }
            .ask(system, user, left)
        }
    }

    /// Requests queue behind a writer still answering: a plan asked while the
    /// writer works on one since withdrawn is answered in its turn, never
    /// dropped as though the writer had gone.
    #[test]
    fn a_request_behind_a_busy_writer_is_answered_in_its_turn() {
        let (release, gate) = mpsc::channel();
        let asked = Arc::new(AtomicUsize::new(0));
        let gated = Gated {
            gate: Arc::new(Mutex::new(gate)),
            asked: Arc::clone(&asked),
        };
        let mut background = Background::new(
            Box::new(Scripted {
                answers: Vec::new().into(),
                asked: Vec::new(),
            }),
            Box::new(move || Box::new(gated) as Box<dyn Generator>),
        );
        let (image, stage) = capture();
        let palette = palette_of(&image, &stage).expect("a palette");
        let scope = scope();
        let ask = Ask {
            goal: "press the red dots, never the blue",
            scope: &scope,
            stage: &stage,
            palette: &palette,
            previous: None,
        };
        let wall = Duration::from_secs(5);
        let withdrawn = background.plan_later(&ask, wall);
        let began = Instant::now();
        while asked.load(Ordering::SeqCst) == 0 {
            assert!(began.elapsed() < wall, "the writer never took the request");
            std::thread::sleep(Duration::from_millis(5));
        }
        let second = background.plan_later(&ask, wall);
        drop(withdrawn);
        let third = background.plan_later(&ask, wall);
        assert!(
            matches!(third.poll(), Ok(None)),
            "a request behind a busy writer waits for its turn"
        );
        release.send(()).expect("the gate");
        let answered = |pending: &PendingPlan| {
            let began = Instant::now();
            loop {
                match pending.poll() {
                    Ok(None) if began.elapsed() < wall => {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    other => return other,
                }
            }
        };
        for (which, pending) in [("second", &second), ("third", &third)] {
            let written = answered(pending)
                .unwrap_or_else(|word| panic!("the {which} request lost its writer: {word}"))
                .unwrap_or_else(|| panic!("the {which} request was never answered"));
            assert!(written.plan.is_ok(), "{which}: a plan");
        }
        assert_eq!(
            asked.load(Ordering::SeqCst),
            3,
            "one request each, none twice"
        );
    }
}
