//! IDE 가 고른 계정 — 프로세스 안의 덮어쓰기.
//!
//! 판이 태어날 때 IDE 는 계정을 **환경 변수**로 건넨다(`CLAUDE_CONFIG_DIR`,
//! `CODEX_HOME`). 그 판이 도는 동안 사람이 IDE 에서 계정을 바꾸면 환경 변수는
//! 이미 굳어 있다 — 자식 프로세스의 env 는 밖에서 못 고친다. 그래서 채널의
//! `auth.reload` 는 **값을 실어** 오고, 그 값이 여기 앉아 env 를 이긴다.
//!
//! 이 모듈이 계약의 전부다: 자격을 찾는 두 자리(`crate::providers::anthropic::keychain`
//! 의 `CLAUDE_CONFIG_DIR`, [`crate::oauth_store::codex_auth`] 의 `CODEX_HOME`)는
//! env 를 직접 읽지 않고 여기를 통해 읽는다.
//!
//! OpenAI 쪽은 한 자리가 더 있다. 창은 `CODEX_HOME` 을 **zo 판에만** 준다
//! (`zerocode_core::account::providers_for`). Claude 판이 도구로 부른 zo, 맨
//! 터미널에 친 zo, A/B 하네스가 띄운 zo 는 그 변수를 못 받고 제 저장소의 옛
//! 로그인으로 떨어져 `401 token_expired` 로 말했다 — 로그인이 만료된 게 아니라
//! **다른 로그인**으로 말한 것이다(2026-09-21 실측). 그래서 [`resolve_codex_home`]
//! 은 env 다음에 **창의 관리 codex 홈**을 본다: 어디서 돌든 창에 로그인된
//! 계정을 따라간다. `~/.codex` 는 여전히 암묵적으로 빌리지 않는다 — 그것은
//! 사람의 CLI 로그인이지 창이 고른 계정이 아니다.
//!
//! 라벨은 창이 준 것을 그대로 든다 — 마스킹 규칙을 zo 에서 다시 만들지 않는다
//! (`docs/design/zo-ide-account-oauth.md` §2.3).

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use serde_json::Value;

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

/// Codex 계정 홈 — [`resolve_codex_home`] 의 답에서 경로만.
#[must_use]
pub fn codex_home() -> Option<OsString> {
    resolve_codex_home().map(|home| home.path.into_os_string())
}

/// 이 판의 OpenAI 자격이 어느 codex 홈에서 왔는가.
///
/// 표의 행 이름이고, `/status` 의 출처 칸이 이것을 읽는다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodexHomeSource {
    /// ① 채널의 `auth.reload` 가 실어 온 계정 — 창이 방금 고른 것.
    Channel,
    /// ② 판이 태어날 때의 `CODEX_HOME`.
    Env,
    /// ③ ZeroCode 창의 관리 codex 홈 — 창이 이 판에게 아무것도 말해 주지
    ///    않았을 때 찾아낸, 창이 지금 쓰고 있는 계정.
    IdeManaged,
}

/// 한 codex 홈과 그 출처.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexHome {
    pub path: PathBuf,
    pub source: CodexHomeSource,
}

/// **OpenAI 계정 해석 순서 — 이 함수 하나가 정본이다.**
///
/// ① 채널 override → ② `CODEX_HOME` env → ③ 창의 관리 codex 홈. 셋 다
/// 없으면 `None` 이고, 그때 [`crate::oauth_store`] 가 zo 제 저장소(④)로
/// 떨어진다.
///
/// 빈 override 는 "말하지 않음" 이 아니라 **"관리 계정 없음"** 이다: 창이
/// 「시스템 기본값」 으로 돌아갔다는 뜻이라 판이 태어날 때의 env — 사람이 방금
/// 떠난 계정 — 로는 되돌아가지 않는다. ③ 은 그 금지에 걸리지 않는다. 그 홈은
/// 굳은 값이 아니라 창이 **지금** 쓰는 계정을 매번 다시 읽는 자리이기 때문이다.
#[must_use]
pub fn resolve_codex_home() -> Option<CodexHome> {
    if let Some(path) = with_managed(|managed| managed.codex_home.clone()).flatten() {
        if !path.as_os_str().is_empty() {
            return Some(CodexHome {
                path,
                source: CodexHomeSource::Channel,
            });
        }
        return ide_codex_home().map(CodexHome::ide_managed);
    }
    if let Some(home) = std::env::var_os(CODEX_HOME_ENV).filter(|value| !value.is_empty()) {
        return Some(CodexHome {
            path: PathBuf::from(home),
            source: CodexHomeSource::Env,
        });
    }
    ide_codex_home().map(CodexHome::ide_managed)
}

