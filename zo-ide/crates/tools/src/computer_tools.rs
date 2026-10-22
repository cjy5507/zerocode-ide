//! `Computer` — zo's first-class desktop operator (docs/design/
//! computer-use-full-operator.md §3).
//!
//! The action vocabulary is Anthropic's `computer_20250124` (`screenshot`,
//! `mouse_move`, the clicks, `left_click_drag`, `scroll`, `key`, `type`,
//! `hold_key`, `cursor_position`, `zoom`, `wait`) plus this window's meaning
//! layer (`launch`, `open`, `find`, `wait_for`, `read`, `window`,
//! `list_windows`, `run`, `stop`, `resume`, `status`, `evidence`). Every
//! action becomes one `zerocode-computer` command
//! line — the pane's own shim, which carries the person's token to the
//! window — and the window judges everything that matters: budgets, the
//! stop, the last step on money and deletion, the evidence. There is no
//! second road: without the shim there is no desktop.
//!
//! A screenshot comes back as an image block (staged through the same guard
//! `read_image` uses), and after an action that changes the screen the tool
//! looks once on its own, so the model always sees what it did — or is told
//! plainly that nothing changed, without a second copy of the same picture.
//!
//! The model speaks in the pixels of the last picture it was shown; the hand
//! moves in screen points. The tool keeps that picture's frame and translates
//! both ways, so a click lands where the model looked and every position the
//! tool answers is in the pixels the model sees.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use runtime::permission_enforcer::PermissionEnforcer;
use runtime::PermissionMode;
use runtime::ToolExecutionResult;
use serde::Deserialize;
use serde_json::{json, Value};
use zerocode_core::computer_use::{
    computer_deadline_ms, walk_steps, walk_words, ComputerMethod, BATCH_COMMANDS_FLAG, COMPUTER_BATCH_MAX_STEPS,
    COMPUTER_BRIDGE_GRACE_MS, COMPUTER_MARKS_ENV, WALK_OVERLAP_FLAG, WALK_REPLAY_FLAG, WALK_RESCUE_FLAG, WALK_STEPS_MAX,
    WATCH_UNTIL,
};
use zerocode_core::agent_browser::{BROWSER_CLI, WORKSPACE_REVISION_FLAG, WORKSPACE_REVISION_KEY};
use zerocode_core::jev::{JevMode, GOAL_CHAR_CAP};
use zerocode_core::surface_execution::SurfaceExecution;
use zerocode_core::screen_action::GIVE_UP;
use zerocode_core::computer_use_protocol::error_code;
use zerocode_core::computer_use_protocol::frame::ShotFrame;
use zerocode_core::computer_use_protocol::marks::{legend_line, ITEMS_KEY, LEGEND_KEY, LOOK_ID_KEY, TOOL_ONLY_KEYS};

use super::{
    from_value, maybe_enforce_permission_check, to_pretty_json, ToolContext, ToolError, ToolSpec,
};
use crate::context::{ComputerObservation, ComputerTarget};

/// The pane's shim — the one road to the window, named as the core names it.
pub const COMPUTER_SHIM: &str = zerocode_core::computer_use::COMPUTER_CLI;
/// Whether an action that changes the screen is followed by a look.
pub(crate) const LOOK_AFTER_ACT: bool = true;
/// Ordinary UI actions wait for quiet unless the caller requests an immediate
/// observation of continuous motion. Both action roads read this default.
const SETTLE_AFTER_ACT: bool = true;
/// The look after an act: the frame `screenshot` answers, without the
/// evidence a `screenshot` step leaves, and with what changed since this
/// model's last look — so an act that changed nothing costs no second picture.
/// The window first waits for what the act did to finish painting.
const LOOK_DIFF_FLAG: &str = "--diff";
const LOOK_SETTLE_FLAG: &str = "--settle";
const LOOK_AFTER_ARGV: &[&str] = &["observe", LOOK_DIFF_FLAG, LOOK_SETTLE_FLAG];
/// The look that learns the picture's frame when the model acts before this
/// process has shown it one.
const FRAME_LOOK_ARGV: &[&str] = &["observe"];
/// What a look adds to number the controls it shows, when this process's
/// looks carry marks.
const MARKS_FLAG: &str = "--marks";
/// The clicks a mark can stand in for, and the command a click by mark is.
const MARK_CLICKS: &[&str] = &["left_click", "right_click", "middle_click", "double_click", "triple_click"];
/// The click an element of an app's tree is pressed by: the element's own
/// press, which has no button or count.
const ELEMENT_CLICK: &str = "left_click";
/// What a click re-picked after `element_not_found` compared to hold to the
/// model's document, as its answer says (t-68358): the title and the
/// document's identity, or the title alone when the window gives none.
const REPICK_COMPARED_DOCUMENT: &str = "title and document";
const REPICK_COMPARED_TITLE: &str = "title only";
/// Why a re-pick kept the window's refusal, by name (t-70459): the window
/// shows another document than the model's last look at it did, or one of
/// the two gave the document's identity and the other did not — a web area
/// that has no address yet — so the document is not confirmed. Both lead
/// to the same next step: observe the window again.
const REPICK_DOCUMENT_CHANGED: &str = "document_changed";
const REPICK_DOCUMENT_UNCONFIRMED: &str = "document_unconfirmed";
/// The action that leaves no window of its app to look at after it.
const QUIT_ACTION: &str = "quit";
/// Actions whose answer names places on the screen: before the first of them
/// the model is shown a picture, so what they answer is in its pixels.
const ANSWERS_PLACES: &[&str] = &["find", "read", "wait_for", LIST_WINDOWS_ACTION, "cursor_position"];
/// The model's own looks: their picture is the one it speaks in next, and
/// what changed is measured against its own last look.
const LOOKS: &[&str] = &["screenshot", "observe"];
/// The frame's own words in an answer — the tool's to read, not the model's:
/// the model speaks in the picture's pixels and never does this arithmetic.
const FRAME_KEYS: &[&str] = &["scale", "origin"];
/// How the tool spells a scroll click for the CLI's line count.
pub(crate) const SCROLL_LINES_PER_CLICK: i64 = 3;
/// The default read when `evidence` is asked without a count.
const EVIDENCE_STEPS: u64 = 20;
/// How often the tool checks that a shim which closed its stdout has also
/// exited — the moment between the two, never the wait for the answer.
const EXIT_POLL: Duration = Duration::from_millis(1);

/// The one action whose steps are actions — named as the core names it.
const BATCH_ACTION: &str = ComputerMethod::Batch.verb_name();
/// The action whose `app` is a filter on the answer, not a target: the CLI
/// lists the whole desktop and the tool keeps the named app's windows.
const LIST_WINDOWS_ACTION: &str = "list_windows";
/// The one action the window walks toward a goal on its own judgment — the
/// core's verb, the loop the window's (`run_goal`); the tool never loops.
const WALK_ACTION: &str = ComputerMethod::Walk.verb_name();
/// Anthropic's actions, verbatim, and this window's extensions — each with the
/// core method it runs (`window` runs the `window-<kind>` its `kind` names; the
/// table holds one of that family). What acts, what presses and what may be a
/// batch step are the core's tables, read through [`methods`], never restated.
pub(crate) const ACTIONS: &[(&str, ComputerMethod)] = &[
    ("screenshot", ComputerMethod::Screenshot),
    ("observe", ComputerMethod::Observe),
    ("zoom", ComputerMethod::Zoom),
    ("mouse_move", ComputerMethod::MouseMove),
    ("left_click", ComputerMethod::MouseClick),
    ("right_click", ComputerMethod::MouseClick),
    ("middle_click", ComputerMethod::MouseClick),
    ("double_click", ComputerMethod::MouseClick),
    ("triple_click", ComputerMethod::MouseClick),
    ("left_click_drag", ComputerMethod::MouseDrag),
    ("scroll", ComputerMethod::MouseScroll),
    ("key", ComputerMethod::Key),
    ("type", ComputerMethod::Type),
    ("hold_key", ComputerMethod::HoldKey),
    ("wait", ComputerMethod::Wait),
    ("cursor_position", ComputerMethod::CursorPosition),
    ("launch", ComputerMethod::Launch),
    ("quit", ComputerMethod::Quit),
    ("activate", ComputerMethod::Activate),
    ("open", ComputerMethod::Open),
    ("run", ComputerMethod::Run),
    ("find", ComputerMethod::Find),
    ("wait_for", ComputerMethod::WaitFor),
    ("read", ComputerMethod::Read),
    ("window", ComputerMethod::WindowFocus),
    ("list_windows", ComputerMethod::ListAllWindows),
    ("clipboard_read", ComputerMethod::ClipboardRead),
    ("clipboard_write", ComputerMethod::ClipboardWrite),
    ("stop", ComputerMethod::Stop),
    ("resume", ComputerMethod::Resume),
    ("status", ComputerMethod::Status),
    ("evidence", ComputerMethod::Evidence),
    ("listen_start", ComputerMethod::ListenStart),
    ("listen_stop", ComputerMethod::ListenStop),
    ("sound_read", ComputerMethod::SoundRead),
    ("sound_wait", ComputerMethod::SoundWait),
    ("watch", ComputerMethod::Watch),
    ("recipe_save", ComputerMethod::RecipeSave),
    ("recipe_list", ComputerMethod::RecipeList),
    ("recipe_show", ComputerMethod::RecipeShow),
    ("recipe_run", ComputerMethod::RecipeRun),
    ("handoff", ComputerMethod::Handoff),
    (BATCH_ACTION, ComputerMethod::Batch),
    (WALK_ACTION, ComputerMethod::Walk),
];
/// The fields a batch step's actions read — the rest of the tool's fields
/// belong to actions no step may be (a test probes every step action for it).
const STEP_FIELDS: &[&str] = &[
    "action",
    "coordinate",
    "start_coordinate",
    "text",
    "scroll_direction",
    "scroll_amount",
    "scroll_pages",
    "background",
    "browser_chrome",
    "duration",
    "region",
    "app",
    "role",
    "label",
    "ocr",
    "window",
    "window_id",
    "kind",
    "x",
    "y",
    "width",
    "height",
    "timeout_ms",
    "absent",
    "confirming",
    "after",
    "min_confidence",
    "until",
    "after_text",
    "offset",
];

/// The core methods an action may run: its own, or — for `window` — every
/// `window-<kind>`.
fn methods(action: &str) -> impl Iterator<Item = ComputerMethod> {
    let named = ACTIONS.iter().find(|(name, _)| *name == action).map(|(_, method)| *method);
    ComputerMethod::ALL.iter().copied().filter(move |method| match named {
        Some(named) if named.window_action().is_some() => method.window_action().is_some(),
        Some(named) => *method == named,
        None => false,
    })
}

/// The action names, in the table's order.
fn action_names(keep: impl Fn(&str) -> bool) -> Vec<&'static str> {
    ACTIONS.iter().map(|(name, _)| *name).filter(|name| keep(name)).collect()
}

/// The window kinds, as the core names them, that `keep` admits.
fn window_kinds(keep: impl Fn(ComputerMethod) -> bool) -> Vec<&'static str> {
    ComputerMethod::ALL.iter().copied().filter(|method| keep(*method)).filter_map(ComputerMethod::window_action).collect()
}

/// How a model writes a boolean or a number when its call is not strict JSON
/// (`"true"`, `"1"`, `"250"`): read by this one pair for every field of every
/// action and every batch step, so the receipts and the command line carry the
/// normalized value (t-55888).
mod loose {
    use serde::de::{DeserializeOwned, Error};
    use serde::{Deserialize, Deserializer};
    use serde_json::{Number, Value};

    pub(super) fn boolean<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<bool>, D::Error> {
        match Option::<Value>::deserialize(deserializer)? {
            None | Some(Value::Null) => Ok(None),
            Some(Value::Bool(flag)) => Ok(Some(flag)),
            Some(Value::String(text)) => match text.trim().to_ascii_lowercase().as_str() {
                "true" | "1" => Ok(Some(true)),
                "false" | "0" => Ok(Some(false)),
                _ => Err(D::Error::custom(format!("invalid value {text:?}, expected a boolean: true or false"))),
            },
            Some(other) => Err(D::Error::custom(format!("invalid type: {other}, expected a boolean"))),
        }
    }

    /// `[790, 450]` as a JSON array or as the JSON text of one (`"[790, 450]"`):
    /// the one shape of a point, offset or region, said once on a refusal (t-62469).
    pub(super) fn point<'de, D: Deserializer<'de>, const N: usize>(deserializer: D) -> Result<Option<[f64; N]>, D::Error> {
        let shape = || format!("expected {N} numbers like {}, as an array or its JSON text", example::<N>());
        let value = match Option::<Value>::deserialize(deserializer)? {
            None | Some(Value::Null) => return Ok(None),
            Some(Value::String(text)) => serde_json::from_str::<Value>(text.trim())
                .map_err(|_| D::Error::custom(format!("invalid value {text:?}, {}", shape())))?,
            Some(other) => other,
        };
        let Value::Array(items) = &value else {
            return Err(D::Error::custom(format!("invalid value {value}, {}", shape())));
        };
        let numbers: Vec<f64> = items.iter().filter_map(Value::as_f64).collect();
        <[f64; N]>::try_from(numbers)
            .ok()
            .filter(|_| items.len() == N)
            .map(Some)
            .ok_or_else(|| D::Error::custom(format!("invalid value {value}, {}", shape())))
    }

    fn example<const N: usize>() -> &'static str {
        if N == 2 { "[790, 450]" } else { "[100, 200, 700, 500]" }
    }

    pub(super) fn number<'de, D: Deserializer<'de>, T: DeserializeOwned>(deserializer: D) -> Result<Option<T>, D::Error> {
        let number = match Option::<Value>::deserialize(deserializer)? {
            None | Some(Value::Null) => return Ok(None),
            Some(Value::Number(number)) => number,
            Some(Value::String(text)) => text
                .trim()
                .parse::<Number>()
                .map_err(|_| D::Error::custom(format!("invalid value {text:?}, expected a number")))?,
            Some(other) => return Err(D::Error::custom(format!("invalid type: {other}, expected a number"))),
        };
        T::deserialize(Value::Number(number)).map(Some).map_err(D::Error::custom)
    }
}

#[derive(Debug, Default, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ComputerInput {
    pub action: String,
    #[serde(default, deserialize_with = "loose::boolean")]
    pub background: Option<bool>,
    /// The person explicitly requested the browser application's menus, tabs or extension UI.
    #[serde(default, deserialize_with = "loose::boolean")]
    pub browser_chrome: Option<bool>,
    /// `[x, y]` in the pixels of the last screenshot, the Anthropic way.
    #[serde(default, deserialize_with = "loose::point")]
    pub coordinate: Option<[f64; 2]>,
    #[serde(default, deserialize_with = "loose::point")]
    pub start_coordinate: Option<[f64; 2]>,
    /// The key chord for `key`/`hold_key`, the text for `type`, the modifiers
    /// held during a click, the text to find/wait for, the clipboard text.
    pub text: Option<String>,
    pub scroll_direction: Option<String>,
    #[serde(default, deserialize_with = "loose::number")]
    pub scroll_amount: Option<u32>,
    #[serde(default, deserialize_with = "loose::number")]
    pub scroll_pages: Option<f64>,
    /// Seconds, the Anthropic way, for `hold_key` and `wait`.
    #[serde(default, deserialize_with = "loose::number")]
    pub duration: Option<f64>,
    /// `[x0, y0, x1, y1]` — two corners, the Anthropic way — in the last
    /// screenshot's pixels, for `zoom`/`find --ocr`/`read --ocr`.
    #[serde(default, deserialize_with = "loose::point")]
    pub region: Option<[f64; 4]>,
    pub app: Option<String>,
    pub url: Option<String>,
    pub path: Option<String>,
    pub program: Option<String>,
    pub args: Option<String>,
    pub role: Option<String>,
    pub label: Option<String>,
    #[serde(default, deserialize_with = "loose::boolean")]
    pub ocr: Option<bool>,
    /// An unscoped window title condition for `wait_for`.
    pub window: Option<String>,
    #[serde(default, deserialize_with = "loose::number")]
    pub window_id: Option<u64>,
    /// The window action (`focus|move|resize|minimize|zoom|close`).
    pub kind: Option<String>,
    #[serde(default, deserialize_with = "loose::number")]
    pub x: Option<f64>,
    #[serde(default, deserialize_with = "loose::number")]
    pub y: Option<f64>,
    #[serde(default, deserialize_with = "loose::number")]
    pub width: Option<f64>,
    #[serde(default, deserialize_with = "loose::number")]
    pub height: Option<f64>,
    #[serde(default, deserialize_with = "loose::number")]
    pub timeout_ms: Option<u64>,
    #[serde(default, deserialize_with = "loose::boolean")]
    pub absent: Option<bool>,
    /// The model's own declaration that this press is a payment, transfer
    /// or delete — the window asks the person before the helper moves.
    pub confirming: Option<String>,
    /// `handoff`: ask for a one-time code on the person's card (the core's
    /// `--ask-code`); the window types it into the field `into` names, and
    /// the answer carries no code.
    #[serde(default, deserialize_with = "loose::boolean")]
    pub ask_code: Option<bool>,
    /// `handoff` with `ask_code`: the words of the input command that puts
    /// the code into its field (`["set-value", "--app", …]`, a browser `type`,
    /// an emulator `text`) — the window runs it, so the code never comes back.
    pub into: Option<Vec<String>>,
    #[serde(default, deserialize_with = "loose::boolean")]
    pub force: Option<bool>,
    #[serde(default, deserialize_with = "loose::boolean")]
    pub reset_budget: Option<bool>,
    #[serde(default, deserialize_with = "loose::number")]
    pub last: Option<u64>,
    /// The ears' cursor: events heard after this number.
    #[serde(default, deserialize_with = "loose::number")]
    pub after: Option<u64>,
    /// The least confidence a heard sound needs to end a `sound_wait`.
    #[serde(default, deserialize_with = "loose::number")]
    pub min_confidence: Option<f64>,
    /// `watch`: `change` (the default) or `quiet`. `walk`: the text on the
    /// screen when it worked.
    pub until: Option<String>,
    /// `walk`: one sentence saying what to reach.
    pub goal: Option<String>,
    /// `walk`: the browser pane it looks at (an `app`, or a `platform` with
    /// its `device`, instead).
    pub pane: Option<String>,
    pub platform: Option<String>,
    pub device: Option<String>,
    /// `walk`: the most presses it may spend (the CLI's `--steps`, named apart
    /// from a batch's `steps`).
    #[serde(default, deserialize_with = "loose::number")]
    pub max_steps: Option<u64>,
    /// `walk`: judge ahead while a press lands; a second reader before the
    /// person; a repeat of a walk walked before.
    #[serde(default, deserialize_with = "loose::boolean")]
    pub overlap: Option<bool>,
    #[serde(default, deserialize_with = "loose::boolean")]
    pub rescue: Option<bool>,
    #[serde(default, deserialize_with = "loose::boolean")]
    pub replay: Option<bool>,
    /// `batch`: the actions to run in order in one call.
    pub steps: Option<Vec<ComputerInput>>,
    /// A recipe's name (`recipe_save`/`recipe_show`/`recipe_run`).
    pub name: Option<String>,
    /// `recipe_run`: the values its `{{name}}`s are filled with.
    pub params: Option<serde_json::Map<String, Value>>,
    /// `recipe_run`: the step to walk from (counted from 1), as a stop said.
    #[serde(default, deserialize_with = "loose::number")]
    pub start: Option<u64>,
    /// A click's target by its number on the last marked look, instead of a
    /// `coordinate`: the window presses that control and refuses if it moved.
    #[serde(default, deserialize_with = "loose::number")]
    pub mark: Option<u64>,
    /// A `left_click`'s target by its index in the named app's tree (as an
    /// `observe` with `app` shows it), pressed through accessibility.
    #[serde(default, deserialize_with = "loose::number")]
    pub element_index: Option<u64>,
    /// A `left_click` by the words a window shows (`app`, `label`, `ocr`):
    /// the line those words come after, telling recurring words apart.
    pub after_text: Option<String>,
    /// A `left_click` by words: `[dx, dy]` from the words' centre, in the
    /// last screenshot's pixels.
    #[serde(default, deserialize_with = "loose::point")]
    pub offset: Option<[f64; 2]>,
    /// Whether the look after an action waits for quiet. Continuous motion
    /// can be sampled immediately, while ordinary UI actions wait by default.
    #[serde(default, deserialize_with = "loose::boolean")]
    pub settle: Option<bool>,
}

/// The properties of one action — the one builder, used for the tool's input
/// and for each step of a batch (whose items must carry real properties: a
/// provider refuses a property-less object): the actions and window kinds it
/// offers, and — for a step — only the fields a step's actions read.
fn action_properties(actions: &[&str], kinds: &[&str], fields: Option<&[&str]>, marks: bool) -> Value {
    let mut properties = json!({
        "action": { "type": "string", "enum": actions },
        "coordinate": { "type": "array", "items": { "type": "number" }, "minItems": 2, "maxItems": 2,
            "description": "[x, y] in the last screenshot's pixels" },
        "start_coordinate": { "type": "array", "items": { "type": "number" }, "minItems": 2, "maxItems": 2,
            "description": "Drag start [x, y], in last screenshot pixels" },
        "text": { "type": "string" },
        "scroll_direction": { "type": "string", "enum": ["up", "down", "left", "right"] },
        "scroll_amount": { "type": "integer", "minimum": 1 },
        "scroll_pages": { "type": "number", "exclusiveMinimum": 0, "description": "App viewport pages; default 1. Desktop uses scroll_amount." },
        "duration": { "type": "number", "minimum": 0 },
        "region": { "type": "array", "items": { "type": "number" }, "minItems": 4, "maxItems": 4,
            "description": "[x0, y0, x1, y1] corners in last screenshot pixels" },
        "browser_chrome": { "type": "boolean", "description": "The person explicitly requested browser menus, tabs or extension UI" },
        "app": { "type": "string" },
        "url": { "type": "string" },
        "path": { "type": "string" },
        "program": { "type": "string" },
        "args": { "type": "string" },
        "role": { "type": "string", "description": "find's role ID, not localized descriptions" },
        "label": { "type": "string" },
        "ocr": { "type": "boolean" },
        "window": { "type": "string", "description": "Unscoped wait_for title only; no app/window selectors" },
        "window_id": { "type": "integer", "minimum": 1, "description": "Keep the observed app window; batch defaults need app too" },
        "kind": { "type": "string", "enum": kinds },
        "x": { "type": "number" },
        "y": { "type": "number" },
        "width": { "type": "number" },
        "height": { "type": "number" },
        "timeout_ms": { "type": "integer", "minimum": 1 },
        "absent": { "type": "boolean" },
        "confirming": { "type": "string", "enum": ["payment", "transfer", "delete"] },
        "ask_code": { "type": "boolean" },
        "into": { "type": "array", "items": { "type": "string" } },
        "force": { "type": "boolean" },
        "reset_budget": { "type": "boolean" },
        "last": { "type": "integer", "minimum": 1 },
        "after": { "type": "integer", "minimum": 0,
            "description": "sound_read/sound_wait: events after this number" },
        "min_confidence": { "type": "number", "minimum": 0, "maximum": 1 },
        "until": { "type": "string", "description": format!("watch: {}; walk: the text on screen when it worked", WATCH_UNTIL.join("|")) },
        "element_index": { "type": "integer", "minimum": 0 },
        "after_text": { "type": "string" },
        "offset": { "type": "array", "items": { "type": "number" }, "minItems": 2, "maxItems": 2 },
        "settle": { "type": "boolean",
            "description": "false samples without verifying completion; true (default) waits for quiet" },
        "name": { "type": "string" },
        "params": { "type": "object" },
        "start": { "type": "integer", "minimum": 1 }
    });
    // A walk's own fields: what the CLI's walk takes, and nothing it lacks.
    let walk = json!({
        "background": { "type": "boolean", "description": "App-only input; no global fallback. Supported batch steps inherit it." },
        "goal": { "type": "string", "maxLength": GOAL_CHAR_CAP },
        "pane": { "type": "string" },
        "platform": { "type": "string" },
        "device": { "type": "string" },
        "max_steps": { "type": "integer", "minimum": 1, "maximum": WALK_STEPS_MAX },
        "overlap": { "type": "boolean" },
        "rescue": { "type": "boolean" },
        "replay": { "type": "boolean" }
    });
    if let (Some(object), Value::Object(walk)) = (properties.as_object_mut(), walk) {
        object.extend(walk);
    }
    // Only a process whose looks carry marks offers the field; otherwise it
    // costs the schema nothing.
    if marks {
        properties["mark"] = json!({ "type": "integer", "minimum": 1,
            "description": "a click's target by its number in the last look's marks legend, instead of coordinate" });
    }
    if let (Some(fields), Some(object)) = (fields, properties.as_object_mut()) {
        object.retain(|name, _| fields.contains(&name.as_str()));
        for name in ["scroll_pages", "window", "window_id", "background"] {
            if let Some(field) = object.get_mut(name).and_then(Value::as_object_mut) {
                field.remove("description");
            }
        }
    }
    properties
}

pub(crate) fn tool_specs() -> Vec<ToolSpec> {
    vec![ToolSpec {
        name: "Computer",
        description: "Drive desktop apps inside a ZeroCode pane: screenshots/zoom, mouse moves/clicks/drags/scroll, \
            keys/text/waits, app/window management, element/OCR reads, clipboard read/write and programs. \
            For web pages use zerocode-browser open <url>, then read, marks, click, type and fill; set \
            browser_chrome only when the person explicitly requests browser menus, tabs or extension UI. \
            For background work, observe {app}, retain \
            app/window_id and set background: true. Never fall back to activation or global input. \
            Unverified keyboard receiver: use a verified writable-field capability or handoff. \
            Positions/sizes, including returned positions and regions, use the last full screenshot's pixels; \
            zoom does not change that frame. Actions wait for repaint and observe the same target; unchanged \
            pixels are omitted. observe {app} shows the accessibility tree; left_click {app, window_id, \
            element_index} presses its control. label/role resolve a fresh control in that window. \
            list_windows {app} keeps that app's windows (all if none match); \"true\"/\"250\" strings work. \
            Batch familiar steps once with one final look; see steps for scope/waits. For windows without \
            trees (mirrored phones), left_click {app, label, ocr: true, after_text: the preceding line} presses \
            freshly read words; wait_for {app, ocr: true, text} checks values. listen_start hears machine/app; \
            sound_wait {text: \"alarm_clock,siren\"} waits, sound_read reads, listen_stop ends. watch waits \
            for change or quiet without pictures. recipe_save keeps successful steps; recipe_run substitutes \
            params for {{names}}, reports where it stopped and start for resuming. walk {goal: one sentence, \
            app|pane|(platform,device), until: success text, max_steps} runs the window's judgment loop and \
            returns status; after no_look/stuck its ladder looks again: continue from relooked, else take its \
            roads in order, the person last. CAPTCHA/password (secure_input): handoff {text: what the person must do}. \
            One-time code: handoff {ask_code: true, into: input command words, e.g. \
            [\"set-value\",\"--app\",\"A\",\"--element-index\",\"3\"]}: the window types it, you never receive it, \
            and a look after may show it. stopped means stop/report. Payment/transfer/delete \
            requires the person's confirmation; set confirming: payment|transfer|delete.",
        input_schema: {
            let mut properties =
                action_properties(&action_names(|_| true), &window_kinds(|_| true), None, looks_carry_marks());
            let batches = |method: ComputerMethod| method.batches();
            properties["steps"] = json!({
                "type": "array",
                "minItems": 1,
                "maxItems": COMPUTER_BATCH_MAX_STEPS,
                "items": {
                    "type": "object",
                    // A step is never a click by mark: its look can change
                    // between steps.
                    "properties": action_properties(
                        &action_names(|action| methods(action).any(batches)),
                        &window_kinds(batches),
                        Some(STEP_FIELDS),
                        false,
                    ),
                    "required": ["action"],
                    "additionalProperties": false
                },
                "description": "batch: ordered actions, same parameters/pixel frame as single actions. App steps \
                    use labels, not coordinates. Default app+window_id or scope each step; overrides must agree. \
                    wait sends duration only; scoped wait_for requires a matching window receipt. sound_wait/watch \
                    stay unscoped, without background flags. wait_for/sound_wait take timeout_ms. Stop on refusal; \
                    one final look."
            });
            json!({
                "type": "object",
                "properties": properties,
                "required": ["action"],
                "additionalProperties": false
            })
        },
        // The desktop is the whole machine: only a session already trusted
        // with everything may drive it. The window's own guard (budgets,
        // stop, the person's last step) bounds it from there.
        required_permission: PermissionMode::DangerFullAccess,
    }]
}

pub(crate) fn dispatch_result(
    ctx: &ToolContext,
    enforcer: Option<&PermissionEnforcer>,
    name: &str,
    input: &Value,
) -> Option<Result<ToolExecutionResult, ToolError>> {
    match name {
        "Computer" => Some(
            maybe_enforce_permission_check(enforcer, name, input).and_then(|()| {
                let road = ComputerRoad::from_path()?;
                run_computer_execution(input, ctx, &road)
            }),
        ),
        _ => None,
    }
}

pub(crate) fn canonical_call(input: &Value) -> Result<runtime::ResolvedToolCall, ToolError> {
    let parsed: ComputerInput = from_value(input)?;
    let argv = argv_for(&parsed)?;
    Ok(runtime::ResolvedToolCall {
        name: "Computer".to_string(),
        input: json!({"action":parsed.action.trim(), "argv":argv}).to_string(),
    })
}

/// Where the shim is. Looked up on `PATH` the way a shell would — inside a
/// pane of the window it is there; anywhere else the desktop is not ours to drive.
#[derive(Debug, Clone)]
pub(crate) struct ComputerRoad {
    pub program: PathBuf,
}

impl ComputerRoad {
    pub(crate) fn from_path() -> Result<Self, ToolError> {
        let path = std::env::var_os("PATH").unwrap_or_default();
        Self::from_search_path(&path, cfg!(windows))
    }

    fn from_search_path(path: &std::ffi::OsStr, windows: bool) -> Result<Self, ToolError> {
        let program_name = if windows { "zerocode-computer.cmd" } else { COMPUTER_SHIM };
        std::env::split_paths(path)
            .map(|dir| dir.join(program_name))
            .find(|candidate| candidate.is_file())
            .map(|program| Self { program })
            .ok_or_else(|| {
                ToolError::Execution(format!(
                    "`{COMPUTER_SHIM}` is not on PATH — the Computer tool only works inside a ZeroCode pane, \
                     where the window's shim carries the person's token"
                ))
            })
    }
}

fn number(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{value:.0}")
    } else {
        format!("{value}")
    }
}

fn need<'a>(value: Option<&'a str>, action: &str, field: &str) -> Result<&'a str, ToolError> {
    value
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .ok_or_else(|| ToolError::InvalidInput(format!("`{action}` needs `{field}`")))
}

fn point(value: Option<[f64; 2]>, action: &str, field: &str) -> Result<[f64; 2], ToolError> {
    value.ok_or_else(|| ToolError::InvalidInput(format!("`{action}` needs `{field}` as [x, y]")))
}

/// Whether the model gave a position or a length — words in pixels.
fn speaks_in_pixels(input: &ComputerInput) -> bool {
    input.coordinate.is_some()
        || input.offset.is_some()
        || input.start_coordinate.is_some()
        || input.region.is_some()
        || input.x.is_some()
        || input.y.is_some()
        || input.width.is_some()
        || input.height.is_some()
}

/// What the model said, in the screen points the hand moves in: every
/// position and length it gave is read as pixels of `frame`'s picture, and a
/// region's two corners become the CLI's corner and size.
fn in_points(mut input: ComputerInput, frame: ShotFrame) -> Result<ComputerInput, ToolError> {
    let app_coordinates = input.app.is_some()
        && (MARK_CLICKS.contains(&input.action.trim()) || matches!(input.action.trim(), "scroll" | "left_click_drag"));
    let point = |pixel: [f64; 2]| {
        if app_coordinates { [frame.length_to_point(pixel[0]), frame.length_to_point(pixel[1])] }
        else { frame.to_point(pixel) }
    };
    input.coordinate = input.coordinate.map(point);
    input.offset = input.offset.map(|[dx, dy]| [frame.length_to_point(dx), frame.length_to_point(dy)]);
    input.start_coordinate = input.start_coordinate.map(point);
    input.region = match input.region {
        Some([x0, y0, x1, y1]) if x1 > x0 && y1 > y0 => Some(frame.rect_to_point([x0, y0, x1 - x0, y1 - y0])),
        Some(_) => {
            return Err(ToolError::InvalidInput(
                "`region` is [x0, y0, x1, y1]: the top-left corner, then the bottom-right".into(),
            ))
        }
        None => None,
    };
    let [x, y] = frame.to_point([input.x.unwrap_or(0.0), input.y.unwrap_or(0.0)]);
    input.x = input.x.map(|_| x);
    input.y = input.y.map(|_| y);
    input.width = input.width.map(|width| frame.length_to_point(width));
    input.height = input.height.map(|height| frame.length_to_point(height));
    Ok(input)
}

/// Whether, with no frame yet, this action needs one first: it names a place,
/// or its answer will.
fn needs_a_frame(input: &ComputerInput) -> bool {
    speaks_in_pixels(input) || ANSWERS_PLACES.contains(&input.action.trim())
}

/// The command line with this model's name in the window's last-look table.
fn as_viewer(mut argv: Vec<String>, ctx: &ToolContext) -> Vec<String> {
    let at = argv.iter().position(|word| word == "--json").unwrap_or(argv.len());
    argv.splice(at..at, ["--viewer".to_string(), ctx.computer_viewer().to_string()]);
    argv
}

/// Whether the action presses something the person's last step may guard.
pub(crate) fn presses(action: &str) -> bool {
    methods(action).any(ComputerMethod::presses)
}

/// Whether the action changes the screen — and so is followed by a look.
pub(crate) fn acts(action: &str) -> bool {
    methods(action).any(ComputerMethod::acts)
}

fn push(argv: &mut Vec<String>, words: &[&str]) {
    argv.extend(words.iter().map(|word| (*word).to_string()));
}

fn spelled_region(region: [f64; 4]) -> String {
    region.iter().map(|side| number(*side)).collect::<Vec<_>>().join(",")
}

fn app_scoped_action(action: &str) -> bool {
    matches!(action, "screenshot" | "observe" | "find" | "read" | "wait_for" | "type" | "key" | "scroll" | "left_click_drag")
        || MARK_CLICKS.contains(&action)
}

fn timed_wait(action: &str) -> bool {
    methods(action).any(|method| method == ComputerMethod::Wait)
}

/// Batchable waits that neither act nor read an app have no input mode.
/// Their own read conditions still apply: screen/sound scope is not a window scope.
fn background_neutral_action(action: &str) -> bool {
    !app_scoped_action(action) && methods(action).any(|method| method.batches() && !method.acts())
}

/// Both shapes of a scroll, each with an example a model can send as it stands (t-62469 S1).
const SCROLL_SHAPES: &str = "app-scoped scroll takes scroll_pages, e.g. {\"app\":\"Notes\",\"window_id\":41,\"coordinate\":[20,40],\"scroll_direction\":\"down\",\"scroll_pages\":1}; desktop scroll (no app) takes scroll_amount, e.g. {\"coordinate\":[790,450],\"scroll_direction\":\"down\",\"scroll_amount\":3}; both need a coordinate";

fn validate_scope(input: &ComputerInput) -> Result<(), ToolError> {
    let action = input.action.trim();
    let timer = timed_wait(action);
    if input.window.is_some() && action != "wait_for" {
        return Err(ToolError::InvalidInput("`window` is only a wait_for title condition; use app and window_id to select an action's window".into()));
    }
    if input.window.is_some() && (input.app.is_some() || input.window_id.is_some() || input.background == Some(true)) {
        return Err(ToolError::InvalidInput("a wait_for title condition scans the window list and cannot honor app/window_id or background scope; use a scoped element condition instead".into()));
    }
    if timer {
        if input.text.is_some() || input.label.is_some() || input.role.is_some() || input.ocr.is_some()
            || input.absent.is_some() || input.timeout_ms.is_some() || input.until.is_some()
        {
            return Err(ToolError::InvalidInput("`wait` is a duration only; use wait_for for an app/window condition, sound_wait for sound, or watch for screen changes".into()));
        }
        if speaks_in_pixels(input) {
            return Err(ToolError::InvalidInput("a timer wait does not read coordinates; use wait_for for a screen condition".into()));
        }
        if input.app.is_some() != input.window_id.is_some() {
            return Err(ToolError::InvalidInput("a timer's window context needs both app and a positive window_id; omit both for an unscoped delay".into()));
        }
    }
    if input.mark.is_some() && (input.app.is_some() || input.window_id.is_some()) {
        return Err(ToolError::InvalidInput("a mark already pins its app and window; do not combine it with another app/window selector".into()));
    }
    if let Some(window_id) = input.window_id {
        if window_id == 0 || (action != "window" && ((!app_scoped_action(action) && !timer) || input.app.as_deref().is_none_or(|app| app.trim().is_empty()))) {
            return Err(ToolError::InvalidInput("window_id needs a supported app-scoped action and a positive window ID; this selector cannot be ignored".into()));
        }
    }
    if input.app.is_some() && !app_scoped_action(action) && !timer
        && !matches!(action, "launch" | "quit" | "activate" | "open" | "listen_start" | LIST_WINDOWS_ACTION | WALK_ACTION)
    {
        return Err(ToolError::InvalidInput(format!("`app` is not supported by `{action}`; no desktop fallback was attempted")));
    }
    if input.background == Some(true) && action != WALK_ACTION && !background_neutral_action(action) {
        let supported = matches!(action, "observe" | "screenshot" | "find" | "read" | "wait_for" | "type" | "key" | "scroll") || MARK_CLICKS.contains(&action);
        if !supported || (input.app.as_deref().is_none_or(|app| app.trim().is_empty()) && input.mark.is_none()) {
            return Err(ToolError::InvalidInput("background input requires a supported app-scoped action or a pinned mark; global input is never a fallback".into()));
        }
    }
    if input.scroll_pages.is_some() && (action != "scroll" || input.app.is_none()) {
        return Err(ToolError::InvalidInput(format!("scroll_pages belongs to app-scoped scrolling: {SCROLL_SHAPES}")));
    }
    if input.app.is_some() && action == "scroll" && input.scroll_amount.is_some() {
        return Err(ToolError::InvalidInput(format!("scroll_amount belongs to desktop scrolling: {SCROLL_SHAPES}")));
    }
    if input.app.is_some() && (input.coordinate.is_some() || input.start_coordinate.is_some()) && input.window_id.is_none() {
        return Err(ToolError::InvalidInput("app coordinates require the window_id from an observation of that window".into()));
    }
    Ok(())
}

