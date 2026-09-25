use serde_json::{Value, json};

use super::spend::{self, Book, Op};
use super::{
    ATTEMPT, Attempt, BLIND, Blind, CHALLENGED_ROLES, CHALLENGER_DESIGN, CHALLENGER_KEYS,
    CHALLENGER_MODEL, COST_MICROS, Comparison, DaySpend, Designs, EXPECTED_MICROS, HELD, Held,
    INCUMBENT_DESIGN, INCUMBENT_MODEL, OPTIONS, PREFERRED, Preferred, ROLE, Receipt, Receipted,
    STRUCK, STRUCK_PRINT, Side, Standing, VERIFIED, VERIFIED_AT, VERIFIED_SOURCE, WON, ask,
    challenges, draws, eligible as eligible_line, expected_micros, held_row, label_print,
    label_row, label_verdict_at, quality, role_may_be_challenged, standing, strike_row,
    struck_prints, within_day_budget,
};
use crate::jev::choice::ChoiceRefusal;
use crate::jev::promote::{Verdict, judge_seat};
use crate::jev::summary::{AGREED, AT, LABEL, LEDGER_KEYS, asked_something};
use crate::jev::{
    A_WINDOW_OF_COMPARISONS, CHALLENGER, CHALLENGER_DAY_SPEND_PERMILLE, CHALLENGER_DESIGN_CAP,
    CHALLENGER_ONE_IN,
};

/// A receipt on `source`, its verdict recorded at second 3.
fn on(receipt: Receipt, source: &str) -> Receipted {
    Receipted {
        receipt,
        source: source.to_string(),
        verdict_at: 3,
    }
}

fn eligible(key: &str) -> Attempt<'_> {
    Attempt {
        key,
        role: "coding",
        retry_or_handover: false,
        guarded: false,
        pinned: false,
    }
}

/// 이름 천 개 — 추첨·눈가림이 비율대로 나오는지 재는 표본.
fn thousand_names() -> Vec<String> {
    (0..1_000).map(|n| format!("dp-{n}")).collect()
}

