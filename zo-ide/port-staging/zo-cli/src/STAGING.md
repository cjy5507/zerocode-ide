# port-staging 안내 — 66파일, 컴파일 제외(참조·포팅 원재료)

## 포팅 대상 (터미널 비결합 — crates/zo-ide 로 옮기며 트림)
- session/live_cli*.rs · ndjson_summary.rs · runtime_builder/bridge · turn_harness
- session/permission_bridge.rs · socket_permission.rs · user_question_bridge.rs
- session/session_registry(루트)·restart·resume·session_preferences·startup_*
- sinks/ · render.rs · tool_formatting.rs · formatting.rs · util/
- runtime_support.rs · cli_tool_executor.rs · permission_mode.rs · workspace_trust.rs
- model_wire_env.rs · custom_provider_env.rs · conversation_support.rs · response_events.rs

## 참조 전용 (TUI/데몬 결합 — 옮기지 말고 배선 레시피만 읽을 것)
- session/turn_controller.rs — ChannelPrompter+권한 펌프 배선(:2221-2235), 권한 키맵(:3111-3133)
- session/tui_loop.rs — SessionStart/End payload(:73-86), build_hud_state(:2618-2740 → status 프레임 재료)
- serve.rs(rehydrate :608-644, run_turn_detached) · serve/pair.rs · serve_protocol.rs · serve_auth.rs — 세션 연속성 확장 문
- attach.rs — 플레인 라인 클라이언트 선례 · acp_host.rs — 130줄 프런트엔드 선례
- main.rs/main_dispatch.rs/cli_args.rs/lib.rs — 진입·플래그·전역(TUI_ACTIVE) 지도
- doctor.rs — 진단 서브커맨드 참고 · status_actions.rs — /status 리포트(주의: compat-harness 의존 → 포팅 시 트림, 우리 /status 는 모델·모드·ctx·비용 한 줄)
- input.rs — 죽은 rustyline 에디터(라인 편집 참고)

## 의도적 배제 (가져오지 않음 — 사유)
- tui/ 전체·attach_tui.rs: 디자인/재그리기 계층 — 신작 렌더러가 대체
- auth.rs(1.1k줄): /login 플로 — 계정은 zero-ide OAuth 를 따라감(어댑터는 api 크레이트에)
- self_update.rs·init.rs·command_reports.rs·workspace_reports.rs·tests.rs: 배포/프롬프트빌더/리포트 — keep-list 밖
- session/automation*·auto_fanout·confidence_cascade·grind_escalation·refine·self_improve·smart_settings·loop_arms·wakeups: 자동화 실험군 — v1 비활성 결정
- session/ide_bridge.rs: VS Code 확장 브리지 — zerocode 와 무관
- crates/acp·compat-harness·zo-bench: 별도 프로토콜/호환 하네스/벤치 — 필요 시 forge-code 에서 재가져오기
