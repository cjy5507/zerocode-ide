//! HTTP to one phone's runner. One client per phone, so the connection stays open
//! between calls, and one session, kept until the runner says it is gone.

use std::time::Duration;

use serde_json::{Value, json};
use zerocode_core::agent_emulator::real_phone::{SNAPSHOT_MAX_DEPTH, SOURCE_QUERY};

/// How long a read may take. A locked phone's read took 12 s (2026-11-03) and the
/// lock is asked first; a tree on a busy phone took up to 7.8 s.
const READ_TIMEOUT: Duration = Duration::from_secs(20);
/// A connection that cannot open in this long has lost its tunnel: `/status` over
/// the tunnel answered in 30 ms.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);

/// What a call came to, apart from its answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum CallError {
    /// No answer: the runner or the tunnel is gone.
    Transport(String),
    /// The runner's session is gone; no gesture was made.
    NoSession,
    /// The runner answered with an error.
    Refused { status: u16, word: String },
}

/// A call with no answer, and why, down to the system's own word: 「error sending
/// request」 alone does not say whether the runner refused the connection, reset
/// it or never answered (2026-11-04: three tests fell on it under load, and the
/// log could not tell which).
fn transport(error: reqwest::Error) -> CallError {
    let error = error.without_url();
    let mut why = error.to_string();
    let mut cause = std::error::Error::source(&error);
    while let Some(inner) = cause {
        why.push_str(": ");
        why.push_str(&inner.to_string());
        cause = inner.source();
    }
    CallError::Transport(why)
}

pub(super) struct Wda {
    http: reqwest::Client,
    pub(super) base: Option<String>,
    session: Option<String>,
}

impl Wda {
    pub(super) fn new() -> Result<Self, String> {
        let http = reqwest::Client::builder()
            .no_proxy()
            .tcp_nodelay(true)
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(READ_TIMEOUT)
            .http1_only()
            .pool_max_idle_per_host(1)
            .build()
            .map_err(|error| error.to_string())?;
        Ok(Self {
            http,
            base: None,
            session: None,
        })
    }

    /// The runner or the tunnel moved: the next call asks again from nothing.
    pub(super) fn forget(&mut self) {
        self.base = None;
        self.session = None;
    }

    async fn call(&self, get: bool, path: &str, body: Option<&Value>) -> Result<Value, CallError> {
        let base = self
            .base
            .as_deref()
            .ok_or_else(|| CallError::Transport("no address".to_string()))?;
        let url = format!("{base}{path}");
        let request = if get {
            self.http.get(url)
        } else {
            self.http.post(url).json(body.unwrap_or(&json!({})))
        };
        let answer = request.send().await.map_err(transport)?;
        let status = answer.status().as_u16();
        let body = answer.bytes().await.map_err(transport)?;
        let parsed: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
        if (200..300).contains(&status) {
            return Ok(parsed.get("value").cloned().unwrap_or(Value::Null));
        }
        let word = parsed
            .pointer("/value/error")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if word == "invalid session id" {
            return Err(CallError::NoSession);
        }
        Err(CallError::Refused { status, word })
    }

    /// `/wda/locked`: the first question of every verb, about 30 ms, and the one
    /// that shows the runner is there at all. An answer that is not true or false
    /// is refused rather than read as unlocked (검수 t-91811 P2-8).
    pub(super) async fn locked(&self) -> Result<bool, CallError> {
        let value = self.call(true, "/wda/locked", None).await?;
        value.as_bool().ok_or_else(|| CallError::Refused {
            status: 200,
            word: "the lock answer was neither true nor false".to_string(),
        })
    }

    async fn open_session(&mut self) -> Result<String, CallError> {
        let answer = self
            .http_session(&json!({ "capabilities": { "alwaysMatch": {
                "shouldWaitForQuiescence": false,
            }}}))
            .await?;
        let session = answer.ok_or_else(|| CallError::Refused {
            status: 200,
            word: "no session in the answer".to_string(),
        })?;
        // A wait for an idle app is what a press pays seconds for.
        self.call(
            false,
            &format!("/session/{session}/appium/settings"),
            Some(&json!({ "settings": {
                "waitForIdleTimeout": 0,
                "animationCoolOffTimeout": 0,
                "snapshotMaxDepth": SNAPSHOT_MAX_DEPTH,
            }})),
        )
        .await?;
        self.session = Some(session.clone());
        Ok(session)
    }

