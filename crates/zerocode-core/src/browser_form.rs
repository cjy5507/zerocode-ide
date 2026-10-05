//! The browser door's form pair (t-37883): `fields` reads every field a page
//! draws in one look — the words it is known by, its kind, what it holds,
//! its choices, whether it must be filled, and a handle that names it — and
//! `fill` takes a bundle of handle → value and writes them all in one call,
//! reading each field back to say whether it took.
//!
//! Why: on 2026-10-04 another agent booked a parking space on a site it had
//! never seen in under five minutes, where ours took about an hour of
//! pictures — a look and a press per field, ~17 s a model round trip. A
//! form is data and the door already holds the page's DOM, so a page's
//! fields are read as data and written as data, and the round trips are the
//! form's steps, not its fields.
//!
//! Nothing here knows a site. Every handle, word and choice is read from the
//! page; the tables below name the platform's own vocabulary (HTML's
//! controls, ARIA's roles) and the door's limits. The page scripts that walk
//! by them are the window's (`crates/zerocode-shell/src/cmd/browser.rs`);
//! this module is their pure half: the tables, the bundle's grammar, the
//! ledger of a fill's passes and the words the agent reads.

use serde::de::{Deserializer, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Serialize};

/// The elements a page's fields are, as CSS — HTML's controls, an editable
/// region, and the ARIA widgets a page draws its own checkboxes, switches,
/// radio groups and dropdowns with, and a button that says it opens a list
/// (`aria-haspopup="listbox"`: a select drawn by hand). One table: a new kind
/// of field is a row.
pub const BROWSER_FORM_CONTROLS: &[&str] = &[
    "input",
    "select",
    "textarea",
    "[contenteditable]",
    "[role=textbox]",
    "[role=checkbox]",
    "[role=switch]",
    "[role=radiogroup]",
    "[role=combobox]",
    "[aria-haspopup=listbox]",
    "[tabindex]",
];

/// The input types that are no field a person fills: what a form carries
/// unseen, and its buttons (those are [`BROWSER_FORM_ACTIONS`]).
pub const BROWSER_FORM_NOT_FIELDS: &[&str] = &["hidden", "submit", "button", "reset", "image"];

/// The controls a form is sent or moved on by — its submit, a "send the
/// code" beside a phone number, a step's "next" — named in the read beside
/// the fields, so the press after a fill needs no look of its own.
pub const BROWSER_FORM_ACTIONS: &[&str] = &[
    "button",
    "input[type=submit]",
    "input[type=button]",
    "[role=button]",
];

/// The boxes a form's buttons stand in with its fields — the form, a dialog,
/// the page's main region — so a read names the buttons that send or move
/// the form on, not the site's navigation.
pub const BROWSER_FORM_SCOPES: &[&str] =
    &["form", "[role=form]", "dialog", "[role=dialog]", "main"];

/// How many boxes out from a field its caption is looked for, when nothing
/// names the field (no label, no ARIA name, no table header): the words
/// just before it in its own box, its parent's, its grandparent's — a
/// caption a page writes beside a field sits that close.
pub const BROWSER_FORM_CAPTION_DEPTH: usize = 3;

/// How deep a read goes into frames inside frames.
pub const BROWSER_FORM_FRAME_DEPTH: usize = 3;

/// The regions a page announces its changes in without a press — ARIA's alert
/// and status roles and its live regions: the text that is new in one of them
/// after a write or a press is the page speaking, wherever it stands.
pub const BROWSER_FORM_LIVE: &[&str] = &[
    "[role=alert]",
    "[role=status]",
    "[aria-live=assertive]",
    "[aria-live=polite]",
];

/// At most this many pieces of new text are said beside one field, and as
/// notices of the form.
pub const BROWSER_FORM_FRESH_CAP: usize = 3;

/// The most pieces of text one field's box is read for, before a write or a
/// press and after it.
pub const BROWSER_FORM_PIECE_CAP: usize = 60;

/// The lists a dropdown drawn with no ARIA keeps beside the focusable box
/// that opens it — a field of its own kind (`dropdown`), its items the
/// choices.
pub const BROWSER_FORM_LISTS: &[&str] = &["ul", "ol", "[role=listbox]"];

/// The items of such a list.
pub const BROWSER_FORM_LIST_ITEMS: &[&str] = &["li", "[role=option]"];

