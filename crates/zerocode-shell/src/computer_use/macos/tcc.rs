//! What the TCC database records for this app's own rows (t-6058).
//!
//! System Settings shows one switch per app, but macOS grants by the code
//! requirement a row was recorded under. A row recorded while the app was
//! ad-hoc signed names that build's `cdhash`, and every build after it is
//! refused under a switch that still shows on: on 2026-09-22 the window's own
//! Accessibility row read `cdhash H"1e40a0ba…"` (a 2026-09-08 build) while the
//! helper's row, recorded later, named the signing certificate — the settings
//! page said 거부됨 and the person turned the same switch off and on again.
//!
//! So each of this app's rows is read and held against its bundle as it is
//! signed now:
//!
//! * **Only this app's rows.** The query names the service and one of this
//!   app's bundle ids, bound as parameters; no other app's row is read,
//!   counted or logged.
//! * **The requirement is the Security framework's to read.** The blob goes
//!   through `SecRequirementCreateWithData` → `SecRequirementCopyString` (what
//!   the row pins) and `SecStaticCodeCheckValidity` (whether the bundle
//!   satisfies it) — the check TCC itself makes, so a `cdhash` pin, a
//!   certificate pin and anything else the requirement language can say are
//!   judged alike. No byte of the blob is parsed here.
//! * **A database that cannot be read says so.** Full Disk Access is judged by
//!   one open attempt ([`crate::developer_permissions::tcc_database_access`]);
//!   refused, every row reads `unreadable` with that reason — never a grant.
//!
//! Measured 2026-09-26 on macOS 26.3: both services' rows stand in the system
//! database (`permissions.json`'s `database`), and a basic requirement check
//! takes 1–4 ms against 170 ms for a full validation of the app bundle.

use std::path::{Path, PathBuf};

use zerocode_core::computer_use::{
    ComputerPermissionGrant, ComputerPermissionPin, ComputerPermissionUnreadable,
};

use crate::developer_permissions::{PermissionStatus, TCC_DATABASE, tcc_database_access};

/// What TCC's `service` column writes before `tccutil`'s name for a service.
const SERVICE_PREFIX: &str = "kTCCService";
/// `auth_value` of a row that allows.
const ALLOWED: i64 = 2;
/// `client_type` of a row that names a bundle id (`1` names a path).
const BUNDLE_CLIENT: i64 = 0;

/// Which TCC database a service's rows stand in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum TccDatabase {
    /// `/Library/Application Support/com.apple.TCC/TCC.db`.
    System,
    /// The person's own, under their home directory.
    User,
}

impl TccDatabase {
    pub(super) fn path(self) -> Option<PathBuf> {
        match self {
            Self::System => Some(Path::new("/").join(TCC_DATABASE)),
            Self::User => dirs::home_dir().map(|home| home.join(TCC_DATABASE)),
        }
    }
}

/// One row to read: the service (`tccutil`'s name for it), the bundle id the
/// row names, and the bundle whose signature it is held against — `None` when
/// that bundle is not there to read.
pub(super) struct Wanted<'a> {
    pub(super) service: &'a str,
    pub(super) bundle_id: &'a str,
    pub(super) bundle: Option<&'a Path>,
}

/// What reading one row found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RowRead {
    /// The database, the row's requirement or the bundle's signature could
    /// not be read.
    Unreadable(ComputerPermissionUnreadable),
    /// No row for this bundle and service, or a row that does not allow.
    Refused,
    /// A row that allows: what its requirement pins (nothing, when it records
    /// none), and whether the bundle as signed now satisfies it.
    Allows {
        pin: Option<ComputerPermissionPin>,
        satisfied: bool,
    },
}

