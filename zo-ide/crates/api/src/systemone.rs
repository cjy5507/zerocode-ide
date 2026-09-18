//! TypeSafe System One — a typed judgment endpoint, not a message model.
//!
//! `POST /v1/systemone` takes a `state` (a string, object or array), a `model`
//! and named `questions`, and answers each question inside the answer space
//! the question declared: a choice from its named criteria or a position along
//! its ordered levels, a probability for every one of them, and a confidence.
//! It writes no prose, so it does not fit
//! [`crate::ProviderClient`]'s message contract and is not reached by putting
//! its address into a provider row
//! (`docs/design/jev-decision-shadow-20260917.md` §1, §2.3).
//!
//! This module is the wire and nothing else: which questions to ask and what
//! an answer means belong to `runtime::decision`. What it owns is the
//! transport's honesty — one closed table of failure tokens, retries only
//! where the contract says a retry can help (429 and 529), and a deadline that
//! bounds the whole call, backoff included. Without a key it does not touch the
//! network at all.
//!
//! It sends bytes, not a request: what leaves is what the Jev door
//! (`zerocode_core::jev::door`) cleared — consent, budget, withheld lines and
//! caps — and the one caller that holds such bytes is zo's door
//! (`tools::smart_router::jev_gate`). A typed request is serialized there,
//! after the door, never here.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use futures_util::FutureExt;
use serde::{Deserialize, Serialize};

/// The public endpoint's origin.
pub const SYSTEMONE_BASE_URL: &str = "https://api.typesafe.ai";
/// The judgment route under the origin.
pub const SYSTEMONE_PATH: &str = "/v1/systemone";
/// The model the SDKs call by default.
pub const SYSTEMONE_MODEL: &str = "jev-latest";
/// Where the SDKs read the key from, and so where this client reads it.
pub const SYSTEMONE_API_KEY_ENV: &str = "TYPESAFE_API_KEY";
/// Test/diagnostic override for the origin (`http://host:port`), for the same
/// reason the routing probe has `ZO_PROBE_BASE_URL`: only a test that crosses a
/// real socket catches a wire-shaped failure. The key is still required.
pub const SYSTEMONE_BASE_URL_ENV: &str = "ZO_SYSTEMONE_BASE_URL";
/// The first backoff after a 429 or 529; each later one doubles.
pub const SYSTEMONE_RETRY_BASE_DELAY: Duration = Duration::from_millis(200);
/// How many times one call may be re-sent after a 429 or 529.
pub const SYSTEMONE_MAX_RETRIES: u32 = 3;

// The grid an answer's numbers arrive on is a fact about the WIRE, and it is
// spelled once for both programs in `zerocode_core::jev::ANSWER_STEP` — the
// window's screen judgments derive their tolerances from the same rounding,
// and a fact spelled twice is two facts that can disagree. Its reader here is
// `runtime::memory::rerank`, which already reads that crate.

/// The contract's "overloaded" status. It is not in the HTTP registry, so it
/// has no name there.
const OVERLOADED_STATUS: u16 = 529;

/// The statuses the contract names, and the failure each one is. Any other
/// non-success status is [`SystemOneFailure::Http`].
const NAMED_STATUSES: [(u16, SystemOneFailure); 4] = [
    (reqwest::StatusCode::UNAUTHORIZED.as_u16(), SystemOneFailure::Unauthorized),
    (reqwest::StatusCode::UNPROCESSABLE_ENTITY.as_u16(), SystemOneFailure::InvalidRequest),
    (reqwest::StatusCode::TOO_MANY_REQUESTS.as_u16(), SystemOneFailure::RateLimited),
    (OVERLOADED_STATUS, SystemOneFailure::Overloaded),
];

/// Every way one call can end without an answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemOneFailure {
    /// 401 — the key was refused.
    Unauthorized,
    /// 422 — the request body was refused.
    InvalidRequest,
    /// 429 — the account's rate limit, still standing after the retries.
    RateLimited,
    /// 529 — the service is overloaded, still after the retries.
    Overloaded,
    /// Any other non-success status.
    Http(u16),
    /// The request never completed at the socket.
    Transport,
    /// The deadline passed before an answer arrived.
    Timeout,
    /// An answer arrived that is not the contract's shape.
    Schema,
    /// No key is configured; nothing was sent.
    NoKey,
}

impl SystemOneFailure {
    /// The failure's token — the one table of these words, read verbatim by
    /// every counter and ledger that names a failure. `Http` reads `http` here;
    /// [`Self::ledger_token`] adds its status.
    #[must_use]
    pub const fn token(self) -> &'static str {
        match self {
            Self::Unauthorized => "unauthorized",
            Self::InvalidRequest => "invalid_request",
            Self::RateLimited => "rate_limited",
            Self::Overloaded => "overloaded",
            Self::Http(_) => "http",
            Self::Transport => "transport",
            Self::Timeout => "timeout",
            Self::Schema => "schema",
            Self::NoKey => "no_key",
        }
    }

    /// The token a ledger row keeps: [`Self::token`], with the status after it
    /// for an unnamed one (`http_503`). A counter keeps the bare token, since a
    /// counter's reasons must stay a closed set.
    #[must_use]
    pub fn ledger_token(self) -> String {
        match self {
            Self::Http(status) => format!("{}_{status}", self.token()),
            other => other.token().to_string(),
        }
    }

    fn from_status(status: u16) -> Self {
        NAMED_STATUSES
            .iter()
            .find_map(|(named, failure)| (*named == status).then_some(*failure))
            .unwrap_or(Self::Http(status))
    }

    /// Whether the contract says waiting can help: a rate limit or an
    /// overload. A refused key, a refused body or a malformed answer is the
    /// same answer the second time.
    const fn retryable(self) -> bool {
        matches!(self, Self::RateLimited | Self::Overloaded)
    }
}

