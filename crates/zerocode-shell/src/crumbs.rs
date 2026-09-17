//! Bounded, nonblocking process breadcrumbs. No disk or heap on the write road.

use std::fmt::{self, Write as _};
use std::sync::atomic::{AtomicU8, AtomicU64, AtomicUsize, Ordering::SeqCst};

use crate::crash::Limits;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct Crumb {
    pub(crate) seq: u64,
    pub(crate) at_ms: u64,
    pub(crate) line: String,
}

struct Slot {
    stamp: AtomicU64,
    at_ms: AtomicU64,
    len: AtomicUsize,
    bytes: [AtomicU8; Limits::TEXT_BYTES],
}

impl Slot {
    const fn new() -> Self {
        Self {
            stamp: AtomicU64::new(0),
            at_ms: AtomicU64::new(0),
            len: AtomicUsize::new(0),
            bytes: [const { AtomicU8::new(0) }; Limits::TEXT_BYTES],
        }
    }
}

pub(crate) struct RingStorage<const N: usize> {
    next: AtomicU64,
    slots: [Slot; N],
}

impl<const N: usize> RingStorage<N> {
    pub(crate) const fn new() -> Self {
        Self {
            next: AtomicU64::new(1),
            slots: [const { Slot::new() }; N],
        }
    }

    pub(crate) fn write(&self, at_ms: u64, line: &str) {
        let text = masked(line);
        let seq = self.next.fetch_add(1, SeqCst);
        let slot = &self.slots[(seq as usize - 1) % N];
        let before = slot.stamp.load(SeqCst);
        // One attempt, never a spin or a wait. A paused writer costs one slot;
        // every other writer can progress. Late writers cannot replace newer
        // generations. All payload cells are atomic, including during a read.
        if !before.is_multiple_of(2)
            || before >= seq * 2
            || slot
                .stamp
                .compare_exchange(before, seq * 2 + 1, SeqCst, SeqCst)
                .is_err()
        {
            return;
        }
        slot.at_ms.store(at_ms, SeqCst);
        for (destination, byte) in slot.bytes.iter().zip(text.as_str().bytes()) {
            destination.store(byte, SeqCst);
        }
        slot.len.store(text.len, SeqCst);
        slot.stamp.store(seq * 2, SeqCst);
    }

    pub(crate) fn snapshot(&self) -> Vec<Crumb> {
        let head = self.next.load(SeqCst);
        let floor = head.saturating_sub(N as u64);
        let mut rows = Vec::new();
        for slot in &self.slots {
            let stamp = slot.stamp.load(SeqCst);
            let seq = stamp / 2;
            if stamp == 0 || !stamp.is_multiple_of(2) || seq < floor || seq >= head {
                continue;
            }
            let at_ms = slot.at_ms.load(SeqCst);
            let len = slot.len.load(SeqCst).min(Limits::TEXT_BYTES);
            let bytes: Vec<_> = slot.bytes[..len]
                .iter()
                .map(|byte| byte.load(SeqCst))
                .collect();
            // SeqCst brackets every payload load in the same total order as
            // writes. A concurrent replacement makes this generation disappear.
            if slot.stamp.load(SeqCst) != stamp {
                continue;
            }
            if let Ok(line) = String::from_utf8(bytes) {
                rows.push(Crumb { seq, at_ms, line });
            }
        }
        rows.sort_unstable_by_key(|row| row.seq);
        rows
    }
}

pub(crate) struct Text {
    bytes: [u8; Limits::TEXT_BYTES],
    len: usize,
}

impl Text {
    pub(crate) fn new() -> Self {
        Self {
            bytes: [0; Limits::TEXT_BYTES],
            len: 0,
        }
    }
    pub(crate) fn as_str(&self) -> &str {
        std::str::from_utf8(&self.bytes[..self.len]).unwrap_or("")
    }
}

impl fmt::Write for Text {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let mut n = s.len().min(self.bytes.len() - self.len);
        while !s.is_char_boundary(n) {
            n -= 1;
        }
        self.bytes[self.len..self.len + n].copy_from_slice(&s.as_bytes()[..n]);
        self.len += n;
        Ok(())
    }
}