/// What a person can press or focus that no other row names: an element
/// that takes focus or a click of its own, or one of ARIA's pressable roles
/// no field or button stands for. These, and a box that shows a pointer its
/// parent does not, are what a read says as `unknown` rather than drops.
pub const BROWSER_FORM_PRESSABLES: &[&str] = &[
    "[tabindex]:not([tabindex^='-'])",
    "[onclick]",
    "[role=tab]",
    "[role=menuitem]",
    "[role=treeitem]",
    "[role=slider]",
    "[role=spinbutton]",
    "[role=option]",
];

/// The most elements a read looks through for pressable things of no kind
/// — a long form has a few thousand.
pub const BROWSER_FORM_SCAN_CAP: usize = 5_000;

/// The choices a dropdown the page draws itself offers, once it is open
/// (ARIA's option role).
pub const BROWSER_FORM_OPTIONS: &[&str] = &["[role=option]"];

/// The elements a date picker draws its days with — a grid's cells and the
/// buttons inside them — searched only inside what a press on a date field
/// opened, for the one whose words carry the asked date's numbers.
pub const BROWSER_FORM_DAYS: &[&str] = &["[role=gridcell]", "td", "button", "[data-date]"];

/// The attributes a day of a date picker says its whole date in, besides its
/// words — ARIA's name, a title, and the data a picker keys its days by.
pub const BROWSER_FORM_DAY_WORDS: &[&str] =
    &["aria-label", "title", "data-date", "data-value", "datetime"];

/// The fewest day cells a box holds to be a month's calendar (February's).
pub const BROWSER_FORM_MONTH_DAYS: usize = 28;

/// The most months a fill pages a calendar to reach a date — two years; a
/// date further off is the agent's to reach by hand.
pub const BROWSER_FORM_MONTH_PAGES: usize = 24;

/// What separates a frame's handle from the handle inside it. Not CSS (a
/// selector with it is refused by the platform), so a handle with it can
/// only be read as a path into a frame.
pub const BROWSER_FORM_FRAME_SEPARATOR: &str = " >> ";

/// The words HTML itself says a checkbox's state in, besides JSON's `true`
/// and `false`: a bundle may give a checkbox either.
pub const BROWSER_FILL_ON: &[&str] = &["true", "on"];
pub const BROWSER_FILL_OFF: &[&str] = &["false", "off"];

/// At most this many fields in one read — a long application form has about
/// fifty (the long-form bench's 43); past it the read says how many more.
pub const BROWSER_FORM_FIELD_CAP: usize = 200;
/// At most this many actions in one read.
pub const BROWSER_FORM_ACTION_CAP: usize = 40;
/// A field's choices shown in a read: a country list has two hundred, and the
/// agent names a choice by its words, which `fill` matches against all of them.
pub const BROWSER_FORM_OPTION_CAP: usize = 40;
/// The longest value one field of a bundle takes — a message box's worth.
pub const BROWSER_FILL_VALUE_CAP: usize = 10_000;
/// How long a fill keeps trying the fields an earlier value brings — the
/// times a chosen date loads, a field a choice enables, a dropdown that
/// opens on a press — before it says which never came. Inside the door's
/// budget ([`crate::agent_browser::BROWSER_DOOR_BUDGET_MS`]) with a page
/// call to spare.
pub const BROWSER_FILL_PENDING_MS: u64 = 2_000;
/// At most this many passes over a bundle: a value the page keeps rewriting
/// is said, not chased.
pub const BROWSER_FILL_PASSES: usize = 6;

/// One field as the page read it. The page writes these names
/// (`BROWSER_FORM_HELPERS`); every one may be missing, and a missing one
/// reads as nothing.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FormField {
    /// The handle `fill` (and `click`, outside a frame) names it by.
    pub handle: String,
    /// HTML's input type, `select`, `textarea`, `text` for an editable
    /// region, `checkbox`, `radio`, `combobox`.
    pub kind: String,
    pub label: String,
    /// The heading of the region the field stands in.
    pub section: String,
    pub value: FormValue,
    pub options: Vec<String>,
    /// Choices past [`BROWSER_FORM_OPTION_CAP`].
    pub more_options: usize,
    pub required: bool,
    pub disabled: bool,
    pub read_only: bool,
    /// The field holds a secret: its value is never read.
    pub masked: bool,
    pub placeholder: String,
    pub max_length: Option<u32>,
    /// The page's own words about what is wrong with the value it holds.
    pub error: String,
    /// What the page says about a field that is off or cannot be written —
    /// its `aria-describedby`, its title, the words beside its label in the
    /// nearest box that holds no other field — so the agent learns what
    /// switches it on; and, for a group of radios, the sentence between its
    /// title and the group. Empty for a field that is on and has no note.
    pub hint: String,
    /// Whether the list a button opens is open, when the page says
    /// (`aria-expanded`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub open: Option<bool>,
    /// The number in the field's name tells it from fields of the same name
    /// and nothing more: no words of the page set them apart.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub ordinal: bool,
    /// The text that stands new in the box around the field since before a
    /// write or a press (read after the page settled) — said as the page wrote
    /// it, never as an error: the door cannot tell one.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub fresh: Vec<String>,
    /// The page marked the field invalid (`aria-invalid`) and, after a write or a
    /// press, said nothing about why: no linked words, none new beside it,
    /// no notice.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub silent: bool,
}

