//! t-37798 — the wire lowering leaves the OLDEST pictures out of a request whose
//! body would pass the provider's byte ceiling, and keeps the newest.
//!
//! A Computer Use session sends one screenshot per step and re-sends the whole
//! history every step. Compaction keeps a token-budgeted tail whole, and that
//! tail can hold 25 pictures — tens of megabytes, a few tokens each — so the
//! request reached the provider's 32 MiB ceiling with 80% of the context window
//! free and the turn ended on a 413. These tests pin the lever that fixes it:
//! what the lowering sends, never what the session stores.
//!
//! The synthetic pictures are a real (tiny) PNG header padded to size: the wire
//! guard reads dimensions from the header and never decodes the rest, exactly as
//! it does for a real screenshot of that many bytes.

use std::sync::OnceLock;

use api::{InputContentBlock, InputMessage, ToolResultContentBlock};
use base64::Engine as _;
use runtime::{
    convert_messages_for, plan_picture_shed, ContentBlock, ConversationMessage, PictureBudget,
    ReasoningReplay, WireTarget, KEEP_NEWEST_PICTURES,
};

/// A model whose catalog row declares the 32 MiB body ceiling.
const MODEL: &str = "claude-opus-5-5";

/// What the placeholder opens with, as the model reads it.
const LEFT_OUT: &str = "[picture left out";

/// Bytes of one synthetic screenshot (a 1280x800 PNG header padded to size).
const PICTURE_BYTES: usize = 90_000;

const MIB: usize = 1 << 20;

fn png_head() -> &'static [u8] {
    static HEAD: OnceLock<Vec<u8>> = OnceLock::new();
    HEAD.get_or_init(|| {
        let mut out = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(image::RgbImage::new(1280, 800))
            .write_to(&mut out, image::ImageFormat::Png)
            .expect("encode the PNG header");
        out.into_inner()
    })
}

/// A base64 picture of `bytes` file bytes that reads as 1280x800; `salt` makes
/// each one different.
fn picture(bytes: usize, salt: u8) -> String {
    let mut file = png_head().to_vec();
    file.resize(bytes.max(file.len()), salt);
    base64::engine::general_purpose::STANDARD.encode(file)
}

/// One Computer Use step: the model asks for a look, the tool answers with a
/// picture. Plain-text output, so lowering has no reason to reshape it.
fn step(index: usize, data: String) -> [ConversationMessage; 2] {
    let id = format!("toolu_{index:03}");
    [
        ConversationMessage::assistant(vec![ContentBlock::ToolUse {
            id: id.clone(),
            name: "Computer".to_string(),
            input: "{\"action\":\"screenshot\"}".to_string(),
        }]),
        ConversationMessage::tool_result_with_images(
            id,
            "Computer",
            format!("step {index}: screenshot taken"),
            false,
            vec![("image/png".to_string(), data)],
        ),
    ]
}

/// A Computer Use history: the person's request, then `steps` looks.
fn history(steps: usize) -> Vec<ConversationMessage> {
    let mut messages = vec![ConversationMessage::user_text(
        "Open the settings page and check the sync option.",
    )];
    for index in 0..steps {
        let salt = u8::try_from(index % 200).expect("salt fits a byte");
        messages.extend(step(index, picture(PICTURE_BYTES, salt)));
    }
    messages
}

/// Which pictures a lowered request carries, oldest first: `true` for a picture
/// on the wire, `false` for the text that stands in for one.
fn wire_pictures(messages: &[InputMessage]) -> Vec<bool> {
    messages
        .iter()
        .flat_map(|message| &message.content)
        .filter_map(|block| match block {
            InputContentBlock::ToolResult { content, .. } => Some(content),
            _ => None,
        })
        .flatten()
        .filter_map(|block| match block {
            ToolResultContentBlock::Image { .. } => Some(true),
            ToolResultContentBlock::Text { text } if text.starts_with(LEFT_OUT) => Some(false),
            _ => None,
        })
        .collect()
}

