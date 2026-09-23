//! Process accounting for the status-bar resource manager.
//!
//! The boundary is intentionally narrow: callers provide application-owned
//! PTY roots, this module reads the host process table, and the result contains
//! only those roots plus this application's own process subtree. It is not a
//! general process browser and never accepts an arbitrary pid from the webview.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::io::{self, Read};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

const PROCESS_OUTPUT_MAX: usize = 10 * 1024 * 1024;
const PROCESS_STDERR_MAX: usize = 1024 * 1024;
const PROCESS_ROWS_MAX: usize = 65_536;
const PROCESS_LINE_MAX: usize = 4096;
const PROCESS_SAMPLE_TIMEOUT: Duration = Duration::from_secs(8);
const PROCESS_IDENTITY_TIMEOUT: Duration = Duration::from_secs(2);
const PROCESS_POLL_INTERVAL: Duration = Duration::from_millis(10);
const HISTORY_CAPACITY: usize = 60;
const HISTORY_KEY_CAPACITY: usize = 2048;
const HISTORY_STALE_MS: i64 = 10 * 60 * 1000;
const CPU_MIN_SAMPLE_MS: f64 = 250.0;
const CPU_STALE_AFTER_MS: f64 = 10_000.0;
const HUNDRED_NS_TICKS_PER_MS: f64 = 10_000.0;
const APP_HISTORY_KEY: &str = "app";
const WORKTREE_HISTORY_PREFIX: &str = "worktree:";
const UNATTRIBUTED_KEY: &str = "__unattributed__";
static PROCESS_SAMPLE_GATE: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Renderer knowledge used only to group an already-owned session.
#[derive(Debug, Clone, Deserialize)]
pub struct ResourceSeat {
    pub session: String,
    #[serde(default)]
    pub worktree: Option<String>,
}

/// One process root proven by native PTY ownership.
#[derive(Debug, Clone)]
pub struct ResourceRoot {
    pub session: String,
    pub pid: Option<u32>,
    pub worktree: Option<String>,
}

/// One process root supplied by a native subsystem rather than the renderer.
///
/// `started` is the host's process-start identity. A durable owner may retain
/// a pid across an application restart, but the pid alone is never authority:
/// it may have been recycled for an unrelated process while the app was down.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct NativeProcessRoot {
    pub key: String,
    pub label: String,
    pub pid: u32,
    pub started: String,
}

/// One native-owned process shown separately from PTY sessions.
#[derive(Debug, Clone, Serialize)]
pub struct ResourceNativeProcess {
    pub key: String,
    pub label: String,
    pub pid: u32,
    pub cpu: f64,
    pub memory: u64,
}

/// One session's process-subtree metrics.
#[derive(Debug, Clone, Serialize)]
pub struct ResourceSession {
    pub session: String,
    pub pid: u32,
    pub cpu: Option<f64>,
    pub memory: Option<u64>,
}

/// All application-owned sessions grouped under one workspace identity.
#[derive(Debug, Clone, Serialize)]
pub struct ResourceWorktree {
    pub worktree: String,
    pub cpu: Option<f64>,
    pub memory: Option<u64>,
    pub history: Vec<u64>,
    pub sessions: Vec<ResourceSession>,
}

#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct ResourceMetric {
    pub cpu: f64,
    pub memory: u64,
}

/// The native application process and the non-session children beneath it.
#[derive(Debug, Clone, Serialize)]
pub struct ResourceApp {
    pub cpu: f64,
    pub memory: u64,
    pub main: ResourceMetric,
    pub other: ResourceMetric,
    pub history: Vec<u64>,
}

/// One bounded resource-manager sample.
#[derive(Debug, Clone, Serialize)]
pub struct ResourceSnapshot {
    pub app: ResourceApp,
    pub native_processes: Vec<ResourceNativeProcess>,
    pub worktrees: Vec<ResourceWorktree>,
    pub process_memory_metric: &'static str,
    pub total_cpu: f64,
    pub total_memory: u64,
    pub collected_at: i64,
}

#[derive(Debug, Clone)]
struct CpuCounter {
    ticks: u128,
    started: String,
}

#[derive(Debug, Clone)]
struct ProcessRow {
    pid: u32,
    ppid: u32,
    cpu: f64,
    memory: u64,
}

#[derive(Debug)]
struct CapturedStream {
    bytes: Vec<u8>,
    overflowed: bool,
}

#[derive(Debug)]
struct SamplerRun {
    stdout: Vec<u8>,
    pid: u32,
}

/// A process-table read before stateful CPU deltas and history are applied.
#[derive(Debug, Clone)]
pub struct ProcessSample {
    rows: Vec<ProcessRow>,
    counters: HashMap<u32, CpuCounter>,
    identities: HashMap<u32, String>,
    commands: HashMap<u32, String>,
    uids: HashMap<u32, u32>,
    process_groups: HashMap<u32, u32>,
    sampled_at: Instant,
    sampler_pid: Option<u32>,
}

/// One command an agent is running under its pane (t-6428): a process the
/// agent started in a process group of its own. The line is the table's,
/// bounded and unmasked — what of it may be said is the caller's to decide.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AgentCommand {
    pub(crate) pid: u32,
    pub(crate) command: String,
}

/// One process-table row whose real uid and native-authored argv have both
/// been checked by the sampler. This stays crate-private: it is process
/// authority for native cleanup, never a renderer-facing process browser.
#[derive(Debug, Clone)]
pub(crate) struct OwnedProcessMatch {
    pub(crate) pid: u32,
    pub(crate) parent_pid: u32,
    pub(crate) process_group: u32,
    pub(crate) started: String,
    pub(crate) command: String,
}

impl ProcessSample {
    fn available(&self) -> bool {
        !self.rows.is_empty()
    }

    /// Whether this sample still describes the exact process a native owner
    /// recorded, rather than a later process that reused its pid.
    pub(crate) fn matches_start(&self, pid: u32, started: &str) -> bool {
        !started.is_empty()
            && self
                .identities
                .get(&pid)
                .is_some_and(|seen| seen == started)
    }

    /// Recover a launch that crossed the crash window between `spawn` and the
    /// pid/start-identity journal update. The caller supplies unguessable,
    /// native-authored command arguments; renderer input never reaches here.
    pub(crate) fn processes_with_args(&self, required: &[&str]) -> Vec<NativeProcessRoot> {
        self.commands
            .iter()
            .filter(|(_, command)| command_has_args(command, required))
            .filter_map(|(pid, _)| {
                self.identities.get(pid).map(|started| NativeProcessRoot {
                    key: String::new(),
                    label: String::new(),
                    pid: *pid,
                    started: started.clone(),
                })
            })
            .collect()
    }

