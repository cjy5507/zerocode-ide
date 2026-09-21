//! More than one Claude account, and which one an agent runs as.
//!
//! Measured from Orca **1.4.164**'s `ClaudeAccountService`
//! (out/main/index.js:209540-210034) and `applyClaudeEnvPatch` (:34861). The
//! version is part of the citation because these are line numbers into a
//! bundle: the same functions sit at :34610 in 1.4.158, and a reference with no
//! version on it sends the next reader to the wrong file. The whole mechanism
//! rests on one fact
//! about the CLI rather than on any API: **`CLAUDE_CONFIG_DIR` decides which
//! settings and conversations `claude` uses, while
//! `CLAUDE_SECURESTORAGE_CONFIG_DIR` can point its credentials elsewhere.** So
//! an account is a credential DIRECTORY, adding one is running the CLI's own
//! login with that directory pointed at, and switching is choosing which secure
//! storage directory the next launch names.
//!
//! That is why this can be done honestly at all. There is no Anthropic API for
//! "log me in"; there is the CLI's own browser flow, and the only sound way to
//! hold two accounts is to let it write two config directories and remember
//! which is which. No token of ours, no login form of ours, no credential this
//! window has ever read.
//!
//! What this module is: the pure part — identity, duplicate detection, the
//! selection, and the env an account resolves to. The directories, the login
//! subprocess and the storage are the shell's.
//!
//! One measured detail that is a decision rather than plumbing: Orca STRIPS the
//! ambient auth variables when it points at a managed directory
//! (`CLAUDE_AUTH_ENV_VARS`, :34855). A machine with `ANTHROPIC_API_KEY` in its
//! shell would otherwise have every account silently ignored — the key wins over
//! the config directory, so the person switches accounts and nothing changes.

use serde::{Deserialize, Serialize};

/// The variable that decides which settings and conversations the CLI uses.
pub const CONFIG_DIR_VAR: &str = "CLAUDE_CONFIG_DIR";

/// The credential-store override that lets several accounts share one Claude
/// configuration and conversation home.
pub const SECURE_STORAGE_CONFIG_DIR_VAR: &str = "CLAUDE_SECURESTORAGE_CONFIG_DIR";

/// Variables that would override a managed directory, and are therefore removed
/// from a launch that names one (`CLAUDE_AUTH_ENV_VARS`, measured verbatim).
///
/// Each of these is a way of saying "use this credential instead", and leaving
/// one in place makes account switching a control that does nothing.
pub const OVERRIDING_AUTH_VARS: &[&str] = &[
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "CLAUDE_CODE_OAUTH_TOKEN",
    "AWS_BEARER_TOKEN_BEDROCK",
];

/// A fifth way to override a directory, and the one that has to be JUDGED.
pub const CUSTOM_HEADERS_VAR: &str = "ANTHROPIC_CUSTOM_HEADERS";

/// Does this `ANTHROPIC_CUSTOM_HEADERS` value carry a credential?
///
/// Orca asks the same question and clears the variable only when the answer is
/// yes (`isAuthLikeCustomHeaders`, :34874 — the words are its own regex). The
/// judgement is the point: the variable also carries headers that have nothing
/// to do with authentication — a trace id, a beta opt-in — and clearing those
/// would break something a person set deliberately, for a reason that has
/// nothing to do with which account they are on. But a header spelling
/// `x-api-key` beats the config directory exactly like the environment variable
/// does, and leaving it makes the picker a control that changes nothing.
pub fn headers_carry_auth(value: &str) -> bool {
    // Matched by hand rather than by a regex crate: four literals, compared
    // case-insensitively, is the whole rule. `api-key` subsumes `x-api-key`,
    // which is why the measured list only needs three tests here.
    let said = value.to_ascii_lowercase();
    ["authorization", "api-key", "bearer"]
        .iter()
        .any(|mark| said.contains(mark))
}

/// One account this window knows about.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaudeAccount {
    /// Ours, and stable — the directory is named after it.
    pub id: String,
    /// What the CLI reported after logging in. The account's face in the UI.
    pub email: String,
    /// Which organisation this login is for. Two logins with one email in two
    /// organisations are two accounts, which is why this is part of identity
    /// rather than decoration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub organization_uuid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub organization_name: Option<String>,
    /// Stable person identifier reported by the CLI, independent of email.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_uuid: Option<String>,
    /// CLI organizationType; translated by the renderer's shared type table.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub organization_type: Option<String>,
    /// A different login awaiting the person's explicit choice.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending: Option<ClaudeIdentity>,
    /// Absolute path of the directory holding this account's credentials.
    pub config_dir: String,
    /// Epoch milliseconds.
    pub added_at: i64,
}

