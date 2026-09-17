use std::collections::hash_map::DefaultHasher;
use std::env;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum FilesystemIsolationMode {
    Off,
    #[default]
    WorkspaceOnly,
    AllowList,
}

impl FilesystemIsolationMode {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::WorkspaceOnly => "workspace-only",
            Self::AllowList => "allow-list",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct SandboxConfig {
    pub enabled: Option<bool>,
    pub namespace_restrictions: Option<bool>,
    pub network_isolation: Option<bool>,
    pub filesystem_mode: Option<FilesystemIsolationMode>,
    pub allowed_mounts: Vec<String>,
    pub macos_seatbelt: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct SandboxRequest {
    pub enabled: bool,
    pub namespace_restrictions: bool,
    pub network_isolation: bool,
    pub filesystem_mode: FilesystemIsolationMode,
    pub allowed_mounts: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ContainerEnvironment {
    pub in_container: bool,
    pub markers: Vec<String>,
}

#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct SandboxStatus {
    pub enabled: bool,
    pub requested: SandboxRequest,
    pub supported: bool,
    pub active: bool,
    pub namespace_supported: bool,
    pub namespace_active: bool,
    pub network_supported: bool,
    pub network_active: bool,
    pub filesystem_mode: FilesystemIsolationMode,
    /// Compatibility field: true only when writes outside the configured roots
    /// are actively blocked, not for scratch HOME/TMPDIR redirection alone.
    pub filesystem_active: bool,
    #[serde(default)]
    pub home_tmp_redirected: bool,
    #[serde(default)]
    pub filesystem_write_blocking_active: bool,
    #[serde(default)]
    pub macos_seatbelt_opt_in: bool,
    pub allowed_mounts: Vec<String>,
    pub in_container: bool,
    pub container_markers: Vec<String>,
    pub fallback_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxDetectionInputs<'a> {
    pub env_pairs: Vec<(String, String)>,
    pub dockerenv_exists: bool,
    pub containerenv_exists: bool,
    pub proc_1_cgroup: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinuxSandboxCommand {
    pub program: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

impl SandboxConfig {
    #[must_use]
    pub fn resolve_request(
        &self,
        enabled_override: Option<bool>,
        namespace_override: Option<bool>,
        network_override: Option<bool>,
        filesystem_mode_override: Option<FilesystemIsolationMode>,
        allowed_mounts_override: Option<Vec<String>>,
    ) -> SandboxRequest {
        let default_enabled = default_sandbox_enabled();
        let enabled = enabled_override.unwrap_or(self.enabled.unwrap_or(default_enabled));
        SandboxRequest {
            enabled,
            namespace_restrictions: namespace_override
                .unwrap_or(self.namespace_restrictions.unwrap_or(enabled)),
            network_isolation: network_override.unwrap_or(self.network_isolation.unwrap_or(false)),
            filesystem_mode: filesystem_mode_override
                .or(self.filesystem_mode)
                .unwrap_or_default(),
            allowed_mounts: allowed_mounts_override.unwrap_or_else(|| self.allowed_mounts.clone()),
        }
    }
}

fn default_sandbox_enabled() -> bool {
    cfg!(target_os = "linux") && unshare_user_namespace_works()
}

#[must_use]
pub fn detect_container_environment() -> ContainerEnvironment {
    let proc_1_cgroup = fs::read_to_string("/proc/1/cgroup").ok();
    detect_container_environment_from(SandboxDetectionInputs {
        env_pairs: env::vars().collect(),
        dockerenv_exists: Path::new("/.dockerenv").exists(),
        containerenv_exists: Path::new("/run/.containerenv").exists(),
        proc_1_cgroup: proc_1_cgroup.as_deref(),
    })
}

#[must_use]
pub fn detect_container_environment_from(
    inputs: SandboxDetectionInputs<'_>,
) -> ContainerEnvironment {
    let mut markers = Vec::new();
    if inputs.dockerenv_exists {
        markers.push("/.dockerenv".to_string());
    }
    if inputs.containerenv_exists {
        markers.push("/run/.containerenv".to_string());
    }
    for (key, value) in inputs.env_pairs {
        let normalized = key.to_ascii_lowercase();
        if matches!(
            normalized.as_str(),
            "container" | "docker" | "podman" | "kubernetes_service_host"
        ) && !value.is_empty()
        {
            markers.push(format!("env:{key}={value}"));
        }
    }
    if let Some(cgroup) = inputs.proc_1_cgroup {
        for needle in ["docker", "containerd", "kubepods", "podman", "libpod"] {
            if cgroup.contains(needle) {
                markers.push(format!("/proc/1/cgroup:{needle}"));
            }
        }
    }
    markers.sort();
    markers.dedup();
    ContainerEnvironment {
        in_container: !markers.is_empty(),
        markers,
    }
}

#[must_use]
pub fn resolve_sandbox_status(config: &SandboxConfig, cwd: &Path) -> SandboxStatus {
    let request = config.resolve_request(None, None, None, None, None);
    resolve_sandbox_status_for_request_with_macos_seatbelt(
        &request,
        cwd,
        config.macos_seatbelt.unwrap_or(false),
    )
}

#[must_use]
pub fn resolve_sandbox_status_for_request(request: &SandboxRequest, cwd: &Path) -> SandboxStatus {
    resolve_sandbox_status_for_request_with_macos_seatbelt(request, cwd, false)
}

/// Resolve a request while applying the macOS Seatbelt setting in addition to
/// the legacy environment opt-in. Other platforms ignore the setting.
#[must_use]
pub fn resolve_sandbox_status_for_request_with_macos_seatbelt(
    request: &SandboxRequest,
    cwd: &Path,
    macos_seatbelt_setting: bool,
) -> SandboxStatus {
    let container = detect_container_environment();
    let platform = current_platform_tag();
    let namespace_supported = platform == "linux" && unshare_user_namespace_works();
    let seatbelt_opt_in = resolve_macos_seatbelt_opt_in(
        macos_seatbelt_setting,
        env::var_os("ZO_MACOS_SEATBELT").as_deref(),
    );
    resolve_sandbox_status_for_platform(
        request,
        cwd,
        container,
        platform,
        namespace_supported,
        seatbelt_opt_in,
        command_exists("sandbox-exec"),
    )
}

#[allow(clippy::too_many_arguments)]
fn resolve_sandbox_status_for_platform(
    request: &SandboxRequest,
    cwd: &Path,
    container: ContainerEnvironment,
    platform: &str,
    namespace_supported: bool,
    seatbelt_opt_in: bool,
    seatbelt_available: bool,
) -> SandboxStatus {
    let network_supported = namespace_supported;
    let filesystem_requested =
        request.enabled && request.filesystem_mode != FilesystemIsolationMode::Off;
    let home_tmp_redirected = filesystem_requested && platform == "macos";
    let filesystem_write_blocking_active =
        filesystem_requested && platform == "macos" && seatbelt_opt_in && seatbelt_available;
    let mut fallback_reasons = Vec::new();

    if request.enabled && request.namespace_restrictions && !namespace_supported {
        fallback_reasons
            .push("namespace isolation unavailable (requires Linux with `unshare`)".to_string());
    }
    if request.enabled && request.network_isolation && !network_supported {
        fallback_reasons
            .push("network isolation unavailable (requires Linux with `unshare`)".to_string());
    }
    if request.enabled
        && request.filesystem_mode == FilesystemIsolationMode::AllowList
        && request.allowed_mounts.is_empty()
    {
        fallback_reasons
            .push("filesystem allow-list requested without configured mounts".to_string());
    }

    let namespace_active =
        request.enabled && request.namespace_restrictions && namespace_supported;
    let network_active = request.enabled && request.network_isolation && network_supported;
    let active = namespace_active || network_active || filesystem_write_blocking_active;

    let allowed_mounts = normalize_mounts(&request.allowed_mounts, cwd);

    SandboxStatus {
        enabled: request.enabled,
        requested: request.clone(),
        supported: namespace_supported
            || (platform == "macos" && seatbelt_opt_in && seatbelt_available),
        active,
        namespace_supported,
        namespace_active,
        network_supported,
        network_active,
        filesystem_mode: request.filesystem_mode,
        filesystem_active: filesystem_write_blocking_active,
        home_tmp_redirected,
        filesystem_write_blocking_active,
        macos_seatbelt_opt_in: seatbelt_opt_in,
        allowed_mounts,
        in_container: container.in_container,
        container_markers: container.markers,
        fallback_reason: (!fallback_reasons.is_empty()).then(|| fallback_reasons.join("; ")),
    }
}

#[must_use]
pub fn build_linux_sandbox_command(
    command: &str,
    cwd: &Path,
    status: &SandboxStatus,
) -> Option<LinuxSandboxCommand> {
    if !cfg!(target_os = "linux")
        || !status.enabled
        || (!status.namespace_active && !status.network_active)
    {
        return None;
    }

    let mut args = vec![
        "--user".to_string(),
        "--map-root-user".to_string(),
        "--mount".to_string(),
        "--ipc".to_string(),
        "--pid".to_string(),
        "--uts".to_string(),
        "--fork".to_string(),
    ];
    if status.network_active {
        args.push("--net".to_string());
    }
    args.push("sh".to_string());
    args.push("-lc".to_string());
    args.push(command.to_string());

    let (sandbox_home, sandbox_tmp) = sandbox_scratch_dirs(cwd);
    // Only HOME/TMPDIR/PATH are exported. The former `ZO_SANDBOX_FILESYSTEM_MODE`
    // / `ZO_SANDBOX_ALLOWED_MOUNTS` exports had zero readers anywhere — they
    // suggested an enforcement layer the `unshare` wrapper does not implement, so
    // they are dropped rather than left as misleading dead signals.
    let mut env = vec![
        ("HOME".to_string(), sandbox_home.display().to_string()),
        ("TMPDIR".to_string(), sandbox_tmp.display().to_string()),
    ];
    if let Ok(path) = env::var("PATH") {
        env.push(("PATH".to_string(), path));
    }

    Some(LinuxSandboxCommand {
        program: "unshare".to_string(),
        args,
        env,
    })
}

// ---------------------------------------------------------------------------
// Cross-platform sandbox backends (WI-E)
// ---------------------------------------------------------------------------
//
// The Linux `unshare` wrapper above is one platform strategy. `SandboxBackend`
// abstracts "wrap a command so it runs isolated", reusing the existing
// [`LinuxSandboxCommand`] envelope as the return type (no new type). Selection
// is `cfg`-based. macOS uses Seatbelt (`sandbox-exec`), gated behind an explicit
// opt-in (`ZO_MACOS_SEATBELT`) so the default interactive path is unchanged;
// Windows has no native isolation wired and is fail-closed.

/// A platform isolation strategy. `wrap_command` returns the launcher envelope
/// (program + args + env) that runs `command` isolated, or `None` when this
/// backend does not isolate the request (not its platform, sandbox not enabled,
/// or — macOS — not opted in).
pub trait SandboxBackend: Send + Sync {
    /// Stable platform tag (`"linux"` / `"macos"` / `"windows"`).
    fn platform(&self) -> &'static str;
    /// Whether real isolation is achievable on this host right now.
    fn is_available(&self) -> bool;
    /// Wrap `command` for isolated execution, or `None` to run it unwrapped.
    fn wrap_command(
        &self,
        command: &str,
        cwd: &Path,
        status: &SandboxStatus,
    ) -> Option<LinuxSandboxCommand>;
}

#[allow(dead_code)] // one backend is selected per target; the others are cross-platform defs
struct LinuxSandboxBackend;
impl SandboxBackend for LinuxSandboxBackend {
    fn platform(&self) -> &'static str {
        "linux"
    }
    fn is_available(&self) -> bool {
        cfg!(target_os = "linux") && unshare_user_namespace_works()
    }
    fn wrap_command(
        &self,
        command: &str,
        cwd: &Path,
        status: &SandboxStatus,
    ) -> Option<LinuxSandboxCommand> {
        build_linux_sandbox_command(command, cwd, status)
    }
}

#[allow(dead_code)]
struct MacosSandboxBackend;
impl SandboxBackend for MacosSandboxBackend {
    fn platform(&self) -> &'static str {
        "macos"
    }
    fn is_available(&self) -> bool {
        cfg!(target_os = "macos") && command_exists("sandbox-exec")
    }
    fn wrap_command(
        &self,
        command: &str,
        cwd: &Path,
        status: &SandboxStatus,
    ) -> Option<LinuxSandboxCommand> {
        build_macos_sandbox_command(command, cwd, status)
    }
}

#[allow(dead_code)]
struct WindowsSandboxBackend;
impl SandboxBackend for WindowsSandboxBackend {
    fn platform(&self) -> &'static str {
        "windows"
    }
    fn is_available(&self) -> bool {
        false
    }
    fn wrap_command(
        &self,
        _command: &str,
        _cwd: &Path,
        _status: &SandboxStatus,
    ) -> Option<LinuxSandboxCommand> {
        None
    }
}

/// The sandbox backend for the build target. Zero-sized, so a `'static`
/// reference costs nothing.
#[must_use]
pub fn sandbox_backend() -> &'static dyn SandboxBackend {
    #[cfg(target_os = "linux")]
    {
        static BACKEND: LinuxSandboxBackend = LinuxSandboxBackend;
        &BACKEND
    }
    #[cfg(target_os = "macos")]
    {
        static BACKEND: MacosSandboxBackend = MacosSandboxBackend;
        &BACKEND
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        static BACKEND: WindowsSandboxBackend = WindowsSandboxBackend;
        &BACKEND
    }
}