/// A question's answer space.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SystemOneQuestionKind {
    Choice,
    Score,
}

/// The answer space a question offers, in the shape the contract writes it.
///
/// Untagged because the wire already tells the two apart: a choice's criteria
/// is an object of named options, a score's is an array whose order IS the
/// level numbering. One field rather than two question types is what lets a
/// request mix both kinds in the single map the endpoint takes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum SystemOneCriteria {
    /// Option name → what it means, or `None` for an option offered by name.
    Named(BTreeMap<String, Option<String>>),
    /// Level descriptions, the low end of the scale first. A level's number is
    /// its place here, counted from zero.
    Ordered(Vec<String>),
}

/// One named question.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SystemOneQuestion {
    #[serde(rename = "type")]
    pub kind: SystemOneQuestionKind,
    pub instructions: String,
    pub criteria: SystemOneCriteria,
}

impl SystemOneQuestion {
    /// A choice question over `criteria`.
    #[must_use]
    pub fn choice<'a>(
        instructions: &str,
        criteria: impl IntoIterator<Item = (&'a str, Option<&'a str>)>,
    ) -> Self {
        Self {
            kind: SystemOneQuestionKind::Choice,
            instructions: instructions.to_string(),
            criteria: SystemOneCriteria::Named(
                criteria
                    .into_iter()
                    .map(|(name, description)| (name.to_string(), description.map(str::to_string)))
                    .collect(),
            ),
        }
    }

    /// A score question over `levels`, the low end of the scale first.
    ///
    /// The contract takes between two and ten levels, and judges each on its
    /// own: the model is shown a level's words and never its number or its
    /// neighbours, so a level has to describe a situation rather than a degree.
    #[must_use]
    pub fn score<'a>(instructions: &str, levels: impl IntoIterator<Item = &'a str>) -> Self {
        Self {
            kind: SystemOneQuestionKind::Score,
            instructions: instructions.to_string(),
            criteria: SystemOneCriteria::Ordered(levels.into_iter().map(str::to_string).collect()),
        }
    }
}

/// One request: borrowed, since the questions are the same for every call and
/// the state is the caller's.
#[derive(Serialize)]
pub struct SystemOneRequest<'a, S: Serialize + ?Sized> {
    pub state: &'a S,
    pub model: &'a str,
    pub questions: &'a BTreeMap<String, SystemOneQuestion>,
}

/// What the contract bills: input tokens (output is free).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct SystemOneUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

/// A response in the contract's shape. The answers stay raw here: what makes
/// an answer valid depends on the question that was asked, which is the
/// caller's.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct SystemOneResponse {
    pub model: String,
    pub answers: BTreeMap<String, serde_json::Value>,
    pub usage: SystemOneUsage,
}

/// A choice question's answer, as the contract shapes one.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct SystemOneChoiceAnswer {
    #[serde(rename = "type")]
    pub kind: SystemOneQuestionKind,
    pub choice: String,
    pub probabilities: BTreeMap<String, f64>,
    pub confidence: f64,
}

/// A score question's answer, as the contract shapes one.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct SystemOneScoreAnswer {
    #[serde(rename = "type")]
    pub kind: SystemOneQuestionKind,
    /// The position along the levels: every level number weighted by its
    /// probability, so it can land between two. Two different spreads can share
    /// one score, which is why [`Self::probabilities`] is read beside it.
    pub score: f64,
    /// Level number → the description the request gave it. A level may be
    /// described by an object or an array, so whatever was sent is kept.
    pub legend: BTreeMap<String, serde_json::Value>,
    /// Level number → probability. The values sum to one.
    pub probabilities: BTreeMap<String, f64>,
    pub confidence: f64,
}

impl SystemOneResponse {
    /// The answer to `question` read as a choice answer: `None` when there is
    /// no answer by that name, `Some(Err(_))` when there is one that is not
    /// choice-shaped.
    #[must_use]
    pub fn choice_answer(
        &self,
        question: &str,
    ) -> Option<Result<SystemOneChoiceAnswer, serde_json::Error>> {
        self.answers.get(question).map(SystemOneChoiceAnswer::deserialize)
    }

    /// The answer to `question` read as a score answer, on the same terms as
    /// [`Self::choice_answer`].
    #[must_use]
    pub fn score_answer(
        &self,
        question: &str,
    ) -> Option<Result<SystemOneScoreAnswer, serde_json::Error>> {
        self.answers.get(question).map(SystemOneScoreAnswer::deserialize)
    }
}

