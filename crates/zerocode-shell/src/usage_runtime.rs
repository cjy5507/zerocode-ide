use super::*;

pub(super) fn claude_usage_file(local_data_root: &Path) -> PathBuf {
    local_data_root.join(artifact_file::CLAUDE_USAGE)
}

/// The last snapshot, loaded once from disk so a fresh window shows the plan
/// it knew before while the first scan of the day runs.
pub(super) fn claude_usage_cache(
    local_data_root: &Path,
) -> &'static Mutex<Option<usage::ProviderUsage>> {
    static CACHE: std::sync::OnceLock<Mutex<Option<usage::ProviderUsage>>> =
        std::sync::OnceLock::new();
    CACHE.get_or_init(|| {
        Mutex::new(
            std::fs::read_to_string(claude_usage_file(local_data_root))
                .ok()
                .and_then(|text| serde_json::from_str(&text).ok()),
        )
    })
}

/// Drop the usage snapshot, cache and file both.
///
/// Called when the thing the snapshot was ABOUT has changed — a repaired
/// login, say. Without this the row it marked stays marked until the next
/// scan lands, which is a stale verdict wearing a fresh answer's clothes.
pub(super) fn forget_claude_usage(local_data_root: &Path) {
    *claude_usage_cache(local_data_root)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    let _ = std::fs::remove_file(claude_usage_file(local_data_root));
}

pub(super) static CLAUDE_USAGE_SCANNING: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

pub(super) fn epoch_ms_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_millis() as i64)
}

/// The next moment the local clock reads `hour:minute`, as epoch ms.
///
/// Built on the same `localtime_r` road the automations walk — one local
/// clock in this program, not two.
pub(super) fn next_local_clock_epoch_ms(hour: u8, minute: u8) -> i64 {
    let wall = local_minute_now().wall();
    let today = i64::from(wall.hour) * 60 + i64::from(wall.minute);
    let target = i64::from(hour) * 60 + i64::from(minute);
    let mut ahead = (target - today).rem_euclid(1440);
    if ahead == 0 {
        ahead = 1440;
    }
    let now_ms = epoch_ms_now();
    now_ms - now_ms.rem_euclid(60_000) + ahead * 60_000
}

pub(super) fn usage_window_from(
    parsed: Option<(f32, Option<String>)>,
    window_minutes: u32,
) -> Option<usage::UsageWindow> {
    let (pct, words) = parsed?;
    let resets_at = words.as_deref().and_then(|words| {
        match usage::read_reset_mark(words) {
            usage::ResetMark::After(wait) => Some(epoch_ms_now() + wait.as_millis() as i64),
            usage::ResetMark::AtClock { hour, minute } => {
                Some(next_local_clock_epoch_ms(hour, minute))
            }
            // A dated phrase ("oct 7 at 3am") stays words: a wrong month
            // guess is worse than no countdown.
            usage::ResetMark::Unresolved => None,
        }
    });
    Some(usage::UsageWindow {
        used_percent: pct.round().clamp(0.0, 100.0) as u8,
        window_minutes,
        resets_at,
        reset_description: words,
    })
}

/// An OAuth window in the panel's own dress. No reset words: the API hands a
/// timestamp, and the window's face renders a countdown from it directly.
pub(super) fn oauth_usage_window(window: usage_oauth::OauthWindow) -> usage::UsageWindow {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    usage::UsageWindow {
        used_percent: window.used_percent.round().clamp(0.0, 100.0) as u8,
        window_minutes: window.window_minutes,
        resets_at: window.resets_at,
        reset_description: None,
    }
}

/// The `PATH` a hidden usage probe hands its CLI: the list a launch resolves
/// the same name against ([`shell_path::launch_path`]), so a probe and a
/// launch cannot disagree about whether an agent is on this machine.
///
/// A window opened from the Dock or Finder inherits launchd's
/// `/usr/bin:/bin:/usr/sbin:/sbin`, where no agent installer writes. Launches
/// never noticed — `hooks::pty_env` hands every pane the shell's hydrated
/// `PATH` — but both scans spawned their CLI with `TERM` and the account and
/// nothing else, so the name was looked up on launchd's list. On 2026-09-17
/// every OAuth read that fell through to the terminal left the status bar,
/// the summons quota gate and the quota wall's provider witness without a
/// Claude figure (`No viable candidates found in PATH
/// "/usr/bin:/bin:/usr/sbin:/sbin"`), while workers in the same window
/// started `claude` without trouble.
///
/// It waits for the shell's answer, which a launch must not: a probe runs on
/// its own thread and already waits up to [`usage::SCAN_TIMEOUT`] on a
/// screen, and the first scan after boot can otherwise read the list before
/// hydration lands. And the child is handed the list, not only looked up on
/// it — an npm install is a `#!/usr/bin/env node` script, and `env` searches
/// the child's own `PATH`.
pub(super) fn probe_path_env() -> Option<(String, String)> {
    let _ = shell_path::hydrate(false);
    let path = shell_path::launch_path()?.into_string().ok()?;
    Some(("PATH".to_string(), path))
}

