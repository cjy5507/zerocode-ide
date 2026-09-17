//! The terminal grid: a VT state machine with no process attached.
//!
//! Split from [`crate::lane`] on purpose. The grid is where correctness lives,
//! and keeping it free of a child process means every escape sequence we claim
//! to support is proven by feeding bytes to a plain function and asserting
//! cells — no PTY, no timing, no flakiness.
//!
//! ## What is supported
//!
//! Deliberately a subset, and **the tests define it** (ADR 0002). Printing,
//! `CR`/`LF`/`BS`/`TAB`, cursor motion (`CUU`/`CUD`/`CUF`/`CUB`/`CUP`), erase
//! (`ED`/`EL`), graphic rendition (`SGR` — weight, underline, inverse, the
//! 16-colour set, 256-colour indices and truecolor), window title (`OSC 0`/
//! `OSC 2`), bounded clipboard writes (`OSC 52`), bracketed paste, the
//! alternate screen with save/restore
//! (`DECSET`/`DECRST` `?2004` / `?1049`), plus a bounded scrollback.
//!
//! Anything else is **swallowed**. An unsupported sequence must never reach the
//! screen as stray glyphs — that is the failure mode that makes a terminal feel
//! broken, and it is worse than not implementing the sequence at all.

use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::num::NonZeroU32;
use std::sync::Arc;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
use vte::{Params, Parser, Perform};

use crate::colors::{
    Rgb, TerminalColors, format_x_color_rgb_spec, parse_x_color_spec, reads_as_dark,
};
use crate::readers::FrameReaders;

/// Scrollback lines kept per lane.
///
/// The full-width ceiling is not "a few MB": at the measured 95 columns a
/// row requests `95 * 44` bytes and occupies a 5,120-byte macOS allocation,
/// so 5,000 rows are 25.1 MiB per lane (201 MiB for eight) and 50,000 rows are
/// 246 MiB per lane. Compact history below removes each default tail, but a
/// fully painted transcript still reaches that ceiling. Measurement and the
/// arithmetic live in `docs/measurements/pty-footprint-verdict-20260831.md`
/// sections 2.3 and 3.3.
///
/// 5000 is also Orca's default, arrived at independently and then confirmed:
/// `DESKTOP_TERMINAL_SCROLLBACK_ROWS_DEFAULT` (store-BgJxB0hr.js:3419).
pub const DEFAULT_SCROLLBACK_LINES: usize = 5_000;

/// What a person may choose, and Orca's own range (`store:3419`). Both ends
/// are enforced HERE rather than only in the settings row: the value reaches
/// this grid from a JSON file on disk, and a file anyone can edit is not a
/// control this crate may trust.
pub const MIN_SCROLLBACK_LINES: usize = 1_000;
pub const MAX_SCROLLBACK_LINES: usize = 50_000;

/// Largest OSC 52 payload accepted before decoding, matching Orca 1.4.180.
///
/// The limit is on the bytes the child actually sent, including whitespace,
/// so adding ignorable characters cannot turn this into an allocation bypass.
const MAX_OSC52_BASE64_CHARS: usize = 128 * 1024;

/// How many distinct explicit hyperlinks one terminal keeps.
///
/// A ceiling rather than a growing table: a program that prints a fresh
/// address per line — a test runner naming every case — would otherwise turn
/// a scrollback into a list of strings nobody can see. Past it the text still
/// prints; it just prints as text.
const MAX_HYPERLINKS: usize = 4_096;

/// The longest address accepted. Long enough for the deep file URLs build
/// tools print, short enough that a hostile stream cannot spend a terminal's
/// memory a URI at a time.
const MAX_HYPERLINK_URI_BYTES: usize = 4_096;

/// How many OSC 7788 fold regions one terminal remembers.
const MAX_FOLDS: usize = 4_096;

/// Maximum decoded summary size. A summary longer than a screen row is data,
/// not an accessible label, and the window would truncate it again.
const MAX_FOLD_SUMMARY_BYTES: usize = 1_024;

/// A colour as the wire states it. Kept symbolic rather than resolved to RGB:
/// the palette belongs to the theme, so `Indexed(1)` must stay "colour 1" until
/// the renderer maps it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Color {
    #[default]
    Default,
    /// 0–7 standard, 8–15 bright, 16–255 the xterm cube and greyscale ramp.
    Indexed(u8),
    Rgb(u8, u8, u8),
}

/// What this terminal says it is when a program asks (`CSI c`).
///
/// One question, two answers, and the difference is a platform's: ConPTY 1.22
/// and later BLOCKS at spawn until a primary device attributes reply arrives,
/// and it will only accept the basic-conformance one — the original carries
/// the same pair for the same reason (`DEFAULT_DA1_RESPONSE` /
/// `CONPTY_DA1_RESPONSE`, terminal-capability-replies.ts:10-11).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DeviceIdentity {
    /// `ESC[?1;2c` — a VT100 with the advanced video option, which is what
    /// every program on a unix machine expects to be talking to.
    #[default]
    Vt100Avo,
    /// `ESC[?61;4c` — level 1 conformance, the answer ConPTY unblocks on.
    ConPty,
}

impl DeviceIdentity {
    const fn reply(self) -> &'static [u8] {
        match self {
            Self::Vt100Avo => b"\x1b[?1;2c",
            Self::ConPty => b"\x1b[?61;4c",
        }
    }
}

/// Answered for `CSI > c`, byte-identical to what the terminal the original
/// ships answers (measured, `scratchpad/vtbench/xterm-bench/queries.cjs`).
///
/// Reporting a foreign version string looks like vanity until you remember
/// what DA2 is FOR: a program branches on it. Answering exactly what the
/// original's terminal answers is the only way those branches take the same
/// turn in this window as in that one.
const DEVICE_ATTRIBUTES_SECONDARY: &[u8] = b"\x1b[>0;276;0c";

/// The name this terminal goes by when a program asks which terminal it is in:
/// the XTVERSION answer below, and `TERM_PROGRAM` for every child
/// (`lane::DECLARED_TERMINAL`). One name, so the two answers cannot disagree.
pub const TERMINAL_NAME: &str = "ZeroCode";

/// The version beside [`TERMINAL_NAME`] — the workspace's, which is the app's.
pub const TERMINAL_VERSION: &str = env!("CARGO_PKG_VERSION");

/// A colour a program set over the theme's, for as long as it runs.
///
/// Per terminal, because `OSC 11 ; rgb:...` is that program talking about its
/// own screen. They die with the terminal, and a theme change clears them —
/// the same rule the original applies (`clearColorOverrides`), and for the
/// same reason: a theme apply overwrites the palette wholesale, so a mutation
/// layered on the old one is a mutation of something that no longer exists.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ColorOverrides {
    foreground: Option<Rgb>,
    background: Option<Rgb>,
    cursor: Option<Rgb>,
    ansi: BTreeMap<u8, Rgb>,
}

/// Which of the three colours an `OSC 10`/`11`/`12` slot walks onto.
///
/// They stack: `OSC 10 ; ? ; ?` asks for the foreground and then the
/// background, because the slot advances with each parameter (xterm's
/// `_setOrReportSpecialColor`, mirrored in the original at
/// terminal-view-attribute-responder.ts:44-50).
const SPECIAL_COLOR_SLOTS: [(&str, SpecialColor); 3] = [
    ("10", SpecialColor::Foreground),
    ("11", SpecialColor::Background),
    ("12", SpecialColor::Cursor),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SpecialColor {
    Foreground,
    Background,
    Cursor,
}

impl Color {
    /// Whatever the theme decides — the value almost every cell carries.
    #[must_use]
    pub const fn is_unset(&self) -> bool {
        matches!(self, Self::Default)
    }
}

/// A flag nobody set, for `skip_serializing_if`.
fn is_off(flag: &bool) -> bool {
    !*flag
}

/// Everything SGR can say about a cell.
///
/// The agent TUI this crate hosts paints with colour and weight. Dropping it
/// would leave a screen that is legible but wrong — the worse kind of broken,
/// because nobody notices.
///
/// **Every field is omitted from the wire when it is off**, and that is a
/// performance decision, not a cosmetic one: this struct rides inside every
/// cell of every frame, the pump emits one every 16ms, and Tauri serialises
/// the lot to JSON for the webview to parse. Spelling out seven defaults to
/// say "plain" costs about ten times what the character itself does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct CellStyle {
    #[serde(default, skip_serializing_if = "Color::is_unset")]
    pub fg: Color,
    #[serde(default, skip_serializing_if = "Color::is_unset")]
    pub bg: Color,
    #[serde(default, skip_serializing_if = "is_off")]
    pub bold: bool,
    #[serde(default, skip_serializing_if = "is_off")]
    pub dim: bool,
    #[serde(default, skip_serializing_if = "is_off")]
    pub italic: bool,
    #[serde(default, skip_serializing_if = "is_off")]
    pub underline: bool,
    /// Swap foreground and background at paint time. Left as a flag rather than
    /// pre-swapped so a theme can honour its own inverse treatment.
    #[serde(default, skip_serializing_if = "is_off")]
    pub reverse: bool,
    /// `SGR 9`. Struck through — what a TUI draws a removed line or a finished
    /// todo with.
    #[serde(default, skip_serializing_if = "is_off")]
    pub strike: bool,
    /// `SGR 5`. The program asked for this text to blink.
    #[serde(default, skip_serializing_if = "is_off")]
    pub blink: bool,
    /// Which explicit hyperlink this cell belongs to — an index into the
    /// grid's [`TerminalGrid::link_uri`] table, never the URI itself.
    ///
    /// **It rides the style, and that is the whole design.** The wire already
    /// carries a row as runs of equal style, so a link's edges split a run
    /// exactly where they should with no second array to keep in step; the
    /// serializer that writes SGR transitions can write OSC 8 transitions at
    /// the same seam; and every path that already copies a style — a scroll, a
    /// resize, a cluster rewrite — carries the link with it for free. A cell
    /// holding the string instead would put a heap allocation in a `Copy` type
    /// that a full scrollback has six hundred thousand of.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<std::num::NonZeroU32>,
}

impl CellStyle {
    /// Nothing was set — the state of almost every cell a terminal draws.
    #[must_use]
    pub fn is_plain(&self) -> bool {
        *self == Self::default()
    }
}

/// The grapheme still being assembled, and where it was put.
///
/// A cluster's width is not the sum of its scalars' widths — `👨 ZWJ 👩` is
/// two columns, not four, and `☂` is one where `☂️` is two — so the grid
/// cannot decide a placement when it sees the first scalar and be done. It
/// keeps the cluster, re-measures the whole thing each time it grows, and
/// widens the placement if the answer changed.
#[derive(Debug, Clone)]
enum ClusterText {
    /// The overwhelmingly common case: an ASCII glyph needs no heap buffer
    /// while it is waiting to see whether a combining scalar follows it.
    Ascii(char),
    /// Non-ASCII graphemes, or an ASCII grapheme after it grew, need the full
    /// text for UAX #29 and width measurement.
    Owned(String),
}

impl ClusterText {
    fn from_char(ch: char) -> Self {
        if ch.is_ascii() {
            Self::Ascii(ch)
        } else {
            let mut text = String::new();
            text.push(ch);
            Self::Owned(text)
        }
    }

    fn reset(self, ch: char) -> Self {
        match self {
            Self::Ascii(_) if ch.is_ascii() => Self::Ascii(ch),
            Self::Ascii(_) => Self::from_char(ch),
            Self::Owned(mut text) => {
                text.clear();
                text.push(ch);
                Self::Owned(text)
            }
        }
    }

    fn ends_with_ascii_graphic(&self) -> bool {
        match self {
            Self::Ascii(ch) => ch.is_ascii_graphic(),
            Self::Owned(text) => text.ends_with(|last: char| last.is_ascii_graphic()),
        }
    }

    fn ends_with_ascii(&self) -> bool {
        match self {
            Self::Ascii(ch) => ch.is_ascii(),
            Self::Owned(text) => text.ends_with(|last: char| last.is_ascii()),
        }
    }

    fn push(&mut self, ch: char) {
        match self {
            Self::Ascii(base) => {
                let mut text = String::with_capacity(8);
                text.push(*base);
                text.push(ch);
                *self = Self::Owned(text);
            }
            Self::Owned(text) => text.push(ch),
        }
    }

    fn byte_len(&self) -> usize {
        match self {
            Self::Ascii(_) => 1,
            Self::Owned(text) => text.len(),
        }
    }

    fn truncate(&mut self, len: usize) {
        match self {
            Self::Ascii(_) => debug_assert_eq!(len, 1),
            Self::Owned(text) => text.truncate(len),
        }
    }

    fn is_one_grapheme(&self) -> bool {
        match self {
            Self::Ascii(_) => true,
            Self::Owned(text) => text.graphemes(true).count() == 1,
        }
    }

    fn width(&self) -> usize {
        match self {
            Self::Ascii(ch) => UnicodeWidthChar::width(*ch).unwrap_or(0),
            Self::Owned(text) => UnicodeWidthStr::width(text.as_str()),
        }
    }
}

#[derive(Debug, Clone)]
struct Pending {
    row: usize,
    col: usize,
    /// Every scalar, including the ones past [`ZW_CAPACITY`] that the cell
    /// could not store — the width is measured on the cluster that was
    /// actually written, not on the part that fit.
    text: ClusterText,
    /// Columns it currently occupies on screen. Clamped to the row.
    width: usize,
    /// What the width tables say the cluster measures, unclamped. Kept apart
    /// from `width` because a cluster at the right edge is *placed* narrower
    /// than it *measures*, and asking whether a scalar joins has to compare
    /// against the measurement, not against the clamp.
    measured: usize,
}

/// The second column of a double-width glyph.
///
/// A CJK syllable is drawn across two terminal columns, and the column behind
/// it is spoken for: nothing else may be printed there, and nothing may be
/// drawn in it. Said as a value of `ch` rather than as a field on [`Cell`]
/// because the cell crosses to the webview as JSON, and a third key would
/// change that shape for every consumer in order to carry one bit.
///
/// NUL is the only safe choice — it is what no program prints and what a
/// blank cell never holds. A space would make every empty column look claimed.
const CONTINUATION: char = '\0';

/// One screen cell: a character and how to paint it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cell {
    pub ch: char,
    /// Omitted from the wire when nothing was set. See [`CellStyle`] — this is
    /// the field that decides what a frame costs.
    #[serde(default, skip_serializing_if = "CellStyle::is_plain")]
    pub style: CellStyle,
    /// The rest of the grapheme cluster drawn in this cell — combining marks,
    /// an emoji presentation selector, or the joiners and partners that fuse
    /// several emoji into one glyph.
    ///
    /// None of them own a column: put them in cells of their own and the line
    /// comes out longer than the child laid out. Dropping them loses the
    /// accent off `e\u{301}` and breaks a family emoji into three people. So
    /// they ride along with the character they belong to.
    #[serde(default, skip_serializing_if = "ZeroWidth::is_empty")]
    pub zw: ZeroWidth,
}

/// How many scalars past the first a cell can hold.
///
/// Four, because [`Cell`] is `Copy` and a screen is thousands of them over
/// thousands of scrollback lines — a heap allocation per cell is the frame
/// budget, and the bytes are per-lane memory. Four covers any realistic accent
/// stack, an emoji presentation selector, and a two-person family emoji
/// (`ZWJ 👩 ZWJ 👧`).
///
/// **It is a ceiling, and it truncates the glyph rather than the layout.** A
/// three-person family or a tag sequence runs past it: the extra scalars are
/// not stored, but they are still counted when the cluster's width is
/// measured, so the columns stay right and only the drawing is short. A row
/// that is missing a glyph is a bug you can see; a row whose columns are wrong
/// silently moves everything after it.
const ZW_CAPACITY: usize = 4;

/// The scalars a cell carries besides its first.
///
/// A fixed array rather than a `Vec` to keep [`Cell`] `Copy`; a string on the
/// wire, because that is the shortest encoding and the window appends it to
/// the character without having to know how many there are.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(into = "String", from = "String")]
pub struct ZeroWidth {
    scalars: [char; ZW_CAPACITY],
    len: u8,
}

impl Default for ZeroWidth {
    fn default() -> Self {
        Self {
            scalars: ['\0'; ZW_CAPACITY],
            len: 0,
        }
    }
}

impl ZeroWidth {
    /// Nothing rides along — the state of almost every cell.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The scalars, in the order they arrived.
    pub fn scalars(&self) -> impl Iterator<Item = char> + '_ {
        self.scalars.iter().copied().take(self.len as usize)
    }

    /// Take one more if there is room. Past the ceiling the scalar is dropped
    /// from the glyph — never from the width, which is measured on the whole
    /// cluster.
    fn push(&mut self, ch: char) {
        if (self.len as usize) < ZW_CAPACITY {
            self.scalars[self.len as usize] = ch;
            self.len += 1;
        }
    }
}

impl From<ZeroWidth> for String {
    fn from(zw: ZeroWidth) -> Self {
        zw.scalars().collect()
    }
}

impl From<String> for ZeroWidth {
    fn from(text: String) -> Self {
        let mut zw = Self::default();
        for ch in text.chars() {
            zw.push(ch);
        }
        zw
    }
}

impl Cell {
    /// Is this the second column of the glyph to its left?
    ///
    /// A renderer must skip these. The glyph in front already advances two
    /// columns of its own, so drawing anything here draws it twice.
    #[must_use]
    pub const fn is_continuation(&self) -> bool {
        self.ch == CONTINUATION
    }

    /// Does a reader see anything here?
    ///
    /// A blank-looking cell is not always blank: a TUI paints a status bar by
    /// setting a background on spaces, and a row of those is ink. The rule
    /// lives here because two callers need the same answer — where a written
    /// row ENDS (`serialize::row_end`) and where a screen's output ends
    /// ([`TerminalGrid::preview_rows`]) — and two spellings of "blank" is how
    /// one of them comes to trim a coloured bar the other kept.
    #[must_use]
    pub fn is_ink(&self) -> bool {
        !(self.is_continuation() || self.ch == ' ' && self.style.is_plain())
    }
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            ch: ' ',
            style: CellStyle::default(),
            zw: ZeroWidth::default(),
        }
    }
}

/// Which part of a marked fold region a row represents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FoldRole {
    /// The region's always-visible first row.
    Header,
    /// A row shown only while the region is collapsed — the density line a
    /// folded cell keeps (`  └ … +N lines`), declared by `teaser=<n>` on the
    /// `begin` marker. Opening the region hides it: the body it summarised
    /// is on screen then.
    Teaser,
    /// A row hidden while the region is collapsed.
    Body,
}

/// What a consumer was last handed for one screen row.
///
/// Held so the grid can answer a question `dirty_rows` cannot: a flag says
/// somebody WROTE to a row, and the overwhelmingly common thing a full-screen
/// program writes is the same row it wrote last frame. Measured with a 24×80
/// terminal fed a TUI's own redraw: changing exactly one row resent **all
/// twenty-four**, every frame, forever. At 220×60 each of those rows is a
/// 9.7 KB clone, a packed `String`, ~370 B of JSON, a slice of an
/// `evaluateJavaScript` the webview parses as source, and then
/// `expandPackedRow`/`runsOf`/`paintRow` on the other side.
///
/// The three fields together are what a row IS on the wire ([`GridRow`]), so
/// comparing them is comparing the frame the consumer already has.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SentRow {
    cells: Vec<Cell>,
    wrap: Option<u16>,
    fold: Option<RowFold>,
}

/// The fold region attached to one row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RowFold {
    pub id: u32,
    pub role: FoldRole,
}

/// Initial presentation and accessible summary for one fold region.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FoldMeta {
    pub id: u32,
    pub collapsed: bool,
    pub summary: String,
}

#[derive(Debug, Clone, Copy)]
struct OpenFold {
    id: u32,
    header_seen: bool,
    /// Body rows still to be claimed as the region's collapsed teaser.
    teaser_left: u32,
}

/// One changed row, at its index in the visible screen.
///
/// In memory this is cells; **on the wire it is packed**. A frame used to
/// cross to the webview as one JSON object per cell — `{"ch":"a"}` is ten
/// bytes for one letter, and a busy agent screen is thousands of them per
/// tick through serialize, the IPC string, `JSON.parse` and the collector.
/// That soup is what typing lag is made of on the receiving thread. Packed,
/// a row is its text once, style RUNS only where ink was set, and zero-width
/// riders only where they exist:
///
/// ```json
/// {"index":3,"text":"error: two left","runs":[[0,6,{"fg":{"indexed":1}}]]}
/// ```
///
/// One code point per column — the continuation NUL included — so the window
/// maps `[...text]` onto columns with no width logic of its own. The struct
/// itself keeps `cells`: every reader and every test stays cell-shaped, and
/// only the serde boundary translates (`WireRow`, private below).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GridRow {
    pub index: usize,
    pub cells: Vec<Cell>,
    /// How many columns of the row ABOVE this one carried into it, when this
    /// row is the continuation of a soft-wrapped line. `None` starts a fresh
    /// logical line.
    ///
    /// The ledger, not the flag: a wide glyph that wrapped early leaves a cell
    /// it never wrote, so a consumer joining the rows back together has to know
    /// how much of the producer row actually counted. That number is
    /// `TerminalGrid::wrap`'s and this is the only way out of the grid — the
    /// window renders one visual row at a time and cannot derive it.
    ///
    /// Why it has to leave the grid at all: a path or a URL printed at the
    /// right edge is split across two rows, and a scan that sees one row at a
    /// time cannot find it. `TerminalGrid::flatten_logical` already joins
    /// them for SEARCH — this carries the same fact to the renderer's own scan
    /// instead of asking it to re-derive the ledger in another language.
    pub wrap: Option<u16>,
    /// OSC 7788 region membership. Absent for ordinary terminal output.
    pub fold: Option<RowFold>,
}

/// The packed spelling of one [`GridRow`] — see the wire note there.
#[derive(Serialize, Deserialize)]
struct WireRow {
    index: usize,
    text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    runs: Vec<(usize, usize, CellStyle)>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    zw: Vec<(usize, String)>,
    /// Absent on the wire for every row that starts its own line, which is
    /// almost all of them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    wrap: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    fold: Option<RowFold>,
}

impl From<&GridRow> for WireRow {
    fn from(row: &GridRow) -> Self {
        let mut text = String::with_capacity(row.cells.len());
        let mut runs: Vec<(usize, usize, CellStyle)> = Vec::new();
        let mut zw: Vec<(usize, String)> = Vec::new();
        for (col, cell) in row.cells.iter().enumerate() {
            text.push(cell.ch);
            if !cell.zw.is_empty() {
                zw.push((col, String::from(cell.zw)));
            }
            if !cell.style.is_plain() {
                match runs.last_mut() {
                    Some((at, len, style)) if *at + *len == col && *style == cell.style => {
                        *len += 1;
                    }
                    _ => runs.push((col, 1, cell.style)),
                }
            }
        }
        Self {
            index: row.index,
            text,
            runs,
            zw,
            wrap: row.wrap,
            fold: row.fold,
        }
    }
}

impl From<WireRow> for GridRow {
    fn from(wire: WireRow) -> Self {
        let mut cells: Vec<Cell> = wire
            .text
            .chars()
            .map(|ch| Cell {
                ch,
                ..Cell::default()
            })
            .collect();
        for (at, len, style) in wire.runs {
            for cell in cells.iter_mut().skip(at).take(len) {
                cell.style = style;
            }
        }
        for (col, riders) in wire.zw {
            if let Some(cell) = cells.get_mut(col) {
                cell.zw = ZeroWidth::from(riders);
            }
        }
        Self {
            index: wire.index,
            cells,
            wrap: wire.wrap,
            fold: wire.fold,
        }
    }
}

impl Serialize for GridRow {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        WireRow::from(self).serialize(serializer)
    }
}

/// Both spellings deserialize — the packed wire above, and the cell form the
/// window's own gates and any stored snapshot still speak.
impl<'de> Deserialize<'de> for GridRow {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum EitherRow {
            Packed(WireRow),
            Cells {
                index: usize,
                cells: Vec<Cell>,
                /// The older cell form predates the ledger; a snapshot written
                /// then names no wrap, which reads as "starts its own line" —
                /// the same answer the field's absence gives on the wire.
                #[serde(default)]
                wrap: Option<u16>,
                #[serde(default)]
                fold: Option<RowFold>,
            },
        }
        Ok(match EitherRow::deserialize(deserializer)? {
            EitherRow::Packed(wire) => Self::from(wire),
            EitherRow::Cells {
                index,
                cells,
                wrap,
                fold,
            } => Self {
                index,
                cells,
                wrap,
                fold,
            },
        })
    }
}

/// What changed on a screen since the last time anyone asked.
///
/// This is the payload that crosses to the view layer, and it exists because
/// the alternative does not scale: serializing every cell of every lane at
/// every read is what melts a window hosting eight streaming agents.
///
/// ## The shift contract
///
/// `scrolled_lines` is the whole point, and a consumer that ignores it renders
/// garbage. A terminal that is simply printing scrolls constantly, and the rows
/// that scrolled did not *change* — they moved. So they are not resent. The
/// consumer must, in this order:
///
/// 1. if `full`, discard its buffer entirely and treat `rows` as absolute;
/// 2. otherwise shift its own rows up by `scrolled_lines`, filling the exposed
///    bottom with blanks;
/// 3. overwrite the rows named in `rows`.
///
/// `full` wins over `scrolled_lines` and is set whenever a shift would be
/// meaningless — a resize, or the alternate screen swapping in or out. Both
/// change what row *n* even refers to, so a shift applied across one corrupts
/// the view.
///
/// ## The view shift
///
/// A person scrolling back through history moves a window over rows that do
/// not change, and a frame that resends every row for every notch is the
/// stutter they feel: measured 2026-09-15 in the window harness, a 60×220
/// screen cost 12.8 ms a notch repainted whole and 0.5 ms shifted — and a
/// trackpad sends a notch a frame. So when the view moved and nothing else did
/// (no output, no dirty live row, no fold composition, no alternate screen,
/// and a move shorter than the screen), the delta carries `view_shift` and
/// only the exposed rows: the consumer moves its rows by that many — down for
/// a negative shift, up for a positive one, the same motion `scrolled_lines`
/// asks for — and overwrites the exposed edge. Anything else while scrolled
/// back is the absolute frame it always was.
///
/// Rows are **not** trimmed the way [`TerminalGrid::visible_text`] trims: a
/// How much of the mouse a program asked to hear about.
///
/// Off until it says otherwise. The window sends nothing while it is off —
/// which matters most for `Motion`: reporting every pointer move to a program
/// that never asked is a message per frame of pointer travel, for nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MouseTracking {
    #[default]
    Off,
    /// `CSI ?1000h` — presses and releases.
    Click,
    /// `CSI ?1002h` — those, plus motion while a button is down.
    Drag,
    /// `CSI ?1003h` — those, plus motion with no button at all.
    Motion,
}

