//! More than one Codex account, and which one an agent runs as.
//!
//! The Claude half is in [`crate::account`]; this is the same idea against a
//! different CLI, and it is a different module rather than a flag because the
//! two mechanisms only *look* alike. Measured from Orca **1.4.164**'s
//! `CodexAccountService` (out/main/index.js:215690-216480) and its env patch
//! (:33698-33740).
//!
//! What is the same: an account is a DIRECTORY, adding one is running the CLI's
//! own login against that directory, and switching is naming a different one on
//! the next launch. The variable is `CODEX_HOME` rather than
//! `CLAUDE_CONFIG_DIR`.
//!
//! What is different, and why this is not a parameter on the other module:
//!
//! 1. **Where the name comes from.** Claude writes `oauthAccount` into a
//!    settings file as plain fields. Codex writes `auth.json` whose `tokens.
//!    id_token` is a JWT, and the email is a CLAIM inside it — so naming an
//!    account means decoding a token payload rather than reading a field.
//! 2. **Two kinds of login.** `auth.json` may hold an `OPENAI_API_KEY` instead
//!    of OAuth tokens, and that account HAS no email. Orca reports it as
//!    `authKind: "api-key"` and shows it without one (:215869).
//! 3. **The managed home is a whole home**, config included — Orca seeds the
//!    person's own `config.toml` into it and re-syncs it
//!    (`syncCanonicalConfigIntoManagedHome`, :216018). A managed home with no
//!    config is a Codex that has lost the person's settings, which is the same
//!    failure the Claude half had with onboarding.
//!
//! This module is the pure part: identity out of an `auth.json`, and the
//! environment an account resolves to. The directories, the login subprocess
//! and the storage are the shell's. **No credential is ever read for anything
//! but the name** — the access and refresh tokens are not parsed, not kept, and
//! not returned.

use serde::{Deserialize, Serialize};

/// The variable that decides which home the Codex CLI uses.
pub const HOME_VAR: &str = "CODEX_HOME";

/// The file the CLI writes its login into, inside that home.
pub const AUTH_FILE: &str = "auth.json";

/// Where the window remembers the accounts it holds and which one is chosen —
/// one file in the app's data folder ([`crate::app::IDENTIFIER`]).
///
/// Spelled here because it is read from BOTH sides of the window/agent
/// boundary: the window writes it, and a zo running outside a pane reads it to
/// find the account the person is signed in to (t-5777).
pub const STORE_FILE: &str = "codex-accounts.json";

/// The shared Codex home every ZeroCode pane runs in, below the app's data
/// folder: `codex-runtime-home/home`.
///
/// Two segments, so no caller can accidentally name the person's own
/// `~/.codex` — the invariant `zerocode_hookd::codex_mirror::Home` is built on,
/// and the fallback a zo outside a pane borrows when no account is selected.
pub const RUNTIME_HOME_SEGMENTS: [&str; 2] = ["codex-runtime-home", "home"];

/// How an account is signed in.
///
/// Orca's `authKind` (:215822). The distinction is not decoration: an API-key
/// login has no email to show, so a row that insisted on one would call a
/// perfectly good account broken.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AuthKind {
    /// OAuth through the browser — the one with an email in it.
    Oauth,
    /// `OPENAI_API_KEY` in the auth file. Signed in, and nameless.
    ApiKey,
    /// The file is there but says neither.
    None,
}

/// Who logged in, as far as an `auth.json` can say.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodexIdentity {
    #[serde(default)]
    pub email: Option<String>,
    /// The ChatGPT account this login belongs to (`tokens.account_id`, else the
    /// `chatgpt_account_id` claim).
    #[serde(default)]
    pub provider_account_id: Option<String>,
    /// The workspace's display name, when the token names one.
    #[serde(default)]
    pub workspace_label: Option<String>,
}

impl CodexIdentity {
    /// Is this identity enough to put a name on a row?
    ///
    /// An API-key login is signed in and has no email, so this is asked only
    /// where a NAME is needed — never as "is this account usable".
    pub fn is_namable(&self) -> bool {
        self.email
            .as_deref()
            .is_some_and(|one| !one.trim().is_empty())
    }

    /// The face a row shows: the email, with the workspace when there is one —
    /// because that is exactly the case where two rows share an address.
    pub fn label(&self) -> Option<String> {
        let email = self.email.as_deref()?.trim();
        if email.is_empty() {
            return None;
        }
        match self
            .workspace_label
            .as_deref()
            .map(str::trim)
            .filter(|one| !one.is_empty())
        {
            Some(workspace) => Some(format!("{email} · {workspace}")),
            None => Some(email.to_string()),
        }
    }
}

