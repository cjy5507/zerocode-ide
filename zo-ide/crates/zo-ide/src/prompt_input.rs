//! `zo --prompt-input` — what the model is shown before a turn exists.
//!
//! The r15 architecture made the fixed harness — system core, advertised tool
//! schemas, skill index, and idle reminders — one reproducible budget. r49
//! closes that budget and keeps the per-section report so later growth cannot
//! silently spend it again.
//!
//! Two honesty rules this module keeps.
//!
//! 1. **The count is an estimate and says so.** It is `chars / 4`, the same
//!    estimator the runtime's own overflow guard uses
//!    (`runtime::conversation::helpers::estimate_system_prompt_tokens`) — so
//!    the report and the guard can never disagree about the same prompt. It is
//!    NOT a provider tokenizer, and §2.4's gate still demands one before the
//!    comparison against CC and codex is quoted as a product claim.
//! 2. **Sections name themselves.** Each system section is labelled by its own
//!    first line rather than by a list kept here, so the report cannot drift
//!    away from the prompt it is measuring.

use std::fmt::Write as _;

use api::ToolDefinition;

/// Where a piece of the fixed prefix sits in the r15 budget (§2.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Bucket {
    System,
    Skills,
    Tools,
    Reminders,
}

impl Bucket {
    const fn label(self) -> &'static str {
        match self {
            Self::System => "System core",
            Self::Skills => "Skill index",
            Self::Tools => "Tool schemas",
            Self::Reminders => "Reminders",
        }
    }

    /// The r15 per-bucket ceiling (`architecture-r15.md` §2.2).
    const fn budget_tokens(self) -> u64 {
        match self {
            Self::System => 5_500,
            // 2,000 until t-5629, and moved by the seven tokens two deferred
            // names cost — `skill_search` and `skill_load` in the manifest,
            // which is billed here because it is a tool-plane cost. The bucket
            // stood at 1,998 with two tokens of head, and there was no way to
            // pay from inside it: their SCHEMAS are deferred and cost nothing,
            // the names cannot be shortened, and the only other prose in reach
            // is another tool's own one-line advertisement.
            //
            // What the seven buy, and why this is not the growth r49 forbids:
            // they are not a schema on the wire — the wire is still twelve —
            // and on any machine with a skill catalog the same fixed harness
            // loses far more than it gains, because the `# Available skills`
            // index stops being rendered. Measured on this machine's
            // eighteen-skill catalog (2026-09-21): the skills bucket falls
            // 878 → 178 and the whole fixed harness 7,452 → 6,789. The
            // shipped harness carries no machine skills by design
            // (`harness_budget`), so the floor it measures is the one case
            // that pays the seven and collects nothing — which is the honest
            // number to hold this to, and it is seven.
            Self::Tools => 2_010,
            // The renderer's own ceiling (`runtime::SKILL_INDEX_BUDGET_TOKENS`):
            // the index folds its tail to stay under it, so the gate and the
            // prompt ask one number.
            Self::Skills => runtime::SKILL_INDEX_BUDGET_TOKENS as u64,
            // p95, not the 800 hard cap: the idle harness is the p95 case.
            Self::Reminders => 300,
        }
    }

    /// Display order — the order the pieces sit in on the wire.
    const ORDER: [Self; 4] = [Self::System, Self::Tools, Self::Skills, Self::Reminders];
}

/// Sum of the four bucket budgets: the cold fixed harness target (§2.2).
pub const FIXED_HARNESS_BUDGET_TOKENS: u64 = 8_700;

/// Wire tool schemas the attainable r49 architecture allows. r15's eight-tool
/// sketch remains an experiment; twelve keeps every pinned coding affordance.
pub const WIRE_TOOL_BUDGET: usize = 12;

/// First line of the skills section. Taken from the runtime rather than
/// copied: if the skill index re-buckets into System core because a heading
/// was renamed in one crate and not the other, BOTH budget rows lie and
/// nothing says so.
const SKILLS_HEADING: &str = runtime::SKILLS_INDEX_HEADING;

/// First line of `tools::deferred_tool_manifest_section` — a tool-plane cost
/// even though it travels inside the system prompt, so it is billed to Tools.
const DEFERRED_TOOLS_HEADING: &str = "# Deferred tools";

/// First line of the reminder block. Taken from the runtime for the same reason
/// [`SKILLS_HEADING`] is: a block billed to System core because a heading was
/// renamed in one crate and not the other makes BOTH budget rows lie, and the
/// Reminders row — the one this heading feeds — is the row that reads `0 / 300`
/// when nothing is counting.
const REMINDERS_HEADING: &str = runtime::REMINDERS_SECTION_HEADING;