/// cleared row has to arrive as blanks, or the consumer keeps painting stale
/// text under it.
fn is_zero_shift(shift: &isize) -> bool {
    *shift == 0
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GridDelta {
    /// Rows whose cells changed, ascending by index.
    pub rows: Vec<GridRow>,
    /// Absolute buffer line for each slot of a composed full frame. Folding
    /// can skip body rows or fill the live screen from history, so a display
    /// slot is not always `scrollback_len - view_offset + index`. Ordinary
    /// frames omit this map and keep that arithmetic coordinate contract.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub row_lines: Vec<usize>,
    /// How many lines scrolled off the top since the last delta. See the shift
    /// contract above.
    pub scrolled_lines: usize,
    /// How far the VIEW moved over unchanged history since the last delta,
    /// when that move is the only thing that changed: positive toward the live
    /// screen (the consumer's rows move up, the exposed rows are the bottom
    /// ones), negative back into history (rows move down, the exposed rows are
    /// the top ones). `rows` then carries only the exposed rows, in the same
    /// slot numbering. Zero on every other frame — a full frame, an output
    /// shift, a move that printed at the same time — so a consumer that never
    /// learned this field still reads a correct `full` or `scrolled_lines`
    /// frame. See the view shift under the shift contract above.
    #[serde(default, skip_serializing_if = "is_zero_shift")]
    pub view_shift: isize,
    /// Cursor as `(row, col)`, both zero-based.
    pub cursor: (usize, usize),
    /// Present only on the delta that follows a title change (`OSC 0`/`OSC 2`).
    pub title: Option<String>,
    pub alt_screen: bool,
    /// Screen size as `(rows, cols)`, so a consumer can size its buffer without
    /// a second call.
    pub size: (usize, usize),
    /// `rows` is the entire screen and replaces whatever the consumer holds.
    pub full: bool,
    /// Addresses behind [`CellStyle::link`] ids the consumer has not been told
    /// yet, as `(id, uri)` — the whole table when `full`, nothing at all on
    /// the overwhelming majority of frames.
    ///
    /// Sent once rather than per frame: a run's id keeps naming the same
    /// address forever, so a repaint of a line that happens to hold a link
    /// owes the wire nothing.
    ///
    /// The address is shared with the grid's own table rather than copied: a
    /// full frame carries every link it has ever seen, and a scrollback that
    /// has collected thousands of them would otherwise re-copy all of them
    /// every time a consumer reattaches.
    #[serde(default, skip_serializing_if = "Vec::is_empty", with = "link_wire")]
    pub links: Vec<(u32, Arc<str>)>,
    /// Fold metadata not previously sent, or the complete table on a full
    /// frame. Row membership lives on [`GridRow::fold`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub folds: Vec<FoldMeta>,
    /// Whether the caret should be drawn. A TUI hides it while it redraws
    /// (`CSI ?25l`) and asks for it back on its own prompt; drawing one anyway
    /// puts a caret in the middle of an interface that meant to have none.
    pub cursor_visible: bool,
    /// How much of the mouse the program wants. Carried on the frame rather
    /// than asked for separately: the window has to know before it decides
    /// whether a pointer move is worth a message at all.
    pub mouse_tracking: MouseTracking,
    /// The program asked for SGR mouse encoding (`CSI ?1006h`), which is the
    /// only one that can address a screen wider than 223 columns.
    pub mouse_sgr: bool,
    /// The program asked to be told when this terminal gains or loses the
    /// keyboard (`CSI ?1004h`).
    ///
    /// Carried for the same reason the mouse modes are: the window has to know
    /// BEFORE it decides whether a focus change is worth a message. Almost no
    /// shell asks, so almost every tab switch should cost nothing at all — and
    /// a window that had to ask would spend a round trip per switch to be told
    /// "no".
    pub focus_reporting: bool,
    /// The program rang the bell (`BEL`, 0x07) since the last delta.
    ///
    /// An event, not a state — it is true on the one frame that carries it and
    /// false on every frame after, which is why it is taken and cleared like
    /// `title` rather than held like `alt_screen`. A shell rings this when a
    /// long build finishes or when a prompt wants an answer, and swallowing it
    /// (which this grid did) means a background tab has no way at all to say
    /// so. A watched screen carries it with its delta; [`GridNews`] is the
    /// metadata-only road for a screen whose rows stay native.
    pub bell: bool,
    /// How far back into history this frame is showing, in lines. Zero is the
    /// live screen.
    ///
    /// The window has always been able to *ask* for a scroll and has never
    /// been able to see where it landed — which is why this terminal had no
    /// scrollbar. Not "no scrollbar yet": there was no number to draw one
    /// from, so the absence was structural rather than a piece of UI nobody
    /// got round to. Two fields close it, and both ride the frame that
    /// already goes out rather than becoming a question the window has to ask.
    pub view_offset: usize,
    /// How many lines of history stand behind the live screen. With
    /// `view_offset` and the row count this is the whole of a scrollbar: where
    /// the thumb sits, how tall it is, and whether there is anything to scroll
    /// at all.
    pub scrollback_len: usize,
    /// Total history rows removed from the front during this grid's lifetime.
    /// This buffer origin is independent of the DOM shift: full/composed
    /// frames and snapshots carry it too. A consumer subtracts its previous
    /// origin once, including after missed deltas or repeated snapshots.
    #[serde(default)]
    pub scrollback_trimmed: usize,
}

/// How [`GridDelta::links`] rides the wire.
///
/// Written by hand because serde speaks for `Arc<T>` only under its `rc`
/// feature, and turning that on would change how every reference-counted
/// field in the workspace crosses — a decision far larger than one link
/// table. The shape it writes is the pair the field used to be, `[id, uri]`,
/// so a consumer reading these frames cannot tell that the address behind the
/// second slot is now shared rather than owned.
mod link_wire {
    use std::sync::Arc;

    use serde::ser::SerializeSeq;
    use serde::{Deserialize, Deserializer, Serializer};

    pub(super) fn serialize<S: Serializer>(
        links: &[(u32, Arc<str>)],
        out: S,
    ) -> Result<S::Ok, S::Error> {
        let mut seq = out.serialize_seq(Some(links.len()))?;
        for (id, uri) in links {
            seq.serialize_element(&(id, &**uri))?;
        }
        seq.end()
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(
        input: D,
    ) -> Result<Vec<(u32, Arc<str>)>, D::Error> {
        Ok(Vec::<(u32, String)>::deserialize(input)?
            .into_iter()
            .map(|(id, uri)| (id, Arc::from(uri)))
            .collect())
    }
}

/// Title and bell changes that must cross even when nobody is receiving rows.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GridNews {
    pub title: Option<String>,
    pub bell: bool,
}

impl GridNews {
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.title.is_none() && !self.bell
    }
}

/// A pattern the engine could not parse — the person is still typing it.
///
/// Carries no message on purpose: the engine's diagnostics name offsets in a
/// pattern the window is already displaying, and the bar's whole answer is a
/// tint on the input, not a paragraph under it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BadPattern;

impl std::fmt::Display for BadPattern {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str("the pattern does not parse")
    }
}

impl std::error::Error for BadPattern {}

/// Where one match of a search landed.
///
/// `line` is absolute over history AND screen — 0 is the oldest line the
/// scrollback still holds, `scrollback_len + row` is a row of the live screen.
/// One coordinate space rather than two, because the caller's next move is to
/// put the hit on screen, and "which of the two spaces is this in" is exactly
/// the arithmetic that gets that wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TermHit {
    pub line: usize,
    /// Column of the first cell of the match, in grid columns — not in bytes
    /// and not in characters. A Korean line's tenth character is its twentieth
    /// column, and the column is what a highlight has to be drawn at.
    pub col: usize,
    /// How many columns the match covers, for the same reason.
    pub len: usize,
}

/// One end of a selection, in the space [`TermHit`] speaks.
///
/// `col` is a cell BOUNDARY rather than a cell: 0 stands before the first
/// column and `cols` after the last, so a selection's end can name "after
/// the last glyph" without a column that does not exist. The window's
/// selection model holds the same pair (`ui/shell-term-selection.js`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TermPoint {
    pub line: usize,
    pub col: usize,
}

/// The primary screen parked while the alternate screen is up.
#[derive(Debug)]
struct SavedScreen {
    cells: Vec<Vec<Cell>>,
    /// The soft-wrap ledger travels with the rows it describes.
    wrap: Vec<Option<u16>>,
    folds: Vec<Option<RowFold>>,
    cursor_row: usize,
    cursor_col: usize,
    pen: CellStyle,
}

/// One flattened character of a logical line: what it is, and which cell it
/// came from — the two facts every hit translation needs.
#[derive(Debug, Clone, Copy)]
struct FlatChar {
    ch: char,
    line: usize,
    col: usize,
}

/// A logical line, flattened for matching. See
/// [`TerminalGrid::flatten_logical`].
#[derive(Debug, Default)]
struct FlatLine {
    chars: Vec<FlatChar>,
}

/// What the glyph watch caught this round — one take clears both halves.
/// See [`TerminalGrid::take_glyph_drawn`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GlyphDrawn {
    /// The watched glyph was drawn, on either screen.
    pub anywhere: bool,
    /// …and at least one of those draws went to the alternate screen — the
    /// only place grok's `❯` is grok's own rather than a shell prompt's.
    pub in_alt_screen: bool,
}

/// Clipboard text is intentionally redacted from terminal debug output.
///
/// OSC 52 commonly carries tokens, generated passwords, and selected source.
/// It must cross the explicit clipboard event, never an incidental `Debug`
/// log of the terminal state.
struct Osc52ClipboardWrite(String);

impl std::fmt::Debug for Osc52ClipboardWrite {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Osc52ClipboardWrite([redacted])")
    }
}

/// Screen state. Implements [`Perform`], so it is driven by [`Terminal`].
#[derive(Debug)]
pub struct TerminalGrid {
    rows: usize,
    cols: usize,
    cells: Vec<Vec<Cell>>,
    /// The soft-wrap ledger, one entry per screen row. `Some(k)` on row *r*
    /// says the row CONTINUES row *r−1*, which contributed exactly *k*
    /// columns before the edge — `cols` for an ordinary wrap, one less when
    /// a two-column glyph wrapped early and left a cell it never wrote.
    /// That count is what keeps a phantom blank out of the joined text: a
    /// terminal cannot tell a never-written cell from a printed space, but
    /// the wrap knew, at the only moment anyone did.
    ///
    /// Search-only state: it never travels in a [`GridDelta`], because the
    /// window paints rows and the join is the searcher's business.
    wrap: Vec<Option<u16>>,
    /// Fold membership parallel to `cells`, kept per row so millions of
    /// scrollback cells do not repeat the same four-byte fact.
    folds: Vec<Option<RowFold>>,
    scrollback: VecDeque<Vec<Cell>>,
    /// [`Self::wrap`] for the rows history holds, in the same order.
    scroll_wrap: VecDeque<Option<u16>>,
    /// Fold membership parallel to compact scrollback rows.
    scroll_folds: VecDeque<Option<RowFold>>,
    /// The logical width each compact history row had when it left the screen.
    ///
    /// It cannot be reconstructed from today's `cols`: history survives a
    /// resize, so one scrollback can contain rows from several geometries.
    /// Readers pad a stored row to this width with [`Cell::default`].
    scroll_cols: VecDeque<usize>,
    scrollback_cap: usize,
    scrollback_trimmed: usize,
    cursor_row: usize,
    cursor_col: usize,
    /// Style the next printed character carries — the graphic rendition SGR
    /// mutates.
    pen: CellStyle,
    /// Every explicit hyperlink this terminal has been told about, in the
    /// order it heard them. A cell holds `Some(index + 1)`; the offset is what
    /// lets [`Cell`] stay four bytes lighter than an `Option<u32>` would make
    /// it (`NonZeroU32` has no separate discriminant).
    ///
    /// It is not pruned while the scrollback lives: a line scrolled out of
    /// sight still names its link, and a table that forgot would turn that
    /// line's addresses into dangling numbers.
    links: Vec<Arc<str>>,
    /// The same addresses, keyed by address — `OSC 8` arrives once per printed
    /// name and a directory listing asks this question for every file, so the
    /// answer cannot be a walk down a table that is allowed to hold four
    /// thousand rows. The key shares its allocation with the row it points at,
    /// so the index costs a hash slot rather than a second copy of every URI.
    link_ids: HashMap<Arc<str>, NonZeroU32>,
    /// How many of `links` a consumer has already been sent. Everything past
    /// it rides the next delta once — a table re-sent every frame would put a
    /// URI on the wire for every repaint of a line nobody touched.
    links_sent: usize,
    fold_table: Vec<FoldMeta>,
    folds_sent: usize,
    open_fold: Option<OpenFold>,
    title: Option<String>,
    bracketed_paste: bool,
    alt_screen: bool,
    /// Primary screen parked while the alternate screen is up. A full-screen
    /// program expects its host to hand back exactly what was on screen when it
    /// exits.
    saved_screen: Option<SavedScreen>,
    /// Set whenever anything changes, cleared by the renderer. The view layer
    /// only repaints lanes that moved — eight streaming lanes otherwise repaint
    /// the whole window at every read.
    dirty: bool,
    /// The grapheme cluster the last `print` began, if it can still grow.
    ///
    /// Cleared by anything that moves the cursor or rewrites the row: a mark
    /// arriving after `ESC[H` must attach to nothing rather than to whatever
    /// grapheme happens to sit behind the cursor now.
    pending: Option<Pending>,
    /// What the terminal owes the program that is running in it.
    ///
    /// Some sequences are questions — `CSI c` asks what this terminal is,
    /// `CSI 6n` asks where the cursor is — and a program that asks **waits for
    /// the answer**. Swallowing them, which is what "unsupported sequences are
    /// swallowed" quietly did, leaves it waiting until its own timeout: a UI
    /// that does not appear and an app that feels slow. The answers queue here
    /// and the pty writes them back to the child.
    replies: Vec<u8>,
    /// The latest valid OSC 52 clipboard write not yet handed to the host.
    ///
    /// One value is deliberate backpressure: a TUI may repaint and issue many
    /// copies in one pump, but only its latest request can be the clipboard at
    /// the end of that turn. Keeping that value also means a hidden lane gets
    /// the side effect without serialising its screen.
    osc52_clipboard_write: Option<Osc52ClipboardWrite>,
    /// Where the cursor was when `ESC 7`/`CSI s` saved it.
    saved_cursor: Option<(usize, usize)>,
    /// The rows scrolling is confined to, as `CSI r` set them. `None` is the
    /// whole screen.
    scroll_region: Option<(usize, usize)>,
    /// Whether the caret should be drawn — `CSI ?25h`/`l`.
    cursor_visible: bool,
    /// Times the caret has been *shown*, cumulatively — the edge behind the
    /// level above. See [`TerminalGrid::cursor_shows`].
    cursor_shows: u64,
    /// The one glyph a reader has asked to be told about, and whether the
    /// paint path has drawn it since that reader last asked.
    ///
    /// One character rather than a record of everything drawn, because this
    /// is the paint path: a comparison per printed character costs nothing,
    /// and costs nobody at all while no delivery is waiting, where a set of
    /// every character a round drew would put a hash on the same path two
    /// per-cell costs were taken out of. See
    /// [`TerminalGrid::take_glyph_drawn`].
    watched_glyph: Option<char>,
    glyph_drawn: bool,
    /// …and whether any of those draws landed while the ALTERNATE SCREEN was
    /// up, judged at pen time. The screen a glyph went to is part of what the
    /// glyph means: grok's `❯` is its own only there — in the normal buffer
    /// the same character is starship's prompt (Orca anchors the marker on
    /// `?1049h` for exactly this, draft-paste-ready-scanner.ts:49-56).
    glyph_drawn_in_alt: bool,
    /// The program asked to be told when the terminal gains or loses focus
    /// (`CSI ?1004h`), so the window sends `ESC[I`/`ESC[O` when it does.
    focus_reporting: bool,
    /// How much of the mouse the program asked for, and in which encoding.
    mouse_tracking: MouseTracking,
    mouse_sgr: bool,
    /// Set when any of those changes — including focus reporting below.
    ///
    /// None of them dirties a cell, and a mode the window never hears about is
    /// a mode that does not exist as far as the window is concerned. It was
    /// `mouse_dirty` while the pointer was the only mode that travelled; it
    /// carries focus reporting now, for the same reason and by the same road.
    modes_dirty: bool,
    /// A bell rang and no delta has carried it out yet.
    ///
    /// Like `modes_dirty` this is news rather than screen: BEL moves no cell,
    /// so nothing else in the frame would rise for it and a bell rung into a
    /// still screen would never be delivered. It is therefore its own reason
    /// to send a frame.
    bell: bool,
    /// The same two facts — a bell rang, the title changed — as the pump has
    /// yet to ANNOUNCE them, which is a different cursor from the delta's.
    ///
    /// A frame now waits for the screen that reads it to come and take it
    /// (`readers`), and a screen that is hidden or minimised does not come;
    /// the tray and the tab badge must not wait with it. So the pump takes
    /// these every round, off every shell, whether or not anybody takes a
    /// delta — and taking them leaves `bell` and `title_dirty` alone, because
    /// the delta still carries what the screen shows. One edge per fact per
    /// pane is what keeps a bell counted once.
    announce_bell: bool,
    announce_title: bool,
    /// A frame the program declared atomic (`CSI ?2026h`) and has not closed.
    ///
    /// While one is open the grid hands over nothing, so the window never
    /// paints a repaint in pieces. Held with the instant it began, because the
    /// hold is a courtesy and not a lock: a program that sets it and then
    /// crashes must not freeze the screen forever.
    sync_began: Option<std::time::Instant>,
    /// Which rows' *cells* changed, parallel to `cells` and always the same
    /// length. Coarser than `dirty`, which also rises for a cursor move.
    dirty_rows: Vec<bool>,
    /// What the consumer holds for each screen row, or `None` where the grid
    /// cannot say. Parallel to `dirty_rows` and moved wherever those move, so
    /// index *i* always describes the row the consumer has at index *i*.
    ///
    /// The cost of keeping it is one screen's cells per pane — 580 KB at
    /// 220×60 — against a comparison measured at 0.73 µs a row (43.8 µs for a
    /// whole screen, and that is the WORST case: the comparison stops at the
    /// first cell that differs, so a row that really changed leaves early).
    /// What one skipped row saves downstream is several times that before the
    /// frame has even left this process.
    sent_rows: Vec<Option<SentRow>>,
    /// Lines that scrolled off the top since the last delta was taken. The rows
    /// they displaced are not resent — see [`GridDelta`]'s shift contract.
    scrolled_since_take: usize,
    title_dirty: bool,
    /// A resize or an alternate-screen swap happened, so row *n* no longer
    /// means what it did and no shift can bridge it.
    full_repaint: bool,
    /// Whether the caret was being drawn when the last delta was taken. A
    /// change here is a change the window has to hear about even when no cell
    /// moved — hiding the caret IS the frame.
    taken_cursor_visible: bool,
    /// Where the cursor was when the last delta was taken.
    ///
    /// Compared rather than flagged on purpose. A caret moves from a dozen
    /// places — `CR`, `LF`, backspace, tab, five CSI motions, a wrap — and
    /// flagging each one means the day someone adds the thirteenth, the caret
    /// silently stops following. Comparing cannot miss a site.
    taken_cursor: (usize, usize),
    /// How far the view is scrolled back into history, in lines. Zero is the
    /// live screen, which is the only state the shift contract can serve —
    /// while this is nonzero every delta goes out `full`, composed from the
    /// scrollback and however much of the live screen still fits under it.
    ///
    /// Anchored to CONTENT, not to distance: a line entering the scrollback
    /// while somebody is reading history pushes the offset one deeper, so
    /// what they are reading stays under their eyes instead of sliding away
    /// at the program's output rate.
    view_offset: usize,
    /// The scrollbar's two numbers as the last delta reported them, so a
    /// change in either is its own reason to send a frame. See `take_delta`.
    taken_view: (usize, usize),
    /// What this terminal is drawn in, once somebody has resolved it.
    ///
    /// `None` until then, and while it is `None` every colour question is
    /// answered with SILENCE. There is no safe default here: a fabricated
    /// black becomes the child's idea of the screen for the rest of its life,
    /// and it paints a dark theme's colours onto a light one. The original
    /// holds the same invariant for the same reason
    /// (terminal-view-attribute-responder.ts:25-28).
    ///
    /// Shared rather than owned: one resolved palette serves every lane in the
    /// window, and a theme change replaces one pointer instead of copying 768
    /// bytes into each terminal.
    colors: Option<Arc<TerminalColors>>,
    /// What the program running here has said about those colours.
    color_overrides: ColorOverrides,
    /// A program asked to be told when the colour scheme flips (`CSI ?2031h`).
    ///
    /// Subscribing is NOT a question — the question is `CSI ?996n` — so
    /// nothing is sent when this rises. fish arms it for the millisecond it
    /// paints a prompt, and a reply landing after that prints as literal text
    /// (the original's #9993).
    color_scheme_subscribed: bool,
    /// What that program was last told, so an unchanged scheme says nothing.
    /// Seeded when the subscription opens, so the first notification is a
    /// change rather than an introduction.
    told_color_scheme: Option<bool>,
    /// The answer this terminal gives to `CSI c`.
    identity: DeviceIdentity,
}

impl TerminalGrid {
    #[must_use]
    pub fn new(rows: usize, cols: usize) -> Self {
        let rows = rows.max(1);
        let cols = cols.max(1);
        Self {
            rows,
            cols,
            cells: vec![vec![Cell::default(); cols]; rows],
            wrap: vec![None; rows],
            folds: vec![None; rows],
            scrollback: VecDeque::new(),
            scroll_wrap: VecDeque::new(),
            scroll_folds: VecDeque::new(),
            scroll_cols: VecDeque::new(),
            scrollback_cap: DEFAULT_SCROLLBACK_LINES,
            scrollback_trimmed: 0,
            cursor_row: 0,
            cursor_col: 0,
            pen: CellStyle::default(),
            links: Vec::new(),
            link_ids: HashMap::new(),
            links_sent: 0,
            fold_table: Vec::new(),
            folds_sent: 0,
            open_fold: None,
            title: None,
            bracketed_paste: false,
            mouse_tracking: MouseTracking::Off,
            mouse_sgr: false,
            modes_dirty: false,
            bell: false,
            alt_screen: false,
            saved_screen: None,
            dirty: false,
            pending: None,
            replies: Vec::new(),
            osc52_clipboard_write: None,
            saved_cursor: None,
            scroll_region: None,
            cursor_visible: true,
            cursor_shows: 0,
            watched_glyph: None,
            glyph_drawn: false,
            glyph_drawn_in_alt: false,
            taken_cursor_visible: true,
            focus_reporting: false,
            sync_began: None,
            dirty_rows: vec![false; rows],
            sent_rows: vec![None; rows],
            scrolled_since_take: 0,
            title_dirty: false,
            announce_bell: false,
            announce_title: false,
            full_repaint: false,
            taken_cursor: (0, 0),
            view_offset: 0,
            taken_view: (0, 0),
            colors: None,
            color_overrides: ColorOverrides::default(),
            color_scheme_subscribed: false,
            told_color_scheme: None,
            identity: DeviceIdentity::default(),
        }
    }

    /// Hand this terminal the colours it is drawn in.
    ///
    /// Clears whatever the program had layered on top: the palette underneath
    /// those mutations is gone, so keeping them would mean answering with a
    /// colour composed of two different themes.
    pub fn set_colors(&mut self, colors: Arc<TerminalColors>) {
        self.colors = Some(colors);
        self.color_overrides = ColorOverrides::default();
    }

    /// Say what this terminal answers `CSI c` with. See [`DeviceIdentity`].
    pub const fn set_identity(&mut self, identity: DeviceIdentity) {
        self.identity = identity;
    }

    /// The colours this terminal was told it is drawn in, if it was told.
    #[must_use]
    pub const fn colors(&self) -> Option<&Arc<TerminalColors>> {
        self.colors.as_ref()
    }

    /// Tell a subscribed program that the colour scheme flipped.
    ///
    /// Silent unless a program asked (`CSI ?2031h`) and the answer actually
    /// changed — an appearance re-apply that changes a font is not a flip, and
    /// the original guards the same edge (`maybePushMode2031Flip`).
    pub fn notify_color_scheme(&mut self, dark: bool) {
        if !self.color_scheme_subscribed || self.told_color_scheme == Some(dark) {
            return;
        }
        self.told_color_scheme = Some(dark);
        self.replies.extend_from_slice(if dark {
            b"\x1b[?997;1n"
        } else {
            b"\x1b[?997;2n"
        });
    }

    /// The colour one special slot reports: what the program set, else the
    /// theme's, else nothing at all.
    fn special_color(&self, slot: SpecialColor) -> Option<Rgb> {
        let (override_value, themed) = match slot {
            SpecialColor::Foreground => (
                self.color_overrides.foreground,
                self.colors.as_ref().map(|colors| colors.foreground),
            ),
            SpecialColor::Background => (
                self.color_overrides.background,
                self.colors.as_ref().map(|colors| colors.background),
            ),
            SpecialColor::Cursor => (
                self.color_overrides.cursor,
                self.colors.as_ref().map(|colors| colors.cursor),
            ),
        };
        override_value.or(themed)
    }

    /// Whether this terminal reads as a dark one right now, if it can be said
    /// at all. Judged from the colours in force, not from the window's mode.
    fn reads_as_dark_now(&self) -> Option<bool> {
        Some(reads_as_dark(
            self.special_color(SpecialColor::Background)?,
            self.special_color(SpecialColor::Foreground)?,
        ))
    }

    /// Queue one colour report: `OSC {ident} ; rgb:.... ST`.
    ///
    /// ST rather than BEL, and sixteen bits a channel, because that is what
    /// the original replies byte-for-byte
    /// (terminal-view-attribute-responder.ts:84-88) and a program that
    /// compares the reply it gets to the one it expects must find them equal.
    fn report_color(&mut self, ident: &str, rgb: Rgb) {
        let spec = format_x_color_rgb_spec(rgb);
        self.replies
            .extend_from_slice(format!("\x1b]{ident};{spec}\x1b\\").as_bytes());
    }

    /// `OSC 10/11/12`: each parameter walks one slot further along
    /// foreground → background → cursor. `?` asks, anything else sets.
    fn special_colors(&mut self, params: &[&[u8]], first_slot: usize) {
        for (offset, value) in params.iter().skip(1).enumerate() {
            let Some((ident, slot)) = SPECIAL_COLOR_SLOTS.get(first_slot + offset).copied() else {
                return;
            };
            if *value == b"?" {
                if let Some(rgb) = self.special_color(slot) {
                    self.report_color(ident, rgb);
                }
                continue;
            }
            let Ok(text) = std::str::from_utf8(value) else {
                continue;
            };
            let Some(rgb) = parse_x_color_spec(text) else {
                continue;
            };
            match slot {
                SpecialColor::Foreground => self.color_overrides.foreground = Some(rgb),
                SpecialColor::Background => self.color_overrides.background = Some(rgb),
                SpecialColor::Cursor => self.color_overrides.cursor = Some(rgb),
            }
        }
    }

    /// `OSC 4 ; index ; spec`, repeated: one palette entry per pair.
    fn palette_colors(&mut self, params: &[&[u8]]) {
        let mut rest = params.iter().skip(1);
        while let (Some(index), Some(value)) = (rest.next(), rest.next()) {
            let Some(index) = std::str::from_utf8(index)
                .ok()
                .and_then(|text| text.parse::<u8>().ok())
            else {
                continue;
            };
            if *value == b"?" {
                if let Some(rgb) = self.color_overrides.ansi.get(&index).copied().or_else(|| {
                    self.colors
                        .as_ref()
                        .map(|colors| colors.ansi[index as usize])
                }) {
                    self.report_color(&format!("4;{index}"), rgb);
                }
                continue;
            }
            if let Some(rgb) = std::str::from_utf8(value).ok().and_then(parse_x_color_spec) {
                self.color_overrides.ansi.insert(index, rgb);
            }
        }
    }

    /// `OSC 104`: forget palette mutations — all of them, or the named ones.
    fn restore_palette(&mut self, params: &[&[u8]]) {
        let named: Vec<u8> = params
            .iter()
            .skip(1)
            .filter(|value| !value.is_empty())
            .filter_map(|value| std::str::from_utf8(value).ok())
            .filter_map(|text| text.parse::<u8>().ok())
            .collect();
        if named.is_empty() {
            self.color_overrides.ansi.clear();
            return;
        }
        for index in named {
            self.color_overrides.ansi.remove(&index);
        }
    }

    /// `CSI ? Ps $ p` / `CSI Ps $ p` — what state is that mode in.
    ///
    /// `0` means "I do not know that mode", and it is the honest answer for
    /// every mode this grid does not implement: claiming `reset` would tell a
    /// program the mode is available and merely off.
    fn report_mode(&mut self, mode: u16, private: bool) {
        // DECRPM's vocabulary: 0 does not know the mode, 1 is set, 2 is reset.
        let known = |on: bool| if on { 1 } else { 2 };
        let state = if private {
            match mode {
                25 => known(self.cursor_visible),
                1000 => known(self.mouse_tracking == MouseTracking::Click),
                1002 => known(self.mouse_tracking == MouseTracking::Drag),
                1003 => known(self.mouse_tracking == MouseTracking::Motion),
                1004 => known(self.focus_reporting),
                1006 => known(self.mouse_sgr),
                1049 => known(self.alt_screen),
                2004 => known(self.bracketed_paste),
                2026 => known(self.sync_began.is_some()),
                2031 => known(self.color_scheme_subscribed),
                _ => 0,
            }
        } else {
            0
        };
        let marker = if private { "?" } else { "" };
        self.replies
            .extend_from_slice(format!("\x1b[{marker}{mode};{state}$y").as_bytes());
    }

    /// How far the view is scrolled back into history. Zero is live.
    #[must_use]
    pub const fn view_offset(&self) -> usize {
        self.view_offset
    }

    /// Move the view through history: positive is back toward older lines,
    /// negative toward the live screen. Clamped to what the scrollback holds.
    ///
    /// The alternate screen has no history — a full-screen program owns its
    /// whole surface — so scrolling there is refused rather than clamped:
    /// coming back from the primary screen's history into a TUI mid-frame is
    /// not a state anyone asked for.
    pub fn scroll_view(&mut self, lines: isize) {
        if self.alt_screen {
            return;
        }
        let held = isize::try_from(self.view_offset).unwrap_or(isize::MAX);
        let cap = isize::try_from(self.scrollback.len()).unwrap_or(isize::MAX);
        let wanted = held.saturating_add(lines).clamp(0, cap);
        let wanted = usize::try_from(wanted).unwrap_or(0);
        if wanted != self.view_offset {
            self.view_offset = wanted;
            // Not a full repaint: `take_delta` reads the moved view against
            // its baseline and answers a view shift when the move is all that
            // happened, or the absolute frame when it is not.
            self.dirty = true;
        }
    }

    /// Snap the view back to the live screen. Typing calls this: a keystroke
    /// is aimed at the program, and the program is at the bottom.
    pub fn view_to_bottom(&mut self) {
        if self.view_offset != 0 {
            self.view_offset = 0;
            self.dirty = true;
            self.full_repaint = true;
        }
    }

    /// One cell, for the renderer. `None` past the edges.
    #[must_use]
    pub fn cell(&self, row: usize, col: usize) -> Option<Cell> {
        self.cells.get(row)?.get(col).copied()
    }

    /// A whole row of cells, for the renderer's per-row diff.
    #[must_use]
    pub fn row_cells(&self, row: usize) -> &[Cell] {
        self.cells.get(row).map_or(&[], Vec::as_slice)
    }

    /// The style the next printed character will carry.
    #[must_use]
    pub const fn pen(&self) -> CellStyle {
        self.pen
    }

    #[must_use]
    pub const fn rows(&self) -> usize {
        self.rows
    }

    #[must_use]
    pub const fn cols(&self) -> usize {
        self.cols
    }

    /// Cursor as `(row, col)`, both zero-based.
    #[must_use]
    pub const fn cursor(&self) -> (usize, usize) {
        (self.cursor_row, self.cursor_col)
    }

    #[must_use]
    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    /// True while the program has bracketed paste enabled — the view must wrap
    /// pasted text in `ESC[200~`/`ESC[201~` or the program sees a paste as
    /// typing.
    #[must_use]
    pub const fn bracketed_paste(&self) -> bool {
        self.bracketed_paste
    }

    /// How much of the mouse the program running here asked to hear about.
    #[must_use]
    pub const fn mouse_tracking(&self) -> MouseTracking {
        self.mouse_tracking
    }

    /// Whether it asked for SGR encoding for those reports.
    #[must_use]
    pub const fn mouse_sgr(&self) -> bool {
        self.mouse_sgr
    }

    #[must_use]
    pub const fn alt_screen(&self) -> bool {
        self.alt_screen
    }

    #[must_use]
    pub const fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn take_dirty(&mut self) -> bool {
        std::mem::replace(&mut self.dirty, false)
    }

