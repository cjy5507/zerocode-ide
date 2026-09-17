//! Usage commands.

use crate::*;
use zerocode_core::account::Provider;

/// What Claude spent, counted off its own transcripts.
///
/// User-initiated rather than polled: the scan reads every transcript on disk
/// (915 MB and 1.9 s on the machine this was measured on), which is the wrong
/// shape for a fifteen-minute gauge and the right one for a screen somebody
/// opens. `spawn_blocking` because that work is file I/O and belongs off the
/// thread answering commands.
///
/// The dollar figure is an ESTIMATE at API rates and is labelled as one where
/// it is shown: a plan subscription is not billed per token, so this says what
/// the same work would have cost through the API — Orca makes the same claim
/// of its own figure (`ClaudeUsagePane.tsx:224`).
#[tauri::command]
pub(crate) async fn claude_token_usage(
    state: State<'_, AppState>,
    offset_minutes: i32,
) -> Result<claude_tokens::TokenScan, String> {
    // The project list is read HERE because it comes through the window's own
    // state, which does not cross onto a worker thread. Opening those
    // repositories does NOT happen here — that is a `git worktree list` per
    // project, and the thread answering commands is the wrong place for it.
    let projects = known_project_roots(&state);
    let runtime_home = accounts::runtime_home(state.config_root());
    tauri::async_runtime::spawn_blocking(move || {
        let mut places = usage_places::Places::new(workspace_roots(projects));
        Ok(claude_tokens::scan(
            &claude_tokens::projects_root(&runtime_home),
            offset_minutes,
            &mut places,
        ))
    })
    .await
    .map_err(|error| error.to_string())?
}

