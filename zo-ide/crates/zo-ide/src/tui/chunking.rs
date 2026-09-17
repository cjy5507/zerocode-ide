//! 스트림 커밋 틱의 적응 케이던스 — codex `tui/src/streaming/chunking.rs` 포팅.
//!
//! 원문 머리말: "In `ChunkingMode::Smooth`, one queued line is drained per
//! baseline commit tick. When queue pressure rises, it switches to
//! `ChunkingMode::CatchUp` and drains queued backlog immediately so display
//! lag converges as quickly as possible."
//!
//! 두 단 기어다. 임계는 히스테리시스를 쓴다 — 들어갈 때는 높은 압력, 나올 때는
//! 낮은 압력을 `EXIT_HOLD` 동안 유지해야 하고, 나온 뒤에는
//! `REENTER_CATCH_UP_HOLD` 동안 재진입을 막는다(백로그가 심하면 예외).
//! 상수는 원본 값 그대로다.

use std::time::{Duration, Instant};

/// 이 깊이만 넘어도 catch-up 으로 간다.
const ENTER_QUEUE_DEPTH_LINES: usize = 8;
/// 가장 오래 기다린 줄이 이만큼 묵으면 catch-up 으로 간다.
const ENTER_OLDEST_AGE: Duration = Duration::from_millis(120);
/// catch-up 을 나오기 위해 깊이가 내려와야 하는 값.
const EXIT_QUEUE_DEPTH_LINES: usize = 2;
/// catch-up 을 나오기 위해 나이가 내려와야 하는 값.
const EXIT_OLDEST_AGE: Duration = Duration::from_millis(40);
/// 위 두 조건을 이만큼 유지해야 Smooth 로 돌아간다.
const EXIT_HOLD: Duration = Duration::from_millis(250);
/// 방금 나온 뒤 재진입을 막는 창.
const REENTER_CATCH_UP_HOLD: Duration = Duration::from_millis(250);
/// 재진입 창을 무시할 만큼 심한 백로그 — 깊이 기준.
const SEVERE_QUEUE_DEPTH_LINES: usize = 64;
/// 같은 것의 나이 기준.
const SEVERE_OLDEST_AGE: Duration = Duration::from_millis(300);

/// 커밋 애니메이션 한 틱의 간격 — codex `app.rs::COMMIT_ANIMATION_TICK`
/// (= `tui::TARGET_FRAME_INTERVAL` = `Duration::from_nanos(8_333_334)`).
/// Smooth 는 틱마다 한 줄을 내보내므로 이 값이 체감 타이핑 속도를 정한다.
pub const COMMIT_TICK: Duration = Duration::from_nanos(8_333_334);

/// 지금 기어.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    /// 틱마다 한 줄.
    #[default]
    Smooth,
    /// 큐에 쌓인 만큼 한 번에.
    CatchUp,
}

/// 결정에 쓰는 큐 압력.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    /// 아직 화면에 못 나간 줄 수.
    pub queued_lines: usize,
    /// 가장 오래 기다린 줄의 나이.
    pub oldest_age: Option<Duration>,
}

/// 이 틱에 내보낼 줄 수의 계획.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DrainPlan {
    /// 정확히 한 줄.
    Single,
    /// 최대 `usize` 줄.
    Batch(usize),
}

impl DrainPlan {
    /// 계획을 "이번 틱에 최대 몇 줄" 로 편다.
    #[must_use]
    pub const fn rows(self) -> usize {
        match self {
            Self::Single => 1,
            Self::Batch(count) => count,
        }
    }
}

/// 기어와 히스테리시스 상태.
#[derive(Debug, Default)]
pub struct Policy {
    mode: Mode,
    below_exit_threshold_since: Option<Instant>,
    last_catch_up_exit_at: Option<Instant>,
}

impl Policy {
    /// 마지막 결정의 기어.
    #[must_use]
    pub const fn mode(&self) -> Mode {
        self.mode
    }

    /// 스트림이 끝났을 때 기본으로 되돌린다.
    pub fn reset(&mut self) {
        self.mode = Mode::Smooth;
        self.below_exit_threshold_since = None;
        self.last_catch_up_exit_at = None;
    }

    /// 이 스냅샷에 대한 배출 계획.
    pub fn decide(&mut self, snapshot: Snapshot, now: Instant) -> DrainPlan {
        if snapshot.queued_lines == 0 {
            if self.mode == Mode::CatchUp {
                self.last_catch_up_exit_at = Some(now);
            }
            self.mode = Mode::Smooth;
            self.below_exit_threshold_since = None;
            return DrainPlan::Single;
        }
        match self.mode {
            Mode::Smooth => self.maybe_enter_catch_up(snapshot, now),
            Mode::CatchUp => self.maybe_exit_catch_up(snapshot, now),
        }
        match self.mode {
            Mode::Smooth => DrainPlan::Single,
            Mode::CatchUp => DrainPlan::Batch(snapshot.queued_lines.max(1)),
        }
    }

    fn maybe_enter_catch_up(&mut self, snapshot: Snapshot, now: Instant) {
        if !should_enter_catch_up(snapshot) {
            return;
        }
        if self.reentry_hold_active(now) && !is_severe_backlog(snapshot) {
            return;
        }
        self.mode = Mode::CatchUp;
        self.below_exit_threshold_since = None;
        self.last_catch_up_exit_at = None;
    }

