//! SSH targets the settings pane owns: an endpoint and how to reach it.
//!
//! This is the settings-domain half of Orca's SSH host list, and it is
//! deliberately NOT [`mod@crate::ssh_hosts`]. That module pins an exact host key
//! and addresses a password in the OS vault for the native SFTP/PTY client;
//! this one describes a target the system `ssh` binary will be asked to reach,
//! so its whole record is user-owned configuration — an alias, a key PATH, a
//! proxy command — and none of it is a secret. Nothing here may hold one:
//! a passphrase belongs to the connection attempt and to memory, never to a
//! store, which is why there is no credential field to forget to protect.
//!
//! Entries ride in the settings document beside `ssh_hosts` and
//! `remote_servers` rather than in a file of their own. A new file under the
//! state root would have to be declared in [`crate::app_paths`]'s artifact
//! inventory to survive a state-root move, and a list of plain metadata is
//! exactly what that document already is.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::ssh_config::SshConfigHost;

/// The list is a person's own machines, not a directory. Orca's own import
/// caps what `~/.ssh/config` may add at a similar order; a cap here keeps a
/// hand-edited document from becoming an unbounded paint.
const MAX_TARGETS: usize = 64;
const MAX_LABEL_CHARS: usize = 80;
/// A host, a user, an identity path, a proxy command. Long enough for a real
/// `cloudflared access ssh --hostname %h`, short enough that a pasted file
/// cannot become a settings entry.
const MAX_FIELD_CHARS: usize = 512;
pub(crate) const DEFAULT_PORT: u16 = 22;
/// Orca's `EMPTY_FORM` default, and the value the form offers the moment
/// keep-alive is turned off.
pub(crate) const DEFAULT_GRACE_SECONDS: u64 = 86_400;
/// Under a minute is a disconnect, not a grace period; over a week is a
/// terminal nobody asked to keep. `0` is the third answer — "until reset" —
/// and is what `relay_keep_alive_until_reset` writes.
const MIN_GRACE_SECONDS: u64 = 60;
const MAX_GRACE_SECONDS: u64 = 604_800;
/// Everything this pane creates. `~/.ssh/config` import is the only other
/// writer there will ever be, and it must never overwrite a row a person
/// typed — the source is how that rule is enforced.
pub(crate) const MANUAL_SOURCE: &str = "manual";
const SSH_CONFIG_SOURCE: &str = "ssh-config";
/// Deleted aliases are a memory, not a ledger: past this many the oldest is
/// let go, because a suppression list that grows forever is a list that
/// eventually outlives the file it was suppressing. Orca keeps 50 of its own
/// tombstones; the order of magnitude is the same.
const MAX_SUPPRESSED_ALIASES: usize = 128;
const ID_PREFIX: &str = "ssh-";
const ID_SUFFIX_LEN: usize = 6;
const MAX_ID_CHARS: usize = 64;
const BASE36: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";

/// One saved target. Every field carries a serde default so a document
/// written by an older build — or by the next slice, which adds fields — still
/// loads instead of taking the whole settings document down with it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct SshTarget {
    pub(crate) id: String,
    pub(crate) label: String,
    /// The address to dial. Either this or `config_host` has to say where.
    pub(crate) host: String,
    /// A `~/.ssh/config` alias. When it is set the system `ssh` binary is
    /// handed the alias alone, because the user's own config outranks every
    /// field beside it.
    pub(crate) config_host: String,
    pub(crate) port: u16,
    pub(crate) username: String,
    /// A PATH, never key material. Stored as typed so `~` still means the
    /// home directory of whoever opens the connection.
    pub(crate) identity_file: String,
    pub(crate) proxy_command: String,
    pub(crate) jump_host: String,
    pub(crate) system_ssh_connection_reuse: bool,
    pub(crate) relay_grace_period_seconds: u64,
    pub(crate) relay_keep_alive_until_reset: bool,
    pub(crate) source: String,
}

impl Default for SshTarget {
    fn default() -> Self {
        Self {
            id: String::new(),
            label: String::new(),
            host: String::new(),
            config_host: String::new(),
            port: DEFAULT_PORT,
            username: String::new(),
            identity_file: String::new(),
            proxy_command: String::new(),
            jump_host: String::new(),
            system_ssh_connection_reuse: true,
            relay_grace_period_seconds: DEFAULT_GRACE_SECONDS,
            relay_keep_alive_until_reset: true,
            source: MANUAL_SOURCE.to_string(),
        }
    }
}