/// A button beside the fields.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FormAction {
    pub handle: String,
    pub label: String,
    pub disabled: bool,
}

/// One `fields` read.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FormRead {
    pub fields: Vec<FormField>,
    pub actions: Vec<FormAction>,
    /// Fields past [`BROWSER_FORM_FIELD_CAP`].
    pub more: usize,
    /// Frames whose page this one may not read (another origin) — said, so
    /// the agent knows a part of the form is not in the read.
    pub sealed_frames: Vec<String>,
    /// The form's fingerprint: a digest of every field's handle, kind and
    /// words — never a value — that `fill` holds the page to before it
    /// writes (a field gone, renamed or new, another step: `form_stale`).
    pub fingerprint: String,
    /// What a person can press or focus beside the fields that the read
    /// named no kind for — said, so nothing pressable drops out of the read.
    pub unknowns: Vec<FormUnknown>,
    /// The boxes the page lets scroll that have more to read below what is
    /// shown — said, so the agent knows what to read to its end when a field
    /// stays off.
    pub scroll_boxes: Vec<FormScrollBox>,
}

/// A scrollable box not yet read to its end: its handle and its first words.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FormScrollBox {
    pub handle: String,
    pub label: String,
}

/// A pressable thing the read could not name a kind for: its handle, its own
/// words and the caption beside it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FormUnknown {
    pub handle: String,
    pub label: String,
    pub caption: String,
}

/// What a field holds, or is asked to: words, or a checkbox's state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum FormValue {
    Flag(bool),
    Text(String),
}

impl Default for FormValue {
    fn default() -> Self {
        Self::Text(String::new())
    }
}

impl FormValue {
    fn is_empty(&self) -> bool {
        matches!(self, Self::Text(text) if text.is_empty()) || *self == Self::Flag(false)
    }

    fn said(&self) -> String {
        match self {
            Self::Flag(flag) => flag.to_string(),
            Self::Text(text) => format!("\"{text}\""),
        }
    }
}

/// One handle of a bundle and the value it is to hold.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FillEntry {
    pub handle: String,
    pub value: FormValue,
}

/// A bundle in the order it was written: a JSON object of handle → value
/// (its keys in written order, whatever the map type underneath keeps), or
/// an array of `{handle, value}`.
struct Bundle(Vec<(String, serde_json::Value)>);

impl<'de> Deserialize<'de> for Bundle {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Reader;
        impl<'de> Visitor<'de> for Reader {
            type Value = Bundle;
            fn expecting(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                out.write_str("an object of handle → value, or a list of {handle, value}")
            }
            fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Bundle, M::Error> {
                let mut pairs = Vec::new();
                while let Some(pair) = map.next_entry::<String, serde_json::Value>()? {
                    pairs.push(pair);
                }
                Ok(Bundle(pairs))
            }
            fn visit_seq<S: SeqAccess<'de>>(self, mut seq: S) -> Result<Bundle, S::Error> {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Pair {
                    handle: String,
                    value: serde_json::Value,
                }
                let mut pairs = Vec::new();
                while let Some(pair) = seq.next_element::<Pair>()? {
                    pairs.push((pair.handle, pair.value));
                }
                Ok(Bundle(pairs))
            }
        }
        deserializer.deserialize_any(Reader)
    }
}