fn a_drawn_name() -> String {
    thousand_names()
        .into_iter()
        .find(|key| draws(key))
        .expect("some name draws")
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

/// 같은 시도는 몇 번을 물어도 같은 답을 낸다 — 추첨도 눈가림도 난수가 아니라 지문이다.
#[test]
fn a_draw_and_a_blind_are_the_same_answer_however_often_they_are_asked() {
    for key in ["dp-1", "dp-2", "attempt/7", ""] {
        assert_eq!(draws(key), draws(key), "{key}");
        assert_eq!(Blind::over(key), Blind::over(key), "{key}");
    }
}

/// 다섯 중 하나에 가깝게 뽑힌다 — 천 개의 이름으로 재면 15%~25% 사이.
#[test]
fn about_one_attempt_in_five_draws() {
    let drawn = thousand_names().iter().filter(|key| draws(key)).count();
    let one_in_five = 1_000 / usize::try_from(CHALLENGER_ONE_IN).expect("small");
    assert!(
        (one_in_five / 2..=one_in_five * 3 / 2).contains(&drawn),
        "1,000 names drew {drawn}, which is not near one in {CHALLENGER_ONE_IN}"
    );
}

/// 눈가림은 반반이고 추첨과 무관하다 — 뽑힌 시도만 놓고 봐도 두 순서가 다 나온다.
#[test]
fn the_blind_puts_each_side_first_half_the_time_and_independently_of_the_draw() {
    let names = thousand_names();
    let challenger_first = names
        .iter()
        .filter(|key| Blind::over(key).first() == Side::Challenger)
        .count();
    assert!(
        (400..=600).contains(&challenger_first),
        "the challenger was first {challenger_first} times in 1,000"
    );
    let drawn: Vec<&String> = names.iter().filter(|key| draws(key)).collect();
    let among_drawn = drawn
        .iter()
        .filter(|key| Blind::over(key).first() == Side::Challenger)
        .count();
    assert!(
        among_drawn > 0 && among_drawn < drawn.len(),
        "{among_drawn} of {} drawn attempts showed the challenger first",
        drawn.len()
    );
    for key in &names {
        let blind = Blind::over(key);
        assert_ne!(blind.first(), blind.second(), "{key}");
    }
}

/// 하루 몫은 비율이고, 예약과 이번 시도의 예상 비용까지 센다 — 지출 없는 날은 몫도 없다.
#[test]
fn the_day_budget_counts_what_is_reserved_and_what_this_attempt_would_cost() {
    let empty = DaySpend::default();
    assert!(
        !within_day_budget(&empty, 1),
        "a day that has spent nothing has no share to spend"
    );
    assert!(within_day_budget(&empty, 0), "and nothing costs nothing");

    let day = 10_000u64;
    let share = day * u64::from(CHALLENGER_DAY_SPEND_PERMILLE) / 1_000;
    let spent = |challenger_micros, reserved_micros| DaySpend {
        challenger_micros,
        reserved_micros,
        day_micros: day,
    };
    assert!(
        within_day_budget(&spent(0, 0), share),
        "exactly its share still fits"
    );
    assert!(
        !within_day_budget(&spent(0, 0), share + 1),
        "one past it does not"
    );
    assert!(
        !within_day_budget(&spent(share, 0), 1),
        "what the rows say was spent counts"
    );
    assert!(
        !within_day_budget(&spent(0, share), 1),
        "what is reserved for attempts still running counts"
    );
    assert!(
        within_day_budget(&spent(share / 2, share / 4), share / 4),
        "spent, reserved and expected share the one line"
    );
    // 같은 비율이면 큰 날도 작은 날도 같은 답이다.
    let hundredfold = DaySpend {
        challenger_micros: 0,
        reserved_micros: 0,
        day_micros: day * 100,
    };
    assert!(within_day_budget(&hundredfold, share * 100));
    assert!(!within_day_budget(&hundredfold, share * 100 + 1));
    // 자릿수가 커도 넘치지 않는다.
    let vast = DaySpend {
        challenger_micros: u64::MAX / 4,
        reserved_micros: u64::MAX / 4,
        day_micros: u64::MAX,
    };
    assert!(!within_day_budget(&vast, u64::MAX / 4));
}

/// 거절은 §2의 순서대로, 싼 것부터 — 막힌 역할은 지문 한 번도 쓰지 않는다.
#[test]
fn the_lines_are_asked_in_order_and_each_names_itself() {
    let day = DaySpend::default();
    let retry = Attempt {
        retry_or_handover: true,
        ..eligible("dp-retry")
    };
    assert_eq!(challenges(&retry, &day, 0), Err(Held::Retry));

    let guarded = Attempt {
        guarded: true,
        ..eligible("dp-guarded")
    };
    assert_eq!(challenges(&guarded, &day, 0), Err(Held::Guarded));

    // 사람이 모델을 고른 시도는 라우터의 선택이 아니다 — 가드 뒤, 역할 앞.
    let pinned = Attempt {
        pinned: true,
        ..eligible("dp-pinned")
    };
    assert_eq!(challenges(&pinned, &day, 0), Err(Held::Pinned));
    assert_eq!(eligible_line(&pinned), Err(Held::Pinned));
    let pinned_guarded = Attempt {
        guarded: true,
        pinned: true,
        ..eligible("dp-pinned-guarded")
    };
    assert_eq!(eligible_line(&pinned_guarded), Err(Held::Guarded));

    let verifier = Attempt {
        role: "verifier",
        ..eligible("dp-verifier")
    };
    assert_eq!(challenges(&verifier, &day, 0), Err(Held::Role));

    // 재시도이면서 막힌 역할이면 먼저 물은 선이 답이다.
    let both = Attempt {
        role: "verifier",
        retry_or_handover: true,
        ..eligible("dp-both")
    };
    assert_eq!(challenges(&both, &day, 0), Err(Held::Retry));
}

/// 뽑힌 시도만 도전하고, 그마저 그날의 몫에 이번 비용이 안 들어가면 멈춘다.
#[test]
fn a_drawn_attempt_challenges_until_the_day_has_spent_its_share() {
    let drawn = a_drawn_name();
    let held = thousand_names()
        .into_iter()
        .find(|key| !draws(key))
        .expect("some name does not draw");

    let day = DaySpend {
        challenger_micros: 0,
        reserved_micros: 0,
        day_micros: 1_000,
    };
    let share = day.day_micros * u64::from(CHALLENGER_DAY_SPEND_PERMILLE) / 1_000;
    assert_eq!(challenges(&eligible(&drawn), &day, share), Ok(()));
    assert_eq!(
        challenges(&eligible(&held), &day, share),
        Err(Held::NotDrawn)
    );
    assert_eq!(
        challenges(&eligible(&drawn), &day, share + 1),
        Err(Held::DayBudget)
    );
    assert_eq!(
        challenges(&eligible(&drawn), &DaySpend::default(), 1),
        Err(Held::DayBudget),
        "the first challenge of a day waits until the day's own work has bought it"
    );
}

/// 원장에 적히는 낱말은 닫혀 있고 서로 다르다.
#[test]
fn every_holding_and_every_side_writes_a_word_of_its_own() {
    let holdings = [
        Held::Role.token(),
        Held::Retry.token(),
        Held::Guarded.token(),
        Held::NotDrawn.token(),
        Held::DayBudget.token(),
        Held::Pinned.token(),
        Held::NoChallenger.token(),
        Held::Unpriced.token(),
        Held::NoDesign.token(),
    ];
    let mut seen: Vec<&str> = holdings.to_vec();
    seen.sort_unstable();
    seen.dedup();
    assert_eq!(seen.len(), holdings.len(), "two holdings share a word");

    let preferred = [
        Preferred::Incumbent,
        Preferred::Challenger,
        Preferred::Neither,
    ];
    for word in preferred {
        assert_eq!(Preferred::from_token(word.token()), Some(word));
    }
    assert_eq!(
        Preferred::from_token("first"),
        None,
        "a position is not a side"
    );
    assert_ne!(Receipt::Passed.token(), Receipt::Failed.token());
}

/// 판정자는 이름을 보지 않는다 — 상태에는 어느 쪽이 누구인지도, 모델 이름도 없다.
#[test]
fn the_judge_is_shown_two_designs_under_no_name_in_the_blinds_order() {
    let key = "dp-blind";
    let designs = Designs {
        incumbent: "INCUMBENT-PLAN: cache the index",
        challenger: "CHALLENGER-PLAN: stream the index",
    };
    let asked = ask(key, "make the index faster", &designs);

    let text = serde_json::to_string(&asked.state).expect("json");
    for name in [
        Side::Incumbent.token(),
        Side::Challenger.token(),
        "claude",
        "gpt",
        asked.blind().token(),
    ] {
        assert!(!text.contains(name), "the state names {name}: {text}");
    }

    let shown = asked.state[super::STATE_KEYS[1]]
        .as_array()
        .expect("designs");
    assert_eq!(shown.len(), CHALLENGER_DESIGN_CAP);
    let body_of = |side: Side| match side {
        Side::Incumbent => designs.incumbent,
        Side::Challenger => designs.challenger,
    };
    let blind = Blind::over(key);
    assert_eq!(blind, asked.blind());
    assert_eq!(
        shown[0][super::DESIGN_BODY_KEY].as_str(),
        Some(body_of(blind.first()))
    );
    assert_eq!(
        shown[1][super::DESIGN_BODY_KEY].as_str(),
        Some(body_of(blind.second()))
    );

    // 자리 행의 sends가 이름한 자리마다 실제로 무엇인가 있다.
    let body = json!({ "state": asked.state, "questions": asked.questions });
    for sent in CHALLENGER.sends {
        let pointer = sent.at.replace("/*", "/0");
        assert!(
            body.pointer(&pointer).is_some(),
            "{} points at nothing in the body",
            sent.at
        );
    }
}

/// 답은 자리 낱말로 오고, 눈가림을 풀어 어느 쪽인지로 읽힌다 — 규칙 하나를 어기면 통째로 버린다.
#[test]
fn an_answer_names_a_position_and_is_read_back_as_a_side_or_refused_whole() {
    let key = "dp-read";
    let asked = ask(
        key,
        "task",
        &Designs {
            incumbent: "a",
            challenger: "b",
        },
    );
    let blind = asked.blind();
    let answer = |chosen: &str| {
        json!({
            "preferred": {
                "type": "choice",
                "choice": chosen,
                "probabilities": { "first": 0.6, "second": 0.3, "neither": 0.1 },
                "confidence": 0.6
            }
        })
    };

    let first = asked.read(&answer(OPTIONS[0])).expect("first");
    let second = asked.read(&answer(OPTIONS[1])).expect("second");
    let neither = asked.read(&answer(OPTIONS[2])).expect("neither");
    let side_of = |preferred: Preferred| match preferred {
        Preferred::Incumbent => Some(Side::Incumbent),
        Preferred::Challenger => Some(Side::Challenger),
        Preferred::Neither => None,
    };
    assert_eq!(side_of(first.preferred), Some(blind.first()));
    assert_eq!(side_of(second.preferred), Some(blind.second()));
    assert_eq!(neither.preferred, Preferred::Neither);
    assert!((first.confidence - 0.6).abs() < f64::EPSILON);

    assert_eq!(
        asked.read(&answer("incumbent")),
        Err(ChoiceRefusal::UnknownOption),
        "a side is not a word the question offered"
    );
    assert_eq!(asked.read(&json!({})), Err(ChoiceRefusal::NoAnswer));
}

/// 품질 라벨: 영수증이 비교를 이기고, 영수증이 말할 수 없는 칸은 비워 둔다. 완료는 영수증이 아니다.
#[test]
fn a_receipt_outranks_the_comparison_and_says_nothing_where_it_cannot() {
    use Preferred::{Challenger, Incumbent, Neither};
    use Receipt::{Failed, Passed};

    // 영수증 없음: 비교만이 도전자의 말이고, 자리의 표식은 없다.
    assert!(quality(None, Challenger).won);
    assert!(!quality(None, Incumbent).won);
    assert!(!quality(None, Neither).won);
    for preferred in [Challenger, Incumbent, Neither] {
        assert_eq!(quality(None, preferred).agreed, None);
    }

    // 현직이 실패: 도전자를 고른 판정은 옳았고 이겼다; 현직을 고른 판정은 틀렸다.
    assert!(quality(Some(Failed), Challenger).won);
    assert_eq!(quality(Some(Failed), Challenger).agreed, Some(true));
    assert!(!quality(Some(Failed), Incumbent).won);
    assert_eq!(quality(Some(Failed), Incumbent).agreed, Some(false));

    // 현직이 통과: 현직을 고른 판정은 옳았다; 도전자를 고른 판정은 판정할 수 없다 —
    // 도전자의 설계는 실행된 적이 없다.
    assert!(!quality(Some(Passed), Incumbent).won);
    assert_eq!(quality(Some(Passed), Incumbent).agreed, Some(true));
    assert!(!quality(Some(Passed), Challenger).won);
    assert_eq!(quality(Some(Passed), Challenger).agreed, None);

    // 둘 다 아니라는 판정은 무엇에도 반박되지 않는다.
    for receipt in [Passed, Failed] {
        assert!(!quality(Some(receipt), Neither).won);
        assert_eq!(quality(Some(receipt), Neither).agreed, None);
    }
}

fn comparison<'a>(attempt: &'a str, preferred: Option<Preferred>) -> Comparison<'a> {
    Comparison {
        attempt,
        role: "coding",
        incumbent_model: "claude-fable-5-1",
        challenger_model: "claude-opus-5-2",
        expected_micros: 12_500,
        cost_micros: 9_870,
        incumbent_design: "0123456789abcdef",
        challenger_design: "fedcba9876543210",
        blind: Blind::over(attempt),
        preferred,
    }
}

