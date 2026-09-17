//! 공간 — 워크스페이스가 디스크에서 차지하는 자리, 그리고 되찾을 수 있는 자리.
//!
//! [`workspace_cleanup`](crate::workspace_cleanup)과 **판정 축이 다르다.** 저쪽은
//! "유휴"를 묻고 이쪽은 "크기"를 묻는다. 30일 조용한 40KB짜리 체크아웃은 저
//! 화면의 첫 줄이고 이 화면에서는 보이지도 않으며, 어제 만든 9GB짜리
//! `node_modules`는 그 반대다. 그래서 화면이 둘이고, 목록을 만드는 규칙도 둘이다.
//!
//! **그러나 "지워도 되는가"는 하나다.** 그 물음의 답은 전부
//! [`workspace_cleanup::blockers`](crate::workspace_cleanup::blockers)가 하고, 이
//! 모듈은 그 답을 **읽기만** 한다 — [`ready_to_delete`]는 방해물을 다시 세지
//! 않고 비었는지만 본다. 두 번째 판정기를 두면 두 화면이 같은 워크스페이스를
//! 두고 다른 말을 하게 되고, 그중 하나는 사람의 파일을 지우는 쪽이다.
//!
//! 여기 있는 것은 셋이다. **상한**(48개·10만 항목·64MiB), **트리맵의 자리
//! 나누기**, 그리고 **단위 표기**. 셋 다 디스크도 시계도 만지지 않는다 —
//! 훑는 일은 창(`zerocode-shell`)이 한다.

use serde::{Deserialize, Serialize};

use crate::workspace_cleanup::Blocker;

/// 한 워크스페이스가 목록에 싣는 top-level 항목의 최대 수.
///
/// 마흔여덟. 넘는 것은 [`Kind::Other`] 한 줄로 접힌다 — 브레이크다운 패널이
/// 열두 줄만 그리고 트리맵의 사각형은 면적 80 아래에서 라벨을 잃으므로, 그
/// 아래로 내려간 항목 삼백 개를 실어 보내는 것은 창이 즉시 버릴 것을 재고
//  옮기고 그리는 일이다.
pub const MAX_TOP_LEVEL_ENTRIES: usize = 48;

/// 한 워크스페이스를 훑으며 볼 수 있는 항목의 최대 수.
///
/// 십만. 넘으면 그 행은 [`Status::Unavailable`]이고, 숫자를 지어내지 않는다.
pub const MAX_SCAN_ENTRIES: usize = 100_000;

/// 훑는 동안 들고 있어도 되는 경로 바이트의 최대 합.
///
/// 64MiB. 순회는 아직 열지 않은 디렉터리를 쌓아 두고, 깊고 넓은 트리에서 그
/// 더미가 곧 이 프로세스의 메모리다. 항목 수 상한과 별개인 이유는 둘이 다른
/// 모양에서 먼저 닿기 때문이다 — 십만 항목은 넓은 트리가, 64MiB는 경로가 긴
/// 트리가 먼저 친다.
pub const MAX_RETAINED_BYTES: usize = 64 * 1024 * 1024;

/// 단위 사다리. **1024로 나누면서 이름은 KB/MB**다.
///
/// KiB가 아니다. 사람이 이 숫자를 대조할 곳 — macOS의 파인더, `du -h`, Orca
/// 자신 — 이 전부 그렇게 적고, 라벨만 정확하게 고쳐 두면 같은 파일이 이 창에서만
/// 다른 숫자로 보인다. 정확함보다 대조 가능함을 골랐고, 그 선택을 여기 적어 둔다.
pub const BYTE_UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];

/// top-level 항목 하나가 무엇인가.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    Dir,
    File,
    /// [`MAX_TOP_LEVEL_ENTRIES`]를 넘어 접힌 나머지 전부.
    ///
    /// 이름이 없다 — 이름은 창이 자기 언어로 붙인다. 여기서 `"기타"`를 적으면
    /// 그 한 단어만 다섯 언어 중 하나로 고정된다.
    Other,
}

