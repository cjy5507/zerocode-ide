//! The Claude account switch the window makes by itself (t-7538): read every
//! managed account's own gauge, ask the core table which account the next
//! launch should run as, and — for a worker standing at its quota wall —
//! seat the SAME worker again on the new account with its conversation,
//! model, effort and ledger seat intact.
//!
//! Three roads, one policy, one line. The person's picker
//! ([`switch_by_person`]) moves the default and touches no pane. The beat's
//! proposal (`ask`) is a [`SwitchPlan`] the window shows and the person
//! accepts BY TOKEN — an acceptance for a proposal that has since changed is
//! refused, not re-aimed. The beat's own move (`auto`) applies the same plan
//! without asking. All three hold [`SWITCH_LINE`], so two windows, or a press
//! and a beat, converge on one default move and one move per pane.
//!
//! In every case a working pane keeps the login it was started with: the
//! only pane that moves is one whose two wall witnesses the ledger has
//! written down, and it moves by the restore road a window restart already
//! uses — the ledger rests it, the pane closes and its process group is
//! gone, `reseat_one_in_line` opens the new pane with `--resume` — all in
//! the restore line, so neither another restore nor the grace can walk the
//! worker between its rest and its new pane.
//!
//! What never happens here: a credential is not read (the account store's
//! ids and the usage cache's numbers are the only inputs), an email is not
//! written (refusals name the account id), a pane is not closed before the
//! ledger has agreed to rest its worker, and a conversation is never started
//! empty in place of the one the pane had.

use super::*;
use crate::agent_teams::Host;
use zerocode_core::LaunchOverride;
use zerocode_core::account_autoswitch::{
    AccountGauge, AutoSwitchMode, CLAUDE_ACCOUNT_AUTOSWITCH, Decision, Fitness, Question,
    SwitchReason, best_candidate, decide, fitness_table, judge_pane,
};
use zerocode_core::orchestration::{AccountMove, AccountSwitchReceipt, WorkerState};

/// Every switch and every reading-back of a journal, one at a time.
static SWITCH_LINE: Mutex<()> = Mutex::new(());

/// When the window's default last moved, and to whom — the cooldown's
/// clock. Window memory: a restart ends the cooldown, which is the same
/// answer the gauges give (they are read again at boot).
fn last_switch() -> &'static Mutex<Option<(i64, String)>> {
    static HELD: std::sync::OnceLock<Mutex<Option<(i64, String)>>> = std::sync::OnceLock::new();
    HELD.get_or_init(|| Mutex::new(None))
}

/// Accounts a switch failed to select recently, so the next beat does not
/// pick the same failing candidate again (astra B2) — bounded by the
/// cooldown, like the switch itself.
fn recent_failures() -> &'static Mutex<HashMap<String, i64>> {
    static HELD: std::sync::OnceLock<Mutex<HashMap<String, i64>>> = std::sync::OnceLock::new();
    HELD.get_or_init(|| Mutex::new(HashMap::new()))
}

/// What a plan is made of, read by the caller — the window reads its own
/// settings, store, caches and clocks; a test hands its own.
pub(crate) struct Situation {
    pub(crate) mode: AutoSwitchMode,
    pub(crate) active: Option<String>,
    pub(crate) gauges: Vec<AccountGauge>,
    /// When the default last moved — the cooldown's clock.
    pub(crate) last_switch_ms: Option<i64>,
    /// Accounts a select refused, and when — left out for one cooldown.
    pub(crate) failures: Vec<(String, i64)>,
    pub(crate) now_ms: i64,
}

/// The doors a switch walks through: everything it does to the world
/// besides the ledger and the panes.
pub(crate) trait SwitchDoors {
    /// The mode, the selected account and every managed account's gauge,
    /// read now.
    fn situation(&self, now_ms: i64) -> Result<Situation, String>;
    /// Select `to` — `None` for the machine's own login — through the
    /// verified road: the probe, the materialization, the rollback on
    /// refusal. Either the next launch runs as `to`, or nothing moved.
    fn select(&self, to: Option<&str>) -> Result<(), String>;
    /// What follows a selection: the selected gauge forgotten, readiness
    /// told, the zo panes' logins reloaded (t-5777) — and the cooldown's
    /// clock set.
    fn selected(&self, to: Option<&str>, at_ms: i64);
    /// A select of `to` was refused: the next plans leave it out for one
    /// cooldown rather than pick the same failing account every beat.
    fn select_refused(&self, to: &str, at_ms: i64);
    /// The launch overrides a relaunch reads.
    fn overrides(&self) -> Vec<(String, LaunchOverride)>;
    /// Where the switch journal lives.
    fn journal_dir(&self) -> PathBuf;
    /// One line in the window's log.
    fn log(&self, line: &str);
}

/// The window's own doors.
pub(crate) struct WindowDoors<'a> {
    pub(crate) state: &'a AppState,
}