/// 요청 행의 열은 표의 철자로 적히고, 선의 열과 겹치지 않는다.
#[test]
fn a_request_row_spells_its_columns_from_the_table_and_none_of_the_wires() {
    let answered = comparison("dp-1", Some(Preferred::Challenger)).columns();
    for key in [
        ATTEMPT,
        ROLE,
        INCUMBENT_MODEL,
        CHALLENGER_MODEL,
        EXPECTED_MICROS,
        COST_MICROS,
        INCUMBENT_DESIGN,
        CHALLENGER_DESIGN,
        BLIND,
        PREFERRED,
        WON,
    ] {
        assert!(
            answered.contains_key(key.canonical),
            "an answered row lacks {}",
            key.canonical
        );
    }
    assert_eq!(answered[WON.canonical], Value::Bool(true));
    assert_eq!(answered[COST_MICROS.canonical], json!(9_870));
    assert_eq!(
        answered[INCUMBENT_DESIGN.canonical],
        json!("0123456789abcdef")
    );
    assert_eq!(
        answered[PREFERRED.canonical],
        Value::from(Preferred::Challenger.token())
    );
    assert_eq!(
        answered[BLIND.canonical],
        Value::from(Blind::over("dp-1").token())
    );

    let unanswered = comparison("dp-2", None).columns();
    assert!(!unanswered.contains_key(PREFERRED.canonical));
    assert!(!unanswered.contains_key(WON.canonical));

    for key in CHALLENGER_KEYS {
        for wire in LEDGER_KEYS {
            for spelling in wire.spellings() {
                assert_ne!(
                    key.canonical, spelling,
                    "a challenger key re-spells the wire's {spelling}"
                );
            }
        }
    }
    let mut spellings: Vec<&str> = CHALLENGER_KEYS.iter().map(|key| key.canonical).collect();
    spellings.sort_unstable();
    spellings.dedup();
    assert_eq!(
        spellings.len(),
        CHALLENGER_KEYS.len(),
        "two keys share a spelling"
    );
}

