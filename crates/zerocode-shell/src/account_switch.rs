//! The Claude account switch the window makes by itself (t-7538): read every
//! managed account's own gauge, ask the core table which account the next
//! launch should run as, and — for a worker standing at its quota wall —
//! seat the SAME worker again on the new account with its conversation,
//! model, effort and ledger seat intact.
//!
//! Three roads, one policy. The person's picker (`select_claude_account`)
//! moves the default and touches no pane. The beat's proposal (`ask`) is a
//! [`SwitchPlan`] the window shows and the person accepts BY TOKEN — an
//! acceptance for a proposal that has since changed is refused, not
//! re-aimed. The beat's own move (`auto`) applies the same plan without
//! asking. In every case a working pane keeps the login it was started with:
//! the only pane that moves is one whose two wall witnesses the ledger has
//! written down, and it moves by the restore road a window restart already
//! uses — `worker_rested_for_account_switch`, close, `reseat_sleeping` — so
//! the worker id, dispatch, checkout and `--resume` session are the ledger's
//! own facts and not this file's.
//!
//! What never happens here: a credential is not read (the account store's
//! ids and the usage cache's numbers are the only inputs), an email is not
//! written (refusals name the account id), and a pane is not closed before
//! the ledger has agreed to rest its worker.

use super::*;
use crate::agent_teams::Host;
use zerocode_core::account_autoswitch::{
    AutoSwitchMode, CLAUDE_ACCOUNT_AUTOSWITCH, Decision, Fitness, Question, SwitchReason, decide,
    fitness_table,
};
use zerocode_core::orchestration::{AccountMove, AccountSwitchReceipt};

/// When the beat last moved the default, and to whom — the cooldown's clock.
/// Window memory: a restart ends the cooldown, which is the same answer the
/// gauges give (they are re-read at boot).
fn last_switch() -> &'static Mutex<Option<(i64, String)>> {
    static HELD: std::sync::OnceLock<Mutex<Option<(i64, String)>>> = std::sync::OnceLock::new();
    HELD.get_or_init(|| Mutex::new(None))
}

/// Accounts a switch failed to select recently, so the next poll does not
/// pick the same failing candidate again (astra B2): bounded by the
/// cooldown, like the switch itself.
fn recent_failures() -> &'static Mutex<HashMap<String, i64>> {
    static HELD: std::sync::OnceLock<Mutex<HashMap<String, i64>>> = std::sync::OnceLock::new();
    HELD.get_or_init(|| Mutex::new(HashMap::new()))
}

/// One walled pane as the plan lists it for the window.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct WalledRow {
    pub(crate) worker: String,
    pub(crate) term: u32,
    /// The account the pane runs as, when the window recorded one.
    pub(crate) account: Option<String>,
    pub(crate) dispatch: String,
    pub(crate) generation: Option<u32>,
}

/// What the beat would do right now, and everything the window needs to
/// show or to apply it: the table's rows, the decision, the walled panes
/// that would move, and a token that names exactly this situation.
#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct SwitchPlan {
    pub(crate) mode: AutoSwitchMode,
    pub(crate) active: Option<String>,
    pub(crate) decision: Decision,
    pub(crate) fitness: Vec<Fitness>,
    pub(crate) walled: Vec<WalledRow>,
    pub(crate) last_switch_ms: Option<i64>,
    /// Candidates the plan left out because a switch to them failed inside
    /// the cooldown.
    pub(crate) failed_recently: Vec<String>,
    /// Names this exact plan: mode, source, decision and the walled panes'
    /// seats and generations. `apply` re-plans and compares.
    pub(crate) token: String,
    pub(crate) now_ms: i64,
}

