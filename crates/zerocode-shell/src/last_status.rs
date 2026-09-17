//! 재시작을 살아남는, 판마다의 마지막 소식 (P0-13).
//!
//! Orca는 훅 서버가 판별 마지막 상태를 `last-status.json`(v2)으로 적고
//! 부팅에서 도로 읽는다 — 그래서 앱을 껐다 켜도 보드가 비지 않고, 어제의
//! 대화가 "이어서" 행으로 남는다(`agent-hooks/server.ts:180-195,
//! 2933-3010(hydrate), 3170-3233(persist)`). 이쪽의 등가물: 훅이 갱신하는
//! `pane_states`/`pane_sessions`는 메모리뿐이라 재시작이 곧 전소였다
//! (맵 P0-13, `main.rs:345` 부근의 상태 필드들).
//!
//! 이 모듈은 그 파일의 **순수한 반**이다 — 포맷, 위생, 나이 판정, 그리고
//! durable_file을 통한 원자 커밋. 언제 적을지(250ms 디바운스)와 무엇을
//! 적을지(훅 갱신 지점)는 상태를 쥔 main.rs의 배선이 정한다. 산술과 I/O를
//! 이렇게 갈라 두는 것은 resume_watch가 잠 판정에서 한 것과 같은 이유다:
//! 계약은 시계 없이 시험할 수 있어야 한다.
//!
//! TermId는 일부러 열쇠에서 뺐다: 재시작한 백엔드는 새 번호를 발급하므로
//! 옛 번호는 어떤 살아있는 판도 가리키지 않는다. 살아남는 사실은
//! (워크트리, 에이전트, 벤더의 세션 id, 마지막 상태)이고, 소비자는 이
//! 목록으로 보드 카드와 이어서 행을 세운다 — Orca가 stablePaneKey로
//! 남기는 것과 같은 알갱이다.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::durable_file;

/// Orca 자신의 판번호(`LAST_STATUS_FILE_VERSION = 2`, server.ts:187).
/// 불일치는 조용한 무시다 — 옛 파일을 고쳐 읽으려는 시도는 곧 두 번째
/// 파서를 낳는다.
pub const LAST_STATUS_VERSION: u32 = 2;

/// Orca의 버스트 상한 그대로(`STATUS_PERSIST_DEBOUNCE_MS = 250`,
/// server.ts:190): 한 턴이 쏟는 수십 개의 도구 이벤트가 수십 번의 fsync가
/// 되지 않게, 마지막 이벤트 뒤에만 적는다. 배선 쪽 디바운스가 이 값을 읽는다.
pub const PERSIST_DEBOUNCE: Duration = Duration::from_millis(250);

/// Orca의 보관 한도 그대로(`HYDRATE_MAX_AGE_MS = 7d`, server.ts:195):
/// 일주일 넘게 손대지 않은 소식은 부팅에서 걸러진다 — 디스크가 판의 무덤이
/// 되지 않게 하는 축출 규칙이다(북극성: 경계 없는 컬렉션은 결함).
pub const HYDRATE_MAX_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// 한 판의 마지막 소식 — 재시작 후에도 말이 되는 필드만.
///
/// `ask`/`approval`은 싣지 않는다: 질문은 그 한 이벤트 동안만 유효하다는
/// 것이 원 계약이고(hooks.rs `PaneHookReport::prompt` 주석, Orca의
/// interactivePrompt), 죽은 프로세스의 질문에 대답할 손은 없다.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LastStatus {
    /// 카드가 앉을 자리 — 훅 봉투의 `worktree_id` 그대로.
    pub worktree: String,
    /// 에이전트 슬러그(`claude`, `codex`, …) — AgentKind의 문자 옷.
    pub agent: String,
    /// `working`/`needs-attention`/`done` — 탭이 입는 세 사실.
    pub state: zerocode_core::hook::HookState,
    /// 마지막 갱신, epoch 밀리초 — 보드의 정렬 열쇠.
    pub at: i64,
    /// 이 상태에 들어선 시각 — `at`과 다른 질문에 답한다(P0-5의 교훈).
    pub state_started_at: i64,
    /// 사람의 마지막 프롬프트 — Orca `lastUserMessage`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub you: Option<String>,
    /// 에이전트의 마지막 답 — Orca `lastAssistantMessage`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub said: Option<String>,
    /// 벤더의 대화 id — 있어야 "이어서"가 선다.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<zerocode_core::ProviderSession>,
    /// 그 세션으로 실제로 돌아갈 수 있는지 — 테이블은 이쪽이 쥔다.
    #[serde(default)]
    pub resumable: bool,
    /// 이 소식을 받아 적은 시각, epoch 밀리초 — 나이 판정의 기준.
    pub received_at: i64,
    /// 이 `done`이 되살린 세션의 경계였다 — 완료가 아니다.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub session_boundary: bool,
    /// 이 `done`은 **사람이** 끝냈다 — 에이전트가 끝낸 것이 아니다.
    ///
    /// 둘을 싣는 이유는 **재시작 뒤에도 완료가 아니어야** 하기 때문이다.
    /// 이 깃발을 잃으면 하이드레이트가 사람이 멈춘 턴을 「끝난 일」로 되살리고,
    /// 완료 시계와 알림이 그것을 완료로 읽는다(Orca도 둘을 한 표현에서 함께
    /// 읽는다 — `agent-finished-timestamp.ts:27`). 둘 다 `#[serde(default)]`라
    /// 옛 파일은 판번호를 올리지 않고도 그대로 읽힌다.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub interrupted: bool,
}

