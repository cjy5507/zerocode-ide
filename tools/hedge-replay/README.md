# hedge-replay — 이미 일어난 헤지 발동으로 헤지 규칙을 재는 하네스 (t-5874)

두 조각이다. `seed.py` 는 **뽑기만** 하고, 계산은 하나도 하지 않는다 — 헤지의 산술은
`zerocode_core::jev::hedge::plan` 한 곳에만 있고, 하네스가 씨앗의 표본을 그 함수에 그대로
넘긴다(중복 금지). 파이썬에 규칙 사본이 생기면 재는 규칙과 나가는 규칙이 갈라진다.

| 조각 | 무엇 |
| --- | --- |
| `tools/hedge-replay/seed.py` | 두 원장(`decision-shadow.jsonl`·`rerank-shadow.jsonl`)에서 `hedgeFired` 행마다 **그때 규칙이 읽은 표본**과 그 뒤 무슨 일이 있었는지를 씨앗 하나로 |
| `crates/zerocode-core/src/jev/hedge/tests.rs` 의 `the_firings_that_already_happened` | 씨앗을 `plan` 에 넘겨 전/후 표를 찍는 `#[ignore]` 실측 |
| `tools/tests/test_hedge_replay_seed.py` | 씨앗이 `Timed::sample` 과 같은 행을 고르는지 (`just tools-test`) |

## 돌리기

```sh
python3 tools/hedge-replay/seed.py --out /tmp/hedge-replay/seed.json

ZEROCODE_HEDGE_REPLAY_SEED=/tmp/hedge-replay/seed.json \
  cargo test -p zerocode-core --lib \
  jev::hedge::tests::the_firings_that_already_happened -- --ignored --nocapture
```

## 「전」 칸은 옛 규칙의 사본이 아니다

원장의 `hedgeDelayMs` 가 곧 옛 규칙의 출력이다 — 그 시각 그 표본에서 실제로 나간 지연.
그래서 전/후 표의 「전」은 기록이고 「후」만 계산이며, 옛 규칙을 어디에도 남겨 두지 않는다.

씨앗은 자기 읽기를 **스스로 검증**한다: 재구성한 표본에서 옛 공식
`min(p75, wall − p50)` 이 기록된 지연을 그대로 내야 하고, 하나라도 어긋나면 씨앗을
내보내지 않고 종료 코드 1 로 끝난다. 잘못된 행을 읽은 씨앗은 벽이 본 적 없는 분포를
규칙에 넘기는 것이고, 그 비교는 허구 둘의 비교다. 2026-09-22 이 기계에서 68/68 일치.

## 표본은 왜 그 행들만인가

`elapsedMs` 가 한 요청의 제 대기인 행은 보기보다 적다 — 타임아웃 행은 벽 그 자체이고,
메모가 답한 행은 0 이며, 재시도 행은 한 시도에 백오프가 붙은 값이고, 헤지가 붙은 행은
두 사본 중 빠른 쪽이다. 마지막 것을 남기면 헤지가 제 지연을 제가 만든 표본으로 끌어내린다.
`seed.py:one_requests_own_latency` 가 `jev_gate.rs:Timed::sample` 을 그대로 비추고,
위 테스트가 그 둘이 같은 말을 하는지 지킨다.

## 이 하네스가 읽지 않는 것

행은 지연·결과·셈만 든다. 과업의 글도 질의도 원장에 없고(지문뿐), 씨앗에도 없다.

표본 이력이 부족하거나 기록 지연이 없는 발동도 검증 분모에 남는다. 복원할 수 없으면
rc=1이며 씨앗을 내보내지 않는다. 같은 루트나 경로 별칭을 반복해 지정해도 파일은 한 번만 읽는다.
발동 승률은 race의 결과이고, 취소된 패자의 지연을 모르면 rescue율이나 두 사본의 독립성을
식별하지 못한다. `timeout` 14건에서 둘 다 실패한 것은 timeout의 정의여서 독립성의 반증이 아니다.
