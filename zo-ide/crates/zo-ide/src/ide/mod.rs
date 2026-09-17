//! IDE 전용 plain 프런트엔드 — 모듈 지도 (스캐폴드, 로직 없음).
//!
//! | 모듈 | 책임 | 포팅/참조 원재료 (port-staging/zo-cli/src) |
//! |---|---|---|
//! | [`args`] | CLI 인자 → `OpenOptions` + 렌더 옵션 | 신작 |
//! | [`input`] | stdin 단일 소유자·붙여넣기 봉투 누적 | 신작 (`zerocode-pty` `encode_paste` 계약) |
//! | [`run_loop`] | `select!` 상태기계(Idle/Turn/Prompt) | `acp_host.rs`(선례), `session/live_cli.rs:3184` 턴 드라이버 |
//! | [`render`] | `RenderBlock` → codex 동일 스타일 ANSI | `render.rs`, `tool_formatting.rs`, `session/ndjson_summary.rs:354` |
//! | [`prompt`] | 권한 y/a/n · 질문 숫자/자유입력 응답 | `session/permission_bridge.rs`, `session/user_question_bridge.rs` |
//! | [`reporter`] | `ZEROCODE_HOOK_*` → `POST /hook/zo` (claude 철자) | 신작 (계획서 Phase 2) |
//! | [`events`] | 대화형 자동/`--events-bind` TCP 구조화 채널(IDE 모달·Stop·상태) | `zerocode-harness`(정본 클라이언트), `port-staging/.../serve*.rs` |
//! | [`channel`] | 그 채널의 와이어·토큰·상태·소켓 | `port-staging/.../{serve_protocol,serve_auth,serve}.rs` |
//!
//! 금지 목록(이 트리에 절대 들어오면 안 되는 것): raw mode, alt screen,
//! 마우스 캡처, kitty 플래그, CSI 2J/2026, stderr dup2, ratatui 일체.

/// The prefix of the variables that carry the keys of the routers connected in
/// the window's settings. The window hands them to a zo launch alone, and this
/// process adopts them out of its environment first thing in `main`
/// (`api::adopt_launch_keys`), so nothing it spawns — an MCP server, a tool's
/// shell, a hook — inherits them. The window's spelling is
/// `zerocode_harness::ROUTER_KEY_ENV_PREFIX`; a test pins this one to it, the
/// harness being this crate's dev dependency only.
pub const ROUTER_KEY_ENV_PREFIX: &str = "ZEROCODE_ROUTER_";

/// Where the window keeps each router key: one keychain item per variable,
/// this prefix and the variable's name. A zo the window did not launch — one
/// typed into a shell pane — reads a key it was not handed from there, once,
/// into the same table (`api::find_router_keys_in_keychain`). The window's
/// spelling is `zerocode_harness::ROUTER_KEYCHAIN_SERVICE_PREFIX`; a test pins
/// this one to it.
pub const ROUTER_KEYCHAIN_SERVICE_PREFIX: &str = "dev.zerocode.router.";

/// Where the window's settings keep a service key under the key's own variable
/// name — TypeSafe's `TYPESAFE_API_KEY` — one keychain item per variable, this
/// prefix and the name. Every zo reads it from there when first needed
/// (`api::find_service_keys_in_keychain`). The window's spelling is
/// `zerocode_harness::SERVICE_KEYCHAIN_SERVICE_PREFIX`; a test pins this one to
/// it.
pub const SERVICE_KEYCHAIN_SERVICE_PREFIX: &str = "dev.zerocode.key.";

pub mod args;
pub mod channel;
pub mod events;
pub mod input;
pub mod prompt;
pub mod render;
pub mod reporter;
pub mod run_loop;