impl ClaudeAccount {
    /// Whether an observation contradicts an identity we already know.
    /// Missing identifiers cannot prove a change or erase known identifiers.
    pub fn identity_changed(&self, identity: &ClaudeIdentity) -> bool {
        [
            (self.organization_uuid.as_deref(), identity.organization_uuid.as_deref()),
            (self.account_uuid.as_deref(), identity.account_uuid.as_deref()),
        ].into_iter().any(|(old, new)| matches!((tidy_id(old), tidy_id(new)), (Some(old), Some(new)) if old != new))
    }

    /// Keep changed logins pending; enrich the record only for the same login.
    pub fn observe_identity(&mut self, identity: ClaudeIdentity) {
        if !identity.is_namable() {
            return;
        }
        if self.identity_changed(&identity) {
            self.pending = Some(identity);
        } else if self.pending.is_none() {
            self.accept_identity(identity);
        }
    }

    /// Adopt a login after an explicit choice (or a matching observation).
    pub fn accept_identity(&mut self, identity: ClaudeIdentity) {
        if self.identity_changed(&identity) {
            self.account_uuid = None;
            self.organization_uuid = None;
            self.organization_name = None;
            self.organization_type = None;
        }
        if let Some(email) = identity.email {
            self.email = email;
        }
        if identity.account_uuid.is_some() {
            self.account_uuid = identity.account_uuid;
        }
        if identity.organization_uuid.is_some() {
            self.organization_uuid = identity.organization_uuid;
        }
        if identity.organization_name.is_some() {
            self.organization_name = identity.organization_name;
        }
        if identity.organization_type.is_some() {
            self.organization_type = identity.organization_type;
        }
        self.pending = None;
    }

    /// The name a row shows: the email, with the organisation when there is
    /// one — because that is exactly the case where two rows share an email.
    pub fn label(&self) -> String {
        match self
            .organization_name
            .as_deref()
            .filter(|one| !one.is_empty())
        {
            Some(org) => format!("{} · {org}", self.email),
            None => self.email.clone(),
        }
    }
}

/// Identity as the CLI reported it, before it becomes an account.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaudeIdentity {
    #[serde(default)]
    pub account_uuid: Option<String>,
    #[serde(default)]
    pub organization_type: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub organization_uuid: Option<String>,
    #[serde(default)]
    pub organization_name: Option<String>,
}

fn tidy(value: Option<&str>) -> Option<String> {
    let said = value?.trim();
    (!said.is_empty()).then(|| said.to_string())
}

impl ClaudeIdentity {
    /// Read an identity out of the CLI's credentials JSON.
    ///
    /// Orca reads three sources in order and takes the first that answers
    /// (`resolveIdentity`, :219969): the `claude` status output, an
    /// `oauthAccount` blob, and the credentials themselves. This reads the
    /// last, which is the one that is always on disk after a login — and it
    /// tries both spellings of the email field, because the blob has carried
    /// `emailAddress` as well as `email`.
    pub fn from_credentials(json: &str) -> Self {
        let Ok(parsed) = serde_json::from_str::<serde_json::Value>(json) else {
            return Self::default();
        };
        // `oauthAccount` sits beside `claudeAiOauth`; either may carry the
        // identity depending on how the CLI last wrote the file.
        let oauth = parsed.get("claudeAiOauth");
        let account = parsed.get("oauthAccount");
        let read = |key: &str| {
            [account, oauth, Some(&parsed)]
                .into_iter()
                .flatten()
                .find_map(|held| tidy(held.get(key).and_then(serde_json::Value::as_str)))
        };
        Self {
            account_uuid: read("accountUuid").or_else(|| read("accountId")),
            organization_type: read("organizationType"),
            email: read("emailAddress").or_else(|| read("email")),
            organization_uuid: read("organizationUuid")
                .or_else(|| read("organizationId"))
                .or_else(|| read("orgId")),
            organization_name: read("organizationName").or_else(|| read("orgName")),
        }
    }

