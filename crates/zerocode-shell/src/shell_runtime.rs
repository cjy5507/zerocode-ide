use super::*;

/// A command line as the words it means, honouring quotes.
///
/// A splitter, not a shell: quotes group, and nothing else happens — no
/// expansion, no substitution, no operators, no `sh -c`. That is the whole
/// security argument for letting a settings field name a program, and it is
/// why what comes out of here is argv and goes straight to `PtyLane::spawn`.
///
/// Quotes rather than whitespace alone because a program can live at a path
/// with a space in it. Splitting `"/opt/my tools/zo"` on whitespace produced
/// two words, neither of which exists, and the window quietly opened a shell
/// instead of what was asked for.
pub(super) fn split_command(line: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut open: Option<char> = None;
    // Distinct from `word.is_empty()`: `""` is a word somebody wrote, and an
    // empty argument is a thing programs are given on purpose.
    let mut started = false;
    // Whether the previous character was a backslash inside double quotes.
    let mut escaped = false;
    for glyph in line.chars() {
        match open {
            Some(quote) if escaped => {
                // Inside double quotes a backslash escapes the quote itself,
                // which is the only way to write one into an argument. In
                // single quotes it is an ordinary character, the way a shell
                // treats it.
                if quote == '"' && glyph != '"' && glyph != '\\' {
                    word.push('\\');
                }
                word.push(glyph);
                escaped = false;
            }
            Some('"') if glyph == '\\' => escaped = true,
            Some(quote) if glyph == quote => open = None,
            Some(_) => word.push(glyph),
            None if glyph == '"' || glyph == '\'' => {
                open = Some(glyph);
                started = true;
            }
            None if glyph.is_whitespace() => {
                if started {
                    words.push(std::mem::take(&mut word));
                    started = false;
                }
            }
            None => {
                word.push(glyph);
                started = true;
            }
        }
    }
    if started {
        words.push(word);
    }
    words
}

/// A command line from the words it means — the reverse of [`split_command`].
///
/// The settings field shows what was stored, so somebody can edit it; that
/// means the words have to come back as a line, and the line has to split
/// into those same words again. Both halves live here so the round trip is
/// one testable property rather than two rules in two languages that agree
/// only by inspection.
pub(super) fn quote_command(words: &[String]) -> String {
    words
        .iter()
        .map(|word| {
            let plain = !word.is_empty()
                && !word.contains(|glyph: char| glyph.is_whitespace())
                && !word.contains('"')
                && !word.contains('\'');
            if plain {
                return word.clone();
            }
            // The backslash first, and that order is the whole correctness
            // argument: escape the quote first and the backslash you add is
            // itself escaped a moment later, so `say "hi"` comes back as
            // `say \"hi\"`. Doing it the other way round is what corrupted a
            // word ending in a backslash — `a back\` became `"a back\"`,
            // whose closing quote the splitter then read as escaped.
            let escaped = word.replace('\\', "\\\\").replace('"', "\\\"");
            format!("\"{escaped}\"")
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Whether a split command is a line of shell rather than an argv.
///
/// tmux's own contract runs `split-window`'s command through `sh -c`, and
/// Claude Code's Agent Teams writes to that contract: a teammate summons
/// arrives as one line of shell — `cd <repo> && env K=V … <claude>
/// --agent-id …`. Handing that line to [`split_command`] made an argv whose
/// program was `cd`, and the pane died at birth with exit 0 while its agent
/// went on living in-process, invisible (terms 5 and 7 in the window log).
///
/// Judged on the SPLIT words so quoting keeps its meaning: an `&&` inside
/// quotes is an argument, and only a bare operator token makes the line
/// shell. A leading `cd` counts too — a builtin no exec can honour. An
/// operator glued to its neighbour (`a&&b`) errs toward the argv road,
/// which is the road every command took before this function existed.
pub(super) fn command_is_shell(words: &[String]) -> bool {
    words.iter().any(|word| {
        matches!(
            word.as_str(),
            "&&" | "||" | ";" | "|" | "&" | ">" | ">>" | "<" | "<<" | "2>" | "2>>"
        )
    }) || words.first().is_some_and(|word| word == "cd")
}

/// The program a line of shell actually leaves running.
///
/// The pane spawns a shell, but the shell is nobody's agent: which agent
/// sits in the pane — and with it the account, the hook home, the dialect —
/// is decided by the program the LINE runs. Walked the way the shell will
/// walk it: an operator starts a new command, `cd` heads a command without
/// being a program, `env` and `exec` and `NAME=VALUE` assignments stand
/// aside, and the last head standing is the answer, because that is the one
/// left holding the pane.
pub(super) fn shell_spoken_program(words: &[String]) -> Option<String> {
    let mut spoken = None;
    let mut head = true;
    for word in words {
        match word.as_str() {
            "&&" | "||" | ";" | "|" => head = true,
            _ if !head => {}
            "cd" => head = false,
            "env" | "exec" => {}
            assignment
                if assignment.split_once('=').is_some_and(|(name, _)| {
                    !name.is_empty()
                        && !name.starts_with(|glyph: char| glyph.is_ascii_digit())
                        && name
                            .chars()
                            .all(|glyph| glyph.is_ascii_alphanumeric() || glyph == '_')
                }) => {}
            _ => {
                spoken = Some(word.clone());
                head = false;
            }
        }
    }
    spoken
}

/// A pane spawn that honours a line of shell — tmux's `sh -c`, kept.
///
/// Windows has no `/bin/sh`; its counterpart is `%COMSPEC% /C`, wired now so
/// the shell road exists on both platforms rather than only where it was
/// first needed (the operator set above holds for `cmd` too).
#[cfg(not(windows))]
pub(super) fn shell_spawn(line: &str) -> (String, Vec<String>) {
    (
        "/bin/sh".to_string(),
        vec!["-c".to_string(), line.to_string()],
    )
}

#[cfg(windows)]
pub(super) fn shell_spawn(line: &str) -> (String, Vec<String>) {
    let comspec = std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".to_string());
    (comspec, vec!["/C".to_string(), line.to_string()])
}

/// Whether a program name is one this machine can actually start.
///
/// Asked where the spawn will ask: a name carrying a separator is a path and
/// is checked as one, a bare name is looked for on `PATH`. Wrong only in the
/// race between here and the spawn, which is not the failure this catches —
/// the one it catches is a typo, answered while the person is still looking
/// at the field they typed it into.
pub(super) fn program_exists(program: &str) -> bool {
    let named = std::path::Path::new(program);
    if named.is_absolute() || named.components().count() > 1 {
        return is_runnable(named);
    }
    let Ok(path) = std::env::var("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| is_runnable(&dir.join(program)))
}

/// A file this machine would actually execute.
///
/// Present is not the same as runnable: a script with no executable bit saves
/// clean and then fails at spawn time, which lands right back in the silent
/// fallback this check exists to prevent. Windows decides by extension rather
/// than by a mode bit, so there `is_file` is the whole answer.
#[cfg(unix)]
pub(super) fn is_runnable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    path.metadata()
        .is_ok_and(|found| found.is_file() && found.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
pub(super) fn is_runnable(path: &Path) -> bool {
    path.is_file()
}

/// What a new terminal runs, as words — argv, never a shell string.
///
/// Empty means the user's own shell, which is the answer unless they said
/// otherwise.
pub(super) fn stored_terminal_command(legacy: &LegacySettings<'_>) -> Vec<String> {
    legacy
        .read(legacy_settings_file::TERMINAL_COMMAND)
        .unwrap_or_default()
}

pub(super) fn last_workspace_file(config_root: &Path) -> PathBuf {
    config_root.join(artifact_file::LAST_WORKSPACE)
}

/// Write down where the window is looking, for the next boot to reopen.
///
/// Best-effort like the recents file: a failed write costs the next boot its
/// memory, not this session anything. The whole-filesystem guard holds here
/// too — `/` must never become the remembered answer.
pub(super) fn remember_last_workspace(config_root: &Path, root: &Path) {
    if is_whole_filesystem(root) {
        return;
    }
    let file = last_workspace_file(config_root);
    if let Some(parent) = file.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(text) = serde_json::to_string(&root.to_string_lossy()) {
        let _ = std::fs::write(&file, text);
    }
}

/// The workspace the window last showed, if it still exists.
///
/// Validated on the way out because it is read at boot: a checkout deleted
/// since the last session must not put the whole window inside a missing
/// directory — the catalog fallback (or the launch cwd) answers instead.
pub(super) fn stored_last_workspace(config_root: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(last_workspace_file(config_root)).ok()?;
    let held = serde_json::from_str::<String>(&text).ok()?;
    let path = PathBuf::from(held);
    (path.is_dir() && !is_whole_filesystem(&path)).then_some(path)
}

/// The workspace this window should wake up in.
///
/// **A GUI launch has no working directory of its own** — macOS hands a
/// Finder- or Dock-started app `/` (a drive root on Windows), and this
/// window used to take that answer literally: file surfaces on the whole
/// disk, the restart-resumed leader spawned at `/` behind a trust prompt,
/// until the person clicked their project. The catalog guard
/// (`is_whole_filesystem`) already refuses to RECORD such a root; this is
/// the same fact asked at boot, answered with where the window actually was
/// — the workspace it last showed, or failing that the first project the
/// catalog still has on disk. A terminal launch keeps its cwd: that answer
/// is real.
pub(super) fn boot_workspace_root(root: PathBuf, config_root: &Path) -> PathBuf {
    if !is_whole_filesystem(&root) {
        return root;
    }
    stored_last_workspace(config_root)
        .or_else(|| {
            stored_projects(config_root)
                .into_iter()
                .map(PathBuf::from)
                .find(|held| held.is_dir())
        })
        .unwrap_or(root)
}

/// The projects this machine has opened, newest first.
///
/// Best-effort at every step — a missing file, an unreadable one, or a shape
/// this version does not recognise all answer with an empty list. A sidebar
/// section that is merely empty is not a failure anybody can act on, and
/// refusing to boot over a recents file would be absurd.
pub(super) fn stored_projects(config_root: &Path) -> Vec<String> {
    let file = recent_projects_file(config_root);
    let Ok(text) = std::fs::read_to_string(&file) else {
        return Vec::new();
    };
    let mut held = serde_json::from_str::<Vec<String>>(&text).unwrap_or_default();
    // A root recorded by an older build leaves on the next read, and
    // `note_project` writes the shorter list back — the row disappears without
    // anybody having to find the file, and a person who removed it once does
    // not have to remove it again.
    held.retain(|path| !is_whole_filesystem(Path::new(path)));
    held
}

/// The fingerprints that say a worktree of this repository is ours, owned as
/// `String`s so the borrowed [`zerocode_core::WorktreeOurs`] can point at them
/// for the length of one project's decision.
///
/// Empty when the orchestrator would not open — a folder project has no
/// worktree root of ours, and the rules answer "nothing here is external"
/// rather than guessing.
pub(super) struct WorktreeMarks {
    pub(super) root: String,
    /// The same root computed under every layout workspaces were made under
    /// before. Changing the setting moves where NEW workspaces go; it does not
    /// move the ones already on disk, and their path is the only fingerprint
    /// ownership has when the branch prefix is `none`.
    pub(super) past_roots: Vec<String>,
    pub(super) branch_prefix: String,
}

pub(super) fn our_worktree_marks(
    orchestrator: Option<&Orchestrator>,
    prefs: &WorkspaceCreationPrefs,
) -> WorktreeMarks {
    orchestrator.map_or_else(
        || WorktreeMarks {
            root: String::new(),
            past_roots: Vec::new(),
            branch_prefix: String::new(),
        },
        |open| WorktreeMarks {
            root: open.worktree_root().to_string_lossy().into_owned(),
            // A layout that no longer resolves is simply not a fingerprint —
            // a stored directory can name a drive that is gone, and that is
            // not a reason to fail the whole listing.
            past_roots: prefs
                .history
                .iter()
                .filter_map(|layout| {
                    configured_worktree_root(
                        open.repo_root(),
                        &WorkspaceCreationPrefs {
                            directory: layout.directory.clone(),
                            nest_workspaces: layout.nest_workspaces,
                            history: Vec::new(),
                        },
                    )
                    .ok()
                })
                .map(|path| path.to_string_lossy().into_owned())
                .collect(),
            branch_prefix: open.branch_prefix().to_string(),
        },
    )
}

pub(super) fn ours_of(held: &WorktreeMarks) -> zerocode_core::WorktreeOurs<'_> {
    zerocode_core::WorktreeOurs {
        worktree_root: &held.root,
        past_roots: &held.past_roots,
        branch_prefix: &held.branch_prefix,
    }
}

/// Decide, for one project, which of its worktrees this window did not make and
/// what the sidebar should do about them.
///
/// Every row is stamped with its ownership and with whether the external-
/// visibility rule hides it, and the project is handed the four answers its
/// surfaces need. The rules themselves are
/// [`zerocode_core::worktree_ownership`]; what happens here is only the walk.
pub(super) fn decide_external_visibility(
    worktrees: &mut [WorktreeEntry],
    repo_root: &str,
    ours: zerocode_core::WorktreeOurs<'_>,
    stored: &zerocode_core::ExternalVisibility,
    added_at: Option<i64>,
    authoritative: bool,
) -> ExternalWorktreeState {
    let mut hidden: Vec<String> = Vec::new();
    let mut shown = 0usize;
    for entry in worktrees.iter_mut() {
        let facts = zerocode_core::WorktreeFacts {
            path: &entry.path,
            branch: entry.branch.as_deref(),
        };
        // The checkout this switch is never talking about: the repository's own
        // (`is_main` is git's answer; `path == repo_root` catches a folder
        // project, whose single row is its root) — and **the one the window is
        // standing in**, whatever its provenance. That last clause is not
        // decoration. A worktree somebody cut by hand can be activated from the
        // jump palette, and a filter that hides the row you are standing on
        // reads as a broken list, not as a filter. Orca's own rule is the same
        // (`isSelectedCheckout` is the first branch of `shouldShowWorktree`).
        let itself = entry.active || entry.is_main || entry.path == repo_root;
        entry.ownership = zerocode_core::classify(facts, ours).slug();
        let visible = zerocode_core::should_show(facts, ours, stored, added_at, itself);
        entry.external_hidden = !visible;
        // Counted only where a listing really happened, and only for the rows a
        // person would be offered.
        if !authoritative || !zerocode_core::is_user_facing(facts, ours, itself) {
            continue;
        }
        if visible {
            shown += 1;
        } else {
            hidden.push(entry.path.clone());
        }
    }
    let inbox: Vec<String> = {
        let borrowed: Vec<&str> = hidden.iter().map(String::as_str).collect();
        zerocode_core::inbox_paths(&borrowed, stored, added_at)
            .into_iter()
            .map(str::to_string)
            .collect()
    };
    ExternalWorktreeState {
        visibility: zerocode_core::effective_visibility(stored, added_at).slug(),
        legacy: zerocode_core::is_legacy(stored, added_at),
        authoritative,
        shown,
        // The card that asks once is offered only where there is something to
        // ask about: "0 hidden worktrees" tells nobody anything.
        prompt: zerocode_core::should_offer_prompt(stored, added_at) && !hidden.is_empty(),
        hidden,
        inbox,
    }
}

/// Note this window's project as one that has been opened.
///
/// Recorded at boot rather than when the picker returns, so the list also
/// holds the project somebody opened from a terminal — and so the *first*
/// project a person ever opens is in it, which is the one case a
/// record-on-pick would always miss.
pub(super) fn note_project(
    repository: &settings::SettingsRepository,
    config_root: &Path,
    root: &Path,
) {
    if is_whole_filesystem(root) {
        return;
    }
    let file = recent_projects_file(config_root);
    let known = stored_projects(config_root);
    // The moment a project joins the list, written down once and only for a
    // project the list has never held.
    //
    // "Never held" is the whole guard. This runs at every boot, so stamping
    // unconditionally would date TODAY the project somebody has had open for
    // months — and a project dated after the rollout hides its external
    // worktrees by default, which is exactly the sidebar this date exists to
    // keep full. An absent date reads as "was already here", the safe answer.
    let listed = root.to_string_lossy().into_owned();
    if !known.iter().any(|held| held == &listed) {
        let at = now_epoch_ms();
        let _ = update_project_settings(repository, &project_settings_key(&listed), |entry| {
            entry.added_at = Some(at);
        });
    }
    let kept = remember_project(&known, &listed, MAX_PROJECTS);
    if let Some(parent) = file.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(text) = serde_json::to_string(&kept) {
        let _ = std::fs::write(&file, text);
    }
}

/// One blocking exchange with a pane's events channel, on its own thread.
///
/// The harness client's frame callback is not `Send`, so its futures cannot
/// ride tauri's shared runtime; every command that talks the protocol runs
/// its exchange through here on the blocking pool instead.
pub(super) fn with_client<T: Send + 'static>(
    addr: String,
    token: Option<String>,
    exchange: impl AsyncFnOnce(&mut Client) -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    // A dedicated OS thread, unconditionally. Callers reach here from sync
    // contexts AND from tauri's async runtime (the ZO tab launch resolves
    // there), and building a runtime on a runtime worker is the "Cannot
    // start a runtime from within a runtime" panic that killed every launch
    // (measured 2026-09-01). The join blocks the caller exactly as the old
    // inline block_on did — the contract of this door is a blocking exchange.
    std::thread::Builder::new()
        .name("zo-channel-exchange".into())
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|error| error.to_string())?;
            runtime.block_on(async {
                let mut client = Client::connect(&addr, token)
                    .await
                    .map_err(|error| error.to_string())?;
                exchange(&mut client).await
            })
        })
        .map_err(|error| error.to_string())?
        .join()
        .map_err(|_| "channel exchange thread panicked".to_string())?
}

