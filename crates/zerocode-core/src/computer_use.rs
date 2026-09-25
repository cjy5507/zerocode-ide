//! Computer Use's wire vocabulary, guarded CLI grammar, and agent shim.
//!
//! The native provider lives in the desktop shell, but every caller must agree
//! on the command names and parameter shapes before anything touches another
//! app. This module is that shared boundary: a launched agent's
//! `zerocode-computer` shim sends argv over the authenticated hook bridge, the
//! window parses it here, and the macOS helper receives only typed JSON.

use std::collections::BTreeMap;
use std::num::NonZeroU32;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::agent_teams::ARGV_SEPARATOR;

pub const COMPUTER_USE_SKILL_NAME: &str = "computer-use";
/// The program every Computer Use caller runs — the pane's shim, the name a
/// recipe line starts with, and zo's road to the window.
pub const COMPUTER_CLI: &str = "zerocode-computer";
// Protocol 2 requires the window-supplied guard table and its budget shape.
pub const COMPUTER_USE_PROTOCOL_VERSION: u64 = 2;
pub const COMPUTER_USE_DEADLINE_SECONDS: u64 = 65;
/// The environment variable an automation run's shell carries when the run
/// leaves evidence: the folder its frames and step log go to. Every shim
/// forwards it as [`RUN_EVIDENCE_HEADER`], so the window can write the frame
/// after each action itself instead of trusting the agent to.
pub const RUN_EVIDENCE_DIR_ENV: &str = "ZEROCODE_RUN_EVIDENCE_DIR";
/// The request header the shims forward [`RUN_EVIDENCE_DIR_ENV`] in.
pub const RUN_EVIDENCE_HEADER: &str = "x-zerocode-run-evidence";
/// The request header the Computer Use door says where its caller stands in:
/// the shell's working directory, which is the workspace the Jev door consents
/// by when a recipe walk it asked for stops and is judged
/// (docs/design/jev-settings-20260917.md §3). Spelled in hex
/// ([`cwd_header_value`]), because a folder may be named anything: the POSIX
/// door writes its headers into a curl config file, where a quote or a
/// newline in a name would close the value or begin an option line of its
/// own, and PowerShell 7 refuses a header that is not ASCII.
pub const CWD_HEADER: &str = "x-zerocode-cwd";

/// The Computer Use verbs whose request says where its caller stands
/// ([`CWD_HEADER`]): the ones the window judges by that folder. A recipe walk
/// that stops is judged for the workspace it was asked from; no other verb
/// reads the folder, so no other call spends two processes spelling it.
pub const CWD_VERBS: &[&str] = &[
    ComputerMethod::RecipeRun.verb_name(),
    ComputerMethod::Walk.verb_name(),
];

/// [`CWD_HEADER`]'s value for `cwd` — its UTF-8 bytes as lowercase hex, the
/// spelling the door's `od` writes.
#[must_use]
pub fn cwd_header_value(cwd: &str) -> String {
    cwd.bytes().map(|byte| format!("{byte:02x}")).collect()
}

/// The directory a [`CWD_HEADER`] value spells: pairs of hex digits, in
/// either case, of an absolute UTF-8 path. Anything else names no directory
/// and answers `None` — nothing is guessed from it, and a relative path is
/// never resolved against the window's own.
#[must_use]
pub fn cwd_from_header(value: &str) -> Option<String> {
    let digits = value.as_bytes();
    if digits.is_empty() || !digits.len().is_multiple_of(2) {
        return None;
    }
    let bytes = digits
        .chunks_exact(2)
        .map(|pair| {
            let high = char::from(pair[0]).to_digit(16)?;
            let low = char::from(pair[1]).to_digit(16)?;
            u8::try_from(high << 4 | low).ok()
        })
        .collect::<Option<Vec<u8>>>()?;
    String::from_utf8(bytes)
        .ok()
        .filter(|cwd| std::path::Path::new(cwd).is_absolute())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ComputerPermissionId {
    Accessibility,
    Screenshots,
}

impl ComputerPermissionId {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Accessibility => "accessibility",
            Self::Screenshots => "screenshots",
        }
    }
}

impl std::str::FromStr for ComputerPermissionId {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "accessibility" => Ok(Self::Accessibility),
            "screenshots" => Ok(Self::Screenshots),
            _ => Err("--id must be accessibility or screenshots".into()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ComputerPermissionStatus {
    Granted,
    NotGranted,
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComputerPermissionState {
    pub id: ComputerPermissionId,
    pub status: ComputerPermissionStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ComputerSigningIdentity {
    Local,
    Adhoc,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComputerPermissionReport {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<ComputerSigningIdentity>,
    pub platform: String,
    pub helper_app_path: Option<String>,
    pub helper_unavailable_reason: Option<String>,
    pub permissions: Vec<ComputerPermissionState>,
    /// The System Settings row each permission is judged on. macOS judges
    /// Accessibility on the calling process (the helper) but Screen
    /// Recording on the responsible process (the app itself), so the row a
    /// person must turn on — and the bundle a reset names — differs per
    /// permission.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub judged_rows: Vec<ComputerPermissionRow>,
}

/// One System Settings row: which bundle a permission list judges, what the
/// list calls it, and the path to drop in when the row is missing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComputerPermissionRow {
    pub id: ComputerPermissionId,
    pub bundle_id: String,
    pub name: String,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComputerPermissionSetup {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<ComputerSigningIdentity>,
    pub platform: String,
    pub helper_app_path: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub judged_rows: Vec<ComputerPermissionRow>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permission_id: Option<ComputerPermissionId>,
    #[serde(default)]
    pub requested_os: bool,
    pub opened_settings: bool,
    pub launched_helper: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permissions: Option<Vec<ComputerPermissionState>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_step: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComputerPermissionReset {
    #[serde(flatten)]
    pub report: ComputerPermissionReport,
    /// The bundles whose rows were reset, one per permission that was missing.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bundle_ids: Vec<String>,
}

/// The longest a `wait` may hold the agent's turn: a person's pause, not a
/// sleep — anything longer is a scheduler's job.
pub const COMPUTER_WAIT_MAX_MS: u64 = 30_000;

/// The longest a `run` may hold the agent's turn, and the most of its output
/// the answer carries — a person's glance at a terminal, not a log archive.
pub const COMPUTER_RUN_MAX_MS: u64 = 60_000;
pub const COMPUTER_RUN_MAX_BYTES: usize = 65_536;

/// A `launch`: how long the helper waits for the program to start, and — when
/// not told — for its first window to be ready (the helper's own table says
/// the same numbers; a source contract holds them together).
pub const COMPUTER_LAUNCH_PROCESS_MS: u64 = 10_000;
pub const COMPUTER_LAUNCH_WAIT_READY_MS: u64 = 5_000;
/// How long the helper watches a `quit` or an `activate` take hold before it
/// answers what it saw (its own table says the same; a source contract holds
/// them together).
pub const COMPUTER_APP_SETTLE_MS: u64 = 5_000;

/// How long a `run` is allowed, and whether the table shortened it.
#[must_use]
pub const fn desktop_run_ms(asked: Option<u64>) -> (u64, bool) {
    HOLD_RUN.allowed_ms(asked)
}

/// The head of a program's output the answer keeps, and whether it was cut.
#[must_use]
pub fn desktop_run_output(bytes: &[u8]) -> (String, bool) {
    let cut = bytes.len() > COMPUTER_RUN_MAX_BYTES;
    let kept = if cut {
        &bytes[..COMPUTER_RUN_MAX_BYTES]
    } else {
        bytes
    };
    (String::from_utf8_lossy(kept).into_owned(), cut)
}

/// The kinds a person confirms by default and the words their controls
/// carry live in [`crate::guarded`], the one table a walk by judgment reads
/// too (t-6187); the gate reads them from there.
pub use crate::guarded::{ConfirmKind, confirm_kind_of};

/// How long the window waits for the person's answer before the press is
/// refused as unanswered.
pub const COMPUTER_CONFIRM_TIMEOUT_MS: u64 = 120_000;

/// The helper's `confirmation_required` message, `<kind>: <label>`, read back.
#[must_use]
pub fn parse_confirmation_required(message: &str) -> Option<(ConfirmKind, String)> {
    let (kind, label) = message.split_once(':')?;
    Some((ConfirmKind::parse(kind)?, label.trim().to_string()))
}

/// A recipe's file name from the name a person gave it: lowercase letters,
/// digits and single hyphens — the same word on every disk and in every URL.
#[must_use]
pub fn recipe_slug(name: &str) -> String {
    let mut slug = String::new();
    let mut hyphen = false;
    for glyph in name.trim().chars() {
        if glyph.is_alphanumeric() {
            for lower in glyph.to_lowercase() {
                slug.push(lower);
            }
            hyphen = false;
        } else if !hyphen && !slug.is_empty() {
            slug.push('-');
            hyphen = true;
        }
    }
    slug.trim_end_matches('-').to_string()
}

/// How many recent steps a recipe keeps when not told.
pub const RECIPE_STEPS: usize = 200;

/// The eyes (§7.1): what changed between two looks is reported as
/// rectangles over a grid of cells this many pixels wide, at most this many
/// of them (largest first).
pub const OBSERVE_DIFF_CELL: u32 = 24;
pub const OBSERVE_DIFF_RECTS: usize = 20;
/// How many last frames `observe --diff` remembers at once — one per app
/// window or desktop place it has looked at, oldest place forgotten first.
pub const OBSERVE_FRAMES_MAX: usize = 8;
/// The person's turn (§7.4): how long a handoff waits for them.
pub const COMPUTER_HANDOFF_TIMEOUT_MS: u64 = 600_000;
/// Stuck (§7.4): the same action this many times with nothing changing on
/// the screen is a loop, not progress.
pub const COMPUTER_STUCK_REPEATS: u32 = 2;

/// QA's screen comparison (§4): how far a pixel may drift before it counts
/// as different, and how much of a screen may differ before the comparison
/// fails, when the caller sets no bar.
pub const COMPUTER_COMPARE_PIXEL_DELTA: u8 = 32;
/// What counts as a changed pixel for `observe --diff` — "did my act change
/// the screen" — far finer than QA's drift allowance: a focus ring or a
/// greyed checkbox moves a channel by less than 32.
pub const OBSERVE_DIFF_PIXEL_DELTA: u8 = 8;
/// How long a surface is given to repaint after an action before it is
/// looked at — the evidence frame after a step, and zo's look after an act —
/// and how often a look that found nothing changed yet looks again inside it.
pub const COMPUTER_SETTLE_MS: u64 = 350;
pub const COMPUTER_SETTLE_POLL_MS: u64 = 50;
pub const COMPUTER_COMPARE_MAX_DIFF: f64 = 0.01;

/// The marks (§7.1, `computer_use_protocol::marks`): numbered badges on the
/// controls of one window, drawn on the picture the model reads, and
/// `click --mark N --look L` that presses that control through the element
/// path, pinned by what the look saw. The most marks one look carries: two
/// digits, so no badge is wider than two glyphs and the legend stays under a
/// hundred lines.
pub const MARK_CAP: usize = 99;
/// The roles a person presses or types into, in the shared AX spelling both
/// providers use. The order is the tie-break when two marks would name one
/// target: a field outranks the cell it fills, a control the row it sits in.
pub const MARK_ROLES: &[&str] = &[
    "AXTextField",
    "AXSearchField",
    "AXTextArea",
    "AXComboBox",
    "AXButton",
    "AXMenuButton",
    "AXPopUpButton",
    "AXCheckBox",
    "AXRadioButton",
    "AXSlider",
    "AXIncrementor",
    "AXDisclosureTriangle",
    "AXColorWell",
    "AXLink",
    "AXTab",
    "AXMenuItem",
    "AXMenuBarItem",
    "AXCell",
    "AXRow",
    "AXOutlineRow",
];
/// Where typing goes: editable text. A plain click on one focuses it — its
/// confirm action is a Return, not a click (a combo box is not here: a web
/// select-only combobox opens on its press) — and it may fill its window and
/// still be a target.
pub const TEXT_ENTRY_ROLES: &[&str] = &["AXTextField", "AXSearchField", "AXTextArea"];
/// The containers that clip what is inside them: an element's visible part
/// is its frame cut by every one of these above it, and one whose centre is
/// cut off is not marked (a row scrolled under the header, a list's rows
/// below the fold).
pub const MARK_CLIP_ROLES: &[&str] = &["AXScrollArea"];
/// How many parents a hit-test's answer, or the marked control, is walked to
/// meet the other: a mark is pressed only when what is on top at its centre
/// is the control, inside it, or around it.
pub const MARK_HIT_DEPTH: usize = 32;
/// How many marked looks are kept at once: an agent clicks by its latest; an
/// id this far behind is refused and looked again.
pub const MARK_LOOKS_KEPT: usize = 8;
/// The presses the element path tries, in order: the macOS helper's actions
/// (`performClickAction`, a source contract holds it) and the Windows
/// provider's UIA patterns (which read this table). An element of any role
/// that offers one of them, bare, is something a click can press.
pub const MACOS_PRESS_ACTIONS: &[&str] = &["AXPress", "AXConfirm", "AXOpen"];
pub const WINDOWS_PRESS_PATTERNS: &[&str] = &["Invoke", "Toggle", "Select"];
/// Containers and chrome: their press is nobody's target, and their centre
/// sits on everything inside them.
pub const MARK_NEVER_ROLES: &[&str] = &[
    "AXWindow",
    "AXWebArea",
    "AXScrollArea",
    "AXScrollBar",
    "AXSplitGroup",
    "AXSplitter",
];
/// The trait words (both providers write them) whose control a press cannot
/// work: its number would waste a badge.
pub const MARK_SKIP_TRAITS: &[&str] = &["disabled"];
/// AppKit's smallest controls are about 10 points; under 6 on a side is a
/// hairline, a splitter or a focus ring, not something a person hits.
pub const MARK_MIN_SIDE_POINTS: f64 = 6.0;
/// A pressable element over half its window is a canvas or a page: its centre
/// is nobody's target. A text-entry role is exempt — a document's text area
/// fills its window and is where the typing goes.
pub const MARK_MAX_WINDOW_SHARE: f64 = 0.5;
/// Two candidates whose rectangles overlap this much (intersection over
/// union) are one target: a link and the image it fills. A button inside a
/// row covers far less of the row, and both keep their numbers.
pub const MARK_SAME_TARGET_IOU: f64 = 0.75;
/// A label only has to tell two marks apart; the legend is paid on every look.
pub const MARK_LABEL_MAX_CHARS: usize = 40;
/// The digits, 5x7, one row per byte, most significant of the five bits on
/// the left (the HD44780 font). Every pair differs in at least three pixels
/// (a test holds it), and the window draws the same pixels on every platform.
pub const MARK_DIGIT_GLYPHS: [[u8; 7]; 10] = [
    [0x0E, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0E],
    [0x04, 0x0C, 0x04, 0x04, 0x04, 0x04, 0x0E],
    [0x0E, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1F],
    [0x1F, 0x02, 0x04, 0x02, 0x01, 0x11, 0x0E],
    [0x02, 0x06, 0x0A, 0x12, 0x1F, 0x02, 0x02],
    [0x1F, 0x10, 0x1E, 0x01, 0x01, 0x11, 0x0E],
    [0x06, 0x08, 0x10, 0x1E, 0x11, 0x11, 0x0E],
    [0x1F, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08],
    [0x0E, 0x11, 0x11, 0x0E, 0x11, 0x11, 0x0E],
    [0x0E, 0x11, 0x11, 0x0F, 0x01, 0x02, 0x0C],
];
/// Glyph cells, in font pixels.
pub const MARK_GLYPH_COLUMNS: u32 = 5;
pub const MARK_GLYPH_ROWS: u32 = 7;
/// Doubled, a digit is 10x14 px — body text's height in a 1280-px picture;
/// an integer scale keeps every stroke even.
pub const MARK_GLYPH_SCALE: u32 = 2;
/// One font pixel at scale 2 between two digits, so "11" reads as two.
pub const MARK_GLYPH_GAP_PX: u32 = 2;
/// Fill between the ink and the border, and the border itself: the fill alone
/// on white is 1.4:1, the ink border draws the badge's edge there. Together
/// they make a badge 18 px tall and 14 or 26 px wide.
pub const MARK_BADGE_PAD_PX: u32 = 1;
pub const MARK_BADGE_BORDER_PX: u32 = 1;
/// A one-pixel outline in the fill ties each badge to its element (above all
/// an outside one) without hiding the element's own edge or words.
pub const MARK_OUTLINE_PX: u32 = 1;
/// A badge sits on its control only if it hides at most this share of it:
/// rows and fields keep their words, small buttons wear theirs outside.
pub const MARK_INSIDE_MAX_SHARE: f64 = 0.25;
/// Where a badge may go, tried in order, the first free one taken: inside the
/// control's top-left corner when it may sit inside (where a reader looks
/// first; top-right and then top-centre rescue a row that starts with a
/// checkbox and ends with a star), outside it otherwise. A badge with no
/// free anchor is left out and counted, never drawn over another.
pub const MARK_ANCHORS_INSIDE_FIRST: &[crate::computer_use_protocol::marks::MarkAnchor] = {
    use crate::computer_use_protocol::marks::MarkAnchor::{
        InsideTopCenter, InsideTopLeft, InsideTopRight, OutsideAboveLeft, OutsideBelowLeft,
        OutsideLeft,
    };
    &[
        InsideTopLeft,
        InsideTopRight,
        InsideTopCenter,
        OutsideAboveLeft,
        OutsideLeft,
        OutsideBelowLeft,
    ]
};
pub const MARK_ANCHORS_OUTSIDE_FIRST: &[crate::computer_use_protocol::marks::MarkAnchor] = {
    use crate::computer_use_protocol::marks::MarkAnchor::{
        InsideTopCenter, InsideTopLeft, InsideTopRight, OutsideAboveLeft, OutsideBelowLeft,
        OutsideLeft, OutsideRightTop,
    };
    &[
        OutsideAboveLeft,
        OutsideLeft,
        OutsideRightTop,
        OutsideBelowLeft,
        InsideTopLeft,
        InsideTopRight,
        InsideTopCenter,
    ]
};
/// Badge ink and fill: ink on fill is 14.9:1, fill on black 14.9:1, the ink
/// border on white 21:1 — readable on dark and light apps. Fully opaque, so
/// nothing underneath changes a digit.
pub const MARK_FILL_RGBA: [u8; 4] = [255, 214, 10, 255];
pub const MARK_INK_RGBA: [u8; 4] = [0, 0, 0, 255];
/// How far each edge of a marked control may drift before a click by its
/// number is refused: the two units `windowFramesMatch` already trusts.
/// A list shifted by one row moves by a whole row, far past it.
pub const MARK_PIN_TOLERANCE_POINTS: f64 = 2.0;
/// The helper namespace desktop marked looks and mark clicks are filed in, so
/// a marked look never replaces the snapshot another agent's
/// `--element-index` is checked against.
pub const MARK_SNAPSHOT_SESSION: &str = "zerocode-marks";
/// Whether zo's looks carry marks (`observe --marks`) by default. Off: a
/// marked look costs little (docs/design/computer-use-bench.md §6), but
/// whether numbers make the model better is the bench's to say — its
/// `marks` config turns them on for one side of the A/B
/// ([`COMPUTER_MARKS_ENV`]), and the default follows the table.
pub const COMPUTER_LOOKS_CARRY_MARKS: bool = false;
/// A zo process's own answer to [`COMPUTER_LOOKS_CARRY_MARKS`]: `1`/`on`
/// numbers its looks, `0`/`off` does not, unset or anything else keeps the
/// default. The bench runs both sides of the A/B on one build with it.
pub const COMPUTER_MARKS_ENV: &str = "ZO_COMPUTER_MARKS";

/// Whether a zo process's looks carry marks, from its raw
/// [`COMPUTER_MARKS_ENV`] value.
#[must_use]
pub fn looks_carry_marks(asked: Option<&str>) -> bool {
    match asked
        .map(|value| value.trim().to_ascii_lowercase())
        .as_deref()
    {
        Some("1" | "on" | "true" | "yes") => true,
        Some("0" | "off" | "false" | "no") => false,
        _ => COMPUTER_LOOKS_CARRY_MARKS,
    }
}

/// Look until the screen has settled after an act: a look that saw a change
/// (or could not tell) answers at once; one that saw nothing looks again every
/// `COMPUTER_SETTLE_POLL_MS` until `COMPUTER_SETTLE_MS` has passed, and then
/// answers what it saw. A look that failed answers at once. zo's look after
/// an act and a recipe's landing both wait this one way.
pub fn look_until_settled<T, E>(
    mut look: impl FnMut() -> Result<T, E>,
    changed: impl Fn(&T) -> Option<bool>,
    elapsed: impl Fn() -> std::time::Duration,
    mut pause: impl FnMut(std::time::Duration),
) -> Result<T, E> {
    let settle = std::time::Duration::from_millis(COMPUTER_SETTLE_MS);
    loop {
        let seen = look()?;
        if changed(&seen) != Some(false) || elapsed() >= settle {
            return Ok(seen);
        }
        pause(std::time::Duration::from_millis(COMPUTER_SETTLE_POLL_MS));
    }
}

/// The comparison's verdict: the share of pixels that differ, and whether
/// it is within the bar.
#[must_use]
pub fn compare_verdict(different: usize, total: usize, max_diff: Option<f64>) -> (f64, bool) {
    let bar = max_diff.unwrap_or(COMPUTER_COMPARE_MAX_DIFF);
    if total == 0 {
        return (1.0, false);
    }
    #[allow(clippy::cast_precision_loss)]
    let ratio = different as f64 / total as f64;
    (ratio, ratio <= bar)
}

/// The one hand that stops the operator from anywhere: the chord the helper
/// listens for on the whole desktop, spelled the way `key` spells chords.
pub const COMPUTER_STOP_HOTKEY: &str = "control+option+escape";
/// Why the operator is stopped, in the words the window and the helper write
/// (the helper's `StopReason`, a source contract holds them): the person's
/// chord, the window's stop button, the window's signal that carries either
/// stop to the helper, an agent's `stop`, the session's budget.
pub const STOP_REASON_HOTKEY: &str = "hotkey";
pub const STOP_REASON_WINDOW: &str = "window";
pub const STOP_REASON_SIGNAL: &str = "signal";
pub const STOP_REASON_REQUEST: &str = "request";
pub const STOP_REASON_SESSION_BUDGET: &str = "budget:session";
/// The stops a person made: only a person lifts them — the window's resume —
/// never an agent's `resume`, or one hand on the operator would be the
/// operator's own (review, 2026-09-11).
pub const PERSONS_STOP_REASONS: &[&str] = &[STOP_REASON_HOTKEY, STOP_REASON_WINDOW];

/// Whether a stop is the person's to lift.
#[must_use]
pub fn persons_stop(reason: &str) -> bool {
    PERSONS_STOP_REASONS.contains(&reason)
}
/// How fast the hand may go (§1.4), in the one shape every reader takes —
/// the helper that counts, `status`, the usage and the skill.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComputerPace {
    /// No rate of the operator's own: an action goes as soon as the stop and
    /// the session count admit it and the one before it is done.
    Unlimited,
    /// A person's pace: `burst` actions back to back, then `per_second` —
    /// past a burst the hand waits for the pace instead of stopping.
    Paced {
        per_second: NonZeroU32,
        burst: NonZeroU32,
    },
}

/// The hand's pace: none. The person asked for no artificial speed cap —
/// 200 actions a minute is the floor, the fastest reliable hand the goal
/// (m-3903, 2026-09-13) — and a rate of the operator's own was a policy, not
/// anything the system asks for. What still bounds the hand is not a speed:
/// the stop (the chord, the window's button, `stop`) read before every
/// action and between every posted event, the person's last step
/// (`confirmation_required`), the session count below, each command's
/// deadline, and the OS's own event order (a click's inter-event pause, a
/// glide's frames), which live with the input they order and are not pace.
pub const COMPUTER_PACE: ComputerPace = ComputerPace::Unlimited;
/// The session count: the budget that stops the operator and says so
/// (`budget_exceeded`, the person's to lift) — a count, never a rate.
pub const COMPUTER_BUDGET_ACTIONS_PER_SESSION: NonZeroU32 = NonZeroU32::new(5_000).unwrap();

/// The words the pace table's `mode` is written in.
pub const PACE_MODE_UNLIMITED: &str = "unlimited";
pub const PACE_MODE_PACED: &str = "paced";

/// A pace as the window says it — `status`, and the helper's guard table:
/// the mode, and the rate and burst as numbers only when there are any (an
/// unlimited pace answers them `null`, never a stand-in number).
#[must_use]
pub fn pace_table(pace: ComputerPace) -> Value {
    match pace {
        ComputerPace::Unlimited => {
            json!({ "mode": PACE_MODE_UNLIMITED, "perSecond": null, "burst": null })
        }
        ComputerPace::Paced { per_second, burst } => json!({
            "mode": PACE_MODE_PACED,
            "perSecond": per_second.get(),
            "burst": burst.get(),
        }),
    }
}

/// A pace in the usage's and the skill's words.
#[must_use]
pub fn pace_words(pace: ComputerPace) -> String {
    match pace {
        ComputerPace::Unlimited => {
            "no speed cap — each action goes as soon as the one before it is done".into()
        }
        ComputerPace::Paced { per_second, burst } => {
            format!("{burst} actions back to back, then {per_second} a second")
        }
    }
}

/// Where the helper finds the guard table: the window launches it through
/// LaunchServices, which carries no environment but what `open --env` names.
pub const COMPUTER_GUARD_TABLE_ENV: &str = "ZEROCODE_COMPUTER_USE_GUARD_TABLE";

/// What the helper's guard counts by: the pace and the session count, from
/// the table above — the helper keeps no numbers of its own, and acts on
/// nothing when the table never reached it (`provider_incompatible`).
#[must_use]
pub fn guard_table() -> Map<String, Value> {
    let mut table = Map::new();
    table.insert("pace".into(), pace_table(COMPUTER_PACE));
    table.insert(
        "perSession".into(),
        json!(COMPUTER_BUDGET_ACTIONS_PER_SESSION.get()),
    );
    table
}

/// One judgement, many actions (§2.5). A batch is no longer tied to a pace:
/// its time is bounded by what its steps may wait together
/// (`COMPUTER_BATCH_MAX_WAIT_MS`) and by the bridge's deadline, which it
/// stops starting steps short of (`COMPUTER_BATCH_ANSWER_MARGIN_MS`); its
/// freshness by each step's own checks and the stop at the first refusal.
/// Its count shares the existing recipe sequence budget. This bounds one
/// plan and its result envelope without imposing an input rate.
pub const COMPUTER_BATCH_MAX_STEPS: usize = RECIPE_STEPS;
/// What a batch's steps may ask to wait, together — waits, holds, and the
/// budgets of `wait-for` and `sound-wait`: half the bridge's deadline, the
/// other half for the steps' own work and the answer's trip back.
pub const COMPUTER_BATCH_MAX_WAIT_MS: u64 = COMPUTER_USE_DEADLINE_SECONDS * 1_000 / 2;
/// The batch stops starting steps this long before the bridge's deadline, so
/// its answer — which steps ran — still reaches the caller.
pub const COMPUTER_BATCH_ANSWER_MARGIN_MS: u64 = 2_000;
/// The longest glide (`--steps`) a walked step — a batch's or a recipe's — may
/// ask for: a smooth move across a screen, not a crawl that would eat the
/// walk's time unseen.
pub const COMPUTER_WALK_MAX_GLIDE_STEPS: u64 = 60;
/// The flag a batch carries its steps in: a JSON array of command lines.
pub const BATCH_COMMANDS_FLAG: &str = "commands";
/// The flag every walked step — a batch's or a recipe's — carries where its
/// own table allows it: every step answers an envelope.
pub const WALKED_STEP_FLAGS: &[&str] = &["json"];
/// The flag a walked step carries besides when the walk keeps no frame of
/// it — a batch, a Flow at `verdict-only` or `off` (plan D10): an app step's
/// picture is not taken; such a walk is looked at once, after. A walk that
/// frames its steps (`full`) leaves it off: the helper's look after the act
/// rides the answer, and the one evidence writer keeps it as the step's
/// frame — a frame taken after a settle was outrun by the next act at a
/// replay's pace, one kept in twenty (measured 2026-09-15, t-4229).
pub const WALKED_STEP_UNFRAMED_FLAGS: &[&str] = &["no-screenshot"];
/// Flags no walked step may carry: a screen gone stale inside a batch — or a
/// recipe's, saved on another day — must not lift the guard on ZeroCode's own
/// window.
pub const WALKED_STEP_REFUSED_FLAGS: &[&str] = &["allow-self"];
/// Flags that name an element by its index in the tree the model last read —
/// or by its number on the marked look it last read: refused in a batch
/// after a step that rebuilds that tree (`rebuilds_the_tree`), and a
/// recipe's stop for a fresh look.
pub const ELEMENT_INDEX_FLAGS: &[&str] = &[
    "element-index",
    "from-element-index",
    "to-element-index",
    "mark",
];
/// Flags that name a click's control by what it reads — `find`'s matcher,
/// run on a fresh tree right before the press (§2.5). Unlike an index or a
/// mark they never go stale, so a batch step may name its control after the
/// steps before it changed the tree: a whole familiar plan in one call.
pub const QUERY_CLICK_KEYS: &[&str] = &["text", "label", "role"];
/// Result keys a later step makes stale: only the last step that answered
/// one keeps it — its element indexes are the fresh ones.
pub const BATCH_SUPERSEDED_KEYS: &[&str] = &["snapshot"];

/// The ears (§7.1). The system classifier hears in windows of this length,
/// one every hop: at 0.75 s its built-in 303-label model named a sound
/// 61–100 ms after it began, for 3–4% of one core; at its 3 s default it never
/// named a 1.5 s sound at all (measured on this machine, 2026-09-11).
pub const SOUND_WINDOW_SECONDS: f64 = 0.75;
pub const SOUND_HOP_SECONDS: f64 = 0.05;
/// The rate the sound is captured at — the classifier's own.
pub const SOUND_SAMPLE_RATE: u32 = 16_000;
/// A classification below this confidence is not an event.
pub const SOUND_MIN_CONFIDENCE: f64 = 0.5;
/// The events the listener keeps, newest last.
pub const SOUND_EVENTS_MAX: u32 = 256;
/// One label heard again within this is the same sound, not a new event.
pub const SOUND_DEBOUNCE_MS: u64 = 500;
/// A listener nobody has read from in this long stops itself.
pub const SOUND_IDLE_STOP_MS: u64 = 600_000;
/// Labels that are the absence of a sound, never an event.
pub const SOUND_IGNORED_LABELS: &[&str] = &["silence"];
/// How often `sound-wait` asks the helper what it heard; the longest it
/// waits is `wait-for`'s table.
pub const SOUND_WAIT_POLL_MS: u64 = 50;

/// What `listen-start` hands the helper: every number the ears use, from the
/// table above — the helper keeps none of its own.
#[must_use]
pub fn sound_table() -> Map<String, Value> {
    let mut table = Map::new();
    table.insert("windowSeconds".into(), json!(SOUND_WINDOW_SECONDS));
    table.insert("hopSeconds".into(), json!(SOUND_HOP_SECONDS));
    table.insert("sampleRate".into(), json!(SOUND_SAMPLE_RATE));
    table.insert("minConfidence".into(), json!(SOUND_MIN_CONFIDENCE));
    table.insert("eventsMax".into(), json!(SOUND_EVENTS_MAX));
    table.insert("debounceMs".into(), json!(SOUND_DEBOUNCE_MS));
    table.insert("idleStopMs".into(), json!(SOUND_IDLE_STOP_MS));
    table.insert("ignoredLabels".into(), json!(SOUND_IGNORED_LABELS));
    table
}

/// The continuous eye (§7.1, V1): while the desktop is being looked at, the
/// helper keeps a ScreenCaptureKit stream of the display, delivered at the
/// screenshot ladder's first rung (the GPU scales it), and numbers every
/// repaint with where it fell. A desktop look reads the newest frame instead
/// of capturing one; the look after an act, `watch` and an OCR `wait-for`
/// read the repaints instead of taking pictures to compare.
///
/// At most this many frames a second — the stream sends one only when
/// something repainted.
pub const EYE_FRAMES_PER_SECOND: u32 = 30;
/// The screen has settled once nothing the look cares about repainted for
/// this long: three frame intervals, and a transition paints every frame.
pub const EYE_QUIET_MS: u64 = 100;
/// The longest the look after an act waits for the screen to settle: a sheet
/// or a window animates for at most half a second, and motion past a second
/// is content — a spinner, a video — shown as it is (`settled: false`).
pub const EYE_SETTLE_MAX_MS: u64 = 1_000;
/// What was repainting this long before an act (or a watch) began is the
/// screen's own motion — a clock, a video, a spinner — not the act's doing.
pub const EYE_BACKGROUND_MS: u64 = 500;
/// How often the window asks the helper for repaints while it waits: under
/// one frame interval.
pub const EYE_POLL_MS: u64 = 25;
/// The repaints the helper keeps, newest last: 17 s of a screen repainting
/// every frame.
pub const EYE_CHANGES_KEPT: u32 = 512;
/// A stream nobody has read from in this long stops itself, and with it the
/// system's recording indicator.
pub const EYE_IDLE_STOP_MS: u64 = 30_000;
/// How long a start may wait for the stream's first frame.
pub const EYE_FIRST_FRAME_MS: u64 = 2_000;
/// What `watch` waits for: the first repaint, or the screen going still.
pub const WATCH_UNTIL: &[&str] = &["change", "quiet"];
/// How often the window reads a live reflex run's receipts, and with them its
/// status (t-9205). An empirical poll: at 200 actions a minute a second holds
/// about seven leaves against the helper's queue of
/// `reflex::LIMITS.max_expanded_actions`. It is not what keeps a receipt:
/// the helper keeps each one until the window has it on disk and says so, and
/// a queue the window left full ends the run instead of the receipts.
pub const REFLEX_COLLECT_MS: u64 = 1_000;
/// An OCR read of the desktop keeps its last reading where nothing
/// repainted since (§7.1): what did repaint is read again, snapped out to a
/// grid of this many points, widened by this margin and by every line it
/// touches, so a line is read whole. Measured on this Mac (2026-09-12): the
/// whole display 499 ms (1,150 ms of CPU, Vision on several cores), a
/// sixteenth of it 114 ms (110), a sixty-fourth 72 ms (30) — each read pays
/// about 60 ms before any pixel. So past half the display, or four pieces,
/// one whole read costs no more and reads every line in its context.
pub const OCR_REUSE_CELL_POINTS: f64 = 24.0;
pub const OCR_REUSE_MARGIN_POINTS: f64 = 4.0;
pub const OCR_REUSE_MAX_SHARE: f64 = 0.5;
pub const OCR_REUSE_MAX_PIECES: u32 = 4;

/// What an eye start hands the helper: every number the stream uses, from
/// the table above — the helper keeps none of its own.
#[must_use]
pub fn eye_table() -> Map<String, Value> {
    let mut table = Map::new();
    table.insert("framesPerSecond".into(), json!(EYE_FRAMES_PER_SECOND));
    table.insert("changesKept".into(), json!(EYE_CHANGES_KEPT));
    table.insert("idleStopMs".into(), json!(EYE_IDLE_STOP_MS));
    table.insert("firstFrameMs".into(), json!(EYE_FIRST_FRAME_MS));
    table.insert("ocrCellPoints".into(), json!(OCR_REUSE_CELL_POINTS));
    table.insert("ocrMarginPoints".into(), json!(OCR_REUSE_MARGIN_POINTS));
    table.insert("ocrMaxShare".into(), json!(OCR_REUSE_MAX_SHARE));
    table.insert("ocrMaxPieces".into(), json!(OCR_REUSE_MAX_PIECES));
    table
}

/// The labels a `sound-wait --label a,b` names, trimmed and lowercased; none
/// means any sound.
#[must_use]
pub fn sound_wait_labels(raw: Option<&str>) -> Vec<String> {
    raw.unwrap_or_default()
        .split(',')
        .map(|label| label.trim().to_lowercase())
        .filter(|label| !label.is_empty())
        .collect()
}

/// Whether one heard event is what a `sound-wait` waits for: one of the
/// labels named (any, when none is), at the confidence asked.
#[must_use]
pub fn sound_event_matches(event: &Value, labels: &[String], min_confidence: f64) -> bool {
    let label = event
        .get("label")
        .and_then(Value::as_str)
        .map(str::to_lowercase);
    let confident = event
        .get("confidence")
        .and_then(Value::as_f64)
        .is_some_and(|confidence| confidence >= min_confidence);
    confident && label.is_some_and(|label| labels.is_empty() || labels.contains(&label))
}

/// The bridge's grace past a waiting command's own budget: the answer's trip
/// back and the window's bookkeeping.
pub const COMPUTER_BRIDGE_GRACE_MS: u64 = 5_000;
/// The longest any one command may take — the person's whole turn — and so
/// the shim's own clock, which must never cut a command the bridge still
/// waits for.
pub const COMPUTER_LONGEST_DEADLINE_MS: u64 =
    COMPUTER_HANDOFF_TIMEOUT_MS + COMPUTER_BRIDGE_GRACE_MS;

/// How long a `handoff` waits for the person: what it asked, never past the
/// table's turn.
#[must_use]
pub const fn handoff_ms(asked: Option<u64>) -> u64 {
    match asked {
        Some(ms) if ms < COMPUTER_HANDOFF_TIMEOUT_MS => ms,
        _ => COMPUTER_HANDOFF_TIMEOUT_MS,
    }
}

/// How long the bridge waits for one command's answer (§1.3), the one ladder
/// the bridge, the shim and zo read: the person's turn as long as it may
/// last, a press as long as the window may ask the person about it, a batch
/// as long as its one ask, and everything else the one deadline. A person
/// still answering is never cut off by a clock meant for a machine.
#[must_use]
pub fn computer_deadline_ms(argv: &[String]) -> u64 {
    let base = COMPUTER_USE_DEADLINE_SECONDS * 1_000;
    let Ok(command) = parse_command(argv) else {
        return base;
    };
    match command.method {
        ComputerMethod::Handoff => {
            handoff_ms(command.params.get("timeoutMs").and_then(Value::as_u64))
                + COMPUTER_BRIDGE_GRACE_MS
        }
        ComputerMethod::Batch => batch_deadline_ms(&batch_steps(&command)),
        // A repeat holds until the person stops it, its bound, or the
        // person's whole turn — the ladder's top, like a handoff.
        ComputerMethod::RecipeRun if command.params.get("repeat") == Some(&Value::Bool(true)) => {
            COMPUTER_LONGEST_DEADLINE_MS
        }
        method if method.presses() => {
            base.max(COMPUTER_CONFIRM_TIMEOUT_MS + COMPUTER_BRIDGE_GRACE_MS)
        }
        _ => base,
    }
}

/// The steps a parsed batch carries — its command lines, as `parse_command`
/// checked them.
#[must_use]
pub fn batch_steps(command: &ComputerCommand) -> Vec<Vec<String>> {
    command
        .params
        .get(BATCH_COMMANDS_FLAG)
        .cloned()
        .and_then(|steps| serde_json::from_value(steps).ok())
        .unwrap_or_default()
}

/// A batch's deadline: the one deadline, and room for one of its presses to
/// be asked about when it has any.
#[must_use]
pub fn batch_deadline_ms(steps: &[Vec<String>]) -> u64 {
    let base = COMPUTER_USE_DEADLINE_SECONDS * 1_000;
    let presses = steps.iter().any(|step| {
        step.first()
            .and_then(|verb| verb_method(verb))
            .is_some_and(ComputerMethod::presses)
    });
    if presses {
        base + COMPUTER_CONFIRM_TIMEOUT_MS + COMPUTER_BRIDGE_GRACE_MS
    } else {
        base
    }
}

/// A batch's steps (§2.5): a JSON array of command lines, each one this same
/// parser accepts and each a verb that may be a step — checked whole before
/// the first step runs, and normalised: every step answers an envelope, and
/// an app step's picture is not taken.
pub fn batch_commands(raw: &str) -> Result<Vec<Vec<String>>, String> {
    let steps: Vec<Vec<String>> = serde_json::from_str(raw).map_err(|_| {
        format!(
            "--{BATCH_COMMANDS_FLAG} is a JSON array of command lines, e.g. '[[\"mouse-click\",\"--x\",\"10\",\"--y\",\"20\"],[\"type\",\"--text\",\"hi\"]]'"
        )
    })?;
    let most = COMPUTER_BATCH_MAX_STEPS;
    if steps.is_empty() || steps.len() > most {
        return Err(format!(
            "a batch holds 1 to {most} steps (this one has {})",
            steps.len()
        ));
    }
    let mut rebuilt = false;
    let mut waits: u64 = 0;
    let mut normalised = Vec::with_capacity(steps.len());
    for (at, step) in steps.into_iter().enumerate() {
        let n = at + 1;
        let verb = step.first().cloned().unwrap_or_default();
        let method = verb_method(&verb)
            .ok_or_else(|| format!("step {n}: `{verb}` is not a {COMPUTER_CLI} command"))?;
        if !method.batches() {
            return Err(format!(
                "step {n}: `{verb}` cannot be a batch step — a batch holds the hand's actions and the waits; look after it"
            ));
        }
        if rebuilt
            && step
                .iter()
                .skip(1)
                .filter_map(|word| word.strip_prefix("--"))
                .any(|flag| ELEMENT_INDEX_FLAGS.contains(&flag))
        {
            return Err(format!(
                "step {n}: element indexes change after every action and every element wait-for — use coordinates, or batch from a fresh get-app-state"
            ));
        }
        // A batch is looked at once, after: no frame of its steps.
        let step = walked_step(step, false);
        let command =
            parse_command(&step).map_err(|error| format!("step {n} ({verb}): {error}"))?;
        if let Some(why) = walked_step_refusal(&step, &command) {
            return Err(format!("step {n} ({verb}): {why}"));
        }
        waits = waits.saturating_add(
            declared_wait_ms(&command).map_err(|why| format!("step {n} ({verb}) {why}"))?,
        );
        if waits > COMPUTER_BATCH_MAX_WAIT_MS {
            return Err(format!(
                "the steps up to {n} ask to wait {waits} ms together; a batch may wait at most {COMPUTER_BATCH_MAX_WAIT_MS} ms — split it, or give its waits shorter budgets"
            ));
        }
        rebuilt |= rebuilds_the_tree(&command);
        normalised.push(step);
    }
    Ok(normalised)
}

/// Whether answering a command rebuilds the element tree the model's indexes
/// point into: every action does, and so does a `wait-for` that looks
/// through the app's elements — the helper remembers each tree it reads (a
/// window wait reads the window list, an OCR wait the pixels).
#[must_use]
pub fn rebuilds_the_tree(command: &ComputerCommand) -> bool {
    command.method.acts()
        || (command.method == ComputerMethod::WaitFor
            && command.params.get("window").is_none()
            && command.params.get("ocr").and_then(Value::as_bool) != Some(true))
}

/// The flag a step asks for its hold with, and the param it parses into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HoldFlag {
    pub flag: &'static str,
    pub param: &'static str,
}

/// How long one verb may hold the desk: the flag it asks with (none for a verb
/// that only waits the helper's own table), the most the verb's table lets it
/// hold, what it holds when the flag is absent, and what it waits besides —
/// the helper's fixed waits, mirrored from its table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hold {
    pub method: ComputerMethod,
    pub asked_by: Option<HoldFlag>,
    pub max_ms: u64,
    pub absent_ms: u64,
    pub besides_ms: u64,
}

impl Hold {
    /// What the verb's own table allows a step that asks for `asked` — the
    /// one rule the lone command and every walk read — and whether it cut it.
    #[must_use]
    pub const fn allowed_ms(&self, asked: Option<u64>) -> (u64, bool) {
        match asked {
            Some(ms) if ms > self.max_ms => (self.max_ms, true),
            Some(ms) => (ms, false),
            None => (self.absent_ms, false),
        }
    }

    /// How long a step holds the desk when it asks for `asked`.
    #[must_use]
    pub const fn held_ms(&self, asked: Option<u64>) -> u64 {
        self.besides_ms.saturating_add(self.allowed_ms(asked).0)
    }
}

pub const HOLD_WAIT: Hold = Hold {
    method: ComputerMethod::Wait,
    asked_by: Some(HoldFlag {
        flag: "ms",
        param: "ms",
    }),
    max_ms: COMPUTER_WAIT_MAX_MS,
    absent_ms: 0,
    besides_ms: 0,
};
pub const HOLD_WAIT_FOR: Hold = Hold {
    method: ComputerMethod::WaitFor,
    asked_by: Some(HoldFlag {
        flag: "timeout-ms",
        param: "timeoutMs",
    }),
    max_ms: COMPUTER_WAIT_FOR_MAX_MS,
    absent_ms: COMPUTER_WAIT_FOR_MAX_MS,
    besides_ms: 0,
};
pub const HOLD_RUN: Hold = Hold {
    method: ComputerMethod::Run,
    asked_by: Some(HoldFlag {
        flag: "timeout-ms",
        param: "timeoutMs",
    }),
    max_ms: COMPUTER_RUN_MAX_MS,
    absent_ms: COMPUTER_RUN_MAX_MS,
    besides_ms: 0,
};
/// A `quit` or an `activate`: nothing to ask, the helper's settle to wait.
const HOLD_APP_SETTLE: Hold = Hold {
    method: ComputerMethod::Quit,
    asked_by: None,
    max_ms: 0,
    absent_ms: 0,
    besides_ms: COMPUTER_APP_SETTLE_MS,
};

/// Every verb that may hold the desk, by the verbs' own tables: a batch sums
/// them against its budget, a recipe walks a step only when it fits what its
/// run has left. A hold key and a launch's wait have no cap of their own; an
/// `open` may wait for the app it opens with as long as a launch does.
pub const COMPUTER_HOLDS: &[Hold] = &[
    HOLD_WAIT,
    Hold {
        method: ComputerMethod::HoldKey,
        max_ms: u64::MAX,
        ..HOLD_WAIT
    },
    HOLD_WAIT_FOR,
    Hold {
        method: ComputerMethod::SoundWait,
        ..HOLD_WAIT_FOR
    },
    Hold {
        method: ComputerMethod::Watch,
        ..HOLD_WAIT_FOR
    },
    HOLD_RUN,
    Hold {
        method: ComputerMethod::Launch,
        asked_by: Some(HoldFlag {
            flag: "wait-ready",
            param: "waitReadyMs",
        }),
        max_ms: u64::MAX,
        absent_ms: COMPUTER_LAUNCH_WAIT_READY_MS,
        besides_ms: COMPUTER_LAUNCH_PROCESS_MS,
    },
    HOLD_APP_SETTLE,
    Hold {
        method: ComputerMethod::Activate,
        ..HOLD_APP_SETTLE
    },
    Hold {
        method: ComputerMethod::Open,
        besides_ms: COMPUTER_LAUNCH_PROCESS_MS,
        ..HOLD_APP_SETTLE
    },
];

/// How a verb may hold the desk, if it may.
#[must_use]
pub fn hold_of(method: ComputerMethod) -> Option<&'static Hold> {
    COMPUTER_HOLDS.iter().find(|hold| hold.method == method)
}

/// What a step asks its hold flag for, when its verb has one and it asked.
fn asked_hold_ms(hold: &Hold, command: &ComputerCommand) -> Option<u64> {
    command
        .params
        .get(hold.asked_by?.param)
        .and_then(Value::as_u64)
}

/// How long one parsed step holds the desk, by its verb's table.
#[must_use]
pub fn held_ms(command: &ComputerCommand) -> u64 {
    hold_of(command.method).map_or(0, |hold| hold.held_ms(asked_hold_ms(hold, command)))
}

/// What one batch step asks to wait: its hold by the verb's table — refused
/// and named when the flag is missing and would count as the table's whole
/// budget.
fn declared_wait_ms(command: &ComputerCommand) -> Result<u64, String> {
    let Some(hold) = hold_of(command.method) else {
        return Ok(0);
    };
    let asked = asked_hold_ms(hold, command);
    let held = hold.held_ms(asked);
    if let Some(asked_by) = hold.asked_by
        && asked.is_none()
        && hold.absent_ms > 0
    {
        return Err(format!(
            "has no --{} and would count as {held} ms — give it one",
            asked_by.flag
        ));
    }
    Ok(held)
}

/// Why a step may not be walked — in a batch or a recipe — though it may run
/// alone: it lifts the guard on ZeroCode's own window, or glides longer than a
/// walk goes unseen.
#[must_use]
pub fn walked_step_refusal(argv: &[String], command: &ComputerCommand) -> Option<String> {
    if let Some(flag) = argv
        .iter()
        .skip(1)
        .filter_map(|word| word.strip_prefix("--"))
        .find(|flag| WALKED_STEP_REFUSED_FLAGS.contains(flag))
    {
        return Some(format!(
            "--{flag} is not a walked step's — a screen gone stale inside a walk must not lift the guard on ZeroCode's own window"
        ));
    }
    command
        .params
        .get("steps")
        .and_then(Value::as_u64)
        .filter(|glide| *glide > COMPUTER_WALK_MAX_GLIDE_STEPS)
        .map(|_| format!("a walked step glides in at most {COMPUTER_WALK_MAX_GLIDE_STEPS} steps"))
}

/// The flags a walk adds to a step, by whether it keeps a frame of the step:
/// the envelope's always, the picture's refusal only when it does not — an
/// unframed walk's are every flag a walk may add.
pub fn walked_step_flags(framed: bool) -> impl Iterator<Item = &'static str> {
    let besides: &[&str] = if framed {
        &[]
    } else {
        WALKED_STEP_UNFRAMED_FLAGS
    };
    WALKED_STEP_FLAGS.iter().chain(besides).copied()
}

/// A step as a walk sends it: the flags the walk adds (`walked_step_flags`,
/// by whether it keeps a frame of the step), where the step's own table
/// allows them.
#[must_use]
pub fn walked_step(mut step: Vec<String>, framed: bool) -> Vec<String> {
    let Some(method) = step.first().and_then(|verb| verb_method(verb)) else {
        return step;
    };
    for flag in walked_step_flags(framed) {
        let spelled = format!("--{flag}");
        if allowed(method).contains(&flag) && !step.contains(&spelled) {
            step.push(spelled);
        }
    }
    step
}

/// How long the steps of a walk whose bridge waits `deadline_ms` may still
/// take to start: that deadline, less the answer's margin.
#[must_use]
pub const fn walk_budget_ms(deadline_ms: u64) -> u64 {
    deadline_ms.saturating_sub(COMPUTER_BATCH_ANSWER_MARGIN_MS)
}

/// Whether a step that may hold the desk `holds_ms` may still start
/// `elapsed_ms` into a walk whose bridge waits `deadline_ms`: it ends by the
/// budget (the answer's margin is past it).
#[must_use]
pub const fn walk_step_fits(elapsed_ms: u64, holds_ms: u64, deadline_ms: u64) -> bool {
    elapsed_ms.saturating_add(holds_ms) <= walk_budget_ms(deadline_ms)
}

/// How long one walked step may hold the desk: its hold by the verb's table
/// (a flag it left out counts as the table's absent value), or — in a walk
/// that asks the person (a batch; a recipe hands a press back) — for a press,
/// as long as the window may ask them about it.
#[must_use]
pub fn walk_step_holds_ms(step: &[String], asks_the_person: bool) -> u64 {
    parse_command(step).ok().map_or(0, |command| {
        let asks = if asks_the_person && command.method.presses() {
            COMPUTER_CONFIRM_TIMEOUT_MS
        } else {
            0
        };
        held_ms(&command).max(asks)
    })
}

/// One step of a walk as its planner decided it, before it runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Next<K> {
    /// Down the lone command's road, as this command line.
    Run(Vec<String>),
    /// Not walked; the reason goes in the report.
    Skip(&'static str),
    /// The walk ends before this step.
    Halt(K),
}

/// What a walk may spend: the bridge's deadline, and the halt a step that
/// could not answer before it — or a caller gone — ends the walk with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WalkBudget<K> {
    pub deadline_ms: u64,
    pub out_of_time: K,
}

