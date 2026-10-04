//! The picture budget: how many of the oldest pictures one request leaves out so
//! its body stays under the provider's byte ceiling.
//!
//! # Why bytes, and why pictures
//!
//! A provider refuses a request body above a byte ceiling (Anthropic: 32 MiB,
//! `max_request_bytes` in the model catalog) no matter how few tokens it holds.
//! A picture costs ~1,600 tokens and ~1 MB, so a Computer Use session — one
//! screenshot per step, the whole history re-sent every step — reaches the
//! ceiling at a tenth of its context window. Compaction cannot fix that: it
//! summarizes the old prefix and keeps a token-budgeted tail whole, and a tail
//! of 40k tokens holds 25 screenshots. The pictures are the bytes, and the
//! oldest ones are the pictures nobody is looking at.
//!
//! # What this module decides
//!
//! Given the stored history, the provider's ceiling and an optional hard cap,
//! [`shed_count`] says how many of the OLDEST pictures the wire form leaves out
//! (each becomes a short text that says a picture was there). Stored history is
//! never touched: the lowering seam (`convert_messages`) asks per picture and
//! the next, smaller request can carry the pictures again if there is room.
//!
//! # Why steps
//!
//! The provider's prompt cache reads a prefix only while every byte of it is
//! unchanged, so leaving a picture out rewrites the message it sat in and
//! re-bills everything after it as a cache write. Leaving one picture out per
//! request would do that on EVERY request. The count therefore moves in whole
//! steps of [`SHED_STEP_PERCENT`] of the ceiling and is a pure function of the
//! history, so between two steps every request lowers the older messages to the
//! same bytes and only the tail changes — and since nothing is remembered, a
//! smaller history (after a compaction) simply carries its pictures again.

use crate::{ContentBlock, ConversationMessage};

/// Share of the provider's body ceiling at which pictures start to be left out.
///
/// The estimate counts what lowering can see — stored text and the pictures'
/// base64 — and not the system prompt, the tool definitions or JSON escaping.
/// Those are small beside 32 MiB (a full 1M-token transcript of text is about
/// 4 MB), and the NEXT step's pictures arrive after the estimate; the quarter
/// left over covers both, so the exact check that follows (the client's own
/// preflight against the same ceiling) passes without a second round.
pub const SHED_START_PERCENT: u64 = 75;

/// Share of the ceiling one shedding step leaves out, oldest pictures first.
///
/// A step rewrites the history from its oldest kept picture on, which the
/// prompt cache bills as a full re-write of everything after it; the step must
/// therefore be big enough that the next one is many requests away. Half the
/// ceiling is ~13 screenshots of 1.2 MB: one rewrite per ~13 steps instead of
/// one per step, while the newest ~8–20 pictures stay in view.
pub const SHED_STEP_PERCENT: u64 = 50;

/// The newest pictures the budget never leaves out. A Computer Use step acts on
/// its latest look and compares it with the one before, so two is the least
/// that keeps the model's own work readable.
pub const KEEP_NEWEST_PICTURES: usize = 2;

/// JSON around one block that this estimate cannot see inside (type tag, ids,
/// quotes and commas) — rounded up from the measured 25–100 bytes.
const WIRE_BLOCK_OVERHEAD_BYTES: u64 = 64;

/// JSON around one picture's base64 — the type tag, the source object and the
/// media type; an empty-data block serializes to 78 bytes — rounded up for
/// longer media types and the cache marker.
const PICTURE_WIRE_OVERHEAD_BYTES: u64 = 128;

const KIB: u64 = 1024;
const MIB: u64 = KIB * KIB;

/// The two numbers the budget works from, both shares of one ceiling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PictureBudget {
    /// Bytes of body above which pictures start to be left out.
    pub trigger: u64,
    /// Bytes of the oldest pictures one step leaves out.
    pub step: u64,
}

impl PictureBudget {
    /// The budget for a provider that declares `ceiling` bytes — the same
    /// catalog value the client's preflight refuses against, never a second
    /// number.
    #[must_use]
    pub fn for_ceiling(ceiling: u64) -> Self {
        Self {
            trigger: percent_of(ceiling, SHED_START_PERCENT),
            step: percent_of(ceiling, SHED_STEP_PERCENT).max(1),
        }
    }
}

/// `percent` percent of `value`, without overflowing for any `value`.
fn percent_of(value: u64, percent: u64) -> u64 {
    value / 100 * percent + value % 100 * percent / 100
}

