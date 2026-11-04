//! The one yes on a real phone that is the person's: the last step where money
//! leaves (검수 t-91811 P3-7, narrowed by 검수 t-93698 P2-2).
//!
//! A bank or a shop is the agent's to move through — its tabs, its lists, its
//! amount field, its 「다음」. What stops is the press that sends: a forward
//! control on a sheet or screen that states a payment, a transfer or an amount,
//! a slider's swipe there, and `enter` over it. Backing out stays open.
//!
//! What states the money is read where the yes stands: the sheet, alert, popover
//! or dialog the control is in, or the screen's own content when none. A tab
//! bar, a list's or a conversation's rows, the keyboard and a field's own text
//! say nothing here: a bank's 「이체」 tab, a 「결제 완료됐어요」 bubble, a
//! 「구매하기」 row and a typed 「5만원 보내줘」 are no sheet asking to send.

use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

use super::{
    KEYBOARD_TYPE, LABEL_KEY, MARK_ROLES, SHEET_TYPES, TAB_BAR_TYPE, VALUE_KEY, carries, frame_of,
    is_secure_type, says_yes,
};

/// What an act on a phone is, at the tree's own points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MoneyAct {
    Press(f64, f64),
    Swipe { from: (f64, f64), to: (f64, f64) },
    Enter,
}

/// A list's row, a table's cell, a conversation's bubble: its words describe one
/// entry, never the sheet a yes stands in.
const CELL_TYPE: &str = "Cell";

/// What a person types into: its text is the typing, not what a sheet states.
const FIELD_TYPES: [&str; 3] = ["TextField", "SearchField", "TextView"];

/// A control that sends by a drag.
const SLIDER_TYPE: &str = "Slider";

/// Words a control that sends by a drag carries when its type does not say so.
const SLIDE_WORDS: &[&str] = &[
    "밀어서",
    "slide to",
    "swipe to",
    "スライド",
    "滑动",
    "desliza",
];

/// An amount's shape: a currency sign before digits, a currency word after them
/// (「50,000원」, 「5만 원」, 「10 USD」), or digits grouped by thousands.
static AMOUNT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)[₩￦$€£¥￥]\s?\d[\d,]*(?:\.\d+)?|\d[\d,]*(?:\.\d+)?\s?(?:만|천|억)?\s?(?:원|₩|円|元|€|달러|(?:usd|krw|eur|jpy|won|dollars?)\b)|\b\d{1,3}(?:,\d{3})+(?:\.\d+)?\b",
    )
    .expect("the amount's shape is a valid pattern")
});

/// What in a text says money moves: a payment or transfer word of
/// [`crate::guarded`], or an amount's shape. One function, so a sheet's words and
/// a control's are read the same way.
#[must_use]
pub fn money_signal(words: &str) -> Option<String> {
    let shown = words.to_lowercase();
    if let Some(word) = money_word(&shown) {
        return Some(word.to_string());
    }
    AMOUNT.find(&shown).map(|found| found.as_str().to_string())
}

fn money_word(shown: &str) -> Option<&'static str> {
    [
        crate::guarded::PAYMENT_WORDS,
        crate::guarded::TRANSFER_WORDS,
    ]
    .into_iter()
    .flatten()
    .find(|word| carries(shown, word))
    .copied()
}

/// One node of the tree in preorder, with what the guard asks of it.
struct Node<'a> {
    value: &'a Value,
    kind: &'a str,
    frame: [f64; 4],
    parent: Option<usize>,
    /// The first index past its subtree.
    past: usize,
    /// Its nearest sheet ([`SHEET_TYPES`]), itself included: where its yes
    /// stands. `None` is the screen's own content.
    sheet: Option<usize>,
    /// Its words state nothing ([`CELL_TYPE`], a field, the keyboard, a tab bar).
    quiet: bool,
    /// It is in a tab bar: pressing it moves.
    in_tab_bar: bool,
}

#[derive(Clone, Copy)]
struct Inherited {
    parent: Option<usize>,
    sheet: Option<usize>,
    quiet: bool,
    in_tab_bar: bool,
}

fn walk<'a>(value: &'a Value, from: Inherited, out: &mut Vec<Node<'a>>) {
    let at = out.len();
    let kind = value["type"].as_str().unwrap_or_default();
    let in_tab_bar = from.in_tab_bar || kind == TAB_BAR_TYPE;
    let quiet = from.quiet
        || in_tab_bar
        || kind == CELL_TYPE
        || kind == KEYBOARD_TYPE
        || FIELD_TYPES.contains(&kind)
        || is_secure_type(kind);
    let sheet = if SHEET_TYPES.contains(&kind) {
        Some(at)
    } else {
        from.sheet
    };
    out.push(Node {
        value,
        kind,
        frame: frame_of(value),
        parent: from.parent,
        past: 0,
        sheet,
        quiet,
        in_tab_bar,
    });
    let inherited = Inherited {
        parent: Some(at),
        sheet,
        quiet,
        in_tab_bar,
    };
    for child in value["children"].as_array().into_iter().flatten() {
        walk(child, inherited, out);
    }
    out[at].past = out.len();
}

