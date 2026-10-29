use super::*;

#[cfg(target_os = "macos")]
pub(super) fn resident_bytes() -> u64 {
    // `proc_pidinfo` fills a fixed-size struct the kernel owns the layout of.
    // The call is unsafe because it writes through a raw pointer; it is sound
    // here because the buffer is exactly the size being declared to it, and
    // the result is only read when the kernel says it wrote a full struct.
    let mut info = std::mem::MaybeUninit::<libc::proc_taskinfo>::zeroed();
    let size = std::mem::size_of::<libc::proc_taskinfo>() as libc::c_int;
    let wrote = unsafe {
        libc::proc_pidinfo(
            std::process::id() as libc::c_int,
            libc::PROC_PIDTASKINFO,
            0,
            info.as_mut_ptr().cast(),
            size,
        )
    };
    if wrote != size {
        return 0;
    }
    unsafe { info.assume_init() }.pti_resident_size
}

/// The working set — the Windows word for what Task Manager's "Memory" column
/// and Activity Monitor's resident size both mean to a person.
#[cfg(windows)]
pub(super) fn resident_bytes() -> u64 {
    use windows::Win32::System::{
        ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS},
        Threading::GetCurrentProcess,
    };
    let size = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
    let mut counters = PROCESS_MEMORY_COUNTERS {
        cb: size,
        ..PROCESS_MEMORY_COUNTERS::default()
    };
    // SAFETY: the pseudo-handle for this process needs no closing, and the
    // buffer is exactly the `size` bytes being declared to the call.
    match unsafe { GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, size) } {
        Ok(()) => counters.WorkingSetSize as u64,
        Err(_) => 0,
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
pub(super) fn resident_bytes() -> u64 {
    // Every other platform answers this differently and none is a target
    // yet. Nothing is better than a number that means something else.
    0
}

/// One listening TCP socket, as the ports popover draws it.
#[derive(serde::Serialize)]
pub(super) struct ListeningPort {
    pub(super) port: u16,
    /// What the socket is bound to, verbatim: `*`, `127.0.0.1`, `::1`.
    pub(super) bind_host: String,
    /// What a person would actually type to reach it. Orca normalises the
    /// wildcards before the renderer ever sees them, because `*:3000` is not
    /// an address anybody can open (`connectHost`).
    pub(super) connect_host: String,
    pub(super) pid: u32,
    pub(super) process: String,
    /// The workspace this socket was opened from, when one can be shown.
    /// Orca calls the rest `external` and gives them their own section.
    pub(super) owner: Option<String>,
    /// The exact checkout that owns the listener.
    ///
    /// The display name above is not an identity: two projects can both have
    /// a checkout named `main`. Automatic browser opening consumes this path
    /// so a URL printed in one checkout cannot claim a same-named listener in
    /// another.
    pub(super) owner_path: Option<String>,
}

/// What a scan found, or why it could not look.
///
/// The reason is carried rather than swallowed because Orca shows it —
/// `Port scan unavailable on {platform}: {reason}` — and a popover that says
/// "no ports" on a machine it never scanned is a lie told quietly.
#[derive(serde::Serialize)]
pub(super) struct PortScan {
    pub(super) ports: Vec<ListeningPort>,
    pub(super) unavailable: Option<String>,
    pub(super) platform: String,
}

/// Orca stops at 200 after sorting, so a machine with a thousand sockets
/// cannot make the popover the slowest thing in the window.
pub(super) const PORT_LIMIT: usize = 200;

pub(super) struct LsofRow {
    pub(super) pid: u32,
    pub(super) process: String,
    pub(super) bind_host: String,
    pub(super) port: u16,
}

/// Ask the system which TCP sockets are listening, or `None` where we cannot.
#[cfg(target_os = "macos")]
pub(super) fn scan_listeners() -> Option<String> {
    // The exact invocation Orca uses. `-F pcn` is lsof's field mode: one
    // field per line, each prefixed by its letter, which is a format meant to
    // be parsed rather than the columns meant to be read.
    run_lsof(&["-nP", "-iTCP", "-sTCP:LISTEN", "-F", "pcn"])
}

#[cfg(not(target_os = "macos"))]
pub(super) fn scan_listeners() -> Option<String> {
    // Linux reads /proc and Windows shells out to netstat; neither is a
    // target yet. Answering `None` puts the reason on screen instead of an
    // empty list that would read as "nothing is listening".
    None
}

#[cfg(target_os = "macos")]
pub(super) fn run_lsof(args: &[&str]) -> Option<String> {
    let output = crate::proc::quiet_command("lsof")
        .args(args)
        .output()
        .ok()?;
    // lsof exits non-zero when it merely had nothing to report, so the status
    // is not the test — whether it wrote anything is.
    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Where each of these processes is standing.
///
/// One batched call rather than one per pid: attribution needs the working
/// directory and asking two hundred times would cost more than the scan.
#[cfg(target_os = "macos")]
pub(super) fn process_cwds(pids: &[u32]) -> HashMap<u32, String> {
    if pids.is_empty() {
        return HashMap::new();
    }
    let mut seen: Vec<String> = pids.iter().map(u32::to_string).collect();
    seen.sort_unstable();
    seen.dedup();
    let list = seen.join(",");
    let Some(raw) = run_lsof(&["-nP", "-a", "-d", "cwd", "-F", "pn", "-p", &list]) else {
        return HashMap::new();
    };
    parse_lsof_cwds(&raw)
}

#[cfg(not(target_os = "macos"))]
pub(super) fn process_cwds(_pids: &[u32]) -> HashMap<u32, String> {
    HashMap::new()
}

/// How many processes on this machine stand anywhere inside `dir` — any
/// process, not only this window's panes (t-45336): a person's shell in another
/// terminal app, a build running there, an agent another window launched.
///
/// `None` when the table could not be read — no `lsof`, an empty answer (this
/// process always has a working directory, so an empty table is a failure and
/// not a quiet machine), or a platform this does not read. A caller deciding
/// whether a directory may be deleted keeps it on `None`.
///
/// Read on macOS (`lsof`) and Linux (`/proc`); Windows answers `None`, so a
/// kept checkout's build tree is not shed there (review t-79535 P3-7). What
/// counts is where a process STANDS: a build that runs from outside the
/// directory (`cargo --manifest-path`) is seen only through a child that
/// stands inside it, which rustc is while it compiles.
///
/// The whole table in one call, a few hundred rows: asked only of a checkout
/// that every other question has already let go, right before it would go.
#[cfg(target_os = "macos")]
pub(super) fn processes_standing_in(dir: &Path) -> Option<usize> {
    let raw = run_lsof(&["-nP", "-d", "cwd", "-F", "pn"])?;
    let cwds = parse_lsof_cwds(&raw);
    if cwds.is_empty() {
        return None;
    }
    let real = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
    Some(
        cwds.values()
            .filter(|cwd| Path::new(cwd).starts_with(&real) || Path::new(cwd).starts_with(dir))
            .count(),
    )
}

/// The same count from `/proc/<pid>/cwd`. A process this user cannot see
/// into has no readable link and is not counted; when no link at all is
/// readable the table is taken to be unread (`None`), as an empty `lsof` is.
#[cfg(target_os = "linux")]
pub(super) fn processes_standing_in(dir: &Path) -> Option<usize> {
    let real = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
    let (mut read, mut inside) = (0_usize, 0_usize);
    for entry in std::fs::read_dir("/proc").ok()?.flatten() {
        let is_process = entry
            .file_name()
            .to_str()
            .is_some_and(|name| !name.is_empty() && name.bytes().all(|byte| byte.is_ascii_digit()));
        if !is_process {
            continue;
        }
        // A process that exited since the listing has no link: not a row.
        let Ok(cwd) = std::fs::read_link(entry.path().join("cwd")) else {
            continue;
        };
        read += 1;
        if cwd.starts_with(&real) || cwd.starts_with(dir) {
            inside += 1;
        }
    }
    (read > 0).then_some(inside)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub(super) fn processes_standing_in(_dir: &Path) -> Option<usize> {
    None
}

/// Read lsof's field output into rows.
///
/// The format is stateful: `p` opens a process and `c` names it, then each
/// `f` opens one of its files and `n` gives that file's address. So a pid and
/// a command carry forward across the files beneath them, and a row only
/// exists once an `n` arrives.
pub(super) fn parse_lsof_listeners(raw: &str) -> Vec<LsofRow> {
    let mut rows = Vec::new();
    let mut pid = None;
    let mut process = String::new();
    for line in raw.lines() {
        let Some((tag, value)) = line.split_at_checked(1) else {
            continue;
        };
        match tag {
            "p" => {
                pid = value.parse::<u32>().ok();
                process.clear();
            }
            "c" => process = value.to_string(),
            "n" => {
                let (Some(pid), Some((bind_host, port))) = (pid, split_address(value)) else {
                    continue;
                };
                rows.push(LsofRow {
                    pid,
                    process: process.clone(),
                    bind_host,
                    port,
                });
            }
            _ => {}
        }
    }
    rows
}

/// The same format, for the working-directory query: `p` then the `n` under it.
#[cfg(any(target_os = "macos", test))]
pub(super) fn parse_lsof_cwds(raw: &str) -> HashMap<u32, String> {
    let mut cwds = HashMap::new();
    let mut pid = None;
    for line in raw.lines() {
        let Some((tag, value)) = line.split_at_checked(1) else {
            continue;
        };
        match tag {
            "p" => pid = value.parse::<u32>().ok(),
            "n" => {
                if let Some(pid) = pid {
                    cwds.entry(pid).or_insert_with(|| value.to_string());
                }
            }
            _ => {}
        }
    }
    cwds
}

/// Split `127.0.0.1:5178`, `*:8733` or `[::1]:8080` into host and port.
///
/// From the right, because an IPv6 address is full of colons and only the
/// last one separates the port.
pub(super) fn split_address(value: &str) -> Option<(String, u16)> {
    let (host, port) = value.rsplit_once(':')?;
    let port = port.parse::<u16>().ok()?;
    let host = host.trim_start_matches('[').trim_end_matches(']');
    Some((host.to_string(), port))
}

/// What a person can actually open. A socket bound to every interface is
/// reachable at localhost, and `*:3000` is not something anyone can type.
pub(super) fn connect_host(bind: &str) -> String {
    match bind {
        "*" | "0.0.0.0" | "::" | "" => "localhost".to_string(),
        host => host.to_string(),
    }
}

/// Which checkout a process is standing in, if any.
///
/// At-or-below, on a path boundary: a process in `/repo/sub` belongs to
/// `/repo`, but `/repo-two` must not match `/repo`. Orca calls this the `cwd`
/// confidence and has a second, weaker one that reads the command line; this
/// keeps only the one it can be sure of.
pub(super) fn owning_workspace<'a>(
    cwd: &str,
    roots: &'a [(String, String)],
) -> Option<&'a (String, String)> {
    roots
        .iter()
        .find(|(path, _)| cwd == path || cwd.starts_with(&format!("{path}/")))
}

/// The window's black box.
///
/// `eprintln!` reaches /dev/null the moment this app is launched by a watcher
/// or by launchd — which is every launch that matters — and three husk-pane
/// hunts ran blind before this file existed. Appended and stamped so two
/// windows' lines interleave legibly, rotated at a megabyte so a season of
/// quiet cannot fill a disk.
/// Terms whose frames are currently stopping at the pump's gate — kept so the
/// black box records that transition once, not sixty times a second.
pub(super) fn unwatched_noted() -> &'static std::sync::Mutex<std::collections::HashSet<TermId>> {
    static NOTED: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<TermId>>> =
        std::sync::OnceLock::new();
    NOTED.get_or_init(|| std::sync::Mutex::new(std::collections::HashSet::new()))
}

