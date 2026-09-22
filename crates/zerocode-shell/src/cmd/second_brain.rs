//! Second-brain vault integration commands.

use crate::*;

const SECOND_BRAIN_SKILL: &str = "second-brain";
const QUICK_COMMAND_PREFIX: &str = "second-brain";

#[derive(Clone, Serialize)]
pub(crate) struct SecondBrainReport {
    saved_path: String,
    vaults: Vec<zerocode_core::second_brain::DetectedVault>,
    vault: Option<zerocode_core::second_brain::VaultStatus>,
    obsidian_installed: bool,
    created: Vec<String>,
    quick_commands_added: usize,
    skill_installs: Vec<zerocode_core::skill_install::SkillInstallOutcome>,
    /// What each detected agent's GLOBAL instructions say about the vault —
    /// the card's 「전역 연결」 line. Read-only on the status road, written on
    /// setup and on 「다시 연결」.
    guides: Vec<zerocode_core::second_brain::GuideOutcome>,
    /// The opt-in: the skill's weekly review as a zo cron turn in the vault.
    weekly_review_enabled: bool,
    /// What the vault's zo cron registry says about that record — only read
    /// while the switch is on, because reading it is a `zo` process.
    weekly_review: Option<ReviewCronReceipt>,
}

/* ---- the weekly review as a cron turn -------------------------------------
 *
 * The prose half of the lint (`second_brain_lint` is the deterministic half):
 * the skill's 「주간 리뷰」, opened as a turn by the idle-time driver zo landed
 * in t-2901. The record is a zo cron in the VAULT's registry
 * (`<vault>/.zo/registries/crons.json`), so whichever zo pane is idle in the
 * vault project fires it. The window never writes that file: `zo cron` does,
 * with the registry's own merge, lock and tombstone rules, and answers a JSON
 * receipt that names the scheduler. What is spelled here is nothing — the
 * marker, the schedule and the prompt are `zerocode_core::second_brain`'s. */

/// The weekly review's standing in the vault's zo cron registry, as the card
/// shows it. `scheduler` is zo's own `automatic_scheduler_status` — the
/// receipt names who fires the record rather than promising a turn.
#[derive(Clone, Serialize)]
pub(crate) struct ReviewCronReceipt {
    registered: bool,
    cron_id: Option<String>,
    schedule: Option<String>,
    next_due_at: Option<u64>,
    scheduler: Option<String>,
    registry: Option<String>,
    outcome: Option<String>,
    /// Why the record could not be read or written, when it could not: no
    /// `zo`, a `zo` too old to know `cron`, a refused schedule.
    error: Option<String>,
}

impl ReviewCronReceipt {
    fn from_receipt(value: &serde_json::Value) -> Self {
        let text = |key: &str| {
            value
                .get(key)
                .and_then(|held| held.as_str())
                .map(str::to_string)
        };
        Self {
            registered: value
                .get("registered")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
            cron_id: text("cron_id"),
            schedule: text("schedule"),
            next_due_at: value.get("next_due_at").and_then(serde_json::Value::as_u64),
            scheduler: text("automatic_scheduler_status"),
            registry: text("registry"),
            outcome: text("outcome"),
            error: None,
        }
    }

    fn failed(error: String) -> Self {
        Self {
            registered: false,
            cron_id: None,
            schedule: None,
            next_due_at: None,
            scheduler: None,
            registry: None,
            outcome: None,
            error: Some(error),
        }
    }
}

/// Run `zo cron <args>` in `cwd` and read its JSON receipt. `Command::output`
/// waits the child — no bare child is left for the reaper to find — and the
/// prompt travels as one argument: argv never meets a shell, so a prompt of
/// paragraphs is one string end to end.
fn zo_cron(cwd: &Path, args: &[&str]) -> Result<serde_json::Value, String> {
    let zo = zerocode_pty::ZoBinary::discover()
        .ok_or("zo를 찾을 수 없습니다 — 주간 리뷰 cron은 zo가 볼트의 등록부에 씁니다")?;
    let output = crate::proc::quiet_command(&zo.path)
        .arg("cron")
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("zo cron을 실행하지 못했습니다: {error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let said = stderr.trim();
        return Err(if said.is_empty() {
            format!("zo cron이 {}(으)로 끝났습니다", output.status)
        } else {
            said.to_string()
        });
    }
    serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("zo cron의 영수증을 읽지 못했습니다: {error}"))
}

/// The record's standing for the card: nothing while the switch is off (no
/// process is spawned for a feature nobody turned on), the registry's answer
/// while it is on, and the reason as a receipt when the answer cannot be had.
fn review_cron_status(saved: &str, enabled: bool) -> Option<ReviewCronReceipt> {
    if !enabled {
        return None;
    }
    let vault = saved_vault(saved)?;
    let marker = zerocode_core::second_brain::WEEKLY_REVIEW_CRON_MARKER;
    Some(match zo_cron(&vault, &["show", "--description", marker]) {
        Ok(receipt) => ReviewCronReceipt::from_receipt(&receipt),
        Err(error) => ReviewCronReceipt::failed(error),
    })
}

