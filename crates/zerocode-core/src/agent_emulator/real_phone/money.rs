//! A screen where money may leave is the person's on a real phone (검수 t-91811
//! P3-7; t-93450 r5, the person's call on 2026-11-04: money is held back until
//! they widen it).
//!
//! A screen states money when anything on it — a row, a cell, two pieces on one
//! line, a sheet or a plain view — shows an amount's shape or a payment or
//! transfer word ([`money_signal`]). There, only what cannot send is the agent's:
//! a vertical scroll ([`SCROLL_SHARE`], [`SCROLL_STEEPNESS`]), a press that backs
//! out ([`BACK_LABELS`], the navigation bar's back button), typing into a field
//! (the `text` verb, which never comes here) and the home button. Every other
//! press, a short or still drag, a sideways or slanted one, and `enter` are
//! refused. Four reviews in a row found a new way past a list of words that
//! send (t-93698, t-93881, t-94000, t-94180); a list of what may happen has no
//! such hole. A screen that states no money is not touched.

use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

use super::{
    KEYBOARD_TYPE, LABEL_KEY, MARK_ROLES, NAVIGATION_BAR_TYPE, VALUE_KEY, carries, frame_of,
};

/// What an act on a phone is, at the tree's own points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MoneyAct {
    Press(f64, f64),
    Swipe { from: (f64, f64), to: (f64, f64) },
    Enter,
}

/// The least a drag on a money screen moves up or down to be a scroll, as a
/// share of the screen's height: past the 70 pt beyond its edge where a UIKit
/// control still counts a lifted finger as its press, on the smallest iPhone
/// (667 pt high, so 100 pt). A shorter or still drag is a press.
pub const SCROLL_SHARE: f64 = 0.15;

/// How much steeper than sideways a drag on a money screen runs to be a scroll:
/// its sideways move is at most a third of its vertical one, since a
/// slide-to-send follows only the sideways part (재검수 t-94180 P2-3).
pub const SCROLL_STEEPNESS: f64 = 3.0;

/// A control's whole words when pressing it backs out, in the five languages the
/// window speaks, the close icon's own name (`xmark`) among them. Every word on
/// the control must be one of these: 「결제 취소」 or 「취소 확인」 is not a back.
pub const BACK_LABELS: &[&str] = &[
    "뒤로",
    "취소",
    "닫기",
    "아니요",
    "아니오",
    "나중에",
    "back",
    "cancel",
    "close",
    "go back",
    "not now",
    "x",
    "xmark",
    "キャンセル",
    "閉じる",
    "戻る",
    "いいえ",
    "取消",
    "关闭",
    "返回",
    "cancelar",
    "cerrar",
    "volver",
];

/// The identifier WebDriverAgent answers for a navigation bar's back button,
/// whatever its label (the screen before's title).
const BACK_BUTTON_ID: &str = "BackButton";

/// The exporter's key for an element's identifier.
const ID_KEY: &str = "AXUniqueId";

/// An amount's shape: a currency sign or code before digits (「₩5,000」,
/// 「KRW 5000」), a currency word after them (「50,000원」, 「5만 원」,
/// 「10 USD」), or digits grouped by thousands.
static AMOUNT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)[₩￦$€£¥￥₹₽₺₫₱₪]\s?\d[\d,]*(?:\.\d+)?|\b(?:krw|usd|eur|jpy|won)\s?\d[\d,]*(?:\.\d+)?|\d[\d,]*(?:\.\d+)?\s?(?:만|천|억)?\s?(?:원|₩|円|元|€|달러|(?:usd|krw|eur|jpy|won|dollars?)\b)|\b\d{1,3}(?:,\d{3})+(?:\.\d+)?\b",
    )
    .expect("the amount's shape is a valid pattern")
});

/// What in a text says money moves: a payment or transfer word of
/// [`crate::guarded`], or an amount's shape. One function, so every piece of a
/// screen is read the same way.
#[must_use]
pub fn money_signal(words: &str) -> Option<String> {
    let shown = words.to_lowercase();
    [
        crate::guarded::PAYMENT_WORDS,
        crate::guarded::TRANSFER_WORDS,
    ]
    .into_iter()
    .flatten()
    .find(|word| carries(&shown, word))
    .map(|word| (*word).to_string())
    .or_else(|| AMOUNT.find(&shown).map(|found| found.as_str().to_string()))
}

/// One node of the tree in preorder.
struct Node<'a> {
    value: &'a Value,
    kind: &'a str,
    frame: [f64; 4],
    parent: Option<usize>,
    /// The first index past its subtree.
    past: usize,
    /// It is the keyboard or in it: its keys state nothing, and a 「$」 key
    /// beside a digit would read as an amount wherever a keyboard is up.
    in_keyboard: bool,
}

