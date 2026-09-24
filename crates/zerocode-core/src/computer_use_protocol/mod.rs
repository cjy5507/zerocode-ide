//! The Computer Use provider contract, written down once for every platform.
//!
//! `computer_use` (the sibling module) owns the CLI grammar and the agent
//! shim; this module owns what a PROVIDER answers and how it decides. The
//! macOS provider is a Swift helper whose rendering, caching, click-delivery
//! and value-coercion rules were only ever spelled in Swift — so a second
//! platform could agree with it by copying, or drift by forgetting. Every rule
//! here is that Swift rule ported one for one, with the Swift tests ported
//! beside it, so the Windows provider (which is Rust) and any later one read
//! the contract from the same place the tests do.
//!
//! Nothing in here touches an operating system. A tree is walked through a
//! [`render::TreeSource`] the platform supplies, a click is delivered through
//! closures the platform supplies ([`click_plan::deliver`]), and the wire
//! shapes are plain `serde` types whose JSON is byte-compatible with what the
//! macOS helper already emits (`renderSnapshot`, `renderActionResult`,
//! `providerHandshake` in `native/computer-use-macos`).

pub mod cache;
pub mod click_plan;
pub mod coerce;
pub mod eye;
pub mod frame;
pub mod game_state;
pub mod identity;
pub mod keys;
pub mod marks;
pub mod reflex;
pub mod render;
pub mod text_field;
pub mod validate;

use serde::{Deserialize, Serialize};

/// The helper's `TreeRenderer.maxNodes`: past this many indexed elements the
/// snapshot says `truncated` rather than growing without bound.
pub const MAX_TREE_NODES: usize = 1200;
/// `TreeRenderer.maxDepth`.
pub const MAX_TREE_DEPTH: usize = 64;
/// `boundedPngData`: a screenshot whose PNG is larger than this walks down the
/// ladder below until it fits, and the smallest rung is kept if none does.
pub const MAX_SCREENSHOT_PNG_BYTES: usize = 900_000;
/// The first rung of the ladder: the long edge is brought to this many pixels.
pub const SCREENSHOT_RESIZE_START_PX: f64 = 1280.0;
/// Each further rung multiplies the scale by this.
pub const SCREENSHOT_RESIZE_STEP: f64 = 0.85;
/// No further rung brings the long edge below this many pixels — a floor in
/// pixels, so a 6K display walks as many rungs as a laptop does.
pub const SCREENSHOT_RESIZE_FLOOR_PX: f64 = 320.0;

/// The scales a screenshot is encoded at, in order, until one fits the byte
/// budget — exactly as `boundedPngData` loops.
///
/// The first rung brings the long edge to the budget's
/// (`min(1, 1280 / long_edge)`; `1` is the picture as captured) and is always
/// tried, however large the display: a full-resolution encode of a Retina
/// frame is never kept, so it is never paid for. Further rungs multiply by
/// 0.85 while the long edge stays at or above the 320 px floor.
#[must_use]
pub fn screenshot_resize_ladder(width: u32, height: u32) -> Vec<f64> {
    let long_edge = f64::from(width.max(height)).max(1.0);
    let first = (SCREENSHOT_RESIZE_START_PX / long_edge).min(1.0);
    std::iter::successors(Some(first), |scale| {
        Some(scale * SCREENSHOT_RESIZE_STEP)
            .filter(|next| next * long_edge >= SCREENSHOT_RESIZE_FLOOR_PX)
    })
    .collect()
}

/// The envelope a `zerocode-computer` answer carries: the first line that
/// starts with `{` — on stdout when it answered, on stderr when it refused
/// (`{"ok":false,"error":{…}}`). Anything else is the shim's own words.
#[must_use]
pub fn answer_envelope(stdout: &str, stderr: &str) -> Option<serde_json::Value> {
    [stdout, stderr]
        .into_iter()
        .flat_map(str::lines)
        .map(str::trim)
        .find(|line| line.starts_with('{'))
        .and_then(|line| serde_json::from_str(line).ok())
}

/// The code of a refusal as a step's stderr carries it — the envelope's
/// `error.code` — or None for a plain sentence (a refusal without `--json`,
/// the shim's own words) and for an answer that is not a refusal.
#[must_use]
pub fn refusal_code(stderr: &str) -> Option<String> {
    answer_envelope("", stderr)?
        .pointer("/error/code")?
        .as_str()
        .map(str::to_string)
}