/// Turn the weekly review on or off: the vault's zo cron registry first
/// (through `zo cron ensure` / `zo cron remove`), the setting only once that
/// succeeded — a switch that reads "on" while no record stands would be the
/// card promising a turn zo never opens. `ensure` is idempotent on zo's side,
/// so turning it on twice leaves one record.
#[tauri::command]
pub(crate) async fn set_second_brain_weekly_review(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    enabled: bool,
) -> Result<SettingsSnapshot, String> {
    let saved = load_settings_resilient(state.settings())
        .document
        .second_brain_vault;
    let Some(vault) = saved_vault(&saved) else {
        return Err("볼트를 먼저 세팅해 주세요".to_string());
    };
    tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        let marker = zerocode_core::second_brain::WEEKLY_REVIEW_CRON_MARKER;
        if enabled {
            let prompt = zerocode_core::second_brain::weekly_review_prompt(&vault);
            zo_cron(
                &vault,
                &[
                    "ensure",
                    "--description",
                    marker,
                    "--schedule",
                    zerocode_core::second_brain::WEEKLY_REVIEW_SCHEDULE,
                    "--prompt",
                    &prompt,
                ],
            )?;
        } else {
            zo_cron(&vault, &["remove", "--description", marker])?;
        }
        Ok(())
    })
    .await
    .map_err(|join| join.to_string())??;
    commit_setting(
        &app,
        &webview,
        &state,
        &[setting_key::SECOND_BRAIN_WEEKLY_REVIEW],
        move |settings| {
            settings.second_brain_weekly_review = enabled;
            Ok(())
        },
    )
}

struct SetupWork {
    outcome: zerocode_core::second_brain::SetupOutcome,
    quick_commands_added: usize,
    skill_installs: Vec<zerocode_core::skill_install::SkillInstallOutcome>,
    guides: Vec<zerocode_core::second_brain::GuideOutcome>,
}

use crate::skills_runtime::installed_agents;

fn obsidian_config(home: &Path) -> PathBuf {
    #[cfg(target_os = "macos")]
    return home.join("Library/Application Support/obsidian/obsidian.json");
    #[cfg(target_os = "windows")]
    return std::env::var_os("APPDATA").map_or_else(
        || home.join("AppData/Roaming/obsidian/obsidian.json"),
        |root| PathBuf::from(root).join("obsidian/obsidian.json"),
    );
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    home.join(".config/obsidian/obsidian.json")
}

fn obsidian_is_installed(home: &Path) -> bool {
    #[cfg(target_os = "macos")]
    {
        Path::new("/Applications/Obsidian.app").is_dir()
            || home.join("Applications/Obsidian.app").is_dir()
    }
    #[cfg(target_os = "windows")]
    {
        std::env::var_os("LOCALAPPDATA")
            .is_some_and(|root| PathBuf::from(root).join("Obsidian/Obsidian.exe").is_file())
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        ["/usr/bin/obsidian", "/usr/local/bin/obsidian"]
            .iter()
            .any(|path| Path::new(path).is_file())
    }
}

/// The card's whole answer. `guides` is the caller's because the two roads
/// that ask for it are asking different questions — status READS the agents'
/// global instructions, 「다시 연결」 and 세팅 WRITE them — and computing it
/// here as well would walk `PATH` for every agent a second time on the roads
/// that already know.
fn second_brain_report(
    home: &Path,
    saved_path: String,
    selected_path: Option<String>,
    guides: Vec<zerocode_core::second_brain::GuideOutcome>,
    weekly_review_enabled: bool,
) -> SecondBrainReport {
    let vaults = zerocode_core::second_brain::detected_vaults(&obsidian_config(home));
    let selected = selected_path
        .filter(|path| !path.trim().is_empty())
        .or_else(|| (!saved_path.is_empty()).then(|| saved_path.clone()))
        .or_else(|| vaults.first().map(|vault| vault.path.clone()));
    // The SAVED vault's registry, like the guides: the record lives where the
    // vault is, not where somebody is still browsing.
    let weekly_review = review_cron_status(&saved_path, weekly_review_enabled);
    SecondBrainReport {
        vault: selected.map(|path| zerocode_core::second_brain::inspect(Path::new(&path))),
        guides,
        saved_path,
        vaults,
        obsidian_installed: obsidian_is_installed(home),
        created: Vec::new(),
        quick_commands_added: 0,
        skill_installs: Vec::new(),
        weekly_review_enabled,
        weekly_review,
    }
}

/// The saved vault as a path, or nothing — a blank setting means this machine
/// has no second brain, and then the guide block is what gets taken back out.
fn saved_vault(saved_path: &str) -> Option<PathBuf> {
    let trimmed = saved_path.trim();
    (!trimmed.is_empty()).then(|| PathBuf::from(trimmed))
}

#[tauri::command]
pub(crate) async fn second_brain_status(
    state: State<'_, AppState>,
    path: Option<String>,
) -> Result<SecondBrainReport, String> {
    let home = dirs::home_dir().ok_or("홈 폴더를 찾을 수 없습니다")?;
    let document = load_settings_resilient(state.settings()).document;
    let saved = document.second_brain_vault;
    let weekly = document.second_brain_weekly_review;
    tauri::async_runtime::spawn_blocking(move || {
        // The link is about the SAVED vault, never the folder somebody is
        // still browsing: the block in `~/.claude/CLAUDE.md` names the vault
        // agents will actually be sent to.
        let guides = zerocode_core::second_brain::inspect_global_guides(
            &home,
            saved_vault(&saved).as_deref(),
            &installed_agents(),
        );
        second_brain_report(&home, saved, path, guides, weekly)
    })
    .await
    .map_err(|join| join.to_string())
}

