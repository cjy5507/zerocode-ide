use std::collections::BTreeMap;
use std::fmt::Write as _;

use serde_json::Value;

use crate::config::RuntimePermissionRuleConfig;

pub use core_types::PermissionMode;

/// Hook-provided override applied before standard permission evaluation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionOverride {
    Allow,
    Deny,
    Ask,
}

/// Additional permission context supplied by hooks or higher-level orchestration.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PermissionContext {
    override_decision: Option<PermissionOverride>,
    override_reason: Option<String>,
}

impl PermissionContext {
    #[must_use]
    pub fn new(
        override_decision: Option<PermissionOverride>,
        override_reason: Option<String>,
    ) -> Self {
        Self {
            override_decision,
            override_reason,
        }
    }

    #[must_use]
    pub fn override_decision(&self) -> Option<PermissionOverride> {
        self.override_decision
    }

    #[must_use]
    pub fn override_reason(&self) -> Option<&str> {
        self.override_reason.as_deref()
    }
}

/// Full authorization request presented to a permission prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionRequest {
    pub tool_name: String,
    pub input: String,
    pub current_mode: PermissionMode,
    pub required_mode: PermissionMode,
    /// What the *prompt* says: a request addressed to the user, in house voice.
    /// Never denial wording — an approval dialog that reads "this was denied,
    /// do not retry" is self-contradictory (it asked the user to approve).
    pub reason: Option<String>,
    /// Model-facing wording used if this request ends in a denial (the user
    /// answers "no", or no prompter exists). Deliberately separate from
    /// [`Self::reason`]: a denial is deterministic and must tell the model not
    /// to retry, which is exactly the wrong thing to show while still asking.
    /// `None` = reuse `reason` (the ask-rule / hook cases, where one wording
    /// serves both).
    pub deny_reason: Option<String>,
}

/// User-facing decision returned by a [`PermissionPrompter`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionPromptDecision {
    Allow,
    Deny { reason: String },
}

/// Prompting interface used when policy requires interactive approval.
pub trait PermissionPrompter {
    fn decide(&mut self, request: &PermissionRequest) -> PermissionPromptDecision;
}

/// Final authorization result after evaluating static rules and prompts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionOutcome {
    Allow,
    Deny { reason: String },
}

/// Evaluates permission mode requirements plus allow/deny/ask rules.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionPolicy {
    active_mode: PermissionMode,
    /// The stronger session mode saved while a deep-gate PLAN/VERIFY sub-turn
    /// temporarily downgrades `active_mode` (see [`Self::begin_phase_clamp`]).
    /// Lets mode-based denial text name the phase clamp instead of
    /// misattributing the restriction to the user's permission mode.
    phase_clamp_base: Option<PermissionMode>,
    /// Tools the active sub-turn phase forbids outright, regardless of mode or
    /// rules (see [`Self::begin_phase_tool_block`]).
    phase_blocked_tools: Vec<String>,
    tool_requirements: BTreeMap<String, PermissionMode>,
    allow_rules: Vec<PermissionRule>,
    deny_rules: Vec<PermissionRule>,
    ask_rules: Vec<PermissionRule>,
    /// OpenCode-compatible ordered rules. When non-empty, the last rule that
    /// matches a request wins and supersedes the category vectors above.
    ordered_rules: Vec<PermissionDecisionRule>,
    /// Allow rules granted live this session via [`Self::grant_always`] that
    /// have not yet been persisted to a settings file. Drained by the host.
    newly_granted: Vec<String>,
}

impl PermissionPolicy {
    #[must_use]
    pub fn new(active_mode: PermissionMode) -> Self {
        Self {
            active_mode,
            phase_clamp_base: None,
            phase_blocked_tools: Vec::new(),
            tool_requirements: BTreeMap::new(),
            allow_rules: Vec::new(),
            deny_rules: Vec::new(),
            ask_rules: Vec::new(),
            ordered_rules: Vec::new(),
            newly_granted: Vec::new(),
        }
    }

    /// Grant a durable "always allow" for `tool_name`, scoped to the request's
    /// subject (the exact command / path / url) when one is present, else the
    /// whole tool. The rule takes effect immediately for the rest of the
    /// session and is queued in [`Self::take_newly_granted`] for the host to
    /// persist to a settings file. Idempotent — duplicates are not re-added.
    pub fn grant_always(&mut self, tool_name: &str, input: &str) {
        let rule_str = match extract_permission_subject(input) {
            Some(subject) if !subject.trim().is_empty() => {
                format!("{tool_name}({})", escape_rule_subject(subject.trim()))
            }
            _ => tool_name.to_string(),
        };
        let rule = PermissionRule::parse(&rule_str);
        if !self.allow_rules.contains(&rule) {
            self.allow_rules.push(rule);
        }
        if !self.newly_granted.contains(&rule_str) {
            self.newly_granted.push(rule_str);
        }
    }

    /// Drain the allow rules granted this session so the host can persist them.
    #[must_use]
    pub fn take_newly_granted(&mut self) -> Vec<String> {
        std::mem::take(&mut self.newly_granted)
    }

    /// Add ephemeral allow rules for the duration of a scoped phase (e.g. the
    /// deep-lane read-only PLAN/VERIFY sub-turns), returning an opaque grant the
    /// caller passes back to [`Self::remove_temporary_allow_rules`] to restore
    /// the prior rule set exactly. Rules already present are not duplicated nor
    /// recorded, so removal never strips a pre-existing grant. Unlike
    /// [`Self::grant_always`] these are NOT queued for persistence -- they are
    /// intentionally transient (a phase grant must never leak to disk).
    #[must_use]
    pub fn add_temporary_allow_rules(&mut self, specs: &[&str]) -> TemporaryAllowGrant {
        let mut added = Vec::new();
        for spec in specs {
            let rule = PermissionRule::parse(spec);
            if !self.allow_rules.contains(&rule) {
                self.allow_rules.push(rule.clone());
                added.push(rule);
            }
        }
        TemporaryAllowGrant(added)
    }

    /// Remove the allow rules recorded in `grant` (from
    /// [`Self::add_temporary_allow_rules`]), restoring the rule set to its prior
    /// state so a transient phase grant never leaks past the phase.
    pub fn remove_temporary_allow_rules(&mut self, grant: TemporaryAllowGrant) {
        // Consume the grant by value so a token cannot be removed twice; destructure
        // to take ownership of the recorded rules for the retain check.
        let TemporaryAllowGrant(rules) = grant;
        self.allow_rules
            .retain(|existing| !rules.contains(existing));
    }

    #[must_use]
    pub fn with_tool_requirement(
        mut self,
        tool_name: impl Into<String>,
        required_mode: PermissionMode,
    ) -> Self {
        self.tool_requirements
            .insert(tool_name.into(), required_mode);
        self
    }

    /// Upsert tool→required-mode requirements onto the live policy in place,
    /// preserving the active mode, rules, and session grants. Used to propagate
    /// MCP tools discovered AFTER startup: their annotation-derived requirement
    /// (e.g. `readOnlyHint` → [`PermissionMode::ReadOnly`]) must reach the policy,
    /// otherwise [`Self::required_mode_for`] falls back to `DangerFullAccess` and
    /// a read-only MCP tool (`context7`) is denied inside a `ReadOnly` PLAN/VERIFY
    /// sub-turn even though it only reads.
    pub fn set_tool_requirements(
        &mut self,
        requirements: impl IntoIterator<Item = (String, PermissionMode)>,
    ) {
        for (tool_name, required_mode) in requirements {
            self.tool_requirements.insert(tool_name, required_mode);
        }
    }

    #[must_use]
    pub fn with_permission_rules(mut self, config: &RuntimePermissionRuleConfig) -> Self {
        self.allow_rules = config
            .allow()
            .iter()
            .map(|rule| PermissionRule::parse(rule))
            .collect();
        self.deny_rules = config
            .deny()
            .iter()
            .map(|rule| PermissionRule::parse(rule))
            .collect();
        self.ask_rules = config
            .ask()
            .iter()
            .map(|rule| PermissionRule::parse(rule))
            .collect();
        // Ordered specs are validated at config load (see config parsers), so a
        // parse failure here should not happen; skip rather than panic if it does.
        self.ordered_rules = config
            .rules()
            .iter()
            .filter_map(|spec| parse_decision_rule(spec).ok())
            .collect();
        self
    }

    #[must_use]
    pub fn active_mode(&self) -> PermissionMode {
        self.active_mode
    }

    /// Swap the active permission mode, returning the previous one.
    ///
    /// `authorize_with_context` reads `active_mode` live on every tool call, so
    /// a turn-local override takes effect immediately and is undone by restoring
    /// the returned value. The deep-lane gate uses this to run its PLAN and
    /// VERIFY sub-turns under [`PermissionMode::ReadOnly`] (no edits before a
    /// valid plan; the verifier inspects but never mutates), then restore.
    pub fn set_active_mode(&mut self, mode: PermissionMode) -> PermissionMode {
        std::mem::replace(&mut self.active_mode, mode)
    }