fn token_of(
    mode: AutoSwitchMode,
    active: Option<&str>,
    decision: &Decision,
    walled: &[WalledRow],
) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    mode.word().hash(&mut hasher);
    active.hash(&mut hasher);
    serde_json::to_string(decision)
        .unwrap_or_default()
        .hash(&mut hasher);
    for row in walled {
        row.worker.hash(&mut hasher);
        row.term.hash(&mut hasher);
        row.account.hash(&mut hasher);
        row.dispatch.hash(&mut hasher);
        row.generation.hash(&mut hasher);
    }
    format!("{:016x}", hasher.finish())
}

/// The plan, read off the caches and the ledger — nothing is asked of a
/// provider here. `mode` is read fresh from the settings every time, so a
/// person who turned the switch off between two beats is heard.
pub(crate) fn plan(state: &AppState, mode: AutoSwitchMode, now_ms: i64) -> SwitchPlan {
    let config_root = state.config_root();
    let local_data_root = state.local_data_root();
    let active = active_claude_account_id(config_root);
    let mut gauges = claude_account_gauges(config_root, local_data_root);
    let failed_recently: Vec<String> = recent_failures()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .filter(|(_, at)| now_ms.saturating_sub(**at) < CLAUDE_ACCOUNT_AUTOSWITCH.cooldown_ms)
        .map(|(id, _)| id.clone())
        .collect();
    for gauge in &mut gauges {
        if failed_recently.contains(&gauge.id) {
            gauge.status = "error".to_string();
        }
    }
    let walled: Vec<WalledRow> = crate::orchestration::walled_claude_workers(now_ms)
        .into_iter()
        .map(|one| WalledRow {
            account: state.pane_accounts().get(&one.term).cloned(),
            worker: one.worker,
            term: one.term,
            dispatch: one.dispatch,
            generation: one.generation,
        })
        .collect();
    let last_switch_ms = last_switch()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_ref()
        .map(|(at, _)| *at);
    let decision = match (&active, mode.acts()) {
        (Some(source), true) => {
            // A pane at the wall is the second trigger — only one the window
            // can attribute to the source, or one it cannot attribute at all
            // while the source is the selected account (the gauge that
            // judged its wall was the selected account's).
            let walled_on_source = walled.iter().any(|row| {
                row.account.as_deref() == Some(source.as_str()) || row.account.is_none()
            });
            decide(&Question {
                source,
                gauges: &gauges,
                last_switch_ms,
                walled: walled_on_source,
                now_ms,
            })
        }
        (None, _) => Decision::Stay { why: "unknown" },
        (Some(_), false) => Decision::Stay { why: "off" },
    };
    let token = token_of(mode, active.as_deref(), &decision, &walled);
    SwitchPlan {
        mode,
        active,
        decision,
        fitness: fitness_table(&gauges, now_ms),
        walled,
        last_switch_ms,
        failed_recently,
        token,
        now_ms,
    }
}

/// One pane the switch moved, or tried to.
#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct MovedPane {
    pub(crate) worker: String,
    pub(crate) from_term: u32,
    pub(crate) to_term: Option<u32>,
    pub(crate) ok: bool,
    pub(crate) why: Option<String>,
    pub(crate) ms: u128,
}

/// What `apply` did — every effect it had and how long each took, for the
/// toast, the log and the report.
#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct Applied {
    pub(crate) from: Option<String>,
    pub(crate) to: String,
    pub(crate) switched_default: bool,
    pub(crate) default_ms: u128,
    pub(crate) panes: Vec<MovedPane>,
    pub(crate) receipts: usize,
    pub(crate) total_ms: u128,
}

/// The source's fullest window and percentage, for the receipt.
fn observed(plan: &SwitchPlan, source: &str) -> (Option<u8>, Option<String>) {
    let Some(row) = plan.fitness.iter().find(|fit| fit.id == source) else {
        return (None, None);
    };
    match &plan.decision {
        Decision::Switch {
            reason:
                SwitchReason::NearLimit {
                    window,
                    used_percent,
                },
            ..
        } => (Some(*used_percent), Some(window.clone())),
        _ => (row.room_percent.map(|room| 100 - room), None),
    }
}