/// Read a `fill` bundle: handles in the order written, each once, at most
/// [`BROWSER_FORM_FIELD_CAP`] of them; a value is words, a number (written
/// as its words) or a checkbox's `true`/`false`.
pub fn fill_entries(said: &str) -> Result<Vec<FillEntry>, String> {
    let Bundle(pairs) = serde_json::from_str(said).map_err(|_| {
        "fill 값은 JSON입니다 — {\"손잡이\": 값, …} 또는 [{\"handle\": …, \"value\": …}]"
            .to_string()
    })?;
    if pairs.is_empty() {
        return Err("fill 묶음이 비어 있습니다".to_string());
    }
    if pairs.len() > BROWSER_FORM_FIELD_CAP {
        return Err(format!(
            "fill 한 번에 {BROWSER_FORM_FIELD_CAP}칸까지입니다 ({}칸)",
            pairs.len()
        ));
    }
    let mut entries: Vec<FillEntry> = Vec::with_capacity(pairs.len());
    for (handle, value) in pairs {
        let handle = handle.trim().to_string();
        if handle.is_empty() {
            return Err("fill 손잡이가 비어 있습니다".to_string());
        }
        if entries.iter().any(|entry| entry.handle == handle) {
            return Err(format!("fill 손잡이 `{handle}`가 두 번 나옵니다"));
        }
        let value = match value {
            serde_json::Value::String(text) => FormValue::Text(text),
            serde_json::Value::Bool(flag) => FormValue::Flag(flag),
            serde_json::Value::Number(number) => FormValue::Text(number.to_string()),
            _ => return Err(format!("`{handle}`의 값은 글·수·true/false여야 합니다")),
        };
        if matches!(&value, FormValue::Text(text) if text.chars().count() > BROWSER_FILL_VALUE_CAP)
        {
            return Err(format!(
                "`{handle}`의 값이 {BROWSER_FILL_VALUE_CAP}자를 넘습니다"
            ));
        }
        entries.push(FillEntry { handle, value });
    }
    Ok(entries)
}

/// How one field of a bundle went, as the page says it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FillStatus {
    /// Written, and reads back as asked.
    Set,
    /// Already held what was asked; nothing written.
    Same,
    /// Written, and reads back as something else.
    Mismatch,
    /// The handle names nothing (yet).
    NotFound,
    /// No choice reads as the value (yet) — a dropdown not open, times not
    /// loaded.
    NoOption,
    /// The field is off (yet).
    Disabled,
    /// The page keeps it from being written, and nothing it opened took the value.
    ReadOnly,
    /// A password field: `fill` never writes a secret.
    Secret,
    /// A file field: the person's turn.
    File,
    /// Longer than the field takes.
    TooLong,
    /// The handle names something that is no field.
    NotAField,
    /// The handle is no selector the page can read.
    InvalidHandle,
    /// The write took the page to another document: what it held is gone, and
    /// what stands now is the next read's.
    Replaced,
    /// The page said nothing this door knows about the field.
    #[default]
    #[serde(other)]
    Unread,
}

impl FillStatus {
    /// The field holds the value.
    #[must_use]
    pub fn took(self) -> bool {
        matches!(self, Self::Set | Self::Same)
    }

    /// Another pass may still find it: the page brings fields, choices and
    /// switches in after an earlier value, and rewrites one a later value
    /// cleared.
    #[must_use]
    pub fn tries_again(self) -> bool {
        matches!(
            self,
            Self::Mismatch | Self::NotFound | Self::NoOption | Self::Disabled
        )
    }

    fn said(self) -> &'static str {
        match self {
            Self::Set | Self::Same => "들어감",
            Self::Mismatch => "다시 읽으니 다른 값",
            Self::NotFound => "손잡이가 가리키는 칸이 없음",
            Self::NoOption => "그 값의 선택지가 없음",
            Self::Disabled => "꺼져 있음",
            Self::ReadOnly => "페이지가 직접 쓰지 못하게 막음 — click으로 열어 고를 것",
            Self::Secret => "비밀 칸은 fill이 쓰지 않음 — type <label> <손잡이> --value-stdin",
            Self::File => "파일 칸은 사람의 차례",
            Self::TooLong => "칸이 받는 길이를 넘음",
            Self::NotAField => "칸이 아님",
            Self::InvalidHandle => "손잡이를 CSS로 읽을 수 없음",
            Self::Replaced => "쓴 뒤 페이지가 다른 문서로 바뀜 — fields로 다시 읽기",
            Self::Unread => "판이 답하지 않음",
        }
    }
}

/// One field's outcome in a pass.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FillResult {
    pub handle: String,
    pub status: FillStatus,
    pub kind: String,
    pub label: String,
    /// What the field holds after the pass (a secret's: nothing).
    pub now: FormValue,
    pub error: String,
    /// The choices there were, when none read as the value.
    pub options: Vec<String>,
    /// The page's own calendar, when a date could not be picked on it.
    pub widget: Option<FormWidget>,
    /// What the page says about the field, when it was off.
    pub hint: String,
    /// The text new beside the field after the write, as it is on a field of a read.
    pub fresh: Vec<String>,
    /// The page marked the field invalid and said nothing why.
    pub silent: bool,
}

