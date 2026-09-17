//! A tool result that outgrew its ceiling is folded by a fast model before the
//! main model reads it.
//!
//! The dispatch seam caps every result (`runtime::TruncationConfig`) and keeps
//! the full bytes as an artifact. What the model saw of an over-cap result was
//! a blind head+tail cut: the first and last so-many characters of a
//! 3,000-line test log, with the one failing test somewhere in the elided
//! middle — and then a turn spent on `retrieve_tool_output` to go and find it.
//! This module replaces the blind cut with a *selection*: the same provider's
//! fast-tier model is handed the whole output (the plain-text view) and the
//! input that produced it, and returns only lines copied verbatim — errors with
//! their locations, summaries and counts, the matches that answer the call —
//! with `⋯ N lines omitted` markers between non-adjacent runs.
//!
//! Three rules keep it honest:
//! - **Verbatim or nothing.** Every kept line is checked against the source; a
//!   line the source does not contain is dropped, and a reply with more than
//!   [`MAX_UNVERIFIED_PERCENT`] of such lines is rejected wholesale.
//! - **Never load-bearing.** Any failure — no model, no credential, a timeout,
//!   an empty or rejected reply — falls back to the head+tail cut exactly as
//!   before, and the artifact plus its retrieval notice are untouched either
//!   way. After [`GIVE_UP_AFTER_FAILURES`] failures in a row the process stops
//!   trying, so a machine without the fast model's credential pays once.
//! - **Off unless the session turns it on.** The process default is
//!   [`ToolDigestMode::Off`]; the session boot configures it from
//!   `settings.json` (`toolDigest`) or [`TOOL_DIGEST_ENV`], so a unit test or a
//!   hermetic harness never dials a provider by accident.

use std::borrow::Cow;
use std::collections::HashSet;
use std::sync::atomic::{AtomicU32, AtomicU8, Ordering};
use std::time::Duration;

use serde_json::Value;

use crate::context::ToolContext;

/// Environment override for the session setting: `off` disables the fold.
pub const TOOL_DIGEST_ENV: &str = "ZO_TOOL_DIGEST";

/// Tools whose over-cap output is a stream of lines the model scans rather
/// than a document it edits. `read_file` is deliberately absent: an edit needs
/// the exact bytes, and its over-cap view is the structural outline already.
/// `retrieve_tool_output` is absent because the model asked for raw bytes.
const DIGESTIBLE_TOOLS: &[&str] = &[
    "bash",
    "grep_search",
    "glob_search",
    "WebFetch",
    "WebSearch",
    "TaskOutput",
];

/// The digest may keep this fraction of the tool's ceiling: half of a 16 KiB
/// bash cap is 8 KiB of failing tests and summaries, where the blind cut spent
/// the same 16 KiB on the first and last pages of the log.
const DIGEST_BUDGET_DIVISOR: usize = 2;

/// Beyond this many characters the digester is shown the head and tail of the
/// output with a marker between — a 400,000-char result (r27's maximum) is
/// real, and the fast tiers' contexts are large but not that large at a price
/// worth paying for one tool result.
const DIGEST_INPUT_CAP_CHARS: usize = 240_000;

/// Share of the shown characters given to the head when the input is folded.
const DIGEST_INPUT_HEAD_PERCENT: usize = 60;

/// Wall-clock allowance for the fold. It sits on the tool's critical path, but
/// the alternative is a whole main-model turn on `retrieve_tool_output`.
const DIGEST_TIMEOUT: Duration = Duration::from_secs(25);

/// Kept lines the source does not contain, as a share of all kept lines,
/// above which the reply is a paraphrase and is thrown away.
const MAX_UNVERIFIED_PERCENT: usize = 10;

/// Consecutive failures after which the process stops attempting digests.
const GIVE_UP_AFTER_FAILURES: u32 = 3;

/// Floor for the reply's `max_tokens`, so a small budget still fits a summary.
const MIN_DIGEST_TOKENS: u32 = 1_024;

/// Characters of the tool input echoed to the digester as its relevance cue.
const INPUT_CUE_CHARS: usize = 400;

/// Marker prefix for an omitted run; the digester writes it, and it is the one
/// non-verbatim line shape the verifier admits.
const OMITTED_MARK: &str = "⋯";

/// Whether over-cap results are folded in this process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolDigestMode {
    /// Every over-cap result takes the head+tail cut, as before.
    Off,
    /// An over-cap result of a `DIGESTIBLE_TOOLS` member is folded first.
    OverCap,
}

