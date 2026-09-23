//! `zo-ide` 라이브러리 표면.
//!
//! 바이너리(`src/main.rs`)가 얇게 얹히는 층이다. zo-cli 와 같은 lib/bin 분할:
//! 포팅된 코어 모듈은 여기서 공개되고, 통합 테스트와 미래의 임베더가
//! 바이너리를 거치지 않고 직접 닿는다.
//!
//! 포팅 진행 상태 (원본: `../../port-staging/zo-cli/src/STAGING.md`):
//! - [`util`] · [`sinks`] · [`render`] · [`tool_formatting`] — 렌더/와이어 (1차)
//! - [`support`] · [`cli_args`] · [`effort`] — zo-cli 루트 헬퍼·타입의 새 집
//! - [`runtime_support`] · [`cli_tool_executor`] · [`session`] — 엔진 빌드·턴 구동
//! - [`session_registry`] · [`permission_mode`] · [`workspace_trust`] — 영속·권한
//! - [`ide`] — IDE 전용 프런트엔드 (계약 스텁 — Phase 1 본체)
//!
//! 아래 `pub use` 들은 zo-cli `main.rs` 가 크레이트 루트에 두던 이름을 그대로
//! 재현한다 — 포팅된 파일들이 `crate::current_cli_cwd()` 식으로 부르기 때문.

// 포팅 브리지: 엔진 슬라이스는 착륙했지만 소비자(`session::plain_session` ·
// `ide::run_loop`)가 아직 없어 dead_code/unused_imports 가 정직하게 울린다.
// plain_session 이 배선되면 이 두 allow 는 제거한다 — 그 전까지 게이트 소음을 막는 임시 조치.

pub mod cli_args;
pub mod autonomy;
pub mod cli_tool_executor;
pub mod conversation_support;
pub mod cron_cli;
pub mod decision_shadow_cli;
pub mod jev_cli;
pub mod mcp_cli;
pub mod scoreboard_cli;
pub mod vault_cli;
pub mod custom_provider_env;
pub mod doctor;
pub mod dream;
pub mod effort;
pub mod goal;
pub mod ide;
pub mod launch_contract;
pub mod model_wire_env;
pub mod build_info;
pub mod models_cli;
pub mod permission_mode;
pub mod preferences;
pub mod prompt_input;
pub mod remote_mcp;
pub mod render;
pub mod resume;
pub mod response_events;
pub mod runtime_support;
pub mod session;
pub mod session_format;
pub mod session_registry;
pub mod slash;
pub mod sinks;
pub mod status_format;
pub mod support;
pub mod teammate;
pub mod tool_formatting;
pub mod tui;
pub mod usage;
pub mod util;
pub mod workspace_trust;

pub(crate) use cli_args::{AllowedToolSet, DisallowedToolSet};
pub(crate) use cli_tool_executor::CliToolExecutor;
pub(crate) use conversation_support::mark_conversation_cache_breakpoints;
pub(crate) use runtime_support::AnthropicRuntimeClient;
pub(crate) use session::{BuiltRuntime, RuntimePluginState};
pub(crate) use session_format::{
    format_missing_session_reference, format_no_managed_sessions,
};
pub(crate) use session_registry::write_atomic;
pub(crate) use support::{
    build_plugin_manager, current_cli_cwd, default_prompt_date, filter_tool_specs,
    max_tokens_for_model, tui_active, DEFAULT_MODEL, LATEST_SESSION_REFERENCE,
    LEGACY_SESSION_EXTENSION, PRIMARY_SESSION_EXTENSION, SESSION_REFERENCE_ALIASES,
};
pub(crate) use tool_formatting::format_tool_result;

pub use doctor::{diagnose as diagnose_environment, DoctorReport, Finding as DoctorFinding};
pub use preferences::{load as load_preferences, preferences_path, save as save_preferences};
pub use resume::{
    continue_session_id, latest_session, list_recent_sessions, list_recent_sessions_limited,
    render_recent_sessions, ResumeSession,
};
pub use status_format::{
    context_usage_percent, estimated_cost_usd, format_context_usage, format_cost_usd,
    format_estimated_cost, format_tokens, format_usage_status, usage_context_tokens,
    local_offset_seconds,
};