/// When each previewed term's tail last crossed — the throttle's whole memory.
///
/// Beside [`unwatched_noted`] because it has the same shape and the same
/// hazard: keyed by a term id, so it must be emptied where a term is forgotten
/// or it grows for the life of a process that runs for days.
pub(super) fn previewed_last_sent() -> &'static std::sync::Mutex<HashMap<TermId, i64>> {
    static SENT: std::sync::OnceLock<std::sync::Mutex<HashMap<TermId, i64>>> =
        std::sync::OnceLock::new();
    SENT.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

/// The window's black box, and the one rotation it keeps.
pub(crate) const WINDOW_LOG: &str = "window-errors.log";
pub(crate) const WINDOW_LOG_ROTATED: &str = "window-errors.log.1";

pub(super) fn note_window_event(local_data_root: &Path, line: &str) {
    use std::io::Write;
    let path = local_data_root.join(WINDOW_LOG);
    if std::fs::metadata(&path).is_ok_and(|meta| meta.len() > 1_000_000) {
        let _ = std::fs::rename(&path, local_data_root.join(WINDOW_LOG_ROTATED));
    }
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        // ONE write per line: `writeln!` streams its pieces, and the pump
        // thread and a command thread interleaving mid-line is exactly what
        // the first recording came back mangled by. O_APPEND makes a single
        // write atomic; it makes no promise about three.
        let _ = file.write_all(format!("{} {line}\n", now_epoch_ms()).as_bytes());
        // A panic can take the process down before the filesystem sees another
        // write. The black box is a crash recorder, so its durability boundary
        // is this call rather than a later graceful shutdown.
        let _ = file.flush();
    }
}

