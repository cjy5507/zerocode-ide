//! zerocode 훅 리포터 — 패인 상태를 IDE 에 알리는 네이티브 POST.
//!
//! claude 는 훅 스크립트(`sh -lc` + curl)로 같은 일을 한다. 우리는 스크립트도
//! 설정 파일도 없이 프로세스 안에서 곧장 보낸다: IDE 가 런치 env 로 준
//! `ZEROCODE_HOOK_PORT/TOKEN/PANE_KEY(+TAB_ID·LAUNCH_TOKEN·WORKTREE_ID·HOOK_ENV·
//! HOOK_VERSION)` 를 읽어 `POST http://127.0.0.1:$PORT/hook/zo` 에
//! `x-zerocode-hook-token` 헤더로 JSON 을 던진다(`zerocode-hookd::receive_hook`
//! 의 `HookJson` 모양). 셋 중 하나라도 없으면 리포터는 비활성 — 맨 터미널에서
//! 돌 때는 아무 데도 두드리지 않는다.
//!
//! 이벤트 철자는 **claude 식**이다(`UserPromptSubmit`·`PreToolUse`·`PostToolUse`·
//! `PermissionRequest`·`Notification`·`Stop`·`SessionStart`·`SessionEnd`) —
//! IDE 의 `hook_state` 리듀서가 그 어휘로 Working/NeedsAttention/Done 을 셈하고,
//! 페이로드 키(`session_id`·`transcript_path`·`prompt`·`tool_name`·
//! `last_assistant_message`·`message`)도 그 파서들이 읽는 이름 그대로다.
//!
//! 주의: `ZEROCODE_PANE_KEY` 는 에이전트가 아닌 맨 셸 판에도 심어진다 — 그래서
//! 이 변수들은 **리포터 활성화**에만 쓰고, 모드 전환 근거로는 절대 쓰지 않는다.

use std::time::Duration;

use serde::Serialize;

pub const ENV_PORT: &str = "ZEROCODE_HOOK_PORT";
pub const ENV_TOKEN: &str = "ZEROCODE_HOOK_TOKEN";
pub const ENV_PANE_KEY: &str = "ZEROCODE_PANE_KEY";
pub const ENV_TAB_ID: &str = "ZEROCODE_TAB_ID";
pub const ENV_LAUNCH_TOKEN: &str = "ZEROCODE_LAUNCH_TOKEN";
pub const ENV_WORKTREE_ID: &str = "ZEROCODE_WORKTREE_ID";
pub const ENV_HOOK_ENV: &str = "ZEROCODE_HOOK_ENV";
pub const ENV_HOOK_VERSION: &str = "ZEROCODE_HOOK_VERSION";
/// Path to the bridge's endpoint file (`endpoint.env`).
///
/// The PTY hands every agent a port at launch, and then the app restarts and
/// that port belongs to nobody. The file is how an agent that outlived the
/// restart finds the bridge listening NOW — the hook scripts the other agents
/// use `source` it before reading their environment, so the file's values win.
///
/// `zo` reports natively instead of through a script, and it read the port
/// exactly once. That made it the ONE agent on this machine that goes
/// permanently silent when the app restarts on a new port: the pane keeps
/// working and the board stops hearing from it, with nothing to say why.
pub const ENV_HOOK_ENDPOINT: &str = "ZEROCODE_HOOK_ENDPOINT";
/// 런타임 계측용 스위치. 창 env 는 그대로 둔 채 리포터가 만드는 스케줄
/// 변화만 빼서 비교할 때 쓴다. 평소에는 없으므로 동작 비용도 변화도 없다.
pub const PROFILE_DISABLE_ENV: &str = "ZO_PROFILE_DISABLE_HOOK_REPORTER";
pub const TOKEN_HEADER: &str = "x-zerocode-hook-token";
const AGENT_SLUG: &str = "zo";
const POST_TIMEOUT: Duration = Duration::from_millis(1500);

/// hookd `HookJson` 과 같은 모양의 봉투. `payload` 는 이벤트별 JSON 객체.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct HookEnvelope {
    pub pane_key: String,
    pub tab_id: String,
    pub launch_token: String,
    pub worktree_id: String,
    pub env: String,
    pub version: String,
    pub hook_event_name: String,
    pub payload: serde_json::Value,
}

/// What to call a helper row on screen.
///
/// The cell's own detail if there is one, and the spawning tool's name only as
/// a last resort. The board keys rows by id and SHOWS this string, so three
/// concurrent children all called "Agent" are three rows a reader cannot tell
/// apart — knowing which one is which is the roster's whole job.
///
/// Kept short: a sidebar row is not a place for a prompt.
fn subagent_display_name(tool_name: &str, detail: &str) -> String {
    let detail = detail.trim();
    if detail.is_empty() {
        return tool_name.to_string();
    }
    let mut name: String = detail.chars().take(48).collect();
    if detail.chars().count() > 48 {
        name.push('\u{2026}');
    }
    name
}