    /// Is this identity usable as an account? Orca refuses a login it could not
    /// resolve an email for, and so does this: an account with no name is a row
    /// nobody can tell from the next one.
    pub fn is_namable(&self) -> bool {
        self.email
            .as_deref()
            .is_some_and(|one| !one.trim().is_empty())
    }
}

/// An absent organisation and an empty one are the same organisation — which
/// matters because the CLI writes both depending on the login.
fn tidy_id(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|one| !one.is_empty())
}

fn same_id(left: Option<&str>, right: Option<&str>) -> bool {
    tidy_id(left) == tidy_id(right)
}

/// Is this login already here? Known account UUIDs take precedence over email.
/// Legacy records fall back to email AND organisation
/// (`findDuplicateClaudeAccount`), which is what lets one person hold a
/// personal and a work login under the same address.
///
/// Case-insensitive on the email, because an address is.
pub fn duplicate_of<'a>(
    accounts: &'a [ClaudeAccount],
    identity: &ClaudeIdentity,
) -> Option<&'a ClaudeAccount> {
    let email = identity.email.as_deref()?.trim().to_ascii_lowercase();
    if email.is_empty() {
        return None;
    }
    accounts.iter().find(|account| {
        let same_person = match (
            tidy_id(account.account_uuid.as_deref()),
            tidy_id(identity.account_uuid.as_deref()),
        ) {
            (Some(left), Some(right)) => left == right,
            _ => account.email.trim().to_ascii_lowercase() == email,
        };
        same_person
            && same_id(
                account.organization_uuid.as_deref(),
                identity.organization_uuid.as_deref(),
            )
    })
}

/// Which account the next launch runs as, and what that means for its
/// environment.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountSelection {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active: Option<String>,
    /// The person asked for this machine's own login — the original's
    /// 「시스템 기본값」 row.
    ///
    /// A flag rather than "no id", because those are different answers and were
    /// being given the same one. An empty selection means "nothing chosen yet",
    /// and falling back to the first account for it is right. This means
    /// "chosen: none of them", and the fallback must not fire — otherwise the
    /// row cannot be selected at all, which is exactly how it behaved.
    #[serde(default, skip_serializing_if = "is_not_set")]
    pub system_default: bool,
}

fn is_not_set(flag: &bool) -> bool {
    !*flag
}

/// The account a launch should use, from a selection that may name one that has
/// since been removed.
///
/// Falls back to the first rather than to nothing: a window whose selection
/// points at a deleted account should behave like a window with one account,
/// not like a window with none.
///
/// Unless the person asked for the machine's own login, which is a different
/// statement from an empty selection and the one case where "none" is the
/// answer. The original keeps the same distinction — its selection is `null`
/// for that row and it takes the restore path rather than a fallback
/// (`runtime-auth-service.ts:230-253`).
pub fn active_account<'a>(
    accounts: &'a [ClaudeAccount],
    selection: &AccountSelection,
) -> Option<&'a ClaudeAccount> {
    if selection.system_default {
        return None;
    }
    let named = selection
        .active
        .as_deref()
        .and_then(|id| accounts.iter().find(|account| account.id == id));
    named.or_else(|| accounts.first())
}

/// The environment a launch gets once an account is selected: every overriding
/// variable cleared, and one caller-chosen runtime config directory.
///
/// **An account is a login, not a workspace.** This used to name the account's
/// own directory in [`CONFIG_DIR_VAR`], and the CLI keeps its conversations in
/// that directory too — so switching accounts switched the drawer the person's
/// history was read from, and their conversations vanished. Reported exactly
/// that way: "계정을 바꾸면 세션이 공유가 안됨".
///
/// The important invariant is one runtime directory for every account. The
/// shell caller owns that directory below Zerocode's config root, so sessions
/// stay shared inside the app without changing a `claude` launched in another
/// terminal. What switches is the secure credential store, not the runtime
/// home itself.
///
/// The clears stay, and they are the half that is easy to leave out and
/// impossible to notice: a shell with `ANTHROPIC_API_KEY` exported ignores
/// every one of these answers, so without them an account picker is a control
/// that changes nothing. Cleared entries are returned as empty strings — the
/// spawn layer removes them, and returning them keeps this function pure.
///
/// `custom_headers` is the ambient value of [`CUSTOM_HEADERS_VAR`], passed in
/// rather than read here so this stays pure. It is cleared only when it carries
/// a credential; see [`headers_carry_auth`].
#[must_use]
pub fn launch_env(
    custom_headers: Option<&str>,
    runtime_config_dir: Option<&str>,
    secure_storage_config_dir: Option<&str>,
) -> Vec<(String, String)> {
    let mut env = Vec::new();
    if let Some(dir) = runtime_config_dir
        .map(str::trim)
        .filter(|dir| !dir.is_empty())
    {
        env.push((CONFIG_DIR_VAR.to_string(), dir.to_string()));
    }
    if let Some(dir) = secure_storage_config_dir
        .map(str::trim)
        .filter(|dir| !dir.is_empty())
    {
        env.push((SECURE_STORAGE_CONFIG_DIR_VAR.to_string(), dir.to_string()));
    }
    for name in OVERRIDING_AUTH_VARS {
        env.push(((*name).to_string(), String::new()));
    }
    if custom_headers.is_some_and(headers_carry_auth) {
        env.push((CUSTOM_HEADERS_VAR.to_string(), String::new()));
    }
    env
}