pub(super) fn panic_payload(payload: &(dyn std::any::Any + Send)) -> String {
    let message = payload
        .downcast_ref::<&str>()
        .map(|message| (*message).to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "<non-string payload>".to_string());
    // The headline is deliberately one line; the forced backtrace owns the
    // lines below it. A payload containing a newline must not impersonate a
    // second black-box record.
    message.replace(['\r', '\n'], " ")
}

pub(super) fn record_panic(
    local_data_root: &Path,
    payload: &(dyn std::any::Any + Send),
    location: Option<(&str, u32)>,
    thread_name: Option<&str>,
) {
    let site = location.map_or_else(
        || "<unknown>".to_string(),
        |(file, line)| format!("{file}:{line}"),
    );
    let thread = thread_name.unwrap_or("<unnamed>");
    // Formatted once: the same text is the log's backtrace and the source of
    // the report's symbol frames (t-3014 §2.4), so the two never disagree.
    let backtrace = std::backtrace::Backtrace::force_capture().to_string();
    let origin = crash::Origin {
        site: location,
        thread: thread_name,
        backtrace: Some(&backtrace),
    };
    if let Err(error) = crash::record(
        local_data_root,
        crash::Kind::Panic,
        &panic_payload(payload),
        origin,
    ) {
        note_window_event(local_data_root, &format!("crash evidence: {error}"));
    }
    note_window_event(
        local_data_root,
        &format!(
            "panic: {} @ {site} [thread {thread}]\nbacktrace:\n{backtrace}",
            panic_payload(payload)
        ),
    );
}

/// Arm the process-wide black box before application state or the event loop.
///
/// The previous hook still prints the platform's ordinary panic report. This
/// hook adds the durable copy that survives Finder/launchd redirecting stderr
/// to `/dev/null`, and forces a backtrace even when `RUST_BACKTRACE` is unset.
pub(super) fn install_window_panic_hook(local_data_root: PathBuf) {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let current = std::thread::current();
        record_panic(
            &local_data_root,
            info.payload(),
            info.location()
                .map(|location| (location.file(), location.line())),
            current.name(),
        );
        previous(info);
    }));
}

/// Which of the three the caller asked for, decided in one place.
///
/// Split out from the command so the meaning can be checked without a state
/// directory to write into: the file handling is uninteresting, the tri-state
/// is the whole contract.
// A default is a LIST because Orca's own are (`platformBindings` takes one):
// the floating terminal keeps its measured ⌘⌥A beside the ⌘` this window
// already taught people, and an action with no key defaults to the empty
// list rather than a sentinel.
/// The one default this application ships on macOS and nowhere else.
///
/// Orca's own reason, quoted from `shared/keybindings.ts:551-553`: "macOS only
/// — Windows Ctrl+Alt is AltGr and Linux Ctrl+Alt+T is the desktop 'open
/// terminal', so no safe default there." A chord that swallows what the
/// platform already owns is worse than a command with no chord: the palette
/// still reaches it and the keybinding pane still lets somebody bind their own.
///
/// The renderer says the same thing in its own spelling — `darwinOnly(…)` —
/// and `tests::the_backend_keybinding_registry_matches_the_renderer_defaults`
/// is what keeps the two from drifting apart.
#[cfg(target_os = "macos")]
pub(super) const NEW_AGENT_TAB_CHORD: &[&str] = &["mod+alt+t"];
#[cfg(not(target_os = "macos"))]
pub(super) const NEW_AGENT_TAB_CHORD: &[&str] = &[];