    /// Take the latest valid OSC 52 clipboard write, once.
    ///
    /// This side channel is intentionally independent of screen dirtiness and
    /// deltas: clipboard writes from a background terminal must not disappear
    /// merely because its cells are not being sent to the webview.
    pub fn take_osc52_clipboard_write(&mut self) -> Option<String> {
        self.osc52_clipboard_write.take().map(|write| write.0)
    }

    /// Take the title and bell changes not yet announced, without serializing
    /// screen rows or touching what the next delta carries.
    ///
    /// Every terminal still needs to rename its tab and ask for attention at
    /// the moment it happens, whether or not a screen is taking its frames —
    /// so this is asked of every shell every round, and it answers each fact
    /// once. A synchronized frame keeps this news until the frame closes,
    /// just as [`Self::take_delta`] does.
    pub fn take_news(&mut self) -> GridNews {
        if self.holding_frame() {
            return GridNews::default();
        }
        let news = GridNews {
            title: if self.announce_title {
                self.title.clone()
            } else {
                None
            },
            bell: self.announce_bell,
        };
        self.announce_title = false;
        self.announce_bell = false;
        news
    }

    /// What changed since the last call, or `None` when nothing did.
    ///
    /// Independent of [`Self::take_dirty`]: the boolean answers "should I look
    /// at this lane at all", this answers "what do I send". Taking one does not
    /// clear the other, so a caller may use either or both.
    ///
    /// The result obeys the shift contract documented on [`GridDelta`] — read
    /// it before writing a consumer, because the rows that scrolled are
    /// deliberately absent.
    pub fn take_delta(&mut self) -> Option<GridDelta> {
        // The program said this frame is not finished. Handing it over now is
        // what a half-drawn popup looks like.
        if self.holding_frame() {
            return None;
        }
        let full = self.full_repaint;
        let cursor = (self.cursor_row, self.cursor_col);
        /* Everything EXCEPT the row flags, kept apart because the flags lie.
         *
         * `dirty_rows` says somebody wrote, and after the content gate below a
         * write that changed nothing produces no row. Without this split the
         * frame would still go out — empty, but a frame: an emit, a script for
         * the webview to parse, and a pass through `apply` that touches the
         * caret, the rail and the selection. An idle full-screen TUI would pay
         * that sixty-two times a second to say nothing at all. */
        let besides_rows = full
            || self.scrolled_since_take > 0
            || self.title_dirty
            || cursor != self.taken_cursor
            || self.cursor_visible != self.taken_cursor_visible
            || self.modes_dirty
            // A bell rung into a screen that is otherwise still is the whole
            // case this exists for — a build that finished without printing.
            // Leaving it out of this list would mean the news waits for the
            // next unrelated change, which for an idle shell is never.
            || self.bell
            // The scrollbar's two numbers. Compared rather than flagged, for
            // the reason `taken_cursor` is: history grows from several places
            // (a newline, a scroll region, a resize reflow) and a flag at each
            // site is a flag somebody adding the next site forgets. Without
            // this, a shell printing into history while the view sits still
            // moves the thumb only when some other change happens to send a
            // frame — a scrollbar that lags its own scrollback.
            || (self.view_offset, self.scrollback.len()) != self.taken_view;
        let changed = besides_rows || self.dirty_rows.iter().any(|row| *row);
        if !changed {
            return None;
        }

        // A view that only MOVED over unchanged history is a shift of the
        // consumer's rows, not a new screen (the view shift, above): the same
        // rows stand, a few enter at one edge and leave at the other. Only
        // when nothing else moved, and for a move shorter than the screen —
        // anything else is the absolute frame below.
        let moved = isize::try_from(self.taken_view.0).unwrap_or(0)
            - isize::try_from(self.view_offset).unwrap_or(0);
        if !full
            && moved != 0
            && moved.unsigned_abs() < self.rows
            && !self.alt_screen
            && self.scrolled_since_take == 0
            && self.taken_view.1 == self.scrollback.len()
            && !self.dirty_rows.iter().any(|row| *row)
            && !self.fold_table.iter().any(|fold| fold.collapsed)
        {
            let exposed = moved.unsigned_abs();
            let rows: Vec<GridRow> = self
                .view_rows()
                .into_iter()
                .filter(|row| {
                    if moved > 0 {
                        row.index + exposed >= self.rows
                    } else {
                        row.index < exposed
                    }
                })
                .collect();
            let news = self.take_news_unchecked();
            let delta = GridDelta {
                rows,
                row_lines: Vec::new(),
                scrolled_lines: 0,
                view_shift: moved,
                cursor,
                title: news.title,
                alt_screen: false,
                size: (self.rows, self.cols),
                full: false,
                links: self.fresh_links(),
                folds: self.fold_table[self.folds_sent.min(self.fold_table.len())..].to_vec(),
                cursor_visible: self.cursor_visible && self.view_offset == 0,
                mouse_tracking: self.mouse_tracking,
                mouse_sgr: self.mouse_sgr,
                focus_reporting: self.focus_reporting,
                bell: news.bell,
                view_offset: self.view_offset,
                scrollback_len: self.scrollback.len(),
                scrollback_trimmed: self.scrollback_trimmed,
            };
            self.advance_delta_baseline(cursor);
            return Some(delta);
        }

        // Otherwise a scrolled view has no incremental story to tell: row
        // indices name positions in a window over history, and the output
        // shift contract only knows the live screen. Every frame goes out
        // absolute until the view comes back down — and the frame that
        // brings it down is absolute too, when the move was not the shift
        // above: the consumer holds a window over history, and a return that
        // named only the rows output touched (or none, after a fling longer
        // than the screen) left that window on screen under a scrollbar that
        // said live — the pane that "would not come back down" when scrolled
        // up and down (2026-09-17). A view that moved is a new window over
        // the buffer whichever way it went.
        let (scrolled_back, fill) = self.served_window();
        let full = full || scrolled_back || fill > 0 || moved != 0;
        let rows = if scrolled_back {
            self.view_rows()
        } else if fill > 0 {
            self.view_rows_from(fill)
        } else if full {
            self.every_row()
        } else {
            /* Dirty means somebody WROTE here. It does not mean the row looks
             * any different, and for the programs that cost the most it
             * usually does not: a full-screen TUI repaints its whole frame to
             * move one spinner, and every one of those rows arrives here
             * flagged. So the flag opens the question and the content answers
             * it.
             *
             * The comparison is exact rather than a digest. A hash would be
             * cheaper to STORE, but a collision here is a row of the screen
             * that silently never updates again — the worst failure this file
             * can have — and the measurement says it would not even be
             * cheaper to compute: a hash must read every cell, while this
             * stops at the first one that differs. */
            let dirty: Vec<usize> = self
                .dirty_rows
                .iter()
                .enumerate()
                .filter(|(_, dirty)| **dirty)
                .map(|(index, _)| index)
                .collect();
            let mut moved = Vec::with_capacity(dirty.len());
            for index in dirty {
                let wrap = self.wrap_of_screen_row(index);
                let fold = self.folds[index];
                let held = self.sent_rows.get(index).and_then(Option::as_ref);
                if held.is_some_and(|sent| {
                    sent.wrap == wrap && sent.fold == fold && sent.cells == self.cells[index]
                }) {
                    continue;
                }
                let cells = self.cells[index].clone();
                if let Some(slot) = self.sent_rows.get_mut(index) {
                    *slot = Some(SentRow {
                        cells: cells.clone(),
                        wrap,
                        fold,
                    });
                }
                moved.push(GridRow {
                    index,
                    cells,
                    wrap,
                    fold,
                });
            }
            moved
        };
        self.settle_sent_rows(scrolled_back, fill, full);

        // The flags were a false alarm, whole: every row they named held what
        // the consumer already has, and nothing else moved. The baseline still
        // advances — the flags have been answered and must not ask again.
        if rows.is_empty() && !besides_rows {
            self.advance_delta_baseline(cursor);
            return None;
        }

        let news = self.take_news_unchecked();
        let delta = GridDelta {
            rows,
            row_lines: if scrolled_back || fill > 0 {
                self.view_line_numbers(if scrolled_back {
                    self.view_offset
                } else {
                    fill
                })
                .collect()
            } else {
                Vec::new()
            },
            // A full repaint replaces the buffer outright, so a shift on top of
            // it would move rows that are already absolute.
            scrolled_lines: if full { 0 } else { self.scrolled_since_take },
            view_shift: 0,
            cursor: if fill > 0 {
                self.composed_cursor(fill)
            } else {
                cursor
            },
            title: news.title,
            alt_screen: self.alt_screen,
            size: (self.rows, self.cols),
            full,
            // A full frame replaces what the consumer holds, its link map
            // included; an incremental one owes only what is new.
            links: if full {
                self.all_links()
            } else {
                self.fresh_links()
            },
            folds: if full {
                self.fold_table.clone()
            } else {
                self.fold_table[self.folds_sent.min(self.fold_table.len())..].to_vec()
            },
            // History has no caret: the cursor's coordinates name a place on
            // the live screen, which is not what is being shown.
            cursor_visible: self.cursor_visible && !scrolled_back,
            mouse_tracking: self.mouse_tracking,
            mouse_sgr: self.mouse_sgr,
            focus_reporting: self.focus_reporting,
            bell: news.bell,
            view_offset: self.view_offset,
            scrollback_len: self.scrollback.len(),
            scrollback_trimmed: self.scrollback_trimmed,
        };

        self.advance_delta_baseline(cursor);
        Some(delta)
    }

    /// The whole screen as a delta, for a first paint or a reattach.
    ///
    /// Read-only on purpose: taking a snapshot must not swallow a pending
    /// change, or a caller that snapshots and then polls loses whatever
    /// happened in between.
    #[must_use]
    pub fn snapshot(&self) -> GridDelta {
        let (scrolled_back, fill) = self.served_window();
        GridDelta {
            // The same window `take_delta` is serving: a consumer that
            // snapshots while somebody is reading history must not flash the
            // live screen over what they are reading — and a live screen with
            // folded rows is composed the same way there and here.
            rows: if scrolled_back {
                self.view_rows()
            } else if fill > 0 {
                self.view_rows_from(fill)
            } else {
                self.every_row()
            },
            row_lines: if scrolled_back || fill > 0 {
                self.view_line_numbers(if scrolled_back {
                    self.view_offset
                } else {
                    fill
                })
                .collect()
            } else {
                Vec::new()
            },
            scrolled_lines: 0,
            view_shift: 0,
            cursor: if fill > 0 {
                self.composed_cursor(fill)
            } else {
                (self.cursor_row, self.cursor_col)
            },
            title: self.title.clone(),
            alt_screen: self.alt_screen,
            size: (self.rows, self.cols),
            full: true,
            // A snapshot IS the whole screen, so it owes the whole table —
            // and read-only, so it must not mark any of it as delivered.
            links: self.all_links(),
            folds: self.fold_table.clone(),
            cursor_visible: self.cursor_visible && !scrolled_back,
            mouse_tracking: self.mouse_tracking,
            mouse_sgr: self.mouse_sgr,
            focus_reporting: self.focus_reporting,
            // Never on a snapshot. A snapshot is read-only — it cannot clear
            // the flag — so carrying a pending bell here would ring it once for
            // the snapshot and again on the delta that finally takes it. A
            // reattach showing an old screen is also not the moment to announce
            // a bell that rang before anyone was listening.
            bell: false,
            // These two are state, not news, so a snapshot carries them: a
            // window rebuilding its screen from a reattach has to draw the
            // scrollbar it is reattaching to.
            view_offset: self.view_offset,
            scrollback_len: self.scrollback.len(),
            scrollback_trimmed: self.scrollback_trimmed,
        }
    }

    /// Take the whole screen and make it the baseline for the next delta.
    ///
    /// Unlike [`Self::snapshot`], this is a consuming protocol boundary: a
    /// consumer that installs this full frame must see only changes made after
    /// it. In particular, `scrolled_since_take` is already represented in the
    /// full frame and cannot cross again without scrolling the consumer twice.
    pub fn take_snapshot(&mut self) -> GridDelta {
        let snapshot = self.snapshot();
        // The consumer installs this frame, so the content gate's record has
        // to become it too. Left alone, the record still describes the last
        // DELTA — from before a background tab went away — and a row that
        // goes back to that content matches a record the consumer no longer
        // holds, is never sent, and leaves the snapshot's version on screen.
        let (scrolled_back, fill) = self.served_window();
        self.settle_sent_rows(scrolled_back, fill, true);
        let cursor = (self.cursor_row, self.cursor_col);
        self.advance_delta_baseline(cursor);
        snapshot
    }

    /// The delta's own cursor over the bell and the title: what the frame
    /// being built carries, cleared as it is carried. Independent of
    /// [`Self::take_news`], which announces the same facts once each.
    fn take_news_unchecked(&mut self) -> GridNews {
        let news = GridNews {
            title: if self.title_dirty {
                self.title.clone()
            } else {
                None
            },
            bell: self.bell,
        };
        self.title_dirty = false;
        self.bell = false;
        news
    }

    fn advance_delta_baseline(&mut self, cursor: (usize, usize)) {
        self.bell = false;
        self.links_sent = self.links.len();
        self.folds_sent = self.fold_table.len();
        self.taken_view = (self.view_offset, self.scrollback.len());
        self.taken_cursor_visible = self.cursor_visible;
        self.dirty_rows.iter_mut().for_each(|row| *row = false);
        self.scrolled_since_take = 0;
        self.title_dirty = false;
        self.full_repaint = false;
        self.taken_cursor = cursor;
        self.modes_dirty = false;
    }

    /// The tail of the screen, for a surface that shows several shells at once.
    ///
    /// A board card cannot be given a whole screen per agent per beat — that
    /// cost is the reason frames are gated at all — and it does not need one:
    /// what a person reads off eight cards is the last few lines each agent
    /// printed. Read-only for [`TerminalGrid::snapshot`]'s reason, and serving
    /// the same window it does, so a shell somebody has scrolled back in
    /// previews the history they are reading rather than the live screen
    /// underneath it.
    ///
    /// Re-indexed from zero: the surface drawing these is `rows` tall and this
    /// grid is not, so a row that kept its index on a 40-row screen would be
    /// drawn off the bottom of a six-row card.
    #[must_use]
    pub fn preview_rows(&self, rows: usize) -> Vec<GridRow> {
        if rows == 0 {
            return Vec::new();
        }
        let mut shown = if self.view_offset > 0 {
            self.view_rows()
        } else {
            self.every_row()
        };
        // The blank floor under a screen nobody has filled yet is not what
        // the agent is doing. An agent three lines into its first answer sits
        // at the TOP of a forty-row screen, so a plain tail would hand the
        // card six empty rows and call it a preview. The same trim
        // `serialize_tail` makes for the same reason, and it costs a TUI
        // nothing: a full-screen interface has ink on its bottom row.
        while shown
            .last()
            .is_some_and(|row| !row.cells.iter().any(Cell::is_ink))
        {
            shown.pop();
        }
        let cut = shown.len().saturating_sub(rows);
        shown.drain(..cut);
        for (index, row) in shown.iter_mut().enumerate() {
            row.index = index;
        }
        shown
    }

    fn every_row(&self) -> Vec<GridRow> {
        self.cells
            .iter()
            .enumerate()
            .map(|(index, cells)| GridRow {
                index,
                cells: cells.clone(),
                wrap: self.wrap_of_screen_row(index),
                fold: self.folds[index],
            })
            .collect()
    }

    /// The screen-sized window the scrolled view shows: the tail of the
    /// scrollback first, then however much of the live screen still fits
    /// under it. History rows ride out at the logical width they had when
    /// stored; compact default tails are materialised again for the renderer.
    fn view_rows(&self) -> Vec<GridRow> {
        self.view_rows_from(self.view_offset)
    }

    /// Rows of the live screen a collapsed fold hides (a body while it is
    /// closed, a teaser while it is open). The window used to hide those rows
    /// in place, which left the screen that many rows short: an inline TUI
    /// that folds every tool's output — zo — drew its composer with a blank
    /// pane under it, and new output never seemed to reach the bottom
    /// ("zo 출력될 때 자동 스크롤 내려가게", 2026-09-02). The live frame makes
    /// them up from history instead.
    fn live_hidden_rows(&self) -> usize {
        self.folds
            .iter()
            .filter(|fold| self.fold_hidden(**fold))
            .count()
    }

    /// How far into history the live frame reaches to fill the rows its folds
    /// hide: as many lines as it takes to gather that many VISIBLE rows —
    /// history keeps collapsed regions of its own — bounded by what the buffer
    /// holds. Zero when nothing is hidden, nothing is kept, or the alternate
    /// screen is up (it has no history to reach into).
    fn live_fill_offset(&self) -> usize {
        let wanted = self.live_hidden_rows();
        if wanted == 0 || self.alt_screen {
            return 0;
        }
        let mut gathered = 0;
        let mut offset = 0;
        while gathered < wanted && offset < self.scrollback.len() {
            offset += 1;
            let at = self.scrollback.len() - offset;
            if !self.fold_hidden(self.scroll_folds.get(at).copied().flatten()) {
                gathered += 1;
            }
        }
        offset
    }

    /// Where the live cursor lands in a frame composed with `fill` lines of
    /// history above the screen and the screen's hidden rows taken out.
    fn composed_cursor(&self, fill: usize) -> (usize, usize) {
        let start = self.scrollback.len().saturating_sub(fill);
        let shown_history = (start..self.scrollback.len())
            .filter(|at| !self.fold_hidden(self.scroll_folds.get(*at).copied().flatten()))
            .count();
        let hidden_above = self.folds[..self.cursor_row.min(self.rows)]
            .iter()
            .filter(|fold| self.fold_hidden(**fold))
            .count();
        let row = (shown_history + self.cursor_row - hidden_above).min(self.rows.saturating_sub(1));
        (row, self.cursor_col)
    }

    /// The screen-sized window that starts `offset` lines up in history: the
    /// tail of the scrollback first, then however much of the live screen
    /// still fits under it.
    fn view_rows_from(&self, offset: usize) -> Vec<GridRow> {
        self.view_line_numbers(offset)
            .enumerate()
            .map(|(index, line)| {
                let cells = if line < self.scrollback.len() {
                    self.scrollback_cells(line)
                        .map(Cow::into_owned)
                        .unwrap_or_default()
                } else {
                    self.cells[line - self.scrollback.len()].clone()
                };
                GridRow {
                    index,
                    cells,
                    wrap: self.wrapped_from_previous(line),
                    fold: self.fold_at(line),
                }
            })
            .collect()
    }

