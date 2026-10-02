//! A re-plan's writer, off the collector's thread (t-21494): the collector
//! asks for a later plan and polls for it each collect, so the valid run it
//! stands on keeps being read and judged while the model thinks. The writer
//! is made on its own thread — a key store is read where it is asked from.
//! A pending answer dropped is a request withdrawn, and a newer request
//! passes every one still waiting: behind one write in progress the newest
//! request alone is written, and a wire already answering is not waited on.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};

use super::*;
use crate::computer_use::errand::value::Setup;

/// How a writer is made, on the thread that asks it.
type Factory = Box<dyn FnOnce() -> Box<dyn Generator> + Send>;

/// The name the writer's thread carries.
const WRITER_THREAD: &str = "reflex-plan";

/// The word a plan's row names a request passed by a newer one while it
/// waited: never asked of the model.
pub(crate) const PLAN_SUPERSEDED: &str = "plan_superseded";

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
/// from the moment it was asked; its withdrawal; its place in the order
/// asked, against the newest; and where its answer goes.
struct Request {
    goal: String,
    scope: Scope,
    stage: Stage,
    palette: Palette,
    previous: Option<(ReflexPlan, Value)>,
    began: Instant,
    left: Duration,
    cancelled: Arc<AtomicBool>,
    stamp: u64,
    newest: Arc<AtomicU64>,
    answer: Sender<Written>,
}

impl Request {
    /// Withdrawn by whoever asked, or passed by a newer request.
    fn withdrawn(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst) || self.newest.load(Ordering::SeqCst) != self.stamp
    }
}

