//! Which app an agent means, and which apps no agent may touch.
//!
//! `resolveApp`/`matches`/`blockedBundleIds`/`isKnownBrowser` from the macOS
//! helper, with the Windows spellings of the same lists beside them: a
//! password manager is blocked by bundle id there and by executable name
//! here, and both refuse with `app_blocked`.

use super::{ProviderError, error_code};

/// `--app` as the agent typed it: a pid, or a name/identifier to match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppQuery {
    Pid(u32),
    Named(String),
}

impl AppQuery {
    /// Trims, refuses empty, reads `pid:N` (N > 0) as a pid.
    pub fn parse(raw: &str) -> Result<Self, ProviderError> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(ProviderError::invalid_argument(
                "app query must not be empty",
            ));
        }
        if let Some(pid) = trimmed.strip_prefix("pid:") {
            return match pid.parse::<u32>() {
                Ok(pid) if pid > 0 => Ok(Self::Pid(pid)),
                // `pid:` followed by nonsense is looked up as a name and not
                // found — the helper's `parsePid` answers nil the same way.
                _ => Ok(Self::Named(trimmed.to_string())),
            };
        }
        Ok(Self::Named(trimmed.to_string()))
    }

    /// The `app_not_found` refusal for this query.
    #[must_use]
    pub fn not_found(&self) -> ProviderError {
        let spelled = match self {
            Self::Pid(pid) => format!("pid:{pid}"),
            Self::Named(name) => name.clone(),
        };
        ProviderError::new(
            error_code::APP_NOT_FOUND,
            format!("app '{spelled}' not found"),
        )
    }
}

/// macOS: the `Info.plist` keys a bundle names itself by, unlocalized — the
/// `Calculator` a person types while the running app calls itself `계산기`.
/// The helper reads them in this order (`bundleNameKeys` in
/// `DesktopApps.swift`; a source contract holds the spelling), and `launch`
/// answers the first as `bundleName` so the next verb can ask by that word.
pub const MACOS_BUNDLE_NAME_KEYS: &[&str] = &["CFBundleDisplayName", "CFBundleName"];

/// `matches`: the name or the identifier, case-insensitively — or one of the
/// other names a person knows the app by, read off the app itself and never
/// guessed: on both platforms its executable's file stem (`notepad` for
/// `notepad.exe`; `Calculator` for `Calculator.app/Contents/MacOS/Calculator`),
/// and on macOS the bundle's own unlocalized names
/// ([`MACOS_BUNDLE_NAME_KEYS`]). Never a fragment: `note` is not `Notes`.
/// The Windows provider calls this with its stem; the macOS helper's
/// `applicationAnswers` (`DesktopApps.swift`) is the same rule in Swift.
#[must_use]
pub fn matches(
    query: &str,
    name: &str,
    bundle_id: Option<&str>,
    executable_stem: Option<&str>,
) -> bool {
    matches_any(query, name, bundle_id, executable_stem)
}

/// [`matches()`] with every other name the platform read off the app.
#[must_use]
pub fn matches_any<'a>(
    query: &str,
    name: &str,
    bundle_id: Option<&str>,
    other_names: impl IntoIterator<Item = &'a str>,
) -> bool {
    name.eq_ignore_ascii_case(query)
        || bundle_id.is_some_and(|id| id.eq_ignore_ascii_case(query))
        || other_names
            .into_iter()
            .any(|other| other.eq_ignore_ascii_case(query))
}

/// macOS: bundle identifiers the helper refuses.
pub const BLOCKED_BUNDLE_IDS: &[&str] = &[
    "com.1password.1password",
    "com.1password.safari",
    "com.bitwarden.desktop",
    "com.dashlane.dashlanephonefinal",
    "com.lastpass.LastPass",
    "com.nordsec.nordpass",
    "me.proton.pass.electron",
    "me.proton.pass.catalyst",
];

/// Windows: executable names (lowercase, with extension) of the same
/// products, plus the desktop password managers common there.
pub const BLOCKED_WINDOWS_EXECUTABLES: &[&str] = &[
    "1password.exe",
    "bitwarden.exe",
    "dashlane.exe",
    "lastpass.exe",
    "nordpass.exe",
    "proton pass.exe",
    "keepass.exe",
    "keepassxc.exe",
];

/// Windows: AppUserModelId prefixes of the packaged (Store) editions.
pub const BLOCKED_WINDOWS_AUMID_PREFIXES: &[&str] = &[
    "AgileBits.1Password",
    "8bitSolutionsLLC.bitwardendesktop",
    "Dashlane.Dashlane",
    "LastPass.LastPass",
    "NordSecurity.NordPass",
    "ProtonAG.ProtonPass",
];

/// The `app_blocked` refusal, worded as the helper words it.
#[must_use]
pub fn blocked(identity: &str) -> ProviderError {
    ProviderError::new(
        error_code::APP_BLOCKED,
        format!("app '{identity}' is blocked for safety"),
    )
}

#[must_use]
pub fn is_blocked_bundle_id(bundle_id: &str) -> bool {
    BLOCKED_BUNDLE_IDS.contains(&bundle_id)
}

/// Windows: blocked by executable file name or packaged identity.
#[must_use]
pub fn is_blocked_windows_app(executable_name: Option<&str>, aumid: Option<&str>) -> bool {
    executable_name
        .is_some_and(|name| BLOCKED_WINDOWS_EXECUTABLES.contains(&name.to_lowercase().as_str()))
        || aumid.is_some_and(|aumid| {
            BLOCKED_WINDOWS_AUMID_PREFIXES
                .iter()
                .any(|prefix| aumid.to_lowercase().starts_with(&prefix.to_lowercase()))
        })
}

