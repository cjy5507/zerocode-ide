//! The window's browser-read seat (t-6041, `zerocode_core::jev::BROWSER_READ`,
//! `smart.jevBrowserRead`): what `zerocode-browser read` hands an agent when
//! the seat is asked.
//!
//! The question, the answer space, the fold and the containment rule a label
//! is written by are the core's (`zerocode_core::browser_read`); the socket,
//! the door and the ledger are the window's one wire
//! (`crate::systemone`). This file holds the road between them:
//!
//! 1. the page is read whole AND cut into blocks in one page script
//!    (`cmd::browser::automate_read_blocks`) — the whole text is the very
//!    expression the plain read evaluates, so every road that hands the
//!    page back whole hands back the same bytes;
//! 2. the blocks' questions leave in shards, side by side, under the seat's
//!    one wall ([`READ_DEADLINE`]) — the read is a tool result the agent is
//!    waiting on, so the shards are asked on threads of their own and the
//!    slowest of them is what the read waits for;
//! 3. an answer that came back whole is folded under `on`, or an `auto`
//!    the seat's own judge raised, and recorded under `shadow`; anything
//!    else — a wall, a refusal, a shard that broke the closed choice's rules
//!    — hands back the whole page, which is what the read did before the
//!    seat existed;
//! 4. the judgment is kept under the pane's label until the agent's next
//!    `click` or `type` on that pane, which labels it: a press that landed
//!    in a block the judgment called chrome is a fold the agent would have
//!    reached back into ([`note_press`]).
//!
//! Under `off`, nothing here runs: the dispatcher takes the plain read's
//! road, and a source contract holds that both roads print through one
//! formatter.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use zerocode_core::browser_read::{self, ReadAsk, Verdict, inside};
use zerocode_core::jev::promote::SEAT_RECORDING;
use zerocode_core::jev::summary::{
    AGREED, APPLIED, AT, ELAPSED_MS, INPUT_TOKENS, LABEL, ROUTE_USE,
};
use zerocode_core::jev::{
    BROWSER_READ, BROWSER_READ_APPLY_DEADLINE_MS, JevMode, ROUTE_USE_APPLIED, ROUTE_USE_FALLBACK,
};

use crate::cmd::browser::BrowserReadPage;
use crate::systemone::{Asked, Wire, request_body};

/// How long one read waits for every shard's answer — the seat's own wall,
/// read from the use table that judges a rising seat against it.
pub(crate) const READ_DEADLINE: Duration = Duration::from_millis(BROWSER_READ_APPLY_DEADLINE_MS);

/// The row's outcome for a read whose every shard answered in shape.
const ANSWERED: &str = zerocode_core::jev::summary::ANSWERED;

/// The row's key for why the seat did not act, beside `routeUse`.
const REASON: &str = "reason";

/// The row's key for the read it is about, and the label row's [`LABEL`]:
/// `<pane label>@<at>`, the one name a read and its label share.
const READ_KEY: &str = "read";

/// The command an agent runs to read the folded page whole — the hint the
/// fold line carries, spelled from the door's own words.
fn full_hint(label: &str) -> String {
    format!(
        "`{} read {label} {}`",
        zerocode_core::agent_browser::BROWSER_CLI,
        zerocode_core::agent_browser::READ_FULL_FLAG
    )
}

/// What the seat decided about one read, kept under the pane's label until
/// the agent's next press on that pane labels it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReadJudgment {
    /// The row's own name — what the label row points at.
    pub(crate) read: String,
    /// The page's address at the read, cleared of credentials.
    pub(crate) url: String,
    /// The paths of every block the judgment called chrome, whether or not
    /// the fold dropped it: what is compared is the judgment, not the fold.
    pub(crate) chrome: Vec<String>,
    /// Whether the read handed back the folded page.
    pub(crate) applied: bool,
    pub(crate) mode: JevMode,
}

/// What one judged read came to.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Judged {
    /// The text the read hands the agent.
    pub(crate) text: String,
    /// The ledger row, when anything was asked.
    pub(crate) row: Option<Value>,
    /// The judgment a later press may label, when one answered and called
    /// at least one block chrome — a judgment that folds nothing has
    /// nothing a press could regret.
    pub(crate) judgment: Option<ReadJudgment>,
}

/// The read's judgments, one per pane, until a press labels each.
fn store() -> &'static Mutex<HashMap<String, ReadJudgment>> {
    static STORE: OnceLock<Mutex<HashMap<String, ReadJudgment>>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Keep `judgment` under `label`, in place of any earlier read's — a read
