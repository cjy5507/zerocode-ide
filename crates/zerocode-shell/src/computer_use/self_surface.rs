//! ZeroCode's own window as a Computer Use target — the window's half
//! (t-73906; the rule is core `computer_use_protocol::self_surface`).
//!
//! A request whose `--app` names this process never reaches the helper: the
//! page answers it. A look asks the page for its controls and the facts each
//! one carries; an act asks the page for one control's facts again, judges
//! them here, and only then asks the page to press it in its own document.
//! The page makes no OS event, so the person's pointer, focus and keys stay
//! theirs while an agent works in the same window.

use serde_json::Value;
use zerocode_core::computer_use_protocol::self_surface::OwnApp;

use super::ComputerUseError;

/// The window's way of asking its page: one request, one answer.
pub(crate) type Page = dyn Fn(&Value) -> Result<Value, ComputerUseError> + Send + Sync;

/// The answer to a request that names ZeroCode's own app, or `None` when the
/// request is the helper's.
pub(super) fn route(_method: &str, _params: &Value) -> Option<Result<Value, ComputerUseError>> {
    None
}

/// The page stood in for by a test, on this thread.
#[cfg(test)]
pub(crate) mod stand_in {
    use std::cell::RefCell;
    use std::sync::Arc;

    use super::{OwnApp, Page};

    thread_local! {
        pub(super) static STANDING: RefCell<Option<(OwnApp, Arc<Page>)>> = const { RefCell::new(None) };
    }

    /// The page stands while this lives.
    pub(crate) struct Standing(());

    impl Drop for Standing {
        fn drop(&mut self) {
            STANDING.with(|standing| standing.borrow_mut().take());
        }
    }

    pub(crate) fn stand(own: OwnApp, page: Arc<Page>) -> Standing {
        STANDING.with(|standing| *standing.borrow_mut() = Some((own, page)));
        Standing(())
    }
}

#[cfg(test)]
mod tests;
