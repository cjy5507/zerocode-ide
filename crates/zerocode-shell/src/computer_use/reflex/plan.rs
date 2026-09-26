//! The plan a reflex autopilot runs, written by a model from the person's
//! goal (t-10223 §2.3): what the model is shown, how its answer is read, and
//! the row each written plan leaves.
//!
//! The model is shown no picture of the screen. What it reads is the goal,
//! the display's size and the app's window on it, and the colours that window
//! shows — counted here off one capture that never leaves the machine — with
//! the contract's own example and tables. Its answer is read the contract's
//! way and no other: a plan in the contract's words ([`ReflexPlan`]), for the
//! scope the person named, hashed by the contract, written out as the three
//! sections a Flow document carries and read back by the contract's own
//! reader before the window's door sees it. A refusal anywhere on that road
//! goes back to the model in the contract's own sentence
//! ([`zerocode_core::computer_use::REFLEX_PLAN_RETRIES`] times), so the
//! grammar is learned from its refusals rather than from a second copy of it.
//!
//! The generator is the one the Computer Use pane's key card sets up — the
//! value seat's chosen row, asked down its road with the key a person put
//! there ([`LiveWriter`]); with none there is no autopilot. No subscription
//! login is ever one.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use zerocode_core::computer_use::{
    REFLEX_PLAN_DEADLINE_MS, REFLEX_PLAN_MAX_TOKENS, REFLEX_PLAN_RETRIES,
};
use zerocode_core::computer_use_protocol::game_state::{self, ColorClass, PaletteSpace};
use zerocode_core::computer_use_protocol::reflex::{
    self, Pick, ReflexPlan, Scope, VERSION, ValidatedPlan, plan_hash,
};

use super::super::errand::value::{LiveWriter, Said, Tokens};
use super::super::screenshot_png::RgbaImage;

/// The version of the words below: a plan's first-thirty-seconds label is
/// read per version, so a changed word is a new version
/// (`the_version_is_pinned_to_the_words`).
pub(crate) const PROMPT_VERSION: u32 = 1;

/// What the model is told, as the system of the one request.
pub(crate) const INSTRUCTIONS: &str = "You write the plan a live reflex run acts on: colour detectors, rules and macros that a hand runs on a person's screen, sixty times a second, for the goal they gave. Answer with one JSON object and nothing else — compact, no prose, no code fence: the plan in the contract's own words, with the keys and shapes `example` shows, written for the screen in `display`, `window` and `palette`, never for the example's. Write `version` as `contract.version`, `plan_hash` as an empty string (the window hashes the plan), and `scope` exactly as `contract.scope`. Every detector is a colour detector: its classes and its ground are colours from `palette`, its reference extent is `display`, and its ROI lies inside `window`. A rule fires its macro when its predicate over its detector's value holds, and a macro's move and click act on the target that detector follows; a detector's `pick`, one of `contract.pick`, says which blob its target follows once the one it followed is gone, and is left out for the first. Stay inside `limits`. Ids are letters, digits, `-` and `_`. When `refused` is given, the window refused the plan you wrote for those reasons: answer with the whole plan again, corrected. When `previous` is given, the run was acting on that plan and `previous.outcomes` counts how its actions ended: write a better plan for the same goal.";

/// The word for an autopilot nobody set a generator up for (§2.5, 2k): the
/// autopilot does not start, and a person's own plan (`reflex-start`) is
/// untouched.
pub(crate) const NO_GENERATOR: &str = "reflex_no_generator";

/// The contract's own example plan, whole — the golden both sides of the
/// contract read (`fixtures/reflex-contract/valid_basic.json`).
const EXAMPLE: &str =
    include_str!("../../../../zerocode-core/fixtures/reflex-contract/valid_basic.json");

/// The model that writes plans, as the autopilot asks it — the seam a test
/// replaces. Held on the thread that asks: a key store is read where it is
/// asked from.
pub(crate) trait Generator {
    /// Why nobody set it up, as the value writer's own word, or `None`.
    fn unready(&self) -> Option<&'static str>;
    /// The model its row names.
    fn model(&self) -> Option<String>;
    /// One request: the text the answer wrote and what it cost.
    ///
    /// # Errors
    ///
    /// The wire's word for why there is none.
    fn ask(&mut self, system: &str, user: &str, left: Duration) -> Result<Said, String>;
}

