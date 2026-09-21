//! The mirror home's login — kept a mirror, in both directions.
//!
//! [`crate::codex_mirror`] gives ZeroCode's Codex a `CODEX_HOME` of its own so
//! trust can be written without forging the user's consent. What it did not
//! carry was the login: Codex reads `auth.json` strictly from `CODEX_HOME`, so
//! a mirror without one asks a signed-in person to sign in again ("실제로
//! 로그인이 되어있는데 로그인된 oauth정보를 못보는거같아" — the bug report this
//! module answers).
//!
//! Copying the file once would fix the prompt and plant a slower bug: Codex
//! ROTATES its OAuth tokens on refresh. A refresh inside the mirror leaves
//! `~/.codex` holding a revoked refresh token, and the user's own terminal
//! Codex logs out days later with no visible cause. Orca ships a whole
//! provenance manager for exactly this (`codex-runtime-home` auth sync,
//! index.js: `resolveSystemDefaultMirrorClaim`, `codexAuthIsMonotonicallyFresher`,
//! `readBackRefreshedSystemDefaultAuth`); this is the same contract at our
//! size:
//!
//! - **The system home is the source.** A fresh login or refresh in
//!   `~/.codex` reaches the mirror before every launch.
//! - **A refresh inside the mirror flows back** — but only when the mirror's
//!   copy is PROVEN to have started as ours (provenance hash), belongs to the
//!   same account, and is monotonically fresher. Anything less and writing to
//!   `~/.codex` would be hijacking the user's own login with a file we cannot
//!   vouch for.
//! - **A logout propagates.** A system home with no login takes back the copy
//!   we planted, and only that copy — a login somebody performed INSIDE the
//!   mirror pane is theirs, not ours to delete.
//!
//! Provenance is a SHA-256 of the bytes we last planted, never a second copy
//! of the credential: one secret file per home is already one more than
//! anyone wants.

use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

use base64::Engine as _;

use crate::codex_mirror::Home;

/// Codex's credential file, in either home.
pub const AUTH_FILE: &str = "auth.json";

/// The mirror-side record of what this module last wrote there.
const PROVENANCE_FILE: &str = ".zerocode-auth-provenance";

/// The inter-process lease held while a Codex mirror launch is prepared.
///
/// A regular mutex would only cover workers in one shell process. The window
/// can be restarted while the old Codex processes are still starting, so the
/// lease lives in the mirror and uses the operating system's file lock. The
/// file is intentionally kept after the first launch: removing it would let a
/// second process open a different inode and walk around the lease.
const LAUNCH_LOCK_FILE: &str = ".zerocode-auth-launch.lock";

/// The textual width of a hyphenated UUID, which Codex uses for ChatGPT
/// account ids.
const UUID_TEXT_LENGTH: usize = 36;

const INVALID_CREDENTIAL_MESSAGE: &str = "refusing to write a malformed Codex credential (expected ChatGPT access/refresh tokens and a UUID account id, or a non-empty API key)";
const INVALID_SYSTEM_HOME_MESSAGE: &str =
    "refusing to use an app-owned Codex home as the user credential target";

/// One holder of the mirror's Codex launch lease.
///
/// The file handle is the lock's lifetime. Keeping it in a value that callers
/// must carry through `PtyLane::spawn` makes dropping the lease before the
/// child exists a type-visible mistake in the launch path.
#[derive(Debug)]
pub struct LaunchLock {
    file: File,
}

impl Drop for LaunchLock {
    fn drop(&mut self) {
        // `File::unlock` is best effort here. The OS releases the advisory
        // lock when the handle closes, and there is no useful error path from
        // `Drop` for a failure to release it explicitly.
        let _ = self.file.unlock();
    }
}

pub fn acquire_launch_lock(mirror: &Home) -> std::io::Result<LaunchLock> {
    std::fs::create_dir_all(mirror.path())?;
    let path = mirror.path().join(LAUNCH_LOCK_FILE);
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&path)?;
    crate::private_file::make_private(&path)?;
    file.lock()?;
    Ok(LaunchLock { file })
}

/// What one pre-launch sync did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthSync {
    /// The mirror had no login and the system's was planted.
    MirrorSeeded,
    /// The system's login changed and the mirror's copy was replaced.
    MirrorRefreshed,
    /// Codex refreshed tokens inside the mirror; the system home was updated
    /// so the user's own terminal keeps its login.
    SystemWrittenBack,
    /// Both sides already agree.
    Unchanged,
    /// Nobody is logged in anywhere; nothing was created.
    NoLogin,
    /// The mirror holds a login this module did not plant and cannot vouch
    /// for — left standing, and the system home left alone.
    ForeignKept,
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