/// The rows `wanted` names, in its order, from one open of `database`.
pub(super) fn read_rows(database: &Path, wanted: &[Wanted<'_>]) -> Vec<RowRead> {
    let every = |why| vec![RowRead::Unreadable(why); wanted.len()];
    match tcc_database_access(database) {
        PermissionStatus::Granted => {}
        PermissionStatus::Denied => return every(ComputerPermissionUnreadable::NoFullDiskAccess),
        _ => return every(ComputerPermissionUnreadable::Database),
    }
    let Ok(db) = crate::sqlite_read::open_read_only(database) else {
        return every(ComputerPermissionUnreadable::Database);
    };
    wanted.iter().map(|row| read_row(&db, row)).collect()
}

/// One row: the one statement that reads the database, bound to the service
/// and this app's bundle id.
fn read_row(db: &rusqlite::Connection, wanted: &Wanted<'_>) -> RowRead {
    use rusqlite::OptionalExtension as _;
    let found = db
        .query_row(
            "SELECT auth_value, csreq FROM access \
             WHERE service = ?1 AND client = ?2 AND client_type = ?3",
            rusqlite::params![
                format!("{SERVICE_PREFIX}{}", wanted.service),
                wanted.bundle_id,
                BUNDLE_CLIENT
            ],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Option<Vec<u8>>>(1)?)),
        )
        .optional();
    match found {
        Err(_) => RowRead::Unreadable(ComputerPermissionUnreadable::Database),
        Ok(None) => RowRead::Refused,
        Ok(Some((auth, _))) if auth != ALLOWED => RowRead::Refused,
        Ok(Some((_, requirement))) => allows(
            requirement.as_deref().filter(|blob| !blob.is_empty()),
            wanted.bundle,
        ),
    }
}

/// The grant a row reads as.
///
/// macOS's own answer for the process a row judges comes first — `live` is
/// that answer, for the judged row only and only when the helper's probe gave
/// one — then what the database records. A row macOS refused is never called
/// granted, whatever it records; a row macOS granted is granted, whatever the
/// database says (a managed Mac grants through a profile, which writes no row
/// here).
pub(super) fn verdict(
    live: Option<bool>,
    read: RowRead,
) -> (
    ComputerPermissionGrant,
    Option<ComputerPermissionUnreadable>,
    Option<ComputerPermissionPin>,
) {
    use ComputerPermissionGrant::{Denied, Granted, Stale, Unreadable};
    match (live, read) {
        (Some(true), RowRead::Allows { pin, .. }) => (Granted, None, pin),
        (Some(true), _) => (Granted, None, None),
        (_, RowRead::Unreadable(why)) => (Unreadable, Some(why), None),
        (_, RowRead::Refused) => (Denied, None, None),
        (
            _,
            RowRead::Allows {
                pin,
                satisfied: false,
            },
        ) => (Stale, None, pin),
        (Some(false), RowRead::Allows { pin, .. }) => (Denied, None, pin),
        (None, RowRead::Allows { pin, .. }) => (Granted, None, pin),
    }
}

/// An allowing row's recorded requirement, held against `bundle`.
fn allows(requirement: Option<&[u8]>, bundle: Option<&Path>) -> RowRead {
    let Some(blob) = requirement else {
        // A row that records no requirement binds its grant to no build.
        return RowRead::Allows {
            pin: None,
            satisfied: true,
        };
    };
    let Some(requirement) = security::Requirement::from_data(blob) else {
        return RowRead::Unreadable(ComputerPermissionUnreadable::Requirement);
    };
    let pin = requirement.text().map(|text| pin_of(&text));
    match bundle.and_then(|bundle| security::satisfied(&requirement, bundle)) {
        Some(satisfied) => RowRead::Allows { pin, satisfied },
        None => RowRead::Unreadable(ComputerPermissionUnreadable::Signature),
    }
}

/// What a requirement, as the Security framework writes it, binds a grant to.
fn pin_of(text: &str) -> ComputerPermissionPin {
    if text.contains("cdhash H\"") {
        ComputerPermissionPin::Cdhash
    } else if text.contains("certificate") || text.contains("anchor") {
        ComputerPermissionPin::Signature
    } else {
        ComputerPermissionPin::Other
    }
}

/// The Security framework calls a row is read with — the only reader of a
/// requirement's bytes.
mod security {
    use std::path::Path;

