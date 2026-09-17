//! Process-global live output tails for foreground subprocess tools.
//!
//! Registrations are RAII-owned: dropping [`LiveOutputHandle`] removes the
//! exact entry it installed, including on unwind and early-return paths. The
//! registry retains only a bounded tail per stream; callers that need a
//! lossless final result must keep their own full capture buffer.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

const RETAINED_TAIL_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveTailSnapshot {
    pub stdout_tail: String,
    pub stderr_tail: String,
    pub last_output_age: Option<Duration>,
    pub elapsed: Duration,
}

#[derive(Debug)]
struct Entry {
    key: String,
    sequence: u64,
    started_at: Instant,
    last_output_at: Mutex<Option<Instant>>,
    stdout: Mutex<Vec<u8>>,
    stderr: Mutex<Vec<u8>>,
}

#[derive(Default)]
struct Registry {
    entries: HashMap<String, Arc<Entry>>,
}

fn registry() -> &'static Mutex<Registry> {
    static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(Registry::default()))
}

fn next_sequence() -> u64 {
    static SEQUENCE: AtomicU64 = AtomicU64::new(1);
    SEQUENCE.fetch_add(1, Ordering::Relaxed)
}

/// Owns one live registration. Dropping it removes only this exact entry, so a
/// newer run that reused the same key cannot be removed by the older guard.
#[derive(Debug)]
pub struct LiveOutputHandle {
    entry: Arc<Entry>,
}

impl LiveOutputHandle {
    #[must_use]
    pub fn key(&self) -> &str {
        &self.entry.key
    }

    #[must_use]
    pub fn writer(&self) -> LiveOutputWriter {
        LiveOutputWriter {
            entry: Arc::clone(&self.entry),
        }
    }

    #[must_use]
    pub fn tail(&self, max_bytes: usize) -> LiveTailSnapshot {
        snapshot_entry(&self.entry, max_bytes)
    }
}

impl Drop for LiveOutputHandle {
    fn drop(&mut self) {
        let mut registry = registry()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if registry
            .entries
            .get(&self.entry.key)
            .is_some_and(|entry| Arc::ptr_eq(entry, &self.entry))
        {
            registry.entries.remove(&self.entry.key);
        }
    }
}

/// Cloneable writer handed to stdout/stderr reader tasks. It does not own the
/// registration lifetime; the parent [`LiveOutputHandle`] remains the RAII
/// cleanup guard.
#[derive(Debug, Clone)]
pub struct LiveOutputWriter {
    entry: Arc<Entry>,
}

impl LiveOutputWriter {
    pub fn append_stdout(&self, chunk: &[u8]) {
        self.append(&self.entry.stdout, chunk);
    }

    pub fn append_stderr(&self, chunk: &[u8]) {
        self.append(&self.entry.stderr, chunk);
    }

    fn append(&self, stream: &Mutex<Vec<u8>>, chunk: &[u8]) {
        if chunk.is_empty() {
            return;
        }
        let mut buffer = stream
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        retain_tail(&mut buffer, chunk, RETAINED_TAIL_BYTES);
        drop(buffer);
        *self
            .entry
            .last_output_at
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Instant::now());
    }
}

/// Register a foreground run. `None` or an empty key gets a generated id and
/// remains discoverable through [`current`], which is the v1 fallback for
/// dispatch paths that do not carry a tool-call id.
#[must_use]
pub fn register(key: Option<&str>) -> LiveOutputHandle {
    let sequence = next_sequence();
    let key = key
        .filter(|key| !key.is_empty())
        .map_or_else(|| format!("live-run-{sequence}"), ToOwned::to_owned);
    let entry = Arc::new(Entry {
        key: key.clone(),
        sequence,
        started_at: Instant::now(),
        last_output_at: Mutex::new(None),
        stdout: Mutex::new(Vec::new()),
        stderr: Mutex::new(Vec::new()),
    });
    registry()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .entries
        .insert(key, Arc::clone(&entry));
    LiveOutputHandle { entry }
}

