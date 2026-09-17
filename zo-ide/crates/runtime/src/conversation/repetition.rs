//! Tool-call repetition detection for [`ConversationRuntime`], split out of
//! `mod.rs` so the turn loops there read as orchestration. Behaviour-preserving:
//! these were module-level helpers and `ConversationRuntime` methods, now with
//! `pub(super)`/`pub(crate)` visibility so the loops in `mod.rs` (and the tests)
//! still reach them.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use crate::permissions::PermissionOutcome;
use crate::session::ContentBlock;

use super::Attendance;
use super::{
    is_concurrency_safe, is_edit_or_write_tool, ApiClient, ConversationRuntime, ToolExecutor,
    TurnContinuity, TurnStopSignal,
};

/// Per-turn count of identical (normalized) tool calls that triggers one
/// repetition advisory. A per-turn tally — not a rolling window — so fan-out
/// width cannot starve it: the old 6-deep window could never reach 3 when each
/// turn round issued 4+ distinct calls (a `git diff` plus several reads).
pub(super) const TOOL_REPETITION_THRESHOLD: usize = 3;

/// Per-turn count of identical (normalized) tool calls that HARD-STOPS the turn.
/// The soft advisory at [`TOOL_REPETITION_THRESHOLD`] must first be delivered in
/// a tool result; only a later model-request batch may hard-stop. The count is
/// still one more than the advisory threshold, but [`Self::arm_tool_repetition_hard_stops`]
/// prevents another identical call in the same assistant-emitted tool batch from
/// being treated as ignoring advice the model has not seen yet.
pub(crate) const TOOL_REPETITION_HARD_STOP: usize = TOOL_REPETITION_THRESHOLD + 1;

/// Cross-turn count of identical (normalized) tool calls that fires a one-time
/// cross-turn advisory. Unlike [`TOOL_REPETITION_THRESHOLD`] this tally is NOT
/// reset at turn start, so it catches a no-progress re-read loop that spans turn
/// boundaries (one re-read per auto-continued turn) — exactly the loop the
/// per-turn guard cannot see because its counter is cleared every turn.
pub(crate) const TOOL_REPETITION_CROSS_TURN_ADVISE: usize = 3;

/// Cross-turn count of identical tool calls that HARD-STOPS the turn. The
/// advisory fires at [`TOOL_REPETITION_CROSS_TURN_ADVISE`]; one more identical
/// call across a later turn means the model stayed in the same no-progress loop.
pub(crate) const TOOL_REPETITION_CROSS_TURN_HARD_STOP: usize = TOOL_REPETITION_CROSS_TURN_ADVISE + 1;

/// How many single-read batches in a row draw the one nudge of a turn.
/// Three: two could be a read that decided the next; three one at a time is
/// a habit, and the third result is where the model reads the reminder.
pub(crate) const SERIAL_READS_NUDGE_AT: u8 = 3;

/// The nudge itself — one sentence, once a turn, on the tool result.
pub(crate) const SERIAL_READS_NUDGE: &str = "<system-reminder>The last three tool batches each carried a single read-only call. Reads, searches and lookups whose inputs do not depend on one another belong in ONE message so they run at once — batch the next ones.</system-reminder>";

/// Outcome of recording one tool call for repetition detection.
#[derive(Debug)]
pub(crate) enum ToolRepetition {
    /// Not repeated enough to act on this turn.
    Ok,
    /// One-time soft advisory (at [`TOOL_REPETITION_THRESHOLD`]) to fold into the
    /// tool result, nudging the model to change approach.
    Advise(String),
    /// Hard stop (at [`TOOL_REPETITION_HARD_STOP`]): the loop should not keep
    /// repeating this call. Carries a notice to fold into the tool result naming
    /// the loop. `terminates` controls whether the surrounding turn also ends:
    ///
    /// - `true` for mutations, error results, and cross-turn no-progress loops.
    ///   Those cannot make progress by definition, so the turn ends after the
    ///   batch.
    /// - `false` for the first successful read-only `read_file` exact repeat: a
    ///   later identical call is skipped in the same armed batch, but a normal
    ///   workflow that re-reads a short or truncated file gets one model round
    ///   to recover. Ignoring that notice and repeating again ends the turn.
    HardStop { notice: String, terminates: bool },
}
/// Stable fingerprint of a `(tool_name, input)` pair. Uses `DefaultHasher`
/// (fixed-seed, deterministic within a process — the same choice `prompt.rs`
/// makes) so a re-issued identical call hashes equal. A delimiter byte keeps
/// `("ab","c")` distinct from `("a","bc")`.
pub(super) fn fingerprint_tool_call(tool_name: &str, input: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    tool_name.hash(&mut hasher);
    0xFFu8.hash(&mut hasher);
    normalized_fingerprint_input(tool_name, input).hash(&mut hasher);
    hasher.finish()
}