impl Generator for LiveWriter {
    fn unready(&self) -> Option<&'static str> {
        Self::unready(self)
    }

    fn model(&self) -> Option<String> {
        use super::super::errand::value::ValueWriter as _;
        self.row().map(|row| row.model.clone())
    }

    fn ask(&mut self, system: &str, user: &str, left: Duration) -> Result<Said, String> {
        self.ask_text(
            system,
            user,
            usize::try_from(REFLEX_PLAN_MAX_TOKENS).unwrap_or(usize::MAX),
            left,
        )
    }
}

/// A rectangle in whole units — points of the display, or pixels of a
/// capture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Rect {
    pub x: i64,
    pub y: i64,
    pub width: i64,
    pub height: i64,
}

impl Rect {
    fn json(self) -> Value {
        json!({ "x": self.x, "y": self.y, "width": self.width, "height": self.height })
    }
}

/// Where the run acts, as its detectors are written against it: the
/// display's size in points — a plan's reference extent — and the app's
/// window on it in the same points, from the display's corner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Stage {
    pub width: i64,
    pub height: i64,
    pub window: Rect,
}

/// The stage `app`'s first window stands on, on display `display`, from the
/// helper's `displays` and `listWindows` answers — and the name the plan's
/// scope carries for the app: its bundle id where it has one, the helper's
/// own name for it otherwise.
///
/// # Errors
///
/// A sentence for a display the helper does not name, an app with no window
/// on it, or a window that is not on that display.
pub(crate) fn stage_of(
    displays: &Value,
    windows: &Value,
    display: u64,
) -> Result<(Stage, String), String> {
    let number = |value: &Value, key: &str| {
        value
            .get(key)
            .and_then(Value::as_f64)
            .map(|number| number.round() as i64)
    };
    let bounds = displays
        .get("displays")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|shown| shown.get("index").and_then(Value::as_u64) == Some(display))
        .and_then(|shown| shown.get("bounds"))
        .ok_or_else(|| format!("display {display} is not one the helper shows"))?;
    let screen = Rect {
        x: number(bounds, "x").unwrap_or(0),
        y: number(bounds, "y").unwrap_or(0),
        width: number(bounds, "width").unwrap_or(0),
        height: number(bounds, "height").unwrap_or(0),
    };
    let window = windows
        .get("windows")
        .and_then(Value::as_array)
        .and_then(|windows| windows.first())
        .ok_or("the app shows no window")?;
    let (Some(x), Some(y), Some(width), Some(height)) = (
        number(window, "x"),
        number(window, "y"),
        number(window, "width"),
        number(window, "height"),
    ) else {
        return Err("the app's window has no place".into());
    };
    let (middle_x, middle_y) = (x + width / 2, y + height / 2);
    if screen.width <= 0
        || screen.height <= 0
        || !(screen.x..screen.x + screen.width).contains(&middle_x)
        || !(screen.y..screen.y + screen.height).contains(&middle_y)
    {
        return Err(format!("the app's window is not on display {display}"));
    }
    let app = &window["app"];
    let target = app
        .get("bundleId")
        .and_then(Value::as_str)
        .or_else(|| app.get("name").and_then(Value::as_str))
        .filter(|name| !name.is_empty())
        .ok_or("the helper names no app for the window")?;
    Ok((
        Stage {
            width: screen.width,
            height: screen.height,
            window: Rect {
                x: x - screen.x,
                y: y - screen.y,
                width,
                height,
            },
        },
        target.to_string(),
    ))
}

/// The colours an app's window shows, as a plan's detectors read them: the
/// largest area its ground and the next, by area, its classes — at most the
/// perception table's classes, each with the widest tolerance that keeps it
/// apart from every other colour of the palette and never wider than the
/// table allows, so the palette passes the table's own check
/// ([`game_state::check_palette`]) by construction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Palette {
    pub ground: ColorClass,
    pub classes: Vec<ColorClass>,
    /// The share of the window each covers, per thousand: the ground's
    /// first, then each class's.
    pub area_permille: Vec<u64>,
}

