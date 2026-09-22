# judgment-cache-replay — 판정 캐시 자리를 이 기계의 걷기 원장으로 재는 하네스 (t-6132)

두 조각이다. `seed.py` 는 **모으기만** 하고, 키는 하나도 다시 만들지 않는다 — 캐시의 키는
문이 통과시킨 바이트의 지문(`jev::memo::key_of`)이고 원장 행에는 그 바이트가 없다. 씨앗이
읽는 것은 화면 자리(browser·desktop·emulator)의 행마다 「질문의 모양」(자리, Flow 다이제스트,
errand, 후보 수, 앞선 누름 수, attempt, shows 줄 수)뿐이고, 같은 모양이 앞서 한 번 있었던
행의 몫이 곧 이 기계의 걷기에서 memo가 답할 수 있었던 상한이다.

| 조각 | 무엇 |
| --- | --- |
| `tools/judgment-cache-replay/seed.py` | `<config home>/jev/*-action.jsonl` 과 `<data root>/computer-use/sessions/*/*-action.jsonl` 에서 자리별 행 수·다른 모양 수·앞선 모양을 되풀이한 행 수·와이어 지연 p50/p95·센 요청 수를 씨앗 하나로 — 낱말은 없다(Flow 이름은 다이제스트) |
| `crates/zerocode-shell/src/computer_use/errand/live/tests.rs` 의 `the_second_walk_of_the_same_flow_sends_nothing` | 헤르메틱 실측: 가짜 응답기(250 ms 지연) 앞에서 같은 Flow(세 누름)를 두 번 — 첫 회 와이어 3회, 둘째 회 0회 — 와 두 회의 걸음 시간을 찍는다 (`just shell-test`) |
| `crates/zerocode-shell/src/computer_use/errand/live/tests.rs` 의 `what_the_memo_would_have_agreed_with_against_the_real_endpoint` | 실기: 실제 엔드포인트에 같은 화면 질문을 pass 수만큼 묻고, 첫 답과 뒤 답이 같은 번호를 골랐는지(= shadow 의 `agreed`)의 원비율과 Wilson 하한을 찍는 `#[ignore]` 실측 |
| `tools/tests/test_judgment_cache_replay_seed.py` | 씨앗이 판정 행만 세고 낱말을 싣지 않는지, 상수가 Rust 원본과 같은지 (`just tools-test`) |

## 돌리기

```sh
python3 tools/judgment-cache-replay/seed.py --out /tmp/judgment-cache/seed.json

# 실기 일치율 (키는 창 키체인 길, 원장은 임시 홈):
TYPESAFE_API_KEY=$(security find-generic-password \
    -s dev.zerocode.key.TYPESAFE_API_KEY -a "$USER" -w) \
ZEROCODE_JEV_BENCH_LOOK=/tmp/look.json ZEROCODE_JEV_BENCH_GOAL='저장' \
ZEROCODE_JEV_BENCH_KEY="$TYPESAFE_API_KEY" ZEROCODE_JEV_BENCH_PASSES=5 \
  cargo test -p zerocode-shell --bin zerocode-shell \
  errand::live::tests::what_the_memo_would_have_agreed_with_against_the_real_endpoint -- --ignored --nocapture
```

| 손잡이 | 무엇 |
| --- | --- |
| `ZEROCODE_JEV_BENCH_LOOK` | 표시된 보기의 JSON(`zerocode-browser marks <label> --json` 또는 `zerocode-computer observe --marks --json`) |
| `ZEROCODE_JEV_BENCH_GOAL` | 목표 문장 |
| `ZEROCODE_JEV_BENCH_PASSES` | 같은 질문을 몇 번 물을지(기본 5). 첫 답이 memo 가 기억할 답이고, 뒤 답마다 `agreed` 하나 |
| `--no-sessions` (seed) | 세션 폴더는 두고 config home 의 원장만 |

## 이 하네스가 지키는 세 가지

1. **키를 다시 만들지 않는다.** 키는 문 뒤의 바이트에서만 나온다. 씨앗은 모양을 세고, 모양이
   같다고 바이트가 같다고 말하지 않는다 — 그래서 되풀이 몫은 memo 적중률의 **상한**이다.
2. **낱말은 싣지 않는다.** Flow 이름은 SHA-256 앞 16자리, 화면의 글은 어느 칸에도 없다.
3. **숫자는 pooled 원비율과 pass별 Wilson 하한만.** 실기 일치율은 pass마다 한 번 물은 값에
   Wilson 하한을 붙이고, 헤르메틱 실측은 호출 수와 ms 를 그대로 찍는다.
