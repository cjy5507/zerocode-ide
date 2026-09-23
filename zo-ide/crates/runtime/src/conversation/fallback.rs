//! Refusal and cross-provider quota fallback for [`ConversationRuntime`], split
//! out of `mod.rs` so the turn loops there read as orchestration.
//! Behaviour-preserving: these were `ConversationRuntime` methods and
//! module-level helpers, now `pub(super)` where the loops in `mod.rs` still
//! reach them.

use std::sync::Arc;

use tokio::sync::mpsc;

use crate::message_stream::types::{BlockIdGen, RenderBlock, WireModelSource};
use crate::session::{ContentBlock, ConversationMessage};

use super::{
    ApiClient, ApiRequest, AssistantEvent, AsyncApiClient, ConversationRuntime, ModelSwitch,
    RuntimeError, ToolExecutor, DEFAULT_STREAMING_CHANNEL_CAPACITY,
};
use crate::model_router::SwitchTrigger;

/// The SAME-provider model a safety-classifier refusal on `model` retries on,
/// as the catalog declares it: the first candidate in the lineup's
/// `refusal_fallback` list served by `model`'s own provider (Fable/Sonnet/Haiku
/// → the Opus head). `None` when the lineup declares none, or when every
/// candidate is on another provider (Opus itself) — that case is handled by the
/// cross-provider handoff, never by a bound-client model override, since putting
/// a foreign model id on the Anthropic client would 400. Nothing here names a
/// lineup; which classifier declines and which family stands in are catalog
/// facts, so a new lineup is a catalog edit and never a rebuild.
fn refusal_fallback_for(model: &str) -> Option<String> {
    let provider = api::detect_provider_kind(model);
    api::refusal_fallback_candidates(model)
        .into_iter()
        .find(|candidate| api::detect_provider_kind(candidate) == provider)
}
/// System-level warn naming the same-provider model a refusal was auto-retried
/// on (Fable/Sonnet/Haiku → the Opus head). Wraps the shared vocabulary in
/// [`core_types::retry_signal`] so every notice lives in one place.
pub(super) fn refusal_fallback_warn(target: &str) -> String {
    core_types::retry_signal::refusal_fallback_warn(target)
}
/// System-level warn naming the cross-provider model this doubly-refused turn is
/// handed to, like a quota fallback.
pub(super) fn refusal_cross_provider_warn(target: &str) -> String {
    core_types::retry_signal::refusal_cross_provider_warn(target)
}
/// System-level notice for the P3 context-cleaning retry (no fallback left).
pub(super) const REFUSAL_CONTEXT_CLEANED_WARN: &str = core_types::REFUSAL_CONTEXT_CLEANED_WARN;
/// System-level notice shown when a refusal cannot be auto-retried: the turn is
/// already on a fallback that refused too, the active model is non-Anthropic, or
/// every avenue (same-model retry, provider handoff, context cleaning) is spent.
/// Surfaced honestly instead of looping forever. Names `/model` for another
/// provider as the concrete next step, per P4.
pub(super) const REFUSAL_SURFACED_NOTICE: &str =
    "The model's safety classifier declined this request; giving up on automatic \
     retries. Rephrasing usually clears it, or /model to another provider — on a long \
     session, /compact can too.";

/// System-level warn shown when an Anthropic refusal with no same-provider
/// fallback is retried once on the same model (the classifier samples).
pub(super) const REFUSAL_SAME_MODEL_RETRY_WARN: &str =
    "The model's safety classifier declined this response — retrying once (the \
     classifier is sampling-dependent, so an identical retry often passes).";

/// Consecutive public turns that must hit the refusal fallback before the
/// session stops probing Fable/Mythos on every new turn. Two catches a sticky
/// context-triggered classifier without parking the session after one isolated
/// refusal. See [`ConversationRuntime::refusal_consecutive_turns`].
const REFUSAL_DRY_TURN_THRESHOLD: u8 = 2;

/// Session-scoped refusal cooldown after [`REFUSAL_DRY_TURN_THRESHOLD`]
/// consecutive public turns. Thirty minutes avoids paying one doomed Fable
/// request per turn in a long classifier-sticky session while still probing
/// Fable again automatically. Process memory only, like the quota cooldown.
const REFUSAL_DRY_COOLDOWN: std::time::Duration = std::time::Duration::from_secs(30 * 60);

/// Session-scoped cooldown applied after a quota fallback fires when the
/// provider gave no `retry_after` hint. Fifteen minutes: long enough to ride
/// out a typical subscription/quota throttle window without re-spending the
/// main model's rate-limit retry budget every turn, short enough that a
/// transient exhaustion never parks the whole session off its configured model
/// for the rest of the day. While the session is inside this window every turn
/// pre-arms straight onto the fallback client; once it elapses the session
/// returns to the main model on its own. See [`ConversationRuntime::quota_dry_until`].
const QUOTA_FALLBACK_DEFAULT_COOLDOWN: std::time::Duration =
    std::time::Duration::from_secs(15 * 60);

/// Ceiling on a provider-supplied `retry_after` when it sets the pre-arm
/// window. One hour, against a 15-minute default.
///
/// Once it elapses the session simply tries the main model again; if the wall
/// is still up that 429 re-arms with a fresh hint. So the cost of a ceiling
/// that is too short is one probe request per hour, while the cost of no
/// ceiling is a whole session spent on a model the user did not choose. `api`
/// bounds every other provider-hinted window for the same reason — a garbage
/// or weekly-scale hint must not mute a provider for the process lifetime.
const QUOTA_FALLBACK_MAX_COOLDOWN: std::time::Duration = std::time::Duration::from_secs(60 * 60);

/// How long the session pre-arms onto the fallback after a quota escape.
fn quota_fallback_cooldown(retry_after: Option<std::time::Duration>) -> std::time::Duration {
    retry_after.map_or(QUOTA_FALLBACK_DEFAULT_COOLDOWN, |hint| {
        hint.min(QUOTA_FALLBACK_MAX_COOLDOWN)
    })
}

/// Loud warn line for the mid-turn quota swap: the main model's quota window is
/// exhausted, so this turn continues on the cross-provider fallback and the
/// session returns to the main model once the recorded cooldown clears.
pub(super) fn quota_fallback_swap_warn(model: &str) -> String {
    format!(
        "{prefix}{model}; the main model is rate-limited (quota exhausted), so this turn \
         continues on {model}. The main model resumes automatically once the cooldown clears.",
        prefix = core_types::QUOTA_FALLBACK_ACTIVE_NOTICE_PREFIX,
    )
}