/// Tools whose JSON object input should be key-order normalized for repetition
/// detection. These tools often come from model-emitted JSON, so semantically
/// identical calls may differ only by key order. Keep `offset`/`limit` intact:
/// adjacent or later windows over the same file/artifact are real exploration
/// progress, and collapsing them makes normal source reading look like a
/// no-progress loop.
fn tool_normalizes_json_input(tool_name: &str) -> bool {
    matches!(tool_name, "read_file" | "retrieve_output")
}

/// Canonicalize a tool input before fingerprinting. For selected JSON-object
/// tools, sort keys so equivalent inputs hash together while preserving paging
/// fields (`offset`/`limit`) so different line windows remain distinct. Every
/// other tool — and any input that is not a JSON object — hashes verbatim.
fn normalized_fingerprint_input<'a>(tool_name: &str, input: &'a str) -> std::borrow::Cow<'a, str> {
    use std::borrow::Cow;
    if !tool_normalizes_json_input(tool_name) {
        return Cow::Borrowed(input);
    }
    let Ok(serde_json::Value::Object(map)) = serde_json::from_str::<serde_json::Value>(input) else {
        return Cow::Borrowed(input);
    };
    let sorted: std::collections::BTreeMap<String, serde_json::Value> = map.into_iter().collect();
    serde_json::to_string(&sorted).map_or(Cow::Borrowed(input), Cow::Owned)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ReadFileRange {
    start: u64,
    /// Exclusive end. `None` means "through EOF".
    end: Option<u64>,
}

impl ReadFileRange {
    fn overlaps_or_touches(self, other: Self) -> bool {
        match self.end {
            None => true,
            Some(end) => end >= other.start,
        }
    }