fn target_of(input: &ComputerInput) -> Option<ComputerTarget> {
    input.app.as_ref().map(|app| ComputerTarget { app: app.clone(), window_id: input.window_id })
}

/// Where an answer names the one window it is of, within its result or at
/// its top: the window's `id`, its `title` — the document it shows — and the
/// document's identity in `document` when the window gives one.
const ANSWER_ROOTS: &[&str] = &["/result/", "/"];
const WINDOW_OBJECTS: &[&str] = &["snapshot/window", "tree/window", "window"];

fn window_pointers(field: &'static str) -> impl Iterator<Item = String> {
    ANSWER_ROOTS.iter().flat_map(move |root| WINDOW_OBJECTS.iter().map(move |object| format!("{root}{object}/{field}")))
}

/// The title of the window an answer is of: the document it shows.
fn window_title(answer: &Value) -> Option<String> {
    window_pointers("title").find_map(|pointer| answer.pointer(&pointer)?.as_str().map(str::to_string))
}

/// The identity of the document the window an answer is of shows, when the
/// window gives one — its `AXDocument`, a browser page's URL (t-68358). A
/// title can stay through a change of document (Finder, Terminal, a web app
/// with one title per site); the identity does not.
fn window_document(answer: &Value) -> Option<String> {
    window_pointers("document").find_map(|pointer| answer.pointer(&pointer)?.as_str().map(str::to_string))
}

fn checked_window(answer: &Value, expected: Option<u64>) -> Result<Option<u64>, ToolError> {
    let mut found = None;
    let acted_on = ANSWER_ROOTS.iter().map(|root| format!("{root}action/targetWindowId"));
    for pointer in acted_on.chain(window_pointers("id")) {
        if let Some(window_id) = answer.pointer(&pointer).and_then(Value::as_u64).filter(|window_id| *window_id > 0) {
            if expected.is_some_and(|expected| expected != window_id) || found.is_some_and(|found| found != window_id) {
                return Err(ToolError::Execution("window_scope_mismatch: the selected window did not answer; its contents were not returned".into()));
            }
            found = Some(window_id);
        }
    }
    if expected.is_some() && found.is_none() {
        return Err(ToolError::Execution("window_scope_unverified: the answer did not identify the selected window; its contents were not returned".into()));
    }
    Ok(found)
}

fn unverified_action_receipt(action: &str, window_id: Option<u64>, scope_error: &ToolError, ran: usize) -> Result<String, ToolError> {
    to_pretty_json(json!({
        "ok": true,
        "action": action,
        "input_dispatched": true,
        "completion_verified": false,
        "ran": ran,
        "window_scope": { "verified": false, "requested_window_id": window_id, "reason": scope_error.to_string() },
        "do_not_repeat": true,
        "next": "Input was already dispatched. Its target or result could not be verified. Do not repeat it; observe the requested app/window and check the result first.",
        "result": null,
        "screenshot": null,
        "looked_after": null,
    }))
}

/// The command line an action means. Pure: the table a test pins, in three
/// families — looking and input, apps and the system, the meaning layer.
pub(crate) fn argv_for(input: &ComputerInput) -> Result<Vec<String>, ToolError> {
    let action = input.action.trim();
    validate_scope(input)?;
    if input.mark.is_some() && !MARK_CLICKS.contains(&action) {
        return Err(ToolError::InvalidInput(format!("`mark` belongs to the clicks ({}), not `{action}`", MARK_CLICKS.join(", "))));
    }
    if input.element_index.is_some() && action != ELEMENT_CLICK {
        return Err(ToolError::InvalidInput(format!("`element_index` is a `{ELEMENT_CLICK}`'s target, not `{action}`'s")));
    }
    let mut argv: Vec<String> = Vec::new();
    let handled = argv_look_and_input(action, input, &mut argv)?
        || argv_apps_and_system(action, input, &mut argv)?
        || argv_meaning(action, input, &mut argv)
        || argv_sound(action, input, &mut argv)
        || argv_eye(action, input, &mut argv)
        || argv_recipes(action, input, &mut argv)?
        || argv_persons_turn(action, input, &mut argv)?
        || argv_walk(action, input, &mut argv);
    if !handled {
        return Err(ToolError::InvalidInput(format!(
            "unknown Computer action {action:?}; one of {}",
            action_names(|_| true).join(", ")
        )));
    }
    if let Some(window_id) = input.window_id.filter(|_| !timed_wait(action)) {
        if !matches!(action, "window" | "observe" | "find" | "read" | "wait_for") {
            push(&mut argv, &["--window-id", &window_id.to_string()]);
        }
    }
    if input.background == Some(true) && action != WALK_ACTION && !background_neutral_action(action) {
        argv.push("--background".into());
    }
    if presses(action) {
        if let Some(kind) = input.confirming.as_deref() {
            push(&mut argv, &["--confirming", kind]);
        }
    }
    if input.browser_chrome == Some(true) {
        argv.push("--browser-chrome".into());
    }
    argv.push("--json".into());
    Ok(argv)
}

/// Anthropic's own actions: the screen, the mouse and the keys.
/// A click's spelling: one target — a mark, an element's index, a control named by
/// what it reads (label/role with its app), or a screen point — and the button, the
/// count and the modifiers `text` carries for clicks.
fn argv_click(action: &str, input: &ComputerInput, argv: &mut Vec<String>) -> Result<(), ToolError> {
        let (button, count) = match action {
            "right_click" => ("right", "1"),
            "middle_click" => ("middle", "1"),
            "double_click" => ("left", "2"),
            "triple_click" => ("left", "3"),
            _ => ("left", "1"),
        };
        // A control named by what it reads (label, role) with its app:
        // find's matcher, run on a fresh tree right before the press —
        // the one target a batch step may name after earlier steps.
        let by_reading = input.label.is_some() || input.role.is_some();
        let targets = usize::from(input.mark.is_some())
            + usize::from(input.element_index.is_some())
            + usize::from(input.coordinate.is_some())
            + usize::from(by_reading);
        if targets > 1 {
            return Err(ToolError::InvalidInput(format!(
                "`{action}` takes one target: `mark`, `element_index`, `coordinate`, or `label`/`role` with `app`"
            )));
        }
        if let Some(index) = input.element_index {
            let app = need(input.app.as_deref(), action, "app")?;
            push(argv, &["click", "--app", app, "--element-index", &index.to_string(), "--no-screenshot"]);
        } else if by_reading && input.ocr == Some(true) {
            // A window with no tree (a mirrored phone): the words its pixels
            // show, read at the press — the core's `click --ocr`.
            let app = need(input.app.as_deref(), action, "app")?;
            let words = need(input.label.as_deref(), action, "label")?;
            push(argv, &["click", "--app", app, "--ocr", "--text", words]);
            if let Some(after) = input.after_text.as_deref() {
                push(argv, &["--after-text", after]);
            }
            if let Some([dx, dy]) = input.offset {
                push(argv, &["--dx", &number(dx), "--dy", &number(dy)]);
            }
            push(argv, &["--mouse-button", button, "--click-count", count]);
        } else if by_reading {
            let app = need(input.app.as_deref(), action, "app")?;
            push(argv, &["click", "--app", app]);
            if let Some(label) = input.label.as_deref() {
                push(argv, &["--label", label]);
            }
            if let Some(role) = input.role.as_deref() {
                push(argv, &["--role", role]);
            }
            push(argv, &["--mouse-button", button, "--click-count", count, "--no-screenshot"]);
        } else if let Some(mark) = input.mark {
            push(argv, &["click", "--mark", &mark.to_string(), "--mouse-button", button, "--click-count", count]);
        } else if let Some(app) = input.app.as_deref() {
            let [horizontal, vertical] = point(input.coordinate, action, "coordinate")?;
            push(argv, &["click", "--app", app, "--x", &number(horizontal), "--y", &number(vertical), "--mouse-button", button, "--click-count", count, "--no-screenshot"]);
        } else {
            let [x, y] = point(input.coordinate, action, "coordinate")?;
            push(argv, &["mouse-click", "--x", &number(x), "--y", &number(y), "--mouse-button", button, "--click-count", count]);
        }
        if let Some(modifiers) = input.text.as_deref().map(str::trim).filter(|text| !text.is_empty()) {
            push(argv, &["--modifiers", modifiers]);
        }
    
    Ok(())
}

fn argv_look_and_input(action: &str, input: &ComputerInput, argv: &mut Vec<String>) -> Result<bool, ToolError> {
    match action {
        "screenshot" => {
            if let Some(app) = input.app.as_deref() {
                push(argv, &["observe", "--app", app]);
            } else {
                push(argv, &["screenshot"]);
            }
        }
        "observe" => {
            // An explicit look says what changed since this model's last one.
            push(argv, &["observe", "--diff"]);
            if let Some(app) = input.app.as_deref() {
                push(argv, &["--app", app]);
            }
            if let Some(id) = input.window_id {
                push(argv, &["--window-id", &id.to_string()]);
            }
            if input.ocr == Some(true) {
                push(argv, &["--ocr"]);
            }
        }
        "zoom" => {
            let region = input
                .region
                .ok_or_else(|| ToolError::InvalidInput("`zoom` needs `region` as [x, y, width, height]".into()))?;
            push(argv, &["zoom", "--region", &spelled_region(region)]);
        }
        "mouse_move" => {
            let [x, y] = point(input.coordinate, action, "coordinate")?;
            push(argv, &["mouse-move", "--x", &number(x), "--y", &number(y)]);
        }
        "left_click" | "right_click" | "middle_click" | "double_click" | "triple_click" => argv_click(action, input, argv)?,
        "left_click_drag" => {
            let [from_x, from_y] = point(input.start_coordinate, action, "start_coordinate")?;
            let [to_x, to_y] = point(input.coordinate, action, "coordinate")?;
            if let Some(app) = input.app.as_deref() {
                push(argv, &["drag", "--app", app, "--no-screenshot"]);
            } else {
                push(argv, &["mouse-drag"]);
            }
            push(argv, &["--from-x", &number(from_x), "--from-y", &number(from_y), "--to-x", &number(to_x), "--to-y", &number(to_y)]);
        }
        "scroll" => {
            let [x, y] = point(input.coordinate, action, "coordinate")?;
            if let Some(app) = input.app.as_deref() {
                let direction = need(input.scroll_direction.as_deref(), action, "scroll_direction")?;
                let pages = input.scroll_pages.unwrap_or(1.0);
                if !["up", "down", "left", "right"].contains(&direction) || !pages.is_finite() || pages <= 0.0 {
                    return Err(ToolError::InvalidInput("app scrolling needs a direction and positive finite scroll_pages".into()));
                }
                push(argv, &["scroll", "--app", app, "--x", &number(x), "--y", &number(y), "--direction", direction, "--pages", &number(pages), "--no-screenshot"]);
                return Ok(true);
            }
            let clicks = i64::from(input.scroll_amount.unwrap_or(1)) * SCROLL_LINES_PER_CLICK;
            let (flag, lines) = match need(input.scroll_direction.as_deref(), action, "scroll_direction")? {
                "up" => ("--dy", clicks),
                "down" => ("--dy", -clicks),
                "left" => ("--dx", clicks),
                "right" => ("--dx", -clicks),
                other => {
                    return Err(ToolError::InvalidInput(format!(
                        "`scroll_direction` must be up, down, left or right (not {other:?})"
                    )));
                }
            };
            push(argv, &["mouse-scroll", "--x", &number(x), "--y", &number(y), flag, &lines.to_string()]);
        }
        "key" => {
            let chord = need(input.text.as_deref(), action, "text")?;
            if let Some(app) = input.app.as_deref() {
                push(argv, &["press-key", "--app", app, "--key", chord, "--no-screenshot"]);
            } else {
                push(argv, &["key", "--key", chord]);
            }
        }
        "hold_key" => {
            let chord = need(input.text.as_deref(), action, "text")?;
            let ms = seconds_to_ms(input.duration, action)?;
            push(argv, &["hold-key", "--key", chord, "--ms", &ms.to_string()]);
        }
        "type" => {
            let text = input
                .text
                .as_deref()
                .ok_or_else(|| ToolError::InvalidInput("`type` needs `text`".into()))?;
            if let Some(app) = input.app.as_deref() {
                push(argv, &["type-text", "--app", app, "--text", text, "--no-screenshot"]);
            } else {
                push(argv, &["type", "--text", text]);
            }
        }
        "wait" => {
            let ms = seconds_to_ms(input.duration, action)?;
            push(argv, &["wait", "--ms", &ms.to_string()]);
        }
        "cursor_position" => push(argv, &["cursor-position"]),
        _ => return Ok(false),
    }
    Ok(true)
}

/// Apps, windows, the clipboard, a program, and the one hand.
fn argv_apps_and_system(action: &str, input: &ComputerInput, argv: &mut Vec<String>) -> Result<bool, ToolError> {
    match action {
        "launch" => {
            let app = need(input.app.as_deref(), action, "app")?;
            push(argv, &["launch", "--app", app]);
            if let Some(args) = input.args.as_deref() {
                push(argv, &["--args", args]);
            }
            if let Some(ms) = input.timeout_ms {
                push(argv, &["--wait-ready", &ms.to_string()]);
            }
        }
        "quit" => {
            let app = need(input.app.as_deref(), action, "app")?;
            push(argv, &["quit", "--app", app]);
            if input.force == Some(true) {
                push(argv, &["--force"]);
            }
        }
        "activate" => {
            let app = need(input.app.as_deref(), action, "app")?;
            push(argv, &["activate", "--app", app]);
        }
        "open" => {
            push(argv, &["open"]);
            match (input.url.as_deref(), input.path.as_deref()) {
                (Some(url), None) => push(argv, &["--url", url]),
                (None, Some(path)) => push(argv, &["--path", path]),
                _ => return Err(ToolError::InvalidInput("`open` needs exactly one of `url` or `path`".into())),
            }
            if let Some(with) = input.app.as_deref() {
                push(argv, &["--with", with]);
            }
        }
        "run" => {
            let program = need(input.program.as_deref(), action, "program")?;
            push(argv, &["run", "--program", program]);
            if let Some(args) = input.args.as_deref() {
                push(argv, &["--args", args]);
            }
            if let Some(cwd) = input.path.as_deref() {
                push(argv, &["--cwd", cwd]);
            }
            if let Some(ms) = input.timeout_ms {
                push(argv, &["--timeout-ms", &ms.to_string()]);
            }
        }
        "window" => {
            let kind = need(input.kind.as_deref(), action, "kind")?;
            let id = input
                .window_id
                .ok_or_else(|| ToolError::InvalidInput("`window` needs `window_id`".into()))?;
            let verb = format!("window-{kind}");
            push(argv, &[&verb, "--id", &id.to_string()]);
            if let (Some(x), Some(y)) = (input.x, input.y) {
                push(argv, &["--x", &number(x), "--y", &number(y)]);
            }
            if let (Some(width), Some(height)) = (input.width, input.height) {
                push(argv, &["--width", &number(width), "--height", &number(height)]);
            }
        }
        LIST_WINDOWS_ACTION => push(argv, &["list-all-windows"]),
        "clipboard_read" => push(argv, &["clipboard-read"]),
        "clipboard_write" => {
            let text = input
                .text
                .as_deref()
                .ok_or_else(|| ToolError::InvalidInput("`clipboard_write` needs `text`".into()))?;
            push(argv, &["clipboard-write", "--text", text]);
        }
        "stop" => push(argv, &["stop"]),
        "resume" => {
            push(argv, &["resume"]);
            if input.reset_budget == Some(true) {
                push(argv, &["--reset-budget"]);
            }
        }
        "status" => push(argv, &["status"]),
        "evidence" => {
            let last = input.last.unwrap_or(EVIDENCE_STEPS).to_string();
            push(argv, &["evidence", "--last", &last]);
        }
        _ => return Ok(false),
    }
    Ok(true)
}

/// The meaning layer: find, read, wait for — through the tree or the pixels.
fn argv_meaning(action: &str, input: &ComputerInput, argv: &mut Vec<String>) -> bool {
    let verb = match action {
        "find" => "find",
        "read" => "read",
        "wait_for" => "wait-for",
        _ => return false,
    };
    push(argv, &[verb]);
    if let Some(app) = input.app.as_deref() {
        push(argv, &["--app", app]);
    }
    if input.ocr == Some(true) {
        push(argv, &["--ocr"]);
        if let Some(region) = input.region {
            push(argv, &["--region", &spelled_region(region)]);
        }
    }
    if let Some(text) = input.text.as_deref() {
        push(argv, &["--text", text]);
    }
    if let Some(role) = input.role.as_deref() {
        push(argv, &["--role", role]);
    }
    if let Some(label) = input.label.as_deref() {
        push(argv, &["--label", label]);
    }
    if let Some(id) = input.window_id {
        push(argv, &["--window-id", &id.to_string()]);
    }
    if action == "wait_for" {
        if let Some(title) = input.window.as_deref() {
            push(argv, &["--window", title]);
        }
        if let Some(ms) = input.timeout_ms {
            push(argv, &["--timeout-ms", &ms.to_string()]);
        }
        if input.absent == Some(true) {
            push(argv, &["--absent"]);
        }
    }
    true
}

/// The ears: listen, read what was heard, wait for a sound, stop.
fn argv_sound(action: &str, input: &ComputerInput, argv: &mut Vec<String>) -> bool {
    let verb = match action {
        "listen_start" => "listen-start",
        "listen_stop" => "listen-stop",
        "sound_read" => "sound-read",
        "sound_wait" => "sound-wait",
        _ => return false,
    };
    push(argv, &[verb]);
    if action == "listen_start" {
        if let Some(app) = input.app.as_deref() {
            push(argv, &["--app", app]);
        }
    }
    if action == "sound_wait" {
        if let Some(labels) = input.text.as_deref().map(str::trim).filter(|labels| !labels.is_empty()) {
            push(argv, &["--label", labels]);
        }
        if let Some(bar) = input.min_confidence {
            push(argv, &["--min-confidence", &number(bar)]);
        }
        if let Some(ms) = input.timeout_ms {
            push(argv, &["--timeout-ms", &ms.to_string()]);
        }
    }
    if matches!(action, "sound_read" | "sound_wait") {
        if let Some(after) = input.after {
            push(argv, &["--after", &after.to_string()]);
        }
    }
    true
}

/// The eye: wait for the screen to change, or to go still — no pictures.
fn argv_eye(action: &str, input: &ComputerInput, argv: &mut Vec<String>) -> bool {
    if !methods(action).any(|method| method == ComputerMethod::Watch) {
        return false;
    }
    push(argv, &[ComputerMethod::Watch.verb_name()]);
    if let Some(until) = input.until.as_deref() {
        push(argv, &["--until", until]);
    }
    if let Some(ms) = input.timeout_ms {
        push(argv, &["--timeout-ms", &ms.to_string()]);
    }
    true
}

/// What a one-time code asked for with no field to put it in is told: the
/// code never comes back to the model, so the handoff needs `into`.
const NO_FIELD_FOR_THE_CODE: &str = "`ask_code` needs `into`: the one-time code does not come back to you — the window types it \
    into the field `into` names, so it never reaches this conversation. Give `into` the words of the input command that puts it \
    there, for example [\"set-value\",\"--app\",\"Form\",\"--element-index\",\"3\"] or [\"browser\",\"type\",\"browser-1\",\"#otp\"]";

/// What a field named with no code to put in it is told.
const NO_CODE_FOR_THE_FIELD: &str = "`into` names the field a one-time code is typed into: it goes with `ask_code`";

/// The person's turn — a CAPTCHA, a password field, a code they read out
/// (`ask_code`, with the field it goes into, `into`): they do it, and the
/// call waits until they say so (or its `timeout_ms`).
fn argv_persons_turn(action: &str, input: &ComputerInput, argv: &mut Vec<String>) -> Result<bool, ToolError> {
    if !methods(action).any(|method| method == ComputerMethod::Handoff) {
        return Ok(false);
    }
    push(argv, &[ComputerMethod::Handoff.verb_name(), "--reason", need(input.text.as_deref(), action, "text")?]);
    // A one-time code the person reads out: the window gives the card its
    // line, types what is typed into the field `into` names and answers
    // without it, or refuses a reason that names a secret — never judged
    // here. A code with no field to go to, or a field with no code, is
    // refused before the shell is called.
    match (input.ask_code == Some(true), input.into.as_deref()) {
        (true, Some(words)) if !words.is_empty() => {
            let spelled = serde_json::to_string(words).map_err(|error| ToolError::InvalidInput(error.to_string()))?;
            push(argv, &["--ask-code", "--into", &spelled]);
        }
        (true, _) => return Err(ToolError::InvalidInput(NO_FIELD_FOR_THE_CODE.into())),
        (false, Some(_)) => return Err(ToolError::InvalidInput(NO_CODE_FOR_THE_FIELD.into())),
        (false, None) => {}
    }
    if let Some(ms) = input.timeout_ms {
        push(argv, &["--timeout-ms", &ms.to_string()]);
    }
    Ok(true)
}

/// A walk toward a goal: the CLI's own verb and flags, each given only when
/// the model gave it — what a walk needs (a goal, one screen) and what it
/// may not have is the core's parser to say, never a second table here.
fn argv_walk(action: &str, input: &ComputerInput, argv: &mut Vec<String>) -> bool {
    if !methods(action).any(|method| method == ComputerMethod::Walk) {
        return false;
    }
    push(argv, &[WALK_ACTION]);
    for (flag, value) in [
        ("--goal", input.goal.as_deref()),
        ("--app", input.app.as_deref()),
        ("--pane", input.pane.as_deref()),
        ("--platform", input.platform.as_deref()),
        ("--device", input.device.as_deref()),
        ("--until", input.until.as_deref()),
    ] {
        if let Some(value) = value {
            push(argv, &[flag, value]);
        }
    }
    if let Some(steps) = input.max_steps {
        push(argv, &["--steps", &steps.to_string()]);
    }
    for (flag, asked) in [(WALK_OVERLAP_FLAG, input.overlap), (WALK_RESCUE_FLAG, input.rescue), (WALK_REPLAY_FLAG, input.replay), ("background", input.background)] {
        if asked == Some(true) {
            argv.push(format!("--{flag}"));
        }
    }
    true
}

/// Recipes: keep a procedure that worked, list and show them, walk one.
fn argv_recipes(action: &str, input: &ComputerInput, argv: &mut Vec<String>) -> Result<bool, ToolError> {
    let method = match ACTIONS.iter().find(|(name, _)| *name == action) {
        Some((_, method @ (ComputerMethod::RecipeSave | ComputerMethod::RecipeList | ComputerMethod::RecipeShow | ComputerMethod::RecipeRun))) => *method,
        _ => return Ok(false),
    };
    push(argv, &[method.verb_name()]);
    if method != ComputerMethod::RecipeList {
        push(argv, &["--name", need(input.name.as_deref(), action, "name")?]);
    }
    match method {
        ComputerMethod::RecipeSave => {
            if let Some(note) = input.text.as_deref() {
                push(argv, &["--note", note]);
            }
            if let Some(last) = input.last {
                push(argv, &["--last", &last.to_string()]);
            }
        }
        ComputerMethod::RecipeRun => {
            if let Some(params) = input.params.as_ref() {
                push(argv, &["--params", &Value::Object(params.clone()).to_string()]);
            }
            if let Some(start) = input.start {
                push(argv, &["--start", &start.to_string()]);
            }
        }
        _ => {}
    }
    Ok(true)
}

// Seconds are validated finite and positive first; rounding to whole
// milliseconds is the point of the cast.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn seconds_to_ms(duration: Option<f64>, action: &str) -> Result<u64, ToolError> {
    let seconds = duration.ok_or_else(|| ToolError::InvalidInput(format!("`{action}` needs `duration` in seconds")))?;
    if !seconds.is_finite() || seconds <= 0.0 {
        return Err(ToolError::InvalidInput(format!("`{action}` needs a positive `duration`")));
    }
    Ok((seconds * 1000.0).round() as u64)
}

/// One trip down the shim: the command, its JSON envelope back, bounded.
fn call_shim(road: &ComputerRoad, argv: &[String], cwd: Option<&Path>) -> Result<Value, ToolError> {
    let trusted = crate::surface_shim::is_installed(&road.program, zerocode_core::agent_shims::SurfaceShim::Computer);
    let mut command = std::process::Command::new(&road.program);
    command
        .args(argv)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(cwd) = cwd.filter(|dir| dir.is_dir()) {
        command.current_dir(cwd);
    }
    let started = Instant::now();
    let mut child = command.spawn().map_err(|error| {
        ToolError::Execution(format!("could not run `{}`: {error}", road.program.display()))
    })?;
    let mut stdout = child.stdout.take();
    let mut stderr = child.stderr.take();
    // The answer is whole when the shim closes its stdout; waiting on that
    // (rather than polling the process) returns the moment it does.
    let (closed, answered) = std::sync::mpsc::channel::<()>();
    let out_reader = std::thread::spawn(move || {
        use std::io::Read as _;
        let mut bytes = Vec::new();
        if let Some(pipe) = stdout.as_mut() {
            let _ = pipe.read_to_end(&mut bytes);
        }
        let _ = closed.send(());
        bytes
    });
    let err_reader = std::thread::spawn(move || {
        use std::io::Read as _;
        let mut bytes = Vec::new();
        if let Some(pipe) = stderr.as_mut() {
            let _ = pipe.read_to_end(&mut bytes);
        }
        bytes
    });
    // The bridge's own ladder for this command, and the answer's trip back:
    // a handoff waits for the person, a click for the window's question.
    let budget = Duration::from_millis(computer_deadline_ms(argv) + COMPUTER_BRIDGE_GRACE_MS);
    let timed_out = |child: &mut std::process::Child| {
        let _ = child.kill();
        let _ = child.wait();
        ToolError::Execution(format!(
            "`{COMPUTER_SHIM} {}` gave no answer within {} s",
            argv.join(" "),
            budget.as_secs()
        ))
    };
    if let Err(std::sync::mpsc::RecvTimeoutError::Timeout) =
        answered.recv_timeout(budget.saturating_sub(started.elapsed()))
    {
        return Err(timed_out(&mut child));
    }
    // The answer is whole; the exit is a moment away — still inside the budget.
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if started.elapsed() >= budget => return Err(timed_out(&mut child)),
            Ok(None) => std::thread::sleep(EXIT_POLL),
            Err(_) => break None,
        }
    };
    let stdout = String::from_utf8_lossy(&out_reader.join().unwrap_or_default()).into_owned();
    let stderr = String::from_utf8_lossy(&err_reader.join().unwrap_or_default()).into_owned();
    // The envelope: on stdout when the window answered, on stderr when it
    // refused (`{"ok":false,"error":{...}}`); anything else is the shim's
    // own words (no window, no token) and is reported as they are.
    let envelope = zerocode_core::computer_use_protocol::answer_envelope(&stdout, &stderr);
    match envelope {
        Some(mut envelope) => {
            if !trusted {
                if let Some(object) = envelope.as_object_mut() { object.remove("surfaceExecution"); }
            }
            super::computer_tally::receipt(&envelope);
            Ok(envelope)
        },
        None => Err(ToolError::Execution(format!(
            "`{COMPUTER_SHIM} {}` answered without an envelope (exit {}): {}",
            argv.join(" "),
            status.and_then(|status| status.code()).map_or("?".to_string(), |code| code.to_string()),
            [stderr.trim(), stdout.trim()].iter().find(|text| !text.is_empty()).unwrap_or(&"(nothing)")
        ))),
    }
}

/// The private PNG a window's answer points at, if any.
fn screenshot_path(answer: &Value) -> Option<String> {
    answer
        .pointer("/result/screenshot/path")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// Stage a screenshot the window exported (a private PNG file) as an image
/// block, through the same guard `read_image` uses; the file is then gone.
fn stage_screenshot(answer: &mut Value, ctx: &ToolContext) -> Option<Value> {
    let path = screenshot_path(answer)?;
    let staged = crate::file_tools::stage_image(
        &serde_json::from_value(json!({ "path": path })).ok()?,
        ctx,
        crate::file_tools::ImageIntake::Screenshot,
    )
    .ok()
    .and_then(|text| serde_json::from_str::<Value>(&text).ok());
    let _ = std::fs::remove_file(&path);
    if let Some(screenshot) = answer
        .pointer_mut("/result/screenshot")
        .and_then(Value::as_object_mut)
    {
        screenshot.remove("path");
        screenshot.insert("staged".into(), Value::Bool(staged.is_some()));
    }
    staged
}

/// Present a look once. Provider diffs are per target; only the last picture
/// actually staged for this model may be omitted. Failed staging leaves its
/// coordinate frame intact and invalidates the baseline for the next look.
fn present_observation(
    answer: &mut Value,
    ctx: &ToolContext,
    app: Option<&str>,
    window_id: Option<u64>,
    diff: bool,
) -> Option<Value> {
    let window_id = checked_window(answer, None).ok().flatten().or(window_id);
    // An app's look is the app the window resolved, whatever name the
    // model asked it by — one window under three names is one observation.
    let app = answer
        .pointer("/result/app/bundleId")
        .and_then(Value::as_str)
        .or(app)
        .map(str::to_string);
    let observation = answer.get("result").and_then(ShotFrame::from_answer).map(|frame| ComputerObservation {
        app: app.clone(),
        window_id,
        frame,
        marked_look: answer.pointer(&format!("/result/marks/{LOOK_ID_KEY}")).and_then(Value::as_str).map(str::to_string),
        title: window_title(answer),
        document: window_document(answer),
    });
    let unchanged = diff && answer.pointer("/result/changed").and_then(Value::as_array).is_some_and(Vec::is_empty);
    if unchanged && observation.as_ref().is_some_and(|seen| ctx.has_computer_observation(seen)) {
        discard_screenshot(answer);
        return None;
    }
    let staged = stage_screenshot(answer, ctx);
    if staged.is_some() {
        if let Some(seen) = observation.as_ref() {
            ctx.set_computer_frame(seen.frame);
        }
        ctx.set_computer_observation(observation);
    } else {
        ctx.set_computer_observation(None);
    }
    staged
}

/// A window's refusal: its code and its words.
struct Refusal {
    code: String,
    message: String,
}

impl From<Refusal> for ToolError {
    fn from(refusal: Refusal) -> Self {
        ToolError::Execution(format!("{}: {}", refusal.code, refusal.message))
    }
}

/// Why a look did not answer: the window refused it, or the road failed.
enum LookError {
    Refused(Refusal),
    Road(ToolError),
}

impl From<LookError> for ToolError {
    fn from(error: LookError) -> Self {
        match error {
            LookError::Refused(refusal) => refusal.into(),
            LookError::Road(error) => error,
        }
    }
}

/// A window answer's refusal, if it is one.
fn refusal(answer: &Value) -> Option<Refusal> {
    (answer.get("ok").and_then(Value::as_bool) != Some(true)).then(|| Refusal {
        code: answer.pointer("/error/code").and_then(Value::as_str).unwrap_or("error").to_string(),
        message: answer.pointer("/error/message").and_then(Value::as_str).unwrap_or("").to_string(),
    })
}

/// An answer as the model is shown it: in the frame's pixels, without the
/// frame's own words.
fn shown(mut value: Value, frame: Option<ShotFrame>) -> Value {
    fn unframe(value: &mut Value) {
        match value {
            Value::Object(object) => {
                for key in FRAME_KEYS {
                    object.remove(*key);
                }
                object.values_mut().for_each(unframe);
            }
            Value::Array(items) => items.iter_mut().for_each(unframe),
            _ => {}
        }
    }
    if let Some(frame) = frame {
        frame.pixelize(&mut value);
    }
    unframe(&mut value);
    value
}

/// A picture the model will not be shown: the file goes, and so does its name.
fn discard_screenshot(answer: &mut Value) {
    if let Some(path) = screenshot_path(answer) {
        let _ = std::fs::remove_file(path);
    }
    if let Some(screenshot) = answer.pointer_mut("/result/screenshot").and_then(Value::as_object_mut) {
        screenshot.remove("path");
        screenshot.insert("staged".into(), Value::Bool(false));
    }
}

/// A marked look as the model reads it: one legend line per mark, in the
/// picture's pixels, and not the tool's own words; its id is the tool's to
/// keep for a click by number.
fn present_marks(screen: &mut Value, ctx: &ToolContext) {
    // The numbers the model may click by are the ones on the picture it was
    // just shown: a picture with none — marks unavailable, or no marks asked —
    // leaves it none, never an older look's.
    let look = screen.pointer(&format!("/marks/{LOOK_ID_KEY}")).and_then(Value::as_str).map(str::to_string);
    ctx.set_computer_marked_look(look.as_deref());
    let Some(marks) = screen.get_mut("marks").and_then(Value::as_object_mut) else {
        return;
    };
    let legend: Vec<Value> = marks
        .get(ITEMS_KEY)
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(legend_line).map(Value::String).collect())
        .unwrap_or_default();
    for key in TOOL_ONLY_KEYS.iter().chain(&[LOOK_ID_KEY, "app"]) {
        marks.remove(*key);
    }
    if !legend.is_empty() {
        marks.insert(LEGEND_KEY.into(), Value::Array(legend));
    }
}

/// A click by mark names the look its number was read from — the last marked
/// look this model was shown.
fn with_marked_look(mut argv: Vec<String>, ctx: &ToolContext) -> Result<Vec<String>, ToolError> {
    if argv.first().map(String::as_str) != Some("click") || !argv.iter().any(|word| word == "--mark") {
        return Ok(argv);
    }
    let look = ctx.computer_marked_look().ok_or_else(|| {
        ToolError::InvalidInput("no marked look yet: a `mark` is a number on a look that carried marks".into())
    })?;
    let at = argv.iter().position(|word| word == "--json").unwrap_or(argv.len());
    argv.splice(at..at, ["--look".to_string(), look]);
    Ok(argv)
}

/// One desktop look for the model, as this model's viewer: its frame becomes
/// the one the model speaks in. With `--diff`, a look that found nothing
/// changed since the model's own last look is not shown — the model's last
/// picture is still the screen — and says so (`changed` is null when there
/// was nothing to compare with).
struct ComputerLook {
    value: Value,
    surface: Option<SurfaceExecution>,
}

fn host_surface_execution(answer: &Value) -> Option<SurfaceExecution> {
    serde_json::from_value::<SurfaceExecution>(answer.get("surfaceExecution")?.clone())
        .ok().filter(SurfaceExecution::is_valid)
}

fn look(road: &ComputerRoad, ctx: &ToolContext, words: &[&str], target: Option<&ComputerTarget>) -> Result<Value, LookError> {
    look_execution(road, ctx, words, target).map(|look| look.value)
}

fn look_execution(road: &ComputerRoad, ctx: &ToolContext, words: &[&str], target: Option<&ComputerTarget>) -> Result<ComputerLook, LookError> {
    let mut asked = words.to_vec();
    let window_id = target.and_then(|target| target.window_id).map(|window_id| window_id.to_string());
    if let Some(target) = target {
        asked.extend(["--app", target.app.as_str()]);
    }
    if let Some(window_id) = window_id.as_deref() {
        asked.extend(["--window-id", window_id]);
    }
    let argv = look_argv(&asked, looks_carry_marks());
    let mut answer = call_shim(road, &as_viewer(argv, ctx), ctx.cwd.as_deref()).map_err(LookError::Road)?;
    if let Some(refused) = refusal(&answer) {
        return Err(LookError::Refused(refused));
    }
    checked_window(&answer, target.and_then(|target| target.window_id)).map_err(LookError::Road)?;
    let surface = host_surface_execution(&answer);
    let changed = if words.contains(&LOOK_DIFF_FLAG) {
        answer.pointer("/result/changed").and_then(Value::as_array).map(|regions| !regions.is_empty())
    } else {
        None
    };
    let staged = present_observation(&mut answer, ctx, target.map(|target| target.app.as_str()), target.and_then(|target| target.window_id), words.contains(&LOOK_DIFF_FLAG));
    let mut screen = shown(answer.get("result").cloned().unwrap_or(Value::Null), ctx.computer_frame());
    // This look is a picture: an app's whole tree is the model's to ask for
    // (`observe` with `app`), not a cost every look after a batch pays.
    if let Some(tree) = screen.get_mut("tree").and_then(Value::as_object_mut) {
        tree.remove("text");
    }
    // A look that showed no new picture leaves the model's numbers as they were.
    if staged.is_some() {
        present_marks(&mut screen, ctx);
    }
    // How the window waited is the model's business only when the screen
    // was still moving at the cap: the picture shows it mid-motion.
    if screen.pointer("/settle/settled") == Some(&Value::Bool(true)) {
        if let Some(screen) = screen.as_object_mut() {
            screen.remove("settle");
        }
    }
    Ok(ComputerLook { value: json!({
        "staged": staged.is_some(),
        "changed": changed,
        "screen": screen,
    }), surface })
}

/// A look's command line: numbered when this process's looks carry marks.
fn look_argv(words: &[&str], marks: bool) -> Vec<String> {
    let marks = marks.then_some(MARKS_FLAG);
    words.iter().copied().chain(marks).chain(["--json"]).map(str::to_string).collect()
}

/// Whether this process's looks number the controls they show: the core's
/// switch, unless the process was started with `ZO_COMPUTER_MARKS`.
fn looks_carry_marks() -> bool {
    zerocode_core::computer_use::looks_carry_marks(std::env::var(COMPUTER_MARKS_ENV).ok().as_deref())
}

/// The look after an act. The window waits for what the act did to finish
/// painting (`observe --settle`: on the eye's repaints, or by looking until
/// the table's settle), then looks once; nothing changed says
/// `changed: false`. It looks at `app`'s window when the acts were in one.
fn look_after(road: &ComputerRoad, ctx: &ToolContext, settle: bool, target: Option<&ComputerTarget>) -> Value {
    look_after_execution(road, ctx, settle, target).value
}

fn look_after_execution(road: &ComputerRoad, ctx: &ToolContext, settle: bool, target: Option<&ComputerTarget>) -> ComputerLook {
    let words = if settle { LOOK_AFTER_ARGV } else { FRAME_LOOK_ARGV };
    let mut answer =
        look_execution(road, ctx, words, target).unwrap_or_else(|error| ComputerLook {
            value: json!({ "error": ToolError::from(error).to_string() }),
            surface: None,
        });
    if !settle {
        answer.value["waited_for_settle"] = Value::Bool(false);
    }
    answer
}

