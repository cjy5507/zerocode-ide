//! The judge behind [`super::ActionJudge`] that actually asks
//! (docs/design/jev-browser-action-20260917.md §2.1): a stopped walk's
//! question, sent down the window's one System One wire
//! ([`crate::systemone`]) and read back through the question that was asked.
//!
//! The question, the answer space and every validation rule come from
//! `zerocode_core::browser_action`; the socket, the deadline, the failure words
//! and the Jev door every request passes are the wire's. This file holds the
//! browser row's name for the request and nothing else.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};
use zerocode_core::branching::BranchAsk;
use zerocode_core::jev::door::{Memo, Memoed};
use zerocode_core::jev::summary::{AGREED, AT, ELAPSED_MS};
use zerocode_core::jev::{
    BRANCHING, BRANCHING_APPLY_DEADLINE_MS, JUDGMENT_CACHE, JevMode, JevUse, memo,
};
use zerocode_core::screen_action::ActionAsk;

use super::{ACTION_DEADLINE, ActionJudge, Compared, Done, Judged, Pending, Spent};
use crate::api_routers::RouterKeys;
use crate::systemone::{SCHEMA, Wire, memo_path, request_body};

/// The key the judgment cache's rows keep the screen seat they answered for
/// under, and the memo key the lookup was made with.
const FOR_SEAT_KEY: &str = "seat";
const MEMO_KEY: &str = "key";
/// The key a lookup that found nothing is written under — a row with no
/// `outcome`, which the judge does not count and a hit-rate reader does.
const MISS_KEY: &str = "miss";

/// What a successful body says about the question, or why it says nothing.
/// Every shape rule is `browser_action`'s: this reads the envelope and hands
/// the answers straight to the question that was asked.
#[must_use]
pub fn read_body(ask: &ActionAsk, body: &str) -> Judged {
    let Ok(parsed) = serde_json::from_str::<Value>(body) else {
        return Judged::Refused(SCHEMA.to_string());
    };
    let Some(answers) = parsed.get("answers") else {
        return Judged::Refused(SCHEMA.to_string());
    };
    match ask.read(answers) {
        Ok(choice) => Judged::Chose(choice),
        // An answer that broke one of the closed choice's rules is refused by
        // THAT rule's own word. One word for every refusal puts the cause out
        // of reach of the row that records it, and the cause is the row's
        // whole value: a sum the wire's rounding explains and an option nobody
        // offered are not the same event, and only one of them is ours to fix.
        Err(why) => Judged::Refused(why.token().to_string()),
    }
}

/// The request body, as the endpoint takes it.
#[must_use]
pub fn request_of(ask: &ActionAsk) -> Value {
    request_body(&ask.state, &ask.questions)
}

/// What a successful body says about a forked step's comparison, read the
/// way [`read_body`] reads a press: the envelope here, every shape rule the
/// question's own ([`BranchAsk::read`]).
#[must_use]
pub fn read_compared(ask: &BranchAsk, body: &str) -> Compared {
    let Ok(parsed) = serde_json::from_str::<Value>(body) else {
        return Compared::Refused(SCHEMA.to_string());
    };
    let Some(answers) = parsed.get("answers") else {
        return Compared::Refused(SCHEMA.to_string());
    };
    match ask.read(answers) {
        Ok(choice) => Compared::Chose(choice),
        Err(why) => Compared::Refused(why.token().to_string()),
    }
}

/// Where a test points the door: zo's settings file and the workspace the
/// stopped walk runs for.
#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Doorway {
    pub settings: Option<PathBuf>,
    pub workspace: Option<PathBuf>,
    pub seat: &'static JevUse,
}

#[cfg(test)]
impl Default for Doorway {
    fn default() -> Self {
        Self {
            settings: None,
            workspace: None,
            seat: &zerocode_core::jev::BROWSER,
        }
    }
}

/// The judge that actually asks. Built once per walk, so a walk that is
/// barred or never asks does not read the keychain at all.
pub struct LiveJudge {
    wire: Wire,
    /// The workspace the walk runs for — the folder it was asked from, as the
    /// Computer Use door said it; `None` consents to nothing.
    workspace: Option<PathBuf>,
    /// The Jev use table's row for the surface being walked: whose consent
    /// the door checks, and whose ledger the row lands in.
    seat: &'static JevUse,
    spent: Option<Spent>,
    /// Whether the last question was answered by the memo.
    cached: bool,
    /// The judgment cache's own rows — one per lookup the memo was asked —
    /// written to its ledger by the walk's caller ([`Self::write_memo_rows`])
    /// through the same road every seat's rows take.
    memo_rows: Vec<Value>,
}

/// How the judgment cache stands for one question: the seat's mode as the
/// person set it, and whether a risen `auto` may answer from the memo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CacheStand {
    mode: JevMode,
    applying: bool,
}