fn placeholders(messages: &[InputMessage]) -> Vec<String> {
    messages
        .iter()
        .flat_map(|message| &message.content)
        .filter_map(|block| match block {
            InputContentBlock::ToolResult { content, .. } => Some(content),
            _ => None,
        })
        .flatten()
        .filter_map(|block| match block {
            ToolResultContentBlock::Text { text } if text.starts_with(LEFT_OUT) => {
                Some(text.clone())
            }
            _ => None,
        })
        .collect()
}

fn body_bytes(messages: &[InputMessage]) -> usize {
    serde_json::to_vec(messages).expect("serialize the request").len()
}

/// A small budget a test can read: bodies over 1 MiB shed in steps of 512 KiB.
fn small_budget() -> PictureBudget {
    PictureBudget {
        trigger: MIB as u64,
        step: MIB as u64 / 2,
    }
}

fn target_with(budget: Option<PictureBudget>) -> WireTarget {
    WireTarget::for_model(MODEL).with_picture_budget(budget)
}

#[test]
fn a_history_under_the_trigger_keeps_every_picture() {
    let history = history(6);
    let lowered = convert_messages_for(&history, target_with(Some(small_budget())));
    assert_eq!(wire_pictures(&lowered), vec![true; 6]);
    assert!(body_bytes(&lowered) < MIB);
}

#[test]
fn over_the_trigger_the_oldest_pictures_become_text_and_the_newest_stay() {
    let history = history(12);
    let budget = small_budget();
    let plan = plan_picture_shed(&history, target_with(Some(budget)));
    assert_eq!(plan.pictures, 12);
    assert!(
        plan.left_out > 0,
        "twelve pictures of {PICTURE_BYTES} bytes pass a 1 MiB trigger, so some are left out: {plan:?}"
    );
    assert!(plan.left_out <= 12 - KEEP_NEWEST_PICTURES);
    assert!(plan.body_after <= budget.trigger, "{plan:?}");

    let lowered = convert_messages_for(&history, target_with(Some(budget)));
    let mut expected = vec![false; plan.left_out];
    expected.extend(vec![true; 12 - plan.left_out]);
    assert_eq!(wire_pictures(&lowered), expected, "the oldest go, in order");
    assert!(
        body_bytes(&lowered) <= usize::try_from(budget.trigger).expect("trigger fits"),
        "the request now fits the trigger: {} bytes",
        body_bytes(&lowered)
    );

    // The newest two are intact: the model is acting on them.
    let stored_newest: Vec<&String> = history
        .iter()
        .flat_map(|message| &message.blocks)
        .filter_map(|block| match block {
            ContentBlock::ToolResult { images, .. } => images.last().map(|(_, data)| data),
            _ => None,
        })
        .rev()
        .take(KEEP_NEWEST_PICTURES)
        .collect();
    let wire_newest: Vec<&String> = lowered
        .iter()
        .flat_map(|message| &message.content)
        .filter_map(|block| match block {
            InputContentBlock::ToolResult { content, .. } => Some(content),
            _ => None,
        })
        .flatten()
        .filter_map(|block| match block {
            ToolResultContentBlock::Image { source } => Some(&source.data),
            _ => None,
        })
        .rev()
        .take(KEEP_NEWEST_PICTURES)
        .collect();
    assert_eq!(wire_newest, stored_newest, "the newest pictures ride unchanged");
}

#[test]
fn a_placeholder_says_kind_size_and_step() {
    let history = history(12);
    let lowered = convert_messages_for(&history, target_with(Some(small_budget())));
    let notes = placeholders(&lowered);
    assert!(!notes.is_empty(), "something was left out");
    // The first look is the 3rd message of the history: 1 prompt, then the
    // model's call and the tool's answer.
    let first = &notes[0];
    assert!(first.contains("image/png"), "kind: {first}");
    assert!(first.contains("1280x800"), "pixels: {first}");
    assert!(first.contains("KiB") || first.contains("MiB"), "size: {first}");
    assert!(first.contains("Computer"), "which tool: {first}");
    assert!(first.contains("message 3"), "which step: {first}");
    // Every note names its own step: the notes are told apart.
    let distinct: std::collections::BTreeSet<_> = notes.iter().collect();
    assert_eq!(distinct.len(), notes.len(), "one note per picture: {notes:?}");
}