/// How a walk went: a report per step it ran or skipped, in order, and the
/// step (counted from 1) it halted at, with why.
#[derive(Debug, Clone, PartialEq)]
pub struct Walked<K> {
    pub reports: Vec<Value>,
    pub halted: Option<(usize, K)>,
    pub elapsed_ms: u64,
}

impl<K> Walked<K> {
    /// Whether the walk reached its end: no step halted it.
    #[must_use]
    pub const fn finished(&self) -> bool {
        self.halted.is_none()
    }
}

/// The one loop that walks several commands (§2.5): a batch and a recipe are
/// two planners over it. Before each step the caller must still be waiting,
/// the planner decides whether it runs (and as what command line), and a step
/// that could not answer before the bridge gives up is never begun — each of
/// those halts the walk out of time. A step is given what it says it holds
/// plus the most any step before it ran past its own (an app's tree, a last
/// look: work no table counts), so a slow desk ends the walk early rather than
/// losing its answer; the first step of a call needs only to fit the budget.
/// `run` answers a step with its report and whether the walk halts there; the
/// walk adds how long each step took. `holds_ms` is asked with the step
/// itself: a recipe's line knows its door, and each door has its own table.
pub fn walk<S, K: Clone>(
    steps: &[S],
    budget: &WalkBudget<K>,
    holds_ms: impl Fn(&S, &[String]) -> u64,
    mut plan: impl FnMut(usize, &S) -> Next<K>,
    mut run: impl FnMut(usize, &S, &[String]) -> (Value, Option<K>),
    caller_waits: impl Fn() -> bool,
    elapsed_ms: impl Fn() -> u64,
) -> Walked<K> {
    let mut reports = Vec::with_capacity(steps.len());
    let mut halted = None;
    let mut overrun: u64 = 0;
    let mut ran_any = false;
    for (at, step) in steps.iter().enumerate() {
        let n = at + 1;
        let now = elapsed_ms();
        if !caller_waits() {
            halted = Some((n, budget.out_of_time.clone()));
            break;
        }
        let argv = match plan(n, step) {
            Next::Run(argv) => argv,
            Next::Skip(why) => {
                reports.push(json!({ "n": n, "skipped": why }));
                continue;
            }
            Next::Halt(kind) => {
                halted = Some((n, kind));
                break;
            }
        };
        let holds = holds_ms(step, &argv);
        let fits = if ran_any {
            walk_step_fits(now, holds.saturating_add(overrun), budget.deadline_ms)
        } else {
            holds <= walk_budget_ms(budget.deadline_ms) && now < walk_budget_ms(budget.deadline_ms)
        };
        if !fits {
            halted = Some((n, budget.out_of_time.clone()));
            break;
        }
        let began = elapsed_ms();
        let (mut report, halt) = run(n, step, &argv);
        let took = elapsed_ms().saturating_sub(began);
        overrun = overrun.max(took.saturating_sub(holds));
        ran_any = true;
        report["ms"] = json!(took);
        reports.push(report);
        if let Some(kind) = halt {
            halted = Some((n, kind));
            break;
        }
    }
    Walked {
        reports,
        halted,
        elapsed_ms: elapsed_ms(),
    }
}

/// What a walked step's result says when its answer is ok but its work did
/// not happen — a walk goes on past a step only when the work did: a program
/// that did not start, ran out its budget, died or failed; an app that showed
/// no window, did not quit, or did not come forward.
#[must_use]
pub fn walked_result_unfinished(method: ComputerMethod, result: &Value) -> Option<String> {
    let says = |key: &str, value: bool| result.get(key) == Some(&Value::Bool(value));
    match method {
        ComputerMethod::Run if says("spawned", false) => Some("the program did not start".into()),
        ComputerMethod::Run if says("timedOut", true) => {
            Some("the program ran past its --timeout-ms and was stopped".into())
        }
        ComputerMethod::Run => match result.get("exitCode") {
            Some(Value::Null) => Some("the program was ended by a signal".into()),
            Some(code) if code.as_i64() != Some(0) => Some(format!("the program exited {code}")),
            _ => None,
        },
        ComputerMethod::Launch if says("ready", false) => {
            Some("the app showed no window within --wait-ready".into())
        }
        ComputerMethod::Quit if says("terminated", false) => {
            Some("the app did not quit — a sheet may be asking the person".into())
        }
        ComputerMethod::Activate if says("active", false) => {
            Some("the app did not come to the front".into())
        }
        _ => None,
    }
}

/// Where a step that names its point leaves the pointer: where a drag lets
/// go, else where the act lands.
#[must_use]
pub fn named_point(command: &ComputerCommand) -> Option<(f64, f64)> {
    if !command.method.moves_the_pointer() {
        return None;
    }
    let point = |x: &str, y: &str| {
        Some((
            command.params.get(x)?.as_f64()?,
            command.params.get(y)?.as_f64()?,
        ))
    };
    point("toX", "toY").or_else(|| point("x", "y"))
}

/// One walked step's line — a batch's or a recipe's: its number, its verb,
/// whether it went (the road answered, and ok, and its result says the work
/// happened), and its own result or refusal; the walk adds how long it took.
#[must_use]
pub fn walk_step_report(
    n: usize,
    argv: &[String],
    answered: bool,
    envelope: Option<&Value>,
    stderr: &str,
) -> Value {
    use crate::computer_use_protocol::error_code;
    let verb = argv.first().map_or("", String::as_str);
    let said_ok = answered
        && envelope
            .and_then(|envelope| envelope.get("ok"))
            .and_then(Value::as_bool)
            == Some(true);
    let result = envelope
        .and_then(|envelope| envelope.get("result"))
        .cloned()
        .unwrap_or(Value::Null);
    let unfinished = said_ok
        .then(|| verb_method(verb))
        .flatten()
        .and_then(|method| walked_result_unfinished(method, &result));
    let mut report = json!({ "n": n, "verb": verb, "ok": said_ok && unfinished.is_none() });
    if let Some(why) = unfinished {
        report["result"] = result;
        report["error"] = json!({ "code": error_code::UNFINISHED, "message": why });
    } else if said_ok {
        report["result"] = result;
    } else {
        let message = if stderr.trim().is_empty() {
            "the step answered nothing"
        } else {
            stderr.trim()
        };
        report["error"] = envelope
            .and_then(|envelope| envelope.get("error"))
            .cloned()
            .unwrap_or_else(|| json!({ "code": error_code::UNANSWERED, "message": message }));
    }
    report
}

/// A batch's answer from its step lines: every step it ran, the first one
/// refused (its own code, so a skill recovers from it as from a lone
/// command), or the deadline that stopped it starting more. A tree a later
/// step made stale stays only on the last step that answered one.
#[must_use]
pub fn batch_answer(mut steps: Vec<Value>, of: usize, elapsed_ms: u64, deadline: bool) -> Value {
    for key in BATCH_SUPERSEDED_KEYS {
        let pointer = format!("/result/{key}");
        if let Some(last) = steps
            .iter()
            .rposition(|step| step.pointer(&pointer).is_some())
        {
            for step in &mut steps[..last] {
                if let Some(result) = step.get_mut("result").and_then(Value::as_object_mut) {
                    result.remove(*key);
                }
            }
        }
    }
    let ran = steps
        .iter()
        .filter(|step| step["ok"] == Value::Bool(true))
        .count();
    let refused = steps
        .last()
        .filter(|step| step["ok"] == Value::Bool(false))
        .cloned();
    let mut result = json!({ "ran": ran, "of": of, "elapsedMs": elapsed_ms, "steps": steps });
    if let Some(step) = refused {
        result["refusedAt"] = step["n"].clone();
        let verb = step["verb"].as_str().unwrap_or_default();
        let code = step
            .pointer("/error/code")
            .and_then(Value::as_str)
            .unwrap_or(crate::computer_use_protocol::error_code::UNANSWERED);
        let message = step
            .pointer("/error/message")
            .and_then(Value::as_str)
            .unwrap_or_default();
        return json!({
            "ok": false,
            "error": { "code": code, "message": format!("step {} of {of} ({verb}): {message}", step["n"]) },
            "result": result,
        });
    }
    if deadline && ran < of {
        return json!({
            "ok": false,
            "error": {
                "code": crate::computer_use_protocol::error_code::BATCH_DEADLINE,
                "message": format!("stopped before step {} of {of}: the bridge's deadline was near; the rest did not run", ran + 1),
            },
            "result": result,
        });
    }
    json!({ "ok": true, "result": result })
}

/// A batch's answer as a person reads it: one line per step, then the tally
/// or the refusal.
#[must_use]
pub fn batch_text(answer: &Value) -> String {
    let mut lines: Vec<String> = answer
        .pointer("/result/steps")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|step| {
            let (n, verb, ms) = (
                &step["n"],
                step["verb"].as_str().unwrap_or_default(),
                &step["ms"],
            );
            if step["ok"] == Value::Bool(true) {
                format!("{n} {verb} ok {ms} ms")
            } else {
                format!(
                    "{n} {verb} refused {}: {}",
                    step.pointer("/error/code")
                        .and_then(Value::as_str)
                        .unwrap_or(crate::computer_use_protocol::error_code::UNANSWERED),
                    step.pointer("/error/message")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                )
            }
        })
        .collect();
    match answer.pointer("/error/message").and_then(Value::as_str) {
        Some(message) => lines.push(format!("{COMPUTER_CLI}: {message}")),
        None => lines.push(format!(
            "ran {} of {} in {} ms",
            answer.pointer("/result/ran").unwrap_or(&Value::Null),
            answer.pointer("/result/of").unwrap_or(&Value::Null),
            answer.pointer("/result/elapsedMs").unwrap_or(&Value::Null)
        )),
    }
    lines.join("\n")
}

/// The longest a `wait-for` watches, how often it looks, and the most of a
/// window's text a `read` answers with.
pub const COMPUTER_WAIT_FOR_MAX_MS: u64 = 60_000;
pub const COMPUTER_WAIT_FOR_POLL_MS: u64 = 250;
/// The least a `wait-for` may be asked to watch: one look and no wait — the
/// loop (`desktop_wait_for`) looks before it reads the clock, so `--timeout-ms
/// 0` reads the screen once and answers what it saw.
pub const COMPUTER_WAIT_FOR_MIN_MS: u64 = 0;

/// A Flow's two budgets (docs/plans/flow-engine-implementation-20260914.md
/// D9): its acts are held by the verbs' own tables, its verify lines by
/// this one. A verify line waits a person's pause (`COMPUTER_WAIT_MAX_MS`,
/// the longest a `wait` may hold a turn): three times a replayed check's
/// default (`computer_recipe::RECIPE_CHECK_MS`, 10 s — a check that must
/// come is given longer than one that merely places the walk), under a
/// `wait-for`'s own ceiling and inside one `recipe-run`'s room, so a verdict
/// is never cut short by a clock meant for an act.
pub const FLOW_VERIFY_MS: u64 = COMPUTER_WAIT_MAX_MS;
/// A Flow's baseline probe: before its first act, every event line is
/// asked once with no wait — the least a `wait-for` allows — so an event
/// that already holds at the start is known to be stale, not this run's.
pub const FLOW_BASELINE_PROBE_MS: u64 = COMPUTER_WAIT_FOR_MIN_MS;
/// A Flow's trigger (`## Trigger`, design §2 `event`; `recipe-run
/// --repeat`): the most one wait for its line to flip may hold — the longest
/// a `wait-for` watches. A blocking check waits inside its own verb's
/// table; a check that answers at once (`find`) is asked again only once a
/// whole wait has passed, so a trigger is one look a minute at most and the
/// evidence log is not a poll's.
pub const FLOW_TRIGGER_MS: u64 = COMPUTER_WAIT_FOR_MAX_MS;
/// How many presses one `walk` spends when its caller names no number.
///
/// Measured, on this machine's own recorded walks (2026-09-18): of the 43
/// Computer Use sessions under `computer-use/sessions`, 23 acted at all, and
/// their acting steps run p50 8, p75 25, p90 40. Thirty covers 18 of those 23
/// — all but two APM benches, whose hundreds of presses at random targets are
/// not an errand, and three long operator sessions. The call's own deadline
/// ([`COMPUTER_USE_DEADLINE_SECONDS`]) bounds the walk regardless, and the
/// screen-did-not-move rule ends it before either.
pub const WALK_STEPS_DEFAULT: usize = 30;

/// The most presses a caller may ask one `walk` for: the p90 of those same 23
/// sessions. Past a walk's own ninetieth percentile it is not reaching a
/// goal, it is hunting, and hunting unattended is what the
/// screen-did-not-move rule exists to end.
pub const WALK_STEPS_MAX: usize = 40;

/// How many presses this walk may spend: what the caller asked, clamped to
/// [`WALK_STEPS_MAX`], or [`WALK_STEPS_DEFAULT`] when they asked for none.
/// The parser refuses anything out of range, so this reads a checked value —
/// it clamps so that the number the walk spends is never a number nobody
/// wrote down.
/// `walk --overlap`: begin the next judgment on the last look while the press
/// lands (the window's `errand::Options::overlap`, t-6132 S2). A flag and its
/// parameter key, spelled once for the parser, the verb table and the walk.
pub const WALK_OVERLAP_FLAG: &str = "overlap";
pub const WALK_OVERLAP_PARAM: &str = "overlap";

/// Whether a walk was asked to judge ahead of its looks (`--overlap`).
#[must_use]
pub fn walk_overlaps(params: &Value) -> bool {
    params.get(WALK_OVERLAP_PARAM) == Some(&Value::Bool(true))
}

/// `walk --rescue` / `recipe-run --rescue`: when the screen seat's judgment
/// is under its press floor, ask a second reader — the frontier, headless —
/// the same closed choice before handing the walk to the person (the
/// window's `errand::Options::rescue`, t-6132 S3).
pub const WALK_RESCUE_FLAG: &str = "rescue";
pub const WALK_RESCUE_PARAM: &str = "rescue";

/// Whether a walk was asked to try a second reader before the person.
#[must_use]
pub fn walk_rescues(params: &Value) -> bool {
    params.get(WALK_RESCUE_PARAM) == Some(&Value::Bool(true))
}

/// `walk --replay`: this walk repeats one walked before — a QA run, a Flow
/// walked again (t-6385) — so the uses the Jev table moves in a repeated run
/// stand where it says (`jev::JevUse::repeat`): the judgment cache answers a
/// question it has answered before rather than only recording it.
pub const WALK_REPLAY_FLAG: &str = "replay";
pub const WALK_REPLAY_PARAM: &str = "replay";

/// The run a walk was asked in: repeated when it said `--replay`.
#[must_use]
pub fn walk_run(params: &Value) -> crate::jev::Run {
    if params.get(WALK_REPLAY_PARAM) == Some(&Value::Bool(true)) {
        crate::jev::Run::Repeated
    } else {
        crate::jev::Run::Fresh
    }
}

#[must_use]
pub fn walk_steps(params: &Value) -> usize {
    params
        .get("steps")
        .and_then(Value::as_u64)
        .and_then(|steps| usize::try_from(steps).ok())
        .map_or(WALK_STEPS_DEFAULT, |steps| steps.min(WALK_STEPS_MAX))
}

/// How many rounds of a repeat may fail in a row before it ends: the stuck
/// rule's count (`COMPUTER_STUCK_REPEATS`) — the same failure this many
/// times with nothing changing is a loop, not progress.
pub const FLOW_REPEAT_MAX_FAILS: u32 = COMPUTER_STUCK_REPEATS;
pub const COMPUTER_READ_MAX_CHARS: usize = 32_768;

/// The param a request that types text carries the keyboard's table in (B3):
/// how long the helper gives a field to show typed text, how often it looks
/// meanwhile, and the most of a field it reads back — this table's numbers, so
/// the helper keeps none of its own. Without it the helper types a text that
/// moves the focus (a Tab, a Return) not at all, and judges no landing.
pub const KEYBOARD_GUARD_PARAM: &str = "keyboardGuard";

/// What `KEYBOARD_GUARD_PARAM` carries.
#[must_use]
pub fn keyboard_guard() -> Value {
    json!({
        "settleMs": COMPUTER_SETTLE_MS,
        "pollMs": COMPUTER_SETTLE_POLL_MS,
        "maxChars": COMPUTER_READ_MAX_CHARS,
    })
}
/// Helper refusals a `wait-for` keeps looking through: the app or window it
/// waits for may simply not be there yet.
pub const WAIT_FOR_RETRIABLE_CODES: &[&str] = &["app_not_found", "window_not_found"];

/// How long a `wait-for` is allowed, and whether the table shortened it.
#[must_use]
pub const fn desktop_wait_for_ms(asked: Option<u64>) -> (u64, bool) {
    HOLD_WAIT_FOR.allowed_ms(asked)
}

/// Whether a `wait-for` is satisfied by what it just saw: something present,
/// or — with `--absent` — nothing left.
#[must_use]
pub const fn wait_for_settled(matches: usize, absent: bool) -> bool {
    if absent { matches == 0 } else { matches > 0 }
}

/// Whether a helper refusal is worth another look during a `wait-for`.
#[must_use]
pub fn wait_for_retries(code: &str) -> bool {
    WAIT_FOR_RETRIABLE_CODES.contains(&code)
}

/// Whether a window title answers a `wait-for --window` — a case-insensitive
/// fragment, the way a person names a window.
#[must_use]
pub fn window_title_matches(title: &str, wanted: &str) -> bool {
    let wanted = wanted.trim().to_lowercase();
    !wanted.is_empty() && title.to_lowercase().contains(&wanted)
}

/// The head of a window's text a `read` answers with, and whether it was cut —
/// cut at a character, never inside one.
#[must_use]
pub fn desktop_read_text(text: &str) -> (String, bool) {
    if text.len() <= COMPUTER_READ_MAX_CHARS {
        return (text.to_string(), false);
    }
    let mut end = COMPUTER_READ_MAX_CHARS;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    (text[..end].to_string(), true)
}