pub(crate) fn masked(raw: &str) -> Text {
    let mut text = Text::new();
    for word in raw.split_whitespace() {
        if text.len > 0 {
            let _ = text.write_char(' ');
        }
        // The credential words are the core's one table
        // (`zerocode_core::credential`); a crumb masks more than a credential,
        // because a crash report can reach a stranger: an address, a home
        // path or a URL is somebody's, too. A long mixed word that is a
        // symbol path (`native::samples_1::objc_release`) survives the core's
        // opaque-word rule — the 2026-09-10 hang crumbs lost all 63 frames
        // to it before the `::` exception.
        let sensitive_key = zerocode_core::credential::names_a_credential(word)
            || zerocode_core::credential::contains_ascii_case(word, "email");
        let sensitive_value = zerocode_core::credential::looks_like_a_credential(word)
            || PRIVATE_MARKS
                .iter()
                .any(|mark| zerocode_core::credential::contains_ascii_case(word, mark));
        if sensitive_key || sensitive_value {
            let _ = text.write_str("[redacted]");
            // Header/key values may contain whitespace, commas or JSON quotes.
            // Once a credential key is seen the rest of this bounded line goes.
            if sensitive_key {
                break;
            }
        } else {
            for ch in word.chars().filter(|ch| !ch.is_control()) {
                let _ = text.write_char(ch);
            }
        }
        if text.len == Limits::TEXT_BYTES {
            break;
        }
    }
    text
}

/// What a crumb masks beyond a credential: a person's home path, a URL, an
/// address.
const PRIVATE_MARKS: [&str; 5] = ["/Users/", "/home/", "\\Users\\", "://", "@"];

pub(crate) type Ring = RingStorage<{ Limits::RING_SIZE }>;

static RING: Ring = Ring::new();
static LAST_COMMAND: RingStorage<1> = RingStorage::new();

pub(crate) fn record(kind: &str, detail: fmt::Arguments<'_>) {
    let mut line = Text::new();
    let _ = write!(line, "{kind} {detail}");
    RING.write(crate::now_epoch_ms().max(0) as u64, line.as_str());
}

pub(crate) fn snapshot() -> Vec<Crumb> {
    RING.snapshot()
}

pub(crate) fn last_command() -> String {
    LAST_COMMAND
        .snapshot()
        .last()
        .map(|row| row.line.clone())
        .unwrap_or_else(|| "none".into())
}

// Main-thread scopes are distinct from historical crumbs: a completed command
// must never be named as the work still blocking the event loop.
static MAIN_SCOPE: RingStorage<1> = RingStorage::new();
static MAIN_THREAD: std::sync::OnceLock<std::thread::ThreadId> = std::sync::OnceLock::new();

pub(crate) fn register_main_thread() {
    let _ = MAIN_THREAD.set(std::thread::current().id());
}

pub(crate) fn is_main_thread() -> bool {
    MAIN_THREAD.get() == Some(&std::thread::current().id())
}
thread_local! {
    static SCOPE_NAME: std::cell::Cell<&'static str> = const { std::cell::Cell::new("native_event_loop") };
}

pub(crate) fn main_scope() -> String {
    MAIN_SCOPE
        .snapshot()
        .last()
        .map(|row| row.line.clone())
        .unwrap_or_else(|| "native_event_loop".into())
}

pub(crate) struct MainScope {
    previous: Option<(&'static str, std::thread::ThreadId)>,
}

impl MainScope {
    pub(crate) fn enter(name: &'static str) -> Self {
        Self::enter_if(
            name,
            MAIN_THREAD.get() == Some(&std::thread::current().id()),
        )
    }