/// One measured piece of the fixed prefix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Piece {
    pub bucket: Bucket,
    /// The piece's own first line, trimmed — never a name kept in this file.
    pub label: String,
    pub chars: usize,
    pub est_tokens: u64,
    pub digest: u64,
}

/// The provider's own count of this harness, split where it matters.
///
/// Two counts rather than one because the `chars / 4` estimate is not wrong by
/// a single factor: prose tokenizes near 4 chars/token and a JSON tool schema
/// nowhere near it, so a blended error hides which of the four budgets is
/// actually mis-set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderCount {
    /// Exact input tokens for system + tools + a one-character user message.
    pub total: u32,
    /// The same request with the tool block removed. `None` when the second
    /// count could not be taken.
    pub without_tools: Option<u32>,
}

impl ProviderCount {
    /// Exact tokens the wire tool block costs.
    #[must_use]
    pub fn tools(self) -> Option<u32> {
        self.total.checked_sub(self.without_tools?)
    }
}

/// The cold fixed harness of one session, measured.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptInput {
    /// Provider family selected for this measurement.
    pub provider_family: String,
    pub model: String,
    pub workspace: String,
    pub pieces: Vec<Piece>,
    /// Number of tool schemas actually advertised on the wire.
    pub wire_tools: usize,
    /// What the provider says this harness actually costs, when it was asked.
    ///
    /// Every budget in this repo is written in `chars / 4`, which is a guard's
    /// unit, not a gate's. `architecture-r15.md` §2.4 will not let the token
    /// claim ship on an approximation, so the report carries the provider's own
    /// number next to the estimate whenever it can get one — and the gap between
    /// them is the calibration nobody had.
    pub provider_tokens: Option<ProviderCount>,
}

impl PromptInput {
    /// Measure the pieces a fresh request would carry.
    ///
    /// `system` is the assembled system prompt sections, `tools` the wire
    /// advertisement (`filter_tool_specs`), `reminders` the transient system
    /// reminders standing at rest.
    #[must_use]
    pub fn measure(
        model: &str,
        workspace: &str,
        system: &[String],
        tools: &[ToolDefinition],
        reminders: &[String],
    ) -> Self {
        let mut pieces = Vec::with_capacity(system.len() + tools.len() + reminders.len());
        for section in system {
            let label = first_line(section);
            let bucket = if label == SKILLS_HEADING {
                Bucket::Skills
            } else if label == DEFERRED_TOOLS_HEADING {
                Bucket::Tools
            } else if label == REMINDERS_HEADING {
                Bucket::Reminders
            } else {
                Bucket::System
            };
            pieces.push(piece(bucket, label, section));
        }
        for tool in tools {
            // What travels is the serialized definition, so that — not the
            // description alone — is what gets counted.
            let json = serde_json::to_string(&serde_json::json!({
                "name": tool.name,
                "description": tool.description,
                "input_schema": tool.input_schema,
            }))
            .unwrap_or_default();
            pieces.push(piece(Bucket::Tools, format!("tool {}", tool.name), &json));
        }
        for reminder in reminders {
            pieces.push(piece(Bucket::Reminders, first_line(reminder), reminder));
        }
        Self {
            provider_family: api::detect_provider_kind(model)
                .rate_limit_key()
                .to_string(),
            model: model.to_string(),
            workspace: workspace.to_string(),
            pieces,
            wire_tools: tools.len(),
            provider_tokens: None,
        }
    }

    /// Attach the provider's exact input-token count for this harness.
    #[must_use]
    pub fn with_provider_tokens(mut self, tokens: Option<ProviderCount>) -> Self {
        self.provider_tokens = tokens;
        self
    }

    /// How far the `chars / 4` estimate is from the provider's count, in
    /// percent of the provider's number. Positive means the estimate is HIGH.
    #[must_use]
    pub fn estimate_error_percent(&self) -> Option<i64> {
        error_percent(self.fixed_harness_tokens(), self.provider_tokens?.total)
    }

    /// The same gap for the wire tool block alone.
    #[must_use]
    pub fn tool_estimate_error_percent(&self) -> Option<i64> {
        error_percent(
            self.bucket_tokens(Bucket::Tools),
            self.provider_tokens?.tools()?,
        )
    }