/// the agent never pressed after leaves no label.
pub(crate) fn remember(label: &str, judgment: ReadJudgment) {
    store()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(label.to_string(), judgment);
}

/// Forget what a pane's last read decided — a pane that closed, or a test
/// that wants a clean slate.
pub(crate) fn forget(label: &str) {
    store()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .remove(label);
}

/// A page's host and the fingerprint of its path, for the row (t-6155 F8):
/// the host names the site, and the fingerprint lets two reads of one page
/// be grouped without the path — which can carry a ticket number, a user
/// id or a signed URL's tail — ever reaching a ledger. No other window seat
/// writes an address, and the label needs none: it is joined by the read's
/// own key, and the page is compared in memory (`same_page`).
fn where_of(url: &str) -> (String, String) {
    url.parse::<tauri::Url>()
        .map(|parsed| {
            (
                parsed.host_str().unwrap_or_default().to_string(),
                zerocode_core::jev::fingerprint_of(parsed.path()),
            )
        })
        .unwrap_or_default()
}

/// The verb a `read --full` label row carries — the agent asked for the
/// whole page back after the seat folded it.
pub(crate) const READ_FULL_VERB: &str = "read_full";

/// Two addresses name the same page when they agree up to the fragment —
/// a `#section` the agent scrolled to is the same page.
fn same_page(read: &str, now: &str) -> bool {
    let cut = |url: &str| url.split('#').next().unwrap_or(url).to_string();
    cut(read) == cut(now)
}

/// The input tokens a System One body says it billed, if it says.
fn input_tokens_of(body: &str) -> u64 {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|parsed| parsed["usage"]["input_tokens"].as_u64())
        .unwrap_or(0)
}