/// The error codes a provider may answer with. A skill recovers by code, so
/// the spelling is the contract (`skills/computer-use/SKILL.md` § Recovery).
pub mod error_code {
    /// A mobile mark no longer names the same pressable control on that look.
    pub const PIN_BROKEN: &str = "pin_broken";
    pub const INVALID_ARGUMENT: &str = "invalid_argument";
    pub const APP_NOT_FOUND: &str = "app_not_found";
    pub const APP_BLOCKED: &str = "app_blocked";
    pub const WINDOW_NOT_FOUND: &str = "window_not_found";
    pub const WINDOW_STALE: &str = "window_stale";
    pub const WINDOW_NOT_FOCUSED: &str = "window_not_focused";
    pub const ELEMENT_NOT_FOUND: &str = "element_not_found";
    pub const ELEMENT_NOT_CLICKABLE: &str = "element_not_clickable";
    pub const ACTION_NOT_SUPPORTED: &str = "action_not_supported";
    pub const VALUE_NOT_SETTABLE: &str = "value_not_settable";
    pub const PERMISSION_DENIED: &str = "permission_denied";
    pub const ACCESSIBILITY_ERROR: &str = "accessibility_error";
    pub const ACTION_TIMEOUT: &str = "action_timeout";
    pub const SCREENSHOT_FAILED: &str = "screenshot_failed";
    pub const PROVIDER_INCOMPATIBLE: &str = "provider_incompatible";
    pub const UNSUPPORTED_CAPABILITY: &str = "unsupported_capability";
    /// The operator was stopped — the hotkey, `stop`, or a budget — and
    /// refuses actions until `resume`.
    pub const STOPPED: &str = "stopped";
    /// The session's action budget was reached; the operator stopped itself
    /// and said so.
    pub const BUDGET_EXCEEDED: &str = "budget_exceeded";
    /// A `wait-for` or `sound-wait` saw its budget pass without what it
    /// waited for.
    pub const TIMEOUT: &str = "timeout";
    /// The ears were asked what they heard while nothing was listening
    /// (`listen-start` first), or the listener stopped.
    pub const NOT_LISTENING: &str = "not_listening";
    /// A different request owns a desktop sequence; no stale input is queued.
    pub const SEQUENCE_BUSY: &str = "sequence_busy";
    /// A batch stopped starting steps because the bridge's deadline was near;
    /// its answer says which steps ran.
    pub const BATCH_DEADLINE: &str = "batch_deadline";
    /// The helper found the press lands on a payment, transfer or delete
    /// control (§1.5); the message is `<kind>: <label>` and the window asks.
    pub const CONFIRMATION_REQUIRED: &str = "confirmation_required";
    /// The person said no, or nobody answered within the table.
    pub const CONFIRMATION_REFUSED: &str = "confirmation_refused";
    /// An action came while the person was being asked about another step
    /// (or had the desk): refused, so nothing answers the question for them.
    pub const PERSON_ASKED: &str = "person_asked";
    pub const CONFIRMATION_TIMEOUT: &str = "confirmation_timeout";
    /// A walked step answered ok, but its result says its work did not
    /// happen — a program killed or failed, an app that showed no window, did
    /// not quit or did not come forward; the message says which (§7.2).
    pub const UNFINISHED: &str = "unfinished";
    /// A step answered no envelope — a crash, or a road that answered text.
    pub const UNANSWERED: &str = "error";
    /// A recipe walk stopped before its end: `result.stop` says why (and the
    /// step's own code), `result.next` the step to run again from.
    pub const RECIPE_STOPPED: &str = "recipe_stopped";
    /// Keys would write a secret: the focused field is a password field (or
    /// a focus-moving character carried the keys into one). Nothing — or only
    /// what the message says — was typed; the person types it (`handoff`).
    /// The message is `secret_field: <app>[; typed <i> of <n>]`.
    pub const SECURE_INPUT: &str = "secure_input";
    /// A Flow's recorded fingerprint no longer matches what the run sees
    /// (`computer_flow::Fingerprint::matches`): an app or a host outside the
    /// recorded set, or another protocol. Nothing was pressed; the message
    /// names what is missing.
    pub const FLOW_STALE: &str = "flow_stale";
    /// A Flow's act aimed at an app or a host its fingerprint does not allow
    /// (`Fingerprint::allows`): refused before the press, whatever the screen.
    pub const FLOW_ENV_MISMATCH: &str = "flow_env_mismatch";
    /// A Flow with a money line was asked to walk without its transaction
    /// (docs/design/flow-engine-guarded-money-path.md §1): one of the three
    /// parameters the line names has no value, or the amount is not a plain
    /// number (`computer_flow::Amount`). Nothing moved: the transaction is
    /// the caller's to give, never a screen's to read.
    pub const FLOW_MONEY_UNBOUND: &str = "flow_money_unbound";
    /// At the money step the page did not show the amount and the recipient
    /// the run was given (`guarded::page_agrees`, asked at no wait): the
    /// hand stayed still. The message names what was missing.
    pub const FLOW_PAGE_DISAGREES: &str = "flow_page_disagrees";
    /// The Flow's ledger already holds this transaction acting or acted
    /// (`guarded::Ledger`): it is not moved again, whoever asks.
    pub const FLOW_TXN_SEEN: &str = "flow_txn_seen";
    /// `--confirm` named a transaction other than the one the run was given:
    /// a confirmation is the transaction's own, or it is none.
    pub const FLOW_CONFIRM_UNBOUND: &str = "flow_confirm_unbound";
    /// An arena walk (`recipe-run --arena`) asked for a step the recording
    /// never made — or a check it never asked: the recorded folder answers
    /// only what it holds, in order, and invents no outcome. The message
    /// names the step asked and the line the recording holds instead.
    pub const ARENA_OFF_SCRIPT: &str = "arena_off_script";
}

