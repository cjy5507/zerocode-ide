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
//! written down AND the window still sees at the moment of the move (astra
//! R2), and it moves by the restore road a window restart already uses —
//! the ledger rests it, the pane closes and its process group is gone,
//! `reseat_one_in_line` opens the new pane with `--resume` — all in the
//! restore line, so neither another restore nor the grace can walk the
//! worker between its rest and its new pane. The program in the pane is
//! written on the worker's row by the rest itself, before the close, and
//! every road that would open the conversation again waits until a look
//! sees that program gone (R3) — the ledger holds it, so neither a window
//! that restarts nor a journal that could not be written lets it go.
//!
//! Every effect is written down before it happens and stays written down
//! until the ledger has accepted its receipt ([`JournalBook`], R4): a
//! refused receipt, a window that dies halfway, a second switch on top of
//! an unfinished one — none of them loses what happened.
//!
//! What never happens here: a credential is not read (the account store's
//! ids and the usage cache's numbers are the only inputs), an email is not
//! written (refusals name the account id), a pane is not closed before the
//! ledger has agreed to rest its worker, a conversation is never started
//! empty in place of the one the pane had, and a model, effort or
//! permission mode the pane's transcript does not say is never filled in
//! from the summons (R5).

use super::*;
use crate::agent_teams::{Host, PaneExit, PaneLogin};
use zerocode_core::LaunchOverride;
use zerocode_core::account_autoswitch::{
    AccountGauge, AutoSwitchMode, CLAUDE_ACCOUNT_AUTOSWITCH, Decision, Fitness, Question,
    SwitchReason, best_candidate, decide, fitness_table, judge_pane,
};
use zerocode_core::orchestration::{AccountMove, AccountSwitchReceipt, SwitchRest, WorkerState};

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
    /// Which login every managed account is right now — its store and its
    /// identity, by id (`claude_login_key`). The plan's token names them,
    /// so a yes given while an id named one login is refused once it names
    /// another (astra R6); never shown, never written.
    pub(crate) logins: Vec<(String, String)>,
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
    /// The clock a pane's move is committed by — the moment of use, after
    /// the selection and the checks, never the moment the plan was read.
    fn now_ms(&self) -> i64;
    /// One receipt in the ledger's own voice. `Ok(false)` is a receipt no
    /// run is concerned with, or one already written (its key); `Err` is a
    /// ledger that refused, and the journal keeps the receipt owed.
    fn record(&self, receipt: AccountSwitchReceipt, now_ms: i64) -> Result<bool, String> {
        crate::orchestration::record_account_switch(receipt, now_ms)
    }
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
        let store = accounts::read_store(config_root);
        Ok(Situation {
            mode,
            active: zerocode_core::active_account(&store.accounts, &store.selection)
                .map(|account| account.id.clone()),
            gauges: claude_account_gauges_of(&store, self.state.local_data_root()),
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
            logins: store
                .accounts
                .iter()
                .map(|account| (account.id.clone(), claude_login_key(account)))
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

    fn now_ms(&self) -> i64 {
        epoch_ms_now()
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

/// What a pane runs right now — model, effort, permission mode — every
/// part of it read off its own transcript, or the parts that are not
/// (astra R5). A switch never fills a part it could not read with the
/// summons' value, the account's default or a guess: a transcript whose
/// tail no longer holds the newest word (a long tool output after it), or
/// a CLI that never wrote one, leaves the pane where it is, and the answer
/// says which part.
fn known_tuning(tuning: &PaneTuning) -> Result<(String, String, String), String> {
    let known = |held: &Option<String>| {
        held.as_deref()
            .map(str::trim)
            .filter(|word| !word.is_empty())
            .map(str::to_string)
    };
    match (
        known(&tuning.model),
        known(&tuning.effort),
        known(&tuning.permission_mode),
    ) {
        (Some(model), Some(effort), Some(mode)) => Ok((model, effort, mode)),
        (model, effort, mode) => {
            let unread: Vec<&str> = [
                ("model", model.is_none()),
                ("effort", effort.is_none()),
                ("permission mode", mode.is_none()),
            ]
            .into_iter()
            .filter(|(_, unread)| *unread)
            .map(|(part, _)| part)
            .collect();
            Err(format!(
                "the pane's transcript does not say its current {} — not moved, rather than \
                 relaunched on what it was summoned with",
                unread.join(", ")
            ))
        }
    }
}

/// The model word the relaunch carries: the pane's own id — the model it
/// really runs. The row's spelling stays only where it names exactly that
/// id with a context suffix the transcript's id does not show (`[1m]`);
/// a row that named a family word (`opus[1m]`) lends the pane's id its
/// suffix, and nothing more — a family word is not taken for the version
/// the pane ran.
fn relaunch_model(row: Option<&str>, pane: &str) -> String {
    let pane = pane.trim();
    let Some(row) = row.map(str::trim).filter(|row| !row.is_empty()) else {
        return pane.to_string();
    };
    let (named, suffix) = row.split_at(row.find('[').unwrap_or(row.len()));
    let named = named.trim().to_ascii_lowercase();
    let id = pane.to_ascii_lowercase();
    if named == id {
        return row.to_string();
    }
    if !suffix.is_empty() && !named.is_empty() && id.split('-').any(|word| word == named) {
        return format!("{pane}{suffix}");
    }
    pane.to_string()
}

/// Whether a relaunch through `overrides` starts the pane in the permission
/// mode it runs in now (t-7538, condition 6). Claude Code's own mode words
/// ([`zerocode_core::agent::agent_voice`]) against the launch plan's: an
/// unattended launch starts in `bypassPermissions`, an asking one in
/// `default`, and a launch somebody edited by hand says nothing a mode
/// could be compared with. The pane's mode is always a word its transcript
/// said ([`known_tuning`]).
fn relaunch_keeps_permission(
    pane_mode: &str,
    overrides: &[(String, LaunchOverride)],
) -> Result<(), String> {
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
    /// Names this exact plan: mode, source, decision, every walled pane's
    /// seat, generation and verdict, and which login every account id names
    /// (astra R6). `apply` re-plans and compares.
    pub(crate) token: String,
    /// Which login every account id named when this plan was read — what a
    /// yes to it approved, compared again at every pane's last door and at
    /// its rest (astra R2). Never shown, never written.
    #[serde(skip)]
    pub(crate) logins: Vec<(String, String)>,
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
    logins: &[(String, String)],
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
    let mut logins: Vec<&(String, String)> = logins.iter().collect();
    logins.sort();
    for (id, login) in logins {
        id.hash(&mut hasher);
        login.hash(&mut hasher);
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
        logins,
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
    let mut walled: Vec<WalledRow> = crate::orchestration::walled_claude_workers(host, now_ms)
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
    let token = token_of(mode, active.as_deref(), &decision, &walled, &logins);
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
        logins,
        now_ms,
    }
}

/// The login `id` names in `logins`, when it names one.
fn login_of(logins: &[(String, String)], id: &str) -> Option<String> {
    logins
        .iter()
        .find(|(named, _)| named == id)
        .map(|(_, login)| login.clone())
}

/// What a yes was given to beyond the plan's own token, for ONE pane (astra
/// R2): the road that asked, and the login the pane's account and its
/// landing named when the plan was read. The default's selection moves the
/// plan's token by itself, so the token cannot be compared again after it —
/// these facts can, and every one of them must still stand when the pane is
/// touched.
struct Approval<'a> {
    by: &'a str,
    from: &'a str,
    from_login: Option<String>,
    landing: &'a str,
    to_login: Option<String>,
}

impl<'a> Approval<'a> {
    fn of(plan: &SwitchPlan, row: &'a WalledRow, by: &'a str, landing: &'a str) -> Option<Self> {
        let from = row.account.as_deref()?;
        Some(Self {
            by,
            from,
            from_login: login_of(&plan.logins, from),
            landing,
            to_login: login_of(&plan.logins, landing),
        })
    }

    /// Whether this approval still holds under `mode`, with every account
    /// id naming `logins`.
    fn stands(&self, mode: AutoSwitchMode, logins: &[(String, String)]) -> Result<(), String> {
        match (self.by, mode) {
            (_, AutoSwitchMode::Off) => {
                return Err(
                    "the switch was turned off before this pane moved — not moved".to_string(),
                );
            }
            ("auto", AutoSwitchMode::Ask) => {
                return Err(
                    "the switch asks first now — this pane waits for a person's yes".to_string(),
                );
            }
            _ => {}
        }
        for (id, approved) in [
            (self.from, &self.from_login),
            (self.landing, &self.to_login),
        ] {
            if login_of(logins, id) != *approved {
                return Err(format!(
                    "the login account {id} names changed since the yes — not moved; a new plan \
                     needs a new yes"
                ));
            }
        }
        Ok(())
    }
}

/* ---- the journal: nothing that happened is lost before its receipt ---- */

/// Every switch whose effects are not all receipted yet, oldest first — the
/// file a window reads back after a crash, a refused receipt or a restart
/// (astra B2, R4). A switch is written down BEFORE its first effect and
/// taken out only once the ledger has accepted every receipt it owes, or a
/// pane it set out to move is known not to be its to report. A new switch
/// is added beside an unfinished one, never over it.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct JournalBook {
    pub(crate) switches: Vec<Journal>,
}

/// One switch in flight. A window that reads it back finishes the
/// RECEIPTS — never the effects: the selection either landed or it did
/// not, and a rested worker is the restore road's, which a restart walks
/// anyway.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Journal {
    pub(crate) key: String,
    pub(crate) by: String,
    pub(crate) reason: String,
    pub(crate) from: Option<String>,
    pub(crate) to: Option<String>,
    /// The default moves in this switch and its receipt is not accepted
    /// yet.
    pub(crate) default: bool,
    /// The selection answered yes — written down the moment it did, so a
    /// look after a crash does not have to infer it from the store.
    #[serde(default)]
    pub(crate) landed: bool,
    pub(crate) observed_percent: Option<u8>,
    pub(crate) observed_window: Option<String>,
    pub(crate) generation: Option<u32>,
    pub(crate) panes: Vec<JournalPane>,
    /// Panes this switch has seen continue in a new pane so far — what the
    /// default's receipt counts.
    #[serde(default)]
    pub(crate) moved: u32,
    pub(crate) began_ms: i64,
}