impl SshTarget {
    /// What the second line of a card reads, and the label a nameless target
    /// falls back to: `user@host` when there is a user, the host otherwise.
    pub(crate) fn endpoint(&self) -> String {
        let host = self.address();
        if self.username.is_empty() {
            host.to_string()
        } else {
            format!("{}@{host}", self.username)
        }
    }

    /// The alias wins: a row that names one is reached through the user's own
    /// `~/.ssh/config`, and the dialled host is that file's to decide.
    fn address(&self) -> &str {
        if self.config_host.is_empty() {
            &self.host
        } else {
            &self.config_host
        }
    }
}

/// A target on its way into the document, and whether it claims to already be
/// in it. The two travel together because an update that finds nothing and an
/// insert that collides are different failures.
#[derive(Debug)]
pub(crate) struct PreparedSshTarget {
    pub(crate) target: SshTarget,
    pub(crate) update: bool,
}

impl SshTarget {
    /// Validate at the boundary and mint what is missing.
    ///
    /// The renderer checks the same things to keep the form honest, but this
    /// is the gate: a target that reaches the document has an address, a
    /// dialable port, a grace period inside the window, and a name.
    pub(crate) fn prepare(mut self) -> Result<PreparedSshTarget, SshStoreError> {
        let update = !self.id.trim().is_empty();
        self.id = if update {
            let id = self.id.trim().to_string();
            if !valid_id(&id) {
                return Err(SshStoreError::InvalidId);
            }
            id
        } else {
            mint_id()
        };

        self.host = trimmed_field(&self.host)?;
        self.config_host = trimmed_field(&self.config_host)?;
        if self.host.is_empty() && self.config_host.is_empty() {
            return Err(SshStoreError::HostRequired);
        }
        self.username = trimmed_field(&self.username)?;
        self.identity_file = trimmed_field(&self.identity_file)?;
        self.proxy_command = trimmed_field(&self.proxy_command)?;
        self.jump_host = trimmed_field(&self.jump_host)?;

        // `u16` already refuses everything above 65535 at the wire, so zero is
        // the only unusable value that can still arrive here.
        if self.port == 0 {
            return Err(SshStoreError::PortOutOfRange);
        }
        // Keep-alive and a countdown are one answer with two spellings. The
        // toggle is the authority: while it is on the seconds mean nothing,
        // so they are written as the "until reset" zero rather than left as
        // whatever the disabled field last held.
        if self.relay_keep_alive_until_reset {
            self.relay_grace_period_seconds = 0;
        } else if !(MIN_GRACE_SECONDS..=MAX_GRACE_SECONDS)
            .contains(&self.relay_grace_period_seconds)
        {
            return Err(SshStoreError::GraceOutOfBounds);
        }

        self.label = normalized_input_label(&self.label, &self)?;
        // This pane writes one source. An imported row is
        // [`sync_from_config`]'s to write, and a manual row stays its own —
        // the rule that keeps an import from quietly taking over a machine
        // somebody typed in. It also means EDITING an imported row adopts it:
        // it becomes that person's, and the file stops speaking for it.
        self.source = MANUAL_SOURCE.to_string();
        Ok(PreparedSshTarget {
            target: self,
            update,
        })
    }

    /// What a hand-edited or older document is read back as. Never fails:
    /// a row that cannot be rescued is dropped by [`normalize_entries`],
    /// because refusing to load the whole settings document over one bad
    /// target would take every other setting with it.
    fn normalized(mut self) -> Self {
        self.id = self.id.trim().to_string();
        self.host = clamped_field(&self.host);
        self.config_host = clamped_field(&self.config_host);
        self.username = clamped_field(&self.username);
        self.identity_file = clamped_field(&self.identity_file);
        self.proxy_command = clamped_field(&self.proxy_command);
        self.jump_host = clamped_field(&self.jump_host);
        if self.port == 0 {
            self.port = DEFAULT_PORT;
        }
        if self.relay_keep_alive_until_reset {
            self.relay_grace_period_seconds = 0;
        } else {
            self.relay_grace_period_seconds = self
                .relay_grace_period_seconds
                .clamp(MIN_GRACE_SECONDS, MAX_GRACE_SECONDS);
        }
        if self.source != SSH_CONFIG_SOURCE {
            self.source = MANUAL_SOURCE.to_string();
        }
        self.label = normalized_stored_label(&self.label, &self);
        self
    }

