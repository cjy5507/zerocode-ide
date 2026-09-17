//! Library surface for `zo-cli`.
//!
//! The crate is primarily a binary (`src/main.rs`), but a thin library
//! target is exposed so integration tests and future embedders can reach
//! the extracted Phase 3 modules directly without shelling out to the
//! `zo` binary.
//!
//! Publicly exported modules:
//! - [`sinks`] — output sinks (Text, Json, Ndjson) for non-TTY paths (L4)
//! - [`tui`]   — ratatui-based terminal UI foundation (L2)
//! - [`util`]  — cross-cutting helpers shared between bin and lib (ANSI,
//!   future text utilities). Keep this layer leaf-level — no
//!   dependency on `tui`, `session`, or runtime types.
//!
//! Nothing under `main.rs` (legacy line-mode renderer, session manager,
//! input reader, slash commands) is re-exported: those stay private to
//! the binary until Lane L7 wires them into the new TUI. See
//! `code-rules.md` R1 / R2.

pub mod sinks;
pub mod tui;
pub mod util;

/// Lib-target twin of the bin's `test_env_lock` (see `main.rs`): one
/// process-wide lock every lib test that mutates a shared environment
/// variable must hold. The lib and bin test binaries are separate processes,
/// so each having its own single lock is correct — the invariant is ONE lock
/// per process, because per-module `static ENV_LOCK`s only serialize within
/// their own file and let cross-module tests stomp each other's `set_var`.
#[cfg(test)]
pub(crate) fn test_env_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| {
        // Mirror the bin lock's init guard: tests must never consult the
        // developer's real Claude Code keychain (see `main.rs`).
        std::env::set_var("ZO_DISABLE_KEYCHAIN", "1");
        std::sync::Mutex::new(())
    })
    .lock()
    .unwrap_or_else(std::sync::PoisonError::into_inner)
}