    /// Swap in a temporarily downgraded sub-turn mode (deep-gate PLAN/VERIFY),
    /// remembering a stronger base mode so mode-based denials can name the
    /// phase clamp instead of telling the model to ask the user for a
    /// permission the session already has. Returns the prior mode for
    /// [`Self::end_phase_clamp`].
    pub fn begin_phase_clamp(&mut self, mode: PermissionMode) -> PermissionMode {
        let saved = std::mem::replace(&mut self.active_mode, mode);
        if saved != mode && saved.satisfies(mode) {
            self.phase_clamp_base = Some(saved);
        }
        saved
    }

    /// Restore the pre-clamp mode saved by [`Self::begin_phase_clamp`].
    pub fn end_phase_clamp(&mut self, saved: PermissionMode) {
        self.active_mode = saved;
        self.phase_clamp_base = None;
    }

    /// Forbid `tools` outright for the duration of a sub-turn phase, returning
    /// the prior list for [`Self::end_phase_tool_block`] (so nested phases
    /// restore rather than clear).
    ///
    /// This is the phase's *execution* gate, and it exists because the
    /// alternative — dropping those tools from the advertised set for the leg —
    /// is paid for in prompt cache. Tool definitions sit in front of the
    /// messages, so a tool set that changes between two requests of the same
    /// conversation invalidates the whole prefix behind it: measured at 41% of
    /// one day's cache-write, every occurrence a fully cold re-read. Blocking at
    /// authorize time keeps the advertised set byte-identical all session while
    /// enforcing exactly the same rule, and the model learns the boundary from
    /// a refusal it can act on instead of from a silently missing tool.
    ///
    /// Mode-independent by design: the tools this blocks
    /// (`Agent`/`SpawnMultiAgent`/`Workflow`) only require `ReadOnly`, so the
    /// phase's read-only clamp does not stop them.
    pub fn begin_phase_tool_block(&mut self, tools: &[&str]) -> Vec<String> {
        std::mem::replace(
            &mut self.phase_blocked_tools,
            tools.iter().map(|tool| (*tool).to_string()).collect(),
        )
    }

    /// Restore the list saved by [`Self::begin_phase_tool_block`].
    pub fn end_phase_tool_block(&mut self, saved: Vec<String>) {
        self.phase_blocked_tools = saved;
    }

    /// The phase-block denial for `tool_name`, if the active phase forbids it.
    /// Case-insensitive: the registry accepts aliases that differ only in case,
    /// and a block that a rename of case could slip past would not be a block.
    fn phase_tool_block_reason(&self, tool_name: &str) -> Option<String> {
        self.phase_blocked_tools
            .iter()
            .any(|blocked| blocked.eq_ignore_ascii_case(tool_name))
            .then(|| {
                format!(
                    "tool '{tool_name}' is unavailable during this architect phase: the \
                     PLAN/VERIFY legs must reason for themselves and must not delegate. \
                     Permission audit: this is a phase restriction, NOT the session \
                     permission mode — do the work inline in this turn instead of spawning \
                     anything, and it becomes available again once the phase ends."
                )
            })
    }

    #[must_use]
    pub fn required_mode_for(&self, tool_name: &str) -> PermissionMode {
        self.tool_requirements
            .get(tool_name)
            .copied()
            .unwrap_or(PermissionMode::DangerFullAccess)
    }

    /// Input-aware refinement of [`Self::required_mode_for`]: a `bash`
    /// command the shared read-only classifier proves safe (`git log`,
    /// `grep`, …) only needs [`PermissionMode::ReadOnly`], so a read-only
    /// session can run it instead of hitting the static tool-level
    /// `DangerFullAccess` wall before the classifier is ever consulted.
    /// Every other tool, and any bash input without a provable command,
    /// keeps the static requirement.
    #[must_use]
    pub fn required_mode_for_input(&self, tool_name: &str, input: &str) -> PermissionMode {
        let base = self.required_mode_for(tool_name);
        if tool_name == "bash" && base == PermissionMode::DangerFullAccess {
            if let Some(command) = extract_bash_command(input) {
                return crate::bash_validation::required_mode_for_command(&command);
            }
        }
        base
    }

    /// The explicit `deny`-rule reason for `tool_name`/`input`, if any — the part
    /// of a permission decision that holds regardless of the active mode. An
    /// operator `deny` means "never — don't even prompt", so the enforcement
    /// layer consults this to keep denies effective in Prompt mode, where allow
    /// and ask escalation are otherwise deferred to the interactive prompter.
    /// Returns the same audit-formatted reason a full [`Self::authorize`] deny
    /// would produce.
    #[must_use]
    pub fn deny_reason(&self, tool_name: &str, input: &str) -> Option<String> {
        // A phase block is as unconditional as an operator deny, so the
        // enforcement layer must see it here too — otherwise Prompt mode would
        // defer the call to the interactive prompter and let it through.
        if let Some(reason) = self.phase_tool_block_reason(tool_name) {
            return Some(reason);
        }
        self.rule_signals(tool_name, input).deny.map(|reason| {
            permission_audit_reason(
                reason,
                ModeAudit {
                    current_mode: self.active_mode(),
                    required_mode: self.required_mode_for(tool_name),
                    phase_clamp_base: self.phase_clamp_base,
                },
            )
        })
    }

    #[must_use]
    pub fn authorize(
        &self,
        tool_name: &str,
        input: &str,
        prompter: Option<&mut dyn PermissionPrompter>,
    ) -> PermissionOutcome {
        self.authorize_with_context(tool_name, input, &PermissionContext::default(), prompter)
    }

    #[must_use]
    pub fn authorize_with_context(
        &self,
        tool_name: &str,
        input: &str,
        context: &PermissionContext,
        prompter: Option<&mut dyn PermissionPrompter>,
    ) -> PermissionOutcome {
        // Before rules, modes, and hook overrides: a phase block is structural.
        // A `PreToolUse` hook returning Allow must not resurrect a delegation
        // call inside a PLAN/VERIFY leg, which is exactly what checking this
        // after the override arm would do.
        if let Some(reason) = self.phase_tool_block_reason(tool_name) {
            return PermissionOutcome::Deny { reason };
        }
        let signals = self.rule_signals(tool_name, input);
        let current_mode = self.active_mode();
        let required_mode = self.required_mode_for_input(tool_name, input);
        let mode_audit = ModeAudit {
            current_mode,
            required_mode,
            phase_clamp_base: self.phase_clamp_base,
        };

        if let Some(reason) = signals.deny {
            return PermissionOutcome::Deny {
                reason: permission_audit_reason(reason, mode_audit),
            };
        }

        let ask_reason = signals.ask;
        let allow_matched = signals.allow;

        match context.override_decision() {
            Some(PermissionOverride::Deny) => {
                return PermissionOutcome::Deny {
                    reason: context.override_reason().map_or_else(
                        || format!("tool '{tool_name}' denied by hook"),
                        ToOwned::to_owned,
                    ),
                };
            }
            Some(PermissionOverride::Ask) => {
                let reason = context.override_reason().map_or_else(
                    || format!("tool '{tool_name}' requires approval due to hook guidance"),
                    ToOwned::to_owned,
                );
                return Self::prompt_or_deny(
                    tool_name,
                    input,
                    mode_audit,
                    Some(reason),
                    None,
                    prompter,
                );
            }
            Some(PermissionOverride::Allow) => {
                if let Some(reason) = ask_reason {
                    return Self::prompt_or_deny(
                        tool_name,
                        input,
                        mode_audit,
                        Some(reason),
                        None,
                        prompter,
                    );
                }
                if allow_matched
                    || current_mode == PermissionMode::Allow
                    || current_mode.satisfies(required_mode)
                {
                    return PermissionOutcome::Allow;
                }
            }
            None => {}
        }

        Self::evaluate_standard_decision(
            tool_name,
            input,
            mode_audit,
            ask_reason,
            allow_matched,
            prompter,
        )
    }

    /// Standard permission decision once hook overrides are resolved: honor an
    /// `ask` signal, allow when the mode/rule permits, prompt on a legal
    /// escalation, otherwise deny. Split out of [`Self::authorize_with_context`].
    fn evaluate_standard_decision(
        tool_name: &str,
        input: &str,
        mode_audit: ModeAudit,
        ask_reason: Option<String>,
        allow_matched: bool,
        prompter: Option<&mut dyn PermissionPrompter>,
    ) -> PermissionOutcome {
        let ModeAudit {
            current_mode,
            required_mode,
            ..
        } = mode_audit;
        if let Some(reason) = ask_reason {
            return Self::prompt_or_deny(tool_name, input, mode_audit, Some(reason), None, prompter);
        }

        if allow_matched
            || current_mode == PermissionMode::Allow
            || current_mode.satisfies(required_mode)
        {
            return PermissionOutcome::Allow;
        }

        if current_mode == PermissionMode::Prompt
            || (current_mode == PermissionMode::WorkspaceWrite
                && required_mode == PermissionMode::DangerFullAccess)
        {
            // Two wordings, one decision point. The prompt asks; the denial
            // explains. Sharing one string here used to put "This denial is
            // mode-based and deterministic — do not retry it" inside a dialog
            // that was still asking the user to approve.
            let prompt_reason = Some(format!(
                "tool '{tool_name}' requests escalation from {} to {} — approve to run it, or deny",
                current_mode.as_str(),
                required_mode.as_str()
            ));
            let deny_reason = Some(permission_audit_reason(
                format!(
                    "tool '{tool_name}' requires approval to escalate from {} to {}",
                    current_mode.as_str(),
                    required_mode.as_str()
                ),
                mode_audit,
            ));
            return Self::prompt_or_deny(
                tool_name,
                input,
                mode_audit,
                prompt_reason,
                deny_reason,
                prompter,
            );
        }

        PermissionOutcome::Deny {
            reason: permission_audit_reason(
                format!(
                    "tool '{tool_name}' requires {} permission; current mode is {}",
                    required_mode.as_str(),
                    current_mode.as_str()
                ),
                mode_audit,
            ),
        }
    }