/// Why an action's outcome is not verified — the words every provider (the
/// macOS helper, the Windows provider, the shared core) reports in an
/// action's `verification.reason`. One table: the Swift helper's literals are
/// pinned against it by a source contract.
pub mod unverified_reason {
    /// The bytes were posted as synthetic input; nothing read them back.
    pub const SYNTHETIC_INPUT: &str = "synthetic_input";
    /// Written, but the field now holds something else (it consumed,
    /// rewrote or reformatted what it was given).
    pub const VALUE_MISMATCH: &str = "value_mismatch";
    /// Written, but the field could not be read back.
    pub const READBACK_UNSUPPORTED: &str = "readback_unsupported";
    /// The write call itself could not confirm it went through.
    pub const WRITE_UNCONFIRMED: &str = "write_unconfirmed";
    /// Typed, and after the settle the field still reads what it read
    /// before: the keys may not have reached it.
    pub const VALUE_UNCHANGED: &str = "value_unchanged";
    /// Written into a password field, which is never read back.
    pub const SECRET_FIELD: &str = "secret_field";
    /// Pasted through the clipboard; the paste is not read back.
    pub const CLIPBOARD_PASTE: &str = "clipboard_paste";
    /// Pasted, and the person's clipboard could not be put back.
    pub const CLIPBOARD_RESTORE_FAILED: &str = "clipboard_restore_failed";
    /// The provider could not say what the action did.
    pub const PROVIDER_UNAVAILABLE: &str = "provider_unavailable";
    /// The window the action named changed under it.
    pub const WINDOW_CHANGED: &str = "window_changed";
    /// Every reason, in one list — the table the Swift literals are held to.
    pub const ALL: &[&str] = &[
        SYNTHETIC_INPUT,
        VALUE_MISMATCH,
        READBACK_UNSUPPORTED,
        WRITE_UNCONFIRMED,
        VALUE_UNCHANGED,
        SECRET_FIELD,
        CLIPBOARD_PASTE,
        CLIPBOARD_RESTORE_FAILED,
        PROVIDER_UNAVAILABLE,
        WINDOW_CHANGED,
    ];
}

/// A provider refusal: a code the skill can act on and a sentence for a person.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderError {
    pub code: String,
    pub message: String,
}

impl ProviderError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }

    pub fn invalid_argument(message: impl Into<String>) -> Self {
        Self::new(error_code::INVALID_ARGUMENT, message)
    }
}

impl std::fmt::Display for ProviderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for ProviderError {}

/// `{name, bundleId, pid}` — the app half of every answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppIdentity {
    pub name: String,
    /// macOS: the bundle identifier. Windows: the AppUserModelId of a
    /// packaged app, `null` for a plain executable — the same honesty macOS
    /// keeps for a process that has no bundle.
    pub bundle_id: Option<String>,
    pub pid: u32,
}

