//! Startup workspace risk classification.
//!
//! This module is intentionally pure and UI-free: it decides whether a cwd is
//! too broad to silently inherit the default full-access permission posture. A
//! later interactive trust prompt can reuse this classifier without entangling
//! prompting with permission-mode parsing.

use std::path::{Path, PathBuf};

use core_types::paths::{
    default_config_home, restrict_permissions_owner_only, zo_global_config_roots, ZO_DIR_NAME,
};
use runtime::PermissionMode;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WorkspaceRisk {
    Normal,
    HighBlastRadius,
}

impl WorkspaceRisk {
    #[must_use]
    pub(crate) const fn requires_safe_default(self) -> bool {
        matches!(self, Self::HighBlastRadius)
    }
}

#[must_use]
pub(crate) fn classify_cwd(cwd: &Path) -> WorkspaceRisk {
    let cwd = normalize_existing_path(cwd);
    if is_filesystem_root(&cwd) || is_global_home_or_ancestor(&cwd) {
        WorkspaceRisk::HighBlastRadius
    } else {
        WorkspaceRisk::Normal
    }
}

fn is_filesystem_root(path: &Path) -> bool {
    path.parent().is_none()
}

fn is_global_home_or_ancestor(cwd: &Path) -> bool {
    global_home_roots()
        .into_iter()
        .any(|root| root.matches_cwd(cwd))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct GlobalHomeRoot {
    path: PathBuf,
    protect_ancestors: bool,
}

impl GlobalHomeRoot {
    fn matches_cwd(&self, cwd: &Path) -> bool {
        cwd == self.path || (self.protect_ancestors && self.path.starts_with(cwd))
    }
}

fn global_home_roots() -> Vec<GlobalHomeRoot> {
    let mut roots = Vec::new();
    for config_home in zo_global_config_roots()
        .into_iter()
        .filter(|path| !path.as_os_str().is_empty())
    {
        let config_home = normalize_existing_path(&config_home);
        push_global_home_root(
            &mut roots,
            GlobalHomeRoot {
                path: config_home.clone(),
                protect_ancestors: false,
            },
        );
        if is_conventional_zo_config_dir(&config_home) {
            if let Some(parent) = config_home.parent() {
                push_global_home_root(
                    &mut roots,
                    GlobalHomeRoot {
                        path: parent.to_path_buf(),
                        protect_ancestors: true,
                    },
                );
            }
        }
    }
    roots
}

fn push_global_home_root(roots: &mut Vec<GlobalHomeRoot>, root: GlobalHomeRoot) {
    if let Some(existing) = roots.iter_mut().find(|existing| existing.path == root.path) {
        existing.protect_ancestors |= root.protect_ancestors;
    } else {
        roots.push(root);
    }
}

fn is_conventional_zo_config_dir(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|name| name == std::ffi::OsStr::new(ZO_DIR_NAME))
}

fn normalize_existing_path(path: &Path) -> PathBuf {
    if let Ok(canonical) = std::fs::canonicalize(path) {
        return canonical;
    }
    if let (Some(parent), Some(file_name)) = (path.parent(), path.file_name()) {
        if !parent.as_os_str().is_empty() && parent != path {
            return normalize_existing_path(parent).join(file_name);
        }
    }
    core_types::paths::normalize_path_components(path)
}

// ============================================================================
// Interactive trust gate (Claude-Code-style "do you trust this folder?")
// ============================================================================

/// Persisted decisions, keyed by canonical cwd → permission-mode label, under
/// `~/.zo/trusted_workspaces.json`. Remembered so a folder is asked about
/// only on its first interactive visit.
const TRUST_STORE_FILE: &str = "trusted_workspaces.json";

/// The single canonical *write* location for trust decisions: the primary
/// (highest-priority) global config home. Reads still consult every root; only
/// writes land here, so a merged view never gets scattered back across roots.
fn primary_trust_store_path() -> PathBuf {
    default_config_home().join(TRUST_STORE_FILE)
}

/// Merge the trust store across every canonical global config root, with
/// higher-priority roots winning on key collision.
///
/// `zo_global_config_roots()` is highest-priority first (`ZO_CONFIG_HOME` →
/// `ZO_HOME` → `~/.zo` → read-only legacy `~/.forge`). We apply the roots
/// low-to-high so a higher-priority root's decision overwrites a lower one's
/// for the same workspace key, giving deterministic precedence regardless of
/// how many roots exist.
fn load_trust_store() -> std::collections::BTreeMap<String, String> {
    let mut roots = zo_global_config_roots();
    if roots.is_empty() {
        roots.push(default_config_home());
    }
    let mut merged = std::collections::BTreeMap::new();
    for root in roots.into_iter().rev() {
        let path = root.join(TRUST_STORE_FILE);
        let Some(entries) = std::fs::read_to_string(&path)
            .ok()
            .and_then(|raw| {
                serde_json::from_str::<std::collections::BTreeMap<String, String>>(&raw).ok()
            })
        else {
            continue;
        };
        merged.extend(entries);
    }
    merged
}

