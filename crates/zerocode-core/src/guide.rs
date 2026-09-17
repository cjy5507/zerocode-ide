//! 마법사가 닫힌 다음에도 남는 세 표면 — 체크리스트, 코치마크, 팁.
//!
//! Orca의 첫 실행 경험은 마법사 하나가 아니라 저장 키·트리거·해제 경로가 각각
//! 다른 여덟 표면이다(1-ea). 마법사와 랜딩은 앞 조각이 옮겼고, 여기 있는 것은
//! 그 뒤에 남는 것들이다:
//!
//! - **시작 체크리스트** — 사이드바 한 줄과 모달. Orca가 이 표면에서 제일 잘한
//!   것은 완료 여부를 **저장하지 않는 것**이다(`use-setup-guide-progress`). 매
//!   렌더 앱 상태에서 파생하므로 "완료 표시" 오버라이드가 없고, 그래서 상태와
//!   화면이 어긋날 자리가 없다. 저장하는 순간 워크트리를 지운 사람의 목록에
//!   초록 체크가 남는다.
//! - **코치마크 투어** — 백드롭 없는 말풍선. 노출은 **세션당 하나**뿐이고, 한
//!   번 뜬 투어는 다시 뜨지 않는다.
//! - **일회성 팁** — **앱 오픈당 하나**. 온보딩이 떠 있는 실행에서는 아예
//!   봉인된다: 마법사를 막 끝낸 사람에게 곧바로 팁을 띄우면 첫 성공의 순간을
//!   광고로 덮는다.
//!
//! 파생할 수 없는 사실 — "diff를 열어 봤나" 같은 것 — 만 체크리스트 칸으로
//! 저장되고, 그 칸은 전부 창의 어떤 이벤트 지점 하나가 켠다
//! ([`crate::onboarding::MARKS`]).

use serde::Serialize;

use crate::onboarding::Onboarding;

/* ---- 시작 체크리스트 ---- */

/// 두 무리. Orca도 `Setup`과 `Milestones`로 나눠 센다.
pub const SECTION_SETUP: &str = "setup";
pub const SECTION_MILESTONE: &str = "milestone";

/// 한 칸과, 지금 그것이 끝났는지.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Step {
    pub id: &'static str,
    pub section: &'static str,
    pub done: bool,
}

/// 저장할 수 없는 쪽의 사실들 — 창이 지금 보고 있는 앱 상태.
///
/// 이 구조체가 곧 "파생하라"는 원칙의 형태다. 여기 있는 넷은 어디에도 적히지
/// 않고 매번 다시 재어지므로, 프로젝트를 하나 지운 사람의 체크리스트는 다음
/// 그리기에서 스스로 되돌아간다.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Facts {
    /// 기본 에이전트를 골라 두었나.
    pub chose_agent: bool,
    /// 이 창이 아는 프로젝트 수.
    pub projects: usize,
    /// main이 아닌 체크아웃 수 — 병렬 작업이 실제로 서 있는가.
    pub side_worktrees: usize,
    /// GitHub CLI가 연결돼 있나.
    pub github: bool,
}

/// 선언된 칸 전부와, 그것을 켜는 근거.
///
/// 근거가 [`Facts`]인 칸과 저장된 체크인 칸이 섞여 있는 것은 의도다. 파생할 수
/// 있는 것은 파생하고, 파생할 수 없는 것만 적는다.
#[must_use]
pub fn steps(state: &Onboarding, facts: &Facts) -> Vec<Step> {
    let check = state.checklist;
    vec![
        Step {
            id: "default-agent",
            section: SECTION_SETUP,
            done: facts.chose_agent,
        },
        Step {
            id: "notifications",
            section: SECTION_SETUP,
            done: check.notified,
        },
        Step {
            id: "github",
            section: SECTION_SETUP,
            done: facts.github,
        },
        Step {
            id: "two-projects",
            section: SECTION_SETUP,
            done: facts.projects >= 2,
        },
        Step {
            id: "two-worktrees",
            section: SECTION_MILESTONE,
            done: facts.side_worktrees >= 1,
        },
        Step {
            id: "parallel-agents",
            section: SECTION_MILESTONE,
            done: check.ran_second_agent,
        },
        Step {
            id: "review-diff",
            section: SECTION_MILESTONE,
            done: check.reviewed_diff,
        },
        Step {
            id: "open-pr",
            section: SECTION_MILESTONE,
            done: check.opened_pr,
        },
    ]
}

