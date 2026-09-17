//! 드리머 결과 — 마지막 패스가 무엇을 승격했는지, 두 자리에서 읽는 한 줄.
//!
//! 드리머는 세션이 끝날 때 조용히 돌고([`crate::session`] 의 세션 종료 훅)
//! 로그 한 줄만 남겼다. 사람 눈에는 아무 일도 일어나지 않은 것과 같아서,
//! 이 크레이트가 내세우는 "긴 세션은 기억을 남긴다"가 화면에서는 증거 없는
//! 주장이었다.
//!
//! 여기는 그 증거를 **프로세스 안에** 붙잡아 두는 자리다. 판단은 하지 않는다:
//! 문안은 엔진의 [`DreamReport::summary_line`] 이 이미 가지고 있고
//! (승격 몇 · 이미 최신 몇 · 건너뛴 몇), 아무것도 승격되지 않았다는 판단도
//! 엔진의 [`DreamReport::is_noop`] 이 든다. 이 모듈이 더하는 것은 **언제**와
//! **무엇을**(승격된 슬러그) 뿐이다.
//!
//! 기록은 세션 종료 훅의 작업 스레드 안에서 일어난다. 종료 예산(2초)을 넘긴
//! 패스는 호출자에게 `None` 을 돌려주고 분리된 스레드에서 마저 도는데, 그
//! 늦은 결과도 여기에는 남아야 한다 — `/resume` 으로 세션을 갈아 끼운 뒤의
//! `/status` 가 그것을 읽는 실제 경로다.

use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use runtime::memory::{DreamReport, WriteOutcome};

/// `/status` 한 줄에 이름을 늘어놓을 최대 개수. 넘치면 몇 개가 더 있는지
/// **말한다** — 조용히 자르면 "이게 전부"로 읽힌다.
const MAX_NAMED_SLUGS: usize = 3;

/// 마지막으로 끝난 드리머 패스.
struct LastPass {
    /// 유닉스 밀리초. [`crate::tui::sessions::relative_time`] 가 세션 피커의
    /// 날짜 열에 쓰는 것과 같은 눈금이라 화면에서 두 시간 표기가 갈리지 않는다.
    at_millis: u128,
    /// 엔진의 요약 문안 그대로.
    summary: String,
    /// 실제로 **쓰인** 항목의 슬러그. 이미 최신이라 손대지 않은 항목
    /// ([`WriteOutcome::Unchanged`])은 빠진다 — 일어나지 않은 변화를 이름으로
    /// 세면 없던 소란을 보고하는 셈이다.
    promoted: Vec<String>,
}

static LAST_PASS: Mutex<Option<LastPass>> = Mutex::new(None);

/// 끝난 패스 하나를 기록한다. 세션 종료 훅이 부른다.
pub(crate) fn record(report: &DreamReport) {
    let pass = LastPass {
        at_millis: now_millis(),
        summary: report.summary_line(),
        promoted: report
            .applied
            .iter()
            .filter(|applied| applied.outcome != WriteOutcome::Unchanged)
            .map(|applied| applied.slug.clone())
            .collect(),
    };
    *lock() = Some(pass);
}

/// `/status` 카드의 드리머 줄 — 언제 돌았고 무엇이 승격됐는지.
///
/// 문안이 스스로를 밝히므로(`dreamer: …`) 카드가 라벨을 따로 붙일 필요가 없다.
/// 이 프로세스에서 패스가 한 번도 끝나지 않았으면 `None` 이고, 그때 카드는 줄을
/// 접는다 — "아직 모른다"를 "아무것도 없었다"로 그리지 않기 위해서다.
#[must_use]
pub fn last_pass_line() -> Option<String> {
    let pass = lock();
    let pass = pass.as_ref()?;
    let when = crate::tui::sessions::relative_time(now_millis(), pass.at_millis);
    match named_slugs(&pass.promoted) {
        Some(names) => Some(format!("{} ({when}): {names}", pass.summary)),
        None => Some(format!("{} ({when})", pass.summary)),
    }
}

/// 세션 종료 요약에 덧붙일 줄 — 승격된 것이 없으면 `None`.
///
/// 종료 요약은 사람이 창을 닫는 순간에 지나가는 두 줄이라 자리값이 비싸다.
/// 아무것도 승격되지 않은 패스는 할 말이 없으므로 찍지 않는다(엔진의
/// [`DreamReport::is_noop`] 과 같은 판단: 쓰인 것이 하나도 없다).
#[must_use]
pub fn session_end_line() -> Option<String> {
    let pass = lock();
    let pass = pass.as_ref()?;
    if pass.promoted.is_empty() {
        return None;
    }
    Some(pass.summary.clone())
}

/// 이름 목록 — 없으면 `None`, 많으면 남은 개수를 밝힌다.
fn named_slugs(slugs: &[String]) -> Option<String> {
    let (named, rest) = slugs.split_at(slugs.len().min(MAX_NAMED_SLUGS));
    let named = named.join(", ");
    match (named.is_empty(), rest.len()) {
        (true, _) => None,
        (false, 0) => Some(named),
        (false, more) => Some(format!("{named} +{more} more")),
    }
}

