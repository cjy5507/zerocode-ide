//! The common store kept in step on disk (`zerocode_core::library` plans, this
//! module reads and writes).
//!
//! One look reads the store, every present agent's MCP list and skill folders,
//! asks the planner what to do, and does it: a server into each agent through
//! that agent's own command (or a backed-up edit of the file where it has no
//! command that can), a skill moved or copied into the store, a duplicate to
//! the Trash. A look runs when the window starts, when a watched file moves
//! (the `library` lane of the window's file watch), and when the person acts on
//! the Skills·MCP page. Looks never overlap ([`RUN`]).
//!
//! What it never does: write the person's own Claude home (`~/.claude`,
//! `~/.claude.json` — read only, `accounts`); write a list it could not read;
//! print, log or keep a server's environment or header values anywhere but
//! the store and the agents' own files, where the agents keep them already.
//! What the person may want back — a removed server, a trashed skill — is held
//! in memory for this session's 「되돌리기」 ([`UNDO`]), and a file the window
//! edits — an agent's list, or the store's own — is copied to
//! `library/backups` first.

use super::*;
use std::ffi::OsString;
use std::sync::mpsc;
use std::sync::{Mutex, OnceLock, PoisonError};

use zerocode_core::library;
use zerocode_core::library::agents::{
    AGENTS, LibraryAgent, MAC_CLAUDE, MAC_CLAUDE_MCP, MAC_CLAUDE_SKILLS, McpPlace, Skills,
};
use zerocode_core::library::ledger::{Candidate, Ledger, SkillCandidate};
use zerocode_core::library::mcp::{self, Format, McpDef, McpEntry, Write};
use zerocode_core::library::plan::{
    self, AgentList, McpStep, McpView, Note, SkillFolder, SkillPlace, SkillStep, SkillsView,
};

/// The file-watch lane the store's folders and the agents' lists ride.
pub(crate) const WATCH_LANE: &str = "library";
/// What the window hears after a look changed something.
pub(crate) const CHANGED_EVENT: &str = "library:changed";

const FOLDER: &str = "library";
const LEDGER_FILE: &str = "ledger.json";
const LOG_FILE: &str = "log.json";
const BACKUPS: &str = "backups";
const HELD: &str = "held";
/// Beside the window's Claude `skills`, where the link to the store is made
/// before anything moves ([`link_claude_home`]).
const CLAUDE_LINKING: &str = "skills.linking";
/// In the window's `library` folder: the link for the window's Claude could
/// not be made here, and is not tried again until [`CLAUDE_RELINK_AFTER`]
/// has passed since its time ([`link_for_claude`]).
const CLAUDE_UNLINKED: &str = "claude-home-unlinked";
/// How long a [`CLAUDE_UNLINKED`] refusal stands: a cause that passes — a
/// scanner holding the folder, a `cmd` that would not start — leaves the
/// window's Claude apart from the store for a day at most (the second review
/// of v1.1.129's skills, t-99089, P3-C).
const CLAUDE_RELINK_AFTER: Duration = Duration::from_secs(24 * 60 * 60);
/// Lines of 「최근 변경」 kept.
const LOG_KEEP: usize = 200;
/// Backups kept per agent file, and of the store's list. Each is one small
/// JSON or TOML file; five went in a few looks (the review of v1.1.128, P1),
/// and twenty still sit under a megabyte for a list of servers.
const BACKUPS_KEEP: usize = 20;
/// The backups folder the store's own list is kept in, beside the agents'.
const STORE_BACKUPS: &str = "store";
/// The most entries (files, folders and links) a skill folder is copied with,
/// and the most bytes. A skill is a page of instructions with a few scripts
/// and assets beside it; a folder past either is a project linked in by
/// mistake — a repository with its `.git` and `node_modules` — and copying it
/// whole, again at every edit, could fill the disk (the review of v1.1.128,
/// P2-3). Such a folder is refused, and the line says why.
const SKILL_COPY_ENTRIES: usize = 2_000;
const SKILL_COPY_BYTES: u64 = 128 * 1024 * 1024;
/// How long a burst of watched changes is let settle before one look.
const SETTLE: Duration = Duration::from_millis(800);
/// How long one agent command may take.
const CLI_BUDGET: Duration = Duration::from_secs(45);
/// How long the repository listing may take (it may fetch the lister).
const LIST_BUDGET: Duration = Duration::from_secs(120);
/// The most entries a skill folder is read to by its digest.
const DIGEST_ENTRIES: usize = 400;
/// The longest line of an agent's refusal kept in 「최근 변경」.
const DETAIL_CHARS: usize = 200;
/// The window's own Codex config, re-copied from `~/.codex` at every launch:
/// read for what a pane added there, never written.
const CODEX_WINDOW: &str = "codex-window";

/// One look at a time, from any thread.
static RUN: Mutex<()> = Mutex::new(());
/// The background thread's doorbell.
static POKE: OnceLock<mpsc::Sender<()>> = OnceLock::new();
/// What a 「되돌리기」 puts back, for this session only — a server's whole
/// definition, its values included, never touches a disk the window keeps.
static UNDO: Mutex<BTreeMap<u64, Undo>> = Mutex::new(BTreeMap::new());
/// The last log id handed out.
static LAST_ID: Mutex<u64> = Mutex::new(0);
/// Whether a look is running, for the page's pill.
static RUNNING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// What the last watched look started from ([`inputs_stamp`]).
static LOOKED_AT: Mutex<Option<u64>> = Mutex::new(None);

enum Undo {
    /// A server removed: put it back into the store.
    Server { name: String, entry: McpEntry },
    /// A server added by the person: take it out.
    Added { name: String },
    /// A server switched off or on.
    Switched { name: String, was_disabled: bool },
    /// A server switched off or on for one agent.
    Agent {
        name: String,
        agent: String,
        was_off: bool,
    },
    /// A server taken in: keep it with every agent that held it then, and
    /// out of the others (the review of v1.1.128 skills-p2, P3-5).
    TakenIn {
        name: String,
        holders: BTreeSet<String>,
    },
    /// Folders moved to the Trash, and links removed ([`put_away`]).
    Trashed {
        name: String,
        receipts: Vec<Removed>,
    },
}

/// What [`put_away`] took away, and how it comes back.
enum Removed {
    /// A folder, through the Trash.
    Trashed(file_tree_io::TrashReceipt),
    /// A link, removed itself: `at` read `reads`, as it was written.
    Link { at: PathBuf, reads: PathBuf },
}

impl Removed {
    fn restore(&self) -> Result<(), String> {
        match self {
            Self::Trashed(receipt) => receipt.restore(),
            Self::Link { at, reads } => {
                if std::fs::symlink_metadata(at).is_ok() {
                    return Err("tree.changed".to_string());
                }
                link_back(reads, at)
            }
        }
    }
}

/// Everything one look needs, read off the window once.
#[derive(Clone)]
pub(crate) struct Context {
    home: PathBuf,
    data: PathBuf,
    /// The home the window launches Claude Code with, while it is the
    /// window's own; `None` while the person chose the machine's own login.
    claude_home: Option<PathBuf>,
    /// The home the window launches Codex with.
    codex_home: PathBuf,
    /// The XDG config home (`XDG_CONFIG_HOME`, `~/.config` when unset): an
    /// agent's folder written under `.config/` lives here instead.
    config_home: PathBuf,
    present: Vec<&'static LibraryAgent>,
    path: Option<OsString>,
}

impl Context {
    pub(crate) fn from_state(state: &AppState) -> Result<Self, String> {
        // `HOME` first, as every agent reads it; Windows has none and keeps
        // its home in the profile folder.
        let home = std::env::var_os("HOME")
            .filter(|home| !home.is_empty())
            .map(PathBuf::from)
            .or_else(dirs::home_dir)
            .ok_or("library.homeUnavailable")?;
        let data = state.local_data_root().to_path_buf();
        let claude_home = match accounts::login_look(state.config_root()) {
            accounts::LoginLook::Managed(home) => Some(home),
            accounts::LoginLook::System(_) => None,
        };
        let codex_home = codex_accounts::runtime_home(&data);
        let config_home = std::env::var_os("XDG_CONFIG_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".config"));
        let installed: Vec<String> = skills_runtime::installed_agents()
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        let path = shell_path::hydrated().map(OsString::from);
        let present = AGENTS
            .iter()
            .filter(|agent| {
                (agent.in_catalog && installed.iter().any(|id| id == agent.id))
                    || on_path(agent.binary, path.as_deref())
            })
            .collect();
        Ok(Self {
            home,
            data,
            claude_home,
            codex_home,
            config_home,
            present,
            path,
        })
    }

    fn folder(&self) -> PathBuf {
        self.data.join(FOLDER)
    }

    fn store_skills(&self) -> PathBuf {
        library::store_skills_dir(&self.home)
    }

    fn store_mcp(&self) -> PathBuf {
        library::store_mcp_file(&self.home)
    }

    /// The file an agent keeps its user MCP list in.
    fn mcp_file(&self, agent: &LibraryAgent) -> Option<PathBuf> {
        match agent.mcp {
            McpPlace::Home(relative) => Some(self.home.join(relative)),
            McpPlace::ClaudeWindow => self
                .claude_home
                .as_ref()
                .map(|home| home.join(".claude.json")),
            McpPlace::OpenCode => Some(self.config_home.join("opencode").join("opencode.json")),
        }
    }

    /// The folders a present agent installs skills into by itself.
    fn own_skill_dirs(&self, agent: &LibraryAgent) -> Vec<PathBuf> {
        let mut dirs: Vec<PathBuf> = agent
            .own_skills
            .iter()
            .map(|relative| match relative.strip_prefix(".config/") {
                // Under the XDG config home, as the agent reads it (the
                // review of the skills store, P3-3).
                Some(under) => self.config_home.join(under),
                None => self.home.join(relative),
            })
            .collect();
        if agent.id == "codex" {
            dirs.push(self.codex_home.join("skills"));
        }
        dirs
    }
}

/// Whether `binary` is a file on `path` — the shell's PATH, as every agent
/// road reads it.
fn on_path(binary: &str, path: Option<&std::ffi::OsStr>) -> bool {
    let Some(path) = path else {
        return false;
    };
    let names: Vec<String> = if cfg!(windows) {
        ["", ".exe", ".cmd", ".bat"]
            .iter()
            .map(|suffix| format!("{binary}{suffix}"))
            .collect()
    } else {
        vec![binary.to_string()]
    };
    std::env::split_paths(path)
        .filter(|dir| !dir.as_os_str().is_empty())
        .any(|dir| names.iter().any(|name| dir.join(name).is_file()))
}

/// The variable the window's terminals carry so zo reads the store too.
pub(crate) fn shell_env() -> (String, String) {
    (library::ZO_COMMON_SKILLS_ENV.to_string(), "1".to_string())
}

fn now() -> i64 {
    now_epoch_ms()
}

fn next_id() -> u64 {
    let mut last = LAST_ID.lock().unwrap_or_else(PoisonError::into_inner);
    let candidate = u64::try_from(now()).unwrap_or(0);
    *last = candidate.max(*last + 1);
    *last
}

// ---- the record -----------------------------------------------------------

fn read_ledger(ctx: &Context) -> Ledger {
    std::fs::read_to_string(ctx.folder().join(LEDGER_FILE))
        .map(|text| Ledger::read(&text))
        .unwrap_or_else(|_| Ledger::new())
}

fn write_private(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    durable_file::replace_bytes(path, bytes)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

/// Replace a file the person or an agent owns, in place: a whole new copy
/// beside it takes its name, with the mode the file had (owner-only for a new
/// one — a list can hold a server's values). The folders on the way are left
/// as they are, and a file that is a link has its target replaced, so a
/// dotfiles link stays a link.
fn replace_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write as _;
    let path = match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => {
            canonical(path).ok_or("library.listUnreadable")?
        }
        _ => path.to_path_buf(),
    };
    let dir = path.parent().ok_or("library.listUnreadable")?;
    std::fs::create_dir_all(dir).map_err(|error| error.to_string())?;
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let staged = dir.join(format!(
        ".{name}.zerocode-{}-{}",
        std::process::id(),
        next_id()
    ));
    let written = (|| -> std::io::Result<()> {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        let mut file = options.open(&staged)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        if let Ok(meta) = std::fs::metadata(&path) {
            std::fs::set_permissions(&staged, meta.permissions())?;
        }
        std::fs::rename(&staged, &path)
    })();
    written.map_err(|error| {
        let _ = std::fs::remove_file(&staged);
        error.to_string()
    })
}

fn write_ledger(ctx: &Context, ledger: &Ledger) {
    if let Ok(text) = serde_json::to_vec_pretty(ledger)
        && let Err(error) = write_private(&ctx.folder().join(LEDGER_FILE), &text)
    {
        eprintln!("library ledger: {error}");
    }
}

/// One line of 「최근 변경」. Words are the window's to choose: the line
/// carries what happened, and the page says it in the person's language.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct LogLine {
    pub id: u64,
    pub at_ms: i64,
    /// `taken-in`, `edited`, `removed`, `asked`, `added`, `deleted`,
    /// `switched-off`, `switched-on`, `agent-off`, `agent-on`, `chosen`,
    /// `aligned`, `shared`, `linked`, `first-merge`, `failed`, `undone`.
    pub kind: String,
    /// `mcp` or `skill` (or `both` for the first merge).
    pub item: String,
    pub name: String,
    /// The agent it happened in, `person`, or `store`.
    pub who: String,
    #[serde(default)]
    pub count_skills: usize,
    #[serde(default)]
    pub count_servers: usize,
    #[serde(default)]
    pub count_asked: usize,
    /// An agent's refusal, credential-masked and cut short; empty otherwise.
    #[serde(default)]
    pub detail: String,
}

fn line(kind: &str, item: &str, name: &str, who: &str) -> LogLine {
    LogLine {
        id: next_id(),
        at_ms: now(),
        kind: kind.to_string(),
        item: item.to_string(),
        name: name.to_string(),
        who: who.to_string(),
        count_skills: 0,
        count_servers: 0,
        count_asked: 0,
        detail: String::new(),
    }
}

fn read_log(ctx: &Context) -> Vec<LogLine> {
    std::fs::read_to_string(ctx.folder().join(LOG_FILE))
        .ok()
        .and_then(|text| serde_json::from_str::<Vec<LogLine>>(&text).ok())
        .unwrap_or_default()
}

fn append_log(ctx: &Context, mut lines: Vec<LogLine>) {
    if lines.is_empty() {
        return;
    }
    let mut kept = read_log(ctx);
    lines.reverse();
    lines.append(&mut kept);
    lines.truncate(LOG_KEEP);
    if let Ok(text) = serde_json::to_vec_pretty(&lines)
        && let Err(error) = write_private(&ctx.folder().join(LOG_FILE), &text)
    {
        eprintln!("library log: {error}");
    }
}

fn masked(text: &str) -> String {
    let first = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default();
    let mut said = zerocode_core::credential::mask_values(first);
    if said.chars().count() > DETAIL_CHARS {
        said = said.chars().take(DETAIL_CHARS).collect::<String>() + "…";
    }
    said
}

// ---- reading ----------------------------------------------------------------

/// The parse of each `.claude.json` read in this burst, by path, with the
/// size and time it had. Claude Code rewrites that file for its own reasons
/// and it can run long: while its size and time stand, the stamp's read and
/// the look's read of one burst parse it once (the review of the skills
/// store, P3-4). It lives for that burst alone ([`in_one_burst`]) and on its
/// thread: a clock that ticks by the second (HFS+, FAT, a network share)
/// leaves a rewrite of the same size at the same time, which the next burst
/// reads anew (the review of v1.1.129's skills, t-98860, P3-4).
type Parsed = (u64, std::time::SystemTime, BTreeMap<String, McpEntry>);
thread_local! {
    static CLAUDE_PARSED: std::cell::RefCell<Option<BTreeMap<PathBuf, Parsed>>> =
        const { std::cell::RefCell::new(None) };
}

/// Run one burst's reads with [`CLAUDE_PARSED`] kept between them.
fn in_one_burst<T>(work: impl FnOnce() -> T) -> T {
    CLAUDE_PARSED.with(|parsed| *parsed.borrow_mut() = Some(BTreeMap::new()));
    let done = work();
    CLAUDE_PARSED.with(|parsed| *parsed.borrow_mut() = None);
    done
}

/// A list's servers: `Some(empty)` for a file that does not exist yet, `None`
/// for one that cannot be read — never written over. A `.claude.json` read
/// before in this burst, its size and time unchanged, is its parse of then
/// ([`CLAUDE_PARSED`]); only a read that succeeded is kept.
fn read_list(path: &Path, format: Format) -> Option<BTreeMap<String, McpEntry>> {
    if format != Format::Claude {
        return read_list_now(path, format);
    }
    let stamp = std::fs::metadata(path)
        .ok()
        .and_then(|meta| Some((meta.len(), meta.modified().ok()?)));
    let Some((len, at)) = stamp else {
        return read_list_now(path, format);
    };
    let held = CLAUDE_PARSED.with(|parsed| {
        let parsed = parsed.borrow();
        let (held_len, held_at, servers) = parsed.as_ref()?.get(path)?;
        ((*held_len, *held_at) == (len, at)).then(|| servers.clone())
    });
    if held.is_some() {
        return held;
    }
    let servers = read_list_now(path, format)?;
    CLAUDE_PARSED.with(|parsed| {
        if let Some(parsed) = parsed.borrow_mut().as_mut() {
            parsed.insert(path.to_path_buf(), (len, at, servers.clone()));
        }
    });
    Some(servers)
}

fn read_list_now(path: &Path, format: Format) -> Option<BTreeMap<String, McpEntry>> {
    match std::fs::read_to_string(path) {
        Ok(text) => mcp::read_text(format, &text),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Some(BTreeMap::new()),
        Err(_) => None,
    }
}

/// What one look read of the MCP lists, and which could not be read.
struct McpLook {
    view: McpView,
    unreadable: BTreeSet<String>,
}

fn read_mcp(ctx: &Context) -> Result<McpLook, String> {
    let store = read_list(&ctx.store_mcp(), Format::Store).ok_or("library.storeUnreadable")?;
    let mut lists = Vec::new();
    let mut unreadable = BTreeSet::new();
    for agent in &ctx.present {
        let Some(file) = ctx.mcp_file(agent) else {
            // Claude Code on the machine's own login: its list is the
            // person's, read below as their own Claude, never written.
            continue;
        };
        match read_list(&file, agent.format) {
            Some(servers) => lists.push(AgentList {
                agent: agent.id.to_string(),
                writable: true,
                servers,
            }),
            None => {
                unreadable.insert(agent.id.to_string());
            }
        }
    }
    if let Some(servers) = read_list(&ctx.home.join(MAC_CLAUDE_MCP), Format::Claude)
        && !servers.is_empty()
    {
        lists.push(AgentList {
            agent: MAC_CLAUDE.to_string(),
            writable: false,
            servers,
        });
    }
    if let Some(servers) = read_list(&ctx.codex_home.join("config.toml"), Format::Codex)
        && !servers.is_empty()
    {
        lists.push(AgentList {
            agent: CODEX_WINDOW.to_string(),
            writable: false,
            servers,
        });
    }
    Ok(McpLook {
        view: McpView { store, lists },
        unreadable,
    })
}

/// The digest a skill folder is compared by: its `SKILL.md` and the names and
/// sizes of what sits beside it, read to a bound.
fn skill_digest(dir: &Path, skill_md: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(skill_md);
    let mut entries: Vec<(String, u64)> = Vec::new();
    let mut pending = vec![(dir.to_path_buf(), 0usize)];
    while let Some((at, depth)) = pending.pop() {
        let Ok(read) = std::fs::read_dir(&at) else {
            continue;
        };
        for entry in read.flatten() {
            if entries.len() >= DIGEST_ENTRIES {
                break;
            }
            let path = entry.path();
            let relative = path
                .strip_prefix(dir)
                .map(|rest| rest.to_string_lossy().into_owned())
                .unwrap_or_default();
            match std::fs::metadata(&path) {
                Ok(meta) if meta.is_dir() && depth < 4 => pending.push((path, depth + 1)),
                Ok(meta) => entries.push((relative, meta.len())),
                Err(_) => {}
            }
        }
    }
    entries.sort();
    for (name, size) in entries {
        hasher.update(name.as_bytes());
        hasher.update(size.to_le_bytes());
    }
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// The files under `dir`, by path below it, to the digest's bounds.
fn digest_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut pending = vec![(dir.to_path_buf(), 0usize)];
    while let Some((at, depth)) = pending.pop() {
        let Ok(read) = std::fs::read_dir(&at) else {
            continue;
        };
        for entry in read.flatten() {
            if files.len() >= DIGEST_ENTRIES {
                break;
            }
            let path = entry.path();
            match std::fs::metadata(&path) {
                Ok(meta) if meta.is_dir() && depth < 4 => pending.push((path, depth + 1)),
                Ok(meta) if meta.is_file() => {
                    if let Ok(relative) = path.strip_prefix(dir) {
                        files.push(relative.to_path_buf());
                    }
                }
                _ => {}
            }
        }
    }
    files.sort();
    files
}

/// Whether every file under `copy` is byte for byte the one at its place in
/// `held` — asked only of two folders whose digests already agree, so the
/// two hold the same names at the same sizes.
fn same_bytes(held: &Path, copy: &Path) -> bool {
    digest_files(copy).iter().all(|relative| {
        match (
            std::fs::read(copy.join(relative)),
            std::fs::read(held.join(relative)),
        ) {
            (Ok(mine), Ok(theirs)) => mine == theirs,
            _ => false,
        }
    })
}

/// A digest of every file's path and text under `dir`, for a copy whose
/// sizes agree with the store's but whose text does not.
fn content_digest(dir: &Path) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(b"bytes");
    for relative in digest_files(dir) {
        hasher.update(relative.to_string_lossy().as_bytes());
        hasher.update(std::fs::read(dir.join(&relative)).unwrap_or_default());
    }
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// A skill folder's digest as [`skill_folders`] reads it; `None` without its
/// `SKILL.md`.
fn folder_digest(dir: &Path) -> Option<String> {
    let bytes = std::fs::read(dir.join(zerocode_core::skill::SKILL_FILE)).ok()?;
    Some(skill_digest(dir, &bytes))
}

fn canonical(path: &Path) -> Option<PathBuf> {
    std::fs::canonicalize(path).ok()
}

/// The checkout `real`, a link's target, reads into: the nearest folder at or
/// above it that holds `.git`, short of `home` — a home kept under git is no
/// package of skills.
fn checkout_of(real: &Path, home: &Path) -> Option<PathBuf> {
    real.ancestors()
        .take_while(|at| !home.starts_with(at))
        .find(|at| at.join(".git").exists())
        .map(Path::to_path_buf)
}