impl Kind {
    pub const fn slug(self) -> &'static str {
        match self {
            Kind::Dir => "dir",
            Kind::File => "file",
            Kind::Other => "other",
        }
    }
}

/// 한 워크스페이스의 top-level 항목 하나.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// [`Kind::Other`]이면 빈 문자열.
    pub name: String,
    pub size_bytes: u64,
    pub kind: Kind,
}

/// 이 행을 재는 일이 어떻게 끝났는가.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Status {
    /// 다 셌다.
    Ok,
    /// 디렉터리가 없다. git은 아직 이 워크트리를 알고 있다.
    Missing,
    /// 읽을 권한이 없다.
    NoAccess,
    /// 상한에 닿았다 — [`MAX_SCAN_ENTRIES`]이거나 [`MAX_RETAINED_BYTES`].
    Unavailable,
    /// 그 밖의 이유로 못 셌다.
    Failed,
}

impl Status {
    pub const fn slug(self) -> &'static str {
        match self {
            Status::Ok => "ok",
            Status::Missing => "missing",
            Status::NoAccess => "no-access",
            Status::Unavailable => "unavailable",
            Status::Failed => "failed",
        }
    }
}

/// 훑기가 상한에 닿았을 때.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Limit {
    /// [`MAX_SCAN_ENTRIES`].
    Entries,
    /// [`MAX_RETAINED_BYTES`].
    Memory,
    /// 사람이 취소했다. 상한은 아니지만 순회를 멈추는 이유라는 점에서 같은 자리다.
    Cancelled,
}

impl Limit {
    /// 이 끝맺음이 그 행에 남기는 상태.
    ///
    /// 취소는 상태를 만들지 않는다 — 취소된 스캔의 행은 아예 실리지 않는다.
    pub const fn status(self) -> Option<Status> {
        match self {
            Limit::Entries | Limit::Memory => Some(Status::Unavailable),
            Limit::Cancelled => None,
        }
    }
}

/// 크기순으로 세우고, [`MAX_TOP_LEVEL_ENTRIES`]를 넘는 것은 한 줄로 접는다.
///
/// 접힌 줄은 **맨 뒤**에 온다. 합이 아무리 커도 그렇다 — 이름 없는 한 줄이 목록의
/// 첫 줄에 앉으면, 사람이 읽는 첫 사실이 "무언가 많다"가 된다.
///
/// 정렬의 두 번째 열쇠는 이름이다. 같은 크기 항목이 스캔할 때마다 자리를 바꾸면
/// 트리맵의 사각형들이 이유 없이 움직인다 — `read_dir`의 순서는 파일 시스템이
/// 정하고 두 번 같으리라는 보장이 없다.
pub fn cap_entries(mut entries: Vec<Entry>) -> Vec<Entry> {
    entries.sort_by(|left, right| {
        right
            .size_bytes
            .cmp(&left.size_bytes)
            .then_with(|| left.name.cmp(&right.name))
    });
    if entries.len() <= MAX_TOP_LEVEL_ENTRIES {
        return entries;
    }
    let folded: u64 = entries[MAX_TOP_LEVEL_ENTRIES..]
        .iter()
        .map(|one| one.size_bytes)
        .sum();
    entries.truncate(MAX_TOP_LEVEL_ENTRIES);
    entries.push(Entry {
        name: String::new(),
        size_bytes: folded,
        kind: Kind::Other,
    });
    entries
}

/// 트리맵 사각형 하나. 좌표는 0..1이 아니라 호출자가 준 상자의 단위 그대로다.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    pub fn area(&self) -> f64 {
        self.w * self.h
    }
}

