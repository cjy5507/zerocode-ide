//! A Flow (docs/design/flow-engine-operator-and-qa.md §2, plan
//! docs/plans/flow-engine-implementation-20260914.md D1·D2·D3): a recipe
//! document with two more sections — `## Flow` (policy, evidence level,
//! fingerprint, the assets seen; and for money, the transaction's
//! parameters and how it is confirmed — docs/design/flow-engine-guarded-
//! money-path.md) and `## Checks` (the oracle lines, each a check command
//! the walk already knows how to run). Everything here is pure: the
//! grammar, the fingerprint's identity match, the amount grammar, and the
//! verdict a run's observations earn. The window reads the words from here
//! and adds nothing of its own.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::agent_browser;
use crate::agent_emulator;
use crate::computer_recipe::{
    NumberedLine, RECIPE_CHECKS, RECIPE_MARK_MONEY, RECIPE_MARK_SEPARATOR, RecipeTool, money_step,
    numbered_code_span, recipe_command_words, recipe_line_text, recipe_lines, recipe_param_name,
    section,
};
use crate::computer_use::{COMPUTER_USE_PROTOCOL_VERSION, parse_command, verb_method};
use crate::computer_use_protocol::AppIdentity;

/// The grammar of a Flow's two sections — the one table its reader
/// (`parse_flow`) and its writer (`FlowSpec::written`) share. A key, a word
/// or a field not in this block is refused by name, never skipped.
pub const FLOW_HEADING: &str = "## Flow";
pub const FLOW_HEADING_CHECKS: &str = "## Checks";
/// The one line that starts a round of `recipe-run --repeat` (design §2
/// `event`): a check line like the oracle's, and an event — something that
/// happens, never a state. Without the section a repeat's rounds walk on
/// the caller's clock (a schedule).
pub const FLOW_HEADING_TRIGGER: &str = "## Trigger";
/// The `- key: value` lines under `## Flow`: the first three may not be
/// absent; `assets` is information; `money` makes a Flow one that moves
/// money (and a `guarded` Flow must have it); `confirm` rides `money`.
pub const FLOW_KEY_POLICY: &str = "policy";
pub const FLOW_KEY_EVIDENCE: &str = "evidence";
pub const FLOW_KEY_FINGERPRINT: &str = "fingerprint";
pub const FLOW_KEY_ASSETS: &str = "assets";
pub const FLOW_KEY_MONEY: &str = "money";
pub const FLOW_KEY_CONFIRM: &str = "confirm";
pub const FLOW_KEYS: [&str; 6] = [
    FLOW_KEY_POLICY,
    FLOW_KEY_EVIDENCE,
    FLOW_KEY_FINGERPRINT,
    FLOW_KEY_ASSETS,
    FLOW_KEY_MONEY,
    FLOW_KEY_CONFIRM,
];
/// A check line's second mark: whether a verdict may be green without it.
pub const FLOW_WORD_REQUIRED: &str = "required";
pub const FLOW_WORD_OPTIONAL: &str = "optional";
/// The fingerprint line's `field=value` words; `protocol` may not be absent.
pub const FLOW_FINGERPRINT_APPS: &str = "apps";
pub const FLOW_FINGERPRINT_HOSTS: &str = "hosts";
pub const FLOW_FINGERPRINT_PROTOCOL: &str = "protocol";
pub const FLOW_FINGERPRINT_FIELDS: [&str; 3] = [
    FLOW_FINGERPRINT_APPS,
    FLOW_FINGERPRINT_HOSTS,
    FLOW_FINGERPRINT_PROTOCOL,
];
/// The money line's `field=value` words — each the name of a recipe
/// parameter the money step types; none may be absent.
pub const FLOW_MONEY_ID: &str = "id";
pub const FLOW_MONEY_AMOUNT: &str = "amount";
pub const FLOW_MONEY_RECIPIENT: &str = "recipient";
pub const FLOW_MONEY_FIELDS: [&str; 3] = [FLOW_MONEY_ID, FLOW_MONEY_AMOUNT, FLOW_MONEY_RECIPIENT];
/// The confirm line's first word, and the one field `auto` takes.
pub const FLOW_CONFIRM_PERSON: &str = "person";
pub const FLOW_CONFIRM_AUTO: &str = "auto";
pub const FLOW_CONFIRM_CAP: &str = "cap";
/// The amount grammar's one mark: the decimal point.
const FLOW_AMOUNT_POINT: char = '.';
/// `- ` opens a Flow line, `:` parts its key from its value, `,` parts the
/// items of a list and a check's two marks, `=` a fingerprint field from its
/// value.
const FLOW_BULLET: &str = "- ";
const FLOW_KEY_VALUE: char = ':';
const FLOW_LIST_SEPARATOR: char = ',';
const FLOW_FIELD_VALUE: char = '=';
/// What makes a network row an asset worth recording: a script whose query
/// carries a version (`settlement.js?v=13`).
const FLOW_ASSET_SCRIPT_SUFFIX: &str = ".js";
const FLOW_ASSET_VERSION_KEY: &str = "v";
/// Where an answer carries the app it acted in (`AppIdentity`, the app half
/// of every computer answer — bare, under `result`, or in a snapshot), and
/// where a browser answer carries its page's URL — JSON pointers tried in
/// order until one answers. A `tabs` answer is a row per pane, each with its
/// own URL; a `network` answer's rows are requests, whose hosts are not the
/// page's and are read for assets only.
const FLOW_APP_POINTERS: [&str; 4] = [
    "/app",
    "/result/app",
    "/result/snapshot/app",
    "/snapshot/app",
];
const FLOW_URL_POINTERS: [&str; 4] = [
    "/url",
    "/document/url",
    "/result/url",
    "/result/document/url",
];
const FLOW_NETWORK_ROWS_POINTER: &str = "/entries";
const FLOW_NETWORK_ROW_URL: &str = "url";
/// Where an emulator answer names its foreground app: its package (an Android
/// package name, or an iOS bundle id), a bare string beside the desktop's
/// `AppIdentity` — bare or under `result`. Read into the same `apps` set, so
/// an emulator act is bound to `Target::App` the same way a desktop act is.
const FLOW_APP_PACKAGE_POINTERS: [&str; 2] = ["/app/package", "/result/app/package"];

/// The word of a grammar table's row, if the word is one of its.
fn word_of<T: Copy>(
    all: impl IntoIterator<Item = T>,
    as_str: fn(T) -> &'static str,
    word: &str,
) -> Option<T> {
    all.into_iter().find(|row| as_str(*row) == word)
}

/// A grammar table's words, for a refusal that names what was allowed.
fn words_of<T: Copy>(all: impl IntoIterator<Item = T>, as_str: fn(T) -> &'static str) -> String {
    all.into_iter()
        .map(|row| format!("`{}`", as_str(row)))
        .collect::<Vec<_>>()
        .join(" | ")
}

/// A line's `field=value` words held by field — each field one of `fields`
/// and given once, refused by name otherwise. The one reader the
/// fingerprint line, the money line and an auto confirmation share.
fn fielded<'a>(
    value: &'a str,
    of: &str,
    fields: &[&'static str],
) -> Result<BTreeMap<&'a str, &'a str>, String> {
    let mut held = BTreeMap::new();
    for word in value.split_whitespace() {
        let (field, given) = word.split_once(FLOW_FIELD_VALUE).ok_or_else(|| {
            format!("`{word}` is not a `field{FLOW_FIELD_VALUE}value` word of the {of}")
        })?;
        if !fields.contains(&field) {
            return Err(format!(
                "`{field}` is not a {of} field ({})",
                words_of(fields.iter().copied(), |field| field)
            ));
        }
        if held.insert(field, given).is_some() {
            return Err(format!("{of} `{field}` twice"));
        }
    }
    Ok(held)
}

/// The side-effect gate — the one axis that parts QA from money.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Policy {
    /// Staging: every effect can be undone; production is the one refusal.
    /// A money step is rehearsed up to and never pressed.
    Dry,
    /// Money: the money step is pressed past its gate alone
    /// (docs/design/flow-engine-guarded-money-path.md — the transaction
    /// given, confirmed, unseen by the ledger, and shown by the page).
    Guarded,
}

impl Policy {
    pub const ALL: [Self; 2] = [Self::Dry, Self::Guarded];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Dry => "dry",
            Self::Guarded => "guarded",
        }
    }

    #[must_use]
    pub fn from_word(word: &str) -> Option<Self> {
        word_of(Self::ALL, Self::as_str, word)
    }
}

/// An amount as a document or a caller writes it: digits, at most one
/// decimal point with digits on both sides — no sign, no currency, no
/// thousands mark, no exponent (the review's P0 3: a financial value is
/// never guessed). Two amounts compare exactly, digit by digit, in the
/// unit the document writes; no float ever sees them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Amount {
    /// The whole digits without leading zeros (`0` when none).
    whole: String,
    /// The fraction digits without trailing zeros (empty when none).
    fraction: String,
}

impl Amount {
    pub fn read(text: &str) -> Result<Self, String> {
        let (whole, fraction) = text.split_once(FLOW_AMOUNT_POINT).unwrap_or((text, ""));
        let digits =
            |part: &str| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit());
        if !digits(whole) || (text.contains(FLOW_AMOUNT_POINT) && !digits(fraction)) {
            return Err(format!(
                "`{text}` is not an amount: digits, at most one `{FLOW_AMOUNT_POINT}` with digits on both sides — no sign, currency or thousands mark"
            ));
        }
        let whole = whole.trim_start_matches('0');
        Ok(Self {
            whole: if whole.is_empty() { "0" } else { whole }.to_string(),
            fraction: fraction.trim_end_matches('0').to_string(),
        })
    }
}

impl Ord for Amount {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (self.whole.len(), &self.whole, &self.fraction).cmp(&(
            other.whole.len(),
            &other.whole,
            &other.fraction,
        ))
    }
}

impl PartialOrd for Amount {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// The money line (design §1): the recipe's three parameters the money
/// step types — the transaction's id, its amount, its recipient — and the
/// one step that carries the money mark. The values are the caller's
/// (`--params`), never a screen's; the line names, it never holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Money {
    pub id: String,
    pub amount: String,
    pub recipient: String,
    /// The step marked `— money`, counted from 1 (`computer_recipe::money_step`).
    pub step: usize,
}

