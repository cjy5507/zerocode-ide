//! One panel worker and one authenticated connection. Callers only replace
//! a pending presentation; no helper reply delays command admission.

use std::sync::{Arc, Condvar, Mutex, OnceLock, PoisonError};
use std::time::Duration;

use serde_json::Value;

use super::{PanelEndpoint, Wire};

/// A busy provider is checked again without waiting for another presentation.
const BUSY_RETRY_INTERVAL: Duration = Duration::from_millis(250);

#[derive(Clone)]
struct Update {
    state: Value,
    endpoint: Option<PanelEndpoint>,
    helper_lost: bool,
}

#[derive(Default)]
struct Pending {
    latest: Option<Update>,
    revision: u64,
    had_endpoint: bool,
}

#[derive(Default)]
struct Mailbox {
    pending: Mutex<Pending>,
    changed: Condvar,
}

impl Mailbox {
    fn publish(&self, state: &Value, endpoint: Option<PanelEndpoint>) {
        let mut pending = self.pending.lock().unwrap_or_else(PoisonError::into_inner);
        let revision = state.get("revision").and_then(Value::as_u64).unwrap_or(0);
        if revision < pending.revision {
            return;
        }
        pending.revision = revision;
        // Presentations coalesce, but a helper dying before the worker reads
        // its endpoint must still permit the share's one recovery attempt.
        let helper_lost = (pending.had_endpoint && endpoint.is_none())
            || pending
                .latest
                .as_ref()
                .is_some_and(|update| update.helper_lost);
        pending.had_endpoint = endpoint.is_some();
        pending.latest = Some(Update {
            state: state.clone(),
            endpoint,
            helper_lost,
        });
        self.changed.notify_one();
    }

    fn next(&self, retry: Option<&Update>, retry_after: Duration) -> Update {
        let mut pending = self.pending.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            if let Some(update) = pending.latest.take() {
                return update;
            }
            if let Some(retry) = retry {
                let (held, timeout) = self
                    .changed
                    .wait_timeout_while(pending, retry_after, |pending| pending.latest.is_none())
                    .unwrap_or_else(PoisonError::into_inner);
                pending = held;
                if timeout.timed_out() && pending.latest.is_none() {
                    return retry.clone();
                }
            } else {
                pending = self
                    .changed
                    .wait(pending)
                    .unwrap_or_else(PoisonError::into_inner);
            }
        }
    }
}

#[derive(Default)]
struct Delivery {
    endpoint: Option<PanelEndpoint>,
    wire: Option<Wire>,
    startup_attempted: bool,
    recovery_used: bool,
    startup_share: (Option<String>, u64),
    retry: Option<Update>,
}

impl Delivery {
    fn send(
        &mut self,
        update: Update,
        mut connect: impl FnMut(&PanelEndpoint) -> Option<Wire>,
        mut ensure: impl FnMut() -> bool,
    ) {
        // A new presentation supersedes a busy retry, including share end.
        self.retry = None;
        let target = update.state.get("target").and_then(Value::as_str);
        let generation = update
            .state
            .get("generation")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        if self.startup_share.0.as_deref() != target || self.startup_share.1 != generation {
            // Remember a failed launch across notices of the same share, but
            // let a new target or ownership generation recover a lost helper.
            self.startup_share = (target.map(str::to_string), generation);
            self.startup_attempted = false;
            self.recovery_used = false;
        }
        let helper_lost =
            update.helper_lost || (self.endpoint.is_some() && update.endpoint.is_none());
        if helper_lost && !self.recovery_used {
            self.recovery_used = true;
            self.startup_attempted = false;
        }
        if self.endpoint != update.endpoint {
            self.endpoint = update.endpoint;
            self.wire = None;
        }
        let Some(endpoint) = self.endpoint.as_ref() else {
            if !self.startup_attempted && target.is_some() {
                self.startup_attempted = ensure();
                if !self.startup_attempted {
                    // try_lock was busy: no launch happened. Retry the last
                    // state even if losing this helper was the last notice.
                    self.retry = Some(Update {
                        state: update.state,
                        endpoint: None,
                        helper_lost: false,
                    });
                }
            }
            return;
        };
        if self.wire.is_none() {
            self.wire = connect(endpoint);
        }
        if self
            .wire
            .as_mut()
            .is_none_or(|wire| wire.request("sharePanel", update.state).is_err())
        {
            // Recover on the next pending presentation, with no retry loop.
            self.wire = None;
        }
    }
}

fn connect(endpoint: &PanelEndpoint) -> Option<Wire> {
    let stream = std::os::unix::net::UnixStream::connect(&endpoint.socket).ok()?;
    let wire = Wire::new(stream, endpoint.token.clone()).ok()?;
    // Keep the existing presentation timeout off the command's own thread.
    let limit = Some(std::time::Duration::from_secs(2));
    wire.stream.set_read_timeout(limit).ok()?;
    wire.stream.set_write_timeout(limit).ok()?;
    Some(wire)
}

pub(super) fn publish(state: &Value, endpoint: Option<PanelEndpoint>) {
    static WORKER: OnceLock<Option<Arc<Mailbox>>> = OnceLock::new();
    let mailbox = WORKER.get_or_init(|| {
        let mailbox = Arc::new(Mailbox::default());
        let worker = Arc::clone(&mailbox);
        std::thread::Builder::new()
            .name("computer-share-panel".into())
            .spawn(move || {
                let mut delivery = Delivery::default();
                loop {
                    let update = worker.next(delivery.retry.as_ref(), BUSY_RETRY_INTERVAL);
                    #[cfg(not(test))]
                    let ensure = super::super::ensure_share_panel;
                    // Tests never launch an application, even if a fixture advertises a panel.
                    #[cfg(test)]
                    let ensure = || true;
                    delivery.send(update, connect, ensure);
                }
            })
            .ok()?;
        Some(mailbox)
    });
    if let Some(mailbox) = mailbox {
        mailbox.publish(state, endpoint);
    }
}

#[cfg(test)]
mod tests;