#[tauri::command]
pub(crate) async fn second_brain_setup(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    path: String,
) -> Result<SecondBrainReport, String> {
    let raw = path.trim();
    if raw.is_empty() {
        return Err("볼트 폴더를 선택해 주세요".to_string());
    }
    let target = PathBuf::from(raw)
        .canonicalize()
        .map_err(|error| format!("{raw}: {error}"))?;
    if !target.is_dir() {
        return Err("볼트 경로가 폴더가 아닙니다".to_string());
    }
    let home = dirs::home_dir().ok_or("홈 폴더를 찾을 수 없습니다")?;
    let config_root = state.config_root().to_path_buf();
    let weekly = load_settings_resilient(state.settings())
        .document
        .second_brain_weekly_review;
    let saved_path = target.to_string_lossy().into_owned();
    let setup_path = target.clone();
    let setup_home = home.clone();
    let work = tauri::async_runtime::spawn_blocking(move || -> Result<SetupWork, String> {
        let outcome =
            zerocode_core::second_brain::setup(&setup_path).map_err(|error| error.to_string())?;
        let skill_targets = installed_agents();
        let skill_installs =
            skills_runtime::install_bundled_skill(SECOND_BRAIN_SKILL, &setup_home, &skill_targets)?;
        // And the same agents' GLOBAL instructions learn where the vault is,
        // so a pane opened in any other project already knows.
        let guides = zerocode_core::second_brain::link_global_guides(
            &setup_home,
            Some(&setup_path),
            &skill_targets,
        );
        let quick_agent = skill_targets.iter().find_map(|(agent, _)| {
            agent_spec(agent)
                .filter(|spec| spec.capabilities().takes_prompt_at_start())
                .map(|_| agent.clone())
        });
        let quick_commands_added =
            ensure_second_brain_quick_commands(&config_root, &setup_path, quick_agent.as_deref())?;
        Ok(SetupWork {
            outcome,
            quick_commands_added,
            skill_installs,
            guides,
        })
    })
    .await
    .map_err(|join| join.to_string())??;

    commit_setting(
        &app,
        &webview,
        &state,
        &[setting_key::SECOND_BRAIN_VAULT],
        |settings| {
            settings.second_brain_vault = saved_path.clone();
            Ok(())
        },
    )?;
    let mut report = second_brain_report(&home, saved_path, None, work.guides, weekly);
    report.created = work.outcome.created;
    report.vault = Some(work.outcome.status);
    report.quick_commands_added = work.quick_commands_added;
    report.skill_installs = work.skill_installs;
    Ok(report)
}

/// Write the vault into every detected agent's global instructions again —
/// the card's 「다시 연결」.
///
/// The button exists because the answer goes stale without anything having
/// happened here: an agent installed after the vault was set up has a home
/// root with no block in it, and a person who edited their own `CLAUDE.md`
/// may have taken ours out with it. Nothing is asked of the person — this
/// re-runs what setup did, for the vault already saved.
#[tauri::command]
pub(crate) async fn second_brain_link(
    state: State<'_, AppState>,
) -> Result<SecondBrainReport, String> {
    let home = dirs::home_dir().ok_or("홈 폴더를 찾을 수 없습니다")?;
    let document = load_settings_resilient(state.settings()).document;
    let saved = document.second_brain_vault;
    let weekly = document.second_brain_weekly_review;
    tauri::async_runtime::spawn_blocking(move || {
        let guides = link_saved_vault(&home, &saved);
        second_brain_report(&home, saved, None, guides, weekly)
    })
    .await
    .map_err(|join| join.to_string())
}

/// The SAVED vault into every installed agent's global instructions — what
/// setup writes, and what the 「다시 연결」 button and the boot run again.
fn link_saved_vault(home: &Path, saved: &str) -> Vec<zerocode_core::second_brain::GuideOutcome> {
    zerocode_core::second_brain::link_global_guides(
        home,
        saved_vault(saved).as_deref(),
        &installed_agents(),
    )
}

/// At boot, for a vault an earlier run saved: the same link, on a thread of
/// its own so the window does not wait on two files and a PATH probe.
///
/// The block used to be written by setup alone, so a vault set up before this
/// road existed — or an agent installed since — had a home root with no block
/// in it until somebody found the button ("껏다 켯어" and the global files were
/// still empty, 2026-09-03). The write is idempotent: a block that already
/// says the right thing is left as it is, and a person's own lines around it
/// are never touched. Without a saved vault nothing is written — the block is
/// a fact about that vault alone. What went wrong, if anything, the card says
/// the next time it is opened.
pub(crate) fn relink_saved_vault_at_boot(saved: String) {
    if saved_vault(&saved).is_none() {
        return;
    }
    let Some(home) = dirs::home_dir() else {
        return;
    };
    let _ = std::thread::Builder::new()
        .name("second-brain-link".into())
        .spawn(move || {
            let _ = link_saved_vault(&home, &saved);
        });
}

