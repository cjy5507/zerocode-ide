//! The one yes on a real phone that is the person's: the last step where money
//! leaves (검수 t-91811 P3-7, narrowed by 검수 t-93698 P2-2, 재검수 t-93881).
//!
//! A bank or a shop is the agent's to move through — its tabs, its scrolling,
//! its amount field and the 「다음」 under it. What stops is the press that
//! sends: a forward control on a sheet or screen that states a payment, a
//! transfer or an amount, a sideways drag or a slider's drag there, and `enter`
//! over it. Backing out stays open.
//!
//! What states the money is every word where the yes stands — the sheet, alert,
//! popover or dialog the control is in, or the screen's own content when none —
//! a table's rows included, since a confirm is often drawn as rows. Only a tab
//! bar and the keyboard say nothing there. A step that still asks is told by a
//! field there that takes a value: it opens a step onward (「다음」), and only
//! that — a last confirm has its memo field too (재검수 t-94000 P1-1).

use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

use super::{
    KEYBOARD_TYPE, LABEL_KEY, MARK_ROLES, SHEET_TYPES, TAB_BAR_TYPE, VALUE_KEY, carries, finishes,
    frame_of, says_onward, says_yes,
};

/// What an act on a phone is, at the tree's own points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MoneyAct {
    Press(f64, f64),
    Swipe { from: (f64, f64), to: (f64, f64) },
    Enter,
}

/// A list's row, a table's cell, a conversation's bubble: `enter` answers the
/// screen's own control, never a row's.
const CELL_TYPE: &str = "Cell";

/// What a person types into. A secret field is none: a PIN typed is the last
/// step itself, never a step that still asks.
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

/// Words a control carries when pressing it backs out, in the five languages the
/// window speaks: 「결제 취소」 leaves the payment, whatever it names. One that
/// also says yes (「취소 확인」) goes forward.
const BACK_WORDS: &[&str] = &[
    "취소",
    "닫기",
    "뒤로",
    "아니요",
    "아니오",
    "나중에",
    "cancel",
    "close",
    "go back",
    "not now",
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
    /// It is in a tab bar: pressing it moves, and its words state nothing.
    in_tab_bar: bool,
    /// It is the keyboard or in it: its keys state nothing.
    in_keyboard: bool,
    /// It is a row ([`CELL_TYPE`]) or in one.
    in_row: bool,
}

#[derive(Clone, Copy)]
struct Inherited {
    parent: Option<usize>,
    sheet: Option<usize>,
    in_tab_bar: bool,
    in_keyboard: bool,
    in_row: bool,
}