/// How one call went: the answer or the failure, how many times it was
/// re-sent, how long it took end to end, and what it really cost.
#[derive(Debug, Clone, PartialEq)]
pub struct SystemOneCall {
    /// The winning attempt's outcome, and the winner's own re-sends.
    pub outcome: Result<SystemOneResponse, SystemOneFailure>,
    pub retries: u32,
    /// The whole call, first byte to the answer that was used.
    pub elapsed: Duration,
    /// Requests that actually left, across every attempt this call made —
    /// re-sends and a hedge's second copy alike, including one a dropped
    /// loser had already sent.
    ///
    /// The client is the only thing that knows this number, so it says it.
    /// A caller must not derive it from [`Self::retries`]: with a hedge, one
    /// plus the winner's re-sends is no longer what the day was billed for.
    pub requests: u32,
    /// Present only when a second request actually left.
    pub hedge: Option<HedgeRan>,
}

/// A hedge that fired: when its second request left, whether that copy won,
/// and what the losing copy's own latency turned out to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HedgeRan {
    /// How long after the first request the second one was planned to leave,
    /// which is also when it did.
    pub delay: Duration,
    /// Whether the second copy is the one that answered.
    pub won: bool,
    /// The loser's own latency, when it had already answered by the time the
    /// winner was read; `None` when it was dropped still waiting, which is
    /// the ordinary case — nothing waits on a loser.
    ///
    /// Every published hedge rests on the two copies being independent draws,
    /// and the same literature warns they are not when one back-end is slow
    /// for both. Nobody measures it, because nobody writes the loser down.
    /// This is that column: winner and loser latencies side by side, so the
    /// correlation can be read off a real ledger instead of assumed.
    pub loser_ms: Option<u64>,
}

/// One attempt sequence: an outcome, its own re-sends, and its own latency —
/// measured from its own first byte, not the call's, because a hedge's two
/// copies are only comparable that way.
struct Attempted {
    outcome: Result<SystemOneResponse, SystemOneFailure>,
    retries: u32,
    elapsed: Duration,
}

/// A latency as the whole milliseconds a ledger column holds.
fn millis(elapsed: Duration) -> u64 {
    u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
}

/// A System One client bound to one origin and one key.
#[derive(Clone)]
pub struct SystemOneClient {
    http: reqwest::Client,
    endpoint: String,
    api_key: String,
}

impl std::fmt::Debug for SystemOneClient {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SystemOneClient")
            .field("endpoint", &self.endpoint)
            .finish_non_exhaustive()
    }
}

/// Where and with which key this environment says to call System One, read
/// from the environment alone: knowing whether a call could be made builds no
/// client, and an unkeyed environment never gets one.
#[derive(Clone, PartialEq, Eq)]
pub struct SystemOneConfig {
    base_url: String,
    api_key: String,
}

impl std::fmt::Debug for SystemOneConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SystemOneConfig")
            .field("base_url", &self.base_url)
            .finish_non_exhaustive()
    }
}

impl SystemOneConfig {
    /// The key from [`SYSTEMONE_API_KEY_ENV`], and the origin from
    /// [`SYSTEMONE_BASE_URL_ENV`] when set.
    ///
    /// # Errors
    /// [`SystemOneFailure::NoKey`] when no key is configured.
    pub fn from_env() -> Result<Self, SystemOneFailure> {
        let api_key = crate::providers::read_env_non_empty(SYSTEMONE_API_KEY_ENV)
            .ok()
            .flatten()
            .ok_or(SystemOneFailure::NoKey)?;
        let base_url = std::env::var(SYSTEMONE_BASE_URL_ENV)
            .ok()
            .map(|url| url.trim().to_string())
            .filter(|url| !url.is_empty())
            .unwrap_or_else(|| SYSTEMONE_BASE_URL.to_string());
        Ok(Self { base_url, api_key })
    }

    /// The client this configuration names.
    #[must_use]
    pub fn into_client(self) -> SystemOneClient {
        SystemOneClient::new(&self.base_url, self.api_key)
    }
}

impl SystemOneClient {
    /// A client for `base_url` speaking with `api_key`, on the process's shared
    /// connection pool.
    #[must_use]
    pub fn new(base_url: &str, api_key: impl Into<String>) -> Self {
        Self {
            http: crate::providers::shared_http_client(),
            endpoint: format!("{}{SYSTEMONE_PATH}", base_url.trim_end_matches('/')),
            api_key: api_key.into(),
        }
    }