/// Whether an app is a browser, which turns on tab-strip compaction.
#[must_use]
pub fn is_known_browser(
    name: &str,
    bundle_id: Option<&str>,
    executable_name: Option<&str>,
) -> bool {
    let bundle = bundle_id.unwrap_or_default().to_lowercase();
    let app_name = name.to_lowercase();
    let executable = executable_name.unwrap_or_default().to_lowercase();
    bundle == "com.apple.safari"
        || bundle == "org.mozilla.firefox"
        || bundle == "company.thebrowser.browser"
        || bundle == "app.zen-browser.zen"
        || bundle.starts_with("com.google.chrome")
        || bundle.starts_with("com.microsoft.edgemac")
        || bundle.starts_with("com.brave.browser")
        || bundle.starts_with("com.operasoftware.opera")
        || bundle.starts_with("com.vivaldi.vivaldi")
        || app_name == "safari"
        || app_name == "firefox"
        || app_name == "arc"
        || app_name == "zen"
        || app_name.contains("chrome")
        || app_name.contains("chromium")
        || app_name.contains("edge")
        || app_name.contains("brave")
        || app_name.contains("opera")
        || app_name.contains("vivaldi")
        || matches!(
            executable.as_str(),
            "chrome.exe"
                | "msedge.exe"
                | "firefox.exe"
                | "brave.exe"
                | "opera.exe"
                | "vivaldi.exe"
                | "arc.exe"
                | "zen.exe"
                | "chromium.exe"
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_query_is_a_pid_or_a_name_and_never_empty() {
        assert_eq!(AppQuery::parse(" pid:42 ").unwrap(), AppQuery::Pid(42));
        assert_eq!(
            AppQuery::parse("pid:0").unwrap(),
            AppQuery::Named("pid:0".into())
        );
        assert_eq!(
            AppQuery::parse("pid:x").unwrap(),
            AppQuery::Named("pid:x".into())
        );
        assert_eq!(
            AppQuery::parse("Notes").unwrap(),
            AppQuery::Named("Notes".into())
        );
        assert_eq!(
            AppQuery::parse("  ").unwrap_err().message,
            "app query must not be empty"
        );
        let missing = AppQuery::Named("Nope".into()).not_found();
        assert_eq!(missing.code, "app_not_found");
        assert_eq!(missing.message, "app 'Nope' not found");
        assert_eq!(
            AppQuery::Pid(7).not_found().message,
            "app 'pid:7' not found"
        );
    }

    #[test]
    fn names_identifiers_and_executable_stems_match_case_insensitively() {
        assert!(matches("notes", "Notes", Some("com.apple.Notes"), None));
        assert!(matches(
            "COM.APPLE.NOTES",
            "Notes",
            Some("com.apple.Notes"),
            None
        ));
        assert!(matches("NOTEPAD", "Notepad", None, Some("notepad")));
        assert!(!matches(
            "note",
            "Notes",
            Some("com.apple.Notes"),
            Some("notes")
        ));
    }

    #[test]
    fn a_macos_app_answers_to_its_bundles_own_names_and_its_executable_never_a_fragment() {
        // `launch --app Calculator` answered name=계산기 (2026-09-21); the
        // next verb asks by the word it typed, and is answered by the same app.
        let calculator = ["Calculator", "Calculator"];
        assert!(matches_any(
            "calculator",
            "계산기",
            Some("com.apple.calculator"),
            calculator
        ));
        assert!(matches_any(
            "계산기",
            "계산기",
            Some("com.apple.calculator"),
            calculator
        ));
        assert!(matches_any(
            "electron",
            "Visual Studio Code",
            Some("com.microsoft.VSCode"),
            ["Code", "Electron"]
        ));
        assert!(!matches_any(
            "calc",
            "계산기",
            Some("com.apple.calculator"),
            calculator
        ));
        assert!(!matches_any(
            "calculator",
            "계산기",
            Some("com.apple.calculator"),
            std::iter::empty()
        ));
        assert_eq!(
            MACOS_BUNDLE_NAME_KEYS,
            ["CFBundleDisplayName", "CFBundleName"]
        );
    }

    #[test]
    fn password_managers_are_blocked_on_both_platforms() {
        assert!(is_blocked_bundle_id("com.1password.1password"));
        assert!(!is_blocked_bundle_id("com.apple.Notes"));
        assert!(is_blocked_windows_app(Some("1Password.exe"), None));
        assert!(is_blocked_windows_app(Some("KeePassXC.exe"), None));
        assert!(is_blocked_windows_app(
            None,
            Some("AgileBits.1Password_1a2b3c!App")
        ));
        assert!(!is_blocked_windows_app(
            Some("notepad.exe"),
            Some("Microsoft.WindowsNotepad_8wekyb3d8bbwe!App")
        ));
        assert_eq!(blocked("1password.exe").code, "app_blocked");
        assert_eq!(blocked("x").message, "app 'x' is blocked for safety");
    }

    #[test]
    fn browsers_are_recognised_by_bundle_name_or_executable() {
        assert!(is_known_browser("Safari", Some("com.apple.Safari"), None));
        assert!(is_known_browser("Google Chrome", None, None));
        assert!(is_known_browser("Microsoft Edge", None, Some("msedge.exe")));
        assert!(is_known_browser("", None, Some("firefox.exe")));
        assert!(!is_known_browser("Notepad", None, Some("notepad.exe")));
    }
}