    #[must_use]
    pub fn bucket_tokens(&self, bucket: Bucket) -> u64 {
        self.pieces
            .iter()
            .filter(|piece| piece.bucket == bucket)
            .map(|piece| piece.est_tokens)
            .sum()
    }

    #[must_use]
    pub fn fixed_harness_tokens(&self) -> u64 {
        self.pieces.iter().map(|piece| piece.est_tokens).sum()
    }

    /// One digest over the whole fixed prefix, in the ledger's own hash
    /// vocabulary. Two sessions that print the same digest are shown the same
    /// harness; a session whose digest moves mid-run has broken the
    /// byte-stability invariant (`architecture-r15.md` §1 invariants 3 and 4).
    #[must_use]
    pub fn digest(&self) -> u64 {
        let mut hash = FNV_OFFSET_BASIS;
        for piece in &self.pieces {
            for byte in piece.digest.to_le_bytes() {
                hash ^= u64::from(byte);
                hash = hash.wrapping_mul(FNV_PRIME);
            }
        }
        hash
    }

    /// Every fixed-harness ceiling this build is over, as sentences. Empty is
    /// the CI gate's definition of an in-budget harness.
    #[must_use]
    pub fn overruns(&self) -> Vec<String> {
        let mut over = Vec::new();
        for bucket in Bucket::ORDER {
            let tokens = self.bucket_tokens(bucket);
            let budget = bucket.budget_tokens();
            if tokens > budget {
                over.push(format!(
                    "{} {tokens} > {budget} (over by {})",
                    bucket.label(),
                    tokens - budget
                ));
            }
        }
        if self.wire_tools > WIRE_TOOL_BUDGET {
            over.push(format!(
                "wire tool schemas {} > {WIRE_TOOL_BUDGET}",
                self.wire_tools
            ));
        }
        let total = self.fixed_harness_tokens();
        if total > FIXED_HARNESS_BUDGET_TOKENS {
            over.push(format!(
                "fixed harness {total} > {FIXED_HARNESS_BUDGET_TOKENS} (over by {})",
                total - FIXED_HARNESS_BUDGET_TOKENS
            ));
        }
        over
    }

    /// The report, in the `/status` two-column shape.
    #[must_use]
    pub fn render(&self) -> String {
        let total = self.fixed_harness_tokens();
        let mut out = String::new();
        let _ = writeln!(out, "Prompt input");
        let _ = writeln!(out, "  {:<18}{}", "Provider family", self.provider_family);
        let _ = writeln!(out, "  {:<18}{}", "Model", self.model);
        let _ = writeln!(out, "  {:<18}{}", "Workspace", self.workspace);
        let _ = writeln!(
            out,
            "  {:<18}chars/4 estimate — not a provider tokenizer",
            "Counting"
        );
        let _ = writeln!(out, "  {:<18}v1-{:016x}", "Digest", self.digest());
        if let (Some(exact), Some(error)) = (self.provider_tokens, self.estimate_error_percent()) {
            let _ = writeln!(
                out,
                "  {:<18}{} tokens — the estimate below is {}",
                "Provider count",
                thousands(u64::from(exact.total)),
                signed_percent(error)
            );
            if let (Some(tool_tokens), Some(tool_error)) =
                (exact.tools(), self.tool_estimate_error_percent())
            {
                let _ = writeln!(
                    out,
                    "  {:<18}{} of that is the tool block — estimated {}",
                    "",
                    thousands(u64::from(tool_tokens)),
                    signed_percent(tool_error)
                );
            }
        }
        let _ = writeln!(out);
        let _ = writeln!(
            out,
            "  {:<18}{} tokens ({}% of the {} budget)",
            "Fixed harness",
            thousands(total),
            total.saturating_mul(100) / FIXED_HARNESS_BUDGET_TOKENS,
            thousands(FIXED_HARNESS_BUDGET_TOKENS)
        );
        for bucket in Bucket::ORDER {
            let tokens = self.bucket_tokens(bucket);
            let budget = bucket.budget_tokens();
            let count = self
                .pieces
                .iter()
                .filter(|piece| piece.bucket == bucket)
                .count();
            let verdict = if tokens > budget {
                format!("OVER by {}", thousands(tokens - budget))
            } else {
                format!("{} left", thousands(budget - tokens))
            };
            let _ = writeln!(
                out,
                "    {:<16}{:>7} / {:<7}  {:<16}{count} {}",
                bucket.label(),
                thousands(tokens),
                thousands(budget),
                verdict,
                if count == 1 { "piece" } else { "pieces" }
            );
        }
        let _ = writeln!(
            out,
            "    {:<16}{:>7} / {:<7}  {}",
            "Wire schemas",
            self.wire_tools,
            WIRE_TOOL_BUDGET,
            if self.wire_tools > WIRE_TOOL_BUDGET {
                "OVER"
            } else {
                "ok"
            }
        );
        let _ = writeln!(out);
        let _ = writeln!(out, "  Pieces (largest first)");
        let mut ordered: Vec<&Piece> = self.pieces.iter().collect();
        ordered.sort_by(|left, right| {
            right
                .est_tokens
                .cmp(&left.est_tokens)
                .then_with(|| left.label.cmp(&right.label))
        });
        for piece in ordered {
            let _ = writeln!(
                out,
                "    {:>6}  {:<12}{:08x}  {}",
                thousands(piece.est_tokens),
                bucket_tag(piece.bucket),
                // Low 32 bits: a short, eye-comparable tag, not an id.
                piece.digest & 0xffff_ffff,
                truncate(&piece.label, 68)
            );
        }
        out
    }
}