/// Persist the merged trust store to the primary root only, hardening the
/// directory and file to owner-only access (`0o700`/`0o600` on Unix) so a
/// no-home fallback home or a fresh config dir never leaves trust decisions
/// world-readable.
#[cfg(test)]
fn save_trust_store(store: &std::collections::BTreeMap<String, String>) {
    let _ = try_save_trust_store(store);
}

fn try_save_trust_store(
    store: &std::collections::BTreeMap<String, String>,
) -> std::io::Result<()> {
    let path = primary_trust_store_path();
    let json = serde_json::to_string_pretty(store).map_err(std::io::Error::other)?;
    write_private_trust_store(&path, json.as_bytes())
}

fn validate_private_trust_store(path: &Path) -> std::io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() => {
            restrict_permissions_owner_only(path)
        }
        Ok(_) => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "workspace trust store must be a regular file",
        )),
        Err(error) => Err(error),
    }
}

fn create_private_trust_store(path: &Path) -> std::io::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options
            .mode(0o600)
            .custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_NONBLOCK);
    }
    match options.open(path) {
        Ok(file) if file.metadata()?.is_file() => restrict_permissions_owner_only(path),
        Ok(_) => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "workspace trust store must be a regular file",
        )),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            validate_private_trust_store(path)
        }
        Err(error) => Err(error),
    }
}

fn write_private_trust_store(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
        restrict_permissions_owner_only(parent)?;
    }
    match validate_private_trust_store(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            create_private_trust_store(path)?;
        }
        Err(error) => return Err(error),
    }

    replace_private_trust_store(path, contents)?;
    restrict_permissions_owner_only(path)
}

fn replace_private_trust_store(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    crate::write_atomic(path, contents)
}

fn mode_label(mode: PermissionMode) -> &'static str {
    match mode {
        PermissionMode::ReadOnly => "read-only",
        PermissionMode::WorkspaceWrite => "workspace-write",
        PermissionMode::DangerFullAccess => "danger-full-access",
        PermissionMode::Prompt => "prompt",
        PermissionMode::Allow => "allow",
    }
}

fn mode_from_label(label: &str) -> Option<PermissionMode> {
    match label {
        "read-only" => Some(PermissionMode::ReadOnly),
        "workspace-write" => Some(PermissionMode::WorkspaceWrite),
        "danger-full-access" => Some(PermissionMode::DangerFullAccess),
        "prompt" => Some(PermissionMode::Prompt),
        "allow" => Some(PermissionMode::Allow),
        _ => None,
    }
}

fn trust_key(cwd: &Path) -> String {
    normalize_existing_path(cwd).to_string_lossy().into_owned()
}

/// Resolve the permission mode for the *interactive* (TUI) entry, applying the
/// CC-style trust gate.
///
/// - A folder already recorded in the trust store reuses its saved mode — no
///   prompt, so a trusted project opens straight into its chosen mode.
/// - A first interactive visit (a real TTY on both stdin and stdout) shows the
///   3-way prompt, persists the choice, and uses it.
/// - With no TTY (headless redirect, CI, a piped stdin) there is nothing to
///   prompt, so the caller's risk-aware `default_for` is returned unchanged —
///   this is why `-p`/serve never block on a prompt.
pub(crate) fn resolve_trust_for_cwd(
    cwd: &Path,
    default_for: impl Fn(&Path) -> PermissionMode,
    inline: bool,
) -> PermissionMode {
    let key = trust_key(cwd);
    let mut store = load_trust_store();
    if let Some(saved) = store.get(&key).and_then(|label| mode_from_label(label)) {
        return saved;
    }
    if is_interactive_tty() {
        if let Some(chosen) = prompt_workspace_trust(cwd, inline) {
            store.insert(key, mode_label(chosen).to_string());
            if let Err(error) = try_save_trust_store(&store) {
                eprintln!(
                    "[zo] warning: failed to remember workspace trust for {}: {error}",
                    cwd.display()
                );
            }
            return chosen;
        }
    }
    default_for(cwd)
}

fn is_interactive_tty() -> bool {
    use std::io::IsTerminal;
    std::io::stdin().is_terminal() && std::io::stdout().is_terminal()
}

