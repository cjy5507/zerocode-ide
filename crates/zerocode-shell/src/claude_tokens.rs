//! What Claude actually spent, read off the transcripts it already wrote.
//!
//! The plan gauge in the status bar answers "how much of my quota is left".
//! This answers a different question — "where did the tokens go" — and the
//! window has never been able to answer it at all. Orca has a whole screen for
//! it (`src/main/claude-usage/`), built the same way: nobody reports these
//! numbers, so they are counted from the JSONL the vendor leaves behind.
//!
//! **The counting is the hard part, and a naive count is wrong by half.**
//! Measured on this machine's own corpus — 733 files, 915 MB, 245,578 lines —
//! 107,686 assistant records carry a `usage` block, but only 55,806 of them
//! are distinct: **45.3% of the naive token total is the same record counted
//! again**, and half the output tokens (50.3%) move when the duplicates are
//! folded. A transcript is appended to as a turn streams and re-read on
//! resume, so the same message lands more than once, sometimes with a
//! partially filled block. That is why the key and the merge below exist, and
//! why a `wc`-style tally would produce a confident number that is nearly
//! twice the truth.
//!
//! This slice counts. It does not price: a dollar figure is a constant table
//! that no test can catch being wrong, so it lands separately against a
//! checked reference.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::token_scan;
use crate::usage_places::Places;

use serde::Serialize;
use zerocode_core::civil;

/// Where the vendor keeps the transcripts, under the person's home.
///
/// `~/.claude/projects` alone. Orca also names a `transcripts` root
/// (`transcript-file-discovery.ts`), and it does not exist on this machine —
/// an unexercised root is an untested branch, so it is added when there is
/// something to read in it.
const PROJECTS_DIR: [&str; 2] = [".claude", "projects"];

/// The substring an assistant record cannot avoid carrying.
///
/// Checked before the line is parsed: 56.1% of lines are rejected by it, and
/// `serde_json` on a megabyte of user turn costs far more than a memchr. The
/// false positives (a user turn quoting the word) are dropped by the real
/// `type` check after parsing.
const ASSISTANT_MARK: &str = "\"assistant\"";

/// How many conversations the answer carries.
///
/// The rows are newest-first and every range the screen can ask for is a
/// recency prefix, so a cap here can only ever hide conversations OLDER than
/// the last one carried — never one a narrower range would have shown. Headroom
/// measured on this machine: 281 conversations across 736 transcript files, so
/// the cap is nearly twenty times the corpus. When it does bite,
/// [`TokenScan::sessions_seen`] says by how much and the screen says so.
pub(crate) const SESSION_ROWS_MAX: usize = 5_000;

/// One reading's tokens, in the five buckets the vendor reports.
///
/// `cache_create_1h` is carried from day one though nothing spends it yet:
/// it is present on 70% of the rows here, it is priced differently from the
/// five-minute cache, and adding it later would mean rescanning the whole
/// corpus to fill it in.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub(crate) struct Tokens {
    pub(crate) input: u64,
    pub(crate) output: u64,
    pub(crate) cache_read: u64,
    pub(crate) cache_create: u64,
    pub(crate) cache_create_1h: u64,
}

impl Tokens {
    /// The larger of each field, taken side by side.
    ///
    /// NOT a sum, and that is the whole point: two rows under one key are one
    /// message seen twice, not two messages. A streamed turn can land first
    /// with a partly filled block and again complete, so the fuller reading
    /// wins field by field rather than the later one wholesale — Orca merges
    /// the same way (`store.ts`'s per-field max).
    fn absorb(&mut self, other: Self) {
        self.input = self.input.max(other.input);
        self.output = self.output.max(other.output);
        self.cache_read = self.cache_read.max(other.cache_read);
        self.cache_create = self.cache_create.max(other.cache_create);
        self.cache_create_1h = self.cache_create_1h.max(other.cache_create_1h);
    }

    /// Everything both readings hold, added — for rolling distinct messages up.
    fn add(&mut self, other: Self) {
        self.input += other.input;
        self.output += other.output;
        self.cache_read += other.cache_read;
        self.cache_create += other.cache_create;
        self.cache_create_1h += other.cache_create_1h;
    }
}

/// The few hundred distinct words a corpus of tens of thousands of messages
/// keeps saying over and over — model ids, session uuids, working directories,
/// branch names.
///
/// The fold below holds one [`Reading`] per DISTINCT MESSAGE for the whole
/// pass (55,806 of them on this machine), and every one of them names four
/// strings drawn from a set of a few hundred. Storing the text in each would
/// put several megabytes of the same handful of words in memory to count them;
/// an index is four bytes and compares faster than the string it stands for.
#[derive(Default)]
struct Words {
    at: HashMap<String, u32>,
    said: Vec<String>,
}

impl Words {
    /// The index of `text`, adding it if this is the first sighting.
    fn take(&mut self, text: &str) -> u32 {
        if let Some(held) = self.at.get(text) {
            return *held;
        }
        let at = u32::try_from(self.said.len()).unwrap_or(u32::MAX);
        self.said.push(text.to_string());
        self.at.insert(text.to_string(), at);
        at
    }

    /// The text an index stands for.
    fn read(&self, at: u32) -> &str {
        self.said.get(at as usize).map_or("", String::as_str)
    }
}

/// One message as read off disk, before the rows are rolled up.
///
/// Every field but the tokens is an index into [`Words`] — see its note for
/// why a whole-corpus map does not hold the strings themselves.
struct Reading {
    model: u32,
    tokens: Tokens,
    /// When it was sent, when the record said. `None` for a record with no
    /// readable stamp — those land in [`TokenScan::undated`] rather than on a
    /// guessed day.
    at: Option<i64>,
    /// The conversation this message belonged to (`sessionId`), or `None` on a
    /// record that named none. Sessions are what the screen counts when it
    /// says how many conversations a week held.
    session: Option<u32>,
    /// The directory the agent was working in (`cwd`). What the place a turn
    /// is filed under is decided from — see [`crate::usage_places`].
    cwd: Option<u32>,
    /// The branch that directory was on (`gitBranch`), for the sessions table.
    branch: Option<u32>,
}

/// One day's share, for one model, in one place.
///
/// Three-part key, which is Orca's own (`usage-aggregation.ts:158` joins
/// `day::model::projectKey`) and is the whole reason the screen can answer for
/// any range, any scope, any model and any project WITHOUT walking the disk
/// again: every figure the screen shows is a sum over a subset of these rows.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct DayRow {
    /// `YYYY-MM-DD`, in the CALLER's day — the offset comes in with the ask.
    pub(crate) date: String,
    pub(crate) model: String,
    /// The place key ([`crate::usage_places::Place::key`]).
    pub(crate) project: String,
    /// What to call that place, or `None` when only the window has a word for
    /// it. A sentence in one language must not reach a window that has four.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) label: Option<String>,
    /// Whether the place is a workspace this window manages — what the screen's
    /// scope switch selects on (Orca's `entry.worktreeId !== null`,
    /// `claude-usage-scope-filters.ts:15`).
    pub(crate) ours: bool,
    pub(crate) messages: u64,
    /// Messages that read NOTHING back from the cache.
    ///
    /// The share of turns that paid full price for a prompt they could have
    /// re-read — Orca counts the same thing per daily row
    /// (`usage-aggregation.ts:162-163`) and shows it as a percentage.
    pub(crate) zero_cache_read: u64,
    #[serde(flatten)]
    pub(crate) tokens: Tokens,
    pub(crate) cost_usd: Option<f64>,
}