/// Short info line for a turn that pre-arms onto the fallback because the
/// session is still inside the quota-dry cooldown from an earlier turn.
///
/// Names the cooling-down model and the time left whenever the caller knows
/// them: the bare "the main model is still cooling down from a rate limit"
/// read as a statement about the *fallback* provider's quota to at least one
/// user ("luna has quota left, why is this showing?") — the rate limit is the
/// main model's, and the parenthetical bound is what makes the parking
/// legible as temporary.
pub(super) fn quota_fallback_prearm_info(
    model: &str,
    main_model: Option<&str>,
    remaining: Option<std::time::Duration>,
) -> String {
    let main = main_model.filter(|main| !main.is_empty()).unwrap_or("the main model");
    let left = remaining
        .filter(|left| !left.is_zero())
        .map(|left| format!(" ({} left)", human_wait(left)))
        .unwrap_or_default();
    format!(
        "{prefix}{model}; {main} is still cooling down from a rate limit{left}, so this turn \
         continues on {model}.",
        prefix = core_types::QUOTA_FALLBACK_ACTIVE_NOTICE_PREFIX,
    )
}

/// `92s` → `~2m`, `45s` → `45s`: the coarse human wait used by the quota
/// notices. Sub-second waits round up to `1s` so a live notice never claims a
/// zero wait.
fn human_wait(wait: std::time::Duration) -> String {
    core_types::retry_signal::human_reset_wait(wait)
}

/// Loud warn line for holding the turn on the main model instead of falling
/// back: the quota window lifts within the configured wait band, so the turn
/// waits it out rather than switching providers.
/// Warn line for a turn the provider shed, continuing on a lighter tier of the
/// same provider. Names both models: the user chose the heavier one, and a silent
/// swap would make the reply's quality look like the model they picked.
pub(super) fn overload_demotion_warn(shed: &str, lighter: &str) -> String {
    format!(
        "Provider capacity refused {shed} (not your rate limit — the window is not the wall), \
         so this turn continues on {lighter}. The next turn starts on {shed} again."
    )
}

pub(super) fn quota_wait_hold_warn(model: &str, wait: std::time::Duration) -> String {
    let human = human_wait(wait);
    // Starts with the shared prefix so the TUI can flip the spinner into the
    // quota-hold state — see `core_types::QUOTA_HOLD_NOTICE_PREFIX`.
    format!(
        "{prefix} ({model}); its quota window resets in {human}, so this turn \
         holds on {model} rather than switching providers. Press esc to interrupt.",
        prefix = core_types::QUOTA_HOLD_NOTICE_PREFIX,
    )
}

/// Which installed cross-provider client this turn has swapped onto, and why.
///
/// The swap machinery is shared: a quota-exhausted turn and a doubly-refused
/// turn both continue on a different provider's client through the same
/// `active_async_client` / `sync_stream_events` / wire-model path. This says
/// which of the two armed it, so `active_async_client` picks the matching
/// installed client and the wire badge shows the right reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CrossFallback {
    /// The main model's quota window is exhausted (see [`QuotaEscape::Fallback`]).
    Quota,
    /// The model's safety classifier declined and the catalog's refusal
    /// fallback for it is on another provider (see [`RefusalDecision::CrossProvider`]).
    Refusal,
}

/// How the turn loop should escape a hard `RateLimit` on the main model.
pub(super) enum QuotaEscape {
    /// Hold on the main model: its exhausted window lifts within the wait band.
    /// The caller sleeps this long (the TUI keeps its live elapsed + interrupt
    /// affordance), then re-requests the SAME turn on the SAME model — no
    /// provider swap and no session cooldown recorded.
    Wait(std::time::Duration),
    /// Swap this turn onto the cross-provider fallback (`model`) and re-request.
    Fallback(String),
    /// Re-request this turn on a lighter model of the SAME provider (`model`),
    /// because the provider is shedding the heavier one.
    ///
    /// Preferred over [`Self::Fallback`] for a provider overload: it is the
    /// smaller change (same provider, same credentials, same tool surface), and
    /// the evidence says the tier is the wall — every Opus request was refused
    /// while Haiku on the same account answered in the same second. Costs one
    /// demotion per turn; a lighter tier that is shed too escalates to the
    /// cross-provider fallback on the next refusal.
    Lighter(String),
    /// Neither applies — the caller fails the turn as it did before the feature.
    None,
}

/// Whether a provider stop reason is a safety-classifier refusal. Case- and
/// whitespace-insensitive so a provider reporting `"Refusal"` or a padded value
/// still matches. Only Anthropic emits this; the fallback path additionally
/// gates on the active model's provider (see [`is_anthropic_model`]) so a
/// non-Anthropic provider is never affected even if one ever reported it.
pub(super) fn is_refusal_stop_reason(reason: &str) -> bool {
    reason.trim().eq_ignore_ascii_case("refusal")
}

/// Whether `model` is served by Anthropic — the only provider that emits a
/// `refusal` stop reason. Gates the refusal-fallback path so Gemini/OpenAI/
/// `DeepSeek` are never touched. The provider is the catalog's answer for
/// the id (aliases and dated variants included), not a family substring.
fn is_anthropic_model(model: &str) -> bool {
    api::detect_provider_kind(model) == api::ProviderKind::Anthropic
}

/// How the turn loop reacts to a `stop_reason: "refusal"`.
pub(super) enum RefusalDecision {
    /// Not a refusal this runtime handles (non-Anthropic or unknown model) —
    /// consume the turn through the ordinary content/empty path.
    Proceed,
    /// Fell back to the refused model's same-provider candidate for this turn
    /// (the override is now set — Fable/Sonnet/Haiku → the Opus head); the
    /// caller drops the refused partial and re-requests the same turn once.
    Retry,
    /// Re-request the same turn once on the *same* model. The classifier is
    /// sampling-dependent, so a decline of a benign request frequently passes on
    /// an identical second attempt — tried before any cross-provider handoff for
    /// a model with no same-provider fallback (Opus itself), since it is free.
    RetrySameModel,
    /// Handed this turn to the installed cross-provider refusal client (the
    /// override state is now armed): the same-model retry declined too and the
    /// catalog's next candidate is on another provider. The caller drops the
    /// refused partial and re-requests, which now dispatches on that client.
    CrossProvider,
    /// No fallback is left, but the earlier declined exchange is still in
    /// history dragging the classifier down. Drop it and re-request the same
    /// model once. Capped at one per turn.
    RetryCleaned,
    /// Cannot retry (every avenue spent). Surface a notice and end the turn.
    Surface,
}