fn tidy(value: Option<&str>) -> Option<String> {
    let said = value?.trim();
    (!said.is_empty()).then(|| said.to_string())
}

/// Decode a JWT's payload without verifying it.
///
/// **Unverified on purpose, and safe because of what it is used for.** This
/// token was written by the CLI into a file in the person's own home, and the
/// only thing taken out of it is a string to print on a row. Nothing is
/// authorised by it and nothing is sent anywhere. Verifying it would mean
/// fetching a signing key over the network to decide what to draw in a list,
/// which is a worse trade in every direction. Orca reads it the same way
/// (`parseJwtPayload`, :216452 — a base64url split, no verification).
fn jwt_payload(token: &str) -> Option<serde_json::Value> {
    let mut parts = token.split('.');
    let _header = parts.next()?;
    let payload = parts.next()?;
    // A JWT must have three segments; two would be a different thing that
    // happens to split the same way.
    parts.next()?;
    let bytes = base64url(payload)?;
    serde_json::from_slice(&bytes).ok()
}

/// base64url without padding, decoded by hand.
///
/// Four characters in, three bytes out, `-` and `_` for `+` and `/`. Written
/// here rather than pulled in as a dependency: this is the only base64 in the
/// workspace, and a decoder that refuses anything it does not recognise is
/// exactly what should be reading a file to print a name from.
fn base64url(text: &str) -> Option<Vec<u8>> {
    let value_of = |byte: u8| -> Option<u32> {
        Some(match byte {
            b'A'..=b'Z' => u32::from(byte - b'A'),
            b'a'..=b'z' => u32::from(byte - b'a') + 26,
            b'0'..=b'9' => u32::from(byte - b'0') + 52,
            b'-' => 62,
            b'_' => 63,
            _ => return None,
        })
    };
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let mut held: u32 = 0;
    let mut bits = 0u32;
    for byte in text.bytes() {
        // Padding is not part of base64url, but a token that carries it is
        // still a token — and it only ever appears at the end.
        if byte == b'=' {
            break;
        }
        held = (held << 6) | value_of(byte)?;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((held >> bits) & 0xff) as u8);
        }
    }
    Some(out)
}

fn claim<'a>(payload: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    payload.get(key)?.as_str()
}

/// Read an identity out of the CLI's `auth.json`.
///
/// The claims and their fallbacks are Orca's, in Orca's order
/// (`resolveIdentityFromCredentials`, :216451): the email from the token's own
/// `email` claim or the profile claim block, the account id from
/// `tokens.account_id` or the auth claim block, the workspace name from either
/// claim block.
pub fn identity_from_auth(json: &str) -> (AuthKind, CodexIdentity) {
    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(json) else {
        return (AuthKind::None, CodexIdentity::default());
    };
    // An API key beats the tokens, exactly as it does inside the CLI — and it
    // has no email in it, which is a fact about the login and not a failure to
    // read one.
    if tidy(parsed.get(API_KEY_VAR).and_then(|one| one.as_str())).is_some() {
        return (AuthKind::ApiKey, CodexIdentity::default());
    }
    let tokens = parsed.get("tokens");
    let payload = tokens
        .and_then(|held| held.get("id_token"))
        .and_then(|held| held.as_str())
        .and_then(jwt_payload);
    let Some(payload) = payload else {
        return (AuthKind::None, CodexIdentity::default());
    };
    let auth_claims = payload.get("https://api.openai.com/auth");
    let profile_claims = payload.get("https://api.openai.com/profile");
    fn from_block<'a>(block: Option<&'a serde_json::Value>, key: &str) -> Option<&'a str> {
        block?.get(key)?.as_str()
    }
    let identity = CodexIdentity {
        email: tidy(claim(&payload, "email").or_else(|| from_block(profile_claims, "email"))),
        provider_account_id: tidy(
            tokens
                .and_then(|held| held.get("account_id"))
                .and_then(|held| held.as_str())
                .or_else(|| from_block(auth_claims, "chatgpt_account_id"))
                .or_else(|| claim(&payload, "chatgpt_account_id")),
        ),
        workspace_label: tidy(
            from_block(auth_claims, "workspace_name")
                .or_else(|| from_block(profile_claims, "workspace_name")),
        ),
    };
    (AuthKind::Oauth, identity)
}

/// One Codex account this window knows about.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodexAccount {
    pub id: String,
    /// May be absent: an API-key login has no email, and Orca shows it anyway.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_account_id: Option<String>,
    /// Absolute path of the managed home. `CODEX_HOME` is set to exactly this.
    pub home_dir: String,
    pub added_at: i64,
}