/// Wrap a shell command through the platform backend. The single entry point
/// bash and typed actions share, so they isolate identically.
#[must_use]
pub fn wrap_sandbox_command(
    command: &str,
    cwd: &Path,
    status: &SandboxStatus,
) -> Option<LinuxSandboxCommand> {
    sandbox_backend().wrap_command(command, cwd, status)
}

/// Resolve the macOS Seatbelt opt-in from either settings or the legacy
/// environment route. An absent setting and absent variable keep the default
/// off; the environment variable remains presence-based for compatibility.
fn resolve_macos_seatbelt_opt_in(
    setting: bool,
    env_value: Option<&std::ffi::OsStr>,
) -> bool {
    setting || env_value.is_some()
}

/// Build the macOS `sandbox-exec` launcher for a shell command, or `None` when
/// Seatbelt is not applicable (not macOS, sandbox not filesystem-active, or not
/// opted in).
fn build_macos_sandbox_command(
    command: &str,
    cwd: &Path,
    status: &SandboxStatus,
) -> Option<LinuxSandboxCommand> {
    if !cfg!(target_os = "macos")
        || !status.enabled
        || !status.filesystem_write_blocking_active
    {
        return None;
    }
    let (sandbox_home, sandbox_tmp) = sandbox_scratch_dirs(cwd);
    let mut env = vec![
        ("HOME".to_string(), sandbox_home.display().to_string()),
        ("TMPDIR".to_string(), sandbox_tmp.display().to_string()),
    ];
    if let Ok(path) = env::var("PATH") {
        env.push(("PATH".to_string(), path));
    }
    Some(LinuxSandboxCommand {
        program: "sandbox-exec".to_string(),
        args: vec![
            "-p".to_string(),
            build_seatbelt_profile(cwd, status),
            "sh".to_string(),
            "-lc".to_string(),
            command.to_string(),
        ],
        env,
    })
}