/// Ask a pane-owned event channel which durable session it represents.
pub(super) fn pane_channel_session_id(
    addr: String,
    token: Option<String>,
) -> Result<String, String> {
    with_client(addr, token, async |client| {
        let info = client
            .call(method::INFO, json!({}))
            .await
            .map_err(|error| error.to_string())?;
        info.get("id")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
            .filter(|id| !id.is_empty())
            .ok_or_else(|| "session.info returned no id".to_string())
    })
}

/// Resolve a session only to the private channel its pane published.
pub(super) fn channel_addr(state: &AppState, session: &str) -> Result<String, String> {
    state
        .channels()
        .get(session)
        .cloned()
        .ok_or_else(|| format!("세션 {session}의 이벤트 채널을 모릅니다"))
}

/// Every live zo pane channel, as `(session, address)`.
///
/// Pure over the three registries so the routing can be asserted without an
/// app: a session is a target only when this window is holding its channel AND
/// a pane running `zo` reported it. A Claude or Codex pane never gets an
/// `auth.reload` — its agent re-reads the account directory at launch, and the
/// method is not on its wire at all.
pub(super) fn zo_channel_targets(
    channels: &HashMap<String, String>,
    pane_sessions: &HashMap<TermId, zerocode_core::ProviderSession>,
    agent_terms: &HashMap<TermId, &'static str>,
) -> Vec<(String, String)> {
    let mut targets: Vec<(String, String)> = channels
        .iter()
        .filter(|(session, _)| {
            pane_sessions.iter().any(|(term, held)| {
                held.id == **session
                    && agent_terms
                        .get(term)
                        .is_some_and(|agent| *agent == AgentKind::Zo.slug())
            })
        })
        .map(|(session, addr)| (session.clone(), addr.clone()))
        .collect();
    // One pane can be reported twice (a restored pane and its adoption), and
    // the map's order is not stable. Sorting and deduplicating makes the
    // broadcast exactly-once and its test deterministic.
    targets.sort();
    targets.dedup();
    targets
}

/// The params one `auth.reload` carries.
///
/// VALUES, not a bare "reload yourself": a pane was handed its account through
/// `CLAUDE_CONFIG_DIR` / `CODEX_HOME` when it was born, and nobody can change a
/// running child's environment from outside. The label is the display name this
/// window already shows in the account list — zo must not rebuild the masking
/// rule (`docs/design/zo-ide-account-oauth.md` §2.3).
pub(super) fn auth_reload_params(
    provider: zerocode_core::account::Provider,
    label: Option<&str>,
    claude_config_dir: Option<&Path>,
    codex_home: Option<&Path>,
) -> serde_json::Value {
    let mut params = json!({ "provider": account_provider_slug(provider) });
    let object = params.as_object_mut().expect("a JSON object");
    // Carried whenever the caller is speaking about the name at all — EMPTY
    // when the lane is back on the machine's own login. Leaving the key out
    // there would leave the pane showing the name of the account it no longer
    // speaks as.
    if let Some(label) = label.map(str::trim) {
        object.insert("label".to_string(), json!(label));
    }
    if let Some(dir) = claude_config_dir {
        object.insert(
            "claude_config_dir".to_string(),
            json!(dir.to_string_lossy()),
        );
    }
    if let Some(home) = codex_home {
        object.insert("codex_home".to_string(), json!(home.to_string_lossy()));
    }
    params
}

/// The name zo answers to for one provider lane.
pub(super) const fn account_provider_slug(
    provider: zerocode_core::account::Provider,
) -> &'static str {
    match provider {
        zerocode_core::account::Provider::Anthropic => "anthropic",
        zerocode_core::account::Provider::OpenAi => "openai",
    }
}

/// Tell every live zo pane, once each, that the account changed.
///
/// `send` is the transport so the fan-out can be asserted without sockets. A
/// pane that refuses or has already gone is skipped, not retried and never
/// fatal: an account switch must land in the picker even when one pane died
/// between the enumeration and the call. Returns how many panes took it.
pub(super) fn broadcast_auth_reload(
    targets: &[(String, String)],
    params: &serde_json::Value,
    mut send: impl FnMut(&str, &serde_json::Value) -> Result<(), String>,
) -> usize {
    targets
        .iter()
        .filter(|(_, addr)| send(addr, params).is_ok())
        .count()
}

/// The display name this window shows for the account one provider lane is on,
/// or `None` when the selection is the machine's own login.
///
/// The SAME string the account picker renders — zo is handed it rather than
/// rebuilding the masking rule from a credentials file
/// (`docs/design/zo-ide-account-oauth.md` §2.3).
pub(super) fn active_account_label(
    config_root: &Path,
    provider: zerocode_core::account::Provider,
) -> Option<String> {
    match provider {
        zerocode_core::account::Provider::Anthropic => {
            let store = accounts::read_store(config_root);
            zerocode_core::account::active_account(&store.accounts, &store.selection)
                .map(zerocode_core::ClaudeAccount::label)
        }
        zerocode_core::account::Provider::OpenAi => {
            let store = codex_accounts::read_store(config_root);
            zerocode_core::codex_account::active_account(&store.accounts, &store.selection)
                .map(zerocode_core::codex_account::CodexAccount::label)
        }
    }
}

/// Tell every live zo pane which account this window just switched to.
///
/// The values come from `account_env_for` — the same table a launch reads — so
/// a running pane and the next one cannot end up on different accounts. A lane
/// whose selection is now the machine's own login sends an EMPTY path, which
/// says "no managed account": saying nothing instead would leave the pane on
/// the environment it was born with, which names the account the person just
/// left.
///
/// Best effort, and deliberately so: a pane that died between the enumeration
/// and the call must not fail the switch the picker is waiting on.
pub(super) fn announce_account_switch(
    state: &AppState,
    provider: zerocode_core::account::Provider,
) {
    let config_root = state.config_root().to_path_buf();
    let env = match account_env_for(&config_root, AgentKind::Zo.slug()) {
        Ok(env) => env,
        // A selected Claude account whose login has gone cannot be
        // materialized, and the launch table refuses the WHOLE environment for
        // it. Saying nothing is right for that lane — a pane must not be
        // pushed onto a credential this window could not produce — but the
        // OpenAI lane the person just switched still has to reach them.
        Err(_) if provider == zerocode_core::account::Provider::OpenAi => {
            codex_accounts::launch_env(&config_root)
        }
        Err(_) => return,
    };
    let named = |name: &str| {
        env.iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| PathBuf::from(value))
            .unwrap_or_default()
    };
    // `None` here is the machine's own login, which is a name to CLEAR rather
    // than a name to leave alone — the switch door always knows.
    let label = active_account_label(&config_root, provider).unwrap_or_default();
    let (claude_config_dir, codex_home) = match provider {
        zerocode_core::account::Provider::Anthropic => {
            (Some(named(zerocode_core::account::CONFIG_DIR_VAR)), None)
        }
        zerocode_core::account::Provider::OpenAi => {
            (None, Some(named(zerocode_core::codex_account::HOME_VAR)))
        }
    };
    let params = auth_reload_params(
        provider,
        Some(label.as_str()),
        claude_config_dir.as_deref(),
        codex_home.as_deref(),
    );
    // The registries are read under their locks and released BEFORE the first
    // socket call: a channel exchange blocks its caller, and holding the pane
    // maps across one would stop every other command in this window.
    let targets = {
        let channels = state.channels();
        let sessions = state.pane_sessions();
        let agents = state.agent_terms();
        zo_channel_targets(&channels, &sessions, &agents)
    };
    if targets.is_empty() {
        return;
    }
    let token = state
        .supervisor()
        .and_then(|supervisor| supervisor.token().map(str::to_string));
    broadcast_auth_reload(&targets, &params, |addr, params| {
        let params = params.clone();
        with_client(addr.to_string(), token.clone(), async move |client| {
            client
                .call(method::AUTH_RELOAD, params)
                .await
                .map(|_| ())
                .map_err(|error| error.to_string())
        })
    });
}

/// Tell every live zo pane that this window's Google login changed.
///
/// Google is not in the core provider table on purpose: the window's login
/// writes into zo's OWN credentials file, so there is no launch environment to
/// hand over (설계 §2.4 — no second account store). What a running pane still
/// needs is the nudge: its Gemini client captured a bearer when it was built
/// and would keep using it until that token expires.
pub(super) fn announce_google_login(state: &AppState, label: Option<&str>) {
    let mut params = json!({ "provider": "google" });
    if let Some(label) = label.map(str::trim) {
        params
            .as_object_mut()
            .expect("a JSON object")
            .insert("label".to_string(), json!(label));
    }
    let targets = {
        let channels = state.channels();
        let sessions = state.pane_sessions();
        let agents = state.agent_terms();
        zo_channel_targets(&channels, &sessions, &agents)
    };
    if targets.is_empty() {
        return;
    }
    let token = state
        .supervisor()
        .and_then(|supervisor| supervisor.token().map(str::to_string));
    broadcast_auth_reload(&targets, &params, |addr, params| {
        let params = params.clone();
        with_client(addr.to_string(), token.clone(), async move |client| {
            client
                .call(method::AUTH_RELOAD, params)
                .await
                .map(|_| ())
                .map_err(|error| error.to_string())
        })
    });
}

pub(super) fn zo_pane_terms_for_session(app: &AppHandle, session: &str) -> Vec<TermId> {
    let state = app.state::<AppState>();
    let matching: Vec<TermId> = state
        .pane_sessions()
        .iter()
        .filter_map(|(term, held)| (held.id == session).then_some(*term))
        .collect();
    let agents = state.agent_terms();
    matching
        .into_iter()
        .filter(|term| {
            agents
                .get(term)
                .is_some_and(|agent| *agent == AgentKind::Zo.slug())
        })
        .collect()
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct ZoChannelReport {
    pub(super) state: zerocode_core::hook::HookState,
    pub(super) event: &'static str,
    pub(super) interrupted: bool,
    pub(super) acknowledges_prompt: bool,
    pub(super) ask: Option<String>,
}

/// A zo pane's `PushNotification` as the channel delivers it (t-2943): the
/// `notify` frame, on the window road, with something to say. The pane keeps
/// the title for its own terminal; the window composes its own from the
/// worktree and the agent, the way every bell here is titled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ZoPush {
    pub(super) body: String,
}