/// Run the hidden terminal and read the `/usage` screen. Blocking — always
/// called from its own thread.
pub(super) fn scan_claude_usage_now(config_root: &Path) -> usage::ProviderUsage {
    let whose = active_claude_account_id(config_root);
    let failed = |status: &str, error: String| usage::ProviderUsage {
        provider: "claude".to_string(),
        session: None,
        weekly: None,
        fable_weekly: None,
        monthly: None,
        buckets: None,
        updated_at: epoch_ms_now(),
        error: Some(error),
        status: status.to_string(),
        failure_kind: None,
        retry_at_ms: None,
        plan_type: None,
        reset_credits: None,
        account: whose.clone(),
    };
    // The OAuth road FIRST — one round trip against the endpoint the CLI
    // itself reads, exactly Orca's order (claude-fetcher.ts:46; the hidden
    // terminal below is its fallback, not its peer). This is what ends the
    // twenty-five-second waits the map filed under P0-11. Read as the
    // SELECTED account for the terminal road's own reason: the credentials
    // file lives in the account's config directory.
    let config_dir = accounts::reading_env_for(config_root, "claude")
        .into_iter()
        .find(|(key, _)| key == zerocode_core::account::CONFIG_DIR_VAR)
        .map(|(_, value)| PathBuf::from(value));
    if let Err(error) = accounts::prepare_selected_store(config_root) {
        return failed("error", error);
    }
    let asked = usage_oauth::claude(config_dir.as_deref(), epoch_ms_now());
    // An auth or limit answer from the API IS the user-visible answer. Walking
    // the terminal road after it spawns Claude Code, waits up to twenty-five
    // seconds, and is told the same thing (`claude-oauth-usage-error.ts:19-21`).
    if let Err(failure) = &asked
        && failure.skip_cli_fallback
    {
        return usage::ProviderUsage {
            provider: "claude".to_string(),
            session: None,
            weekly: None,
            fable_weekly: None,
            monthly: None,
            buckets: None,
            updated_at: epoch_ms_now(),
            error: Some(failure.message.clone()),
            status: "error".to_string(),
            failure_kind: Some(failure.recovery.kind),
            retry_at_ms: failure.retry_at_ms,
            plan_type: None,
            reset_credits: None,
            account: whose,
        };
    }
    if let Ok(read) = asked {
        return usage::ProviderUsage {
            provider: "claude".to_string(),
            session: read.session.map(oauth_usage_window),
            weekly: read.weekly.map(oauth_usage_window),
            fable_weekly: read.fable_weekly.map(oauth_usage_window),
            monthly: None,
            buckets: None,
            updated_at: epoch_ms_now(),
            error: None,
            status: "ok".to_string(),
            failure_kind: None,
            retry_at_ms: None,
            plan_type: None,
            reset_credits: None,
            account: whose,
        };
    }
    // Spawned in the home directory, deliberately not the checkout: the scan
    // must not trip a project trust prompt it then has to answer.
    let home = std::env::var_os("HOME").map(PathBuf::from);
    // And as the SELECTED account. This scan is the number the status bar
    // shows, so a scan that ignores the picker is a picker that visibly does
    // nothing — which is exactly how it was reported ("계정 체인지를 해도
    // 체인지 안 되는 거 같아"): terminals had followed the switch since the
    // last fix, and the one figure on screen had not. Orca reads usage per
    // account for the same reason (`fetchManagedUsagePanelSupplement` runs its
    // pty under `envPatch: { CLAUDE_CONFIG_DIR }` with `stripAuthEnv`,
    // out/main/index.js:209490-209516).
    let mut env = vec![("TERM".to_string(), "xterm-256color".to_string())];
    env.extend(accounts::reading_env_for(config_root, "claude"));
    env.extend(probe_path_env());
    let mut pty = match PtyLane::spawn(
        "claude",
        &[],
        home.as_deref(),
        &env,
        PROBE_PTY_ROWS,
        PROBE_PTY_COLS,
    ) {
        Ok(pty) => pty,
        Err(error) => {
            return failed(
                "unavailable",
                format!("claude를 시작하지 못했습니다: {error}"),
            );
        }
    };
    let started = Instant::now();
    let mut asked = false;
    let mut nudged_at = Instant::now();
    let mut settle_at: Option<Instant> = None;
    let mut saw_slow_tabs = false;
    let screen = loop {
        std::thread::sleep(usage::SCAN_PUMP);
        let pumped = pty.pump();
        let text = pty.terminal().grid().visible_text();
        if usage::wants_trust_answer(&text) {
            let _ = pty.write_input(b"y\r");
        }
        if !asked && started.elapsed() >= usage::STARTUP_DELAY {
            let _ = pty.write_input(b"/usage\r");
            asked = true;
            nudged_at = Instant::now();
        }
        if asked {
            // A section heading settles fast; the 2.1 tabs screen renders
            // slowly and gets the longer settle. Whichever deadline is
            // earlier wins, exactly as Orca's two timers race.
            if usage::shows_a_usage_section(&text) {
                let deadline = Instant::now() + usage::SETTLE_AFTER_STOP;
                settle_at = Some(settle_at.map_or(deadline, |held| held.min(deadline)));
            } else if !saw_slow_tabs && usage::shows_slow_usage_tabs(&text) {
                saw_slow_tabs = true;
                settle_at = Some(Instant::now() + usage::SETTLE_AFTER_TABS);
            }
            if settle_at.is_none() && nudged_at.elapsed() >= usage::NUDGE_EVERY {
                let _ = pty.write_input(b"\r");
                nudged_at = Instant::now();
            }
        }
        if let Some(at) = settle_at
            && Instant::now() >= at
        {
            break text;
        }
        if pumped.ended || started.elapsed() >= usage::SCAN_TIMEOUT {
            break text;
        }
    };
    let _ = pty.kill();
    let parsed = usage::parse_usage_screen(&screen);
    let session = usage_window_from(parsed.session, usage::SESSION_WINDOW_MINUTES);
    let weekly = usage_window_from(parsed.weekly, usage::WEEKLY_WINDOW_MINUTES);
    let fable_weekly = usage_window_from(parsed.fable_weekly, usage::WEEKLY_WINDOW_MINUTES);
    if session.is_none() && weekly.is_none() && fable_weekly.is_none() {
        let low = screen.to_lowercase();
        let error = if low.contains("rate limited") {
            "지금은 사용량 조회가 제한되어 있습니다".to_string()
        } else if low.contains("failed to load usage data") {
            "지금은 사용량을 불러올 수 없습니다".to_string()
        } else if saw_slow_tabs {
            "이 Claude CLI에서는 플랜 사용량을 읽을 수 없습니다".to_string()
        } else if usage::shows_signed_out(&screen) {
            // Not a failed scan — a fact about the chosen account. The screen
            // the person would see in a terminal is the screen we just read,
            // so say what it says rather than blaming the renderer. Carried
            // as its OWN status because the account list reads it: a row that
            // can name somebody may still have a token that has died, and the
            // directory alone cannot tell the two apart (1-g15).
            // The road out is the one this window already built. 1-g15 exists
            // because 지우기 + 계정 추가 throws away the directory, its
            // settings and its place in the list for what is only a dead
            // credential — and the row this very status marks is the row that
            // carries 「다시 로그인」 (`account_relogin`, ui/shell.js). Naming
            // the discarding road here sent people down the one that slice was
            // written to close.
            return failed(
                usage::SIGNED_OUT_STATUS,
                "이 계정은 로그인되어 있지 않습니다 — 설정에서 다시 로그인하세요".to_string(),
            );
        } else {
            "/usage 화면이 렌더되지 않았습니다".to_string()
        };
        return failed("error", error);
    }
    usage::ProviderUsage {
        provider: "claude".to_string(),
        session,
        weekly,
        fable_weekly,
        monthly: None,
        buckets: None,
        updated_at: epoch_ms_now(),
        error: None,
        status: "ok".to_string(),
        failure_kind: None,
        retry_at_ms: None,
        plan_type: None,
        reset_credits: None,
        account: whose,
    }
}

