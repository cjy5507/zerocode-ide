//! Session-scope verified-state ledger — what the harness ITSELF observed
//! about this session's verification, so the model does not re-derive it.
//!
//! ## The measured waste
//!
//! In long multi-stage sessions the model rebuilds an ad-hoc verification
//! script (a bash heredoc) at every stage. Deep-bench transcripts across four
//! sessions put 37–62% of self-verification volume (11.7k–43.8k chars per
//! task) into re-checks issued while *nothing the previous green check could
//! have covered had been edited since* — an upper-bound estimate, but the
//! shape is unambiguous: the model has no way to know a check already ran
//! green, because nothing in its context says so.
//!
//! ## What this ledger claims, and what it refuses to claim
//!
//! It records exactly two runtime-owned IO facts, in order:
//!
//! 1. a check-shaped `bash` call ran to completion with exit 0 (the executor
//!    recorded the exit — not a model claim about it), and
//! 2. a later `edit_file`/`write_file` succeeded on some path.
//!
//! It deliberately does NOT claim to know which files a check *covers*. That
//! mapping is unknowable to the harness (a `cargo test -p runtime` covers
//! files no argument names), and a harness that guessed it would produce
//! confident nonsense — the failure mode the whole attestation discipline
//! exists to prevent. So the observation lists the edit delta and leaves the
//! judgement to the model: "this ran green, and these files changed since".
//!
//! ## Recorded in real time, not at turn end
//!
//! Every fact is folded in the moment the tool result settles
//! ([`VerifiedStateLedger::record_event`]), not when the turn completes. That
//! is not a performance choice — it is what makes the ledger safe to QUOTE
//! mid-turn. A ledger that settled at turn end could not know about the edit
//! the turn had just made, so anything reading it mid-turn (the deep gate's
//! VERIFY leg, which opens right after EXEC) would be told "nothing edited
//! since" about a tree EXEC had just changed. Recording at the seam makes that
//! lie unrepresentable rather than merely unlikely.
//!
//! [`VerifiedStateLedger::note_turn_boundary`] is all that is left at turn end,
//! and it only advances the turn counter — no facts arrive there.
//!
//! ## Persistence (why a sidecar and not memory)
//!
//! A bench stage is a fresh process resuming a session, and headless rebuilds
//! the runtime every turn. An in-memory ledger would therefore be empty at
//! exactly the moment it is worth reading — the start of stage N+1. It
//! write-throughs to `<session>.verified-state.json` and reloads on rebind,
//! the same shape as [`crate::file_read_registry::FileReadRegistry`].
//!
//! Write-through happens per recorded event, so a session killed mid-turn
//! resumes knowing the greens that turn had already earned. The cost is one
//! bounded atomic write (a pruned ledger is a few KB) per green check or
//! successful edit — orders of magnitude below the tool call that produced it,
//! which had just written the very file being recorded.
//!
//! ## Bounded by construction
//!
//! Only the newest `STORED_MAX_CHECKS` commands are kept, an edit that
//! predates the oldest retained check is dropped (it can never be rendered),
//! and both commands and paths dedupe newest-wins — so a session that runs the
//! same check a hundred times stores one row for it.

use std::fs;
use std::path::{Path, PathBuf};

/// One ordered fact the conversation layer hands to the ledger. The classifier
/// that decides what qualifies lives next to the existing `exec_green_checks`
/// vocabulary (`conversation::verified_state`), so both consumers of "is this a
/// check?" stay on one definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifiedStateEvent {
    /// A check-shaped bash command that exited 0 in the foreground. Already
    /// capped in length by the producer.
    GreenCheck(String),
    /// A successful `edit_file`/`write_file` target path, as the tool reported it.
    Edit(String),
}

/// Newest commands kept in the ledger. Larger than the render cap so a session
/// that alternates between two check sets does not thrash its own history.
const STORED_MAX_CHECKS: usize = 12;
/// Newest edited paths kept. Only paths newer than the oldest retained check
/// survive pruning anyway; this bounds the pathological case (a huge refactor
/// right after one check).
const STORED_MAX_EDITS: usize = 64;
/// Commands reported in one observation block (newest kept).
const RENDER_MAX_CHECKS: usize = 6;
/// Paths listed on one invalidated command's line before it summarizes.
const RENDER_MAX_FILES: usize = 8;

