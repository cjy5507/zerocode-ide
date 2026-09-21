//! 노력(effort) 단계 — 사고 예산과 와이어 `api::EffortLevel` 매핑의 단일 진실.
//!
//! 포팅: forge-code `crates/zo-cli/src/tui/modals/effort_picker.rs` 의 `Effort`
//! 열거형과 그 impl 만 (슬라이더/모달 UI 는 가져오지 않음). 도메인 타입이
//! TUI 모듈에 살고 있었던 것을 엔진 곁으로 옮긴 것이다.

/// Canonical effort level. Single source of truth for the thinking
/// budget, the slash-command preset table ([`crate::tui`] re-exports it
/// for the dispatcher), and the slider stops in `EFFORT_STEPS`.
///
/// `Smart` additionally injects a parallel-orchestration system
/// reminder at turn time (handled by the session layer), which is why
/// its description mentions agent fan-out rather than a raw budget —
/// and unlike every other level, its wire effort is a DYNAMIC BAND
/// (`[xhigh .. model ceiling]`, resolved per request by
/// `api::resolve_effort_band`) rather than one static tier: see
/// [`Self::level`] (returns the band floor) and [`Self::band_ceiling`]
/// (the band top).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effort {
    /// Extended thinking disabled.
    Off,
    Low,
    Medium,
    High,
    Xhigh,
    Max,
    /// Static top-tier pin, one rung above `Max`. Unlike `Smart`, this is a
    /// single named wire level with no per-request escalation and no
    /// orchestration hint — the "just always send the real top tier" preset.
    Ultra,
    /// Dynamic band `[xhigh .. model ceiling]` plus a parallel-agent
    /// orchestration hint. Formerly named `Ultracode`; still accepts
    /// `ultracode` as a legacy alias (including in persisted settings).
    Smart,
}