/// One live background helper, as the board's roster reads it.
///
/// The array this becomes is the ONLY road a detached helper's existence
/// travels: a background `Agent` call returns `status: running` and its
/// `SubagentStop` fires when the SPAWN returns, not when the child does.
/// Without this list the window's `done_is_held` has an empty `inventory_busy`
/// and retires the card while children are still working — the limit the
/// `SubagentStart`/`Stop` pair deliberately left open.
///
/// `status` is always `running` because the roster this is built from is
/// already filtered to running manifests: an agent that finished simply stops
/// appearing, and the reader retires a row a COMPLETE list does not name. That
/// makes ABSENCE the signal, which is why this list must never be sent partial.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BackgroundTask {
    pub id: String,
    pub agent_type: String,
    pub description: String,
    /// The pane this helper runs in, when it got one of its own
    /// (`runtime::subagent_panes`). Absent — and absent from the wire — for a
    /// helper that is a thread of the pane already reporting, because a pane
    /// id here is what tells the window there is a second screen to draw.
    pub pane: Option<String>,
    /// What the last `SendMessage` to this helper came to (`consumed` |
    /// `queued` | `rejected`, t-2513 §2.3). Absent — and absent from the
    /// wire — until somebody sends.
    pub last_receipt: Option<&'static str>,
}

impl BackgroundTask {
    fn to_wire(&self) -> serde_json::Value {
        let mut wire = serde_json::json!({
            "type": "subagent",
            "id": self.id,
            "agent_type": self.agent_type,
            "description": self.description,
            "status": "running",
        });
        if let Some(object) = wire.as_object_mut() {
            if let Some(pane) = self.pane.as_deref() {
                object.insert(
                    "pane".to_string(),
                    serde_json::Value::String(pane.to_string()),
                );
            }
            if let Some(receipt) = self.last_receipt {
                object.insert(
                    "last_receipt".to_string(),
                    serde_json::Value::String(receipt.to_string()),
                );
            }
        }
        wire
    }

    fn array(tasks: &[Self]) -> serde_json::Value {
        serde_json::Value::Array(tasks.iter().map(Self::to_wire).collect())
    }
}

/// 한 패인의 리포터. `Clone` 이 싸므로 루프와 턴이 각자 들고 다닌다.
#[derive(Debug, Clone)]
pub struct HookReporter {
    /// Coordinates from the launch environment — the fallback when the
    /// endpoint file is absent, unreadable, or incomplete.
    launch: Coordinates,
    /// Where the bridge publishes its current coordinates, when it does.
    endpoint_path: Option<std::path::PathBuf>,
    /// The model this pane is running, kept current.
    ///
    /// `hook::model_in_payload` reads `model` off ANY event and the board's
    /// `paneModels` keeps it sticky, so the badge worked off `SessionStart`
    /// alone — right up until `/model` switched mid-session and the badge
    /// froze on the model that had been replaced. Carrying it on every event
    /// is what makes the badge follow the switch, and it is why this is a cell
    /// rather than a field: the reporter is cloned into the loop and the turn,
    /// and a switch has to reach both.
    model: std::sync::Arc<std::sync::Mutex<String>>,
    identity: PaneIdentity,
    client: reqwest::Client,
    sink: Sink,
}

/// Where a posted envelope ends up.
#[derive(Debug, Clone)]
enum Sink {
    /// The window's hook bridge, over HTTP — fire and forget.
    Bridge,
    /// A test's own list, written synchronously on the caller's thread, so a
    /// unit test can read the exact hook stream one screen event produced
    /// without standing up a listener or waiting on a spawned task.
    #[cfg(test)]
    Capture(std::sync::Arc<std::sync::Mutex<Vec<HookEnvelope>>>),
}

/// Where a hook POST goes and what proves it may.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Coordinates {
    port: u16,
    token: String,
    env: String,
    version: String,
}

impl Coordinates {
    fn url(&self) -> String {
        format!("http://127.0.0.1:{}/hook/{AGENT_SLUG}", self.port)
    }
}