/// The tree as the guard reads it: [`super::tree_from_source`]'s shape.
struct Screen<'a> {
    nodes: Vec<Node<'a>>,
}

impl<'a> Screen<'a> {
    fn new(tree: &'a Value) -> Option<Self> {
        let root = tree
            .as_array()
            .and_then(|roots| roots.first())
            .unwrap_or(tree);
        root.is_object().then_some(())?;
        let mut nodes = Vec::new();
        walk(
            root,
            Inherited {
                parent: None,
                sheet: None,
                quiet: false,
                in_tab_bar: false,
            },
            &mut nodes,
        );
        Some(Self { nodes })
    }

    fn own_words(&self, at: usize) -> String {
        let node = self.nodes[at].value;
        [LABEL_KEY, VALUE_KEY]
            .into_iter()
            .filter_map(|key| node[key].as_str())
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// A control's words: its own and those of what is inside it.
    fn words(&self, at: usize) -> String {
        (at..self.nodes[at].past)
            .map(|inner| self.own_words(inner))
            .filter(|words| !words.trim().is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn on_screen(&self, at: usize) -> bool {
        let [x, y, w, h] = self.nodes[at].frame;
        let [sx, sy, sw, sh] = self.nodes[0].frame;
        w > 0.0 && h > 0.0 && x < sx + sw && y < sy + sh && x + w > sx && y + h > sy
    }

    fn contains(&self, at: usize, (px, py): (f64, f64)) -> bool {
        let [x, y, w, h] = self.nodes[at].frame;
        w > 0.0 && h > 0.0 && px >= x && py >= y && px <= x + w && py <= y + h
    }

    /// What states money where `sheet`'s yes stands, if anything does. The root
    /// is the app, and its name (「Google Pay」) states nothing.
    fn signal(&self, sheet: Option<usize>) -> Option<String> {
        (1..self.nodes.len())
            .filter(|&at| {
                let node = &self.nodes[at];
                node.sheet == sheet && !node.quiet && self.on_screen(at)
            })
            .find_map(|at| money_signal(&self.own_words(at)))
    }

    fn pressable(&self, at: usize) -> bool {
        let kind = self.nodes[at].kind;
        !FIELD_TYPES.contains(&kind) && MARK_ROLES.contains(&format!("AX{kind}").as_str())
    }

    /// What a finger at `point` lands on: the last-drawn element there (the last
    /// in preorder holding the point, so the innermost of the topmost branch), or
    /// the control that holds it.
    fn target(&self, point: (f64, f64)) -> usize {
        let drawn = (0..self.nodes.len())
            .rev()
            .find(|&at| self.contains(at, point))
            .unwrap_or(0);
        let mut at = Some(drawn);
        while let Some(here) = at {
            if self.pressable(here) {
                return here;
            }
            at = self.nodes[here].parent;
        }
        drawn
    }

    /// Whether pressing `at` goes forward: it says yes, names the money, or says
    /// nothing at all (an icon's check or arrow). 「취소」, 「닫기」, 「뒤로」 say
    /// none of that and stay open; 「결제 취소」 names the money and is the person's.
    fn forward(&self, at: usize) -> bool {
        let shown = self.words(at).to_lowercase();
        shown.trim().is_empty() || says_yes(&shown) || money_word(&shown).is_some()
    }

    fn slider(&self, at: usize) -> bool {
        let shown = self.words(at).to_lowercase();
        self.nodes[at].kind == SLIDER_TYPE || SLIDE_WORDS.iter().any(|word| carries(&shown, word))
    }

    /// The money `at`'s yes would answer, if it goes forward where money is stated.
    fn refused_press(&self, at: usize) -> Option<String> {
        let node = &self.nodes[at];
        if node.in_tab_bar || !self.forward(at) {
            return None;
        }
        self.signal(node.sheet)
    }
}

/// The money an act on this screen would say yes to, when that yes is the
/// person's: a forward press on a sheet or screen that states money, a swipe that
/// starts on a slider there or runs sideways there, and `enter` while any forward
/// control stands on one. A scroll, a tab, a back-out, a list's row, a digit, and
/// everything on a screen that states no money are the agent's.
#[must_use]
pub fn money_refusal(tree: &Value, act: MoneyAct) -> Option<String> {
    let screen = Screen::new(tree)?;
    match act {
        MoneyAct::Press(x, y) => screen.refused_press(screen.target((x, y))),
        MoneyAct::Swipe { from, to } => {
            let at = screen.target(from);
            let node = &screen.nodes[at];
            let sideways = (to.0 - from.0).abs() > (to.1 - from.1).abs();
            if node.in_tab_bar || !(sideways || screen.slider(at)) {
                return None;
            }
            screen.signal(node.sheet)
        }
        MoneyAct::Enter => (0..screen.nodes.len())
            .filter(|&at| screen.pressable(at) && !screen.nodes[at].quiet && screen.on_screen(at))
            .find_map(|at| screen.refused_press(at)),
    }
}

#[cfg(test)]
mod tests;