pub(super) const KEYBINDING_ACTIONS: &[(&str, &[&str])] = &[
    ("tree.undo", &["mod+z"]),
    ("tree.redo", &["mod+shift+z"]),
    ("lane.new", &["mod+n"]),
    ("tab.close", &["mod+w"]),
    ("tab.reopen", &["mod+shift+t"]),
    ("file.save", &["mod+s"]),
    ("file.find", &["mod+f"]),
    ("find.next", &["mod+g"]),
    ("find.prev", &[]),
    ("file.quickOpen", &["mod+p"]),
    ("worktree.jump", &["mod+j"]),
    ("terminal.toggle", &["mod+alt+a", "mod+`"]),
    ("terminal.newTab", &["mod+t"]),
    ("terminal.newAgentTab", NEW_AGENT_TAB_CHORD),
    ("worktree.create", &["mod+shift+n"]),
    ("view.toggleSidebar", &["mod+b"]),
    ("view.toggleAside", &["mod+l"]),
    ("browser.reload", &["mod+r"]),
    // Palette-only since the zoom router took the chords: a front-and-
    // uncovered browser still gets its page zoom FIRST, through that router.
    ("browser.zoomIn", &[]),
    ("browser.zoomOut", &[]),
    ("browser.zoomReset", &[]),
    // One chord, four domains — Orca's resolveZoomTarget on its menu
    // accelerators (resolve-zoom-target.ts:33-56).
    ("zoom.in", &["mod+="]),
    ("zoom.out", &["mod+-"]),
    ("zoom.reset", &["mod+0"]),
    ("activity.files", &["mod+shift+e"]),
    ("activity.search", &["mod+shift+f"]),
    ("activity.scm", &["mod+shift+g"]),
    ("project.open", &["mod+o"]),
    ("app.settings", &["mod+,"]),
    ("ask.focusDocked", &["mod+shift+h"]),
    ("terminal.splitRight", &["mod+d"]),
    ("terminal.splitDown", &["mod+shift+d"]),
    ("terminal.focusNextPane", &["mod+]"]),
    ("terminal.focusPreviousPane", &["mod+["]),
    ("terminal.expandPane", &["mod+shift+enter"]),
    ("terminal.equalizePaneSizes", &[]),
    ("terminal.setTitle", &[]),
    ("terminal.clearPaneTitle", &[]),
];

/// The identity emitted by the renderer's `chordOf`: `mod`, optional `alt`,
/// optional `shift`, then one lower-case physical key id.
pub(super) fn normalize_keybinding_chord(raw: &str) -> Result<String, String> {
    // `keyId` emits a literal space for Space. Preserve it while still
    // accepting whitespace around named keys in an edited settings file.
    let raw = raw.trim_start();
    let raw = if raw.ends_with("+ ") {
        raw
    } else {
        raw.trim_end()
    };
    let normalized = raw.to_ascii_lowercase();
    let valid = normalized.strip_prefix("mod+").is_some_and(|key| {
        let key = key.strip_prefix("alt+").unwrap_or(key);
        let key = key.strip_prefix("shift+").unwrap_or(key);
        keybinding_key_id(key)
    });
    if !valid {
        return Err(format!("{raw}는 이 창이 인식하는 단축키가 아닙니다"));
    }
    Ok(normalized)
}

/// `KeyboardEvent.key` is either one printable character or a named key
/// (Enter, ArrowLeft, F24, MediaPlayPause, ...). Validate that grammar rather
/// than maintaining another platform's list of key names. Both settings
/// writes and effective-binding reads pass through this one validator.
fn keybinding_key_id(key: &str) -> bool {
    let mut chars = key.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if chars.next().is_none() {
        return !first.is_control();
    }
    first.is_ascii_alphabetic() && key.bytes().all(|byte| byte.is_ascii_alphanumeric())
}

pub(super) fn effective_keybinding_chords(
    overrides: &BTreeMap<String, Vec<String>>,
    action_id: &str,
    default: &[&str],
) -> Vec<String> {
    let Some(stored) = overrides.get(action_id) else {
        return default.iter().map(|chord| (*chord).to_string()).collect();
    };
    let mut normalized = Vec::new();
    for chord in stored {
        if let Ok(chord) = normalize_keybinding_chord(chord)
            && !normalized.contains(&chord)
        {
            normalized.push(chord);
        }
    }
    if normalized.is_empty() && !stored.is_empty() {
        default.iter().map(|chord| (*chord).to_string()).collect()
    } else {
        normalized
    }
}

pub(super) fn digit_keybinding_index(chord: &str) -> Option<u8> {
    let digit = chord.strip_prefix("mod+")?;
    (digit.len() == 1 && ('1'..='9').contains(&digit.chars().next()?))
        .then(|| digit.parse().expect("one ASCII digit"))
}

