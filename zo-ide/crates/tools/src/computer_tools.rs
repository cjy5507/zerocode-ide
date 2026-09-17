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
use serde::Deserialize;
use serde_json::{json, Value};
use zerocode_core::computer_use::{
    computer_deadline_ms, ComputerMethod, BATCH_COMMANDS_FLAG, COMPUTER_BATCH_MAX_STEPS, COMPUTER_BRIDGE_GRACE_MS,
    COMPUTER_MARKS_ENV, WATCH_UNTIL,
};
use zerocode_core::computer_use_protocol::error_code;
use zerocode_core::computer_use_protocol::frame::ShotFrame;
use zerocode_core::computer_use_protocol::marks::{legend_line, ITEMS_KEY, LEGEND_KEY, LOOK_ID_KEY, TOOL_ONLY_KEYS};

use super::{
    from_value, maybe_enforce_permission_check, to_pretty_json, ToolContext, ToolError, ToolSpec,
};
use crate::context::ComputerObservation;

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
/// Actions whose answer names places on the screen: before the first of them
/// the model is shown a picture, so what they answer is in its pixels.
const ANSWERS_PLACES: &[&str] = &["find", "read", "wait_for", "list_windows", "cursor_position"];
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

#[derive(Debug, Default, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ComputerInput {
    pub action: String,
    /// `[x, y]` in the pixels of the last screenshot, the Anthropic way.
    pub coordinate: Option<[f64; 2]>,
    pub start_coordinate: Option<[f64; 2]>,
    /// The key chord for `key`/`hold_key`, the text for `type`, the modifiers
    /// held during a click, the text to find/wait for, the clipboard text.
    pub text: Option<String>,
    pub scroll_direction: Option<String>,
    pub scroll_amount: Option<u32>,
    /// Seconds, the Anthropic way, for `hold_key` and `wait`.
    pub duration: Option<f64>,
    /// `[x0, y0, x1, y1]` — two corners, the Anthropic way — in the last
    /// screenshot's pixels, for `zoom`/`find --ocr`/`read --ocr`.
    pub region: Option<[f64; 4]>,
    pub app: Option<String>,
    pub url: Option<String>,
    pub path: Option<String>,
    pub program: Option<String>,
    pub args: Option<String>,
    pub role: Option<String>,
    pub label: Option<String>,
    pub ocr: Option<bool>,
    /// A window title fragment for `wait_for`.
    pub window: Option<String>,
    pub window_id: Option<u64>,
    /// The window action (`focus|move|resize|minimize|zoom|close`).
    pub kind: Option<String>,
    pub x: Option<f64>,
    pub y: Option<f64>,
    pub width: Option<f64>,
    pub height: Option<f64>,
    pub timeout_ms: Option<u64>,
    pub absent: Option<bool>,
    /// The model's own declaration that this press is a payment, transfer
    /// or delete — the window asks the person before the helper moves.
    pub confirming: Option<String>,
    pub force: Option<bool>,
    pub reset_budget: Option<bool>,
    pub last: Option<u64>,
    /// The ears' cursor: events heard after this number.
    pub after: Option<u64>,
    /// The least confidence a heard sound needs to end a `sound_wait`.
    pub min_confidence: Option<f64>,
    /// `watch`: `change` (the default) or `quiet`.
    pub until: Option<String>,
    /// `batch`: the actions to run in order in one call.
    pub steps: Option<Vec<ComputerInput>>,
    /// A recipe's name (`recipe_save`/`recipe_show`/`recipe_run`).
    pub name: Option<String>,
    /// `recipe_run`: the values its `{{name}}`s are filled with.
    pub params: Option<serde_json::Map<String, Value>>,
    /// `recipe_run`: the step to walk from (counted from 1), as a stop said.
    pub start: Option<u64>,
    /// A click's target by its number on the last marked look, instead of a
    /// `coordinate`: the window presses that control and refuses if it moved.
    pub mark: Option<u64>,
    /// A `left_click`'s target by its index in the named app's tree (as an
    /// `observe` with `app` shows it), pressed through accessibility.
    pub element_index: Option<u64>,
    /// Whether the look after an action waits for quiet. Continuous motion
    /// can be sampled immediately, while ordinary UI actions wait by default.
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
            "description": "[x, y] where a drag starts, in the last screenshot's pixels" },
        "text": { "type": "string" },
        "scroll_direction": { "type": "string", "enum": ["up", "down", "left", "right"] },
        "scroll_amount": { "type": "integer", "minimum": 1 },
        "duration": { "type": "number", "minimum": 0 },
        "region": { "type": "array", "items": { "type": "number" }, "minItems": 4, "maxItems": 4,
            "description": "[x0, y0, x1, y1]: the top-left and bottom-right corners, in the last screenshot's pixels" },
        "app": { "type": "string" },
        "url": { "type": "string" },
        "path": { "type": "string" },
        "program": { "type": "string" },
        "args": { "type": "string" },
        "role": { "type": "string" },
        "label": { "type": "string" },
        "ocr": { "type": "boolean" },
        "window": { "type": "string" },
        "window_id": { "type": "integer", "minimum": 0 },
        "kind": { "type": "string", "enum": kinds },
        "x": { "type": "number" },
        "y": { "type": "number" },
        "width": { "type": "number" },
        "height": { "type": "number" },
        "timeout_ms": { "type": "integer", "minimum": 1 },
        "absent": { "type": "boolean" },
        "confirming": { "type": "string", "enum": ["payment", "transfer", "delete"] },
        "force": { "type": "boolean" },
        "reset_budget": { "type": "boolean" },
        "last": { "type": "integer", "minimum": 1 },
        "after": { "type": "integer", "minimum": 0,
            "description": "sound_read/sound_wait: only sounds heard after this event number" },
        "min_confidence": { "type": "number", "minimum": 0, "maximum": 1 },
        "until": { "type": "string", "enum": WATCH_UNTIL },
        "element_index": { "type": "integer", "minimum": 0 },
        "settle": { "type": "boolean",
            "description": "Action/batch: false samples immediately, without confirming completion. Default true waits for quiet." },
        "name": { "type": "string" },
        "params": { "type": "object" },
        "start": { "type": "integer", "minimum": 1 }
    });
    // Only a process whose looks carry marks offers the field; otherwise it
    // costs the schema nothing.
    if marks {
        properties["mark"] = json!({ "type": "integer", "minimum": 1,
            "description": "a click's target by its number in the last look's marks legend, instead of coordinate" });
    }
    if let (Some(fields), Some(object)) = (fields, properties.as_object_mut()) {
        object.retain(|name, _| fields.contains(&name.as_str()));
    }
    properties
}