fn walk<'a>(value: &'a Value, from: Inherited, out: &mut Vec<Node<'a>>) {
    let at = out.len();
    let kind = value["type"].as_str().unwrap_or_default();
    let sheet = if SHEET_TYPES.contains(&kind) {
        Some(at)
    } else {
        from.sheet
    };
    let inherited = Inherited {
        parent: Some(at),
        sheet,
        in_tab_bar: from.in_tab_bar || kind == TAB_BAR_TYPE,
        in_keyboard: from.in_keyboard || kind == KEYBOARD_TYPE,
        in_row: from.in_row || kind == CELL_TYPE,
    };
    out.push(Node {
        value,
        kind,
        frame: frame_of(value),
        parent: from.parent,
        past: 0,
        sheet,
        in_tab_bar: inherited.in_tab_bar,
        in_keyboard: inherited.in_keyboard,
        in_row: inherited.in_row,
    });
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
                in_tab_bar: false,
                in_keyboard: false,
                in_row: false,
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

    /// What states money where `sheet`'s yes stands, if anything does: every
    /// element's words there but a tab bar's and the keyboard's, and two
    /// neighbours' read as one where they stand on one line, so 「50000」 and
    /// 「원」 drawn apart are an amount. The root is the app, and its name
    /// (「Google Pay」) states nothing.
    fn signal(&self, sheet: Option<usize>) -> Option<String> {
        let read: Vec<(usize, String)> = (1..self.nodes.len())
            .filter(|&at| {
                let node = &self.nodes[at];
                node.sheet == sheet && !node.in_tab_bar && !node.in_keyboard && self.on_screen(at)
            })
            .map(|at| (at, self.own_words(at)))
            .filter(|(_, words)| !words.trim().is_empty())
            .collect();
        read.iter()
            .find_map(|(_, words)| money_signal(words))
            .or_else(|| {
                read.windows(2)
                    .filter(|pair| self.one_line(pair[0].0, pair[1].0))
                    .find_map(|pair| money_signal(&format!("{} {}", pair[0].1, pair[1].1)))
            })
    }

    /// Whether `sheet` still asks: a field there takes a value, typed into or
    /// not. Not its focus: WebDriverAgent's `isFocused` is the focus engine's
    /// (`hasFocus`), which an iPhone may leave "0" under a raised keyboard
    /// (재검수 t-94000 P1-0).
    fn asks(&self, sheet: Option<usize>) -> bool {
        (1..self.nodes.len()).any(|at| {
            let node = &self.nodes[at];
            node.sheet == sheet
                && FIELD_TYPES.contains(&node.kind)
                && node.value["enabled"] != false
                && self.on_screen(at)
        })
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

    /// What pressing `at` says, lowercased, and whether its saying nothing makes
    /// it an icon. What is no control is read by its own words, and says nothing
    /// only as a leaf (a plain view drawn as an icon): a tap on a screen's empty
    /// place lands on a container, and the words inside it are not its own.
    fn shown(&self, at: usize) -> (String, bool) {
        let (shown, may_be_icon) = if self.pressable(at) {
            (self.words(at), true)
        } else {
            (self.own_words(at), self.nodes[at].past == at + 1)
        };
        (shown.to_lowercase(), may_be_icon)
    }

    /// The money `at`'s press would answer, if it is the person's: it goes
    /// forward — it says yes, names the money, or says nothing at all (an icon's
    /// check or arrow) — where money is stated. A tab and a press that backs out
    /// ([`BACK_WORDS`]) move. Where a field still asks ([`Self::asks`]) and
    /// `onward_opens`, a step onward and nothing else goes on: 「다음」, never
    /// 「보내기」, 「확인」, an icon or a press that names the money.
    fn refused_press(&self, at: usize, onward_opens: bool) -> Option<String> {
        let node = &self.nodes[at];
        if node.in_tab_bar {
            return None;
        }
        let (shown, may_be_icon) = self.shown(at);
        let yes = says_yes(&shown);
        let names_money = money_word(&shown).is_some();
        let backs_out = !yes && BACK_WORDS.iter().any(|word| carries(&shown, word));
        let forward = (may_be_icon && shown.trim().is_empty()) || yes || names_money;
        if backs_out || !forward {
            return None;
        }
        let said = self.signal(node.sheet)?;
        let onward = says_onward(&shown) && !finishes(&shown) && !names_money;
        (!(onward_opens && onward && self.asks(node.sheet))).then_some(said)
    }

    /// Whether a drag from `at` is a slider's: it, or what it is drawn in, is a
    /// slider or says it sends by a drag.
    fn slider(&self, at: usize) -> bool {
        if self.nodes[at].kind == SLIDER_TYPE {
            return true;
        }
        let shown = self.words(at).to_lowercase();
        let says = |words: &str| SLIDE_WORDS.iter().any(|word| carries(words, word));
        if says(&shown) {
            return true;
        }
        let mut up = self.nodes[at].parent;
        while let Some(here) = up.filter(|&here| here > 0) {
            if self.nodes[here].kind == SLIDER_TYPE || says(&self.own_words(here).to_lowercase()) {
                return true;
            }
            up = self.nodes[here].parent;
        }
        false
    }
}

/// The money an act on this screen would say yes to, when that yes is the
/// person's: a forward press where money is stated, but a step onward where a
/// field still asks; a sideways drag there from anywhere, since a slide-to-send
/// may be a bare track with its words beside it (재검수 t-94000 P2-1), and a drag
/// that starts on a slider in any direction; and `enter` while the screen's own
/// forward control stands on one, whatever field takes it. Scrolling, a tab, a
/// back-out, a digit, and everything on a screen that states no money are the
/// agent's.
#[must_use]
pub fn money_refusal(tree: &Value, act: MoneyAct) -> Option<String> {
    let screen = Screen::new(tree)?;
    match act {
        MoneyAct::Press(x, y) => screen.refused_press(screen.target((x, y)), true),
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
            .filter(|&at| {
                let node = &screen.nodes[at];
                screen.pressable(at) && !node.in_row && !node.in_keyboard && screen.on_screen(at)
            })
            .find_map(|at| screen.refused_press(at, false)),
    }
}

#[cfg(test)]
mod tests;