pub(super) fn apply_keybinding(
    overrides: &mut BTreeMap<String, Vec<String>>,
    action_id: String,
    bindings: Option<Vec<String>>,
) -> Result<(), String> {
    let Some((_, default)) = KEYBINDING_ACTIONS
        .iter()
        .find(|(candidate, _)| *candidate == action_id)
    else {
        return Err(format!("{action_id}는 이 창에 있는 단축키 액션이 아닙니다"));
    };
    let replacement = match bindings {
        None => None,
        Some(chords) => {
            let mut normalized = Vec::new();
            for chord in chords {
                let chord = normalize_keybinding_chord(&chord)?;
                if !normalized.contains(&chord) {
                    normalized.push(chord);
                }
            }
            Some(normalized)
        }
    };
    let candidate = replacement.as_ref().map_or_else(
        || default.iter().map(|chord| (*chord).to_string()).collect(),
        Clone::clone,
    );
    for chord in &candidate {
        if digit_keybinding_index(chord).is_some() {
            return Err(format!(
                "{chord}는 워크트리 1–9 이동에 이미 지정되어 있습니다"
            ));
        }
        for (other_id, other_default) in KEYBINDING_ACTIONS {
            if *other_id == action_id {
                continue;
            }
            if effective_keybinding_chords(overrides, other_id, other_default).contains(chord) {
                return Err(format!(
                    "{chord}는 이미 {other_id} 액션에 지정되어 있습니다"
                ));
            }
        }
    }
    match replacement {
        // Reset: the row goes back to whatever the registry defaults to, and
        // the way to say that is to stop storing anything for it. An entry
        // that stored the default instead would pin it — a default we improve
        // later would never reach the person who once pressed `기본값`.
        None => {
            overrides.remove(&action_id);
        }
        // An empty list is not "nothing to say", it is "this action has no
        // key", and it has to outlive the restart to mean anything.
        Some(normalized) => {
            overrides.insert(action_id, normalized);
        }
    }
    Ok(())
}

/// Resolve renderer-provided workspace paths through the same catalog boundary
/// activation uses. A board mutation must not turn an arbitrary filesystem
/// path into durable application state merely because a webview named it.
pub(super) fn known_workspace_board_paths(
    config_root: &Path,
    paths: Vec<String>,
) -> Result<Vec<String>, String> {
    if paths.is_empty() {
        return Err("옮길 워크스페이스가 없습니다".to_string());
    }
    if paths.len() > 512 {
        return Err("한 번에 옮길 수 있는 워크스페이스가 너무 많습니다".to_string());
    }
    let mut known = BTreeSet::new();
    for path in paths {
        let resolved = match known_workspace_context(config_root, &path)? {
            KnownWorkspace::Git(_, worktree) => worktree.path,
            KnownWorkspace::Folder(path) => path,
        };
        known.insert(resolved.to_string_lossy().into_owned());
    }
    Ok(known.into_iter().collect())
}

pub(super) fn pane_layouts_read(file: &Path, worktree: &str) -> Vec<pane_layout::TabLayout> {
    pane_layout::read(file)
        .remove(worktree)
        .unwrap_or_default()
        .into_iter()
        .map(|mut layout| {
            // The window rebuilds the shape; the buffers replay through
            // `replay_stored_screen` on this side — fed by the spawn itself —
            // and the stored pty ids died
            // with the session that wrote them. Sending either would move
            // half a megabyte per leaf across the bridge to be ignored.
            layout.buffers = HashMap::new();
            layout.terms = HashMap::new();
            // And a pane stored under a word that only says go on comes back
            // under what its conversation was asked (t-11540).
            layout.named_by_their_turns()
        })
        .collect()
}

pub(super) fn stage_layouts_file(config_root: &Path) -> PathBuf {
    config_root.join(artifact_file::STAGE_LAYOUTS)
}