/// Render the 3-way trust prompt and read a choice from stdin. `None` only if
/// the read fails (e.g. EOF), in which case the caller keeps its safe default.
/// Render the CC-style trust modal as a real TUI (`ratatui` on an
/// alternate-screen / raw-mode terminal) and read the choice with
/// arrow keys + Enter. `None` if the user aborts (Esc / Ctrl-C) or the terminal
/// can't be put into raw mode.
#[allow(clippy::too_many_lines)]
fn prompt_workspace_trust(cwd: &Path, inline: bool) -> Option<PermissionMode> {
    if inline {
        prompt_workspace_trust_inline(cwd)
    } else {
        prompt_workspace_trust_fullscreen(cwd)
    }
}

#[allow(clippy::too_many_lines)]
fn prompt_workspace_trust_fullscreen(cwd: &Path) -> Option<PermissionMode> {
    use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
    use crossterm::execute;
    use crossterm::terminal::{
        disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
    };
    use ratatui::backend::CrosstermBackend;
    use ratatui::layout::{Alignment, Rect};
    use ratatui::style::{Color, Modifier, Style};
    use ratatui::text::{Line, Span};
    use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};
    use ratatui::Terminal;

    // Put the terminal into the same alternate-screen / raw-mode posture the
    // main TUI uses, so the modal renders at the top of a clean canvas and
    // keystrokes arrive as `KeyEvent`s (Up/Down/Enter) instead of buffered lines.
    if enable_raw_mode().is_err() {
        return None;
    }
    let mut stdout = std::io::stdout();
    if execute!(stdout, EnterAlternateScreen).is_err() {
        let _ = disable_raw_mode();
        return None;
    }
    let backend = CrosstermBackend::new(stdout);
    let Ok(mut terminal) = Terminal::new(backend) else {
        let _ = execute!(std::io::stdout(), LeaveAlternateScreen);
        let _ = disable_raw_mode();
        return None;
    };
    let _ = terminal.clear();

    let options: [(&str, &str, PermissionMode); 3] = [
        (
            "Full access",
            "remember this folder — full read / edit / run",
            PermissionMode::DangerFullAccess,
        ),
        (
            "Ask before edits / commands",
            "ask each time before a write or command",
            PermissionMode::Prompt,
        ),
        (
            "Read-only",
            "browse only, no changes",
            PermissionMode::ReadOnly,
        ),
    ];
    // Default to the safer "Ask" option, matching CC.
    let mut selected: usize = 1;
    let cwd_str = cwd.display().to_string();

    let result: Option<PermissionMode> = loop {
        let draw = terminal.draw(|f| {
            let full = f.area();
            // Center a fixed-size modal — large enough for the longest hint, small
            // enough to read as a dialog on tall terminals.
            let modal_w: u16 = 72;
            let modal_h: u16 = 14;
            let x = full.x + full.width.saturating_sub(modal_w) / 2;
            let y = full.y + full.height.saturating_sub(modal_h) / 3;
            let area = Rect::new(
                x,
                y,
                modal_w.min(full.width),
                modal_h.min(full.height),
            );
            // Clear the underlying buffer so the modal does not pick up stray
            // characters from whatever was on screen before.
            f.render_widget(Clear, area);

            let border = Block::default()
                .title(Span::styled(
                    " Trust this folder? ",
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ))
                .title_alignment(Alignment::Left)
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(Color::Yellow));

            let mut lines: Vec<Line> = Vec::with_capacity(12);
            lines.push(Line::from(Span::styled(
                cwd_str.clone(),
                Style::default().fg(Color::Gray),
            )));
            lines.push(Line::from(""));
            lines.push(Line::from(
                "Zo can read, edit & run commands in this folder.",
            ));
            lines.push(Line::from(Span::styled(
                "Only proceed if you trust the contents.",
                Style::default().fg(Color::DarkGray),
            )));
            lines.push(Line::from(""));

            for (i, (label, hint, _)) in options.iter().enumerate() {
                let is_sel = i == selected;
                let marker = if is_sel { "▸" } else { " " };
                let head_style = if is_sel {
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::Yellow)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().add_modifier(Modifier::BOLD)
                };
                lines.push(Line::from(vec![
                    Span::raw(format!(" {marker}  {}. ", i + 1)),
                    Span::styled(format!(" {label} "), head_style),
                    Span::raw("  "),
                    Span::styled(*hint, Style::default().fg(Color::DarkGray)),
                ]));
            }
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "↑/↓ select   1/2/3 jump   Enter confirm   Esc cancel   \
                 — choice is remembered, you won't be asked again here",
                Style::default().fg(Color::DarkGray),
            )));

            // Wrapped with the shared engine instead of `.wrap(...)`: ratatui's
            // `WordWrapper` deletes a 2-cell grapheme that lands on the break,
            // and the first row here is the working directory — a path that is
            // routinely non-ASCII. A trust prompt that garbles the very path it
            // is asking about is the one place that must stay legible.
            // `border` takes one cell of padding on each side.
            let inner_width = usize::from(area.width.saturating_sub(2)).max(1);
            let wrapped: Vec<Line> = lines
                .iter()
                .flat_map(|line| {
                    zo_cli::tui::text_metrics::wrap_line_to_cells(line, inner_width, false)
                })
                .collect();
            let para = Paragraph::new(wrapped).block(border);
            f.render_widget(para, area);
        });
        if draw.is_err() {
            break None;
        }

        match event::read() {
            Ok(Event::Key(k)) if k.kind != KeyEventKind::Release => match k.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    selected = selected.saturating_sub(1);
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    if selected + 1 < options.len() {
                        selected += 1;
                    }
                }
                KeyCode::Char('1') => break Some(options[0].2),
                KeyCode::Char('2') => break Some(options[1].2),
                KeyCode::Char('3') => break Some(options[2].2),
                KeyCode::Enter => break Some(options[selected].2),
                // CC parity: Esc means "do not trust — exit zo", NOT
                // "fall through to the default". Before this fix Esc returned
                // `None`, the caller substituted the workspace-risk-aware
                // default (DangerFullAccess for a normal project), and zo
                // launched anyway — the opposite of what the user just asked.
                KeyCode::Esc => {
                    let _ = execute!(terminal.backend_mut(), LeaveAlternateScreen);
                    let _ = disable_raw_mode();
                    eprintln!(
                        "Trust prompt cancelled — exiting zo. \
                         Re-run from a folder you trust, or pass \
                         `--permission-mode read-only` to browse without trusting it."
                    );
                    std::process::exit(0);
                }
                KeyCode::Char('c') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                    let _ = execute!(terminal.backend_mut(), LeaveAlternateScreen);
                    let _ = disable_raw_mode();
                    // 128 + SIGINT(2): the conventional exit code for "killed
                    // by Ctrl-C", so shells and scripts can tell the difference
                    // from a deliberate Esc-cancel above.
                    std::process::exit(130);
                }
                _ => {}
            },
            Ok(_) => {}
            Err(_) => break None,
        }
    };

    // Tear the alternate screen down in the *reverse* order it was set up so
    // the main TUI inherits a normal-mode, primary-screen terminal — the same
    // contract the rest of the binary uses on shutdown (`main.rs` parity).
    let _ = execute!(terminal.backend_mut(), LeaveAlternateScreen);
    let _ = disable_raw_mode();
    result
}