    /// The same walk supplies both composed cells and their selection
    /// coordinates. Keeping the filter here prevents a hidden fold body from
    /// making the copied range name a different row than the painted one.
    fn view_line_numbers(&self, offset: usize) -> impl Iterator<Item = usize> + '_ {
        let start = self.scrollback.len() - offset.min(self.scrollback.len());
        (start..self.scrollback.len() + self.rows)
            .filter(|line| !self.fold_hidden(self.fold_at(*line)))
            .take(self.rows)
    }

    /// Whether a row is the body of a region the reader has collapsed.
    fn fold_hidden(&self, fold: Option<RowFold>) -> bool {
        fold.is_some_and(|fold| {
            let collapsed = self
                .fold_table
                .iter()
                .any(|meta| meta.id == fold.id && meta.collapsed);
            match fold.role {
                FoldRole::Header => false,
                FoldRole::Body => collapsed,
                FoldRole::Teaser => !collapsed,
            }
        })
    }

    /// Open or close a fold region on the reader's behalf.
    ///
    /// The window keeps the same answer for its live-screen hiding; here it
    /// steers what a scrolled frame carries. Unknown ids are refused so a
    /// stale handle cannot invent a region. A real change produces a frame.
    pub fn set_fold_collapsed(&mut self, id: u32, collapsed: bool) -> bool {
        let Some(meta) = self.fold_table.iter_mut().find(|meta| meta.id == id) else {
            return false;
        };
        if meta.collapsed != collapsed {
            meta.collapsed = collapsed;
            self.dirty = true;
            self.full_repaint = true;
        }
        true
    }

    fn mark_row(&mut self, row: usize) {
        if let Some(flag) = self.dirty_rows.get_mut(row) {
            *flag = true;
        }
    }

    fn mark_all_rows(&mut self) {
        self.dirty_rows.iter_mut().for_each(|row| *row = true);
    }

    /// Which window over the buffer a frame serves right now: whether somebody
    /// is reading history, and otherwise how many rows a collapsed fold hides
    /// from the live screen. Either makes the frame composed, and a composed
    /// frame is absolute. `take_delta`, `snapshot` and `take_snapshot` ask the
    /// same question and must get the same answer.
    fn served_window(&self) -> (bool, usize) {
        let scrolled_back = self.view_offset > 0;
        // A live screen with rows a collapsed fold hides is composed too: the
        // hidden rows are made up from history so the bottom stays at the
        // bottom (see `live_fill_offset`).
        let fill = if scrolled_back {
            0
        } else {
            self.live_fill_offset()
        };
        (scrolled_back, fill)
    }

    /// What the consumer holds once a frame has gone out, for the next frame's
    /// content gate to compare against.
    ///
    /// A view of history hands it rows that are not the live screen at all
    /// (`scrolled_back`, `fill`), so the ledger cannot describe them and must
    /// say it knows nothing rather than guess. A plain full frame — a delta's
    /// or a snapshot's — hands over exactly the screen, so the ledger is the
    /// screen. An incremental frame keeps the ledger itself, row by row.
    fn settle_sent_rows(&mut self, scrolled_back: bool, fill: usize, full: bool) {
        if scrolled_back || fill > 0 {
            self.forget_sent_rows();
        } else if full {
            self.remember_whole_screen();
        }
    }

    /// The consumer holds something this grid cannot name — a window into
    /// history, or a screen from before a resize. Every row is owed afresh.
    fn forget_sent_rows(&mut self) {
        self.sent_rows.clear();
        self.sent_rows.resize(self.rows, None);
    }

    /// The consumer holds exactly the live screen, which is what a full frame
    /// hands it. Rare — a resize, an alternate screen crossing, a background
    /// tab coming back — so the whole screen's worth of copying here is paid
    /// about as often as a person drags a window edge or switches a tab.
    fn remember_whole_screen(&mut self) {
        self.sent_rows.clear();
        self.sent_rows.reserve(self.rows);
        for index in 0..self.rows {
            self.sent_rows.push(Some(SentRow {
                cells: self.cells[index].clone(),
                wrap: self.wrap_of_screen_row(index),
                fold: self.folds[index],
            }));
        }
    }

    #[must_use]
    pub fn scrollback_len(&self) -> usize {
        self.scrollback.len()
    }

    /// One visible row, trailing blanks trimmed.
    #[must_use]
    pub fn line(&self, row: usize) -> String {
        self.cells.get(row).map_or_else(String::new, |cells| {
            let text: String = cells
                .iter()
                .filter(|cell| !cell.is_continuation())
                .flat_map(|cell| std::iter::once(cell.ch).chain(cell.zw.scalars()))
                .collect();
            text.trim_end().to_string()
        })
    }

    /// Every visible row joined by newlines, trailing blank rows trimmed.
    #[must_use]
    pub fn visible_text(&self) -> String {
        let mut lines: Vec<String> = (0..self.rows).map(|row| self.line(row)).collect();
        while lines.last().is_some_and(String::is_empty) {
            lines.pop();
        }
        lines.join("\n")
    }

    /// A scrollback row's logical cells, oldest first — styles and all.
    ///
    /// Storage stops at the last non-default cell; this accessor restores the
    /// default tail to the width the row had when it left the screen. A full
    /// row stays borrowed, while a compact row owns only this read's padding.
    /// `scrollback_line` below can read compact storage directly because it
    /// trims the same default tail from its text answer.
    #[must_use]
    pub fn scrollback_cells(&self, index: usize) -> Option<Cow<'_, [Cell]>> {
        let cells = self.scrollback.get(index)?;
        let cols = self.scroll_cols.get(index).copied().unwrap_or(cells.len());
        if cells.len() >= cols {
            return Some(Cow::Borrowed(&cells[..cols]));
        }
        let mut padded = Vec::with_capacity(cols);
        padded.extend_from_slice(cells);
        padded.resize(cols, Cell::default());
        Some(Cow::Owned(padded))
    }

    /// The compact representation, for readers whose own contract discards
    /// trailing default cells. Session serialization is one: `render_row`
    /// stops at the last ink, so materialising up to 50,000 padded rows first
    /// would spend the memory this representation exists to save.
    pub(crate) fn stored_scrollback_cells(&self, index: usize) -> Option<&[Cell]> {
        self.scrollback.get(index).map(Vec::as_slice)
    }

    /// How many rows the visible screen holds.
    #[must_use]
    pub const fn screen_rows(&self) -> usize {
        self.rows
    }

    /// A scrollback row, oldest first.
    #[must_use]
    pub fn scrollback_line(&self, index: usize) -> String {
        self.scrollback
            .get(index)
            .map_or_else(String::new, |cells| {
                let text: String = cells
                    .iter()
                    .filter(|cell| !cell.is_continuation())
                    .flat_map(|cell| std::iter::once(cell.ch).chain(cell.zw.scalars()))
                    .collect();
                text.trim_end().to_string()
            })
    }

    /// How many lines of history to keep. Clamped to the range a person can
    /// actually choose from, because the value arrives from a settings file
    /// anyone can edit.
    ///
    /// Shrinking drops the OLDEST lines, which is the only answer that keeps
    /// what is on screen and nearest to it — a person lowering this is trying
    /// to spend less memory, not to throw away the last thing they read.
    pub fn set_scrollback_cap(&mut self, lines: usize) {
        let cap = lines.clamp(MIN_SCROLLBACK_LINES, MAX_SCROLLBACK_LINES);
        self.scrollback_cap = cap;
        while self.scrollback.len() > cap {
            drop(self.pop_scrollback());
            // The view is anchored to CONTENT, so history falling off the back
            // pulls it with it — otherwise the offset names a line that is no
            // longer there and the screen jumps somewhere nobody asked for.
            self.view_offset = self.view_offset.saturating_sub(1);
        }
    }

    /// How many lines of history are being kept.
    #[must_use]
    pub const fn scrollback_cap(&self) -> usize {
        self.scrollback_cap
    }

    /// Every place `needle` occurs, oldest line first.
    ///
    /// Over history AND the live screen, in one coordinate space — see
    /// [`TermHit`]. The scrollback is the whole point: the thing somebody is
    /// looking for in a terminal is almost never on the screen in front of
    /// them, which is why a search that could only see the screen would be a
    /// search nobody would use.
    ///
    /// Literal, not a pattern. A regular expression would mean a new
    /// dependency, and this window does not add one for a feature that reads
    /// perfectly well without it — the audit's own note. Case folding is the
    /// one option, because it is the one people actually reach for.
    ///
    /// Columns, not byte offsets. A row is cells and a cell is a column: the
    /// caller's next move is to draw a highlight over the match, and a byte
    /// offset into a line with a Korean word in it names the wrong place.
    #[must_use]
    pub fn search(&self, needle: &str, case_sensitive: bool, cap: usize) -> Vec<TermHit> {
        let mut found = Vec::new();
        if needle.is_empty() {
            return found;
        }
        let wanted: Vec<char> = if case_sensitive {
            needle.chars().collect()
        } else {
            needle.to_lowercase().chars().collect()
        };
        let total = self.scrollback.len() + self.rows;
        let mut at = 0;
        while at < total && found.len() < cap {
            let (flat, next) = self.flatten_logical(at, case_sensitive);
            self.scan_flat(&flat, &wanted, cap, &mut found);
            at = next;
        }
        found
    }

    /// Every hit for a pattern, oldest line first, at most `cap` of them —
    /// the search bar's other toggle (the original hands the query to a JS
    /// `RegExp`, `TerminalSearch.tsx:34,46`).
    ///
    /// A pattern that does not parse is the person still typing it — `(` on
    /// its way to `(foo)` — so it is an [`Err`] for the bar to tint, never a
    /// panic and never silently zero hits. A match of empty width (every
    /// position, for `x*`) is skipped: it names no cells, so there is nothing
    /// to highlight and nothing to jump to.
    ///
    /// Case folds through the engine's own `(?i)`, which does Unicode simple
    /// folding — the literal path above folds one scalar per cell instead.
    /// The engine is the more correct of the two; the literal path keeps its
    /// cheaper fold because a cell can only hold one scalar anyway.
    pub fn search_regex(
        &self,
        pattern: &str,
        case_sensitive: bool,
        cap: usize,
    ) -> Result<Vec<TermHit>, BadPattern> {
        let mut found = Vec::new();
        if pattern.is_empty() {
            return Ok(found);
        }
        let engine = regex::RegexBuilder::new(pattern)
            .case_insensitive(!case_sensitive)
            .build()
            .map_err(|_| BadPattern)?;
        let total = self.scrollback.len() + self.rows;
        let mut at = 0;
        while at < total && found.len() < cap {
            let (flat, next) = self.flatten_logical(at, true);
            let mut text = String::with_capacity(flat.chars.len());
            // Byte offset where each flattened char begins, index-aligned
            // with `flat.chars` — the engine speaks byte ranges and the
            // highlight speaks columns.
            let mut bytes: Vec<usize> = Vec::with_capacity(flat.chars.len());
            for entry in &flat.chars {
                bytes.push(text.len());
                text.push(entry.ch);
            }
            for hit in engine.find_iter(&text) {
                if hit.start() == hit.end() {
                    // Empty width names no cells: nothing to highlight,
                    // nothing to jump to.
                    continue;
                }
                let Some(first) = bytes.iter().position(|byte| *byte == hit.start()) else {
                    continue;
                };
                let upto = bytes
                    .iter()
                    .position(|byte| *byte >= hit.end())
                    .unwrap_or(flat.chars.len());
                found.push(self.flat_hit(&flat, first, upto));
                if found.len() >= cap {
                    break;
                }
            }
            at = next;
        }
        Ok(found)
    }

    /// The soft-wrap flag of a line in [`TermHit`] coordinates: history
    /// first, then the screen.
    fn wrapped_from_previous(&self, line: usize) -> Option<u16> {
        let history = self.scrollback.len();
        if line < history {
            self.scroll_wrap.get(line).copied().flatten()
        } else {
            self.wrap.get(line - history).copied().flatten()
        }
    }

    /// One live-screen row's wrap ledger, in the flattener's coordinate space.
    ///
    /// Named rather than inlined at the three delta sites because the
    /// translation from a screen index to a [`TermHit`] line is the part a
    /// fourth site would get wrong, and getting it wrong is silent: the row
    /// would simply carry somebody else's number.
    fn wrap_of_screen_row(&self, index: usize) -> Option<u16> {
        self.wrapped_from_previous(self.scrollback.len() + index)
    }

    /// A line's logical width, in the same coordinate space.
    fn logical_width_at(&self, line: usize) -> usize {
        let history = self.scrollback.len();
        if line < history {
            self.scroll_cols
                .get(line)
                .copied()
                .unwrap_or(self.scrollback[line].len())
        } else {
            self.cells.get(line - history).map_or(0, Vec::len)
        }
    }

    /// One logical cell without materialising a compact row's default tail.
    fn logical_cell_at(&self, line: usize, col: usize) -> Option<Cell> {
        let history = self.scrollback.len();
        if line < history {
            if col >= self.logical_width_at(line) {
                return None;
            }
            Some(self.scrollback[line].get(col).copied().unwrap_or_default())
        } else {
            self.cells.get(line - history)?.get(col).copied()
        }
    }

    /// One LOGICAL line, flattened: the visual rows a program's single line
    /// was wrapped across, joined back into the character sequence it
    /// printed. This is what both searches walk — the original searches the
    /// unwrapped line too (xterm joins wrapped rows before matching), and a
    /// needle split by the right edge of the pane is otherwise unfindable.
    ///
    /// Every row but the last contributes only the columns the ledger says
    /// it carried ([`Self::wrap`]) — which is how the cell a wide glyph's
    /// early wrap never wrote stays out of the joined text instead of
    /// standing in it as a space nobody printed.
    fn flatten_logical(&self, start: usize, keep_case: bool) -> (FlatLine, usize) {
        let total = self.scrollback.len() + self.rows;
        let mut flat = FlatLine::default();
        let mut line = start;
        loop {
            let next = line + 1;
            let carried = if next < total {
                self.wrapped_from_previous(next)
            } else {
                None
            };
            let width = self.logical_width_at(line);
            let take = carried.map_or(width, |k| (k as usize).min(width));
            for col in 0..take {
                let Some(cell) = self.logical_cell_at(line, col) else {
                    continue;
                };
                if cell.is_continuation() {
                    continue;
                }
                let ch = if keep_case {
                    cell.ch
                } else {
                    // `to_lowercase` yields a sequence — `İ` folds to two
                    // scalars. The first is the one that lines up with a
                    // needle folded the same way, and taking it keeps this a
                    // one-cell-one-char scan.
                    cell.ch.to_lowercase().next().unwrap_or(cell.ch)
                };
                flat.chars.push(FlatChar { ch, line, col });
            }
            if carried.is_none() {
                return (flat, next);
            }
            line = next;
        }
    }

    /// The flattened characters, matched as a window of literal chars.
    fn scan_flat(&self, flat: &FlatLine, wanted: &[char], cap: usize, found: &mut Vec<TermHit>) {
        let chars = &flat.chars;
        if chars.len() < wanted.len() {
            return;
        }
        for at in 0..=chars.len() - wanted.len() {
            if !(0..wanted.len()).all(|step| chars[at + step].ch == wanted[step]) {
                continue;
            }
            found.push(self.flat_hit(flat, at, at + wanted.len()));
            if found.len() >= cap {
                return;
            }
        }
    }

    /// A [`TermHit`] for flattened chars `[start, upto)`.
    ///
    /// The hit names the row the match STARTS on. A match that runs past
    /// that row's edge is clipped to the edge — the jump lands right, and a
    /// highlight that stops at the fold is the honest drawing of a match the
    /// fold split. Within one row the rule is unchanged: the column after
    /// the last matched cell, so a match ending in a wide glyph covers both
    /// of its columns.
    fn flat_hit(&self, flat: &FlatLine, start: usize, upto: usize) -> TermHit {
        let first = &flat.chars[start];
        let row_len = self.logical_width_at(first.line);
        let last_on_row = flat.chars[start..upto]
            .iter()
            .take_while(|entry| entry.line == first.line)
            .last()
            .map_or(first.col, |entry| entry.col);
        let crosses = flat
            .chars
            .get(upto)
            .is_none_or(|next| next.line != first.line)
            && flat.chars[start..upto]
                .last()
                .is_some_and(|entry| entry.line != first.line);
        let end = if crosses {
            row_len
        } else {
            flat.chars
                .get(upto)
                .filter(|next| next.line == first.line)
                .map_or(row_len.min(last_on_row + 2), |next| next.col)
        };
        TermHit {
            line: first.line,
            col: first.col,
            len: end.saturating_sub(first.col).max(1),
        }
    }

    /// The fold a line belongs to, in the same coordinate space — history's
    /// ledger for a history line, the screen's for a screen row.
    fn fold_at(&self, line: usize) -> Option<RowFold> {
        let history = self.scrollback.len();
        if line < history {
            self.scroll_folds.get(line).copied().flatten()
        } else {
            self.folds.get(line - history).copied().flatten()
        }
    }

    /// The text between two points, for the window's selection to copy.
    ///
    /// `from` is inclusive and `to` exclusive, both `(line, col)` in
    /// [`TermHit`] coordinates. The window paints only the rows it can see,
    /// so a selection dragged past the top of the pane names lines it does
    /// not hold; they are all here, and so are the rules of a copied line,
    /// kept beside the cells they are about: every row contributes its cells
    /// between the two columns that apply to it (`from`'s on the first row,
    /// `to`'s on the last, everything in between), a continuation cell is
    /// skipped and a wide glyph is whole when the span starts on its second
    /// column, a combining mark rides its base out, trailing blanks go — a
    /// row is padded to full width and the padding is nobody's — and rows end
    /// in a newline whether or not the program soft-wrapped them, because a
    /// screen is a grid of rows and rows are what the person dragged over.
    /// The body rows of a collapsed fold are not on that screen and are not
    /// in the copy.
    #[must_use]
    pub fn text_between(&self, from: (usize, usize), to: (usize, usize)) -> String {
        let total = self.scrollback.len() + self.rows;
        if from >= to || from.0 >= total {
            return String::new();
        }
        let last = to.0.min(total - 1);
        let mut lines: Vec<String> = Vec::new();
        for line in from.0..=last {
            if self.fold_hidden(self.fold_at(line)) {
                continue;
            }
            let width = self.logical_width_at(line);
            let mut start = if line == from.0 { from.1.min(width) } else { 0 };
            let end = if line == to.0 { to.1.min(width) } else { width };
            // Starting on the second column of a wide glyph means the glyph:
            // the boundary fell in the middle of it, and half a syllable is
            // not a thing a person can have meant.
            if start > 0
                && start < width
                && self
                    .logical_cell_at(line, start)
                    .is_some_and(|cell| cell.is_continuation())
            {
                start -= 1;
            }
            let mut text = String::new();
            for col in start..end {
                let Some(cell) = self.logical_cell_at(line, col) else {
                    continue;
                };
                if cell.is_continuation() {
                    continue;
                }
                text.push(cell.ch);
                text.extend(cell.zw.scalars());
            }
            let kept = text.trim_end().len();
            text.truncate(kept);
            lines.push(text);
        }
        lines.join("\n")
    }

    /// Put `line` (in [`TermHit`] coordinates) on screen, and answer where the
    /// view ended up.
    ///
    /// The caller could compute this from `view_offset` and `scrollback_len` —
    /// and would then own a copy of the clamping rules, which is the second
    /// place they drift apart. A hit already on screen moves nothing.
    pub fn view_to_line(&mut self, line: usize) -> usize {
        let history = self.scrollback.len();
        let top = history.saturating_sub(self.view_offset);
        if line >= top && line < top + self.rows {
            return self.view_offset;
        }
        // A third of a screen of lead-in, so a hit does not land on the very
        // top row with everything that explains it still out of sight.
        let lead = self.rows / 3;
        let wanted = history.saturating_sub(line.saturating_sub(lead));
        let wanted = wanted.min(history);
        if wanted != self.view_offset {
            self.view_offset = wanted;
            self.dirty = true;
            self.full_repaint = true;
        }
        self.view_offset
    }

    /// Should the window draw a caret?
    #[must_use]
    pub const fn cursor_visible(&self) -> bool {
        self.cursor_visible
    }

    /// How many times the program has *shown* its cursor (`CSI ?25h`),
    /// cumulatively.
    ///
    /// A counter rather than the level above, for the reader that needs the
    /// edge: prompt delivery treats "showed the cursor after the bracketed
    /// paste handshake" as an agent saying its input line is drawn, and two
    /// reads of a level cannot see a hide-and-show that happened between
    /// them. Wraps at `u64::MAX`, which no real program reaches.
    #[must_use]
    pub const fn cursor_shows(&self) -> u64 {
        self.cursor_shows
    }

    /// Watch for one glyph in what gets printed, and take whether the watch
    /// caught it.
    ///
    /// The other half of the question [`Self::cursor_shows`] answers, for the
    /// agents that say their input line is drawn by printing a character on it
    /// (Codex's `›`) rather than by showing a caret. Both are **edges**: this
    /// reports that a cell *became* the glyph since the last ask, never that
    /// one still holds it. A marker left on screen by the previous run is
    /// scrollback, and a wait satisfied by scrollback is a prompt typed into a
    /// program that has not drawn its input line yet.
    ///
    /// Taking is what makes it an edge, so the reader has to ask **every
    /// round**, `None` included — a round left untaken keeps its answer, and
    /// the next reader is then handed an edge from a round it did not live
    /// through.
    ///
    /// `watch` is the glyph to notice from here on, and the answer is about
    /// the glyph that was being watched during the round just ended. So the
    /// first ask after a watch is armed is always `false`: nothing was
    /// noticing during that round, and an answer about an unwatched round
    /// would be exactly the guess this is built to refuse.
    pub fn take_glyph_drawn(&mut self, watch: Option<char>) -> GlyphDrawn {
        self.watched_glyph = watch;
        GlyphDrawn {
            anywhere: std::mem::replace(&mut self.glyph_drawn, false),
            in_alt_screen: std::mem::replace(&mut self.glyph_drawn_in_alt, false),
        }
    }

    /// Does the program want to hear about focus changes?
    #[must_use]
    pub const fn focus_reporting(&self) -> bool {
        self.focus_reporting
    }

    /// How long an unclosed atomic frame is honoured before the screen is
    /// handed over anyway. Long enough for a real repaint, short enough that a
    /// program which forgot to close one is not felt as a freeze.
    const SYNC_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(150);

    /// Is the program mid-way through a frame it asked to be shown whole?
    ///
    /// The readers ask too: a snapshot taken now would be the half-drawn
    /// screen the hold exists to keep from view.
    pub(crate) fn holding_frame(&self) -> bool {
        self.sync_began
            .is_some_and(|began| began.elapsed() < Self::SYNC_TIMEOUT)
    }

    /// Give up on an unclosed frame, for the test that proves the hold is not
    /// a lock.
    #[doc(hidden)]
    pub fn expire_sync_for_test(&mut self) {
        self.sync_began = Some(std::time::Instant::now() - Self::SYNC_TIMEOUT * 2);
    }

    /// Take what the terminal owes the program, and forget it.
    ///
    /// Drained rather than read, because an answer sent twice is read as a
    /// second, stray reply to a question nobody asked.
    pub fn take_replies(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.replies)
    }

    /// The rows scrolling moves, as `CSI r` set them.
    fn region(&self) -> (usize, usize) {
        self.scroll_region
            .unwrap_or((0, self.rows.saturating_sub(1)))
    }

    /// Resize the screen. Rows that no longer fit scroll into scrollback rather
    /// than vanishing, which is what a person expects when they drag a window
    /// narrower and then wider again.
    pub fn resize(&mut self, rows: usize, cols: usize) {
        // Rows are re-fitted and reflowed, so a held placement is stale.
        self.end_grapheme();
        let rows = rows.max(1);
        let cols = cols.max(1);
        if rows == self.rows && cols == self.cols {
            return;
        }

        for line in &mut self.cells {
            refit_line(line, cols);
        }
        while self.cells.len() > rows {
            let evicted = self.cells.remove(0);
            let evicted_wrap = self.wrap.remove(0);
            let evicted_fold = self.folds.remove(0);
            // A resize is a rare, human-paced event: the displaced line's
            // allocation is not worth carrying here.
            drop(self.push_scrollback(evicted, evicted_wrap, evicted_fold));
            self.cursor_row = self.cursor_row.saturating_sub(1);
        }
        while self.cells.len() < rows {
            self.cells.push(vec![Cell::default(); cols]);
            self.wrap.push(None);
            self.folds.push(None);
        }

        self.rows = rows;
        self.cols = cols;
        self.cursor_row = self.cursor_row.min(rows - 1);
        self.cursor_col = self.cursor_col.min(cols - 1);
        self.dirty = true;
        // Row *n* now means something else, so no accumulated shift can bridge
        // this. The consumer has to take the next delta as absolute.
        self.dirty_rows.resize(rows, true);
        // Row *n* means something else now, so nothing the consumer holds can
        // be recognised — every row is owed afresh.
        self.sent_rows.clear();
        self.sent_rows.resize(rows, None);
        self.mark_all_rows();
        self.scrolled_since_take = 0;
        self.full_repaint = true;
        // The window over history was sized in the old geometry; a resize
        // lands the view back on the live screen rather than somewhere the
        // arithmetic can no longer name.
        self.view_offset = 0;
    }

    /// Returns the offered full-width line so the caller can reuse it for the
    /// fresh screen row. History has its own compact allocation pool: once the
    /// cap is full, the oldest compact buffer is filled with the new prefix.
    /// Repeated same-width transcript lines therefore return to zero allocation
    /// traffic at steady state without retaining screen-wide history buffers.
    fn push_scrollback(
        &mut self,
        line: Vec<Cell>,
        wrapped: Option<u16>,
        fold: Option<RowFold>,
    ) -> Option<Vec<Cell>> {
        if self.scrollback_cap == 0 {
            return Some(line);
        }
        let logical_cols = line.len();
        let mut stored = if self.scrollback.len() == self.scrollback_cap {
            self.pop_scrollback().unwrap_or_default()
        } else {
            Vec::new()
        };
        let used = line
            .iter()
            .rposition(|cell| *cell != Cell::default())
            .map_or(0, |at| at + 1);
        stored.clear();
        stored.extend_from_slice(&line[..used]);
        stored.truncate(used);
        stored.shrink_to_fit();
        self.scrollback.push_back(stored);
        self.scroll_wrap.push_back(wrapped);
        self.scroll_folds.push_back(fold);
        self.scroll_cols.push_back(logical_cols);
        Some(line)
    }

    /// Retire one buffer coordinate along with its row and metadata. Both
    /// steady-state recycling and a smaller history cap use this boundary.
    fn pop_scrollback(&mut self) -> Option<Vec<Cell>> {
        let line = self.scrollback.pop_front()?;
        self.scroll_wrap.pop_front();
        self.scroll_folds.pop_front();
        self.scroll_cols.pop_front();
        self.scrollback_trimmed = self.scrollback_trimmed.saturating_add(1);
        Some(line)
    }

    /// A blank row ready to become the screen's new bottom line, cut from a
    /// displaced line's allocation when one is available.
    fn recycled_row(&self, displaced: Option<Vec<Cell>>) -> Vec<Cell> {
        let mut row = displaced.unwrap_or_default();
        row.clear();
        row.resize(self.cols, Cell::default());
        row
    }

    fn scroll_up(&mut self) {
        // A region is how a TUI reserves a header or a status line: rows
        // outside it must not move. What decides whether the line that leaves
        // is KEPT is where that region begins — at the top of the screen the
        // line is going off the screen, and anywhere below it the line was
        // overwritten inside the screen and there is nothing to keep. That is
        // xterm's own rule, and reading it as "only when no region is set at
        // all" is how an agent's whole transcript went nowhere: `ratatui`'s
        // `insert_before` — how codex puts a finished turn above its composer
        // — sets a region from row one down to the top of its live area and
        // scrolls THAT, so every line it retired was discarded and scrolling
        // back found an empty history (reported live, twice).
        let (top, bottom) = self.region();
        let bottom = bottom.min(self.rows.saturating_sub(1));
        // And the alternate screen keeps nothing: a full-screen program owns
        // its surface, and its frames are not the history behind it.
        if top > 0 || self.alt_screen {
            self.rotate_region_up();
            return;
        }
        let evicted = self.cells.remove(0);
        let evicted_wrap = self.wrap.remove(0);
        let evicted_fold = self.folds.remove(0);
        let recyclable = self.push_scrollback(evicted, evicted_wrap, evicted_fold);
        let fresh = self.recycled_row(recyclable);
        self.cells.insert(bottom, fresh);
        self.wrap.insert(bottom, None);
        self.folds.insert(bottom, None);
        // A reader back in history stays on the CONTENT they were reading:
        // the line that just entered the scrollback pushes their offset one
        // deeper, up to what the buffer can hold. Without this, a program
        // that keeps printing drags the view toward live at its own pace.
        if self.view_offset > 0 {
            self.view_offset = (self.view_offset + 1).min(self.scrollback.len());
        }
        // The dirty flags move with the rows they describe: a row that was
        // pending and slid up is still pending at its new index, and dropping
        // that would silently lose its content. The freshly exposed bottom row
        // is blank and has to be sent as blanks.
        self.dirty_rows.remove(0);
        self.dirty_rows.insert(bottom, true);
        if bottom + 1 == self.rows {
            // The whole screen moved, so the window can shift its own rows.
            self.scrolled_since_take += 1;
            /* And the ledger of what the consumer holds moves exactly where
             * the consumer does — here, and nowhere else.
             *
             * It belongs INSIDE this branch and not beside the dirty flags
             * above, which was the first spelling and was wrong: a REGION
             * scroll moves the grid's rows without telling the window
             * anything (the `else` below says why), so the window's row *i*
             * is still the row it was. A ledger that shifted there would
             * claim the consumer holds a row it has never seen, and the
             * content gate in `take_delta` would then skip sending it —
             * twelve rows of a scrolling screen stayed blank forever
             * (`delta_replay`, both consumer models). Left alone, those rows
             * are compared against what the consumer really has, differ, and
             * are sent. */
            if !self.sent_rows.is_empty() {
                self.sent_rows.remove(0);
                self.sent_rows
                    .insert(bottom.min(self.sent_rows.len()), None);
            }
        } else {
            // Only the region moved. Claiming a shift would make the window
            // slide the rows BELOW it too — rows that never moved — so the
            // region is resent instead. Costlier by exactly the rows that
            // changed, and correct.
            for row in top..=bottom {
                self.mark_row(row);
            }
        }
    }

    fn newline(&mut self) {
        let (_, bottom) = self.region();
        if self.cursor_row >= bottom {
            self.scroll_up();
        } else {
            self.cursor_row += 1;
        }
    }

    /// Move up a row, scrolling the region backwards at its top — `ESC M`.
    fn reverse_index(&mut self) {
        let (top, _) = self.region();
        if self.cursor_row > top {
            self.cursor_row -= 1;
            return;
        }
        self.rotate_region_down();
    }

    /// Move every row in the scrolling region up one, blanking the row that
    /// opens at the bottom. Nothing leaves the screen — see [`Self::scroll_up`]
    /// for the full-screen case, which is the only one that feeds scrollback.
    fn rotate_region_up(&mut self) {
        let (top, bottom) = self.region();
        let bottom = bottom.min(self.rows.saturating_sub(1));
        if top >= bottom {
            return;
        }
        let evicted = self.cells.remove(top);
        let fresh = self.recycled_row(Some(evicted));
        self.cells.insert(bottom, fresh);
        // A region rotation reshuffles which row sits above which; the
        // ledger's claims inside it are about neighbours that moved, so
        // they are withdrawn rather than left pointing at strangers.
        self.wrap.remove(top);
        self.wrap.insert(bottom, None);
        for flag in &mut self.wrap[top..=bottom] {
            *flag = None;
        }
        self.folds.remove(top);
        self.folds.insert(bottom, None);
        for row in top..=bottom {
            self.mark_row(row);
        }
    }

    /// The same rotation the other way: rows move down and the region's top
    /// row opens blank. History never comes back up — a line that left the
    /// screen is in the scrollback, which is a reader's, not a program's.
    fn rotate_region_down(&mut self) {
        let (top, bottom) = self.region();
        let bottom = bottom.min(self.rows.saturating_sub(1));
        if top >= bottom {
            return;
        }
        let evicted = self.cells.remove(bottom);
        let fresh = self.recycled_row(Some(evicted));
        self.cells.insert(top, fresh);
        self.wrap.remove(bottom);
        self.wrap.insert(top, None);
        // Same withdrawal as the forward rotation: rearranged neighbours
        // make every claim inside the span stale.
        for flag in &mut self.wrap[top..=bottom] {
            *flag = None;
        }
        self.folds.remove(bottom);
        self.folds.insert(top, None);
        for row in top..=bottom {
            self.mark_row(row);
        }
    }

    /// `CSI Ps S` / `CSI Ps T` — scroll the region by whole lines.
    ///
    /// This is how a program that keeps a transcript ABOVE its own live area
    /// makes room for one more entry: set the region, scroll it, print into
    /// the space that opened. `ratatui`'s `insert_before` is exactly that, and
    /// it is how `codex` puts a finished turn behind its composer — so a
    /// terminal that swallows these two shows the composer and loses every
    /// turn that came before it, with nothing in the scrollback to scroll
    /// back to (reported live: "이전 대화를 볼 수 없다").
    fn scroll_lines(&mut self, count: usize, up: bool) {
        self.end_grapheme();
        // Bounded by the region's own height: a program asking for a thousand
        // lines of a twenty-row region means "clear it", and it costs one pass.
        let (top, bottom) = self.region();
        let span = bottom.min(self.rows.saturating_sub(1)).saturating_sub(top) + 1;
        for _ in 0..count.min(span) {
            if up {
                self.scroll_up();
            } else {
                self.rotate_region_down();
            }
        }
        self.dirty = true;
    }

    /// `CSI Ps L` / `CSI Ps M` — open or close whole lines AT THE CURSOR.
    ///
    /// The region below the cursor shifts; the rows above it and outside the
    /// scrolling region never move. Nothing reaches the scrollback: what
    /// leaves the bottom of the region was overwritten inside the screen, not
    /// scrolled off it.
    fn open_or_close_lines(&mut self, count: usize, open: bool) {
        self.end_grapheme();
        let (top, bottom) = self.region();
        let bottom = bottom.min(self.rows.saturating_sub(1));
        // A cursor parked outside the region is not in the span these edit,
        // so they do nothing at all — DEC's own rule, and the one that keeps
        // a status line safe from a program editing the pane above it.
        if self.cursor_row < top || self.cursor_row > bottom {
            return;
        }
        let span = bottom - self.cursor_row + 1;
        for _ in 0..count.min(span) {
            if open {
                let evicted = self.cells.remove(bottom);
                let fresh = self.recycled_row(Some(evicted));
                self.cells.insert(self.cursor_row, fresh);
                self.wrap.remove(bottom);
                self.wrap.insert(self.cursor_row, None);
                self.folds.remove(bottom);
                self.folds.insert(self.cursor_row, None);
            } else {
                let evicted = self.cells.remove(self.cursor_row);
                let fresh = self.recycled_row(Some(evicted));
                self.cells.insert(bottom, fresh);
                self.wrap.remove(self.cursor_row);
                self.wrap.insert(bottom, None);
                self.folds.remove(self.cursor_row);
                self.folds.insert(bottom, None);
            }
        }
        for flag in &mut self.wrap[self.cursor_row..=bottom] {
            *flag = None;
        }
        for row in self.cursor_row..=bottom {
            self.mark_row(row);
        }
        self.dirty = true;
    }

    /// `CSI Ps @` / `CSI Ps P` — open or close cells WITHIN the cursor's row.
    ///
    /// The rest of the line slides; what passes the right edge is gone. A
    /// line editor redrawing one changed character uses these instead of
    /// repainting the row, so a terminal without them shows the old text with
    /// the new text stamped on top of it.
    fn open_or_close_cells(&mut self, count: usize, open: bool) {
        self.end_grapheme();
        let (row, col) = (self.cursor_row, self.cursor_col);
        if col >= self.cols || row >= self.rows {
            return;
        }
        let count = count.min(self.cols - col);
        let line = &mut self.cells[row];
        if open {
            for _ in 0..count {
                line.pop();
                line.insert(col, Cell::default());
            }
        } else {
            for _ in 0..count {
                line.remove(col);
                line.push(Cell::default());
            }
        }
        // A pair split down the middle by the slide is no longer a pair.
        self.heal_wide_pairs(row);
        self.wrap[row] = None;
        self.mark_row(row);
        self.dirty = true;
    }

    /// After cells slide, a wide glyph may have lost its second half or a
    /// continuation may have lost the glyph it belonged to. Either half alone
    /// is a lie about what occupies the column, so both are blanked.
    fn heal_wide_pairs(&mut self, row: usize) {
        for col in 0..self.cols {
            let wide = |cell: &Cell| cell.ch.width().unwrap_or(0) >= 2;
            let orphaned_tail = self.cells[row][col].is_continuation()
                && (col == 0 || !wide(&self.cells[row][col - 1]));
            let headless = wide(&self.cells[row][col])
                && (col + 1 >= self.cols || !self.cells[row][col + 1].is_continuation());
            if orphaned_tail || headless {
                self.cells[row][col] = Cell::default();
            }
        }
    }

    /// Erase `count` cells from the cursor without moving anything — `CSI X`.
    fn erase_chars(&mut self, count: usize) {
        let (row, col) = (self.cursor_row, self.cursor_col);
        self.mark_row(row);
        for offset in 0..count {
            self.break_cluster_at(row, col + offset, 0);
        }
        if let Some(line) = self.cells.get_mut(row) {
            for cell in line.iter_mut().skip(col).take(count) {
                *cell = Cell::default();
            }
        }
    }

    /// Blank the whole cluster covering `col`, however many columns it spans.
    ///
    /// Anything landing on part of a multi-column glyph destroys the rest's
    /// meaning: a head with a missing tail advances into a column somebody
    /// else now owns, and a tail with no head is a claimed column with
    /// nothing in front of it. Neither is representable, so the cluster goes
    /// as a unit.
    ///
    /// **Written as a run rather than a pair on purpose.** It handled exactly
    /// two columns while a wide glyph was always two — but a cluster is as
    /// wide as the width tables say, and `🏴 ZWJ Ⓜ` is three. Clearing two of
    /// its three cells left the third behind.
    ///
    /// `floor` is the leftmost column the walk may reach. A caller that is
    /// mid-way through placing its own cluster passes the column after it, so
    /// looking for somebody else's head cannot wander into its own.
    fn break_cluster_at(&mut self, row: usize, col: usize, floor: usize) {
        let Some(line) = self.cells.get_mut(row) else {
            return;
        };
        if col >= line.len() {
            return;
        }
        // A continuation belongs to the glyph in front of it, so walk to the
        // head before deciding how much to clear.
        let mut head = col;
        while head > floor && line[head].is_continuation() {
            head -= 1;
        }
        if line[head].is_continuation() {
            // Still inside a tail at the floor, or an orphan with no head at
            // all. Clear what is here and stop rather than guess.
            line[head] = Cell::default();
            return;
        }
        let width = cell_width(&line[head]);
        if width <= 1 {
            return;
        }
        for offset in 0..width {
            if let Some(cell) = line.get_mut(head + offset) {
                *cell = Cell::default();
            }
        }
    }

    /// Stop the grapheme on screen from growing any further.
    ///
    /// Called by everything that moves the cursor or rewrites the row. A mark
    /// arriving afterwards has nothing to attach to, which is right: the
    /// cursor sitting after a character is not the same as having just
    /// printed it, and decorating whatever is behind it corrupts a grapheme
    /// somebody else wrote.
    fn end_grapheme(&mut self) {
        self.pending = None;
    }

    /// Point the pen at `uri`, or at nothing when it is empty (the close).
    ///
    /// The same address twice is the same link: `ls --hyperlink` prints one
    /// `OSC 8` per file name, and a table that grew a row per printed name
    /// would be a directory listing's worth of duplicate strings.
    fn open_hyperlink(&mut self, uri: &[u8]) {
        if uri.is_empty() {
            self.pen.link = None;
            return;
        }
        // A URI is bytes on the wire and text everywhere else; anything that
        // is not UTF-8 is not an address this window could open.
        let Ok(uri) = std::str::from_utf8(uri) else {
            self.pen.link = None;
            return;
        };
        if uri.len() > MAX_HYPERLINK_URI_BYTES {
            self.pen.link = None;
            return;
        }
        if let Some(id) = self.link_ids.get(uri) {
            self.pen.link = Some(*id);
            return;
        }
        // Past the ceiling the text still prints — it just prints as text. A
        // program that emits a fresh address per line would otherwise turn a
        // scrollback into a list of strings nobody can see.
        if self.links.len() >= MAX_HYPERLINKS {
            self.pen.link = None;
            return;
        }
        // The number names a row that does not exist yet, and the table and its
        // index are filled together below — so an id that cannot be made is a
        // link never recorded at all, rather than a row the index has no
        // answer for.
        let Some(id) = u32::try_from(self.links.len() + 1)
            .ok()
            .and_then(NonZeroU32::new)
        else {
            self.pen.link = None;
            return;
        };
        let uri: Arc<str> = Arc::from(uri);
        self.links.push(Arc::clone(&uri));
        self.link_ids.insert(uri, id);
        self.pen.link = Some(id);
    }

    /// Apply an OSC 7788 `begin` or `end` marker.
    fn fold_marker(&mut self, params: &[&[u8]]) {
        let Some(id) = params
            .get(2)
            .and_then(|raw| std::str::from_utf8(raw).ok())
            .and_then(|text| text.trim().parse::<u32>().ok())
        else {
            return;
        };
        match params.get(1).copied() {
            Some(b"begin") => {
                // The protocol forbids nesting. An inner begin and its end do
                // not disturb the outer region.
                if self.open_fold.is_some() || self.fold_table.len() >= MAX_FOLDS {
                    return;
                }
                let flags = params.get(3).copied().unwrap_or(b"");
                let collapsed = flags
                    .split(|byte| *byte == b',')
                    .all(|word| word.trim_ascii() != b"expanded");
                // `teaser=<n>`: the first n body rows stay visible while the
                // region is collapsed and go away when it opens.
                let teaser_left = flags
                    .split(|byte| *byte == b',')
                    .find_map(|word| word.trim_ascii().strip_prefix(b"teaser="))
                    .and_then(|count| std::str::from_utf8(count).ok())
                    .and_then(|count| count.trim().parse::<u32>().ok())
                    .unwrap_or(0);
                let summary = percent_decode(params.get(4).copied().unwrap_or(b""));
                self.open_fold = Some(OpenFold {
                    id,
                    header_seen: false,
                    teaser_left,
                });
                if let Some(meta) = self.fold_table.iter_mut().find(|meta| meta.id == id) {
                    meta.collapsed = collapsed;
                    meta.summary = summary;
                } else {
                    self.fold_table.push(FoldMeta {
                        id,
                        collapsed,
                        summary,
                    });
                }
            }
            Some(b"end") if self.open_fold.is_some_and(|open| open.id == id) => {
                self.open_fold = None;
            }
            _ => {}
        }
    }

    /// Mark the printed row as the open region's header or body.
    fn claim_fold_row(&mut self, row: usize) {
        let Some(open) = self.open_fold else { return };
        if self
            .folds
            .get(row)
            .copied()
            .flatten()
            .is_some_and(|fold| fold.id == open.id)
        {
            return;
        }
        let role = if !open.header_seen {
            FoldRole::Header
        } else if open.teaser_left > 0 {
            FoldRole::Teaser
        } else {
            FoldRole::Body
        };
        let Some(slot) = self.folds.get_mut(row) else {
            return;
        };
        *slot = Some(RowFold { id: open.id, role });
        if let Some(open) = self.open_fold.as_mut() {
            match role {
                FoldRole::Header => open.header_seen = true,
                FoldRole::Teaser => open.teaser_left = open.teaser_left.saturating_sub(1),
                FoldRole::Body => {}
            }
        }
    }

    /// The address behind a cell's [`CellStyle::link`], if the id is one of
    /// ours.
    #[must_use]
    pub fn link_uri(&self, id: NonZeroU32) -> Option<&str> {
        self.links.get(id.get() as usize - 1).map(|held| &**held)
    }

    /// Links a consumer has not been sent yet, as `(id, uri)`.
    ///
    /// Marked as sent by [`Self::take_delta`], so a reader that takes deltas
    /// sees each address exactly once and a reader that asks for a full frame
    /// gets the whole table (see [`Self::all_links`]).
    #[must_use]
    fn fresh_links(&self) -> Vec<(u32, Arc<str>)> {
        self.links
            .iter()
            .enumerate()
            .skip(self.links_sent)
            .map(|(at, uri)| (at as u32 + 1, Arc::clone(uri)))
            .collect()
    }

    /// The whole table, for the frame that replaces everything a consumer
    /// holds.
    #[must_use]
    fn all_links(&self) -> Vec<(u32, Arc<str>)> {
        self.links
            .iter()
            .enumerate()
            .map(|(at, uri)| (at as u32 + 1, Arc::clone(uri)))
            .collect()
    }

    /// Put the cursor back where it was saved, if it was.
    fn restore_cursor(&mut self) {
        self.end_grapheme();
        if let Some((row, col)) = self.saved_cursor {
            self.cursor_row = row.min(self.rows - 1);
            self.cursor_col = col.min(self.cols - 1);
        }
    }

    /// Does this scalar belong to the grapheme already on screen?
    ///
    /// **This is a segmentation question, and it took three tries to stop
    /// answering it with something else.** First a hand-written list of
    /// joiners, which missed skin-tone modifiers. Then "does adding the
    /// scalar cost a column", asked of the width tables — and that is wrong
    /// too, because a prefix's width is not monotone toward the finished
    /// one: `🏴 ZWJ Ⓜ FE0F` measures 2, while its three-scalar prefix
    /// `🏴 ZWJ Ⓜ` measures 3, since `Ⓜ` only becomes emoji-presentation once
    /// the selector arrives. Width answers how many columns; it cannot answer
    /// where a grapheme ends.
    ///
    /// UAX #29 is the standard that does, and it settles every case the two
    /// guesses got wrong at once — a modifier extends, a joiner binds two
    /// pictographics, a joiner with no partner does not bind a letter, and a
    /// regional-indicator pair is one flag rather than two letters.
    fn try_extend_cluster(&mut self, ch: char) -> bool {
        let joins = {
            let Some(pending) = self.pending.as_mut() else {
                return false;
            };
            // Ordinary text never joins, and this is asked for every printable
            // character — two graphic ASCII scalars are always a boundary, so
            // the common case never reaches the segmenter.
            if ch.is_ascii_graphic() && pending.text.ends_with_ascii_graphic() {
                return false;
            }

            // Ask UAX #29 on the pending buffer itself. The old path copied the
            // whole cluster into a second String, appended `ch` there, then
            // appended it again below when the answer was yes. Appending once
            // and rolling back a no keeps one buffer and one copy in the path.
            let before = pending.text.byte_len();
            pending.text.push(ch);
            let joins = pending.text.is_one_grapheme();
            if !joins {
                pending.text.truncate(before);
            }
            joins
        };
        if joins {
            self.extend_cluster(ch);
        }
        joins
    }

    /// Add a scalar to the grapheme on screen, and re-place it if the width
    /// changed.
    ///
    /// Re-measured on the whole cluster every time, because the answer moves
    /// as it is assembled and **it moves in both directions**: `☂` is one
    /// column and `☂️` is two, but `🏴 ZWJ Ⓜ` is three and `🏴 ZWJ Ⓜ FE0F` is
    /// two again. So the placement has to be able to give a column back as
    /// well as take one — a grow-only placement leaves the third column
    /// claimed by a glyph that no longer reaches it.
    ///
    /// The child's own cursor advances by the finished cluster's width, so
    /// converging on that is what keeps this grid level with it.
    fn extend_cluster(&mut self, ch: char) {
        let (row, col, was, want): (usize, usize, usize, usize) = {
            let Some(pending) = self.pending.as_mut() else {
                return;
            };
            // The whole cluster, including scalars the cell had no room for:
            // the ceiling truncates the glyph, never the layout.
            let measured = pending.text.width();
            pending.measured = measured;
            (pending.row, pending.col, pending.width, measured)
        };
        // A cluster occupies at least the column it was placed in, and never
        // columns the row does not have.
        let want = want.clamp(1, self.cols.saturating_sub(col).max(1));
        if let Some(pending) = self.pending.as_mut() {
            pending.width = want;
        }
        if let Some(cell) = self.cells.get_mut(row).and_then(|line| line.get_mut(col)) {
            cell.zw.push(ch);
        }
        let pen = self.pen;
        for taken in was..want {
            // Growing reaches into a column somebody else may hold, so it goes
            // through the same rule an ordinary write does.
            // Never below the cluster being placed: its own head sits at `col`.
            self.break_cluster_at(row, col + taken, col + was);
            if let Some(cell) = self
                .cells
                .get_mut(row)
                .and_then(|line| line.get_mut(col + taken))
            {
                cell.ch = CONTINUATION;
                cell.style = pen;
                cell.zw = ZeroWidth::default();
            }
        }
        for given_back in want..was {
            // Blank rather than left claimed: a continuation with no glyph
            // reaching it is the defect this whole seam exists to prevent.
            if let Some(cell) = self
                .cells
                .get_mut(row)
                .and_then(|line| line.get_mut(col + given_back))
            {
                *cell = Cell::default();
            }
        }
        self.cursor_col = col + want;
        self.mark_row(row);
    }

    /// Print one character at the cursor, in the columns it is actually drawn
    /// across.
    ///
    /// Width is not a rendering preference — it is the contract the program on
    /// the other end of the pty already follows. It lays a Korean line out at
    /// two columns per syllable and moves its own cursor by two, so a grid
    /// that advances one lands N columns to the left of the child after N
    /// syllables and every cell after that goes to the wrong place. On screen
    /// that was a Korean line whose tail ran off the right edge.
    fn write_char(&mut self, ch: char) {
        // The hot lane: almost every byte a shell, a compiler or a build log
        // prints is printable ASCII, and for those everything the general
        // path consults is already known by inspection — the width tables say
        // 1, UAX #29 says two ASCII scalars never join (the rule
        // `try_extend_cluster` itself opens with), and the only cleanup a
        // width-1 write can owe is when it lands on a wide glyph's head or
        // tail, which one cheap look at the target cell rules out. The
        // general path stays the single authority for everything else.
        if ch.is_ascii_graphic() || ch == ' ' {
            let boundary = self
                .pending
                .as_ref()
                .is_none_or(|held| held.text.ends_with_ascii());
            if boundary {
                self.wrap_before(1);
                let (row, col) = (self.cursor_row, self.cursor_col);
                let plain = self
                    .cells
                    .get(row)
                    .and_then(|line| line.get(col))
                    .is_some_and(|cell| {
                        cell.ch != CONTINUATION && cell.ch.is_ascii() && cell.zw.is_empty()
                    });
                if !plain {
                    // A wide glyph or a cluster owns this column; evict the
                    // whole of it the way the general path would.
                    self.break_cluster_at(row, col, 0);
                }
                let pen = self.pen;
                if let Some(cell) = self.cells.get_mut(row).and_then(|line| line.get_mut(col)) {
                    cell.ch = ch;
                    cell.style = pen;
                    cell.zw = ZeroWidth::default();
                }
                self.note_glyph_drawn(ch);
                self.mark_row(row);
                self.claim_fold_row(row);
                self.cursor_col += 1;
                self.begin_grapheme(ch, row, col, 1);
                return;
            }
        }
        // Part of the grapheme already on screen rather than a new one — a
        // combining mark, a presentation selector, or the partner a joiner is
        // waiting for. It joins that cluster instead of starting one.
        if self.try_extend_cluster(ch) {
            return;
        }
        // Ambiguous-width characters (box drawing, some punctuation) count as
        // one here, which is what modern terminals default to; the CJK-wide
        // variant would shift every box-drawing TUI a column at a time.
        let width = UnicodeWidthChar::width(ch).unwrap_or(0);
        // A scalar that owns no column and has no grapheme in front of it
        // modifies nothing, so there is nowhere for it to go.
        if width == 0 {
            return;
        }
        self.wrap_before(width);
        // Read the cursor only after the wrap above may have moved it — marking
        // the row first would name the row we just left.
        let (row, col) = (self.cursor_row, self.cursor_col);
        let pen = self.pen;
        for offset in 0..width {
            self.break_cluster_at(row, col + offset, 0);
        }
        if let Some(line) = self.cells.get_mut(row) {
            if let Some(cell) = line.get_mut(col) {
                cell.ch = ch;
                cell.style = pen;
                // Whatever was here is gone, and so is anything hanging off it.
                cell.zw = ZeroWidth::default();
            }
            if width == 2
                && let Some(tail) = line.get_mut(col + 1)
            {
                // The pen too: a reversed or coloured background has to cover
                // the whole glyph, not its left half.
                tail.ch = CONTINUATION;
                tail.style = pen;
            }
        }
        self.note_glyph_drawn(ch);
        self.mark_row(row);
        self.claim_fold_row(row);
        self.cursor_col += width;
        self.begin_grapheme(ch, row, col, width);
    }

    /// The child wraps a pair rather than splitting it across the edge.
    fn wrap_before(&mut self, width: usize) {
        if self.cursor_col + width > self.cols {
            // The columns the row actually carried before the edge — `cols`
            // normally, one less when this pair is what pushed us over. The
            // one moment that distinction exists is now.
            let written = u16::try_from(self.cursor_col).unwrap_or(u16::MAX);
            self.cursor_col = 0;
            self.newline();
            self.wrap[self.cursor_row] = Some(written);
        }
    }

    /// Somebody is waiting for this character to be drawn, and it just was.
    /// Recorded here rather than read off the cell afterwards, because the
    /// fact wanted is the *drawing* — the cell will still hold the glyph a
    /// thousand rounds from now, and that is not news. Which screen took it
    /// is recorded in the same breath: pen time is the only moment that
    /// fact exists. Cleared by whoever takes it; see `take_glyph_drawn`.
    fn note_glyph_drawn(&mut self, ch: char) {
        if self.watched_glyph == Some(ch) {
            self.glyph_drawn = true;
            if self.alt_screen {
                self.glyph_drawn_in_alt = true;
            }
        }
    }

    /// Start the on-screen grapheme this character opened.
    ///
    /// ASCII stays inline in [`ClusterText::Ascii`], while an owned grapheme
    /// buffer is taken from the previous pending cluster and reused. This is
    /// the most-executed line in the crate, so a plain glyph should pay neither
    /// a heap allocation nor a temporary copy; [`Self::try_extend_cluster`]
    /// promotes to an owned buffer only when a scalar might join it.
    fn begin_grapheme(&mut self, ch: char, row: usize, col: usize, width: usize) {
        let text = self
            .pending
            .take()
            .map_or_else(|| ClusterText::from_char(ch), |held| held.text.reset(ch));
        self.pending = Some(Pending {
            row,
            col,
            text,
            width,
            measured: width,
        });
    }

    /// Apply one SGR sequence to the pen.
    ///
    /// Two encodings must both work, and they can be mixed in one sequence.
    /// Separate parameters (`ESC[38;5;196m`) arrive as one group per number,
    /// while the colon form (`ESC[38:5:196m`) arrives as a single group of
    /// subparameters.
    ///
    /// **Order is the whole problem.** SGR is a sequence of mutations, so
    /// handling one encoding first and the other afterwards silently reverses
    /// them: `ESC[0;38:2:255:0:0m` means "reset, then red" but a two-pass
    /// implementation applies red and then the reset, and the colour vanishes.
    /// So this walks the groups exactly once, in wire order.
    fn apply_sgr(&mut self, params: &Params) {
        if params.is_empty() {
            // A bare `ESC[m` is `ESC[0m`.
            self.pen = CellStyle::default();
            return;
        }

        let mut groups = params.iter();
        while let Some(group) = groups.next() {
            if group.len() > 1 {
                // Colon form: self-contained.
                self.apply_sgr_slice(group);
                continue;
            }
            let Some(&code) = group.first() else {
                continue;
            };
            if matches!(code, 38 | 48) {
                // Semicolon form: the colour's operands are the single-valued
                // groups that follow. Read exactly that fixed-size tail from
                // the parser's own iterator, leaving later attributes for the
                // next loop without an intermediate Vec.
                let Some(color) = extended_color_groups(&mut groups) else {
                    // Malformed: stop rather than reinterpreting the tail as
                    // unrelated attributes.
                    return;
                };
                if code == 38 {
                    self.pen.fg = color;
                } else {
                    self.pen.bg = color;
                }
                continue;
            }
            self.apply_sgr_slice(&[code]);
        }
    }

    fn apply_sgr_slice(&mut self, values: &[u16]) {
        let mut index = 0;
        while index < values.len() {
            let code = values[index];
            match code {
                0 => self.pen = CellStyle::default(),
                1 => self.pen.bold = true,
                2 => self.pen.dim = true,
                3 => self.pen.italic = true,
                4 => self.pen.underline = true,
                // Blink (SGR 5). Kept as a flag and left to the renderer to
                // decide what it means — xterm blinks the text, and a person
                // who has asked their system for less motion is owed the
                // colour without the motion.
                5 => self.pen.blink = true,
                7 => self.pen.reverse = true,
                // Strikethrough (SGR 9). A diff or a todo list draws its
                // done/removed lines with this, and without it those lines
                // read as ordinary text saying the opposite of what they mean.
                9 => self.pen.strike = true,
                22 => {
                    self.pen.bold = false;
                    self.pen.dim = false;
                }
                23 => self.pen.italic = false,
                24 => self.pen.underline = false,
                25 => self.pen.blink = false,
                27 => self.pen.reverse = false,
                29 => self.pen.strike = false,
                30..=37 => self.pen.fg = Color::Indexed((code - 30) as u8),
                39 => self.pen.fg = Color::Default,
                40..=47 => self.pen.bg = Color::Indexed((code - 40) as u8),
                49 => self.pen.bg = Color::Default,
                90..=97 => self.pen.fg = Color::Indexed((code - 90 + 8) as u8),
                100..=107 => self.pen.bg = Color::Indexed((code - 100 + 8) as u8),
                38 | 48 => {
                    let foreground = code == 38;
                    match extended_color(&values[index + 1..]) {
                        Some((color, consumed)) => {
                            if foreground {
                                self.pen.fg = color;
                            } else {
                                self.pen.bg = color;
                            }
                            index += consumed;
                        }
                        // Malformed extended colour: stop rather than
                        // reinterpreting its tail as unrelated attributes.
                        None => break,
                    }
                }
                _ => {}
            }
            index += 1;
        }
    }

    fn erase_in_display(&mut self, mode: u16) {
        match mode {
            // Cursor to end of screen.
            0 => {
                self.erase_in_line(0);
                for row in (self.cursor_row + 1)..self.rows {
                    self.cells[row].fill(Cell::default());
                    self.wrap[row] = None;
                    self.folds[row] = None;
                    self.mark_row(row);
                }
            }
            // Start of screen to cursor.
            1 => {
                for row in 0..self.cursor_row {
                    self.cells[row].fill(Cell::default());
                    self.folds[row] = None;
                    self.mark_row(row);
                }
                // The cursor row's own flag goes too: it claimed to continue
                // a row that is blank now.
                for flag in &mut self.wrap[0..=self.cursor_row] {
                    *flag = None;
                }
                self.erase_in_line(1);
            }
            // Whole screen. `2` keeps the cursor, `3` also drops scrollback.
            2 | 3 => {
                for row in 0..self.rows {
                    self.cells[row].fill(Cell::default());
                }
                self.wrap.fill(None);
                self.folds.fill(None);
                self.mark_all_rows();
                if mode == 3 {
                    self.scrollback_trimmed = self
                        .scrollback_trimmed
                        .saturating_add(self.scrollback.len());
                    self.scrollback.clear();
                    self.scroll_wrap.clear();
                    self.scroll_folds.clear();
                    self.scroll_cols.clear();
                }
            }
            _ => {}
        }
    }

    fn erase_in_line(&mut self, mode: u16) {
        let (row, col) = (self.cursor_row, self.cursor_col);
        // Only the modes the match below actually clears; an unknown mode
        // leaves the row alone and must not be reported as changed.
        if matches!(mode, 0..=2) {
            self.mark_row(row);
        }
        // The boundary can fall on half of a double-width glyph, and the half
        // outside the erased span would be left claiming a column alone.
        if matches!(mode, 0 | 1) {
            self.break_cluster_at(row, col, 0);
        }
        let Some(line) = self.cells.get_mut(row) else {
            return;
        };
        match mode {
            0 => {
                for cell in line.iter_mut().skip(col) {
                    *cell = Cell::default();
                }
            }
            1 => {
                for cell in line.iter_mut().take(col + 1) {
                    *cell = Cell::default();
                }
            }
            2 => {
                for cell in line.iter_mut() {
                    *cell = Cell::default();
                }
            }
            _ => {}
        }
    }

    fn set_private_mode(&mut self, mode: u16, enabled: bool) {
        match mode {
            // Measured on a real agent TUI: `claude` sets 25, 2004, 1004,
            // 2031 and 2026 before it draws anything.
            25 => {
                // Counted as an emission, not just kept as a level. "The
                // program showed its cursor" is the ready signal one family
                // of agent TUIs uses (they hide it to redraw and show it when
                // the input line is back), and a level cannot say *shown
                // again* — a `?25l…?25h` pair inside one pump round leaves
                // the level exactly where it started.
                if enabled {
                    self.cursor_shows = self.cursor_shows.wrapping_add(1);
                }
                if self.cursor_visible != enabled {
                    self.cursor_visible = enabled;
                    self.dirty = true;
                }
            }
            1004 => {
                self.focus_reporting = enabled;
                // The window has to hear about this or it cannot know whether
                // saying "focused" is worth a round trip — and a round trip
                // per tab switch, for a program that never asked, is exactly
                // the cost the mouse modes ride the frame to avoid.
                self.modes_dirty = true;
            }
            // Turning one ON names a level. Turning ANY of them off takes the
            // mouse away entirely.
            //
            // This used to read the three as a ladder — "nothing sets two, so
            // `?1003l` after `?1000h` only means no motion" — and the premise
            // was measured false. A real agent TUI sets all three at once:
            // `claude` opens with `?1049h ?1000h ?1002h ?1003h ?1006h` before
            // it draws. Against a ladder, a program that then let go with a
            // SUBSET (`?1002l ?1000l`, which is what crossterm's
            // `DisableMouseCapture` writes) leaves this stuck on `Motion`
            // forever — and a window that believes the mouse is still wanted
            // keeps handing the wheel to a program that stopped listening, so
            // the terminal never scrolls again for the life of that shell.
            //
            // Both references answer any `l` with "off": xterm's own
            // `dpmodes`, and the xterm.js Orca ships
            // (`I18nProvider-4EBrmTGg.js`: `case 9: case 1e3: case 1002: case
            // 1003: activeProtocol = "NONE"`).
            1000 | 1002 | 1003 => {
                let wanted = if enabled {
                    match mode {
                        1000 => MouseTracking::Click,
                        1002 => MouseTracking::Drag,
                        _ => MouseTracking::Motion,
                    }
                } else {
                    MouseTracking::Off
                };
                if self.mouse_tracking != wanted {
                    self.mouse_tracking = wanted;
                    self.modes_dirty = true;
                }
            }
            1006 => {
                if self.mouse_sgr != enabled {
                    self.mouse_sgr = enabled;
                    self.modes_dirty = true;
                }
            }
            2004 => self.bracketed_paste = enabled,
            // Colour-scheme updates. Subscribing is not a question, so nothing
            // is sent here — the seed only makes the FIRST notification a
            // change rather than an introduction.
            2031 => {
                self.color_scheme_subscribed = enabled;
                self.told_color_scheme = if enabled {
                    self.reads_as_dark_now()
                } else {
                    None
                };
            }
            2026 => {
                self.sync_began = if enabled {
                    Some(std::time::Instant::now())
                } else {
                    None
                };
            }
            1049 => self.set_alt_screen(enabled),
            _ => {}
        }
    }

    /// Enter or leave the alternate screen, preserving the primary one.
    ///
    /// This is what makes a full-screen program safe to run in a lane: an
    /// editor or pager takes the whole screen, and on exit the shell output
    /// that was there before must come back untouched. Clearing instead of
    /// restoring silently eats the user's scroll position and last command.
    fn set_alt_screen(&mut self, enabled: bool) {
        // The whole buffer is swapped out from under the placement.
        self.end_grapheme();
        if enabled == self.alt_screen {
            return;
        }
        if enabled {
            self.saved_screen = Some(SavedScreen {
                cells: std::mem::replace(
                    &mut self.cells,
                    vec![vec![Cell::default(); self.cols]; self.rows],
                ),
                wrap: std::mem::replace(&mut self.wrap, vec![None; self.rows]),
                folds: std::mem::replace(&mut self.folds, vec![None; self.rows]),
                cursor_row: self.cursor_row,
                cursor_col: self.cursor_col,
                pen: self.pen,
            });
            self.cursor_row = 0;
            self.cursor_col = 0;
            self.pen = CellStyle::default();
        } else if let Some(saved) = self.saved_screen.take() {
            self.cells = saved.cells;
            self.wrap = saved.wrap;
            self.folds = saved.folds;
            // The window may have been resized while the alternate screen was
            // up, so the restored buffer is re-fitted rather than trusted.
            let cols = self.cols;
            for line in &mut self.cells {
                refit_line(line, cols);
            }
            self.cells
                .resize(self.rows, vec![Cell::default(); self.cols]);
            self.wrap.resize(self.rows, None);
            self.folds.resize(self.rows, None);
            self.cursor_row = saved.cursor_row.min(self.rows - 1);
            self.cursor_col = saved.cursor_col.min(self.cols - 1);
            self.pen = saved.pen;
        }
        self.alt_screen = enabled;
        self.dirty = true;
        // A whole different buffer is on screen now. Like a resize, this makes
        // any pending shift meaningless, so the next delta is absolute.
        self.mark_all_rows();
        self.scrolled_since_take = 0;
        self.full_repaint = true;
        // Whatever history was being read belongs to the screen that left.
        self.view_offset = 0;
    }
}