/// What one request may carry in pictures: the provider's budget, when it
/// declares a ceiling, and a hard cap a refusal asked for.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct PictureLimits {
    pub(crate) budget: Option<PictureBudget>,
    pub(crate) cap: Option<usize>,
}

impl PictureLimits {
    /// The limits of the provider serving `model`: the budget of its declared
    /// body ceiling, or none when the catalog declares none ("unknown", never
    /// "unlimited" — a guessed ceiling would shed pictures for nothing).
    pub(crate) fn for_model(model: &str) -> Self {
        Self {
            budget: api::max_request_bytes_for_model(model).map(PictureBudget::for_ceiling),
            cap: None,
        }
    }

    fn is_none(self) -> bool {
        self.budget.is_none() && self.cap.is_none()
    }
}

/// How many of the OLDEST of `sizes` (each picture's wire bytes, oldest first)
/// a request leaves out.
///
/// * With a `budget`: nothing while `other` + all pictures fit the trigger.
///   Past it, the oldest pictures whose running total stays within the
///   smallest whole number of steps that frees what is over — so the answer
///   only changes when the body has outgrown the last step, and by one step
///   when it does. The newest [`KEEP_NEWEST_PICTURES`] stay whatever the body.
/// * With a `cap`: all but the newest `cap`, which wins over the budget's keep.
#[must_use]
pub(crate) fn shed_count(
    sizes: &[u64],
    other: u64,
    budget: Option<PictureBudget>,
    cap: Option<usize>,
) -> usize {
    let by_budget = budget.map_or(0, |budget| shed_for_budget(sizes, other, budget));
    let by_cap = cap.map_or(0, |cap| sizes.len().saturating_sub(cap));
    by_budget.max(by_cap)
}

fn shed_for_budget(sizes: &[u64], other: u64, budget: PictureBudget) -> usize {
    let total = sizes.iter().fold(0_u64, |sum, size| sum.saturating_add(*size));
    let need = other.saturating_add(total).saturating_sub(budget.trigger);
    if need == 0 {
        return 0;
    }
    let sheddable = sizes.len().saturating_sub(KEEP_NEWEST_PICTURES);
    // A step is never zero: a hand-built budget with one would divide by it.
    let step = budget.step.max(1);
    let mut shed = 0;
    let mut left_out = 0_u64;
    let mut reach = need.div_ceil(step).saturating_mul(step);
    loop {
        while shed < sheddable && left_out.saturating_add(sizes[shed]) <= reach {
            left_out += sizes[shed];
            shed += 1;
        }
        if left_out >= need || shed == sheddable {
            return shed;
        }
        // The next picture does not fit this step: take one more whole step,
        // the least that holds it.
        reach = left_out
            .saturating_add(sizes[shed])
            .div_ceil(step)
            .saturating_mul(step);
    }
}

/// What the stored history costs on the wire, split the way the budget needs it.
struct Measured {
    /// Each picture's wire bytes, oldest first.
    sizes: Vec<u64>,
    /// Everything else, estimated from stored lengths: the text a lowering can
    /// see. Raw lengths, not lowered ones — the lowered text of the newest tool
    /// result changes shape once it is no longer the newest, and an estimate
    /// that moved with it could hand a picture back for nothing.
    other: u64,
}

fn measure(messages: &[ConversationMessage]) -> Measured {
    let mut sizes = Vec::new();
    let mut other = 0_u64;
    for block in messages.iter().flat_map(|message| &message.blocks) {
        other = other.saturating_add(WIRE_BLOCK_OVERHEAD_BYTES);
        let text_bytes = match block {
            ContentBlock::Text { text } => text.len() as u64,
            ContentBlock::ToolUse { name, input, .. } => (name.len() + input.len()) as u64,
            ContentBlock::ToolResult { output, images, .. } => {
                sizes.extend(images.iter().map(|(_, data)| picture_wire_bytes(data)));
                output.len() as u64
            }
            ContentBlock::Image { data, .. } => {
                sizes.push(picture_wire_bytes(data));
                0
            }
            ContentBlock::Thinking {
                thinking,
                signature,
            } => (thinking.len() + signature.len()) as u64,
            ContentBlock::RedactedThinking { data } => data.len() as u64,
        };
        other = other.saturating_add(text_bytes);
    }
    Measured { sizes, other }
}