/// How long a `wait` actually holds, and whether the table shortened it.
#[must_use]
pub const fn desktop_wait_ms(asked: u64) -> (u64, bool) {
    HOLD_WAIT.allowed_ms(Some(asked))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComputerMethod {
    Capabilities,
    ListApps,
    Permissions,
    ListWindows,
    GetAppState,
    Click,
    PerformSecondaryAction,
    Scroll,
    Drag,
    TypeText,
    PressKey,
    Hotkey,
    PasteText,
    SetValue,
    /// The desktop verbs (docs/design/computer-use-full-operator.md §2.1):
    /// the screen, the mouse and the keys at screen coordinates, no app named.
    Screenshot,
    Zoom,
    MouseMove,
    MouseClick,
    MouseDrag,
    MouseScroll,
    CursorPosition,
    Key,
    HoldKey,
    Type,
    /// Answered by the window itself — a bounded sleep needs no helper.
    Wait,
    Displays,
    /// Apps, windows and the system (§2.2): what a person does before and
    /// around an app — start it, bring it forward, quit it, open a file or an
    /// address, run a program, place a window, hold the clipboard.
    Launch,
    Quit,
    Activate,
    Open,
    /// Answered by the window itself — a bounded run of a program, no helper.
    Run,
    ListAllWindows,
    WindowFocus,
    WindowMove,
    WindowResize,
    WindowMinimize,
    WindowZoom,
    WindowClose,
    ClipboardRead,
    ClipboardWrite,
    /// The meaning layer (§2.3): find an element or a piece of screen text,
    /// wait for it to appear or go, read what a window says. `--ocr` reads
    /// the pixels when the accessibility tree is empty.
    Find,
    /// Answered by the window itself — a poll over `find`/`listAllWindows`.
    WaitFor,
    Read,
    /// The one hand on the operator (§1.3): stop now, resume, and say where
    /// things stand. All three are the window's — a stop must never queue
    /// behind the action it interrupts.
    Stop,
    Resume,
    Status,
    /// Where this session's evidence is, and its last steps — the window's.
    Evidence,
    /// QA (§4): the scenario's verdict into the evidence folder, and a
    /// screen against a baseline image — both the window's.
    Verdict,
    Compare,
    /// The eyes and the person's turn (§7.1, §7.4): one observation that
    /// merges the tree, the pixels, the text and what changed; and the
    /// handoff card that gives the desk to the person and waits.
    Observe,
    Handoff,
    /// Memory that outlives a session (§7.2): a walked procedure saved as a
    /// document, listed, and read back — the window's.
    RecipeSave,
    RecipeList,
    RecipeShow,
    /// Walk a saved recipe in one call (§7.2) — the window's, like a batch.
    RecipeRun,
    /// The ears (§7.1): listen to the machine's sound through the Screen &
    /// System Audio Recording permission the eyes already hold, read what the
    /// system's sound classifier heard, and wait for a sound — the window
    /// asks the helper, the way `wait-for` looks.
    ListenStart,
    ListenStop,
    SoundRead,
    SoundWait,
    /// The continuous eye (§7.1): wait for the screen to change or to go
    /// still, reading the display's repaints — the window's, like wait-for.
    Watch,
    /// One judgement, many actions (§2.5) — the window's: each step walks the
    /// lone command's road, so it is stopped, paced, confirmed and logged as
    /// if alone.
    Batch,
    /// The one step of a procedure whose next press cannot be written down in
    /// advance (t-4774) — the window's, like a batch. Everything a caller
    /// already knows the shape of belongs in a `batch`; this is where the
    /// screen decides, and a judgment reads the controls it is showing and
    /// picks one of their numbers. It presses and nothing else, and it ends
    /// on the caller's own `--until` check, on the judgment saying it is
    /// there, on a screen that will not move, or on its step budget.
    Walk,
    /// A live reflex run (realtime v1, t-9205) — the window's: it reads a
    /// Flow document's reflex plan, admits it at its door, hands the helper
    /// the plan with the window's tables and the run's policy, and answers at
    /// once while the helper's one hand runs the plan until its deadline.
    ReflexStart,
    /// Where one reflex run stands (`--run`), and never another's.
    ReflexStatus,
    /// End one reflex run (`--run`) and no other; the operator's `stop` ends
    /// whatever runs.
    ReflexStop,
}

/// The provider methods the Windows provider answers today
/// (`computer_use/windows/provider.rs`), and the ones held for it — the
/// contract the parity work resumes from (docs/design/computer-use-windows-parity.md §1.3).
pub const WINDOWS_PROVIDER_METHODS: &[&str] = &[
    "handshake",
    "listApps",
    "listWindows",
    "getAppState",
    "click",
    "performSecondaryAction",
    "setValue",
    "typeText",
    "pressKey",
    "hotkey",
    "pasteText",
    "scroll",
    "drag",
];
pub const WINDOWS_HOLD_METHODS: &[&str] = &[
    "screenshotDesktop",
    "displays",
    "cursorPosition",
    "mouseMove",
    "mouseClick",
    "mouseDrag",
    "mouseScroll",
    "key",
    "holdKey",
    "type",
    "launchApp",
    "quitApp",
    "activateApp",
    "openTarget",
    "listAllWindows",
    "windowAction",
    "clipboardRead",
    "clipboardWrite",
    "findElements",
    "readText",
    "listenStart",
    "listenStop",
    "soundRead",
];

/// Where a method stands on Windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowsStanding {
    /// The Windows provider answers it.
    Answered,
    /// Held: the provider refuses it as `unsupported_capability` until the
    /// parity work lands.
    Held,
    /// The window answers it on every platform; no provider method exists.
    WindowsOwn,
}

impl ComputerMethod {
    /// Every verb, in the CLI's order — the list the contracts walk.
    pub const ALL: &'static [Self] = &[
        Self::Capabilities,
        Self::ListApps,
        Self::Permissions,
        Self::ListWindows,
        Self::GetAppState,
        Self::Click,
        Self::PerformSecondaryAction,
        Self::Scroll,
        Self::Drag,
        Self::TypeText,
        Self::PressKey,
        Self::Hotkey,
        Self::PasteText,
        Self::SetValue,
        Self::Screenshot,
        Self::Zoom,
        Self::MouseMove,
        Self::MouseClick,
        Self::MouseDrag,
        Self::MouseScroll,
        Self::CursorPosition,
        Self::Key,
        Self::HoldKey,
        Self::Type,
        Self::Wait,
        Self::Displays,
        Self::Launch,
        Self::Quit,
        Self::Activate,
        Self::Open,
        Self::Run,
        Self::ListAllWindows,
        Self::WindowFocus,
        Self::WindowMove,
        Self::WindowResize,
        Self::WindowMinimize,
        Self::WindowZoom,
        Self::WindowClose,
        Self::ClipboardRead,
        Self::ClipboardWrite,
        Self::Find,
        Self::WaitFor,
        Self::Read,
        Self::Stop,
        Self::Resume,
        Self::Status,
        Self::Evidence,
        Self::Verdict,
        Self::Compare,
        Self::Observe,
        Self::Handoff,
        Self::RecipeSave,
        Self::RecipeList,
        Self::RecipeShow,
        Self::RecipeRun,
        Self::ListenStart,
        Self::ListenStop,
        Self::SoundRead,
        Self::SoundWait,
        Self::Watch,
        Self::Batch,
        Self::Walk,
        Self::ReflexStart,
        Self::ReflexStatus,
        Self::ReflexStop,
    ];

    /// Where the verb stands on Windows, by the two tables.
    #[must_use]
    pub fn windows_standing(self) -> WindowsStanding {
        match self.provider_name() {
            None => WindowsStanding::WindowsOwn,
            Some(name) if WINDOWS_PROVIDER_METHODS.contains(&name) => WindowsStanding::Answered,
            Some(_) => WindowsStanding::Held,
        }
    }

    #[must_use]
    pub const fn provider_name(self) -> Option<&'static str> {
        match self {
            Self::Capabilities => Some("handshake"),
            Self::ListApps => Some("listApps"),
            Self::Permissions => None,
            Self::ListWindows => Some("listWindows"),
            Self::GetAppState => Some("getAppState"),
            Self::Click => Some("click"),
            Self::PerformSecondaryAction => Some("performSecondaryAction"),
            Self::Scroll => Some("scroll"),
            Self::Drag => Some("drag"),
            Self::TypeText => Some("typeText"),
            Self::PressKey => Some("pressKey"),
            Self::Hotkey => Some("hotkey"),
            Self::PasteText => Some("pasteText"),
            Self::SetValue => Some("setValue"),
            Self::Screenshot | Self::Zoom => Some("screenshotDesktop"),
            Self::MouseMove => Some("mouseMove"),
            Self::MouseClick => Some("mouseClick"),
            Self::MouseDrag => Some("mouseDrag"),
            Self::MouseScroll => Some("mouseScroll"),
            Self::CursorPosition => Some("cursorPosition"),
            Self::Key => Some("key"),
            Self::HoldKey => Some("holdKey"),
            Self::Type => Some("type"),
            Self::Wait => None,
            Self::Displays => Some("displays"),
            Self::Launch => Some("launchApp"),
            Self::Quit => Some("quitApp"),
            Self::Activate => Some("activateApp"),
            Self::Open => Some("openTarget"),
            Self::Run => None,
            Self::ListAllWindows => Some("listAllWindows"),
            Self::WindowFocus
            | Self::WindowMove
            | Self::WindowResize
            | Self::WindowMinimize
            | Self::WindowZoom
            | Self::WindowClose => Some("windowAction"),
            Self::ClipboardRead => Some("clipboardRead"),
            Self::ClipboardWrite => Some("clipboardWrite"),
            Self::Find => Some("findElements"),
            Self::WaitFor => None,
            Self::Read => Some("readText"),
            Self::Stop | Self::Resume | Self::Status | Self::Evidence => None,
            Self::Verdict | Self::Compare | Self::Observe | Self::Handoff => None,
            Self::RecipeSave | Self::RecipeList | Self::RecipeShow | Self::RecipeRun => None,
            Self::ListenStart => Some("listenStart"),
            Self::ListenStop => Some("listenStop"),
            Self::SoundRead => Some("soundRead"),
            Self::SoundWait | Self::Watch | Self::Batch | Self::Walk => None,
            Self::ReflexStart | Self::ReflexStatus | Self::ReflexStop => None,
        }
    }

    /// The CLI word for the method — what a person or a log calls it.
    #[must_use]
    pub const fn verb_name(self) -> &'static str {
        match self {
            Self::Capabilities => "capabilities",
            Self::ListApps => "list-apps",
            Self::Permissions => "permissions",
            Self::ListWindows => "list-windows",
            Self::GetAppState => "get-app-state",
            Self::Click => "click",
            Self::PerformSecondaryAction => "perform-secondary-action",
            Self::Scroll => "scroll",
            Self::Drag => "drag",
            Self::TypeText => "type-text",
            Self::PressKey => "press-key",
            Self::Hotkey => "hotkey",
            Self::PasteText => "paste-text",
            Self::SetValue => "set-value",
            Self::Screenshot => "screenshot",
            Self::Zoom => "zoom",
            Self::MouseMove => "mouse-move",
            Self::MouseClick => "mouse-click",
            Self::MouseDrag => "mouse-drag",
            Self::MouseScroll => "mouse-scroll",
            Self::CursorPosition => "cursor-position",
            Self::Key => "key",
            Self::HoldKey => "hold-key",
            Self::Type => "type",
            Self::Wait => "wait",
            Self::Displays => "displays",
            Self::Launch => "launch",
            Self::Quit => "quit",
            Self::Activate => "activate",
            Self::Open => "open",
            Self::Run => "run",
            Self::ListAllWindows => "list-all-windows",
            Self::WindowFocus => "window-focus",
            Self::WindowMove => "window-move",
            Self::WindowResize => "window-resize",
            Self::WindowMinimize => "window-minimize",
            Self::WindowZoom => "window-zoom",
            Self::WindowClose => "window-close",
            Self::ClipboardRead => "clipboard-read",
            Self::ClipboardWrite => "clipboard-write",
            Self::Find => "find",
            Self::WaitFor => "wait-for",
            Self::Read => "read",
            Self::Stop => "stop",
            Self::Resume => "resume",
            Self::Status => "status",
            Self::Evidence => "evidence",
            Self::Verdict => "verdict",
            Self::Compare => "compare",
            Self::Observe => "observe",
            Self::Handoff => "handoff",
            Self::RecipeSave => "recipe-save",
            Self::RecipeList => "recipe-list",
            Self::RecipeShow => "recipe-show",
            Self::RecipeRun => "recipe-run",
            Self::ListenStart => "listen-start",
            Self::ListenStop => "listen-stop",
            Self::SoundRead => "sound-read",
            Self::SoundWait => "sound-wait",
            Self::Watch => "watch",
            Self::Batch => "batch",
            Self::Walk => "walk",
            Self::ReflexStart => "reflex-start",
            Self::ReflexStatus => "reflex-status",
            Self::ReflexStop => "reflex-stop",
        }
    }

    /// Whether the helper types the verb's text as keys — the requests that
    /// carry the keyboard's table (`KEYBOARD_GUARD_PARAM`): a desktop `type`,
    /// and an app's `type-text` when it falls back to keys.
    #[must_use]
    pub const fn types_keys(self) -> bool {
        matches!(self, Self::Type | Self::TypeText)
    }

    /// Whether the verb presses something a confirmation may guard (§1.5):
    /// a click on an element or a point, a drag (a click on what it lets go
    /// of), or the key that fires a default button — tapped or held.
    #[must_use]
    pub const fn presses(self) -> bool {
        matches!(
            self,
            Self::Click
                | Self::PerformSecondaryAction
                | Self::PressKey
                | Self::Hotkey
                | Self::MouseClick
                | Self::MouseDrag
                | Self::Key
                | Self::HoldKey
        )
    }

    /// Whether the verb changes something — injects input, starts or ends a
    /// program, moves a window, writes the clipboard. A stopped operator
    /// refuses these and still answers every look.
    #[must_use]
    pub const fn acts(self) -> bool {
        matches!(
            self,
            Self::Click
                | Self::PerformSecondaryAction
                | Self::Scroll
                | Self::Drag
                | Self::TypeText
                | Self::PressKey
                | Self::Hotkey
                | Self::PasteText
                | Self::SetValue
                | Self::MouseMove
                | Self::MouseClick
                | Self::MouseDrag
                | Self::MouseScroll
                | Self::Key
                | Self::HoldKey
                | Self::Type
                | Self::Launch
                | Self::Quit
                | Self::Activate
                | Self::Open
                | Self::Run
                | Self::WindowFocus
                | Self::WindowMove
                | Self::WindowResize
                | Self::WindowMinimize
                | Self::WindowZoom
                | Self::WindowClose
                | Self::ClipboardWrite
                | Self::Batch
                | Self::RecipeRun
                | Self::Walk
                | Self::ReflexStart
        )
    }

    /// Whether the verb may be a step of a `batch` (§2.5): the hand's actions
    /// and the three waits — for time, the screen and the ears — so a familiar
    /// sequence keeps time and stops at a miss. Bringing an app or a window
    /// forward is a hand's step too. Left out: looks (their answer is what
    /// gets judged, and a batch is looked at once, after); the verbs that
    /// change the screen wholesale or answer output to read (run, launch,
    /// quit, open, window minimize, zoom and close); the person's turn
    /// (handoff); and a batch itself — no nesting.
    #[must_use]
    pub const fn batches(self) -> bool {
        matches!(
            self,
            Self::Click
                | Self::PerformSecondaryAction
                | Self::Scroll
                | Self::Drag
                | Self::TypeText
                | Self::PressKey
                | Self::Hotkey
                | Self::PasteText
                | Self::SetValue
                | Self::MouseMove
                | Self::MouseClick
                | Self::MouseDrag
                | Self::MouseScroll
                | Self::Key
                | Self::HoldKey
                | Self::Type
                | Self::Activate
                | Self::WindowFocus
                | Self::WindowMove
                | Self::WindowResize
                | Self::ClipboardWrite
                | Self::Wait
                | Self::WaitFor
                | Self::SoundWait
                | Self::Watch
        )
    }

    /// Whether the verb leaves the pointer at a desktop point it names: where
    /// a move, a click or a scroll lands, where a drag lets go.
    #[must_use]
    pub const fn moves_the_pointer(self) -> bool {
        matches!(
            self,
            Self::MouseMove | Self::MouseClick | Self::MouseDrag | Self::MouseScroll
        )
    }

    /// Whether the verb may leave the pointer at a point its command does not
    /// name: an app's click presses a window's point — or an element's, when
    /// the helper falls back to a synthetic press — with the system's own
    /// pointer. (An app's drag and scroll go to the app, not the pointer.)
    #[must_use]
    pub const fn moves_the_pointer_unnamed(self) -> bool {
        matches!(self, Self::Click)
    }

    /// The window verbs' one helper method takes the action by name.
    #[must_use]
    pub const fn window_action(self) -> Option<&'static str> {
        match self {
            Self::WindowFocus => Some("focus"),
            Self::WindowMove => Some("move"),
            Self::WindowResize => Some("resize"),
            Self::WindowMinimize => Some("minimize"),
            Self::WindowZoom => Some("zoom"),
            Self::WindowClose => Some("close"),
            _ => None,
        }
    }

    /// A verb that works on the whole desktop names no app.
    #[must_use]
    pub const fn is_desktop(self) -> bool {
        matches!(
            self,
            Self::Screenshot
                | Self::Zoom
                | Self::MouseMove
                | Self::MouseClick
                | Self::MouseDrag
                | Self::MouseScroll
                | Self::CursorPosition
                | Self::Key
                | Self::HoldKey
                | Self::Type
                | Self::Wait
                | Self::Displays
                | Self::Open
                | Self::Run
                | Self::ListAllWindows
                | Self::WindowFocus
                | Self::WindowMove
                | Self::WindowResize
                | Self::WindowMinimize
                | Self::WindowZoom
                | Self::WindowClose
                | Self::ClipboardRead
                | Self::ClipboardWrite
                | Self::Find
                | Self::WaitFor
                | Self::Read
                | Self::Stop
                | Self::Resume
                | Self::Status
                | Self::Evidence
                | Self::Verdict
                | Self::Compare
                | Self::Observe
                | Self::Handoff
                | Self::RecipeSave
                | Self::RecipeList
                | Self::RecipeShow
                | Self::RecipeRun
                | Self::ListenStart
                | Self::ListenStop
                | Self::SoundRead
                | Self::SoundWait
                | Self::Watch
                | Self::Batch
                | Self::ReflexStart
                | Self::ReflexStatus
                | Self::ReflexStop
        )
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ComputerCommand {
    pub method: ComputerMethod,
    pub params: Value,
    pub json: bool,
}

/// Mobile automation has its own vocabulary because it must never be
/// interpreted as a request to click the Simulator/Emulator desktop window.
/// The shell dispatches these commands to the exact backend that paints the
/// ZeroCode emulator pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmulatorMethod {
    Find,
    Foreground,
    List,
    Open,
    Tree,
    Marks,
    Click,
    Tap,
    Swipe,
    Text,
    Button,
    Rotate,
    Screenshot,
}

impl EmulatorMethod {
    /// Every verb, in the usage's order.
    pub const ALL: [Self; 13] = [
        Self::List,
        Self::Open,
        Self::Tree,
        Self::Marks,
        Self::Find,
        Self::Foreground,
        Self::Click,
        Self::Tap,
        Self::Swipe,
        Self::Text,
        Self::Button,
        Self::Rotate,
        Self::Screenshot,
    ];

    /// The CLI word for the method — what a person or a log calls it.
    #[must_use]
    pub const fn verb_name(self) -> &'static str {
        match self {
            Self::Find => "find",
            Self::Foreground => "foreground",
            Self::List => "list",
            Self::Open => "open",
            Self::Tree => "tree",
            Self::Marks => "marks",
            Self::Click => "click",
            Self::Tap => "tap",
            Self::Swipe => "swipe",
            Self::Text => "text",
            Self::Button => "button",
            Self::Rotate => "rotate",
            Self::Screenshot => "screenshot",
        }
    }
}

/// The emulator verbs whose request says where its caller stands
/// ([`CWD_HEADER`]): the one that writes a file where it is told —
/// `screenshot --out <path>` takes a relative path from the shell's own
/// folder, as a person would.
pub const EMULATOR_CWD_VERBS: &[&str] = &[EmulatorMethod::Screenshot.verb_name()];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EmulatorPlatform {
    Ios,
    Android,
}

impl EmulatorPlatform {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ios => "ios",
            Self::Android => "android",
        }
    }
}

impl std::str::FromStr for EmulatorPlatform {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "ios" => Ok(Self::Ios),
            "android" => Ok(Self::Android),
            _ => Err("--platform must be ios or android".into()),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct EmulatorCommand {
    pub method: EmulatorMethod,
    pub app: Option<String>,
    pub platform: Option<EmulatorPlatform>,
    pub device: Option<String>,
    pub x: Option<f64>,
    pub y: Option<f64>,
    pub x1: Option<f64>,
    pub y1: Option<f64>,
    pub x2: Option<f64>,
    pub y2: Option<f64>,
    pub ms: Option<u32>,
    pub text: Option<String>,
    pub name: Option<String>,
    pub rotation: Option<u32>,
    pub out: Option<String>,
    pub mark: Option<usize>,
    pub look: Option<String>,
    /// `click … --preview`: answer the marks the screen the press settled on
    /// would carry ([`EMULATOR_PREVIEW_FLAG`]).
    pub preview: bool,
    pub json: bool,
}

/// `zerocode-emulator click … --preview` (t-6385): besides the press, answer
/// the marks the screen it settled on would carry, numbered off the tree the
/// press waited on, under the answer's own key of the same word. Not a look —
/// nothing in it can be pressed: a walk begins its next judgment on it while
/// the look is taken.
pub const EMULATOR_PREVIEW_FLAG: &str = "preview";

/// Parse the built-in-emulator CLI without forwarding unknown or partial
/// gestures. Coordinates are normalized because both pane backends speak the
/// same 0…1 contract.
pub fn parse_emulator_command(argv: &[String]) -> Result<EmulatorCommand, String> {
    let Some(verb) = argv.first().map(String::as_str) else {
        return Err(emulator_usage());
    };
    if matches!(verb, "-h" | "--help" | "help") {
        return Err(emulator_usage());
    }
    let Some(method) = EmulatorMethod::ALL
        .into_iter()
        .find(|method| method.verb_name() == verb)
    else {
        return Err(format!(
            "unknown emulator command `{verb}`\n\n{}",
            emulator_usage()
        ));
    };
    let flags = flags(&argv[1..])?;
    let allowed: &[&str] = match method {
        EmulatorMethod::List => &["json"],
        EmulatorMethod::Open => &["json", "platform", "device"],
        EmulatorMethod::Tree => &["json", "platform", "device"],
        EmulatorMethod::Marks => &["json", "platform", "device", "text"],
        EmulatorMethod::Click => &[
            "json",
            "platform",
            "device",
            "mark",
            "look",
            "text",
            EMULATOR_PREVIEW_FLAG,
        ],
        EmulatorMethod::Tap => &["json", "platform", "device", "x", "y"],
        EmulatorMethod::Swipe => &["json", "platform", "device", "x1", "y1", "x2", "y2", "ms"],
        EmulatorMethod::Text | EmulatorMethod::Find => &["json", "platform", "device", "text"],
        EmulatorMethod::Foreground => &["json", "platform", "device", "app"],
        EmulatorMethod::Button => &["json", "platform", "device", "name"],
        EmulatorMethod::Rotate => &["json", "platform", "device", "rotation"],
        EmulatorMethod::Screenshot => &["json", "platform", "device", "out"],
    };
    for name in flags.keys() {
        if !allowed.contains(&name.as_str()) {
            return Err(format!("unknown flag --{name}"));
        }
    }

    let platform = optional_string(&flags, "platform")?
        .map(|value| value.parse())
        .transpose()?;
    let device = optional_string(&flags, "device")?;
    let coordinate = |name: &str| -> Result<Option<f64>, String> {
        let value = optional_number(&flags, name)?;
        if value.is_some_and(|value| !(0.0..=1.0).contains(&value)) {
            return Err(format!("--{name} must be between 0 and 1"));
        }
        Ok(value)
    };
    let x = coordinate("x")?;
    let y = coordinate("y")?;
    let x1 = coordinate("x1")?;
    let y1 = coordinate("y1")?;
    let x2 = coordinate("x2")?;
    let y2 = coordinate("y2")?;
    let ms = optional_non_negative_integer(&flags, "ms")?
        .map(|value| u32::try_from(value).map_err(|_| "--ms is too large".to_string()))
        .transpose()?;
    if ms.is_some_and(|value| {
        !(crate::agent_emulator::EMULATOR_SWIPE_MS_MIN
            ..=crate::agent_emulator::EMULATOR_SWIPE_MS_MAX)
            .contains(&value)
    }) {
        return Err(format!(
            "--ms must be between {} and {}",
            crate::agent_emulator::EMULATOR_SWIPE_MS_MIN,
            crate::agent_emulator::EMULATOR_SWIPE_MS_MAX
        ));
    }
    let text = optional_string_allowing_empty(&flags, "text")?;
    let app = optional_string(&flags, "app")?;
    let name = optional_string(&flags, "name")?;
    let rotation = optional_non_negative_integer(&flags, "rotation")?
        .map(|value| u32::try_from(value).map_err(|_| "--rotation is too large".to_string()))
        .transpose()?;
    let out = optional_string(&flags, "out")?;
    let mark = optional_non_negative_integer(&flags, "mark")?
        .map(|value| {
            usize::try_from(value)
                .ok()
                .filter(|value| (1..=MARK_CAP).contains(value))
                .ok_or_else(|| format!("--mark must be between 1 and {MARK_CAP}"))
        })
        .transpose()?;
    let look = optional_string(&flags, "look")?;

    if method != EmulatorMethod::List && platform.is_none() {
        return Err("missing required --platform".into());
    }
    if !matches!(method, EmulatorMethod::List | EmulatorMethod::Open) && device.is_none() {
        return Err("missing required --device".into());
    }
    match method {
        EmulatorMethod::Find => {
            let value = text.as_deref().ok_or("missing required --text")?;
            if value.trim().is_empty() {
                return Err("--text must not be empty".into());
            }
        }
        EmulatorMethod::Marks | EmulatorMethod::Click
            if text.as_deref().is_some_and(|value| value.trim().is_empty()) =>
        {
            return Err("--text must not be empty".into());
        }
        EmulatorMethod::Foreground => {
            let value = app.as_deref().ok_or("missing required --app")?;
            if value.trim().is_empty() {
                return Err("--app must not be empty".into());
            }
        }
        EmulatorMethod::Click => {
            mark.ok_or("missing required --mark")?;
            look.as_ref().ok_or("missing required --look")?;
        }
        EmulatorMethod::Tap if x.is_none() || y.is_none() => {
            return Err("tap needs both --x and --y".into());
        }
        EmulatorMethod::Swipe if [x1, y1, x2, y2].into_iter().any(|value| value.is_none()) => {
            return Err("swipe needs --x1, --y1, --x2, and --y2".into());
        }
        EmulatorMethod::Text => {
            let value = text.as_deref().ok_or("missing required --text")?;
            if value.is_empty() || value.len() > 4096 || value.chars().any(char::is_control) {
                return Err("--text must be 1–4096 printable bytes".into());
            }
        }
        EmulatorMethod::Button if name.is_none() => {
            return Err("missing required --name".into());
        }
        EmulatorMethod::Rotate => match rotation {
            Some(0..=3) => {}
            _ => return Err("--rotation must be 0, 1, 2, or 3".into()),
        },
        _ => {}
    }

    Ok(EmulatorCommand {
        method,
        app,
        platform,
        device,
        x,
        y,
        x1,
        y1,
        x2,
        y2,
        ms,
        text,
        name,
        rotation,
        out,
        mark,
        look,
        preview: flags.contains_key(EMULATOR_PREVIEW_FLAG),
        json: flags.contains_key("json"),
    })
}

/// The remote and terminal vocabulary, for the same reason the emulator has
/// its own: a machine ZeroCode already knows how to reach must never be
/// reached by clicking ZeroCode's own window through desktop accessibility.
///
/// Every verb here lands in a PANE — a terminal tab the person can watch,
/// scroll and kill. That is deliberate: an SSH session in this app IS a
/// terminal (`SshConnection::open_pty`), so the door that opens one also
/// opens a local shell and a remote workspace, and nothing an agent does on
/// a remote machine happens somewhere nobody can see.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SshMethod {
    List,
    Open,
    Send,
    Read,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SshCommand {
    pub method: SshMethod,
    pub host: Option<String>,
    pub local: bool,
    pub cwd: Option<String>,
    pub pane: Option<u32>,
    pub text: Option<String>,
    pub enter: bool,
    pub rows: Option<u16>,
    pub cols: Option<u16>,
    pub json: bool,
}

/// The grid a pane this door opens is born with. The window resizes it to the
/// real box as soon as the tab is on screen (`resizeTermTab`), so this is the
/// floor a first frame is drawn against, not the size it keeps.
const SSH_DEFAULT_ROWS: u16 = 24;
const SSH_DEFAULT_COLS: u16 = 96;

/// What a pane may be told in one `send`. The same ceiling `--text` carries
/// on the emulator door, for the same reason: a keystroke road is not a file
/// transfer, and a runaway payload is a runaway payload on either surface.
const SSH_MAX_TEXT_BYTES: usize = 4096;

#[must_use]
pub fn ssh_usage() -> String {
    [
        "zerocode-ssh — use the terminals, SSH hosts and remote workspaces ZeroCode owns",
        "",
        "  zerocode-ssh list [--json]",
        "  zerocode-ssh open (--host <id>|--local [--cwd <path>]) [--rows N] [--cols N] [--json]",
        "  zerocode-ssh send --pane <id> (--text <text>|--text-stdin) [--enter] [--json]",
        "  zerocode-ssh read --pane <id> [--json]",
        "",
        "`list` names every saved host and every pane this window holds. `open`",
        "stands a terminal tab in the window and answers with its pane id; every",
        "later verb addresses that id. Remote work stays in a pane the person can",
        "see and stop.",
    ]
    .join("\n")
}

/// Parse the terminal/SSH CLI without forwarding a half-formed instruction.
///
/// `open` insists on exactly one target so a missing `--host` can never be
/// read as "open a local shell instead" — the two reach different machines,
/// and guessing between them is the one mistake this parser exists to refuse.
pub fn parse_ssh_command(argv: &[String]) -> Result<SshCommand, String> {
    let Some(verb) = argv.first().map(String::as_str) else {
        return Err(ssh_usage());
    };
    if matches!(verb, "-h" | "--help" | "help") {
        return Err(ssh_usage());
    }
    let method = match verb {
        "list" => SshMethod::List,
        "open" => SshMethod::Open,
        "send" => SshMethod::Send,
        "read" => SshMethod::Read,
        _ => {
            return Err(format!("unknown ssh command `{verb}`\n\n{}", ssh_usage()));
        }
    };
    let flags = flags(&argv[1..])?;
    let allowed: &[&str] = match method {
        SshMethod::List => &["json"],
        SshMethod::Open => &["json", "host", "local", "cwd", "rows", "cols"],
        SshMethod::Send => &["json", "pane", "text", "enter"],
        SshMethod::Read => &["json", "pane"],
    };
    for name in flags.keys() {
        if !allowed.contains(&name.as_str()) {
            return Err(format!("unknown flag --{name}"));
        }
    }

    let host = optional_string(&flags, "host")?;
    let local = flags.contains_key("local");
    let cwd = optional_string(&flags, "cwd")?;
    let pane = optional_non_negative_integer(&flags, "pane")?
        .map(|value| u32::try_from(value).map_err(|_| "--pane is too large".to_string()))
        .transpose()?;
    let text = optional_string_allowing_empty(&flags, "text")?;
    let enter = flags.contains_key("enter");
    let grid = |name: &str, fallback: u16| -> Result<Option<u16>, String> {
        let Some(value) = optional_non_negative_integer(&flags, name)? else {
            return Ok(None);
        };
        let value = u16::try_from(value).map_err(|_| format!("--{name} is too large"))?;
        if !(1..=1000).contains(&value) {
            return Err(format!("--{name} must be between 1 and 1000"));
        }
        let _ = fallback;
        Ok(Some(value))
    };
    let rows = grid("rows", SSH_DEFAULT_ROWS)?;
    let cols = grid("cols", SSH_DEFAULT_COLS)?;

    match method {
        SshMethod::Open => {
            if host.is_some() == local {
                return Err("open needs exactly one of --host or --local".into());
            }
            if cwd.is_some() && !local {
                return Err("--cwd belongs to --local".into());
            }
        }
        SshMethod::Send => {
            if pane.is_none() {
                return Err("missing required --pane".into());
            }
            let value = text.as_deref().ok_or("missing required --text")?;
            if value.is_empty() || value.len() > SSH_MAX_TEXT_BYTES {
                return Err(format!("--text must be 1–{SSH_MAX_TEXT_BYTES} bytes"));
            }
            // A newline inside the payload would submit whatever stands before
            // it — which is `--enter`'s job, and the caller's decision. Tabs
            // are left alone because a shell completes on them.
            if value.chars().any(|ch| ch.is_control() && ch != '\t') {
                return Err(
                    "--text may not carry control characters; use --enter to submit".into(),
                );
            }
        }
        SshMethod::Read if pane.is_none() => {
            return Err("missing required --pane".into());
        }
        _ => {}
    }

    Ok(SshCommand {
        method,
        host,
        local,
        cwd,
        pane,
        text,
        enter,
        rows,
        cols,
        json: flags.contains_key("json"),
    })
}

impl SshCommand {
    /// The grid a newly opened pane starts on.
    #[must_use]
    pub fn grid(&self) -> (u16, u16) {
        (
            self.rows.unwrap_or(SSH_DEFAULT_ROWS),
            self.cols.unwrap_or(SSH_DEFAULT_COLS),
        )
    }

    /// The words `send` puts on the pane's input, WITHOUT its submission.
    ///
    /// This used to answer `format!("{text}\r")`, and the window wrote that
    /// one string with one call. Glued to its text, a carriage return is not
    /// a submission — it is the last byte of a paste, and an agent TUI that
    /// is still assembling the line either swallows it or takes it as a
    /// newline in a draft. That is the composer a person then found holding
    /// words nobody had sent (live report: `--enter` answered
    /// `submitted: true` over a claude draft that needed a human Enter).
    ///
    /// The submission is [`Self::submits`], and the gap between the two
    /// belongs to the delivery machine that owns readiness and the envelope —
    /// `zerocode_pty::ready::PromptDelivery` — not to a string.
    #[must_use]
    pub fn typed_text(&self) -> String {
        self.text.clone().unwrap_or_default()
    }

    /// Whether this send asked for its line to be SUBMITTED.
    ///
    /// A request, never a receipt. What actually happened to the Enter is the
    /// pane's answer, and the door reports it separately.
    #[must_use]
    pub const fn submits(&self) -> bool {
        self.enter
    }
}

/// The method a CLI word names — the one table `parse_command`, `usage` and
/// the contracts read.
#[must_use]
pub fn verb_method(verb: &str) -> Option<ComputerMethod> {
    Some(match verb {
        "capabilities" => ComputerMethod::Capabilities,
        "list-apps" => ComputerMethod::ListApps,
        "permissions" => ComputerMethod::Permissions,
        "list-windows" => ComputerMethod::ListWindows,
        "get-app-state" => ComputerMethod::GetAppState,
        "click" => ComputerMethod::Click,
        "perform-secondary-action" => ComputerMethod::PerformSecondaryAction,
        "scroll" => ComputerMethod::Scroll,
        "drag" => ComputerMethod::Drag,
        "type-text" => ComputerMethod::TypeText,
        "press-key" => ComputerMethod::PressKey,
        "hotkey" => ComputerMethod::Hotkey,
        "paste-text" => ComputerMethod::PasteText,
        "set-value" => ComputerMethod::SetValue,
        "screenshot" => ComputerMethod::Screenshot,
        "zoom" => ComputerMethod::Zoom,
        "mouse-move" => ComputerMethod::MouseMove,
        "mouse-click" => ComputerMethod::MouseClick,
        "mouse-drag" => ComputerMethod::MouseDrag,
        "mouse-scroll" => ComputerMethod::MouseScroll,
        "cursor-position" => ComputerMethod::CursorPosition,
        "key" => ComputerMethod::Key,
        "hold-key" => ComputerMethod::HoldKey,
        "type" => ComputerMethod::Type,
        "wait" => ComputerMethod::Wait,
        "displays" => ComputerMethod::Displays,
        "launch" => ComputerMethod::Launch,
        "quit" => ComputerMethod::Quit,
        "activate" => ComputerMethod::Activate,
        "open" => ComputerMethod::Open,
        "run" => ComputerMethod::Run,
        "list-all-windows" => ComputerMethod::ListAllWindows,
        "window-focus" => ComputerMethod::WindowFocus,
        "window-move" => ComputerMethod::WindowMove,
        "window-resize" => ComputerMethod::WindowResize,
        "window-minimize" => ComputerMethod::WindowMinimize,
        "window-zoom" => ComputerMethod::WindowZoom,
        "window-close" => ComputerMethod::WindowClose,
        "clipboard-read" => ComputerMethod::ClipboardRead,
        "clipboard-write" => ComputerMethod::ClipboardWrite,
        "find" => ComputerMethod::Find,
        "wait-for" => ComputerMethod::WaitFor,
        "read" => ComputerMethod::Read,
        "stop" => ComputerMethod::Stop,
        "resume" => ComputerMethod::Resume,
        "status" => ComputerMethod::Status,
        "evidence" => ComputerMethod::Evidence,
        "verdict" => ComputerMethod::Verdict,
        "compare" => ComputerMethod::Compare,
        "observe" => ComputerMethod::Observe,
        "handoff" => ComputerMethod::Handoff,
        "recipe-save" => ComputerMethod::RecipeSave,
        "recipe-list" => ComputerMethod::RecipeList,
        "recipe-show" => ComputerMethod::RecipeShow,
        "recipe-run" => ComputerMethod::RecipeRun,
        "listen-start" => ComputerMethod::ListenStart,
        "listen-stop" => ComputerMethod::ListenStop,
        "sound-read" => ComputerMethod::SoundRead,
        "sound-wait" => ComputerMethod::SoundWait,
        "watch" => ComputerMethod::Watch,
        "walk" => ComputerMethod::Walk,
        "reflex-start" => ComputerMethod::ReflexStart,
        "reflex-status" => ComputerMethod::ReflexStatus,
        "reflex-stop" => ComputerMethod::ReflexStop,
        "batch" => ComputerMethod::Batch,
        _ => return None,
    })
}

/// The words of a `recipe-run --repeat` bound (`--until`): a count of rounds,
/// or a time of day on the local clock.
const REPEAT_UNTIL_CLOCK_SEPARATOR: char = ':';
const HOURS_PER_DAY: i64 = 24;
const MINUTES_PER_HOUR: i64 = 60;
const MS_PER_MINUTE: i64 = 60_000;

/// Where a `recipe-run --repeat` ends when the person does not stop it
/// first: after this many rounds, or once the local clock passes this time
/// of day — the next such minute from the repeat's start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepeatUntil {
    Rounds(u64),
    Clock { hour: u8, minute: u8 },
}

