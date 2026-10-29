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

`smart.plan.apply: true`는 비용 기반 턴 선택을 명시적으로 켠다(기본 `false`). 같은 프로젝트·역할·난이도·위험도와 정확한 모델·effort·실행 형태·검증 방식으로 비교하며, 최근 28일 안의 독립 과업 영수증이 현재 경로와 대안 각각 12개 이상 있어야 한다. 객관 검사 통과율은 최소 90%, 전체 관측 비용 절감은 최소 10%를 요구한다. 모델 변경의 문맥 재전송 비용도 더하며, 명시한 모델·effort는 변경하지 않는다.

현재 적용 가능한 형태는 `smart.policy: "classic"`에서 객관 검사 명령이 있는 단독 턴이다. Architect의 역할 분리 계약이나 명시적인 걸음 조절 정책을 덮어쓰지 않고 보류한다. 기록은 `smart-router/plan-receipts.jsonl`에 남으며, 부족한 표본·실패한 검사·손상된 기록·클라이언트 준비 실패는 기존 실행으로 돌아간다. 하위 작업 비용 미정산, 압축, 모델/effort 변경, 누락된 사용량, 혼합 캐시 TTL 등은 미상으로 남겨 자동 선택 근거에서 제외한다. 달러는 공개 단가로 계산한 API 환산값이며 구독 요금 청구액이 아니다. 적용 시 턴에 선택한 모델이 표시된다.

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

## 자동화 그래프

`Workflow`의 `mode: "graph"`는 `agent`·`check`·`branch`·`dream`·`end` 노드를 실행한다.
[검증·수리 예제](examples/workflows/verified-repair-graph.json)를 `WorkflowValidate`로 확인한 뒤
`Workflow`에 `{"spec": <예제>, "input": "작업 내용", "runId": "repair-1"}` 형태로 전달한다.
검증 명령은 해당 프로젝트의 명령으로 바꾸고, 실제 변경을 검증하는 조건을 사용한다.

- `agent`는 기존 `phases`의 에이전트를 실행한다. 단일 실행의 `{item}`에는 직전 노드 결과가 들어온다.
- `check`는 실제 명령 종료 코드로 성공·실패 경로를 고른다. `branch`는 지정한 노드 결과의 JSON pointer 값을 비교한다.
- 순환 경로는 `graph.max_steps`로 제한한다. 에이전트 수·출력 토큰·비용 예산도 재개 시 이어진다.
- `dream`은 마지막 작업 이후 통과한 검증 명령을 기존 Dreamer에 기록한다. 서로 다른 세션에서 반복·검증된 교훈만 기억으로 승격한다. 모델 재학습이나 자동 스킬 활성화는 하지 않는다.

같은 `runId`로 다시 호출하면 완료한 노드를 재실행하지 않는다. `resumeFromRunId`도 같은 경로이며,
그래프 재개는 원래 명세와 입력이 같아야 한다. 새 작업이나 다음 예약 실행에는 새 `runId`를 사용한다.
실행 중 프로세스가 죽어 결과가 불확실하면 `blocked`로 멈춘다. 외부 효과를 확인하기 전에는 새 실행을 만들지 않는다.
재개 후 dreaming에는 새 검증이 필요하며, 완료 기록 재생의 검증 표시는 과거 실행을 뜻한다.
창의 원장 워커를 쓰는 그래프는 전용 작업 워크트리에서 실행하고, 참조하는 phase에 `worktree: false`를 명시한다.
별도 원장 워커 워크트리의 자동 병합은 지원하지 않으므로 해당 조합은 실행 전에 거부한다.
네이티브 격리 실행은 `isolation: "worktree"`와 `apply: "sequential"`을 함께 써야 한다.

체크포인트는 기존 workflow 저장소의 `<runId>.graph.json`에 남고, `WorkflowRuns`로 단계 기록을 확인한다.

## 자동 시작과 예약

zo 세션이 켜져 있으면 기본 60초 유휴 뒤 실행 기록을 확인하고 dreaming을 시작한다.
실행 기록이 없으면 curation을 건너뛴다. 유휴 curation의 성공 간격은 6시간이며, 여러 세션과 압축·종료 훅은 같은 잠금과 성공 기록을 공유한다.
`autoDreamEnabled: false` 또는 `ZO_DREAM=0`으로 자동 dreaming을 끈다.
유휴 지연은 `autonomy.idleDreamDelaySecs` 또는 `ZO_AUTONOMY_IDLE_DREAM_DELAY_SECS`로 설정한다.

그래프를 반복하려면 `WorkflowLibrary`에 저장한 뒤 `CronCreate`에 한 번 등록한다.
예를 들어 저장된 `repair-check` 그래프를 30분마다 실행하는 등록 입력은 다음과 같다.

```json
{
  "schedule": "*/30 * * * *",
  "workflow": {"library": "repair-check", "input": "실패한 회귀 검사 수리"},
  "description": "회귀 검사와 수리"
}
```

`workflow`에는 `{"spec": <그래프 JSON>, "input": ...}`도 쓸 수 있다. 등록 시 라이브러리의 명세를 복사하므로 이후 라이브러리 수정이 예약을 바꾸지 않는다.
에이전트가 있는 예약에는 `budget.max_agents`와 `budget.max_output_tokens`가 필요하다.
현재 세션의 권한·도구 제한과 남은 예산을 적용하며, 부모 모델의 호출 없이 엔진이 직접 시작한다.
사용량 영수증이 없는 원장 워커는 예약 그래프에서 거부한다. 네이티브 워커 또는 검사만 있는 그래프를 사용한다.

주기마다 실행 ID를 새로 만들되, 같은 예약의 이전 실행이 끝나기 전에는 다른 세션도 다음 실행을 시작하지 않는다.
결과나 사용량이 불확실하면 공유 예약을 중지한다. `CronList`의 `workflow_active_at`과 `workflow_blocked_reason`으로 확인한다.
외부 효과를 확인한 뒤 `CronDelete`로 기존 예약을 지우고 다시 등록한다. 사용량 불확실성으로 멈춘 세션은 새 세션에서 다시 시작한다.
`CronRunDue`는 그래프를 프롬프트 작업으로 바꾸지 않으며, `CronList`·`CronDelete`는 기존 예약 관리 도구를 그대로 쓴다.
이 자동 실행은 켜져 있는 zo 세션의 유휴 엔진을 사용한다. 앱을 닫은 뒤 실행하는 OS 서비스는 설치하지 않는다.

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