impl CodexAccount {
    /// What a row says. An account with no email is named by its workspace, and
    /// failing that by the key it signed in with — never by nothing.
    pub fn label(&self) -> String {
        let identity = CodexIdentity {
            email: self.email.clone(),
            provider_account_id: self.provider_account_id.clone(),
            workspace_label: self.workspace_label.clone(),
        };
        identity
            .label()
            .or_else(|| self.workspace_label.clone())
            .unwrap_or_else(|| "API 키 로그인".to_string())
    }
}

/// Is this login already here?
///
/// By email AND workspace when there is an email — one person's personal and
/// work logins share an address, which is the case the second half exists for.
/// An API-key login has neither, so it is matched by its provider account id
/// and otherwise treated as new.
pub fn duplicate_of<'a>(
    accounts: &'a [CodexAccount],
    identity: &CodexIdentity,
) -> Option<&'a CodexAccount> {
    let same = |left: Option<&str>, right: Option<&str>| {
        left.map(|one| one.trim().to_ascii_lowercase())
            == right.map(|one| one.trim().to_ascii_lowercase())
    };
    if let Some(email) = identity.email.as_deref() {
        return accounts.iter().find(|account| {
            same(account.email.as_deref(), Some(email))
                && same(
                    account.workspace_label.as_deref(),
                    identity.workspace_label.as_deref(),
                )
        });
    }
    let id = identity.provider_account_id.as_deref()?;
    accounts
        .iter()
        .find(|account| same(account.provider_account_id.as_deref(), Some(id)))
}

/// Which account the next launch runs as.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodexSelection {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active: Option<String>,
}

/// The account a launch should use.
///
/// Unlike the Claude half this does NOT fall back to the first account. Orca
/// keeps a real "system default" for Codex — `activeAccountId: null` means the
/// machine's own `~/.codex`, read live and never written (:215812). A selection
/// naming a removed account therefore means "back to the machine's own login",
/// which is a state a person can actually be in here.
pub fn active_account<'a>(
    accounts: &'a [CodexAccount],
    selection: &CodexSelection,
) -> Option<&'a CodexAccount> {
    let named = selection.active.as_deref()?;
    accounts.iter().find(|account| account.id == named)
}

/// The environment a launch gets for this account.
///
/// One variable. Orca's Codex env patch is `CODEX_HOME` and its own
/// `ORCA_CODEX_HOME` marker (:33703) — the marker is theirs, for their WSL path
/// rewriting, and copying a competitor's variable name into our launches would
/// be putting their trademark in our product for no function.
///
/// Deliberately no clearing of `OPENAI_API_KEY`: the Claude half strips the
/// overriding variables because Orca strips them there, and Orca does not strip
/// this one here. A key in the person's shell is how a great many Codex setups
/// work, and removing it because they added an account would break the login
/// they were using before they touched anything.
pub fn launch_env(account: &CodexAccount) -> Vec<(String, String)> {
    vec![(HOME_VAR.to_string(), account.home_dir.clone())]
}

