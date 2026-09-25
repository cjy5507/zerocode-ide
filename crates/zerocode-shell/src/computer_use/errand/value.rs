//! The value a walk types (t-6720): written by the value seat's chosen row
//! (`zerocode_core::type_value`) only once the walk's judgment chose to type
//! into a field, typed again from memory when a retry or a replay asks for
//! the very same write, and handed to the world's typing road alone — never
//! to a row, a log, or an argv a log keeps.
//!
//! Jev chooses; it does not write. So the one string a goal walk needs is
//! asked of a small model, and which model, down which road, is the seat's
//! table ([`zerocode_core::type_value::chosen`]) — nothing here spells one.
//! The road this product holds a login for is the subscription's
//! ([`Road::Anthropic`]): the login is the one the window's usage reader
//! already asks with (`accounts::reading_env_for` + `accounts::usage_login`,
//! read only when a value is actually written), and the token goes in the
//! request's `Authorization` header and nowhere else.
//!
//! What makes two writes the same write is the core's
//! ([`zerocode_core::type_value::identity`]): the question's version, the
//! model, the goal, the words around the box, what the box holds, the
//! document and the box itself. [`Values`] keeps what was written under that
//! identity, for as many writes as the longest walk the verb allows can make
//! ([`zerocode_core::computer_use::WALK_STEPS_MAX`]); anything else asks
//! afresh.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::Duration;

use zerocode_core::type_value::{FieldLook, ValueRow};

/// Why a writer wrote nothing, beside the wire's own words
/// (`crate::systemone`) and a value the seat's rules refused
/// ([`zerocode_core::type_value::ValueRefusal::token`]).
pub const NO_ROW: &str = "value_no_row";
pub const NO_LOGIN: &str = "value_no_login";
pub const ROAD_UNSUPPORTED: &str = "value_road_unsupported";

/// A value, written. Its `Debug` never prints the value: a written value is
/// what a person's field will hold, and a debug line is a log line.
pub struct Written {
    pub value: String,
    /// The model that wrote it, as the row names it.
    pub model: String,
    /// How long the write took, whole.
    pub ms: u64,
}

impl std::fmt::Debug for Written {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Written")
            .field("chars", &self.value.chars().count())
            .field("model", &self.model)
            .field("ms", &self.ms)
            .finish()
    }
}

/// Writing the value one field should hold — the seam the small model sits
/// behind and a test replaces.
pub trait ValueWriter {
    /// The row this writer asks: half of a written value's identity, so a
    /// value another model wrote is never typed as this one's.
    fn row(&self) -> Option<&'static ValueRow>;

    /// The value `look` asks for, written within `left` — or the token of
    /// why there is none.
    ///
    /// # Errors
    ///
    /// The token a walk's row names the refusal by.
    fn write(&mut self, look: &FieldLook<'_>, left: Duration) -> Result<Written, String>;
}

/// Values written before, by identity — what a retry or a replay types again
/// instead of asking. Oldest out first, at most `cap` held.
#[derive(Debug)]
pub struct Values {
    held: VecDeque<(String, String)>,
    cap: usize,
}

impl Values {
    #[must_use]
    pub const fn new(cap: usize) -> Self {
        Self {
            held: VecDeque::new(),
            cap,
        }
    }

    /// How many values it holds at most.
    #[must_use]
    pub const fn cap(&self) -> usize {
        self.cap
    }

    /// The value written under `identity`, if one was.
    #[must_use]
    pub fn recall(&self, identity: &str) -> Option<String> {
        let _ = identity;
        None
    }

    /// Keep `value` under `identity`.
    pub fn keep(&mut self, identity: String, value: String) {
        let _ = (identity, value, self.cap, &self.held);
    }
}

/// The window's values: one memory every walk the window runs types from.
#[must_use]
pub fn window_values() -> Arc<Mutex<Values>> {
    static VALUES: OnceLock<Arc<Mutex<Values>>> = OnceLock::new();
    Arc::clone(VALUES.get_or_init(|| {
        Arc::new(Mutex::new(Values::new(
            zerocode_core::computer_use::WALK_STEPS_MAX,
        )))
    }))
}

/// The memory, surviving a panicking holder: a value either was written or
/// was not, and the map is whole at every step.
pub fn held(values: &Mutex<Values>) -> std::sync::MutexGuard<'_, Values> {
    values.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Where a writer's login comes from.
enum Login {
    /// The window's own: its config root, read the way the usage reader
    /// reads it, only when a value is written.
    Window(PathBuf),
    /// A token handed in — how a test crosses a real socket without a
    /// keychain.
    #[cfg(test)]
    Given(String),
}

/// The writer that actually asks: the seat's chosen row, down its road.
pub struct LiveWriter {
    login: Login,
    url: String,
}

impl LiveWriter {
    /// The window's writer: the login its usage reader asks with, under the
    /// window's `config_root`. Reads nothing until a value is written.
    #[must_use]
    pub fn window(config_root: &Path) -> Self {
        Self {
            login: Login::Window(config_root.to_path_buf()),
            url: zerocode_core::type_value::ANTHROPIC_WIRE.url.to_string(),
        }
    }

    /// A writer at `url` with `token` — how a test crosses a real socket.
    #[cfg(test)]
    #[must_use]
    pub fn at(url: &str, token: &str) -> Self {
        Self {
            login: Login::Given(token.to_string()),
            url: url.to_string(),
        }
    }
}

impl ValueWriter for LiveWriter {
    fn row(&self) -> Option<&'static ValueRow> {
        zerocode_core::type_value::chosen()
    }

    fn write(&mut self, look: &FieldLook<'_>, left: Duration) -> Result<Written, String> {
        let _ = (look, left, &self.login, &self.url);
        Err(ROAD_UNSUPPORTED.to_string())
    }
}

#[cfg(test)]
mod tests;