impl RepeatUntil {
    /// The `--until` word: a count of rounds (from 1), or `HH:MM`.
    pub fn parse(word: &str) -> Result<Self, String> {
        let grammar = || {
            format!(
                "--until is a count of rounds from 1, or a time of day HH{REPEAT_UNTIL_CLOCK_SEPARATOR}MM (not `{word}`)"
            )
        };
        if let Ok(rounds) = word.parse::<u64>() {
            return if rounds == 0 {
                Err(grammar())
            } else {
                Ok(Self::Rounds(rounds))
            };
        }
        let (hour, minute) = word
            .split_once(REPEAT_UNTIL_CLOCK_SEPARATOR)
            .ok_or_else(grammar)?;
        let (hour, minute) = (
            hour.parse::<u8>().map_err(|_| grammar())?,
            minute.parse::<u8>().map_err(|_| grammar())?,
        );
        if i64::from(hour) >= HOURS_PER_DAY || i64::from(minute) >= MINUTES_PER_HOUR {
            return Err(grammar());
        }
        Ok(Self::Clock { hour, minute })
    }

    /// The count of rounds, when the bound is one.
    #[must_use]
    pub const fn rounds(self) -> Option<u64> {
        match self {
            Self::Rounds(rounds) => Some(rounds),
            Self::Clock { .. } => None,
        }
    }

    /// When a clock bound ends, in epoch milliseconds: the next minute the
    /// local clock (`local_offset_secs` from UTC) reads `HH:MM` after
    /// `now_epoch_ms` — today while it is still ahead, tomorrow once it is
    /// here or past. None for a count of rounds.
    #[must_use]
    pub const fn ends_at_epoch_ms(self, now_epoch_ms: i64, local_offset_secs: i64) -> Option<i64> {
        let Self::Clock { hour, minute } = self else {
            return None;
        };
        let day_ms = HOURS_PER_DAY * MINUTES_PER_HOUR * MS_PER_MINUTE;
        let local_now = now_epoch_ms + local_offset_secs * 1_000;
        let of_day = (hour as i64 * MINUTES_PER_HOUR + minute as i64) * MS_PER_MINUTE;
        let mut ends = local_now.div_euclid(day_ms) * day_ms + of_day;
        if ends <= local_now {
            ends += day_ms;
        }
        Some(ends - local_offset_secs * 1_000)
    }
}

/// Parse the public CLI without guessing or passing unknown flags onward.
pub fn parse_command(argv: &[String]) -> Result<ComputerCommand, String> {
    let Some(verb) = argv.first().map(String::as_str) else {
        return Err(usage());
    };
    if matches!(verb, "-h" | "--help" | "help") {
        return Err(usage());
    }
    let Some(method) = verb_method(verb) else {
        return Err(format!(
            "unknown computer-use command `{verb}`\n\n{}",
            usage()
        ));
    };
    let flags = flags(&argv[1..])?;
    let json = flags.contains_key("json");
    reject_unknown(method, &flags)?;
    if flags.contains_key("instant")
        && let Some(refusal) = crate::computer_use_protocol::reflex::instant_pointer_refusal(
            crate::computer_use_protocol::reflex::capability(
                crate::computer_use_protocol::reflex::Surface::MacosDesktop,
            ),
        )
    {
        return Err(refusal.into());
    }
    let mut params = Map::new();

    if let Some(app) = optional_string(&flags, "app")? {
        params.insert("app".into(), Value::String(app));
    }
    for (flag, key) in [
        ("window-id", "windowId"),
        ("window-index", "windowIndex"),
        ("element-index", "elementIndex"),
        ("click-count", "clickCount"),
        ("from-element-index", "fromElementIndex"),
        ("to-element-index", "toElementIndex"),
        ("display", "display"),
        ("steps", "steps"),
        ("ms", "ms"),
        ("wait-ready", "waitReadyMs"),
        ("timeout-ms", "timeoutMs"),
        ("last", "last"),
        ("after", "after"),
        ("start", "start"),
        ("end", "end"),
        ("mark", "mark"),
        ("seconds", "seconds"),
    ] {
        if let Some(value) = optional_non_negative_integer(&flags, flag)? {
            params.insert(key.into(), json!(value));
        }
    }
    for (flag, key) in [
        ("x", "x"),
        ("y", "y"),
        ("pages", "pages"),
        ("from-x", "fromX"),
        ("from-y", "fromY"),
        ("to-x", "toX"),
        ("to-y", "toY"),
        ("dx", "dx"),
        ("dy", "dy"),
        ("width", "width"),
        ("height", "height"),
        ("max-diff", "maxDiff"),
        ("min-confidence", "minConfidence"),
    ] {
        if let Some(value) = optional_number(&flags, flag)? {
            params.insert(key.into(), json!(value));
        }
    }
    for (flag, key) in [
        ("worktree", "worktree"),
        ("session", "session"),
        ("mouse-button", "mouseButton"),
        ("modifiers", "modifiers"),
        ("action", "action"),
        ("direction", "direction"),
        ("text", "text"),
        ("key", "key"),
        ("value", "value"),
        ("args", "args"),
        ("path", "path"),
        ("url", "url"),
        ("with", "with"),
        ("program", "program"),
        ("cwd", "cwd"),
        ("role", "role"),
        ("label", "label"),
        ("window", "window"),
        ("confirming", "confirming"),
        ("confirm", "confirm"),
        ("reason", "reason"),
        ("viewer", "viewer"),
        ("baseline", "baseline"),
        ("against", "against"),
        ("name", "name"),
        ("from", "from"),
        ("note", "note"),
        ("look", "look"),
        ("until", "until"),
        ("arena", "arena"),
        ("goal", "goal"),
        ("pane", "pane"),
        ("platform", "platform"),
        ("device", "device"),
        ("flow", "flow"),
        ("run", "run"),
    ] {
        if let Some(value) = optional_string_allowing_empty(&flags, flag)? {
            params.insert(key.into(), Value::String(value));
        }
    }
    for (flag, key) in [
        ("restore-window", "restoreWindow"),
        ("no-screenshot", "noScreenshot"),
        ("full-res", "fullRes"),
        ("force", "force"),
        ("ocr", "ocr"),
        ("absent", "absent"),
        ("reset-budget", "resetBudget"),
        ("reset", "reset"),
        ("allow-self", "allowSelf"),
        ("instant", "instant"),
        ("pass", "pass"),
        ("fail", "fail"),
        ("diff", "diff"),
        ("marks", "marks"),
        ("settle", "settle"),
        (
            "all-layers",
            crate::computer_use_protocol::marks::EVERY_LAYER_KEY,
        ),
        ("verify", "verify"),
        ("repeat", "repeat"),
        (WALK_OVERLAP_FLAG, WALK_OVERLAP_PARAM),
        (WALK_RESCUE_FLAG, WALK_RESCUE_PARAM),
        (WALK_REPLAY_FLAG, WALK_REPLAY_PARAM),
        ("renew", "renew"),
    ] {
        if flags.contains_key(flag) {
            params.insert(key.into(), Value::Bool(true));
        }
    }
    if method == ComputerMethod::Permissions
        && let Some(id) = optional_string(&flags, "id")?
    {
        params.insert("id".into(), Value::String(id));
    }
    // A window verb names its window by id and its action by its own name.
    if let Some(action) = method.window_action() {
        if let Some(id) = optional_non_negative_integer(&flags, "id")? {
            params.insert("windowId".into(), json!(id));
        }
        params.insert("action".into(), Value::String(action.to_string()));
    }
    if let Some(region) = optional_string(&flags, "region")? {
        params.insert("region".into(), parse_region(&region)?);
    }
    // A zoom is a screenshot of one region at the screen's own resolution.
    if method == ComputerMethod::Zoom {
        params.insert("fullRes".into(), Value::Bool(true));
    }
    // The ears hear by the window's table: the helper keeps no numbers of its own.
    if method == ComputerMethod::ListenStart {
        params.extend(sound_table());
    }
    // A recipe's values are checked before anything is walked.
    if method == ComputerMethod::RecipeRun
        && let Some(raw) = optional_string(&flags, "params")?
    {
        params.insert(
            "recipeParams".into(),
            Value::Object(crate::computer_recipe::recipe_params(&raw)?),
        );
    }
    // A batch's steps are checked whole, before the first one runs.
    if method == ComputerMethod::Batch
        && let Some(raw) = optional_string(&flags, BATCH_COMMANDS_FLAG)?
    {
        params.insert(BATCH_COMMANDS_FLAG.into(), json!(batch_commands(&raw)?));
    }

    validate(method, &flags, &params)?;
    Ok(ComputerCommand {
        method,
        params: Value::Object(params),
        json,
    })
}

/// Flags whose value is a program's own argument words — dashes and all.
pub(crate) const PROGRAM_WORD_FLAGS: &[&str] = &["args"];

fn flags(argv: &[String]) -> Result<BTreeMap<String, Option<String>>, String> {
    let mut flags = BTreeMap::new();
    let mut at = 0;
    while at < argv.len() {
        let raw = &argv[at];
        let Some(name) = raw.strip_prefix("--") else {
            return Err(format!("unexpected argument `{raw}`"));
        };
        if name.is_empty() {
            return Err("empty flag".into());
        }
        let boolean = matches!(
            name,
            "json"
                | "restore-window"
                | "no-screenshot"
                | "local"
                | "enter"
                | "full-res"
                | "force"
                | "ocr"
                | "absent"
                | "reset-budget"
                | "reset"
                | "allow-self"
                | "instant"
                | "pass"
                | "fail"
                | "diff"
                | "marks"
                | "settle"
                | "all-layers"
                | "verify"
                | "repeat"
                | WALK_OVERLAP_FLAG
                | WALK_RESCUE_FLAG
                | WALK_REPLAY_FLAG
                | EMULATOR_PREVIEW_FLAG
        );
        if flags.contains_key(name) {
            return Err(format!("duplicate --{name}"));
        }
        if boolean {
            flags.insert(name.to_string(), None);
            at += 1;
            continue;
        }
        // A program's own words may start with dashes; `--args` takes the
        // next token whatever it looks like. Every other flag refuses a value
        // that reads like a flag, so a forgotten value is caught, not eaten.
        let carries_program_words = PROGRAM_WORD_FLAGS.contains(&name);
        let value = argv
            .get(at + 1)
            .filter(|value| carries_program_words || !value.starts_with("--"))
            .ok_or_else(|| format!("--{name} needs a value"))?;
        flags.insert(name.to_string(), Some(value.clone()));
        at += 2;
    }
    Ok(flags)
}

pub(crate) fn allowed(method: ComputerMethod) -> &'static [&'static str] {
    const BASIC: &[&str] = &["json"];
    const APP: &[&str] = &["json", "app"];
    const OBSERVE: &[&str] = &[
        "json",
        "app",
        "worktree",
        "session",
        "window-id",
        "window-index",
        "restore-window",
        "no-screenshot",
    ];
    match method {
        ComputerMethod::Capabilities | ComputerMethod::ListApps => BASIC,
        ComputerMethod::Permissions => &["json", "id", "reset"],
        ComputerMethod::ListWindows => APP,
        ComputerMethod::GetAppState => OBSERVE,
        ComputerMethod::Click => &[
            "json",
            "app",
            "worktree",
            "session",
            "window-id",
            "window-index",
            "restore-window",
            "no-screenshot",
            "element-index",
            "x",
            "y",
            "click-count",
            "mouse-button",
            "modifiers",
            "confirming",
            "mark",
            "look",
            "viewer",
            "text",
            "label",
            "role",
        ],
        ComputerMethod::PerformSecondaryAction => &[
            "json",
            "app",
            "worktree",
            "session",
            "window-id",
            "window-index",
            "restore-window",
            "no-screenshot",
            "element-index",
            "action",
            "confirming",
        ],
        ComputerMethod::Scroll => &[
            "json",
            "app",
            "worktree",
            "session",
            "window-id",
            "window-index",
            "restore-window",
            "no-screenshot",
            "element-index",
            "x",
            "y",
            "direction",
            "pages",
        ],
        ComputerMethod::Drag => &[
            "json",
            "app",
            "worktree",
            "session",
            "window-id",
            "window-index",
            "restore-window",
            "no-screenshot",
            "from-element-index",
            "to-element-index",
            "from-x",
            "from-y",
            "to-x",
            "to-y",
        ],
        ComputerMethod::TypeText | ComputerMethod::PasteText => &[
            "json",
            "app",
            "worktree",
            "session",
            "window-id",
            "window-index",
            "restore-window",
            "no-screenshot",
            "text",
        ],
        ComputerMethod::PressKey | ComputerMethod::Hotkey => &[
            "json",
            "app",
            "worktree",
            "session",
            "window-id",
            "window-index",
            "restore-window",
            "no-screenshot",
            "key",
            "confirming",
        ],
        ComputerMethod::SetValue => &[
            "json",
            "app",
            "worktree",
            "session",
            "window-id",
            "window-index",
            "restore-window",
            "no-screenshot",
            "element-index",
            "value",
        ],
        ComputerMethod::Screenshot => &["json", "display", "region", "full-res", "viewer"],
        ComputerMethod::Zoom => &["json", "display", "region"],
        ComputerMethod::MouseMove => &["json", "x", "y", "steps", "instant"],
        ComputerMethod::MouseClick => &[
            "json",
            "x",
            "y",
            "mouse-button",
            "click-count",
            "modifiers",
            "instant",
            "allow-self",
            "confirming",
        ],
        ComputerMethod::MouseDrag => &[
            "json",
            "from-x",
            "from-y",
            "to-x",
            "to-y",
            "steps",
            "instant",
            "allow-self",
            "confirming",
        ],
        ComputerMethod::MouseScroll => &["json", "x", "y", "dx", "dy", "allow-self"],
        ComputerMethod::CursorPosition | ComputerMethod::Displays => BASIC,
        ComputerMethod::Key => &["json", "key", "allow-self", "confirming"],
        ComputerMethod::HoldKey => &["json", "key", "ms", "allow-self", "confirming"],
        ComputerMethod::Type => &["json", "text", "allow-self"],
        ComputerMethod::Wait => &["json", "ms"],
        ComputerMethod::Launch => &["json", "app", "args", "wait-ready"],
        ComputerMethod::Quit => &["json", "app", "force"],
        ComputerMethod::Activate => APP,
        ComputerMethod::Open => &["json", "path", "url", "with"],
        ComputerMethod::Run => &["json", "program", "args", "cwd", "timeout-ms"],
        ComputerMethod::ListAllWindows => &["json", "all-layers"],
        ComputerMethod::ClipboardRead => BASIC,
        ComputerMethod::WindowFocus
        | ComputerMethod::WindowMinimize
        | ComputerMethod::WindowZoom
        | ComputerMethod::WindowClose => &["json", "id"],
        ComputerMethod::WindowMove => &["json", "id", "x", "y"],
        ComputerMethod::WindowResize => &["json", "id", "width", "height"],
        ComputerMethod::ClipboardWrite => &["json", "text"],
        ComputerMethod::Find => &[
            "json",
            "app",
            "text",
            "role",
            "label",
            "window-id",
            "window-index",
            "ocr",
            "display",
            "region",
        ],
        ComputerMethod::WaitFor => &[
            "json",
            "app",
            "text",
            "role",
            "label",
            "window",
            "window-id",
            "ocr",
            "timeout-ms",
            "absent",
            "display",
            "region",
        ],
        ComputerMethod::Read => &[
            "json",
            "app",
            "window-id",
            "window-index",
            "ocr",
            "display",
            "region",
        ],
        ComputerMethod::Stop | ComputerMethod::Status => BASIC,
        ComputerMethod::Resume => &["json", "reset-budget"],
        ComputerMethod::Evidence => &["json", "last", "verify"],
        ComputerMethod::Verdict => &["json", "pass", "fail", "reason"],
        ComputerMethod::Observe => &[
            "json",
            "no-screenshot",
            "app",
            "window-id",
            "window-index",
            "ocr",
            "diff",
            "display",
            "region",
            "viewer",
            "marks",
            "settle",
        ],
        ComputerMethod::Handoff => &["json", "reason", "timeout-ms"],
        ComputerMethod::RecipeSave => &["json", "name", "from", "last", "note"],
        ComputerMethod::RecipeList => BASIC,
        ComputerMethod::RecipeShow => &["json", "name"],
        ComputerMethod::RecipeRun => &[
            "json",
            "name",
            "params",
            "start",
            "end",
            "confirm",
            "repeat",
            "until",
            "arena",
            WALK_RESCUE_FLAG,
        ],
        ComputerMethod::ListenStart => APP,
        ComputerMethod::ListenStop => BASIC,
        ComputerMethod::SoundRead => &["json", "after"],
        ComputerMethod::SoundWait => &["json", "label", "min-confidence", "timeout-ms", "after"],
        ComputerMethod::Watch => &["json", "until", "timeout-ms", "display"],
        ComputerMethod::Batch => &["json", BATCH_COMMANDS_FLAG],
        ComputerMethod::Walk => &[
            "json",
            "goal",
            "app",
            "pane",
            "platform",
            "device",
            "until",
            "steps",
            WALK_OVERLAP_FLAG,
            WALK_RESCUE_FLAG,
            WALK_REPLAY_FLAG,
        ],
        ComputerMethod::Compare => &[
            "json", "baseline", "against", "region", "display", "max-diff",
        ],
        ComputerMethod::ReflexStart => &["json", "flow", "display", "seconds", "renew"],
        ComputerMethod::ReflexStatus | ComputerMethod::ReflexStop => &["json", "run"],
    }
}

/// `x,y,w,h` — four numbers in screen points — as the helper's region object.
fn parse_region(raw: &str) -> Result<Value, String> {
    let parts: Vec<f64> = raw
        .split(',')
        .map(|part| part.trim().parse::<f64>())
        .collect::<Result<_, _>>()
        .map_err(|_| "--region must be x,y,w,h in screen points".to_string())?;
    let [x, y, width, height] = parts.as_slice() else {
        return Err("--region must be x,y,w,h in screen points".into());
    };
    if *width <= 0.0 || *height <= 0.0 {
        return Err("--region needs a positive width and height".into());
    }
    Ok(json!({ "x": x, "y": y, "width": width, "height": height }))
}

fn reject_unknown(
    method: ComputerMethod,
    flags: &BTreeMap<String, Option<String>>,
) -> Result<(), String> {
    for name in flags.keys() {
        if !allowed(method).contains(&name.as_str()) {
            return Err(format!("unknown flag --{name}"));
        }
    }
    Ok(())
}

fn validate(
    method: ComputerMethod,
    flags: &BTreeMap<String, Option<String>>,
    params: &Map<String, Value>,
) -> Result<(), String> {
    if let Some(declared) = params.get("confirming").and_then(Value::as_str)
        && ConfirmKind::parse(declared).is_none()
    {
        return Err(format!(
            "--confirming must be one of payment, transfer, delete (not '{declared}')"
        ));
    }
    let has = |key: &str| params.contains_key(key);
    if has("windowId") && has("windowIndex") {
        return Err("use either --window-id or --window-index, not both".into());
    }
    // A mark names its own app and window: the look it was drawn on.
    let by_mark = method == ComputerMethod::Click && has("mark");
    // A control named by what it reads is found on a fresh tree at the press.
    let by_query = method == ComputerMethod::Click && QUERY_CLICK_KEYS.iter().any(|key| has(key));
    // A method that offers `--pane` names its place that way instead of by an
    // app, and the flag table above is what says which ones do. Read from a
    // second list here, the walk's browser form was in the usage and refused
    // by the gate, so the seat that judges a browser walk could not be reached
    // at all — its ledger had no rows because nothing could call it
    // (2026-09-19).
    let by_pane = allowed(method).contains(&"pane") && has("pane");
    let by_device = allowed(method).contains(&"device") && (has("device") || has("platform"));
    if !matches!(
        method,
        ComputerMethod::Capabilities | ComputerMethod::ListApps | ComputerMethod::Permissions
    ) && !method.is_desktop()
        && !has("app")
        && !by_mark
        && !by_pane
        && !by_device
    {
        return Err("missing required --app".into());
    }
    match method {
        ComputerMethod::Permissions => {
            if flags.contains_key("reset") && !flags.contains_key("id") {
                return Err("permissions --reset requires --id accessibility|screenshots".into());
            }
            if let Some(id) = optional_string(flags, "id")? {
                let _: ComputerPermissionId = id.parse()?;
            }
        }
        ComputerMethod::Click | ComputerMethod::Scroll => {
            if by_mark {
                validate_mark_click(params)?;
            } else if by_query {
                validate_query_click(params)?;
            } else {
                if has("look") {
                    return Err("--look names the marked look a --mark was read from".into());
                }
                validate_element_or_coordinates(has("elementIndex"), has("x"), has("y"))?;
            }
            if method == ComputerMethod::Scroll {
                match params.get("direction").and_then(Value::as_str) {
                    Some("up" | "down" | "left" | "right") => {}
                    _ => return Err("--direction must be up, down, left, or right".into()),
                }
                if params
                    .get("pages")
                    .and_then(Value::as_f64)
                    .is_some_and(|value| value <= 0.0)
                {
                    return Err("--pages must be positive".into());
                }
            }
            if let Some(button) = params.get("mouseButton").and_then(Value::as_str)
                && !matches!(button, "left" | "right" | "middle")
            {
                return Err("--mouse-button must be left, right, or middle".into());
            }
            if let Some(count) = params.get("clickCount").and_then(Value::as_u64)
                && count == 0
            {
                return Err("--click-count must be positive".into());
            }
        }
        ComputerMethod::PerformSecondaryAction => {
            require(params, "elementIndex", "--element-index")?;
            require(params, "action", "--action")?;
        }
        ComputerMethod::Drag => {
            let elements = has("fromElementIndex") && has("toElementIndex");
            let partial_elements = has("fromElementIndex") || has("toElementIndex");
            let coordinates = ["fromX", "fromY", "toX", "toY"];
            let all_coordinates = coordinates.iter().all(|key| has(key));
            let partial_coordinates = coordinates.iter().any(|key| has(key));
            if elements == all_coordinates
                || (partial_elements && !elements)
                || (partial_coordinates && !all_coordinates)
            {
                return Err(
                    "drag needs both element indexes or all four coordinates, not a mixture".into(),
                );
            }
        }
        ComputerMethod::TypeText | ComputerMethod::PasteText => {
            let text = params
                .get("text")
                .and_then(Value::as_str)
                .ok_or("missing required --text")?;
            if text.is_empty() {
                return Err("--text may not be empty".into());
            }
        }
        ComputerMethod::PressKey => validate_press_key(
            params
                .get("key")
                .and_then(Value::as_str)
                .ok_or("missing required --key")?,
        )?,
        ComputerMethod::Hotkey => validate_hotkey(
            params
                .get("key")
                .and_then(Value::as_str)
                .ok_or("missing required --key")?,
        )?,
        ComputerMethod::SetValue => {
            require(params, "elementIndex", "--element-index")?;
            require(params, "value", "--value")?;
        }
        ComputerMethod::MouseMove | ComputerMethod::MouseClick | ComputerMethod::MouseScroll => {
            if !(has("x") && has("y")) {
                return Err("this verb needs both --x and --y in screen points".into());
            }
            // A stepped move glides in one round trip; zero steps is no move.
            if method == ComputerMethod::MouseMove
                && params
                    .get("steps")
                    .and_then(Value::as_u64)
                    .is_some_and(|steps| steps == 0)
            {
                return Err("--steps must be positive".into());
            }
            if method == ComputerMethod::MouseScroll && !(has("dx") || has("dy")) {
                return Err("mouse-scroll needs --dx and/or --dy".into());
            }
            if let Some(button) = params.get("mouseButton").and_then(Value::as_str)
                && !matches!(button, "left" | "right" | "middle")
            {
                return Err("--mouse-button must be left, right, or middle".into());
            }
            if let Some(count) = params.get("clickCount").and_then(Value::as_u64)
                && count == 0
            {
                return Err("--click-count must be positive".into());
            }
        }
        ComputerMethod::MouseDrag => {
            if !["fromX", "fromY", "toX", "toY"].iter().all(|key| has(key)) {
                return Err("mouse-drag needs all four coordinates".into());
            }
            if params
                .get("steps")
                .and_then(Value::as_u64)
                .is_some_and(|steps| steps == 0)
            {
                return Err("--steps must be positive".into());
            }
        }
        ComputerMethod::Zoom => require(params, "region", "--region")?,
        ComputerMethod::Key | ComputerMethod::HoldKey => {
            let key = params
                .get("key")
                .and_then(Value::as_str)
                .ok_or("missing required --key")?;
            if key.trim().is_empty() {
                return Err("--key may not be empty".into());
            }
            if method == ComputerMethod::HoldKey {
                match params.get("ms").and_then(Value::as_u64) {
                    Some(ms) if ms > 0 => {}
                    _ => return Err("hold-key needs a positive --ms".into()),
                }
            }
        }
        ComputerMethod::Type => {
            let text = params
                .get("text")
                .and_then(Value::as_str)
                .ok_or("missing required --text")?;
            if text.is_empty() {
                return Err("--text may not be empty".into());
            }
        }
        ComputerMethod::Wait => match params.get("ms").and_then(Value::as_u64) {
            Some(ms) if ms > 0 => {}
            _ => return Err("wait needs a positive --ms".into()),
        },
        ComputerMethod::Open => {
            if has("path") == has("url") {
                return Err("open needs exactly one of --path or --url".into());
            }
        }
        ComputerMethod::Run => {
            let program = params
                .get("program")
                .and_then(Value::as_str)
                .ok_or("missing required --program")?;
            if program.trim().is_empty() {
                return Err("--program may not be empty".into());
            }
        }
        ComputerMethod::WindowFocus
        | ComputerMethod::WindowMinimize
        | ComputerMethod::WindowZoom
        | ComputerMethod::WindowClose => require(params, "windowId", "--id")?,
        ComputerMethod::WindowMove => {
            require(params, "windowId", "--id")?;
            if !(has("x") && has("y")) {
                return Err("window-move needs both --x and --y".into());
            }
        }
        ComputerMethod::WindowResize => {
            require(params, "windowId", "--id")?;
            let side = |key: &str| params.get(key).and_then(Value::as_f64);
            match (side("width"), side("height")) {
                (Some(width), Some(height)) if width > 0.0 && height > 0.0 => {}
                _ => return Err("window-resize needs a positive --width and --height".into()),
            }
        }
        ComputerMethod::ClipboardWrite => {
            if params.get("text").and_then(Value::as_str).is_none() {
                return Err("missing required --text".into());
            }
        }
        ComputerMethod::Find => {
            if !(has("text") || has("role") || has("label")) {
                return Err("find needs --text, --role or --label".into());
            }
            if !has("app") && !has("ocr") {
                return Err("find needs --app, or --ocr to read the screen's pixels".into());
            }
        }
        ComputerMethod::WaitFor => {
            if !(has("text") || has("role") || has("label") || has("window")) {
                return Err("wait-for needs --text, --role, --label or --window <title>".into());
            }
            if has("window") && (has("text") || has("role") || has("label")) {
                return Err(
                    "wait-for watches either a window title or an element, not both".into(),
                );
            }
            if !has("window") && !has("app") && !has("ocr") {
                return Err(
                    "wait-for needs --app, --ocr to read the screen's pixels, or --window <title>"
                        .into(),
                );
            }
        }
        ComputerMethod::Read => {
            if !has("app") && !has("ocr") {
                return Err("read needs --app, or --ocr to read the screen's pixels".into());
            }
        }
        ComputerMethod::Capabilities
        | ComputerMethod::ListApps
        | ComputerMethod::ListWindows
        | ComputerMethod::GetAppState
        | ComputerMethod::Screenshot
        | ComputerMethod::CursorPosition
        | ComputerMethod::Displays
        | ComputerMethod::Launch
        | ComputerMethod::Quit
        | ComputerMethod::Activate
        | ComputerMethod::ListAllWindows
        | ComputerMethod::ClipboardRead
        | ComputerMethod::Stop
        | ComputerMethod::Resume
        | ComputerMethod::Status
        | ComputerMethod::Evidence => {}
        ComputerMethod::Verdict => {
            if has("pass") == has("fail") {
                return Err("verdict needs exactly one of --pass or --fail".into());
            }
            if has("fail") && !has("reason") {
                return Err("a failing verdict needs --reason".into());
            }
        }
        ComputerMethod::Observe | ComputerMethod::RecipeList => {}
        ComputerMethod::ListenStart | ComputerMethod::ListenStop | ComputerMethod::SoundRead => {}
        ComputerMethod::Batch => require(
            params,
            BATCH_COMMANDS_FLAG,
            &format!("--{BATCH_COMMANDS_FLAG}"),
        )?,
        ComputerMethod::Watch => {
            if let Some(until) = params.get("until").and_then(Value::as_str)
                && !WATCH_UNTIL.contains(&until)
            {
                return Err(format!("--until is one of {}", WATCH_UNTIL.join(", ")));
            }
        }
        ComputerMethod::SoundWait => {
            if let Some(bar) = params.get("minConfidence").and_then(Value::as_f64)
                && !(0.0..=1.0).contains(&bar)
            {
                return Err("--min-confidence is a share between 0 and 1".into());
            }
        }
        ComputerMethod::Walk => {
            let goal = params
                .get("goal")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|goal| !goal.is_empty())
                .ok_or("a walk needs --goal: one sentence saying what to reach")?;
            if goal.chars().count() > crate::jev::GOAL_CHAR_CAP {
                return Err(format!(
                    "--goal is at most {} characters — past a sentence a goal is a plan, and a plan is a recipe",
                    crate::jev::GOAL_CHAR_CAP
                ));
            }
            if has("device") != has("platform") {
                return Err("a mobile walk requires both --platform and --device".into());
            }
            if let Some(platform) = params.get("platform").and_then(Value::as_str) {
                platform.parse::<EmulatorPlatform>()?;
                if params
                    .get("device")
                    .and_then(Value::as_str)
                    .is_none_or(|id| id.trim().is_empty())
                {
                    return Err("--device needs a device id".into());
                }
            }
            if ["app", "pane", "device"]
                .into_iter()
                .filter(|key| has(key))
                .count()
                != 1
            {
                return Err(
                    "a walk looks at one screen: name --app <app>, --pane <browser pane>, or --platform <ios|android> --device <id>"
                        .into(),
                );
            }
            if let Some(steps) = params.get("steps").and_then(Value::as_u64)
                && (steps == 0 || steps > WALK_STEPS_MAX as u64)
            {
                return Err(format!("--steps is between 1 and {WALK_STEPS_MAX}"));
            }
            if params
                .get("until")
                .and_then(Value::as_str)
                .is_some_and(|until| until.trim().is_empty())
            {
                return Err("--until is the text that is on screen when it worked".into());
            }
        }
        ComputerMethod::ReflexStart => {
            require(
                params,
                "flow",
                "--flow <a Flow document with its reflex sections>",
            )?;
            require(params, "display", "--display N")?;
            require(params, "seconds", "--seconds N")?;
            // The seconds become the run policy's nanoseconds here, at the door:
            // none, too many to count, or past the table's longest run is refused
            // before anything reaches a helper.
            let seconds = params.get("seconds").and_then(Value::as_u64).unwrap_or(0);
            crate::computer_use_protocol::reflex::RunPolicy::for_seconds(seconds, has("renew"))
                .map_err(|_| {
                    format!(
                        "--seconds is a whole number from 1 to {}",
                        crate::computer_use_protocol::reflex::LIMITS.max_run_ns / 1_000_000_000
                    )
                })?;
        }
        ComputerMethod::ReflexStatus | ComputerMethod::ReflexStop => {
            require(params, "run", "--run <run id>")?;
            if !params
                .get("run")
                .and_then(Value::as_str)
                .is_some_and(crate::computer_use_protocol::reflex::identifier)
            {
                return Err(format!(
                    "--run is 1 to {} of A-Z a-z 0-9 - _, as reflex-start answered it",
                    crate::computer_use_protocol::reflex::MAX_IDENTIFIER_BYTES
                ));
            }
        }
        ComputerMethod::RecipeSave | ComputerMethod::RecipeShow | ComputerMethod::RecipeRun => {
            for flag in ["start", "end"] {
                if params.get(flag).and_then(Value::as_u64) == Some(0) {
                    return Err(format!("--{flag} counts steps from 1"));
                }
            }
            if has("end") && has("repeat") {
                return Err("--end selects a partial walk and cannot be repeated".into());
            }
            let name = params
                .get("name")
                .and_then(Value::as_str)
                .ok_or("a recipe needs --name")?;
            if recipe_slug(name).is_empty() {
                return Err("--name needs at least one letter or digit".into());
            }
            // A repeat's bound needs its repeat; an arena needs its folder.
            if let Some(until) = params.get("until").and_then(Value::as_str) {
                if !has("repeat") {
                    return Err("--until bounds a --repeat: say --repeat too".into());
                }
                RepeatUntil::parse(until)?;
            }
            if params
                .get("arena")
                .and_then(Value::as_str)
                .is_some_and(|dir| dir.trim().is_empty())
            {
                return Err("--arena needs the recorded evidence folder".into());
            }
        }
        ComputerMethod::Handoff => {
            let reason = params
                .get("reason")
                .and_then(Value::as_str)
                .ok_or("handoff needs --reason: what the person has to do")?;
            if reason.trim().is_empty() {
                return Err("--reason may not be empty".into());
            }
        }
        ComputerMethod::Compare => {
            require(params, "baseline", "--baseline")?;
            if let Some(bar) = params.get("maxDiff").and_then(Value::as_f64)
                && !(0.0..=1.0).contains(&bar)
            {
                return Err("--max-diff is a share between 0 and 1".into());
            }
        }
    }
    Ok(())
}

/// The flags a click by mark refuses: the mark's look already names the
/// app, the window, the element and the helper namespace it is pressed in.
const MARK_CLICK_REFUSES: &[(&str, &str)] = &[
    ("app", "--app"),
    ("windowId", "--window-id"),
    ("windowIndex", "--window-index"),
    ("elementIndex", "--element-index"),
    ("x", "--x"),
    ("y", "--y"),
    ("session", "--session"),
    ("worktree", "--worktree"),
    ("restoreWindow", "--restore-window"),
    ("text", "--text"),
    ("label", "--label"),
    ("role", "--role"),
];

