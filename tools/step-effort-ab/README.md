# step-effort-ab — 걸음 effort 조절기 A/B (t-5633 §6)

한 zo 세션(`--resume`으로 이어 감)에 과업 여섯을 교차 순서(A B B A A B, `--reverse`면 B A A B B A)로 돌리고,
턴마다 프로젝트의 `.zo/settings.json`으로 조절기의 낱말을 바꾼다 — A = 낱말 없음(표는 그림자로 돌고
행만 남김), B = `smart.zoStepEffort: "on"`. 읽는 원장은 셋:

| 열 | 어디서 | 무엇 |
| --- | --- | --- |
| `input`·`cache_rd`·`cache_wr`·`output`·`efforts` | `~/.zo/cache/prompt-cache/<session>/requests.jsonl` | 요청마다 토큰 넷과 요청이 실은 effort 단(`effort` 열) |
| `ttfb_ms` | `~/.zo/projects/<slug>/state/request-timings/timings.jsonl` | 요청이 떠난 뒤 첫 바이트까지(중앙값) |
| `deltas`·`reasons`·`applied`·`held` | `~/.zo/projects/<slug>/state/smart-router/step-effort-zo.jsonl` | 조절기의 걸음 행(`-` 한 단 아래 · `.` 그대로 · `+` 한 단 위 · `^` 두 단 위) |
| `rewrites` | requests.jsonl | 한 요청의 `cache_creation`이 10,000 토큰을 넘은 횟수 — 접두를 다시 쓴 요청 |
| `wall_s` | 하네스 | 턴의 벽시계 |

턴과 원장 행은 시각 창(`started_ms..ended_ms+2 s`)으로 잇는다 — attempt 서수는 resume 뒤 한 칸 밀린다
(`@2`가 없고 `@3`부터; 2026-09-21 실측).

## 돌리기

```sh
cd zo-ide && cargo build -p zo-ide --bin zo          # target/debug/zo
python3 tools/step-effort-ab/run.py run --zo zo-ide/target/debug/zo --model claude-opus-5 --out /tmp/ab-opus
python3 tools/step-effort-ab/run.py analyze --out /tmp/ab-opus
python3 tools/step-effort-ab/run.py run --zo zo-ide/target/debug/zo --model gpt-5.6-sol --out /tmp/ab-sol --reverse
python3 tools/step-effort-ab/run.py analyze --out /tmp/ab-sol
```

- 인증은 zo의 것을 그대로 쓴다: Anthropic은 Claude Code 세션 자격, Google은 `zo login google`. OpenAI는 창(ZeroCode
  window)에 로그인된 계정을 따라간다 — 하네스는 `CODEX_HOME` 없이 zo를 띄우지만 zo가 창의 관리 codex 홈을 스스로
  찾는다(t-5777). 다른 계정으로 재려면 그 홈을 `CODEX_HOME=<홈>`으로 직접 건네고, 창이 없는 기계에서는
  `zo login openai`가 남긴 제 로그인을 쓴다. 토큰이 만료되면 첫 턴이 5초 만에 `401 token_expired`로 끝나고
  `session: null`이 찍힌다 — 그 실행은 버린다. 어느 계정으로 말하는지는 `/status`의 출처 칸(IDE-managed ·
  environment · own login)이 답한다.
- 설정 홈을 바꾸려면 `ZO_CONFIG_HOME`(원장·캐시·자격이 모두 그 아래로 간다); 하네스도 같은 변수를 읽는다.
- `smart.jev.*` 동의가 없는 워크스페이스에서는 B팔의 Jev 물음이 `not_consented` 행으로 남고 표는 그대로 돈다.
- 조절기는 Anthropic 와이어에서 낱말이 `on`이어도 적용하지 않는다(행의 `held: anthropic_cache_prefix`) —
  `docs/design/zo-step-effort-governor-20260921.md` §6의 실측 때문이다. OpenAI·Google에서 켤지는 이 하네스의
  숫자로 판정한다(OpenAI 쪽 「토큰 만료」는 t-5777에서 창 계정을 따라가며 풀렸다).

## 과업 여섯

`run.py`의 `TASKS` — 작은 파이썬 패키지(`seed/`)에 함수·클래스 추가, 버그 고침, 리팩터, CLI 추가. 각 과업은
`python3 -m unittest -q`(검사 모양 명령)를 돌리게 되어 있어 읽기·편집·검사 걸음이 모두 나온다.
