# routing-replay — 모델 선택 판단(routing 2판)을 이 기계의 zo 전사로 재는 하네스 (t-6346)

두 조각이다. `seed.py` 는 **고르기만** 하고 계산은 하나도 하지 않는다. 한 턴이 무엇을 묻는지(사람의 첫 말, 키워드 표의 읽기,
채팅 프로브의 문턱)는 `smart_router::turn`, 그 턴이 무엇을 했는지는 `smart_router::route_label`, 질문과 답 읽기는
`runtime::model_router::decision` 에만 있고, 하네스가 씨앗의 전사를 그 함수들에 그대로 넘긴다(중복 금지). 어느 파일이 전사인지도
`tools/compaction-replay/seed.py` 의 읽기를 불러다 쓴다.

| 조각 | 무엇 |
| --- | --- |
| `tools/routing-replay/seed.py` | `~/.zo/projects/*/sessions/session-*.jsonl` 중 사람이 말한 턴이 있는 전사마다 경로·메시지 수·턴 수·스폰 수, 라우팅 원장(`decision-shadow.jsonl`)과 결과 원장(`route-outcomes.jsonl`)마다 행 수·프로브 답 수·시도 키 수 |
| `zo-ide/crates/tools/src/misc_tools/smart_router/routing_replay.rs` 의 `the_routing_seat_replayed_on_this_machines_turns` | 씨앗의 전사를 사람의 턴으로 나눠 두 판(1판·2판)을 실제 엔드포인트에 묻고, 턴이 한 일로 채점해 표를 찍는 `#[ignore]` 실측 |
| `tools/tests/test_routing_replay_seed.py` | 씨앗이 전사와 원장만 고르고 글을 싣지 않는지, 베낀 이름이 Rust 원본과 같은지 (`just tools-test`) |

씨앗의 턴 수는 크기 안내일 뿐이다. 재생은 전사를 `replay_support::history_as_it_stood` 로 그 시각의 모습으로 되살린 뒤
`runtime::patch_review::persons_turns` 로 나누므로, 표의 턴 수는 재생이 센 것이 정본이다.

## 돌리기

```sh
python3 tools/routing-replay/seed.py --days 30 --out /tmp/routing-replay/seed.json

cd zo-ide
TYPESAFE_API_KEY="$(security find-generic-password \
    -s dev.zerocode.key.TYPESAFE_API_KEY -a "$(id -un)" -w)" \
ZEROCODE_ROUTING_REPLAY_SEED=/tmp/routing-replay/seed.json \
ZEROCODE_ROUTING_REPLAY_OUT=/tmp/routing-replay/out.json \
  cargo test -p tools --lib smart_router::routing_replay -- --ignored --nocapture
```

키는 그 명령의 환경에만 둔다 — 파일·로그 어디에도 쓰지 않는다.

| 손잡이 | 무엇 |
| --- | --- |
| `ZEROCODE_ROUTING_REPLAY_SEED` | 씨앗 경로 (필수) |
| `ZEROCODE_ROUTING_REPLAY_LIMIT` | 물을 과업 수(기본 400). 프로브 답이 기록된 과업을 모두 먼저 넣고, 나머지는 고른 간격으로 |
| `ZEROCODE_ROUTING_REPLAY_OUT` | 표 전체와 과업마다의 숫자 행(수준·확신도·분포·사실 — 글 없음)을 이 파일에 |

지출 상한은 시험 안의 `REPLAY_SPEND_CAP_USD`(\$0.15, 두 판 합)다. 한 번에 여섯 과업(`IN_FLIGHT`)씩, 한 요청의 벽은 채팅 프로브의 것
(`PROBE_TIMEOUT`)이라 느린 답도 잘리지 않고 재진다 — 작동 벽(`ROUTING_APPLY_DEADLINE_MS`) 안의 몫은 따로 찍는다.

## 이 하네스가 지키는 네 가지

1. **씨앗에 글이 없다.** 경로와 개수뿐이다. 전사 본문은 Rust 쪽이 직접 읽고, 엔드포인트로 나가는 것은 자리 행(`ROUTING.sends`)이
   선언한 것뿐이다. 1판은 걸음 노력 자리(`ZO_STEP_EFFORT`)가 오늘도 보내는 평문 과업이라 그 행의 포인터로 문을 지난다.