    /// Return only same-uid processes carrying every exact argv word. The
    /// caller still has to validate its subsystem-specific unguessable marker
    /// before this becomes termination authority.
    pub(crate) fn owned_processes_with_args(
        &self,
        uid: u32,
        required: &[&str],
    ) -> Vec<OwnedProcessMatch> {
        self.commands
            .iter()
            .filter(|(pid, command)| {
                self.uids.get(pid) == Some(&uid) && command_has_args(command, required)
            })
            .filter_map(|(pid, command)| {
                let row = self.rows.iter().find(|row| row.pid == *pid)?;
                Some(OwnedProcessMatch {
                    pid: *pid,
                    parent_pid: row.ppid,
                    process_group: self.process_groups.get(pid).copied().unwrap_or_default(),
                    started: self.identities.get(pid)?.clone(),
                    command: command.clone(),
                })
            })
            .collect()
    }

    pub(crate) fn contains_pid(&self, pid: u32) -> bool {
        self.rows.iter().any(|row| row.pid == pid)
    }

    /// The commands an agent is running under the pane rooted at `root`
    /// (t-6428): what a restart would cut besides the agent's own turn.
    ///
    /// Claude Code, Codex and zo each start a tool's command as the leader
    /// of a process group of its own, so an interrupt can signal the whole
    /// command, and keep their helpers — `caffeinate`, a language server, an
    /// MCP server, Codex's native binary under its Node launcher — in the
    /// agent's own group (measured on this machine's panes, 2026-09-24). So
    /// a command is a child of the agent's body outside the agent's group.
    /// The body is the root and every process in its group whose program is
    /// one of `body` — the catalog's names for the agent. A helper's own
    /// children are not the agent's commands: the browser an MCP server
    /// starts in a group of its own belongs to the server, and a pane that
    /// merely has one open is not running anything a restart would cut.
    ///
    /// `None` when this table cannot say: the root is not in it, or the
    /// platform's table carries no process groups (Windows).
    pub(crate) fn agent_commands(&self, root: u32, body: &[&str]) -> Option<Vec<AgentCommand>> {
        let group = *self.process_groups.get(&root)?;
        if !self.contains_pid(root) {
            return None;
        }
        let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
        for row in &self.rows {
            if row.pid != row.ppid {
                children.entry(row.ppid).or_default().push(row.pid);
            }
        }
        let mut found = Vec::new();
        let mut seen = HashSet::new();
        let mut walking = vec![root];
        while let Some(parent) = walking.pop() {
            if !seen.insert(parent) {
                continue;
            }
            for &child in children.get(&parent).into_iter().flatten() {
                let command = self.commands.get(&child).cloned().unwrap_or_default();
                if self.process_groups.get(&child) != Some(&group) {
                    found.push(AgentCommand {
                        pid: child,
                        command,
                    });
                } else if program_is_one_of(&command, body) {
                    walking.push(child);
                }
            }
        }
        found.sort_by_key(|one| one.pid);
        Some(found)
    }

    /// A sample read from `ps`-shaped text — the listing `enumerate_unix`
    /// parses — so a test can hold a process tree with its groups and argv.
    #[cfg(test)]
    pub(crate) fn from_ps_listing(listing: &str) -> ProcessSample {
        let parsed = parse_ps_output(listing);
        ProcessSample {
            rows: parsed.rows,
            counters: parsed.counters,
            identities: parsed.identities,
            commands: parsed.commands,
            uids: parsed.uids,
            process_groups: parsed.process_groups,
            sampled_at: Instant::now(),
            sampler_pid: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn fixture(processes: &[(u32, u32, f64, u64, &str, &str)]) -> ProcessSample {
        ProcessSample {
            rows: processes
                .iter()
                .map(|(pid, ppid, cpu, memory, _, _)| ProcessRow {
                    pid: *pid,
                    ppid: *ppid,
                    cpu: *cpu,
                    memory: *memory,
                })
                .collect(),
            counters: HashMap::new(),
            identities: processes
                .iter()
                .map(|(pid, _, _, _, started, _)| (*pid, (*started).to_string()))
                .collect(),
            commands: processes
                .iter()
                .map(|(pid, _, _, _, _, command)| (*pid, (*command).to_string()))
                .collect(),
            uids: HashMap::new(),
            process_groups: HashMap::new(),
            sampled_at: Instant::now(),
            sampler_pid: None,
        }
    }
}

#[derive(Debug, Clone)]
struct PriorCpuSample {
    counters: HashMap<u32, CpuCounter>,
    sampled_at: Instant,
}

#[derive(Debug, Clone)]
struct HistoryRing {
    samples: VecDeque<u64>,
    touched_at: i64,
}

/// State that intentionally survives the two-second samples and nothing else.
#[derive(Debug, Default)]
pub struct ResourceUsageState {
    history: HashMap<String, HistoryRing>,
    prior_cpu: Option<PriorCpuSample>,
}

#[derive(Default)]
struct PendingWorktree {
    cpu: f64,
    memory: u64,
    measured: bool,
    sessions: Vec<ResourceSession>,
}

struct ProcessIndex {
    by_pid: HashMap<u32, ProcessRow>,
    children: HashMap<u32, Vec<u32>>,
    identities: HashMap<u32, String>,
    available: bool,
}

fn worktree_history_key(worktree: &str) -> String {
    format!("{WORKTREE_HISTORY_PREFIX}{worktree}")
}

impl ProcessIndex {
    fn new(sample: ProcessSample) -> Self {
        let available = sample.available();
        let mut by_pid = HashMap::new();
        let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
        for row in sample.rows {
            children.entry(row.ppid).or_default().push(row.pid);
            by_pid.insert(row.pid, row);
        }
        Self {
            by_pid,
            children,
            identities: sample.identities,
            available,
        }
    }

    fn matches_start(&self, root: &NativeProcessRoot) -> bool {
        !root.started.is_empty()
            && self
                .identities
                .get(&root.pid)
                .is_some_and(|started| started == &root.started)
    }

    fn subtree(&self, root: u32) -> Vec<u32> {
        let mut found = Vec::new();
        let mut seen = HashSet::new();
        let mut pending = vec![root];
        while let Some(pid) = pending.pop() {
            if !seen.insert(pid) {
                continue;
            }
            if self.by_pid.contains_key(&pid) {
                found.push(pid);
            }
            if let Some(children) = self.children.get(&pid) {
                pending.extend(children.iter().copied());
            }
        }
        found
    }
}

/// Whether the program a command line runs is named one of `names` — the
/// first word, by its file name, so a path and a bare name read the same.
fn program_is_one_of(command: &str, names: &[&str]) -> bool {
    let program = command.split_ascii_whitespace().next().unwrap_or_default();
    let name = program.rsplit(['/', '\\']).next().unwrap_or(program);
    !name.is_empty() && names.contains(&name)
}

fn command_has_args(command: &str, required: &[&str]) -> bool {
    required.iter().all(|required| {
        command
            .split_ascii_whitespace()
            .map(|word| word.trim_matches(['\'', '"']))
            .any(|word| word == *required)
    })
}

/// Validate renderer grouping hints without treating them as process authority.
pub fn seat_map(seats: Vec<ResourceSeat>) -> Result<HashMap<String, Option<String>>, String> {
    if seats.len() > 512 {
        return Err("한 번에 분류할 수 있는 리소스 세션이 너무 많습니다".to_string());
    }
    let mut map = HashMap::new();
    for seat in seats {
        let session = seat.session.trim();
        if session.is_empty()
            || session.len() > 128
            || !(session.starts_with("term:") || session.starts_with("lane:"))
        {
            continue;
        }
        let worktree = seat.worktree.and_then(|path| {
            let path = path.trim();
            (!path.is_empty() && path.len() <= 4096).then(|| path.to_string())
        });
        map.entry(session.to_string()).or_insert(worktree);
    }
    Ok(map)
}

/// Read the host process table. The expensive platform command is isolated so
/// the Tauri command can run it on a blocking worker.
pub fn enumerate_processes() -> Result<ProcessSample, String> {
    let _sample_guard = PROCESS_SAMPLE_GATE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    #[cfg(windows)]
    {
        enumerate_windows()
    }
    #[cfg(not(windows))]
    {
        enumerate_unix()
    }
}

fn capture_stream(mut reader: impl Read, limit: usize) -> io::Result<CapturedStream> {
    let mut bytes = Vec::with_capacity(limit.min(64 * 1024));
    let mut overflowed = false;
    let mut chunk = [0u8; 8192];
    loop {
        let read = reader.read(&mut chunk)?;
        if read == 0 {
            break;
        }
        let room = limit.saturating_sub(bytes.len());
        let kept = room.min(read);
        bytes.extend_from_slice(&chunk[..kept]);
        overflowed |= kept < read;
    }
    Ok(CapturedStream { bytes, overflowed })
}

fn run_sampler(mut command: Command, timeout: Duration) -> Result<SamplerRun, String> {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|error| format!("프로세스 목록을 시작할 수 없습니다: {error}"))?;
    let pid = child.id();
    let Some(stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Err("프로세스 목록 출력을 읽을 수 없습니다".to_string());
    };
    let Some(stderr) = child.stderr.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Err("프로세스 목록 오류 출력을 읽을 수 없습니다".to_string());
    };
    let stdout_reader = std::thread::spawn(move || capture_stream(stdout, PROCESS_OUTPUT_MAX));
    let stderr_reader = std::thread::spawn(move || capture_stream(stderr, PROCESS_STDERR_MAX));
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() < timeout => {
                std::thread::sleep(PROCESS_POLL_INTERVAL);
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err("프로세스 목록 읽기 시간이 초과되었습니다".to_string());
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err(format!("프로세스 목록 상태를 읽을 수 없습니다: {error}"));
            }
        }
    };
    let stdout = stdout_reader
        .join()
        .map_err(|_| "프로세스 목록 출력 읽기가 중단되었습니다".to_string())?
        .map_err(|error| format!("프로세스 목록 출력을 읽을 수 없습니다: {error}"))?;
    let stderr = stderr_reader
        .join()
        .map_err(|_| "프로세스 목록 오류 읽기가 중단되었습니다".to_string())?
        .map_err(|error| format!("프로세스 목록 오류를 읽을 수 없습니다: {error}"))?;
    if stdout.overflowed || stderr.overflowed {
        return Err("프로세스 목록이 너무 큽니다".to_string());
    }
    if !status.success() {
        return Err("프로세스 목록을 읽을 수 없습니다".to_string());
    }
    Ok(SamplerRun {
        stdout: stdout.bytes,
        pid,
    })
}