    fn addressable(&self) -> bool {
        valid_id(&self.id) && !(self.host.is_empty() && self.config_host.is_empty())
    }
}

/// Read back a list without inventing rows. A duplicate id is the same target
/// twice and only the first is kept; a row with no id or no address names
/// nothing this window could ever open, so it goes.
pub(crate) fn normalize_entries(entries: Vec<SshTarget>) -> Vec<SshTarget> {
    let mut seen = HashSet::new();
    entries
        .into_iter()
        .map(SshTarget::normalized)
        .filter(SshTarget::addressable)
        .filter(|target| seen.insert(target.id.clone()))
        .take(MAX_TARGETS)
        .collect()
}

pub(crate) fn upsert_entry(
    entries: &mut Vec<SshTarget>,
    target: SshTarget,
    update: bool,
) -> Result<(), SshStoreError> {
    match entries.iter().position(|saved| saved.id == target.id) {
        Some(index) if update => entries[index] = target,
        Some(_) => return Err(SshStoreError::TargetAlreadyExists),
        None if update => return Err(SshStoreError::TargetNotFound),
        None if entries.len() >= MAX_TARGETS => return Err(SshStoreError::TargetLimitReached),
        None => entries.push(target),
    }
    Ok(())
}

pub(crate) fn remove_entry(
    entries: &mut Vec<SshTarget>,
    id: &str,
) -> Result<SshTarget, SshStoreError> {
    let index = entries
        .iter()
        .position(|target| target.id == id)
        .ok_or(SshStoreError::TargetNotFound)?;
    Ok(entries.remove(index))
}

/* ---- ~/.ssh/config --------------------------------------------------------
 *
 * The file is the other author of this list, and the whole of the contract
 * with it is three rules:
 *
 *   1. **A manual row is untouchable.** Somebody typed it, and a file that
 *      happens to name the same machine does not get to rewrite what they
 *      wrote. Editing an imported row makes it manual, so this is also how a
 *      person takes a row back from the file for good.
 *   2. **An imported row is the file's.** Every field it carries is
 *      overwritten the moment the file says something else, because the row
 *      is a VIEW of a block and a stale view is worse than no card.
 *   3. **A deletion is remembered.** Removing an imported row records its
 *      alias, and the silent sync that runs on every pane open skips it — or
 *      the same card would come back within the second, forever. The import
 *      button is the amnesty: it forgives the whole list and re-adopts.
 *
 * What this never does is REMOVE a row whose block left the file. A machine
 * that dropped out of `~/.ssh/config` is still a machine, and taking its card
 * away — with whatever a later slice hangs off that target — is a decision for
 * the person, not for a sync that runs when a pane opens. */

/// Fold what the file says into the list. Answers how many rows CHANGED,
/// which is the number the toast reads out: an import that found nothing new
/// says so rather than claiming work.
pub(crate) fn sync_from_config(
    entries: &mut Vec<SshTarget>,
    suppressed: &[String],
    parsed: &[SshConfigHost],
) -> usize {
    let mut changed = 0;
    for host in parsed {
        if host.alias.is_empty() || suppressed.iter().any(|alias| alias == &host.alias) {
            continue;
        }
        match entries
            .iter()
            .position(|entry| claims_alias(entry, &host.alias))
        {
            Some(index) if entries[index].source == SSH_CONFIG_SOURCE => {
                let adopted = adopted_target(&entries[index], host);
                if adopted != entries[index] {
                    entries[index] = adopted;
                    changed += 1;
                }
            }
            // A row somebody typed. The file does not speak for it.
            Some(_) => {}
            // The cap is the list's, not the file's: a config with a hundred
            // blocks fills what there is room for and the rest wait rather
            // than pushing a target out.
            None if entries.len() >= MAX_TARGETS => {}
            None => {
                let fresh = SshTarget {
                    id: mint_id(),
                    ..SshTarget::default()
                };
                entries.push(adopted_target(&fresh, host));
                changed += 1;
            }
        }
    }
    changed
}

/// Whether a row already answers to this alias. `config_host` is where an
/// imported row keeps it, and a manual row that typed the alias into its host
/// field is naming the same machine — matching only the first would import a
/// second card for a machine already on screen.
fn claims_alias(entry: &SshTarget, alias: &str) -> bool {
    entry.config_host == alias || (entry.config_host.is_empty() && entry.host == alias)
}