/// `last_refresh` out of an auth.json — number of millis or an RFC3339
/// string, Orca's `readCodexLastRefresh` shape.
fn last_refresh(auth_json: &str) -> Option<i64> {
    let parsed: serde_json::Value = serde_json::from_str(auth_json).ok()?;
    match parsed.get("last_refresh")? {
        serde_json::Value::Number(number) => number.as_i64(),
        serde_json::Value::String(text) => rfc3339_millis(text.trim()),
        _ => None,
    }
}

/// `YYYY-MM-DDThh:mm:ss(.frac)?(Z|±hh:mm)` → epoch millis.
///
/// Written here rather than pulled in as a crate because this file needs one
/// comparison, not a calendar: Codex writes the stamp, this module only asks
/// which of two is later. Days-from-civil is Hinnant's formula.
fn rfc3339_millis(text: &str) -> Option<i64> {
    // Split the UTC offset off the tail. The date's own dashes all sit
    // before index 10, so an offset's `-` is the last one and past the `T`.
    let (stamp, offset_minutes) = if let Some(head) = text.strip_suffix('Z') {
        (head, 0i64)
    } else if let Some((head, tail)) = text.rsplit_once('+') {
        (head, offset_of(tail)?)
    } else {
        let at = text.rfind('-').filter(|&at| at > 10)?;
        (&text[..at], -offset_of(&text[at + 1..])?)
    };
    let (date, time) = stamp.split_once(['T', 't'])?;
    let mut ymd = date.split('-');
    let year: i64 = ymd.next()?.parse().ok()?;
    let month: i64 = ymd.next()?.parse().ok()?;
    let day: i64 = ymd.next()?.parse().ok()?;
    if ymd.next().is_some() || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let (clock, frac) = match time.split_once('.') {
        Some((clock, frac)) => (clock, frac),
        None => (time, ""),
    };
    let mut hms = clock.split(':');
    let hour: i64 = hms.next()?.parse().ok()?;
    let minute: i64 = hms.next()?.parse().ok()?;
    let second: i64 = hms.next()?.parse().ok()?;
    if hms.next().is_some() || hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    // Fractional seconds to millis: the first three digits, right-padded.
    let millis: i64 = if frac.is_empty() {
        0
    } else {
        let digits: String = frac.chars().take(3).collect();
        if !digits.chars().all(|ch| ch.is_ascii_digit()) {
            return None;
        }
        format!("{digits:0<3}").parse().ok()?
    };
    // Days from civil (Hinnant): epoch days for year/month/day.
    let years = if month <= 2 { year - 1 } else { year };
    let era = if years >= 0 { years } else { years - 399 } / 400;
    let year_of_era = years - era * 400;
    let day_of_year = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146097 + day_of_era - 719468;
    Some((((days * 24 + hour) * 60 + minute - offset_minutes) * 60 + second) * 1000 + millis)
}

fn offset_of(tail: &str) -> Option<i64> {
    let (hours, minutes) = tail.split_once(':')?;
    Some(hours.parse::<i64>().ok()? * 60 + minutes.parse::<i64>().ok()?)
}

/// Is `candidate` provably fresher than `baseline`? Unprovable is `false` —
/// a write-back that cannot show its work does not happen.
///
/// Public because the managed-account half of the window
/// (`zerocode_shell::codex_accounts`) decides the same direction between a
/// managed home and the runtime mirror, and a rotating refresh token cannot
/// have two rules: whichever side is written second becomes the live branch,
/// and the other's token is `invalid_grant` the next time it is used.
pub fn monotonically_fresher(candidate: &str, baseline: &str) -> bool {
    match (last_refresh(candidate), last_refresh(baseline)) {
        (Some(newer), Some(older)) => newer > older,
        (Some(_), None) => true,
        _ => false,
    }
}

/// Who this credential belongs to, as far as a comparison needs.
///
/// `tokens.account_id` when the login is OAuth; the API key itself when it is
/// a key (equality is the identity there). `None` means "cannot tell", which
/// every caller treats as NOT matching — the conservative reading.
fn identity(auth_json: &str) -> Option<String> {
    let parsed: serde_json::Value = serde_json::from_str(auth_json).ok()?;
    if let Some(account) = parsed
        .get("tokens")
        .and_then(|tokens| tokens.get("account_id"))
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty())
    {
        return Some(format!("account:{account}"));
    }
    parsed
        .get("OPENAI_API_KEY")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty())
        .map(|key| format!("key:{key}"))
}

/// Are these two files the same login? Unknown on either side is NOT the same
/// one — the conservative reading, and the guard that keeps one account's
/// tokens out of another's home.
pub fn same_identity(left: &str, right: &str) -> bool {
    match (identity(left), identity(right)) {
        (Some(mine), Some(theirs)) => mine == theirs,
        _ => false,
    }
}