/// Re-fit one row to a new width.
///
/// Truncating can take a double-width glyph's second column with it, leaving a
/// glyph that still advances two into a column the row no longer has — on
/// screen, a character hanging off the right edge. The surviving half becomes
/// a blank instead.
///
/// A free function because two paths re-fit rows and they must agree: `resize`
/// does it to the visible screen, and leaving the alternate screen does it to
/// the primary buffer that was parked while a full-screen program ran — a
/// buffer no resize in the meantime ever touched.
fn refit_line(line: &mut Vec<Cell>, cols: usize) {
    line.resize(cols, Cell::default());
    let Some(last) = cols.checked_sub(1) else {
        return;
    };
    // Find the head of whatever covers the final column, then ask whether its
    // glyph still fits. Walking rather than checking the last cell alone,
    // because a cluster three columns wide can lose only its third and leave
    // a head plus one continuation that still looks like an ordinary pair.
    let mut head = last;
    while head > 0 && line[head].is_continuation() {
        head -= 1;
    }
    if line[head].is_continuation() {
        line[head] = Cell::default();
        return;
    }
    if head + cell_width(&line[head]) > cols {
        for cell in &mut line[head..] {
            *cell = Cell::default();
        }
    }
}

/// How many columns this cell's glyph is drawn across.
///
/// The whole cluster, not its first scalar: `☂` is one column and `☂️` is two,
/// and the difference lives in the tail the cell carries. A number rather than
/// "is it wide", because a cluster is not limited to two — `🏴 ZWJ Ⓜ` is
/// three, and every rule that assumed a pair left its third cell behind.
fn cell_width(cell: &Cell) -> usize {
    if cell.zw.is_empty() {
        return UnicodeWidthChar::width(cell.ch).unwrap_or(1);
    }
    // A cell carries at most one head plus ZW_CAPACITY riders, and a UTF-8
    // scalar is at most four bytes. The old String here allocated every time
    // an overwrite or resize had to inspect a complex glyph; the fixed bound
    // is already part of Cell's representation, so use it directly.
    let mut bytes = [0; (ZW_CAPACITY + 1) * 4];
    let mut len = 0;
    for ch in std::iter::once(cell.ch).chain(cell.zw.scalars()) {
        len += ch.encode_utf8(&mut bytes[len..]).len();
    }
    let text = std::str::from_utf8(&bytes[..len]).expect("chars encode as valid UTF-8");
    UnicodeWidthStr::width(text).max(1)
}

/// Decode an extended colour written as separate semicolon parameters.
///
/// [`Params`] already stores these in a fixed array. Pulling the two or four
/// values directly keeps every SGR sequence allocation-free and leaves any
/// following attribute for the caller's next iteration.
fn extended_color_groups<'a>(groups: &mut impl Iterator<Item = &'a [u16]>) -> Option<Color> {
    let single = |group: &[u16]| match group {
        [value] => Some(*value),
        _ => None,
    };
    match single(groups.next()?)? {
        5 => Some(Color::Indexed(u8::try_from(single(groups.next()?)?).ok()?)),
        2 => Some(Color::Rgb(
            u8::try_from(single(groups.next()?)?).ok()?,
            u8::try_from(single(groups.next()?)?).ok()?,
            u8::try_from(single(groups.next()?)?).ok()?,
        )),
        _ => None,
    }
}

/// Decode the tail of an `SGR 38`/`48` extended colour, returning the colour
/// and how many extra values it consumed.
///
/// `5;n` is a palette index and `2;r;g;b` is direct colour. Anything else is
/// rejected so the caller can stop rather than treat the remainder as
/// attributes — which is how a truecolor sequence turns into stray bold text.
fn extended_color(rest: &[u16]) -> Option<(Color, usize)> {
    match rest.first()? {
        5 => {
            let index = *rest.get(1)?;
            Some((Color::Indexed(u8::try_from(index).ok()?), 2))
        }
        2 => {
            let red = u8::try_from(*rest.get(1)?).ok()?;
            let green = u8::try_from(*rest.get(2)?).ok()?;
            let blue = u8::try_from(*rest.get(3)?).ok()?;
            Some((Color::Rgb(red, green, blue), 4))
        }
        _ => None,
    }
}

/// First parameter of a CSI sequence, defaulting when absent or zero.
/// `CSI c` and `CSI 0 c` are the primary device attributes question; every
/// other parameter makes it someone else's question.
fn is_primary_device_attributes(params: &Params) -> bool {
    let mut values = params.iter();
    match values.next() {
        None => true,
        Some(first) => values.next().is_none() && matches!(first, [] | [0]),
    }
}

fn first_param(params: &Params, default: u16) -> u16 {
    params
        .iter()
        .next()
        .and_then(|values| values.first().copied())
        .filter(|value| *value != 0)
        .unwrap_or(default)
}

