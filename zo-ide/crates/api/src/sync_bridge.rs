//! Shared sync-context concurrency policy: the single sync↔async bridge
//! ([`run_blocking`]) and the workspace Mutex-poison recovery helper
//! ([`lock_recovered`]).
//!
//! Historically five hand-rolled copies of this bridge lived across the
//! workspace (api client token refresh, CLI auth / runtime-support / MCP /
//! LSP runtimes), each re-implementing the runtime-flavor check with a
//! subtly different fallback. The flavor check is load-bearing:
//! `tokio::task::block_in_place` panics on a `current_thread` runtime
//! (tokio docs: "This function panics if called from a `current_thread`
//! runtime"), so any copy that skips or botches the branch is a latent
//! panic path. This module is the one implementation; the former copies
//! delegate here.

use std::future::Future;
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};

use tokio::runtime::{Handle, RuntimeFlavor};

/// Acquire `mutex`, recovering the guard when a previous holder panicked.
///
/// Workspace poison policy — decided per protected *type*, not per call
/// site, and recorded as a comment where the type is declared:
/// - Recover with this helper when the protected data's invariants hold at
///   every possible panic point under the lock (single-field replaces,
///   append-only pushes, full-value map inserts). For such types the poison
///   flag carries no usable information: the data is exactly as consistent
///   as before the panicking holder acquired the lock, while propagating
///   the panic would brick every later user of the value.
/// - Keep `.lock().expect("<why unrecoverable>")` when a writer can panic
///   between dependent mutations, leaving half-updated state that later
///   readers must not observe.
pub fn lock_recovered<T: ?Sized>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Lazily-built private runtime backing [`run_blocking`] whenever the
/// ambient runtime (if any) cannot be re-entered. `current_thread` flavor:
/// it spawns no idle worker threads, and the thread parked inside
/// `Runtime::block_on` drives the IO/time drivers itself. `enable_all` is
/// required — bridged futures use timers and sockets (OAuth, MCP, SSE).
fn fallback_runtime() -> &'static tokio::runtime::Runtime {
    static FALLBACK: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    FALLBACK.get_or_init(|| {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("sync_bridge fallback runtime: building a current_thread runtime only fails when the OS denies an IO/time driver")
    })
}

/// Drive `fut` to completion from a synchronous context, whether or not a
/// tokio runtime is already active on this thread.
///
/// Branches (tokio 1.x documented semantics):
/// - **Ambient multi-thread runtime** — `block_in_place` hands other tasks
///   off this worker before blocking, then `Handle::block_on` re-enters the
///   async context; this is the exact bridge pattern the `Handle::block_on`
///   and `block_in_place` docs prescribe.
/// - **Ambient `current_thread` runtime** — `block_in_place` would panic
///   there, and `Handle::block_on` from a helper thread only makes progress
///   while the runtime's owner thread happens to be parked in `block_on`
///   (nothing else drives its IO/time drivers). The private fallback
///   runtime owns its drivers, so the future progresses unconditionally.
/// - **No ambient runtime** — plain sync entry point: fallback runtime.
///
/// Like every blocking bridge, this must not be called from *async* code on
/// a `current_thread` runtime — the only driver thread would block itself,
/// and tokio aborts with "Cannot start a runtime from within a runtime"
/// (the per-copy implementations this replaces failed the same way).
pub fn run_blocking<F: Future>(fut: F) -> F::Output {
    match Handle::try_current() {
        Ok(handle) if handle.runtime_flavor() == RuntimeFlavor::MultiThread => {
            tokio::task::block_in_place(|| handle.block_on(fut))
        }
        _ => fallback_runtime().block_on(fut),
    }
}

#[cfg(test)]
mod tests {
    use super::run_blocking;

    /// Exercises the fallback runtime's time driver: if `fallback_runtime`
    /// ever loses `enable_all`, this sleep panics with "no reactor running".
    const DRIVER_PROBE_SLEEP: std::time::Duration = std::time::Duration::from_millis(1);

    async fn probe() -> u32 {
        tokio::time::sleep(DRIVER_PROBE_SLEEP).await;
        42
    }

    #[test]
    fn no_ambient_runtime_uses_fallback() {
        assert_eq!(run_blocking(probe()), 42);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn multi_thread_ambient_reenters_via_block_in_place() {
        assert_eq!(run_blocking(probe()), 42);
    }

    #[test]
    fn lock_recovered_survives_a_poisoned_mutex() {
        let cell = std::sync::Mutex::new(7_u32);
        let poison = std::thread::scope(|scope| {
            scope
                .spawn(|| {
                    let _guard = cell.lock().expect("first acquire");
                    panic!("poison the lock on purpose");
                })
                .join()
        });
        assert!(poison.is_err(), "holder thread must have panicked");
        assert_eq!(*super::lock_recovered(&cell), 7);
    }

    #[tokio::test]
    async fn current_thread_ambient_routes_helper_threads_to_fallback() {
        // spawn_blocking threads inherit the runtime context, so the bridge
        // sees a CurrentThread handle and must take the fallback branch
        // (block_in_place would panic, Handle::block_on could stall).
        let out = tokio::task::spawn_blocking(|| run_blocking(probe()))
            .await
            .expect("spawn_blocking join");
        assert_eq!(out, 42);
    }
}