/// The panes still running as the window goes whose conversations the exit
/// writes down when no saved leaf names them ([`pane_layout::file_unheld`]),
/// each with the workspace its agent last reported from: every zo pane
/// (t-12063), and every pane whose conversation holds a run's coordinator
/// seat, of any agent (t-76489) — with that run, for the line the exit files.
///
/// zo is the one agent whose conversation the window learns from the pane's
/// own channel rather than from a tab's report, and the one a restart lost
/// in September. A coordinator is the one conversation a restart must never
/// lose: on 2026-10-26 the saved set had no line for run-4275's claude, and
/// the next window brought back the zo beside it alone. Never a pane seated
/// under another pane's team — a ledger worker comes back on the ledger's
/// road and a teammate is its leader's. `in_front` says whether the pane's
/// agent still holds its terminal, `None` when it cannot tell
/// ([`exit_agent_in_front`]).
fn live_unheld_panes(
    state: &AppState,
    in_front: &dyn Fn(TermId) -> Option<bool>,
) -> Vec<(pane_layout::LivePane, Option<String>)> {
    // Without the teammates a teammate would be filed as a tab of its own, so
    // a team table held past the patience files nothing this time (t-51380).
    let Some(seated) = seated_teammates() else {
        return Vec::new();
    };
    let agents = state.agent_terms().clone();
    let sessions = state.pane_sessions().clone();
    let statuses = state.last_statuses().clone();
    // The seats, off a ledger that answered the goodbye just before: one
    // that did not would hold this write past its patience, and the zo
    // panes are still filed without it.
    let coordinators = crate::orchestration::goodbye_answered()
        .then(crate::orchestration::seated_conversations)
        .flatten();
    unheld_panes(
        agents,
        &sessions,
        &statuses,
        &seated,
        coordinators.as_ref().map(|seated| seated as SeatOf<'_>),
        in_front,
    )
}

/// The run whose coordinator seat a conversation holds, by agent, key word
/// and id ([`crate::orchestration::seated_conversations`]).
pub(super) type SeatOf<'a> = &'a dyn Fn(&str, &str, &str) -> Option<String>;

/// [`live_unheld_panes`] off the window's maps as read: `agents` and
/// `sessions` the window's records of each pane, `seated` the teammates,
/// `coordinators` the run a conversation's seat is, and `in_front` whether
/// a pane's agent holds its terminal now. A pane whose agent was quit back
/// to its shell keeps both records yet holds nothing (t-76489): it is no
/// pane to write down, and a coordinator it once was is now another pane's.
/// A pane `in_front` cannot tell of (`None`) is written down as it ran
/// (t-76489 r5): not knowing is not knowing it holds nothing — and it is
/// filed as one not known to hold its conversation (r6,
/// [`pane_layout::LivePane::sure`]), which adds and takes nothing away.
pub(super) fn unheld_panes(
    agents: HashMap<TermId, &'static str>,
    sessions: &HashMap<TermId, zerocode_core::ProviderSession>,
    statuses: &HashMap<String, crate::last_status::LastStatus>,
    seated: &std::collections::HashSet<u32>,
    coordinators: Option<SeatOf<'_>>,
    in_front: &dyn Fn(TermId) -> Option<bool>,
) -> Vec<(pane_layout::LivePane, Option<String>)> {
    let mut live: Vec<_> = agents
        .into_iter()
        .filter(|(term, _)| !seated.contains(term))
        .filter_map(|(term, agent)| {
            let sure = match in_front(term) {
                Some(false) => return None,
                held => held == Some(true),
            };
            let session = sessions.get(&term)?;
            let key = match session.key {
                zerocode_core::SessionKey::SessionId => "session_id",
                zerocode_core::SessionKey::ConversationId => "conversation_id",
            };
            let run = coordinators.and_then(|seated| seated(agent, key, &session.id));
            if agent != AgentKind::Zo.slug() && run.is_none() {
                return None;
            }
            let worktree = pane_worktree(statuses, term, &session.id).unwrap_or_default();
            Some((
                pane_layout::LivePane {
                    term,
                    worktree,
                    wake: pane_layout::WakeAgent {
                        agent: agent.to_string(),
                        key: key.to_string(),
                        id: session.id.clone(),
                        transcript_path: session.transcript_path.clone(),
                        interrupted: false,
                    },
                    sure,
                },
                run,
            ))
        })
        .collect();
    live.sort_by_key(|(pane, _)| pane.term);
    live
}

/// Whether `term`'s agent holds its terminal as the window goes (t-76489),
/// each pane's lock waited for at most [`EXIT_LOCK_PATIENCE`] — `None` when
/// it cannot tell. A pane the window knows no agent of holds nothing.
fn exit_agent_in_front(state: &AppState, term: TermId) -> Option<bool> {
    let shell_pane = { state.shell_panes().contains(&term) };
    let Some(agent) = ({ state.agent_terms().get(&term).copied() }) else {
        return Some(false);
    };
    crate::agent_tools_runtime::agent_in_front_within(
        state.terminals(),
        shell_pane,
        term,
        agent,
        Some(EXIT_LOCK_PATIENCE),
    )
}

/// The workspace the pane `term`'s own hooks last named for the conversation
/// `session` — or none the window can place.
fn pane_worktree(
    statuses: &HashMap<String, crate::last_status::LastStatus>,
    term: TermId,
    session: &str,
) -> Option<String> {
    let seat = format!("\u{0}{term}");
    statuses
        .iter()
        .filter(|(key, row)| {
            key.ends_with(&seat) && row.session.as_ref().is_some_and(|held| held.id == session)
        })
        .max_by_key(|(_, row)| row.at)
        .map(|(_, row)| row.worktree.clone())
}

/// Every pane a team seats under its leader — the teammates the exit does not
/// file as tabs of their own — or `None` when the team table stayed held past
/// [`EXIT_LOCK_PATIENCE`] (t-51380).
pub(super) fn seated_teammates() -> Option<std::collections::HashSet<u32>> {
    let teams = agent_teams::teams_within(EXIT_LOCK_PATIENCE)?;
    Some(
        teams
            .values()
            .flat_map(|team| {
                team.panes()
                    .filter(|pane| pane.term != team.leader_term)
                    .map(|pane| pane.term)
                    .collect::<Vec<_>>()
            })
            .collect(),
    )
}

/// How long the way out waits for the screens (t-51380). The goodbye before
/// it waits at most [`crate::exit_runtime::LEDGER_PATIENCE`] (1.5 s), and the two
/// together stay under the hang watchdog's first threshold (2 s).
const EXIT_CAPTURE_PATIENCE: Duration = Duration::from_millis(300);

/// How long the capture waits for any one lock on its road — the team table, a
/// pane's terminal — before it keeps what the file already holds (t-51380).
/// The pump holds a pane for one round's drain, [`crate::DRAIN_BUDGET`] (8 ms).
const EXIT_LOCK_PATIENCE: Duration = Duration::from_millis(50);

/// The capture, run on the way out and waited for at most `patience`
/// (t-51380). `true` when it finished in time.
///
/// `capture` is told through `wanted` whether the way out still waits for it.
/// A capture that answers past the patience must not write: the panes it reads
/// are being ended by then, and a pane read on its way out is not the screen
/// it last showed.
pub(super) fn exit_capture_within(
    patience: Duration,
    capture: impl FnOnce(&dyn Fn() -> bool) + Send + 'static,
) -> bool {
    let wanted = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let asked = Arc::clone(&wanted);
    let (finished, answered) = std::sync::mpsc::sync_channel(1);
    // A thread that cannot start leaves the file as the window's own saves
    // left it: its sender goes with it, and the wait below ends at once.
    let _ = std::thread::Builder::new()
        .name(EXIT_CAPTURE_THREAD.to_string())
        .spawn(move || {
            capture(&|| asked.load(std::sync::atomic::Ordering::SeqCst));
            let _ = finished.send(());
        });
    let done = answered.recv_timeout(patience).is_ok();
    wanted.store(false, std::sync::atomic::Ordering::SeqCst);
    done
}

/// The capture's thread, by name, so a sample of a stuck exit says whose wait
/// it is.
const EXIT_CAPTURE_THREAD: &str = "zerocode-exit-screens";

/// One pane's screen for the exit's capture, or `None` for a terminal that is
/// already gone.
///
/// The pane's lock is waited for at most [`EXIT_LOCK_PATIENCE`] (t-51380). A
/// pane busy past it is still standing, and an empty screen is how
/// `pane_layout::capture_exit` keeps the one the file already holds for it.
pub(super) fn exit_screen(terminals: &TerminalRegistry, term: TermId) -> Option<String> {
    let held = terminals.handle(term)?;
    let Some(pty) = terminal_registry::lock_pty_within(&held, EXIT_LOCK_PATIENCE) else {
        return Some(String::new());
    };
    Some(zerocode_pty::serialize_tail(
        pty.terminal().grid(),
        zerocode_pty::SCROLLBACK_BUFFER_BYTE_LIMIT,
    ))
}

/// Write what every remembered leaf's screen holds, on the way out.
///
/// Orca's shutdown capture (`captureTerminalShutdownLayout`,
/// index-ftls8Hg_.js:100863): each live pane serialised under the byte cap
/// and stored beside the layout it belongs to. The pty ids the session
/// recorded are spent here and cleared — they die with this process, and a
/// number left in the file would point the NEXT session's capture at
/// somebody else's shell. Buffers already stored for panes this window
/// never opened stay: their worktrees were not visited, and their last
/// screens are still the truth about them.
///
/// The main thread runs this from the exit event and waits for it no longer
/// than [`EXIT_CAPTURE_PATIENCE`] (t-51380). Every lock on its road — the
/// window's pane maps, the team table, the file's writer, each pane's
/// terminal — can be held by a thread that is itself stuck: on 2026-10-13 the
/// team table was, and the way out stood on it for 41 s until the window was
/// killed. A capture that does not finish in time writes nothing, and the
/// file stays as the window's own saves left it.
pub(super) fn capture_scrollback_at_exit(app: &AppHandle) {
    let app = app.clone();
    let _ = exit_capture_within(EXIT_CAPTURE_PATIENCE, move |wanted| {
        let state = app.state::<AppState>();
        let file = pane_layouts_file(state.config_root());
        let in_front = |term| exit_agent_in_front(&state, term);
        let live = live_unheld_panes(&state, &in_front);
        let sessions = state.pane_sessions().clone();
        let agents = state.agent_terms().clone();
        let terminals = state.terminals();
        let exited = exit_write(
            &file,
            live.iter().map(|(pane, _)| pane.clone()).collect(),
            &|wake, term| {
                crate::cmd::settings::pane_holds(&sessions, &agents, &in_front, wake, term)
            },
            &|term| terminals.handle(term).is_some(),
            &|term| exit_screen(terminals, term),
            wanted,
        );
        exit_notices(state.local_data_root(), &live, &exited);
    });
}

/// What the exit's own write did ([`exit_write`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Exited {
    /// Each conversation taken off a leaf because another leaf names it.
    pub(crate) doubled: Vec<String>,
    /// What became of each live pane, in the order given.
    pub(crate) filed: Vec<pane_layout::Filed>,
    /// Whether the file holds any of it.
    pub(crate) written: Written,
}