impl Money {
    /// The three parameter names, in the line's order.
    #[must_use]
    pub fn names(&self) -> [&str; 3] {
        [&self.id, &self.amount, &self.recipient]
    }

    fn written(&self) -> String {
        FLOW_MONEY_FIELDS
            .iter()
            .zip(self.names())
            .map(|(field, name)| format!("{field}{FLOW_FIELD_VALUE}{name}"))
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// The money line's words: the three fields, each a parameter name.
    /// The step is bound afterwards, from the document's marks.
    fn read(value: &str) -> Result<Self, String> {
        let held = fielded(value, FLOW_KEY_MONEY, &FLOW_MONEY_FIELDS)?;
        let name = |field: &str| -> Result<String, String> {
            let given = held
                .get(field)
                .ok_or_else(|| format!("the `{FLOW_KEY_MONEY}` line has no `{field}`"))?;
            if !recipe_param_name(given) {
                return Err(format!(
                    "`{field}{FLOW_FIELD_VALUE}{given}` is not a parameter name: the transaction is given with --params, never written in the document"
                ));
            }
            Ok((*given).to_string())
        };
        Ok(Self {
            id: name(FLOW_MONEY_ID)?,
            amount: name(FLOW_MONEY_AMOUNT)?,
            recipient: name(FLOW_MONEY_RECIPIENT)?,
            step: 0,
        })
    }
}

/// How a guarded money step is confirmed (design §4): by the person, bound
/// to the transaction — or without anyone under a cap the document writes
/// in the amount's own unit, by the person above it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "by", rename_all = "lowercase")]
pub enum Confirm {
    #[default]
    Person,
    Auto {
        cap: String,
    },
}

impl Confirm {
    /// Whether an amount moves without the person: under an auto cap.
    #[must_use]
    pub fn covers(&self, amount: &Amount) -> bool {
        match self {
            Self::Person => false,
            Self::Auto { cap } => Amount::read(cap).is_ok_and(|cap| *amount <= cap),
        }
    }

    fn written(&self) -> String {
        match self {
            Self::Person => FLOW_CONFIRM_PERSON.to_string(),
            Self::Auto { cap } => {
                format!("{FLOW_CONFIRM_AUTO} {FLOW_CONFIRM_CAP}{FLOW_FIELD_VALUE}{cap}")
            }
        }
    }

    /// The confirm line's words: `person` alone, or `auto cap=<amount>`.
    fn read(value: &str) -> Result<Self, String> {
        let grammar = || {
            format!(
                "`{FLOW_CONFIRM_PERSON}` | `{FLOW_CONFIRM_AUTO} {FLOW_CONFIRM_CAP}{FLOW_FIELD_VALUE}<amount>`"
            )
        };
        let (word, rest) = value
            .split_once(char::is_whitespace)
            .map_or((value, ""), |(word, rest)| (word, rest.trim()));
        match word {
            FLOW_CONFIRM_PERSON if rest.is_empty() => Ok(Self::Person),
            FLOW_CONFIRM_PERSON => Err(format!(
                "`{FLOW_CONFIRM_PERSON}` takes no words, not `{rest}`: {}",
                grammar()
            )),
            FLOW_CONFIRM_AUTO => {
                let held = fielded(rest, FLOW_CONFIRM_AUTO, &[FLOW_CONFIRM_CAP])?;
                let cap = held
                    .get(FLOW_CONFIRM_CAP)
                    .ok_or_else(|| format!("`{FLOW_CONFIRM_AUTO}` needs its cap: {}", grammar()))?;
                Amount::read(cap).map_err(|why| format!("{FLOW_CONFIRM_CAP}: {why}"))?;
                Ok(Self::Auto {
                    cap: (*cap).to_string(),
                })
            }
            other => Err(format!("`{other}` is not a confirmation: {}", grammar())),
        }
    }
}

/// How much a run leaves behind. Whatever the level, the verdict
/// (`flow-verdict.json`) and the walk's own record are always written: the
/// level decides the frames and the report, never whether the run is judged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum EvidenceLevel {
    /// Every step's frames before and after, its reading, its check — and
    /// the HTML report. The default: what a recipe without a Flow keeps,
    /// and the level a folder whose walk said none is read at (design
    /// §3.5 — QA and regression keep everything).
    #[default]
    Full,
    /// The verdict and the failed steps, no frames.
    VerdictOnly,
    /// No evidence folder beyond the verdict and the walk record.
    Off,
}

impl EvidenceLevel {
    pub const ALL: [Self; 3] = [Self::Full, Self::VerdictOnly, Self::Off];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::VerdictOnly => "verdict-only",
            Self::Off => "off",
        }
    }

    #[must_use]
    pub fn from_word(word: &str) -> Option<Self> {
        word_of(Self::ALL, Self::as_str, word)
    }

    /// Whether a step's frames are kept: only `full` keeps them.
    #[must_use]
    pub const fn frames(self) -> bool {
        matches!(self, Self::Full)
    }
}

/// What a check line asserts about the run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckKind {
    /// Something this run made happen: absent at the baseline, present after
    /// (or one more than before). Present already at the baseline is stale.
    Event,
    /// Something true at the end, whatever the baseline held.
    State,
}

impl CheckKind {
    pub const ALL: [Self; 2] = [Self::Event, Self::State];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Event => "event",
            Self::State => "state",
        }
    }

    #[must_use]
    pub fn from_word(word: &str) -> Option<Self> {
        word_of(Self::ALL, Self::as_str, word)
    }
}

/// One oracle line: a check command the walk already knows how to run
/// (`RECIPE_CHECKS` through the desktop helper, the browser table's check
/// verbs through the browser door), what kind of assertion it is, and
/// whether the verdict needs it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Check {
    /// Counted from 1 in the order written — the number in front of the line.
    pub id: usize,
    pub kind: CheckKind,
    pub required: bool,
    pub tool: RecipeTool,
    pub argv: Vec<String>,
}

/// The identity a recipe is bound to: the apps it acted in (bundle ids, or
/// names for a process without a bundle), the hosts its pages came from, and
/// the protocol its answers were read with. `assets` is information for a
/// person reading the document — a site's versioned scripts — and never part
/// of the match, since a version bump is not another site.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fingerprint {
    pub apps: BTreeSet<String>,
    pub hosts: BTreeSet<String>,
    pub protocol: u64,
    pub assets: BTreeSet<String>,
}

/// What a live fingerprint holds that the recorded one does not.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stale {
    pub missing_apps: BTreeSet<String>,
    pub missing_hosts: BTreeSet<String>,
    /// `(recorded, live)` when the two differ.
    pub protocol: Option<(u64, u64)>,
}

impl Stale {
    /// The refusal a person reads (`flow_stale`): what the run saw that the
    /// recording never did.
    #[must_use]
    pub fn said(&self) -> String {
        let mut parts = Vec::new();
        if !self.missing_apps.is_empty() {
            parts.push(format!("기록에 없는 앱 {}", joined(&self.missing_apps)));
        }
        if !self.missing_hosts.is_empty() {
            parts.push(format!(
                "기록에 없는 호스트 {}",
                joined(&self.missing_hosts)
            ));
        }
        if let Some((recorded, live)) = self.protocol {
            parts.push(format!("프로토콜 {recorded}이(가) 지금은 {live}"));
        }
        format!("지문이 낡았다: {}", parts.join("; "))
    }
}

/// Where an act aims — the question `Fingerprint::allows` answers before a
/// press (plan D8, `flow_env_mismatch`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// A bundle id, or the name of an app without one.
    App(String),
    Host(String),
}

/// A set's items as one list word.
fn joined(set: &BTreeSet<String>) -> String {
    set.iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(&FLOW_LIST_SEPARATOR.to_string())
}

/// A list value's items — trimmed, none empty.
fn listed(value: &str) -> Result<BTreeSet<String>, String> {
    let items: BTreeSet<String> = value
        .split(FLOW_LIST_SEPARATOR)
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_string)
        .collect();
    if items.is_empty() {
        return Err(format!("`{value}` names nothing"));
    }
    Ok(items)
}

/// The host a page URL names, if it names one (`about:blank` does not) —
/// the word a fingerprint holds a page by, and a walk asks `allows` about.
#[must_use]
pub fn page_host(url: &str) -> Option<String> {
    url::Url::parse(url).ok()?.host_str().map(str::to_string)
}

/// A request URL as the asset it fetched, when it is a versioned script:
/// the file name with its query, so a person sees the version.
fn versioned_script(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).ok()?;
    let file = parsed.path_segments()?.next_back()?;
    if !file.ends_with(FLOW_ASSET_SCRIPT_SUFFIX)
        || !parsed
            .query_pairs()
            .any(|(key, _)| key == FLOW_ASSET_VERSION_KEY)
    {
        return None;
    }
    Some(format!("{file}?{}", parsed.query()?))
}