/// One row of `listApps`. `lastUsedAt` and `useCount` are always `null`: the
/// helper renders them so and no provider has a source for them yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListedApp {
    #[serde(flatten)]
    pub identity: AppIdentity,
    pub is_running: bool,
    pub last_used_at: Option<String>,
    pub use_count: Option<u64>,
}

impl ListedApp {
    #[must_use]
    pub fn running(identity: AppIdentity) -> Self {
        Self {
            identity,
            is_running: true,
            last_used_at: None,
            use_count: None,
        }
    }
}

/// A window's geometry in the provider's screen coordinate space (points on
/// macOS, physical pixels on Windows — see the design note) plus the facts a
/// caller picks a target by.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowInfo {
    pub id: u64,
    pub title: String,
    pub x: i64,
    pub y: i64,
    pub width: i64,
    pub height: i64,
    pub is_minimized: bool,
    pub is_offscreen: bool,
    pub screen_index: Option<usize>,
    /// Platform-native facts, never interpreted by the shared layer: macOS
    /// carries `layer` and `alpha`, Windows carries what its enumerator knows.
    pub platform: serde_json::Value,
}

/// One row of `listWindows`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListedWindow {
    pub index: usize,
    pub app: AppIdentity,
    #[serde(flatten)]
    pub window: WindowInfo,
    pub is_main: Option<bool>,
}

/// The `listWindows` answer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WindowListing {
    pub app: ListedApp,
    pub windows: Vec<ListedWindow>,
}

/// The `listApps` answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppListing {
    pub apps: Vec<ListedApp>,
}

/// `snapshot.truncation`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Truncation {
    pub truncated: bool,
    pub max_nodes: usize,
    pub max_depth: usize,
    pub max_depth_reached: bool,
}

/// The `snapshot` object of an observation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotBody {
    pub id: String,
    pub app: AppIdentity,
    pub window: WindowInfo,
    /// Always `"window"`: every coordinate an agent passes is window-local.
    pub coordinate_space: &'static str,
    pub tree_text: String,
    pub element_count: usize,
    pub focused_element_id: Option<usize>,
    pub truncation: Truncation,
    /// Every indexed element with a frame, only when the look asked for them
    /// (`elementFrames`): what a marked look is planned from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub elements: Option<Vec<marks::ElementFace>>,
}

/// The `screenshot` object when a capture succeeded.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScreenshotPayload {
    /// Base64 PNG. The window replaces it with a file path before the agent
    /// sees it (`export_screenshot`), so the wire carries it only once.
    pub data: String,
    pub format: &'static str,
    pub width: u32,
    pub height: u32,
    /// `png_width / window_width`: divide screenshot pixels by this to get
    /// window-local coordinates.
    pub scale: f64,
}

/// `screenshotStatus.metadata`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScreenshotMetadata {
    pub engine: String,
    pub window_id: u64,
}

/// `screenshotStatus`, serialized exactly as `renderScreenshotStatus` does:
/// `{state:"captured", metadata}`, `{state:"skipped", reason:"no_screenshot_flag"}`
/// or `{state:"failed", code:"screenshot_failed", message, metadata}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScreenshotStatus {
    Captured(ScreenshotMetadata),
    Skipped,
    Failed {
        message: String,
        metadata: ScreenshotMetadata,
    },
}

impl Serialize for ScreenshotStatus {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap as _;
        match self {
            Self::Captured(metadata) => {
                let mut map = serializer.serialize_map(Some(2))?;
                map.serialize_entry("state", "captured")?;
                map.serialize_entry("metadata", metadata)?;
                map.end()
            }
            Self::Skipped => {
                let mut map = serializer.serialize_map(Some(2))?;
                map.serialize_entry("state", "skipped")?;
                map.serialize_entry("reason", "no_screenshot_flag")?;
                map.end()
            }
            Self::Failed { message, metadata } => {
                let mut map = serializer.serialize_map(Some(4))?;
                map.serialize_entry("state", "failed")?;
                map.serialize_entry("code", error_code::SCREENSHOT_FAILED)?;
                map.serialize_entry("message", message)?;
                map.serialize_entry("metadata", metadata)?;
                map.end()
            }
        }
    }
}