/// One pane the switch set out to move, and what became of it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct JournalPane {
    pub(crate) worker: String,
    /// The attempt and the conversation the switch moved. The same worker
    /// seated later on another attempt, or in another conversation, is not
    /// this switch's move to report (astra R4).
    #[serde(default)]
    pub(crate) dispatch: String,
    #[serde(default)]
    pub(crate) session: String,
    pub(crate) from_pane: String,
    pub(crate) from_account: String,
    /// The ledger rested the worker for THIS switch — written down before
    /// its pane is closed. Only a rested pane is this switch's move: one
    /// the switch set out to move but never rested, and that a restart put
    /// to sleep and seated again, was moved by the restart.
    #[serde(default)]
    pub(crate) rested: bool,
    /// Where it continues, once it does — kept while its receipt is owed.
    /// Written ONCE, the first time a look sees the move complete, and
    /// never read again off the worker after that (astra R4): what the
    /// worker does next — ends, or moves on to another pane — is not what
    /// this switch did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) to_pane: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) to_account: Option<String>,
}

fn journal_file(dir: &Path) -> PathBuf {
    dir.join(app_paths::artifact_file::CLAUDE_ACCOUNT_SWITCH)
}

/// The book on disk: empty when there is no file; the first round's
/// one-switch file read as a book of one.
fn read_book(dir: &Path) -> Result<JournalBook, String> {
    let text = match std::fs::read_to_string(journal_file(dir)) {
        Ok(text) => text,
        Err(why) if why.kind() == std::io::ErrorKind::NotFound => {
            return Ok(JournalBook::default());
        }
        Err(why) => return Err(why.to_string()),
    };
    if let Ok(book) = serde_json::from_str::<JournalBook>(&text) {
        return Ok(book);
    }
    serde_json::from_str::<Journal>(&text)
        .map(|one| JournalBook {
            switches: vec![one],
        })
        .map_err(|why| why.to_string())
}

