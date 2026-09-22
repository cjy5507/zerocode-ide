//! The one door every Jev request passes before a byte of it leaves the
//! machine (docs/design/jev-settings-20260917.md §3).
//!
//! In order, each refusal named by its own token ([`Refused`]):
//!
//! 1. a key to send with — `no_key`;
//! 2. Jev switched on (`smart.jev.enabled`) — `off`;
//! 3. the person's consent for the workspace the words come from
//!    (`smart.jev.workspaces`) — `not_consented`;
//! 4. room in the day's budget (`smart.jev.dailyRequests`) — `budget`.
//!
//! A request that passes all four is cleared. Every text its use's row points
//! at ([`super::Sent`]) loses each line that may carry a credential — the whole
//! line, because a mask cannot see a value under a name that says nothing
//! ([`crate::credential::may_carry_a_credential`], the one table every road
//! that must not carry a credential onward reads) — and is then cut to its
//! cap. What comes out is the request body's bytes, and the programs that ask
//! send those and nothing else.
//!
//! Consent is read from the person's own settings file and nowhere else. A
//! repository arrives with its own `.zo/settings.json`; if that file could
//! name the workspace it sits in, a clone would consent for the person. What
//! a consented root carries with it is its own linked worktrees
//! ([`JevSettings::consents`]), which that same clone cannot become.
//!
//! [`may_send`] is pure. [`pass`] reads the day's count and counts the request
//! the door lets through ([`super::count`]); reading the settings and writing a
//! refusal's row belong to the program that asks.
//!
//! A memo stands between the door and the wire ([`pass_remembering`],
//! [`super::memo`]): once the four questions are answered, the cleared bytes
//! are looked up, and a hit under a seat that may answer from it is a request
//! that passed and does not leave — no byte sent, no place taken in the day.
//! The memo is asked after consent and the switch, never before: a memo hit
//! is still the person's words being judged, and a workspace they withdrew
//! consent from is answered by nothing, remembered or not.
//!
//! A judgment asked twice meets the fourth question twice. The second request
//! ([`super::hedge`]) carries the bytes the door already cleared, so consent,
//! the switch and the key have been answered for it; what has not is the day's
//! budget, and [`count_a_hedge`] asks that one before the second request may
//! leave.

use std::path::Path;
use std::time::Instant;

use serde_json::Value;

use super::{CUT_MARK, Cap, JevMode, JevUse, SMART_SETTINGS_KEY, count, memo};
use crate::credential::{MASK, may_carry_a_credential};

/// The object under `smart` the door's own settings live in.
pub const JEV_SETTINGS_KEY: &str = "jev";
/// `smart.jev.enabled`: `false` stops every use at once.
pub const ENABLED_SETTING: &str = "enabled";
/// `smart.jev.workspaces`: the workspace roots a person consented to send
/// words from. A folder under a root is consented; a folder beside it is not.
pub const WORKSPACES_SETTING: &str = "workspaces";
/// `smart.jev.dailyRequests`: the most requests one local day may send. Unset,
/// every request is counted and none is refused for the count — a default is
/// chosen from measured days, never written before there are any.
pub const DAILY_REQUESTS_SETTING: &str = "dailyRequests";

/// The key a ledger row keeps how many requests it sent under: `0` when the
/// door refused it or a memo answered.
pub const REQUESTS_KEY: &str = "requests";
/// The key a ledger row keeps how many lines the door withheld under. Every
/// row written since the door carries it, so a row without it predates the
/// door.
pub const REDACTED_LINES_KEY: &str = "redactedLines";

/// What a ledger row's `outcome` says when an answer arrived and passed the
/// use's own checks. Every Jev ledger spells it this way, and one word is what
/// lets a reader of somebody else's ledger — the hedge rule's sample of past
/// latencies ([`super::hedge`]) — know an answer from a wall.
pub const ANSWERED_OUTCOME: &str = "answered";

/// What a withheld line reads as in what is sent.
pub const WITHHELD_LINE: &str = MASK;

/// Whether a ledger row was written before the door existed: a JSON object
/// without [`REDACTED_LINES_KEY`]. A line that is not a JSON object says
/// nothing either way, and reads as not.
#[must_use]
pub fn predates_the_door(row: &str) -> bool {
    serde_json::from_str::<Value>(row)
        .ok()
        .and_then(|row| {
            row.as_object()
                .map(|fields| !fields.contains_key(REDACTED_LINES_KEY))
        })
        .unwrap_or(false)
}