#[allow(clippy::too_many_lines)]
fn prompt_workspace_trust_inline(cwd: &Path) -> Option<PermissionMode> {
    use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
    use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
    use ratatui::backend::CrosstermBackend;
    use ratatui::layout::Rect;
    use ratatui::style::{Color, Modifier, Style};
    use ratatui::text::{Line, Span};
    use ratatui::widgets::{Block, BorderType, Borders, Paragraph};
    use ratatui::{Terminal, TerminalOptions, Viewport};

    if enable_raw_mode().is_err() {
        return None;
    }
    let backend = CrosstermBackend::new(std::io::stdout());
    let Ok(mut terminal) = Terminal::with_options(
        backend,
        TerminalOptions {
            viewport: Viewport::Inline(zo_cli::tui::INLINE_VIEWPORT_MIN_HEIGHT),
        },
    ) else {
        let _ = disable_raw_mode();
        return None;
    };

    let options: [(&str, &str, PermissionMode); 3] = [
        (
            "Full access",
            "read / edit / run",
            PermissionMode::DangerFullAccess,
        ),
        (
            "Ask before edits / commands",
            "confirm writes and commands",
            PermissionMode::Prompt,
        ),
        ("Read-only", "browse only", PermissionMode::ReadOnly),
    ];
    let mut selected = 1usize;
    let cwd_str = cwd.display().to_string();

    let result = loop {
        if terminal
            .draw(|frame| {
                let full = frame.area();
                let area = Rect::new(full.x, full.y, full.width, full.height);
                let border = Block::default()
                    .title(Span::styled(
                        " Trust this folder? ",
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    ))
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .border_style(Style::default().fg(Color::Yellow));
                let mut lines = vec![
                    Line::from(Span::styled(cwd_str.clone(), Style::default().fg(Color::Gray))),
                    Line::from("Only proceed if you trust the contents."),
                    Line::from(""),
                ];
                for (index, (label, hint, _)) in options.iter().enumerate() {
                    let selected_row = index == selected;
                    let marker = if selected_row { "▸" } else { " " };
                    let style = if selected_row {
                        Style::default()
                            .fg(Color::Black)
                            .bg(Color::Yellow)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().add_modifier(Modifier::BOLD)
                    };
                    lines.push(Line::from(vec![
                        Span::raw(format!(" {marker}  {}. ", index + 1)),
                        Span::styled(format!(" {label} "), style),
                        Span::raw("  "),
                        Span::styled(*hint, Style::default().fg(Color::DarkGray)),
                    ]));
                }
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    "↑/↓ select · Enter confirm · Esc cancel",
                    Style::default().fg(Color::DarkGray),
                )));
                frame.render_widget(Paragraph::new(lines).block(border), area);
            })
            .is_err()
        {
            break None;
        }

        match event::read() {
            Ok(Event::Key(key)) if key.kind != KeyEventKind::Release => match key.code {
                KeyCode::Up | KeyCode::Char('k') => selected = selected.saturating_sub(1),
                KeyCode::Down | KeyCode::Char('j') => {
                    selected = selected.saturating_add(1).min(options.len() - 1);
                }
                KeyCode::Char('1') => break Some(options[0].2),
                KeyCode::Char('2') => break Some(options[1].2),
                KeyCode::Char('3') => break Some(options[2].2),
                KeyCode::Enter => break Some(options[selected].2),
                KeyCode::Esc => {
                    restore_inline_trust_terminal(&mut terminal);
                    eprintln!(
                        "Trust prompt cancelled — exiting zo. Re-run from a folder you trust, \
                         or pass `--permission-mode read-only` to browse without trusting it."
                    );
                    std::process::exit(0);
                }
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    restore_inline_trust_terminal(&mut terminal);
                    std::process::exit(130);
                }
                _ => {}
            },
            Ok(_) => {}
            Err(_) => break None,
        }
    };

    restore_inline_trust_terminal(&mut terminal);
    result
}