/// Apply the plan the window was shown — and only that plan: the situation
/// is read again and, if anything the token names moved (the mode, the
/// selected account, the decision, a walled pane's seat or generation),
/// nothing happens and the caller is told to look again (astra B1).
///
/// `by` is the receipt's word for who decided: `auto` or `ask`. The
/// person's own picker never comes here.
///
/// `models` is what each pane's own hook last said it runs (the window's
/// `paneModels`): the row is brought up to it before the worker rests, so
/// the restore resumes on the model the person chose inside the pane, not
/// the one the summons named (astra B4). Effort is not in any hook, so the
/// row's word stands for it — see the report.
pub(crate) fn apply(
    app: &AppHandle,
    token: &str,
    by: &str,
    models: &HashMap<u32, String>,
) -> Result<Applied, String> {
    let state = app.state::<AppState>();
    let began = Instant::now();
    let now_ms = epoch_ms_now();
    let mode = load_settings(state.settings())?
        .document
        .claude_autoswitch_mode;
    let plan = plan(&state, mode, now_ms);
    if plan.token != token {
        return Err("상황이 바뀌어 전환하지 않았습니다 — 다시 확인하세요".to_string());
    }
    if !mode.acts() {
        return Err("자동 전환이 꺼져 있습니다".to_string());
    }
    let Decision::Switch { from, to, reason } = plan.decision.clone() else {
        return Err("지금은 바꿀 계정이 없습니다".to_string());
    };
    let config_root = state.config_root().to_path_buf();
    let local_data_root = state.local_data_root().to_path_buf();
    let reason_word = match &reason {
        SwitchReason::NearLimit { .. } => "near_limit",
        SwitchReason::Walled => "walled",
    };
    let (observed_percent, observed_window) = observed(&plan, &from);
    let receipt = |moved: AccountMove, key: String, panes_moved: u32| AccountSwitchReceipt {
        key,
        agent: "claude".to_string(),
        moved,
        from_account: Some(from.clone()),
        to_account: to.clone(),
        by: by.to_string(),
        reason: reason_word.to_string(),
        observed_percent,
        observed_window: observed_window.clone(),
        generation: plan.walled.first().and_then(|row| row.generation),
        panes_moved,
    };
    let mut receipts = 0;
    // ① The default, through the same verified switch the picker uses —
    // the probe, the materialization, the rollback on refusal — so the
    // next launch runs as `to` or nothing moved. A refusal is remembered
    // against `to`, and the next plan leaves it out for one cooldown.
    let default_began = Instant::now();
    let switched_default = plan.active.as_deref() != Some(to.as_str());
    if switched_default {
        let program = claude_program().ok_or("이 기계에서 claude를 찾지 못했습니다")?;
        if let Err(why) = accounts::select_account(&config_root, &program, &to) {
            recent_failures()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(to.clone(), now_ms);
            note_window_event(
                &local_data_root,
                &format!("account-switch: refused to select {to} ({by}, {reason_word}): {why}"),
            );
            return Err(why);
        }
        forget_claude_usage(&local_data_root);
        readiness_runtime::login_moved(zerocode_core::account::Provider::Anthropic);
        announce_account_switch(&state, zerocode_core::account::Provider::Anthropic);
        *last_switch()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some((now_ms, to.clone()));
        if crate::orchestration::record_account_switch(
            receipt(
                AccountMove::Default,
                format!("default-{from}-{to}-{now_ms}"),
                0,
            ),
            now_ms,
        )
        .unwrap_or(false)
        {
            receipts += 1;
        }
    }
    let default_ms = default_began.elapsed().as_millis();
    // ② Each walled pane on the source: rest → close → seat again. A pane
    // the ledger refuses to rest (the wall lifted, a person took it) is
    // left exactly as it is and named in the answer.
    let window = TeamWindow { app: app.clone() };
    let mut panes = Vec::new();
    for row in plan.walled.iter().filter(|row| {
        row.account.as_deref() == Some(from.as_str())
            || (row.account.is_none() && plan.active.as_deref() == Some(from.as_str()))
    }) {
        let pane_began = Instant::now();
        let mut moved = MovedPane {
            worker: row.worker.clone(),
            from_term: row.term,
            to_term: None,
            ok: false,
            why: None,
            ms: 0,
        };
        let seat = crate::orchestration::worker_seat_now(&row.worker);
        let Some((team, from_pane, Some(term), _)) = seat.clone() else {
            moved.why = Some("the worker's seat is not a pane of this window".to_string());
            moved.ms = pane_began.elapsed().as_millis();
            panes.push(moved);
            continue;
        };
        if term != row.term {
            moved.why = Some("the worker moved seats since the plan".to_string());
            moved.ms = pane_began.elapsed().as_millis();
            panes.push(moved);
            continue;
        }
        if let Some(model) = models.get(&row.term) {
            crate::orchestration::observe_worker_tuning(
                &row.worker,
                Some(model.as_str()),
                None,
                epoch_ms_now(),
            );
        }
        if let Err(why) = crate::orchestration::rest_worker_for_switch(&row.worker, epoch_ms_now())
        {
            moved.why = Some(why);
            moved.ms = pane_began.elapsed().as_millis();
            panes.push(moved);
            continue;
        }
        crate::orchestration::leave_switch_nudge(
            &row.worker,
            format!(
                "Your Claude account hit its usage limit, so this window moved this \
                 conversation to another of the person's accounts ({to}) and resumed it \
                 here — same task, same checkout, same worker seat. Continue exactly where \
                 you left off; if the work was already finished, say so briefly."
            ),
        );
        // The old pane goes only now: the ledger has the worker asleep, so
        // its exit settles nothing. The restore road then cuts the new pane
        // under the same coordinator and reseats the same worker.
        window.close(term);
        let Some(leader) = crate::orchestration::leader_term_of_team(&team) else {
            moved.why = Some("the worker's team has no leader pane to restore under".to_string());
            moved.ms = pane_began.elapsed().as_millis();
            panes.push(moved);
            continue;
        };
        let overrides = stored_launch_overrides(state.settings())
            .unwrap_or_default()
            .into_iter()
            .collect();
        let actor = receipt_actor_of(&state, leader);
        let restored =
            crate::orchestration::reseat_sleeping(&window, overrides, leader, actor.as_deref());
        let landed = crate::orchestration::worker_seat_now(&row.worker);
        moved.to_term = landed.as_ref().and_then(|(_, _, term, _)| *term);
        moved.ok = restored > 0
            && landed
                .as_ref()
                .is_some_and(|(_, _, term, state)| term.is_some() && state.is_live());
        if !moved.ok {
            moved.why = Some(
                "the worker is asleep with its attempt open; the coordinator's restore road \
                 seats it on its next beat"
                    .to_string(),
            );
        }
        moved.ms = pane_began.elapsed().as_millis();
        let to_pane = landed
            .as_ref()
            .map(|(_, pane, _, _)| pane.clone())
            .unwrap_or_default();
        if crate::orchestration::record_account_switch(
            receipt(
                AccountMove::Pane {
                    worker: row.worker.clone(),
                    from_pane,
                    to_pane,
                },
                format!("pane-{}-{}", row.dispatch, row.term),
                1,
            ),
            epoch_ms_now(),
        )
        .unwrap_or(false)
        {
            receipts += 1;
        }
        note_window_event(
            &local_data_root,
            &format!(
                "account-switch: pane {} worker {} {}→{} {}ms ok={}{}",
                row.term,
                row.worker,
                from,
                to,
                moved.ms,
                moved.ok,
                moved
                    .why
                    .as_deref()
                    .map(|why| format!(" ({why})"))
                    .unwrap_or_default()
            ),
        );
        panes.push(moved);
    }
    let applied = Applied {
        from: Some(from.clone()),
        to: to.clone(),
        switched_default,
        default_ms,
        panes,
        receipts,
        total_ms: began.elapsed().as_millis(),
    };
    note_window_event(
        &local_data_root,
        &format!(
            "account-switch: {by} {reason_word} {from}→{to} default={} ({}ms) panes={} receipts={} total={}ms",
            applied.switched_default,
            applied.default_ms,
            applied.panes.len(),
            applied.receipts,
            applied.total_ms
        ),
    );
    Ok(applied)
}

