//! Per-turn request assembly and session-trace recording for
//! [`ConversationRuntime`], split out of `mod.rs` so the turn loop there reads
//! as orchestration. Behaviour-preserving: these were `impl ConversationRuntime`
//! methods, now `pub(super)` so the loop in `mod.rs` still calls them.

use std::borrow::Cow;
use std::sync::Arc;

use serde_json::{Value, json};
use telemetry::SessionTracer;

use super::{
    ApiClient, ApiRequest, CompactionConfig, ContentBlock, ConversationMessage,
    ConversationRuntime, DEFAULT_MAX_OUTPUT_TOKENS, DEFAULT_MEMORY_RECALL_LIMIT, MemoryRetriever,
    MessageRole, RuntimeError, StreamingTurnError, ToolExecutor, TurnSummary,
    estimate_session_tokens, estimate_system_prompt_tokens, render_recalled_memory_section,
    trace_attrs,
};
use crate::memory::recall::MAX_RECALLED_ENTRIES;
use crate::prompt::{render_reminders, ReminderCandidate, MAX_REMINDER_LINES};

/// Entries one recall asks for: the [`DEFAULT_MEMORY_RECALL_LIMIT`] the
/// recalled-memory section renders, plus enough for a full reminder block
/// behind it.
///
/// One call, not two. The reminder block is what the recall section did not
/// have room for, so asking the retriever once and splitting the answer is
/// both cheaper and the only way the two can be guaranteed disjoint — a second
/// query could return an entry the first already rendered.
const RECALL_AND_REMINDER_LIMIT: usize = DEFAULT_MEMORY_RECALL_LIMIT + MAX_REMINDER_LINES;

/// Split one recall into the section a turn reads and the reminders behind it.
///
/// The section comes first because it is the richer rendering — link, path,
/// summary, and a snippet for the top entries. The reminder block behind it
/// carries what the section had no room for, one terse line each, and never an
/// entry the section already named: a duplicate is this block's whole budget
/// spent saying something the request already says.
///
/// Only lesson-shaped slugs become reminders. A promoted lesson is always
/// `<kind>-<signature>` ([`decision_core::dreamer::slug_for`]), so the prefix
/// is the cheap, IO-free way to tell "what this project learned" from a
/// hand-filed note — and a hand-filed `gotcha-…` is exactly as much a lesson.
///
/// Both halves read the hits the recall seat handed back, so a note that seat
/// left out is out of the reminder block too ([`crate::RecallSeat`]). That is
/// the same answer given once rather than twice: a lesson a judgment read as
/// bearing on nothing in the request is not a lesson this turn needs reminding
/// of either.
fn recall_and_reminder_sections(hits: &[core_types::MemoryHit]) -> Vec<String> {
    let rendered = &hits[..hits.len().min(MAX_RECALLED_ENTRIES)];
    let already: Vec<String> = rendered
        .iter()
        .map(|hit| hit.entry.slug.clone())
        .collect();
    if !reminders_enabled() {
        let mut only_recall = Vec::with_capacity(1);
        only_recall.extend(render_recalled_memory_section(rendered));
        return only_recall;
    }
    let candidates: Vec<ReminderCandidate> = hits
        .iter()
        .filter(|hit| is_lesson_slug(&hit.entry.slug))
        .map(|hit| ReminderCandidate {
            slug: hit.entry.slug.clone(),
            lesson: hit.entry.summary.clone(),
        })
        .collect();
    let mut sections = Vec::with_capacity(2);
    sections.extend(render_recalled_memory_section(rendered));
    sections.extend(render_reminders(&candidates, &already));
    sections
}

/// Whether a slug was minted by the curation gate — `<kind>-<signature>`.
///
/// Read from the last path segment, so a lesson answers to this both in the
/// memory store (`gotcha-zsh-pipe`) and in the vault the write-back mirrors it
/// into (`wiki/zo/gotcha-zsh-pipe`). Those are one lesson filed at two depths,
/// and only one of them being reminder-eligible would mean a person who reads
/// their lessons in Obsidian never gets them back.
fn is_lesson_slug(slug: &str) -> bool {
    let leaf = slug.rsplit('/').next().unwrap_or(slug);
    [
        decision_core::dreamer::LessonKind::Preference,
        decision_core::dreamer::LessonKind::Gotcha,
        decision_core::dreamer::LessonKind::Workflow,
        decision_core::dreamer::LessonKind::Constraint,
    ]
    .iter()
    .any(|kind| {
        leaf.strip_prefix(kind.as_str())
            .is_some_and(|rest| rest.starts_with('-'))
    })
}