/// Read Codex's `/status` the way Orca does — a hidden terminal, the command
/// typed, the two limit lines parsed.
///
/// Deliberately the PTY road and not the other two. Orca has three
/// (`fetchCodexRateLimits`, out/main/index.js:210593): a JSON-RPC session
/// against the Codex app-server, a WSL backend, and this. The RPC road is a
/// protocol this window has not measured, and a client written from a guess
/// against somebody's account is worse than a screen read slowly. The screen is
/// the same one the person can open themselves, which also makes a wrong figure
/// something they can check.
pub(super) fn scan_codex_usage_now(config_root: &Path) -> usage::ProviderUsage {
    let whose = active_codex_account_id(config_root);
    let done = |status: &str, error: Option<String>, session, weekly| usage::ProviderUsage {
        provider: "codex".to_string(),
        session,
        weekly,
        fable_weekly: None,
        monthly: None,
        buckets: None,
        updated_at: epoch_ms_now(),
        error,
        status: status.to_string(),
        failure_kind: None,
        retry_at_ms: None,
        plan_type: None,
        reset_credits: None,
        account: whose.clone(),
    };
    // Signed in first, and answered as its own STATUS rather than as a failed
    // scan (Orca's `probeCodexAuthPresence`, :210596). "There is nothing to
    // read" and "this could not be read" look identical in a status bar and
    // mean opposite things to the person deciding whether to report a bug.
    let home = codex_accounts::active_home(config_root)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".codex")));
    if !home
        .as_deref()
        .is_some_and(|home| home.join(zerocode_core::codex_account::AUTH_FILE).exists())
    {
        return done(
            usage::SIGNED_OUT_STATUS,
            Some("Codex에 로그인되어 있지 않습니다".to_string()),
            None,
            None,
        );
    }
    // The backend road FIRST, with the login just proven on disk — Codex's
    // own endpoint, one round trip (codex-fetcher.ts:544; Orca keeps the
    // terminal as the fallback, and so does this). The update-prompt
    // incident below cannot happen on a road that spawns nothing.
    let asked = home
        .as_deref()
        .map(|home| usage_oauth::codex(home, epoch_ms_now()));
    // Same rule as the Claude road above: the endpoint already answered.
    if let Some(Err(failure)) = &asked
        && failure.skip_cli_fallback
    {
        return usage::ProviderUsage {
            failure_kind: Some(failure.recovery.kind),
            retry_at_ms: failure.retry_at_ms,
            ..done("error", Some(failure.message.clone()), None, None)
        };
    }
    if let Some(Ok(read)) = asked {
        // The two facts only this road can know. The terminal road below reads
        // a screen that prints percentages and nothing else, so a fallback
        // answer carries no plan and no credits — which is why they are set
        // HERE and not in `done`.
        return usage::ProviderUsage {
            plan_type: read.plan_type,
            reset_credits: read.reset_credits,
            ..done(
                "ok",
                None,
                read.session.map(oauth_usage_window),
                read.weekly.map(oauth_usage_window),
            )
        };
    }
    let mut env = vec![("TERM".to_string(), "xterm-256color".to_string())];
    env.extend(codex_accounts::launch_env(config_root));
    env.extend(probe_path_env());
    // In the home directory rather than the checkout, for the same reason the
    // Claude scan is: a scan must not trip a project trust prompt it then has
    // to answer.
    let cwd = std::env::var_os("HOME").map(PathBuf::from);
    let mut pty = match PtyLane::spawn(
        "codex",
        &[],
        cwd.as_deref(),
        &env,
        PROBE_PTY_ROWS,
        PROBE_PTY_COLS,
    ) {
        Ok(pty) => pty,
        Err(error) => {
            return done(
                "unavailable",
                Some(format!("codex를 시작하지 못했습니다: {error}")),
                None,
                None,
            );
        }
    };
    let started = Instant::now();
    let mut asked_at: Option<Instant> = None;
    let mut asks = 0u32;
    let mut settle_at: Option<Instant> = None;
    // A question this window did not ask, and must not answer. See below.
    let mut interrupted = false;
    let screen = loop {
        std::thread::sleep(usage::SCAN_PUMP);
        let pumped = pty.pump();
        let text = pty.terminal().grid().visible_text();
        // **This scan never presses Enter at a screen it did not put there.**
        //
        // Learned the hard way, on the machine this was built on: `codex` opens
        // with an "Update available — 1. Update now" prompt whose DEFAULT is the
        // update, and a probe that nudged with a bare `\r` ran
        // `npm install -g @openai/codex` on somebody's machine without being
        // asked. A hidden terminal that answers arbitrary questions is a hidden
        // terminal that can do anything the CLI's default action is — and the
        // reason to read a plan figure is never worth that.
        //
        // So the scan gives up instead. The panel says the CLI is asking
        // something, which is a sentence a person can act on, and the terminal
        // they open themselves is where they should be answering it.

        if usage::shows_a_question(&text) {
            interrupted = true;
            break text;
        }
        let ready = asked_at.is_none_or(|at| at.elapsed() >= usage::NUDGE_EVERY);
        if started.elapsed() >= usage::STARTUP_DELAY && settle_at.is_none() && ready && asks < 3 {
            // The command and the Enter apart, as Orca sends them (:210446): a
            // composer still mounting swallows a line that arrives with its
            // newline already attached. Re-sent as the PAIR rather than nudged
            // with a lone newline — on this machine the composer is not ready
            // for twelve seconds while MCP servers start, and the first ask is
            // simply lost.
            let _ = pty.write_input(b"/status");
            std::thread::sleep(usage::SCAN_PUMP);
            let _ = pty.write_input(b"\r");
            asked_at = Some(Instant::now());
            asks += 1;
        }
        let read = usage::parse_codex_status(&text);
        if read.session.is_some() || read.weekly.is_some() {
            let deadline = Instant::now() + usage::SETTLE_AFTER_STOP;
            settle_at = Some(settle_at.map_or(deadline, |held| held.min(deadline)));
        }
        if let Some(at) = settle_at
            && Instant::now() >= at
        {
            break text;
        }
        if pumped.ended || started.elapsed() >= usage::SCAN_TIMEOUT {
            break text;
        }
    };
    let _ = pty.kill();
    if interrupted {
        return done(
            "unavailable",
            Some("codex가 무언가 묻고 있습니다 — 터미널에서 직접 열어 답해주세요".to_string()),
            None,
            None,
        );
    }
    let parsed = usage::parse_codex_status(&screen);
    let session = usage_window_from(parsed.session, usage::SESSION_WINDOW_MINUTES);
    let weekly = usage_window_from(parsed.weekly, usage::WEEKLY_WINDOW_MINUTES);
    if session.is_none() && weekly.is_none() {
        return done(
            if usage::shows_codex_signed_out(&screen) {
                usage::SIGNED_OUT_STATUS
            } else {
                "error"
            },
            Some(if usage::shows_codex_signed_out(&screen) {
                "Codex에 로그인되어 있지 않습니다".to_string()
            } else {
                "/status 화면이 렌더되지 않았습니다".to_string()
            }),
            None,
            None,
        );
    }
    done("ok", None, session, weekly)
}

/// Which Codex account a scan would run as, or `None` for the machine's own.
pub(super) fn active_codex_account_id(config_root: &Path) -> Option<String> {
    let store = codex_accounts::read_store(config_root);
    zerocode_core::codex_account::active_account(&store.accounts, &store.selection)
        .filter(|account| codex_accounts::signed_in(Path::new(&account.home_dir)))
        .map(|account| account.id.clone())
}

/// Which account a scan would run as.
///
/// The launch directory is intentionally the same for every account now, so
/// identity comes from the selection and its readable account store rather
/// than trying to reverse-map that shared runtime path.
pub(super) fn active_claude_account_id(config_root: &Path) -> Option<String> {
    let store = accounts::read_store(config_root);
    zerocode_core::active_account(&store.accounts, &store.selection)
        .filter(|account| accounts::signed_in(Path::new(&account.config_dir)))
        .map(|account| account.id.clone())
}

/// The account environment a launch of `agent` gets.
///
/// The one door every named launch goes through, so an agent that grows an
/// account mechanism is wired into resume, launch, automation and the send
/// targets by adding it HERE rather than by finding five call sites. Each
/// module answers only for its own agent and returns nothing for the others,
/// which is why they can simply be stacked.
pub(super) fn account_env_for(
    config_root: &Path,
    agent: &str,
) -> Result<Vec<(String, String)>, String> {
    use zerocode_core::account::{Provider, providers_for};

    let mut env = Vec::new();
    for provider in providers_for(agent) {
        match provider {
            Provider::Anthropic => env.extend(accounts::launch_env_for(config_root, agent)?),
            Provider::OpenAi => env.extend(codex_accounts::launch_env(config_root)),
        }
    }
    Ok(env)
}

/// Every agent's chosen account at once.
///
/// For a PLAIN shell, which is not a launch of anything and could be a launch
/// of everything: a person types `claude` in it on Monday and `codex` on
/// Tuesday, and both have to be the account the picker names. This is the same
/// argument that put the Claude account in `spawn_shell` in the first place,
/// carried to the second agent that has accounts.
/// The file a worktree's history directory keeps its own name in.
pub(super) const HISTORY_RECORD: &str = "meta.json";

/// What a worktree's history directory remembers about itself.
///
/// `worktree` so a later sweep can say which checkout a directory belongs to
/// without inverting a hash. `fish_data_dir` is the load-bearing one: fish
/// keeps its history in the person's OWN data dir, so this is the only thing
/// that will ever be able to find that file again — the directory this record
/// sits in holds no fish history at all.
///
/// The session NAME is deliberately absent. It is a pure function of the
/// worktree, so writing it down would only create a second answer that could
/// disagree with the first — and a record on disk is not a thing a removal
/// should be steered by.
///
/// No timestamp, because nothing reads one. A sweep asks whether the worktree
/// still exists, which is a question about the worktree and not about an age.
#[derive(Serialize, Deserialize, PartialEq)]
pub(super) struct HistoryRecord {
    pub(super) worktree: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) fish_data_dir: Option<String>,
}