/// The look that learns the frame, when the model names a place (or asks for
/// an answer that will) before this process has shown it a picture — so its
/// pixels and the hand's points agree. A screen that cannot be seen (no
/// screen recording) is still one the hand can move: the frame is then the
/// screen itself, and what goes in and what comes out stay in points.
fn first_look(inputs: &[&ComputerInput], ctx: &ToolContext, road: &ComputerRoad) -> Result<Option<Value>, ToolError> {
    let Some(input) = inputs.iter().find(|input| needs_a_frame(input)) else {
        return Ok(None);
    };
    let target = target_of(input);
    if ctx.computer_frame().is_some() {
        if target.as_ref().is_none_or(|target| ctx.computer_target_frame(target).is_some()) {
            return Ok(None);
        }
        if speaks_in_pixels(input) {
            return Err(ToolError::InvalidInput("the last picture is not of this app/window; observe the selected window before using its coordinates".into()));
        }
    }
    match look(road, ctx, FRAME_LOOK_ARGV, target.as_ref()) {
        Ok(seen) if seen["staged"] != true || ctx.computer_frame().is_none() => Err(ToolError::Execution(
            "the first desktop picture could not be shown with its coordinate frame; observe again before acting in picture pixels".into(),
        )),
        Ok(seen) => Ok(Some(seen)),
        Err(LookError::Refused(refused)) if refused.code == error_code::PERMISSION_DENIED && target.is_none() => {
            ctx.set_computer_frame(ShotFrame::UNIT);
            Ok(Some(json!({ "blind": true, "why": refused.message })))
        }
        Err(error) => Err(error.into()),
    }
}

/// The app the look after acts is of (t-37883): the last app the acts named
/// (a quit's leaves no window to look at), else the one the model last
/// looked at — so a batch on a phone's mirrored window is shown that window,
/// settled and legible, not the whole desktop the model would only look at
/// again. A timer's context cannot redirect that observation.
fn acted_in(acts: &[&ComputerInput], ctx: &ToolContext) -> Option<ComputerTarget> {
    acts.iter()
        .rev()
        .filter(|act| act.action.trim() != QUIT_ACTION && !timed_wait(act.action.trim()))
        .find_map(|act| target_of(act))
        .or_else(|| ctx.computer_observed_target())
}

/// A batch's command line: each step spelled exactly as the same action
/// alone, in the same frame — the core's batch verb and flag, the window's
/// checks.
fn batch_argv(steps: &[ComputerInput], frame: ShotFrame) -> Result<Vec<String>, ToolError> {
    check_batch_len(steps.len())?;
    let commands = steps
        .iter()
        .enumerate()
        .map(|(at, step)| {
            let action = step.action.trim();
            if action == BATCH_ACTION || step.steps.is_some() {
                return Err(ToolError::InvalidInput(format!("step {}: a batch holds actions, not batches", at + 1)));
            }
            if step.settle.is_some() {
                return Err(ToolError::InvalidInput("`settle` belongs on the batch, whose steps share one look after".into()));
            }
            if step.mark.is_some() || step.element_index.is_some() {
                return Err(ToolError::InvalidInput(format!(
                    "step {}: a click by `mark` or `element_index` goes one at a time — the look or tree it names can change between steps",
                    at + 1
                )));
            }
            in_points(step.clone(), frame)
                .and_then(|step| argv_for(&step))
                .map_err(|error| ToolError::InvalidInput(format!("step {} ({action}): {error}", at + 1)))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(vec![
        ComputerMethod::Batch.verb_name().to_string(),
        format!("--{BATCH_COMMANDS_FLAG}"),
        serde_json::to_string(&commands).map_err(|error| ToolError::Execution(error.to_string()))?,
        "--json".to_string(),
    ])
}

fn check_batch_len(count: usize) -> Result<(), ToolError> {
    if count == 0 || count > COMPUTER_BATCH_MAX_STEPS {
        return Err(ToolError::InvalidInput(format!(
            "a batch holds 1 to {COMPUTER_BATCH_MAX_STEPS} steps (this one has {count})"
        )));
    }
    Ok(())
}

/// One batch: checked whole by the core before any look is paid, one trip
/// down the shim, one look after — and each step answered under the model's
/// own action name, in its picture's pixels.
fn run_computer_batch(mut input: ComputerInput, ctx: &ToolContext, road: &ComputerRoad) -> Result<String, ToolError> {
    let mut steps = input
        .steps
        .take()
        .ok_or_else(|| ToolError::InvalidInput("`batch` needs `steps`: the actions to run in order".into()))?;
    check_batch_len(steps.len())?;
    if input.window.is_some() {
        return Err(ToolError::InvalidInput("`window` is a wait_for title condition, not a batch default; select an observed app/window_id pair".into()));
    }
    let default_target = match (input.app.as_deref(), input.window_id) {
        (None, None) => None,
        (Some(app), Some(window_id)) if !app.trim().is_empty() && window_id > 0 => {
            Some(ComputerTarget { app: app.to_string(), window_id: Some(window_id) })
        }
        _ => return Err(ToolError::InvalidInput("batch window defaults need both a nonempty app and a positive window_id from an observation; otherwise scope each step".into())),
    };
    let settle = input.settle.unwrap_or(SETTLE_AFTER_ACT);
    for (at, step) in steps.iter_mut().enumerate() {
        step.browser_chrome = step.browser_chrome.or(input.browser_chrome);
        if let Some(target) = &default_target {
            if step.app.as_deref().is_some_and(|app| app != target.app.as_str())
                || step.window_id.is_some_and(|window_id| Some(window_id) != target.window_id)
            {
                return Err(ToolError::InvalidInput(format!("step {}: app/window_id conflicts with the batch defaults; use separate batches or scope every step without batch defaults", at + 1)));
            }
            step.app.get_or_insert_with(|| target.app.clone());
            step.window_id = target.window_id;
        }
        if input.background == Some(true) {
            if step.background == Some(false) {
                return Err(ToolError::InvalidInput("a background batch cannot contain a foreground step".into()));
            }
            if !background_neutral_action(step.action.trim()) {
                step.background = Some(true);
            }
        }
        if step.app.is_some() && (step.coordinate.is_some() || step.start_coordinate.is_some()) {
            return Err(ToolError::InvalidInput("an app-coordinate action needs its own observed window frame; run it separately or use label-based batch steps".into()));
        }
    }
    zerocode_core::computer_use::parse_command(&batch_argv(&steps, ShotFrame::UNIT)?).map_err(ToolError::InvalidInput)?;
    let first_look = first_look(&steps.iter().collect::<Vec<_>>(), ctx, road)?;
    let frame = ctx.computer_frame().unwrap_or(ShotFrame::UNIT);
    let answer = call_shim(road, &batch_argv(&steps, frame)?, ctx.cwd.as_deref())?;
    let mut reports = answer
        .pointer("/result/steps")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let ran = reports.iter().filter(|report| report["ok"] == Value::Bool(true)).count();
    if let Some(refused) = refusal(&answer).filter(|_| ran == 0) {
        return Err(refused.into());
    }
    for report in &mut reports {
        let n = report["n"].as_u64().and_then(|n| usize::try_from(n).ok()).unwrap_or(0);
        if let Some(step) = n.checked_sub(1).and_then(|at| steps.get(at)) {
            if report["ok"] == Value::Bool(true) && step.action.trim() != "window" && !timed_wait(step.action.trim()) {
                if let Err(error) = checked_window(report.get("result").unwrap_or(&Value::Null), step.window_id) {
                    if steps.iter().take(ran).any(|step| acts(step.action.trim())) {
                        return unverified_action_receipt(BATCH_ACTION, step.window_id, &error, ran);
                    }
                    return Err(error);
                }
            }
            report["verb"] = Value::String(step.action.trim().to_string());
        }
    }
    let reports = shown(Value::Array(reports), ctx.computer_frame());
    let refused = refusal(&answer).map(|refused| {
        let at = answer.pointer("/result/refusedAt").cloned().unwrap_or(Value::Null);
        let action = at
            .as_u64()
            .and_then(|n| usize::try_from(n).ok())
            .and_then(|n| n.checked_sub(1))
            .and_then(|at| steps.get(at))
            .map(|step| step.action.trim().to_string());
        json!({ "step": at, "action": action, "code": refused.code, "message": refused.message })
    });
    let ran_steps: Vec<&ComputerInput> = steps.iter().take(ran).collect();
    let acted = ran_steps.iter().any(|step| acts(step.action.trim()));
    let looked_after = (LOOK_AFTER_ACT && acted).then(|| look_after(road, ctx, settle, acted_in(&ran_steps, ctx).as_ref()));
    to_pretty_json(json!({
        "ok": refused.is_none(),
        "action": BATCH_ACTION,
        "ran": ran,
        "of": steps.len(),
        "refused": refused,
        "steps": reports,
        "first_look": first_look,
        "looked_after": looked_after,
    }))
}

/// What a walk came to, for the model to act on (docs/design/
/// computer-use-fast-path-20260926.md §4.3): the window's answer read into
/// one word, apart from whether the call went through. Nothing here counts a
/// press, an `ok` or the judgment's own `done` as the goal reached.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WalkStatus {
    /// The caller's own condition was read on the screen, and read as the
    /// walk's doing: absent before it, or absent at a check between presses.
    Verified,
    /// The judgment said `done`, or the condition already held before the
    /// walk: look once to confirm, nothing more.
    NeedsVerification,
    /// The walk could not go on — the seat off, no judge, recording only, the
    /// judgment unsure or unanswered, nothing to press — or its look came
    /// back empty and so did zo's own (t-70459): plan from the reason, or
    /// take the ladder's roads in order.
    NeedsFallback,
    /// A person's stop, a guard's refusal, or the budget spent: end as a stop
    /// ends.
    Stopped,
    /// A press or a look failed, or the screen would not move: recover from
    /// the step it names — after `no_look` or `stuck`, from the tree zo's
    /// own look read.
    Failed,
}

impl WalkStatus {
    #[cfg(test)]
    pub(crate) const ALL: [Self; 5] =
        [Self::Verified, Self::NeedsVerification, Self::NeedsFallback, Self::Stopped, Self::Failed];

    pub(crate) const fn word(self) -> &'static str {
        match self {
            Self::Verified => "verified",
            Self::NeedsVerification => "needs_verification",
            Self::NeedsFallback => "needs_fallback",
            Self::Stopped => "stopped",
            Self::Failed => "failed",
        }
    }

    /// Whether the screen after the walk is one nobody has read yet: the walk
    /// pressed, and did not end on a screen it read as the goal.
    const fn leaves_an_unread_screen(self) -> bool {
        matches!(self, Self::NeedsFallback | Self::Stopped | Self::Failed)
    }
}

/// The window's refusals that are a stop — the person's, their answer to a
/// confirmation, or the budget — rather than a failure.
const WALK_STOPS: &[&str] = &[
    error_code::STOPPED,
    error_code::CONFIRMATION_REFUSED,
    error_code::CONFIRMATION_TIMEOUT,
    error_code::BUDGET_EXCEEDED,
];
/// The tool's own reasons, where the window's answer has none to give: the
/// seat is off; the seat is on but there is no judge to ask (no key); the
/// condition already held before the walk; a press the hand could not make;
/// every step spent without reaching the goal.
const SEAT_OFF: &str = "seat_off";
const NO_JUDGE: &str = "no_judge";
const UNTIL_HELD_BEFORE: &str = "until_held_before";
const PRESS_FAILED: &str = "press_failed";
const STEPS_SPENT: &str = "steps_spent";
/// How a verified walk got there: the caller's `until`, by the field's name.
const REACHED_BY_UNTIL: &str = "until";

/// A walk's answer read into its status and the facts beside it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct WalkRead {
    pub status: WalkStatus,
    pub reason: Option<String>,
    pub pressed: u64,
    pub reached_by: Option<String>,
    /// The step the walk ended at, counted from 1.
    pub step: Option<u64>,
    /// The presses the walk's budget still held.
    pub steps_left: u64,
    /// The last step whose look the judgment answered on.
    pub last_observed_step: Option<u64>,
    /// What a `no_look` row's reason says its look ran into, when it says
    /// anything (t-70459), and — when a door refused that look — the door's
    /// own code beside it.
    pub cause: Option<String>,
    pub refusal: Option<String>,
}

/// Read the window's answer to a walk whose budget was `budget` steps.
pub(crate) fn read_walk(answer: &Value, budget: u64) -> WalkRead {
    let said = answer.get("result").unwrap_or(&Value::Null);
    let rows = said.get(walk_words::ROWS).and_then(Value::as_array).map_or(&[][..], Vec::as_slice);
    let word = |row: &Value, key: &str| row.get(key).and_then(Value::as_str).map(str::to_string);
    let step = rows.iter().filter_map(|row| row.get(walk_words::ATTEMPT)?.as_u64()).max();
    let mut read = WalkRead {
        status: WalkStatus::Failed,
        reason: None,
        pressed: said.get(walk_words::PRESSED).and_then(Value::as_u64).unwrap_or_default(),
        reached_by: None,
        step,
        steps_left: budget.saturating_sub(step.unwrap_or_default()),
        last_observed_step: rows
            .iter()
            .rev()
            .find(|row| word(row, walk_words::OUTCOME).as_deref() == Some(zerocode_core::jev::summary::ANSWERED))
            .and_then(|row| row.get(walk_words::ATTEMPT)?.as_u64()),
        cause: None,
        refusal: None,
    };
    if let Some(refused) = refusal(answer) {
        read.status = if WALK_STOPS.contains(&refused.code.as_str()) {
            WalkStatus::Stopped
        } else if refused.code == error_code::PERMISSION_DENIED {
            WalkStatus::NeedsFallback
        } else {
            WalkStatus::Failed
        };
        read.reason = Some(refused.code);
        return read;
    }
    if said.pointer("/background/interrupted") == Some(&Value::Bool(true)) {
        read.status = WalkStatus::Stopped;
        read.reason = Some(walk_words::BACKGROUND_INTERRUPTED.into());
        return read;
    }
    let last = rows.last();
    let recheck = |row: &Value| row.get(walk_words::RECHECK).and_then(Value::as_bool);
    let (status, reason) = if said.get(walk_words::REACHED) == Some(&Value::Bool(true)) {
        if let Some(by) = last.and_then(|row| word(row, walk_words::REACHED_BY)) {
            read.reached_by = Some(by);
            (WalkStatus::NeedsVerification, None)
        } else if last.and_then(recheck) == Some(true) {
            read.reached_by = Some(REACHED_BY_UNTIL.to_string());
            let absent_before = said.get(walk_words::UNTIL_BEFORE) == Some(&Value::Bool(false));
            if absent_before || rows.iter().any(|row| recheck(row) == Some(false)) {
                (WalkStatus::Verified, None)
            } else {
                (WalkStatus::NeedsVerification, Some(UNTIL_HELD_BEFORE.to_string()))
            }
        } else {
            (WalkStatus::NeedsVerification, None)
        }
    } else if said.get(walk_words::MODE).and_then(Value::as_str) == Some(JevMode::Off.key()) {
        (WalkStatus::NeedsFallback, Some(SEAT_OFF.to_string()))
    } else if let Some(row) = last {
        let outcome = word(row, walk_words::OUTCOME);
        let barred = word(row, zerocode_core::jev::summary::BARRED.canonical);
        let why = word(row, walk_words::REASON);
        if let Some(outcome) = outcome.clone().filter(|outcome| [walk_words::STUCK, walk_words::NO_LOOK].contains(&outcome.as_str())) {
            read.cause = why.filter(|why| !why.trim().is_empty());
            read.refusal = word(row, walk_words::REFUSAL).filter(|code| !code.trim().is_empty());
            (WalkStatus::Failed, Some(outcome))
        } else if let Some(barred) = barred {
            let status = if barred == walk_words::LOW_CONFIDENCE { WalkStatus::NeedsFallback } else { WalkStatus::Stopped };
            (status, Some(why.unwrap_or(barred)))
        } else if let Some(why) = why {
            (WalkStatus::NeedsFallback, Some(why))
        } else if let Some(outcome) = outcome.filter(|outcome| outcome != zerocode_core::jev::summary::ANSWERED) {
            (WalkStatus::NeedsFallback, Some(outcome))
        } else if word(row, walk_words::CHOSEN).as_deref() == Some(GIVE_UP) {
            (WalkStatus::NeedsFallback, Some(GIVE_UP.to_string()))
        } else if row.get(walk_words::PRESSED) == Some(&Value::Bool(false)) {
            (WalkStatus::Failed, Some(PRESS_FAILED.to_string()))
        } else {
            (WalkStatus::Stopped, Some(STEPS_SPENT.to_string()))
        }
    } else {
        (WalkStatus::NeedsFallback, Some(NO_JUDGE.to_string()))
    };
    read.status = status;
    read.reason = reason;
    read
}

/// One walk: checked by the core's own parser before anything moves, one trip
/// down the shim — the window walks it, observing, choosing, pressing and
/// checking on its own marks — and one answer. No picture goes before it (a
/// walk speaks in no pixels) and none after, unless it pressed at an app and
/// ended on a screen nobody read, or the ladder after `no_look` or `stuck`
/// looks once more ([`climb`]) — then that look is the one after.
fn run_computer_walk(input: &ComputerInput, ctx: &ToolContext, road: &ComputerRoad, settle: bool) -> Result<String, ToolError> {
    let argv = argv_for(input)?;
    let command = zerocode_core::computer_use::parse_command(&argv).map_err(ToolError::InvalidInput)?;
    let budget = u64::try_from(walk_steps(&command.params)).unwrap_or(u64::MAX);
    let answer = call_shim(road, &argv, ctx.cwd.as_deref())?;
    let mut walked = read_walk(&answer, budget);
    let ladder = climb(input, &mut walked, ctx, road);
    let relooked = ladder.as_ref().is_some_and(|ladder| ladder.get(RELOOKED).is_some());
    let (surface, target) = [("app", &input.app), ("pane", &input.pane), ("device", &input.device)]
        .into_iter()
        .find_map(|(surface, target)| target.as_deref().map(|target| (surface, target)))
        .unwrap_or_default();
    let looked_after = (LOOK_AFTER_ACT && walked.pressed > 0 && walked.status.leaves_an_unread_screen() && input.app.is_some() && !relooked)
        .then(|| look_after(road, ctx, settle, target_of(input).as_ref()));
    let mut result = answer.get("result").cloned().unwrap_or(Value::Null);
    let last = result.get(walk_words::ROWS).and_then(Value::as_array).and_then(|rows| rows.last()).cloned();
    if let Some(result) = result.as_object_mut() {
        result.remove(walk_words::ROWS);
    }
    let mut said = json!({
        "action": WALK_ACTION,
        "status": walked.status.word(),
        "reason": walked.reason,
        "pressed": walked.pressed,
        "reachedBy": walked.reached_by,
        "at": { (surface): target, "step": walked.step },
        "stepsLeft": walked.steps_left,
        "lastObservedStep": walked.last_observed_step,
        "result": result,
        "lastRow": last,
        "looked_after": looked_after,
    });
    if let Some(ladder) = ladder {
        said["ladder"] = ladder;
    }
    to_pretty_json(said)
}

/// A door's refusal zo reads as any other, reported as that door's defect,
/// which a test names: the window's own command road could not reach the
/// window (t-70855). The rest of a `no_look` row's words are the core's
/// `walk_words` (t-70425); the core spells this one beside them once t-70855
/// lands.
#[cfg(test)]
const DOOR_CONNECTION_FAILED: &str = "door_connection_failed";
/// The ladder's own words (t-70459): why the walk ended, the look zo took,
/// and what to do next.
const NO_LOOK_WHY: &str = "no look came back";
const STUCK_WHY: &str = "the screen did not move";
const RELOOKED: &str = "relooked";
const LADDER_CONTINUE: &str =
    "zo looked once more and read the window's tree: continue from relooked — press by element_index, or by label and role";
const LADDER_ROADS: &str = "take the roads in order; asking the person is the last";
const LADDER_STOP: &str = "a second look meets the same door: stop and report it";
const LADDER_DOOR_ROAD: &str = "the pane's own door may still look and press: take its road, and report the refusal";
const PERSON_TOOK_OVER: &str = "the person took the foreground; the background walk stopped";
const LADDER_PERSONS_TURN: &str = "the screen is the person's now: ask them with handoff before going on";
/// What the person is asked: the ladder's last road, and the one road left
/// when they took the foreground from the walk.
const ASK_THE_DESKTOP_APP: &str = "<what the person must do, e.g. open the document in its desktop app>";
const ASK_TO_GO_ON: &str = "<ask the person to let the walk go on, or to finish the step themselves>";

/// The ladder after a walk that ended `no_look` or `stuck`, climbed before
/// anyone asks the person (t-70459), by the cause its last row names. A
/// door's refusal is reported and never looked through again — at a pane
/// its own door's road stays. A pane's look is its door's, which zo's road
/// does not reach: its roads, in that door's words. At an app zo looks once
/// more — after finding a window that was not found, and never at a tree
/// already said to be empty — and a look that reads the window's tree is
/// the model's to continue from; otherwise the roads, in order, the person
/// last. A device's walk keeps its answer: no look of zo's reaches it, and
/// it has no desktop app to be opened in. `None`: no ladder to climb.
fn climb(input: &ComputerInput, walked: &mut WalkRead, ctx: &ToolContext, road: &ComputerRoad) -> Option<Value> {
    let after = walked
        .reason
        .clone()
        .filter(|reason| walked.status == WalkStatus::Failed && [walk_words::NO_LOOK, walk_words::STUCK].contains(&reason.as_str()))?;
    let why = if after == walk_words::STUCK { STUCK_WHY } else { NO_LOOK_WHY };
    let cause = walked.cause.clone();
    let mut ladder = json!({ "after": after, "cause": cause, "why": why });
    if let Some(cause) = cause.as_deref().filter(|cause| ![walk_words::NO_WINDOW, walk_words::EMPTY_TREE, walk_words::LOOK_TIMEOUT].contains(cause)) {
        ladder[walk_words::REFUSAL] = json!(walked.refusal);
        // The person took the foreground: no door failed, and the walk goes
        // on only when they say so.
        if cause == walk_words::LOOK_REFUSED && walked.refusal.as_deref() == Some(walk_words::BACKGROUND_INTERRUPTED) {
            walked.status = WalkStatus::Stopped;
            ladder["report"] = json!(PERSON_TOOK_OVER);
            ladder["roads"] = json!([person_road(ASK_TO_GO_ON)]);
            ladder["next"] = json!(LADDER_PERSONS_TURN);
            return Some(ladder);
        }
        // A refused look names its door's code beside it; a reason outside
        // the four is its own name.
        let code = if cause == walk_words::LOOK_REFUSED { walked.refusal.as_deref() } else { Some(cause) };
        let door = code.map_or_else(String::new, |code| format!(" {code}"));
        ladder["report"] = json!(format!("door refusal{door}: report it as a defect"));
        if let Some(pane) = input.pane.as_deref() {
            walked.status = WalkStatus::NeedsFallback;
            ladder["roads"] = pane_roads(pane, false);
            ladder["next"] = json!(LADDER_DOOR_ROAD);
        } else {
            walked.status = WalkStatus::Stopped;
            ladder["next"] = json!(LADDER_STOP);
        }
        return Some(ladder);
    }
    if let Some(pane) = input.pane.as_deref() {
        walked.status = WalkStatus::NeedsFallback;
        ladder["roads"] = pane_roads(pane, true);
        ladder["next"] = json!(LADDER_ROADS);
        return Some(ladder);
    }
    let app = input.app.as_deref()?;
    let mut window_id = None;
    if cause.as_deref() == Some(walk_words::NO_WINDOW) {
        let refound = refind(app, ctx, road);
        window_id = refound.pointer("/windows/0/id").and_then(Value::as_u64);
        ladder["refound"] = refound;
    }
    if cause.as_deref() != Some(walk_words::EMPTY_TREE) {
        let mut look = json!({ "action": "observe", "app": app });
        if let Some(window_id) = window_id {
            look["window_id"] = json!(window_id);
        }
        if input.background == Some(true) {
            look["background"] = json!(true);
        }
        let relooked = run_computer_inner(&look, ctx, road)
            .and_then(|seen| serde_json::from_str::<Value>(&seen.text).map_err(ToolError::from))
            .unwrap_or_else(|error| json!({ "error": error.to_string() }));
        window_id = checked_window(&relooked, None).ok().flatten().or(window_id);
        let tree = relooked.pointer("/result/tree");
        let reads_tree = tree.and_then(|tree| tree.get("elementCount")).and_then(Value::as_u64).is_some_and(|count| count > 0)
            || tree.and_then(|tree| tree.get("text")).and_then(Value::as_str).is_some_and(|text| !text.trim().is_empty());
        ladder[RELOOKED] = relooked;
        if reads_tree {
            ladder["next"] = json!(LADDER_CONTINUE);
            return Some(ladder);
        }
    }
    walked.status = WalkStatus::NeedsFallback;
    ladder["roads"] = app_roads(app, window_id, input.background == Some(true));
    ladder["next"] = json!(LADDER_ROADS);
    Some(ladder)
}

/// The walk's app's windows found again, before a look at a window the walk
/// did not find: each window's id and title — never its place, which no
/// picture has framed for the model.
fn refind(app: &str, ctx: &ToolContext, road: &ComputerRoad) -> Value {
    let list = ComputerInput { action: LIST_WINDOWS_ACTION.into(), app: Some(app.to_string()), ..ComputerInput::default() };
    let answer = match argv_for(&list).and_then(|argv| call_shim(road, &argv, ctx.cwd.as_deref())) {
        Ok(answer) => answer,
        Err(error) => return json!({ "error": error.to_string() }),
    };
    if let Some(refused) = refusal(&answer) {
        return json!({ "error": ToolError::from(refused).to_string() });
    }
    let windows: Vec<Value> = answer
        .pointer("/result/windows")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice)
        .iter()
        .filter(|window| window_of_app(window, app))
        .map(|window| json!({ "id": window.get("id"), "title": window.get("title") }))
        .collect();
    json!({ "windows": windows })
}

/// The person, asked by `text`: the ladder's last road, never its first.
fn person_road(text: &str) -> Value {
    json!({ "road": "person", "calls": [{ "action": "handoff", "text": text }] })
}

/// The roads at an app after a look that read no tree, in the order to take
/// them, each in the tool's own calls at the walk's window and in its
/// background — `<…>` is what the model fills in: the picture, pressed by its
/// marks before its pixels; the page's own address field — a spreadsheet's
/// name box, a search field — and the keys; the person last.
fn app_roads(app: &str, window_id: Option<u64>, background: bool) -> Value {
    let at = |mut call: Value| {
        call["app"] = json!(app);
        if let Some(window_id) = window_id {
            call["window_id"] = json!(window_id);
        }
        if background {
            call["background"] = json!(true);
        }
        call
    };
    let mut mark = json!({ "action": "left_click", "mark": "<its number on that picture's marks>" });
    if background {
        mark["background"] = json!(true);
    }
    // A press by the picture's pixels is held to the window that picture is of.
    let mut pixels = at(json!({ "action": "left_click", "coordinate": "<[x, y] in that picture's pixels, where no mark is>" }));
    if window_id.is_none() {
        pixels["window_id"] = json!("<the window_id the screenshot answered>");
    }
    json!([
        { "road": "picture", "calls": [
            at(json!({ "action": "screenshot" })),
            mark,
            pixels,
        ] },
        { "road": "address_field", "calls": [
            at(json!({ "action": "find", "label": "<the page's own address field: a spreadsheet's name box, a search field>" })),
            at(json!({ "action": "left_click", "label": "<that field's label>", "role": "<its role>" })),
            at(json!({ "action": "type", "text": "<the cell or the words to go to>" })),
            at(json!({ "action": "key", "text": "Return" })),
        ] },
        person_road(ASK_THE_DESKTOP_APP),
    ])
}

/// The roads at a browser pane, in the work browser's own door, which zo
/// runs as its browser road: zo's Computer road does not look at a pane (the
/// CLI's `observe` and `screenshot` take none), and that door looks and
/// presses there — its numbered controls and their picture, pressed by
/// number; the page's own fields — a spreadsheet's name box, a search field
/// — and the typing; the person last. After a door refused the walk's look,
/// the pane's own door is the one road left (`all` false).
fn pane_roads(pane: &str, all: bool) -> Value {
    let door = |words: String| json!(format!("{BROWSER_CLI} {words}"));
    let revision = format!("{WORKSPACE_REVISION_FLAG} <revision>");
    let picture = json!({ "road": "picture", "calls": [
        door(format!("marks {pane} --json")),
        door(format!("screenshot {pane} --marks")),
        door(format!("click {pane} --mark <n> {revision}")),
    ], "fill": format!(
        "<n>: an item's mark in the marks answer's {ITEMS_KEY}; <revision>: that answer's {WORKSPACE_REVISION_KEY}"
    ) });
    if !all {
        return json!([picture]);
    }
    json!([
        picture,
        { "road": "address_field", "calls": [
            door(format!("fields {pane} --json")),
            door(format!("type {pane} <css> <text> {revision}")),
        ], "fill": format!(
            "<css>: the field's handle in the fields answer — a spreadsheet's name box, a search field; \
             <text>: the cell or the words to go to; <revision>: that answer's {WORKSPACE_REVISION_KEY}"
        ) },
        person_road(ASK_THE_DESKTOP_APP),
    ])
}

/// `list_windows {app}`: keep the windows `app` names — by its name in any
/// case, or `pid:<n>` — and say so in the receipt. An app with no window on the
/// desktop is not a refusal: the whole list stays and the receipt says the
/// filter stood aside, so the model sees what is there to choose from.
fn filter_windows_by_app(answer: &mut Value, app: &str) {
    let Some(result) = answer.get_mut("result").and_then(Value::as_object_mut) else {
        return;
    };
    let Some(Value::Array(windows)) = result.get_mut("windows") else {
        return;
    };
    let wanted = app.trim();
    let of = windows.len();
    let kept: Vec<Value> = windows.iter().filter(|window| window_of_app(window, wanted)).cloned().collect();
    let filter = if kept.is_empty() {
        json!({ "app": wanted, "state": "fell_back", "reason": "no window of that app is on the desktop; the whole list is shown", "of": of })
    } else {
        json!({ "app": wanted, "state": "applied", "kept": kept.len(), "of": of })
    };
    if !kept.is_empty() {
        *windows = kept;
    }
    result.insert("app_filter".into(), filter);
}

/// Whether a listed window is `app`'s: by the app's name in any case, or by
/// `pid:<n>`.
fn window_of_app(window: &Value, app: &str) -> bool {
    let wanted = app.trim();
    let pid = wanted.strip_prefix("pid:").and_then(|pid| pid.trim().parse::<u64>().ok());
    let owner = window.get("app");
    let named = owner.and_then(|owner| owner.get("name")).and_then(Value::as_str);
    named.is_some_and(|name| name.trim().eq_ignore_ascii_case(wanted))
        || pid.is_some_and(|pid| owner.and_then(|owner| owner.get("pid")).and_then(Value::as_u64) == Some(pid))
}

#[cfg(test)]
pub(crate) fn run_computer(input: &Value, ctx: &ToolContext, road: &ComputerRoad) -> Result<String, ToolError> {
    run_computer_execution(input, ctx, road).map(|result| result.text)
}

pub(crate) fn run_computer_execution(input: &Value, ctx: &ToolContext, road: &ComputerRoad) -> Result<ToolExecutionResult, ToolError> {
    super::computer_tally::run(input, ctx, || run_computer_inner(input, ctx, road))
}

fn run_computer_inner(input: &Value, ctx: &ToolContext, road: &ComputerRoad) -> Result<ToolExecutionResult, ToolError> {
    if input.get("action").and_then(Value::as_str).map(str::trim) == Some(BATCH_ACTION) {
        if let Some(steps) = input.get("steps").and_then(Value::as_array) {
            check_batch_len(steps.len())?;
        }
    }
    let input: ComputerInput = from_value(input)?;
    if input.background == Some(true) && input.action.trim() != BATCH_ACTION {
        argv_for(&input)?;
    }
    let settle = input.settle.unwrap_or(SETTLE_AFTER_ACT);
    let action = input.action.trim().to_string();
    if action == BATCH_ACTION {
        return run_computer_batch(input, ctx, road).map(ToolExecutionResult::from);
    }
    if input.steps.is_some() {
        return Err(ToolError::InvalidInput(format!("`steps` belongs to `batch`, not `{action}`")));
    }
    if action == WALK_ACTION {
        return run_computer_walk(&input, ctx, road, settle).map(ToolExecutionResult::from);
    }
    // A command the tool cannot spell costs no look.
    argv_for(&input)?;
    let first_look = first_look(&[&input], ctx, road)?;
    let frame = if let Some(target) = target_of(&input).filter(|_| input.coordinate.is_some() || input.start_coordinate.is_some()) {
        Some(ctx.computer_target_frame(&target).ok_or_else(|| ToolError::InvalidInput("the selected window has no matching picture frame".into()))?)
    } else {
        ctx.computer_frame()
    };
    let input = in_points(input, frame.unwrap_or(ShotFrame::UNIT))?;
    let mut argv = with_marked_look(argv_for(&input)?, ctx)?;
    // A process whose looks carry marks numbers its own looks too.
    if action == "observe" && looks_carry_marks() {
        argv.push(MARKS_FLAG.into());
    }
    if LOOKS.contains(&action.as_str()) || argv.iter().any(|word| word == "--mark") {
        argv = as_viewer(argv, ctx);
    }
    let mut answer = call_shim(road, &argv, ctx.cwd.as_deref())?;
    let mut repicked = None;
    if let Some(refused) = refusal(&answer) {
        let (pressed, picked) = repick(&input, refused, ctx, road)?;
        answer = pressed;
        repicked = Some(picked);
    }
    if action == LIST_WINDOWS_ACTION {
        if let Some(app) = input.app.as_deref() {
            filter_windows_by_app(&mut answer, app);
        }
    }
    let returned_window = match checked_window(&answer, input.window_id.filter(|_| action != "window" && !timed_wait(&action))) {
        Ok(window_id) => window_id,
        Err(error) if acts(&action) => return unverified_action_receipt(&action, input.window_id, &error, 1).map(ToolExecutionResult::from),
        Err(error) => return Err(error),
    };
    let surface = host_surface_execution(&answer);
    let mut after_target = if action == "window" { None } else { acted_in(&[&input], ctx) };
    if let Some(target) = &mut after_target {
        target.window_id = returned_window.or(target.window_id);
    }
    let staged = if LOOKS.contains(&action.as_str()) {
        let (app, window_id) = (input.app.as_deref(), input.window_id);
        present_observation(&mut answer, ctx, app, window_id, action == "observe")
    } else {
        let staged = stage_screenshot(&mut answer, ctx);
        if staged.is_some() {
            ctx.set_computer_observation(None); // A zoom or action image changed what the model last saw.
        }
        staged
    };
    if action == "screenshot" && input.app.is_some() {
        if let Some(result) = answer.get_mut("result").and_then(Value::as_object_mut) {
            result.remove("tree");
        }
    }
    let mut result = shown(answer.get("result").cloned().unwrap_or(Value::Null), ctx.computer_frame());
    // A picture of its own (a screenshot, a zoom) carries no numbers: after it
    // there is no marked look to click by until one is shown again.
    if staged.is_some() {
        present_marks(&mut result, ctx);
    }
    // One look after the act, so the model sees what it did.
    let looked_after =
        (LOOK_AFTER_ACT && acts(&action) && (input.window_id.is_none() || after_target.is_some()))
            .then(|| look_after_execution(road, ctx, settle, after_target.as_ref()));
    let surface = match looked_after.as_ref().and_then(|look| look.surface.as_ref()) {
        Some(after) if surface.as_ref().is_some_and(|before| before.surface_id != after.surface_id) => None,
        Some(after) => Some(after.clone()),
        None => surface,
    };
    let mut said = json!({
        "ok": true,
        "action": action,
        "result": result,
        "screenshot": staged,
        "first_look": first_look,
        "looked_after": looked_after.map(|look| look.value),
    });
    if let Some(picked) = repicked {
        said["repicked"] = picked;
    }
    let text = to_pretty_json(said)?;
    Ok(ToolExecutionResult { text, surface })
}

/// A lone click by reading that the window refused `element_not_found` —
/// no control read its words, refused before any input, since the helper
/// chooses the control before it presses — on a page that moved on (t-66760
/// b1): the same window is read again for the same words and, while it shows
/// the document the model's last look at it showed — the same title, and the
/// same identity when the window gives one (t-68358) — and one control reads
/// them, the click is pressed once more by those words: the helper chooses
/// the control again on the press's own tree and refuses none or several, so
/// a tree that moved after the read is never pressed by the read's index. No
/// look at the window, another document, no control or several: the refusal
/// stands, with the candidates named when there are several. Another
/// document, or one whose identity only one side gave, is named by its word
/// with the look to take next (t-70459). One read, one press — never a loop.
fn repick(input: &ComputerInput, refused: Refusal, ctx: &ToolContext, road: &ComputerRoad) -> Result<(Value, Value), ToolError> {
    let reading = (input.label.is_some() || input.role.is_some()) && input.ocr != Some(true);
    let (Some(app), Some(window_id)) = (input.app.as_deref(), input.window_id) else {
        return Err(refused.into());
    };
    if !reading || input.action.trim() != ELEMENT_CLICK || refused.code != error_code::ELEMENT_NOT_FOUND {
        return Err(refused.into());
    }
    let Some(looked) = ctx.computer_window_look(window_id).filter(|looked| looked.title.is_some()) else {
        return Err(refused.into());
    };
    let lookup = ComputerInput {
        action: "find".into(),
        app: Some(app.to_string()),
        window_id: Some(window_id),
        label: input.label.clone(),
        role: input.role.clone(),
        background: input.background,
        ..ComputerInput::default()
    };
    let Some(found) = argv_for(&lookup)
        .ok()
        .and_then(|argv| call_shim(road, &argv, ctx.cwd.as_deref()).ok())
        .filter(|found| refusal(found).is_none() && checked_window(found, Some(window_id)).is_ok())
    else {
        return Err(refused.into());
    };
    let document = window_document(&found);
    let kept = if window_title(&found) != looked.title || (document.is_some() && looked.document.is_some() && document != looked.document) {
        Some((REPICK_DOCUMENT_CHANGED, "the window's document changed since your last look at it"))
    } else if document.is_some() != looked.document.is_some() {
        Some((REPICK_DOCUMENT_UNCONFIRMED, "only one of your last look at the window and this read of it gave the document's identity, so it is not confirmed"))
    } else {
        None
    };
    if let Some((word, why)) = kept {
        let mut look = json!({ "action": "observe", "app": app, "window_id": window_id });
        if input.background == Some(true) {
            look["background"] = json!(true);
        }
        let message = format!("{}; {word}: {why} — no other control was chosen; observe it again with {look}, then send the click again", refused.message);
        return Err(Refusal { message, ..refused }.into());
    }
    let matches = found.pointer("/result/matches").and_then(Value::as_array).map_or(&[][..], Vec::as_slice);
    let more = found.pointer("/result/truncated") == Some(&Value::Bool(true));
    let only = match matches {
        [only] if !more => only,
        [] => return Err(refused.into()),
        several => {
            let named = several.iter().map(candidate).collect::<Vec<_>>().join("; ");
            let message = format!(
                "{}; {}{} controls read these words in the window now: {named} — add role or label, or press one by element_index",
                refused.message,
                several.len(),
                if more { " or more" } else { "" }
            );
            return Err(Refusal { message, ..refused }.into());
        }
    };
    let pressed = call_shim(road, &argv_for(input)?, ctx.cwd.as_deref())?;
    if let Some(refused) = refusal(&pressed) {
        return Err(refused.into());
    }
    super::computer_tally::note(super::computer_tally::REPICKED);
    let compared = if looked.document.is_some() { REPICK_COMPARED_DOCUMENT } else { REPICK_COMPARED_TITLE };
    Ok((pressed, json!({ "label": only.get("label"), "role": only.get("role"), "compared": compared })))
}