#[tauri::command(async)]
pub(crate) fn second_brain_open(path: String) -> Result<(), String> {
    let target = PathBuf::from(path.trim())
        .canonicalize()
        .map_err(|error| error.to_string())?;
    if !target.is_dir() {
        return Err("볼트 경로가 폴더가 아닙니다".to_string());
    }
    let uri = zerocode_core::second_brain::obsidian_open_uri(&target)
        .ok_or("Obsidian에서 열 볼트 이름을 찾지 못했습니다")?;
    #[cfg(target_os = "macos")]
    let launcher = "open";
    #[cfg(target_os = "linux")]
    let launcher = "xdg-open";
    #[cfg(target_os = "windows")]
    let launcher = "explorer";
    let mut opener = crate::proc::quiet_command(launcher);
    opener.arg(uri);
    zerocode_core::reap::spawn_forgotten(opener)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

/* ---- the knowledge graph -------------------------------------------------
 *
 * `wiki/` as a picture: pages are points, `[[wikilinks]]` are lines. The scan
 * is `zerocode_core::second_brain_graph`, and what lives here is the one thing
 * a pure function cannot hold — the per-vault parse cache that makes reopening
 * the view re-read only the pages that changed since it was last looked at.
 */

/// Vaults whose parse cache is kept. A person works in one vault; four is
/// generous, and the fifth simply pays for one full scan.
const MAX_GRAPH_CACHES: usize = 4;

/// The file watcher lane this vault's `wiki/` rides in (t-2931). The same
/// poller and the same thread the artifact store borrows; the lane's targets
/// are the bounded list `second_brain_live::watch_targets` answers, re-aimed
/// after every scan. No second scanner: a change here is a window event, and
/// the graph view answers it by asking for the graph again.
pub(crate) const WATCH_LANE: &str = "second-brain";
/// The window event that says the vault moved under the graph.
pub(crate) const CHANGED_EVENT: &str = "second-brain:changed";

/// What one vault's live layer remembers between two scans: the last graph
/// answered (the diff's "before") and the change rows the diffs produced,
/// newest first and bounded by the table.
#[derive(Default)]
struct LiveMemory {
    last: Option<zerocode_core::second_brain_graph::VaultGraph>,
    changes: Vec<zerocode_core::second_brain_live::BusRow>,
}

fn live_memories() -> &'static Mutex<HashMap<PathBuf, LiveMemory>> {
    static MEMORIES: OnceLock<Mutex<HashMap<PathBuf, LiveMemory>>> = OnceLock::new();
    MEMORIES.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The live layer for `graph` at `root`, and this scan remembered as the next
/// diff's "before". The first scan of a vault after boot has no before and
/// says nothing — nothing changed while the window was watching.
pub(crate) fn live_layer_for(
    root: &Path,
    graph: &zerocode_core::second_brain_graph::VaultGraph,
    now_ms: i64,
    utc_offset_minutes: i32,
) -> zerocode_core::second_brain_live::LiveLayer {
    use zerocode_core::second_brain_live::{Limits, bus_log, diff_rows, live_layer};
    let limits = Limits::default();
    let changes = {
        let mut held = live_memories()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if held.len() >= MAX_GRAPH_CACHES && !held.contains_key(root) {
            held.clear();
        }
        let memory = held.entry(root.to_path_buf()).or_default();
        if let Some(before) = memory.last.as_ref() {
            let fresh = diff_rows(before, graph, &limits);
            if !fresh.is_empty() {
                let mut rows = fresh;
                rows.append(&mut memory.changes);
                memory.changes = bus_log(rows, &limits);
            }
        }
        memory.last = Some(graph.clone());
        memory.changes.clone()
    };
    live_layer(root, graph, changes, now_ms, utc_offset_minutes, &limits)
}

/// Re-aim the watcher lane at what this scan saw: the lane's whole target
/// list, replaced, so a page that stopped being recent stops being watched.
pub(crate) fn aim_watch_lane(
    watched: &crate::file_watch::WatchSet,
    root: &Path,
    graph: &zerocode_core::second_brain_graph::VaultGraph,
) {
    let limits = zerocode_core::second_brain_live::Limits::default();
    let targets = zerocode_core::second_brain_live::watch_targets(root, graph, &limits)
        .into_iter()
        .map(|(name, path)| (format!("{WATCH_LANE}:{name}"), Some(path)))
        .collect();
    watched.replace_lane(WATCH_LANE, targets, true);
}

/// The trace's retention, on the artifact store's own sweep — same beat, same
/// days (`orchestration::beat`). Answers how many lines went, or nothing when
/// there is no saved vault.
pub(crate) fn sweep_recall_trace(now_ms: i64, days: u32) -> Option<usize> {
    let vault = crate::hooks::second_brain_vault()?;
    let root = graph_root(vault, None).ok()?;
    zerocode_core::second_brain_live::prune_recalls(&root, now_ms, days).ok()
}

fn graph_caches() -> &'static Mutex<HashMap<PathBuf, zerocode_core::second_brain_graph::GraphCache>>
{
    static CACHES: OnceLock<
        Mutex<HashMap<PathBuf, zerocode_core::second_brain_graph::GraphCache>>,
    > = OnceLock::new();
    CACHES.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The vault's graph, through its per-vault cache: one scan for a vault seen
/// for the first time, a stat pass for one seen before. The one road to a
/// graph — the view and the prompt hook read the same picture.
pub(crate) fn scanned_graph(
    root: &Path,
    sources: bool,
) -> zerocode_core::second_brain_graph::VaultGraph {
    let mut held = graph_caches()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    // Starting over costs one scan; growing without bound costs the window's
    // memory for vaults nobody is looking at any more.
    if held.len() >= MAX_GRAPH_CACHES && !held.contains_key(root) {
        held.clear();
    }
    held.entry(root.to_path_buf())
        .or_default()
        .scan(root, sources)
}

/// Sessions whose shown pages are remembered. A person has a few panes open;
/// past this the oldest session forgets, and its next prompt may see a page
/// again — a repeat, never a loss.
const MAX_KNOWLEDGE_SESSIONS: usize = 256;
/// Pages one session is not shown twice. Bounded so a long session does not
/// grow a set forever; the oldest is forgotten first, and may come back.
const MAX_SHOWN_PER_SESSION: usize = 64;

/// The second brain's answer to a prompt hook — the pages the prompt is about,
/// as one bounded block ahead of the turn (`zerocode_core::second_brain_related`).
///
/// This is what gives a Claude pane and a Codex pane the effect zo has from
/// its own prompt section and recall: every agent reads the same vault and is
/// shown the same pages for the same words, in every project, with nothing to
/// set up beyond the vault itself. The bridge asks through
/// [`zerocode_hookd::PromptKnowledge`]; this side owns the vault path, the
/// graph cache and the memory of what each session was already shown.
#[derive(Default)]
pub(crate) struct VaultKnowledge {
    /// Per session, the slugs already put in front of the agent, oldest first.
    /// A page named twice is noise, not emphasis.
    shown: Mutex<HashMap<String, std::collections::VecDeque<String>>>,
}

impl VaultKnowledge {
    /// The block for `prompt` from the vault at `root`, minus the pages this
    /// session has already seen; the pages named are remembered as seen, and
    /// written to the vault's recall trace so the graph can show them
    /// (t-2931). The trace is best effort: a vault on a read-only disk still
    /// gets its block.
    pub(crate) fn block_for(
        &self,
        root: &Path,
        session_key: &str,
        pane_key: &str,
        prompt: &str,
    ) -> Option<String> {
        let graph = scanned_graph(root, false);
        let skip: std::collections::HashSet<String> = self
            .shown
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(session_key)
            .map(|seen| seen.iter().cloned().collect())
            .unwrap_or_default();
        let block = zerocode_core::second_brain_related::related_block(&graph, prompt, &skip)?;
        self.remember(session_key, &block.slugs);
        let _ = zerocode_core::second_brain_live::append_recall(
            root,
            &zerocode_core::second_brain_live::RecallLine {
                at: crate::project_runtime::now_epoch_ms(),
                pages: block.slugs.clone(),
                session: session_key.to_string(),
                pane: pane_key.to_string(),
            },
            &zerocode_core::second_brain_live::Limits::default(),
        );
        Some(block.text)
    }

    fn remember(&self, session_key: &str, slugs: &[String]) {
        let mut shown = self
            .shown
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !shown.contains_key(session_key) && shown.len() >= MAX_KNOWLEDGE_SESSIONS {
            // HashMap has no age; dropping every session is a repeat at worst,
            // and it happens once per few hundred sessions.
            shown.clear();
        }
        let seen = shown.entry(session_key.to_string()).or_default();
        for slug in slugs {
            if seen.iter().any(|held| held == slug) {
                continue;
            }
            if seen.len() >= MAX_SHOWN_PER_SESSION {
                seen.pop_front();
            }
            seen.push_back(slug.clone());
        }
    }
}

impl zerocode_hookd::PromptKnowledge for VaultKnowledge {
    fn related_block(&self, session_key: &str, pane_key: &str, prompt: &str) -> Option<String> {
        // The SAVED vault, published where every pane reads it; no vault, no
        // block. A path that stopped being a folder is refused the same way
        // the graph view refuses it.
        let vault = crate::hooks::second_brain_vault()?;
        let root = graph_root(vault, None).ok()?;
        self.block_for(&root, session_key, pane_key, prompt)
    }
}

/// How long one project's code layer stands before `zo vault code` is asked
/// again (t-5970): the view refreshes on every watcher tick, and a tick must
/// not spawn zo and walk the project each time.
const CODE_LAYER_TTL: std::time::Duration = std::time::Duration::from_secs(30);
/// Layers held — one per (vault, project) the window has looked at.
const MAX_CODE_LAYERS: usize = 8;

type HeldCodeLayer = (
    Instant,
    Result<zerocode_core::second_brain_code::CodeLayer, String>,
);

fn code_layers() -> &'static Mutex<HashMap<(PathBuf, PathBuf), HeldCodeLayer>> {
    static LAYERS: OnceLock<Mutex<HashMap<(PathBuf, PathBuf), HeldCodeLayer>>> = OnceLock::new();
    LAYERS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The project's code layer for this vault, held while fresh and otherwise
/// asked of `zo vault code` — the codegraph index is zo's, and nothing in the
/// window reads it. A refusal is held for as long as an answer, so a missing
/// zo is not asked again on every tick.
fn code_layer_for(
    vault: &Path,
    project: &Path,
) -> Result<zerocode_core::second_brain_code::CodeLayer, String> {
    let key = (vault.to_path_buf(), project.to_path_buf());
    if let Some((at, held)) = code_layers()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&key)
        && at.elapsed() < CODE_LAYER_TTL
    {
        return held.clone();
    }
    let answer = zo_vault_code(vault, project);
    let mut held = code_layers()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if held.len() >= MAX_CODE_LAYERS && !held.contains_key(&key) {
        held.clear();
    }
    held.insert(key, (Instant::now(), answer.clone()));
    answer
}

fn zo_vault_code(
    vault: &Path,
    project: &Path,
) -> Result<zerocode_core::second_brain_code::CodeLayer, String> {
    let zo = zerocode_pty::ZoBinary::discover()
        .ok_or("zo를 찾을 수 없습니다 — 코드 층은 zo가 프로젝트의 인덱스에서 읽습니다")?;
    let output = crate::proc::quiet_command(&zo.path)
        .args(["vault", "code", "--json", "--vault"])
        .arg(vault)
        .arg("--project")
        .arg(project)
        .current_dir(project)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("zo vault code를 실행하지 못했습니다: {error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let said = stderr.trim();
        return Err(if said.is_empty() {
            format!("zo vault code가 {}(으)로 끝났습니다", output.status)
        } else {
            said.to_string()
        });
    }
    serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("zo vault code의 답을 읽지 못했습니다: {error}"))
}

