//! `auth.reload` — IDE 에서 계정이 바뀌었다.
//!
//! 이 판은 태어날 때 계정을 환경 변수로 받았다(`CLAUDE_CONFIG_DIR` ·
//! `CODEX_HOME`). 자식 프로세스의 env 는 밖에서 못 고치므로, 창은 새 경로와
//! 표시 이름을 **params 에 실어** 보내고 이 문이 그것을 프로세스 안의
//! 덮어쓰기([`api::managed_account`])에 앉힌다 — 그 자리가 env 를 이긴다.
//!
//! 그다음 하는 일은 「잊기」뿐이다. 자격을 여기서 다시 읽지 않는다: 캐시를
//! 비우고 재빌드 깃발을 세우면 다음 요청 길목(`runtime_support::
//! refresh_oauth_if_near_expiry`)이 새 계정으로 해석한다. 소켓 핸들러가 파일을
//! 읽고 네트워크 갱신을 돌면 그 사이 IDE 의 요청은 답을 못 받는다.
//!
//! ```text
//! → {"method":"auth.reload","params":{"provider":"anthropic","label":"work",
//!                                     "claude_config_dir":"/…/accounts/a"}}
//! ← {"reloaded":["anthropic"],"account":{"provider":"anthropic","label":"work"}}
//! ```
//!
//! `provider` 가 없거나 `null` 이면 셋 다. 모르는 이름은 `-32602`.

use std::path::PathBuf;

use api::managed_account::{self, ManagedAccountUpdate, ManagedProvider};
use serde_json::{json, Value};

use super::wire::{CODE_INVALID_PARAMS, RpcResponse};

/// 한 번의 재적재. 어느 프로바이더를 다시 읽게 했는지 돌려준다.
pub(crate) fn handle(params: &Value, id: u64) -> RpcResponse {
    let providers = match targets(params) {
        Ok(providers) => providers,
        Err(message) => return RpcResponse::err(id, CODE_INVALID_PARAMS, message),
    };
    let update = update_from(params);
    for provider in &providers {
        reload(*provider, &update);
    }
    // 응답의 `account` 는 이름을 실어 온 프로바이더 하나를 가리킨다. 전부를
    // 되짚는 재적재는 이름이 없을 수 있고, 그때는 창이 이미 아는 것을 되돌려
    // 줄 이유가 없다.
    let named = providers
        .iter()
        .find(|provider| managed_account::label(**provider).is_some());
    let account = named.map_or(Value::Null, |provider| {
        json!({
            "provider": provider.slug(),
            "label": managed_account::label(*provider),
        })
    });
    RpcResponse::ok(
        id,
        json!({
            "reloaded": providers
                .iter()
                .map(|provider| provider.slug())
                .collect::<Vec<_>>(),
            "account": account,
        }),
    )
}

/// 이번 재적재가 겨누는 프로바이더들. 없거나 `null` 이면 전부.
fn targets(params: &Value) -> Result<Vec<ManagedProvider>, String> {
    match params.get("provider") {
        None | Some(Value::Null) => Ok(ManagedProvider::all().to_vec()),
        Some(Value::String(named)) => ManagedProvider::from_slug(named)
            .map(|provider| vec![provider])
            .ok_or_else(|| format!("auth.reload does not know provider {named}")),
        Some(_) => Err("auth.reload needs provider: a string or null".to_string()),
    }
}

/// params 가 싣고 온 값들.
///
/// 열쇠가 **없는** 것과 **비어 있는** 것은 다르다: 없으면 그 자리를 그대로 두고,
/// 비어 있으면 "관리 계정 없음" 이다 — 창이 「시스템 기본값」 행으로 돌아갔을 때
/// 판이 태어날 때의 env(= 방금 떠난 계정)로 되돌아가지 않도록.
fn update_from(params: &Value) -> ManagedAccountUpdate {
    let text = |key: &str| {
        params
            .get(key)
            .and_then(Value::as_str)
            .map(|value| value.trim().to_string())
    };
    ManagedAccountUpdate {
        label: text("label"),
        claude_config_dir: text("claude_config_dir").map(PathBuf::from),
        codex_home: text("codex_home").map(PathBuf::from),
    }
}