impl LiveJudge {
    /// Read the key the settings pane keeps; the door reads zo's settings file
    /// for the walk's `workspace`, and `seat` is the surface's own row.
    #[must_use]
    pub fn new(keys: &dyn RouterKeys, workspace: Option<&Path>, seat: &'static JevUse) -> Self {
        let wire = Wire::new(keys);
        // The walk's first look is longer than a handshake: connect now,
        // so the first question pays for its answer alone.
        wire.warm();
        Self {
            wire,
            workspace: workspace.map(Path::to_path_buf),
            seat,
            spent: None,
            cached: false,
            memo_rows: Vec::new(),
        }
    }

    /// A judge pointed at one origin with one key, behind the door `doorway`
    /// names — how a test crosses a real socket.
    #[cfg(test)]
    #[must_use]
    pub fn at(base: &str, key: &str, doorway: Doorway) -> Self {
        Self {
            wire: Wire::at(base, key, doorway.settings),
            workspace: doorway.workspace,
            seat: doorway.seat,
            spent: None,
            cached: false,
            memo_rows: Vec::new(),
        }
    }

    /// Whether this judge can ask at all — what a caller reads to skip a look
    /// it would only throw away.
    #[must_use]
    pub fn armed(&self) -> bool {
        self.wire.armed()
    }

    /// The wire this walk asks down, for the two questions the caller has to
    /// answer from the same settings file and the same config home the
    /// questions go through: whether the seat is acting
    /// ([`crate::systemone::applies`]) and where its ledger lives
    /// ([`super::write_rows`]). Borrowed rather than built a second time —
    /// two wires could read the settings at two different moments and have
    /// a walk press on one answer while recording under the other.
    #[must_use]
    pub const fn wire(&self) -> &Wire {
        &self.wire
    }

    /// Where the judgment cache stands right now, read from the same settings
    /// file and the same ledger root the questions go through. `None` when
    /// the cache is off — and then no memo is asked, no file is touched and
    /// no row is written: today's walk to the byte.
    fn cache_stand(&self) -> Option<CacheStand> {
        let mode = JUDGMENT_CACHE.mode_in(&self.wire.settings_root());
        mode.asks().then(|| CacheStand {
            mode,
            applying: crate::systemone::applies(&self.wire, &JUDGMENT_CACHE),
        })
    }

    /// The judgment cache's rows this walk produced, taken — the caller
    /// writes them beside the walk's own through [`super::write_rows`].
    pub fn write_memo_rows(&mut self, dir: Option<&Path>, now_ms: i64) {
        let rows = std::mem::take(&mut self.memo_rows);
        super::write_rows(&JUDGMENT_CACHE, &self.wire, dir, &rows, now_ms);
    }

    /// One question down the wire, with the memo in front of it when the
    /// cache asks (t-6132). What the memo said becomes one row of the cache
    /// seat's own: a miss (no `outcome`), a hit compared against the fresh
    /// answer under `shadow` and an unraised `auto` (`agreed`), or a hit that
    /// answered under a risen `auto` (`routeUse: applied`). A remembered body
    /// the question's own rules refuse is a hit that no longer reads: the
    /// wire is asked after all, and the row says `schema`.
    fn choose_remembering(&mut self, ask: &ActionAsk, stand: CacheStand) -> Judged {
        let Some(path) = memo_path(&self.wire) else {
            return self.choose_plain(ask);
        };
        let asked = self.wire.ask_remembering(
            self.seat,
            self.workspace.as_deref(),
            request_of(ask),
            ACTION_DEADLINE,
            Some(Memo {
                path: &path,
                seat: self.seat,
                applying: stand.applying,
            }),
        );
        let Some(memoed) = asked.memo.clone() else {
            // Refused at the door before the memo was reached: no lookup, no
            // row of the cache's own.
            self.spent = Some(asked.spent);
            return match asked.answer {
                Ok(body) => read_body(ask, &body),
                Err(token) => Judged::Refused(token),
            };
        };
        let now_ms = crate::project_runtime::now_epoch_ms();
        let mut row = json!({
            AT.canonical: now_ms,
            FOR_SEAT_KEY: self.seat.id,
            MEMO_KEY: memoed.key,
            "mode": stand.mode.key(),
        });
        let Some(recalled) = memoed.recalled.as_ref() else {
            // A miss: the wire answers as ever, and what it answered is
            // remembered for the next walk that asks these bytes.
            row[MISS_KEY] = json!(true);
            self.memo_rows.push(row);
            return self.answer_and_remember(ask, &asked.answer, asked.spent, &path, &memoed);
        };
        row[ELAPSED_MS.canonical] = json!(memoed.lookup_ms);
        // A hit sends nothing, and the version on its row is the one that
        // gave the answer the memo kept (t-6187).
        Spent {
            requests: 0,
            redacted_lines: 0,
            model: crate::systemone::answered_by(&recalled.answer),
        }
        .stamp(&mut row);
        if memoed.answered {
            // The memo's answer stands in for the wire's — if it still reads.
            match read_body(ask, &recalled.answer) {
                Judged::Chose(choice) => {
                    row["outcome"] = json!("answered");
                    row["routeUse"] = json!(zerocode_core::jev::ROUTE_USE_APPLIED);
                    self.memo_rows.push(row);
                    self.spent = Some(asked.spent);
                    self.cached = true;
                    return Judged::Chose(choice);
                }
                Judged::Refused(token) => {
                    row["outcome"] = json!(token);
                    row["routeUse"] = json!(zerocode_core::jev::ROUTE_USE_FALLBACK);
                    self.memo_rows.push(row);
                    // The wire after all, counted as any request.
                    let fresh = self.wire.ask(
                        self.seat,
                        self.workspace.as_deref(),
                        request_of(ask),
                        ACTION_DEADLINE,
                    );
                    return self.answer_and_remember(
                        ask,
                        &fresh.answer,
                        fresh.spent,
                        &path,
                        &memoed,
                    );
                }
            }
        }
        // A hit under a seat that only compares: the wire was asked, and the
        // memo is labeled by whether it named the same number.
        row["outcome"] = json!("answered");
        row["routeUse"] = json!(JevMode::Shadow.key());
        let fresh = self.answer_and_remember(ask, &asked.answer, asked.spent, &path, &memoed);
        if let (Judged::Chose(fresh), Judged::Chose(remembered)) =
            (&fresh, read_body(ask, &recalled.answer))
        {
            row[AGREED.canonical] = json!(fresh.chosen == remembered.chosen);
        }
        self.memo_rows.push(row);
        fresh
    }

