/// Permission level assigned to a tool invocation or runtime session.
///
/// Deliberately NOT `Ord`/`PartialOrd`: the variants do not form a total order.
/// `ReadOnly` → `WorkspaceWrite` → `DangerFullAccess` is a privilege ladder, but
/// `Prompt` (ask the user) and `Allow` (decide by rule) describe *how* to decide,
/// not a static access level. A derived `Ord` ordered them *above*
/// `DangerFullAccess` by declaration index, so `mode >= required` silently
/// treated `Prompt`/`Allow` as the highest privilege — use [`Self::satisfies`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionMode {
    ReadOnly,
    WorkspaceWrite,
    DangerFullAccess,
    Prompt,
    Allow,
}

impl PermissionMode {
    /// Rank on the privilege ladder `ReadOnly` < `WorkspaceWrite` <
    /// `DangerFullAccess`. `Prompt`/`Allow` are not static access levels and so
    /// have no rank.
    fn privilege_rank(self) -> Option<u8> {
        match self {
            Self::ReadOnly => Some(0),
            Self::WorkspaceWrite => Some(1),
            Self::DangerFullAccess => Some(2),
            Self::Prompt | Self::Allow => None,
        }
    }

    /// Whether this mode's static privilege level meets or exceeds `required`.
    ///
    /// Only the `ReadOnly` → `WorkspaceWrite` → `DangerFullAccess` ladder counts.
    /// `Prompt` and `Allow` have no rank, so they never satisfy a requirement *by
    /// privilege* — their authorization is decided by their own paths (an
    /// interactive prompt / a rule match), never by this comparison. Replaces the
    /// removed `mode >= required`, which mis-ranked `Prompt`/`Allow` as the
    /// highest privilege.
    #[must_use]
    pub fn satisfies(self, required: PermissionMode) -> bool {
        matches!(
            (self.privilege_rank(), required.privilege_rank()),
            (Some(current), Some(required)) if current >= required
        )
    }

    /// Clamp this mode to `ceiling` on the privilege ladder: a ranked mode
    /// above a ranked ceiling collapses to the ceiling; every other
    /// combination (either side `Prompt`/`Allow`) is returned unchanged,
    /// since unranked modes resolve authorization through their own paths.
    #[must_use]
    pub fn clamp_to(self, ceiling: Self) -> Self {
        match (self.privilege_rank(), ceiling.privilege_rank()) {
            (Some(own), Some(max)) if own > max => ceiling,
            _ => self,
        }
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ReadOnly => "read-only",
            Self::WorkspaceWrite => "workspace-write",
            Self::DangerFullAccess => "danger-full-access",
            Self::Prompt => "prompt",
            Self::Allow => "allow",
        }
    }

    /// Lenient inverse of [`Self::as_str`]: parse a canonical mode name,
    /// ignoring surrounding whitespace and ASCII case. Returns `None` for an
    /// unrecognized value rather than panicking, so callers can decide whether
    /// to fail closed (reject) or fall back to a default.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "read-only" => Some(Self::ReadOnly),
            "workspace-write" => Some(Self::WorkspaceWrite),
            "danger-full-access" => Some(Self::DangerFullAccess),
            "prompt" => Some(Self::Prompt),
            "allow" => Some(Self::Allow),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::PermissionMode;

    #[test]
    fn parse_round_trips_every_canonical_name() {
        for mode in [
            PermissionMode::ReadOnly,
            PermissionMode::WorkspaceWrite,
            PermissionMode::DangerFullAccess,
            PermissionMode::Prompt,
            PermissionMode::Allow,
        ] {
            assert_eq!(PermissionMode::parse(mode.as_str()), Some(mode));
        }
    }

    #[test]
    fn parse_ignores_whitespace_and_case() {
        assert_eq!(
            PermissionMode::parse("  Read-Only "),
            Some(PermissionMode::ReadOnly)
        );
        assert_eq!(
            PermissionMode::parse("DANGER-FULL-ACCESS"),
            Some(PermissionMode::DangerFullAccess)
        );
    }

    #[test]
    fn parse_rejects_unknown_without_panicking() {
        assert_eq!(PermissionMode::parse("read only"), None);
        assert_eq!(PermissionMode::parse("raed-only"), None);
        assert_eq!(PermissionMode::parse(""), None);
    }

    #[test]
    fn satisfies_respects_the_privilege_ladder() {
        use PermissionMode::{DangerFullAccess, ReadOnly, WorkspaceWrite};
        // Higher (or equal) privilege satisfies the requirement.
        assert!(DangerFullAccess.satisfies(WorkspaceWrite));
        assert!(WorkspaceWrite.satisfies(ReadOnly));
        assert!(ReadOnly.satisfies(ReadOnly));
        // Lower privilege does not.
        assert!(!ReadOnly.satisfies(WorkspaceWrite));
        assert!(!WorkspaceWrite.satisfies(DangerFullAccess));
    }

    #[test]
    fn prompt_and_allow_never_satisfy_by_privilege() {
        use PermissionMode::{Allow, DangerFullAccess, Prompt, ReadOnly};
        // The foot-gun guard: Prompt/Allow are off the ladder, so they never
        // auto-satisfy a required privilege (the old derived `Ord` made
        // `Prompt`/`Allow` >= `DangerFullAccess` — full access by accident).
        for required in [ReadOnly, DangerFullAccess] {
            assert!(
                !Prompt.satisfies(required),
                "Prompt must not satisfy {required:?}"
            );
            assert!(
                !Allow.satisfies(required),
                "Allow must not satisfy {required:?}"
            );
        }
    }

    #[test]
    fn clamp_to_collapses_only_ranked_excess() {
        use PermissionMode::{Allow, DangerFullAccess, Prompt, ReadOnly, WorkspaceWrite};
        // Ranked above a ranked ceiling → the ceiling.
        assert_eq!(DangerFullAccess.clamp_to(ReadOnly), ReadOnly);
        assert_eq!(DangerFullAccess.clamp_to(WorkspaceWrite), WorkspaceWrite);
        assert_eq!(WorkspaceWrite.clamp_to(ReadOnly), ReadOnly);
        // At or below the ceiling → unchanged.
        assert_eq!(ReadOnly.clamp_to(DangerFullAccess), ReadOnly);
        assert_eq!(WorkspaceWrite.clamp_to(WorkspaceWrite), WorkspaceWrite);
        // Unranked on either side → unchanged (their authorization paths
        // gate interactively, not by static privilege).
        assert_eq!(DangerFullAccess.clamp_to(Prompt), DangerFullAccess);
        assert_eq!(DangerFullAccess.clamp_to(Allow), DangerFullAccess);
        assert_eq!(Prompt.clamp_to(ReadOnly), Prompt);
    }
}