/// The book to add to or settle: the one this window could not write, when
/// it holds one (astra R4-1) — it is newer than the file — and otherwise
/// the file. A file this window cannot read is not written over — it is
/// set aside under its own name, for a person to look at, and the log says
/// where (astra R4). A file that cannot even be set aside refuses the
/// switch.
fn open_book(doors: &dyn SwitchDoors) -> Result<JournalBook, String> {
    let dir = doors.journal_dir();
    if let Some(held) = unwritten_book(&dir) {
        return Ok(held);
    }
    match read_book(&dir) {
        Ok(book) => Ok(book),
        Err(why) => {
            let aside = dir.join(format!(
                "{}.unreadable-{}",
                app_paths::artifact_file::CLAUDE_ACCOUNT_SWITCH,
                doors.now_ms()
            ));
            std::fs::rename(journal_file(&dir), &aside).map_err(|moved| {
                format!("전환 일지를 읽지도 옮기지도 못했습니다: {why} / {moved}")
            })?;
            doors.log(&format!(
                "account-switch: the journal could not be read ({why}); set aside as {}",
                aside.display()
            ));
            Ok(JournalBook::default())
        }
    }
}

/// The book this window last meant to write and could not, by journal
/// directory (t-7538, astra R4-1): read in place of the file, and written
/// again on every look, until a write lands. A move seen complete is not
/// given up because one replace failed. A window that ends first takes it
/// along — the reason a completion is also written down before its receipt
/// is asked for ([`settle`]), not only after.
fn unwritten_books() -> &'static Mutex<HashMap<PathBuf, JournalBook>> {
    static HELD: std::sync::OnceLock<Mutex<HashMap<PathBuf, JournalBook>>> =
        std::sync::OnceLock::new();
    HELD.get_or_init(|| Mutex::new(HashMap::new()))
}

fn unwritten_book(dir: &Path) -> Option<JournalBook> {
    unwritten_books()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(dir)
        .cloned()
}

/// `book`, left for the next look to write — or nothing left, once a write
/// landed.
fn leave_unwritten(dir: &Path, book: Option<&JournalBook>) {
    let mut held = unwritten_books()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    match book {
        Some(book) => {
            held.insert(dir.to_path_buf(), book.clone());
        }
        None => {
            held.remove(dir);
        }
    }
}

/// A new process of the window: nothing it could not write survives it.
#[cfg(test)]
pub(crate) fn a_new_process(dir: &Path) {
    leave_unwritten(dir, None);
}

/// Write the book down. One that is not written stays this window's book
/// to read and write again (astra R4-1): what it records happened.
fn write_book(dir: &Path, book: &JournalBook) -> Result<(), String> {
    let written = replace_book(dir, book);
    leave_unwritten(dir, written.as_ref().err().map(|_| book));
    written
}

/// Write the book with a new switch in it, before that switch's first
/// effect. A refusal refuses the switch — it never began — so nothing of
/// this book is kept to write later; what an earlier look left unwritten
/// stays as it was.
fn write_before_the_first_effect(dir: &Path, book: &JournalBook) -> Result<(), String> {
    let written = replace_book(dir, book);
    if written.is_ok() {
        leave_unwritten(dir, None);
    }
    written
}

fn replace_book(dir: &Path, book: &JournalBook) -> Result<(), String> {
    if book.switches.is_empty() {
        return match std::fs::remove_file(journal_file(dir)) {
            Err(why) if why.kind() != std::io::ErrorKind::NotFound => {
                Err(format!("전환 일지를 지우지 못했습니다: {why}"))
            }
            _ => Ok(()),
        };
    }
    let text = serde_json::to_string_pretty(book).map_err(|why| why.to_string())?;
    durable_file::replace_bytes(&journal_file(dir), text.as_bytes())
        .map(|_| ())
        .map_err(|why| format!("전환 일지를 적지 못했습니다: {why}"))
}

/// `journal` into the book by its key — or out of it, once it owes
/// nothing.
fn keep_in(book: &mut JournalBook, journal: &Journal) {
    book.switches.retain(|held| held.key != journal.key);
    if journal.owes() {
        book.switches.push(journal.clone());
    }
}

impl Journal {
    fn owes(&self) -> bool {
        self.default || !self.panes.is_empty()
    }

    fn receipt(
        &self,
        moved: AccountMove,
        key: String,
        panes_moved: u32,
        panes_pending: u32,
    ) -> AccountSwitchReceipt {
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
            panes_pending,
        }
    }

    fn default_key(&self) -> String {
        format!("{}/default", self.key)
    }

    fn pane_key(&self, worker: &str) -> String {
        format!("{}/pane/{worker}", self.key)
    }
}

/// What settling a journal wrote, and the ledger's refusal of what it kept
/// owed.
#[derive(Debug, Default)]
struct Settled {
    written: usize,
    refused: Option<String>,
}