    /// `deny_reason` is the wording used when the request ends in a denial;
    /// `None` reuses `reason` (ask rules and hook `Ask` overrides, whose single
    /// wording reads correctly both as a question and as a refusal).
    fn prompt_or_deny(
        tool_name: &str,
        input: &str,
        mode_audit: ModeAudit,
        reason: Option<String>,
        deny_reason: Option<String>,
        mut prompter: Option<&mut dyn PermissionPrompter>,
    ) -> PermissionOutcome {
        let request = PermissionRequest {
            tool_name: tool_name.to_string(),
            input: input.to_string(),
            current_mode: mode_audit.current_mode,
            required_mode: mode_audit.required_mode,
            reason: reason.clone(),
            deny_reason: deny_reason.clone(),
        };

        match prompter.as_mut() {
            Some(prompter) => match prompter.decide(&request) {
                PermissionPromptDecision::Allow => PermissionOutcome::Allow,
                PermissionPromptDecision::Deny { reason } => PermissionOutcome::Deny { reason },
            },
            // No prompter = nobody can approve, so this resolves as a denial and
            // takes the denial wording. `permission_audit_reason` is idempotent
            // (it early-returns once the audit sentence is present), so an
            // already-suffixed `deny_reason` passes through byte-identical.
            None => PermissionOutcome::Deny {
                reason: permission_audit_reason(
                    deny_reason.or(reason).unwrap_or_else(|| {
                        format!(
                            "tool '{tool_name}' requires approval to run while mode is {}",
                            mode_audit.current_mode.as_str()
                        )
                    }),
                    mode_audit,
                ),
            },
        }
    }

    fn find_matching_rule<'a>(
        rules: &'a [PermissionRule],
        tool_name: &str,
        input: &str,
    ) -> Option<&'a PermissionRule> {
        rules.iter().find(|rule| rule.matches(tool_name, input))
    }

    /// Resolve what the configured rules say about a `(tool, input)` pair,
    /// independent of mode and hook overrides.
    ///
    /// When OpenCode-compatible ordered rules are configured they are the sole
    /// authority and the *last* matching rule wins. Otherwise the legacy
    /// category vectors apply (first match within each, deny > ask > allow).
    fn rule_signals(&self, tool_name: &str, input: &str) -> RuleSignals {
        if !self.ordered_rules.is_empty() {
            let matched = self
                .ordered_rules
                .iter()
                .rev()
                .find(|rule| rule.rule.matches(tool_name, input));
            return match matched.map(|rule| (rule.action, rule.rule.raw.as_str())) {
                Some((PermissionRuleAction::Deny, raw)) => RuleSignals {
                    deny: Some(deny_rule_reason(tool_name, raw)),
                    ask: None,
                    allow: false,
                },
                Some((PermissionRuleAction::Ask, raw)) => RuleSignals {
                    deny: None,
                    ask: Some(ask_rule_reason(tool_name, raw)),
                    allow: false,
                },
                Some((PermissionRuleAction::Allow, _)) => RuleSignals {
                    deny: None,
                    ask: None,
                    allow: true,
                },
                None => RuleSignals::default(),
            };
        }

        RuleSignals {
            deny: Self::find_matching_rule(&self.deny_rules, tool_name, input)
                .map(|rule| deny_rule_reason(tool_name, &rule.raw)),
            ask: Self::find_matching_rule(&self.ask_rules, tool_name, input)
                .map(|rule| ask_rule_reason(tool_name, &rule.raw)),
            allow: Self::find_matching_rule(&self.allow_rules, tool_name, input).is_some(),
        }
    }
}

/// The mode facts a denial's audit trailer is built from: the live (possibly
/// phase-clamped) mode, the mode the call needs, and the stronger session
/// mode saved by an active PLAN/VERIFY clamp (`None` outside one).
#[derive(Clone, Copy)]
struct ModeAudit {
    current_mode: PermissionMode,
    required_mode: PermissionMode,
    phase_clamp_base: Option<PermissionMode>,
}

fn permission_audit_reason(reason: String, mode_audit: ModeAudit) -> String {
    let ModeAudit {
        current_mode,
        required_mode,
        phase_clamp_base,
    } = mode_audit;
    if reason.contains("Permission audit:") {
        return reason;
    }

    let mut audit = format!(
        "Permission audit: active mode is {}; required mode is {}.",
        current_mode.as_str(),
        required_mode.as_str()
    );
    if reason.contains("denied by rule") {
        audit.push_str(" To allow explicitly, remove or change the matching permission deny rule.");
    } else if reason.contains("ask rule") {
        audit.push_str(" To allow explicitly, approve the prompt in the TUI or change the matching permission ask rule.");
    } else if phase_clamp_base.is_some_and(|base| base.satisfies(required_mode)) {
        // The restriction is a temporary sub-turn clamp, not the user's mode:
        // steer the model back to finishing the phase instead of sending it
        // off to request a permission the session already has.
        audit.push_str(
            " This block comes from the architect PLAN/VERIFY phase, which runs read-only by \
             design — NOT from the session permission mode, which already allows this call. \
             Mutating tools come back automatically in the implementation phase. Finish this \
             phase's plan/analysis output; do not retry this call in this phase and do not \
             ask the user to change permissions.",
        );
    } else {
        let _ = write!(
            audit,
            " This denial is mode-based and deterministic — the identical call will be denied \
             again, so do not retry it. Continue with tools allowed under {} mode, or ask the \
             user to escalate (TUI: `/permissions {}`, or restart with `--permission-mode {}`).",
            current_mode.as_str(),
            required_mode.as_str(),
            required_mode.as_str()
        );
    }

    format!("{reason}. {audit}")
}

/// The user-facing reason shown when a deny rule (`raw`) blocks `tool_name`.
/// Shared by the ordered and legacy branches of [`PermissionPolicy::rule_signals`]
/// so the wording lives in one place.
fn deny_rule_reason(tool_name: &str, raw: &str) -> String {
    format!("Permission to use {tool_name} has been denied by rule '{raw}'")
}

/// The user-facing reason shown when an ask rule (`raw`) forces a prompt for
/// `tool_name`.
fn ask_rule_reason(tool_name: &str, raw: &str) -> String {
    format!("tool '{tool_name}' requires approval due to ask rule '{raw}'")
}

/// What the configured permission rules say about a request, before mode and
/// hook-override handling. `deny`/`ask` carry the human-facing reason string.
#[derive(Debug, Default)]
struct RuleSignals {
    deny: Option<String>,
    ask: Option<String>,
    allow: bool,
}

/// The effect of a single OpenCode-compatible permission rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PermissionRuleAction {
    Allow,
    Ask,
    Deny,
}

impl PermissionRuleAction {
    fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            "allow" => Some(Self::Allow),
            "ask" => Some(Self::Ask),
            "deny" => Some(Self::Deny),
            _ => None,
        }
    }
}

/// A matcher paired with the action it triggers, used for OpenCode-style ordered
/// permission evaluation.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PermissionDecisionRule {
    rule: PermissionRule,
    action: PermissionRuleAction,
}

/// Why an ordered permission rule spec (`"bash(git *)=allow"`) is malformed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PermissionRuleSpecError {
    /// The spec has no `=action` suffix.
    MissingAction { spec: String },
    /// The action after `=` is not one of allow/ask/deny.
    UnknownAction { spec: String, action: String },
}

impl std::fmt::Display for PermissionRuleSpecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingAction { spec } => write!(
                f,
                "rule '{spec}' is missing an '=action' suffix (e.g. 'bash(git *)=allow')"
            ),
            Self::UnknownAction { spec, action } => write!(
                f,
                "rule '{spec}' has unknown action '{action}'; use allow, ask, or deny"
            ),
        }
    }
}

/// Parse an OpenCode-compatible ordered rule spec such as `"bash(git *)=allow"`.
///
/// The action is taken from the text after the final `=`, so subjects may
/// themselves contain `=` (e.g. `bash(FOO=bar)=allow`).
fn parse_decision_rule(spec: &str) -> Result<PermissionDecisionRule, PermissionRuleSpecError> {
    let trimmed = spec.trim();
    let (rule_part, action_part) =
        trimmed
            .rsplit_once('=')
            .ok_or_else(|| PermissionRuleSpecError::MissingAction {
                spec: trimmed.to_string(),
            })?;
    let action = PermissionRuleAction::parse(action_part).ok_or_else(|| {
        PermissionRuleSpecError::UnknownAction {
            spec: trimmed.to_string(),
            action: action_part.trim().to_string(),
        }
    })?;
    Ok(PermissionDecisionRule {
        rule: PermissionRule::parse(rule_part.trim()),
        action,
    })
}

/// Validate an ordered rule spec without retaining the parsed result. Used by
/// the config parser so malformed entries fail at load time.
pub(crate) fn validate_decision_rule_spec(spec: &str) -> Result<(), PermissionRuleSpecError> {
    parse_decision_rule(spec).map(|_| ())
}