impl Perform for TerminalGrid {
    fn print(&mut self, ch: char) {
        self.write_char(ch);
        self.dirty = true;
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            b'\n' => {
                self.end_grapheme();
                self.newline();
                self.dirty = true;
            }
            b'\r' => {
                self.end_grapheme();
                self.cursor_col = 0;
                self.dirty = true;
            }
            0x08 => {
                self.end_grapheme();
                self.cursor_col = self.cursor_col.saturating_sub(1);
                self.dirty = true;
            }
            b'\t' => {
                self.end_grapheme();
                let next = ((self.cursor_col / 8) + 1) * 8;
                self.cursor_col = next.min(self.cols - 1);
                self.dirty = true;
            }
            // The bell. Raised, not drawn: it moves no cell and prints no
            // glyph, so a grapheme being assembled around one survives it
            // untouched — and `dirty` is deliberately NOT set, because nothing
            // about the screen changed. What it does set is its own flag, which
            // `take_delta` treats as reason enough to send a frame.
            0x07 => {
                self.bell = true;
                self.announce_bell = true;
            }
            // Everything else: swallowed, never drawn — and they move nothing,
            // so a grapheme being assembled around one survives them.
            _ => {}
        }
    }

    fn csi_dispatch(&mut self, params: &Params, intermediates: &[u8], _ignore: bool, action: char) {
        // A grapheme ends where the cursor moves or a cell is rewritten, and
        // **that is decided by what actually runs**, not by the action
        // character. Asking the character alone was wrong twice: it caught
        // `SGR`, which only sets the pen, and then it caught `ESC[?1J`, which
        // is private, falls through to the swallow arm and touches nothing —
        // and both threw away the combining mark that came next.
        //
        // So the call lives in the arms that mutate, and an arm added later
        // that moves the cursor or writes a cell has to make it too.
        let private = intermediates.first() == Some(&b'?');
        // Any OTHER intermediate marks a different dialect wearing the same
        // final letter — `CSI > 4;2 m` is XTMODKEYS (modifyOtherKeys), not
        // SGR, and reading it as SGR is how every glyph went underlined and
        // dim the moment the tmux shim answered `extended-keys on` and
        // claude started saying it (1-fu, "화면이 깨져있어"). Swallowed
        // whole: this grid does not speak those dialects, and half-reading
        // one is the only wrong answer.
        // Two questions wear an intermediate byte, and the swallow below
        // cannot tell a dialect this grid does not speak from a question it
        // owes an answer to. So they are answered before it.
        if intermediates.last() == Some(&b'$') && action == 'p' {
            self.report_mode(first_param(params, 0), private);
            return;
        }
        if intermediates.first() == Some(&b'>') && action == 'c' {
            self.replies.extend_from_slice(DEVICE_ATTRIBUTES_SECONDARY);
            return;
        }
        // XTVERSION, in xterm's own `name(version)` spelling. Only `CSI > q`
        // and `CSI > 0 q` are that question; the swallow below takes the rest.
        if intermediates.first() == Some(&b'>') && action == 'q' {
            if first_param(params, 0) == 0 {
                self.replies.extend_from_slice(
                    format!("\x1bP>|{TERMINAL_NAME}({TERMINAL_VERSION})\x1b\\").as_bytes(),
                );
            }
            return;
        }
        if !private && !intermediates.is_empty() {
            return;
        }
        match (private, action) {
            (true, 'h' | 'l') => {
                let enabled = action == 'h';
                for values in params.iter() {
                    if let Some(mode) = values.first() {
                        self.set_private_mode(*mode, enabled);
                    }
                }
            }
            (false, 'A') => {
                self.end_grapheme();
                self.cursor_row = self
                    .cursor_row
                    .saturating_sub(first_param(params, 1) as usize);
            }
            (false, 'B') => {
                self.end_grapheme();
                let step = first_param(params, 1) as usize;
                self.cursor_row = (self.cursor_row + step).min(self.rows - 1);
            }
            (false, 'C') => {
                self.end_grapheme();
                let step = first_param(params, 1) as usize;
                self.cursor_col = (self.cursor_col + step).min(self.cols - 1);
            }
            (false, 'D') => {
                self.end_grapheme();
                self.cursor_col = self
                    .cursor_col
                    .saturating_sub(first_param(params, 1) as usize);
            }
            (false, 'H' | 'f') => {
                self.end_grapheme();
                let mut iter = params.iter();
                let row = iter
                    .next()
                    .and_then(|values| values.first().copied())
                    .filter(|value| *value != 0)
                    .unwrap_or(1) as usize;
                let col = iter
                    .next()
                    .and_then(|values| values.first().copied())
                    .filter(|value| *value != 0)
                    .unwrap_or(1) as usize;
                self.cursor_row = (row - 1).min(self.rows - 1);
                self.cursor_col = (col - 1).min(self.cols - 1);
            }
            (false, 'J') => {
                self.end_grapheme();
                self.erase_in_display(first_param(params, 0).min(3));
            }
            (false, 'K') => {
                self.end_grapheme();
                self.erase_in_line(first_param(params, 0).min(2));
            }
            (false, 'm') => self.apply_sgr(params),
            // Straight to a column or a row. Most TUIs begin every line they
            // draw with one of these, so swallowing them left each line
            // starting wherever the last one ended.
            (false, 'G' | '`') => {
                self.end_grapheme();
                self.cursor_col = (first_param(params, 1) as usize - 1).min(self.cols - 1);
            }
            (false, 'd') => {
                self.end_grapheme();
                self.cursor_row = (first_param(params, 1) as usize - 1).min(self.rows - 1);
            }
            (false, 'E' | 'F') => {
                self.end_grapheme();
                let step = first_param(params, 1) as usize;
                self.cursor_col = 0;
                self.cursor_row = if action == 'E' {
                    (self.cursor_row + step).min(self.rows - 1)
                } else {
                    self.cursor_row.saturating_sub(step)
                };
            }
            // Erase a run in place, without redrawing the rest of the line.
            (false, 'X') => {
                self.end_grapheme();
                self.erase_chars((first_param(params, 1) as usize).max(1));
            }
            // The scroll region. This is how a TUI reserves a header or a
            // status line; without it, scrolling took the whole screen and the
            // reserved rows slid away with everything else.
            (false, 'r') => {
                self.end_grapheme();
                let mut iter = params.iter();
                let top = iter
                    .next()
                    .and_then(|values| values.first().copied())
                    .filter(|value| *value != 0)
                    .unwrap_or(1) as usize;
                let bottom = iter
                    .next()
                    .and_then(|values| values.first().copied())
                    .filter(|value| *value != 0)
                    .unwrap_or(self.rows as u16) as usize;
                let top = top.saturating_sub(1).min(self.rows - 1);
                let bottom = bottom.saturating_sub(1).min(self.rows - 1);
                self.scroll_region = if top < bottom && (top, bottom) != (0, self.rows - 1) {
                    Some((top, bottom))
                } else {
                    None
                };
                // Setting the region homes the cursor, which is what the
                // programs that use it expect.
                self.cursor_row = 0;
                self.cursor_col = 0;
            }
            (false, 's') => self.saved_cursor = Some((self.cursor_row, self.cursor_col)),
            (false, 'u') => self.restore_cursor(),
            // Questions, not instructions. A program that asks waits for the
            // answer, so swallowing these is what makes a TUI look like it
            // never started.
            // Only the PRIMARY device attributes question — `CSI c` or
            // `CSI 0 c`. `CSI 1 c` is a different question and answering it
            // with this reply is worse than silence (the original declines it
            // too, terminal-capability-replies.ts:21-23).
            (false, 'c') if is_primary_device_attributes(params) => {
                self.replies.extend_from_slice(self.identity.reply());
            }
            (false, 'n') => match first_param(params, 0) {
                5 => self.replies.extend_from_slice(b"\x1b[0n"),
                6 => {
                    let report = format!("\x1b[{};{}R", self.cursor_row + 1, self.cursor_col + 1);
                    self.replies.extend_from_slice(report.as_bytes());
                }
                _ => {}
            },
            // Private DSR. `?6n` is DECXCPR — the same question as CPR
            // wearing the private marker, and its answer wears it too. `?996n`
            // asks the one thing about the palette a program can act on.
            (true, 'n') => match first_param(params, 0) {
                6 => {
                    let report = format!("\x1b[?{};{}R", self.cursor_row + 1, self.cursor_col + 1);
                    self.replies.extend_from_slice(report.as_bytes());
                }
                996 => {
                    if let Some(dark) = self.reads_as_dark_now() {
                        self.told_color_scheme = Some(dark);
                        self.replies.extend_from_slice(if dark {
                            b"\x1b[?997;1n"
                        } else {
                            b"\x1b[?997;2n"
                        });
                    }
                }
                _ => {}
            },
            // `CSI Ps S` / `CSI Ps T`: scroll the region by whole lines.
            (false, 'S') => self.scroll_lines(first_param(params, 1) as usize, true),
            (false, 'T') => self.scroll_lines(first_param(params, 1) as usize, false),
            // `CSI Ps L` / `CSI Ps M`: open or close whole lines at the cursor.
            (false, 'L') => self.open_or_close_lines(first_param(params, 1) as usize, true),
            (false, 'M') => self.open_or_close_lines(first_param(params, 1) as usize, false),
            // `CSI Ps @` / `CSI Ps P`: open or close cells within the row.
            (false, '@') => self.open_or_close_cells(first_param(params, 1) as usize, true),
            (false, 'P') => self.open_or_close_cells(first_param(params, 1) as usize, false),
            // Everything else: parsed, then swallowed. Never printed.
            _ => {}
        }
        self.dirty = true;
    }

    fn esc_dispatch(&mut self, _intermediates: &[u8], _ignore: bool, byte: u8) {
        match byte {
            // `ESC 7` / `ESC 8`. A matched pair a TUI wraps its scroll-region
            // setup in — `claude` emits exactly that on startup — and with
            // neither implemented the cursor was left wherever the sequence
            // put it and everything after landed in the wrong place.
            b'7' => self.saved_cursor = Some((self.cursor_row, self.cursor_col)),
            b'8' => self.restore_cursor(),
            // `ESC M`: up a row, scrolling the region backwards at its top.
            b'M' => {
                self.end_grapheme();
                self.reverse_index();
                self.dirty = true;
            }
            _ => {}
        }
    }

    fn osc_dispatch(&mut self, params: &[&[u8]], _bell_terminated: bool) {
        if params.first() == Some(&b"52".as_slice()) {
            if let Some(text) = parse_osc52_clipboard_write(params) {
                self.osc52_clipboard_write = Some(Osc52ClipboardWrite(text));
            }
            return;
        }
        // OSC 8 = an explicit hyperlink: `OSC 8 ; params ; URI ST` opens one
        // and `OSC 8 ; ; ST` closes it. What makes it worth carrying is that
        // the text and the address are allowed to differ — `ls --hyperlink`
        // and every modern build tool print a short name that opens a long
        // path — so a window that only linkifies text that LOOKS like a URL
        // shows those as ordinary words.
        if params.first() == Some(&b"8".as_slice()) {
            self.open_hyperlink(params.get(2).copied().unwrap_or(b""));
            return;
        }
        if params.first() == Some(&b"7788".as_slice()) {
            self.fold_marker(params);
            return;
        }
        // The colour questions. A program is allowed to ask what it is being
        // drawn in, and the ones that ask are the ones that adapt — which is
        // why silence here is not neutral: it makes them guess.
        match params.first().copied() {
            Some(b"4") => {
                self.palette_colors(params);
                return;
            }
            Some(b"10") => {
                self.special_colors(params, 0);
                return;
            }
            Some(b"11") => {
                self.special_colors(params, 1);
                return;
            }
            Some(b"12") => {
                self.special_colors(params, 2);
                return;
            }
            // The restores: drop the mutation and the theme's colour is what
            // answers again.
            Some(b"104") => {
                self.restore_palette(params);
                return;
            }
            Some(b"110") => {
                self.color_overrides.foreground = None;
                return;
            }
            Some(b"111") => {
                self.color_overrides.background = None;
                return;
            }
            Some(b"112") => {
                self.color_overrides.cursor = None;
                return;
            }
            _ => {}
        }
        // OSC 0 = icon + title, OSC 2 = title. Both are how an agent CLI tells
        // us what it is doing, which is what the lane header shows.
        let Some(kind) = params.first() else { return };
        if !matches!(*kind, b"0" | b"2") {
            return;
        }
        if let Some(value) = params.get(1) {
            self.title = Some(String::from_utf8_lossy(value).into_owned());
            self.dirty = true;
            self.title_dirty = true;
            self.announce_title = true;
        }
    }
}

/// Decode the percent escapes used by OSC 7788 summaries.
fn percent_decode(raw: &[u8]) -> String {
    let mut out = Vec::with_capacity(raw.len());
    let mut at = 0;
    while at < raw.len() {
        let byte = raw[at];
        let hex = |one: u8| (one as char).to_digit(16);
        if byte == b'%'
            && let (Some(hi), Some(lo)) = (
                raw.get(at + 1).copied().and_then(hex),
                raw.get(at + 2).copied().and_then(hex),
            )
        {
            out.push((hi * 16 + lo) as u8);
            at += 3;
            continue;
        }
        out.push(byte);
        at += 1;
    }
    out.truncate(MAX_FOLD_SUMMARY_BYTES);
    String::from_utf8_lossy(&out).into_owned()
}

fn parse_osc52_clipboard_write(params: &[&[u8]]) -> Option<String> {
    if params.len() != 3 || params[0] != b"52" {
        return None;
    }
    let selections = params[1];
    let payload = params[2];
    let valid_selections = if selections.is_empty() {
        true
    } else {
        selections
            .iter()
            .all(|byte| matches!(byte, b'c' | b'p' | b'q' | b's' | b'0'..=b'7'))
    };
    if !valid_selections || payload == b"?" || payload.len() > MAX_OSC52_BASE64_CHARS {
        return None;
    }

    let mut compact = Vec::with_capacity(payload.len());
    for byte in payload {
        if matches!(byte, b' ' | b'\t' | b'\n' | b'\x0b' | b'\x0c' | b'\r') {
            continue;
        }
        if !matches!(byte, b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'+' | b'/' | b'=') {
            return None;
        }
        compact.push(*byte);
    }
    let decoded = STANDARD.decode(compact).ok()?;
    (!decoded.is_empty()).then(|| String::from_utf8_lossy(&decoded).into_owned())
}

/// A parser bound to a grid. Two fields rather than one struct because
/// `Parser::advance` borrows the performer mutably and disjointly.
///
/// And the screens reading that grid ([`crate::readers`]): a take and the
/// shares it fills must be one step, so the delivery state lives under the
/// same lock as the grid it takes from.
pub struct Terminal {
    parser: Parser,
    grid: TerminalGrid,
    readers: FrameReaders,
}

// `vte::Parser` is not `Debug`, and its internal state is not useful in a
// dump anyway — the grid is what anyone debugging a lane wants to see.
impl std::fmt::Debug for Terminal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Terminal")
            .field("grid", &self.grid)
            .finish()
    }
}

impl Terminal {
    #[must_use]
    pub fn new(rows: usize, cols: usize) -> Self {
        Self {
            parser: Parser::new(),
            grid: TerminalGrid::new(rows, cols),
            readers: FrameReaders::default(),
        }
    }

    /// Feed raw PTY bytes. Partial UTF-8 across calls is handled by the parser,
    /// so a multibyte character split across two reads still prints once.
    pub fn feed(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.grid, bytes);
    }

    #[must_use]
    pub const fn grid(&self) -> &TerminalGrid {
        &self.grid
    }

    pub const fn grid_mut(&mut self) -> &mut TerminalGrid {
        &mut self.grid
    }

    /// The screens reading this terminal, with the grid they take from —
    /// borrowed together, because every question they answer takes a delta.
    pub const fn readers_mut(&mut self) -> (&mut FrameReaders, &mut TerminalGrid) {
        (&mut self.readers, &mut self.grid)
    }
}

/// The largest screen [`drawn`] will build. The window clamps a pane's grid to
/// the same pair (`gridSize`, ui/shell.js), and a caller asking for a thousand
/// rows is a bug rather than a screen.
const DRAWN_ROWS_MAX: usize = 200;
const DRAWN_COLS_MAX: usize = 400;

/// A finished stream's SCREEN, or `None` when its bytes were only ever text.
///
/// Bytes a program wrote are not characters to print. Pasted into a page as
/// characters, a run that painted itself arrives as rubble — `[2K`, `[1A`,
/// `[?2026h`, reported 2026-08-21 against a finished `claude --resume` — because
/// the escape that steers is unprintable and everything steering on it survives
/// as literal text. Only an emulator can say what those bytes drew, and this
/// crate owns the one every live pane already runs on ([`Terminal`]); one is
/// built here, fed once, and dropped with the answer.
///
/// `None` for a stream that never steered a cursor. A plain log IS its text —
/// showing it as text is not a lie, and pouring it into a `rows`-tall screen
/// would throw away every line that scrolled off the top.
#[must_use]
pub fn drawn(bytes: &[u8], rows: usize, cols: usize) -> Option<GridDelta> {
    if !bytes.iter().copied().any(steers) {
        return None;
    }
    let mut terminal = Terminal::new(rows.clamp(1, DRAWN_ROWS_MAX), cols.clamp(1, DRAWN_COLS_MAX));
    terminal.feed(bytes);
    Some(terminal.grid().snapshot())
}

/// A byte that moves the cursor rather than printing under it.
///
/// `\n` and `\t` are left out on purpose: a `<pre>` lays both out where a
/// terminal would, so a log built from them is text and not a screen. Every
/// byte named here is ASCII-range, so none of them can be mistaken for the tail
/// of a multi-byte character — which is why C1 CSI (`U+009B`) is not in the
/// list: its continuation byte rides inside ordinary Hangul.
const fn steers(byte: u8) -> bool {
    matches!(byte, 0x07 | 0x08 | 0x0d | 0x1b)
}

#[cfg(test)]
mod mouse_tests {
    use super::*;

    fn fed(bytes: &[u8]) -> Terminal {
        let mut term = Terminal::new(4, 10);
        term.feed(bytes);
        term
    }

    /// A modifier-key report is not a pen stroke.
    ///
    /// `CSI > 4;2 m` is XTMODKEYS — claude says it the moment the tmux shim
    /// answers `extended-keys on` — and a parser that ignores the `>` reads
    /// it as SGR 4;2: every glyph from then on underlined and dim, which is
    /// exactly how the break arrived ("화면이 깨져있어", 1-fu). An
    /// intermediate that is not `?` names a dialect this grid does not
    /// speak, and the whole sequence is swallowed; the real SGR beside it
    /// still lands.
    #[test]
    fn a_modifier_key_report_is_not_a_pen_stroke() {
        let term = fed(b"\x1b[>4;2mA");
        let struck = term.grid().cell(0, 0).expect("the glyph landed");
        assert!(!struck.style.underline, "XTMODKEYS read as SGR underline");
        assert!(!struck.style.dim, "XTMODKEYS read as SGR dim");

        let term = fed(b"\x1b[4;2mB");
        let styled = term.grid().cell(0, 0).expect("the glyph landed");
        assert!(styled.style.underline && styled.style.dim, "real SGR lost");
    }

    /// Turning one on names a level; turning any of them off ends the mouse.
    ///
    /// This test used to say the opposite — that the three are a ladder, and
    /// `?1003l` after `?1000h` only means "no motion" — on the premise that
    /// nothing sets two at once. Measured against a real agent TUI the premise
    /// is false: `claude` opens with `?1000h ?1002h ?1003h` together. So the
    /// only safe reading of an `l` is xterm's: the mouse is off. The cost of
    /// the other reading is not a missed click but a dead wheel — see
    /// [`a_partial_release_still_ends_the_mouse`].
    #[test]
    fn turning_any_level_off_ends_the_mouse() {
        let term = fed(b"\x1b[?1000h");
        assert_eq!(term.grid().mouse_tracking(), MouseTracking::Click);

        let term = fed(b"\x1b[?1000h\x1b[?1000l");
        assert_eq!(term.grid().mouse_tracking(), MouseTracking::Off);

        let term = fed(b"\x1b[?1002h");
        assert_eq!(term.grid().mouse_tracking(), MouseTracking::Drag);
        let term = fed(b"\x1b[?1003h");
        assert_eq!(term.grid().mouse_tracking(), MouseTracking::Motion);

        // A level nobody set still ends it — which is exactly what xterm and
        // the xterm.js Orca ships both do.
        let term = fed(b"\x1b[?1000h\x1b[?1003l");
        assert_eq!(term.grid().mouse_tracking(), MouseTracking::Off);
    }

    /// The three set together, released in part, are released entirely.
    ///
    /// The measured shape of the bug: `claude` turns all three on, and a
    /// crossterm program lets go with `?1006l ?1015l ?1002l ?1000l` — no
    /// `?1003l` anywhere in it. Read as a ladder that leaves the grid on
    /// `Motion` with nobody listening, and every wheel notch from then on is
    /// posted to a program that has stopped reading its mouse instead of
    /// moving this terminal's own history.
    #[test]
    fn a_partial_release_still_ends_the_mouse() {
        let term =
            fed(b"\x1b[?1000h\x1b[?1002h\x1b[?1003h\x1b[?1006h\x1b[?1006l\x1b[?1002l\x1b[?1000l");
        assert_eq!(
            term.grid().mouse_tracking(),
            MouseTracking::Off,
            "a shell whose TUI let go of the mouse is still swallowing the wheel"
        );
        assert!(!term.grid().mouse_sgr());
    }

    /// A mode change reaches the window on its own.
    ///
    /// Neither tracking nor the encoding dirties a cell, so without saying so
    /// the news would wait for the program's next output — and a program that
    /// enables the mouse and then sits still would never be clickable.
    #[test]
    fn turning_the_mouse_on_is_a_frame() {
        let mut term = Terminal::new(4, 10);
        term.feed(b"x");
        let _ = term.grid_mut().take_delta();
        assert!(
            term.grid_mut().take_delta().is_none(),
            "the screen was not quiet"
        );

        term.feed(b"\x1b[?1000h\x1b[?1006h");
        let delta = term
            .grid_mut()
            .take_delta()
            .expect("the mode change was not reported");
        assert_eq!(delta.mouse_tracking, MouseTracking::Click);
        assert!(delta.mouse_sgr);
        assert!(
            term.grid_mut().take_delta().is_none(),
            "the mode change kept reporting itself"
        );
    }