    /// Send a request body the door cleared, re-sending only after a 429 or
    /// 529, never past `deadline`; with `hedge`, send a second copy of the
    /// same bytes once that long has passed with no answer, and use whichever
    /// answer comes first.
    ///
    /// The deadline bounds the whole call — every attempt and every wait. A
    /// backoff that would end past it is not taken: the call ends on the
    /// failure that asked for it, which is the truer account than a timeout
    /// nobody waited for. A retry re-sends these same bytes.
    ///
    /// A hedge is not a retry, and the two are kept apart here because they
    /// answer different questions. A retry waits for a failure and asks again
    /// because the first answer said to; a hedge waits for nothing and asks
    /// again because no answer came, which on this wire is its own condition
    /// — the slowest routing judgment this machine has recorded, 6,798 ms,
    /// carried the smallest body of them all. So each copy is a whole attempt
    /// sequence, retries included, and `hedge` adds at most one such copy —
    /// the rule's own `MAX_ATTEMPTS` is two, and there is no third.
    ///
    /// `None` is the road as it was: one attempt sequence, awaited.
    ///
    /// The loser is dropped where it stands. `reqwest` cancels a request its
    /// future is dropped on, so the wire is released and only what the server
    /// already did is billed — and nothing waits for it, which is the point.
    ///
    /// What finishes first is the call's, a failure included. The two copies
    /// are the same bytes to the same endpoint, so a refusal of one is a
    /// refusal of both, and where it is not — a connection that breaks under
    /// one copy alone — the failure is the answer a single request would have
    /// given anyway. A hedge is never worse than asking once.
    pub async fn decide_body(
        &self,
        body: Vec<u8>,
        deadline: Duration,
        hedge: Option<Duration>,
    ) -> SystemOneCall {
        let opened = Instant::now();
        // Shared, because a loser is dropped without being read: a copy that
        // is thrown away has still spent what it sent, and the count of what
        // left has to say so.
        let requests = AtomicU32::new(0);
        let done = |run: Attempted, ran: Option<HedgeRan>| SystemOneCall {
            outcome: run.outcome,
            retries: run.retries,
            elapsed: opened.elapsed(),
            requests: requests.load(Ordering::Relaxed),
            hedge: ran,
        };
        let first = self.attempts(&body, deadline, opened, &requests);
        tokio::pin!(first);
        let Some(delay) = hedge else {
            return done(first.await, None);
        };
        // An answer before the hedge's moment is one request, exactly as if
        // no hedge had been planned. Biased so that an answer already in hand
        // beats the delay it arrived on: at a tie, no second request leaves.
        if let Some(run) = tokio::select! {
            biased;
            run = first.as_mut() => Some(run),
            () = tokio::time::sleep(delay) => None,
        } {
            return done(run, None);
        }
        let second = self.attempts(&body, deadline, opened, &requests);
        tokio::pin!(second);
        // Whichever answers first is the call's answer. The loser is then
        // read once without waiting: already finished, its latency is worth a
        // column; still in flight, it is dropped here.
        //
        // Biased towards the copy that left first, so that two answers ready
        // in the same breath name a winner the same way every time.
        let (run, loser, won) = tokio::select! {
            biased;
            run = first.as_mut() => (run, second.as_mut().now_or_never(), false),
            run = second.as_mut() => (run, first.as_mut().now_or_never(), true),
        };
        let ran = HedgeRan {
            delay,
            won,
            loser_ms: loser.map(|loser| millis(loser.elapsed)),
        };
        done(run, Some(ran))
    }

    /// One attempt sequence: send `body`, re-send it after a 429 or 529, and
    /// stop at `deadline` measured from `opened` — the whole call's origin, so
    /// a second copy of a judgment is bounded by the same wall as the first
    /// rather than given a fresh one.
    ///
    /// Every request that leaves is counted in `requests` before it is sent,
    /// so the number holds even for a sequence nobody ever reads.
    async fn attempts(
        &self,
        body: &[u8],
        deadline: Duration,
        opened: Instant,
        requests: &AtomicU32,
    ) -> Attempted {
        let began = Instant::now();
        let finish = |outcome, retries| Attempted { outcome, retries, elapsed: began.elapsed() };
        let mut retries = 0;
        loop {
            let Some(remaining) = deadline.checked_sub(opened.elapsed()) else {
                return finish(Err(SystemOneFailure::Timeout), retries);
            };
            requests.fetch_add(1, Ordering::Relaxed);
            let failure = match tokio::time::timeout(remaining, self.send_once(body)).await {
                Err(_) => return finish(Err(SystemOneFailure::Timeout), retries),
                Ok(Ok(response)) => return finish(Ok(response), retries),
                Ok(Err(failure)) => failure,
            };
            let backoff = SYSTEMONE_RETRY_BASE_DELAY.saturating_mul(2u32.saturating_pow(retries));
            if !failure.retryable()
                || retries >= SYSTEMONE_MAX_RETRIES
                || opened.elapsed() + backoff >= deadline
            {
                return finish(Err(failure), retries);
            }
            tokio::time::sleep(backoff).await;
            retries += 1;
        }
    }

    async fn send_once(&self, body: &[u8]) -> Result<SystemOneResponse, SystemOneFailure> {
        let response = self
            .http
            .post(&self.endpoint)
            .bearer_auth(&self.api_key)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body.to_vec())
            .send()
            .await
            .map_err(|_| SystemOneFailure::Transport)?;
        let status = response.status();
        if !status.is_success() {
            return Err(SystemOneFailure::from_status(status.as_u16()));
        }
        let bytes = response.bytes().await.map_err(|_| SystemOneFailure::Transport)?;
        serde_json::from_slice(&bytes).map_err(|_| SystemOneFailure::Schema)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    use super::*;

    /// A deadline far past any scripted delay, for the cases that are not
    /// about time.
    const UNHURRIED: Duration = Duration::from_secs(10);

    /// One scripted reply: a status, a body, and how long to wait first.
    #[derive(Clone)]
    struct Reply {
        status: u16,
        body: String,
        delay: Duration,
    }