/// What Codex spent, counted off its own rollouts.
///
/// The sibling of [`claude_token_usage`] and the same shape of ask: user
/// initiated, off the command thread, the day boundary supplied by the window.
/// The counting rule is not the same, because Codex reports a running session
/// total beside each turn and adding those up overstates by fifty
/// (`codex_tokens`).
#[tauri::command]
pub(crate) async fn codex_token_usage(
    state: State<'_, AppState>,
    offset_minutes: i32,
) -> Result<codex_tokens::CodexScan, String> {
    let projects = known_project_roots(&state);
    tauri::async_runtime::spawn_blocking(move || {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let mut places = usage_places::Places::new(workspace_roots(projects));
        home.map(|home| codex_tokens::scan(&home, offset_minutes, &mut places))
            .ok_or_else(|| "홈 디렉터리를 찾지 못했습니다".to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

/// The three figures at the head of the stats pane.
///
/// Counted rather than derived: no ledger this window reads knows that a
/// person started an agent or that this window opened a pull request. See
/// [`zerocode_core::stats_events`] for what each verb does to them.
#[tauri::command(async)]
pub(crate) fn stats_summary() -> zerocode_core::stats_events::StatsCounters {
    stats_events_store::read()
}

/// The token ledger, for one scope and range.
///
/// Never blocks: the answer is whatever the last finished scan holds, and a
/// scan is started behind it when there is none or the caller forced one.
/// `offset_minutes` is the WINDOW's UTC offset, because a day is a human unit
/// and only the window knows which midnight the person means.
#[tauri::command(async)]
pub(crate) fn claude_usage_stats(
    state: State<'_, AppState>,
    scope: String,
    range: String,
    offset_minutes: i32,
    force: bool,
) -> Result<StatsReport<zerocode_core::usage_stats::Report>, String> {
    let scope = zerocode_core::usage_stats::Scope::parse(&scope)
        .ok_or_else(|| format!("알 수 없는 범위: {scope}"))?;
    let range = zerocode_core::usage_stats::Range::parse(&range)
        .ok_or_else(|| format!("알 수 없는 기간: {range}"))?;
    if !usage_analytics_on(&state, "claude") {
        return Ok(StatsReport::off());
    }
    let now = epoch_ms_now();
    let held = usage_stats_scan::held();
    if force || held.is_none() {
        usage_stats_scan::start(
            state.config_root().to_path_buf(),
            managed_worktrees(&state),
            offset_minutes,
            now,
        );
    }
    Ok(StatsReport {
        enabled: true,
        report: held.as_ref().map(|scan| {
            zerocode_core::usage_stats::report(&scan.ledger, scope, range, now, offset_minutes)
        }),
        scanning: usage_stats_scan::scanning(),
        scanned_at: held.as_ref().map(|scan| scan.scanned_at),
        files: held.as_ref().map_or(0, |scan| scan.files),
        capped: held.as_ref().is_some_and(|scan| scan.capped),
        error: held.as_ref().and_then(|scan| scan.error.clone()),
    })
}

/// The Codex token ledger, for one scope and range.
///
/// Its own command rather than a `provider` argument on the Claude one: the
/// two ledgers count different buckets — Codex reports cached input as a
/// SUBSET of input and reasoning inside output, where Claude reports four
/// independent counters — and one command answering both shapes would have to
/// return the union, which is a row of nulls the window then has to guess
/// about. Two shapes, two doors, the same never-blocking contract.
#[tauri::command(async)]
pub(crate) fn codex_usage_stats(
    state: State<'_, AppState>,
    scope: String,
    range: String,
    offset_minutes: i32,
    force: bool,
) -> Result<StatsReport<zerocode_core::usage_report::Report>, String> {
    let scope = zerocode_core::usage_stats::Scope::parse(&scope)
        .ok_or_else(|| format!("알 수 없는 범위: {scope}"))?;
    let range = zerocode_core::usage_stats::Range::parse(&range)
        .ok_or_else(|| format!("알 수 없는 기간: {range}"))?;
    if !usage_analytics_on(&state, "codex") {
        return Ok(StatsReport::off());
    }
    let now = epoch_ms_now();
    let held = usage_stats_scan::codex_held();
    if force || held.is_none() {
        usage_stats_scan::codex_start(
            state.config_root().to_path_buf(),
            managed_worktrees(&state),
            offset_minutes,
            now,
        );
    }
    Ok(StatsReport {
        enabled: true,
        report: held.as_ref().map(|scan| {
            zerocode_core::usage_stats_codex::report(
                &scan.ledger,
                scope,
                range,
                now,
                offset_minutes,
            )
        }),
        scanning: usage_stats_scan::codex_scanning(),
        scanned_at: held.as_ref().map(|scan| scan.scanned_at),
        files: held.as_ref().map_or(0, |scan| scan.files),
        capped: held.as_ref().is_some_and(|scan| scan.capped),
        // A rollout this reader cannot parse is skipped rather than fatal, so
        // there is never a whole-scan failure to report here.
        error: None,
    })
}

/// The OpenCode token ledger, for one scope and range.
///
/// Reads a SQLite file rather than a directory of transcripts, so `files`
/// counts rows. Everything else is the same contract as the two doors above:
/// never blocks, answers the last finished read, starts one behind it.
#[tauri::command(async)]
pub(crate) fn opencode_usage_stats(
    state: State<'_, AppState>,
    scope: String,
    range: String,
    offset_minutes: i32,
    force: bool,
) -> Result<StatsReport<zerocode_core::usage_report::Report>, String> {
    let scope = zerocode_core::usage_stats::Scope::parse(&scope)
        .ok_or_else(|| format!("알 수 없는 범위: {scope}"))?;
    let range = zerocode_core::usage_stats::Range::parse(&range)
        .ok_or_else(|| format!("알 수 없는 기간: {range}"))?;
    if !usage_analytics_on(&state, "opencode") {
        return Ok(StatsReport::off());
    }
    let now = epoch_ms_now();
    let held = usage_stats_opencode_scan::held();
    if force || held.is_none() {
        usage_stats_opencode_scan::start(managed_worktrees(&state), offset_minutes, now);
    }
    Ok(StatsReport {
        enabled: true,
        report: held.as_ref().map(|scan| {
            zerocode_core::usage_stats_opencode::report(
                &scan.ledger,
                scope,
                range,
                now,
                offset_minutes,
            )
        }),
        scanning: usage_stats_opencode_scan::scanning(),
        scanned_at: held.as_ref().map(|scan| scan.scanned_at),
        files: held.as_ref().map_or(0, |scan| scan.rows),
        capped: held.as_ref().is_some_and(|scan| scan.capped),
        error: held.as_ref().and_then(|scan| scan.error.clone()),
    })
}

#[tauri::command(async)]
pub(crate) fn claude_usage(state: State<'_, AppState>, force: bool) -> UsageReport {
    let whose = active_claude_account_id(state.config_root());
    let config_root = state.config_root().to_path_buf();
    let local_data_root = state.local_data_root().to_path_buf();
    usage_report(
        UsageGauge {
            cache: claude_usage_cache(&local_data_root),
            file: claude_usage_file(&local_data_root),
            scanning: &CLAUDE_USAGE_SCANNING,
        },
        // A percentage belongs to one login. A snapshot read as somebody else
        // is not a stale answer to this question, it is an answer to a
        // different one — so it is dropped rather than shown while the rescan
        // runs. Shown, it would be the previous account's figure sitting under
        // the new account's name, which reads as a switch that did nothing.
        |snapshot| snapshot.account == whose,
        force,
        move || scan_claude_usage_now(&config_root),
    )
}

/// The same contract as [`claude_usage`], against Codex's own screen: never
/// blocks, never repeats within [`usage::MIN_REFETCH`] unless forced, never
/// runs two scans at once, and never shows another account's figure.
///
/// Its own cache file and its own in-flight flag rather than a shared one:
/// these are two hidden terminals against two different CLIs, and one lock
/// between them would make each provider's scan wait on the other's slowest
/// screen.
#[tauri::command(async)]
pub(crate) fn codex_usage(state: State<'_, AppState>, force: bool) -> UsageReport {
    let whose = active_codex_account_id(state.config_root());
    let config_root = state.config_root().to_path_buf();
    let local_data_root = state.local_data_root().to_path_buf();
    usage_report(
        UsageGauge {
            cache: codex_usage_cache(&local_data_root),
            file: codex_usage_file(&local_data_root),
            scanning: &CODEX_USAGE_SCANNING,
        },
        // The same rule the Claude side learned, for the same reason.
        |snapshot| snapshot.account == whose,
        force,
        move || scan_codex_usage_now(&config_root),
    )
}

/// Save (or clear) the pasted cookie. The value never touches settings.json —
/// only the fact that one is on file does.
#[tauri::command(async)]
pub(crate) fn set_opencode_cookie(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    cookie: String,
) -> Result<SettingsSnapshot, String> {
    let store = opencode_cookie_store();
    let held = cookie.trim().to_string();
    if held.is_empty() {
        let _ = store.delete(OPENCODE_COOKIE_ACCOUNT);
    } else {
        store
            .write(OPENCODE_COOKIE_ACCOUNT, zeroize::Zeroizing::new(held))
            .map_err(|error| format!("{error:?}"))?;
    }
    let configured = !cookie.trim().is_empty();
    let snapshot = commit_setting(
        &app,
        &webview,
        &state,
        &[setting_key::OPENCODE_COOKIE_CONFIGURED],
        move |settings| {
            settings.opencode_cookie_configured = configured;
            Ok(())
        },
    )?;
    // Clearing the cookie clears the reading too: a percentage read with a
    // credential the person just took back is not theirs to keep on screen.
    if !configured {
        *opencode_usage_cache(state.local_data_root())
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        let _ = std::fs::remove_file(opencode_usage_file(state.local_data_root()));
    }
    Ok(snapshot)
}

/// Which workspace to read, when the person names one.
///
/// Validated HERE rather than only at scan time: a typo saved is a typo that
/// fails silently every fifteen minutes until somebody opens the settings
/// again.
#[tauri::command(async)]
pub(crate) fn set_opencode_workspace(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    workspace: String,
) -> Result<SettingsSnapshot, String> {
    let held = workspace.trim().to_string();
    if !held.is_empty() && !usage_opencode::is_workspace_id(&held) {
        return Err("워크스페이스 id 형식이 아닙니다 — wrk_… 또는 wk_… 여야 합니다".to_string());
    }
    commit_setting(
        &app,
        &webview,
        &state,
        &[setting_key::OPENCODE_WORKSPACE],
        move |settings| {
            settings.opencode_workspace = held;
            Ok(())
        },
    )
}

/// OpenCode Go's gauge — the cookie-configured kind, which is the third
/// reason a held snapshot gets dropped.
///
/// The settings gate is read HERE, on the command thread: taking the cookie
/// back should empty the gauge on the
/// very next poll rather than whenever a scan happens to end.
#[tauri::command(async)]
pub(crate) fn opencode_usage(state: State<'_, AppState>, force: bool) -> UsageReport {
    let document = load_settings_resilient(state.settings()).document;
    let configured = document.opencode_cookie_configured;
    let workspace = document.opencode_workspace.clone();
    let local_data_root = state.local_data_root().to_path_buf();
    let cookie = configured
        .then(|| opencode_cookie_store().read(OPENCODE_COOKIE_ACCOUNT).ok())
        .flatten();
    usage_report(
        UsageGauge {
            cache: opencode_usage_cache(&local_data_root),
            file: opencode_usage_file(&local_data_root),
            scanning: &OPENCODE_USAGE_SCANNING,
        },
        // A reading taken with a cookie that has since been cleared is not an
        // answer to the question being asked now.
        move |snapshot| configured || snapshot.status == "unavailable",
        force,
        move || {
            usage_opencode::scan(
                cookie.as_deref().map_or("", |held| held.as_str()),
                (!workspace.is_empty()).then_some(workspace.as_str()),
                &server_fn_instance(),
                epoch_ms_now(),
            )
        },
    )
}

/// Grok's gauge, on the same never-block door as the other five.
///
/// Account-scoped like Claude and Codex, but the account is not one this
/// window picks — it is whichever session the Grok CLI's `auth.json` holds. So
/// the filter compares against the reading's OWN account rather than a chosen
/// one: a snapshot read as somebody else is dropped when the CLI's session
/// changes underneath us, which is the same failure the picker prevents on the
/// other two roads and can happen here without anybody touching this window.
#[tauri::command(async)]
pub(crate) fn grok_usage(state: State<'_, AppState>, force: bool) -> UsageReport {
    let local_data_root = state.local_data_root().to_path_buf();
    let whose = usage_grok::grok_home()
        .map(|home| usage_grok::auth_file(&home))
        .and_then(|file| match usage_grok::read_auth(&file, epoch_ms_now()) {
            usage_grok::Auth::Held(session) => session.email.or(session.user_id),
            _ => None,
        });
    usage_report(
        UsageGauge {
            cache: grok_usage_cache(&local_data_root),
            file: grok_usage_file(&local_data_root),
            scanning: &GROK_USAGE_SCANNING,
        },
        move |snapshot| snapshot.account == whose,
        force,
        move || usage_grok::scan(epoch_ms_now()),
    )
}

/// Kimi's gauge, on the same never-block door as the other four.
///
/// The third kind of "drop the held snapshot" reason, and the simplest: none.
/// Kimi has no account picker in this window and no opt-in gate — the CLI's
/// own credentials file is the whole story — so the held reading always
/// stands, and `keep` says exactly that rather than borrowing one of the
/// other providers' questions and answering it wrong.
#[tauri::command(async)]
pub(crate) fn kimi_usage(state: State<'_, AppState>, force: bool) -> UsageReport {
    let local_data_root = state.local_data_root().to_path_buf();
    usage_report(
        UsageGauge {
            cache: kimi_usage_cache(&local_data_root),
            file: kimi_usage_file(&local_data_root),
            scanning: &KIMI_USAGE_SCANNING,
        },
        |_| true,
        force,
        move || usage_kimi::scan(epoch_ms_now()),
    )
}

#[tauri::command(async)]
pub(crate) fn antigravity_usage(state: State<'_, AppState>, force: bool) -> UsageReport {
    antigravity_usage_report(&state, force)
}

#[tauri::command(async)]
pub(crate) fn codex_account_list(state: State<'_, AppState>) -> CodexAccountsReport {
    codex_accounts_report(state.config_root())
}

/// Add a Codex account by running the CLI's own login. Long, so it rides the
/// blocking pool with its own ceiling inside.
#[tauri::command]
pub(crate) async fn add_codex_account(
    state: State<'_, AppState>,
) -> Result<CodexAccountsReport, String> {
    let program = agent_program("codex").ok_or("이 기계에서 codex를 찾지 못했습니다")?;
    let now = epoch_ms_now();
    let config = state.config_root().to_path_buf();
    let local = state.local_data_root().to_path_buf();
    tauri::async_runtime::spawn_blocking(move || {
        codex_accounts::add_account(&config, &local, &program, now)
    })
    .await
    .map_err(|error| error.to_string())??;
    readiness_runtime::login_moved(Provider::OpenAi);
    Ok(codex_accounts_report(state.config_root()))
}

/// Whether each Codex account's login is still alive, asked of the CLI.
///
/// The Claude pair is `verify_claude_accounts` and this is the same shape for
/// the same reason: the store cannot tell a live login from a dead one, and
/// four dead logins looked perfectly connected until a terminal refused them.
/// One thread per row, off the paint path, and an empty answer when this
/// machine has no `codex` at all — a missing CLI is not a verdict about
/// anybody's credentials.
#[tauri::command]
pub(crate) async fn verify_codex_accounts(
    state: State<'_, AppState>,
) -> Result<Vec<(String, bool)>, String> {
    let Some(program) = agent_program("codex") else {
        return Ok(Vec::new());
    };
    let rows: Vec<(String, String)> = codex_accounts::read_store(state.config_root())
        .accounts
        .iter()
        .map(|account| (account.id.clone(), account.home_dir.clone()))
        .collect();
    tauri::async_runtime::spawn_blocking(move || {
        std::thread::scope(|scope| {
            let asked: Vec<_> = rows
                .iter()
                .map(|(id, home)| {
                    let program = program.clone();
                    (
                        id.clone(),
                        scope.spawn(move || codex_accounts::login_alive(&program, Path::new(home))),
                    )
                })
                .collect();
            asked
                .into_iter()
                .map(|(id, waiting)| (id, waiting.join().unwrap_or(false)))
                .collect()
        })
    })
    .await
    .map_err(|error| error.to_string())
}

/// Log in again into the home a Codex account already has.
///
/// The Claude pair is `relogin_claude_account`. Codex had no repair at all,
/// so a dead token's only road out was 지우기 + 계정 추가 — which takes the
/// home's carried `AGENTS.md` and every setting in it (1-g15's lesson, on the
/// other provider).
#[tauri::command]
pub(crate) async fn relogin_codex_account(
    state: State<'_, AppState>,
    id: String,
) -> Result<CodexAccountsReport, String> {
    let program = agent_program("codex").ok_or("이 기계에서 codex를 찾지 못했습니다")?;
    let config = state.config_root().to_path_buf();
    tauri::async_runtime::spawn_blocking(move || {
        codex_accounts::relogin_account(&config, &program, &id)
    })
    .await
    .map_err(|error| error.to_string())??;
    readiness_runtime::login_moved(Provider::OpenAi);
    Ok(codex_accounts_report(state.config_root()))
}

/// Choose a Codex account, or `None` to go back to the machine's own login.
#[tauri::command(async)]
pub(crate) fn select_codex_account(
    state: State<'_, AppState>,
    id: Option<String>,
) -> Result<CodexAccountsReport, String> {
    codex_accounts::select_account(state.config_root(), state.local_data_root(), id.as_deref())?;
    readiness_runtime::login_moved(Provider::OpenAi);
    // A pane that is already running was handed its account through `CODEX_HOME`
    // at launch, and nobody can change a running child's environment from
    // outside — so the new account is pushed to it over its channel.
    announce_account_switch(&state, zerocode_core::account::Provider::OpenAi);
    Ok(codex_accounts_report(state.config_root()))
}

#[tauri::command(async)]
pub(crate) fn remove_codex_account(
    state: State<'_, AppState>,
    id: String,
) -> Result<CodexAccountsReport, String> {
    codex_accounts::remove_account(state.config_root(), state.local_data_root(), &id)?;
    readiness_runtime::login_moved(Provider::OpenAi);
    Ok(codex_accounts_report(state.config_root()))
}

/// Log the machine's own Codex login in again — the row that names no
/// managed account. The CLI owns the browser round trip; this side waits.
#[tauri::command(async)]
pub(crate) async fn relogin_codex_login(
    state: State<'_, AppState>,
) -> Result<CodexAccountsReport, String> {
    let program = agent_program("codex").ok_or("이 기계에서 codex를 찾지 못했습니다")?;
    tauri::async_runtime::spawn_blocking(move || codex_accounts::relogin_system(&program))
        .await
        .map_err(|error| error.to_string())??;
    readiness_runtime::login_moved(Provider::OpenAi);
    Ok(codex_accounts_report(state.config_root()))
}

/// Log the machine's own Codex login out, through `codex logout`.
#[tauri::command(async)]
pub(crate) fn logout_codex_login(
    state: State<'_, AppState>,
) -> Result<CodexAccountsReport, String> {
    let program = agent_program("codex").ok_or("이 기계에서 codex를 찾지 못했습니다")?;
    codex_accounts::logout_system(&program)?;
    readiness_runtime::login_moved(Provider::OpenAi);
    Ok(codex_accounts_report(state.config_root()))
}

#[tauri::command(async)]
pub(crate) fn claude_accounts(state: State<'_, AppState>) -> AccountsReport {
    accounts_report(state.config_root(), state.local_data_root())
}

/// Add an account by running the CLI's own login.
///
/// Long — it opens a browser and waits for a person — so it rides the blocking
/// pool with its own five-minute ceiling inside.
#[tauri::command]
pub(crate) async fn add_claude_account(
    state: State<'_, AppState>,
) -> Result<AccountsReport, String> {
    let program = claude_program().ok_or("이 기계에서 claude를 찾지 못했습니다")?;
    let now = epoch_ms_now();
    let config = state.config_root().to_path_buf();
    let local = state.local_data_root().to_path_buf();
    tauri::async_runtime::spawn_blocking(move || {
        accounts::add_account(&config, &local, &program, now)?;
        // The account was just born; its settings.json must carry the hook
        // BEFORE anything launches under it. Waiting for the next boot's
        // reconcile is exactly the by-hand gap that was reported ("사용자가
        // 실제로 설치하면 자동으로 되야하는데") — the person who adds an
        // account and immediately launches would get a silent agent.
        hooks::reconcile(&config, &local, &installed_agent_slugs());
        Ok::<(), String>(())
    })
    .await
    .map_err(|error| error.to_string())??;
    readiness_runtime::login_moved(Provider::Anthropic);
    Ok(accounts_report(
        state.config_root(),
        state.local_data_root(),
    ))
}

/// Every account's login, checked against the CLI itself (1-g17).
///
/// The settings panel calls this AFTER painting from the store: the rows
/// appear instantly from what the files say, and the truth about the tokens
/// arrives a moment later and repaints the dead ones. Concurrent because the
/// answer is per-directory and a person with four accounts should not wait
/// four times for one.
#[tauri::command]
pub(crate) async fn verify_claude_accounts(
    state: State<'_, AppState>,
) -> Result<Vec<(String, Option<bool>)>, String> {
    let rows = accounts::read_store(state.config_root()).accounts;
    let Some(program) = claude_program() else {
        return Ok(rows.into_iter().map(|row| (row.id, None)).collect());
    };
    let config = state.config_root().to_path_buf();
    tauri::async_runtime::spawn_blocking(move || {
        let results = std::thread::scope(|scope| {
            let asked: Vec<_> = rows
                .iter()
                .map(|row| {
                    let program = &program;
                    scope.spawn(move || {
                        accounts::probe_identity(program, Path::new(&row.config_dir))
                    })
                })
                .collect();
            asked
                .into_iter()
                .map(|waiting| waiting.join().unwrap_or_default())
                .collect::<Vec<_>>()
        });
        let observations = rows
            .iter()
            .zip(&results)
            .map(|(row, (_, identity))| (row.clone(), identity.clone()))
            .collect::<Vec<_>>();
        accounts::observe_identities(&config, &observations)?;
        Ok(rows
            .into_iter()
            .zip(results)
            .map(|(row, (alive, _))| (row.id, alive))
            .collect())
    })
    .await
    .map_err(|error| error.to_string())?
}

/// Resolve a changed Claude organization only after the row's explicit choice.
#[tauri::command]
pub(crate) async fn resolve_claude_account_identity(
    state: State<'_, AppState>,
    id: String,
    choice: String,
) -> Result<AccountsReport, String> {
    let config = state.config_root().to_path_buf();
    let local = state.local_data_root().to_path_buf();
    tauri::async_runtime::spawn_blocking(move || {
        accounts::resolve_identity(&config, &local, &id, &choice, epoch_ms_now())
    })
    .await
    .map_err(|error| error.to_string())??;
    forget_claude_usage(state.local_data_root());
    announce_account_switch(&state, zerocode_core::account::Provider::Anthropic);
    Ok(accounts_report(
        state.config_root(),
        state.local_data_root(),
    ))
}

/// Log in again into the account's own directory (1-g15).
///
/// Blocking and long — a browser is waiting for a person — so it goes on the
/// pool like the add road does.
#[tauri::command]
pub(crate) async fn relogin_claude_account(
    state: State<'_, AppState>,
    id: String,
) -> Result<AccountsReport, String> {
    let program = claude_program().ok_or("이 기계에서 claude를 찾지 못했습니다")?;
    let config = state.config_root().to_path_buf();
    tauri::async_runtime::spawn_blocking(move || accounts::relogin_account(&config, &program, &id))
        .await
        .map_err(|error| error.to_string())??;
    // The scan's verdict was about the credentials that just changed.
    forget_claude_usage(state.local_data_root());
    readiness_runtime::login_moved(Provider::Anthropic);
    Ok(accounts_report(
        state.config_root(),
        state.local_data_root(),
    ))
}

#[tauri::command]
pub(crate) async fn select_claude_account(
    state: State<'_, AppState>,
    id: String,
) -> Result<AccountsReport, String> {
    let program = claude_program().ok_or("이 기계에서 claude를 찾지 못했습니다")?;
    let config = state.config_root().to_path_buf();
    tauri::async_runtime::spawn_blocking(move || accounts::select_account(&config, &program, &id))
        .await
        .map_err(|error| error.to_string())??;
    // Selection and verified runtime materialization have completed before the
    // UI receives success, so the next worker and the usage bar cannot observe
    // different accounts.
    forget_claude_usage(state.local_data_root());
    readiness_runtime::login_moved(Provider::Anthropic);
    announce_account_switch(&state, zerocode_core::account::Provider::Anthropic);
    Ok(accounts_report(
        state.config_root(),
        state.local_data_root(),
    ))
}

/// Back to this machine's own Claude login — the original's 「시스템 기본값」 row.
#[tauri::command(async)]
pub(crate) fn use_system_claude_login(
    state: State<'_, AppState>,
) -> Result<AccountsReport, String> {
    accounts::use_system_default(state.config_root())?;
    readiness_runtime::login_moved(Provider::Anthropic);
    announce_account_switch(&state, zerocode_core::account::Provider::Anthropic);
    Ok(accounts_report(
        state.config_root(),
        state.local_data_root(),
    ))
}

#[tauri::command(async)]
pub(crate) fn remove_claude_account(
    state: State<'_, AppState>,
    id: String,
) -> Result<AccountsReport, String> {
    accounts::remove_account(state.config_root(), state.local_data_root(), &id)?;
    readiness_runtime::login_moved(Provider::Anthropic);
    Ok(accounts_report(
        state.config_root(),
        state.local_data_root(),
    ))
}

/* ---- this machine's Google login ----------------------------------------
 *
 * The third provider with a card in 설정 > AI 제공자 계정, and the only one
 * whose login this window makes ITSELF. Claude and Codex both have a CLI that
 * owns their browser round trip, so those cards spawn it and wait; Google has
 * no such CLI here — zo does its own `zo login google` — so the window runs
 * the OAuth flow ([`google_login`]) and writes the result into the file zo
 * reads. One login, shared: the Antigravity gauge and zo both use it.
 *
 * Two commands rather than one because the CONSENT SCREEN belongs on this
 * surface: `google_login_start` binds the loopback seat and hands back the
 * URL, the window opens it in its own browser tab, and
 * `google_login_finish` waits for the redirect. The secondary 「시스템
 * 브라우저에서 열기」 needs no command of its own — the window still holds
 * that URL and `open_url` already exists for exactly this.
 */

/// One sign-in in flight. `flow` is taken by the waiting road; the flag stays
/// so 취소 can reach a wait that has already started.
struct GoogleLoginAttempt {
    flow: Option<google_login::Flow>,
    cancelled: Arc<std::sync::atomic::AtomicBool>,
}

fn google_login_attempt() -> &'static Mutex<Option<GoogleLoginAttempt>> {
    static ATTEMPT: OnceLock<Mutex<Option<GoogleLoginAttempt>>> = OnceLock::new();
    ATTEMPT.get_or_init(|| Mutex::new(None))
}

/// Give up whatever attempt is in flight: the flag for a wait that has begun,
/// and the entry itself so a listener nobody is waiting on frees its port.
fn abandon_google_login() {
    if let Some(attempt) = google_login_attempt()
        .lock()
        .ok()
        .and_then(|mut held| held.take())
    {
        attempt
            .cancelled
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

/// What a finished sign-in leaves behind.
enum GoogleLoginOutcome {
    /// 취소 — not a failure, and it must not be dressed as one.
    Cancelled,
    /// Signed in, with the name the login belongs to when Google would say.
    SignedIn(Option<String>),
}

/// The card's whole reading: the store's word plus the name cached at
/// sign-in. No network — a card repaints on every settings open.
fn google_standing(state: &State<'_, AppState>) -> google_login::Standing {
    let email = load_settings_resilient(state.settings())
        .document
        .google_account_email;
    google_login::standing(
        google_login::credentials_path().as_deref(),
        Some(email),
        epoch_ms_now(),
    )
}

#[tauri::command(async)]
pub(crate) fn google_account(state: State<'_, AppState>) -> google_login::Standing {
    google_standing(&state)
}

/// Bind the loopback seat and say where the person consents.
///
/// A second press replaces the first attempt rather than failing beside it:
/// the seat is a FIXED port, so two live attempts cannot both hold it, and the
/// one a person just asked for is the one they meant.
#[tauri::command]
pub(crate) async fn google_login_start() -> Result<String, String> {
    abandon_google_login();
    // The bind waits, briefly, for a seat an abandoned attempt is still
    // holding — so it belongs on the blocking pool rather than on the thread
    // answering commands.
    let flow = tauri::async_runtime::spawn_blocking(google_login::Flow::open)
        .await
        .map_err(|error| error.to_string())??;
    let url = flow.consent_url();
    *google_login_attempt()
        .lock()
        .map_err(|_| "로그인 상태를 읽지 못했습니다")? = Some(GoogleLoginAttempt {
        flow: Some(flow),
        cancelled: Arc::new(std::sync::atomic::AtomicBool::new(false)),
    });
    Ok(url)
}

/// Wait for Google's redirect, save the login, and read who arrived.
///
/// Long — a person is at a consent screen — so the wait rides the blocking
/// pool with its own five-minute ceiling inside. The name is asked for HERE,
/// once, and cached in this window's settings: the card paints on every
/// settings open and a network round trip per paint is not a name worth
/// having.
#[tauri::command]
pub(crate) async fn google_login_finish(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
) -> Result<google_login::Standing, String> {
    let (flow, cancelled) = {
        let mut held = google_login_attempt()
            .lock()
            .map_err(|_| "로그인 상태를 읽지 못했습니다")?;
        let attempt = held.as_mut().ok_or("진행 중인 Google 로그인이 없습니다")?;
        let flow = attempt
            .flow
            .take()
            .ok_or("이미 기다리고 있는 로그인입니다")?;
        (flow, Arc::clone(&attempt.cancelled))
    };
    let path = google_login::credentials_path().ok_or("홈 폴더를 찾지 못했습니다")?;
    let now = epoch_ms_now();
    let outcome = tauri::async_runtime::spawn_blocking(move || {
        let Some(held) = flow.wait(&cancelled, now)? else {
            return Ok(GoogleLoginOutcome::Cancelled);
        };
        google_login::save(&path, &held)?;
        let name = google_login::usable_token(&held, now)
            .and_then(|token| google_login::email_of(&token, now));
        Ok::<_, String>(GoogleLoginOutcome::SignedIn(name))
    })
    .await
    .map_err(|error| error.to_string());
    abandon_google_login();
    match outcome? {
        // Nothing was written, so nothing is forgotten either — the previous
        // login (if any) is still exactly as it was.
        Ok(GoogleLoginOutcome::Cancelled) => {}
        Ok(GoogleLoginOutcome::SignedIn(name)) => {
            let name = name.unwrap_or_default();
            announce_google_login(&state, Some(name.as_str()));
            remember_google_account(&app, &webview, &state, name)?;
        }
        Err(error) => return Err(error),
    }
    Ok(google_standing(&state))
}

/// Give up a sign-in the person no longer wants. Answers even when nothing is
/// in flight: the button is pressed against a screen, not against a lock.
#[tauri::command(async)]
pub(crate) fn cancel_google_login() {
    abandon_google_login();
}

/// Take this machine's Google login off file.
///
/// Only this one key goes: zo keeps its Anthropic and OpenAI logins, its
/// adapter keys and its MCP tokens in the same file, and a logout here must
/// not be a logout there. The cached name goes with it — a name with no login
/// behind it is a card claiming somebody is signed in.
#[tauri::command]
pub(crate) async fn google_logout(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
) -> Result<google_login::Standing, String> {
    abandon_google_login();
    let path = google_login::credentials_path().ok_or("홈 폴더를 찾지 못했습니다")?;
    tauri::async_runtime::spawn_blocking(move || google_login::clear(&path))
        .await
        .map_err(|error| error.to_string())??;
    remember_google_account(&app, &webview, &state, String::new())?;
    Ok(google_standing(&state))
}

/// Cache — or forget — the name this login belongs to.
fn remember_google_account(
    app: &AppHandle,
    webview: &tauri::Webview,
    state: &State<'_, AppState>,
    email: String,
) -> Result<(), String> {
    commit_setting(
        app,
        webview,
        state,
        &[setting_key::GOOGLE_ACCOUNT_EMAIL],
        |settings| {
            settings.google_account_email = email;
            Ok(())
        },
    )
    .map(|_| ())
}