    /// A reattach learns the modes without waiting for the next frame.
    #[test]
    fn a_snapshot_carries_the_modes() {
        let term = fed(b"\x1b[?1003h\x1b[?1006h");
        let shot = term.grid().snapshot();
        assert_eq!(shot.mouse_tracking, MouseTracking::Motion);
        assert!(shot.mouse_sgr);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn capped_selection_history() -> Terminal {
        let mut term = Terminal::new(4, 20);
        term.grid_mut().scrollback_cap = 4;
        term.feed(b"zero\r\none\r\ntwo\r\nthree\r\nfour\r\nfive\r\nsix\r\n");
        let _ = term.grid_mut().take_delta();
        assert_eq!(term.grid().scrollback_len(), 4);
        term
    }

    fn wire_trimmed(frame: &GridDelta) -> u64 {
        serde_json::to_value(frame).expect("wire frame")["scrollback_trimmed"]
            .as_u64()
            .unwrap_or(0)
    }

    /// The view shift: a quiet scroll back is the exposed rows and a shift,
    /// not the whole window; a scroll while output arrived is absolute.
    #[test]
    fn a_quiet_scroll_is_a_view_shift_and_a_noisy_one_is_absolute() {
        let mut term = Terminal::new(6, 8);
        for n in 0..16 {
            term.feed(format!("L{n}\r\n").as_bytes());
        }
        let _ = term.grid_mut().take_delta();
        let text = |row: &GridRow| {
            row.cells
                .iter()
                .map(|cell| cell.ch)
                .collect::<String>()
                .trim_end()
                .to_string()
        };

        term.grid_mut().scroll_view(3);
        let back = term
            .grid_mut()
            .take_delta()
            .expect("a moved view is a frame");
        assert!(!back.full, "a quiet scroll back resent the whole window");
        assert_eq!(back.view_shift, -3, "back into history: rows move down");
        assert_eq!(back.scrolled_lines, 0);
        assert_eq!(back.view_offset, 3);
        assert_eq!(
            back.rows
                .iter()
                .map(|row| (row.index, text(row)))
                .collect::<Vec<_>>(),
            vec![(0, "L8".into()), (1, "L9".into()), (2, "L10".into())],
            "only the three exposed top rows travel"
        );

        term.grid_mut().scroll_view(-2);
        let forward = term
            .grid_mut()
            .take_delta()
            .expect("a moved view is a frame");
        assert!(!forward.full);
        assert_eq!(
            forward.view_shift, 2,
            "toward the live screen: rows move up"
        );
        assert_eq!(
            forward
                .rows
                .iter()
                .map(|row| (row.index, text(row)))
                .collect::<Vec<_>>(),
            vec![(4, "L14".into()), (5, "L15".into())],
            "only the two exposed bottom rows travel"
        );

        // A move as long as the screen has nothing to keep: absolute.
        term.grid_mut().scroll_view(6);
        let far = term
            .grid_mut()
            .take_delta()
            .expect("a moved view is a frame");
        assert!(
            far.full && far.view_shift == 0,
            "a screen-long move is absolute"
        );

        // Output while scrolled back changes the history the window sits on.
        term.grid_mut().scroll_view(-3);
        let _ = term.grid_mut().take_delta();
        term.feed(b"more\r\n");
        term.grid_mut().scroll_view(1);
        let noisy = term
            .grid_mut()
            .take_delta()
            .expect("output and a move are a frame");
        assert!(
            noisy.full && noisy.view_shift == 0,
            "a move beside output is absolute"
        );

        // The wire leaves the field out when it is zero, so old consumers see
        // the frames they always saw.
        let json = serde_json::to_string(&noisy).unwrap();
        assert!(!json.contains("view_shift"), "{json}");
        let json = serde_json::to_string(&back).unwrap();
        assert!(json.contains("\"view_shift\":-3"), "{json}");
    }

    /// A move that lands on the live screen and is not a shift is the whole
    /// screen. The consumer holds a window over history; a frame naming only
    /// the rows output touched — or no row at all, after a fling longer than
    /// the screen — leaves that window on screen under a scrollbar that says
    /// live, and nothing further down exists to scroll to.
    #[test]
    fn a_move_home_that_is_not_a_shift_is_the_whole_live_screen() {
        let mut term = Terminal::new(6, 8);
        for n in 0..16 {
            term.feed(format!("L{n}\r\n").as_bytes());
        }
        let _ = term.grid_mut().take_delta();
        let text = |row: &GridRow| {
            row.cells
                .iter()
                .map(|cell| cell.ch)
                .collect::<String>()
                .trim_end()
                .to_string()
        };

        // A fling home from deeper than the screen: no row the consumer
        // holds is a row of the live screen.
        term.grid_mut().scroll_view(8);
        let far = term
            .grid_mut()
            .take_delta()
            .expect("a moved view is a frame");
        assert!(far.full && far.view_offset == 8);
        term.grid_mut().scroll_view(-8);
        let home = term
            .grid_mut()
            .take_delta()
            .expect("a moved view is a frame");
        assert!(
            home.full && home.view_shift == 0,
            "a screen-long move home is absolute"
        );
        assert_eq!(home.view_offset, 0);
        assert_eq!(home.rows.len(), 6, "every row of the live screen travels");
        assert_eq!(text(&home.rows[0]), "L11");
        assert_eq!(text(&home.rows[4]), "L15");
        assert!(home.cursor_visible, "the live screen has its caret back");

        // Home beside output: the rows output touched are not the only rows
        // the consumer is missing.
        term.grid_mut().scroll_view(2);
        let _ = term.grid_mut().take_delta();
        term.feed(b"L16\r\n");
        assert_eq!(term.grid().view_offset(), 3, "the reader stayed on L9");
        term.grid_mut().scroll_view(-3);
        let noisy = term
            .grid_mut()
            .take_delta()
            .expect("output and a move are a frame");
        assert!(
            noisy.full && noisy.view_shift == 0 && noisy.scrolled_lines == 0,
            "a move home beside output is absolute"
        );
        assert_eq!(noisy.view_offset, 0);
        assert_eq!(noisy.rows.len(), 6);
        assert_eq!(text(&noisy.rows[0]), "L12");
        assert_eq!(text(&noisy.rows[4]), "L16");
    }

    #[test]
    fn scrollback_eviction_crosses_a_scrolled_full_frame() {
        let mut term = capped_selection_history();
        term.grid_mut().scroll_view(3);
        let before = term.grid_mut().take_delta().expect("history frame");
        let selected = term.grid().text_between((1, 0), (1, 20));
        term.feed(b"next\r\n");
        let frame = term.grid_mut().take_delta().expect("output frame");
        assert!(frame.full);
        assert_eq!(frame.scrolled_lines, 0);
        assert_eq!(term.grid().text_between((0, 0), (0, 20)), selected);
        assert_eq!(wire_trimmed(&frame) - wire_trimmed(&before), 1);
    }

    #[test]
    fn scrollback_eviction_crosses_a_fold_composed_full_frame() {
        let mut term = capped_selection_history();
        term.feed(b"\x1b[1;1H\x1b]7788;begin;7;;\x1b\\head\r\nbody1\r\nbody2\r\n\x1b]7788;end;7\x1b\\after");
        let before = term.grid_mut().take_delta().expect("fold frame");
        term.feed(b"\r\n");
        let frame = term.grid_mut().take_delta().expect("output frame");
        assert!(frame.full);
        assert!(!frame.row_lines.is_empty());
        assert_eq!(frame.scrolled_lines, 0);
        assert_eq!(wire_trimmed(&frame) - wire_trimmed(&before), 1);
    }

    #[test]
    fn scrollback_eviction_crosses_a_top_start_partial_region() {
        let mut term = capped_selection_history();
        let before = term.grid().snapshot();
        let selected = term.grid().text_between((1, 0), (1, 20));
        let footer = term.grid().line(3);
        term.feed(b"\x1b[1;3r\x1b[3;1Hnext\r\n");
        let frame = term.grid_mut().take_delta().expect("partial region output");
        assert!(!frame.full);
        assert_eq!(frame.scrolled_lines, 0);
        assert_eq!(term.grid().line(3), footer);
        assert_eq!(term.grid().text_between((0, 0), (0, 20)), selected);
        assert_eq!(wire_trimmed(&frame) - wire_trimmed(&before), 1);
    }

    #[test]
    fn scrollback_eviction_is_state_across_snapshots_and_missed_deltas() {
        let mut term = capped_selection_history();
        term.feed(b"next\r\n");
        let first = term.grid().snapshot();
        assert_eq!(wire_trimmed(&first), 1);
        assert_eq!(wire_trimmed(&term.grid().snapshot()), 1);
        assert_eq!(wire_trimmed(&term.grid_mut().take_snapshot()), 1);
        assert!(term.grid_mut().take_delta().is_none());
        term.feed(b"later\r\n");
        let _ = term.grid_mut().take_delta();
        term.feed(b"latest\r\n");
        assert_eq!(wire_trimmed(&term.grid_mut().take_snapshot()), 3);
    }

    #[test]
    fn scrollback_eviction_counts_cap_reduction_and_history_erase() {
        let mut term = Terminal::new(4, 20);
        for _ in 0..MIN_SCROLLBACK_LINES + 8 {
            term.feed(b"line\r\n");
        }
        let before = term.grid().scrollback_len();
        term.grid_mut().set_scrollback_cap(MIN_SCROLLBACK_LINES);
        assert_eq!(
            wire_trimmed(&term.grid().snapshot()),
            (before - MIN_SCROLLBACK_LINES) as u64
        );
        term.feed(b"\x1b[3J");
        assert_eq!(wire_trimmed(&term.grid().snapshot()), before as u64);
    }

    #[test]
    fn scrollback_eviction_excludes_growth_and_alternate_screen_rotation() {
        let mut term = Terminal::new(4, 20);
        term.feed(b"zero\r\none\r\ntwo\r\nthree\r\nfour\r\n");
        assert_eq!(wire_trimmed(&term.grid().snapshot()), 0);
        term.grid_mut().scrollback_cap = term.grid().scrollback_len();
        term.feed(b"\x1b[?1049hzero\r\none\r\ntwo\r\nthree\r\nfour\r\n");
        assert_eq!(wire_trimmed(&term.grid().snapshot()), 0);
    }

    fn screen(rows: usize, cols: usize, text: &str) -> Terminal {
        let mut term = Terminal::new(rows, cols);
        term.feed(text.as_bytes());
        term
    }

    /// A Korean syllable occupies TWO terminal columns.
    ///
    /// That is not a rendering preference, it is the contract the program on
    /// the other end of the pty already follows: it lays its line out for two
    /// columns per syllable and moves its own cursor accordingly. A grid that
    /// advances one column per character therefore ends up N columns to the
    /// left of where the child thinks it is after N syllables, and every cell
    /// printed afterwards lands in the wrong place — which on screen is a
    /// Korean line whose tail runs past the right edge.
    #[test]
    fn a_korean_syllable_takes_the_two_columns_it_is_drawn_in() {
        let term = screen(4, 20, "가나다X");
        assert_eq!(
            term.grid().cursor(),
            (0, 7),
            "three syllables are six columns, so the X after them starts at 6"
        );
        assert_eq!(term.grid().cell(0, 0).map(|c| c.ch), Some('가'));
        assert_eq!(term.grid().cell(0, 2).map(|c| c.ch), Some('나'));
        assert_eq!(term.grid().cell(0, 6).map(|c| c.ch), Some('X'));
    }

    /// The column a wide glyph's second half occupies is spoken for, and the
    /// renderer has to be told so — a cell that merely repeated the glyph
    /// would draw it twice.
    #[test]
    fn the_second_column_of_a_wide_glyph_is_marked_as_taken() {
        let term = screen(4, 20, "가");
        let tail = term.grid().cell(0, 1).expect("the second column exists");
        assert!(
            tail.is_continuation(),
            "the column is not claimed by anybody"
        );
        assert!(!term.grid().cell(0, 0).unwrap().is_continuation());
    }

    /// A pair cannot straddle the right edge: the child wraps rather than
    /// splitting a glyph, so a grid that splits it disagrees about which row
    /// the rest of the line is on.
    #[test]
    fn a_wide_glyph_that_does_not_fit_wraps_instead_of_splitting() {
        let term = screen(4, 5, "abcd가");
        assert_eq!(term.grid().cell(0, 4).map(|c| c.ch), Some(' '));
        assert_eq!(term.grid().cell(1, 0).map(|c| c.ch), Some('가'));
        assert_eq!(term.grid().cursor(), (1, 2));
    }

    /// Overwriting half of a pair has to take the other half with it, or the
    /// row keeps a claimed column with nothing in front of it.
    #[test]
    fn overwriting_half_a_pair_does_not_leave_the_other_half_behind() {
        // `ESC[H` returns to the origin, so the `x` lands on the 가's left half.
        let term = screen(4, 20, "가나\x1b[Hx");
        assert_eq!(term.grid().cell(0, 0).map(|c| c.ch), Some('x'));
        assert_eq!(
            term.grid().cell(0, 1).map(|c| c.ch),
            Some(' '),
            "the orphaned second column still claims to continue something"
        );
    }

    /// A combining mark is drawn inside the cell before it, so it occupies no
    /// column of its own and must not push the line along.
    #[test]
    fn a_zero_width_mark_does_not_move_the_cursor() {
        let term = screen(4, 20, "e\u{0301}");
        assert_eq!(term.grid().cursor(), (0, 1));
        assert_eq!(term.grid().cell(0, 0).map(|c| c.ch), Some('e'));
    }

    /// The text helpers are what tests and the lane title read, so the marker
    /// must never reach them.
    #[test]
    fn the_marker_never_shows_up_in_text() {
        let term = screen(4, 20, "가나다");
        assert_eq!(term.grid().line(0), "가나다");
        assert!(!term.grid().visible_text().contains('\0'));
    }

    /// A family emoji is one glyph, and one glyph is two columns.
    ///
    /// `👨 ZWJ 👩 ZWJ 👧` measures 2 as a string and 6 as a sum of its
    /// scalars' widths, and the grid was adding up the scalars. So it reserved
    /// six columns for a glyph the font draws in two, and every character
    /// after it on the row sat four columns right of where the child put it.
    /// Width is a property of the cluster, which is why it is asked of the
    /// cluster.
    #[test]
    fn a_family_emoji_is_one_grapheme_in_two_columns() {
        let term = screen(4, 20, "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}X");
        assert_eq!(term.grid().cursor(), (0, 3), "the family took six columns");
        assert_eq!(term.grid().cell(0, 0).map(|c| c.ch), Some('\u{1F468}'));
        assert!(term.grid().cell(0, 1).unwrap().is_continuation());
        assert_eq!(term.grid().cell(0, 2).map(|c| c.ch), Some('X'));
        assert_eq!(
            term.grid().line(0),
            "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}X"
        );
    }

    /// A presentation selector widens the glyph in front of it.
    ///
    /// `☂` is one column; `☂️` is two. The selector has no width of its own,
    /// so a grid that places on the first scalar has already decided one
    /// column by the time it learns better — the placement has to be able to
    /// grow.
    #[test]
    fn an_emoji_presentation_selector_widens_the_glyph_before_it() {
        let term = screen(4, 20, "\u{2602}\u{FE0F}X");
        assert_eq!(term.grid().cursor(), (0, 3), "the umbrella stayed narrow");
        assert!(
            term.grid().cell(0, 1).unwrap().is_continuation(),
            "the column it grew into is not claimed"
        );
        assert_eq!(term.grid().cell(0, 2).map(|c| c.ch), Some('X'));
    }

    /// Every mark on one letter is kept, not just the last.
    #[test]
    fn a_letter_keeps_all_of_its_marks() {
        let term = screen(4, 20, "e\u{0301}\u{0302}");
        assert_eq!(term.grid().cursor(), (0, 1), "a mark took a column");
        assert_eq!(term.grid().line(0), "e\u{0301}\u{0302}");
    }

    /// A mark after a cursor move belongs to nothing.
    ///
    /// The cursor sitting after a character is not the same as having just
    /// printed it. Attaching to whatever is behind the cursor decorates a
    /// grapheme somebody else wrote — silent corruption no ASCII test sees.
    #[test]
    fn a_mark_after_a_cursor_move_attaches_to_nothing() {
        // `ESC[H` home, then two columns forward: the cursor is behind `b`
        // without anything having been printed.
        let term = screen(4, 20, "ab\x1b[H\x1b[2C\u{0301}");
        assert_eq!(term.grid().line(0), "ab", "the mark landed on a stale cell");
    }

    /// The tail has a stated ceiling, and it truncates the glyph, not the row.
    ///
    /// A cluster longer than [`ZW_CAPACITY`] cannot be stored whole. What
    /// must not happen is the columns going wrong with it: a row missing a
    /// glyph is visible, a row whose columns are wrong moves everything after
    /// it and nothing says why.
    #[test]
    fn a_cluster_past_the_ceiling_still_lands_on_the_right_columns() {
        let long = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}\u{200D}\u{1F466}";
        let term = screen(4, 20, &format!("{long}X"));
        assert_eq!(term.grid().cursor(), (0, 3));
        assert_eq!(term.grid().cell(0, 2).map(|c| c.ch), Some('X'));
        assert_eq!(
            term.grid().cell(0, 0).unwrap().zw.scalars().count(),
            ZW_CAPACITY,
            "the tail did not fill to its stated ceiling"
        );
    }

    /// A skin-tone modifier joins the emoji in front of it.
    ///
    /// `🏽` measures 2 on its own, and `👍🏽` measures 2 as a string — the
    /// modifier costs no columns because it recolours the hand rather than
    /// standing beside it. The first version of this fix asked "does a joiner
    /// come before it", which is a hand-written list of the ways graphemes
    /// join, and the list was missing this one. The question is instead
    /// whether adding the scalar costs a column, which is what the width
    /// tables already answer.
    #[test]
    fn a_skin_tone_modifier_joins_the_emoji_it_recolours() {
        let term = screen(4, 20, "\u{1F44D}\u{1F3FD}X");
        assert_eq!(term.grid().cursor(), (0, 3), "the modifier took columns");
        assert_eq!(term.grid().cell(0, 0).map(|c| c.ch), Some('\u{1F44D}'));
        assert!(term.grid().cell(0, 1).unwrap().is_continuation());
        assert_eq!(term.grid().cell(0, 2).map(|c| c.ch), Some('X'));
        assert_eq!(term.grid().line(0), "\u{1F44D}\u{1F3FD}X");
    }

    /// A dangling joiner does not swallow the next character.
    ///
    /// `👨 ZWJ A` is an emoji, a joiner that found no partner, and a letter —
    /// three columns. Asking the width tables gets this right; asking "did a
    /// joiner just arrive" does not.
    #[test]
    fn a_joiner_with_no_partner_does_not_absorb_a_letter() {
        let term = screen(4, 20, "\u{1F468}\u{200D}A");
        assert_eq!(term.grid().cursor(), (0, 3));
        assert_eq!(term.grid().cell(0, 2).map(|c| c.ch), Some('A'));
    }

    /// A glyph that became two columns is still two columns at a re-fit.
    ///
    /// `refit_line` asked the head scalar how wide it was, and `☂` is one
    /// column — but `☂️` is two, and the cell holds the selector that made it
    /// so. Truncating the row then took the continuation and left a two-column
    /// glyph hanging off the right edge, which is exactly the defect
    /// `refit_line` exists to prevent.
    #[test]
    fn a_glyph_widened_by_a_selector_is_re_fitted_as_wide() {
        let mut term = screen(4, 6, "abcd\u{2602}\u{FE0F}");
        assert_eq!(term.grid().cell(0, 4).map(|c| c.ch), Some('\u{2602}'));
        term.grid_mut().resize(4, 5);
        assert_eq!(
            term.grid().cell(0, 4).map(|c| c.ch),
            Some(' '),
            "a two-column glyph is still claiming a column the row lost"
        );
    }

    /// Colour does not end a grapheme.
    ///
    /// Clearing the pending cluster on every CSI was too broad: `SGR` sets
    /// the pen and moves nothing, so a program that colours a letter and then
    /// sends its accent — which is ordinary — had the accent thrown away.
    /// Only the sequences that move the cursor or rewrite cells end it.
    #[test]
    fn setting_a_colour_does_not_break_a_grapheme_apart() {
        let term = screen(4, 20, "e\x1b[31m\u{0301}!");
        assert_eq!(term.grid().line(0), "e\u{0301}!", "the accent was dropped");
        assert_eq!(term.grid().cursor(), (0, 2));
    }

    /// Nor does a bell, which is not motion either.
    #[test]
    fn a_bell_does_not_break_a_grapheme_apart() {
        let term = screen(4, 20, "e\x07\u{0301}");
        assert_eq!(term.grid().line(0), "e\u{0301}");
    }

    /// A sequence whose width falls as it completes is still one grapheme.
    ///
    /// `🏴 ZWJ Ⓜ FE0F` measures **2**, but its three-scalar prefix `🏴 ZWJ Ⓜ`
    /// measures **3** — `Ⓜ` only becomes emoji-presentation once the selector
    /// arrives. A prefix's width is not monotone toward the final answer, so
    /// "did adding this scalar cost a column" cannot decide a boundary. That
    /// was the rule this replaced, and it split the sequence in two.
    ///
    /// The boundary is UAX #29's answer; the width is the width tables'. They
    /// are different questions and fusing them is what broke.
    #[test]
    fn a_sequence_whose_width_falls_is_still_one_grapheme() {
        let term = screen(4, 20, "\u{1F3F4}\u{200D}\u{24C2}\u{FE0F}X");
        assert_eq!(term.grid().cursor(), (0, 3), "the sequence was split apart");
        assert_eq!(term.grid().cell(0, 0).map(|c| c.ch), Some('\u{1F3F4}'));
        assert!(term.grid().cell(0, 1).unwrap().is_continuation());
        assert_eq!(term.grid().cell(0, 2).map(|c| c.ch), Some('X'));
        assert_eq!(term.grid().line(0), "\u{1F3F4}\u{200D}\u{24C2}\u{FE0F}X");
    }

    /// A cluster that borrowed a third column gives it back.
    ///
    /// The same sequence without anything after it: while `🏴 ZWJ Ⓜ` was on
    /// screen the placement was three columns wide, and the selector takes it
    /// back to two. A placement that could only grow would leave the third
    /// column claimed by a glyph that no longer reaches it.
    #[test]
    fn a_cluster_that_narrows_gives_back_the_column_it_borrowed() {
        let term = screen(4, 20, "\u{1F3F4}\u{200D}\u{24C2}\u{FE0F}");
        assert_eq!(term.grid().cursor(), (0, 2));
        assert_eq!(
            term.grid().cell(0, 2).map(|c| c.ch),
            Some(' '),
            "the borrowed column is still claimed"
        );
    }

    /// A flag is two regional indicators and one glyph.
    ///
    /// Each indicator measures 1 alone and the pair measures 2, so the width
    /// rule saw "adding it cost a column" and started a second cell. The
    /// columns happened to come out right, but the flag was two cells and the
    /// window drew it as two letters.
    #[test]
    fn a_regional_indicator_pair_is_one_flag() {
        let term = screen(4, 20, "\u{1F1F0}\u{1F1F7}X");
        assert_eq!(term.grid().cursor(), (0, 3));
        assert_eq!(term.grid().cell(0, 0).map(|c| c.ch), Some('\u{1F1F0}'));
        assert!(
            term.grid().cell(0, 1).unwrap().is_continuation(),
            "the flag is two cells, so the window draws two letters"
        );
        assert_eq!(term.grid().cell(0, 2).map(|c| c.ch), Some('X'));
    }

    /// A sequence the grid does not implement moves nothing.
    ///
    /// `ESC[?1J` is private, falls through to the swallow arm, and touches
    /// neither the cursor nor a cell — but the guard was keyed on the action
    /// character rather than on what actually ran, so it threw the next
    /// combining mark away. The same defect as clearing on `SGR`, one round
    /// later and one level deeper.
    #[test]
    fn a_swallowed_sequence_does_not_break_a_grapheme_apart() {
        let term = screen(4, 20, "e\x1b[?1J\u{0301}!");
        assert_eq!(term.grid().line(0), "e\u{0301}!", "the accent was dropped");
        assert_eq!(term.grid().cursor(), (0, 2));
    }

    /// A cluster can be wider than two columns, and everything that clears
    /// one has to know that.
    ///
    /// `🏴 ZWJ Ⓜ` — the same sequence as above but without the selector that
    /// narrows it — measures **three** columns. The cell bookkeeping was
    /// written when a wide glyph was always a pair, so overwriting any part of
    /// a three-column cluster cleared two of its cells and left the third
    /// behind: a head still claiming a column it no longer covers, or a
    /// continuation with nothing in front of it.
    #[test]
    fn overwriting_a_three_column_cluster_takes_all_of_it() {
        // `ESC[H` home, so the `x` lands on the cluster's own head.
        let term = screen(4, 20, "\u{1F3F4}\u{200D}\u{24C2}\x1b[Hx");
        assert_eq!(term.grid().cell(0, 0).map(|c| c.ch), Some('x'));
        assert_eq!(
            term.grid().cell(0, 1).map(|c| c.ch),
            Some(' '),
            "the second column still continues a glyph that is gone"
        );
        assert_eq!(
            term.grid().cell(0, 2).map(|c| c.ch),
            Some(' '),
            "the third column still continues a glyph that is gone"
        );
    }

    /// The same from the other end: landing on the last of three columns has
    /// to take the head two cells to the left with it.
    #[test]
    fn overwriting_the_far_end_of_a_three_column_cluster_takes_its_head() {
        let term = screen(4, 20, "\u{1F3F4}\u{200D}\u{24C2}\x1b[1;3Hx");
        assert_eq!(term.grid().cell(0, 2).map(|c| c.ch), Some('x'));
        assert_eq!(
            term.grid().cell(0, 0).map(|c| c.ch),
            Some(' '),
            "the head is still claiming three columns it no longer has"
        );
        assert_eq!(term.grid().cell(0, 1).map(|c| c.ch), Some(' '));
    }

    /// And a re-fit has to pull a three-column cluster back too.
    ///
    /// `refit_line` only recognised a head measuring exactly two, so a
    /// narrower screen cut a three-column glyph in half and left the rest
    /// hanging off the right edge — the very defect it exists to prevent, one
    /// column wider than it knew about.
    #[test]
    fn a_three_column_cluster_is_re_fitted_whole() {
        let mut term = screen(4, 6, "abc\u{1F3F4}\u{200D}\u{24C2}");
        assert_eq!(term.grid().cell(0, 3).map(|c| c.ch), Some('\u{1F3F4}'));
        term.grid_mut().resize(4, 5);
        assert_eq!(
            term.grid().cell(0, 3).map(|c| c.ch),
            Some(' '),
            "a three-column glyph is still claiming columns the row lost"
        );
        assert_eq!(term.grid().cell(0, 4).map(|c| c.ch), Some(' '));
    }

    /// Erasing from inside a three-column cluster takes the whole thing.
    ///
    /// `ESC[K` clears from the cursor to the end of the row, and the cursor
    /// can be sitting on the middle column of a glyph. The part outside the
    /// erased span is what gets left behind: a head whose glyph no longer
    /// reaches the columns it claims.
    #[test]
    fn erasing_to_the_end_from_inside_a_cluster_takes_its_head() {
        // Home, one column forward: the cursor is on the cluster's middle.
        let term = screen(4, 20, "\u{1F3F4}\u{200D}\u{24C2}z\x1b[H\x1b[1C\x1b[K");
        assert_eq!(
            term.grid().cell(0, 0).map(|c| c.ch),
            Some(' '),
            "the head survived an erase that took the rest of its glyph"
        );
        assert_eq!(term.grid().line(0), "", "the row is not actually empty");
    }

    /// And from the other side: `ESC[1K` clears the start of the row through
    /// the cursor, so the columns *after* it are the ones left orphaned.
    #[test]
    fn erasing_to_the_cursor_from_inside_a_cluster_takes_its_tail() {
        let term = screen(4, 20, "\u{1F3F4}\u{200D}\u{24C2}z\x1b[H\x1b[1C\x1b[1K");
        assert_eq!(term.grid().cell(0, 1).map(|c| c.ch), Some(' '));
        assert_eq!(
            term.grid().cell(0, 2).map(|c| c.ch),
            Some(' '),
            "a column is still continuing a glyph the erase removed"
        );
        assert_eq!(term.grid().line(0), "   z", "the row lost the wrong cells");
    }

    /// A terminal answers when it is asked what it is.
    ///
    /// `CSI c` is Device Attributes — a **question**, not an instruction. A
    /// program sends it at startup to learn what it is talking to and waits
    /// for the answer. Swallowing it, which is what "unsupported sequences are
    /// swallowed" did, leaves the program waiting until its own timeout: on
    /// screen that is a UI that does not appear and an app that feels slow.
    ///
    /// Measured, not guessed — starting `claude` in a pty emits `CSI c` twice
    /// before it draws anything.
    #[test]
    fn a_device_attributes_query_is_answered() {
        let mut term = Terminal::new(4, 20);
        term.feed(b"\x1b[c");
        assert_eq!(
            term.grid_mut().take_replies(),
            b"\x1b[?1;2c",
            "the terminal did not say what it is, so the program is still waiting"
        );
        // Asked once, answered once: a reply left in the queue would be sent
        // again on the next pump and read as a second, stray answer.
        assert!(term.grid_mut().take_replies().is_empty());
        // `CSI 0 c` is the same question spelled out.
        term.feed(b"\x1b[0c");
        assert_eq!(term.grid_mut().take_replies(), b"\x1b[?1;2c");
        // Every other parameter makes it a question this is not the answer to.
        term.feed(b"\x1b[1c");
        assert!(
            term.grid_mut().take_replies().is_empty(),
            "a non-primary DA query was answered with the primary reply"
        );
    }

    /// The identity is a platform's, not a preference: ConPTY 1.22 and later
    /// blocks at spawn until it sees a reply it accepts.
    #[test]
    fn the_identity_a_terminal_gives_can_be_the_one_conpty_unblocks_on() {
        let mut term = Terminal::new(4, 20);
        term.grid_mut().set_identity(DeviceIdentity::ConPty);
        term.feed(b"\x1b[c");
        assert_eq!(term.grid_mut().take_replies(), b"\x1b[?61;4c");
    }

    /// `CSI > c` is a different question wearing the same final letter, and
    /// answering it with the primary reply is how a program that branches on
    /// the answer takes the wrong turn.
    #[test]
    fn the_secondary_attributes_question_gets_its_own_answer() {
        let mut term = Terminal::new(4, 20);
        term.feed(b"\x1b[>c");
        assert_eq!(term.grid_mut().take_replies(), b"\x1b[>0;276;0c");
    }

    /// `CSI > q` asks the terminal's name and version (XTVERSION), and a
    /// program that gets no answer takes the most cautious road it has.
    ///
    /// Measured 2026-09-17 on Claude Code 2.1.273: it asks this before it
    /// draws, and only once it is answered does it ask DECRQM 2026 and wrap its
    /// frames in synchronized output. Unanswered, its fullscreen repaints came
    /// through unsynchronized — 0 of 0 frames wrapped in a 7 s run with a
    /// resize, against 3 of 3 once answered — so the grid's frame hold was
    /// never used by the program that repaints the most.
    #[test]
    fn the_version_question_is_answered_with_this_terminals_name() {
        let answer = format!("\x1bP>|{TERMINAL_NAME}({TERMINAL_VERSION})\x1b\\");
        let mut term = Terminal::new(4, 20);
        term.feed(b"\x1b[>0q");
        assert_eq!(
            term.grid_mut().take_replies(),
            answer.as_bytes(),
            "the program asked which terminal it is in and heard nothing"
        );
        // `CSI > q` is the same question with the parameter left out.
        term.feed(b"\x1b[>q");
        assert_eq!(term.grid_mut().take_replies(), answer.as_bytes());
        // Any other parameter is not this question.
        term.feed(b"\x1b[>1q");
        assert!(
            term.grid_mut().take_replies().is_empty(),
            "a question that is not XTVERSION was answered as if it were"
        );
        // And nothing of it reaches the screen.
        assert_eq!(term.grid().visible_text().trim(), "");
    }

    /// And when it is asked where the cursor is.
    #[test]
    fn a_cursor_position_query_is_answered_with_the_cursor() {
        let mut term = Terminal::new(10, 40);
        term.feed(b"abc\x1b[6n");
        // Reported one-based, which is what the sequence means.
        assert_eq!(term.grid_mut().take_replies(), b"\x1b[1;4R");
    }

    /// Saving and restoring the cursor is a matched pair a TUI leans on.
    ///
    /// `claude` emits `ESC 7` … `ESC 8` around its scroll-region setup. With
    /// neither implemented the cursor is wherever the sequence left it, and
    /// everything drawn afterwards lands in the wrong place.
    #[test]
    fn the_cursor_can_be_saved_and_restored() {
        let mut term = Terminal::new(6, 20);
        term.feed(b"\x1b[3;5H\x1b7\x1b[1;1Hx\x1b8");
        assert_eq!(term.grid().cursor(), (2, 4));
        assert_eq!(term.grid().cell(0, 0).map(|c| c.ch), Some('x'));
    }

    /// A scroll region keeps the rest of the screen still.
    ///
    /// `CSI r` is how a TUI reserves a header or a status line: scrolling
    /// inside the region must not move the rows outside it. Without it a
    /// program that reserves the bottom line watches its header scroll away.
    #[test]
    fn scrolling_stays_inside_the_region_that_was_set() {
        let mut term = Terminal::new(5, 10);
        term.feed(b"\x1b[HkeepA\r\n\x1b[2;4r\x1b[4;1Hlast\r\n");
        assert_eq!(
            term.grid().line(0),
            "keepA",
            "a row outside the region scrolled with it"
        );
        assert_eq!(term.grid().line(2), "last", "the region did not scroll");
    }

    /// Moving to a column, which is how most TUIs start every line they draw.
    #[test]
    fn the_cursor_can_be_put_on_a_column_or_a_row_directly() {
        let mut term = Terminal::new(4, 20);
        term.feed(b"\x1b[8Gx\x1b[3dy");
        assert_eq!(term.grid().cell(0, 7).map(|c| c.ch), Some('x'));
        // Row absolute keeps the column, which is what the sequence means —
        // the cursor was at 8 after printing the x, so the y lands there.
        assert_eq!(term.grid().cell(2, 8).map(|c| c.ch), Some('y'));
    }

    /// Erasing a run of cells without redrawing the rest of the line.
    #[test]
    fn a_run_of_cells_can_be_erased_in_place() {
        let mut term = Terminal::new(4, 20);
        term.feed(b"abcdef\x1b[1;2H\x1b[3X");
        assert_eq!(term.grid().line(0), "a   ef");
    }

    /// A program that hides the cursor gets a hidden cursor.
    ///
    /// `CSI ?25l` is what a TUI sends before it redraws, and `?25h` when it
    /// wants the caret back on its own prompt. Ignoring it left a caret
    /// sitting in the middle of a drawn interface, where the program had
    /// deliberately put none. Measured: `claude` sends both on startup.
    #[test]
    fn hiding_the_cursor_hides_it() {
        let mut term = Terminal::new(4, 20);
        assert!(
            term.grid().cursor_visible(),
            "a fresh terminal shows a caret"
        );
        term.feed(b"\x1b[?25l");
        assert!(!term.grid().cursor_visible());
        let delta = term.grid_mut().take_delta().expect("the change is a frame");
        assert!(!delta.cursor_visible, "the window was never told");
        term.feed(b"\x1b[?25h");
        assert!(term.grid().cursor_visible());
    }

    /// A frame the program declared atomic is not painted half-drawn.
    ///
    /// `CSI ?2026h` means "hold everything until I say done" — it is how a
    /// TUI stops a repaint being seen in pieces. Without it the window paints
    /// whatever arrived at the 16ms tick, so a popup being drawn is shown
    /// mid-draw and then completed: on screen that is a flicker, or a panel
    /// that appears to flash and vanish. Measured: `claude` brackets its
    /// frames with it.
    #[test]
    fn a_synchronised_frame_is_held_until_it_is_complete() {
        let mut term = Terminal::new(4, 20);
        term.feed(b"settled");
        term.grid_mut().take_delta();

        term.feed(b"\x1b[?2026h");
        term.feed(b"\x1b[HHALF");
        assert!(
            term.grid_mut().take_delta().is_none(),
            "a frame the program said was incomplete was handed over anyway"
        );
        term.feed(b"DONE\x1b[?2026l");
        let delta = term.grid_mut().take_delta().expect("the finished frame");
        assert!(
            delta.rows.iter().any(|row| row.index == 0),
            "the held rows were lost instead of held"
        );
        assert_eq!(term.grid().line(0), "HALFDONE");
    }

    /// A program cannot freeze the screen by never closing its frame.
    ///
    /// The hold is a courtesy, not a lock: a program that sets `?2026h` and
    /// then crashes or forgets would otherwise leave the window showing a
    /// stale screen forever. Real terminals time the hold out, and so does
    /// this one.
    #[test]
    fn a_frame_left_open_is_given_up_on() {
        let mut term = Terminal::new(4, 20);
        term.feed(b"\x1b[?2026h\x1b[Hstuck");
        assert!(term.grid_mut().take_delta().is_none());
        term.grid_mut().expire_sync_for_test();
        assert!(
            term.grid_mut().take_delta().is_some(),
            "an unclosed frame held the screen forever"
        );
    }

    /// Focus reporting is remembered, so the window knows whether to send it.
    #[test]
    fn focus_reporting_is_remembered_when_a_program_asks_for_it() {
        let mut term = Terminal::new(4, 20);
        assert!(!term.grid().focus_reporting());
        term.feed(b"\x1b[?1004h");
        assert!(term.grid().focus_reporting());
        term.feed(b"\x1b[?1004l");
        assert!(!term.grid().focus_reporting());
    }