/// 디스크에 눕는 전체 모양 — Orca의 `LastStatusFile`과 같은 세 자리
/// (버전, 엔트리; authorityCommitments는 훅 인증이 이쪽 설계에 없으므로
/// 자리 자체가 없다 — 빈 흉내는 두 번째 거짓말이다).
///
/// 엔트리의 종류가 열린 것은 **읽기와 쓰기가 다른 것을 원하기** 때문이다.
/// 쓸 때는 [`LastStatus`] 그대로지만, 읽을 때는 먼저 [`serde_json::Value`]로
/// 받아 행마다 따로 해석한다 — 한 행이 낯설다고 다른 판 전부의 소식을
/// 버리지 않기 위해서다.
#[derive(Debug, Serialize, Deserialize)]
struct LastStatusFile<Entries> {
    version: u32,
    entries: Entries,
}

/// 부팅이 디스크에서 되찾은 것 — 행들과, 그것을 되찾는 동안 잃은 것의 수와,
/// 디스크에 있던 그대로의 바이트.
#[derive(Debug, Default)]
pub struct Hydrated {
    /// 살아 온 행들.
    pub entries: HashMap<String, LastStatus>,
    /// 파일이 들고 있었으나 살아 오지 못한 행의 수 — 모양이 낯설었거나
    /// 나이가 문턱 밖이었던 것 둘 다. 호출자가 한 줄로 남긴다.
    pub pruned: usize,
    /// 디스크에 있던 바이트 그대로.
    ///
    /// [`Self::pruned`]가 0일 때만 동일성 스킵의 **예열**에 쓸 수 있다: 하나라도
    /// 버렸으면 우리가 들고 있는 것과 파일이 갈라졌으므로, 그때는 예열하지 않는
    /// 것이 곧 「고친 파일을 반드시 적는다」가 된다(Orca도 걸러냈으면 되적고
    /// 아니면 원바이트로 예열한다 — `server.ts:3046-3057`).
    pub raw: Vec<u8>,
}

