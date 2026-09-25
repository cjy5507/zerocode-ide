# challenger-replay — 챌린저 자리의 원장을 제품의 함수로 다시 읽는 하네스 (t-6263)

두 조각이다. `seed.py` 는 **고르기만** 한다 — 추첨(`zerocode_core::jev::challenger::draws`)·눈가림(`Blind::over`)·쌍의 전적(`standing`)은
Rust 한 곳에만 있고, 재생이 씨앗의 행을 그 함수에 그대로 넘긴다(중복 금지). 파이썬에 규칙 사본이 생기면 재는 규칙과 나가는 규칙이 갈라진다.

| 조각 | 무엇 |
| --- | --- |
| `tools/challenger-replay/seed.py` | `~/.zo/projects/*/state/smart-router/challenger.jsonl` 의 행을 `--until` 시각까지만 씨앗 하나로 |
| `crates/zerocode-core/src/jev/challenger/tests.rs` 의 `the_challenger_rows_this_machine_wrote_replayed` | 씨앗의 행마다 키에서 추첨·눈가림을 다시 뽑아 행과 대조하고, (역할, 도전 모델)별 `compared·won·Wilson 하한·예약·실비`와 붙든 이유를 찍는 `#[ignore]` 실측 |
| `tools/tests/test_challenger_replay_seed.py` | 씨앗이 이 자리의 원장만, 그 시각까지만, 행 그대로 싣는지와 이름이 Rust 원본과 같은지 (`just tools-test`) |

## 돌리기

```sh
python3 tools/challenger-replay/seed.py --out /tmp/challenger-replay/seed.json

ZEROCODE_CHALLENGER_REPLAY_SEED=/tmp/challenger-replay/seed.json \
  cargo test -p zerocode-core --lib \
  jev::challenger::tests::the_challenger_rows_this_machine_wrote_replayed -- --ignored --nocapture
```

네트워크도 키도 쓰지 않는다. 원장은 읽기만 한다.

## 이 하네스가 지키는 네 가지

1. **재생 행은 제 시각까지의 원장만 본다.** `--until` 뒤에 쓰인 행(늦게 온 영수증의 라벨 포함)은 씨앗에 들지 않는다
   ([[a-replay-row-sees-only-what-the-ledger-knew-at-its-own-clock]]).
2. **추첨과 눈가림은 키에서 다시 뽑는다.** 행의 `attempt` 가 뽑히지 않는 키이거나, `blind` 낱말이 키에서 다시 뽑은 순서와 다르면
   `undrawn`·`blindMismatches` 로 센다 — 0이 아니면 행을 쓴 쪽과 판정하는 쪽이 다른 규칙을 읽은 것이다.
3. **숫자는 pooled 원비율과 Wilson 하한만.** 쌍마다 시도당 마지막 말 하나(요청 행의 비교를 라벨 행의 영수증이 덮는다)로 센
   `compared·won` 과 95% Wilson 하한. 합성 행에서 나온 승률은 모델의 승률이 아니다 — 계약 시험일 뿐이다.
4. **비용은 두 칸이다.** `expectedMicros`(뽑을 때 몫에 잡은 천장)와 `costMicros`(설계와 비교의 실비). 둘의 차가 정산이 돌려준 몫이다.

## 씨앗이 읽지 않는 것

글이 없다. 자리는 과업도 설계도 원장에 쓰지 않고 지문(`incumbentDesign`·`challengerDesign`)과 숫자만 쓴다 — 씨앗은 그 행을 그대로 옮긴다.
