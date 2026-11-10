//! A test binary that runs turns without naming a workspace roots their traces
//! (`.zo/turns`, `.zo/dream`) at the working folder, and `cargo test` gives a
//! test binary its crate's folder as the working folder: every run left its
//! session logs in `crates/runtime/.zo`, where `.gitignore` hides them
//! (112 files from one run, t-78045). The `cfg(test)` fallback of the runtime
//! crate does not reach an integration test, which links the library built
//! without it. `ZO_TRACE_ROOT` names a folder of the process's own instead.

/// Call first in every test that runs a turn. The first call sets the variable
/// for the whole process; later calls do nothing.
pub fn isolate() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        // The folders of runs that ended go first: nothing removes a process's own when it ends.
        let _ = core_types::ended_process_folders::remove_folders_of_ended_processes(
            &std::env::temp_dir(),
            "zo-runtime-itest-traces-",
        );
        let folder = std::env::temp_dir()
            .join(format!("zo-runtime-itest-traces-{}", std::process::id()));
        std::env::set_var("ZO_TRACE_ROOT", folder);
    });
}