/// Take a worktree's shell history away with the worktree.
///
/// Orca does the same thing through a tombstone
/// (`main/terminal-history-deletion.ts:206`): it renames the tree aside and
/// removes it asynchronously, because a directory's name is derived from the
/// worktree's PATH and a worktree recreated at that path can own the directory
/// again before an async removal lands. We are already on the blocking road
/// that just removed the worktree, and the tree is two small files, so the
/// removal happens here and there is no window to lose that race in. If this
/// ever moves off that road, the tombstone comes back with it.
///
/// fish is dealt with first, because its history is the one file NOT in the
/// directory about to go: fish keeps history in the person's own data dir under
/// a name of ours. Two directories are tried — the one recorded at spawn and
/// this process's own — because the two disagree exactly when our environment
/// differs from the PTY's, which is why it was worth recording.
///
/// The session name is DERIVED here rather than read back, so a record somebody
/// edited cannot point this at a file it does not own. Orca re-derives it for
/// the same reason.
pub(super) fn forget_worktree_history(data_root: &Path, worktree_id: &str) {
    use zerocode_core::shell_history as history;

    let dir = history::history_dir(data_root, worktree_id);
    let named =
        history::fish_history_file(&history::fish_session(&history::worktree_hash(worktree_id)));
    let look = |name: &str| std::env::var(name).ok();
    // Absolute, and fish's OWN directory. The name of the file is derived, so a
    // record somebody edited cannot choose that — but it could still choose
    // WHERE we look, and this is the shape `fish_data_dir` is the only writer
    // of. (The original has this hole; ours is narrower, not closed: a person
    // who can write our record can already write our data root.)
    let recorded = read_history_record(&dir)
        .and_then(|held| held.fish_data_dir)
        .map(PathBuf::from)
        .filter(|at| at.is_absolute() && at.file_name() == Some(std::ffi::OsStr::new("fish")));
    for at in [recorded, history::fish_data_dir(&look)]
        .into_iter()
        .flatten()
    {
        remove_plain_file(&at.join(&named));
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// What a worktree's history directory says about itself, or nothing.
pub(super) fn read_history_record(dir: &Path) -> Option<HistoryRecord> {
    serde_json::from_slice(&std::fs::read(dir.join(HISTORY_RECORD)).ok()?).ok()
}

/// Remove one file, refusing anything that is not a regular file.
///
/// A symlink here would take the removal out of the directory it was aimed at,
/// and that directory is the person's own fish data dir — full of other tools'
/// files, and not ours to be careless in.
pub(super) fn remove_plain_file(at: &Path) {
    if std::fs::symlink_metadata(at).is_ok_and(|held| held.is_file()) {
        let _ = std::fs::remove_file(at);
    }
}

/// The environment that puts a shell's history in its worktree's own file.
///
/// Every checkout is different work, and one history file for all of them
/// buries the command you want under four other worktrees' — and keeps
/// answering `Ctrl-R` with a checkout deleted last week. Orca gives each
/// worktree its own (`main/terminal-history.ts:179`).
///
/// One door, asked with the program each spawn is about to run, because the
/// answer is the program's: a configured terminal command that happens to be
/// `bash` gets the same isolation as a shell candidate, and one that is not a
/// shell we know gets nothing. Every rule lives in
/// [`zerocode_core::shell_history`] — this side only does what can fail.
///
/// A directory we cannot make is not a terminal we refuse to open: history
/// falls back to the shell's own, which is where it was before this existed.
/// The path is handed out only after the directory is there, because a
/// `HISTFILE` pointing at nothing is a shell that silently remembers nothing.
pub(super) fn worktree_history_env(
    data_root: &Path,
    worktree: &Path,
    program: &str,
) -> Vec<(String, String)> {
    use zerocode_core::shell_history as history;

    let shell = history::shell_of(program);
    let worktree_id = worktree.to_string_lossy();
    let dir = history::history_dir(data_root, &worktree_id);
    let look = |name: &str| std::env::var(name).ok();
    // Two mechanisms, because the shells do not agree. bash and fish each read
    // a variable; zsh has a config file that destroys the one it would read, so
    // it is pointed at a `ZDOTDIR` of ours instead — see `zsh_wrapper`.
    let env = match shell {
        history::Shell::Zsh => zsh_wrapper::wrapper_dir(data_root)
            .map(|wrapper| history::zsh_history_env(wrapper, &dir, &look))
            .unwrap_or_default(),
        _ => history::history_env(&dir, &history::worktree_hash(&worktree_id), shell, &look),
    };
    if env.is_empty() {
        return env;
    }
    if durable_file::ensure_private_directory(&dir).is_err() {
        return Vec::new();
    }
    remember_history_dir(&dir, &worktree_id, shell, &look);
    env
}

/// Write the record beside a worktree's history, when it is not already right.
///
/// Rewritten only on a change: a pane opening is not news, and every terminal
/// tab would otherwise cost a write. Failing to record is not failing to open
/// a terminal — it costs a later sweep its attribution and nothing else.
pub(super) fn remember_history_dir(
    dir: &Path,
    worktree_id: &str,
    shell: zerocode_core::shell_history::Shell,
    look: &impl Fn(&str) -> Option<String>,
) {
    use zerocode_core::shell_history as history;

    // Carried forward on a spawn that is not fish's. Writing `None` there
    // erased the only thing that can ever find that file again: open bash once
    // in a worktree where fish had run, and the fish history became
    // undeletable — worse when the two runs saw different data directories,
    // because then the recorded one was the only true answer. The original
    // preserves it for the same reason (`terminal-history.ts:93-101`).
    let held = read_history_record(dir);
    let record = HistoryRecord {
        worktree: worktree_id.to_string(),
        fish_data_dir: if shell == history::Shell::Fish {
            history::fish_data_dir(look).map(|at| at.to_string_lossy().into_owned())
        } else {
            held.as_ref().and_then(|held| held.fish_data_dir.clone())
        },
    };
    if held.is_some_and(|held| held == record) {
        return;
    }
    let Ok(bytes) = serde_json::to_vec(&record) else {
        return;
    };
    let _ = durable_file::replace_bytes(&dir.join(HISTORY_RECORD), &bytes);
}

pub(super) fn shell_account_env(config_root: &Path) -> Result<Vec<(String, String)>, String> {
    // A plain shell is the repair road as well as a future agent launcher. It
    // names the selected runtime homes but never refuses to open because one
    // is signed out; the provider's own login command can run in this shell.
    // Named agent launches remain strict through `account_env_for` above.
    let mut env = accounts::reading_env_for(config_root, "claude");
    if let Some((_, dir)) = env
        .iter()
        .find(|(key, _)| key == zerocode_core::account::SECURE_STORAGE_CONFIG_DIR_VAR)
    {
        // A typed `claude` will read this store too. Missing login files remain
        // a valid repair shell; a failed file-era migration must be reported.
        accounts::seed_scoped_keychain(Path::new(dir))?;
    }
    env.extend(codex_accounts::launch_env(config_root));
    Ok(env)
}

#[derive(Serialize)]
pub(super) struct UsageReport {
    pub(super) usage: Option<usage::ProviderUsage>,
    pub(super) fetching: bool,
}

/// The plan snapshot, and — when it is old enough or somebody insists — a
/// fresh scan kicked off behind it.
///
/// Never blocks on the terminal: the command answers with what is known and
/// `fetching: true`, and the window asks again. A scan is never repeated
/// within [`usage::MIN_REFETCH`] unless forced, and two scans never run at
/// once whatever the timing of the asks.
/// How many times in a row a provider's scan has failed, and when it was last
/// attempted.
///
/// Two facts a snapshot cannot carry. `updated_at` is the FIGURES' timestamp,
/// and after a failure those figures are the previous read's — see
/// [`usage_through_failure`] — so a retry measured from it would hammer a
/// provider for as long as its last good numbers stood.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct FailureRun {
    pub(super) streak: u32,
    pub(super) attempted_at: i64,
}

/// The run of failures per provider id. One map rather than a static beside
/// each cache: the providers differ in where their figures come from and in
/// nothing else about this.
pub(super) fn usage_failure_runs() -> &'static Mutex<HashMap<String, FailureRun>> {
    static RUNS: std::sync::OnceLock<Mutex<HashMap<String, FailureRun>>> =
        std::sync::OnceLock::new();
    RUNS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Note what a finished scan was, for the next one's sake.
///
/// The streak resets on anything that is not an error — Orca resets on `ok`
/// AND on `unavailable` (`service.ts:1565-1566`), because an unconfigured
/// provider is an answer and not a failure to back off from.
pub(super) fn note_usage_attempt(provider: &str, status: &str, now_ms: i64) {
    let mut runs = usage_failure_runs()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let run = runs.entry(provider.to_string()).or_default();
    run.attempted_at = now_ms;
    run.streak = if status == "error" {
        run.streak
            .saturating_add(1)
            .min(zerocode_core::usage_limit::MAX_ACTIVE_FAILURE_STREAK)
    } else {
        0
    };
}

/// Whether a scan should be skipped — the one gate all three providers ask
/// before spending a round trip.
///
/// Three reasons to hold, and `force` answers past all of them because a
/// person pressing refresh is not an automated poll (Orca gates only the
/// automated road, `service.ts:1459-1461`):
///
/// - the figures are younger than [`usage::MIN_REFETCH`];
/// - the last read failed and the SERVER named a time to come back at;
/// - it failed and named none, so the streak's own backoff applies —
///   thirty seconds doubling to the poll cadence
///   (`zerocode_core::usage_limit::failure_backoff_ms`).
///
/// Without the last two, a provider that answered 429 was asked again on the
/// ordinary cadence forever, which is how a rate limit is kept alive.
pub(super) fn usage_scan_holds(
    held: Option<&usage::ProviderUsage>,
    force: bool,
    now_ms: i64,
) -> bool {
    if force {
        return false;
    }
    let Some(snapshot) = held else {
        return false;
    };
    if now_ms - snapshot.updated_at < usage::MIN_REFETCH.as_millis() as i64 {
        return true;
    }
    if snapshot.status != "error" {
        return false;
    }
    if snapshot.retry_at_ms.is_some_and(|at| at > now_ms) {
        return true;
    }
    let run = usage_failure_runs()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&snapshot.provider)
        .copied()
        .unwrap_or_default();
    if run.streak == 0 {
        return false;
    }
    now_ms - run.attempted_at < zerocode_core::usage_limit::failure_backoff_ms(run.streak)
}