/// What a control named by what it reads takes no other name by.
const QUERY_CLICK_REFUSES: &[(&str, &str)] = &[
    ("elementIndex", "--element-index"),
    ("x", "--x"),
    ("y", "--y"),
    ("look", "--look"),
];

/// `click --app A --text T | --label L | --role R [--label L]`: the control by
/// what it reads, and nothing that names it another way. A word must read
/// something — an empty `--text` would match every control.
fn validate_query_click(params: &Map<String, Value>) -> Result<(), String> {
    for (key, flag) in QUERY_CLICK_REFUSES {
        if params.contains_key(*key) {
            return Err(format!(
                "a control named by what it reads takes no {flag}: drop it, or drop --text/--label/--role"
            ));
        }
    }
    let reads = QUERY_CLICK_KEYS.iter().any(|key| {
        params
            .get(*key)
            .and_then(Value::as_str)
            .is_some_and(|word| !word.trim().is_empty())
    });
    if !reads {
        return Err("--text, --label or --role must read something".into());
    }
    Ok(())
}

/// `click --mark N --look L`: a number from the named look, and nothing that
/// would say again what the mark already says.
fn validate_mark_click(params: &Map<String, Value>) -> Result<(), String> {
    if params.get("mark").and_then(Value::as_u64) == Some(0) {
        return Err("--mark counts from 1".into());
    }
    if params
        .get("look")
        .and_then(Value::as_str)
        .is_none_or(|look| look.trim().is_empty())
    {
        return Err(
            "--mark needs --look <lookId>: the marked look its number was read from".into(),
        );
    }
    for (key, flag) in MARK_CLICK_REFUSES {
        if params.contains_key(*key) {
            return Err(format!(
                "a mark names its own app, window and element: drop {flag}"
            ));
        }
    }
    Ok(())
}

fn validate_element_or_coordinates(element: bool, x: bool, y: bool) -> Result<(), String> {
    if !(element || x && y) {
        return Err("action needs --element-index or both --x and --y".into());
    }
    if x != y || (element && (x || y)) {
        return Err("use either --element-index or both coordinates, not both".into());
    }
    Ok(())
}

fn require(params: &Map<String, Value>, key: &str, flag: &str) -> Result<(), String> {
    params
        .contains_key(key)
        .then_some(())
        .ok_or_else(|| format!("missing required {flag}"))
}

fn optional_string(
    flags: &BTreeMap<String, Option<String>>,
    name: &str,
) -> Result<Option<String>, String> {
    let value = optional_string_allowing_empty(flags, name)?;
    if value.as_deref() == Some("") {
        return Err(format!("--{name} may not be empty"));
    }
    Ok(value)
}

fn optional_string_allowing_empty(
    flags: &BTreeMap<String, Option<String>>,
    name: &str,
) -> Result<Option<String>, String> {
    match flags.get(name) {
        None => Ok(None),
        Some(Some(value)) => Ok(Some(value.clone())),
        Some(None) => Err(format!("--{name} needs a value")),
    }
}

fn optional_number(
    flags: &BTreeMap<String, Option<String>>,
    name: &str,
) -> Result<Option<f64>, String> {
    optional_string(flags, name)?
        .map(|value| {
            value
                .parse::<f64>()
                .ok()
                .filter(|value| value.is_finite())
                .ok_or_else(|| format!("--{name} must be a finite number"))
        })
        .transpose()
}

fn optional_non_negative_integer(
    flags: &BTreeMap<String, Option<String>>,
    name: &str,
) -> Result<Option<u64>, String> {
    optional_string(flags, name)?
        .map(|value| {
            value
                .parse::<u64>()
                .map_err(|_| format!("--{name} must be a non-negative integer"))
        })
        .transpose()
}

fn validate_press_key(key: &str) -> Result<(), String> {
    if key.trim().is_empty() || key.contains('+') {
        return Err(
            "press-key accepts one key without modifiers; use hotkey for combinations".into(),
        );
    }
    Ok(())
}

fn validate_hotkey(key: &str) -> Result<(), String> {
    let parts = key.split('+').map(str::trim).collect::<Vec<_>>();
    if parts.len() < 2 || parts.iter().any(|part| part.is_empty()) {
        return Err("hotkey requires modifiers plus one key, such as CmdOrCtrl+A".into());
    }
    let modifiers = &parts[..parts.len() - 1];
    if modifiers.iter().any(|part| {
        !matches!(
            part.to_ascii_lowercase().as_str(),
            "cmdorctrl"
                | "commandorcontrol"
                | "cmd"
                | "command"
                | "ctrl"
                | "control"
                | "shift"
                | "alt"
                | "option"
                | "meta"
                | "super"
        )
    }) {
        return Err("hotkey contains an unsupported modifier".into());
    }
    Ok(())
}

/// The command's concise built-in manual. The installed skill carries the
/// safety workflow; `--help` carries the vocabulary.
#[must_use]
pub fn usage() -> String {
    let text = [
        "zerocode-computer — inspect and operate local desktop apps",
        "",
        "  zerocode-computer capabilities [--json]",
        "  zerocode-computer permissions [--id accessibility|screenshots [--reset]] [--json]",
        "  zerocode-computer list-apps [--json]",
        "  zerocode-computer list-windows --app <name|bundle|pid:N> [--json]",
        "  zerocode-computer get-app-state --app <app> [--window-id N|--window-index N] [--restore-window] [--no-screenshot] [--json]",
        "  zerocode-computer click --app <app> (--element-index N|--x X --y Y|--text <t>|--role <r> [--label <l>]) [--mouse-button left|right|middle] [--click-count N] [--modifiers chord] [--json]",
        "    --text/--label/--role name the control by what it reads (find's matcher), read off a fresh tree right before the press —",
        "    the target a batch step may name after earlier steps; one match presses, none or several answers which",
        "  zerocode-computer click --mark N --look <lookId> [--mouse-button left|right|middle] [--click-count N] [--modifiers chord] [--viewer <id>] [--json]",
        "    presses the control numbered N on the marked look <lookId> (observe --marks) through the element path; refused if it moved or changed since",
        "  zerocode-computer perform-secondary-action --app <app> --element-index N --action <name> [--json]",
        "  zerocode-computer scroll --app <app> (--element-index N|--x X --y Y) --direction <up|down|left|right> [--json]",
        "  zerocode-computer drag --app <app> (--from-element-index N --to-element-index N|--from-x X --from-y Y --to-x X --to-y Y) [--json]",
        "  zerocode-computer type-text --app <app> (--text text|--text-stdin) [--json]",
        "  zerocode-computer press-key --app <app> --key <key> [--json]",
        "  zerocode-computer hotkey --app <app> --key <modifier+key> [--json]",
        "  zerocode-computer paste-text --app <app> (--text text|--text-stdin) [--json]",
        "  zerocode-computer set-value --app <app> --element-index N (--value text|--value-stdin) [--json]",
        "",
        "  the desktop, no app named — screen points, the mouse and the keys where a person has them:",
        "  zerocode-computer screenshot [--display N] [--region x,y,w,h] [--full-res] [--viewer <id>] [--json]",
        "  zerocode-computer zoom --region x,y,w,h [--display N] [--json]",
        "  zerocode-computer mouse-move --x X --y Y [--steps N] [--instant] [--json]",
        "  zerocode-computer mouse-click --x X --y Y [--mouse-button left|right|middle] [--click-count N] [--modifiers chord] [--instant] [--json]",
        "  zerocode-computer mouse-drag --from-x X --from-y Y --to-x X --to-y Y [--steps N] [--instant] [--json]",
        "  zerocode-computer mouse-scroll --x X --y Y [--dx N] [--dy N] [--json]",
        "  zerocode-computer cursor-position [--json]",
        "  zerocode-computer key --key <key|modifier+key> [--json]",
        "  zerocode-computer hold-key --key <key> --ms N [--json]",
        "  zerocode-computer type (--text text|--text-stdin) [--json]",
        "  zerocode-computer wait --ms N [--json]",
        "  zerocode-computer displays [--json]",
        "",
        "  apps, windows and the system — what a person does around an app:",
        "  zerocode-computer launch --app <name|bundle-id|path> [--args <words>] [--wait-ready ms] [--json]",
        "  zerocode-computer quit --app <app> [--force] [--json]",
        "  zerocode-computer activate --app <app> [--json]",
        "  zerocode-computer open (--path <file> | --url <address>) [--with <app>] [--json]",
        "  zerocode-computer run --program <path> [--args <words>] [--cwd <dir>] [--timeout-ms N] [--json]",
        "  zerocode-computer list-all-windows [--all-layers] [--json]",
        "  zerocode-computer window-focus|window-minimize|window-zoom|window-close --id <window-id> [--json]",
        "  zerocode-computer window-move --id <window-id> --x X --y Y [--json]",
        "  zerocode-computer window-resize --id <window-id> --width W --height H [--json]",
        "  zerocode-computer clipboard-read [--json]",
        "  zerocode-computer clipboard-write --text <text> [--json]",
        "",
        "  the meaning layer — find, wait for, read (--ocr reads pixels when the tree is empty):",
        "  zerocode-computer find (--app <app> | --ocr [--display N] [--region x,y,w,h]) (--text <t> | --role <r> [--label <l>]) [--json]",
        "  zerocode-computer wait-for (--text <t> | --role <r> [--label <l>]) (--app <app> | --ocr) [--timeout-ms N] [--absent] [--json]",
        "  zerocode-computer wait-for --window <title fragment> [--timeout-ms N] [--absent] [--json]",
        "  zerocode-computer read (--app <app> [--window-id N] | --ocr [--display N] [--region x,y,w,h]) [--json]",
        "",
        "  the one hand on the operator — a stop refuses every action until resume, looks still answer:",
        "  zerocode-computer stop [--json]            (same as the desktop hotkey control+option+escape)",
        "  zerocode-computer resume [--reset-budget] [--json]",
        "  zerocode-computer status [--json]",
        "  zerocode-computer evidence [--last N] [--verify] [--json]   (this session's folder and its last steps; --verify reproduces a Flow's verdict from the folder)",
        "",
        "  the eyes and the person's turn:",
        "  zerocode-computer observe [--app <app> [--window-id N]] [--ocr] [--diff] [--marks] [--settle] [--display N] [--region x,y,w,h] [--viewer <id>] [--json]",
        "      (one look: the tree when an app is named, the frame, the text with --ocr, what changed since the last look with --diff;",
        "       --viewer keeps that last look your own when other agents look at the same screen;",
        "       --marks numbers the controls of one window on the picture and answers marks {lookId, items} for click --mark;",
        "       --settle first waits for what the last act did to finish painting, at most a second)",
        "  zerocode-computer watch [--until change|quiet] [--timeout-ms N] [--display N] [--json]",
        "      (waits for the screen to change, or to go still, from the display's repaints — no pictures taken;",
        "       answers where it changed; ZeroCode's own windows and what was already moving do not count)",
        "  zerocode-computer handoff --reason <what the person must do> [--timeout-ms N] [--json]",
        "      (2FA, CAPTCHA, the last step you may not press: the window shows a card and waits for the person)",
        "  zerocode-computer recipe-save --name <name> [--from <evidence dir>] [--last N] [--note <text>] [--json]",
        "      (this session's walked steps as a document a person can read and edit; next time, walk the recipe first)",
        "  zerocode-computer recipe-list [--json]",
        "  zerocode-computer recipe-show --name <name> [--json]",
        "  zerocode-computer recipe-run --name <name> [--params '{\"name\":\"value\"}'] [--start N] [--end N] [--confirm <txn>] [--repeat [--until <HH:MM|N>]] [--arena <evidence dir>] [--rescue] [--json]",
        "  zerocode-computer walk --goal <what to reach> (--app <app> | --pane <browser pane> | --platform <ios|android> --device <id>) [--until <text on screen when it worked>] [--steps N] [--overlap] [--rescue] [--replay] [--json]",
        "      (--replay: this walk repeats one walked before, so a question it asked then is answered from the judgment memo)",
        "      (walks the steps in one call, filling {{name}} from --params; stops at the person's turn or last step,",
        "       a step naming the saved screen's element or window, a check the screen fails, an act that changed",
        "       nothing, or a person's hand on the pointer — and answers the step to resume from; a guarded Flow's",
        "       money step waits for --confirm <txn>, the person's word on the transaction the card names;",
        "       --repeat walks round after round — each when the document's `## Trigger` line flips, at once without one —",
        "       until --until (a time of day, or N rounds), a stop, a hand on the pointer, or the person's whole turn;",
        "       --arena rehearses the walk against a recorded evidence folder: every step is answered as recorded,",
        "       nothing on a desk moves, and the evidence lands in a folder of its own)",
        "",
        "  a live reflex run — a Flow document's reflex plan, run by the helper's one hand until its deadline;",
        "  macOS only, and only with the live reflex setting on (capabilities and reflex-status say liveReflex",
        "  {supported, enabled}: whether this Mac and its helper can run one, and whether the setting lets it):",
        "  zerocode-computer reflex-start --flow <Flow document> --display N --seconds N [--renew] [--json]",
        "      (answers {runId, state} at once; the run holds the hand, and every verb that acts waits for it to end;",
        "       --renew gives a rule its max_fires back once they are spent, on a new edge — the deadline never moves)",
        "  zerocode-computer reflex-status --run <run id> [--json]",
        "  zerocode-computer reflex-stop --run <run id> [--json]      (the operator's stop ends every run)",
        "",
        "  the ears — the machine's sound, through the screen-recording permission the eyes already hold:",
        "  zerocode-computer listen-start [--app <app>] [--json]      (the whole machine, or one app's sound)",
        "  zerocode-computer sound-read [--after N] [--json]          (what the classifier heard: label, confidence, seq)",
        "  zerocode-computer sound-wait [--label siren,alarm_clock] [--min-confidence 0.5] [--timeout-ms N] [--after N] [--json]",
        "  zerocode-computer listen-stop [--json]",
        "",
        "  QA — the scenario's verdict, and a screen against a baseline:",
        "  zerocode-computer verdict (--pass | --fail --reason <why>) [--json]   (into the run's evidence folder)",
        "  zerocode-computer compare --baseline <png> [--against <png>] [--region x,y,w,h] [--display N] [--max-diff 0.01] [--json]",
        "",
        "  the last step (payment, transfer, delete) is the person's: a press that lands on such a control",
        "  is held while the window asks them — say --confirming <kind> when you know you are there.",
    ]
    .join("\n");
    let pace = pace_words(COMPUTER_PACE);
    format!(
        "{text}\n\n  one judgement, many actions — the hand's steps in order, each stopped, confirmed and logged as if alone (pace: {pace}):\n  zerocode-computer batch --{BATCH_COMMANDS_FLAG} '[[\"mouse-click\",\"--x\",\"10\",\"--y\",\"20\"],[\"type\",\"--text\",\"hi\"]]' [--json]\n      (at most {COMPUTER_BATCH_MAX_STEPS} steps, waiting {COMPUTER_BATCH_MAX_WAIT_MS} ms together; stops at the first refusal; look after it)"
    )
}

/// The launched-agent-only shim. Tokens travel through a 0600 curl config,
/// stdin payloads stay off process argv, and curl cannot inherit proxy/config
/// routes that would exfiltrate either capability.
pub fn shim_script(port_var: &str, computer_token_var: &str, hook_token_var: &str) -> String {
    let manual = usage();
    computer_door(&manual, port_var, computer_token_var, hook_token_var).render_posix()
}

/// The Computer Use door on the chassis every `/computer` door shares. It
/// alone says where its caller stands ([`CWD_HEADER`]), for the verbs the
/// window judges by that folder ([`CWD_VERBS`]): a recipe walk is asked
/// through it, and a stopped walk is judged only for a workspace the person
/// consented to. The emulator and terminal doors have no use for the folder.
fn computer_door<'a>(
    manual: &'a str,
    port_var: &'a str,
    computer_token_var: &'a str,
    hook_token_var: &'a str,
) -> PowerShellBridgeShim<'a> {
    PowerShellBridgeShim {
        cwd_verbs: CWD_VERBS,
        ..computer_route_shim(
            COMPUTER_CLI,
            manual,
            "",
            port_var,
            computer_token_var,
            hook_token_var,
        )
    }
}

#[must_use]
pub fn emulator_usage() -> String {
    [
        "zerocode-emulator — control ZeroCode's built-in iOS and Android surfaces",
        "",
        "  zerocode-emulator list [--json]",
        "  zerocode-emulator open --platform ios|android [--device <id>] [--json]",
        "  zerocode-emulator tree --platform ios|android --device <id> [--json]",
        "  zerocode-emulator marks --platform ios|android --device <id> [--text <fragment>] [--json]",
        "    --text also counts, in the same look, what find would (count; 0 means absent).",
        "  zerocode-emulator find --platform ios|android --device <id> --text <fragment> [--json]",
        "  zerocode-emulator foreground --platform ios|android --device <id> --app <package|bundle> [--json]",
        "    Checks answer count (0 means absent). find matches a case-insensitive name fragment.",
        "    foreground requires an exported app package; unavailable metadata is an error (including iOS).",
        "  zerocode-emulator click --platform ios|android --device <id> --mark <n> --look <id> [--text <fragment>] [--preview] [--json]",
        "    On iOS a click answers once the screen it led to stops changing; --text counts what find would",
        "    in the tree it stopped on (count; 0 is not proof of absence — look with marks --text); --preview",
        "    answers the marks that screen would carry (not a look: look again before pressing one).",
        "  zerocode-emulator tap --platform ios|android --device <id> --x <0..1> --y <0..1> [--json]",
        "  zerocode-emulator swipe --platform ios|android --device <id> --x1 N --y1 N --x2 N --y2 N [--ms N] [--json]",
        "  zerocode-emulator text --platform ios|android --device <id> (--text <text>|--text-stdin) [--json]",
        "  zerocode-emulator button --platform ios|android --device <id> --name <button> [--json]",
        "  zerocode-emulator rotate --platform ios|android --device <id> --rotation <0..3> [--json]",
        "  zerocode-emulator screenshot --platform ios|android --device <id> [--out <path>] [--json]",
        "    A relative --out is taken from the shell's own folder; without --out, a private scratch file.",
    ]
    .join("\n")
}

pub fn emulator_shim_script(
    port_var: &str,
    computer_token_var: &str,
    hook_token_var: &str,
) -> String {
    let (manual, prefix) = (emulator_usage(), format!("emulator{ARGV_SEPARATOR}"));
    emulator_door(
        &manual,
        &prefix,
        port_var,
        computer_token_var,
        hook_token_var,
    )
    .render_posix()
}

/// The emulator door on the shared chassis: it says where its caller stands
/// for the verbs that write where they are told ([`EMULATOR_CWD_VERBS`]), and
/// which pane asked, in the browser door's own header — the mirror an agent
/// opens is seated in that pane's checkout, not beside whatever the person
/// happens to be looking at (t-6379).
fn emulator_door<'a>(
    manual: &'a str,
    prefix: &'a str,
    port_var: &'a str,
    computer_token_var: &'a str,
    hook_token_var: &'a str,
) -> PowerShellBridgeShim<'a> {
    PowerShellBridgeShim {
        cwd_verbs: EMULATOR_CWD_VERBS,
        pane_header: Some(crate::agent_browser::PANE_HEADER),
        ..computer_route_shim(
            "zerocode-emulator",
            manual,
            prefix,
            port_var,
            computer_token_var,
            hook_token_var,
        )
    }
}

/// The agent's road to this window's own terminals — local shells, saved SSH
/// hosts, remote workspaces and remote servers. The third route on the same
/// guarded bridge, for the reason the second one exists: the surface ZeroCode
/// already owns must not be reached by clicking ZeroCode.
pub fn ssh_shim_script(port_var: &str, computer_token_var: &str, hook_token_var: &str) -> String {
    let (manual, prefix) = (ssh_usage(), format!("ssh{ARGV_SEPARATOR}"));
    computer_route_shim(
        "zerocode-ssh",
        &manual,
        &prefix,
        port_var,
        computer_token_var,
        hook_token_var,
    )
    .render_posix()
}

/// The chassis every `/computer` door shares — the Computer Use route and
/// capability, the deadline of its longest command, the stdin road — rendered
/// by each door for a POSIX host or a PowerShell one.
fn computer_route_shim<'a>(
    command: &'a str,
    manual: &'a str,
    prefix: &'a str,
    port_var: &'a str,
    computer_token_var: &'a str,
    hook_token_var: &'a str,
) -> PowerShellBridgeShim<'a> {
    PowerShellBridgeShim {
        command,
        manual,
        prefix,
        route: "/computer",
        port_var,
        capability_var: computer_token_var,
        capability_header: "x-zerocode-computer-token",
        capability_missing: "this pane has no Computer Use capability",
        hook_token_var,
        deadline_seconds: COMPUTER_LONGEST_DEADLINE_MS.div_ceil(1_000),
        stdin_flags: true,
        pane_header: None,
        cwd_verbs: &[],
        cwd_flag: None,
    }
}

/// The same door for a shell that cannot read a shebang.
///
/// On Windows an agent's tool call arrives from one of three hosts: Git Bash
/// reads the POSIX script above; PowerShell runs the `.ps1` it finds on PATH
/// and cmd resolves the `.cmd` through PATHEXT, and neither can run a file
/// with no extension at all. This is the PowerShell body; the `.cmd` beside
/// it is `agent_teams::shim_cmd`. The choice of PowerShell over a `.cmd`
/// around `curl.exe` is the one the ledger door already made and wrote down
/// (`agent_teams::shim_script_powershell`): a verb is typed when an agent has
/// something to say, and posting in-process keeps both tokens off argv and
/// off disk. Every edge of the POSIX script is kept: `--help` answers
/// locally, a missing bridge or capability fails closed on stderr, a
/// separator inside an argument or in stdin is refused, no proxy stands
/// between a pane and its window, the deadline is the bridge's, and the
/// answer goes where the status says.
pub(crate) struct PowerShellBridgeShim<'a> {
    pub command: &'a str,
    pub manual: &'a str,
    pub prefix: &'a str,
    pub route: &'a str,
    pub port_var: &'a str,
    pub capability_var: &'a str,
    pub capability_header: &'a str,
    pub capability_missing: &'a str,
    pub hook_token_var: &'a str,
    pub deadline_seconds: u64,
    /// Whether `--text-stdin`/`--value-stdin` read the payload from stdin.
    pub stdin_flags: bool,
    /// Whether the pane key rides along as a header (the browser and
    /// emulator doors seat an agent-opened tab in the asking pane's
    /// checkout).
    pub pane_header: Option<&'a str>,
    /// The verbs whose request carries the shell's working directory as a
    /// header ([`CWD_HEADER`], spelled in hex) — for a window that must know
    /// which workspace such a command was asked from. Empty for a door whose
    /// window never asks.
    pub cwd_verbs: &'a [&'a str],
    /// Optional argv flag carrying the shell's working directory.
    pub cwd_flag: Option<&'a str>,
}

impl PowerShellBridgeShim<'_> {
    pub(crate) fn render_posix(&self) -> String {
        let Self {
            command,
            manual,
            prefix,
            route,
            port_var,
            capability_var,
            hook_token_var,
            capability_header,
            capability_missing,
            deadline_seconds,
            ..
        } = self;
        let cwd_args = self.cwd_flag.map_or(String::new(), |flag| {
            format!("set -- \"$@\" '{flag}' \"$PWD\"\n")
        });
        // The pane key rides as a header where the door seats a pane (the
        // browser's), through the same guarded file as the tokens.
        let pane_lines = self.pane_header.map_or(String::new(), |header| {
            format!(
                "if [ -n \"${{{pane}:-}}\" ]; then\n  printf 'header = \"{header}: %s\"\\n' \"${pane}\" >> \"$headers\"\nfi\n",
                pane = crate::hook::PANE_KEY_ENV
            )
        });
        // Where the caller stands rides the same file, for the verbs the
        // window judges by it, as hex digits only: `od -v` spells every byte
        // (never a `*` for a repeated line) and `tr` joins its lines. A
        // spelling that fails sends no folder, never a guess.
        let cwd_lines = if self.cwd_verbs.is_empty() {
            String::new()
        } else {
            format!(
                "case \"${{1:-}}\" in\n  {verbs})\n    cwd=$(printf '%s' \"${{PWD:-}}\" | od -An -v -tx1 2>/dev/null | tr -d ' \\n') || cwd=\"\"\n    if [ -n \"$cwd\" ]; then\n      printf 'header = \"{CWD_HEADER}: %s\"\\n' \"$cwd\" >> \"$headers\"\n    fi\n    ;;\nesac\n",
                verbs = self.cwd_verbs.join("|")
            )
        };
        let usage_lines = manual
            .lines()
            .map(|line| format!("  echo '{}'", line.replace('\'', "'\\''")))
            .collect::<Vec<_>>()
            .join("\n");
        format!(
            r#"#!/usr/bin/env sh
set -eu
if [ "${{1:-}}" = "--help" ] || [ "${{1:-}}" = "-h" ]; then
{usage_lines}
  exit 0
fi
port="${{{port_var}:-}}"
capability="${{{capability_var}:-}}"
hook_token="${{{hook_token_var}:-}}"
if [ -z "$port" ] || [ -z "$hook_token" ]; then
  echo "{command}: this shell is not inside a ZeroCode window" >&2
  exit 1
fi
if [ -z "$capability" ]; then
  echo "{command}: {capability_missing}" >&2
  exit 1
fi
if ! command -v curl >/dev/null 2>&1; then
  echo "{command}: curl is not installed" >&2
  exit 1
fi
sep=$(printf '\037')
body="{prefix}"
stdin_name=""
{cwd_args}for arg in "$@"; do
  case "$arg" in
    --text-stdin) stdin_name="text"; continue ;;
    --value-stdin) stdin_name="value"; continue ;;
    *"$sep"*)
      echo "{command}: an argument may not contain the separator byte" >&2
      exit 1
      ;;
  esac
  body="$body$arg$sep"
done
if [ -n "$stdin_name" ]; then
  payload=$(cat)
  case "$payload" in
    *"$sep"*)
      echo "{command}: stdin may not contain the separator byte" >&2
      exit 1
      ;;
  esac
  body="$body--$stdin_name$sep$payload$sep"
fi
headers=$(mktemp) || exit 1
trap 'rm -f "$headers"' EXIT
chmod 600 "$headers"
printf 'header = "x-zerocode-hook-token: %s"\nheader = "{capability_header}: %s"\n' \
  "$hook_token" "$capability" > "$headers"
if [ -n "${{{RUN_EVIDENCE_DIR_ENV}:-}}" ]; then
  printf 'header = "{RUN_EVIDENCE_HEADER}: %s"\n' "${RUN_EVIDENCE_DIR_ENV}" >> "$headers"
fi
{pane_lines}{cwd_lines}out=$(printf '%s' "$body" | curl -q -sS --noproxy '*' --proto =http \
  --max-time {deadline_seconds} -w '\n%{{http_code}}' \
  -K "$headers" -H "content-type: application/octet-stream" --data-binary @- \
  "http://127.0.0.1:$port{route}" 2>/dev/null) || {{
  echo "{command}: could not reach the window" >&2
  exit 1
}}
code="${{out##*
}}"
said="${{out%
*}}"
if [ "$code" != "200" ]; then
  printf '%s' "$said" >&2
  exit 1
fi
printf '%s' "$said"
"#
        )
    }

    pub(crate) fn render(&self) -> String {
        let Self {
            command,
            manual,
            prefix,
            route,
            port_var,
            capability_var,
            capability_header,
            capability_missing,
            hook_token_var,
            deadline_seconds,
            stdin_flags,
            pane_header,
            cwd_verbs,
            cwd_flag,
        } = self;
        let cwd_args = cwd_flag.map_or(String::new(), |flag| {
            format!("$args = @($args) + @('{flag}', (Get-Location).Path)\n")
        });
        let deadline_ms = deadline_seconds * 1000;
        let stdin_arms = if *stdin_flags {
            "  if ($word -eq '--text-stdin') { $stdinName = 'text'; continue }\n  if ($word -eq '--value-stdin') { $stdinName = 'value'; continue }\n"
        } else {
            ""
        };
        let pane_lines = pane_header.map_or(String::new(), |header| {
            format!(
                "  $pane = $env:{PANE_KEY_ENV}\n  if ($pane) {{ $request.Headers.Add('{header}', $pane) }}\n",
                PANE_KEY_ENV = crate::hook::PANE_KEY_ENV
            )
        });
        // The same verbs, matched as `case` matches them (with case), and the
        // filesystem's spelling of the location (a mapped drive's root, not
        // its name) as hex digits in capitals.
        let cwd_lines = if cwd_verbs.is_empty() {
            String::new()
        } else {
            let verbs = cwd_verbs
                .iter()
                .map(|verb| format!("'{verb}'"))
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "  if (@({verbs}) -ccontains [string]$args[0]) {{\n    $cwd = (Get-Location).ProviderPath\n    if ($cwd) {{ $request.Headers.Add('{CWD_HEADER}', [BitConverter]::ToString([System.Text.Encoding]::UTF8.GetBytes($cwd)).Replace('-', '')) }}\n  }}\n"
            )
        };
        format!(
            r#"# Generated by ZeroCode: {command} for PowerShell-hosted agents. The POSIX
# sibling beside this file serves sh (Git Bash) and every unix; cmd reaches
# this one through {command}.cmd. It posts in-process, so the tokens touch
# no argv and no file, and it fails closed: a window it cannot reach is a
# word on stderr and exit 1.
try {{ [Console]::OutputEncoding = [System.Text.Encoding]::UTF8 }} catch {{}}
try {{ [Console]::InputEncoding = [System.Text.Encoding]::UTF8 }} catch {{}}
if ($args.Count -gt 0 -and ((([string]$args[0]) -eq '--help') -or (([string]$args[0]) -eq '-h'))) {{
  [Console]::Out.WriteLine(@'
{manual}
'@)
  exit 0
}}
$port = $env:{port_var}
$capability = $env:{capability_var}
$hookToken = $env:{hook_token_var}
if (-not $port -or -not $hookToken) {{
  [Console]::Error.WriteLine('{command}: this shell is not inside a ZeroCode window')
  exit 1
}}
if (-not $capability) {{
  [Console]::Error.WriteLine('{command}: {capability_missing}')
  exit 1
}}
$sep = [string][char]0x1f
$body = '{prefix}'
$stdinName = ''
{cwd_args}foreach ($arg in $args) {{
  $word = [string]$arg
{stdin_arms}  if ($word.Contains($sep)) {{
    [Console]::Error.WriteLine('{command}: an argument may not contain the separator byte')
    exit 1
  }}
  $body = $body + $word + $sep
}}
if ($stdinName) {{
  $payload = [Console]::In.ReadToEnd()
  if ($payload.Contains($sep)) {{
    [Console]::Error.WriteLine('{command}: stdin may not contain the separator byte')
    exit 1
  }}
  $body = $body + '--' + $stdinName + $sep + $payload + $sep
}}
$bytes = [System.Text.Encoding]::UTF8.GetBytes($body)
try {{
  $request = [System.Net.HttpWebRequest]::Create("http://127.0.0.1:$port{route}")
  $request.Method = 'POST'
  $request.Proxy = $null
  $request.Timeout = {deadline_ms}
  $request.ReadWriteTimeout = {deadline_ms}
  $request.ContentType = 'application/octet-stream'
  $request.Headers.Add('x-zerocode-hook-token', $hookToken)
  $request.Headers.Add('{capability_header}', $capability)
  $evidence = $env:{RUN_EVIDENCE_DIR_ENV}
  if ($evidence) {{ $request.Headers.Add('{RUN_EVIDENCE_HEADER}', $evidence) }}
{pane_lines}{cwd_lines}  $asking = $request.GetRequestStream()
  $asking.Write($bytes, 0, $bytes.Length)
  $asking.Close()
  $response = $request.GetResponse()
  $reader = New-Object System.IO.StreamReader($response.GetResponseStream(), [System.Text.Encoding]::UTF8)
  $said = $reader.ReadToEnd()
  $response.Close()
  [Console]::Out.Write($said)
  exit 0
}} catch {{
  # PowerShell hands a .NET method's exception over wrapped in a
  # MethodInvocationException, so the WebException carrying the bridge's
  # answer is one or more InnerException hops down. Walk to it: a refusal
  # with the window's own sentence must reach the agent as that sentence.
  $failed = $_.Exception
  while ($null -ne $failed -and -not ($failed -is [System.Net.WebException])) {{
    $failed = $failed.InnerException
  }}
  $answer = if ($null -ne $failed) {{ $failed.Response }} else {{ $null }}
  if ($null -ne $answer) {{
    $reader = New-Object System.IO.StreamReader($answer.GetResponseStream(), [System.Text.Encoding]::UTF8)
    [Console]::Error.Write($reader.ReadToEnd())
    exit 1
  }}
  [Console]::Error.WriteLine('{command}: could not reach the window')
  exit 1
}}
"#
        )
    }
}

/// `zerocode-computer.ps1` — [`shim_script`] for a PowerShell host.
pub fn shim_script_powershell(
    port_var: &str,
    computer_token_var: &str,
    hook_token_var: &str,
) -> String {
    let manual = usage();
    computer_door(&manual, port_var, computer_token_var, hook_token_var).render()
}

/// `zerocode-emulator.ps1` — [`emulator_shim_script`] for a PowerShell host.
pub fn emulator_shim_script_powershell(
    port_var: &str,
    computer_token_var: &str,
    hook_token_var: &str,
) -> String {
    let (manual, prefix) = (emulator_usage(), format!("emulator{ARGV_SEPARATOR}"));
    emulator_door(
        &manual,
        &prefix,
        port_var,
        computer_token_var,
        hook_token_var,
    )
    .render()
}

/// `zerocode-ssh.ps1` — [`ssh_shim_script`] for a PowerShell host.
pub fn ssh_shim_script_powershell(
    port_var: &str,
    computer_token_var: &str,
    hook_token_var: &str,
) -> String {
    let (manual, prefix) = (ssh_usage(), format!("ssh{ARGV_SEPARATOR}"));
    computer_route_shim(
        "zerocode-ssh",
        &manual,
        &prefix,
        port_var,
        computer_token_var,
        hook_token_var,
    )
    .render()
}

pub fn pack_argv(argv: &[String]) -> String {
    let mut body = String::new();
    for arg in argv {
        body.push_str(arg);
        body.push(ARGV_SEPARATOR);
    }
    body
}

#[cfg(test)]
mod windows_shim_tests {
    use super::*;