/// 부팅의 되읽기 — Orca `hydrateLastStatusFromDisk`의 관용 그대로:
/// 파일 없음은 첫 실행이라 조용하고, 깨진 JSON·낯선 버전은 한 줄 경고감이지
/// 부팅을 멈출 일이 아니다. 여기서는 그 판정만 내리고 로그는 호출자가 남긴다
/// (husk가 그쪽에 산다).
///
/// **행 하나의 손상은 파일 전체의 손상이 아니다.** 예전에는 엔트리 맵을
/// [`LastStatus`]로 곧장 역직렬화했고, 그러면 한 행의 `state` 문자열 하나가
/// 낯선 것만으로 **다른 판 전부의 소식이 사라졌다** — 보드가 어제를 통째로
/// 잊는다. Orca는 그 행만 버리고 나머지를 살리며(`sanitizeHydratedEntry`,
/// `server.ts:2967-3028`), 파일 전체가 못 읽힐 때만 손을 든다(`:2926-2960`).
/// 여기서도 그렇게 한다: 봉투는 엄격하게, 행은 하나씩.
///
/// `now_ms`를 인자로 받는 것은 시험이 시계를 쥐기 위해서다.
pub fn load(path: &Path, now_ms: i64) -> Result<Hydrated, String> {
    let raw = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Hydrated::default());
        }
        Err(error) => return Err(format!("last-status 파일을 읽지 못했습니다: {error}")),
    };
    let parsed: LastStatusFile<BTreeMap<String, serde_json::Value>> =
        match serde_json::from_slice(&raw) {
            Ok(file) => file,
            Err(error) => return Err(format!("last-status 파일이 JSON이 아닙니다: {error}")),
        };
    if parsed.version != LAST_STATUS_VERSION {
        return Err(format!(
            "last-status 판번호가 다릅니다({} != {LAST_STATUS_VERSION}); 무시합니다",
            parsed.version
        ));
    }
    let cutoff = now_ms.saturating_sub(HYDRATE_MAX_AGE.as_millis() as i64);
    let held = parsed.entries.len();
    let mut entries = HashMap::with_capacity(held);
    for (key, value) in parsed.entries {
        let Ok(mut entry) = serde_json::from_value::<LastStatus>(value) else {
            continue;
        };
        if entry.received_at < cutoff {
            continue;
        }
        // 산 상태는 **되살리지 않는다.** working·needs-attention은 「지금
        // 이 순간」의 말인데, 이 파일은 창이 죽어 있던 시간의 기록이다 — 그
        // 사이 에이전트가 턴을 끝냈으면 done/Stop 훅은 받을 창이 없어
        // 유실됐고, 박제된 working이 「작업 중 5d」로 사이드바에 눌러앉았다
        // (실기 보고). 진짜 도는 세션은 재개 훅(SessionStart·PreToolUse)이
        // 몇 초 안에 다시 working을 세우므로, 강등의 대가는 「잠깐 idle」
        // 이고 박제의 대가는 「영원한 거짓」이다. Orca도 hydrate에서 옛
        // 판의 권위를 대조해 무력화한다(launchTokenHash·
        // getAgentStatusDisposition의 restart) — 우리 축은 상태 강등이다.
        if matches!(
            entry.state,
            zerocode_core::hook::HookState::Working
                | zerocode_core::hook::HookState::NeedsAttention
        ) {
            entry.state = zerocode_core::hook::HookState::Idle;
        }
        // done 출처는 **읽을 때도** 잠근다. 살아 있는 이벤트는 모순을
        // 들고 올 수 없지만 디스크의 바이트는 들고 올 수 있다 — 옛 빌드,
        // 손으로 고친 원장, 죽기 직전 반쯤 써진 파일. 쓸 때만 잠그면
        // 그 셋이 그대로 통과한다.
        entry.session_boundary =
            zerocode_core::hook::done_provenance(entry.state, entry.session_boundary);
        entry.interrupted = zerocode_core::hook::done_provenance(entry.state, entry.interrupted);
        entries.insert(key, entry);
    }
    Ok(Hydrated {
        pruned: held - entries.len(),
        entries,
        raw,
    })
}

/// 원자 커밋 — durable_file이 임시쓰기→rename→부모 fsync까지 답한다.
/// 내용이 같으면 적지 않는 동일성 스킵(Orca `lastWrittenJson`)은 배선의
/// 일이 아니라 여기의 일이다: 직전 바이트를 돌려줘서 호출자가 들고 있게 한다.
///
/// **바이트는 결정적이다.** [`BTreeMap`]으로 눕히는 것이 그 이유이고, 그것이
/// 동일성 스킵을 실제로 작동하게 만든다: 메모리의 [`HashMap`]은 프로세스마다
/// 다른 씨앗으로 순서를 정하므로, 같은 내용을 담고도 매번 다른 바이트가 되고
/// 그러면 「같으면 적지 않는다」가 한 번도 걸리지 않는다. 프로세스 안에서만
/// 견주면 그 사실이 드러나지 않고, **부팅에서 디스크의 바이트로 예열하는
/// 순간 드러난다** — 지난 프로세스의 순서는 이 프로세스의 순서가 아니다.
/// 열쇠순으로 눕히면 그 둘이 같아진다(파일이 사람 눈에 읽히고 diff가 되는
/// 것은 딸린 이득이다).
pub fn save(
    path: &Path,
    entries: &HashMap<String, LastStatus>,
    last_written: Option<&[u8]>,
) -> Result<Option<Vec<u8>>, String> {
    let file = LastStatusFile {
        version: LAST_STATUS_VERSION,
        entries: entries.iter().collect::<BTreeMap<_, _>>(),
    };
    let json = serde_json::to_vec(&file).map_err(|error| error.to_string())?;
    if last_written == Some(json.as_slice()) {
        return Ok(None);
    }
    durable_file::replace_bytes(path, &json).map_err(|error| error.to_string())?;
    Ok(Some(json))
}