/// Every project this window remembers, canonical and deduplicated.
///
/// Cheap on purpose — it reads a small file and canonicalises paths, and that
/// is the only half of the answer that needs the window's own state. Opening
/// the repositories is the other half and runs where slow work belongs
/// ([`workspace_roots`]).
pub(super) fn known_project_roots(state: &AppState) -> Vec<PathBuf> {
    let here = state.active();
    let active_project = here
        .orchestrator
        .as_ref()
        .map_or_else(|| here.root.clone(), |open| open.repo_root().to_path_buf());
    let mut paths = stored_projects(state.config_root());
    paths.push(active_project.to_string_lossy().into_owned());
    let mut roots = Vec::new();
    let mut seen = HashSet::new();
    for path in paths {
        let Ok(canonical) = PathBuf::from(&path).canonicalize() else {
            continue;
        };
        if seen.insert(canonical.clone()) {
            roots.push(canonical);
        }
    }
    roots
}

/// Every workspace under those projects, with the word the sidebar calls it by.
///
/// Runs a `git worktree list` per project, so it belongs on the blocking side
/// of the scan and not on the thread answering commands. The word is the
/// sidebar's own (`worktreeWord`: the branch, or the directory's name for a
/// detached or folder workspace), so a row on the usage screen and a row in
/// the sidebar say the same thing about the same checkout.
pub(super) fn workspace_roots(projects: Vec<PathBuf>) -> Vec<(PathBuf, String)> {
    let mut roots = Vec::new();
    for canonical in projects {
        let Ok(orchestrator) = Orchestrator::open(&canonical) else {
            // A folder project has no git to list; it is still a place turns
            // happen in, and its own name is what to call it.
            roots.push((canonical.clone(), directory_word(&canonical)));
            continue;
        };
        let Ok(listed) = orchestrator.list() else {
            continue;
        };
        for worktree in listed {
            let word = worktree
                .branch
                .clone()
                .unwrap_or_else(|| directory_word(&worktree.path));
            roots.push((worktree.path, word));
        }
    }
    roots
}

/// A directory's own name, for the rows whose word is not a branch.
pub(super) fn directory_word(path: &Path) -> String {
    path.file_name()
        .and_then(std::ffi::OsStr::to_str)
        .unwrap_or_else(|| path.to_str().unwrap_or("workspace"))
        .to_string()
}

/// The stale policy: a failed read keeps the figures the last good one left.
///
/// Orca's `applyStalePolicy` (`service.ts:2087-2141`). Only a FAILURE can
/// inherit — a reading that succeeded replaces what was there outright, and so
/// does one that says the provider is not configured, which Orca discards
/// stale data for deliberately (`:2104-2106`). The figures are the previous
/// read's and everything about the failure is this one's, so the bar keeps a
/// number while still saying what went wrong.
pub(super) fn usage_through_failure(
    previous: usage::ProviderUsage,
    fresh: usage::ProviderUsage,
    now_ms: i64,
) -> usage::ProviderUsage {
    if fresh.status != "error" {
        return fresh;
    }
    // A percentage is a fact about ONE login. Inheriting across a switch would
    // put the previous account's number under the new account's name — the
    // same rule the read path already applies before showing a snapshot.
    if previous.account != fresh.account {
        return fresh;
    }
    let has_figures = previous.session.is_some()
        || previous.weekly.is_some()
        || previous.fable_weekly.is_some()
        || previous
            .buckets
            .as_ref()
            .is_some_and(|held| !held.is_empty());
    if !has_figures
        || !zerocode_core::usage_limit::may_stand(fresh.failure_kind, previous.updated_at, now_ms)
    {
        return fresh;
    }
    usage::ProviderUsage {
        session: previous.session,
        weekly: previous.weekly,
        fable_weekly: previous.fable_weekly,
        buckets: previous.buckets,
        // The figures' own timestamp travels with them: it is what the next
        // failure measures its staleness against, and stamping it `now` would
        // let a snapshot live forever through a steady drip of failures.
        updated_at: previous.updated_at,
        ..fresh
    }
}

/// A finished scan lands here, and nowhere else.
///
/// Three providers wrote the same six lines — serialise, make the directory,
/// write, take the cache — and all three carried the same hole: the result
/// replaced whatever was there, so one dropped packet blanked a good reading
/// until the next scan came back. The rule that fixes it lives in one place
/// now because the writing does. The file goes through [`durable_file`] for
/// the same reason every other record in this window does: a torn snapshot is
/// a snapshot that will not parse on the next boot.
pub(super) fn land_usage_scan(
    cache: &'static Mutex<Option<usage::ProviderUsage>>,
    file: &Path,
    fresh: usage::ProviderUsage,
) {
    let now = epoch_ms_now();
    let mut held = cache
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let landed = match held.take() {
        Some(previous) => usage_through_failure(previous, fresh, now),
        None => fresh,
    };
    if let Ok(text) = serde_json::to_string_pretty(&landed) {
        let _ = durable_file::replace_bytes(file, text.as_bytes());
    }
    note_usage_attempt(&landed.provider, &landed.status, now);
    *held = Some(landed);
}

/// Where one provider's gauge keeps its state.
///
/// Three of these, soon more, and every one of them is the same three things:
/// somewhere to hold the last reading, somewhere to write it, and a flag
/// saying a scan is already out. They are deliberately NOT shared between
/// providers — one flag would let the faster scan clear the slower one's, and
/// one file would show one CLI's figure under another's name.
pub(super) struct UsageGauge {
    pub(super) cache: &'static Mutex<Option<usage::ProviderUsage>>,
    pub(super) file: PathBuf,
    pub(super) scanning: &'static std::sync::atomic::AtomicBool,
}