/// One conversation, as the transcripts remember it.
///
/// Two totals rather than a list of places. A session can move between
/// directories mid-conversation, and the screen asks exactly one question about
/// that — "was any of this in a workspace I manage?" — so two fixed columns
/// answer it exactly while a nested list would grow the wire with
/// sessions × places for an answer nobody reads that finely. Orca keeps the
/// list (`usage-aggregation.ts:136-156`) because its renderer re-derives the
/// filter from it; ours arrives already filtered.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct SessionRow {
    pub(crate) session: String,
    /// First and last message, in epoch milliseconds — the two ends the screen
    /// subtracts to say how long a conversation ran.
    pub(crate) first_at: i64,
    pub(crate) last_at: i64,
    /// The CALLER's day of `last_at`. The range filter compares days rather
    /// than instants so the sessions table and the chart cannot disagree about
    /// which side of midnight a session fell on (Orca's own note,
    /// `claude-usage-scope-filters.ts:29-31`).
    pub(crate) last_date: String,
    /// The model of the LAST message that named one — a conversation that
    /// switched models is remembered by the one it ended on.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) model: Option<String>,
    /// The branch the last message's directory was on.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) branch: Option<String>,
    /// The heaviest place this conversation touched, and how many it touched.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) label: Option<String>,
    pub(crate) places: u32,
    /// The same, counted only over the workspaces this window manages.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) ours_label: Option<String>,
    pub(crate) ours_places: u32,
    /// Whether ANY of it happened in a workspace this window manages.
    pub(crate) ours: bool,
    pub(crate) messages: u64,
    pub(crate) ours_messages: u64,
    pub(crate) tokens: Tokens,
    pub(crate) ours_tokens: Tokens,
    pub(crate) cost_usd: Option<f64>,
    pub(crate) ours_cost_usd: Option<f64>,
}

/// One model's share of the corpus.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct ModelRow {
    pub(crate) model: String,
    /// Distinct messages, after the duplicates are folded.
    pub(crate) messages: u64,
    #[serde(flatten)]
    pub(crate) tokens: Tokens,
    /// What it cost at API rates, or `None` for a model with no rate on file.
    ///
    /// An estimate and labelled as one wherever it is shown: a plan
    /// subscription is not billed per token, so this is what the same work
    /// would have cost through the API — the same claim Orca makes of its own
    /// figure (`ClaudeUsagePane.tsx:224`).
    pub(crate) cost_usd: Option<f64>,
}

/// What one pass over the transcripts found.
///
/// Deliberately unfiltered: the screen slices these rows for whichever range
/// and scope is showing, which is why the day key has three parts and why one
/// walk answers every question the filters can ask. If a future corpus makes
/// `days` too large for one payload, the fix is to pass the range down and cut
/// the rows HERE against `civil::iso_date_of` — never to cap `days`, because a
/// capped `days` makes every headline on the screen a smaller number wearing
/// the same confidence.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct TokenScan {
    /// Per day, model and place — the primitive every figure on the screen is
    /// summed from. Newest first.
    pub(crate) days: Vec<DayRow>,
    /// Per model, heaviest first — the order a reader wants and a stable one
    /// for a test, since ties fall back to the name.
    pub(crate) models: Vec<ModelRow>,
    /// Every conversation, most recently active first — up to
    /// [`SESSION_ROWS_MAX`].
    pub(crate) sessions: Vec<SessionRow>,
    /// How many distinct conversations the walk actually found, before the cap.
    /// The gap between this and `sessions.len()` is what the screen says out
    /// loud rather than quietly showing a shorter list.
    pub(crate) sessions_seen: u64,
    pub(crate) total: Tokens,
    /// Files opened.
    pub(crate) files: u64,
    /// Assistant records carrying a usage block, before folding.
    pub(crate) records: u64,
    /// Distinct messages after folding. The gap between this and `records` is
    /// the duplication the whole key exists for.
    pub(crate) messages: u64,
    /// Messages whose record carried no readable timestamp. They are counted
    /// here and left OUT of the rows: a message on a guessed day would move a
    /// number somebody reads, and there is nothing to guess from.
    pub(crate) undated: u64,
    /// Every priced row added up, in dollars.
    pub(crate) cost_usd: f64,
    /// The models that had no rate, named rather than folded in as free — a
    /// total that quietly swallowed an unknown model would be a smaller number
    /// wearing the same confidence.
    pub(crate) unpriced: Vec<String>,
}

/// The transcript root under `home`.
pub(crate) fn projects_root(home: &Path) -> PathBuf {
    PROJECTS_DIR
        .iter()
        .fold(home.to_path_buf(), |at, part| at.join(part))
}

/// The key that says two records are the same message.
///
/// The vendor's own id first, paired with the request that produced it: a
/// resumed session replays `message.id`, and the request tells a replay from
/// a genuine retry. Then the id alone, then the record's own uuid — each step
/// down is a record the step above could not name, and the uuid at least keeps
/// distinct rows distinct.
fn message_key(row: &serde_json::Value, message: &serde_json::Value) -> Option<String> {
    fn said(value: Option<&serde_json::Value>) -> Option<&str> {
        value
            .and_then(serde_json::Value::as_str)
            .filter(|held| !held.is_empty())
    }
    match (said(message.get("id")), said(row.get("requestId"))) {
        (Some(id), Some(request)) => Some(format!("{id}:{request}")),
        (Some(id), None) => Some(format!("msg:{id}")),
        (None, _) => said(row.get("uuid")).map(|uuid| format!("uuid:{uuid}")),
    }
}

/// The five buckets out of one `usage` block.
fn tokens_in(usage: &serde_json::Value) -> Tokens {
    let count = |name: &str| {
        usage
            .get(name)
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0)
    };
    Tokens {
        input: count("input_tokens"),
        output: count("output_tokens"),
        cache_read: count("cache_read_input_tokens"),
        cache_create: count("cache_creation_input_tokens"),
        cache_create_1h: usage
            .get("cache_creation")
            .and_then(|held| held.get("ephemeral_1h_input_tokens"))
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0),
    }
}