/// The door's settings, as the person's own settings file holds them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JevSettings {
    pub enabled: bool,
    pub workspaces: Vec<String>,
    /// `None` when unset: counted, never refused for the count.
    pub daily_requests: Option<u64>,
}

impl JevSettings {
    /// The door's settings in a settings document (`smart.jev`). Absent, Jev is
    /// on, nothing is consented and nothing is capped. A value of the wrong
    /// shape never widens what is sent: a switch that is not `true` or `false`
    /// is off, a workspace that is not a non-empty string is skipped, and a
    /// budget that is not a whole number sends nothing.
    #[must_use]
    pub fn from_root(root: &Value) -> Self {
        let Some(jev) = root
            .get(SMART_SETTINGS_KEY)
            .and_then(|smart| smart.get(JEV_SETTINGS_KEY))
        else {
            return Self {
                enabled: true,
                workspaces: Vec::new(),
                daily_requests: None,
            };
        };
        let Some(jev) = jev.as_object() else {
            return Self {
                enabled: false,
                workspaces: Vec::new(),
                daily_requests: None,
            };
        };
        let enabled = jev.get(ENABLED_SETTING).is_none_or(|on| on == true);
        let workspaces = jev
            .get(WORKSPACES_SETTING)
            .and_then(Value::as_array)
            .map(|roots| {
                roots
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::trim)
                    .filter(|root| !root.is_empty())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        let daily_requests = match jev.get(DAILY_REQUESTS_SETTING) {
            None | Some(Value::Null) => None,
            Some(cap) => Some(cap.as_u64().unwrap_or(0)),
        };
        Self {
            enabled,
            workspaces,
            daily_requests,
        }
    }

    /// Whether the person consented to the words of `workspace`: a consented
    /// root, a folder under one, or a linked git worktree of a checkout that
    /// is one.
    ///
    /// The worktree arm is what makes a run of workers answerable. A worker
    /// works in a checkout cut from the person's own — `git worktree add` —
    /// and named a folder the person never typed, so on the rule's first
    /// spelling 86% of one run's step-effort rows were `not_consented`
    /// (t-4781): a whole run of judgments refused for a repository the person
    /// HAD consented to. It does not reopen what this module's head refuses:
    /// consent is still read from the person's own settings file, and a clone
    /// cannot make itself a worktree of a consented checkout — only that
    /// checkout can cut one.
    ///
    /// A linked checkout must be registered by its owning repository: the
    /// untrusted checkout's own pointer alone cannot grant consent. A root
    /// that is directly consented needs no Git ownership lookup.
    #[must_use]
    pub fn consents(&self, workspace: &str) -> bool {
        let resolved;
        let workspace = if Path::new(workspace).is_absolute() {
            resolved = resolved_path(Path::new(workspace));
            resolved.as_str()
        } else {
            workspace
        };
        self.names(workspace) || self.owns_a_consented_checkout(Path::new(workspace))
    }

    /// The checkout that owns `workspace`'s repository is one of the consented
    /// roots — the arm that answers for a linked worktree.
    ///
    /// A workspace that is not an absolute path is one nobody could name, and
    /// consents to nothing, exactly as a workspace of `None` does: asking git
    /// about a relative path would ask about the ASKING PROGRAM's directory,
    /// and a crate test's cwd is not the words' workspace.
    fn owns_a_consented_checkout(&self, workspace: &Path) -> bool {
        workspace.is_absolute()
            && crate::git_dir::owning_checkout_of(workspace)
                .is_some_and(|checkout| self.names(&resolved_path(&checkout)))
    }

    /// One of the consented roots IS `workspace` or holds it, compared segment
    /// by segment (`/a/bc` is not under `/a/b`).
    fn names(&self, workspace: &str) -> bool {
        if Path::new(workspace)
            .components()
            .any(|part| part == std::path::Component::ParentDir)
        {
            return false;
        }
        self.workspaces
            .iter()
            .any(|root| crate::vault::inside_or_equal(root, workspace))
    }

    /// These settings with each root spelled as the filesystem spells it, the
    /// way the asking program spells its working directory (`/tmp` is
    /// `/private/tmp` on macOS). A root that does not resolve is kept as
    /// written.
    #[must_use]
    pub fn resolved(mut self) -> Self {
        for root in &mut self.workspaces {
            *root = resolved_path(Path::new(root.as_str()));
        }
        self
    }
}

/// `path` as the filesystem spells it, or as given when it does not resolve.
#[must_use]
pub fn resolved_path(path: &Path) -> String {
    std::fs::canonicalize(path)
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .into_owned()
}

/// Why the door sent nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refused {
    /// No key to send with.
    NoKey,
    /// `smart.jev.enabled` is `false`.
    Off,
    /// The words come from a workspace the person has not consented to.
    NotConsented,
    /// The day's budget is spent.
    Budget,
}