impl Palette {
    /// The palette as the request and the ledger name it: a spec's own
    /// words — its space, its ground, its classes — beside each one's area.
    pub(crate) fn json(&self) -> Value {
        json!({
            "space": PaletteSpace::Srgb,
            "ground": self.ground,
            "classes": self.classes,
            "areaPermille": self.area_permille,
        })
    }
}

/// The palette of the part of `image` that shows `window` of a `stage`
/// whose display the image is a capture of: every colour there counted, each
/// one a class at the table's widest tolerance could not tell from a larger
/// one merged into it, the merged colours ranked by area. `None` for a
/// window the capture does not show, or one colour alone — a ground with
/// nothing on it.
pub(crate) fn palette_of(image: &RgbaImage, stage: &Stage) -> Option<Palette> {
    let widest = u8::try_from(game_state::LIMITS.max_tolerance).unwrap_or(u8::MAX);
    let classes_at_most = usize::try_from(game_state::LIMITS.max_classes).unwrap_or(usize::MAX);
    let crop = in_pixels(image, stage)?;
    let mut counted: HashMap<[u8; 3], u64> = HashMap::new();
    for y in crop.y..crop.y + crop.height {
        for x in crop.x..crop.x + crop.width {
            let at = usize::try_from(y * i64::from(image.width) + x).ok()? * 4;
            *counted
                .entry([image.pixels[at], image.pixels[at + 1], image.pixels[at + 2]])
                .or_default() += 1;
        }
    }
    let total: u64 = counted.values().sum();
    let mut colours: Vec<([u8; 3], u64)> = counted.into_iter().collect();
    colours.sort_unstable_by(|(a, many_a), (b, many_b)| many_b.cmp(many_a).then(a.cmp(b)));
    let mut merged: Vec<([u8; 3], u64)> = Vec::new();
    for (colour, many) in colours {
        match merged
            .iter_mut()
            .find(|(kept, _)| apart_by(*kept, colour) <= widest)
        {
            Some((_, kept)) => *kept += many,
            None => merged.push((colour, many)),
        }
    }
    merged.sort_by(|(a, many_a), (b, many_b)| many_b.cmp(many_a).then(a.cmp(b)));
    merged.truncate(classes_at_most + 1);
    if merged.len() < 2 || total == 0 {
        return None;
    }
    let class = |at: usize| {
        let (colour, _) = merged[at];
        let nearest = merged
            .iter()
            .enumerate()
            .filter(|(other, _)| *other != at)
            .map(|(_, (other, _))| apart_by(colour, *other))
            .min()
            .unwrap_or(u8::MAX);
        ColorClass {
            r: colour[0],
            g: colour[1],
            b: colour[2],
            tolerance: nearest.saturating_sub(1).div_euclid(2).min(widest),
        }
    };
    let palette = Palette {
        ground: class(0),
        classes: (1..merged.len()).map(class).collect(),
        area_permille: merged
            .iter()
            .map(|(_, many)| many.saturating_mul(1_000) / total)
            .collect(),
    };
    game_state::check_palette(&palette.classes, Some(&palette.ground), &game_state::LIMITS)
        .ok()
        .map(|()| palette)
}

/// The Chebyshev distance of two colours: the farthest apart they are on
/// any one channel — the distance a class's tolerance is read in.
fn apart_by(a: [u8; 3], b: [u8; 3]) -> u8 {
    (0..3).map(|at| a[at].abs_diff(b[at])).max().unwrap_or(0)
}

/// `stage`'s window as pixels of `image`, a capture of its display: scaled
/// by the capture's width over the display's, cut to the image. `None` for a
/// window the image does not show.
fn in_pixels(image: &RgbaImage, stage: &Stage) -> Option<Rect> {
    if stage.width <= 0 {
        return None;
    }
    let scale = f64::from(image.width) / stage.width as f64;
    let at = |points: i64| (points as f64 * scale).round() as i64;
    let x = at(stage.window.x).clamp(0, i64::from(image.width));
    let y = at(stage.window.y).clamp(0, i64::from(image.height));
    let right = at(stage.window.x + stage.window.width).clamp(0, i64::from(image.width));
    let bottom = at(stage.window.y + stage.window.height).clamp(0, i64::from(image.height));
    (right > x && bottom > y).then_some(Rect {
        x,
        y,
        width: right - x,
        height: bottom - y,
    })
}

