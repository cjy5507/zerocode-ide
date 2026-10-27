//! TypeSafe (Jev) settings IPC — the key and the one switch.
use crate::api_routers::{self, Keychain, RouterRefusal};
use crate::jev_scope;
use crate::typesafe_settings::{
    self, CustomDecisions, DayBudget, DecisionDraft, DecisionRefusal, DraftCheck, SeatNumbers,
    SummonRowRefusal, TypeSafeCheck, TypeSafeSettings,
};
use tauri::State;

use crate::*;

fn settings_path() -> Result<std::path::PathBuf, RouterRefusal> {
    api_routers::zo_settings_path()
        .ok_or_else(|| RouterRefusal::from("설정 경로를 찾을 수 없습니다".to_string()))
}

fn settings_now() -> Result<TypeSafeSettings, RouterRefusal> {
    let keychain = Keychain::of_this_machine();
    typesafe_settings::read_settings(&settings_path()?, &keychain, keychain.keeps_keys())
}

/// Whether a key could be kept here, whether one is saved, and the switch.
/// The worker rows say whether each agent could be summoned now, so the
/// agents a summons tunes are looked at first — at a look's freshness, the
/// picker's — and the answer peeks at what that look left.
#[tauri::command(async)]
pub(crate) fn typesafe_settings() -> Result<TypeSafeSettings, RouterRefusal> {
    for agent in crate::summon_lineup::tunable_agents() {
        let _ = crate::readiness_runtime::ensure(agent, zerocode_core::readiness::Purpose::Display);
    }
    settings_now()
}

/// Keep the key in the keychain item every zo reads.
#[tauri::command(async)]
pub(crate) fn save_typesafe_key(
    key: String,
    provider_identity: Option<String>,
    provider: Option<judgment_provider::Provider>,
) -> Result<TypeSafeSettings, RouterRefusal> {
    if let Some(provider) = provider {
        let provider = provider
            .endpoint_validated()
            .map_err(|reason| RouterRefusal::from(reason.to_string()))?;
        if !provider.requires_auth {
            return Err("provider_auth_not_required".to_string().into());
        }
        typesafe_settings::save_candidate_key(
            &settings_path()?,
            &provider,
            provider_identity.as_deref(),
            &key,
            &Keychain::of_this_machine(),
        )?;
    } else {
        typesafe_settings::save_selected_key(
            &settings_path()?,
            provider_identity.as_deref(),
            &key,
            &Keychain::of_this_machine(),
        )?;
    }
    settings_now()
}

/// Forget the saved key.
#[tauri::command(async)]
pub(crate) fn remove_typesafe_key(
    provider_identity: Option<String>,
) -> Result<TypeSafeSettings, RouterRefusal> {
    typesafe_settings::remove_selected_key(
        &settings_path()?,
        provider_identity.as_deref(),
        &Keychain::of_this_machine(),
    )?;
    settings_now()
}

/// The day's Jev requests from this machine against the person's limit, for
/// the dashboard's strip (t-6243 D1): a file's length and the settings file,
/// no keychain and no process, so it rides every refresh.
#[tauri::command(async)]
pub(crate) fn jev_day() -> Result<DayBudget, RouterRefusal> {
    Ok(typesafe_settings::read_day(&settings_path()?))
}

