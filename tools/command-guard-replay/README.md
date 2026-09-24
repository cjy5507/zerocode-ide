# command-guard-replay — 위험 명령·주입 글 거르기를 합성 사례로 재는 하네스 (t-6348)

두 조각이다. `seed.py`는 **모으기만** 하고 계산은 하지 않는다 — 탐지율·오탐률·확신도 구간·지연·요청 바이트·비용·하루 요청 수는
Rust 하네스 한 곳(`tools::smart_router::tool_guard::replay::the_guards_on_the_synthetic_cases`)이 씨앗을 읽어
제품의 질문·문·전선(`tool_guard::ask`)으로 한 건씩 물으며 센다(중복 금지).

| 조각 | 무엇 |
| --- | --- |
| `tools/command-guard-replay/seed.py` | 네 묶음(각 60건 이상)과 최근 7일 zo 전사 위치: 되돌릴 수 없는 명령·안전 명령(여기서 씀, 폴더는 `/Users/dev/work/zerocode`, 주소는 모두 `.invalid`), 주입 글(운반체 16 × 지시 16 조합), 평범한 글(이 저장소의 추적 파일 경로 — 하네스가 진짜 `read_file`로 읽고, 넷 중 하나는 창의 울타리로 감싼 브라우저 읽기로) |
| `zo-ide/crates/tools/src/misc_tools/smart_router/tool_guard/replay.rs` | 씨앗의 사례를 진짜 엔드포인트에 한 번씩 물어 표를 찍는 `#[ignore]` 실측. 요청 상한 300 |
| `tools/tests/test_command_guard_replay_seed.py` | 상한 상수가 core 표와 같은지, 묶음마다 60건 이상인지, 경로·주소에 사람의 것이 없는지, 씨앗이 계산하지 않는지 |

## 돌리기

```sh
python3 tools/command-guard-replay/seed.py --out "$SCRATCH/command-guard-seed.json"

TYPESAFE_API_KEY="$(security find-generic-password -s dev.zerocode.key.TYPESAFE_API_KEY -a "$(id -un)" -w)" \
ZO_TOOL_GUARD_REPLAY_SEED="$SCRATCH/command-guard-seed.json" \
ZO_TOOL_GUARD_REPLAY_OUT="$SCRATCH/command-guard-report.json" \
  cargo test -p tools --lib \
  misc_tools::smart_router::tool_guard::replay::the_guards_on_the_synthetic_cases \
  -- --ignored --nocapture
```

키는 그 한 명령의 환경에만 있다. 하네스는 임시 설정 홈(동의된 임시 워크스페이스 하나)을 세우고 사람의 설정·원장에는 쓰지 않는다.
씨앗에는 이 기계의 전사 위치가 들어가므로 스크래치패드에 둔다. 보고서에는 사례의 id·지문·수치만 실린다.

## 읽는 법

- **탐지율**은 걸러야 할 사례(되돌릴 수 없는 명령·주입 글) 중 가드가 `flagged`라고 답한 비율, **오탐률**은 걸러선 안 될 사례(안전 명령·평범한 글) 중 `flagged`의 비율이다. 둘 다 답이 온 사례만 센다(못 받은 답은 outcome 낱말로 따로).
- **오늘 규칙**은 같은 사례에 제품이 지금 가진 판별을 적용한 값이다: 명령은 zo의 위험·경로 표와 공유 트리 표, Computer Use `guarded` 낱말 표; 글은 「창이 이미 울타리로 감쌌나」.
- **제품에서 물었나**는 그 명령이 오늘 규칙으로 읽기 전용이 증명돼 아예 묻지 않는지(`runtime::tool_guard::command_of`)다.
- **하루 요청 수**는 최근 7일 전사에서 두 가드가 받았을 명령·블록 수를 날수로 나눈 값, **하루 비용**은 거기에 이번 실측의 요청당 평균 비용을 곱한 값이다.
- 주입 글의 지시는 머리 2,000자 안에 있다. 그 뒤에 놓인 지시는 가드가 보지 못한다(요청은 머리만 보낸다).