/// 라벨 행은 요청 행을 시도 이름으로 가리키고, 영수증이 말할 수 있을 때만 합의 표식을 단다.
#[test]
fn a_label_row_names_its_attempt_and_marks_agreement_only_where_the_receipt_can_say() {
    let vindicated = label_row(
        "dp-1",
        &on(Receipt::Failed, "tree-1"),
        Preferred::Challenger,
        7,
    );
    assert_eq!(
        LABEL.read(&vindicated).and_then(Value::as_str),
        Some("dp-1")
    );
    assert_eq!(AT.read(&vindicated).and_then(Value::as_i64), Some(7));
    assert_eq!(
        VERIFIED.read(&vindicated).and_then(Value::as_str),
        Some(Receipt::Failed.token())
    );
    assert_eq!(
        VERIFIED_SOURCE.read(&vindicated).and_then(Value::as_str),
        Some("tree-1"),
        "what was verified stands beside what it said"
    );
    assert_eq!(
        VERIFIED_AT.read(&vindicated).and_then(Value::as_u64),
        Some(3),
        "and when its verdict was recorded"
    );
    assert_eq!(label_verdict_at(&vindicated), Some(3));
    assert_eq!(WON.read(&vindicated).and_then(Value::as_bool), Some(true));
    assert_eq!(
        AGREED.read(&vindicated).and_then(Value::as_bool),
        Some(true)
    );
    assert!(
        asked_something(&vindicated).is_none(),
        "a label row is not a request"
    );

    let undecidable = label_row(
        "dp-2",
        &on(Receipt::Passed, "tree-2"),
        Preferred::Challenger,
        8,
    );
    assert_eq!(WON.read(&undecidable).and_then(Value::as_bool), Some(false));
    assert!(AGREED.read(&undecidable).is_none());
}

/// 기록이 반박한 라벨을 긋는 행은 그 라벨 행 하나만 가리키고 요청·표식·라벨 어느 것으로도 읽히지 않는다(t-6263 R4c-1):
/// 같은 시도에 나중에 쓰인 라벨은 다른 행이라 그어지지 않는다.
#[test]
fn a_strike_names_one_label_row_and_reads_as_nothing_else() {
    let label = label_row(
        "dp-1",
        &on(Receipt::Failed, "tree-1"),
        Preferred::Challenger,
        7,
    );
    let strike = strike_row(&label, 9).expect("a label is struck");
    assert_eq!(STRUCK.read(&strike).and_then(Value::as_str), Some("dp-1"));
    assert_eq!(
        STRUCK_PRINT.read(&strike).and_then(Value::as_str),
        Some(label_print(&label).as_str())
    );
    assert!(
        asked_something(&strike).is_none(),
        "a strike is not a request"
    );
    assert!(
        LABEL.read(&strike).is_none()
            && AGREED.read(&strike).is_none()
            && WON.read(&strike).is_none()
    );
    let later = label_row(
        "dp-1",
        &on(Receipt::Failed, "tree-1"),
        Preferred::Challenger,
        8,
    );
    assert_ne!(
        label_print(&later),
        label_print(&label),
        "a later label of the same attempt is another row"
    );
    let reread: Value = serde_json::from_str(&label.to_string()).expect("json");
    assert_eq!(
        label_print(&reread),
        label_print(&label),
        "and a row read back is the row that was written"
    );
    let rows = vec![label.clone(), strike];
    assert_eq!(
        struck_prints(&rows).into_iter().collect::<Vec<_>>(),
        [label_print(&label).as_str()]
    );
    assert!(
        strike_row(&json!({"at": 1}), 2).is_none(),
        "only a label is struck"
    );
    assert_eq!(
        standing(&rows, "coding", "claude-opus-5-2"),
        Standing {
            compared: 0,
            won: 0
        }
    );
}

/// 붙들린 행은 요청이 아니다 — 창에도 몫에도 들지 않고 이유만 적는다.
#[test]
fn a_held_row_asks_nothing_and_says_why() {
    let row = held_row(&eligible("dp-held"), Held::DayBudget, 9);
    assert!(asked_something(&row).is_none());
    assert_eq!(
        HELD.read(&row).and_then(Value::as_str),
        Some(Held::DayBudget.token())
    );
    assert_eq!(ATTEMPT.read(&row).and_then(Value::as_str), Some("dp-held"));
    assert_eq!(ROLE.read(&row).and_then(Value::as_str), Some("coding"));
}

fn request_row(attempt: &str, at: i64, preferred: Option<Preferred>) -> Value {
    let mut row = serde_json::Map::from_iter([
        ("at".to_string(), json!(at)),
        ("outcome".to_string(), json!("answered")),
        ("elapsedMs".to_string(), json!(120)),
        ("model".to_string(), json!("jev-1.13.0")),
    ]);
    row.extend(comparison(attempt, preferred).columns());
    Value::Object(row)
}