/// What the code lens put on the picture (t-5970), or why it put nothing.
#[derive(Clone, Serialize)]
pub(crate) struct CodeLens {
    /// The project whose index answered, as the view asked for it.
    project: String,
    /// Distinct mentions the vault's pages made, and how many the index
    /// could place.
    mentions: usize,
    resolved: usize,
    grafted: zerocode_core::second_brain_code::GraftSummary,
    /// The refusal, verbatim, when there is no layer.
    error: Option<String>,
}

/// Graft `project`'s code layer onto `graph` and say what happened.
fn graft_code_lens(
    vault: &Path,
    project: &str,
    graph: &mut zerocode_core::second_brain_graph::VaultGraph,
) -> CodeLens {
    let mut lens = CodeLens {
        project: project.to_string(),
        mentions: 0,
        resolved: 0,
        grafted: zerocode_core::second_brain_code::GraftSummary::default(),
        error: None,
    };
    let layer = PathBuf::from(project)
        .canonicalize()
        .map_err(|error| format!("{project}: {error}"))
        .and_then(|root| code_layer_for(vault, &root));
    match layer {
        Ok(layer) => {
            lens.mentions = layer.mentions;
            lens.resolved = layer.resolved;
            lens.grafted = zerocode_core::second_brain_code::graft(
                graph,
                &layer,
                &zerocode_core::second_brain_code::CodeLimits::default(),
            );
        }
        Err(error) => lens.error = Some(error),
    }
    lens
}