/// 한 프로바이더를 갈아 끼우고 그것을 기억하는 자리를 모두 비운다.
fn reload(provider: ManagedProvider, update: &ManagedAccountUpdate) {
    managed_account::apply(provider, update);
    managed_account::note_reload(provider);
    if provider == ManagedProvider::Anthropic {
        // 이 둘이 Anthropic 의 기억 전부다: api 의 키체인/파일 메모와, 출처를
        // 프로세스 수명으로 핀하는 CLI 의 메모.
        api::invalidate_claude_code_keychain_cache();
        crate::runtime_support::forget_cached_claude_auth();
    }
    // OpenAI 와 Google 은 클라이언트를 지을 때마다 파일을 다시 읽는다. 남는
    // 것은 이미 지어진 클라이언트가 쥔 bearer 하나이고, 그것은 위의
    // `note_reload` 가 다음 턴에 다시 짓게 한다. Google 의 Code Assist
    // 프로젝트 기억은 그 bearer 의 계정(grant)에 매여 있어서, 새 계정으로
    // 지어진 클라이언트는 떠난 계정의 프로젝트를 받지 않는다 — 여기서 비울
    // 것이 없다.
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code_of(response: &RpcResponse) -> Option<i64> {
        response.error.as_ref().map(|error| error.code)
    }

    fn result_of(response: &RpcResponse) -> Value {
        response.result.clone().expect("a result")
    }

    #[test]
    fn a_named_switch_carries_its_paths_into_the_process() {
        let _lock = crate::ide::channel::auth_reload::tests_support::lock();
        managed_account::clear();
        let response = handle(
            &json!({
                "provider": "anthropic",
                "label": "work",
                "claude_config_dir": "/tmp/zo-auth-reload-anthropic"
            }),
            7,
        );

        assert!(response.error.is_none(), "{response:?}");
        let result = result_of(&response);
        assert_eq!(result["reloaded"], json!(["anthropic"]));
        assert_eq!(result["account"]["provider"], "anthropic");
        assert_eq!(result["account"]["label"], "work");
        assert_eq!(
            managed_account::claude_config_dir(),
            Some("/tmp/zo-auth-reload-anthropic".into())
        );
        assert!(
            managed_account::reload_pending(ManagedProvider::Anthropic),
            "the live client was not asked to rebuild"
        );
        assert!(
            !managed_account::reload_pending(ManagedProvider::OpenAi),
            "an Anthropic switch rebuilt the OpenAI client too"
        );
        managed_account::clear();
    }

    #[test]
    fn a_reload_without_a_provider_touches_every_lane() {
        let _lock = crate::ide::channel::auth_reload::tests_support::lock();
        managed_account::clear();
        let response = handle(&json!({}), 3);

        assert!(response.error.is_none(), "{response:?}");
        assert_eq!(
            result_of(&response)["reloaded"],
            json!(["anthropic", "openai", "google"])
        );
        for provider in ManagedProvider::all() {
            assert!(
                managed_account::reload_pending(*provider),
                "{} was left on its old account",
                provider.slug()
            );
        }
        // Nobody named an account, so there is nothing to show — an empty
        // label would read as "signed out" on the status line.
        assert_eq!(result_of(&response)["account"], Value::Null);
        managed_account::clear();
    }

    #[test]
    fn an_openai_switch_carries_the_codex_home_it_names() {
        let _lock = crate::ide::channel::auth_reload::tests_support::lock();
        managed_account::clear();
        let response = handle(
            &json!({
                "provider": "openai",
                "label": "personal",
                "codex_home": "/tmp/zo-auth-reload-codex"
            }),
            9,
        );

        assert!(response.error.is_none(), "{response:?}");
        assert_eq!(
            managed_account::codex_home(),
            Some("/tmp/zo-auth-reload-codex".into())
        );
        assert_eq!(result_of(&response)["account"]["label"], "personal");
        managed_account::clear();
    }

    #[test]
    fn a_provider_this_pane_has_no_credentials_for_is_invalid_params() {
        let _lock = crate::ide::channel::auth_reload::tests_support::lock();
        managed_account::clear();
        for params in [json!({"provider": "bedrock"}), json!({"provider": 4})] {
            let response = handle(&params, 1);
            assert_eq!(code_of(&response), Some(CODE_INVALID_PARAMS), "{params}");
        }
        for provider in ManagedProvider::all() {
            assert!(
                !managed_account::reload_pending(*provider),
                "a refused reload still moved {}",
                provider.slug()
            );
        }
        managed_account::clear();
    }

    /// 창이 「시스템 기본값」 으로 돌아가면 빈 값이 온다. 그것은 "말하지 않음"
    /// 이 아니라 "관리 계정 없음" 이고, 판이 태어날 때의 env — 사람이 방금 떠난
    /// 계정 — 로 되돌아가서는 안 된다.
    #[test]
    fn going_back_to_the_system_default_clears_the_override_rather_than_falling_back() {
        let _lock = crate::ide::channel::auth_reload::tests_support::lock();
        managed_account::clear();
        handle(
            &json!({
                "provider": "anthropic",
                "label": "work",
                "claude_config_dir": "/tmp/zo-auth-reload-left-behind"
            }),
            1,
        );
        assert!(managed_account::claude_config_dir().is_some());

        let response = handle(
            &json!({"provider": "anthropic", "label": "", "claude_config_dir": ""}),
            2,
        );

        assert!(response.error.is_none(), "{response:?}");
        assert_eq!(managed_account::claude_config_dir(), None);
        assert_eq!(managed_account::label(ManagedProvider::Anthropic), None);
        assert_eq!(result_of(&response)["account"], Value::Null);
        managed_account::clear();
    }

    /// 열쇠가 없는 것은 그 자리를 건드리지 않는다 — 라벨만 온 재적재는 경로를
    /// 지우지 않는다.
    #[test]
    fn a_key_nobody_sent_leaves_its_slot_alone() {
        let _lock = crate::ide::channel::auth_reload::tests_support::lock();
        managed_account::clear();
        let update = update_from(&json!({ "label": "work" }));
        assert_eq!(update.label.as_deref(), Some("work"));
        assert_eq!(update.claude_config_dir, None);
        assert_eq!(update.codex_home, None);

        let cleared = update_from(&json!({ "claude_config_dir": "  " }));
        assert_eq!(cleared.claude_config_dir, Some(PathBuf::new()));
        managed_account::clear();
    }
}

/// 이 파일의 시험들은 프로세스 전역 덮어쓰기를 함께 만지므로 한 줄로 선다.
#[cfg(test)]
pub(crate) mod tests_support {
    use std::sync::{Mutex, MutexGuard, OnceLock};

    pub(crate) fn lock() -> MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}