/// The row a block describes, keeping the id of the row it replaces.
fn adopted_target(existing: &SshTarget, host: &SshConfigHost) -> SshTarget {
    SshTarget {
        id: existing.id.clone(),
        // Named after where it goes. A row somebody renamed is a MANUAL row
        // and never reaches here, so there is no chosen name to preserve.
        label: String::new(),
        host: host.hostname.clone(),
        config_host: host.alias.clone(),
        port: host.port,
        username: host.username.clone(),
        identity_file: host.identity_file.clone(),
        proxy_command: host.proxy_command.clone(),
        jump_host: host.jump_host.clone(),
        // `~/.ssh/config` has nothing to say about these, so what the row
        // already holds survives the sync instead of being reset to a default
        // on every drift.
        system_ssh_connection_reuse: existing.system_ssh_connection_reuse,
        relay_grace_period_seconds: existing.relay_grace_period_seconds,
        relay_keep_alive_until_reset: existing.relay_keep_alive_until_reset,
        source: SSH_CONFIG_SOURCE.to_string(),
    }
    // Through the same door a stored row comes back through, so the answer is
    // one the document already agrees with: drift is a real difference, never
    // this function and `normalized` disagreeing about a clamp.
    .normalized()
}

/// Remember a deletion the silent sync would otherwise undo. A manual row has
/// no alias to suppress — nothing was going to bring it back.
pub(crate) fn record_removed_alias(suppressed: &mut Vec<String>, removed: &SshTarget) {
    if removed.source != SSH_CONFIG_SOURCE {
        return;
    }
    let alias = removed.config_host.trim();
    if alias.is_empty() || suppressed.iter().any(|held| held == alias) {
        return;
    }
    if suppressed.len() >= MAX_SUPPRESSED_ALIASES {
        suppressed.remove(0);
    }
    suppressed.push(alias.to_string());
}

/// Read the suppression list back the way [`normalize_entries`] reads targets:
/// a hand-edited document cannot make this list unbounded, and an alias that
/// could never have come out of a config file is not one.
pub(crate) fn normalize_aliases(aliases: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    aliases
        .into_iter()
        .map(|alias| clamped_field(&alias))
        .filter(|alias| !alias.is_empty())
        .filter(|alias| seen.insert(alias.clone()))
        .take(MAX_SUPPRESSED_ALIASES)
        .collect()
}

pub(crate) fn parse_id(id: &str) -> Result<String, SshStoreError> {
    let id = id.trim();
    if valid_id(id) {
        Ok(id.to_string())
    } else {
        Err(SshStoreError::InvalidId)
    }
}

/// `ssh-<epoch ms>-<6 base36>` — Orca's own shape, and the one the remote
/// workspace scheme (`ssh:<targetId>`) will encode in a later slice.
fn mint_id() -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis());
    format!("{ID_PREFIX}{millis}-{}", random_suffix())
}

/// Six lowercase base-36 glyphs. Drawn from a v4 UUID's random bytes so this
/// module needs no generator of its own; `u64` is far wider than `36^6`, so
/// the bias the modulus leaves is not measurable in an id suffix.
fn random_suffix() -> String {
    let bytes = uuid::Uuid::new_v4().into_bytes();
    let mut value = u64::from_be_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
    ]);
    let mut suffix = String::with_capacity(ID_SUFFIX_LEN);
    for _ in 0..ID_SUFFIX_LEN {
        suffix.push(char::from(BASE36[(value % 36) as usize]));
        value /= 36;
    }
    suffix
}

/// An id names a file-less row, but it also becomes part of a workspace path
/// later. Keep it to what a path segment can carry without escaping.
fn valid_id(id: &str) -> bool {
    id.len() <= MAX_ID_CHARS
        && id.starts_with(ID_PREFIX)
        && id.len() > ID_PREFIX.len()
        && id
            .chars()
            .all(|glyph| glyph.is_ascii_lowercase() || glyph.is_ascii_digit() || glyph == '-')
}

fn trimmed_field(value: &str) -> Result<String, SshStoreError> {
    let value = value.trim();
    if value.chars().count() > MAX_FIELD_CHARS || value.chars().any(char::is_control) {
        return Err(SshStoreError::InvalidField);
    }
    Ok(value.to_string())
}

fn clamped_field(value: &str) -> String {
    value
        .trim()
        .chars()
        .filter(|glyph| !glyph.is_control())
        .take(MAX_FIELD_CHARS)
        .collect()
}