/// The synthetic assistant message recorded when a refusal is surfaced (rather
/// than retried), so the turn is well-formed — a user turn is never left with no
/// assistant response — and the notice is visible on the headless path.
pub(super) fn refusal_surfaced_message() -> ConversationMessage {
    ConversationMessage::assistant(vec![ContentBlock::Text {
        text: REFUSAL_SURFACED_NOTICE.to_string(),
    }])
}
/// Drive an [`AsyncApiClient`] to completion from a synchronous context and
/// return its assembled [`AssistantEvent`] sequence, discarding the render
/// deltas. Used only by the sync turn loop's quota-fallback dispatch
/// ([`ConversationRuntime::sync_stream_events`]): the cross-provider fallback
/// client is async, but `run_turn_once` runs without an ambient runtime on the
/// headless `-p` text path. Mirrors the runtime-flavor-aware sync→async bridge
/// in [`crate::bash`] so it is correct whether called with no runtime (fresh
/// current-thread runtime), inside a multi-thread runtime (`block_in_place`), or
/// inside a current-thread runtime such as a `#[tokio::test]` (a dedicated
/// thread, since `block_in_place` panics on single-threaded executors).
fn block_on_async_client(
    client: Arc<dyn AsyncApiClient>,
    request: ApiRequest,
) -> Result<Vec<AssistantEvent>, RuntimeError> {
    // A bounded render channel drained concurrently with the stream: the client
    // "should short-circuit as soon as the next send().await fails", so the
    // receiver must stay alive until the stream finishes. Discard every block.
    async fn drive(
        client: Arc<dyn AsyncApiClient>,
        request: ApiRequest,
    ) -> Result<Vec<AssistantEvent>, RuntimeError> {
        let (tx, mut rx) = mpsc::channel::<RenderBlock>(DEFAULT_STREAMING_CHANNEL_CAPACITY);
        let block_id = BlockIdGen::default().next();
        let stream = client.stream_async(request, tx, block_id);
        tokio::pin!(stream);
        loop {
            tokio::select! {
                result = &mut stream => {
                    while rx.try_recv().is_ok() {}
                    return result;
                }
                _ = rx.recv() => {}
            }
        }
    }

    use tokio::runtime::{Builder, Handle, RuntimeFlavor};
    if let Ok(handle) = Handle::try_current() {
        return if handle.runtime_flavor() == RuntimeFlavor::CurrentThread {
            std::thread::spawn(move || {
                let runtime = Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|error| RuntimeError::new(error.to_string()))?;
                runtime.block_on(drive(client, request))
            })
            .join()
            .map_err(|_| RuntimeError::new("quota fallback bridge thread panicked"))?
        } else {
            tokio::task::block_in_place(|| handle.block_on(drive(client, request)))
        };
    }
    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| RuntimeError::new(error.to_string()))?;
    runtime.block_on(drive(client, request))
}