impl Fingerprint {
    /// The fingerprint a run's answers add up to: every app an answer names
    /// (its bundle id, or its name without one), the host of every page URL
    /// an answer carries, this build's protocol — and, as information, the
    /// versioned scripts the network rows fetched.
    #[must_use]
    pub fn observe(envelopes: &[Value]) -> Self {
        let mut live = Self {
            protocol: COMPUTER_USE_PROTOCOL_VERSION,
            ..Self::default()
        };
        for envelope in envelopes {
            if let Some(identity) = FLOW_APP_POINTERS
                .iter()
                .find_map(|pointer| envelope.pointer(pointer))
                .and_then(|app| AppIdentity::deserialize(app).ok())
            {
                live.apps
                    .insert(identity.bundle_id.unwrap_or(identity.name));
            }
            // The emulator names its app by a bare package, not an AppIdentity.
            if let Some(package) = FLOW_APP_PACKAGE_POINTERS
                .iter()
                .find_map(|pointer| envelope.pointer(pointer))
                .and_then(Value::as_str)
            {
                live.apps.insert(package.to_string());
            }
            let pages = envelope
                .as_array()
                .into_iter()
                .flatten()
                .chain(std::iter::once(envelope));
            for page in pages {
                if let Some(host) = FLOW_URL_POINTERS
                    .iter()
                    .find_map(|pointer| page.pointer(pointer))
                    .and_then(Value::as_str)
                    .and_then(page_host)
                {
                    live.hosts.insert(host);
                }
            }
            for row in envelope
                .pointer(FLOW_NETWORK_ROWS_POINTER)
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                if let Some(asset) = row
                    .get(FLOW_NETWORK_ROW_URL)
                    .and_then(Value::as_str)
                    .and_then(versioned_script)
                {
                    live.assets.insert(asset);
                }
            }
        }
        live
    }

    /// Whether a live fingerprint is this recording's: every app and host
    /// the run saw is one the recording saw, on the same protocol. Assets
    /// are ignored. Otherwise what is missing, for the refusal.
    pub fn matches(&self, live: &Self) -> Result<(), Stale> {
        let stale = Stale {
            missing_apps: live.apps.difference(&self.apps).cloned().collect(),
            missing_hosts: live.hosts.difference(&self.hosts).cloned().collect(),
            protocol: (self.protocol != live.protocol).then_some((self.protocol, live.protocol)),
        };
        if stale == Stale::default() {
            Ok(())
        } else {
            Err(stale)
        }
    }

    /// Whether an act may aim at `target`: only an app or a host of the
    /// recording.
    #[must_use]
    pub fn allows(&self, target: &Target) -> bool {
        match target {
            Target::App(app) => self.apps.contains(app),
            Target::Host(host) => self.hosts.contains(host),
        }
    }

    /// The fingerprint line's words, as `read` reads them back.
    fn written(&self) -> String {
        let mut words = Vec::new();
        for (field, set) in [
            (FLOW_FINGERPRINT_APPS, &self.apps),
            (FLOW_FINGERPRINT_HOSTS, &self.hosts),
        ] {
            if !set.is_empty() {
                words.push(format!("{field}{FLOW_FIELD_VALUE}{}", joined(set)));
            }
        }
        words.push(format!(
            "{FLOW_FINGERPRINT_PROTOCOL}{FLOW_FIELD_VALUE}{}",
            self.protocol
        ));
        words.join(" ")
    }

    /// The fingerprint line's words: `field=value` each, `apps` and `hosts`
    /// lists, `protocol` a number that may not be absent; a field twice or
    /// a field not in the table is refused. Assets ride their own line.
    fn read(value: &str) -> Result<Self, String> {
        let held = fielded(value, FLOW_KEY_FINGERPRINT, &FLOW_FINGERPRINT_FIELDS)?;
        let list = |field: &str| held.get(field).map(|given| listed(given)).transpose();
        let protocol = held
            .get(FLOW_FINGERPRINT_PROTOCOL)
            .ok_or_else(|| format!("the fingerprint has no `{FLOW_FINGERPRINT_PROTOCOL}`"))?;
        Ok(Self {
            apps: list(FLOW_FINGERPRINT_APPS)?.unwrap_or_default(),
            hosts: list(FLOW_FINGERPRINT_HOSTS)?.unwrap_or_default(),
            protocol: protocol.parse::<u64>().map_err(|_| {
                format!("fingerprint `{FLOW_FINGERPRINT_PROTOCOL}` is not a number: `{protocol}`")
            })?,
            assets: BTreeSet::new(),
        })
    }
}

/// A Flow, read from its recipe document: the `## Flow` lines, the
/// `## Checks` lines and, when a repeat has one, the `## Trigger` line. The
/// steps stay the recipe's own (`recipe_lines`); a money line binds to the
/// one step among them that carries the mark.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlowSpec {
    pub policy: Policy,
    pub evidence: EvidenceLevel,
    pub fingerprint: Fingerprint,
    pub checks: Vec<Check>,
    /// The money line, when the Flow moves money (design §1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub money: Option<Money>,
    /// How the money step is confirmed: the person's unless written (§4).
    #[serde(default)]
    pub confirm: Confirm,
    /// What starts a round: an event line asked past where it stood after
    /// the last round. Absent from a record written before triggers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger: Option<Check>,
}

impl FlowSpec {
    /// The lines that watch the money step: every required event line — what
    /// this run made happen, and what the verdict cannot do without. A money
    /// line needs at least one (§3: a money step nobody watches does not
    /// move).
    pub fn money_lines(&self) -> impl Iterator<Item = &Check> {
        self.checks
            .iter()
            .filter(|check| check.kind == CheckKind::Event && check.required)
    }

    /// Whether the money moved as far as this screen can say: every line
    /// watching it passed (§3 — `acted`, not settled: the bank's word is a
    /// gap the design names).
    #[must_use]
    pub fn money_acted(&self, verdict: &FlowVerdict) -> bool {
        let mut lines = self.money_lines().peekable();
        lines.peek().is_some()
            && lines.all(|check| {
                verdict
                    .lines
                    .iter()
                    .any(|line| line.id == check.id && line.status == LineStatus::Pass)
            })
    }

    /// The two sections as a recipe document carries them, after its steps —
    /// what `parse_flow` reads back to this same spec.
    #[must_use]
    pub fn written(&self) -> String {
        let mut out = format!("{FLOW_HEADING}\n\n");
        let mut line = |key: &str, value: &str| {
            out.push_str(&format!("{FLOW_BULLET}{key}{FLOW_KEY_VALUE} {value}\n"));
        };
        line(FLOW_KEY_POLICY, self.policy.as_str());
        line(FLOW_KEY_EVIDENCE, self.evidence.as_str());
        line(FLOW_KEY_FINGERPRINT, &self.fingerprint.written());
        if !self.fingerprint.assets.is_empty() {
            let assets = self
                .fingerprint
                .assets
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join(&format!("{FLOW_LIST_SEPARATOR} "));
            line(FLOW_KEY_ASSETS, &assets);
        }
        if let Some(money) = &self.money {
            line(FLOW_KEY_MONEY, &money.written());
            line(FLOW_KEY_CONFIRM, &self.confirm.written());
        }
        out.push_str(&format!("\n{FLOW_HEADING_CHECKS}\n\n"));
        for check in &self.checks {
            out.push_str(&check_line(check));
        }
        if let Some(trigger) = &self.trigger {
            out.push_str(&format!("\n{FLOW_HEADING_TRIGGER}\n\n"));
            out.push_str(&check_line(trigger));
        }
        out
    }
}

/// One check line as a document writes it: its number, its span, its two
/// marks — the oracle's lines and the trigger's alike.
fn check_line(check: &Check) -> String {
    let marks = format!(
        "{}{FLOW_LIST_SEPARATOR} {}",
        check.kind.as_str(),
        if check.required {
            FLOW_WORD_REQUIRED
        } else {
            FLOW_WORD_OPTIONAL
        }
    );
    format!(
        "{}. {}\n",
        check.id,
        recipe_line_text(check.tool, &check.argv, &[marks])
    )
}

/// A recipe document's Flow, if it has one: both sections present and
/// strict — a key, word, field or mark outside the grammar block, a line
/// that is not `- key: value`, a key twice, a check that is not a check verb
/// (or a browser check with the wrong number of words), a check without its
/// two marks, or a `## Checks` without a required line is refused by name;
/// so is a money line without exactly one marked step or without a
/// required event line, a confirm line without a money line, and a
/// `guarded` policy without a money line. A document with neither section
/// is just a recipe (`Ok(None)`); one section without the other is a
/// mistake, not a recipe.
/// A `## Trigger` is optional, and belongs to a Flow: one event line
/// (`read_trigger`).
pub fn parse_flow(text: &str) -> Result<Option<FlowSpec>, String> {
    let trigger = section(text, FLOW_HEADING_TRIGGER);
    match (
        section(text, FLOW_HEADING),
        section(text, FLOW_HEADING_CHECKS),
    ) {
        (None, None) if trigger.is_some() => Err(format!(
            "`{FLOW_HEADING_TRIGGER}` without its `{FLOW_HEADING}`: a trigger starts a Flow's round"
        )),
        (None, None) => Ok(None),
        (Some(_), None) => Err(format!(
            "`{FLOW_HEADING}` without its `{FLOW_HEADING_CHECKS}`"
        )),
        (None, Some(_)) => Err(format!(
            "`{FLOW_HEADING_CHECKS}` without its `{FLOW_HEADING}`"
        )),
        (Some(flow), Some(checks)) => {
            let mut spec = read_flow_lines(&flow)?;
            spec.checks = read_checks(FLOW_HEADING_CHECKS, &checks)?;
            spec.trigger = trigger.as_deref().map(read_trigger).transpose()?;
            bind_money(&mut spec, text)?;
            Ok(Some(spec))
        }
    }
}

/// The money contracts a document keeps before it is walked (design
/// §1–§4): a `guarded` Flow names its money line; a money line binds to
/// exactly one step marked `— money`, and has a required event line to
/// watch that step's outcome.
fn bind_money(spec: &mut FlowSpec, text: &str) -> Result<(), String> {
    let Some(money) = spec.money.as_mut() else {
        if spec.policy == Policy::Guarded {
            return Err(format!(
                "a `{}` Flow without a `{FLOW_KEY_MONEY}` line has nothing its contracts guard",
                Policy::Guarded.as_str()
            ));
        }
        return Ok(());
    };
    money.step = money_step(&recipe_lines(text)?)?.ok_or_else(|| {
        format!(
            "the `{FLOW_KEY_MONEY}` line names a transaction but no step is marked `{RECIPE_MARK_MONEY}`"
        )
    })?;
    if spec.money_lines().next().is_none() {
        return Err(format!(
            "`{FLOW_HEADING_CHECKS}` has no `{}{FLOW_LIST_SEPARATOR} {FLOW_WORD_REQUIRED}` line: a money step whose outcome nobody watches does not move",
            CheckKind::Event.as_str()
        ));
    }
    Ok(())
}