impl CodexHome {
    fn ide_managed(path: PathBuf) -> Self {
        Self {
            path,
            source: CodexHomeSource::IdeManaged,
        }
    }
}

/// ZeroCode 창이 지금 쓰고 있는 codex 홈. 창이 이 기계에 없거나, 그 홈에
/// 로그인이 없으면 `None`.
///
/// 바깥 자격 저장소를 아예 보지 말라고 선언한 판
/// ([`external_credentials_disabled`])에서는 찾지 않는다 — 이 홈이 바로 그
/// 스위치가 막는 것이고, 하네스가 띄운 헤르메틱 zo 가 개발자 기계의 계정을
/// 주워 오면 그 판은 더 이상 헤르메틱이 아니다.
#[must_use]
pub fn ide_codex_home() -> Option<PathBuf> {
    if external_credentials_disabled() {
        return None;
    }
    zerocode_app_data_roots()
        .iter()
        .find_map(|root| ide_codex_home_in(root))
}

/// [`ide_codex_home`] 을 앱 데이터 폴더 하나에 대고 — 뿌리를 손으로 건네는
/// 쪽이라 시험이 개발자의 진짜 폴더를 건드리지 않는다.
///
/// 창이 고른 계정의 홈이 먼저고, 고른 계정이 없으면(= 창이 이 기계의 기본
/// 로그인을 쓰는 중) 모든 판이 함께 쓰는 런타임 홈이다. 로그인 파일을 실제로
/// 들고 있는 홈만 답한다.
#[must_use]
pub fn ide_codex_home_in(app_data_root: &Path) -> Option<PathBuf> {
    active_codex_account_home(app_data_root)
        .into_iter()
        .chain(std::iter::once(ide_codex_runtime_home(app_data_root)))
        .find(|home| home.join(CODEX_AUTH_FILE).is_file())
}

/// 창의 계정 목록이 가리키는 활성 계정의 홈.
///
/// 목록이 없거나, 고른 계정이 없거나, 그 계정이 목록에서 사라졌으면 `None` —
/// 창에게 그것은 "이 기계의 기본 로그인" 이고, 그 자리는 런타임 홈이 받는다.
fn active_codex_account_home(app_data_root: &Path) -> Option<PathBuf> {
    let raw = std::fs::read(app_data_root.join(CODEX_ACCOUNT_STORE_FILE)).ok()?;
    let store: Value = serde_json::from_slice(&raw).ok()?;
    let active = store
        .get("active")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())?;
    store
        .get("accounts")?
        .as_array()?
        .iter()
        .find(|account| account.get("id").and_then(Value::as_str) == Some(active))?
        .get("home_dir")
        .and_then(Value::as_str)
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
}

/// 창이 모든 판에게 나눠 주는 공유 codex 런타임 홈.
fn ide_codex_runtime_home(app_data_root: &Path) -> PathBuf {
    CODEX_RUNTIME_HOME_SEGMENTS
        .iter()
        .fold(app_data_root.to_path_buf(), |path, segment| {
            path.join(segment)
        })
}

/// 창의 앱 데이터 폴더가 있을 수 있는 자리들, 먼저 볼 것부터.
///
/// 창은 Tauri 의 플랫폼 폴더를 쓴다 — 설정 뿌리(계정 목록)와 데이터 뿌리(런타임
/// 홈)는 맥에서 같은 폴더지만 리눅스에서는 다르다. 아직 플랫폼 폴더로 옮기지
/// 않은 창은 `~/.zerocode/state` 하나에 전부 둔다.
fn zerocode_app_data_roots() -> Vec<PathBuf> {
    let candidates = [
        dirs::config_dir().map(|root| root.join(ZEROCODE_APP_IDENTIFIER)),
        dirs::data_local_dir().map(|root| root.join(ZEROCODE_APP_IDENTIFIER)),
        dirs::home_dir().map(|home| {
            ZEROCODE_LEGACY_STATE_SEGMENTS
                .iter()
                .fold(home, |path, segment| path.join(segment))
        }),
    ];
    let mut roots: Vec<PathBuf> = Vec::new();
    for root in candidates.into_iter().flatten() {
        if !roots.contains(&root) {
            roots.push(root);
        }
    }
    roots
}

