//! Board commands.

use crate::conversation_wake::{self, ConversationWake};
use crate::*;
use zerocode_core::capabilities::SpawnRoad;

#[tauri::command(async)]
pub(crate) fn hooks_report(state: State<'_, AppState>) -> HooksReport {
    hooks_report_of(&state)
}

/// Turn managed hooks on or off, and make the agents' own configs agree before
/// answering — so the report the window paints is what is on disk, not what was
/// asked for.
///
/// Rides the blocking pool: this walks `PATH` for the agent scan and rewrites up
/// to ten settings files.
#[tauri::command]
pub(crate) async fn set_hooks_enabled(
    state: State<'_, AppState>,
    enabled: bool,
) -> Result<HooksReport, String> {
    let config = state.config_root().to_path_buf();
    let local = state.local_data_root().to_path_buf();
    tauri::async_runtime::spawn_blocking(move || {
        hooks::set_hooks_enabled(&config, enabled)?;
        let installed = installed_agent_slugs();
        hooks::reconcile(&config, &local, &installed);
        Ok::<_, String>(())
    })
    .await
    .map_err(|error| error.to_string())??;
    Ok(hooks_report_of(&state))
}

/// The project's orchestration accuracy report, read by exec-ing the installed
/// zo (`~/.local/bin/zo --orchestration-accuracy` in that project) so this
/// window neither links zo internals nor re-derives its project state slug.
/// Returns the one JSON object zo prints; a missing binary or a non-zero exit
/// is an error the board card shows as "no evidence" — never a fabricated
/// report.
#[tauri::command]
pub(crate) async fn orchestration_accuracy(path: String) -> Result<serde_json::Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let home = dirs::home_dir().ok_or_else(|| "no home directory".to_string())?;
        let bin = crate::zo_companion::zo_path_under(&home);
        if !bin.exists() {
            return Err(format!("zo not installed at {}", bin.display()));
        }
        let output = crate::proc::quiet_command(&bin)
            .arg("--orchestration-accuracy")
            .current_dir(&path)
            .output()
            .map_err(|error| error.to_string())?;
        if !output.status.success() {
            return Err(format!(
                "zo --orchestration-accuracy exited {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        serde_json::from_slice::<serde_json::Value>(&output.stdout)
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

/// Install (or re-install) the managed hooks now.
///
/// Orca reconciles on every startup and on every settings change; this window
/// does the startup pass in the background and offers this for the case a person
/// installed an agent CLI while the window was up — the same reason the agent
/// list has a re-scan.
#[tauri::command]
pub(crate) async fn install_hooks(state: State<'_, AppState>) -> Result<HooksReport, String> {
    let config = state.config_root().to_path_buf();
    let local = state.local_data_root().to_path_buf();
    tauri::async_runtime::spawn_blocking(move || {
        let installed = installed_agent_slugs();
        hooks::reconcile(&config, &local, &installed);
    })
    .await
    .map_err(|error| error.to_string())?;
    Ok(hooks_report_of(&state))
}

/// Say whether silence belongs to startup, the hook channel, or a dead pane.
/// A direct marker outranks an older report; a newer report outranks the
/// actor's one-beat-old durable failure. Terminal liveness outranks both.
pub(crate) fn pane_hearing(
    living: bool,
    report_at: Option<i64>,
    ledger: Option<&crate::orchestration::LedgerPaneState>,
    direct_failure: Option<i64>,
    bridge_listening: bool,
    now_ms: i64,
) -> (&'static str, i64) {
    if !living {
        return ("gone", report_at.unwrap_or(0));
    }
    let durable_failure = ledger.and_then(|one| one.hook_unreachable_since_ms);
    let failure = direct_failure.or(durable_failure).filter(|&failed_at| {
        report_at.is_none_or(|report| report < failed_at) || direct_failure.is_some()
    });
    let readiness_expired = ledger
        .and_then(|one| one.ready_by_ms)
        .filter(|&ready_by| ready_by <= now_ms && report_at.is_none());
    if let Some(failed_at) = failure.or(readiness_expired) {
        return ("unreachable", failed_at);
    }
    if let Some(report_at) = report_at {
        return ("heard", report_at);
    }
    if let Some(worker) = ledger {
        // A hand on the pane also retires readiness, and legacy rows predate
        // the window entirely. Without the pane's own report, neither is proof
        // the hook channel spoke.
        return ("pending", worker.started_ms);
    }
    match bridge_listening {
        true => ("pending", 0),
        false => ("unreachable", 0),
    }
}

/// Every pane this window knows holds an agent, with its last reported state.
///
/// The board's data half. Deliberately does NOT filter to "interesting" panes:
/// an agent sitting idle is a card in the idle column, and deciding here which
/// panes deserve to exist would take that decision away from the column rules
/// that are tested (`zerocode_core::board`).
#[tauri::command]
pub(crate) fn pane_agents(state: State<'_, AppState>) -> Vec<PaneAgent> {
    let _crumb = crate::crumbs::Command::enter("pane_agents");
    let now_ms = epoch_ms_now();
    let states = state.pane_states().clone();
    let sessions = state.pane_sessions().clone();
    // Who started whom. Reported as recorded, with no attempt to check that
    // the parent is still open: a pane whose parent has closed is an ORPHAN,
    // and `zerocode_core::agent_lineage` promotes an orphan to a root rather
    // than dropping it. Filtering here would be a second, untested copy of
    // that judgement — and the copy that runs first.
    let parents = state.pane_parents().clone();
    // Which panes still have a live shell behind them. An agent that never
    // reported a hook used to default to "idle" — the one bucket the board
    // hides — so a vendor with no hook integration was running invisibly,
    // which is the exact situation the board exists to show. A live pty we
    // launched as an agent is doing SOMETHING; only a dead one may rest.
    // Collected before the `agent_terms` lock so the two guards never overlap.
    let living: std::collections::HashSet<TermId> = state.terminals().terms().into_iter().collect();
    // What the LEDGER says about the seats it summoned, gathered once for the
    // whole paint rather than per pane. A pane the ledger never seated is
    // absent from this map, and absence leaves the agent's own report standing.
    let seated = crate::orchestration::ledger_states_by_term();
    let bridge_listening = hooks::bridge().is_some();
    let delivery_failures = hooks::delivery_failures_cached();
    let mut listed: Vec<PaneAgent> = state
        .agent_terms()
        .iter()
        .map(|(term, agent)| {
            let held = states.get(term);
            let ledger = seated.get(term);
            let direct_failure = delivery_failures.get(term).copied();
            let (hearing, hearing_at) = pane_hearing(
                living.contains(term),
                held.map(|one| one.at),
                ledger,
                direct_failure,
                bridge_listening,
                now_ms,
            );
            let mut listed = PaneAgent {
                term: *term,
                agent,
                state: held
                    .map(|one| one.state)
                    .and_then(|one| serde_json::to_value(one).ok())
                    .and_then(|one| one.as_str().map(str::to_string))
                    .unwrap_or_else(|| {
                        if living.contains(term) {
                            "working"
                        } else {
                            "idle"
                        }
                        .to_string()
                    }),
                ledger: ledger.map(|one| one.ledger.clone()).unwrap_or_default(),
                hearing,
                hearing_at,
                at: held.map_or(0, |one| one.at),
                state_started_at: held.map_or(0, |one| one.state_started_at),
                you: held.and_then(|one| one.you.clone()),
                said: held.and_then(|one| one.said.clone()),
                ask: held.and_then(|one| one.ask.clone()),
                ask_prompt: held.and_then(|one| one.ask_prompt.clone()),
                approval: held.and_then(|one| one.approval.clone()),
                autonomy: held.and_then(|one| one.autonomy.clone()),
                parent: parents.get(term).copied(),
                // The same question `pane_sessions` answers, asked through the
                // same table: whether the vendor offers a way back into this
                // conversation. An offer that fails is worse than no offer.
                resumable: zerocode_core::AgentKind::from_slug(agent).is_some_and(|kind| {
                    sessions
                        .get(term)
                        .is_some_and(|session| zerocode_core::resume_argv(kind, session).is_some())
                }),
            };
            // The liveness default above answers a pane with no hook fact —
            // a live pty we launched is doing SOMETHING — unless the ledger
            // has closed that seat's dispatch, in which case the work ended
            // and the ledger wrote down when. That is a stored epoch, so the
            // navigator's snapshot fence takes it where it refuses the `at: 0`
            // liveness guess; a hook fact, when there is one, still comes
            // first, and a newer hook still wins later. Without this, an
            // agent that speaks no hooks (zo) finished its slice, filed
            // `worker_done`, and sat on the board as "working" for an hour.
            if held.is_none()
                && let Some(ended_ms) = ledger.and_then(|one| one.work_ended_ms)
            {
                listed.state = "done".to_string();
                listed.at = ended_ms;
                listed.state_started_at = ended_ms;
            }
            listed
        })
        .collect();
    // A stable order out of a hash map, so the board's own tie-break has
    // something that does not change between calls to break ties with.
    listed.sort_by_key(|one| one.term);
    listed
}

/// Every worker the ORCHESTRATION LEDGER still holds, seat or no seat.
///
/// The board's third source, and the one that keeps a coordinator from losing
/// an agent it summoned. [`pane_agents`] above can only answer for terminals
/// this window has open, and the supervised lanes beside it are the same kind
/// of answer — so a worker whose seat this window does not hold was on no
/// surface at all, while `worker-list` printed it perfectly well.
///
/// Deliberately NOT filtered to the seatless ones. Which rows the board keeps
/// is the window's merge — it is the half that knows which panes actually
/// became cards — and answering "here is what the ledger holds, and here is
/// the seat I found for each" leaves that decision in one place instead of
/// splitting it across the wire.
///
/// Reads the board snapshot published by the existing standing-order beat.
/// No actor request, revision rebuild or pane-table walk runs on this thread.
#[tauri::command]
pub(crate) fn ledger_agents() -> std::sync::Arc<Vec<crate::orchestration::LedgerAgent>> {
    let _crumb = crate::crumbs::Command::enter("ledger_agents");
    std::sync::Arc::clone(&crate::orchestration::board_ledger_snapshot().agents)
}

/// The task board's coordinator desk (t-6588): the runs in play and their
/// tasks by pipeline stage, as the standing-order beat last published them
/// — the same board snapshot `ledger_agents` reads, so no actor request,
/// revision rebuild or pane-table walk runs on this thread.
#[tauri::command]
pub(crate) fn board_desk() -> std::sync::Arc<crate::orchestration::desk::DeskSnapshot> {
    let _crumb = crate::crumbs::Command::enter("board_desk");
    std::sync::Arc::clone(&crate::orchestration::board_ledger_snapshot().desk)
}

/// Answer, from the task board, a question put to a run's coordinator —
/// as that seat, which this window must hold (t-6588, `desk::reply`). The
/// main window only: a popped-out board shows the letter and leaves the
/// answering to the window that holds the seat.
#[tauri::command]
pub(crate) async fn desk_reply(
    webview: tauri::Webview,
    run: String,
    message: String,
    body: String,
    retry_request: String,
) -> Result<serde_json::Value, String> {
    crate::from_the_main_webview(&webview)?;
    tauri::async_runtime::spawn_blocking(move || {
        crate::orchestration::desk::reply(
            &run,
            &message,
            &body,
            &retry_request,
            crate::now_epoch_ms(),
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

/// Acknowledge, from the task board, the batch a run's coordinator holds —
/// the whole batch, as that seat (t-6588, `desk::acknowledge`).
#[tauri::command]
pub(crate) async fn desk_ack(
    webview: tauri::Webview,
    run: String,
    delivery: String,
    retry_request: String,
) -> Result<serde_json::Value, String> {
    crate::from_the_main_webview(&webview)?;
    tauri::async_runtime::spawn_blocking(move || {
        crate::orchestration::desk::acknowledge(
            &run,
            &delivery,
            &retry_request,
            crate::now_epoch_ms(),
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

/// The task board's worker roster's git facts (t-6588): commits past the
/// base and changed files for each worker checkout this window catalogues
/// (`desk_checkout_facts`). git is a process, so the blocking pool.
#[tauri::command]
pub(crate) async fn desk_checkouts(
    state: State<'_, AppState>,
    paths: Vec<String>,
) -> Result<Vec<DeskCheckout>, String> {
    let here = state.active();
    let active_project = here
        .orchestrator
        .map_or_else(|| here.root.clone(), |open| open.repo_root().to_path_buf());
    let config_root = state.config_root().to_path_buf();
    tauri::async_runtime::spawn_blocking(move || {
        desk_checkout_facts(&config_root, &active_project, &paths)
    })
    .await
    .map_err(|error| error.to_string())
}

/// The task board's machine strip (t-6588): the ledger's volume with the
/// verdict the next `--worktree` summons would meet, the load against the
/// cores, and the booted simulators and emulators — what `df -g`, `uptime`
/// and `xcrun simctl list` were typed for. `simctl` and `adb` are processes,
/// so the answer is read on the blocking pool.
#[tauri::command]
pub(crate) async fn machine_load(
    state: State<'_, AppState>,
) -> Result<crate::orchestration::desk::MachineLoad, String> {
    let volume = state.local_data_root().to_path_buf();
    tauri::async_runtime::spawn_blocking(move || crate::orchestration::desk::machine_load(&volume))
        .await
        .map_err(|error| error.to_string())
}

/// Bounded recent active/pending runs for the manual coordinator picker.
#[tauri::command]
pub(crate) async fn coordinator_seat_runs(
    limit: Option<usize>,
) -> Result<serde_json::Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        crate::orchestration::coordinator_handover::recent_runs(limit)
    })
    .await
    .map_err(|error| error.to_string())?
}

/// A human explicitly gives the selected pane the coordinator seat.
#[tauri::command]
pub(crate) async fn claim_coordinator_seat(
    app: AppHandle,
    run_id: String,
    target_term: u32,
    expected_generation: u32,
    retry_request: String,
) -> Result<serde_json::Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let host = crate::agent_tools_runtime::TeamWindow { app };
        crate::orchestration::coordinator_handover::claim_seat(
            &host,
            &run_id,
            target_term,
            expected_generation,
            &retry_request,
            crate::now_epoch_ms(),
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

/// Read eligibility from the ledger and the window's live pane registry.
#[tauri::command]
pub(crate) async fn coordinator_handover_status(
    app: AppHandle,
    run_id: String,
) -> Result<serde_json::Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let host = crate::agent_tools_runtime::TeamWindow { app };
        crate::orchestration::coordinator_handover::status(&host, &run_id)
    })
    .await
    .map_err(|error| error.to_string())?
}

/// A human's explicit standing order. No capability or witness is renderer input.
#[tauri::command]
pub(crate) async fn set_coordinator_handover(
    app: AppHandle,
    run_id: String,
    expected_generation: u32,
    target_term: Option<u32>,
    retry_request: String,
) -> Result<serde_json::Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let host = crate::agent_tools_runtime::TeamWindow { app };
        crate::orchestration::coordinator_handover::set_policy(
            &host,
            &run_id,
            expected_generation,
            target_term,
            &retry_request,
            crate::now_epoch_ms(),
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

/// Every helper this window has been told about, by pane.
///
/// The seed for a window that opened after the helpers did — a popped-out
/// board, or a reload. Without it, a person who opens the board mid-run sees an
/// agent with five workers as an agent with none, until one of them happens to
/// stop. There is no polling behind this: it is asked once at boot, and
/// `hook:subagent` keeps it true from then on.
#[tauri::command]
pub(crate) fn pane_subagents(state: State<'_, AppState>) -> Vec<PaneSubagents> {
    let _crumb = crate::crumbs::Command::enter("pane_subagents");
    let mut listed: Vec<PaneSubagents> = state
        .subagents()
        .iter()
        .map(|(term, rows)| PaneSubagents {
            term: *term,
            rows: rows.clone(),
        })
        .collect();
    listed.sort_by_key(|one| one.term);
    listed
}

/// Everything this window has been told an agent did, by card.
///
/// The seed for a window that opened after the work started — a popped-out
/// board, or a reload — and the road back for a window that missed an emit:
/// the throttle in [`ActivityRing::due`] can leave the tail of a burst sitting
/// in the ring until the agent does something else, and this hands it over
/// without waiting for that.
///
/// `pane` narrows it to one card, which is what a surface drawing a single
/// agent wants; asked for nothing it answers for every card in one round trip,
/// which is what a board opening wants. Two callers, one door — a per-card
/// command called once per card at boot is the sixteen round trips this window
/// has already deleted once.
#[tauri::command]
pub(crate) fn pane_activities(
    state: State<'_, AppState>,
    pane: Option<String>,
) -> Vec<PaneActivities> {
    let _crumb = crate::crumbs::Command::enter("pane_activities");
    let held = state.activities();
    let mut listed: Vec<PaneActivities> = held
        .iter()
        .filter(|(card, _)| pane.as_ref().is_none_or(|one| one == *card))
        .map(|(card, ring)| PaneActivities {
            pane: card.clone(),
            activities: ring.held.iter().cloned().collect(),
        })
        .collect();
    // A stable order out of a hash map, for the reason `pane_agents` sorts:
    // an answer that reshuffles between calls is an answer a diff cannot read.
    listed.sort_by(|one, other| one.pane.cmp(&other.pane));
    listed
}

/// Group the board's cards into its columns.
///
/// The rules live in `zerocode_core::board` — which column a state means, the
/// column order, the sort inside a column, what search matches. The window
/// The dock badge, fed by the window with the SAME number the board's door
/// badge shows — the length of the attention column, counted once by
/// `board_columns`. This command draws it and adds nothing: a badge doing its
/// own arithmetic is a badge that can disagree with the list behind it.
///
/// Orca clamps the label at "99+" (out/main/index.js:104431-104434) — a dock
/// badge is a glance, and a five-digit number reads as decoration.
///
/// macOS draws a dock LABEL and can say "99+"; Windows and Linux take a
/// COUNT (taskbar overlay, launcher badge — `set_badge_label` does not exist
/// there), which clamps at the same number without the plus.
#[tauri::command(async)]
pub(crate) fn set_dock_badge(app: AppHandle, count: usize) -> Result<(), String> {
    let Some(window) = app.get_webview_window("main") else {
        return Ok(());
    };
    #[cfg(target_os = "macos")]
    let drawn = if count == 0 {
        window.set_badge_label(None)
    } else if count > DOCK_BADGE_CLAMP {
        window.set_badge_label(Some(format!("{DOCK_BADGE_CLAMP}+")))
    } else {
        window.set_badge_label(Some(count.to_string()))
    };
    #[cfg(not(target_os = "macos"))]
    let drawn = window.set_badge_count(
        (count > 0).then(|| i64::try_from(count.min(DOCK_BADGE_CLAMP)).unwrap_or(i64::MAX)),
    );
    drawn.map_err(|error| error.to_string())
}

/// Largest number the badge spells out; above it macOS says "99+" and the
/// count platforms stop at 99.
const DOCK_BADGE_CLAMP: usize = 99;

/// 보드를 별도의 OS 창으로 연다 — 이미 있으면 만들지 않고 앞으로 가져온다.
///
/// 제목은 창을 연 웹뷰가 들고 온다. 번역표는 `ui/shell.js`에 있고 Rust는
/// 그것을 모르므로, 여기서 문자열 하나를 지어내면 이 창만 언어를 어긴다.
///
/// 싣는 문서는 메인 창과 **같은** `index.html`이고, 표면만 질의 문자열로
/// 갈린다. 보드 카드를 그리는 두 번째 파일은 곧 서로 다르게 자라는 두
/// 보드이기 때문이다(Orca도 같은 `AgentKanbanBoard`를 양쪽에서 쓴다).
#[tauri::command(async)]
pub(crate) fn open_board_popout(
    app: AppHandle,
    state: State<'_, AppState>,
    title: String,
) -> Result<(), String> {
    if let Some(open) = app.get_webview_window(BOARD_POPOUT_LABEL) {
        // 최소화된 창에 포커스만 주면 아무 일도 일어나지 않는다.
        let _ = open.unminimize();
        let _ = open.set_title(&title);
        return open.set_focus().map_err(|error| error.to_string());
    }
    let bounds_file = state.local_data_root().join(artifact_file::POPOUT_BOUNDS);
    let restored = restorable_bounds(
        stored_window_bounds(&bounds_file),
        &monitor_rects(&app),
        POPOUT_MIN_SIZE,
    );
    let (default_width, default_height) = POPOUT_DEFAULT_SIZE;
    let (min_width, min_height) = POPOUT_MIN_SIZE;
    let mut builder = tauri::WebviewWindowBuilder::new(
        &app,
        BOARD_POPOUT_LABEL,
        tauri::WebviewUrl::App("index.html?surface=popout".into()),
    )
    // The `devtools` cargo feature exists for the BROWSER pane's inspector
    // button; with it on, every webview that does not say otherwise gets an
    // inspector plus the ⌘⌥I toggle — and this one holds IPC (1-g0 리뷰
    // 발견 3). The main window's own line lives in tauri.conf.json.
    .devtools(false)
    .title(title)
    .inner_size(
        restored.map_or(default_width, |bounds| bounds.width),
        restored.map_or(default_height, |bounds| bounds.height),
    )
    .min_inner_size(min_width, min_height);
    // 저장된 자리가 검증을 통과했을 때만. 통과하지 못한 값은 위치를 아예
    // 주지 않는 것과 같아야 하고, 그러면 OS가 자기 규칙대로 놓는다.
    if let Some(bounds) = restored {
        builder = builder.position(bounds.x, bounds.y);
    }
    // 프레임도 제목 표시줄도 손대지 않는다: 메인 창은 오버레이 크롬을 직접
    // 그리지만 팝아웃은 평범한 OS 창이어야 한다 — 닫기 버튼이 그 안에
    // 없으므로(Orca도 없다) OS 크롬이 유일한 문이다.
    let window = builder.build().map_err(|error| error.to_string())?;
    watch_window_bounds(&window, bounds_file, POPOUT_MIN_SIZE);
    forget_watched_when_closed(&app, &window);
    // 저장된 자리로 태어나는 창은 메인 창의 부팅과 같은 경주를 연다 — 같은
    // 방어를 같은 자리에서.
    keep_main_webview_fitted(&app, BOARD_POPOUT_LABEL);
    Ok(())
}

/// 팝아웃이 카드 하나를 열었다 — 그것은 메인 창도 본 것이다.
///
/// Orca의 `dashboardPopout:ackAgent` → `ui:ackDashboardAgent`. 보드의 내용은
/// 두 창이 각자 백엔드에서 읽으므로, 창을 건너는 것은 "봤다"는 사실 하나다.
#[tauri::command]
pub(crate) fn ack_board_agent(app: AppHandle, pane: String) -> Result<(), String> {
    let _crumb = crate::crumbs::Command::enter("ack_board_agent");
    app.emit_to("main", "board:ack", BoardPane { pane })
        .map_err(|error| error.to_string())
}

/// 팝아웃의 "워크스페이스 열기".
///
/// 무대는 메인 창에만 있으므로 그 창을 올리고, 어느 카드였는지 건넨다 —
/// Orca의 `safelyRevealWindow` + `ui:revealDashboardAgent`.
#[tauri::command]
pub(crate) fn reveal_board_agent(app: AppHandle, pane: String) -> Result<(), String> {
    let _crumb = crate::crumbs::Command::enter("reveal_board_agent");
    let Some(main) = app.get_webview_window("main") else {
        return Err("메인 창이 없습니다".to_string());
    };
    let _ = main.unminimize();
    let _ = main.set_focus();
    app.emit_to("main", "board:reveal", BoardPane { pane })
        .map_err(|error| error.to_string())
}

/// 이 창이 지금 읽고 있는 셸들 — 전부, 매번.
///
/// 창이 자기 **현재 집합 전체**를 말한다. 늘렸다 줄였다 하는 셈(refcount)이
/// 아니라 선언인 이유는 하나다: 셈은 한 번 어긋나면 영영 어긋나고, 그 어긋남의
/// 증상이 "터미널이 조용히 안 그려짐"이다. 같은 집합을 두 번 말하는 것은
/// 아무 일도 아니고(멱등), 창이 문을 부르기만 하면 언제나 진실이 복구된다.
///
/// 라벨은 창 자신이 들고 오지 않는다 — 백엔드가 이미 아는 사실을 웹뷰가 다시
/// 지어내면 두 철자가 생기고, 팝아웃이 자기를 `popout`이라 부르는 동안 창의
/// 라벨은 `board-popout`인 것 같은 어긋남이 바로 그것이다.
///
/// Declaring is the whole of subscribing. A shell this window starts reading
/// gets this window as a reader that is owed a snapshot, and one it stops
/// reading loses it — both under that shell's own lock and before the new set
/// is visible, so the window's first pull after this reply is paid its floor
/// (`zerocode_pty::readers`) and a shell it just hid holds no share for it.
/// Notices then come under a name this window's label is part of
/// ([`crate::pane_runtime::dirty_event_for`]).
#[tauri::command(async)]
pub(crate) fn set_watched_terms(
    webview: tauri::Webview,
    state: State<'_, AppState>,
    terms: Vec<TermId>,
) {
    // The declaration itself goes in the black box: the failure this door
    // exists to prevent is a terminal that silently stops painting, and the
    // ledger of who declared what, when, is the whole forensic record of it.
    let mut said: Vec<TermId> = terms.clone();
    said.sort_unstable();
    note_window_event(
        state.local_data_root(),
        &format!("watch {}: {said:?}", webview.label()),
    );
    let label = webview.label().to_string();
    let next: HashSet<TermId> = terms.into_iter().collect();
    // Held until the replacement is visible, so a pull or a pump round that
    // reads the declaration reads it together with the readers it implies.
    // Lock order: the declaration, then (one at a time) a terminal's lock —
    // nothing takes the declaration while holding a terminal.
    let mut watched = state.watched_terms();
    let previous = watched.get(&label).cloned().unwrap_or_default();
    for term in next.difference(&previous) {
        if let Some(held) = state.terminals().handle(*term) {
            let mut pty = lock_pty(&held);
            pty.terminal_mut().readers_mut().0.declare(&label);
        }
    }
    for term in previous.difference(&next) {
        if let Some(held) = state.terminals().handle(*term) {
            let mut pty = lock_pty(&held);
            pty.terminal_mut().readers_mut().0.leave(&label);
        }
    }
    watched.insert(label, next);
}

/// What one pull answers: the frames each shell owes this window, in order,
/// and the declared shells that are not there at all.
#[derive(Serialize, Default)]
pub(crate) struct PulledScreens {
    screens: Vec<PulledScreen>,
    missing: Vec<TermId>,
}

#[derive(Serialize)]
struct PulledScreen {
    term: TermId,
    frames: Vec<zerocode_pty::GridDelta>,
}

/// Everything this window's terminals owe it, in one answer (design rule 5).
///
/// The screen asks when it is ready — on a notice while it was quiet, then on
/// each display frame while frames keep coming — so a webview that paints
/// slower than the pump reads is handed one frame per paint, folded by the
/// grid, and never a queue of screens that are already stale.
///
/// **Bytes, not an event.** The answer is a `tauri::ipc::Response` body: JSON
/// the webview decodes as data, where an event's payload is spliced into
/// JavaScript SOURCE and evaluated. And it is an answer to a question, so
/// nothing is pushed at a page that did not ask.
///
/// Each shell is locked on its own, one at a time: the pull that pays a debt
/// takes the delta for the other readers first and the snapshot after, in one
/// lock (`zerocode_pty::FrameReaders::pull`). A declared shell that is not in
/// the pool is named in `missing` — for a board card, that is "no live
/// terminal".
///
/// `resync` is the window saying its last pull failed. The frames that pull
/// took are gone with it, and a model with a hole in it is wrong from then on;
/// so every screen it reads is owed whole again, and this pull pays.
#[tauri::command(async)]
pub(crate) fn term_pull(
    webview: tauri::Webview,
    state: State<'_, AppState>,
    resync: Option<bool>,
) -> tauri::ipc::Response {
    let label = webview.label();
    let resync = resync.unwrap_or(false);
    let mut terms: Vec<TermId> = state
        .watched_terms()
        .get(label)
        .map(|terms| terms.iter().copied().collect())
        .unwrap_or_default();
    terms.sort_unstable();
    let now = std::time::Instant::now();
    let mut answer = PulledScreens::default();
    for term in terms {
        let Some(held) = state.terminals().handle(term) else {
            answer.missing.push(term);
            continue;
        };
        let frames = {
            let mut pty = lock_pty(&held);
            let (readers, grid) = pty.terminal_mut().readers_mut();
            if resync {
                readers.declare(label);
            }
            readers.pull(label, grid, now)
        };
        if !frames.is_empty() {
            answer.screens.push(PulledScreen { term, frames });
        }
    }
    tauri::ipc::Response::new(serde_json::to_vec(&answer).unwrap_or_default())
}

/// 이 창이 지금 **미리보고** 있는 셸들 — 전부, 매번.
///
/// [`set_watched_terms`]와 글자 그대로 같은 계약이고, 그것이 요점이다: 선언은
/// 집합 전체를 통째로 교체하며, 라벨은 백엔드가 안다. 셈이 아니므로 한 번
/// 빗나가도 다음 선언이 진실을 복구한다.
///
/// 다른 것은 무엇을 받느냐이다. 이쪽은 몇 줄짜리 꼬리를 느린 박자로 받고,
/// 그래서 여덟 장을 동시에 들고 있을 수 있다 — 전속력 티어는 그럴 수 없고,
/// 그것이 보드가 얼어붙어 있던 이유다.
#[tauri::command(async)]
pub(crate) fn set_previewed_terms(
    window: tauri::Window,
    state: State<'_, AppState>,
    terms: Vec<TermId>,
) {
    // 같은 이유로 같은 장부에 적는다: "보고 있는데 안 그려지는 터미널"의
    // 법의학은 누가 언제 무엇을 선언했는가가 전부이고, 티어가 둘이면 그 기록도
    // 둘이어야 한다.
    let mut said: Vec<TermId> = terms.clone();
    said.sort_unstable();
    note_window_event(
        state.local_data_root(),
        &format!("preview {}: {said:?}", window.label()),
    );
    state
        .previewed_terms()
        .insert(window.label().to_string(), terms.into_iter().collect());
}

/// 한 셸의 화면 전체를, 지금 이 순간의 모습으로 — 읽기만.
///
/// 화면을 **그리는** 창은 이 문을 쓰지 않는다: 새로 읽기 시작한 창의 바닥은 그
/// 창의 첫 당김이 스냅샷 빚으로 받는다(`term_pull`). 이 문은 그리지 않는 쪽의
/// 것이다 — 스킬 설치 화면이 판의 글을 읽어 가는 것처럼. 그래서 **읽기
/// 전용**이다: 기준선을 옮기는 `take_snapshot`이었다면 그 판을 그리고 있는
/// 창의 다음 델타가 그 사이의 변경을 잃는다.
///
/// pty는 옮겨지지 않고 뷰어가 하나 더 붙을 뿐이므로 새 경로가 아니라 읽기
/// 하나이고, 떠 있지 않은 셸에 대한 `None`이 곧 "라이브 터미널 없음"의
/// 근거다.
#[tauri::command(async)]
pub(crate) fn term_snapshot(
    state: State<'_, AppState>,
    term: TermId,
) -> Option<zerocode_pty::GridDelta> {
    state
        .terminals()
        .handle(term)
        .map(|pty| lock_pty(&pty).terminal().grid().snapshot())
}

/// 끝난 중첩 실행의 화면 — 살아 있는 판을 그리는 바로 그 에뮬레이터로.
///
/// 페이지는 실행이 돌려준 바이트를 `<pre>`에 글자로 붙였고, 스스로를 그리는
/// 실행은 사람이 지켜본 화면이 아니라 잔해로 도착했다(`[2K`, `[1A`; 2026-08-21
/// 신고). 창에는 그 바이트를 읽을 것이 없다 — 이 창이 그리는 모든 화면은 여기서
/// 만든 프레임이다. 그래서 바이트가 이리로 와서 [`term_snapshot`]이 살아 있는
/// 셸에 대해 돌려주는 것과 **같은 델타**로 돌아간다.
///
/// `None`은 "이건 화면이 아니라 글이다"라는 답이다 — 그런 출력은 페이지가
/// 그대로 글로 보여 주는 것이 옳다.
///
/// pty도, 프로세스도 없다: [`zerocode_pty::drawn`]이 이 호출 동안만 사는 격자
/// 하나를 세운다.
#[tauri::command]
pub(crate) fn worker_screen(text: String, rows: u16, cols: u16) -> Option<zerocode_pty::GridDelta> {
    let _crumb = crate::crumbs::Command::enter("worker_screen");
    zerocode_pty::drawn(text.as_bytes(), rows as usize, cols as usize)
}

#[tauri::command(async)]
pub(crate) fn continuation_source(state: State<'_, AppState>, term: TermId) -> ContinuationSource {
    let agent = state.agent_terms().get(&term).copied();
    let transcript_path = state
        .pane_sessions()
        .get(&term)
        .and_then(|session| session.transcript_path.clone())
        .filter(|path| !path.trim().is_empty());
    // Captured only when no transcript exists — Orca's exact asymmetry
    // (`capturedText: transcriptPath ? "" : pane.serializeAddon.serialize(…)`).
    let captured = if transcript_path.is_some() {
        None
    } else {
        state
            .terminals()
            .handle(term)
            .map(|pty| continuation_capture(lock_pty(&pty).terminal().grid()))
            .filter(|text| !text.is_empty())
    };
    let states = state.pane_states();
    let held = states.get(&term);
    ContinuationSource {
        agent,
        transcript_path,
        captured,
        you: held.and_then(|one| one.you.clone()),
        said: held.and_then(|one| one.said.clone()),
    }
}

/// Group the board's cards into its columns.
///
/// The rules live in `zerocode_core::board` — which column a state means, the
/// column order, the sort inside a column, what search matches. The window
/// assembles the cards because that is where the tabs and the workspace names
/// are, and hands them here so no second copy of those rules exists in
/// JavaScript.
#[tauri::command]
pub(crate) fn board_columns(
    cards: Vec<zerocode_core::board::BoardCard>,
    query: zerocode_core::board::BoardQuery,
) -> Vec<zerocode_core::board::BoardColumn> {
    let _crumb = crate::crumbs::Command::enter("board_columns");
    // This process's clock, not the webview's: the decay rule compares against
    // the same stamps this process wrote, and two clocks make a card that is
    // stale on one side and fresh on the other.
    zerocode_core::board::columns(&cards, &query, epoch_ms_now())
}

/// Classify the graph and its closed-door attention badge on one clock.
///
/// The graph can be filtered while the badge must remain global. Returning
/// both answers from one core pass avoids gathering and classifying every
/// running agent twice on each hook paint beat.
#[tauri::command]
pub(crate) fn board_snapshot(
    cards: Vec<zerocode_core::board::BoardCard>,
    query: zerocode_core::board::BoardQuery,
) -> GraphBoardSnapshot {
    let _crumb = crate::crumbs::Command::enter("board_snapshot");
    GraphBoardSnapshot {
        board: zerocode_core::board::snapshot(&cards, &query, epoch_ms_now()),
        overlays: crate::orchestration::graph_overlay_snapshot(),
    }
}

/// The existing board contract flattened verbatim, plus relations that are
/// explicitly transient. Existing callers keep reading `columns`,
/// `attention_count` and `total_count` at the same paths.
#[derive(serde::Serialize)]
pub(crate) struct GraphBoardSnapshot {
    #[serde(flatten)]
    board: zerocode_core::board::BoardSnapshot,
    overlays: std::sync::Arc<crate::orchestration::GraphOverlaySnapshot>,
}

#[tauri::command]
pub(crate) fn pane_sessions(state: State<'_, AppState>) -> Vec<PaneSession> {
    let _crumb = crate::crumbs::Command::enter("pane_sessions");
    let agents = state.agent_terms().clone();
    let mut listed: Vec<PaneSession> = state
        .pane_sessions()
        .iter()
        .filter_map(|(&term, session)| {
            let slug = agents.get(&term).copied()?;
            let kind = zerocode_core::AgentKind::from_slug(slug)?;
            Some(PaneSession {
                term,
                agent: slug,
                resumable: zerocode_core::resume_argv(kind, session).is_some(),
                session: session.clone(),
            })
        })
        .collect();
    // Stable order, because this list is drawn: a map's iteration order would
    // shuffle the rows between two paints.
    listed.sort_by_key(|one| one.term);
    listed
}

/// One fully assembled provider-resume command.
///
/// Private to the backend: the Tauri wire never accepts cwd or raw argv. A
/// worker reseat and the interactive resume command call this same builder;
/// only the worker supplies launch tuning carried by its durable row.
pub(crate) struct ResumeCommand {
    pub(crate) kind: zerocode_core::AgentKind,
    pub(crate) argv: Vec<String>,
    pub(crate) plan: zerocode_core::LaunchPlan,
}

pub(crate) fn resume_command(
    agent: &str,
    session: &zerocode_core::ProviderSession,
    nudge: Option<&str>,
    tuning: &[String],
    launch_override: Option<&zerocode_core::LaunchOverride>,
) -> Result<ResumeCommand, String> {
    let kind = zerocode_core::AgentKind::from_slug(agent)
        .ok_or_else(|| format!("{agent}은(는) 이 창이 모르는 에이전트입니다"))?;
    // A pane that died mid-turn is continued, not merely reopened — where
    // the vendor has a spelling for that. Where it does not, the plain
    // resume below is the honest fallback, and the person speaks.
    let continuing =
        nudge.and_then(|said| zerocode_core::resume_argv_continuing(kind, session, said));
    let mut argv = match continuing {
        Some(argv) => argv,
        None => zerocode_core::resume_argv(kind, session)
            .ok_or("이 세션은 이 창이 다시 열 수 없습니다")?,
    };
    // A resumed agent is the same agent the launch button starts. Orca's
    // resume plan opens with the resolved BASE command — the default yolo
    // args plus the person's saved ones — and appends the selector to it
    // (`buildAgentResumeStartupPlan` → `buildAgentResumeLaunchCommand`).
    // Spawning the bare selector relaunched claude without
    // `--dangerously-skip-permissions`, so a resumed conversation stopped to
    // ask before every tool — the map's P0-2. The base is scrubbed of the
    // row's own resume selectors first (#12982): one authoritative selector
    // only, and an agent with no selectors gets its args back untouched.
    let plan = zerocode_core::launch_plan(kind.slug(), launch_override);
    let spec = agent_spec(kind.slug())
        .ok_or_else(|| format!("{agent}은(는) 이 창이 모르는 에이전트입니다"))?;
    let caps = spec.capabilities();
    let base_args = caps.resume.launch_args_without_selectors(&plan.args);
    // Stored worker model/effort words ride after launch defaults and before
    // the selector (and its optional positional nudge). The Tauri caller sends
    // an empty slice; a worker plan supplies only its durable tuning receipt.
    argv.splice(1..1, base_args.into_iter().chain(tuning.iter().cloned()));
    Ok(ResumeCommand { kind, argv, plan })
}

/// What the launch button starts for this agent, for a wake whose
/// conversation was never written: `launch_agent_tab`'s program and launch
/// plan, with no selector for the agent to fail on.
pub(crate) fn fresh_command(
    agent: &str,
    launch_override: Option<&zerocode_core::LaunchOverride>,
) -> Result<ResumeCommand, String> {
    let (Some(spec), Some(kind)) = (
        agent_spec(agent),
        zerocode_core::AgentKind::from_slug(agent),
    ) else {
        return Err(format!("{agent}은(는) 이 창이 모르는 에이전트입니다"));
    };
    let plan = zerocode_core::launch_plan(spec.id, launch_override);
    let mut argv: Vec<String> = spec.launch.split_whitespace().map(str::to_string).collect();
    argv.extend(plan.args.iter().cloned());
    Ok(ResumeCommand { kind, argv, plan })
}

/// Only a definite absence: a directory this process cannot read is not a
/// missing file.
pub(crate) fn transcript_absent(path: &str) -> bool {
    std::fs::metadata(path).is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
}

/// Six wire arguments, and they stay six: a `#[tauri::command]`'s payload
/// parameters ARE its wire shape (`launch_agent_tab` says the same), so
/// folding `interrupted` and `restore` into a struct would rename what every
/// door sends — a payload change to quiet a lint about a payload. `AppHandle`
/// and `State` are injected by Tauri and are not wire fields.
#[allow(clippy::too_many_arguments)]
#[tauri::command(async)]
pub(crate) fn resume_session(
    app: AppHandle,
    state: State<'_, AppState>,
    agent: String,
    session: zerocode_core::ProviderSession,
    rows: u16,
    cols: u16,
    interrupted: Option<bool>,
    // A restored leaf's closed-window screen, put back before the resumed
    // agent's first byte is parsed (`replay_stored_screen`). A sidebar row or
    // any other door that re-enters a conversation outside a restore sends
    // none.
    restore: Option<super::settings::StoredScreen>,
) -> Result<ConversationWake, String> {
    // One conversation, one process (`conversation_wake`). Asked before
    // anything of this wake happens — the rollout read, the nudge's git
    // status, the keychain write, the trust mark, the spawn — so a door that
    // finds the conversation already held, or already waking, costs none of
    // them and starts no second process on its transcript. The claim lives
    // until this function returns: past the spawn that failed, and past the
    // pane `pane_sessions` answers for from then on.
    let term = state.take_term_id();
    let _claim = match conversation_wake::claim_for_wake(&state, &agent, &session, term) {
        Ok(claim) => claim,
        Err(holder) => return Ok(ConversationWake::standing(holder)),
    };
    // The record's mark is the hook's word at the last persist. For a Codex
    // pane the rollout the record names knows better whether the turn was
    // cut (t-2874), so the wake asks it here, once, before the nudge is built
    // and before the receipt is armed — one verdict for both.
    let interrupted = restart_nudge_runtime::wake_interrupted(
        &agent,
        interrupted.unwrap_or(false),
        session.transcript_path.as_deref(),
    );
    let launch_override = stored_launch_override(state.settings(), &agent)?;
    // The words the wake carries are decided here, before the argv is built
    // (t-3058): whether a sleeper in the ledger is this very conversation
    // — the seat sentence rides only then — and where the checkout stands
    // by git's word. The seat itself moves below, once the pane exists.
    let root = state.active_root();
    let root_words = root.to_string_lossy().into_owned();
    let reseating = crate::orchestration::sleeper_awaiting(&root_words, &agent, &session.id);
    // A conversation that was never written down has nothing to re-enter
    // (`conversation_never_written`): the pane gets what the launch button
    // starts, and none of the resume's nudge, record, seat or receipt. A
    // ledger sleeper waiting on this id keeps the resume road; its seat is
    // the ledger's to settle.
    let fresh = reseating.is_none()
        && zerocode_core::AgentKind::from_slug(&agent).is_some_and(|kind| {
            zerocode_core::conversation_never_written(kind, &session, transcript_absent)
        });
    // Nothing was said, so nothing was cut.
    let interrupted = interrupted && !fresh;
    // What the restart cut under this worker's pane, read at the goodbye
    // (t-6428 ⑤): a wake whose turn had ended is still nudged when commands
    // it left running were cut, and the nudge names them — the same road,
    // the same receipt.
    let cut = reseating
        .as_deref()
        .map(|worker| {
            crate::orchestration::restart_census::take_cut(state.local_data_root(), worker)
        })
        .unwrap_or_default();
    let marked = interrupted || !cut.is_empty();
    let nudge = marked.then(|| {
        restart_nudge_runtime::resume_nudge(
            interrupted,
            reseating.is_some(),
            restart_nudge_runtime::worktree_state(
                &root,
                u64::try_from(now_epoch_ms() / 1_000).unwrap_or_default(),
            )
            .as_ref(),
            &cut,
        )
    });
    let ResumeCommand {
        kind,
        mut argv,
        plan,
    } = if fresh {
        fresh_command(&agent, launch_override.as_ref())?
    } else {
        resume_command(
            &agent,
            &session,
            nudge.as_deref(),
            &[],
            launch_override.as_ref(),
        )?
    };
    // The row's answers for the two decisions below that used to be spelled
    // by name: which trust menu to pre-answer, and how the pane is opened.
    let caps = agent_spec(kind.slug())
        .ok_or_else(|| format!("{agent}은(는) 이 창이 모르는 에이전트입니다"))?
        .capabilities();
    // A resumed claude is the same claude the launch button starts — Orca
    // builds its team plan for every direct claude command it spawns
    // (`buildClaudeAgentTeamsLaunchPlan` gates on `isDirectClaudeCommand`
    // alone). A window restart resumes its leaders, and a leader that split
    // panes yesterday and backgrounds its teammates the morning after reads
    // as broken ("소넷은 왜 안보이는거지"), not as off.
    let teams_mode = load_settings_for_boot(state.settings())
        .document
        .agent_teams_mode;
    let mode_args = zerocode_core::agent_teams::teammate_mode_args(&argv, teams_mode);
    if !mode_args.is_empty() {
        argv.splice(1..1, mode_args);
    }
    let (program, args) = argv
        .split_first()
        .map(|(program, args)| (program.clone(), args.to_vec()))
        .ok_or("실행할 명령이 없습니다")?;
    let launch_token = new_launch_token(term);
    // A resumed agent reports exactly like a launched one: same pane key, same
    // nonce. Without them the conversation would come back on screen and go
    // silent, which reads as a resume that did not work. The plan's own env
    // rides too, and FIRST, for the launch road's reason: the account is the
    // more specific answer and must be able to override it.
    let mut env = plan.env.clone();
    env.extend(hooks::pty_env(
        &hooks::pane_key_of(term),
        Some(&launch_token),
        &root,
        pty_path_in(&env),
    ));
    env.extend(account_env_for(state.config_root(), &agent)?);
    let (agent_env, _auth_launch_lock) =
        hooks::agent_launch_env_with_lock(state.local_data_root(), &agent);
    env.extend(agent_env);
    let team_env = agent_teams::open_team(
        state.local_data_root(),
        term,
        teams_mode,
        &pty_path_of(&env),
        &program,
    );
    env.extend(team_env.iter().cloned());
    // The same pre-mark the launch road makes, for the same menu: a resumed
    // agent in a workspace it has never trusted asks the same question, and
    // the resume's own paste would answer it (P0-8).
    if let Some(preset) = caps.trust_menu()
        && let Err(error) = agent_trust_presets::mark_workspace_trusted(preset, &root, &env)
    {
        eprintln!(
            "zerocode-shell: the {} trust preset was not written: {error}",
            kind.slug()
        );
    }
    // Zo resumes through the same pane-owned channel as a fresh IDE pane.
    // Replaying `zo --resume ID` in a generic PTY restores the conversation
    // but omits --events-bind and never republishes the private address, which
    // is exactly the channel-less restart this road exists to prevent.
    let spawned = if caps.spawn == SpawnRoad::SocketPane {
        let addr_file = std::env::temp_dir().join(format!(
            "zerocode-events-term-{term}-{}.addr",
            LaneId::random()
        ));
        state
            .supervisor()
            .ok_or_else(|| "`zo`가 PATH에 없습니다".to_string())
            .and_then(|supervisor| {
                crate::cmd::project::open_zo_pane_process(
                    supervisor,
                    &root,
                    Some(&session.id),
                    &addr_file,
                    &env,
                    &args,
                    rows,
                    cols,
                )
            })
            .map(|opened| {
                let crate::cmd::project::OpenedZoPane {
                    pty,
                    addr,
                    session_id,
                    token,
                    observation,
                } = opened;
                (pty, Some((addr, token, session_id, observation)))
            })
    } else {
        PtyLane::spawn(&program, &args, Some(&root), &env, rows, cols)
            .map(|pty| (pty.into(), None))
            .map_err(|error| error.to_string())
    };
    let (mut pty, zo_channel) = spawned.map_err(|error| {
        agent_teams::forget_term(term);
        note_window_event(
            state.local_data_root(),
            &format!("term {term} refused {program}: {error}"),
        );
        error
    })?;
    if let Some(screen) = &restore {
        super::settings::replay_stored_screen(state.config_root(), screen, pty.terminal_mut());
    }
    state.hold_terminal(term, pty);
    if !team_env.is_empty() {
        state.team_envs().insert(term, env.clone());
    }
    state.agent_terms().insert(term, kind.slug());
    state.launch_tokens().insert(term, launch_token);
    if fresh {
        // Not that conversation: the fresh agent's own SessionStart names
        // the one this pane now holds.
        note_window_event(
            state.local_data_root(),
            &restart_nudge_runtime::fresh_line(term, kind.slug(), &session.id),
        );
        state.cadence().wake();
        return Ok(ConversationWake::opened(term));
    }
    // The session is recorded straight away rather than waited for: this pane IS
    // that conversation by construction, and the agent's first event may be
    // minutes away.
    let session = if let Some((_, _, session_id, _)) = &zo_channel {
        // The running process is the authority. Normally this equals the
        // durable id offered above; keeping `session.info`'s answer also makes
        // aliases/fallbacks honest instead of filing the channel under an id
        // it did not open.
        zerocode_core::ProviderSession {
            key: zerocode_core::SessionKey::SessionId,
            id: session_id.clone(),
            transcript_path: session.transcript_path,
        }
    } else {
        session
    };
    let resumed_session_id = session.id.clone();
    state.pane_sessions().insert(term, session);
    // The witness (t-3058): this pane, this checkout, this agent, this
    // session. A sleeper that is this conversation is seated here as the
    // same worker, before the nudge that tells it so is armed.
    let _seated = crate::orchestration::pane_resumed(
        term,
        &root_words,
        kind.slug(),
        &resumed_session_id,
        now_epoch_ms(),
    );
    restart_nudge_runtime::register_wake(
        &app,
        term,
        kind.slug(),
        &resumed_session_id,
        marked,
        nudge.as_deref().unwrap_or_default(),
    );
    if let Some((addr, token, session_id, observation)) = zo_channel {
        crate::cmd::project::attach_zo_pane_channel(
            &app,
            state.inner(),
            ZoChannelOwner::Term(term),
            addr,
            token,
            session_id,
            Some(observation),
        );
    }
    state.cadence().wake();
    Ok(ConversationWake::opened(term))
}

/// One agent's real mark, as a `data:` URL the window can draw.
///
/// Rides the blocking pool: the first ask for a domain talks to the network,
/// and a list of agents paints its letters immediately and swaps each mark in
/// as it lands. See `icon.rs` for why the fetch is not in the webview.
#[tauri::command]
pub(crate) async fn agent_icon(
    state: State<'_, AppState>,
    domain: String,
) -> Result<Option<String>, String> {
    let cache = state.cache_root().to_path_buf();
    tauri::async_runtime::spawn_blocking(move || icon::agent_icon(&cache, &domain))
        .await
        .map_err(|error| error.to_string())
}

/// Mint the file Orca's "New Markdown" mints: `untitled.md` in the checkout
/// root, counting past taken names (store-BgJxB0hr.js:40487 — baseName
/// "untitled", 100 attempts, created EMPTY on disk and opened as an ordinary
/// file). `create_new` is the claim, so two windows racing the same name
/// cannot both win it; the minted path is remembered so the untouched-close
/// road below may take back exactly what this road handed out.
#[tauri::command(async)]
pub(crate) fn create_untitled_markdown(state: State<'_, AppState>) -> Result<String, String> {
    let root = state.active_root();
    for attempt in 1..=100 {
        let path = root.join(zerocode_core::project::untitled_markdown_name(attempt));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(_) => {
                let said = path.to_string_lossy().into_owned();
                state.untitled_markdowns().insert(said.clone());
                return Ok(said);
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.to_string()),
        }
    }
    Err("빈 이름이 남아 있지 않습니다".to_string())
}

/// An explicit save is a claim of ownership: the person said KEEP, even if
/// what they kept is empty — so the mint is released and the delete road
/// below can never touch this path again (gpt-sol review, finding 1: an
/// empty file somebody ⌘S'd must not be reclaimed by its close).
#[tauri::command]
pub(crate) fn release_untitled_markdown(state: State<'_, AppState>, path: String) {
    let _crumb = crate::crumbs::Command::enter("release_untitled_markdown");
    state.untitled_markdowns().remove(&path);
}

/// Take back an untitled markdown that was closed untouched — Orca's
/// `deleteUntouchedOnClose` (store-BgJxB0hr.js:40965): a file nobody typed
/// into was never wanted, and trying the door must cost no litter. Two
/// server-side guards, because a loopback webview's word alone must not
/// delete files: the path has to be one THIS window minted, and the file has
/// to still be empty — a byte in it means somebody meant it after all.
///
/// The emptiness check and the delete cannot be one syscall, so the gap
/// between them is closed with a RENAME (gpt-sol review, finding 2): the
/// claim is atomic, a file that turns out to hold bytes is renamed straight
/// back, and only a still-empty hostage is removed. The mint is consumed on
/// the paths that settle the question and KEPT on an I/O failure, so a
/// transient error does not orphan the file forever.
///
/// Answers whether the file was actually removed — `false` means it held
/// bytes and stays — because the window's reopen stack is decided by it.
#[tauri::command(async)]
pub(crate) fn delete_untitled_markdown(
    state: State<'_, AppState>,
    path: String,
) -> Result<bool, String> {
    if !state.untitled_markdowns().contains(&path) {
        return Err("이 창이 만든 무제 파일이 아닙니다".to_string());
    }
    let answer = reclaim_untitled_file(&path)?;
    // Consumed only once the question is SETTLED — removed, or kept because
    // it held bytes. An I/O failure leaves the mint standing so the close
    // can try again instead of orphaning the file forever.
    state.untitled_markdowns().remove(&path);
    Ok(answer)
}

/// 클립보드에서 온 이미지를 임시 파일로 앉힌다 — 터미널에 붙는 것은 그
/// 경로(글자)뿐이고, 실행되는 것은 없다(Orca의 "Image in clipboard ·
/// ctrl+v to paste" 계약). 파일명에 공백이 없어 따옴표도 필요 없다.
///
/// The picture arrives as the request's RAW body rather than as base64 in a
/// JSON field, and the difference is what a refusal costs. Base64 is a third
/// again in bytes and a second full copy to decode, and both were spent before
/// anybody asked how big the picture was: an oversized screenshot travelled as
/// a string half again its size, was decoded whole, and only then refused.
/// Here the size is read where the bytes already lie and the write borrows
/// them, so a picture past the ceiling costs nothing but the arrival, and one
/// under it is never copied at all.
///
/// The format rides a header because a raw body has no room for a second
/// field. An absent or unknown one is a PNG — the same reading the JSON road
/// gave an absent `kind`, and the format every clipboard on this platform
/// offers.
#[tauri::command(async)]
pub(crate) fn save_pasted_image(
    webview: tauri::Webview,
    request: tauri::ipc::Request<'_>,
) -> Result<String, String> {
    from_the_main_webview(&webview)?;
    // A JSON body here is a caller that did not get the memo, not a picture
    // to be salvaged: decoding one would put back the copy this road exists
    // to remove.
    let tauri::ipc::InvokeBody::Raw(raw) = request.body() else {
        return Err("붙여넣은 이미지는 원시 바이트로 보내야 합니다".to_string());
    };
    // Before the write, and before anything is copied. `seat_pasted_image`
    // asks again for the clipboard road, which has no request to ask of.
    if raw.len() > MAX_PASTED_IMAGE_BYTES {
        return Err("이미지가 너무 큽니다 (32MB 초과)".to_string());
    }
    let ext = match request
        .headers()
        .get("x-image-kind")
        .and_then(|kind| kind.to_str().ok())
    {
        Some("image/jpeg") => "jpg",
        Some("image/gif") => "gif",
        Some("image/webp") => "webp",
        Some("image/tiff") => "tiff",
        _ => "png",
    };
    seat_pasted_image(raw, ext)
}

/// A pasted image larger than this is refused rather than seated: the file
/// would only be read back by an agent whose own image limit is far lower.
const MAX_PASTED_IMAGE_BYTES: usize = 32 * 1024 * 1024;

/// Seat image bytes as a temp file and answer its path — the one writer
/// behind both roads a pasted picture takes into a terminal.
fn seat_pasted_image(raw: &[u8], ext: &str) -> Result<String, String> {
    if raw.is_empty() {
        return Err("클립보드의 이미지가 비어 있습니다".to_string());
    }
    if raw.len() > MAX_PASTED_IMAGE_BYTES {
        return Err("이미지가 너무 큽니다 (32MB 초과)".to_string());
    }
    let dir = std::env::temp_dir().join("zerocode-paste");
    std::fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
    let at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_millis();
    let path = dir.join(format!("paste-{at}.{ext}"));
    std::fs::write(&path, raw).map_err(|error| error.to_string())?;
    Ok(path.to_string_lossy().into_owned())
}

/// The picture on the clipboard, seated the way a pasted one is
/// ([`save_pasted_image`]), or `None` when the clipboard holds no picture.
///
/// The `paste` event road reads the image the webview was handed. The
/// keyboard road — ⌘V with the focus on the body, which is where a click or
/// a drag in a pane leaves it — asks the clipboard itself, and it asked for
/// text alone: a screenshot pasted there went nowhere ("복붙도 안돼",
/// 2026-09-03). This is that road's picture. macOS hands over the PNG the
/// screenshot tool wrote, or a TIFF re-encoded as PNG by the same bitmap rep
/// the browser snapshot uses; elsewhere the clipboard plugin offers raw
/// pixels and this crate carries no encoder, so those platforms keep the
/// event road only.
#[tauri::command(async)]
pub(crate) fn save_clipboard_image(webview: tauri::Webview) -> Result<Option<String>, String> {
    from_the_main_webview(&webview)?;
    match clipboard_png()? {
        Some(png) => seat_pasted_image(&png, "png").map(Some),
        None => Ok(None),
    }
}

/// Whether the clipboard holds a picture right now — what the composer's
/// '+' menu enables its 이미지 붙여넣기 row by (t-2993 §2.1). Nothing is
/// written to answer: the same two pasteboard types [`clipboard_png`] reads
/// are asked for by name, and the picture is seated only when the row is
/// chosen ([`save_clipboard_image`]). Elsewhere than macOS the clipboard road
/// has no picture (see [`clipboard_png`]), so the row stays closed there.
#[tauri::command(async)]
pub(crate) fn clipboard_has_image(webview: tauri::Webview) -> Result<bool, String> {
    from_the_main_webview(&webview)?;
    Ok(clipboard_holds_image())
}

#[cfg(target_os = "macos")]
fn clipboard_holds_image() -> bool {
    use objc2_app_kit::{NSPasteboard, NSPasteboardTypePNG, NSPasteboardTypeTIFF};
    use objc2_foundation::NSArray;
    // SAFETY: the two type names are AppKit's own constants (extern statics,
    // read the way `clipboard_png` reads them); the answer is a name copied
    // out, and nothing here outlives the call.
    let pasteboard = NSPasteboard::generalPasteboard();
    let wanted = unsafe { NSArray::from_slice(&[NSPasteboardTypePNG, NSPasteboardTypeTIFF]) };
    pasteboard.availableTypeFromArray(&wanted).is_some()
}

#[cfg(not(target_os = "macos"))]
fn clipboard_holds_image() -> bool {
    false
}

#[cfg(target_os = "macos")]
fn clipboard_png() -> Result<Option<Vec<u8>>, String> {
    use objc2_app_kit::{
        NSBitmapImageFileType, NSBitmapImageRep, NSPasteboard, NSPasteboardTypePNG,
        NSPasteboardTypeTIFF,
    };
    use objc2_foundation::NSDictionary;
    // SAFETY (below): the two type names are AppKit's own constants and the
    // reads copy the bytes out; nothing here outlives the call.
    let pasteboard = NSPasteboard::generalPasteboard();
    if let Some(png) = unsafe { pasteboard.dataForType(NSPasteboardTypePNG) } {
        return Ok(Some(png.to_vec()));
    }
    let Some(tiff) = (unsafe { pasteboard.dataForType(NSPasteboardTypeTIFF) }) else {
        return Ok(None);
    };
    NSBitmapImageRep::imageRepWithData(&tiff)
        .and_then(|rep| unsafe {
            rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new())
        })
        .map(|png| Some(png.to_vec()))
        .ok_or_else(|| "PNG 인코딩에 실패했습니다".to_string())
}

#[cfg(not(target_os = "macos"))]
fn clipboard_png() -> Result<Option<Vec<u8>>, String> {
    Ok(None)
}