/// Ask every shard side by side and wait for the slowest, under one wall.
fn ask_shards(wire: &Wire, workspace: Option<&Path>, asks: &[ReadAsk]) -> Vec<Asked> {
    std::thread::scope(|scope| {
        let handles: Vec<_> = asks
            .iter()
            .map(|ask| {
                scope.spawn(move || {
                    wire.ask(
                        &BROWSER_READ,
                        workspace,
                        request_body(&ask.state, &ask.questions),
                        READ_DEADLINE,
                    )
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| {
                handle.join().unwrap_or_else(|_| Asked {
                    answer: Err(crate::systemone::TRANSPORT.to_string()),
                    spent: crate::systemone::Spent::default(),
                    request_bytes: 0,
                    memo: None,
                })
            })
            .collect()
    })
}

/// Judge one read: ask, fold or record, and say what the agent reads.
///
/// `acting` is whether the seat acts right now (`crate::systemone::applies`,
/// read by the caller from the same settings the mode came from). `now_ms`
/// stamps the row and names the read.
pub(crate) fn settle(
    wire: &Wire,
    workspace: Option<&Path>,
    label: &str,
    page: &BrowserReadPage,
    mode: JevMode,
    acting: bool,
    now_ms: i64,
) -> Judged {
    let whole = || Judged {
        text: page.report.text.clone(),
        row: None,
        judgment: None,
    };
    if !mode.asks() {
        return whole();
    }
    let asks = browser_read::ask(&page.report.title, &page.blocks);
    if asks.is_empty() {
        return whole();
    }
    let (host, path_fingerprint) = where_of(&page.report.url);
    let read = format!("{label}@{now_ms}");
    let mut row = json!({
        AT.canonical: now_ms,
        READ_KEY: read,
        "pane": label,
        "host": host,
        "pathFingerprint": path_fingerprint,
        "mode": mode.key(),
        "rubricVersion": browser_read::BROWSER_READ_RUBRIC_VERSION,
        "blocks": page.blocks.len(),
        "asked": asks.iter().map(|ask| ask.asked().len()).sum::<usize>(),
        "shards": asks.len(),
        "charsBefore": page.report.text.chars().count(),
    });
    let began = Instant::now();
    let answers = ask_shards(wire, workspace, &asks);
    row[ELAPSED_MS.canonical] =
        json!(u64::try_from(began.elapsed().as_millis()).unwrap_or(u64::MAX));
    crate::systemone::Spent::together(answers.iter().map(|asked| &asked.spent)).stamp(&mut row);
    row["requestBytes"] = json!(
        answers
            .iter()
            .map(|asked| asked.request_bytes)
            .sum::<usize>()
    );
    row[INPUT_TOKENS.canonical] = json!(
        answers
            .iter()
            .filter_map(|asked| asked.answer.as_deref().ok())
            .map(input_tokens_of)
            .sum::<u64>()
    );

    // Every shard read through the question it asked; the first that did
    // not come back whole names the row's outcome, and the page goes back
    // whole.
    let mut readings = Vec::new();
    let mut failed = None;
    let mut shards_answered = 0_usize;
    for (ask, asked) in asks.iter().zip(&answers) {
        let read = asked
            .answer
            .as_ref()
            .map_err(String::clone)
            .and_then(|body| {
                serde_json::from_str::<Value>(body)
                    .ok()
                    .and_then(|parsed| parsed.get("answers").cloned())
                    .ok_or_else(|| crate::systemone::SCHEMA.to_string())
                    .and_then(|answers| ask.read(&answers).map_err(|why| why.token().to_string()))
            });
        match read {
            Ok(shard) => {
                shards_answered += 1;
                readings.extend(shard);
            }
            Err(token) => {
                failed.get_or_insert(token);
            }
        }
    }
    row["shardsAnswered"] = json!(shards_answered);
    if let Some(token) = failed {
        row["outcome"] = json!(token);
        row[ROUTE_USE.canonical] = json!(ROUTE_USE_FALLBACK);
        row[APPLIED.canonical] = json!(false);
        row["charsAfter"] = json!(page.report.text.chars().count());
        return Judged {
            text: page.report.text.clone(),
            row: Some(row),
            judgment: None,
        };
    }

    let verdict = Verdict::of(readings);
    // From the act line the seat's labels drew, where they drew one
    // (t-9468): the fold's floor otherwise.
    let droppable = verdict.droppable_at(
        &BROWSER_READ,
        crate::systemone::act_line(wire, &BROWSER_READ),
    );
    let chrome: Vec<String> = verdict
        .chrome
        .keys()
        .filter_map(|index| page.blocks.get(*index))
        .map(|block| block.path.clone())
        .collect();
    row["outcome"] = json!(ANSWERED);
    row["chrome"] = json!(verdict.chrome.len());
    row["droppable"] = json!(droppable.len());
    let text = if acting {
        let folded = browser_read::fold(&page.blocks, &droppable, &full_hint(label));
        row["folded"] = json!(folded.dropped.len());
        row[ROUTE_USE.canonical] = json!(ROUTE_USE_APPLIED);
        row[APPLIED.canonical] = json!(true);
        folded.text
    } else {
        row["folded"] = json!(0);
        row[ROUTE_USE.canonical] = json!(JevMode::Shadow.key());
        row[APPLIED.canonical] = json!(false);
        row[REASON] = json!(SEAT_RECORDING);
        page.report.text.clone()
    };
    row["charsAfter"] = json!(text.chars().count());
    let judgment = (!chrome.is_empty()).then(|| ReadJudgment {
        read: row[READ_KEY].as_str().unwrap_or_default().to_string(),
        url: page.report.url.clone(),
        chrome,
        applied: acting,
        mode,
    });
    Judged {
        text,
        row: Some(row),
        judgment,
    }
}

/// Label a pane's last judged read by the press that followed it: `agreed`
/// when the press landed outside every block the judgment called chrome,
/// not when it landed inside one. One label per read; a press on another
/// page, or one whose block the page could not name, labels nothing and
/// the judgment is spent either way.
///
/// `url_now` and `block_path` are what the press's own page script answered
/// (`cmd::browser::BrowserInputReport`), so the page that was pressed and
/// the block that was pressed come from the same look.
pub(crate) fn note_press(
    label: &str,
    verb: &str,
    url_now: Option<&str>,
    block_path: Option<&str>,
    now_ms: i64,
) -> Option<Value> {
    let judgment = store()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .remove(label)?;
    let url_now = url_now?;
    if !same_page(&judgment.url, url_now) {
        return None;
    }
    let chain = block_path?;
    let regretted = judgment.chrome.iter().any(|path| inside(path, chain));
    Some(json!({
        AT.canonical: now_ms,
        LABEL.canonical: judgment.read,
        AGREED.canonical: !regretted,
        "verb": verb,
        "pane": label,
        "mode": judgment.mode.key(),
        APPLIED.canonical: judgment.applied,
        "blockPath": chain,
    }))
}

/// Label a pane's last judged read by a `read --full` that followed it on
/// the same page (t-6155 F6): the agent asked for the whole page back, which
/// is the fold's first and plainest regret — `agreed: false`, one label per
/// read, in the same book a press writes to. A whole read under a seat that
/// folded nothing (recording, or a judgment nothing was applied from) says
/// nothing and spends nothing: the agent already had the page whole, and
/// the press that follows may still label the judgment. Another page spends
/// the judgment as a press on another page does.
pub(crate) fn note_full_read(label: &str, url_now: &str, now_ms: i64) -> Option<Value> {
    let mut held = store().lock().unwrap_or_else(PoisonError::into_inner);
    if !held.get(label)?.applied {
        return None;
    }
    let judgment = held.remove(label)?;
    drop(held);
    if !same_page(&judgment.url, url_now) {
        return None;
    }
    Some(json!({
        AT.canonical: now_ms,
        LABEL.canonical: judgment.read,
        AGREED.canonical: false,
        "verb": READ_FULL_VERB,
        "pane": label,
        "mode": judgment.mode.key(),
        APPLIED.canonical: judgment.applied,
    }))
}

/// Append `rows` to the seat's ledger through the window's one Jev-ledger
/// door — which also judges the seat when a judgment is due.
pub(crate) fn record(wire: &Wire, rows: &[Value], now_ms: i64) {
    if let Some(ledger) = crate::systemone::ledger_of(wire, &BROWSER_READ) {
        crate::systemone::record_rows(&BROWSER_READ, &ledger, rows, now_ms);
    }
}

/// The whole-document read an agent's `zerocode-browser read <label>` gets
/// when the seat is asked: the page read and cut in one script, judged off
/// the async runtime, recorded, and remembered for the press that labels it.
///
/// `workspace` is the checkout the pane runs in — the workspace the door
/// asks the person's consent for. Under `off` this is never called: the
/// dispatcher takes the plain read's road.
pub(crate) async fn read_judged(
    app: &tauri::AppHandle,
    state: &crate::AppState,
    label: &str,
    workspace: Option<PathBuf>,
    mode: JevMode,
) -> Result<crate::cmd::browser::BrowserReadReport, String> {
    let page = crate::cmd::browser::automate_read_blocks(app, state, label).await?;
    let label = label.to_string();
    let now_ms = crate::project_runtime::now_epoch_ms();
    let judged = tauri::async_runtime::spawn_blocking(move || {
        // The key is read once, here, off the runtime: every shard rides
        // the same wire rather than reading the keychain a shard at a time.
        let wire = Wire::new(&crate::api_routers::Keychain::of_this_machine());
        let acting = crate::systemone::applies(&wire, &BROWSER_READ);
        let judged = settle(
            &wire,
            workspace.as_deref(),
            &label,
            &page,
            mode,
            acting,
            now_ms,
        );
        if let Some(row) = &judged.row {
            record(&wire, std::slice::from_ref(row), now_ms);
        }
        if let Some(judgment) = judged.judgment.clone() {
            remember(&label, judgment);
        } else {
            forget(&label);
        }
        (page, judged)
    })
    .await
    .map_err(|_| "브라우저 판의 본문 판정이 끝나지 않았습니다".to_string())?;
    let (page, judged) = judged;
    Ok(crate::cmd::browser::BrowserReadReport {
        title: page.report.title,
        url: page.report.url,
        text: judged.text,
        dom: None,
    })
}

/// Write the label a press leaves on a pane's last judged read, if any —
/// called by the click and type arms after the press really happened.
pub(crate) fn label_press(
    label: &str,
    verb: &str,
    report: &crate::cmd::browser::BrowserInputReport,
) {
    let now_ms = crate::project_runtime::now_epoch_ms();
    let Some(row) = note_press(
        label,
        verb,
        report.page_url.as_deref(),
        report.block_path.as_deref(),
        now_ms,
    ) else {
        return;
    };
    // The ledger's folder is the settings file's; no key is read for a row.
    let wire = Wire::of_this_machine();
    std::thread::spawn(move || record(&wire, &[row], now_ms));
}

/// Write the label a `read --full` leaves on a pane's last judged read, if
/// any — called by the read arm after the whole page really was read.
pub(crate) fn label_full_read(label: &str, url_now: &str) {
    let now_ms = crate::project_runtime::now_epoch_ms();
    let Some(row) = note_full_read(label, url_now, now_ms) else {
        return;
    };
    let wire = Wire::of_this_machine();
    std::thread::spawn(move || record(&wire, &[row], now_ms));
}

#[cfg(test)]
mod tests;