#[cfg(test)]
pub(crate) use support::{test_cwd_lock, test_env_lock, SessionRootPin};

/// The binary's first act is to adopt the router keys the window handed it:
/// done any later, a thread may already be reading the environment, and every
/// child spawned before it (an MCP server, a tool's shell) inherits the keys.
#[cfg(test)]
mod main_contract {
    #[test]
    fn main_adopts_the_windows_router_keys_before_anything_else() {
        let main = include_str!("main.rs");
        let body = &main[main.find("fn main() -> ExitCode {").expect("main")..];
        let adopt = body
            .find("api::adopt_launch_keys(zo_ide::ide::ROUTER_KEY_ENV_PREFIX)")
            .expect("main adopts the window's router keys");
        let first_other = body
            .find("runtime::relocate_traces_out_of_tree()")
            .expect("main's first act otherwise");
        assert!(adopt < first_other, "the keys are adopted before anything else runs");
    }

    /// One spelling on both sides of the handoff: the window's is the harness
    /// crate's (the wire's canonical), this binary's mirrors it.
    #[test]
    fn the_router_key_prefix_is_the_windows() {
        assert_eq!(
            crate::ide::ROUTER_KEY_ENV_PREFIX,
            zerocode_harness::ROUTER_KEY_ENV_PREFIX
        );
        assert_eq!(
            crate::ide::ROUTER_KEYCHAIN_SERVICE_PREFIX,
            zerocode_harness::ROUTER_KEYCHAIN_SERVICE_PREFIX
        );
    }

    /// The TypeSafe key the window's settings save is the one zo reads: the
    /// same keychain prefix, and the same variable name System One's client
    /// asks for.
    #[test]
    fn the_service_key_names_are_the_windows() {
        assert_eq!(
            crate::ide::SERVICE_KEYCHAIN_SERVICE_PREFIX,
            zerocode_harness::SERVICE_KEYCHAIN_SERVICE_PREFIX
        );
        assert_eq!(api::SYSTEMONE_API_KEY_ENV, zerocode_harness::TYPESAFE_API_KEY_ENV);
    }

    /// 창이 고른 OpenAI 계정을 따라가려면 창의 폴더를 정확히 같은 이름으로
    /// 불러야 한다. 창 쪽 철자는 부모 저장소의 것이고, 이 쪽 사본을 그것에
    /// 묶는다 — 창이 폴더 이름을 바꾸면 이 시험이 먼저 빨개진다(t-5777).
    #[test]
    fn the_windows_codex_home_is_spelled_the_windows_way() {
        assert_eq!(
            api::managed_account::ZEROCODE_APP_IDENTIFIER,
            zerocode_core::app::IDENTIFIER
        );
        assert_eq!(
            api::managed_account::CODEX_ACCOUNT_STORE_FILE,
            zerocode_core::codex_account::STORE_FILE
        );
        assert_eq!(
            api::managed_account::CODEX_RUNTIME_HOME_SEGMENTS,
            zerocode_core::codex_account::RUNTIME_HOME_SEGMENTS
        );
        assert_eq!(
            api::managed_account::CODEX_AUTH_FILE,
            zerocode_core::codex_account::AUTH_FILE
        );
        assert_eq!(
            api::managed_account::CODEX_HOME_ENV,
            zerocode_core::codex_account::HOME_VAR
        );
    }