    impl Reply {
        fn now(status: u16, body: impl Into<String>) -> Self {
            Self { status, body: body.into(), delay: Duration::ZERO }
        }
    }

    /// What the mock saw of one request.
    #[derive(Debug, Clone)]
    struct Seen {
        head: String,
        body: String,
    }

    /// An HTTP/1.1 endpoint that speaks the documented contract: the n-th
    /// connection gets the n-th scripted reply (the last one repeats), and
    /// every request is recorded — the count is how a test proves "no
    /// request" and "exactly one retry".
    struct MockSystemOne {
        base_url: String,
        seen: Arc<Mutex<Vec<Seen>>>,
    }

    impl MockSystemOne {
        async fn serving(script: Vec<Reply>) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind the mock");
            let base_url = format!("http://{}", listener.local_addr().expect("mock address"));
            let seen = Arc::new(Mutex::new(Vec::new()));
            let recorder = Arc::clone(&seen);
            tokio::spawn(async move {
                let mut served = 0usize;
                while let Ok((stream, _)) = listener.accept().await {
                    let reply = script
                        .get(served)
                        .or_else(|| script.last())
                        .cloned()
                        .expect("a scripted reply");
                    served += 1;
                    tokio::spawn(answer(stream, reply, Arc::clone(&recorder)));
                }
            });
            Self { base_url, seen }
        }