/// The push a `notify` frame carries, or `None`: a frame on the terminal road
/// already rang the pane's own terminal, a skipped road sent nothing, and an
/// empty body is a bell with nothing to say.
pub(super) fn zo_push_notice(frame: &serde_json::Value) -> Option<ZoPush> {
    if frame.get("type").and_then(serde_json::Value::as_str) != Some("notify") {
        return None;
    }
    if frame.get("road").and_then(serde_json::Value::as_str) != Some("window") {
        return None;
    }
    let body = frame
        .get("body")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|body| !body.is_empty())?;
    Some(ZoPush {
        body: body.to_string(),
    })
}

/// Fold the Zo event-channel vocabulary into the pane lifecycle vocabulary.
pub(super) fn zo_channel_report(frame: &serde_json::Value) -> Option<ZoChannelReport> {
    let kind = frame.get("type").and_then(serde_json::Value::as_str);
    let phase = frame.get("phase").and_then(serde_json::Value::as_str);
    let outcome = frame.get("outcome").and_then(serde_json::Value::as_str);
    let text = |key: &str| {
        frame
            .get(key)
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    };
    match (kind, phase) {
        (Some("turn"), Some("start")) => Some(ZoChannelReport {
            state: zerocode_core::hook::HookState::Working,
            event: "zo-turn-start",
            interrupted: false,
            acknowledges_prompt: true,
            ask: None,
        }),
        (Some("turn"), Some("end")) => Some(ZoChannelReport {
            state: zerocode_core::hook::HookState::Done,
            event: "zo-turn-end",
            interrupted: outcome == Some("cancelled"),
            acknowledges_prompt: false,
            ask: None,
        }),
        (Some("permission_prompt"), _) => Some(ZoChannelReport {
            state: zerocode_core::hook::HookState::NeedsAttention,
            event: "zo-permission-prompt",
            interrupted: false,
            acknowledges_prompt: false,
            ask: text("reasoning").or_else(|| text("tool_name")),
        }),
        (Some("user_question_prompt"), _) => Some(ZoChannelReport {
            state: zerocode_core::hook::HookState::NeedsAttention,
            event: "zo-user-question",
            interrupted: false,
            acknowledges_prompt: false,
            ask: text("question"),
        }),
        (Some("prompt_resolved"), _) => Some(ZoChannelReport {
            state: zerocode_core::hook::HookState::Working,
            event: "zo-prompt-resolved",
            interrupted: false,
            acknowledges_prompt: false,
            ask: None,
        }),
        _ => None,
    }
}

pub(super) fn acknowledge_zo_worker_prompt(app: &AppHandle, term: TermId) {
    if let Some(waiting) = app
        .state::<AppState>()
        .worker_prompt_submits()
        .remove(&term)
    {
        let _ = waiting.send(());
    }
}

pub(super) fn note_zo_pane_state(
    app: &AppHandle,
    term: TermId,
    state: zerocode_core::hook::HookState,
    event: &'static str,
    interrupted: bool,
    model: Option<String>,
    ask: Option<String>,
) {
    let session = app.state::<AppState>().pane_sessions().get(&term).cloned();
    note_pane_state(
        app,
        None,
        &hooks::PaneHookReport {
            permission_mode: None,
            term,
            agent: AgentKind::Zo,
            state,
            interrupted,
            session_boundary: false,
            child_attributed: false,
            submit_shape: zerocode_core::ask::SubmitShape::NotAQuestion,
            event: event.to_string(),
            resumable: session.is_some(),
            session,
            prompt: None,
            said: None,
            ask,
            ask_prompt: None,
            approval: None,
            model,
        },
    );
}

/// The registry a zo `subagents` frame says it is the whole of, when it says.
pub(super) fn zo_frame_registry(frame: &serde_json::Value) -> Option<String> {
    frame
        .get("registry")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
}

/// The frame's generation — the registry's roster-change counter — when it
/// carries one.
pub(super) fn zo_frame_generation(frame: &serde_json::Value) -> Option<u64> {
    frame.get("generation").and_then(serde_json::Value::as_u64)
}

/// Whether this frame is OLDER than one the window already folded for the
/// same session and registry — a stale snapshot arriving after a newer delta
/// (the subscribe replay racing the live stream), which folded as if current
/// would resurrect helpers that have since finished, or finish ones that
/// have since started. Kept per session and registry; a frame without a
/// generation is never stale, and never advances the mark.
pub(super) fn zo_frame_is_stale(
    session: &str,
    registry: Option<&str>,
    generation: Option<u64>,
) -> bool {
    static NEWEST: std::sync::OnceLock<std::sync::Mutex<HashMap<(String, String), u64>>> =
        std::sync::OnceLock::new();
    let (Some(registry), Some(generation)) = (registry, generation) else {
        return false;
    };
    let mut newest = NEWEST
        .get_or_init(|| std::sync::Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    // Bounded like every other session-keyed map here: a session that ended
    // never comes back under the same registry id, so the oldest marks are
    // safe to forget when the map grows past what a window ever holds live.
    if newest.len() >= 256
        && !newest.contains_key(&(session.to_string(), registry.to_string()))
        && let Some(oldest) = newest
            .iter()
            .min_by_key(|(_, generation)| **generation)
            .map(|(key, _)| key.clone())
    {
        newest.remove(&oldest);
    }
    let mark = newest
        .entry((session.to_string(), registry.to_string()))
        .or_insert(0);
    if generation < *mark {
        return true;
    }
    *mark = generation;
    false
}

/// One helper as a zo `subagents` frame describes it.
///
/// Two facts arrive together and travel apart. The ROW — name, state,
/// transcript, tool count — belongs to the roster the window keeps per pane.
/// What the helper is DOING does not: activity is filed under a CARD, where
/// every other vendor's is, so the sidebar and the board draw all of them with
/// the window's one `activityLine` rather than with a zo-shaped second reader.
pub(super) struct ZoHelper {
    pub(super) row: hooks::SubagentRow,
    pub(super) activity: Option<zerocode_core::hook::Activity>,
}

pub(super) fn zo_subagent_helpers(frame: &serde_json::Value) -> Option<Vec<ZoHelper>> {
    if frame.get("type").and_then(serde_json::Value::as_str) != Some("subagents") {
        return None;
    }
    let running = frame.get("running")?.as_array()?;
    Some(
        running
            .iter()
            .filter_map(|raw| {
                let id = raw.get("id").and_then(serde_json::Value::as_str)?.trim();
                if id.is_empty() {
                    return None;
                }
                let named = |key| {
                    raw.get(key)
                        .and_then(serde_json::Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                };
                let name = named("label")
                    .or_else(|| named("model"))
                    .unwrap_or_else(|| id.split('-').rfind(|part| !part.is_empty()).unwrap_or(id));
                Some(ZoHelper {
                    row: hooks::SubagentRow {
                        id: id.to_string(),
                        name: name.to_string(),
                        state: hooks::SubagentState::Running,
                        born_listed: true,
                        transcript: helper_transcript_named(id, named("transcript")),
                        // A vendor that counts for itself is believed; one
                        // that says nothing starts at nothing, and the tool
                        // road is what moves it.
                        tool_calls: raw
                            .get("tool_calls")
                            .and_then(serde_json::Value::as_u64)
                            .unwrap_or(0),
                        registry: None,
                    },
                    activity: named("activity").and_then(zerocode_core::hook::activity_said),
                })
            })
            .collect(),
    )
}

/// The helper's own transcript, as the vendor named it — and only when the
/// name is the helper's.
///
/// zo writes each helper's conversation beside its manifest as
/// `<id>.session.jsonl` and says so in its `subagents` frame. The window later
/// reads that file for the helper's page (`subagent_log`), so the frame is
/// held to the one file that can be this helper's: a path whose file name is
/// not the id's is somebody else's file, and is not kept.
pub(super) fn helper_transcript_named(id: &str, said: Option<&str>) -> Option<PathBuf> {
    let path = PathBuf::from(said?);
    let wanted = format!("{id}.session.jsonl");
    (path.file_name().and_then(|name| name.to_str()) == Some(wanted.as_str())).then_some(path)
}

/// Parse the status payload through the same type the board round trip knows.
/// A malformed or older frame contributes no new fact; it must not take down
/// the pane's lifecycle stream merely because optional display metadata grew.
pub(super) fn zo_session_autonomy(
    frame: &serde_json::Value,
) -> Option<zerocode_core::board::BoardCardAutonomy> {
    if frame.get("type").and_then(serde_json::Value::as_str) != Some("session_status") {
        return None;
    }
    serde_json::from_value(frame.get("autonomy")?.clone()).ok()
}

pub(super) fn note_zo_session_frame(app: &AppHandle, session: &str, frame: &serde_json::Value) {
    let terms = zo_pane_terms_for_session(app, session);
    if terms.is_empty() {
        return;
    }
    if let Some(helpers) = zo_subagent_helpers(frame) {
        // Which roster this frame is the whole of, and how far along it is —
        // the two marks the session-registry design puts on every
        // `subagents` frame (§3). Absent on a producer that predates them,
        // in which case the frame keeps its one-roster meaning and nothing
        // is dropped.
        let registry = zo_frame_registry(frame);
        if zo_frame_is_stale(session, registry.as_deref(), zo_frame_generation(frame)) {
            return;
        }
        for term in terms {
            // The frame names what is running; what it stopped naming has
            // finished and stays as a finished row (`fold_helper_roster`),
            // so five helpers run in parallel do not vanish one by one — and
            // what ANOTHER registry named is not this frame's to retire.
            let rows = {
                let state = app.state::<AppState>();
                let mut held = state.subagents();
                let roster = held.entry(term).or_default();
                hooks::fold_helper_roster_from(
                    roster,
                    helpers.iter().map(|helper| helper.row.clone()).collect(),
                    registry.as_deref(),
                );
                let rows = roster.clone();
                if rows.is_empty() {
                    held.remove(&term);
                }
                rows
            };
            publish_pane_subagents(app, term, rows.clone());
            // And what each of them is DOING, onto its own card — the road
            // every vendor's activity travels, so one `activityLine` in the
            // window draws a helper's line the way it draws its parent's.
            for helper in &helpers {
                if let Some(activity) = helper.activity.clone() {
                    note_helper_activity(
                        app,
                        hooks::activity_subagent(term, &helper.row.id),
                        activity,
                    );
                }
            }
            let waiting = app
                .state::<AppState>()
                .pane_states()
                .get(&term)
                .is_some_and(|held| held.state == zerocode_core::hook::HookState::NeedsAttention);
            if hooks::any_helper_running(&rows) && !waiting {
                note_zo_pane_state(
                    app,
                    term,
                    zerocode_core::hook::HookState::Working,
                    "zo-subagents",
                    false,
                    None,
                    None,
                );
            }
        }
        return;
    }
    if let Some(push) = zo_push_notice(frame) {
        // The agent asked for the person on purpose. Not a state change —
        // the pane keeps working — so it never touches the pane row; it
        // rings through the one ladder, addressed to the pane a click
        // comes back to.
        for term in terms {
            ring_zo_push(app, term, &push);
        }
        return;
    }
    let kind = frame.get("type").and_then(serde_json::Value::as_str);
    if let Some(report) = zo_channel_report(frame) {
        for term in terms {
            if report.acknowledges_prompt {
                acknowledge_zo_worker_prompt(app, term);
            }
            note_zo_pane_state(
                app,
                term,
                report.state,
                report.event,
                report.interrupted,
                None,
                report.ask.clone(),
            );
        }
        return;
    }
    if kind == Some("session_status") {
        let autonomy = zo_session_autonomy(frame);
        let model = frame
            .get("model")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|model| !model.is_empty())
            .map(str::to_string);
        for term in terms {
            if let Some(held) = app.state::<AppState>().pane_states().get_mut(&term) {
                held.autonomy = autonomy.clone();
            }
            // Metadata repaints between turns without replaying a hook that
            // can dismiss a question or announce a second turn end.
            let _ = app.emit(
                "pane:autonomy",
                serde_json::json!({ "term": term, "session": session,
                    "autonomy": autonomy, "model": model }),
            );
        }
    }
}

pub(super) fn note_zo_session_history(app: &AppHandle, session: &str, history: &serde_json::Value) {
    let Some(history) = history.as_array() else {
        return;
    };
    for kind in ["turn", "session_status", "subagents"] {
        if let Some(frame) = history
            .iter()
            .rev()
            .find(|frame| frame.get("type").and_then(serde_json::Value::as_str) == Some(kind))
        {
            note_zo_session_frame(app, session, frame);
        }
    }
}

pub(super) fn clear_zo_session_subagents(app: &AppHandle, session: &str) {
    for term in zo_pane_terms_for_session(app, session) {
        if let Some(held) = app.state::<AppState>().pane_states().get_mut(&term) {
            held.autonomy = None;
        }
        let _ = app.emit(
            "pane:autonomy",
            serde_json::json!({ "term": term, "session": session, "autonomy": null }),
        );
        app.state::<AppState>().subagents().remove(&term);
        publish_pane_subagents(app, term, Vec::new());
    }
}

/// Release a fresh Zo worker's briefing only after its subscriber is live.
pub(super) fn activate_zo_worker_delivery(app: &AppHandle, owner: ZoChannelOwner, session: &str) {
    let ZoChannelOwner::Term(term) = owner else {
        return;
    };
    let state = app.state::<AppState>();
    let same_session = state
        .pane_sessions()
        .get(&term)
        .is_some_and(|held| held.id == session);
    if !same_session {
        return;
    }
    // A refused exact launch (t-2773): the parked briefing is settled as
    // withheld before it can reach the line, its waiter told by name — the
    // worker start then fails as a launch that did not accept its briefing,
    // and the person sees the guard's own sentence in the pane and the board.
    if zo_integration_runtime::delivery_refusal(app, term).is_some() {
        let Some(delivery) = state.zo_worker_deliveries().remove(&term) else {
            return;
        };
        state.worker_prompt_submits().remove(&term);
        let refusal = zerocode_pty::ready::Refusal::LaunchRefused;
        if let Some(waiting) = state.delivery_waiters().remove(&term) {
            let _ = waiting.send(DeliveryOutcome::Refused(refusal));
        }
        let _ = app.emit(
            "term:prompt",
            PromptSettled {
                term,
                delivered: false,
                pasted: false,
                why: Some(refusal.says()),
                text: Some(delivery.text().to_string()),
            },
        );
        return;
    }
    let delivery = state.zo_worker_deliveries().remove(&term);
    if let Some(delivery) = delivery {
        state.deliveries().insert(term, delivery);
        state.cadence().wake();
    }
}

/// Stream one session's structured frames into the webview.
///
/// Its own thread with its own connection: `subscribe` turns the socket into
/// a firehose, and sharing it with request/response traffic would interleave
/// frames into someone else's reply.
///
/// One subscriber per session, enforced here rather than trusted to callers:
/// re-attaching to a session this window already listens to used to add a
/// second channel, and every `permission_prompt` then arrived twice — one
/// modal for a prompt the person had already answered, carrying a
/// `prompt_id` the server had retired.
///
/// The thread ends when the server closes the stream, when the connection
/// fails, or when [`close_lane`] drops the session from `live` — and it
/// always says so, because this channel is the only path a permission prompt
/// takes (제품 원칙 1).
#[allow(clippy::too_many_arguments)]
pub(super) fn spawn_subscriber(
    app: AppHandle,
    owner: ZoChannelOwner,
    epoch: u64,
    addr: String,
    token: Option<String>,
    session: String,
    live: Arc<Mutex<HashSet<String>>>,
) {
    std::thread::spawn(move || {
        let id = session.clone();
        let emitter = app.clone();
        let cancel = Arc::clone(&live);
        let probe_addr = addr.clone();
        let probe_token = token.clone();
        let result = with_client(addr, token, async move |client| {
            zo_integration_runtime::negotiate(&emitter, owner, epoch, &probe_addr, probe_token)
                .await;
            if !zo_integration_runtime::is_current(&emitter.state::<AppState>(), owner, epoch) {
                return Ok(());
            }
            let hydrated = client
                .subscribe(&session, true)
                .await
                .map_err(|error| error.to_string())?;
            activate_zo_worker_delivery(&emitter, owner, &session);
            if let Some(history) = hydrated.get("history") {
                for frame in history.as_array().into_iter().flatten() {
                    zo_integration_runtime::frame(&emitter, owner, epoch, frame);
                }
                note_zo_session_history(&emitter, &session, history);
            }
            let _ = emitter.emit(
                "session:history",
                json!({ "session": session, "history": hydrated.get("history") }),
            );
            loop {
                match client.next_incoming().await {
                    Ok(Some(zerocode_harness::Incoming::Frame(frame))) => {
                        // Checked **before** forwarding: the lane that wanted
                        // this channel is gone, so a modal raised now would
                        // point at a lane the person already closed.
                        if !zo_integration_runtime::is_current(
                            &emitter.state::<AppState>(),
                            owner,
                            epoch,
                        ) || !subscribed(&cancel).contains(&session)
                            || emitter.state::<AppState>().channel_owners().get(&session)
                                != Some(&owner)
                        {
                            return Ok(());
                        }
                        zo_integration_runtime::frame(&emitter, owner, epoch, &frame);
                        note_zo_session_frame(&emitter, &session, &frame);
                        let _ = emitter.emit(
                            "session:frame",
                            json!({ "session": session, "frame": frame }),
                        );
                    }
                    Ok(Some(zerocode_harness::Incoming::Response { .. })) => {}
                    Ok(None) => return Ok(()),
                    Err(error) => return Err(error.to_string()),
                }
            }
        });
        // End the exact claim this thread was born from. A duplicate attach
        // never became an owner, so it can neither arrive here nor tear down
        // the winning lane/terminal's command route.
        if !zo_integration_runtime::is_current(&app.state::<AppState>(), owner, epoch) {
            return;
        }
        zo_integration_runtime::disconnected(&app, owner, epoch);
        if detach_zo_channel_if_owned(&app.state::<AppState>(), &id, owner) {
            clear_zo_session_subagents(&app, &id);
        }
        // A subscriber dying must not take the window down — the lane keeps
        // rendering through the pty — but it must not be silent either. The
        // webview gates the lane on this, rather than leaving a lane that can
        // no longer report a permission prompt looking idle.
        // Error envelopes can carry arbitrary private data. Only a fixed reason leaves this boundary.
        let reason = result.err().map(|_| "channel-disconnected");
        let _ = app.emit("session:ended", json!({ "session": id, "reason": reason }));
    });
}

/// One directory level, for the file panel and the folder browser.
///
/// The tree's `list_dir` hands out levels of the project only and refuses a
/// path that escapes it; the browser's `browse_dir` walks anywhere, because
/// choosing a folder to open IS browsing anywhere. Both read a level through
/// [`read_dir_entries`], so the two never sort differently.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(super) struct DirEntry {
    pub(super) name: String,
    pub(super) is_dir: bool,
}

