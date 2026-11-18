//! The person's home folder, read the way the agents the window runs read it:
//! core's one rule (`zerocode_core::person_home` — the profile folder on
//! Windows, which sets no `HOME`, and `HOME` elsewhere), with the platform's
//! own answer (`dirs::home_dir`) behind it for a machine that names neither.
//!
//! A road that read `HOME` alone found no home on a fresh Windows install:
//! the Skills page and its bundled-skill installs refused with
//! `skills.homeUnavailable`, and the agents' hooks were never written
//! (t-113689: 「윈도우에서 설치 했는데 동봉스킬도 없고 전부 없던데」).
use std::path::PathBuf;

/// The person's home, as the agents read it.
pub(crate) fn person_home() -> Option<PathBuf> {
    zerocode_core::person_home::person_home().or_else(dirs::home_dir)
}