2. **문은 임시 홈에 선다**(`JevDoor::at`). 사람의 원장·하루 셈은 움직이지 않는다.
3. **라벨은 턴이 한 일이다.** 턴의 도구 호출·편집한 파일·띄운 에이전트를 `route_label::observed_level` 이 수준으로 읽고,
   판단의 수준을 라우터가 읽었을 때 그 일에 맞는 등급을 골랐는지(`route_label::same_tier`)로 채점한다. 같은 표시로 키워드 표와
   「늘 같은 답」 판독기 넷을 함께 찍는다 — 판단이 이 기준선들을 넘지 못하면 신호가 없는 것이다.
4. **숫자는 pooled 비율과 Wilson 하한.** 같은 과업을 두 번 묻지 않는다.

## 찍는 것

| 칸 | 무엇 |
| --- | --- |
| `first` · `second` | 판마다 답한 수·지연 p50/p95·입력 토큰과 값·의도 「그 밖」 비율(2판은 열 칸의 원래 답과 라우터의 넷으로 접은 답 둘) |
| `second.bands` | 확신도 세 구간(행동·확인·기권)에 든 수 |
| `besideTheProbe` | 라우팅 원장에 기록된 채팅 프로브 답과 축마다 일치 — 1판(지금 물은 것·기록된 것)·2판·키워드 표 |
| `againstTheWork` | 턴이 한 일과 등급 일치(`sameTier`)·수준 일치(`exact`) — 1판·2판·키워드 표·`always`(늘 한 수준), 한국어 턴과 그 밖 |
| `probe` | 결과 원장의 프로브 기록(호출 수·실패 몫·p50/p95)과 100턴당 프로브 호출 — 오늘 기록·1판 작동·2판 작동 |
| `applied` · `turnStartDelay` | 판마다 작동했을 때 쓰인 몫과 턴 시작 지연의 모형: 판단의 실측 지연 + 프로브가 불렸을 턴에는 기록된 프로브 시간 |
| `routeOutcomes` | 결과 원장의 키워드 등급별 완료·실패·중단, 턴과 이어지는 행 수 |
| `curveAgainstTheWork` | 판마다 복잡도 확신도 다섯 구간별 등급 일치 — 구간 선을 읽는 곡선 |

## 우리 쪽 비용 — 턴 시작과 원장 상태 읽기

재생의 지연은 통신 왕복이다. 턴을 붙잡는 우리 쪽 몫은 `smart_router::roads_tests` 의 `#[ignore]` 측정 둘이 가짜 System One
(즉시 답함)으로 잰다 — 키도 엔드포인트도 쓰지 않는다.

```sh
cd zo-ide
cargo test -p tools --lib smart_router::roads_tests::what_each_word_adds_to_a_turns_start -- --ignored --nocapture
cargo test -p tools --lib smart_router::roads_tests::what_a_full_ledger_costs_the_seats_standing -- --ignored --nocapture
```

첫째는 `off`·`shadow`·`auto`·`on` 마다 200턴에서 `assess_turn_probed` 가 턴을 붙잡은 시간(p50·p95)을, 둘째는 2판 행을 원장 상한
(`SHADOW_LEDGER_MAX_BYTES`)까지 채운 원장에서 `auto` 가 턴마다 읽는 상태 읽기 시간을 — 모든 행을 푸는 읽기와 오르내림 줄만 푸는
읽기(`promote::stand_in`) 둘로 — 찍는다.

## 한계

- **스폰에는 라벨이 없다.** 스폰의 일은 자식 세션의 것이라 이 전사로는 채점하지 않는다(묻기와 지연·비용에는 들어간다).
- **프로브 답은 적다.** 라우팅 원장에 남은 프로브 답 중 전사의 턴과 지문이 맞는 것만 잇는다(30일에 65과업).
- **턴 시작 지연은 모형이다.** 프로브 시간은 결과 원장의 `route-tax` 행을 차례로 돌려 쓴다. 판단 쪽은 실제로 잰 왕복이다.