/// Read one transcript into `held`, keyed by message.
///
/// Streaming, with one buffer reused across the whole file: a transcript is
/// hundreds of megabytes in aggregate and reading it into a `String` first
/// would put the corpus in memory to count it.
fn absorb_file(
    path: &Path,
    held: &mut HashMap<String, Reading>,
    records: &mut u64,
    words: &mut Words,
) {
    token_scan::for_each_line(path, |text| {
        if !text.contains(ASSISTANT_MARK) {
            return;
        }
        let Ok(row) = serde_json::from_str::<serde_json::Value>(text) else {
            return;
        };
        if row.get("type").and_then(serde_json::Value::as_str) != Some("assistant") {
            return;
        }
        let Some(message) = row.get("message") else {
            return;
        };
        let Some(usage) = message.get("usage").filter(|held| held.is_object()) else {
            return;
        };
        let Some(key) = message_key(&row, message) else {
            return;
        };
        *records += 1;
        let model = words.take(
            message
                .get("model")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unknown"),
        );
        // The three facts about WHERE and WHEN, taken from the record rather
        // than from the file's own name: a resumed session copies records into
        // a new file, so the filename says where the bytes landed and the
        // record says whose conversation they belong to.
        let said = |name: &str| {
            row.get(name)
                .and_then(serde_json::Value::as_str)
                .filter(|held| !held.is_empty())
        };
        let session = said("sessionId").map(|held| words.take(held));
        let cwd = said("cwd").map(|held| words.take(held));
        let branch = said("gitBranch").map(|held| words.take(held));
        let tokens = tokens_in(usage);
        // The stamp of the FIRST sighting wins. A resumed session replays a
        // message with the moment it was replayed, and a message belongs to
        // the day it was sent on.
        let at = row
            .get("timestamp")
            .and_then(serde_json::Value::as_str)
            .and_then(zerocode_core::civil::epoch_ms_of_iso);
        held.entry(key)
            .and_modify(|standing| standing.tokens.absorb(tokens))
            .or_insert(Reading {
                model,
                tokens,
                at,
                session,
                cwd,
                branch,
            });
    });
}

/// What one place contributed to one conversation.
///
/// Only the weight is kept, not the tokens: the row names its heaviest place
/// and counts the rest, and the tokens are already summed on the two totals.
#[derive(Default)]
struct PlaceWeight {
    weight: u64,
    ours: bool,
}

/// One conversation, while it is still being added up.
struct SessionBuild {
    first_at: i64,
    last_at: i64,
    /// The model and branch of the message with the largest `at` seen so far —
    /// Orca keeps the last one the same way (`usage-aggregation.ts:124-129`).
    model: u32,
    branch: Option<u32>,
    messages: u64,
    ours_messages: u64,
    tokens: Tokens,
    ours_tokens: Tokens,
    spent: f64,
    priced: bool,
    ours_spent: f64,
    ours_priced: bool,
    places: HashMap<u32, PlaceWeight>,
}

/// One day's tally, while the rows are still keyed by index.
#[derive(Default)]
struct DayTally {
    messages: u64,
    zero_cache_read: u64,
    tokens: Tokens,
}

/// One pass over every transcript under `root`, rolled up in the caller's day.
///
/// `offset_minutes` is the window's own offset east of UTC. A day is a human
/// unit and only the window knows which midnight the person means — passing it
/// in is why nothing back here needs a timezone database.
///
/// `places` decides which workspace a turn's directory belongs to, and is
/// handed in rather than built here because the answer comes from the sidebar's
/// own catalogue and this module has no business opening repositories.
pub(crate) fn scan(root: &Path, offset_minutes: i32, places: &mut Places) -> TokenScan {
    let files = token_scan::files_under(root, ".jsonl");
    // Keyed by message for the whole pass, not per file: the same message
    // appears in more than one transcript when a session is resumed into a new
    // file, and a per-file fold would count it once per file.
    let mut held: HashMap<String, Reading> = HashMap::new();
    let mut words = Words::default();
    let mut records = 0;
    for path in &files {
        absorb_file(path, &mut held, &mut records, &mut words);
    }

    // By day, model AND place, because that triple is what every figure the
    // screen shows is a sum over — see [`DayRow`]. The price of a token
    // depends on the first two of them: a launch rate has an end date, so a
    // model's total is the sum of its days rather than its days being a slice
    // of its total.
    //
    // Keyed by INDEX and not by text. This loop runs once per distinct message
    // — 56,669 times on this machine — and a `(String, String, String)` key
    // would mint three strings on every one of them to name a row that already
    // exists.
    let mut by_day: HashMap<(u32, u32, u32), DayTally> = HashMap::new();
    let mut by_session: HashMap<u32, SessionBuild> = HashMap::new();
    let mut undated = 0;
    for reading in held.into_values() {
        let Some(at) = reading.at else {
            undated += 1;
            continue;
        };
        let day = words.take(&civil::iso_date_of(at, offset_minutes));
        let place = places.of(reading.cwd.map(|held| words.read(held)));
        let ours = places.at(place).ours;
        let row = by_day.entry((day, reading.model, place)).or_default();
        row.messages += 1;
        if reading.tokens.cache_read == 0 {
            row.zero_cache_read += 1;
        }
        row.tokens.add(reading.tokens);

        // A message with no conversation on its record cannot be filed under
        // one. It is still counted everywhere else — the day rows above hold
        // it — so the sessions table is the only place it is missing from.
        let Some(session) = reading.session else {
            continue;
        };
        let spent = cost_usd(
            words.read(reading.model),
            reading.tokens,
            day_of_iso_date(words.read(day)),
        );
        let standing = by_session.entry(session).or_insert_with(|| SessionBuild {
            first_at: at,
            last_at: at,
            model: reading.model,
            branch: reading.branch,
            messages: 0,
            ours_messages: 0,
            tokens: Tokens::default(),
            ours_tokens: Tokens::default(),
            spent: 0.0,
            priced: false,
            ours_spent: 0.0,
            ours_priced: false,
            places: HashMap::new(),
        });
        standing.first_at = standing.first_at.min(at);
        if at >= standing.last_at {
            standing.last_at = at;
            standing.model = reading.model;
            standing.branch = reading.branch;
        }
        standing.messages += 1;
        standing.tokens.add(reading.tokens);
        if let Some(dollars) = spent {
            standing.spent += dollars;
            standing.priced = true;
        }
        if ours {
            standing.ours_messages += 1;
            standing.ours_tokens.add(reading.tokens);
            if let Some(dollars) = spent {
                standing.ours_spent += dollars;
                standing.ours_priced = true;
            }
        }
        let visited = standing.places.entry(place).or_default();
        visited.weight += reading.tokens.input + reading.tokens.output;
        visited.ours = ours;
    }

    let mut days: Vec<DayRow> = by_day
        .into_iter()
        .map(|((day, model, place), tally)| {
            let one = places.at(place);
            DayRow {
                date: words.read(day).to_string(),
                model: words.read(model).to_string(),
                project: one.key.clone(),
                label: one.label.clone(),
                ours: one.ours,
                messages: tally.messages,
                zero_cache_read: tally.zero_cache_read,
                tokens: tally.tokens,
                cost_usd: None,
            }
        })
        .collect();
    // Newest first; the model and then the place break a tie so two identical
    // scans read the same way.
    days.sort_by(|left, right| {
        right
            .date
            .cmp(&left.date)
            .then_with(|| left.model.cmp(&right.model))
            .then_with(|| left.project.cmp(&right.project))
    });

    let sessions_seen = by_session.len() as u64;
    let mut sessions: Vec<SessionRow> = by_session
        .into_iter()
        .map(|(session, built)| {
            // The heaviest place names the row, and the heaviest of OURS names
            // the row when the screen is showing only ours. Ties break on the
            // place key so two runs read alike.
            let heaviest = |mine: bool| {
                built
                    .places
                    .iter()
                    .filter(|(_, weight)| !mine || weight.ours)
                    .max_by(|left, right| {
                        left.1
                            .weight
                            .cmp(&right.1.weight)
                            .then_with(|| places.at(*right.0).key.cmp(&places.at(*left.0).key))
                    })
                    .and_then(|(at, _)| places.at(*at).label.clone())
            };
            let ours_places = built.places.values().filter(|weight| weight.ours).count();
            SessionRow {
                session: words.read(session).to_string(),
                first_at: built.first_at,
                last_at: built.last_at,
                last_date: civil::iso_date_of(built.last_at, offset_minutes),
                model: Some(words.read(built.model).to_string()),
                branch: built.branch.map(|held| words.read(held).to_string()),
                label: heaviest(false),
                places: u32::try_from(built.places.len()).unwrap_or(u32::MAX),
                ours_label: heaviest(true),
                ours_places: u32::try_from(ours_places).unwrap_or(u32::MAX),
                ours: built.ours_messages > 0,
                messages: built.messages,
                ours_messages: built.ours_messages,
                tokens: built.tokens,
                ours_tokens: built.ours_tokens,
                cost_usd: built.priced.then_some(built.spent),
                ours_cost_usd: built.ours_priced.then_some(built.ours_spent),
            }
        })
        .collect();
    sessions.sort_by(|left, right| {
        right
            .last_at
            .cmp(&left.last_at)
            .then_with(|| left.session.cmp(&right.session))
    });
    // Newest first, so the cap only ever drops conversations older than the
    // last one carried — never one a narrower range would have shown.
    sessions.truncate(SESSION_ROWS_MAX);

    let mut by_model: HashMap<String, ModelRow> = HashMap::new();
    let mut total = Tokens::default();
    let mut spent = 0.0;
    let mut unpriced: Vec<String> = Vec::new();
    for row in &mut days {
        // Priced on ITS OWN day. The day number is recovered from the date the
        // row was filed under, so the rate that applies is the one that was in
        // force when the message was sent.
        let day = day_of_iso_date(&row.date);
        row.cost_usd = cost_usd(&row.model, row.tokens, day);
        total.add(row.tokens);
        let held = by_model
            .entry(row.model.clone())
            .or_insert_with(|| ModelRow {
                model: row.model.clone(),
                messages: 0,
                tokens: Tokens::default(),
                cost_usd: None,
            });
        held.messages += row.messages;
        held.tokens.add(row.tokens);
        match row.cost_usd {
            Some(dollars) => {
                spent += dollars;
                held.cost_usd = Some(held.cost_usd.unwrap_or(0.0) + dollars);
            }
            None => {
                if !unpriced.contains(&row.model) {
                    unpriced.push(row.model.clone());
                }
            }
        }
    }

    let mut models: Vec<ModelRow> = by_model.into_values().collect();
    models.sort_by(|left, right| {
        let weight = |row: &ModelRow| {
            row.tokens.input + row.tokens.output + row.tokens.cache_read + row.tokens.cache_create
        };
        weight(right)
            .cmp(&weight(left))
            .then_with(|| left.model.cmp(&right.model))
    });
    unpriced.sort();
    let messages = models.iter().map(|row| row.messages).sum();
    TokenScan {
        days,
        models,
        sessions,
        sessions_seen,
        total,
        files: files.len() as u64,
        records,
        messages,
        undated,
        cost_usd: spent,
        unpriced,
    }
}