    /// The PowerShell doors keep every edge the POSIX doors have, and speak
    /// to the same route with the same headers — read off the string, since
    /// only Windows can run it.
    #[test]
    fn the_powershell_doors_keep_the_posix_contract_without_curl() {
        let computer = shim_script_powershell("PORT_V", "COMPUTER_V", "HOOK_V");
        let timeout = format!(
            "$request.Timeout = {}",
            COMPUTER_LONGEST_DEADLINE_MS.div_ceil(1_000) * 1_000
        );
        for expected in [
            "$env:PORT_V",
            "$env:COMPUTER_V",
            "$env:HOOK_V",
            "http://127.0.0.1:$port/computer",
            "'x-zerocode-hook-token', $hookToken",
            "'x-zerocode-computer-token', $capability",
            "$request.Proxy = $null",
            timeout.as_str(),
            "--text-stdin",
            "--value-stdin",
            "[Console]::In.ReadToEnd()",
            "this pane has no Computer Use capability",
            "an argument may not contain the separator byte",
            "stdin may not contain the separator byte",
            "could not reach the window",
            "$env:ZEROCODE_RUN_EVIDENCE_DIR",
            "'x-zerocode-run-evidence', $evidence",
            "if (@('recipe-run', 'walk') -ccontains [string]$args[0]) {",
            "(Get-Location).ProviderPath",
            "'x-zerocode-cwd', [BitConverter]::ToString([System.Text.Encoding]::UTF8.GetBytes($cwd)).Replace('-', '')",
            "zerocode-computer — inspect and operate local desktop apps",
        ] {
            assert!(
                computer.contains(expected),
                "computer door lost {expected}:\n{computer}"
            );
        }
        assert!(
            !computer.contains("curl"),
            "the PowerShell door posts in-process"
        );
        assert!(
            !computer.contains("x-zerocode-pane"),
            "the Computer Use door opens nothing the person sees, so it seats no pane"
        );
        assert!(computer.contains("$body = ''"));

        let emulator = emulator_shim_script_powershell("P", "C", "H");
        assert!(
            emulator.contains(&format!("$body = 'emulator{ARGV_SEPARATOR}'")),
            "{emulator}"
        );
        assert!(
            emulator.contains("$env:ZEROCODE_PANE_KEY")
                && emulator.contains("'x-zerocode-pane', $pane"),
            "the emulator door seats the mirror it opens in the asking pane's checkout:\n{emulator}"
        );
        assert!(emulator.contains("zerocode-emulator — control ZeroCode"));
        let ssh = ssh_shim_script_powershell("P", "C", "H");
        assert!(
            ssh.contains(&format!("$body = 'ssh{ARGV_SEPARATOR}'")),
            "{ssh}"
        );
        assert!(ssh.contains("zerocode-ssh — use the terminals, SSH hosts"));
        assert!(ssh.contains("http://127.0.0.1:$port/computer"));
    }

    /// The manual is a here-string, so an apostrophe in the prose is not a
    /// syntax error — and no manual line may close the here-string early.
    #[test]
    fn the_manual_cannot_close_its_own_here_string() {
        for manual in [usage(), emulator_usage(), ssh_usage()] {
            assert!(manual.lines().all(|line| line.trim() != "'@"), "{manual}");
        }
    }
}