/// 바깥 자격 저장소를 하나도 보지 말라는 선언.
///
/// 하네스는 이 변수로 판을 완전히 헤르메틱하게 띄운다(`e2e/harness.rs`). 창의
/// 관리 홈도, 기계의 Gemini ADC 도 다 이 스위치 아래에 있다 — 판정은 여기
/// 하나다.
#[must_use]
pub fn external_credentials_disabled() -> bool {
    std::env::var(EXTERNAL_CREDENTIALS_DISABLED_ENV)
        .ok()
        .is_some_and(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
}

/// Claude Code 의 설정 폴더 변수. 정본은 여기 하나다.
pub const CLAUDE_CONFIG_DIR_ENV: &str = "CLAUDE_CONFIG_DIR";
/// Codex 의 홈 변수. `oauth_store::codex_auth` 가 다시 내보낸다.
pub const CODEX_HOME_ENV: &str = "CODEX_HOME";
/// 이 기계의 바깥 자격 저장소를 전부 가리는 스위치.
pub const EXTERNAL_CREDENTIALS_DISABLED_ENV: &str = "ZO_DISABLE_EXTERNAL_CREDENTIALS";
/// 창의 앱 데이터 폴더 이름 = Tauri 번들 식별자. 창 쪽 정본은
/// `zerocode_core::app::IDENTIFIER` 이고, 워크스페이스가 둘이라 이쪽에 한 번
/// 적는다 — 두 철자는 `zo-ide` 의 시험이 묶는다.
pub const ZEROCODE_APP_IDENTIFIER: &str = "dev.zerocode.app";
/// 창이 계정 목록을 적는 파일(`zerocode_core::codex_account::STORE_FILE`).
pub const CODEX_ACCOUNT_STORE_FILE: &str = "codex-accounts.json";
/// 창의 공유 런타임 홈 자리(`zerocode_core::codex_account::RUNTIME_HOME_SEGMENTS`).
pub const CODEX_RUNTIME_HOME_SEGMENTS: [&str; 2] = ["codex-runtime-home", "home"];
/// codex 홈 안의 로그인 파일(`zerocode_core::codex_account::AUTH_FILE`).
pub const CODEX_AUTH_FILE: &str = "auth.json";
/// 플랫폼 폴더로 옮기기 전 창이 모든 것을 두던 자리
/// (`zerocode_lane::legacy_state_root`), `$HOME` 아래.
const ZEROCODE_LEGACY_STATE_SEGMENTS: [&str; 2] = [".zerocode", "state"];

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

        fn clear(name: &'static str) -> Self {
            let prior = std::env::var_os(name);
            std::env::remove_var(name);
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

    /// 창의 앱 데이터 폴더 하나를 픽스처로 짓는다: 계정 목록과, 로그인이 든
    /// 홈들.
    fn window_app_data(root: &Path, active: Option<&str>, accounts: &[(&str, bool)]) -> PathBuf {
        let mut rows = Vec::new();
        for (id, signed_in) in accounts {
            let home = root.join("codex-accounts").join(id).join("home");
            std::fs::create_dir_all(&home).expect("account home");
            if *signed_in {
                std::fs::write(home.join(CODEX_AUTH_FILE), "{}").expect("seed the login");
            }
            rows.push(format!(
                r#"{{"id":"{id}","home_dir":{}}}"#,
                serde_json::to_string(&home.to_string_lossy()).expect("path")
            ));
        }
        let active = active.map_or(String::new(), |id| format!(r#""active":"{id}","#));
        std::fs::create_dir_all(root).expect("app data root");
        std::fs::write(
            root.join(CODEX_ACCOUNT_STORE_FILE),
            format!("{{{active}\"accounts\":[{}]}}", rows.join(",")),
        )
        .expect("account store");
        root.join("codex-accounts")
    }

    fn runtime_home_of(root: &Path) -> PathBuf {
        let home = ide_codex_runtime_home(root);
        std::fs::create_dir_all(&home).expect("runtime home");
        home
    }

    /// 창이 고른 계정의 홈이 먼저고, 고른 계정이 없으면 공유 런타임 홈이다 —
    /// 창에게 "고른 계정 없음" 은 이 기계의 기본 로그인이고, 런타임 홈이 바로
    /// 그 로그인의 사본이기 때문이다.
    #[test]
    fn the_windows_chosen_account_comes_first_and_the_runtime_home_is_the_fallback() {
        let root = tempfile::tempdir().expect("an app data root");
        let root = root.path();
        let accounts = window_app_data(root, Some("acct-1"), &[("acct-1", true)]);
        let runtime = runtime_home_of(root);
        std::fs::write(runtime.join(CODEX_AUTH_FILE), "{}").expect("runtime login");

        assert_eq!(
            ide_codex_home_in(root),
            Some(accounts.join("acct-1").join("home")),
            "the account the window chose is the one to follow"
        );

        // 고른 계정 없음 — 창은 이 기계의 기본 로그인을 쓰는 중이고, 그 사본은
        // 런타임 홈이다.
        window_app_data(root, None, &[("acct-1", true)]);
        assert_eq!(ide_codex_home_in(root), Some(runtime.clone()));

        // 고른 계정이 로그인을 들고 있지 않으면 그 홈은 답이 아니다.
        window_app_data(root, Some("acct-2"), &[("acct-2", false)]);
        assert_eq!(ide_codex_home_in(root), Some(runtime.clone()));

        // 로그인이 아무 데도 없으면 아무것도 빌리지 않는다.
        std::fs::remove_file(runtime.join(CODEX_AUTH_FILE)).expect("log the window out");
        assert_eq!(ide_codex_home_in(root), None);

        // 목록 자체가 없는 폴더(창이 없는 기계)도 마찬가지다.
        let bare = tempfile::tempdir().expect("a bare root");
        assert_eq!(ide_codex_home_in(bare.path()), None);
    }

    /// 순서표 그대로: 채널 → env → 창의 관리 홈. 그리고 빈 override 는 env 로
    /// 되돌아가지 않되 창이 **지금** 쓰는 계정은 따라간다.
    #[test]
    fn the_resolution_table_reads_top_down_and_names_where_it_borrowed() {
        let _lock = crate::test_env_lock();
        clear();
        let root = tempfile::tempdir().expect("an app data root");
        let legacy = ZEROCODE_LEGACY_STATE_SEGMENTS
            .iter()
            .fold(root.path().to_path_buf(), |path, segment| path.join(segment));
        let ide = runtime_home_of(&legacy);
        std::fs::write(ide.join(CODEX_AUTH_FILE), "{}").expect("the window's login");
        let _switch = EnvVarGuard::set(EXTERNAL_CREDENTIALS_DISABLED_ENV, "0");
        let env_home = PathBuf::from("/tmp/zo-codex-home-env");
        let channel_home = PathBuf::from("/tmp/zo-codex-home-channel");

        // ③ 만 있을 때: CODEX_HOME 없는 셸에서도 창의 계정을 따라간다. 창의
        // 폴더는 `$HOME` 아래 옛 뿌리로 세운다 — 세 OS 가 같은 규칙이라 시험이
        // 개발자의 진짜 폴더를 건드리지 않는다.
        let _home = EnvVarGuard::set("HOME", &root.path().to_string_lossy());
        let _codex_cleared = EnvVarGuard::clear(CODEX_HOME_ENV);
        assert_eq!(
            ide_codex_home_in(&legacy),
            Some(ide.clone()),
            "the fixture window is signed in"
        );
        let resolved = resolve_codex_home().expect("the window's own home is the third source");
        assert_eq!(resolved.path, ide);
        assert_eq!(resolved.source, CodexHomeSource::IdeManaged);

        // ② env 는 ③ 을 이긴다 — 창이 이 판에 직접 건넨 계정이다.
        let _codex = EnvVarGuard::set(CODEX_HOME_ENV, "/tmp/zo-codex-home-env");
        let resolved = resolve_codex_home().expect("the launch environment names one");
        assert_eq!(resolved.path, env_home);
        assert_eq!(resolved.source, CodexHomeSource::Env);

        // ① 채널이 실어 온 계정은 그 env 를 이긴다.
        apply(
            ManagedProvider::OpenAi,
            &ManagedAccountUpdate {
                label: Some("personal".to_string()),
                claude_config_dir: None,
                codex_home: Some(channel_home.clone()),
            },
        );
        let resolved = resolve_codex_home().expect("the channel named one");
        assert_eq!(resolved.path, channel_home);
        assert_eq!(resolved.source, CodexHomeSource::Channel);

        // 빈 값 = 「관리 계정 없음」: 사람이 방금 떠난 env 로는 돌아가지 않고,
        // 창이 지금 쓰는 홈을 읽는다.
        apply(
            ManagedProvider::OpenAi,
            &ManagedAccountUpdate {
                label: None,
                claude_config_dir: None,
                codex_home: Some(PathBuf::new()),
            },
        );
        let resolved = resolve_codex_home().expect("the window is still signed in");
        assert_ne!(
            resolved.path, env_home,
            "an emptied override fell back to the account the person just left"
        );
        assert_eq!(resolved.source, CodexHomeSource::IdeManaged);
        clear();
    }

    /// 헤르메틱 판은 이 기계의 계정을 하나도 줍지 않는다 — 하네스가 켜는 그
    /// 스위치 하나가 창의 관리 홈도 함께 가린다.
    #[test]
    fn a_hermetic_pane_never_borrows_the_windows_account() {
        let _lock = crate::test_env_lock();
        clear();
        let _switch = EnvVarGuard::set(EXTERNAL_CREDENTIALS_DISABLED_ENV, "1");
        assert!(external_credentials_disabled());
        assert_eq!(ide_codex_home(), None);
    }

    #[test]
    fn provider_slugs_round_trip_and_reject_strangers() {
        for provider in ManagedProvider::all() {
            assert_eq!(ManagedProvider::from_slug(provider.slug()), Some(*provider));
        }
        assert_eq!(ManagedProvider::from_slug("bedrock"), None);
    }
}