/// Read local review samples. Opening or refreshing this view never asks a model.
#[tauri::command(async)]
pub(crate) fn jev_review(
    state: State<'_, AppState>,
    seat: String,
    seed: Option<u64>,
    scope: Option<jev_scope::Scope>,
) -> Result<serde_json::Value, String> {
    use zerocode_core::jev::learning::{self, store::Store};
    let feature =
        zerocode_core::jev::jev_use(&seat).ok_or_else(|| "unknown Jev feature".to_string())?;
    let path = settings_path().map_err(|error| error.message)?;
    let home = path
        .parent()
        .ok_or_else(|| "settings file has no parent".to_string())?;
    let settings = serde_json::Value::Object(api_routers::read_zo_settings_root(&path)?);
    let workspace = zerocode_core::jev::door::resolved_path(&state.active_root());
    let every_project = matches!(scope, Some(jev_scope::Scope::Projects));
    let enabled = zerocode_core::jev::promote::label_drafts_wanted(&settings);
    let review = Store::at(home)
        .review(
            Some(&seat),
            (!every_project).then_some(workspace.as_str()),
            5,
            1,
            seed.unwrap_or(0),
        )
        .map_err(|error| error.to_string())?;
    let gate = zerocode_core::jev::door::JevSettings::from_root(&settings);
    Ok(serde_json::json!({"schemaVersion":1,"enabled":enabled,
        "captureEnabled":enabled && gate.enabled && gate.consents(&workspace) && feature.mode_in(&settings).asks(),
        "cases":review.cases,"invalid":review.invalid,"total":review.total,
        "capacity":learning::store::MAX_CASES,"samples":review.samples}))
}

#[tauri::command(async)]
pub(crate) fn set_jev_review_enabled(on: bool) -> Result<(), String> {
    typesafe_settings::set_label_drafts(&settings_path().map_err(|error| error.message)?, on)
}

/// A person reviewed this exact case and question; the store validates both again.
#[tauri::command(async)]
pub(crate) fn jev_review_outcome(
    case_id: String,
    question: String,
    correct: bool,
    note: String,
) -> Result<(), String> {
    use zerocode_core::jev::learning::{Outcome, Reviewer, store::Store};
    let path = settings_path().map_err(|error| error.message)?;
    let home = path
        .parent()
        .ok_or_else(|| "settings file has no parent".to_string())?;
    Store::at(home)
        .outcome(Outcome {
            case_id,
            question,
            correct,
            note,
            reviewer: Reviewer::Human,
            at: crate::now_epoch_ms(),
        })
        .map_err(|error| error.to_string())
}

/// Turn Jev on or off — the one switch the card and the dashboard wear
/// (docs/design/jev-settings-20260917.md §6.1). On consents every folder and
/// preserves saved decisions and otherwise uses recommended modes; off sends nothing.
#[tauri::command(async)]
pub(crate) fn set_jev_enabled(on: bool) -> Result<TypeSafeSettings, RouterRefusal> {
    typesafe_settings::set_enabled(&settings_path()?, on)?;
    settings_now()
}

#[tauri::command(async)]
pub(crate) fn update_builtin_decision(
    id: String,
    action: String,
    expected: zerocode_core::jev::builtin::Decision,
    draft: Option<zerocode_core::jev::builtin::Decision>,
) -> Result<TypeSafeSettings, RouterRefusal> {
    if action == "save"
        && let Some(draft) = draft.as_ref().filter(|draft| !draft.questions.is_empty())
    {
        zerocode_core::jev::builtin::validate_against(
            &draft.questions,
            &load_builtin_questions(&id)?,
        )
        .map_err(|error| RouterRefusal::from(error.to_string()))?;
    }
    typesafe_settings::update_builtin_decision(&settings_path()?, &id, &action, &expected, draft)?;
    settings_now()
}

#[tauri::command(async)]
pub(crate) fn builtin_decision_definition(
    id: String,
) -> Result<typesafe_settings::BuiltinDecisionEditor, RouterRefusal> {
    typesafe_settings::builtin_decision_editor(&settings_path()?, &id, load_builtin_questions(&id)?)
        .map_err(RouterRefusal::from)
}