#[cfg(not(windows))]
fn enumerate_unix() -> Result<ProcessSample, String> {
    let mut command = crate::proc::quiet_command("ps");
    command
        .args([
            "-ww",
            "-eo",
            "uid=,pid=,ppid=,pgid=,pcpu=,rss=,lstart=,command=",
        ])
        .env("LC_ALL", "C")
        .env("LANG", "C");
    let output = run_sampler(command, PROCESS_SAMPLE_TIMEOUT)?;
    let parsed = parse_ps_output(&String::from_utf8_lossy(&output.stdout));
    Ok(ProcessSample {
        rows: parsed.rows,
        counters: parsed.counters,
        identities: parsed.identities,
        commands: parsed.commands,
        uids: parsed.uids,
        process_groups: parsed.process_groups,
        sampled_at: Instant::now(),
        sampler_pid: Some(output.pid),
    })
}

#[cfg(windows)]
fn enumerate_windows() -> Result<ProcessSample, String> {
    let script = "$ErrorActionPreference='Stop';$ProgressPreference='SilentlyContinue';\
Get-CimInstance Win32_Process -OperationTimeoutSec 5 -Property ProcessId,ParentProcessId,WorkingSetSize,KernelModeTime,UserModeTime,CreationDate,CommandLine | \
ForEach-Object { try { [string]::Join([char]9,@($_.ProcessId,$_.ParentProcessId,$_.WorkingSetSize,[string]$_.KernelModeTime,[string]$_.UserModeTime,$_.CreationDate.ToUniversalTime().Ticks,$_.CommandLine)) } catch {} }";
    let mut command = crate::proc::quiet_command("powershell.exe");
    command.args([
        "-NoLogo",
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        script,
    ]);
    let output = run_sampler(command, PROCESS_SAMPLE_TIMEOUT)?;
    let parsed = parse_windows_output(&String::from_utf8_lossy(&output.stdout));
    Ok(ProcessSample {
        rows: parsed.rows,
        counters: parsed.counters,
        identities: parsed.identities,
        commands: parsed.commands,
        uids: parsed.uids,
        process_groups: parsed.process_groups,
        sampled_at: Instant::now(),
        sampler_pid: Some(output.pid),
    })
}

#[cfg(any(not(windows), test))]
fn parse_ps_output(output: &str) -> ParsedProcesses {
    let mut rows = Vec::new();
    let mut identities = HashMap::new();
    let mut commands = HashMap::new();
    let mut uids = HashMap::new();
    let mut process_groups = HashMap::new();
    for line in output.lines().take(PROCESS_ROWS_MAX) {
        let mut fields = line.split_ascii_whitespace();
        let (Ok(uid), Ok(pid), Ok(ppid), Ok(process_group), Ok(cpu), Ok(rss_kib)) = (
            fields.next().unwrap_or_default().parse::<u32>(),
            fields.next().unwrap_or_default().parse::<u32>(),
            fields.next().unwrap_or_default().parse::<u32>(),
            fields.next().unwrap_or_default().parse::<u32>(),
            fields.next().unwrap_or_default().parse::<f64>(),
            fields.next().unwrap_or_default().parse::<u64>(),
        ) else {
            continue;
        };
        if pid == 0 || !cpu.is_finite() {
            continue;
        }
        rows.push(ProcessRow {
            pid,
            ppid,
            cpu: cpu.max(0.0),
            memory: rss_kib.saturating_mul(1024),
        });
        uids.insert(pid, uid);
        process_groups.insert(pid, process_group);
        // `lstart` is five whitespace-delimited fields under LC_ALL=C.
        let started = (0..5).filter_map(|_| fields.next()).collect::<Vec<_>>();
        if started.len() == 5 {
            identities.insert(pid, started.join(" "));
            let command = bounded_command(fields);
            if !command.is_empty() {
                commands.insert(pid, command);
            }
        }
    }
    ParsedProcesses {
        rows,
        counters: HashMap::new(),
        identities,
        commands,
        uids,
        process_groups,
    }
}