#[derive(Clone, Serialize)]
pub(crate) struct SecondBrainGraphReport {
    /// The vault the picture was read from, canonicalized.
    vault: String,
    graph: zerocode_core::second_brain_graph::VaultGraph,
    /// How long the scan took. The view shows it beside the refresh button —
    /// an incremental scan that stops being incremental has to be visible.
    scanned_ms: u64,
    /// The scan re-read nothing and the vault holds no page: the empty state
    /// this view opens on before anything has been ingested.
    empty: bool,
    /// What is live about the picture (t-2931): recalls, the bus log, merge
    /// candidates, and the table every number came from.
    live: zerocode_core::second_brain_live::LiveLayer,
    /// The code lens's answer (t-5970); absent while the lens is off.
    #[serde(skip_serializing_if = "Option::is_none")]
    code: Option<CodeLens>,
}

/// The vault path this window will read a graph from.
///
/// Refuses anything that is not a directory rather than answering with an
/// empty graph: "no pages" and "no such folder" are different sentences and
/// the view says different things about them.
pub(crate) fn graph_root(saved: String, asked: Option<String>) -> Result<PathBuf, String> {
    let raw = asked
        .filter(|path| !path.trim().is_empty())
        .unwrap_or(saved);
    let raw = raw.trim();
    if raw.is_empty() {
        return Err("볼트 폴더를 먼저 선택해 주세요".to_string());
    }
    let root = PathBuf::from(raw)
        .canonicalize()
        .map_err(|error| format!("{raw}: {error}"))?;
    if !root.is_dir() {
        return Err("볼트 경로가 폴더가 아닙니다".to_string());
    }
    Ok(root)
}

/// `utc_offset_minutes` is the window's `Date.getTimezoneOffset()`: the
/// journal's stamps are wall-clock time and the live layer dates them in the
/// window's own zone. `code` with a `project` grafts that project's code
/// layer (t-5970) after the live layer and the watch lane read the vault's
/// own picture — code is not a page to watch or to call alive.
#[tauri::command]
pub(crate) async fn second_brain_graph(
    state: State<'_, AppState>,
    path: Option<String>,
    sources: Option<bool>,
    code: Option<bool>,
    project: Option<String>,
    utc_offset_minutes: Option<i32>,
) -> Result<SecondBrainGraphReport, String> {
    let saved = load_settings_resilient(state.settings())
        .document
        .second_brain_vault;
    let sources = sources.unwrap_or(false);
    let watched = Arc::clone(state.watched());
    tauri::async_runtime::spawn_blocking(move || {
        let root = graph_root(saved, path)?;
        let began = Instant::now();
        let mut graph = scanned_graph(&root, sources);
        let live = live_layer_for(
            &root,
            &graph,
            crate::project_runtime::now_epoch_ms(),
            utc_offset_minutes.unwrap_or(0),
        );
        aim_watch_lane(&watched, &root, &graph);
        let code = code
            .unwrap_or(false)
            .then_some(project)
            .flatten()
            .filter(|project| !project.trim().is_empty())
            .map(|project| graft_code_lens(&root, &project, &mut graph));
        Ok(SecondBrainGraphReport {
            vault: root.to_string_lossy().into_owned(),
            empty: graph.pages == 0,
            graph,
            scanned_ms: u64::try_from(began.elapsed().as_millis()).unwrap_or(u64::MAX),
            live,
            code,
        })
    })
    .await
    .map_err(|join| join.to_string())?
}