const fn bucket_tag(bucket: Bucket) -> &'static str {
    match bucket {
        Bucket::System => "system",
        Bucket::Skills => "skills",
        Bucket::Tools => "tools",
        Bucket::Reminders => "reminder",
    }
}

fn piece(bucket: Bucket, label: impl Into<String>, body: &str) -> Piece {
    let chars = body.chars().count();
    Piece {
        bucket,
        label: label.into(),
        chars,
        // Matches `estimate_system_prompt_tokens` exactly — the report must
        // never disagree with the guard that acts on the same prompt.
        est_tokens: (chars / 4 + 1) as u64,
        digest: stable_hash(body.as_bytes()),
    }
}

fn first_line(section: &str) -> String {
    section.lines().next().unwrap_or_default().trim().to_string()
}

fn truncate(value: &str, limit: usize) -> String {
    if value.chars().count() <= limit {
        return value.to_string();
    }
    let head: String = value.chars().take(limit.saturating_sub(1)).collect();
    format!("{head}…")
}

/// `-31` → `-31%`, `4` → `+4%`. The sign is the point: a reader must be able
/// to tell "we are guessing high" from "we are guessing low" at a glance.
fn signed_percent(value: i64) -> String {
    format!("{}{value}%", if value >= 0 { "+" } else { "" })
}

/// `(estimated - exact) / exact` as a percentage. Positive means the estimate
/// is HIGH. `None` when there is nothing to divide by.
fn error_percent(estimated: u64, exact: u32) -> Option<i64> {
    let exact = i64::from(exact);
    if exact == 0 {
        return None;
    }
    Some((i64::try_from(estimated).ok()? - exact) * 100 / exact)
}

/// `1234567` → `1,234,567`. Shared with the doctor's cache rows: the two
/// report the same ledger and should not disagree about how a number looks.
#[must_use]
pub fn thousands(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, ch) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

// FNV-1a 64, the same construction the cache ledger fingerprints requests with
// (`api::prompt_cache::stable_hash_bytes`). Digests here are change detectors,
// not security, and sharing the vocabulary keeps one hash idiom in the product.
const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