/// A calendar the fill could not pick a day on, as the page shows it: its
/// heading's words and month, the controls that may page it, the box of its
/// days — what the agent presses to finish by hand.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FormWidget {
    pub heading: String,
    /// `YYYY-MM`, when the heading read as one.
    pub month: String,
    pub pagers: Vec<String>,
    pub days: String,
}

/// One pass of a fill, as the page answers it: each asked field's outcome,
/// and the fields that still want something once the pass is done — empty
/// and required, or holding a value the page calls wrong.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FillPass {
    pub results: Vec<FillResult>,
    pub left: Vec<FormField>,
    /// The form was not the one the agent read: nothing was written.
    pub stale: bool,
    /// The form's fingerprint once the pass was done (before it, when stale).
    pub fingerprint: String,
    /// The buttons the form has once the pass was done, as a `fields` read
    /// names them — on or off, so the agent presses the step's button with no
    /// read between.
    pub actions: Vec<FormAction>,
    /// Whether the page was still changing when the pass read it: `Some(true)`
    /// when the settle that followed its writes ran out of time, `Some(false)`
    /// when the page stood still first, `None` when the pass wrote nothing and
    /// waited for nothing.
    pub moving: Option<bool>,
    /// The text new in the page's live regions (alert, status) that stands
    /// beside no field — a notice the write brought.
    pub alerts: Vec<String>,
}

/// A fill's passes, kept: each entry's latest outcome, and the page's word
/// on what is left and on the buttons after the last pass.
#[derive(Debug, Clone)]
pub struct FillLedger {
    entries: Vec<FillEntry>,
    results: Vec<Option<FillResult>>,
    left: Vec<FormField>,
    actions: Vec<FormAction>,
    alerts: Vec<String>,
    moving: bool,
    settled: bool,
    passes: usize,
    stale: bool,
    fingerprint: String,
}

impl FillLedger {
    #[must_use]
    pub fn new(entries: Vec<FillEntry>) -> Self {
        let results = vec![None; entries.len()];
        Self {
            entries,
            results,
            left: Vec::new(),
            actions: Vec::new(),
            alerts: Vec::new(),
            moving: false,
            settled: false,
            passes: 0,
            stale: false,
            fingerprint: String::new(),
        }
    }

    /// The entries the next pass writes, in the bundle's order: those not
    /// yet tried and those another pass may still find — none once
    /// [`BROWSER_FILL_PASSES`] passes are spent.
    #[must_use]
    pub fn next(&self) -> Vec<FillEntry> {
        if self.stale || self.passes >= BROWSER_FILL_PASSES {
            return Vec::new();
        }
        self.entries
            .iter()
            .zip(&self.results)
            .filter(|(_, result)| result.as_ref().is_none_or(|said| said.status.tries_again()))
            .map(|(entry, _)| entry.clone())
            .collect()
    }

    /// One pass's answer, matched to the entries by handle; an entry the
    /// pass was asked for and did not answer reads `unread`. A stale pass
    /// wrote nothing and ends the fill.
    pub fn record(&mut self, asked: &[FillEntry], pass: FillPass) {
        self.passes += 1;
        self.fingerprint = pass.fingerprint;
        if pass.stale {
            self.stale = true;
            return;
        }
        for entry in asked {
            let Some(at) = self
                .entries
                .iter()
                .position(|held| held.handle == entry.handle)
            else {
                continue;
            };
            let said = pass
                .results
                .iter()
                .find(|result| result.handle == entry.handle)
                .cloned()
                .unwrap_or_else(|| FillResult {
                    handle: entry.handle.clone(),
                    ..FillResult::default()
                });
            self.results[at] = Some(said);
        }
        self.left = pass.left;
        self.actions = pass.actions;
        // A pass that wrote nothing heard no settle: the last word stands.
        if let Some(moving) = pass.moving {
            self.moving = moving;
            self.settled = true;
        }
    }

    #[must_use]
    pub fn report(self) -> FillReport {
        let results = self
            .entries
            .into_iter()
            .zip(self.results)
            .map(|(entry, result)| {
                result.unwrap_or(FillResult {
                    handle: entry.handle,
                    ..FillResult::default()
                })
            })
            .collect();
        FillReport {
            results,
            left: self.left,
            actions: self.actions,
            alerts: self.alerts,
            moving: self.moving,
            settled: self.settled,
            passes: self.passes,
            stale: self.stale,
            fingerprint: self.fingerprint,
            ..FillReport::default()
        }
    }
}