    /// The tail crosses to the window whole, as one string.
    ///
    /// On the packed wire the riders sit beside their COLUMN (`"zw":[[0,"…"]]`)
    /// — still one string per cluster, still absent entirely for a row where
    /// nothing rides, and the round trip puts every scalar back on the cell
    /// it left.
    #[test]
    fn the_tail_crosses_whole_and_costs_nothing_when_empty() {
        let mut term = Terminal::new(4, 20);
        term.feed("e\u{0301}\u{0302}a".as_bytes());
        let delta = term.grid_mut().take_delta().expect("a frame");
        let wire = serde_json::to_string(&delta).expect("serialises");

        assert!(
            wire.contains("\"zw\":[[0,\"\u{0301}\u{0302}\"]]"),
            "the tail did not cross whole, at its column: {wire}"
        );
        let back: GridDelta = serde_json::from_str(&wire).expect("round-trips");
        assert_eq!(
            back.rows[0].cells[0].zw.scalars().collect::<String>(),
            "\u{0301}\u{0302}"
        );
        assert!(
            back.rows[0].cells[1].zw.is_empty(),
            "a plain cell came back carrying something"
        );
        assert_eq!(
            wire.matches("\"zw\"").count(),
            1,
            "a rider-free row still paid for the key: {wire}"
        );
    }

    /// What a cell costs in memory, pinned.
    ///
    /// Every lane keeps thousands of scrollback lines of these, so the size is
    /// per-lane memory and not just a wire question. Holding the cluster
    /// inline is what keeps [`Cell`] `Copy`; this is the bill for that, stated
    /// so it cannot creep.
    #[test]
    fn a_cell_stays_small_enough_to_keep_thousands_of_them() {
        assert!(
            std::mem::size_of::<Cell>() <= 48,
            "a cell grew to {} bytes",
            std::mem::size_of::<Cell>()
        );
        assert!(
            std::mem::size_of::<ZeroWidth>() <= 20,
            "the tail grew to {} bytes",
            std::mem::size_of::<ZeroWidth>()
        );
    }

    /// A frame of ordinary text must not cost a kilobyte a row.
    ///
    /// This delta is the window's hot path: the pump emits one every 16ms and
    /// Tauri serialises it to JSON for the webview to parse. Almost every cell
    /// a terminal ever draws is unstyled, so a cell that spells out seven
    /// default style fields pays ~118 bytes to say "plain a" — around ten
    /// times what the character is worth, on every cell of every frame. That
    /// is the whole of the lag a person feels while a program prints.
    #[test]
    fn a_plain_frame_does_not_pay_for_styles_nobody_set() {
        let mut term = Terminal::new(24, 80);
        term.feed(b"the quick brown fox jumps over the lazy dog");
        let delta = term.grid_mut().take_delta().expect("a frame");
        let wire = serde_json::to_string(&delta).expect("serialises");

        // One 80-column row. Spelling out every default style costs ~9.5 KiB;
        // sending only what was actually set costs a few hundred bytes.
        assert!(
            wire.len() < 2_000,
            "a plain row costs {} bytes on the wire",
            wire.len()
        );
    }

    /// Shrinking the wire must not lose what a program actually set.
    #[test]
    fn a_styled_cell_still_carries_its_style() {
        let mut term = Terminal::new(4, 20);
        term.feed(b"\x1b[1;31mred\x1b[0m");
        let delta = term.grid_mut().take_delta().expect("a frame");
        let wire = serde_json::to_string(&delta).expect("serialises");
        let back: GridDelta = serde_json::from_str(&wire).expect("round-trips");

        let cell = back.rows[0].cells[0];
        assert_eq!(cell.ch, 'r');
        assert!(cell.style.bold);
        assert_eq!(cell.style.fg, Color::Indexed(1));
        assert_eq!(back.rows[0].cells[3].style, CellStyle::default());
    }

    /// A full frame taken as a new consumer floor also becomes the next
    /// delta's floor.
    ///
    /// The ten shifts before the snapshot are already IN that full frame. If
    /// they survive in `scrolled_since_take`, applying the next delta to the
    /// snapshot scrolls those same ten lines a second time. Dirty rows are the
    /// other half of the same baseline: rows written before the snapshot must
    /// not be resent as though they happened afterwards.
    #[test]
    fn a_taken_snapshot_resets_the_delta_baseline() {
        let mut term = Terminal::new(20, 16);
        term.feed(b"baseline");
        let _ = term.grid_mut().take_delta();

        term.feed(b"\x1b[20;1H");
        for n in 0..10 {
            term.feed(format!("old{n}\r\n").as_bytes());
        }
        let shot = term.grid_mut().take_snapshot();
        assert!(shot.full, "the new floor is not a full frame");

        for n in 0..5 {
            term.feed(format!("new{n}\r\n").as_bytes());
        }
        let delta = term
            .grid_mut()
            .take_delta()
            .expect("the post-snapshot writes are a frame");
        assert_eq!(
            delta.scrolled_lines, 5,
            "the ten shifts already represented by the snapshot crossed again"
        );
        assert_eq!(
            delta.rows.iter().map(|row| row.index).collect::<Vec<_>>(),
            (14..20).collect::<Vec<_>>(),
            "rows dirtied before the snapshot leaked into its successor"
        );
        let said = delta
            .rows
            .iter()
            .map(|row| {
                row.cells
                    .iter()
                    .map(|cell| cell.ch)
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect::<Vec<_>>();
        assert_eq!(said, ["new0", "new1", "new2", "new3", "new4", ""]);
    }

    /// A combining mark owns no column, but it is still content.
    ///
    /// Giving it a cell of its own made the line a column longer than the
    /// child laid out — the drift this whole change exists to end. Dropping it
    /// instead is the other wrong answer: `e\u{301}` silently becomes `e` and
    /// the accent is gone with no way to tell. It belongs to the character in
    /// front of it, so it travels in that cell.
    #[test]
    fn a_combining_mark_stays_with_the_letter_it_modifies() {
        let term = screen(4, 20, "e\u{0301}!");
        assert_eq!(term.grid().cursor(), (0, 2), "the mark took a column");
        let base = term.grid().cell(0, 0).expect("the letter is there");
        assert_eq!(base.ch, 'e');
        assert_eq!(
            base.zw.scalars().collect::<String>(),
            "\u{0301}",
            "the accent was thrown away"
        );
        assert_eq!(term.grid().line(0), "e\u{0301}!");
    }

    /// The joiner that fuses two emoji is zero-width too, and losing it turns
    /// one glyph into two.
    #[test]
    fn a_zero_width_joiner_is_not_dropped() {
        let term = screen(4, 20, "\u{1F468}\u{200D}\u{1F469}");
        assert_eq!(
            term.grid()
                .cell(0, 0)
                .map(|c| c.zw.scalars().collect::<String>()),
            Some("\u{200D}\u{1F469}".to_string()),
            "the join was lost, so the pair renders as two people"
        );
        assert_eq!(term.grid().line(0), "\u{1F468}\u{200D}\u{1F469}");
    }

    /// Leaving the alternate screen re-fits the buffer that was parked, and
    /// that is a second place a pair can be cut in half.
    ///
    /// `resize` blanks a glyph whose second column it just truncated, but the
    /// primary screen is not `self.cells` while a full-screen program is up —
    /// it is parked, untouched by the resize, and re-fitted on the way back.
    /// Without the same cleanup there, quitting an editor in a window that was
    /// narrowed leaves a glyph overhanging the right edge.
    #[test]
    fn coming_back_from_the_alternate_screen_does_not_leave_a_glyph_overhanging() {
        let mut term = screen(4, 6, "abcd가");
        assert_eq!(term.grid().cell(0, 4).map(|c| c.ch), Some('가'));
        term.feed(b"\x1b[?1049h");
        term.grid_mut().resize(4, 5);
        term.feed(b"\x1b[?1049l");
        assert_eq!(
            term.grid().cell(0, 4).map(|c| c.ch),
            Some(' '),
            "a glyph is still claiming a column the row no longer has"
        );
    }

    /// Ten lines through a three-row screen: line8 and line9 remain visible
    /// with a blank prompt row under them, and eight lines are history.
    fn scrolled_screen() -> Terminal {
        let mut term = Terminal::new(3, 10);
        for n in 0..10 {
            term.feed(format!("line{n}\r\n").as_bytes());
        }
        term
    }

    fn padded_row(cols: usize, text: &str) -> Vec<Cell> {
        let mut row = vec![Cell::default(); cols];
        for (at, ch) in text.chars().enumerate() {
            row[at].ch = ch;
        }
        row
    }

    /// Compact storage is an implementation detail: every reader must still
    /// see the full logical row that existed before its default tail was
    /// removed. This one fixture covers the cells accessor, a scrolled-back
    /// frame, text extraction, literal search, and wrapped-line flattening.
    ///
    /// The rows deliberately include all of the shapes that make "blank"
    /// subtle: mixed style, a double-width Korean glyph, a soft wrap, a style
    /// only at the tail, and a BCE-shaped background-painted blank at the
    /// tail. The last two are not `Cell::default()` and therefore must stay.
    #[test]
    fn compact_scrollback_reads_exactly_like_full_width_rows() {
        let cols = 12;
        let mut grid = TerminalGrid::new(6, cols);

        let mut styled = padded_row(cols, "styled");
        styled[1].style = CellStyle {
            fg: Color::Indexed(1),
            bold: true,
            ..CellStyle::default()
        };
        drop(grid.push_scrollback(styled, None, None));

        let mut korean = padded_row(cols, "");
        korean[0].ch = '한';
        korean[1].ch = CONTINUATION;
        korean[2].ch = '글';
        korean[3].ch = CONTINUATION;
        drop(grid.push_scrollback(korean, None, None));

        drop(grid.push_scrollback(padded_row(cols, "wrap"), None, None));
        drop(grid.push_scrollback(padded_row(cols, "tail"), Some(4), None));

        let mut styled_tail = padded_row(cols, "ink");
        styled_tail[cols - 2].style = CellStyle {
            underline: true,
            ..CellStyle::default()
        };
        drop(grid.push_scrollback(styled_tail, None, None));

        let mut bce_tail = padded_row(cols, "bar");
        bce_tail[cols - 1].style = CellStyle {
            bg: Color::Indexed(4),
            ..CellStyle::default()
        };
        drop(grid.push_scrollback(bce_tail, None, None));

        assert_eq!(
            grid.scrollback.iter().map(Vec::len).collect::<Vec<_>>(),
            [6, 4, 4, 4, 11, 12],
            "storage did not stop at each row's last non-default cell"
        );

        let full_cells = (0..grid.scrollback_len())
            .map(|index| {
                grid.scrollback_cells(index)
                    .expect("history row")
                    .iter()
                    .copied()
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        assert!(full_cells[1][1].is_continuation());
        assert!(full_cells[1][3].is_continuation());
        assert!(full_cells[4][cols - 2].style.underline);
        assert_eq!(full_cells[5][cols - 1].style.bg, Color::Indexed(4));
        grid.scroll_view(grid.scrollback_len() as isize);
        let full_view = grid.view_rows();
        let full_lines = (0..grid.scrollback_len())
            .map(|index| grid.scrollback_line(index))
            .collect::<Vec<_>>();
        let wrapped_hit = grid.search("wraptail", true, 10);
        let default_tail_hit = grid.search("tail  ", true, 10);
        assert_eq!(wrapped_hit.len(), 1, "the fixture lost its soft wrap");
        assert!(
            !default_tail_hit.is_empty(),
            "the fixture has no logical default-tail hit"
        );

        // Emulate the representation this change introduces before changing
        // any reader. On the old implementation this makes the assertions
        // below red; after the surgery it is already a no-op.
        for row in &mut grid.scrollback {
            let used = row
                .iter()
                .rposition(|cell| *cell != Cell::default())
                .map_or(0, |at| at + 1);
            row.truncate(used);
            row.shrink_to_fit();
        }

        for (index, expected) in full_cells.iter().enumerate() {
            let actual = grid.scrollback_cells(index).expect("history row");
            assert_eq!(
                actual.iter().copied().collect::<Vec<_>>(),
                *expected,
                "the cells accessor exposed compact storage for row {index}"
            );
        }
        assert_eq!(
            grid.view_rows(),
            full_view,
            "the history view became narrow"
        );
        assert_eq!(
            (0..grid.scrollback_len())
                .map(|index| grid.scrollback_line(index))
                .collect::<Vec<_>>(),
            full_lines,
            "text extraction changed"
        );
        assert_eq!(
            grid.search("wraptail", true, 10),
            wrapped_hit,
            "a wrapped logical line changed"
        );
        assert_eq!(
            grid.search("tail  ", true, 10),
            default_tail_hit,
            "logical default cells vanished from search"
        );
    }

    /// A one-character transcript line must not retain a screen-wide
    /// allocation. The small allowance admits allocator rounding without
    /// weakening the property being bought: capacity follows used width, not
    /// terminal width.
    #[test]
    fn compact_scrollback_capacity_tracks_the_written_width() {
        let cols = 95;
        let mut grid = TerminalGrid::new(2, cols);
        drop(grid.push_scrollback(padded_row(cols, "x"), None, None));
        drop(grid.push_scrollback(padded_row(cols, ""), None, None));

        let one = &grid.scrollback[0];
        assert_eq!(one.len(), 1, "default cells were retained");
        assert!(
            one.capacity() <= one.len() + 2,
            "one used cell retained capacity {}",
            one.capacity()
        );
        let blank = &grid.scrollback[1];
        assert!(blank.is_empty(), "a default-only row retained cells");
        assert!(
            blank.capacity() <= 2,
            "a default-only row retained capacity {}",
            blank.capacity()
        );
    }

    /// History can contain several geometries: rows already in scrollback
    /// keep the width they had before a resize, while rows evicted by the
    /// resize have first been re-fitted to the new width. Compact rows must
    /// retain that per-row geometry rather than being padded to today's
    /// `grid.cols` indiscriminately.
    #[test]
    fn resizing_across_compact_history_preserves_each_rows_geometry() {
        let mut grid = TerminalGrid::new(3, 12);
        drop(grid.push_scrollback(padded_row(12, "old"), None, None));
        grid.cells[0] = padded_row(12, "a");
        grid.cells[1] = padded_row(12, "bb");

        grid.resize(1, 5);
        assert_eq!(grid.scrollback_len(), 3);
        let widths = (0..grid.scrollback_len())
            .map(|index| grid.scrollback_cells(index).expect("history row").len())
            .collect::<Vec<_>>();
        assert_eq!(
            widths,
            [12, 5, 5],
            "rows were not read at the geometry in which they left the screen"
        );
        assert_eq!(
            grid.scrollback.iter().map(Vec::len).collect::<Vec<_>>(),
            [3, 1, 2],
            "the resized rows were not stored compactly"
        );

        grid.resize(1, 9);
        let widths_after = (0..grid.scrollback_len())
            .map(|index| grid.scrollback_cells(index).expect("history row").len())
            .collect::<Vec<_>>();
        assert_eq!(
            widths_after, widths,
            "a later resize rewrote historical geometry"
        );
        assert_eq!(grid.search("old     ", true, 10).len(), 1);
        assert_eq!(grid.search("bb ", true, 10).len(), 1);
    }

    /// Scrolling back shows history, absolutely and without a caret.
    ///
    /// A quiet scroll back is a view shift — how far the window moved and the
    /// rows it exposed — and history has no caret either way: the cursor's
    /// coordinates name a place on the live screen, which is not what is
    /// being shown. A move as long as the screen has no rows to keep, so it
    /// arrives `full` with the whole window: history first, then the live
    /// screen's remainder.
    #[test]
    fn scrolling_back_shows_history_as_a_view_shift_without_a_caret() {
        let mut term = scrolled_screen();
        let _ = term.grid_mut().take_delta();
        term.grid_mut().scroll_view(2);
        let delta = term.grid_mut().take_delta().expect("a scroll is a frame");
        assert!(!delta.full, "a quiet scroll back resent the whole window");
        assert_eq!(
            delta.view_shift, -2,
            "two rows back: the rows move down by two"
        );
        assert!(!delta.cursor_visible, "history has no caret");
        let text = |row: &GridRow| {
            row.cells
                .iter()
                .map(|c| c.ch)
                .collect::<String>()
                .trim_end()
                .to_string()
        };
        assert_eq!(
            delta
                .rows
                .iter()
                .map(|row| (row.index, text(row)))
                .collect::<Vec<_>>(),
            vec![(0, "line6".into()), (1, "line7".into())],
            "two lines back from line8: only the exposed top travels"
        );
        term.grid_mut().scroll_view(-2);
        let _ = term.grid_mut().take_delta();
        term.grid_mut().scroll_view(3);
        let far = term.grid_mut().take_delta().expect("a scroll is a frame");
        assert!(far.full, "a screen-long move has no rows to keep");
        assert_eq!(far.view_shift, 0);
        assert!(!far.cursor_visible, "history has no caret");
        assert_eq!(far.rows[0].index, 0);
        assert_eq!(text(&far.rows[0]), "line5", "three lines back from line8");
        // The window is history first, then the live screen's remainder.
        assert_eq!(text(&far.rows[2]), "line7");
    }

    /// The view is clamped to what the scrollback holds, on both ends.
    #[test]
    fn the_view_stops_at_the_oldest_line_and_at_live() {
        let mut term = scrolled_screen();
        term.grid_mut().scroll_view(100);
        assert_eq!(term.grid().view_offset(), 8, "eight lines left the screen");
        term.grid_mut().scroll_view(-100);
        assert_eq!(term.grid().view_offset(), 0);
    }

    /// New output does not drag a reader away from what they are reading:
    /// the offset deepens as lines enter history, so the same content stays
    /// put — until the buffer's cap starts eating the oldest lines.
    #[test]
    fn output_while_scrolled_keeps_the_content_under_the_reader() {
        let mut term = scrolled_screen();
        term.grid_mut().scroll_view(3);
        let _ = term.grid_mut().take_delta();
        term.feed(b"line10\r\n");
        let delta = term.grid_mut().take_delta().expect("output is a frame");
        // This is the frame where the flag matters most: nothing set
        // `full_repaint`, so only the scrolled-back rule can make it
        // absolute. A view frame with `full` false and a nonzero shift makes
        // the consumer slide rows that are already absolute.
        assert!(delta.full, "a view frame went out as an increment");
        assert_eq!(
            delta.scrolled_lines, 0,
            "a shift rode along with a view frame"
        );
        let first: String = delta.rows[0].cells.iter().map(|c| c.ch).collect();
        assert_eq!(
            first.trim_end(),
            "line5",
            "the line under the reader changed while they were reading it"
        );
        assert_eq!(term.grid().view_offset(), 4, "one deeper per new line");
    }

    /// A keystroke is aimed at the program, and the program is at the bottom.
    #[test]
    fn coming_back_to_the_bottom_shows_the_live_screen_again() {
        let mut term = scrolled_screen();
        term.grid_mut().scroll_view(5);
        let _ = term.grid_mut().take_delta();
        term.grid_mut().view_to_bottom();
        let delta = term.grid_mut().take_delta().expect("the return is a frame");
        assert!(delta.full);
        assert!(delta.cursor_visible);
        let first: String = delta.rows[0].cells.iter().map(|c| c.ch).collect();
        assert_eq!(first.trim_end(), "line8", "the live screen's own top row");
    }

    /// The alternate screen has no history to scroll: a full-screen program
    /// owns its whole surface, and the view refuses rather than clamps.
    #[test]
    fn the_alternate_screen_refuses_to_scroll() {
        let mut term = scrolled_screen();
        term.feed(b"\x1b[?1049h");
        term.grid_mut().scroll_view(3);
        assert_eq!(term.grid().view_offset(), 0);
        // And history being read on the primary screen belongs to it: a TUI
        // starting mid-read swaps the whole surface, so the view resets.
        term.feed(b"\x1b[?1049l");
        term.grid_mut().scroll_view(3);
        term.feed(b"\x1b[?1049h");
        assert_eq!(term.grid().view_offset(), 0, "the swap kept a stale view");
    }

    /// The wire carries a row PACKED — its text once, style runs only where
    /// ink was set — and both spellings come back as the same cells. The size
    /// claim is asserted, not narrated: a plain prompt row must cost a
    /// fraction of the object-per-cell form it replaces, because that soup is
    /// what typing lag was made of on the receiving thread.
    #[test]
    fn a_row_crosses_the_wire_packed_and_comes_back_whole() {
        let styled = CellStyle {
            bold: true,
            ..CellStyle::default()
        };
        let mut cells: Vec<Cell> = "error: two left"
            .chars()
            .map(|ch| Cell {
                ch,
                ..Cell::default()
            })
            .collect();
        for cell in cells.iter_mut().take(6) {
            cell.style = styled;
        }
        // A double-width glyph's continuation column and a combining rider,
        // both of which the packing must carry column-exactly.
        cells.push(Cell {
            ch: '한',
            ..Cell::default()
        });
        cells.push(Cell {
            ch: CONTINUATION,
            ..Cell::default()
        });
        cells.push(Cell {
            ch: 'e',
            zw: ZeroWidth::from("\u{301}".to_string()),
            ..Cell::default()
        });
        let row = GridRow {
            index: 7,
            cells,
            wrap: None,
            fold: None,
        };

        let packed = serde_json::to_string(&row).expect("serialize");
        assert!(
            packed.contains("\"text\""),
            "the wire lost its packing:\n{packed}"
        );
        assert!(
            !packed.contains("\"cells\""),
            "the wire still speaks one object per cell:\n{packed}"
        );
        // The six bold columns are ONE run.
        assert_eq!(
            packed.matches("[0,6,").count(),
            1,
            "runs did not coalesce:\n{packed}"
        );
        let back: GridRow = serde_json::from_str(&packed).expect("deserialize packed");
        assert_eq!(back, row, "the packed round trip changed the row");

        // The cell spelling still deserializes — the window's gates and any
        // stored snapshot speak it.
        let cell_form = serde_json::json!({
            "index": 7,
            "cells": row.cells.iter().map(|cell| {
                let mut one = serde_json::Map::new();
                one.insert("ch".into(), serde_json::json!(cell.ch));
                if !cell.style.is_plain() {
                    one.insert("style".into(), serde_json::to_value(cell.style).unwrap());
                }
                if !cell.zw.is_empty() {
                    one.insert("zw".into(), serde_json::json!(String::from(cell.zw)));
                }
                serde_json::Value::Object(one)
            }).collect::<Vec<_>>(),
        })
        .to_string();
        let legacy: GridRow = serde_json::from_str(&cell_form).expect("deserialize cells");
        assert_eq!(
            legacy, row,
            "the cell spelling stopped meaning the same row"
        );

        // And the point of the whole exercise, measured: a plain 120-column
        // row at a third of the old cost or better.
        let plain = GridRow {
            index: 0,
            wrap: None,
            fold: None,
            cells: "x"
                .repeat(120)
                .chars()
                .map(|ch| Cell {
                    ch,
                    ..Cell::default()
                })
                .collect(),
        };
        let packed_len = serde_json::to_string(&plain).expect("serialize").len();
        let cell_len = serde_json::to_string(&serde_json::json!({
            "index": 0,
            "cells": plain.cells.iter().map(|cell| serde_json::json!({"ch": cell.ch})).collect::<Vec<_>>(),
        }))
        .expect("serialize")
        .len();
        assert!(
            packed_len * 3 <= cell_len,
            "packing stopped paying: {packed_len} vs {cell_len}"
        );
    }

    /// The stream a person actually saw as rubble, drawn instead.
    ///
    /// Taken from the reported screen (2026-08-21): an agent redraws its status
    /// line by erasing and stepping back up. As characters that is
    /// `[2K[1A[2K✓ 끝`; as a screen it is one line saying `✓ 끝`, which is what
    /// the run drew and what the page owes.
    #[test]
    fn a_run_that_painted_itself_is_drawn_and_not_spelled_out() {
        let delta = drawn(b"working\r\n\x1b[1A\x1b[2K\xe2\x9c\x93 done\r\n", 4, 20)
            .expect("bytes that steer a cursor are a screen");
        let first = delta
            .rows
            .iter()
            .find(|row| row.index == 0)
            .expect("a full delta carries every row");
        let said: String = first.cells.iter().map(|cell| cell.ch).collect();
        assert_eq!(
            said.trim_end(),
            "✓ done",
            "the erase and the step up have to happen, not be printed"
        );
        assert!(
            !said.contains('['),
            "no steering byte may survive as a character: {said:?}"
        );
    }

    /// And the other half of the rule: text stays text.
    ///
    /// A plain log has no cursor to move, and a screen `rows` tall would keep
    /// only its last `rows` lines — so the answer is `None` and the caller goes
    /// on showing the whole thing as the text it is.
    #[test]
    fn a_log_that_never_steered_is_left_as_the_text_it_is() {
        assert!(drawn(b"one\ntwo\tthree\n", 4, 20).is_none());
        assert!(drawn("한 줄\n또 한 줄\n".as_bytes(), 4, 20).is_none());
        assert!(drawn(b"", 4, 20).is_none());
    }

    /// A caller's geometry is taken, but only as far as a screen can go.
    #[test]
    fn a_screen_asked_for_a_thousand_rows_gets_the_clamp() {
        let delta = drawn(b"\x1b[0m", 100_000, 100_000).expect("an escape is a screen");
        assert_eq!(delta.size, (DRAWN_ROWS_MAX, DRAWN_COLS_MAX));
        let delta = drawn(b"\x1b[0m", 0, 0).expect("an escape is a screen");
        assert_eq!(delta.size, (1, 1));
    }

    /// The text between two points, for the window's selection to copy.
    ///
    /// The window paints only the rows it can see, so a selection dragged
    /// past the top of the pane names lines it does not hold; the grid holds
    /// them all, in the one coordinate space [`TermHit`] speaks. The rules of
    /// a copied line live here with the cells: trailing blanks off, a newline
    /// per row whether or not the program soft-wrapped it, a wide glyph whole
    /// when either end of the selection lands on its second column, and the
    /// body rows of a collapsed fold not there at all — they are not on the
    /// screen the person dragged over.
    #[test]
    fn composed_rows_name_the_buffer_lines_that_selection_copies() {
        let mut term = Terminal::new(4, 20);
        term.feed(b"before\r\n\x1b]7788;begin;7;;\x1b\\head\r\nbody1\r\nbody2\r\n\x1b]7788;end;7\x1b\\after\r\n");
        for scrolled in [false, true] {
            if scrolled {
                term.grid_mut().scroll_view(2);
            }
            let snapshot = term.grid().snapshot();
            let delta = term.grid_mut().take_delta().expect("composed frame");
            assert_eq!(delta.row_lines, snapshot.row_lines);
            assert_eq!(snapshot.rows.len(), snapshot.row_lines.len());
            assert!(!snapshot.row_lines.is_empty());
            for (row, line) in snapshot.rows.iter().zip(&snapshot.row_lines) {
                let text = row.cells.iter().map(|cell| cell.ch).collect::<String>();
                assert_eq!(
                    term.grid().text_between((*line, 0), (*line, 20)),
                    text.trim_end(),
                    "each displayed slot must copy the text it shows"
                );
            }
            assert!(
                snapshot
                    .row_lines
                    .windows(2)
                    .any(|pair| pair[1] > pair[0] + 1)
            );
            let wire = serde_json::to_value(&snapshot).expect("wire delta");
            let decoded: GridDelta = serde_json::from_value(wire).expect("round trip");
            assert_eq!(decoded.row_lines, snapshot.row_lines);
        }
    }

    #[test]
    fn the_text_between_two_points_reads_history_and_screen_as_one_grid() {
        // Four rows, twelve columns: seven lines scroll into history.
        let mut term = Terminal::new(4, 12);
        for line in 0..11 {
            term.feed(format!("line {line:02}\r\n").as_bytes());
        }
        let grid = term.grid();
        assert_eq!(
            grid.scrollback_len(),
            8,
            "eleven newlines on four rows keep eight"
        );
        // Lines 2..=9 straddle the seam between history (0..8) and the screen (8..).
        assert_eq!(
            grid.text_between((2, 5), (9, 4)),
            "02\nline 03\nline 04\nline 05\nline 06\nline 07\nline 08\nline",
            "history and screen read as one grid, with the end column exclusive"
        );
        // An empty span, and one wholly past the buffer, is nothing.
        assert_eq!(grid.text_between((3, 4), (3, 4)), "");
        assert_eq!(grid.text_between((40, 0), (41, 0)), "");
        // Padding is nobody's: a whole row is its text, not its width.
        assert_eq!(grid.text_between((0, 0), (1, 0)), "line 00\n");
        assert_eq!(grid.text_between((0, 0), (0, 12)), "line 00");

        // A wide glyph is one character on two columns. Starting on its
        // second column still copies it; ending on its second column copies
        // it too, because its head is inside the span.
        let term = screen(2, 12, "a가b");
        let grid = term.grid();
        assert_eq!(grid.text_between((0, 2), (0, 4)), "가b");
        assert_eq!(grid.text_between((0, 0), (0, 2)), "a가");
        assert_eq!(grid.text_between((0, 1), (0, 2)), "가");

        // A combining mark rides its base character out.
        let term = screen(2, 12, "e\u{301}x");
        assert_eq!(term.grid().text_between((0, 0), (0, 1)), "e\u{301}");

        // The body of a collapsed fold is not on the screen and not in the copy;
        // the header is.
        let mut term = Terminal::new(6, 12);
        term.feed(b"before\r\n\x1b]7788;begin;7;;\x1b\\head\r\nbody1\r\nbody2\r\n\x1b]7788;end;7\x1b\\after\r\n");
        let grid = term.grid();
        assert_eq!(
            grid.text_between((0, 0), (4, 12)),
            "before\nhead\nafter",
            "a collapsed body is skipped; the header stays"
        );
        assert!(
            term.grid_mut().set_fold_collapsed(7, false),
            "the region is known"
        );
        assert_eq!(
            term.grid().text_between((0, 0), (4, 12)),
            "before\nhead\nbody1\nbody2\nafter",
            "an opened fold copies whole"
        );
    }
}