/// A control a read found, as a refusal names it: `12 AXButton «Save»`.
fn candidate(found: &Value) -> String {
    let index = found.get("index").map_or_else(|| "?".to_string(), Value::to_string);
    let role = found.get("role").and_then(Value::as_str).unwrap_or("?");
    let reads = ["label", "value", "description"]
        .into_iter()
        .find_map(|key| found.get(key).and_then(Value::as_str).filter(|text| !text.trim().is_empty()));
    reads.map_or_else(|| format!("{index} {role}"), |reads| format!("{index} {role} «{reads}»"))
}

#[cfg(test)]
pub(crate) mod tests {
    #[test]
    fn browser_chrome_is_an_explicit_tool_claim_and_cli_flag() {
        let parsed = serde_json::from_value::<ComputerInput>(json!({
            "action": "left_click", "app": "Google Chrome", "element_index": 194,
            "browser_chrome": true
        }));
        assert!(parsed.is_ok(), "Computer must accept the explicit browser_chrome claim: {parsed:?}");
        let argv = argv_for(&parsed.unwrap()).unwrap();
        assert!(argv.iter().any(|word| word == "--browser-chrome"), "the claim must reach the window: {argv:?}");
        let plain = argv_for(&input(json!({"action": "left_click", "app": "Google Chrome", "element_index": 194}))).unwrap();
        assert!(!plain.iter().any(|word| word == "--browser-chrome"), "ordinary page input never declares browser chrome");
        assert_eq!(tool_specs()[0].input_schema["properties"]["browser_chrome"]["type"], "boolean");
    }

    #[test]
    fn the_computer_description_routes_web_pages_and_names_the_browser_chrome_claim() {
        let description = tool_specs()[0].description;
        assert!(description.contains("For web pages use zerocode-browser open <url>"), "the Computer description must name the work browser");
        assert!(description.contains("browser_chrome") && description.contains("menus, tabs or extension UI"), "the exception must claim browser chrome");
    }

    #[test]
    fn review_windows_computer_resolution_selects_the_installed_cmd_launcher() {
        let dir = tempfile::tempdir().unwrap();
        let extensionless = dir.path().join("zerocode-computer");
        let launcher = dir.path().join("zerocode-computer.cmd");
        std::fs::write(&extensionless, "POSIX fixture").unwrap();
        std::fs::write(&launcher, "Windows fixture").unwrap();
        let road = ComputerRoad::from_search_path(dir.path().as_os_str(), true).unwrap();
        assert_eq!(road.program, launcher, "Windows Computer resolution must select the installed .cmd launcher, not the POSIX file");
    }

    #[cfg(unix)]
    mod surface_host {
        include!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../../crates/zerocode-core/tests/support/surface_host.rs"));
    }

    use super::*;
    #[cfg(unix)]
    use runtime::{CompactionSummarizer, ContentBlock, ConversationMessage, MessageRole, Session};

    fn input(fields: Value) -> ComputerInput {
        serde_json::from_value(fields).expect("input")
    }

    #[cfg(unix)]
    fn native_wait_fixtures() -> Vec<Value> {
        serde_json::from_str(include_str!("../../../../crates/zerocode-shell/tests/fixtures/scoped-wait-for.json")).unwrap()
    }

    #[cfg(unix)]
    #[test]
    fn the_shared_batch_limit_precedes_deserialization_and_scope_default_expansion() {
        let dir = tempfile::tempdir().unwrap();
        let (road, _) = fake_shim(dir.path());
        let ctx = crate::tests::tool_context_outside_cwd_windows();
        let fields = json!({"action":"batch", "app":"Fixture", "window_id":41,
            "steps": vec![json!({"action":"wait", "unknown":"invalid"}); COMPUTER_BATCH_MAX_STEPS + 1]});
        let error = run_computer(&fields, &ctx, &road).unwrap_err();
        assert!(error.to_string().contains(&format!("1 to {COMPUTER_BATCH_MAX_STEPS} steps")), "{error}");
        let direct = ComputerInput {
            action: BATCH_ACTION.into(), app: Some("Fixture".into()), window_id: Some(41),
            steps: Some(vec![input(json!({"action":"wait", "app":"Other", "window_id":52})); COMPUTER_BATCH_MAX_STEPS + 1]),
            ..ComputerInput::default()
        };
        let error = run_computer_batch(direct, &ctx, &road).unwrap_err();
        assert!(error.to_string().contains(&format!("1 to {COMPUTER_BATCH_MAX_STEPS} steps")), "{error}");
        assert!(calls(dir.path()).is_empty());
    }

    #[test]
    fn batch_serialization_rejects_the_shared_limit_before_per_step_validation() {
        let invalid = input(json!({"action":"batch", "steps":[]}));
        let error = batch_argv(&vec![invalid; COMPUTER_BATCH_MAX_STEPS + 1], ShotFrame::UNIT).unwrap_err();
        assert!(error.to_string().contains(&format!("1 to {COMPUTER_BATCH_MAX_STEPS} steps")), "{error}");
        assert!(batch_argv(&[], ShotFrame::UNIT).is_err());
    }

    #[test]
    fn batch_serialization_accepts_exactly_the_existing_shared_step_limit() {
        let steps = vec![input(json!({"action":"wait", "duration":0.001})); COMPUTER_BATCH_MAX_STEPS];
        let argv = batch_argv(&steps, ShotFrame::UNIT).unwrap();
        let parsed = zerocode_core::computer_use::parse_command(&argv).unwrap();
        assert_eq!(zerocode_core::computer_use::batch_steps(&parsed).len(), COMPUTER_BATCH_MAX_STEPS);
    }

    #[cfg(unix)]
    #[test]
    fn native_producer_fixture_receipts_reach_the_scoped_wait_adapter() {
        for case in native_wait_fixtures() {
            let dir = tempfile::tempdir().unwrap();
            let (road, _) = fake_shim(dir.path());
            let ctx = crate::tests::tool_context_outside_cwd_windows();
            ctx.set_computer_frame(ShotFrame::UNIT);
            ctx.set_computer_observation(Some(ComputerObservation {
                app: case["input"]["app"].as_str().map(str::to_string),
                window_id: case["input"]["window_id"].as_u64(),
                frame: ShotFrame::UNIT,
                marked_look: None,
                title: None,
                document: None,
            }));
            std::fs::write(dir.path().join("wait-for"), json!({"ok":true,"result":case["result"]}).to_string()).unwrap();
            let response = run_computer(&case["input"], &ctx, &road).unwrap();
            let answer: Value = serde_json::from_str(&response).unwrap();
            assert_eq!(answer["result"], case["result"]);
            assert_eq!(answer["result"]["app"], case["helper"]["app"]);
            assert_eq!(answer["result"]["window"], case["helper"]["window"]);
            let argv: Vec<String> = std::fs::read_to_string(dir.path().join("wait-for-argv")).unwrap().lines().map(str::to_string).collect();
            assert_eq!(zerocode_core::computer_use::parse_command(&argv).unwrap().params, case["params"]);
            assert_eq!(calls(dir.path()), ["wait-for"]);
            assert!(answer["first_look"].is_null());
            assert!(answer["looked_after"].is_null());
        }
    }

    #[cfg(unix)]
    #[test]
    fn native_producer_receipts_survive_batch_defaults_and_keep_the_final_input_observation() {
        for case in native_wait_fixtures() {
            let dir = tempfile::tempdir().unwrap();
            let (road, _) = fake_shim(dir.path());
            let ctx = crate::tests::tool_context_outside_cwd_windows();
            ctx.set_computer_frame(ShotFrame::UNIT);
            ctx.set_computer_observation(Some(ComputerObservation {
                app: case["input"]["app"].as_str().map(str::to_string),
                window_id: case["input"]["window_id"].as_u64(),
                frame: ShotFrame::UNIT,
                marked_look: None,
                title: None,
                document: None,
            }));
            let mut wait = case["input"].clone();
            for key in ["app", "window_id", "background"] { wait.as_object_mut().unwrap().remove(key); }
            let steps = json!([wait, {"action":"left_click", "label":"Save", "role":"AXButton"}]);
            let reports = json!([
                {"n":1,"ok":true,"result":case["result"]},
                {"n":2,"ok":true,"result":{"action":{"targetWindowId":41}}},
            ]);
            std::fs::write(dir.path().join("batch"), json!({"ok":true,"result":{"ran":2,"steps":reports}}).to_string()).unwrap();
            let response = run_computer(&json!({"action":"batch","app":case["input"]["app"],"window_id":41,"background":true,"steps":steps}), &ctx, &road).unwrap();
            let answer: Value = serde_json::from_str(&response).unwrap();
            assert_eq!(answer["ran"], 2);
            assert_eq!(answer["steps"][0]["result"], case["result"]);
            let argv: Vec<Vec<String>> = serde_json::from_str(&std::fs::read_to_string(dir.path().join("batch-commands")).unwrap()).unwrap();
            assert_eq!(zerocode_core::computer_use::parse_command(&argv[0]).unwrap().params, case["params"]);
            let observed = answer["looked_after"]["screen"]["said"].as_str().unwrap();
            assert!(observed.contains("--app org.example.fixture") && observed.contains("--window-id 41"));
            assert_eq!(calls(dir.path()), ["batch", "observe"]);
            assert!(answer["first_look"].is_null());
        }
    }

    fn screen_with_look(look: &str) -> Value {
        json!({ "marks": { "lookId": look, "items": [] } })
    }

    #[test]
    fn an_app_click_keeps_the_selected_window_for_each_target_kind() {
        for target in [json!({"element_index": 32}), json!({"label": "Save"}), json!({"label": "Save", "ocr": true})] {
            let mut fields = json!({"action": "left_click", "app": "Fixture", "window_id": 41});
            fields.as_object_mut().unwrap().extend(target.as_object().unwrap().clone());
            let command = zerocode_core::computer_use::parse_command(&argv_for(&input(fields)).unwrap()).unwrap();
            assert_eq!(command.params.get("windowId"), Some(&json!(41)), "a click must not choose the app's other window");
        }
    }

    #[test]
    fn background_app_input_never_becomes_global_desktop_input() {
        for (action, extra, method) in [
            ("left_click", json!({"label": "Save"}), ComputerMethod::Click),
            ("key", json!({"text": "Tab"}), ComputerMethod::PressKey),
            ("type", json!({"text": "fixture text"}), ComputerMethod::TypeText),
            ("scroll", json!({"coordinate": [20, 40], "scroll_direction": "down"}), ComputerMethod::Scroll),
        ] {
            let mut fields = json!({"action": action, "app": "Fixture", "window_id": 41, "background": true});
            fields.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
            let command = zerocode_core::computer_use::parse_command(&argv_for(&input(fields)).expect("scoped background input")).unwrap();
            assert_eq!(command.method, method);
            assert_eq!(command.params.get("app"), Some(&json!("Fixture")));
            assert_eq!(command.params.get("windowId"), Some(&json!(41)));
            assert_eq!(command.params.get("background"), Some(&json!(true)));
            assert_eq!(command.params.get("noScreenshot"), Some(&json!(true)));
        }
    }

    #[test]
    fn a_point_written_as_json_text_is_read_like_the_array() {
        let fields = input(json!({"action": "left_click", "coordinate": "[790, 450]", "start_coordinate": " [1, 2.5] "}));
        assert_eq!(fields.coordinate, Some([790.0, 450.0]));
        assert_eq!(fields.start_coordinate, Some([1.0, 2.5]));
        let offset = input(json!({"action": "left_click", "app": "Fixture", "label": "Save", "offset": "[3, -4]"}));
        assert_eq!(offset.offset, Some([3.0, -4.0]));
    }

    #[test]
    fn a_point_of_another_shape_is_refused_once_with_the_accepted_shape() {
        for bad in [json!("790,450"), json!("[1, 2, 3]"), json!([1]), json!(["a", "b"]), json!(7)] {
            let message = serde_json::from_value::<ComputerInput>(json!({"action": "left_click", "coordinate": bad}))
                .unwrap_err()
                .to_string();
            assert!(message.contains("expected 2 numbers like [790, 450]"), "{bad}: {message}");
        }
    }

    #[test]
    fn a_refused_scroll_names_both_shapes_with_examples() {
        let app_wheel = input(json!({"action": "scroll", "app": "Fixture", "window_id": 41, "scroll_direction": "down", "scroll_amount": 3}));
        let desktop_pages = input(json!({"action": "scroll", "coordinate": [20, 40], "scroll_direction": "down", "scroll_pages": 1}));
        for refused in [app_wheel, desktop_pages] {
            let message = argv_for(&refused).unwrap_err().to_string();
            assert!(message.contains("scroll_pages") && message.contains("scroll_amount"), "{message}");
            assert!(message.contains("\"scroll_pages\":1") && message.contains("\"scroll_amount\":3"), "{message}");
        }
    }

    /// A refused scroll's examples are sent as they stand and accepted (t-62469 S1): a model that copies one
    /// meets no second refusal.
    #[test]
    fn a_refused_scrolls_examples_are_accepted_as_they_stand() {
        let app_wheel = input(json!({"action": "scroll", "app": "Fixture", "window_id": 41, "scroll_direction": "down", "scroll_amount": 3}));
        let desktop_pages = input(json!({"action": "scroll", "coordinate": [20, 40], "scroll_direction": "down", "scroll_pages": 1}));
        for refused in [app_wheel, desktop_pages] {
            let message = argv_for(&refused).unwrap_err().to_string();
            let examples: Vec<Value> = message
                .match_indices("e.g. ")
                .filter_map(|(at, said)| serde_json::Deserializer::from_str(&message[at + said.len()..]).into_iter::<Value>().next()?.ok())
                .collect();
            assert_eq!(examples.len(), 2, "both shapes carry an example: {message}");
            for mut example in examples {
                example["action"] = json!("scroll");
                let sent = serde_json::from_value::<ComputerInput>(example.clone()).map(|fields| argv_for(&fields));
                assert!(matches!(sent, Ok(Ok(_))), "the refusal's example, sent as it stands, is refused: {example} -> {sent:?}");
            }
        }
    }

    #[test]
    fn a_selected_app_coordinate_is_window_local_not_a_global_mouse_click() {
        let fields = input(json!({"action": "left_click", "app": "Fixture", "window_id": 41, "coordinate": [20, 40]}));
        let translated = in_points(fields, ShotFrame::new((1000.0, 500.0), 0.5).unwrap()).unwrap();
        assert_eq!(translated.coordinate, Some([40.0, 80.0]));
        let command = zerocode_core::computer_use::parse_command(&argv_for(&translated).unwrap()).unwrap();
        assert_eq!(command.method, ComputerMethod::Click);
        assert_eq!(command.params.get("windowId"), Some(&json!(41)));
    }