/// Whether the reminder block is emitted at all. `ZO_REMINDERS=0` turns it off.
///
/// The bench's off arm and nothing else. Two arms that are two binaries measure
/// the build; two arms of one binary measure the feature, and a switch is the
/// only way to have the second kind. Recall is untouched either way, so the off
/// arm is this arm minus exactly one block.
fn reminders_enabled() -> bool {
    !matches!(
        std::env::var("ZO_REMINDERS").as_deref(),
        Ok("0" | "off" | "false" | "no")
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TurnStopOrigin {
    User,
    HostFailure,
}

impl TurnStopOrigin {
    const fn candidate_kind(self) -> decision_core::dreamer::CandidateKind {
        match self {
            Self::User => decision_core::dreamer::CandidateKind::UserCancelled,
            Self::HostFailure => decision_core::dreamer::CandidateKind::TurnFailure,
        }
    }

    const fn summary(self) -> &'static str {
        match self {
            Self::User => "turn cancelled by user or host",
            Self::HostFailure => "turn failed because the host stopped consuming",
        }
    }

    const fn trace_outcome(self) -> crate::turn_trace::TurnOutcome {
        match self {
            Self::User => crate::turn_trace::TurnOutcome::Cancelled,
            Self::HostFailure => crate::turn_trace::TurnOutcome::Failed,
        }
    }

    const fn trace_event(self) -> &'static str {
        match self {
            Self::User => "turn_cancelled",
            Self::HostFailure => "turn_host_failure",
        }
    }
}