/// The run a new plan is asked in place of: the plan it acted on and how its
/// actions ended (the helper's outcome counts).
pub(crate) struct Previous<'a> {
    pub plan: &'a ReflexPlan,
    pub outcomes: &'a Value,
}

/// What one plan is asked from.
pub(crate) struct Ask<'a> {
    pub goal: &'a str,
    pub scope: &'a Scope,
    pub stage: &'a Stage,
    pub palette: &'a Palette,
    pub previous: Option<Previous<'a>>,
}

/// The one user message a plan request carries: the goal, the contract's
/// version and the scope the plan must carry, the stage and its palette, the
/// contract's tables and its example, and — as they apply — the plan being
/// replaced with its outcomes and the sentences the window refused the
/// answers before with.
pub(crate) fn user_text(ask: &Ask<'_>, refused: &[String]) -> String {
    let example = serde_json::from_str::<Value>(EXAMPLE)
        .ok()
        .and_then(|golden| golden.get("plan").cloned())
        .unwrap_or(Value::Null);
    let mut said = json!({
        "goal": ask.goal,
        "contract": { "version": VERSION, "scope": ask.scope, "pick": Pick::ALL.map(Pick::word) },
        "display": { "width": ask.stage.width, "height": ask.stage.height },
        "window": ask.stage.window.json(),
        "palette": ask.palette.json(),
        "limits": { "reflex": reflex::LIMITS, "perception": game_state::LIMITS },
        "example": example,
    });
    if let Some(previous) = &ask.previous {
        said["previous"] = json!({ "plan": previous.plan, "outcomes": previous.outcomes });
    }
    if !refused.is_empty() {
        said["refused"] = json!(refused);
    }
    said.to_string()
}

/// Why a plan an answer wrote was refused: the sentence it is asked again
/// with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Refused {
    /// Written for another scope than the run's (§2.3, 2i): asked again
    /// once, and a second one stops the run — a plan is never where the
    /// permission to act somewhere else comes from.
    Scope(String),
    /// Anything else the contract refused, in its own sentence.
    Contract(String),
}

impl Refused {
    fn sentence(&self) -> &str {
        match self {
            Self::Scope(sentence) | Self::Contract(sentence) => sentence,
        }
    }
}

/// The plan `answer` writes, read the contract's way: one JSON object in the
/// contract's words (the one pair of code fences a model wraps an answer in
/// however plainly asked not to taken off), for `scope` and no other, its
/// hash the contract's — the one field a model cannot write — and its three
/// sections read back by the contract's own reader.
///
/// # Errors
///
/// [`Refused`], in the sentence the model is asked again with.
pub(crate) fn plan_of(answer: &str, scope: &Scope) -> Result<ValidatedPlan, Refused> {
    let mut plan: ReflexPlan = serde_json::from_str(unfenced(answer.trim())).map_err(|error| {
        Refused::Contract(format!(
            "the answer is not one plan in the contract's words: {error}"
        ))
    })?;
    if plan.scope != *scope {
        return Err(Refused::Scope(format!(
            "the plan's scope is {}, and this run may act only in {}: write `scope` exactly as `contract.scope`",
            json!(plan.scope),
            json!(scope)
        )));
    }
    plan.plan_hash = plan_hash(&plan);
    reflex::read_sections(&plan.written_sections())
        .map_err(Refused::Contract)?
        .ok_or_else(|| Refused::Contract("the plan wrote no reflex sections".into()))
}

/// A text without the one pair of code fences around it, and the fence's
/// language word: ```` ```json\n{…}\n``` ```` reads as `{…}`.
fn unfenced(text: &str) -> &str {
    let Some(inner) = text
        .strip_prefix("```")
        .and_then(|rest| rest.strip_suffix("```"))
    else {
        return text;
    };
    match inner.split_once('\n') {
        Some((word, rest)) if word.trim().chars().all(char::is_alphanumeric) => rest.trim(),
        _ => inner.trim(),
    }
}

