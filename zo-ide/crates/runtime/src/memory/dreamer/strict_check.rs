use std::fs;
#[cfg(any(target_os = "linux", test))]
use std::ffi::OsString;
use std::io::Read;
use std::path::Path;
use std::process::{Command, ExitStatus};
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::process::ChildStderr;

#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::path::PathBuf;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::process::Stdio;

use decision_core::dreamer::PatchCheckResult;

use super::QuarantineCheckCommand;

const CHECK_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const CHECK_DIAGNOSTIC_CAP: usize = 8 * 1024;
const CHECK_DIAGNOSTIC_DRAIN_GRACE: Duration = Duration::from_millis(250);

pub(super) fn run(
    worktree: &Path,
    check_state: &Path,
    check: &QuarantineCheckCommand,
) -> PatchCheckResult {
    let command_line = vec![check.program.clone(), "<arguments-redacted>".to_string()];
    let failed = |reason: &str| PatchCheckResult {
        name: check.name.clone(),
        command: command_line.clone(),
        exit_code: None,
        success: false,
        stdout: String::new(),
        stderr: reason.to_string(),
    };
    if check.program.trim().is_empty() {
        return failed("empty_check_program");
    }
    if let Err(reason) = prepare_check_state(check_state) {
        return if cleanup_check_state(check_state).is_err() {
            failed("check_state_cleanup_failed")
        } else {
            failed(&reason)
        };
    }
    let result = match strict_check_command(worktree, check_state, check) {
        Ok(mut command) => match spawn_and_wait(&mut command) {
            Ok((status, diagnostic)) => PatchCheckResult {
                name: check.name.clone(),
                command: command_line.clone(),
                exit_code: status.code(),
                success: status.success(),
                stdout: String::new(),
                stderr: classify_check_diagnostic(
                    status.success(),
                    &diagnostic,
                    worktree,
                    check_state,
                ),
            },
            Err(reason) => failed(reason),
        },
        Err(reason) => failed(&reason),
    };
    if cleanup_check_state(check_state).is_err() {
        return failed("check_state_cleanup_failed");
    }
    result
}

fn cleanup_check_state(check_state: &Path) -> Result<(), ()> {
    match fs::remove_dir_all(check_state) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(()),
    }
}

fn prepare_check_state(check_state: &Path) -> Result<(), String> {
    let parent = check_state
        .parent()
        .ok_or_else(|| "check_state_parent_unavailable".to_string())?;
    let name = check_state
        .file_name()
        .ok_or_else(|| "check_state_name_unavailable".to_string())?;
    crate::secure_fs::ensure_private_dir(parent, Path::new(name))
        .map_err(|_| "check_state_unavailable".to_string())?;
    for child in ["home", "tmp", "cargo-home", "target", "rustup-home", "root"] {
        crate::secure_fs::ensure_private_dir(check_state, Path::new(child))
            .map_err(|_| "check_state_unavailable".to_string())?;
    }
    crate::secure_fs::write_atomic_owner_only(
        check_state,
        Path::new("cargo-home/config.toml"),
        b"[net]\noffline = true\n",
    )
    .map_err(|_| "check_state_unavailable".to_string())?;
    Ok(())
}

fn spawn_and_wait(command: &mut Command) -> Result<(ExitStatus, Vec<u8>), &'static str> {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.process_group(0);
    }
    let mut child = command.spawn().map_err(|_| "check_spawn_error")?;
    #[cfg(unix)]
    let mut diagnostic = match child.stderr.take() {
        Some(stderr) => {
            if let Ok(diagnostic) = DiagnosticDrain::start(stderr) {
                Some(diagnostic)
            } else {
                terminate_check_tree(child.id());
                let _ = child.wait();
                return Err("check_diagnostic_setup_error");
            }
        }
        None => None,
    };
    #[cfg(not(unix))]
    let diagnostic = ();
    let deadline = Instant::now() + CHECK_TIMEOUT;
    loop {
        #[cfg(unix)]
        if let Some(diagnostic) = diagnostic.as_mut() {
            diagnostic.drain();
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                terminate_check_tree(child.id());
                #[cfg(unix)]
                return Ok((status, finish_diagnostic(diagnostic)));
                #[cfg(not(unix))]
                return Ok((status, Vec::new()));
            }
            Ok(None) if Instant::now() >= deadline => {
                terminate_check_tree(child.id());
                let _ = child.wait();
                #[cfg(unix)]
                let _ = finish_diagnostic(diagnostic);
                return Err("check_timeout");
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(_) => {
                terminate_check_tree(child.id());
                let _ = child.wait();
                #[cfg(unix)]
                let _ = finish_diagnostic(diagnostic);
                return Err("check_wait_error");
            }
        }
    }
}

#[cfg(unix)]
struct DiagnosticDrain {
    reader: ChildStderr,
    retained: Vec<u8>,
    closed: bool,
}

#[cfg(unix)]
impl DiagnosticDrain {
    fn start(reader: ChildStderr) -> Result<Self, ()> {
        use std::os::fd::AsRawFd as _;

        use nix::fcntl::{FcntlArg, OFlag, fcntl};

        let flags = fcntl(reader.as_raw_fd(), FcntlArg::F_GETFL).map_err(|_| ())?;
        let flags = OFlag::from_bits_truncate(flags);
        fcntl(
            reader.as_raw_fd(),
            FcntlArg::F_SETFL(flags | OFlag::O_NONBLOCK),
        )
        .map_err(|_| ())?;
        Ok(Self {
            reader,
            retained: Vec::with_capacity(CHECK_DIAGNOSTIC_CAP),
            closed: false,
        })
    }