impl Effort {
    /// Every level in canonical order (`Off` first, `Smart` last).
    /// The slider in `EFFORT_STEPS` skips `Off`.
    pub const ALL: &'static [Effort] = &[
        Effort::Off,
        Effort::Low,
        Effort::Medium,
        Effort::High,
        Effort::Xhigh,
        Effort::Max,
        Effort::Ultra,
        Effort::Smart,
    ];

    /// Thinking-token budget. `0` disables extended thinking.
    #[must_use]
    pub const fn budget(self) -> u32 {
        match self {
            Effort::Off => 0,
            Effort::Low => 1_024,
            Effort::Medium => 4_096,
            Effort::High => 10_000,
            Effort::Xhigh => 16_000,
            Effort::Max => 24_000,
            // Between Max (24_000) and Smart (28_000) — a distinct value so
            // `from_budget`/`step_for_budget` reverse lookups can tell the two
            // apart. Matches the CLI ladder order (Max < Ultra < Smart), which
            // also mirrors `effort_rank`'s Max < Ultra ordering (`runtime_bridge.rs`).
            Effort::Ultra => 26_000,
            // Above Ultra's 26_000 so budget-derived paths (`effort_level_for_budget`,
            // which has no Ultra bucket and tops out at Max) tier Smart at least
            // as high as Max/Ultra. Smart's *named* wire tier is the band floor
            // (`Xhigh`, see `level()`) carried independently of this legacy
            // budget, so this value must still remain distinct from Ultra's
            // 26_000 for `from_budget`/`step_for_budget`.
            Effort::Smart => 28_000,
        }
    }

    /// The provider-neutral [`api::EffortLevel`] this level sends on the wire, or
    /// `None` for [`Effort::Off`] (no effort control — the backend default
    /// applies). This is the single source of truth for the CLI level → wire
    /// effort mapping, so a headless `ZO_EFFORT=max` reaches Anthropic as
    /// `output_config.effort="max"` and reaches GPT through the model-specific
    /// projection (`max` only on confirmed GPT-5.6 families, otherwise `xhigh`)
    /// instead of being re-derived from a thinking budget and landing a tier low.
    ///
    /// `Max` maps to `EffortLevel::Max`. `Ultra` maps to `EffortLevel::Ultra` as
    /// a STATIC pin (no band) — the wire always carries the real top tier
    /// (clamped per-provider same as before). `Smart` maps to
    /// `EffortLevel::Xhigh`, the dynamic band's FLOOR — see [`Self::band_ceiling`]
    /// for the band's top; callers building a wire request must set BOTH.
    #[must_use]
    // Xhigh and Smart deliberately return the identical `Some(L::Xhigh)`
    // today (Smart's floor happens to equal the static Xhigh tier) — kept as
    // separate arms rather than merged because they mean different things
    // (a static pin vs. a dynamic band's floor) and are free to diverge
    // independently later; do not let clippy quietly collapse them.
    #[allow(clippy::match_same_arms)]
    pub const fn level(self) -> Option<api::EffortLevel> {
        use api::EffortLevel as L;
        match self {
            Effort::Off => None,
            Effort::Low => Some(L::Low),
            Effort::Medium => Some(L::Medium),
            Effort::High => Some(L::High),
            Effort::Xhigh => Some(L::Xhigh),
            Effort::Max => Some(L::Max),
            Effort::Ultra => Some(L::Ultra),
            Effort::Smart => Some(L::Xhigh),
        }
    }

    /// The dynamic band's ceiling — `Some(EffortLevel::Max)` for
    /// [`Effort::Smart`] only, `None` for every static level (including
    /// [`Effort::Ultra`], which has no band). Callers building a
    /// [`api::MessageRequest`] set `effort: level()` and
    /// `effort_band_ceiling: band_ceiling()`; the wire backends resolve the
    /// band via `api::resolve_effort_band` per request. `None` here is what
    /// keeps every other preset's wire behavior byte-identical to before
    /// dynamic bands existed.
    ///
    /// Smart is deliberately a two-rung `xhigh → max` band, NOT `xhigh → ultra`.
    /// `Ultra` stays reachable, but only as the explicit static [`Effort::Ultra`]
    /// pin: an automatic band should not be able to spend the top tier on a
    /// difficulty guess. Escalating to `max` is the ceiling Smart may choose on
    /// its own; going past it is the operator's call.
    #[must_use]
    pub const fn band_ceiling(self) -> Option<api::EffortLevel> {
        match self {
            Effort::Smart => Some(api::EffortLevel::Max),
            Effort::Off
            | Effort::Low
            | Effort::Medium
            | Effort::High
            | Effort::Xhigh
            | Effort::Max
            | Effort::Ultra => None,
        }
    }

    /// Primary name shown in banners and under the slider.
    #[must_use]
    pub const fn canonical(self) -> &'static str {
        match self {
            Effort::Off => "off",
            Effort::Low => "low",
            Effort::Medium => "medium",
            Effort::High => "high",
            Effort::Xhigh => "xhigh",
            Effort::Max => "max",
            Effort::Ultra => "ultra",
            Effort::Smart => "smart",
        }
    }

    /// Accepted aliases (matched case-insensitively at parse time).
    #[must_use]
    pub const fn aliases(self) -> &'static [&'static str] {
        match self {
            Effort::Off => &["none", "disable"],
            Effort::Medium => &["med"],
            // Legacy spellings — `ultra` itself moved to its own static level
            // (`Effort::Ultra`) and is deliberately NOT an alias here anymore;
            // `ultracode` MUST keep parsing (persisted settings/scripts) and
            // `smartcode`/`uc` are the new/short spellings for the same preset.
            Effort::Smart => &["smartcode", "ultracode", "uc"],
            Effort::Low | Effort::High | Effort::Xhigh | Effort::Max | Effort::Ultra => &[],
        }
    }

    /// Aliases worth advertising in user-facing level tables.
    #[must_use]
    pub const fn display_aliases(self) -> &'static [&'static str] {
        match self {
            Effort::Max => &[],
            other => other.aliases(),
        }
    }

    /// One-line hint for the levels table and the slider.
    #[must_use]
    pub const fn description(self) -> &'static str {
        match self {
            Effort::Off => "no extended thinking",
            Effort::Low => "quick answers, short reasoning",
            Effort::Medium => "balanced reasoning",
            Effort::High => "deep reasoning, longer turns",
            Effort::Xhigh => "extended thinking + bigger budget",
            Effort::Max => "maximum thinking budget",
            Effort::Ultra => "true top-tier reasoning, no orchestration",
            // Deliberately arrow-free: this string is always shown in the
            // full levels table (`format_effort_levels`), including when
            // rendering a status banner for a DIFFERENT level — a "→" here
            // would make any "no arrow" assertion about that banner false
            // regardless of which level is actually active.
            Effort::Smart => "dynamic top band, xhigh up to ceiling, + parallel agent orchestration",
        }
    }

    /// Resolve a level from a token (canonical name or alias),
    /// case-insensitively. Returns `None` for unrecognized input.
    #[must_use]
    pub fn from_token(token: &str) -> Option<Effort> {
        Self::ALL.iter().copied().find(|level| {
            token.eq_ignore_ascii_case(level.canonical())
                || level
                    .aliases()
                    .iter()
                    .any(|alias| token.eq_ignore_ascii_case(alias))
        })
    }

    /// The static preset that sends `level` — the road back from a wire level
    /// to the ladder's own thinking budget ([`Self::budget`]). The step effort
    /// governor lowers or raises a request's level by a rung, and a legacy
    /// budget model must then spend that rung's budget rather than the turn's:
    /// with `High` on the wire, `High`'s 10,000, not `Smart`'s 28,000.
    /// `None` only for a level no static preset sends, which the ladder has
    /// none of today.
    #[must_use]
    pub fn for_level(level: api::EffortLevel) -> Option<Effort> {
        Self::ALL
            .iter()
            .copied()
            .find(|preset| preset.band_ceiling().is_none() && preset.level() == Some(level))
    }

    /// Resolve the level whose budget matches `budget` exactly. A `None`
    /// or zero budget maps to [`Effort::Off`]; any other unmatched value
    /// returns `None` (the caller treats it as a custom budget).
    #[must_use]
    pub fn from_budget(budget: Option<u32>) -> Option<Effort> {
        match budget {
            None | Some(0) => Some(Effort::Off),
            Some(value) => Self::ALL
                .iter()
                .copied()
                .find(|level| level.budget() == value),
        }
    }

    /// For [`Effort::Smart`] only: the dynamic band's floor/ceiling display
    /// labels as actually projected onto `model` via
    /// `api::effective_effort_for_model` — e.g. `("xhigh", "max")` on sol and
    /// fable, or a degenerate `("xhigh", "xhigh")` / `("high", "high")` where
    /// the model's ceiling collapses the whole band onto one rung. `None` for
    /// every other level (no band to show).
    ///
    /// Truth surfaces (`hud::effort_badge_label`, `/effort show`) use this
    /// instead of the single-tier clamp check every other preset gets, since
    /// Smart's `level()` is only the band FLOOR — showing just that would
    /// silently hide the escalation headroom the preset actually has.
    #[must_use]
    pub fn band_labels_for_model(self, model: &str) -> Option<(&'static str, &'static str)> {
        if self != Effort::Smart {
            return None;
        }
        let floor = self.level()?;
        let ceiling = self.band_ceiling()?;
        // Mirror the model-capability selection path in two steps: (1) resolve
        // the band the same way the backends do (`api::resolve_effort_band`,
        // which already caps against the model's internal ceiling — e.g. Max
        // for Luna); (2) run the result through the same model-specific
        // capability projection every other level's display uses
        // (`api::effective_effort_for_model`) — this is what applies e.g.
        // Anthropic's sonnet/haiku "no xhigh" clamp on top of the band pick,
        // so a floor of Xhigh on Sonnet correctly displays as "high".
        let no_signals = api::BandDifficulty::default();
        let max_signals = api::BandDifficulty {
            heavy_intent: true,
            large_context: true,
            long_ask: true,
        };
        let floor_resolved = api::resolve_effort_band(floor, ceiling, model, no_signals);
        let ceiling_resolved = api::resolve_effort_band(floor, ceiling, model, max_signals);
        let floor_label = effort_level_label(api::effective_effort_for_model(floor_resolved, model));
        let ceiling_label =
            effort_level_label(api::effective_effort_for_model(ceiling_resolved, model));
        Some((floor_label, ceiling_label))
    }
}

/// 와이어 effort 단계의 소문자 라벨(`/effort` 표기·상태 줄).
#[must_use]
pub fn effort_level_label(level: api::EffortLevel) -> &'static str {
    use api::EffortLevel as L;
    match level {
        L::Low => "low",
        L::Medium => "medium",
        L::High => "high",
        L::Xhigh => "xhigh",
        L::Max => "max",
        L::Ultra => "ultra",
    }
}