/// How the exit's own write ended (t-76489). Only [`Written::Done`] put
/// anything in the file: the layouts are changed in memory first, and a
/// write cancelled or failed after that leaves the file as the window's own
/// saves left it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Written {
    /// The file holds what the exit wrote.
    Done,
    /// The way out moved on before the write was done: nothing written.
    Cancelled,
    /// The file could not be replaced, in the error's own words.
    Failed(String),
}

/// The exit's one write of the layouts file, through its one writer
/// (t-7812 D): one leaf per conversation, the one `holds` says holds it
/// now (t-76489); the zo panes and coordinators in `live` no saved leaf
/// names (t-12063), `standing` saying which shells are still open; every
/// leaf's `screen` and which tabs were standing (t-14036) — and written only
/// while `wanted` says the way out still waits.
pub(super) fn exit_write(
    file: &Path,
    live: Vec<pane_layout::LivePane>,
    holds: &dyn Fn(&pane_layout::WakeAgent, u32) -> Option<bool>,
    standing: &dyn Fn(u32) -> bool,
    screen: &dyn Fn(u32) -> Option<String>,
    wanted: &dyn Fn() -> bool,
) -> Exited {
    let mut doubled = Vec::new();
    let mut filed = Vec::new();
    let wrote = pane_layout::rewrite(file, |layouts| {
        // One leaf per conversation (t-76489): the one whose pane holds
        // it keeps it, before anything is filed against the names.
        doubled = pane_layout::one_wake_each(layouts, holds);
        // Then the zo panes and the coordinators no saved leaf names
        // (t-12063, t-76489), so their screens are captured below with
        // everyone else's.
        filed = pane_layout::file_unheld(layouts, live, standing);
        // Every leaf's screen, and which tabs were standing (t-14036).
        pane_layout::capture_exit(layouts, screen);
        wanted()
    });
    let written = match wrote {
        Ok(true) => Written::Done,
        Ok(false) => Written::Cancelled,
        Err(error) => Written::Failed(error.to_string()),
    };
    Exited {
        doubled,
        filed,
        written,
    }
}