    fn drain(&mut self) {
        let mut buffer = [0_u8; 8 * 1024];
        loop {
            match self.reader.read(&mut buffer) {
                Ok(0) => {
                    self.closed = true;
                    break;
                }
                Ok(read) => retain_diagnostic(&mut self.retained, &buffer[..read]),
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(_) => {
                    self.closed = true;
                    break;
                }
            }
        }
    }

    fn finish(mut self) -> Vec<u8> {
        let deadline = Instant::now() + CHECK_DIAGNOSTIC_DRAIN_GRACE;
        loop {
            self.drain();
            if self.closed || Instant::now() >= deadline {
                return self.retained;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

#[cfg(unix)]
fn retain_diagnostic(retained: &mut Vec<u8>, bytes: &[u8]) {
    let remaining = CHECK_DIAGNOSTIC_CAP.saturating_sub(retained.len());
    retained.extend_from_slice(&bytes[..remaining.min(bytes.len())]);
}

#[cfg(unix)]
fn finish_diagnostic(diagnostic: Option<DiagnosticDrain>) -> Vec<u8> {
    diagnostic.map(DiagnosticDrain::finish).unwrap_or_default()
}

fn classify_check_diagnostic(
    success: bool,
    bytes: &[u8],
    worktree: &Path,
    check_state: &Path,
) -> String {
    if success {
        return String::new();
    }
    let diagnostic = String::from_utf8_lossy(bytes);
    if diagnostic.contains("Cargo.lock") && diagnostic.contains("--locked") {
        "check_failed_locked_state".to_string()
    } else if diagnostic.contains("could not execute process") {
        "check_failed_process_execution".to_string()
    } else if diagnostic.contains("failed to create")
        || diagnostic.contains("could not create")
    {
        "check_failed_state_create".to_string()
    } else if diagnostic.contains("failed to open") {
        "check_failed_file_open".to_string()
    } else if diagnostic.contains("failed to write") {
        "check_failed_state_write".to_string()
    } else if diagnostic.contains("rustup could not choose") {
        "check_failed_rustup_resolution".to_string()
    } else if diagnostic.contains("failed to get") {
        "check_failed_offline_dependency".to_string()
    } else if diagnostic.contains("Operation not permitted")
        || diagnostic.contains("Permission denied")
    {
        format!(
            "check_failed_sandbox_permission_{}",
            diagnostic_scope(&diagnostic, worktree, check_state)
        )
    } else {
        "check_failed_diagnostic_redacted".to_string()
    }
}

fn diagnostic_scope(diagnostic: &str, worktree: &Path, check_state: &Path) -> &'static str {
    let contains_path = |path: &Path| diagnostic.contains(&path.to_string_lossy().into_owned());
    if contains_path(check_state) {
        return "check_state";
    }
    if contains_path(worktree) {
        return "worktree";
    }
    for (name, scope) in [
        ("CARGO_HOME", "cargo_cache"),
        ("RUSTUP_HOME", "rustup"),
        ("HOME", "home"),
        ("TMPDIR", "temp"),
    ] {
        if let Some(path) = std::env::var_os(name).map(std::path::PathBuf::from) {
            if contains_path(&path) {
                return scope;
            }
        }
    }
    if diagnostic.contains("rustc") || diagnostic.contains("rustup") {
        "toolchain"
    } else if diagnostic.contains("cargo") {
        "cargo"
    } else {
        "host"
    }
}

#[cfg(unix)]
fn terminate_check_tree(pid: u32) {
    use nix::sys::signal::{Signal, killpg};
    use nix::unistd::Pid;

    let Ok(pid) = i32::try_from(pid) else {
        return;
    };
    let _ = killpg(Pid::from_raw(pid), Signal::SIGKILL);
}

#[cfg(not(unix))]
fn terminate_check_tree(_pid: u32) {}

fn strict_check_command(
    worktree: &Path,
    check_state: &Path,
    check: &QuarantineCheckCommand,
) -> Result<Command, String> {
    #[cfg(target_os = "linux")]
    {
        strict_linux_command(worktree, check_state, check)
    }
    #[cfg(target_os = "macos")]
    {
        strict_macos_command(worktree, check_state, check)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = (worktree, check_state, check);
        Err("strict_filesystem_network_isolation_unavailable".to_string())
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn canonical_trusted_file(worktree: &Path, candidate: &Path) -> Result<PathBuf, String> {
    if !candidate.is_absolute() {
        return Err("trusted_check_program_unavailable".to_string());
    }
    let worktree = fs::canonicalize(worktree)
        .map_err(|_| "trusted_check_program_unavailable".to_string())?;
    let resolved = fs::canonicalize(candidate)
        .map_err(|_| "trusted_check_program_unavailable".to_string())?;
    if resolved.starts_with(&worktree) || !resolved.is_file() {
        return Err("trusted_check_program_unavailable".to_string());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        if fs::metadata(&resolved)
            .map_err(|_| "trusted_check_program_unavailable".to_string())?
            .permissions()
            .mode()
            & 0o111
            == 0
        {
            return Err("trusted_check_program_unavailable".to_string());
        }
    }
    Ok(resolved)
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn canonical_child(root: &Path, child: &Path) -> Result<PathBuf, String> {
    if child.is_absolute()
        || !child
            .components()
            .all(|component| matches!(component, std::path::Component::Normal(_)))
    {
        return Err("trusted_check_path_unavailable".to_string());
    }
    let root = fs::canonicalize(root).map_err(|_| "trusted_check_path_unavailable".to_string())?;
    let resolved = fs::canonicalize(root.join(child))
        .map_err(|_| "trusted_check_path_unavailable".to_string())?;
    if !resolved.starts_with(&root) {
        return Err("trusted_check_path_unavailable".to_string());
    }
    Ok(resolved)
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn trusted_state_root(worktree: &Path, explicit: &str, fallback: &str) -> Option<PathBuf> {
    let value = std::env::var_os(explicit)
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(fallback)))?;
    if !value.is_absolute() {
        return None;
    }
    let resolved_worktree = fs::canonicalize(worktree).ok()?;
    let resolved = fs::canonicalize(value).ok()?;
    (resolved.is_dir() && !resolved.starts_with(resolved_worktree)).then_some(resolved)
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn trusted_cache_child(root: Option<&Path>, child: &str) -> Option<PathBuf> {
    root.and_then(|root| canonical_child(root, Path::new(child)).ok())
        .filter(|path| path.is_dir())
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn trusted_toolchain_root(worktree: &Path, program: &Path) -> Option<PathBuf> {
    let rustup_home = trusted_state_root(worktree, "RUSTUP_HOME", ".rustup")?;
    let toolchains = canonical_child(&rustup_home, Path::new("toolchains")).ok()?;
    let bin = program.parent()?;
    if bin.file_name().and_then(|name| name.to_str()) != Some("bin") {
        return None;
    }
    let toolchain = bin.parent()?.to_path_buf();
    (toolchain.parent() == Some(toolchains.as_path()) && toolchain.is_dir()).then_some(toolchain)
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn trusted_cargo_binary(worktree: &Path) -> Result<PathBuf, String> {
    for candidate in [Path::new("/usr/bin/cargo"), Path::new("/usr/local/bin/cargo")] {
        if let Ok(program) = canonical_trusted_file(worktree, candidate) {
            return Ok(program);
        }
    }
    let Some(rustup_home) = trusted_state_root(worktree, "RUSTUP_HOME", ".rustup") else {
        return Err("trusted_check_program_unavailable".to_string());
    };
    let toolchains = canonical_child(&rustup_home, Path::new("toolchains"))?;
    let entries = fs::read_dir(&toolchains).map_err(|_| "trusted_check_program_unavailable".to_string())?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let candidate = Path::new("toolchains").join(name).join("bin/cargo");
        let Ok(program) = canonical_child(&rustup_home, &candidate) else {
            continue;
        };
        if let Ok(program) = canonical_trusted_file(worktree, &program) {
            return Ok(program);
        }
    }
    Err("trusted_check_program_unavailable".to_string())
}

/// Resolve Git from fixed operating-system toolchain locations. This is shared
/// by metadata discovery and outer quarantine operations so an inherited PATH
/// can never choose the Git binary that mutates a quarantine worktree.
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(super) fn trusted_git_binary(worktree: &Path) -> Result<PathBuf, String> {
    #[cfg(target_os = "macos")]
    let candidates = [
        Path::new("/Applications/Xcode.app/Contents/Developer/usr/bin/git"),
        Path::new("/Library/Developer/CommandLineTools/usr/bin/git"),
    ];
    #[cfg(target_os = "linux")]
    let candidates = [Path::new("/usr/bin/git"), Path::new("/bin/git")];
    candidates
        .into_iter()
        .find_map(|candidate| canonical_trusted_file(worktree, candidate).ok())
        .ok_or_else(|| "trusted_check_program_unavailable".to_string())
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn trusted_check_program(worktree: &Path, program: &str) -> Result<PathBuf, String> {
    match program {
        "cargo" => trusted_cargo_binary(worktree),
        "git" => trusted_git_binary(worktree),
        _ => Err("unapproved_check_program".to_string()),
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn trusted_git_metadata_paths(worktree: &Path, git: &Path) -> Result<Vec<PathBuf>, String> {
    let worktree = fs::canonicalize(worktree)
        .map_err(|_| "trusted_git_metadata_unavailable".to_string())?;
    let mut metadata_paths: Vec<PathBuf> = Vec::new();
    for flag in ["--absolute-git-dir", "--git-common-dir"] {
        let output = Command::new(git)
            .arg("-C")
            .arg(&worktree)
            .arg("rev-parse")
            .arg(flag)
            .env_clear()
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .output()
            .map_err(|_| "trusted_git_metadata_unavailable".to_string())?;
        if !output.status.success() {
            continue;
        }
        let value = std::str::from_utf8(&output.stdout)
            .map_err(|_| "trusted_git_metadata_unavailable".to_string())?
            .trim();
        if value.is_empty() {
            continue;
        }
        let path = PathBuf::from(value);
        let resolved = fs::canonicalize(if path.is_absolute() {
            path
        } else {
            worktree.join(path)
        })
        .map_err(|_| "trusted_git_metadata_unavailable".to_string())?;
        if !resolved.is_dir() || metadata_paths.iter().any(|existing| existing == &resolved) {
            continue;
        }
        if metadata_paths.iter().any(|existing| resolved.starts_with(existing)) {
            continue;
        }
        metadata_paths.retain(|existing| !existing.starts_with(&resolved));
        metadata_paths.push(resolved);
    }
    Ok(metadata_paths)
}

#[cfg(any(target_os = "linux", test))]
const LINUX_SCRIPT_FIXED_ARGUMENTS: usize = 10;

#[cfg(any(target_os = "linux", test))]
fn linux_script_arguments(
    fixed_paths: [&Path; 9],
    program_name: &str,
    check_args: &[String],
) -> Vec<OsString> {
    let [
        sandbox_root,
        worktree,
        check_state,
        program,
        cargo_registry,
        cargo_git,
        toolchain,
        metadata_one,
        metadata_two,
    ] = fixed_paths;
    let mut arguments = Vec::with_capacity(LINUX_SCRIPT_FIXED_ARGUMENTS + check_args.len());
    arguments.extend([
        sandbox_root.as_os_str().to_os_string(),
        worktree.as_os_str().to_os_string(),
        check_state.as_os_str().to_os_string(),
        program.as_os_str().to_os_string(),
        OsString::from(program_name),
        cargo_registry.as_os_str().to_os_string(),
        cargo_git.as_os_str().to_os_string(),
        toolchain.as_os_str().to_os_string(),
        metadata_one.as_os_str().to_os_string(),
        metadata_two.as_os_str().to_os_string(),
    ]);
    arguments.extend(check_args.iter().map(OsString::from));
    arguments
}

#[cfg(target_os = "linux")]
const LINUX_STRICT_CHECK_SCRIPT: &str = r#"
set -eu
root=$1
workspace=$2
state=$3
program=$4
program_name=$5
cargo_registry=$6
cargo_git=$7
toolchain=$8
git_metadata_one=$9
git_metadata_two=${10}
shift 10
mount --make-rprivate /
mount -t tmpfs -o mode=0700 tmpfs "$root"
for source in /usr /bin /lib /lib64 /etc /opt /nix/store /run/current-system; do
  if [ -e "$source" ]; then
    mkdir -p "$root$source"
    mount --bind "$source" "$root$source"
    mount -o remount,bind,ro "$root$source"
  fi
done
mkdir -p "$root/workspace" "$root/state/home" "$root/state/tmp" "$root/state/cargo-home" "$root/state/target" "$root/check-bin" "$root/toolchain" "$root/dev" "$root/proc"
mount --bind "$workspace" "$root/workspace"
mount -o remount,bind,ro "$root/workspace"
for child in home tmp cargo-home target; do
  mount --bind "$state/$child" "$root/state/$child"
done
touch "$root/check-bin/$program_name"
mount --bind "$program" "$root/check-bin/$program_name"
mount -o remount,bind,ro "$root/check-bin/$program_name"
if [ -n "$cargo_registry" ]; then
  mkdir -p "$root/state/cargo-home/registry"
  mount --bind "$cargo_registry" "$root/state/cargo-home/registry"
  mount -o remount,bind,ro "$root/state/cargo-home/registry"
fi
if [ -n "$cargo_git" ]; then
  mkdir -p "$root/state/cargo-home/git"
  mount --bind "$cargo_git" "$root/state/cargo-home/git"
  mount -o remount,bind,ro "$root/state/cargo-home/git"
fi
if [ -n "$toolchain" ]; then
  mount --bind "$toolchain" "$root/toolchain"
  mount -o remount,bind,ro "$root/toolchain"
fi
for metadata in "$git_metadata_one" "$git_metadata_two"; do
  if [ -n "$metadata" ]; then
    mkdir -p "$root$metadata"
    mount --bind "$metadata" "$root$metadata"
    mount -o remount,bind,ro "$root$metadata"
  fi
done
for device in null random urandom; do
  touch "$root/dev/$device"
  mount --bind "/dev/$device" "$root/dev/$device"
done
mount -t proc proc "$root/proc"
exec chroot "$root" /bin/sh -c 'cd /workspace && exec "$@"' zo-check \
  /usr/bin/env -i PATH=/check-bin:/toolchain/bin HOME=/state/home TMPDIR=/state/tmp TMP=/state/tmp TEMP=/state/tmp \
  CARGO_HOME=/state/cargo-home CARGO_TARGET_DIR=/state/target CARGO_NET_OFFLINE=true \
  "/check-bin/$program_name" "$@"
"#;

#[cfg(target_os = "linux")]
fn strict_linux_command(
    worktree: &Path,
    check_state: &Path,
    check: &QuarantineCheckCommand,
) -> Result<Command, String> {
    const SCRIPT: &str = LINUX_STRICT_CHECK_SCRIPT;

    let request = crate::sandbox::SandboxRequest {
        enabled: true,
        namespace_restrictions: true,
        network_isolation: true,
        filesystem_mode: crate::sandbox::FilesystemIsolationMode::WorkspaceOnly,
        allowed_mounts: Vec::new(),
    };
    let status = crate::sandbox::resolve_sandbox_status_for_request(&request, worktree);
    if !status.namespace_active || !status.network_active {
        return Err("strict_filesystem_network_isolation_unavailable".to_string());
    }
    let unshare = [Path::new("/usr/bin/unshare"), Path::new("/bin/unshare")]
        .into_iter()
        .find_map(|path| canonical_trusted_file(worktree, path).ok())
        .ok_or_else(|| "strict_filesystem_network_isolation_unavailable".to_string())?;
    let program = trusted_check_program(worktree, &check.program)?;
    let metadata_git = trusted_git_binary(worktree)?;
    let git_metadata = trusted_git_metadata_paths(worktree, &metadata_git)?;
    let cargo_home = trusted_state_root(worktree, "CARGO_HOME", ".cargo");
    let cargo_registry = trusted_cache_child(cargo_home.as_deref(), "registry").unwrap_or_default();
    let cargo_git = trusted_cache_child(cargo_home.as_deref(), "git").unwrap_or_default();
    let toolchain = trusted_toolchain_root(worktree, &program).unwrap_or_default();
    let metadata_one = git_metadata.first().cloned().unwrap_or_default();
    let metadata_two = git_metadata.get(1).cloned().unwrap_or_default();
    let sandbox_root = check_state.join("root");
    let mut command = Command::new(unshare);
    command
        .args([
            "--user",
            "--map-root-user",
            "--mount",
            "--ipc",
            "--pid",
            "--uts",
            "--fork",
            "--net",
            "sh",
            "-c",
            SCRIPT,
            "zo-check",
        ])
        .args(linux_script_arguments(
            [
                &sandbox_root,
                worktree,
                check_state,
                &program,
                &cargo_registry,
                &cargo_git,
                &toolchain,
                &metadata_one,
                &metadata_two,
            ],
            &check.program,
            &check.args,
        ))
        .current_dir(worktree)
        .env_clear()
        .env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    Ok(command)
}

#[cfg(target_os = "macos")]
fn strict_macos_command(
    worktree: &Path,
    check_state: &Path,
    check: &QuarantineCheckCommand,
) -> Result<Command, String> {
    use std::os::unix::fs::symlink;

    let sandbox_exec = canonical_trusted_file(worktree, Path::new("/usr/bin/sandbox-exec"))
        .map_err(|_| "strict_filesystem_network_isolation_unavailable".to_string())?;
    let check_state = fs::canonicalize(check_state)
        .map_err(|_| "strict_filesystem_network_isolation_unavailable".to_string())?;
    let program = trusted_check_program(worktree, &check.program)?;
    let toolchain = trusted_toolchain_root(worktree, &program);
    let metadata_git = trusted_git_binary(worktree)?;
    let git_metadata = trusted_git_metadata_paths(worktree, &metadata_git)?;
    let cargo_home = trusted_state_root(worktree, "CARGO_HOME", ".cargo");
    let isolated_cargo = check_state.join("cargo-home");
    let mut cargo_cache_reads = Vec::new();
    for child in ["registry", "git"] {
        let Some(source) = trusted_cache_child(cargo_home.as_deref(), child) else {
            continue;
        };
        let destination = isolated_cargo.join(child);
        if !destination.exists() {
            symlink(&source, &destination).map_err(|_| "sandbox_cache_unavailable".to_string())?;
        }
        cargo_cache_reads.push(source);
    }
    let profile = strict_macos_profile(
        worktree,
        &check_state,
        &program,
        toolchain.as_deref(),
        &cargo_cache_reads,
        &git_metadata,
    );
    let mut command = Command::new(sandbox_exec);
    command
        .arg("-p")
        .arg(profile)
        .arg(&program)
        .args(&check.args)
        .current_dir(worktree)
        .env_clear()
        .env("PATH", program.parent().unwrap_or(Path::new("/usr/bin")))
        .env("HOME", check_state.join("home"))
        .env("CFFIXED_USER_HOME", check_state.join("home"))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("TMPDIR", check_state.join("tmp"))
        .env("TMP", check_state.join("tmp"))
        .env("TEMP", check_state.join("tmp"))
        .env("CARGO_HOME", &isolated_cargo)
        .env("RUSTUP_HOME", check_state.join("rustup-home"))
        .env("CARGO_TARGET_DIR", check_state.join("target"))
        .env("CARGO_NET_OFFLINE", "true")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    Ok(command)
}

#[cfg(target_os = "macos")]
fn strict_macos_profile(
    worktree: &Path,
    check_state: &Path,
    program: &Path,
    toolchain: Option<&Path>,
    cargo_cache_reads: &[PathBuf],
    git_metadata: &[PathBuf],
) -> String {
    use std::fmt::Write as _;

    let mut reads = vec![
        worktree.to_path_buf(),
        check_state.to_path_buf(),
        PathBuf::from("/System"),
        PathBuf::from("/usr/bin"),
        PathBuf::from("/usr/lib"),
        PathBuf::from("/usr/share"),
        PathBuf::from("/bin"),
        PathBuf::from("/sbin"),
        PathBuf::from("/Library/Apple"),
        PathBuf::from("/Applications/Xcode.app"),
        PathBuf::from("/Library/Developer/CommandLineTools"),
        PathBuf::from("/private/var/db"),
        PathBuf::from("/Library/Preferences"),
        PathBuf::from("/private/etc/localtime"),
        PathBuf::from("/private/etc/ssl/openssl.cnf"),
        program.to_path_buf(),
        program.parent().unwrap_or(Path::new("/usr/bin")).to_path_buf(),
    ];
    if let Some(toolchain) = toolchain {
        reads.push(toolchain.to_path_buf());
    }
    reads.extend(cargo_cache_reads.iter().cloned());
    reads.extend(git_metadata.iter().cloned());
    let mut profile = String::from(
        r#"(version 1)
(deny default)
(deny network*)
(allow process-fork)
(allow process-info* (target self))
(allow signal (target self))
(allow sysctl-read)
(allow user-preference-read
  (preference-domain "kCFPreferencesAnyApplication"))
"#,
    );
    let reads: Vec<_> = reads
        .into_iter()
        .filter(|path| path.exists())
        .map(|path| fs::canonicalize(&path).unwrap_or(path))
        .collect();
    let metadata_ancestors: std::collections::BTreeSet<_> = reads
        .iter()
        .flat_map(|path| path.ancestors().skip(1))
        .filter(|path| *path != Path::new("/"))
        .map(Path::to_path_buf)
        .collect();
    profile.push_str("(allow file-read-data\n");
    for path in &metadata_ancestors {
        let _ = writeln!(profile, "  (literal {})", sbpl_quote(path));
    }
    profile.push_str(")\n");
    profile.push_str("(allow process-exec*\n");
    let posix_shell = fs::canonicalize("/bin/sh").unwrap_or_else(|_| PathBuf::from("/bin/sh"));
    for path in [
        program.to_path_buf(),
        program.parent().unwrap_or(Path::new("/usr/bin")).to_path_buf(),
        posix_shell,
        PathBuf::from("/bin"),
        PathBuf::from("/usr/bin"),
    ] {
        let _ = writeln!(profile, "  (subpath {})", sbpl_quote(&path));
    }
    if let Some(toolchain) = toolchain {
        let _ = writeln!(profile, "  (subpath {})", sbpl_quote(toolchain));
    }
    profile.push_str(")\n");
    // Seatbelt requires a literal root-directory data permission to traverse
    // canonical absolute paths for trusted executables. This is intentionally
    // not a root subpath grant: content access remains limited to `reads`.
    profile.push_str("(allow file-read-data (literal \"/\"))\n");
    profile.push_str("(allow file-read-metadata (literal \"/\"))\n");
    profile.push_str("(allow file-read-metadata\n");
    for path in &metadata_ancestors {
        let _ = writeln!(profile, "  (literal {})", sbpl_quote(path));
    }
    profile.push_str(")\n");
    for operation in ["file-read*", "file-read-metadata"] {
        let _ = writeln!(profile, "(allow {operation}");
        for path in &reads {
            let _ = writeln!(profile, "  (subpath {})", sbpl_quote(path));
        }
        profile.push_str(
            "  (literal \"/dev/null\")\n  (literal \"/dev/random\")\n  (literal \"/dev/urandom\"))\n",
        );
    }
    profile.push_str("(allow file-write*\n");
    let state = fs::canonicalize(check_state).unwrap_or_else(|_| check_state.to_path_buf());
    let _ = writeln!(profile, "  (subpath {})", sbpl_quote(&state));
    profile.push_str(
        "  (literal \"/dev/null\")\n  (literal \"/dev/stdout\")\n  (literal \"/dev/stderr\"))\n",
    );
    profile
}

#[cfg(target_os = "macos")]
fn sbpl_quote(path: &Path) -> String {
    let path = path.display().to_string();
    format!("\"{}\"", path.replace('\\', "\\\\").replace('"', "\\\""))
}

#[cfg(test)]
mod linux_argument_tests {
    use super::*;

    #[test]
    fn linux_launcher_preserves_ten_fixed_arguments_and_user_arguments() {
        let worktree = Path::new("/worktree");
        let check_state = Path::new("/check-state");
        let check = QuarantineCheckCommand {
            name: "argument probe".to_string(),
            program: "cargo".to_string(),
            args: vec![
                "--message-format=json".to_string(),
                "argument with spaces".to_string(),
                String::new(),
            ],
        };
        let sandbox_root = check_state.join("root");
        let arguments = linux_script_arguments(
            [
                &sandbox_root,
                worktree,
                check_state,
                Path::new("/toolchain/bin/cargo"),
                Path::new("/cargo/registry"),
                Path::new("/cargo/git"),
                Path::new("/toolchain"),
                Path::new("/git/dir"),
                Path::new("/git/common"),
            ],
            &check.program,
            &check.args,
        );
        let fixed = vec![
            OsString::from("/check-state/root"),
            OsString::from("/worktree"),
            OsString::from("/check-state"),
            OsString::from("/toolchain/bin/cargo"),
            OsString::from("cargo"),
            OsString::from("/cargo/registry"),
            OsString::from("/cargo/git"),
            OsString::from("/toolchain"),
            OsString::from("/git/dir"),
            OsString::from("/git/common"),
        ];

        assert_eq!(fixed.len(), 10);
        assert_eq!(&arguments[..fixed.len()], fixed.as_slice());
        assert_eq!(
            &arguments[fixed.len()..],
            [
                OsString::from("--message-format=json"),
                OsString::from("argument with spaces"),
                OsString::new(),
            ]
        );
    }
}

#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
mod resolver_tests {
    use super::*;

    const PATH_PROBE_ENV: &str = "ZO_STRICT_CHECK_PATH_PROBE";
    const TEST_NAME: &str = "memory::dreamer::strict_check::resolver_tests::trusted_git_binary_ignores_malicious_inherited_path";

    #[test]
    fn trusted_git_binary_ignores_malicious_inherited_path() {
        if let Some(fake_dir) = std::env::var_os(PATH_PROBE_ENV) {
            let fake_dir = PathBuf::from(fake_dir);
            let temp = tempfile::tempdir().unwrap();
            let resolved = trusted_git_binary(temp.path()).expect("system Git resolves directly");
            assert!(
                !resolved.starts_with(&fake_dir),
                "resolver selected inherited PATH Git: {}",
                resolved.display()
            );
            return;
        }

        let temp = tempfile::tempdir().unwrap();
        let fake_dir = temp.path().join("malicious-bin");
        fs::create_dir(&fake_dir).unwrap();
        fs::write(fake_dir.join("git"), "#!/bin/sh\nexit 99\n").unwrap();
        let output = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", TEST_NAME, "--nocapture"])
            .env_clear()
            .env("PATH", &fake_dir)
            .env(PATH_PROBE_ENV, &fake_dir)
            .output()
            .expect("path-isolated test process starts");

        assert!(
            output.status.success(),
            "trusted resolver accepted inherited PATH: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[cfg(all(test, unix))]
mod stderr_tests {
    use super::*;

    #[test]
    fn verbose_stderr_is_drained_after_the_capture_limit() {
        let mut command = Command::new("/bin/sh");
        command
            .args([
                "-c",
                "i=0; while [ \"$i\" -lt 4096 ]; do \
                 printf '0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\\n' \
                 >&2 || exit 91; i=$((i + 1)); done",
            ])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped());

        let (status, diagnostic) = spawn_and_wait(&mut command).expect("child exits normally");

        assert!(status.success(), "verbose child failed: {status:?}");
        assert_eq!(diagnostic.len(), CHECK_DIAGNOSTIC_CAP);
    }
    #[test]
    fn stderr_drain_deadline_handles_a_descendant_that_keeps_the_pipe_open() {
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", "sleep 5 >&2 & exit 0"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped());

        let started = Instant::now();
        let (status, diagnostic) = spawn_and_wait(&mut command).expect("parent exits normally");

        assert!(status.success(), "parent failed: {status:?}");
        assert!(diagnostic.is_empty());
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "stderr drain exceeded its deadline: {:?}",
            started.elapsed()
        );
    }
}

#[cfg(all(test, target_os = "macos"))]
mod macos_tests {
    use super::*;

    fn with_env_var<T>(name: &str, value: &Path, f: impl FnOnce() -> T) -> T {
        let previous = std::env::var_os(name);
        std::env::set_var(name, value);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
        match previous {
            Some(value) => std::env::set_var(name, value),
            None => std::env::remove_var(name),
        }
        match result {
            Ok(value) => value,
            Err(payload) => std::panic::resume_unwind(payload),
        }
    }

    #[test]
    fn profile_is_default_deny_with_only_check_state_writable() {
        let temp = tempfile::tempdir().unwrap();
        let worktree = temp.path().join("worktree");
        let check_state = temp.path().join("check-state");
        fs::create_dir_all(&worktree).unwrap();
        fs::create_dir_all(&check_state).unwrap();

        let profile = strict_macos_profile(
            &worktree,
            &check_state,
            Path::new("/usr/bin/git"),
            None,
            &[],
            &[],
        );
        let worktree = fs::canonicalize(&worktree).unwrap();
        let check_state = fs::canonicalize(&check_state).unwrap();

        assert!(profile.starts_with("(version 1)\n(deny default)\n"));
        assert!(!profile.contains("(allow default)"));
        assert!(profile.contains("(deny network*)"));
        for operation in [
            "(allow process-fork)",
            "(allow process-info* (target self))",
            "(allow signal (target self))",
            "(allow sysctl-read)",
        ] {
            assert!(profile.contains(operation), "missing {operation}: {profile}");
        }
        for broad_operation in [
            "(allow process-exec*)",
            "(allow process*)",
            "(allow mach",
            "(allow ipc",
            "(allow file-read*)",
            "(deny file-read*",
        ] {
            assert!(
                !profile.contains(broad_operation),
                "unexpected broad operation {broad_operation}: {profile}"
            );
        }
        assert_eq!(profile.matches("(allow file-read*").count(), 1);
        for sensitive_root in [
            "/Users",
            "/home",
            "/root",
            "/private/var/folders",
            "/private/var/tmp",
            "/private/tmp",
            "/tmp",
            "/Volumes",
            "/Network",
            "/Library",
            "/opt/homebrew",
        ] {
            assert!(
                !profile.contains(&format!("(subpath \"{sensitive_root}\")")),
                "unexpected broad read root {sensitive_root}: {profile}"
            );
        }
        assert!(profile.contains("(allow file-read-metadata (literal \"/\"))"));
        assert!(profile.contains("(allow file-read-data (literal \"/\"))"));
        assert!(!profile.contains("(subpath \"/\")"));
        assert!(profile.contains("(allow process-exec*\n"));
        assert!(profile.contains(&format!(
            "(subpath {})",
            sbpl_quote(&fs::canonicalize("/bin/sh").unwrap())
        )));
        assert!(profile.contains(&format!("(subpath {})", sbpl_quote(&worktree))));
        assert!(profile.contains(&format!("(subpath {})", sbpl_quote(&check_state))));
        for developer_root in [
            Path::new("/Applications/Xcode.app"),
            Path::new("/Library/Developer/CommandLineTools"),
        ]
        .into_iter()
        .filter(|root| root.exists())
        {
            let developer_root = fs::canonicalize(developer_root).unwrap();
            assert!(
                profile.contains(&format!("(subpath {})", sbpl_quote(&developer_root))),
                "missing canonical developer root {}: {profile}",
                developer_root.display()
            );
        }
        let writable = profile
            .split("(allow file-write*\n")
            .nth(1)
            .expect("profile has a write allow-list");
        assert!(writable.contains(&sbpl_quote(&check_state)));
        assert!(!writable.contains(&sbpl_quote(&worktree)));
        assert!(!profile.contains("(subpath \"/dev\")"));
    }

    #[test]
    fn profile_grants_resolved_cargo_cache_and_toolchain_reads() {
        let temp = tempfile::tempdir().unwrap();
        let worktree = temp.path().join("worktree");
        let check_state = temp.path().join("check-state");
        let registry = temp.path().join("cargo-home/registry");
        let git = temp.path().join("cargo-home/git");
        let toolchain = temp.path().join("rustup/toolchains/test-toolchain");
        for path in [&worktree, &check_state, &registry, &git, &toolchain] {
            fs::create_dir_all(path).unwrap();
        }
        let registry = fs::canonicalize(registry).unwrap();
        let git = fs::canonicalize(git).unwrap();
        let toolchain = fs::canonicalize(toolchain).unwrap();

        let profile = strict_macos_profile(
            &worktree,
            &check_state,
            Path::new("/usr/bin/git"),
            Some(&toolchain),
            &[registry.clone(), git.clone()],
            &[],
        );
        let data_reads = profile
            .split("(allow file-read*\n")
            .nth(1)
            .and_then(|rest| rest.split(")\n(allow file-read-metadata\n").next())
            .expect("profile has a file data read allow-list");

        for path in [&registry, &git, &toolchain] {
            assert!(
                data_reads.contains(&format!("(subpath {})", sbpl_quote(path))),
                "missing resolved read path {}: {profile}",
                path.display()
            );
        }
    }

    #[test]
    fn default_deny_uses_a_direct_xcode_git_binary() {
        let temp = tempfile::tempdir().unwrap();
        let program = trusted_git_binary(temp.path())
            .expect("git is available through the system toolchain");

        assert_ne!(program, PathBuf::from("/usr/bin/git"));
        let trusted_roots = [
            Path::new("/Applications/Xcode.app/Contents/Developer"),
            Path::new("/Library/Developer/CommandLineTools"),
        ]
        .into_iter()
        .filter_map(|root| fs::canonicalize(root).ok())
        .collect::<Vec<_>>();
        assert!(!trusted_roots.is_empty(), "no trusted developer root found");
        assert!(
            trusted_roots.iter().any(|root| program.starts_with(root)),
            "unexpected direct git path: {}",
            program.display()
        );
    }

    #[test]
    fn strict_check_denies_host_reads_writes_and_network_without_output() {
        use std::io::Write as _;
        use std::net::TcpListener;

        let _lock = crate::test_env_lock();
        let temp = tempfile::tempdir().unwrap();
        let worktree = temp.path().join("worktree");
        let check_state = temp.path().join("check-state");
        let secret = temp.path().join("host-secret");
        let host_write = temp.path().join("host-write");
        fs::create_dir_all(&worktree).unwrap();
        fs::write(&secret, "host-secret\n").unwrap();
        assert!(Path::new("/usr/bin/curl").is_file());

        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let connection_seen = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(1);
            while Instant::now() < deadline {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");
                        return true;
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(_) => return false,
                }
            }
            false
        });

        let alias = format!(
            "!if test -e '{}'; then exit 10; fi; \\
             if cat '{}' >/dev/null 2>&1; then exit 11; fi; \\
             if touch '{}' >/dev/null 2>&1; then exit 12; fi; \\
             if /usr/bin/curl --fail --silent --show-error --connect-timeout 1 http://{} \\
             >/dev/null 2>&1; then exit 13; fi",
            secret.display(),
            secret.display(),
            host_write.display(),
            address,
        );
        let check = QuarantineCheckCommand {
            name: "deny host capabilities".to_string(),
            program: "git".to_string(),
            args: vec![
                "-c".to_string(),
                format!("alias.strict-check-probe={alias}"),
                "strict-check-probe".to_string(),
            ],
        };

        let result = run(&worktree, &check_state, &check);

        assert!(result.success, "unexpected check result: {result:?}");
        assert!(result.stdout.is_empty());
        assert!(result.stderr.is_empty());
        assert!(!host_write.exists());
        assert!(!check_state.exists());
        assert!(!connection_seen.join().unwrap());
    }

    #[test]
    fn strict_check_reads_through_isolated_cargo_home_symlink() {
        let _lock = crate::test_env_lock();
        let temp = tempfile::tempdir().unwrap();
        let worktree = temp.path().join("worktree");
        let check_state = temp.path().join("check-state");
        let cargo_home = temp.path().join("host-cargo-home");
        fs::create_dir_all(&worktree).unwrap();
        fs::create_dir_all(cargo_home.join("registry")).unwrap();
        fs::write(cargo_home.join("registry/cache-probe"), "cache-ok\n").unwrap();
        let check = QuarantineCheckCommand {
            name: "read cargo cache symlink".to_string(),
            program: "git".to_string(),
            args: vec![
                "-c".to_string(),
                "alias.strict-check-probe=!/bin/cat \"$CARGO_HOME/registry/cache-probe\" >/dev/null"
                    .to_string(),
                "strict-check-probe".to_string(),
            ],
        };

        let result = with_env_var("CARGO_HOME", &cargo_home, || {
            run(&worktree, &check_state, &check)
        });

        assert!(result.success, "unexpected check result: {result:?}");
        assert!(result.stdout.is_empty());
        assert!(result.stderr.is_empty());
    }
}

#[cfg(all(test, not(any(target_os = "linux", target_os = "macos"))))]
mod unsupported_platform_tests {
    use super::*;

    #[test]
    fn strict_check_fails_closed_without_supported_isolation() {
        let temp = tempfile::tempdir().unwrap();
        let check = QuarantineCheckCommand {
            name: "unsupported platform".to_string(),
            program: "git".to_string(),
            args: Vec::new(),
        };

        let error = strict_check_command(temp.path(), &temp.path().join("state"), &check)
            .expect_err("unsupported platforms must not execute checks unsandboxed");
        assert_eq!(error, "strict_filesystem_network_isolation_unavailable");
    }
}