    #[test]
    fn unsupported_window_or_app_selectors_are_refused_instead_of_discarded() {
        for fields in [
            json!({"action": "left_click", "app": "Fixture", "window": "Target document", "label": "Save"}),
            json!({"action": "left_click", "window_id": 41, "coordinate": [20, 40]}),
            json!({"action": "left_click", "app": "Fixture", "window_id": 0, "label": "Save"}),
            json!({"action": "cursor_position", "app": "Fixture"}),
            json!({"action": "hold_key", "app": "Fixture", "window_id": 41, "text": "Tab", "duration": 0.1}),
            json!({"action": "walk", "app": "Fixture", "window_id": 41, "goal": "Read the target document"}),
        ] {
            assert!(argv_for(&input(fields.clone())).is_err(), "unsupported scope was silently ignored: {fields}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn the_first_find_picture_is_of_the_same_selected_window() {
        let dir = tempfile::tempdir().unwrap();
        let (road, _) = fake_shim(dir.path());
        let ctx = crate::tests::tool_context_outside_cwd_windows();
        let answer: Value = serde_json::from_str(&run_computer(&json!({"action":"find","app":"Fixture","window_id":41,"label":"Save"}), &ctx, &road).unwrap()).unwrap();
        let asked = answer["first_look"]["screen"]["said"].as_str().unwrap_or_default();
        assert!(asked.contains("--app Fixture") && asked.contains("--window-id 41"), "preparation observed another surface: {asked}");
    }

    #[cfg(unix)]
    #[test]
    fn the_picture_after_a_click_keeps_the_clicked_window() {
        let dir = tempfile::tempdir().unwrap();
        let (road, _) = fake_shim(dir.path());
        let ctx = crate::tests::tool_context_outside_cwd_windows();
        let answer: Value = serde_json::from_str(&run_computer(&json!({"action":"left_click","app":"Fixture","window_id":41,"label":"Save"}), &ctx, &road).unwrap()).unwrap();
        let asked = answer["looked_after"]["screen"]["said"].as_str().unwrap_or_default();
        assert!(asked.contains("--app Fixture") && asked.contains("--window-id 41"), "verification observed another window: {asked}");
    }

    /// The app, window and document of the person's photo scene (10-20,
    /// t-66760 b1), in fixture names.
    #[cfg(unix)]
    pub(crate) const PHOTO_APP: &str = "org.example.fixture";
    #[cfg(unix)]
    pub(crate) const PHOTO_WINDOW: u64 = 41;
    #[cfg(unix)]
    pub(crate) const PHOTO_TITLE: &str = "Fixture document";
    /// The identity the photo scene's window gives its document when a test
    /// has it give one — an `AXDocument`, a page's URL (t-68358).
    #[cfg(unix)]
    const PHOTO_DOCUMENT: &str = "fixture-document-a";

    /// The photo scene's click: by label and role, in the background, in the
    /// window the model looked at.
    #[cfg(unix)]
    fn photo_click() -> Value {
        json!({"action":"left_click","app":PHOTO_APP,"window_id":PHOTO_WINDOW,"background":true,"label":"Continue","role":"AXButton"})
    }

    /// A control that reads the photo scene's words, at `index` of a read.
    #[cfg(unix)]
    pub(crate) fn continue_button(index: u64) -> Value {
        json!({"index":index,"role":"AXButton","label":"Continue"})
    }

    /// What a read of the photo scene's window answers: its document's
    /// `title` and the controls that read the words.
    #[cfg(unix)]
    pub(crate) fn photo_found(title: &str, matches: &Value) -> Value {
        json!({"source":"accessibility","snapshotId":"fixture-snapshot-2","app":"Fixture",
            "window":{"id":PHOTO_WINDOW,"title":title},"matches":matches,"truncated":false})
    }

    /// A read of the photo scene's window whose document gives `identity`.
    #[cfg(unix)]
    fn of_document(mut found: Value, identity: &str) -> Value {
        found["window"]["document"] = json!(identity);
        found
    }

    /// The photo scene: the model looks at the window (its document is
    /// `PHOTO_TITLE`), then clicks by reading; the window refuses that click
    /// `element_not_found` before any input — the page moved on — and a read
    /// of the same window answers `found`.
    #[cfg(unix)]
    pub(crate) fn photo_scene(dir: &Path, ctx: &ToolContext, road: &ComputerRoad, found: &Value) -> Result<String, ToolError> {
        std::fs::write(dir.join("window-title"), PHOTO_TITLE).unwrap();
        run_computer(&json!({"action":"observe","app":PHOTO_APP,"window_id":PHOTO_WINDOW}), ctx, road).expect("the model's look at the window");
        refuse_the_first_click(dir, "element_not_found", "no control in Fixture's window reads --role AXButton --label «Continue»; look with --marks, or find");
        std::fs::write(dir.join("reply-find"), json!({"ok":true,"result":found}).to_string()).unwrap();
        run_computer(&photo_click(), ctx, road)
    }

    /// The window's refusal of the first click it is asked for.
    #[cfg(unix)]
    fn refuse_the_first_click(dir: &Path, code: &str, message: &str) {
        std::fs::write(dir.join("reply-click.1"), json!({"ok":false,"error":{"code":code,"message":message}}).to_string()).unwrap();
    }

    /// A window's refusal as the tool hands it on: its code, then its words.
    #[cfg(unix)]
    fn refused_with(result: Result<String, ToolError>) -> String {
        match result {
            Err(ToolError::Execution(message)) => message,
            other => panic!("a window's refusal, not {other:?}"),
        }
    }

    /// The words of the `n`th call of `verb` the fake shim was asked.
    #[cfg(unix)]
    fn shim_words(dir: &Path, verb: &str, n: usize) -> Vec<String> {
        std::fs::read_to_string(dir.join(format!("argv-{verb}.{n}"))).unwrap_or_default().lines().map(str::to_string).collect()
    }

    #[cfg(unix)]
    fn names(words: &[String], flag: &str, value: &str) -> bool {
        words.windows(2).any(|pair| pair[0] == flag && pair[1] == value)
    }

    #[cfg(unix)]
    fn clicks(dir: &Path) -> usize {
        calls(dir).iter().filter(|verb| *verb == "click").count()
    }

    /// Photo scene (a), t-66760 b1: the same document, one control that
    /// reads the words — pressed once, by those words (t-68358), in the
    /// same window and the same background; the answer says it was
    /// re-picked, which control, and that only the title was compared: the
    /// fixture's window gives no document identity.
    #[cfg(unix)]
    #[test]
    fn the_photo_scene_click_is_repicked_once_on_the_same_document() {
        let dir = tempfile::tempdir().unwrap();
        let (road, _) = fake_shim(dir.path());
        let ctx = crate::tests::tool_context_outside_cwd_windows();
        let answer = photo_scene(dir.path(), &ctx, &road, &photo_found(PHOTO_TITLE, &json!([continue_button(7)])));
        assert!(answer.is_ok(), "photo scene (a): one control reads the words on the same document, so the click is re-picked, not refused: {answer:?}");
        let answer: Value = serde_json::from_str(&answer.unwrap()).unwrap();
        assert_eq!(
            answer["repicked"],
            json!({"label":"Continue","role":"AXButton","compared":"title only"}),
            "photo scene (a): the answer says it re-picked, which control, and that only the title was compared"
        );
        assert_eq!(calls(dir.path()), ["observe", "click", "find", "click", "observe"], "photo scene (a): one read of the window, one press, one look after");
        let window = PHOTO_WINDOW.to_string();
        let reread = shim_words(dir.path(), "find", 1);
        assert!(
            names(&reread, "--label", "Continue") && names(&reread, "--role", "AXButton") && names(&reread, "--window-id", &window),
            "photo scene (a): the same window is read for the same words: {reread:?}"
        );
        let pressed = shim_words(dir.path(), "click", 2);
        assert!(
            names(&pressed, "--label", "Continue") && names(&pressed, "--window-id", &window) && pressed.iter().any(|word| word == "--background"),
            "photo scene (a): the press names the control by its words, in the same window and the background: {pressed:?}"
        );
    }

    /// Photo scene (b): the window shows another document than the model's
    /// look did — the same words may be another control; the refusal stands
    /// and says why, and nothing more is pressed.
    #[cfg(unix)]
    #[test]
    fn the_photo_scene_click_on_a_changed_document_keeps_its_refusal() {
        let dir = tempfile::tempdir().unwrap();
        let (road, _) = fake_shim(dir.path());
        let ctx = crate::tests::tool_context_outside_cwd_windows();
        let refused = refused_with(photo_scene(dir.path(), &ctx, &road, &photo_found("Fixture results", &json!([continue_button(7)]))));
        assert!(refused.starts_with("element_not_found:"), "photo scene (b): the window's refusal stands: {refused}");
        assert!(refused.contains("document changed"), "photo scene (b): the refusal says the document changed since the look: {refused}");
        assert_eq!(clicks(dir.path()), 1, "photo scene (b): nothing pressed after the document changed");
    }

    /// Photo scene (c): two controls read the words — the tool never guesses
    /// between them; the refusal names both, and nothing more is pressed.
    #[cfg(unix)]
    #[test]
    fn the_photo_scene_click_with_two_candidates_is_refused_with_them() {
        let dir = tempfile::tempdir().unwrap();
        let (road, _) = fake_shim(dir.path());
        let ctx = crate::tests::tool_context_outside_cwd_windows();
        let refused = refused_with(photo_scene(dir.path(), &ctx, &road, &photo_found(PHOTO_TITLE, &json!([continue_button(7), continue_button(9)]))));
        assert!(refused.starts_with("element_not_found:"), "photo scene (c): the window's refusal stands: {refused}");
        assert!(
            refused.contains("7 AXButton «Continue»") && refused.contains("9 AXButton «Continue»"),
            "photo scene (c): the refusal names the candidates: {refused}"
        );
        assert_eq!(clicks(dir.path()), 1, "photo scene (c): nothing pressed between two candidates");
    }

    /// Photo scene (d), t-68358: the window keeps its title through a change
    /// of document — Finder, Terminal, a web app with one title per site —
    /// and gives its document's identity. The identity holds the re-pick to
    /// the document as well as the title: another identity, or one on one
    /// side only, keeps the refusal, and nothing more is pressed. The refusal
    /// names which (t-70459): `document_changed`, or `document_unconfirmed`.
    #[cfg(unix)]
    #[test]
    fn a_repick_whose_title_stayed_but_whose_document_changed_is_refused() {
        for (looked, read) in [(Some(PHOTO_DOCUMENT), Some("fixture-document-b")), (Some(PHOTO_DOCUMENT), None), (None, Some(PHOTO_DOCUMENT))] {
            let word = if looked.is_some() && read.is_some() { "document_changed" } else { "document_unconfirmed" };
            let dir = tempfile::tempdir().unwrap();
            let (road, _) = fake_shim(dir.path());
            let ctx = crate::tests::tool_context_outside_cwd_windows();
            if let Some(looked) = looked {
                std::fs::write(dir.path().join("window-document"), looked).unwrap();
            }
            let mut found = photo_found(PHOTO_TITLE, &json!([continue_button(7)]));
            if let Some(read) = read {
                found = of_document(found, read);
            }
            let answer = photo_scene(dir.path(), &ctx, &road, &found);
            assert!(
                answer.as_ref().is_err_and(|error| error.to_string().contains(&format!("; {word}: "))),
                "photo scene (d), {looked:?} → {read:?}: the same title on another or an unconfirmed document keeps the refusal, named {word}: {answer:?}"
            );
            assert_eq!(clicks(dir.path()), 1, "photo scene (d), {looked:?} → {read:?}: nothing pressed on another document");
        }
    }

    /// Photo scene (e), t-68358: the same title and the same document
    /// identity — the click is re-picked, and the answer says both were
    /// compared.
    #[cfg(unix)]
    #[test]
    fn a_repick_on_the_same_title_and_document_says_it_compared_both() {
        let dir = tempfile::tempdir().unwrap();
        let (road, _) = fake_shim(dir.path());
        let ctx = crate::tests::tool_context_outside_cwd_windows();
        std::fs::write(dir.path().join("window-document"), PHOTO_DOCUMENT).unwrap();
        let found = of_document(photo_found(PHOTO_TITLE, &json!([continue_button(7)])), PHOTO_DOCUMENT);
        let answer = photo_scene(dir.path(), &ctx, &road, &found);
        assert!(answer.is_ok(), "photo scene (e): the same title and document, so the click is re-picked: {answer:?}");
        let answer: Value = serde_json::from_str(&answer.unwrap()).unwrap();
        assert_eq!(answer["repicked"]["compared"], "title and document", "photo scene (e): the answer says the title and the document were compared");
        assert_eq!(clicks(dir.path()), 2, "photo scene (e): one press after the refused click");
    }

    /// The document's identity as the helper gives it (t-69973, sixteen
    /// lowercase hex digits).
    const HELPER_DOCUMENT: &str = "0123456789abcdef";

    /// The helper gives a window's document identity (t-69973) at three
    /// places — find's `result.window`, get-app-state's
    /// `result.snapshot.window`, observe's `result.tree.window` — and zo
    /// reads it, and the title beside it, at each (t-70459).
    #[test]
    fn the_window_document_is_read_at_each_place_the_helper_answers_it() {
        let window = json!({"id":41,"title":"Fixture document","document":HELPER_DOCUMENT});
        for answer in [
            json!({"ok":true,"result":{"window":window,"matches":[]}}),
            json!({"ok":true,"result":{"snapshot":{"window":window}}}),
            json!({"ok":true,"result":{"tree":{"window":window,"text":"","elementCount":0}}}),
        ] {
            assert_eq!(window_document(&answer).as_deref(), Some(HELPER_DOCUMENT), "the identity at its place: {answer}");
            assert_eq!(window_title(&answer).as_deref(), Some("Fixture document"), "the title beside it: {answer}");
        }
    }

    /// The look the refusal of a re-pick names as the next step, as the
    /// model would send it.
    #[cfg(unix)]
    fn the_look_it_names(refused: &str) -> Value {
        let at = refused.find(" with {").map(|at| at + " with ".len()).expect("the refusal names a look");
        serde_json::Deserializer::from_str(&refused[at..]).into_iter::<Value>().next().expect("a call").expect("a JSON call")
    }

    /// The three cases of the document comparison (t-70459), in the helper's
    /// own answer shapes: the model's look answered as observe answers it
    /// (`tree.window`) or as get-app-state does (`snapshot.window`), the read
    /// as find does (`window`). The same document is pressed; another is
    /// refused as `document_changed`; one only one side identifies is
    /// refused as `document_unconfirmed` — and the refusal names the look to
    /// take next, at the same window, in the same background.
    #[cfg(unix)]
    #[test]
    fn a_repick_compares_the_documents_at_the_helpers_own_places() {
        let other = "fedcba9876543210";
        for place in ["tree", "snapshot"] {
            for (looked, read, word) in [
                (Some(HELPER_DOCUMENT), Some(HELPER_DOCUMENT), None),
                (Some(HELPER_DOCUMENT), Some(other), Some("document_changed")),
                (Some(HELPER_DOCUMENT), None, Some("document_unconfirmed")),
                (None, Some(HELPER_DOCUMENT), Some("document_unconfirmed")),
            ] {
                let dir = tempfile::tempdir().unwrap();
                let (road, _) = fake_shim(dir.path());
                let ctx = crate::tests::tool_context_outside_cwd_windows();
                helper_look(dir.path(), 1, place, &photo_window(looked), "AXButton «Continue»", 3);
                run_computer(&json!({"action":"observe","app":PHOTO_APP,"window_id":PHOTO_WINDOW}), &ctx, &road).expect("the model's look at the window");
                refuse_the_first_click(dir.path(), "element_not_found", "no control in Fixture's window reads --role AXButton --label «Continue»");
                let mut found = photo_found(PHOTO_TITLE, &json!([continue_button(7)]));
                if let Some(read) = read {
                    found = of_document(found, read);
                }
                std::fs::write(dir.path().join("reply-find"), json!({"ok":true,"result":found}).to_string()).unwrap();
                let answer = run_computer(&photo_click(), &ctx, &road);
                let case = format!("{place}: {looked:?} → {read:?}");
                if let Some(word) = word {
                    let refused = refused_with(answer);
                    assert!(refused.contains(&format!("; {word}: ")), "{case}: the refusal names why, {word}: {refused}");
                    assert_eq!(clicks(dir.path()), 1, "{case}: nothing pressed: {refused}");
                    assert_eq!(
                        the_look_it_names(&refused),
                        json!({"action":"observe","app":PHOTO_APP,"window_id":PHOTO_WINDOW,"background":true}),
                        "{case}: the next step is a look at the same window, in the same background: {refused}"
                    );
                } else {
                    let answer: Value = serde_json::from_str(&answer.expect("the same document is pressed")).unwrap();
                    assert_eq!(answer["repicked"]["compared"], "title and document", "{case}: both were compared");
                    assert_eq!(clicks(dir.path()), 2, "{case}: one press after the refused click");
                }
            }
        }
    }

    /// The flicker (t-70459): the model's look gave the document's identity
    /// and the read for the re-pick gives none — a web area that has no
    /// address yet — so the re-pick is refused, `document_unconfirmed`. The
    /// model sends the look that refusal names; it shows the same window and
    /// the same document, and the same click, refused again before any
    /// input, is re-picked on that document and pressed.
    #[cfg(unix)]
    #[test]
    fn a_flicker_of_the_document_is_refused_and_pressed_after_the_look_it_names() {
        let dir = tempfile::tempdir().unwrap();
        let (road, _) = fake_shim(dir.path());
        let ctx = crate::tests::tool_context_outside_cwd_windows();
        let not_found = json!({"ok":false,"error":{"code":"element_not_found","message":"no control in Fixture's window reads --role AXButton --label «Continue»"}});
        let found = photo_found(PHOTO_TITLE, &json!([continue_button(7)]));
        helper_look(dir.path(), 1, "tree", &photo_window(Some(HELPER_DOCUMENT)), "AXButton «Continue»", 3);
        run_computer(&json!({"action":"observe","app":PHOTO_APP,"window_id":PHOTO_WINDOW}), &ctx, &road).expect("the model's look at the window");
        std::fs::write(dir.path().join("reply-click.1"), not_found.to_string()).unwrap();
        std::fs::write(dir.path().join("reply-find.1"), json!({"ok":true,"result":found}).to_string()).unwrap();
        let refused = refused_with(run_computer(&photo_click(), &ctx, &road));
        assert!(refused.contains("; document_unconfirmed: "), "flicker: a read without the identity is refused as unconfirmed: {refused}");
        helper_look(dir.path(), 2, "tree", &photo_window(Some(HELPER_DOCUMENT)), "AXButton «Continue»", 3);
        run_computer(&the_look_it_names(&refused), &ctx, &road).expect("the look the refusal names");
        std::fs::write(dir.path().join("reply-click.2"), not_found.to_string()).unwrap();
        std::fs::write(dir.path().join("reply-find.2"), json!({"ok":true,"result":of_document(found, HELPER_DOCUMENT)}).to_string()).unwrap();
        let answer: Value = serde_json::from_str(&run_computer(&photo_click(), &ctx, &road).expect("flicker: pressed after the look again")).unwrap();
        assert_eq!(answer["repicked"]["compared"], "title and document", "flicker: the same window and document on both sides: {answer}");
        assert_eq!(
            calls(dir.path()),
            ["observe", "click", "find", "observe", "click", "find", "click", "observe"],
            "flicker: one refused re-pick, one look, one press: {answer}"
        );
    }

    /// Photo scene (f), t-68358: the window's tree moved between the read
    /// and the press, so the read's index 7 may name another control by
    /// then. The press is the click the model asked, once more, by its
    /// words: the helper chooses the control on the press's own tree and
    /// refuses none or several; the read's index is never sent.
    #[cfg(unix)]
    #[test]
    fn a_repick_on_a_tree_that_moved_presses_by_the_words_not_a_stale_index() {
        let dir = tempfile::tempdir().unwrap();
        let (road, _) = fake_shim(dir.path());
        let ctx = crate::tests::tool_context_outside_cwd_windows();
        let answer = photo_scene(dir.path(), &ctx, &road, &photo_found(PHOTO_TITLE, &json!([continue_button(7)])));
        assert!(answer.is_ok(), "photo scene (f): one control reads the words on the same document: {answer:?}");
        let (asked, pressed) = (shim_words(dir.path(), "click", 1), shim_words(dir.path(), "click", 2));
        assert_eq!(pressed, asked, "photo scene (f): the press is the click the model asked, by its words — not by the read's index 7");
    }

    /// Photo scene (g): the re-picked press is itself refused — by then the
    /// tree moved again, and no control or several read the words. That
    /// refusal is the answer; nothing is read or pressed a third time.
    #[cfg(unix)]
    #[test]
    fn a_repicked_press_the_window_refuses_is_the_answer_and_nothing_is_pressed_again() {
        let dir = tempfile::tempdir().unwrap();
        let (road, _) = fake_shim(dir.path());
        let ctx = crate::tests::tool_context_outside_cwd_windows();
        let several = json!({"ok":false,"error":{"code":"ambiguous_target","message":"2 controls read --role AXButton --label «Continue»"}});
        std::fs::write(dir.path().join("reply-click.2"), several.to_string()).unwrap();
        let refused = refused_with(photo_scene(dir.path(), &ctx, &road, &photo_found(PHOTO_TITLE, &json!([continue_button(7)]))));
        assert!(refused.starts_with("ambiguous_target:"), "photo scene (g): the press's own refusal is the answer: {refused}");
        assert_eq!(calls(dir.path()), ["observe", "click", "find", "click"], "photo scene (g): one read, one press — never a third");
    }

    /// No look at the window: there is no document to hold a re-pick to, so
    /// the refusal stands and the window is not read again.
    #[cfg(unix)]
    #[test]
    fn a_refused_click_by_reading_with_no_look_at_its_window_is_not_repicked() {
        let dir = tempfile::tempdir().unwrap();
        let (road, _) = fake_shim(dir.path());
        let ctx = crate::tests::tool_context_outside_cwd_windows();
        refuse_the_first_click(dir.path(), "element_not_found", "no control reads the words");
        std::fs::write(dir.path().join("reply-find"), json!({"ok":true,"result":photo_found(PHOTO_TITLE, &json!([continue_button(7)]))}).to_string()).unwrap();
        let refused = refused_with(run_computer(&photo_click(), &ctx, &road));
        assert!(refused.starts_with("element_not_found:"), "{refused}");
        assert_eq!(calls(dir.path()), ["click"], "a window never looked at is not read again");
    }

    /// Only `element_not_found` — a refusal before any input — is re-picked:
    /// the photo scene's `requires_foreground`, the helper's own
    /// `ambiguous_target`, a road that answered nothing, and the same click
    /// as a batch step each stand as they came, with one press asked.
    #[cfg(unix)]
    #[test]
    fn only_a_lone_click_refused_element_not_found_is_repicked() {
        for (code, message) in [
            ("requires_foreground", "background input pauses while the person is using the target app"),
            ("ambiguous_target", "2 controls read --label «Continue»"),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let (road, _) = fake_shim(dir.path());
            let ctx = crate::tests::tool_context_outside_cwd_windows();
            std::fs::write(dir.path().join("window-title"), PHOTO_TITLE).unwrap();
            run_computer(&json!({"action":"observe","app":PHOTO_APP,"window_id":PHOTO_WINDOW}), &ctx, &road).unwrap();
            refuse_the_first_click(dir.path(), code, message);
            let refused = refused_with(run_computer(&photo_click(), &ctx, &road));
            assert!(refused.starts_with(&format!("{code}:")), "{refused}");
            assert_eq!(calls(dir.path()), ["observe", "click"], "{code} is not re-picked");
        }
        let dir = tempfile::tempdir().unwrap();
        let (road, _) = fake_shim(dir.path());
        let ctx = crate::tests::tool_context_outside_cwd_windows();
        std::fs::write(dir.path().join("window-title"), PHOTO_TITLE).unwrap();
        run_computer(&json!({"action":"observe","app":PHOTO_APP,"window_id":PHOTO_WINDOW}), &ctx, &road).unwrap();
        std::fs::write(dir.path().join("reply-click.1"), "not an envelope").unwrap();
        assert!(run_computer(&photo_click(), &ctx, &road).is_err());
        assert_eq!(calls(dir.path()), ["observe", "click"], "a road that answered nothing is not re-picked: the press may have gone");
        let dir = tempfile::tempdir().unwrap();
        let (road, _) = fake_shim(dir.path());
        let ctx = crate::tests::tool_context_outside_cwd_windows();
        std::fs::write(dir.path().join("window-title"), PHOTO_TITLE).unwrap();
        run_computer(&json!({"action":"observe","app":PHOTO_APP,"window_id":PHOTO_WINDOW}), &ctx, &road).unwrap();
        std::fs::write(dir.path().join("batch"), json!({"ok":false,"error":{"code":"element_not_found","message":"step 1 refused"},
            "result":{"ran":0,"of":1,"refusedAt":1,"steps":[{"n":1,"verb":"click","ok":false,"error":{"code":"element_not_found","message":"no control reads the words"}}]}}).to_string()).unwrap();
        let mut step = photo_click();
        for key in ["app", "window_id", "background"] {
            step.as_object_mut().unwrap().remove(key);
        }
        let batch = json!({"action":"batch","app":PHOTO_APP,"window_id":PHOTO_WINDOW,"background":true,"steps":[step]});
        assert!(run_computer(&batch, &ctx, &road).is_err());
        assert_eq!(calls(dir.path()), ["observe", "batch"], "a batch step is not re-picked: the batch stops at its first refusal");
    }

    #[cfg(unix)]
    #[test]
    fn another_windows_answer_is_never_staged_or_returned() {
        let dir = tempfile::tempdir().unwrap();
        let (road, _) = fake_shim(dir.path());
        std::fs::write(dir.path().join("forced-window"), "52").unwrap();
        let ctx = crate::tests::tool_context_outside_cwd_windows();
        let error = run_computer(&json!({"action":"observe","app":"Fixture","window_id":41}), &ctx, &road)
            .expect_err("a different window must not reach the model");
        assert!(!error.to_string().contains("unrelated-window-fixture"));
        assert_eq!(staged(&ctx), 0);
    }

    #[cfg(unix)]
    #[test]
    fn uncertain_background_action_recovery_never_changes_the_selected_surface() {
        let dir = tempfile::tempdir().unwrap();
        let (road, _) = fake_shim(dir.path());
        let ctx = crate::tests::tool_context_outside_cwd_windows();
        run_computer(&json!({"action":"observe","app":"Fixture","window_id":41}), &ctx, &road).unwrap();
        std::fs::write(dir.path().join("forced-window"), "52").unwrap();
        let response = run_computer(&json!({"action":"left_click","app":"Fixture","window_id":41,"background":true,"label":"Next"}), &ctx, &road).unwrap();
        let answer: Value = serde_json::from_str(&response).unwrap();
        assert_eq!(answer["completion_verified"], false);
        let recovery = run_computer(&json!({"action":"observe","app":"Fixture","window_id":41}), &ctx, &road);
        assert!(recovery.is_err(), "recovery must refuse the different window");
        assert_eq!(calls(dir.path()), ["observe", "click", "observe"], "recovery must never call screenshot, open or activate");
    }

    #[cfg(unix)]
    #[test]
    fn production_registry_and_capability_preserve_only_outer_host_surface_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let host = surface_host::SurfaceHost::start(dir.path(), None, zerocode_core::agent_shims::SurfaceShim::Computer);
        let host_environment = crate::tests::EnvGuard::set_many(host.environment());
        let inherited_path = std::env::var_os("PATH").unwrap_or_default();
        let path = std::env::join_paths(std::iter::once(dir.path().to_path_buf())
            .chain(std::env::split_paths(&inherited_path))).unwrap();
        let _path = host_environment.set_also("PATH", &path);
        let expected = SurfaceExecution { surface_id: "fixture-host/computer/41/lease-a".into(), state_revision: 7,
            effect: zerocode_core::surface_execution::SurfaceEffect::Observation };
        let host = json!({"ok":true,"surfaceExecution":expected,"result":{"window":{"id":41}}});
        std::fs::write(dir.path().join("reply-observe"), host.to_string()).unwrap();
        let registry = crate::GlobalToolRegistry::builtin();
        let input = json!({"action":"observe","app":"Fixture","window_id":41});
        let direct = registry.execute_result("Computer", &input).unwrap();
        assert_eq!(direct.surface.as_ref(), Some(&expected));
        let wrapped = registry.execute_result("CapabilityInvoke", &json!({"name":"Computer","input":input,"nonce":"ignored-wrapper-field"})).unwrap();
        assert_eq!(wrapped.surface.as_ref(), Some(&expected));
        let visible: Value = serde_json::from_str(&wrapped.text).unwrap();
        assert!(visible.get("surfaceExecution").is_none(), "host metadata must remain outside the tool body");

        std::fs::write(dir.path().join("reply-observe"), json!({"ok":true,"result":{
            "window":{"id":41}, "surfaceExecution":expected,
        }}).to_string()).unwrap();
        let impostor = registry.execute_result("Computer", &input).unwrap();
        assert!(impostor.surface.is_none(), "a nested body field is not host metadata");
    }

    #[cfg(unix)]
    #[test]
    fn a_replaced_computer_shim_cannot_attest_outer_host_surface_metadata() {
        let dir = tempfile::tempdir().unwrap();
        fake_shim(dir.path());
        let expected = SurfaceExecution { surface_id:"fixture-host/computer/41/lease-a".into(), state_revision:7,
            effect:zerocode_core::surface_execution::SurfaceEffect::Observation };
        std::fs::write(dir.path().join("reply-observe"), json!({"ok":true,"surfaceExecution":expected,
            "result":{"window":{"id":41}}}).to_string()).unwrap();
        let inherited_path = std::env::var_os("PATH").unwrap_or_default();
        let path = std::env::join_paths(std::iter::once(dir.path().to_path_buf()).chain(std::env::split_paths(&inherited_path))).unwrap();
        let _path = crate::tests::EnvGuard::set("PATH", path.to_str().unwrap());
        let result = crate::GlobalToolRegistry::builtin().execute_result("Computer",
            &json!({"action":"observe","app":"Fixture","window_id":41})).unwrap();
        assert!(result.surface.is_none(), "a replaced shim must not attest host metadata even at the outer envelope");
        assert_eq!(calls(dir.path()), ["observe"], "legacy output still executes once");
    }

    #[cfg(unix)]
    #[test]
    fn production_registry_surface_results_preserve_permission_toggle_and_nested_denials() {
        let dir = tempfile::tempdir().unwrap();
        fake_shim(dir.path());
        let inherited_path = std::env::var_os("PATH").unwrap_or_default();
        let path = std::env::join_paths(std::iter::once(dir.path().to_path_buf())
            .chain(std::env::split_paths(&inherited_path))).unwrap();
        let _path = crate::tests::EnvGuard::set("PATH", path.to_str().unwrap());
        let input = json!({"name":"Computer","input":{"action":"observe","app":"Fixture","window_id":41}});
        let policy = runtime::PermissionPolicy::new(PermissionMode::ReadOnly)
            .with_tool_requirement("Computer", PermissionMode::DangerFullAccess);
        let denied = crate::GlobalToolRegistry::builtin().with_enforcer(PermissionEnforcer::new(policy))
            .execute_result("CapabilityInvoke", &input).unwrap_err();
        assert!(matches!(denied, ToolError::PermissionDenied { .. }));
        let disabled = crate::GlobalToolRegistry::builtin().with_disabled_tools(["Computer".to_string()].into_iter().collect())
            .execute_result("CapabilityInvoke", &input).unwrap_err();
        assert!(matches!(disabled, ToolError::PermissionDenied { .. }));
        let nested = crate::GlobalToolRegistry::builtin().execute_result("CapabilityInvoke", &json!({
            "name":"CapabilityInvoke", "input":input,
        })).unwrap_err();
        assert!(nested.to_string().contains("cannot invoke itself"));
        assert!(calls(dir.path()).is_empty(), "all denials must happen before the Computer road executes");
    }

    #[cfg(unix)]
    #[test]
    fn a_post_action_observation_keeps_its_host_effect_outside_the_visible_body() {
        let dir = tempfile::tempdir().unwrap();
        let host = surface_host::SurfaceHost::start(dir.path(), None, zerocode_core::agent_shims::SurfaceShim::Computer);
        let _host_environment = crate::tests::EnvGuard::set_many(host.environment());
        let road = ComputerRoad { program:dir.path().join(COMPUTER_SHIM) };
        let ctx = ToolContext::new();
        let acknowledged = SurfaceExecution { surface_id:"fixture-host/computer/41/lease-a".into(), state_revision:7,
            effect:zerocode_core::surface_execution::SurfaceEffect::Acknowledged };
        let confirmed = SurfaceExecution { surface_id:acknowledged.surface_id.clone(), state_revision:8,
            effect:zerocode_core::surface_execution::SurfaceEffect::ConfirmedChange { previous_revision:7 } };
        std::fs::write(dir.path().join("reply-click"), json!({"ok":true,"surfaceExecution":acknowledged,
            "result":{"action":{"targetWindowId":41}}}).to_string()).unwrap();
        std::fs::write(dir.path().join("reply-observe"), json!({"ok":true,"surfaceExecution":confirmed,
            "result":{"window":{"id":41}}}).to_string()).unwrap();
        let executed = run_computer_execution(&json!({"action":"left_click","app":"Fixture","window_id":41,"label":"Next"}), &ctx, &road).unwrap();
        assert_eq!(executed.surface.as_ref(), Some(&confirmed));
        assert!(!executed.text.contains("surfaceExecution"));
        assert_eq!(calls(dir.path()), ["click", "observe"]);
    }

    #[cfg(unix)]
    #[test]
    fn a_changed_control_identity_during_post_action_observation_cannot_claim_progress() {
        let dir = tempfile::tempdir().unwrap();
        let host = surface_host::SurfaceHost::start(dir.path(), None, zerocode_core::agent_shims::SurfaceShim::Computer);
        let _host_environment = crate::tests::EnvGuard::set_many(host.environment());
        let road = ComputerRoad { program:dir.path().join(COMPUTER_SHIM) };
        let ctx = ToolContext::new();
        std::fs::write(dir.path().join("reply-click"), json!({"ok":true,"surfaceExecution":{
            "surfaceId":"fixture-host/computer/41/lease-a","stateRevision":7,"effect":{"kind":"acknowledged"}},
            "result":{"action":{"targetWindowId":41}}}).to_string()).unwrap();
        std::fs::write(dir.path().join("reply-observe"), json!({"ok":true,"surfaceExecution":{
            "surfaceId":"fixture-host/computer/41/lease-b","stateRevision":8,"effect":{"kind":"confirmedChange","previousRevision":7}},
            "result":{"window":{"id":41}}}).to_string()).unwrap();
        let executed = run_computer_execution(&json!({"action":"left_click","app":"Fixture","window_id":41,"label":"Next"}), &ctx, &road).unwrap();
        assert!(executed.surface.is_none(), "a changed control interval must not confirm the previous action");
        assert_eq!(calls(dir.path()), ["click", "observe"]);
    }

    #[test]
    fn every_window_identity_must_agree_before_its_contents_are_used() {
        for result in [
            json!({"window": {"id": 41}}),
            json!({"tree": {"window": {"id": 41}}}),
            json!({"snapshot": {"window": {"id": 41}}}),
            json!({"action": {"targetWindowId": 41}}),
        ] {
            assert_eq!(checked_window(&result, Some(41)).unwrap(), Some(41));
            assert_eq!(checked_window(&json!({"result": result}), Some(41)).unwrap(), Some(41));
        }
        for answer in [
            json!({}),
            json!({"result": {"window": {"id": 0}}}),
            json!({"result": {"window": {"id": 52}}}),
            json!({"result": {"action": {"targetWindowId": 41}, "snapshot": {"window": {"id": 52}}}}),
        ] {
            assert!(checked_window(&answer, Some(41)).is_err(), "unproved scope: {answer}");
        }
        assert!(checked_window(&json!({"window": {"id": 41}, "tree": {"window": {"id": 52}}}), None).is_err());
    }

    #[test]
    fn windows_post_action_source_keeps_the_requested_window_selector() {
        let source = include_str!("../../../../crates/zerocode-shell/src/computer_use/windows/provider.rs");
        let body = source.split_once("fn run_action(").unwrap().1.split_once("/// An answer as JSON;").unwrap().0;
        assert_eq!(body.matches("self.observe(").count(), 1, "post-action observation must not retry another window");
        assert!(!body.contains("remove(\"windowId\")"));
        assert!(!body.contains("remove(\"windowIndex\")"));
    }

    #[test]
    fn background_reads_preserve_the_same_selected_window() {
        for action in ["screenshot", "observe", "find", "read", "wait_for"] {
            let mut fields = json!({"action": action, "app": "Fixture", "window_id": 41, "background": true});
            if matches!(action, "find" | "wait_for") { fields["label"] = json!("Save"); }
            let command = zerocode_core::computer_use::parse_command(&argv_for(&input(fields)).unwrap()).unwrap();
            assert_eq!(command.params.get("windowId"), Some(&json!(41)), "{action}");
            assert_eq!(command.params.get("background"), Some(&json!(true)), "{action}");
        }
    }

    #[test]
    fn app_scrolling_keeps_its_units_and_never_accepts_desktop_wheel_clicks() {
        let fields = input(json!({"action": "scroll", "app": "Fixture", "window_id": 41, "coordinate": [20, 40], "scroll_direction": "down", "scroll_pages": 0.25}));
        let command = zerocode_core::computer_use::parse_command(&argv_for(&fields).unwrap()).unwrap();
        assert_eq!(command.params.get("pages"), Some(&json!(0.25)));
        for fields in [
            json!({"action": "scroll", "app": "Fixture", "window_id": 41, "coordinate": [20, 40], "scroll_direction": "down", "scroll_amount": 2}),
            json!({"action": "scroll", "coordinate": [20, 40], "scroll_direction": "down", "scroll_pages": 2}),
            json!({"action": "scroll", "app": "Fixture", "window_id": 41, "coordinate": [20, 40], "scroll_direction": "down", "scroll_pages": 0}),
        ] {
            assert!(argv_for(&input(fields)).is_err());
        }
    }

    #[cfg(unix)]
    #[test]
    fn coordinates_from_one_window_cannot_be_used_in_another_window() {
        let dir = tempfile::tempdir().unwrap();
        let (road, _) = fake_shim(dir.path());
        let ctx = crate::tests::tool_context_outside_cwd_windows();
        run_computer(&json!({"action": "observe", "app": "Fixture", "window_id": 41}), &ctx, &road).unwrap();
        let error = run_computer(&json!({"action": "left_click", "app": "Fixture", "window_id": 52, "coordinate": [20, 40]}), &ctx, &road).unwrap_err();
        assert!(error.to_string().contains("last picture is not of this app/window"));
        assert_eq!(calls(dir.path()), ["observe"]);
        run_computer(&json!({"action": "left_click", "app": "Fixture", "window_id": 41, "coordinate": [20, 40]}), &ctx, &road).unwrap();
        assert_eq!(calls(dir.path()), ["observe", "click", "observe"]);
    }

    #[cfg(unix)]
    #[test]
    fn actual_observed_window_is_kept_even_when_the_request_named_only_the_app() {
        let dir = tempfile::tempdir().unwrap();
        let (road, _) = fake_shim(dir.path());
        std::fs::write(dir.path().join("forced-window"), "41").unwrap();
        let ctx = crate::tests::tool_context_outside_cwd_windows();
        run_computer(&json!({"action": "observe", "app": "Fixture"}), &ctx, &road).unwrap();
        assert_eq!(ctx.computer_observed_target(), Some(ComputerTarget { app: "Fixture".into(), window_id: Some(41) }));
    }

    #[cfg(unix)]
    #[test]
    fn invalid_scope_is_refused_before_any_read_or_input() {
        let dir = tempfile::tempdir().unwrap();
        let (road, _) = fake_shim(dir.path());
        let ctx = crate::tests::tool_context_outside_cwd_windows();
        for fields in [
            json!({"action": "left_click", "app": "Fixture", "window": "A document", "label": "Save"}),
            json!({"action": "key", "window_id": 41, "text": "Tab"}),
            json!({"action": "batch", "window_id": 41, "steps": [{"action": "type", "text": "fixture"}]}),
            json!({"action": "batch", "background": true, "steps": [{"action": "key", "app": "Fixture", "window_id": 41, "text": "Tab", "background": false}]}),
            json!({"action": "batch", "background": true, "steps": [{"action": "key", "text": "Tab"}]}),
            json!({"action": "batch", "steps": [{"action": "left_click", "app": "Fixture", "window_id": 41, "coordinate": [20, 40]}]}),
        ] {
            assert!(run_computer(&fields, &ctx, &road).is_err(), "{fields}");
        }
        assert!(calls(dir.path()).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn background_batches_pin_every_step_and_the_final_observation() {
        let dir = tempfile::tempdir().unwrap();
        let (road, _) = fake_shim(dir.path());
        let ctx = crate::tests::tool_context_outside_cwd_windows();
        let steps = json!([
            {"action": "left_click", "app": "Fixture", "window_id": 41, "label": "Save"},
            {"action": "type", "app": "Fixture", "window_id": 52, "text": "fixture"},
        ]);
        let reports = json!([
            {"n": 1, "ok": true, "result": {"action": {"targetWindowId": 41}}},
            {"n": 2, "ok": true, "result": {"snapshot": {"window": {"id": 52}}}},
        ]);
        std::fs::write(dir.path().join("batch"), json!({"ok": true, "result": {"ran": 2, "steps": reports}}).to_string()).unwrap();
        let answer: Value = serde_json::from_str(&run_computer(&json!({"action": "batch", "background": true, "steps": steps}), &ctx, &road).unwrap()).unwrap();
        let commands: Vec<Vec<String>> = serde_json::from_str(&std::fs::read_to_string(dir.path().join("batch-commands")).unwrap()).unwrap();
        for (command, window_id) in commands.iter().zip([41, 52]) {
            let parsed = zerocode_core::computer_use::parse_command(command).unwrap();
            assert_eq!(parsed.params.get("windowId"), Some(&json!(window_id)));
            assert_eq!(parsed.params.get("background"), Some(&json!(true)));
        }
        let observed = answer["looked_after"]["screen"]["said"].as_str().unwrap();
        assert!(observed.contains("--app Fixture") && observed.contains("--window-id 52"));
    }

    #[test]
    fn a_timed_wait_keeps_context_out_of_its_native_command() {
        for background in [false, true] {
            let fields = input(json!({"action": "wait", "duration": 0.01, "app": "Fixture", "window_id": 41, "background": background}));
            let argv = argv_for(&fields).expect("a timer does not inject input or read a window");
            assert_eq!(argv, ["wait", "--ms", "10", "--json"]);
            let command = zerocode_core::computer_use::parse_command(&argv).unwrap();
            assert_eq!(command.method, ComputerMethod::Wait);
            assert_eq!(command.params.get("ms"), Some(&json!(10)));
            assert!(command.params.get("app").is_none() && command.params.get("windowId").is_none() && command.params.get("background").is_none());
        }
    }

    #[test]
    fn a_timed_wait_cannot_silently_turn_a_condition_into_a_delay() {
        for condition in [json!({"text": "Ready"}), json!({"label": "Ready"}), json!({"role": "AXButton"}), json!({"ocr": true}), json!({"absent": true})] {
            let mut fields = json!({"action": "wait", "duration": 0.01, "app": "Fixture", "window_id": 41});
            fields.as_object_mut().unwrap().extend(condition.as_object().unwrap().clone());
            let error = argv_for(&input(fields)).expect_err("a condition needs a conditional wait");
            assert!(error.to_string().contains("wait_for"), "{error}");
        }
    }

    #[test]
    fn title_waits_cannot_claim_a_window_scope_the_executor_does_not_check() {
        for scope in [json!({"app": "Fixture"}), json!({"app": "Fixture", "window_id": 41}), json!({"background": true})] {
            let mut fields = json!({"action": "wait_for", "window": "Target document", "timeout_ms": 50});
            fields.as_object_mut().unwrap().extend(scope.as_object().unwrap().clone());
            assert!(argv_for(&input(fields)).is_err(), "title waits scan the window list");
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_scoped_timed_wait_requires_no_window_receipt_or_observation() {
        let dir = tempfile::tempdir().unwrap();
        let (road, _) = fake_shim(dir.path());
        let ctx = crate::tests::tool_context_outside_cwd_windows();
        let response = run_computer(&json!({"action": "wait", "duration": 0.01, "app": "Fixture", "window_id": 41, "background": true}), &ctx, &road).unwrap();
        let answer: Value = serde_json::from_str(&response).unwrap();
        assert_eq!(answer["ok"], true);
        assert_eq!(calls(dir.path()), ["wait"]);
        assert!(answer["first_look"].is_null() && answer["looked_after"].is_null());
        assert_eq!(staged(&ctx), 0);
        assert_eq!(answer["result"]["said"], "wait --ms 10 --json");
    }

    #[cfg(unix)]
    #[test]
    fn timer_only_batch_defaults_do_not_claim_window_observations() {
        let dir = tempfile::tempdir().unwrap();
        let (road, _) = fake_shim(dir.path());
        let ctx = crate::tests::tool_context_outside_cwd_windows();
        std::fs::write(dir.path().join("batch"), json!({"ok": true, "result": {"ran": 1, "steps": [
            {"n": 1, "ok": true, "result": {}},
        ]}}).to_string()).unwrap();
        let response = run_computer(&json!({"action": "batch", "app": "Fixture", "window_id": 41, "background": true,
            "steps": [{"action": "wait", "duration": 0.01}]}), &ctx, &road).unwrap();
        let answer: Value = serde_json::from_str(&response).unwrap();
        assert_eq!(answer["ran"], 1);
        assert!(answer["first_look"].is_null() && answer["looked_after"].is_null());
        assert_eq!(calls(dir.path()), ["batch"]);
        assert_eq!(staged(&ctx), 0);
    }

    #[test]
    fn timer_context_still_rejects_missing_invalid_or_empty_window_selectors() {
        for scope in [json!({"app": "Fixture"}), json!({"window_id": 41}), json!({"app": "Fixture", "window_id": 0}), json!({"app": " ", "window_id": 41})] {
            let mut fields = json!({"action": "wait", "duration": 0.01, "background": true});
            fields.as_object_mut().unwrap().extend(scope.as_object().unwrap().clone());
            assert!(argv_for(&input(fields)).is_err());
        }
    }

    #[cfg(unix)]
    #[test]
    fn batch_defaults_never_return_another_windows_final_observation() {
        let dir = tempfile::tempdir().unwrap();
        let (road, _) = fake_shim(dir.path());
        let ctx = crate::tests::tool_context_outside_cwd_windows();
        std::fs::write(dir.path().join("forced-window"), "52").unwrap();
        std::fs::write(dir.path().join("batch"), json!({"ok": true, "result": {"ran": 2, "steps": [
            {"n": 1, "ok": true, "result": {"action": {"targetWindowId": 41}}},
            {"n": 2, "ok": true, "result": {}},
        ]}}).to_string()).unwrap();
        let response = run_computer(&json!({"action": "batch", "app": "Fixture", "window_id": 41, "background": true, "steps": [
            {"action": "left_click", "label": "Save"}, {"action": "wait", "duration": 0.01},
        ]}), &ctx, &road).unwrap();
        let answer: Value = serde_json::from_str(&response).unwrap();
        assert_eq!(answer["ran"], 2);
        assert!(answer["looked_after"]["error"].as_str().unwrap().contains("window_scope_mismatch"));
        assert!(!response.contains("unrelated-window-fixture"));
        assert_eq!(staged(&ctx), 0);
        assert_eq!(calls(dir.path()), ["batch", "observe"]);
    }

    #[cfg(unix)]
    #[test]
    fn batch_window_defaults_reach_inputs_and_conditional_waits_but_not_timer_flags() {
        let dir = tempfile::tempdir().unwrap();
        let (road, _) = fake_shim(dir.path());
        let ctx = crate::tests::tool_context_outside_cwd_windows();
        let reports = json!([
            {"n": 1, "ok": true, "result": {"action": {"targetWindowId": 41}}},
            {"n": 2, "ok": true, "result": {}},
            {"n": 3, "ok": true, "result": {"snapshot": {"window": {"id": 41}}}},
            {"n": 4, "ok": true, "result": {"window": {"id": 41}}},
        ]);
        std::fs::write(dir.path().join("batch"), json!({"ok": true, "result": {"ran": 4, "steps": reports}}).to_string()).unwrap();
        let response = run_computer(&json!({"action": "batch", "app": "Fixture", "window_id": 41, "background": true, "steps": [
            {"action": "left_click", "label": "Save", "role": "AXButton"},
            {"action": "wait", "duration": 0.01},
            {"action": "type", "app": "Fixture", "text": "fixture"},
            {"action": "wait_for", "window_id": 41, "label": "Ready", "role": "AXStaticText", "timeout_ms": 50},
        ]}), &ctx, &road).unwrap();
        let answer: Value = serde_json::from_str(&response).unwrap();
        assert_eq!(answer["ran"], 4);
        assert_eq!(calls(dir.path()), ["observe", "batch", "observe"]);
        let commands: Vec<Vec<String>> = serde_json::from_str(&std::fs::read_to_string(dir.path().join("batch-commands")).unwrap()).unwrap();
        for at in [0, 2, 3] {
            let command = zerocode_core::computer_use::parse_command(&commands[at]).unwrap();
            assert_eq!(command.params.get("app"), Some(&json!("Fixture")));
            assert_eq!(command.params.get("windowId"), Some(&json!(41)));
            assert_eq!(command.params.get("background"), Some(&json!(true)));
        }
        assert_eq!(commands[1], ["wait", "--ms", "10", "--json"]);
        assert_eq!(zerocode_core::computer_use::parse_command(&commands[0]).unwrap().params.get("role"), Some(&json!("AXButton")));
        assert_eq!(zerocode_core::computer_use::parse_command(&commands[3]).unwrap().params.get("role"), Some(&json!("AXStaticText")));
        let observed = answer["looked_after"]["screen"]["said"].as_str().unwrap();
        assert!(observed.contains("--app Fixture") && observed.contains("--window-id 41"));
        assert!(!response.contains("window_scope_unverified"));
    }

    #[cfg(unix)]
    #[test]
    fn background_batches_accept_timers_between_explicitly_scoped_inputs() {
        let dir = tempfile::tempdir().unwrap();
        let (road, _) = fake_shim(dir.path());
        let ctx = crate::tests::tool_context_outside_cwd_windows();
        let reports = json!([
            {"n": 1, "ok": true, "result": {"action": {"targetWindowId": 41}}},
            {"n": 2, "ok": true, "result": {}},
        ]);
        std::fs::write(dir.path().join("batch"), json!({"ok": true, "result": {"ran": 2, "steps": reports}}).to_string()).unwrap();
        let response = run_computer(&json!({"action": "batch", "background": true, "steps": [
            {"action": "left_click", "app": "Fixture", "window_id": 41, "label": "Save"},
            {"action": "wait", "duration": 0.01},
        ]}), &ctx, &road).unwrap();
        let answer: Value = serde_json::from_str(&response).unwrap();
        assert_eq!(answer["ran"], 2);
        assert_eq!(calls(dir.path()), ["batch", "observe"]);
        let observed = answer["looked_after"]["screen"]["said"].as_str().unwrap();
        assert!(observed.contains("--window-id 41"));
    }

    #[cfg(unix)]
    #[test]
    fn timer_context_cannot_redirect_the_observation_after_an_input() {
        let dir = tempfile::tempdir().unwrap();
        let (road, _) = fake_shim(dir.path());
        let ctx = crate::tests::tool_context_outside_cwd_windows();
        let reports = json!([
            {"n": 1, "ok": true, "result": {"action": {"targetWindowId": 41}}},
            {"n": 2, "ok": true, "result": {}},
        ]);
        std::fs::write(dir.path().join("batch"), json!({"ok": true, "result": {"ran": 2, "steps": reports}}).to_string()).unwrap();
        let response = run_computer(&json!({"action": "batch", "steps": [
            {"action": "left_click", "app": "Fixture", "window_id": 41, "label": "Save"},
            {"action": "wait", "app": "Fixture", "window_id": 52, "background": true, "duration": 0.01},
        ]}), &ctx, &road).unwrap();
        let answer: Value = serde_json::from_str(&response).unwrap();
        assert_eq!(answer["ran"], 2);
        let observed = answer["looked_after"]["screen"]["said"].as_str().unwrap();
        assert!(observed.contains("--window-id 41") && !observed.contains("--window-id 52"));
    }

    #[cfg(unix)]
    #[test]
    fn background_mode_does_not_add_input_flags_to_non_input_waits() {
        let dir = tempfile::tempdir().unwrap();
        let (road, _) = fake_shim(dir.path());
        let ctx = crate::tests::tool_context_outside_cwd_windows();
        let reports = json!([
            {"n": 1, "ok": true, "result": {}},
            {"n": 2, "ok": true, "result": {}},
            {"n": 3, "ok": true, "result": {}},
        ]);
        std::fs::write(dir.path().join("batch"), json!({"ok": true, "result": {"ran": 3, "steps": reports}}).to_string()).unwrap();
        let response = run_computer(&json!({"action": "batch", "background": true, "steps": [
            {"action": "wait", "duration": 0.01, "background": true},
            {"action": "sound_wait", "text": "notification", "timeout_ms": 50, "background": true},
            {"action": "watch", "until": "quiet", "timeout_ms": 50, "background": true},
        ]}), &ctx, &road).unwrap();
        let answer: Value = serde_json::from_str(&response).unwrap();
        assert_eq!(answer["ran"], 3);
        assert_eq!(calls(dir.path()), ["batch"]);
        assert!(answer["looked_after"].is_null());
        let commands: Vec<Vec<String>> = serde_json::from_str(&std::fs::read_to_string(dir.path().join("batch-commands")).unwrap()).unwrap();
        for command in commands {
            assert!(!command.iter().any(|word| matches!(word.as_str(), "--app" | "--window-id" | "--background")));
            assert!(!zerocode_core::computer_use::parse_command(&command).unwrap().method.acts());
        }
    }

    #[cfg(unix)]
    #[test]
    fn batch_scope_defaults_reject_conflicts_and_ambiguity_before_any_read() {
        let dir = tempfile::tempdir().unwrap();
        let (road, _) = fake_shim(dir.path());
        let ctx = crate::tests::tool_context_outside_cwd_windows();
        for extra in [
            json!({"app": "OtherFixture"}), json!({"window_id": 52}), json!({"window_id": 0}), json!({"app": ""}),
            json!({"background": false}), json!({"window": "Target document"}),
        ] {
            let mut step = json!({"action": "type", "text": "fixture"});
            step.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
            let fields = json!({"action": "batch", "app": "Fixture", "window_id": 41, "background": true, "steps": [
                {"action": "left_click", "label": "Save"}, step,
            ]});
            assert!(run_computer(&fields, &ctx, &road).is_err(), "conflicting default: {fields}");
        }
        for scope in [json!({"app": "Fixture"}), json!({"window_id": 41}), json!({"app": "Fixture", "window_id": 0}), json!({"app": " ", "window_id": 41}), json!({"window": "Target document"})] {
            let mut fields = json!({"action": "batch", "steps": [{"action": "wait", "duration": 0.01}]});
            fields.as_object_mut().unwrap().extend(scope.as_object().unwrap().clone());
            assert!(run_computer(&fields, &ctx, &road).is_err(), "ambiguous default: {fields}");
        }
        let fields = json!({"action": "batch", "background": true, "steps": [{"action": "wait", "duration": 0.01, "background": false}]});
        let error = run_computer(&fields, &ctx, &road).expect_err("an explicit foreground downgrade remains an error");
        assert!(error.to_string().contains("foreground"));
        assert!(calls(dir.path()).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn a_window_default_cannot_be_dropped_by_an_unsupported_step() {
        let dir = tempfile::tempdir().unwrap();
        let (road, _) = fake_shim(dir.path());
        let ctx = crate::tests::tool_context_outside_cwd_windows();
        for step in [
            json!({"action": "watch", "until": "quiet", "timeout_ms": 50}),
            json!({"action": "sound_wait", "text": "notification", "timeout_ms": 50}),
            json!({"action": "activate", "app": "Fixture"}),
            json!({"action": "hold_key", "text": "Tab", "duration": 0.01}),
            json!({"action": "clipboard_write", "text": "fixture"}),
        ] {
            let fields = json!({"action": "batch", "app": "Fixture", "window_id": 41, "steps": [{"action": "left_click", "label": "Save"}, step]});
            assert!(run_computer(&fields, &ctx, &road).is_err(), "scope cannot be ignored: {fields}");
        }
        assert!(calls(dir.path()).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn batch_defaults_keep_coordinate_index_mark_and_late_validation_guards() {
        let dir = tempfile::tempdir().unwrap();
        let (road, _) = fake_shim(dir.path());
        let ctx = crate::tests::tool_context_outside_cwd_windows();
        for step in [
            json!({"action": "left_click", "coordinate": [20, 40]}),
            json!({"action": "left_click_drag", "start_coordinate": [20, 40], "coordinate": [30, 50]}),
            json!({"action": "left_click", "element_index": 3}),
            json!({"action": "left_click", "mark": 3}),
            json!({"action": "wait", "duration": -1}),
            json!({"action": "left_click"}),
            json!({"action": "batch", "steps": [{"action": "wait", "duration": 0.01}]}),
        ] {
            let fields = json!({"action": "batch", "app": "Fixture", "window_id": 41, "steps": [{"action": "left_click", "label": "Save"}, step]});
            assert!(run_computer(&fields, &ctx, &road).is_err(), "unsafe step: {fields}");
        }
        assert!(calls(dir.path()).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn a_conditional_wait_still_needs_its_selected_windows_receipt() {
        let dir = tempfile::tempdir().unwrap();
        let (road, _) = fake_shim(dir.path());
        let ctx = crate::tests::tool_context_outside_cwd_windows();
        let reports = json!([
            {"n": 1, "ok": true, "result": {"action": {"targetWindowId": 41}}},
            {"n": 2, "ok": true, "result": {}},
        ]);
        std::fs::write(dir.path().join("batch"), json!({"ok": true, "result": {"ran": 2, "steps": reports}}).to_string()).unwrap();
        let response = run_computer(&json!({"action": "batch", "app": "Fixture", "window_id": 41, "background": true, "steps": [
            {"action": "left_click", "label": "Save"},
            {"action": "wait_for", "label": "Ready", "timeout_ms": 50},
        ]}), &ctx, &road).unwrap();
        let receipt: Value = serde_json::from_str(&response).unwrap();
        assert_eq!(receipt["input_dispatched"], true);
        assert_eq!(receipt["do_not_repeat"], true);
        assert_eq!(receipt["completion_verified"], false);
        assert_eq!(receipt["ran"], 2);
        assert!(receipt["result"].is_null());
        assert!(response.contains("window_scope_unverified"));
        assert_eq!(calls(dir.path()), ["observe", "batch"]);
    }

    #[cfg(unix)]
    #[test]
    fn batch_defaults_keep_partial_execution_and_keyboard_receiver_refusals() {
        let dir = tempfile::tempdir().unwrap();
        let (road, _) = fake_shim(dir.path());
        let ctx = crate::tests::tool_context_outside_cwd_windows();
        std::fs::write(dir.path().join("batch"), json!({
            "ok": false, "error": {"code": "focus_changed", "message": "the background keyboard receiver cannot be verified in the target window"},
            "result": {"ran": 2, "of": 3, "refusedAt": 3, "steps": [
                {"n": 1, "ok": true, "result": {"action": {"targetWindowId": 41}}},
                {"n": 2, "ok": true, "result": {}},
                {"n": 3, "ok": false, "error": {"code": "focus_changed", "message": "receiver unverified"}},
            ]},
        }).to_string()).unwrap();
        let response = run_computer(&json!({"action": "batch", "app": "Fixture", "window_id": 41, "background": true, "steps": [
            {"action": "left_click", "label": "Editor", "role": "AXTextArea"},
            {"action": "wait", "duration": 0.01},
            {"action": "type", "text": "fixture"},
        ]}), &ctx, &road).unwrap();
        let answer: Value = serde_json::from_str(&response).unwrap();
        assert_eq!(answer["ok"], false);
        assert_eq!(answer["ran"], 2);
        assert_eq!(answer["refused"]["step"], 3);
        assert_eq!(answer["refused"]["action"], "type");
        assert_eq!(answer["refused"]["code"], "focus_changed");
        assert_eq!(answer["steps"][2]["ok"], false);
        assert!(answer["looked_after"]["screen"]["said"].as_str().unwrap().contains("--window-id 41"));
        assert_eq!(calls(dir.path()), ["batch", "observe"]);
    }

    #[cfg(unix)]
    #[test]
    fn a_batch_never_discloses_a_different_windows_step_result_or_invites_repeating_input() {
        let dir = tempfile::tempdir().unwrap();
        let (road, _) = fake_shim(dir.path());
        let ctx = crate::tests::tool_context_outside_cwd_windows();
        let reports = json!([{"n": 1, "ok": true, "result": {"window": {"id": 52}, "text": "unrelated-window-fixture"}}]);
        std::fs::write(dir.path().join("batch"), json!({"ok": true, "result": {"ran": 1, "steps": reports}}).to_string()).unwrap();
        let response = run_computer(&json!({"action": "batch", "steps": [{"action": "left_click", "app": "Fixture", "window_id": 41, "label": "Save"}]}), &ctx, &road).unwrap();
        let receipt: Value = serde_json::from_str(&response).unwrap();
        assert_eq!(receipt["ok"], true);
        assert_eq!(receipt["input_dispatched"], true);
        assert_eq!(receipt["completion_verified"], false);
        assert_eq!(receipt["do_not_repeat"], true);
        assert!(receipt["result"].is_null());
        assert!(response.contains("window_scope_mismatch"));
        assert!(!response.contains("unrelated-window-fixture"));
        assert_eq!(calls(dir.path()), ["batch"]);
        assert_eq!(staged(&ctx), 0);
    }

    #[cfg(unix)]
    #[test]
    fn a_sent_click_with_unverified_scope_returns_a_receipt_not_a_repeatable_error() {
        for reported_window in ["52", "0"] {
            let dir = tempfile::tempdir().unwrap();
            let (road, _) = fake_shim(dir.path());
            std::fs::write(dir.path().join("forced-window"), reported_window).unwrap();
            let ctx = crate::tests::tool_context_outside_cwd_windows();
            let response = run_computer(&json!({"action": "left_click", "app": "Fixture", "window_id": 41, "label": "Save"}), &ctx, &road).unwrap();
            let receipt: Value = serde_json::from_str(&response).unwrap();
            assert_eq!(receipt["ok"], true);
            assert_eq!(receipt["input_dispatched"], true);
            assert_eq!(receipt["completion_verified"], false);
            assert_eq!(receipt["do_not_repeat"], true);
            assert_eq!(receipt["window_scope"]["requested_window_id"], 41);
            assert!(receipt["result"].is_null());
            assert!(!response.contains("unrelated-window-fixture"));
            assert_eq!(calls(dir.path()), ["click"]);
            assert_eq!(staged(&ctx), 0);
        }
    }

    #[cfg(unix)]
    #[test]
    fn an_app_screenshot_keeps_its_window_but_does_not_repeat_the_accessibility_tree() {
        let dir = tempfile::tempdir().unwrap();
        let (road, _) = fake_shim(dir.path());
        let script = std::fs::read_to_string(&road.program).unwrap().replace("\"cursor\":", "\"tree\":{\"text\":\"fixture tree\"},\"cursor\":");
        std::fs::write(&road.program, script).unwrap();
        let ctx = crate::tests::tool_context_outside_cwd_windows();
        let response = run_computer(&json!({"action": "screenshot", "app": "Fixture", "window_id": 41}), &ctx, &road).unwrap();
        let receipt: Value = serde_json::from_str(&response).unwrap();
        assert!(receipt["result"].get("tree").is_none());
        assert_eq!(ctx.computer_observed_target().unwrap().window_id, Some(41));
        assert_eq!(staged(&ctx), 1);
    }

    /// A process whose looks carry marks (`ZO_COMPUTER_MARKS`, the bench's
    /// A/B) numbers both of its looks and offers the `mark` field; one whose
    /// looks do not spells them as before and the field costs nothing. A
    /// batch step is never a click by mark either way.
    #[test]
    fn a_process_whose_looks_carry_marks_numbers_its_looks_and_offers_the_field() {
        assert_eq!(look_argv(LOOK_AFTER_ARGV, false), ["observe", "--diff", "--settle", "--json"]);
        assert_eq!(look_argv(LOOK_AFTER_ARGV, true), ["observe", "--diff", "--settle", "--marks", "--json"]);
        assert_eq!(look_argv(FRAME_LOOK_ARGV, true), ["observe", "--marks", "--json"]);
        assert_eq!(look_argv(FRAME_LOOK_ARGV, false), ["observe", "--json"]);
        let offered = |marks: bool| action_properties(&action_names(|_| true), &window_kinds(|_| true), None, marks);
        assert!(offered(true).get("mark").is_some());
        assert!(offered(false).get("mark").is_none());
        let step = action_properties(&action_names(|_| true), &window_kinds(|_| true), Some(STEP_FIELDS), true);
        assert!(step.get("mark").is_none(), "a step's look can change between steps");
    }

    /// A click by a mark's number is the CLI's `click --mark`, never beside a
    /// coordinate, never another action's, never a batch step; the look it
    /// names is the last marked one this model was shown.
    #[test]
    fn a_click_by_mark_is_the_clis_click_on_the_last_marked_look() {
        let spelled = |fields: Value| argv_for(&input(fields));
        assert_eq!(
            spelled(json!({ "action": "left_click", "mark": 7 })).unwrap(),
            ["click", "--mark", "7", "--mouse-button", "left", "--click-count", "1", "--json"]
        );
        assert_eq!(
            spelled(json!({ "action": "double_click", "mark": 3 })).unwrap(),
            ["click", "--mark", "3", "--mouse-button", "left", "--click-count", "2", "--json"]
        );
        assert_eq!(
            spelled(json!({ "action": "right_click", "mark": 2, "text": "shift" })).unwrap(),
            ["click", "--mark", "2", "--mouse-button", "right", "--click-count", "1", "--modifiers", "shift", "--json"]
        );
        assert!(spelled(json!({ "action": "left_click", "mark": 1, "coordinate": [1, 2] })).is_err());
        // An element of an app's tree is pressed through accessibility.
        assert_eq!(
            spelled(json!({ "action": "left_click", "app": "Mail", "element_index": 12 })).unwrap(),
            ["click", "--app", "Mail", "--element-index", "12", "--no-screenshot", "--json"]
        );
        assert_eq!(
            spelled(json!({ "action": "left_click", "app": "Mail", "label": "Send" })).unwrap(),
            ["click", "--app", "Mail", "--label", "Send", "--mouse-button", "left", "--click-count", "1", "--no-screenshot", "--json"]
        );
        assert_eq!(
            spelled(json!({ "action": "right_click", "app": "Mail", "label": "Inbox", "role": "row" })).unwrap(),
            ["click", "--app", "Mail", "--label", "Inbox", "--role", "row", "--mouse-button", "right", "--click-count", "1", "--no-screenshot", "--json"]
        );
        assert!(spelled(json!({ "action": "left_click", "label": "Send" })).is_err(), "a reading is an app's");
        assert!(spelled(json!({ "action": "left_click", "app": "Mail", "label": "Send", "element_index": 2 })).is_err(), "one target");
        assert_eq!(
            spelled(json!({ "action": "left_click", "app": "Mail", "element_index": 12 })).unwrap()[0],
            "click"
        );
        assert!(spelled(json!({ "action": "left_click", "element_index": 12 })).is_err(), "an element is an app's");
        assert!(spelled(json!({ "action": "left_click", "app": "Mail", "element_index": 1, "mark": 2 })).is_err());
        assert!(spelled(json!({ "action": "double_click", "app": "Mail", "element_index": 1 })).is_err(), "its press has no count");
        assert!(spelled(json!({ "action": "key", "text": "a", "mark": 1 })).is_err());
        assert!(batch_argv(&[input(json!({ "action": "left_click", "mark": 1 }))], ShotFrame::UNIT).is_err());
        assert!(
            batch_argv(&[input(json!({ "action": "left_click", "app": "Mail", "element_index": 1 }))], ShotFrame::UNIT).is_err(),
            "an element click goes alone too"
        );
        let ctx = crate::tests::tool_context_outside_cwd_windows();
        let argv = spelled(json!({ "action": "left_click", "mark": 7 })).unwrap();
        assert!(with_marked_look(argv.clone(), &ctx).is_err(), "no marked look yet");
        let mut screen = json!({ "marks": {
            "coordinateSpace": "shot", "lookId": "9.4", "app": "Mail", "pid": 30, "windowId": 9,
            "items": [{ "mark": 1, "elementIndex": 4, "role": "button", "label": "Back", "x": 10, "y": 20,
                        "width": 20, "height": 22, "centerX": 20.0, "centerY": 31.0 }],
            "candidates": 1, "omitted": 0 } });
        present_marks(&mut screen, &ctx);
        assert_eq!(screen["marks"], json!({ "legend": ["1 button Back @20,31"], "candidates": 1, "omitted": 0 }),
            "the model reads the legend, not the tool's words");
        assert_eq!(ctx.computer_marked_look().as_deref(), Some("9.4"));
        let named = with_marked_look(argv.clone(), &ctx).unwrap();
        assert_eq!(&named[named.len() - 3..], ["--look", "9.4", "--json"]);
        present_marks(&mut json!({ "marks": { "unavailable": "window_not_found: none" } }), &ctx);
        assert_eq!(ctx.computer_marked_look(), None, "a picture without numbers leaves none to click by");
        assert!(with_marked_look(argv.clone(), &ctx).is_err());
        present_marks(&mut screen_with_look("9.5"), &ctx);
        present_marks(&mut json!({ "screenshot": { "width": 640 } }), &ctx);
        assert_eq!(ctx.computer_marked_look(), None, "an unmarked picture (a screenshot, a zoom) too");
        let coordinate = spelled(json!({ "action": "left_click", "coordinate": [1, 2] })).unwrap();
        assert_eq!(with_marked_look(coordinate.clone(), &ctx).unwrap(), coordinate, "a coordinate click names no look");
    }

    /// The table: every Anthropic action and every extension becomes the CLI
    /// verb it means, with its flags in the CLI's spelling and `--json` last.
    #[test]
    fn every_action_maps_to_the_cli_verb_it_means() {
        let cases: Vec<(Value, &[&str])> = vec![
            (json!({ "action": "screenshot" }), &["screenshot", "--json"]),
            (json!({ "action": "zoom", "region": [10, 20, 300, 200] }), &["zoom", "--region", "10,20,300,200", "--json"]),
            (json!({ "action": "mouse_move", "coordinate": [100, 200] }), &["mouse-move", "--x", "100", "--y", "200", "--json"]),
            (json!({ "action": "left_click", "coordinate": [100.5, 200] }), &["mouse-click", "--x", "100.5", "--y", "200", "--mouse-button", "left", "--click-count", "1", "--json"]),
            (json!({ "action": "right_click", "coordinate": [1, 2], "text": "shift" }), &["mouse-click", "--x", "1", "--y", "2", "--mouse-button", "right", "--click-count", "1", "--modifiers", "shift", "--json"]),
            (json!({ "action": "middle_click", "coordinate": [1, 2] }), &["mouse-click", "--x", "1", "--y", "2", "--mouse-button", "middle", "--click-count", "1", "--json"]),
            (json!({ "action": "double_click", "coordinate": [1, 2] }), &["mouse-click", "--x", "1", "--y", "2", "--mouse-button", "left", "--click-count", "2", "--json"]),
            (json!({ "action": "triple_click", "coordinate": [1, 2], "confirming": "payment" }), &["mouse-click", "--x", "1", "--y", "2", "--mouse-button", "left", "--click-count", "3", "--confirming", "payment", "--json"]),
            (json!({ "action": "left_click_drag", "start_coordinate": [1, 2], "coordinate": [3, 4] }), &["mouse-drag", "--from-x", "1", "--from-y", "2", "--to-x", "3", "--to-y", "4", "--json"]),
            (json!({ "action": "scroll", "coordinate": [5, 6], "scroll_direction": "down", "scroll_amount": 2 }), &["mouse-scroll", "--x", "5", "--y", "6", "--dy", "-6", "--json"]),
            (json!({ "action": "scroll", "coordinate": [5, 6], "scroll_direction": "left" }), &["mouse-scroll", "--x", "5", "--y", "6", "--dx", "3", "--json"]),
            (json!({ "action": "key", "text": "cmd+s" }), &["key", "--key", "cmd+s", "--json"]),
            (json!({ "action": "key", "text": "Return", "confirming": "transfer" }), &["key", "--key", "Return", "--confirming", "transfer", "--json"]),
            (json!({ "action": "hold_key", "text": "shift", "duration": 1.5 }), &["hold-key", "--key", "shift", "--ms", "1500", "--json"]),
            (json!({ "action": "type", "text": "hello" }), &["type", "--text", "hello", "--json"]),
            (json!({ "action": "wait", "duration": 2 }), &["wait", "--ms", "2000", "--json"]),
            (json!({ "action": "cursor_position" }), &["cursor-position", "--json"]),
            (json!({ "action": "launch", "app": "TextEdit", "args": "--new", "timeout_ms": 3000 }), &["launch", "--app", "TextEdit", "--args", "--new", "--wait-ready", "3000", "--json"]),
            (json!({ "action": "quit", "app": "TextEdit", "force": true }), &["quit", "--app", "TextEdit", "--force", "--json"]),
            (json!({ "action": "activate", "app": "Finder" }), &["activate", "--app", "Finder", "--json"]),
            (json!({ "action": "open", "url": "https://example.test/", "app": "Safari" }), &["open", "--url", "https://example.test/", "--with", "Safari", "--json"]),
            (json!({ "action": "run", "program": "/bin/echo", "args": "hi", "path": "/tmp", "timeout_ms": 500 }), &["run", "--program", "/bin/echo", "--args", "hi", "--cwd", "/tmp", "--timeout-ms", "500", "--json"]),
            (json!({ "action": "find", "app": "Safari", "role": "button", "label": "Buy" }), &["find", "--app", "Safari", "--role", "button", "--label", "Buy", "--json"]),
            (json!({ "action": "find", "ocr": true, "region": [0, 0, 800, 600], "text": "Total" }), &["find", "--ocr", "--region", "0,0,800,600", "--text", "Total", "--json"]),
            (json!({ "action": "wait_for", "window": "Preferences", "timeout_ms": 5000, "absent": true }), &["wait-for", "--window", "Preferences", "--timeout-ms", "5000", "--absent", "--json"]),
            (json!({ "action": "read", "app": "Notes", "window_id": 12 }), &["read", "--app", "Notes", "--window-id", "12", "--json"]),
            (json!({ "action": "window", "kind": "move", "window_id": 7, "x": 10, "y": 20 }), &["window-move", "--id", "7", "--x", "10", "--y", "20", "--json"]),
            (json!({ "action": "window", "kind": "resize", "window_id": 7, "width": 800, "height": 600 }), &["window-resize", "--id", "7", "--width", "800", "--height", "600", "--json"]),
            (json!({ "action": "list_windows" }), &["list-all-windows", "--json"]),
            (json!({ "action": "clipboard_read" }), &["clipboard-read", "--json"]),
            (json!({ "action": "clipboard_write", "text": "x" }), &["clipboard-write", "--text", "x", "--json"]),
            (json!({ "action": "status" }), &["status", "--json"]),
            (json!({ "action": "stop" }), &["stop", "--json"]),
            (json!({ "action": "resume", "reset_budget": true }), &["resume", "--reset-budget", "--json"]),
            (json!({ "action": "evidence" }), &["evidence", "--last", "20", "--json"]),
            (json!({ "action": "listen_start" }), &["listen-start", "--json"]),
            (json!({ "action": "listen_start", "app": "Music" }), &["listen-start", "--app", "Music", "--json"]),
            (json!({ "action": "sound_read", "after": 4 }), &["sound-read", "--after", "4", "--json"]),
            (json!({ "action": "sound_wait", "text": "siren,alarm_clock", "min_confidence": 0.7, "timeout_ms": 5000, "after": 2 }),
                &["sound-wait", "--label", "siren,alarm_clock", "--min-confidence", "0.7", "--timeout-ms", "5000", "--after", "2", "--json"]),
            (json!({ "action": "listen_stop" }), &["listen-stop", "--json"]),
            (json!({ "action": "watch" }), &["watch", "--json"]),
            (json!({ "action": "observe" }), &["observe", "--diff", "--json"]),
            (json!({ "action": "observe", "app": "Mail", "window_id": 4, "ocr": true }),
                &["observe", "--diff", "--app", "Mail", "--window-id", "4", "--ocr", "--json"]),
            (json!({ "action": "watch", "until": "quiet", "timeout_ms": 3000 }), &["watch", "--until", "quiet", "--timeout-ms", "3000", "--json"]),
            (json!({ "action": "recipe_save", "name": "Mail", "text": "worked", "last": 5 }), &["recipe-save", "--name", "Mail", "--note", "worked", "--last", "5", "--json"]),
            (json!({ "action": "recipe_list" }), &["recipe-list", "--json"]),
            (json!({ "action": "recipe_show", "name": "Mail" }), &["recipe-show", "--name", "Mail", "--json"]),
            (json!({ "action": "recipe_run", "name": "x", "params": { "text-3": "hi" }, "start": 3 }),
                &["recipe-run", "--name", "x", "--params", "{\"text-3\":\"hi\"}", "--start", "3", "--json"]),
            (json!({ "action": "handoff", "text": "Type your password", "timeout_ms": 60000 }),
                &["handoff", "--reason", "Type your password", "--timeout-ms", "60000", "--json"]),
            (json!({ "action": "handoff", "text": "카카오톡 인증번호", "ask_code": true,
                "into": ["set-value", "--app", "Form", "--element-index", "3"], "timeout_ms": 180_000 }),
                &["handoff", "--reason", "카카오톡 인증번호", "--ask-code", "--into",
                    r#"["set-value","--app","Form","--element-index","3"]"#, "--timeout-ms", "180000", "--json"]),
            (json!({ "action": "walk", "goal": "도움말에서 설치 안내를 연다", "pane": "browser-13", "until": "설치 안내", "max_steps": 6 }),
                &["walk", "--goal", "도움말에서 설치 안내를 연다", "--pane", "browser-13", "--until", "설치 안내", "--steps", "6", "--json"]),
        ];
        let mut covered = std::collections::BTreeSet::new();
        for (fields, expected) in cases {
            let argv = argv_for(&input(fields.clone())).unwrap_or_else(|error| panic!("{fields}: {error}"));
            assert_eq!(argv, expected.iter().map(|w| (*w).to_string()).collect::<Vec<_>>(), "{fields}");
            // The table's method is the one the verb names: acts, presses and
            // batches are read off it, so it must be the verb's own.
            let action = fields["action"].as_str().unwrap();
            let ran = zerocode_core::computer_use::verb_method(&argv[0]).expect("a core verb");
            assert!(methods(action).any(|method| method == ran), "{action} runs {ran:?}");
            covered.insert(action.to_string());
        }
        let unpinned = action_names(|action| action != BATCH_ACTION && !covered.contains(action));
        assert!(unpinned.is_empty(), "actions with no row in the table: {unpinned:?}");
        assert!(argv_for(&input(json!({ "action": "left_click" }))).is_err(), "a click needs a coordinate");
        assert!(argv_for(&input(json!({ "action": "scroll", "coordinate": [1, 1], "scroll_direction": "sideways" }))).is_err());
        assert!(argv_for(&input(json!({ "action": "open", "url": "a", "path": "b" }))).is_err(), "one target only");
        assert!(argv_for(&input(json!({ "action": "wait", "duration": 0 }))).is_err(), "a positive wait");
        assert!(argv_for(&input(json!({ "action": "teleport" }))).is_err(), "an action from the table");
        assert!(argv_for(&input(json!({ "action": "recipe_run" }))).is_err(), "a recipe needs its name");
        assert!(argv_for(&input(json!({ "action": "handoff" }))).is_err(), "the person is told what to do");
        assert!(serde_json::from_value::<ComputerInput>(json!({ "action": "screenshot", "extra": 1 })).is_err(), "no unknown fields");
        let spec = tool_specs().pop().expect("one spec");
        assert_eq!(spec.name, "Computer");
        assert_eq!(spec.required_permission, PermissionMode::DangerFullAccess);
        assert_eq!(spec.input_schema["properties"]["action"]["enum"].as_array().map(Vec::len), Some(ACTIONS.len()));
    }

    /// The words of the input command that puts a typed code into a field of
    /// a desktop app (t-40807).
    const INTO_A_FIELD: [&str; 5] = ["set-value", "--app", "Form", "--element-index", "3"];

    /// A one-time code asked for on the person's turn, with the field it goes
    /// into.
    fn a_code_into_a_field() -> Value {
        json!({ "action": "handoff", "text": "The site's one-time code", "ask_code": true,
            "into": INTO_A_FIELD, "timeout_ms": 180_000 })
    }

    /// A one-time code is asked for with `ask_code` and the words of the input
    /// command that puts it into its field, `into` (t-40807): the window types
    /// what the person types, and the answer carries no code. The tool passes
    /// both through, never judged here — whether the reason names a secret is
    /// the window's to say — in the order the core's line takes them, with no
    /// word that would hand a code to the field from this side, and says in its
    /// one sentence that the code does not come back, so the model knows the road.
    #[test]
    fn the_persons_turn_asks_for_a_one_time_code_by_into() {
        let spec = tool_specs().pop().expect("one spec");
        let properties = &spec.input_schema["properties"];
        assert_eq!(properties["ask_code"]["type"], json!("boolean"), "the person's turn has no switch for a code");
        assert_eq!(properties["into"]["type"], json!("array"), "the person's turn has no place to put a code");
        assert_eq!(properties["into"]["items"]["type"], json!("string"), "the words of an input command are strings");
        assert!(
            spec.description.contains("ask_code") && spec.description.contains("into"),
            "the tool's sentence does not say what ask_code and into are for"
        );
        assert!(spec.description.contains("never receive"), "the tool's sentence does not say the code does not come back");
        for field in ["ask_code", "into"] {
            assert!(
                properties["steps"]["items"]["properties"].get(field).is_none(),
                "the person's turn is never a batch step: {field}"
            );
        }
        let asked = argv_for(&input(a_code_into_a_field())).expect("a code asked for with the field it goes into");
        let spelled = serde_json::to_string(&INTO_A_FIELD).expect("the words spell as JSON");
        assert_eq!(
            asked,
            ["handoff", "--reason", "The site's one-time code", "--ask-code", "--into", spelled.as_str(), "--timeout-ms", "180000", "--json"]
        );
        assert!(
            !asked.iter().any(|word| word == "--value" || word == "--text" || word.ends_with("-stdin")),
            "a word hands a code to the field from this side: {asked:?}"
        );
        let plain = argv_for(&input(json!({ "action": "handoff", "text": "x", "ask_code": false }))).expect("a handoff");
        assert_eq!(plain, ["handoff", "--reason", "x", "--json"], "ask_code false is the plain turn");
    }

    /// A code asked for with no field to put it in is refused before the
    /// shell is called (t-40807): the code never comes back to the model, so a
    /// handoff that asks for one and names no field has nowhere to send it, and
    /// a field named with no code to put in it is a mistake.
    #[cfg(unix)]
    #[test]
    fn a_code_asked_for_without_into_is_refused_before_the_shell_is_called() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (road, _) = fake_shim(dir.path());
        let ctx = crate::tests::tool_context_outside_cwd_windows();
        let cases = [
            ("ask_code alone", json!({ "action": "handoff", "text": "x", "ask_code": true })),
            ("an empty into", json!({ "action": "handoff", "text": "x", "ask_code": true, "into": [] })),
            ("into alone", json!({ "action": "handoff", "text": "x", "into": INTO_A_FIELD })),
            ("into beside ask_code false", json!({ "action": "handoff", "text": "x", "ask_code": false, "into": INTO_A_FIELD })),
        ];
        for (what, fields) in &cases {
            let answer = run_computer(fields, &ctx, &road);
            assert!(matches!(answer, Err(ToolError::InvalidInput(_))), "{what}: {answer:?}");
        }
        let verbs = calls(dir.path());
        assert!(verbs.is_empty(), "the shell was called for a code with no field to put it in: {verbs:?}");
        let refused = run_computer(&cases[0].1, &ctx, &road);
        let told = refused.err().map(|error| error.to_string()).unwrap_or_default();
        assert!(
            told.contains("into") && told.contains("never reaches this conversation"),
            "the refusal does not tell the model where the code goes: {told}"
        );
    }

    /// The person's turn changes nothing on the screen by itself, and the
    /// window's typing of the code into its field is no press of the model's:
    /// no look follows a handoff (t-40807), so no picture of the field the
    /// code is in comes back with the answer.
    #[cfg(unix)]
    #[test]
    fn a_handoff_never_takes_a_look_after_picture() {
        assert!(!acts("handoff") && !presses("handoff"), "a handoff is a turn the person takes, no act of the model's");
        let dir = tempfile::tempdir().expect("tempdir");
        let (road, _) = fake_shim(dir.path());
        let ctx = crate::tests::tool_context_outside_cwd_windows();
        let answer = run_computer(&a_code_into_a_field(), &ctx, &road);
        assert!(answer.is_ok(), "the handoff answers: {answer:?}");
        let verbs = calls(dir.path());
        assert_eq!(verbs, ["handoff"], "a look followed the handoff: {verbs:?}");
        let words = handoff_words(dir.path());
        assert!(words.iter().any(|word| word == "--into"), "the window was not told where to type the code: {words:?}");
        assert_eq!(ctx.computer_frame(), None, "a handoff leaves the model's picture as it was");
        assert_eq!(staged(&ctx), 0, "a picture came back with the code typed");
    }

    /// Looks are not followed by a look; a type is not a press; the presses
    /// are the clicks and the key that fires a default button.
    #[test]
    fn looks_acts_and_presses_are_told_apart() {
        for look in [
            "screenshot", "zoom", "cursor_position", "find", "read", "wait_for", "list_windows", "status", "evidence", "wait",
            "listen_start", "listen_stop", "sound_read", "sound_wait", "watch", "observe",
        ] {
            assert!(!acts(look) && !presses(look), "{look}");
        }
        for act in ["left_click", "type", "launch", "window", "run", "clipboard_write"] {
            assert!(acts(act), "{act}");
        }
        assert!(presses("left_click") && presses("key") && !presses("type") && !presses("scroll"));
        assert!(
            presses("hold_key") && presses("left_click_drag"),
            "a held key fires a default button and a drag clicks what it lets go of"
        );
        assert!(acts("recipe_run"), "a walk changes the screen: a look follows it");
        for look in ["recipe_save", "recipe_list", "recipe_show"] {
            assert!(!acts(look) && !presses(look), "{look}");
        }
    }

    /// A 1×1 PNG, the smallest picture the guard accepts.
    const TINY_PNG: &[u8] = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x01,
        0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x44, 0x41,
        0x54, 0x78, 0x9C, 0x63, 0xF8, 0xCF, 0xC0, 0x00, 0x00, 0x03, 0x01, 0x01, 0x00, 0x18, 0xDD, 0x8D, 0xB0, 0x00, 0x00, 0x00,
        0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];

    /// A frame at half scale, to the left of the main display.
    fn half(origin: (f64, f64)) -> ShotFrame {
        ShotFrame::new(origin, 0.5).expect("a frame")
    }

    /// The model's pixels become the hand's points, positions and lengths
    /// alike; what carries no position is left as it is.
    #[test]
    fn what_the_model_says_in_pixels_is_moved_in_points() {
        let frame = half((-100.0, 0.0));
        let argv = |fields: Value| argv_for(&in_points(input(fields), frame).expect("in points")).expect("argv");
        assert_eq!(
            argv(json!({ "action": "left_click", "coordinate": [100, 40] }))[..5],
            ["mouse-click", "--x", "100", "--y", "80"],
            "100 px at half scale is 200 points past an origin 100 points to the left"
        );
        assert_eq!(
            argv(json!({ "action": "left_click_drag", "start_coordinate": [0, 0], "coordinate": [10, 10] }))[..9],
            ["mouse-drag", "--from-x", "-100", "--from-y", "0", "--to-x", "-80", "--to-y", "20"]
        );
        assert_eq!(
            argv(json!({ "action": "zoom", "region": [50, 50, 100, 60] }))[2],
            "0,100,100,20",
            "a region is two corners, the Anthropic way: 50 px wide and 10 px high here"
        );
        assert!(
            in_points(input(json!({ "action": "zoom", "region": [100, 60, 50, 50] })), frame).is_err(),
            "the bottom-right corner comes second"
        );
        assert_eq!(
            argv(json!({ "action": "window", "kind": "resize", "window_id": 7, "width": 400, "height": 300 }))[3..7],
            ["--width", "800", "--height", "600"]
        );
        assert_eq!(argv(json!({ "action": "key", "text": "cmd+s" })), ["key", "--key", "cmd+s", "--json"]);
        assert!(!speaks_in_pixels(&input(json!({ "action": "type", "text": "x" }))));
        assert!(speaks_in_pixels(&input(json!({ "action": "window", "kind": "move", "window_id": 1, "x": 0, "y": 0 }))));
        assert!(needs_a_frame(&input(json!({ "action": "find", "ocr": true, "text": "Save" }))), "its answer names places");
        assert!(!needs_a_frame(&input(json!({ "action": "key", "text": "Tab" }))));
    }

    /// The one-time code the synthetic window of the code tests answers with
    /// when it is the old window (t-40807).
    #[cfg(unix)]
    const A_CODE: &str = "ZC7TEST9";

    /// A fake shim on disk, answering like the window: a look carries a frame
    /// at half scale and what changed (read from a file the test flips), and
    /// every answer carries a position in points. A handoff answers as its
    /// window does: with an `--into` in its line the answer carries no code,
    /// without one it carries the old window's (`A_CODE`); the line's words
    /// are kept, one to a line. The `n`th call of a verb keeps its words in
    /// `argv-<verb>.<n>` and answers `reply-<verb>.<n>` when the test wrote
    /// one; a window it answers for shows the title in `window-title`, and
    /// gives the identity in `window-document` as its document's.
    #[cfg(unix)]
    pub(crate) fn fake_shim(dir: &Path) -> (ComputerRoad, PathBuf) {
        use std::os::unix::fs::PermissionsExt as _;
        let png = dir.join("shot.png");
        std::fs::write(&png, TINY_PNG).unwrap();
        let changed = dir.join("changed");
        std::fs::write(&changed, "null").unwrap();
        let program = dir.join(COMPUTER_SHIM);
        std::fs::write(dir.join("batch"), r#"{"ok":true,"result":{"ran":0,"of":0,"steps":[]}}"#).unwrap();
        let script = r#"#!/bin/sh
verb="$1"
echo "$verb" >> '@DIR@/calls'
asked="$(grep -cx "$verb" '@DIR@/calls')"
for word in "$@"; do printf '%s\n' "$word"; done > '@DIR@/argv-'"$verb"'.'"$asked"
window=''
previous=''
for word in "$@"; do
  [ "$previous" = --window-id ] && window="$word"
  previous="$word"
done
[ -f '@DIR@/forced-window' ] && window="$(cat '@DIR@/forced-window')"
title=''
[ -f '@DIR@/window-title' ] && title=',"title":"'"$(cat '@DIR@/window-title')"'"'
[ -f '@DIR@/window-document' ] && title="$title"',"document":"'"$(cat '@DIR@/window-document')"'"'
target=''
[ -n "$window" ] && target=',"window":{"id":'"$window""$title"'},"fixture":"unrelated-window-fixture"'
[ -f '@DIR@/reply-'"$verb"'.'"$asked" ] && { cat '@DIR@/reply-'"$verb"'.'"$asked"; echo; exit 0; }
[ -f '@DIR@/reply-'"$verb" ] && { cat '@DIR@/reply-'"$verb"; echo; exit 0; }
[ "$verb" = batch ] && { printf '%s' "$3" > '@DIR@/batch-commands'; cat '@DIR@/batch'; echo; exit 0; }
[ "$verb" = wait-for ] && [ -f '@DIR@/wait-for' ] && { for word in "$@"; do printf '%s\n' "$word"; done > '@DIR@/wait-for-argv'; cat '@DIR@/wait-for'; echo; exit 0; }
[ "$verb" = list-all-windows ] && [ -f '@DIR@/list-all-windows' ] && { cat '@DIR@/list-all-windows'; echo; exit 0; }
[ "$verb" = walk ] && [ -f '@DIR@/walk' ] && { printf '%s\n' "$*" > '@DIR@/walk-argv'; cat '@DIR@/walk'; echo; exit 0; }
if [ "$verb" = handoff ]; then
  for word in "$@"; do printf '%s\n' "$word"; done > '@DIR@/handoff-argv'
  case "$*" in
    *--into*) printf '%s\n' '{"ok":true,"result":{"resumed":true,"reason":"a code","codeLength":8,"entered":true,"into":{"tool":"computer","verb":"set-value"},"verification":"verified"}}' ;;
    *) printf '%s\n' '{"ok":true,"result":{"resumed":true,"reason":"a code","code":"@CODE@","codeLength":8}}' ;;
  esac
  exit 0
fi
case "$verb" in
  screenshot|observe)
    cp '@PNG@' '@PNG@'.$$
    look=',"screenshot":{"path":"@PNG@.'$$'","width":640,"height":415,"scale":0.5},"origin":{"x":0,"y":0},"changed":'"$(cat '@CHANGED@')" ;;
  *) look='' ;;
esac
printf '%s
' '{"ok":true,"result":{"verb":"'"$verb"'","said":"'"$*"'","cursor":{"x":200,"y":80}'"$look$target"'}}'
"#
        .replace("@PNG@", &png.display().to_string())
        .replace("@CHANGED@", &changed.display().to_string())
        .replace("@CODE@", A_CODE)
        .replace("@DIR@", &dir.display().to_string());
        std::fs::write(&program, script).unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        (ComputerRoad { program }, changed)
    }

    #[cfg(unix)]
    fn staged(ctx: &ToolContext) -> usize {
        ctx.image_sink.lock().unwrap_or_else(std::sync::PoisonError::into_inner).len()
    }

    /// The desktop list a shim answers: two windows of Notes, one of Mail.
    #[cfg(unix)]
    fn desktop_with_windows(dir: &Path) {
        let row = |id: u64, app: &str, pid: u64| json!({ "id": id, "title": "", "app": { "name": app, "pid": pid }, "x": 0, "y": 0, "width": 640, "height": 400 });
        let answer = json!({ "ok": true, "result": { "windows": [row(11, "Notes", 501), row(12, "Mail", 502), row(13, "Notes", 501)] } });
        std::fs::write(dir.join("list-all-windows"), answer.to_string()).unwrap();
    }

    /// t-55888: the person saw `list_windows` with an `app` refused ("no desktop fallback was
    /// attempted"); the app is a filter on the list, and an app with no window is not a refusal.
    #[cfg(unix)]
    #[test]
    fn list_windows_takes_an_app_as_a_filter_and_steps_back_to_the_whole_list() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (road, _) = fake_shim(dir.path());
        desktop_with_windows(dir.path());
        let ctx = crate::tests::tool_context_outside_cwd_windows();
        let list = |app: &str| -> Value {
            let answer = run_computer(&json!({ "action": "list_windows", "app": app }), &ctx, &road).expect("list_windows takes an app");
            serde_json::from_str(&answer).unwrap()
        };
        let ids = |answer: &Value| -> Vec<u64> {
            answer["result"]["windows"].as_array().unwrap().iter().filter_map(|row| row["id"].as_u64()).collect()
        };
        let notes = list("notes");
        assert_eq!(ids(&notes), [11, 13], "only the named app's windows, any case");
        assert_eq!(notes["result"]["app_filter"]["state"], "applied");
        assert_eq!(notes["result"]["app_filter"]["kept"], 2);
        assert_eq!(notes["result"]["app_filter"]["of"], 3);
        assert_eq!(ids(&list("pid:502")), [12], "the helper's own pid: form names an app too");
        let missing = list("Calendar");
        assert_eq!(ids(&missing), [11, 12, 13], "no window of that app: the whole desktop list, not a refusal");
        assert_eq!(missing["result"]["app_filter"]["state"], "fell_back");
        let plain: Value = serde_json::from_str(&run_computer(&json!({ "action": "list_windows" }), &ctx, &road).unwrap()).unwrap();
        assert!(plain["result"].get("app_filter").is_none(), "no app, no filter receipt");
        assert_eq!(ids(&plain), [11, 12, 13]);
    }

    /// t-55888: `observe` with `"true"` failed twice with `invalid type: string "true", expected
    /// a boolean`; every boolean and number of every action reads the common spellings.
    #[test]
    fn booleans_and_numbers_accept_the_strings_a_model_writes() {
        let strict: ComputerInput = serde_json::from_value(json!({
            "action": "observe", "app": "Fixture", "background": true, "window_id": 41, "timeout_ms": 250, "duration": 0.5,
        })).unwrap();
        let loose: ComputerInput = serde_json::from_value(json!({
            "action": "observe", "app": "Fixture", "background": "true", "window_id": "41", "timeout_ms": " 250 ", "duration": "0.5",
        })).expect("strings read as the values they spell");
        assert_eq!(
            (loose.background, loose.window_id, loose.timeout_ms, loose.duration),
            (strict.background, strict.window_id, strict.timeout_ms, strict.duration),
            "the normalized value is the strict one"
        );
        for (spelled, flag) in [("true", true), ("TRUE", true), ("1", true), ("false", false), ("False", false), ("0", false)] {
            for field in ["background", "ocr", "absent", "ask_code", "force", "reset_budget", "overlap", "rescue", "replay", "settle"] {
                let input: ComputerInput = serde_json::from_value(json!({ "action": "observe", (field): spelled }))
                    .unwrap_or_else(|error| panic!("{field}: {spelled:?}: {error}"));
                let read = [input.background, input.ocr, input.absent, input.ask_code, input.force, input.reset_budget, input.overlap, input.rescue, input.replay, input.settle];
                assert_eq!(read.iter().flatten().copied().collect::<Vec<_>>(), [flag], "{field}: {spelled:?}");
            }
        }
        let steps: ComputerInput = serde_json::from_value(json!({ "action": "batch", "steps": [{ "action": "wait", "duration": "1" }, { "action": "observe", "background": "false" }] })).expect("a step reads the same way");
        assert_eq!(steps.steps.unwrap()[1].background, Some(false));
        let bads = [
            json!({ "action": "observe", "background": "maybe" }),
            json!({ "action": "observe", "background": "" }),
            json!({ "action": "wait", "duration": "soon" }),
            json!({ "action": "wait", "duration": "NaN" }),
            json!({ "action": "wait", "duration": "inf" }),
            json!({ "action": "wait", "duration": [1] }),
            json!({ "action": "wait", "timeout_ms": "-5" }),
            json!({ "action": "wait", "timeout_ms": "2.5" }),
            json!({ "action": "scroll", "scroll_amount": "-1" }),
            json!({ "action": "scroll", "scroll_amount": "99999999999" }),
        ];
        for bad in bads {
            assert!(serde_json::from_value::<ComputerInput>(bad.clone()).is_err(), "{bad} must still be refused");
        }
    }

    #[cfg(unix)]
    #[test]
    fn repeated_observe_omits_only_a_picture_already_shown_from_the_same_target() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (road, changed) = fake_shim(dir.path());
        let ctx = crate::tests::tool_context_outside_cwd_windows();
        let observe = |app: &str| -> Value {
            serde_json::from_str(&run_computer(&json!({ "action": "observe", "app": app }), &ctx, &road).unwrap()).unwrap()
        };
        observe("Mail");
        std::fs::write(&changed, "[]").unwrap();
        let same = observe("Mail");
        assert_eq!(same["result"]["screenshot"]["staged"], false);
        assert_eq!(staged(&ctx), 1, "same picture costs no second image block");
        observe("Notes");
        assert_eq!(staged(&ctx), 2, "another target with identical geometry must be shown");
        observe("Mail");
        assert_eq!(staged(&ctx), 3, "returning to the previous target must also be shown");
        std::fs::write(&changed, r#"[{"x":10,"y":20,"width":4,"height":8}]"#).unwrap();
        observe("Mail");
        assert_eq!(staged(&ctx), 4, "changed pixels still reach the model");
    }

    #[cfg(unix)]
    #[test]
    fn an_unstaged_picture_does_not_replace_the_models_coordinate_frame() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (road, _) = fake_shim(dir.path());
        let ctx = crate::tests::tool_context_outside_cwd_windows();
        let previous = half((400.0, 200.0));
        ctx.set_computer_frame(previous);
        std::fs::remove_file(dir.path().join("shot.png")).unwrap();
        for action in ["observe", "screenshot"] {
            run_computer(&json!({ "action": action }), &ctx, &road).unwrap();
            assert_eq!(staged(&ctx), 0);
            assert_eq!(ctx.computer_frame(), Some(previous), "the unseen frame must not move the next click");
        }
        run_computer(&json!({ "action": "left_click", "coordinate": [0, 0] }), &ctx, &road).unwrap();
        assert_eq!(ctx.computer_frame(), Some(previous), "an automatic look has the same rule");
    }

    #[cfg(unix)]
    #[test]
    fn a_first_picture_that_cannot_be_staged_does_not_send_coordinate_input() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (road, _) = fake_shim(dir.path());
        std::fs::remove_file(dir.path().join("shot.png")).unwrap();
        let ctx = crate::tests::tool_context_outside_cwd_windows();
        for input in [json!({ "action": "left_click", "coordinate": [0, 0] }),
            json!({ "action": "batch", "steps": [{ "action": "left_click", "coordinate": [0, 0] }] })] {
            assert!(run_computer(&input, &ctx, &road).unwrap_err().to_string().contains("could not be shown"));
        }
        assert_eq!(calls(dir.path()), ["observe", "observe"], "neither a click nor a batch was sent");
        assert_eq!(ctx.computer_frame(), None);
    }

    #[test]
    fn an_unchanged_look_is_shown_after_window_marks_or_frame_change_and_after_staging_failure() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ctx = crate::tests::tool_context_outside_cwd_windows();
        let path = dir.path().join("shot.png");
        let show = |window, mark: &str, origin, valid, diff| {
            if valid {
                std::fs::write(&path, TINY_PNG).unwrap();
            }
            let mut answer = json!({ "ok": true, "result": {
                "screenshot": { "path": path, "scale": 0.5 }, "origin": { "x": origin, "y": 0 },
                "marks": { "lookId": mark }, "changed": []
            } });
            present_observation(&mut answer, &ctx, Some("Mail"), Some(window), diff).is_some()
        };
        assert!(show(1, "look-1", 0, true, true), "first picture even if the provider says unchanged");
        assert!(!show(1, "look-1", 0, true, true));
        assert!(show(2, "look-1", 0, true, true), "different window");
        assert!(show(2, "look-2", 0, true, true), "different numbering");
        assert!(show(2, "look-2", 50, true, true), "moved frame");
        assert!(!show(2, "look-3", 100, false, true), "image staging failed");
        assert_eq!(ctx.computer_frame(), Some(half((50.0, 0.0))));
        assert!(show(2, "look-3", 100, true, true), "the next unchanged look must repair the failed delivery");
        assert!(show(2, "look-3", 100, true, false), "explicit non-diff screenshot always shows");
    }

    #[cfg(unix)]
    #[test]
    fn a_zoom_does_not_hide_the_next_unchanged_full_observation() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (road, changed) = fake_shim(dir.path());
        let script = std::fs::read_to_string(&road.program).unwrap().replace("screenshot|observe)", "screenshot|observe|zoom)");
        std::fs::write(&road.program, script).unwrap();
        let ctx = crate::tests::tool_context_outside_cwd_windows();
        run_computer(&json!({ "action": "observe" }), &ctx, &road).unwrap();
        std::fs::write(&changed, "[]").unwrap();
        run_computer(&json!({ "action": "zoom", "region": [0, 0, 10, 10] }), &ctx, &road).unwrap();
        run_computer(&json!({ "action": "observe" }), &ctx, &road).unwrap();
        assert_eq!(staged(&ctx), 3);
        assert_eq!(ctx.computer_frame(), Some(half((0.0, 0.0))));
    }

    #[cfg(unix)]
    #[test]
    fn continuous_motion_can_be_sampled_without_waiting_for_quiet() {
        for settle in [None, Some(true), Some(false)] {
            let dir = tempfile::tempdir().expect("tempdir");
            let (road, changed) = fake_shim(dir.path());
            if settle == Some(false) {
                std::fs::write(changed, "[]").unwrap();
            }
            let ctx = crate::tests::tool_context_outside_cwd_windows();
            ctx.set_computer_frame(half((0.0, 0.0)));
            let mut fields = json!({ "action": "left_click", "coordinate": [10, 10] });
            if let Some(settle) = settle {
                fields["settle"] = settle.into();
            }
            let answer: Value = serde_json::from_str(&run_computer(&fields, &ctx, &road).expect("action and look")).unwrap();
            let said = answer["looked_after"]["screen"]["said"].as_str().expect("look argv");
            assert_eq!(said.split_whitespace().any(|word| word == "--settle"), settle.unwrap_or(true));
            assert_eq!(calls(dir.path()), ["mouse-click", "observe"]);
            assert_eq!(staged(&ctx), 1, "immediate mode still shows a picture");
            assert!(answer["result"]["said"].as_str().unwrap().starts_with("mouse-click --x 20 --y 20"), "the same coordinate conversion");
            if settle == Some(false) {
                assert_eq!(answer["looked_after"]["waited_for_settle"], false);
                assert!(answer["looked_after"]["changed"].is_null(), "an immediate sample does not judge the action's result");
                assert!(!said.split_whitespace().any(|word| word == LOOK_DIFF_FLAG));
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn an_immediate_batch_still_stops_on_a_refused_action() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (road, _) = fake_shim(dir.path());
        let ctx = crate::tests::tool_context_outside_cwd_windows();
        let steps = json!([{ "action": "key", "text": "a" }, { "action": "key", "text": "b" }]);
        std::fs::write(dir.path().join("batch"), json!({
            "ok": false, "error": { "code": "stopped", "message": "the person stopped" },
            "result": { "ran": 1, "of": 2, "refusedAt": 2, "steps": [
                { "n": 1, "verb": "key", "ok": true, "result": {} },
                { "n": 2, "verb": "key", "ok": false, "error": { "code": "stopped" } }
            ] }
        }).to_string()).unwrap();
        let answer: Value = serde_json::from_str(&run_computer(&json!({
            "action": "batch", "steps": steps, "settle": false
        }), &ctx, &road).expect("partial batch report")).unwrap();
        assert_eq!(answer["ok"], false);
        assert_eq!(answer["ran"], 1);
        assert_eq!(answer["refused"]["code"], "stopped");
        assert_eq!(calls(dir.path()), ["batch", "observe"]);
        assert!(!answer["looked_after"]["screen"]["said"].as_str().unwrap().contains("--settle"));
        assert!(batch_argv(&[input(json!({ "action": "key", "text": "a", "settle": false }))], ShotFrame::UNIT).is_err());
    }

    /// The road end to end: a look is staged and teaches the frame, an act is
    /// moved in points and answered in pixels, the look after it is shown
    /// only when something changed, and a refusal is the window's own words.
    #[cfg(unix)]
    #[test]
    fn the_shim_road_is_answered_in_the_pixels_the_model_sees() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (road, changed) = fake_shim(dir.path());
        let ctx = crate::tests::tool_context_outside_cwd_windows();
        let run = |fields: Value| -> Value {
            serde_json::from_str(&run_computer(&fields, &ctx, &road).expect("an answer")).expect("json")
        };

        let looked = run(json!({ "action": "screenshot" }));
        assert_eq!(looked["result"]["verb"], "screenshot");
        assert_eq!(looked["screenshot"]["staged"], true, "{looked}");
        assert!(looked["looked_after"].is_null(), "a look is not followed by a look");
        assert_eq!(ctx.computer_frame(), ShotFrame::new((0.0, 0.0), 0.5), "the look taught the frame");
        assert_eq!(looked["result"]["cursor"], json!({ "x": 100.0, "y": 40.0 }), "answers speak pixels");
        assert_eq!(staged(&ctx), 1, "one image block");

        assert!(looked["result"].get("origin").is_none() && looked["result"]["screenshot"].get("scale").is_none(),
            "the frame's own words are the tool's, not the model's: {looked}");
        assert!(
            looked["result"]["said"].as_str().unwrap().contains("--viewer zo-"),
            "a look is this model's own in the window's last-look table: {looked}"
        );
        let acted = run(json!({ "action": "left_click", "coordinate": [100, 40] }));
        assert!(
            acted["result"]["said"].as_str().unwrap().starts_with("mouse-click --x 200 --y 80 "),
            "the hand moves in points: {acted}"
        );
        assert_eq!(acted["looked_after"]["changed"], Value::Null, "nothing to compare with: shown, not judged");
        assert_eq!(acted["looked_after"]["staged"], true, "{acted}");
        assert_eq!(staged(&ctx), 2, "the look after the act");

        std::fs::write(&changed, "[]").unwrap();
        let looks_before = calls(dir.path()).iter().filter(|verb| *verb == "observe").count();
        let idle = run(json!({ "action": "left_click", "coordinate": [1, 1] }));
        assert_eq!(idle["looked_after"]["changed"], false, "{idle}");
        assert_eq!(idle["looked_after"]["staged"], false);
        assert!(idle["looked_after"]["screen"]["screenshot"].get("path").is_none(), "no name of a file that is gone");
        assert!(
            idle["looked_after"]["screen"]["said"].as_str().unwrap().starts_with("observe --diff --settle "),
            "the window gives the act's paint the settle, and the tool looks once: {idle}"
        );
        assert_eq!(calls(dir.path()).iter().filter(|verb| *verb == "observe").count(), looks_before + 1);
        assert_eq!(staged(&ctx), 2, "nothing changed: no second copy of the same picture");

        std::fs::write(&changed, r#"[{"x":10,"y":20,"width":4,"height":8}]"#).unwrap();
        let moved = run(json!({ "action": "key", "text": "Tab" }));
        assert_eq!(moved["looked_after"]["screen"]["changed"][0], json!({ "x": 5.0, "y": 10.0, "width": 2.0, "height": 4.0 }));
        assert_eq!(staged(&ctx), 3);

        // A process that has shown its model no picture shows one first.
        let fresh = crate::tests::tool_context_outside_cwd_windows();
        let first: Value = serde_json::from_str(
            &run_computer(&json!({ "action": "left_click", "coordinate": [100, 40] }), &fresh, &road).expect("an answer"),
        )
        .unwrap();
        assert_eq!(first["first_look"]["staged"], true, "{first}");
        assert!(first["result"]["said"].as_str().unwrap().starts_with("mouse-click --x 200 --y 80 "), "{first}");
        // A process whose first action answers places is shown a picture
        // first too, so the place it answers is in the pixels it will click.
        let finder = crate::tests::tool_context_outside_cwd_windows();
        let found: Value = serde_json::from_str(
            &run_computer(&json!({ "action": "find", "ocr": true, "text": "Save" }), &finder, &road).expect("an answer"),
        )
        .unwrap();
        assert_eq!(found["first_look"]["staged"], true, "{found}");
        assert_eq!(found["result"]["cursor"], json!({ "x": 100.0, "y": 40.0 }), "{found}");
        // A screen that cannot be seen: the look is refused, the hand still moves, in points.
        std::fs::create_dir_all(dir.path().join("blind")).unwrap();
        let (blind_road, _) = fake_shim(&dir.path().join("blind"));
        std::fs::write(
            &blind_road.program,
            "#!/bin/sh\ncase \"$1\" in observe) printf '%s\\n' '{\"ok\":false,\"error\":{\"code\":\"permission_denied\",\"message\":\"no screen recording\"}}' >&2; exit 1;; esac\nprintf '%s\\n' '{\"ok\":true,\"result\":{\"said\":\"'\"$*\"'\"}}'\n",
        )
        .unwrap();
        let blind_ctx = crate::tests::tool_context_outside_cwd_windows();
        let blind: Value = serde_json::from_str(
            &run_computer(&json!({ "action": "left_click", "coordinate": [100, 40] }), &blind_ctx, &blind_road)
                .expect("the click still answers"),
        )
        .unwrap();
        assert_eq!(blind["first_look"]["blind"], true, "{blind}");
        assert!(blind["result"]["said"].as_str().unwrap().starts_with("mouse-click --x 100 --y 40 "), "{blind}");
        assert_eq!(blind_ctx.computer_frame(), Some(ShotFrame::UNIT), "blind once: points both ways, no look again");
        // Any other refusal of the first look stops the act: the model's
        // pixels are not guessed into points.
        std::fs::write(
            &blind_road.program,
            "#!/bin/sh\ncase \"$1\" in observe) printf '%s\\n' '{\"ok\":false,\"error\":{\"code\":\"action_timeout\",\"message\":\"slow\"}}' >&2; exit 1;; esac\nprintf '%s\\n' '{\"ok\":true,\"result\":{}}'\n",
        )
        .unwrap();
        let stopped = run_computer(&json!({ "action": "left_click", "coordinate": [1, 1] }), &crate::tests::tool_context_outside_cwd_windows(), &blind_road)
            .expect_err("no frame, no act");
        assert!(stopped.to_string().contains("action_timeout"), "{stopped}");
        let untouched: Value = serde_json::from_str(
            &run_computer(&json!({ "action": "type", "text": "x" }), &crate::tests::tool_context_outside_cwd_windows(), &road).expect("an answer"),
        )
        .unwrap();
        assert!(untouched["first_look"].is_null(), "no position, nothing to learn");

        // A refusal is an error with the window's code and message.
        std::fs::write(&road.program, "#!/bin/sh\nprintf '%s\\n' '{\"ok\":false,\"error\":{\"code\":\"stopped\",\"message\":\"the operator is stopped\"}}' >&2\nexit 1\n").unwrap();
        let refused = run_computer(&json!({ "action": "type", "text": "x" }), &ctx, &road).expect_err("a refusal");
        assert!(refused.to_string().contains("stopped"), "{refused}");

        // No shim on PATH: an honest refusal, not a desktop.
        let missing = ComputerRoad { program: dir.path().join("nothing-here") };
        let error = run_computer(&json!({ "action": "screenshot" }), &ctx, &missing).expect_err("no road");
        assert!(error.to_string().contains("could not run"), "{error}");
    }

    #[cfg(unix)]
    pub(crate) fn calls(dir: &Path) -> Vec<String> {
        std::fs::read_to_string(dir.join("calls")).unwrap_or_default().lines().map(str::to_string).collect()
    }

    /// The words of the last handoff the fake shim was called with.
    #[cfg(unix)]
    fn handoff_words(dir: &Path) -> Vec<String> {
        std::fs::read_to_string(dir.join("handoff-argv")).unwrap_or_default().lines().map(str::to_string).collect()
    }

    /// Each step of a batch is spelled exactly as the same action alone, in
    /// the same frame, and the core accepts the whole.
    #[test]
    fn a_batch_spells_every_step_as_a_single_action_in_the_same_frame() {
        let frame = half((-100.0, 0.0));
        let steps = vec![
            input(json!({ "action": "left_click", "coordinate": [100, 40] })),
            input(json!({ "action": "type", "text": "hi" })),
            input(json!({ "action": "key", "text": "Return", "confirming": "payment" })),
        ];
        let argv = batch_argv(&steps, frame).expect("a batch");
        assert_eq!(argv[0], ComputerMethod::Batch.verb_name());
        assert_eq!(argv[1], format!("--{BATCH_COMMANDS_FLAG}"));
        let commands: Vec<Vec<String>> = serde_json::from_str(&argv[2]).unwrap();
        for (command, step) in commands.iter().zip(&steps) {
            assert_eq!(command, &argv_for(&in_points(step.clone(), frame).unwrap()).unwrap());
        }
        assert_eq!(commands[0][..5], ["mouse-click", "--x", "100", "--y", "80"]);
        assert!(zerocode_core::computer_use::parse_command(&argv).is_ok());
        assert!(batch_argv(&[input(json!({ "action": "batch", "steps": [] }))], frame).is_err(), "no nesting");
    }

    /// Five hand steps: singly, ten trips down the shim and five pictures;
    /// as one batch, two trips (the batch and one look) and one picture.
    #[cfg(unix)]
    #[test]
    fn a_batch_is_one_trip_and_one_look() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (road, _) = fake_shim(dir.path());
        let five = json!([
            { "action": "left_click", "coordinate": [10, 10] },
            { "action": "type", "text": "hello" },
            { "action": "key", "text": "Tab" },
            { "action": "type", "text": "world" },
            { "action": "key", "text": "Return" },
        ]);
        let singly = crate::tests::tool_context_outside_cwd_windows();
        singly.set_computer_frame(half((0.0, 0.0)));
        for step in five.as_array().unwrap() {
            run_computer(step, &singly, &road).expect("an answer");
        }
        assert_eq!(calls(dir.path()).len(), 10, "{:?}", calls(dir.path()));
        assert_eq!(staged(&singly), 5);

        std::fs::remove_file(dir.path().join("calls")).unwrap();
        let steps: Vec<Value> = (1..=5).map(|n| json!({ "n": n, "verb": "x", "ok": true, "ms": 1, "result": { "cursor": { "x": 20, "y": 20 } } })).collect();
        std::fs::write(dir.path().join("batch"), json!({ "ok": true, "result": { "ran": 5, "of": 5, "steps": steps } }).to_string()).unwrap();
        let batched = crate::tests::tool_context_outside_cwd_windows();
        batched.set_computer_frame(half((0.0, 0.0)));
        let answer: Value = serde_json::from_str(&run_computer(&json!({ "action": "batch", "steps": five }), &batched, &road).expect("an answer")).unwrap();
        assert_eq!(calls(dir.path()), ["batch", "observe"], "one batch, one look");
        assert_eq!(staged(&batched), 1);
        assert_eq!(answer["ran"], 5);
        assert_eq!(answer["steps"][2]["verb"], "key", "each step under the model's own action name");
        assert_eq!(answer["steps"][0]["result"]["cursor"], json!({ "x": 10.0, "y": 10.0 }), "in the picture's pixels");
    }

    /// A batch sent before this model has seen a picture looks first, and its
    /// steps are placed in the frame that look learned — not in a unit frame
    /// that would land every click at half its coordinates on a 2x screen.
    #[cfg(unix)]
    #[test]
    fn a_batch_before_any_picture_is_placed_in_the_frame_its_first_look_learned() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (road, _) = fake_shim(dir.path());
        let ran: Vec<Value> = (1..=2).map(|n| json!({ "n": n, "verb": "x", "ok": true, "ms": 1, "result": {} })).collect();
        std::fs::write(dir.path().join("batch"), json!({ "ok": true, "result": { "ran": 2, "of": 2, "steps": ran } }).to_string()).unwrap();
        let fresh = crate::tests::tool_context_outside_cwd_windows();
        let steps = json!([{ "action": "left_click", "coordinate": [10, 10] }, { "action": "type", "text": "hi" }]);
        run_computer(&json!({ "action": "batch", "steps": steps }), &fresh, &road).expect("an answer");
        assert_eq!(calls(dir.path()), ["observe", "batch", "observe"], "a look, the batch, a look");
        let commands: Vec<Vec<String>> =
            serde_json::from_str(&std::fs::read_to_string(dir.path().join("batch-commands")).unwrap()).unwrap();
        assert_eq!(
            commands[0][..5],
            ["mouse-click", "--x", "20", "--y", "20"],
            "the fake's picture is half the screen's points: pixel 10 is point 20"
        );
    }

    /// A batch refused midway says what ran and still looks; one refused
    /// before any step ran is the window's refusal, like a single action's.
    #[cfg(unix)]
    #[test]
    fn a_batch_refused_midway_says_what_ran_and_still_looks() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (road, _) = fake_shim(dir.path());
        let ctx = crate::tests::tool_context_outside_cwd_windows();
        ctx.set_computer_frame(half((0.0, 0.0)));
        let three = json!([{ "action": "key", "text": "a" }, { "action": "key", "text": "b" }, { "action": "left_click", "coordinate": [1, 1] }]);
        std::fs::write(dir.path().join("batch"), json!({
            "ok": false,
            "error": { "code": "confirmation_refused", "message": "step 3 of 3 (mouse-click): no" },
            "result": { "ran": 2, "of": 3, "refusedAt": 3, "steps": [
                { "n": 1, "verb": "key", "ok": true, "ms": 1, "result": {} },
                { "n": 2, "verb": "key", "ok": true, "ms": 1, "result": {} },
                { "n": 3, "verb": "mouse-click", "ok": false, "ms": 1, "error": { "code": "confirmation_refused", "message": "no" } }
            ] }
        }).to_string()).unwrap();
        let answer: Value = serde_json::from_str(&run_computer(&json!({ "action": "batch", "steps": three }), &ctx, &road).expect("an answer")).unwrap();
        assert_eq!(answer["ok"], false);
        assert_eq!(answer["ran"], 2);
        assert_eq!(answer["refused"]["step"], 3);
        assert_eq!(answer["refused"]["action"], "left_click");
        assert_eq!(answer["refused"]["code"], "confirmation_refused");
        assert!(answer["looked_after"].is_object(), "what ran changed the screen: {answer}");

        std::fs::write(dir.path().join("batch"), r#"{"ok":false,"error":{"code":"stopped","message":"the operator is stopped"},"result":{"ran":0,"of":3,"steps":[]}}"#).unwrap();
        let error = run_computer(&json!({ "action": "batch", "steps": three }), &ctx, &road).expect_err("refused whole");
        assert!(error.to_string().contains("stopped: the operator is stopped"), "{error}");
    }

    /// A batch holds only the hand's vocabulary, and a bad one costs no look.
    #[cfg(unix)]
    #[test]
    fn a_batch_holds_only_the_hands_vocabulary_and_pays_no_look_for_a_bad_one() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (road, _) = fake_shim(dir.path());
        for steps in [
            json!([{ "action": "screenshot" }]),
            json!([{ "action": "find", "ocr": true, "text": "x" }]),
            json!([{ "action": "run", "program": "/bin/echo" }]),
            json!([{ "action": "wait_for", "window": "Save" }]),
            json!((0..=COMPUTER_BATCH_MAX_STEPS).map(|_| json!({ "action": "wait", "duration": 0.01 })).collect::<Vec<_>>()),
        ] {
            let error = run_computer(&json!({ "action": "batch", "steps": steps }), &crate::tests::tool_context_outside_cwd_windows(), &road).expect_err("refused");
            assert!(matches!(error, ToolError::InvalidInput(_)), "{error}");
        }
        assert!(calls(dir.path()).is_empty(), "no trip, no look: {:?}", calls(dir.path()));
        let stray = run_computer(&json!({ "action": "key", "text": "a", "steps": [] }), &crate::tests::tool_context_outside_cwd_windows(), &road).expect_err("stray steps");
        assert!(stray.to_string().contains("belongs to `batch`"));

        let spec = tool_specs().pop().expect("one spec");
        let steps = &spec.input_schema["properties"]["steps"];
        assert_eq!(steps["maxItems"], COMPUTER_BATCH_MAX_STEPS);
        let items = &steps["items"]["properties"];
        let offered = |field: &str| -> Vec<String> {
            items[field]["enum"].as_array().unwrap().iter().map(|word| word.as_str().unwrap().to_string()).collect()
        };
        let batchable = action_names(|action| methods(action).any(ComputerMethod::batches));
        assert_eq!(offered("action"), batchable, "a step offers exactly the actions the core lets a batch hold");
        assert!(!batchable.contains(&BATCH_ACTION) && !batchable.contains(&"screenshot") && batchable.contains(&"wait_for"));
        assert_eq!(offered("kind"), ["focus", "move", "resize"], "a window step is one the core batches");
        assert_eq!(
            spec.input_schema["properties"]["kind"]["enum"].as_array().map(Vec::len),
            Some(ComputerMethod::ALL.iter().filter(|method| method.window_action().is_some()).count())
        );
        assert!(items.as_object().unwrap().len() > 1, "a step carries real properties");
    }

    /// The window's answer to a walk, as `run_goal` writes it: the rows it
    /// walked, the presses, whether it got there, and the condition's standing
    /// before the first look when one was given.
    fn walk_answer(mode: &str, pressed: u64, reached: bool, until_before: Option<bool>, rows: &[Value]) -> Value {
        let mut said = json!({
            "goal": "g",
            (walk_words::MODE): mode,
            (walk_words::PRESSED): pressed,
            (walk_words::REACHED): reached,
            (walk_words::ROWS): rows,
        });
        if let Some(before) = until_before {
            said[walk_words::UNTIL_BEFORE] = json!(before);
        }
        json!({ "ok": true, "result": said })
    }

    fn walk_row(attempt: u64, more: Value) -> Value {
        let mut row = json!({ (walk_words::ATTEMPT): attempt, (walk_words::OUTCOME): zerocode_core::jev::summary::ANSWERED });
        if let (Some(row), Value::Object(more)) = (row.as_object_mut(), more) {
            row.extend(more);
        }
        row
    }

    /// The tool's walk is the CLI's walk: the same goal, screen, condition,
    /// budget and switches read by the core's own parser into the same
    /// command, so what the window walks — the mode it reads, the presses it
    /// may spend, where it stops — cannot differ by who asked. Its fields
    /// are the walk's own; a batch's `steps` stays the batch's.
    #[test]
    fn a_walk_is_the_clis_walk_read_by_the_same_parser() {
        for action in ["left_click", "key", "batch"] {
            let input: ComputerInput = serde_json::from_value(json!({ "action": action, "background": true })).unwrap();
            assert!(argv_for(&input).is_err(), "background must not disappear from {action}");
        }
        let parse = |words: &[&str]| {
            zerocode_core::computer_use::parse_command(&words.iter().map(|word| (*word).to_string()).collect::<Vec<_>>())
                .expect("the CLI's walk")
        };
        let cases: Vec<(Value, Vec<&str>)> = vec![
            (json!({ "action": "walk", "goal": "g", "app": "Fixture", "background": true }),
                vec!["walk", "--app", "Fixture", "--goal", "g", "--background", "--json"]),
            (json!({ "action": "walk", "goal": "g", "pane": "browser-13", "until": "u", "max_steps": 6, "overlap": true }),
                vec!["walk", "--pane", "browser-13", "--goal", "g", "--steps", "6", "--until", "u", "--overlap", "--json"]),
            (json!({ "action": "walk", "goal": "g", "app": "Calculator", "rescue": true, "replay": true }),
                vec!["walk", "--app", "Calculator", "--goal", "g", "--rescue", "--replay", "--json"]),
            (json!({ "action": "walk", "goal": "g", "platform": "android", "device": "emulator-5554" }),
                vec!["walk", "--platform", "android", "--device", "emulator-5554", "--goal", "g", "--json"]),
        ];
        for (fields, words) in cases {
            let tool = zerocode_core::computer_use::parse_command(&argv_for(&input(fields.clone())).unwrap()).expect("the tool's walk");
            let cli = parse(&words);
            assert_eq!((tool.method, &tool.params, tool.json), (cli.method, &cli.params, cli.json), "{fields}");
            assert_eq!(walk_steps(&tool.params), walk_steps(&cli.params), "the same budget");
        }
        // What a walk may not be is the core's to say, read before anything moves.
        let ctx = crate::tests::tool_context_outside_cwd_windows();
        let road = ComputerRoad { program: PathBuf::from("/nonexistent/zerocode-computer") };
        for (fields, why) in [
            (json!({ "action": "walk", "pane": "browser-1" }), "a goal"),
            (json!({ "action": "walk", "goal": "   ", "pane": "browser-1" }), "a goal that says something"),
            (json!({ "action": "walk", "goal": "g" }), "one screen"),
            (json!({ "action": "walk", "goal": "g", "app": "A", "pane": "browser-1" }), "only one screen"),
            (json!({ "action": "walk", "goal": "g", "device": "d" }), "a device with its platform"),
            (json!({ "action": "walk", "goal": "g", "app": "A", "max_steps": WALK_STEPS_MAX + 1 }), "the core's ceiling"),
            (json!({ "action": "walk", "goal": "g", "app": "A", "until": "" }), "a condition that says something"),
            (json!({ "action": "walk", "goal": "x".repeat(GOAL_CHAR_CAP + 1), "app": "A" }), "a sentence, not a plan"),
            (json!({ "action": "walk", "goal": "g", "app": "A", "steps": [{ "action": "wait", "duration": 1 }] }), "a batch's steps"),
        ] {
            let refused = run_computer(&fields, &ctx, &road).expect_err(why);
            assert!(matches!(refused, ToolError::InvalidInput(_)), "{why}: {refused}");
        }
        let schema = &tool_specs().pop().expect("one spec").input_schema["properties"];
        assert_eq!(schema["max_steps"]["maximum"], json!(WALK_STEPS_MAX));
        assert_eq!(schema["goal"]["maxLength"], json!(GOAL_CHAR_CAP));
        assert_eq!(schema["background"]["type"], "boolean");
        assert!(schema["steps"]["items"]["properties"].get("goal").is_none(), "a walk is never a batch step");
    }

    /// Every status from one fixed answer, and none from a press, an `ok` or
    /// the judgment's own `done` alone.
    #[test]
    fn a_walks_answer_reads_into_one_of_five_statuses() {
        let mut interrupted = walk_answer("on", 1, true, Some(false), &[walk_row(1, json!({ "recheck": true }))]);
        interrupted["result"]["background"] = json!({ "requested": true, "interrupted": true });
        let stopped = read_walk(&interrupted, 6);
        assert_eq!(stopped.status, WalkStatus::Stopped);
        assert_eq!(stopped.reason.as_deref(), Some("background_interrupted"));
        let on = JevMode::On.key();
        let barred = zerocode_core::jev::summary::BARRED.canonical;
        let pressed = |attempt: u64, recheck: Option<bool>| {
            let mut row = walk_row(attempt, json!({ (walk_words::PRESSED): true }));
            if let Some(recheck) = recheck {
                row[walk_words::RECHECK] = json!(recheck);
            }
            row
        };
        let cases: Vec<(&str, Value, WalkStatus, Option<&str>)> = vec![
            ("until read after a press, absent before", walk_answer(on, 2, true, Some(false), &[pressed(1, Some(false)), pressed(2, Some(true))]),
                WalkStatus::Verified, None),
            ("until absent at a check between presses", walk_answer(on, 2, true, None, &[pressed(1, Some(false)), pressed(2, Some(true))]),
                WalkStatus::Verified, None),
            ("until held before and never read absent", walk_answer(on, 1, true, Some(true), &[pressed(1, Some(true))]),
                WalkStatus::NeedsVerification, Some(UNTIL_HELD_BEFORE)),
            ("the judgment's own done", walk_answer(on, 1, true, None, &[pressed(1, None), walk_row(2, json!({ (walk_words::CHOSEN): "done", (walk_words::REACHED_BY): "judgment" }))]),
                WalkStatus::NeedsVerification, None),
            ("recording only", walk_answer(on, 0, false, None, &[walk_row(1, json!({ (walk_words::PRESSED): false, (walk_words::REASON): "seat_recording" }))]),
                WalkStatus::NeedsFallback, Some("seat_recording")),
            ("unsure", walk_answer(on, 0, false, None, &[walk_row(1, json!({ (walk_words::PRESSED): false, (barred): walk_words::LOW_CONFIDENCE }))]),
                WalkStatus::NeedsFallback, Some(walk_words::LOW_CONFIDENCE)),
            ("unanswered", walk_answer(on, 0, false, None, &[json!({ (walk_words::ATTEMPT): 1, (walk_words::OUTCOME): "unauthorized" })]),
                WalkStatus::NeedsFallback, Some("unauthorized")),
            ("a dead end", walk_answer(on, 1, false, None, &[pressed(1, None), walk_row(2, json!({ (walk_words::CHOSEN): GIVE_UP }))]),
                WalkStatus::NeedsFallback, Some(GIVE_UP)),
            ("a guard's refusal", walk_answer(on, 0, false, None, &[walk_row(1, json!({ (walk_words::PRESSED): false, (barred): "injected", (walk_words::REASON): "injected" }))]),
                WalkStatus::Stopped, Some("injected")),
            ("the clock spent", walk_answer(on, 1, false, None, &[pressed(1, None), json!({ (walk_words::ATTEMPT): 2, (walk_words::OUTCOME): "barred", (barred): "no_budget" })]),
                WalkStatus::Stopped, Some("no_budget")),
            ("every step spent", walk_answer(on, 2, false, Some(false), &[pressed(1, Some(false)), pressed(2, Some(false))]),
                WalkStatus::Stopped, Some(STEPS_SPENT)),
            ("the person's stop", json!({ "ok": false, "error": { "code": error_code::STOPPED, "message": "stopped" } }),
                WalkStatus::Stopped, Some(error_code::STOPPED)),
            ("the screen would not move", walk_answer(on, 2, false, None, &[pressed(1, None), pressed(2, None), json!({ (walk_words::ATTEMPT): 3, (walk_words::OUTCOME): walk_words::STUCK, (walk_words::PRESSED): 2 })]),
                WalkStatus::Failed, Some(walk_words::STUCK)),
            ("a press the hand could not make", walk_answer(on, 0, false, None, &[walk_row(1, json!({ (walk_words::PRESSED): false }))]),
                WalkStatus::Failed, Some(PRESS_FAILED)),
            ("no look came back", walk_answer(on, 0, false, None, &[json!({ (walk_words::ATTEMPT): 1, (walk_words::OUTCOME): walk_words::NO_LOOK })]),
                WalkStatus::Failed, Some(walk_words::NO_LOOK)),
            ("a refusal that is no stop", json!({ "ok": false, "error": { "code": error_code::INVALID_ARGUMENT, "message": "x" } }),
                WalkStatus::Failed, Some(error_code::INVALID_ARGUMENT)),
        ];
        let mut seen = std::collections::BTreeSet::new();
        for (case, answer, status, reason) in cases {
            let read = read_walk(&answer, 6);
            assert_eq!((read.status, read.reason.as_deref()), (status, reason), "{case}");
            seen.insert(status.word());
        }
        assert_eq!(seen.len(), WalkStatus::ALL.len(), "every status has a case");
        // The facts beside the status: where it ended, what the budget still
        // holds, the last look the judgment answered on, and how it got there.
        let read = read_walk(&walk_answer(on, 2, true, Some(false), &[pressed(1, Some(false)), pressed(2, Some(true))]), 6);
        assert_eq!((read.pressed, read.step, read.steps_left, read.last_observed_step), (2, Some(2), 4, Some(2)));
        assert_eq!(read.reached_by.as_deref(), Some(REACHED_BY_UNTIL));
        // `ok`, presses and a `reached` with nothing to show for it are not
        // verified.
        let bare = read_walk(&walk_answer(on, 3, true, None, &[pressed(1, None), pressed(2, None), pressed(3, None)]), 6);
        assert_eq!(bare.status, WalkStatus::NeedsVerification);
    }

    /// A walk that could not start — the seat off, or on with no judge —
    /// answers so the first time, from one call: no second walk, no click, no
    /// look, and nothing written (the tool writes no setting at all).
    #[cfg(unix)]
    #[test]
    fn a_walk_that_cannot_apply_says_why_the_first_time() {
        for (mode, reason) in [(JevMode::Off.key(), SEAT_OFF), (JevMode::On.key(), NO_JUDGE), (JevMode::Auto.key(), NO_JUDGE)] {
            let dir = tempfile::tempdir().expect("tempdir");
            let (road, _) = fake_shim(dir.path());
            std::fs::write(dir.path().join("walk"), walk_answer(mode, 0, false, None, &[]).to_string()).unwrap();
            let ctx = crate::tests::tool_context_outside_cwd_windows();
            let said: Value = serde_json::from_str(
                &run_computer(&json!({ "action": "walk", "goal": "g", "app": "Calculator" }), &ctx, &road).unwrap(),
            )
            .unwrap();
            assert_eq!((said["status"].as_str(), said["reason"].as_str()), (Some(WalkStatus::NeedsFallback.word()), Some(reason)), "{mode}");
            assert_eq!(said["result"][walk_words::MODE], json!(mode), "the seat's standing as the window read it");
            assert_eq!(calls(dir.path()), ["walk"], "one call, no look, no press: {mode}");
            assert_eq!(staged(&ctx), 0);
        }
    }

    /// A walk carries its own looks: none before it (it speaks in no pixels),
    /// none after a walk that ended on a screen it read — and one after a walk
    /// that pressed at an app and ended on a screen nobody read: after
    /// `stuck`, the ladder's own look (t-70459).
    #[cfg(unix)]
    #[test]
    fn a_walk_pays_no_look_it_does_not_need() {
        let on = JevMode::On.key();
        let pressed = |attempt: u64, recheck: bool| walk_row(attempt, json!({ (walk_words::PRESSED): true, (walk_words::RECHECK): recheck }));
        let stuck = json!({ (walk_words::ATTEMPT): 2, (walk_words::OUTCOME): walk_words::STUCK, (walk_words::PRESSED): 1 });
        for (screen, answer, looks) in [
            (json!({ "pane": "browser-13" }), walk_answer(on, 1, true, Some(false), &[pressed(1, true)]), false),
            (json!({ "app": "Calculator" }), walk_answer(on, 1, true, Some(false), &[pressed(1, true)]), false),
            (json!({ "app": "Calculator" }), walk_answer(on, 1, false, None, &[pressed(1, false), stuck.clone()]), true),
            (json!({ "pane": "browser-13" }), walk_answer(on, 1, false, None, &[pressed(1, false), stuck.clone()]), false),
        ] {
            let dir = tempfile::tempdir().expect("tempdir");
            let (road, _) = fake_shim(dir.path());
            std::fs::write(dir.path().join("walk"), answer.to_string()).unwrap();
            let ctx = crate::tests::tool_context_outside_cwd_windows();
            let mut fields = json!({ "action": "walk", "goal": "도움말에서 설치 안내를 연다", "until": "설치 안내", "max_steps": 6 });
            for (key, value) in screen.as_object().unwrap() {
                fields[key] = value.clone();
            }
            let said: Value = serde_json::from_str(&run_computer(&fields, &ctx, &road).unwrap()).unwrap();
            let expected: &[&str] = if looks { &["walk", "observe"] } else { &["walk"] };
            assert_eq!(calls(dir.path()), expected, "{screen}: {said}");
            let looked = !said["looked_after"].is_null() || !said["ladder"]["relooked"].is_null();
            assert_eq!(looked, looks, "{screen}: {said}");
            assert_eq!(said["first_look"], Value::Null);
            assert!(said["result"].get(walk_words::ROWS).is_none(), "the rows stay the ledger's; the last is shown");
            assert!(said["lastRow"].is_object());
        }
    }

    /// The photo scene's window as the helper's tree names it: its id, its
    /// document's title, and the document's identity when it gives one.
    #[cfg(unix)]
    fn photo_window(document: Option<&str>) -> Value {
        let mut window = json!({"id":PHOTO_WINDOW,"title":PHOTO_TITLE});
        if let Some(document) = document {
            window["document"] = json!(document);
        }
        window
    }

    /// The `n`th look the fake shim answers, in the helper's own shape
    /// (t-70459): a picture, its tree's text and how many elements it read,
    /// and the window it is of at `place` — `tree`, as `observe` answers, or
    /// `snapshot`, as `get-app-state` does.
    #[cfg(unix)]
    fn helper_look(dir: &Path, n: usize, place: &str, window: &Value, text: &str, elements: u64) {
        let picture = dir.join(format!("look-{n}.png"));
        std::fs::write(&picture, TINY_PNG).unwrap();
        let mut result = json!({
            "screenshot": {"path": picture, "width": 640, "height": 415, "scale": 0.5},
            "origin": {"x": 0, "y": 0},
            "changed": null,
            "tree": {"id": "fixture-tree", "text": text, "elementCount": elements},
        });
        result[place]["window"] = window.clone();
        std::fs::write(dir.join(format!("reply-observe.{n}")), json!({"ok": true, "result": result}).to_string()).unwrap();
    }

    /// The photo scene's walk (t-70459): its first step ends `no_look` with
    /// nothing pressed, its row naming the cause and, beside it, the refusing
    /// door's code when given — t-70425's words, as string fixtures until the
    /// core spells them.
    #[cfg(unix)]
    fn no_look_walk(dir: &Path, cause: Option<&str>, refusal: Option<&str>) {
        let mut row = json!({ (walk_words::ATTEMPT): 1, (walk_words::OUTCOME): walk_words::NO_LOOK });
        if let Some(cause) = cause {
            row[walk_words::REASON] = json!(cause);
        }
        if let Some(code) = refusal {
            row["refusal"] = json!(code);
        }
        std::fs::write(dir.join("walk"), walk_answer(JevMode::On.key(), 0, false, None, &[row]).to_string()).unwrap();
    }

    /// The photo scene's walk at its app, in the background, or at a pane.
    #[cfg(unix)]
    fn photo_walk() -> Value {
        json!({"action":"walk","goal":"open the fixture cell","app":PHOTO_APP,"background":true})
    }

    #[cfg(unix)]
    fn pane_walk() -> Value {
        json!({"action":"walk","goal":"open the fixture cell","pane":"browser-13"})
    }

    #[cfg(unix)]
    fn walked(ctx: &ToolContext, road: &ComputerRoad, fields: &Value) -> Value {
        serde_json::from_str(&run_computer(fields, ctx, road).expect("a walk answers")).unwrap()
    }

    /// The ladder's roads by name, in order.
    #[cfg(unix)]
    fn road_names(said: &Value) -> Vec<String> {
        said["ladder"]["roads"].as_array().into_iter().flatten().filter_map(|road| road["road"].as_str().map(str::to_string)).collect()
    }

    /// A road's call with what the model fills in filled in: each `<…>` by a
    /// value of its field's kind.
    #[cfg(unix)]
    fn filled(call: &Value) -> Value {
        let mut call = call.clone();
        if let Some(fields) = call.as_object_mut() {
            for (field, value) in fields {
                if value.as_str().is_some_and(|text| text.starts_with('<')) {
                    *value = match field.as_str() {
                        "mark" => json!(3),
                        "window_id" => json!(41),
                        "coordinate" => json!([20, 40]),
                        _ => json!("fixture words"),
                    };
                }
            }
        }
        call
    }

    /// Every call of the roads at an app is one of the tool's own actions,
    /// sent as the tool reads it once its blanks are filled in.
    #[cfg(unix)]
    fn app_roads_are_the_tools_own(said: &Value) {
        for road in said["ladder"]["roads"].as_array().expect("roads") {
            for call in road["calls"].as_array().expect("calls") {
                let action = call["action"].as_str().unwrap_or_default();
                assert!(ACTIONS.iter().any(|(name, _)| *name == action), "a road names an action the tool has: {call}");
                let sent = serde_json::from_value::<ComputerInput>(filled(call)).map(|fields| argv_for(&fields));
                assert!(matches!(sent, Ok(Ok(_))), "a road's call, filled in, is one the tool sends: {call} -> {sent:?}");
            }
        }
    }

    /// Every command of the roads at a pane is the work browser's own door,
    /// which zo runs as its browser road, once its blanks are filled in — and
    /// the road says which answer's field each revision is read from; the
    /// person is asked by the tool's own `handoff`.
    #[cfg(unix)]
    fn pane_roads_are_the_browser_doors(said: &Value) {
        for road in said["ladder"]["roads"].as_array().expect("roads") {
            for call in road["calls"].as_array().expect("calls") {
                if let Some(command) = call.as_str().filter(|command| command.contains("<revision>")) {
                    assert!(
                        road["fill"].as_str().is_some_and(|fill| fill.contains(&format!("<revision>: that answer's {}", zerocode_core::agent_browser::WORKSPACE_REVISION_KEY))),
                        "a pane's road says where its revision is read: {command} in {road}"
                    );
                }
                if let Some(command) = call.as_str() {
                    let sent = command.replace("<n>", "3").replace("<revision>", "7").replace("<css>", ".name-box").replace("<text>", "B12");
                    assert!(
                        crate::browser_execution::canonical_call(&json!({ "command": sent })).is_some(),
                        "a pane's road is zo's browser road: {sent}"
                    );
                } else {
                    assert_eq!(call["action"], "handoff", "a pane's road is its door's, or the person's: {call}");
                }
            }
        }
    }

    /// The person's scene of 10-22 (t-70459), in fixture words: a walk ends
    /// `no_look` at its first step, nothing pressed, no cause named. Before
    /// anyone asks the person, zo looks at the walk's app once more, in its
    /// background; that look reads the window's tree, so the answer carries
    /// it and the model continues from it — the status stays `failed`, and
    /// there are no roads to take.
    #[cfg(unix)]
    #[test]
    fn a_no_look_walk_looks_once_more_and_continues_from_the_tree() {
        let dir = tempfile::tempdir().unwrap();
        let (road, _) = fake_shim(dir.path());
        let ctx = crate::tests::tool_context_outside_cwd_windows();
        no_look_walk(dir.path(), None, None);
        helper_look(dir.path(), 1, "tree", &photo_window(None), "AXButton «Continue»", 3);
        let said = walked(&ctx, &road, &photo_walk());
        assert_eq!(calls(dir.path()), ["walk", "observe"], "no_look (a): one more look at the walk's app, and nothing pressed: {said}");
        let looked = shim_words(dir.path(), "observe", 1);
        assert!(
            names(&looked, "--app", PHOTO_APP) && looked.iter().any(|word| word == "--background"),
            "no_look (a): the look is at the walk's app, in its background: {looked:?}"
        );
        assert!(
            said["ladder"]["relooked"]["result"]["tree"]["text"].as_str().is_some_and(|text| text.contains("Continue")),
            "no_look (a): the answer carries the tree the second look read: {said}"
        );
        assert_eq!(
            (said["status"].as_str(), said["reason"].as_str()),
            (Some(WalkStatus::Failed.word()), Some(walk_words::NO_LOOK)),
            "no_look (a): a tree to continue from leaves the walk failed: {said}"
        );
        assert!(
            said["ladder"]["next"].as_str().is_some_and(|next| next.contains("relooked")),
            "no_look (a): the answer says to continue from that look: {said}"
        );
        assert!(said["ladder"]["roads"].is_null(), "no_look (a): no roads while there is a tree: {said}");
    }

    /// The person's scene, (b): zo's second look reads an empty tree too.
    /// The walk needs a fallback, and the answer names the roads in the
    /// order to take them, each in the tool's own calls at the window that
    /// look was of and in the walk's background: the picture, pressed by its
    /// marks before its pixels; the page's own address field and the keys;
    /// asking the person — the desktop app — last.
    #[cfg(unix)]
    #[test]
    fn a_no_look_walk_whose_second_look_is_empty_takes_the_roads_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let (road, _) = fake_shim(dir.path());
        let ctx = crate::tests::tool_context_outside_cwd_windows();
        no_look_walk(dir.path(), None, None);
        helper_look(dir.path(), 1, "tree", &photo_window(None), "", 0);
        let said = walked(&ctx, &road, &photo_walk());
        assert_eq!(said["status"], WalkStatus::NeedsFallback.word(), "no_look (b): an empty second look leaves a fallback: {said}");
        assert_eq!(road_names(&said), ["picture", "address_field", "person"], "no_look (b): the roads in order, the person last: {said}");
        app_roads_are_the_tools_own(&said);
        let picture = said["ladder"]["roads"][0]["calls"].as_array().cloned().unwrap_or_default();
        let by = |field: &str| picture.iter().position(|call| call.get(field).is_some());
        assert!(
            by("mark").is_some() && by("coordinate").is_some() && by("mark") < by("coordinate"),
            "no_look (b): the picture is pressed by its marks before its pixels: {said}"
        );
        assert_eq!(picture[0]["window_id"], json!(PHOTO_WINDOW), "no_look (b): the roads are at the window the second look was of: {said}");
        for road in said["ladder"]["roads"].as_array().unwrap() {
            for call in road["calls"].as_array().unwrap().iter().filter(|call| call["action"] != "handoff") {
                assert_eq!(call["background"], true, "no_look (b): every road stays in the walk's background: {call}");
            }
        }
        assert_eq!(said["ladder"]["roads"][2]["calls"][0]["action"], "handoff", "no_look (b): the person is asked by handoff, last: {said}");
        assert!(said["ladder"]["relooked"].is_object(), "no_look (b): the second look is in the answer, its picture the first road's: {said}");
        assert_eq!(calls(dir.path()), ["walk", "observe"], "no_look (b): one look, nothing pressed: {said}");
    }

    /// The cause a `no_look` row names picks where the ladder starts
    /// (t-70459): a window not found is found again before the look, and
    /// the look is at the window found; a tree said to be empty is not looked
    /// at again — the roads come at once; the clock is one more look, as no
    /// cause is.
    #[cfg(unix)]
    #[test]
    fn a_no_look_walk_climbs_by_the_cause_its_row_names() {
        let dir = tempfile::tempdir().unwrap();
        let (road, _) = fake_shim(dir.path());
        let ctx = crate::tests::tool_context_outside_cwd_windows();
        no_look_walk(dir.path(), Some("no_window"), None);
        let row = |id: u64, app: &str| json!({ "id": id, "title": PHOTO_TITLE, "app": { "name": app, "pid": 501 }, "x": 0, "y": 0, "width": 640, "height": 400 });
        let desktop = json!({ "ok": true, "result": { "windows": [row(12, "Mail"), row(52, PHOTO_APP)] } });
        std::fs::write(dir.path().join("list-all-windows"), desktop.to_string()).unwrap();
        helper_look(dir.path(), 1, "tree", &json!({"id":52,"title":PHOTO_TITLE}), "AXButton «Continue»", 3);
        let said = walked(&ctx, &road, &photo_walk());
        assert_eq!(calls(dir.path()), ["walk", "list-all-windows", "observe"], "no_window: the window is found again before the look: {said}");
        assert!(names(&shim_words(dir.path(), "observe", 1), "--window-id", "52"), "no_window: the look is at the window found: {said}");
        assert_eq!(said["ladder"]["refound"]["windows"], json!([{"id":52,"title":PHOTO_TITLE}]), "no_window: the app's windows found, by id and title: {said}");
        assert_eq!(said["status"], WalkStatus::Failed.word(), "no_window: the look found reads the tree: {said}");

        let dir = tempfile::tempdir().unwrap();
        let (road, _) = fake_shim(dir.path());
        let ctx = crate::tests::tool_context_outside_cwd_windows();
        no_look_walk(dir.path(), Some("empty_tree"), None);
        let said = walked(&ctx, &road, &photo_walk());
        assert_eq!(calls(dir.path()), ["walk"], "empty_tree: no second look at a tree said to be empty: {said}");
        assert_eq!(said["status"], WalkStatus::NeedsFallback.word(), "empty_tree: the roads at once: {said}");
        assert_eq!(road_names(&said), ["picture", "address_field", "person"], "empty_tree: {said}");
        app_roads_are_the_tools_own(&said);

        let dir = tempfile::tempdir().unwrap();
        let (road, _) = fake_shim(dir.path());
        let ctx = crate::tests::tool_context_outside_cwd_windows();
        no_look_walk(dir.path(), Some("look_timeout"), None);
        helper_look(dir.path(), 1, "tree", &photo_window(None), "AXButton «Continue»", 3);
        let said = walked(&ctx, &road, &photo_walk());
        assert_eq!(calls(dir.path()), ["walk", "observe"], "look_timeout: one more look, as with no cause: {said}");
        assert_eq!(said["ladder"]["cause"], "look_timeout", "look_timeout: the cause is in the answer: {said}");

        let dir = tempfile::tempdir().unwrap();
        let (road, _) = fake_shim(dir.path());
        let ctx = crate::tests::tool_context_outside_cwd_windows();
        no_look_walk(dir.path(), Some(""), None);
        helper_look(dir.path(), 1, "tree", &photo_window(None), "AXButton «Continue»", 3);
        let said = walked(&ctx, &road, &photo_walk());
        assert_eq!(calls(dir.path()), ["walk", "observe"], "an empty reason: the ladder from its start: {said}");
        assert!(said["ladder"]["cause"].is_null(), "an empty reason names no cause: {said}");
    }

    /// The person's scene as the window found it (t-70459, 4275): the work
    /// browser refused the walk's look of its pane. The row says so —
    /// reason `look_refused`, the door's code beside it in `refusal`
    /// (`error` when the window could not read it) — and is not looked at
    /// again, which would meet the same door: the answer reports the door's
    /// code as a defect, or no code when the row gives none; a reason outside
    /// the four is a door's refusal too, reported by that word. At an app
    /// the walk stops; at a pane its own door, which zo runs as its browser
    /// road, is the one road left.
    #[cfg(unix)]
    #[test]
    fn a_no_look_walk_a_door_refused_is_reported_and_not_looked_at_again() {
        for (screen, cause, refusal, status, report, roads) in [
            (photo_walk(), "look_refused", Some("workspace_actor_required"), WalkStatus::Stopped,
                "door refusal workspace_actor_required: report it as a defect", &[][..]),
            (pane_walk(), "look_refused", Some("workspace_actor_required"), WalkStatus::NeedsFallback,
                "door refusal workspace_actor_required: report it as a defect", &["picture"][..]),
            (photo_walk(), "look_refused", Some("error"), WalkStatus::Stopped, "door refusal error: report it as a defect", &[][..]),
            (photo_walk(), "look_refused", None, WalkStatus::Stopped, "door refusal: report it as a defect", &[][..]),
            (photo_walk(), "no_such_word", None, WalkStatus::Stopped, "door refusal no_such_word: report it as a defect", &[][..]),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let (road, _) = fake_shim(dir.path());
            let ctx = crate::tests::tool_context_outside_cwd_windows();
            no_look_walk(dir.path(), Some(cause), refusal);
            let said = walked(&ctx, &road, &screen);
            let case = format!("{cause} {refusal:?} at {screen}");
            assert_eq!(said["ladder"]["report"], report, "{case}: the door is reported as a defect, by its code: {said}");
            assert_eq!(calls(dir.path()), ["walk"], "{case}: a door's refusal is not looked through again: {said}");
            assert_eq!(said["status"], status.word(), "{case}: {said}");
            assert_eq!(road_names(&said), roads, "{case}: {said}");
            if !roads.is_empty() {
                pane_roads_are_the_browser_doors(&said);
            }
        }
    }

    /// The person took the foreground while a background walk looked
    /// (t-70425): its row says `look_refused`, `background_interrupted`
    /// beside it. No door failed — the walk stops for the person, says so
    /// without calling it a defect, and names one road: asking the person,
    /// by handoff, whether it goes on (t-70459).
    #[cfg(unix)]
    #[test]
    fn a_background_walk_the_person_took_over_stops_for_them_and_is_no_defect() {
        let dir = tempfile::tempdir().unwrap();
        let (road, _) = fake_shim(dir.path());
        let ctx = crate::tests::tool_context_outside_cwd_windows();
        no_look_walk(dir.path(), Some("look_refused"), Some("background_interrupted"));
        let said = walked(&ctx, &road, &photo_walk());
        assert_eq!(
            said["ladder"]["report"], "the person took the foreground; the background walk stopped",
            "background_interrupted: the person took over, no defect: {said}"
        );
        assert_eq!(calls(dir.path()), ["walk"], "background_interrupted: no second look at a screen the person holds: {said}");
        assert_eq!(said["status"], WalkStatus::Stopped.word(), "background_interrupted: the walk stops for the person: {said}");
        assert_eq!(road_names(&said), ["person"], "background_interrupted: the one road is the person's: {said}");
        assert_eq!(said["ladder"]["roads"][0]["calls"][0]["action"], "handoff", "background_interrupted: the person is asked by handoff: {said}");
        app_roads_are_the_tools_own(&said);
    }

    /// The window's own command road could not reach the window (t-70855:
    /// "could not reach the window", or curl's `(7)`), and the row says
    /// `look_refused`, `door_connection_failed` beside it. A second look would
    /// go down the same road: the walk at an app stops, and the answer
    /// reports the code as the door's defect (t-70459).
    #[cfg(unix)]
    #[test]
    fn a_no_look_walk_whose_door_could_not_reach_the_window_stops_and_reports_the_door() {
        let dir = tempfile::tempdir().unwrap();
        let (road, _) = fake_shim(dir.path());
        let ctx = crate::tests::tool_context_outside_cwd_windows();
        no_look_walk(dir.path(), Some(walk_words::LOOK_REFUSED), Some(DOOR_CONNECTION_FAILED));
        let said = walked(&ctx, &road, &photo_walk());
        assert_eq!(calls(dir.path()), ["walk"], "door_connection_failed: no second look down a road that did not reach the window: {said}");
        assert_eq!(said["status"], WalkStatus::Stopped.word(), "door_connection_failed: the walk stops: {said}");
        assert_eq!(
            said["ladder"]["report"], "door refusal door_connection_failed: report it as a defect",
            "door_connection_failed: reported as the door's defect, by its code: {said}"
        );
        assert!(said["ladder"]["roads"].is_null(), "door_connection_failed: no road at an app: {said}");
    }

    /// A walk that ends `stuck` climbs the same ladder and says why: the
    /// screen did not move (t-70459). At an app zo's one look after it is
    /// the ladder's, with its tree — not a second; at a pane the roads are
    /// the pane's own door's.
    #[cfg(unix)]
    #[test]
    fn a_stuck_walk_climbs_the_same_ladder_saying_the_screen_did_not_move() {
        let stuck = walk_answer(JevMode::On.key(), 1, false, None, &[
            walk_row(1, json!({ (walk_words::PRESSED): true })),
            json!({ (walk_words::ATTEMPT): 2, (walk_words::OUTCOME): walk_words::STUCK, (walk_words::PRESSED): 1 }),
        ]);
        let dir = tempfile::tempdir().unwrap();
        let (road, _) = fake_shim(dir.path());
        let ctx = crate::tests::tool_context_outside_cwd_windows();
        std::fs::write(dir.path().join("walk"), stuck.to_string()).unwrap();
        helper_look(dir.path(), 1, "tree", &photo_window(None), "AXButton «Continue»", 3);
        let said = walked(&ctx, &road, &photo_walk());
        assert_eq!(said["ladder"]["why"], "the screen did not move", "stuck at an app: the answer says why: {said}");
        assert_eq!(calls(dir.path()), ["walk", "observe"], "stuck at an app: one look after the walk: {said}");
        assert!(said["looked_after"].is_null(), "stuck at an app: the ladder's look is the one after, not a second: {said}");
        assert!(
            said["ladder"]["relooked"]["result"]["tree"]["text"].as_str().is_some_and(|text| text.contains("Continue")),
            "stuck at an app: the look carries its tree: {said}"
        );
        assert_eq!((said["status"].as_str(), said["reason"].as_str()), (Some(WalkStatus::Failed.word()), Some(walk_words::STUCK)), "stuck at an app: {said}");

        let dir = tempfile::tempdir().unwrap();
        let (road, _) = fake_shim(dir.path());
        let ctx = crate::tests::tool_context_outside_cwd_windows();
        std::fs::write(dir.path().join("walk"), stuck.to_string()).unwrap();
        let said = walked(&ctx, &road, &pane_walk());
        assert_eq!(said["ladder"]["why"], "the screen did not move", "stuck at a pane: the answer says why: {said}");
        assert_eq!(calls(dir.path()), ["walk"], "stuck at a pane: zo's road does not look at a pane: {said}");
        assert_eq!(said["status"], WalkStatus::NeedsFallback.word(), "stuck at a pane: {said}");
        assert_eq!(road_names(&said), ["picture", "address_field", "person"], "stuck at a pane: the roads in order, the person last: {said}");
        pane_roads_are_the_browser_doors(&said);
    }

    /// The words of a `no_look` row zo climbs by, letter for letter as the
    /// window writes them: the core's walk words (t-70425), and the one
    /// refusal zo names before the core does (t-70855) — so a word renamed
    /// on either side shows here (t-70459).
    #[test]
    fn the_no_look_rows_words_are_spelled_as_the_window_writes_them() {
        assert_eq!(
            [
                walk_words::NO_WINDOW,
                walk_words::EMPTY_TREE,
                walk_words::LOOK_TIMEOUT,
                walk_words::LOOK_REFUSED,
                walk_words::REFUSAL,
                walk_words::BACKGROUND_INTERRUPTED,
                DOOR_CONNECTION_FAILED,
            ],
            ["no_window", "empty_tree", "look_timeout", "look_refused", "refusal", "background_interrupted", "door_connection_failed"]
        );
    }

    /// The tool's own words say the ladder (t-70459): after `no_look` or
    /// `stuck` the walk looks again, and the person is asked last.
    #[test]
    fn the_computer_description_names_the_walks_ladder() {
        let description = tool_specs()[0].description;
        assert!(
            description.contains("no_look/stuck") && description.contains("relooked") && description.contains("the person last"),
            "the Computer description says the walk's ladder: {description}"
        );
    }

    /// The skill's road table: every road a model may take, each named by
    /// the words the zo tool and the CLI actually take, and a walk example
    /// the tool reads as the CLI's walk.
    #[test]
    fn the_skill_names_every_road_by_its_real_words() {
        let skill = include_str!("../../../../skills/computer-use/SKILL.md");
        let table = &skill[skill.find("## Choosing a road").expect("the road table")..];
        let table = &table[..table[3..].find("\n## ").map_or(table.len(), |end| end + 3)];
        for road in ["batch", "recipe_run", WALK_ACTION, ComputerMethod::ReflexStart.verb_name()] {
            assert!(table.contains(&format!("`{road}`")) || table.contains(road), "the table names {road}");
        }
        for action in ["left_click", BATCH_ACTION, "recipe_run", WALK_ACTION] {
            assert!(ACTIONS.iter().any(|(name, _)| *name == action), "{action} is a Computer action");
            assert!(table.contains(&format!("`{action}`")), "the table names the zo action `{action}`");
        }
        assert!(table.contains("zerocode-computer walk --goal"), "the one line other agents are given");
        assert!(
            table.split_whitespace().collect::<Vec<_>>().join(" ").contains(&format!(
                "{} by default, {WALK_STEPS_MAX} at most",
                zerocode_core::computer_use::WALK_STEPS_DEFAULT
            )),
            "the table says the core's budget"
        );
        let example = table
            .split("```json")
            .nth(1)
            .and_then(|block| block.split("```").next())
            .expect("a walk example");
        let example: Value = serde_json::from_str(example).expect("the example is JSON");
        let command = zerocode_core::computer_use::parse_command(&argv_for(&input(example)).unwrap()).expect("the example walks");
        assert_eq!(command.method, ComputerMethod::Walk);
        for status in WalkStatus::ALL {
            assert!(table.contains(&format!("`{}`", status.word())), "the table says what {} means", status.word());
        }
    }

    /// A step's fields are exactly the ones a step's actions read: set every
    /// field, take one away, and see whether any batchable action's command
    /// line changes.
    #[test]
    fn a_steps_fields_are_the_ones_its_actions_read() {
        let full = |action: &str| -> Value {
            json!({
                "action": action, "coordinate": [1, 2], "start_coordinate": [3, 4], "text": "t",
                "scroll_direction": "down", "scroll_amount": 2, "duration": 0.5, "region": [0, 0, 9, 9],
                "app": "A", "url": "u", "path": "/p", "program": "/bin/p", "args": "a", "role": "r", "label": "l",
                "ocr": true, "window": "w", "window_id": 7, "kind": "move", "x": 5, "y": 6, "width": 7, "height": 8,
                "timeout_ms": 100, "absent": true, "confirming": "payment", "force": true, "reset_budget": true,
                "last": 3, "after": 2, "min_confidence": 0.5, "until": "quiet", "after_text": "f",
                "offset": [1, 2], "scroll_pages": 0.5, "background": true, "browser_chrome": true
            })
        };
        let argv = |fields: &Value| argv_for(&input(fields.clone())).map_err(|error| error.to_string());
        // A click by words reads its own fields only when no coordinate
        // competes for its one target: every set is also tried without one.
        let without_coordinate = |mut fields: Value| {
            fields.as_object_mut().unwrap().remove("coordinate");
            fields
        };
        let mut read = std::collections::BTreeSet::from(["action".to_string()]);
        for action in action_names(|action| methods(action).any(ComputerMethod::batches)) {
            for app_scope in [false, true] {
                let mut fields = full(action);
                let object = fields.as_object_mut().unwrap();
                object.remove("window"); // Title and element conditions are separate capabilities.
                if !app_scope || (!app_scoped_action(action) && action != "activate") { object.remove("app"); }
                if action != "window" && (!object.contains_key("app") || !app_scoped_action(action)) { object.remove("window_id"); }
                if !object.contains_key("app") || matches!(action, "left_click_drag" | "activate") { object.remove("background"); }
                if action == "scroll" && object.contains_key("app") {
                    object.remove("scroll_amount");
                } else {
                    object.remove("scroll_pages");
                }
                let mut variants = vec![fields.clone(), without_coordinate(fields)];
                if action == "wait_for" {
                    variants.push(json!({"action": "wait_for", "window": "w", "timeout_ms": 100, "absent": true}));
                }
                for whole in variants {
                    for field in whole.as_object().unwrap().keys().filter(|field| *field != "action") {
                        let mut without = whole.clone();
                        without.as_object_mut().unwrap().remove(field);
                        if argv(&whole) != argv(&without) {
                            read.insert(field.clone());
                        }
                    }
                }
            }
        }
        let listed: std::collections::BTreeSet<String> = STEP_FIELDS.iter().map(|field| (*field).to_string()).collect();
        assert_eq!(listed, read, "STEP_FIELDS is what the step actions read");
    }

    /// t-37883: a press by the words a window with no tree shows is the
    /// core's `click --ocr`, its nudge in the picture's pixels moved into
    /// points like every other place the model names.
    #[test]
    fn a_click_by_the_words_a_window_shows_is_the_cores_ocr_click() {
        let frame = half((100.0, 0.0));
        let click = input(json!({ "action": "left_click", "app": "iPhone Mirroring", "label": "No", "ocr": true,
            "after_text": "visited a farm", "offset": [0, 4] }));
        assert!(speaks_in_pixels(&click), "a nudge is a length in the picture's pixels");
        let argv = argv_for(&in_points(click, frame).unwrap()).unwrap();
        assert_eq!(
            argv,
            ["click", "--app", "iPhone Mirroring", "--ocr", "--text", "No", "--after-text", "visited a farm", "--dx", "0", "--dy", "8",
                "--mouse-button", "left", "--click-count", "1", "--json"]
        );
        assert!(zerocode_core::computer_use::parse_command(&argv).is_ok(), "the window takes it");
    }

    /// t-37883: a batch whose steps acted in an app is looked at in that
    /// app's window, not the whole desktop the model would only look at
    /// again; with no app named, the window the model last looked at.
    #[cfg(unix)]
    #[test]
    fn the_look_after_a_batch_is_of_the_app_it_acted_in() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (road, _) = fake_shim(dir.path());
        let ran: Vec<Value> = (1..=2).map(|n| json!({ "n": n, "verb": "x", "ok": true, "ms": 1, "result": {} })).collect();
        std::fs::write(dir.path().join("batch"), json!({ "ok": true, "result": { "ran": 2, "of": 2, "steps": ran } }).to_string()).unwrap();
        let ctx = crate::tests::tool_context_outside_cwd_windows();
        ctx.set_computer_frame(half((0.0, 0.0)));
        let steps = json!([{ "action": "activate", "app": "iPhone Mirroring" }, { "action": "left_click", "coordinate": [10, 10] }]);
        let answer: Value = serde_json::from_str(&run_computer(&json!({ "action": "batch", "steps": steps }), &ctx, &road).expect("an answer")).unwrap();
        let said = answer["looked_after"]["screen"]["said"].as_str().unwrap_or_default().to_string();
        assert!(said.contains("observe --diff --settle --app iPhone Mirroring"), "the look after: {said}");
        let unnamed = json!([{ "action": "left_click", "coordinate": [10, 10] }, { "action": "key", "text": "a" }]);
        let answer: Value = serde_json::from_str(&run_computer(&json!({ "action": "batch", "steps": unnamed }), &ctx, &road).expect("an answer")).unwrap();
        let said = answer["looked_after"]["screen"]["said"].as_str().unwrap_or_default().to_string();
        assert!(said.contains("--app iPhone Mirroring"), "the app the model last looked at: {said}");
    }