/// 다 끝났나. 빈 목록은 끝난 것이 아니다 — 아무것도 재지 않은 것이다.
#[must_use]
pub fn complete(steps: &[Step]) -> bool {
    !steps.is_empty() && steps.iter().all(|one| one.done)
}

/// 사이드바에 그 한 줄이 서 있어야 하나.
///
/// Orca의 조건 그대로 셋(`ready && !setupComplete && !dismissed`)이되, **한
/// 가지는 일부러 다르다**: Orca는 파일을 읽는 시점에 "온보딩이 닫혀 있으면
/// 자동으로 숨김"을 걸어(`resolveSetupGuideSidebarDismissedOnLoad`) 이 표면을
/// 사실상 죽은 기능으로 만든다 — 체크리스트를 보는 사람은 "이번 실행에서
/// 마법사를 막 끝낸 사람"뿐이다. 우리는 그 기본값을 걸지 않는다. 치우는 것은
/// 사람이 하고, 다 끝나면 알아서 사라진다.
#[must_use]
pub fn entry_visible(ready: bool, complete: bool, dismissed: bool) -> bool {
    ready && !complete && !dismissed
}

/* ---- 코치마크 투어 ---- */

/// 이 창에 실제로 앵커가 서 있는 투어들.
///
/// Orca의 일곱 중 넷은 우리에게 가리킬 것이 없어 옮기지 않았다(1-eb). 가리킬
/// 것이 없는 코치마크는 화면 한복판에 뜨는 설명문이 되고, 그건 투어가 아니라
/// 광고다.
pub const TOURS: &[&str] = &["board", "tasks", "automations"];

/// 왜 지금은 안 뜨는가. Orca의 `getContextualTourRequestDecision`이 돌려주는
/// 타입화된 거절 사유와 같은 목록이다 — 불리언 하나로 뭉치면 "왜 안 뜨지"를
/// 물을 수 없고, 그 질문은 이 기능에서 가장 자주 나온다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TourRefusal {
    /// 이 화면이 그 투어의 자리가 아니다.
    NotHere,
    /// 창이 아직 저장된 상태를 다 읽지 않았다.
    NotReady,
    /// 이 프로필은 자동 투어를 받지 않는다(백필된 기존 사용자).
    AutoDisabled,
    /// 마법사가 떠 있다.
    Onboarding,
    /// 무언가가 화면을 덮고 있다.
    Modal,
    /// 이미 다른 투어가 떠 있다.
    ActiveTour,
    /// 이 실행에서 이미 하나를 보여 줬다.
    SessionSpent,
    /// 이미 본 투어다.
    Seen,
    /// 첫 스텝이 가리킬 요소가 화면에 없다.
    NoTarget,
    /// 우리가 모르는 투어 이름.
    Unknown,
}

/// 투어 하나를 띄워 달라는 요청, 그 순간의 사실 전부.
#[derive(Debug, Clone, Copy)]
pub struct TourAsk<'a> {
    pub id: &'a str,
    /// 이 화면이 그 투어의 자리인가.
    pub here: bool,
    pub ready: bool,
    pub auto: Option<bool>,
    pub onboarding_open: bool,
    pub modal_open: bool,
    pub active_tour: bool,
    pub spent_this_session: bool,
    pub seen: &'a [String],
    pub has_target: bool,
}