/// One level of `target`, directories first and each half alphabetical —
/// the order every file panel a person has used sorts in — cut at `cap`
/// entries when one is given. Answers the entries kept and how many there
/// were before the cut, so a caller can say "… 외 N" honestly.
///
/// Sorted BEFORE the cut: a cap applied to `read_dir`'s own order would keep
/// an arbitrary five thousand of a bigger folder, and the folder's first
/// screen would differ from run to run.
pub(super) fn read_dir_entries(
    target: &Path,
    cap: Option<usize>,
) -> std::io::Result<(Vec<DirEntry>, usize)> {
    let mut entries: Vec<DirEntry> = std::fs::read_dir(target)?
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let name = entry.file_name().to_string_lossy().into_owned();
            // A symlink to a folder is a folder to browse into — `file_type`
            // says "symlink" and would file it under the files.
            let is_dir = entry
                .file_type()
                .ok()
                .is_some_and(|kind| kind.is_dir() || (kind.is_symlink() && entry.path().is_dir()));
            Some(DirEntry { name, is_dir })
        })
        .collect();
    entries.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then(a.name.cmp(&b.name)));
    let total = entries.len();
    if let Some(cap) = cap {
        entries.truncate(cap);
    }
    Ok((entries, total))
}

/// A file's text and the state of the file it came out of.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TextFile {
    pub(super) text: String,
    pub(super) version: String,
    /// Present only for an explicitly allowed recovery read whose path is
    /// safely inside the chosen root but has no file on disk yet.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub(super) missing: bool,
}

impl TextFile {
    fn missing() -> Self {
        Self {
            text: String::new(),
            version: String::new(),
            missing: true,
        }
    }
}

/// What the refusal of a stale save is called, so the window can recognise it.
///
/// A marker, not prose. The window has to tell this failure apart from every
/// other one — it is the only one with a way forward rather than just a
/// message — and matching on a sentence would break the moment somebody
/// reworded it. The sentence is the window's to write, in its own languages.
pub(super) const STALE_SAVE: &str = "zerocode:stale-save";

/// What a file was, as one value: when it changed, how long it is, and a
/// digest of what is in it.
///
/// The first two come free out of a stat, and for a while they were the whole
/// stamp — a content digest costs a read, and this is asked on every save and
/// every tab activation. That was wrong, and the test named after it says why:
/// mtime and length CANNOT tell two files apart. An agent that rewrites a line
/// in place, inside one filesystem tick, leaves both of them identical, and on
/// a filesystem whose clock is coarser than this window's edits that is not a
/// coincidence to wait for. What the collision costs is somebody's work, so
/// the read is the right price.
///
/// The digest is over the bytes the caller actually has, so a version
/// describes the text that was handed over rather than a stat that happened
/// near it.
pub(super) fn stamp(bytes: &[u8], meta: &std::fs::Metadata) -> String {
    let moved = meta
        .modified()
        .ok()
        .and_then(|at| at.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|since| since.as_nanos())
        .unwrap_or(0);
    format!("{moved}:{}:{:016x}", meta.len(), digest(bytes))
}