/// Snapshot a keyed still-running entry, copying at most `max_bytes` from each
/// stream tail.
#[must_use]
pub fn snapshot(key: &str, max_bytes: usize) -> Option<LiveTailSnapshot> {
    let entry = registry()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .entries
        .get(key)
        .cloned()?;
    Some(snapshot_entry(&entry, max_bytes))
}

/// Snapshot the most recently registered still-running entry. Normal TUI Bash
/// dispatch is keyed by tool-call id; this accessor is the documented fallback
/// for direct/headless paths and is not parallel-safe attribution by itself.
#[must_use]
pub fn current(max_bytes: usize) -> Option<LiveTailSnapshot> {
    let entry = registry()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .entries
        .values()
        .max_by_key(|entry| entry.sequence)
        .cloned()?;
    Some(snapshot_entry(&entry, max_bytes))
}

fn snapshot_entry(entry: &Entry, max_bytes: usize) -> LiveTailSnapshot {
    let stdout_tail = tail_string(&entry.stdout, max_bytes);
    let stderr_tail = tail_string(&entry.stderr, max_bytes);
    let last_output_age = entry
        .last_output_at
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .map(|at| Instant::now().saturating_duration_since(at));
    LiveTailSnapshot {
        stdout_tail,
        stderr_tail,
        last_output_age,
        elapsed: entry.started_at.elapsed(),
    }
}

fn tail_string(stream: &Mutex<Vec<u8>>, max_bytes: usize) -> String {
    let buffer = stream
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let start = buffer.len().saturating_sub(max_bytes);
    String::from_utf8_lossy(&buffer[start..]).into_owned()
}

fn retain_tail(buffer: &mut Vec<u8>, chunk: &[u8], cap: usize) {
    if cap == 0 {
        buffer.clear();
    } else if chunk.len() >= cap {
        buffer.clear();
        buffer.extend_from_slice(&chunk[chunk.len() - cap..]);
    } else {
        let overflow = buffer.len().saturating_add(chunk.len()).saturating_sub(cap);
        if overflow > 0 {
            buffer.drain(..overflow);
        }
        buffer.extend_from_slice(chunk);
    }
}

thread_local! {
    static DISPATCH_KEY: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Run `f` with the server-injected tool-call id visible to Bash dispatch.
/// Restores the previous value on normal return or unwind.
#[doc(hidden)]
pub fn with_dispatch_key<T>(key: Option<&str>, f: impl FnOnce() -> T) -> T {
    struct Restore(Option<String>);
    impl Drop for Restore {
        fn drop(&mut self) {
            DISPATCH_KEY.with(|slot| {
                slot.replace(self.0.take());
            });
        }
    }

    let previous = DISPATCH_KEY.with(|slot| slot.replace(key.map(str::to_owned)));
    let _restore = Restore(previous);
    f()
}

pub(crate) fn dispatch_key() -> Option<String> {
    DISPATCH_KEY.with(|slot| slot.borrow().clone())
}

/// Strip terminal CSI/OSC sequences and resolve carriage-return repaint lines
/// without touching printable Unicode.
#[must_use]
pub fn sanitize_live_output(input: &str) -> String {
    let mut visible = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\u{1b}' {
            visible.push(ch);
            continue;
        }
        match chars.next() {
            Some('[') => {
                for next in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&next) {
                        break;
                    }
                }
            }
            Some(']') => {
                while let Some(next) = chars.next() {
                    if next == '\u{7}' {
                        break;
                    }
                    if next == '\u{1b}' && matches!(chars.peek(), Some('\\')) {
                        chars.next();
                        break;
                    }
                }
            }
            Some(_) | None => {}
        }
    }

    let mut out = String::with_capacity(visible.len());
    for (index, line) in visible.split('\n').enumerate() {
        if index > 0 {
            out.push('\n');
        }
        let repainted = line.rsplit('\r').next().unwrap_or_default();
        for ch in repainted.chars() {
            if ch == '\t' {
                out.push(' ');
            } else if !ch.is_control() {
                out.push(ch);
            }
        }
    }
    out
}