impl SwitchDoors for WindowDoors<'_> {
    fn situation(&self, now_ms: i64) -> Result<Situation, String> {
        let config_root = self.state.config_root();
        // A settings file that cannot be read says nothing about what the
        // person allowed, so the beat takes the answer that moves nothing —
        // and the person's own pick, which does not ask the setting, still
        // works.
        let mode = match load_settings(self.state.settings()) {
            Ok(loaded) => loaded.document.claude_autoswitch_mode,
            Err(why) => {
                self.log(&format!(
                    "account-switch: settings unreadable, the beat stands off: {why}"
                ));
                AutoSwitchMode::Off
            }
        };
        Ok(Situation {
            mode,
            active: active_claude_account_id(config_root),
            gauges: claude_account_gauges(config_root, self.state.local_data_root()),
            last_switch_ms: last_switch()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_ref()
                .map(|(at, _)| *at),
            failures: recent_failures()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .iter()
                .map(|(id, at)| (id.clone(), *at))
                .collect(),
            now_ms,
        })
    }

    fn select(&self, to: Option<&str>) -> Result<(), String> {
        let config_root = self.state.config_root();
        match to {
            Some(id) => {
                let program = claude_program().ok_or("이 기계에서 claude를 찾지 못했습니다")?;
                accounts::select_account(config_root, &program, id).map(|_| ())
            }
            None => accounts::use_system_default(config_root).map(|_| ()),
        }
    }

    fn selected(&self, to: Option<&str>, at_ms: i64) {
        forget_claude_usage(self.state.local_data_root());
        readiness_runtime::login_moved(zerocode_core::account::Provider::Anthropic);
        announce_account_switch(self.state, zerocode_core::account::Provider::Anthropic);
        *last_switch()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Some((at_ms, to.unwrap_or_default().to_string()));
    }

    fn select_refused(&self, to: &str, at_ms: i64) {
        recent_failures()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(to.to_string(), at_ms);
    }

    fn overrides(&self) -> Vec<(String, LaunchOverride)> {
        stored_launch_overrides(self.state.settings())
            .unwrap_or_default()
            .into_iter()
            .collect()
    }

    fn journal_dir(&self) -> PathBuf {
        self.state.local_data_root().to_path_buf()
    }

    fn log(&self, line: &str) {
        note_window_event(self.state.local_data_root(), line);
    }
}

/// The conversation a Claude pane is in and what it runs right now, read
/// off its own transcript — the authoritative current values, including a
/// person's `/model`, `/effort` and Shift+Tab inside the pane (astra B4),
/// which no launch argument remembers.
#[derive(Debug, Clone)]
pub(crate) struct PaneTuning {
    pub(crate) session: zerocode_core::ProviderSession,
    pub(crate) model: Option<String>,
    pub(crate) effort: Option<String>,
    pub(crate) permission_mode: Option<String>,
}

/// `None` for a pane with no conversation the window can read — which the
/// switch will not move, because a restore without one starts empty.
pub(crate) fn pane_tuning(host: &dyn Host, term: u32) -> Option<PaneTuning> {
    let session = host.provider_session(term)?;
    let path = session.transcript_path.clone()?;
    let lines = zerocode_core::transcript::tail_lines(Path::new(&path))?;
    let raw = lines.join("\n");
    Some(PaneTuning {
        model: zerocode_core::transcript::model_in(&raw),
        effort: zerocode_core::transcript::session_effort_in(&raw),
        permission_mode: zerocode_core::transcript::permission_mode_in(&raw),
        session,
    })
}

/// The model word the relaunch should carry: the row's own when the pane's
/// transcript names that same model (the row keeps the launch spelling — a
/// `[1m]` context the transcript's id does not show), the transcript's when
/// the person moved the pane to another one.
fn relaunch_model(row: Option<&str>, pane: Option<&str>) -> Option<String> {
    let pane = pane?;
    let same = row.is_some_and(|row| {
        let named = row
            .split('[')
            .next()
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        let id = pane.to_ascii_lowercase();
        !named.is_empty() && (id == named || id.split('-').any(|word| word == named))
    });
    (!same).then(|| pane.to_string())
}

/// Whether a relaunch through `overrides` starts the pane in the permission
/// mode it runs in now (t-7538, condition 6). Claude Code's own mode words
/// ([`zerocode_core::agent::agent_voice`]) against the launch plan's: an
/// unattended launch starts in `bypassPermissions`, an asking one in
/// `default`, and a launch somebody edited by hand says nothing a mode
/// could be compared with. A pane whose CLI never said is started as it was
/// summoned.
fn relaunch_keeps_permission(
    pane_mode: Option<&str>,
    overrides: &[(String, LaunchOverride)],
) -> Result<(), String> {
    let Some(pane_mode) = pane_mode else {
        return Ok(());
    };
    let launch = zerocode_core::launch::launch_plan(
        "claude",
        overrides
            .iter()
            .find(|(agent, _)| agent == "claude")
            .map(|(_, held)| held),
    );
    let starts_in = match launch.permission {
        zerocode_core::launch::PermissionMode::Unattended => "bypassPermissions",
        zerocode_core::launch::PermissionMode::Asks => "default",
        zerocode_core::launch::PermissionMode::Mixed => {
            return Err(format!(
                "the pane runs in `{pane_mode}` and a relaunch's hand-edited launch words name \
                 no mode to compare — not moved"
            ));
        }
    };
    let same = zerocode_core::agent::agent_voice("claude")
        .permission_modes
        .iter()
        .find(|mode| mode.mode == starts_in)
        .is_some_and(|mode| mode.mode == pane_mode || mode.aliases.contains(&pane_mode));
    if same {
        Ok(())
    } else {
        Err(format!(
            "the pane runs in `{pane_mode}` and a relaunch would start in `{starts_in}` — not moved"
        ))
    }
}

/// One walled pane as the plan lists it, with what the table made of it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct WalledRow {
    pub(crate) worker: String,
    pub(crate) term: u32,
    /// The account the pane runs as, when the window recorded one — and
    /// `None`, "unattributed", is never presumed the selected one.
    pub(crate) account: Option<String>,
    /// The model the pane runs, its transcript's word before the row's.
    pub(crate) model: Option<String>,
    pub(crate) dispatch: String,
    pub(crate) generation: Option<u32>,
    /// Where the table sends it: `switch` to the landing, `wait` for its
    /// reset, `stay` with a reason word.
    pub(crate) verdict: Decision,
}

