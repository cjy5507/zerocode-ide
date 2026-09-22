# summon-replay — 소환 자리 일치율을 이미 일어난 행으로 재는 하네스 (t-5873)

두 조각이다. `seed.py` 는 **뽑기만** 하고, 계산은 하나도 하지 않는다 — 소환을 에이전트의
기록으로 접는 산술은 `zerocode_core::summon_choice::records` 한 곳에만 있고, 하네스가 씨앗의
행들을 그 함수에 그대로 넘긴다(중복 금지).

| 조각 | 무엇 |
| --- | --- |
| `tools/summon-replay/seed.py` | 원장 행(`~/.zo/jev/summon-choice.jsonl`)·권위 저장소(`authority.sqlite`)·`zerocode-orc agent-list` 를 읽어 씨앗 하나로 |
| `crates/zerocode-shell/src/orchestration/summon_choice/tests.rs` 의 `the_seats_agreement_over_the_rows_that_already_happened` | 씨앗을 실제 엔드포인트에 다시 물어 전/후 표를 찍는 `#[ignore]` 실측 |

## 돌리기

```sh
python3 tools/summon-replay/seed.py \
  --ledger ~/.zo/jev/summon-choice.jsonl \
  --store "$HOME/Library/Application Support/dev.zerocode.app/authority/authority.sqlite" \
  --out /tmp/summon-replay/seed.json

TYPESAFE_API_KEY=$(security find-generic-password \
    -s dev.zerocode.key.TYPESAFE_API_KEY -a "$USER" -w) \
ZEROCODE_SUMMON_REPLAY_SEED=/tmp/summon-replay/seed.json \
ZEROCODE_SUMMON_REPLAY_ARM=record \
ZEROCODE_SUMMON_REPLAY_GAUGE=unread \
ZEROCODE_SUMMON_REPLAY_RUNS=3 \
  cargo test -p zerocode-shell --bin zerocode-shell \
  orchestration::summon_choice::tests::the_seats_agreement_over_the_rows_that_already_happened \
  -- --ignored --nocapture
```

| 손잡이 | 무엇 |
| --- | --- |
| `ZEROCODE_SUMMON_REPLAY_ARM` | `room` = 선택지가 쿼터만 싣는다 · `record` = 원장의 기록까지 싣는다 |
| `ZEROCODE_SUMMON_REPLAY_GAUGE` | `seed` = 씨앗이 잡은 쿼터 · `unread` = 모든 선택지를 「읽은 게이지 없음」으로 고정 |
| `ZEROCODE_SUMMON_REPLAY_RUNS` | 행마다 몇 번 물을지(기본 3) |

## 이 하네스가 지키는 세 가지

1. **키는 환경에서.** 시험 빌드에서는 `accounts::security_command` 가 맵을 든 픽스처라
   `Wire::of_this_machine()` 이 없는 키체인을 읽는다. 위 명령줄이 진짜 키체인에서 꺼내
   건네고, 어떤 파일에도 로그에도 남지 않는다.
2. **사람의 원장·하루 셈을 건드리지 않는다.** 물음은 이 실측만의 임시 zo 홈으로 나간다.
3. **행보다 미래를 보지 않는다.** 선택지의 기록은 그 행의 시각 **이전** 소환만으로 접는다
   (`started_ms < row.at`). 이걸 안 하면 09-21 행이 09-22 의 과업 제목을 근거로 받는다 —
   같은 루브릭이 네 시간 차로 36/54 와 24/54 로 나왔던 이유다.

## 아직 못 하는 것

쿼터 게이지는 행이 기록하지 않았으므로 어느 팔도 그 행의 실제 게이지로 물을 수 없다. 하루
사이 `claude` 가 75% → 89% → 100% 로 움직이고, 100% 에서는 판정이 라벨인 바로 그
에이전트를 (옳게) 거절한다 — 그래서 팔 비교는 `GAUGE=unread` 로 고정해서만 읽는다.
t-5873 부터 행이 선택지의 게이지·기록을 직접 적으므로(`offered`), 한 주 뒤에는 충실한
재생이 가능하다.

## 시간 경계와 반복 표본 (t-5961)

씨앗은 SQLite backup으로 WAL까지 포함한 읽기 전용 스냅샷을 만든다. 각 행은 그 시각 이전에
시작한 소환과 배정만 고르고, 이후 완료는 `endedMs`/`succeeded`에 싣지 않는다. 재사용 워커의
미래 과업도 과거 제목을 덮지 않는다. attempts/failures는 행에 있으면 그 값을 쓰고, 없으면
당시 배정 이력으로 복원한다. 과거 과업 본문 수정이나 수동 실패 횟수 초기화는 저장소에 버전
이력이 없으므로 복원할 수 없다. 옛 전역 `carried` 씨앗은 다시 뽑아야 한다.

같은 행을 여러 번 물은 결과는 독립 표본이 아니다. 하네스는 pooled 원비율만 보고하고,
Wilson 하한은 한 행당 한 번씩 묻는 각 pass에만 출력한다. 이것도 다른 행 사이의 독립성을
보증하지 않는다. 기존 76/162 = 46.9%는 로그의 산술로 재현되지만, 그 씨앗 54행 중 39행이
미래 완료 정보를 보았으므로 누수 없는 개선 효과의 증거로 쓰지 않는다.
