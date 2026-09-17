# 에이전트 축 실측 하네스 (zo ↔ Claude Code)

`docs/design/zo-agent-parity-vs-cc.md` §2 의 축을 재는 자. 하나의 시나리오를 두 제품에
같은 모양으로 물려서, 판정이 구호가 아니라 같은 자로 잰 수가 되게 한다.
기록은 `docs/analysis/zo-agent-parity-<날짜>.md` 의 표 한 장이다.

## 레인

- **결정론(기본)** — 두 CLI 를 같은 스크립트 Anthropic 서비스(`mock.py`)에
  `ANTHROPIC_BASE_URL` 로 물리고, `zo-ide/crates/zo-ide/tests/e2e/harness.rs` 의
  `configure_command` 와 같은 헤르메틱 환경(사설 `HOME`·세션·상태 트리, 더미 키,
  키체인·외부 자격 차단)에서 pty 로 띄운다. **프로바이더 지출 0.**
- **판(`--panes`)** — 서브에이전트를 설치된 ZeroCode 창의 진짜 판으로 자른다. 창의 `tmux`
  심이 이벤트 채널이므로 팀 좌표 넷(`TMUX`·`TMUX_PANE`·팀 id·토큰)은 지우지 않고 통과시키고,
  판 자식은 부모의 환경을 물려받지 않으므로(창이 판마다 제 환경을 준다) `tmux` 앞의 래퍼가
  `split-window … -- <program>` 을 `… -- /usr/bin/env <이 실행의 변수> <program>` 으로
  고쳐 쓴다. 그 래퍼는 실행마다 `runs/<run>/bin/tmux` 로 생성된다.

## 한 곳씩

| 파일 | 하는 일 |
|---|---|
| `mock.py` | 스크립트 Anthropic 서비스. 요청마다 도착·응답 시각·도구 목록·원문을 `runs/<run>/requests.jsonl` 에 적는다. 프롬프트가 든 표식(`PARITYAXIS:`·`PARITYCHILD:`)으로 축과 자식을 가른다 |
| `scripts.py` | 축마다 한 스크립트. **캐논 의도**만 적는다 |
| `adapt.py` | 캐논 의도 → 각 제품의 도구 이름·인자 모양. 두 제품의 어휘가 나오는 **유일한** 자리 |
| `drive.py` | 한 주체 × 한 축을 pty 로 띄우고, 마지막 표식 뒤 `--settle` 만큼 기다린 뒤 거둔다. `--panes` 가 판 레인 |
| `analyze.py` | 실행 하나 → 축 지표. 축마다 함수 하나 |
| `panes_on_disk.py` | 판 자식을 디스크에서 읽는 한 곳(브리핑·`result.json`·`result-final.json`). 스크립트의 스냅샷과 분석기가 같은 질문을 한 번만 적는다 |
| `table.py` | 지표 → 문서의 표 셀. 문서의 수를 손으로 옮겨 적지 않기 위해 |
| `summarise.py` | 여러 실행의 min/중앙/max |
| `emit_runs.py` | `fixtures/runs.jsonl` — 한 행 = 한 실행, 필드는 `docs/design/agent-workflow-quality-baseline.md` §2 |
| `spawn_window.py` | `Agent` 플러시와 자식 첫 요청 사이에 각 제품이 만진 파일(축 A 의 메커니즘) |
| `wave2.sh`·`wave3.sh`·`pane-wave.sh`·`wave4.sh` | 그 날 파도의 호출표. 축마다 프롬프트·타임아웃·중단 지점이 다르므로(축 K·H 는 제 프롬프트, G 는 중단+후속, L 은 `/loop` 슬래시) 그 표가 한 곳에 있어야 다음 파도가 같은 자를 쓴다. 09-07 에 이것을 한 번 틀려서 L·H·G 를 다시 돌렸다 |
| `cells.sh` | 문서의 표 순서 그대로 모든 셀을 한 번에 |

## 다시 돌리기

```sh
R=/tmp/zo-agent-parity-$(date +%Y%m%d)
mkdir -p $R && cp -R tools/agent-parity/* $R/     # harness/ 와 fixtures/
export PARITY_ROOT=$R
export PARITY_ZO_BIN=<이 체크아웃의 릴리즈 zo>
for s in zo claude; do (cd $R && git init -q repo-$s && ...) done   # 축 I 의 리포
bash $R/harness/wave2.sh                          # 축 K..L, 두 주체
PARITY_H_CRON_TZ=utc bash $R/harness/wave3.sh      # H₁ 을 zo 의 시계로 격리
bash $R/harness/pane-wave.sh                      # 판 레인 (창 안에서만 됨)
```

축 E 는 이 하네스가 아니라 리포의 헤르메틱 e2e 다:
`cd zo-ide && cargo test -p zo-ide --test e2e_hermetic teammate`.