fn load_builtin_questions(
    id: &str,
) -> Result<Vec<zerocode_core::jev::builtin::QuestionDefinition>, RouterRefusal> {
    use zerocode_core::jev::builtin;
    builtin::seat(id).ok_or_else(|| RouterRefusal::from("builtin_decision_missing".to_string()))?;
    if let Some(questions) = builtin::default_questions(id) {
        return Ok(questions);
    }
    let unavailable = || RouterRefusal::from("builtin_decision_definition_unavailable".to_string());
    let home = dirs::home_dir().ok_or_else(unavailable)?;
    let bin = crate::zo_companion::zo_path_under(&home);
    let output = crate::proc::quiet_command(&bin)
        .args(["jev", "definition", id, "--json"])
        .output()
        .map_err(|_| unavailable())?;
    let body: serde_json::Value =
        serde_json::from_slice(&output.stdout).map_err(|_| unavailable())?;
    if !output.status.success() || body.get("id").and_then(serde_json::Value::as_str) != Some(id) {
        return Err(unavailable());
    }
    // An older companion's fingerprint is not a question definition.
    serde_json::from_value(body.get("questions").cloned().ok_or_else(unavailable)?)
        .map_err(|_| unavailable())
}

/// Move the routing classifier — the gate in front of the routing seat. Its
/// own door, because it is zo's routing setting rather than a row of the use
/// table, and its words are the classifier's own.
#[tauri::command(async)]
pub(crate) fn set_route_classifier(mode: String) -> Result<TypeSafeSettings, RouterRefusal> {
    typesafe_settings::set_classifier(&settings_path()?, &mode)?;
    settings_now()
}

/// Write one agent's worker model at one difficulty — a field left empty
/// follows the lineup, both empty clear the row — checked against the
/// lineup this window holds; the answer's row is what the next summons runs.
#[tauri::command(async)]
pub(crate) fn set_summon_profile(
    agent: String,
    difficulty: String,
    model: Option<String>,
    effort: Option<String>,
) -> Result<TypeSafeSettings, SummonRowRefusal> {
    let word = |value: Option<String>| {
        value
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    };
    let written = zerocode_core::summon_difficulty::Written {
        model: word(model),
        effort: word(effort),
    };
    let lineup = crate::summon_lineup::snapshot();
    typesafe_settings::set_summon_profile(
        &settings_path()?,
        &agent,
        &difficulty,
        &written,
        lineup.lineups.get(&agent),
    )?;
    Ok(settings_now()?)
}

/// Read every agent's lineup again now, zo asked afresh, and answer with
/// what that read holds: the worker rows' 「목록 새로고침」.
#[tauri::command(async)]
pub(crate) fn refresh_summon_lineup() -> Result<TypeSafeSettings, RouterRefusal> {
    crate::summon_lineup::refresh_and_wait();
    settings_now()
}

/// Pin the model every Jev request names, or unpin it with an empty word
/// (`smart.jevModel`, t-6187). Its own door, like the classifier's: no seat's
/// switch, and a model id rather than a mode.
#[tauri::command(async)]
pub(crate) fn set_jev_model(model: String) -> Result<TypeSafeSettings, RouterRefusal> {
    typesafe_settings::set_model(&settings_path()?, &model)?;
    settings_now()
}

/// zo's config home, where a person's own decisions are kept beside zo's
/// settings file — the home zo reads them from.
fn decisions_home() -> Result<std::path::PathBuf, RouterRefusal> {
    settings_path()?
        .parent()
        .map(std::path::Path::to_path_buf)
        .ok_or_else(|| RouterRefusal::from("설정 경로를 찾을 수 없습니다".to_string()))
}

fn decisions_now() -> Result<CustomDecisions, DecisionRefusal> {
    let root =
        api_routers::read_zo_settings_root(&settings_path()?).map_err(RouterRefusal::from)?;
    Ok(typesafe_settings::custom_decisions_at(
        &decisions_home()?,
        &serde_json::Value::Object(root),
    )?)
}

/// The decisions a person made, the deleted ones still to be restored, and
/// the folders a decision may be narrowed to (t-62170 stage 3).
#[tauri::command(async)]
pub(crate) fn list_custom_decisions() -> Result<CustomDecisions, DecisionRefusal> {
    decisions_now()
}