impl ToolDigestMode {
    /// Parse the `toolDigest` setting or the [`TOOL_DIGEST_ENV`] value.
    /// Anything but an explicit off is on: the fold is the default.
    #[must_use]
    pub fn from_setting(value: Option<&str>) -> Self {
        match value.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
            Some("off" | "false" | "0" | "no") => Self::Off,
            _ => Self::OverCap,
        }
    }
}

static MODE: AtomicU8 = AtomicU8::new(0);

/// Set the process-wide mode. The session boot calls this once from its
/// settings; nothing else should.
pub fn configure_tool_digest(mode: ToolDigestMode) {
    MODE.store(
        match mode {
            ToolDigestMode::Off => 0,
            ToolDigestMode::OverCap => 1,
        },
        Ordering::Relaxed,
    );
}

fn mode() -> ToolDigestMode {
    if MODE.load(Ordering::Relaxed) == 1 {
        ToolDigestMode::OverCap
    } else {
        ToolDigestMode::Off
    }
}

/// Whether `tool` (canonical name) is one whose over-cap output is folded.
#[must_use]
pub(crate) fn is_digestible(tool: &str) -> bool {
    DIGESTIBLE_TOOLS.contains(&tool)
}

/// Running tally of consecutive failures; a struct so a test owns its own.
pub(crate) struct DigestHealth(AtomicU32);

impl DigestHealth {
    pub(crate) const fn new() -> Self {
        Self(AtomicU32::new(0))
    }

    fn given_up(&self) -> bool {
        self.0.load(Ordering::Relaxed) >= GIVE_UP_AFTER_FAILURES
    }

    fn note_failure(&self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }

    fn note_success(&self) {
        self.0.store(0, Ordering::Relaxed);
    }
}

static HEALTH: DigestHealth = DigestHealth::new();

/// One reply from the fast model. A trait so the fold is tested without a
/// provider; the production implementation is [`ProviderDigester`].
pub(crate) trait Digester {
    /// The model the reply is attributed to in the digest header.
    fn model(&self) -> &str;
    /// The reply to `system` + `user`, bounded by `max_tokens`.
    fn reply(&self, system: &[String], user: &str, max_tokens: u32) -> Result<String, String>;
}

/// The session provider's fast-tier model, dialled the way sub-agents are.
struct ProviderDigester {
    model: String,
    client: api::ProviderClient,
}

impl ProviderDigester {
    fn for_parent(parent_model: Option<&str>) -> Result<Self, String> {
        // The cheap-model choice is a family alias (`haiku`); the wire wants
        // the release id, the same way the sub-agent client resolves it.
        let model = api::resolve_model_alias(&crate::fanout::decompose_model(parent_model));
        let client = crate::misc_tools::build_provider_client_for_agent(&model)?;
        Ok(Self { model, client })
    }
}

impl Digester for ProviderDigester {
    fn model(&self) -> &str {
        &self.model
    }