/// What the beat would do right now, and everything the window needs to
/// show or to apply it: the table's rows, the default's decision, the
/// walled panes and where each goes, and a token that names exactly this.
#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct SwitchPlan {
    pub(crate) mode: AutoSwitchMode,
    pub(crate) active: Option<String>,
    pub(crate) decision: Decision,
    /// The account launches run as once the decision is made — where a
    /// moved pane lands.
    pub(crate) landing: Option<String>,
    pub(crate) fitness: Vec<Fitness>,
    /// The account the default would move to if it moved now — the status
    /// bar's "next", chosen by the table and nowhere else.
    pub(crate) next: Option<Fitness>,
    pub(crate) walled: Vec<WalledRow>,
    pub(crate) last_switch_ms: Option<i64>,
    /// When the cooldown after the last switch ends, while it runs.
    pub(crate) cooldown_until_ms: Option<i64>,
    /// Candidates left out because a switch to them failed inside the
    /// cooldown.
    pub(crate) failed_recently: Vec<String>,
    /// Names this exact plan: mode, source, decision and every walled
    /// pane's seat, generation and verdict. `apply` re-plans and compares.
    pub(crate) token: String,
    pub(crate) now_ms: i64,
}

impl SwitchPlan {
    /// The panes this plan moves.
    pub(crate) fn moves(&self) -> impl Iterator<Item = &WalledRow> {
        self.walled
            .iter()
            .filter(|row| matches!(row.verdict, Decision::Switch { .. }))
    }

    /// Whether applying it would do anything at all.
    pub(crate) fn acts(&self) -> bool {
        self.mode.acts()
            && (matches!(self.decision, Decision::Switch { .. }) || self.moves().next().is_some())
    }
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
        serde_json::to_string(&row.verdict)
            .unwrap_or_default()
            .hash(&mut hasher);
    }
    format!("{:016x}", hasher.finish())
}

/// The plan, read off the situation and the ledger — nothing is asked of a
/// provider here, and nothing moves.
pub(crate) fn plan_with(host: &dyn Host, situation: Situation) -> SwitchPlan {
    let Situation {
        mode,
        active,
        mut gauges,
        last_switch_ms,
        failures,
        now_ms,
    } = situation;
    let failed_recently: Vec<String> = failures
        .into_iter()
        .filter(|(_, at)| now_ms.saturating_sub(*at) < CLAUDE_ACCOUNT_AUTOSWITCH.cooldown_ms)
        .map(|(id, _)| id)
        .collect();
    for gauge in &mut gauges {
        if failed_recently.contains(&gauge.id) {
            gauge.status = "error".to_string();
        }
    }
    let cooldown_until_ms = last_switch_ms
        .map(|at| at.saturating_add(CLAUDE_ACCOUNT_AUTOSWITCH.cooldown_ms))
        .filter(|until| *until > now_ms);
    let mut walled: Vec<WalledRow> = crate::orchestration::walled_claude_workers(now_ms)
        .into_iter()
        .map(|one| WalledRow {
            account: host.pane_account(one.term),
            model: pane_tuning(host, one.term)
                .and_then(|tuning| tuning.model)
                .or(one.model),
            worker: one.worker,
            term: one.term,
            dispatch: one.dispatch,
            generation: one.generation,
            verdict: Decision::Stay { why: "off" },
        })
        .collect();
    let decision = match (&active, mode.acts()) {
        (_, false) => Decision::Stay { why: "off" },
        (None, true) => Decision::Stay { why: "unknown" },
        (Some(source), true) => decide(&Question {
            source,
            gauges: &gauges,
            model: None,
            last_switch_ms,
            // Only a pane the window can NAME the account of is a witness
            // against the source (astra A4).
            walled: walled
                .iter()
                .any(|row| row.account.as_deref() == Some(source.as_str())),
            now_ms,
        }),
    };
    let landing = match &decision {
        Decision::Switch { to, .. } => Some(to.clone()),
        _ => active.clone(),
    };
    if mode.acts() {
        for row in &mut walled {
            row.verdict = match (row.account.as_deref(), landing.as_deref()) {
                (None, _) => Decision::Stay {
                    why: "unattributed",
                },
                (Some(_), None) => Decision::Stay { why: "unknown" },
                (Some(account), Some(landing)) => {
                    judge_pane(account, row.model.as_deref(), landing, &gauges, now_ms)
                }
            };
        }
    }
    let token = token_of(mode, active.as_deref(), &decision, &walled);
    let next = active.as_deref().and_then(|source| {
        best_candidate(
            source,
            &gauges,
            None,
            CLAUDE_ACCOUNT_AUTOSWITCH.default_moves_at_percent,
            now_ms,
        )
    });
    SwitchPlan {
        mode,
        active,
        decision,
        landing,
        fitness: fitness_table(&gauges, now_ms),
        next,
        walled,
        last_switch_ms,
        cooldown_until_ms,
        failed_recently,
        token,
        now_ms,
    }
}

/* ---- the journal: a switch that dies halfway still gets its receipts ---- */

/// One switch in flight, written down before its first effect and taken
/// away after its last receipt (astra B2). A window that dies in between
/// reads it back and finishes the RECEIPTS — never the effects: the
/// selection either landed or it did not, and a rested worker is the
/// restore road's, which a restart walks anyway.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Journal {
    pub(crate) key: String,
    pub(crate) by: String,
    pub(crate) reason: String,
    pub(crate) from: Option<String>,
    pub(crate) to: Option<String>,
    /// Whether the default moves in this switch, and its receipt is owed.
    pub(crate) default: bool,
    pub(crate) observed_percent: Option<u8>,
    pub(crate) observed_window: Option<String>,
    pub(crate) generation: Option<u32>,
    pub(crate) panes: Vec<JournalPane>,
    pub(crate) began_ms: i64,
}

/// One pane the switch set out to move.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct JournalPane {
    pub(crate) worker: String,
    pub(crate) from_pane: String,
    pub(crate) from_account: String,
}

fn journal_file(dir: &Path) -> PathBuf {
    dir.join(app_paths::artifact_file::CLAUDE_ACCOUNT_SWITCH)
}

