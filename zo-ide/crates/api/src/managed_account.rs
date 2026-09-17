//! IDE 가 고른 계정 — 프로세스 안의 덮어쓰기.
//!
//! 판이 태어날 때 IDE 는 계정을 **환경 변수**로 건넨다(`CLAUDE_CONFIG_DIR`,
//! `CODEX_HOME`). 그 판이 도는 동안 사람이 IDE 에서 계정을 바꾸면 환경 변수는
//! 이미 굳어 있다 — 자식 프로세스의 env 는 밖에서 못 고친다. 그래서 채널의
//! `auth.reload` 는 **값을 실어** 오고, 그 값이 여기 앉아 env 를 이긴다.
//!
//! 이 모듈이 계약의 전부다: 자격을 찾는 두 자리(`crate::providers::anthropic::keychain`
//! 의 `CLAUDE_CONFIG_DIR`, [`crate::oauth_store::codex_auth`] 의 `CODEX_HOME`)는
//! env 를 직접 읽지 않고 여기를 통해 읽는다. 덮어쓰기가 없으면 답은 env 그대로라,
//! 채널이 없는 판(맨 셸에서 손으로 친 `zo`)의 동작은 하나도 바뀌지 않는다.
//!
//! 라벨은 창이 준 것을 그대로 든다 — 마스킹 규칙을 zo 에서 다시 만들지 않는다
//! (`docs/design/zo-ide-account-oauth.md` §2.3).

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

/// 계정 자격의 한 갈래. 창의 `zerocode_core::account::Provider` 와 같은 이름을
/// 쓰되, Google 은 파일 하나를 공유하므로 라벨만 있고 경로가 없다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManagedProvider {
    Anthropic,
    OpenAi,
    Google,
}

impl ManagedProvider {
    /// 채널 params 와 `/status` 가 쓰는 이름.
    #[must_use]
    pub const fn slug(self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic",
            Self::OpenAi => "openai",
            Self::Google => "google",
        }
    }

    /// 모르는 이름은 `None` — 채널은 그것을 `-32602` 로 답한다.
    #[must_use]
    pub fn from_slug(slug: &str) -> Option<Self> {
        match slug {
            "anthropic" => Some(Self::Anthropic),
            "openai" => Some(Self::OpenAi),
            "google" => Some(Self::Google),
            _ => None,
        }
    }

    /// 재적재 깃발 배열의 자리.
    const fn index(self) -> usize {
        match self {
            Self::Anthropic => 0,
            Self::OpenAi => 1,
            Self::Google => 2,
        }
    }

    /// `null`(= 전부)일 때 도는 차례.
    #[must_use]
    pub const fn all() -> &'static [Self] {
        &[Self::Anthropic, Self::OpenAi, Self::Google]
    }
}

/// `auth.reload` 한 번이 싣고 오는 것.
///
/// 세 필드 모두 세 가지를 말한다: `None` 은 **말하지 않았다**(그 자리는 그대로),
/// 빈 값은 **없다**(창이 「시스템 기본값」으로 돌아갔다 — env 로 되돌아가지 않고
/// 관리 계정이 없음을 못 박는다), 값이 있으면 그것.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ManagedAccountUpdate {
    pub label: Option<String>,
    pub claude_config_dir: Option<PathBuf>,
    pub codex_home: Option<PathBuf>,
}

#[derive(Debug, Default)]
struct ManagedAccounts {
    claude_config_dir: Option<PathBuf>,
    codex_home: Option<PathBuf>,
    anthropic_label: Option<String>,
    openai_label: Option<String>,
    google_label: Option<String>,
}

/// Poison policy: recover — 쓰기는 짧고, 잠금이 오염됐다고 판이 자격을 잃을
/// 이유가 없다.
static MANAGED: Mutex<Option<ManagedAccounts>> = Mutex::new(None);

fn with_managed<T>(read: impl FnOnce(&ManagedAccounts) -> T) -> Option<T> {
    let guard = MANAGED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    guard.as_ref().map(read)
}