/// 띄울까. 순서가 곧 뜻이다 — 앞의 사유일수록 "지금이 아니다"에 가깝고, 뒤로
/// 갈수록 "영영 아니다"에 가깝다.
///
/// # Errors
/// 띄우지 않는 이유 하나.
pub fn tour_decision(ask: &TourAsk<'_>) -> Result<(), TourRefusal> {
    if !TOURS.contains(&ask.id) {
        return Err(TourRefusal::Unknown);
    }
    if !ask.here {
        return Err(TourRefusal::NotHere);
    }
    if !ask.ready {
        return Err(TourRefusal::NotReady);
    }
    if ask.auto != Some(true) {
        return Err(TourRefusal::AutoDisabled);
    }
    if ask.onboarding_open {
        return Err(TourRefusal::Onboarding);
    }
    if ask.modal_open {
        return Err(TourRefusal::Modal);
    }
    if ask.active_tour {
        return Err(TourRefusal::ActiveTour);
    }
    if ask.spent_this_session {
        return Err(TourRefusal::SessionSpent);
    }
    if ask.seen.iter().any(|one| one == ask.id) {
        return Err(TourRefusal::Seen);
    }
    if !ask.has_target {
        return Err(TourRefusal::NoTarget);
    }
    Ok(())
}

/* ---- 일회성 팁 ---- */

/// 팁 셋, 뜨는 순서대로.
///
/// Orca는 `priority === "new"`를 앞세워 정렬하는데(`store:30717`), 우리 셋은
/// 선언 순서가 곧 그 순서라 별도의 우선순위 칸을 두지 않았다 — 값이 하나뿐인
/// 칸은 정렬하지 않는다.
pub const TIPS: &[Tip] = &[
    Tip {
        id: "palette",
        met_by: crate::onboarding::MARK_USED_PALETTE,
    },
    Tip {
        id: "panels",
        met_by: crate::onboarding::MARK_SHAPED_SIDEBAR,
    },
    Tip {
        id: "editor",
        met_by: crate::onboarding::MARK_OPENED_FILE,
    },
];

/// 한 팁과, 그것을 이미 아는 사람을 알아보는 근거.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tip {
    pub id: &'static str,
    /// 이 체크가 켜져 있으면 그 사람은 이미 이 기능을 쓴 것이다.
    pub met_by: &'static str,
}

/// 이번 실행에 팁을 띄울지, 그리고 띄우지 않고 조용히 끝낼 것들.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TipVerdict {
    /// 마법사가 떠 있다. Orca처럼 이 실행 **전체**를 봉인한다 — 마법사를 닫는
    /// 순간 팁이 튀어나오는 것을 막는 것이 이 사유가 따로 있는 이유다.
    SuppressForOnboarding,
    /// 지금은 아니다.
    Skip,
    /// 이것을 띄운다. `settle`은 이미 조건을 충족해 **보여 주지 않고** 본 것으로
    /// 적을 것들이다 — 이미 쓰고 있는 기능을 가르치는 팁은 방해일 뿐이다.
    Show {
        id: &'static str,
        settle: Vec<&'static str>,
    },
}

/// 팁 하나를 띄워 달라는 요청, 그 순간의 사실 전부.
#[derive(Debug, Clone, Copy)]
pub struct TipAsk {
    pub ready: bool,
    pub onboarding_open: bool,
    /// 이 실행에서 이미 마법사 때문에 봉인됐다.
    pub sealed: bool,
    /// 이 실행에서 이미 하나 띄웠다.
    pub spent_this_open: bool,
    pub modal_open: bool,
}