/// The `## Trigger` lines as the one check that starts a round: the check
/// grammar (`read_checks`, so a trigger is any check the walk can ask), then
/// exactly one line, and an event — a state does not happen, so it starts
/// nothing; two lines would be two triggers; an optional trigger is refused
/// by the grammar (no required line).
fn read_trigger(lines: &[&str]) -> Result<Check, String> {
    let mut checks = read_checks(FLOW_HEADING_TRIGGER, lines)?;
    if checks.len() != 1 {
        return Err(format!(
            "`{FLOW_HEADING_TRIGGER}` has {} lines: a trigger is one event line",
            checks.len()
        ));
    }
    let trigger = checks.remove(0);
    if trigger.kind != CheckKind::Event {
        return Err(format!(
            "`{FLOW_HEADING_TRIGGER}`: a `{}` line is not a trigger — 방아쇠는 일어나는 일이다: mark it `{}`",
            trigger.kind.as_str(),
            CheckKind::Event.as_str()
        ));
    }
    Ok(trigger)
}

/// The `## Flow` lines as a spec without its checks (assets folded into the
/// fingerprint; the money step bound afterwards).
fn read_flow_lines(lines: &[&str]) -> Result<FlowSpec, String> {
    let mut held: BTreeMap<&str, &str> = BTreeMap::new();
    for line in lines.iter().filter(|line| !line.is_empty()) {
        let (key, value) = line
            .strip_prefix(FLOW_BULLET)
            .and_then(|rest| rest.split_once(FLOW_KEY_VALUE))
            .map(|(key, value)| (key.trim(), value.trim()))
            .filter(|(key, value)| !key.is_empty() && !value.is_empty())
            .ok_or_else(|| {
                format!("`{FLOW_HEADING}`: `{line}` is not a `{FLOW_BULLET}key{FLOW_KEY_VALUE} value` line")
            })?;
        if !FLOW_KEYS.contains(&key) {
            return Err(format!(
                "`{FLOW_HEADING}`: `{key}` is not a Flow key ({})",
                words_of(FLOW_KEYS, |key| key)
            ));
        }
        if held.insert(key, value).is_some() {
            return Err(format!("`{FLOW_HEADING}`: `{key}` twice"));
        }
    }
    let required = |key: &str| {
        held.get(key)
            .copied()
            .ok_or_else(|| format!("`{FLOW_HEADING}` has no `{key}` line"))
    };
    let word = required(FLOW_KEY_POLICY)?;
    let policy = Policy::from_word(word).ok_or_else(|| {
        format!(
            "`{FLOW_KEY_POLICY}: {word}` — the policy is {}",
            words_of(Policy::ALL, Policy::as_str)
        )
    })?;
    let word = required(FLOW_KEY_EVIDENCE)?;
    let evidence = EvidenceLevel::from_word(word).ok_or_else(|| {
        format!(
            "`{FLOW_KEY_EVIDENCE}: {word}` — the level is {}",
            words_of(EvidenceLevel::ALL, EvidenceLevel::as_str)
        )
    })?;
    let mut fingerprint = Fingerprint::read(required(FLOW_KEY_FINGERPRINT)?)
        .map_err(|why| format!("`{FLOW_KEY_FINGERPRINT}`: {why}"))?;
    if let Some(assets) = held.get(FLOW_KEY_ASSETS) {
        fingerprint.assets = listed(assets).map_err(|why| format!("`{FLOW_KEY_ASSETS}`: {why}"))?;
    }
    let money = held
        .get(FLOW_KEY_MONEY)
        .map(|value| Money::read(value).map_err(|why| format!("`{FLOW_KEY_MONEY}`: {why}")))
        .transpose()?;
    let confirm = match held.get(FLOW_KEY_CONFIRM) {
        None => Confirm::default(),
        Some(_) if money.is_none() => {
            return Err(format!(
                "`{FLOW_KEY_CONFIRM}` without a `{FLOW_KEY_MONEY}` line: nothing to confirm"
            ));
        }
        Some(value) => {
            Confirm::read(value).map_err(|why| format!("`{FLOW_KEY_CONFIRM}`: {why}"))?
        }
    };
    Ok(FlowSpec {
        policy,
        evidence,
        fingerprint,
        checks: Vec::new(),
        money,
        confirm,
        trigger: None,
    })
}

/// The lines of a check section (`## Checks`, `## Trigger` — `heading`
/// names it in a refusal) as checks: the step grammar's numbered code span,
/// then ` — <kind>, <required|optional>`. Prose is ignored; a numbered line
/// that is not a check is refused by number.
fn read_checks(heading: &str, lines: &[&str]) -> Result<Vec<Check>, String> {
    let mut checks = Vec::new();
    for trimmed in lines {
        let Some(NumberedLine { shown, span }) = numbered_code_span(trimmed) else {
            continue;
        };
        let id = checks.len() + 1;
        let at = |why: &str| format!("check {id} ({shown}.): {why}");
        let (command, marks) = span.map_err(at)?;
        let (tool, argv) = recipe_command_words(command).map_err(|why| at(&why))?;
        let (kind, required) = read_check_marks(marks).map_err(|why| at(&why))?;
        check_command(tool, &argv).map_err(|why| at(&why))?;
        checks.push(Check {
            id,
            kind,
            required,
            tool,
            argv,
        });
    }
    if checks.is_empty() {
        return Err(format!("`{heading}` has no check line"));
    }
    if !checks.iter().any(|check| check.required) {
        return Err(format!(
            "`{heading}` has no `{FLOW_WORD_REQUIRED}` line: a verdict nobody can fail is not a verdict, and an optional trigger starts nothing"
        ));
    }
    Ok(checks)
}

/// A check line's marks after its span: exactly the kind and the need.
fn read_check_marks(marks: &str) -> Result<(CheckKind, bool), String> {
    let grammar = || {
        format!(
            "`{RECIPE_MARK_SEPARATOR}<{}>{FLOW_LIST_SEPARATOR} <`{FLOW_WORD_REQUIRED}` | `{FLOW_WORD_OPTIONAL}`>`",
            words_of(CheckKind::ALL, CheckKind::as_str)
        )
    };
    let words = marks
        .strip_prefix(RECIPE_MARK_SEPARATOR)
        .ok_or_else(|| format!("its marks are missing: {}", grammar()))?;
    let mut parts = words.split(FLOW_LIST_SEPARATOR).map(str::trim);
    let (Some(kind), Some(need), None) = (parts.next(), parts.next(), parts.next()) else {
        return Err(format!("its marks are `{words}`, not {}", grammar()));
    };
    let kind = CheckKind::from_word(kind)
        .ok_or_else(|| format!("`{kind}` is not a check kind: {}", grammar()))?;
    let required = match need {
        FLOW_WORD_REQUIRED => true,
        FLOW_WORD_OPTIONAL => false,
        other => return Err(format!("`{other}` is not a need: {}", grammar())),
    };
    Ok((kind, required))
}

/// Whether a check line's command is one the walk can judge: a desktop
/// check verb (`RECIPE_CHECKS`) that parses, or a browser check verb with
/// the words its table counts.
fn check_command(tool: RecipeTool, argv: &[String]) -> Result<(), String> {
    let verb = argv.first().map(String::as_str).unwrap_or_default();
    match tool {
        RecipeTool::Computer => {
            if !verb_method(verb).is_some_and(|method| RECIPE_CHECKS.contains(&method)) {
                return Err(format!(
                    "`{verb}` is not a check: {}",
                    words_of(RECIPE_CHECKS.iter().copied(), |method| method.verb_name())
                ));
            }
            parse_command(argv).map(drop)
        }
        RecipeTool::Browser => {
            agent_browser::arity_ok(argv)?;
            if !agent_browser::is_check(verb) {
                return Err(format!(
                    "`{verb}` is not a check: {}",
                    words_of(
                        agent_browser::BROWSER_VERBS
                            .iter()
                            .filter(|row| row.check)
                            .map(|row| row.word),
                        |word| word
                    )
                ));
            }
            Ok(())
        }
        RecipeTool::Emulator => {
            agent_emulator::arity_ok(argv)?;
            if !agent_emulator::is_check(verb) {
                return Err(format!(
                    "`{verb}` is not a check: {}",
                    words_of(
                        agent_emulator::EMULATOR_VERBS
                            .iter()
                            .filter(|row| row.check)
                            .map(|row| row.word),
                        |word| word
                    )
                ));
            }
            crate::computer_use::parse_emulator_command(argv).map(drop)
        }
    }
}

/// What one look at a check found: whether its subject was there, and how
/// many when the verb counts (`find`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Presence {
    pub present: bool,
    pub count: Option<usize>,
}

/// What the run's final look at a check line came to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Observed {
    Seen(Presence),
    /// The check could not be run or read — a dead pane, a refused verb —
    /// and why. Never a pass.
    NotEvaluable(String),
}

/// A judged line's standing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LineStatus {
    Pass,
    Fail,
    /// An event that already held at the baseline: not this run's doing.
    Stale,
    NotEvaluable,
}

/// One check line, judged, with the sentence a person reads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JudgedLine {
    pub id: usize,
    pub status: LineStatus,
    pub said: String,
}

/// The run's verdict — written to `flow-verdict.json` as it is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlowVerdict {
    /// Every required line passed. A Flow without a required line never
    /// passes: a verdict nobody can fail is not a verdict.
    pub pass: bool,
    pub lines: Vec<JudgedLine>,
}

/// What a walk records of its Flow beside its steps (plan D4, `walk-NNN.json`
/// `flow`): the spec it ran under, the baseline probe, the final
/// observations and the verdict they earned — enough for `evidence
/// --verify` to judge again from the folder alone and compare.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlowRecord {
    #[serde(flatten)]
    pub spec: FlowSpec,
    pub baseline: BTreeMap<usize, Presence>,
    pub observed: BTreeMap<usize, Observed>,
    pub verdict: FlowVerdict,
}

impl FlowRecord {
    /// The verdict judged again from the record's own baseline and
    /// observations — what a stored verdict must equal to be believed.
    #[must_use]
    pub fn rejudged(&self) -> FlowVerdict {
        judge(&self.spec.checks, &self.baseline, &self.observed)
    }
}