impl Refused {
    /// Every refusal, in the order the door asks.
    pub const ALL: [Self; 4] = [Self::NoKey, Self::Off, Self::NotConsented, Self::Budget];

    /// The token a ledger row, a counter and a pane name this refusal by.
    #[must_use]
    pub const fn token(self) -> &'static str {
        match self {
            Self::NoKey => "no_key",
            Self::Off => JevMode::Off.key(),
            Self::NotConsented => "not_consented",
            Self::Budget => "budget",
        }
    }

    /// The refusal a ledger token names, if it names one.
    #[must_use]
    pub fn from_token(token: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|refused| refused.token() == token)
    }
}

/// What the door is told about one request.
#[derive(Debug, Clone, Copy)]
pub struct Asking<'a> {
    /// A key is configured to send with.
    pub key: bool,
    pub settings: &'a JevSettings,
    /// The workspace the words come from, spelled as the filesystem spells it;
    /// `None` when the asking program cannot say, which consents to nothing.
    pub workspace: Option<&'a str>,
    /// Requests already counted today, across every use and program.
    pub sent_today: u64,
}

/// A request the door let through: the body's bytes, and how many lines of
/// what is sent were withheld.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cleared {
    bytes: Vec<u8>,
    withheld_lines: usize,
}

impl Cleared {
    /// The request body, exactly as it may be sent.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    #[must_use]
    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }

    /// Lines withheld from what is sent because they may carry a credential.
    #[must_use]
    pub const fn withheld_lines(&self) -> usize {
        self.withheld_lines
    }
}

/// Whether one more request fits in the day, `sent` already counted.
#[must_use]
pub fn within_budget(settings: &JevSettings, sent: u64) -> bool {
    settings.daily_requests.is_none_or(|most| sent < most)
}

/// Ask the door about one request of `row`'s, its body built as the use
/// builds it. Cleared, the body's pointed-at texts have lost every line that
/// may carry a credential and are cut to their caps.
///
/// # Errors
/// The first of the door's four questions the request fails.
pub fn may_send(row: &JevUse, asking: &Asking<'_>, mut body: Value) -> Result<Cleared, Refused> {
    admit(asking, true)?;
    let mut withheld_lines = 0;
    for sent in row.sends {
        let path: Vec<&str> = sent.at.split('/').skip(1).collect();
        visit(&mut body, &path, &mut |value| {
            withheld_lines += clear(value, sent.cap);
        });
    }
    Ok(Cleared {
        bytes: body.to_string().into_bytes(),
        withheld_lines,
    })
}

/// Ask the door about a key check: a sentence no person wrote, sent when a
/// person asks whether a key works. No workspace's words go, so consent is not
/// asked; a key, the switch and the budget are.
///
/// # Errors
/// No key, Jev off, or the day's budget spent.
pub fn may_check_key(asking: &Asking<'_>, body: &Value) -> Result<Cleared, Refused> {
    admit(asking, false)?;
    Ok(Cleared {
        bytes: body.to_string().into_bytes(),
        withheld_lines: 0,
    })
}