/// A whole fill: each entry's last outcome in the bundle's order, what the
/// page says is left, the buttons after it, and how many passes it took.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FillReport {
    pub results: Vec<FillResult>,
    pub left: Vec<FormField>,
    /// The buttons the form has after the last pass.
    pub actions: Vec<FormAction>,
    /// The notices the page's live regions made after the last pass that waited for it.
    pub alerts: Vec<String>,
    /// The page was still changing when the last pass that waited for it read
    /// it: the settle ran out of time (or the page went to another document),
    /// so nothing here is said to be final.
    pub moving: bool,
    /// A pass of the fill waited for the page (it wrote something and heard a
    /// settle), so what the answer says is the page as it stood still then.
    pub settled: bool,
    pub passes: usize,
    /// The form was not the one the agent read: nothing was written.
    pub stale: bool,
    /// The form's fingerprint after the last pass.
    pub fingerprint: String,
    /// Whether the form is no longer the one the agent read — `None` when it
    /// read none, so there is nothing to be the same as ([`Self::against`]).
    pub changed: Option<bool>,
    /// The buttons the agent's read had that the form no longer draws, said
    /// only when the form is otherwise the same: none now has their handle or
    /// their words (a button found by its place moves when a banner is added
    /// above it, and is the same button).
    pub hidden: Vec<FormAction>,
}

impl FillReport {
    /// The report set against the form the agent read before the fill — its
    /// fingerprint, and the buttons that read had — which is what the last
    /// line of the answer says: the same form or another, and a button the
    /// page took away. A form that changed has lost every button of the old
    /// one, so none is said as hidden then. The buttons are matched one to one,
    /// by their words first and then, among those left, by their handle (a
    /// button found by its place moves when a banner is added above it and is
    /// the same button): a button of the read that no button now matches is the
    /// one the page took away.
    #[must_use]
    pub fn against(mut self, known: Option<&str>, buttons: &[FormAction]) -> Self {
        self.changed = known.map(|print| print != self.fingerprint);
        self.hidden = Vec::new();
        if self.changed == Some(false) {
            let mut now: Vec<&FormAction> = self.actions.iter().collect();
            let mut unmatched = Vec::new();
            for before in buttons {
                match now.iter().position(|one| one.label == before.label) {
                    Some(at) => {
                        now.remove(at);
                    }
                    None => unmatched.push(before),
                }
            }
            for before in unmatched {
                match now.iter().position(|one| one.handle == before.handle) {
                    Some(at) => {
                        now.remove(at);
                    }
                    None => self.hidden.push(before.clone()),
                }
            }
        }
        self
    }

    #[must_use]
    pub fn took(&self) -> usize {
        self.results
            .iter()
            .filter(|result| result.status.took())
            .count()
    }

    #[must_use]
    pub fn all_took(&self) -> bool {
        !self.stale && self.took() == self.results.len()
    }
}

/// What the page says of itself once a press by selector has been made and
/// the page has settled: the form's fingerprint, its buttons, the fields
/// that carry something to say (an error, a value still wanted, text new beside
/// them) and the notices its live regions made.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PressRead {
    pub fingerprint: String,
    pub actions: Vec<FormAction>,
    pub noted: Vec<FormField>,
    pub alerts: Vec<String>,
}

/// A press's read set against the form the agent knew before it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PressAfter {
    pub read: PressRead,
    /// Whether the form is no longer the one the agent read.
    pub changed: Option<bool>,
    /// The buttons the agent's read had that the form no longer draws.
    pub hidden: Vec<FormAction>,
    /// The page was still changing when it was read.
    pub moving: bool,
    /// The page was waited for before it was read.
    pub settled: bool,
}

impl PressAfter {
    /// The read set against the form the agent knew (its fingerprint and
    /// buttons).
    #[must_use]
    pub fn against(
        read: PressRead,
        known: Option<&str>,
        buttons: &[FormAction],
        moving: bool,
    ) -> Self {
        let _ = (known, buttons);
        Self {
            read,
            moving,
            ..Self::default()
        }
    }
}

/// The words after a press the agent made in a form it had read.
#[must_use]
pub fn press_lines(_after: &PressAfter) -> String {
    String::new()
}

/// The choices a line shows, and how many more there are.
fn choices(options: &[String], more: usize) -> String {
    if options.is_empty() {
        return String::new();
    }
    let more = if more > 0 {
        format!(" (+{more})")
    } else {
        String::new()
    };
    format!(" ▸ {}{more}", options.join(" | "))
}