/// Build a Seatbelt (SBPL) profile that allows everything by default but denies
/// filesystem writes outside the workspace and the sandbox scratch dirs, and
/// denies network when network isolation was requested. Pure so the policy is
/// unit-testable without spawning `sandbox-exec`.
///
/// Order matters in SBPL — last match wins — so the broad `(allow default)` is
/// narrowed by `(deny file-write*)` and then re-opened only for the workspace,
/// scratch, and write-safe device nodes.
#[must_use]
pub fn build_seatbelt_profile(cwd: &Path, status: &SandboxStatus) -> String {
    use std::fmt::Write as _;
    let (home, tmp) = sandbox_scratch_dirs(cwd);
    let mut profile = String::from("(version 1)\n(allow default)\n(deny file-write*)\n");
    profile.push_str("(allow file-write*\n");
    // The workspace + scratch dirs are always writable; the AllowList mode's
    // configured mounts must be too, otherwise opting into allow-list isolation
    // silently produces a profile identical to workspace-only (the mounts were
    // ignored). `allowed_mounts` is pre-normalized to absolute paths in
    // `resolve_sandbox_status_for_request`.
    let writable = [cwd.to_path_buf(), home, tmp]
        .into_iter()
        .chain(status.allowed_mounts.iter().map(PathBuf::from));
    for path in writable {
        // Seatbelt matches the *real* path, so resolve symlinks (on macOS
        // `/tmp` is a symlink to `/private/tmp`); fall back to the literal path
        // when it does not exist yet (scratch dirs are created just-in-time).
        let resolved = fs::canonicalize(&path).unwrap_or(path);
        // Writing to a String is infallible, so the Result is intentionally dropped.
        let _ = writeln!(
            profile,
            "  (subpath {})",
            sbpl_quote(&resolved.display().to_string())
        );
    }
    profile.push_str(
        "  (literal \"/dev/null\")\n  (literal \"/dev/stdout\")\n  (literal \"/dev/stderr\")\n  (regex #\"^/dev/tty\"))\n",
    );
    if status.network_active {
        profile.push_str("(deny network*)\n");
    }
    profile
}