    /// A summarizer that keeps what it was asked to summarize.
    #[cfg(unix)]
    #[derive(Default)]
    struct Recording(std::cell::RefCell<String>);

    #[cfg(unix)]
    impl CompactionSummarizer for Recording {
        fn summarize(&self, messages: &[ConversationMessage]) -> String {
            *self.0.borrow_mut() =
                messages.iter().map(|message| message.to_json().render()).collect::<Vec<_>>().join("\n");
            String::from("<summary>Conversation summary: a turn passed.</summary>")
        }
    }

    /// Where a value is found after a conversation has been written down,
    /// compacted and read again by the roads of a zo session.
    #[cfg(unix)]
    #[derive(Debug, Default, PartialEq, Eq)]
    struct Found {
        /// The files a persisted session leaves: its transcript, a rotated
        /// copy, its vault.
        files: Vec<String>,
        /// The roads that carry the conversation on: the next request, what a
        /// summary is asked about, the deterministic summary, the turn trace.
        roads: Vec<&'static str>,
    }

    /// A conversation around `middle`: the person's first words, `middle`, and
    /// four turns that follow, so a compaction seals `middle` into the vault.
    #[cfg(unix)]
    fn a_conversation(middle: Vec<ConversationMessage>) -> Vec<ConversationMessage> {
        let says = |words: &str| vec![ContentBlock::Text { text: words.to_string() }];
        let mut messages = vec![ConversationMessage::user_text("Sign in with the code the site sends")];
        messages.extend(middle);
        messages.extend([
            ConversationMessage::assistant(says("Signed in.")),
            ConversationMessage::user_text("Thanks."),
            ConversationMessage::assistant(says("Anything else?")),
            ConversationMessage::user_text("No."),
        ]);
        messages
    }