/// Core's check of a draft, with nothing kept.
#[tauri::command(async)]
pub(crate) fn check_custom_decision(draft: DecisionDraft) -> DraftCheck {
    typesafe_settings::check_custom_decision(draft)
}

/// Keep a draft: a new decision recording before anything acts on it, an
/// edit as its next version with its mode kept. The store checks it again
/// under its lock.
#[tauri::command(async)]
pub(crate) fn save_custom_decision(
    draft: DecisionDraft,
) -> Result<CustomDecisions, DecisionRefusal> {
    typesafe_settings::save_custom_decision_at(&decisions_home()?, draft, crate::now_epoch_ms())?;
    decisions_now()
}

/// Delete a decision. Its records stay, and the store keeps it to be
/// restored for its days.
#[tauri::command(async)]
pub(crate) fn delete_custom_decision(id: String) -> Result<CustomDecisions, DecisionRefusal> {
    typesafe_settings::delete_custom_decision_at(&decisions_home()?, &id, crate::now_epoch_ms())?;
    decisions_now()
}

/// Bring a deleted decision back as it was, mode and version.
#[tauri::command(async)]
pub(crate) fn restore_custom_decision(id: String) -> Result<CustomDecisions, DecisionRefusal> {
    zerocode_core::jev::custom::store::restore(&decisions_home()?, &id)?;
    decisions_now()
}

/// Move a decision to another of the modes a decision offers.
#[tauri::command(async)]
pub(crate) fn set_custom_decision_mode(
    id: String,
    mode: String,
) -> Result<CustomDecisions, DecisionRefusal> {
    typesafe_settings::set_custom_decision_mode_at(
        &decisions_home()?,
        &id,
        &mode,
        crate::now_epoch_ms(),
    )?;
    decisions_now()
}

/// Select `provider`. With `expected_provider_identity` the write lands only
/// while the settings still select that provider — what a preflight's
/// `activeProviderIdentity` names — and is refused as `settings_changed`
/// with nothing written otherwise; without it, it lands as it always did.
#[tauri::command(async)]
pub(crate) fn set_judgment_provider(
    provider: judgment_provider::Provider,
    expected_provider_identity: Option<String>,
    key: Option<String>,
) -> Result<TypeSafeSettings, RouterRefusal> {
    let path = settings_path()?;
    match (
        expected_provider_identity.as_deref(),
        key.as_deref().map(str::trim).filter(|key| !key.is_empty()),
    ) {
        (Some(expected), Some(key)) => {
            typesafe_settings::set_provider_with_key_if_identity(
                &path,
                provider,
                expected,
                key,
                &Keychain::of_this_machine(),
            )?;
        }
        (None, Some(_)) => return Err("settings_changed".to_string().into()),
        (Some(expected), None) => {
            typesafe_settings::set_provider_if_identity(&path, provider, expected, None)?;
        }
        (None, None) => typesafe_settings::set_provider(&path, provider)?,
    }
    super::managed_judgment::settings_changed();
    settings_now()
}

/// What this machine's records say of the selected judgment model's real
/// use: the newest request a feature put to the client the settings select
/// now ([`crate::systemone::latest::read`], t-53792). Reads the records'
/// newest bytes; sends nothing and moves no setting.
#[tauri::command(async)]
pub(crate) fn judgment_latest_request() -> crate::systemone::latest::LatestRequest {
    crate::systemone::latest::read(
        &crate::systemone::Wire::of_this_machine(),
        crate::now_epoch_ms(),
    )
}