/// The never-block ask, for every provider that has one.
///
/// Four commands had written this out separately and were beginning to
/// disagree — another provider already had a different filter and a different
/// set of thread inputs, and a fifth provider would have been a fifth copy of
/// a contract nobody could read in one place. The contract is: answer from the
/// cache immediately, start at most one scan, never scan again inside the
/// refetch floor unless forced, and never block the command thread.
///
/// `keep` is what makes this shareable, and it runs HERE rather than in the
/// worker. Every provider drops a held snapshot for its own reason — the wrong
/// login for Claude and Codex — and those are not
/// the same question wearing two hats. A boolean would have made them one and
/// silently defaulted somebody wrong: a snapshot shown under the wrong
/// account's name is a plausible number that is a lie, on the one surface
/// whose whole purpose is to be believed at a glance. Reading the gate on this
/// thread is also what makes flipping an opt-in answer the very NEXT poll
/// rather than whenever a scan happens to end.
///
/// `scan` is a closure and not a function pointer for a reason that is not
/// taste: it runs on a spawned thread, so it can only see what it owns. Taking
/// it as `FnOnce() -> ProviderUsage + Send + 'static` makes the compiler check
/// that each caller extracted its roots and settings BEFORE the spawn, which
/// is exactly the discipline the four copies were keeping by hand.
pub(super) fn usage_report(
    gauge: UsageGauge,
    keep: impl Fn(&usage::ProviderUsage) -> bool,
    force: bool,
    scan: impl FnOnce() -> usage::ProviderUsage + Send + 'static,
) -> UsageReport {
    use std::sync::atomic::Ordering;
    let held = gauge
        .cache
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
        .filter(|snapshot| keep(snapshot));
    if gauge.scanning.load(Ordering::SeqCst) {
        return UsageReport {
            usage: held,
            fetching: true,
        };
    }
    if usage_scan_holds(held.as_ref(), force, epoch_ms_now())
        || gauge
            .scanning
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
    {
        return UsageReport {
            usage: held,
            fetching: false,
        };
    }
    let UsageGauge {
        cache,
        file,
        scanning,
    } = gauge;
    std::thread::spawn(move || {
        // Held for the whole scan: a panic in the parse must not leave the
        // segment saying "reading…" for the rest of the session.
        let _flag = ScanFlag(scanning);
        let result = scan();
        land_usage_scan(cache, &file, result);
    });
    UsageReport {
        usage: held,
        fetching: true,
    }
}

/// What one provider's half of the stats pane draws, plus how fresh it is.
///
/// One shape for all three ledgers, generic only over what a `report` is:
/// Claude's four independent counters and the five-counter shape the other two
/// share are different types, but "did it scan, when, how much did it read"
/// is the same question three times. Written three times it was three places
/// to forget `scanning: false` on a switched-off pane.
#[derive(serde::Serialize)]
pub(super) struct StatsReport<T> {
    /// Whether this window reads this provider's ledger at all. A pane that
    /// is off draws its own card and never asks for a scan.
    pub(super) enabled: bool,
    /// `None` before the first scan has ever finished.
    pub(super) report: Option<T>,
    /// A scan is running; the pane says so rather than showing a spinner over
    /// nothing.
    pub(super) scanning: bool,
    /// Epoch milliseconds of the scan behind `report`.
    pub(super) scanned_at: Option<i64>,
    /// What the scan read — transcripts, rollouts or database rows — and
    /// whether its bound cut the walk short.
    pub(super) files: usize,
    pub(super) capped: bool,
    /// Why the read did not finish, when it did not. A ledger this window
    /// could not open must say so: an unreadable database reported as zero
    /// tokens is a wrong answer nobody looking can tell from a right one.
    pub(super) error: Option<String>,
}

impl<T> StatsReport<T> {
    /// The answer a switched-off pane gets: no ledger, and no scan was asked
    /// for. `scanning: false` matters — a pane that is off must not spin.
    pub(super) fn off() -> Self {
        Self {
            enabled: false,
            report: None,
            scanning: false,
            scanned_at: None,
            files: 0,
            capped: false,
            error: None,
        }
    }
}

/// The providers whose token ledgers this window reads, and so the only words
/// the analytics switch accepts.
///
/// One list rather than a condition per door: a fourth ledger that is added to
/// the panes but not to this list gets a switch that answers "not a provider",
/// which reads as a bug in the switch rather than as a missing line here.
pub(super) const LEDGER_PROVIDERS: [&str; 3] = ["claude", "codex", "opencode"];

/// Whether this window reads one provider's token ledger.
///
/// Read on the command thread rather than cached, for the reason the OpenCode
/// gate is: turning the switch off should stop the next scan, not the one
/// after some other event happens to trigger.
pub(super) fn usage_analytics_on(state: &State<'_, AppState>, provider: &str) -> bool {
    !load_settings_resilient(state.settings())
        .document
        .usage_analytics_off
        .iter()
        .any(|row| row == provider)
}

/// Every worktree this window manages, as the attribution needs to see it.
///
/// Canonicalized here rather than in the core module, which does no I/O: the
/// transcripts record the directory the agent actually ran in, and on this
/// platform that is the resolved path while the catalog holds the one the
/// person typed. Comparing the two unresolved is how a whole checkout's
/// tokens end up filed under "not one of ours".
pub(super) fn managed_worktrees(state: &AppState) -> Vec<zerocode_core::usage_stats::WorktreeRef> {
    let mut refs = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut paths = stored_projects(state.config_root());
    let here = state.active();
    paths.push(here.root.to_string_lossy().into_owned());
    for path in paths {
        let Ok(canonical) = PathBuf::from(&path).canonicalize() else {
            continue;
        };
        let Some(open) = Orchestrator::open(&canonical).ok() else {
            // A folder project is still a place this window owns, even though
            // git will not list worktrees for it.
            let path = canonical.to_string_lossy().into_owned();
            if seen.insert(path.clone()) {
                let name = canonical
                    .file_name()
                    .and_then(std::ffi::OsStr::to_str)
                    .unwrap_or("project")
                    .to_string();
                refs.push(zerocode_core::usage_stats::WorktreeRef {
                    repo_id: path.clone(),
                    worktree_id: path.clone(),
                    path,
                    display_name: name,
                });
            }
            continue;
        };
        let repo_id = open
            .shared_root()
            .unwrap_or_else(|_| open.repo_root().to_path_buf())
            .to_string_lossy()
            .into_owned();
        let repo_name = open
            .repo_root()
            .file_name()
            .and_then(std::ffi::OsStr::to_str)
            .unwrap_or("project")
            .to_string();
        let Ok(listed) = open.list() else {
            continue;
        };
        for worktree in listed {
            let resolved = worktree
                .path
                .canonicalize()
                .unwrap_or_else(|_| worktree.path.clone());
            let path = resolved.to_string_lossy().into_owned();
            if !seen.insert(path.clone()) {
                continue;
            }
            let leaf = resolved
                .file_name()
                .and_then(std::ffi::OsStr::to_str)
                .unwrap_or("worktree")
                .to_string();
            let display_name = if worktree.is_main {
                repo_name.clone()
            } else {
                format!("{repo_name} · {leaf}")
            };
            refs.push(zerocode_core::usage_stats::WorktreeRef {
                repo_id: repo_id.clone(),
                worktree_id: path.clone(),
                path,
                display_name,
            });
        }
    }
    refs
}