fn normalized_input_label(label: &str, target: &SshTarget) -> Result<String, SshStoreError> {
    let label = label.trim();
    if label.chars().any(char::is_control) || label.chars().count() > MAX_LABEL_CHARS {
        return Err(SshStoreError::InvalidLabel);
    }
    Ok(if label.is_empty() {
        target.endpoint()
    } else {
        label.to_string()
    })
}

fn normalized_stored_label(label: &str, target: &SshTarget) -> String {
    let label: String = label
        .trim()
        .chars()
        .filter(|glyph| !glyph.is_control())
        .take(MAX_LABEL_CHARS)
        .collect();
    if label.is_empty() {
        target.endpoint()
    } else {
        label
    }
}

/// Failures the renderer has to be able to TELL APART, so they are spelled as
/// stable codes rather than as prose. The form maps `host_required` and
/// `grace_out_of_bounds` onto the field they belong to; a sentence written
/// here would be a sentence in one language, matched by substring.
///
/// Nothing here names a target's host, user, or key path — an error line is
/// read out of logs and screenshots, and only the label and id are ours to
/// repeat.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SshStoreError {
    InvalidId,
    InvalidLabel,
    InvalidField,
    HostRequired,
    PortOutOfRange,
    GraceOutOfBounds,
    TargetAlreadyExists,
    TargetNotFound,
    TargetLimitReached,
}

impl fmt::Display for SshStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidId => "invalid_id",
            Self::InvalidLabel => "invalid_label",
            Self::InvalidField => "invalid_field",
            Self::HostRequired => "host_required",
            Self::PortOutOfRange => "port_out_of_range",
            Self::GraceOutOfBounds => "grace_out_of_bounds",
            Self::TargetAlreadyExists => "target_already_exists",
            Self::TargetNotFound => "target_not_found",
            Self::TargetLimitReached => "target_limit_reached",
        })
    }
}

