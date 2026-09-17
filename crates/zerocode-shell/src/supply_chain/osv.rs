//! The OSV wire: batches of questions, their later pages, one read per
//! record, one deadline over all of it, and one closed table of failure words.
//!
//! What a question may carry is `zerocode_core::supply_chain::osv_queries`'
//! decision; this file only sends what that decided.

use std::collections::BTreeSet;
use std::time::Duration;

use serde_json::Value;
use zerocode_core::supply_chain::{
    OSV_BATCH_MAX_QUERIES, OsvFindings, OsvQuery, OsvRecord, osv_batch_body, read_osv_batch,
    read_osv_record,
};

/// The public service.
pub(crate) const OSV_BASE_URL: &str = "https://api.osv.dev";
const QUERY_BATCH_PATH: &str = "/v1/querybatch";
const RECORD_PATH: &str = "/v1/vulns/";

/// The failure words, the whole closed table. An HTTP refusal keeps its
/// status in its word ([`http_word`]).
pub(crate) const OFFLINE: &str = "offline";
pub(crate) const TIMEOUT: &str = "timeout";
pub(crate) const BAD_ANSWER: &str = "bad_answer";

/// How long one lookup may take, every batch, page and record read together.
/// Past it the answer is `timeout` and the components stand without it.
///
/// Measured against api.osv.dev on 2026-09-17: this repository's first lookup
/// (1,074 questions, 2 batches, 28 records) took 11.8 s end to end, so a
/// minute is five of those — room for a workspace a few times larger or a
/// slow network, and still an answer the card can wait for once a day.
pub(crate) const LOOKUP_DEADLINE: Duration = Duration::from_secs(60);

/// The largest answer body read. OSV's own limit on a response over HTTP/1.1
/// is 32 MiB (google.github.io/osv.dev/api); a body past it is not an answer
/// OSV gives.
pub(crate) const ANSWER_MAX_BYTES: usize = 32 * 1024 * 1024;

/// The word an HTTP refusal is answered with.
pub(crate) fn http_word(status: u16) -> String {
    format!("http_{status}")
}

/// Where the questions go, and how long they may take.
#[derive(Debug, Clone)]
pub(crate) struct OsvWire {
    base: String,
    deadline: Duration,
}

impl OsvWire {
    /// OSV itself.
    pub(crate) fn public() -> Self {
        Self::at(OSV_BASE_URL, LOOKUP_DEADLINE)
    }

    /// A wire pointed at one origin with one deadline — the road a test
    /// crosses a real socket with.
    pub(crate) fn at(base: &str, deadline: Duration) -> Self {
        Self {
            base: base.trim_end_matches('/').to_string(),
            deadline,
        }
    }
}

/// The requests a lookup sent, counted whether it succeeded or not.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Sent {
    pub(crate) batches: usize,
    pub(crate) records: usize,
}

/// What a lookup learned.
#[derive(Debug)]
pub(crate) struct Looked {
    pub(crate) findings: OsvFindings,
    pub(crate) records: Vec<OsvRecord>,
}

/// Ask OSV about `queries`: each batch of at most
/// [`OSV_BATCH_MAX_QUERIES`], each later page of the questions that have one,
/// then each record once. `Err` is a failure word; `sent` counts the requests
/// either way.
pub(crate) fn look_up(
    wire: &OsvWire,
    queries: &[OsvQuery],
    sent: &mut Sent,
) -> Result<Looked, String> {
    tauri::async_runtime::block_on(async {
        tokio::time::timeout(wire.deadline, ask(wire, queries, sent))
            .await
            .unwrap_or_else(|_| Err(TIMEOUT.to_string()))
    })
}

async fn ask(wire: &OsvWire, queries: &[OsvQuery], sent: &mut Sent) -> Result<Looked, String> {
    let client = reqwest::Client::builder()
        .build()
        .map_err(|_| OFFLINE.to_string())?;
    let batch_url = format!("{}{QUERY_BATCH_PATH}", wire.base);
    let mut findings = OsvFindings::new();
    for batch in queries.chunks(OSV_BATCH_MAX_QUERIES) {
        let mut pending: Vec<OsvQuery> = batch.to_vec();
        while !pending.is_empty() {
            sent.batches += 1;
            let answer = send(client.post(&batch_url).json(&osv_batch_body(&pending))).await?;
            let results =
                read_osv_batch(&answer, pending.len()).map_err(|_| BAD_ANSWER.to_string())?;
            let mut later = Vec::new();
            for (query, result) in pending.iter().zip(results) {
                if !result.ids.is_empty() {
                    findings
                        .entry(query.purl().to_string())
                        .or_default()
                        .extend(result.ids);
                }
                if let Some(token) = result.next_page_token {
                    // A page that names itself again would be asked forever.
                    if query.page_token() == Some(token.as_str()) {
                        return Err(BAD_ANSWER.to_string());
                    }
                    later.push(query.next_page(&token));
                }
            }
            pending = later;
        }
    }
    for ids in findings.values_mut() {
        ids.sort();
        ids.dedup();
    }
    let ids: BTreeSet<&str> = findings.values().flatten().map(String::as_str).collect();
    let mut records = Vec::with_capacity(ids.len());
    for id in ids {
        // An id is the answer's word, so it is one path segment and no more.
        let url = format!(
            "{}{RECORD_PATH}{}",
            wire.base,
            crate::remote_repo::encode_component(id)
        );
        sent.records += 1;
        let record =
            read_osv_record(&send(client.get(&url)).await?).map_err(|_| BAD_ANSWER.to_string())?;
        if record.id != id {
            return Err(BAD_ANSWER.to_string());
        }
        records.push(record);
    }
    Ok(Looked { findings, records })
}

/// One request, and its JSON answer or its failure word.
async fn send(request: reqwest::RequestBuilder) -> Result<Value, String> {
    let mut answer = request.send().await.map_err(transport_word)?;
    let status = answer.status();
    if !status.is_success() {
        return Err(http_word(status.as_u16()));
    }
    let mut body = Vec::new();
    while let Some(chunk) = answer.chunk().await.map_err(transport_word)? {
        if body.len().saturating_add(chunk.len()) > ANSWER_MAX_BYTES {
            return Err(BAD_ANSWER.to_string());
        }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&body).map_err(|_| BAD_ANSWER.to_string())
}

/// A request that did not come back: out of time, or not through at all.
fn transport_word(error: reqwest::Error) -> String {
    if error.is_timeout() {
        TIMEOUT.to_string()
    } else {
        OFFLINE.to_string()
    }
}
