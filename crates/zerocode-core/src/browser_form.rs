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

use serde::{Deserialize, Serialize};

/// The elements a page's fields are, as CSS — HTML's controls, an editable
/// region, and the ARIA widgets a page draws its own checkboxes, switches,
/// radio groups and dropdowns with. One table: a new kind of field is a row.
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
pub const BROWSER_FORM_SCOPES: &[&str] = &["form", "[role=form]", "dialog", "[role=dialog]", "main"];

/// How many boxes out from a field its caption is looked for, when nothing
/// names the field (no label, no ARIA name, no table header): the words
/// just before it in its own box, its parent's, its grandparent's — a
/// caption a page writes beside a field sits that close.
pub const BROWSER_FORM_CAPTION_DEPTH: usize = 3;

/// How deep a read goes into frames inside frames.
pub const BROWSER_FORM_FRAME_DEPTH: usize = 3;

/// The choices a dropdown the page draws itself offers, once it is open
/// (ARIA's option role).
pub const BROWSER_FORM_OPTIONS: &[&str] = &["[role=option]"];

/// The elements a date picker draws its days with — a grid's cells and the
/// buttons inside them — searched only inside what a press on a date field
/// opened, for the one whose words carry the asked date's numbers.
pub const BROWSER_FORM_DAYS: &[&str] = &["[role=gridcell]", "td", "button", "[data-date]"];

/// The attributes a day of a date picker says its whole date in, besides its
/// words — ARIA's name, a title, and the data a picker keys its days by.
pub const BROWSER_FORM_DAY_WORDS: &[&str] = &["aria-label", "title", "data-date", "data-value"];

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


/// One handle of a bundle and the value it is to hold.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FillEntry {
    pub handle: String,
    pub value: FormValue,
}


/// Read a `fill` bundle: handles in the order written, each once, at most
/// [`BROWSER_FORM_FIELD_CAP`] of them; a value is words, a number (written
/// as its words) or a checkbox's `true`/`false`.
pub fn fill_entries(said: &str) -> Result<Vec<FillEntry>, String> {
    let _ = said;
    Err("fill은 아직 없습니다 (빨강)".to_string())
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
    /// The page said nothing this door knows about the field.
    #[default]
    #[serde(other)]
    Unread,
}

impl FillStatus {
    /// The field holds the value.
    #[must_use]
    pub fn took(self) -> bool {
        let _ = self;
        false
    }

    /// Another pass may still find it: the page brings fields, choices and
    /// switches in after an earlier value, and rewrites one a later value
    /// cleared.
    #[must_use]
    pub fn tries_again(self) -> bool {
        let _ = self;
        false
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
}

/// One pass of a fill, as the page answers it: each asked field's outcome,
/// and the fields that still want something once the pass is done — empty
/// and required, or holding a value the page calls wrong.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FillPass {
    pub results: Vec<FillResult>,
    pub left: Vec<FormField>,
}

/// A fill's passes, kept: each entry's latest outcome, and the page's word
/// on what is left after the last pass.
#[derive(Debug, Clone)]
pub struct FillLedger {
    entries: Vec<FillEntry>,
    results: Vec<Option<FillResult>>,
    left: Vec<FormField>,
    passes: usize,
}

impl FillLedger {
    #[must_use]
    pub fn new(entries: Vec<FillEntry>) -> Self {
        let results = vec![None; entries.len()];
        Self {
            entries,
            results,
            left: Vec::new(),
            passes: 0,
        }
    }

    /// The entries the next pass writes, in the bundle's order: those not
    /// yet tried and those another pass may still find — none once
    /// [`BROWSER_FILL_PASSES`] passes are spent.
    #[must_use]
    pub fn next(&self) -> Vec<FillEntry> {
        let _ = self;
        Vec::new()
    }

    /// One pass's answer, matched to the entries by handle; an entry the
    /// pass was asked for and did not answer reads `unread`.
    pub fn record(&mut self, asked: &[FillEntry], pass: FillPass) {
        let _ = (asked, pass);
    }

    #[must_use]
    pub fn report(self) -> FillReport {
        let _ = self;
        FillReport::default()
    }
}

/// A whole fill: each entry's last outcome in the bundle's order, what the
/// page says is left, and how many passes it took.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FillReport {
    pub results: Vec<FillResult>,
    pub left: Vec<FormField>,
    pub passes: usize,
}

impl FillReport {
    #[must_use]
    pub fn took(&self) -> usize {
        let _ = self;
        0
    }

    #[must_use]
    pub fn all_took(&self) -> bool {
        let _ = self;
        false
    }
}



/// A `fields` read for the agent: a count, the fields under their sections in
/// the page's order, the buttons, and what the read could not reach.
#[must_use]
pub fn fields_lines(read: &FormRead) -> String {
    let _ = read;
    String::new()
}

/// A `fields --json` answer: the read itself, flagged as the page's words.
#[must_use]
pub fn fields_json(read: &FormRead) -> serde_json::Value {
    let _ = read;
    serde_json::json!({})
}


/// A fill for the agent: how many took in how many passes, every field's
/// line — what it holds now, or why it did not take — and what is left.
#[must_use]
pub fn fill_lines(report: &FillReport) -> String {
    let _ = report;
    String::new()
}

#[cfg(test)]
mod tests;