/// The day number a launch window's last day sits on.
///
/// The price table is a leaf crate both this tree and zo compile, so it keeps
/// the date it was written with rather than a day number — turning one into
/// the other is [`civil`]'s job, and it happens here, at the one comparison
/// that needs it.
fn day_of_civil_date(date: model_prices::CivilDate) -> i64 {
    civil::days_from_civil(date.year, date.month, date.day)
}

/// The day number a `YYYY-MM-DD` string names.
///
/// The rows are filed under the written date, so this reads it back rather
/// than carrying a second field that could disagree with the first.
fn day_of_iso_date(date: &str) -> i64 {
    let mut parts = date.split('-');
    let year = parts.next().and_then(|held| held.parse().ok()).unwrap_or(0);
    let month = parts.next().and_then(|held| held.parse().ok()).unwrap_or(1);
    let day = parts.next().and_then(|held| held.parse().ok()).unwrap_or(1);
    civil::days_from_civil(year, month, day)
}

/* ---- what it cost ------------------------------------------------------- */

/// What one model's tokens cost, in dollars — or `None` for a model this table
/// does not know.
///
/// `None` rather than zero on purpose: a model nobody has a rate for costs an
/// unknown amount, and folding it in as free would quietly shrink the total.
/// The caller says so out loud instead.
pub(crate) fn cost_usd(model: &str, tokens: Tokens, day: i64) -> Option<f64> {
    let rate = model_prices::anthropic_rate(model)?;
    // The price in force on the day it was sent, not the price today.
    let (input, output) = match rate.intro {
        Some(intro) if day <= day_of_civil_date(intro.through) => (intro.input, intro.output),
        _ => (rate.input, rate.output),
    };
    // The five-minute share is what is left after the hour-long share — the
    // vendor reports the total and the hour-long part, not the five-minute one.
    // An hour-long write costs twice, not 1.25 times: Orca carries a single
    // cacheWrite and bills every cache-creation token at the five-minute rate,
    // 37.5% under on the 70% of this machine's corpus that is hour-TTL, which
    // is why the scanner carries [`Tokens::cache_create_1h`] separately.
    let write_1h = tokens.cache_create_1h.min(tokens.cache_create);
    let write_5m = tokens.cache_create - write_1h;
    let (write_5m_rate, write_1h_rate) = model_prices::anthropic_cache_write_rates();
    let per = |count: u64, dollars: f64| (count as f64) * dollars;
    Some(
        (per(tokens.input, input)
            + per(tokens.output, output)
            + per(tokens.cache_read, input * rate.cache_read)
            + per(write_5m, input * write_5m_rate)
            + per(write_1h, input * write_1h_rate))
            / 1_000_000.0,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A lookup that knows no workspaces — the shape most of these fixtures
    /// want, where every directory is somewhere this window never heard of.
    fn nowhere() -> Places {
        Places::new([])
    }

    /// A day well after every launch window, so a fixture that does not care
    /// about dates is not quietly priced at one.
    const PLAIN_DAY: &str = "2026-12-01T09:00:00.000Z";

    fn row(uuid: &str, id: Option<&str>, request: Option<&str>, model: &str, out: u64) -> String {
        dated_row(uuid, id, request, model, out, PLAIN_DAY)
    }

    fn dated_row(
        uuid: &str,
        id: Option<&str>,
        request: Option<&str>,
        model: &str,
        out: u64,
        at: &str,
    ) -> String {
        let mut message = serde_json::json!({
            "role": "assistant",
            "model": model,
            "usage": {
                "input_tokens": 1,
                "output_tokens": out,
                "cache_read_input_tokens": 10,
                "cache_creation_input_tokens": 20,
                "cache_creation": { "ephemeral_1h_input_tokens": 5 },
            },
        });
        if let Some(id) = id {
            message["id"] = serde_json::Value::String(id.to_string());
        }
        let mut held = serde_json::json!({
            "type": "assistant",
            "uuid": uuid,
            "timestamp": at,
            "message": message,
        });
        if let Some(request) = request {
            held["requestId"] = serde_json::Value::String(request.to_string());
        }
        held.to_string()
    }

    /// One turn, with every fact the rollup files it under.
    ///
    /// A struct rather than nine positional arguments: the fixtures below care
    /// about one or two fields each, and a call site reading
    /// `turn(Turn { cwd: Some("/work/repo"), ..plain("u1") })` says which.
    struct Turn<'a> {
        uuid: &'a str,
        model: &'a str,
        out: u64,
        cache_read: u64,
        at: &'a str,
        session: Option<&'a str>,
        cwd: Option<&'a str>,
        branch: Option<&'a str>,
    }

    /// A turn that names nothing but itself.
    fn plain(uuid: &str) -> Turn<'_> {
        Turn {
            uuid,
            model: "claude-opus-5",
            out: 10,
            cache_read: 10,
            at: PLAIN_DAY,
            session: Some("s1"),
            cwd: None,
            branch: None,
        }
    }

    fn turn(one: Turn<'_>) -> String {
        let mut held = serde_json::json!({
            "type": "assistant",
            "uuid": one.uuid,
            "timestamp": one.at,
            "message": {
                "role": "assistant",
                "id": one.uuid,
                "model": one.model,
                "usage": {
                    "input_tokens": 1,
                    "output_tokens": one.out,
                    "cache_read_input_tokens": one.cache_read,
                    "cache_creation_input_tokens": 0,
                },
            },
        });
        for (name, value) in [
            ("sessionId", one.session),
            ("cwd", one.cwd),
            ("gitBranch", one.branch),
        ] {
            if let Some(value) = value {
                held[name] = serde_json::Value::String(value.to_string());
            }
        }
        held.to_string()
    }

    /// A lookup that knows one repository and one workspace inside it.
    fn known() -> Places {
        Places::new([
            (PathBuf::from("/work/repo"), "main".to_string()),
            (
                PathBuf::from("/work/repo/.zo/drain"),
                "wt/drain".to_string(),
            ),
        ])
    }

    fn corpus(lines: &[&str]) -> tempfile::TempDir {
        let home = tempfile::tempdir().expect("tempdir");
        let deep = projects_root(home.path()).join("a-project").join("nested");
        std::fs::create_dir_all(&deep).expect("mkdir");
        std::fs::write(deep.join("session.jsonl"), lines.join("\n")).expect("write");
        home
    }

    /// A turn is filed under the workspace it happened in, and a turn from a
    /// directory this window never heard of says so rather than being dropped.
    ///
    /// This is the whole of the screen's scope switch: 이 앱 sums the rows
    /// whose `ours` is true, 전체 sums them all. Orca decides it the same way
    /// and calls the answer `worktreeId` (`worktree-attribution.ts:126-138`).
    #[test]
    fn a_turn_is_filed_under_the_workspace_it_happened_in() {
        let home = corpus(&[
            &turn(Turn {
                cwd: Some("/work/repo/.zo/drain/src"),
                session: Some("inside"),
                ..plain("u1")
            }),
            &turn(Turn {
                cwd: Some("/work/repo/src"),
                session: Some("trunk"),
                ..plain("u2")
            }),
            &turn(Turn {
                cwd: Some("/Users/someone/2026/elsewhere"),
                session: Some("outside"),
                ..plain("u3")
            }),
        ]);
        let held = scan(&projects_root(home.path()), 0, &mut known());
        let filed = |key: &str| {
            held.days
                .iter()
                .find(|row| row.project == key)
                .unwrap_or_else(|| panic!("no row filed under {key}"))
                .clone()
        };
        // The nested workspace wins over the repository that contains it —
        // the longest known path, not the first one listed.
        let inner = filed("worktree:/work/repo/.zo/drain");
        assert!(inner.ours);
        assert_eq!(inner.label.as_deref(), Some("wt/drain"));
        let trunk = filed("worktree:/work/repo");
        assert!(trunk.ours);
        assert_eq!(trunk.label.as_deref(), Some("main"));
        let away = filed("cwd:/Users/someone/2026/elsewhere");
        assert!(!away.ours, "a stranger's directory was counted as ours");
        assert_eq!(
            away.label.as_deref(),
            Some("2026/elsewhere"),
            "an unclaimed directory lost the two segments that tell it apart"
        );
        // Every turn is still counted once, wherever it was filed.
        assert_eq!(held.messages, 3);
    }

    /// A conversation is one row however many files it was written across, and
    /// the model and branch it is remembered by are the ones it ENDED on.
    #[test]
    fn a_conversation_is_one_row_however_many_files_it_wandered_through() {
        let home = corpus(&[
            &turn(Turn {
                at: "2026-08-17T01:00:00.000Z",
                model: "claude-sonnet-5",
                branch: Some("wt/first"),
                cwd: Some("/work/repo"),
                out: 5,
                ..plain("u1")
            }),
            &turn(Turn {
                at: "2026-08-19T01:00:00.000Z",
                model: "claude-opus-5",
                branch: Some("wt/last"),
                cwd: Some("/work/repo"),
                out: 7,
                ..plain("u2")
            }),
        ]);
        let held = scan(&projects_root(home.path()), 0, &mut known());
        assert_eq!(held.sessions.len(), 1, "one conversation became two rows");
        let one = &held.sessions[0];
        assert_eq!(one.session, "s1");
        assert_eq!(one.messages, 2);
        assert_eq!(one.tokens.output, 12);
        assert_eq!(one.last_date, "2026-08-19");
        assert!(one.first_at < one.last_at);
        assert_eq!(
            one.model.as_deref(),
            Some("claude-opus-5"),
            "a conversation is remembered by the model it started on, not the one it ended on"
        );
        assert_eq!(one.branch.as_deref(), Some("wt/last"));
        // Its cost is the sum of its own turns, priced on their own days —
        // not the whole session priced on the last one.
        assert!(one.cost_usd.is_some_and(|dollars| dollars > 0.0));
    }

    /// The turns that read nothing back from the cache are counted apart.
    ///
    /// The screen shows the share of them; a day row that only knew its total
    /// could not answer for a week without the scan running again.
    #[test]
    fn the_turns_that_read_nothing_back_are_counted_apart() {
        let home = corpus(&[
            &turn(Turn {
                cache_read: 0,
                ..plain("u1")
            }),
            &turn(Turn {
                cache_read: 0,
                ..plain("u2")
            }),
            &turn(Turn {
                cache_read: 500,
                ..plain("u3")
            }),
        ]);
        let held = scan(&projects_root(home.path()), 0, &mut nowhere());
        assert_eq!(held.days.len(), 1);
        assert_eq!(held.days[0].messages, 3);
        assert_eq!(held.days[0].zero_cache_read, 2);
    }

    /// A conversation that moved between places remembers both, heaviest
    /// first — the head is what names the row.
    #[test]
    fn a_conversation_that_moved_remembers_where_it_spent_the_most() {
        let home = corpus(&[
            &turn(Turn {
                cwd: Some("/work/repo"),
                out: 5,
                ..plain("u1")
            }),
            &turn(Turn {
                cwd: Some("/work/repo/.zo/drain"),
                out: 500,
                ..plain("u2")
            }),
        ]);
        let held = scan(&projects_root(home.path()), 0, &mut known());
        let one = &held.sessions[0];
        assert_eq!(one.places, 2);
        assert_eq!(one.ours_places, 2);
        // The heavier of the two names the row.
        assert_eq!(one.label.as_deref(), Some("wt/drain"));
        assert_eq!(one.ours_label.as_deref(), Some("wt/drain"));
        assert!(one.ours);
    }

    /// A record with no conversation on it still counts where it can.
    ///
    /// It cannot join a session row — there is nothing to join it to — and
    /// dropping it from the day rows as well would make the totals disagree
    /// with the tables under them.
    #[test]
    fn a_turn_with_no_conversation_still_counts_on_its_day() {
        let home = corpus(&[
            &turn(Turn {
                session: None,
                ..plain("u1")
            }),
            &turn(Turn {
                session: Some("s2"),
                ..plain("u2")
            }),
        ]);
        let held = scan(&projects_root(home.path()), 0, &mut nowhere());
        assert_eq!(held.messages, 2);
        assert_eq!(held.days[0].messages, 2);
        assert_eq!(held.sessions.len(), 1);
        assert_eq!(held.sessions[0].session, "s2");
    }

    /// A conversation that straddles the scope carries both totals.
    ///
    /// This is what makes the screen's 범위 switch honest on the sessions
    /// table: the same conversation appears under both scopes, with the
    /// numbers each scope is entitled to — not the whole session's figures
    /// wearing the narrower label.
    #[test]
    fn a_conversation_that_straddles_the_scope_carries_both_totals() {
        let home = corpus(&[
            &turn(Turn {
                cwd: Some("/work/repo/src"),
                out: 100,
                ..plain("u1")
            }),
            &turn(Turn {
                cwd: Some("/somewhere/else"),
                out: 7,
                ..plain("u2")
            }),
        ]);
        let held = scan(&projects_root(home.path()), 0, &mut known());
        let one = &held.sessions[0];
        assert!(one.ours, "a session with a turn of ours reads as not ours");
        assert_eq!(one.messages, 2);
        assert_eq!(one.ours_messages, 1);
        assert_eq!(one.tokens.output, 107);
        assert_eq!(one.ours_tokens.output, 100);
        assert_eq!(one.places, 2);
        assert_eq!(one.ours_places, 1);
        // Each label names the heaviest place its own scope can see.
        assert_eq!(one.label.as_deref(), Some("main"));
        assert_eq!(one.ours_label.as_deref(), Some("main"));
        let both = one.cost_usd.expect("priced");
        let mine = one.ours_cost_usd.expect("priced");
        assert!(mine < both, "the narrower scope did not cost less");
    }

    /// The conversation list is capped, and says how many it left behind.
    ///
    /// The rows are newest-first, so the cap can only drop conversations older
    /// than the last one carried — but a shorter list with no word about it is
    /// a list that looks complete.
    #[test]
    fn the_conversation_list_is_capped_and_says_what_it_left() {
        let mut lines: Vec<String> = Vec::new();
        let over = SESSION_ROWS_MAX + 3;
        for at in 0..over {
            // One conversation each, an hour apart, oldest first.
            let hour = format!("2026-01-01T{:02}:00:00.000Z", at % 24);
            let uuid = format!("u{at}");
            let session = format!("s{at:06}");
            lines.push(turn(Turn {
                uuid: &uuid,
                session: Some(&session),
                at: &hour,
                ..plain("ignored")
            }));
        }
        let held = corpus(&lines.iter().map(String::as_str).collect::<Vec<&str>>());
        let scanned = scan(&projects_root(held.path()), 0, &mut nowhere());
        assert_eq!(scanned.sessions.len(), SESSION_ROWS_MAX);
        assert_eq!(scanned.sessions_seen, over as u64);
        // The cap took the OLDEST, so the newest conversation survived.
        assert!(scanned.sessions[0].last_at >= scanned.sessions[1].last_at);
        // And it moved no headline: every message is still counted.
        assert_eq!(scanned.messages, over as u64);
    }

    /// The wire says what the window reads.
    ///
    /// The screen sums these rows itself for whichever range and scope is
    /// showing, so a field renamed back here is a figure that silently becomes
    /// zero out there. Named rather than shape-checked, because that is the
    /// failure this pins.
    #[test]
    fn the_wire_names_every_field_the_screen_sums() {
        let home = corpus(&[&turn(Turn {
            cwd: Some("/work/repo"),
            branch: Some("wt/drain"),
            ..plain("u1")
        })]);
        let held = scan(&projects_root(home.path()), 0, &mut known());
        let wire = serde_json::to_value(&held).expect("serialise");
        for name in [
            "days", "models", "sessions", "total", "files", "records", "messages", "undated",
            "cost_usd", "unpriced",
        ] {
            assert!(wire.get(name).is_some(), "the scan stopped saying {name}");
        }
        let day = &wire["days"][0];
        for name in [
            "date",
            "model",
            "project",
            "label",
            "ours",
            "messages",
            "zero_cache_read",
            "input",
            "output",
            "cache_read",
            "cache_create",
            "cache_create_1h",
            "cost_usd",
        ] {
            assert!(day.get(name).is_some(), "a day row stopped saying {name}");
        }
        let session = &wire["sessions"][0];
        for name in [
            "session",
            "first_at",
            "last_at",
            "last_date",
            "model",
            "branch",
            "label",
            "places",
            "ours_label",
            "ours_places",
            "ours",
            "messages",
            "ours_messages",
            "tokens",
            "ours_tokens",
            "cost_usd",
            "ours_cost_usd",
        ] {
            assert!(
                session.get(name).is_some(),
                "a session row stopped saying {name}"
            );
        }
        // The session's tokens are an object, NOT flattened — the window picks
        // a whole bucket by scope and would have to name ten fields otherwise.
        assert!(session["tokens"].get("input").is_some());
        assert!(session["ours_tokens"].get("cache_read").is_some());
    }

    /// The same message twice is one message, and the fuller reading wins.
    ///
    /// This is the whole reason the scan is not a `wc`: on the real corpus
    /// 45.3% of the naive total is a record counted again, and half the output
    /// tokens move when they are folded.
    #[test]
    fn one_message_seen_twice_is_counted_once_at_its_fullest() {
        // Same id AND same request: one message, streamed twice, the second
        // reading fuller.
        let home = corpus(&[
            &row("u1", Some("msg_a"), Some("req_a"), "claude-opus-5", 100),
            &row("u2", Some("msg_a"), Some("req_a"), "claude-opus-5", 250),
        ]);
        let held = scan(&projects_root(home.path()), 0, &mut nowhere());
        assert_eq!(held.records, 2, "both records were read");
        assert_eq!(held.messages, 1, "the duplicate was not folded");
        assert_eq!(held.total.output, 250, "the fuller reading did not win");
        // Per-field, not wholesale: the fields the second reading did not
        // shrink stay where they were.
        assert_eq!(held.total.cache_read, 10);
        assert_eq!(held.total.cache_create_1h, 5);

        // A RETRY of the same message is a different request, so it counts.
        let home = corpus(&[
            &row("u1", Some("msg_a"), Some("req_a"), "claude-opus-5", 100),
            &row("u2", Some("msg_a"), Some("req_b"), "claude-opus-5", 100),
        ]);
        let held = scan(&projects_root(home.path()), 0, &mut nowhere());
        assert_eq!(held.messages, 2, "a retry was folded into its first try");
        assert_eq!(held.total.output, 200);
    }

    /// A record with no id falls back, and never onto its neighbour.
    #[test]
    fn a_record_without_an_id_keeps_its_own_place() {
        let home = corpus(&[
            &row("u1", None, None, "claude-opus-5", 7),
            &row("u2", None, None, "claude-opus-5", 9),
            // An id with no request still folds against itself.
            &row("u3", Some("msg_b"), None, "claude-opus-5", 11),
            &row("u4", Some("msg_b"), None, "claude-opus-5", 11),
        ]);
        let held = scan(&projects_root(home.path()), 0, &mut nowhere());
        assert_eq!(held.messages, 3);
        assert_eq!(held.total.output, 7 + 9 + 11);
    }

    /// The rows are grouped by model and ordered so two identical scans read
    /// the same way.
    #[test]
    fn the_table_is_grouped_by_model_and_ordered_the_same_every_time() {
        let home = corpus(&[
            &row("u1", Some("m1"), Some("r1"), "claude-sonnet-5", 5),
            &row("u2", Some("m2"), Some("r2"), "claude-opus-5", 900),
            &row("u3", Some("m3"), Some("r3"), "claude-opus-5", 900),
        ]);
        let held = scan(&projects_root(home.path()), 0, &mut nowhere());
        assert_eq!(held.models.len(), 2);
        assert_eq!(
            held.models[0].model, "claude-opus-5",
            "the heaviest is not first"
        );
        assert_eq!(held.models[0].messages, 2);
        assert_eq!(held.models[1].model, "claude-sonnet-5");
        assert_eq!(held.total.output, 1805);
        assert_eq!(
            scan(&projects_root(home.path()), 0, &mut nowhere()),
            held,
            "two scans disagreed"
        );
    }

    /// Everything that is not an assistant reading is skipped, including the
    /// user turn that merely says the word.
    #[test]
    fn only_an_assistant_record_with_a_usage_block_is_counted() {
        let home = corpus(&[
            r#"{"type":"user","message":{"role":"user","content":"is \"assistant\" a word"}}"#,
            r#"{"type":"assistant","uuid":"u9","message":{"role":"assistant","model":"m"}}"#,
            r#"not json at all"#,
            "",
            &row("u1", Some("msg_c"), Some("req_c"), "claude-opus-5", 3),
        ]);
        let held = scan(&projects_root(home.path()), 0, &mut nowhere());
        assert_eq!(held.records, 1);
        assert_eq!(held.messages, 1);
        assert_eq!(held.total.output, 3);
    }

    /// Every rate below is asked for on a day outside any launch window, so a
    /// row with an intro price answers with its standard one.
    fn cost_usd_on(model: &str, tokens: Tokens) -> Option<f64> {
        cost_usd(model, tokens, civil::days_from_civil(2026, 12, 1))
    }

    /// The rates match both references, and the cache rates are the arithmetic
    /// rather than a second table to drift.
    #[test]
    fn a_million_of_each_token_costs_what_both_references_say() {
        let million = Tokens {
            input: 1_000_000,
            output: 0,
            cache_read: 0,
            cache_create: 0,
            cache_create_1h: 0,
        };
        // Base input, straight off both tables.
        assert_eq!(cost_usd_on("claude-opus-5", million), Some(5.0));
        assert_eq!(cost_usd_on("claude-fable-5", million), Some(10.0));
        assert_eq!(cost_usd_on("claude-fable-5-1", million), Some(10.0));
        assert_eq!(cost_usd_on("claude-sonnet-5", million), Some(3.0));
        assert_eq!(cost_usd_on("claude-haiku-4-5", million), Some(1.0));

        let out = Tokens {
            input: 0,
            output: 1_000_000,
            ..million
        };
        assert_eq!(cost_usd_on("claude-opus-5", out), Some(25.0));
        assert_eq!(cost_usd_on("claude-fable-5", out), Some(50.0));

        // A tenth for a read, and Orca's written-out 0.5 for Opus.
        let read = Tokens {
            input: 0,
            cache_read: 1_000_000,
            ..million
        };
        assert_eq!(cost_usd_on("claude-opus-5", read), Some(0.5));
        // Fable 5.1 reads at a fortieth, Fable 5 at a tenth — the one number
        // the two rows disagree on.
        assert_eq!(cost_usd_on("claude-fable-5-1", read), Some(0.25));
        assert_eq!(cost_usd_on("claude-fable-5", read), Some(1.0));
        // 1.25x for a five-minute write — Orca's written-out 6.25.
        let write = Tokens {
            input: 0,
            cache_create: 1_000_000,
            ..million
        };
        assert_eq!(cost_usd_on("claude-opus-5", write), Some(6.25));
    }

    /// An hour-long cache write costs twice, not 1.25 times — the one place
    /// this table is not Orca's, and the place its single `cacheWrite` is 37.5%
    /// under on this machine's own corpus.
    #[test]
    fn an_hour_long_cache_write_is_priced_above_a_five_minute_one() {
        let hour = Tokens {
            input: 0,
            output: 0,
            cache_read: 0,
            cache_create: 1_000_000,
            cache_create_1h: 1_000_000,
        };
        assert_eq!(cost_usd_on("claude-opus-5", hour), Some(10.0));
        // Half and half, out of one reported total.
        let mixed = Tokens {
            cache_create_1h: 400_000,
            ..hour
        };
        let want = (600_000.0 * 5.0 * 1.25 + 400_000.0 * 5.0 * 2.0) / 1_000_000.0;
        assert_eq!(cost_usd_on("claude-opus-5", mixed), Some(want));
        // A vendor that reports more hour-tokens than total cannot make the
        // five-minute share negative.
        let impossible = Tokens {
            cache_create: 1000,
            cache_create_1h: 9999,
            ..hour
        };
        assert_eq!(
            cost_usd_on("claude-opus-5", impossible),
            Some(1000.0 * 5.0 * 2.0 / 1_000_000.0)
        );
    }

    /// The legacy Opus 4 row bills three times the modern one, and an id that
    /// merely looks like it does not fall onto it.
    #[test]
    fn a_legacy_opus_4_is_not_billed_at_todays_opus_rate() {
        let million = Tokens {
            input: 1_000_000,
            output: 0,
            cache_read: 0,
            cache_create: 0,
            cache_create_1h: 0,
        };
        for legacy in [
            "claude-opus-4",
            "claude-opus-4-20250514",
            "claude-opus-4-thinking",
        ] {
            assert_eq!(
                cost_usd_on(legacy, million),
                Some(15.0),
                "{legacy} lost its legacy rate"
            );
        }
        assert_eq!(cost_usd_on("claude-opus-4-1", million), Some(15.0));
        // And the modern ones stay modern, dots and vendor prefixes included.
        for modern in [
            "claude-opus-4-8",
            "claude-opus-4.8",
            "claude-opus-4-8-thinking",
            "anthropic/claude-opus-5",
            "claude-opus-5-20260101",
        ] {
            assert_eq!(
                cost_usd_on(modern, million),
                Some(5.0),
                "{modern} lost its rate"
            );
        }
        // An Opus 4 point release nobody has taught this table about bills at
        // the current rate, not the legacy one — three times too much is a
        // worse answer than a little too little.
        assert_eq!(cost_usd_on("claude-opus-4-9", million), Some(5.0));
    }

    /// A model with no rate costs an unknown amount, and says so.
    #[test]
    fn an_unpriced_model_is_unknown_rather_than_free() {
        let million = Tokens {
            input: 1_000_000,
            output: 0,
            cache_read: 0,
            cache_create: 0,
            cache_create_1h: 0,
        };
        // `<synthetic>` is real: 79 messages of it sit in this machine's corpus.
        for unknown in ["<synthetic>", "unknown", "gpt-4", ""] {
            assert_eq!(
                cost_usd_on(unknown, million),
                None,
                "{unknown} was given a price"
            );
        }
    }

    /// A day is the CALLER's midnight, and a message with no stamp is counted
    /// apart rather than put on a guessed one.
    #[test]
    fn the_rows_fall_on_the_day_the_caller_means() {
        // 2026-08-18T00:30:00Z — the small hours in London, mid-morning in
        // Seoul, and still the night before in New York.
        let home = corpus(&[&dated_row(
            "u1",
            Some("m1"),
            Some("r1"),
            "claude-opus-5",
            10,
            "2026-08-18T00:30:00.000Z",
        )]);
        let root = projects_root(home.path());
        assert_eq!(scan(&root, 0, &mut nowhere()).days[0].date, "2026-08-18");
        assert_eq!(
            scan(&root, 9 * 60, &mut nowhere()).days[0].date,
            "2026-08-18"
        );
        assert_eq!(
            scan(&root, -5 * 60, &mut nowhere()).days[0].date,
            "2026-08-17"
        );

        // Two messages either side of a local midnight land on two days, and
        // the newest is first.
        let home = corpus(&[
            &dated_row(
                "u1",
                Some("m1"),
                Some("r1"),
                "claude-opus-5",
                10,
                "2026-08-17T14:00:00.000Z",
            ),
            &dated_row(
                "u2",
                Some("m2"),
                Some("r2"),
                "claude-opus-5",
                20,
                "2026-08-17T15:30:00.000Z",
            ),
        ]);
        let held = scan(&projects_root(home.path()), 9 * 60, &mut nowhere());
        assert_eq!(held.days.len(), 2, "one local midnight was not crossed");
        assert_eq!(held.days[0].date, "2026-08-18");
        assert_eq!(held.days[1].date, "2026-08-17");
        // The model row is the sum of its days.
        assert_eq!(held.models[0].tokens.output, 30);
        assert_eq!(held.total.output, 30);

        // A record with no stamp is counted and set aside.
        let home = corpus(&[
            r#"{"type":"assistant","uuid":"u9","message":{"role":"assistant","model":"claude-opus-5","usage":{"output_tokens":5}}}"#,
        ]);
        let held = scan(&projects_root(home.path()), 0, &mut nowhere());
        assert_eq!(held.records, 1);
        assert_eq!(held.undated, 1);
        assert!(held.days.is_empty(), "an undated message was put on a day");
        assert_eq!(held.total.output, 0);
    }

    /// A message sent inside a launch window is billed at the launch price,
    /// and one sent after it is not.
    ///
    /// Orca prices the whole window at the standard rate because it carries no
    /// date (`claude-model-pricing.ts:26-27`). A message has a timestamp, so
    /// this asks what it cost on the day it was sent.
    #[test]
    fn a_message_is_billed_at_the_price_of_the_day_it_was_sent() {
        let million = Tokens {
            input: 1_000_000,
            output: 0,
            cache_read: 0,
            cache_create: 0,
            cache_create_1h: 0,
        };
        let day = |year, month, day| civil::days_from_civil(year, month, day);
        // Inside the window: the launch price.
        assert_eq!(
            cost_usd("claude-sonnet-5", million, day(2026, 8, 18)),
            Some(2.0)
        );
        // Its last day is inclusive…
        assert_eq!(
            cost_usd("claude-sonnet-5", million, day(2026, 8, 31)),
            Some(2.0)
        );
        // …and the day after is the standard rate, with nothing to edit.
        assert_eq!(
            cost_usd("claude-sonnet-5", million, day(2026, 9, 1)),
            Some(3.0)
        );
        // Output too, not just input.
        let out = Tokens {
            input: 0,
            output: 1_000_000,
            ..million
        };
        assert_eq!(
            cost_usd("claude-sonnet-5", out, day(2026, 8, 18)),
            Some(10.0)
        );
        assert_eq!(
            cost_usd("claude-sonnet-5", out, day(2026, 9, 1)),
            Some(15.0)
        );
        // A model with no launch price is unmoved by the date.
        for at in [day(2020, 1, 1), day(2026, 8, 18), day(2030, 1, 1)] {
            assert_eq!(cost_usd("claude-opus-5", million, at), Some(5.0));
        }

        // And it reaches the rows: the same tokens on two sides of the cutover
        // cost different amounts.
        let home = corpus(&[
            &dated_row(
                "u1",
                Some("m1"),
                Some("r1"),
                "claude-sonnet-5",
                1_000_000,
                "2026-08-18T00:00:00.000Z",
            ),
            &dated_row(
                "u2",
                Some("m2"),
                Some("r2"),
                "claude-sonnet-5",
                1_000_000,
                "2026-09-01T00:00:00.000Z",
            ),
        ]);
        let held = scan(&projects_root(home.path()), 0, &mut nowhere());
        let priced = |date: &str| {
            held.days
                .iter()
                .find(|row| row.date == date)
                .and_then(|row| row.cost_usd)
                .expect("a day went missing")
        };
        assert!(
            priced("2026-09-01") > priced("2026-08-18"),
            "the launch window did not reach the rows: {:?}",
            held.days
        );
    }

    /// A root that is not there is an empty answer, not a failure — a machine
    /// that has never run the vendor's CLI has nothing to count.
    #[test]
    fn a_missing_root_counts_nothing_and_says_so_quietly() {
        let home = tempfile::tempdir().expect("tempdir");
        let held = scan(&projects_root(home.path()), 0, &mut nowhere());
        assert_eq!(held.files, 0);
        assert_eq!(held.records, 0);
        assert!(held.models.is_empty());
        assert_eq!(held.total, Tokens::default());
    }
}