/// (역할, 도전 모델)별 전적은 시도마다 마지막 말 하나로 센다 — 라벨이 비교를 덮고, 남의 짝은 끼지 않는다.
#[test]
fn a_standing_reads_one_latest_word_per_attempt_of_its_own_pair() {
    let mut other = comparison("dp-other", Some(Preferred::Challenger));
    other.role = "fast";
    let mut other_row = serde_json::Map::from_iter([
        ("at".to_string(), json!(1)),
        ("outcome".to_string(), json!("answered")),
    ]);
    other_row.extend(other.columns());

    let rows = vec![
        request_row("dp-1", 1, Some(Preferred::Challenger)),
        request_row("dp-2", 2, Some(Preferred::Challenger)),
        request_row("dp-3", 3, Some(Preferred::Incumbent)),
        request_row("dp-4", 4, None),
        Value::Object(other_row),
        // dp-2의 영수증: 현직이 통과했으니 도전자의 승리는 취소된다.
        label_row(
            "dp-2",
            &on(Receipt::Passed, "tree-2"),
            Preferred::Challenger,
            5,
        ),
        // 모르는 시도의 라벨은 세지 않는다.
        label_row(
            "dp-nobody",
            &on(Receipt::Failed, "tree-x"),
            Preferred::Challenger,
            6,
        ),
    ];
    let record = standing(&rows, "coding", "claude-opus-5-2");
    assert_eq!(
        record,
        Standing {
            compared: 3,
            won: 1
        }
    );
    assert_eq!(
        standing(&rows, "fast", "claude-opus-5-2"),
        Standing {
            compared: 1,
            won: 1
        }
    );
    assert_eq!(standing(&rows, "coding", "gpt-6-luna"), Standing::default());
    assert_eq!(Standing::default().lower_bound(), None);
}

/// source를 적지 않은 라벨(옛 형식, 또는 빈 source)은 판정할 수 없는 라벨이라 전적의 말을 바꾸지 않는다(t-6263 R4c): 그
/// 시도의 말은 비교 자신의 것이고, source를 적은 라벨만 비교를 덮는다.
#[test]
fn a_label_that_names_no_source_leaves_the_comparisons_word_standing() {
    let unsourced = |attempt: &str, at: i64| {
        let mut row = label_row(
            attempt,
            &on(Receipt::Passed, "tree"),
            Preferred::Challenger,
            at,
        );
        row.as_object_mut()
            .expect("a row")
            .remove(VERIFIED_SOURCE.canonical);
        row
    };
    let rows = vec![
        request_row("dp-1", 1, Some(Preferred::Challenger)),
        request_row("dp-2", 2, Some(Preferred::Challenger)),
        request_row("dp-3", 3, Some(Preferred::Challenger)),
        unsourced("dp-1", 4),
        label_row("dp-2", &on(Receipt::Passed, ""), Preferred::Challenger, 5),
        label_row(
            "dp-3",
            &on(Receipt::Passed, "tree-3"),
            Preferred::Challenger,
            6,
        ),
    ];
    assert_eq!(
        standing(&rows, "coding", "claude-opus-5-2"),
        Standing {
            compared: 3,
            won: 2
        },
        "dp-1 and dp-2 keep their comparisons' words; dp-3's label, bound to its source, outranks its own"
    );
}

/// 역할의 모델이 움직이는 선: 한 창만큼의 비교 위에서 Wilson 하한이 현직의 비율을 넘을 때만.
#[test]
fn a_standing_passes_only_on_a_window_of_comparisons_whose_lower_bound_clears_the_incumbent() {
    let thin = Standing {
        compared: A_WINDOW_OF_COMPARISONS - 1,
        won: A_WINDOW_OF_COMPARISONS - 1,
    };
    assert!(
        !thin.passes(0.0),
        "a record thinner than a window passes nothing, however good"
    );

    let perfect = Standing {
        compared: A_WINDOW_OF_COMPARISONS,
        won: A_WINDOW_OF_COMPARISONS,
    };
    let bound = perfect.lower_bound().expect("compared");
    assert!(bound > 0.8 && bound < 1.0, "{bound}");
    assert!(perfect.passes(bound - 0.01));
    assert!(!perfect.passes(bound));
    assert!(!perfect.passes(bound + 0.01));

    let even = Standing {
        compared: A_WINDOW_OF_COMPARISONS * 2,
        won: A_WINDOW_OF_COMPARISONS,
    };
    assert!(
        !even.passes(0.5),
        "a coin never passes an incumbent that wins half the time"
    );
}

/// 이 모듈이 쓴 행을 자리 판정기가 그대로 읽는다 — 영수증이 다는 합의 표식으로 자리가 오른다.
#[test]
fn the_seat_judge_reads_the_rows_this_module_writes_and_raises_the_seat_on_receipts() {
    let requests = 80usize;
    let mut rows: Vec<Value> = (0..requests)
        .map(|n| {
            let attempt = format!("dp-{n}");
            request_row(
                &attempt,
                i64::try_from(n).expect("small"),
                Some(Preferred::Challenger),
            )
        })
        .collect();
    // Enough receipts to bound above the line with the three the label said
    // no to inside — a judge that preferred the incumbent on an attempt the
    // receipt failed (t-6342).
    let labels = 45;
    let misses = crate::jev::NEGATIVES_WANTED;
    let first_labeled = requests - labels;
    for n in first_labeled..requests {
        let preferred = if n - first_labeled < misses {
            Preferred::Incumbent
        } else {
            Preferred::Challenger
        };
        rows.push(label_row(
            &format!("dp-{n}"),
            &on(Receipt::Failed, "tree"),
            preferred,
            i64::try_from(requests + n).expect("small"),
        ));
    }

    let judged = judge_seat(&CHALLENGER, &rows).expect("the seat promotes");
    assert_eq!(judged.agreement.compared, labels);
    assert_eq!(judged.agreement.agreed, labels - misses);
    // The incumbent's design is what every one of those receipts failed.
    assert_eq!(
        (
            judged.agreement.baseline_compared,
            judged.agreement.baseline_agreed
        ),
        (labels, 0)
    );
    assert_eq!(judged.model.as_deref(), Some("jev-1.13.0"));
    assert_eq!(judged.verdict, Verdict::Rise, "{judged:?}");

    assert_eq!(
        standing(&rows, "coding", "claude-opus-5-2"),
        Standing {
            compared: requests,
            won: requests - misses
        }
    );
}