fn read_journal(dir: &Path) -> Option<Journal> {
    serde_json::from_str(&std::fs::read_to_string(journal_file(dir)).ok()?).ok()
}

fn write_journal(dir: &Path, journal: &Journal) -> Result<(), String> {
    let text = serde_json::to_string_pretty(journal).map_err(|why| why.to_string())?;
    durable_file::replace_bytes(&journal_file(dir), text.as_bytes())
        .map(|_| ())
        .map_err(|why| format!("전환 일지를 적지 못했습니다: {why}"))
}

fn clear_journal(dir: &Path) {
    let _ = std::fs::remove_file(journal_file(dir));
}

impl Journal {
    fn receipt(&self, moved: AccountMove, key: String, panes_moved: u32) -> AccountSwitchReceipt {
        AccountSwitchReceipt {
            key,
            agent: "claude".to_string(),
            moved,
            from_account: self.from.clone(),
            to_account: self.to.clone(),
            by: self.by.clone(),
            reason: self.reason.clone(),
            observed_percent: self.observed_percent,
            observed_window: self.observed_window.clone(),
            generation: self.generation,
            panes_moved,
        }
    }

    fn default_key(&self) -> String {
        format!("{}/default", self.key)
    }

    fn pane_key(&self, worker: &str) -> String {
        format!("{}/pane/{worker}", self.key)
    }
}

/// A receipt in the ledger's own voice. `Ok(false)` is a switch no run is
/// concerned with (or one already written — the key); `Err` is a ledger
/// that refused, which the caller reports instead of a success.
fn receipt_written(receipt: AccountSwitchReceipt, now_ms: i64) -> Result<bool, String> {
    crate::orchestration::record_account_switch(receipt, now_ms)
}

/// Finish what a journal left owed: the receipt of a default that landed,
/// and the receipt of every pane that is live again in another pane. A
/// pane still asleep stays in the journal for the restore road to seat; a
/// pane that never moved, or ended, owes nothing. Answers the receipts
/// written.
pub(crate) fn reconcile(host: &dyn Host, doors: &dyn SwitchDoors, now_ms: i64) -> usize {
    let dir = doors.journal_dir();
    let Some(mut journal) = read_journal(&dir) else {
        return 0;
    };
    let mut written = 0;
    let mut pending = Vec::new();
    let mut moved = 0;
    for pane in std::mem::take(&mut journal.panes) {
        match crate::orchestration::worker_seat_now(&pane.worker) {
            Some((_, now_pane, term, state)) if state.is_live() && now_pane != pane.from_pane => {
                moved += 1;
                let mut receipt = journal.receipt(
                    AccountMove::Pane {
                        worker: pane.worker.clone(),
                        from_pane: pane.from_pane.clone(),
                        to_pane: now_pane,
                    },
                    journal.pane_key(&pane.worker),
                    1,
                );
                receipt.from_account = Some(pane.from_account.clone());
                if let Some(account) = term.and_then(|term| host.pane_account(term)) {
                    receipt.to_account = Some(account);
                }
                if receipt_written(receipt, now_ms).unwrap_or(false) {
                    written += 1;
                }
            }
            Some((_, _, _, WorkerState::Sleeping)) => pending.push(pane),
            _ => {}
        }
    }
    if journal.default {
        let active = doors
            .situation(now_ms)
            .ok()
            .and_then(|situation| situation.active);
        if active == journal.to
            && receipt_written(
                journal.receipt(AccountMove::Default, journal.default_key(), moved),
                now_ms,
            )
            .unwrap_or(false)
        {
            written += 1;
        }
        journal.default = false;
    }
    if pending.is_empty() {
        clear_journal(&dir);
    } else {
        journal.panes = pending;
        let _ = write_journal(&dir, &journal);
    }
    if written > 0 {
        doors.log(&format!(
            "account-switch: journal {} finished {written} receipt(s)",
            journal.key
        ));
    }
    written
}

/* ---- applying ---- */

/// One pane the switch moved, or tried to.
#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct MovedPane {
    pub(crate) worker: String,
    pub(crate) from_term: u32,
    pub(crate) to_term: Option<u32>,
    /// The account the new pane really runs as, read off its launch.
    pub(crate) to_account: Option<String>,
    pub(crate) ok: bool,
    pub(crate) why: Option<String>,
    pub(crate) ms: u128,
}

/// What a switch did — every effect it had and how long each took, for the
/// toast, the log and the report.
#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct Applied {
    pub(crate) key: String,
    pub(crate) from: Option<String>,
    pub(crate) to: Option<String>,
    pub(crate) switched_default: bool,
    pub(crate) default_ms: u128,
    pub(crate) panes: Vec<MovedPane>,
    pub(crate) receipts: usize,
    /// The ledger refused a receipt: the switch happened and the window
    /// says so, but not as a success.
    pub(crate) receipt_error: Option<String>,
    pub(crate) total_ms: u128,
}

/// The source's fullest window and percentage, for the receipt.
fn observed(plan: &SwitchPlan) -> (Option<u8>, Option<String>) {
    match &plan.decision {
        Decision::Switch {
            reason:
                SwitchReason::NearLimit {
                    window,
                    used_percent,
                },
            ..
        } => (Some(*used_percent), Some(window.clone())),
        _ => (
            plan.active
                .as_deref()
                .and_then(|source| plan.fitness.iter().find(|fit| fit.id == source))
                .and_then(|fit| fit.room_percent)
                .map(|room| 100 - room),
            None,
        ),
    }
}

/// The words the resumed pane is told.
fn switch_nudge(to: &str) -> String {
    format!(
        "Your Claude account hit its usage limit, so this window moved this conversation to \
         another of the person's accounts ({to}) and resumed it here — same task, same \
         checkout, same worker seat, same model and effort. Continue exactly where you left \
         off; if the work was already finished, say so briefly."
    )
}