/// What `getAppState` answers, and what every action answers beneath its
/// `action` object.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Observation {
    pub snapshot: SnapshotBody,
    pub screenshot: Option<ScreenshotPayload>,
    pub screenshot_status: ScreenshotStatus,
}

/// How an action reached the app.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ActionPath {
    Accessibility,
    Synthetic,
    Clipboard,
}

/// `action.verification`: whether the provider read the expected outcome
/// back. The skill reads this apart from transport success.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "state",
    rename_all = "lowercase",
    rename_all_fields = "camelCase"
)]
pub enum Verification {
    Verified {
        property: String,
        expected: Option<String>,
        actual_preview: Option<String>,
    },
    Unverified {
        reason: String,
        expected: Option<String>,
        actual_preview: Option<String>,
    },
}

impl Verification {
    pub fn verified(
        property: impl Into<String>,
        expected: Option<String>,
        actual_preview: Option<String>,
    ) -> Self {
        Self::Verified {
            property: property.into(),
            expected,
            actual_preview,
        }
    }

    pub fn unverified(
        reason: impl Into<String>,
        expected: Option<String>,
        actual_preview: Option<String>,
    ) -> Self {
        Self::Unverified {
            reason: reason.into(),
            expected,
            actual_preview,
        }
    }

    /// The reason every synthetic input carries: the bytes were posted, the
    /// outcome was not read back.
    #[must_use]
    pub fn synthetic_input() -> Self {
        Self::unverified(unverified_reason::SYNTHETIC_INPUT, None, None)
    }
}

/// `action` — `actionMetadata` plus the `targetWindowId` `renderActionResult`
/// stamps on it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionMetadata {
    pub path: ActionPath,
    pub action_name: Option<String>,
    pub fallback_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verification: Option<Verification>,
    pub target_window_id: u64,
}

impl ActionMetadata {
    /// The shape an action returns before the observation that follows it
    /// stamps the window id on.
    #[must_use]
    pub fn new(path: ActionPath) -> Self {
        Self {
            path,
            action_name: None,
            fallback_reason: None,
            verification: None,
            target_window_id: 0,
        }
    }

    #[must_use]
    pub fn named(mut self, action_name: impl Into<String>) -> Self {
        self.action_name = Some(action_name.into());
        self
    }

    #[must_use]
    pub fn fallback(mut self, reason: impl Into<String>) -> Self {
        self.fallback_reason = Some(reason.into());
        self
    }

    #[must_use]
    pub fn verified_as(mut self, verification: Verification) -> Self {
        self.verification = Some(verification);
        self
    }
}

/// An action's answer: the observation after it, plus `action`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ActionResult {
    #[serde(flatten)]
    pub observation: Observation,
    pub action: ActionMetadata,
}

