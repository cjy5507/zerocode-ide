# ZeroCode

**여러 코딩 에이전트를 한 창에서 지휘하는 컨트롤 룸.** Claude Code·Codex·zo 같은 에이전트 CLI를 각자의 git worktree와
터미널 판에서 동시에 굴리고, **지금 누가 사람을 필요로 하는지**를 한눈에 보여 주며, 원장(ledger)으로 일을 나누고 거둔다.
Electron이 아니라 Rust + Tauri로 만든 네이티브 앱이고, 같은 저장소에 자체 에이전트 CLI **`zo`**가 들어 있다.

- 제품 사양(현재 출하 상태의 정본): [`docs/PRODUCT.md`](docs/PRODUCT.md)
- 문서 지도: [`docs/README.md`](docs/README.md)
- 디자인 계약: [`docs/design/direction.md`](docs/design/direction.md) · 값의 원본은 [`ui/tokens.css`](ui/tokens.css)
- 배포·패키징: [`docs/release-packaging.md`](docs/release-packaging.md)
- `zo` CLI: [`zo-ide/README.md`](zo-ide/README.md)

## 무엇을 하는가

| 축 | 내용 |
|---|---|
| **에이전트 판** | 설치된 에이전트 CLI를 그대로 PTY 판에 띄운다(이식 없음). 훅 브리지가 상태(작업 중·확인 필요·완료)와 활동(도구·대상)을 창으로 올린다. 창을 닫았다 열면 판이 같은 대화로 되살아나고, 턴 중간에 끊긴 판은 이어서 일하게 넛지한다 |
| **워크트리** | 작업 하나 = worktree 하나. 사이드바에서 만들고 판을 그 안에서 연다. 착지한 워크트리는 걷는다 |
| **에이전트 보드** | 프로젝트 → 워크스페이스 → 에이전트 → 헬퍼의 라이브 그래프. 확인 필요 카드는 그 자리에서 답하거나 승인하고, 카드는 최근 도구를 낱말로 말한다. 메일·의존·병합 오버레이 |
| **오케스트레이션 원장** | `zerocode-orc`로 run·task·worker·gate·mail을 기록한다. 코디네이터 에이전트가 워커를 제 판에 소환하고(`worker-start --worktree`), 보고를 거두고(`worker_done`), 질문에 답한다(`ask`/`reply`) |
| **zo** | 자체 에이전트 CLI. Anthropic·OpenAI(ChatGPT/Codex)·Google 프로바이더, 창의 계정을 따라가는 OAuth, 서브에이전트(Agent·SpawnMultiAgent·Workflow·Task)와 팀메이트 판, `/goal`·`/loop` 자율 실행, 모델 카탈로그 자동 발견, 세컨드 브레인 회상 |
| **세컨드 브레인** | Obsidian 볼트를 지식 그래프로 그린다(타입 관계·허브·고아·유령 링크). 에이전트가 배운 것을 위키로 남기고 서로 잇는다 |
| **브라우저·에뮬레이터·컴퓨터** | 창 안의 브라우저(프로필·쿠키 가져오기), iOS·Android 에뮬레이터 미러, 데스크톱 Computer Use — 에이전트는 `zerocode-browser`·`zerocode-emulator`·`zerocode-computer`로 같은 표면을 쓴다 |
| **자동화** | 프롬프트·명령을 워크스페이스에 예약 실행하고 증거 폴더(steps.jsonl + 스크린샷)를 남긴다. 빠른 명령, 프로젝트 스크립트 |
| **작업·연동** | GitHub·GitLab·Jira·Linear 작업 페이지, SSH 원격 워크스페이스, 포트·localhost 라벨, 세션 보관함(다시 열기) |
| **설정** | 에이전트별 실행 인자·계정(Claude·Codex 다중 계정 전환)·훅·테마·단축키(충돌 검사)·언어(한국어·English·日本語·中文·Español) |

## 지원 에이전트

카탈로그(`crates/zerocode-core/src/agent.rs`)가 진실이다. 2026-09-05 기준 35종:
zo · claude · openclaude · codex · devin · ante · trae · autohand · opencode · mimo-code · pi · omp · prime-agent · antigravity ·
aider · goose · amp · kilo · kiro · crush · aug · cline · codebuff · command-code · continue · cursor · droid · kimi ·
mistral-vibe · qwen-code · rovo · hermes · openclaw · copilot · grok.
설치된 것만 판에 뜨고(`zerocode-orc agent-list`), 훅 스크립트는 에이전트 홈에 앱이 직접 쓴다.