    fn reply(&self, system: &[String], user: &str, max_tokens: u32) -> Result<String, String> {
        let request = api::MessageRequest {
            model: self.model.clone(),
            max_tokens: max_tokens.min(api::max_tokens_for_model(&self.model)),
            messages: vec![api::InputMessage::user_text(user)],
            // The identity-first split the sub-agent path uses: the Claude Max
            // OAuth route rejects any other first system block.
            system: Some(runtime::split_system_with_identity(&system.join("\n\n"))),
            tools: None,
            tool_choice: None,
            stream: false,
            thinking: None,
            output_config: None,
            effort: None,
            effort_band_ceiling: None,
        };
        let response = api::sync_bridge::run_blocking(async {
            tokio::time::timeout(DIGEST_TIMEOUT, self.client.send_message(&request)).await
        })
        .map_err(|_| format!("digest timed out after {}s", DIGEST_TIMEOUT.as_secs()))?
        .map_err(|error| error.to_string())?;
        Ok(response
            .content
            .into_iter()
            .filter_map(|block| match block {
                api::OutputContentBlock::Text { text } => Some(text),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n"))
    }
}

/// Fold an over-cap result of `tool` for the session in `ctx`. `plain_output`
/// is the model-facing plain-text view (the lossless compression pass), and
/// `limit` the tool's character ceiling. `None` means: take the ordinary cut.
pub(crate) fn digest_over_cap(
    ctx: &ToolContext,
    tool: &str,
    input: &Value,
    plain_output: &str,
    limit: usize,
) -> Option<String> {
    if mode() == ToolDigestMode::Off || !is_digestible(tool) || HEALTH.given_up() {
        return None;
    }
    let Ok(digester) = ProviderDigester::for_parent(ctx.active_model().as_deref()) else {
        HEALTH.note_failure();
        return None;
    };
    digest_with(&digester, &HEALTH, tool, input, plain_output, limit)
}

/// The fold itself, with the model behind a trait and the failure tally
/// passed in. Returns the headed digest, or `None` when the reply was
/// unusable — in which case the caller keeps the head+tail cut.
pub(crate) fn digest_with(
    digester: &dyn Digester,
    health: &DigestHealth,
    tool: &str,
    input: &Value,
    output: &str,
    limit: usize,
) -> Option<String> {
    let budget = limit / DIGEST_BUDGET_DIVISOR;
    let shown = bounded_view(output);
    let user = user_prompt(tool, input, &shown);
    let Ok(reply) = digester.reply(&system_prompt(budget), &user, max_tokens_for_budget(budget))
    else {
        health.note_failure();
        return None;
    };
    let Some(kept) = verified_lines(&reply, output) else {
        health.note_failure();
        return None;
    };
    health.note_success();
    let kept = fit_budget(kept, budget);
    let kept_count = kept.iter().filter(|line| !line.starts_with(OMITTED_MARK)).count();
    let header = format!(
        "[digest] {} read {} lines ({} chars) of {tool} output and kept {kept_count} verbatim; {OMITTED_MARK} marks omitted runs.",
        digester.model(),
        output.lines().count(),
        output.chars().count(),
    );
    let mut content = header;
    for line in kept {
        content.push('\n');
        content.push_str(&line);
    }
    Some(content)
}

fn max_tokens_for_budget(budget_chars: usize) -> u32 {
    // Two characters a token is the pessimistic end (dense CJK); a reply
    // under the character budget always fits the token budget.
    u32::try_from(budget_chars / 2)
        .unwrap_or(u32::MAX)
        .max(MIN_DIGEST_TOKENS)
}

fn system_prompt(budget_chars: usize) -> Vec<String> {
    vec![
        runtime::CLAUDE_CODE_IDENTITY.to_string(),
        format!(
            "You compress one tool result for another engineer-model that must act on it. You are given the tool's name, the exact input it ran with, and its output. Return ONLY lines copied verbatim from the output — the ones that engineer needs: every error and failure with its location, warnings, final summaries and counts, and the lines that answer the input. Where kept lines were not adjacent in the output, put a marker line `{OMITTED_MARK} N lines omitted` between them. Never paraphrase, reorder, merge, trim or annotate a line; never add commentary or a code fence; never invent. Keep at most {budget_chars} characters in total. If nothing stands out, return the last 20 lines verbatim."
        ),
    ]
}

fn user_prompt(tool: &str, input: &Value, shown: &str) -> String {
    let cue: String = serde_json::to_string(input)
        .unwrap_or_default()
        .chars()
        .take(INPUT_CUE_CHARS)
        .collect();
    format!(
        "Tool: {tool}\nInput: {cue}\nOutput ({} lines):\n<<<\n{shown}\n>>>",
        shown.lines().count()
    )
}

/// The output as the digester sees it: whole when it fits
/// [`DIGEST_INPUT_CAP_CHARS`], otherwise head and tail cut at line boundaries
/// with a marker naming how many lines fell between.
fn bounded_view(output: &str) -> Cow<'_, str> {
    if output.chars().count() <= DIGEST_INPUT_CAP_CHARS {
        return Cow::Borrowed(output);
    }
    let head_chars = DIGEST_INPUT_CAP_CHARS * DIGEST_INPUT_HEAD_PERCENT / 100;
    let tail_chars = DIGEST_INPUT_CAP_CHARS - head_chars;
    let head_end = byte_offset_of_char(output, head_chars);
    let head_end = output[..head_end].rfind('\n').unwrap_or(head_end);
    let total_chars = output.chars().count();
    let tail_start = byte_offset_of_char(output, total_chars - tail_chars);
    let tail_start = output[tail_start..]
        .find('\n')
        .map_or(tail_start, |at| tail_start + at + 1);
    let tail_start = tail_start.max(head_end);
    let hidden_lines = output[head_end..tail_start].lines().count();
    Cow::Owned(format!(
        "{}\n{OMITTED_MARK} {hidden_lines} lines not shown to you {OMITTED_MARK}\n{}",
        &output[..head_end],
        &output[tail_start..]
    ))
}

fn byte_offset_of_char(text: &str, char_index: usize) -> usize {
    text.char_indices()
        .nth(char_index)
        .map_or(text.len(), |(at, _)| at)
}

/// The reply's lines that the source really contains (compared trimmed), plus
/// its omission markers. `None` when nothing verified or too much did not.
fn verified_lines(reply: &str, source: &str) -> Option<Vec<String>> {
    let source_lines: HashSet<&str> = source
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    let mut kept = Vec::new();
    let mut verified = 0usize;
    let mut unverified = 0usize;
    for line in reply.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with("```") {
            continue;
        }
        if trimmed.starts_with(OMITTED_MARK) {
            kept.push(trimmed.to_string());
            continue;
        }
        if source_lines.contains(trimmed) {
            verified += 1;
            kept.push(line.trim_end().to_string());
        } else {
            unverified += 1;
        }
    }
    if verified == 0 || unverified * 100 > (verified + unverified) * MAX_UNVERIFIED_PERCENT {
        return None;
    }
    Some(kept)
}

/// Cut the kept lines at whole lines to `budget` characters, marking the cut.
fn fit_budget(kept: Vec<String>, budget: usize) -> Vec<String> {
    let mut used = 0usize;
    let mut fitted = Vec::with_capacity(kept.len());
    for line in kept {
        let cost = line.chars().count() + 1;
        if used + cost > budget {
            fitted.push(format!("{OMITTED_MARK} digest cut at its {budget}-character budget"));
            break;
        }
        used += cost;
        fitted.push(line);
    }
    fitted
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;

    use serde_json::json;

    use super::{
        bounded_view, digest_with, is_digestible, DigestHealth, Digester, ToolDigestMode,
        DIGEST_INPUT_CAP_CHARS, GIVE_UP_AFTER_FAILURES, OMITTED_MARK,
    };

    struct Scripted(Result<&'static str, &'static str>);

    impl Digester for Scripted {
        fn model(&self) -> &'static str {
            "fast-model"
        }
        fn reply(&self, _system: &[String], _user: &str, _max_tokens: u32) -> Result<String, String> {
            self.0.map(str::to_string).map_err(str::to_string)
        }
    }