/// `handshake.supports` — what this provider will do, declared truthfully.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SupportMatrix {
    pub apps: AppSupport,
    pub windows: WindowSupport,
    pub observation: ObservationSupport,
    pub actions: ActionSupport,
    pub surfaces: SurfaceSupport,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSupport {
    pub list: bool,
    pub bundle_ids: bool,
    pub pids: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowSupport {
    pub list: bool,
    pub target_by_id: bool,
    pub target_by_index: bool,
    pub focus: bool,
    pub move_resize: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ObservationSupport {
    pub screenshot: bool,
    pub annotated_screenshot: bool,
    pub element_frames: bool,
    pub ocr: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionSupport {
    pub click: bool,
    pub type_text: bool,
    pub press_key: bool,
    pub hotkey: bool,
    pub paste_text: bool,
    pub scroll: bool,
    pub drag: bool,
    pub set_value: bool,
    pub perform_action: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SurfaceSupport {
    pub menus: bool,
    pub dialogs: bool,
    pub dock: bool,
    pub menubar: bool,
}

/// The `handshake` answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderCapabilities {
    pub platform: String,
    pub provider: String,
    pub provider_version: String,
    pub protocol_version: u64,
    pub supports: SupportMatrix,
}

impl SupportMatrix {
    /// What the macOS helper declares (`providerHandshake`). A provider that
    /// implements the same nine actions and the same observation declares
    /// this; one that does not must not.
    #[must_use]
    pub const fn full_desktop() -> Self {
        Self {
            apps: AppSupport {
                list: true,
                bundle_ids: true,
                pids: true,
            },
            windows: WindowSupport {
                list: true,
                target_by_id: true,
                target_by_index: true,
                focus: false,
                move_resize: false,
            },
            observation: ObservationSupport {
                screenshot: true,
                annotated_screenshot: false,
                element_frames: true,
                ocr: false,
            },
            actions: ActionSupport {
                click: true,
                type_text: true,
                press_key: true,
                hotkey: true,
                paste_text: true,
                scroll: true,
                drag: true,
                set_value: true,
                perform_action: true,
            },
            surfaces: SurfaceSupport {
                menus: false,
                dialogs: false,
                dock: false,
                menubar: false,
            },
        }
    }
}

/// Read a JSON number the way the helper's `boundedInteger` does: truncate
/// toward zero, refuse non-finite and out-of-range values instead of
/// trapping on them (`elementIndex: 1e300` used to crash the whole helper).
#[must_use]
pub fn bounded_integer<T: TryFrom<i128>>(value: f64) -> Option<T> {
    if !value.is_finite() {
        return None;
    }
    let truncated = value.trunc();
    // i128 holds every finite f64 with a magnitude below 2^127; anything
    // beyond that cannot fit a narrower integer either.
    if truncated.abs() >= 1.7e38 {
        return None;
    }
    T::try_from(truncated as i128).ok()
}

/// The typed read of a request's `params`, one place per shape so every
/// provider refuses the same malformed request with the same words.
pub mod params {
    use serde_json::{Map, Value};

    use super::{ProviderError, bounded_integer};

    pub fn required_string(
        params: &Map<String, Value>,
        key: &str,
    ) -> Result<String, ProviderError> {
        match params.get(key).and_then(Value::as_str) {
            Some(value) if !value.is_empty() => Ok(value.to_string()),
            _ => Err(ProviderError::invalid_argument(format!("missing {key}"))),
        }
    }

    pub fn required_string_allowing_empty(
        params: &Map<String, Value>,
        key: &str,
    ) -> Result<String, ProviderError> {
        params
            .get(key)
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| ProviderError::invalid_argument(format!("missing {key}")))
    }

    pub fn required_number(params: &Map<String, Value>, key: &str) -> Result<f64, ProviderError> {
        match params.get(key).and_then(Value::as_f64) {
            Some(value) if value.is_finite() => Ok(value),
            _ => Err(ProviderError::invalid_argument(format!("missing {key}"))),
        }
    }

    pub fn required_integer(params: &Map<String, Value>, key: &str) -> Result<i64, ProviderError> {
        bounded_integer(required_number(params, key)?)
            .ok_or_else(|| ProviderError::invalid_argument(format!("{key} is out of range")))
    }

    pub fn optional_integer(
        params: &Map<String, Value>,
        key: &str,
    ) -> Result<Option<i64>, ProviderError> {
        let Some(raw) = params.get(key).and_then(Value::as_f64) else {
            return Ok(None);
        };
        bounded_integer(raw)
            .map(Some)
            .ok_or_else(|| ProviderError::invalid_argument(format!("{key} is out of range")))
    }

    /// A non-negative index (`elementIndex`, `windowIndex`) as `usize`.
    pub fn optional_index(
        params: &Map<String, Value>,
        key: &str,
    ) -> Result<Option<usize>, ProviderError> {
        let Some(raw) = params.get(key).and_then(Value::as_f64) else {
            return Ok(None);
        };
        if raw < 0.0 {
            return Err(ProviderError::invalid_argument(format!(
                "{key} is out of range"
            )));
        }
        bounded_integer::<usize>(raw)
            .map(Some)
            .ok_or_else(|| ProviderError::invalid_argument(format!("{key} is out of range")))
    }

    pub fn required_index(params: &Map<String, Value>, key: &str) -> Result<usize, ProviderError> {
        optional_index(params, key)?
            .ok_or_else(|| ProviderError::invalid_argument(format!("missing {key}")))
    }

    /// `windowId` as the provider's window identity type.
    pub fn optional_window_id(params: &Map<String, Value>) -> Result<Option<u64>, ProviderError> {
        let Some(raw) = params.get("windowId").and_then(Value::as_f64) else {
            return Ok(None);
        };
        if raw < 0.0 {
            return Err(ProviderError::invalid_argument("windowId is out of range"));
        }
        bounded_integer::<u64>(raw)
            .map(Some)
            .ok_or_else(|| ProviderError::invalid_argument("windowId is out of range"))
    }

    pub fn flag(params: &Map<String, Value>, key: &str) -> bool {
        params.get(key).and_then(Value::as_bool) == Some(true)
    }

    pub fn optional_str<'a>(params: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
        params.get(key).and_then(Value::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn identity() -> AppIdentity {
        AppIdentity {
            name: "Notes".into(),
            bundle_id: Some("com.apple.Notes".into()),
            pid: 42,
        }
    }

    fn window() -> WindowInfo {
        WindowInfo {
            id: 7,
            title: "Untitled".into(),
            x: 10,
            y: -20,
            width: 800,
            height: 600,
            is_minimized: false,
            is_offscreen: false,
            screen_index: Some(0),
            platform: json!({ "layer": 0 }),
        }
    }

    /// The wire shape is the helper's, key for key, so an agent reading a
    /// Windows answer sees the same JSON a macOS answer has.
    #[test]
    fn an_observation_serializes_to_the_helpers_shape() {
        let observation = Observation {
            snapshot: SnapshotBody {
                id: "S".into(),
                app: identity(),
                window: window(),
                coordinate_space: "window",
                tree_text: "App=com.apple.Notes (pid 42)".into(),
                element_count: 3,
                focused_element_id: None,
                truncation: Truncation {
                    truncated: false,
                    max_nodes: MAX_TREE_NODES,
                    max_depth: MAX_TREE_DEPTH,
                    max_depth_reached: false,
                },
                elements: None,
            },
            screenshot: None,
            screenshot_status: ScreenshotStatus::Skipped,
        };
        let value = serde_json::to_value(&observation).unwrap();
        assert_eq!(value["snapshot"]["app"]["bundleId"], "com.apple.Notes");
        assert_eq!(value["snapshot"]["window"]["isOffscreen"], false);
        assert_eq!(value["snapshot"]["window"]["platform"]["layer"], 0);
        assert_eq!(value["snapshot"]["coordinateSpace"], "window");
        assert_eq!(value["snapshot"]["focusedElementId"], Value::Null);
        assert_eq!(value["snapshot"]["truncation"]["maxNodes"], 1200);
        assert!(
            value["snapshot"].get(marks::ELEMENTS_KEY).is_none(),
            "no faces unless a look asked for them: the wire stays the helper's"
        );
        assert_eq!(value["screenshot"], Value::Null);
        assert_eq!(
            value["screenshotStatus"],
            json!({ "state": "skipped", "reason": "no_screenshot_flag" })
        );
    }

    #[test]
    fn screenshot_statuses_and_actions_keep_every_helper_key() {
        let metadata = ScreenshotMetadata {
            engine: "printWindow".into(),
            window_id: 7,
        };
        assert_eq!(
            serde_json::to_value(ScreenshotStatus::Captured(metadata.clone())).unwrap(),
            json!({ "state": "captured", "metadata": { "engine": "printWindow", "windowId": 7 } })
        );
        assert_eq!(
            serde_json::to_value(ScreenshotStatus::Failed {
                message: "no image".into(),
                metadata
            })
            .unwrap(),
            json!({
                "state": "failed",
                "code": "screenshot_failed",
                "message": "no image",
                "metadata": { "engine": "printWindow", "windowId": 7 }
            })
        );
        let action = ActionMetadata::new(ActionPath::Accessibility)
            .named("Invoke")
            .verified_as(Verification::verified(
                "value",
                Some("a".into()),
                Some("a".into()),
            ));
        assert_eq!(
            serde_json::to_value(&action).unwrap(),
            json!({
                "path": "accessibility",
                "actionName": "Invoke",
                "fallbackReason": null,
                "verification": { "state": "verified", "property": "value", "expected": "a", "actualPreview": "a" },
                "targetWindowId": 0
            })
        );
        let synthetic = ActionMetadata::new(ActionPath::Synthetic)
            .fallback("actionUnsupported")
            .verified_as(Verification::synthetic_input());
        assert_eq!(
            serde_json::to_value(&synthetic).unwrap()["verification"],
            json!({ "state": "unverified", "reason": "synthetic_input", "expected": null, "actualPreview": null })
        );
        let bare = ActionMetadata::new(ActionPath::Synthetic);
        assert!(
            serde_json::to_value(&bare)
                .unwrap()
                .get("verification")
                .is_none()
        );
    }

    #[test]
    fn listed_apps_and_windows_flatten_like_the_helper() {
        let listed = serde_json::to_value(ListedApp::running(identity())).unwrap();
        assert_eq!(listed["name"], "Notes");
        assert_eq!(listed["isRunning"], true);
        assert_eq!(listed["lastUsedAt"], Value::Null);
        assert_eq!(listed["useCount"], Value::Null);
        let row = serde_json::to_value(ListedWindow {
            index: 0,
            app: identity(),
            window: window(),
            is_main: None,
        })
        .unwrap();
        assert_eq!(row["index"], 0);
        assert_eq!(row["id"], 7);
        assert_eq!(row["app"]["pid"], 42);
        assert_eq!(row["isMain"], Value::Null);
        assert_eq!(row["y"], -20);
    }

    #[test]
    fn the_full_desktop_matrix_is_the_macos_declaration() {
        let value = serde_json::to_value(SupportMatrix::full_desktop()).unwrap();
        assert_eq!(value["apps"]["bundleIds"], true);
        assert_eq!(value["windows"]["targetByIndex"], true);
        assert_eq!(value["windows"]["moveResize"], false);
        assert_eq!(value["observation"]["annotatedScreenshot"], false);
        assert_eq!(value["actions"]["performAction"], true);
        assert_eq!(value["surfaces"]["menubar"], false);
    }

    #[test]
    fn bounded_integers_truncate_and_refuse_like_the_helper() {
        assert_eq!(bounded_integer::<i64>(5.0), Some(5));
        assert_eq!(bounded_integer::<i64>(5.9), Some(5));
        assert_eq!(bounded_integer::<i64>(-5.9), Some(-5));
        assert_eq!(bounded_integer::<i64>(1e300), None);
        assert_eq!(bounded_integer::<i64>(f64::NAN), None);
        assert_eq!(bounded_integer::<i64>(f64::INFINITY), None);
        assert_eq!(bounded_integer::<u32>(4.0), Some(4));
        assert_eq!(bounded_integer::<u32>(-1.0), None);
        assert_eq!(bounded_integer::<i32>(f64::from(i32::MAX)), Some(i32::MAX));
        assert_eq!(bounded_integer::<i32>(f64::from(i32::MAX) + 1.0), None);
    }

    #[test]
    fn params_are_read_with_the_helpers_words() {
        let map = json!({ "app": "Notes", "elementIndex": 1e300, "windowIndex": -1, "x": 4.5 });
        let map = map.as_object().unwrap();
        assert_eq!(params::required_string(map, "app").unwrap(), "Notes");
        assert_eq!(
            params::required_string(map, "text").unwrap_err().message,
            "missing text"
        );
        assert_eq!(
            params::optional_index(map, "elementIndex")
                .unwrap_err()
                .message,
            "elementIndex is out of range"
        );
        assert_eq!(
            params::optional_index(map, "windowIndex")
                .unwrap_err()
                .message,
            "windowIndex is out of range"
        );
        assert_eq!(params::optional_index(map, "absent").unwrap(), None);
        assert_eq!(params::required_number(map, "x").unwrap(), 4.5);
        assert!(!params::flag(map, "noScreenshot"));
    }

    #[test]
    fn the_resize_ladder_starts_at_the_long_edge_and_stops_at_the_floor() {
        let rungs = screenshot_resize_ladder(2560, 1440);
        assert!((rungs[0] - 0.5).abs() < 1e-9);
        assert!((rungs[1] - 0.425).abs() < 1e-9);
        assert!(
            rungs
                .iter()
                .all(|scale| scale * 2560.0 >= SCREENSHOT_RESIZE_FLOOR_PX)
        );
        assert!(
            rungs.last().unwrap() * SCREENSHOT_RESIZE_STEP * 2560.0 < SCREENSHOT_RESIZE_FLOOR_PX
        );
        let small = screenshot_resize_ladder(640, 480);
        assert!(
            (small[0] - 1.0).abs() < 1e-9,
            "a small picture is tried as captured"
        );
        // A 6K display starts at the budget's long edge and walks the same
        // rungs a laptop does — never an empty ladder, never one rung that
        // leaves the byte budget unenforced.
        let six_k = screenshot_resize_ladder(6016, 3384);
        assert!((six_k[0] * 6016.0 - SCREENSHOT_RESIZE_START_PX).abs() < 1e-6);
        assert_eq!(six_k.len(), rungs.len(), "the same rungs, in pixels");
    }
}