/// Ask a provider the person has not applied yet the preflight's three
/// questions, with the key its own endpoint keeps, and change nothing
/// ([`typesafe_settings::preflight_provider`]). An answer is a connection
/// that speaks the native schema, not a measure of accuracy.
#[tauri::command]
pub(crate) async fn preflight_judgment_provider(
    provider: judgment_provider::Provider,
    expected_provider_identity: String,
    key: Option<String>,
) -> Result<typesafe_settings::ProviderPreflight, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let path = settings_path().map_err(|error| error.message)?;
        let keys = Keychain::of_this_machine();
        let wire = crate::systemone::Wire::of_this_machine();
        match key.as_deref() {
            Some(key) => typesafe_settings::preflight_provider_with_key(
                &path,
                provider,
                &expected_provider_identity,
                &keys,
                &wire,
                Some(key),
            ),
            None => typesafe_settings::preflight_provider(
                &path,
                provider,
                &expected_provider_identity,
                &keys,
                &wire,
            ),
        }
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) async fn judgment_models(
    provider: judgment_provider::Provider,
    key: Option<String>,
) -> Result<Vec<judgment_provider::Model>, String> {
    use crate::api_routers::RouterKeys;

    let provider = provider
        .endpoint_validated()
        .map_err(|reason| reason.to_string())?;
    let builder = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(8))
        .redirect(reqwest::redirect::Policy::none());
    let builder = if provider.kind == judgment_provider::ProviderKind::Local {
        builder.no_proxy()
    } else {
        builder
    };
    let client = builder.build().map_err(|_| "transport".to_string())?;
    let mut request = client.get(provider.api_url("models"));
    if provider.requires_auth {
        let key = match key.filter(|key| !key.trim().is_empty()) {
            Some(key) => Some(key),
            None => Keychain::of_this_machine()
                .read(&typesafe_settings::provider_keychain_service(&provider))
                .map_err(|reason| reason.message)?,
        }
        .filter(|key| !key.trim().is_empty())
        .ok_or_else(|| "no_key".to_string())?;
        request = request.bearer_auth(key.trim());
    }
    let mut response = request.send().await.map_err(|_| "transport".to_string())?;
    if !response.status().is_success() {
        return Err(crate::systemone::token_for(response.status().as_u16()));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "transport".to_string())?
    {
        if chunk.len() > judgment_provider::MAX_RESPONSE_BYTES.saturating_sub(bytes.len()) {
            return Err(judgment_provider::Refusal::TooLarge.token().to_string());
        }
        bytes.extend_from_slice(&chunk);
    }
    judgment_provider::read_models(&bytes).map_err(|reason| reason.to_string())
}

/// Ask the installed zo whether System One answers with the key it would use:
/// `zo decision-shadow check --json`, the shadow's own question about a task
/// nobody wrote. A check nothing answered is still an answer (its failure
/// token); only a zo that printed no answer at all is an error.
#[tauri::command]
pub(crate) async fn check_typesafe_key(
    provider_identity: Option<String>,
) -> Result<TypeSafeCheck, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let path = settings_path().map_err(|error| error.message)?;
        let provider =
            typesafe_settings::selected_provider(&path).map_err(|error| error.message)?;
        typesafe_settings::require_provider(&provider, provider_identity.as_deref())?;
        let home = dirs::home_dir().ok_or_else(|| "no home directory".to_string())?;
        let bin = crate::zo_companion::zo_path_under(&home);
        if !bin.exists() {
            return Err(format!("zo not installed at {}", bin.display()));
        }
        let output = crate::proc::quiet_command(&bin)
            .args(typesafe_settings::key_check_args(&provider))
            .output()
            .map_err(|error| error.to_string())?;
        let check = typesafe_settings::read_check(&output.stdout).ok_or_else(|| {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if provider.kind != judgment_provider::ProviderKind::Jev
                && stderr.contains("unknown argument")
                && stderr.contains(typesafe_settings::ZO_CHECK_PROVIDER_FLAG)
            {
                return "provider_client_outdated".to_string();
            }
            format!(
                "zo {} exited {}: {}",
                typesafe_settings::ZO_KEY_CHECK_ARGS.join(" "),
                output.status,
                stderr.trim()
            )
        })?;
        let current = typesafe_settings::selected_provider(&path).map_err(|error| error.message)?;
        if current != provider {
            return Err("settings_changed".to_string());
        }
        typesafe_settings::checked_provider(&provider, check)
    })
    .await
    .map_err(|error| error.to_string())?
}