    fn merged_end(self, other: Self) -> Option<u64> {
        match (self.end, other.end) {
            (None, _) | (_, None) => None,
            (Some(a), Some(b)) => Some(a.max(b)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ReadFileRequestWindow {
    path: String,
    range: ReadFileRange,
}

fn json_u64(value: &serde_json::Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_i64().and_then(|value| u64::try_from(value).ok()))
}

fn read_file_request_window(input: &str) -> Option<ReadFileRequestWindow> {
    let serde_json::Value::Object(map) = serde_json::from_str::<serde_json::Value>(input).ok()? else {
        return None;
    };
    let path = map.get("path")?.as_str()?.to_string();
    let start = map.get("offset").and_then(json_u64).unwrap_or(0);
    let end = map
        .get("limit")
        .and_then(json_u64)
        .map(|limit| start.saturating_add(limit));
    Some(ReadFileRequestWindow {
        path,
        range: ReadFileRange { start, end },
    })
}

/// The file a tool call targets, for the tools that name one.
///
/// `path` is the shared key across this codebase's file tools (`read_file`,
/// `edit_file`, `write_file`, …), which is what lets one extractor serve both the
/// read index and the mutation lookup. `None` for a tool that names no file — the
/// caller then falls back to the unscoped behaviour rather than guessing.
fn tool_path(input: &str) -> Option<String> {
    let serde_json::Value::Object(map) = serde_json::from_str::<serde_json::Value>(input).ok()?
    else {
        return None;
    };
    let path = map.get("path")?.as_str()?;
    (!path.is_empty()).then(|| path.to_string())
}

fn read_file_range_is_covered(ranges: &[ReadFileRange], needle: ReadFileRange) -> bool {
    let Some(needle_end) = needle.end else {
        return ranges
            .iter()
            .any(|range| range.start <= needle.start && range.end.is_none());
    };
    let mut covered_to = needle.start;
    for range in ranges {
        if range.end.is_some_and(|end| end <= covered_to) {
            continue;
        }
        if range.start > covered_to {
            return false;
        }
        match range.end {
            None => return true,
            Some(end) => {
                covered_to = covered_to.max(end);
                if covered_to >= needle_end {
                    return true;
                }
            }
        }
    }
    false
}

fn insert_read_file_range(ranges: &mut Vec<ReadFileRange>, range: ReadFileRange) {
    ranges.push(range);
    ranges.sort_by_key(|range| range.start);
    let mut merged: Vec<ReadFileRange> = Vec::with_capacity(ranges.len());
    for range in ranges.drain(..) {
        if let Some(last) = merged.last_mut() {
            if last.overlaps_or_touches(range) {
                last.end = (*last).merged_end(range);
                continue;
            }
        }
        merged.push(range);
    }
    *ranges = merged;
}

fn redundant_read_file_advisory(path: &str) -> String {
    format!(
        "<system-reminder>This `read_file` window for `{path}` is already covered by lines read earlier in this turn. Re-reading covered ranges repeats tokens without adding new source context. Use the previous result, read a new range, or switch tools if you need different evidence.</system-reminder>"
    )
}

/// Increment and return this turn's running count for fingerprint `fp`. A
/// per-turn tally (reset at turn start), not a rolling window, so it is immune
/// to fan-out width: a `read_file` re-read inside a 9-call batch still adds one
/// hit per turn round, so a no-progress loop crosses
/// [`TOOL_REPETITION_THRESHOLD`] no matter how many distinct calls separate the
/// repeats.
pub(super) fn record_tool_fingerprint(counts: &mut HashMap<u64, usize>, fp: u64) -> usize {
    let count = counts.entry(fp).or_insert(0);
    *count += 1;
    *count
}

fn microcompacted_reread_advisory(tool_name: &str) -> String {
    format!(
        "<system-reminder>The previous result for this exact `{tool_name}` call was compacted and its body was cleared, so this re-read restored missing context and is allowed. Use this fresh result now; do not repeat the exact same call again unless you change the request or need newly changed data.</system-reminder>"
    )
}

fn per_turn_tool_repetition_hard_stop_notice(tool_name: &str, count: usize) -> String {
    format!(
        "<system-reminder>Ending this turn: `{tool_name}` has been called with identical input {count} times without making progress — a no-progress loop, since the repeated call keeps returning the same result. Stopping so you can rethink; when you continue, take a different approach or ask the user for guidance.</system-reminder>"
    )
}

/// Advisory variant of the per-turn repetition notice used when the loop does
/// NOT end the turn (a successful read-only `read_file` exact repeat). The
/// wording must not tell the model the turn is ending — the terminating notice
/// above says "Ending this turn / Stopping", and a model that reads those words
/// while the harness keeps the turn alive will end its own turn, producing the
/// stop-then-resume stutter this variant exists to prevent.
pub(crate) fn per_turn_tool_repetition_nonterminating_notice(tool_name: &str, count: usize) -> String {
    format!(
        "<system-reminder>`{tool_name}` has been called with identical input {count} times without making progress — the repeated call keeps returning the same result. This exact repeat was skipped. Do not repeat it again; use the result you already have, change the arguments, or move on. The turn continues.</system-reminder>"
    )
}

/// Non-terminating variant of the cross-turn notice, used when the turn is
/// spared by [`ConversationRuntime::resolve_heuristic_stop`] because it has
/// produced fresh objective progress. Same "do not repeat this" instruction, but
/// it must NOT say the turn is ending: a model told it is stopping while the
/// harness keeps the turn alive ends its own turn, which is the stop-then-resume
/// stutter the per-turn variant's doc already warns about.
fn cross_turn_tool_repetition_nonterminating_notice(tool_name: &str, cross: usize) -> String {
    format!(
        "<system-reminder>`{tool_name}` has been called with identical input across {cross} \
         separate turns without making progress. This exact repeat was skipped. This turn has \
         changed files, so it continues — but do not issue this call again: use the result you \
         already have, change the arguments, or move on to the next concrete step.</system-reminder>"
    )
}

fn cross_turn_tool_repetition_hard_stop_notice(tool_name: &str, cross: usize) -> String {
    format!(
        "<system-reminder>Ending this turn: `{tool_name}` has been called with identical input across {cross} separate turns without making progress — a cross-turn no-progress loop that the per-turn guard cannot see (its counter resets each turn). Stop repeating it: take a genuinely different approach, or ask the user for guidance.</system-reminder>"
    )
}

pub(crate) fn skipped_after_repetition_stop_notice(tool_name: &str) -> String {
    format!(
        "<system-reminder>Skipping `{tool_name}` because this turn already hit a repeated-tool no-progress stop. The tool was not executed; change approach or ask the user for guidance.</system-reminder>"
    )
}

#[derive(Default)]
pub(super) struct ToolBatchRepetitionHardStops {
    /// Fingerprints hard-stopped earlier in this batch, mapped to whether that
    /// stop ends the turn. A later identical call is skipped, and it inherits
    /// the original stop's `terminates` so a non-terminating `read_file` skip
    /// does not end the turn.
    hard_stopped_fps: HashMap<u64, bool>,
}

impl ToolBatchRepetitionHardStops {
    pub(super) fn preflight_notice(
        &mut self,
        tool_name: &str,
        input: &str,
        hard_stop_notice: impl FnOnce() -> Option<(String, bool)>,
    ) -> Option<(String, bool)> {
        let fp = fingerprint_tool_call(tool_name, input);
        if let Some(&terminates) = self.hard_stopped_fps.get(&fp) {
            return Some((skipped_after_repetition_stop_notice(tool_name), terminates));
        }
        let notice = hard_stop_notice();
        if let Some((_, terminates)) = &notice {
            self.hard_stopped_fps.insert(fp, *terminates);
        }
        notice
    }

    fn record_outcome(&mut self, tool_name: &str, input: &str, repetition: &ToolRepetition) {
        if let ToolRepetition::HardStop { terminates, .. } = repetition {
            self.hard_stopped_fps
                .insert(fingerprint_tool_call(tool_name, input), *terminates);
        }
    }
}

impl<C, T> ConversationRuntime<C, T>
where
    C: ApiClient,
    T: ToolExecutor,
{
    /// Record an executed tool call and, the first time the same
    /// `(tool_name, input)` pair reaches [`TOOL_REPETITION_THRESHOLD`]
    /// occurrences within the rolling window, return a one-line advisory to
    /// fold into that tool's result so the model can break out of a tight loop.
    ///
    /// Fires once per streak — on the exact threshold count, not on every
    /// subsequent repeat — so a genuinely stuck agent gets a single nudge
    /// rather than a wall of identical reminders.
    pub(super) fn note_tool_repetition(
        &mut self,
        tool_name: &str,
        input: &str,
        is_error: bool,
    ) -> ToolRepetition {
        let fp = fingerprint_tool_call(tool_name, input);
        let is_mutation = is_edit_or_write_tool(tool_name);
        // A SUCCESSFUL file mutation is real progress: re-reading a just-edited
        // file is legitimate, so clear the cross-turn tally (which, unlike the
        // per-turn one, survives turn boundaries) whenever an edit/write tool
        // succeeds. This keeps a normal read → edit → read-to-confirm sequence
        // from ever looking like a cross-turn loop. A FAILED mutation made no
        // progress (the file is unchanged), so it must neither clear the loop
        // signal nor count toward it — otherwise a single failed edit dropped
        // into a re-read loop would silently reset the cross-turn guard and let
        // the loop run unbounded.
        let mutation_succeeded = is_mutation && !is_error;
        if mutation_succeeded {
            // Scoped to the file that was actually written. The forgiveness above
            // is about ONE file — the confirm-read of what was just edited — but
            // clearing every tally handed it to every other path too, and a long
            // editing session is a stream of successful edits. Measured
            // consequence: 641 identical-input `read_file` calls across this
            // repository's twelve longest sessions (~6% of all tool calls), with
            // one test file re-read 404 times, none of which the guard ever saw.
            //
            // A mutation whose path cannot be parsed keeps the old whole-clear:
            // over-forgiving is the safe direction for a loop guard, and the
            // alternative would leave a real edit looking like no progress.
            match tool_path(input) {
                Some(path) => self.forgive_repetition_for_path(&path, fp),
                None => self.forgive_all_repetition(),
            }
        }
        let count = record_tool_fingerprint(&mut self.tool_fingerprint_counts, fp);
        if !is_mutation {
            self.index_read_fingerprint(input, fp);
        }
        let recovering_microcompacted_result = !is_mutation
            && !is_error
            && self.latest_matching_tool_result_was_microcompacted(fp);
        // Cross-turn tally: it counts the number of DISTINCT turns that issued
        // this identical call, so it must advance at most ONCE per turn — on the
        // fingerprint's FIRST occurrence this turn (`count == 1`, because the
        // per-turn map is cleared at every turn start). Mutation tools never
        // count (a success cleared the tally above; a failure is not a repeated
        // read). Within-turn repeats (`count > 1`) are the PER-TURN guard's job:
        // counting them here would both turn the "across N separate turns" tally
        // into a lie AND let a single-turn read burst hit the lower cross-turn
        // hard stop (6) instead of the per-turn one (8), silently tightening the
        // same-turn threshold — so they contribute 0 and fall through to the
        // per-turn escalation only.
        let cross = if is_mutation || count != 1 {
            0
        } else {
            record_tool_fingerprint(&mut self.cross_turn_tool_fingerprints, fp)
        };
        // If the newest matching result in the transcript was microcompact-cleared,
        // the model's identical read is recovering information the runtime removed,
        // not proving that the tool "keeps returning the same result". Count it so
        // the microcompact-thrash promotion can still see the re-read signal, but
        // never hard-stop this recovery call. Once this fresh result is appended, a
        // later identical call will no longer match the cleared-result exemption and
        // the ordinary no-progress guard applies again.
        if recovering_microcompacted_result
            && (count >= TOOL_REPETITION_THRESHOLD
                || cross >= TOOL_REPETITION_CROSS_TURN_ADVISE
                || self.tool_repetition_hard_stop_fps.contains(&fp)
                || self.cross_turn_tool_repetition_hard_stop_fps.contains(&fp))
        {
            self.tool_repetition_pending_hard_stop_fps.insert(fp);
            self.cross_turn_tool_repetition_pending_hard_stop_fps.insert(fp);
            return ToolRepetition::Advise(microcompacted_reread_advisory(tool_name));
        }

        if tool_name == "read_file" && !is_error {
            if let Some(repetition) = self.note_read_file_window(input, count) {
                return repetition;
            }
        }

        // Precedence: a hard stop (per-turn, then cross-turn) wins over an
        // advisory; each advisory stage fires once, on the exact count, so a
        // stuck agent gets a single nudge then a stop — not a wall of reminders.
        //
        // Ending the turn here is a *heuristic*, not a cost breaker: it infers
        // "stuck" from one repeated fingerprint, which misfires on a turn that is
        // demonstrably producing work elsewhere — and unlike the budget stops it
        // leaves no `BudgetExhausted` marker, so the host's progress-gated
        // auto-continue never even sees it and the turn dies silently mid-work.
        // A repeat that is itself a mutation or an error made no progress by
        // definition, so those never demote; anything else asks
        // `resolve_heuristic_stop`, which spares a productive turn exactly once.
        // Who is there decides whether a stop is an end. An attended turn is
        // never ended by this guard: the repeat is skipped, the model is told,
        // and the person at the keyboard — who can see the loop — is the
        // breaker, as in Claude Code and Codex. An unattended turn has nobody
        // to press Esc, so a real no-progress loop still ends it.
        let attended = self.attendance == Attendance::Attended;
        if count >= TOOL_REPETITION_HARD_STOP && self.tool_repetition_hard_stop_fps.contains(&fp) {
            let terminates = !attended && (is_error || is_mutation || tool_name != "read_file");
            if terminates
                && !is_error
                && !is_mutation
                && self.resolve_heuristic_stop(TurnStopSignal::RepetitionPerTurn)
                    == TurnContinuity::Nudge
            {
                return ToolRepetition::HardStop {
                    notice: per_turn_tool_repetition_nonterminating_notice(tool_name, count),
                    terminates: false,
                };
            }
            ToolRepetition::HardStop {
                notice: if terminates {
                    per_turn_tool_repetition_hard_stop_notice(tool_name, count)
                } else {
                    per_turn_tool_repetition_nonterminating_notice(tool_name, count)
                },
                terminates,
            }
        } else if cross >= TOOL_REPETITION_CROSS_TURN_HARD_STOP
            && self.cross_turn_tool_repetition_hard_stop_fps.contains(&fp)
        {
            if !is_error
                && !is_mutation
                && self.resolve_heuristic_stop(TurnStopSignal::RepetitionCrossTurn)
                    == TurnContinuity::Nudge
            {
                return ToolRepetition::HardStop {
                    notice: cross_turn_tool_repetition_nonterminating_notice(tool_name, cross),
                    terminates: false,
                };
            }
            ToolRepetition::HardStop {
                notice: if attended {
                    cross_turn_tool_repetition_nonterminating_notice(tool_name, cross)
                } else {
                    cross_turn_tool_repetition_hard_stop_notice(tool_name, cross)
                },
                terminates: !attended,
            }
        } else if count == TOOL_REPETITION_THRESHOLD {
            self.tool_repetition_pending_hard_stop_fps.insert(fp);
            ToolRepetition::Advise(format!(
                "<system-reminder>You have now called `{tool_name}` with identical input \
                 {TOOL_REPETITION_THRESHOLD} times this turn. Repeating the same call will not \
                 produce a different result — change approach: vary the arguments, try a \
                 different tool, or if you are blocked, say so and ask the user. Do not repeat \
                 this exact call again.</system-reminder>"
            ))
        } else if cross == TOOL_REPETITION_CROSS_TURN_ADVISE {
            self.cross_turn_tool_repetition_pending_hard_stop_fps.insert(fp);
            ToolRepetition::Advise(format!(
                "<system-reminder>You have now called `{tool_name}` with identical input across \
                 {cross} separate turns. Re-issuing it keeps returning the same result and is \
                 making no progress — change approach, try a different tool, or ask the user what \
                 they need. Do not repeat this exact call again.</system-reminder>"
            ))
        } else {
            ToolRepetition::Ok
        }
    }

    /// Index a non-mutating call under the file it touched, so a later
    /// successful edit to that file can forgive exactly these fingerprints and
    /// no others. A call that names no file — a command, a search — is indexed
    /// as pathless: any successful edit forgives it, because running it again
    /// after the edit is how the model checks its work.
    fn index_read_fingerprint(&mut self, input: &str, fp: u64) {
        match tool_path(input) {
            Some(path) => {
                self.read_fingerprints_by_path
                    .entry(path)
                    .or_default()
                    .insert(fp);
            }
            None => {
                self.pathless_fingerprints.insert(fp);
            }
        }
    }

    /// Drop the repetition state that belongs to `path` (plus the mutation's own
    /// fingerprint) and to every call that names no path, leaving every other
    /// path's tally intact. The pathless calls go because the file just changed
    /// under them: the same test command, the same grep, is a re-check now.
    fn forgive_repetition_for_path(&mut self, path: &str, mutation_fp: u64) {
        let mut fingerprints = self
            .read_fingerprints_by_path
            .remove(path)
            .unwrap_or_default();
        fingerprints.insert(mutation_fp);
        fingerprints.extend(self.pathless_fingerprints.drain());
        for fp in &fingerprints {
            self.tool_fingerprint_counts.remove(fp);
            self.tool_repetition_pending_hard_stop_fps.remove(fp);
            self.tool_repetition_hard_stop_fps.remove(fp);
            self.cross_turn_tool_fingerprints.remove(fp);
            self.cross_turn_tool_repetition_pending_hard_stop_fps.remove(fp);
            self.cross_turn_tool_repetition_hard_stop_fps.remove(fp);
        }
        self.read_file_ranges_by_path.remove(path);
        self.read_file_redundant_advised_paths.remove(path);
    }

    /// The pre-scoping behaviour, kept for a mutation whose target path is not
    /// recoverable from its input.
    fn forgive_all_repetition(&mut self) {
        self.tool_fingerprint_counts.clear();
        self.pathless_fingerprints.clear();
        self.tool_repetition_pending_hard_stop_fps.clear();
        self.tool_repetition_hard_stop_fps.clear();
        self.read_file_ranges_by_path.clear();
        self.read_file_redundant_advised_paths.clear();
        self.read_fingerprints_by_path.clear();
        self.cross_turn_tool_fingerprints.clear();
        self.cross_turn_tool_repetition_pending_hard_stop_fps.clear();
        self.cross_turn_tool_repetition_hard_stop_fps.clear();
    }

    fn note_read_file_window(
        &mut self,
        input: &str,
        exact_repeat_count: usize,
    ) -> Option<ToolRepetition> {
        // Exact same-input loops are handled by the generic fingerprint guard.
        // This range-aware guard is only for different `read_file` inputs whose
        // requested lines are already fully covered by earlier reads this turn.
        if exact_repeat_count > 1 {
            return None;
        }
        let request = read_file_request_window(input)?;
        let ranges = self
            .read_file_ranges_by_path
            .entry(request.path.clone())
            .or_default();
        if read_file_range_is_covered(ranges, request.range) {
            if self
                .read_file_redundant_advised_paths
                .insert(request.path.clone())
            {
                return Some(ToolRepetition::Advise(redundant_read_file_advisory(
                    &request.path,
                )));
            }
            return None;
        }
        insert_read_file_range(ranges, request.range);
        None
    }

    /// Errors are excluded from this scan: the hard-stop guard itself appends a
    /// synthetic `is_error: true` skip-notice result for the very fingerprint it
    /// just skipped, and that notice would otherwise shadow the real (possibly
    /// microcompacted) result as "the latest match", permanently disabling the
    /// exemption once the guard has fired even once.
    ///
    /// No surviving successful result at all is treated as cleared too: the
    /// only way a fingerprint reaches the repetition guard with its results
    /// entirely absent from the transcript is that the runtime removed them
    /// (full compaction, distill) — "use the result you already have" points
    /// at nothing, so the repeat is recovery, not a no-progress loop.
    fn latest_matching_tool_result_was_microcompacted(&self, fp: u64) -> bool {
        let mut tool_use_fingerprints: HashMap<&str, u64> = HashMap::new();
        let mut latest_match_was_cleared = None;

        for message in self.session.messages.iter() {
            for block in &message.blocks {
                match block {
                    ContentBlock::ToolUse { id, name, input } => {
                        tool_use_fingerprints
                            .insert(id.as_str(), fingerprint_tool_call(name, input));
                    }
                    ContentBlock::ToolResult {
                        tool_use_id,
                        output,
                        is_error,
                        ..
                    } => {
                        if *is_error {
                            continue;
                        }
                        if tool_use_fingerprints.get(tool_use_id.as_str()) == Some(&fp) {
                            latest_match_was_cleared =
                                Some(output == crate::MICROCOMPACT_PLACEHOLDER);
                        }
                    }
                    ContentBlock::Text { .. }
                    | ContentBlock::Image { .. }
                    | ContentBlock::Thinking { .. }
                    | ContentBlock::RedactedThinking { .. } => {}
                }
            }
        }

        latest_match_was_cleared.unwrap_or(true)
    }

    /// True when some tool call has repeated (same normalized fingerprint) at
    /// least [`TOOL_REPETITION_THRESHOLD`] times this turn — the same signal the
    /// repetition advisory fires on. Gates the microcompact thrash-escape so it
    /// distinguishes a genuine re-read loop (a repeat is present) from a wide but
    /// progressing multi-file read (many distinct calls, no repeat), which would
    /// otherwise trip a premature full compaction just for trimming a few rounds.
    pub(super) fn has_repeated_tool_call(&self) -> bool {
        self.tool_fingerprint_counts
            .values()
            .any(|&count| count >= TOOL_REPETITION_THRESHOLD)
    }

    /// True when some tool call has repeated (same normalized fingerprint) at
    /// least [`TOOL_REPETITION_CROSS_TURN_ADVISE`] times ACROSS turns — the
    /// session-scoped analogue of [`Self::has_repeated_tool_call`]. Lets the
    /// microcompact thrash-escape fire on a re-read loop that spans turn
    /// boundaries, which the per-turn tally (reset every turn) would otherwise
    /// hide.
    pub(super) fn has_cross_turn_repeated_tool_call(&self) -> bool {
        self.cross_turn_tool_fingerprints
            .values()
            .any(|&count| count >= TOOL_REPETITION_CROSS_TURN_ADVISE)
    }

    /// Mark repetition advisories emitted in the just-finished tool batch as
    /// visible to the model. Hard stops are allowed only after this boundary so
    /// several identical tool calls from one assistant response do not punish the
    /// model for failing to react to advice it has not observed yet.
    pub(super) fn arm_tool_repetition_hard_stops(&mut self) {
        self.tool_repetition_hard_stop_fps
            .extend(self.tool_repetition_pending_hard_stop_fps.drain());
        self.cross_turn_tool_repetition_hard_stop_fps
            .extend(self.cross_turn_tool_repetition_pending_hard_stop_fps.drain());
    }

    /// Predict, before executing the next identical call, whether it would trip
    /// a hard stop — and whether that stop ends the turn. Returns the notice to
    /// fold into the synthetic tool result plus a `terminates` flag mirroring
    /// [`ToolRepetition::HardStop`]. A successful repeated `read_file` gets one
    /// non-terminating skipped round; repeating after seeing that notice ends
    /// the turn. Mutations, cross-turn loops, and every other tool end at the
    /// first armed hard stop.
    ///
    /// This is the arm the live turn loops ACTUALLY reach once a stop is armed:
    /// the repeat is skipped before dispatch, so [`Self::note_tool_repetition`]
    /// never runs for it. The productive-turn reprieve therefore has to be
    /// decided here as well, or it is dead code everywhere except a test that
    /// calls `note_tool_repetition` directly.
    pub(super) fn next_tool_repetition_hard_stop_notice(
        &mut self,
        tool_name: &str,
        input: &str,
    ) -> Option<(String, bool)> {
        let fp = fingerprint_tool_call(tool_name, input);
        let is_mutation = is_edit_or_write_tool(tool_name);
        if !is_mutation && self.latest_matching_tool_result_was_microcompacted(fp) {
            return None;
        }
        let next_count = self
            .tool_fingerprint_counts
            .get(&fp)
            .copied()
            .unwrap_or(0)
            .saturating_add(1);
        if next_count >= TOOL_REPETITION_HARD_STOP
            && self.tool_repetition_hard_stop_fps.contains(&fp)
        {
            // Count the skipped round so a successful repeated read gets one
            // chance to recover after seeing the advisory, but cannot request
            // the same cached result forever now that interactive turns have no
            // generic iteration ceiling.
            self.tool_fingerprint_counts.insert(fp, next_count);
            // The same rule as the recorded stop above: an attended turn is
            // never ended by this guard — the call is skipped, the model is
            // told, and the person at the keyboard is the breaker. This path
            // used to decide without asking, which closed a live turn that was
            // polling `ListAgents`/`Sleep` for a sub-agent with "Tool-repetition
            // guard exhausted after 27 iteration(s)" (2026-09-07).
            let attended = self.attendance == Attendance::Attended;
            let mut terminates = !attended
                && (is_mutation
                    || tool_name != "read_file"
                    || next_count > TOOL_REPETITION_HARD_STOP);
            // A repeated mutation is never progress (it is the same write
            // again), so it always terminates; anything else asks the shared
            // heuristic gate, which spares a turn with fresh objective progress
            // exactly once. The call is skipped either way — only the turn's
            // life is at stake here.
            if terminates
                && !is_mutation
                && self.resolve_heuristic_stop(TurnStopSignal::RepetitionPerTurn)
                    == TurnContinuity::Nudge
            {
                terminates = false;
            }
            let notice = if terminates {
                per_turn_tool_repetition_hard_stop_notice(tool_name, next_count)
            } else {
                per_turn_tool_repetition_nonterminating_notice(tool_name, next_count)
            };
            return Some((notice, terminates));
        }
        if !is_mutation && next_count == 1 {
            let next_cross = self
                .cross_turn_tool_fingerprints
                .get(&fp)
                .copied()
                .unwrap_or(0)
                .saturating_add(1);
            if next_cross >= TOOL_REPETITION_CROSS_TURN_HARD_STOP
                && self.cross_turn_tool_repetition_hard_stop_fps.contains(&fp)
            {
                // Same reprieve as the per-turn arm above (this branch is
                // already mutation-free): a turn that has produced fresh
                // progress keeps running for one more round.
                let terminates = self.attendance == Attendance::Unattended
                    && self.resolve_heuristic_stop(TurnStopSignal::RepetitionCrossTurn)
                        != TurnContinuity::Nudge;
                let notice = if terminates {
                    cross_turn_tool_repetition_hard_stop_notice(tool_name, next_cross)
                } else {
                    cross_turn_tool_repetition_nonterminating_notice(tool_name, next_cross)
                };
                return Some((notice, terminates));
            }
        }
        None
    }

    /// Fold repeated same-class permission denials within one turn: the first
    /// denial of a (tool, audit-class) pair keeps its full reason and
    /// remediation, every later one collapses to a single line. The class is
    /// the "Permission audit: …" sentence, which is identical across
    /// *different* inputs denied by the same mode — exactly the case the
    /// per-input fingerprint guard cannot see. Mode denials are
    /// deterministic, so repeating the full audit wall only bloats the
    /// transcript and goads the model into re-litigating the mode.
    pub(super) fn fold_repeated_mode_denial(&mut self, tool_name: &str, body: String) -> String {
        let Some(class) = super::denial_audit_class(&body).map(str::to_owned) else {
            return body;
        };
        let count = self
            .mode_denial_counts
            .entry((tool_name.to_owned(), class.clone()))
            .and_modify(|count| *count += 1)
            .or_insert(1);
        if *count <= 1 {
            body
        } else {
            format!(
                "denied — same permission class as an earlier `{tool_name}` denial this turn \
                 (occurrence #{count}; {class}). This denial is deterministic: do not retry \
                 the class; continue with tools allowed in the current mode or ask the user \
                 to change it."
            )
        }
    }

    /// After a tool batch: count a batch that was exactly one read-only call,
    /// and at [`SERIAL_READS_NUDGE_AT`] in a row append [`SERIAL_READS_NUDGE`]
    /// to the batch's result message — once per turn. A batch of two or more
    /// calls, or one that wrote something, ends the run: the model did what
    /// the rubric asks, or had a reason to go one at a time.
    pub(super) fn nudge_serial_reads_after_batch(&mut self) {
        let single_read = self
            .session
            .messages
            .iter()
            .rev()
            .find(|message| message.role == crate::session::MessageRole::Assistant)
            .is_some_and(|message| {
                let mut uses = message.blocks.iter().filter_map(|block| match block {
                    ContentBlock::ToolUse { name, .. } => Some(name.as_str()),
                    _ => None,
                });
                matches!((uses.next(), uses.next()), (Some(name), None) if is_concurrency_safe(name))
            });
        if !single_read {
            self.serial_read_batches = 0;
            return;
        }
        self.serial_read_batches = self.serial_read_batches.saturating_add(1);
        if self.serial_reads_nudged || self.serial_read_batches < SERIAL_READS_NUDGE_AT {
            return;
        }
        let messages = Arc::make_mut(&mut self.session.messages);
        let Some(last) = messages
            .last_mut()
            .filter(|message| message.role == crate::session::MessageRole::Tool)
        else {
            return;
        };
        last.blocks.push(ContentBlock::Text {
            text: SERIAL_READS_NUDGE.to_string(),
        });
        self.serial_reads_nudged = true;
        self.session.mark_transcript_dirty();
    }

    pub(super) fn append_tool_repetition_notice(
        &mut self,
        output: &mut String,
        tool_name: &str,
        input: &str,
        is_error: bool,
        batch_hard_stops: &mut ToolBatchRepetitionHardStops,
    ) {
        let repetition = self.note_tool_repetition(tool_name, input, is_error);
        batch_hard_stops.record_outcome(tool_name, input, &repetition);
        match repetition {
            ToolRepetition::Ok => {}
            ToolRepetition::Advise(advisory) => {
                output.push_str("\n\n");
                output.push_str(&advisory);
            }
            ToolRepetition::HardStop { notice, terminates } => {
                output.push_str("\n\n");
                output.push_str(&notice);
                if terminates {
                    self.tool_loop_break_requested = true;
                }
            }
        }
    }

    pub(super) fn parallel_batch_has_repetition_risk<'a>(
        &self,
        tools: impl IntoIterator<Item = (&'a str, &'a str, &'a PermissionOutcome)>,
    ) -> bool {
        let mut projected_counts = self.tool_fingerprint_counts.clone();
        let mut projected_cross_counts = self.cross_turn_tool_fingerprints.clone();
        for (tool_name, input, permission_outcome) in tools {
            if !matches!(permission_outcome, PermissionOutcome::Allow)
                || !is_concurrency_safe(tool_name)
            {
                continue;
            }
            let fp = fingerprint_tool_call(tool_name, input);
            let is_mutation = is_edit_or_write_tool(tool_name);
            let recovering_microcompacted_result =
                !is_mutation && self.latest_matching_tool_result_was_microcompacted(fp);

            let next_count = projected_counts
                .get(&fp)
                .copied()
                .unwrap_or(0)
                .saturating_add(1);
            if !recovering_microcompacted_result
                && next_count >= TOOL_REPETITION_HARD_STOP
                && self.tool_repetition_hard_stop_fps.contains(&fp)
            {
                return true;
            }

            if !is_mutation && next_count == 1 {
                let next_cross = projected_cross_counts
                    .get(&fp)
                    .copied()
                    .unwrap_or(0)
                    .saturating_add(1);
                if !recovering_microcompacted_result
                    && next_cross >= TOOL_REPETITION_CROSS_TURN_HARD_STOP
                    && self.cross_turn_tool_repetition_hard_stop_fps.contains(&fp)
                {
                    return true;
                }
                projected_cross_counts.insert(fp, next_cross);
            }

            projected_counts.insert(fp, next_count);
        }
        false
    }
}