impl<C, T> ConversationRuntime<C, T>
where
    C: ApiClient,
    T: ToolExecutor,
{
    /// Install (or clear with `None`) the cross-provider client the turn loop
    /// swaps to when the main model's subscription/quota window is exhausted,
    /// together with its model id. The host re-derives and re-installs this on
    /// every turn entry — the top-ranked alternative on a *different* provider —
    /// so a model switch, a `/smart quota-fallback off`, or a change in
    /// connected providers never leaves a stale fallback installed. `None`
    /// disables the feature for the turn (a quota-exhausted turn then fails as
    /// it did before this feature existed). Mirrors [`Self::set_deep_verify_client`].
    pub fn set_quota_fallback_client(
        &mut self,
        client: Option<(Arc<dyn AsyncApiClient>, String)>,
    ) {
        self.quota_fallback_client = client;
    }

    /// Install (or clear with `None`) the cross-provider client the refusal
    /// path hands the turn to when the active model's classifier declines and
    /// its catalog `refusal_fallback` list names a candidate on another
    /// provider (Opus → an OpenAI/Google peer). The host resolves and installs
    /// this every turn entry from [`api::refusal_fallback_candidates`], exactly
    /// as it installs the quota fallback, so a model switch or a change in
    /// connected providers never leaves a stale client. `None` leaves a
    /// cross-provider refusal with nowhere to go — the turn then surfaces (or
    /// takes the context-cleaning retry). Mirrors [`Self::set_quota_fallback_client`].
    pub fn set_refusal_fallback_client(
        &mut self,
        client: Option<(Arc<dyn AsyncApiClient>, String)>,
    ) {
        self.refusal_fallback_client = client;
    }

    /// The model id of the currently installed cross-provider refusal fallback
    /// client, or `None`. A read accessor for the host contract test.
    pub fn refusal_fallback_client_model(&self) -> Option<&str> {
        self.refusal_fallback_client.as_ref().map(|(_, model)| model.as_str())
    }

    /// The model id of the currently installed quota fallback client, or `None`
    /// when no fallback is installed. A read accessor so a host contract test
    /// can assert that a turn-entry install set (or a `None` route cleared) the
    /// fallback without reaching into private state.
    pub fn quota_fallback_model(&self) -> Option<&str> {
        self.quota_fallback_client.as_ref().map(|(_, model)| model.as_str())
    }

    /// Install (or clear) the per-turn confidence-cascade model escalation:
    /// the wire model this turn's requests run on instead of the session
    /// model. Host-managed set-or-clear at every turn entry (mirrors
    /// [`Self::set_quota_fallback_client`]) so an escalation is scoped to
    /// exactly the one armed turn. Same-provider only — this changes the wire
    /// model id on the bound client, never the client itself; the host
    /// resolver enforces the provider match.
    pub fn set_escalation_model_override(&mut self, model: Option<String>) {
        self.escalation_model_override = model.filter(|model| !model.trim().is_empty());
        // Freshness latch: the NEXT turn begin consumes this install; any
        // later begin without a fresh install clears the override (see the
        // field doc on `escalation_armed_fresh`).
        self.escalation_armed_fresh = self.escalation_model_override.is_some();
    }

    /// The per-turn wire-model escalation currently installed by the host.
    #[must_use]
    pub fn escalation_model_override(&self) -> Option<&str> {
        self.escalation_model_override.as_deref()
    }

    /// Escalation freshness at a turn begin (see `escalation_armed_fresh`):
    /// the first PUBLIC turn begin after the host installed an escalation
    /// keeps it — that IS the escalated turn; any later public begin without
    /// a fresh install drops the stale override, so a slash-command or
    /// queued-text turn on the same runtime never silently runs on a
    /// previous turn's Deep model. Internal deep-lane subturns are exempt:
    /// they run INSIDE the escalated turn (unlike the per-leg refusal reset,
    /// which deliberately gives each leg its own fallback budget).
    pub(super) fn begin_turn_escalation(&mut self, internal_subturn: bool) {
        if internal_subturn {
            return;
        }
        if self.escalation_armed_fresh {
            self.escalation_armed_fresh = false;
        } else {
            self.escalation_model_override = None;
        }
    }

    /// Set how close to a quota reset the turn loop holds on the main model
    /// instead of falling back (see [`Self::quota_wait_band`]). `ZERO` disables
    /// the band. Re-applied by the host each turn from `smart.quotaWaitBandMinutes`.
    pub fn set_quota_wait_band(&mut self, band: std::time::Duration) {
        self.quota_wait_band = band;
    }

    /// The quota-wait band currently armed on this runtime. A read accessor so a
    /// host contract test can assert that a turn-entry install refreshed the
    /// band from settings.
    pub fn quota_wait_band(&self) -> std::time::Duration {
        self.quota_wait_band
    }

    /// Whether this turn is riding an installed cross-provider fallback client
    /// (quota or refusal) rather than the native client. The one predicate the
    /// override-suppression, retry-cap and effort-stand-aside checks share, so a
    /// new cross-provider reason is one match arm, not a scattered new bool.
    pub(super) fn cross_fallback_active(&self) -> bool {
        self.active_cross_fallback.is_some()
    }

    /// The installed client the active cross-provider fallback points at:
    /// [`Self::quota_fallback_client`] for a quota swap, `refusal_fallback_client`
    /// for a refusal handoff. `None` when no cross fallback is active or its
    /// client was cleared mid-session.
    fn active_cross_fallback_client(&self) -> Option<&(Arc<dyn AsyncApiClient>, String)> {
        match self.active_cross_fallback? {
            CrossFallback::Quota => self.quota_fallback_client.as_ref(),
            CrossFallback::Refusal => self.refusal_fallback_client.as_ref(),
        }
    }

    /// The async client the current streaming request runs on: the active
    /// cross-provider fallback when this turn has swapped to one (mid-turn
    /// exhaustion, a refusal handoff, or a cooldown pre-arm), else the
    /// natively-installed streaming client. Keeps the swap in one place so the
    /// request-dispatch site stays a single expression. A cross-fallback-active
    /// turn with no client installed (impossible today — arming requires the
    /// client) degrades to the native client rather than panicking.
    pub(super) fn active_async_client(&self) -> Option<&Arc<dyn AsyncApiClient>> {
        // A deep-gate leg's swapped client (cross-model planner/verifier or the
        // Architect implementer) wins over the cross-provider fallback.
        if !self.deep_plan_leg_active
            && !self.deep_verify_leg_active
            && !self.exec_impl_leg_active
        {
            if let Some((client, _)) = self.active_cross_fallback_client() {
                return Some(client);
            }
        }
        self.async_api_client.as_ref()
    }
    /// The model the current request goes out on and the decision that put it
    /// there — the ONE answer to "which model is on the wire", most specific
    /// first: an Architect EXEC leg (the implementer), a deep PLAN or VERIFY leg
    /// (their swapped clients), the cross-provider quota fallback, then the
    /// same-provider overrides on the bound client in recency order (overload
    /// demotion, refusal fallback, escalation), and finally the session model.
    /// `None` when the host never reported a model (bare test harness).
    ///
    /// Announced to the render stream before every request (`RenderBlock::WireModel`)
    /// so an attended surface shows the wire truth, not the setting.
    pub(super) fn wire_model(&self) -> Option<(&str, WireModelSource)> {
        self.leg_wire_model()
            .or_else(|| self.cross_wire_model())
            .or_else(|| self.ordinary_wire_model())
    }

    /// A deep-gate or Architect leg's swapped model while that leg is active.
    fn leg_wire_model(&self) -> Option<(&str, WireModelSource)> {
        if self.exec_impl_leg_active {
            if let Some(contract) = &self.exec_contract {
                return Some((
                    contract.impl_model.as_str(),
                    WireModelSource::ExecImplementer,
                ));
            }
        }
        if self.deep_plan_leg_active {
            if let Some((_, model)) = &self.deep_plan_client {
                return Some((model.as_str(), WireModelSource::DeepPlan));
            }
        }
        if self.deep_verify_leg_active {
            if let Some((_, model)) = self
                .deep_verify_candidates
                .get(self.deep_verify_candidate_idx)
            {
                return Some((model.as_str(), WireModelSource::DeepVerify));
            }
        }
        None
    }

    /// The active cross-provider fallback (quota or refusal) while this turn
    /// rides it — the same condition under which `active_async_client` hands out
    /// that client. A refusal handoff wears the refusal badge (`RefusalCooldown`
    /// when a cooldown pre-armed it, else `RefusalFallback`); a quota swap wears
    /// the quota badge.
    fn cross_wire_model(&self) -> Option<(&str, WireModelSource)> {
        if self.deep_plan_leg_active || self.deep_verify_leg_active || self.exec_impl_leg_active {
            return None;
        }
        let source = match self.active_cross_fallback? {
            CrossFallback::Quota => WireModelSource::QuotaFallback,
            CrossFallback::Refusal if self.refusal_dry_until.is_some() => {
                WireModelSource::RefusalCooldown
            }
            CrossFallback::Refusal => WireModelSource::RefusalFallback,
        };
        self.active_cross_fallback_client()
            .map(|(_, model)| (model.as_str(), source))
    }

    /// The same-provider override riding the bound client, else the session
    /// model. Precedence is recency: an overload demotion is the newest and
    /// most specific verdict available — the provider just refused the model
    /// the other two overrides would select — so it wins. It is same-provider
    /// by construction (`api::starvation_demotion_model` walks one provider's
    /// ladder), so it cannot smuggle a foreign model id onto the bound client.
    fn ordinary_wire_model(&self) -> Option<(&str, WireModelSource)> {
        if let Some(model) = &self.overload_demotion_model {
            return Some((model.as_str(), WireModelSource::OverloadDemotion));
        }
        if let Some(model) = &self.refusal_fallback_model {
            let source = if self.refusal_dry_until.is_some() {
                WireModelSource::RefusalCooldown
            } else {
                WireModelSource::RefusalFallback
            };
            return Some((model.as_str(), source));
        }
        if let Some(model) = &self.escalation_model_override {
            return Some((model.as_str(), WireModelSource::Escalation));
        }
        self.context_model
            .as_deref()
            .map(|model| (model, WireModelSource::Session))
    }

    /// The model override `assemble_request` puts on the bound client: the
    /// ordinary wire model, unless that is simply the session model.
    pub(super) fn bound_client_model_override(&self) -> Option<String> {
        self.ordinary_wire_model()
            .filter(|(_, source)| *source != WireModelSource::Session)
            .map(|(model, _)| model.to_string())
    }

    /// The model id the current request runs on for REFUSAL judgement —
    /// `wire_model`, except that a deep-gate VERIFY leg's verifier is a
    /// temporary client swap the refusal path deliberately ignores: refusal
    /// routing still belongs to the ordinary turn model (unchanged
    /// pre-quota-fallback behavior), and the quota fallback is not on that wire
    /// either. `None` when the host never reported a model (bare test harness).
    ///
    /// Judging the *active* model is the consistency the two fallbacks need to
    /// coexist: a fallback on a non-Anthropic provider makes
    /// `is_anthropic_claude_model` false, so a spurious `refusal` from that
    /// provider yields `Proceed` instead of arming the Opus override (which
    /// would force an Anthropic model id onto a non-Anthropic client). Likewise
    /// during an Architect EXEC leg a GPT implementer's spurious `refusal` is
    /// judged against the GPT model on the wire.
    pub(super) fn effective_request_model(&self) -> Option<&str> {
        match self.leg_wire_model() {
            Some((_, WireModelSource::DeepVerify)) => {
                self.ordinary_wire_model().map(|(model, _)| model)
            }
            Some((model, _)) => Some(model),
            None => self
                .cross_wire_model()
                .or_else(|| self.ordinary_wire_model())
                .map(|(model, _)| model),
        }
    }

    /// Model whose provider owns capacity errors from the active stream: the
    /// wire model. Deep PLAN/VERIFY swap only the client, so the leg model must
    /// override the ordinary turn model for quota accounting — the verifier
    /// included, unlike `effective_request_model`.
    pub(super) fn rate_limit_model_for_active_stream(&self) -> Option<&str> {
        self.wire_model().map(|(model, _)| model)
    }

    /// Decide how to react to a `stop_reason: "refusal"`, walking the catalog's
    /// candidate ladder for the refused model in one place (P2):
    ///
    /// 1. A same-provider candidate not yet tried (Fable/Sonnet/Haiku → the Opus
    ///    head) arms the per-turn model override → [`RefusalDecision::Retry`].
    /// 2. No same-provider candidate (Opus itself): one same-model retry first,
    ///    the classifier samples so it is the cheapest fix → [`RefusalDecision::RetrySameModel`].
    /// 3. That having refused too, an installed cross-provider refusal client
    ///    takes the turn like a quota fallback → [`RefusalDecision::CrossProvider`].
    /// 4. No fallback left but an earlier declined exchange is still in context:
    ///    drop it and ask the same model once → [`RefusalDecision::RetryCleaned`].
    /// 5. Everything spent → [`RefusalDecision::Surface`].
    ///
    /// Anthropic-only: a non-Anthropic active model (including a refusal already
    /// handed to a cross-provider client) yields [`RefusalDecision::Proceed`] so
    /// that provider's own reply is consumed, never re-judged as our refusal.
    /// The caller drops the refused partial (by not pushing it) and re-requests.
    pub(super) fn decide_refusal_fallback(&mut self) -> RefusalDecision {
        // Own the string so the `&self` borrow is dropped before the mutation.
        let Some(model) = self.effective_request_model().map(str::to_string) else {
            return RefusalDecision::Proceed;
        };
        if !is_anthropic_model(&model) {
            return RefusalDecision::Proceed;
        }
        // Step 1/2: while no fallback has been applied yet this turn, try the
        // same-provider override, else one free same-model retry.
        if self.refusal_fallback_model.is_none() && !self.cross_fallback_active() {
            if let Some(fallback) = refusal_fallback_for(&model) {
                self.note_model_switch(SwitchTrigger::Refusal, &model, &fallback);
                self.refusal_fallback_model = Some(fallback);
                self.mark_refusal_turn_hit();
                return RefusalDecision::Retry;
            }
            if !self.refusal_same_model_retry_used {
                self.refusal_same_model_retry_used = true;
                return RefusalDecision::RetrySameModel;
            }
        }
        // Step 3: the same-provider override (or the same-model retry) refused
        // too — hand the turn to an installed cross-provider refusal client,
        // through the SAME swap machinery a quota fallback uses. Skipped when
        // one is already riding (its own refusal fell through to Proceed above).
        if !self.cross_fallback_active() {
            if let Some((_, to)) = self.refusal_fallback_client.as_ref() {
                let to = to.clone();
                self.active_cross_fallback = Some(CrossFallback::Refusal);
                // A same-provider override from the main model's world must
                // never ride the cross-provider client (it has its own model).
                self.refusal_fallback_model = None;
                self.escalation_model_override = None;
                self.note_model_switch(SwitchTrigger::Refusal, &model, &to);
                self.mark_refusal_turn_hit();
                return RefusalDecision::CrossProvider;
            }
        }
        // Step 4: no fallback is available. If an earlier declined exchange is
        // still in history — a sticky classifier reads the whole conversation —
        // drop it and ask once more. Exactly one per public turn.
        if !self.refusal_context_clean_used && self.has_prior_declined_exchange() {
            self.refusal_context_clean_used = true;
            return RefusalDecision::RetryCleaned;
        }
        RefusalDecision::Surface
    }

    /// Fold this refused public turn into the consecutive-refusal streak and,
    /// on the threshold, arm the session-scoped cooldown that pre-arms the next
    /// turns onto the fallback. Shared by the same-provider override and the
    /// cross-provider handoff so both count toward the pre-arm.
    fn mark_refusal_turn_hit(&mut self) {
        if self.refusal_turn_hit {
            return;
        }
        self.refusal_turn_hit = true;
        if self.refusal_consecutive_turns.saturating_add(1) >= REFUSAL_DRY_TURN_THRESHOLD
            && self
                .refusal_dry_until
                .is_none_or(|until| std::time::Instant::now() >= until)
        {
            self.refusal_dry_until = Some(std::time::Instant::now() + REFUSAL_DRY_COOLDOWN);
            // The first begin that actually pre-arms owns the notice; do not let
            // this mid-turn retry emit it prematurely.
            self.refusal_prearm_notice_pending = false;
            self.refusal_prearm_notice_latched = false;
        }
    }

    /// Whether history still holds an earlier surfaced refusal — a user message
    /// answered only by [`REFUSAL_SURFACED_NOTICE`]. That exchange is what a
    /// sticky classifier keeps reading, so [`Self::drop_last_declined_exchange`]
    /// removes it before the context-cleaning retry.
    fn has_prior_declined_exchange(&self) -> bool {
        self.last_declined_exchange_index().is_some()
    }

    /// The history index of the assistant message that surfaced a refusal, most
    /// recent first, when the message just before it is the user turn it
    /// declined. `None` when no such pair exists.
    fn last_declined_exchange_index(&self) -> Option<usize> {
        let messages = &self.session.messages;
        messages.iter().enumerate().rev().find_map(|(idx, message)| {
            let is_surfaced = message.role == crate::session::MessageRole::Assistant
                && message.blocks.iter().any(|block| {
                    matches!(block, ContentBlock::Text { text } if text == REFUSAL_SURFACED_NOTICE)
                });
            (is_surfaced
                && idx > 0
                && messages[idx - 1].role == crate::session::MessageRole::User)
            .then_some(idx)
        })
    }

    /// Drop the most recent surfaced-refusal exchange (the user turn and its
    /// refusal notice) from history so the context-cleaning retry asks a clean
    /// conversation. Returns whether anything was removed.
    pub(super) fn drop_last_declined_exchange(&mut self) -> bool {
        let Some(idx) = self.last_declined_exchange_index() else {
            return false;
        };
        let messages = Arc::make_mut(&mut self.session.messages);
        // Remove the assistant notice first (higher index), then the user turn.
        messages.remove(idx);
        messages.remove(idx - 1);
        self.session.mark_transcript_dirty();
        true
    }

    /// Fold the just-finished PUBLIC turn into the consecutive-refusal streak.
    /// Callers deliberately skip this for internal subturns/continuations so
    /// multiple deep-lane legs cannot count as multiple user turns. The hit
    /// flag is cleared only at this public boundary; each successful
    /// [`RefusalDecision::Retry`] within the new turn can set it once.
    pub(super) fn fold_finished_refusal_turn(&mut self) {
        if self.refusal_turn_hit {
            self.refusal_consecutive_turns = self.refusal_consecutive_turns.saturating_add(1);
        } else {
            self.refusal_consecutive_turns = 0;
        }
        self.refusal_turn_hit = false;
    }

    /// Turn-start refusal-cooldown management, called after the ordinary
    /// per-leg override reset and after quota/escalation turn state has settled:
    ///
    /// - an elapsed cooldown clears itself and lets Fable/Mythos serve natively;
    /// - an active cooldown pre-arms the current Opus family head only when the model that would
    ///   otherwise serve this request is Fable/Mythos on the native client;
    /// - the first such pre-arm latches one warning, while later dry turns stay
    ///   silent.
    ///
    /// This still runs for internal subturns because each leg resets its
    /// one-shot override. Only the streak fold above is public-turn-only.
    pub(super) fn begin_turn_refusal_fallback(&mut self) {
        if self
            .refusal_dry_until
            .is_some_and(|until| std::time::Instant::now() >= until)
        {
            self.refusal_dry_until = None;
            self.refusal_prearm_notice_pending = false;
            self.refusal_prearm_notice_latched = false;
        }
        // A refusal pre-arm must never displace an active quota fallback (that
        // swap already runs on its own provider's client). A refusal cross
        // pre-arm setting `active_cross_fallback = Refusal` below is fine.
        if self.refusal_dry_until.is_none()
            || self.active_cross_fallback == Some(CrossFallback::Quota)
        {
            return;
        }
        let Some(model) = self.effective_request_model().map(str::to_string) else {
            return;
        };
        // A same-provider candidate (Opus) rides the bound client as a
        // per-turn model override — the cheap, in-provider pre-arm.
        if let Some(fallback) = refusal_fallback_for(&model) {
            self.refusal_fallback_model = Some(fallback);
            self.latch_refusal_prearm_notice();
            return;
        }
        // No same-provider candidate (Opus itself): pre-arm the installed
        // cross-provider refusal client for the whole turn, exactly as the
        // quota cooldown pre-arms its client. Silent when none is connected.
        if self.refusal_fallback_client.is_some() {
            self.active_cross_fallback = Some(CrossFallback::Refusal);
            self.latch_refusal_prearm_notice();
        }
    }

    /// Latch the one refusal pre-arm notice for this cooldown window; later dry
    /// turns in the same window stay silent.
    fn latch_refusal_prearm_notice(&mut self) {
        if !self.refusal_prearm_notice_latched {
            self.refusal_prearm_notice_pending = true;
            self.refusal_prearm_notice_latched = true;
        }
    }

    /// The pre-arm notice text for the model this turn is pre-armed onto: the
    /// same-provider Opus override or the cross-provider refusal client. Named so
    /// the two emitters (sync eprintln, streaming render block) cannot drift, and
    /// so the cross-provider case says which provider carries the session rather
    /// than the Fable/Opus-only wording.
    pub(super) fn refusal_prearm_notice_text(&self) -> String {
        let model = self
            .refusal_fallback_model
            .as_deref()
            .or_else(|| match self.active_cross_fallback {
                Some(CrossFallback::Refusal) => {
                    self.refusal_fallback_client.as_ref().map(|(_, model)| model.as_str())
                }
                _ => None,
            });
        core_types::retry_signal::refusal_prearm_warn(
            model,
            REFUSAL_DRY_TURN_THRESHOLD,
            REFUSAL_DRY_COOLDOWN,
        )
    }

    /// Turn-start quota-fallback state management, called from both turn entry
    /// points (`begin_turn_once`, `begin_streaming_turn`). Mirrors the per-turn
    /// refusal-fallback reset but with the session-cooldown twist:
    ///
    /// - a stale/elapsed [`Self::quota_dry_until`] is dropped so the session
    ///   returns to the main model on its own (natural recovery);
    /// - while still inside the cooldown window AND a fallback client is
    ///   installed, the turn PRE-arms straight onto the fallback so it does not
    ///   re-spend the main model's rate-limit retry budget just to rediscover
    ///   the wall is still up (a short notice is latched for the turn loop);
    /// - otherwise the turn starts on the main model (a live mid-turn
    ///   exhaustion can still arm the fallback within the turn).
    ///
    /// Applies uniformly to internal deep-lane subturns too: a quota-dry session
    /// applies to every leg.
    pub(super) fn begin_turn_quota_fallback(&mut self) {
        self.begin_turn_quota_fallback_with_gate(::api::quota::quota_fallback_permitted);
    }

    pub(super) fn begin_turn_quota_fallback_with_gate<G>(&mut self, fallback_permitted: G)
    where
        G: Fn(::api::ProviderKind) -> bool,
    {
        // The wait-band one-shot is per turn: a fresh turn may wait again.
        self.quota_waited_this_turn = false;
        // So is the overload demotion. A new turn starts on the model the user
        // chose; carrying a previous turn's demotion forward would silently pin
        // the session to a lighter tier after one capacity dip.
        self.overload_demoted_this_turn = false;
        self.overload_demotion_model = None;
        if self
            .quota_dry_until
            .is_some_and(|until| std::time::Instant::now() >= until)
        {
            self.quota_dry_until = None;
        }
        // Measured recovery ends the cooldown early. `fallback_permitted`
        // returning false means a FRESH utilization reading (this process's
        // response headers or a sibling zo process's shared telemetry) puts the
        // main provider below the swap threshold — the wall is demonstrably
        // down, and every remaining pre-armed minute would park turns on the
        // fallback for no reason (observed: a transient 429/529 with no fresh
        // reading arms the full default cooldown while the account's quota is
        // actually fine). Unknown state keeps the cooldown, exactly like the
        // swap decision: only evidence changes the course.
        if self.quota_dry_until.is_some() {
            let main_provider = self
                .context_model
                .as_deref()
                .map(::api::detect_provider_kind);
            if main_provider.is_some_and(|provider| !fallback_permitted(provider)) {
                self.quota_dry_until = None;
            }
        }
        let cooldown_active = self.quota_dry_until.is_some();
        if cooldown_active && self.quota_fallback_client.is_some() {
            self.active_cross_fallback = Some(CrossFallback::Quota);
            self.quota_prearm_notice_pending = true;
        } else {
            // No cooldown, or the cooldown outlived its fallback client (e.g.
            // `/smart quota-fallback off` mid-session cleared it): start native.
            // Clears any cross fallback so the refusal pre-arm (which runs after
            // this, per the turn-begin order) sees a clean slate before it may
            // re-arm its own.
            self.active_cross_fallback = None;
            self.quota_prearm_notice_pending = false;
        }
    }

    /// The prearm notice for this turn, stamped with the main model's name and
    /// the cooldown time left — both live on `self`, so the two emitters (sync
    /// eprintln, streaming render block) cannot drift apart.
    pub(super) fn quota_prearm_notice_text(&self, fallback_model: &str) -> String {
        quota_fallback_prearm_info(
            fallback_model,
            self.context_model.as_deref(),
            self.quota_dry_until
                .map(|until| until.saturating_duration_since(std::time::Instant::now())),
        )
    }

    /// Decide how a turn-killing error should escape a hard `RateLimit` on the
    /// main model. Only a [`api::ProviderErrorClass::RateLimit`] that survived
    /// the retry budget (the wall is still up) is a candidate; anything else, or
    /// a turn already on the fallback, yields [`QuotaEscape::None`] so the caller
    /// fails exactly as before.
    ///
    /// Preference order:
    /// 1. **Wait** — when the exhausted window lifts within [`Self::quota_wait_band`]
    ///    (from the 429's `retry_after` hint and/or the measured window resets,
    ///    per [`api::quota::reset_wait_within_band`]) and this turn has not waited
    ///    yet. The caller sleeps and re-requests on the SAME model; no swap, no
    ///    cooldown. A single one-shot ([`Self::quota_waited_this_turn`]) prevents
    ///    a lying "reset now" header from looping the wait.
    /// 2. **Fallback** — otherwise, when the provider's measured quota permits
    ///    a model swap and a cross-provider client is installed: arm the swap
    ///    (records [`Self::quota_dry_until`] and clears any refusal→Opus
    ///    override, which would otherwise force the current Opus catalog head
    ///    onto the fallback client whose own target model is correct).
    /// 3. **None** — no band match, fallback is below the Anthropic 95% gate,
    ///    or no fallback client is installed.
    ///
    /// Both account-scoped steps are **skipped for a provider overload**
    /// ([`api::CapacityScope::Provider`]), because both ask a question about the
    /// account's window that a 529 does not answer:
    ///
    /// * waiting for the window to reset is waiting on the wrong clock — capacity
    ///   shedding is unrelated to it (and at low utilization the reset is hours
    ///   away, so the band never matches anyway);
    /// * the 95 %-utilization gate exists so a 429 at low utilization is read as
    ///   burst pressure rather than an exhausted plan — correct for a throttle,
    ///   exactly inverted for an overload. Measured on the reported turn: 2 %
    ///   utilization, so the gate refused the swap and returned `None`, killing a
    ///   turn whose own error text advised switching models. A lighter model on
    ///   another provider was available and succeeded seconds later.
    pub(super) fn decide_quota_escape(&mut self, error: &RuntimeError) -> QuotaEscape {
        self.decide_quota_escape_with_gate(error, ::api::quota::quota_fallback_permitted)
    }

    /// Whether a deep-gate PLAN / VERIFY / EXEC-implementer leg owns the client
    /// this turn is streaming on. A capacity wall there belongs to that leg's
    /// provider, not the main model's window, so both the escape decider and the
    /// retry cap in front of it stand aside — from one reading, so they cannot
    /// come to disagree about which legs count.
    pub(super) fn deep_leg_owns_the_wire(&self) -> bool {
        self.deep_plan_leg_active || self.deep_verify_leg_active || self.exec_impl_leg_active
    }

    /// How many capacity retries the MAIN turn's stream may spend before the
    /// error propagates so [`Self::decide_quota_escape`] can run. `None` keeps
    /// the full `RATE_LIMIT_MAX_ELAPSED` wall-clock budget — the right patience
    /// when riding the wall out is the only recovery there is. `Some` is
    /// `retry::MAIN_TURN_CAPACITY_BURST`, which carries the measurement.
    ///
    /// The answer is decided by whether an escape EXISTS, read from the same
    /// state `decide_quota_escape` answers on, because that decider only ever
    /// sees the error once this cap has already let it through:
    ///
    /// * a cross-provider quota fallback client is installed
    ///   ([`QuotaEscape::Fallback`]), or
    /// * the model on the wire has a lighter rung left on its own provider's
    ///   ladder ([`QuotaEscape::Lighter`]);
    /// * and neither a deep-gate leg nor a turn already running on the fallback
    ///   is in the way — `decide_quota_escape` returns [`QuotaEscape::None`]
    ///   there whatever the error says, so there is nothing to hand over to.
    ///
    /// [`QuotaEscape::Wait`] is deliberately not counted. Whether the window
    /// resets inside the band is a fact about the refusal itself, unknowable
    /// before it arrives — and a *named* reset the budget cannot reach is
    /// already handed over on the first refusal by `classify_for_retry`. Once
    /// the cap does hand over, the wait rule runs unchanged: a reset inside the
    /// band still waits on the main model instead of swapping.
    pub(super) fn main_turn_rate_limit_retry_cap(&self) -> Option<u32> {
        self.main_turn_rate_limit_retry_cap_with_gate(::api::quota::quota_fallback_permitted)
    }

    pub(super) fn main_turn_rate_limit_retry_cap_with_gate(
        &self,
        fallback_permitted: impl FnOnce(::api::ProviderKind) -> bool,
    ) -> Option<u32> {
        if self.deep_leg_owns_the_wire() {
            return None;
        }
        if self.cross_fallback_active() {
            return None;
        }
        // One rung down the same provider — the overload escape, which the
        // account gate never applies to.
        let lighter_rung = !self.overload_demoted_this_turn
            && self
                .effective_request_model()
                .or(self.context_model.as_deref())
                .and_then(::api::starvation_demotion_model)
                .is_some();
        // The cross-provider swap, read through the SAME utilization gate
        // `decide_quota_escape` puts in front of it. A gate that refuses turns
        // an account 429 back into "burst pressure, ride it out" — the escape
        // would answer [`QuotaEscape::None`], so capping here would only shorten
        // the wall into a dead turn.
        let swap_ready = self.quota_fallback_client.is_some()
            && self
                .context_model
                .as_deref()
                .map(::api::detect_provider_kind)
                .is_none_or(fallback_permitted);
        (lighter_rung || swap_ready).then_some(crate::retry::MAIN_TURN_CAPACITY_BURST)
    }

    pub(super) fn decide_quota_escape_with_gate(
        &mut self,
        error: &RuntimeError,
        fallback_permitted: impl FnOnce(::api::ProviderKind) -> bool,
    ) -> QuotaEscape {
        // A deep-gate PLAN/VERIFY/EXEC-implementer leg runs on a swapped deep
        // client, so a hard `RateLimit` here belongs to that provider, not the
        // main model's quota window. Never arm the main-turn quota fallback
        // from one of these legs: that would poison
        // `quota_fallback_active`/`quota_dry_until` and force later main turns
        // onto a fallback chosen for the wrong provider. PLAN/VERIFY have
        // their own candidate handling; EXEC transport failures return to the
        // deep driver's bounded attempt loop.
        if self.deep_leg_owns_the_wire() {
            return QuotaEscape::None;
        }
        if self.cross_fallback_active() {
            return QuotaEscape::None;
        }
        let Some(::api::ProviderErrorClass::RateLimit {
            retry_after,
            scope,
        }) = error.provider_error_class()
        else {
            return QuotaEscape::None;
        };
        // Whose capacity ran out decides which of the two account-scoped steps
        // below still make sense. See this function's doc comment: for a provider
        // overload the answer is "neither" — the account window is not the wall,
        // so waiting on its reset and gating on its utilization are both asking
        // about the wrong thing, and the swap they block is the only recovery.
        let account_window = matches!(scope, ::api::CapacityScope::Account);
        let main_provider = self
            .context_model
            .as_deref()
            .map(::api::detect_provider_kind);
        // Prefer holding on the main model when its wall lifts within the band.
        if account_window && !self.quota_waited_this_turn && !self.quota_wait_band.is_zero() {
            if let Some(provider) = main_provider {
                if let Some(wait) = ::api::quota::reset_wait_within_band(
                    provider,
                    retry_after,
                    self.quota_wait_band,
                ) {
                    self.quota_waited_this_turn = true;
                    return QuotaEscape::Wait(wait);
                }
            }
        }
        // A provider that is shedding refused THIS REQUEST SHAPE, so the cheapest
        // recovery is a lighter shape — one tier down the same provider's ladder,
        // once per turn. Tried before the cross-provider swap because it is the
        // smaller change (same credentials, same tool surface, no session cooldown
        // armed) and because the measurement says the tier is the wall. Without
        // this, a session with no cross-provider fallback configured had NO
        // recovery at all: it rode the budget out on the exact shape being
        // refused and then died.
        if !account_window && !self.overload_demoted_this_turn {
            let shed = self
                .effective_request_model()
                .or(self.context_model.as_deref())
                .unwrap_or_default()
                .to_string();
            if let Some(lighter) = ::api::starvation_demotion_model(&shed) {
                let lighter = ::api::resolve_model_alias(&lighter);
                self.overload_demoted_this_turn = true;
                self.overload_demotion_model = Some(lighter.clone());
                self.note_model_switch(SwitchTrigger::Starvation, &shed, &lighter);
                return QuotaEscape::Lighter(lighter);
            }
        }
        // A blocked swap gets at most the one bounded wait above. If that wait
        // was unavailable or has already been consumed, end the turn normally
        // instead of creating an unbounded same-model retry loop.
        if account_window && main_provider.is_some_and(|provider| !fallback_permitted(provider)) {
            return QuotaEscape::None;
        }
        let Some((_, model)) = self.quota_fallback_client.as_ref() else {
            return QuotaEscape::None;
        };
        let model = model.clone();
        self.active_cross_fallback = Some(CrossFallback::Quota);
        self.refusal_fallback_model = None;
        // Sister clear to the refusal one above, same invariant: a per-turn
        // wire-model override from the main model's world must never ride
        // the cross-provider fallback client (assemble_request also guards
        // on `cross_fallback_active` — this keeps the state itself honest).
        self.escalation_model_override = None;
        let cooldown = quota_fallback_cooldown(retry_after);
        self.quota_dry_until = Some(std::time::Instant::now() + cooldown);
        let walled = self.context_model.clone().unwrap_or_default();
        self.note_model_switch(SwitchTrigger::Quota, &walled, &model);
        QuotaEscape::Fallback(model)
    }


    /// Drive one request through whichever client the SYNC turn loop
    /// ([`Self::run_turn_once`]) should use: the native [`ApiClient::stream`]
    /// normally, or — when this turn has swapped to the cross-provider quota
    /// fallback — the async fallback client driven to completion on a scoped
    /// tokio runtime (mirrors `bash.rs`'s sync→async bridge). The fallback is an
    /// [`AsyncApiClient`]; the sync loop has no ambient runtime (the headless
    /// `-p` text path is a sync context), so the bridge picks the same three
    /// cases `bash.rs` does. The render deltas the async client emits are
    /// drained and discarded — the sync loop rebuilds its own view from the
    /// returned events, exactly as it does for the native client.
    pub(super) fn sync_stream_events(
        &mut self,
        request: ApiRequest,
    ) -> Result<Vec<AssistantEvent>, RuntimeError> {
        if let Some((client, _)) = self.active_cross_fallback_client().cloned() {
            return block_on_async_client(client, request);
        }
        self.api_client.stream(request)
    }
}