/// A field as one line: handle, kind, words, what it holds and what it takes.
fn field_line(field: &FormField) -> String {
    let mut line = format!("  {} · {} · {}", field.handle, field.kind, field.label);
    if field.required {
        line.push_str(" *");
    }
    if field.masked {
        line.push_str(" = (가림)");
    } else {
        line.push_str(&format!(" = {}", field.value.said()));
    }
    match field.open {
        Some(true) => line.push_str(" (열림)"),
        Some(false) => line.push_str(" (닫힘)"),
        None => {}
    }
    if field.ordinal {
        line.push_str(" (같은 이름이라 번호만 붙임)");
    }
    if !field.placeholder.is_empty() && field.placeholder != field.label {
        line.push_str(&format!(" 예: {}", field.placeholder));
    }
    if let Some(cap) = field.max_length {
        line.push_str(&format!(" 최대 {cap}자"));
    }
    if field.disabled {
        line.push_str(" (꺼짐)");
    }
    if field.read_only {
        line.push_str(" (직접 못 씀)");
    }
    if !field.hint.is_empty() {
        line.push_str(&format!(" — 안내: {}", field.hint));
    }
    if !field.error.is_empty() {
        line.push_str(&format!(" ⚠ {}", field.error));
    }
    line.push_str(&choices(&field.options, field.more_options));
    line
}

/// A `fields` read for the agent: a count, the fields under their sections in
/// the page's order, the buttons, and what the read could not reach.
#[must_use]
pub fn fields_lines(read: &FormRead) -> String {
    let required = read.fields.iter().filter(|field| field.required).count();
    // A secret's value is never read: it is no empty field, only one unread.
    let empty = read
        .fields
        .iter()
        .filter(|field| field.required && !field.masked && field.value.is_empty())
        .count();
    let unread = read
        .fields
        .iter()
        .filter(|field| field.required && field.masked)
        .count();
    let unread = if unread > 0 {
        format!(", 값을 읽지 않는 필수 {unread}")
    } else {
        String::new()
    };
    let mut lines = vec![format!(
        "양식 칸 {}개 (필수 {required}, 비어 있는 필수 {empty}{unread})",
        read.fields.len()
    )];
    let mut section: Option<&str> = None;
    for field in &read.fields {
        if section != Some(field.section.as_str()) {
            section = Some(field.section.as_str());
            if !field.section.is_empty() {
                lines.push(format!("[{}]", field.section));
            }
        }
        lines.push(field_line(field));
    }
    if read.more > 0 {
        lines.push(format!("(+{}칸 더 — 이 읽기에 다 싣지 못함)", read.more));
    }
    if !read.actions.is_empty() {
        let buttons: Vec<String> = read
            .actions
            .iter()
            .map(|action| {
                let off = if action.disabled { " (꺼짐)" } else { "" };
                format!("{} 「{}」{off}", action.handle, action.label)
            })
            .collect();
        lines.push(format!("버튼: {}", buttons.join(" · ")));
    }
    if !read.unknowns.is_empty() {
        let things: Vec<String> = read
            .unknowns
            .iter()
            .map(|thing| {
                let caption = if thing.caption.is_empty() || thing.caption == thing.label {
                    String::new()
                } else {
                    format!(" ({})", thing.caption)
                };
                format!("{} 「{}」{caption}", thing.handle, thing.label)
            })
            .collect();
        lines.push(format!(
            "종류를 모르는 조작: {} — click 뒤 fields로 다시 읽기",
            things.join(" · ")
        ));
    }
    if !read.scroll_boxes.is_empty() {
        let boxes: Vec<String> = read
            .scroll_boxes
            .iter()
            .map(|one| format!("{} 「{}」", one.handle, one.label))
            .collect();
        lines.push(format!(
            "끝까지 안 내린 스크롤 상자: {} — 꺼진 칸이 있으면 끝까지 내려 보고 fields로 다시 읽기",
            boxes.join(" · ")
        ));
    }
    if !read.sealed_frames.is_empty() {
        lines.push(format!(
            "읽지 못한 다른 출처의 틀: {}",
            read.sealed_frames.join(" · ")
        ));
    }
    lines.join("\n") + "\n"
}

/// A `fields --json` answer: the read itself, flagged as the page's words.
#[must_use]
pub fn fields_json(read: &FormRead) -> serde_json::Value {
    let mut answer = serde_json::to_value(read).unwrap_or_else(|_| serde_json::json!({}));
    answer[crate::untrusted::JSON_FLAG] = serde_json::Value::Bool(true);
    answer
}

/// A calendar the agent finishes by hand: its heading, its pagers, its days.
fn widget_words(widget: &FormWidget) -> String {
    let pagers = if widget.pagers.is_empty() {
        "—".to_string()
    } else {
        widget.pagers.join(" · ")
    };
    format!(
        " — 달력 「{}」 넘김 {pagers} · 날짜 칸 {} (열고, 넘기고, 날짜를 click)",
        widget.heading, widget.days
    )
}