/// Paths between two pages (t-5966 G3) — the one calculator,
/// `zerocode_core::second_brain_paths::report`, over the same cached picture
/// the view draws (`sources` as the view asked it). The window maps the
/// answer's ids onto whatever its lens is showing and counts nothing itself;
/// `zo vault path` prints the same answer on a pane.
#[tauri::command]
pub(crate) async fn second_brain_paths(
    state: State<'_, AppState>,
    path: Option<String>,
    sources: Option<bool>,
    from: String,
    to: String,
    k: Option<usize>,
) -> Result<zerocode_core::second_brain_paths::PathReport, String> {
    let saved = load_settings_resilient(state.settings())
        .document
        .second_brain_vault;
    tauri::async_runtime::spawn_blocking(move || {
        let root = graph_root(saved, path)?;
        let graph = scanned_graph(&root, sources.unwrap_or(false));
        zerocode_core::second_brain_paths::report(
            &graph,
            &from,
            &to,
            k.unwrap_or(zerocode_core::second_brain_paths::PATH_LIMITS.k_max),
        )
        .map_err(|refusal| refusal.to_string())
    })
    .await
    .map_err(|join| join.to_string())?
}

/// The receipt an export answers with: the gallery row it became.
#[derive(Clone, Serialize)]
pub(crate) struct SecondBrainExportReceipt {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) version: u32,
    pub(crate) bytes: u64,
    pub(crate) path: String,
    pub(crate) nodes: usize,
    pub(crate) edges: usize,
}

/// The folder under the artifact store where a vault's picture is written
/// before it is published — one file per vault, so every export of the same
/// vault is a new version of one gallery row rather than a new row.
const KNOWLEDGE_EXPORT_DIR: &str = "knowledge";
/// The gallery label every knowledge export wears.
const KNOWLEDGE_EXPORT_LABEL: &str = "knowledge-graph";

/// The picture the window is showing, as one self-contained HTML page in the
/// artifact store (t-5966 G4). Rendering is core's
/// (`zerocode_core::second_brain_export::render`, the size bound included);
/// this side writes the page beside the store and publishes it through the
/// same door an agent's page goes through (`Store::publish_page`), so the
/// gallery, its versions and its thumbnails need nothing new.
#[tauri::command]
pub(crate) async fn second_brain_export_html(
    input: zerocode_core::second_brain_export::ExportInput,
) -> Result<SecondBrainExportReceipt, String> {
    let store = artifact_runtime::store().ok_or("아티팩트 스토어가 아직 열리지 않았습니다")?;
    tauri::async_runtime::spawn_blocking(move || {
        let page = zerocode_core::second_brain_export::render(&input)
            .map_err(|refusal| refusal.to_string())?;
        let seat = store.root().join(KNOWLEDGE_EXPORT_DIR);
        std::fs::create_dir_all(&seat).map_err(|error| error.to_string())?;
        let file_path = seat.join(format!(
            "{}.html",
            &artifact_runtime::sha256_hex(input.vault.as_bytes())[..16]
        ));
        std::fs::write(&file_path, page.html.as_bytes()).map_err(|error| error.to_string())?;
        let meta = store.publish_page(&zerocode_core::artifact_publish::PublishInput {
            file_path,
            title: Some(input.title.clone()),
            description: Some(format!(
                "{} · {} nodes · {} edges{}",
                input.vault,
                page.nodes,
                page.edges,
                if input.lenses.is_empty() {
                    String::new()
                } else {
                    format!(" · {}", input.lenses.join(" · "))
                }
            )),
            favicon: None,
            label: Some(KNOWLEDGE_EXPORT_LABEL.to_string()),
        })?;
        if let Some(app) = artifact_runtime::window_handle() {
            use tauri::Emitter as _;
            let _ = app.emit(artifact_runtime::CHANGED_EVENT, ());
        }
        Ok(SecondBrainExportReceipt {
            id: meta.id,
            title: meta.title,
            version: meta.version,
            bytes: meta.bytes,
            path: meta.path.to_string_lossy().into_owned(),
            nodes: page.nodes,
            edges: page.edges,
        })
    })
    .await
    .map_err(|join| join.to_string())?
}

/// Where one graph node's page lives, so the window can open it as a document.
///
/// The id came from the scan and the join is checked there
/// (`second_brain_graph::page_path`), so a ghost node — or a crafted id —
/// answers with an error rather than a path outside the vault.
#[tauri::command]
pub(crate) async fn second_brain_page(
    state: State<'_, AppState>,
    path: Option<String>,
    id: String,
) -> Result<String, String> {
    let saved = load_settings_resilient(state.settings())
        .document
        .second_brain_vault;
    tauri::async_runtime::spawn_blocking(move || {
        let root = graph_root(saved, path)?;
        let page = zerocode_core::second_brain_graph::page_path(&root, &id)
            .ok_or("볼트 안의 페이지가 아닙니다")?;
        if !page.is_file() {
            return Err("아직 없는 페이지입니다".to_string());
        }
        Ok(page.to_string_lossy().into_owned())
    })
    .await
    .map_err(|join| join.to_string())?
}