impl<C, T> ConversationRuntime<C, T>
where
    C: ApiClient,
    T: ToolExecutor,
{
    /// Bind this runtime to the attempt a PARENT is paying for, recorded under
    /// `cache_scope` — the prompt-cache session id its requests land in.
    ///
    /// A spawned agent's runtime uses this instead of minting turn keys: its
    /// cost belongs to the spawn record the parent wrote
    /// (`<agentId>#<runGeneration>`), and only an equal key joins the two.
    #[must_use]
    pub fn with_inherited_attempt(
        mut self,
        attempt: impl Into<String>,
        cache_scope: impl Into<String>,
    ) -> Self {
        let attempt = attempt.into();
        let cache_scope = cache_scope.into();
        self.attempt_cache_scope = (!cache_scope.is_empty()).then_some(cache_scope);
        self.inherited_attempt = (!attempt.is_empty()).then_some(attempt);
        self.begin_attempt();
        self
    }

    /// The attempt this runtime's requests currently belong to, or empty
    /// before its first turn begins.
    #[must_use]
    pub fn attempt(&self) -> &str {
        &self.attempt
    }

    /// The prompt-cache session id this runtime's request rows are written
    /// under — a spawned agent's AGENT id, otherwise the session's own.
    fn attempt_cache_scope(&self) -> &str {
        self.attempt_cache_scope
            .as_deref()
            .unwrap_or(&self.session.session_id)
    }

    /// Declare the attempt the next requests belong to, and deposit it where
    /// the prompt-cache recorder reads it.
    ///
    /// Called at every turn start, from both turn loops, next to
    /// [`Self::record_turn_started`] and BEFORE the user message is pushed —
    /// so the ordinal it counts is the number of turns this session had
    /// already started, and this turn is the next one. A child runtime keeps
    /// the attempt its parent handed it.
    ///
    /// Once per turn, never per request: the deposit is one map write, and the
    /// per-request recorder only reads.
    pub(super) fn begin_attempt(&mut self) {
        self.attempt = self.next_attempt();
        ::api::note_attempt(self.attempt_cache_scope(), &self.attempt);
    }

    /// The attempt the NEXT turn will be spent on, without starting it.
    ///
    /// A host that classifies a turn before handing it over — the routing
    /// probe runs before the first request leaves — needs the key that turn
    /// will carry, so the tax it records lands on the attempt that paid it.
    /// The same computation the turn start commits, and nothing between the
    /// two calls moves it: the user message that would advance the ordinal is
    /// pushed after the mint.
    #[must_use]
    pub fn next_attempt(&self) -> String {
        match &self.inherited_attempt {
            Some(inherited) => inherited.clone(),
            None => crate::turn_attempt_key(&self.session.session_id, self.turn_ordinal()),
        }
    }

    /// The 1-based ordinal of the turn about to start: the user turns this
    /// session has already taken, plus this one.
    ///
    /// Counted off the transcript rather than kept in a counter, so a
    /// `/resume` picks the sequence back up where the session left it instead
    /// of restarting at 1 and colliding with keys already on disk.
    fn turn_ordinal(&self) -> usize {
        self.session
            .messages
            .iter()
            .filter(|message| message.role == MessageRole::User)
            .count()
            + 1
    }

    pub(super) fn record_turn_started(&self, user_input: &str) {
        let Some(session_tracer) = &self.session_tracer else {
            return;
        };
        session_tracer.record(
            "turn_started",
            trace_attrs(json!({"user_input": user_input})),
        );
    }

    /// The connected gateway (a `providers[]` row) serving the active model,
    /// if one does. First-party models — every OAuth road among them — have
    /// none, so nothing gateway-specific ever reaches them.
    pub(super) fn active_gateway(&self) -> Option<String> {
        let model = self.context_model.as_deref()?;
        ::api::custom_provider_for_model(model)
            .map(|provider| provider.config.provider_name.to_string())
    }

    /// The gateways this runtime asks without recalled memory, for the runtime
    /// that takes the same session over ([`Self::withhold_recall_from`]): a
    /// model switch rebuilds the runtime, and a gateway that refused zo's
    /// recall once is not sent it again under the next model.
    #[must_use]
    pub fn gateways_refusing_recall(&self) -> Vec<String> {
        let mut gateways: Vec<String> = self.recall_withheld_gateways.iter().cloned().collect();
        gateways.sort_unstable();
        gateways
    }

    /// Ask these gateways without recalled memory, as the runtime this one
    /// took over from did ([`Self::gateways_refusing_recall`]).
    pub fn withhold_recall_from(&mut self, gateways: impl IntoIterator<Item = String>) {
        self.recall_withheld_gateways.extend(gateways);
    }

    /// Whether this session withholds recalled memory from the active model's
    /// gateway. Free until a gateway has refused: the set is empty.
    fn recall_withheld(&self) -> bool {
        !self.recall_withheld_gateways.is_empty()
            && self
                .active_gateway()
                .is_some_and(|gateway| self.recall_withheld_gateways.contains(&gateway))
    }

    /// A gateway's content filter refused the request the latest reminder rode,
    /// and that reminder carried recalled memory zo attached on its own: take
    /// the reminder back (`keep` is the transcript length before it was
    /// absorbed) and withhold recall from that gateway for the rest of the
    /// session. `None` when this does not apply — no recall rode the request,
    /// the model is first-party, or the gateway was already asked without it —
    /// otherwise the gateway's name, for the notice.
    pub(super) fn withhold_recall_from_refusing_gateway(
        &mut self,
        error: &RuntimeError,
        recall_attached: bool,
        keep: usize,
    ) -> Option<String> {
        if !recall_attached
            || error.provider_error_class() != Some(crate::ProviderErrorClass::ContentRefused)
        {
            return None;
        }
        let gateway = self.active_gateway()?;
        if !self.recall_withheld_gateways.insert(gateway.clone()) {
            return None;
        }
        self.withdraw_undispatched_reminders(keep);
        Some(gateway)
    }

    /// The gateway whose content filter refused a request that carried none of
    /// zo's recalled memory — what it refused is the conversation itself, so
    /// nothing zo can take back will get the turn past it. `None` for any
    /// other failure, and for first-party models.
    pub(super) fn gateway_refusing_the_conversation(&self, error: &RuntimeError) -> Option<String> {
        (error.provider_error_class() == Some(crate::ProviderErrorClass::ContentRefused))
            .then(|| self.active_gateway())
            .flatten()
    }

    /// Take back what the transcript gained after `keep` for a request that
    /// was not answered, and re-derive the recall dedup marks: they are a
    /// property of the TRANSCRIPT (as compaction and `replace_session` treat
    /// them), and a mark surviving its body would collapse the next attempt to
    /// a pointer aimed at a body that is not in context.
    pub(super) fn withdraw_undispatched_reminders(&mut self, keep: usize) {
        if self.session.messages.len() > keep {
            Arc::make_mut(&mut self.session.messages).truncate(keep);
            self.session.mark_transcript_dirty();
            self.recalled_memory_slugs = super::recalled_slugs_from_session(&self.session);
        }
    }

    /// Reminders for the request being built right now: the toggled transient
    /// reminders plus a query-recalled memory section. The caller hands them to
    /// [`Self::absorb_wire_reminders_into_session`], which persists them as a
    /// trailing `System` transcript message; the system prompt stays frozen so
    /// its cache blocks — and every message breakpoint behind them — keep
    /// hitting, and the persisted position keeps historical renders
    /// byte-identical (the old wire-only injection re-billed the prior tail on
    /// every request).
    pub(super) fn request_wire_reminders(&self) -> Arc<[String]> {
        let recalled = self
            .memory_retriever
            .as_ref()
            .and_then(|retriever| {
                let query = self.recall_query_text()?;
                Some(retriever.recall(query.as_ref(), RECALL_AND_REMINDER_LIMIT))
            })
            .unwrap_or_default();
        let mut reminders = self.transient_reminders.clone();
        reminders.extend(recall_and_reminder_sections(&recalled));
        Arc::from(reminders)
    }

    /// Async sibling of [`Self::request_wire_reminders`]' recall half: runs the
    /// memory recall in `spawn_blocking` so a dense (ONNX) embedding forward
    /// pass — which holds a mutex and can take tens of ms — never runs on the
    /// streaming drive-loop `select!` task and starve render/reveal/input
    /// (FREEZE-1). The streaming turn path awaits this; the synchronous
    /// `build_request`/headless paths keep the sync recall (no render loop
    /// there to starve).
    /// Takes OWNED inputs — the caller clones them out of `&self` first — so
    /// the enclosing spawned turn future never holds a `&self` borrow across
    /// this await. That matters because the runtime is `Send` but not `Sync`
    /// (it holds `Send`-only trait objects), so a `&self` borrow spanning an
    /// await would fail to compile.
    pub(super) async fn recall_reminder_section(
        retriever: Option<Arc<dyn MemoryRetriever + Send + Sync>>,
        query: Option<String>,
        tracer: Option<SessionTracer>,
        seat: Option<Arc<dyn crate::RecallSeat>>,
    ) -> Vec<String> {
        let (Some(retriever), Some(query)) = (retriever, query) else {
            return Vec::new();
        };
        let section = match tokio::task::spawn_blocking(move || {
            let hits = retriever.recall(&query, RECALL_AND_REMINDER_LIMIT);
            // After recall has settled and before anything renders, so the seat
            // sees exactly what recall chose and its answer is what the turn
            // reads. A seat in a record-only mode hands the same hits back.
            let hits = match seat {
                Some(seat) => seat.settle(&query, hits),
                None => hits,
            };
            recall_and_reminder_sections(&hits)
        })
        .await
        {
            Ok(section) => section,
            // A panic inside recall (e.g. a poisoned embedding mutex) must not
            // abort the turn — degrade to no recalled context — but is surfaced so
            // a silently-degraded dense retriever is visible. eprintln reaches the
            // TUI's redirected stderr (zo.log) and headless stderr; the trace
            // event reaches OTLP operators who watch the telemetry stream rather
            // than the log (the owned `tracer` keeps this fn self-less, so the
            // spawned turn future stays `Send`).
            Err(error) => {
                eprintln!(
                    "zo: memory recall task failed ({error}); continuing this turn without recalled context"
                );
                if let Some(tracer) = &tracer {
                    tracer.record(
                        "memory_recall_failed",
                        trace_attrs(json!({ "error": error.to_string() })),
                    );
                }
                Vec::new()
            }
        };
        section
    }

    pub(super) fn latest_user_text(&self) -> Option<Cow<'_, str>> {
        self.session.messages.iter().rev().find_map(|message| {
            if message.role != MessageRole::User {
                return None;
            }
            message_text(message)
        })
    }

    pub(super) fn recall_query_text(&self) -> Option<Cow<'_, str>> {
        // The one gate both recall roads (streaming and synchronous) pass: a
        // gateway that refused recalled memory this session is asked without it.
        if self.recall_withheld() {
            return None;
        }
        let latest = self.latest_user_text()?;
        // Keep one substantive earlier user turn in the lexical query even
        // when the latest request is long. A long request can still refer to
        // context from the preceding turn, while assistant/tool text must not
        // become recall input. Short confirmations use this same path.
        let mut saw_latest_user = false;
        for message in self.session.messages.iter().rev() {
            if message.role != MessageRole::User {
                continue;
            }
            let Some(text) = message_text(message) else {
                continue;
            };
            if !saw_latest_user {
                saw_latest_user = true;
                continue;
            }
            if is_short_follow_up(text.as_ref()) {
                continue;
            }
            return Some(Cow::Owned(format!("{}\n{}", text.trim(), latest.trim())));
        }

        Some(latest)
    }

    pub(super) fn build_request(
        &mut self,
        tool_choice: Option<::api::ToolChoice>,
    ) -> Result<ApiRequest, RuntimeError> {
        let wire_reminders = self.request_wire_reminders();
        // Persist the reminder set into the transcript BEFORE the overflow
        // guard, so the guard's session estimate covers it, and pass an empty
        // wire set to assembly — the reminders now ride `request.messages` as
        // a trailing System message with a stable position (the wire-only
        // injection re-billed the previous request's tail every turn).
        self.absorb_wire_reminders_into_session(&wire_reminders);
        self.enforce_request_overflow_guard(&[])?;
        Ok(self.assemble_request(Arc::from(Vec::new()), tool_choice))
    }

    /// Emergency context-overflow guard: if the estimated payload exceeds the
    /// model window (minus output headroom), compact now so the request does
    /// not 400. **Synchronous and potentially expensive** — a fired guard runs
    /// `compact_with_api_fallback`, a blocking LLM summary round-trip on the
    /// caller's thread. On the streaming path that thread is the TUI `select!`
    /// loop, so the streaming caller runs async preflight compaction *before*
    /// building the request and then assembles via [`Self::assemble_request`]
    /// directly, skipping this guard. The synchronous `run_turn` paths (no async
    /// preflight) still call it through [`Self::build_request`]. Split out of
    /// `build_request` so the two responsibilities — mutate-to-fit vs. assemble
    /// a snapshot — read separately (SRP).
    pub(super) fn enforce_request_overflow_guard(
        &mut self,
        wire_reminders: &[String],
    ) -> Result<(), RuntimeError> {
        let profile = crate::turn_profiling_enabled();
        let budget = self.request_context_budget();
        let estimated = self.estimated_request_context_with_reminders(wire_reminders);
        if estimated <= budget {
            return Ok(());
        }
        let guard_t = profile.then(std::time::Instant::now);
        // Emergency overflow compaction is not user-directed: no focus.
        let result = self.compact_with_api_fallback(
            CompactionConfig {
                max_estimated_tokens: 0,
                ..CompactionConfig::default()
            },
            None,
        );
        if result.removed_message_count > 0 {
            self.session = result.compacted_session;
        }

        // Re-check after compaction — if still over budget, aggressively
        // trim preserved messages down to the most recent pair. A pair can
        // still itself be enormous, so a final check below rejects it locally
        // rather than sending a provider-bound request that must 400.
        if self.estimated_request_context_with_reminders(wire_reminders) > budget
            && self.session.messages.len() > 2
        {
            let result = self.compact_with_api_fallback(
                CompactionConfig {
                    max_estimated_tokens: 0,
                    preserve_recent_messages: 2,
                },
                None,
            );
            if result.removed_message_count > 0 {
                self.session = result.compacted_session;
            }
        }
        if let Some(t) = guard_t {
            log_build_segment("overflow_guard_compaction (BLOCKING LLM)", t);
        }
        self.ensure_request_context_budget(wire_reminders)
    }

    pub(super) fn ensure_request_context_budget(
        &self,
        wire_reminders: &[String],
    ) -> Result<(), RuntimeError> {
        let budget = self.request_context_budget();
        let estimated = self.estimated_request_context_with_reminders(wire_reminders);
        if estimated <= budget {
            return Ok(());
        }
        Err(RuntimeError::new(format!(
            "request remains over the context budget after compaction ({estimated} estimated tokens > {budget} token budget); reduce the latest tool result or start a new session before retrying"
        )))
    }

    fn request_context_budget(&self) -> u64 {
        self.context_window.saturating_sub(DEFAULT_MAX_OUTPUT_TOKENS)
    }

    fn estimated_request_context_with_reminders(&self, wire_reminders: &[String]) -> u64 {
        estimate_session_tokens(&self.session) as u64
            + estimate_system_prompt_tokens(&self.system_prompt)
            + estimate_system_prompt_tokens(wire_reminders)
    }

    /// Assemble the request snapshot from current state. Pure and cheap — no
    /// compaction, no LLM call, no deep clone — so it is safe to call on the TUI
    /// `select!` thread every streaming iteration. `wire_reminders` is passed in
    /// (already query-recalled by the caller) so a streaming caller computes it
    /// once and reuses it for both the overflow estimate and the request.
    pub(super) fn assemble_request(
        &self,
        wire_reminders: Arc<[String]>,
        tool_choice: Option<::api::ToolChoice>,
    ) -> ApiRequest {
        // `session.messages` 는 `Arc<Vec<_>>` 이므로 요청 스냅샷은 전체 메시지
        // 를 deep clone 하지 않고 `Arc::clone`(포인터 복사) 으로 공유한다.
        // 메시지 변경은 `Session` 이 `Arc::make_mut`(COW) 로 처리하므로 이
        // 스냅샷은 그 시점 이력의 불변 뷰로 안전하다. (종전의 길이-기반
        // 캐시 + `Arc::new(messages.clone())` deep clone 을 대체.)
        //
        // `tool_consistent_messages` 는 turn 이 mid-flight 로 취소돼 결과 없이
        // 남은 `tool_use`(고아) 를 합성 `tool_result` 로 봉인한 뷰를 돌려준다.
        // 봉인할 게 없으면 위 `Arc::clone` 그대로(제로 카피). 이게 없으면
        // 다음 요청이 `400 tool_use ... without tool_result` 로 거절돼 세션이
        // 영구 손상된다.
        ApiRequest {
            system_prompt: Arc::clone(&self.system_prompt),
            wire_reminders,
            messages: self.session.tool_consistent_messages(),
            tool_choice,
            // Effort floor for this turn (deep-gate escalation); `None` on an
            // ordinary turn leaves the client's configured effort unchanged.
            effort_override: self.effort_override,
            // The step governor's effort for this request, when one is
            // installed and applies; `None` leaves the client's own effort.
            effort_step: self.step_effort_for_request(),
            // Per-turn wire-model override: the refusal fallback wins over a
            // confidence-cascade escalation (a refusal on the escalated model
            // must still swap to the safe fallback); `None` on an ordinary
            // turn leaves the client's bound model in use.
            //
            // Suppressed entirely whenever this request will NOT dispatch on
            // the main bound client: a deep PLAN/VERIFY leg runs on its swapped
            // client, and a quota-fallback turn runs on a different provider's
            // client — an override from
            // the main model's world riding either wire is a foreign model
            // id (400/404) or silently hijacks the verifier.
            model_override: if self.deep_plan_leg_active
                || self.deep_verify_leg_active
                || self.quota_fallback_active
            {
                None
            } else {
                // One precedence, owned by the wire-model resolver (see
                // `bound_client_model_override`), so the footer's answer to
                // "which model is on the wire" and the request's model id
                // cannot drift apart.
                self.bound_client_model_override()
            },
        }
    }

    pub(super) fn record_assistant_iteration(
        &self,
        iteration: usize,
        assistant_message: &ConversationMessage,
        pending_tool_use_count: usize,
    ) {
        let Some(session_tracer) = &self.session_tracer else {
            return;
        };
        session_tracer.record(
            "assistant_iteration_completed",
            trace_attrs(json!({
                "iteration": iteration,
                "assistant_blocks": assistant_message.blocks.len(),
                "pending_tool_use_count": pending_tool_use_count,
            })),
        );
    }

    pub(super) fn record_tool_started(&self, iteration: usize, tool_name: &str) {
        let Some(session_tracer) = &self.session_tracer else {
            return;
        };
        session_tracer.record(
            "tool_execution_started",
            trace_attrs(json!({"iteration": iteration, "tool_name": tool_name})),
        );
        session_tracer.record_security_audit(
            "tool_execution_started",
            trace_attrs(json!({"iteration": iteration, "tool_name": tool_name})),
        );
    }

    /// Settle one tool result: fold its verified-state facts into the session
    /// ledger, retire a now-stale standing observation, and trace it.
    ///
    /// `tool_input` is the EFFECTIVE input the executor ran (post-`PreToolUse`
    /// hook), which is where a bash command's text comes from — the tool RESULT
    /// carries only the exit shape. Both turn loops have it in scope at this
    /// call, which is why the ledger can record here at all: the turn-end fold
    /// it replaces had to join `tool_use` inputs by id after the fact, and by
    /// then a VERIFY leg opened mid-turn had already read a stale ledger.
    pub(super) fn record_tool_finished(
        &mut self,
        iteration: usize,
        result_message: &ConversationMessage,
        tool_input: &str,
    ) {
        // Record BEFORE retiring: the retirement below is a reaction to the very
        // mutation this call just put on record, and a reader between the two
        // must never see the retirement without the fact that caused it.
        self.record_verified_state_from_tool(result_message, tool_input);
        // The step governor reads the same settled result: the batch it is
        // deciding the next request's effort from.
        self.note_step_tool_result(result_message, tool_input);
        // A turn-start verified-state observation is only true until this turn
        // mutates something, and the reminder absorber deliberately RE-ANCHORS
        // an unchanged set near the tail once it drifts past its dedupe window
        // — so a block left standing would eventually re-assert "nothing edited
        // since" BELOW the very edits that falsified it. Retire it at the first
        // successful mutation instead. The copy already persisted at turn-start
        // position stays (it is true where it sits); only the re-assertion
        // stops. `&mut self` exists for this — both turn loops settle every
        // tool result through here.
        self.retire_verified_state_observation_on_mutation(result_message);
        let Some(session_tracer) = &self.session_tracer else {
            return;
        };
        let Some(ContentBlock::ToolResult {
            tool_name,
            output,
            is_error,
            ..
        }) = result_message.blocks.first()
        else {
            return;
        };
        session_tracer.record(
            "tool_execution_finished",
            trace_attrs(json!({
                "iteration": iteration,
                "tool_name": tool_name,
                "is_error": is_error,
            })),
        );
        let mut audit_attrs = trace_attrs(json!({
            "iteration": iteration,
            "tool_name": tool_name,
            "is_error": is_error,
        }));
        if *is_error {
            if let Some(preview) = tool_error_preview(output) {
                audit_attrs.insert("error_preview".to_string(), Value::String(preview));
            }
        }
        session_tracer.record_security_audit("tool_execution_finished", audit_attrs);
    }

    pub(super) fn record_turn_completed(&mut self, summary: &TurnSummary) {
        // Close the verified-state turn. The facts themselves were recorded at
        // the tool-result seam as they settled (`record_tool_finished`), so this
        // only advances the ledger's turn counter — folding the summary here as
        // well would record every check of this turn a second time.
        self.note_verified_state_turn_boundary();

        // Externalize the turn into the durable, compaction-proof trace under
        // `.zo/turns/` (Harness-1: state lives outside the context window).
        // Best-effort: a recording failure must never affect the turn. This runs
        // regardless of whether an OTLP tracer is attached — the durable trace is
        // the audit trail, independent of telemetry export. Rooted at the
        // session's stable workspace so it survives `EnterWorktree` chdirs.
        if let Some(cwd) = self.trace_cwd() {
            let _ = crate::turn_trace::record_completed(
                &cwd,
                &self.session.session_id,
                summary,
                self.session.session_goal.as_deref(),
            );
        }

        let Some(session_tracer) = &self.session_tracer else {
            return;
        };
        session_tracer.record(
            "turn_completed",
            trace_attrs(json!({
                "iterations": summary.iterations,
                "assistant_messages": summary.assistant_messages.len(),
                "tool_results": summary.tool_results.len(),
                "prompt_cache_events": summary.prompt_cache_events.len(),
                // Token counts feed the OTLP `zo_code.token.usage` metric
                // (CC monitoring parity) — keep the attr names stable.
                "input_tokens": summary.usage.input_tokens,
                "output_tokens": summary.usage.output_tokens,
                "cache_read_input_tokens": summary.usage.cache_read_input_tokens,
                "cache_creation_input_tokens": summary.usage.cache_creation_input_tokens,
            })),
        );
    }

    pub(super) fn record_turn_failed(&mut self, iteration: usize, error: &RuntimeError) {
        if let Some(cwd) = self.trace_cwd() {
            let _ = crate::turn_trace::record_terminal(
                &cwd,
                &self.session.session_id,
                crate::turn_trace::TurnOutcome::Failed,
                iteration,
                self.session.session_goal.as_deref(),
            );
            // Segment the candidate by error signature (bounded class + status
            // + provider code), not by one fixed summary: the summary keys the
            // candidate id, and a single "turn failed" bucket mixed every root
            // cause into one candidate no advisor could act on. The evidence
            // detail keeps the leading error text (bounded by the recorder) so
            // the fusion root-cause advisor has concrete signals to cite.
            let signature = decision_core::dreamer::error_signature_label(
                error.failure_signature(),
                &error.to_string(),
            );
            let _ = crate::memory::record_self_improve_pulse_if_enabled(
                self.dream_automation_enabled,
                &cwd,
                decision_core::dreamer::CandidateKind::TurnFailure,
                &self.session.session_id,
                "turn",
                &format!("turn failure: {signature}"),
                &error.to_string(),
                false,
            );
        }

        let Some(session_tracer) = &self.session_tracer else {
            return;
        };
        session_tracer.record(
            "turn_failed",
            trace_attrs(json!({"iteration": iteration, "error": error.to_string()})),
        );
    }

    fn record_turn_stopped(
        &mut self,
        iteration: usize,
        reason: &str,
        origin: TurnStopOrigin,
    ) {
        if let Some(cwd) = self.trace_cwd() {
            let _ = crate::turn_trace::record_terminal(
                &cwd,
                &self.session.session_id,
                origin.trace_outcome(),
                iteration,
                self.session.session_goal.as_deref(),
            );
            let _ = crate::memory::record_self_improve_pulse_if_enabled(
                self.dream_automation_enabled,
                &cwd,
                origin.candidate_kind(),
                &self.session.session_id,
                "turn",
                origin.summary(),
                reason,
                false,
            );
        }

        let Some(session_tracer) = &self.session_tracer else {
            return;
        };
        session_tracer.record(
            origin.trace_event(),
            trace_attrs(json!({"iteration": iteration, "reason": reason})),
        );
    }

    pub(super) fn record_turn_cancelled(&mut self, iteration: usize, reason: &str) {
        self.record_turn_stopped(iteration, reason, TurnStopOrigin::User);
    }

    pub(super) fn record_turn_host_failure(&mut self, iteration: usize, reason: &str) {
        self.record_turn_stopped(iteration, reason, TurnStopOrigin::HostFailure);
    }

    /// Trace a host-side consumer failure and yield the matching streaming
    /// cancellation error. Explicit abort signals use `record_turn_cancelled`
    /// instead, so only intentional cancellation becomes non-actionable.
    pub(super) fn cancel_turn(&mut self, iteration: usize, reason: &str) -> StreamingTurnError {
        self.record_turn_host_failure(iteration, reason);
        StreamingTurnError::Cancelled
    }
}