/// Look at `journal`'s moves still pending, against the ledger as it
/// stands (astra R4). A move rested by this switch and live again in
/// ANOTHER pane, on the same attempt and in the same conversation, is
/// complete: where it continues is written on it once, there and then, and
/// the answer is `true` — the caller writes the book down before any
/// receipt is asked for (astra R4-1), so a completion never rides on the
/// receipt's fate or on the write after it. Still asleep, it stays pending
/// (the ledger holds its conversation while its old program may run).
/// Never rested by this switch, ended, or on another attempt or
/// conversation, it was not this switch's move, and goes, said in the log.
/// A move already seen complete is not read against the worker again —
/// what the worker does next, ending or moving on to another pane, is not
/// what this switch did.
fn observe_moves(host: &dyn Host, doors: &dyn SwitchDoors, journal: &mut Journal) -> bool {
    let key = journal.key.clone();
    let mut completed = 0;
    journal.panes.retain_mut(|pane| {
        if pane.to_pane.is_some() {
            return true;
        }
        let now = crate::orchestration::worker_now(&pane.worker);
        let same = pane.rested
            && now.as_ref().is_some_and(|now| {
                now.dispatch.as_deref() == Some(pane.dispatch.as_str())
                    && now.session.as_deref() == Some(pane.session.as_str())
            });
        match now {
            Some(now) if same && now.state == WorkerState::Sleeping => true,
            Some(now) if same && now.state.is_live() && now.pane != pane.from_pane => {
                completed += 1;
                pane.to_account = now.term.and_then(|term| host.pane_account(term));
                pane.to_pane = Some(now.pane);
                true
            }
            _ => {
                doors.log(&format!(
                    "account-switch: journal {key} lets worker {} go — this switch never rested \
                     it, or it ended, or it carries another attempt or conversation now: not \
                     this switch's move",
                    pane.worker
                ));
                false
            }
        }
    });
    journal.moved += completed;
    completed > 0
}

/// Write down the moves a look saw complete, before any receipt is asked
/// for (astra R4-1). A journal that refuses keeps them in this window's
/// book, written again on the next look ([`write_book`]).
fn write_down_moves(doors: &dyn SwitchDoors, book: &JournalBook) {
    if let Err(why) = write_book(&doors.journal_dir(), book) {
        doors.log(&format!(
            "account-switch: the moves seen complete stay in this window until the journal \
             takes them — {why}"
        ));
    }
}

/// Send what `journal` owes (astra R4). A completed move's receipt is sent
/// as it was written down, whatever the worker has done since; a pending
/// one owes nothing yet. The default's receipt is owed once the selection
/// landed, and counts what has moved and what is still asleep. A receipt
/// the ledger refused stays in the journal: nothing is taken out but what
/// the ledger accepted.
fn send_receipts(
    doors: &dyn SwitchDoors,
    journal: &mut Journal,
    active: Option<&str>,
    now_ms: i64,
) -> Settled {
    let mut settled = Settled::default();
    let mut kept = Vec::new();
    for pane in std::mem::take(&mut journal.panes) {
        let Some(to_pane) = pane.to_pane.clone() else {
            kept.push(pane);
            continue;
        };
        let mut receipt = journal.receipt(
            AccountMove::Pane {
                worker: pane.worker.clone(),
                from_pane: pane.from_pane.clone(),
                to_pane,
            },
            journal.pane_key(&pane.worker),
            1,
            0,
        );
        receipt.from_account = Some(pane.from_account.clone());
        receipt.to_account.clone_from(&pane.to_account);
        match doors.record(receipt, now_ms) {
            Ok(written) => settled.written += usize::from(written),
            Err(why) => {
                settled.refused = Some(why);
                kept.push(pane);
            }
        }
    }
    let pending = u32::try_from(kept.iter().filter(|pane| pane.to_pane.is_none()).count())
        .unwrap_or(u32::MAX);
    journal.panes = kept;
    if journal.default {
        if journal.landed || active == journal.to.as_deref() {
            journal.landed = true;
            match doors.record(
                journal.receipt(
                    AccountMove::Default,
                    journal.default_key(),
                    journal.moved,
                    pending,
                ),
                now_ms,
            ) {
                Ok(written) => {
                    settled.written += usize::from(written);
                    journal.default = false;
                }
                Err(why) => settled.refused = Some(why),
            }
        } else {
            // The selection never landed: nothing moved, nothing is owed.
            journal.default = false;
        }
    }
    settled
}

/// Settle `journal`, one switch of `book`: the moves it sees complete for
/// the first time are written down in the book first (astra R4-1), then
/// what it owes is sent. The caller writes the book after.
fn settle(
    host: &dyn Host,
    doors: &dyn SwitchDoors,
    book: &mut JournalBook,
    journal: &mut Journal,
    active: Option<&str>,
    now_ms: i64,
) -> Settled {
    if observe_moves(host, doors, journal) {
        keep_in(book, journal);
        write_down_moves(doors, book);
    }
    send_receipts(doors, journal, active, now_ms)
}