fn stable_hash(bytes: &[u8]) -> u64 {
    let mut hash = FNV_OFFSET_BASIS;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::{
        Bucket, PromptInput, ProviderCount, DEFERRED_TOOLS_HEADING, REMINDERS_HEADING,
        SKILLS_HEADING,
    };
    use api::ToolDefinition;
    use serde_json::json;

    fn tool(name: &str, description: &str) -> ToolDefinition {
        ToolDefinition {
            name: name.to_string(),
            description: Some(description.to_string()),
            input_schema: json!({"type": "object", "properties": {}}),
        }
    }

    fn measured() -> PromptInput {
        PromptInput::measure(
            "claude-opus-5",
            "/w",
            &[
                "# Zo\nyou are a coding agent".to_string(),
                format!("{SKILLS_HEADING}\n- `a`: does a\n- `b`: does b"),
                format!("{DEFERRED_TOOLS_HEADING}\nAlpha, Beta"),
            ],
            &[tool("Read", "read a file")],
            &["<system-reminder>todo</system-reminder>".to_string()],
        )
    }

    /// The skills section and the deferred manifest are tool/skill costs even
    /// though both ride inside the system prompt. Billing them to System core
    /// would let the skill index grow behind a budget that never moves.
    #[test]
    fn sections_are_billed_by_their_own_heading() {
        let input = measured();
        let bucket = |label: &str| {
            input
                .pieces
                .iter()
                .find(|piece| piece.label == label)
                .map(|piece| piece.bucket)
        };
        assert_eq!(bucket(SKILLS_HEADING), Some(Bucket::Skills));
        assert_eq!(bucket(DEFERRED_TOOLS_HEADING), Some(Bucket::Tools));
        assert_eq!(bucket("# Zo"), Some(Bucket::System));
        assert_eq!(bucket("tool Read"), Some(Bucket::Tools));
    }

    /// The Reminders row read `0 / 300` for as long as there was nothing to
    /// count. A reminder block billed to System core would keep it reading
    /// zero while the harness grew — the exact drift this report exists to
    /// catch.
    #[test]
    fn a_reminder_block_is_billed_to_reminders_not_to_system_core() {
        let block = runtime::render_reminders(
            &[runtime::ReminderCandidate {
                slug: "gotcha-zsh-pipe".to_string(),
                lesson: "check `$?` without a pipe".to_string(),
            }],
            &[],
        )
        .expect("a candidate renders a block");
        let input = PromptInput::measure(
            "m",
            "/w",
            &["# Zo\nyou are a coding agent.".to_string(), block],
            &[],
            &[],
        );
        assert_eq!(
            input
                .pieces
                .iter()
                .find(|piece| piece.label == REMINDERS_HEADING)
                .map(|piece| piece.bucket),
            Some(Bucket::Reminders)
        );
        assert!(
            input.bucket_tokens(Bucket::Reminders) > 0,
            "the row must count what is actually there"
        );
        assert!(
            input.bucket_tokens(Bucket::Reminders) <= Bucket::Reminders.budget_tokens(),
            "and one block must fit inside the row's own ceiling"
        );
    }

    /// The estimate is `chars / 4 + 1` per piece, the runtime's own
    /// (`estimate_system_prompt_tokens`). Drifting from it would make the
    /// report and the overflow guard disagree about one prompt.
    #[test]
    fn the_estimate_is_the_runtimes_own() {
        let input = PromptInput::measure("m", "/w", &["12345678".to_string()], &[], &[]);
        assert_eq!(input.fixed_harness_tokens(), 3);
        assert_eq!(input.pieces[0].chars, 8);
    }

    /// A tool's cost is its serialized definition, not its description: the
    /// schema body is usually the larger half and it is what is on the wire.
    #[test]
    fn a_tools_cost_counts_its_schema_not_just_its_description() {
        let bare = PromptInput::measure("m", "/w", &[], &[tool("R", "x")], &[]);
        let fat = PromptInput::measure(
            "m",
            "/w",
            &[],
            &[ToolDefinition {
                name: "R".to_string(),
                description: Some("x".to_string()),
                input_schema: json!({"type": "object", "properties": {
                    "path": {"type": "string", "description": "a long description of the path"}
                }}),
            }],
            &[],
        );
        assert!(
            fat.fixed_harness_tokens() > bare.fixed_harness_tokens(),
            "schema body must be billed"
        );
    }

    #[test]
    fn overruns_name_every_ceiling_that_broke() {
        let fat = "x".repeat(40_000);
        let wire_tools = super::WIRE_TOOL_BUDGET + 1;
        let input = PromptInput::measure(
            "m",
            "/w",
            &[fat.clone(), format!("{SKILLS_HEADING}\n{fat}")],
            &(0..wire_tools)
                .map(|i| tool(&format!("T{i}"), &fat))
                .collect::<Vec<_>>(),
            &[fat],
        );
        let overruns = input.overruns().join(" | ");
        for expected in [
            "System core",
            "Skill index",
            "Tool schemas",
            "Reminders",
            "fixed harness",
        ] {
            assert!(overruns.contains(expected), "missing {expected} in {overruns}");
        }
        assert!(
            overruns.contains(&format!("wire tool schemas {wire_tools}")),
            "missing wire overrun in {overruns}"
        );
        assert!(measured().overruns().is_empty(), "a small harness is inside budget");
    }

    /// The digest is what tells a session its harness moved mid-run. It must
    /// react to content and to order, or invariant 3/4 drift reads as stable.
    #[test]
    fn the_digest_moves_with_content_and_order() {
        let base = measured();
        let edited = PromptInput::measure(
            "claude-opus-5",
            "/w",
            &[
                "# Zo\nyou are a coding agent.".to_string(),
                format!("{SKILLS_HEADING}\n- `a`: does a\n- `b`: does b"),
                format!("{DEFERRED_TOOLS_HEADING}\nAlpha, Beta"),
            ],
            &[tool("Read", "read a file")],
            &["<system-reminder>todo</system-reminder>".to_string()],
        );
        assert_ne!(base.digest(), edited.digest());
        let reordered = PromptInput::measure(
            "claude-opus-5",
            "/w",
            &[
                format!("{SKILLS_HEADING}\n- `a`: does a\n- `b`: does b"),
                "# Zo\nyou are a coding agent".to_string(),
                format!("{DEFERRED_TOOLS_HEADING}\nAlpha, Beta"),
            ],
            &[tool("Read", "read a file")],
            &["<system-reminder>todo</system-reminder>".to_string()],
        );
        assert_ne!(base.digest(), reordered.digest());
        assert_eq!(base.digest(), measured().digest(), "same harness, same digest");
    }

    /// The report must not quietly become a provider-tokenizer claim: §2.4's
    /// release gate still requires one, and a reader who sees a bare number
    /// will assume it is exact.
    #[test]
    fn the_report_says_the_count_is_an_estimate() {
        let rendered = measured().render();
        assert!(rendered.contains("Provider family   anthropic"), "{rendered}");
        assert!(rendered.contains("Model             claude-opus-5"), "{rendered}");
        assert!(rendered.contains("not a provider tokenizer"), "{rendered}");
        assert!(rendered.contains("Fixed harness"), "{rendered}");
    }

    /// The calibration is the whole reason the provider is asked, so the
    /// arithmetic behind it must be pinned: which way the sign points, and
    /// that the tool block is derived by SUBTRACTION rather than guessed.
    ///
    /// Measured 2026-08-29 against `/v1/messages/count_tokens`: 20,642 exact
    /// vs 14,265 estimated (-30%), of which 7,918 exact vs 5,654 estimated
    /// (-28%) is the tool block. So `chars / 4` runs about 30% LOW and does so
    /// fairly uniformly — prose and JSON schema alike, which is the opposite of
    /// what a "JSON tokenizes denser" hunch predicts. Every budget in this repo
    /// is written in the estimate's unit, so the RATIOS it reports hold and the
    /// absolute figures do not.
    #[test]
    fn the_calibration_reports_the_gap_and_its_direction() {
        let mut input = measured();
        input.provider_tokens = Some(super::ProviderCount {
            total: 20_642,
            without_tools: Some(12_724),
        });
        assert_eq!(input.provider_tokens.and_then(ProviderCount::tools), Some(7_918));

        // A low estimate reads negative, a high one positive — the sign is what
        // a reader acts on.
        assert_eq!(super::error_percent(14_265, 20_642), Some(-30));
        assert_eq!(super::error_percent(20_642, 14_265), Some(44));
        assert_eq!(super::error_percent(100, 0), None, "nothing to divide by");
        assert_eq!(super::signed_percent(-30), "-30%");
        assert_eq!(super::signed_percent(4), "+4%");

        // A single count still reports the total; only the split goes missing.
        input.provider_tokens = Some(super::ProviderCount {
            total: 20_642,
            without_tools: None,
        });
        assert!(input.estimate_error_percent().is_some());
        assert!(input.tool_estimate_error_percent().is_none());
        let rendered = input.render();
        assert!(rendered.contains("Provider count"), "{rendered}");
        assert!(
            !rendered.contains("of that is the tool block"),
            "a missing split must not be rendered as a zero: {rendered}"
        );
    }

    /// With no provider count the report is exactly what it was before — a
    /// diagnostic must not require the network to be useful.
    #[test]
    fn the_report_stands_without_a_provider_count() {
        let rendered = measured().render();
        assert!(!rendered.contains("Provider count"), "{rendered}");
        assert!(rendered.contains("not a provider tokenizer"), "{rendered}");
        assert!(measured().estimate_error_percent().is_none());
    }

    /// The deferred manifest heading IS copied here (the tools crate renders
    /// the section as one string, with no exported heading), so hold the
    /// product to it — otherwise the manifest quietly re-buckets into System
    /// core and the tool budget stops seeing its own cost.
    #[test]
    fn the_deferred_manifest_heading_still_matches_the_tools_crate() {
        assert!(
            tools::deferred_tool_manifest_section().starts_with(DEFERRED_TOOLS_HEADING),
            "deferred manifest heading moved"
        );
    }
}