/// FNV-1a over the file's bytes.
///
/// Written out rather than taken from `std`: `DefaultHasher`'s output is
/// explicitly not stable across releases, and this value crosses a wire and
/// comes back. Not a cryptographic digest and does not need to be — the thing
/// it has to tell apart is two versions of a file, not a forgery.
pub(super) fn digest(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

pub(super) fn stamp_of(target: &Path) -> Result<String, String> {
    let meta = std::fs::metadata(target).map_err(|error| error.to_string())?;
    let bytes = std::fs::read(target).map_err(|error| error.to_string())?;
    Ok(stamp(&bytes, &meta))
}

/// Read a file, and say what it was when it was read.
///
/// The version is built from the bytes this returns, so it describes the text
/// the window is actually holding. A file that changes mid-read therefore
/// leaves a version matching neither the old file nor the new one, and the
/// next save is refused — the person is told, and nothing is lost. That is the
/// safe direction for an unclear answer to fall.
///
/// THE CAP IS 4 MiB, and it was 512 KiB until the editor could carry it.
///
/// The old number was chosen for a `<textarea>` with a `<pre>` of line numbers
/// beside it: at half a megabyte that pair is already redrawing the whole
/// gutter on every keystroke. The window now mounts CodeMirror, which renders
/// only the visible lines and parses incrementally, so the reason for the
/// smaller number went away with the widget that needed it.
///
/// Measured against what we are matching: Orca's text read path is a 10 MB-class
/// ceiling, and a 512 KiB refusal met ordinary source files — `ui/shell.js` in
/// this very repository is over a megabyte and could not be opened in the
/// window that edits it. Raising this WITH the highlighting is the point;
/// shipping syntax colour under a cap that still refuses real files would have
/// moved the complaint rather than answered it.
pub(super) fn read_text_at(target: &Path) -> Result<TextFile, String> {
    const MAX_VIEW_BYTES: u64 = 4 * 1024 * 1024;
    let meta = std::fs::metadata(target).map_err(|error| error.to_string())?;
    if meta.len() > MAX_VIEW_BYTES {
        return Err(format!("파일이 너무 큽니다 ({} KiB)", meta.len() / 1024));
    }
    let bytes = std::fs::read(target).map_err(|error| error.to_string())?;
    if bytes.contains(&0) {
        return Err("바이너리 파일입니다".to_string());
    }
    Ok(TextFile {
        version: stamp(&bytes, &meta),
        text: String::from_utf8_lossy(&bytes).into_owned(),
        missing: false,
    })
}

/// Turn only a genuine absence into the empty recovery buffer.
///
/// Kept separate from the filesystem walk so the distinction that matters —
/// `NotFound` versus permission and every other error — is directly testable.
pub(super) fn text_file_if_missing(error: std::io::Error) -> Result<TextFile, String> {
    if error.kind() == std::io::ErrorKind::NotFound {
        Ok(TextFile::missing())
    } else {
        Err(error.to_string())
    }
}

/// Read an existing text file, or report an explicitly allowed safe absence.
///
/// The ordinary road is exactly [`resolve_in_project`] followed by
/// [`read_text_at`]. Recovery takes the missing-path resolver only after that
/// road fails, then checks the target without following links: a broken link,
/// directory, or inaccessible name is not an absent draft target.
pub(super) fn read_text_in_project(
    root: &Path,
    path: &str,
    allow_missing: bool,
) -> Result<TextFile, String> {
    let unresolved = match resolve_in_project(root, path) {
        Ok(target) => return read_text_at(&target),
        Err(error) => error,
    };
    if !allow_missing {
        return Err(unresolved);
    }
    let target = resolve_new_in_project(root, path)?;
    match std::fs::symlink_metadata(&target) {
        Err(error) => text_file_if_missing(error),
        Ok(_) => Err(unresolved),
    }
}

/// Which save this is, and then that save.
///
/// The whole of the decision lives here so that the two roads below stay what
/// they are: [`write_text_at`] is the save that has always existed, untouched,
/// version check and all, and [`recreate_text_at`] is the new one, which never
/// runs unless this decided it should.
pub(super) fn save_text_at(
    root: &Path,
    path: &str,
    text: &str,
    version: &str,
    create_if_missing: bool,
) -> Result<String, String> {
    // The usual door canonicalises, and canonicalising something that is not
    // there fails — so a deleted file cannot arrive through it at all. Where
    // it WOULD be is resolved by the sibling rule, which stands on the
    // deepest ancestor that does exist and puts that through this same door:
    // a path that escapes the checkout still escapes nothing.
    let target = match resolve_in_project(root, path) {
        Ok(found) => found,
        Err(_) => resolve_new_in_project(root, path)?,
    };
    match save_intent::intended(found_at(&target), create_if_missing) {
        SaveIntent::Overwrite => write_text_at(&target, text, version),
        SaveIntent::Recreate => recreate_text_at(&target, text),
        SaveIntent::Refuse => Err("파일이 아닙니다".to_string()),
    }
}

/// The one look at the disk the save rule is given.
///
/// `symlink_metadata` for the second question, because it does not follow: a
/// symlink pointing at nothing is something standing at that path, and
/// reporting it as [`OnDisk::Absent`] would let a recreate rename over it.
pub(super) fn found_at(target: &Path) -> OnDisk {
    if target.is_file() {
        OnDisk::AFile
    } else if std::fs::symlink_metadata(target).is_ok() {
        OnDisk::Other
    } else {
        OnDisk::Absent
    }
}

pub(super) fn write_text_at(target: &Path, text: &str, version: &str) -> Result<String, String> {
    const MAX_VIEW_BYTES: usize = 4 * 1024 * 1024;
    if text.len() > MAX_VIEW_BYTES {
        return Err(format!("파일이 너무 큽니다 ({} KiB)", text.len() / 1024));
    }
    // A directory resolves fine and would take the write as far as the open.
    if !target.is_file() {
        return Err("파일이 아닙니다".to_string());
    }
    // Before the temporary is made, not after. A refusal that has already
    // created a file leaves litter in the person's checkout for a save that
    // never happened — and it shows up in the source-control panel next to
    // their actual work.
    if stamp_of(target)? != version {
        return Err(STALE_SAVE.to_string());
    }
    let permissions = std::fs::metadata(target)
        .map_err(|error| error.to_string())?
        .permissions();
    let (beside, mut file) = temp_beside(target)?;
    if let Err(error) = file.set_permissions(permissions) {
        drop(file);
        let _ = std::fs::remove_file(&beside);
        return Err(error.to_string());
    }
    let written = std::io::Write::write_all(&mut file, text.as_bytes());
    // Closed before the rename: a handle still open to the source is fine on
    // unix and is not on Windows, and this is the one place both matter.
    drop(file);
    if let Err(error) = written {
        let _ = std::fs::remove_file(&beside);
        return Err(error.to_string());
    }
    rename_if_unmoved(&beside, target, version, text.as_bytes())
}

/// Make a file that is gone, at the address it had.
///
/// Reached only when [`save_intent::intended`] said so — the window was told
/// the file was deleted, and nothing is standing at the path now. What arrives
/// here is therefore a rescue, not a save: the text in the tab is the last
/// copy of that file anywhere, and every editor a person has used puts it
/// back.
///
/// The same temporary-and-rename as the ordinary save, for the same reason,
/// and the parent is made first: deleting a file often means deleting the
/// folder it was in — `git checkout` of a branch without it does exactly that
/// — and a rescue that fails because the folder went too rescues nothing.
/// `create_dir_all` reaches no further than the target's own parent, and the
/// target was resolved inside the checkout before this was called.
///
/// What it does NOT reuse is [`rename_if_unmoved`]: that one refuses when the
/// target is missing, which is this road's starting condition. The guard it
/// stands in for is the other direction — the file coming BACK while this
/// write was in flight, put there by an agent or a branch switch. Renaming
/// over that would bury a file this window has never seen, so it is refused
/// the same way every other mid-save move is, and the person is told.
pub(super) fn recreate_text_at(target: &Path, text: &str) -> Result<String, String> {
    const MAX_VIEW_BYTES: usize = 4 * 1024 * 1024;
    if text.len() > MAX_VIEW_BYTES {
        return Err(format!("파일이 너무 큽니다 ({} KiB)", text.len() / 1024));
    }
    let parent = target.parent().ok_or("파일이 아닙니다")?;
    std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let (beside, mut file) = temp_beside(target)?;
    let written = std::io::Write::write_all(&mut file, text.as_bytes());
    drop(file);
    if let Err(error) = written {
        let _ = std::fs::remove_file(&beside);
        return Err(error.to_string());
    }
    if found_at(target) != OnDisk::Absent {
        let _ = std::fs::remove_file(&beside);
        return Err(STALE_SAVE.to_string());
    }
    std::fs::rename(&beside, target).map_err(|error| {
        let _ = std::fs::remove_file(&beside);
        error.to_string()
    })?;
    stamp_written(target, text.as_bytes())
}

/// Where a file that is NOT on the disk would go, inside the project.
///
/// [`resolve_in_project`] canonicalises, and canonicalising a path that is not
/// there fails — which is exactly right for every reader, and is why a save
/// that means to make a file again needs this instead. The containment rule is
/// not copied: the deepest ancestor that DOES exist goes through
/// `resolve_in_project` itself, so the answer stands on ground that door
/// approved, and the part that does not exist yet is names only.
///
/// Names only is the other half. An absolute path is first stripped against
/// this root (including its canonical spelling), then it has the same rules as
/// a relative one: nothing but plain components, so no `..` or foreign prefix.
/// A component that exists but resolves outside the checkout is refused rather
/// than walked past: a symlink in the middle of the path is the escape this
/// whole rule is for.
pub(super) fn resolve_new_in_project(root: &Path, path: &str) -> Result<PathBuf, String> {
    let canonical_root = root.canonicalize().map_err(|error| error.to_string())?;
    let asked = Path::new(path);
    let relative = if asked.is_absolute() {
        asked
            .strip_prefix(root)
            .or_else(|_| asked.strip_prefix(&canonical_root))
            .map_err(|_| "path escapes the project".to_string())?
    } else {
        asked
    };
    let mut named: Vec<std::ffi::OsString> = Vec::new();
    for part in relative.components() {
        match part {
            std::path::Component::Normal(name) => named.push(name.to_os_string()),
            std::path::Component::CurDir => {}
            _ => return Err("path escapes the project".to_string()),
        }
    }
    let Some((name, folders)) = named.split_last() else {
        return Err("파일이 아닙니다".to_string());
    };
    let mut standing = folders;
    let ground = loop {
        let walked: PathBuf = standing.iter().collect();
        if std::fs::symlink_metadata(canonical_root.join(&walked)).is_ok() {
            break resolve_in_project(&canonical_root, &walked.to_string_lossy())?;
        }
        let Some((_, shorter)) = standing.split_last() else {
            // The checkout root itself did not answer, so there is no ground
            // to stand on and nothing to be made.
            return Err("path escapes the project".to_string());
        };
        standing = shorter;
    };
    Ok(ground
        .join(folders[standing.len()..].iter().collect::<PathBuf>())
        .join(name))
}

/// Put the finished temporary over the target, unless the target moved while
/// that temporary was being written.
///
/// The check at the top of a save is not enough on its own. Writing the
/// temporary takes time, and an agent in this worktree can rewrite the target
/// during it — a save that only looked once would rename its temporary over a
/// change nobody has seen. So the target is asked again here, as the last
/// thing before the rename.
///
/// This shrinks the window to the gap between this stat and the rename
/// syscall. It does not close it, and nothing available here would: closing it
/// needs a lock every writer takes, and the other writers are other people's
/// programs. A target that has vanished refuses too — a save is a save of a
/// file, not the creation of one.
pub(super) fn rename_if_unmoved(
    beside: &Path,
    target: &Path,
    version: &str,
    bytes: &[u8],
) -> Result<String, String> {
    match stamp_of(target) {
        Ok(found) if found == version => {}
        _ => {
            let _ = std::fs::remove_file(beside);
            return Err(STALE_SAVE.to_string());
        }
    }
    std::fs::rename(beside, target).map_err(|error| {
        // The temporary is this function's litter, and leaving it in the
        // person's checkout would show up as an untracked file they did not
        // make — in the source-control panel, next to their actual work.
        let _ = std::fs::remove_file(beside);
        error.to_string()
    })?;
    stamp_written(target, bytes)
}

/// The stamp of what this save just put on disk, checked against what it
/// meant to write.
///
/// The version handed back is what the window sends with its NEXT save, so it
/// has to describe the bytes THIS save put there. Simply looking at the file
/// again would describe whatever is there now — and between the rename and
/// that look, somebody else's write can land. The window would then believe it
/// was in step with a file it had never seen, and its next save would carry a
/// matching version and overwrite that file without asking: the guard
/// defeating itself, which is worse than not having one.
///
/// A mismatch is that race, caught. The save did land; the file has already
/// moved on, so it is reported the same way any other move is.
pub(super) fn stamp_written(target: &Path, bytes: &[u8]) -> Result<String, String> {
    let meta = std::fs::metadata(target).map_err(|error| error.to_string())?;
    let found = std::fs::read(target).map_err(|error| error.to_string())?;
    if found != bytes {
        return Err(STALE_SAVE.to_string());
    }
    Ok(stamp(bytes, &meta))
}

/// Create the file this save will be written into, beside its target.
///
/// `create_new` rather than a plain write, and that is the whole point of this
/// function existing. The temporary's path is *derived from the target's name*,
/// so it is a path a repository can predict and occupy: a symlink checked in at
/// exactly `<name>.zerocode-tmp` would be followed by `std::fs::write`, which
/// truncates whatever it points at — anywhere on the disk — and the rename
/// afterwards would move the link over the file being saved. `resolve_in_project`
/// cannot catch it, because it canonicalises a path that exists and this one
/// does not exist yet. Refusing anything already there closes it outright.
///
/// A temporary left by a save that died mid-write would otherwise wedge saving
/// that file forever, so one is cleared — but only when it is a regular file.
/// Something else at that path is the attack, not litter, and is refused.
pub(super) fn temp_beside(target: &Path) -> Result<(PathBuf, std::fs::File), String> {
    let beside = target.with_extension(format!(
        "{}.zerocode-tmp",
        target
            .extension()
            .map(|held| held.to_string_lossy().into_owned())
            .unwrap_or_default()
    ));
    let fresh = || {
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&beside)
    };
    match fresh() {
        Ok(file) => Ok((beside, file)),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            // `symlink_metadata` does not follow, so this asks what is really
            // sitting there rather than what it points to.
            let held = std::fs::symlink_metadata(&beside).map_err(|error| error.to_string())?;
            if !held.file_type().is_file() {
                return Err("임시 파일 자리에 파일이 아닌 것이 있습니다".to_string());
            }
            std::fs::remove_file(&beside).map_err(|error| error.to_string())?;
            let file = fresh().map_err(|error| error.to_string())?;
            Ok((beside, file))
        }
        Err(error) => Err(error.to_string()),
    }
}

/// A path the webview named, resolved to a real file inside the project.
///
/// One function rather than the rule written out at each reader: it is the
/// only thing standing between a path the window handed over and the rest of
/// the disk, and a second copy is a second chance to get it wrong. Both ends
/// are canonicalised first, so a symlink pointing out of the checkout is
/// caught by the same comparison as `../`.
pub(super) fn resolve_in_project(root: &Path, path: &str) -> Result<PathBuf, String> {
    let root = root.canonicalize().map_err(|error| error.to_string())?;
    let target = root
        .join(path)
        .canonicalize()
        .map_err(|error| error.to_string())?;
    if !target.starts_with(&root) {
        return Err("path escapes the project".to_string());
    }
    Ok(target)
}

/// An image, as something an `<img>` can be pointed at.
#[derive(Serialize)]
pub(super) struct ImageFile {
    /// The media type, from the extension. Not sniffed: a viewer that decides
    /// a file is a PNG because its first bytes look like one would render
    /// whatever a `.txt` happened to begin with.
    pub(super) mime: &'static str,
    /// base64, ready to go straight into a `data:` URL.
    pub(super) data: String,
    /// What it cost, for the caption — the one fact about an image the window
    /// can state without decoding it.
    pub(super) bytes: u64,
}

/// The extensions this window will show, and what each one is.
///
/// A list rather than a sniff, and a short one: every entry is a format the
/// webview can decode on its own. SVG is here because it is an image in an
/// `<img>`, which is the one context where it cannot run script.
pub(super) const IMAGE_TYPES: &[(&str, &str)] = &[
    ("png", "image/png"),
    ("jpg", "image/jpeg"),
    ("jpeg", "image/jpeg"),
    ("gif", "image/gif"),
    ("webp", "image/webp"),
    ("avif", "image/avif"),
    ("bmp", "image/bmp"),
    ("ico", "image/x-icon"),
    ("svg", "image/svg+xml"),
];

/* ---- 파일 트리의 손 (P0-15 후반) ----
 *
 * Orca 트리 우클릭(file-explorer-row-context-menu.tsx:150-309)이 디스크에
 * 대는 손들. 판단(담장·이름 규칙)은 file_tree_ops가 쥐고, 여기는 그 판정
 * 뒤의 시스템 호출뿐이다. 모든 길은 활성 워크트리 담장 안에서만 걷는다 —
 * 경로는 창에서 오고, 창에서 오는 것은 무엇이든 될 수 있었다. */

pub(super) fn fenced_path(state: &State<'_, AppState>, path: &str) -> Result<PathBuf, String> {
    let asked = PathBuf::from(path);
    if !file_tree_ops::inside_root(&state.active_root(), &asked) {
        return Err("워크스페이스 밖의 경로입니다".into());
    }
    Ok(asked)
}

/// How many paths one probe will answer for.
///
/// The list comes from whatever a program printed on a screen, so its length
/// is not ours to trust. Named because the cap has to be readable next to the
/// reason it exists.
pub(super) const TERM_LINK_PROBE_CAP: usize = 256;

/// Show a session's transcript in the file manager.
///
/// Bounded to the session stores, canonicalised on both sides — the same shape as
/// `reveal_skill` and for the same reason: this takes a path from a webview, and a
/// reveal that trusted it would open any file on the machine. The bound is the
/// discovery table itself, so an agent added to [`zerocode_core::vault::AGENT_SOURCES`]
/// A unique landing spot in the downloads directory — Orca saves without a
/// dialog and dedupes by counting ("name (2).ext"), and so does this. A
/// hundred takes is the same ceiling the untitled-markdown mint uses; past
/// it the caller gets an error instead of an overwrite.
pub(super) fn reserved_download_path(
    dir: &std::path::Path,
    wanted: &str,
) -> Result<PathBuf, String> {
    let clean = wanted.trim().trim_matches('.').replace(['/', '\\'], "_");
    let name = if clean.is_empty() {
        "download".to_string()
    } else {
        clean
    };
    let (stem, ext) = match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => (stem.to_string(), format!(".{ext}")),
        _ => (name.clone(), String::new()),
    };
    for take in 0..100 {
        let candidate = if take == 0 {
            dir.join(&name)
        } else {
            dir.join(format!("{stem} ({}){ext}", take + 1))
        };
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    Err("다운로드 이름을 백 번 시도해도 자리가 없습니다".into())
}