    /// The session files in `dir` that hold `needle`, by name.
    #[cfg(unix)]
    fn files_holding(dir: &Path, needle: &str) -> Vec<String> {
        let mut holding: Vec<String> = std::fs::read_dir(dir)
            .map(|entries| entries.flatten().collect::<Vec<_>>())
            .unwrap_or_default()
            .into_iter()
            .filter(|entry| entry.file_name().to_string_lossy().starts_with("session"))
            .filter(|entry| String::from_utf8_lossy(&std::fs::read(entry.path()).unwrap_or_default()).contains(needle))
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        holding.sort();
        holding
    }

    /// `messages` as a zo session lives them: pushed to a persisted session,
    /// compacted once (the summary asked about the evicted turns, their raw
    /// originals sealed to the vault), converted for the next request,
    /// summarized by the deterministic summarizer and traced as a turn — and
    /// where `needle` is found on each road.
    #[cfg(unix)]
    fn scan_everywhere(messages: &[ConversationMessage], needle: &str) -> Found {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut session = Session::new().with_persistence_path(dir.path().join("session.jsonl"));
        for message in messages {
            let pushed = session.push_message(message.clone());
            assert!(pushed.is_ok(), "the session takes a message: {pushed:?}");
        }
        let recording = Recording::default();
        let config = runtime::CompactionConfig { preserve_recent_messages: 2, max_estimated_tokens: 1 };
        let compacted = runtime::compact_session_with(&session, config, &recording);
        assert!(compacted.removed_message_count > 0, "the conversation was compacted, its turns sealed to the vault");
        let turn = runtime::TurnSummary {
            assistant_messages: Vec::new(),
            tool_results: messages.iter().filter(|message| message.role == MessageRole::Tool).cloned().collect(),
            prompt_cache_events: Vec::new(),
            iterations: 1,
            usage: runtime::usage::TokenUsage::default(),
            turn_output_tokens: 0,
            auto_compaction: None,
            microcompact: None,
            deep_verification: None,
            verification_issues: Vec::new(),
            deep_verifier_parse: None,
            deep_verifier_model: None,
            budget_exhausted: None,
        };
        let roads = [
            ("wire", serde_json::to_string(&runtime::convert_messages(messages)).unwrap_or_default()),
            ("summary asked", recording.0.borrow().clone()),
            ("local summary", runtime::LocalSummarizer.summarize(messages)),
            ("turn trace", serde_json::to_string(&runtime::TurnRecord::from_summary("session", 1, &turn, None)).unwrap_or_default()),
        ];
        Found {
            files: files_holding(dir.path(), needle),
            roads: roads.into_iter().filter(|(_, text)| text.contains(needle)).map(|(road, _)| road).collect(),
        }
    }