/// Bound-free so a deep-gate guard, which holds the runtime behind no
/// trait bounds, can report a leg's client swap.
impl<C, T> ConversationRuntime<C, T> {
    /// Tell the installed [`super::SwitchObserver`] that the wire is moving
    /// from `from` to `to` through `trigger` — with the context the new
    /// model has to read as the runtime estimates it now, and the attempt
    /// the switched requests bill to. Silent with no observer, and silent
    /// when the two are the same model (a leg client bound to the main
    /// model rewrites nothing). The observer must not fail the turn, so
    /// nothing here returns an error.
    pub(super) fn note_model_switch(&self, trigger: SwitchTrigger, from: &str, to: &str) {
        let Some(observer) = self.switch_observer.as_ref() else {
            return;
        };
        if from == to {
            return;
        }
        observer(&ModelSwitch {
            trigger,
            from: from.to_string(),
            to: to.to_string(),
            context_tokens: u64::try_from(crate::estimate_session_tokens(&self.session)).unwrap_or(u64::MAX),
            attempt: self.attempt.clone(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::{quota_fallback_prearm_info, quota_fallback_swap_warn};

    #[test]
    fn quota_fallback_notices_round_trip_the_active_model() {
        let model = "openai:gpt-5.6-sol";
        for notice in [
            quota_fallback_swap_warn(model),
            quota_fallback_prearm_info(model, Some("claude-fable-5"), None),
            quota_fallback_prearm_info(model, None, None),
        ] {
            assert_eq!(core_types::parse_quota_fallback_model(&notice), Some(model));
            assert!(!notice.starts_with(core_types::QUOTA_HOLD_NOTICE_PREFIX));
        }
    }

    /// The prearm notice must say WHOSE rate limit and for how long: the bare
    /// "the main model" phrasing was read as a claim about the fallback
    /// provider's quota.
    #[test]
    fn prearm_notice_names_the_cooling_model_and_the_time_left() {
        let notice = quota_fallback_prearm_info(
            "openai:gpt-5.6-luna",
            Some("claude-fable-5"),
            Some(std::time::Duration::from_secs(200)),
        );
        assert!(
            notice.contains("claude-fable-5 is still cooling down") && notice.contains("(4m left)"),
            "got {notice:?}"
        );

        // Unknown main model / elapsed remainder degrade to the generic line.
        let bare = quota_fallback_prearm_info(
            "openai:gpt-5.6-luna",
            None,
            Some(std::time::Duration::ZERO),
        );
        assert!(
            bare.contains("the main model is still cooling down") && !bare.contains("left)"),
            "got {bare:?}"
        );
    }

    use super::{
        quota_fallback_cooldown, QUOTA_FALLBACK_DEFAULT_COOLDOWN, QUOTA_FALLBACK_MAX_COOLDOWN,
    };

    /// [`QUOTA_FALLBACK_DEFAULT_COOLDOWN`]'s contract is that a quota escape
    /// never parks the session off its configured model for the rest of the
    /// day. That held only while the provider stayed silent: a `retry_after`
    /// hint was applied verbatim, so a single weekly-limit 429 advertising many
    /// hours pre-armed every following turn onto the fallback model for that
    /// whole time. The hint is trusted for its shape, not for its size.
    #[test]
    fn a_provider_retry_hint_cannot_park_the_session_past_the_ceiling() {
        use std::time::Duration;

        assert_eq!(
            quota_fallback_cooldown(None),
            QUOTA_FALLBACK_DEFAULT_COOLDOWN,
            "no hint keeps the default window"
        );

        let modest = Duration::from_secs(90);
        assert_eq!(
            quota_fallback_cooldown(Some(modest)),
            modest,
            "a hint inside the ceiling is honored verbatim"
        );

        let weekly_limit = Duration::from_secs(9 * 60 * 60);
        assert_eq!(
            quota_fallback_cooldown(Some(weekly_limit)),
            QUOTA_FALLBACK_MAX_COOLDOWN,
            "an hours-long hint is bounded so the session can probe the main model again"
        );
    }
}