/// 한 프로바이더의 계정을 이 프로세스 안에서 갈아 끼운다.
///
/// 경로를 실어 온 프로바이더만 경로가 바뀐다. 라벨은 실어 온 만큼만 바뀌고,
/// 실리지 않았으면 이전 라벨이 남는다 — 값을 안 준 것과 "이름 없음" 은 다르다.
pub fn apply(provider: ManagedProvider, update: &ManagedAccountUpdate) {
    let mut guard = MANAGED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let managed = guard.get_or_insert_with(ManagedAccounts::default);
    match provider {
        ManagedProvider::Anthropic => {
            if update.claude_config_dir.is_some() {
                managed.claude_config_dir.clone_from(&update.claude_config_dir);
            }
            if update.label.is_some() {
                managed.anthropic_label.clone_from(&update.label);
            }
        }
        ManagedProvider::OpenAi => {
            if update.codex_home.is_some() {
                managed.codex_home.clone_from(&update.codex_home);
            }
            if update.label.is_some() {
                managed.openai_label.clone_from(&update.label);
            }
        }
        ManagedProvider::Google => {
            if update.label.is_some() {
                managed.google_label.clone_from(&update.label);
            }
        }
    }
}

/// 재적재가 요청됐고 그 프로바이더의 오래 사는 클라이언트가 아직 갈아 끼워지지
/// 않았다 — 프로바이더마다 하나.
///
/// 자격 자체는 캐시를 비우면 다음 해석에서 새로 읽히지만, 이미 지어진
/// 클라이언트는 만들 때의 bearer 를 쥐고 있다(ChatGPT·Gemini 는 요청마다 다시
/// 읽지 않는다). 그 클라이언트를 **한 번** 다시 짓게 하는 것이 이 깃발이다.
static RELOAD_PENDING: [AtomicBool; 3] = [
    AtomicBool::new(false),
    AtomicBool::new(false),
    AtomicBool::new(false),
];

/// 이 프로바이더의 자격이 갈렸다고 적어 둔다.
pub fn note_reload(provider: ManagedProvider) {
    RELOAD_PENDING[provider.index()].store(true, Ordering::SeqCst);
}

/// 아직 갈아 끼우지 않았는가. 읽어도 지워지지 않는다 — 지우는 것은 실제로 다시
/// 지은 쪽의 몫이다.
#[must_use]
pub fn reload_pending(provider: ManagedProvider) -> bool {
    RELOAD_PENDING[provider.index()].load(Ordering::SeqCst)
}

/// 다시 지었다.
pub fn clear_reload_pending(provider: ManagedProvider) {
    RELOAD_PENDING[provider.index()].store(false, Ordering::SeqCst);
}

/// 창이 보여 주는 계정 이름. 덮어쓰기가 없으면 `None` 이고, 그때 카드는
/// 파일에서 읽은 것으로 말한다.
#[must_use]
pub fn label(provider: ManagedProvider) -> Option<String> {
    with_managed(|managed| match provider {
        ManagedProvider::Anthropic => managed.anthropic_label.clone(),
        ManagedProvider::OpenAi => managed.openai_label.clone(),
        ManagedProvider::Google => managed.google_label.clone(),
    })
    .flatten()
    .filter(|label| !label.is_empty())
}

/// Claude 자격 폴더 — 덮어쓰기가 먼저, 없으면 프로세스 env.
///
/// 빈 값은 "없음" 이다: 헤르메틱 시험은 `CLAUDE_CONFIG_DIR=""` 로 기계의 진짜
/// 계정을 가린다(`zo-ide/docs/testing.md`).
#[must_use]
pub fn claude_config_dir() -> Option<OsString> {
    if let Some(dir) = with_managed(|managed| managed.claude_config_dir.clone()).flatten() {
        // 빈 경로는 "관리 계정 없음" 이고, 그때 env 로 떨어지지 않는다: 그 env
        // 는 사람이 방금 떠난 계정을 가리킨다.
        return (!dir.as_os_str().is_empty()).then(|| dir.into_os_string());
    }
    std::env::var_os(CLAUDE_CONFIG_DIR_ENV).filter(|value| !value.is_empty())
}

/// Codex 계정 홈 — 덮어쓰기가 먼저, 없으면 프로세스 env.
#[must_use]
pub fn codex_home() -> Option<OsString> {
    if let Some(home) = with_managed(|managed| managed.codex_home.clone()).flatten() {
        return (!home.as_os_str().is_empty()).then(|| home.into_os_string());
    }
    std::env::var_os(CODEX_HOME_ENV).filter(|value| !value.is_empty())
}

/// Claude Code 의 설정 폴더 변수. 정본은 여기 하나다.
pub const CLAUDE_CONFIG_DIR_ENV: &str = "CLAUDE_CONFIG_DIR";
/// Codex 의 홈 변수. `oauth_store::codex_auth` 가 다시 내보낸다.
pub const CODEX_HOME_ENV: &str = "CODEX_HOME";