fn tool_error_preview(output: &str) -> Option<String> {
    const MAX_CHARS: usize = 240;

    let preview = output
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or(output)
        .trim();
    if preview.is_empty() {
        return None;
    }
    Some(preview.chars().take(MAX_CHARS).collect())
}

/// Emit one `ZO_PROFILE_TURN` segment line for a `build_request` sub-step
/// that took a non-trivial amount of wall-clock time. Kept here (not inline) so
/// `build_request` reads as orchestration and the >=10ms noise floor lives in
/// one place. Only ever called when profiling is already enabled, so the format
/// cost is off the hot path.
fn log_build_segment(label: &str, started_at: std::time::Instant) {
    let ms = started_at.elapsed().as_millis();
    if ms >= 10 {
        eprintln!("[TURN-SEG]   build_request/{label} = {ms}ms (synchronous; starves render_tick)");
    }
}

fn message_text(message: &ConversationMessage) -> Option<Cow<'_, str>> {
    let mut texts = message.blocks.iter().filter_map(|block| match block {
        ContentBlock::Text { text } if !text.trim().is_empty() => Some(text.as_str()),
        _ => None,
    });
    let first = texts.next()?;
    let Some(second) = texts.next() else {
        return Some(Cow::Borrowed(first));
    };

    let mut joined = String::with_capacity(first.len() + second.len() + 1);
    joined.push_str(first);
    joined.push('\n');
    joined.push_str(second);
    for text in texts {
        joined.push('\n');
        joined.push_str(text);
    }
    Some(Cow::Owned(joined))
}

