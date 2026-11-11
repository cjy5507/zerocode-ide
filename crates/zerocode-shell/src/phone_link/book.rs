//! What the phone link remembers: whether a person turned it on, which
//! phones were paired, and the one pairing code standing right now.
//!
//! A device token is shown once — to the phone that paired — and never kept:
//! the book holds its SHA-256, so a copy of this file opens nothing. Revoking
//! a phone removes its row, and the next request it makes is a 401.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

pub(crate) const BOOK_FILE: &str = "phone-link.json";
/// A pairing code stands this long (the brief's ten minutes).
pub(crate) const CODE_LIFETIME_MS: i64 = 10 * 60 * 1000;
/// Wrong guesses before the standing code is thrown away. With a 40-bit code
/// five guesses are a 2^-37 chance; the person makes a new code.
pub(crate) const CODE_ATTEMPTS: u32 = 5;
/// How many phones one window holds. A person has one or two; a list that
/// grows without bound is a list nobody reads.
pub(crate) const MAX_DEVICES: usize = 8;
/// The longest device name kept, in characters.
const MAX_NAME_CHARS: usize = 40;
/// No 0/O or 1/I/L: a code can be typed off the screen without a QR.
const CODE_ALPHABET: &[u8] = b"ABCDEFGHJKMNPQRSTUVWXYZ23456789";
const CODE_LEN: usize = 8;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Book {
    /// Whether the person turned the phone link on. Off by default; while off
    /// no port is open.
    #[serde(default)]
    pub(crate) enabled: bool,
    /// The loopback port the listener held last, so the Serve mapping made
    /// for it still points at it after a restart.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) port: Option<u16>,
    /// The tailnet HTTPS port the window's own Serve mapping uses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) https_port: Option<u16>,
    #[serde(default)]
    pub(crate) devices: Vec<Device>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Device {
    pub(crate) id: String,
    pub(crate) name: String,
    /// SHA-256 of the bearer token, lowercase hex.
    pub(crate) token_hash: String,
    pub(crate) paired_ms: i64,
    #[serde(default)]
    pub(crate) last_seen_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) push: Option<PushTarget>,
}

/// A browser's push subscription, as `PushSubscription.toJSON()` gives it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PushTarget {
    pub(crate) endpoint: String,
    pub(crate) p256dh: String,
    pub(crate) auth: String,
}

/// The row the settings pane draws for one phone: never the hash.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DeviceView {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) paired_ms: i64,
    pub(crate) last_seen_ms: i64,
    pub(crate) push: bool,
}

pub(crate) fn token_hash(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    let mut out = String::with_capacity(64);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// Byte equality whose time does not depend on where the inputs differ.
pub(crate) fn same(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0u8, |acc, (one, other)| acc | (one ^ other))
        == 0
}

impl Book {
    pub(crate) fn device_of_token(&self, token: &str) -> Option<usize> {
        let hash = token_hash(token);
        self.devices
            .iter()
            .position(|device| same(device.token_hash.as_bytes(), hash.as_bytes()))
    }

    /// Add a phone under a fresh token; the token is returned once and the
    /// book keeps only its hash. The oldest phone gives way past the cap.
    pub(crate) fn pair(&mut self, name: &str, token: &str, id: String, now_ms: i64) -> &Device {
        while self.devices.len() >= MAX_DEVICES {
            let oldest = self
                .devices
                .iter()
                .enumerate()
                .min_by_key(|(_, device)| device.last_seen_ms.max(device.paired_ms))
                .map(|(at, _)| at)
                .unwrap_or(0);
            self.devices.remove(oldest);
        }
        self.devices.push(Device {
            id,
            name: clean_name(name),
            token_hash: token_hash(token),
            paired_ms: now_ms,
            last_seen_ms: now_ms,
            push: None,
        });
        self.devices.last().expect("just pushed")
    }