/// 예상 비용은 토큰 × 백만 토큰당 달러가 곧 마이크로달러이고, 올림이다 — 몫은 천장이라 내림은 하루치 소수점을 통과시킨다.
#[test]
fn an_expected_cost_is_tokens_times_the_rate_rounded_up() {
    // 1,000 in at $5/M + 1,024 out at $25/M = 5,000 + 25,600 micro-dollars.
    assert_eq!(expected_micros(1_000, 1_024, 5.0, 25.0), 30_600);
    // 세 토큰에 $0.4/M = 1.2 micro → 2.
    assert_eq!(expected_micros(3, 0, 0.4, 0.0), 2);
    assert_eq!(expected_micros(0, 0, 5.0, 25.0), 0);
    assert_eq!(
        expected_micros(1, 0, 0.0, 0.0),
        0,
        "a zero rate is a zero rate — unknown never reaches here"
    );
    // 달러가 아니라 마이크로달러: $1 = 1,000,000.
    assert_eq!(expected_micros(1_000_000, 0, 1.0, 0.0), 1_000_000);
}

/// 하루 파일의 접기: 시도마다 첫 예약이 서고, 정산은 예약을 끝내며 한 번만 세고, 해제는 아무것도 세지 않는다.
#[test]
fn a_days_book_is_one_word_per_attempt_and_a_settlement_ends_a_reservation() {
    let mut text = String::new();
    text.push_str(&spend::line("dp-a", Op::Reserve, 1_000));
    text.push_str(&spend::line("dp-a", Op::Reserve, 9_999)); // a retry of the writer — not a second charge
    text.push_str(&spend::line("dp-b", Op::Reserve, 2_000));
    text.push_str(&spend::line("dp-b", Op::Settle, 1_500));
    text.push_str(&spend::line("dp-b", Op::Settle, 7_777)); // seen twice, counts once
    text.push_str(&spend::line("dp-c", Op::Reserve, 3_000));
    text.push_str(&spend::line("dp-c", Op::Release, 0));
    text.push_str(&spend::line("dp-d", Op::Settle, 400)); // the reservation line was lost: the design still left
    text.push_str("{\"attempt\":\"dp-torn\",\"op\":\"rese"); // a crash's leftover at the tail
    let book = spend::fold(&text);
    assert_eq!(
        book,
        Book {
            reserved_micros: 1_000,
            spent_micros: 1_900,
            reserved: 1,
            settled: 2,
        }
    );

    // 해제 뒤 정산: 뒤늦게 온 비용은 실제 지출이다.
    let late = format!(
        "{}{}",
        spend::line("dp-e", Op::Release, 0),
        spend::line("dp-e", Op::Settle, 50)
    );
    assert_eq!(spend::fold(&late).spent_micros, 50);
    // 정산 뒤 해제: 이미 낸 돈은 되돌아오지 않는다.
    let undone = format!(
        "{}{}",
        spend::line("dp-f", Op::Settle, 60),
        spend::line("dp-f", Op::Release, 0)
    );
    assert_eq!(spend::fold(&undone).spent_micros, 60);
    assert_eq!(spend::fold("").reserved_micros, 0);
    // 빈 줄·다른 모양의 줄은 건너뛴다.
    assert_eq!(spend::fold("\n{\"x\":1}\n").spent_micros, 0);
}

/// 하루 몫의 분모에 도전 지출은 한 번 든다 — 호출자가 더한 나머지와 책의 정산을 여기서 합친다.
#[test]
fn the_days_whole_counts_the_arms_own_spend_exactly_once() {
    let book = Book {
        reserved_micros: 300,
        spent_micros: 700,
        reserved: 1,
        settled: 2,
    };
    let day = spend::day_spend(&book, 9_000);
    assert_eq!(day.challenger_micros, 700);
    assert_eq!(day.reserved_micros, 300);
    assert_eq!(day.day_micros, 9_700);
    // 실효 상한: 나머지 9,000의 10분의 1이 아니라 전체 9,700의 10분의 1 = 970 — 이미 1,000이 잡혀 있으니 한 푼도 더 못 쓴다.
    assert!(!within_day_budget(&day, 1));
    let quieter = spend::day_spend(&Book::default(), 9_000);
    assert!(within_day_budget(&quieter, 900));
    assert!(!within_day_budget(&quieter, 901));
}

/// 하루 파일의 줄과 이름은 한 곳의 철자다.
#[test]
fn a_spend_line_and_a_spend_path_are_spelled_from_this_module() {
    let line = spend::line("dp-1", Op::Reserve, 42);
    assert!(line.ends_with('\n'));
    let event: Value = serde_json::from_str(line.trim_end()).expect("one json line");
    assert_eq!(event[spend::ATTEMPT_KEY], json!("dp-1"));
    assert_eq!(event[spend::OP_KEY], json!(Op::Reserve.token()));
    assert_eq!(event[spend::MICROS_KEY], json!(42));
    let release: Value =
        serde_json::from_str(spend::line("dp-1", Op::Release, 42).trim_end()).expect("json");
    assert!(
        release.get(spend::MICROS_KEY).is_none(),
        "a release carries no amount"
    );
    let path = spend::spend_path(std::path::Path::new("/home/x/.zo"), "2026-09-24");
    assert_eq!(
        path,
        std::path::PathBuf::from("/home/x/.zo")
            .join(crate::jev::count::REQUESTS_DIR)
            .join("challenger-spend-2026-09-24.jsonl")
    );
}

