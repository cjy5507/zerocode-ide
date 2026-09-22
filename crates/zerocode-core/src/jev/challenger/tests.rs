use super::{
    Attempt, CHALLENGED_ROLES, Held, challenges, draws, role_may_be_challenged, within_day_budget,
};
use crate::jev::{CHALLENGER_DAY_SPEND_PERMILLE, CHALLENGER_ONE_IN};

fn eligible(key: &str) -> Attempt<'_> {
    Attempt {
        key,
        role: "coding",
        retry_or_handover: false,
        guarded: false,
    }
}

/// 판정이 이 자리에서 옳은 것을 지키는 역할은 도전받지 않는다.
#[test]
fn the_roles_this_product_relies_on_being_right_are_never_challenged() {
    for held in [
        "verifier",
        "reviewer",
        "judge",
        "synthesizer",
        "writing",
        "design",
        "default",
    ] {
        assert!(
            !role_may_be_challenged(held),
            "{held} must not be challenged"
        );
    }
    for allowed in CHALLENGED_ROLES {
        assert!(role_may_be_challenged(allowed), "{allowed}");
    }
}

/// 같은 시도는 몇 번을 물어도 같은 답을 낸다 — 추첨은 난수가 아니라 지문이다.
#[test]
fn a_draw_is_the_same_answer_however_often_it_is_asked() {
    for key in ["dp-1", "dp-2", "attempt/7", ""] {
        assert_eq!(draws(key), draws(key), "{key}");
    }
}

/// 다섯 중 하나에 가깝게 뽑힌다 — 천 개의 이름으로 재면 15%~25% 사이.
#[test]
fn about_one_attempt_in_five_draws() {
    let drawn = (0..1_000).filter(|n| draws(&format!("dp-{n}"))).count();
    let one_in_five = 1_000 / usize::try_from(CHALLENGER_ONE_IN).expect("small");
    assert!(
        (one_in_five / 2..=one_in_five * 3 / 2).contains(&drawn),
        "1,000 names drew {drawn}, which is not near one in {CHALLENGER_ONE_IN}"
    );
}

/// 하루 예산은 비율이다: 지출이 없는 날은 열려 있고, 한 몫을 넘기면 닫힌다.
#[test]
fn the_day_budget_is_a_share_of_the_day_not_a_fixed_sum() {
    assert!(
        within_day_budget(0, 0),
        "a day that has spent nothing has room"
    );
    let day = 10_000u128;
    let share = day * u128::from(CHALLENGER_DAY_SPEND_PERMILLE) / 1_000;
    assert!(
        within_day_budget(share, day),
        "exactly its share still fits"
    );
    assert!(!within_day_budget(share + 1, day), "one past it does not");
    // 같은 비율이면 큰 날도 작은 날도 같은 답이다.
    assert!(within_day_budget(share * 100, day * 100));
}

/// 거절은 §2의 순서대로, 싼 것부터 — 막힌 역할은 지문 한 번도 쓰지 않는다.
#[test]
fn the_lines_are_asked_in_order_and_each_names_itself() {
    let retry = Attempt {
        retry_or_handover: true,
        ..eligible("dp-retry")
    };
    assert_eq!(challenges(&retry, 0, 0), Err(Held::Retry));

    let guarded = Attempt {
        guarded: true,
        ..eligible("dp-guarded")
    };
    assert_eq!(challenges(&guarded, 0, 0), Err(Held::Guarded));

    let verifier = Attempt {
        role: "verifier",
        ..eligible("dp-verifier")
    };
    assert_eq!(challenges(&verifier, 0, 0), Err(Held::Role));

    // 재시도이면서 막힌 역할이면 먼저 물은 선이 답이다.
    let both = Attempt {
        role: "verifier",
        retry_or_handover: true,
        ..eligible("dp-both")
    };
    assert_eq!(challenges(&both, 0, 0), Err(Held::Retry));
}

/// 뽑힌 시도만 도전하고, 그마저 그날의 몫을 넘기면 멈춘다.
#[test]
fn a_drawn_attempt_challenges_until_the_day_has_spent_its_share() {
    let drawn = (0..1_000)
        .map(|n| format!("dp-{n}"))
        .find(|key| draws(key))
        .expect("some name draws");
    let held = (0..1_000)
        .map(|n| format!("dp-{n}"))
        .find(|key| !draws(key))
        .expect("some name does not");

    assert_eq!(challenges(&eligible(&drawn), 0, 0), Ok(()));
    assert_eq!(challenges(&eligible(&held), 0, 0), Err(Held::NotDrawn));

    let day = 1_000u128;
    let over = day * u128::from(CHALLENGER_DAY_SPEND_PERMILLE) / 1_000 + 1;
    assert_eq!(
        challenges(&eligible(&drawn), over, day),
        Err(Held::DayBudget)
    );
}

/// 원장에 적히는 낱말은 닫혀 있고 서로 다르다.
#[test]
fn every_holding_writes_a_word_of_its_own() {
    let words = [
        Held::Role.token(),
        Held::Retry.token(),
        Held::Guarded.token(),
        Held::NotDrawn.token(),
        Held::DayBudget.token(),
    ];
    let mut sorted = words;
    sorted.sort_unstable();
    let mut deduped = sorted;
    let unique = {
        let slice: &mut [&str] = &mut deduped;
        slice.sort_unstable();
        let mut seen = Vec::with_capacity(slice.len());
        for word in slice.iter() {
            if !seen.contains(word) {
                seen.push(*word);
            }
        }
        seen.len()
    };
    assert_eq!(unique, words.len(), "two holdings share a word: {words:?}");
}