fn bounded_command<'a>(words: impl Iterator<Item = &'a str>) -> String {
    let mut command = String::new();
    for word in words {
        let needed = word.len() + usize::from(!command.is_empty());
        if command.len().saturating_add(needed) > PROCESS_LINE_MAX {
            break;
        }
        if !command.is_empty() {
            command.push(' ');
        }
        command.push_str(word);
    }
    command
}

/// One process listing, read out.
///
/// Named rather than handed back as a many-place tuple: most fields are maps
/// keyed by pid, several share the same type, and a call site that swaps two
/// of them would otherwise destructure without a word of complaint.
struct ParsedProcesses {
    rows: Vec<ProcessRow>,
    counters: HashMap<u32, CpuCounter>,
    identities: HashMap<u32, String>,
    commands: HashMap<u32, String>,
    uids: HashMap<u32, u32>,
    process_groups: HashMap<u32, u32>,
}

#[cfg(any(windows, test))]
fn parse_windows_output(output: &str) -> ParsedProcesses {
    let mut rows = Vec::new();
    let mut counters = HashMap::new();
    let mut identities = HashMap::new();
    let mut commands = HashMap::new();
    let uids = HashMap::new();
    let process_groups = HashMap::new();
    for line in output.lines().take(PROCESS_ROWS_MAX) {
        let fields = line.splitn(7, '\t').map(str::trim).collect::<Vec<_>>();
        if fields.len() < 3 {
            continue;
        }
        let (Ok(pid), Ok(ppid)) = (fields[0].parse::<u32>(), fields[1].parse::<u32>()) else {
            continue;
        };
        if pid == 0 {
            continue;
        }
        rows.push(ProcessRow {
            pid,
            ppid,
            cpu: 0.0,
            memory: fields[2].parse::<u64>().unwrap_or_default(),
        });
        let kernel = fields.get(3).and_then(|value| value.parse::<u128>().ok());
        let user = fields.get(4).and_then(|value| value.parse::<u128>().ok());
        let started = fields.get(5).copied().unwrap_or_default();
        if !started.is_empty() {
            identities.insert(pid, started.to_string());
        }
        if let Some(command) = fields.get(6).filter(|command| !command.is_empty()) {
            commands.insert(pid, bounded_command(command.split_ascii_whitespace()));
        }
        if let (Some(kernel), Some(user), true) = (kernel, user, !started.is_empty()) {
            counters.insert(
                pid,
                CpuCounter {
                    ticks: kernel.saturating_add(user),
                    started: started.to_string(),
                },
            );
        }
    }
    ParsedProcesses {
        rows,
        counters,
        identities,
        commands,
        uids,
        process_groups,
    }
}

/// Read the process-start identity immediately after a native subsystem
/// spawns a durable child. The durable record is not committed without this
/// second half of the identity.
pub(crate) fn process_start_identity(pid: u32) -> Result<String, String> {
    if pid == 0 {
        return Err("프로세스 id가 없습니다".to_string());
    }
    #[cfg(not(windows))]
    let command = {
        let mut command = crate::proc::quiet_command("ps");
        command
            .args(["-p", &pid.to_string(), "-o", "lstart="])
            .env("LC_ALL", "C")
            .env("LANG", "C");
        command
    };
    #[cfg(windows)]
    let command = {
        let mut command = crate::proc::quiet_command("powershell.exe");
        command.args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            &format!(
                "$p=Get-CimInstance Win32_Process -Filter \"ProcessId = {pid}\" -OperationTimeoutSec 1; if($p){{$p.CreationDate.ToUniversalTime().Ticks}}"
            ),
        ]);
        command
    };
    let output = run_sampler(command, PROCESS_IDENTITY_TIMEOUT)?;
    output
        .stdout
        .split(|byte| *byte == b'\n' || *byte == b'\r')
        .filter_map(|line| std::str::from_utf8(line).ok())
        .map(|line| line.split_ascii_whitespace().collect::<Vec<_>>().join(" "))
        .find(|line| !line.is_empty() && line.len() <= 128)
        .ok_or_else(|| "프로세스 시작 식별자를 읽을 수 없습니다".to_string())
}

/// Check native-authored launch arguments on one known pid without exposing a
/// general process query to the renderer.
pub(crate) fn process_has_args(pid: u32, required: &[&str]) -> Result<bool, String> {
    if pid == 0 {
        return Ok(false);
    }
    #[cfg(not(windows))]
    let command = {
        let mut command = crate::proc::quiet_command("ps");
        command.args(["-ww", "-p", &pid.to_string(), "-o", "command="]);
        command
    };
    #[cfg(windows)]
    let command = {
        let mut command = crate::proc::quiet_command("powershell.exe");
        command.args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            &format!(
                "$p=Get-CimInstance Win32_Process -Filter \"ProcessId = {pid}\" -OperationTimeoutSec 1; if($p){{$p.CommandLine}}"
            ),
        ]);
        command
    };
    let output = run_sampler(command, PROCESS_IDENTITY_TIMEOUT)?;
    let command = bounded_command(String::from_utf8_lossy(&output.stdout).split_ascii_whitespace());
    Ok(!command.is_empty() && command_has_args(&command, required))
}

/// Stop a durable native child only while its pid still has the recorded
/// start identity. This is the adopted-process fallback for platforms where
/// the original [`std::process::Child`] handle died with the application.
pub(crate) fn terminate_process(pid: u32, started: &str) -> bool {
    if process_start_identity(pid).as_deref() != Ok(started) {
        return false;
    }
    #[cfg(unix)]
    {
        let Ok(pid) = libc::pid_t::try_from(pid) else {
            return false;
        };
        // SAFETY: `pid` is positive and was checked against its kernel start
        // identity immediately above. `kill` does not dereference pointers.
        if unsafe { libc::kill(pid, libc::SIGTERM) } != 0 {
            return process_start_identity(pid as u32).as_deref() != Ok(started);
        }
        if wait_until_process_changes(pid as u32, started, Duration::from_secs(2)) {
            return true;
        }
        if process_start_identity(pid as u32).as_deref() != Ok(started) {
            return true;
        }
        // SAFETY: the identity was checked again after the TERM deadline.
        let _ = unsafe { libc::kill(pid, libc::SIGKILL) };
        wait_until_process_changes(pid as u32, started, Duration::from_secs(2))
    }
    #[cfg(windows)]
    {
        let mut command = crate::proc::quiet_command("taskkill.exe");
        command.args(["/PID", &pid.to_string(), "/T", "/F"]);
        let _ = run_sampler(command, PROCESS_IDENTITY_TIMEOUT);
        wait_until_process_changes(pid, started, Duration::from_secs(2))
    }
}