fn is_hyphenated_uuid(value: &str) -> bool {
    value.len() == UUID_TEXT_LENGTH
        && value.bytes().enumerate().all(|(at, byte)| match at {
            8 | 13 | 18 | 23 => byte == b'-',
            _ => byte.is_ascii_hexdigit(),
        })
}

fn is_jwt(value: &str) -> bool {
    let mut parts = value.split('.');
    let Some(header) = parts.next() else {
        return false;
    };
    let Some(payload) = parts.next() else {
        return false;
    };
    let Some(signature) = parts.next() else {
        return false;
    };
    if parts.next().is_some() || signature.is_empty() {
        return false;
    }
    let decode_json = |part: &str| {
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(part)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
            .is_some_and(|json| json.is_object())
    };
    decode_json(header)
        && decode_json(payload)
        && base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(signature)
            .is_ok_and(|bytes| !bytes.is_empty())
}

/// Whether bytes have the minimum shape of a credential Codex can use.
///
/// This deliberately does not inspect claims or verify a signature. ZeroCode
/// is not an OAuth verifier; it decodes only enough base64url JSON to distinguish
/// a JWT from padded fixture text that can otherwise replace a rotating refresh
/// token. ChatGPT credentials need the JWT access-token shape, a non-empty
/// rotating refresh token, and Codex's hyphenated UUID account id. API-key
/// credentials need a non-empty key.
fn credential_has_valid_shape(auth_json: &str) -> bool {
    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(auth_json) else {
        return false;
    };
    if parsed
        .get("OPENAI_API_KEY")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|key| !key.trim().is_empty())
    {
        return true;
    }
    let Some(tokens) = parsed.get("tokens") else {
        return false;
    };
    let account_is_uuid = tokens
        .get("account_id")
        .and_then(serde_json::Value::as_str)
        .is_some_and(is_hyphenated_uuid);
    let access_is_jwt = tokens
        .get("access_token")
        .and_then(serde_json::Value::as_str)
        .is_some_and(is_jwt);
    let has_refresh_token = tokens
        .get("refresh_token")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|refresh| !refresh.trim().is_empty());
    account_is_uuid && access_is_jwt && has_refresh_token
}

fn validate_credential(contents: &str) -> std::io::Result<()> {
    if credential_has_valid_shape(contents) {
        Ok(())
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            INVALID_CREDENTIAL_MESSAGE,
        ))
    }
}

/// A credential file written the way credential files are written: same-dir
/// temp + rename so no reader sees half a token, 0600 before the bytes land.
fn write_credential(path: &Path, contents: &str) -> std::io::Result<()> {
    validate_credential(contents)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let stage = path.with_extension("zerocode-stage");
    std::fs::write(&stage, contents)?;
    crate::private_file::make_private(&stage)?;
    std::fs::rename(&stage, path)
}

fn provenance_path(mirror: &Home) -> PathBuf {
    mirror.path().join(PROVENANCE_FILE)
}

fn read_provenance(mirror: &Home) -> Option<String> {
    std::fs::read_to_string(provenance_path(mirror))
        .ok()
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
}

fn write_provenance(mirror: &Home, hash: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(mirror.path())?;
    std::fs::write(provenance_path(mirror), hash)
}

/// Bring the mirror's login up to date with the user's, both ways. Called
/// before every launch that points `CODEX_HOME` at the mirror; every failure
/// is the caller's to ignore, because a launch that cannot sync auth should
/// still launch — Codex asking to log in is a worse UI than this module has,
/// but a better one than no pane at all.
pub fn sync(mirror: &Home, system_home: &Path) -> std::io::Result<AuthSync> {
    validate_system_home_target(mirror, system_home)?;
    let lock = acquire_launch_lock(mirror)?;
    sync_locked(&lock, mirror, system_home)
}

fn validate_system_home_target(mirror: &Home, system_home: &Path) -> std::io::Result<()> {
    let canonical_system = std::fs::canonicalize(system_home).ok();
    let canonical_mirror = std::fs::canonicalize(mirror.path()).ok();
    let is_app_owned = crate::codex_install::is_app_owned_codex_home(system_home)
        || canonical_system
            .as_deref()
            .is_some_and(crate::codex_install::is_app_owned_codex_home);
    let is_the_mirror = system_home == mirror.path()
        || canonical_system
            .as_ref()
            .zip(canonical_mirror.as_ref())
            .is_some_and(|(system, mirror)| system == mirror);
    if is_app_owned || is_the_mirror {
        Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            INVALID_SYSTEM_HOME_MESSAGE,
        ))
    } else {
        Ok(())
    }
}

