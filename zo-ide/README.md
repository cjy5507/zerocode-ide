# zo — ZeroCode의 에이전트 CLI

`zo`는 ZeroCode 저장소 안의 독립 Cargo 워크스페이스(`zo-ide/`)에서 나오는 코딩 에이전트 CLI다. 창 없이도 완결된 CLI이고,
창 안에서는 PATH에서 감지되는 통합이다. 제품 사양 전체는 [`../docs/PRODUCT.md`](../docs/PRODUCT.md) §2.2·§3.4.

## 무엇을 하는가
- **프로바이더**: Anthropic(OAuth·API 키), OpenAI ChatGPT 백엔드(Codex 계정 — WebSocket·SSE, `previous_response_id` 이어 보내기),
  Google(Gemini Code Assist·Antigravity 레지스트리), OpenAI 호환 게이트웨이.
- **계정**: 창이 고른 계정을 `CLAUDE_CONFIG_DIR`/`CODEX_HOME`으로 받고, 전환은 채널 `auth.reload`로 값이 온다. `~/.codex`를 몰래 쓰지 않는다.
- **모델 카탈로그**: shipped 표 + 발견 계층(codex 캐시·Anthropic `/v1/models`·Google·Antigravity) → 가족 별칭(`fable`·`opus`·`sol`·`gemini-flash`…).
  모델의 사실(effort 목록·fast 티어·capabilities·가족·이름)은 카탈로그 행의 필드이고 코드는 id 문법과 프로바이더 기본값만 안다 — 새 세대
  (GPT-6 astra)는 발견되는 날 `openai-latest`를 넘겨받고 `astra`·`gpt-6` 별칭을 얻는다. `zo models [--refresh]`, 정책 `modelUpdatePolicy auto|notify|pinned`, TTL 6 h.
- **TUI**: codex 식 접기·리사이즈 재접기·인라인 팝업, Working 줄이 현재 도구·대상·경과와 헬퍼 파도, 15 s 뒤 모델 대기, 60 s 뒤 정적을 말한다.
- **서브에이전트**: `Agent`·`SpawnMultiAgent`·`Workflow`·`Task`. 인라인 헬퍼 또는 팀메이트 판(`--teammate <dir>`, 창이 판을 연다).
- **난이도 오케스트레이션**: 스마트 라우터가 매 턴을 판정하고, `Large`면 호스트가 먼저 최대 4갈래 읽기 전용 사전 분석을 병렬로 돌려 결과를 문맥에 앉힌 뒤 모델이 그 위에서 종합한다(`smart.orchestration auto|model|off`, 배너로 무엇을 하는지 먼저 말한다). Medium 이하는 모델이 이끈다 — `docs/design/zo-orchestration-by-difficulty.md`.
- **판단 Shadow**(기본 꺼짐): `smart.decisionShadow: "shadow"`이면 라우팅 probe가 읽는 과업마다 같은 규칙표의 typed 판단을 TypeSafe System One(`jev-latest`, 키 `TYPESAFE_API_KEY`)에 묻고
  probe 옆에 적는다 — **과업 글의 앞 2,000자가 `api.typesafe.ai`로 간다.** 라우팅은 바뀌지 않고, 턴은 기다리지 않는다. 원장은 지문만 든 `smart-router/decision-shadow.jsonl`,
  읽는 곳은 `zo --doctor`의 Jev 행과 `zo decision-shadow eval --labels <file.jsonl>` — `../docs/design/jev-decision-shadow-20260917.md`.
- **걸음 effort 조절기**: 한 턴 안의 요청마다 추론 노력을 표 하나가 정한다 — 직전 도구 배치가 읽기뿐이면 한 단 아래, 같은 호출 반복·도구 오류 연속·검사 빨강이면 한두 단 위,
  사람의 `--effort` 상한 안에서. 낱말이 없으면 표만 돌고 `smart-router/step-effort-zo.jsonl`에 적으며 아무것도 적용하지 않는다; `smart.stepEffort: "on"`이 적용, `"shadow"`/`"auto"`는
  엇갈린 걸음과 다섯 걸음마다 Jev에 한 번 묻는다(자리 `step_effort`) — `../docs/design/zo-step-effort-governor-20260921.md`.
- **관련성 컴팩션**(기본 꺼짐): `smart.jevCompaction: "shadow"|"on"|"auto"`이면 문맥이 차서 요약할 때 보존 꼬리 밖 도구 결과 블록마다 「남은 목표에 아직 필요한가」를 Jev에 병렬로 묻고(벽 하나 3 s, 못 온 답=keep),
  `on`·오른 `auto`는 `drop`(P≥0.7)을 요약 입력에서 뺀다 — 원문은 microcompact와 같은 볼트 봉인·복원 기계로 잃지 않는다. 원장 `smart-router/compaction-relevance.jsonl`, 라벨은 사후(다섯 턴 안 되읽기=후회, 자리 `compaction`); 재생 하네스 `tools/compaction-replay`.