/// 합이 절반에 가장 가까워지는 자리.
///
/// 반환값은 **왼쪽 조각의 길이**이고 언제나 `1..values.len()`이다 — 0이나 전체를
/// 답하면 재귀가 같은 목록을 다시 받아 끝나지 않는다. 그래서 두 개짜리 목록의
/// 답은 언제나 1이고, 그것이 이 함수가 크기를 보지 않는 유일한 경우다.
///
/// 값이 전부 0이어도 그렇다: 그때는 어느 자리나 |2·prefix − total| = 0이라
/// 처음 자리가 남고, 처음 자리는 1이다.
pub fn split_balanced(values: &[f64]) -> usize {
    debug_assert!(values.len() >= 2, "자를 것이 없는 목록을 잘랐다");
    let total: f64 = values.iter().sum();
    let mut prefix = 0.0;
    let mut best = 1usize;
    let mut best_gap = f64::INFINITY;
    // 마지막 값은 후보가 아니다 — 그것까지 왼쪽에 주면 오른쪽이 빈다.
    for (index, value) in values.iter().take(values.len() - 1).enumerate() {
        prefix += value;
        let gap = (2.0 * prefix - total).abs();
        if gap < best_gap {
            best_gap = gap;
            best = index + 1;
        }
    }
    best
}

/// 균형 이분할 재귀 — 값 목록 하나를 상자 하나에 눕힌다.
///
/// 반환값은 **입력과 같은 길이, 같은 순서**다. `i`번째 값의 사각형이 `i`번째에
/// 있다 — 정렬은 호출자가 이미 했고(그리고 그것이 [`cap_entries`]다), 이 함수가
/// 다시 정렬하면 창이 값과 색을 짝지을 방법이 없어진다.
///
/// **면적이 값에 비례한다**는 것이 이 함수의 전부이고, 상자를 남기지 않는다는
/// 것이 그 절반이다: 자를 때마다 한쪽에 `w*ratio`를 주고 다른 쪽에 `w - w*ratio`를
/// 준다. `(1 - ratio)`를 곱하지 않는 이유는 부동소수점이 그 둘을 다르게 답하기
/// 때문이고, 다르면 사각형 사이에 아무도 의도하지 않은 1px 틈이 생긴다.
///
/// 합이 0인 목록은 자리를 **똑같이** 나눈다. 빈 워크스페이스 셋이 상자 하나를
/// 두고 하나가 전부 가져가는 그림보다, 셋으로 나뉜 그림이 사실에 가깝다.
pub fn layout_treemap(values: &[f64], into: Rect) -> Vec<Rect> {
    match values.len() {
        0 => Vec::new(),
        1 => vec![into],
        _ => {
            let cut = split_balanced(values);
            let (left, right) = values.split_at(cut);
            let total: f64 = values.iter().sum();
            let ratio = if total > 0.0 {
                left.iter().sum::<f64>() / total
            } else {
                cut as f64 / values.len() as f64
            };
            let (first, second) = if into.w >= into.h {
                let width = into.w * ratio;
                (
                    Rect { w: width, ..into },
                    Rect {
                        x: into.x + width,
                        w: into.w - width,
                        ..into
                    },
                )
            } else {
                let height = into.h * ratio;
                (
                    Rect { h: height, ..into },
                    Rect {
                        y: into.y + height,
                        h: into.h - height,
                        ..into
                    },
                )
            };
            let mut laid = layout_treemap(left, first);
            laid.extend(layout_treemap(right, second));
            laid
        }
    }
}

/// 바이트 하나를 사람이 읽는 말로.
///
/// 1024로 나누고 [`BYTE_UNITS`]의 이름을 붙인다. 자릿수는 값이 정한다 —
/// 100 이상이거나 바이트면 소수점 없음, 10 이상이면 한 자리, 그 아래면 두 자리.
/// `1.50 GB` / `12.3 MB` / `340 MB` / `512 B`.
///
/// 자릿수 사다리가 있는 이유는 폭이다. 이 숫자는 표의 한 칸과 트리맵 사각형
/// 안에 들어가야 하고, `1.503906 GB`는 어느 쪽에도 안 들어간다. 그리고 두
/// 자리로 고정하면 `512.00 B`가 된다 — 소수점 아래를 셀 수 없는 단위에 소수점을
/// 붙인 것이다.
pub fn format_bytes(bytes: u64) -> String {
    let mut value = bytes as f64;
    let mut unit = 0usize;
    while value >= 1024.0 && unit + 1 < BYTE_UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    let places = if value >= 100.0 || unit == 0 {
        0
    } else if value >= 10.0 {
        1
    } else {
        2
    };
    format!("{value:.places$} {}", BYTE_UNITS[unit])
}

