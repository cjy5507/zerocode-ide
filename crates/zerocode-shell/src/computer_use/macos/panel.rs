//! One panel worker and one authenticated connection. Callers only replace
//! a pending presentation; no helper reply delays command admission.

use std::sync::{Arc, Condvar, Mutex, OnceLock, PoisonError};

use serde_json::Value;

use super::{PanelEndpoint, Wire};

struct Update {
    state: Value,
    endpoint: Option<PanelEndpoint>,
}

#[derive(Default)]
struct Pending {
    latest: Option<Update>,
    revision: u64,
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
        pending.latest = Some(Update {
            state: state.clone(),
            endpoint,
        });
        self.changed.notify_one();
    }

    fn next(&self) -> Update {
        let mut pending = self.pending.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            if let Some(update) = pending.latest.take() {
                return update;
            }
            pending = self
                .changed
                .wait(pending)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }
}

#[derive(Default)]
struct Delivery {
    endpoint: Option<PanelEndpoint>,
    wire: Option<Wire>,
    startup_attempted: bool,
}

impl Delivery {
    fn send(
        &mut self,
        update: Update,
        mut connect: impl FnMut(&PanelEndpoint) -> Option<Wire>,
        mut ensure: impl FnMut() -> bool,
    ) {
        if self.endpoint != update.endpoint {
            self.endpoint = update.endpoint;
            self.wire = None;
        }
        let Some(endpoint) = self.endpoint.as_ref() else {
            if !self.startup_attempted
                && update
                    .state
                    .get("target")
                    .is_some_and(|target| !target.is_null())
            {
                self.startup_attempted = ensure();
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
                    let update = worker.next();
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