fn walk<'a>(value: &'a Value, parent: Option<usize>, in_keyboard: bool, out: &mut Vec<Node<'a>>) {
    let at = out.len();
    let kind = value["type"].as_str().unwrap_or_default();
    let in_keyboard = in_keyboard || kind == KEYBOARD_TYPE;
    out.push(Node {
        value,
        kind,
        frame: frame_of(value),
        parent,
        past: 0,
        in_keyboard,
    });
    for child in value["children"].as_array().into_iter().flatten() {
        walk(child, Some(at), in_keyboard, out);
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
        walk(root, None, false, &mut nodes);
        Some(Self { nodes })
    }

    /// An element's own words, label and value apart.
    fn pieces(&self, at: usize) -> impl Iterator<Item = &'a str> {
        let node = self.nodes[at].value;
        [LABEL_KEY, VALUE_KEY]
            .into_iter()
            .filter_map(move |key| node[key].as_str())
            .filter(|words| !words.trim().is_empty())
    }

    fn own_words(&self, at: usize) -> String {
        self.pieces(at).collect::<Vec<_>>().join(" ")
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

    /// Whether `a` and `b` stand on one line: each one's middle is within the
    /// other's height.
    fn one_line(&self, a: usize, b: usize) -> bool {
        let middle = |at: usize| self.nodes[at].frame[1] + self.nodes[at].frame[3] / 2.0;
        let spans = |at: usize, y: f64| {
            let [_, top, _, height] = self.nodes[at].frame;
            y >= top && y <= top + height
        };
        spans(a, middle(b)) && spans(b, middle(a))
    }

    /// What states money anywhere on the screen, if anything does: every
    /// element's words but the keyboard's, and two neighbours' read as one where
    /// they stand on one line, so 「50000」 and 「원」 drawn apart are an amount.
    /// The root is the app, and its name (「Google Pay」) states nothing.
    fn signal(&self) -> Option<String> {
        let read: Vec<(usize, String)> = (1..self.nodes.len())
            .filter(|&at| !self.nodes[at].in_keyboard && self.on_screen(at))
            .map(|at| (at, self.own_words(at)))
            .filter(|(_, words)| !words.is_empty())
            .collect();
        read.iter()
            .find_map(|(_, words)| money_signal(words))
            .or_else(|| {
                read.windows(2)
                    .filter(|pair| self.one_line(pair[0].0, pair[1].0))
                    .find_map(|pair| money_signal(&format!("{} {}", pair[0].1, pair[1].1)))
            })
    }

    fn pressable(&self, at: usize) -> bool {
        MARK_ROLES.contains(&format!("AX{}", self.nodes[at].kind).as_str())
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

    /// Whether pressing `at` backs out: a control whose every word is a back
    /// ([`BACK_LABELS`]), or the navigation bar's own back button.
    fn backs_out(&self, at: usize) -> bool {
        if !self.pressable(at) {
            return false;
        }
        if self.nodes[at].value[ID_KEY] == BACK_BUTTON_ID && self.in_navigation_bar(at) {
            return true;
        }
        let mut pieces = (at..self.nodes[at].past)
            .flat_map(|inner| self.pieces(inner))
            .map(|piece| piece.trim().to_lowercase())
            .peekable();
        pieces.peek().is_some() && pieces.all(|piece| BACK_LABELS.contains(&piece.as_str()))
    }

    fn in_navigation_bar(&self, at: usize) -> bool {
        let mut up = self.nodes[at].parent;
        while let Some(here) = up {
            if self.nodes[here].kind == NAVIGATION_BAR_TYPE {
                return true;
            }
            up = self.nodes[here].parent;
        }
        false
    }

    /// Whether a drag from `from` to `to` scrolls: it moves up or down at least
    /// [`SCROLL_SHARE`] of the screen's height, and [`SCROLL_STEEPNESS`] times
    /// more than sideways. A screen with no height scrolls nothing.
    fn scrolls(&self, from: (f64, f64), to: (f64, f64)) -> bool {
        let height = self.nodes[0].frame[3];
        let (across, down) = ((to.0 - from.0).abs(), (to.1 - from.1).abs());
        height > 0.0 && down >= height * SCROLL_SHARE && across * SCROLL_STEEPNESS <= down
    }
}

/// The money on this screen an act is refused over, when the act is not one the
/// agent may do where money is stated: every act but a vertical scroll and a
/// press that backs out. A screen that states no money refuses nothing.
#[must_use]
pub fn money_refusal(tree: &Value, act: MoneyAct) -> Option<String> {
    let screen = Screen::new(tree)?;
    let said = screen.signal()?;
    let open = match act {
        MoneyAct::Press(x, y) => screen.backs_out(screen.target((x, y))),
        MoneyAct::Swipe { from, to } => screen.scrolls(from, to),
        MoneyAct::Enter => false,
    };
    (!open).then_some(said)
}

#[cfg(test)]
mod tests;