    /// zo reads the Grok and Kimi Code CLIs' logins where the window reads
    /// them (t-6248): the window's spelling is the parent repository's, and
    /// this pins zo's copy to it — a CLI that moves its file turns this red
    /// before either side reads the wrong place.
    #[test]
    fn the_other_clis_logins_are_spelled_the_windows_way() {
        use api::cli_sessions as zo;
        use zerocode_core::cli_login_files::{grok, kimi};
        assert_eq!(zo::GROK_HOME_ENV, grok::HOME_VAR);
        assert_eq!(zo::GROK_HOME_DIR, grok::HOME_DIR);
        assert_eq!(zo::GROK_AUTH_FILE, grok::AUTH_FILE);
        assert_eq!(zo::GROK_PREFERRED_ISSUER, grok::PREFERRED_ISSUER);
        assert_eq!(zo::GROK_TOKEN_SKEW_MS, grok::TOKEN_SKEW_MS);
        assert_eq!(zo::KIMI_CODE_HOME_ENV, kimi::HOME_VAR);
        assert_eq!(zo::KIMI_CODE_HOME_DIR, kimi::HOME_DIR);
        assert_eq!(zo::KIMI_CODE_CREDENTIALS_TAIL, kimi::CREDENTIALS_TAIL);
        assert_eq!(zo::KIMI_CODE_EXPIRY_SKEW_SECONDS, kimi::EXPIRY_SKEW_SECONDS);
        assert_eq!(zo::KIMI_CODE_BASE_URL_ENV, kimi::BASE_URL_VAR);
        assert_eq!(zo::KIMI_CODE_DEFAULT_BASE_URL, kimi::DEFAULT_BASE_URL);
    }

    /// The TypeSafe switch the window's settings write is the one zo's router
    /// reads: every mode the Jev use table offers routing, written where the
    /// table says, reads back through zo's merged settings as itself.
    #[test]
    fn the_decision_shadow_the_windows_settings_write_is_the_one_zo_reads() {
        let home = tempfile::tempdir().expect("a config home");
        let routing = zerocode_core::jev::ROUTING;
        for mode in routing.modes {
            let settings = serde_json::json!({
                (zerocode_core::jev::SMART_SETTINGS_KEY): { (routing.setting): mode.key() }
            });
            std::fs::write(home.path().join("settings.json"), settings.to_string()).expect("settings");
            let loader = runtime::ConfigLoader::new(home.path(), home.path());
            assert_eq!(tools::decision_shadow_mode_from(&loader), Some(*mode), "{settings}");
        }
    }

    /// A zo typed into a shell pane finds the router keys the window keeps:
    /// main registers the keychain road right after adopting, under the same
    /// two prefixes.
    #[test]
    fn main_finds_the_router_keys_it_was_not_handed_in_the_windows_keychain() {
        let main = include_str!("main.rs");
        let body = &main[main.find("fn main() -> ExitCode {").expect("main")..];
        let adopt = body.find("api::adopt_launch_keys(").expect("adopt");
        let find = body
            .find("api::find_router_keys_in_keychain(")
            .expect("main registers the window's keychain");
        let first_other = body
            .find("runtime::relocate_traces_out_of_tree()")
            .expect("main's first act otherwise");
        assert!(adopt < find && find < first_other);
        let call = &body[find..first_other];
        assert!(
            call.contains("zo_ide::ide::ROUTER_KEY_ENV_PREFIX")
                && call.contains("zo_ide::ide::ROUTER_KEYCHAIN_SERVICE_PREFIX")
        );
    }

    /// Every zo finds the TypeSafe key the window's settings keep — typed into
    /// a shell or launched by the window, since no launch hands it over: main
    /// registers the named road before anything else runs, under the window's
    /// prefix and System One's own variable name.
    #[test]
    fn main_finds_the_typesafe_key_in_the_windows_keychain() {
        let main = include_str!("main.rs");
        let body = &main[main.find("fn main() -> ExitCode {").expect("main")..];
        let find = body
            .find("api::find_service_keys_in_keychain(")
            .expect("main registers the window's service keys");
        let first_other = body
            .find("runtime::relocate_traces_out_of_tree()")
            .expect("main's first act otherwise");
        assert!(find < first_other);
        let call = &body[find..first_other];
        assert!(
            call.contains("api::SYSTEMONE_API_KEY_ENV")
                && call.contains("zo_ide::ide::SERVICE_KEYCHAIN_SERVICE_PREFIX")
        );
    }
}
