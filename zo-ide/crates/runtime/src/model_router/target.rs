#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RouteRole {
    Default,
    Fast,
    Coding,
    Debugging,
    Verifier,
    Reviewer,
    Analysis,
    Research,
    Writing,
    Design,
    Judge,
    Synthesizer,
}

impl RouteRole {
    /// Every routable role, in display order. Used to build per-role fallback
    /// recommendations for the `/smart` dashboard.
    #[must_use]
    pub fn all() -> &'static [Self] {
        &[
            Self::Default,
            Self::Fast,
            Self::Coding,
            Self::Debugging,
            Self::Verifier,
            Self::Reviewer,
            Self::Analysis,
            Self::Research,
            Self::Writing,
            Self::Design,
            Self::Judge,
            Self::Synthesizer,
        ]
    }

    /// Stable settings key for this role (matches the `/smart` role target keys).
    #[must_use]
    pub fn key(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Fast => "fast",
            Self::Coding => "coding",
            Self::Debugging => "debugging",
            Self::Verifier => "verifier",
            Self::Reviewer => "reviewer",
            Self::Analysis => "analysis",
            Self::Research => "research",
            Self::Writing => "writing",
            Self::Design => "design",
            Self::Judge => "judge",
            Self::Synthesizer => "synthesizer",
        }
    }

    /// Exact inverse of [`Self::key`] — parses the SAME lowercase key back
    /// into a role. Added for Phase 6 (`model_router::learned`): the v2
    /// outcome schema's `role` field is persisted as this key (`tools::
    /// smart_router::infer::role_key`, which mirrors this projection), so the
    /// learned-specialty aggregator — living in `runtime`, which cannot
    /// depend on the `tools` crate's classifier — needs its own way to parse
    /// that string back into a `RouteRole` without duplicating the tools
    /// crate's keyword-inference logic (which this deliberately does NOT
    /// attempt: an unrecognized/foreign key returns `None`, never a guess).
    #[must_use]
    pub fn from_key(key: &str) -> Option<Self> {
        match key {
            "default" => Some(Self::Default),
            "fast" => Some(Self::Fast),
            "coding" => Some(Self::Coding),
            "debugging" => Some(Self::Debugging),
            "verifier" => Some(Self::Verifier),
            "reviewer" => Some(Self::Reviewer),
            "analysis" => Some(Self::Analysis),
            "research" => Some(Self::Research),
            "writing" => Some(Self::Writing),
            "design" => Some(Self::Design),
            "judge" => Some(Self::Judge),
            "synthesizer" => Some(Self::Synthesizer),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum BuiltinSubagentProfile {
    GeneralPurpose,
    Explore,
    Plan,
    Verification,
    DeepResearch,
    CodeReviewer,
    Debugger,
    DataAnalyst,
    Refactor,
    FrontendDesign,
    ZoGuide,
    StatuslineSetup,
}

impl BuiltinSubagentProfile {
    #[must_use]
    pub fn key(self) -> &'static str {
        match self {
            Self::GeneralPurpose => "general-purpose",
            Self::Explore => "Explore",
            Self::Plan => "Plan",
            Self::Verification => "Verification",
            Self::DeepResearch => "deep-research",
            Self::CodeReviewer => "code-reviewer",
            Self::Debugger => "debugger",
            Self::DataAnalyst => "data-analyst",
            Self::Refactor => "refactor",
            Self::FrontendDesign => "frontend-design",
            Self::ZoGuide => "zo-guide",
            Self::StatuslineSetup => "statusline-setup",
        }
    }

    #[must_use]
    pub fn route_role(self) -> RouteRole {
        match self {
            Self::GeneralPurpose | Self::Refactor => RouteRole::Coding,
            Self::Explore | Self::StatuslineSetup => RouteRole::Fast,
            Self::Plan | Self::DataAnalyst => RouteRole::Analysis,
            Self::Verification => RouteRole::Verifier,
            Self::CodeReviewer => RouteRole::Reviewer,
            Self::Debugger => RouteRole::Debugging,
            Self::DeepResearch => RouteRole::Research,
            Self::FrontendDesign => RouteRole::Design,
            Self::ZoGuide => RouteRole::Writing,
        }
    }

    /// One-line purpose used by command surfaces that enumerate the built-in
    /// agent taxonomy.
    #[must_use]
    pub fn purpose(self) -> &'static str {
        match self {
            Self::GeneralPurpose => "Complete delegated work end-to-end with the full toolset.",
            Self::Explore => "Explore the codebase read-only and return concise file references.",
            Self::Plan => "Investigate the code and produce a concrete implementation plan.",
            Self::Verification => "Build, test, and lint work with exact pass/fail evidence.",
            Self::DeepResearch => "Research in multiple passes and synthesize cited findings.",
            Self::CodeReviewer => "Review code adversarially for concrete correctness risks.",
            Self::Debugger => "Reproduce failures, isolate root causes, and apply minimal fixes.",
            Self::DataAnalyst => "Analyze data, logs, or metrics and report concrete numbers.",
            Self::Refactor => "Improve code structure without changing behavior.",
            Self::FrontendDesign => "Handle frontend implementation with a design-focused role.",
            Self::ZoGuide => "Explain zo behavior and conventions from the source.",
            Self::StatuslineSetup => "Configure and verify the requested status line.",
        }
    }

    #[must_use]
    pub fn all() -> &'static [Self] {
        &[
            Self::GeneralPurpose,
            Self::Explore,
            Self::Plan,
            Self::Verification,
            Self::DeepResearch,
            Self::CodeReviewer,
            Self::Debugger,
            Self::DataAnalyst,
            Self::Refactor,
            Self::FrontendDesign,
            Self::ZoGuide,
            Self::StatuslineSetup,
        ]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SubagentProfileKind { Builtin, Custom }

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct SubagentProfileId {
    key: String,
    kind: SubagentProfileKind,
}

impl SubagentProfileId {
    #[must_use]
    pub fn builtin(profile: BuiltinSubagentProfile) -> Self {
        Self { key: profile.key().to_string(), kind: SubagentProfileKind::Builtin }
    }

    #[must_use]
    pub fn custom(name: impl AsRef<str>) -> Option<Self> {
        let normalized = normalize_custom_key(name.as_ref())?;
        Some(Self { key: format!("custom:{normalized}"), kind: SubagentProfileKind::Custom })
    }

    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        let trimmed = value.trim();
        if trimmed.is_empty() { return None; }
        if let Some(custom) = trimmed.strip_prefix("custom:") { return Self::custom(custom); }
        BuiltinSubagentProfile::all()
            .iter()
            .copied()
            .find(|profile| profile.key().eq_ignore_ascii_case(trimmed))
            .map(Self::builtin)
            .or_else(|| Self::custom(trimmed))
    }

    #[must_use]
    pub fn key(&self) -> &str { &self.key }

    #[must_use]
    pub fn kind(&self) -> SubagentProfileKind { self.kind }

    #[must_use]
    pub fn builtin_profile(&self) -> Option<BuiltinSubagentProfile> {
        if self.kind != SubagentProfileKind::Builtin { return None; }
        BuiltinSubagentProfile::all()
            .iter()
            .copied()
            .find(|profile| profile.key() == self.key)
    }

    #[must_use]
    pub fn route_role_hint(&self) -> Option<RouteRole> {
        self.builtin_profile().map(BuiltinSubagentProfile::route_role)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoutingTarget {
    Foreground,
    Subagent(SubagentProfileId),
    RoleFallback(RouteRole),
    WorkflowPhase { phase_id: String, subagent: Option<SubagentProfileId> },
    Synthesis,
    Judge,
}

impl RoutingTarget {
    #[must_use]
    pub fn route_role_hint(&self) -> Option<RouteRole> {
        match self {
            Self::Foreground => Some(RouteRole::Default),
            Self::Subagent(profile) | Self::WorkflowPhase { subagent: Some(profile), .. } => {
                profile.route_role_hint()
            }
            Self::RoleFallback(role) => Some(*role),
            Self::WorkflowPhase { subagent: None, .. } => None,
            Self::Synthesis => Some(RouteRole::Synthesizer),
            Self::Judge => Some(RouteRole::Judge),
        }
    }
}

fn normalize_custom_key(name: &str) -> Option<String> {
    let normalized = name
        .trim()
        .trim_start_matches("custom:")
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' { ch.to_ascii_lowercase() } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .to_string();
    (!normalized.is_empty()).then_some(normalized)
}