/// The API-key login's name — the key in `auth.json` and the variable in a
/// person's shell, which the launch above deliberately leaves standing. The
/// readiness probe reads the variable as a witness for the same reason.
pub const API_KEY_VAR: &str = "OPENAI_API_KEY";

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a token the way the CLI's is built: three base64url segments,
    /// claims in the middle. The signature is never looked at, so it is a word.
    fn token(payload: serde_json::Value) -> String {
        let encode = |bytes: &[u8]| {
            const ALPHABET: &[u8] =
                b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
            let mut out = String::new();
            for chunk in bytes.chunks(3) {
                let mut held = 0u32;
                for (at, byte) in chunk.iter().enumerate() {
                    held |= u32::from(*byte) << (16 - 8 * at);
                }
                let take = (chunk.len() * 8).div_ceil(6);
                for at in 0..take {
                    out.push(ALPHABET[((held >> (18 - 6 * at)) & 0x3f) as usize] as char);
                }
            }
            out
        };
        format!(
            "eyJhbGciOiJSUzI1NiJ9.{}.signature-not-read",
            encode(payload.to_string().as_bytes())
        )
    }

    #[test]
    fn a_name_is_read_out_of_the_token_the_cli_wrote() {
        let auth = serde_json::json!({
            "tokens": {
                "id_token": token(serde_json::json!({
                    "email": "joe@example.com",
                    "https://api.openai.com/auth": {
                        "chatgpt_account_id": "acct-1",
                        "workspace_name": "Example Inc",
                    },
                })),
                "account_id": "acct-1",
                "access_token": "secret-do-not-read",
                "refresh_token": "secret-do-not-read",
            },
        })
        .to_string();
        let (kind, identity) = identity_from_auth(&auth);
        assert_eq!(kind, AuthKind::Oauth);
        assert_eq!(identity.email.as_deref(), Some("joe@example.com"));
        assert_eq!(identity.provider_account_id.as_deref(), Some("acct-1"));
        assert_eq!(identity.workspace_label.as_deref(), Some("Example Inc"));
        assert_eq!(
            identity.label().as_deref(),
            Some("joe@example.com · Example Inc")
        );
        // The tokens themselves are not part of an identity, and this test
        // exists to keep it that way.
        let json = serde_json::to_string(&identity).expect("serialize");
        assert!(
            !json.contains("secret-do-not-read"),
            "a token travelled inside an identity: {json}"
        );
    }

    #[test]
    fn the_profile_claim_block_answers_when_the_plain_claim_does_not() {
        let auth = serde_json::json!({
            "tokens": {
                "id_token": token(serde_json::json!({
                    "https://api.openai.com/profile": {
                        "email": "late@example.com",
                        "workspace_name": "Second",
                    },
                })),
            },
        })
        .to_string();
        let (kind, identity) = identity_from_auth(&auth);
        assert_eq!(kind, AuthKind::Oauth);
        assert_eq!(identity.email.as_deref(), Some("late@example.com"));
        assert_eq!(identity.workspace_label.as_deref(), Some("Second"));
    }

    #[test]
    fn a_key_login_is_signed_in_and_has_no_name() {
        // The case a reader that demanded an email would call broken.
        let (kind, identity) = identity_from_auth(r#"{"OPENAI_API_KEY":"sk-abc","tokens":null}"#);
        assert_eq!(kind, AuthKind::ApiKey);
        assert!(!identity.is_namable());
        let account = CodexAccount {
            id: "a".into(),
            email: None,
            workspace_label: None,
            provider_account_id: None,
            home_dir: "/state/codex-accounts/a/home".into(),
            added_at: 0,
        };
        assert!(!account.label().is_empty(), "a nameless row said nothing");
    }

    #[test]
    fn a_file_that_says_nothing_is_not_a_login() {
        for hopeless in [
            "{}",
            "not json",
            r#"{"OPENAI_API_KEY":"   "}"#,
            r#"{"tokens":{"id_token":"not-a-jwt"}}"#,
            // Two segments split the same way a JWT does and are not one.
            r#"{"tokens":{"id_token":"aGVhZGVy.eyJlbWFpbCI6ImFAYi5jIn0"}}"#,
            // A payload that is not base64url at all.
            r#"{"tokens":{"id_token":"a.!!!!.c"}}"#,
        ] {
            let (kind, identity) = identity_from_auth(hopeless);
            assert_eq!(kind, AuthKind::None, "`{hopeless}` was read as a login");
            assert!(!identity.is_namable(), "`{hopeless}` produced a name");
        }
    }

    #[test]
    fn one_address_in_two_workspaces_is_two_accounts() {
        let held = vec![CodexAccount {
            id: "a".into(),
            email: Some("joe@example.com".into()),
            workspace_label: Some("Work".into()),
            provider_account_id: Some("acct-1".into()),
            home_dir: "/state/codex-accounts/a/home".into(),
            added_at: 0,
        }];
        let same = CodexIdentity {
            email: Some("JOE@example.com".into()),
            workspace_label: Some("Work".into()),
            provider_account_id: Some("acct-1".into()),
        };
        assert!(
            duplicate_of(&held, &same).is_some(),
            "an address is not case-sensitive"
        );
        let other = CodexIdentity {
            email: Some("joe@example.com".into()),
            workspace_label: Some("Personal".into()),
            provider_account_id: Some("acct-2".into()),
        };
        assert!(duplicate_of(&held, &other).is_none());
    }

    #[test]
    fn no_selection_means_the_machines_own_login_rather_than_the_first_row() {
        let held = vec![CodexAccount {
            id: "a".into(),
            email: Some("joe@example.com".into()),
            workspace_label: None,
            provider_account_id: None,
            home_dir: "/state/codex-accounts/a/home".into(),
            added_at: 0,
        }];
        // Orca keeps a real system default for Codex (`activeAccountId: null`,
        // read live from `~/.codex` and never written) — so an empty selection
        // is a state, not a gap to fill with whichever row happens to be first.
        assert!(active_account(&held, &CodexSelection::default()).is_none());
        let gone = CodexSelection {
            active: Some("removed".into()),
        };
        assert!(active_account(&held, &gone).is_none());
        let named = CodexSelection {
            active: Some("a".into()),
        };
        assert_eq!(
            active_account(&held, &named).map(|one| one.id.as_str()),
            Some("a")
        );
        assert_eq!(
            launch_env(&held[0]),
            vec![(
                "CODEX_HOME".to_string(),
                "/state/codex-accounts/a/home".to_string()
            )]
        );
    }
}