impl std::error::Error for SshStoreError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn draft() -> SshTarget {
        SshTarget {
            host: "deploy.example.test".to_string(),
            username: "deploy".to_string(),
            port: 2222,
            relay_keep_alive_until_reset: false,
            relay_grace_period_seconds: DEFAULT_GRACE_SECONDS,
            ..SshTarget::default()
        }
    }

    #[test]
    fn a_saved_target_survives_the_round_trip_through_json() {
        let prepared = draft().prepare().expect("a complete draft saves");
        let saved = vec![prepared.target.clone()];
        let json = serde_json::to_string(&saved).expect("targets serialize");
        // The wire and the document speak camelCase, which is the shape the
        // renderer's form binds to field by field.
        assert!(
            json.contains("\"identityFile\"") && json.contains("\"relayKeepAliveUntilReset\""),
            "the stored shape stopped being the shape the window reads: {json}"
        );
        let read: Vec<SshTarget> = serde_json::from_str(&json).expect("targets deserialize");
        assert_eq!(read, saved);
        assert_eq!(normalize_entries(read), saved, "a clean row was rewritten");
    }

    #[test]
    fn a_minted_id_carries_orcas_shape() {
        let id = draft().prepare().expect("a complete draft saves").target.id;
        let (millis, suffix) = id
            .strip_prefix(ID_PREFIX)
            .and_then(|rest| rest.split_once('-'))
            .unwrap_or_else(|| panic!("`{id}` is not `ssh-<ms>-<6b36>`"));
        assert!(
            millis.chars().all(|glyph| glyph.is_ascii_digit()) && !millis.is_empty(),
            "`{id}` does not carry a millisecond stamp"
        );
        assert_eq!(
            suffix.chars().count(),
            ID_SUFFIX_LEN,
            "`{id}` suffix length"
        );
        assert!(
            suffix
                .chars()
                .all(|glyph| glyph.is_ascii_lowercase() || glyph.is_ascii_digit()),
            "`{id}` suffix is not lowercase base36"
        );
        assert!(valid_id(&id), "a freshly minted id fails its own check");
        assert_ne!(
            mint_id().split('-').next_back(),
            mint_id().split('-').next_back(),
            "two mints in the same millisecond collide"
        );
    }

    #[test]
    fn a_target_without_an_address_is_refused() {
        let nowhere = SshTarget {
            host: "   ".to_string(),
            ..SshTarget::default()
        };
        assert_eq!(nowhere.prepare().err(), Some(SshStoreError::HostRequired));
        // An alias alone is an address: the user's own `~/.ssh/config` says
        // where it goes.
        let aliased = SshTarget {
            config_host: "bastion".to_string(),
            ..SshTarget::default()
        };
        assert_eq!(
            aliased
                .prepare()
                .expect("an alias is an address")
                .target
                .host,
            ""
        );
        assert_eq!(
            SshTarget { port: 0, ..draft() }.prepare().err(),
            Some(SshStoreError::PortOutOfRange)
        );
    }

    #[test]
    fn a_grace_period_is_a_minute_a_week_or_until_reset() {
        for seconds in [1, 59, MAX_GRACE_SECONDS + 1] {
            assert_eq!(
                SshTarget {
                    relay_grace_period_seconds: seconds,
                    ..draft()
                }
                .prepare()
                .err(),
                Some(SshStoreError::GraceOutOfBounds),
                "{seconds}s was accepted"
            );
        }
        for seconds in [MIN_GRACE_SECONDS, DEFAULT_GRACE_SECONDS, MAX_GRACE_SECONDS] {
            assert!(
                SshTarget {
                    relay_grace_period_seconds: seconds,
                    ..draft()
                }
                .prepare()
                .is_ok(),
                "{seconds}s was refused"
            );
        }
        // Keep-alive is the third answer, and it OVERWRITES whatever the
        // disabled seconds field was left holding — including a value the
        // bounds would otherwise refuse.
        let kept = SshTarget {
            relay_keep_alive_until_reset: true,
            relay_grace_period_seconds: 7,
            ..draft()
        }
        .prepare()
        .expect("keep-alive does not consult the seconds")
        .target;
        assert_eq!(kept.relay_grace_period_seconds, 0);
    }

    #[test]
    fn a_nameless_target_is_named_after_where_it_goes() {
        let named = draft().prepare().expect("saves").target;
        assert_eq!(named.label, "deploy@deploy.example.test");
        let anonymous = SshTarget {
            username: String::new(),
            ..draft()
        }
        .prepare()
        .expect("saves")
        .target;
        assert_eq!(anonymous.label, "deploy.example.test");
        // An alias names itself the same way.
        let aliased = SshTarget {
            host: String::new(),
            config_host: "bastion".to_string(),
            username: String::new(),
            ..draft()
        }
        .prepare()
        .expect("saves")
        .target;
        assert_eq!(aliased.label, "bastion");
        // A label somebody typed is never replaced by one of these.
        let chosen = SshTarget {
            label: "  Build box  ".to_string(),
            ..draft()
        }
        .prepare()
        .expect("saves")
        .target;
        assert_eq!(chosen.label, "Build box");
    }

    #[test]
    fn a_document_from_another_build_still_loads() {
        // Forward compatibility, both directions: fields the next slice adds
        // are ignored rather than fatal, and fields it has not written yet
        // take their defaults.
        let json = r#"[{
            "id": "ssh-1700000000000-a1b2c3",
            "host": "old.example.test",
            "gssapiAuthentication": true,
            "portForwards": [{ "local": 8080 }]
        }]"#;
        let read: Vec<SshTarget> = serde_json::from_str(json).expect("unknown fields are ignored");
        let target = &read[0];
        assert_eq!(target.port, DEFAULT_PORT, "a missing port lost its default");
        assert!(target.system_ssh_connection_reuse);
        assert!(target.relay_keep_alive_until_reset);
        assert_eq!(target.source, MANUAL_SOURCE);
        // And the row is usable rather than merely parsed.
        let kept = normalize_entries(read);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].label, "old.example.test");
    }

    #[test]
    fn a_hand_edited_list_keeps_one_usable_row_per_target() {
        let mut written = vec![
            SshTarget {
                id: "ssh-1-aaaaaa".to_string(),
                host: "one.example.test".to_string(),
                ..SshTarget::default()
            },
            // The same target twice.
            SshTarget {
                id: "ssh-1-aaaaaa".to_string(),
                host: "two.example.test".to_string(),
                ..SshTarget::default()
            },
            // No address, so nothing this window could open.
            SshTarget {
                id: "ssh-2-bbbbbb".to_string(),
                ..SshTarget::default()
            },
            // No id, so nothing this window could name.
            SshTarget {
                host: "three.example.test".to_string(),
                ..SshTarget::default()
            },
        ];
        written.extend((0..MAX_TARGETS).map(|index| SshTarget {
            id: format!("ssh-9-{index:0>6}"),
            host: format!("bulk{index}.example.test"),
            ..SshTarget::default()
        }));
        let kept = normalize_entries(written);
        assert_eq!(kept.len(), MAX_TARGETS, "the list is not capped");
        assert_eq!(kept[0].host, "one.example.test");
        assert!(
            kept.iter().all(|target| target.id != "ssh-2-bbbbbb"),
            "an addressless row survived"
        );
    }

    #[test]
    fn an_update_needs_an_existing_row_and_an_insert_cannot_collide() {
        let mut saved = Vec::new();
        let first = draft().prepare().expect("saves");
        upsert_entry(&mut saved, first.target.clone(), first.update).expect("the first insert");
        assert_eq!(saved.len(), 1);

        // An id nobody minted here cannot be edited into existence.
        let stranger = SshTarget {
            id: "ssh-1700000000000-zzzzzz".to_string(),
            ..draft()
        }
        .prepare()
        .expect("a well-formed id prepares");
        assert!(stranger.update);
        assert_eq!(
            upsert_entry(&mut saved, stranger.target, true),
            Err(SshStoreError::TargetNotFound)
        );

        // And an edit of a row that IS here replaces it rather than doubling it.
        let edited = SshTarget {
            id: first.target.id.clone(),
            label: "Renamed".to_string(),
            ..draft()
        }
        .prepare()
        .expect("saves");
        upsert_entry(&mut saved, edited.target, edited.update).expect("the edit");
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].label, "Renamed");

        assert_eq!(parse_id("  not-an-ssh-id "), Err(SshStoreError::InvalidId));
        assert_eq!(parse_id(&first.target.id), Ok(first.target.id.clone()));
        remove_entry(&mut saved, &first.target.id).expect("the removal");
        assert!(saved.is_empty());
        assert_eq!(
            remove_entry(&mut saved, &first.target.id),
            Err(SshStoreError::TargetNotFound)
        );
    }

    fn config_host(alias: &str) -> SshConfigHost {
        SshConfigHost {
            alias: alias.to_string(),
            hostname: format!("{alias}.example.test"),
            username: "deploy".to_string(),
            ..SshConfigHost::default()
        }
    }

    #[test]
    fn the_file_adds_a_row_then_keeps_it_true_and_counts_only_what_it_moved() {
        let parsed = vec![config_host("web"), config_host("build")];
        let mut entries = Vec::new();
        assert_eq!(sync_from_config(&mut entries, &[], &parsed), 2);
        assert_eq!(entries.len(), 2);
        let web = entries[0].clone();
        assert_eq!(
            (
                web.config_host.as_str(),
                web.host.as_str(),
                web.port,
                web.source.as_str(),
                web.label.as_str(),
            ),
            // The alias is what gets dialled, so it is what names the card.
            (
                "web",
                "web.example.test",
                DEFAULT_PORT,
                "ssh-config",
                "deploy@web"
            )
        );

        // The number is honest: a second sync of the same file moved nothing.
        assert_eq!(sync_from_config(&mut entries, &[], &parsed), 0);
        assert_eq!(entries, vec![web.clone(), entries[1].clone()]);

        // Drift — the file moved the machine, so the row moves with it and
        // keeps the id every other slice will hold it by.
        let moved = vec![
            SshConfigHost {
                hostname: "10.0.0.9".to_string(),
                port: 2222,
                ..config_host("web")
            },
            config_host("build"),
        ];
        assert_eq!(sync_from_config(&mut entries, &[], &moved), 1);
        assert_eq!(entries[0].id, web.id, "an update minted a second row");
        assert_eq!(
            (entries[0].host.as_str(), entries[0].port),
            ("10.0.0.9", 2222)
        );

        // And the cap is the list's: a file with more machines than there is
        // room for fills what is left and stops.
        entries.extend((0..MAX_TARGETS).map(|index| SshTarget {
            id: format!("ssh-9-{index:0>6}"),
            host: format!("bulk{index}.example.test"),
            ..SshTarget::default()
        }));
        entries = normalize_entries(entries);
        assert_eq!(entries.len(), MAX_TARGETS);
        assert_eq!(
            sync_from_config(&mut entries, &[], &[config_host("late")]),
            0
        );
        assert_eq!(entries.len(), MAX_TARGETS, "a full list took one more");
    }

    #[test]
    fn a_row_somebody_typed_is_never_the_files_to_rewrite() {
        // Two ways a manual row can already name the alias: in the field the
        // form writes, and in `config_host` — a row that was imported and
        // then EDITED, which is how a person takes it back for good.
        let mut entries = vec![
            SshTarget {
                id: "ssh-1-aaaaaa".to_string(),
                label: "My web".to_string(),
                host: "web".to_string(),
                ..SshTarget::default()
            },
            SshTarget {
                id: "ssh-1-bbbbbb".to_string(),
                label: "Adopted".to_string(),
                config_host: "build".to_string(),
                username: "mine".to_string(),
                ..SshTarget::default()
            },
        ];
        let before = entries.clone();
        assert_eq!(
            sync_from_config(
                &mut entries,
                &[],
                &[config_host("web"), config_host("build")]
            ),
            0,
            "the file rewrote a row somebody typed"
        );
        assert_eq!(entries, before);
    }

    #[test]
    fn a_removed_alias_stays_removed_until_the_button_forgives_it() {
        let parsed = vec![config_host("web")];
        let mut entries = Vec::new();
        assert_eq!(sync_from_config(&mut entries, &[], &parsed), 1);
        let id = entries[0].id.clone();
        let removed = remove_entry(&mut entries, &id).expect("the removal");

        let mut suppressed = Vec::new();
        record_removed_alias(&mut suppressed, &removed);
        record_removed_alias(&mut suppressed, &removed);
        assert_eq!(suppressed, ["web"], "one deletion was remembered twice");
        // A manual row has no alias to suppress: nothing was going to bring
        // it back, and remembering it would suppress an import that never ran.
        record_removed_alias(
            &mut suppressed,
            &SshTarget {
                config_host: "typed".to_string(),
                ..SshTarget::default()
            },
        );
        assert_eq!(suppressed, ["web"]);

        // The silent sync — the one that runs every time the pane opens —
        // leaves it deleted.
        assert_eq!(sync_from_config(&mut entries, &suppressed, &parsed), 0);
        assert!(entries.is_empty(), "a deleted machine came straight back");

        // 가져오기 is the amnesty: the list is cleared and the card returns.
        suppressed.clear();
        assert_eq!(sync_from_config(&mut entries, &suppressed, &parsed), 1);
        assert_eq!(entries[0].config_host, "web");
        assert_ne!(entries[0].id, id, "a re-adopted row kept a dead id");
    }

    #[test]
    fn the_suppression_list_is_bounded_the_way_the_target_list_is() {
        let written: Vec<String> = std::iter::repeat_n("web".to_string(), 2)
            .chain([
                "  padded  ".to_string(),
                String::new(),
                "\u{7}bell".to_string(),
            ])
            .chain((0..MAX_SUPPRESSED_ALIASES).map(|index| format!("bulk{index}")))
            .collect();
        let kept = normalize_aliases(written);
        assert_eq!(kept.len(), MAX_SUPPRESSED_ALIASES, "the list is not capped");
        assert_eq!(kept[0], "web", "the same alias was kept twice");
        assert_eq!(kept[1], "padded");
        assert_eq!(kept[2], "bell", "a control character survived");

        // And the oldest is let go rather than the newest refused: the alias
        // somebody just deleted is the one the next sync must not resurrect.
        let mut suppressed = kept;
        record_removed_alias(
            &mut suppressed,
            &SshTarget {
                config_host: "newest".to_string(),
                source: "ssh-config".to_string(),
                ..SshTarget::default()
            },
        );
        assert_eq!(suppressed.len(), MAX_SUPPRESSED_ALIASES);
        assert_eq!(suppressed.last().map(String::as_str), Some("newest"));
        assert_ne!(suppressed.first().map(String::as_str), Some("web"));
    }

    #[test]
    fn an_error_is_a_code_the_form_can_route() {
        // The renderer maps these onto the field that is wrong. Prose would
        // make that a substring match against one language.
        assert_eq!(SshStoreError::HostRequired.to_string(), "host_required");
        assert_eq!(
            SshStoreError::GraceOutOfBounds.to_string(),
            "grace_out_of_bounds"
        );
        assert_eq!(
            SshStoreError::PortOutOfRange.to_string(),
            "port_out_of_range"
        );
        // And no code repeats what a target is: a host or a key path in an
        // error line is a host or a key path in a log.
        let told = SshTarget {
            host: "secret.internal.test".to_string(),
            identity_file: "~/.ssh/deploy_key".to_string(),
            ..draft()
        };
        let refused = SshTarget {
            port: 0,
            ..told.clone()
        }
        .prepare()
        .expect_err("port 0 is refused")
        .to_string();
        assert!(
            !refused.contains(&told.host) && !refused.contains(&told.identity_file),
            "an error line repeated the target: {refused}"
        );
    }
}