fn picture_wire_bytes(data_b64: &str) -> u64 {
    (data_b64.len() as u64).saturating_add(PICTURE_WIRE_OVERHEAD_BYTES)
}

/// What the budget decided for one request, for the lowering and for anyone who
/// wants the numbers (the measurement harness, a log line).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PictureShedPlan {
    /// Pictures in the stored history.
    pub pictures: usize,
    /// How many of the oldest the wire form leaves out.
    pub left_out: usize,
    /// Estimated body bytes with every picture.
    pub body_before: u64,
    /// Estimated body bytes once the oldest `left_out` are text.
    pub body_after: u64,
}

/// Plan one request. Costs nothing when no limit applies — the common case for
/// every provider whose catalog row declares no ceiling.
pub(crate) fn plan(messages: &[ConversationMessage], limits: PictureLimits) -> PictureShedPlan {
    if limits.is_none() {
        return PictureShedPlan::default();
    }
    let Measured { sizes, other } = measure(messages);
    let left_out = shed_count(&sizes, other, limits.budget, limits.cap);
    let body = |pictures: &[u64]| {
        pictures
            .iter()
            .fold(other, |bytes, size| bytes.saturating_add(*size))
    };
    PictureShedPlan {
        pictures: sizes.len(),
        left_out,
        body_before: body(sizes.as_slice()),
        body_after: body(&sizes[left_out..]),
    }
}

/// Pictures in `messages`, counted the way [`plan`] and the lowering walk them.
pub(crate) fn count_pictures(messages: &[ConversationMessage]) -> usize {
    messages
        .iter()
        .flat_map(|message| &message.blocks)
        .map(|block| match block {
            ContentBlock::ToolResult { images, .. } => images.len(),
            ContentBlock::Image { .. } => 1,
            _ => 0,
        })
        .sum()
}

/// Where a stored picture came from, for the note that replaces it.
#[derive(Debug, Clone, Copy)]
pub(crate) enum PictureOrigin<'a> {
    /// A tool result's out-of-band picture; carries the tool's name.
    Tool(&'a str),
    /// A picture the person attached to a message.
    Attached,
}

/// Walks a history's pictures in the order [`plan`] counted them and answers,
/// for each, whether the plan leaves it out.
pub(crate) struct PictureCursor {
    left_out: usize,
    next: usize,
}

impl PictureCursor {
    pub(crate) fn new(plan: &PictureShedPlan) -> Self {
        Self {
            left_out: plan.left_out,
            next: 0,
        }
    }

    /// Step past one stored picture. `Some(note)` when it is one of the oldest
    /// the plan leaves out — the text that stands in for it on the wire — and
    /// `None` when it rides along.
    pub(crate) fn take(
        &mut self,
        media_type: &str,
        data_b64: &str,
        origin: PictureOrigin<'_>,
        message_number: usize,
    ) -> Option<String> {
        let ordinal = self.next;
        self.next += 1;
        (ordinal < self.left_out)
            .then(|| left_out_note(media_type, data_b64, origin, message_number))
    }
}

/// The text that stands in for a picture left out: THAT a picture was there,
/// what kind and how big, and which step it came from — so the model knows what
/// it lost and how to get it back. Only facts of the picture and its place in
/// the history go in, never "N steps ago": the note must lower to the same bytes
/// on every request or it would rewrite the cache prefix itself.
pub(crate) fn left_out_note(
    media_type: &str,
    data_b64: &str,
    origin: PictureOrigin<'_>,
    message_number: usize,
) -> String {
    let pixels = crate::image_guard::peek_dimensions(data_b64)
        .map_or_else(String::new, |(width, height)| format!(" {width}x{height}"));
    let size = format_size(decoded_len(data_b64));
    let (from, advice) = match origin {
        PictureOrigin::Tool(tool) => (
            format!("from the {} result", tool_label(tool)),
            "Look again if you still need it.",
        ),
        PictureOrigin::Attached => (
            "attached".to_string(),
            "Ask the user to attach it again if you still need it.",
        ),
    };
    format!(
        "[picture left out to keep the request under the provider's size limit: \
         {media_type}{pixels}, {size}, {from} at message {message_number}. {advice}]"
    )
}

fn tool_label(tool: &str) -> &str {
    if tool.is_empty() {
        "tool"
    } else {
        tool
    }
}

