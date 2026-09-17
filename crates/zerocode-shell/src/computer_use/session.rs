//! What every platform's provider session promises the window.

use serde_json::Value;

use super::ComputerUseError;

/// A refusal, sorted by what the window should do with the session.
#[derive(Debug)]
pub(super) enum SessionFailure {
    /// The road to the provider broke — a closed socket, a dead thread, a
    /// reply that no longer lines up with its request. The session is
    /// discarded; the next call starts a new one.
    Transport(ComputerUseError),
    /// The provider answered and said no. The session, and the snapshot
    /// cache inside it, are kept.
    Provider(ComputerUseError),
}

impl SessionFailure {
    #[cfg(test)]
    pub(super) fn into_error(self) -> ComputerUseError {
        match self {
            Self::Transport(error) | Self::Provider(error) => error,
        }
    }
}

/// One provider session: started lazily by the first request, ended when the
/// window drops it.
pub(super) trait ProviderSession: Sized {
    fn start() -> Result<Self, ComputerUseError>;
    fn request(&mut self, method: &str, params: Value) -> Result<Value, SessionFailure>;
    /// The helper process, when the platform runs one the window can signal.
    fn helper_pid(&self) -> Option<i32> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failure_keeps_its_words_whichever_way_it_is_sorted() {
        let error = ComputerUseError::new("accessibility_error", "closed");
        assert_eq!(SessionFailure::Transport(error.clone()).into_error(), error);
        assert_eq!(SessionFailure::Provider(error.clone()).into_error(), error);
    }
}