#[test]
fn stored_history_is_not_rewritten() {
    let history = history(12);
    let before = history.clone();
    let lowered = convert_messages_for(&history, target_with(Some(small_budget())));
    assert!(
        wire_pictures(&lowered).contains(&false),
        "the test only means something if pictures were left out"
    );
    assert_eq!(history, before, "lowering reads the history and writes nothing");
}

#[test]
fn a_smaller_later_history_carries_the_pictures_again() {
    let history = history(12);
    let target = target_with(Some(small_budget()));
    let crowded = convert_messages_for(&history, target);
    assert!(wire_pictures(&crowded).contains(&false), "twelve do not fit");
    // The same session after a compaction kept only its first three looks: the
    // pictures it still holds all go out, because nothing is remembered.
    let smaller = convert_messages_for(&history[..7], target);
    assert_eq!(wire_pictures(&smaller), vec![true; 3]);
}

#[test]
fn a_cap_leaves_only_the_newest_n_whatever_the_body() {
    let history = history(8);
    // No ceiling declared at all: only the cap speaks.
    let target = WireTarget::from(ReasoningReplay::Native);
    let two = convert_messages_for(&history, target.with_picture_cap(Some(2)));
    let mut expected = vec![false; 6];
    expected.extend([true, true]);
    assert_eq!(wire_pictures(&two), expected);
    let just_newest = convert_messages_for(&history, target.with_picture_cap(Some(1)));
    let mut expected = vec![false; 7];
    expected.push(true);
    assert_eq!(wire_pictures(&just_newest), expected);
    let uncapped = convert_messages_for(&history, target.with_picture_cap(None));
    assert_eq!(wire_pictures(&uncapped), vec![true; 8], "no cap, no ceiling: everything");
}

#[test]
fn an_undeclared_ceiling_leaves_every_picture_alone() {
    // "Unknown" is not "unlimited", but it is not a reason to drop pictures
    // either: with no declared number there is no budget to hold to.
    assert!(api::max_request_bytes_for_model("some-unlisted-local-model").is_none());
    let history = history(40);
    let lowered = convert_messages_for(&history, WireTarget::for_model("some-unlisted-local-model"));
    assert_eq!(wire_pictures(&lowered), vec![true; 40]);
}

#[test]
fn pictures_the_person_attached_follow_the_same_rule() {
    let mut messages = Vec::new();
    for index in 0..12 {
        let salt = u8::try_from(index).expect("salt fits a byte");
        messages.push(ConversationMessage::user_with_images(
            format!("here is picture {index}"),
            vec![("image/png".to_string(), picture(PICTURE_BYTES, salt))],
        ));
        messages.push(ConversationMessage::assistant(vec![ContentBlock::Text {
            text: "noted".to_string(),
        }]));
    }
    let lowered = convert_messages_for(&messages, target_with(Some(small_budget())));
    let (mut left_out, mut kept) = (Vec::new(), 0);
    for block in lowered.iter().flat_map(|message| &message.content) {
        match block {
            InputContentBlock::Text { text, .. } if text.starts_with(LEFT_OUT) => {
                left_out.push(text.clone());
            }
            InputContentBlock::Image { .. } => kept += 1,
            _ => {}
        }
    }
    assert!(!left_out.is_empty(), "twelve attached pictures pass 1 MiB");
    assert_eq!(left_out.len() + kept, 12);
    assert!(kept >= KEEP_NEWEST_PICTURES);
    assert!(left_out[0].contains("attached at message 1."), "{}", left_out[0]);
    assert!(left_out[0].contains("attach it again"), "{}", left_out[0]);
}