/// 이번 오픈에 무엇을 할까.
#[must_use]
pub fn tip_verdict(state: &Onboarding, ask: &TipAsk) -> TipVerdict {
    // 마법사가 먼저다. 다른 어떤 사유보다 앞에 서야 이 실행이 봉인된다.
    if ask.onboarding_open {
        return TipVerdict::SuppressForOnboarding;
    }
    if ask.sealed || ask.spent_this_open || !ask.ready || ask.modal_open {
        return TipVerdict::Skip;
    }
    let seen = |id: &str| state.tips_seen.iter().any(|one| one == id);
    let mut settle = Vec::new();
    let mut show = None;
    for tip in TIPS {
        if seen(tip.id) {
            continue;
        }
        if met(state, tip.met_by) {
            settle.push(tip.id);
            continue;
        }
        if show.is_none() {
            show = Some(tip.id);
        }
    }
    match show {
        Some(id) => TipVerdict::Show { id, settle },
        None => TipVerdict::Skip,
    }
}

/// 그 체크가 켜져 있나.
fn met(state: &Onboarding, mark: &str) -> bool {
    use crate::onboarding as onb;
    let check = state.checklist;
    match mark {
        onb::MARK_USED_PALETTE => check.used_palette,
        onb::MARK_SHAPED_SIDEBAR => check.shaped_sidebar,
        onb::MARK_OPENED_FILE => check.opened_file,
        onb::MARK_NOTIFIED => check.notified,
        onb::MARK_SECOND_AGENT => check.ran_second_agent,
        onb::MARK_REVIEWED_DIFF => check.reviewed_diff,
        onb::MARK_OPENED_PR => check.opened_pr,
        _ => false,
    }
}

/* ---- 기능 투어(수동) ---- */

/// 기능 투어의 갈래들. **자동으로 뜨는 길은 없다** — 도움말 메뉴 한 곳에서만
/// 열린다(Orca도 네이티브 Help 메뉴의 한 항목뿐이고, 그 선택은 옳다).
pub const WALL: &[&str] = &["workspaces", "agents", "workbench", "review"];

/// 창이 "봤다"고 말할 수 있는 목록들, 그리고 그 목록이 아는 이름.
pub const SEEN_TOUR: &str = "tour";
pub const SEEN_TIP: &str = "tip";
pub const SEEN_WALL: &str = "wall";