/// The window log's lines and the runs' letters for what the exit's write
/// did (t-76489): the leaves it took a doubled conversation off, and each
/// coordinator no saved leaf named — written down, or, when the write was
/// cancelled or failed or found it no place, said so under its run's name
/// in the log and on its board.
pub(super) fn exit_notices(
    local: &Path,
    live: &[(pane_layout::LivePane, Option<String>)],
    exited: &Exited,
) {
    if exited.written == Written::Done && !exited.doubled.is_empty() {
        note_window_event(
            local,
            &format!(
                "pane layouts: the exit kept one leaf per conversation and took {} off {} other leaf(s)",
                exited.doubled.join(", "),
                exited.doubled.len()
            ),
        );
    }
    for ((pane, run), filed) in live.iter().zip(exited.filed.iter().copied()) {
        let Some(run) = run else { continue };
        if let Some(line) = coordinator_filed_line(pane, run, filed, &exited.written) {
            note_window_event(local, &line);
        }
        if let Some((body, receipt)) = coordinator_lost_letter(pane, run, filed, &exited.written) {
            crate::orchestration::post_observation_once(
                Some(run),
                &format!("run:{run}"),
                &body,
                &receipt,
                now_epoch_ms(),
            );
        }
    }
}

/// The window log's line for a run's coordinator conversation the exit
/// found no saved leaf naming (t-76489): written into its workspace as a tab
/// of its own, put into the leaf that held its shell or the one that named
/// it from a pane gone, or — named once more as the run's — placed nowhere,
/// left to a tab no pane stood for (t-77269), or lost with a write that was
/// cancelled or failed. Nothing for one a leaf already named. Only the run, the pane, the agent, the conversation's
/// first characters, the workspace and the write's error: nothing anybody
/// wrote.
pub(super) fn coordinator_filed_line(
    pane: &pane_layout::LivePane,
    run: &str,
    filed: pane_layout::Filed,
    written: &Written,
) -> Option<String> {
    let head = format!(
        "exit: coordinator of run:{run} on terminal {} ({} {})",
        pane.term,
        pane.wake.agent,
        crate::restart_nudge_runtime::session_prefix(&pane.wake.id)
    );
    if filed == pane_layout::Filed::Named {
        return None;
    }
    let unwritten = match written {
        Written::Done => None,
        Written::Cancelled => Some("the way out moved on before it was written".to_string()),
        Written::Failed(error) => Some(format!("the write failed: {error}")),
    };
    if let Some(why) = unwritten {
        return Some(format!(
            "{head} was in no saved leaf and {why} — the next window cannot bring it back on its own"
        ));
    }
    match filed {
        pane_layout::Filed::Named => None,
        pane_layout::Filed::Renamed => Some(format!(
            "{head} was in no saved leaf — the leaf holding its shell names it now"
        )),
        pane_layout::Filed::Relinked => Some(format!(
            "{head} was named only by a leaf of a pane gone — that leaf stands for its pane now"
        )),
        pane_layout::Filed::Tab(id) => Some(format!(
            "{head} was in no saved tab — written as tab {id} of {}",
            pane.worktree
        )),
        pane_layout::Filed::Unplaced => Some(format!(
            "{head} was in no saved tab and named no workspace — the next window cannot bring it back on its own"
        )),
        pane_layout::Filed::Asleep => Some(format!(
            "{head} was named only by a saved tab no pane stood for, and named no workspace — the next window brings it back only when that tab's workspace is opened"
        )),
    }
}

/// The run's own letter for a coordinator conversation the next window will
/// not bring back on its own (t-76489) — placed nowhere, left to a tab no
/// pane stood for (t-77269), or its write cancelled or failed — as its body
/// and its receipt: its coordinator card
/// says so, with the way back. The run, the agent and the conversation's id
/// — nothing anybody wrote, not the error's words either. Nothing for one
/// the file holds.
pub(super) fn coordinator_lost_letter(
    pane: &pane_layout::LivePane,
    run: &str,
    filed: pane_layout::Filed,
    written: &Written,
) -> Option<(String, String)> {
    let (why, road) = match (filed, written) {
        (pane_layout::Filed::Named, _) => return None,
        (_, Written::Cancelled) => (
            "in no saved tab, and the window went before its last write of the tabs was done",
            "exit-unwritten",
        ),
        (_, Written::Failed(_)) => (
            "in no saved tab, and the window's last write of the tabs failed",
            "exit-unwritten",
        ),
        (pane_layout::Filed::Unplaced, Written::Done) => (
            "in no saved tab and no workspace it could name",
            "exit-unplaced",
        ),
        (pane_layout::Filed::Asleep, Written::Done) => (
            "named only by a saved tab no pane stood for, and no workspace it could name",
            "exit-asleep",
        ),
        (_, Written::Done) => return None,
    };
    Some((
        format!(
            "the window went with this run's coordinator conversation ({} {}) {why} — reopen it by its id",
            pane.wake.agent, pane.wake.id
        ),
        format!("{road}:{run}:{}", pane.wake.id),
    ))
}