/// 덮어쓰기를 통째로 지운다 — 시험이 다음 시험에 자기 계정을 물려주지 않게.
pub fn clear() {
    *MANAGED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    for provider in ManagedProvider::all() {
        clear_reload_pending(*provider);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 한 변수만 잡았다 되돌려 놓는 작은 가드 — 실패한 단언이 다음 시험에
    /// 자기 env 를 물려주지 않게.
    struct EnvVarGuard {
        name: &'static str,
        prior: Option<OsString>,
    }

    impl EnvVarGuard {
        fn set(name: &'static str, value: &str) -> Self {
            let prior = std::env::var_os(name);
            std::env::set_var(name, value);
            Self { name, prior }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            match self.prior.take() {
                Some(value) => std::env::set_var(self.name, value),
                None => std::env::remove_var(self.name),
            }
        }
    }

    /// 이 시험들은 프로세스 전역 상태 둘(env 와 덮어쓰기)을 함께 만지므로
    /// `test_env_lock` 아래에서만 돈다.
    #[test]
    fn an_in_process_override_outranks_the_launch_environment() {
        let _lock = crate::test_env_lock();
        clear();
        let env_home = PathBuf::from("/tmp/zo-managed-account-env");
        let switched = PathBuf::from("/tmp/zo-managed-account-switched");
        let _claude = EnvVarGuard::set(CLAUDE_CONFIG_DIR_ENV, "/tmp/zo-managed-account-env");
        let _codex = EnvVarGuard::set(CODEX_HOME_ENV, "/tmp/zo-managed-account-env");

        assert_eq!(claude_config_dir(), Some(env_home.clone().into_os_string()));
        assert_eq!(codex_home(), Some(env_home.clone().into_os_string()));

        apply(
            ManagedProvider::Anthropic,
            &ManagedAccountUpdate {
                label: Some("work".to_string()),
                claude_config_dir: Some(switched.clone()),
                codex_home: None,
            },
        );
        apply(
            ManagedProvider::OpenAi,
            &ManagedAccountUpdate {
                label: Some("personal".to_string()),
                claude_config_dir: None,
                codex_home: Some(switched.clone()),
            },
        );

        assert_eq!(claude_config_dir(), Some(switched.clone().into_os_string()));
        assert_eq!(codex_home(), Some(switched.into_os_string()));
        assert_eq!(label(ManagedProvider::Anthropic).as_deref(), Some("work"));
        assert_eq!(label(ManagedProvider::OpenAi).as_deref(), Some("personal"));
        clear();
        assert_eq!(claude_config_dir(), Some(env_home.into_os_string()));
    }

    #[test]
    fn a_label_only_reload_keeps_the_path_it_was_launched_with() {
        let _lock = crate::test_env_lock();
        clear();
        let env_home = PathBuf::from("/tmp/zo-managed-account-kept");
        let _claude = EnvVarGuard::set(CLAUDE_CONFIG_DIR_ENV, "/tmp/zo-managed-account-kept");

        apply(
            ManagedProvider::Anthropic,
            &ManagedAccountUpdate {
                label: Some("still me".to_string()),
                claude_config_dir: None,
                codex_home: None,
            },
        );

        assert_eq!(claude_config_dir(), Some(env_home.into_os_string()));
        assert_eq!(
            label(ManagedProvider::Anthropic).as_deref(),
            Some("still me")
        );
        clear();
    }

    #[test]
    fn a_noted_reload_stands_until_the_client_is_rebuilt() {
        let _lock = crate::test_env_lock();
        clear();
        assert!(!reload_pending(ManagedProvider::OpenAi));
        note_reload(ManagedProvider::OpenAi);
        assert!(reload_pending(ManagedProvider::OpenAi));
        // Reading it does not consume it: the turn path may ask twice before
        // the rebuild lands.
        assert!(reload_pending(ManagedProvider::OpenAi));
        assert!(
            !reload_pending(ManagedProvider::Anthropic),
            "one provider's switch must not rebuild another's client"
        );
        clear_reload_pending(ManagedProvider::OpenAi);
        assert!(!reload_pending(ManagedProvider::OpenAi));
        clear();
    }

    #[test]
    fn provider_slugs_round_trip_and_reject_strangers() {
        for provider in ManagedProvider::all() {
            assert_eq!(ManagedProvider::from_slug(provider.slug()), Some(*provider));
        }
        assert_eq!(ManagedProvider::from_slug("bedrock"), None);
    }
}