/// How many requests, of a session that grows one look at a time, rewrote the
/// history behind its newest message — the requests that cost the provider's
/// prompt cache the whole prefix. A pure append (the previous request, then one
/// more look) rewrites nothing.
fn rewriting_requests(steps: usize, budget: PictureBudget) -> usize {
    let full = history(steps);
    let target = target_with(Some(budget));
    let mut previous: Option<Vec<InputMessage>> = None;
    let mut rewrites = 0;
    for looks in 1..=steps {
        let lowered = convert_messages_for(&full[..1 + 2 * looks], target);
        if let Some(before) = previous.as_ref() {
            // The previous request's own tail may reshape once it is no longer
            // the newest (lowering treats the newest tool result differently);
            // anything older that changed is a rewrite of the prefix.
            let stable = before.len().saturating_sub(1);
            rewrites += usize::from(before[..stable] != lowered[..stable]);
        }
        previous = Some(lowered);
    }
    rewrites
}

#[test]
fn pictures_leave_in_steps_so_the_prefix_holds_between_them() {
    let steps = 60;
    // 17 pictures fit the trigger; a step is ~8.7 pictures.
    let stepped = rewriting_requests(
        steps,
        PictureBudget {
            trigger: 2 * MIB as u64,
            step: MIB as u64,
        },
    );
    // One byte per step: the budget leaves out exactly what the body is over,
    // so every request after the first overflow is a different request.
    let naive = rewriting_requests(
        steps,
        PictureBudget {
            trigger: 2 * MIB as u64,
            step: 1,
        },
    );
    assert!(
        (3..=7).contains(&stepped),
        "about one rewrite per ~9 looks over {steps} looks, not one per look: {stepped}"
    );
    assert!(
        naive >= 30,
        "leaving out one picture at a time rewrites the prefix on almost every request: {naive}"
    );
}

#[test]
fn the_budget_is_the_share_of_the_ceiling_the_catalog_declares() {
    // The number the client's preflight refuses against — never a second one.
    let ceiling = api::max_request_bytes_for_model(MODEL)
        .expect("the catalog declares a body ceiling for this model");
    // 17 pictures of 2 MiB on the wire (1.5 MiB of file each): the case the
    // person hit — more than 32 MiB of screenshots with a handful of tokens'
    // worth of text.
    let mut messages = vec![ConversationMessage::user_text("keep clicking")];
    for index in 0..17 {
        let salt = u8::try_from(index).expect("salt fits a byte");
        messages.extend(step(index, picture(MIB * 3 / 2, salt)));
    }
    let plan = plan_picture_shed(&messages, WireTarget::for_model(MODEL));
    assert_eq!(plan.pictures, 17);
    assert!(
        plan.body_before > ceiling,
        "the case is over the ceiling to begin with: {plan:?} vs {ceiling}"
    );
    assert!(plan.left_out > 0, "{plan:?}");
    assert!(
        plan.body_after <= ceiling / 100 * runtime::SHED_START_PERCENT,
        "after leaving the oldest out the body is under the share of the ceiling: {plan:?}"
    );
    assert!(plan.pictures - plan.left_out >= KEEP_NEWEST_PICTURES, "{plan:?}");
}

#[test]
fn the_plan_counts_the_pictures_the_lowering_walks() {
    // Tool-result pictures and attached ones, in one history: the plan and the
    // lowering must number them the same way or the wrong ones would go.
    let mut messages = vec![ConversationMessage::user_with_images(
        "start here",
        vec![("image/png".to_string(), picture(PICTURE_BYTES, 1))],
    )];
    for index in 0..5 {
        messages.extend(step(index, picture(PICTURE_BYTES, 2)));
    }
    let target = WireTarget::from(ReasoningReplay::Native).with_picture_cap(Some(3));
    let plan = plan_picture_shed(&messages, target);
    assert_eq!(plan.pictures, 6);
    assert_eq!(plan.left_out, 3);
    let lowered = convert_messages_for(&messages, target);
    let kept_in_tool_results = wire_pictures(&lowered).iter().filter(|kept| **kept).count();
    let left_out_in_tool_results = placeholders(&lowered).len();
    let attached_left_out = lowered
        .iter()
        .flat_map(|message| &message.content)
        .filter(|block| matches!(block, InputContentBlock::Text { text, .. } if text.starts_with(LEFT_OUT)))
        .count();
    assert_eq!(kept_in_tool_results, 3, "the newest three are tool pictures");
    assert_eq!(left_out_in_tool_results + attached_left_out, 3);
}