/// Bytes of the file a base64 payload encodes (padding ignored).
fn decoded_len(data_b64: &str) -> u64 {
    (data_b64.len() as u64 / 4) * 3
}

/// `1.2 MiB` / `640 KiB`, in integers: nothing here needs a float.
fn format_size(bytes: u64) -> String {
    if bytes >= MIB {
        let tenths = bytes.saturating_mul(10) / MIB;
        format!("{}.{} MiB", tenths / 10, tenths % 10)
    } else {
        format!("{} KiB", bytes.div_ceil(KIB))
    }
}

/// The sentence a refused request ends on when pictures cannot be cut any
/// further: which picture is the newest, how big it is on the wire, and what the
/// rest of the request weighs — so the person can tell whether a picture or the
/// text is the wall. `None` when the history holds no picture.
pub(crate) fn newest_picture_note(messages: &[ConversationMessage]) -> Option<String> {
    let (message_index, media_type, data_b64, origin) = newest_picture(messages)?;
    let Measured { sizes, other } = measure(messages);
    let pixels = crate::image_guard::peek_dimensions(data_b64)
        .map_or_else(String::new, |(width, height)| format!(" {width}x{height}"));
    let what = match origin {
        PictureOrigin::Tool(tool) => format!("the {} result", tool_label(tool)),
        PictureOrigin::Attached => "the picture attached".to_string(),
    };
    Some(format!(
        "Pictures cannot be cut further: even with only the newest one kept the request is over \
         the provider's size limit. That picture is {what} at message {number} \
         ({media_type}{pixels}, {size} on the wire); everything else in the request comes to \
         about {rest}, and {older} older picture(s) were already left out.",
        number = message_index + 1,
        size = format_size(picture_wire_bytes(data_b64)),
        rest = format_size(other),
        older = sizes.len().saturating_sub(1),
    ))
}