/// The skill folders directly under `dir`; `home` is the person's, canonical.
fn skill_folders(dir: &Path, store: Option<&Path>, home: &Path) -> Vec<SkillFolder> {
    let Ok(read) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let store = store.and_then(canonical);
    let mut folders = Vec::new();
    for entry in read.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !library::is_skill_name(&name) {
            continue;
        }
        let path = entry.path();
        let Ok(bytes) = std::fs::read(path.join(zerocode_core::skill::SKILL_FILE)) else {
            continue;
        };
        let linked =
            std::fs::symlink_metadata(&path).is_ok_and(|meta| meta.file_type().is_symlink());
        let real = if linked { canonical(&path) } else { None };
        let into_store = store
            .as_ref()
            .zip(real.as_ref())
            .is_some_and(|(store, real)| real.starts_with(store));
        let mut digest = skill_digest(&path, &bytes);
        // A digest reads sizes, not texts: a copy that agrees with the
        // store's by it is read byte for byte once, and one a person changed
        // a letter of — same size — is told apart, so it is asked about and
        // never taken for the store's (review t-106726, fourth).
        let held = store.as_ref().map(|store| store.join(&name));
        if let Some(held) = held.filter(|held| {
            !into_store
                && canonical(&path) != canonical(held)
                && folder_digest(held).as_deref() == Some(digest.as_str())
        }) && !same_bytes(&held, &path)
        {
            digest = content_digest(&path);
        }
        folders.push(SkillFolder {
            ours: zerocode_core::skill_install::ships_this_folder(&name, &path),
            digest,
            name,
            path,
            into_store,
            shared: false,
            checkout: real.as_deref().and_then(|real| checkout_of(real, home)),
        });
    }
    folders.sort_by(|a, b| a.name.cmp(&b.name));
    folders
}

/// What one look reads of skills; `None` when the store's folder is there and
/// cannot be read. That is not an empty store — read as one, every skill the
/// window remembers would be marked removed — so the skills step waits for a
/// look that can read it (the review of v1.1.128, P2-4).
///
/// A store link that reads nothing — a project moved for a while, its volume
/// let go — is the same for one skill: while the link stands, the store is
/// read as holding what the ledger last saw there (the review of v1.1.128,
/// P3-7). Removing the link is what removes the skill. The store's folder
/// itself a link that reads nothing is the same for every skill: `None`,
/// not an empty store (the review of v1.1.129's skills, t-98860, P3-5).
fn read_skills(ctx: &Context, ledger: &Ledger) -> Option<SkillsView> {
    let store_dir = ctx.store_skills();
    let read: Vec<std::fs::DirEntry> = match std::fs::read_dir(&store_dir) {
        Ok(read) => read.flatten().collect(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && is_link(&store_dir) => {
            eprintln!("library skills: the store's folder is a link that reads nothing");
            return None;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => {
            eprintln!("library skills: the store's folder cannot be read: {error}");
            return None;
        }
    };
    let home = canonical(&ctx.home).unwrap_or_else(|| ctx.home.clone());
    let mut store: BTreeMap<String, SkillFolder> = skill_folders(&store_dir, None, &home)
        .into_iter()
        .map(|folder| (folder.name.clone(), folder))
        .collect();
    for entry in read {
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = entry.path();
        let Some(record) = ledger.skills.get(&name) else {
            continue;
        };
        if store.contains_key(&name) || !is_link(&path) {
            continue;
        }
        store.insert(
            name.clone(),
            SkillFolder {
                name,
                path,
                digest: record.digest.clone(),
                into_store: false,
                ours: false,
                shared: false,
                checkout: None,
            },
        );
    }
    // The store's copy of a linked folder came from that folder's checkout.
    for (name, folder) in &mut store {
        if let Some(from) = ledger
            .skills
            .get(name)
            .and_then(|record| record.copied_from.as_deref())
            .map(Path::new)
            && is_link(from)
            && !is_link(&folder.path)
        {
            folder.checkout = canonical(from).and_then(|real| checkout_of(&real, &home));
        }
    }
    let mut places = Vec::new();
    for agent in &ctx.present {
        for dir in ctx.own_skill_dirs(agent) {
            if canonical(&dir).is_some_and(|real| Some(real) == canonical(&store_dir)) {
                continue;
            }
            places.push(SkillPlace {
                agent: agent.id.to_string(),
                // A skill leaves an agent's own folder for the store only when
                // the agent reads the store wherever it runs. zo reads it in
                // the window's terminals alone, so its folder keeps the skill
                // and the store gets a copy that follows its edits.
                sees_store: agent.skills.reads_the_store_everywhere(),
                writable: true,
                folders: skill_folders(&dir, Some(&store_dir), &home),
            });
        }
    }
    places.push(SkillPlace {
        agent: MAC_CLAUDE.to_string(),
        sees_store: false,
        writable: false,
        folders: skill_folders(&ctx.home.join(MAC_CLAUDE_SKILLS), Some(&store_dir), &home),
    });
    Some(SkillsView { store, places })
}

// ---- running an agent's own command -------------------------------------------

/// Run one agent command to its end or its budget, through the window's one
/// bounded runner ([`crate::resource_usage::run_bounded`]: both streams read
/// on threads of their own, a child past its budget killed and let go of).
/// Neither the arguments nor what the command printed reach a log: either may
/// carry a server's values.
fn run_command(
    ctx: &Context,
    binary: &str,
    argv: &[String],
    envs: &[(&str, &OsString)],
    budget: Duration,
) -> Result<String, String> {
    let mut command = crate::proc::quiet_command_on_path(binary, ctx.path.as_deref());
    command
        .args(argv)
        .current_dir(&ctx.home)
        .env("NO_COLOR", "1")
        .env("TERM", "dumb")
        // The window's own Codex home must not be what `codex mcp` writes:
        // it is a copy, rewritten from `~/.codex` at every launch.
        .env_remove("CODEX_HOME");
    for (name, value) in envs {
        command.env(name, value);
    }
    let started = Instant::now();
    let run = crate::resource_usage::run_bounded(command, budget).map_err(|_| {
        if started.elapsed() >= budget {
            "library.commandTimedOut".to_string()
        } else {
            "library.commandMissing".to_string()
        }
    })?;
    let out = String::from_utf8_lossy(&run.stdout.bytes).into_owned();
    if run.status.success() {
        return Ok(out);
    }
    let err = String::from_utf8_lossy(&run.stderr.bytes);
    Err(masked(if err.trim().is_empty() { &out } else { &err }))
}

/// A copy of an agent's file before the window first edits it in a look.
fn back_up(ctx: &Context, agent: &str, file: &Path, done: &mut BTreeSet<String>) {
    if !done.insert(agent.to_string()) || !file.is_file() {
        return;
    }
    if let Ok(bytes) = std::fs::read(file) {
        keep_backup(ctx, agent, file, &bytes);
    }
}

/// Keep `bytes`, what `file` held, under `backups/<folder>`, the newest
/// [`BACKUPS_KEEP`] of them.
fn keep_backup(ctx: &Context, folder: &str, file: &Path, bytes: &[u8]) {
    let dir = ctx.folder().join(BACKUPS).join(folder);
    let name = file
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "config".to_string());
    if write_private(&dir.join(format!("{}-{}-{name}", now(), next_id())), bytes).is_err() {
        return;
    }
    if let Ok(read) = std::fs::read_dir(&dir) {
        let mut kept: Vec<PathBuf> = read.flatten().map(|entry| entry.path()).collect();
        kept.sort();
        while kept.len() > BACKUPS_KEEP {
            let _ = std::fs::remove_file(kept.remove(0));
        }
    }
}

/// The store's list as it stood before a look or an action, kept once that
/// changed it: the store is every agent's source, so what a change took from
/// it can be had back from here (the review of v1.1.128, P1).
fn keep_store_before(ctx: &Context, before: Option<Vec<u8>>) {
    let Some(before) = before else {
        return;
    };
    let file = ctx.store_mcp();
    if std::fs::read(&file).ok().as_deref() != Some(before.as_slice()) {
        keep_backup(ctx, STORE_BACKUPS, &file, &before);
    }
}

/// Edit one JSON list: a server in, or out (`None`).
fn edit_json(
    file: &Path,
    format: Format,
    name: &str,
    entry: Option<&McpEntry>,
) -> Result<(), String> {
    let mut doc = match std::fs::read_to_string(file) {
        Ok(text) if text.trim().is_empty() => serde_json::Value::Object(serde_json::Map::new()),
        Ok(text) => {
            serde_json::from_str(&text).map_err(|_| "library.listUnreadable".to_string())?
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            serde_json::Value::Object(serde_json::Map::new())
        }
        Err(error) => return Err(error.to_string()),
    };
    if !doc.is_object() {
        return Err("library.listUnreadable".to_string());
    }
    if !mcp::set_json(&mut doc, format, name, entry) {
        return Ok(());
    }
    let text = serde_json::to_string_pretty(&doc).map_err(|error| error.to_string())? + "\n";
    replace_file(file, text.as_bytes())
}

/// The environment one agent's command runs with.
fn agent_envs(ctx: &Context, agent: &LibraryAgent) -> Vec<(&'static str, OsString)> {
    match (agent.mcp, &ctx.claude_home) {
        (McpPlace::ClaudeWindow, Some(home)) => {
            vec![("CLAUDE_CONFIG_DIR", home.clone().into_os_string())]
        }
        _ => Vec::new(),
    }
}

fn add_server(
    ctx: &Context,
    agent: &LibraryAgent,
    file: &Path,
    name: &str,
    entry: &McpEntry,
) -> Result<(), String> {
    match mcp::add_command(agent.format, name, &entry.def) {
        Write::Command(argv) => {
            let envs = agent_envs(ctx, agent);
            let envs: Vec<(&str, &OsString)> = envs.iter().map(|(k, v)| (*k, v)).collect();
            run_command(ctx, agent.binary, &argv, &envs, CLI_BUDGET)?;
            if agent.format == Format::Codex
                && let Some(table) = mcp::codex_headers(name, &entry.def)
            {
                let mut text = std::fs::read_to_string(file).map_err(|error| error.to_string())?;
                if !text.ends_with('\n') {
                    text.push('\n');
                }
                text.push_str(&table);
                replace_file(file, text.as_bytes())?;
            }
            Ok(())
        }
        Write::File => edit_json(file, agent.format, name, Some(entry)),
        Write::Unsupported(what) => Err(format!("library.unsupported.{what}")),
    }
}

fn remove_server(
    ctx: &Context,
    agent: &LibraryAgent,
    file: &Path,
    name: &str,
) -> Result<(), String> {
    match mcp::remove_command(agent.format, name) {
        Write::Command(argv) => {
            let envs = agent_envs(ctx, agent);
            let envs: Vec<(&str, &OsString)> = envs.iter().map(|(k, v)| (*k, v)).collect();
            run_command(ctx, agent.binary, &argv, &envs, CLI_BUDGET).map(|_| ())
        }
        Write::File => edit_json(file, agent.format, name, None),
        Write::Unsupported(what) => Err(format!("library.unsupported.{what}")),
    }
}

fn store_put(ctx: &Context, name: &str, entry: Option<&McpEntry>) -> Result<(), String> {
    let file = ctx.store_mcp();
    edit_json(&file, Format::Store, name, entry)
}

/// Run the MCP steps; a write is recorded as seen only once the list, read
/// again, holds what was written. That read is the file as it stands, never
/// a parse this burst kept from before the write ([`CLAUDE_PARSED`]): a
/// rewrite at the same size within one tick of a coarse clock would read as
/// unwritten (the second review of v1.1.129's skills, t-99089, P3-D).
fn run_mcp(ctx: &Context, steps: &[McpStep], ledger: &mut Ledger, log: &mut Vec<LogLine>) -> usize {
    let mut backed_up = BTreeSet::new();
    let mut failures = 0;
    for step in steps {
        match step {
            McpStep::Store { name, entry } => {
                if let Err(error) = store_put(ctx, name, Some(entry)) {
                    failures += 1;
                    let mut said = line("failed", "mcp", name, "store");
                    said.detail = masked(&error);
                    log.push(said);
                }
            }
            McpStep::Unstore { name } => {
                if let Err(error) = store_put(ctx, name, None) {
                    failures += 1;
                    let mut said = line("failed", "mcp", name, "store");
                    said.detail = masked(&error);
                    log.push(said);
                }
            }
            McpStep::Push { agent, name, entry } => {
                let Some(known) = AGENTS.iter().find(|known| known.id == agent.as_str()) else {
                    continue;
                };
                let Some(file) = ctx.mcp_file(known) else {
                    continue;
                };
                back_up(ctx, agent, &file, &mut backed_up);
                let held = read_list(&file, known.format).unwrap_or_default();
                let mut result = Ok(());
                if held.contains_key(name) {
                    result = remove_server(ctx, known, &file, name);
                }
                if result.is_ok() {
                    result = add_server(ctx, known, &file, name, entry);
                }
                let written = read_list_now(&file, known.format)
                    .and_then(|servers| servers.get(name).map(|held| held.def.fingerprint()));
                match (result, written) {
                    (Ok(()), Some(fingerprint)) if fingerprint == entry.def.fingerprint() => {
                        ledger
                            .mcp
                            .entry(name.clone())
                            .or_default()
                            .seen
                            .insert(agent.clone(), fingerprint);
                    }
                    (outcome, _) => {
                        failures += 1;
                        let mut said = line("failed", "mcp", name, agent);
                        said.detail =
                            masked(&outcome.err().unwrap_or_else(|| "library.notWritten".into()));
                        log.push(said);
                    }
                }
            }
            McpStep::Pull { agent, name } => {
                let Some(known) = AGENTS.iter().find(|known| known.id == agent.as_str()) else {
                    continue;
                };
                let Some(file) = ctx.mcp_file(known) else {
                    continue;
                };
                back_up(ctx, agent, &file, &mut backed_up);
                let result = remove_server(ctx, known, &file, name);
                let gone = read_list_now(&file, known.format)
                    .is_some_and(|servers| !servers.contains_key(name));
                if gone {
                    if let Some(record) = ledger.mcp.get_mut(name) {
                        record.seen.remove(agent);
                    }
                } else {
                    failures += 1;
                    let mut said = line("failed", "mcp", name, agent);
                    said.detail =
                        masked(&result.err().unwrap_or_else(|| "library.notRemoved".into()));
                    log.push(said);
                }
            }
        }
    }
    failures
}

// ---- skills -------------------------------------------------------------------

fn is_link(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink())
}

/// Copy a skill folder whole, once it is known to be a skill's size: every
/// entry is counted first, without following a link, and a folder past
/// [`SKILL_COPY_ENTRIES`] or [`SKILL_COPY_BYTES`] is refused before a byte
/// is written. On Windows a link to a file counts at the size of what it
/// reads: it may be copied as that file ([`copy_link`], the review of
/// v1.1.129's skills, t-98860, P3-3).
fn copy_skill_folder(from: &Path, to: &Path) -> Result<(), String> {
    let mut entries = 0usize;
    let mut bytes = 0u64;
    let mut pending = vec![from.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir)
            .map_err(|error| error.to_string())?
            .flatten()
        {
            let kind = entry.file_type().map_err(|error| error.to_string())?;
            entries += 1;
            if kind.is_dir() {
                pending.push(entry.path());
            } else if kind.is_file() {
                bytes += entry.metadata().map_or(0, |meta| meta.len());
            } else if kind.is_symlink() && cfg!(not(unix)) {
                bytes += std::fs::metadata(entry.path())
                    .ok()
                    .filter(std::fs::Metadata::is_file)
                    .map_or(0, |meta| meta.len());
            }
            if entries > SKILL_COPY_ENTRIES || bytes > SKILL_COPY_BYTES {
                return Err("library.skillTooLarge".to_string());
            }
        }
    }
    copy_tree(from, from, to)
}