    pub(crate) fn revoke(&mut self, id: &str) -> bool {
        let before = self.devices.len();
        self.devices.retain(|device| device.id != id);
        self.devices.len() != before
    }

    /// Forget a push target the push service called gone (404/410).
    pub(crate) fn forget_push(&mut self, endpoint: &str) -> bool {
        let mut changed = false;
        for device in &mut self.devices {
            if device
                .push
                .as_ref()
                .is_some_and(|push| push.endpoint == endpoint)
            {
                device.push = None;
                changed = true;
            }
        }
        changed
    }

    pub(crate) fn views(&self) -> Vec<DeviceView> {
        self.devices
            .iter()
            .map(|device| DeviceView {
                id: device.id.clone(),
                name: device.name.clone(),
                paired_ms: device.paired_ms,
                last_seen_ms: device.last_seen_ms,
                push: device.push.is_some(),
            })
            .collect()
    }

    pub(crate) fn push_targets(&self) -> Vec<PushTarget> {
        self.devices
            .iter()
            .filter_map(|device| device.push.clone())
            .collect()
    }
}

/// The Unicode direction controls (embeddings, overrides, isolates, marks):
/// in a phone's name they would reorder what the device list shows.
fn turns_direction(one: char) -> bool {
    matches!(
        one,
        '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'
    )
}

/// A name a phone gave itself, made safe to keep and to draw: printable
/// characters only, no direction controls, trimmed, bounded, never empty.
pub(crate) fn clean_name(name: &str) -> String {
    let kept: String = name
        .chars()
        .filter(|one| !one.is_control() && !turns_direction(*one))
        .take(MAX_NAME_CHARS)
        .collect();
    let kept = kept.trim();
    if kept.is_empty() {
        "Phone".to_string()
    } else {
        kept.to_string()
    }
}

pub(crate) fn book_path(local_data_root: &Path) -> PathBuf {
    local_data_root.join(BOOK_FILE)
}

/// Read the book; a missing or unreadable file is an empty, switched-off book
/// — the link fails closed.
pub(crate) fn load(path: &Path) -> Book {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// Write the book beside itself and rename it into place, readable by this
/// user alone. A staging file left by a write that died is removed first:
/// `mode` applies only to a file being created, and one left behind keeps
/// whatever rights it had.
pub(crate) fn save(path: &Path, book: &Book) -> std::io::Result<()> {
    let bytes = serde_json::to_vec_pretty(book).map_err(std::io::Error::other)?;
    let staged = path.with_extension("json.tmp");
    if let Err(error) = std::fs::remove_file(&staged)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        return Err(error);
    }
    {
        use std::io::Write as _;
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        let mut file = options.open(&staged)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
    }
    std::fs::rename(&staged, path)
}

/// The one pairing code standing, in memory only: a restart forgets it.
#[derive(Debug, Default)]
pub(crate) struct Pairing {
    code: Option<StandingCode>,
}

#[derive(Debug)]
struct StandingCode {
    code: String,
    expires_ms: i64,
    misses: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Redeem {
    Accepted,
    /// No code is standing — never made, already used, expired or burned.
    NoCode,
    Expired,
    Wrong,
    /// That wrong guess was the last one; the code is gone.
    Burned,
}

impl Pairing {
    /// A new code replaces any code standing.
    pub(crate) fn issue(&mut self, now_ms: i64) -> Option<(String, i64)> {
        let bytes = super::random_bytes::<CODE_LEN>()?;
        let code: String = bytes
            .iter()
            .map(|byte| CODE_ALPHABET[usize::from(*byte) % CODE_ALPHABET.len()] as char)
            .collect();
        let expires_ms = now_ms + CODE_LIFETIME_MS;
        self.code = Some(StandingCode {
            code: code.clone(),
            expires_ms,
            misses: 0,
        });
        Some((code, expires_ms))
    }

