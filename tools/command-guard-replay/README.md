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
- **오늘 규칙**은 같은 사례에 제품이 지금 가진 판별을 적용한 값이다: 명령은 zo의 위험·경로 표와 공유 트리 표, Computer Use `guarded` 낱말 표; 글은 「호스트가 이미 울타리로 감쌌나」를 **호스트 자신의 말**(`runtime::tool_guard::HostFraming`)로 읽는다 — 런타임 자기 도구(file·web·mcp)의 답은 맨몸(`unfenced`, 규칙=plain), 창의 표식을 실은 셸 답은 바이트가 증언하지 못하므로 `unknown`(규칙 없음, `notEvaluable`로 따로 셈). 라벨 writer와 이 하네스는 같은 한 함수 `tool_guard::todays_text_rule`을 읽는다(t-7058). 각 reading에는 `framing` 낱말이 실린다.
- **제품에서 물었나**는 그 명령이 오늘 규칙으로 읽기 전용이 증명돼 아예 묻지 않는지(`runtime::tool_guard::command_of`)다.
- **하루 요청 수**는 최근 7일 전사에서 두 가드가 받았을 명령·블록 수를 날수로 나눈 값, **하루 비용**은 거기에 이번 실측의 요청당 평균 비용을 곱한 값이다.
- 주입 글의 지시는 머리 2,000자 안에 있다. 그 뒤에 놓인 지시는 가드가 보지 못한다(요청은 머리만 보낸다).

## 저장된 readings 위에서 기준선 다시 세기 (t-7058, 새 호출 0)

글 가드의 기준선은 루브릭 버전마다 뜻이 달랐다 — v1은 사례 바이트 속 문구(하네스가 브라우저 사례에 직접 두른 울타리), v2는 모든 사례 plain 상수, v3는 호스트의 말(브라우저 사례는 채점 제외). 옛 실측의 「오늘 규칙」 수치를 새 SHA에 그대로 쓰면 안 되므로, 같은 고정 readings(모델 답 그대로) 위에서 세 규칙을 나란히 다시 센다. 요청은 하나도 나가지 않는다.

```sh
ZO_TOOL_GUARD_REPLAY_SEED="$SCRATCH/command-guard-seed.json" \
ZO_TOOL_GUARD_REPLAY_REPORT="$SCRATCH/command-guard-report.json" \
ZO_TOOL_GUARD_BASELINE_OUT="$SCRATCH/command-guard-baselines.json" \
  cargo test -p tools --lib \
  misc_tools::smart_router::tool_guard::replay::the_text_baselines_on_the_saved_readings \
  -- --ignored --nocapture
```

씨앗의 사례는 제품이 짓는 그대로(`replay::cases` → `text_ask`) 다시 지어 저장된 reading과 id로 맞춘다 — 저장된 readings에는 호스트의 말이 없으므로 v3는 사례의 도구 종류에서 이 런타임의 seam이 정하는 낱말을 읽고, 본문에서 새 사실을 짓지 않는다. 보고서에서는 저장된 판정·`rule`(v1)·outcome·가려진 줄만 쓴다. 세 규칙은 재생 보고서와 같은 한 셈(`todays_rule_tally`, 답이 온 사례만 분모, 채점 못 하는 사례는 `notEvaluable`로 따로)으로 세고, v1 재산출이 보고서의 「오늘 규칙」과 사례 하나까지 같아야 통과한다(같은 행·같은 분모의 확인). JSON에는 씨앗 경로 지문(보고서가 씨앗을 부르는 방식)과 일치 여부, 씨앗·보고서 본문 지문, 보고서의 커밋(보고서에 칸이 없으면 `null` — 짐작하지 않는다), 보고서·지금의 루브릭 버전, 규칙·라벨 정의, 묶음별 사례·답·미답(outcome별)·가려진 줄이 있던 수·가드 flagged 수, 오늘 제품의 머리 지문과 같은 reading 수, 저장된 v1 규칙과 하네스 자신의 울타리가 어긋난 사례 id가 실린다. 가드 자체의 탐지율·오탐률은 저장된 판정 그대로이며 바뀌지 않는다 — 움직이는 것은 규칙 열뿐이고, 기준선의 숫자가 바뀐 것을 가드가 나아진 것으로 읽지 않는다.
