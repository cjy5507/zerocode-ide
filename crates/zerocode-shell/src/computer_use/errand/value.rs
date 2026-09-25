//! The value a walk types (t-6720): written by the value seat's chosen row
//! (`zerocode_core::type_value`) only once the walk's judgment chose to type
//! into a field, typed again from memory when a retry or a replay asks for
//! the very same write, and handed to the world's typing road alone — never
//! to a row, a log, or an argv a log keeps.
//!
//! Jev chooses; it does not write. So the one string a goal walk needs is
//! asked of a small model, and which model, down which road, is the seat's
//! table ([`zerocode_core::type_value::chosen`]) — nothing here spells one.
//! The road is taken only with a key a PERSON put in the window's key store
//! for that row ([`ValueRow::credential_key`], `dev.zerocode.key.<name>`, or
//! the item a router row names). A subscription login is never one: it is
//! the person's to spend in the vendor's own clients, and this product never
//! speaks for a client it is not (the coordinator's decision m-9526). With no
//! key there is no writer ([`ValueWriter::ready`]), the world offers no entry
//! and the walk presses as it always did — nothing falls back to anything.
//!
//! What makes two writes the same write is the core's
//! ([`zerocode_core::type_value::identity`]): the question's version, the
//! model, the goal, the words around the box, what the box holds, the
//! document and the box itself. [`Values`] keeps what was written under that
//! identity, for as many writes as the longest walk the verb allows can make
//! ([`zerocode_core::computer_use::WALK_STEPS_MAX`]); anything else asks
//! afresh.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use zerocode_core::type_value::{ANTHROPIC_WIRE, FieldLook, Road, ValueRow};
use zerocode_harness::SERVICE_KEYCHAIN_SERVICE_PREFIX;

use crate::api_routers::{Keychain, RouterKeys};
use crate::systemone::{SCHEMA, TIMEOUT, TRANSPORT, token_for};