/// The keys in the CLI's settings file that decide whether it runs its
/// first-run flow, and the values it writes when somebody finishes that flow
/// (`hasCompletedOnboarding:!0,lastOnboardingVersion:VERSION`, claude 2.1.226,
/// in the `confirm:yes` handler of the welcome screen).
///
/// `theme` is here for the same reason: the very first screen of the flow is
/// the theme picker, and a settings file with no theme in it gets it.
///
/// `statusLine` is here because a managed HOME lacking it is the same defect
/// wearing different clothes, reported as "지금 우리쪽에서는 hud 표시같은게
/// 안나와 왜그런지도 확인 같은 cc임": the person's status line is configured in
/// their own settings file, the CLI reads that file from `CLAUDE_CONFIG_DIR`,
/// and a directory this window made has never had one. So the same CLI, in the
/// same repository, drew its HUD in the terminal next door and nothing here —
/// which reads as our terminal being less than the one it copies.
pub const FIRST_RUN_KEYS: &[&str] = &[
    "theme",
    "hasCompletedOnboarding",
    "lastOnboardingVersion",
    "statusLine",
];

/// Carry the person's own first-run answers into a managed config directory.
///
/// **The bug this fixes**, reported as "계정을 4개나 연결해 뒀는데 터미널에서
/// `claude`를 치면 OAuth 인증이 뜬다": an account is a `CLAUDE_CONFIG_DIR`, and
/// the CLI keeps its SETTINGS in that directory too — not only its credentials.
/// A directory that only ever had `auth login` run against it is, to every
/// later `claude`, a machine the person has never used: no theme, no completed
/// onboarding, so it opens the welcome flow. The login is fine — `auth status`
/// in that same directory answers with the right email — but nobody gets past
/// the first screen to see that, and a welcome flow with a login step in it
/// reads exactly like "the account did not connect".
///
/// The rule is narrow on purpose: **only what the person has already answered
/// on this machine about how the CLI looks to them, and only into a key the
/// managed directory does not have.**
/// If their own `~/.claude.json` does not say onboarding was completed, this
/// does nothing and their first run stays their first run — that flow is the
/// CLI's to show, not ours to skip. Nothing outside [`FIRST_RUN_KEYS`] is ever
/// copied, so no credential, no project list, and no trust decision moves: a
/// folder each account has to be trusted in separately is a security answer,
/// and giving it on somebody's behalf is not a papercut fix.
///
/// Returns whether `managed` changed, so the caller only writes when it did.
pub fn carry_first_run(managed: &mut serde_json::Value, home: &serde_json::Value) -> bool {
    // Onboarding completed over there is the whole licence for this. Asked of
    // the home file rather than of the app's own state because it is the
    // person's answer, and this window never asked them anything.
    // Onboarding completed over there is the whole licence for this. Asked of
    // the home file rather than of this window's own state because it is the
    // person's answer, and this window never asked them anything.
    if home.get("hasCompletedOnboarding") != Some(&serde_json::Value::Bool(true)) {
        return false;
    }
    let Some(held) = managed.as_object_mut() else {
        // Not an object — a file we do not understand, and merging into one of
        // those is how a settings file gets destroyed.
        return false;
    };
    let mut carried = false;
    for key in FIRST_RUN_KEYS {
        // Never an overwrite. A theme chosen inside this account is that
        // account's, and a second launch must not reset it to the home one.
        if held.contains_key(*key) {
            continue;
        }
        let Some(value) = home.get(*key) else {
            continue;
        };
        held.insert((*key).to_string(), value.clone());
        carried = true;
    }
    carried
}