    use core_foundation::base::{CFType, CFTypeRef, TCFType as _};
    use core_foundation::data::{CFData, CFDataRef};
    use core_foundation::string::{CFString, CFStringRef};
    use core_foundation::url::{CFURL, CFURLRef};

    type OsStatus = i32;
    /// `SecCSFlags`.
    type Flags = u32;
    const DEFAULT: Flags = 0;
    /// `kSecCSBasicValidateOnly` — `kSecCSDoNotValidateExecutable |
    /// kSecCSDoNotValidateResources`: the signature's own facts (identifier,
    /// certificates, code directory hash) without rehashing every page and
    /// resource, which is what a requirement is judged on.
    const BASIC_VALIDATE_ONLY: Flags = (1 << 1) | (1 << 2);
    const SUCCESS: OsStatus = 0;
    /// `errSecCSReqFailed`: the code does not satisfy the requirement.
    const REQUIREMENT_FAILED: OsStatus = -67_050;

    #[link(name = "Security", kind = "framework")]
    unsafe extern "C" {
        fn SecRequirementCreateWithData(
            data: CFDataRef,
            flags: Flags,
            requirement: *mut CFTypeRef,
        ) -> OsStatus;
        fn SecRequirementCopyString(
            requirement: CFTypeRef,
            flags: Flags,
            text: *mut CFStringRef,
        ) -> OsStatus;
        fn SecStaticCodeCreateWithPath(
            path: CFURLRef,
            flags: Flags,
            code: *mut CFTypeRef,
        ) -> OsStatus;
        fn SecStaticCodeCheckValidity(
            code: CFTypeRef,
            flags: Flags,
            requirement: CFTypeRef,
        ) -> OsStatus;
    }

    /// A code requirement, as a TCC row records it.
    pub(super) struct Requirement(CFType);

    impl Requirement {
        pub(super) fn from_data(blob: &[u8]) -> Option<Self> {
            let data = CFData::from_buffer(blob);
            let mut requirement: CFTypeRef = std::ptr::null();
            // SAFETY: `data` is a live CFData for the duration of the call;
            // on success the requirement comes back retained (the Create
            // rule) and the wrapper releases it.
            let status = unsafe {
                SecRequirementCreateWithData(
                    data.as_concrete_TypeRef(),
                    DEFAULT,
                    &raw mut requirement,
                )
            };
            (status == SUCCESS && !requirement.is_null())
                // SAFETY: a non-null, retained object from a Create call.
                .then(|| Self(unsafe { CFType::wrap_under_create_rule(requirement) }))
        }

        /// The requirement in the language `codesign -r` reads.
        pub(super) fn text(&self) -> Option<String> {
            let mut text: CFStringRef = std::ptr::null();
            // SAFETY: `self.0` is a live SecRequirement; the string comes
            // back retained under the Copy rule and the wrapper releases it.
            let status =
                unsafe { SecRequirementCopyString(self.0.as_CFTypeRef(), DEFAULT, &raw mut text) };
            (status == SUCCESS && !text.is_null())
                // SAFETY: a non-null, retained CFString from a Copy call.
                .then(|| unsafe { CFString::wrap_under_create_rule(text) }.to_string())
        }
    }

    /// The code at `bundle` as it is signed now, when it has a signature to
    /// read.
    fn static_code(bundle: &Path) -> Option<CFType> {
        let url = CFURL::from_path(bundle, bundle.is_dir())?;
        let mut code: CFTypeRef = std::ptr::null();
        // SAFETY: `url` is live for the call; the code object comes back
        // retained (the Create rule) and the wrapper releases it.
        let status = unsafe {
            SecStaticCodeCreateWithPath(url.as_concrete_TypeRef(), DEFAULT, &raw mut code)
        };
        (status == SUCCESS && !code.is_null())
            // SAFETY: a non-null, retained object from a Create call.
            .then(|| unsafe { CFType::wrap_under_create_rule(code) })
    }