/// Ask the door and count what it lets through: the day's count is read from
/// `requests` ([`count::requests_path`]) before asking, and the request is
/// counted only once cleared. A request that finds the day's last place taken
/// by one counted at the same moment is refused after all. A count that
/// cannot be written refuses only when a budget is set — then the door could
/// not keep it; unset, the request goes uncounted.
///
/// # Errors
/// As [`may_send`] or [`may_check_key`], and [`Refused::Budget`] for a
/// request that lost the day's last place.
pub fn pass(
    ask: impl FnOnce(&Asking<'_>) -> Result<Cleared, Refused>,
    key: bool,
    settings: &JevSettings,
    workspace: Option<&str>,
    requests: &Path,
) -> Result<Cleared, Refused> {
    pass_remembering(ask, key, settings, workspace, requests, None).map(|passed| passed.cleared)
}

/// The memo a request may be answered from, when the asking seat keeps one.
#[derive(Debug, Clone, Copy)]
pub struct Memo<'a> {
    /// The memo file ([`memo::MEMO_FILE`] under the Jev folder).
    pub path: &'a Path,
    /// The seat whose question this is — half of the memo's key.
    pub seat: &'a JevUse,
    /// Whether a hit may answer in the wire's place. Under `shadow`, and
    /// under an `auto` nobody has raised, the memo is looked up and the
    /// request still goes, so the two answers can be compared.
    pub applying: bool,
}

/// What the memo said about one cleared request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Memoed {
    /// The key the request was looked up under ([`memo::key_of`]).
    pub key: String,
    /// The answer the memo held, if it held one.
    pub recalled: Option<memo::Recalled>,
    /// How long the lookup took.
    pub lookup_ms: u64,
    /// Whether the memo answers this request: a hit under a seat that may
    /// apply it. Then the request was not counted and must not be sent.
    pub answered: bool,
}

/// A request the door let through, with what the memo knew of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Passed {
    pub cleared: Cleared,
    /// `None` when no memo was asked — a seat whose cache is off, or a use
    /// that keeps none.
    pub memo: Option<Memoed>,
}