/// The newest picture in `messages`: its message index, media type, base64 and
/// origin.
fn newest_picture(
    messages: &[ConversationMessage],
) -> Option<(usize, &str, &str, PictureOrigin<'_>)> {
    messages.iter().enumerate().rev().find_map(|(index, message)| {
        message.blocks.iter().rev().find_map(|block| match block {
            ContentBlock::ToolResult {
                tool_name, images, ..
            } => images.last().map(|(media_type, data)| {
                (
                    index,
                    media_type.as_str(),
                    data.as_str(),
                    PictureOrigin::Tool(tool_name),
                )
            }),
            ContentBlock::Image { media_type, data } => {
                Some((index, media_type.as_str(), data.as_str(), PictureOrigin::Attached))
            }
            _ => None,
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A budget small enough to read: bodies over 100 bytes shed in steps of 50.
    const BUDGET: PictureBudget = PictureBudget {
        trigger: 100,
        step: 50,
    };

    #[test]
    fn nothing_is_left_out_while_the_body_fits_the_trigger() {
        assert_eq!(shed_count(&[10; 5], 20, Some(BUDGET), None), 0);
        assert_eq!(shed_count(&[10; 8], 20, Some(BUDGET), None), 0, "exactly the trigger fits");
    }

    #[test]
    fn the_oldest_pictures_leave_in_whole_steps() {
        // 10 pictures of 20 bytes: 100 over the trigger, so one whole step of 50
        // twice over — the oldest five (100 bytes) go.
        assert_eq!(shed_count(&[20; 10], 0, Some(BUDGET), None), 5);
    }

    #[test]
    fn the_newest_two_stay_whatever_the_body() {
        assert_eq!(shed_count(&[20; 10], 1_000_000, Some(BUDGET), None), 10 - KEEP_NEWEST_PICTURES);
        assert_eq!(shed_count(&[20; 2], 1_000_000, Some(BUDGET), None), 0);
        assert_eq!(shed_count(&[20; 1], 1_000_000, Some(BUDGET), None), 0);
        assert_eq!(shed_count(&[], 1_000_000, Some(BUDGET), None), 0);
    }

    #[test]
    fn a_picture_bigger_than_a_step_still_leaves_when_it_has_to() {
        // 90 bytes each: one picture is more than a step. Four of them over a
        // 100-byte trigger need 260 freed, but the newest two stay: only two can go.
        let shed = shed_count(&[90; 4], 0, Some(BUDGET), None);
        assert_eq!(shed, 2, "the newest two stay, so only two can go");
    }

    #[test]
    fn a_step_that_frees_too_little_reaches_for_the_next_one() {
        // 90 + 90 + 10 + 10 is 100 over the trigger. That rounds up to a reach of 100
        // bytes, which holds the oldest picture (90) and not the second (180), so only
        // 90 of the 100 needed would go: the reach grows to the step that holds the
        // second one, and both leave.
        assert_eq!(shed_count(&[90, 90, 10, 10], 0, Some(BUDGET), None), 2);
        // The same with room to spare at the end: 495 over a trigger of 1,000 is one
        // reach of 500, which holds eight pictures of 60 (480, short of 495); the
        // next reach (1,000) holds sixteen (960), so sixteen leave — not the eight
        // that would leave the body over the trigger, and not all twenty-two.
        let wide = PictureBudget {
            trigger: 1_000,
            step: 500,
        };
        assert_eq!(shed_count(&[60; 24], 55, Some(wide), None), 16);
    }

    #[test]
    fn a_hand_built_budget_with_no_step_still_counts() {
        // `PictureBudget`'s fields are public: a step of zero is read as one byte
        // instead of dividing by it. A body of 200 against a trigger of 100 needs 100
        // freed: five pictures of 20.
        let stepless = PictureBudget {
            trigger: 100,
            step: 0,
        };
        assert_eq!(shed_count(&[20; 10], 0, Some(stepless), None), 5);
    }

    #[test]
    fn the_count_only_grows_as_the_history_grows_and_changes_in_steps() {
        // Pictures of 20 bytes under a trigger of 20 pictures and a step of 10:
        // the count may only move when the body has outgrown the last step, so
        // 100 pictures cost 8 changes — one per ten — and never one per picture.
        let budget = PictureBudget {
            trigger: 400,
            step: 200,
        };
        let mut previous = 0;
        let mut changes = Vec::new();
        for pictures in 1..=100 {
            let left_out = shed_count(&vec![20; pictures], 0, Some(budget), None);
            assert!(
                left_out >= previous,
                "{pictures} pictures handed some back: {left_out} < {previous}"
            );
            if left_out != previous {
                changes.push(pictures);
            }
            previous = left_out;
        }
        assert_eq!(changes, [21, 31, 41, 51, 61, 71, 81, 91]);
    }

    #[test]
    fn a_cap_leaves_only_the_newest_n_and_beats_the_budget_keep() {
        assert_eq!(shed_count(&[20; 10], 0, None, Some(3)), 7);
        assert_eq!(shed_count(&[20; 10], 0, None, Some(1)), 9);
        assert_eq!(shed_count(&[20; 3], 0, None, Some(5)), 0, "a cap above the count leaves everything");
        // Both limits: the stricter one.
        assert_eq!(shed_count(&[20; 10], 0, Some(BUDGET), Some(1)), 9);
        assert_eq!(shed_count(&[20; 10], 0, Some(BUDGET), Some(9)), 5);
    }

    #[test]
    fn no_limit_leaves_nothing() {
        assert_eq!(shed_count(&[1_000_000; 50], 0, None, None), 0);
    }

    #[test]
    fn the_budget_is_a_share_of_the_declared_ceiling() {
        let budget = PictureBudget::for_ceiling(33_554_432);
        assert_eq!(budget.trigger, 25_165_824, "three quarters of 32 MiB");
        assert_eq!(budget.step, 16_777_216, "half of 32 MiB");
        assert_eq!(PictureBudget::for_ceiling(0).step, 1, "a step is never zero");
        assert_eq!(percent_of(u64::MAX, 75) / (u64::MAX / 100), 75, "no overflow at the top");
    }

    #[test]
    fn sizes_read_in_binary_units() {
        assert_eq!(format_size(0), "0 KiB");
        assert_eq!(format_size(1), "1 KiB");
        assert_eq!(format_size(640 * 1024), "640 KiB");
        assert_eq!(format_size(MIB), "1.0 MiB");
        assert_eq!(format_size(1_300_000), "1.2 MiB", "rounded down to a tenth");
    }

    /// A small real PNG of 1280x800, base64.
    fn png_b64() -> String {
        use base64::Engine as _;
        let mut out = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(image::RgbImage::new(1280, 800))
            .write_to(&mut out, image::ImageFormat::Png)
            .expect("encode a PNG");
        base64::engine::general_purpose::STANDARD.encode(out.into_inner())
    }

    fn looks(count: usize) -> Vec<ConversationMessage> {
        let mut messages = vec![ConversationMessage::user_text("go")];
        for index in 0..count {
            messages.push(ConversationMessage::tool_result_with_images(
                format!("toolu_{index}"),
                "Computer",
                "ok",
                false,
                vec![("image/png".to_string(), png_b64())],
            ));
        }
        messages
    }

    #[test]
    fn a_note_for_a_tool_picture_says_what_it_was_and_where_it_came_from() {
        let note = left_out_note("image/png", &png_b64(), PictureOrigin::Tool("Computer"), 12);
        assert!(
            note.starts_with("[picture left out to keep the request under the provider's size limit: image/png 1280x800, "),
            "{note}"
        );
        assert!(note.ends_with(", from the Computer result at message 12. Look again if you still need it.]"), "{note}");
        assert_eq!(
            note,
            left_out_note("image/png", &png_b64(), PictureOrigin::Tool("Computer"), 12),
            "the same picture lowers to the same bytes on every request, or the note would rewrite the cache prefix itself"
        );
    }

    #[test]
    fn a_note_for_an_attached_picture_asks_for_it_again() {
        let note = left_out_note("image/jpeg", "not-a-picture", PictureOrigin::Attached, 1);
        // No pixel size is named when none could be read: the kind, then the size.
        assert!(note.contains("image/jpeg, 1 KiB, attached at message 1."), "{note}");
        assert!(note.contains("Ask the user to attach it again"), "{note}");
    }

    #[test]
    fn pictures_are_counted_the_way_the_plan_walks_them() {
        let mut messages = looks(3);
        messages.push(ConversationMessage::user_with_images(
            "and this",
            vec![("image/png".to_string(), png_b64()); 2],
        ));
        assert_eq!(count_pictures(&messages), 5);
        assert_eq!(count_pictures(&[ConversationMessage::user_text("hi")]), 0);
    }

    #[test]
    fn a_plan_leaves_out_what_the_budget_and_the_cap_say() {
        let messages = looks(10);
        let each = picture_wire_bytes(&png_b64());
        let budget = PictureBudget {
            trigger: each * 6,
            step: each * 2,
        };
        let limits = PictureLimits {
            budget: Some(budget),
            cap: None,
        };
        let plan = plan(&messages, limits);
        assert_eq!(plan.pictures, 10);
        assert!(plan.left_out >= 4, "ten pictures against a trigger of six: {plan:?}");
        assert!(plan.left_out <= 10 - KEEP_NEWEST_PICTURES);
        assert!(plan.body_after <= budget.trigger, "{plan:?}");
        assert!(plan.body_before > budget.trigger, "{plan:?}");
        let capped = super::plan(
            &messages,
            PictureLimits {
                budget: None,
                cap: Some(1),
            },
        );
        assert_eq!(capped.left_out, 9);
        assert_eq!(
            super::plan(&messages, PictureLimits::default()),
            PictureShedPlan::default(),
            "no limit, no work"
        );
    }

    #[test]
    fn the_last_word_names_the_newest_picture_and_what_the_rest_weighs() {
        let note = newest_picture_note(&looks(3));
        assert!(note.is_some(), "the history holds pictures");
        let note = note.expect("checked above");
        assert!(note.starts_with("Pictures cannot be cut further"), "{note}");
        assert!(note.contains("the Computer result at message 4"), "the newest of three looks: {note}");
        assert!(note.contains("image/png 1280x800"), "{note}");
        assert!(note.contains("on the wire"), "{note}");
        assert!(note.contains("2 older picture(s) were already left out"), "{note}");
        assert_eq!(newest_picture_note(&[ConversationMessage::user_text("hi")]), None);
    }

    #[test]
    fn the_wire_overheads_cover_what_they_name() {
        let block = serde_json::to_string(&api::InputContentBlock::Image {
            source: api::ImageSource {
                kind: "base64".to_string(),
                media_type: "image/png".to_string(),
                data: String::new(),
            },
            cache_control: Some(api::CacheControl::ephemeral()),
        })
        .expect("serialize an empty picture block");
        assert!(
            block.len() as u64 <= PICTURE_WIRE_OVERHEAD_BYTES,
            "{} bytes of JSON around a picture exceed the {PICTURE_WIRE_OVERHEAD_BYTES} budgeted: {block}",
            block.len()
        );
    }
}