#[cfg(test)]
mod tests {
    use super::*;
    use zerocode_core::account_autoswitch::{AccountGauge, GaugeWindow};

    fn row(worker: &str, term: u32, account: Option<&str>) -> WalledRow {
        WalledRow {
            worker: worker.to_string(),
            term,
            account: account.map(str::to_string),
            dispatch: format!("dp-{worker}"),
            generation: Some(1),
        }
    }

    /// The token names the situation: the same facts give the same token,
    /// and any fact the apply road re-checks gives another — a late `yes`
    /// to a proposal that changed is refused by comparison alone.
    #[test]
    fn the_plans_token_moves_with_every_fact_the_apply_road_rechecks() {
        let decision = Decision::Switch {
            from: "a".into(),
            to: "b".into(),
            reason: SwitchReason::Walled,
        };
        let same = token_of(
            AutoSwitchMode::Ask,
            Some("a"),
            &decision,
            &[row("w-1", 7, Some("a"))],
        );
        assert_eq!(
            same,
            token_of(
                AutoSwitchMode::Ask,
                Some("a"),
                &decision,
                &[row("w-1", 7, Some("a"))]
            )
        );
        assert_ne!(
            same,
            token_of(
                AutoSwitchMode::Auto,
                Some("a"),
                &decision,
                &[row("w-1", 7, Some("a"))]
            )
        );
        assert_ne!(
            same,
            token_of(
                AutoSwitchMode::Ask,
                Some("b"),
                &decision,
                &[row("w-1", 7, Some("a"))]
            )
        );
        let other = Decision::Switch {
            from: "a".into(),
            to: "c".into(),
            reason: SwitchReason::Walled,
        };
        assert_ne!(
            same,
            token_of(
                AutoSwitchMode::Ask,
                Some("a"),
                &other,
                &[row("w-1", 7, Some("a"))]
            )
        );
        assert_ne!(
            same,
            token_of(
                AutoSwitchMode::Ask,
                Some("a"),
                &decision,
                &[row("w-1", 8, Some("a"))]
            )
        );
        assert_ne!(
            same,
            token_of(AutoSwitchMode::Ask, Some("a"), &decision, &[])
        );
        let mut regenerated = row("w-1", 7, Some("a"));
        regenerated.generation = Some(2);
        assert_ne!(
            same,
            token_of(AutoSwitchMode::Ask, Some("a"), &decision, &[regenerated])
        );
    }