    /// The wire's answer read through the question, and — a choice that
    /// passed its checks — remembered under the memo's key.
    fn answer_and_remember(
        &mut self,
        ask: &ActionAsk,
        answer: &Result<String, String>,
        spent: Spent,
        path: &Path,
        memoed: &Memoed,
    ) -> Judged {
        self.spent = Some(spent);
        let judged = match answer {
            Ok(body) => read_body(ask, body),
            Err(token) => Judged::Refused(token.clone()),
        };
        if let (Judged::Chose(_), Ok(body)) = (&judged, answer)
            && let Err(why) = memo::remember(
                path,
                self.seat,
                &memoed.key,
                body,
                crate::project_runtime::now_epoch_ms(),
            )
        {
            eprintln!("judgment memo: the answer was not remembered: {why}");
        }
        judged
    }

    /// The question as it was asked before the memo existed: the wire alone.
    fn choose_plain(&mut self, ask: &ActionAsk) -> Judged {
        let asked = self.wire.ask(
            self.seat,
            self.workspace.as_deref(),
            request_of(ask),
            ACTION_DEADLINE,
        );
        self.spent = Some(asked.spent);
        match asked.answer {
            Ok(body) => read_body(ask, &body),
            Err(token) => Judged::Refused(token),
        }
    }
}

impl LiveJudge {
    /// This judge's twin for a thread of its own: the same wire, workspace
    /// and seat, with nothing yet asked — what a question begun ahead of the
    /// walk runs on ([`ActionJudge::begin`]).
    fn twin(&self) -> Self {
        Self {
            wire: self.wire.clone(),
            workspace: self.workspace.clone(),
            seat: self.seat,
            spent: None,
            cached: false,
            memo_rows: Vec::new(),
        }
    }
}

impl ActionJudge for LiveJudge {
    fn choose(&mut self, ask: &ActionAsk) -> Judged {
        self.cached = false;
        match self.cache_stand() {
            Some(stand) => self.choose_remembering(ask, stand),
            None => self.choose_plain(ask),
        }
    }

    fn begin(&mut self, ask: &ActionAsk) -> Option<Pending> {
        let (done, waited) = std::sync::mpsc::channel();
        let mut twin = self.twin();
        let ahead = ask.clone();
        std::thread::Builder::new()
            .name("jev-ahead".to_string())
            .spawn(move || {
                let judged = twin.choose(&ahead);
                let _ = done.send(Done {
                    judged,
                    spent: twin.spent,
                    cached: twin.cached,
                    rows: std::mem::take(&mut twin.memo_rows),
                });
            })
            .ok()?;
        Some(Pending::new(ask.clone(), waited))
    }

    fn finish(&mut self, done: Done) -> Judged {
        self.spent = done.spent;
        self.cached = done.cached;
        self.memo_rows.extend(done.rows);
        done.judged
    }

    /// The forked step's comparison (t-6044): the same wire, the same
    /// workspace's consent, under the branching seat's own row and wall.
    fn compare(&mut self, ask: &BranchAsk) -> Compared {
        let asked = self.wire.ask(
            &BRANCHING,
            self.workspace.as_deref(),
            request_body(&ask.state, &ask.questions),
            Duration::from_millis(BRANCHING_APPLY_DEADLINE_MS),
        );
        self.spent = Some(asked.spent);
        match asked.answer {
            Ok(body) => read_compared(ask, &body),
            Err(token) => Compared::Refused(token),
        }
    }

    fn spent(&self) -> Option<Spent> {
        self.spent.clone()
    }

    fn cached(&self) -> bool {
        self.cached
    }
}

#[cfg(test)]
mod tests;