/// The two hands under a finished download row. Both refuse anything outside
/// the downloads directory: the renderer names a path, and a path is exactly
/// the kind of input this process must not `open` on faith.
pub(super) fn downloaded_file_checked(app: &AppHandle, path: &str) -> Result<PathBuf, String> {
    let target = std::path::PathBuf::from(path);
    if !target.is_file() {
        return Err("그 경로에 파일이 없습니다".into());
    }
    let real = target.canonicalize().map_err(|error| error.to_string())?;
    let downloads = app
        .path()
        .download_dir()
        .map_err(|error| error.to_string())?
        .canonicalize()
        .map_err(|error| error.to_string())?;
    if !real.starts_with(&downloads) {
        return Err("다운로드 폴더 안의 파일만 열 수 있습니다".into());
    }
    Ok(real)
}

/// The base a vault reopen opens with, when this agent's plan has anything to
/// add: the resume word plus the launch plan's args — the same clothes the
/// launch button dresses it in. Orca resolves `agentArgs` into the vault
/// resume plan too (renderer ai-vault-resume-command.ts:173); without this a
/// vault reopen lost the yolo default and asked before every tool — the tail
/// of the map's P0-2. Whether this agent's reopen carries the args, and which
/// stale selectors are scrubbed first (#12982, `resume_session`'s reason),
/// are both the agent's row.
pub(super) fn vault_resume_base(
    repository: &settings::SettingsRepository,
    slug: &str,
) -> Result<Option<String>, String> {
    let Some(caps) = zerocode_core::agent_capabilities(slug) else {
        return Ok(None);
    };
    if !caps.resume.vault_carries_launch_args {
        return Ok(None);
    }
    let launch_override = stored_launch_override(repository, slug)?;
    let plan = zerocode_core::launch_plan(slug, launch_override.as_ref());
    let args = caps.resume.launch_args_without_selectors(&plan.args);
    if args.is_empty() {
        return Ok(None);
    }
    let mut base = zerocode_core::vault::resume_base(slug).to_string();
    for word in &args {
        base.push(' ');
        base.push_str(&zerocode_core::vault::resume_word(word));
    }
    Ok(Some(base))
}

/// Both sides of one image, base64, either possibly absent.
#[derive(Serialize)]
pub(super) struct ImageDiff {
    pub(super) mime: &'static str,
    /// What was committed, absent for a file that is new.
    pub(super) committed: Option<String>,
    /// What is on disk, absent for a file that was deleted.
    pub(super) working: Option<String>,
}

/// Where this platform records its general-purpose shell, and what to assume
/// when it does not. `COMSPEC` remains the Windows automation fallback; local
/// terminal panes use the Orca-compatible PowerShell resolver below.
#[cfg(windows)]
pub(super) const SHELL_ENV: (&str, &str) = ("COMSPEC", "cmd.exe");
#[cfg(not(windows))]
pub(super) const SHELL_ENV: (&str, &str) = ("SHELL", "/bin/zsh");

/// Windows has no login-shell concept, and `-l` is not a flag `cmd` accepts —
/// passing it there opens a terminal that greets the user with a complaint.
#[cfg(windows)]
pub(super) const SHELL_ARGS: &[&str] = &[];
#[cfg(not(windows))]
pub(super) const SHELL_ARGS: &[&str] = &["-l"];

/// The shell this platform means by "a terminal", and how to start it
/// interactively.
///
/// The platform difference is the two constants above and nothing else: this
/// body compiles and is tested on every target, so a port cannot break it in
/// a way only the other operating system would discover.
pub(super) fn user_shell() -> (String, Vec<String>) {
    let (variable, fallback) = SHELL_ENV;
    let shell = std::env::var(variable).unwrap_or_else(|_| fallback.to_string());
    (
        shell,
        SHELL_ARGS.iter().map(|arg| (*arg).to_string()).collect(),
    )
}

pub(super) const POWERSHELL_7: &str = "pwsh.exe";
#[cfg(any(windows, test))]
pub(super) const WINDOWS_POWERSHELL: &str = "powershell.exe";
#[cfg(any(windows, test))]
pub(super) const WINDOWS_COMMAND_PROMPT: &str = "cmd.exe";

#[cfg(any(windows, test))]
pub(super) fn windows_env_value<'a>(
    environment: &'a BTreeMap<String, String>,
    names: &[&str],
) -> Option<&'a str> {
    environment.iter().find_map(|(key, value)| {
        names
            .iter()
            .any(|name| key.eq_ignore_ascii_case(name))
            .then_some(value.as_str())
    })
}

#[cfg(any(windows, test))]
pub(super) fn normalized_windows_path(raw: &str) -> String {
    raw.trim()
        .trim_matches('"')
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_string()
}

#[cfg(any(windows, test))]
pub(super) fn is_windows_git_bash_path(raw: &str) -> bool {
    let path = normalized_windows_path(raw).to_ascii_lowercase();
    [
        "/git/bin/bash.exe",
        "/git/usr/bin/bash.exe",
        "/portablegit/bin/bash.exe",
        "/portablegit/usr/bin/bash.exe",
    ]
    .iter()
    .any(|suffix| path.ends_with(suffix))
}

/// Candidate discovery mirrors Orca's Windows host probe without storing a
/// machine-specific executable path in Settings.
#[cfg(any(windows, test))]
pub(super) fn windows_git_bash_candidate_paths(
    environment: &BTreeMap<String, String>,
) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    let mut seen = BTreeSet::new();
    let mut push = |raw: String| {
        let path = normalized_windows_path(&raw);
        if !path.is_empty()
            && is_windows_git_bash_path(&path)
            && seen.insert(path.to_ascii_lowercase())
        {
            paths.push(PathBuf::from(path));
        }
    };

    for names in [
        &["ProgramFiles", "PROGRAMFILES"][..],
        &["ProgramW6432", "PROGRAMW6432"][..],
        &["ProgramFiles(x86)", "PROGRAMFILES(X86)"][..],
        &["LOCALAPPDATA", "LocalAppData"][..],
    ] {
        let Some(root) = windows_env_value(environment, names) else {
            continue;
        };
        let root = normalized_windows_path(root);
        for suffix in [
            "Git/bin/bash.exe",
            "Git/usr/bin/bash.exe",
            "Programs/Git/bin/bash.exe",
            "Programs/Git/usr/bin/bash.exe",
        ] {
            push(format!("{root}/{suffix}"));
        }
    }

    if let Some(path) = windows_env_value(environment, &["Path", "PATH"]) {
        for entry in path.split(';').map(normalized_windows_path) {
            if entry.is_empty() {
                continue;
            }
            push(format!("{entry}/bash.exe"));
            let lower = entry.to_ascii_lowercase();
            if let Some(root) = lower
                .strip_suffix("/cmd")
                .filter(|root| root.ends_with("/git") || root.ends_with("/portablegit"))
            {
                let original_root = &entry[..root.len()];
                push(format!("{original_root}/bin/bash.exe"));
                push(format!("{original_root}/usr/bin/bash.exe"));
            } else if lower.ends_with("/git") || lower.ends_with("/portablegit") {
                push(format!("{entry}/bin/bash.exe"));
                push(format!("{entry}/usr/bin/bash.exe"));
            }
        }
    }
    paths
}

pub(super) fn installed_windows_git_bash() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        let environment = std::env::vars().collect::<BTreeMap<_, _>>();
        return windows_git_bash_candidate_paths(&environment)
            .into_iter()
            .find(|path| is_runnable(path));
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// Resolve Orca's closed PowerShell preference into ordered spawn attempts.
///
/// The availability input is explicit so the platform policy has deterministic
/// tests on every host. The actual Windows road supplies it from the same
/// executable probe exposed to Settings.
#[cfg(any(windows, test))]
pub(super) fn windows_powershell_candidates(
    implementation: WindowsPowerShellImplementation,
    pwsh_available: bool,
) -> Vec<&'static str> {
    match implementation {
        WindowsPowerShellImplementation::Auto if pwsh_available => {
            vec![POWERSHELL_7, WINDOWS_POWERSHELL]
        }
        WindowsPowerShellImplementation::Auto
        | WindowsPowerShellImplementation::WindowsPowerShell => vec![WINDOWS_POWERSHELL],
        WindowsPowerShellImplementation::PowerShell7 => {
            vec![POWERSHELL_7, WINDOWS_POWERSHELL]
        }
    }
}

#[cfg(any(windows, test))]
pub(super) fn windows_terminal_shell_candidates(
    shell: WindowsTerminalShell,
    implementation: WindowsPowerShellImplementation,
    pwsh_available: bool,
    git_bash: Option<&Path>,
) -> Vec<String> {
    let powershell = || {
        windows_powershell_candidates(implementation, pwsh_available)
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>()
    };
    match shell {
        WindowsTerminalShell::PowerShell => powershell(),
        WindowsTerminalShell::CommandPrompt => std::iter::once(WINDOWS_COMMAND_PROMPT.to_string())
            .chain(powershell())
            .collect(),
        WindowsTerminalShell::GitBash => git_bash
            .map(|path| path.to_string_lossy().into_owned())
            .into_iter()
            .chain(powershell())
            .collect(),
    }
}