/// 한 시도는 하루에 한 번만 잡힌다: 이미 적힌 시도는 이름으로 알아본다; 첫 예약은 지난날 파일을 치운다.
#[test]
fn an_attempt_is_named_once_a_day_and_the_first_reservation_forgets_past_days() {
    let text = spend::line("dp-a", Op::Reserve, 1);
    assert!(spend::names(&text, "dp-a"));
    assert!(!spend::names(&text, "dp-b"));
    assert!(!spend::names("{torn", "dp-a"));

    let home = tempfile::tempdir().expect("a home");
    let book = |day: &str| spend::spend_path(home.path(), day);
    let write = |day: &str, text: String| std::fs::write(book(day), text).expect("a book");
    std::fs::create_dir_all(book("x").parent().expect("a folder")).expect("folder");
    let settled = spend::line("dp-old", Op::Reserve, 9) + &spend::line("dp-old", Op::Settle, 7);
    write("2026-09-23", settled);
    write("2026-09-24", spend::line("dp-new", Op::Reserve, 1));
    write("2026-09-25", spend::line("dp-next", Op::Reserve, 1));
    let count = crate::jev::count::requests_path(home.path(), "2026-09-23");
    std::fs::write(&count, "...").expect("the door's own count");
    spend::forget_settled_days(&book("2026-09-24"));
    assert!(
        !book("2026-09-23").exists(),
        "yesterday's settled book is forgotten"
    );
    assert!(book("2026-09-24").exists(), "today's is kept");
    assert!(
        book("2026-09-25").exists(),
        "a later day's book is never an earlier day's to forget"
    );
    assert!(
        count.exists(),
        "the door's count is not the book's to forget"
    );

    // A reservation charged before midnight and still out keeps yesterday's
    // book for its settlement; two days back, a live reservation is a draw
    // that died, and its day is over.
    write("2026-09-24", spend::line("dp-late", Op::Reserve, 5));
    write("2026-09-23", spend::line("dp-dead", Op::Reserve, 5));
    spend::forget_settled_days(&book("2026-09-25"));
    assert!(
        book("2026-09-24").exists(),
        "yesterday is kept while it can still be settled into"
    );
    assert!(
        !book("2026-09-23").exists(),
        "two days back is forgotten, live or not"
    );
    write(
        "2026-09-24",
        spend::line("dp-late", Op::Reserve, 5) + &spend::line("dp-late", Op::Settle, 4),
    );
    spend::forget_settled_days(&book("2026-09-25"));
    assert!(!book("2026-09-24").exists(), "settled, it goes");
}

/// 날짜 경계의 늦은 쓰기: 자정 전에 날을 정한 프로그램이 자정 뒤에 옛날 파일에 처음 쓰더라도, 이미 시작된 새날의 파일은
/// 지우지 않는다 — 문의 셈과 도전 장부가 같은 규칙 하나로.
#[test]
fn a_late_write_into_an_ended_day_never_forgets_the_day_that_began() {
    let home = tempfile::tempdir().expect("a home");
    let today = crate::jev::count::requests_path(home.path(), "2026-09-25");
    let ended = crate::jev::count::requests_path(home.path(), "2026-09-24");
    assert_eq!(
        crate::jev::count::count_one(&today).expect("today's first"),
        1
    );
    assert_eq!(crate::jev::count::count_one(&ended).expect("a late one"), 1);
    assert_eq!(
        crate::jev::count::sent(&today),
        1,
        "the day that began keeps its count"
    );
    assert_eq!(
        crate::jev::count::count_one(&today).expect("today's second"),
        2
    );

    let book = |day: &str| spend::spend_path(home.path(), day);
    std::fs::write(book("2026-09-25"), spend::line("dp-new", Op::Reserve, 3)).expect("today");
    std::fs::write(book("2026-09-24"), spend::line("dp-late", Op::Reserve, 5)).expect("late");
    spend::forget_settled_days(&book("2026-09-24"));
    assert_eq!(
        spend::fold(&std::fs::read_to_string(book("2026-09-25")).expect("today's book"))
            .reserved_micros,
        3,
        "a late first reservation of an ended day leaves the new day's book as it was"
    );

    assert_eq!(
        crate::jev::count::day_before("2026-03-01").as_deref(),
        Some("2026-02-28")
    );
    assert_eq!(
        crate::jev::count::day_before("2028-03-01").as_deref(),
        Some("2028-02-29")
    );
    assert_eq!(
        crate::jev::count::day_before("2026-01-01").as_deref(),
        Some("2025-12-31")
    );
    assert_eq!(crate::jev::count::day_before("not a day"), None);
    let odd = home
        .path()
        .join(crate::jev::count::REQUESTS_DIR)
        .join("challenger-spend-latest.jsonl");
    std::fs::write(&odd, "").expect("a name that is not a day");
    spend::forget_settled_days(&book("2026-09-26"));
    assert!(
        odd.exists(),
        "a name this cannot place is not its to forget"
    );
}

/* ---- the replay: a ledger re-read through the product's own functions ---- */

/// What one pair's rows add up to.
#[derive(Debug, Default, PartialEq)]
struct PairReplay {
    standing: Standing,
    expected_micros: u64,
    cost_micros: u64,
}

/// A ledger's rows, re-read the way the arm reads them: the draw and the
/// blind re-derived from each attempt's key and held against what the row
/// says, one word per attempt for each (role, challenger) pair
/// ([`standing`]), what the share was charged and what the designs cost,
/// and why the held attempts were held.
#[derive(Debug, Default, PartialEq)]
struct Replayed {
    requests: usize,
    pairs: std::collections::BTreeMap<(String, String), PairReplay>,
    held: std::collections::BTreeMap<String, usize>,
    undrawn: Vec<String>,
    blind_mismatches: Vec<String>,
}