    #[cfg(test)]
    pub(crate) fn issue_exact(&mut self, code: &str, now_ms: i64) {
        self.code = Some(StandingCode {
            code: code.to_string(),
            expires_ms: now_ms + CODE_LIFETIME_MS,
            misses: 0,
        });
    }

    pub(crate) fn standing_until(&self, now_ms: i64) -> Option<i64> {
        self.code
            .as_ref()
            .filter(|code| code.expires_ms > now_ms)
            .map(|code| code.expires_ms)
    }

    pub(crate) fn redeem(&mut self, offered: &str, now_ms: i64) -> Redeem {
        let Some(standing) = self.code.as_mut() else {
            return Redeem::NoCode;
        };
        if standing.expires_ms <= now_ms {
            self.code = None;
            return Redeem::Expired;
        }
        let offered = offered.trim().to_ascii_uppercase();
        if same(offered.as_bytes(), standing.code.as_bytes()) {
            self.code = None;
            return Redeem::Accepted;
        }
        standing.misses += 1;
        if standing.misses >= CODE_ATTEMPTS {
            self.code = None;
            return Redeem::Burned;
        }
        Redeem::Wrong
    }

    pub(crate) fn clear(&mut self) {
        self.code = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_paired_phone_is_found_by_its_token_and_the_book_keeps_only_the_hash() {
        let mut book = Book::default();
        let token = "fixture-token-one";
        book.pair("My phone", token, "d1".into(), 10);
        let text = serde_json::to_string(&book).expect("json");
        assert!(!text.contains(token), "the token itself is never stored");
        assert!(text.contains(&token_hash(token)));
        assert_eq!(book.device_of_token(token), Some(0));
        assert_eq!(book.device_of_token("fixture-token-two"), None);
        assert_eq!(token_hash(token).len(), 64);
    }

    #[test]
    fn revoking_a_phone_makes_its_token_open_nothing_at_once() {
        let mut book = Book::default();
        book.pair("a", "fixture-a", "d1".into(), 10);
        book.pair("b", "fixture-b", "d2".into(), 11);
        assert!(book.revoke("d1"));
        assert_eq!(book.device_of_token("fixture-a"), None);
        assert_eq!(book.device_of_token("fixture-b"), Some(0));
        assert!(!book.revoke("d1"), "a second revoke changes nothing");
    }

    #[test]
    fn the_device_list_is_capped_and_the_least_recently_seen_phone_leaves() {
        let mut book = Book::default();
        for at in 0..MAX_DEVICES {
            book.pair(
                "p",
                &format!("fixture-{at}"),
                format!("d{at}"),
                at as i64 + 100,
            );
        }
        book.devices[0].last_seen_ms = 10_000;
        book.pair("new", "fixture-new", "dn".into(), 20_000);
        assert_eq!(book.devices.len(), MAX_DEVICES);
        assert!(book.devices.iter().any(|device| device.id == "d0"));
        assert!(!book.devices.iter().any(|device| device.id == "d1"));
    }

    #[test]
    fn device_views_never_carry_the_hash_or_the_push_secret() {
        let mut book = Book::default();
        book.pair("p", "fixture-x", "d1".into(), 5);
        book.devices[0].push = Some(PushTarget {
            endpoint: "https://push.example.test/abc".into(),
            p256dh: "fixture-p256dh".into(),
            auth: "fixture-auth".into(),
        });
        let views = serde_json::to_string(&book.views()).expect("json");
        assert!(!views.contains(&book.devices[0].token_hash));
        assert!(!views.contains("fixture-auth"));
        assert!(!views.contains("push.example.test"));
        assert!(views.contains("\"push\":true"));
    }

    #[test]
    fn a_gone_push_target_is_forgotten_and_the_phone_stays_paired() {
        let mut book = Book::default();
        book.pair("p", "fixture-x", "d1".into(), 5);
        book.devices[0].push = Some(PushTarget {
            endpoint: "https://push.example.test/abc".into(),
            p256dh: "k".into(),
            auth: "a".into(),
        });
        assert!(book.forget_push("https://push.example.test/abc"));
        assert_eq!(book.devices.len(), 1);
        assert!(book.devices[0].push.is_none());
        assert!(!book.forget_push("https://push.example.test/abc"));
    }

    #[test]
    fn names_are_cleaned_bounded_and_never_empty() {
        assert_eq!(clean_name("  \u{7}\n "), "Phone");
        assert_eq!(clean_name("iPhone\u{0}"), "iPhone");
        assert_eq!(clean_name(&"x".repeat(100)).chars().count(), 40);
        assert_eq!(
            clean_name("\u{202E}enohPi\u{2066}x\u{2069}\u{200F}"),
            "enohPix",
            "a name cannot turn the device list's text around"
        );
    }

    #[test]
    fn a_pairing_code_works_once() {
        let mut pairing = Pairing::default();
        pairing.issue_exact("ABCD2345", 0);
        assert_eq!(pairing.redeem("abcd2345", 1), Redeem::Accepted);
        assert_eq!(pairing.redeem("ABCD2345", 2), Redeem::NoCode);
    }

    #[test]
    fn a_pairing_code_expires_after_ten_minutes() {
        let mut pairing = Pairing::default();
        pairing.issue_exact("ABCD2345", 0);
        assert_eq!(
            pairing.standing_until(CODE_LIFETIME_MS - 1),
            Some(CODE_LIFETIME_MS)
        );
        assert_eq!(
            pairing.redeem("ABCD2345", CODE_LIFETIME_MS),
            Redeem::Expired
        );
        assert_eq!(
            pairing.redeem("ABCD2345", CODE_LIFETIME_MS + 1),
            Redeem::NoCode
        );
    }

    #[test]
    fn wrong_guesses_burn_the_code_after_the_limit() {
        let mut pairing = Pairing::default();
        pairing.issue_exact("ABCD2345", 0);
        for _ in 1..CODE_ATTEMPTS {
            assert_eq!(pairing.redeem("ZZZZZZZZ", 1), Redeem::Wrong);
        }
        assert_eq!(pairing.redeem("ZZZZZZZZ", 1), Redeem::Burned);
        assert_eq!(
            pairing.redeem("ABCD2345", 1),
            Redeem::NoCode,
            "even the right code is gone after the limit"
        );
    }

    #[test]
    fn issued_codes_use_the_unambiguous_alphabet_and_replace_the_last() {
        let mut pairing = Pairing::default();
        let (first, until) = pairing.issue(0).expect("random");
        assert_eq!(until, CODE_LIFETIME_MS);
        assert_eq!(first.len(), CODE_LEN);
        assert!(first.bytes().all(|byte| CODE_ALPHABET.contains(&byte)));
        let (second, _) = pairing.issue(1).expect("random");
        if first != second {
            assert_eq!(pairing.redeem(&first, 2), Redeem::Wrong);
        }
        assert_eq!(pairing.redeem(&second, 3), Redeem::Accepted);
    }

    #[test]
    fn the_book_round_trips_through_its_file_and_a_bad_file_is_an_empty_book() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = book_path(dir.path());
        assert_eq!(load(&path), Book::default());
        let mut book = Book {
            enabled: true,
            port: Some(4321),
            https_port: Some(10443),
            devices: Vec::new(),
        };
        book.pair("p", "fixture-x", "d1".into(), 5);
        // A staging file a dead write left, readable by everyone.
        let staged = path.with_extension("json.tmp");
        std::fs::write(&staged, b"left").expect("staged");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o644))
                .expect("chmod");
        }
        save(&path, &book).expect("save");
        assert!(!staged.exists(), "the staging file is renamed into place");
        assert_eq!(load(&path), book);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&path).expect("meta").permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        std::fs::write(&path, b"{ not json").expect("write");
        assert_eq!(load(&path), Book::default());
    }
}