fn wait_until_process_changes(pid: u32, started: &str, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if process_start_identity(pid).as_deref() != Ok(started) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

impl ResourceUsageState {
    /// Apply CPU deltas, attribute process subtrees once, and append histories.
    #[cfg(test)]
    pub fn snapshot(
        &mut self,
        sample: ProcessSample,
        roots: Vec<ResourceRoot>,
        app_pid: u32,
    ) -> ResourceSnapshot {
        self.snapshot_with_native(sample, roots, app_pid, Vec::new())
    }

    /// The same snapshot with additional roots proven by a native subsystem.
    ///
    /// These roots are counted as application helpers, not renderer-grouped
    /// sessions. Their pid is accepted only when the process-table sample has
    /// the start identity the native owner recorded.
    pub fn snapshot_with_native(
        &mut self,
        mut sample: ProcessSample,
        mut roots: Vec<ResourceRoot>,
        app_pid: u32,
        mut native_roots: Vec<NativeProcessRoot>,
    ) -> ResourceSnapshot {
        self.apply_cpu_delta(&mut sample);
        roots.sort_by(|left, right| left.session.cmp(&right.session));
        roots.dedup_by(|left, right| left.session == right.session);
        native_roots.sort_by(|left, right| {
            (left.pid, left.started.as_str(), left.key.as_str()).cmp(&(
                right.pid,
                right.started.as_str(),
                right.key.as_str(),
            ))
        });
        native_roots.dedup_by(|left, right| {
            left.pid == right.pid && left.started == right.started && left.key == right.key
        });
        let sampler_pid = sample.sampler_pid;
        let index = ProcessIndex::new(sample);
        let mut claimed = HashSet::new();
        if let Some(pid) = sampler_pid {
            claimed.extend(index.subtree(pid));
        }
        let mut worktrees: BTreeMap<String, PendingWorktree> = BTreeMap::new();

        for root in roots {
            let measured =
                index.available && root.pid.is_some_and(|pid| index.by_pid.contains_key(&pid));
            let mut metric = ResourceMetric::default();
            if let Some(pid) = root.pid {
                for member in index.subtree(pid) {
                    if !claimed.insert(member) {
                        continue;
                    }
                    if let Some(row) = index.by_pid.get(&member) {
                        metric.cpu += row.cpu;
                        metric.memory = metric.memory.saturating_add(row.memory);
                    }
                }
            }
            let key = root
                .worktree
                .filter(|path| !path.is_empty())
                .unwrap_or_else(|| format!("{UNATTRIBUTED_KEY}:{}", root.session));
            let bucket = worktrees.entry(key).or_default();
            if measured {
                bucket.measured = true;
                bucket.cpu += metric.cpu;
                bucket.memory = bucket.memory.saturating_add(metric.memory);
            }
            bucket.sessions.push(ResourceSession {
                session: root.session,
                pid: root.pid.unwrap_or_default(),
                cpu: measured.then_some(metric.cpu),
                memory: measured.then_some(metric.memory),
            });
        }

        let mut main = ResourceMetric::default();
        let mut other = ResourceMetric::default();
        let mut native_processes = Vec::new();
        for root in native_roots {
            if !index.matches_start(&root) {
                continue;
            }
            let mut metric = ResourceMetric::default();
            for member in index.subtree(root.pid) {
                if !claimed.insert(member) {
                    continue;
                }
                if let Some(row) = index.by_pid.get(&member) {
                    metric.cpu += row.cpu;
                    metric.memory = metric.memory.saturating_add(row.memory);
                    other.cpu += row.cpu;
                    other.memory = other.memory.saturating_add(row.memory);
                }
            }
            native_processes.push(ResourceNativeProcess {
                key: root.key,
                label: root.label,
                pid: root.pid,
                cpu: metric.cpu,
                memory: metric.memory,
            });
        }
        if index.available {
            for pid in index.subtree(app_pid) {
                if claimed.contains(&pid) {
                    continue;
                }
                let Some(row) = index.by_pid.get(&pid) else {
                    continue;
                };
                let target = if pid == app_pid {
                    &mut main
                } else {
                    &mut other
                };
                target.cpu += row.cpu;
                target.memory = target.memory.saturating_add(row.memory);
            }
        }
        let app_cpu = main.cpu + other.cpu;
        let app_memory = main.memory.saturating_add(other.memory);
        let now = unix_millis();
        if index.available {
            self.push_history(APP_HISTORY_KEY, app_memory, now);
        }

        let mut session_cpu = 0.0;
        let mut session_memory = 0u64;
        let worktrees = worktrees
            .into_iter()
            .map(|(worktree, bucket)| {
                let history_key = worktree_history_key(&worktree);
                if bucket.measured {
                    self.push_history(&history_key, bucket.memory, now);
                    session_cpu += bucket.cpu;
                    session_memory = session_memory.saturating_add(bucket.memory);
                }
                ResourceWorktree {
                    history: self.read_history(&history_key),
                    worktree,
                    cpu: bucket.measured.then_some(bucket.cpu),
                    memory: bucket.measured.then_some(bucket.memory),
                    sessions: bucket.sessions,
                }
            })
            .collect();
        self.sweep_history(now);

        ResourceSnapshot {
            app: ResourceApp {
                cpu: app_cpu,
                memory: app_memory,
                main,
                other,
                history: self.read_history(APP_HISTORY_KEY),
            },
            native_processes,
            worktrees,
            process_memory_metric: if cfg!(windows) { "working-set" } else { "rss" },
            total_cpu: app_cpu + session_cpu,
            total_memory: app_memory.saturating_add(session_memory),
            collected_at: now,
        }
    }

    fn apply_cpu_delta(&mut self, sample: &mut ProcessSample) {
        if sample.counters.is_empty() {
            self.prior_cpu = None;
            return;
        }
        let next = PriorCpuSample {
            counters: sample.counters.clone(),
            sampled_at: sample.sampled_at,
        };
        let Some(prior) = self.prior_cpu.as_ref() else {
            self.prior_cpu = Some(next);
            return;
        };
        if sample.sampled_at <= prior.sampled_at {
            return;
        }
        let elapsed_ms = sample
            .sampled_at
            .duration_since(prior.sampled_at)
            .as_secs_f64()
            * 1000.0;
        if elapsed_ms < CPU_MIN_SAMPLE_MS {
            return;
        }
        if elapsed_ms > CPU_STALE_AFTER_MS {
            self.prior_cpu = Some(next);
            return;
        }
        let ceiling = std::thread::available_parallelism()
            .map(|cores| cores.get() as f64 * 100.0)
            .unwrap_or(100.0);
        for row in &mut sample.rows {
            let (Some(current), Some(previous)) =
                (sample.counters.get(&row.pid), prior.counters.get(&row.pid))
            else {
                continue;
            };
            if current.started != previous.started || current.ticks < previous.ticks {
                continue;
            }
            let cpu_ms = (current.ticks - previous.ticks) as f64 / HUNDRED_NS_TICKS_PER_MS;
            row.cpu = (cpu_ms / elapsed_ms * 100.0).clamp(0.0, ceiling);
        }
        self.prior_cpu = Some(next);
    }

    fn push_history(&mut self, key: &str, memory: u64, now: i64) {
        if !self.history.contains_key(key)
            && self.history.len() >= HISTORY_KEY_CAPACITY
            && let Some(oldest) = self
                .history
                .iter()
                .filter(|(key, _)| key.as_str() != APP_HISTORY_KEY)
                .min_by_key(|(_, ring)| ring.touched_at)
                .map(|(key, _)| key.clone())
        {
            self.history.remove(&oldest);
        }
        let ring = self
            .history
            .entry(key.to_string())
            .or_insert_with(|| HistoryRing {
                samples: VecDeque::new(),
                touched_at: now,
            });
        if now.saturating_sub(ring.touched_at) > HISTORY_STALE_MS {
            ring.samples.clear();
        }
        ring.samples.push_back(memory);
        while ring.samples.len() > HISTORY_CAPACITY {
            ring.samples.pop_front();
        }
        ring.touched_at = now;
    }

    fn read_history(&self, key: &str) -> Vec<u64> {
        self.history
            .get(key)
            .map(|ring| ring.samples.iter().copied().collect())
            .unwrap_or_default()
    }

    fn sweep_history(&mut self, now: i64) {
        self.history
            .retain(|_, ring| now.saturating_sub(ring.touched_at) <= HISTORY_STALE_MS);
    }
}

fn unix_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(rows: Vec<ProcessRow>) -> ProcessSample {
        ProcessSample {
            rows,
            counters: HashMap::new(),
            identities: HashMap::new(),
            commands: HashMap::new(),
            uids: HashMap::new(),
            process_groups: HashMap::new(),
            sampled_at: Instant::now(),
            sampler_pid: None,
        }
    }

    fn counter_sample(sampled_at: Instant, ticks: u128) -> ProcessSample {
        ProcessSample {
            rows: vec![ProcessRow {
                pid: 42,
                ppid: 0,
                cpu: 0.0,
                memory: 4096,
            }],
            counters: HashMap::from([(
                42,
                CpuCounter {
                    ticks,
                    started: "same-process".to_string(),
                },
            )]),
            identities: HashMap::from([(42, "same-process".to_string())]),
            commands: HashMap::new(),
            uids: HashMap::new(),
            process_groups: HashMap::new(),
            sampled_at,
            sampler_pid: None,
        }
    }

    #[test]
    fn ps_rows_are_bounded_typed_and_count_rss_in_bytes() {
        let parsed = parse_ps_output(
            " 501 10 1 7 12.5 2048 Thu Aug 27 13:14:15 2026 /sdk/emulator -avd Pixel\n\
             noise\n\
             501 11 10 7 -3 1 Thu Aug 27 13:14:16 2026 helper\n\
             0 0 0 0 4 8 Thu Aug 27 13:14:17 2026 kernel\n",
        );
        assert_eq!(parsed.rows.len(), 2);
        assert_eq!(parsed.rows[0].pid, 10);
        assert_eq!(parsed.rows[0].memory, 2 * 1024 * 1024);
        assert_eq!(parsed.rows[1].cpu, 0.0);
        assert_eq!(parsed.identities[&10], "Thu Aug 27 13:14:15 2026");
        assert_eq!(parsed.commands[&10], "/sdk/emulator -avd Pixel");
        assert_eq!(parsed.uids[&10], 501);
        assert_eq!(parsed.process_groups[&10], 7);
    }

    /// The three pane shapes measured on this machine (t-6428, 2026-09-24),
    /// as `ps -eo uid,pid,ppid,pgid,pcpu,rss,lstart,command` lists them:
    /// Claude Code with `caffeinate` and a language server beside a tool
    /// shell; Codex's Node launcher over its native binary, an MCP server
    /// whose browser leads a group of its own, and a tool shell; zo with an
    /// MCP server whose child leads its own group, and a tool shell.
    const THREE_PANES: &str = "\
501 100 1 100 0 1 Thu Sep 24 01:00:00 2026 /Users/dev/.local/bin/claude --resume abc
501 101 100 100 0 1 Thu Sep 24 01:00:00 2026 caffeinate -i -t 300
501 102 100 100 0 1 Thu Sep 24 01:00:00 2026 /Users/dev/.rustup/toolchains/stable/bin/rust-analyzer
501 103 102 100 0 1 Thu Sep 24 01:00:00 2026 /Users/dev/.rustup/toolchains/stable/libexec/rust-analyzer-proc-macro-srv
501 110 100 110 0 1 Thu Sep 24 01:00:00 2026 /bin/zsh -c source /tmp/snapshot.sh && eval 'cargo test -p zerocode-shell' < /dev/null
501 111 110 110 0 1 Thu Sep 24 01:00:00 2026 cargo test -p zerocode-shell
501 200 1 200 0 1 Thu Sep 24 01:00:00 2026 node /opt/homebrew/bin/codex resume 01a0
501 201 200 200 0 1 Thu Sep 24 01:00:00 2026 /opt/homebrew/lib/node_modules/@openai/codex/vendor/bin/codex resume 01a0
501 202 201 200 0 1 Thu Sep 24 01:00:00 2026 node /Users/dev/mcp/server.js
501 203 202 203 0 1 Thu Sep 24 01:00:00 2026 /Applications/Chrome.app/Contents/MacOS/Chrome --headless
501 210 201 210 0 1 Thu Sep 24 01:00:00 2026 /bin/zsh -lc just shell-test
501 300 1 300 0 1 Thu Sep 24 01:00:00 2026 /Users/dev/.local/bin/zo --resume s-1
501 301 300 300 0 1 Thu Sep 24 01:00:00 2026 npm exec chrome-devtools-mcp@latest --isolated=true
501 302 301 300 0 1 Thu Sep 24 01:00:00 2026 chrome-devtools-mcp
501 303 302 303 0 1 Thu Sep 24 01:00:00 2026 node /Users/dev/.npm/chrome-devtools-mcp/browser.js
501 310 300 310 0 1 Thu Sep 24 01:00:00 2026 sh -lc node ui/tests/window.mjs --suite update
";

    fn pids(found: Option<Vec<AgentCommand>>) -> Option<Vec<u32>> {
        found.map(|found| found.into_iter().map(|one| one.pid).collect())
    }

    #[test]
    fn an_agents_commands_are_the_groups_it_started_and_never_its_helpers() {
        let table = ProcessSample::from_ps_listing(THREE_PANES);
        // Claude: the tool shell, not caffeinate, not the language server.
        assert_eq!(
            pids(table.agent_commands(100, &["claude"])),
            Some(vec![110])
        );
        // Codex: the shell its native binary started — walked through the
        // launcher by name — and never the browser its MCP server leads.
        assert_eq!(pids(table.agent_commands(200, &["codex"])), Some(vec![210]));
        // zo: the tool shell; the MCP server's own group leader is its own.
        assert_eq!(pids(table.agent_commands(300, &["zo"])), Some(vec![310]));
        let shell = table
            .agent_commands(300, &["zo"])
            .and_then(|found| found.into_iter().next())
            .expect("zo's shell");
        assert_eq!(
            shell.command,
            "sh -lc node ui/tests/window.mjs --suite update"
        );
    }

    #[test]
    fn a_table_that_cannot_say_answers_none_rather_than_nothing_running() {
        let table = ProcessSample::from_ps_listing(THREE_PANES);
        // A root the table does not hold: the pane's process is gone or
        // was never read — not "idle".
        assert_eq!(table.agent_commands(999, &["claude"]), None);
        // A table with no process groups — Windows reads none — cannot tell
        // a command from a helper at all.
        let grouped_nowhere = ProcessSample::fixture(&[
            (100, 1, 0.0, 1, "t", "claude"),
            (110, 100, 0.0, 1, "t", "/bin/zsh -c cargo test"),
        ]);
        assert_eq!(grouped_nowhere.agent_commands(100, &["claude"]), None);
    }

    #[test]
    fn command_recovery_requires_complete_native_arguments() {
        let parsed = parse_ps_output(
            " 501 42 1 42 1 2 Thu Aug 27 13:14:15 2026 /sdk/emulator -avd Pixel -prop qemu.zerocode.managed=secret\n",
        );
        let process_sample = ProcessSample {
            rows: Vec::new(),
            counters: HashMap::new(),
            identities: parsed.identities,
            commands: parsed.commands,
            uids: parsed.uids,
            process_groups: parsed.process_groups,
            sampled_at: Instant::now(),
            sampler_pid: None,
        };
        assert_eq!(
            process_sample.processes_with_args(&["Pixel", "qemu.zerocode.managed=secret"]),
            vec![NativeProcessRoot {
                key: String::new(),
                label: String::new(),
                pid: 42,
                started: "Thu Aug 27 13:14:15 2026".to_string(),
            }]
        );
        assert!(
            process_sample
                .processes_with_args(&["Other", "qemu.zerocode.managed=secret"])
                .is_empty()
        );
    }

    #[test]
    fn native_process_samples_and_targeted_reads_share_one_start_identity() {
        let pid = std::process::id();
        let sample = enumerate_processes().expect("native process sample");
        let started = process_start_identity(pid).expect("targeted process identity");
        assert!(sample.matches_start(pid, &started));
    }

    #[test]
    fn renderer_seats_group_only_known_session_names() {
        let map = seat_map(vec![
            ResourceSeat {
                session: " term:7 ".to_string(),
                worktree: Some(" /repo/main ".to_string()),
            },
            ResourceSeat {
                session: "term:7".to_string(),
                worktree: Some("/repo/forged".to_string()),
            },
            ResourceSeat {
                session: "process:42".to_string(),
                worktree: Some("/repo/bad".to_string()),
            },
        ])
        .expect("bounded renderer hints");
        assert_eq!(map.len(), 1);
        assert_eq!(map["term:7"].as_deref(), Some("/repo/main"));
    }

    #[test]
    fn the_native_snapshot_gets_emulator_roots_from_rust_not_renderer_seats() {
        let command = include_str!("cmd/appearance.rs");
        assert!(command.contains("emulator::managed_resource_processes"));
        assert!(command.contains("snapshot_with_native(sample, roots"));
    }

    #[test]
    fn one_process_is_charged_to_only_one_owned_session() {
        let rows = vec![
            ProcessRow {
                pid: 1,
                ppid: 0,
                cpu: 1.0,
                memory: 100,
            },
            ProcessRow {
                pid: 10,
                ppid: 1,
                cpu: 2.0,
                memory: 200,
            },
            ProcessRow {
                pid: 11,
                ppid: 10,
                cpu: 3.0,
                memory: 300,
            },
        ];
        let roots = vec![
            ResourceRoot {
                session: "term:1".to_string(),
                pid: Some(10),
                worktree: Some("/repo/main".to_string()),
            },
            ResourceRoot {
                session: "term:2".to_string(),
                pid: Some(11),
                worktree: Some("/repo/main".to_string()),
            },
        ];
        let snapshot = ResourceUsageState::default().snapshot(sample(rows), roots, 1);
        assert_eq!(snapshot.worktrees[0].memory, Some(500));
        assert_eq!(snapshot.worktrees[0].sessions[0].memory, Some(500));
        assert_eq!(snapshot.worktrees[0].sessions[1].memory, Some(0));
        assert_eq!(snapshot.app.memory, 100);
        assert_eq!(snapshot.total_memory, 600);
    }

    #[test]
    fn sampler_process_and_its_children_are_not_charged_to_the_app() {
        let mut process_sample = sample(vec![
            ProcessRow {
                pid: 1,
                ppid: 0,
                cpu: 1.0,
                memory: 100,
            },
            ProcessRow {
                pid: 90,
                ppid: 1,
                cpu: 2.0,
                memory: 200,
            },
            ProcessRow {
                pid: 91,
                ppid: 90,
                cpu: 3.0,
                memory: 300,
            },
        ]);
        process_sample.sampler_pid = Some(90);
        let snapshot = ResourceUsageState::default().snapshot(process_sample, Vec::new(), 1);
        assert_eq!(snapshot.app.main.memory, 100);
        assert_eq!(snapshot.app.other.memory, 0);
        assert_eq!(snapshot.total_memory, 100);
    }

    #[test]
    fn a_native_root_survives_reparenting_but_only_with_its_start_identity() {
        let rows = vec![
            ProcessRow {
                pid: 1,
                ppid: 0,
                cpu: 1.0,
                memory: 100,
            },
            ProcessRow {
                pid: 50,
                ppid: 2,
                cpu: 200.0,
                memory: 400,
            },
            ProcessRow {
                pid: 51,
                ppid: 50,
                cpu: 25.0,
                memory: 600,
            },
        ];
        let mut process_sample = sample(rows.clone());
        process_sample
            .identities
            .insert(50, "emulator-born".to_string());
        let snapshot = ResourceUsageState::default().snapshot_with_native(
            process_sample,
            Vec::new(),
            1,
            vec![NativeProcessRoot {
                key: "android:emulator-5554".to_string(),
                label: "Android · Pixel".to_string(),
                pid: 50,
                started: "emulator-born".to_string(),
            }],
        );
        assert_eq!(snapshot.app.main.memory, 100);
        assert_eq!(snapshot.app.other.memory, 1_000);
        assert_eq!(snapshot.native_processes.len(), 1);
        assert_eq!(snapshot.native_processes[0].label, "Android · Pixel");
        assert_eq!(snapshot.native_processes[0].memory, 1_000);
        assert_eq!(snapshot.total_memory, 1_100);

        let mut reused = sample(rows);
        reused.identities.insert(50, "unrelated-reuse".to_string());
        let snapshot = ResourceUsageState::default().snapshot_with_native(
            reused,
            Vec::new(),
            1,
            vec![NativeProcessRoot {
                key: "android:emulator-5554".to_string(),
                label: "Android · Pixel".to_string(),
                pid: 50,
                started: "emulator-born".to_string(),
            }],
        );
        assert_eq!(snapshot.app.other.memory, 0);
        assert!(snapshot.native_processes.is_empty());
        assert_eq!(snapshot.total_memory, 100);
    }

    #[test]
    fn a_native_root_inside_the_app_tree_is_not_counted_twice() {
        let rows = vec![
            ProcessRow {
                pid: 1,
                ppid: 0,
                cpu: 1.0,
                memory: 100,
            },
            ProcessRow {
                pid: 50,
                ppid: 1,
                cpu: 2.0,
                memory: 400,
            },
        ];
        let mut process_sample = sample(rows);
        process_sample
            .identities
            .insert(50, "emulator-born".to_string());
        let snapshot = ResourceUsageState::default().snapshot_with_native(
            process_sample,
            Vec::new(),
            1,
            vec![NativeProcessRoot {
                key: "android:emulator-5554".to_string(),
                label: "Android · Pixel".to_string(),
                pid: 50,
                started: "emulator-born".to_string(),
            }],
        );
        assert_eq!(snapshot.app.memory, 500);
        assert_eq!(snapshot.app.other.memory, 400);
        assert_eq!(snapshot.total_memory, 500);
    }

    #[test]
    fn a_root_missing_from_the_process_table_is_unavailable_not_zero() {
        let roots = vec![ResourceRoot {
            session: "term:missing".to_string(),
            pid: Some(999),
            worktree: Some("/repo/main".to_string()),
        }];
        let snapshot = ResourceUsageState::default().snapshot(
            sample(vec![ProcessRow {
                pid: 1,
                ppid: 0,
                cpu: 1.0,
                memory: 100,
            }]),
            roots,
            1,
        );
        assert_eq!(snapshot.worktrees[0].cpu, None);
        assert_eq!(snapshot.worktrees[0].sessions[0].memory, None);
    }

    #[cfg(unix)]
    #[test]
    fn a_sampler_that_does_not_exit_is_killed_at_its_deadline() {
        let mut command = crate::proc::quiet_command("sh");
        command.args(["-c", "exec sleep 5"]);
        let started = Instant::now();
        let error =
            run_sampler(command, Duration::from_millis(50)).expect_err("the sampler must time out");
        assert!(error.contains("초과"));
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn remote_roots_stay_visible_without_inventing_local_metrics() {
        let roots = vec![ResourceRoot {
            session: "lane:remote".to_string(),
            pid: None,
            worktree: Some("/srv/project".to_string()),
        }];
        let snapshot = ResourceUsageState::default().snapshot(sample(Vec::new()), roots, 1);
        assert_eq!(snapshot.worktrees[0].cpu, None);
        assert_eq!(snapshot.worktrees[0].sessions[0].memory, None);
    }

    #[test]
    fn history_is_a_sixty_sample_ring() {
        let mut state = ResourceUsageState::default();
        for value in 0..75 {
            state.push_history("/repo", value, value as i64);
        }
        let history = state.read_history("/repo");
        assert_eq!(history.len(), HISTORY_CAPACITY);
        assert_eq!(history[0], 15);
        assert_eq!(history[59], 74);
    }

    #[test]
    fn a_stale_history_starts_again_instead_of_joining_old_samples() {
        let mut state = ResourceUsageState::default();
        state.push_history("worktree:/repo", 10, 0);
        state.push_history("worktree:/repo", 20, HISTORY_STALE_MS + 1);
        assert_eq!(state.read_history("worktree:/repo"), vec![20]);
    }

    #[test]
    fn history_keys_are_bounded_and_renderer_names_cannot_alias_the_app() {
        let mut state = ResourceUsageState::default();
        for index in 0..(HISTORY_KEY_CAPACITY + 5) {
            state.push_history(&format!("worktree:{index}"), index as u64, index as i64);
        }
        assert_eq!(state.history.len(), HISTORY_KEY_CAPACITY);

        let rows = vec![
            ProcessRow {
                pid: 1,
                ppid: 0,
                cpu: 1.0,
                memory: 100,
            },
            ProcessRow {
                pid: 10,
                ppid: 1,
                cpu: 2.0,
                memory: 200,
            },
        ];
        let roots = vec![ResourceRoot {
            session: "term:1".to_string(),
            pid: Some(10),
            worktree: Some(APP_HISTORY_KEY.to_string()),
        }];
        let snapshot = ResourceUsageState::default().snapshot(sample(rows), roots, 1);
        assert_eq!(snapshot.app.history, vec![100]);
        assert_eq!(snapshot.worktrees[0].history, vec![200]);
    }

    #[test]
    fn windows_rows_keep_cpu_counters_apart_from_memory() {
        let parsed =
            parse_windows_output("42\t1\t4096\t100\t200\t638000\temulator.exe -avd Pixel\n");
        assert_eq!(parsed.rows[0].memory, 4096);
        assert_eq!(parsed.counters[&42].ticks, 300);
        assert_eq!(parsed.counters[&42].started, "638000");
        assert_eq!(parsed.identities[&42], "638000");
        assert_eq!(parsed.commands[&42], "emulator.exe -avd Pixel");
    }

    #[test]
    fn old_and_too_close_cpu_samples_do_not_replace_a_good_baseline() {
        let mut state = ResourceUsageState::default();
        let baseline = Instant::now();
        let mut first = counter_sample(baseline, 100);
        state.apply_cpu_delta(&mut first);

        let old_at = baseline
            .checked_sub(Duration::from_secs(1))
            .expect("a recent instant has one second behind it");
        let mut old = counter_sample(old_at, 50);
        state.apply_cpu_delta(&mut old);
        assert_eq!(state.prior_cpu.as_ref().unwrap().sampled_at, baseline);

        let mut too_close = counter_sample(baseline + Duration::from_millis(100), 1_000_100);
        state.apply_cpu_delta(&mut too_close);
        assert_eq!(state.prior_cpu.as_ref().unwrap().sampled_at, baseline);

        let valid_at = baseline + Duration::from_secs(1);
        let mut valid = counter_sample(valid_at, 10_000_100);
        state.apply_cpu_delta(&mut valid);
        assert!((valid.rows[0].cpu - 100.0).abs() < f64::EPSILON);
        assert_eq!(state.prior_cpu.as_ref().unwrap().sampled_at, valid_at);
    }
}