/// One provider whose selected account can be handed to an agent process.
///
/// The enum describes the credential lane, not the implementation that owns
/// it: Anthropic is materialized by `accounts`, OpenAI by `codex_accounts`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    Anthropic,
    OpenAi,
}

const CLAUDE_PROVIDERS: &[Provider] = &[Provider::Anthropic];
const CODEX_PROVIDERS: &[Provider] = &[Provider::OpenAi];
const ZO_PROVIDERS: &[Provider] = &[Provider::Anthropic, Provider::OpenAi];

/// Every selected account provider a launched agent consumes.
///
/// This is the single routing table for account materialization, launch locks,
/// and exit-time token write-back. Google is absent because Zo and the window
/// already share its credential file directly; it has no launch environment.
#[must_use]
pub fn providers_for(agent: &str) -> &'static [Provider] {
    match agent {
        "claude" => CLAUDE_PROVIDERS,
        "codex" => CODEX_PROVIDERS,
        "zo" => ZO_PROVIDERS,
        _ => &[],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_providers_are_declared_once_per_agent() {
        assert_eq!(providers_for("claude"), &[Provider::Anthropic]);
        assert_eq!(providers_for("codex"), &[Provider::OpenAi]);
        assert_eq!(
            providers_for("zo"),
            &[Provider::Anthropic, Provider::OpenAi]
        );
        assert!(providers_for("opencode").is_empty());
    }

    fn account(id: &str, email: &str, org: Option<&str>) -> ClaudeAccount {
        ClaudeAccount {
            id: id.into(),
            email: email.into(),
            organization_uuid: org.map(str::to_string),
            organization_name: None,
            config_dir: format!("/state/accounts/{id}"),
            added_at: 0,
            ..ClaudeAccount::default()
        }
    }

    #[test]
    fn observations_keep_known_organizations_pending_and_never_erase_identifiers() {
        let mut held = account("old", "same@example.test", Some("team"));
        held.account_uuid = Some("person".into());
        let before = held.clone();
        let changed = ClaudeIdentity {
            email: Some(held.email.clone()),
            account_uuid: Some("person".into()),
            organization_uuid: Some("max".into()),
            organization_type: Some("claude_max".into()),
            ..ClaudeIdentity::default()
        };
        held.observe_identity(changed.clone());
        assert_eq!(held.pending, Some(changed.clone()));
        let mut old = held.clone();
        old.pending = None;
        assert_eq!(old, before);
        held.accept_identity(changed);
        assert_eq!(held.organization_uuid.as_deref(), Some("max"));
        assert!(held.pending.is_none());
        held.observe_identity(ClaudeIdentity {
            email: Some(held.email.clone()),
            ..ClaudeIdentity::default()
        });
        assert_eq!(held.organization_uuid.as_deref(), Some("max"));
        assert_eq!(held.account_uuid.as_deref(), Some("person"));
    }

    #[test]
    fn old_account_records_deserialize_without_pending_or_uuid_metadata() {
        let held: ClaudeAccount = serde_json::from_str(
            r#"{"id":"a","email":"a@b.test","config_dir":"/fixture","added_at":0}"#,
        )
        .unwrap();
        assert!(
            held.pending.is_none()
                && held.account_uuid.is_none()
                && held.organization_type.is_none()
        );
    }

    #[test]
    fn cli_identity_retains_account_uuid_and_organization_type() {
        let identity = ClaudeIdentity::from_credentials(
            r#"{"oauthAccount":{"emailAddress":"a@b.test","accountUuid":"person","organizationUuid":"org","organizationType":"claude_team"}}"#,
        );
        let row = serde_json::to_value(identity).unwrap();
        assert_eq!(row["account_uuid"], "person");
        assert_eq!(row["organization_type"], "claude_team");
    }

    #[test]
    fn an_identity_is_read_from_the_credentials_the_cli_wrote() {
        let identity = ClaudeIdentity::from_credentials(
            r#"{"claudeAiOauth":{"accessToken":"x","emailAddress":"joe@example.com",
                "organizationUuid":"org-1","organizationName":"Example Inc"}}"#,
        );
        assert_eq!(identity.email.as_deref(), Some("joe@example.com"));
        assert_eq!(identity.organization_uuid.as_deref(), Some("org-1"));
        assert_eq!(identity.organization_name.as_deref(), Some("Example Inc"));
        assert!(identity.is_namable());
    }

    #[test]
    fn both_spellings_of_the_email_field_are_read() {
        // The blob has carried `email` as well as `emailAddress`; a reader that
        // knows one of them turns a good login into "could not resolve email".
        let plain = ClaudeIdentity::from_credentials(
            r#"{"oauthAccount":{"email":"a@b.test","organizationId":"org-2"}}"#,
        );
        assert_eq!(plain.email.as_deref(), Some("a@b.test"));
        assert_eq!(plain.organization_uuid.as_deref(), Some("org-2"));
    }

    #[test]
    fn a_login_with_no_email_is_not_an_account() {
        for hopeless in [
            "{}",
            "not json",
            r#"{"claudeAiOauth":{"accessToken":"x"}}"#,
            r#"{"claudeAiOauth":{"email":"   "}}"#,
        ] {
            assert!(
                !ClaudeIdentity::from_credentials(hopeless).is_namable(),
                "`{hopeless}` was accepted as an account"
            );
        }
    }

    #[test]
    fn one_address_in_two_organisations_is_two_accounts() {
        // The case the organisation is part of identity FOR: a personal login
        // and a work login under the same address.
        let held = vec![account("a", "joe@example.com", Some("org-personal"))];
        let work = ClaudeIdentity {
            email: Some("joe@example.com".into()),
            organization_uuid: Some("org-work".into()),
            organization_name: None,
            ..ClaudeIdentity::default()
        };
        assert!(duplicate_of(&held, &work).is_none());

        let same = ClaudeIdentity {
            email: Some("JOE@Example.com".into()),
            organization_uuid: Some("org-personal".into()),
            organization_name: None,
            ..ClaudeIdentity::default()
        };
        // And the address is compared as an address: case does not make a
        // second account.
        assert_eq!(
            duplicate_of(&held, &same).map(|one| one.id.as_str()),
            Some("a")
        );
    }

    #[test]
    fn a_selection_pointing_at_a_removed_account_falls_back_rather_than_off() {
        let held = vec![
            account("a", "one@example.com", None),
            account("b", "two@example.com", None),
        ];
        let gone = AccountSelection {
            active: Some("deleted".into()),
            system_default: false,
        };
        assert_eq!(
            active_account(&held, &gone).map(|one| one.id.as_str()),
            Some("a"),
            "a stale selection left the window with no account"
        );
        let chosen = AccountSelection {
            active: Some("b".into()),
            system_default: false,
        };
        assert_eq!(
            active_account(&held, &chosen).map(|one| one.id.as_str()),
            Some("b")
        );
        assert!(active_account(&[], &chosen).is_none());
    }

    /// Every selected account names the one shared runtime — and clears
    /// everything that would ignore it.
    #[test]
    fn a_launch_names_one_runtime_and_clears_what_would_ignore_it() {
        let env = launch_env(
            None,
            Some("  /app/zerocode/.claude  "),
            Some("  /app/zerocode/accounts/a-1  "),
        );
        assert_eq!(
            env.iter()
                .find(|(key, _)| key == CONFIG_DIR_VAR)
                .map(|(_, value)| value.as_str()),
            Some("/app/zerocode/.claude")
        );
        assert_eq!(
            env.iter()
                .find(|(key, _)| key == SECURE_STORAGE_CONFIG_DIR_VAR)
                .map(|(_, value)| value.as_str()),
            Some("/app/zerocode/accounts/a-1")
        );
        // An empty one is not a setting.
        assert!(
            !launch_env(None, Some("   "), Some("   "))
                .iter()
                .any(|(key, _)| { key == CONFIG_DIR_VAR || key == SECURE_STORAGE_CONFIG_DIR_VAR })
        );
        // The half that is easy to omit and impossible to notice: a shell with
        // ANTHROPIC_API_KEY exported makes the picker a control that does
        // nothing.
        for name in OVERRIDING_AUTH_VARS {
            let held = env
                .iter()
                .find(|(key, _)| key == name)
                .unwrap_or_else(|| panic!("`{name}` is not cleared"));
            assert!(held.1.is_empty(), "`{name}` is set rather than cleared");
        }
    }

    #[test]
    fn a_custom_header_is_cleared_when_it_carries_a_credential_and_kept_when_it_does_not() {
        let cleared = |headers: Option<&str>| {
            launch_env(headers, None, None)
                .iter()
                .any(|(key, value)| key == CUSTOM_HEADERS_VAR && value.is_empty())
        };
        // A header spelling a credential beats the config directory exactly as
        // the environment variable does.
        assert!(cleared(Some("X-Api-Key: sk-abc")));
        assert!(cleared(Some("Authorization: Bearer xyz")));
        assert!(cleared(Some("authorization: token")));
        // And one that does not is left alone: somebody set it on purpose, for
        // a reason that has nothing to do with which account they are on.
        assert!(!cleared(Some("X-Trace-Id: 42")));
        assert!(!cleared(Some("anthropic-beta: prompt-caching")));
        assert!(!cleared(Some("")));
        assert!(!cleared(None));
    }

    #[test]
    fn a_row_says_the_organisation_only_when_there_is_one() {
        let mut one = account("a", "joe@example.com", Some("org"));
        assert_eq!(one.label(), "joe@example.com");
        one.organization_name = Some("Example Inc".into());
        assert_eq!(one.label(), "joe@example.com · Example Inc");
        // An empty name is not a name.
        one.organization_name = Some(String::new());
        assert_eq!(one.label(), "joe@example.com");
    }

    /// A managed directory is a whole Claude Code home, not a credential store,
    /// and one that has never been run in opens the welcome flow. This is the
    /// reported "OAuth 인증이 뜬다" with four accounts connected.
    #[test]
    fn an_account_directory_inherits_the_first_run_answers_the_person_already_gave() {
        let home = serde_json::json!({
            "theme": "dark-daltonized",
            "hasCompletedOnboarding": true,
            "lastOnboardingVersion": "2.1.226",
            "oauthAccount": {"emailAddress": "home@example.com"},
            "projects": {"/work": {"hasTrustDialogAccepted": true}},
        });
        // What a directory looks like after `auth login` and nothing else.
        let mut managed = serde_json::json!({
            "oauthAccount": {"emailAddress": "work@example.com"},
            "userID": "u-1",
        });
        assert!(carry_first_run(&mut managed, &home));
        assert_eq!(managed["theme"], "dark-daltonized");
        assert_eq!(managed["hasCompletedOnboarding"], true);
        assert_eq!(managed["lastOnboardingVersion"], "2.1.226");
        // This account's own login stays this account's, and nothing outside
        // the first-run keys travels: the home identity is a DIFFERENT person's
        // credentials as far as this directory is concerned, and a trust
        // decision is an answer to a security question, given per account.
        assert_eq!(managed["oauthAccount"]["emailAddress"], "work@example.com");
        assert!(managed.get("projects").is_none());
        // Idempotent — a second launch has nothing to carry and writes nothing.
        assert!(!carry_first_run(&mut managed, &home));
    }

    #[test]
    fn nothing_is_carried_when_the_person_has_not_answered_yet() {
        let mut managed = serde_json::json!({});
        // No home file at all, and a home file that says the flow is unfinished:
        // in both cases their first run IS their first run, and the CLI's
        // welcome flow is the CLI's to show.
        for home in [
            serde_json::json!({}),
            serde_json::json!({"theme": "dark", "hasCompletedOnboarding": false}),
        ] {
            assert!(!carry_first_run(&mut managed, &home));
            assert_eq!(managed, serde_json::json!({}));
        }
    }

    #[test]
    fn a_choice_made_inside_the_account_is_never_overwritten() {
        let home = serde_json::json!({"theme": "light", "hasCompletedOnboarding": true});
        let mut managed = serde_json::json!({"theme": "dark"});
        // Only the missing key is filled. Carrying the home theme over an
        // account's own would undo a `/theme` the person ran in that account.
        assert!(carry_first_run(&mut managed, &home));
        assert_eq!(managed["theme"], "dark");
        assert_eq!(managed["hasCompletedOnboarding"], true);
        // And a file that is not an object is left exactly as it is rather than
        // replaced — merging into something we do not understand destroys it.
        let mut foreign = serde_json::json!(["not", "a", "config"]);
        assert!(!carry_first_run(&mut foreign, &home));
        assert_eq!(foreign, serde_json::json!(["not", "a", "config"]));
    }
}
