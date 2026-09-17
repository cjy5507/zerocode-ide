//! Provider-neutral notification contract for orchestration mail pointers.
//!
//! This module deliberately cannot represent an orchestration message body.
//! A provider adapter receives [`PointerNotice`], whose text is constructed by
//! [`zerocode_core::orchestration::pointer_text`], or receives nothing. The
//! ledger therefore remains the only authority that leases, acknowledges, or
//! completes mail even when a provider offers a faster wake-up path.

use std::num::NonZeroUsize;

/// Stable identity of one pending-mail watermark.
///
/// The newest pending message changes whenever new durable mail arrives. Run
/// and address keep identical message ids in different inboxes distinct. The
/// fields stay private so an adapter cannot reinterpret them as provider
/// routing credentials.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct NoticeId {
    run: String,
    address: String,
    newest: String,
}

impl std::fmt::Debug for NoticeId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("NoticeId(<opaque-ledger-watermark>)")
    }
}

/// The one payload a native session route may receive.
///
/// There is no public constructor accepting text. This is the structural
/// guard against promoting message bodies, task prose, receipts, or
/// `worker_done` into a provider's best-effort channel.
#[derive(Clone, PartialEq, Eq)]
pub struct PointerNotice {
    id: NoticeId,
    text: String,
    pending: NonZeroUsize,
}

impl PointerNotice {
    /// Build a pointer for a non-empty durable inbox.
    ///
    /// Empty identity parts are refused instead of being normalized: the
    /// caller can retain the existing PTY fallback without inventing a native
    /// deduplication key.
    #[must_use]
    pub fn new(
        run: impl Into<String>,
        address: impl Into<String>,
        newest: impl Into<String>,
        pending: usize,
    ) -> Option<Self> {
        let run = run.into();
        let address = address.into();
        let newest = newest.into();
        let pending = NonZeroUsize::new(pending)?;
        if run.is_empty() || address.is_empty() || newest.is_empty() {
            return None;
        }
        Some(Self {
            id: NoticeId {
                run,
                address,
                newest,
            },
            text: zerocode_core::orchestration::pointer_text(pending.get()),
            pending,
        })
    }

    /// Stable watermark used by the process-local deduplication fence.
    #[must_use]
    pub fn id(&self) -> &NoticeId {
        &self.id
    }

    /// Fixed advice text; never an orchestration message body.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }
}

impl std::fmt::Debug for PointerNotice {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PointerNotice")
            .field("id", &self.id)
            .field("pending", &self.pending)
            .field("text", &"<fixed-orchestration-pointer>")
            .finish()
    }
}

/// What one at-most-once native attempt established.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationOutcome {
    /// The provider confirmed that it accepted the pointer.
    Confirmed,
    /// No provider action began, so the PTY path is safe immediately.
    DefinitelyUnsent,
    /// Provider action may have begun; native retry is forbidden for this
    /// watermark and route generation.
    Unknown,
}

/// One provider route capable of attempting the fixed pointer.
///
/// Implementations must bound their own I/O and classify every result without
/// parsing provider prose. The hub invokes this outside all route and attempt
/// locks.
pub trait SessionNotifier: Send + Sync + 'static {
    fn notify(&self, notice: &PointerNotice) -> NotificationOutcome;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_contract_can_construct_only_the_fixed_ledger_pointer() {
        let notice =
            PointerNotice::new("run-7", "worker:w-9", "m-11", 2).expect("a non-empty watermark");

        assert_eq!(notice.text(), zerocode_core::orchestration::pointer_text(2));
        assert!(!notice.text().contains("m-11"));
        assert!(PointerNotice::new("run-7", "worker:w-9", "", 2).is_none());
        assert!(PointerNotice::new("run-7", "worker:w-9", "m-11", 0).is_none());
    }

    #[test]
    fn debug_output_redacts_every_ledger_identity_and_the_pointer_text() {
        let notice = PointerNotice::new(
            "secret-run-name",
            "secret-worker-address",
            "secret-message-id",
            3,
        )
        .expect("a notice");
        let debug = format!("{notice:?}");

        for private in [
            "secret-run-name",
            "secret-worker-address",
            "secret-message-id",
            "zerocode-orc check",
        ] {
            assert!(!debug.contains(private), "debug leaked {private}: {debug}");
        }
        assert!(debug.contains("pending: 3"));
    }
}