/// 그 목록이 그 이름을 아는가. 모르는 이름은 적히지 않는다 — 저장 파일은 사람이
/// 열어 고칠 수 있는 곳에 있고, 우리가 더는 그리지 않는 투어의 이름이 목록에
/// 쌓이면 그 목록은 아무 질문에도 답하지 못하게 된다.
#[must_use]
pub fn known(kind: &str, id: &str) -> bool {
    match kind {
        SEEN_TOUR => TOURS.contains(&id),
        SEEN_TIP => TIPS.iter().any(|tip| tip.id == id),
        SEEN_WALL => WALL.contains(&id),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::onboarding::{self, Checklist};

    fn ids(list: &[&str]) -> Vec<String> {
        list.iter().map(|one| (*one).to_string()).collect()
    }

    /// 완료는 저장되지 않는다 — 앱 상태가 물러나면 체크도 물러난다.
    ///
    /// 이 시험이 이 모듈의 존재 이유다. 저장했다면 프로젝트를 하나 지운 뒤에도
    /// "저장소 둘"이 초록으로 남고, 그 순간 목록은 화면이 아니라 기억이 된다.
    #[test]
    fn a_derived_step_goes_back_when_the_thing_it_measured_goes_away() {
        let state = Onboarding::default();
        let two = Facts {
            chose_agent: true,
            projects: 2,
            side_worktrees: 1,
            github: true,
        };
        let done = |facts: &Facts, id: &str| {
            steps(&state, facts)
                .into_iter()
                .find(|step| step.id == id)
                .expect("사라진 칸")
                .done
        };

        assert!(done(&two, "two-projects"));
        assert!(done(&two, "two-worktrees"));
        assert!(done(&two, "default-agent"));
        assert!(done(&two, "github"));

        let one = Facts { projects: 1, ..two };
        assert!(
            !done(&one, "two-projects"),
            "지운 프로젝트가 초록으로 남았다"
        );
        let alone = Facts {
            side_worktrees: 0,
            ..two
        };
        assert!(!done(&alone, "two-worktrees"));

        // 그리고 파생되는 칸은 저장된 체크를 **하나도** 읽지 않는다. 한 칸이라도
        // 곁눈질하면 그 칸은 파생인 척하는 저장이 되고, 근거가 물러나도 초록으로
        // 남는다 — 이 목록이 피하려던 실패 그 자체다.
        let everything = Onboarding {
            checklist: Checklist {
                chose_agent: true,
                dismissed: true,
                notified: true,
                ran_second_agent: true,
                reviewed_diff: true,
                opened_pr: true,
                used_palette: true,
                shaped_sidebar: true,
                opened_file: true,
            },
            ..Onboarding::default()
        };
        let nothing = Facts::default();
        for id in ["default-agent", "two-projects", "two-worktrees", "github"] {
            let derived = steps(&everything, &nothing)
                .into_iter()
                .find(|step| step.id == id)
                .expect("사라진 칸");
            assert!(
                !derived.done,
                "`{id}`이(가) 저장된 체크를 읽는다 — 파생인 척하는 저장이다"
            );
        }

        // 그리고 파생할 수 없는 쪽은 체크가 켠다.
        assert!(!done(&two, "review-diff"));
        let read = onboarding::marked(&state, onboarding::MARK_REVIEWED_DIFF).expect("모르는 칸");
        assert!(
            steps(&read, &two)
                .into_iter()
                .find(|step| step.id == "review-diff")
                .expect("사라진 칸")
                .done
        );
    }

    /// 다 끝나면 사라지고, 사람이 치우면 사라지고, 아직 안 읽었으면 안 뜬다.
    #[test]
    fn the_sidebar_line_stands_only_while_it_has_something_to_say() {
        assert!(entry_visible(true, false, false));
        assert!(!entry_visible(false, false, false), "안 읽은 상태를 그린다");
        assert!(
            !entry_visible(true, true, false),
            "다 끝난 목록이 남아 있다"
        );
        assert!(!entry_visible(true, false, true), "치운 줄이 돌아왔다");

        // 빈 목록은 완료가 아니다.
        assert!(!complete(&[]));
        let state = Onboarding::default();
        let all = Facts {
            chose_agent: true,
            projects: 2,
            side_worktrees: 1,
            github: true,
        };
        assert!(!complete(&steps(&state, &all)), "체크 넷이 아직 비어 있다");

        let mut walked = state;
        for mark in [
            onboarding::MARK_NOTIFIED,
            onboarding::MARK_SECOND_AGENT,
            onboarding::MARK_REVIEWED_DIFF,
            onboarding::MARK_OPENED_PR,
        ] {
            walked = onboarding::marked(&walked, mark).expect("모르는 칸");
        }
        assert!(complete(&steps(&walked, &all)));
    }

    /// 세션당 하나, 한 번 본 것은 다시 안 뜬다, 그리고 백필된 사람에게는 영영.
    #[test]
    fn a_tour_gets_one_chance_per_run_and_only_where_it_can_point() {
        let seen: Vec<String> = ids(&["tasks"]);
        let base = TourAsk {
            id: "board",
            here: true,
            ready: true,
            auto: Some(true),
            onboarding_open: false,
            modal_open: false,
            active_tour: false,
            spent_this_session: false,
            seen: &seen,
            has_target: true,
        };
        assert_eq!(tour_decision(&base), Ok(()));

        for (ask, why) in [
            (TourAsk { id: "nope", ..base }, TourRefusal::Unknown),
            (
                TourAsk {
                    here: false,
                    ..base
                },
                TourRefusal::NotHere,
            ),
            (
                TourAsk {
                    ready: false,
                    ..base
                },
                TourRefusal::NotReady,
            ),
            (TourAsk { auto: None, ..base }, TourRefusal::AutoDisabled),
            (
                TourAsk {
                    auto: Some(false),
                    ..base
                },
                TourRefusal::AutoDisabled,
            ),
            (
                TourAsk {
                    onboarding_open: true,
                    ..base
                },
                TourRefusal::Onboarding,
            ),
            (
                TourAsk {
                    modal_open: true,
                    ..base
                },
                TourRefusal::Modal,
            ),
            (
                TourAsk {
                    active_tour: true,
                    ..base
                },
                TourRefusal::ActiveTour,
            ),
            (
                TourAsk {
                    spent_this_session: true,
                    ..base
                },
                TourRefusal::SessionSpent,
            ),
            (
                TourAsk {
                    id: "tasks",
                    ..base
                },
                TourRefusal::Seen,
            ),
            (
                TourAsk {
                    has_target: false,
                    ..base
                },
                TourRefusal::NoTarget,
            ),
        ] {
            assert_eq!(tour_decision(&ask), Err(why), "{why:?}가 통과했다");
        }
    }

    /// 오픈당 하나. 그리고 마법사가 떠 있던 실행은 통째로 봉인된다.
    #[test]
    fn a_tip_shows_once_per_open_and_never_in_the_run_that_taught_the_wizard() {
        let fresh = Onboarding::default();
        let ask = TipAsk {
            ready: true,
            onboarding_open: false,
            sealed: false,
            spent_this_open: false,
            modal_open: false,
        };

        assert_eq!(
            tip_verdict(&fresh, &ask),
            TipVerdict::Show {
                id: "palette",
                settle: Vec::new(),
            }
        );
        // 마법사가 떠 있으면 그 사실 자체가 답이다 — 창은 이 답을 받아 이
        // 실행을 봉인한다.
        assert_eq!(
            tip_verdict(
                &fresh,
                &TipAsk {
                    onboarding_open: true,
                    ..ask
                }
            ),
            TipVerdict::SuppressForOnboarding
        );
        for blocked in [
            TipAsk {
                sealed: true,
                ..ask
            },
            TipAsk {
                spent_this_open: true,
                ..ask
            },
            TipAsk {
                ready: false,
                ..ask
            },
            TipAsk {
                modal_open: true,
                ..ask
            },
        ] {
            assert_eq!(tip_verdict(&fresh, &blocked), TipVerdict::Skip);
        }

        // 이미 그 기능을 쓰고 있는 사람에게는 보여 주지 않고 조용히 끝낸다.
        let handy = Onboarding {
            checklist: Checklist {
                used_palette: true,
                shaped_sidebar: true,
                ..Checklist::default()
            },
            ..Onboarding::default()
        };
        assert_eq!(
            tip_verdict(&handy, &ask),
            TipVerdict::Show {
                id: "editor",
                settle: vec!["palette", "panels"],
            }
        );

        // 본 것은 다시 안 뜬다. 셋을 다 지나면 더 띄울 것이 없다.
        let done = Onboarding {
            tips_seen: ids(&["palette", "panels", "editor"]),
            ..Onboarding::default()
        };
        assert_eq!(tip_verdict(&done, &ask), TipVerdict::Skip);
    }

    /// 목록은 자기가 아는 이름만 받는다.
    #[test]
    fn a_list_refuses_a_name_no_surface_answers_to() {
        assert!(known(SEEN_TOUR, "board"));
        assert!(!known(SEEN_TOUR, "browser"), "옮기지 않은 투어가 적힌다");
        assert!(known(SEEN_TIP, "palette"));
        assert!(!known(SEEN_TIP, "board"), "목록끼리 이름이 새어 든다");
        assert!(known(SEEN_WALL, "review"));
        assert!(!known("whatever", "board"));

        // 더해지기만 하고, 같은 것을 두 번 적지 않는다.
        let held = ids(&["board"]);
        assert_eq!(
            onboarding::seen(&held, "tasks"),
            Some(ids(&["board", "tasks"]))
        );
        assert_eq!(onboarding::seen(&held, "board"), None, "중복이 적힌다");
    }
}