/// What asking for one plan came to: the plan, or the word for why there is
/// none, and what every request cost — the plan's ledger row.
#[derive(Debug)]
pub(crate) struct Written {
    pub plan: Result<ValidatedPlan, String>,
    pub requests: u32,
    pub bytes_out: usize,
    pub bytes_in: usize,
    pub rtt_ms: u64,
    pub tokens: Option<Tokens>,
    pub refusals: Vec<String>,
}

/// The word a plan's row names a model's plan that never passed its checks
/// by, and the one it names a second plan written for another scope by.
pub(crate) const PLAN_REFUSED: &str = "plan_refused";
pub(crate) const SCOPE_REFUSED: &str = "scope";

/// One plan asked of `generator`, and asked again with the window's refusal
/// attached while the retries last — a second plan for another scope ends
/// it at once. A request the wire could not answer is not asked again: that
/// is the network or the key, not the plan.
pub(crate) fn write_plan(generator: &mut dyn Generator, ask: &Ask<'_>) -> Written {
    let mut written = Written {
        plan: Err(PLAN_REFUSED.to_string()),
        requests: 0,
        bytes_out: 0,
        bytes_in: 0,
        rtt_ms: 0,
        tokens: None,
        refusals: Vec::new(),
    };
    let mut scope_refused = false;
    for _ in 0..=REFLEX_PLAN_RETRIES {
        let began = Instant::now();
        let said = generator.ask(
            INSTRUCTIONS,
            &user_text(ask, &written.refusals),
            Duration::from_millis(REFLEX_PLAN_DEADLINE_MS),
        );
        written.requests += 1;
        written.rtt_ms = written
            .rtt_ms
            .saturating_add(u64::try_from(began.elapsed().as_millis()).unwrap_or(u64::MAX));
        let said = match said {
            Ok(said) => said,
            Err(token) => {
                written.plan = Err(token);
                return written;
            }
        };
        written.bytes_out += said.bytes_out;
        written.bytes_in += said.bytes_in;
        if let Some(tokens) = said.tokens {
            let spent = written.tokens.get_or_insert(Tokens {
                input: 0,
                output: 0,
            });
            spent.input += tokens.input;
            spent.output += tokens.output;
        }
        match plan_of(&said.text, ask.scope) {
            Ok(plan) => {
                written.plan = Ok(plan);
                return written;
            }
            Err(refused) => {
                let again = matches!(refused, Refused::Scope(_));
                written.refusals.push(refused.sentence().to_string());
                if again && scope_refused {
                    written.plan = Err(SCOPE_REFUSED.to_string());
                    return written;
                }
                scope_refused |= again;
            }
        }
    }
    written
}

/// The ledger row one written plan leaves (§2.3): the run it started and its
/// epoch and hash (none for a plan that never ran), where it came from, the
/// goal by its fingerprint alone, the palette it was written from, the words
/// that asked it, what asking cost, every refusal on the way, and what it came
/// to.
#[allow(clippy::too_many_arguments)] // One row's columns, each from where it is known.
pub(crate) fn ledger_row(
    at: i64,
    run: Option<&str>,
    epoch: u64,
    goal: &str,
    palette: &Palette,
    model: Option<&str>,
    written: &Written,
    outcome: &str,
) -> Value {
    json!({
        "at": at,
        "run": run,
        "epoch": epoch,
        "planHash": written.plan.as_ref().ok().map(|plan| plan.plan().plan_hash.clone()),
        "source": SOURCE_MODEL,
        "goalHash": zerocode_core::jev::fingerprint_of(goal),
        "palette": palette.json(),
        "promptVersion": PROMPT_VERSION,
        "model": model,
        "requests": written.requests,
        "bytesOut": written.bytes_out,
        "bytesIn": written.bytes_in,
        "rttMs": written.rtt_ms,
        "tokens": written.tokens.map(|tokens| json!({ "input": tokens.input, "output": tokens.output })),
        "refusals": written.refusals,
        "outcome": outcome,
    })
}

/// Where a plan came from: written by the model (every plan an autopilot
/// runs), or by a person (`reflex-start`).
pub(crate) const SOURCE_MODEL: &str = "model";

#[cfg(test)]
pub(super) mod tests;
