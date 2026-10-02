//! A re-plan's writer lives off the collector thread. Its key store is
//! constructed there, not sent across threads. Dropping its pending answer
//! cancels further requests, without waiting for a wire already answering.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError};

use super::*;
use crate::computer_use::errand::value::Setup;

type Factory = Box<dyn FnOnce() -> Box<dyn Generator> + Send>;

pub(crate) struct PendingPlan {
    answer: Receiver<Written>,
    cancelled: Arc<AtomicBool>,
}

impl PendingPlan {
    pub(super) fn ready(written: Written) -> Self {
        let (answer, received) = mpsc::channel();
        let _ = answer.send(written);
        Self {
            answer: received,
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }

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

struct Request {
    goal: String,
    scope: Scope,
    stage: Stage,
    palette: Palette,
    previous: Option<(ReflexPlan, Value)>,
    began: Instant,
    left: Duration,
    cancelled: Arc<AtomicBool>,
    answer: mpsc::Sender<Written>,
}

pub(crate) struct Background {
    first: Box<dyn Generator>,
    factory: Option<Factory>,
    requests: Option<SyncSender<Request>>,
}

impl Background {
    pub(crate) fn window(setup: Setup) -> Self {
        Self::new(
            Box::new(LiveWriter::window(setup.clone())),
            Box::new(move || Box::new(LiveWriter::window(setup))),
        )
    }

    pub(crate) fn new(first: Box<dyn Generator>, factory: Factory) -> Self {
        Self {
            first,
            factory: Some(factory),
            requests: None,
        }
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
        if let Some(factory) = self.factory.take() {
            let (send, requests) = mpsc::sync_channel::<Request>(1);
            self.requests = Some(send);
            let _ = std::thread::Builder::new()
                .name("reflex-plan".into())
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
        }
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
        if let Some(requests) = &self.requests {
            let _ = requests.try_send(request);
        }
        PendingPlan {
            answer: received,
            cancelled,
        }
    }
}

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
        let mut generator = super::super::tests::Scripted {
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
}