/// Apply the plan the window was shown — and only that plan: the situation
/// is read again under the switch line and, if anything the token names
/// moved (the mode, the selected account, the decision, a walled pane's
/// seat, generation or verdict), nothing happens and the caller is told to
/// look again (astra B1). `by` is `auto` (the beat; only under `auto`) or
/// `ask` (a person's yes to the proposal). The person's own picker is
/// [`switch_by_person`].
pub(crate) fn apply_with(
    host: &dyn Host,
    doors: &dyn SwitchDoors,
    token: &str,
    by: &str,
    now_ms: i64,
) -> Result<Applied, String> {
    let _line = SWITCH_LINE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let began = Instant::now();
    reconcile(host, doors, now_ms);
    let plan = plan_with(host, doors.situation(now_ms)?);
    if plan.token != token {
        return Err("상황이 바뀌어 전환하지 않았습니다 — 다시 확인하세요".to_string());
    }
    match (by, plan.mode) {
        (_, AutoSwitchMode::Off) => return Err("자동 전환이 꺼져 있습니다".to_string()),
        ("auto", AutoSwitchMode::Ask) => {
            return Err("물어보기 설정에서는 사람이 누를 때만 바꿉니다".to_string());
        }
        _ => {}
    }
    if !plan.acts() {
        return Err("지금은 바꿀 계정이 없습니다".to_string());
    }
    let (reason, from, to) = match &plan.decision {
        Decision::Switch { from, reason, .. } => {
            (reason.word(), Some(from.clone()), plan.landing.clone())
        }
        _ => (
            SwitchReason::Walled.word(),
            plan.active.clone(),
            plan.landing.clone(),
        ),
    };
    let (observed_percent, observed_window) = observed(&plan);
    let dir = doors.journal_dir();
    let journal = Journal {
        key: format!("switch-{now_ms}-{}", plan.token),
        by: by.to_string(),
        reason: reason.to_string(),
        from,
        to: to.clone(),
        default: matches!(plan.decision, Decision::Switch { .. }),
        observed_percent,
        observed_window,
        generation: plan.moves().find_map(|row| row.generation),
        panes: plan
            .moves()
            .filter_map(|row| {
                let (_, from_pane, _, _) = crate::orchestration::worker_seat_now(&row.worker)?;
                Some(JournalPane {
                    worker: row.worker.clone(),
                    from_pane,
                    from_account: row.account.clone()?,
                })
            })
            .collect(),
        began_ms: now_ms,
    };
    write_journal(&dir, &journal)?;
    // ① The default, through the same verified select the picker uses, so
    // the next launch runs as `to` or nothing moved. A refusal is
    // remembered against `to`, and the next plan leaves it out for one
    // cooldown; the journal goes, since nothing happened.
    let default_began = Instant::now();
    if journal.default {
        if let Err(why) = doors.select(to.as_deref()) {
            if let Some(to) = &to {
                doors.select_refused(to, now_ms);
            }
            clear_journal(&dir);
            doors.log(&format!(
                "account-switch: {by} {reason} refused to select {}: {why}",
                to.as_deref().unwrap_or("system")
            ));
            return Err(why);
        }
        doors.selected(to.as_deref(), now_ms);
    }
    let default_ms = default_began.elapsed().as_millis();
    // ② Each walled pane the table sent to the landing: its own receipt as
    // it lands; a pane that cannot be moved is left exactly as it was and
    // named in the answer.
    let landing = to.clone().unwrap_or_default();
    let mut panes = Vec::new();
    let mut receipts = 0;
    let mut receipt_error = None;
    for row in plan.moves() {
        let moved = move_pane(host, doors, row, &landing, &journal.key, now_ms);
        if moved.ok
            && let Some(entry) = journal
                .panes
                .iter()
                .find(|entry| entry.worker == row.worker)
        {
            let to_pane = crate::orchestration::worker_seat_now(&row.worker)
                .map(|(_, pane, _, _)| pane)
                .unwrap_or_default();
            let mut receipt = journal.receipt(
                AccountMove::Pane {
                    worker: row.worker.clone(),
                    from_pane: entry.from_pane.clone(),
                    to_pane,
                },
                journal.pane_key(&row.worker),
                1,
            );
            receipt.from_account = Some(entry.from_account.clone());
            receipt.to_account.clone_from(&moved.to_account);
            match receipt_written(receipt, epoch_ms_now()) {
                Ok(true) => receipts += 1,
                Ok(false) => {}
                Err(why) => receipt_error = Some(why),
            }
        }
        doors.log(&format!(
            "account-switch: pane {} worker {} {}→{} {}ms ok={}{}",
            row.term,
            row.worker,
            row.account.as_deref().unwrap_or("?"),
            landing,
            moved.ms,
            moved.ok,
            moved
                .why
                .as_deref()
                .map(|why| format!(" ({why})"))
                .unwrap_or_default()
        ));
        panes.push(moved);
    }
    // ③ The default's receipt last, so it can say how many panes moved
    // with it.
    let moved_count = u32::try_from(panes.iter().filter(|pane| pane.ok).count()).unwrap_or(0);
    if journal.default {
        match receipt_written(
            journal.receipt(AccountMove::Default, journal.default_key(), moved_count),
            epoch_ms_now(),
        ) {
            Ok(true) => receipts += 1,
            Ok(false) => {}
            Err(why) => receipt_error = Some(why),
        }
    }
    // What is still asleep stays in the journal for the restore road; the
    // rest is done.
    let asleep: Vec<JournalPane> = journal
        .panes
        .iter()
        .filter(|entry| {
            matches!(
                crate::orchestration::worker_seat_now(&entry.worker),
                Some((_, _, _, WorkerState::Sleeping))
            )
        })
        .cloned()
        .collect();
    if asleep.is_empty() {
        clear_journal(&dir);
    } else {
        let _ = write_journal(
            &dir,
            &Journal {
                default: false,
                panes: asleep,
                ..journal.clone()
            },
        );
    }
    let applied = Applied {
        key: journal.key.clone(),
        from: journal.from.clone(),
        to,
        switched_default: journal.default,
        default_ms,
        panes,
        receipts,
        receipt_error,
        total_ms: began.elapsed().as_millis(),
    };
    doors.log(&format!(
        "account-switch: {by} {reason} {}→{} default={} ({}ms) panes={} moved={moved_count} \
         receipts={} total={}ms",
        applied.from.as_deref().unwrap_or("system"),
        applied.to.as_deref().unwrap_or("system"),
        applied.switched_default,
        applied.default_ms,
        applied.panes.len(),
        applied.receipts,
        applied.total_ms
    ));
    Ok(applied)
}