/// 열쇠 — 워크트리와 (그 생애 동안의) 판 번호. NUL 구분은 경로가 품을 수
/// 없는 바이트라 충돌이 없다(창의 autoDevServerKey와 같은 선택).
/// 재시작 후 이 열쇠는 판을 가리키지 않지만, 그럴 필요도 없다: 소비자는
/// 열쇠가 아니라 값의 (worktree, session)으로 카드를 세운다.
#[must_use]
pub fn key(worktree: &str, term: u32) -> String {
    format!("{worktree}\u{0}{term}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a_status(received_at: i64) -> LastStatus {
        LastStatus {
            worktree: "wt-1".into(),
            agent: "claude".into(),
            state: zerocode_core::hook::HookState::Done,
            at: received_at,
            state_started_at: received_at,
            you: Some("테스트 돌려줘".into()),
            said: Some("617개 전부 초록입니다".into()),
            session: None,
            resumable: false,
            received_at,
            session_boundary: false,
            interrupted: false,
        }
    }

    /// 왕복이 곧 계약: 적은 그대로 읽히고, 같은 내용은 두 번 적히지 않는다.
    #[test]
    fn a_round_trip_returns_the_same_news_and_skips_identical_writes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("last-status.json");
        let mut entries = HashMap::new();
        entries.insert(key("wt-1", 7), a_status(1_000));

        let written = save(&path, &entries, None).expect("first save");
        let bytes = written.expect("first save writes");
        assert_eq!(
            save(&path, &entries, Some(&bytes)).expect("second save"),
            None,
            "identical content must not be rewritten"
        );

        let loaded = load(&path, 2_000).expect("load");
        assert_eq!(loaded.entries, entries);
    }

    /// Orca의 관용 그대로: 없는 파일은 첫 실행(빈 손), 깨진 JSON과 낯선
    /// 판번호는 경고 한 줄로 강등되지 부팅을 넘어뜨리지 않는다.
    #[test]
    fn absence_is_quiet_and_damage_is_a_warning_not_a_crash() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("last-status.json");
        assert_eq!(
            load(&path, 0).expect("missing file").entries,
            HashMap::new()
        );

        std::fs::write(&path, b"{ not json").expect("write damage");
        assert!(load(&path, 0).is_err(), "damage is reported, not panicked");

        std::fs::write(
            &path,
            serde_json::to_vec(&LastStatusFile {
                version: 1,
                entries: BTreeMap::<String, LastStatus>::new(),
            })
            .expect("serialize v1"),
        )
        .expect("write v1");
        assert!(
            load(&path, 0).unwrap_err().contains("판번호"),
            "a foreign version is ignored by name"
        );
    }

    /// 이레의 문턱, Orca 자신의 수(`HYDRATE_MAX_AGE_MS`): 문턱 위는 살고
    /// 아래는 걸러진다 — 경계 없는 파일은 그 자체로 결함이므로.
    #[test]
    fn week_old_news_is_dropped_on_the_floor_at_hydrate() {
        assert_eq!(HYDRATE_MAX_AGE, Duration::from_secs(7 * 24 * 60 * 60));
        assert_eq!(PERSIST_DEBOUNCE, Duration::from_millis(250));
        assert_eq!(LAST_STATUS_VERSION, 2);

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("last-status.json");
        let week = HYDRATE_MAX_AGE.as_millis() as i64;
        let now = week * 2;
        let mut entries = HashMap::new();
        entries.insert(key("wt-1", 1), a_status(now - week)); // 문턱 위, 정확히
        entries.insert(key("wt-1", 2), a_status(now - week - 1)); // 하루 아래
        save(&path, &entries, None).expect("save");

        let loaded = load(&path, now).expect("load");
        assert_eq!(
            loaded.entries.len(),
            1,
            "only the entry on the threshold survives"
        );
        assert!(loaded.entries.contains_key(&key("wt-1", 1)));
    }

    /// 열쇠의 구분자는 경로가 품을 수 없는 바이트다 — 워크트리 이름이 어떤
    /// 모양이어도 두 열쇠가 겹치지 않는다.
    #[test]
    fn keys_cannot_collide_across_worktrees() {
        assert_ne!(key("a", 12), key("a\u{0}1", 2));
        assert!(key("wt", 3).contains('\u{0}'));
    }

    /// 행 하나의 손상은 파일 전체의 손상이 아니다.
    ///
    /// 엔트리 맵을 `LastStatus`로 곧장 역직렬화하던 시절에는 낯선 `state`
    /// 문자열 **하나**가 다른 판 전부의 소식을 데려갔다 — 보드가 어제를
    /// 통째로 잊는다. Orca는 그 행만 버리고 나머지를 살린다
    /// (`sanitizeHydratedEntry`, `server.ts:2967-3028`).
    #[test]
    fn one_damaged_row_does_not_take_the_others_with_it() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("last-status.json");
        let good = serde_json::to_value(a_status(1_000)).expect("serialize");
        let mut broken = good.clone();
        broken["state"] = serde_json::Value::String("banana".into());
        let mut missing = good.clone();
        missing
            .as_object_mut()
            .expect("an object")
            .remove("received_at");

        std::fs::write(
            &path,
            serde_json::to_vec(&serde_json::json!({
                "version": LAST_STATUS_VERSION,
                "entries": {
                    key("wt-1", 1): good,
                    key("wt-1", 2): broken,
                    key("wt-1", 3): missing,
                },
            }))
            .expect("write"),
        )
        .expect("write");

        let read = load(&path, 2_000).expect("a damaged row is not a damaged file");
        assert_eq!(read.entries.len(), 1, "the good row did not survive");
        assert!(read.entries.contains_key(&key("wt-1", 1)));
        assert_eq!(
            read.pruned, 2,
            "the drops are not counted for the husk line"
        );

        // 봉투는 여전히 엄격하다: 판번호가 다르거나 JSON이 아니면 파일 전체가
        // 답이 없다 — 행 하나의 손상과 파일의 손상은 다른 사실이다.
        std::fs::write(&path, b"{ not json").expect("write");
        assert!(load(&path, 0).is_err());
    }

    /// 바이트가 결정적이라, 부팅의 예열이 실제로 걸린다.
    ///
    /// 메모리의 `HashMap`은 프로세스마다 다른 순서를 낸다. 그걸 그대로 눕히면
    /// 같은 내용이 매번 다른 바이트가 되고, **지난 프로세스가 적은 파일로
    /// 예열하는 순간** 동일성 스킵이 한 번도 걸리지 않는다. 열쇠순으로 눕히면
    /// 그 둘이 같아진다.
    #[test]
    fn the_bytes_are_keyed_in_order_so_a_hydrate_can_prime_the_skip() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("last-status.json");

        // 두 맵이 같은 내용을 다른 순서로 담아도 바이트가 같다.
        let mut one = HashMap::new();
        one.insert(key("wt-b", 2), a_status(1_000));
        one.insert(key("wt-a", 1), a_status(1_000));
        let mut other = HashMap::new();
        other.insert(key("wt-a", 1), a_status(1_000));
        other.insert(key("wt-b", 2), a_status(1_000));
        let first = save(&path, &one, None).expect("save").expect("writes");
        assert_eq!(
            save(&path, &other, Some(&first)).expect("save"),
            None,
            "the same content in another order rewrote the file"
        );

        // 그리고 디스크에서 되읽은 원바이트가 그 스킵을 예열한다 — 버린 것이
        // 없을 때만이라는 것도 함께.
        let read = load(&path, 2_000).expect("load");
        assert_eq!(read.pruned, 0);
        assert_eq!(
            save(&path, &read.entries, Some(&read.raw)).expect("save"),
            None,
            "a boot that changed nothing still rewrote the ledger"
        );
    }

    /// done의 출처는 **읽을 때도** 잠긴다.
    ///
    /// 살아 있는 이벤트는 이 모순을 만들 수 없다 — 쓰는 길이 이미 clamp를
    /// 지난다. 디스크는 만들 수 있다: 옛 빌드가 적은 파일, 손으로 고친 원장,
    /// 죽기 직전 반쯤 써진 바이트. 쓸 때만 잠그면 그 셋이 그대로 통과해서
    /// **일하고 있는 판이 「사람이 멈춘 완료」로 되살아난다**.
    #[test]
    fn a_contradiction_on_disk_does_not_survive_the_hydrate() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("last-status.json");

        let mut working = a_status(1_000);
        working.state = zerocode_core::hook::HookState::Working;
        working.session_boundary = true;
        working.interrupted = true;
        let mut done = a_status(1_000);
        done.interrupted = true;

        let mut entries = HashMap::new();
        entries.insert(key("wt-1", 1), working);
        entries.insert(key("wt-1", 2), done);
        save(&path, &entries, None).expect("save");

        let loaded = load(&path, 2_000).expect("load").entries;
        let live = &loaded[&key("wt-1", 1)];
        assert!(
            !live.interrupted && !live.session_boundary,
            "a working row kept a done-only flag through the hydrate"
        );
        // 그리고 done 줄의 깃발은 살아남는다 — clamp는 거르는 것이지
        // 지우는 것이 아니다.
        assert!(loaded[&key("wt-1", 2)].interrupted);
    }

    /// 창이 죽어 있던 시간의 「작업 중」은 되살아나지 않는다.
    ///
    /// 재시작 사이에 턴이 끝났으면 done 훅은 받을 창이 없어 유실됐고, 그
    /// 박제가 사이드바에 「작업 중 5d」로 눌러앉았다. 진짜 도는 세션은 재개
    /// 훅이 몇 초 안에 다시 working을 세운다 — 강등의 대가는 잠깐의 idle,
    /// 박제의 대가는 영원한 거짓이다.
    #[test]
    fn a_working_row_wakes_up_idle() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("last-status.json");

        let mut working = a_status(1_000);
        working.state = zerocode_core::hook::HookState::Working;
        let mut asking = a_status(1_000);
        asking.state = zerocode_core::hook::HookState::NeedsAttention;
        let done = a_status(1_000);

        let mut entries = HashMap::new();
        entries.insert(key("wt-1", 1), working);
        entries.insert(key("wt-1", 2), asking);
        entries.insert(key("wt-1", 3), done);
        save(&path, &entries, None).expect("save");

        let loaded = load(&path, 2_000).expect("load").entries;
        assert_eq!(
            loaded[&key("wt-1", 1)].state,
            zerocode_core::hook::HookState::Idle,
            "a working row from a dead window came back still working"
        );
        assert_eq!(
            loaded[&key("wt-1", 2)].state,
            zerocode_core::hook::HookState::Idle,
            "a needs-attention row from a dead window kept asking nobody"
        );
        assert_eq!(
            loaded[&key("wt-1", 3)].state,
            a_status(1_000).state,
            "a settled state was touched by the demotion"
        );
    }

    /// 판 번호 하나는 행을 지목하지 못한다 — 자리(열쇠 전체)만이 지목한다.
    ///
    /// 봉투 없는 소식(`clear_departed_agent`의 `foreground-returned`)이 고칠
    /// 행을 `key.ends_with("\0{term}")`로 찾던 시절의 계약을 못박는다. 번호는
    /// 재시작마다 다시 발급되므로(`main.rs`의 `next_term`), 그 꼬리는 살아
    /// 있는 판 하나가 아니라 **워크트리마다 하나씩**을 가리켰다: 새 판 7의
    /// 이탈이 지난 세션 판 7의 소식을 워크트리 수만큼 덮었다. 모듈 머리말이
    /// 열쇠 설계로 피했다고 적어 둔 그 위험이 실제로는 이 꼬리로 되돌아와
    /// 있었다.
    #[test]
    fn a_term_number_alone_does_not_name_a_row_but_a_seat_does() {
        let mut ledger = HashMap::new();
        ledger.insert(key("wt-a", 7), a_status(1_000));
        ledger.insert(key("wt-b", 7), a_status(1_000));
        ledger.insert(key("wt-a", 17), a_status(1_000));

        // 꼬리는 두 워크트리를 함께 문다 — 이것이 버그였다.
        let tail = format!("\u{0}{}", 7);
        let bitten: Vec<&String> = ledger.keys().filter(|key| key.ends_with(&tail)).collect();
        assert_eq!(bitten.len(), 2, "a term tail names one row per worktree");

        // 자리는 하나만 문다.
        assert!(ledger.contains_key(&key("wt-a", 7)));
        assert_eq!(
            ledger
                .keys()
                .filter(|held| *held == &key("wt-a", 7))
                .count(),
            1,
            "a seat names exactly one row"
        );

        // 그리고 꼬리가 물지 않는 것도 계약이다: 17은 7로 끝나지만 NUL이
        // 앞에 서 있어 7의 꼬리가 아니다 — 즉 이 결함은 한 워크트리 안의
        // 번호 혼동이 아니라 **워크트리를 가로지르는** 혼동이었다.
        assert!(
            !key("wt-a", 17).ends_with(&tail),
            "the NUL is what keeps 17 out of 7's tail"
        );
    }
}