/// Synchronize while a caller holds the launch lease.
///
/// `codex_install::launch_env_with_lock` keeps the lease alive through the
/// child spawn. Calling [`sync`] from there would try to lock the same file a
/// second time, so this private-to-the-crate entry point makes the lock's
/// ownership explicit without weakening the public `sync` contract.
pub(crate) fn sync_locked(
    _lock: &LaunchLock,
    mirror: &Home,
    system_home: &Path,
) -> std::io::Result<AuthSync> {
    validate_system_home_target(mirror, system_home)?;
    let system_auth = system_home.join(AUTH_FILE);
    let mirror_auth = mirror.path().join(AUTH_FILE);
    let system = std::fs::read_to_string(&system_auth)
        .ok()
        .filter(|text| !text.trim().is_empty());
    let held = std::fs::read_to_string(&mirror_auth)
        .ok()
        .filter(|text| !text.trim().is_empty());
    let planted = read_provenance(mirror);

    match (system, held) {
        (None, None) => Ok(AuthSync::NoLogin),
        // The system home logged out. Our copy goes with it; a login somebody
        // performed inside the mirror is not ours to delete.
        (None, Some(mirror_copy)) => {
            if planted.as_deref() == Some(sha256_hex(mirror_copy.as_bytes()).as_str()) {
                std::fs::remove_file(&mirror_auth)?;
                let _ = std::fs::remove_file(provenance_path(mirror));
                Ok(AuthSync::NoLogin)
            } else {
                Ok(AuthSync::ForeignKept)
            }
        }
        (Some(source), None) => {
            write_credential(&mirror_auth, &source)?;
            write_provenance(mirror, &sha256_hex(source.as_bytes()))?;
            Ok(AuthSync::MirrorSeeded)
        }
        (Some(source), Some(mirror_copy)) => {
            if source == mirror_copy {
                validate_credential(&source)?;
                if planted.is_none() {
                    write_provenance(mirror, &sha256_hex(source.as_bytes()))?;
                }
                return Ok(AuthSync::Unchanged);
            }
            // A file another repository wrote through an inherited CODEX_HOME
            // is not a foreign login when it cannot be a credential at all.
            // Repair the app-owned mirror from the user's source before this
            // launch; `write_credential` still validates the source, so two
            // malformed files never bless one another.
            if !credential_has_valid_shape(&mirror_copy) {
                write_credential(&mirror_auth, &source)?;
                write_provenance(mirror, &sha256_hex(source.as_bytes()))?;
                return Ok(AuthSync::MirrorRefreshed);
            }
            let source_hash = sha256_hex(source.as_bytes());
            let mirror_hash = sha256_hex(mirror_copy.as_bytes());
            // The mirror still holds exactly what we planted, so whatever
            // changed happened in the system home — the source refreshed, and
            // the mirror follows it.
            if planted.as_deref() == Some(mirror_hash.as_str()) {
                write_credential(&mirror_auth, &source)?;
                write_provenance(mirror, &sha256_hex(source.as_bytes()))?;
                return Ok(AuthSync::MirrorRefreshed);
            }
            // Codex rewrote the mirror's copy. It flows back only when every
            // part of the claim holds: same account, provably fresher. This
            // is the write into the user's own `~/.codex`, and it exists so
            // their terminal Codex keeps the login after our pane's refresh
            // rotated the tokens.
            if planted.as_deref() == Some(source_hash.as_str())
                && same_identity(&mirror_copy, &source)
                && monotonically_fresher(&mirror_copy, &source)
            {
                write_credential(&system_auth, &mirror_copy)?;
                write_provenance(mirror, &mirror_hash)?;
                return Ok(AuthSync::SystemWrittenBack);
            }
            // Same account but the system's copy is the fresher one (or
            // freshness cannot be proven while the system plainly moved on):
            // the source wins into the mirror.
            if same_identity(&mirror_copy, &source) {
                write_credential(&mirror_auth, &source)?;
                write_provenance(mirror, &sha256_hex(source.as_bytes()))?;
                return Ok(AuthSync::MirrorRefreshed);
            }
            // A different login lives in the mirror — someone signed in
            // inside the pane. Neither side is overwritten with the other.
            Ok(AuthSync::ForeignKept)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc, Barrier,
        atomic::{AtomicUsize, Ordering},
    };

    const ACCOUNT_ONE: &str = "00000000-0000-0000-0000-000000000001";
    const ACCOUNT_TWO: &str = "00000000-0000-0000-0000-000000000002";

    fn auth(account: &str, refreshed_at: i64) -> String {
        format!(
            r#"{{"tokens":{{"account_id":"{account}","access_token":"eyJhbGciOiJSUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiJ0ZXN0In0.c2lnbmF0dXJl","refresh_token":"rt-{refreshed_at}"}},"last_refresh":{refreshed_at}}}"#
        )
    }

    fn truncated_fixture() -> &'static str {
        r#"{"tokens":{"account_id":"acct","access_token":"oauth-token","refresh_token":"refresh-token"},"last_refresh":300}"#
    }

    fn homes() -> (tempfile::TempDir, Home, PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        let mirror = Home::under(&dir.path().join("data"));
        let system = dir.path().join("dot-codex");
        std::fs::create_dir_all(&system).expect("mkdir");
        assert!(mirror.path().starts_with(dir.path()));
        assert!(system.starts_with(dir.path()));
        for inherited in [std::env::var_os("HOME"), std::env::var_os("CODEX_HOME")]
            .into_iter()
            .flatten()
        {
            let inherited = PathBuf::from(inherited);
            assert_ne!(mirror.path(), inherited);
            assert_ne!(system, inherited);
        }
        (dir, mirror, system)
    }

    /// The bug this module answers: a signed-in system home reaches the
    /// mirror before the first launch, so Codex never asks again.
    #[test]
    fn a_signed_in_system_home_is_planted_into_the_mirror() {
        let (_held, mirror, system) = homes();
        assert_eq!(sync(&mirror, &system).expect("sync"), AuthSync::NoLogin);
        assert!(!mirror.path().join(AUTH_FILE).exists());

        std::fs::write(system.join(AUTH_FILE), auth(ACCOUNT_ONE, 100)).expect("login");
        assert_eq!(
            sync(&mirror, &system).expect("sync"),
            AuthSync::MirrorSeeded
        );
        let planted = std::fs::read_to_string(mirror.path().join(AUTH_FILE)).expect("read");
        assert_eq!(planted, auth(ACCOUNT_ONE, 100));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(mirror.path().join(AUTH_FILE))
                .expect("stat")
                .permissions()
                .mode();
            assert_eq!(
                mode & 0o777,
                0o600,
                "a credential file is 0600 or it is a leak"
            );
        }
        assert_eq!(sync(&mirror, &system).expect("sync"), AuthSync::Unchanged);

        // The system home refreshed on its own; the mirror follows.
        std::fs::write(system.join(AUTH_FILE), auth(ACCOUNT_ONE, 200)).expect("refresh");
        assert_eq!(
            sync(&mirror, &system).expect("sync"),
            AuthSync::MirrorRefreshed
        );
        assert_eq!(
            std::fs::read_to_string(mirror.path().join(AUTH_FILE)).expect("read"),
            auth(ACCOUNT_ONE, 200)
        );
    }

    /// Codex rotates tokens on refresh. A refresh inside the mirror flows
    /// back to `~/.codex`, or the user's own terminal logs out days later.
    #[test]
    fn a_refresh_inside_the_mirror_flows_back_to_the_system_home() {
        let (_held, mirror, system) = homes();
        std::fs::write(system.join(AUTH_FILE), auth(ACCOUNT_ONE, 100)).expect("login");
        sync(&mirror, &system).expect("seed");

        // Codex refreshed in the pane: same account, fresher stamp.
        std::fs::write(mirror.path().join(AUTH_FILE), auth(ACCOUNT_ONE, 300)).expect("rotate");
        assert_eq!(
            sync(&mirror, &system).expect("sync"),
            AuthSync::SystemWrittenBack
        );
        assert_eq!(
            std::fs::read_to_string(system.join(AUTH_FILE)).expect("read"),
            auth(ACCOUNT_ONE, 300),
            "the rotated tokens never reached the user's own home"
        );
        // And the next sync has nothing to do.
        assert_eq!(sync(&mirror, &system).expect("sync"), AuthSync::Unchanged);
    }

    /// Four worker summons can arrive on separate blocking tasks. The lease
    /// has to cover the whole mirror preparation window, not only the bytes
    /// written by one call to `sync`, or those tasks can still hand Codex the
    /// same refresh token concurrently.
    #[test]
    fn four_concurrent_worker_summons_take_the_mirror_lease_one_at_a_time() {
        const WORKERS: usize = 4;
        const CRITICAL_SECTION: std::time::Duration = std::time::Duration::from_millis(15);

        let (_held, mirror, system) = homes();
        std::fs::write(system.join(AUTH_FILE), auth(ACCOUNT_ONE, 100)).expect("login");
        let gate = Arc::new(Barrier::new(WORKERS));
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let handles: Vec<_> = (0..WORKERS)
            .map(|_| {
                let gate = Arc::clone(&gate);
                let active = Arc::clone(&active);
                let peak = Arc::clone(&peak);
                let mirror = mirror.clone();
                let system = system.clone();
                std::thread::spawn(move || {
                    gate.wait();
                    let lock = acquire_launch_lock(&mirror).expect("launch lease");
                    sync_locked(&lock, &mirror, &system).expect("sync");
                    let now = active.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(now, Ordering::SeqCst);
                    std::thread::sleep(CRITICAL_SECTION);
                    active.fetch_sub(1, Ordering::SeqCst);
                    drop(lock);
                })
            })
            .collect();
        for handle in handles {
            handle.join().expect("worker summon");
        }
        assert_eq!(
            peak.load(Ordering::SeqCst),
            1,
            "worker launch critical sections overlapped"
        );
        assert_eq!(
            std::fs::read_to_string(mirror.path().join(AUTH_FILE)).expect("mirror auth"),
            auth(ACCOUNT_ONE, 100)
        );
    }

    /// Resume uses the same inter-process lease as a fresh worker summon. A
    /// second resume must remain outside the critical section until the first
    /// child has been spawned and its guard is dropped.
    #[test]
    fn a_second_codex_resume_waits_for_the_mirror_lease() {
        let (_held, mirror, _system) = homes();
        let first = acquire_launch_lock(&mirror).expect("first resume lease");
        let (begun_tx, begun_rx) = std::sync::mpsc::channel();
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let waiting = mirror.clone();
        let second = std::thread::spawn(move || {
            begun_tx.send(()).expect("announce second resume");
            let lock = acquire_launch_lock(&waiting).expect("second resume lease");
            entered_tx.send(()).expect("announce lease entry");
            drop(lock);
        });
        begun_rx.recv().expect("the second resume began");
        assert!(
            entered_rx
                .recv_timeout(std::time::Duration::from_millis(30))
                .is_err(),
            "the second resume entered while the first held the mirror lease"
        );
        drop(first);
        entered_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("the second resume did not enter after release");
        second.join().expect("second resume");
    }

    /// The write-back must be able to PROVE its claim. A different account,
    /// or a stamp that is not fresher, and `~/.codex` is not touched.
    #[test]
    fn an_unproven_mirror_login_never_overwrites_the_system_home() {
        let (_held, mirror, system) = homes();
        std::fs::write(system.join(AUTH_FILE), auth(ACCOUNT_ONE, 100)).expect("login");
        sync(&mirror, &system).expect("seed");

        // Somebody signed into a DIFFERENT account inside the pane.
        std::fs::write(mirror.path().join(AUTH_FILE), auth(ACCOUNT_TWO, 400)).expect("foreign");
        assert_eq!(sync(&mirror, &system).expect("sync"), AuthSync::ForeignKept);
        assert_eq!(
            std::fs::read_to_string(system.join(AUTH_FILE)).expect("read"),
            auth(ACCOUNT_ONE, 100),
            "a foreign login hijacked the user's own home"
        );
        assert_eq!(
            std::fs::read_to_string(mirror.path().join(AUTH_FILE)).expect("read"),
            auth(ACCOUNT_TWO, 400),
            "the login somebody performed in the pane was destroyed"
        );

        // Same account but STALER than the system copy: the source wins.
        std::fs::write(mirror.path().join(AUTH_FILE), auth(ACCOUNT_ONE, 50)).expect("stale");
        assert_eq!(
            sync(&mirror, &system).expect("sync"),
            AuthSync::MirrorRefreshed
        );
        assert_eq!(
            std::fs::read_to_string(mirror.path().join(AUTH_FILE)).expect("read"),
            auth(ACCOUNT_ONE, 100)
        );
    }

    /// A logout takes back the copy this module planted — and only that copy.
    #[test]
    fn a_system_logout_takes_back_our_copy_and_only_ours() {
        let (_held, mirror, system) = homes();
        std::fs::write(system.join(AUTH_FILE), auth(ACCOUNT_ONE, 100)).expect("login");
        sync(&mirror, &system).expect("seed");

        std::fs::remove_file(system.join(AUTH_FILE)).expect("logout");
        assert_eq!(sync(&mirror, &system).expect("sync"), AuthSync::NoLogin);
        assert!(
            !mirror.path().join(AUTH_FILE).exists(),
            "the logout did not reach the mirror"
        );

        // A login performed inside the pane survives a system logout.
        std::fs::write(mirror.path().join(AUTH_FILE), auth(ACCOUNT_TWO, 500)).expect("pane login");
        assert_eq!(sync(&mirror, &system).expect("sync"), AuthSync::ForeignKept);
        assert!(mirror.path().join(AUTH_FILE).exists());
    }

    /// The exact fixture that clobbered the shared mirror on 2026-08-27 must
    /// never travel from a system home into a ZeroCode mirror.
    #[test]
    fn a_truncated_credential_is_never_planted() {
        let (_held, mirror, system) = homes();
        std::fs::write(system.join(AUTH_FILE), truncated_fixture()).expect("fixture login");

        let error = sync(&mirror, &system).expect_err("truncated credential was accepted");

        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert!(
            !mirror.path().join(AUTH_FILE).exists(),
            "the truncated credential reached the mirror"
        );
        assert!(
            !provenance_path(&mirror).exists(),
            "rejected bytes gained ZeroCode provenance"
        );
    }

    /// An access token without the refresh token is a delayed logout, not a
    /// usable ChatGPT credential: it works only until the access token expires.
    #[test]
    fn a_chatgpt_credential_without_a_refresh_token_is_never_planted() {
        let (_held, mirror, system) = homes();
        let access_only = format!(
            r#"{{"tokens":{{"account_id":"{ACCOUNT_ONE}","access_token":"header.payload.signature","refresh_token":""}},"last_refresh":300}}"#
        );
        std::fs::write(system.join(AUTH_FILE), access_only).expect("fixture login");

        let error = sync(&mirror, &system).expect_err("access-only credential was accepted");

        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert!(!mirror.path().join(AUTH_FILE).exists());
        assert!(!provenance_path(&mirror).exists());
    }

    /// Padding fixture strings to the two old width/count checks must not turn
    /// them into a credential: this is still neither a UUID nor a JWT.
    #[test]
    fn a_padded_fixture_is_never_planted() {
        let (_held, mirror, system) = homes();
        let padded = r#"{"tokens":{"account_id":"xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx","access_token":"oauth.token.fixture","refresh_token":"refresh-token"},"last_refresh":300}"#;
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(padded)
                .expect("fixture json")["tokens"]["account_id"]
                .as_str()
                .expect("account")
                .len(),
            UUID_TEXT_LENGTH
        );
        std::fs::write(system.join(AUTH_FILE), padded).expect("fixture login");

        let error = sync(&mirror, &system).expect_err("padded fixture was accepted");

        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert!(!mirror.path().join(AUTH_FILE).exists());
        assert!(!provenance_path(&mirror).exists());
    }

    /// Shape validation is not token expiry validation. A refreshable ChatGPT
    /// login remains usable when its current access token expires, field order
    /// and whitespace are irrelevant JSON details, and API keys are the other
    /// supported Codex login mode.
    #[test]
    fn both_real_credential_modes_pass_without_reading_an_ambient_home() {
        let refreshable_but_expired = format!(
            r#"{{
                "auth_mode": "a-future-codex-spelling",
                "last_refresh": "2026-08-27T00:00:00.123Z",
                "tokens": {{
                    "refresh_token": "refreshable",
                    "access_token": "eyJhbGciOiJSUzI1NiJ9.eyJleHAiOjB9.c2ln",
                    "account_id": "{ACCOUNT_ONE}"
                }}
            }}"#
        );
        assert!(credential_has_valid_shape(&refreshable_but_expired));
        assert!(credential_has_valid_shape(
            r#"{"auth_mode":"api-key","OPENAI_API_KEY":"  sk-live  ","tokens":null}"#
        ));
        assert!(!credential_has_valid_shape(
            r#"{"tokens":{"id_token":"eyJhbGciOiJSUzI1NiJ9.eyJzdWIiOiJ0ZXN0In0.c2ln"}}"#
        ));
    }

    /// Matching an account id is not provenance. A same-account file placed in
    /// the mirror by a test or another product must never acquire permission to
    /// replace the user's login merely by claiming a later timestamp.
    #[test]
    fn a_same_account_mirror_without_provenance_never_overwrites_the_user_home() {
        let (_held, mirror, system) = homes();
        let user_login = auth(ACCOUNT_ONE, 100);
        let unproven = auth(ACCOUNT_ONE, 300);
        std::fs::write(system.join(AUTH_FILE), &user_login).expect("login");
        std::fs::create_dir_all(mirror.path()).expect("mirror");
        std::fs::write(mirror.path().join(AUTH_FILE), &unproven).expect("foreign fixture");

        assert_eq!(
            sync(&mirror, &system).expect("unproven mirror is repaired"),
            AuthSync::MirrorRefreshed
        );
        assert_eq!(
            std::fs::read_to_string(system.join(AUTH_FILE)).expect("user auth"),
            user_login,
            "an unproven mirror replaced the user's own login"
        );
        assert_eq!(
            std::fs::read_to_string(mirror.path().join(AUTH_FILE)).expect("mirror auth"),
            user_login,
            "an unproven mirror survived instead of following the user source"
        );
    }

    /// Environment-derived callers are not the only callers of `sync`. The
    /// filesystem boundary itself must refuse an app-owned mirror as its
    /// supposed user-home target, or an inherited CODEX_HOME can make one test
    /// mirror write rotated fixture bytes into the real shared mirror.
    #[test]
    fn an_app_owned_home_can_never_be_the_user_write_target() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mirror = Home::under(&dir.path().join("test-data"));
        let inherited = Home::under(&dir.path().join("shared-app-data"));
        std::fs::create_dir_all(inherited.path()).expect("shared mirror");
        let user_login = auth(ACCOUNT_ONE, 100);
        std::fs::write(inherited.path().join(AUTH_FILE), &user_login).expect("shared auth");

        let error = sync(&mirror, inherited.path())
            .expect_err("an app-owned home was accepted as the user write target");

        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        assert_eq!(
            std::fs::read_to_string(inherited.path().join(AUTH_FILE)).expect("shared auth"),
            user_login
        );
        assert!(!mirror.path().join(AUTH_FILE).exists());
    }

    /// Another repository can inherit CODEX_HOME and overwrite the mirror
    /// directly. Malformed bytes are not a foreign login and must not keep a
    /// valid user-home credential from being replanted before launch.
    #[test]
    fn a_truncated_mirror_overwrite_is_replanted_from_the_user_home() {
        let (_held, mirror, system) = homes();
        let user_login = auth(ACCOUNT_ONE, 100);
        std::fs::write(system.join(AUTH_FILE), &user_login).expect("login");
        sync(&mirror, &system).expect("seed");
        std::fs::write(mirror.path().join(AUTH_FILE), truncated_fixture()).expect("clobber");

        assert_eq!(
            sync(&mirror, &system).expect("repair before launch"),
            AuthSync::MirrorRefreshed
        );
        assert_eq!(
            std::fs::read_to_string(mirror.path().join(AUTH_FILE)).expect("mirror auth"),
            user_login
        );
    }

    /// A rotated mirror credential reaches the user's home first, so even if
    /// the mirror is later recreated it is R2, not the consumed R1, that is
    /// planted into the new file.
    #[test]
    fn a_mirror_refresh_reaches_the_user_home_before_it_is_replanted() {
        let (_held, mirror, system) = homes();
        let first = auth(ACCOUNT_ONE, 100);
        let rotated = auth(ACCOUNT_ONE, 300);
        std::fs::write(system.join(AUTH_FILE), first).expect("login");
        sync(&mirror, &system).expect("seed");
        std::fs::write(mirror.path().join(AUTH_FILE), &rotated).expect("mirror refresh");

        assert_eq!(
            sync(&mirror, &system).expect("write back"),
            AuthSync::SystemWrittenBack
        );
        assert_eq!(
            std::fs::read_to_string(system.join(AUTH_FILE)).expect("system auth"),
            rotated
        );

        std::fs::remove_file(mirror.path().join(AUTH_FILE)).expect("recreate mirror");
        assert_eq!(
            sync(&mirror, &system).expect("replant"),
            AuthSync::MirrorSeeded
        );
        assert_eq!(
            std::fs::read_to_string(mirror.path().join(AUTH_FILE)).expect("mirror auth"),
            rotated
        );
    }

    /// The freshness stamp reads both spellings and refuses everything else.
    #[test]
    fn the_freshness_stamp_reads_both_spellings() {
        assert_eq!(last_refresh(r#"{"last_refresh":1000}"#), Some(1000));
        assert_eq!(
            last_refresh(r#"{"last_refresh":"2026-08-14T01:00:00Z"}"#),
            Some(1786669200000)
        );
        // The two offset spellings mean the same instant.
        assert_eq!(
            rfc3339_millis("2026-08-14T02:00:00+01:00"),
            Some(1786669200000)
        );
        assert_eq!(
            rfc3339_millis("2026-08-13T22:30:00-02:30"),
            Some(1786669200000)
        );
        assert_eq!(
            rfc3339_millis("2026-08-14T01:00:00.250Z"),
            Some(1786669200250)
        );
        assert_eq!(last_refresh(r#"{"last_refresh":"soon"}"#), None);
        assert_eq!(last_refresh(r#"{}"#), None);
        assert!(monotonically_fresher(
            r#"{"last_refresh":2}"#,
            r#"{"last_refresh":1}"#
        ));
        assert!(!monotonically_fresher(
            r#"{"last_refresh":1}"#,
            r#"{"last_refresh":1}"#
        ));
        assert!(!monotonically_fresher(r#"{}"#, r#"{}"#));
        // An API key is its own identity; an unreadable file matches nothing.
        assert!(same_identity(
            r#"{"OPENAI_API_KEY":"sk-1"}"#,
            r#"{"OPENAI_API_KEY":"sk-1"}"#
        ));
        assert!(!same_identity(
            r#"{"OPENAI_API_KEY":"sk-1"}"#,
            r#"{"OPENAI_API_KEY":"sk-2"}"#
        ));
        assert!(!same_identity("not json", r#"{"OPENAI_API_KEY":"sk-1"}"#));
    }
}