    async fn http_session(&self, capabilities: &Value) -> Result<Option<String>, CallError> {
        let base = self
            .base
            .as_deref()
            .ok_or_else(|| CallError::Transport("no address".to_string()))?;
        let answer = self
            .http
            .post(format!("{base}/session"))
            .json(capabilities)
            .send()
            .await
            .map_err(transport)?;
        let status = answer.status().as_u16();
        let parsed: Value = answer.json().await.map_err(transport)?;
        if !(200..300).contains(&status) {
            return Err(CallError::Refused {
                status,
                word: "the session would not open".to_string(),
            });
        }
        Ok(parsed
            .get("sessionId")
            .and_then(Value::as_str)
            .or_else(|| parsed.pointer("/value/sessionId").and_then(Value::as_str))
            .map(str::to_string))
    }

    /// A call under the session, opened now if none stands; a session the runner
    /// no longer knows (it answers before anything happens on the phone) is opened
    /// again, and the call made once more.
    async fn in_session(
        &mut self,
        get: bool,
        path: &str,
        body: Option<&Value>,
    ) -> Result<Value, CallError> {
        for _ in 0..2 {
            let session = match self.session.clone() {
                Some(session) => session,
                None => self.open_session().await?,
            };
            match self
                .call(get, &format!("/session/{session}{path}"), body)
                .await
            {
                Err(CallError::NoSession) => self.session = None,
                other => return other,
            }
        }
        Err(CallError::NoSession)
    }

    pub(super) async fn active_app(&mut self) -> Result<Option<String>, CallError> {
        self.in_session(true, "/wda/activeAppInfo", None)
            .await
            .map(|value| {
                value
                    .get("bundleId")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
    }

    pub(super) async fn source(&mut self) -> Result<Value, CallError> {
        self.in_session(true, &format!("/source?{SOURCE_QUERY}"), None)
            .await
    }

    pub(super) async fn window_size(&mut self) -> Result<(f64, f64), CallError> {
        let value = self.in_session(true, "/window/size", None).await?;
        let side = |key: &str| value.get(key).and_then(Value::as_f64).unwrap_or_default();
        Ok((side("width"), side("height")))
    }

    pub(super) async fn actions(&mut self, gesture: &Value) -> Result<(), CallError> {
        self.in_session(false, "/actions", Some(gesture))
            .await
            .map(drop)
    }

    /// The type of the element the keyboard writes into, or `None` when nothing
    /// has the focus.
    pub(super) async fn focused_type(&mut self) -> Result<Option<String>, CallError> {
        let active = match self.in_session(true, "/element/active", None).await {
            Ok(active) => active,
            Err(CallError::Refused { status: 404, .. }) => return Ok(None),
            Err(other) => return Err(other),
        };
        let Some(id) = active.get("ELEMENT").and_then(Value::as_str) else {
            return Ok(None);
        };
        let name = self
            .in_session(true, &format!("/element/{id}/name"), None)
            .await?;
        Ok(name.as_str().map(str::to_string))
    }

    /// The runner's own text input: whole characters, Korean included (a Korean
    /// sentence of 22 characters took 604 ms on a simulator and came out whole),
    /// with no keyboard map, no Mac key and no clipboard.
    pub(super) async fn keys(&mut self, text: &str) -> Result<(), CallError> {
        let characters: Vec<String> = text.chars().map(String::from).collect();
        self.in_session(false, "/wda/keys", Some(&json!({ "value": characters })))
            .await
            .map(drop)
    }

    pub(super) async fn home(&self) -> Result<(), CallError> {
        self.call(false, "/wda/homescreen", None).await.map(drop)
    }

    /// PNG bytes of the screen.
    pub(super) async fn screenshot(&self) -> Result<Vec<u8>, CallError> {
        use base64::Engine as _;
        let value = self.call(true, "/screenshot", None).await?;
        let text = value.as_str().unwrap_or_default();
        base64::engine::general_purpose::STANDARD
            .decode(text.replace(['\n', '\r'], ""))
            .map_err(|_| CallError::Refused {
                status: 200,
                word: "the screenshot is not base64".to_string(),
            })
    }
}
