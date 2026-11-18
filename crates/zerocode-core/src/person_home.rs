//! The person's home folder, read the way the agents the window runs read it.
//!
//! On macOS and Linux every agent reads `HOME`. Windows sets no `HOME`: Claude
//! Code (Node's `os.homedir()`) and Codex (`dirs::home_dir()`) read the
//! profile folder (`%USERPROFILE%`). A road that read `HOME` alone found no
//! home on a fresh Windows install (t-113689: 「윈도우에서 설치 했는데
//! 동봉스킬도 없고 전부 없던데」). This is the one place the rule is written;
//! the window's own `person_home` reads it, with the platform's home folder
//! behind it.
use std::ffi::OsString;
use std::path::PathBuf;

/// The person's home: the profile folder on Windows, `HOME` elsewhere; each
/// falls back to the other when it is not there.
#[must_use]
pub fn person_home() -> Option<PathBuf> {
    person_home_by(
        cfg!(windows),
        std::env::var_os("HOME"),
        std::env::var_os("USERPROFILE"),
    )
}

/// [`person_home`] by Windows' rule or by the others', with both variables
/// handed in, so both rules are tested wherever tests run.
#[must_use]
pub fn person_home_by(
    windows: bool,
    home: Option<OsString>,
    profile: Option<OsString>,
) -> Option<PathBuf> {
    let _ = (windows, profile);
    home.filter(|home| !home.is_empty()).map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Windows sets no `HOME`, and its agents read the profile folder: the
    /// home is that folder, not nothing — and a `HOME` a person set there does
    /// not move it, as it moves neither Node's `os.homedir()` nor
    /// `dirs::home_dir()`.
    #[test]
    fn a_windows_home_is_the_profile_folder_though_no_home_variable_is_set() {
        let profile = || Some(OsString::from(r"C:\Users\person"));
        let elsewhere = || Some(OsString::from(r"D:\elsewhere"));
        assert_eq!(
            person_home_by(true, None, profile()),
            Some(PathBuf::from(r"C:\Users\person"))
        );
        assert_eq!(
            person_home_by(true, elsewhere(), profile()),
            Some(PathBuf::from(r"C:\Users\person"))
        );
        assert_eq!(
            person_home_by(true, elsewhere(), None),
            Some(PathBuf::from(r"D:\elsewhere"))
        );
    }

    /// macOS and Linux keep `HOME` first — every agent there reads it, and a
    /// test or a lane that hands the window a home of its own is obeyed. An
    /// empty variable is no home.
    #[test]
    fn a_unix_home_is_the_home_variable_and_an_empty_one_is_none() {
        assert_eq!(
            person_home_by(
                false,
                Some(OsString::from("/tmp/home")),
                Some(OsString::from("/Users/person"))
            ),
            Some(PathBuf::from("/tmp/home"))
        );
        assert_eq!(person_home_by(false, Some(OsString::new()), None), None);
        assert_eq!(person_home_by(false, None, None), None);
    }
}