pub(super) fn terminal_user_shell_candidates(prefs: &TerminalPrefs) -> Vec<(String, Vec<String>)> {
    #[cfg(windows)]
    {
        let git_bash = installed_windows_git_bash();
        return windows_terminal_shell_candidates(
            prefs.windows_shell,
            prefs.windows_powershell_implementation,
            program_exists(POWERSHELL_7),
            git_bash.as_deref(),
        )
        .into_iter()
        .map(|program| (program.to_string(), Vec::new()))
        .collect();
    }
    #[cfg(not(windows))]
    {
        let _ = prefs;
        vec![user_shell()]
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ShellStartup {
    Configured,
    Plain,
    /// The tab IS this argv — a remote terminal's `ssh -t …`, spawned
    /// directly, never through a shell string.
    Remote(Vec<String>),
}

pub(super) fn terminal_command_for(startup: ShellStartup, configured: Vec<String>) -> Vec<String> {
    match startup {
        ShellStartup::Configured => configured,
        ShellStartup::Plain => Vec::new(),
        ShellStartup::Remote(argv) => argv,
    }
}

/// Open the floating terminal's shell, if it is not already running.
///
/// The user's own shell, in the project root, on top of the inherited
/// environment — so whatever they can run in their terminal (`claude`, `zo`,
/// `just verify`) runs identically here.
/// Start a shell in the checkout being looked at.
///
/// The checkout, not the project root: a shell opened while a worktree is
/// staged belongs in that worktree. One already running stays where it
/// started — a process cannot be moved.
/// Returns the pty AND the program it actually runs: the caller that wants
/// to know whether this terminal was born an agent session must hear about
/// the fallback, or a broken `claude` setting would be recorded as a
/// running Claude.
/// The `PATH` this pty will actually start with — the LAST entry wins,
/// exactly as the spawn applies its environment. The team's shim directory
/// must sit in front of THIS path, not the process's: the process path knows
/// nothing about the mirror shims already prepended for the pane, and a team
/// built over it would quietly take the mirror off every leader's `PATH`.
pub(super) fn pty_path_of(env: &[(String, String)]) -> String {
    pty_path_in(env)
        .map(str::to_string)
        .unwrap_or_else(|| std::env::var("PATH").unwrap_or_default())
}

/// The `PATH` this environment ALREADY carries, or nothing.
///
/// The same read without the fallback, and the difference is the whole reason
/// there are two: [`pty_path_of`] answers "what will this pane run with", which
/// is why it may answer with the process's. `hooks::pty_env` is asking a
/// narrower question — "did the caller decide a `PATH`" — and handing it the
/// process's for a caller that decided nothing would put launchd's `PATH` in
/// front of the shell's hydrated one, which is how a Finder-launched window
/// stops being able to find `claude` at all.
pub(super) fn pty_path_in(env: &[(String, String)]) -> Option<&str> {
    env.iter()
        .rev()
        .find(|(key, _)| key == "PATH")
        .map(|(_, value)| value.as_str())
}

pub(super) fn spawn_shell(
    state: &State<'_, AppState>,
    term: TermId,
    rows: u16,
    cols: u16,
    startup: ShellStartup,
    seat: Option<PathBuf>,
) -> Result<(PtyLane, String), String> {
    let root = state.active_root();
    // 앉을 자리와 소속은 다른 질문이다 (P0-15 "Open in Terminal"): seat는
    // 셸의 cwd만 바꾸고, 훅 좌표·계정 env가 말하는 워크트리 문맥은 활성
    // 루트 그대로다 — Orca의 폴더-우클릭 터미널도 그 워크트리의 탭이다.
    let seat = seat.unwrap_or_else(|| root.clone());
    // The hook coordinates go to EVERY shell, not only to launched agents. An
    // agent somebody starts by hand in a plain terminal is the same agent, and
    // it already has our hook installed in its own config — without the
    // coordinates its script would find no port and exit quietly, which reads
    // as "hooks do not work" for the most ordinary way to start a tool.
    let mut env = hooks::pty_env(&hooks::pane_key_of(term), None, &root, None);
    // And the selected ACCOUNT goes to every shell, by the same argument: a
    // `claude` somebody types into a plain terminal is the same claude the
    // launch button starts, and the account picker's promise is that this
    // window runs it as the chosen login. Without this line the picker held
    // for launched tabs and quietly did not for typed ones — which is how
    // "계정을 변경해도 그 아이디로 로그인이 안 된다" shipped: the shell had
    // no CLAUDE_CONFIG_DIR, and the CLI fell back to `~/.claude`.
    env.extend(shell_account_env(state.config_root())?);
    // What the person asked for, if they asked. Words, never a shell string —
    // this comes out of a settings field and is spawned directly.
    let chosen = terminal_command_for(
        startup,
        split_command(
            &load_settings_for_boot(state.settings())
                .document
                .terminal_command,
        ),
    );
    if let Some((program, rest)) = chosen.split_first() {
        // A configured terminal command that IS claude is a leader like any
        // launched one — Orca's own gate is `isDirectClaudeCommand` and
        // nothing else. Anything that is not claude gets no `TMUX` in its
        // environment and no shim on its `PATH`: a person running a real
        // tmux inside a plain shell must never find ours in front.
        let mut args = rest.to_vec();
        let teams_mode = load_settings_for_boot(state.settings())
            .document
            .agent_teams_mode;
        let mode_args = zerocode_core::agent_teams::teammate_mode_args(&chosen, teams_mode);
        if !mode_args.is_empty() {
            args.splice(0..0, mode_args);
        }
        let team_env = agent_teams::open_team(
            state.local_data_root(),
            term,
            teams_mode,
            &pty_path_of(&env),
            program,
        );
        // The team rides a COPY: the fallback shell below must not inherit a
        // `TMUX` pointing at a team whose leader never started.
        let mut launch_env = env.clone();
        launch_env.extend(team_env.iter().cloned());
        // A setting that will not start must not leave the window with no
        // terminal at all — the shell is the answer it had before. This is
        // the late failure only: `set_terminal_command` already refused a
        // program it could not find, so reaching here means it stopped
        // working after it was saved, which the log is the right place for.
        launch_env.extend(worktree_history_env(
            state.local_data_root(),
            &root,
            program,
        ));
        match PtyLane::spawn(program, &args, Some(&seat), &launch_env, rows, cols) {
            Ok(pty) => {
                if !team_env.is_empty() {
                    state.team_envs().insert(term, launch_env.clone());
                }
                // A configured shell is a shell: an agent typed into it runs
                // in front of it, and the shell back in front is that agent
                // gone. Any other program — claude itself, a wrapper, an ssh
                // — is not asked (`Host::shell_in_front`).
                if !matches!(
                    zerocode_core::shell_history::shell_of(program),
                    zerocode_core::shell_history::Shell::Other
                ) {
                    state.shell_panes().insert(term);
                }
                crumbs::record("pane", format_args!("spawn term={term}"));
                note_window_event(
                    state.local_data_root(),
                    &format!("term {term} spawned {program}"),
                );
                return Ok((pty, program.clone()));
            }
            Err(error) => {
                agent_teams::forget_term(term);
                eprintln!("{program}: {error} — 대신 셸을 엽니다");
                note_window_event(
                    state.local_data_root(),
                    &format!("term {term} refused {program}: {error} — falling back to a shell"),
                );
            }
        }
    }
    let prefs = load_settings_for_boot(state.settings())
        .document
        .terminal_prefs;
    let mut last_error = None;
    for (shell, args) in terminal_user_shell_candidates(&prefs) {
        let mut env = env.clone();
        env.extend(worktree_history_env(state.local_data_root(), &root, &shell));
        match PtyLane::spawn(&shell, &args, Some(&seat), &env, rows, cols) {
            Ok(pty) => {
                state.shell_panes().insert(term);
                crumbs::record("pane", format_args!("spawn term={term}"));
                note_window_event(
                    state.local_data_root(),
                    &format!("term {term} spawned {shell}"),
                );
                return Ok((pty, shell));
            }
            Err(error) => {
                eprintln!("{shell}: {error} — 다음 셸 후보를 시도합니다");
                note_window_event(
                    state.local_data_root(),
                    &format!("term {term} refused {shell}: {error}"),
                );
                last_error = Some(error.to_string());
            }
        }
    }
    note_window_event(
        state.local_data_root(),
        &format!("term {term} has NO shell"),
    );
    Err(last_error.unwrap_or_else(|| "시작할 터미널 셸이 없습니다".to_string()))
}

/// The agent a program name means, when it means one.
///
/// The name as spawned, matched against every name the registry knows an
/// agent under — `claude` and `/usr/local/bin/claude` are the same claim.
pub(super) fn agent_for_program(program: &str) -> Option<&'static str> {
    let name = std::path::Path::new(program).file_name()?.to_str()?;
    zerocode_core::AGENT_SPECS
        .iter()
        .find(|spec| spec.detect_names().any(|known| known == name))
        .map(|spec| spec.id)
}

/// One helper's own conversation, and where to read on from.
#[derive(Serialize)]
pub(super) struct SubagentLog {
    pub(super) skipped: bool,
    pub(super) turns: Vec<zerocode_core::transcript::TranscriptTurn>,
    /// The model the transcript says wrote it, when it says. The window learns
    /// a pane's model from a hook that carries one and most carry none, so a
    /// pane adopted after the fact — or one whose window restarted — had no
    /// model to put beside its mark. Its own file knows.
    pub(super) model: Option<String>,
    /// The byte to ask from next time. Handed back rather than remembered
    /// here: several windows may read one helper, and a cursor kept in this
    /// process would be a fourth thing to keep in step with three of them.
    pub(super) next: u64,
    /// Whether the vendor keeps a transcript for this helper at all. `false`
    /// is an answer, not a failure — an in-process helper of an agent that
    /// writes none has no page, and the row stays what it always was.
    pub(super) found: bool,
    /// Whether the file holds bytes past `next`: a page that opened late on a
    /// long transcript reads on at once instead of one chunk a poll, so the
    /// history a person asked for does not stream in as if it were live.
    pub(super) more: bool,
    /// Whether this read began at the file's TAIL (asked with no cursor) and
    /// left turns above it unread: a pane hours into a session has tens of
    /// megabytes behind it, and a view opened on it shows the end and says
    /// the rest is folded rather than painting the whole past as arriving.
    pub(super) folded: bool,
    /// How full the context was when this chunk's last answer was written —
    /// the composer's meter for a pane that has neither a wire nor a channel
    /// to ask. The file knows no window, so only the count stands.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) usage: Option<zerocode_core::transcript::TranscriptUsage>,
}

/// How much of a helper's transcript one ask may carry.
///
/// A page polls while somebody is looking at it, so the bound is about the
/// FIRST ask: a helper that has been running for an hour must not put a
/// megabyte through the wire in one message. Later asks are the tail since
/// the last one, which is a few lines.
pub(super) const SUBAGENT_LOG_CHUNK: u64 = 256 * 1024;

/// The directory a session's helpers write their transcripts in.
///
/// Derived from the transcript path the AGENT ITSELF reported (`pane_sessions`)
/// — never from anything the window says. That is the whole guard: a window
/// naming a path could read any file on the machine through this door, and
/// here it names only a pane it can already see and a helper id.
pub(super) fn subagent_transcript_root(transcript: &str) -> PathBuf {
    let path = Path::new(transcript);
    path.parent()
        .unwrap_or(Path::new("."))
        .join(path.file_stem().unwrap_or_default())
        .join("subagents")
}

/// Is this a helper id and not a path?
///
/// The id is spliced into a file name, so anything but the vendor's own
/// alphabet is refused rather than escaped — a `..` here would be a traversal
/// out of the session's own directory.
pub(super) fn is_subagent_id(said: &str) -> bool {
    !said.is_empty()
        && said.len() <= 64
        && said
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

/// Find `agent-<id>.jsonl` under the session's helper directory.
///
/// A bounded walk: the vendor nests these one directory per run kind
/// (`subagents/workflows/<run>/`), so three levels is the shape of the tree
/// rather than a guess, and a deeper one is not this window's business.
pub(super) fn find_subagent_transcript(root: &Path, id: &str, depth: u8) -> Option<PathBuf> {
    let wanted = format!("agent-{id}.jsonl");
    let mut folders = Vec::new();
    for entry in std::fs::read_dir(root).ok()?.flatten() {
        let path = entry.path();
        if path.file_name().and_then(|name| name.to_str()) == Some(wanted.as_str()) {
            return Some(path);
        }
        if depth > 0 && path.is_dir() {
            folders.push(path);
        }
    }
    folders
        .into_iter()
        .find_map(|folder| find_subagent_transcript(&folder, id, depth - 1))
}

/// Everything this process remembers ABOUT a shell, forgotten in one door.
///
/// Not the shell itself and not its pending prompt. Every caller drops those
/// two itself, beside its own reason for dropping them — the tab close because
/// a prompt addressed to a shell that is gone has nowhere to go, the reaper
/// because the child has already been buried. Kept out of here for a second
/// reason as well: the reaper decides who died while holding both of those
/// locks, and this door has to stay callable from inside that block. It is one
/// edit away from being moved there, and the edit must not be a deadlock.
///
/// It exists because there are THREE ways a shell ends and only one of them
/// used to clean up. `close_term` — the tab close — forgot everything below.
/// The pump's reaper, which is the road a shell that ended on its own takes
/// (`exit`, a crash, an agent finishing its session), forgot all of it: it
/// dropped the pty and the delivery and left the token, the session, the last
/// reported state, the lineage, the helpers and the team standing. And
/// `TeamWindow::close`, the tmux shim's road, forgot half. So the map that
/// tells the board which agents are running grew an entry per shell ever
/// opened, and a window left running for a day of agent work was answering
/// `pane_agents` out of a table mostly full of the dead.
///
/// One door, so the next map added here is added once. That is the whole
/// argument: the leak was never that somebody wrote the wrong line, it was
/// that the right line had to be written in three places and was written in
/// one.
pub(super) enum TermLedgerSettlement {
    Recorded(Option<String>),
    /// A PTY the host opened but refused before the worker-start effect was
    /// accepted. The outer effect rollback owns the ledger row; settling it
    /// here as an exited worker would race that rollback.
    Unrecorded,
}

pub(super) fn forget_term_state(state: &AppState, term: TermId, settlement: TermLedgerSettlement) {
    // A worker still inside its launch-readiness transaction has not started
    // an attempt yet. The reaper may win the race to this door, so preserve
    // its screen for the post-fence waiter and suppress the ordinary terminal
    // settlement; that waiter will roll the reservation back as not-started.
    let settlement = {
        let mut readiness = state.worker_readiness();
        if let Some(slot) = readiness.get_mut(&term) {
            if let TermLedgerSettlement::Recorded(screen) = &settlement
                && slot.exited_screen.is_none()
            {
                slot.exited_screen = screen.clone();
            }
            TermLedgerSettlement::Unrecorded
        } else {
            settlement
        }
    };
    // A Codex refresh rotates the refresh token in the mirror. Launch sync is
    // still the primary door, but a pane ending is the next measured chance to
    // return that token before another process replants the consumed system
    // copy. Read the agent before its per-terminal fact is removed; failures
    // stay best effort just as they do during launch.
    let agent = { state.agent_terms().get(&term).copied() };
    if let Some(agent) = agent {
        let _ = hooks::sync_agent_auth(state.local_data_root(), agent);
    }
    // A closed terminal is not a native or PTY send target any more. Removing
    // the Codex route drops its generation lease immediately; process cleanup
    // continues off this teardown road.
    codex_queue::forget_term(term);
    restart_nudge_runtime::forgotten(state, term);
    // A pane that is gone collects nothing. Beside the Codex route's own
    // teardown because it is the same fact about the same terminal.
    crate::orchestration_pointer_mailbox::forget_term(term);
    crate::human_input::forget_term(term);
    zo_integration_runtime::prune_owner(state, ZoChannelOwner::Term(term));
    state.zo_worker_deliveries().remove(&term);
    state.agent_terms().remove(&term);
    state.shell_panes().remove(&term);
    // Its token goes with it: an event still in flight for a pane that closed
    // has no tab to paint, and a token left behind would let the NEXT occupant
    // of this id inherit the last one's identity.
    state.launch_tokens().remove(&term);
    hooks::revoke_pane_capabilities(term);
    // A pid-discovered channel is released by the process that claimed it.
    // A duplicate session belongs to the lane/pane that won the session-id
    // claim, so this term must not tear that subscriber down on exit.
    let zo_adoption = state.zo_adoptions().remove(&term);
    if let Some(adoption) = zo_adoption.as_ref() {
        release_zo_subscription(state, term, adoption);
    }
    // The session goes too. Ids are never reused, so a stale entry could not be
    // mistaken for a live pane's — but a list of conversations that grows for
    // every terminal ever opened is a list nobody can read.
    let pane_session = state.pane_sessions().remove(&term);
    // A pane-owned Zo channel dies with its terminal, not only with a lane.
    // Removing the subscriber mark first also tells a stream already draining
    // buffered frames to stop before it can repaint a tab that no longer
    // exists. Other agents have no entry in `channels`, so this is a no-op for
    // their provider-session records.
    if agent == Some(AgentKind::Zo.slug())
        && zo_adoption.is_none()
        && let Some(session) = pane_session
    {
        detach_zo_channel_if_owned(state, &session.id, ZoChannelOwner::Term(term));
    }
    // And its last reported state, for the same reason: the board draws a card
    // per pane this window believes holds an agent, and a pane that closed does
    // not.
    state.pane_states().remove(&term);
    // 그 판이 원장에서 앉아 있던 자리도 잊는다. 원장의 행 자체는 남는다 —
    // 재시작을 살아넘는 것이 그 행의 일이므로 — 잊는 것은 **번호에서 자리로
    // 가는 지도**뿐이다. 남겨 두면 다음에 이 번호를 물려받는 판이 지난
    // 세입자의 행을 자기 것으로 고치고, 그것이 꼬리 맞춤 시절의 버그를
    // 번호 하나 크기로 되살린다.
    state.last_status_seats().remove(&term);
    // A pane that closed holds nothing awake.
    state.awake().forget_pane(term);
    // And the run of foreground looks behind that state. A pane that closed is
    // not a pane whose agent left; leaving the run standing would hand the next
    // occupant of this id a watch that is already most of the way to a verdict.
    state.foreground_watch().remove(&term);
    // And the viewer pages its runs opened — the window marks its own tabs
    // orphaned off `term:exited`; this is only the map not leaking.
    state.workers().retain(|_, seat| seat.0 != term);
    // And which helper this pane WAS (zo's pane lane, t-3024). The pane was
    // the helper, so the pane ending is the helper finishing — and nobody
    // else says so in time: zo's stop pair closed the spawn at the summons,
    // the next roll call rides the parent's next spawn or its turn's end,
    // and a TUI pane relays no `subagents` frame (measured 2026-09-07: the
    // pane left 39 ms after the parent's StopAgent and the card drew the
    // helper as running for nineteen more minutes, t-3098). Retired here,
    // at the one door every ending passes, through the finishing road a
    // vendor's own stop takes; BEFORE the lineage goes, because the parent
    // is read off it. This door holds only the state, so the publish is
    // written down as owed and the pump pays it (`publish_owed_rosters`).
    // One guard at a time: the roster's lock is never held with another.
    let helper = state.pane_helpers().remove(&term);
    if let Some(helper) = helper {
        let parent = state.pane_parents().get(&term).copied();
        if let Some(parent) = parent {
            let retired = state
                .subagents()
                .get_mut(&parent)
                .is_some_and(|rows| hooks::close_helper_row(rows, &helper, false));
            if retired {
                state.unpublished_rosters().insert(parent);
            }
        }
    }
    // And who started it. Only this pane's own entry — the panes THIS one
    // started keep theirs and become roots on the board, which is what the
    // lineage rules do with a parent that is gone. Erasing them here would be
    // the same answer given twice, in two places, one of them untested.
    state.pane_parents().remove(&term);
    // And the helpers that were running inside it. They live in the agent this
    // pane held, so the pane closing is their end too — and no Stop event will
    // ever arrive for them to clear the rows, because the process that would
    // have sent it is the one that just died. The all-clear those rows may
    // have been holding back dies with them.
    state.subagents().remove(&term);
    state.pending_done().remove(&term);
    // And everything those helpers and this pane were recorded DOING. Its own
    // door because the keys are strings the board names its cards by, and the
    // helpers' keys are ones nothing else could ever name again — a `remove`
    // by term could not reach them.
    state.forget_activities(term);
    // And its team environment.
    state.team_envs().remove(&term);
    state.delivery_waiters().remove(&term);
    state.worker_prompt_submits().remove(&term);
    state.completed_worker_cleanups().remove(&term);
    state.isolated_worker_terms().remove(&term);
    // And which answer-walk this pane was on. The generation exists to let a
    // new answer overtake an old one's timers; a pane with no agent in it has
    // no walk to overtake, and this was the one map that no road removed from
    // at all — not even the tab close.
    state.ask_sends().remove(&term);
    // 그 판의 제스처 기억과 셋틀 세대도 함께. 반쯤 눌린 이중 Escape의 첫
    // 다리를 남겨 두면 이 번호를 물려받는 다음 판의 첫 Escape가 **남의 짝**이
    // 되어 아무도 두 번 누르지 않은 인터럽트를 만든다.
    state.interrupt_inference().remove(&term);
    state.inference_sends().remove(&term);
    // And its team. A LEADER closing ends the team outright; a teammate
    // closing takes one row out of it, so the leader's `list-panes` stops
    // naming a pane that is no longer there — the distinction is
    // `forget_term`'s, and it is a distinction because a team thrown away on
    // any pane's death would leave a leader unable to learn what happened.
    //
    // The ledger settles FIRST, because the seat is how it finds the worker and
    // `forget_term` is what takes the seat away. Until this call existed a
    // teammate's pane could exit and its dispatch stayed open forever, holding
    // a standing order's ceiling against a worker that no longer had a screen.
    if let TermLedgerSettlement::Recorded(screen) = settlement {
        orchestration::terminal_gone_with_archive(term, screen, now_epoch_ms());
    }
    agent_teams::forget_term(term);
    // And the black box's note that this term's frames were stopping at the
    // pump's gate. The note is cleared when a term becomes WATCHED (:5643) —
    // a term that was never watched again simply closed, and its id would sit
    // in that set for the life of the process. One entry is nothing; a set
    // that only ever grows, across a session that runs for days, is the shape
    // of a leak whatever its size.
    unwatched_noted()
        .lock()
        .map(|mut noted| noted.remove(&term))
        .ok();
    // And the preview tier's two: when this term's tail last crossed, and its
    // place in every window's preview set. Both are keyed by an id nothing
    // will name again, which is what makes a leftover entry a leak rather than
    // stale data.
    previewed_last_sent()
        .lock()
        .map(|mut sent| sent.remove(&term))
        .ok();
    for terms in state.previewed_terms().values_mut() {
        terms.remove(&term);
    }
}

/// Whether the terminal currently belongs to a foreground job rather than
/// the shell process ZeroCode launched.
///
/// `None` is deliberately preserved: platforms or transports without a
/// trustworthy process-group answer must not be reported as idle. The
/// renderer can then fall back to the hook state it already owns.
pub(super) fn running_process_from_foreground(shell_is_foreground: Option<bool>) -> Option<bool> {
    shell_is_foreground.map(|shell_is_foreground| !shell_is_foreground)
}

pub(super) fn managed_terminal_session(
    term: TermId,
    started_as: Option<&'static str>,
    shell_is_foreground: Option<bool>,
    programs: Vec<String>,
) -> ManagedTerminalSession {
    let agent = started_as.or_else(|| {
        programs
            .iter()
            .find_map(|program| zerocode_core::agent::agent_spec_by_process(program))
            .map(|spec| spec.id)
    });
    let program = programs.into_iter().find_map(|program| {
        Path::new(&program)
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.trim().is_empty())
            .map(str::to_owned)
    });
    ManagedTerminalSession {
        term,
        running: running_process_from_foreground(shell_is_foreground),
        agent,
        program,
    }
}

/// Remove one shell and every fact that names it.
///
/// Returning whether a shell was present lets the management commands remain
/// idempotent when a process exits between list and click.
pub(super) fn retire_terminal(state: &AppState, term: TermId) -> bool {
    let Some(pty) = state.terminals().remove(&term) else {
        return false;
    };
    let screen = lock_pty(&pty).terminal().grid().visible_text();
    // Stop a deliberately closed process before its final auth sync. The
    // self-exit and failed-launch roads have already dropped their PTYs before
    // they reach common cleanup.
    let _ = lock_pty(&pty).kill();
    drop(pty);
    state.deliveries().remove(&term);
    state.prompt_queue().remove(&term);
    forget_term_state(state, term, TermLedgerSettlement::Recorded(Some(screen)));
    true
}

pub(super) fn announce_retired_terminal(app: &AppHandle, term: TermId) {
    let _ = app.emit("term:exited", TermGone { term });
}

/// What to watch for before typing at this agent.
///
/// The registry's fact, translated into the pty layer's vocabulary — a composer
/// glyph for three agents (each row naming its own), a shown cursor for two,
/// silence for the other thirty. An unnamed agent waits on silence, which is the honest answer
/// for a program nothing has identified and also what most named ones want.
///
/// One function rather than a `match` at each call site: a person's prompt and
/// a scheduled one must wait for the same thing, and two copies of this is how
/// they would come to differ.
pub(super) fn ready_signal_for(agent: Option<&str>) -> ReadySignal {
    agent
        .and_then(zerocode_core::agent_capabilities)
        .map_or(ReadySignal::Quiet, |caps| ready_signal_of(&caps))
}

/// The pump's launch signal for one row's startup mark — the one mapping
/// from the table's four marks to the pty's four signals. Takes the row's
/// answer rather than a name so a test can hand it an edited row. A glyph
/// travels from the row as it is: this door has no glyph of its own.
pub(super) fn ready_signal_of(caps: &zerocode_core::AgentCapabilities) -> ReadySignal {
    match caps.startup.mark {
        ReadyMark::Quiet => ReadySignal::Quiet,
        ReadyMark::CursorShown => ReadySignal::CursorShown,
        ReadyMark::ComposerPrompt(glyph) => ReadySignal::Prompt(glyph),
        ReadyMark::AltScreenPrompt(glyph) => ReadySignal::AltScreenPrompt(glyph),
    }
}

/// Whether a send to an already-running composer may clear with edit keys.
/// Unknown agents take the safe default: no control burst.
pub(super) fn composer_clear_for(agent: Option<&str>) -> bool {
    agent
        .and_then(zerocode_core::agent_capabilities)
        .is_some_and(|caps| caps.steer.clear == ComposerClear::Keys)
}

/// How long a launch must stay quiet after the bracketed-paste handshake.
/// Most TUIs use the shared window; a catalog override is a measured startup
/// gate that keeps painting after its composer first appears. Kept beside the
/// signal resolver so every launch road reads the same two readiness facts.
pub(super) fn ready_quiet_for(agent: Option<&str>) -> Duration {
    agent
        .and_then(zerocode_core::agent_capabilities)
        .and_then(|caps| caps.startup.quiet_ms)
        .map_or(zerocode_pty::ready::QUIET, |ms| {
            Duration::from_millis(u64::from(ms))
        })
}

/// How long a LAUNCH delivery waits before giving up — Orca's
/// `resolveDraftPasteReadyTimeoutMs`, minus the per-call override nothing
/// here passes: the agent's own figure when the catalog carries one, the
/// shared eight seconds otherwise. Codex needs it for a cold composer;
/// Antigravity needs room for its longer quiet window after authentication.
/// The send road keeps the shared deadline: a RUNNING terminal answers on
/// rest, not on mount time.
pub(super) fn ready_timeout_for(agent: Option<&str>) -> Duration {
    agent
        .and_then(zerocode_core::agent_capabilities)
        .and_then(|caps| caps.startup.timeout_ms)
        .map_or(zerocode_pty::ready::TIMEOUT, |ms| {
            Duration::from_millis(u64::from(ms))
        })
}

/// One prompt parked behind another — its ingredients, not a built delivery,
/// for [`AppState::prompt_queue`]'s clock reason.
pub(super) struct QueuedPrompt {
    pub(super) text: String,
    pub(super) submit: bool,
    pub(super) signal: ReadySignal,
    pub(super) clearing: bool,
    /// What the delivery will refuse to write over, carried to the queued
    /// copy so a send parked behind another keeps the guard of the door that
    /// made it. Decided at the write, like the active delivery's.
    pub(super) guard: zerocode_pty::ready::Guard,
    pub(super) completion: std::sync::mpsc::SyncSender<DeliveryOutcome>,
}

pub(super) fn with_terminal<T>(
    state: &State<'_, AppState>,
    term: TermId,
    action: impl FnOnce(&mut PtyHandle) -> Result<T, PtyTransportError>,
) -> Result<T, String> {
    let held = state
        .terminals()
        .handle(term)
        .ok_or("터미널이 떠 있지 않습니다")?;
    let done = {
        let mut pty = lock_pty(&held);
        action(&mut pty).map_err(|error| error.to_string())
    };
    // Everything that reaches a shell comes through here, so this is where
    // the pump is told to stop napping — AFTER the action, under whose lock a
    // write records that it waits for the child's answer. The round this wake
    // starts has to find that record: a round that did not would take any
    // other shell's output for the answer and end the chase before the write
    // had even landed (`ChaseRound`). Nothing is lost by waking second — the
    // write only queues for the writer thread, so the child cannot have
    // answered yet, and a pump that is mid-round sees the wake as it rests.
    state.cadence().wake();
    done
}

pub(super) fn write_terminal_text(
    state: &State<'_, AppState>,
    term: TermId,
    text: &str,
) -> Result<(), String> {
    with_terminal(state, term, |pty| {
        pty.terminal_mut().grid_mut().view_to_bottom();
        pty.write_input(text.as_bytes())
    })
}

/// How many matches a search will report before it stops counting.
///
/// A person steps through hits; they do not step through fifty thousand. The
/// bound is the same idea as the file finder's `FIND_MAX` and exists for the
/// same reason — searching `e` across a full 50,000-line scrollback is work
/// nobody asked for, and the answer past the first few hundred is not one
/// anybody reads.
pub(super) const TERM_SEARCH_MAX: usize = 2_000;

/// Build the supervisor the window will run lanes with.
///
/// Token order: the environment when the user exported one (so the window
/// joins the server their terminals already share), else the project's
/// **persisted** token. Persistence is what makes closing and reopening the
/// window land on the same server — the serve outlives us by design, and it
/// only ever accepts the secret it was started with. If persistence fails the
/// lane feature stays unavailable while the editor and settings remain usable;
/// silently minting a replacement would make the detached server reject us.
pub(super) fn discover_supervisor(
    root: &std::path::Path,
    state_dir: &Path,
) -> Result<Option<LaneSupervisor>, String> {
    let Some(zo) = ZoBinary::discover() else {
        return Ok(None);
    };
    let bind = default_bind_for(root);
    let token = match std::env::var(TOKEN_ENV) {
        Ok(existing) if !existing.is_empty() => existing,
        _ => stored_supervisor_token(state_dir, root)?,
    };
    LaneSupervisor::new(zo, bind, Some(token))
        .map(Some)
        .map_err(|error| error.to_string())
}

pub(super) fn stored_supervisor_token(state_dir: &Path, root: &Path) -> Result<String, String> {
    project_token(state_dir, root).map_err(|error| {
        format!(
            "the persisted session token at {} is unavailable: {error}",
            state_dir.display()
        )
    })
}