/// Clears a provider's in-flight flag however the scan ends.
///
/// A scan is a screen parse of text a CLI drew, which means it is the kind of
/// code that can panic on an input nobody predicted — and a panic in the
/// spawned thread would skip the `store(false)` at the end, leaving the flag
/// on FOREVER. The segment then says "reading…" for the rest of the session
/// and no later ask can start a scan, because every one of them sees a scan
/// already running. A `Drop` runs on the unwind, which is the whole point.
pub(super) struct ScanFlag(pub(super) &'static std::sync::atomic::AtomicBool);

impl Drop for ScanFlag {
    fn drop(&mut self) {
        self.0.store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

pub(super) fn codex_usage_file(local_data_root: &Path) -> PathBuf {
    local_data_root.join(artifact_file::CODEX_USAGE)
}

pub(super) fn codex_usage_cache(
    local_data_root: &Path,
) -> &'static Mutex<Option<usage::ProviderUsage>> {
    static CACHE: std::sync::OnceLock<Mutex<Option<usage::ProviderUsage>>> =
        std::sync::OnceLock::new();
    CACHE.get_or_init(|| {
        Mutex::new(
            std::fs::read_to_string(codex_usage_file(local_data_root))
                .ok()
                .and_then(|text| serde_json::from_str(&text).ok()),
        )
    })
}

pub(super) static CODEX_USAGE_SCANNING: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Where the OpenCode Go session cookie lives — the OS keychain, under one
/// account name.
///
/// Orca keeps it in a file behind a `orca-minimax-cookie:v1:` prefix for the
/// sibling provider, which is an envelope rather than protection: anything
/// that can read the file can read the cookie. This window already has a
/// verified keychain store standing (jira and the SSH hosts use it), so the
/// cookie goes where the other secrets go.
pub(super) const OPENCODE_CREDENTIAL_SERVICE: &str = "dev.zerocode.shell.opencode";
pub(super) const OPENCODE_COOKIE_ACCOUNT: &str = "session-cookie";

/// Deliberately NOT cfg-split for tests, and the reason is a trap worth
/// naming: every source-shape gate in this file finds the shipped half by
/// splitting on the first test-module attribute, so a test-only branch out
/// here truncates what those gates can see — 95 of them went blind on the
/// first attempt at this function. Nothing in the tests constructs it, so
/// there is nothing to stand in for anyway.
pub(super) fn opencode_cookie_store() -> credential_store::VerifiedSecretStore {
    credential_store::VerifiedSecretStore::new(OPENCODE_CREDENTIAL_SERVICE)
}

pub(super) fn opencode_usage_file(local_data_root: &Path) -> PathBuf {
    local_data_root.join(artifact_file::OPENCODE_USAGE)
}

pub(super) fn opencode_usage_cache(
    local_data_root: &Path,
) -> &'static Mutex<Option<usage::ProviderUsage>> {
    static CACHE: std::sync::OnceLock<Mutex<Option<usage::ProviderUsage>>> =
        std::sync::OnceLock::new();
    CACHE.get_or_init(|| {
        Mutex::new(
            std::fs::read_to_string(opencode_usage_file(local_data_root))
                .ok()
                .and_then(|text| serde_json::from_str(&text).ok()),
        )
    })
}

pub(super) static OPENCODE_USAGE_SCANNING: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// A fresh instance token per call, as the site's server-fn protocol expects
/// (`opencode-go-usage-fetcher.ts:177`). Built from the clock and a counter
/// rather than pulling in a uuid crate for one header this window never reads
/// back: the protocol wants it unique per request, not unguessable.
pub(super) fn server_fn_instance() -> String {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let count = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("{:x}-{count:x}", epoch_ms_now())
}

pub(super) fn grok_usage_file(local_data_root: &Path) -> PathBuf {
    local_data_root.join(artifact_file::GROK_USAGE)
}

pub(super) fn grok_usage_cache(
    local_data_root: &Path,
) -> &'static Mutex<Option<usage::ProviderUsage>> {
    static CACHE: std::sync::OnceLock<Mutex<Option<usage::ProviderUsage>>> =
        std::sync::OnceLock::new();
    CACHE.get_or_init(|| {
        Mutex::new(
            std::fs::read_to_string(grok_usage_file(local_data_root))
                .ok()
                .and_then(|text| serde_json::from_str(&text).ok()),
        )
    })
}

pub(super) static GROK_USAGE_SCANNING: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

pub(super) fn kimi_usage_file(local_data_root: &Path) -> PathBuf {
    local_data_root.join(artifact_file::KIMI_USAGE)
}

pub(super) fn kimi_usage_cache(
    local_data_root: &Path,
) -> &'static Mutex<Option<usage::ProviderUsage>> {
    static CACHE: std::sync::OnceLock<Mutex<Option<usage::ProviderUsage>>> =
        std::sync::OnceLock::new();
    CACHE.get_or_init(|| {
        Mutex::new(
            std::fs::read_to_string(kimi_usage_file(local_data_root))
                .ok()
                .and_then(|text| serde_json::from_str(&text).ok()),
        )
    })
}

pub(super) static KIMI_USAGE_SCANNING: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

pub(super) fn antigravity_usage_file(local_data_root: &Path) -> PathBuf {
    local_data_root.join(artifact_file::ANTIGRAVITY_USAGE)
}

pub(super) fn antigravity_usage_cache(
    local_data_root: &Path,
) -> &'static Mutex<Option<usage::ProviderUsage>> {
    static CACHE: std::sync::OnceLock<Mutex<Option<usage::ProviderUsage>>> =
        std::sync::OnceLock::new();
    CACHE.get_or_init(|| {
        Mutex::new(
            std::fs::read_to_string(antigravity_usage_file(local_data_root))
                .ok()
                .and_then(|text| serde_json::from_str(&text).ok()),
        )
    })
}

pub(super) static ANTIGRAVITY_USAGE_SCANNING: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// The same never-block contract as [`claude_usage`] and [`codex_usage`],
/// against Google's quota API — no hidden terminal at all, but the same
/// cache file, in-flight flag and refetch floor, so all gauges age by one rule.
pub(super) fn antigravity_usage_report(state: &State<'_, AppState>, force: bool) -> UsageReport {
    let local_data_root = state.local_data_root().to_path_buf();
    usage_report(
        UsageGauge {
            cache: antigravity_usage_cache(&local_data_root),
            file: antigravity_usage_file(&local_data_root),
            scanning: &ANTIGRAVITY_USAGE_SCANNING,
        },
        |_| true,
        force,
        move || usage_antigravity::scan(epoch_ms_now()),
    )
}

/* ---- CI checks --------------------------------------------------------------
 *
 * What CI says about the review this checkout is on. The normalisers are in
 * `zerocode-core::checks` and the `gh` calls in `gh.rs`; what lives here is
 * the pair of commands and the one decision they make — that a window with no
 * `gh`, no PR, or no network gets a *reason* rather than an empty list.
 *
 * No cache of our own. `gh api --cache 60s` already holds one, keyed by the
 * request, shared with everything else the person's `gh` does — a second cache
 * in this process would only be a second thing to invalidate. The plan segment
 * needed one because typing `/usage` at a hidden terminal costs seconds; three
 * cached HTTP reads do not. */

/// What the checks panel is looking at, or why it is looking at nothing.
#[derive(Serialize)]
pub(super) struct ChecksReport {
    /// `None` when this checkout's branch has no pull request.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) review: Option<gh::HostedReview>,
    pub(super) checks: Vec<CheckRun>,
    /// `missing`, `refused` or `unreadable` — a key the window localises.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) error: Option<String>,
    /// What `gh` said, when it said something worth reading.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) detail: Option<String>,
    /// True when the list is at the wire's ceiling, so the panel can say so
    /// instead of showing a prefix as if it were everything.
    pub(super) at_page_limit: bool,
    /// The stack the review is on (t-2733), appended after the fields the
    /// panel already reads. `None` when there is no review or the open
    /// reviews could not be listed — no map rather than a wrong one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) stack: Option<checks_runtime::PrStack>,
}

impl ChecksReport {
    pub(super) fn refused(error: gh::GhError) -> Self {
        Self {
            review: None,
            checks: Vec::new(),
            detail: error.detail().map(str::to_string),
            error: Some(error.reason().to_string()),
            at_page_limit: false,
            stack: None,
        }
    }
}

/* ---- more than one Claude account -----------------------------------------
 *
 * An account is a `CLAUDE_CONFIG_DIR`, which is the only fact this rests on:
 * the CLI reads its credentials from there, so two directories are two
 * accounts and switching is naming a different one. The model is in
 * `zerocode-core::account`, the directories and the login in `accounts.rs`.
 *
 * This window never holds a token. Adding an account runs the CLI's own login
 * against a directory we made; the one thing read back is who arrived. */

#[derive(Serialize)]
pub(super) struct AccountsReport {
    pub(super) accounts: Vec<AccountRow>,
    /// The one a launch will use, resolved — not the stored id, which can name
    /// an account that has since been removed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) active: Option<String>,
    /// False when this machine has no `claude` to log in with, so the window
    /// can say that instead of offering a button that cannot work.
    pub(super) can_add: bool,
}