/// Write or take away one declared relation in a wiki page's frontmatter
/// (t-4140 S5). The road is `zerocode_core::second_brain_relate::relate`:
/// `wiki/` only, the file replaced whole, `remove` the same road backwards —
/// so the window's undo is this call again. The graph cache sees the page's
/// new mtime on the next scan and the watcher lane says the vault moved.
#[tauri::command]
pub(crate) async fn second_brain_relate(
    state: State<'_, AppState>,
    path: Option<String>,
    from: String,
    to: String,
    kind: String,
    remove: Option<bool>,
) -> Result<zerocode_core::second_brain_relate::RelationWrite, String> {
    let saved = load_settings_resilient(state.settings())
        .document
        .second_brain_vault;
    tauri::async_runtime::spawn_blocking(move || {
        let root = graph_root(saved, path)?;
        let kind = zerocode_core::second_brain_graph::EdgeKind::from_key(&kind)
            .ok_or("frontmatter 관계 키가 아닙니다")?;
        zerocode_core::second_brain_relate::relate(&root, &from, &to, kind, remove.unwrap_or(false))
    })
    .await
    .map_err(|join| join.to_string())?
}

/// The pages one seat was shown (`second_brain_live::seat_recalls`) — the task
/// board's 「참고한 지식」, the one proven edge between a task and the graph.
/// `session` is the agent's own session id as the window holds it
/// (`paneSessions`); `pane` the board's pane spelled as the hook spells it
/// (`term-<n>`). The session wins when both are given: a restart renumbers
/// panes, not conversations.
#[tauri::command]
pub(crate) async fn second_brain_seat_recalls(
    state: State<'_, AppState>,
    path: Option<String>,
    session: Option<String>,
    pane: Option<String>,
) -> Result<Vec<zerocode_core::second_brain_live::SeatRecall>, String> {
    let saved = load_settings_resilient(state.settings())
        .document
        .second_brain_vault;
    tauri::async_runtime::spawn_blocking(move || {
        let root = graph_root(saved, path)?;
        Ok(zerocode_core::second_brain_live::seat_recalls(
            &root,
            session.as_deref(),
            pane.as_deref(),
            &zerocode_core::second_brain_live::Limits::default(),
        ))
    })
    .await
    .map_err(|join| join.to_string())?
}

fn second_brain_quick_commands(path: &Path, agent: Option<&str>) -> [QuickCommand; 3] {
    let scope = path.to_string_lossy().into_owned();
    let stamp = zerocode_core::skill::path_id(path);
    let command = |suffix: &str, label: &str, body: &str| QuickCommand {
        id: format!("{QUICK_COMMAND_PREFIX}-{stamp}-{suffix}"),
        label: label.to_string(),
        workspace: Some(scope.clone()),
        body: body.to_string(),
        agent: agent.map(str::to_string),
        append_enter: agent.is_some(),
    };
    [
        command(
            "ingest",
            "raw 취합",
            "raw/에 있지만 wiki/에 아직 반영되지 않은 항목을 모두 취합하고 wiki/log.md에 기록해 줘.",
        ),
        command(
            "ask",
            "위키에 질문",
            "내가 질문할 내용을 먼저 물어본 다음 wiki/를 우선 검색하고, 부족할 때만 raw/를 검색해서 출처 링크와 함께 답해 줘.",
        ),
        command(
            "weekly",
            "주간 리뷰",
            "이번 주 취합 내용을 요약하고, 고아 wiki 페이지와 아직 취합되지 않은 raw 항목, 다음에 읽을 것을 찾아 줘.",
        ),
    ]
}

pub(crate) fn ensure_second_brain_quick_commands(
    config_root: &Path,
    path: &Path,
    agent: Option<&str>,
) -> Result<usize, String> {
    let mut rows = stored_quick_commands(config_root);
    let mut added = 0;
    for command in second_brain_quick_commands(path, agent) {
        if rows.iter().any(|held| held.id == command.id) {
            continue;
        }
        validate_quick_command(&command)?;
        rows.push(command);
        added += 1;
    }
    if added > 0 {
        write_quick_commands(config_root, &rows)?;
    }
    Ok(added)
}

/// Persist saved knowledge scenes/phrases per vault path as one JSON line in settings.
#[tauri::command]
pub(crate) async fn set_second_brain_scenes(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    vault: String,
    scenes: String,
) -> Result<SettingsSnapshot, String> {
    commit_setting(
        &app,
        &webview,
        &state,
        &[setting_key::SECOND_BRAIN_SCENES],
        move |settings| {
            let trimmed = scenes.trim();
            if trimmed.is_empty() || trimmed == "[]" || trimmed == "{}" {
                settings.second_brain_scenes.remove(&vault);
            } else {
                settings
                    .second_brain_scenes
                    .insert(vault, trimmed.to_string());
            }
            Ok(())
        },
    )
}

#[tauri::command]
pub(crate) async fn get_second_brain_scenes(
    state: State<'_, AppState>,
    vault: String,
) -> Result<String, String> {
    let document = load_settings_resilient(state.settings()).document;
    Ok(document
        .second_brain_scenes
        .get(&vault)
        .cloned()
        .unwrap_or_default())
}

/// Persist the knowledge graph's last exploration per vault path — mode,
/// centre, depth as one JSON line beside the scenes (t-4140 S2). An empty
/// line forgets the vault; the document never carries an empty table.
#[tauri::command]
pub(crate) async fn set_second_brain_explore(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    vault: String,
    explore: String,
) -> Result<SettingsSnapshot, String> {
    commit_setting(
        &app,
        &webview,
        &state,
        &[setting_key::SECOND_BRAIN_EXPLORE],
        move |settings| {
            let trimmed = explore.trim();
            if trimmed.is_empty() || trimmed == "{}" {
                settings.second_brain_explore.remove(&vault);
            } else {
                settings
                    .second_brain_explore
                    .insert(vault, trimmed.to_string());
            }
            Ok(())
        },
    )
}