/// An opaque record of the allow rules a scoped phase added via
/// [`PermissionPolicy::add_temporary_allow_rules`], handed back to
/// [`PermissionPolicy::remove_temporary_allow_rules`]. Holds only the rules it
/// actually inserted, so restoring never disturbs pre-existing grants; opaque so
/// the internal `PermissionRule` representation stays private.
#[derive(Debug, Clone, Default)]
pub struct TemporaryAllowGrant(Vec<PermissionRule>);

#[derive(Debug, Clone, PartialEq, Eq)]
struct PermissionRule {
    raw: String,
    tool_name: String,
    matcher: PermissionRuleMatcher,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PermissionRuleMatcher {
    Any,
    Exact(String),
    Prefix(String),
    /// Glob pattern (`*`, `?`, `[..]`) matched against the whole subject.
    /// Enables OpenCode-style rules such as `edit_file(*.env)` or
    /// `bash(git *)` that a literal `Exact`/`Prefix` match would silently
    /// never fire on — turning a `deny` rule into a no-op. `glob::Pattern`
    /// derives `Eq`, so the enclosing `PermissionRule` keeps its derives.
    Glob(glob::Pattern),
}

impl PermissionRule {
    fn parse(raw: &str) -> Self {
        let trimmed = raw.trim();
        let open = find_first_unescaped(trimmed, '(');
        let close = find_last_unescaped(trimmed, ')');

        if let (Some(open), Some(close)) = (open, close) {
            if close == trimmed.len() - 1 && open < close {
                let tool_name = trimmed[..open].trim();
                let content = &trimmed[open + 1..close];
                if !tool_name.is_empty() {
                    let matcher = parse_rule_matcher(content);
                    return Self {
                        raw: trimmed.to_string(),
                        tool_name: tool_name.to_string(),
                        matcher,
                    };
                }
            }
        }

        Self {
            raw: trimmed.to_string(),
            tool_name: trimmed.to_string(),
            matcher: PermissionRuleMatcher::Any,
        }
    }