/// A link inside a skill is copied as a link and never followed: one that
/// points back at its own folder would otherwise copy forever.
fn copy_tree(root: &Path, from: &Path, to: &Path) -> Result<(), String> {
    std::fs::create_dir_all(to).map_err(|error| error.to_string())?;
    for entry in std::fs::read_dir(from)
        .map_err(|error| error.to_string())?
        .flatten()
    {
        let kind = entry.file_type().map_err(|error| error.to_string())?;
        let (path, target) = (entry.path(), to.join(entry.file_name()));
        if kind.is_symlink() {
            copy_link(root, &path, &target)?;
        } else if kind.is_dir() {
            copy_tree(root, &path, &target)?;
        } else {
            std::fs::copy(&path, &target).map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

/// What a link copied out of a skill is written to read. One that stays
/// inside the skill, or reaches a sibling skill the store holds beside it
/// (`ref -> ../b/lib`, the review of skills-p2-r2, P3-4), reads as it was
/// written; one that reaches anywhere else (`common -> ../_common`) would find
/// nothing from the store, so it reads the whole path of what it reached (the
/// review of 4ca485472, P2).
fn link_target(root: &Path, from: &Path) -> Result<PathBuf, String> {
    let written = std::fs::read_link(from).map_err(|error| error.to_string())?;
    let reached = from
        .parent()
        .map(|dir| dir.join(&written))
        .and_then(|at| canonical(&at));
    let found_from_store = match (&reached, canonical(root)) {
        (Some(reached), Some(root)) => {
            reached.starts_with(&root) || in_a_sibling_skill(&root, reached)
        }
        _ => true,
    };
    Ok(match reached {
        Some(reached) if written.is_relative() && !found_from_store => reached,
        _ => written,
    })
}

/// Whether `reached` lies in another skill beside the skill at `root` — one
/// the keeper takes into the store too, under the same name. Both are whole
/// paths.
fn in_a_sibling_skill(root: &Path, reached: &Path) -> bool {
    let Some(skills) = root.parent() else {
        return false;
    };
    let Some(std::path::Component::Normal(name)) = reached
        .strip_prefix(skills)
        .ok()
        .and_then(|rest| rest.components().next())
    else {
        return false;
    };
    library::is_skill_name(&name.to_string_lossy())
        && skills
            .join(name)
            .join(zerocode_core::skill::SKILL_FILE)
            .is_file()
}

/// A link inside a skill is copied as a link ([`link_target`]).
#[cfg(unix)]
fn copy_link(root: &Path, from: &Path, to: &Path) -> Result<(), String> {
    std::os::unix::fs::symlink(link_target(root, from)?, to).map_err(|error| error.to_string())
}

/// Windows makes a link only with a privilege it rarely grants (Developer
/// Mode grants it). Without it, a link to a folder that reads a whole path is
/// a junction to the whole path it reached ([`link_folder`],
/// [`junction_reads`]), and a
/// link to a file is copied as the file it reads, counted toward the copy's
/// bound ([`copy_skill_folder`]) (the review of v1.1.129's skills, t-98860,
/// P3-3). A folder link not even a junction can make stops the copy with a
/// line that says why — never left out without a word (the review of
/// skills-p2, P3-6). A link that reads nothing carries nothing.
#[cfg(not(unix))]
fn copy_link(root: &Path, from: &Path, to: &Path) -> Result<(), String> {
    let target = link_target(root, from)?;
    let made = if from.is_dir() {
        std::os::windows::fs::symlink_dir(&target, to)
    } else {
        std::os::windows::fs::symlink_file(&target, to)
    };
    if made.is_ok() || !from.exists() {
        return Ok(());
    }
    if from.is_dir() {
        let reached = junction_reads(&target, from)?;
        return link_folder(&reached, to).map_err(|_| "library.linkNotCopied".to_string());
    }
    std::fs::copy(from, to)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

/// The whole path a junction made for the folder link `from` reads, where
/// [`link_target`] said the copy reads `target`. A junction holds a whole
/// path only, so one for a link that reads inside the skill or a sibling
/// skill (`target` relative) would bind the store's copy to the folder it
/// came from, which the move takes away: that link is refused, as before
/// junctions stood in (the second review of v1.1.129's skills, t-99089,
/// P3-B).
#[cfg(any(not(unix), test))]
fn junction_reads(target: &Path, from: &Path) -> Result<PathBuf, String> {
    if target.is_relative() {
        return Err("library.linkNotCopied".to_string());
    }
    canonical(from).ok_or_else(|| "library.linkNotCopied".to_string())
}

/// A skill folder that is itself a link — a project someone writes a skill
/// in — is put into the store as a link to that same folder, by its whole
/// path, so a link relative to the folder it stood in still finds it. On
/// Windows that link may be a junction ([`link_folder`]); where neither can
/// be made, the folder it points at is copied, to the same bound, and the
/// agent's link stays where it was ([`kept_folders`]), so the folder it reads
/// stays kept (the review of skills-p2-r3, P3-3).
fn link_skill_folder(from: &Path, to: &Path) -> Result<(), String> {
    let real = canonical(from).ok_or("library.noSkillFile")?;
    let linked = link_folder(&real, to);
    if cfg!(unix) || linked.is_ok() {
        return linked;
    }
    copy_skill_folder(from, to)
}

/// Take `path` away: a folder to the Trash; a link — it holds no data, and a
/// Trash might follow it (a junction on Windows) — removed itself, what it
/// read kept for 「되돌리기」 (the review of v1.1.129's skills, t-98860,
/// P3-8).
fn put_away(path: &Path) -> Result<Removed, String> {
    if !is_link(path) {
        return file_tree_io::trash_path(path).map(Removed::Trashed);
    }
    let reads = std::fs::read_link(path).map_err(|error| error.to_string())?;
    std::fs::remove_file(path)
        .or_else(|_| std::fs::remove_dir(path))
        .map_err(|error| error.to_string())?;
    Ok(Removed::Link {
        at: path.to_path_buf(),
        reads,
    })
}

/// Make again a link [`put_away`] removed. A unix link reads a file or a
/// folder alike, as it was written.
#[cfg(unix)]
fn link_back(reads: &Path, at: &Path) -> Result<(), String> {
    link_folder(reads, at)
}

/// Windows: a folder by its whole path ([`link_folder`], a junction where a
/// link is not allowed), a file by a link as it was written.
#[cfg(not(unix))]
fn link_back(reads: &Path, at: &Path) -> Result<(), String> {
    let whole = at
        .parent()
        .map_or_else(|| reads.to_path_buf(), |dir| dir.join(reads));
    if whole.is_dir() {
        return link_folder(&whole, at);
    }
    std::os::windows::fs::symlink_file(reads, at).map_err(|error| error.to_string())
}

/// Copy a skill folder into the store under `name`, replacing the store's:
/// the copy is made beside it first, so a copy that stops part way leaves the
/// store's folder as it was.
fn copy_into_store(ctx: &Context, from: &Path, name: &str) -> Result<Option<Removed>, String> {
    let store = ctx.store_skills();
    std::fs::create_dir_all(&store).map_err(|error| error.to_string())?;
    let incoming = store.join(format!(".zerocode-incoming-{name}"));
    // `remove_dir_all` never follows a link: a staged link goes, not the
    // folder it points at.
    let _ = std::fs::remove_dir_all(&incoming);
    let made = if is_link(from) {
        link_skill_folder(from, &incoming)
    } else {
        copy_skill_folder(from, &incoming)
    };
    made.inspect_err(|_| {
        let _ = std::fs::remove_dir_all(&incoming);
    })?;
    let target = store.join(name);
    // A link goes itself, never what it points at ([`put_away`]).
    let receipt = if std::fs::symlink_metadata(&target).is_ok() {
        Some(put_away(&target).inspect_err(|_| {
            let _ = std::fs::remove_dir_all(&incoming);
        })?)
    } else {
        None
    };
    std::fs::rename(&incoming, &target).map_err(|error| error.to_string())?;
    Ok(receipt)
}

/// Whether `path` holds what the store's `held` folder holds: the same text,
/// byte for byte, or, whole, a copy this build or an earlier release ships,
/// whose store copy is the one kept — not one a person added a file to or
/// changed a letter of.
fn duplicates(held: &Path, path: &Path, name: &str) -> bool {
    let Some(store_text) = folder_digest(held) else {
        return false;
    };
    let Ok(bytes) = std::fs::read(path.join(zerocode_core::skill::SKILL_FILE)) else {
        return false;
    };
    (skill_digest(path, &bytes) == store_text && same_bytes(held, path))
        || zerocode_core::skill_install::ships_this_folder(name, path)
}

/// Where a folder's own entry stands, by its whole path: a link is an entry
/// of its own, apart from the folder it reads.
fn entry_of(path: &Path) -> Option<PathBuf> {
    Some(canonical(path.parent()?)?.join(path.file_name()?))
}

/// The most links one link is followed through ([`links_passed`]); a loop
/// of links ends here too.
const LINK_HOPS: usize = 40;

/// The entries of the links `path` reads through on its way to a folder,
/// past its own: `~/.claude/skills/bar -> ~/.codex/skills/foo -> ~/proj/foo`
/// passes `~/.codex/skills/foo`. A link on the way above where a link points
/// is passed too: `~/.claude/skills/bar -> ~/.codex/skills/foo/inner` passes
/// `~/.codex/skills/foo` when that is a link (the second review of v1.1.129's
/// skills, t-99089, P3-A). Followed one link at a time, to [`LINK_HOPS`].
fn links_passed(path: &Path) -> Vec<PathBuf> {
    let mut passed = Vec::new();
    let mut at = path.to_path_buf();
    for _ in 0..LINK_HOPS {
        let Ok(written) = std::fs::read_link(&at) else {
            break;
        };
        let next = match at.parent() {
            Some(dir) => dir.join(written),
            None => written,
        };
        passed.extend(
            next.ancestors()
                .filter(|step| is_link(step))
                .filter_map(entry_of),
        );
        at = next;
    }
    passed
}

/// What the keeper neither sends to the Trash nor moves, by whole paths:
/// what a link reads — an agent's, the person's, the store's own — an entry
/// two places see as one (a skills folder linked whole, `~/.codex/skills ->
/// ~/.claude/skills`), and every entry in the person's own Claude skills,
/// which the window never writes (the reviews of 74874b969 and before).
fn kept_folders(ctx: &Context, view: &SkillsView) -> BTreeSet<PathBuf> {
    let claude = canonical(&ctx.home.join(MAC_CLAUDE_SKILLS));
    let mut kept: BTreeSet<PathBuf> = view
        .places
        .iter()
        .flat_map(|place| &place.folders)
        .chain(view.store.values())
        .filter(|folder| is_link(&folder.path))
        .filter_map(|folder| canonical(&folder.path))
        .collect();
    // A link that reads a link keeps every link on its way too (the review of
    // v1.1.129's skills, t-98860, P2-2): moved, `~/.codex/skills/foo` would
    // leave `~/.claude/skills/bar -> ~/.codex/skills/foo` reading nothing.
    let passed: Vec<PathBuf> = view
        .places
        .iter()
        .flat_map(|place| &place.folders)
        .chain(view.store.values())
        .flat_map(|folder| links_passed(&folder.path))
        .collect();
    kept.extend(passed);
    let mut seen: BTreeMap<PathBuf, usize> = BTreeMap::new();
    for folder in view.places.iter().flat_map(|place| &place.folders) {
        let Some(entry) = entry_of(&folder.path) else {
            continue;
        };
        if claude
            .as_ref()
            .is_some_and(|claude| entry.starts_with(claude))
        {
            kept.insert(entry.clone());
        }
        *seen.entry(entry).or_default() += 1;
    }
    kept.extend(
        seen.into_iter()
            .filter(|(_, places)| *places > 1)
            .map(|(entry, _)| entry),
    );
    // Windows without the link privilege: the store holds a copy of what an
    // agent's link reads, so that link is all that says another place reads
    // the folder. It stays, and is followed (the review of skills-p2-r3,
    // P3-3); a store that could link keeps the folder by its own link.
    #[cfg(not(unix))]
    kept.extend(
        view.places
            .iter()
            .flat_map(|place| &place.folders)
            .filter(|folder| is_link(&folder.path))
            .filter(|folder| {
                view.store
                    .get(&folder.name)
                    .is_some_and(|held| !is_link(&held.path))
            })
            .filter_map(|folder| entry_of(&folder.path)),
    );
    kept
}

/// Whether the folder at `path` is one [`kept_folders`] keeps, or holds one:
/// moved or sent to the Trash, a folder takes what is inside it along
/// (`~/.claude/skills/bar -> ~/.codex/skills/foo/inner` reads into Codex's
/// `foo`). A folder whose whole path cannot be read is kept too — nothing
/// says another place does not read it (the review of 94e1edaf7, P3-1·P3-2).
/// A link is judged by its own entry: removing it leaves what it reads, so
/// a link in an agent that reads the store moves, or goes as a duplicate,
/// like any folder there (the review of cdf998355, P3-3).
///
/// What it cannot know: a link no look reads — a project's own
/// `.claude/skills/foo -> ~/.codex/skills/foo` — finds the folder in the
/// store, or in the Trash, which gives it back.
fn is_kept(kept: &BTreeSet<PathBuf>, path: &Path) -> bool {
    if is_link(path) {
        return entry_of(path).is_none_or(|entry| kept.contains(&entry));
    }
    canonical(path).is_none_or(|real| kept.iter().any(|held| held.starts_with(&real)))
}

/// Run the skill steps. `kept` is [`kept_folders`]: none of them is sent to
/// the Trash, and one a step would move is copied instead. The planner was
/// told the same ([`SkillFolder::shared`]); this is the second guard.
fn run_skills(
    ctx: &Context,
    steps: &[SkillStep],
    kept: &BTreeSet<PathBuf>,
    ledger: &mut Ledger,
    log: &mut Vec<LogLine>,
) -> usize {
    let store = ctx.store_skills();
    let mut failures = 0;
    for step in steps {
        let (outcome, name, who) = match step {
            SkillStep::Move { agent, from, name } => {
                let target = store.join(name);
                let outcome = std::fs::create_dir_all(&store)
                    .map_err(|error| error.to_string())
                    .and_then(|()| {
                        if std::fs::symlink_metadata(&target).is_ok() {
                            return Err("library.skillExists".to_string());
                        }
                        if is_kept(kept, from) {
                            // Another place reads this folder: the store
                            // takes a copy and the folder stays (t-97954).
                            return copy_into_store(ctx, from, name).map(|_| ());
                        }
                        if is_link(from) {
                            // A link moves as a link to the same folder by its
                            // whole path: renamed, a relative one would point
                            // at nothing from the store.
                            // Where the store could only take a copy (Windows
                            // without the link privilege), the agent's link
                            // stays, and with it what it reads.
                            return copy_into_store(ctx, from, name).and_then(|_| {
                                if !is_link(&target) {
                                    return Ok(());
                                }
                                std::fs::remove_file(from)
                                    .or_else(|_| std::fs::remove_dir(from))
                                    .map_err(|error| error.to_string())
                            });
                        }
                        match std::fs::rename(from, &target) {
                            Ok(()) => Ok(()),
                            // Another volume: a copy, then the original to
                            // the Trash.
                            Err(_) => copy_into_store(ctx, from, name)
                                .and_then(|_| put_away(from).map(|_| ())),
                        }
                    });
                (outcome, name, agent)
            }
            SkillStep::Copy { agent, from, name } => {
                (copy_into_store(ctx, from, name).map(|_| ()), name, agent)
            }
            SkillStep::Trash { agent, path, name } => {
                // A duplicate goes only while the store holds the same text,
                // and never a kept folder or the one the store's link reads
                // (the reviews of 4ca485472, P1; t-97910; t-97954):
                // sent to the Trash, the skill would go with it. A copy into
                // the store that failed leaves nothing this folder duplicates.
                // A link goes alone, so one reading the store's folder goes.
                let held = store.join(name);
                let read_through_a_link = is_kept(kept, path)
                    || (!is_link(path)
                        && canonical(path).is_some_and(|real| {
                            canonical(&held).is_some_and(|stored| stored == real)
                        }));
                let outcome = if read_through_a_link || !duplicates(&held, path, name) {
                    eprintln!("library skills: {name} at {agent} left in place");
                    Ok(())
                } else {
                    put_away(path).map(|_| ())
                };
                (outcome, name, agent)
            }
        };
        if let Err(error) = outcome {
            failures += 1;
            // An import that did not happen is not remembered, or the next
            // look would read the missing folder as a removal.
            if matches!(step, SkillStep::Move { .. } | SkillStep::Copy { .. })
                && !store.join(name).exists()
            {
                ledger.skills.remove(name);
            } else if matches!(step, SkillStep::Copy { .. })
                && let Some(held) = folder_digest(&store.join(name))
                && let Some(record) = ledger.skills.get_mut(name)
            {
                // A follow that failed left the store's text as it was: the
                // record says so, so the next look follows again rather than
                // read the store as the person's edit (the review of
                // v1.1.128, P3-1).
                record.digest.clone_from(&held);
                record.source_digest = Some(held);
            }
            let mut said = line("failed", "skill", name, who);
            said.detail = masked(&error);
            log.push(said);
        }
    }
    failures
}

/// Claude Code reads only its config home's `skills` (its skills page): the
/// window's own home has that folder be the store. What stood in it moves into
/// the store first; a different text under a store skill's name is held and
/// asked about; the claude.ai account's `synced` folder stays out of the store,
/// in the window's home, reached through a link the account can keep writing
/// through. On Windows the link is a junction where a link is not allowed
/// ([`link_folder`]), so the window's Claude reads the store there too (the
/// review of the skills store, P3-4). The link is made first, beside the
/// folder ([`CLAUDE_LINKING`]), and only once it stands does anything move:
/// where neither can be made, the window's Claude keeps reading its own
/// skills (the review of v1.1.129's skills, t-98860, P3-2).
fn link_claude_home(ctx: &Context, ledger: &mut Ledger, log: &mut Vec<LogLine>) {
    let Some(home) = &ctx.claude_home else {
        return;
    };
    let skills = home.join("skills");
    let store = ctx.store_skills();
    if std::fs::create_dir_all(&store).is_err() {
        return;
    }
    let synced_home = home.join("skills-synced");
    match std::fs::symlink_metadata(&skills) {
        Ok(meta) if meta.file_type().is_symlink() => {
            // Linked already — ours, or somebody else's link, which stays.
        }
        Ok(meta) if meta.is_dir() => {
            let held = ctx.folder().join(HELD).join("claude");
            let Ok(read) = std::fs::read_dir(&skills) else {
                return;
            };
            let linking = home.join(CLAUDE_LINKING);
            if !link_for_claude(ctx, &store, &linking, log) {
                return;
            }
            for entry in read.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                let path = entry.path();
                if name == library::CLAUDE_SYNCED {
                    if !synced_home.exists() && std::fs::rename(&path, &synced_home).is_err() {
                        return;
                    }
                    continue;
                }
                let target = store.join(&name);
                let skill_md = std::fs::read(path.join(zerocode_core::skill::SKILL_FILE)).ok();
                let moved = match (&skill_md, std::fs::symlink_metadata(&target).is_ok()) {
                    (Some(_), false) if library::is_skill_name(&name) => {
                        std::fs::rename(&path, &target).is_ok()
                    }
                    (Some(bytes), true) => {
                        let theirs = skill_digest(&path, bytes);
                        let store_bytes =
                            std::fs::read(target.join(zerocode_core::skill::SKILL_FILE))
                                .unwrap_or_default();
                        let same = skill_digest(&target, &store_bytes) == theirs;
                        if same || zerocode_core::skill_install::ships_this_folder(&name, &path) {
                            put_away(&path).is_ok()
                        } else {
                            let keep = held.join(&name);
                            let _ = std::fs::create_dir_all(&held);
                            let moved = std::fs::rename(&path, &keep).is_ok();
                            if moved {
                                let record = ledger.skills.entry(name.clone()).or_default();
                                record.pick.push(SkillCandidate {
                                    agent: "claude".to_string(),
                                    path: keep.to_string_lossy().into_owned(),
                                    digest: theirs,
                                });
                            }
                            moved
                        }
                    }
                    _ => {
                        let _ = std::fs::create_dir_all(&held);
                        std::fs::rename(&path, held.join(&name)).is_ok()
                    }
                };
                if !moved {
                    return;
                }
            }
            if std::fs::remove_dir(&skills).is_err() {
                return;
            }
            if std::fs::rename(&linking, &skills).is_err() {
                // Put an empty folder back rather than leave Claude none.
                let _ = std::fs::create_dir_all(&skills);
                return;
            }
            log.push(line("linked", "skill", "", "claude"));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if link_for_claude(ctx, &store, &skills, log) {
                log.push(line("linked", "skill", "", "claude"));
            }
        }
        _ => {}
    }
    // The account's skills: a link inside the store to the window's home, so
    // Claude Code's writes through `skills/synced` land outside the store.
    let synced_link = store.join(library::CLAUDE_SYNCED);
    if synced_home.is_dir() && std::fs::symlink_metadata(&synced_link).is_err() {
        let _ = link_folder(&synced_home, &synced_link);
    }
}

/// Make the link at `at` the window's Claude reads the store through. One
/// left by a look that stopped part way is used as it is when it reads the
/// store, and made again when it does not — so a move that keeps failing
/// does not make the link on every look (on Windows, a `cmd` each time; the
/// second review of v1.1.129's skills, t-99089, P3-C). Where it cannot be
/// made — a drive that takes neither a link nor a junction — the line says so
/// once and [`CLAUDE_UNLINKED`] keeps later looks from trying again for
/// [`CLAUDE_RELINK_AFTER`].
fn link_for_claude(ctx: &Context, store: &Path, at: &Path, log: &mut Vec<LogLine>) -> bool {
    let refused = ctx.folder().join(CLAUDE_UNLINKED);
    let refusal = std::fs::symlink_metadata(&refused).ok();
    let refused_lately = refusal.as_ref().is_some_and(|meta| {
        meta.modified()
            .ok()
            .and_then(|at| at.elapsed().ok())
            .is_none_or(|age| age < CLAUDE_RELINK_AFTER)
    });
    if refused_lately {
        return false;
    }
    let reads_store =
        is_link(at) && canonical(at).is_some_and(|reads| canonical(store) == Some(reads));
    if !reads_store && is_link(at) {
        let _ = std::fs::remove_file(at).or_else(|_| std::fs::remove_dir(at));
    }
    if reads_store || link_folder(store, at).is_ok() {
        let _ = std::fs::remove_file(&refused);
        return true;
    }
    let _ = write_private(&refused, b"");
    if refusal.is_none() {
        let mut said = line("failed", "skill", "", "claude");
        said.detail = "library.linkNotMade".to_string();
        log.push(said);
    }
    false
}

/// A link at `link` to the folder `target`, a whole path.
#[cfg(unix)]
fn link_folder(target: &Path, link: &Path) -> Result<(), String> {
    std::os::unix::fs::symlink(target, link).map_err(|error| error.to_string())
}

/// Windows: a link where Developer Mode allows one, else a junction, which
/// asks for no privilege — `mklink /J`, a command of `cmd` itself, handed
/// the line [`junction_line`] writes. Either reads as a link ([`is_link`])
/// and is removed alone.
#[cfg(not(unix))]
fn link_folder(target: &Path, link: &Path) -> Result<(), String> {
    use std::os::windows::process::CommandExt as _;
    if std::os::windows::fs::symlink_dir(target, link).is_ok() {
        return Ok(());
    }
    // `mklink` reads a drive path, not the `\\?\` spelling canonicalize gives.
    let line = junction_line(dunce::simplified(target), dunce::simplified(link))?;
    let made = crate::proc::quiet_command("cmd")
        .raw_arg(line)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map_err(|error| error.to_string())?;
    if made.success() && is_link(link) {
        Ok(())
    } else {
        Err("library.linkNotMade".to_string())
    }
}

/// The line `cmd` is handed to make a junction at `link` that reads
/// `target`, each path in quotes of its own: `std` quotes an argument only
/// when it holds a space, and outside quotes `cmd` reads `&`, `|` and `^` as
/// its own. Inside them it still reads `%name%`, and a `"` would end them, so
/// a path holding either — or a control character, or what is not text — is
/// refused before `cmd` starts (the review of v1.1.129's skills, t-98860,
/// P2-3). `/D` skips the AutoRun command a registry may name, and `/V:OFF`
/// the delayed expansion one may turn on, which reads `!name!` even inside
/// quotes (the second review, t-99089, P3-E).
#[cfg(any(not(unix), test))]
fn junction_line(target: &Path, link: &Path) -> Result<String, String> {
    let quoted = |path: &Path| {
        path.to_str()
            .filter(|text| {
                !text
                    .chars()
                    .any(|at| at == '"' || at == '%' || at.is_control())
            })
            .map(|text| format!("\"{text}\""))
            .ok_or_else(|| "library.linkNotMade".to_string())
    };
    Ok(format!(
        "/D /V:OFF /C mklink /J {} {}",
        quoted(link)?,
        quoted(target)?
    ))
}

/// Whether the window's own Claude home reads the store.
fn claude_reads_store(ctx: &Context) -> bool {
    ctx.claude_home.as_ref().is_some_and(|home| {
        let skills = home.join("skills");
        std::fs::symlink_metadata(&skills).is_ok_and(|meta| meta.file_type().is_symlink())
            && canonical(&skills) == canonical(&ctx.store_skills())
    })
}

// ---- one look -------------------------------------------------------------------

/// What one look did.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct Looked {
    pub changed: bool,
}

fn note_lines(notes: &[Note], item: &str) -> Vec<LogLine> {
    notes
        .iter()
        .map(|note| match note {
            Note::TakenIn { name, from } => line("taken-in", item, name, from),
            Note::Edited { name, at } => line("edited", item, name, at),
            Note::Removed { name, at } => line("removed", item, name, at),
            Note::Asked { name } => line("asked", item, name, "store"),
            Note::Reset { at } => line("reset", item, "", at),
        })
        .collect()
}

/// One look: read, plan, do, record. Never two at once.
pub(crate) fn look(ctx: &Context) -> Result<Looked, String> {
    let _held = RUN.lock().unwrap_or_else(PoisonError::into_inner);
    RUNNING.store(true, std::sync::atomic::Ordering::Relaxed);
    let result = look_held(ctx);
    RUNNING.store(false, std::sync::atomic::Ordering::Relaxed);
    result
}

fn look_held(ctx: &Context) -> Result<Looked, String> {
    let before = read_ledger(ctx);
    if before.paused {
        return Ok(Looked::default());
    }
    let mut ledger = before.clone();
    let first = ledger.merged_at.is_none();
    let started = now();
    let mut log = Vec::new();
    link_claude_home(ctx, &mut ledger, &mut log);

    let mcp_look = read_mcp(ctx)?;
    let mcp_plan = plan::plan_mcp(&mcp_look.view, &ledger, started);
    ledger = mcp_plan.ledger.clone();
    let mut mcp_notes = note_lines(&mcp_plan.notes, "mcp");
    // A server taken in can be kept with the agents that held it again —
    // every list holding it before this look wrote anything.
    for (said, note) in mcp_notes.iter().zip(&mcp_plan.notes) {
        match note {
            Note::TakenIn { name, .. } => {
                remember(
                    said.id,
                    Undo::TakenIn {
                        name: name.clone(),
                        holders: mcp_look
                            .view
                            .lists
                            .iter()
                            .filter(|list| list.servers.contains_key(name))
                            .map(|list| list.agent.clone())
                            .collect(),
                    },
                );
            }
            Note::Removed { name, .. } => {
                if let Some(entry) = mcp_look.view.store.get(name) {
                    remember(
                        said.id,
                        Undo::Server {
                            name: name.clone(),
                            entry: entry.clone(),
                        },
                    );
                }
            }
            _ => {}
        }
    }
    let store_before = std::fs::read(ctx.store_mcp()).ok();
    run_mcp(ctx, &mcp_plan.steps, &mut ledger, &mut log);
    keep_store_before(ctx, store_before);

    let mut skill_steps = false;
    let mut skill_notes = Vec::new();
    if let Some(mut skills_view) = read_skills(ctx, &ledger) {
        ledger.keep_left_copies_that_stand(|path| std::fs::symlink_metadata(path).is_ok());
        let kept = kept_folders(ctx, &skills_view);
        for folder in skills_view
            .places
            .iter_mut()
            .flat_map(|place| place.folders.iter_mut())
        {
            folder.shared = is_kept(&kept, &folder.path);
        }
        let skills_plan = plan::plan_skills(&skills_view, &ledger, started);
        ledger = skills_plan.ledger.clone();
        skill_notes = note_lines(&skills_plan.notes, "skill");
        run_skills(ctx, &skills_plan.steps, &kept, &mut ledger, &mut log);
        skill_steps = !skills_plan.steps.is_empty();
    }

    let changed = !mcp_plan.steps.is_empty()
        || skill_steps
        || !log.is_empty()
        || !mcp_notes.is_empty()
        || !skill_notes.is_empty();
    if first {
        ledger.merged_at = Some(started);
        let mut merged = line("first-merge", "both", "", "store");
        merged.count_servers = mcp_notes
            .iter()
            .filter(|said| said.kind == "taken-in")
            .count();
        merged.count_skills = skill_notes
            .iter()
            .filter(|said| said.kind == "taken-in")
            .count();
        merged.count_asked = mcp_notes
            .iter()
            .chain(skill_notes.iter())
            .filter(|said| said.kind == "asked")
            .count();
        log.push(merged);
        // The first look's own lines are one line, not dozens.
        mcp_notes.retain(|said| said.kind != "taken-in" && said.kind != "asked");
    } else {
        log.extend(skill_notes.clone());
    }
    let mut lines = mcp_notes;
    lines.append(&mut log);
    if first {
        lines.extend(
            skill_notes
                .into_iter()
                .filter(|said| said.kind == "removed"),
        );
    }
    // A look that found everything in step leaves the disk alone.
    if ledger != before {
        write_ledger(ctx, &ledger);
    }
    append_log(ctx, lines);
    Ok(Looked { changed })
}

fn remember(id: u64, undo: Undo) {
    UNDO.lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(id, undo);
}

// ---- the background keeper ------------------------------------------------------

/// Ring the keeper: something it watches moved.
pub(crate) fn poke() {
    if let Some(bell) = POKE.get() {
        let _ = bell.send(());
    }
}

/// The paths the keeper watches, for the `library` lane.
fn watch_targets(ctx: &Context) -> Vec<(String, Option<PathBuf>)> {
    let mut paths = vec![ctx.store_skills(), ctx.store_mcp()];
    for agent in &ctx.present {
        paths.extend(ctx.own_skill_dirs(agent));
        if let Some(file) = ctx.mcp_file(agent) {
            paths.push(file);
        }
    }
    paths.push(ctx.home.join(MAC_CLAUDE_SKILLS));
    paths.push(ctx.home.join(MAC_CLAUDE_MCP));
    paths.push(ctx.codex_home.join("config.toml"));
    paths.sort();
    paths.dedup();
    paths
        .into_iter()
        .map(|path| (path.to_string_lossy().into_owned(), Some(path)))
        .collect()
}

/// What a look reads, stamped cheaply: every list's servers and every skill
/// folder's name and `SKILL.md` size and time. A watched burst that changed
/// none of it — Claude Code rewriting its own config for its own reasons,
/// most of all — costs this read, not a look. `None` when a list cannot be
/// read: the look says why.
fn inputs_stamp(ctx: &Context) -> Option<u64> {
    use std::hash::{Hash as _, Hasher as _};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    let look = read_mcp(ctx).ok()?;
    let mut servers = |agent: &str, held: &BTreeMap<String, McpEntry>| {
        agent.hash(&mut hasher);
        for (name, entry) in held {
            name.hash(&mut hasher);
            entry.def.fingerprint().hash(&mut hasher);
            entry.disabled.hash(&mut hasher);
        }
    };
    servers("store", &look.view.store);
    for list in &look.view.lists {
        servers(&list.agent, &list.servers);
    }
    look.unreadable.hash(&mut hasher);
    let mut dirs = vec![ctx.store_skills(), ctx.home.join(MAC_CLAUDE_SKILLS)];
    for agent in &ctx.present {
        dirs.extend(ctx.own_skill_dirs(agent));
    }
    if let Some(home) = &ctx.claude_home {
        dirs.push(home.join("skills"));
    }
    for dir in dirs {
        dir.hash(&mut hasher);
        std::fs::symlink_metadata(&dir)
            .is_ok_and(|meta| meta.file_type().is_symlink())
            .hash(&mut hasher);
        let mut entries: Vec<(String, Option<(u64, std::time::SystemTime)>)> =
            std::fs::read_dir(&dir)
                .map(|read| {
                    read.flatten()
                        .map(|entry| {
                            let stamp = std::fs::metadata(
                                entry.path().join(zerocode_core::skill::SKILL_FILE),
                            )
                            .ok()
                            .and_then(|meta| Some((meta.len(), meta.modified().ok()?)));
                            (entry.file_name().to_string_lossy().into_owned(), stamp)
                        })
                        .collect()
                })
                .unwrap_or_default();
        entries.sort();
        entries.hash(&mut hasher);
    }
    Some(hasher.finish())
}

fn one_round(app: &AppHandle) {
    let state = app.state::<AppState>();
    let Ok(ctx) = Context::from_state(&state) else {
        return;
    };
    in_one_burst(|| {
        // Stamped before the look, so a change that lands while it runs makes
        // the next round's stamp differ and is looked at then.
        let stamp = inputs_stamp(&ctx);
        let seen =
            stamp.is_some() && *LOOKED_AT.lock().unwrap_or_else(PoisonError::into_inner) == stamp;
        if !seen {
            match look(&ctx) {
                Ok(looked) => {
                    if looked.changed {
                        let _ = app.emit(CHANGED_EVENT, ());
                    }
                }
                Err(error) => eprintln!("library look: {error}"),
            }
            *LOOKED_AT.lock().unwrap_or_else(PoisonError::into_inner) = stamp;
        }
    });
    state
        .watched()
        .replace_lane(WATCH_LANE, watch_targets(&ctx), true);
}

/// Start the keeper: one look now, one after every settled burst of changes.
pub(crate) fn start(app: AppHandle) {
    let (bell, rung) = mpsc::channel::<()>();
    if POKE.set(bell).is_err() {
        return;
    }
    let _ = std::thread::Builder::new()
        .name("library".into())
        .spawn(move || {
            one_round(&app);
            while rung.recv().is_ok() {
                // Let a burst settle: one look for a save-and-rename, not two.
                while rung.recv_timeout(SETTLE).is_ok() {}
                one_round(&app);
            }
        });
}

// ---- what the page reads ---------------------------------------------------------

#[derive(Debug, Serialize)]
pub(crate) struct AgentRow {
    pub id: &'static str,
    pub label: &'static str,
    pub present: bool,
    /// How it sees the store's skills: `native`, `window-home`, `window-env`,
    /// `own-only`.
    pub skills: &'static str,
    /// Whether it does now: `sees`, `machine-login` (Claude on the person's
    /// own login), `not-linked`, `own-only`, `absent`.
    pub skills_state: &'static str,
    /// Its MCP list: `ok`, `unreadable`, `machine-login`, `absent`.
    pub mcp_state: &'static str,
    pub mcp_file: String,
    /// `command` or `file`.
    pub writes: &'static str,
    /// Whether its own command can sign in to a remote server.
    pub login: bool,
    /// Where the window fetches its mark (the agent catalog's), if anywhere.
    pub favicon_domain: &'static str,
}

#[derive(Debug, Serialize)]
pub(crate) struct ServerAgent {
    pub id: String,
    /// `has`, `missing`, `off`, `elsewhere` (bound to another agent),
    /// `differs`, `unreadable`, `unsupported`.
    pub state: &'static str,
}

#[derive(Debug, Serialize)]
pub(crate) struct ServerRow {
    pub name: String,
    pub kind: &'static str,
    pub summary: String,
    /// The names of the values it is handed — never the values.
    pub secrets: Vec<String>,
    pub origin: String,
    pub at_ms: i64,
    pub in_store: bool,
    pub disabled: bool,
    pub own: Option<String>,
    /// Every agent the person kept it with, when they did (the review of
    /// v1.1.129's skills, t-98860, P3-8); `own` is the first of them.
    pub kept_with: Vec<String>,
    pub shared: bool,
    pub off: Vec<String>,
    pub pick: Vec<Candidate>,
    pub agents: Vec<ServerAgent>,
}

#[derive(Debug, Serialize)]
pub(crate) struct SkillRow {
    pub name: String,
    pub origin: String,
    pub at_ms: i64,
    pub pick: Vec<SkillCandidate>,
}

#[derive(Debug, Serialize)]
pub(crate) struct LogRow {
    #[serde(flatten)]
    pub line: LogLine,
    pub undo: bool,
}

#[derive(Debug, Serialize)]
pub(crate) struct Report {
    pub paused: bool,
    pub running: bool,
    pub merged_at: Option<i64>,
    pub store_skills: String,
    pub store_mcp: String,
    pub agents: Vec<AgentRow>,
    pub servers: Vec<ServerRow>,
    pub skills: Vec<SkillRow>,
    pub log: Vec<LogRow>,
}

fn kind_of(def: &McpDef) -> &'static str {
    match def {
        McpDef::Stdio { .. } => "stdio",
        McpDef::Http { .. } => "http",
        McpDef::Sse { .. } => "sse",
    }
}

pub(crate) fn report(ctx: &Context) -> Report {
    let ledger = read_ledger(ctx);
    let looked = read_mcp(ctx).ok();
    let store = looked
        .as_ref()
        .map(|look| look.view.store.clone())
        .unwrap_or_default();
    let lists: BTreeMap<String, &BTreeMap<String, McpEntry>> = looked
        .as_ref()
        .map(|look| {
            look.view
                .lists
                .iter()
                .filter(|list| list.writable)
                .map(|list| (list.agent.clone(), &list.servers))
                .collect()
        })
        .unwrap_or_default();
    let unreadable = looked
        .as_ref()
        .map(|look| look.unreadable.clone())
        .unwrap_or_default();
    let claude_linked = claude_reads_store(ctx);
    let agents = AGENTS
        .iter()
        .map(|agent| {
            let present = ctx.present.iter().any(|known| known.id == agent.id);
            let skills_state = match (present, agent.skills) {
                (false, _) => "absent",
                (true, Skills::OwnOnly) => "own-only",
                (true, Skills::WindowHome) if ctx.claude_home.is_none() => "machine-login",
                (true, Skills::WindowHome) if !claude_linked => "not-linked",
                _ => "sees",
            };
            let mcp_state = match (present, ctx.mcp_file(agent)) {
                (false, _) => "absent",
                (true, None) => "machine-login",
                (true, Some(_)) if unreadable.contains(agent.id) => "unreadable",
                _ => "ok",
            };
            AgentRow {
                id: agent.id,
                label: agent.label,
                present,
                skills: match agent.skills {
                    Skills::Native => "native",
                    Skills::WindowHome => "window-home",
                    Skills::WindowEnv => "window-env",
                    Skills::OwnOnly => "own-only",
                },
                skills_state,
                mcp_state,
                mcp_file: ctx
                    .mcp_file(agent)
                    .map(|file| file.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                writes: match mcp::remove_command(agent.format, "x") {
                    Write::Command(_) => "command",
                    _ => "file",
                },
                login: matches!(agent.id, "claude" | "codex" | "zo" | "opencode"),
                favicon_domain: zerocode_core::agent::agent_spec(agent.id)
                    .map_or("", |spec| spec.favicon_domain),
            }
        })
        .collect();
    let mut names: BTreeSet<String> = store.keys().cloned().collect();
    names.extend(ledger.mcp.keys().cloned());
    let servers = names
        .into_iter()
        .filter_map(|name| {
            let record = ledger.mcp.get(&name).cloned().unwrap_or_default();
            let entry = store.get(&name).cloned().or_else(|| {
                // Bound to one agent, or asked about: shown by what an agent
                // holds.
                lists
                    .values()
                    .find_map(|servers| servers.get(&name).cloned())
                    .or_else(|| {
                        looked.as_ref().and_then(|look| {
                            look.view
                                .lists
                                .iter()
                                .find_map(|list| list.servers.get(&name).cloned())
                        })
                    })
            })?;
            if !store.contains_key(&name)
                && record.own.is_none()
                && record.kept_with.is_empty()
                && record.pick.is_empty()
            {
                return None;
            }
            let fingerprint = entry.def.fingerprint();
            let agents = ctx
                .present
                .iter()
                .filter(|agent| ctx.mcp_file(agent).is_some())
                .map(|agent| {
                    let held = lists.get(agent.id).and_then(|servers| servers.get(&name));
                    let state = if unreadable.contains(agent.id) {
                        "unreadable"
                    } else if matches!(
                        mcp::add_command(agent.format, &name, &entry.def),
                        Write::Unsupported(_)
                    ) {
                        "unsupported"
                    } else if (record.own.as_deref().is_some_and(|own| own != agent.id)
                        || (!record.kept_with.is_empty() && !record.kept_with.contains(agent.id)))
                        && !record.shared
                    {
                        "elsewhere"
                    } else if record.off.contains(agent.id) || entry.disabled {
                        "off"
                    } else {
                        match held {
                            Some(held) if held.def.fingerprint() == fingerprint => "has",
                            Some(_) => "differs",
                            None => "missing",
                        }
                    };
                    ServerAgent {
                        id: agent.id.to_string(),
                        state,
                    }
                })
                .collect();
            Some(ServerRow {
                kind: kind_of(&entry.def),
                summary: entry.def.summary(),
                secrets: entry.def.secret_names(),
                origin: record.origin.clone(),
                at_ms: record.at_ms,
                in_store: store.contains_key(&name),
                disabled: entry.disabled,
                // A server the person kept with its agents reads as theirs,
                // as one bound to its agent does.
                own: record
                    .own
                    .clone()
                    .or_else(|| record.kept_with.iter().next().cloned()),
                kept_with: record.kept_with.iter().cloned().collect(),
                shared: record.shared,
                off: record.off.iter().cloned().collect(),
                pick: record.pick.clone(),
                agents,
                name,
            })
        })
        .collect();
    let skills = ledger
        .skills
        .iter()
        .map(|(name, record)| SkillRow {
            name: name.clone(),
            origin: record.origin.clone(),
            at_ms: record.at_ms,
            pick: record.pick.clone(),
        })
        .collect();
    let undoable = UNDO.lock().unwrap_or_else(PoisonError::into_inner);
    let log = read_log(ctx)
        .into_iter()
        .map(|line| LogRow {
            undo: undoable.contains_key(&line.id),
            line,
        })
        .collect();
    Report {
        paused: ledger.paused,
        running: RUNNING.load(std::sync::atomic::Ordering::Relaxed),
        merged_at: ledger.merged_at,
        store_skills: ctx.store_skills().to_string_lossy().into_owned(),
        store_mcp: ctx.store_mcp().to_string_lossy().into_owned(),
        agents,
        servers,
        skills,
        log,
    }
}

// ---- what the person does ------------------------------------------------------------

/// Change the record and the store under the look lock, then look once so
/// every agent follows, and answer with the page's report.
fn act(
    ctx: &Context,
    change: impl FnOnce(&mut Ledger) -> Result<Vec<LogLine>, String>,
) -> Result<Report, String> {
    {
        let _held = RUN.lock().unwrap_or_else(PoisonError::into_inner);
        let mut ledger = read_ledger(ctx);
        let store_before = std::fs::read(ctx.store_mcp()).ok();
        let changed = change(&mut ledger);
        keep_store_before(ctx, store_before);
        let lines = changed?;
        write_ledger(ctx, &ledger);
        append_log(ctx, lines);
    }
    look(ctx)?;
    Ok(report(ctx))
}

/// Every writable list holding `name` now, by agent, with its fingerprint.
fn holders(ctx: &Context, name: &str) -> Vec<(String, String)> {
    read_mcp(ctx)
        .map(|look| {
            look.view
                .lists
                .iter()
                .filter(|list| list.writable)
                .filter_map(|list| {
                    list.servers
                        .get(name)
                        .map(|held| (list.agent.clone(), held.def.fingerprint()))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn valid_def(def: &McpDef) -> Result<(), String> {
    match def {
        McpDef::Stdio { command, .. } if !command.trim().is_empty() => Ok(()),
        McpDef::Http { url, .. } | McpDef::Sse { url, .. }
            if url.starts_with("https://") || url.starts_with("http://") =>
        {
            Ok(())
        }
        _ => Err("library.invalidServer".to_string()),
    }
}

/// Add a server, or replace one, as the person wrote it: every list holding
/// the name takes this version.
pub(crate) fn save_server(
    ctx: &Context,
    name: &str,
    def: McpDef,
    previous: Option<&str>,
    shown: Option<&str>,
) -> Result<Report, String> {
    if !mcp::is_server_name(name) {
        return Err("library.invalidName".to_string());
    }
    let before = previous.or(Some(name)).and_then(|old| {
        read_list(&ctx.store_mcp(), Format::Store).and_then(|store| store.get(old).cloned())
    });
    let def = match before {
        Some(old) => kept_values(def, &old.def, shown),
        None => def,
    };
    valid_def(&def)?;
    let renamed = previous.filter(|old| *old != name).map(str::to_string);
    let held_now = holders(ctx, name);
    let old_holders = renamed
        .as_deref()
        .map(|old| holders(ctx, old))
        .unwrap_or_default();
    act(ctx, |ledger| {
        let existed = read_list(&ctx.store_mcp(), Format::Store)
            .is_some_and(|store| store.contains_key(name));
        let disabled = read_list(&ctx.store_mcp(), Format::Store)
            .and_then(|store| store.get(name).map(|entry| entry.disabled))
            .unwrap_or(false);
        let entry = McpEntry { def, disabled };
        store_put(ctx, name, Some(&entry))?;
        ledger.gone_mcp.remove(name);
        let record = ledger.mcp.entry(name.to_string()).or_default();
        if record.origin.is_empty() {
            record.origin = "person".to_string();
            record.at_ms = now();
        }
        record.pick.clear();
        record.own = None;
        record.kept_with.clear();
        for (agent, fingerprint) in held_now {
            record.seen.insert(agent, fingerprint);
        }
        let mut lines = Vec::new();
        if let Some(old) = &renamed {
            store_put(ctx, old, None)?;
            ledger.gone_mcp.insert(old.clone(), now());
            let old_record = ledger.mcp.entry(old.clone()).or_default();
            for (agent, fingerprint) in old_holders {
                old_record.seen.insert(agent, fingerprint);
            }
        }
        let said = line(
            if existed { "edited" } else { "added" },
            "mcp",
            name,
            "person",
        );
        if !existed {
            remember(
                said.id,
                Undo::Added {
                    name: name.to_string(),
                },
            );
        }
        lines.push(said);
        Ok(lines)
    })
}

/// What the page sends back for a server it showed: the page never holds a
/// value, so an empty value means 「keep it」, and a command line or address
/// left as the page showed it (credential-masked) means the one stored.
fn kept_values(mut def: McpDef, old: &McpDef, shown: Option<&str>) -> McpDef {
    let unchanged =
        shown.is_some_and(|shown| def.summary() == shown || summary_of_line(&def) == shown);
    match (&mut def, old) {
        (
            McpDef::Stdio { command, args, env },
            McpDef::Stdio {
                command: old_command,
                args: old_args,
                env: old_env,
            },
        ) => {
            if unchanged {
                command.clone_from(old_command);
                args.clone_from(old_args);
            }
            for (key, value) in env.iter_mut() {
                if value.is_empty()
                    && let Some(kept) = old_env.get(key)
                {
                    value.clone_from(kept);
                }
            }
        }
        (
            McpDef::Http { url, headers } | McpDef::Sse { url, headers },
            McpDef::Http {
                url: old_url,
                headers: old_headers,
            }
            | McpDef::Sse {
                url: old_url,
                headers: old_headers,
            },
        ) => {
            if unchanged {
                url.clone_from(old_url);
            }
            for (key, value) in headers.iter_mut() {
                if value.is_empty()
                    && let Some(kept) = old_headers.get(key)
                {
                    value.clone_from(kept);
                }
            }
        }
        _ => {}
    }
    def
}

/// A stdio server's line as the page writes it back: the command and its
/// arguments joined, before any mask.
fn summary_of_line(def: &McpDef) -> String {
    match def {
        McpDef::Stdio { command, args, .. } => std::iter::once(command.as_str())
            .chain(args.iter().map(String::as_str))
            .collect::<Vec<_>>()
            .join(" "),
        McpDef::Http { url, .. } | McpDef::Sse { url, .. } => url.clone(),
    }
}

/// The skills a repository offers, read by the open `skills` tool in the
/// background — `npx --yes skills add <repository> --list` — so the page can
/// offer them without a terminal.
pub(crate) fn list_repository(ctx: &Context, repository: &str) -> Result<Vec<String>, String> {
    if zerocode_core::skill::bundle_list_command(repository).is_none() {
        return Err("skills.invalidPlan".to_string());
    }
    let argv: Vec<String> = ["--yes", "skills", "add", repository, "--list"]
        .iter()
        .map(|part| (*part).to_string())
        .collect();
    let out = run_command(ctx, "npx", &argv, &[], LIST_BUDGET)?;
    let listing =
        zerocode_core::skill::parse_bundle_output(&out, &zerocode_core::skill::DEFAULT_POLICY);
    if let Some(error) = listing.error {
        return Err(masked(&error));
    }
    if listing.names.is_empty() {
        return Err("skills.noBundleSkills".to_string());
    }
    Ok(listing.names)
}

/// The servers a pasted README block names, for the dialog to show.
pub(crate) fn read_pasted(text: &str) -> Vec<(String, &'static str, String, Vec<String>)> {
    mcp::read_pasted(text)
        .unwrap_or_default()
        .into_iter()
        .filter(|(name, _)| mcp::is_server_name(name))
        .map(|(name, entry)| {
            (
                name,
                kind_of(&entry.def),
                entry.def.summary(),
                entry.def.secret_names(),
            )
        })
        .collect()
}

/// Add every server a pasted block names.
pub(crate) fn add_pasted(ctx: &Context, text: &str) -> Result<Report, String> {
    let found = mcp::read_pasted(text).ok_or("library.pasteUnreadable")?;
    let found: Vec<(String, McpEntry)> = found
        .into_iter()
        .filter(|(name, _)| mcp::is_server_name(name))
        .collect();
    if found.is_empty() {
        return Err("library.pasteEmpty".to_string());
    }
    let mut last = None;
    for (name, entry) in found {
        last = Some(save_server(ctx, &name, entry.def, None, None)?);
    }
    last.ok_or_else(|| "library.pasteEmpty".to_string())
}

/// Remove a server from the store and every agent's list.
pub(crate) fn remove_server_everywhere(ctx: &Context, name: &str) -> Result<Report, String> {
    let held_now = holders(ctx, name);
    act(ctx, |ledger| {
        let store = read_list(&ctx.store_mcp(), Format::Store).unwrap_or_default();
        let entry = store.get(name).cloned();
        store_put(ctx, name, None)?;
        ledger.gone_mcp.insert(name.to_string(), now());
        let record = ledger.mcp.entry(name.to_string()).or_default();
        record.pick.clear();
        for (agent, fingerprint) in held_now {
            record.seen.insert(agent, fingerprint);
        }
        let said = line("deleted", "mcp", name, "person");
        if let Some(entry) = entry {
            remember(
                said.id,
                Undo::Server {
                    name: name.to_string(),
                    entry,
                },
            );
        }
        Ok(vec![said])
    })
}

/// Switch a server off for every agent, or on.
pub(crate) fn switch_server(ctx: &Context, name: &str, on: bool) -> Result<Report, String> {
    act(ctx, |_ledger| {
        let store = read_list(&ctx.store_mcp(), Format::Store).unwrap_or_default();
        let mut entry = store.get(name).cloned().ok_or("library.notFound")?;
        let was_disabled = entry.disabled;
        entry.disabled = !on;
        store_put(ctx, name, Some(&entry))?;
        let said = line(
            if on { "switched-on" } else { "switched-off" },
            "mcp",
            name,
            "person",
        );
        remember(
            said.id,
            Undo::Switched {
                name: name.to_string(),
                was_disabled,
            },
        );
        Ok(vec![said])
    })
}

/// Switch a server off for one agent, or on.
pub(crate) fn switch_server_for(
    ctx: &Context,
    name: &str,
    agent: &str,
    on: bool,
) -> Result<Report, String> {
    if !AGENTS.iter().any(|known| known.id == agent) {
        return Err("library.notFound".to_string());
    }
    act(ctx, |ledger| {
        let record = ledger.mcp.entry(name.to_string()).or_default();
        let was_off = record.off.contains(agent);
        if on {
            record.off.remove(agent);
        } else {
            record.off.insert(agent.to_string());
        }
        let said = line(
            if on { "agent-on" } else { "agent-off" },
            "mcp",
            name,
            agent,
        );
        remember(
            said.id,
            Undo::Agent {
                name: name.to_string(),
                agent: agent.to_string(),
                was_off,
            },
        );
        Ok(vec![said])
    })
}

/// The person chose one version of a server asked about.
pub(crate) fn choose_server(
    ctx: &Context,
    name: &str,
    fingerprint: &str,
) -> Result<Report, String> {
    let look = read_mcp(ctx)?;
    let chosen = look
        .view
        .store
        .get(name)
        .filter(|entry| entry.def.fingerprint() == fingerprint)
        .or_else(|| {
            look.view.lists.iter().find_map(|list| {
                list.servers
                    .get(name)
                    .filter(|entry| entry.def.fingerprint() == fingerprint)
            })
        })
        .cloned()
        .ok_or("library.notFound")?;
    let held_now = holders(ctx, name);
    act(ctx, |ledger| {
        store_put(
            ctx,
            name,
            Some(&McpEntry {
                def: chosen.def.clone(),
                disabled: false,
            }),
        )?;
        let record = ledger.mcp.entry(name.to_string()).or_default();
        record.pick.clear();
        for (agent, seen) in held_now {
            record.seen.insert(agent, seen);
        }
        Ok(vec![line("chosen", "mcp", name, "person")])
    })
}

/// Share a server bound to one agent with every agent anyway.
pub(crate) fn share_server(ctx: &Context, name: &str) -> Result<Report, String> {
    let look = read_mcp(ctx)?;
    act(ctx, |ledger| {
        let record = ledger.mcp.entry(name.to_string()).or_default();
        let (own, entry) = look
            .view
            .lists
            .iter()
            .filter(|list| {
                record.own.as_deref() == Some(list.agent.as_str())
                    || record.kept_with.contains(&list.agent)
            })
            .find_map(|list| Some((list.agent.clone(), list.servers.get(name)?.clone())))
            .ok_or("library.notFound")?;
        record.shared = true;
        record.kept_with.clear();
        record.seen.insert(own.clone(), entry.def.fingerprint());
        store_put(ctx, name, Some(&entry))?;
        Ok(vec![line("shared", "mcp", name, &own)])
    })
}

/// Whether the server answers: Claude Code connects to it and says so.
#[derive(Debug, Serialize)]
pub(crate) struct Check {
    /// `connected`, `failed`, `unknown`.
    pub state: &'static str,
    pub detail: String,
}

pub(crate) fn check_server(ctx: &Context, name: &str) -> Check {
    // A name the store could not hold would reach Claude's command as one of
    // its flags (`--help`): it is not asked about (the review of v1.1.128,
    // P3-4).
    if !mcp::is_server_name(name) {
        return Check {
            state: "unknown",
            detail: String::new(),
        };
    }
    let Some(claude) = AGENTS.iter().find(|agent| agent.id == "claude") else {
        return Check {
            state: "unknown",
            detail: String::new(),
        };
    };
    if ctx.claude_home.is_none() || !ctx.present.iter().any(|agent| agent.id == "claude") {
        return Check {
            state: "unknown",
            detail: String::new(),
        };
    }
    let envs = agent_envs(ctx, claude);
    let envs: Vec<(&str, &OsString)> = envs.iter().map(|(k, v)| (*k, v)).collect();
    let argv = vec!["mcp".to_string(), "get".to_string(), name.to_string()];
    match run_command(ctx, claude.binary, &argv, &envs, CLI_BUDGET) {
        Ok(out) => {
            let status = out
                .lines()
                .map(str::trim)
                .find(|line| line.to_ascii_lowercase().starts_with("status"))
                .unwrap_or_default()
                .to_string();
            let lower = status.to_ascii_lowercase();
            let state = if lower.contains("connected") && !lower.contains("fail") {
                "connected"
            } else if lower.contains("fail") || lower.contains("error") {
                "failed"
            } else {
                "unknown"
            };
            Check {
                state,
                detail: masked(&status),
            }
        }
        Err(error) => Check {
            state: "failed",
            detail: masked(&error),
        },
    }
}

/// The command a person signs in with, typed into a terminal by the page.
pub(crate) fn login_command(name: &str, agent: &str) -> Result<String, String> {
    if !mcp::is_server_name(name) {
        return Err("library.invalidName".to_string());
    }
    let command = match agent {
        "claude" => format!("claude mcp login {name}"),
        "codex" => format!("codex mcp login {name}"),
        "zo" => format!("zo mcp login {name}"),
        "opencode" => format!("opencode mcp auth {name}"),
        _ => return Err("library.noLogin".to_string()),
    };
    Ok(command)
}

/// Remove a skill from the store — to the Trash — and, if asked, every copy
/// an agent the window may write still holds.
pub(crate) fn remove_skill(ctx: &Context, name: &str, also_copies: bool) -> Result<Report, String> {
    if !library::is_skill_name(name) {
        return Err("library.invalidName".to_string());
    }
    act(ctx, |ledger| {
        let target = ctx.store_skills().join(name);
        if std::fs::symlink_metadata(&target).is_err() {
            return Err("library.notFound".to_string());
        }
        let mut receipts = vec![put_away(&target)?];
        if also_copies {
            for agent in &ctx.present {
                for dir in ctx.own_skill_dirs(agent) {
                    let copy = dir.join(name);
                    if std::fs::symlink_metadata(&copy).is_ok()
                        && let Ok(receipt) = put_away(&copy)
                    {
                        receipts.push(receipt);
                    }
                }
            }
        }
        ledger.skill_removed(name, now());
        let said = line("deleted", "skill", name, "person");
        remember(
            said.id,
            Undo::Trashed {
                name: name.to_string(),
                receipts,
            },
        );
        Ok(vec![said])
    })
}

/// Take a skill folder the person picked into the store.
pub(crate) fn import_skill(ctx: &Context, folder: &Path) -> Result<Report, String> {
    let name = folder
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .ok_or("library.invalidName")?;
    if !library::is_skill_name(&name) {
        return Err("library.invalidName".to_string());
    }
    if !folder.join(zerocode_core::skill::SKILL_FILE).is_file() {
        return Err("library.noSkillFile".to_string());
    }
    if std::fs::symlink_metadata(ctx.store_skills().join(&name)).is_ok() {
        return Err("library.skillExists".to_string());
    }
    act(ctx, |ledger| {
        copy_into_store(ctx, folder, &name)?;
        ledger.skill_back(&name);
        let record = ledger.skills.entry(name.clone()).or_default();
        record.origin = "person".to_string();
        record.at_ms = now();
        Ok(vec![line("added", "skill", &name, "person")])
    })
}

/// Make a new skill in the store, and say where its `SKILL.md` is.
pub(crate) fn make_store_skill(
    ctx: &Context,
    name: &str,
    description: &str,
) -> Result<PathBuf, String> {
    if !library::is_skill_name(name)
        || !name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Err("library.invalidSkillName".to_string());
    }
    let dir = ctx.store_skills().join(name);
    if std::fs::symlink_metadata(&dir).is_ok() {
        return Err("library.skillExists".to_string());
    }
    // One line, in YAML's double quotes: a backslash and a quote are escaped,
    // so a Windows path in the description still parses.
    let summary = description
        .lines()
        .next()
        .unwrap_or_default()
        .trim()
        .replace('\\', "\\\\")
        .replace('"', "\\\"");
    let file = dir.join(zerocode_core::skill::SKILL_FILE);
    act(ctx, |ledger| {
        std::fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
        let body = format!("---\nname: {name}\ndescription: \"{summary}\"\n---\n\n# {name}\n\n");
        std::fs::write(&file, body).map_err(|error| error.to_string())?;
        ledger.skill_back(name);
        let record = ledger.skills.entry(name.to_string()).or_default();
        record.origin = "person".to_string();
        record.at_ms = now();
        Ok(vec![line("added", "skill", name, "person")])
    })?;
    Ok(file)
}

/// The person chose which text of a skill stays: the store's (`None`) or a
/// folder an agent holds.
pub(crate) fn choose_skill(
    ctx: &Context,
    name: &str,
    path: Option<&str>,
) -> Result<Report, String> {
    if !library::is_skill_name(name) {
        return Err("library.invalidName".to_string());
    }
    act(ctx, |ledger| {
        let record = ledger.skills.entry(name.to_string()).or_default();
        let candidates = std::mem::take(&mut record.pick);
        if let Some(path) = path {
            let chosen = candidates
                .iter()
                .find(|candidate| candidate.path == path)
                .cloned()
                .ok_or("library.notFound")?;
            // The store's text before goes to the Trash with the copy.
            copy_into_store(ctx, Path::new(&chosen.path), name)?;
            record.digest.clone_from(&chosen.digest);
            // The store now follows the folder chosen, as it follows any
            // folder it copied from.
            record.copied_from = Some(chosen.path.clone());
            record.source_digest = Some(chosen.digest.clone());
        }
        let kept = record.digest.clone();
        for candidate in &candidates {
            if candidate.digest != kept && !record.kept_over.contains(&candidate.digest) {
                record.kept_over.push(candidate.digest.clone());
            }
        }
        Ok(vec![line("chosen", "skill", name, "person")])
    })
}

/// The folder under the keeper's own where a copy the person set to the
/// store's text is kept before it goes ([`align_skill`]): outside the store
/// and every agent's home.
const ALIGNED: &str = "aligned";

/// The person set an agent's copy of a skill to the store's text (「저장소
/// 것으로 맞추기」): the agent reads the store by itself, so the copy in its
/// own folder is the same name read twice, in another text. Each such copy
/// the record asks about is kept whole under the keeper's folder
/// ([`ALIGNED`]), then goes to the Trash (「되돌리기」 brings it back), and
/// the question about it is closed. The folder is the record's own and must
/// stand in one of the agent's own skills folders; the page names the skill
/// and the agent, never a path.
pub(crate) fn align_skill(ctx: &Context, name: &str, agent: &str) -> Result<Report, String> {
    if !library::is_skill_name(name) {
        return Err("library.invalidName".to_string());
    }
    let reads_store = AGENTS
        .iter()
        .find(|known| known.id == agent)
        .filter(|known| known.skills.reads_the_store_everywhere())
        .ok_or("library.notFound")?;
    if std::fs::symlink_metadata(ctx.store_skills().join(name)).is_err() {
        return Err("library.notFound".to_string());
    }
    let own: Vec<PathBuf> = ctx
        .own_skill_dirs(reads_store)
        .into_iter()
        .map(|dir| dir.join(name))
        .collect();
    act(ctx, |ledger| {
        let record = ledger.skills.get_mut(name).ok_or("library.notFound")?;
        let (copies, rest): (Vec<SkillCandidate>, Vec<SkillCandidate>) =
            std::mem::take(&mut record.pick)
                .into_iter()
                .partition(|candidate| {
                    candidate.agent == agent
                        && own.iter().any(|path| Path::new(&candidate.path) == path)
                });
        record.pick = rest;
        if copies.is_empty() {
            return Err("library.notFound".to_string());
        }
        let kept = ctx
            .folder()
            .join(ALIGNED)
            .join(format!("{}-{agent}", now()));
        let mut receipts = Vec::new();
        for (at, copy) in copies.iter().enumerate() {
            let from = Path::new(&copy.path);
            if std::fs::symlink_metadata(from).is_err() {
                continue;
            }
            let into = kept.join(at.to_string()).join(name);
            copy_skill_folder(from, &into)?;
            receipts.push(put_away(from)?);
        }
        let said = line("aligned", "skill", name, agent);
        remember(
            said.id,
            Undo::Trashed {
                name: name.to_string(),
                receipts,
            },
        );
        Ok(vec![said])
    })
}

/// Pause the keeper, or let it run again.
pub(crate) fn set_paused(ctx: &Context, paused: bool) -> Result<Report, String> {
    {
        let _held = RUN.lock().unwrap_or_else(PoisonError::into_inner);
        let mut ledger = read_ledger(ctx);
        ledger.paused = paused;
        write_ledger(ctx, &ledger);
    }
    if !paused {
        look(ctx)?;
    }
    Ok(report(ctx))
}

/// Put back what one line of 「최근 변경」 did.
///
/// One that cannot be done now — a skill's place taken meanwhile — stays to
/// be asked for again (the review of the skills store, P3-4).
pub(crate) fn undo(ctx: &Context, id: u64) -> Result<Report, String> {
    let taken = UNDO
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .remove(&id);
    let Some(mut undo) = taken else {
        return Err("library.cannotUndo".to_string());
    };
    let mut failed = false;
    let done = act(ctx, |ledger| match put_back(ctx, ledger, &mut undo) {
        Ok((item, name)) => Ok(vec![line("undone", item, &name, "person")]),
        Err(error) => {
            failed = true;
            Err(error)
        }
    });
    if failed {
        UNDO.lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id, undo);
    }
    done
}

/// Do one undo, and say what it was about. A folder put back leaves `undo`,
/// so asking again after a failure puts back only the rest.
fn put_back(
    ctx: &Context,
    ledger: &mut Ledger,
    undo: &mut Undo,
) -> Result<(&'static str, String), String> {
    Ok(match undo {
        Undo::Server { name, entry } => {
            store_put(ctx, name, Some(&*entry))?;
            ledger.gone_mcp.remove(name.as_str());
            ("mcp", name.clone())
        }
        Undo::Added { name } => {
            store_put(ctx, name, None)?;
            ledger.gone_mcp.insert(name.clone(), now());
            for (agent, fingerprint) in holders(ctx, name) {
                ledger
                    .mcp
                    .entry(name.clone())
                    .or_default()
                    .seen
                    .insert(agent, fingerprint);
            }
            ("mcp", name.clone())
        }
        Undo::Switched { name, was_disabled } => {
            let store = read_list(&ctx.store_mcp(), Format::Store).unwrap_or_default();
            let mut entry = store
                .get(name.as_str())
                .cloned()
                .ok_or("library.notFound")?;
            entry.disabled = *was_disabled;
            store_put(ctx, name, Some(&entry))?;
            ("mcp", name.clone())
        }
        Undo::Agent {
            name,
            agent,
            was_off,
        } => {
            let record = ledger.mcp.entry(name.clone()).or_default();
            if *was_off {
                record.off.insert(agent.clone());
            } else {
                record.off.remove(agent.as_str());
            }
            ("mcp", name.clone())
        }
        Undo::TakenIn { name, holders } => {
            let record = ledger.mcp.entry(name.clone()).or_default();
            record.kept_with = holders.clone();
            record.shared = false;
            store_put(ctx, name, None)?;
            ("mcp", name.clone())
        }
        Undo::Trashed { name, receipts } => {
            while let Some(receipt) = receipts.first() {
                receipt.restore()?;
                receipts.remove(0);
            }
            ledger.skill_back(name);
            ("skill", name.clone())
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context_of(home: &Path, data: &Path, ids: &[&str]) -> Context {
        Context {
            present: AGENTS
                .iter()
                .filter(|agent| ids.contains(&agent.id))
                .collect(),
            ..context(home, data)
        }
    }

    fn context(home: &Path, data: &Path) -> Context {
        fake_trash::use_bin(&data.join("trash"));
        Context {
            home: home.to_path_buf(),
            data: data.to_path_buf(),
            claude_home: None,
            codex_home: data.join("codex-runtime-home/home"),
            config_home: home.join(".config"),
            present: AGENTS
                .iter()
                .filter(|agent| matches!(agent.id, "kiro" | "cursor"))
                .collect(),
            path: None,
        }
    }

    /// The person's example on a fake home, through the two agents whose
    /// lists the window edits as files: Kiro adds a, Cursor adds b, and one
    /// look leaves both in both — and in the store.
    #[test]
    fn a_server_each_agent_adds_reaches_the_other() {
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(home.path().join(".kiro/settings")).unwrap();
        std::fs::create_dir_all(home.path().join(".cursor")).unwrap();
        std::fs::write(
            home.path().join(".kiro/settings/mcp.json"),
            r#"{"mcpServers":{"a":{"command":"a-mcp","args":["--x"],"disabled":false}}}"#,
        )
        .unwrap();
        std::fs::write(
            home.path().join(".cursor/mcp.json"),
            r#"{"mcpServers":{"b":{"url":"https://b.example.com/mcp"}},"other":true}"#,
        )
        .unwrap();
        let ctx = context(home.path(), data.path());
        look(&ctx).unwrap();
        let kiro = read_list(&home.path().join(".kiro/settings/mcp.json"), Format::Kiro).unwrap();
        let cursor = read_list(&home.path().join(".cursor/mcp.json"), Format::Cursor).unwrap();
        let store = read_list(&ctx.store_mcp(), Format::Store).unwrap();
        for list in [&kiro, &cursor, &store] {
            assert!(list.contains_key("a") && list.contains_key("b"), "{list:?}");
        }
        let cursor_doc: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(home.path().join(".cursor/mcp.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            cursor_doc["other"], true,
            "another key of the agent's file was lost"
        );
        // A second look finds nothing to do.
        assert!(!look(&ctx).unwrap().changed);
        // Backups were taken before the first edit.
        assert!(
            data.path()
                .join("library/backups/kiro")
                .read_dir()
                .unwrap()
                .next()
                .is_some()
        );
    }

    #[test]
    fn a_server_removed_in_one_agent_leaves_the_other() {
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(home.path().join(".kiro/settings")).unwrap();
        std::fs::create_dir_all(home.path().join(".cursor")).unwrap();
        std::fs::write(
            home.path().join(".kiro/settings/mcp.json"),
            r#"{"mcpServers":{"a":{"command":"a-mcp"},"b":{"command":"b-mcp"}}}"#,
        )
        .unwrap();
        let ctx = context(home.path(), data.path());
        look(&ctx).unwrap();
        assert!(
            read_list(&home.path().join(".cursor/mcp.json"), Format::Cursor)
                .unwrap()
                .contains_key("a")
        );
        // Kiro keeps b: its list was not reset, so a missing is a removal.
        std::fs::write(
            home.path().join(".kiro/settings/mcp.json"),
            r#"{"mcpServers":{"b":{"command":"b-mcp"}}}"#,
        )
        .unwrap();
        look(&ctx).unwrap();
        assert!(
            !read_list(&home.path().join(".cursor/mcp.json"), Format::Cursor)
                .unwrap()
                .contains_key("a")
        );
        assert!(
            !read_list(&ctx.store_mcp(), Format::Store)
                .unwrap()
                .contains_key("a")
        );
        let log = read_log(&ctx);
        assert!(
            log.iter()
                .any(|line| line.kind == "removed" && line.name == "a" && line.who == "kiro"),
            "{log:?}"
        );
    }

    /// The review of v1.1.128 (t-97098 P1): an agent's MCP file deleted (a home
    /// reset, a reinstall) is a reset — the window writes the store's servers
    /// back into it, and nothing leaves the store or the other agents.
    #[test]
    fn a_deleted_agent_list_is_written_back_and_takes_nothing_from_the_others() {
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(home.path().join(".kiro/settings")).unwrap();
        std::fs::create_dir_all(home.path().join(".cursor")).unwrap();
        std::fs::write(
            home.path().join(".kiro/settings/mcp.json"),
            r#"{"mcpServers":{"a":{"command":"a-mcp"}}}"#,
        )
        .unwrap();
        let ctx = context(home.path(), data.path());
        look(&ctx).unwrap();
        std::fs::remove_file(home.path().join(".kiro/settings/mcp.json")).unwrap();
        look(&ctx).unwrap();
        // Kiro's file is written back too, not only left out of the removal
        // (the review of the v1.1.128 integration, P3).
        for list in [
            (home.path().join(".kiro/settings/mcp.json"), Format::Kiro),
            (home.path().join(".cursor/mcp.json"), Format::Cursor),
            (ctx.store_mcp(), Format::Store),
        ] {
            assert!(
                read_list(&list.0, list.1).unwrap().contains_key("a"),
                "{} lost a",
                list.0.display()
            );
        }
        let log = read_log(&ctx);
        assert!(!log.iter().any(|line| line.kind == "removed"), "{log:?}");
        assert!(
            log.iter()
                .any(|line| line.kind == "reset" && line.who == "kiro"),
            "{log:?}"
        );
    }

    /// The review of v1.1.128 (P2-1), on disk: Kiro adds a server, the look
    /// takes it in and writes it into Cursor; 「되돌리기」 on that line keeps it
    /// with Kiro alone — out of the store and out of Cursor, and the next look
    /// does not take it in again.
    #[test]
    fn undoing_a_take_in_keeps_the_server_with_its_agent() {
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(home.path().join(".kiro/settings")).unwrap();
        std::fs::create_dir_all(home.path().join(".cursor")).unwrap();
        let kiro = home.path().join(".kiro/settings/mcp.json");
        let cursor = home.path().join(".cursor/mcp.json");
        std::fs::write(&kiro, r#"{"mcpServers":{}}"#).unwrap();
        let ctx = context(home.path(), data.path());
        look(&ctx).unwrap();
        std::fs::write(&kiro, r#"{"mcpServers":{"a":{"command":"a-mcp"}}}"#).unwrap();
        look(&ctx).unwrap();
        assert!(
            read_list(&cursor, Format::Cursor)
                .unwrap()
                .contains_key("a")
        );
        let taken = report(&ctx)
            .log
            .into_iter()
            .find(|row| row.line.kind == "taken-in" && row.line.name == "a")
            .unwrap();
        assert!(taken.undo);
        undo(&ctx, taken.line.id).unwrap();
        look(&ctx).unwrap();
        assert!(
            !read_list(&ctx.store_mcp(), Format::Store)
                .unwrap()
                .contains_key("a"),
            "the server was taken in again"
        );
        assert!(
            !read_list(&cursor, Format::Cursor)
                .unwrap()
                .contains_key("a"),
            "the window's copy stayed in Cursor"
        );
        assert!(read_list(&kiro, Format::Kiro).unwrap().contains_key("a"));
    }

    /// The review of v1.1.128 skills-p2 (P3-5): Kiro and Cursor both held
    /// a before it was taken in. 「되돌리기」 keeps it with both — Cursor's own
    /// copy is not pulled for being one the window had seen.
    #[test]
    fn undoing_a_take_in_keeps_the_server_with_every_agent_that_held_it() {
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(home.path().join(".kiro/settings")).unwrap();
        std::fs::create_dir_all(home.path().join(".cursor")).unwrap();
        let kiro = home.path().join(".kiro/settings/mcp.json");
        let cursor = home.path().join(".cursor/mcp.json");
        std::fs::write(&kiro, r#"{"mcpServers":{"b":{"command":"b-mcp"}}}"#).unwrap();
        std::fs::write(&cursor, r#"{"mcpServers":{"b":{"command":"b-mcp"}}}"#).unwrap();
        let ctx = context(home.path(), data.path());
        look(&ctx).unwrap();
        let a = r#""a":{"command":"a-mcp"}"#;
        std::fs::write(
            &kiro,
            format!(r#"{{"mcpServers":{{{a},"b":{{"command":"b-mcp"}}}}}}"#),
        )
        .unwrap();
        std::fs::write(
            &cursor,
            format!(r#"{{"mcpServers":{{{a},"b":{{"command":"b-mcp"}}}}}}"#),
        )
        .unwrap();
        look(&ctx).unwrap();
        let taken = report(&ctx)
            .log
            .into_iter()
            .find(|row| row.line.kind == "taken-in" && row.line.name == "a")
            .expect("a was taken in");
        undo(&ctx, taken.line.id).unwrap();
        look(&ctx).unwrap();
        // The page names every agent it stays with, not the first alone (the
        // review of v1.1.129's skills, t-98860, P3-8).
        let row = report(&ctx)
            .servers
            .into_iter()
            .find(|row| row.name == "a")
            .expect("a is listed");
        assert_eq!(row.kept_with, ["cursor", "kiro"]);
        for (file, format) in [(&kiro, Format::Kiro), (&cursor, Format::Cursor)] {
            assert!(
                read_list(file, format).unwrap().contains_key("a"),
                "{} lost its own a",
                file.display()
            );
        }
        assert!(
            !read_list(&ctx.store_mcp(), Format::Store)
                .unwrap()
                .contains_key("a")
        );
    }

    /// The review of the skills store (P3-4, undo): a 「되돌리기」 that could
    /// not put a skill back — its folder's place taken meanwhile — can be
    /// asked for again once the place is free, instead of being spent.
    #[test]
    fn an_undo_that_fails_can_be_asked_for_again() {
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let ctx = context(home.path(), data.path());
        let store = ctx.store_skills();
        std::fs::create_dir_all(store.join("z")).unwrap();
        std::fs::write(store.join("z/SKILL.md"), "---\nname: z\n---\nz\n").unwrap();
        look(&ctx).unwrap();
        let removed = remove_skill(&ctx, "z", false).unwrap();
        let deleted = removed
            .log
            .iter()
            .find(|row| row.line.kind == "deleted" && row.line.name == "z")
            .unwrap()
            .line
            .id;
        std::fs::write(store.join("z"), "in the way").unwrap();
        assert!(undo(&ctx, deleted).is_err());
        std::fs::remove_file(store.join("z")).unwrap();
        undo(&ctx, deleted).unwrap();
        assert!(store.join("z/SKILL.md").is_file());
    }

    /// The review of v1.1.128 skills-p2 (P3-4): the copy a removed skill
    /// left behind stays out only while it stands. Once the person deletes
    /// it, the same text installed there again is an install, and comes back.
    #[test]
    fn a_left_copy_the_person_deleted_is_no_longer_one() {
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let ctx = context(home.path(), data.path());
        let copy = home.path().join(MAC_CLAUDE_SKILLS).join("z");
        let text = "---\nname: z\n---\nz\n";
        std::fs::create_dir_all(&copy).unwrap();
        std::fs::write(copy.join("SKILL.md"), text).unwrap();
        look(&ctx).unwrap();
        let store = ctx.store_skills().join("z");
        assert!(store.join("SKILL.md").is_file());
        remove_skill(&ctx, "z", false).unwrap();
        look(&ctx).unwrap();
        assert!(!store.exists(), "the copy left behind came back");

        std::fs::remove_dir_all(&copy).unwrap();
        look(&ctx).unwrap();
        std::fs::create_dir_all(&copy).unwrap();
        std::fs::write(copy.join("SKILL.md"), text).unwrap();
        look(&ctx).unwrap();
        assert!(store.join("SKILL.md").is_file(), "the install stayed out");
    }

    /// The review of v1.1.128 (P1, the store's half): the store's list is
    /// every agent's source, so a look that changes it keeps what it held
    /// first, beside the agents' backups.
    #[test]
    fn the_store_list_is_backed_up_before_a_look_changes_it() {
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(home.path().join(".kiro/settings")).unwrap();
        std::fs::create_dir_all(home.path().join(".cursor")).unwrap();
        std::fs::write(
            home.path().join(".kiro/settings/mcp.json"),
            r#"{"mcpServers":{"a":{"command":"a-mcp"}}}"#,
        )
        .unwrap();
        let ctx = context(home.path(), data.path());
        look(&ctx).unwrap();
        std::fs::write(
            home.path().join(".cursor/mcp.json"),
            r#"{"mcpServers":{"a":{"command":"a-mcp"},"b":{"command":"b-mcp"}}}"#,
        )
        .unwrap();
        look(&ctx).unwrap();
        assert!(
            read_list(&ctx.store_mcp(), Format::Store)
                .unwrap()
                .contains_key("b")
        );
        let kept: Vec<PathBuf> = std::fs::read_dir(data.path().join("library/backups/store"))
            .map(|read| read.flatten().map(|entry| entry.path()).collect())
            .unwrap_or_default();
        assert_eq!(kept.len(), 1, "{kept:?}");
        let before = read_list(&kept[0], Format::Store).unwrap();
        assert_eq!(
            before.keys().cloned().collect::<Vec<_>>(),
            vec!["a".to_string()]
        );
    }

    fn skill_at(dir: &Path, name: &str) -> PathBuf {
        let at = dir.join(name);
        std::fs::create_dir_all(&at).unwrap();
        std::fs::write(at.join("SKILL.md"), format!("---\nname: {name}\n---\n")).unwrap();
        at
    }

    /// The review of v1.1.128 (P2-3): a skill folder that is a link — a
    /// project someone is writing a skill in — goes into the store as a link
    /// to that same folder, never as a copy of the project; a link inside a
    /// skill is copied as a link, never followed (a link to its own folder
    /// would copy forever).
    #[cfg(unix)]
    #[test]
    fn a_linked_skill_is_linked_into_the_store_and_a_link_inside_is_not_followed() {
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let project = skill_at(&home.path().join("dev"), "foo");
        std::fs::create_dir_all(home.path().join(".claude/skills")).unwrap();
        std::os::unix::fs::symlink(&project, home.path().join(".claude/skills/foo")).unwrap();
        let k = skill_at(&home.path().join(".kiro/skills"), "k");
        std::os::unix::fs::symlink(".", k.join("again")).unwrap();
        let ctx = context(home.path(), data.path());
        look(&ctx).unwrap();
        let store = ctx.store_skills();
        assert!(
            std::fs::symlink_metadata(store.join("foo"))
                .unwrap()
                .file_type()
                .is_symlink(),
            "the linked project was copied"
        );
        assert_eq!(
            std::fs::canonicalize(store.join("foo")).unwrap(),
            std::fs::canonicalize(&project).unwrap()
        );
        assert!(store.join("k/SKILL.md").is_file(), "{:?}", read_log(&ctx));
        assert!(
            std::fs::symlink_metadata(store.join("k/again"))
                .unwrap()
                .file_type()
                .is_symlink(),
            "the link inside was followed"
        );
    }

    /// The review of 4ca485472 (P1): zo's skill is a link to Gemini's own
    /// folder. The store links to that folder, so the folder is no duplicate
    /// to send to the Trash — it is what the store reads, and every agent
    /// would lose the skill with it.
    #[cfg(unix)]
    #[test]
    fn the_folder_a_store_link_points_at_is_never_trashed_as_a_duplicate() {
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let real = skill_at(&home.path().join(".gemini/skills"), "foo");
        std::fs::create_dir_all(home.path().join(".zo/skills")).unwrap();
        std::os::unix::fs::symlink(&real, home.path().join(".zo/skills/foo")).unwrap();
        let ctx = context_of(home.path(), data.path(), &["zo", "gemini"]);
        look(&ctx).unwrap();
        look(&ctx).unwrap();
        assert!(real.join("SKILL.md").is_file(), "Gemini's own folder went");
        assert!(
            ctx.store_skills().join("foo/SKILL.md").is_file(),
            "the store lost the skill"
        );
        assert!(!read_log(&ctx).iter().any(|line| line.kind == "removed"));
    }

    /// The review of cdf998355 (t-97910, P2-1): the store holds a copy of its
    /// own (Codex's skill moved in first), and zo's skill is a link to
    /// Gemini's own folder of the same text. That folder is what zo reads:
    /// it is not sent to the Trash as Gemini's duplicate.
    #[cfg(unix)]
    #[test]
    fn a_folder_another_agents_link_points_at_is_never_trashed_as_a_duplicate() {
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        skill_at(&home.path().join(".codex/skills"), "foo");
        let real = skill_at(&home.path().join(".gemini/skills"), "foo");
        std::fs::create_dir_all(home.path().join(".zo/skills")).unwrap();
        let zo = home.path().join(".zo/skills/foo");
        std::os::unix::fs::symlink(&real, &zo).unwrap();
        let ctx = context_of(home.path(), data.path(), &["codex", "zo", "gemini"]);
        look(&ctx).unwrap();
        look(&ctx).unwrap();
        assert!(
            real.join("SKILL.md").is_file(),
            "Gemini's folder zo reads went"
        );
        assert!(zo.join("SKILL.md").is_file(), "zo lost the skill");
        assert!(ctx.store_skills().join("foo/SKILL.md").is_file());
    }

    /// The review of cdf998355 (t-97910, P2-2): zo's copy into the store
    /// fails (here the store's folder cannot be made), so Gemini's folder of
    /// the same text is no duplicate of anything the store holds and stays.
    /// The copy cannot be staged: a file stands where it would be made — for
    /// root too, whom a folder's permissions would not stop (the review of
    /// skills-p2-r3, P3-2). The store's folder itself reads as it is (a link
    /// that reads nothing is no empty store, the review of v1.1.129's skills,
    /// t-98860, P3-5).
    #[test]
    fn a_folder_is_trashed_as_a_duplicate_only_of_what_the_store_holds() {
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        skill_at(&home.path().join(".zo/skills"), "foo");
        let real = skill_at(&home.path().join(".gemini/skills"), "foo");
        let ctx = context_of(home.path(), data.path(), &["zo", "gemini"]);
        let store = ctx.store_skills();
        std::fs::create_dir_all(&store).unwrap();
        std::fs::write(store.join(".zerocode-incoming-foo"), "taken\n").unwrap();
        look(&ctx).unwrap();
        assert!(!ctx.store_skills().join("foo").exists());
        assert!(
            real.join("SKILL.md").is_file(),
            "Gemini's folder went though the store holds nothing"
        );
    }

    /// The review of 74874b969 (t-97954, P1): Codex's skills folder is a link
    /// to the person's own Claude skills folder, and the store already holds
    /// the same text. Codex's `foo` is the person's Claude folder seen from
    /// another place: it is never sent to the Trash as Codex's duplicate.
    #[cfg(unix)]
    #[test]
    fn a_folder_seen_through_a_linked_skills_folder_is_never_trashed() {
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let claude = home.path().join(".claude/skills");
        let real = skill_at(&claude, "foo");
        skill_at(&home.path().join(".agents/skills"), "foo");
        std::fs::create_dir_all(home.path().join(".codex")).unwrap();
        std::os::unix::fs::symlink(&claude, home.path().join(".codex/skills")).unwrap();
        let ctx = context_of(home.path(), data.path(), &["codex"]);
        look(&ctx).unwrap();
        look(&ctx).unwrap();
        assert!(
            real.join("SKILL.md").is_file(),
            "the person's Claude folder went"
        );
    }

    /// The same link, and the store holds nothing yet: Codex's skill is
    /// copied into the store, never moved out of the person's Claude home.
    #[cfg(unix)]
    #[test]
    fn a_folder_seen_through_a_linked_skills_folder_is_copied_not_moved() {
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let claude = home.path().join(".claude/skills");
        let real = skill_at(&claude, "foo");
        std::fs::create_dir_all(home.path().join(".codex")).unwrap();
        std::os::unix::fs::symlink(&claude, home.path().join(".codex/skills")).unwrap();
        let ctx = context_of(home.path(), data.path(), &["codex"]);
        look(&ctx).unwrap();
        look(&ctx).unwrap();
        assert!(
            real.join("SKILL.md").is_file(),
            "the person's Claude folder moved"
        );
        assert!(ctx.store_skills().join("foo/SKILL.md").is_file());
    }

    /// The review of 94e1edaf7 (t-98010, P2-1): Codex's skills folder is a
    /// link to the person's Claude skills. The store's copy follows the
    /// person's edit there — never asked about — and a look with nothing new
    /// plans nothing (no Trash step the run would only skip, P3-5 of the
    /// review of 74874b969).
    #[cfg(unix)]
    #[test]
    fn a_folder_seen_through_a_linked_skills_folder_is_followed_not_asked() {
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let claude = home.path().join(".claude/skills");
        let real = skill_at(&claude, "foo");
        std::fs::create_dir_all(home.path().join(".codex")).unwrap();
        std::os::unix::fs::symlink(&claude, home.path().join(".codex/skills")).unwrap();
        let ctx = context_of(home.path(), data.path(), &["codex"]);
        look(&ctx).unwrap();
        look(&ctx).unwrap();
        assert!(!look(&ctx).unwrap().changed, "a look planned what it skips");
        assert!(
            read_ledger(&ctx).skills["foo"].copied_from.is_some(),
            "{:?}",
            read_ledger(&ctx)
        );
        std::fs::write(real.join("SKILL.md"), "---\nname: foo\n---\nedited\n").unwrap();
        look(&ctx).unwrap();
        assert!(
            std::fs::read_to_string(ctx.store_skills().join("foo/SKILL.md"))
                .unwrap()
                .ends_with("edited\n"),
            "the store did not follow the person's edit"
        );
        assert!(
            !read_log(&ctx).iter().any(|line| line.kind == "asked"),
            "{:?}",
            read_log(&ctx)
        );
        assert!(real.join("SKILL.md").is_file());
    }

    /// One package's two texts of `foo` — a Claude Code one and a Codex one,
    /// as im-not-ai ships humanize-korean — linked from one checkout: the
    /// store's link reads the Codex text, the person's Claude's the other.
    /// `bar` is linked from two checkouts. Returns the person's Claude skills
    /// folder.
    #[cfg(unix)]
    fn one_checkouts_two_texts(ctx: &Context) -> PathBuf {
        let package = ctx.home.join("src/package");
        let fork = ctx.home.join("src/fork");
        for checkout in [&package, &fork] {
            std::fs::create_dir_all(checkout.join(".git")).unwrap();
        }
        let claude_text = skill_at(&package.join("skills"), "foo");
        let codex_text = skill_at(&package.join("codex/skills"), "foo");
        std::fs::write(
            codex_text.join("SKILL.md"),
            "---\nname: foo\n---\none call\n",
        )
        .unwrap();
        let bar = skill_at(&package.join("codex/skills"), "bar");
        let forked = skill_at(&fork, "bar");
        std::fs::write(forked.join("SKILL.md"), "---\nname: bar\n---\nforked\n").unwrap();
        let store = ctx.store_skills();
        let claude = ctx.home.join(MAC_CLAUDE_SKILLS);
        for (target, link) in [
            (&codex_text, store.join("foo")),
            (&claude_text, claude.join("foo")),
            (&bar, store.join("bar")),
            (&forked, claude.join("bar")),
        ] {
            std::fs::create_dir_all(link.parent().unwrap()).unwrap();
            std::os::unix::fs::symlink(target, link).unwrap();
        }
        claude
    }

    #[cfg(unix)]
    #[test]
    fn one_checkouts_texts_of_a_skill_are_never_asked_about() {
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let ctx = context(home.path(), data.path());
        one_checkouts_two_texts(&ctx);
        look(&ctx).unwrap();
        let ledger = read_ledger(&ctx);
        assert!(
            ledger.skills["foo"].pick.is_empty(),
            "{:?}",
            ledger.skills["foo"]
        );
        assert_eq!(
            ledger.skills["bar"].pick.len(),
            1,
            "{:?}",
            ledger.skills["bar"]
        );
    }

    /// The person's ledger from v1.1.134 holds the question: the next look
    /// drops it, and both links stand.
    #[cfg(unix)]
    #[test]
    fn a_question_an_earlier_build_saved_about_one_checkouts_text_is_dropped() {
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let ctx = context(home.path(), data.path());
        let asked = one_checkouts_two_texts(&ctx).join("foo");
        look(&ctx).unwrap();
        let mut saved = read_ledger(&ctx);
        saved
            .skills
            .get_mut("foo")
            .unwrap()
            .pick
            .push(SkillCandidate {
                agent: MAC_CLAUDE.to_string(),
                path: asked.to_string_lossy().into_owned(),
                digest: folder_digest(&asked).unwrap(),
            });
        write_ledger(&ctx, &saved);
        look(&ctx).unwrap();
        assert!(
            read_ledger(&ctx).skills["foo"].pick.is_empty(),
            "{:?}",
            read_ledger(&ctx).skills["foo"]
        );
        assert!(is_link(&asked) && is_link(&ctx.store_skills().join("foo")));
    }

    /// The review of cdf998355 (t-97910, P3-3) and the review of v1.1.128
    /// (P3-8): Codex and Gemini each hold a link to one dotfiles folder. A link
    /// is its own entry, apart from what it reads: Codex's moves into the
    /// store as a link to the same folder, Gemini's goes as the store's
    /// duplicate — so neither agent lists the skill twice — and the folder
    /// both read stays where it is.
    #[cfg(unix)]
    #[test]
    fn a_link_in_an_agent_that_reads_the_store_moves_and_its_folder_stays() {
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let real = skill_at(&home.path().join("dotfiles"), "foo");
        for agent in [".codex/skills", ".gemini/skills"] {
            std::fs::create_dir_all(home.path().join(agent)).unwrap();
            std::os::unix::fs::symlink(&real, home.path().join(agent).join("foo")).unwrap();
        }
        let ctx = context_of(home.path(), data.path(), &["codex", "gemini"]);
        look(&ctx).unwrap();
        look(&ctx).unwrap();
        let store = ctx.store_skills().join("foo");
        assert!(
            std::fs::symlink_metadata(&store)
                .unwrap()
                .file_type()
                .is_symlink(),
            "the store copied the linked folder"
        );
        assert_eq!(
            std::fs::canonicalize(&store).unwrap(),
            std::fs::canonicalize(&real).unwrap()
        );
        for agent in [".codex/skills/foo", ".gemini/skills/foo"] {
            assert!(
                std::fs::symlink_metadata(home.path().join(agent)).is_err(),
                "{agent} still lists the skill beside the store's"
            );
        }
        assert!(real.join("SKILL.md").is_file(), "the folder both read went");
    }

    /// The review of 74874b969 (t-97954, P2): the person's Claude reads
    /// Codex's folder through a link. Codex's skill is copied into the store,
    /// so the link keeps finding it.
    #[cfg(unix)]
    #[test]
    fn a_folder_a_link_reads_is_copied_into_the_store_not_moved() {
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let real = skill_at(&home.path().join(".codex/skills"), "foo");
        std::fs::create_dir_all(home.path().join(".claude/skills")).unwrap();
        let claude = home.path().join(".claude/skills/foo");
        std::os::unix::fs::symlink(&real, &claude).unwrap();
        let ctx = context_of(home.path(), data.path(), &["codex"]);
        look(&ctx).unwrap();
        look(&ctx).unwrap();
        assert!(
            claude.join("SKILL.md").is_file(),
            "the person's Claude lost the skill"
        );
        assert!(ctx.store_skills().join("foo/SKILL.md").is_file());
    }

    /// The review of 94e1edaf7 (t-98010, P3-1): a folder whose whole path the
    /// look cannot read is kept — copied, never moved or sent to the Trash —
    /// rather than let through as nobody's.
    #[test]
    fn a_folder_whose_whole_path_cannot_be_read_is_kept() {
        let home = tempfile::tempdir().unwrap();
        assert!(is_kept(&BTreeSet::new(), &home.path().join("gone/foo")));
    }

    /// The review of 94e1edaf7 (t-98010, P3-2): the person's Claude reads a
    /// folder inside Codex's skill (`bar -> ~/.codex/skills/foo/inner`). Codex's
    /// `foo` holds what that link reads: it is copied into the store and
    /// stays, so the link keeps finding it.
    #[cfg(unix)]
    #[test]
    fn a_folder_holding_what_a_link_reads_is_copied_not_moved() {
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let foo = skill_at(&home.path().join(".codex/skills"), "foo");
        let inner = skill_at(&foo, "inner");
        std::fs::create_dir_all(home.path().join(".claude/skills")).unwrap();
        let bar = home.path().join(".claude/skills/bar");
        std::os::unix::fs::symlink(&inner, &bar).unwrap();
        let ctx = context_of(home.path(), data.path(), &["codex"]);
        look(&ctx).unwrap();
        look(&ctx).unwrap();
        assert!(foo.join("SKILL.md").is_file(), "Codex's folder moved");
        assert!(
            bar.join("SKILL.md").is_file(),
            "the person's Claude lost bar"
        );
        assert!(ctx.store_skills().join("foo/SKILL.md").is_file());
    }

    /// The review of v1.1.129's skills (t-98860, P2-2): the person's Claude
    /// reads a link that reads a link — `~/.claude/skills/bar ->
    /// ~/.codex/skills/foo -> ~/proj/foo`. Codex's link is a place on the way:
    /// it stays, so `bar` keeps finding the skill.
    #[cfg(unix)]
    #[test]
    fn a_link_another_link_passes_through_stays() {
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let real = skill_at(&home.path().join("proj"), "foo");
        std::fs::create_dir_all(home.path().join(".codex/skills")).unwrap();
        let codex = home.path().join(".codex/skills/foo");
        std::os::unix::fs::symlink(&real, &codex).unwrap();
        std::fs::create_dir_all(home.path().join(".claude/skills")).unwrap();
        let bar = home.path().join(".claude/skills/bar");
        std::os::unix::fs::symlink(&codex, &bar).unwrap();
        let ctx = context_of(home.path(), data.path(), &["codex"]);
        look(&ctx).unwrap();
        look(&ctx).unwrap();
        assert!(is_link(&codex), "Codex's link that bar reads through went");
        assert!(
            bar.join("SKILL.md").is_file(),
            "the person's Claude lost bar"
        );
        assert!(ctx.store_skills().join("foo/SKILL.md").is_file());
    }

    /// The second review of v1.1.129's skills (t-99089, P3-A): a link that
    /// reads into a folder whose parent is a link —
    /// `~/.claude/skills/bar -> ~/.codex/skills/foo/inner`, and
    /// `~/.codex/skills/foo -> ~/proj/foo` — keeps that parent link where it
    /// stands, or bar would read nothing.
    #[cfg(unix)]
    #[test]
    fn a_link_whose_parent_on_the_way_is_a_link_keeps_that_link() {
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let real = skill_at(&home.path().join("proj"), "foo");
        skill_at(&real, "inner");
        std::fs::create_dir_all(home.path().join(".codex/skills")).unwrap();
        let codex = home.path().join(".codex/skills/foo");
        std::os::unix::fs::symlink(&real, &codex).unwrap();
        std::fs::create_dir_all(home.path().join(".claude/skills")).unwrap();
        let bar = home.path().join(".claude/skills/bar");
        std::os::unix::fs::symlink(codex.join("inner"), &bar).unwrap();
        let ctx = context_of(home.path(), data.path(), &["codex"]);
        look(&ctx).unwrap();
        look(&ctx).unwrap();
        assert!(is_link(&codex), "Codex's link that bar reads into went");
        assert!(
            bar.join("SKILL.md").is_file(),
            "the person's Claude lost bar"
        );
    }

    /// The second review of v1.1.129's skills (t-99089, P3-B): on Windows
    /// without the link privilege, a folder link copied out of a skill is a
    /// junction only to a whole path. One that reads inside the skill
    /// (`lib -> ./shared`) or a sibling skill (`ref -> ../b/lib`) is refused as
    /// before: a junction would bind the store's copy to the folder it came
    /// from, which the move takes away.
    #[cfg(unix)]
    #[test]
    fn a_junction_stands_in_only_for_a_folder_link_read_by_its_whole_path() {
        let home = tempfile::tempdir().unwrap();
        let skills = home.path().join(".kiro/skills");
        let shared = skills.join("_common");
        std::fs::create_dir_all(&shared).unwrap();
        let b = skill_at(&skills, "b");
        std::fs::create_dir_all(b.join("lib")).unwrap();
        let k = skill_at(&skills, "k");
        std::fs::create_dir_all(k.join("shared")).unwrap();
        std::os::unix::fs::symlink("./shared", k.join("lib")).unwrap();
        std::os::unix::fs::symlink("../b/lib", k.join("ref")).unwrap();
        std::os::unix::fs::symlink("../_common", k.join("common")).unwrap();
        std::os::unix::fs::symlink(&shared, k.join("whole")).unwrap();
        let reads = |name: &str| {
            let from = k.join(name);
            junction_reads(&link_target(&k, &from).unwrap(), &from)
        };
        for inside in ["lib", "ref"] {
            assert_eq!(
                reads(inside),
                Err("library.linkNotCopied".to_string()),
                "{inside}"
            );
        }
        for whole in ["common", "whole"] {
            assert_eq!(reads(whole), Ok(canonical(&shared).unwrap()), "{whole}");
        }
    }

    /// The review of v1.1.129's skills (t-98860, P2-3): the junction `cmd`
    /// makes on Windows gets each path in quotes of its own, so a `&` in a
    /// folder's name stays a name; a path `cmd` would read into anyway — a
    /// `%name%`, a quote — is refused before `cmd` starts. `cmd` runs it
    /// with no AutoRun command and no delayed expansion, which would read a
    /// `!name!` inside the quotes (the second review, t-99089, P3-E).
    #[test]
    fn a_junction_line_quotes_each_path_and_refuses_what_cmd_would_read() {
        assert_eq!(
            junction_line(
                Path::new(r"C:\work\R&D!x!\skill"),
                Path::new(r"C:\store\foo")
            ),
            Ok(r#"/D /V:OFF /C mklink /J "C:\store\foo" "C:\work\R&D!x!\skill""#.to_string())
        );
        for bad in [r"C:\Users\%USERNAME%\skill", r#"C:\a"b"#, "C:\\a\nb"] {
            assert_eq!(
                junction_line(Path::new(bad), Path::new(r"C:\store\foo")),
                Err("library.linkNotMade".to_string()),
                "{bad}"
            );
            assert_eq!(
                junction_line(Path::new(r"C:\store\foo"), Path::new(bad)),
                Err("library.linkNotMade".to_string()),
                "{bad}"
            );
        }
    }

    /// The review of 4ca485472 (P2): a link inside a skill that reaches out of
    /// it (`common -> ../_common`) still finds what it reached once the skill
    /// is in the store; one inside the skill stays as it was written.
    #[cfg(unix)]
    #[test]
    fn a_relative_link_out_of_a_skill_still_finds_its_folder_from_the_store() {
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let skills = home.path().join(".kiro/skills");
        let shared = skills.join("_common");
        std::fs::create_dir_all(&shared).unwrap();
        std::fs::write(shared.join("style.md"), "shared\n").unwrap();
        let k = skill_at(&skills, "k");
        std::os::unix::fs::symlink("../_common", k.join("common")).unwrap();
        std::os::unix::fs::symlink("SKILL.md", k.join("readme")).unwrap();
        let ctx = context(home.path(), data.path());
        look(&ctx).unwrap();
        let copied = ctx.store_skills().join("k");
        assert_eq!(
            std::fs::read_to_string(copied.join("common/style.md")).unwrap(),
            "shared\n"
        );
        assert_eq!(
            std::fs::read_link(copied.join("readme")).unwrap(),
            PathBuf::from("SKILL.md")
        );
    }

    /// The review of the skills store (P3-4): the window's own Claude home
    /// reads the store — on Windows too, through a junction where a link is
    /// not allowed — and a skill that stood in it moves into the store.
    #[test]
    fn the_windows_claude_home_reads_the_store() {
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let claude = data.path().join("claude-home");
        skill_at(&claude.join("skills"), "s");
        let ctx = Context {
            claude_home: Some(claude.clone()),
            ..context(home.path(), data.path())
        };
        look(&ctx).unwrap();
        assert!(claude_reads_store(&ctx));
        assert!(ctx.store_skills().join("s/SKILL.md").is_file());
        assert!(claude.join("skills/s/SKILL.md").is_file());
        assert!(
            read_log(&ctx)
                .iter()
                .any(|line| line.kind == "linked" && line.who == "claude")
        );
    }

    /// The review of v1.1.129's skills (t-98860, P3-2): where the link the
    /// window's Claude would read the store through cannot be made (a drive
    /// that takes neither a link nor a junction; here, its place beside the
    /// folder is taken), nothing moves out of that Claude's `skills`, the line
    /// says so once, and later looks do not try again.
    #[test]
    fn a_claude_home_link_that_cannot_be_made_moves_nothing_and_is_said_once() {
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let claude = data.path().join("claude-home");
        skill_at(&claude.join("skills"), "s");
        std::fs::create_dir_all(claude.join("skills.linking/taken")).unwrap();
        let ctx = Context {
            claude_home: Some(claude.clone()),
            ..context(home.path(), data.path())
        };
        look(&ctx).unwrap();
        look(&ctx).unwrap();
        assert!(!claude_reads_store(&ctx));
        assert!(
            claude.join("skills/s/SKILL.md").is_file(),
            "the window's Claude lost its skill"
        );
        assert!(!is_link(&claude.join("skills")));
        let said = read_log(&ctx)
            .into_iter()
            .filter(|line| line.kind == "failed" && line.who == "claude")
            .collect::<Vec<_>>();
        assert_eq!(said.len(), 1, "{said:?}");
        assert_eq!(said[0].detail, "library.linkNotMade");
    }

    /// The second review of v1.1.129's skills (t-99089, P3-C ①): a link that
    /// could not be made a day ago — a scanner held the folder, `cmd` would
    /// not start — is tried again, so the window's Claude is not left apart
    /// from the store for good.
    #[test]
    fn a_claude_home_link_refused_a_day_ago_is_tried_again() {
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let claude = data.path().join("claude-home");
        skill_at(&claude.join("skills"), "s");
        let ctx = Context {
            claude_home: Some(claude.clone()),
            ..context(home.path(), data.path())
        };
        let refused = ctx.folder().join(CLAUDE_UNLINKED);
        std::fs::create_dir_all(ctx.folder()).unwrap();
        std::fs::write(&refused, "").unwrap();
        std::fs::File::options()
            .write(true)
            .open(&refused)
            .unwrap()
            .set_modified(std::time::SystemTime::now() - Duration::from_secs(2 * 24 * 60 * 60))
            .unwrap();
        look(&ctx).unwrap();
        assert!(claude_reads_store(&ctx), "the refusal of then still stands");
        assert!(ctx.store_skills().join("s/SKILL.md").is_file());
        assert!(std::fs::symlink_metadata(&refused).is_err());
    }

    /// The second review of v1.1.129's skills (t-99089, P3-C ②): a link left
    /// beside the window's Claude `skills` by a look that could not finish
    /// the move, already reading the store, is used as it is — not removed
    /// and made again on every look (on Windows, a `cmd` each time).
    #[cfg(unix)]
    #[test]
    fn a_claude_home_link_left_reading_the_store_is_used_as_it_is() {
        use std::os::unix::fs::MetadataExt as _;
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let claude = data.path().join("claude-home");
        skill_at(&claude.join("skills"), "s");
        let ctx = Context {
            claude_home: Some(claude.clone()),
            ..context(home.path(), data.path())
        };
        std::fs::create_dir_all(ctx.store_skills()).unwrap();
        let linking = claude.join(CLAUDE_LINKING);
        std::os::unix::fs::symlink(ctx.store_skills(), &linking).unwrap();
        let left = std::fs::symlink_metadata(&linking).unwrap().ino();
        look(&ctx).unwrap();
        assert!(claude_reads_store(&ctx));
        assert_eq!(
            std::fs::symlink_metadata(claude.join("skills"))
                .unwrap()
                .ino(),
            left,
            "the link left reading the store was made again"
        );
    }

    /// The review of the skills store (P3-4): a `.claude.json` whose size and
    /// time stand is parsed once — the stamp's read and the look's read of
    /// one burst share it. Shown here by a text changed behind an unchanged
    /// size and time, which only a kept parse reads as before; a size or a
    /// time that moved is read anew.
    #[test]
    fn a_claude_list_whose_size_and_time_stand_is_parsed_once() {
        let home = tempfile::tempdir().unwrap();
        let file = home.path().join(MAC_CLAUDE_MCP);
        std::fs::write(&file, r#"{"mcpServers":{"a":{"command":"a-mcp"}}}"#).unwrap();
        let at = std::fs::metadata(&file).unwrap().modified().unwrap();
        in_one_burst(|| {
            assert!(read_list(&file, Format::Claude).unwrap().contains_key("a"));
            std::fs::write(&file, r#"{"mcpServers":{"b":{"command":"b-mcp"}}}"#).unwrap();
            std::fs::File::options()
                .write(true)
                .open(&file)
                .unwrap()
                .set_modified(at)
                .unwrap();
            assert!(read_list(&file, Format::Claude).unwrap().contains_key("a"));
            std::fs::write(&file, r#"{"mcpServers":{"bb":{"command":"b-mcp"}}}"#).unwrap();
            assert!(read_list(&file, Format::Claude).unwrap().contains_key("bb"));
        });
    }

    /// The review of v1.1.129's skills (t-98860, P3-4): a `.claude.json`
    /// Claude Code rewrote at the same size within one tick of a coarse clock
    /// (HFS+, FAT, a network share) is read anew by the next read outside
    /// the burst that parsed it — the kept parse lives for one burst only.
    #[test]
    fn a_claude_list_rewritten_at_the_same_size_and_time_is_read_anew_after_its_burst() {
        let home = tempfile::tempdir().unwrap();
        let file = home.path().join(MAC_CLAUDE_MCP);
        std::fs::write(&file, r#"{"mcpServers":{"a":{"command":"a-mcp"}}}"#).unwrap();
        let at = std::fs::metadata(&file).unwrap().modified().unwrap();
        assert!(read_list(&file, Format::Claude).unwrap().contains_key("a"));
        std::fs::write(&file, r#"{"mcpServers":{"b":{"command":"b-mcp"}}}"#).unwrap();
        std::fs::File::options()
            .write(true)
            .open(&file)
            .unwrap()
            .set_modified(at)
            .unwrap();
        assert!(read_list(&file, Format::Claude).unwrap().contains_key("b"));
    }

    /// The second review of v1.1.129's skills (t-99089, P3-D): within one
    /// burst, the read after a write to the window's Claude list is the file
    /// as written — never the parse of before, which a rewrite at the same
    /// size within one tick of a coarse clock would leave standing. The
    /// fake `claude` here writes the new value and sets the old time back.
    #[cfg(unix)]
    #[test]
    fn a_write_to_the_claude_list_is_checked_against_the_file_as_written() {
        use std::os::unix::fs::PermissionsExt as _;
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let claude = data.path().join("claude-home");
        std::fs::create_dir_all(&claude).unwrap();
        let file = claude.join(".claude.json");
        let next = data.path().join("next.json");
        let stamp = data.path().join("stamp");
        std::fs::write(&file, r#"{"mcpServers":{"a":{"command":"x-mcp"}}}"#).unwrap();
        std::fs::write(&next, r#"{"mcpServers":{"a":{"command":"y-mcp"}}}"#).unwrap();
        std::fs::write(&stamp, "").unwrap();
        let at = std::time::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        for path in [&file, &stamp] {
            std::fs::File::options()
                .write(true)
                .open(path)
                .unwrap()
                .set_modified(at)
                .unwrap();
        }
        let bin = data.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let fake = bin.join("claude");
        std::fs::write(
            &fake,
            format!(
                "#!/bin/sh\n[ \"$2\" = add-json ] || exit 0\n/bin/cp '{}' '{}' && /usr/bin/touch -r '{}' '{}'\n",
                next.display(),
                file.display(),
                stamp.display(),
                file.display(),
            ),
        )
        .unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let ctx = Context {
            claude_home: Some(claude.clone()),
            path: Some(bin.into_os_string()),
            ..context(home.path(), data.path())
        };
        let entry = mcp::read_text(Format::Claude, &std::fs::read_to_string(&next).unwrap())
            .unwrap()
            .remove("a")
            .unwrap();
        let steps = [McpStep::Push {
            agent: "claude".to_string(),
            name: "a".to_string(),
            entry: entry.clone(),
        }];
        let mut ledger = Ledger::new();
        let mut log = Vec::new();
        let failures = in_one_burst(|| run_mcp(&ctx, &steps, &mut ledger, &mut log));
        assert_eq!(failures, 0, "{log:?}");
        assert_eq!(
            std::fs::metadata(&file).unwrap().modified().unwrap(),
            at,
            "the fake claude did not set the old time back"
        );
        assert_eq!(
            ledger.mcp["a"].seen.get("claude"),
            Some(&entry.def.fingerprint())
        );
    }

    /// The review of the skills store (P3-3): OpenCode keeps its skills
    /// under the XDG config home, as it keeps its MCP list — a person who
    /// moved that home sees one folder for both.
    #[test]
    fn opencode_skills_follow_the_xdg_config_home() {
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let config = home.path().join("elsewhere");
        let ctx = Context {
            config_home: config.clone(),
            ..context_of(home.path(), data.path(), &["opencode"])
        };
        skill_at(&config.join("opencode/skills"), "x");
        skill_at(&home.path().join(".config/opencode/skills"), "y");
        look(&ctx).unwrap();
        assert!(ctx.store_skills().join("x/SKILL.md").is_file());
        assert!(!ctx.store_skills().join("y").exists());
        assert_eq!(
            ctx.mcp_file(AGENTS.iter().find(|one| one.id == "opencode").unwrap()),
            Some(config.join("opencode/opencode.json"))
        );
    }

    /// The review of skills-p2-r2 (P3-4): a link to a sibling skill
    /// (`ref -> ../b/lib`) stays as it was written — the store holds `b`
    /// beside it too — instead of reading the agent's folder by its whole path.
    #[cfg(unix)]
    #[test]
    fn a_relative_link_to_a_sibling_skill_reads_the_stores_sibling() {
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let skills = home.path().join(".kiro/skills");
        let b = skill_at(&skills, "b");
        std::fs::create_dir_all(b.join("lib")).unwrap();
        std::fs::write(b.join("lib/x.md"), "b's\n").unwrap();
        let a = skill_at(&skills, "a");
        std::os::unix::fs::symlink("../b/lib", a.join("ref")).unwrap();
        let ctx = context(home.path(), data.path());
        look(&ctx).unwrap();
        let copied = ctx.store_skills().join("a");
        assert_eq!(
            std::fs::read_link(copied.join("ref")).unwrap(),
            PathBuf::from("../b/lib")
        );
        assert_eq!(
            std::fs::read_to_string(copied.join("ref/x.md")).unwrap(),
            "b's\n"
        );
    }

    /// The review of skills-p2 (P3-6), Windows: a link to a file inside a
    /// skill reaches the store — as a link, or as the file it reads where the
    /// window may not make links — instead of being left out without a word.
    /// Where the test cannot make the link it starts from, there is nothing
    /// to copy and nothing to check.
    #[cfg(not(unix))]
    #[test]
    fn a_file_link_inside_a_skill_reaches_the_store_on_windows() {
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let k = skill_at(&home.path().join(".kiro/skills"), "k");
        std::fs::write(k.join("notes.md"), "notes\n").unwrap();
        if std::os::windows::fs::symlink_file("notes.md", k.join("readme")).is_err() {
            return;
        }
        let ctx = context(home.path(), data.path());
        look(&ctx).unwrap();
        assert_eq!(
            std::fs::read_to_string(ctx.store_skills().join("k/readme")).unwrap(),
            "notes\n"
        );
    }

    /// The review of v1.1.129's skills (t-98860, P3-3), Windows: a link to a
    /// folder inside a skill reaches the store as a junction to the whole
    /// path it reached where the window may not make links — it no longer
    /// stops the skill's copy. The test's own link is a junction, which asks
    /// for no privilege.
    #[cfg(not(unix))]
    #[test]
    fn a_folder_link_inside_a_skill_reaches_the_store_on_windows() {
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let skills = home.path().join(".kiro/skills");
        let shared = skills.join("_common");
        std::fs::create_dir_all(&shared).unwrap();
        std::fs::write(shared.join("style.md"), "shared\n").unwrap();
        let k = skill_at(&skills, "k");
        link_folder(&canonical(&shared).unwrap(), &k.join("common")).unwrap();
        let ctx = context(home.path(), data.path());
        look(&ctx).unwrap();
        assert_eq!(
            std::fs::read_to_string(ctx.store_skills().join("k/common/style.md")).unwrap(),
            "shared\n"
        );
    }

    /// The same review (P3-3), Windows: a link to a file is counted at the
    /// size of what it reads, since it may be copied as that file — two
    /// links to one file past half the bound refuse the skill. Where the test
    /// cannot make the links it starts from, there is nothing to check.
    #[cfg(not(unix))]
    #[test]
    fn file_links_inside_a_skill_count_toward_the_copy_bound_on_windows() {
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let big = home.path().join("big.bin");
        std::fs::File::create(&big)
            .unwrap()
            .set_len(SKILL_COPY_BYTES / 2 + 1)
            .unwrap();
        let k = skill_at(&home.path().join(".kiro/skills"), "k");
        for name in ["a.bin", "b.bin"] {
            if std::os::windows::fs::symlink_file(&big, k.join(name)).is_err() {
                return;
            }
        }
        let ctx = context(home.path(), data.path());
        look(&ctx).unwrap();
        assert!(!ctx.store_skills().join("k").exists());
        assert!(read_log(&ctx).iter().any(|line| line.kind == "failed"
            && line.name == "k"
            && line.detail == "library.skillTooLarge"));
    }

    /// The review of v1.1.128 (P2-3): a folder holding far more than a skill
    /// does (a repository with its `.git` and `node_modules`) is not copied —
    /// the line says why — and the agent's folder is left as it was.
    #[test]
    fn a_skill_folder_past_the_copy_bound_is_not_copied() {
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let big = skill_at(&home.path().join(".kiro/skills"), "big");
        // More entries than the copy's bound lets through.
        std::fs::create_dir_all(big.join("node_modules")).unwrap();
        for at in 0..=SKILL_COPY_ENTRIES {
            std::fs::write(big.join(format!("node_modules/{at}.js")), "x").unwrap();
        }
        let ctx = context(home.path(), data.path());
        look(&ctx).unwrap();
        assert!(!ctx.store_skills().join("big").exists());
        assert!(big.join("SKILL.md").is_file());
        let log = read_log(&ctx);
        assert!(
            log.iter().any(|line| line.kind == "failed"
                && line.name == "big"
                && line.detail == "library.skillTooLarge"),
            "{log:?}"
        );
    }

    /// The review of v1.1.129's skills (t-98860, P3-6): the bound by bytes —
    /// a skill folder holding one file past [`SKILL_COPY_BYTES`] is not
    /// copied. The file is sparse: its length is set, next to no disk is
    /// written.
    #[test]
    fn a_skill_folder_past_the_copy_bound_in_bytes_is_not_copied() {
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let big = skill_at(&home.path().join(".kiro/skills"), "big");
        std::fs::File::create(big.join("model.bin"))
            .unwrap()
            .set_len(SKILL_COPY_BYTES + 1)
            .unwrap();
        let ctx = context(home.path(), data.path());
        look(&ctx).unwrap();
        assert!(!ctx.store_skills().join("big").exists());
        assert!(big.join("SKILL.md").is_file());
        let log = read_log(&ctx);
        assert!(
            log.iter().any(|line| line.kind == "failed"
                && line.name == "big"
                && line.detail == "library.skillTooLarge"),
            "{log:?}"
        );
    }

    /// The review of v1.1.128 (P3-1): a copy that follows its source and
    /// fails leaves the store's text, and the record says so — the next look
    /// follows again instead of reading the store as the person's own edit.
    #[test]
    fn a_follow_that_failed_is_tried_again() {
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let z = skill_at(&home.path().join(".zo/skills"), "z");
        let ctx = context_of(home.path(), data.path(), &["zo"]);
        look(&ctx).unwrap();
        let store = ctx.store_skills();
        let copied = std::fs::read_to_string(store.join("z/SKILL.md")).unwrap();
        std::fs::create_dir_all(z.join("node_modules")).unwrap();
        for at in 0..=SKILL_COPY_ENTRIES {
            std::fs::write(z.join(format!("node_modules/{at}.js")), "x").unwrap();
        }
        look(&ctx).unwrap();
        look(&ctx).unwrap();
        assert_eq!(
            std::fs::read_to_string(store.join("z/SKILL.md")).unwrap(),
            copied
        );
        let failed = read_log(&ctx)
            .iter()
            .filter(|line| line.kind == "failed" && line.name == "z")
            .count();
        assert_eq!(failed, 2, "the follow was not tried again");
    }

    /// The review of v1.1.128 (P2-4): a store folder the look cannot read is
    /// not an empty store — no skill is read as removed, and the skills step
    /// waits for a look that can read it.
    #[cfg(unix)]
    #[test]
    fn a_store_folder_that_cannot_be_read_removes_no_skill() {
        use std::os::unix::fs::PermissionsExt as _;
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        skill_at(&home.path().join(".kiro/skills"), "k");
        let ctx = context(home.path(), data.path());
        look(&ctx).unwrap();
        let store = ctx.store_skills();
        assert!(store.join("k/SKILL.md").is_file());
        std::fs::set_permissions(&store, std::fs::Permissions::from_mode(0o000)).unwrap();
        let looked = look(&ctx);
        std::fs::set_permissions(&store, std::fs::Permissions::from_mode(0o755)).unwrap();
        looked.unwrap();
        let ledger = read_ledger(&ctx);
        assert!(ledger.skills.contains_key("k"), "{ledger:?}");
        assert!(ledger.gone_skills.is_empty(), "{ledger:?}");
        assert!(!read_log(&ctx).iter().any(|line| line.kind == "removed"));
    }

    /// The review of v1.1.128 (P3-7): the store's link to a project the
    /// person writes a skill in reads nothing for a while (the project moved,
    /// its volume let go). The skill is not read as removed — the store is
    /// read as holding what it last held — and once the folder is back, so
    /// is the skill.
    #[cfg(unix)]
    #[test]
    fn a_store_link_that_reads_nothing_for_a_while_removes_no_skill() {
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let project = skill_at(&home.path().join("dev"), "foo");
        std::fs::create_dir_all(home.path().join(".claude/skills")).unwrap();
        std::os::unix::fs::symlink(&project, home.path().join(".claude/skills/foo")).unwrap();
        let ctx = context(home.path(), data.path());
        look(&ctx).unwrap();
        let away = home.path().join("dev/away");
        std::fs::rename(&project, &away).unwrap();
        look(&ctx).unwrap();
        let ledger = read_ledger(&ctx);
        assert!(ledger.skills.contains_key("foo"), "{ledger:?}");
        assert!(ledger.gone_skills.is_empty(), "{ledger:?}");
        assert!(!read_log(&ctx).iter().any(|line| line.kind == "removed"));
        std::fs::rename(&away, &project).unwrap();
        look(&ctx).unwrap();
        assert!(ctx.store_skills().join("foo/SKILL.md").is_file());
        assert!(!read_ledger(&ctx).gone_skills.contains_key("foo"));
    }

    /// The review of v1.1.129's skills (t-98860, P3-5): the store's folder
    /// is a link that reads nothing for a while (its volume let go). That is
    /// not an empty store: no skill is read as removed, and the skills step
    /// waits for a look that can read it.
    #[cfg(unix)]
    #[test]
    fn a_store_folder_link_that_reads_nothing_removes_no_skill() {
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let k = skill_at(&home.path().join(".kiro/skills"), "k");
        let ctx = context(home.path(), data.path());
        look(&ctx).unwrap();
        let store = ctx.store_skills();
        let away = home.path().join("away");
        std::fs::rename(&store, &away).unwrap();
        std::os::unix::fs::symlink(&away, &store).unwrap();
        assert!(store.join("k/SKILL.md").is_file());
        std::fs::rename(&away, home.path().join("gone")).unwrap();
        look(&ctx).unwrap();
        let ledger = read_ledger(&ctx);
        assert!(ledger.skills.contains_key("k"), "{ledger:?}");
        assert!(ledger.gone_skills.is_empty(), "{ledger:?}");
        assert!(!read_log(&ctx).iter().any(|line| line.kind == "removed"));
        assert!(k.join("SKILL.md").is_file(), "Kiro lost the skill");
    }

    /// The review of v1.1.129's skills (t-98860, P3-8): a store skill that
    /// is a link is removed itself — it holds no data, and a Trash may follow
    /// a junction — never sent to the Trash; 「되돌리기」 makes the same link
    /// again. The folder it reads stays.
    #[cfg(unix)]
    #[test]
    fn removing_a_linked_skill_removes_the_link_and_undo_makes_it_again() {
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let real = skill_at(&home.path().join("proj"), "foo");
        let ctx = context(home.path(), data.path());
        let store = ctx.store_skills();
        std::fs::create_dir_all(&store).unwrap();
        std::os::unix::fs::symlink(&real, store.join("foo")).unwrap();
        look(&ctx).unwrap();
        remove_skill(&ctx, "foo", false).unwrap();
        assert!(std::fs::symlink_metadata(store.join("foo")).is_err());
        assert!(real.join("SKILL.md").is_file(), "the folder it read went");
        let bin = data.path().join("trash");
        assert!(
            std::fs::read_dir(&bin).map_or(true, |mut read| read.next().is_none()),
            "the link went to the Trash"
        );
        let deleted = report(&ctx)
            .log
            .into_iter()
            .find(|row| row.line.kind == "deleted" && row.line.name == "foo")
            .unwrap();
        undo(&ctx, deleted.line.id).unwrap();
        assert_eq!(std::fs::read_link(store.join("foo")).unwrap(), real);
    }

    /// The review of v1.1.128 (P3-4): a skill name the store could not hold
    /// is refused before anything is written.
    #[test]
    fn choosing_a_skill_by_a_name_the_store_cannot_hold_is_refused() {
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let ctx = context(home.path(), data.path());
        assert_eq!(
            choose_skill(&ctx, "../x", None).err().as_deref(),
            Some("library.invalidName")
        );
        assert!(!read_ledger(&ctx).skills.contains_key("../x"));
    }

    /// A copy of a shipped skill in a Native agent's own folder goes to the
    /// Trash only when the whole folder is ours (review t-106726, fourth): a
    /// shipped `SKILL.md` beside a file the person added stays where it is,
    /// and is asked about; the same text alone goes.
    #[test]
    fn a_native_agents_copy_with_a_file_of_the_persons_stays_and_is_asked() {
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let shipped = |name: &str| zerocode_core::skill_install::bundled_skill(name).unwrap();
        for name in ["computer-use", "dataviz"] {
            for root in [".agents/skills", ".gemini/skills"] {
                let dir = home.path().join(root).join(name);
                std::fs::create_dir_all(&dir).unwrap();
                std::fs::write(
                    dir.join(zerocode_core::skill::SKILL_FILE),
                    shipped(name).content,
                )
                .unwrap();
            }
        }
        let added = home.path().join(".gemini/skills/computer-use");
        std::fs::write(added.join("notes.md"), "mine").unwrap();
        let ctx = context_of(home.path(), data.path(), &["gemini"]);
        look(&ctx).unwrap();
        assert_eq!(
            std::fs::read_to_string(added.join("notes.md")).unwrap(),
            "mine",
            "a folder with the person's file went"
        );
        let asked = &read_ledger(&ctx).skills["computer-use"].pick;
        assert_eq!(asked.len(), 1, "{asked:?}");
        assert_eq!(asked[0].agent, "gemini");
        assert!(
            std::fs::symlink_metadata(home.path().join(".gemini/skills/dataviz")).is_err(),
            "a copy of ours alone stayed"
        );
    }

    /// A copy whose reference file a person changed a letter of — the same
    /// size, so the digest agrees with the store's — stays and is asked
    /// about; a copy the same byte for byte still goes (review t-106726,
    /// fourth, the rest).
    #[test]
    fn a_native_agents_copy_changed_at_the_same_size_stays_and_is_asked() {
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        for name in ["notes-a", "notes-b"] {
            for root in [".agents/skills", ".gemini/skills"] {
                let dir = home.path().join(root).join(name);
                std::fs::create_dir_all(&dir).unwrap();
                std::fs::write(
                    dir.join(zerocode_core::skill::SKILL_FILE),
                    format!("---\nname: {name}\n---\nbody\n"),
                )
                .unwrap();
                std::fs::write(dir.join("verbs.md"), "verbs: ask, tell\n").unwrap();
            }
        }
        let changed = home.path().join(".gemini/skills/notes-a");
        std::fs::write(changed.join("verbs.md"), "verbs: ask, sell\n").unwrap();
        let ctx = context_of(home.path(), data.path(), &["gemini"]);
        look(&ctx).unwrap();
        assert_eq!(
            std::fs::read_to_string(changed.join("verbs.md")).unwrap(),
            "verbs: ask, sell\n",
            "a copy changed at the same size went"
        );
        let asked = &read_ledger(&ctx).skills["notes-a"].pick;
        assert_eq!(asked.len(), 1, "{asked:?}");
        assert_eq!(asked[0].agent, "gemini");
        assert!(
            std::fs::symlink_metadata(home.path().join(".gemini/skills/notes-b")).is_err(),
            "a copy the same byte for byte stayed"
        );
    }

    /// 「저장소 것으로 맞추기」 (2026-11-13): Gemini CLI reads the store by
    /// itself, and an old text of a shipped skill in its own folder is a
    /// question. The person's press keeps that copy under the keeper's own
    /// folder, sends it to the Trash and closes the question; 「되돌리기」
    /// brings it back. An agent that does not read the store, and a name
    /// with no question, are refused with nothing moved.
    #[test]
    fn setting_a_native_agents_copy_to_the_stores_keeps_it_and_takes_it_out() {
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let store = home.path().join(".agents/skills/computer-use");
        std::fs::create_dir_all(&store).unwrap();
        std::fs::write(
            store.join(zerocode_core::skill::SKILL_FILE),
            "---\nname: computer-use\n---\nstore\n",
        )
        .unwrap();
        let copy = home.path().join(".gemini/skills/computer-use");
        std::fs::create_dir_all(&copy).unwrap();
        std::fs::write(
            copy.join(zerocode_core::skill::SKILL_FILE),
            "---\nname: computer-use\n---\nmine\n",
        )
        .unwrap();
        let ctx = context_of(home.path(), data.path(), &["gemini", "kiro"]);
        look(&ctx).unwrap();
        let asked = &read_ledger(&ctx).skills["computer-use"].pick;
        assert_eq!(asked.len(), 1, "{asked:?}");
        assert_eq!(asked[0].agent, "gemini");

        assert_eq!(
            align_skill(&ctx, "computer-use", "kiro").err().as_deref(),
            Some("library.notFound")
        );
        assert_eq!(
            align_skill(&ctx, "other", "gemini").err().as_deref(),
            Some("library.notFound")
        );
        assert!(
            copy.join(zerocode_core::skill::SKILL_FILE).is_file(),
            "a refusal moved the copy"
        );

        let answered = align_skill(&ctx, "computer-use", "gemini").unwrap();
        assert!(
            std::fs::symlink_metadata(&copy).is_err(),
            "the copy is still there"
        );
        let kept: Vec<PathBuf> = std::fs::read_dir(data.path().join("library").join(ALIGNED))
            .unwrap()
            .flatten()
            .map(|entry| {
                entry
                    .path()
                    .join("0/computer-use")
                    .join(zerocode_core::skill::SKILL_FILE)
            })
            .collect();
        assert_eq!(kept.len(), 1);
        assert_eq!(
            std::fs::read_to_string(&kept[0]).unwrap(),
            "---\nname: computer-use\n---\nmine\n"
        );
        assert!(read_ledger(&ctx).skills["computer-use"].pick.is_empty());
        assert!(
            answered.skills.iter().all(|row| row.pick.is_empty()),
            "the question is still asked"
        );
        assert_eq!(
            std::fs::read_to_string(store.join(zerocode_core::skill::SKILL_FILE)).unwrap(),
            "---\nname: computer-use\n---\nstore\n",
            "the store's text moved"
        );
        let aligned = answered
            .log
            .iter()
            .find(|row| row.line.kind == "aligned")
            .expect("the log says what was done");
        assert_eq!(aligned.line.who, "gemini");
        undo(&ctx, aligned.line.id).unwrap();
        assert!(
            copy.join(zerocode_core::skill::SKILL_FILE).is_file(),
            "「되돌리기」 brings it back"
        );
    }

    /// A skill Kiro installs is copied (Kiro reads only its own folder); the
    /// person's own Claude home is read and never changed.
    #[test]
    fn a_skill_is_taken_into_the_store_without_touching_the_persons_claude() {
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        for (dir, name) in [(".kiro/skills", "k"), (".claude/skills", "m")] {
            let at = home.path().join(dir).join(name);
            std::fs::create_dir_all(&at).unwrap();
            std::fs::write(at.join("SKILL.md"), format!("---\nname: {name}\n---\n")).unwrap();
        }
        let ctx = context(home.path(), data.path());
        look(&ctx).unwrap();
        let store = ctx.store_skills();
        assert!(store.join("k/SKILL.md").is_file() && store.join("m/SKILL.md").is_file());
        assert!(
            home.path().join(".kiro/skills/k/SKILL.md").is_file(),
            "an agent that reads only its own folder lost it"
        );
        assert!(
            home.path().join(".claude/skills/m/SKILL.md").is_file(),
            "the person's Claude home was changed"
        );
        assert!(!look(&ctx).unwrap().changed);
    }

    #[test]
    fn the_person_adds_switches_and_removes_a_server() {
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(home.path().join(".kiro/settings")).unwrap();
        std::fs::create_dir_all(home.path().join(".cursor")).unwrap();
        let ctx = context(home.path(), data.path());
        let def = McpDef::Stdio {
            command: "npx".into(),
            args: vec!["-y".into(), "@acme/mcp".into()],
            env: [("ACME_TOKEN".to_string(), "s3cret".to_string())].into(),
        };
        let said = save_server(&ctx, "acme", def, None, None).unwrap();
        let row = said.servers.iter().find(|row| row.name == "acme").unwrap();
        assert_eq!(row.secrets, vec!["ACME_TOKEN".to_string()]);
        assert!(
            !serde_json::to_string(&said).unwrap().contains("s3cret"),
            "a value reached the page"
        );
        assert!(
            read_list(&home.path().join(".cursor/mcp.json"), Format::Cursor)
                .unwrap()
                .contains_key("acme")
        );
        assert!(
            !std::fs::read_to_string(data.path().join("library/log.json"))
                .unwrap()
                .contains("s3cret")
        );
        switch_server_for(&ctx, "acme", "cursor", false).unwrap();
        assert!(
            !read_list(&home.path().join(".cursor/mcp.json"), Format::Cursor)
                .unwrap()
                .contains_key("acme")
        );
        assert!(
            read_list(&home.path().join(".kiro/settings/mcp.json"), Format::Kiro)
                .unwrap()
                .contains_key("acme")
        );
        switch_server(&ctx, "acme", false).unwrap();
        assert!(
            !read_list(&home.path().join(".kiro/settings/mcp.json"), Format::Kiro)
                .unwrap()
                .contains_key("acme")
        );
        switch_server(&ctx, "acme", true).unwrap();
        let report = remove_server_everywhere(&ctx, "acme").unwrap();
        assert!(
            !read_list(&home.path().join(".kiro/settings/mcp.json"), Format::Kiro)
                .unwrap()
                .contains_key("acme")
        );
        let deleted = report
            .log
            .iter()
            .find(|row| row.line.kind == "deleted")
            .unwrap();
        assert!(deleted.undo);
        undo(&ctx, deleted.line.id).unwrap();
        assert!(
            read_list(&home.path().join(".kiro/settings/mcp.json"), Format::Kiro)
                .unwrap()
                .contains_key("acme")
        );
    }

    #[test]
    fn a_list_that_cannot_be_read_is_never_written() {
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(home.path().join(".kiro/settings")).unwrap();
        std::fs::create_dir_all(home.path().join(".cursor")).unwrap();
        std::fs::write(
            home.path().join(".kiro/settings/mcp.json"),
            r#"{"mcpServers":{"a":{"command":"a-mcp"}}}"#,
        )
        .unwrap();
        std::fs::write(home.path().join(".cursor/mcp.json"), "{ broken").unwrap();
        let ctx = context(home.path(), data.path());
        look(&ctx).unwrap();
        assert_eq!(
            std::fs::read_to_string(home.path().join(".cursor/mcp.json")).unwrap(),
            "{ broken"
        );
        let report = report(&ctx);
        assert_eq!(
            report
                .agents
                .iter()
                .find(|row| row.id == "cursor")
                .unwrap()
                .mcp_state,
            "unreadable"
        );
    }

    /// Codex reads the store wherever it runs, so its skill moves there; zo
    /// reads it only in the window's terminals, so its folder keeps the skill
    /// and the store keeps a copy. An edit the person makes in the store's
    /// copy is no question about zo's unchanged folder, and a skill the person
    /// removes is not taken back from it — and comes back from the Trash,
    /// which here is the test's own folder ([`fake_trash`]).
    #[test]
    fn a_codex_skill_moves_into_the_store_and_a_zo_skill_is_copied() {
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        for (dir, name) in [(".codex/skills", "a"), (".zo/skills", "z")] {
            let at = home.path().join(dir).join(name);
            std::fs::create_dir_all(&at).unwrap();
            std::fs::write(at.join("SKILL.md"), format!("---\nname: {name}\n---\n")).unwrap();
        }
        let ctx = context_of(home.path(), data.path(), &["codex", "zo"]);
        look(&ctx).unwrap();
        let store = ctx.store_skills();
        assert!(store.join("a/SKILL.md").is_file() && store.join("z/SKILL.md").is_file());
        assert!(
            !home.path().join(".codex/skills/a").exists(),
            "Codex reads the store wherever it runs, so its copy moved"
        );
        let zo_copy = home.path().join(".zo/skills/z/SKILL.md");
        assert!(
            zo_copy.is_file(),
            "zo's folder lost a skill a terminal outside the window reads"
        );
        assert!(!look(&ctx).unwrap().changed);

        std::fs::write(store.join("z/SKILL.md"), "---\nname: z\n---\nmine\n").unwrap();
        look(&ctx).unwrap();
        let said = report(&ctx);
        assert!(
            said.skills
                .iter()
                .find(|row| row.name == "z")
                .unwrap()
                .pick
                .is_empty(),
            "an edit in the store asked about the unchanged folder it came from"
        );
        assert!(
            std::fs::read_to_string(store.join("z/SKILL.md"))
                .unwrap()
                .ends_with("mine\n")
        );

        let removed = remove_skill(&ctx, "z", false).unwrap();
        look(&ctx).unwrap();
        assert!(!store.join("z").exists(), "a removed skill came back");
        assert!(zo_copy.is_file());
        let trashed = data.path().join("trash").read_dir().unwrap().count();
        assert_eq!(trashed, 1, "the removed skill is not in the test's Trash");
        let deleted = removed
            .log
            .iter()
            .find(|row| row.line.kind == "deleted" && row.line.name == "z")
            .unwrap();
        undo(&ctx, deleted.line.id).unwrap();
        assert!(store.join("z/SKILL.md").is_file());
    }

    /// A look with nothing to do leaves the disk alone, and the stamp a
    /// watched round compares stays the same while only what a look ignores
    /// moves — another key in an agent's list, the way Claude Code rewrites
    /// its own config.
    #[test]
    fn a_look_that_finds_everything_in_step_leaves_the_disk_alone() {
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(home.path().join(".kiro/settings")).unwrap();
        std::fs::create_dir_all(home.path().join(".cursor")).unwrap();
        let kiro = home.path().join(".kiro/settings/mcp.json");
        std::fs::write(&kiro, r#"{"mcpServers":{"a":{"command":"a-mcp"}}}"#).unwrap();
        let ctx = context(home.path(), data.path());
        let before = inputs_stamp(&ctx);
        look(&ctx).unwrap();
        let ledger = data.path().join("library/ledger.json");
        let written = std::fs::read(&ledger).unwrap();
        let stamp = inputs_stamp(&ctx);
        assert!(
            stamp.is_some() && stamp != before,
            "the look's own writes moved nothing the stamp reads"
        );
        assert_eq!(inputs_stamp(&ctx), stamp);
        std::fs::remove_file(&ledger).unwrap();
        std::fs::write(&ledger, &written).unwrap();
        let held = std::fs::metadata(&ledger).unwrap().modified().unwrap();
        assert!(!look(&ctx).unwrap().changed);
        assert_eq!(
            std::fs::metadata(&ledger).unwrap().modified().unwrap(),
            held,
            "a look with nothing to do rewrote the ledger"
        );
        std::fs::write(
            &kiro,
            r#"{"mcpServers":{"a":{"command":"a-mcp"}},"tips":3}"#,
        )
        .unwrap();
        assert_eq!(
            inputs_stamp(&ctx),
            stamp,
            "a key that is not a server changed the stamp"
        );
        std::fs::write(
            &kiro,
            r#"{"mcpServers":{"a":{"command":"a-mcp","args":["--v2"]}}}"#,
        )
        .unwrap();
        assert_ne!(
            inputs_stamp(&ctx),
            stamp,
            "an edited server left the stamp as it was"
        );
    }

    /// The Trash a test's keeper sends to: a folder of the test's own, never
    /// the person's (4275, 2026-11-06 — a gate or CI run would pile up there,
    /// and a regression would move a real folder into it). Every test names
    /// its folder through [`context`]; one that reaches the Trash without
    /// naming it fails instead of falling back to the real one.
    pub(super) mod fake_trash {
        use std::cell::RefCell;
        use std::path::{Path, PathBuf};

        thread_local! {
            static BIN: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
        }

        pub(crate) fn use_bin(dir: &Path) {
            BIN.with(|bin| *bin.borrow_mut() = Some(dir.to_path_buf()));
        }

        pub(crate) struct TrashReceipt {
            original: PathBuf,
            held: PathBuf,
        }

        impl TrashReceipt {
            pub(crate) fn restore(&self) -> Result<(), String> {
                if std::fs::symlink_metadata(&self.original).is_ok() {
                    return Err("tree.changed".to_string());
                }
                std::fs::rename(&self.held, &self.original).map_err(|error| error.to_string())
            }
        }

        /// Moves `path` — a link as a link — into the test's bin.
        pub(crate) fn trash_path(path: &Path) -> Result<TrashReceipt, String> {
            let bin = BIN
                .with(|bin| bin.borrow().clone())
                .expect("a library test reached the Trash without a bin of its own");
            std::fs::create_dir_all(&bin).map_err(|error| error.to_string())?;
            let name = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            let held = bin.join(format!("{}-{name}", super::next_id()));
            std::fs::rename(path, &held).map_err(|error| error.to_string())?;
            Ok(TrashReceipt {
                original: path.to_path_buf(),
                held,
            })
        }
    }
}

// In this module's tests the keeper's Trash is the test's own folder. It
// stands after the test module: the source contracts read the shipped half
// up to the first test marker, which must open `mod tests`. A module, not a
// `use`: clippy's `items_after_test_module` lets a module follow the tests,
// and one defined here comes before the `file_tree_io` of `use super::*`.
#[cfg(test)]
mod file_tree_io {
    pub(crate) use super::tests::fake_trash::*;
}