pub(crate) fn tool_specs() -> Vec<ToolSpec> {
    vec![ToolSpec {
        name: "Computer",
        description: "Drive the person's desktop through the ZeroCode window: look (screenshot, zoom), \
            move/click/drag/scroll the mouse, press keys, type, wait; launch, activate, quit and open \
            apps; find/wait_for/read elements or screen text (ocr reads pixels); place windows; \
            read/write the clipboard; run a program. Every position \
            you give — coordinate, start_coordinate, region [x0, y0, x1, y1], a window's x/y/width/height \
            — is in the pixels of the last full screenshot you were shown, and every position the tool \
            answers is in those pixels too; the tool places them on the screen. A zoom only shows a \
            region closer: keep speaking in the screenshot's pixels. After an action that changes the \
            screen the tool gives it a moment to repaint and looks for you; when nothing changed it says \
            changed: false and shows no new picture. The tool hears too: listen_start (the whole \
            machine, or one app), then sound_wait for a sound by label (text: \"alarm_clock,siren\"; \
            the system's classifier names speech, music, knock, bell, beep, siren and some 300 more) \
            or sound_read what was heard; listen_stop when done. watch waits for the screen to change (until: \
            quiet, to go still) without pictures and says where. observe with app shows its accessibility \
            tree, each line led by an element_index a left_click with app presses; app + label (and role) presses \
            the control that reads so, chosen at the press — never stale, so a batch step can name it. A familiar run \
            of hand steps you would not look between is one batch (steps: [...]): one call, one look after — plan it \
            whole (left_click by label, type, key, wait_for) rather than a look per press. recipe_save \
            keeps a procedure that worked; recipe_run walks it again in one call (params fill its \
            {{names}}) and says where it stopped and the start to resume from. A code, CAPTCHA or \
            password field (secure_input) is the person's: handoff (text: what they do) waits for them. \
            The person keeps one hand on you: a \
            `stopped` answer means stop and report; a press on a payment, transfer or delete control is \
            held while the window asks them — say confirming: payment|transfer|delete when you know you \
            are there. Only works inside a ZeroCode pane.",
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
                "description": "batch: the actions to run in order in one call, each spelled exactly as alone, positions \
                    in the same screenshot's pixels; a wait_for or sound_wait step gives timeout_ms. Stops at the first \
                    refusal; the tool looks once, after."
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

pub(crate) fn dispatch(
    ctx: &ToolContext,
    enforcer: Option<&PermissionEnforcer>,
    name: &str,
    input: &Value,
) -> Option<Result<String, ToolError>> {
    match name {
        "Computer" => Some(
            maybe_enforce_permission_check(enforcer, name, input).and_then(|()| {
                let road = ComputerRoad::from_path()?;
                run_computer(input, ctx, &road)
            }),
        ),
        _ => None,
    }
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
        std::env::split_paths(&path)
            .map(|dir| dir.join(COMPUTER_SHIM))
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
    input.coordinate = input.coordinate.map(|pixel| frame.to_point(pixel));
    input.start_coordinate = input.start_coordinate.map(|pixel| frame.to_point(pixel));
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

/// The command line an action means. Pure: the table a test pins, in three
/// families — looking and input, apps and the system, the meaning layer.
pub(crate) fn argv_for(input: &ComputerInput) -> Result<Vec<String>, ToolError> {
    let action = input.action.trim();
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
        || argv_persons_turn(action, input, &mut argv)?;
    if !handled {
        return Err(ToolError::InvalidInput(format!(
            "unknown Computer action {action:?}; one of {}",
            action_names(|_| true).join(", ")
        )));
    }
    if presses(action) {
        if let Some(kind) = input.confirming.as_deref() {
            push(&mut argv, &["--confirming", kind]);
        }
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
        "screenshot" => push(argv, &["screenshot"]),
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
            push(argv, &[
                "mouse-drag", "--from-x", &number(from_x), "--from-y", &number(from_y), "--to-x", &number(to_x), "--to-y", &number(to_y),
            ]);
        }
        "scroll" => {
            let [x, y] = point(input.coordinate, action, "coordinate")?;
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
            push(argv, &["key", "--key", chord]);
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
            push(argv, &["type", "--text", text]);
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
        "list_windows" => push(argv, &["list-all-windows"]),
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

/// The person's turn — a code, a CAPTCHA, a password field: they do it, and
/// the call waits until they say so (or its `timeout_ms`).
fn argv_persons_turn(action: &str, input: &ComputerInput, argv: &mut Vec<String>) -> Result<bool, ToolError> {
    if !methods(action).any(|method| method == ComputerMethod::Handoff) {
        return Ok(false);
    }
    push(argv, &[ComputerMethod::Handoff.verb_name(), "--reason", need(input.text.as_deref(), action, "text")?]);
    if let Some(ms) = input.timeout_ms {
        push(argv, &["--timeout-ms", &ms.to_string()]);
    }
    Ok(true)
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
        Some(envelope) => Ok(envelope),
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
    let staged = crate::file_tools::run_read_image(
        &serde_json::from_value(json!({ "path": path })).ok()?,
        ctx,
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
    let observation = answer.get("result").and_then(ShotFrame::from_answer).map(|frame| ComputerObservation {
        app: app.map(str::to_string),
        window_id,
        frame,
        marked_look: answer.pointer(&format!("/result/marks/{LOOK_ID_KEY}")).and_then(Value::as_str).map(str::to_string),
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
fn look(road: &ComputerRoad, ctx: &ToolContext, words: &[&str]) -> Result<Value, LookError> {
    let argv = look_argv(words, looks_carry_marks());
    let mut answer = call_shim(road, &as_viewer(argv, ctx), ctx.cwd.as_deref()).map_err(LookError::Road)?;
    if let Some(refused) = refusal(&answer) {
        return Err(LookError::Refused(refused));
    }
    let changed = if words.contains(&LOOK_DIFF_FLAG) {
        answer.pointer("/result/changed").and_then(Value::as_array).map(|regions| !regions.is_empty())
    } else {
        None
    };
    let staged = present_observation(&mut answer, ctx, None, None, words.contains(&LOOK_DIFF_FLAG));
    let mut screen = shown(answer.get("result").cloned().unwrap_or(Value::Null), ctx.computer_frame());
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
    Ok(json!({
        "staged": staged.is_some(),
        "changed": changed,
        "screen": screen,
    }))
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
/// `changed: false`.
fn look_after(road: &ComputerRoad, ctx: &ToolContext, settle: bool) -> Value {
    let words = if settle { LOOK_AFTER_ARGV } else { FRAME_LOOK_ARGV };
    let mut answer = look(road, ctx, words).unwrap_or_else(|error| json!({ "error": ToolError::from(error).to_string() }));
    if !settle {
        answer["waited_for_settle"] = Value::Bool(false);
    }
    answer
}

/// The look that learns the frame, when the model names a place (or asks for
/// an answer that will) before this process has shown it a picture — so its
/// pixels and the hand's points agree. A screen that cannot be seen (no
/// screen recording) is still one the hand can move: the frame is then the
/// screen itself, and what goes in and what comes out stay in points.
fn first_look(inputs: &[&ComputerInput], ctx: &ToolContext, road: &ComputerRoad) -> Result<Option<Value>, ToolError> {
    if ctx.computer_frame().is_some() || !inputs.iter().any(|input| needs_a_frame(input)) {
        return Ok(None);
    }
    match look(road, ctx, FRAME_LOOK_ARGV) {
        Ok(seen) if seen["staged"] != true || ctx.computer_frame().is_none() => Err(ToolError::Execution(
            "the first desktop picture could not be shown with its coordinate frame; observe again before acting in picture pixels".into(),
        )),
        Ok(seen) => Ok(Some(seen)),
        Err(LookError::Refused(refused)) if refused.code == error_code::PERMISSION_DENIED => {
            ctx.set_computer_frame(ShotFrame::UNIT);
            Ok(Some(json!({ "blind": true, "why": refused.message })))
        }
        Err(error) => Err(error.into()),
    }
}

/// A batch's command line: each step spelled exactly as the same action
/// alone, in the same frame — the core's batch verb and flag, the window's
/// checks.
fn batch_argv(steps: &[ComputerInput], frame: ShotFrame) -> Result<Vec<String>, ToolError> {
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

/// One batch: checked whole by the core before any look is paid, one trip
/// down the shim, one look after — and each step answered under the model's
/// own action name, in its picture's pixels.
fn run_computer_batch(input: ComputerInput, ctx: &ToolContext, road: &ComputerRoad) -> Result<String, ToolError> {
    let settle = input.settle.unwrap_or(SETTLE_AFTER_ACT);
    let steps = input
        .steps
        .ok_or_else(|| ToolError::InvalidInput("`batch` needs `steps`: the actions to run in order".into()))?;
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
    let acted = steps
        .iter()
        .take(ran)
        .any(|step| acts(step.action.trim()));
    let looked_after = (LOOK_AFTER_ACT && acted).then(|| look_after(road, ctx, settle));
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

pub(crate) fn run_computer(input: &Value, ctx: &ToolContext, road: &ComputerRoad) -> Result<String, ToolError> {
    let input: ComputerInput = from_value(input)?;
    let settle = input.settle.unwrap_or(SETTLE_AFTER_ACT);
    let action = input.action.trim().to_string();
    if action == BATCH_ACTION {
        return run_computer_batch(input, ctx, road);
    }
    if input.steps.is_some() {
        return Err(ToolError::InvalidInput(format!("`steps` belongs to `batch`, not `{action}`")));
    }
    // A command the tool cannot spell costs no look.
    argv_for(&input)?;
    let first_look = first_look(&[&input], ctx, road)?;
    let frame = ctx.computer_frame();
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
    if let Some(refused) = refusal(&answer) {
        return Err(refused.into());
    }
    let staged = if LOOKS.contains(&action.as_str()) {
        let (app, window_id) = if action == "observe" { (input.app.as_deref(), input.window_id) } else { (None, None) };
        present_observation(&mut answer, ctx, app, window_id, action == "observe")
    } else {
        let staged = stage_screenshot(&mut answer, ctx);
        if staged.is_some() {
            ctx.set_computer_observation(None); // A zoom or action image changed what the model last saw.
        }
        staged
    };
    let mut result = shown(answer.get("result").cloned().unwrap_or(Value::Null), ctx.computer_frame());
    // A picture of its own (a screenshot, a zoom) carries no numbers: after it
    // there is no marked look to click by until one is shown again.
    if staged.is_some() {
        present_marks(&mut result, ctx);
    }
    // One look after the act, so the model sees what it did.
    let looked_after = (LOOK_AFTER_ACT && acts(&action)).then(|| look_after(road, ctx, settle));
    to_pretty_json(json!({
        "ok": true,
        "action": action,
        "result": result,
        "screenshot": staged,
        "first_look": first_look,
        "looked_after": looked_after,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(fields: Value) -> ComputerInput {
        serde_json::from_value(fields).expect("input")
    }

    fn screen_with_look(look: &str) -> Value {
        json!({ "marks": { "lookId": look, "items": [] } })
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
        let ctx = ToolContext::new();
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

    /// A fake shim on disk, answering like the window: a look carries a frame
    /// at half scale and what changed (read from a file the test flips), and
    /// every answer carries a position in points.
    #[cfg(unix)]
    fn fake_shim(dir: &Path) -> (ComputerRoad, PathBuf) {
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
[ "$verb" = batch ] && { printf '%s' "$3" > '@DIR@/batch-commands'; cat '@DIR@/batch'; echo; exit 0; }
case "$verb" in
  screenshot|observe)
    cp '@PNG@' '@PNG@'.$$
    look=',"screenshot":{"path":"@PNG@.'$$'","width":640,"height":415,"scale":0.5},"origin":{"x":0,"y":0},"changed":'"$(cat '@CHANGED@')" ;;
  *) look='' ;;
esac
printf '%s
' '{"ok":true,"result":{"verb":"'"$verb"'","said":"'"$*"'","cursor":{"x":200,"y":80}'"$look"'}}'
"#
        .replace("@PNG@", &png.display().to_string())
        .replace("@CHANGED@", &changed.display().to_string())
        .replace("@DIR@", &dir.display().to_string());
        std::fs::write(&program, script).unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        (ComputerRoad { program }, changed)
    }

    #[cfg(unix)]
    fn staged(ctx: &ToolContext) -> usize {
        ctx.image_sink.lock().unwrap_or_else(std::sync::PoisonError::into_inner).len()
    }

    #[cfg(unix)]
    #[test]
    fn repeated_observe_omits_only_a_picture_already_shown_from_the_same_target() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (road, changed) = fake_shim(dir.path());
        let ctx = ToolContext::new();
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
        let ctx = ToolContext::new();
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
        let ctx = ToolContext::new();
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
        let ctx = ToolContext::new();
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
        let ctx = ToolContext::new();
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
            let ctx = ToolContext::new();
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
        let ctx = ToolContext::new();
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
        let ctx = ToolContext::new();
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
        let fresh = ToolContext::new();
        let first: Value = serde_json::from_str(
            &run_computer(&json!({ "action": "left_click", "coordinate": [100, 40] }), &fresh, &road).expect("an answer"),
        )
        .unwrap();
        assert_eq!(first["first_look"]["staged"], true, "{first}");
        assert!(first["result"]["said"].as_str().unwrap().starts_with("mouse-click --x 200 --y 80 "), "{first}");
        // A process whose first action answers places is shown a picture
        // first too, so the place it answers is in the pixels it will click.
        let finder = ToolContext::new();
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
        let blind_ctx = ToolContext::new();
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
        let stopped = run_computer(&json!({ "action": "left_click", "coordinate": [1, 1] }), &ToolContext::new(), &blind_road)
            .expect_err("no frame, no act");
        assert!(stopped.to_string().contains("action_timeout"), "{stopped}");
        let untouched: Value = serde_json::from_str(
            &run_computer(&json!({ "action": "type", "text": "x" }), &ToolContext::new(), &road).expect("an answer"),
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
    fn calls(dir: &Path) -> Vec<String> {
        std::fs::read_to_string(dir.join("calls")).unwrap_or_default().lines().map(str::to_string).collect()
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
        let singly = ToolContext::new();
        singly.set_computer_frame(half((0.0, 0.0)));
        for step in five.as_array().unwrap() {
            run_computer(step, &singly, &road).expect("an answer");
        }
        assert_eq!(calls(dir.path()).len(), 10, "{:?}", calls(dir.path()));
        assert_eq!(staged(&singly), 5);

        std::fs::remove_file(dir.path().join("calls")).unwrap();
        let steps: Vec<Value> = (1..=5).map(|n| json!({ "n": n, "verb": "x", "ok": true, "ms": 1, "result": { "cursor": { "x": 20, "y": 20 } } })).collect();
        std::fs::write(dir.path().join("batch"), json!({ "ok": true, "result": { "ran": 5, "of": 5, "steps": steps } }).to_string()).unwrap();
        let batched = ToolContext::new();
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
        let fresh = ToolContext::new();
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
        let ctx = ToolContext::new();
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
            let error = run_computer(&json!({ "action": "batch", "steps": steps }), &ToolContext::new(), &road).expect_err("refused");
            assert!(matches!(error, ToolError::InvalidInput(_)), "{error}");
        }
        assert!(calls(dir.path()).is_empty(), "no trip, no look: {:?}", calls(dir.path()));
        let stray = run_computer(&json!({ "action": "key", "text": "a", "steps": [] }), &ToolContext::new(), &road).expect_err("stray steps");
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
                "last": 3, "after": 2, "min_confidence": 0.5, "until": "quiet"
            })
        };
        let argv = |fields: &Value| argv_for(&input(fields.clone())).map_err(|error| error.to_string());
        let mut read = std::collections::BTreeSet::from(["action".to_string()]);
        for action in action_names(|action| methods(action).any(ComputerMethod::batches)) {
            let whole = full(action);
            for field in whole.as_object().unwrap().keys().filter(|field| *field != "action") {
                let mut without = whole.clone();
                without.as_object_mut().unwrap().remove(field);
                if argv(&whole) != argv(&without) {
                    read.insert(field.clone());
                }
            }
        }
        let listed: std::collections::BTreeSet<String> = STEP_FIELDS.iter().map(|field| (*field).to_string()).collect();
        assert_eq!(listed, read, "STEP_FIELDS is what the step actions read");
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
    /// with app, a sentence and the plan-it-whole clause) ~40.
    #[test]
    fn the_computer_schema_is_measured() {
        const CEILING_TOKENS: usize = 1_685;
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