/// Move ONE walled pane to `landing` — the same worker id, dispatch,
/// checkout and conversation (t-7538, condition 6). Everything that can
/// be known beforehand is checked before anything is touched; then, in the
/// restore line, the row learns what the pane really runs, the ledger
/// rests the worker, the pane closes and its process group is gone, and
/// the restore road seats that one worker again. A pane that fails a check
/// is left exactly as it was.
fn move_pane(
    host: &dyn Host,
    doors: &dyn SwitchDoors,
    row: &WalledRow,
    landing: &str,
    key: &str,
    now_ms: i64,
) -> MovedPane {
    let began = Instant::now();
    let mut moved = MovedPane {
        worker: row.worker.clone(),
        from_term: row.term,
        to_term: None,
        to_account: None,
        ok: false,
        why: None,
        ms: 0,
    };
    let checked = (|| -> Result<(u32, PaneTuning), String> {
        let Some((team, _, Some(term), state)) = crate::orchestration::worker_seat_now(&row.worker)
        else {
            return Err("the worker's seat is not a pane of this window".to_string());
        };
        if term != row.term || !state.is_live() {
            return Err("the worker moved seats since the plan".to_string());
        }
        let leader = crate::orchestration::leader_term_of_team(&team)
            .ok_or("the worker's team has no leader pane to restore under")?;
        let tuning = pane_tuning(host, term).ok_or(
            "no conversation the window can read in the pane — a restore would start it empty",
        )?;
        crate::orchestration::switch_move_ready(&row.worker, &tuning.session.id, now_ms)?;
        relaunch_keeps_permission(tuning.permission_mode.as_deref(), &doors.overrides())?;
        Ok((leader, tuning))
    })();
    let (leader, tuning) = match checked {
        Ok(checked) => checked,
        Err(why) => {
            moved.why = Some(why);
            moved.ms = began.elapsed().as_millis();
            return moved;
        }
    };
    let overrides = doors.overrides();
    let actor = host.actor_for(leader);
    let walked = crate::orchestration::in_restore_line(|line| -> Result<usize, String> {
        let row_model = crate::orchestration::worker_model_now(&row.worker);
        crate::orchestration::observe_worker_tuning(
            &row.worker,
            relaunch_model(row_model.as_deref(), tuning.model.as_deref()).as_deref(),
            tuning.effort.as_deref(),
            now_ms,
        );
        crate::orchestration::rest_worker_for_switch(&row.worker, &tuning.session.id, now_ms)?;
        crate::orchestration::leave_switch_mark(
            &row.worker,
            crate::orchestration::SwitchMark {
                nudge: switch_nudge(landing),
                occasion: key.to_string(),
            },
        );
        // The old pane goes only now: the ledger has the worker asleep, so
        // its exit settles nothing — and the new one opens only once the old
        // CLI is gone, so the conversation never has two writers.
        if !host.close_gone(row.term) {
            return Err(
                "the old pane's program outlived the wait; the worker sleeps with its attempt \
                 open for the restore road"
                    .to_string(),
            );
        }
        Ok(crate::orchestration::reseat_one_in_line(
            line,
            host,
            overrides,
            leader,
            actor.as_deref(),
            &row.worker,
        ))
    });
    let landed = crate::orchestration::worker_seat_now(&row.worker);
    moved.to_term = landed.as_ref().and_then(|(_, _, term, _)| *term);
    moved.to_account = moved.to_term.and_then(|term| host.pane_account(term));
    match walked {
        Err(why) => moved.why = Some(why),
        Ok(restored) => {
            moved.ok = restored > 0
                && landed
                    .as_ref()
                    .is_some_and(|(_, _, term, state)| term.is_some() && state.is_live())
                && moved.to_account.as_deref() == Some(landing);
            if !moved.ok {
                moved.why = Some(if restored == 0 {
                    "the worker is asleep with its attempt open; the restore road seats it"
                        .to_string()
                } else {
                    format!(
                        "the new pane runs as {} rather than {landing}",
                        moved.to_account.as_deref().unwrap_or("an unrecorded login")
                    )
                });
            }
        }
    }
    moved.ms = began.elapsed().as_millis();
    moved
}