/// Machine-readable errors keep a code that a skill can recover from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ComputerErrorEnvelope<'a> {
    pub ok: bool,
    pub error: ComputerErrorBody<'a>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ComputerErrorBody<'a> {
    pub code: &'a str,
    pub message: &'a str,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn computer_use_permissions_reset_is_explicit_and_targeted() {
        let command =
            parse_command(&words(&["permissions", "--id", "screenshots", "--reset"])).unwrap();
        assert_eq!(command.params, json!({"id": "screenshots", "reset": true}));
        assert!(parse_command(&words(&["permissions", "--reset"])).is_err());
        assert!(parse_command(&words(&["permissions", "--id", "camera", "--reset"])).is_err());
        assert!(parse_command(&words(&["screenshot", "--reset"])).is_err());
    }

    fn words(input: &[&str]) -> Vec<String> {
        input.iter().map(|word| (*word).to_string()).collect()
    }

    /// A click by a mark's number names the look it read, and nothing the
    /// mark already says; a look carries marks only when asked.
    #[test]
    fn a_click_names_its_control_by_what_it_reads_and_a_batch_may_after_any_step() {
        let command = parse_command(&words(&[
            "click",
            "--app",
            "Mail",
            "--text",
            "Send",
            "--role",
            "button",
            "--label",
            "Send now",
            "--mouse-button",
            "right",
            "--click-count",
            "2",
        ]))
        .unwrap();
        assert_eq!(command.method, ComputerMethod::Click);
        assert_eq!(
            command.params,
            json!({ "app": "Mail", "text": "Send", "role": "button", "label": "Send now", "mouseButton": "right", "clickCount": 2 })
        );
        assert!(parse_command(&words(&["click", "--app", "Mail", "--role", "button"])).is_ok());
        for (argv, word) in [
            (&["click", "--text", "Send"][..], "--app"),
            (
                &["click", "--app", "Mail", "--text", " "][..],
                "read something",
            ),
            (
                &["click", "--app", "Mail", "--label", ""][..],
                "read something",
            ),
            (
                &[
                    "click",
                    "--app",
                    "Mail",
                    "--text",
                    "Send",
                    "--element-index",
                    "4",
                ][..],
                "--element-index",
            ),
            (
                &[
                    "click", "--app", "Mail", "--text", "Send", "--x", "1", "--y", "2",
                ][..],
                "--x",
            ),
            (
                &["click", "--app", "Mail", "--text", "Send", "--look", "L"][..],
                "--look",
            ),
            (
                &["click", "--mark", "3", "--look", "L", "--text", "Send"][..],
                "--text",
            ),
            (
                &[
                    "scroll",
                    "--app",
                    "Mail",
                    "--text",
                    "Send",
                    "--direction",
                    "down",
                ][..],
                "--text",
            ),
        ] {
            let error = parse_command(&words(argv)).expect_err(word);
            assert!(error.contains(word), "{argv:?}: {error}");
        }
        // Read at the press, never stale: a batch names its controls after
        // steps that rebuild the tree — where an index is refused.
        let planned = batch(&json!([
            ["click", "--app", "Notes", "--text", "New Note"],
            ["type-text", "--app", "Notes", "--text", "hello"],
            [
                "click", "--app", "Notes", "--role", "button", "--label", "Done"
            ],
            [
                "wait-for",
                "--app",
                "Notes",
                "--text",
                "Saved",
                "--timeout-ms",
                "2000"
            ],
        ]))
        .expect("a plan in one call");
        assert_eq!(batch_steps(&planned).len(), 4);
        assert!(
            batch(&json!([
                ["click", "--app", "Notes", "--text", "New Note"],
                ["click", "--app", "Notes", "--element-index", "4"],
            ]))
            .expect_err("stale index")
            .contains("element indexes change")
        );
        assert!(usage().contains("--text <t>|--role <r> [--label <l>]"));
    }

    #[test]
    fn a_click_by_mark_names_its_look_and_one_target() {
        let command = parse_command(&words(&[
            "click",
            "--mark",
            "7",
            "--look",
            "4242.3",
            "--mouse-button",
            "right",
            "--viewer",
            "zo-1",
        ]))
        .unwrap();
        assert_eq!(command.method, ComputerMethod::Click);
        assert_eq!(
            command.params,
            json!({ "mark": 7, "look": "4242.3", "mouseButton": "right", "viewer": "zo-1" })
        );
        for (argv, words_) in [
            (
                &["click", "--mark", "0", "--look", "L"][..],
                "counts from 1",
            ),
            (&["click", "--mark", "3"][..], "--look"),
            (&["click", "--mark", "3", "--look", " "][..], "--look"),
            (
                &["click", "--mark", "3", "--look", "L", "--app", "X"][..],
                "--app",
            ),
            (
                &[
                    "click", "--mark", "3", "--look", "L", "--x", "1", "--y", "2",
                ][..],
                "--x",
            ),
            (
                &[
                    "click",
                    "--mark",
                    "3",
                    "--look",
                    "L",
                    "--element-index",
                    "4",
                ][..],
                "--element-index",
            ),
            (
                &["click", "--mark", "3", "--look", "L", "--window-id", "9"][..],
                "--window-id",
            ),
            (
                &["click", "--mark", "3", "--look", "L", "--session", "s"][..],
                "--session",
            ),
            (
                &["click", "--app", "X", "--element-index", "2", "--look", "L"][..],
                "--look",
            ),
        ] {
            let refused = parse_command(&words(argv)).unwrap_err();
            assert!(refused.contains(words_), "{argv:?}: {refused}");
        }
        let marked =
            parse_command(&words(&["observe", "--app", "Mail", "--marks", "--diff"])).unwrap();
        assert_eq!(marked.params["marks"], true);
        assert!(
            parse_command(&words(&["observe", "--marks"])).is_ok(),
            "a desktop look too"
        );
        assert!(
            parse_command(&words(&[
                "click",
                "--mark",
                "2",
                "--look",
                "L",
                "--no-screenshot"
            ]))
            .is_ok(),
            "a batch step carries --no-screenshot; the mark click sets it anyway"
        );
        let batch = r#"[["click","--mark","2","--look","L"]]"#;
        assert!(
            parse_command(&words(&["batch", "--commands", batch])).is_ok(),
            "a mark may be a batch's first step"
        );
        assert!(
            parse_command(&words(&["screenshot", "--marks"])).is_err(),
            "only a look is marked"
        );
        assert!(
            parse_command(&words(&[
                "scroll", "--app", "X", "--mark", "2", "--look", "L"
            ]))
            .is_err()
        );
        let usage = usage();
        assert!(usage.contains("click --mark N --look <lookId>") && usage.contains("--marks"));
    }

    /// A zo process's looks follow the core's switch unless its environment
    /// says otherwise, in the words the other zo switches take.
    #[test]
    fn a_process_turns_its_marked_looks_on_or_off() {
        assert_eq!(looks_carry_marks(None), COMPUTER_LOOKS_CARRY_MARKS);
        assert_eq!(looks_carry_marks(Some("maybe")), COMPUTER_LOOKS_CARRY_MARKS);
        assert_eq!(looks_carry_marks(Some("")), COMPUTER_LOOKS_CARRY_MARKS);
        for on in ["1", "on", " ON ", "true", "yes"] {
            assert!(looks_carry_marks(Some(on)), "{on:?}");
        }
        for off in ["0", "off", "False", "no"] {
            assert!(!looks_carry_marks(Some(off)), "{off:?}");
        }
    }

    /// Apps, windows and the system (§2.2): the verbs a person uses around an
    /// app — each names its helper method (or the window, for `run`), the
    /// window verbs share one method and name their action, and the tables
    /// cap a run's time and output.
    #[test]
    fn the_app_window_and_system_verbs_name_their_methods_and_actions() {
        let cases: &[(&[&str], ComputerMethod, Option<&str>, serde_json::Value)] = &[
            (
                &[
                    "launch",
                    "--app",
                    "TextEdit",
                    "--args",
                    "--new-doc",
                    "--wait-ready",
                    "3000",
                ],
                ComputerMethod::Launch,
                Some("launchApp"),
                json!({ "app": "TextEdit", "args": "--new-doc", "waitReadyMs": 3000 }),
            ),
            (
                &["quit", "--app", "TextEdit", "--force"],
                ComputerMethod::Quit,
                Some("quitApp"),
                json!({ "app": "TextEdit", "force": true }),
            ),
            (
                &["activate", "--app", "Finder"],
                ComputerMethod::Activate,
                Some("activateApp"),
                json!({ "app": "Finder" }),
            ),
            (
                &["open", "--url", "https://example.test/", "--with", "Safari"],
                ComputerMethod::Open,
                Some("openTarget"),
                json!({ "url": "https://example.test/", "with": "Safari" }),
            ),
            (
                &["open", "--path", "/tmp/a.pdf"],
                ComputerMethod::Open,
                Some("openTarget"),
                json!({ "path": "/tmp/a.pdf" }),
            ),
            (
                &[
                    "run",
                    "--program",
                    "/bin/echo",
                    "--args",
                    "hi there",
                    "--cwd",
                    "/tmp",
                    "--timeout-ms",
                    "500",
                ],
                ComputerMethod::Run,
                None,
                json!({ "program": "/bin/echo", "args": "hi there", "cwd": "/tmp", "timeoutMs": 500 }),
            ),
            (
                &["list-all-windows", "--json"],
                ComputerMethod::ListAllWindows,
                Some("listAllWindows"),
                json!({}),
            ),
            (
                &["window-focus", "--id", "77"],
                ComputerMethod::WindowFocus,
                Some("windowAction"),
                json!({ "windowId": 77, "action": "focus" }),
            ),
            (
                &["window-move", "--id", "77", "--x", "10", "--y", "20"],
                ComputerMethod::WindowMove,
                Some("windowAction"),
                json!({ "windowId": 77, "action": "move", "x": 10.0, "y": 20.0 }),
            ),
            (
                &[
                    "window-resize",
                    "--id",
                    "77",
                    "--width",
                    "800",
                    "--height",
                    "600",
                ],
                ComputerMethod::WindowResize,
                Some("windowAction"),
                json!({ "windowId": 77, "action": "resize", "width": 800.0, "height": 600.0 }),
            ),
            (
                &["window-minimize", "--id", "77"],
                ComputerMethod::WindowMinimize,
                Some("windowAction"),
                json!({ "windowId": 77, "action": "minimize" }),
            ),
            (
                &["window-zoom", "--id", "77"],
                ComputerMethod::WindowZoom,
                Some("windowAction"),
                json!({ "windowId": 77, "action": "zoom" }),
            ),
            (
                &["window-close", "--id", "77"],
                ComputerMethod::WindowClose,
                Some("windowAction"),
                json!({ "windowId": 77, "action": "close" }),
            ),
            (
                &["clipboard-read"],
                ComputerMethod::ClipboardRead,
                Some("clipboardRead"),
                json!({}),
            ),
            (
                &["clipboard-write", "--text", "hello"],
                ComputerMethod::ClipboardWrite,
                Some("clipboardWrite"),
                json!({ "text": "hello" }),
            ),
        ];
        for (argv, method, provider, params) in cases {
            let command =
                parse_command(&words(argv)).unwrap_or_else(|error| panic!("{argv:?}: {error}"));
            assert_eq!(&command.method, method, "{argv:?}");
            assert_eq!(command.method.provider_name(), *provider, "{argv:?}");
            assert_eq!(command.params, *params, "{argv:?}");
        }
        assert!(
            parse_command(&words(&["open", "--path", "/a", "--url", "https://b"])).is_err(),
            "one target only"
        );
        assert!(
            parse_command(&words(&["open"])).is_err(),
            "a target is needed"
        );
        assert!(
            parse_command(&words(&["window-move", "--id", "1", "--x", "3"])).is_err(),
            "both coordinates"
        );
        assert!(
            parse_command(&words(&[
                "window-resize",
                "--id",
                "1",
                "--width",
                "0",
                "--height",
                "5"
            ]))
            .is_err(),
            "positive sides"
        );
        assert!(
            parse_command(&words(&["window-focus"])).is_err(),
            "a window id is needed"
        );
        assert!(
            parse_command(&words(&["run"])).is_err(),
            "a program is needed"
        );
        assert!(
            parse_command(&words(&["launch"])).is_err(),
            "an app is needed"
        );
        assert_eq!(desktop_run_ms(None), (COMPUTER_RUN_MAX_MS, false));
        assert_eq!(desktop_run_ms(Some(500)), (500, false));
        assert_eq!(
            desktop_run_ms(Some(COMPUTER_RUN_MAX_MS + 1)),
            (COMPUTER_RUN_MAX_MS, true)
        );
        let (kept, cut) = desktop_run_output(&vec![b'x'; COMPUTER_RUN_MAX_BYTES + 10]);
        assert!(cut && kept.len() == COMPUTER_RUN_MAX_BYTES);
        assert_eq!(desktop_run_output(b"ok\n"), ("ok\n".to_string(), false));
        assert!(usage().contains("window-resize") && usage().contains("clipboard-write"));
    }

    /// The meaning layer (§2.3): find, wait-for and read name their helper
    /// methods (wait-for is the window's), need something to look for and
    /// somewhere to look, and the tables bound the watch and the read.
    #[test]
    fn find_wait_for_and_read_need_a_target_and_a_place_to_look() {
        let cases: &[(&[&str], ComputerMethod, Option<&str>, serde_json::Value)] = &[
            (
                &["find", "--app", "Safari", "--text", "Sign in"],
                ComputerMethod::Find,
                Some("findElements"),
                json!({ "app": "Safari", "text": "Sign in" }),
            ),
            (
                &[
                    "find", "--app", "Safari", "--role", "button", "--label", "Buy",
                ],
                ComputerMethod::Find,
                Some("findElements"),
                json!({ "app": "Safari", "role": "button", "label": "Buy" }),
            ),
            (
                &[
                    "find",
                    "--ocr",
                    "--text",
                    "Total",
                    "--region",
                    "0,0,800,600",
                ],
                ComputerMethod::Find,
                Some("findElements"),
                json!({ "ocr": true, "text": "Total", "region": { "x": 0.0, "y": 0.0, "width": 800.0, "height": 600.0 } }),
            ),
            (
                &[
                    "wait-for",
                    "--app",
                    "Mail",
                    "--text",
                    "Sent",
                    "--timeout-ms",
                    "3000",
                    "--absent",
                ],
                ComputerMethod::WaitFor,
                None,
                json!({ "app": "Mail", "text": "Sent", "timeoutMs": 3000, "absent": true }),
            ),
            (
                &["wait-for", "--window", "Preferences"],
                ComputerMethod::WaitFor,
                None,
                json!({ "window": "Preferences" }),
            ),
            (
                &["wait-for", "--ocr", "--text", "Done"],
                ComputerMethod::WaitFor,
                None,
                json!({ "ocr": true, "text": "Done" }),
            ),
            (
                &["read", "--app", "Notes", "--window-id", "12"],
                ComputerMethod::Read,
                Some("readText"),
                json!({ "app": "Notes", "windowId": 12 }),
            ),
            (
                &["read", "--ocr", "--display", "1"],
                ComputerMethod::Read,
                Some("readText"),
                json!({ "ocr": true, "display": 1 }),
            ),
        ];
        for (argv, method, provider, params) in cases {
            let command =
                parse_command(&words(argv)).unwrap_or_else(|error| panic!("{argv:?}: {error}"));
            assert_eq!(&command.method, method, "{argv:?}");
            assert_eq!(command.method.provider_name(), *provider, "{argv:?}");
            assert_eq!(command.params, *params, "{argv:?}");
        }
        assert!(
            parse_command(&words(&["find", "--app", "Safari"])).is_err(),
            "something to look for"
        );
        assert!(
            parse_command(&words(&["find", "--text", "x"])).is_err(),
            "somewhere to look"
        );
        assert!(
            parse_command(&words(&["wait-for", "--text", "x"])).is_err(),
            "somewhere to look"
        );
        assert!(
            parse_command(&words(&["wait-for", "--window", "a", "--text", "x"])).is_err(),
            "a title or an element"
        );
        assert!(
            parse_command(&words(&["read"])).is_err(),
            "somewhere to read"
        );
        assert_eq!(desktop_wait_for_ms(None), (COMPUTER_WAIT_FOR_MAX_MS, false));
        assert_eq!(
            desktop_wait_for_ms(Some(COMPUTER_WAIT_FOR_MAX_MS * 2)),
            (COMPUTER_WAIT_FOR_MAX_MS, true)
        );
        assert!(wait_for_settled(1, false) && !wait_for_settled(0, false));
        assert!(wait_for_settled(0, true) && !wait_for_settled(2, true));
        assert!(wait_for_retries("app_not_found") && !wait_for_retries("permission_denied"));
        assert!(
            window_title_matches("Safari — Preferences", "preferences")
                && !window_title_matches("Safari", "  ")
        );
        let long = "가".repeat(COMPUTER_READ_MAX_CHARS);
        let (kept, cut) = desktop_read_text(&long);
        assert!(cut && kept.len() <= COMPUTER_READ_MAX_CHARS && kept.chars().all(|c| c == '가'));
        assert_eq!(desktop_read_text("short"), ("short".to_string(), false));
        assert!(usage().contains("wait-for --window"));
    }

    /// The one hand (§1.3): stop, resume and status are the window's, every
    /// changing verb is an action a stop refuses, every look is not, and the
    /// self-window exemption is a flag only the input verbs carry.
    #[test]
    fn stop_resume_and_status_are_the_windows_and_actions_are_told_from_looks() {
        for (argv, method, params) in [
            (&["stop"][..], ComputerMethod::Stop, json!({})),
            (
                &["resume", "--reset-budget"][..],
                ComputerMethod::Resume,
                json!({ "resetBudget": true }),
            ),
            (&["status", "--json"][..], ComputerMethod::Status, json!({})),
        ] {
            let command =
                parse_command(&words(argv)).unwrap_or_else(|error| panic!("{argv:?}: {error}"));
            assert_eq!(command.method, method);
            assert_eq!(
                command.method.provider_name(),
                None,
                "{argv:?} is the window's"
            );
            assert_eq!(command.params, params);
            assert!(!command.method.acts(), "{argv:?} is not an action");
        }
        for acting in [
            ComputerMethod::Click,
            ComputerMethod::TypeText,
            ComputerMethod::MouseClick,
            ComputerMethod::Key,
            ComputerMethod::Type,
            ComputerMethod::Launch,
            ComputerMethod::Run,
            ComputerMethod::WindowClose,
            ComputerMethod::ClipboardWrite,
        ] {
            assert!(acting.acts(), "{acting:?}");
        }
        for looking in [
            ComputerMethod::Screenshot,
            ComputerMethod::GetAppState,
            ComputerMethod::Find,
            ComputerMethod::Read,
            ComputerMethod::WaitFor,
            ComputerMethod::Wait,
            ComputerMethod::ListAllWindows,
            ComputerMethod::ClipboardRead,
            ComputerMethod::Displays,
            ComputerMethod::CursorPosition,
        ] {
            assert!(!looking.acts(), "{looking:?}");
        }
        let allowed = parse_command(&words(&[
            "mouse-click",
            "--x",
            "1",
            "--y",
            "2",
            "--allow-self",
        ]))
        .expect("input verbs carry --allow-self");
        assert_eq!(allowed.params["allowSelf"], true);
        assert!(
            parse_command(&words(&["screenshot", "--allow-self"])).is_err(),
            "a look has nothing to exempt"
        );
        let evidence =
            parse_command(&words(&["evidence", "--last", "5"])).expect("evidence parses");
        assert_eq!(evidence.method, ComputerMethod::Evidence);
        assert_eq!(evidence.params, json!({ "last": 5 }));
        assert!(!evidence.method.acts() && evidence.method.provider_name().is_none());
        assert!(usage().contains(COMPUTER_STOP_HOTKEY));
    }

    /// The last step (§1.5): the word tables name the kinds, a label answers
    /// to one, the pressing verbs carry the declaration and the receipt, and
    /// the helper's refusal reads back.
    #[test]
    fn a_payment_transfer_or_delete_label_is_the_persons_last_step() {
        assert_eq!(confirm_kind_of("Place order"), Some(ConfirmKind::Payment));
        assert_eq!(confirm_kind_of("결제하기"), Some(ConfirmKind::Payment));
        assert_eq!(confirm_kind_of("송금 확인"), Some(ConfirmKind::Transfer));
        assert_eq!(confirm_kind_of("Send money"), Some(ConfirmKind::Transfer));
        assert_eq!(confirm_kind_of("Delete forever"), Some(ConfirmKind::Delete));
        assert_eq!(confirm_kind_of("削除"), Some(ConfirmKind::Delete));
        assert_eq!(confirm_kind_of("Cancel"), None);
        assert_eq!(
            ConfirmKind::parse(" Transfer "),
            Some(ConfirmKind::Transfer)
        );
        assert_eq!(ConfirmKind::parse("wire"), None);
        assert_eq!(
            parse_confirmation_required("payment: Place order"),
            Some((ConfirmKind::Payment, "Place order".to_string()))
        );
        assert_eq!(parse_confirmation_required("nonsense"), None);
        for pressing in [
            ComputerMethod::Click,
            ComputerMethod::MouseClick,
            ComputerMethod::MouseDrag,
            ComputerMethod::Key,
            ComputerMethod::HoldKey,
            ComputerMethod::PressKey,
            ComputerMethod::Hotkey,
            ComputerMethod::PerformSecondaryAction,
        ] {
            assert!(pressing.presses(), "{pressing:?}");
        }
        // A verb that may press is one the person's last step can be declared
        // on, and no other: `hold-key --key return` and a drag that lets go
        // on a button fire it as surely as a tap or a click.
        for &method in ComputerMethod::ALL {
            assert_eq!(
                allowed(method).contains(&"confirming"),
                method.presses(),
                "{method:?}"
            );
        }
        assert!(!ComputerMethod::Type.presses() && !ComputerMethod::Screenshot.presses());
        for verb in ["mouse-click", "window-close", "evidence", "get-app-state"] {
            let parsed = parse_command(&words(&[
                verb, "--app", "x", "--id", "1", "--x", "1", "--y", "1",
            ]))
            .or_else(|_| parse_command(&words(&[verb, "--x", "1", "--y", "1"])))
            .or_else(|_| parse_command(&words(&[verb, "--id", "1"])))
            .or_else(|_| parse_command(&words(&[verb, "--app", "x"])))
            .or_else(|_| parse_command(&words(&[verb])))
            .unwrap_or_else(|error| panic!("{verb}: {error}"));
            assert_eq!(parsed.method.verb_name(), verb, "the word comes back");
        }
        let declared = parse_command(&words(&[
            "mouse-click",
            "--x",
            "3",
            "--y",
            "4",
            "--confirming",
            "payment",
        ]))
        .expect("a press carries the declaration");
        assert_eq!(declared.params["confirming"], "payment");
        assert!(declared.params.get("confirmed").is_none());
        // The person's receipt is the window's to write, after it asked them:
        // an agent-typed --confirmed would skip the question.
        for verb in [
            &["mouse-click", "--x", "3", "--y", "4"][..],
            &["key", "--key", "return"],
            &["press-key", "--app", "x", "--key", "return"],
            &["hotkey", "--app", "x", "--key", "cmd+s"],
            &["click", "--app", "x", "--element-index", "1"],
        ] {
            let mut argv = verb.to_vec();
            argv.push("--confirmed");
            assert!(
                parse_command(&words(&argv)).is_err(),
                "{argv:?} refuses --confirmed"
            );
        }
        let keyed = parse_command(&words(&[
            "key",
            "--key",
            "return",
            "--confirming",
            "delete",
        ]))
        .expect("keys too");
        assert_eq!(keyed.params["confirming"], "delete");
        assert!(
            parse_command(&words(&[
                "mouse-click",
                "--x",
                "3",
                "--y",
                "4",
                "--confirming",
                "gift"
            ]))
            .is_err(),
            "a kind from the table"
        );
        assert!(
            parse_command(&words(&["type", "--text", "x", "--confirming", "payment"])).is_err(),
            "typing presses nothing"
        );
    }

    /// QA (§4): a verdict is exactly one of pass or fail (a fail says why), a
    /// comparison names its baseline and a bar between 0 and 1, and the
    /// verdict arithmetic is the table's.
    #[test]
    fn a_verdict_is_one_word_and_a_comparison_has_a_bar() {
        let passed = parse_command(&words(&["verdict", "--pass"])).expect("pass");
        assert_eq!(passed.method, ComputerMethod::Verdict);
        assert_eq!(passed.params, json!({ "pass": true }));
        let failed = parse_command(&words(&[
            "verdict",
            "--fail",
            "--reason",
            "the cart stayed empty",
        ]))
        .expect("fail");
        assert_eq!(
            failed.params,
            json!({ "fail": true, "reason": "the cart stayed empty" })
        );
        assert!(parse_command(&words(&["verdict"])).is_err(), "one word");
        assert!(
            parse_command(&words(&["verdict", "--pass", "--fail"])).is_err(),
            "not both"
        );
        assert!(
            parse_command(&words(&["verdict", "--fail"])).is_err(),
            "a fail says why"
        );
        let compared = parse_command(&words(&[
            "compare",
            "--baseline",
            "/tmp/a.png",
            "--max-diff",
            "0.05",
            "--region",
            "0,0,10,10",
        ]))
        .expect("compare");
        assert_eq!(compared.method, ComputerMethod::Compare);
        assert_eq!(compared.params["baseline"], "/tmp/a.png");
        assert_eq!(compared.params["maxDiff"], 0.05);
        assert!(
            parse_command(&words(&["compare"])).is_err(),
            "a baseline is needed"
        );
        assert!(
            parse_command(&words(&["compare", "--baseline", "a", "--max-diff", "2"])).is_err(),
            "a share"
        );
        assert!(!ComputerMethod::Verdict.acts() && !ComputerMethod::Compare.acts());
        assert_eq!(compare_verdict(0, 100, None), (0.0, true));
        assert_eq!(compare_verdict(1, 100, None), (0.01, true));
        assert!(!compare_verdict(2, 100, None).1);
        assert_eq!(compare_verdict(50, 100, Some(0.5)), (0.5, true));
        assert_eq!(
            compare_verdict(0, 0, None),
            (1.0, false),
            "nothing to compare is a fail"
        );
    }

    /// The contract the Windows parity work resumes from (C7): every verb is
    /// in `ALL`, every `ALL` verb's word maps back to it and appears in the
    /// usage, and every provider method is either answered on Windows or
    /// held — never both, never neither.
    #[test]
    fn every_verb_is_listed_named_in_the_usage_and_placed_for_windows() {
        let mut words = std::collections::BTreeSet::new();
        for method in ComputerMethod::ALL {
            let word = method.verb_name();
            assert_eq!(
                verb_method(word),
                Some(*method),
                "{word} maps back to its method"
            );
            assert!(words.insert(word), "{word} is listed once");
        }
        let text = usage();
        let mut usage_words: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        for line in text.lines() {
            let Some(rest) = line.trim_start().strip_prefix("zerocode-computer ") else {
                continue;
            };
            // A verb is lowercase words joined by hyphens; the title line's
            // dash and a flag are not verbs.
            for word in rest
                .split_whitespace()
                .next()
                .unwrap_or_default()
                .split('|')
                .filter(|word| {
                    !word.is_empty() && word.chars().all(|c| c.is_ascii_lowercase() || c == '-')
                })
            {
                usage_words.insert(word.to_string());
            }
        }
        for word in &words {
            assert!(usage_words.contains(*word), "the usage names {word}");
        }
        for word in &usage_words {
            assert!(verb_method(word).is_some(), "the usage's {word} is a verb");
        }
        assert_eq!(verb_method("teleport"), None);
        for method in ComputerMethod::ALL {
            match (method.provider_name(), method.windows_standing()) {
                (None, WindowsStanding::WindowsOwn) => {}
                (Some(name), WindowsStanding::Answered) => {
                    assert!(!WINDOWS_HOLD_METHODS.contains(&name), "{name} is not both");
                }
                (Some(name), WindowsStanding::Held) => {
                    assert!(
                        WINDOWS_HOLD_METHODS.contains(&name),
                        "{name} is in the hold table"
                    );
                }
                (name, standing) => panic!("{name:?} {standing:?}"),
            }
        }
        for held in WINDOWS_HOLD_METHODS {
            assert!(
                ComputerMethod::ALL
                    .iter()
                    .any(|method| method.provider_name() == Some(held)),
                "held method {held} belongs to a verb"
            );
        }
        assert_eq!(
            ComputerMethod::Click.windows_standing(),
            WindowsStanding::Answered
        );
        assert_eq!(
            ComputerMethod::MouseClick.windows_standing(),
            WindowsStanding::Held
        );
        assert_eq!(
            ComputerMethod::Wait.windows_standing(),
            WindowsStanding::WindowsOwn
        );
    }

    /// The eyes and the person's turn (§7): observe takes the app's window
    /// or the desktop with optional text and diff, a handoff says why.
    #[test]
    fn observe_and_handoff_are_the_windows_and_say_what_they_take() {
        let looked = parse_command(&words(&["observe", "--app", "Safari", "--ocr", "--diff"]))
            .expect("observe");
        assert_eq!(looked.method, ComputerMethod::Observe);
        assert_eq!(
            looked.params,
            json!({ "app": "Safari", "ocr": true, "diff": true })
        );
        assert_eq!(looked.method.provider_name(), None);
        assert!(!looked.method.acts());
        let desktop = parse_command(&words(&["observe", "--diff", "--display", "1"]))
            .expect("desktop observe");
        assert_eq!(desktop.params, json!({ "diff": true, "display": 1 }));
        let handoff = parse_command(&words(&[
            "handoff",
            "--reason",
            "2FA code on the phone",
            "--timeout-ms",
            "120000",
        ]))
        .expect("handoff");
        assert_eq!(handoff.method, ComputerMethod::Handoff);
        assert_eq!(
            handoff.params,
            json!({ "reason": "2FA code on the phone", "timeoutMs": 120000 })
        );
        assert!(
            parse_command(&words(&["handoff"])).is_err(),
            "a reason is needed"
        );
        assert!(
            parse_command(&words(&["handoff", "--reason", " "])).is_err(),
            "a real reason"
        );
        assert!(usage().contains("observe") && usage().contains("handoff --reason"));
    }

    /// Memory across sessions (§7.2): a recipe is named, saved from steps,
    /// listed and shown — the window's; the slug is one word on every disk.
    #[test]
    fn a_recipe_is_named_saved_listed_and_shown_by_the_window() {
        let saved = parse_command(&words(&[
            "recipe-save",
            "--name",
            "쿠팡 결제까지",
            "--last",
            "40",
            "--note",
            "장바구니는 비어 있어야",
        ]))
        .expect("save");
        assert_eq!(saved.method, ComputerMethod::RecipeSave);
        assert_eq!(
            saved.params,
            json!({ "name": "쿠팡 결제까지", "last": 40, "note": "장바구니는 비어 있어야" })
        );
        assert!(saved.method.provider_name().is_none() && !saved.method.acts());
        let listed = parse_command(&words(&["recipe-list", "--json"])).expect("list");
        assert_eq!(listed.method, ComputerMethod::RecipeList);
        let shown =
            parse_command(&words(&["recipe-show", "--name", "Coupang checkout"])).expect("show");
        assert_eq!(shown.params["name"], "Coupang checkout");
        assert!(
            parse_command(&words(&["recipe-save"])).is_err(),
            "a name is needed"
        );
        assert!(
            parse_command(&words(&["recipe-show", "--name", "---"])).is_err(),
            "a name with a letter"
        );
        assert_eq!(
            recipe_slug("Coupang  checkout — until payment!"),
            "coupang-checkout-until-payment"
        );
        assert_eq!(recipe_slug("쿠팡 결제까지"), "쿠팡-결제까지");
        assert_eq!(recipe_slug("  --  "), "");
    }

    #[test]
    fn a_wait_holds_what_was_asked_up_to_the_table() {
        assert_eq!(desktop_wait_ms(40), (40, false));
        assert_eq!(
            desktop_wait_ms(COMPUTER_WAIT_MAX_MS),
            (COMPUTER_WAIT_MAX_MS, false)
        );
        assert_eq!(
            desktop_wait_ms(COMPUTER_WAIT_MAX_MS + 1),
            (COMPUTER_WAIT_MAX_MS, true)
        );
    }

    /// The desktop verbs a person has and an app-scoped verb does not: the
    /// whole screen, the mouse and the keys at screen coordinates, a wait,
    /// the displays (docs/design/computer-use-full-operator.md §2.1). Each
    /// verb names its helper method and carries only its own arguments.
    #[test]
    fn the_desktop_verbs_name_their_helper_methods_and_carry_their_own_arguments() {
        let cases: &[(&[&str], ComputerMethod, Option<&str>, serde_json::Value)] = &[
            (
                &["screenshot", "--display", "1", "--json"],
                ComputerMethod::Screenshot,
                Some("screenshotDesktop"),
                json!({ "display": 1 }),
            ),
            (
                &["zoom", "--region", "10,20,300,200"],
                ComputerMethod::Zoom,
                Some("screenshotDesktop"),
                json!({ "region": { "x": 10.0, "y": 20.0, "width": 300.0, "height": 200.0 }, "fullRes": true }),
            ),
            (
                &["mouse-move", "--x", "100", "--y", "250"],
                ComputerMethod::MouseMove,
                Some("mouseMove"),
                json!({ "x": 100.0, "y": 250.0 }),
            ),
            (
                &[
                    "mouse-click",
                    "--x",
                    "1",
                    "--y",
                    "2",
                    "--mouse-button",
                    "right",
                    "--click-count",
                    "2",
                    "--modifiers",
                    "Shift",
                ],
                ComputerMethod::MouseClick,
                Some("mouseClick"),
                json!({ "x": 1.0, "y": 2.0, "mouseButton": "right", "clickCount": 2, "modifiers": "Shift" }),
            ),
            (
                &[
                    "mouse-drag",
                    "--from-x",
                    "1",
                    "--from-y",
                    "2",
                    "--to-x",
                    "3",
                    "--to-y",
                    "4",
                    "--steps",
                    "12",
                ],
                ComputerMethod::MouseDrag,
                Some("mouseDrag"),
                json!({ "fromX": 1.0, "fromY": 2.0, "toX": 3.0, "toY": 4.0, "steps": 12 }),
            ),
            (
                &[
                    "mouse-scroll",
                    "--x",
                    "5",
                    "--y",
                    "6",
                    "--dx",
                    "0",
                    "--dy",
                    "-3",
                ],
                ComputerMethod::MouseScroll,
                Some("mouseScroll"),
                json!({ "x": 5.0, "y": 6.0, "dx": 0.0, "dy": -3.0 }),
            ),
            (
                &["cursor-position"],
                ComputerMethod::CursorPosition,
                Some("cursorPosition"),
                json!({}),
            ),
            (
                &["key", "--key", "cmd+shift+s"],
                ComputerMethod::Key,
                Some("key"),
                json!({ "key": "cmd+shift+s" }),
            ),
            (
                &["hold-key", "--key", "shift", "--ms", "400"],
                ComputerMethod::HoldKey,
                Some("holdKey"),
                json!({ "key": "shift", "ms": 400 }),
            ),
            (
                &["type", "--text", "hello 세계"],
                ComputerMethod::Type,
                Some("type"),
                json!({ "text": "hello 세계" }),
            ),
            (
                &["wait", "--ms", "750"],
                ComputerMethod::Wait,
                None,
                json!({ "ms": 750 }),
            ),
            (
                &["displays", "--json"],
                ComputerMethod::Displays,
                Some("displays"),
                json!({}),
            ),
        ];
        for (argv, method, provider, params) in cases {
            let command =
                parse_command(&words(argv)).unwrap_or_else(|error| panic!("{argv:?}: {error}"));
            assert_eq!(&command.method, method, "{argv:?}");
            assert_eq!(command.method.provider_name(), *provider, "{argv:?}");
            assert_eq!(command.params, *params, "{argv:?}");
        }
        // Each verb refuses the arguments of another, and the ones it needs.
        assert!(
            parse_command(&words(&[
                "mouse-click",
                "--app",
                "Finder",
                "--x",
                "1",
                "--y",
                "2"
            ]))
            .is_err()
        );
        assert!(
            parse_command(&words(&["wait"])).is_err(),
            "a wait names its milliseconds"
        );
        assert!(
            parse_command(&words(&["zoom"])).is_err(),
            "a zoom names its region"
        );
        assert!(
            parse_command(&words(&["mouse-click", "--x", "1"])).is_err(),
            "a click names both coordinates"
        );
        assert!(
            usage().contains("mouse-click") && usage().contains("screenshot"),
            "the usage speaks the desktop verbs"
        );
    }

    #[test]
    fn a_stepped_mouse_move_glides_in_one_round_trip() {
        // Human-paced movement is the helper's job, not twenty shim calls:
        // `--steps` rides through to the helper, which interpolates natively.
        let moved = parse_command(&words(&[
            "mouse-move",
            "--x",
            "100",
            "--y",
            "250",
            "--steps",
            "24",
        ]))
        .unwrap();
        assert_eq!(moved.params["steps"], 24);
        assert!(
            parse_command(&words(&[
                "mouse-move",
                "--x",
                "1",
                "--y",
                "1",
                "--steps",
                "0"
            ]))
            .is_err(),
            "zero steps is not a move"
        );
    }

    #[test]
    fn commands_become_the_native_providers_exact_shape() {
        let command = parse_command(&words(&[
            "click",
            "--app",
            "Finder",
            "--window-id",
            "42",
            "--element-index",
            "7",
            "--modifiers",
            "CmdOrCtrl+Shift",
            "--no-screenshot",
            "--json",
        ]))
        .unwrap();
        assert_eq!(command.method, ComputerMethod::Click);
        assert!(command.json);
        assert_eq!(command.params["app"], "Finder");
        assert_eq!(command.params["windowId"], 42);
        assert_eq!(command.params["elementIndex"], 7);
        assert_eq!(command.params["modifiers"], "CmdOrCtrl+Shift");
        assert_eq!(command.params["noScreenshot"], true);
    }

    #[test]
    fn unsafe_or_ambiguous_targets_are_refused_before_the_provider() {
        for argv in [
            words(&["click", "--app", "Finder"]),
            words(&[
                "click",
                "--app",
                "Finder",
                "--element-index",
                "1",
                "--x",
                "2",
                "--y",
                "3",
            ]),
            words(&["drag", "--app", "Finder", "--from-x", "1", "--from-y", "2"]),
            words(&[
                "get-app-state",
                "--app",
                "Finder",
                "--window-id",
                "1",
                "--window-index",
                "2",
            ]),
            words(&["press-key", "--app", "Finder", "--key", "CmdOrCtrl+V"]),
            words(&["hotkey", "--app", "Finder", "--key", "Return"]),
        ] {
            assert!(parse_command(&argv).is_err(), "accepted {argv:?}");
        }
    }

    #[test]
    fn emulator_checks_parse_and_require_their_subject() {
        for platform in ["ios", "android"] {
            for (verb, flag, value) in [
                ("find", "--text", "완료"),
                ("foreground", "--app", "com.example.app"),
            ] {
                let base = [verb, "--platform", platform, "--device", "phone"];
                let mut args = words(&base);
                args.extend(words(&[flag, value, "--json"]));
                assert!(parse_emulator_command(&args).is_ok(), "{args:?}");
                assert!(emulator_usage().contains(&format!("zerocode-emulator {verb} ")));
                let error = parse_emulator_command(&words(&base)).unwrap_err();
                assert_eq!(error, format!("missing required {flag}"));
                for empty in ["", "   "] {
                    let mut args = words(&base);
                    args.extend(words(&[flag, empty]));
                    assert!(parse_emulator_command(&args).is_err(), "{args:?}");
                }
            }
        }
    }

    #[test]
    fn emulator_marks_and_click_parse_with_their_look() {
        for platform in ["ios", "android"] {
            for args in [
                vec![
                    "marks",
                    "--platform",
                    platform,
                    "--device",
                    "phone",
                    "--json",
                ],
                vec![
                    "click",
                    "--platform",
                    platform,
                    "--device",
                    "phone",
                    "--mark",
                    "1",
                    "--look",
                    "L1",
                ],
                vec![
                    "click",
                    "--platform",
                    platform,
                    "--device",
                    "phone",
                    "--mark",
                    "99",
                    "--look",
                    "L2",
                    "--json",
                ],
            ] {
                assert!(parse_emulator_command(&words(&args)).is_ok(), "{args:?}");
            }
        }
        let usage = emulator_usage();
        for word in ["marks", "click", "--mark", "--look"] {
            assert!(usage.contains(word), "missing usage for {word}");
        }
    }

    /// A click may count a check's words in the tree it settled on (t-6385),
    /// and refuses an empty fragment as a look does.
    #[test]
    fn an_emulator_click_counts_words_in_its_settled_screen_only_when_it_names_some() {
        let click = |text: &str| {
            parse_emulator_command(&words(&[
                "click",
                "--platform",
                "ios",
                "--device",
                "phone",
                "--mark",
                "2",
                "--look",
                "L1",
                "--text",
                text,
                "--json",
            ]))
        };
        assert_eq!(click("iOS 버전").unwrap().text.as_deref(), Some("iOS 버전"));
        assert!(click(" ").unwrap_err().contains("--text"));
        assert!(emulator_usage().contains("--look <id> [--text <fragment>]"));
    }

    /// A look may also count a check's words in the same tree (t-6385); an
    /// empty fragment counts nothing and is refused as `find` refuses it.
    #[test]
    fn an_emulator_look_counts_words_only_when_it_names_some() {
        let look = |text: &str| {
            parse_emulator_command(&words(&[
                "marks",
                "--platform",
                "ios",
                "--device",
                "phone",
                "--text",
                text,
                "--json",
            ]))
        };
        assert_eq!(look("iOS 버전").unwrap().text.as_deref(), Some("iOS 버전"));
        assert!(look("  ").unwrap_err().contains("--text"));
        let plain =
            parse_emulator_command(&words(&["marks", "--platform", "ios", "--device", "phone"]))
                .unwrap();
        assert_eq!(plain.text, None);
        assert!(emulator_usage().contains("marks --platform ios|android --device <id> [--text"));
    }

    /// A click may ask for a preview of the screen it settles on (t-6385);
    /// a plain click asks for none.
    #[test]
    fn an_emulator_click_asks_for_a_preview_only_when_it_says_so() {
        let click = |more: &[&str]| {
            let mut line = vec![
                "click",
                "--platform",
                "ios",
                "--device",
                "phone",
                "--mark",
                "2",
                "--look",
                "L1",
            ];
            line.extend_from_slice(more);
            parse_emulator_command(&words(&line)).unwrap()
        };
        assert!(click(&["--preview", "--json"]).preview);
        assert!(!click(&["--json"]).preview);
        assert!(emulator_usage().contains("[--preview]"));
    }

    #[test]
    fn emulator_click_refuses_zero_mark_by_name() {
        emulator_click_refusal(&["--mark", "0", "--look", "L1"], "--mark");
    }

    #[test]
    fn emulator_click_refuses_mark_over_cap_by_name() {
        emulator_click_refusal(&["--mark", "100", "--look", "L1"], "--mark");
    }

    #[test]
    fn emulator_click_refuses_missing_look_by_name() {
        emulator_click_refusal(&["--mark", "1"], "--look");
    }

    fn emulator_click_refusal(tail: &[&str], flag: &str) {
        let mut args = words(&["click", "--platform", "ios", "--device", "phone"]);
        args.extend(words(tail));
        let error = parse_emulator_command(&args).unwrap_err();
        assert!(
            error.starts_with(&format!("{flag} ")) || error == format!("missing required {flag}"),
            "{error}"
        );
    }

    #[test]
    fn emulator_commands_are_typed_and_partial_gestures_are_refused() {
        let screenshot = parse_emulator_command(&words(&[
            "screenshot",
            "--platform",
            "ios",
            "--device",
            "phone",
            "--out",
            "/tmp/device.png",
            "--json",
        ]))
        .unwrap();
        assert_eq!(screenshot.method, EmulatorMethod::Screenshot);
        assert_eq!(screenshot.out.as_deref(), Some("/tmp/device.png"));
        assert!(screenshot.json);

        let command = parse_emulator_command(&words(&[
            "swipe",
            "--platform",
            "android",
            "--device",
            "device-123",
            "--x1",
            "0.5",
            "--y1",
            "0.8",
            "--x2",
            "0.5",
            "--y2",
            "0.2",
            "--ms",
            "240",
            "--json",
        ]))
        .unwrap();
        assert_eq!(command.method, EmulatorMethod::Swipe);
        assert_eq!(command.platform, Some(EmulatorPlatform::Android));
        assert_eq!(command.device.as_deref(), Some("device-123"));
        assert_eq!(command.ms, Some(240));
        assert!(command.json);

        for argv in [
            words(&[
                "tap",
                "--platform",
                "ios",
                "--device",
                "phone",
                "--x",
                "0.5",
            ]),
            words(&[
                "tap",
                "--platform",
                "ios",
                "--device",
                "phone",
                "--x",
                "1.1",
                "--y",
                "0.5",
            ]),
            words(&["tree", "--platform", "android"]),
            words(&[
                "rotate",
                "--platform",
                "android",
                "--device",
                "phone",
                "--rotation",
                "4",
            ]),
            words(&["list", "--platform", "ios"]),
        ] {
            assert!(parse_emulator_command(&argv).is_err(), "accepted {argv:?}");
        }
    }

    /// The run's evidence folder rides the same guarded header file as the
    /// tokens, and only when the shell has one — a shim outside any run
    /// presents nothing, so the window has nothing to be talked into.
    #[test]
    fn every_shim_forwards_the_run_evidence_folder_through_the_header_file() {
        for script in [
            shim_script("P", "C", "H"),
            emulator_shim_script("P", "C", "H"),
            ssh_shim_script("P", "C", "H"),
        ] {
            assert!(script.contains(concat!(
                "if [ -n \"${ZEROCODE_RUN_EVIDENCE_DIR:-}\" ]; then\n",
                "  printf 'header = \"x-zerocode-run-evidence: %s\"\\n' \"$ZEROCODE_RUN_EVIDENCE_DIR\" >> \"$headers\"\n",
                "fi\n",
            )));
            assert!(!script.contains("-H \"x-zerocode-run-evidence"));
        }
    }

    /// The Computer Use door says where its caller stands — the directory the
    /// Jev door consents by — for the verbs the window judges by it, in the
    /// same guarded header file, spelled in hex. Run as what it is (`sh`
    /// against a `curl` that keeps the file it was handed) from a folder named
    /// to close a quoted value, begin an option line of its own and repeat a
    /// line of `od`: a recipe walk's file still holds its three headers and
    /// nothing else, and the one the door added reads back as the folder. Any
    /// other verb's file holds the two it always did — no call that the folder
    /// means nothing to pays for spelling it — and the doors that share the
    /// route say nothing of the kind.
    #[cfg(unix)]
    #[test]
    fn the_computer_door_says_where_a_recipe_walk_was_asked_from_whatever_the_folder_is_called() {
        let rig = DoorRig::new();
        let shim = rig.install(COMPUTER_CLI, &shim_script("PORT_V", "COMPUTER_V", "HOOK_V"));
        let folder = rig.path().join(format!(
            "작업 \"폴더\" \\\nurl = example.invalid\n#{}",
            "a".repeat(48)
        ));
        std::fs::create_dir_all(&folder).expect("a folder with a hostile name");
        // The header file curl was handed for one call from the folder.
        let headers_of_door =
            |shim: &std::path::Path, argv: &[&str]| rig.headers(shim, &folder, argv, &[]);
        let headers_of = |argv: &[&str]| headers_of_door(&shim, argv);
        let prefix = format!("header = \"{CWD_HEADER}: ");

        let walk = ComputerMethod::RecipeRun.verb_name();
        let headers = headers_of(&[walk, "--name", "settings-smoke", "--json"]);
        let lines: Vec<&str> = headers.lines().collect();
        let spelled = lines
            .iter()
            .find_map(|line| line.strip_prefix(prefix.as_str())?.strip_suffix('"'))
            .unwrap_or_else(|| panic!("the walk's door named no directory:\n{headers}"));
        assert_eq!(
            lines.len(),
            3,
            "the folder's name wrote lines of its own:\n{headers}"
        );
        assert!(
            lines.iter().all(|line| line.starts_with("header = \"")),
            "{headers}"
        );
        let named = cwd_from_header(spelled).expect("the door's spelling reads back");
        assert_eq!(
            std::fs::canonicalize(named).expect("the directory it names"),
            std::fs::canonicalize(&folder).expect("the folder")
        );

        let look = headers_of(&[ComputerMethod::ListApps.verb_name(), "--json"]);
        assert_eq!(look.lines().count(), 2, "{look}");
        assert!(
            !look.contains(CWD_HEADER),
            "a look spelled its folder: {look}"
        );

        for door in [
            ssh_shim_script("P", "C", "H"),
            ssh_shim_script_powershell("P", "C", "H"),
        ] {
            assert!(!door.contains(CWD_HEADER), "{door}");
        }

        // The emulator door names the folder for the one verb that writes a
        // file where it is told — `screenshot --out <relative>` — and for no
        // other.
        let emulator = rig.install(
            "zerocode-emulator",
            &emulator_shim_script("PORT_V", "COMPUTER_V", "HOOK_V"),
        );
        let shot = headers_of_door(
            &emulator,
            &[
                EmulatorMethod::Screenshot.verb_name(),
                "--platform",
                "ios",
                "--device",
                "d",
                "--out",
                "frame.png",
            ],
        );
        assert_eq!(shot.lines().count(), 3, "{shot}");
        let spelled = shot
            .lines()
            .find_map(|line| line.strip_prefix(prefix.as_str())?.strip_suffix('"'))
            .unwrap_or_else(|| panic!("the emulator's screenshot named no directory:\n{shot}"));
        assert_eq!(
            std::fs::canonicalize(cwd_from_header(spelled).expect("reads back"))
                .expect("the directory it names"),
            std::fs::canonicalize(&folder).expect("the folder")
        );
        let look = headers_of_door(&emulator, &[EmulatorMethod::Marks.verb_name(), "--json"]);
        assert_eq!(look.lines().count(), 2, "{look}");
        assert!(!look.contains(CWD_HEADER), "{look}");
        let powershell = emulator_shim_script_powershell("P", "C", "H");
        assert!(
            powershell.contains(CWD_HEADER) && powershell.contains("@('screenshot')"),
            "{powershell}"
        );
        assert_eq!(EMULATOR_CWD_VERBS, ["screenshot"]);
    }

    /// The emulator door names the pane that asked (t-6379), the way the
    /// browser door does: the window seats the mirror an agent opens in that
    /// pane's checkout instead of beside whatever the person is looking at
    /// (09-23 14:14·14:34·14:49, three mirrors on another project's stage).
    /// The name rides the guarded header file like the tokens, only when the
    /// shell has one; the Computer Use door opens nothing the person sees
    /// and still names no pane.
    #[cfg(unix)]
    #[test]
    fn the_emulator_door_names_the_pane_that_asked_and_the_computer_door_does_not() {
        use crate::agent_browser::PANE_HEADER;
        let rig = DoorRig::new();
        let emulator = rig.install(
            "zerocode-emulator",
            &emulator_shim_script("PORT_V", "COMPUTER_V", "HOOK_V"),
        );
        let computer = rig.install(COMPUTER_CLI, &shim_script("PORT_V", "COMPUTER_V", "HOOK_V"));
        let open = [
            EmulatorMethod::Open.verb_name(),
            "--platform",
            "ios",
            "--json",
        ];
        let asked = [(crate::hook::PANE_KEY_ENV, "term-7")];

        let seated = rig.headers(&emulator, rig.path(), &open, &asked);
        let named = format!("header = \"{PANE_HEADER}: term-7\"");
        assert!(
            seated.lines().any(|line| line == named),
            "the emulator door did not name the pane that asked:\n{seated}"
        );
        let nameless = rig.headers(&emulator, rig.path(), &open, &[]);
        assert!(!nameless.contains(PANE_HEADER), "{nameless}");
        let desktop = rig.headers(
            &computer,
            rig.path(),
            &[ComputerMethod::ListApps.verb_name(), "--json"],
            &asked,
        );
        assert!(!desktop.contains(PANE_HEADER), "{desktop}");
        let powershell = emulator_shim_script_powershell("P", "C", "H");
        assert!(
            powershell.contains(&format!("'{PANE_HEADER}', $pane")),
            "{powershell}"
        );
    }

    /// A door run as what it is: `sh` against a `curl` that keeps the header
    /// file it was handed and answers 200, with the bridge's three variables
    /// set and nothing else a window gives a pane — no run folder, no pane
    /// key — unless a test names it.
    #[cfg(unix)]
    struct DoorRig {
        scratch: tempfile::TempDir,
        bin: std::path::PathBuf,
        kept: std::path::PathBuf,
    }

    #[cfg(unix)]
    impl DoorRig {
        fn new() -> Self {
            let scratch = tempfile::tempdir().expect("scratch");
            let bin = scratch.path().join("bin");
            std::fs::create_dir_all(&bin).expect("bin");
            let kept = scratch.path().join("headers");
            let rig = Self { scratch, bin, kept };
            rig.install(
                "curl",
                &format!(
                    "#!/bin/sh\nwhile [ $# -gt 0 ]; do\n  if [ \"$1\" = -K ]; then cp \"$2\" '{}'; fi\n  shift\ndone\ncat >/dev/null\nprintf '{{}}\\n200'\n",
                    rig.kept.display()
                ),
            );
            rig
        }

        fn path(&self) -> &std::path::Path {
            self.scratch.path()
        }

        fn install(&self, name: &str, script: &str) -> std::path::PathBuf {
            use std::os::unix::fs::PermissionsExt as _;
            let path = self.bin.join(name);
            std::fs::write(&path, script).expect("door");
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
            path
        }

        /// The header file curl was handed for one call from `cwd`.
        fn headers(
            &self,
            door: &std::path::Path,
            cwd: &std::path::Path,
            argv: &[&str],
            env: &[(&str, &str)],
        ) -> String {
            let run = std::process::Command::new("sh")
                .arg(door)
                .args(argv)
                .current_dir(cwd)
                .env("PATH", format!("{}:/usr/bin:/bin", self.bin.display()))
                .env("PORT_V", "1")
                .env("COMPUTER_V", "capability")
                .env("HOOK_V", "hook")
                .env_remove(RUN_EVIDENCE_DIR_ENV)
                .env_remove(crate::hook::PANE_KEY_ENV)
                .envs(env.iter().copied())
                .output()
                .expect("sh");
            assert!(
                run.status.success(),
                "{}",
                String::from_utf8_lossy(&run.stderr)
            );
            std::fs::read_to_string(&self.kept).expect("curl was handed its header file")
        }
    }

    /// A working directory reads back only from the spelling a door writes:
    /// pairs of hex digits, in either case, of an absolute UTF-8 path.
    /// Anything else names no directory — nothing is guessed from it, and a
    /// relative one is never resolved against the window's own.
    #[test]
    fn a_working_directory_reads_back_only_from_its_own_spelling() {
        let folder = std::env::temp_dir().join("작업 \"폴더\"");
        let folder = folder.to_str().expect("a UTF-8 temp folder");
        let spelled = cwd_header_value(folder);
        assert!(
            spelled
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
            "{spelled}"
        );
        assert_eq!(cwd_from_header(&spelled).as_deref(), Some(folder));
        assert_eq!(
            cwd_from_header(&spelled.to_ascii_uppercase()).as_deref(),
            Some(folder),
            "PowerShell spells its hex in capitals"
        );
        for unreadable in [
            String::new(),
            spelled[1..].to_string(),
            format!("{spelled}zz"),
            format!("+f{spelled}"),
            format!(" {spelled} "),
            "ff".to_string(),
            cwd_header_value("relative/folder"),
        ] {
            assert_eq!(cwd_from_header(&unreadable), None, "{unreadable:?}");
        }
    }

    #[test]
    fn the_agent_shim_keeps_capabilities_off_argv_and_off_the_network() {
        let script = shim_script(
            "ZEROCODE_HOOK_PORT",
            "ZEROCODE_COMPUTER_TOKEN",
            "ZEROCODE_HOOK_TOKEN",
        );
        for expected in [
            "x-zerocode-computer-token",
            "chmod 600",
            "-K \"$headers\"",
            "--noproxy '*'",
            "--proto =http",
            "http://127.0.0.1:$port/computer",
            "--text-stdin",
        ] {
            assert!(script.contains(expected), "shim lost {expected}:\n{script}");
        }
        assert!(!script.contains("-H \"x-zerocode-computer-token"));
    }

    #[test]
    fn ssh_commands_are_typed_and_an_ambiguous_target_is_refused() {
        let opened = parse_ssh_command(&words(&["open", "--host", "build-box", "--json"]))
            .expect("open a saved host");
        assert_eq!(opened.method, SshMethod::Open);
        assert_eq!(opened.host.as_deref(), Some("build-box"));
        assert!(!opened.local);
        assert_eq!(opened.grid(), (24, 96));

        let local = parse_ssh_command(&words(&[
            "open", "--local", "--cwd", "/tmp", "--rows", "40",
        ]))
        .expect("open a local shell");
        assert!(local.local);
        assert_eq!(local.cwd.as_deref(), Some("/tmp"));
        assert_eq!(local.grid(), (40, 96));

        // Naming both machines, or neither, is the one guess this door refuses:
        // the two reach different computers.
        assert!(parse_ssh_command(&words(&["open"])).is_err());
        assert!(parse_ssh_command(&words(&["open", "--host", "a", "--local"])).is_err());
        assert!(parse_ssh_command(&words(&["open", "--host", "a", "--cwd", "/tmp"])).is_err());

        let sent = parse_ssh_command(&words(&["send", "--pane", "7", "--text", "ls", "--enter"]))
            .expect("send keystrokes");
        assert_eq!(sent.pane, Some(7));
        // The text and its submission are two facts, and the carriage return
        // is not part of the words. A glued `ls\r` is one paste, and one paste
        // is what a composer can hold as a draft.
        assert_eq!(sent.typed_text(), "ls");
        assert!(sent.submits());
        let held = parse_ssh_command(&words(&["send", "--pane", "7", "--text", "ls"]))
            .expect("send without submitting");
        assert_eq!(held.typed_text(), "ls");
        assert!(!held.submits());

        // Submitting is `--enter`'s decision, never a byte smuggled in the text.
        assert!(parse_ssh_command(&words(&["send", "--pane", "7", "--text", "ls\r"])).is_err());
        assert!(parse_ssh_command(&words(&["send", "--text", "ls"])).is_err());
        assert!(parse_ssh_command(&words(&["read"])).is_err());
        assert!(parse_ssh_command(&words(&["exec", "--host", "a"])).is_err());
        assert!(parse_ssh_command(&words(&["list", "--host", "a"])).is_err());
    }

    #[test]
    fn the_ssh_shim_uses_the_same_guarded_bridge_with_an_owned_route() {
        let script = ssh_shim_script(
            "ZEROCODE_HOOK_PORT",
            "ZEROCODE_COMPUTER_TOKEN",
            "ZEROCODE_HOOK_TOKEN",
        );
        assert!(
            script.contains(&format!("body=\"ssh{ARGV_SEPARATOR}\"")),
            "{script}"
        );
        assert!(script.contains("zerocode-ssh — use the terminals, SSH hosts"));
        assert!(script.contains("http://127.0.0.1:$port/computer"));
        assert!(!script.contains("-H \"x-zerocode-computer-token"));
        // And it is a SCRIPT, not a string that looks like one. Every line of
        // the manual is echoed inside single quotes, so one apostrophe in the
        // prose is a syntax error in every agent's door — and the door fails
        // at the moment somebody asks for help, which is the worst moment to
        // discover it. `sh -n` parses without running a line of it.
        for (name, body) in [
            ("zerocode-ssh", &script),
            (
                "zerocode-emulator",
                &emulator_shim_script(
                    "ZEROCODE_HOOK_PORT",
                    "ZEROCODE_COMPUTER_TOKEN",
                    "ZEROCODE_HOOK_TOKEN",
                ),
            ),
            (
                "zerocode-computer",
                &shim_script(
                    "ZEROCODE_HOOK_PORT",
                    "ZEROCODE_COMPUTER_TOKEN",
                    "ZEROCODE_HOOK_TOKEN",
                ),
            ),
        ] {
            let parsed = std::process::Command::new("/bin/sh")
                .arg("-n")
                .arg("-c")
                .arg(body.as_str())
                .output()
                .expect("parse the shim with sh");
            assert!(
                parsed.status.success(),
                "{name} is not a valid shell script: {}",
                String::from_utf8_lossy(&parsed.stderr)
            );
        }
    }

    #[test]
    fn the_emulator_shim_uses_the_same_guarded_bridge_with_an_owned_route() {
        let script = emulator_shim_script(
            "ZEROCODE_HOOK_PORT",
            "ZEROCODE_COMPUTER_TOKEN",
            "ZEROCODE_HOOK_TOKEN",
        );
        assert!(
            script.contains(&format!("body=\"emulator{ARGV_SEPARATOR}\"")),
            "{script}"
        );
        assert!(script.contains("zerocode-emulator — control ZeroCode"));
        assert!(script.contains("http://127.0.0.1:$port/computer"));
        assert!(!script.contains("-H \"x-zerocode-computer-token"));
        assert!(
            emulator_usage().contains(
                "screenshot --platform ios|android --device <id> [--out <path>] [--json]"
            )
        );
    }

    /// The ears: four verbs, the table riding `listen-start`, the wait the
    /// window's own, and the rule that decides what a wait was waiting for.
    /// The continuous eye's verbs (§7.1): `watch` waits for the screen to
    /// change or to go still and moves nothing; `observe --settle` first waits
    /// for the last act's paint; the stream's numbers are the table's.
    #[test]
    fn the_eye_watches_and_settles_and_moves_nothing() {
        let watch = parse_command(&words(&[
            "watch",
            "--until",
            "quiet",
            "--timeout-ms",
            "3000",
        ]))
        .expect("watch");
        assert_eq!(watch.method, ComputerMethod::Watch);
        assert_eq!(watch.params["until"], "quiet");
        assert_eq!(watch.params["timeoutMs"], 3000);
        assert!(
            parse_command(&words(&["watch"])).is_ok(),
            "until a change by default"
        );
        let refused = parse_command(&words(&["watch", "--until", "still"])).unwrap_err();
        assert!(refused.contains("change, quiet"), "{refused}");
        assert!(
            parse_command(&words(&["watch", "--app", "Mail"])).is_err(),
            "the whole display"
        );
        let settled = parse_command(&words(&["observe", "--diff", "--settle"])).expect("settle");
        assert_eq!(settled.params["settle"], true);
        assert!(
            parse_command(&words(&["screenshot", "--settle"])).is_err(),
            "only a look settles"
        );
        let method = ComputerMethod::Watch;
        assert!(!method.acts() && !method.presses() && method.batches() && method.is_desktop());
        assert_eq!(method.windows_standing(), WindowsStanding::WindowsOwn);
        assert!(usage().contains("watch [--until change|quiet]") && usage().contains("--settle"));
        let table = eye_table();
        assert_eq!(table["framesPerSecond"], EYE_FRAMES_PER_SECOND);
        assert_eq!(table["changesKept"], EYE_CHANGES_KEPT);
        assert_eq!(table["idleStopMs"], EYE_IDLE_STOP_MS);
        assert_eq!(table["firstFrameMs"], EYE_FIRST_FRAME_MS);
        assert_eq!(table["ocrMaxPieces"], OCR_REUSE_MAX_PIECES);
        assert_eq!(table["ocrMaxShare"], OCR_REUSE_MAX_SHARE);
        // Three silent frames are the quiet, and the poll is under one frame.
        const {
            let frame_ms = 1_000 / EYE_FRAMES_PER_SECOND as u64;
            assert!(EYE_QUIET_MS >= 3 * frame_ms && EYE_POLL_MS < frame_ms);
            assert!(EYE_SETTLE_MAX_MS > COMPUTER_SETTLE_MS && EYE_QUIET_MS < COMPUTER_SETTLE_MS);
        }
    }

    #[test]
    fn the_ears_hear_by_the_windows_table() {
        let start =
            parse_command(&words(&["listen-start", "--app", "Music"])).expect("listen-start");
        assert_eq!(start.method, ComputerMethod::ListenStart);
        assert_eq!(start.params["app"], "Music");
        for (key, value) in sound_table() {
            assert_eq!(
                start.params[&key], value,
                "{key} rides listen-start from the table"
            );
        }
        assert_eq!(start.params["ignoredLabels"], json!(["silence"]));
        let read =
            parse_command(&words(&["sound-read", "--after", "7", "--json"])).expect("sound-read");
        assert_eq!(
            (read.method, read.params["after"].clone(), read.json),
            (ComputerMethod::SoundRead, json!(7), true)
        );
        let wait = parse_command(&words(&[
            "sound-wait",
            "--label",
            "siren, Alarm_Clock",
            "--min-confidence",
            "0.8",
            "--timeout-ms",
            "5000",
        ]))
        .expect("sound-wait");
        assert_eq!(wait.params["label"], "siren, Alarm_Clock");
        assert_eq!(wait.params["minConfidence"], 0.8);
        assert!(parse_command(&words(&["sound-wait", "--min-confidence", "2"])).is_err());
        assert!(
            parse_command(&words(&["listen-stop", "--app", "x"])).is_err(),
            "listen-stop names nothing"
        );
        assert!(parse_command(&words(&["listen-stop"])).is_ok());

        assert_eq!(
            ComputerMethod::ListenStart.windows_standing(),
            WindowsStanding::Held
        );
        assert_eq!(
            ComputerMethod::SoundRead.windows_standing(),
            WindowsStanding::Held
        );
        assert_eq!(
            ComputerMethod::SoundWait.windows_standing(),
            WindowsStanding::WindowsOwn
        );
        for method in [
            ComputerMethod::ListenStart,
            ComputerMethod::ListenStop,
            ComputerMethod::SoundRead,
            ComputerMethod::SoundWait,
        ] {
            assert!(
                !method.acts() && !method.presses(),
                "listening moves nothing: {method:?}"
            );
            assert!(usage().contains(method.verb_name()));
        }

        let labels = sound_wait_labels(Some("siren, Alarm_Clock,,"));
        assert_eq!(labels, ["siren", "alarm_clock"]);
        let heard = |label: &str, confidence: f64| json!({ "seq": 1, "label": label, "confidence": confidence });
        assert!(sound_event_matches(&heard("Siren", 0.9), &labels, 0.5));
        assert!(
            !sound_event_matches(&heard("siren", 0.4), &labels, 0.5),
            "below the confidence asked"
        );
        assert!(
            !sound_event_matches(&heard("speech", 0.9), &labels, 0.5),
            "not a label named"
        );
        assert!(
            sound_event_matches(&heard("speech", 0.9), &[], 0.5),
            "no label named: any sound"
        );
        assert!(sound_wait_labels(None).is_empty());
    }

    fn batch(commands: &serde_json::Value) -> Result<ComputerCommand, String> {
        parse_command(&words(&["batch", "--commands", &commands.to_string()]))
    }

    /// A batch is command lines this same parser accepts, normalised: every
    /// step answers an envelope and an app step takes no picture.
    #[test]
    fn a_batch_is_command_lines_this_same_parser_accepts() {
        let command = batch(&json!([
            ["mouse-click", "--x", "10", "--y", "20"],
            ["type-text", "--app", "Notes", "--text", "hi"],
            ["key", "--key", "return", "--confirming", "payment"],
        ]))
        .expect("a batch");
        assert_eq!(command.method, ComputerMethod::Batch);
        assert_eq!(command.method.provider_name(), None);
        assert_eq!(
            command.method.windows_standing(),
            WindowsStanding::WindowsOwn
        );
        assert!(command.method.acts() && !command.method.presses());
        assert_eq!(
            command.params[BATCH_COMMANDS_FLAG],
            json!([
                ["mouse-click", "--x", "10", "--y", "20", "--json"],
                [
                    "type-text",
                    "--app",
                    "Notes",
                    "--text",
                    "hi",
                    "--json",
                    "--no-screenshot"
                ],
                [
                    "key",
                    "--key",
                    "return",
                    "--confirming",
                    "payment",
                    "--json"
                ],
            ])
        );
        assert!(usage().contains("zerocode-computer batch --commands"));
        assert!(usage().contains(&format!("at most {COMPUTER_BATCH_MAX_STEPS} steps")));
    }

    /// A batch is refused whole, before its first step, with a short message
    /// that names the step.
    #[test]
    fn a_batch_is_refused_whole_before_its_first_step() {
        let refused = |commands: serde_json::Value| batch(&commands).expect_err("refused");
        assert!(parse_command(&words(&["batch", "--commands", "not json"])).is_err());
        assert!(refused(json!([])).contains("1 to"));
        let too_many: Vec<_> = (0..=COMPUTER_BATCH_MAX_STEPS)
            .map(|_| json!(["wait", "--ms", "1"]))
            .collect();
        assert!(refused(json!(too_many)).contains("1 to"));
        let error = refused(json!([["wait", "--ms", "1"], ["teleport"]]));
        assert!(
            error.starts_with("step 2:") && !error.contains("zerocode-computer —"),
            "{error}"
        );
        for look in [
            "screenshot",
            "observe",
            "find",
            "read",
            "run",
            "launch",
            "quit",
            "open",
            "window-close",
            "handoff",
            "batch",
        ] {
            assert!(
                refused(json!([[look]])).contains("cannot be a batch step"),
                "{look}"
            );
        }
        assert!(
            refused(json!([[
                "mouse-click",
                "--x",
                "1",
                "--y",
                "1",
                "--allow-self"
            ]]))
            .contains("allow-self")
        );
        assert!(
            refused(json!([[
                "mouse-click",
                "--x",
                "1",
                "--y",
                "1",
                "--confirmed"
            ]]))
            .starts_with("step 1")
        );
        let stale = refused(json!([
            ["click", "--app", "x", "--element-index", "3"],
            ["click", "--app", "x", "--element-index", "4"],
        ]));
        assert!(
            stale.starts_with("step 2:") && stale.contains("element indexes"),
            "{stale}"
        );
        assert!(
            batch(&json!([
                ["wait", "--ms", "1"],
                ["click", "--app", "x", "--element-index", "4"]
            ]))
            .is_ok(),
            "a wait does not rebuild the tree"
        );
        let after_wait_for = |wait_for: &[&str]| {
            let mut first = vec!["wait-for"];
            first.extend_from_slice(wait_for);
            first.extend_from_slice(&["--timeout-ms", "500"]);
            batch(&json!([
                first,
                ["click", "--app", "x", "--element-index", "4"]
            ]))
        };
        assert!(
            after_wait_for(&["--app", "x", "--text", "Loaded"])
                .is_err_and(|why| why.starts_with("step 2:") && why.contains("element indexes")),
            "a wait-for through the app's elements rebuilds the tree the index points into"
        );
        assert!(
            after_wait_for(&["--window", "Done"]).is_ok(),
            "a window wait reads the window list"
        );
        assert!(
            after_wait_for(&["--ocr", "--text", "Loaded"]).is_ok(),
            "an OCR wait reads the pixels"
        );
        assert!(
            refused(json!([[
                "mouse-move",
                "--x",
                "1",
                "--y",
                "1",
                "--steps",
                "61"
            ]]))
            .contains("glides")
        );
        let untimed = refused(json!([["wait-for", "--window", "Save"]]));
        assert!(
            untimed.starts_with("step 1 (wait-for)") && untimed.contains("--timeout-ms"),
            "{untimed}"
        );
        assert!(
            refused(json!([
                ["wait", "--ms", "30000"],
                ["hold-key", "--key", "a", "--ms", "5000"]
            ]))
            .contains("at most")
        );
        let exactly: Vec<_> = (0..COMPUTER_BATCH_MAX_STEPS)
            .map(|_| json!(["wait", "--ms", "1"]))
            .collect();
        assert!(
            batch(&json!(exactly)).is_ok(),
            "exactly the most steps is a batch"
        );
    }

    /// The person asked for no artificial speed cap (m-3903): the table says
    /// so in one explicit shape — never a stand-in rate — and every reader
    /// reads that shape.
    #[test]
    fn the_hand_has_no_speed_cap_and_the_table_says_so() {
        assert_eq!(COMPUTER_PACE, ComputerPace::Unlimited);
        assert_eq!(
            pace_table(COMPUTER_PACE),
            json!({ "mode": PACE_MODE_UNLIMITED, "perSecond": null, "burst": null })
        );
        let guard = guard_table();
        assert_eq!(
            guard.keys().map(String::as_str).collect::<Vec<_>>(),
            ["pace", "perSession"]
        );
        assert_eq!(guard["pace"], pace_table(COMPUTER_PACE));
        assert_eq!(
            guard["perSession"],
            json!(5_000),
            "the session count stays finite"
        );
        // A pace, were one ever chosen again, is spelled with real numbers.
        let paced = ComputerPace::Paced {
            per_second: NonZeroU32::new(10).expect("ten"),
            burst: NonZeroU32::new(20).expect("twenty"),
        };
        assert_eq!(
            pace_table(paced),
            json!({ "mode": PACE_MODE_PACED, "perSecond": 10, "burst": 20 })
        );
        assert_eq!(
            pace_words(paced),
            "20 actions back to back, then 10 a second"
        );
        let usage = usage();
        assert!(usage.contains(&format!("(pace: {})", pace_words(COMPUTER_PACE))));
        assert!(
            usage.contains("no speed cap") && !usage.contains("paced"),
            "{usage}"
        );
    }

    #[test]
    fn all_layer_window_observation_uses_the_existing_occlusion_contract() {
        let command = parse_command(&["list-all-windows".into(), "--all-layers".into()]).unwrap();
        assert_eq!(
            command.params[crate::computer_use_protocol::marks::EVERY_LAYER_KEY],
            true
        );
        assert!(parse_command(&["click".into(), "--all-layers".into()]).is_err());
    }

    /// A batch's length is not a pace's burst: the old cap of twenty steps —
    /// the burst it was tied to — is no ceiling, and the count that bounds it
    /// is the existing recipe sequence budget.
    #[test]
    fn a_batch_is_not_bounded_by_a_pace() {
        assert_eq!(COMPUTER_BATCH_MAX_STEPS, RECIPE_STEPS);
        let clicks: Vec<_> = (0..21)
            .map(|at| json!(["mouse-click", "--x", at.to_string(), "--y", "20"]))
            .collect();
        let command = batch(&json!(clicks)).expect("twenty-one actions in one judgement");
        assert_eq!(batch_steps(&command).len(), 21);
    }

    #[test]
    fn the_batchable_verbs_are_the_hands_and_the_waits() {
        for method in ComputerMethod::ALL {
            if method.batches() {
                assert!(
                    method.acts()
                        || matches!(
                            method,
                            ComputerMethod::Wait
                                | ComputerMethod::WaitFor
                                | ComputerMethod::SoundWait
                                | ComputerMethod::Watch
                        ),
                    "{method:?}"
                );
                assert_ne!(*method, ComputerMethod::Batch, "no nesting");
            }
        }
        assert!(COMPUTER_BATCH_MAX_WAIT_MS < walk_budget_ms(COMPUTER_USE_DEADLINE_SECONDS * 1_000));
        for flag in walked_step_flags(false)
            .chain(ELEMENT_INDEX_FLAGS.iter().copied())
            .chain(WALKED_STEP_REFUSED_FLAGS.iter().copied())
        {
            assert!(
                ComputerMethod::ALL
                    .iter()
                    .any(|method| allowed(*method).contains(&flag)),
                "{flag} is a real flag"
            );
        }
        let base = COMPUTER_USE_DEADLINE_SECONDS * 1_000;
        assert!(
            walk_step_fits(0, 1_000, base)
                && !walk_step_fits(walk_budget_ms(base) - 500, 1_000, base)
        );
        assert_eq!(
            walk_step_holds_ms(&words(&["wait", "--ms", "250"]), true),
            250
        );
        assert_eq!(
            walk_step_holds_ms(&words(&["mouse-click", "--x", "1", "--y", "1"]), true),
            COMPUTER_CONFIRM_TIMEOUT_MS,
            "a press may be asked about"
        );
        assert_eq!(
            walk_step_holds_ms(&words(&["mouse-click", "--x", "1", "--y", "1"]), false),
            0,
            "a walk that hands a press back never waits for the person"
        );
    }

    /// A walked step's flags follow whether the walk keeps a frame of it
    /// (t-4229): every walked step answers an envelope; only a walk that
    /// frames nothing — a batch, a Flow at `verdict-only` or `off` — tells an
    /// app step to take no picture. A walk that frames its steps leaves the
    /// helper's look after the act on the answer, for the writer to keep as
    /// the step's frame. A desktop verb whose table has no picture flag gets
    /// none either way.
    #[test]
    fn a_walked_step_keeps_its_picture_when_the_walk_frames_and_drops_it_when_it_does_not() {
        let spelled = |flags: &[&str]| -> Vec<String> {
            flags.iter().map(|flag| format!("--{flag}")).collect()
        };
        let (envelope, unframed) = (
            spelled(WALKED_STEP_FLAGS),
            spelled(WALKED_STEP_UNFRAMED_FLAGS),
        );
        assert!(!envelope.is_empty() && !unframed.is_empty());
        assert_eq!(
            walked_step_flags(true).collect::<Vec<_>>(),
            WALKED_STEP_FLAGS,
            "a framed walk adds the envelope's flags only"
        );
        assert_eq!(
            walked_step_flags(false).collect::<Vec<_>>(),
            WALKED_STEP_FLAGS
                .iter()
                .chain(WALKED_STEP_UNFRAMED_FLAGS)
                .copied()
                .collect::<Vec<_>>(),
            "an unframed walk adds the picture's refusal besides"
        );
        let click = words(&["click", "--app", "Mail", "--x", "1", "--y", "1"]);
        let framed = walked_step(click.clone(), true);
        assert!(
            envelope.iter().all(|flag| framed.contains(flag))
                && unframed.iter().all(|flag| !framed.contains(flag)),
            "a framed walk keeps the app step's picture: {framed:?}"
        );
        let frameless = walked_step(click, false);
        assert!(
            envelope
                .iter()
                .chain(&unframed)
                .all(|flag| frameless.contains(flag)),
            "an unframed walk takes no picture: {frameless:?}"
        );
        let desktop = walked_step(words(&["mouse-click", "--x", "1", "--y", "1"]), true);
        assert!(
            envelope.iter().all(|flag| desktop.contains(flag))
                && unframed.iter().all(|flag| !desktop.contains(flag)),
            "no picture to keep or skip: {desktop:?}"
        );
        let batch = parse_command(&words(&[
            "batch",
            "--commands",
            r#"[["click","--app","Mail","--x","1","--y","1"]]"#,
        ]))
        .expect("a batch");
        let step: Vec<String> =
            serde_json::from_value(batch.params[BATCH_COMMANDS_FLAG][0].clone()).unwrap();
        assert!(
            envelope
                .iter()
                .chain(&unframed)
                .all(|flag| step.contains(flag)),
            "a batch is looked at once, after: {step:?}"
        );
    }

    #[test]
    fn a_batch_answer_reports_every_step_and_the_first_refusal() {
        let ok = |n, verb: &str| {
            walk_step_report(
                n,
                &words(&[verb]),
                true,
                Some(&json!({ "ok": true, "result": { "snapshot": { "id": n } } })),
                "",
            )
        };
        let answer = batch_answer(vec![ok(1, "click"), ok(2, "click")], 2, 7, false);
        assert_eq!(answer["ok"], true);
        assert_eq!(answer["result"]["ran"], 2);
        assert!(
            answer.pointer("/result/steps/0/result/snapshot").is_none(),
            "a stale tree is dropped"
        );
        assert_eq!(
            answer["result"]["steps"][1]["result"]["snapshot"]["id"], 2,
            "the fresh one stays"
        );
        assert!(batch_text(&answer).ends_with("ran 2 of 2 in 7 ms"));

        let refused = walk_step_report(
            2,
            &words(&["mouse-click"]),
            false,
            Some(
                &json!({ "ok": false, "error": { "code": "confirmation_refused", "message": "no" } }),
            ),
            "",
        );
        let answer = batch_answer(vec![ok(1, "wait"), refused], 4, 9, false);
        assert_eq!(answer["ok"], false);
        assert_eq!(
            answer["error"]["code"], "confirmation_refused",
            "the step's own code"
        );
        assert!(
            answer["error"]["message"]
                .as_str()
                .unwrap()
                .starts_with("step 2 of 4 (mouse-click): no")
        );
        assert_eq!(answer["result"]["refusedAt"], 2);

        let late = batch_answer(vec![ok(1, "wait")], 3, 60_000, true);
        assert_eq!(
            late["error"]["code"],
            crate::computer_use_protocol::error_code::BATCH_DEADLINE
        );
        assert!(
            late["error"]["message"]
                .as_str()
                .unwrap()
                .contains("before step 2 of 3")
        );
        let silent = walk_step_report(1, &words(&["key"]), false, None, "helper gone\n");
        assert_eq!(
            (
                silent["error"]["code"].as_str(),
                silent["error"]["message"].as_str()
            ),
            (
                Some(crate::computer_use_protocol::error_code::UNANSWERED),
                Some("helper gone")
            )
        );
        // An ok answer whose result says the work did not happen did not go.
        let asleep = walk_step_report(
            1,
            &words(&["activate", "--app", "X"]),
            true,
            Some(&json!({ "ok": true, "result": { "active": false } })),
            "",
        );
        assert_eq!(asleep["ok"], false);
        assert_eq!(
            asleep["error"]["code"],
            crate::computer_use_protocol::error_code::UNFINISHED
        );
        for (verb, result, unfinished) in [
            (
                "run",
                json!({ "spawned": true, "exitCode": 0, "timedOut": false }),
                false,
            ),
            ("run", json!({ "spawned": false }), true),
            (
                "run",
                json!({ "spawned": true, "exitCode": null, "timedOut": true }),
                true,
            ),
            (
                "run",
                json!({ "spawned": true, "exitCode": null, "timedOut": false }),
                true,
            ),
            (
                "run",
                json!({ "spawned": true, "exitCode": 2, "timedOut": false }),
                true,
            ),
            ("launch", json!({ "ready": true }), false),
            ("launch", json!({ "ready": false }), true),
            ("quit", json!({ "terminated": false }), true),
            ("activate", json!({ "active": true }), false),
            ("mouse-click", json!({}), false),
        ] {
            let method = verb_method(verb).unwrap();
            assert_eq!(
                walked_result_unfinished(method, &result).is_some(),
                unfinished,
                "{verb} {result}"
            );
        }
    }

    /// The one walk: each step planned, run, timed and judged in order; a
    /// skip is reported and passed; a halt from the planner or the run ends
    /// it there; a caller gone or a step that could not answer in time is
    /// never begun.
    #[test]
    fn a_walk_plans_runs_and_halts_each_step_in_order() {
        let steps = ["a", "skip", "b", "halt", "c"];
        let clock = std::cell::Cell::new(0_u64);
        let mut ran = Vec::new();
        let budget = WalkBudget {
            deadline_ms: COMPUTER_USE_DEADLINE_SECONDS * 1_000,
            out_of_time: "late",
        };
        let walked = walk(
            &steps,
            &budget,
            |_, _| 0,
            |_, step: &&str| match *step {
                "skip" => Next::Skip("a look"),
                "halt" => Next::Halt("stop"),
                other => Next::Run(vec![other.to_string()]),
            },
            |n, _, argv| {
                clock.set(clock.get() + 5);
                ran.push(argv[0].clone());
                (json!({ "n": n }), None)
            },
            || true,
            || clock.get(),
        );
        assert_eq!(ran, ["a", "b"], "the halted step and the rest never run");
        assert_eq!(walked.halted, Some((4, "stop")));
        assert_eq!(walked.reports[1], json!({ "n": 2, "skipped": "a look" }));
        assert_eq!(walked.reports[0]["ms"], 5, "the walk times every step");

        let refused = walk(
            &steps,
            &budget,
            |_, _| 0,
            |_, step: &&str| Next::Run(vec![(*step).to_string()]),
            |_, _, _| (json!({}), Some("refused")),
            || true,
            || 0,
        );
        assert_eq!(
            (refused.halted, refused.reports.len()),
            (Some((1, "refused")), 1)
        );

        let tight = WalkBudget {
            deadline_ms: COMPUTER_BATCH_ANSWER_MARGIN_MS + 30,
            out_of_time: "late",
        };
        let late = walk(
            &steps,
            &tight,
            |_, argv| if argv[0] == "b" { 50 } else { 0 },
            |_, step: &&str| Next::Run(vec![(*step).to_string()]),
            |_, _, _| (json!({}), None),
            || true,
            || 0,
        );
        assert_eq!(
            late.halted,
            Some((3, "late")),
            "a step that could not answer in time is not begun"
        );
        // A step that ran past what it said it would hold makes every later
        // step reserve that much more: a slow desk ends the walk early.
        let base = COMPUTER_USE_DEADLINE_SECONDS * 1_000;
        let clock = std::cell::Cell::new(walk_budget_ms(base) - 25_000);
        let slow = walk(
            &["a", "b", "c"],
            &WalkBudget {
                deadline_ms: base,
                out_of_time: "late",
            },
            |_, _| 1_000,
            |_, step: &&str| Next::Run(vec![(*step).to_string()]),
            |_, _, argv| {
                // "a" says 1 s and takes 21 s; "b" fits alone but not with that.
                clock.set(clock.get() + if argv[0] == "a" { 21_000 } else { 1_000 });
                (json!({}), None)
            },
            || true,
            || clock.get(),
        );
        assert_eq!(
            slow.halted,
            Some((2, "late")),
            "4 s were left: enough for b's 1 s, not for the 20 s a ran over"
        );
        // The first step of a call needs only to fit the budget: a step
        // that holds nearly all of it still runs, where the fit check with
        // the walk's own start-up would never let it.
        let first = walk(
            &["long"],
            &WalkBudget {
                deadline_ms: base,
                out_of_time: "late",
            },
            |_, _| walk_budget_ms(base),
            |_, step: &&str| Next::Run(vec![(*step).to_string()]),
            |_, _, _| (json!({}), None),
            || true,
            || 7,
        );
        assert_eq!(
            first.halted, None,
            "the first step is never budgeted forever"
        );
        let gone = walk(
            &steps,
            &tight,
            |_, _| 0,
            |_, _| Next::Run(vec![]),
            |_, _, _| (json!({}), None),
            || false,
            || 0,
        );
        assert_eq!((gone.halted, gone.reports.len()), (Some((1, "late")), 0));
    }

    /// A look after an act ends at the first change (or a look that cannot
    /// tell), or once the settle has passed; a failed look ends it at once.
    #[test]
    fn a_look_after_an_act_ends_at_the_first_change_or_the_settle() {
        let clock = std::cell::Cell::new(std::time::Duration::ZERO);
        let looks = std::cell::Cell::new(0_u64);
        let seen: Result<Option<bool>, ()> = look_until_settled(
            || {
                looks.set(looks.get() + 1);
                Ok(Some(false))
            },
            |seen| *seen,
            || clock.get(),
            |pause| clock.set(clock.get() + pause),
        );
        assert_eq!(seen, Ok(Some(false)));
        assert_eq!(
            looks.get(),
            COMPUTER_SETTLE_MS / COMPUTER_SETTLE_POLL_MS + 1,
            "nothing changed: a look every poll until the settle"
        );
        for first in [Some(true), None] {
            let looks = std::cell::Cell::new(0);
            let _ = look_until_settled::<_, ()>(
                || {
                    looks.set(looks.get() + 1);
                    Ok(first)
                },
                |seen| *seen,
                || std::time::Duration::ZERO,
                |_| {},
            );
            assert_eq!(looks.get(), 1, "{first:?} answers at once");
        }
        assert_eq!(
            look_until_settled::<Option<bool>, &str>(
                || Err("blind"),
                |seen| *seen,
                || std::time::Duration::ZERO,
                |_| {}
            ),
            Err("blind")
        );
    }

    /// One table says how long a step may hold the desk, by the verbs' own
    /// numbers: the lone command reads its rule, a batch sums it, a recipe
    /// walks a step only when it fits.
    #[test]
    fn every_hold_is_read_off_the_verbs_own_table() {
        let held = |method| hold_of(method).expect("a hold").held_ms(None);
        assert_eq!(held(ComputerMethod::WaitFor), COMPUTER_WAIT_FOR_MAX_MS);
        assert_eq!(held(ComputerMethod::Run), COMPUTER_RUN_MAX_MS);
        assert_eq!(
            held(ComputerMethod::Launch),
            COMPUTER_LAUNCH_PROCESS_MS + COMPUTER_LAUNCH_WAIT_READY_MS
        );
        assert_eq!(held(ComputerMethod::Quit), COMPUTER_APP_SETTLE_MS);
        assert_eq!(held(ComputerMethod::Activate), COMPUTER_APP_SETTLE_MS);
        assert_eq!(held(ComputerMethod::Open), COMPUTER_LAUNCH_PROCESS_MS);
        assert_eq!(
            hold_of(ComputerMethod::Wait)
                .unwrap()
                .held_ms(Some(COMPUTER_WAIT_MAX_MS + 1)),
            COMPUTER_WAIT_MAX_MS,
            "the verb's own cap"
        );
        // The lone command's rule is the table's row, at every edge.
        for asked in [
            None,
            Some(0),
            Some(COMPUTER_WAIT_FOR_MAX_MS),
            Some(COMPUTER_WAIT_FOR_MAX_MS + 1),
        ] {
            assert_eq!(desktop_wait_for_ms(asked), HOLD_WAIT_FOR.allowed_ms(asked));
            assert_eq!(desktop_run_ms(asked), HOLD_RUN.allowed_ms(asked));
        }
        assert_eq!(
            desktop_wait_ms(COMPUTER_WAIT_MAX_MS + 1),
            (COMPUTER_WAIT_MAX_MS, true)
        );
        // Every row's flag is its verb's and parses into its param: a sample
        // line per verb carries the flags it needs besides.
        let needs: &[(ComputerMethod, &[&str])] = &[
            (ComputerMethod::HoldKey, &["--key", "a"]),
            (ComputerMethod::WaitFor, &["--app", "X", "--text", "a"]),
            (ComputerMethod::SoundWait, &[]),
            (ComputerMethod::Watch, &[]),
            (ComputerMethod::Run, &["--program", "/bin/echo"]),
            (ComputerMethod::Launch, &["--app", "X"]),
        ];
        for hold in COMPUTER_HOLDS {
            let Some(asked_by) = hold.asked_by else {
                assert!(
                    hold.besides_ms > 0 && hold.max_ms == 0,
                    "{hold:?} waits only its fixed hold"
                );
                continue;
            };
            assert!(allowed(hold.method).contains(&asked_by.flag), "{hold:?}");
            let mut line = words(&[
                hold.method.verb_name(),
                &format!("--{}", asked_by.flag),
                "7",
            ]);
            line.extend(
                needs
                    .iter()
                    .find(|(method, _)| *method == hold.method)
                    .map_or(&[][..], |(_, flags)| *flags)
                    .iter()
                    .map(|word| (*word).to_string()),
            );
            let parsed = parse_command(&line).unwrap_or_else(|why| panic!("{line:?}: {why}"));
            assert_eq!(
                parsed.params[asked_by.param], 7,
                "{hold:?} parses into its param"
            );
            assert_eq!(held_ms(&parsed), hold.besides_ms + 7);
        }
        assert!(hold_of(ComputerMethod::MouseClick).is_none());
    }

    /// The one ladder: a person answering is never cut off by a clock meant
    /// for a machine — the bridge, the shim and zo all read it.
    #[test]
    fn a_person_answering_is_never_cut_off_by_the_machines_clock() {
        let base = COMPUTER_USE_DEADLINE_SECONDS * 1_000;
        let deadline = |argv: &[&str]| computer_deadline_ms(&words(argv));
        assert_eq!(
            deadline(&["wait-for", "--window", "x", "--timeout-ms", "60000"]),
            base
        );
        assert_eq!(
            deadline(&["handoff", "--reason", "2FA"]),
            COMPUTER_HANDOFF_TIMEOUT_MS + COMPUTER_BRIDGE_GRACE_MS
        );
        assert_eq!(
            deadline(&["handoff", "--reason", "2FA", "--timeout-ms", "70000"]),
            70_000 + COMPUTER_BRIDGE_GRACE_MS,
            "a 70 s turn is not cut at 65 s"
        );
        assert_eq!(
            deadline(&["handoff", "--reason", "x", "--timeout-ms", "9999999"]),
            COMPUTER_HANDOFF_TIMEOUT_MS + COMPUTER_BRIDGE_GRACE_MS,
            "never past the table's turn"
        );
        assert_eq!(
            deadline(&["mouse-click", "--x", "1", "--y", "1"]),
            COMPUTER_CONFIRM_TIMEOUT_MS + COMPUTER_BRIDGE_GRACE_MS,
            "a press may be asked about for the question's whole time"
        );
        let with_press =
            serde_json::json!([["wait", "--ms", "1"], ["key", "--key", "return"]]).to_string();
        assert!(
            deadline(&["batch", "--commands", &with_press]) > base + COMPUTER_CONFIRM_TIMEOUT_MS
        );
        let without = serde_json::json!([["wait", "--ms", "1"]]).to_string();
        assert_eq!(deadline(&["batch", "--commands", &without]), base);
        assert_eq!(deadline(&["not-a-verb"]), base);
        for argv in [
            &["handoff", "--reason", "x"][..],
            &["mouse-click", "--x", "1", "--y", "1"],
            &["batch", "--commands", &with_press],
        ] {
            assert!(
                deadline(argv) <= COMPUTER_LONGEST_DEADLINE_MS,
                "{argv:?} fits the shim's own clock"
            );
        }
        assert!(shim_script("P", "C", "H").contains(&format!(
            "--max-time {}",
            COMPUTER_LONGEST_DEADLINE_MS.div_ceil(1_000)
        )));
    }

    /// `recipe-run --repeat [--until <HH:MM|N>] [--arena <evidence dir>]`
    /// (second wave, unit B): the flags parse from the one table, a bound
    /// needs its repeat, the bound's word is a count of rounds or a time of
    /// day on the local clock — the next such minute, today while it is
    /// ahead and tomorrow once it is past — a repeat holds the person's
    /// whole turn on the bridge's ladder, and the trigger's wait and the
    /// fail ceiling sit in the one table beside the Flow's other budgets.
    #[test]
    fn recipe_run_parses_repeat_until_and_arena() {
        let parsed = |extra: &[&str]| {
            let mut argv = words(&["recipe-run", "--name", "song-geum"]);
            argv.extend(words(extra));
            parse_command(&argv)
        };
        let repeat = parsed(&["--repeat"]).expect("a repeat");
        assert_eq!(repeat.params["repeat"], json!(true));
        assert!(repeat.params.get("until").is_none());
        assert_eq!(
            parsed(&["--repeat", "--until", "3"])
                .expect("a bound")
                .params["until"],
            json!("3")
        );
        assert_eq!(RepeatUntil::parse("3"), Ok(RepeatUntil::Rounds(3)));
        assert_eq!(
            RepeatUntil::parse("23:05"),
            Ok(RepeatUntil::Clock {
                hour: 23,
                minute: 5
            })
        );
        for bad in ["0", "24:00", "12:60", "noon", "1:2:3", "-1", "", "7:"] {
            assert!(RepeatUntil::parse(bad).is_err(), "{bad}");
            assert!(
                parsed(&["--repeat", "--until", bad]).is_err(),
                "--until {bad}"
            );
        }
        assert!(
            parsed(&["--until", "3"]).is_err(),
            "a bound needs its repeat"
        );
        assert_eq!(RepeatUntil::Rounds(3).rounds(), Some(3));
        assert_eq!(RepeatUntil::Clock { hour: 1, minute: 0 }.rounds(), None);
        let offset_secs: i64 = 9 * 60 * 60;
        let day_ms = crate::civil::days_from_civil(2026, 9, 15) * 86_400_000;
        let ten_local = day_ms + 10 * 3_600_000 - offset_secs * 1_000;
        assert_eq!(
            RepeatUntil::Clock {
                hour: 11,
                minute: 30
            }
            .ends_at_epoch_ms(ten_local, offset_secs),
            Some(ten_local + 90 * 60_000)
        );
        assert_eq!(
            RepeatUntil::Clock { hour: 9, minute: 0 }.ends_at_epoch_ms(ten_local, offset_secs),
            Some(ten_local + 23 * 3_600_000)
        );
        assert_eq!(
            RepeatUntil::Clock {
                hour: 10,
                minute: 0
            }
            .ends_at_epoch_ms(ten_local, offset_secs),
            Some(ten_local + 24 * 3_600_000),
            "this minute is already here: the next one is tomorrow's"
        );
        assert_eq!(
            RepeatUntil::Rounds(2).ends_at_epoch_ms(ten_local, offset_secs),
            None
        );
        let arena = parsed(&["--arena", "/runs/2026-09-15"]).expect("an arena");
        assert_eq!(arena.params["arena"], json!("/runs/2026-09-15"));
        assert!(
            parsed(&["--arena", ""]).is_err(),
            "an arena needs its folder"
        );
        assert!(parsed(&["--arena"]).is_err());
        assert!(
            parsed(&["--repeat", "--arena", "/runs/x"]).is_ok(),
            "a rehearsed repeat"
        );
        assert!(
            parse_command(&words(&["watch", "--repeat"])).is_err(),
            "the flags are recipe-run's"
        );
        assert!(parse_command(&words(&["recipe-show", "--name", "x", "--arena", "/r"])).is_err());
        assert_eq!(
            computer_deadline_ms(&words(&["recipe-run", "--name", "x", "--repeat"])),
            COMPUTER_LONGEST_DEADLINE_MS,
            "a repeat holds the person's whole turn, like a handoff"
        );
        assert_eq!(
            computer_deadline_ms(&words(&["recipe-run", "--name", "x"])),
            COMPUTER_USE_DEADLINE_SECONDS * 1_000
        );
        const {
            assert!(FLOW_TRIGGER_MS == COMPUTER_WAIT_FOR_MAX_MS);
            assert!(FLOW_TRIGGER_MS < COMPUTER_LONGEST_DEADLINE_MS);
            assert!(FLOW_REPEAT_MAX_FAILS == COMPUTER_STUCK_REPEATS);
            assert!(FLOW_REPEAT_MAX_FAILS >= 1);
        }
        assert_eq!(
            crate::computer_use_protocol::error_code::ARENA_OFF_SCRIPT,
            "arena_off_script"
        );
        let usage = usage();
        for shown in ["--repeat [--until <HH:MM|N>]", "--arena <evidence dir>"] {
            assert!(usage.contains(shown), "{shown} is not in the manual");
        }
    }

    /// `--replay` (t-6385) is a walk's own word too: it says the walk runs
    /// in a repeated run, and a plain walk runs fresh.
    #[test]
    fn a_walk_may_say_it_repeats_one_walked_before_and_a_plain_walk_does_not() {
        let argv = |words: &[&str]| words.iter().map(|w| (*w).to_string()).collect::<Vec<_>>();
        let plain =
            parse_command(&argv(&["walk", "--goal", "pay", "--pane", "b"])).expect("a walk");
        assert_eq!(walk_run(&plain.params), crate::jev::Run::Fresh);
        let replay = parse_command(&argv(&["walk", "--goal", "pay", "--pane", "b", "--replay"]))
            .expect("a replayed walk");
        assert_eq!(walk_run(&replay.params), crate::jev::Run::Repeated);
        assert!(usage().contains("[--replay]"));
        assert!(
            parse_command(&argv(&["click", "--x", "1", "--y", "2", "--replay"])).is_err(),
            "no other verb takes it"
        );
    }

    /// `--overlap` (t-6132 S2) is a walk's own word: the walk reads it, the
    /// manual shows it, and no other verb takes it.
    #[test]
    fn a_walk_may_be_asked_to_judge_ahead_of_its_looks_and_nothing_else_may() {
        let argv = |words: &[&str]| words.iter().map(|w| (*w).to_string()).collect::<Vec<_>>();
        let plain =
            parse_command(&argv(&["walk", "--goal", "pay", "--pane", "b"])).expect("a walk");
        assert!(!walk_overlaps(&plain.params), "off unless asked");
        assert!(plain.params.get(WALK_OVERLAP_PARAM).is_none());
        let ahead = parse_command(&argv(&[
            "walk",
            "--goal",
            "pay",
            "--pane",
            "b",
            "--overlap",
        ]))
        .expect("a walk asked to judge ahead");
        assert!(walk_overlaps(&ahead.params));
        assert_eq!(ahead.params[WALK_OVERLAP_PARAM], Value::Bool(true));
        assert!(
            parse_command(&argv(&[
                "walk",
                "--goal",
                "pay",
                "--pane",
                "b",
                "--overlap",
                "yes"
            ]))
            .is_err(),
            "a switch takes no value"
        );
        assert!(
            parse_command(&argv(&["click", "--x", "1", "--y", "2", "--overlap"])).is_err(),
            "no other verb takes it"
        );
        assert!(usage().contains("[--overlap]"), "the manual shows it");

        // `--rescue` (t-6132 S3) is a walk's and a recipe run's: the two
        // verbs that judge a screen and would otherwise hand it to the person.
        let rescued = parse_command(&argv(&["walk", "--goal", "pay", "--pane", "b", "--rescue"]))
            .expect("a walk asked to try a second reader");
        assert!(walk_rescues(&rescued.params));
        assert!(!walk_rescues(&plain.params));
        let recipe = parse_command(&argv(&["recipe-run", "--name", "x", "--rescue"]))
            .expect("a recipe run asked the same");
        assert!(walk_rescues(&recipe.params));
        assert!(parse_command(&argv(&["click", "--x", "1", "--y", "2", "--rescue"])).is_err());
        assert_eq!(
            usage().matches("[--rescue]").count(),
            2,
            "both verbs show it"
        );
    }

    #[test]
    fn a_mobile_goal_walk_names_one_platform_and_device() {
        let words = |extra: &[&str]| {
            ["walk", "--goal", "일반 화면 열기"]
                .into_iter()
                .chain(extra.iter().copied())
                .map(str::to_string)
                .collect::<Vec<_>>()
        };
        for platform in ["ios", "android"] {
            let parsed = parse_command(&words(&["--platform", platform, "--device", "phone"]))
                .expect("a mobile goal has its own target");
            assert_eq!(parsed.method, ComputerMethod::Walk);
            assert_eq!(parsed.params["platform"], platform);
            assert_eq!(parsed.params["device"], "phone");
        }
        for invalid in [
            vec!["--platform", "ios"],
            vec!["--device", "phone"],
            vec!["--platform", "other", "--device", "phone"],
            vec![
                "--platform",
                "ios",
                "--device",
                "phone",
                "--app",
                "Settings",
            ],
            vec!["--platform", "ios", "--device", "phone", "--pane", "page"],
        ] {
            assert!(parse_command(&words(&invalid)).is_err(), "{invalid:?}");
        }
        assert!(usage().contains("--platform <ios|android> --device <id>"));
    }

    #[test]
    fn a_walk_names_its_place_by_a_pane_as_readily_as_by_an_app() {
        // The usage offered `walk --goal … --pane <browser pane>` and the gate
        // refused it, so the seat that judges a browser walk had never been
        // asked anything — its ledger was empty because nothing could reach it
        // (2026-09-19).
        let argv = |words: &[&str]| {
            words
                .iter()
                .map(|word| (*word).to_string())
                .collect::<Vec<_>>()
        };
        let walked = parse_command(&argv(&["walk", "--goal", "pay", "--pane", "browser-13"]))
            .expect("a walk named by its pane");
        assert_eq!(walked.method, ComputerMethod::Walk);
        assert_eq!(
            walked.params.get("pane").and_then(Value::as_str),
            Some("browser-13")
        );
        parse_command(&argv(&["walk", "--goal", "pay", "--app", "Safari"]))
            .expect("a walk named by its app");
        assert_eq!(
            parse_command(&argv(&["walk", "--goal", "pay"])).unwrap_err(),
            "missing required --app",
            "a walk that names no place at all still has to name one"
        );
        // And a method the flag table gives no `pane` is not let through by
        // one: the gate reads that table rather than a list of its own.
        assert!(!allowed(ComputerMethod::Screenshot).contains(&"pane"));
        assert_eq!(
            parse_command(&argv(&["screenshot", "--pane", "browser-13"])).unwrap_err(),
            "unknown flag --pane",
        );
    }
    #[test]
    fn pointer_style_instant_is_a_closed_cli_flag() {
        let words = |parts: &[&str]| {
            parts
                .iter()
                .map(|part| (*part).to_string())
                .collect::<Vec<_>>()
        };
        for command in [
            vec!["mouse-move", "--x", "1", "--y", "2", "--instant"],
            vec!["mouse-click", "--x", "1", "--y", "2", "--instant"],
            vec![
                "mouse-drag",
                "--from-x",
                "1",
                "--from-y",
                "2",
                "--to-x",
                "3",
                "--to-y",
                "4",
                "--instant",
            ],
        ] {
            assert!(
                parse_command(&words(&command))
                    .unwrap_err()
                    .contains("unsupported_capability")
            );
        }
        assert!(parse_command(&words(&["key", "--key", "a", "--instant"])).is_err());
    }
}
