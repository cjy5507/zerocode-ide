//! One window shared with an agent, held by the window (t-73906; the rule is
//! core `computer_use_protocol::share`).
//!
//! A command is admitted here before it acts: the first act in an app asks
//! the person once through the question popup (`computer:share`, answered by
//! `computer_confirm_answer`), and the answer is held for the window's
//! lifetime. The band paints the share (`computer:share-state`) and offers
//! "take control" and "hand back"; a request admitted under one generation
//! is refused at its next provider step once the person took control.

use serde::Serialize;
use serde_json::Value;
use zerocode_core::computer_use::ComputerMethod;

use super::ComputerUseError;
use super::confirm::Decision;

/// The card the window raises: which app an agent asks for, and how long the
/// window waits before the person's silence is the answer.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct ShareAsk {
    pub(crate) id: String,
    pub(crate) app: String,
    #[serde(rename = "timeoutMs")]
    pub(crate) timeout_ms: u64,
}

pub(crate) type Asker = dyn Fn(&ShareAsk) -> Decision + Send + Sync;
pub(crate) type Notice = dyn Fn(&Value) + Send + Sync;

/// While this lives, the command it was admitted for is held to the share's
/// generation it saw.
pub(crate) struct Scope(());

/// Admit `method` with `params` under the share: ask the person when it is
/// the first act in an app, refuse what lies outside the shared window.
pub(crate) fn admit(_method: ComputerMethod, _params: &Value) -> Result<Scope, ComputerUseError> {
    Ok(Scope(()))
}

/// Whether the request this thread runs is still under the share it was
/// admitted to: refused once the person took control since.
pub(crate) fn ensure_current() -> Result<(), ComputerUseError> {
    Ok(())
}

/// The person takes control of the shared window.
pub(crate) fn take_control() {}

/// The person hands the shared window back.
pub(crate) fn give_back() {}

/// The share as the band paints it.
pub(crate) fn state() -> Value {
    Value::Null
}

/// Hand the window's card and band to the share — at boot.
pub(crate) fn install(_asker: Box<Asker>, _notice: Box<Notice>) {}

#[cfg(test)]
pub(crate) mod stand_in {
    use std::cell::RefCell;
    use std::rc::Rc;

    use serde_json::Value;

    use super::{Decision, ShareAsk};

    thread_local! {
        pub(super) static ASKED: RefCell<Option<Rc<dyn Fn(&ShareAsk) -> Decision>>> = const { RefCell::new(None) };
        pub(super) static NOTICES: RefCell<Vec<Value>> = const { RefCell::new(Vec::new()) };
    }

    /// The person stood in on this thread: answers every card with `answer`
    /// and counts the cards.
    pub(crate) struct Standing(());

    impl Drop for Standing {
        fn drop(&mut self) {
            ASKED.with(|asked| asked.borrow_mut().take());
            NOTICES.with(|notices| notices.borrow_mut().clear());
        }
    }

    pub(crate) fn stand(answer: impl Fn(&ShareAsk) -> Decision + 'static) -> Standing {
        ASKED.with(|asked| *asked.borrow_mut() = Some(Rc::new(answer)));
        Standing(())
    }

    /// What the band was told so far on this thread.
    pub(crate) fn notices() -> Vec<Value> {
        NOTICES.with(|notices| notices.borrow().clone())
    }
}

#[cfg(test)]
mod tests;