/// The person's own pick (t-7538): the default moves — or goes back to the
/// machine's own login when `to` is `None` — and NO pane is touched, working
/// or walled, busy or resting: every open pane keeps the login it was
/// started with. One receipt, in the ledger's voice, `by: person`.
pub(crate) fn switch_by_person(
    host: &dyn Host,
    doors: &dyn SwitchDoors,
    to: Option<&str>,
    now_ms: i64,
) -> Result<Applied, String> {
    let _line = SWITCH_LINE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let began = Instant::now();
    reconcile(host, doors, now_ms);
    let situation = doors.situation(now_ms)?;
    let from = situation.active.clone();
    let observed = from.as_deref().and_then(|source| {
        fitness_table(&situation.gauges, now_ms)
            .into_iter()
            .find(|fit| fit.id == source)
            .and_then(|fit| fit.room_percent)
            .map(|room| 100 - room)
    });
    let dir = doors.journal_dir();
    let journal = Journal {
        key: format!("person-{now_ms}"),
        by: "person".to_string(),
        reason: "picked".to_string(),
        from,
        to: to.map(str::to_string),
        default: true,
        observed_percent: observed,
        observed_window: None,
        generation: None,
        panes: Vec::new(),
        began_ms: now_ms,
    };
    write_journal(&dir, &journal)?;
    if let Err(why) = doors.select(to) {
        clear_journal(&dir);
        return Err(why);
    }
    doors.selected(to, now_ms);
    let default_ms = began.elapsed().as_millis();
    let (receipts, receipt_error) = match receipt_written(
        journal.receipt(AccountMove::Default, journal.default_key(), 0),
        epoch_ms_now(),
    ) {
        Ok(written) => (usize::from(written), None),
        Err(why) => (0, Some(why)),
    };
    clear_journal(&dir);
    doors.log(&format!(
        "account-switch: person picked {}→{} ({}ms) receipts={receipts}",
        journal.from.as_deref().unwrap_or("system"),
        to.unwrap_or("system"),
        default_ms
    ));
    Ok(Applied {
        key: journal.key.clone(),
        from: journal.from.clone(),
        to: journal.to.clone(),
        switched_default: true,
        default_ms,
        panes: Vec::new(),
        receipts,
        receipt_error,
        total_ms: began.elapsed().as_millis(),
    })
}

/// The window's plan right now: a journal left owed is finished first, so a
/// plan never speaks over a switch that is still settling.
pub(crate) fn plan(state: &AppState, host: &dyn Host, now_ms: i64) -> Result<SwitchPlan, String> {
    let doors = WindowDoors { state };
    let _line = SWITCH_LINE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    reconcile(host, &doors, now_ms);
    Ok(plan_with(host, doors.situation(now_ms)?))
}