        fn seen(&self) -> Vec<Seen> {
            self.seen.lock().map(|seen| seen.clone()).unwrap_or_default()
        }
    }

    async fn answer(mut stream: TcpStream, reply: Reply, recorder: Arc<Mutex<Vec<Seen>>>) {
        let mut buffer = Vec::new();
        let mut chunk = [0u8; 4096];
        let head_end = loop {
            let Ok(read) = stream.read(&mut chunk).await else { return };
            if read == 0 {
                return;
            }
            buffer.extend_from_slice(&chunk[..read]);
            if let Some(end) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
                break end + 4;
            }
        };
        let head = String::from_utf8_lossy(&buffer[..head_end]).to_string();
        let length = head
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length").then(|| value.trim().parse::<usize>().ok())?
            })
            .unwrap_or(0);
        while buffer.len() < head_end + length {
            let Ok(read) = stream.read(&mut chunk).await else { return };
            if read == 0 {
                break;
            }
            buffer.extend_from_slice(&chunk[..read]);
        }
        let body = String::from_utf8_lossy(&buffer[head_end..]).to_string();
        if let Ok(mut seen) = recorder.lock() {
            seen.push(Seen { head, body });
        }
        tokio::time::sleep(reply.delay).await;
        let response = format!(
            "HTTP/1.1 {} Scripted\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
            reply.status,
            reply.body.len(),
            reply.body
        );
        let _ = stream.write_all(response.as_bytes()).await;
        let _ = stream.shutdown().await;
    }

    fn questions() -> BTreeMap<String, SystemOneQuestion> {
        BTreeMap::from([(
            "risk".to_string(),
            SystemOneQuestion::choice(
                "blast radius of a wrong edit",
                [("low", None), ("high", Some("credentials, deletion, security"))],
            ),
        )])
    }

    /// A success exactly as the contract documents one.
    fn contract_answer() -> String {
        serde_json::json!({
            "model": "jev-latest",
            "answers": {
                "risk": {
                    "type": "choice",
                    "choice": "high",
                    "probabilities": { "low": 0.2, "high": 0.8 },
                    "confidence": 0.61,
                },
            },
            "usage": { "input_tokens": 212, "output_tokens": 0 },
        })
        .to_string()
    }

    async fn ask(mock: &MockSystemOne, deadline: Duration) -> SystemOneCall {
        asking(mock, deadline, None).await
    }

    /// One call to `mock`, hedged as `hedge` says.
    async fn asking(mock: &MockSystemOne, deadline: Duration, hedge: Option<Duration>) -> SystemOneCall {
        let questions = questions();
        let request = SystemOneRequest {
            state: "rotate the deploy credentials",
            model: SYSTEMONE_MODEL,
            questions: &questions,
        };
        SystemOneClient::new(&mock.base_url, "test-key")
            .decide_body(body_of(&request), deadline, hedge)
            .await
    }

    /// A reply `delay` from now.
    fn slow(delay: Duration) -> Reply {
        Reply { status: 200, body: contract_answer(), delay }
    }

    /// A typed request as the bytes the door would hand the wire.
    fn body_of<S: Serialize + ?Sized>(request: &SystemOneRequest<'_, S>) -> Vec<u8> {
        serde_json::to_vec(request).expect("a request serializes")
    }

    #[tokio::test]
    async fn a_documented_success_is_one_request_with_the_contracts_body_and_bearer() {
        let mock = MockSystemOne::serving(vec![Reply::now(200, contract_answer())]).await;
        let call = ask(&mock, UNHURRIED).await;

        let response = call.outcome.expect("the documented success parses");
        assert_eq!(call.retries, 0);
        assert_eq!(call.requests, 1, "one call, one request — the client says so itself");
        assert_eq!(call.hedge, None, "an unhedged call has nothing to say about a second copy");
        assert_eq!(response.model, "jev-latest");
        assert_eq!(response.usage, SystemOneUsage { input_tokens: 212, output_tokens: 0 });
        let risk = response.choice_answer("risk").expect("answered").expect("choice-shaped");
        assert_eq!(risk.kind, SystemOneQuestionKind::Choice);
        assert_eq!(risk.choice, "high");
        assert_eq!(risk.probabilities.get("high"), Some(&0.8));
        assert!(response.choice_answer("intent").is_none());

        let seen = mock.seen();
        assert_eq!(seen.len(), 1, "one call, one request");
        let head = seen[0].head.to_ascii_lowercase();
        assert!(head.starts_with(&format!("post {SYSTEMONE_PATH} http/1.1")), "{head}");
        assert!(head.contains("authorization: bearer test-key"), "{head}");
        assert!(head.contains("content-type: application/json"), "{head}");
        let body: serde_json::Value = serde_json::from_str(&seen[0].body).expect("a JSON body");
        assert_eq!(body["state"], "rotate the deploy credentials");
        assert_eq!(body["model"], SYSTEMONE_MODEL);
        assert_eq!(body["questions"]["risk"]["type"], "choice");
        assert_eq!(body["questions"]["risk"]["criteria"]["low"], serde_json::Value::Null);
        assert_eq!(
            body["questions"]["risk"]["criteria"]["high"],
            "credentials, deletion, security"
        );
    }

    #[test]
    fn a_score_question_writes_its_levels_as_the_array_that_numbers_them() {
        let questions = BTreeMap::from([(
            "relevance".to_string(),
            SystemOneQuestion::score(
                "How much does `notes[0]` help with `request`?",
                ["nothing to do with it", "same background only", "answers it"],
            ),
        )]);

        let body = serde_json::to_value(SystemOneRequest {
            state: "a recalled note",
            model: SYSTEMONE_MODEL,
            questions: &questions,
        })
        .expect("a serialisable request");

        assert_eq!(body["questions"]["relevance"]["type"], "score");
        assert_eq!(
            body["questions"]["relevance"]["criteria"],
            serde_json::json!(["nothing to do with it", "same background only", "answers it"]),
            "a level's number is its place in the array, so the order IS the question"
        );
    }

    #[test]
    fn a_score_answer_reads_back_its_position_its_legend_and_its_spread() {
        let response: SystemOneResponse = serde_json::from_value(serde_json::json!({
            "model": "jev-1.13.0",
            "answers": {
                "relevance": {
                    "type": "score",
                    "score": 1.3,
                    "confidence": 0.54,
                    "legend": {"0": "nothing", "1": "some", "2": "directly"},
                    "probabilities": {"0": 0.0, "1": 0.7, "2": 0.3}
                }
            },
            "usage": {"input_tokens": 332, "output_tokens": 18}
        }))
        .expect("a response in the contract's shape");

        let answer = response
            .score_answer("relevance")
            .expect("an answer by that name")
            .expect("score-shaped");
        assert_eq!(answer.kind, SystemOneQuestionKind::Score);
        assert!((answer.score - 1.3).abs() < f64::EPSILON);
        assert_eq!(answer.probabilities.get("2"), Some(&0.3));
        assert_eq!(answer.legend["1"], "some");
        assert!(response.score_answer("intent").is_none());
        assert!(
            response.choice_answer("relevance").expect("an answer").is_err(),
            "the two answer shapes are told apart by more than their type token"
        );
    }

    #[tokio::test]
    async fn a_refused_key_or_body_is_final_after_one_request() {
        for (status, failure) in [
            (401, SystemOneFailure::Unauthorized),
            (422, SystemOneFailure::InvalidRequest),
            (503, SystemOneFailure::Http(503)),
        ] {
            let mock = MockSystemOne::serving(vec![
                Reply::now(status, "{\"error\":\"refused\"}"),
                Reply::now(200, contract_answer()),
            ])
            .await;
            let call = ask(&mock, UNHURRIED).await;
            assert_eq!(call.outcome, Err(failure), "{status}");
            assert_eq!(call.retries, 0, "{status}");
            assert_eq!(mock.seen().len(), 1, "{status} is not retried");
        }
        assert_eq!(SystemOneFailure::Http(503).ledger_token(), "http_503");
        assert_eq!(SystemOneFailure::Unauthorized.ledger_token(), "unauthorized");
    }

    #[tokio::test]
    async fn a_rate_limit_is_retried_once_and_then_answers() {
        let mock = MockSystemOne::serving(vec![
            Reply::now(429, "{\"error\":\"rate limited\"}"),
            Reply::now(200, contract_answer()),
        ])
        .await;
        let call = ask(&mock, UNHURRIED).await;
        assert!(call.outcome.is_ok(), "{:?}", call.outcome);
        assert_eq!(call.retries, 1);
        assert_eq!(call.requests, 2, "a re-send is a request that left");
        assert_eq!(mock.seen().len(), 2);
        assert!(call.elapsed >= SYSTEMONE_RETRY_BASE_DELAY, "the retry waited: {:?}", call.elapsed);
    }

    #[tokio::test]
    async fn an_overload_that_never_clears_spends_every_retry_and_says_so() {
        let mock = MockSystemOne::serving(vec![Reply::now(529, "{\"error\":\"overloaded\"}")]).await;
        let call = ask(&mock, UNHURRIED).await;
        assert_eq!(call.outcome, Err(SystemOneFailure::Overloaded));
        assert_eq!(call.retries, SYSTEMONE_MAX_RETRIES);
        assert_eq!(call.requests, SYSTEMONE_MAX_RETRIES + 1);
        assert_eq!(mock.seen().len(), usize::try_from(SYSTEMONE_MAX_RETRIES).unwrap() + 1);
        // Exponential: base, 2×base, 4×base, … — the sum of every wait.
        let waited: Duration = (0..SYSTEMONE_MAX_RETRIES)
            .map(|retry| SYSTEMONE_RETRY_BASE_DELAY * 2u32.pow(retry))
            .sum();
        assert!(call.elapsed >= waited, "{:?} < {waited:?}", call.elapsed);
    }

    #[tokio::test]
    async fn a_backoff_never_sleeps_past_the_deadline() {
        let mock = MockSystemOne::serving(vec![Reply::now(529, "{}")]).await;
        // Room for the first retry's wait, not the second's.
        let deadline = SYSTEMONE_RETRY_BASE_DELAY * 2;
        let call = ask(&mock, deadline).await;
        assert_eq!(call.outcome, Err(SystemOneFailure::Overloaded));
        assert_eq!(call.retries, 1);
        assert!(call.elapsed < deadline, "{:?}", call.elapsed);
    }

    #[tokio::test]
    async fn an_answer_slower_than_the_deadline_is_a_timeout() {
        let deadline = Duration::from_millis(300);
        let mock = MockSystemOne::serving(vec![slow(deadline * 5)]).await;
        let call = ask(&mock, deadline).await;
        assert_eq!(call.outcome, Err(SystemOneFailure::Timeout));
        assert!(call.elapsed < deadline * 3, "the wall held: {:?}", call.elapsed);
    }

    #[tokio::test]
    async fn broken_json_and_a_violated_shape_are_schema_failures() {
        let violated = serde_json::json!({
            "model": "jev-latest",
            "answers": ["high"],
            "usage": { "input_tokens": 12, "output_tokens": 0 },
        })
        .to_string();
        let no_usage = serde_json::json!({ "model": "jev-latest", "answers": {} }).to_string();
        for body in ["{\"model\": \"jev-latest\", \"answers\": {", violated.as_str(), no_usage.as_str()] {
            let mock = MockSystemOne::serving(vec![Reply::now(200, body)]).await;
            let call = ask(&mock, UNHURRIED).await;
            assert_eq!(call.outcome, Err(SystemOneFailure::Schema), "{body}");
            assert_eq!(mock.seen().len(), 1, "a malformed answer is not retried: {body}");
        }
    }

    #[tokio::test]
    async fn without_a_key_nothing_is_sent() {
        let mock = MockSystemOne::serving(vec![Reply::now(200, contract_answer())]).await;
        // The environment is process-wide: every read and write of it happens
        // under the crate's lock, and the lock is released before any await.
        let (without, empty, with) = {
            let _env = crate::test_env_lock();
            let previous_key = std::env::var_os(SYSTEMONE_API_KEY_ENV);
            let previous_base = std::env::var_os(SYSTEMONE_BASE_URL_ENV);
            std::env::remove_var(SYSTEMONE_API_KEY_ENV);
            std::env::set_var(SYSTEMONE_BASE_URL_ENV, &mock.base_url);
            let without = SystemOneConfig::from_env();
            std::env::set_var(SYSTEMONE_API_KEY_ENV, "");
            let empty = SystemOneConfig::from_env();
            std::env::set_var(SYSTEMONE_API_KEY_ENV, "test-key");
            let with = SystemOneConfig::from_env();
            match previous_key {
                Some(value) => std::env::set_var(SYSTEMONE_API_KEY_ENV, value),
                None => std::env::remove_var(SYSTEMONE_API_KEY_ENV),
            }
            match previous_base {
                Some(value) => std::env::set_var(SYSTEMONE_BASE_URL_ENV, value),
                None => std::env::remove_var(SYSTEMONE_BASE_URL_ENV),
            }
            (without, empty, with)
        };

        assert_eq!(without.map(|_| ()), Err(SystemOneFailure::NoKey));
        assert_eq!(empty.map(|_| ()), Err(SystemOneFailure::NoKey), "an empty key is no key");
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(mock.seen().len(), 0, "no key, no request");

        // The override moves the address, and only the address.
        let config = with.expect("a key is configured");
        assert!(!format!("{config:?}").contains("test-key"), "the key never prints");
        let client = config.into_client();
        let questions = questions();
        let request = SystemOneRequest { state: "x", model: SYSTEMONE_MODEL, questions: &questions };
        assert!(client.decide_body(body_of(&request), UNHURRIED, None).await.outcome.is_ok());
        assert_eq!(mock.seen().len(), 1);
        assert!(!format!("{client:?}").contains("test-key"), "the key never prints");
    }

    /// A hedge planned but never reached is one request, and says so: an
    /// answer inside the delay ends the call before the second copy's moment.
    #[tokio::test]
    async fn an_answer_before_the_hedges_moment_leaves_once() {
        let hedge = Duration::from_millis(400);
        let mock = MockSystemOne::serving(vec![Reply::now(200, contract_answer())]).await;
        let call = asking(&mock, UNHURRIED, Some(hedge)).await;

        assert!(call.outcome.is_ok(), "{:?}", call.outcome);
        assert_eq!(call.requests, 1, "the second copy never left");
        assert_eq!(call.hedge, None, "nothing to record about a hedge that did not fire");
        assert_eq!(mock.seen().len(), 1);
        assert!(call.elapsed < hedge, "the answer did not wait for the delay: {:?}", call.elapsed);
    }

    /// The bytes a hedge sends are the first request's, to the letter — the
    /// same body, the same bearer, the same route. A second copy of a judgment
    /// is a second copy, not a second question.
    #[tokio::test]
    async fn a_hedge_sends_the_same_bytes_and_the_first_answer_wins() {
        let mock =
            MockSystemOne::serving(vec![slow(Duration::from_millis(300)), slow(Duration::from_secs(3))])
                .await;
        let call = asking(&mock, UNHURRIED, Some(Duration::from_millis(60))).await;

        assert!(call.outcome.is_ok(), "{:?}", call.outcome);
        assert_eq!(call.requests, 2, "the first answer was late, so a second copy left");
        let ran = call.hedge.expect("a hedge fired");
        assert_eq!(ran.delay, Duration::from_millis(60));
        assert!(!ran.won, "the first copy still answered first");
        assert_eq!(ran.loser_ms, None, "a copy still in flight is dropped, not read");

        let seen = mock.seen();
        assert_eq!(seen.len(), 2);
        assert_eq!(seen[0].body, seen[1].body, "the same bytes");
        let heads: Vec<String> = seen.iter().map(|seen| seen.head.to_ascii_lowercase()).collect();
        for head in &heads {
            assert!(head.starts_with(&format!("post {SYSTEMONE_PATH} http/1.1")), "{head}");
            assert!(head.contains("authorization: bearer test-key"), "{head}");
        }
        assert_eq!(call.retries, 0, "a hedge is not a retry");
    }

    /// The second copy wins, and the loser is dropped where it stands: the
    /// call ends on the wall clock long before the first copy's own reply was
    /// due. This is the whole point — 36.4% of this machine's routing
    /// judgments were being thrown away for want of it.
    #[tokio::test]
    async fn a_second_copy_that_wins_does_not_wait_for_the_loser() {
        let unanswerable = Duration::from_secs(5);
        let mock = MockSystemOne::serving(vec![slow(unanswerable), Reply::now(200, contract_answer())])
            .await;
        let hedge = Duration::from_millis(60);
        let call = asking(&mock, UNHURRIED, Some(hedge)).await;

        assert!(call.outcome.is_ok(), "{:?}", call.outcome);
        assert_eq!(call.requests, 2);
        let ran = call.hedge.expect("a hedge fired");
        assert!(ran.won, "the second copy answered first");
        assert!(
            call.elapsed < unanswerable / 2,
            "the loser was dropped, not waited for: {:?} of {unanswerable:?}",
            call.elapsed
        );
        assert!(call.elapsed >= hedge, "the second copy did leave at the delay: {:?}", call.elapsed);
    }

    /// The wall bounds both copies, not each of them: a second copy gets what
    /// is left of the deadline, never a fresh one.
    #[tokio::test]
    async fn both_copies_end_at_the_one_deadline() {
        let deadline = Duration::from_millis(400);
        let mock = MockSystemOne::serving(vec![slow(deadline * 10)]).await;
        let call = asking(&mock, deadline, Some(deadline / 4)).await;

        assert_eq!(call.outcome, Err(SystemOneFailure::Timeout));
        assert_eq!(call.requests, 2, "the second copy left and timed out too");
        let ran = call.hedge.expect("a hedge fired");
        assert!(
            call.elapsed < deadline * 2,
            "one wall for the pair, not one each: {:?}",
            call.elapsed
        );
        // Both copies time out at the same instant, so the loser has often
        // finished by the time the winner is read — which is exactly the
        // column's case: a latency, never a wait.
        if let Some(loser_ms) = ran.loser_ms {
            assert!(loser_ms <= millis(deadline * 2), "the loser's own latency: {loser_ms} ms");
        }
    }

    #[test]
    fn every_failure_has_its_own_token() {
        let failures = [
            SystemOneFailure::Unauthorized,
            SystemOneFailure::InvalidRequest,
            SystemOneFailure::RateLimited,
            SystemOneFailure::Overloaded,
            SystemOneFailure::Http(500),
            SystemOneFailure::Transport,
            SystemOneFailure::Timeout,
            SystemOneFailure::Schema,
            SystemOneFailure::NoKey,
        ];
        let mut tokens: Vec<&str> = failures.iter().map(|failure| failure.token()).collect();
        tokens.sort_unstable();
        tokens.dedup();
        assert_eq!(tokens.len(), failures.len());
        assert_eq!(SystemOneFailure::from_status(529), SystemOneFailure::Overloaded);
        assert_eq!(SystemOneFailure::from_status(418), SystemOneFailure::Http(418));
        assert!(SystemOneFailure::RateLimited.retryable() && SystemOneFailure::Overloaded.retryable());
        assert!(!SystemOneFailure::Unauthorized.retryable() && !SystemOneFailure::Schema.retryable());
    }
}