fn is_short_follow_up(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return false;
    }
    let lower = trimmed.to_lowercase();
    matches!(
        lower.as_str(),
        "y" | "yes" | "ok" | "okay" | "continue" | "계속" | "ㅇㅇ"
    ) || is_numeric_choice(&lower)
}

fn is_numeric_choice(text: &str) -> bool {
    let digits = text.strip_suffix("번").unwrap_or(text);
    matches!(digits, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9")
}

#[cfg(test)]
mod tests {
    use super::TurnStopOrigin;

    #[test]
    fn stop_origin_keeps_user_cancellation_separate_from_host_failure() {
        assert_eq!(
            TurnStopOrigin::User.candidate_kind(),
            decision_core::dreamer::CandidateKind::UserCancelled
        );
        assert!(!TurnStopOrigin::User.candidate_kind().is_actionable());
        assert_eq!(
            TurnStopOrigin::HostFailure.candidate_kind(),
            decision_core::dreamer::CandidateKind::TurnFailure
        );
        assert!(TurnStopOrigin::HostFailure.candidate_kind().is_actionable());
        assert_eq!(
            TurnStopOrigin::User.trace_outcome(),
            crate::turn_trace::TurnOutcome::Cancelled
        );
        assert_eq!(
            TurnStopOrigin::HostFailure.trace_outcome(),
            crate::turn_trace::TurnOutcome::Failed
        );
    }
}

#[cfg(test)]
mod bench_fixture_tests {
/// r49 — the reminder bench's own fixture, checked before it costs a turn.
///
/// The bench's two arms differ by one block, so the block has to carry
/// something the other arm does not already have. That is only true if the
/// planted pitfall lesson ranks BELOW the recalled-memory section's five for
/// the task's own prompt: rank it first and both arms see it, the arms are
/// identical, and the bench measures nothing while looking like it measured
/// something.
///
/// So this reads the checked-in vault the bench points `zo` at, splits one
/// recall exactly as a turn does, and pins where the lesson lands. It runs in
/// the gate, with no provider and no quota, which is the point: a fixture that
/// stopped separating the arms should fail here and not two hours into a run.
#[test]
fn the_reminder_bench_fixture_keeps_its_pitfall_out_of_the_recall_section() {
    use crate::memory::LexicalMemoryRetriever;
    use core_types::MemoryRetriever as _;

    let vault_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../bench/tasks/17-pipe-exit-code/vault");
    assert!(
        vault_root.is_dir(),
        "the bench vault moved: {}",
        vault_root.display()
    );
    let vault = crate::second_brain::SecondBrain::at(&vault_root);
    let corpus = crate::second_brain::corpus::scan(&vault);
    assert_eq!(corpus.pages.len(), 9, "eight pages and an index");

    // The task's prompt, verbatim from `tasks/17-pipe-exit-code/prompt.txt`.
    let query = "./run_tests.sh fails. Fix gate_check.sh so it passes, without changing run_tests.sh.";
    let retriever = LexicalMemoryRetriever::from_index_markdown("# Zo memory\n").with_corpus(corpus);
    let hits = retriever.recall(query, super::RECALL_AND_REMINDER_LIMIT);
    let sections = super::recall_and_reminder_sections(&hits);

    let pitfall = "gotcha-pipeline-hides-a-failure";
    let recalled = sections
        .iter()
        .find(|section| section.starts_with("# Recalled memory"))
        .expect("the fixture recalls something");
    let reminders = sections
        .iter()
        .find(|section| section.starts_with(crate::prompt::REMINDERS_SECTION_HEADING))
        .unwrap_or_else(|| {
            panic!(
                "no reminder block — the on arm would equal the off arm.\nhits: {:#?}",
                hits.iter().map(|hit| &hit.entry.slug).collect::<Vec<_>>()
            )
        });

    assert!(
        !recalled.contains(pitfall),
        "the pitfall is already in the recall section, so both arms see it:\n{recalled}"
    );
    assert!(
        reminders.contains(pitfall),
        "the pitfall must be what the reminder block carries:\n{reminders}\nhits: {:#?}",
        hits.iter().map(|hit| &hit.entry.slug).collect::<Vec<_>>()
    );

    // And the off arm is this arm minus exactly that block.
    let off = crate::test_env_lock();
    std::env::set_var("ZO_REMINDERS", "0");
    let without = super::recall_and_reminder_sections(&hits);
    std::env::remove_var("ZO_REMINDERS");
    drop(off);
    assert_eq!(
        without,
        vec![recalled.clone()],
        "the switch must take the block and nothing else"
    );
}
}