zo는 프로세스마다 세션과 이벤트 채널을 열고, 대화는 파일에 영속한다. 창은 판 레이아웃과 세션 id를 저장한 뒤
새 프로세스의 `zo --resume <id>`와 새 채널로 복원한다. 창 종료 뒤 같은 프로세스의 실행이 계속된다는 보장은 없다
(`PtyLane::drop`은 자식 프로세스를 종료한다). `serve/attach` 데몬은 현재 zo의 실행 경로가 아니다
([세션 계약](docs/PRODUCT.md#44-세션계정워크트리)).

## 설치와 실행

- **릴리즈**: macOS는 서명·공증된 `.dmg`(`app`+`dmg`), Windows는 `.msi`·NSIS. 태그가 번들 버전과 같아야 릴리즈 잡이 돈다
  ([`docs/release-packaging.md`](docs/release-packaging.md)). 버전의 진실은 루트 `Cargo.toml`의 `[workspace.package] version`이고 `tools/release/bump.sh`가 세 파일을 함께 올린다(문서에 숫자를 박지 않는다); 번들 `dev.zerocode.app`.
- **개발**: Rust stable(1.94+), Node 25, Tauri CLI 2.11.

```bash
just shell        # 창을 띄운다(개발 빌드)
just verify       # fmt · clippy · rustdoc · 전체 테스트 · 창 clippy · 설정/창 브라우저 하네스
cd zo-ide && just verify   # zo CLI 게이트
just package-macos          # 서명 없는 로컬 번들(스모크)
CI=true npm run tauri:release -- --bundles app --ci --no-sign   # 앱 번들만
cd zo-ide && cargo build --release -p zo-ide   # zo 바이너리(PATH의 zo를 temp+mv로 교체)
```

실행 중인 앱 번들이나 `zo` 바이너리를 제자리에서 덮어쓰지 않는다(코드 서명 무효화로 SIGKILL). 새 번들을 옆에 두고
원자적으로 바꾼 뒤 사용자가 재시작한다.

## 저장소 구조

```
crates/
  zerocode-core          도메인 어휘 — 에이전트 카탈로그·훅 상태·판 키·계정·오케스트레이션 타입
  zerocode-shell         창(Tauri). main.rs + 도메인별 *_runtime.rs, cmd/ (명령), tests/source_contracts (소스 계약)
  zerocode-shell-state   창이 소유하는 상태
  zerocode-shell-cmd-jira / -linear   연동 명령
  zerocode-hookd         루프백 훅 브리지(에이전트 → 창)
  zerocode-harness       zo 이벤트 채널(JSON-RPC over loopback) 클라이언트
  zerocode-pty           PTY 호스트 + Rust가 소유하는 VT 그리드
  zerocode-orchestrator  원장(run·task·worker·gate·mail) 저장소와 규칙
  zerocode-lane          zo 판 실행 배선
  zerocode-ssh           검증된 SSH 연결
  zerocode-devproxy      localhost 워크트리 라벨 소켓
  zerocode-app           `zerocode` CLI 진입점
ui/                      창 화면(vanilla JS/CSS) · ui/tokens.css가 값의 원본 · ui/tests 브라우저 하네스
zo-ide/                  `zo` CLI 워크스페이스(runtime·api·tools·zo-ide)
skills/                  에이전트에게 설치되는 스킬(orchestration·computer-use·second-brain·design)
docs/                    사양·설계 브리프·분석·측정·결정 기록 — docs/README.md가 지도
```

## 검증

| 게이트 | 내용 |
|---|---|
| `just verify` | Rust 전체 테스트, 창 소스 계약(`tests/source_contracts`), clippy(`-D warnings`), rustdoc, 창·설정 브라우저 하네스 |
| `node ui/tests/window.mjs` | 창 하네스 850 케이스(보드·그래프·터미널·복원·성능 계약) |
| `node ui/tests/settings.mjs` | 설정 하네스 103 케이스 |
| `node ui/tests/test-knowledge-graph.mjs` | 지식 그래프 |
| `cd zo-ide && just verify` | zo 유닛·헤르메틱 e2e·TUI 바이트 골든·하네스 예산 |
| CI `verify.yml` | macOS·Windows 매트릭스, zo-ide, 서명 없는 패키지 스모크(설치·실행·재실행), 태그 릴리즈 |

부하 플레이크는 단독 재실행으로 판정하고, 게이트는 파이프 없이 종료 코드로 읽는다.

## 원칙

1. 막힌 판은 조용할 수 없다 — 확인 필요는 보드·사이드바·알림으로 승격된다.
2. 훅은 항상 성공하고 짧게 끝난다. 창 재시작 뒤에는 저장된 대화와 판을 복원한다(프로세스 생존 보장은 위 세션 계약 참고).
3. 권한 프롬프트를 코드가 대신 답하는 길은 없다.
4. 루프백 + 토큰이 유일한 기본이다.
5. 모르는 프레임·필드는 흘려보낸다 — 턴을 실패시키지 않는다.

에이전트가 이 저장소 안에서 다른 에이전트를 부를 때는 [`CLAUDE.md`](CLAUDE.md)/[`AGENTS.md`](AGENTS.md)의 원장 규칙을 따른다.