fn replay(rows: &[Value]) -> Replayed {
    let mut replayed = Replayed::default();
    let text = |row: &Value, key: &crate::jev::summary::LedgerKey| {
        key.read(row).and_then(Value::as_str).map(str::to_string)
    };
    for row in rows {
        let Some(attempt) = text(row, &ATTEMPT) else {
            continue;
        };
        if !draws(&attempt) {
            replayed.undrawn.push(attempt.clone());
        }
        if let Some(word) = text(row, &HELD) {
            *replayed.held.entry(word).or_default() += 1;
            continue;
        }
        if asked_something(row).is_none() {
            continue;
        }
        replayed.requests += 1;
        if let Some(blind) = text(row, &BLIND)
            && blind != Blind::over(&attempt).token()
        {
            replayed.blind_mismatches.push(attempt.clone());
        }
        let (Some(role), Some(challenger)) = (text(row, &ROLE), text(row, &CHALLENGER_MODEL))
        else {
            continue;
        };
        let pair = replayed.pairs.entry((role, challenger)).or_default();
        pair.expected_micros += EXPECTED_MICROS
            .read(row)
            .and_then(Value::as_u64)
            .unwrap_or(0);
        pair.cost_micros += COST_MICROS.read(row).and_then(Value::as_u64).unwrap_or(0);
    }
    for ((role, challenger), pair) in &mut replayed.pairs {
        pair.standing = standing(rows, role, challenger);
    }
    replayed
}

/// 재생은 키에서 추첨·눈가림을 다시 뽑아 행과 대조하고, 쌍마다 시도당 마지막 말 하나로 세며, 비용을 더하고, 붙든 이유를 센다.
#[test]
fn a_replay_rederives_the_draw_and_the_blind_and_reads_one_word_per_attempt() {
    let drawn: Vec<String> = thousand_names()
        .into_iter()
        .filter(|key| draws(key))
        .take(3)
        .collect();
    let mut rows: Vec<Value> = drawn
        .iter()
        .enumerate()
        .map(|(at, key)| {
            let mut row = request_row(
                key,
                i64::try_from(at).expect("small"),
                Some(Preferred::Challenger),
            );
            row[COST_MICROS.canonical] = json!(9_870);
            row
        })
        .collect();
    // A receipt that outranks the first comparison.
    rows.push(label_row(
        &drawn[0],
        &on(Receipt::Passed, "tree"),
        Preferred::Incumbent,
        10,
    ));
    // A held attempt, and a row whose blind was written wrong.
    rows.push(held_row(&eligible(&drawn[1]), Held::NoDesign, 11));
    let mut tampered = request_row(&drawn[2], 12, Some(Preferred::Incumbent));
    tampered[BLIND.canonical] = json!(if Blind::over(&drawn[2]).first() == Side::Challenger {
        "incumbent_first"
    } else {
        "challenger_first"
    });
    rows.push(tampered);
    let undrawn = thousand_names()
        .into_iter()
        .find(|key| !draws(key))
        .expect("an undrawn name");
    rows.push(request_row(&undrawn, 13, Some(Preferred::Neither)));

    let replayed = replay(&rows);
    assert_eq!(replayed.requests, 5);
    assert_eq!(replayed.undrawn, vec![undrawn]);
    assert_eq!(replayed.blind_mismatches, vec![drawn[2].clone()]);
    assert_eq!(replayed.held.get(Held::NoDesign.token()), Some(&1));
    let pair = &replayed.pairs[&("coding".to_string(), "claude-opus-5-2".to_string())];
    // drawn[0]: the label's word (lost); drawn[1]: won; drawn[2]: its later row's word (lost); undrawn: lost.
    assert_eq!(
        pair.standing,
        Standing {
            compared: 4,
            won: 1
        }
    );
    assert_eq!(pair.cost_micros, 3 * 9_870 + 2 * 9_870);
    assert_eq!(pair.expected_micros, 5 * 12_500);
}

/// 이 기계의 도전 원장을 씨앗(`tools/challenger-replay/seed.py`)으로 받아 제품의 함수로 다시 읽는다 — 네트워크도 쓰기도 없다.
#[test]
#[ignore = "reads a seed of this machine's ledgers; run by hand"]
fn the_challenger_rows_this_machine_wrote_replayed() {
    let path = std::env::var_os("ZEROCODE_CHALLENGER_REPLAY_SEED").expect("a seed path");
    let seed: Value =
        serde_json::from_str(&std::fs::read_to_string(path).expect("a seed")).expect("json");
    let mut table = Vec::new();
    for ledger in seed["ledgers"].as_array().expect("ledgers") {
        let rows: Vec<Value> = ledger["rows"].as_array().cloned().unwrap_or_default();
        let replayed = replay(&rows);
        for ((role, challenger), pair) in &replayed.pairs {
            table.push(json!({
                "role": role,
                "challenger": challenger,
                "compared": pair.standing.compared,
                "won": pair.standing.won,
                "wonLowerBound": pair.standing.lower_bound(),
                "expectedMicros": pair.expected_micros,
                "costMicros": pair.cost_micros,
            }));
        }
        println!(
            "{}",
            json!({
                "ledger": ledger["path"],
                "rows": rows.len(),
                "requests": replayed.requests,
                "held": replayed.held,
                "undrawn": replayed.undrawn.len(),
                "blindMismatches": replayed.blind_mismatches.len(),
            })
        );
    }
    println!("{}", json!({ "until": seed["until"], "pairs": table }));
}