/// Why a writer wrote nothing, beside the wire's own words
/// (`crate::systemone`) and a value the seat's rules refused
/// ([`zerocode_core::type_value::ValueRefusal::token`]): the table names no
/// row; the person put no key in the window's key store for it; its road is
/// one this product does not take (it would have to speak as another client,
/// or it is not built).
pub const NO_ROW: &str = "value_no_row";
pub const NO_KEY: &str = "value_no_key";
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

    /// Whether a person set this writer up: its row's road is one this
    /// product takes, and its key is where the row says a person puts it.
    /// `false`, no question offers an entry and nothing is ever asked.
    fn ready(&self) -> bool;

    /// The value `look` asks for, written within `left` — or the token of
    /// why there is none.
    ///
    /// # Errors
    ///
    /// The token a walk's row names the refusal by.
    fn write(&mut self, look: &FieldLook<'_>, left: Duration) -> Result<Written, String>;

    /// Open the road's connection ahead of the first write, off the
    /// caller's thread — while the judgment that may ask for a value is
    /// still in flight. Nothing is sent but a bare request of the endpoint's
    /// origin, no key with it. A writer with no road to warm does nothing.
    fn warm(&self) {}
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
    #[cfg(test)]
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

/// Where a row's key lives: the item a router row names, else the window's
/// key store under the row's key name. `None` for a row that names neither.
#[must_use]
pub fn key_service(row: &ValueRow) -> Option<String> {
    row.keychain_service.clone().or_else(|| {
        row.credential_key
            .as_ref()
            .map(|name| format!("{SERVICE_KEYCHAIN_SERVICE_PREFIX}{name}"))
    })
}

/// The endpoint a row's road asks, when it is one this product takes: the
/// Messages API for [`Road::Anthropic`], a row's own `/chat/completions` for
/// [`Road::OpenaiCompat`] — unless the row would have the request speak as
/// another client ([`ValueRow::client_fingerprint`]), which this product
/// never does. Code Assist is not built here.
fn endpoint_of(row: &ValueRow) -> Option<String> {
    if row.client_fingerprint.is_some() {
        return None;
    }
    match row.road {
        Road::Anthropic => Some(ANTHROPIC_WIRE.url.to_string()),
        Road::OpenaiCompat => row
            .base_url
            .as_deref()
            .map(|base| format!("{}/chat/completions", base.trim_end_matches('/'))),
        Road::CodeAssist => None,
    }
}

/// The writer that actually asks: the seat's chosen row, down its road, with
/// the key a person put where the row says.
pub struct LiveWriter {
    keys: Box<dyn RouterKeys>,
    /// Where the request goes instead of the row's own endpoint — a test's
    /// loopback.
    endpoint: Option<String>,
    /// The key, read once and only when first needed: a walk that never
    /// meets a field never touches the key store.
    key: OnceLock<Option<String>>,
}

impl LiveWriter {
    /// The window's writer: its key store (`Keychain`), read nothing until a
    /// walk's look has a field to type into.
    #[must_use]
    pub fn window() -> Self {
        Self {
            keys: Box::new(Keychain::of_this_machine()),
            endpoint: None,
            key: OnceLock::new(),
        }
    }

    /// A writer over `keys`, asking `endpoint` — how a test crosses a real
    /// socket with a key store of its own.
    #[cfg(test)]
    #[must_use]
    pub fn at(endpoint: &str, keys: Box<dyn RouterKeys>) -> Self {
        Self {
            keys,
            endpoint: Some(endpoint.to_string()),
            key: OnceLock::new(),
        }
    }

    /// The row's key, read once from where the row says a person puts it.
    fn key(&self) -> Option<&str> {
        self.key
            .get_or_init(|| {
                let service = key_service(self.row()?)?;
                self.keys
                    .read(&service)
                    .ok()
                    .flatten()
                    .map(|key| key.trim().to_string())
                    .filter(|key| !key.is_empty())
            })
            .as_deref()
    }

    /// Where this writer's requests go.
    fn endpoint(&self) -> Option<String> {
        self.endpoint.clone().or_else(|| endpoint_of(self.row()?))
    }

    /// One request for one value, bounded whole by `deadline`: the text the
    /// answer wrote, or the wire's word for why there is none.
    async fn ask(
        &self,
        row: &ValueRow,
        endpoint: &str,
        key: &str,
        look: &FieldLook<'_>,
        deadline: Instant,
    ) -> Result<String, String> {
        let client = client().ok_or_else(|| TRANSPORT.to_string())?;
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or_else(|| TIMEOUT.to_string())?;
        // The table's own question: its instructions as the system, the
        // rendered field as the one user line. A value is a line of at most
        // the question's cap in characters, so no more tokens than that are
        // ever worth waiting for.
        let question = zerocode_core::type_value::asked();
        let said = zerocode_core::type_value::render(look);
        let request = client
            .post(endpoint)
            .timeout(remaining)
            .header("Content-Type", "application/json");
        let request = match row.road {
            Road::Anthropic => request
                .header(ANTHROPIC_WIRE.key_header, key)
                .header("anthropic-version", ANTHROPIC_WIRE.version)
                .body(
                    json!({
                        "model": row.model,
                        "max_tokens": question.value_char_cap,
                        "system": question.instructions,
                        "messages": [{ "role": "user", "content": said }],
                    })
                    .to_string(),
                ),
            Road::OpenaiCompat | Road::CodeAssist => request.bearer_auth(key).body(
                json!({
                    "model": row.model,
                    "max_tokens": question.value_char_cap,
                    "messages": [
                        { "role": "system", "content": question.instructions },
                        { "role": "user", "content": said },
                    ],
                })
                .to_string(),
            ),
        };
        let answer = request.send().await.map_err(|err| failure(&err))?;
        let status = answer.status();
        if !status.is_success() {
            return Err(token_for(status.as_u16()));
        }
        let text = answer.text().await.map_err(|err| failure(&err))?;
        let parsed: Value = serde_json::from_str(&text).map_err(|_| SCHEMA.to_string())?;
        let written = match row.road {
            Road::Anthropic => parsed
                .get("content")
                .and_then(Value::as_array)
                .and_then(|blocks| {
                    blocks
                        .iter()
                        .find(|block| block.get("type").and_then(Value::as_str) == Some("text"))
                        .and_then(|block| block.get("text").and_then(Value::as_str))
                }),
            Road::OpenaiCompat | Road::CodeAssist => parsed
                .pointer("/choices/0/message/content")
                .and_then(Value::as_str),
        };
        written
            .map(str::to_string)
            .ok_or_else(|| SCHEMA.to_string())
    }
}

impl ValueWriter for LiveWriter {
    fn row(&self) -> Option<&'static ValueRow> {
        zerocode_core::type_value::chosen()
    }

    fn ready(&self) -> bool {
        self.row().is_some_and(|row| endpoint_of(row).is_some()) && self.key().is_some()
    }

    /// A first write in a process paid for the client and the handshake:
    /// 895 ms against 657 and 672 for the two behind it on the same endpoint
    /// (2026-09-26). A warm-up that outlives the seat's own wall was no help,
    /// so that wall bounds it. Only a writer a person set up warms anything.
    fn warm(&self) {
        let (true, Some(client), Some(endpoint)) = (self.ready(), client(), self.endpoint()) else {
            return;
        };
        let Ok(url) = url::Url::parse(&endpoint) else {
            return;
        };
        let origin = url.origin().ascii_serialization();
        let wall = Duration::from_millis(zerocode_core::type_value::seat().deadline_ms);
        tauri::async_runtime::spawn(async move {
            let _ = client.get(&origin).timeout(wall).send().await;
        });
    }

    fn write(&mut self, look: &FieldLook<'_>, left: Duration) -> Result<Written, String> {
        let row = self.row().ok_or_else(|| NO_ROW.to_string())?;
        let endpoint = self
            .endpoint()
            .filter(|_| endpoint_of(row).is_some())
            .ok_or_else(|| ROAD_UNSUPPORTED.to_string())?;
        if left.is_zero() {
            return Err(TIMEOUT.to_string());
        }
        let began = Instant::now();
        let key = self.key().ok_or_else(|| NO_KEY.to_string())?.to_string();
        // Every walk drives sync roads from a thread of its own, and blocks
        // on the window's runtime as its judge's wire does.
        let said =
            tauri::async_runtime::block_on(self.ask(row, &endpoint, &key, look, began + left))?;
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
pub(super) mod tests;