    /// The code a window answers is in no store of a zo conversation (t-40807):
    /// the answer of a window that types the code into its field carries none,
    /// and a handoff that asks for a code with no field to put it in is refused
    /// before the window is asked — so neither result reaches the transcript,
    /// its rotated copies, the vault, the next request, what a summary is asked
    /// about, the summary, or the turn trace; nor does the model hold a code
    /// to type into a call of its own. The same scan finds a code planted in a
    /// tool result on every road that carries one, and on the turn trace — which
    /// keeps no output — it finds none.
    #[cfg(unix)]
    #[test]
    fn a_code_the_window_answers_is_in_no_store_of_a_zo_conversation() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (road, _) = fake_shim(dir.path());
        let ctx = crate::tests::tool_context_outside_cwd_windows();
        let calls_made = [
            ("call-1", a_code_into_a_field()),
            ("call-2", json!({ "action": "handoff", "text": "The site's one-time code", "ask_code": true })),
        ];
        let mut middle = Vec::new();
        for (id, fields) in &calls_made {
            let answer = run_computer(fields, &ctx, &road);
            let is_error = answer.is_err();
            let output = answer.unwrap_or_else(|error| error.to_string());
            let call = ContentBlock::ToolUse { id: (*id).to_string(), name: "Computer".to_string(), input: fields.to_string() };
            middle.push(ConversationMessage::assistant(vec![call]));
            middle.push(ConversationMessage::tool_result(*id, "Computer", output, is_error));
        }
        let conversation = a_conversation(middle);

        let planted_answer = format!("{{\"ok\":true,\"result\":{{\"code\":\"{A_CODE}\"}}}}");
        let planted = scan_everywhere(
            &a_conversation(vec![
                ConversationMessage::assistant(vec![ContentBlock::ToolUse {
                    id: "plant".to_string(),
                    name: "Computer".to_string(),
                    input: "{}".to_string(),
                }]),
                ConversationMessage::tool_result("plant", "Computer", planted_answer, false),
            ]),
            A_CODE,
        );
        assert!(planted.files.iter().any(|name| name.ends_with(".vault.jsonl")), "the vault keeps the evicted turns raw: {planted:?}");
        for stored in ["wire", "summary asked", "local summary"] {
            assert!(planted.roads.contains(&stored), "the scan does not see a planted code on the {stored} road: {planted:?}");
        }
        assert!(!planted.roads.contains(&"turn trace"), "the turn trace keeps no tool output: {planted:?}");
        let alive = scan_everywhere(&conversation, "call-1");
        assert!(alive.roads.contains(&"wire") && !alive.files.is_empty(), "the roads do not carry this conversation: {alive:?}");

        let found = scan_everywhere(&conversation, A_CODE);
        assert_eq!(found, Found::default(), "a code the window answered reached a store of the conversation");
        for message in &conversation {
            for block in &message.blocks {
                if let ContentBlock::ToolUse { input, .. } = block {
                    assert!(!input.contains(A_CODE), "the model was handed a code to type: {input}");
                }
            }
        }
    }

    /// The tool's seat in a conversation once it is looked up, measured the
    /// way the harness measures every schema (the serialized definition,
    /// `chars/4 + 1`), and pinned from above so a batch's step items cannot
    /// quietly grow back into a second copy of the tool. 1,436 with batch;
    /// the recipe actions (four names, three fields, one sentence) add ~80,
    /// the person's turn (one name, one sentence) ~25, the eye's `watch`
    /// (one name in both enums, one field in both, one sentence) ~52, a
    /// look by `observe` and a press by `element_index` (one name, one field,
    /// one sentence; never a batch step) ~42, a press by reading (label and role
    /// with app, a sentence and the plan-it-whole clause) ~40, a press by the
    /// words a window with no tree shows (two fields in the tool and in a
    /// step, one sentence; t-37883) ~135, a walk (one
    /// name, eight fields, `until` read by two actions, one sentence) ~134,
    /// the person's turn asking for a one-time code (one boolean, one clause)
    /// ~19, where the window types it (`into`: one array property, one
    /// sentence) ~55, and the ladder after a walk's `no_look` or `stuck` (one
    /// clause; t-70459) ~29.
    #[test]
    fn the_computer_schema_is_measured() {
        const CEILING_TOKENS: usize = 2_080;
        let spec = tool_specs().pop().expect("one spec");
        let definition = json!({ "name": spec.name, "description": spec.description, "input_schema": spec.input_schema });
        let compact = serde_json::to_string(&definition).unwrap().chars().count();
        let pretty = serde_json::to_string_pretty(&definition).unwrap().chars().count();
        let steps = serde_json::to_string(&spec.input_schema["properties"]["steps"]).unwrap().chars().count();
        let tokens = compact / 4 + 1;
        eprintln!("Computer schema: {compact} chars compact ({steps} of them the batch's steps), {pretty} pretty ≈ {tokens} tokens");
        assert!(tokens <= CEILING_TOKENS, "the Computer schema grew to {tokens} tokens");
    }
}