/// Judge every check line from the baseline probe (each event line asked
/// once before the first act) and the final observations. An event passes
/// when it was absent at the baseline and present after, or counted one
/// more; present already is stale. A state passes when present after. A
/// line nobody observed, an event nobody probed, or an observation that
/// could not be made is not evaluable. Optional lines never block the pass.
#[must_use]
pub fn judge(
    checks: &[Check],
    baseline: &BTreeMap<usize, Presence>,
    observed: &BTreeMap<usize, Observed>,
) -> FlowVerdict {
    let lines: Vec<JudgedLine> = checks
        .iter()
        .map(|check| judge_line(check, baseline.get(&check.id), observed.get(&check.id)))
        .collect();
    let mut required = checks
        .iter()
        .zip(&lines)
        .filter(|(check, _)| check.required)
        .peekable();
    let pass =
        required.peek().is_some() && required.all(|(_, line)| line.status == LineStatus::Pass);
    FlowVerdict { pass, lines }
}

fn judge_line(
    check: &Check,
    baseline: Option<&Presence>,
    observed: Option<&Observed>,
) -> JudgedLine {
    let (status, said) = match (observed, check.kind, baseline) {
        (None, _, _) => (
            LineStatus::NotEvaluable,
            "판정 불가: 관측이 없다".to_string(),
        ),
        (Some(Observed::NotEvaluable(why)), _, _) => {
            (LineStatus::NotEvaluable, format!("판정 불가: {why}"))
        }
        (Some(Observed::Seen(seen)), CheckKind::State, _) => {
            if seen.present {
                (LineStatus::Pass, "관측에 있다".to_string())
            } else {
                (LineStatus::Fail, "관측에 없다".to_string())
            }
        }
        (Some(Observed::Seen(_)), CheckKind::Event, None) => (
            LineStatus::NotEvaluable,
            "판정 불가: 기준점을 재지 않아 이번 실행의 사건인지 알 수 없다".to_string(),
        ),
        (Some(Observed::Seen(seen)), CheckKind::Event, Some(_)) if !seen.present => {
            (LineStatus::Fail, "관측에 없다".to_string())
        }
        (Some(Observed::Seen(_)), CheckKind::Event, Some(before)) if !before.present => {
            (LineStatus::Pass, "기준점에 없다가 관측에 있다".to_string())
        }
        (Some(Observed::Seen(seen)), CheckKind::Event, Some(before)) => {
            match (before.count, seen.count) {
                (Some(was), Some(now)) if now > was => (
                    LineStatus::Pass,
                    format!("기준점 {was}건에서 {now}건으로 늘었다"),
                ),
                (Some(was), Some(now)) => (
                    LineStatus::Stale,
                    format!(
                        "기준점에서 이미 있었다({was}건, 지금도 {now}건): 이번 실행의 사건이 아니다"
                    ),
                ),
                _ => (
                    LineStatus::Stale,
                    "기준점에서 이미 있었다: 이번 실행의 사건이 아니다".to_string(),
                ),
            }
        }
    };
    JudgedLine {
        id: check.id,
        status,
        said,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::computer_recipe::{
        RECIPE_CHECK_MS, RECIPE_HEADING_STEPS, RECIPE_MARK_MONEY, RECIPE_MARK_SEPARATOR,
        RecipeTool, recipe_lines,
    };
    use crate::computer_use::{
        COMPUTER_USE_DEADLINE_SECONDS, COMPUTER_USE_PROTOCOL_VERSION, COMPUTER_WAIT_FOR_MAX_MS,
        COMPUTER_WAIT_FOR_MIN_MS, COMPUTER_WAIT_FOR_POLL_MS, FLOW_BASELINE_PROBE_MS,
        FLOW_VERIFY_MS, desktop_wait_for_ms, parse_command, usage, walk_budget_ms,
    };
    use crate::computer_use_protocol::error_code;
    use serde_json::json;
    use std::collections::{BTreeMap, BTreeSet};

    fn words(list: &[&str]) -> Vec<String> {
        list.iter().map(|word| (*word).to_string()).collect()
    }

    fn set(list: &[&str]) -> BTreeSet<String> {
        list.iter().map(|word| (*word).to_string()).collect()
    }

    /// The document of the brief: a recipe with its Flow and Checks sections.
    fn document(policy: &str) -> String {
        format!(
            "# 송금 QA\n\n{RECIPE_HEADING_STEPS}\n\n1. `zerocode-computer launch --app Safari`\n\n\
             {FLOW_HEADING}\n\n\
             - {FLOW_KEY_POLICY}: {policy}\n\
             - {FLOW_KEY_EVIDENCE}: full\n\
             - {FLOW_KEY_FINGERPRINT}: apps=com.apple.Safari,com.example.admin hosts=stg-admin.example.internal protocol={COMPUTER_USE_PROTOCOL_VERSION}\n\
             - {FLOW_KEY_ASSETS}: settlement.js?v=13, transfer.js?v=acctcol20260620\n\n\
             {FLOW_HEADING_CHECKS}\n\n\
             1. `zerocode-computer wait-for --text \"송금 완료\" --app Safari` — event, required\n\
             2. `zerocode-browser find browser-1 \"RCPT-\"` — event, required\n\
             3. `zerocode-computer wait-for --text \"로그인\" --app Safari` — state, optional\n"
        )
    }

    fn check(id: usize, kind: CheckKind, required: bool) -> Check {
        Check {
            id,
            kind,
            required,
            tool: RecipeTool::Computer,
            argv: words(&["wait-for", "--app", "X", "--text", "Done"]),
        }
    }

    fn present(count: Option<usize>) -> Presence {
        Presence {
            present: true,
            count,
        }
    }

    fn absent() -> Presence {
        Presence {
            present: false,
            count: None,
        }
    }

    fn status_of(verdict: &FlowVerdict, id: usize) -> LineStatus {
        verdict
            .lines
            .iter()
            .find(|line| line.id == id)
            .unwrap_or_else(|| panic!("line {id} judged: {verdict:?}"))
            .status
    }

    #[test]
    fn emulator_checks_parse_in_a_flow_and_reject_malformed_subjects() {
        for (verb, flag, value) in [
            ("find", "--text", "완료"),
            ("foreground", "--app", "com.example.app"),
        ] {
            let base = document("dry");
            let text = format!(
                "{}{}\n\n1. `zerocode-emulator {verb} --platform android --device phone {flag} {value}` — state, required\n",
                base.split(FLOW_HEADING_CHECKS).next().unwrap(),
                FLOW_HEADING_CHECKS
            );
            let spec = parse_flow(&text).unwrap().unwrap();
            assert_eq!(spec.checks[0].tool, RecipeTool::Emulator);
            assert!(parse_flow(&text.replace(&format!("{flag} {value}"), "--bogus no")).is_err());
        }
    }

    #[test]
    fn a_flow_section_is_read_from_the_same_recipe_document() {
        let text = document("dry");
        let spec = parse_flow(&text).expect("readable").expect("a Flow");
        assert_eq!(spec.policy, Policy::Dry);
        assert_eq!(spec.evidence, EvidenceLevel::Full);
        assert_eq!(
            spec.fingerprint,
            Fingerprint {
                apps: set(&["com.apple.Safari", "com.example.admin"]),
                hosts: set(&["stg-admin.example.internal"]),
                protocol: COMPUTER_USE_PROTOCOL_VERSION,
                assets: set(&["settlement.js?v=13", "transfer.js?v=acctcol20260620"]),
            }
        );
        assert_eq!(spec.checks.len(), 3, "{:?}", spec.checks);
        assert_eq!(
            spec.checks[0],
            Check {
                id: 1,
                kind: CheckKind::Event,
                required: true,
                tool: RecipeTool::Computer,
                argv: words(&["wait-for", "--text", "송금 완료", "--app", "Safari"]),
            }
        );
        assert_eq!(
            (spec.checks[1].tool, &spec.checks[1].argv),
            (RecipeTool::Browser, &words(&["find", "browser-1", "RCPT-"]))
        );
        assert_eq!(
            (spec.checks[2].kind, spec.checks[2].required),
            (CheckKind::State, false)
        );
        // Written back, it reads the same — and the recipe's own steps are
        // untouched by the two new sections.
        let written = spec.written();
        assert_eq!(
            parse_flow(&written).expect("its own writing").as_ref(),
            Some(&spec),
            "{written}"
        );
        assert_eq!(recipe_lines(&text).expect("a recipe").len(), 1);
        assert_eq!(
            serde_json::from_value::<FlowSpec>(serde_json::to_value(&spec).unwrap()).unwrap(),
            spec,
            "the spec rides flow-verdict.json as it is"
        );
        // A recipe without the sections is just a recipe.
        let steps_only =
            format!("{RECIPE_HEADING_STEPS}\n\n1. `zerocode-computer key --key tab`\n");
        assert_eq!(parse_flow(&steps_only), Ok(None));
        // Strict: an unknown key, word or verb is refused, never skipped.
        let flow = |lines: &str, checks: &str| {
            format!("{FLOW_HEADING}\n\n{lines}\n\n{FLOW_HEADING_CHECKS}\n\n{checks}\n")
        };
        let good_lines = format!(
            "- policy: dry\n- evidence: verdict-only\n- fingerprint: protocol={COMPUTER_USE_PROTOCOL_VERSION}"
        );
        let good_check = "1. `zerocode-computer wait-for --app X --text a` — state, required";
        assert!(
            parse_flow(&flow(&good_lines, good_check))
                .unwrap()
                .is_some()
        );
        for (why, lines, checks) in [
            (
                "an unknown key",
                format!("{good_lines}\n- trigger: manual"),
                good_check.to_string(),
            ),
            (
                "an unknown policy word",
                good_lines.replace("dry", "wet"),
                good_check.to_string(),
            ),
            (
                "an unknown evidence word",
                good_lines.replace("verdict-only", "some"),
                good_check.to_string(),
            ),
            (
                "a repeated key",
                format!("{good_lines}\n- policy: dry"),
                good_check.to_string(),
            ),
            (
                "a line that is not `- key: value`",
                format!("{good_lines}\npolicy dry"),
                good_check.to_string(),
            ),
            (
                "a fingerprint without its protocol",
                good_lines.replace(
                    &format!("protocol={COMPUTER_USE_PROTOCOL_VERSION}"),
                    "apps=a",
                ),
                good_check.to_string(),
            ),
            (
                "an unknown fingerprint field",
                format!("{good_lines} tenant=x"),
                good_check.to_string(),
            ),
            (
                "no policy",
                good_lines.replace("- policy: dry\n", ""),
                good_check.to_string(),
            ),
            (
                "a check that is not a check verb",
                good_lines.clone(),
                "1. `zerocode-computer click --app X --x 1 --y 1` — state, required".into(),
            ),
            (
                "a browser check that is not a check verb",
                good_lines.clone(),
                "1. `zerocode-browser click browser-1 a` — state, required".into(),
            ),
            (
                "a browser check with the wrong number of words",
                good_lines.clone(),
                "1. `zerocode-browser find browser-1` — state, required".into(),
            ),
            (
                "a check line without its marks",
                good_lines.clone(),
                "1. `zerocode-computer wait-for --app X --text a`".into(),
            ),
            (
                "an unknown mark word",
                good_lines.clone(),
                "1. `zerocode-computer wait-for --app X --text a` — event, maybe".into(),
            ),
            (
                "a third mark",
                good_lines.clone(),
                "1. `zerocode-computer wait-for --app X --text a` — event, required, fast".into(),
            ),
            (
                "a check whose span does not close",
                good_lines.clone(),
                "1. `zerocode-computer wait-for --app X --text \"a` — event, required".into(),
            ),
            ("no check at all", good_lines.clone(), "prose only".into()),
            (
                "no required check",
                good_lines.clone(),
                "1. `zerocode-computer wait-for --app X --text a` — state, optional".into(),
            ),
        ] {
            assert!(
                parse_flow(&flow(&lines, &checks)).is_err(),
                "{why} was read as a Flow:\n{lines}\n{checks}"
            );
        }
        assert!(
            parse_flow(&format!("{FLOW_HEADING_CHECKS}\n\n{good_check}\n")).is_err(),
            "checks without a Flow section"
        );
        assert!(
            parse_flow(&format!("{FLOW_HEADING}\n\n{good_lines}\n")).is_err(),
            "a Flow without its checks"
        );
    }

    #[test]
    fn a_guarded_policy_needs_its_money_line_and_then_parses() {
        let refused = parse_flow(&document("guarded")).unwrap_err();
        assert!(
            refused.contains(FLOW_KEY_MONEY),
            "a guarded Flow without a money line has nothing its contracts guard: {refused}"
        );
        let spec = parse_flow(&money_document("guarded", None, &[3]))
            .expect("readable")
            .expect("a Flow");
        assert_eq!(spec.policy, Policy::Guarded);
        assert!(
            spec.money.is_some(),
            "the second wave builds guarded around its money step"
        );
    }

    #[test]
    fn an_event_line_that_already_held_at_the_baseline_is_stale_not_pass() {
        let checks = [check(1, CheckKind::Event, true)];
        let judged = |baseline: Presence, seen: Presence| {
            judge(
                &checks,
                &BTreeMap::from([(1, baseline)]),
                &BTreeMap::from([(1, Observed::Seen(seen))]),
            )
        };
        let stale = judged(present(None), present(None));
        assert_eq!(status_of(&stale, 1), LineStatus::Stale);
        assert!(!stale.pass, "{stale:?}");
        let flipped = judged(absent(), present(None));
        assert_eq!(status_of(&flipped, 1), LineStatus::Pass);
        assert!(flipped.pass, "{flipped:?}");
        let grew = judged(present(Some(2)), present(Some(3)));
        assert_eq!(
            status_of(&grew, 1),
            LineStatus::Pass,
            "one more than the baseline is a new event"
        );
        let same_count = judged(present(Some(2)), present(Some(2)));
        assert_eq!(status_of(&same_count, 1), LineStatus::Stale);
        let never = judged(absent(), absent());
        assert_eq!(status_of(&never, 1), LineStatus::Fail);
        assert!(!never.pass);
        // A state line reads the observation alone: the baseline is no evidence
        // against it.
        let state = judge(
            &[check(1, CheckKind::State, true)],
            &BTreeMap::from([(1, present(None))]),
            &BTreeMap::from([(1, Observed::Seen(present(None)))]),
        );
        assert_eq!(status_of(&state, 1), LineStatus::Pass);
        assert!(state.pass);
        // An event line without a baseline cannot be judged an event.
        let unprobed = judge(
            &checks,
            &BTreeMap::new(),
            &BTreeMap::from([(1, Observed::Seen(present(None)))]),
        );
        assert_eq!(status_of(&unprobed, 1), LineStatus::NotEvaluable);
        assert!(!unprobed.pass);
        for line in &stale.lines {
            assert!(!line.said.is_empty(), "{line:?}");
        }
    }

    #[test]
    fn a_required_line_that_cannot_be_evaluated_blocks_a_green_verdict() {
        let checks = [
            check(1, CheckKind::Event, true),
            check(2, CheckKind::State, false),
        ];
        let baseline = BTreeMap::from([(1, absent())]);
        let blocked = judge(
            &checks,
            &baseline,
            &BTreeMap::from([
                (1, Observed::NotEvaluable("판이 죽었다".into())),
                (2, Observed::Seen(present(None))),
            ]),
        );
        assert_eq!(status_of(&blocked, 1), LineStatus::NotEvaluable);
        assert!(!blocked.pass, "{blocked:?}");
        assert!(
            blocked.lines[0].said.contains("판이 죽었다"),
            "the reason rides the line: {:?}",
            blocked.lines[0]
        );
        let green = judge(
            &checks,
            &baseline,
            &BTreeMap::from([
                (1, Observed::Seen(present(None))),
                (2, Observed::NotEvaluable("없는 판".into())),
            ]),
        );
        assert_eq!(status_of(&green, 2), LineStatus::NotEvaluable);
        assert!(green.pass, "an optional line never blocks: {green:?}");
        // A line nobody observed is not evaluable either — never a pass.
        let unobserved = judge(&checks, &baseline, &BTreeMap::new());
        assert_eq!(status_of(&unobserved, 1), LineStatus::NotEvaluable);
        assert!(!unobserved.pass);
        // A verdict nobody can fail is not a verdict.
        let nothing_required = judge(
            &[check(1, CheckKind::State, false)],
            &BTreeMap::new(),
            &BTreeMap::from([(1, Observed::Seen(present(None)))]),
        );
        assert!(!nothing_required.pass);
        assert_eq!(
            serde_json::from_value::<FlowVerdict>(serde_json::to_value(&green).unwrap()).unwrap(),
            green,
            "the verdict rides flow-verdict.json as it is"
        );
        assert_eq!(
            serde_json::to_value(LineStatus::NotEvaluable).unwrap(),
            json!("not-evaluable")
        );
    }

    #[test]
    fn a_fingerprint_matches_by_identity_and_ignores_asset_versions() {
        let recorded = Fingerprint {
            apps: set(&["com.apple.Safari", "Plain"]),
            hosts: set(&["stg-admin.example.internal"]),
            protocol: COMPUTER_USE_PROTOCOL_VERSION,
            assets: set(&["settlement.js?v=13"]),
        };
        let live = Fingerprint::observe(&[
            json!({ "ok": true, "result": { "snapshot": { "app": { "name": "Safari", "bundleId": "com.apple.Safari", "pid": 1 } } } }),
            json!({ "ok": true, "result": { "app": { "name": "Plain", "bundleId": null, "pid": 2 } } }),
            json!({ "url": "https://stg-admin.example.internal/login" }),
            json!({ "document": { "status": 200, "url": "http://stg-admin.example.internal:8080/" } }),
            json!([{ "label": "browser-1", "url": "about:blank" }, { "label": "browser-2", "url": "https://stg-admin.example.internal/" }]),
            json!({ "entries": [
                { "kind": "resource", "url": "https://stg-admin.example.internal/js/settlement.js?v=14" },
                { "kind": "resource", "url": "https://cdn.example/x.png?v=3" },
                { "kind": "fetch", "url": "https://api.example/pay" },
                { "kind": "resource", "url": "https://cdn.example/lib.js" }
            ] }),
        ]);
        assert_eq!(live.apps, set(&["com.apple.Safari", "Plain"]));
        assert_eq!(
            live.hosts,
            set(&["stg-admin.example.internal"]),
            "request hosts are not the page's host"
        );
        assert_eq!(live.assets, set(&["settlement.js?v=14"]));
        assert_eq!(live.protocol, COMPUTER_USE_PROTOCOL_VERSION);
        assert_eq!(
            recorded.matches(&live),
            Ok(()),
            "an asset's version is information, not identity"
        );
        let mut elsewhere = live.clone();
        elsewhere.hosts.insert("evil.example".into());
        let stale = recorded.matches(&elsewhere).unwrap_err();
        assert_eq!(stale.missing_hosts, set(&["evil.example"]));
        assert!(stale.missing_apps.is_empty() && stale.protocol.is_none());
        assert!(stale.said().contains("evil.example"), "{}", stale.said());
        let mut other_protocol = live.clone();
        other_protocol.protocol += 1;
        assert_eq!(
            recorded.matches(&other_protocol).unwrap_err().protocol,
            Some((
                COMPUTER_USE_PROTOCOL_VERSION,
                COMPUTER_USE_PROTOCOL_VERSION + 1
            ))
        );
        assert!(recorded.allows(&Target::App("com.apple.Safari".into())));
        assert!(recorded.allows(&Target::Host("stg-admin.example.internal".into())));
        assert!(!recorded.allows(&Target::App("com.apple.Mail".into())));
        assert!(!recorded.allows(&Target::Host("evil.example".into())));
        assert_eq!(
            Fingerprint::observe(&[]).protocol,
            COMPUTER_USE_PROTOCOL_VERSION,
            "the protocol is this build's"
        );
        assert_eq!(
            serde_json::from_value::<Stale>(serde_json::to_value(&stale).unwrap()).unwrap(),
            stale
        );
    }

    /// The document of the design (docs/design/flow-engine-guarded-money-path.md
    /// §6): a money line naming its three parameters, a confirm line, and
    /// the step that carries the money mark.
    fn money_document(policy: &str, confirm: Option<&str>, money_steps: &[usize]) -> String {
        let mark = |at: usize| {
            if money_steps.contains(&at) {
                format!("{RECIPE_MARK_SEPARATOR}{RECIPE_MARK_MONEY}")
            } else {
                String::new()
            }
        };
        let confirm = confirm
            .map(|line| format!("- {FLOW_KEY_CONFIRM}: {line}\n"))
            .unwrap_or_default();
        format!(
            "# 송금\n\n{RECIPE_HEADING_STEPS}\n\n\
             1. `zerocode-browser type browser-1 #recipient {{{{recipient}}}}`{}\n\
             2. `zerocode-browser type browser-1 #amount {{{{amount}}}}`{}\n\
             3. `zerocode-browser click browser-1 #send`{}\n\n\
             {FLOW_HEADING}\n\n\
             - {FLOW_KEY_POLICY}: {policy}\n\
             - {FLOW_KEY_EVIDENCE}: full\n\
             - {FLOW_KEY_FINGERPRINT}: hosts=admin.example protocol={COMPUTER_USE_PROTOCOL_VERSION}\n\
             - {FLOW_KEY_MONEY}: {FLOW_MONEY_ID}=txn {FLOW_MONEY_AMOUNT}=amount {FLOW_MONEY_RECIPIENT}=recipient\n\
             {confirm}\n\
             {FLOW_HEADING_CHECKS}\n\n\
             1. `zerocode-browser find browser-1 \"송금이 완료되었습니다\"` — event, required\n",
            mark(1),
            mark(2),
            mark(3),
        )
    }

    #[test]
    fn a_money_line_names_three_parameters_and_a_confirm_line_is_person_or_a_capped_auto() {
        let spec = parse_flow(&money_document("guarded", None, &[3]))
            .expect("readable")
            .expect("a Flow");
        assert_eq!(
            spec.money,
            Some(Money {
                id: "txn".into(),
                amount: "amount".into(),
                recipient: "recipient".into(),
                step: 3,
            })
        );
        assert_eq!(
            spec.confirm,
            Confirm::Person,
            "the person confirms unless the document says otherwise"
        );
        let capped = parse_flow(&money_document(
            "guarded",
            Some(&format!("{FLOW_CONFIRM_AUTO} {FLOW_CONFIRM_CAP}=50000")),
            &[3],
        ))
        .expect("readable")
        .expect("a Flow");
        assert_eq!(
            capped.confirm,
            Confirm::Auto {
                cap: "50000".into()
            }
        );
        // Written back, both lines read the same; the record rides JSON as it
        // is, and a record without the two lines (the first wave's) still reads.
        for spec in [&spec, &capped] {
            let document = format!(
                "{RECIPE_HEADING_STEPS}\n\n1. `zerocode-browser click browser-1 #send`{RECIPE_MARK_SEPARATOR}{RECIPE_MARK_MONEY}\n\n{}",
                spec.written()
            );
            let read_back = FlowSpec {
                money: spec.money.clone().map(|money| Money { step: 1, ..money }),
                ..spec.clone()
            };
            assert_eq!(
                parse_flow(&document).expect("its own writing"),
                Some(read_back),
                "{document}"
            );
            assert_eq!(
                serde_json::from_value::<FlowSpec>(serde_json::to_value(spec).unwrap()).unwrap(),
                *spec
            );
        }
        let first_wave =
            serde_json::to_value(parse_flow(&document("dry")).unwrap().unwrap()).unwrap();
        let read: FlowSpec = serde_json::from_value(first_wave).expect("a record without money");
        assert_eq!((read.money, read.confirm), (None, Confirm::Person));
        // The amount grammar is one: a plain decimal, no sign, no currency, no
        // thousands mark — compared exactly, never as a float.
        let amount = |text: &str| Amount::read(text).unwrap_or_else(|why| panic!("{text}: {why}"));
        assert!(amount("49999.99") <= amount("50000"));
        assert!(amount("50000") <= amount("50000"));
        assert!(amount("50000.01") > amount("50000"));
        assert!(amount("1000") > amount("999.99"));
        assert_eq!(
            amount("0100.50"),
            amount("100.5"),
            "leading and trailing zeros are no digits"
        );
        assert!(
            amount("0.1") < amount("0.2"),
            "no float ever sees the digits"
        );
        for bad in [
            "₩50000", "$5", "50,000", "-1", "+1", "1e3", "", ".", "1.", ".5", "1.2.3", "５",
        ] {
            assert!(Amount::read(bad).is_err(), "`{bad}` read as an amount");
        }
        // Strict: a money line missing a field, a field twice, an unknown
        // field, a value that is no parameter name; a confirm line whose word
        // is unknown, an auto without its cap or with a cap that is no plain
        // number, an extra word; confirm without money; guarded without money.
        let flow = |money: Option<&str>, confirm: Option<&str>, policy: &str| {
            let lines: String = [
                Some(format!("- {FLOW_KEY_POLICY}: {policy}")),
                Some(format!("- {FLOW_KEY_EVIDENCE}: off")),
                Some(format!(
                    "- {FLOW_KEY_FINGERPRINT}: protocol={COMPUTER_USE_PROTOCOL_VERSION}"
                )),
                money.map(|line| format!("- {FLOW_KEY_MONEY}: {line}")),
                confirm.map(|line| format!("- {FLOW_KEY_CONFIRM}: {line}")),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join("\n");
            format!(
                "{RECIPE_HEADING_STEPS}\n\n1. `zerocode-browser click browser-1 #send`{RECIPE_MARK_SEPARATOR}{RECIPE_MARK_MONEY}\n\n\
                 {FLOW_HEADING}\n\n{lines}\n\n{FLOW_HEADING_CHECKS}\n\n\
                 1. `zerocode-browser find browser-1 done` — event, required\n"
            )
        };
        let good_money = "id=txn amount=amount recipient=recipient";
        assert!(
            parse_flow(&flow(Some(good_money), Some("person"), "guarded"))
                .unwrap()
                .is_some()
        );
        assert!(
            parse_flow(&flow(Some(good_money), None, "dry"))
                .unwrap()
                .is_some(),
            "a dry Flow may name its money step: the rehearsal stops before it"
        );
        for (why, money, confirm, policy) in [
            (
                "a money line without its recipient",
                Some("id=txn amount=amount"),
                None,
                "guarded",
            ),
            (
                "a money field twice",
                Some("id=txn id=t amount=amount recipient=recipient"),
                None,
                "guarded",
            ),
            (
                "an unknown money field",
                Some("id=txn amount=amount recipient=recipient currency=KRW"),
                None,
                "guarded",
            ),
            (
                "a money value that is no parameter name",
                Some("id=txn amount=50000 recipient=a b"),
                None,
                "guarded",
            ),
            (
                "a money value that is a literal amount",
                Some("id=txn amount=50000.00 recipient=recipient"),
                None,
                "guarded",
            ),
            (
                "a money word that is not field=value",
                Some("id=txn amount recipient=recipient"),
                None,
                "guarded",
            ),
            (
                "an unknown confirm word",
                Some(good_money),
                Some("nobody"),
                "guarded",
            ),
            (
                "an auto without its cap",
                Some(good_money),
                Some("auto"),
                "guarded",
            ),
            (
                "a cap with a currency sign",
                Some(good_money),
                Some("auto cap=₩50000"),
                "guarded",
            ),
            (
                "a cap with a thousands mark",
                Some(good_money),
                Some("auto cap=50,000"),
                "guarded",
            ),
            (
                "a cap that is a parameter",
                Some(good_money),
                Some("auto cap={{cap}}"),
                "guarded",
            ),
            (
                "an extra confirm word",
                Some(good_money),
                Some("auto cap=1 extra=2"),
                "guarded",
            ),
            (
                "a person with a cap",
                Some(good_money),
                Some("person cap=1"),
                "guarded",
            ),
            (
                "a confirm line without a money line",
                None,
                Some("person"),
                "dry",
            ),
            ("a guarded Flow without a money line", None, None, "guarded"),
        ] {
            assert!(
                parse_flow(&flow(money, confirm, policy)).is_err(),
                "{why} was read as a Flow"
            );
        }
        for key in [FLOW_KEY_MONEY, FLOW_KEY_CONFIRM] {
            assert!(FLOW_KEYS.contains(&key));
        }
    }

    #[test]
    fn a_flow_with_a_money_step_needs_exactly_one_and_an_event_line_that_watches_it() {
        let none = parse_flow(&money_document("guarded", None, &[])).unwrap_err();
        assert!(
            none.contains(RECIPE_MARK_MONEY),
            "no step carries the mark: {none}"
        );
        let two = parse_flow(&money_document("guarded", None, &[1, 3])).unwrap_err();
        assert!(
            two.contains('1') && two.contains('3'),
            "two steps carry the mark, named: {two}"
        );
        let one = parse_flow(&money_document("guarded", None, &[2]))
            .unwrap()
            .unwrap();
        assert_eq!(one.money.as_ref().map(|money| money.step), Some(2));
        // The money step's outcome is watched by an event line that must pass:
        // a Flow whose only required line is a state, or whose event line is
        // optional, does not watch the money.
        let watched_by = |checks: &str| {
            let text = money_document("guarded", None, &[3]);
            let (head, _) = text.split_once(FLOW_HEADING_CHECKS).unwrap();
            format!("{head}{FLOW_HEADING_CHECKS}\n\n{checks}\n")
        };
        for (why, checks) in [
            (
                "a state line alone",
                "1. `zerocode-browser find browser-1 done` — state, required",
            ),
            (
                "an optional event line",
                "1. `zerocode-browser find browser-1 done` — event, optional\n2. `zerocode-browser find browser-1 home` — state, required",
            ),
        ] {
            assert!(
                parse_flow(&watched_by(checks)).is_err(),
                "{why} watched the money step"
            );
        }
        let spec = parse_flow(&watched_by(
            "1. `zerocode-browser find browser-1 done` — event, required\n2. `zerocode-browser find browser-1 home` — state, optional\n3. `zerocode-browser find browser-1 receipt` — event, required",
        ))
        .unwrap()
        .unwrap();
        let money_lines: Vec<usize> = spec.money_lines().map(|check| check.id).collect();
        assert_eq!(
            money_lines,
            [1, 3],
            "the required event lines watch the money"
        );
        // The money moved when every line watching it passed — the state
        // line's word is not the money's.
        let seen = |ids: &[usize]| -> BTreeMap<usize, Observed> {
            (1..=3)
                .map(|id| {
                    let found = if ids.contains(&id) {
                        present(Some(1))
                    } else {
                        absent()
                    };
                    (id, Observed::Seen(found))
                })
                .collect()
        };
        let baseline: BTreeMap<usize, Presence> = [(1, absent()), (3, absent())].into();
        let acted = judge(&spec.checks, &baseline, &seen(&[1, 3]));
        assert!(spec.money_acted(&acted), "{acted:?}");
        let half = judge(&spec.checks, &baseline, &seen(&[1]));
        assert!(!spec.money_acted(&half), "{half:?}");
        let only_state = judge(&spec.checks, &baseline, &seen(&[2]));
        assert!(!spec.money_acted(&only_state));
        assert!(!spec.money_lines().any(|check| check.id == 2));
    }

    /// An emulator answer names its foreground app by its package (an Android
    /// package, or an iOS bundle id) — a bare string beside the desktop's
    /// `{name,bundleId,pid}` — and it lands in `apps` the same way, so an
    /// emulator act is bound to `Target::App` like any other.
    #[test]
    fn a_fingerprint_reads_an_emulator_answers_package() {
        let recorded = Fingerprint {
            apps: set(&["com.example.wallet"]),
            hosts: BTreeSet::new(),
            protocol: COMPUTER_USE_PROTOCOL_VERSION,
            assets: BTreeSet::new(),
        };
        // As the walk reads it (the step's result object) and as the observe
        // test reads it (the whole envelope) alike.
        let from_result = Fingerprint::observe(&[
            json!({ "performed": true, "app": { "package": "com.example.wallet" } }),
        ]);
        assert_eq!(from_result.apps, set(&["com.example.wallet"]));
        let from_envelope = Fingerprint::observe(&[
            json!({ "ok": true, "result": { "performed": true, "app": { "package": "com.example.wallet" } } }),
        ]);
        assert_eq!(from_envelope.apps, set(&["com.example.wallet"]));
        assert_eq!(recorded.matches(&from_envelope), Ok(()));
        assert!(recorded.allows(&Target::App("com.example.wallet".into())));
        // A package the recording never saw is stale, named for the refusal.
        let elsewhere = Fingerprint::observe(&[
            json!({ "ok": true, "result": { "app": { "package": "com.phish.app" } } }),
        ]);
        let stale = recorded.matches(&elsewhere).unwrap_err();
        assert_eq!(stale.missing_apps, set(&["com.phish.app"]));
        // No package, no app: the door's other answers add nothing.
        assert!(
            Fingerprint::observe(&[json!({ "ok": true, "result": { "performed": true } })])
                .apps
                .is_empty()
        );
    }

    #[test]
    fn the_flow_tables_fit_the_checks_and_the_run() {
        const {
            assert!(FLOW_VERIFY_MS >= RECIPE_CHECK_MS);
            assert!(FLOW_VERIFY_MS <= COMPUTER_WAIT_FOR_MAX_MS);
            assert!(FLOW_VERIFY_MS < walk_budget_ms(COMPUTER_USE_DEADLINE_SECONDS * 1_000));
            assert!(FLOW_BASELINE_PROBE_MS == COMPUTER_WAIT_FOR_MIN_MS);
            assert!(FLOW_BASELINE_PROBE_MS < COMPUTER_WAIT_FOR_POLL_MS);
        }
        assert_eq!(
            desktop_wait_for_ms(Some(FLOW_BASELINE_PROBE_MS)),
            (FLOW_BASELINE_PROBE_MS, false),
            "the probe is one look, as the table allows"
        );
        assert!(
            parse_command(&words(&[
                "wait-for",
                "--app",
                "X",
                "--text",
                "a",
                "--timeout-ms",
                &FLOW_BASELINE_PROBE_MS.to_string()
            ]))
            .is_ok()
        );
        for policy in Policy::ALL {
            assert_eq!(Policy::from_word(policy.as_str()), Some(policy));
            assert_eq!(
                serde_json::to_value(policy).unwrap(),
                json!(policy.as_str()),
                "the JSON word is the grammar's"
            );
        }
        for level in EvidenceLevel::ALL {
            assert_eq!(EvidenceLevel::from_word(level.as_str()), Some(level));
            assert_eq!(serde_json::to_value(level).unwrap(), json!(level.as_str()));
            assert_eq!(
                level.frames(),
                level == EvidenceLevel::Full,
                "only full keeps frames; a verdict is written at every level"
            );
        }
        for kind in CheckKind::ALL {
            assert_eq!(CheckKind::from_word(kind.as_str()), Some(kind));
            assert_eq!(serde_json::to_value(kind).unwrap(), json!(kind.as_str()));
        }
        assert_eq!(FLOW_KEYS.len(), 6);
        assert_ne!(FLOW_WORD_REQUIRED, FLOW_WORD_OPTIONAL);
        assert_eq!(
            serde_json::to_value(RecipeTool::Browser).unwrap(),
            json!(RecipeTool::Browser.as_str())
        );
        // The verify flag is parsed today; F1 gives it its work.
        let verify = parse_command(&words(&["evidence", "--verify", "--json"])).expect("parsed");
        assert_eq!(verify.params["verify"], json!(true));
        assert!(usage().contains("evidence [--last N] [--verify] [--json]"));
        let codes = [
            error_code::FLOW_STALE,
            error_code::FLOW_ENV_MISMATCH,
            error_code::FLOW_MONEY_UNBOUND,
            error_code::FLOW_PAGE_DISAGREES,
            error_code::FLOW_TXN_SEEN,
            error_code::FLOW_CONFIRM_UNBOUND,
        ];
        assert_eq!(
            codes,
            [
                "flow_stale",
                "flow_env_mismatch",
                "flow_money_unbound",
                "flow_page_disagrees",
                "flow_txn_seen",
                "flow_confirm_unbound",
            ]
        );
        // The confirm flag rides recipe-run alone, as the transaction's id.
        let confirmed = parse_command(&words(&[
            "recipe-run",
            "--name",
            "pay",
            "--confirm",
            "TXN-1",
        ]))
        .expect("parsed");
        assert_eq!(confirmed.params["confirm"], json!("TXN-1"));
        let range = parse_command(&words(&[
            "recipe-run",
            "--name",
            "pay",
            "--start",
            "2",
            "--end",
            "2",
        ]))
        .unwrap();
        assert_eq!(range.params["end"], json!(2));
        assert!(parse_command(&words(&["recipe-run", "--name", "pay", "--end", "0"])).is_err());
        assert!(
            parse_command(&words(&[
                "recipe-run",
                "--name",
                "pay",
                "--end",
                "2",
                "--repeat"
            ]))
            .is_err()
        );
        assert!(usage().contains(
            "recipe-run --name <name> [--params '{\"name\":\"value\"}'] [--start N] [--end N] [--confirm <txn>] [--repeat [--until <HH:MM|N>]] [--arena <evidence dir>] [--json]"
        ));
        assert!(
            parse_command(&words(&["recipe-show", "--name", "pay", "--confirm", "x"])).is_err()
        );
    }

    /// A `## Trigger` (design §2 `event`, plan §1 of the second wave): one
    /// check line that says what starts a round — a numbered code span with
    /// `— event, required` and nothing else. A state is not a trigger (a
    /// trigger is something that happens), an optional trigger starts
    /// nothing, two lines would be two triggers, and a trigger without a
    /// Flow has no round to start. Written back, it reads the same; a record
    /// written before triggers still reads.
    #[test]
    fn a_trigger_section_holds_one_event_line_and_nothing_else() {
        let line = "1. `zerocode-browser find browser-1 \"입금\"` — event, required";
        let with_trigger =
            |trigger: &str| format!("{}\n{FLOW_HEADING_TRIGGER}\n\n{trigger}\n", document("dry"));
        let spec = parse_flow(&with_trigger(line))
            .expect("readable")
            .expect("a Flow");
        assert_eq!(
            spec.trigger,
            Some(Check {
                id: 1,
                kind: CheckKind::Event,
                required: true,
                tool: RecipeTool::Browser,
                argv: words(&["find", "browser-1", "입금"]),
            })
        );
        assert_eq!(spec.checks.len(), 3, "the oracle lines are their own");
        let written = spec.written();
        assert!(written.contains(FLOW_HEADING_TRIGGER), "{written}");
        assert_eq!(
            parse_flow(&written).expect("its own writing").as_ref(),
            Some(&spec),
            "{written}"
        );
        let json = serde_json::to_value(&spec).unwrap();
        assert!(
            json.get("trigger").is_some(),
            "the trigger rides the record"
        );
        let mut earlier = json.clone();
        earlier.as_object_mut().unwrap().remove("trigger");
        assert_eq!(
            serde_json::from_value::<FlowSpec>(earlier).unwrap().trigger,
            None,
            "a record written before triggers still reads"
        );
        assert_eq!(
            parse_flow(&document("dry")).unwrap().unwrap().trigger,
            None,
            "no section, no trigger: a repeat walks on its caller's clock"
        );
        for (why, trigger) in [
            (
                "a state line",
                "1. `zerocode-browser find browser-1 \"입금\"` — state, required",
            ),
            (
                "an optional line",
                "1. `zerocode-browser find browser-1 \"입금\"` — event, optional",
            ),
            (
                "two lines",
                "1. `zerocode-browser find browser-1 a` — event, required\n2. `zerocode-browser find browser-1 b` — event, required",
            ),
            ("no line", "prose only"),
            (
                "a line that is not a check",
                "1. `zerocode-browser click browser-1 a` — event, required",
            ),
            (
                "a line without its marks",
                "1. `zerocode-browser find browser-1 a`",
            ),
        ] {
            assert!(
                parse_flow(&with_trigger(trigger)).is_err(),
                "{why} was read as a trigger:\n{trigger}"
            );
        }
        let state = parse_flow(&with_trigger(
            "1. `zerocode-browser find browser-1 a` — state, required",
        ))
        .unwrap_err();
        assert!(
            state.contains("일어나는 일"),
            "the refusal names the rule: {state}"
        );
        assert!(
            parse_flow(&format!(
                "{RECIPE_HEADING_STEPS}\n\n1. `zerocode-computer key --key tab`\n\n{FLOW_HEADING_TRIGGER}\n\n{line}\n"
            ))
            .is_err(),
            "a trigger without a Flow has no round to start"
        );
    }
}