/// [`pass`], with the memo between the door and the count: the four questions
/// first, then the lookup of the cleared bytes, then the day's place — taken
/// only for a request that will leave. A hit under `applying` leaves nothing
/// and takes nothing; a hit under a seat that only compares is counted like
/// any request, because the request still goes.
///
/// # Errors
/// As [`pass`]. A memo hit never widens what the door lets through: every
/// refusal is asked before the memo is.
pub fn pass_remembering(
    ask: impl FnOnce(&Asking<'_>) -> Result<Cleared, Refused>,
    key: bool,
    settings: &JevSettings,
    workspace: Option<&str>,
    requests: &Path,
    memo: Option<Memo<'_>>,
) -> Result<Passed, Refused> {
    let asking = Asking {
        key,
        settings,
        workspace,
        sent_today: count::sent(requests),
    };
    let cleared = ask(&asking)?;
    let memo = memo.map(|memo| {
        let looking = Instant::now();
        let key = memo::key_of(memo.seat, cleared.bytes());
        let recalled = memo::recall(memo.path, &key);
        Memoed {
            answered: memo.applying && recalled.is_some(),
            key,
            recalled,
            lookup_ms: u64::try_from(looking.elapsed().as_millis()).unwrap_or(u64::MAX),
        }
    });
    if memo.as_ref().is_some_and(|memo| memo.answered) {
        return Ok(Passed { cleared, memo });
    }
    take_a_place(settings, requests).map(|()| Passed { cleared, memo })
}

/// Take the day's place for a second request of a judgment already cleared —
/// the hedge [`super::hedge::plan`] names, whose whole cost is that one extra
/// request.
///
/// A hedge is not a second judgment: the door has already asked its four
/// questions of these same bytes, and the only one that can answer
/// differently for the second copy is the budget, because the second copy is
/// the person's money spent twice. So this asks that one question, with the
/// same primitive [`pass`] counts with, and a hedge it refuses must not be
/// sent. Count first, send second: a hedge counted after it left is a budget
/// that learns of the spending too late to stop it.
///
/// # Errors
/// [`Refused::Budget`] when the day has no place left for it.
pub fn count_a_hedge(settings: &JevSettings, requests: &Path) -> Result<(), Refused> {
    take_a_place(settings, requests)
}

/// Count one request in the day and answer whether the day had room for it.
///
/// The count is written before the answer is known, so a request that lost
/// the day's last place to one counted at the same moment is refused with its
/// byte already spent: the count may say one more request than went out, and
/// never one fewer, which is the direction a budget may err in.
fn take_a_place(settings: &JevSettings, requests: &Path) -> Result<(), Refused> {
    match count::count_one(requests) {
        Ok(place) if within_budget(settings, place.saturating_sub(1)) => Ok(()),
        Ok(_) => Err(Refused::Budget),
        Err(_) if settings.daily_requests.is_some() => Err(Refused::Budget),
        Err(_) => Ok(()),
    }
}

fn admit(asking: &Asking<'_>, for_a_workspace: bool) -> Result<(), Refused> {
    if !asking.key {
        return Err(Refused::NoKey);
    }
    if !asking.settings.enabled {
        return Err(Refused::Off);
    }
    if for_a_workspace
        && !asking
            .workspace
            .is_some_and(|workspace| asking.settings.consents(workspace))
    {
        return Err(Refused::NotConsented);
    }
    if !within_budget(asking.settings, asking.sent_today) {
        return Err(Refused::Budget);
    }
    Ok(())
}

/// Every value `path` reaches under `value`; `*` fans out over an array's
/// elements or an object's values.
fn visit(value: &mut Value, path: &[&str], found: &mut dyn FnMut(&mut Value)) {
    let Some((step, rest)) = path.split_first() else {
        found(value);
        return;
    };
    match (value, *step) {
        (Value::Array(items), "*") => {
            for item in items {
                visit(item, rest, found);
            }
        }
        (Value::Object(fields), "*") => {
            for item in fields.values_mut() {
                visit(item, rest, found);
            }
        }
        (Value::Object(fields), key) => {
            if let Some(item) = fields.get_mut(key) {
                visit(item, rest, found);
            }
        }
        (Value::Array(items), index) => {
            if let Some(item) = index.parse::<usize>().ok().and_then(|at| items.get_mut(at)) {
                visit(item, rest, found);
            }
        }
        _ => {}
    }
}

/// One pointed-at value cleared under `cap`: a text as [`clear_text`] leaves
/// it, a list cut to its first items. Answers the lines withheld from what is
/// kept.
fn clear(value: &mut Value, cap: Cap) -> usize {
    match value {
        Value::String(text) => {
            let (kept, withheld) = clear_text(text, cap);
            *text = kept;
            withheld
        }
        Value::Array(items) => {
            if let Cap::Items(most) = cap {
                items.truncate(most);
            }
            0
        }
        _ => 0,
    }
}

/// `text` as it may be sent under `cap` — each line that may carry a
/// credential replaced by [`WITHHELD_LINE`], then cut — and how many withheld
/// lines are in what is kept. Reading stops once what is kept has passed the
/// cap, since nothing after that point is sent; the result is the same as
/// clearing the whole text and cutting it.
#[must_use]
pub fn clear_text(text: &str, cap: Cap) -> (String, usize) {
    let mut kept = String::new();
    let mut withheld_at = Vec::new();
    let mut chars = 0;
    for (index, line) in text.split('\n').enumerate() {
        if index > 0 {
            kept.push('\n');
            chars += 1;
        }
        let past = match cap {
            Cap::Chars(most) => chars > most,
            Cap::Bytes(most) => kept.len() > most,
            Cap::Items(_) | Cap::Uncut => false,
        };
        if past {
            break;
        }
        if may_carry_a_credential(line) {
            withheld_at.push(kept.len());
            kept.push_str(WITHHELD_LINE);
            chars += WITHHELD_LINE.chars().count();
        } else {
            kept.push_str(line);
            chars += line.chars().count();
        }
    }
    let end = cut_in_place(&mut kept, cap);
    let withheld = withheld_at.into_iter().filter(|at| *at < end).count();
    (kept, withheld)
}

/// `text` cut to `cap` and nothing else: characters on a character boundary;
/// bytes on a character boundary with [`CUT_MARK`] after the cut. Cutting a
/// text this already cut leaves it as it was.
#[must_use]
pub fn cut(text: &str, cap: Cap) -> String {
    let mut text = text.to_string();
    cut_in_place(&mut text, cap);
    text
}

/// Cut in place; answer where the kept words end, before any mark.
fn cut_in_place(text: &mut String, cap: Cap) -> usize {
    match cap {
        Cap::Chars(most) => {
            if let Some((end, _)) = text.char_indices().nth(most) {
                text.truncate(end);
            }
        }
        Cap::Bytes(most) if text.len() > most => {
            let mut end = most;
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            text.truncate(end);
            text.push_str(CUT_MARK);
            return end;
        }
        Cap::Bytes(_) | Cap::Items(_) | Cap::Uncut => {}
    }
    text.len()
}

#[cfg(test)]
mod tests;