    fn maybe_exit_catch_up(&mut self, snapshot: Snapshot, now: Instant) {
        if !should_exit_catch_up(snapshot) {
            self.below_exit_threshold_since = None;
            return;
        }
        match self.below_exit_threshold_since {
            Some(since) if now.saturating_duration_since(since) >= EXIT_HOLD => {
                self.mode = Mode::Smooth;
                self.below_exit_threshold_since = None;
                self.last_catch_up_exit_at = Some(now);
            }
            Some(_) => {}
            None => self.below_exit_threshold_since = Some(now),
        }
    }

    fn reentry_hold_active(&self, now: Instant) -> bool {
        self.last_catch_up_exit_at
            .is_some_and(|exit| now.saturating_duration_since(exit) < REENTER_CATCH_UP_HOLD)
    }
}

/// 깊이든 나이든 하나만 넘어도 catch-up 으로 간다.
fn should_enter_catch_up(snapshot: Snapshot) -> bool {
    snapshot.queued_lines >= ENTER_QUEUE_DEPTH_LINES
        || snapshot
            .oldest_age
            .is_some_and(|oldest| oldest >= ENTER_OLDEST_AGE)
}

/// 나올 때는 둘 다 내려와야 한다 — 한쪽만 보면 경계에서 기어가 떨린다.
fn should_exit_catch_up(snapshot: Snapshot) -> bool {
    snapshot.queued_lines <= EXIT_QUEUE_DEPTH_LINES
        && snapshot
            .oldest_age
            .is_some_and(|oldest| oldest <= EXIT_OLDEST_AGE)
}

/// 재진입 창을 무시할 만큼 심한 백로그인가.
fn is_severe_backlog(snapshot: Snapshot) -> bool {
    snapshot.queued_lines >= SEVERE_QUEUE_DEPTH_LINES
        || snapshot
            .oldest_age
            .is_some_and(|oldest| oldest >= SEVERE_OLDEST_AGE)
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::{DrainPlan, Mode, Policy, Snapshot};

    fn snapshot(queued: usize, age_ms: u64) -> Snapshot {
        Snapshot {
            queued_lines: queued,
            oldest_age: Some(Duration::from_millis(age_ms)),
        }
    }

    #[test]
    fn an_empty_queue_stays_smooth() {
        let mut policy = Policy::default();
        let plan = policy.decide(Snapshot::default(), Instant::now());
        assert_eq!(plan, DrainPlan::Single);
        assert_eq!(policy.mode(), Mode::Smooth);
    }

    #[test]
    fn smooth_drains_one_line_per_tick() {
        let mut policy = Policy::default();
        assert_eq!(policy.decide(snapshot(3, 10), Instant::now()), DrainPlan::Single);
    }

    #[test]
    fn queue_depth_alone_enters_catch_up() {
        let mut policy = Policy::default();
        let plan = policy.decide(snapshot(8, 0), Instant::now());
        assert_eq!(plan, DrainPlan::Batch(8));
        assert_eq!(policy.mode(), Mode::CatchUp);
    }

    #[test]
    fn queue_age_alone_enters_catch_up() {
        let mut policy = Policy::default();
        let plan = policy.decide(snapshot(1, 120), Instant::now());
        assert_eq!(plan, DrainPlan::Batch(1));
    }

    /// 나오려면 낮은 압력을 `EXIT_HOLD`(250ms) 동안 유지해야 한다.
    #[test]
    fn leaving_catch_up_waits_out_the_exit_hold() {
        let mut policy = Policy::default();
        let start = Instant::now();
        policy.decide(snapshot(64, 400), start);
        assert_eq!(policy.mode(), Mode::CatchUp);
        policy.decide(snapshot(1, 5), start + Duration::from_millis(10));
        assert_eq!(policy.mode(), Mode::CatchUp, "the hold has not elapsed yet");
        policy.decide(snapshot(1, 5), start + Duration::from_millis(400));
        assert_eq!(policy.mode(), Mode::Smooth);
    }

    /// 압력이 다시 오르면 유지 시계가 처음으로 돌아간다.
    #[test]
    fn pressure_returning_restarts_the_exit_hold() {
        let mut policy = Policy::default();
        let start = Instant::now();
        policy.decide(snapshot(64, 400), start);
        policy.decide(snapshot(1, 5), start + Duration::from_millis(10));
        policy.decide(snapshot(9, 200), start + Duration::from_millis(20));
        policy.decide(snapshot(1, 5), start + Duration::from_millis(200));
        assert_eq!(policy.mode(), Mode::CatchUp);
    }

    /// 방금 나왔으면 웬만한 압력으로는 다시 못 들어간다 — 심한 백로그는 예외.
    #[test]
    fn the_reentry_hold_only_yields_to_severe_backlog() {
        let mut policy = Policy::default();
        let start = Instant::now();
        policy.decide(snapshot(64, 400), start);
        // 첫 저압 틱은 유지 시계를 켜기만 한다 — 나오는 것은 그 다음 틱이다.
        policy.decide(snapshot(1, 5), start + Duration::from_millis(10));
        assert_eq!(policy.mode(), Mode::CatchUp);
        policy.decide(snapshot(1, 5), start + Duration::from_millis(300));
        assert_eq!(policy.mode(), Mode::Smooth);
        policy.decide(snapshot(8, 0), start + Duration::from_millis(320));
        assert_eq!(policy.mode(), Mode::Smooth, "re-entry hold is active");
        policy.decide(snapshot(64, 0), start + Duration::from_millis(330));
        assert_eq!(policy.mode(), Mode::CatchUp, "severe backlog bypasses the hold");
    }
}
