use serde::{Deserialize, Serialize};

use crate::hook::HookState;
use crate::lane::LaneState;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SurfaceClass {
    Empty,
    Active,
    Idle,
    Paused,
    Streaming,
    Waiting,
    Blocked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WorktreeIndicator {
    Inactive,
    Active,
    Done,
    Paused,
    Working,
    Permission,
}

impl SurfaceClass {
    pub const ALL: [Self; 7] = [
        Self::Empty,
        Self::Active,
        Self::Idle,
        Self::Paused,
        Self::Streaming,
        Self::Waiting,
        Self::Blocked,
    ];

    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::Empty => "empty",
            Self::Active => "active",
            Self::Idle => "idle",
            Self::Paused => "paused",
            Self::Streaming => "streaming",
            Self::Waiting => "waiting",
            Self::Blocked => "blocked",
        }
    }

    #[must_use]
    pub const fn rank(self) -> u8 {
        match self {
            Self::Empty => 0,
            Self::Active => 1,
            Self::Idle => 2,
            Self::Paused => 3,
            Self::Streaming => 4,
            Self::Waiting => 5,
            Self::Blocked => 6,
        }
    }

    #[must_use]
    pub const fn is_live(self) -> bool {
        matches!(self, Self::Streaming | Self::Waiting | Self::Blocked)
    }

    #[must_use]
    pub const fn indicator(self) -> WorktreeIndicator {
        match self {
            Self::Empty => WorktreeIndicator::Inactive,
            Self::Active => WorktreeIndicator::Active,
            Self::Idle => WorktreeIndicator::Done,
            Self::Paused => WorktreeIndicator::Paused,
            Self::Streaming => WorktreeIndicator::Working,
            Self::Waiting | Self::Blocked => WorktreeIndicator::Permission,
        }
    }

    #[must_use]
    pub fn fold(states: impl IntoIterator<Item = Self>) -> Self {
        states
            .into_iter()
            .max_by_key(|state| state.rank())
            .unwrap_or(Self::Empty)
    }
}

impl WorktreeIndicator {
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::Inactive => "inactive",
            Self::Active => "active",
            Self::Done => "done",
            Self::Paused => "paused",
            Self::Working => "working",
            Self::Permission => "permission",
        }
    }
}

impl HookState {
    pub const ALL: [Self; 4] = [Self::Working, Self::NeedsAttention, Self::Done, Self::Idle];

    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::Working => "working",
            Self::NeedsAttention => "needs-attention",
            Self::Done => "done",
            Self::Idle => "idle",
        }
    }

    #[must_use]
    pub const fn surface_class(self) -> SurfaceClass {
        match self {
            Self::Working => SurfaceClass::Streaming,
            Self::NeedsAttention => SurfaceClass::Waiting,
            Self::Done | Self::Idle => SurfaceClass::Idle,
        }
    }

    #[must_use]
    pub const fn is_live(self) -> bool {
        self.surface_class().is_live()
    }
}

impl LaneState {
    pub const ALL: [Self; 5] = [
        Self::Idle,
        Self::Streaming,
        Self::AwaitingPermission,
        Self::Blocked,
        Self::Exited,
    ];

    #[must_use]
    pub const fn surface_class(self) -> SurfaceClass {
        match self {
            Self::Idle => SurfaceClass::Idle,
            Self::Streaming => SurfaceClass::Streaming,
            Self::AwaitingPermission => SurfaceClass::Waiting,
            Self::Blocked => SurfaceClass::Blocked,
            Self::Exited => SurfaceClass::Empty,
        }
    }

    #[must_use]
    pub const fn is_live(self) -> bool {
        self.surface_class().is_live()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hook_and_lane_states_project_consistently_and_serialize_to_word() {
        for state in HookState::ALL {
            let encoded = serde_json::to_string(&state).expect("hook state serializes");
            assert_eq!(encoded, format!("\"{}\"", state.word()));
            assert_eq!(state.is_live(), state.surface_class().is_live());
        }
        assert_eq!(HookState::Working.surface_class(), SurfaceClass::Streaming);
        assert_eq!(
            HookState::NeedsAttention.surface_class(),
            SurfaceClass::Waiting
        );
        assert_eq!(HookState::Done.surface_class(), SurfaceClass::Idle);
        assert_eq!(HookState::Idle.surface_class(), SurfaceClass::Idle);

        assert_eq!(LaneState::Idle.surface_class(), SurfaceClass::Idle);
        assert_eq!(
            LaneState::Streaming.surface_class(),
            SurfaceClass::Streaming
        );
        assert_eq!(
            LaneState::AwaitingPermission.surface_class(),
            SurfaceClass::Waiting
        );
        assert_eq!(LaneState::Blocked.surface_class(), SurfaceClass::Blocked);
        assert_eq!(LaneState::Exited.surface_class(), SurfaceClass::Empty);
    }

    #[test]
    fn worktree_fold_picks_worst_ranked_state_and_maps_indicators() {
        assert_eq!(SurfaceClass::fold([]), SurfaceClass::Empty);
        assert_eq!(
            SurfaceClass::fold([SurfaceClass::Idle, SurfaceClass::Active]),
            SurfaceClass::Idle
        );
        assert_eq!(
            SurfaceClass::fold([
                SurfaceClass::Streaming,
                SurfaceClass::Idle,
                SurfaceClass::Waiting,
            ]),
            SurfaceClass::Waiting
        );
        assert_eq!(
            SurfaceClass::fold([SurfaceClass::Waiting, SurfaceClass::Blocked]),
            SurfaceClass::Blocked
        );
        for class in SurfaceClass::ALL {
            let encoded = serde_json::to_string(&class).expect("surface class serializes");
            assert_eq!(encoded, format!("\"{}\"", class.word()));
            let indicator = serde_json::to_string(&class.indicator()).expect("indicator");
            assert_eq!(indicator, format!("\"{}\"", class.indicator().word()));
        }
    }
}