    /// Whether the code at `bundle`, as signed now, satisfies `requirement` —
    /// `None` when there is no signature there to judge.
    pub(super) fn satisfied(requirement: &Requirement, bundle: &Path) -> Option<bool> {
        let code = static_code(bundle)?;
        // SAFETY: both objects are live Security framework types for the
        // call, which only reads them.
        match unsafe {
            SecStaticCodeCheckValidity(
                code.as_CFTypeRef(),
                BASIC_VALIDATE_ONLY,
                requirement.0.as_CFTypeRef(),
            )
        } {
            SUCCESS => Some(true),
            REQUIREMENT_FAILED => Some(false),
            _ => None,
        }
    }

    /// The designated requirement of the code at `bundle`, as bytes — what
    /// TCC records for it. Tests only: the reader never writes a row.
    #[cfg(test)]
    pub(super) fn designated_requirement(bundle: &Path) -> Option<Vec<u8>> {
        #[link(name = "Security", kind = "framework")]
        unsafe extern "C" {
            fn SecCodeCopyDesignatedRequirement(
                code: CFTypeRef,
                flags: Flags,
                requirement: *mut CFTypeRef,
            ) -> OsStatus;
            fn SecRequirementCopyData(
                requirement: CFTypeRef,
                flags: Flags,
                data: *mut CFDataRef,
            ) -> OsStatus;
        }
        let code = static_code(bundle)?;
        let mut requirement: CFTypeRef = std::ptr::null();
        // SAFETY: `code` is live; the requirement comes back retained.
        if unsafe {
            SecCodeCopyDesignatedRequirement(code.as_CFTypeRef(), DEFAULT, &raw mut requirement)
        } != SUCCESS
            || requirement.is_null()
        {
            return None;
        }
        // SAFETY: a non-null, retained object from a Copy call.
        let requirement = unsafe { CFType::wrap_under_create_rule(requirement) };
        let mut data: CFDataRef = std::ptr::null();
        // SAFETY: `requirement` is live; the data comes back retained.
        if unsafe { SecRequirementCopyData(requirement.as_CFTypeRef(), DEFAULT, &raw mut data) }
            != SUCCESS
            || data.is_null()
        {
            return None;
        }
        // SAFETY: a non-null, retained CFData from a Copy call.
        Some(
            unsafe { CFData::wrap_under_create_rule(data) }
                .bytes()
                .to_vec(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ComputerPermissionGrant::{Denied, Granted, Stale, Unreadable};
    use ComputerPermissionUnreadable::NoFullDiskAccess;

    /// The window's own Accessibility row on 2026-09-22, byte for byte: a
    /// requirement pinned to the 2026-09-08 ad-hoc build's code directory.
    const PINNED_0922: &str =
        "fade0c00000000280000000100000008000000141e40a0baa5313edd8834f75259e33e190aa79ec3";
    /// The rows as the signing certificate records them, in the shape the
    /// system database holds them — the certificate's hash made up.
    const APP_LEAF: &str = "fade0c0000000048000000010000000600000002000000106465762e7a65726f636f64652e6170700000000400000000000000145a17e5a1e0a1c0de5a17e5a1e0a1c0de5a17e5a1";
    const HELPER_LEAF: &str = "fade0c00000000580000000100000006000000020000001d6465762e7a65726f636f64652e6170702e636f6d70757465722d7573650000000000000400000000000000145a17e5a1e0a1c0de5a17e5a1e0a1c0de5a17e5a1";
    /// A Developer ID's requirement, and one that names an identifier alone.
    const DEVELOPER_ID: &str = "fade0c000000009c000000010000000600000006000000060000000600000002000000106465762e7a65726f636f64652e6170700000000f0000000e000000010000000a2a864886f763640602060000000000000000000e000000000000000a2a864886f7636406010d0000000000000000000b000000000000000a7375626a6563742e4f550000000000010000000a414243444531323334350000";
    const IDENTIFIER_ONLY: &str =
        "fade0c00000000240000000100000002000000106465762e7a65726f636f64652e617070";

    fn bytes(hex: &str) -> Vec<u8> {
        (0..hex.len())
            .step_by(2)
            .map(|at| u8::from_str_radix(&hex[at..at + 2], 16).expect("hex"))
            .collect()
    }

    /// This test binary: code the linker signed ad-hoc, so its designated
    /// requirement is one `cdhash` — the shape a 2026-09-08 build carried.
    fn this_build() -> PathBuf {
        std::env::current_exe().expect("the test binary")
    }

    /// Each fixture reads back, through the Security framework, as the text
    /// it was compiled from (`csreq -r '=…' -b`), and pins what it names.
    #[test]
    fn every_recorded_requirement_reads_as_its_text_and_pins_what_it_names() {
        for (hex, text, pin) in [
            (
                PINNED_0922,
                "cdhash H\"1e40a0baa5313edd8834f75259e33e190aa79ec3\"",
                ComputerPermissionPin::Cdhash,
            ),
            (
                APP_LEAF,
                "identifier \"dev.zerocode.app\" and certificate leaf = H\"5a17e5a1e0a1c0de5a17e5a1e0a1c0de5a17e5a1\"",
                ComputerPermissionPin::Signature,
            ),
            (
                HELPER_LEAF,
                "identifier \"dev.zerocode.app.computer-use\" and certificate leaf = H\"5a17e5a1e0a1c0de5a17e5a1e0a1c0de5a17e5a1\"",
                ComputerPermissionPin::Signature,
            ),
            (
                DEVELOPER_ID,
                "identifier \"dev.zerocode.app\" and anchor apple generic and certificate 1[field.1.2.840.113635.100.6.2.6] /* exists */ and certificate leaf[field.1.2.840.113635.100.6.1.13] /* exists */ and certificate leaf[subject.OU] = ABCDE12345",
                ComputerPermissionPin::Signature,
            ),
            (
                IDENTIFIER_ONLY,
                "identifier \"dev.zerocode.app\"",
                ComputerPermissionPin::Other,
            ),
        ] {
            let requirement =
                security::Requirement::from_data(&bytes(hex)).expect("a requirement blob");
            let read = requirement.text().expect("its text");
            assert_eq!(read, text);
            assert_eq!(pin_of(&read), pin, "{read}");
        }
    }

    /// A blob that is no requirement reads unreadable — never a grant, never
    /// a pin guessed from its bytes.
    #[test]
    fn a_blob_that_is_no_requirement_is_unreadable() {
        assert!(security::Requirement::from_data(b"garbage").is_none());
        assert_eq!(
            allows(Some(b"garbage"), Some(&this_build())),
            RowRead::Unreadable(ComputerPermissionUnreadable::Requirement)
        );
    }

    /// The comparison is the one TCC makes: this build satisfies its own
    /// designated requirement and neither the 2026-09-08 build's pin nor a
    /// certificate it was never signed with; a bundle that is not there has
    /// no signature to compare.
    #[test]
    fn a_requirement_is_held_against_the_bundle_as_signed_now() {
        let build = this_build();
        let own = security::designated_requirement(&build).expect("this build's requirement");
        assert_eq!(
            allows(Some(&own), Some(&build)),
            RowRead::Allows {
                pin: Some(ComputerPermissionPin::Cdhash),
                satisfied: true,
            }
        );
        assert_eq!(
            allows(Some(&bytes(PINNED_0922)), Some(&build)),
            RowRead::Allows {
                pin: Some(ComputerPermissionPin::Cdhash),
                satisfied: false,
            }
        );
        assert_eq!(
            allows(Some(&bytes(APP_LEAF)), Some(&build)),
            RowRead::Allows {
                pin: Some(ComputerPermissionPin::Signature),
                satisfied: false,
            }
        );
        let missing = Path::new("/nonexistent/ZeroCode.app");
        for bundle in [Some(missing), None] {
            assert_eq!(
                allows(Some(&bytes(APP_LEAF)), bundle),
                RowRead::Unreadable(ComputerPermissionUnreadable::Signature)
            );
        }
        // A row that records no requirement binds its grant to no build.
        assert_eq!(
            allows(None, None),
            RowRead::Allows {
                pin: None,
                satisfied: true,
            }
        );
    }

    /// Every cell of the verdict: macOS's answer for the judged row first,
    /// then the database; an unread row is never granted unless macOS itself
    /// said so, and a row macOS refused is never granted.
    #[test]
    fn a_row_reads_as_one_of_four_grants() {
        let unread = RowRead::Unreadable(NoFullDiskAccess);
        let current = RowRead::Allows {
            pin: Some(ComputerPermissionPin::Signature),
            satisfied: true,
        };
        let old = RowRead::Allows {
            pin: Some(ComputerPermissionPin::Cdhash),
            satisfied: false,
        };
        let pin = |read: RowRead| match read {
            RowRead::Allows { pin, .. } => pin,
            _ => None,
        };
        for (live, read, grant, why) in [
            (None, unread, Unreadable, Some(NoFullDiskAccess)),
            (Some(false), unread, Unreadable, Some(NoFullDiskAccess)),
            (Some(true), unread, Granted, None),
            (None, RowRead::Refused, Denied, None),
            (Some(false), RowRead::Refused, Denied, None),
            (Some(true), RowRead::Refused, Granted, None),
            (None, current, Granted, None),
            (Some(false), current, Denied, None),
            (Some(true), current, Granted, None),
            (None, old, Stale, None),
            (Some(false), old, Stale, None),
            (Some(true), old, Granted, None),
        ] {
            let expected = (grant, why, if why.is_some() { None } else { pin(read) });
            assert_eq!(verdict(live, read), expected, "{live:?} {read:?}");
        }
    }

    /// A fixture database with the system database's `access` columns this
    /// reader asks for, holding `rows` (service, client, auth_value, csreq).
    fn fixture_database(dir: &Path, rows: &[(&str, &str, i64, Option<Vec<u8>>)]) -> PathBuf {
        let path = dir.join("TCC.db");
        let db = rusqlite::Connection::open(&path).expect("a fixture database");
        db.execute_batch(
            "CREATE TABLE access (service TEXT NOT NULL, client TEXT NOT NULL, \
             client_type INTEGER NOT NULL, auth_value INTEGER NOT NULL, csreq BLOB, \
             indirect_object_identifier TEXT NOT NULL DEFAULT 'UNUSED', \
             PRIMARY KEY (service, client, client_type, indirect_object_identifier));",
        )
        .expect("the access table");
        for (service, client, auth, csreq) in rows {
            db.execute(
                "INSERT INTO access (service, client, client_type, auth_value, csreq) \
                 VALUES (?1, ?2, 0, ?3, ?4)",
                rusqlite::params![service, client, auth, csreq],
            )
            .expect("a fixture row");
        }
        path
    }

    /// The rows read out of a database: this app's rows only, each read as
    /// what it records; the row of an app that is not this one never answers
    /// for it.
    #[test]
    fn this_apps_rows_are_read_as_the_database_records_them() {
        let dir = tempfile::tempdir().expect("a fixture folder");
        let build = this_build();
        let own = security::designated_requirement(&build).expect("this build's requirement");
        let database = fixture_database(
            dir.path(),
            &[
                (
                    "kTCCServiceAccessibility",
                    "dev.zerocode.app",
                    2,
                    Some(bytes(PINNED_0922)),
                ),
                (
                    "kTCCServiceAccessibility",
                    "dev.zerocode.app.computer-use",
                    2,
                    Some(own.clone()),
                ),
                ("kTCCServiceScreenCapture", "dev.zerocode.app", 0, Some(own)),
                ("kTCCServiceScreenCapture", "dev.example.other", 2, None),
            ],
        );
        let wanted = |service, bundle_id| Wanted {
            service,
            bundle_id,
            bundle: Some(build.as_path()),
        };
        assert_eq!(
            read_rows(
                &database,
                &[
                    wanted("Accessibility", "dev.zerocode.app"),
                    wanted("Accessibility", "dev.zerocode.app.computer-use"),
                    wanted("ScreenCapture", "dev.zerocode.app"),
                    wanted("ScreenCapture", "dev.zerocode.app.computer-use"),
                ],
            ),
            [
                RowRead::Allows {
                    pin: Some(ComputerPermissionPin::Cdhash),
                    satisfied: false,
                },
                RowRead::Allows {
                    pin: Some(ComputerPermissionPin::Cdhash),
                    satisfied: true,
                },
                RowRead::Refused,
                RowRead::Refused,
            ]
        );
    }

    /// The 2026-09-22 case: the app's Accessibility row allows the 09-08
    /// build and nothing since. Read against any other build it is stale —
    /// whether the page reads it beside the helper's row or macOS refused it.
    #[test]
    fn the_row_pinned_to_the_0908_build_reads_stale_on_every_build_since() {
        let dir = tempfile::tempdir().expect("a fixture folder");
        let database = fixture_database(
            dir.path(),
            &[(
                "kTCCServiceAccessibility",
                "dev.zerocode.app",
                2,
                Some(bytes(PINNED_0922)),
            )],
        );
        let build = this_build();
        let [read] = read_rows(
            &database,
            &[Wanted {
                service: "Accessibility",
                bundle_id: "dev.zerocode.app",
                bundle: Some(&build),
            }],
        )[..] else {
            panic!("one row read");
        };
        for live in [None, Some(false)] {
            assert_eq!(
                verdict(live, read),
                (Stale, None, Some(ComputerPermissionPin::Cdhash)),
                "{live:?}"
            );
        }
    }

    /// The 2026-09-22 row held against the app installed on this machine now:
    /// whatever build it is, it is not the 09-08 one.
    #[test]
    #[ignore = "reads the installed app's signature; run with ZEROCODE_COMPUTER_MACOS_HELPER_APP_PATH naming the installed helper"]
    fn the_0922_row_reads_stale_against_the_installed_app() {
        let helper = super::super::helper_app_path().expect("the installed helper");
        let app = super::super::app_bundle_path(&helper);
        let read = allows(Some(&bytes(PINNED_0922)), Some(&app));
        eprintln!("{}: {read:?} -> {:?}", app.display(), verdict(None, read));
        assert_eq!(
            verdict(None, read),
            (Stale, None, Some(ComputerPermissionPin::Cdhash))
        );
    }

    /// No Full Disk Access, reproduced on a fixture this process may not open
    /// (mode 000 — `EACCES` here, `EPERM` from TCC's own guard; std reads both
    /// as `PermissionDenied`): every row says unreadable, with that reason,
    /// and none reads as granted.
    #[test]
    fn a_database_this_process_may_not_open_reads_unreadable_for_every_row() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().expect("a fixture folder");
        let own =
            security::designated_requirement(&this_build()).expect("this build's requirement");
        let database = fixture_database(
            dir.path(),
            &[("kTCCServiceAccessibility", "dev.zerocode.app", 2, Some(own))],
        );
        std::fs::set_permissions(&database, std::fs::Permissions::from_mode(0o000))
            .expect("shut the fixture");
        assert!(
            std::fs::File::open(&database).is_err(),
            "the fixture must refuse this process (is it running as root?)"
        );
        let build = this_build();
        let reads = read_rows(
            &database,
            &[
                Wanted {
                    service: "Accessibility",
                    bundle_id: "dev.zerocode.app",
                    bundle: Some(&build),
                },
                Wanted {
                    service: "ScreenCapture",
                    bundle_id: "dev.zerocode.app",
                    bundle: Some(&build),
                },
            ],
        );
        assert_eq!(reads, [RowRead::Unreadable(NoFullDiskAccess); 2]);
        for read in reads {
            assert_eq!(
                verdict(None, read),
                (Unreadable, Some(NoFullDiskAccess), None)
            );
        }
        // A database that is not there at all is no verdict on Full Disk
        // Access either.
        assert_eq!(
            read_rows(
                &dir.path().join("absent.db"),
                &[Wanted {
                    service: "Accessibility",
                    bundle_id: "dev.zerocode.app",
                    bundle: Some(&build),
                }],
            ),
            [RowRead::Unreadable(ComputerPermissionUnreadable::Database)]
        );
        std::fs::set_permissions(&database, std::fs::Permissions::from_mode(0o600))
            .expect("reopen the fixture for cleanup");
    }
}