/// What is left of a form after a fill, one field to a line.
fn left_line(field: &FormField) -> String {
    let why = if !field.error.is_empty() {
        field.error.clone()
    } else if field.masked {
        "값을 읽지 않음".to_string()
    } else {
        "필수, 비어 있음".to_string()
    };
    format!(
        "  {} · {} · {} — {why}",
        field.handle, field.kind, field.label
    )
}

/// The last line of a fill: whether the form is the one the agent read, the
/// buttons it has now — on or off — and the ones the page took away. Nothing
/// when the agent read no form and there is no button to say.
fn after_line(report: &FillReport) -> Option<String> {
    // A page still changing is no "same form" and no settled button: it says so first.
    let form = match report.changed {
        _ if report.moving => Some("아직 바뀌는 중(fields로 다시 읽기)"),
        Some(true) => Some("양식 바뀜(fields로 다시 읽기)"),
        Some(false) => Some("양식 그대로"),
        None => None,
    };
    let mut parts = Vec::new();
    if !report.actions.is_empty() {
        let buttons: Vec<String> = report
            .actions
            .iter()
            .map(|action| {
                let state = if action.disabled { "꺼짐" } else { "켜짐" };
                format!("{} 「{}」 {state}", action.handle, action.label)
            })
            .collect();
        parts.push(format!("버튼: {}", buttons.join(", ")));
    }
    if !report.hidden.is_empty() {
        let gone: Vec<String> = report
            .hidden
            .iter()
            .map(|action| format!("{} 「{}」", action.handle, action.label))
            .collect();
        parts.push(format!("숨김: {}", gone.join(", ")));
    }
    match (form, parts.is_empty()) {
        (None, true) => None,
        (None, false) => Some(parts.join("; ")),
        (Some(form), true) => Some(form.to_string()),
        (Some(form), false) => Some(format!("{form} — {}", parts.join("; "))),
    }
}

/// What a read made once the page stood still cannot see, said at the end of a
/// fill that waited for the page and found it still: an error a quiet timer
/// shows later is not in the answer, so it is not the page's last word.
fn late_note() -> String {
    format!(
        "※ 쓴 뒤 {} ms 동안 가만히 있는 것을 보고 읽었습니다. 그 뒤에 뜨는 오류는 못 봅니다 — 제출 전에 fields로 다시 읽으세요",
        crate::agent_browser::BROWSER_SETTLE_QUIET_MS
    )
}

/// A fill for the agent: how many took in how many passes, every field's
/// line — what it holds now, or why it did not take — what is left, the state
/// of the form's buttons after it, and what a read after the page stood still
/// cannot see.
#[must_use]
pub fn fill_lines(report: &FillReport) -> String {
    if report.stale {
        return format!(
            "{}: 양식이 읽은 뒤 바뀌었습니다 (칸이 사라지거나 바뀌었거나 새 단계) — 아무 칸도 쓰지 않았습니다; fields로 다시 읽고 채우세요\n",
            crate::computer_use_protocol::error_code::FORM_STALE
        );
    }
    let mut lines = vec![format!(
        "채움 {}/{}칸 ({}회)",
        report.took(),
        report.results.len(),
        report.passes
    )];
    for result in &report.results {
        let mark = if result.status.took() { "✓" } else { "✗" };
        let mut line = format!("  {mark} {} {}", result.handle, result.label);
        if result.status.took() {
            line.push_str(&format!(" = {}", result.now.said()));
        } else {
            line.push_str(&format!(": {}", result.status.said()));
            if !result.hint.is_empty() {
                line.push_str(&format!(" — 안내: {}", result.hint));
            }
            if result.status == FillStatus::Mismatch {
                line.push_str(&format!(" ({})", result.now.said()));
            }
            line.push_str(&choices(&result.options, 0));
            if let Some(widget) = &result.widget {
                line.push_str(&widget_words(widget));
            }
        }
        if !result.error.is_empty() {
            line.push_str(&format!(" ⚠ {}", result.error));
        }
        lines.push(line);
    }
    if !report.left.is_empty() {
        lines.push("남은 칸:".to_string());
        lines.extend(report.left.iter().map(left_line));
    }
    lines.extend(after_line(report));
    // A page still changing already says to read again.
    if report.settled && !report.moving {
        lines.push(late_note());
    }
    lines.join("\n") + "\n"
}

#[cfg(test)]
mod tests;