/// 중독된 잠금은 복구한다 — 한 줄짜리 표시 상태 때문에 `/status` 가 죽는 것이
/// 그 상태를 잃는 것보다 나쁘다.
fn lock() -> std::sync::MutexGuard<'static, Option<LastPass>> {
    LAST_PASS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis())
}

#[cfg(test)]
mod tests {
    use super::{last_pass_line, named_slugs, record, session_end_line, LastPass, MAX_NAMED_SLUGS};
    use runtime::memory::{AppliedPromotion, DreamReport, WriteOutcome};

    /// 상태가 프로세스 전역이라 이 모듈의 테스트는 한 줄로 세운다.
    fn test_lock() -> std::sync::MutexGuard<'static, ()> {
        static GUARD: std::sync::Mutex<()> = std::sync::Mutex::new(());
        GUARD
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn report(applied: Vec<(&str, WriteOutcome)>) -> DreamReport {
        DreamReport {
            applied: applied
                .into_iter()
                .map(|(slug, outcome)| AppliedPromotion {
                    slug: slug.to_string(),
                    outcome,
                })
                .collect(),
            // 계획(건너뛴 후보)은 이 모듈이 읽지 않는다 — 문안은 엔진 것이다.
            ..DreamReport::default()
        }
    }

    /// 패스가 한 번도 안 끝났으면 두 자리 모두 조용하다 — "모른다"를 "없었다"로
    /// 그리지 않는다.
    #[test]
    fn nothing_recorded_shows_nothing_anywhere() {
        let _guard = test_lock();
        *super::lock() = None;
        assert!(last_pass_line().is_none());
        assert!(session_end_line().is_none());
    }

    /// 승격이 있으면 카드는 엔진 문안 + 시각 + 이름을, 종료 요약은 엔진 문안만.
    #[test]
    fn a_promoting_pass_reaches_both_readers() {
        let _guard = test_lock();
        record(&report(vec![
            ("gate-order", WriteOutcome::Created),
            ("pty-lane-flush", WriteOutcome::Updated),
        ]));

        let card = last_pass_line().expect("a finished pass has a card line");
        assert!(card.starts_with("dreamer: promoted 2"), "{card}");
        assert!(card.contains("(now)"), "{card}");
        assert!(card.ends_with(": gate-order, pty-lane-flush"), "{card}");
        assert_eq!(session_end_line().as_deref(), Some("dreamer: promoted 2, skipped 0"));
    }

    /// 이미 최신이라 손대지 않은 항목뿐인 패스는 승격이 아니다: 종료 요약은
    /// 침묵하고, 카드는 "돌았지만 새것은 없었다"를 말한다.
    #[test]
    fn a_pass_that_wrote_nothing_stays_out_of_the_exit_summary() {
        let _guard = test_lock();
        record(&report(vec![("gate-order", WriteOutcome::Unchanged)]));

        assert!(session_end_line().is_none());
        let card = last_pass_line().expect("the card still says the pass ran");
        assert!(card.contains("already current"), "{card}");
        assert!(!card.contains(':') || !card.ends_with("gate-order"), "{card}");
    }

    /// 빈 패스(계획은 돌았고 승격은 없음)도 카드에는 남는다 — 드리머가 살아
    /// 있다는 것이 `/status` 가 답해야 할 질문이다.
    #[test]
    fn an_empty_pass_is_still_a_pass_on_the_card() {
        let _guard = test_lock();
        record(&report(Vec::new()));

        assert!(session_end_line().is_none());
        assert_eq!(
            last_pass_line().as_deref(),
            Some("dreamer: promoted 0, skipped 0 (now)")
        );
    }

    /// 이름이 넘치면 몇 개가 더 있는지 말한다 — 조용히 자르면 "이게 전부"다.
    #[test]
    fn an_overflowing_name_list_says_how_many_it_left_out() {
        let names: Vec<String> = (0..MAX_NAMED_SLUGS + 2)
            .map(|index| format!("slug-{index}"))
            .collect();
        assert_eq!(
            named_slugs(&names).as_deref(),
            Some("slug-0, slug-1, slug-2 +2 more")
        );
        assert_eq!(named_slugs(&[]), None);
    }

    /// 오래된 패스는 카드에서 나이를 밝힌다 — 세션 피커의 날짜 열과 같은 눈금.
    #[test]
    fn an_older_pass_carries_its_age() {
        let _guard = test_lock();
        record(&report(vec![("gate-order", WriteOutcome::Created)]));
        if let Some(pass) = super::lock().as_mut() {
            pass.at_millis = pass.at_millis.saturating_sub(3 * 60 * 1_000);
        }
        let card = last_pass_line().expect("card line");
        assert!(card.contains("(3m ago)"), "{card}");
    }

    /// 필드가 쓰이지 않는 채로 남지 않도록 — 기록 구조는 세 값을 다 든다.
    #[test]
    fn a_recorded_pass_keeps_when_what_and_how_many() {
        let _guard = test_lock();
        record(&report(vec![("gate-order", WriteOutcome::Created)]));
        let pass = super::lock();
        let pass: &LastPass = pass.as_ref().expect("recorded");
        assert!(pass.at_millis > 0);
        assert_eq!(pass.promoted, vec!["gate-order".to_string()]);
        assert!(pass.summary.starts_with("dreamer:"));
    }
}