/// Ask the installed zo what every seat's ledger says: `zo jev summary
/// --json` (docs/design/jev-settings-20260917.md §5). A zo too old to know
/// the verb answers nothing this reader understands, and the card draws its
/// switches without numbers rather than refusing to draw at all.
///
/// `recent` asks for each seat's last that many requests as well — the
/// dashboard's list under the table; the card leaves it out and the answer
/// carries none.
///
/// `scope` is which numbers (t-9091): the checkout the person is looking at
/// — the default — or every project with zo records, summed
/// ([`jev_scope::read`]). Either way the answer names what it counted, and
/// where else records are when the checkout has none.
#[tauri::command]
pub(crate) async fn jev_summary(
    state: State<'_, AppState>,
    recent: Option<usize>,
    scope: Option<jev_scope::Scope>,
) -> Result<jev_scope::JevReading, String> {
    // The screen seats append beside each walk's evidence under this
    // window's Computer Use sessions, not under a root zo knows; handed over
    // on the exec boundary so one counter counts every seat.
    let sessions = state
        .local_data_root()
        .join(crate::computer_use::evidence::SESSIONS_DIR);
    // The project whose ledgers are counted: zo's routing and recall seats
    // append under the project's own state directory, so the card asks about
    // the checkout the person is looking at, not the window's cwd.
    let workspace = state.active_root();
    // The folders the window knows: a checkout whose zo state only its
    // panes' rows filled has no zo session naming it (t-11349).
    let here = state.active();
    let mut known = vec![here.root.clone()];
    // zo's home, where every project's records are: the settings file's
    // folder, the same one `jev_day` counts the day in.
    let zo_home = settings_path()
        .map_err(|refusal| refusal.message)?
        .parent()
        .map(std::path::Path::to_path_buf)
        .ok_or_else(|| "zo's settings file has no folder".to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        if let Some(listed) = here
            .orchestrator
            .as_ref()
            .and_then(|orchestrator| orchestrator.list().ok())
        {
            known.extend(listed.into_iter().map(|worktree| worktree.path));
        }
        let home = dirs::home_dir().ok_or_else(|| "no home directory".to_string())?;
        let bin = crate::zo_companion::zo_path_under(&home);
        if !bin.exists() {
            return Err(format!("zo not installed at {}", bin.display()));
        }
        let ask = |project: &std::path::Path| -> Result<Vec<SeatNumbers>, String> {
            let mut command = crate::proc::quiet_command(&bin);
            command
                .args(typesafe_settings::ZO_JEV_SUMMARY_ARGS)
                .arg(typesafe_settings::ZO_JEV_SUMMARY_CWD_FLAG)
                .arg(project)
                .arg(typesafe_settings::ZO_JEV_SUMMARY_SESSIONS_FLAG)
                .arg(&sessions);
            if let Some(count) = recent.filter(|count| *count > 0) {
                command
                    .arg(typesafe_settings::ZO_JEV_SUMMARY_RECENT_FLAG)
                    .arg(count.to_string());
            }
            let output = command.output().map_err(|error| error.to_string())?;
            typesafe_settings::read_summary(&output.stdout).ok_or_else(|| {
                format!(
                    "zo {} exited {}: {}",
                    typesafe_settings::ZO_JEV_SUMMARY_ARGS.join(" "),
                    output.status,
                    String::from_utf8_lossy(&output.stderr).trim()
                )
            })
        };
        let places = jev_scope::Places {
            zo_home,
            sessions: sessions.clone(),
            temporary: jev_scope::temporary_roots(),
            known,
        };
        jev_scope::read(
            &ask,
            &places,
            &workspace,
            scope.unwrap_or_default(),
            crate::now_epoch_ms(),
        )
    })
    .await
    .map_err(|error| error.to_string())?
}