/// Quote a path as an SBPL string literal: wrap in double quotes and escape
/// backslashes and quotes. Sufficient for filesystem paths.
fn sbpl_quote(path: &str) -> String {
    let escaped = path.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

/// Stable tag for the build target, used by the fail-closed policy.
fn current_platform_tag() -> &'static str {
    if cfg!(target_os = "linux") {
        "linux"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else {
        "windows"
    }
}

/// Whether a *requested* sandbox cannot be honored, so the command must be
/// refused rather than run unprotected (WI-E). Pure over its inputs so the
/// per-platform policy is unit-testable on any host.
///
/// - **Linux**: a present-but-broken `unshare` (a `fallback_reason`) is a real
///   misconfiguration → fatal.
/// - **macOS**: fatal only when Seatbelt was explicitly opted into but
///   `sandbox-exec` is missing; the default path degrades to scratch isolation
///   (failing closed there would reject every command — the "frozen retry loop"
///   regression).
/// - **Windows / other**: no native isolation wired; always degrade.
#[must_use]
// Each bool is a distinct, independently-named capability signal; folding them
// into a struct would only add indirection to a flat policy predicate.
#[allow(clippy::fn_params_excessive_bools)]
pub fn sandbox_unavailability_reason(
    platform: &str,
    enabled: bool,
    filesystem_active: bool,
    fallback_reason: Option<&str>,
    seatbelt_opt_in: bool,
    seatbelt_available: bool,
) -> Option<String> {
    if !enabled {
        return None;
    }
    match platform {
        "linux" => fallback_reason.map(str::to_string),
        "macos" if seatbelt_opt_in && filesystem_active && !seatbelt_available => Some(
            "macOS Seatbelt was requested by sandbox.macosSeatbelt or ZO_MACOS_SEATBELT, but `sandbox-exec` is unavailable"
                .to_string(),
        ),
        _ => None,
    }
}

/// Apply [`sandbox_unavailability_reason`] to the current host and status.
#[must_use]
pub fn current_sandbox_unavailability(status: &SandboxStatus) -> Option<String> {
    sandbox_unavailability_reason(
        current_platform_tag(),
        status.enabled,
        status.requested.filesystem_mode != FilesystemIsolationMode::Off,
        status.fallback_reason.as_deref(),
        status.macos_seatbelt_opt_in,
        command_exists("sandbox-exec"),
    )
}

/// Wrap a typed-action argv (program + args, no shell) for isolated execution,
/// returning the replacement `(program, args)`. Typed actions thereby inherit
/// the same sandbox as bash (WI-E): on Linux the `unshare` launcher is prepended
/// to the argv; on macOS (Seatbelt opt-in) `sandbox-exec -p <profile>` is. When
/// no backend wraps (the common case), the argv is returned unchanged.
#[must_use]
pub fn sandbox_wrap_argv(
    program: &str,
    args: &[String],
    cwd: &Path,
    status: &SandboxStatus,
) -> (String, Vec<String>, Vec<(String, String)>) {
    // Build the launcher for a no-op shell command, then strip its trailing
    // `sh -lc <cmd>` and exec the real program directly — a typed action stays
    // shell-free while inheriting the same isolation flags/profile as bash.
    let template = match sandbox_backend().platform() {
        "linux" => build_linux_sandbox_command("true", cwd, status),
        "macos" => build_macos_sandbox_command("true", cwd, status),
        _ => None,
    };
    let Some(launcher) = template else {
        return (program.to_string(), args.to_vec(), Vec::new());
    };
    let mut wrapped: Vec<String> = launcher
        .args
        .into_iter()
        .take_while(|arg| arg != "sh")
        .collect();
    wrapped.push(program.to_string());
    wrapped.extend(args.iter().cloned());
    (launcher.program, wrapped, launcher.env)
}

/// Resolve the `HOME` / `TMPDIR` scratch directories used to isolate
/// sandboxed shell commands from the user's real environment.
///
/// These are deliberately placed **outside** the working tree so that a
/// non-interactive agent run never pollutes the target repository with
/// `.sandbox-home/` / `.sandbox-tmp/` (or, worse, real cache content written
/// by tools into `$HOME`/`$TMPDIR`). The default base is the OS temp dir,
/// keyed by a stable hash of the workspace path so each repo gets its own
/// isolated — but reusable — scratch space.
///
/// Set `ZO_SANDBOX_DIR` to an explicit artifact root to override the base
/// (e.g. a tmpfs mount, or a per-job directory in CI).
#[must_use]
pub fn sandbox_scratch_dirs(cwd: &Path) -> (PathBuf, PathBuf) {
    let base = sandbox_scratch_base(cwd);
    (base.join("home"), base.join("tmp"))
}

fn sandbox_scratch_base(cwd: &Path) -> PathBuf {
    if let Some(dir) = env::var_os("ZO_SANDBOX_DIR") {
        return PathBuf::from(dir).join(workspace_scratch_key(cwd));
    }
    env::temp_dir()
        .join("zo-sandbox")
        .join(workspace_scratch_key(cwd))
}

/// A short, stable, filesystem-safe key derived from the workspace path so
/// concurrent runs in different repos never share scratch state.
#[must_use]
pub fn workspace_scratch_key(cwd: &Path) -> String {
    let mut hasher = DefaultHasher::new();
    cwd.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn normalize_mounts(mounts: &[String], cwd: &Path) -> Vec<String> {
    let cwd = cwd.to_path_buf();
    mounts
        .iter()
        .map(|mount| {
            let path = PathBuf::from(mount);
            if path.is_absolute() {
                path
            } else {
                cwd.join(path)
            }
        })
        .map(|path| path.display().to_string())
        .collect()
}

fn command_exists(command: &str) -> bool {
    env::var_os("PATH")
        .is_some_and(|paths| env::split_paths(&paths).any(|path| path.join(command).exists()))
}

/// Check whether `unshare --user` actually works on this system.
/// On some CI environments (e.g. GitHub Actions), the binary exists but
/// user namespaces are restricted, causing silent failures.
fn unshare_user_namespace_works() -> bool {
    use std::sync::OnceLock;
    static RESULT: OnceLock<bool> = OnceLock::new();
    *RESULT.get_or_init(|| {
        if !command_exists("unshare") {
            return false;
        }
        std::process::Command::new("unshare")
            .args(["--user", "--map-root-user", "true"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    })
}

#[cfg(test)]
mod tests {
    use super::{
        build_linux_sandbox_command, detect_container_environment_from, FilesystemIsolationMode,
        SandboxConfig, SandboxDetectionInputs,
    };
    use std::path::Path;

    #[test]
    fn detects_container_markers_from_multiple_sources() {
        let detected = detect_container_environment_from(SandboxDetectionInputs {
            env_pairs: vec![("container".to_string(), "docker".to_string())],
            dockerenv_exists: true,
            containerenv_exists: false,
            proc_1_cgroup: Some("12:memory:/docker/abc"),
        });

        assert!(detected.in_container);
        assert!(detected
            .markers
            .iter()
            .any(|marker| marker == "/.dockerenv"));
        assert!(detected
            .markers
            .iter()
            .any(|marker| marker == "env:container=docker"));
        assert!(detected
            .markers
            .iter()
            .any(|marker| marker == "/proc/1/cgroup:docker"));
    }

    #[test]
    fn resolves_request_with_overrides() {
        let config = SandboxConfig {
            enabled: Some(true),
            namespace_restrictions: Some(true),
            network_isolation: Some(false),
            filesystem_mode: Some(FilesystemIsolationMode::WorkspaceOnly),
            allowed_mounts: vec!["logs".to_string()],
            macos_seatbelt: Some(true),
        };

        let request = config.resolve_request(
            Some(true),
            Some(false),
            Some(true),
            Some(FilesystemIsolationMode::AllowList),
            Some(vec!["tmp".to_string()]),
        );

        assert!(request.enabled);
        assert!(!request.namespace_restrictions);
        assert!(request.network_isolation);
        assert_eq!(request.filesystem_mode, FilesystemIsolationMode::AllowList);
        assert_eq!(request.allowed_mounts, vec!["tmp"]);
    }

    #[test]
    fn default_request_uses_supported_sandbox_only() {
        let request = SandboxConfig::default().resolve_request(None, None, None, None, None);
        let default_enabled = super::default_sandbox_enabled();

        assert_eq!(request.enabled, default_enabled);
        assert_eq!(request.namespace_restrictions, default_enabled);
    }

    #[test]
    fn explicit_sandbox_enable_still_requests_namespace_isolation() {
        let config = SandboxConfig {
            enabled: Some(true),
            ..SandboxConfig::default()
        };

        let request = config.resolve_request(None, None, None, None, None);

        assert!(request.enabled);
        assert!(request.namespace_restrictions);
    }

    #[test]
    fn explicit_sandbox_enable_still_fails_closed_when_unsupported() {
        let config = SandboxConfig {
            enabled: Some(true),
            ..SandboxConfig::default()
        };
        let request = config.resolve_request(None, None, None, None, None);
        let status = super::resolve_sandbox_status_for_request(&request, Path::new("/workspace"));

        if status.namespace_supported {
            assert!(status.active);
            assert!(status.fallback_reason.is_none());
        } else {
            assert!(status.fallback_reason.is_some());
            assert!(
                status
                    .fallback_reason
                    .as_deref()
                    .unwrap_or_default()
                    .contains("namespace isolation unavailable"),
                "fallback should explain unsupported namespace isolation, got: {:?}",
                status.fallback_reason
            );
        }
    }

    #[test]
    fn scratch_dirs_live_outside_the_workspace() {
        let workspace = Path::new("/some/benchmark/repo");
        let (home, tmp) = super::sandbox_scratch_dirs(workspace);
        assert!(
            !home.starts_with(workspace),
            "sandbox HOME ({home:?}) must not live inside the workspace"
        );
        assert!(
            !tmp.starts_with(workspace),
            "sandbox TMPDIR ({tmp:?}) must not live inside the workspace"
        );
        assert!(home.ends_with("home"));
        assert!(tmp.ends_with("tmp"));
    }

    #[test]
    fn scratch_dirs_are_stable_and_repo_distinct() {
        let a1 = super::sandbox_scratch_dirs(Path::new("/repo/a"));
        let a2 = super::sandbox_scratch_dirs(Path::new("/repo/a"));
        let b = super::sandbox_scratch_dirs(Path::new("/repo/b"));
        assert_eq!(a1, a2, "same workspace must map to the same scratch dirs");
        assert_ne!(a1, b, "different workspaces must not share scratch dirs");
    }

    #[test]
    fn builds_linux_launcher_with_network_flag_when_requested() {
        let config = SandboxConfig::default();
        let status = super::resolve_sandbox_status_for_request(
            &config.resolve_request(
                Some(true),
                Some(true),
                Some(true),
                Some(FilesystemIsolationMode::WorkspaceOnly),
                None,
            ),
            Path::new("/workspace"),
        );

        if let Some(launcher) =
            build_linux_sandbox_command("printf hi", Path::new("/workspace"), &status)
        {
            assert_eq!(launcher.program, "unshare");
            assert!(launcher.args.iter().any(|arg| arg == "--mount"));
            assert!(launcher.args.iter().any(|arg| arg == "--net") == status.network_active);
            // The sandbox HOME must not point back into the workspace, else a
            // tool writing to `$HOME` would pollute the target repo.
            let home = launcher
                .env
                .iter()
                .find(|(key, _)| key == "HOME")
                .map(|(_, value)| value.clone())
                .expect("launcher sets HOME");
            assert!(
                !home.starts_with("/workspace"),
                "sandbox HOME ({home}) must live outside the workspace"
            );
        }
    }

    fn enabled_filesystem_request() -> super::SandboxRequest {
        super::SandboxRequest {
            enabled: true,
            namespace_restrictions: false,
            network_isolation: false,
            filesystem_mode: FilesystemIsolationMode::WorkspaceOnly,
            allowed_mounts: Vec::new(),
        }
    }

    fn resolve_for_test_platform(
        platform: &str,
        seatbelt_opt_in: bool,
    ) -> super::SandboxStatus {
        super::resolve_sandbox_status_for_platform(
            &enabled_filesystem_request(),
            Path::new("/workspace"),
            super::ContainerEnvironment::default(),
            platform,
            platform == "linux",
            seatbelt_opt_in,
            true,
        )
    }

    #[test]
    fn macos_enabled_without_opt_in_reports_redirection_without_write_blocking() {
        let status = resolve_for_test_platform("macos", false);

        assert!(status.home_tmp_redirected);
        assert!(!status.filesystem_write_blocking_active);
        assert!(!status.filesystem_active);
        assert!(!status.macos_seatbelt_opt_in);
    }

    #[test]
    fn macos_enabled_with_setting_reports_write_blocking() {
        let opted_in = super::resolve_macos_seatbelt_opt_in(true, None);
        let status = resolve_for_test_platform("macos", opted_in);
        let profile = super::build_seatbelt_profile(Path::new("/workspace"), &status);

        assert!(status.home_tmp_redirected);
        assert!(status.filesystem_write_blocking_active);
        assert!(status.filesystem_active);
        assert!(profile.contains("(deny file-write*)"));
    }

    #[test]
    fn macos_enabled_with_env_opt_in_reports_write_blocking() {
        let opted_in = super::resolve_macos_seatbelt_opt_in(
            false,
            Some(std::ffi::OsStr::new("1")),
        );
        let status = resolve_for_test_platform("macos", opted_in);

        assert!(status.macos_seatbelt_opt_in);
        assert!(status.filesystem_write_blocking_active);
    }

    #[test]
    fn linux_does_not_report_filesystem_redirection_or_write_blocking() {
        let status = resolve_for_test_platform("linux", false);

        assert!(!status.home_tmp_redirected);
        assert!(!status.filesystem_write_blocking_active);
        assert!(!status.filesystem_active);
    }
}

#[cfg(test)]
mod wi_e_backend_tests {
    use super::{
        build_seatbelt_profile, sandbox_backend, sandbox_unavailability_reason, sandbox_wrap_argv,
        wrap_sandbox_command, SandboxStatus,
    };

    #[test]
    fn sandbox_backend_selected_per_platform() {
        let platform = sandbox_backend().platform();
        #[cfg(target_os = "linux")]
        assert_eq!(platform, "linux");
        #[cfg(target_os = "macos")]
        assert_eq!(platform, "macos");
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        assert_eq!(platform, "windows");
    }

    #[test]
    fn seatbelt_profile_grants_allowed_mounts() {
        // AllowList mode's configured mounts must become writable subpaths in the
        // profile; previously they were ignored, making allow-list identical to
        // workspace-only. `build_seatbelt_profile` is pure, so this is hermetic.
        let status = SandboxStatus {
            enabled: true,
            filesystem_active: true,
            allowed_mounts: vec!["/opt/zo-data".to_string()],
            ..SandboxStatus::default()
        };
        let profile = build_seatbelt_profile(std::path::Path::new("/ws"), &status);
        assert!(
            profile.contains("/opt/zo-data"),
            "allow-listed mount must appear as a writable subpath:\n{profile}"
        );
    }

    #[test]
    fn unsupported_sandbox_fails_closed() {
        // Linux with a present-but-broken `unshare` → fatal (real misconfig).
        assert_eq!(
            sandbox_unavailability_reason(
                "linux",
                true,
                true,
                Some("unshare broken"),
                false,
                false
            ),
            Some("unshare broken".to_string())
        );
        // macOS Seatbelt opted in but `sandbox-exec` missing → fatal.
        assert!(
            sandbox_unavailability_reason("macos", true, true, None, true, false).is_some(),
            "opted-in Seatbelt without sandbox-exec must fail closed"
        );
        // macOS default (no opt-in) → degrade, never fatal (avoids the freeze bug).
        assert_eq!(
            sandbox_unavailability_reason("macos", true, true, None, false, true),
            None
        );
        // Windows / other → no native isolation, always degrade.
        assert_eq!(
            sandbox_unavailability_reason("windows", true, true, None, true, false),
            None
        );
        // Sandbox not requested → never fatal anywhere.
        assert_eq!(
            sandbox_unavailability_reason("linux", false, true, Some("x"), false, false),
            None
        );
    }

    #[test]
    fn typed_action_inherits_sandbox() {
        let cwd = std::env::temp_dir();
        let status = SandboxStatus {
            enabled: true,
            filesystem_active: true,
            ..SandboxStatus::default()
        };
        // A typed action is wrapped exactly when — and with the same launcher
        // program as — a bash command, but stays shell-free (no `sh -lc`).
        let bash_wrap = wrap_sandbox_command("cargo --version", &cwd, &status);
        let (program, args, _env) =
            sandbox_wrap_argv("cargo", &["--version".to_string()], &cwd, &status);
        if let Some(launcher) = bash_wrap {
            assert_eq!(program, launcher.program, "same sandbox launcher as bash");
            assert!(args.contains(&"cargo".to_string()));
            assert!(args.contains(&"--version".to_string()));
            assert!(
                !args.contains(&"sh".to_string()),
                "typed action stays shell-free"
            );
        } else {
            // No backend wraps here (default macOS/Windows, or Linux w/o unshare).
            assert_eq!(program, "cargo");
            assert_eq!(args, vec!["--version".to_string()]);
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_seatbelt_blocks_outside_write() {
        use std::path::Path;
        use std::process::Command;

        let base = std::env::temp_dir().join(format!("zo-seatbelt-{}", std::process::id()));
        let workspace = base.join("ws");
        let outside = base.join("outside");
        std::fs::create_dir_all(&workspace).expect("mk ws");
        std::fs::create_dir_all(&outside).expect("mk outside");

        let status = SandboxStatus {
            enabled: true,
            filesystem_active: true,
            ..SandboxStatus::default()
        };
        let profile = build_seatbelt_profile(&workspace, &status);

        let write_under = |dir: &Path| -> bool {
            Command::new("sandbox-exec")
                .arg("-p")
                .arg(&profile)
                .arg("sh")
                .arg("-c")
                .arg(format!("echo hi > {}/probe.txt", dir.display()))
                .status()
                .map(|status| status.success())
                .unwrap_or(false)
        };

        assert!(
            write_under(&workspace),
            "write inside the workspace is allowed"
        );
        assert!(
            !write_under(&outside),
            "write outside the workspace is denied by SBPL"
        );
        assert!(workspace.join("probe.txt").exists());
        assert!(!outside.join("probe.txt").exists());
        let _ = std::fs::remove_dir_all(&base);
    }
}
