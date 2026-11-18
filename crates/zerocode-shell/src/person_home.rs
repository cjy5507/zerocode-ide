//! The person's home folder, read the way the agents the window runs read it.
//!
//! On macOS and Linux every agent reads `HOME`. Windows sets no `HOME`: Claude
//! Code (Node's `os.homedir()`) and Codex (`dirs::home_dir()`) read the
//! profile folder (`%USERPROFILE%`). A road that read `HOME` alone found no
//! home on a fresh Windows install at all — the Skills page and its
//! bundled-skill installs refused with `skills.homeUnavailable`, and the
//! agents' hooks were never written (t-113689: 「윈도우에서 설치 했는데
//! 동봉스킬도 없고 전부 없던데」).
use std::ffi::OsString;
use std::path::PathBuf;

/// The person's home: on Windows the profile folder, elsewhere `HOME`; each
/// falls back to the other when it is not there.
pub(crate) fn person_home() -> Option<PathBuf> {
    person_home_by(cfg!(windows), std::env::var_os("HOME"), dirs::home_dir)
}

/// [`person_home`] by Windows' rule or by unix's, with the variable and the
/// profile folder handed in, so both rules are tested wherever tests run.
fn person_home_by(
    windows: bool,
    home_var: Option<OsString>,
    profile: impl FnOnce() -> Option<PathBuf>,
) -> Option<PathBuf> {
    let home_var = home_var.filter(|home| !home.is_empty()).map(PathBuf::from);
    if windows {
        return profile().or(home_var);
    }
    home_var.or_else(profile)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Windows sets no `HOME`, and its agents read the profile folder: the
    /// home is that folder, not nothing.
    #[test]
    fn a_windows_home_is_the_profile_folder_though_no_home_variable_is_set() {
        let profile = PathBuf::from(r"C:\Users\person");
        assert_eq!(
            person_home_by(true, None, || Some(profile.clone())),
            Some(profile.clone())
        );
        // Node's `os.homedir()` and `dirs::home_dir()` read the profile there
        // whatever `HOME` says, so a `HOME` a person set does not move it.
        assert_eq!(
            person_home_by(true, Some(OsString::from(r"D:\elsewhere")), || Some(
                profile.clone()
            )),
            Some(profile)
        );
        // With no profile folder known, a set `HOME` is better than nothing.
        assert_eq!(
            person_home_by(true, Some(OsString::from(r"D:\elsewhere")), || None),
            Some(PathBuf::from(r"D:\elsewhere"))
        );
    }

    /// macOS and Linux keep `HOME` first — every agent there reads it, and a
    /// test or a lane that hands the window a home of its own is obeyed. An
    /// empty `HOME` is no home; the profile folder stands in for it.
    #[test]
    fn a_unix_home_is_the_home_variable_and_an_empty_one_is_none() {
        let profile = PathBuf::from("/Users/person");
        assert_eq!(
            person_home_by(false, Some(OsString::from("/tmp/home")), || Some(
                profile.clone()
            )),
            Some(PathBuf::from("/tmp/home"))
        );
        assert_eq!(
            person_home_by(false, Some(OsString::new()), || Some(profile.clone())),
            Some(profile.clone())
        );
        assert_eq!(
            person_home_by(false, None, || Some(profile.clone())),
            Some(profile)
        );
        assert_eq!(person_home_by(false, None, || None), None);
    }
}