    const LOG: &str = "running 3 tests\ntest a ... ok\ntest b ... FAILED\ntest c ... ok\n\nfailures:\n\n---- b stdout ----\nassertion failed: left == right\n  left: 1\n right: 2\n\ntest result: FAILED. 2 passed; 1 failed\n";

    #[test]
    fn only_the_listed_tools_are_folded_and_off_is_the_only_off() {
        assert!(is_digestible("bash") && is_digestible("grep_search"));
        assert!(!is_digestible("read_file") && !is_digestible("retrieve_tool_output"));
        assert_eq!(ToolDigestMode::from_setting(Some("off")), ToolDigestMode::Off);
        assert_eq!(ToolDigestMode::from_setting(Some(" OFF ")), ToolDigestMode::Off);
        assert_eq!(ToolDigestMode::from_setting(None), ToolDigestMode::OverCap);
        assert_eq!(ToolDigestMode::from_setting(Some("over-cap")), ToolDigestMode::OverCap);
    }

    /// The happy path: the fast model returns the failing lines and a marker,
    /// and the digest is those lines under a header naming what was read.
    #[test]
    fn verbatim_lines_and_markers_come_back_under_a_header() {
        let health = DigestHealth::new();
        let reply = "test b ... FAILED\n⋯ 5 lines omitted\nassertion failed: left == right\n  left: 1\n right: 2\ntest result: FAILED. 2 passed; 1 failed\n";
        let digest = digest_with(
            &Scripted(Ok(reply)),
            &health,
            "bash",
            &json!({"command": "cargo test"}),
            LOG,
            16_384,
        )
        .expect("digest");
        let mut lines = digest.lines();
        assert_eq!(
            lines.next().unwrap(),
            format!(
                "[digest] fast-model read {} lines ({} chars) of bash output and kept 5 verbatim; ⋯ marks omitted runs.",
                LOG.lines().count(),
                LOG.chars().count()
            )
        );
        assert_eq!(lines.next().unwrap(), "test b ... FAILED");
        assert_eq!(lines.next().unwrap(), "⋯ 5 lines omitted");
        assert!(digest.ends_with("test result: FAILED. 2 passed; 1 failed"));
        assert!(!health.given_up());
    }