/// 이 행의 체크박스가 켜질 수 있는가.
///
/// **방해물을 세지 않는다.** [`workspace_cleanup::blockers`](crate::workspace_cleanup::blockers)가
/// 만든 목록이 비었는지만 보고, 크기를 못 잰 행은 그 앞에서 걸린다 — 얼마를
/// 되찾는지 모르는 채로 삭제 목록에 오르는 행은 "회수 가능" 합계를 거짓말로
/// 만든다.
///
/// 이 함수가 짧은 것이 요점이다. 조건을 하나라도 여기서 다시 쓰는 순간 이
/// 화면은 두 번째 판정기를 갖게 되고, 두 판정기는 반드시 어긋난다 — 어긋나는
/// 방향은 언제나 "저쪽 화면은 지우면 안 된다고 했는데 이쪽 화면이 지웠다"이다.
pub fn ready_to_delete(status: Status, blockers: &[Blocker]) -> bool {
    matches!(status, Status::Ok) && blockers.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace_cleanup::{GitEvidence, WorktreeFacts, blockers};

    fn entry(name: &str, size: u64) -> Entry {
        Entry {
            name: name.to_string(),
            size_bytes: size,
            kind: Kind::Dir,
        }
    }

    const BOX: Rect = Rect {
        x: 0.0,
        y: 0.0,
        w: 400.0,
        h: 288.0,
    };

    /// 눕힌 사각형들의 면적 합은 상자의 면적이고, 하나도 남기지 않는다.
    #[test]
    fn the_boxes_fill_what_they_were_given_and_no_more() {
        for count in 1..=60usize {
            // 크기를 흩뜨려 둔다 — 같은 값들만으로는 비율 계산이 시험되지 않는다.
            let values: Vec<f64> = (0..count)
                .map(|one| (count - one) as f64 * 3.0 + 1.0)
                .collect();
            let laid = layout_treemap(&values, BOX);
            assert_eq!(laid.len(), count, "사각형 수가 값의 수와 다르다");
            let area: f64 = laid.iter().map(Rect::area).sum();
            assert!(
                (area - BOX.area()).abs() < 1e-6,
                "{count}개에서 면적이 새어 나갔다: {area} vs {}",
                BOX.area()
            );
            // 그리고 전부 상자 안에 있다.
            for one in &laid {
                assert!(
                    one.x >= BOX.x - 1e-9
                        && one.y >= BOX.y - 1e-9
                        && one.x + one.w <= BOX.x + BOX.w + 1e-9
                        && one.y + one.h <= BOX.y + BOX.h + 1e-9,
                    "사각형이 상자 밖으로 나갔다: {one:?}"
                );
            }
        }
    }

    /// 면적은 값에 비례하고, 순서는 입력의 순서다.
    #[test]
    fn a_bigger_value_gets_a_bigger_box_in_the_order_it_arrived() {
        let values = vec![100.0, 50.0, 25.0, 25.0];
        let laid = layout_treemap(&values, BOX);
        let total: f64 = values.iter().sum();
        for (value, rect) in values.iter().zip(&laid) {
            let want = BOX.area() * value / total;
            assert!(
                (rect.area() - want).abs() < 1e-6,
                "{value}의 면적이 비례하지 않는다: {} vs {want}",
                rect.area()
            );
        }
        // 순서가 그대로다 — 첫 값이 첫 사각형이다.
        assert!(laid[0].area() > laid[1].area());
        assert!(laid[1].area() > laid[2].area());
    }

    /// 자르는 자리는 합의 절반에 가장 가까운 자리다.
    #[test]
    fn the_cut_lands_where_the_two_halves_are_closest() {
        // 10 20 30 40: 왼쪽 합이 30이면 오른쪽 70, 60이면 40 — 60|40이 더 가깝다.
        assert_eq!(split_balanced(&[10.0, 20.0, 30.0, 40.0]), 3);
        // 하나가 전부인 경우에도 오른쪽을 비우지 않는다.
        assert_eq!(split_balanced(&[100.0, 1.0, 1.0]), 1);
        // 두 개짜리는 언제나 1 — 자를 자리가 하나뿐이다.
        assert_eq!(split_balanced(&[1.0, 1_000_000.0]), 1);
        // 전부 0이면 처음 자리.
        assert_eq!(split_balanced(&[0.0, 0.0, 0.0]), 1);
    }

    /// 합이 0인 목록도 상자를 나눠 갖는다.
    #[test]
    fn empty_workspaces_still_share_the_box() {
        let laid = layout_treemap(&[0.0, 0.0, 0.0, 0.0], BOX);
        let area: f64 = laid.iter().map(Rect::area).sum();
        assert!((area - BOX.area()).abs() < 1e-6);
        for one in &laid {
            assert!(one.area() > 0.0, "빈 워크스페이스가 자리를 못 받았다");
        }
    }

    /// 마흔여덟까지는 그대로, 그 위는 이름 없는 한 줄.
    #[test]
    fn the_forty_ninth_item_and_everything_after_it_becomes_one_line() {
        // 정확히 48개는 접히지 않는다.
        let exactly: Vec<Entry> = (0..MAX_TOP_LEVEL_ENTRIES)
            .map(|one| entry(&format!("d{one:02}"), (one as u64 + 1) * 10))
            .collect();
        let held = cap_entries(exactly);
        assert_eq!(held.len(), MAX_TOP_LEVEL_ENTRIES);
        assert!(held.iter().all(|one| one.kind != Kind::Other));

        // 49개부터 접힌다. 접힌 줄의 크기는 밀려난 것들의 합이다.
        let many: Vec<Entry> = (0..60)
            .map(|one| entry(&format!("d{one:02}"), (60 - one as u64) * 10))
            .collect();
        let want: u64 = (0..60u64)
            .map(|one| (60 - one) * 10)
            .collect::<Vec<_>>()
            .into_iter()
            .skip(MAX_TOP_LEVEL_ENTRIES)
            .sum();
        let held = cap_entries(many);
        assert_eq!(held.len(), MAX_TOP_LEVEL_ENTRIES + 1);
        let folded = held.last().unwrap();
        assert_eq!(folded.kind, Kind::Other);
        assert_eq!(folded.size_bytes, want);
        assert!(folded.name.is_empty(), "접힌 줄이 이름을 지어냈다");
        // 그리고 접힌 줄은 맨 뒤에 있다 — 합이 앞의 어느 줄보다 커도.
        assert!(
            held[..MAX_TOP_LEVEL_ENTRIES]
                .iter()
                .all(|one| one.kind != Kind::Other)
        );
    }

    /// 같은 크기의 두 항목은 언제나 같은 순서로 선다.
    #[test]
    fn a_tie_is_broken_by_name_so_the_picture_stops_moving() {
        let held = cap_entries(vec![
            entry("zebra", 10),
            entry("apple", 10),
            entry("mango", 10),
        ]);
        let names: Vec<&str> = held.iter().map(|one| one.name.as_str()).collect();
        assert_eq!(names, vec!["apple", "mango", "zebra"]);
    }

    /// 1024 경계와 자릿수 사다리.
    #[test]
    fn the_units_climb_at_1024_and_the_places_follow_the_value() {
        // 바이트에는 소수점이 없다.
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1023), "1023 B");
        // 경계에서 한 칸 오른다.
        assert_eq!(format_bytes(1024), "1.00 KB");
        // 10 아래는 두 자리, 10 이상은 한 자리, 100 이상은 없음.
        assert_eq!(format_bytes(1024 * 9), "9.00 KB");
        assert_eq!(format_bytes(1024 * 10), "10.0 KB");
        assert_eq!(format_bytes(1024 * 99), "99.0 KB");
        assert_eq!(format_bytes(1024 * 100), "100 KB");
        assert_eq!(format_bytes(1024 * 1023), "1023 KB");
        assert_eq!(format_bytes(1024 * 1024), "1.00 MB");
        // 사다리의 끝에서 멈춘다 — PB 위는 없다.
        assert_eq!(format_bytes(1024u64.pow(5)), "1.00 PB");
        assert_eq!(format_bytes(2048 * 1024u64.pow(5)), "2048 PB");
        // 1.50 GB — 스펙이 예로 든 값.
        assert_eq!(format_bytes(1024 * 1024 * 1536), "1.50 GB");
    }

    /// 삭제 준비 판정은 분류기의 방해물 목록 하나만 읽는다.
    #[test]
    fn the_only_judge_of_deletability_is_the_cleanup_classifier() {
        // 증명된 깨끗함 + 방해물 없음 = 준비 완료.
        let clean = WorktreeFacts {
            git: GitEvidence::clean(),
            ..WorktreeFacts::default()
        };
        assert!(blockers(&clean).is_empty());
        assert!(ready_to_delete(Status::Ok, &blockers(&clean)));

        // 분류기가 방해물을 하나라도 말하면 준비되지 않았다. 여기서 조건을
        // 다시 쓰지 않으므로, 저쪽에 방해물이 하나 더 생기면 이쪽도 같이 안다.
        for facts in [
            WorktreeFacts {
                is_main: true,
                ..clean.clone()
            },
            WorktreeFacts {
                active: true,
                ..clean.clone()
            },
            WorktreeFacts {
                pinned: true,
                ..clean.clone()
            },
            WorktreeFacts {
                running_terminal: true,
                ..clean.clone()
            },
            WorktreeFacts {
                unsaved_edits: true,
                ..clean.clone()
            },
            WorktreeFacts {
                git: GitEvidence {
                    dirty_files: 1,
                    ..GitEvidence::clean()
                },
                ..clean.clone()
            },
            // 아직 git에게 묻지 않은 행 — 이 화면이 열리자마자의 모든 행이다.
            WorktreeFacts {
                git: GitEvidence::unknown(),
                ..clean.clone()
            },
        ] {
            let held = blockers(&facts);
            assert!(!held.is_empty());
            assert!(
                !ready_to_delete(Status::Ok, &held),
                "방해물이 있는데 체크박스가 켜진다: {held:?}"
            );
        }

        // 크기를 못 잰 행은 방해물이 없어도 켜지지 않는다.
        for status in [
            Status::Missing,
            Status::NoAccess,
            Status::Unavailable,
            Status::Failed,
        ] {
            assert!(
                !ready_to_delete(status, &[]),
                "{}인 행이 삭제 목록에 올랐다",
                status.slug()
            );
        }
    }

    #[test]
    fn every_slug_is_its_own_and_is_what_goes_on_the_wire() {
        let mut seen = vec![
            Status::Ok.slug(),
            Status::Missing.slug(),
            Status::NoAccess.slug(),
            Status::Unavailable.slug(),
            Status::Failed.slug(),
            Kind::Dir.slug(),
            Kind::File.slug(),
            Kind::Other.slug(),
        ];
        let held = seen.len();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), held, "두 이름이 겹친다");
        assert_eq!(
            serde_json::to_string(&Status::NoAccess).unwrap(),
            "\"no-access\""
        );
        assert_eq!(serde_json::to_string(&Kind::Other).unwrap(), "\"other\"");
        // 상한에 닿은 것은 unavailable이고, 취소는 상태를 만들지 않는다.
        assert_eq!(Limit::Entries.status(), Some(Status::Unavailable));
        assert_eq!(Limit::Memory.status(), Some(Status::Unavailable));
        assert_eq!(Limit::Cancelled.status(), None);
    }
}