/// Reminder prefix, owned here so the producer and every recognizer bind to one
/// constant. Public because the host-side tests and the conversation layer both
/// match on it.
pub const VERIFIED_STATE_REMINDER_PREFIX: &str = "[zo:verified-state]";

/// Extension of the session sidecar this ledger persists to
/// (`<session>.verified-state.json`). The single source for every host that
/// binds the sidecar — a second literal is a second writer that can drift.
pub const VERIFIED_STATE_SIDECAR_EXTENSION: &str = "verified-state.json";

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct GreenCheck {
    command: String,
    seq: u64,
    turn: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct EditRecord {
    path: String,
    seq: u64,
    turn: u64,
}

/// The on-disk form. Every field defaults so a sidecar written by an older
/// build (or truncated mid-write) degrades to "less history", never to a parse
/// failure that would silently disable the feature.
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
struct PersistedLedger {
    #[serde(default)]
    seq: u64,
    #[serde(default)]
    turn: u64,
    #[serde(default)]
    checks: Vec<GreenCheck>,
    #[serde(default)]
    edits: Vec<EditRecord>,
}

/// Session-lifetime ledger of green checks and the edits that followed them.
///
/// Both vectors are kept sorted by `seq` ascending: dedupe removes the stale
/// row before pushing the new one, so append order IS sequence order and every
/// "since" question is a single comparison.
#[derive(Debug, Default)]
pub struct VerifiedStateLedger {
    state: PersistedLedger,
    /// `None` = pure in-memory (sub-agent contexts, tests), same convention as
    /// the file-read registry: unbound means no write-through, not an error.
    sidecar: Option<PathBuf>,
    /// Whether any event has been recorded since the last
    /// [`Self::note_turn_boundary`]. Deliberately NOT persisted: it exists only
    /// to keep `turn` counting turns-that-contributed-something, and a process
    /// that dies mid-turn simply never closes that turn — the facts it recorded
    /// are already on disk, which is the half that matters.
    dirty_since_boundary: bool,
}

impl VerifiedStateLedger {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Bind to a session sidecar and restore its contents. A missing or corrupt
    /// file degrades to an empty ledger — i.e. to byte-neutral behaviour, never
    /// to a wrong observation. Binding itself writes nothing: a resume must not
    /// blank its own sidecar before it has read it.
    pub fn rebind_sidecar(&mut self, sidecar: Option<PathBuf>) {
        self.sidecar = sidecar;
        self.state = self.load_sidecar();
        self.dirty_since_boundary = false;
    }

    fn load_sidecar(&self) -> PersistedLedger {
        let Some(path) = self.sidecar.as_ref() else {
            return PersistedLedger::default();
        };
        let Ok(raw) = fs::read(path) else {
            return PersistedLedger::default();
        };
        serde_json::from_slice::<PersistedLedger>(&raw).unwrap_or_default()
    }

    /// Write-through, best effort and atomic (tmp + rename). A failure leaves
    /// the in-memory ledger authoritative for this process; the next stage then
    /// simply observes less, which is the safe direction.
    fn persist(&self) {
        let Some(path) = self.sidecar.as_ref() else {
            return;
        };
        let Ok(payload) = serde_json::to_vec(&self.state) else {
            return;
        };
        let tmp = path.with_extension("json.tmp");
        if fs::write(&tmp, payload).is_ok() {
            let _ = fs::rename(&tmp, path);
        }
    }

    /// Fold ONE settled tool result's fact into the ledger, at the moment the
    /// executor settled it.
    ///
    /// Arrival order is load-bearing: a check followed by an edit must come out
    /// as "green, then invalidated", which is representable because each event
    /// takes its own sequence number. Recording at the seam is what makes the
    /// ledger true DURING a turn and therefore quotable by a mid-turn reader
    /// (see the module docs).
    ///
    /// Returns whether anything was recorded — an empty command/path is
    /// ignored, consumes no sequence number, and touches no disk.
    pub fn record_event(&mut self, event: &VerifiedStateEvent) -> bool {
        // The turn in flight is the one after the last closed boundary; the
        // boundary below promotes the counter to match, so a record/boundary
        // pair numbers events exactly as a turn-end fold used to.
        let turn = self.state.turn + 1;
        match event {
            VerifiedStateEvent::GreenCheck(command) => {
                let command = command.trim();
                if command.is_empty() {
                    return false;
                }
                self.state.seq += 1;
                let seq = self.state.seq;
                self.state.checks.retain(|check| check.command != command);
                self.state.checks.push(GreenCheck {
                    command: command.to_string(),
                    seq,
                    turn,
                });
            }
            VerifiedStateEvent::Edit(path) => {
                let path = path.trim();
                if path.is_empty() {
                    return false;
                }
                self.state.seq += 1;
                let seq = self.state.seq;
                self.state.edits.retain(|edit| edit.path != path);
                self.state.edits.push(EditRecord {
                    path: path.to_string(),
                    seq,
                    turn,
                });
            }
        }
        self.dirty_since_boundary = true;
        self.prune();
        self.persist();
        true
    }

    /// Close the current turn. All this does is advance the turn counter, and
    /// only when the turn actually contributed a fact — so a chat-only turn
    /// costs nothing and `recorded_turns` keeps meaning "turns that recorded
    /// something". No fact is ever folded here: that would be a second write
    /// path for events the seam already recorded, and the same check would land
    /// twice.
    pub fn note_turn_boundary(&mut self) {
        if !self.dirty_since_boundary {
            return;
        }
        self.dirty_since_boundary = false;
        self.state.turn += 1;
        self.persist();
    }

    /// Drop what can never be rendered. An edit older than the oldest retained
    /// check answers no question this ledger can be asked ("was anything edited
    /// AFTER check X?"), so it is not history worth keeping — which is also
    /// what keeps a long session's sidecar small.
    fn prune(&mut self) {
        if self.state.checks.len() > STORED_MAX_CHECKS {
            self.state
                .checks
                .drain(..self.state.checks.len() - STORED_MAX_CHECKS);
        }
        // An edit older than the oldest retained check answers no "since"
        // question — except while its turn is still open: the turn-end
        // receipt asks "did a check run green after this turn's edits", and a
        // session that has never run a check must still be able to say no.
        let floor = self.state.checks.first().map_or(u64::MAX, |check| check.seq);
        let open_turn = self.open_turn();
        self.state
            .edits
            .retain(|edit| edit.seq > floor || edit.turn == open_turn);
        if self.state.edits.len() > STORED_MAX_EDITS {
            self.state
                .edits
                .drain(..self.state.edits.len() - STORED_MAX_EDITS);
        }
    }

    /// The observation block for a turn start, or `None` when the ledger holds
    /// no green check at all.
    ///
    /// `None` is the byte-neutrality guarantee: a session that never ran a
    /// check must produce a request byte-identical to one built without this
    /// feature, so an automatic observation can never become an automatic tax.
    ///
    /// `root` shortens display paths only (edit tools report canonical absolute
    /// paths); it never affects what is stored or compared.
    #[must_use]
    pub fn render_observation(&self, root: Option<&Path>) -> Option<String> {
        if self.state.checks.is_empty() {
            return None;
        }
        let start = self.state.checks.len().saturating_sub(RENDER_MAX_CHECKS);
        let mut lines = String::new();
        for check in &self.state.checks[start..] {
            lines.push_str("- `");
            lines.push_str(&first_line(&check.command));
            lines.push_str("` — ");
            lines.push_str(&self.status_for(check.seq, root));
            lines.push('\n');
        }
        Some(format!(
            "{VERIFIED_STATE_REMINDER_PREFIX} <system-reminder>\nVerification already on record \
             for THIS session — each line is a check-shaped bash command the runtime observed \
             exiting 0 (recorded IO facts, not claims), followed by what has been edited since it \
             ran:\n{lines}A command whose line says nothing edited since has a result you already \
             have; re-running it re-derives a fact the harness is holding for you. The harness \
             does NOT know which files a command covers — when files are listed, judge for \
             yourself whether they can affect that command's outcome before re-running \
             it.\n</system-reminder>"
        ))
    }

    /// `"ran green, nothing edited since"` or the invalidated form naming the
    /// paths, capped and newest-first.
    fn status_for(&self, since_seq: u64, root: Option<&Path>) -> String {
        let mut changed: Vec<&EditRecord> = self
            .state
            .edits
            .iter()
            .filter(|edit| edit.seq > since_seq)
            .collect();
        if changed.is_empty() {
            return "ran green, nothing edited since".to_string();
        }
        changed.reverse();
        let overflow = changed.len().saturating_sub(RENDER_MAX_FILES);
        let listed = changed
            .iter()
            .take(RENDER_MAX_FILES)
            .map(|edit| display_path(&edit.path, root))
            .collect::<Vec<_>>()
            .join(", ");
        if overflow == 0 {
            format!("ran green BUT these files changed since: {listed}")
        } else {
            format!("ran green BUT these files changed since: {listed} (+{overflow} more)")
        }
    }

    /// The commands that ran green with NOTHING recorded as edited since, newest
    /// last, at most `max`.
    ///
    /// This is the ONLY slice of the ledger fit to hand a verifier. The turn-start
    /// observation deliberately reports invalidated checks too — telling the
    /// planner "this ran green BUT these files changed" is the useful half there,
    /// because the planner can judge coverage. A VERIFY leg is a different
    /// contract: it is handed harness-owned IO facts to CITE, so a line it cannot
    /// verify is a line it can only be misled by. Anything with an edit after it
    /// is therefore withheld rather than qualified.
    ///
    /// Because the seam records this turn's edits in real time, an attempt that
    /// edited anything invalidates every earlier check by construction — which is
    /// exactly the guarantee that makes this safe to quote mid-turn.
    #[must_use]
    pub fn fresh_green_checks(&self, max: usize) -> Vec<&str> {
        let newest_edit = self.state.edits.last().map_or(0, |edit| edit.seq);
        let mut fresh: Vec<&str> = self
            .state
            .checks
            .iter()
            .filter(|check| check.seq > newest_edit)
            .map(|check| check.command.as_str())
            .collect();
        if fresh.len() > max {
            fresh.drain(..fresh.len() - max);
        }
        fresh
    }

    /// The turn in flight: events recorded since the last closed boundary
    /// carry this number.
    fn open_turn(&self) -> u64 {
        self.state.turn + 1
    }

    /// Paths edited in the open turn that no green check has followed — the
    /// receipt's "unverified" half. Empty when nothing was edited this turn or
    /// a check ran green after the last edit.
    #[must_use]
    pub fn unverified_edits_this_turn(&self) -> Vec<&str> {
        let newest_check = self.state.checks.last().map_or(0, |check| check.seq);
        let open_turn = self.open_turn();
        self.state
            .edits
            .iter()
            .filter(|edit| edit.turn == open_turn && edit.seq > newest_check)
            .map(|edit| edit.path.as_str())
            .collect()
    }

    /// Checks that ran green in the open turn AFTER its last edit — the
    /// receipt's evidence half, oldest first. Empty when the turn edited
    /// nothing (a check with nothing to vouch for is not a receipt).
    #[must_use]
    pub fn receipt_checks_this_turn(&self) -> Vec<&str> {
        let open_turn = self.open_turn();
        let Some(newest_edit) = self
            .state
            .edits
            .iter()
            .filter(|edit| edit.turn == open_turn)
            .map(|edit| edit.seq)
            .max()
        else {
            return Vec::new();
        };
        self.state
            .checks
            .iter()
            .filter(|check| check.turn == open_turn && check.seq > newest_edit)
            .map(|check| check.command.as_str())
            .collect()
    }

    /// Recorded commands, oldest first — diagnostics and tests.
    #[must_use]
    pub fn green_check_commands(&self) -> Vec<&str> {
        self.state
            .checks
            .iter()
            .map(|check| check.command.as_str())
            .collect()
    }

    /// Turns that contributed at least one recorded event.
    #[must_use]
    pub const fn recorded_turns(&self) -> u64 {
        self.state.turn
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.state.checks.is_empty() && self.state.edits.is_empty()
    }
}

/// One display line for a possibly multi-line command (heredoc check scripts are
/// routinely 1.9k chars): the first line, marked so the model can tell a
/// truncation from a one-liner rather than mistaking a prefix for the whole
/// command.
#[must_use]
pub fn first_line(command: &str) -> String {
    let mut lines = command.lines();
    let head = lines.next().unwrap_or_default().trim_end();
    if lines.next().is_some() {
        format!("{head} …")
    } else {
        head.to_string()
    }
}

/// Display-only shortening against the session's workspace root.
fn display_path(path: &str, root: Option<&Path>) -> String {
    let Some(root) = root else {
        return path.to_string();
    };
    Path::new(path)
        .strip_prefix(root)
        .map_or_else(|_| path.to_string(), |rest| rest.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::{
        VerifiedStateEvent, VerifiedStateLedger, RENDER_MAX_CHECKS, STORED_MAX_CHECKS,
        VERIFIED_STATE_REMINDER_PREFIX,
    };
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("zo-verified-state-{name}-{unique}"));
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    fn green(command: &str) -> VerifiedStateEvent {
        VerifiedStateEvent::GreenCheck(command.to_string())
    }

    fn edit(path: &str) -> VerifiedStateEvent {
        VerifiedStateEvent::Edit(path.to_string())
    }

    /// One whole turn's worth of facts, in arrival order, then its boundary —
    /// i.e. exactly what the runtime seam does across a turn, expressed in one
    /// call so these tests keep reading as "a turn recorded X".
    fn record_turn(ledger: &mut VerifiedStateLedger, events: &[VerifiedStateEvent]) {
        for event in events {
            ledger.record_event(event);
        }
        ledger.note_turn_boundary();
    }

    /// (a) A green check recorded in one turn is observable at the next turn's
    /// start, and reads as FRESH while nothing has been edited since.
    /// This turn's edits with no green check after them are the unverified
    /// half; a check after the last edit turns them into a receipt; and a
    /// session that never ran a check still remembers the open turn's edits.
    #[test]
    fn the_open_turn_knows_its_unverified_edits_and_its_receipt() {
        let mut ledger = VerifiedStateLedger::new();
        ledger.record_event(&VerifiedStateEvent::Edit("/ws/a.rs".into()));
        ledger.record_event(&VerifiedStateEvent::Edit("/ws/b.rs".into()));
        assert_eq!(ledger.unverified_edits_this_turn(), vec!["/ws/a.rs", "/ws/b.rs"]);
        assert!(ledger.receipt_checks_this_turn().is_empty());
        ledger.record_event(&VerifiedStateEvent::GreenCheck("cargo test".into()));
        assert!(ledger.unverified_edits_this_turn().is_empty());
        assert_eq!(ledger.receipt_checks_this_turn(), vec!["cargo test"]);
        ledger.record_event(&VerifiedStateEvent::Edit("/ws/c.rs".into()));
        assert_eq!(ledger.unverified_edits_this_turn(), vec!["/ws/c.rs"]);
        assert!(ledger.receipt_checks_this_turn().is_empty(), "an edit after the check voids it");
        ledger.note_turn_boundary();
        assert!(ledger.unverified_edits_this_turn().is_empty(), "a closed turn is not this turn");
        ledger.record_event(&VerifiedStateEvent::GreenCheck("just gate".into()));
        assert!(ledger.receipt_checks_this_turn().is_empty(), "a check with no edit this turn receipts nothing");
    }

    #[test]
    fn a_green_check_with_no_later_edit_renders_as_fresh() {
        let mut ledger = VerifiedStateLedger::new();
        record_turn(&mut ledger, &[green("cargo test -p runtime")]);
        let block = ledger.render_observation(None).expect("observation");
        assert!(block.starts_with(VERIFIED_STATE_REMINDER_PREFIX));
        assert!(block.contains("`cargo test -p runtime` — ran green, nothing edited since"));
        assert!(!block.contains("changed since"));
    }

    /// (b) An edit recorded AFTER the check invalidates it, and the changed
    /// paths are named — the harness reports the delta and judges nothing.
    #[test]
    fn an_edit_after_the_check_marks_it_invalidated_and_names_the_files() {
        let mut ledger = VerifiedStateLedger::new();
        record_turn(&mut ledger, &[
            green("cargo test -p runtime"),
            edit("/repo/crates/runtime/src/a.rs"),
        ]);
        let block = ledger
            .render_observation(Some(std::path::Path::new("/repo")))
            .expect("observation");
        assert!(
            block.contains(
                "`cargo test -p runtime` — ran green BUT these files changed since: \
                 crates/runtime/src/a.rs"
            ),
            "{block}"
        );
    }

    /// An edit that PRECEDES the check does not invalidate it — the check saw
    /// that edit's tree. This is the ordering the event slice encodes.
    #[test]
    fn an_edit_before_the_check_leaves_it_fresh() {
        let mut ledger = VerifiedStateLedger::new();
        record_turn(&mut ledger, &[edit("/repo/a.rs"), green("cargo test")]);
        let block = ledger.render_observation(None).expect("observation");
        assert!(block.contains("ran green, nothing edited since"), "{block}");
        // The now-unanswerable edit stays only while its turn is open — the
        // turn-end receipt asks about it — and the next event after the turn
        // closes prunes it rather than carrying it forever.
        assert_eq!(ledger.state.edits.len(), 1);
        ledger.note_turn_boundary();
        record_turn(&mut ledger, &[green("cargo test")]);
        assert!(ledger.state.edits.is_empty());
    }

    /// Re-running an invalidated check restores it to fresh, and drops the edit
    /// that can no longer invalidate anything.
    #[test]
    fn rerunning_a_check_restores_freshness_and_prunes_the_stale_edit() {
        let mut ledger = VerifiedStateLedger::new();
        record_turn(&mut ledger, &[green("cargo test"), edit("/repo/a.rs")]);
        assert!(ledger
            .render_observation(None)
            .expect("observation")
            .contains("changed since"));
        record_turn(&mut ledger, &[green("cargo test")]);
        let block = ledger.render_observation(None).expect("observation");
        assert!(block.contains("ran green, nothing edited since"), "{block}");
        assert_eq!(ledger.green_check_commands(), vec!["cargo test"]);
        assert!(ledger.state.edits.is_empty());
    }

    /// (c) No green check anywhere = no observation at all. This is the
    /// byte-neutrality pin: an edit-only session must render nothing.
    #[test]
    fn without_any_green_check_the_observation_is_empty() {
        let mut ledger = VerifiedStateLedger::new();
        assert!(ledger.render_observation(None).is_none());
        record_turn(&mut ledger, &[edit("/repo/a.rs"), edit("/repo/b.rs")]);
        assert!(
            ledger.render_observation(None).is_none(),
            "edits alone must not produce an observation"
        );
        record_turn(&mut ledger, &[]);
        assert_eq!(ledger.recorded_turns(), 1, "an empty turn records nothing");
    }

    /// (d) Sidecar round trip: a ledger written by one process is restored by a
    /// freshly rebound one, and still renders the same observation. This is the
    /// stage-boundary guarantee.
    #[test]
    fn sidecar_roundtrip_survives_a_new_ledger() {
        let dir = temp_dir("roundtrip");
        let sidecar = dir.join("session.verified-state.json");

        let mut writer = VerifiedStateLedger::new();
        writer.rebind_sidecar(Some(sidecar.clone()));
        record_turn(&mut writer, &[green("cargo test -p runtime"), edit("/repo/a.rs")]);
        assert!(sidecar.exists(), "record must write through");

        let mut resumed = VerifiedStateLedger::new();
        resumed.rebind_sidecar(Some(sidecar.clone()));
        let block = resumed.render_observation(None).expect("observation");
        assert!(block.contains("cargo test -p runtime"), "{block}");
        assert!(block.contains("/repo/a.rs"), "{block}");
        assert_eq!(resumed.recorded_turns(), 1);

        // A later stage keeps counting from where the previous one stopped.
        record_turn(&mut resumed, &[green("cargo clippy -p runtime")]);
        assert_eq!(resumed.recorded_turns(), 2);
        let mut third = VerifiedStateLedger::new();
        third.rebind_sidecar(Some(sidecar));
        assert_eq!(
            third.green_check_commands(),
            vec!["cargo test -p runtime", "cargo clippy -p runtime"]
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A corrupt sidecar degrades to empty (= byte-neutral), never to a wrong
    /// observation.
    #[test]
    fn a_corrupt_sidecar_degrades_to_empty() {
        let dir = temp_dir("corrupt");
        let sidecar = dir.join("session.verified-state.json");
        std::fs::write(&sidecar, b"{not json").expect("corrupt seed");

        let mut ledger = VerifiedStateLedger::new();
        ledger.rebind_sidecar(Some(sidecar));
        assert!(ledger.is_empty());
        assert!(ledger.render_observation(None).is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A conversation reset is a REBIND, not a wipe: the new session reads its
    /// own empty sidecar while the previous session's file is left intact, so
    /// `/resume` still finds the history it was promised. (Wiping in place is
    /// the documented trap on the read-registry swap.)
    #[test]
    fn rebinding_to_a_new_session_leaves_the_old_sidecar_intact() {
        let dir = temp_dir("rebind");
        let old = dir.join("old.verified-state.json");
        let new = dir.join("new.verified-state.json");

        let mut ledger = VerifiedStateLedger::new();
        ledger.rebind_sidecar(Some(old.clone()));
        record_turn(&mut ledger, &[green("cargo test -p runtime")]);

        ledger.rebind_sidecar(Some(new));
        assert!(ledger.is_empty(), "the fresh session starts empty");
        assert!(ledger.render_observation(None).is_none());

        let mut resumed = VerifiedStateLedger::new();
        resumed.rebind_sidecar(Some(old));
        assert_eq!(resumed.green_check_commands(), vec!["cargo test -p runtime"]);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// (e) Both caps keep the NEWEST evidence: storage prunes the oldest
    /// commands, and the render shows only the most recent ones.
    #[test]
    fn caps_keep_the_newest_checks() {
        let mut ledger = VerifiedStateLedger::new();
        for index in 0..(STORED_MAX_CHECKS + 4) {
            record_turn(&mut ledger, &[green(&format!("cargo test case{index}"))]);
        }
        let stored = ledger.green_check_commands();
        assert_eq!(stored.len(), STORED_MAX_CHECKS);
        assert_eq!(stored[0], "cargo test case4");
        assert_eq!(stored[STORED_MAX_CHECKS - 1], "cargo test case15");

        let block = ledger.render_observation(None).expect("observation");
        assert_eq!(
            block.matches("ran green").count(),
            RENDER_MAX_CHECKS,
            "render cap keeps the block short: {block}"
        );
        assert!(block.contains("cargo test case15"), "newest is kept");
        assert!(!block.contains("cargo test case9"), "oldest is dropped");
    }

    /// The per-line file cap summarizes the tail instead of printing a wall.
    #[test]
    fn the_file_list_is_capped_and_summarized() {
        let mut ledger = VerifiedStateLedger::new();
        let mut events = vec![green("cargo test")];
        for index in 0..12 {
            events.push(edit(&format!("/repo/f{index}.rs")));
        }
        record_turn(&mut ledger, &events);
        let block = ledger.render_observation(None).expect("observation");
        assert!(block.contains("(+4 more)"), "{block}");
        // Newest-first: the last edit leads the list.
        assert!(
            block.contains("changed since: /repo/f11.rs, /repo/f10.rs"),
            "{block}"
        );
        assert!(!block.contains("/repo/f3.rs"), "{block}");
    }

    /// The VERIFY-leg slice reports only checks with nothing edited since, and
    /// an edit recorded WITHOUT a turn boundary (i.e. mid-turn, the way the seam
    /// records) invalidates them immediately. This is the property the leg join
    /// was blocked on: mid-turn truth, not turn-end truth.
    #[test]
    fn fresh_green_checks_exclude_anything_edited_since_even_mid_turn() {
        let mut ledger = VerifiedStateLedger::new();
        record_turn(&mut ledger, &[green("cargo test"), green("cargo clippy")]);
        assert_eq!(
            ledger.fresh_green_checks(8),
            vec!["cargo test", "cargo clippy"]
        );

        // No boundary here: a turn is in flight and it just edited a file.
        ledger.record_event(&edit("/repo/a.rs"));
        assert!(
            ledger.fresh_green_checks(8).is_empty(),
            "an edit invalidates every earlier check the moment it lands"
        );

        // Re-running one restores exactly that one.
        ledger.record_event(&green("cargo test"));
        assert_eq!(ledger.fresh_green_checks(8), vec!["cargo test"]);
    }

    /// The leg cap keeps the NEWEST fresh commands, like every other cap here.
    #[test]
    fn fresh_green_checks_cap_keeps_the_newest() {
        let mut ledger = VerifiedStateLedger::new();
        for index in 0..5 {
            record_turn(&mut ledger, &[green(&format!("cargo test case{index}"))]);
        }
        assert_eq!(
            ledger.fresh_green_checks(2),
            vec!["cargo test case3", "cargo test case4"]
        );
        assert!(ledger.fresh_green_checks(0).is_empty());
    }

    /// A heredoc check script renders as ONE line — the observation must stay
    /// compact no matter how large the command that produced it was.
    #[test]
    fn a_multiline_command_renders_on_a_single_line() {
        let mut ledger = VerifiedStateLedger::new();
        record_turn(&mut ledger, &[green("python3 - <<'PY'\nassert 1\nPY")]);
        let block = ledger.render_observation(None).expect("observation");
        assert!(block.contains("`python3 - <<'PY' …` — ran green"), "{block}");
        assert!(!block.contains("assert 1"), "{block}");
    }
}