/// The window's writer for an autopilot: the first plan is written here, on
/// the collector's thread, before any hand moves; every later plan goes to a
/// writer of the same making on a thread of its own, started at the first
/// later plan and kept until this generator is dropped. Requests queue in
/// the order they were asked, and the newest wins: one withdrawn or passed
/// by a newer request before its turn is answered [`PLAN_SUPERSEDED`] and
/// never asked of the model, so behind one write in progress the newest
/// request alone is written — the write in progress keeps the answer it is
/// waiting on and asks nothing more.
pub(crate) struct Background {
    first: Box<dyn Generator>,
    factory: Option<Factory>,
    requests: Option<Sender<Request>>,
    /// The stamp of the newest request asked.
    newest: Arc<AtomicU64>,
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
            newest: Arc::new(AtomicU64::new(0)),
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
                        if request.withdrawn() {
                            let _ = request
                                .answer
                                .send(unwritten(PLAN_SUPERSEDED, generator.source()));
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
                            request: &request,
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
        let stamp = self.newest.fetch_add(1, Ordering::SeqCst) + 1;
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
            stamp,
            newest: Arc::clone(&self.newest),
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

/// A writer held to its request: a request asked after the wall, after a
/// withdrawal or after a newer request is refused before it reaches the
/// model, and no further road is asked for it.
struct Bounded<'a> {
    generator: &'a mut dyn Generator,
    request: &'a Request,
}

impl Bounded<'_> {
    /// What is left of the request's wall.
    fn left(&self) -> Duration {
        self.request
            .left
            .saturating_sub(self.request.began.elapsed())
    }
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
        if self.request.withdrawn() {
            return Err(PLAN_REFUSED.to_string());
        }
        let left = left.min(self.left());
        if left.is_zero() {
            return Err(crate::systemone::TIMEOUT.to_string());
        }
        self.generator.ask(system, user, left)
    }

    fn pass_over(&mut self, why: &str) -> bool {
        !self.request.withdrawn() && !self.left().is_zero() && self.generator.pass_over(why)
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

    /// A request as the writer's thread holds it, its answer going nowhere.
    fn request(left: Duration, stamp: u64, newest: &Arc<AtomicU64>) -> Request {
        let (image, stage) = capture();
        Request {
            goal: "press the red dots, never the blue".into(),
            scope: scope(),
            stage,
            palette: palette_of(&image, &stage).expect("a palette"),
            previous: None,
            began: Instant::now(),
            left,
            cancelled: Arc::new(AtomicBool::new(false)),
            stamp,
            newest: Arc::clone(newest),
            answer: mpsc::channel().0,
        }
    }

    /// A request whose wall is spent, one withdrawn and one passed by a
    /// newer request are each refused before the model hears of them, and
    /// no other road is asked for them.
    #[test]
    fn expired_cancelled_or_superseded_requests_never_reach_the_generator() {
        let newest = Arc::new(AtomicU64::new(1));
        let mut generator = Scripted {
            answers: vec![Ok(answer_for(&scope()).to_string()); 3].into(),
            asked: Vec::new(),
        };
        let mut spent = request(Duration::ZERO, 1, &newest);
        let mut bounded = Bounded {
            generator: &mut generator,
            request: &spent,
        };
        assert_eq!(
            bounded.ask("", "", Duration::from_secs(1)).err().as_deref(),
            Some(crate::systemone::TIMEOUT)
        );
        assert!(!bounded.pass_over(PLAN_REFUSED));
        spent.left = Duration::from_secs(1);
        spent.cancelled.store(true, Ordering::SeqCst);
        let mut bounded = Bounded {
            generator: &mut generator,
            request: &spent,
        };
        assert_eq!(
            bounded.ask("", "", Duration::from_secs(1)).err().as_deref(),
            Some(PLAN_REFUSED),
            "withdrawn"
        );
        assert!(!bounded.pass_over(PLAN_REFUSED));
        let passed_by = request(Duration::from_secs(1), 1, &newest);
        newest.store(2, Ordering::SeqCst);
        let mut bounded = Bounded {
            generator: &mut generator,
            request: &passed_by,
        };
        assert_eq!(
            bounded.ask("", "", Duration::from_secs(1)).err().as_deref(),
            Some(PLAN_REFUSED),
            "passed by a newer request"
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

    /// Behind a writer still answering, the newest request alone is
    /// written: every request queued before it waits its turn — none is
    /// dropped as though the writer had gone — and is then answered
    /// `plan_superseded` without the model hearing of it; the write in
    /// progress keeps the answer it was waiting on.
    #[test]
    fn the_newest_request_alone_is_written_behind_a_busy_writer() {
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
        let busy = background.plan_later(&ask, wall);
        let began = Instant::now();
        while asked.load(Ordering::SeqCst) == 0 {
            assert!(began.elapsed() < wall, "the writer never took the request");
            std::thread::sleep(Duration::from_millis(5));
        }
        let waiting: Vec<PendingPlan> = (0..3).map(|_| background.plan_later(&ask, wall)).collect();
        for pending in &waiting {
            assert!(
                matches!(pending.poll(), Ok(None)),
                "a request behind a busy writer waits, it is not dropped"
            );
        }
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
        let written = |pending: &PendingPlan, which: &str| {
            answered(pending)
                .unwrap_or_else(|word| panic!("the {which} request lost its writer: {word}"))
                .unwrap_or_else(|| panic!("the {which} request was never answered"))
        };
        assert!(
            written(&busy, "busy").plan.is_ok(),
            "the write in progress keeps its answer"
        );
        let (passed_by, newest) = waiting.split_at(2);
        for (at, pending) in passed_by.iter().enumerate() {
            let answer = written(pending, &format!("{at}th waiting"));
            assert_eq!(
                answer.plan.as_ref().err().map(String::as_str),
                Some(PLAN_SUPERSEDED),
                "a request passed by a newer one"
            );
            assert_eq!(answer.requests, 0, "never asked");
        }
        assert!(
            written(&newest[0], "newest").plan.is_ok(),
            "the newest is written"
        );
        assert_eq!(
            asked.load(Ordering::SeqCst),
            2,
            "the write in progress and the newest: one request each"
        );
    }
}
