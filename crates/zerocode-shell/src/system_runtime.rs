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

#[cfg(not(target_os = "macos"))]
pub(super) fn resident_bytes() -> u64 {
    // Every platform answers this differently and none of the others is a
    // target yet. Nothing is better than a number that means something else.
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

pub(super) fn note_window_event(local_data_root: &Path, line: &str) {
    use std::io::Write;
    let path = local_data_root.join("window-errors.log");
    if std::fs::metadata(&path).is_ok_and(|meta| meta.len() > 1_000_000) {
        let _ = std::fs::rename(&path, local_data_root.join("window-errors.log.1"));
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
    let normalized = raw.trim().to_ascii_lowercase();
    let parts: Vec<&str> = normalized.split('+').collect();
    let valid = matches!(parts.as_slice(), ["mod", key] if !key.is_empty())
        || matches!(parts.as_slice(), ["mod", "alt", key] if !key.is_empty())
        || matches!(parts.as_slice(), ["mod", "shift", key] if !key.is_empty())
        || matches!(parts.as_slice(), ["mod", "alt", "shift", key] if !key.is_empty());
    if !valid {
        return Err(format!("{raw}는 이 창이 인식하는 단축키가 아닙니다"));
    }
    Ok(normalized)
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
            layout
        })
        .collect()
}

pub(super) fn stage_layouts_file(config_root: &Path) -> PathBuf {
    config_root.join(artifact_file::STAGE_LAYOUTS)
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
pub(super) fn capture_scrollback_at_exit(app: &AppHandle) {
    let state = app.state::<AppState>();
    let file = pane_layouts_file(state.config_root());
    let mut layouts = pane_layout::read(&file);
    let terminals = state.terminals();
    for tabs in layouts.values_mut() {
        for layout in tabs.iter_mut() {
            for (ordinal, term) in std::mem::take(&mut layout.terms) {
                let Some(held) = terminals.handle(term) else {
                    continue;
                };
                let pty = lock_pty(&held);
                let written = zerocode_pty::serialize_tail(
                    pty.terminal().grid(),
                    zerocode_pty::SCROLLBACK_BUFFER_BYTE_LIMIT,
                );
                if !written.is_empty() {
                    layout.buffers.insert(ordinal, written);
                }
            }
        }
    }
    if let Ok(text) = serde_json::to_string(&layouts) {
        let _ = std::fs::write(&file, text);
    }
}
