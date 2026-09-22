# branching-replay — 갈라지는 미래 자리를 재는 하네스 (t-6044)

두 조각이다. `seed.py` 는 **모으기만** 하고, 계산은 하나도 하지 않는다 — 어느 컨트롤이 후보인지는
`zerocode_core::branching::top_k`, 질문은 `branching::ask`, 라벨의 표식은 `branching::agreed` 한 곳에만
있고, 하네스가 씨앗의 행을 그 함수들에 그대로 넘긴다(중복 금지). 파이썬에 규칙 사본이 생기면 재는 규칙과
나가는 규칙이 갈라진다.

| 조각 | 무엇 |
| --- | --- |
| `tools/branching-replay/seed.py` | 창의 증거 폴더(`~/Library/Application Support/dev.zerocode.app/computer-use/sessions/*/emulator-action.jsonl` + `steps.jsonl`)와 에뮬레이터 자리의 한 원장에서, 휴대폰 걷기의 판정이 누른 걸음마다 — 그 순간 자리가 답한 확률 분포·고른 번호·플랫폼·기기·그 걸음의 보기/누르기 ms — 와 라벨(자리의 `agreed`)을 씨앗 하나로 |
| `crates/zerocode-shell/src/computer_use/errand/branch/tests.rs` 의 `the_forks_this_desk_would_take` | 가짜 desk 시나리오 여덟 개(목표·이전 화면·후보 둘~셋과 각각이 이끈 화면)를 배포되는 질문 그대로 실제 엔드포인트에 묻고, 시나리오의 정답·오늘의 누름(첫 후보)과 비교해 **합성 골든의** 정답률(pass별)·지연·비용을 찍는 `#[ignore]` 측정. 씨앗을 주면 이 기계의 걸음 중 몇이 갈라졌을지도 센다 |
| `crates/zerocode-shell/src/computer_use/errand/branch/tests.rs` 의 `measure_forked_steps_on_a_fake_desk` | 가짜 desk(이 기계가 기록한 보기 1,386 ms·누르기 1,420 ms, AVD 스냅샷 1,700 ms)에서 단일 걸음 대 k=2 분기 걸음의 시간 배수, 스크립트된 세계의 구조 걸음 수(스크립트의 값이지 주장이 아님), 그리고 마진 게이트 아래 1위/2위 격자(0.30~0.95) 중 갈리는 걸음 비율을 찍는 헤르메틱 시험(`just shell-test`) |
| `tools/tests/test_branching_replay_seed.py` | 씨앗이 답한 걸음만 싣고, 규칙을 담지 않고, 상수가 Rust 원본과 같은지 (`just tools-test`) |

## 돌리기

```sh
python3 tools/branching-replay/seed.py --out /tmp/branching-replay/seed.json

TYPESAFE_API_KEY=$(security find-generic-password \
    -s dev.zerocode.key.TYPESAFE_API_KEY -a "$USER" -w) \
ZEROCODE_BRANCHING_REPLAY_SEED=/tmp/branching-replay/seed.json \
ZEROCODE_BRANCHING_REPLAY_RUNS=2 \
  cargo test -p zerocode-shell --bin zerocode-shell \
  computer_use::errand::branch::tests::the_forks_this_desk_would_take -- --ignored --nocapture
```

| 손잡이 | 무엇 |
| --- | --- |
| `ZEROCODE_BRANCHING_REPLAY_RUNS` | 시나리오마다 몇 번 물을지(기본 1). 둘째 pass부터는 근거가 아니라 같은 물음에 같은 답을 하는지의 반복성이다 |
| `ZEROCODE_BRANCHING_REPLAY_SEED` | 씨앗(선택). 주면 이 기계의 걸음 중 몇이 갈라졌을지와 보기/누르기 p50을 함께 찍는다 |

## 언제 갈리는가

확률을 받은 후보가 둘 이상이면 갈리던 것은 t-6155 F3에서 닫혔다. 이제 `branching::fork_wanted` 는
자리의 1위가 2위를 `BRANCHING_FORK_MARGIN_PERMILLE`(200‰) 미만으로 앞서거나, 1위 자체가 press 바닥
(`SCREEN_PRESS_FLOOR_PERMILLE`) 아래일 때만 k 후보를 낸다 — 뚜렷한 우세는 걸음 하나다. 값은 `jev.rs`
의 상수 한 곳에 있고, 가짜 desk의 1위/2위 격자 14칸 중 6칸이 갈린다(게이트 전 14/14).

## 왜 씨앗의 행은 묻지 않는가

증거 폴더는 걸음의 **낱말**(argv·관측 ms)과 자리의 **답**(확률·고른 번호)을 남기지, 화면이 보여 준
컨트롤의 범례 줄이나 누른 뒤의 화면은 남기지 않는다. 그래서 씨앗의 행으로는 분기 질문을 다시 지을 수 없고,
하네스는 씨앗을 「갈라졌을 걸음 수」와 「걸음의 시간」을 세는 데만 쓴다. 질문의 정답률·지연·비용은 가짜 desk의
시나리오로 잰다 — 후보의 결과 화면이 시나리오에 적혀 있으므로 정답이 있고, 그 정답률은 **시나리오의**
정답률이지 이 기계의 걷기에서 잰 것이 아니다. 정답 후보가 이끈 화면의 범례에는 목표의 낱말이 없다
(`no_scenario_leaks_its_goal_into_the_right_candidates_result`): 목표를 문자열로 맞추는 답은 골든이 아니다. 실제 걷기의 표식은 자리의 원장(`branching.jsonl`)에 걸음마다
쌓이는 `agreed`이고, 승격은 그것으로만 오른다.

## 이 하네스가 지키는 세 가지

1. **행보다 미래를 보지 않는다.** 씨앗은 걸음마다 그 시각의 사실만 싣고, 라벨(자리의 `agreed`)은 답을
   받은 **뒤에만** 읽는다. 시나리오의 정답은 질문에 실리지 않는다.
2. **키는 환경에서, 원장은 아무 데도.** 문은 임시 홈에 서고(`Wire::at`), 사람의 `branching.jsonl`·하루 셈은
   움직이지 않는다.
3. **숫자는 pass별 Wilson 하한.** 정답률은 pass마다 따로 세고 첫 pass에만 Wilson 하한을 붙인다(둘째부터는
   `repeated`). pass를 합친 n 위의 하한은 표본 부풀림이다(t-6155 F2: 16/16→80.6%는 8/8→67.6%였다).

## 실기 Android

기기가 있을 때(`adb get-state`가 `device`를 답할 때)만 스냅샷 save/load 왕복 ms를 잰다:
`ZEROCODE_LIVE_ANDROID_SERIAL=emulator-5554 cargo test -p zerocode-shell --bin zerocode-shell
a_live_avds_snapshot_round_trip -- --ignored --nocapture`. 기기가 없으면 표는 「미실측」이고, 추정하지 않는다.