/// Read the bridge's current coordinates out of the endpoint file.
///
/// The file is `KEY=VALUE` lines written by `zerocode-hookd::endpoint`, whose
/// writer refuses any value that is not shell-safe (letters, digits and
/// `. _ : / -`). Parsed here as plain lines and never executed: a script
/// `source`s it because a script has no other way to read a file, which is not
/// a reason for a process that does.
///
/// `None` unless BOTH a port and a token parse — half an endpoint is worse
/// than none, because it would send this pane's token to a port that did not
/// issue it.
fn read_endpoint_file(path: &std::path::Path) -> Option<Coordinates> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut port = None;
    let mut token = None;
    let mut env = String::new();
    let mut version = String::new();
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            ENV_PORT => port = value.parse::<u16>().ok(),
            ENV_TOKEN if !value.is_empty() => token = Some(value.to_string()),
            ENV_HOOK_ENV => env = value.to_string(),
            ENV_HOOK_VERSION => version = value.to_string(),
            _ => {}
        }
    }
    Some(Coordinates {
        port: port?,
        token: token?,
        env,
        version,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PaneIdentity {
    pane_key: String,
    tab_id: String,
    launch_token: String,
    worktree_id: String,
}

impl HookReporter {
    /// 런치 env 에서 리포터를 만든다. 판 키가 없거나, 좌표(포트·토큰)를 env 에서도
    /// endpoint 파일에서도 못 읽으면 `None`.
    #[must_use]
    pub fn from_env() -> Option<Self> {
        if std::env::var_os(PROFILE_DISABLE_ENV).is_some() {
            return None;
        }
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Option<Self> {
        let pane_key = lookup(ENV_PANE_KEY).filter(|value| !value.trim().is_empty())?;
        let optional = |name: &str| lookup(name).unwrap_or_default();
        let endpoint_path = lookup(ENV_HOOK_ENDPOINT)
            .filter(|value| !value.trim().is_empty())
            .map(std::path::PathBuf::from);
        // The bridge hands a pane its coordinates two ways: port and token in
        // the launch environment, or the endpoint file alone — the window
        // stopped putting the token in every child's environment and gives
        // the file's path instead, which the script-based agents' shims
        // `source`. A reporter that read the environment only came up `None`
        // in such a pane and never posted a hook (2026-09-16: a zo pane whose
        // conversation the window could not read, because it never learned
        // the session). Either place is a launch; a pane with neither has no
        // bridge.
        let from_environment = || {
            let port = lookup(ENV_PORT)?.trim().parse::<u16>().ok()?;
            let token = lookup(ENV_TOKEN).filter(|value| !value.trim().is_empty())?;
            Some(Coordinates {
                port,
                token,
                env: optional(ENV_HOOK_ENV),
                version: optional(ENV_HOOK_VERSION),
            })
        };
        let launch = from_environment()
            .or_else(|| endpoint_path.as_deref().and_then(read_endpoint_file))?;
        let client = reqwest::Client::builder()
            .timeout(POST_TIMEOUT)
            .build()
            .ok()?;
        Some(Self {
            launch,
            endpoint_path,
            model: std::sync::Arc::new(std::sync::Mutex::new(String::new())),
            identity: PaneIdentity {
                pane_key,
                tab_id: optional(ENV_TAB_ID),
                launch_token: optional(ENV_LAUNCH_TOKEN),
                worktree_id: optional(ENV_WORKTREE_ID),
            },
            client,
            sink: Sink::Bridge,
        })
    }

    /// A reporter whose every post lands in the returned list instead of on
    /// the wire. The envelopes are the ones the bridge would have received,
    /// in the order they were posted.
    #[cfg(test)]
    pub(crate) fn capturing() -> (Self, std::sync::Arc<std::sync::Mutex<Vec<HookEnvelope>>>) {
        let lookup = |name: &str| match name {
            ENV_PORT => Some("1".to_string()),
            ENV_TOKEN => Some("capture".to_string()),
            ENV_PANE_KEY => Some("test/pane".to_string()),
            _ => None,
        };
        let mut reporter = Self::from_lookup(lookup).expect("capture reporter");
        let captured = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        reporter.sink = Sink::Capture(std::sync::Arc::clone(&captured));
        (reporter, captured)
    }

    /// The coordinates to use for THIS post.
    ///
    /// Re-read per POST, not cached: the whole point is to notice a bridge that
    /// moved, and a cache is a copy of the thing that went stale. The file is
    /// four short lines and hooks fire a handful of times per turn, so the read
    /// is not on any hot path.
    fn coordinates(&self) -> Coordinates {
        self.endpoint_path
            .as_deref()
            .and_then(read_endpoint_file)
            .unwrap_or_else(|| self.launch.clone())
    }

    #[must_use]
    pub fn envelope(&self, event: &str, payload: serde_json::Value) -> HookEnvelope {
        self.envelope_with(&self.coordinates(), event, payload)
    }

    fn envelope_with(
        &self,
        coordinates: &Coordinates,
        event: &str,
        payload: serde_json::Value,
    ) -> HookEnvelope {
        HookEnvelope {
            pane_key: self.identity.pane_key.clone(),
            tab_id: self.identity.tab_id.clone(),
            launch_token: self.identity.launch_token.clone(),
            worktree_id: self.identity.worktree_id.clone(),
            // `env`/`version` say which app instance these coordinates belong
            // to, so they must travel WITH the coordinates: a dev build and a
            // real one running side by side would otherwise steal each other's
            // hooks after a restart moved one of them.
            env: coordinates.env.clone(),
            version: coordinates.version.clone(),
            hook_event_name: event.to_string(),
            payload: self.with_model(payload),
        }
    }

    /// Stamp the current model onto an event payload.
    ///
    /// Only when the payload is an object that does not already say `model` —
    /// `SessionStart` sets its own, and a non-object payload is a vendor shape
    /// this reporter has no business rewriting.
    fn with_model(&self, payload: serde_json::Value) -> serde_json::Value {
        let model = self
            .model
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if model.is_empty() {
            return payload;
        }
        let serde_json::Value::Object(mut map) = payload else {
            return payload;
        };
        map.entry("model")
            .or_insert_with(|| serde_json::Value::from(model));
        serde_json::Value::Object(map)
    }

    /// Tell the reporter which model this pane is running now.
    ///
    /// Called at session open and again on every `/model` switch. Cheap and
    /// idempotent; the cell is shared with every clone.
    pub fn set_model(&self, model: &str) {
        if let Ok(mut current) = self.model.lock() {
            model.clone_into(&mut current);
        }
    }

    /// 보내고 잊는다 — 실패는 조용히 버린다(IDE 가 죽었다고 턴이 느려지면 안 된다).
    /// tokio 런타임 안에서 부른다.
    pub fn post(&self, event: &str, payload: serde_json::Value) {
        let coordinates = self.coordinates();
        let envelope = self.envelope_with(&coordinates, event, payload);
        if self.capture(&envelope) {
            return;
        }
        let request = self
            .client
            .post(coordinates.url())
            .header(TOKEN_HEADER, &coordinates.token)
            .json(&envelope);
        tokio::spawn(async move {
            let _ = request.send().await;
        });
    }

    /// 종료 직전처럼 런타임이 곧 사라지는 자리에서 쓰는 동기 판.
    pub async fn post_and_wait(&self, event: &str, payload: serde_json::Value) {
        let coordinates = self.coordinates();
        let envelope = self.envelope_with(&coordinates, event, payload);
        if self.capture(&envelope) {
            return;
        }
        let _ = self
            .client
            .post(coordinates.url())
            .header(TOKEN_HEADER, &coordinates.token)
            .json(&envelope)
            .send()
            .await;
    }

    /// True when the envelope was taken by a test capture and must not go
    /// on the wire.
    #[cfg(test)]
    fn capture(&self, envelope: &HookEnvelope) -> bool {
        match &self.sink {
            Sink::Bridge => false,
            Sink::Capture(list) => {
                list.lock().expect("hook capture").push(envelope.clone());
                true
            }
        }
    }

    /// Outside tests the only sink is the bridge: nothing is ever captured.
    #[cfg(not(test))]
    fn capture(&self, _envelope: &HookEnvelope) -> bool {
        match self.sink {
            Sink::Bridge => false,
        }
    }

    // ---- 이벤트별 페이로드 (IDE 파서가 읽는 키 이름 그대로) ----

    pub fn session_start(&self, source: &str, session_id: &str, transcript_path: &str, cwd: &str, model: &str) {
        self.post(
            "SessionStart",
            serde_json::json!({
                "source": source,
                "session_id": session_id,
                "transcript_path": transcript_path,
                "cwd": cwd,
                "model": model,
            }),
        );
    }

    pub fn user_prompt_submit(&self, prompt: &str, session_id: &str) {
        self.post(
            "UserPromptSubmit",
            serde_json::json!({ "prompt": prompt, "session_id": session_id }),
        );
    }

    pub fn autonomy_goal(&self, event: &str, session_id: &str, status: &str) {
        self.post(
            event,
            serde_json::json!({"session_id": session_id, "status": status}),
        );
    }

    pub fn loop_iteration(
        &self,
        session_id: &str,
        loop_id: &str,
        noop: bool,
        status: &str,
    ) {
        self.post(
            "LoopIteration",
            serde_json::json!({
                "session_id": session_id,
                "loop_id": loop_id,
                "outcome": if noop { "noop" } else { "acted" },
                "status": status,
            }),
        );
    }

    /// Tools whose call brackets a sub-agent's life.
    ///
    /// The board draws a helper row per `SubagentStart` and holds the parent's
    /// card while one is open (`done_is_held`'s `roster_busy`). `zo` ships
    /// these three ON by default and reported none of them, so its children
    /// drew zero rows and every `Stop` released the card as if the pane were
    /// alone. Claude Code's background Agent fires no `SubagentStart` at all
    /// (measured live by the window's own hook reader), so this is a surface
    /// `zo` can simply have.
    /// The humanized half of a cell's summary — the part that is not the
    /// tool's own name.
    ///
    /// `preview_summary` renders `"{name} {detail}"`, so the detail is what
    /// remains once the name is taken off. A stop carries no summary, and
    /// passing `""` is correct there: the row it closes is found by id, and
    /// the name it was born with stands.
    #[must_use]
    pub fn detail_of<'a>(tool_name: &str, summary: &'a str) -> &'a str {
        summary
            .strip_prefix(tool_name)
            .unwrap_or(summary)
            .trim_start()
    }

    #[must_use]
    pub fn spawns_subagent(tool_name: &str) -> bool {
        crate::session::plain_session::is_spawn_family_tool(tool_name)
    }

    /// Announce a sub-agent, keyed by the spawning tool call.
    ///
    /// The id is the `tool_call_id` rather than the child's own label because
    /// it is the one string BOTH ends of the pair can see: the label arrives
    /// with the child's result, long after the row had to exist. Keying on it
    /// makes an orphaned row structurally impossible — the row a start opens is
    /// the row its own tool result closes.
    ///
    /// LIMIT, stated because the board will look like it says more than it
    /// does: an `Agent` call is DETACHED by default and returns
    /// `status: running`, so this pair brackets the SPAWN, not the child's
    /// whole life. A blocking spawn (`background: false`) is bracketed exactly.
    /// Representing a detached child's real lifetime needs an id its
    /// completion notification also carries, which it does not today.
    /// `detail` is the humanized one-liner the cell already shows
    /// (`general-purpose · wire the sol harness`). It becomes the row's NAME:
    /// the board reads `subagent_type` for that, and sending the tool's own
    /// name there drew every child as the word "Agent" — a roster of identical
    /// rows that says nothing about which child is which.
    pub fn subagent_start(
        &self,
        tool_call_id: &str,
        tool_name: &str,
        detail: &str,
        session_id: &str,
    ) {
        self.post(
            "SubagentStart",
            serde_json::json!({
                "subagent_type": subagent_display_name(tool_name, detail),
                "subagent_id": tool_call_id,
                "session_id": session_id,
            }),
        );
    }

    pub fn subagent_stop(
        &self,
        tool_call_id: &str,
        tool_name: &str,
        detail: &str,
        session_id: &str,
        background_tasks: &[BackgroundTask],
    ) {
        self.post(
            "SubagentStop",
            serde_json::json!({
                "subagent_type": subagent_display_name(tool_name, detail),
                "subagent_id": tool_call_id,
                "session_id": session_id,
                "background_tasks": BackgroundTask::array(background_tasks),
            }),
        );
    }

    /// One real tool START — exactly one per call the runtime minted, never
    /// a repaint of one.
    ///
    /// The window's reducer reads `PreToolUse` as claude's: "the agent reached
    /// for a tool", which turns the pane `Working` and drops any parked
    /// approval or question. So this road carries tool starts only, keyed by
    /// the call the runtime minted: two `Read`s of one path are two calls and
    /// two hooks, while one call seen twice is one. Elapsed seconds, model
    /// waits, retries and quiet stretches are status FACTS and go out on
    /// `session_status.activity` (`EventsChannel::publish_activity`) — a
    /// per-second `PreToolUse` named `quiet` demoted a pane waiting on a
    /// person to "working" every second and filled the window's activity ring
    /// with a stopwatch (t-2550).
    ///
    /// `tool_input` is what the window's activity row shows as the target and
    /// `activity` is the same compact fact `session_status.activity` carries
    /// at the moment the call began.
    pub fn pre_tool_use(
        &self,
        tool_name: &str,
        tool_input: &str,
        activity: &crate::ide::channel::state::ActivityCard,
        session_id: &str,
    ) {
        self.post(
            "PreToolUse",
            serde_json::json!({
                "tool_name": tool_name,
                "tool_input": tool_input,
                "activity": activity,
                "session_id": session_id,
            }),
        );
    }

    pub fn post_tool_use(&self, tool_name: &str, is_error: bool, session_id: &str) {
        self.post(
            if is_error { "PostToolUseFailure" } else { "PostToolUse" },
            serde_json::json!({ "tool_name": tool_name, "session_id": session_id }),
        );
    }

    pub fn permission_request(&self, tool_name: &str, reasoning: &str, session_id: &str) {
        self.post(
            "PermissionRequest",
            serde_json::json!({
                "tool_name": tool_name,
                "message": format!("Zo needs permission to run `{tool_name}`: {reasoning}"),
                "session_id": session_id,
            }),
        );
    }

    /// `AskUserQuestion` — IDE 는 `Notification` 의 `message` 에서 질문 낱말을
    /// 읽어 `NeedsAttention` 으로 셈한다(`NOTIFICATION_ASKING_WORDS`).
    pub fn question(&self, question: &str, session_id: &str) {
        self.post(
            "Notification",
            serde_json::json!({
                "notification_type": "question",
                "message": format!("question: {question}"),
                "session_id": session_id,
            }),
        );
    }

    /// 턴 종료. 기다린다(`post_and_wait`) — 곧이어 나갈 `SessionEnd` 보다 먼저
    /// 도착해야 IDE 카드가 "끝난 뒤 말했다" 로 어긋나지 않는다.
    pub async fn stop(
        &self,
        last_assistant_message: &str,
        session_id: &str,
        transcript_path: &str,
        background_tasks: &[BackgroundTask],
    ) {
        self.post_and_wait(
            "Stop",
            serde_json::json!({
                "last_assistant_message": last_assistant_message,
                "session_id": session_id,
                "transcript_path": transcript_path,
                "background_tasks": BackgroundTask::array(background_tasks),
            }),
        )
        .await;
    }

    pub async fn session_end(&self, session_id: &str, reason: &str) {
        self.post_and_wait(
            "SessionEnd",
            serde_json::json!({ "session_id": session_id, "reason": reason }),
        )
        .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    fn lookup_from<'a>(map: &'a HashMap<&'a str, &'a str>) -> impl Fn(&str) -> Option<String> + 'a {
        move |name| map.get(name).map(|value| (*value).to_string())
    }

    fn base_env() -> HashMap<&'static str, &'static str> {
        HashMap::from([
            (ENV_PORT, "9001"),
            (ENV_TOKEN, "launch-token"),
            (ENV_PANE_KEY, "term-3"),
            (ENV_HOOK_ENV, "production"),
            (ENV_HOOK_VERSION, "1"),
        ])
    }

    /// The regression this file exists to end: `zo` read the port ONCE, so an
    /// app restart on a new port left this pane posting into a dead socket
    /// forever while every script-based agent moved with the bridge.
    #[test]
    fn a_bridge_that_moved_is_followed_on_the_next_post() {
        let dir = tempfile::tempdir().expect("temp dir");
        let endpoint = dir.path().join("endpoint.env");
        let path = endpoint.display().to_string();
        let mut map = base_env();
        map.insert(ENV_HOOK_ENDPOINT, path.as_str());
        let reporter = HookReporter::from_lookup(lookup_from(&map)).expect("reporter");

        // No file yet: the launch coordinates carry it.
        assert_eq!(reporter.coordinates().port, 9001);
        assert_eq!(reporter.coordinates().token, "launch-token");

        std::fs::write(
            &endpoint,
            format!("{ENV_PORT}=9999\n{ENV_TOKEN}=moved-token\n{ENV_HOOK_ENV}=dev\n{ENV_HOOK_VERSION}=2\n"),
        )
        .expect("write endpoint");
        let moved = reporter.coordinates();
        assert_eq!(moved.port, 9999, "the file's port must win");
        assert_eq!(moved.token, "moved-token");
        assert!(moved.url().contains("9999"));

        // `env`/`version` travel WITH the coordinates: a dev bridge and a real
        // one running side by side must not claim each other's hooks.
        let envelope = reporter.envelope("Stop", serde_json::json!({}));
        assert_eq!(envelope.env, "dev");
        assert_eq!(envelope.version, "2");

        // A later move is followed too — the read is per POST, not cached.
        std::fs::write(&endpoint, format!("{ENV_PORT}=7777\n{ENV_TOKEN}=again\n"))
            .expect("rewrite endpoint");
        assert_eq!(reporter.coordinates().port, 7777);
    }

    /// A pane the window launched with the endpoint file's PATH and no token in
    /// its environment (the window's shims `source` the file) still gets a
    /// reporter — its coordinates are the file's. Without the file, or with
    /// half of one, there is no bridge to post to.
    #[test]
    fn a_launch_without_the_token_in_its_environment_reads_the_endpoint_file() {
        let dir = tempfile::tempdir().expect("temp dir");
        let endpoint = dir.path().join("endpoint.env");
        let path = endpoint.display().to_string();
        std::fs::write(
            &endpoint,
            format!("{ENV_PORT}=64304
{ENV_TOKEN}=file-token
{ENV_HOOK_ENV}=production
{ENV_HOOK_VERSION}=2
"),
        )
        .expect("write endpoint");
        let mut map = base_env();
        map.remove(ENV_TOKEN);
        map.insert(ENV_HOOK_ENDPOINT, path.as_str());
        let reporter = HookReporter::from_lookup(lookup_from(&map)).expect("the file is the launch");
        assert_eq!(reporter.launch.port, 64304);
        assert_eq!(reporter.launch.token, "file-token");
        assert_eq!(reporter.launch.version, "2");
        let envelope = reporter.envelope("SessionStart", serde_json::json!({}));
        assert_eq!((envelope.env.as_str(), envelope.version.as_str()), ("production", "2"));

        // No token anywhere: no reporter.
        let mut bare = base_env();
        bare.remove(ENV_TOKEN);
        assert!(HookReporter::from_lookup(lookup_from(&bare)).is_none());
        // Half a file is no launch either.
        std::fs::write(&endpoint, format!("{ENV_PORT}=64304
")).expect("half");
        assert!(HookReporter::from_lookup(lookup_from(&map)).is_none());
        // And the environment's token still wins when both are given.
        std::fs::write(&endpoint, format!("{ENV_PORT}=1
{ENV_TOKEN}=file-token
")).expect("file");
        let both = HookReporter::from_lookup(lookup_from(&{
            let mut m = base_env();
            m.insert(ENV_HOOK_ENDPOINT, path.as_str());
            m
        }))
        .expect("reporter");
        assert_eq!(both.launch.token, "launch-token");
    }

    /// Half an endpoint is worse than none: sending this pane's token to a port
    /// that never issued it is an authentication failure at best.
    #[test]
    fn an_incomplete_or_unreadable_endpoint_falls_back_to_the_launch_env() {
        let dir = tempfile::tempdir().expect("temp dir");
        let endpoint = dir.path().join("endpoint.env");
        let path = endpoint.display().to_string();
        let mut map = base_env();
        map.insert(ENV_HOOK_ENDPOINT, path.as_str());
        let reporter = HookReporter::from_lookup(lookup_from(&map)).expect("reporter");

        for half in [
            format!("{ENV_PORT}=9999\n"),
            format!("{ENV_TOKEN}=only-a-token\n"),
            "garbage without an equals sign\n".to_string(),
            format!("{ENV_PORT}=not-a-number\n{ENV_TOKEN}=t\n"),
        ] {
            std::fs::write(&endpoint, &half).expect("write endpoint");
            let coordinates = reporter.coordinates();
            assert_eq!(coordinates.port, 9001, "fell back for {half:?}");
            assert_eq!(coordinates.token, "launch-token");
        }
    }

    /// The board draws a helper row per `SubagentStart` and holds the parent's
    /// card while one is open. The pair is keyed on the spawning tool call —
    /// the one id BOTH ends can see — so a row can never be left standing.
    #[test]
    fn a_spawn_brackets_its_subagent_with_a_matched_pair() {
        let map = base_env();
        let reporter = HookReporter::from_lookup(lookup_from(&map)).expect("reporter");

        for name in crate::session::plain_session::SPAWN_FAMILY_TOOLS {
            assert!(HookReporter::spawns_subagent(name), "{name}");
        }
        for name in ["bash", "read_file", "ToolSearch", "CapabilityInvoke"] {
            assert!(!HookReporter::spawns_subagent(name), "{name}");
        }

        // The row's NAME is the cell's own detail, not the tool's name: three
        // concurrent children all reading "Agent" is a roster nobody can use.
        assert_eq!(
            HookReporter::detail_of("Agent", "Agent general-purpose · wire the sol harness"),
            "general-purpose · wire the sol harness"
        );
        assert_eq!(HookReporter::detail_of("Agent", "Agent"), "");
        assert_eq!(super::subagent_display_name("Agent", ""), "Agent");
        assert_eq!(
            super::subagent_display_name("Agent", "general-purpose · wire the sol harness"),
            "general-purpose · wire the sol harness"
        );
        let long = "x".repeat(80);
        let cut = super::subagent_display_name("Agent", &long);
        assert_eq!(cut.chars().count(), 49, "48 chars plus the ellipsis: {cut}");
        assert!(cut.ends_with('…'));

        let start = reporter.envelope(
            "SubagentStart",
            serde_json::json!({"subagent_type": "Agent", "subagent_id": "tu-7"}),
        );
        let stop = reporter.envelope(
            "SubagentStop",
            serde_json::json!({"subagent_type": "Agent", "subagent_id": "tu-7"}),
        );
        assert_eq!(start.hook_event_name, "SubagentStart");
        assert_eq!(stop.hook_event_name, "SubagentStop");
        assert_eq!(
            start.payload["subagent_id"], stop.payload["subagent_id"],
            "a stop must close the row its own start opened"
        );
    }

    /// The roster is the only road a DETACHED helper travels.
    ///
    /// A background `Agent` returns `status: running`, so its `SubagentStop`
    /// closes the spawn and not the child. Without `background_tasks` the
    /// window's `done_is_held` sees an empty inventory and retires the card
    /// while children are still working — which is the hole the tool-call pair
    /// was documented as leaving open.
    #[test]
    fn stop_events_carry_the_live_background_roster() {
        let map = base_env();
        let reporter = HookReporter::from_lookup(lookup_from(&map)).expect("reporter");
        let roster = vec![
            BackgroundTask {
                id: "ag-1".to_string(),
                agent_type: "Explore".to_string(),
                description: "reading the registry".to_string(),
                pane: Some("%4".to_string()),
                last_receipt: Some("queued"),
            },
            BackgroundTask {
                id: "ag-2".to_string(),
                agent_type: "general-purpose".to_string(),
                description: String::new(),
                pane: None,
                last_receipt: None,
            },
        ];
        let wire = BackgroundTask::array(&roster);
        let entries = wire.as_array().expect("array");
        assert_eq!(entries.len(), 2);
        // The reader skips any entry whose `type` is not subagent/teammate and
        // requires an id; a row it cannot parse makes the whole list truncated,
        // which makes absence stop meaning anything.
        for entry in entries {
            assert_eq!(entry["type"], "subagent");
            assert!(entry["id"].as_str().is_some_and(|id| !id.is_empty()));
            assert_eq!(
                entry["status"], "running",
                "the roster is pre-filtered to running manifests"
            );
        }
        // A helper with a pane of its own points at it; one that is a thread
        // of this pane says nothing rather than naming the parent's screen.
        assert_eq!(entries[0]["pane"], "%4");
        assert!(entries[1].get("pane").is_none());
        assert_eq!(entries[0]["last_receipt"], "queued");
        assert!(entries[1].get("last_receipt").is_none(), "an unsent helper carries no receipt");
        assert_eq!(entries[0]["agent_type"], "Explore");

        // An empty roster must still send the FIELD: an absent field means
        // "an older CLI, keep whatever roster you have", while an empty array
        // is the roll call that retires every row.
        let none = BackgroundTask::array(&[]);
        assert_eq!(none, serde_json::json!([]));
        let _ = &reporter;
    }

    /// `hook::model_in_payload` reads `model` off any event and the board keeps
    /// it sticky, so a badge set at `SessionStart` froze when `/model` switched.
    #[test]
    fn every_event_carries_the_current_model() {
        let map = base_env();
        let reporter = HookReporter::from_lookup(lookup_from(&map)).expect("reporter");

        // Before anything is known, nothing is invented.
        assert!(reporter.envelope("Stop", serde_json::json!({})).payload["model"].is_null());

        reporter.set_model("claude-opus-5");
        for event in ["Stop", "PreToolUse", "SubagentStart", "Notification"] {
            assert_eq!(
                reporter.envelope(event, serde_json::json!({}))
                    .payload["model"],
                "claude-opus-5",
                "{event} must carry the model"
            );
        }

        // A switch reaches every clone — the loop and the turn hold their own.
        let clone = reporter.clone();
        reporter.set_model("gpt-5.6-sol");
        assert_eq!(
            clone.envelope("Stop", serde_json::json!({})).payload["model"],
            "gpt-5.6-sol",
            "a clone must see the switch"
        );

        // `SessionStart` sets its own and must not be overwritten.
        let explicit = reporter.envelope("SessionStart", serde_json::json!({"model": "pinned"}));
        assert_eq!(explicit.payload["model"], "pinned");

        // A non-object payload is a vendor shape; leave it alone.
        let text = reporter.envelope("Stop", serde_json::json!("raw"));
        assert_eq!(text.payload, serde_json::json!("raw"));
    }

    #[test]
    fn inactive_without_the_three_required_vars() {
        let mut map = HashMap::new();
        map.insert(ENV_PORT, "4321");
        map.insert(ENV_TOKEN, "t");
        assert!(HookReporter::from_lookup(lookup_from(&map)).is_none());
        map.insert(ENV_PANE_KEY, "1/2");
        assert!(HookReporter::from_lookup(lookup_from(&map)).is_some());
        map.insert(ENV_PORT, "not-a-port");
        assert!(HookReporter::from_lookup(lookup_from(&map)).is_none());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn posts_hookd_shaped_json_with_token_header() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut buffer = vec![0u8; 16 * 1024];
            let read = stream.read(&mut buffer).expect("read");
            let _ = stream.write_all(b"HTTP/1.1 202 Accepted\r\ncontent-length: 0\r\n\r\n");
            String::from_utf8_lossy(&buffer[..read]).into_owned()
        });

        let port_text = port.to_string();
        let mut map = HashMap::new();
        map.insert(ENV_PORT, port_text.as_str());
        map.insert(ENV_TOKEN, "secret");
        map.insert(ENV_PANE_KEY, "tab-1/leaf-2");
        map.insert(ENV_LAUNCH_TOKEN, "launch-9");
        let reporter = HookReporter::from_lookup(lookup_from(&map)).expect("reporter");
        reporter
            .post_and_wait("UserPromptSubmit", serde_json::json!({ "prompt": "hi", "session_id": "s-1" }))
            .await;

        let request = server.join().expect("server thread");
        assert!(request.starts_with("POST /hook/zo HTTP/1.1"), "{request}");
        assert!(request.to_lowercase().contains("x-zerocode-hook-token: secret"), "{request}");
        assert!(request.to_lowercase().contains("content-type: application/json"), "{request}");
        let body = request.split("\r\n\r\n").nth(1).unwrap_or_default();
        let parsed: serde_json::Value = serde_json::from_str(body).expect("json body");
        assert_eq!(parsed["pane_key"], "tab-1/leaf-2");
        assert_eq!(parsed["launch_token"], "launch-9");
        assert_eq!(parsed["hook_event_name"], "UserPromptSubmit");
        assert_eq!(parsed["payload"]["prompt"], "hi");
        assert_eq!(parsed["payload"]["session_id"], "s-1");
    }

    #[test]
    fn a_tool_start_carries_its_name_its_target_and_the_started_fact() {
        let (reporter, captured) = HookReporter::capturing();
        let activity = crate::ide::channel::state::ActivityCard {
            verb: "read".to_string(),
            target: Some("tui/view.rs".to_string()),
            phase: "started".to_string(),
            elapsed_secs: 0,
        };
        reporter.pre_tool_use("Read", "tui/view.rs", &activity, "s-activity");

        let posted = captured.lock().expect("capture");
        assert_eq!(posted.len(), 1);
        let envelope = &posted[0];
        assert_eq!(envelope.hook_event_name, "PreToolUse");
        assert_eq!(envelope.pane_key, "test/pane");
        // The REAL tool name, so the window's `AskUserQuestion` rule and its
        // verb table keep working; the compact target the window's row shows;
        // and the same fact `session_status.activity` says, at second zero.
        assert_eq!(envelope.payload["tool_name"], "Read");
        assert_eq!(envelope.payload["tool_input"], "tui/view.rs");
        assert_eq!(
            envelope.payload["activity"],
            serde_json::json!({
                "verb": "read",
                "target": "tui/view.rs",
                "phase": "started",
                "elapsed_secs": 0
            })
        );
        assert_eq!(envelope.payload["session_id"], "s-activity");
    }

    #[test]
    fn the_capture_reporter_takes_waited_posts_too() {
        let (reporter, captured) = HookReporter::capturing();
        tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("runtime")
            .block_on(reporter.post_and_wait("Stop", serde_json::json!({ "session_id": "s" })));
        reporter.post("SessionEnd", serde_json::json!({ "session_id": "s" }));
        let events: Vec<String> = captured
            .lock()
            .expect("capture")
            .iter()
            .map(|envelope| envelope.hook_event_name.clone())
            .collect();
        assert_eq!(events, ["Stop", "SessionEnd"]);
    }
}