    fn enter_if(name: &'static str, on_main: bool) -> Self {
        let previous = on_main.then(|| {
            let previous = SCOPE_NAME.with(|held| held.replace(name));
            MAIN_SCOPE.write(crate::now_epoch_ms().max(0) as u64, name);
            (previous, std::thread::current().id())
        });
        Self { previous }
    }
}

impl Drop for MainScope {
    fn drop(&mut self) {
        if let Some((previous, owner)) = self.previous
            && owner == std::thread::current().id()
        {
            SCOPE_NAME.with(|held| held.set(previous));
            MAIN_SCOPE.write(crate::now_epoch_ms().max(0) as u64, previous);
        }
    }
}

/// Command history records both synchronous and asynchronous roads. Only a
/// command actually running on the registered main thread changes its scope.
/// Unwinding still records leave; the panic hook sees enter before that unwind.
pub(crate) struct Command {
    _scope: MainScope,
    name: &'static str,
    start: std::time::Instant,
}

impl Command {
    pub(crate) fn enter(name: &'static str) -> Self {
        LAST_COMMAND.write(crate::now_epoch_ms().max(0) as u64, name);
        record("command", format_args!("enter {name}"));
        Self {
            _scope: MainScope::enter(name),
            name,
            start: std::time::Instant::now(),
        }
    }
}

impl Drop for Command {
    fn drop(&mut self) {
        record(
            "command",
            format_args!(
                "leave {} ms={}",
                self.name,
                self.start.elapsed().as_millis()
            ),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn main_scope_leaves_completed_commands_and_restores_nested_events_on_unwind() {
        let initial = SCOPE_NAME.with(|held| held.get());
        {
            let _event = MainScope::enter_if("event_window_resized", true);
            let _ = std::panic::catch_unwind(|| {
                let _command = MainScope::enter_if("ledger_agents", true);
                assert_eq!(SCOPE_NAME.with(|held| held.get()), "ledger_agents");
                panic!("fixture interrupted the command");
            });
            assert_eq!(SCOPE_NAME.with(|held| held.get()), "event_window_resized");
        }
        assert_eq!(SCOPE_NAME.with(|held| held.get()), initial);
    }

    #[test]
    fn a_background_command_cannot_claim_the_main_scope() {
        let _event = MainScope::enter_if("event_window_resized", true);
        let _background = MainScope::enter_if("update_check", false);
        assert_eq!(SCOPE_NAME.with(|held| held.get()), "event_window_resized");
    }

    #[test]
    fn ring_bounds_orders_and_never_waits_for_a_stalled_slot() {
        let ring = Ring::new();
        for n in 0..Limits::RING_SIZE * 3 {
            ring.write(n as u64, "boot stage");
        }
        let rows = ring.snapshot();
        assert_eq!(rows.len(), Limits::RING_SIZE);
        assert!(rows.windows(2).all(|pair| pair[0].seq < pair[1].seq));
        assert_eq!(rows[0].at_ms, (Limits::RING_SIZE * 2) as u64);
        ring.slots[0].stamp.store(1, SeqCst); // paused writer
        for _ in 0..Limits::RING_SIZE * 2 {
            ring.write(999, "boot ready");
        }
        assert_eq!(ring.snapshot().len(), Limits::RING_SIZE - 1);
    }

    #[test]
    fn secrets_are_masked_before_they_enter_the_ring() {
        let ring = Ring::new();
        for secret in [
            "AKIAIOSFODNN7EXAMPLE",
            "glpat-test",
            "npm_abcdef",
            "sk_test_abcdef",
            "gho_abcdef",
            "sk-proj-test123456789",
            "ghp_123456789",
            "github_pat_test123",
            "xoxb-test",
            "eyJhbGciOi.test.signature",
            "person@example.org",
            "/Users/person/private/file",
            "C:\\Users\\person\\private",
            "token=short",
            "Authorization: Bearer short",
            "Cookie: session=short",
            "password short",
            "email short",
        ] {
            ring.write(1, &format!("hook {secret}"));
            let rows = ring.snapshot();
            let line = &rows.last().unwrap().line;
            assert!(!line.contains(secret), "leaked {secret}: {line}");
            assert!(line.contains("[redacted]"), "not masked: {line}");
        }
    }

    /// A backtrace frame is a symbol path, not a credential: the hang sample's
    /// `native::samples_N::symbol` words are long and mixed, and the credential
    /// heuristic blanked every one of them (63 `[redacted]` crumbs on
    /// 2026-09-10) while the crash record kept the same frames.
    #[test]
    fn symbol_frames_survive_the_credential_heuristic() {
        let ring = Ring::new();
        for frame in [
            "native::samples_1::___NSTrackingAreaAKManager__updateActiveTrackingAreasForWindowLocation:modifierFlags:_",
            "native::samples_1::tao::platform_impl::platform::window::send_event::h7f78a2b3f8b21b08",
            "12: zerocode_shell::orchestration::refresh_board_ledger",
        ] {
            ring.write(1, &format!("main_sample_after_detection {frame}"));
            let line = ring.snapshot().last().unwrap().line.clone();
            assert!(line.contains(frame), "frame masked: {line}");
        }
        // A token-shaped word without `::` still goes.
        ring.write(1, "hook Xk9pQ2mZ7vL4nR8sT1wY6bC3dF5g");
        let line = ring.snapshot().last().unwrap().line.clone();
        assert!(line.contains("[redacted]"), "token kept: {line}");
    }

    #[test]
    fn concurrent_writers_never_publish_torn_rows() {
        let ring = std::sync::Arc::new(Ring::new());
        std::thread::scope(|scope| {
            for _ in 0..8 {
                let ring = &ring;
                scope.spawn(move || {
                    for _ in 0..10000 {
                        ring.write(4, "hook pane=7 Working");
                    }
                });
            }
            for _ in 0..100 {
                for row in ring.snapshot() {
                    assert_eq!(row.line, "hook pane=7 Working");
                }
            }
        });
        assert!(!ring.snapshot().is_empty());
    }

    #[test]
    fn crumb_write_cost() {
        let ring = Ring::new();
        let start = std::time::Instant::now();
        let count = 100000;
        for _ in 0..count {
            ring.write(7, std::hint::black_box("command enter list_dir"));
        }
        let cost = start.elapsed().as_nanos() / count;
        eprintln!("crumb_write_ns={cost} count={count}");
        if !cfg!(debug_assertions) {
            assert!(
                cost < Limits::WRITE_BUDGET_NS,
                "crumb write exceeded its release budget"
            );
        }
        assert_eq!(ring.snapshot().len(), Limits::RING_SIZE);
    }
}
