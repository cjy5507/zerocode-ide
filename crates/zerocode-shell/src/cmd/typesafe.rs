//! TypeSafe (Jev) settings IPC — the key and the one switch.
use crate::api_routers::{self, Keychain, RouterRefusal};
use crate::jev_scope;
use crate::typesafe_settings::{self, DayBudget, SeatNumbers, TypeSafeCheck, TypeSafeSettings};
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
#[tauri::command(async)]
pub(crate) fn typesafe_settings() -> Result<TypeSafeSettings, RouterRefusal> {
    settings_now()
}

/// Keep the key in the keychain item every zo reads.
#[tauri::command(async)]
pub(crate) fn save_typesafe_key(key: String) -> Result<TypeSafeSettings, RouterRefusal> {
    typesafe_settings::save_key(&key, &Keychain::of_this_machine())?;
    settings_now()
}

/// Forget the saved key.
#[tauri::command(async)]
pub(crate) fn remove_typesafe_key() -> Result<TypeSafeSettings, RouterRefusal> {
    typesafe_settings::remove_key(&Keychain::of_this_machine())?;
    settings_now()
}

/// The day's Jev requests from this machine against the person's limit, for
/// the dashboard's strip (t-6243 D1): a file's length and the settings file,
/// no keychain and no process, so it rides every refresh.
#[tauri::command(async)]
pub(crate) fn jev_day() -> Result<DayBudget, RouterRefusal> {
    Ok(typesafe_settings::read_day(&settings_path()?))
}

/// Turn Jev on or off — the one switch the card and the dashboard wear
/// (docs/design/jev-settings-20260917.md §6.1). On consents every folder and
/// hands every feature its recommended mode; off sends nothing.
#[tauri::command(async)]
pub(crate) fn set_jev_enabled(on: bool) -> Result<TypeSafeSettings, RouterRefusal> {
    typesafe_settings::set_enabled(&settings_path()?, on)?;
    settings_now()
}

/// Move the routing classifier — the gate in front of the routing seat. Its
/// own door, because it is zo's routing setting rather than a row of the use
/// table, and its words are the classifier's own.
#[tauri::command(async)]
pub(crate) fn set_route_classifier(mode: String) -> Result<TypeSafeSettings, RouterRefusal> {
    typesafe_settings::set_classifier(&settings_path()?, &mode)?;
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

/// Ask the installed zo whether System One answers with the key it would use:
/// `zo decision-shadow check --json`, the shadow's own question about a task
/// nobody wrote. A check nothing answered is still an answer (its failure
/// token); only a zo that printed no answer at all is an error.
#[tauri::command]
pub(crate) async fn check_typesafe_key() -> Result<TypeSafeCheck, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let home = dirs::home_dir().ok_or_else(|| "no home directory".to_string())?;
        let bin = crate::zo_companion::zo_path_under(&home);
        if !bin.exists() {
            return Err(format!("zo not installed at {}", bin.display()));
        }
        let output = crate::proc::quiet_command(&bin)
            .args(typesafe_settings::ZO_KEY_CHECK_ARGS)
            .output()
            .map_err(|error| error.to_string())?;
        typesafe_settings::read_check(&output.stdout).ok_or_else(|| {
            format!(
                "zo {} exited {}: {}",
                typesafe_settings::ZO_KEY_CHECK_ARGS.join(" "),
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            )
        })
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
    // zo's home, where every project's records are: the settings file's
    // folder, the same one `jev_day` counts the day in.
    let zo_home = settings_path()
        .map_err(|refusal| refusal.message)?
        .parent()
        .map(std::path::Path::to_path_buf)
        .ok_or_else(|| "zo's settings file has no folder".to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
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