fn restore_inline_trust_terminal(
    terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
) {
    use crossterm::terminal::disable_raw_mode;
    use ratatui::layout::Position;

    let viewport_top = terminal.get_frame().area().y;
    let _ = terminal.clear();
    let _ = terminal.set_cursor_position(Position::new(0, viewport_top));
    let _ = terminal.show_cursor();
    let _ = disable_raw_mode();
}

#[cfg(test)]
mod tests {
    use super::{classify_cwd, WorkspaceRisk};
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "zo-workspace-trust-{label}-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ))
    }

    fn with_home<T>(home: &Path, f: impl FnOnce() -> T) -> T {
        with_env_paths(&[("HOME", Some(home))], f)
    }

    fn with_env_paths<T>(vars: &[(&str, Option<&Path>)], f: impl FnOnce() -> T) -> T {
        let previous = vars
            .iter()
            .map(|(key, _)| (*key, std::env::var_os(key)))
            .collect::<Vec<_>>();
        for (key, value) in vars {
            match value {
                Some(path) => std::env::set_var(key, path),
                None => std::env::remove_var(key),
            }
        }
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
        for (key, value) in previous {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
        match result {
            Ok(value) => value,
            Err(payload) => std::panic::resume_unwind(payload),
        }
    }

    #[test]
    fn workspace_trust_marks_home_as_high_blast_radius() {
        let _guard = crate::test_env_lock();
        let root = temp_dir("home");
        let home = root.join("joe");
        std::fs::create_dir_all(&home).expect("home dir");
        let risk = with_home(&home, || classify_cwd(&home));
        std::fs::remove_dir_all(root).expect("cleanup");
        assert_eq!(risk, WorkspaceRisk::HighBlastRadius);
    }

    #[test]
    fn workspace_trust_marks_home_ancestor_as_high_blast_radius() {
        let _guard = crate::test_env_lock();
        let root = temp_dir("ancestor");
        let home = root.join("users").join("joe");
        std::fs::create_dir_all(&home).expect("home dir");
        let users_dir = home.parent().expect("home parent").to_path_buf();
        let risk = with_home(&home, || classify_cwd(&users_dir));
        std::fs::remove_dir_all(root).expect("cleanup");
        assert_eq!(risk, WorkspaceRisk::HighBlastRadius);
    }

    #[test]
    fn workspace_trust_allows_project_below_home() {
        let _guard = crate::test_env_lock();
        let root = temp_dir("project");
        let home = root.join("joe");
        let project = home.join("repo");
        std::fs::create_dir_all(&project).expect("project dir");
        let risk = with_home(&home, || classify_cwd(&project));
        std::fs::remove_dir_all(root).expect("cleanup");
        assert_eq!(risk, WorkspaceRisk::Normal);
    }

    #[test]
    fn workspace_trust_marks_zo_config_home_parent_as_high_blast_radius() {
        let _guard = crate::test_env_lock();
        let root = temp_dir("zo-config-home");
        let protected_home = root.join("global-home");
        let config_home = protected_home.join(".zo");
        let unrelated_home = root.join("unrelated-home");
        std::fs::create_dir_all(&config_home).expect("config home dir");
        std::fs::create_dir_all(&unrelated_home).expect("unrelated home dir");
        let risk = with_env_paths(
            &[
                ("HOME", Some(&unrelated_home)),
                ("ZO_CONFIG_HOME", Some(&config_home)),
                ("ZO_HOME", None),
            ],
            || classify_cwd(&protected_home),
        );
        std::fs::remove_dir_all(root).expect("cleanup");
        assert_eq!(risk, WorkspaceRisk::HighBlastRadius);
    }

    #[test]
    fn workspace_trust_does_not_mark_arbitrary_config_parent_as_high_blast_radius() {
        let _guard = crate::test_env_lock();
        let root = temp_dir("custom-config-home");
        let config_parent = root.join("config-parent");
        let config_home = config_parent.join("zo-config");
        let unrelated_home = root.join("unrelated-home");
        std::fs::create_dir_all(&config_home).expect("config home dir");
        std::fs::create_dir_all(&unrelated_home).expect("unrelated home dir");
        let risk = with_env_paths(
            &[
                ("HOME", Some(&unrelated_home)),
                ("ZO_CONFIG_HOME", Some(&config_home)),
                ("ZO_HOME", None),
            ],
            || classify_cwd(&config_parent),
        );
        std::fs::remove_dir_all(root).expect("cleanup");
        assert_eq!(risk, WorkspaceRisk::Normal);
    }

    #[test]
    fn workspace_trust_marks_filesystem_root_as_high_blast_radius() {
        let _guard = crate::test_env_lock();
        let mut root = std::env::temp_dir();
        while let Some(parent) = root.parent() {
            root = parent.to_path_buf();
        }
        assert_eq!(classify_cwd(&root), WorkspaceRisk::HighBlastRadius);
    }

    #[test]
    fn mode_label_roundtrips_all_modes() {
        use super::{mode_from_label, mode_label};
        use runtime::PermissionMode;
        for mode in [
            PermissionMode::ReadOnly,
            PermissionMode::WorkspaceWrite,
            PermissionMode::DangerFullAccess,
            PermissionMode::Prompt,
            PermissionMode::Allow,
        ] {
            assert_eq!(mode_from_label(mode_label(mode)), Some(mode));
        }
    }

    #[test]
    fn resolve_trust_reuses_recorded_mode_without_prompt() {
        use super::{resolve_trust_for_cwd, trust_key};
        use runtime::PermissionMode;
        let _guard = crate::test_env_lock();
        let root = temp_dir("trust-store");
        let config_home = root.join(".zo");
        let project = root.join("project");
        std::fs::create_dir_all(&config_home).expect("config home dir");
        std::fs::create_dir_all(&project).expect("project dir");
        with_env_paths(
            &[
                ("ZO_CONFIG_HOME", Some(config_home.as_path())),
                ("ZO_HOME", None),
            ],
            || {
                // Pre-seed: this folder was trusted as full access.
                let key = trust_key(&project);
                let store = std::collections::BTreeMap::from([(
                    key,
                    "danger-full-access".to_string(),
                )]);
                std::fs::write(
                    config_home.join("trusted_workspaces.json"),
                    serde_json::to_string(&store).expect("serialize store"),
                )
                .expect("write store");
                // The recorded mode wins over the (ReadOnly) default, and the
                // store hit short-circuits before any prompt — safe under test.
                let mode =
                    resolve_trust_for_cwd(&project, |_| PermissionMode::ReadOnly, false);
                assert_eq!(mode, PermissionMode::DangerFullAccess);
            },
        );
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    fn write_store(dir: &Path, entries: &[(&str, &str)]) {
        let store = entries
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect::<std::collections::BTreeMap<String, String>>();
        std::fs::write(
            dir.join(super::TRUST_STORE_FILE),
            serde_json::to_string(&store).expect("serialize store"),
        )
        .expect("write store");
    }

    #[test]
    fn load_trust_store_merges_roots_with_higher_priority_winning() {
        use super::load_trust_store;
        let _guard = crate::test_env_lock();
        let root = temp_dir("merge-precedence");
        // Primary (ZO_CONFIG_HOME) is highest priority; secondary (ZO_HOME)
        // lower. Both name the same workspace key with different modes, plus a
        // key unique to each root, so we can prove precedence *and* union.
        let primary = root.join("primary");
        let secondary = root.join("secondary");
        std::fs::create_dir_all(&primary).expect("primary dir");
        std::fs::create_dir_all(&secondary).expect("secondary dir");
        write_store(
            &primary,
            &[("/shared", "danger-full-access"), ("/only-primary", "prompt")],
        );
        write_store(
            &secondary,
            &[("/shared", "read-only"), ("/only-secondary", "workspace-write")],
        );
        let merged = with_env_paths(
            &[
                ("ZO_CONFIG_HOME", Some(primary.as_path())),
                ("ZO_HOME", Some(secondary.as_path())),
                ("HOME", None),
            ],
            load_trust_store,
        );
        std::fs::remove_dir_all(&root).expect("cleanup");
        // Higher-priority primary wins the collision; both unique keys survive.
        assert_eq!(merged.get("/shared").map(String::as_str), Some("danger-full-access"));
        assert_eq!(merged.get("/only-primary").map(String::as_str), Some("prompt"));
        assert_eq!(
            merged.get("/only-secondary").map(String::as_str),
            Some("workspace-write")
        );
    }

    #[test]
    fn save_trust_store_writes_primary_root_only() {
        use super::{load_trust_store, save_trust_store, TRUST_STORE_FILE};
        let _guard = crate::test_env_lock();
        let root = temp_dir("primary-only-write");
        let primary = root.join("primary");
        let secondary = root.join("secondary");
        std::fs::create_dir_all(&primary).expect("primary dir");
        std::fs::create_dir_all(&secondary).expect("secondary dir");
        // Seed a decision only in the lower-priority root.
        write_store(&secondary, &[("/from-secondary", "read-only")]);
        with_env_paths(
            &[
                ("ZO_CONFIG_HOME", Some(primary.as_path())),
                ("ZO_HOME", Some(secondary.as_path())),
                ("HOME", None),
            ],
            || {
                // Load merges both roots, then we record a new decision.
                let mut store = load_trust_store();
                store.insert("/new-project".to_string(), "prompt".to_string());
                save_trust_store(&store);
            },
        );
        // The write landed in primary only; the secondary file is untouched.
        assert!(primary.join(TRUST_STORE_FILE).exists());
        let secondary_raw =
            std::fs::read_to_string(secondary.join(TRUST_STORE_FILE)).expect("secondary store");
        let secondary_store: std::collections::BTreeMap<String, String> =
            serde_json::from_str(&secondary_raw).expect("parse secondary");
        assert_eq!(
            secondary_store,
            std::collections::BTreeMap::from([(
                "/from-secondary".to_string(),
                "read-only".to_string()
            )]),
            "lower-priority root must not be rewritten"
        );
        let primary_raw =
            std::fs::read_to_string(primary.join(TRUST_STORE_FILE)).expect("primary store");
        let primary_store: std::collections::BTreeMap<String, String> =
            serde_json::from_str(&primary_raw).expect("parse primary");
        // Primary holds the full merged view (both roots' keys plus the new one).
        assert_eq!(primary_store.get("/from-secondary").map(String::as_str), Some("read-only"));
        assert_eq!(primary_store.get("/new-project").map(String::as_str), Some("prompt"));
        std::fs::remove_dir_all(&root).expect("cleanup");
    }

    #[test]
    fn legacy_forge_trust_loads_but_save_writes_primary_zo_only() {
        use super::{load_trust_store, save_trust_store, TRUST_STORE_FILE};
        let _guard = crate::test_env_lock();
        let root = temp_dir("legacy-forge-store");
        let legacy = root.join(".forge");
        let primary = root.join(".zo");
        std::fs::create_dir_all(&legacy).expect("legacy dir");
        write_store(&legacy, &[("/legacy-project", "read-only")]);
        let legacy_before = std::fs::read_to_string(legacy.join(TRUST_STORE_FILE))
            .expect("legacy store before save");

        with_env_paths(
            &[
                ("ZO_CONFIG_HOME", None),
                ("ZO_HOME", None),
                ("HOME", Some(root.as_path())),
            ],
            || {
                let mut store = load_trust_store();
                assert_eq!(
                    store.get("/legacy-project").map(String::as_str),
                    Some("read-only")
                );
                store.insert("/new-project".to_string(), "prompt".to_string());
                save_trust_store(&store);
            },
        );

        assert!(primary.join(TRUST_STORE_FILE).exists());
        assert_eq!(
            std::fs::read_to_string(legacy.join(TRUST_STORE_FILE))
                .expect("legacy store after save"),
            legacy_before,
            "legacy trust store must remain read-only"
        );
        let primary_store: std::collections::BTreeMap<String, String> = serde_json::from_str(
            &std::fs::read_to_string(primary.join(TRUST_STORE_FILE)).expect("primary store"),
        )
        .expect("parse primary store");
        assert_eq!(
            primary_store.get("/legacy-project").map(String::as_str),
            Some("read-only")
        );
        assert_eq!(primary_store.get("/new-project").map(String::as_str), Some("prompt"));
        std::fs::remove_dir_all(&root).expect("cleanup");
    }

    #[cfg(unix)]
    #[test]
    fn save_trust_store_hardens_directory_and_file_owner_only() {
        use super::{save_trust_store, TRUST_STORE_FILE};
        use std::os::unix::fs::PermissionsExt as _;
        let _guard = crate::test_env_lock();
        let root = temp_dir("perms");
        // Config home starts world-accessible; a pre-existing loose store file
        // exercises the "harden an already-broad file" path, not just creation.
        let config_home = root.join("global");
        std::fs::create_dir_all(&config_home).expect("config dir");
        std::fs::set_permissions(&config_home, std::fs::Permissions::from_mode(0o755))
            .expect("loosen dir");
        let store_path = config_home.join(TRUST_STORE_FILE);
        std::fs::write(&store_path, "{}").expect("seed store");
        std::fs::set_permissions(&store_path, std::fs::Permissions::from_mode(0o644))
            .expect("loosen file");
        with_env_paths(
            &[
                ("ZO_CONFIG_HOME", Some(config_home.as_path())),
                ("ZO_HOME", None),
                ("HOME", None),
            ],
            || {
                let store = std::collections::BTreeMap::from([(
                    "/project".to_string(),
                    "prompt".to_string(),
                )]);
                save_trust_store(&store);
            },
        );
        let dir_mode =
            std::fs::metadata(&config_home).expect("dir meta").permissions().mode() & 0o777;
        let file_mode =
            std::fs::metadata(&store_path).expect("file meta").permissions().mode() & 0o777;
        std::fs::remove_dir_all(&root).expect("cleanup");
        assert_eq!(dir_mode, 0o700, "config dir must be owner-only");
        assert_eq!(file_mode, 0o600, "trust store file must be owner-only");
    }

    #[cfg(unix)]
    #[test]
    fn trust_store_replace_failure_preserves_previous_bytes() {
        use super::replace_private_trust_store;
        use std::os::unix::fs::PermissionsExt as _;

        let root = temp_dir("atomic-failure");
        std::fs::create_dir_all(&root).expect("create trust directory");
        let path = root.join("trusted_workspaces.json");
        let before = br#"{"/old":"prompt"}"#;
        std::fs::write(&path, before).expect("seed trust store");
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o555))
            .expect("make trust directory read-only");

        let probe = root.join("probe");
        if std::fs::write(&probe, b"probe").is_ok() {
            let _ = std::fs::remove_file(probe);
            std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755))
                .expect("restore trust directory permissions");
            let _ = std::fs::remove_dir_all(root);
            return;
        }

        let result = replace_private_trust_store(&path, br#"{"/new":"read-only"}"#);

        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755))
            .expect("restore trust directory permissions");
        let after = std::fs::read(&path).expect("read trust store after failed save");
        std::fs::remove_dir_all(&root).expect("cleanup");
        assert!(result.is_err(), "allocating the sibling temp must fail");
        assert_eq!(after, before, "failed save must preserve prior trust bytes");
    }

    #[cfg(unix)]
    #[test]
    fn save_trust_store_hardens_no_home_fallback() {
        use super::{load_trust_store, save_trust_store, TRUST_STORE_FILE};
        use core_types::paths::default_config_home;
        use std::os::unix::fs::PermissionsExt as _;
        let _guard = crate::test_env_lock();
        // With no home resolvable at all, default_config_home() allocates a
        // private temporary home; the trust store written there must still be
        // owner-only rather than leaking into a shared temp dir world-readable.
        with_env_paths(
            &[
                ("ZO_CONFIG_HOME", None),
                ("ZO_HOME", None),
                ("HOME", None),
            ],
            || {
                let store = std::collections::BTreeMap::from([(
                    "/project".to_string(),
                    "prompt".to_string(),
                )]);
                save_trust_store(&store);
                assert_eq!(
                    load_trust_store(),
                    store,
                    "no-home fallback writes must be visible to canonical reads"
                );
                let home = default_config_home();
                let store_path = home.join(TRUST_STORE_FILE);
                let file_mode = std::fs::metadata(&store_path)
                    .expect("fallback store meta")
                    .permissions()
                    .mode()
                    & 0o777;
                assert_eq!(file_mode, 0o600, "fallback trust store must be owner-only");
            },
        );
    }
}