    /// A reply that paraphrases is a summary the model cannot trust; it is
    /// refused and the ordinary cut stands. One stray line among many
    /// verbatim ones is dropped, not fatal.
    #[test]
    fn a_paraphrase_is_refused_and_a_stray_line_is_dropped() {
        let health = DigestHealth::new();
        let paraphrase = "One test failed: b, because 1 != 2.\nEverything else passed.";
        assert_eq!(
            digest_with(&Scripted(Ok(paraphrase)), &health, "bash", &json!({}), LOG, 16_384),
            None
        );
        let mostly_verbatim = "running 3 tests\ntest a ... ok\ntest b ... FAILED\ntest c ... ok\nfailures:\n---- b stdout ----\nassertion failed: left == right\n  left: 1\n right: 2\ntest result: FAILED. 2 passed; 1 failed\n(summary: b failed)";
        let digest = digest_with(&Scripted(Ok(mostly_verbatim)), &health, "bash", &json!({}), LOG, 16_384)
            .expect("ten verbatim lines carry one stray");
        assert!(!digest.contains("(summary: b failed)"));
        assert!(digest.contains("kept 10 verbatim"));
    }

    /// The budget is half the tool's ceiling and is enforced at a line.
    #[test]
    fn the_digest_is_cut_at_whole_lines_to_half_the_ceiling() {
        let health = DigestHealth::new();
        let digest = digest_with(&Scripted(Ok(LOG)), &health, "bash", &json!({}), LOG, 100)
            .expect("digest");
        let body: Vec<&str> = digest.lines().skip(1).collect();
        assert!(body.last().unwrap().starts_with(&format!("{OMITTED_MARK} digest cut at its 50-character budget")));
        let kept_chars: usize = body[..body.len() - 1].iter().map(|line| line.chars().count() + 1).sum();
        assert!(kept_chars <= 50, "{kept_chars}");
    }

    /// Failures count, three in a row give up, and one success resets.
    #[test]
    fn three_failures_in_a_row_give_up_and_a_success_resets() {
        let health = DigestHealth::new();
        for _ in 0..GIVE_UP_AFTER_FAILURES {
            assert!(!health.given_up());
            assert_eq!(digest_with(&Scripted(Err("boom")), &health, "bash", &json!({}), LOG, 16_384), None);
        }
        assert!(health.given_up());
        health.note_success();
        assert!(!health.given_up());
    }

    /// Live probe, ignored by default: dials the real fast model for the
    /// session provider and prints what came back, so a silent fallback in
    /// production can be reproduced by hand:
    /// `cargo test -p tools --lib -- --ignored live_digest --nocapture`.
    #[test]
    #[ignore = "dials the provider with the machine's credentials"]
    fn live_digest_probe() {
        let health = DigestHealth::new();
        let parent = std::env::var("ZO_DIGEST_PROBE_PARENT").ok();
        let digester = match super::ProviderDigester::for_parent(parent.as_deref()) {
            Ok(digester) => digester,
            Err(error) => panic!("no digester: {error}"),
        };
        eprintln!("digester model: {}", digester.model());
        // A test log with the one failure buried mid-stream: the shape the
        // blind head+tail cut loses and the fold must keep.
        let mut output = String::from("running 3000 tests\n");
        for n in 1..=3000 {
            if n == 1_487 {
                output.push_str("test conversation::tests::plan_reminder_is_reanchored ... FAILED\n");
            } else {
                let _ = writeln!(output, "test module_{}::case_{n} ... ok", n % 17);
            }
        }
        output.push_str("\nfailures:\n\n---- conversation::tests::plan_reminder_is_reanchored stdout ----\nassertion `left == right` failed\n  left: 2\n right: 1\n\ntest result: FAILED. 2999 passed; 1 failed; 0 ignored\n");
        let input = json!({"command": "cargo test -p runtime"});
        let digest = digest_with(&digester, &health, "bash", &input, &output, 16_384);
        eprintln!("digest: {digest:?}");
        assert!(digest.is_some());
    }

    /// An output past the input cap reaches the digester as head and tail
    /// with a marker naming the hidden lines; one under it goes whole.
    #[test]
    fn a_huge_output_is_shown_as_head_and_tail() {
        assert!(matches!(bounded_view("small"), std::borrow::Cow::Borrowed(_)));
        let line = "0123456789abcdef\n";
        let huge = line.repeat(DIGEST_INPUT_CAP_CHARS / line.len() + 1_000);
        let shown = bounded_view(&huge);
        assert!(shown.chars().count() < huge.chars().count());
        let marker = shown.lines().find(|l| l.starts_with(OMITTED_MARK)).expect("marker");
        let hidden: usize = marker.split_whitespace().nth(1).unwrap().parse().unwrap();
        assert!(hidden > 900 && hidden < 1_100, "{hidden}");
        assert!(shown.starts_with(line) && shown.ends_with(line));
    }
}