/// Finish what the journal left owed — every switch in it, against the
/// ledger as it stands — and write the book back only if that changed it,
/// or if it is a book an earlier look could not write (astra R4-1): a look
/// over a written journal with nothing to settle writes nothing. Answers
/// the receipts written. The caller holds [`SWITCH_LINE`] (or is a test
/// that owns the journal).
pub(crate) fn reconcile(host: &dyn Host, doors: &dyn SwitchDoors, now_ms: i64) -> usize {
    let dir = doors.journal_dir();
    let owed_a_write = unwritten_book(&dir).is_some();
    let mut book = match open_book(doors) {
        Ok(book) => book,
        Err(why) => {
            doors.log(&format!("account-switch: {why}"));
            return 0;
        }
    };
    if book.switches.is_empty() {
        if owed_a_write && let Err(why) = write_book(&dir, &book) {
            doors.log(&format!("account-switch: {why}"));
        }
        return 0;
    }
    let before = book.clone();
    let active = doors
        .situation(now_ms)
        .ok()
        .and_then(|situation| situation.active);
    let mut completed = false;
    for journal in &mut book.switches {
        completed |= observe_moves(host, doors, journal);
    }
    if completed {
        write_down_moves(doors, &book);
    }
    let mut written = 0;
    for journal in &mut book.switches {
        let settled = send_receipts(doors, journal, active.as_deref(), now_ms);
        written += settled.written;
        if let Some(why) = settled.refused {
            doors.log(&format!(
                "account-switch: journal {} keeps a receipt owed — {why}",
                journal.key
            ));
        }
    }
    book.switches.retain(Journal::owes);
    if (book != before || owed_a_write)
        && let Err(why) = write_book(&dir, &book)
    {
        doors.log(&format!("account-switch: {why}"));
    }
    if written > 0 {
        doors.log(&format!(
            "account-switch: the journal finished {written} receipt(s)"
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
    /// The ledger refused a receipt, or the journal could not be written:
    /// the switch happened and the window says so, but not as a success —
    /// what is owed stays in the journal for the next look.
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
/// seat, generation or verdict, the login an account id names), nothing
/// happens and the caller is told to look again (astra B1, R6). `by` is
/// `auto` (the beat; only under `auto`) or `ask` (a person's yes to the
/// proposal). The person's own picker is [`switch_by_person`].
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
    let switched_default = matches!(plan.decision, Decision::Switch { .. });
    let (observed_percent, observed_window) = observed(&plan);
    let dir = doors.journal_dir();
    let mut journal = Journal {
        key: format!("switch-{now_ms}-{}", plan.token),
        by: by.to_string(),
        reason: reason.to_string(),
        from,
        to: to.clone(),
        default: switched_default,
        landed: false,
        observed_percent,
        observed_window,
        generation: plan.moves().find_map(|row| row.generation),
        panes: plan
            .moves()
            .filter_map(|row| {
                let now = crate::orchestration::worker_now(&row.worker)?;
                Some(JournalPane {
                    worker: row.worker.clone(),
                    dispatch: row.dispatch.clone(),
                    session: now.session?,
                    from_pane: now.pane,
                    from_account: row.account.clone()?,
                    rested: false,
                    to_pane: None,
                    to_account: None,
                })
            })
            .collect(),
        moved: 0,
        began_ms: now_ms,
    };
    // Written down before the first effect, beside whatever an earlier
    // switch still owes — never over it.
    let mut book = open_book(doors)?;
    keep_in(&mut book, &journal);
    write_before_the_first_effect(&dir, &book)?;
    // ① The default, through the same verified select the picker uses, so
    // the next launch runs as `to` or nothing moved. A refusal is
    // remembered against `to`, and the next plan leaves it out for one
    // cooldown; this switch leaves the journal, since nothing happened.
    let default_began = Instant::now();
    if switched_default {
        if let Err(why) = doors.select(to.as_deref()) {
            if let Some(to) = &to {
                doors.select_refused(to, now_ms);
            }
            book.switches.retain(|held| held.key != journal.key);
            if let Err(unwritten) = write_book(&dir, &book) {
                doors.log(&format!("account-switch: {unwritten}"));
            }
            doors.log(&format!(
                "account-switch: {by} {reason} refused to select {}: {why}",
                to.as_deref().unwrap_or("system")
            ));
            return Err(why);
        }
        doors.selected(to.as_deref(), now_ms);
        journal.landed = true;
        keep_in(&mut book, &journal);
        if let Err(why) = write_book(&dir, &book) {
            doors.log(&format!("account-switch: {why}"));
        }
    }
    let default_ms = default_began.elapsed().as_millis();
    // ② Each walled pane the table sent to the landing. A pane that cannot
    // be moved is left exactly as it was and named in the answer; what a
    // moved one's rest and close leave is written down as it happens.
    let landing = to.clone().unwrap_or_default();
    let key = journal.key.clone();
    let mut panes = Vec::new();
    for row in plan.moves() {
        let approval = Approval::of(&plan, row, by, &landing);
        let moved = move_pane(
            host,
            doors,
            row,
            approval.as_ref(),
            &key,
            now_ms,
            &mut || {
                if let Some(entry) = journal
                    .panes
                    .iter_mut()
                    .find(|entry| entry.worker == row.worker)
                {
                    entry.rested = true;
                }
                keep_in(&mut book, &journal);
                write_book(&dir, &book)
            },
        );
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
    // ③ The receipts, through the one settling a later look uses: a move
    // seen complete is written down first (astra R4-1), then a moved pane's
    // receipt, then the default's with the count of what moved and what is
    // still asleep. What the ledger refuses stays owed in the journal, and a
    // journal that refuses a write stays this window's to write again.
    let settled = settle(
        host,
        doors,
        &mut book,
        &mut journal,
        to.as_deref(),
        doors.now_ms(),
    );
    keep_in(&mut book, &journal);
    let mut receipt_error = settled.refused;
    if let Err(why) = write_book(&dir, &book) {
        receipt_error.get_or_insert(why);
    }
    let moved_count = panes.iter().filter(|pane| pane.ok).count();
    let applied = Applied {
        key: journal.key.clone(),
        from: journal.from.clone(),
        to,
        switched_default,
        default_ms,
        panes,
        receipts: settled.written,
        receipt_error,
        total_ms: began.elapsed().as_millis(),
    };
    doors.log(&format!(
        "account-switch: {by} {reason} {}→{} default={} ({}ms) panes={} moved={moved_count} \
         receipts={} owed={} total={}ms",
        applied.from.as_deref().unwrap_or("system"),
        applied.to.as_deref().unwrap_or("system"),
        applied.switched_default,
        applied.default_ms,
        applied.panes.len(),
        applied.receipts,
        journal.owes(),
        applied.total_ms
    ));
    Ok(applied)
}

/// The policy as it stands NOW, asked at the last door before a pane is
/// touched (astra R2). The selection and the checks before it take time,
/// and the setting, the candidate, the logins or the pane's own verdict may
/// have moved under them: the approval must still stand — the mode allows
/// this road, and the pane's account and the landing name the logins they
/// named when the yes was given — and the plan read again must still send
/// this worker — the same seat, attempt and coordinator generation — to the
/// same landing.
fn still_moves(
    host: &dyn Host,
    doors: &dyn SwitchDoors,
    row: &WalledRow,
    approval: &Approval<'_>,
) -> Result<(), String> {
    let now = plan_with(host, doors.situation(doors.now_ms())?);
    approval.stands(now.mode, &now.logins)?;
    let landing = approval.landing;
    let still = now.moves().any(|held| {
        held.worker == row.worker
            && held.term == row.term
            && held.dispatch == row.dispatch
            && held.generation == row.generation
            && matches!(&held.verdict, Decision::Switch { to, .. } if to == landing)
    });
    if still {
        Ok(())
    } else {
        Err(format!(
            "the plan read again no longer sends this pane to {landing} — not moved"
        ))
    }
}

/// What the rest and the close left, inside the restore line.
enum Walked {
    /// The old pane's program is gone, and the restore road seated this
    /// many (0 or 1).
    Seated(usize),
    /// The old pane's program outlived the wait; the ledger's row holds it
    /// since the rest.
    Lingered,
}

/// Move ONE walled pane to its approval's landing — the same worker id,
/// dispatch, checkout and conversation (t-7538, condition 6). Everything
/// that can be known beforehand is checked before anything is touched: the
/// seat and the attempt the plan saw, the login the pane was launched as,
/// the pane's own model, effort and permission mode (R5), the ledger's word,
/// and the approval and the policy as they stand now (R2). Then, in the
/// restore line, the ledger rests the worker at the fence — only while the
/// window still sees the wall and the approval still stands — with the
/// pane's tuning and the program running in it on its row (R3), `note`
/// writes that down, the pane closes, and the restore road seats that one
/// worker again once the old program is gone. A program that outlives the
/// wait stays held on the row, against every restore, until a look sees it
/// gone. A pane that fails a check is left exactly as it was.
fn move_pane(
    host: &dyn Host,
    doors: &dyn SwitchDoors,
    row: &WalledRow,
    approval: Option<&Approval<'_>>,
    key: &str,
    now_ms: i64,
    note: &mut dyn FnMut() -> Result<(), String>,
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
    let checked = (|| -> Result<(u32, SwitchRest, PaneLogin, &Approval<'_>), String> {
        let approval = approval.ok_or("the window did not record the account the pane runs as")?;
        let now = crate::orchestration::worker_now(&row.worker)
            .ok_or("the ledger no longer knows the worker")?;
        let Some(term) = now.term else {
            return Err("the worker's seat is not a pane of this window".to_string());
        };
        if term != row.term || !now.state.is_live() {
            return Err("the worker moved seats since the plan".to_string());
        }
        let pane = host
            .pane_login(term)
            .filter(|held| held.account == approval.from)
            .ok_or("the window did not record the login the pane runs as")?;
        let leader = crate::orchestration::leader_term_of_team(&now.team)
            .ok_or("the worker's team has no leader pane to restore under")?;
        let tuning = pane_tuning(host, term).ok_or(
            "no conversation the window can read in the pane — a restore would start it empty",
        )?;
        let (model, effort, mode) = known_tuning(&tuning)?;
        relaunch_keeps_permission(&mode, &doors.overrides())?;
        let rest = SwitchRest {
            worker: row.worker.clone(),
            dispatch: row.dispatch.clone(),
            generation: row.generation,
            session: tuning.session.id.clone(),
            model: relaunch_model(now.model.as_deref(), &model),
            effort,
            exit: host.exit_witness(term),
        };
        crate::orchestration::switch_move_ready(&rest, now_ms)?;
        still_moves(host, doors, row, approval)?;
        Ok((leader, rest, pane, approval))
    })();
    let (leader, rest, pane, approval) = match checked {
        Ok(checked) => checked,
        Err(why) => {
            moved.why = Some(why);
            moved.ms = began.elapsed().as_millis();
            return moved;
        }
    };
    let landing = approval.landing;
    let overrides = doors.overrides();
    let actor = host.actor_for(leader);
    let walked = crate::orchestration::in_restore_line(|line| -> Result<Walked, String> {
        crate::orchestration::rest_worker_for_switch(
            host,
            &rest,
            &pane,
            // The approval as it stands at the moment of the rest, asked at
            // the fence beside the wall (astra R2).
            &|| {
                let now = doors.situation(doors.now_ms())?;
                approval.stands(now.mode, &now.logins)
            },
            &|| doors.now_ms(),
            now_ms,
        )?;
        // Rested: written down before the pane closes. A journal that
        // cannot be written is said in the log and the move goes on — a
        // sleeping worker beside its own live pane is the one state this
        // road must never leave behind, and the program in the pane is held
        // on the ledger's row, not here.
        if let Err(why) = note() {
            doors.log(&format!("account-switch: {why}"));
        }
        crate::orchestration::switch_rested(&row.worker, switch_nudge(landing), key);
        // The old pane goes only now: the ledger has the worker asleep, so
        // its exit settles nothing — and the new one opens only once the old
        // program is gone, so the conversation never has two writers.
        match host.close_gone(row.term) {
            PaneExit::Gone => Ok(Walked::Seated(crate::orchestration::reseat_one_in_line(
                line,
                host,
                overrides,
                leader,
                actor.as_deref(),
                &row.worker,
            ))),
            // Held since the rest, on the ledger's row: no restore — this
            // window's, a restart's, a door's or the grace's — opens the
            // conversation beside a program that did not leave.
            PaneExit::Lingering(_) => Ok(Walked::Lingered),
        }
    });
    let landed = crate::orchestration::worker_now(&row.worker);
    moved.to_term = landed.as_ref().and_then(|now| now.term);
    moved.to_account = moved.to_term.and_then(|term| host.pane_account(term));
    match walked {
        Err(why) => moved.why = Some(why),
        Ok(Walked::Lingered) => {
            moved.why = Some(
                "the old pane's program outlived the wait; the worker sleeps with its attempt \
                 open, and nothing opens its conversation again until that program is gone"
                    .to_string(),
            );
        }
        Ok(Walked::Seated(restored)) => {
            moved.ok = restored > 0
                && landed
                    .as_ref()
                    .is_some_and(|now| now.term.is_some() && now.state.is_live())
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
/// started with. One receipt, in the ledger's voice, `by: person`; a
/// receipt the ledger refuses stays owed in the journal and the answer
/// says so (astra R4).
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
    let mut journal = Journal {
        key: format!("person-{now_ms}-{}", to.unwrap_or("system")),
        by: "person".to_string(),
        reason: "picked".to_string(),
        from,
        to: to.map(str::to_string),
        default: true,
        landed: false,
        observed_percent: observed,
        observed_window: None,
        generation: None,
        panes: Vec::new(),
        moved: 0,
        began_ms: now_ms,
    };
    let mut book = open_book(doors)?;
    keep_in(&mut book, &journal);
    write_before_the_first_effect(&dir, &book)?;
    if let Err(why) = doors.select(to) {
        book.switches.retain(|held| held.key != journal.key);
        if let Err(unwritten) = write_book(&dir, &book) {
            doors.log(&format!("account-switch: {unwritten}"));
        }
        return Err(why);
    }
    doors.selected(to, now_ms);
    journal.landed = true;
    let default_ms = began.elapsed().as_millis();
    let settled = settle(host, doors, &mut book, &mut journal, to, doors.now_ms());
    keep_in(&mut book, &journal);
    let mut receipt_error = settled.refused;
    if let Err(why) = write_book(&dir, &book) {
        receipt_error.get_or_insert(why);
    }
    doors.log(&format!(
        "account-switch: person picked {}→{} ({}ms) receipts={} owed={}",
        journal.from.as_deref().unwrap_or("system"),
        to.unwrap_or("system"),
        default_ms,
        settled.written,
        journal.owes()
    ));
    Ok(Applied {
        key: journal.key.clone(),
        from: journal.from.clone(),
        to: journal.to.clone(),
        switched_default: true,
        default_ms,
        panes: Vec::new(),
        receipts: settled.written,
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
        let logins = [("a".to_string(), "login-of-a".to_string())];
        let same = token_of(
            AutoSwitchMode::Ask,
            Some("a"),
            &decision,
            &[row("w-1", 7, Some("a"))],
            &logins,
        );
        assert_eq!(
            same,
            token_of(
                AutoSwitchMode::Ask,
                Some("a"),
                &decision,
                &[row("w-1", 7, Some("a"))],
                &logins
            )
        );
        assert_ne!(
            same,
            token_of(
                AutoSwitchMode::Auto,
                Some("a"),
                &decision,
                &[row("w-1", 7, Some("a"))],
                &logins
            )
        );
        assert_ne!(
            same,
            token_of(
                AutoSwitchMode::Ask,
                Some("b"),
                &decision,
                &[row("w-1", 7, Some("a"))],
                &logins
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
                &[row("w-1", 7, Some("a"))],
                &logins
            )
        );
        assert_ne!(
            same,
            token_of(
                AutoSwitchMode::Ask,
                Some("a"),
                &decision,
                &[row("w-1", 8, Some("a"))],
                &logins
            )
        );
        assert_ne!(
            same,
            token_of(AutoSwitchMode::Ask, Some("a"), &decision, &[], &logins)
        );
        let mut regenerated = row("w-1", 7, Some("a"));
        regenerated.generation = Some(2);
        assert_ne!(
            same,
            token_of(
                AutoSwitchMode::Ask,
                Some("a"),
                &decision,
                &[regenerated],
                &logins
            )
        );
        // A pane the table stopped sending anywhere is another plan.
        let mut stayed = row("w-1", 7, Some("a"));
        stayed.verdict = Decision::Stay {
            why: "no_candidate",
        };
        assert_ne!(
            same,
            token_of(
                AutoSwitchMode::Ask,
                Some("a"),
                &decision,
                &[stayed],
                &logins
            )
        );
        // The same id naming another login is another plan (astra R6): a
        // yes given while `a` named one login is refused once it names
        // another, whatever the numbers say.
        assert_ne!(
            same,
            token_of(
                AutoSwitchMode::Ask,
                Some("a"),
                &decision,
                &[row("w-1", 7, Some("a"))],
                &[("a".to_string(), "login-of-somebody-else".to_string())]
            )
        );
        // The order the store lists them in is not a fact.
        let two = [
            ("a".to_string(), "login-of-a".to_string()),
            ("b".to_string(), "login-of-b".to_string()),
        ];
        let owt = [two[1].clone(), two[0].clone()];
        assert_eq!(
            token_of(AutoSwitchMode::Ask, Some("a"), &decision, &[], &two),
            token_of(AutoSwitchMode::Ask, Some("a"), &decision, &[], &owt)
        );
    }

    /// The relaunch carries the model the pane really ran (astra R5): its
    /// own id. The row's spelling stays only where it names exactly that id
    /// with a context suffix the transcript does not show; a row that named
    /// a family word lends the id its suffix and nothing more — `opus` is
    /// not taken for the version the pane ran.
    #[test]
    fn a_relaunch_names_the_panes_own_model_and_keeps_only_the_rows_context_suffix() {
        let pane = "claude-opus-5-5";
        assert_eq!(
            relaunch_model(Some("claude-opus-5-5[1m]"), pane),
            "claude-opus-5-5[1m]"
        );
        assert_eq!(
            relaunch_model(Some("opus[1m]"), pane),
            "claude-opus-5-5[1m]"
        );
        assert_eq!(relaunch_model(Some("opus"), pane), "claude-opus-5-5");
        assert_eq!(
            relaunch_model(Some("Claude-Opus-5-5"), pane),
            "Claude-Opus-5-5"
        );
        assert_eq!(
            relaunch_model(Some("claude-fable-5-1"), pane),
            "claude-opus-5-5"
        );
        assert_eq!(relaunch_model(Some("fable[1m]"), pane), "claude-opus-5-5");
        assert_eq!(relaunch_model(None, pane), "claude-opus-5-5");
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
            .expect("the bare account's reading landed")
            .usage;
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
            .expect("B's reading")
            .usage;
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
            let row = &mut held.get_mut("t7538-denied-c").expect("C's refusal").usage;
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

    /// A reading is its login's (astra R6). Account B is read as one login;
    /// then its row names another under the same id — the pending-identity
    /// "apply" choice, a re-login — and B has NO reading: not in the window
    /// that kept the number in memory, not in a window that loads the file
    /// cold, and never as a candidate on the old number. The id alone was
    /// the key before, and the old login's number stood for the new one.
    #[test]
    fn a_reading_is_its_logins_and_another_login_under_the_same_id_has_none() {
        let config = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let b = inactive_fixture(config.path(), "t7538-login-b");
        let scanned = scan_claude_account_usage_with(
            &b,
            |_| {
                accounts::AccountLogin::Found(
                    "login-document".to_string(),
                    accounts::LoginFrom::File,
                )
            },
            |_, _| Ok(a_reading(10.0)),
        );
        assert!(land_claude_account_usage(
            config.path(),
            data.path(),
            &b,
            scanned.usage
        ));
        let gauge_of_b = |gauges: Vec<AccountGauge>| {
            gauges
                .into_iter()
                .find(|gauge| gauge.id == b.id)
                .expect("B's row")
        };
        let read = gauge_of_b(claude_account_gauges_of(
            &accounts::read_store(config.path()),
            data.path(),
        ));
        assert_eq!(read.status, "ok");
        assert!(!read.windows.is_empty());
        // The same id now names another login.
        let mut store = accounts::read_store(config.path());
        store
            .accounts
            .iter_mut()
            .filter(|account| account.id == b.id)
            .for_each(|account| account.account_uuid = Some("somebody-else".to_string()));
        std::fs::write(
            config.path().join(accounts::ACCOUNT_STORE_FILE),
            serde_json::to_string(&store).unwrap(),
        )
        .unwrap();
        let warm = gauge_of_b(claude_account_gauges_of(
            &accounts::read_store(config.path()),
            data.path(),
        ));
        assert!(warm.windows.is_empty(), "{warm:?}");
        assert_eq!(warm.status, "unknown");
        let now = accounts::read_store(config.path());
        let relogged = now
            .accounts
            .iter()
            .find(|account| account.id == b.id)
            .expect("B's row");
        let cold = load_account_readings(data.path());
        assert!(cold.contains_key(&b.id), "the file keeps what it read");
        assert!(
            reading_of(&cold, relogged).is_none(),
            "a cold window took the old login's number for the new one"
        );
        // The table has nobody to move to on it.
        let source = AccountGauge {
            id: "t7538-login-b-selected".to_string(),
            org_type: Some("claude_max".to_string()),
            identity: Some("the-selected-login".to_string()),
            windows: vec![zerocode_core::account_autoswitch::GaugeWindow {
                kind: "session".to_string(),
                used_percent: 95,
                resets_at_ms: Some(epoch_ms_now() + 3_600_000),
            }],
            observed_at_ms: epoch_ms_now() - 60_000,
            status: "ok".to_string(),
        };
        assert_eq!(
            best_candidate(
                &source.id,
                &[source.clone(), warm],
                None,
                CLAUDE_ACCOUNT_AUTOSWITCH.default_moves_at_percent,
                epoch_ms_now()
            ),
            None
        );
    }

    /// A figure outside 0–100 is no reading an account may be chosen by
    /// (astra R6). The server's -5% was clamped to 0% — "100% room" — and
    /// its 130% to 100%, and both went out as `ok`; now the read is
    /// `invalid`, carries no window, and the table passes the account over
    /// for a switch that would otherwise have gone to it.
    #[test]
    fn an_inactive_read_outside_the_range_is_invalid_and_never_room() {
        let config = tempfile::tempdir().unwrap();
        let b = inactive_fixture(config.path(), "t7538-range-b");
        let now = epoch_ms_now();
        let source = AccountGauge {
            id: "t7538-range-b-selected".to_string(),
            org_type: Some("claude_max".to_string()),
            identity: Some("the-selected-login".to_string()),
            windows: vec![zerocode_core::account_autoswitch::GaugeWindow {
                kind: "session".to_string(),
                used_percent: 95,
                resets_at_ms: Some(now + 3_600_000),
            }],
            observed_at_ms: now - 60_000,
            status: "ok".to_string(),
        };
        for said in [-5.0_f32, 130.0] {
            let scanned = scan_claude_account_usage_with(
                &b,
                |_| {
                    accounts::AccountLogin::Found(
                        "login-document".to_string(),
                        accounts::LoginFrom::File,
                    )
                },
                |_, _| Ok(a_reading(said)),
            );
            assert_eq!(scanned.usage.status, ACCOUNT_READING_INVALID, "{said}");
            assert!(scanned.usage.session.is_none(), "{said}");
            let gauge = account_gauge_of(&b, Some(&scanned.usage));
            let decided = decide(&Question {
                source: &source.id,
                gauges: &[source.clone(), gauge],
                model: None,
                last_switch_ms: None,
                walled: false,
                now_ms: now,
            });
            assert_eq!(
                decided,
                Decision::Stay {
                    why: "no_candidate"
                },
                "{said}"
            );
        }
    }
}