    /// The quiet poll asks nobody: an inactive account whose own store holds
    /// no login is answered `unavailable` on the local road with no request
    /// made, and the next ambient ask inside the floor sends nothing at all
    /// (t-7538 measurement: requests per ask = 0 once read). The selected
    /// account is never read on this road — its own gauge reads it.
    #[test]
    fn a_quiet_poll_sends_nothing_and_an_account_without_a_login_asks_nobody() {
        let config = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let active_id = "t7538-quiet-active";
        let bare_id = "t7538-quiet-bare";
        let store_dir = config.path().join("stores");
        let account = |id: &str| zerocode_core::ClaudeAccount {
            id: id.to_string(),
            email: format!("{id}@example.test"),
            config_dir: store_dir.join(id).to_string_lossy().into_owned(),
            added_at: 1,
            ..zerocode_core::ClaudeAccount::default()
        };
        for id in [active_id, bare_id] {
            std::fs::create_dir_all(store_dir.join(id)).unwrap();
        }
        let store = accounts::AccountStore {
            accounts: vec![account(active_id), account(bare_id)],
            selection: zerocode_core::account::AccountSelection {
                active: Some(active_id.to_string()),
                system_default: false,
            },
        };
        std::fs::write(
            config.path().join(accounts::ACCOUNT_STORE_FILE),
            serde_json::to_string(&store).unwrap(),
        )
        .unwrap();
        let sent =
            refresh_inactive_claude_accounts(config.path(), data.path(), AccountPoll::Ambient);
        assert_eq!(sent, 1, "only the inactive account is read here");
        let began = Instant::now();
        while claude_account_scanning_now(bare_id) && began.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(10));
        }
        let landed = claude_account_usage_cache(data.path())
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(bare_id)
            .cloned()
            .expect("the bare account's reading landed");
        assert_eq!(landed.status, "unavailable");
        assert_eq!(landed.account.as_deref(), Some(bare_id));
        assert!(landed.session.is_none() && landed.weekly.is_none());
        assert!(
            !claude_account_usage_cache(data.path())
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .contains_key(active_id),
            "the selected account was read on the inactive road"
        );
        // Inside the ambient floor: nothing goes out, however often asked.
        for _ in 0..10 {
            assert_eq!(
                refresh_inactive_claude_accounts(config.path(), data.path(), AccountPoll::Ambient),
                0
            );
        }
        // The file on disk holds ids and figures only.
        let on_disk =
            std::fs::read_to_string(claude_account_usage_file(data.path())).unwrap_or_default();
        assert!(on_disk.contains(bare_id));
        assert!(
            !on_disk.contains("@example.test"),
            "an address reached the cache file"
        );
    }

    /// The gauge row the switch table reads is the account's own reading:
    /// windows and status, the id, the organisation word — and a snapshot
    /// with no windows is unknown.
    #[test]
    fn an_account_gauge_is_the_snapshots_windows_and_nothing_more() {
        let account = zerocode_core::ClaudeAccount {
            id: "a-fixture".into(),
            email: "somebody@example.test".into(),
            organization_type: Some("claude_team".into()),
            config_dir: "/nowhere/fixture".into(),
            added_at: 0,
            ..zerocode_core::ClaudeAccount::default()
        };
        let snapshot = usage::ProviderUsage {
            provider: "claude".into(),
            session: Some(usage::UsageWindow {
                used_percent: 40,
                window_minutes: 300,
                resets_at: Some(10),
                reset_description: None,
            }),
            weekly: Some(usage::UsageWindow {
                used_percent: 96,
                window_minutes: 10080,
                resets_at: Some(20),
                reset_description: None,
            }),
            fable_weekly: None,
            monthly: None,
            buckets: None,
            updated_at: 5,
            error: None,
            status: "ok".into(),
            failure_kind: None,
            retry_at_ms: None,
            plan_type: None,
            reset_credits: None,
            account: Some("a-fixture".into()),
        };
        let gauge: AccountGauge = account_gauge_of(&account, Some(&snapshot));
        assert_eq!(gauge.id, "a-fixture");
        assert_eq!(gauge.org_type.as_deref(), Some("claude_team"));
        assert_eq!(gauge.observed_at_ms, 5);
        assert_eq!(gauge.status, "ok");
        assert_eq!(
            gauge.windows,
            vec![
                GaugeWindow {
                    kind: "session".into(),
                    used_percent: 40,
                    resets_at_ms: Some(10)
                },
                GaugeWindow {
                    kind: "weekly".into(),
                    used_percent: 96,
                    resets_at_ms: Some(20)
                },
            ]
        );
        let json = serde_json::to_string(&gauge).unwrap();
        assert!(!json.contains('@'), "an address in a gauge row: {json}");
        let unknown = account_gauge_of(&account, None);
        assert!(unknown.windows.is_empty());
        assert_eq!(unknown.status, "unknown");
    }
}