    fn matches(&self, tool_name: &str, input: &str) -> bool {
        if self.tool_name != tool_name {
            return false;
        }

        match &self.matcher {
            PermissionRuleMatcher::Any => true,
            PermissionRuleMatcher::Exact(expected) => permission_subject_candidates(input)
                .iter()
                .any(|candidate| candidate == expected),
            PermissionRuleMatcher::Prefix(prefix) => permission_subject_candidates(input)
                .iter()
                .any(|candidate| candidate.starts_with(prefix)),
            PermissionRuleMatcher::Glob(pattern) => permission_subject_candidates(input)
                .iter()
                .any(|candidate| pattern.matches(candidate)),
        }
    }
}

fn parse_rule_matcher(content: &str) -> PermissionRuleMatcher {
    let unescaped = unescape_rule_content(content.trim());
    if unescaped.is_empty() || unescaped == "*" {
        PermissionRuleMatcher::Any
    } else if let Some(prefix) = unescaped.strip_suffix(":*") {
        // Preserve the Claude Code `tool(prefix:*)` convention as a literal
        // prefix match — `prefix` itself is taken verbatim, never as a glob.
        PermissionRuleMatcher::Prefix(prefix.to_string())
    } else if contains_glob_meta(&unescaped) {
        // OpenCode-style glob: `edit_file(*.env)`, `bash(git *)`,
        // `read_file(**/secret*)`. Default match options let `*` span path
        // separators, which suits command/path/url subjects. A pattern that
        // fails to compile degrades to a literal match rather than being
        // dropped silently.
        match glob::Pattern::new(&unescaped) {
            Ok(pattern) => PermissionRuleMatcher::Glob(pattern),
            Err(_) => PermissionRuleMatcher::Exact(unescaped),
        }
    } else {
        PermissionRuleMatcher::Exact(unescaped)
    }
}

/// Whether `s` carries a glob metacharacter (`*`, `?`, or a `[` class opener)
/// and should be compiled as a [`PermissionRuleMatcher::Glob`] rather than
/// compared literally.
fn contains_glob_meta(s: &str) -> bool {
    s.contains(['*', '?', '['])
}

fn unescape_rule_content(content: &str) -> String {
    content
        .replace(r"\(", "(")
        .replace(r"\)", ")")
        .replace(r"\\", r"\")
}

/// Inverse of [`unescape_rule_content`] — escape a subject so it survives a
/// round-trip through [`PermissionRule::parse`] when embedded in `tool(subject)`.
fn escape_rule_subject(subject: &str) -> String {
    subject
        .replace('\\', r"\\")
        .replace('(', r"\(")
        .replace(')', r"\)")
}

fn find_first_unescaped(value: &str, needle: char) -> Option<usize> {
    let mut escaped = false;
    for (idx, ch) in value.char_indices() {
        if ch == '\\' {
            escaped = !escaped;
            continue;
        }
        if ch == needle && !escaped {
            return Some(idx);
        }
        escaped = false;
    }
    None
}

fn find_last_unescaped(value: &str, needle: char) -> Option<usize> {
    let chars = value.char_indices().collect::<Vec<_>>();
    for (pos, (idx, ch)) in chars.iter().enumerate().rev() {
        if *ch != needle {
            continue;
        }
        let mut backslashes = 0;
        for (_, prev) in chars[..pos].iter().rev() {
            if *prev == '\\' {
                backslashes += 1;
            } else {
                break;
            }
        }
        if backslashes % 2 == 0 {
            return Some(*idx);
        }
    }
    None
}

/// The discrete subject candidates a scoped rule matcher may match against.
///
/// The first candidate is the primary subject ([`extract_permission_subject`],
/// which keeps its whole-JSON fallback so existing whole-input rules never stop
/// matching). The rest are additional discrete subjects carried by tools whose
/// security-relevant subject is not a command/path/url:
///   - `Agent`.`subagent_type` and `MemoryWrite`.`slug` (top level),
///   - a generic top-level `name` (`Workflow` / many MCP tools),
///   - `SpawnMultiAgent`, whose members each carry a `subagent_type` INSIDE the
///     `agents[]` array (not at the top level) — a scoped
///     `SpawnMultiAgent(researcher)` rule matches if ANY spawned member is that
///     type.
///
/// Without them a scoped `SpawnMultiAgent(researcher)` / `MemoryWrite(secret:*)`
/// deny/ask rule matched the whole JSON blob and so never fired (`Tool(*)`/`Any`
/// was the only reliable form).
///
/// Purely ADDITIVE — it only ever appends candidates and never drops the primary
/// one — so a `deny`/`ask` rule fires at least as often as before (no security
/// regression), while a scoped rule now also matches the intended field. A tool
/// with a bespoke or deeply-nested key still falls back to the whole blob; scope
/// those with a `Tool(name)` or `*` rule.
fn permission_subject_candidates(input: &str) -> Vec<String> {
    let mut candidates = Vec::new();
    if let Some(primary) = extract_permission_subject(input) {
        candidates.push(primary);
    }
    let Ok(Value::Object(object)) = serde_json::from_str::<Value>(input) else {
        return candidates;
    };
    // Top-level discrete subjects (`Agent`.subagent_type, `MemoryWrite`.slug,
    // a generic `Workflow`/MCP `name`).
    for key in ["subagent_type", "slug", "name"] {
        if let Some(value) = object.get(key).and_then(Value::as_str) {
            push_unique_subject(&mut candidates, value);
        }
    }
    // `SpawnMultiAgent` nests each spawn's `subagent_type` inside `agents[]`, so
    // pull every member's type out as a candidate.
    if let Some(agents) = object.get("agents").and_then(Value::as_array) {
        for agent in agents {
            if let Some(value) = agent.get("subagent_type").and_then(Value::as_str) {
                push_unique_subject(&mut candidates, value);
            }
        }
    }
    candidates
}

/// Append `value` as a subject candidate unless it is empty or already present.
fn push_unique_subject(candidates: &mut Vec<String>, value: &str) {
    if !value.is_empty() && !candidates.iter().any(|existing| existing == value) {
        candidates.push(value.to_string());
    }
}

/// The `command` string of a bash tool input. Accepts both the serialized
/// JSON object the dispatch layer authorizes (`{"command": "…"}`) and a bare
/// command string (rule-matching callers may pass the subject directly).
/// Returns `None` when neither shape yields a command, so the caller falls
/// back to the static tool requirement.
fn extract_bash_command(input: &str) -> Option<String> {
    match serde_json::from_str::<Value>(input) {
        Ok(Value::Object(object)) => object
            .get("command")
            .and_then(Value::as_str)
            .map(str::to_owned),
        Ok(_) => None,
        Err(_) => (!input.trim().is_empty()).then(|| input.to_owned()),
    }
}

fn extract_permission_subject(input: &str) -> Option<String> {
    let parsed = serde_json::from_str::<Value>(input).ok();
    if let Some(Value::Object(object)) = parsed {
        for key in [
            "command",
            "path",
            "file_path",
            "filePath",
            "notebook_path",
            "notebookPath",
            "url",
            "pattern",
            "code",
            "message",
            // Typed actions (`Cargo`/`Git`) carry no free-text command; their
            // subject is the discrete subcommand verb. Kept last so a tool that
            // has both a richer subject and an `action` (none today) still
            // prefers the specific one. Lets a scoped allow rule target a single
            // verb — e.g. `Cargo(test)` — instead of the whole JSON blob.
            "action",
        ] {
            if let Some(value) = object.get(key).and_then(Value::as_str) {
                return Some(value.to_string());
            }
        }
    }

    (!input.trim().is_empty()).then(|| input.to_string())
}

#[cfg(test)]
mod tests {
    use super::{
        PermissionContext, PermissionMode, PermissionOutcome, PermissionOverride, PermissionPolicy,
        PermissionPromptDecision, PermissionPrompter, PermissionRequest,
    };
    use crate::config::RuntimePermissionRuleConfig;

    struct RecordingPrompter {
        seen: Vec<PermissionRequest>,
        allow: bool,
    }

    impl PermissionPrompter for RecordingPrompter {
        fn decide(&mut self, request: &PermissionRequest) -> PermissionPromptDecision {
            self.seen.push(request.clone());
            if self.allow {
                PermissionPromptDecision::Allow
            } else {
                PermissionPromptDecision::Deny {
                    reason: "not now".to_string(),
                }
            }
        }
    }

    /// An approval dialog must not read like a refusal. The escalation prompt
    /// used to reuse the denial wording verbatim, so the modal asked the user
    /// to approve while asserting "This denial is mode-based and deterministic
    /// — do not retry it". The ask and the refusal are now separate strings;
    /// the refusal wording is unchanged because it is correct for real denials.
    #[test]
    fn escalation_prompt_asks_while_the_denial_still_explains() {
        let mut prompter = RecordingPrompter {
            seen: Vec::new(),
            allow: true,
        };
        let policy = PermissionPolicy::new(PermissionMode::WorkspaceWrite)
            .with_tool_requirement("browser", PermissionMode::DangerFullAccess);

        assert_eq!(
            policy.authorize("browser", "{}", Some(&mut prompter)),
            PermissionOutcome::Allow
        );

        let request = prompter.seen.first().expect("the prompt must be raised");
        let prompt = request.reason.as_deref().expect("prompt reason");
        assert_eq!(
            prompt,
            "tool 'browser' requests escalation from workspace-write to danger-full-access \
             — approve to run it, or deny",
            "the modal must ask in house voice"
        );
        assert!(
            !prompt.contains("denial") && !prompt.contains("do not retry"),
            "an approval request must not claim it was already denied: {prompt}"
        );

        let deny = request.deny_reason.as_deref().expect("deny reason");
        assert!(
            deny.contains("tool 'browser' requires approval to escalate from workspace-write to \
                 danger-full-access"),
            "the denial wording must be preserved verbatim: {deny}"
        );
        assert!(
            deny.contains("This denial is mode-based and deterministic"),
            "a real denial must still tell the model not to retry: {deny}"
        );
    }

    /// With no prompter nobody can approve, so the escalation resolves as a
    /// denial — and must carry the denial wording, not the ask wording.
    #[test]
    fn escalation_without_a_prompter_denies_with_the_denial_wording() {
        let policy = PermissionPolicy::new(PermissionMode::WorkspaceWrite)
            .with_tool_requirement("browser", PermissionMode::DangerFullAccess);

        let PermissionOutcome::Deny { reason } = policy.authorize("browser", "{}", None) else {
            panic!("an unanswerable escalation must deny");
        };
        assert!(
            reason.contains("requires approval to escalate from workspace-write to \
                 danger-full-access")
                && reason.contains("This denial is mode-based and deterministic"),
            "the no-prompter denial must keep the deterministic-denial wording: {reason}"
        );
        assert!(
            !reason.contains("approve to run it, or deny"),
            "the ask wording must not leak into a denial: {reason}"
        );
    }

    /// The screenshot regression: a read-only session must run provably
    /// read-only bash (`git log`, `echo`, …) instead of hitting the static
    /// tool-level `DangerFullAccess` wall before the classifier is consulted.
    #[test]
    fn read_only_mode_allows_provably_read_only_bash() {
        let policy = PermissionPolicy::new(PermissionMode::ReadOnly)
            .with_tool_requirement("bash", PermissionMode::DangerFullAccess);

        for input in [
            r#"{"command":"git log --oneline -50 && echo '=== status ===' && git status --short"}"#,
            r#"{"command":"grep -r 'route hint' crates"}"#,
        ] {
            assert_eq!(
                policy.authorize("bash", input, None),
                PermissionOutcome::Allow,
                "read-only-safe command must pass: {input}"
            );
        }
    }

    #[test]
    fn read_only_mode_still_denies_unprovable_bash() {
        let policy = PermissionPolicy::new(PermissionMode::ReadOnly)
            .with_tool_requirement("bash", PermissionMode::DangerFullAccess);

        for input in [
            r#"{"command":"rm notes.txt"}"#,
            r#"{"command":"cat Cargo.toml > out.txt"}"#,
            r#"{"command":"echo $(curl example.com)"}"#,
        ] {
            assert!(
                matches!(
                    policy.authorize("bash", input, None),
                    PermissionOutcome::Deny { .. }
                ),
                "unprovable command must stay denied: {input}"
            );
        }
    }

    /// An operator deny rule outranks the classifier: even a provably
    /// read-only command stays denied when a rule names it.
    #[test]
    fn deny_rule_outranks_read_only_classifier() {
        let rules = RuntimePermissionRuleConfig::new(
            Vec::new(),
            vec!["bash(git log*)".to_string()],
            Vec::new(),
        );
        let policy = PermissionPolicy::new(PermissionMode::ReadOnly)
            .with_tool_requirement("bash", PermissionMode::DangerFullAccess)
            .with_permission_rules(&rules);

        assert!(matches!(
            policy.authorize("bash", r#"{"command":"git log --oneline"}"#, None),
            PermissionOutcome::Deny { .. }
        ));
    }

    #[test]
    fn allows_tools_when_active_mode_meets_requirement() {
        let policy = PermissionPolicy::new(PermissionMode::WorkspaceWrite)
            .with_tool_requirement("read_file", PermissionMode::ReadOnly)
            .with_tool_requirement("write_file", PermissionMode::WorkspaceWrite);

        assert_eq!(
            policy.authorize("read_file", "{}", None),
            PermissionOutcome::Allow
        );
        assert_eq!(
            policy.authorize("write_file", "{}", None),
            PermissionOutcome::Allow
        );
    }

    #[test]
    fn danger_full_access_satisfies_all_standard_tool_requirements() {
        let policy = PermissionPolicy::new(PermissionMode::DangerFullAccess)
            .with_tool_requirement("read_file", PermissionMode::ReadOnly)
            .with_tool_requirement("write_file", PermissionMode::WorkspaceWrite)
            .with_tool_requirement("bash", PermissionMode::DangerFullAccess);

        assert_eq!(
            policy.authorize("read_file", "{}", None),
            PermissionOutcome::Allow
        );
        assert_eq!(
            policy.authorize("write_file", "{}", None),
            PermissionOutcome::Allow
        );
        assert_eq!(
            policy.authorize("bash", r#"{"command":"echo ok"}"#, None),
            PermissionOutcome::Allow
        );
    }

    #[test]
    fn temporary_allow_rule_permits_matching_read_only_bash_under_read_only() {
        // Reproduces the /goal·/loop PLAN/VERIFY denial: bash requires
        // DangerFullAccess, so under ReadOnly it is denied — until a scoped
        // read-only allow rule is injected, which permits the matching command
        // while leaving non-matching (write) bash denied.
        let mut policy = PermissionPolicy::new(PermissionMode::ReadOnly)
            .with_tool_requirement("bash", PermissionMode::DangerFullAccess);

        let cargo = r#"{"command":"cargo test --all"}"#;
        let destructive = r#"{"command":"rm -rf build"}"#;

        // Before the grant: even a read-only cargo command is denied.
        assert!(matches!(
            policy.authorize("bash", cargo, None),
            PermissionOutcome::Deny { .. }
        ));

        let grant = policy.add_temporary_allow_rules(&["bash(cargo *)"]);
        // The allow rule bypasses the mode requirement for the matching subject…
        assert_eq!(
            policy.authorize("bash", cargo, None),
            PermissionOutcome::Allow
        );
        // …but a non-matching (destructive) bash stays denied.
        assert!(matches!(
            policy.authorize("bash", destructive, None),
            PermissionOutcome::Deny { .. }
        ));

        // After removing the grant, the read-only cargo command is denied again
        // (no leak past the scoped phase).
        policy.remove_temporary_allow_rules(grant);
        assert!(matches!(
            policy.authorize("bash", cargo, None),
            PermissionOutcome::Deny { .. }
        ));
    }

    #[test]
    fn temporary_allow_rule_permits_typed_cargo_action_under_read_only() {
        // The deep VERIFY/PLAN denial of the shell-free `Cargo` typed tool: it
        // requires WorkspaceWrite (writes `target/`), so under a downgraded
        // ReadOnly phase it is denied — until a scoped `Cargo(<verb>)` allow
        // rule is injected. Its subject is the discrete `action` verb, so a
        // per-verb rule targets one subcommand and leaves write verbs gated.
        let mut policy = PermissionPolicy::new(PermissionMode::ReadOnly)
            .with_tool_requirement("Cargo", PermissionMode::WorkspaceWrite);

        let test = r#"{"action":"test","args":["-p","api"]}"#;
        let build = r#"{"action":"build"}"#;

        // Before the grant: even a read-only `cargo test` typed action is denied.
        assert!(matches!(
            policy.authorize("Cargo", test, None),
            PermissionOutcome::Deny { .. }
        ));

        let grant = policy.add_temporary_allow_rules(&["Cargo(test)", "Cargo(check)"]);
        // The allow rule matches on the `action` subject and bypasses the mode
        // requirement for the named verb…
        assert_eq!(
            policy.authorize("Cargo", test, None),
            PermissionOutcome::Allow
        );
        // …but an un-listed (write) verb like `build` stays denied.
        assert!(matches!(
            policy.authorize("Cargo", build, None),
            PermissionOutcome::Deny { .. }
        ));

        // No leak past the scoped phase.
        policy.remove_temporary_allow_rules(grant);
        assert!(matches!(
            policy.authorize("Cargo", test, None),
            PermissionOutcome::Deny { .. }
        ));
    }

    #[test]
    fn remove_temporary_allow_rules_preserves_preexisting_grants() {
        // A pre-existing allow rule must survive add+remove of a scoped grant.
        let mut policy = PermissionPolicy::new(PermissionMode::ReadOnly)
            .with_tool_requirement("bash", PermissionMode::DangerFullAccess);
        policy.grant_always("bash", r#"{"command":"git status"}"#);

        let grant = policy.add_temporary_allow_rules(&["bash(cargo *)"]);
        assert_eq!(
            policy.authorize("bash", r#"{"command":"cargo build"}"#, None),
            PermissionOutcome::Allow
        );
        policy.remove_temporary_allow_rules(grant);

        // The scoped rule is gone…
        assert!(matches!(
            policy.authorize("bash", r#"{"command":"cargo build"}"#, None),
            PermissionOutcome::Deny { .. }
        ));
        // …but the pre-existing grant remains.
        assert_eq!(
            policy.authorize("bash", r#"{"command":"git status"}"#, None),
            PermissionOutcome::Allow
        );
    }

    #[test]
    fn denies_read_only_escalations_without_prompt() {
        let policy = PermissionPolicy::new(PermissionMode::ReadOnly)
            .with_tool_requirement("write_file", PermissionMode::WorkspaceWrite)
            .with_tool_requirement("bash", PermissionMode::DangerFullAccess);

        match policy.authorize("write_file", "{}", None) {
            PermissionOutcome::Deny { reason } => {
                assert!(reason.contains("requires workspace-write permission"));
                assert!(reason.contains("Permission audit: active mode is read-only"));
                assert!(reason.contains("required mode is workspace-write"));
                assert!(reason.contains("/permissions workspace-write"));
                assert!(reason.contains("--permission-mode workspace-write"));
            }
            PermissionOutcome::Allow => panic!("write_file should be denied"),
        }
        match policy.authorize("bash", "{}", None) {
            PermissionOutcome::Deny { reason } => {
                assert!(reason.contains("requires danger-full-access permission"));
                assert!(reason.contains("required mode is danger-full-access"));
                assert!(reason.contains("/permissions danger-full-access"));
                assert!(reason.contains("--permission-mode danger-full-access"));
            }
            PermissionOutcome::Allow => panic!("bash should be denied"),
        }
    }

    #[test]
    fn phase_clamped_denial_names_the_phase_instead_of_the_session_mode() {
        let mut policy = PermissionPolicy::new(PermissionMode::DangerFullAccess)
            .with_tool_requirement("bash", PermissionMode::DangerFullAccess);

        let saved = policy.begin_phase_clamp(PermissionMode::ReadOnly);
        match policy.authorize("bash", "{}", None) {
            PermissionOutcome::Deny { reason } => {
                assert!(reason.contains("architect PLAN/VERIFY phase"), "{reason}");
                assert!(
                    reason.contains("do not ask the user to change permissions"),
                    "{reason}"
                );
                // The escalation advice would be wrong: the session already
                // has the permission — the phase clamp is the restriction.
                assert!(!reason.contains("/permissions danger-full-access"), "{reason}");
            }
            PermissionOutcome::Allow => panic!("bash should be denied during the clamp"),
        }

        policy.end_phase_clamp(saved);
        assert_eq!(policy.authorize("bash", "{}", None), PermissionOutcome::Allow);

        // A genuine read-only session (no clamp) keeps the escalation advice.
        let read_only = PermissionPolicy::new(PermissionMode::ReadOnly)
            .with_tool_requirement("bash", PermissionMode::DangerFullAccess);
        match read_only.authorize("bash", "{}", None) {
            PermissionOutcome::Deny { reason } => {
                assert!(reason.contains("/permissions danger-full-access"), "{reason}");
                assert!(!reason.contains("architect PLAN/VERIFY phase"), "{reason}");
            }
            PermissionOutcome::Allow => panic!("bash should be denied under read-only"),
        }
    }

    #[test]
    fn refreshed_mcp_requirements_allow_read_only_and_gate_write_under_readonly() {
        // Simulates an MCP server discovered after startup: a read-only fetch
        // (context7, readOnlyHint) and a write-capable tool. Their requirements
        // are merged into the live policy via set_tool_requirements (the fix).
        let mut policy = PermissionPolicy::new(PermissionMode::ReadOnly);
        // Before the merge, an unregistered MCP tool defaults to DangerFullAccess
        // and is denied even though it only reads.
        assert!(matches!(
            policy.authorize("mcp__context7__resolve-library-id", "{}", None),
            PermissionOutcome::Deny { .. }
        ));
        policy.set_tool_requirements([
            (
                "mcp__context7__resolve-library-id".to_string(),
                PermissionMode::ReadOnly,
            ),
            (
                "mcp__fs__write".to_string(),
                PermissionMode::DangerFullAccess,
            ),
        ]);
        // Read-only MCP now passes inside a ReadOnly (PLAN/VERIFY) sub-turn...
        assert_eq!(
            policy.authorize("mcp__context7__resolve-library-id", "{}", None),
            PermissionOutcome::Allow
        );
        // ...while a write-capable MCP tool is still gated.
        assert!(matches!(
            policy.authorize("mcp__fs__write", "{}", None),
            PermissionOutcome::Deny { .. }
        ));
    }

    #[test]
    fn prompts_for_workspace_write_to_danger_full_access_escalation() {
        let policy = PermissionPolicy::new(PermissionMode::WorkspaceWrite)
            .with_tool_requirement("bash", PermissionMode::DangerFullAccess);
        let mut prompter = RecordingPrompter {
            seen: Vec::new(),
            allow: true,
        };

        // Not provably read-only, so the danger-full-access requirement holds
        // and the workspace-write session must be prompted.
        let outcome = policy.authorize("bash", "cargo build", Some(&mut prompter));

        assert_eq!(outcome, PermissionOutcome::Allow);
        assert_eq!(prompter.seen.len(), 1);
        assert_eq!(prompter.seen[0].tool_name, "bash");
        assert_eq!(
            prompter.seen[0].current_mode,
            PermissionMode::WorkspaceWrite
        );
        assert_eq!(
            prompter.seen[0].required_mode,
            PermissionMode::DangerFullAccess
        );
        // The prompt asks; the audit/escalation guidance rides on the denial
        // wording, which is what the model sees only if this ends in a refusal.
        let reason = prompter.seen[0].reason.as_deref().expect("prompt reason");
        assert!(reason.contains("requests escalation from workspace-write to danger-full-access"));
        let deny = prompter.seen[0]
            .deny_reason
            .as_deref()
            .expect("deny reason");
        assert!(deny.contains("Permission audit: active mode is workspace-write"));
        assert!(deny.contains("/permissions danger-full-access"));
    }

    #[test]
    fn honors_prompt_rejection_reason() {
        let policy = PermissionPolicy::new(PermissionMode::WorkspaceWrite)
            .with_tool_requirement("bash", PermissionMode::DangerFullAccess);
        let mut prompter = RecordingPrompter {
            seen: Vec::new(),
            allow: false,
        };

        assert!(matches!(
            policy.authorize("bash", "cargo build", Some(&mut prompter)),
            PermissionOutcome::Deny { reason } if reason == "not now"
        ));
    }

    #[test]
    fn applies_rule_based_denials_and_allows() {
        let rules = RuntimePermissionRuleConfig::new(
            vec!["bash(git:*)".to_string()],
            vec!["bash(rm -rf:*)".to_string()],
            Vec::new(),
        );
        let policy = PermissionPolicy::new(PermissionMode::ReadOnly)
            .with_tool_requirement("bash", PermissionMode::DangerFullAccess)
            .with_permission_rules(&rules);

        assert_eq!(
            policy.authorize("bash", r#"{"command":"git status"}"#, None),
            PermissionOutcome::Allow
        );
        assert!(matches!(
            policy.authorize("bash", r#"{"command":"rm -rf /tmp/x"}"#, None),
            PermissionOutcome::Deny { reason } if reason.contains("denied by rule")
                && reason.contains("Permission audit:")
                && reason.contains("matching permission deny rule")
        ));
    }

    #[test]
    fn scoped_rules_match_subagent_type_slug_and_name_subjects() {
        // Discrete subjects that are not a command/path/url must be matchable by a
        // scoped deny/ask rule (they previously fell back to the whole JSON blob
        // and never fired). These use the REAL on-wire shapes — crucially
        // `SpawnMultiAgent` nests `subagent_type` inside each `agents[]` member,
        // not at the top level. Config signature: new(allow, deny, ask).
        let rules = RuntimePermissionRuleConfig::new(
            Vec::new(),
            vec![
                "Agent(researcher)".to_string(),
                "SpawnMultiAgent(researcher)".to_string(),
                "MemoryWrite(secret:*)".to_string(),
                "Workflow(deploy)".to_string(),
            ],
            Vec::new(),
        );
        let policy = PermissionPolicy::new(PermissionMode::Allow).with_permission_rules(&rules);

        // `Agent` carries `subagent_type` at the top level.
        assert!(matches!(
            policy.authorize(
                "Agent",
                r#"{"subagent_type":"researcher","description":"d","prompt":"p"}"#,
                None
            ),
            PermissionOutcome::Deny { .. }
        ));

        // `SpawnMultiAgent` nests it inside `agents[]` (the real payload shape); a
        // scoped rule must fire if ANY member is the denied type.
        assert!(matches!(
            policy.authorize(
                "SpawnMultiAgent",
                r#"{"agents":[{"subagent_type":"Explore","prompt":"a"},{"subagent_type":"researcher","prompt":"b"}]}"#,
                None
            ),
            PermissionOutcome::Deny { .. }
        ));
        // A batch with no denied member is untouched.
        assert_eq!(
            policy.authorize(
                "SpawnMultiAgent",
                r#"{"agents":[{"subagent_type":"Explore","prompt":"a"}]}"#,
                None
            ),
            PermissionOutcome::Allow
        );

        // Prefix deny on the MemoryWrite `slug` fires; a non-matching slug does not.
        assert!(matches!(
            policy.authorize(
                "MemoryWrite",
                r#"{"slug":"secret-key","summary":"s","body":"b"}"#,
                None
            ),
            PermissionOutcome::Deny { .. }
        ));
        assert_eq!(
            policy.authorize(
                "MemoryWrite",
                r#"{"slug":"public-note","summary":"s","body":"b"}"#,
                None
            ),
            PermissionOutcome::Allow
        );

        // A generic top-level `name` (Workflow bare-spec form / MCP-style).
        assert!(matches!(
            policy.authorize("Workflow", r#"{"name":"deploy","phases":[]}"#, None),
            PermissionOutcome::Deny { .. }
        ));
    }

    #[test]
    fn ask_rules_force_prompt_even_when_mode_allows() {
        let rules = RuntimePermissionRuleConfig::new(
            Vec::new(),
            Vec::new(),
            vec!["bash(git:*)".to_string()],
        );
        let policy = PermissionPolicy::new(PermissionMode::DangerFullAccess)
            .with_tool_requirement("bash", PermissionMode::DangerFullAccess)
            .with_permission_rules(&rules);
        let mut prompter = RecordingPrompter {
            seen: Vec::new(),
            allow: true,
        };

        let outcome = policy.authorize("bash", r#"{"command":"git status"}"#, Some(&mut prompter));

        assert_eq!(outcome, PermissionOutcome::Allow);
        assert_eq!(prompter.seen.len(), 1);
        assert!(
            prompter.seen[0]
                .reason
                .as_deref()
                .is_some_and(|reason| reason.contains("ask rule"))
        );
    }

    #[test]
    fn hook_allow_still_respects_ask_rules() {
        let rules = RuntimePermissionRuleConfig::new(
            Vec::new(),
            Vec::new(),
            vec!["bash(git:*)".to_string()],
        );
        let policy = PermissionPolicy::new(PermissionMode::ReadOnly)
            .with_tool_requirement("bash", PermissionMode::DangerFullAccess)
            .with_permission_rules(&rules);
        let context = PermissionContext::new(
            Some(PermissionOverride::Allow),
            Some("hook approved".to_string()),
        );
        let mut prompter = RecordingPrompter {
            seen: Vec::new(),
            allow: true,
        };

        let outcome = policy.authorize_with_context(
            "bash",
            r#"{"command":"git status"}"#,
            &context,
            Some(&mut prompter),
        );

        assert_eq!(outcome, PermissionOutcome::Allow);
        assert_eq!(prompter.seen.len(), 1);
    }

    #[test]
    fn hook_deny_short_circuits_permission_flow() {
        let policy = PermissionPolicy::new(PermissionMode::DangerFullAccess)
            .with_tool_requirement("bash", PermissionMode::DangerFullAccess);
        let context = PermissionContext::new(
            Some(PermissionOverride::Deny),
            Some("blocked by hook".to_string()),
        );

        assert_eq!(
            policy.authorize_with_context("bash", "{}", &context, None),
            PermissionOutcome::Deny {
                reason: "blocked by hook".to_string(),
            }
        );
    }

    #[test]
    fn hook_ask_forces_prompt() {
        let policy = PermissionPolicy::new(PermissionMode::DangerFullAccess)
            .with_tool_requirement("bash", PermissionMode::DangerFullAccess);
        let context = PermissionContext::new(
            Some(PermissionOverride::Ask),
            Some("hook requested confirmation".to_string()),
        );
        let mut prompter = RecordingPrompter {
            seen: Vec::new(),
            allow: true,
        };

        let outcome = policy.authorize_with_context("bash", "{}", &context, Some(&mut prompter));

        assert_eq!(outcome, PermissionOutcome::Allow);
        assert_eq!(prompter.seen.len(), 1);
        assert_eq!(
            prompter.seen[0].reason.as_deref(),
            Some("hook requested confirmation")
        );
    }

    #[test]
    fn grant_always_records_subject_scoped_rule_and_allows_repeat() {
        let mut policy = PermissionPolicy::new(PermissionMode::WorkspaceWrite)
            .with_tool_requirement("bash", PermissionMode::DangerFullAccess);
        // `cargo build` is not provably read-only, so it keeps the
        // danger-full-access requirement (an `ls`-style probe would now be
        // auto-allowed by the classifier and never exercise the grant flow).
        let input = r#"{"command":"cargo build"}"#;
        // Before granting, the escalation needs a prompt → deny without one.
        assert!(matches!(
            policy.authorize("bash", input, None),
            PermissionOutcome::Deny { .. }
        ));

        policy.grant_always("bash", input);

        // Recorded exactly once for persistence; draining is idempotent.
        assert_eq!(
            policy.take_newly_granted(),
            vec!["bash(cargo build)".to_string()]
        );
        assert!(policy.take_newly_granted().is_empty());

        // The same command is now allowed live without a prompter…
        assert_eq!(
            policy.authorize("bash", input, None),
            PermissionOutcome::Allow
        );
        // …but a different command is NOT (exact-subject scope).
        assert!(matches!(
            policy.authorize("bash", r#"{"command":"rm -rf /"}"#, None),
            PermissionOutcome::Deny { .. }
        ));
    }

    #[test]
    fn grant_always_escapes_parens_and_falls_back_to_whole_tool() {
        let mut policy = PermissionPolicy::new(PermissionMode::WorkspaceWrite);
        policy.grant_always("bash", r#"{"command":"echo (hi)"}"#);
        assert_eq!(
            policy.take_newly_granted(),
            vec![r"bash(echo \(hi\))".to_string()]
        );

        // No subject in the input → grant the whole tool by name.
        let mut whole = PermissionPolicy::new(PermissionMode::WorkspaceWrite);
        whole.grant_always("some_tool", "");
        assert_eq!(whole.take_newly_granted(), vec!["some_tool".to_string()]);
    }

    #[test]
    fn glob_deny_rules_match_wildcard_subjects() {
        // `edit_file(*.env)` would parse as an `Exact` literal and silently
        // never fire (no real path is the string `*.env`). As a glob it denies
        // any path ending in `.env`, including nested ones. `bash(* | sh)`
        // catches piping any command into a shell.
        let rules = RuntimePermissionRuleConfig::new(
            Vec::new(),
            vec!["edit_file(*.env)".to_string(), "bash(* | sh)".to_string()],
            Vec::new(),
        );
        let policy = PermissionPolicy::new(PermissionMode::DangerFullAccess)
            .with_tool_requirement("edit_file", PermissionMode::WorkspaceWrite)
            .with_tool_requirement("bash", PermissionMode::DangerFullAccess)
            .with_permission_rules(&rules);

        // Denied — the glob matches.
        for path in [".env", "config/.env", "deep/nested/.env"] {
            let input = format!(r#"{{"file_path":"{path}"}}"#);
            assert!(
                matches!(
                    policy.authorize("edit_file", &input, None),
                    PermissionOutcome::Deny { .. }
                ),
                "expected deny for {path}"
            );
        }
        assert!(matches!(
            policy.authorize("bash", r#"{"command":"cat secrets | sh"}"#, None),
            PermissionOutcome::Deny { .. }
        ));

        // Allowed — no glob match (mode already permits the write).
        assert_eq!(
            policy.authorize("edit_file", r#"{"file_path":"src/main.rs"}"#, None),
            PermissionOutcome::Allow
        );
    }

    #[test]
    fn glob_allow_rule_with_space_separated_wildcard() {
        // `bash(git *)` — a space-bearing wildcard that both the `:*` prefix
        // form and a literal `Exact` match would miss. The legacy `git:*`
        // prefix form keeps working (see `applies_rule_based_denials_and_allows`).
        let rules = RuntimePermissionRuleConfig::new(
            vec!["bash(git *)".to_string()],
            Vec::new(),
            Vec::new(),
        );
        let policy = PermissionPolicy::new(PermissionMode::ReadOnly)
            .with_tool_requirement("bash", PermissionMode::DangerFullAccess)
            .with_permission_rules(&rules);

        assert_eq!(
            policy.authorize("bash", r#"{"command":"git status"}"#, None),
            PermissionOutcome::Allow
        );
        // A non-git command stays gated — the allow rule does not match.
        assert!(matches!(
            policy.authorize("bash", r#"{"command":"rm -rf /tmp/x"}"#, None),
            PermissionOutcome::Deny { .. }
        ));
    }

    #[test]
    fn legacy_prefix_and_exact_matchers_are_unchanged() {
        // The `:*` suffix stays a literal prefix (no glob meaning inside it),
        // and a plain subject stays an exact match — guarding backward compat.
        let rules = RuntimePermissionRuleConfig::new(
            vec![
                "bash(npm run test:*)".to_string(),
                "bash(cargo build)".to_string(),
            ],
            Vec::new(),
            Vec::new(),
        );
        let policy = PermissionPolicy::new(PermissionMode::ReadOnly)
            .with_tool_requirement("bash", PermissionMode::DangerFullAccess)
            .with_permission_rules(&rules);

        // Prefix form.
        assert_eq!(
            policy.authorize("bash", r#"{"command":"npm run test:unit"}"#, None),
            PermissionOutcome::Allow
        );
        // Exact form — only the literal command matches. (The probes must not
        // be provably read-only, or the classifier allows them without rules.)
        assert_eq!(
            policy.authorize("bash", r#"{"command":"cargo build"}"#, None),
            PermissionOutcome::Allow
        );
        assert!(matches!(
            policy.authorize("bash", r#"{"command":"cargo build --release"}"#, None),
            PermissionOutcome::Deny { .. }
        ));
    }

    #[test]
    fn malformed_glob_pattern_falls_back_to_exact_without_panic() {
        // An unterminated `[` class is an invalid glob. Parsing must NOT panic
        // and must degrade to a literal `Exact` match — not silently drop the
        // rule, which would disable a deny.
        let rules = RuntimePermissionRuleConfig::new(
            Vec::new(),
            vec!["bash([oops)".to_string()],
            Vec::new(),
        );
        let policy = PermissionPolicy::new(PermissionMode::DangerFullAccess)
            .with_tool_requirement("bash", PermissionMode::DangerFullAccess)
            .with_permission_rules(&rules);

        // The literal subject matches the Exact fallback → denied.
        assert!(matches!(
            policy.authorize("bash", r#"{"command":"[oops"}"#, None),
            PermissionOutcome::Deny { .. }
        ));
        // Anything else is unaffected.
        assert_eq!(
            policy.authorize("bash", r#"{"command":"echo hi"}"#, None),
            PermissionOutcome::Allow
        );
    }

    #[test]
    fn glob_rules_respect_deny_ask_allow_precedence() {
        // All three rule classes are globs over `git` subcommands. The
        // existing deny > ask > allow ordering must hold for globs too: deny
        // wins outright, ask outranks allow and forces a prompt, allow is the
        // floor.
        let rules = RuntimePermissionRuleConfig::new(
            vec!["bash(git *)".to_string()],       // allow: any git command
            vec!["bash(git push*)".to_string()],   // deny: pushes
            vec!["bash(git commit*)".to_string()], // ask: commits
        );
        let policy = PermissionPolicy::new(PermissionMode::DangerFullAccess)
            .with_tool_requirement("bash", PermissionMode::DangerFullAccess)
            .with_permission_rules(&rules);

        // deny ∩ allow → deny wins.
        assert!(matches!(
            policy.authorize("bash", r#"{"command":"git push origin"}"#, None),
            PermissionOutcome::Deny { .. }
        ));

        // ask ∩ allow → prompt (ask outranks allow even though mode allows).
        let mut prompter = RecordingPrompter {
            seen: Vec::new(),
            allow: true,
        };
        let outcome = policy.authorize(
            "bash",
            r#"{"command":"git commit -m x"}"#,
            Some(&mut prompter),
        );
        assert_eq!(outcome, PermissionOutcome::Allow);
        assert_eq!(prompter.seen.len(), 1);
        assert!(
            prompter.seen[0]
                .reason
                .as_deref()
                .is_some_and(|reason| reason.contains("ask rule"))
        );

        // allow only → allowed without a prompt.
        assert_eq!(
            policy.authorize("bash", r#"{"command":"git status"}"#, None),
            PermissionOutcome::Allow
        );
    }

    fn opencode_ordered_policy() -> PermissionPolicy {
        let rules = RuntimePermissionRuleConfig::new(Vec::new(), Vec::new(), Vec::new())
            .with_rules(vec![
                "bash(*)=ask".to_string(),
                "bash(git *)=allow".to_string(),
                "bash(git push*)=deny".to_string(),
            ]);
        PermissionPolicy::new(PermissionMode::DangerFullAccess)
            .with_tool_requirement("bash", PermissionMode::DangerFullAccess)
            .with_permission_rules(&rules)
    }

    #[test]
    fn opencode_ordered_rules_last_match_wins() {
        let policy = opencode_ordered_policy();

        // `git status`: matches bash(*) and bash(git *); last match (allow) wins.
        assert_eq!(
            policy.authorize("bash", r#"{"command":"git status"}"#, None),
            PermissionOutcome::Allow
        );

        // `git push origin main`: also matches bash(git push*); last match (deny) wins.
        assert!(matches!(
            policy.authorize("bash", r#"{"command":"git push origin main"}"#, None),
            PermissionOutcome::Deny { reason } if reason.contains("bash(git push*)")
        ));
    }

    #[test]
    fn opencode_ordered_rules_fall_through_to_ask_then_prompt() {
        let policy = opencode_ordered_policy();
        let mut prompter = RecordingPrompter {
            seen: Vec::new(),
            allow: true,
        };

        // `npm test` matches only bash(*)=ask → prompts even in full-access mode.
        let outcome = policy.authorize("bash", r#"{"command":"npm test"}"#, Some(&mut prompter));
        assert_eq!(outcome, PermissionOutcome::Allow);
        assert_eq!(prompter.seen.len(), 1);
        assert!(
            prompter.seen[0]
                .reason
                .as_deref()
                .is_some_and(|reason| reason.contains("bash(*)"))
        );
    }

    #[test]
    fn opencode_ordered_deny_overrides_hook_allow() {
        use super::PermissionContext;
        let policy = opencode_ordered_policy();
        let context = PermissionContext::new(Some(PermissionOverride::Allow), None);

        // A matched ordered deny is strong: it wins even over a hook Allow.
        assert!(matches!(
            policy.authorize_with_context(
                "bash",
                r#"{"command":"git push --force"}"#,
                &context,
                None
            ),
            PermissionOutcome::Deny { .. }
        ));
    }

    #[test]
    fn ordered_rules_do_not_affect_unconfigured_tools() {
        // Ordered rules only mention `bash`; another tool falls through to the
        // mode-based default (here: allowed because mode meets the requirement).
        let policy =
            opencode_ordered_policy().with_tool_requirement("read_file", PermissionMode::ReadOnly);
        assert_eq!(
            policy.authorize("read_file", "{}", None),
            PermissionOutcome::Allow
        );
    }

    #[test]
    fn ordered_rules_supersede_legacy_category_vectors() {
        // Config load rejects mixing the two forms, but a directly-built policy
        // must still honor the documented contract: once ordered rules exist they
        // are the sole authority and the legacy deny vector is superseded.
        let config = RuntimePermissionRuleConfig::new(
            Vec::new(),
            vec!["edit_file(*.env)".to_string()], // legacy deny on dotenv files
            Vec::new(),
        )
        .with_rules(vec!["edit_file(*)=allow".to_string()]); // ordered allow-all
        let policy = PermissionPolicy::new(PermissionMode::DangerFullAccess)
            .with_tool_requirement("edit_file", PermissionMode::WorkspaceWrite)
            .with_permission_rules(&config);

        assert_eq!(
            policy.authorize("edit_file", r#"{"file_path":".env"}"#, None),
            PermissionOutcome::Allow
        );
    }

    #[test]
    fn decision_rule_spec_parsing_reports_actionable_errors() {
        use super::{PermissionRuleSpecError, parse_decision_rule, validate_decision_rule_spec};

        assert!(parse_decision_rule("bash(git *)=allow").is_ok());
        // Subjects may contain '=' because we split on the final '='.
        assert!(parse_decision_rule("bash(FOO=bar)=deny").is_ok());

        assert_eq!(
            validate_decision_rule_spec("bash(git *)"),
            Err(PermissionRuleSpecError::MissingAction {
                spec: "bash(git *)".to_string()
            })
        );
        assert_eq!(
            validate_decision_rule_spec("bash(git *)=maybe"),
            Err(PermissionRuleSpecError::UnknownAction {
                spec: "bash(git *)=maybe".to_string(),
                action: "maybe".to_string()
            })
        );
    }
}