#[derive(Serialize)]
pub(super) struct AccountRow {
    pub(super) id: String,
    pub(super) email: String,
    pub(super) label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) organization_name: Option<String>,
    pub(super) organization_uuid: Option<String>,
    pub(super) account_uuid: Option<String>,
    pub(super) organization_type: Option<String>,
    pub(super) pending: Option<zerocode_core::account::ClaudeIdentity>,
    pub(super) added_at: i64,
    /// False when the directory behind this row has lost its credentials —
    /// deleted by hand, or a state directory restored without them. The row
    /// stays visible and says so rather than vanishing, because a person who
    /// added an account and cannot find it has no way to fix it.
    pub(super) signed_in: bool,
    /// True when the last usage scan RAN as this account and met the CLI's
    /// logged-out screen. `signed_in` asks the directory who it is; only the
    /// CLI can answer whether the token still works, and a row that looks
    /// healthy while every terminal it opens says "Please run /login" is the
    /// bug this reports (live report 2026-08-14, 1-g15).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub(super) login_expired: bool,
}

/// Which account a usage snapshot says has no login, if it says that at all.
///
/// Pure so the decision can be tested without a scan: the two ways to get
/// this wrong are marking on a snapshot that only FAILED (a read error is
/// not a verdict about anybody's credentials) and marking a row the scan
/// never ran as (1-g15).
pub(super) fn signed_out_account(snapshot: Option<&usage::ProviderUsage>) -> Option<&str> {
    snapshot
        .filter(|one| one.status == usage::SIGNED_OUT_STATUS)
        .and_then(|one| one.account.as_deref())
}

pub(super) fn accounts_report(config_root: &Path, local_data_root: &Path) -> AccountsReport {
    let store = accounts::read_store(config_root);
    let active =
        zerocode_core::active_account(&store.accounts, &store.selection).map(|one| one.id.clone());
    // Which account the last scan found logged OUT, if any. One snapshot is
    // kept, so this can only ever mark the account it ran as — which is the
    // one the person is using, and the one the sentence is about.
    let expired = signed_out_account(
        claude_usage_cache(local_data_root)
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref(),
    )
    .map(str::to_string);
    AccountsReport {
        accounts: store
            .accounts
            .iter()
            .map(|account| AccountRow {
                id: account.id.clone(),
                email: account.email.clone(),
                label: account.label(),
                organization_name: account.organization_name.clone(),
                organization_uuid: account.organization_uuid.clone(),
                account_uuid: account.account_uuid.clone(),
                organization_type: account.organization_type.clone(),
                pending: account.pending.clone(),
                added_at: account.added_at,
                signed_in: accounts::signed_in(Path::new(&account.config_dir)),
                login_expired: expired.as_deref() == Some(account.id.as_str()),
            })
            .collect(),
        active,
        can_add: claude_program().is_some(),
    }
}

/// The command that is `claude` on this machine, from the catalogue's own
/// detection — a machine may have it under an alias, and the scan already knows
/// which name it answered to.
pub(super) fn claude_program() -> Option<String> {
    agent_program("claude")
}

/// The same question for any agent the catalogue detects.
pub(super) fn agent_program(agent: &str) -> Option<String> {
    detected_agents(false)
        .into_iter()
        .find(|row| row.id == agent && row.installed)
        .and_then(|row| row.found_as)
}

/// The Codex accounts panel: the rows, the selection, and who the machine's
/// own login is.
///
/// The last one is the difference from the Claude report. Codex has a real
/// system default — selecting no account means `~/.codex`, live and never
/// written (Orca's `resolveSystemDefaultIdentity`, out/main/index.js:215812) —
/// so the window has to be able to draw that as a row a person can choose, and
/// to say whose login it is.
pub(super) fn codex_accounts_report(config_root: &Path) -> CodexAccountsReport {
    let store = codex_accounts::read_store(config_root);
    let active = zerocode_core::codex_account::active_account(&store.accounts, &store.selection)
        .map(|one| one.id.clone());
    let (system_kind, system) = match codex_accounts::system_identity() {
        Some((kind, identity)) => (Some(kind), Some(identity)),
        None => (None, None),
    };
    CodexAccountsReport {
        accounts: store
            .accounts
            .iter()
            .map(|account| CodexAccountRow {
                id: account.id.clone(),
                label: account.label(),
                email: account.email.clone(),
                workspace_label: account.workspace_label.clone(),
                added_at: account.added_at,
                signed_in: codex_accounts::signed_in(Path::new(&account.home_dir)),
                // False here, filled by the window from `verify_codex_accounts`
                // a moment later. The Claude pair also SEEDS this from its
                // usage snapshot so the first paint can already be right —
                // deliberately not copied, because that seed can only ever
                // mark the one account the last scan ran as, while the oracle
                // answers for every row. The cost is that a dead login shows
                // one paint late; the gain is that it shows at all for the
                // rows nobody has run yet.
                login_expired: false,
            })
            .collect(),
        active,
        system_label: system
            .as_ref()
            .and_then(zerocode_core::codex_account::CodexIdentity::label),
        system_kind: system_kind.map(|kind| match kind {
            zerocode_core::codex_account::AuthKind::Oauth => "oauth",
            zerocode_core::codex_account::AuthKind::ApiKey => "api-key",
            zerocode_core::codex_account::AuthKind::None => "none",
        }),
        can_add: agent_program("codex").is_some(),
    }
}

#[derive(Serialize)]
pub(super) struct CodexAccountsReport {
    pub(super) accounts: Vec<CodexAccountRow>,
    /// `None` means the machine's own login — a state, not a gap.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) active: Option<String>,
    /// Who `~/.codex` is signed in as, when it can be named.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) system_label: Option<String>,
    /// `oauth`, `api-key`, or absent when the machine has no Codex login.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) system_kind: Option<&'static str>,
    pub(super) can_add: bool,
}

#[derive(Serialize)]
pub(super) struct CodexAccountRow {
    pub(super) id: String,
    /// What the row says: the email with its workspace, or the workspace, or
    /// that this is a key login — never nothing.
    pub(super) label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) email: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) workspace_label: Option<String>,
    pub(super) added_at: i64,
    pub(super) signed_in: bool,
    /// Set by the window from `verify_codex_accounts`, never by the store.
    /// `signed_in` only says `auth.json` has something in it, and a token
    /// that DIED leaves that file untouched — the two facts are independent
    /// and the row needs both (the Claude pair's `login_expired`, 1-g15).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(super) login_expired: bool,
}

/* ---- agent event hooks ----------------------------------------------------
 *
 * The other half of what Orca calls hooks (the first is `zerocode.yaml`'s
 * repository scripts, §1-ay): the bridge an agent CLI reports its own lifecycle
 * to. See `hooks.rs` for the policy and `zerocode-hookd` for the server, the
 * scripts and the installers. */

/// The nonce a launched agent reports under.
///
/// Not a secret — the bridge's token is the secret — so the terminal id plus a
/// process-lifetime counter is enough: all this has to do is DIFFER between two
/// occupants of the same pane, and never repeat within one run of the app.
pub(super) fn new_launch_token(term: TermId) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    format!("lt{term}-{}", NEXT.fetch_add(1, Ordering::Relaxed))
}

/// Which agents this machine actually has, as slugs the installer understands.
pub(super) fn installed_agent_slugs() -> Vec<String> {
    detected_agents(false)
        .into_iter()
        .filter(|row| row.installed)
        .map(|row| row.id.to_string())
        .collect()
}

/// Whether hooks are on, and what each managed agent's config currently says.
#[derive(Serialize)]
pub(super) struct HooksReport {
    pub(super) enabled: bool,
    /// True once the bridge is listening. False on a machine where the bind or
    /// the token read failed, which is worth saying: the entries are installed
    /// and nothing will arrive.
    pub(super) listening: bool,
    pub(super) agents: Vec<zerocode_hookd::install::HookStatus>,
}

pub(super) fn hooks_report_of(state: &AppState) -> HooksReport {
    let config = state.config_root();
    HooksReport {
        enabled: hooks::hooks_enabled(config),
        listening: hooks::bridge().is_some(),
        agents: hooks::statuses(config, state.local_data_root()),
    }
}