/// The window's apply.
pub(crate) fn apply(app: &AppHandle, token: &str, by: &str) -> Result<Applied, String> {
    let state = app.state::<AppState>();
    let window = TeamWindow { app: app.clone() };
    apply_with(
        &window,
        &WindowDoors { state: &state },
        token,
        by,
        epoch_ms_now(),
    )
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
            model: None,
            dispatch: format!("dp-{worker}"),
            generation: Some(1),
            verdict: Decision::Switch {
                from: "a".into(),
                to: "b".into(),
                reason: SwitchReason::Walled,
            },
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
        // A pane the table stopped sending anywhere is another plan.
        let mut stayed = row("w-1", 7, Some("a"));
        stayed.verdict = Decision::Stay {
            why: "no_candidate",
        };
        assert_ne!(
            same,
            token_of(AutoSwitchMode::Ask, Some("a"), &decision, &[stayed])
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
        assert_eq!(landed.status, "signed_out");
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

    /// One inactive account, in a store of its own under `root` — beside a
    /// selected one, since a store with no selection runs as its first.
    fn inactive_fixture(root: &Path, id: &str) -> zerocode_core::ClaudeAccount {
        let account_in = |named: &str| {
            let dir = root.join("stores").join(named);
            std::fs::create_dir_all(&dir).unwrap();
            zerocode_core::ClaudeAccount {
                id: named.to_string(),
                email: format!("{named}@example.test"),
                account_uuid: Some(format!("{named}-person")),
                organization_uuid: Some(format!("{named}-org")),
                config_dir: dir.to_string_lossy().into_owned(),
                added_at: 1,
                ..zerocode_core::ClaudeAccount::default()
            }
        };
        let selected = account_in(&format!("{id}-selected"));
        let account = account_in(id);
        let store = accounts::AccountStore {
            accounts: vec![selected.clone(), account.clone()],
            selection: zerocode_core::account::AccountSelection {
                active: Some(selected.id),
                system_default: false,
            },
        };
        std::fs::write(
            root.join(accounts::ACCOUNT_STORE_FILE),
            serde_json::to_string(&store).unwrap(),
        )
        .unwrap();
        account
    }

    fn a_reading(used: f32) -> crate::usage_oauth::OauthUsage {
        crate::usage_oauth::OauthUsage {
            session: Some(crate::usage_oauth::OauthWindow {
                used_percent: used,
                window_minutes: 300,
                resets_at: Some(epoch_ms_now() + 3_600_000),
            }),
            ..crate::usage_oauth::OauthUsage::default()
        }
    }

    /// Account B's read asks with B's own login and lands under B only while
    /// B is still the account it began as (astra A1): removed, re-logged as
    /// another person, or moved to another store mid-flight, the answer is
    /// dropped; and an answer that arrives after a newer one never replaces
    /// it.
    #[test]
    fn an_inactive_read_lands_only_on_the_account_it_began_as_and_never_over_a_newer_one() {
        let config = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let b = inactive_fixture(config.path(), "t7538-land-b");
        let asked = std::cell::RefCell::new(Vec::new());
        let scanned = scan_claude_account_usage_with(
            &b,
            |account| {
                accounts::AccountLogin::Found(
                    format!("login-document-of-{}", account.id),
                    accounts::LoginFrom::File,
                )
            },
            |login, _| {
                asked.borrow_mut().push(login.to_string());
                Ok(a_reading(30.0))
            },
        );
        assert_eq!(
            asked.borrow().as_slice(),
            ["login-document-of-t7538-land-b"]
        );
        assert_eq!(scanned.usage.status, "ok");
        assert_eq!(scanned.usage.account.as_deref(), Some("t7538-land-b"));
        assert!(land_claude_account_usage(
            config.path(),
            data.path(),
            &b,
            scanned.usage.clone()
        ));
        // An older answer after it: the newer stands.
        let mut older = scanned.usage.clone();
        older.updated_at -= 60_000;
        older.session = None;
        assert!(land_claude_account_usage(
            config.path(),
            data.path(),
            &b,
            older
        ));
        let held = claude_account_usage_cache(data.path())
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get("t7538-land-b")
            .cloned()
            .expect("B's reading");
        assert_eq!(held.updated_at, scanned.usage.updated_at);
        assert!(held.session.is_some(), "a late answer replaced a newer one");
        // Re-logged as another person under the same id: dropped.
        let mut relogged = b.clone();
        relogged.account_uuid = Some("somebody-else".to_string());
        inactive_fixture(config.path(), "t7538-land-b");
        let mut store = accounts::read_store(config.path());
        store
            .accounts
            .iter_mut()
            .filter(|account| account.id == "t7538-land-b")
            .for_each(|account| account.account_uuid = Some("somebody-else".to_string()));
        std::fs::write(
            config.path().join(accounts::ACCOUNT_STORE_FILE),
            serde_json::to_string(&store).unwrap(),
        )
        .unwrap();
        assert!(!land_claude_account_usage(
            config.path(),
            data.path(),
            &b,
            scanned.usage.clone()
        ));
        // Removed: dropped.
        std::fs::write(
            config.path().join(accounts::ACCOUNT_STORE_FILE),
            serde_json::to_string(&accounts::AccountStore::default()).unwrap(),
        )
        .unwrap();
        assert!(!land_claude_account_usage(
            config.path(),
            data.path(),
            &relogged,
            scanned.usage
        ));
    }

    /// A read that cannot ask asks nobody, and says which of two repairs it
    /// needs (astra A2): a keychain that would not answer is `denied` and is
    /// asked again only by a person's press — a timer that asked would be a
    /// password dialog every poll — and an empty store is `signed_out`. A
    /// read that failed carries no token and no address anywhere it is
    /// kept, whatever the server's body said (astra C1).
    #[test]
    fn an_inactive_read_that_cannot_ask_asks_nobody_and_keeps_no_secret() {
        const SENTINEL: &str = "sk-ant-oat01-T7538-SENTINEL";
        let config = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let b = inactive_fixture(config.path(), "t7538-sentinel-b");
        let mut asks = 0;
        let refused = scan_claude_account_usage_with(
            &b,
            |_| accounts::AccountLogin::Refused,
            |_, _| {
                asks += 1;
                Ok(a_reading(1.0))
            },
        );
        assert_eq!(refused.usage.status, ACCOUNT_LOGIN_REFUSED);
        let missing = scan_claude_account_usage_with(
            &b,
            |_| accounts::AccountLogin::Missing,
            |_, _| {
                asks += 1;
                Ok(a_reading(1.0))
            },
        );
        assert_eq!(missing.usage.status, "signed_out");
        assert_eq!(asks, 0, "a read with no login of its own asked anyway");
        // The server refused and said things in its body: none of it, and
        // none of the login, is kept.
        let failed = scan_claude_account_usage_with(
            &b,
            |_| {
                accounts::AccountLogin::Found(
                    format!(r#"{{"claudeAiOauth":{{"accessToken":"{SENTINEL}"}}}}"#),
                    accounts::LoginFrom::Keychain,
                )
            },
            |login, _| {
                assert!(login.contains(SENTINEL));
                Err(crate::usage_http::Failure {
                    recovery: zerocode_core::usage_limit::classify_http(
                        401,
                        &format!("{{\"error\":\"bad token {SENTINEL} for b@example.test\"}}"),
                    ),
                    retry_at_ms: None,
                    message: "HTTP 401".to_string(),
                    skip_cli_fallback: true,
                })
            },
        );
        assert_eq!(failed.usage.status, "error");
        assert!(land_claude_account_usage(
            config.path(),
            data.path(),
            &b,
            failed.usage.clone()
        ));
        let kept = serde_json::to_string(&failed.usage).unwrap()
            + &std::fs::read_to_string(claude_account_usage_file(data.path())).unwrap_or_default();
        assert!(!kept.contains(SENTINEL), "{kept}");
        assert!(!kept.contains("example.test"), "{kept}");
        // The refused keychain is held until a person asks: a beat sends
        // nothing for it, a press does. On an account of its own, whose
        // only reading is the refusal — no failure streak to hold it too.
        let config_c = tempfile::tempdir().unwrap();
        let data_c = tempfile::tempdir().unwrap();
        let c = inactive_fixture(config_c.path(), "t7538-denied-c");
        let refused_c = scan_claude_account_usage_with(
            &c,
            |_| accounts::AccountLogin::Refused,
            |_, _| panic!("a refused keychain's read asked the server"),
        );
        assert!(land_claude_account_usage(
            config_c.path(),
            data_c.path(),
            &c,
            refused_c.usage
        ));
        {
            let mut held = claude_account_usage_cache(data_c.path())
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let row = held.get_mut("t7538-denied-c").expect("C's refusal");
            assert_eq!(row.status, ACCOUNT_LOGIN_REFUSED);
            // Past every floor, so only the refusal can hold it.
            row.updated_at -= 24 * 60 * 60_000;
        }
        assert_eq!(
            refresh_inactive_claude_accounts(config_c.path(), data_c.path(), AccountPoll::Ambient),
            0
        );
        assert_eq!(
            refresh_inactive_claude_accounts(
                config_c.path(),
                data_c.path(),
                AccountPoll::Candidate
            ),
            0
        );
        assert_eq!(
            refresh_inactive_claude_accounts(config_c.path(), data_c.path(), AccountPoll::Person),
            1
        );
        let began = Instant::now();
        while claude_account_scanning_now("t7538-denied-c")
            && began.elapsed() < Duration::from_secs(10)
        {
            std::thread::sleep(Duration::from_millis(10));
        }
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