- **의도 재순위**(기본 꺼짐): `smart.jevMentionRerank: "shadow"|"on"|"auto"`이면 작성창 `@` 팝업의 퍼지 첫 페이지(여덟 행)와 `/resume` 목록 첫 페이지를 두고 「쓰고 있는 문장이 어느 행을 뜻하는가」를 Jev에 비동기로 한 번 묻는다(자리 `mention_rerank`, 벽 1.5 s, 최신 물음 하나만). 퍼지 순서가 먼저 뜨고, `on`·오른 `auto`는 고르기가 아직 첫 행일 때만 답의 순서를 얹는다; 못 온 답·늦은 답·움직인 고르기=퍼지 그대로. 보내는 것은 문장 앞 1,000자·친 글자·행 이름 여덟·행이 보여 주는 머리 320 B(파일 본문 없음). 원장 `smart-router/mention-rerank.jsonl`, 라벨은 비교(고른 행==Jev 1위, 완성마다); 재생 하네스 `tools/mention-rerank-replay`.
- **자율 실행**: `/goal`, `/loop`, 헤드리스 `--loop-every/--loop-until/--loop-max`. 한도는 `autonomy/limits.rs` 표 하나, 429는 자동 재개.
- **고정 하네스**: 시스템 핵심·도구 스키마·스킬 색인·리마인더를 계열별 예산 게이트로 관리(`--prompt-input`). 드리머 리마인더는 wire 채널.
- **회상**: 본문 색인 + 세컨드 브레인 그래프 관계. 완료 영수증, 도구 습관 넛지, 긴 결과 다이제스트.
- **창 연동**: 훅 리포터(`POST /hook/zo`, claude 철자 이벤트)와 이벤트 채널(loopback JSON-RPC; `session.*`, `permission.respond`, `auth.reload`,
  `session_status`/`turn`/`subagents` 프레임). 계약은 [`docs/events-channel.md`](docs/events-channel.md), JSON 출력은 [`docs/json-contract.md`](docs/json-contract.md).

## 사용
```bash
zo                                   # 대화형 TUI (이벤트 채널 :0 자동)
zo --model astra --effort xhigh      # 별칭 또는 릴리즈 id (gpt-6-astra)
zo --resume                          # 최근 세션 목록 / --continue = 최신
printf 'prompt' | zo -p              # append-only 출력
printf 'prompt' | zo --json --last-message answer.md
zo --permission-mode read-only --status
zo models --refresh
zo decision-shadow eval --labels labels.jsonl   # probe·typed 판단을 사람의 라벨로 채점
```
슬래시: `/model /effort /fast /permissions /compact /status /new /new-project /resume /goal /loop /clear /help /exit`.
권한 모드 `read-only | workspace-write | danger-full-access`(Claude Code 별칭 수용), effort `off…max|ultra|smart`.

## 구조
```
zo-ide/
├── crates/
│   ├── core-types/ telemetry/ plugins/ decision-core/ codegraph/   # 잎
│   ├── api/            프로바이더·OAuth·클라이언트·모델 문맥 창
│   ├── runtime/        대화 런타임·설정·훅·권한·모델 발견(model_discovery)·스트리밍
│   ├── tools/          도구 레지스트리(spawn 패밀리·워크트리·워크플로 엔진·에이전트 명부)
│   ├── commands/       슬래시 카탈로그
│   ├── mock-anthropic-service/   시험용 목 서버
│   └── zo-ide/         ★ 바이너리 `zo`: session/(plain_session·subagent_progress·completion pump)
│                       tui/(app·view·painter·activity·strings) ide/(args·events·reporter·channel) autonomy/(limits·scheduler·runner·loops)
├── docs/               events-channel · json-contract · model-catalog-discovery · code-rules · testing · analysis/ briefs/ bench/
└── port-staging/       zo-cli 원재료(컴파일 제외, 참조 전용) — 세션 데몬 재료 보존
```

## 데이터
`~/.zo/settings.json`(model 가족 별칭·reasoningEffort·providers·mcpServers·workspaceTrust·autonomy 오버레이),
`~/.zo/model-catalog.json`(모델 카탈로그 오버레이 — settings 의 `modelCatalog` 키는 파일이 없을 때만 읽는 레거시),
`~/.zo/projects/<slug>/sessions/*.jsonl`(턴마다 영속), `~/.zo/projects/<slug>/state/agents/`(헬퍼 명부 — 세션 소유 registry + 채택 미러, t-2511 착지),
`~/.zo/cache/`(모델 카탈로그·프롬프트 캐시 원장), `~/.zo/logs/zo-ide.log`.

## 검증과 배포
```bash
cd zo-ide && just verify                    # fmt · clippy · 유닛 · 헤르메틱 e2e · 골든 · 하네스 예산
cargo test -p zo-ide --test tui_bytes       # TUI 바이트 골든 — COLORTERM 유무 둘 다 초록이어야 한다
cargo test -p zo-ide --test e2e_hermetic -- --test-threads=1
cargo build --release -p zo-ide && cp target/release/zo ~/.local/bin/zo.new && mv -f ~/.local/bin/zo.new ~/.local/bin/zo
```
실행 중인 `zo`를 제자리에서 덮어쓰지 않는다(코드 서명 무효화). 헤르메틱 시험은 `ZO_CLAUDE_HOME`/`ZO_CODEX_HOME`을 빈 값으로 핀한다.

## 규율
- ratatui/crossterm raw-mode 계열 의존을 추가하지 않는다(자체 painter).
- 숫자는 한 표에(`autonomy/limits.rs`, 하네스 예산), 문안은 `tui/strings.rs`에 한 번.
- 훅 길(`PreToolUse`)은 도구 시작 때 한 번만·프롬프트 파킹 중 침묵; 채널 `session_status.activity`가 매 초를 맡는다.
- 새 `RenderBlock`·프레임 변종은 소비자 다섯(TUI·plain·json·채널·리포터)을 함께 고친다.

## 기원
forge-code(zo)의 코어 크레이트(core-types·telemetry·plugins·decision-core·codegraph·api·runtime·tools·commands·mock)를 가져와
IDE 전용 프런트엔드 `crates/zo-ide`를 새로 썼다. TUI·attach/serve·remote control·self-improve는 가져오지 않았고, 데몬 기반
세션 연속성 재료는 `port-staging/`에 참조용으로만 남겼다(현재 연속성은 파일 영속 + `--resume`).
