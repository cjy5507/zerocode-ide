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
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use zerocode_core::type_value::{ANTHROPIC_WIRE, FieldLook, Road, ValueRow};

use crate::systemone::{SCHEMA, TIMEOUT, TRANSPORT, token_for};

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
        self.held
            .iter()
            .find(|(kept, _)| kept == identity)
            .map(|(_, value)| value.clone())
    }

    /// Keep `value` under `identity`, in place of whatever it held; the
    /// oldest goes when the memory is full.
    pub fn keep(&mut self, identity: String, value: String) {
        self.held.retain(|(kept, _)| *kept != identity);
        self.held.push_back((identity, value));
        while self.held.len() > self.cap {
            self.held.pop_front();
        }
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

/// The agent whose login the window reads a subscription with — the one
/// its usage reader asks the same endpoint's owner with
/// (`usage_runtime`, `scm_runtime`).
const LOGIN_AGENT: &str = "claude";

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

    /// The token this writer asks with, read now: the window's login is the
    /// one its usage reader reads — the keychain item the CLI refreshes for
    /// the selected account, then the runtime home's copy — and nothing here
    /// keeps it past the request.
    fn token(&self) -> Option<String> {
        match &self.login {
            Login::Window(config_root) => {
                let env = crate::accounts::reading_env_for(config_root, LOGIN_AGENT);
                let (document, _) = crate::accounts::usage_login(&env)?;
                crate::usage_oauth::claude_access_token(&document)
            }
            #[cfg(test)]
            Login::Given(token) => Some(token.clone()),
        }
    }

    /// One request for one value, bounded whole by `deadline`: the text of
    /// the answer's first text block, or the wire's word for why there is
    /// none.
    async fn ask(&self, token: &str, body: Value, deadline: Instant) -> Result<String, String> {
        let client = client().ok_or_else(|| TRANSPORT.to_string())?;
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or_else(|| TIMEOUT.to_string())?;
        let answer = client
            .post(&self.url)
            .timeout(remaining)
            .header("Authorization", format!("Bearer {token}"))
            .header("anthropic-version", ANTHROPIC_WIRE.version)
            .header("anthropic-beta", ANTHROPIC_WIRE.beta)
            .header("Content-Type", "application/json")
            .body(body.to_string())
            .send()
            .await
            .map_err(|err| failure(&err))?;
        let status = answer.status();
        if !status.is_success() {
            return Err(token_for(status.as_u16()));
        }
        let text = answer.text().await.map_err(|err| failure(&err))?;
        let parsed: Value = serde_json::from_str(&text).map_err(|_| SCHEMA.to_string())?;
        parsed
            .get("content")
            .and_then(Value::as_array)
            .and_then(|blocks| {
                blocks
                    .iter()
                    .find(|block| block.get("type").and_then(Value::as_str) == Some("text"))
            })
            .and_then(|block| block.get("text").and_then(Value::as_str))
            .map(str::to_string)
            .ok_or_else(|| SCHEMA.to_string())
    }
}

impl ValueWriter for LiveWriter {
    fn row(&self) -> Option<&'static ValueRow> {
        zerocode_core::type_value::chosen()
    }

    fn write(&mut self, look: &FieldLook<'_>, left: Duration) -> Result<Written, String> {
        let row = self.row().ok_or_else(|| NO_ROW.to_string())?;
        if row.road != Road::Anthropic {
            return Err(ROAD_UNSUPPORTED.to_string());
        }
        if left.is_zero() {
            return Err(TIMEOUT.to_string());
        }
        let began = Instant::now();
        let token = self.token().ok_or_else(|| NO_LOGIN.to_string())?;
        // The table's own question, down the road the probe that measured
        // the table took: the road's identity, then the question's words, as
        // the system; the rendered field as the one user line. A value is a
        // line of at most the question's cap in characters, so no more
        // tokens than that are ever worth waiting for.
        let question = zerocode_core::type_value::asked();
        let body = json!({
            "model": row.model,
            "max_tokens": question.value_char_cap,
            "system": [
                { "type": "text", "text": ANTHROPIC_WIRE.identity },
                { "type": "text", "text": question.instructions },
            ],
            "messages": [
                { "role": "user", "content": zerocode_core::type_value::render(look) },
            ],
        });
        // Every walk drives sync roads from a thread of its own, and blocks
        // on the window's runtime as its judge's wire does.
        let said = tauri::async_runtime::block_on(self.ask(&token, body, began + left))?;
        let value = zerocode_core::type_value::read(&said)
            .map_err(|refusal| refusal.token().to_string())?;
        Ok(Written {
            value,
            model: row.model.clone(),
            ms: u64::try_from(began.elapsed().as_millis()).unwrap_or(u64::MAX),
        })
    }
}

/// The one HTTP client every written value goes through: its pool keeps the
/// endpoint's TLS session between a walk's entries, so the second value
/// rides the first one's socket.
fn client() -> Option<&'static reqwest::Client> {
    static CLIENT: OnceLock<Option<reqwest::Client>> = OnceLock::new();
    CLIENT
        .get_or_init(|| reqwest::Client::builder().build().ok())
        .as_ref()
}

/// The wire's word for a request that never answered: its wall, or anything
/// else on the way (`crate::systemone`'s own two words).
fn failure(err: &reqwest::Error) -> String {
    if err.is_timeout() {
        TIMEOUT.to_string()
    } else {
        TRANSPORT.to_string()
    }
}

#[cfg(test)]
mod tests;
