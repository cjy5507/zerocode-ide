//! A real Android phone, driven over `adb` from inside it (t-93824): the verbs of
//! an emulator's road on a phone on a cable or the network, with no Mac pointer,
//! front app or clipboard, and no pane.
//!
//! The words, the list of what may be sent and the lock reader are
//! `zerocode_core::agent_emulator::real_android`'s. Here is what runs them, in
//! the shape the real iPhone's road (`emulator::wda`) set:
//!
//! - The road stays off until the person's answer on the window's card for this
//!   one phone, sealed by the iPhone road's own seal into the same list. Without
//!   it a verb is refused before `adb` is asked anything.
//! - Every verb reads `adb devices -l` for the phone's serial (a phone that has not
//!   allowed this computer, or is offline, is refused with why), then the phone's
//!   own lock, and only an open phone is read or pressed; a lock that cannot be
//!   read is a locked phone.
//! - Every command to a serial that is not an emulator's passes [`refusal`] —
//!   through [`adb_command`], or through `accessibility::run`, which the tree,
//!   display and screen reads share — so the `emu` console, a power key, a
//!   settings write or an app install never leaves the window, whoever builds
//!   the command.
//! - `phone-check` before the person's answer reads only `adb devices -l`; the
//!   phone itself is asked nothing.
//! - One phone takes one verb at a time; nothing else is shared.
//! - A press by number or by position is refused on a payment or a transfer
//!   whatever the desktop setting says (`wda::phone_policy`), waits for the
//!   screen it led to, and answers how it settled.
//! - On a screen that states money, only a vertical scroll, a press that backs
//!   out, typing, home, back and delete pass: a press, a short or sideways drag
//!   and `enter` are refused by the real iPhone's own guard
//!   (`real_phone::money_refusal`) on the tree read for that act.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use zerocode_core::agent_emulator::physical::{
    ANDROID_PHONE_REASON, AdbRow, LinkState, PhysicalDevice, parse_adb_rows,
};
use zerocode_core::agent_emulator::real_android::{
    self as words, LockRead, PHONE_UNAUTHORIZED, ROAD_ADB, adb_refusal, android_phone_handle,
    is_android_phone,
};
use zerocode_core::agent_emulator::real_phone::{
    CheckItem, LANDED_KEY, LANDED_UNCONFIRMED, MAC_POINTER_KEY, MoneyAct, PHONE_LOCKED,
    PHONE_NOT_ALLOWED, PHONE_SECURE_FIELD, PHONE_UNREACHABLE, ROAD_KEY, check_answer,
};
use zerocode_core::computer_use_protocol::{ProviderError, error_code};

use super::{AndroidSdk, NO_FOCUS_EXIT, SECURE_FIELD_EXIT, accessibility};
use crate::emulator::marks::{
    PinnedTap, Pressed, Proof, Settle, Settled, Settling, Snapshot, backend_error,
    refuse_guarded_at, shape,
};

/// The most a screenshot of a phone may weigh.
const SCREENSHOT_MAX_BYTES: u64 = 16 * 1024 * 1024;

/// What the road needs of the machine: the `adb` it runs, and the person's answer
/// for a phone. A test hands a fake `adb` and its own answer.
pub(super) struct Road {
    pub(super) sdk: AndroidSdk,
    pub(super) allowed: Box<dyn Fn(&str) -> bool + Send + Sync>,
}

/// This machine's road: its SDK's `adb` and the person's sealed answers.
fn road() -> Result<Arc<Road>, ProviderError> {
    let sdk = super::android_sdk().map_err(|search| backend_error(search.to_string()))?;
    Ok(Arc::new(Road {
        sdk,
        allowed: Box::new(crate::emulator::wda::allowed_now),
    }))
}

/// Why `args` may not go to `serial`: only a serial that is not an emulator's is
/// asked, and only [`adb_refusal`]'s list goes to one.
pub(super) fn refusal(serial: &str, args: &[&str]) -> Result<(), ProviderError> {
    if serial.starts_with(super::EMULATOR_SERIAL_PREFIX) {
        return Ok(());
    }
    match adb_refusal(args) {
        Some((code, sentence)) => Err(ProviderError::new(code, sentence)),
        None => Ok(()),
    }
}

/// The one way a command for one device is built on the Android roads: a real
/// phone's takes only what its list names, and a refusal spends no process.
pub(super) fn adb_command(
    adb: &Path,
    serial: &str,
    args: &[&str],
) -> Result<std::process::Command, ProviderError> {
    refusal(serial, args)?;
    let mut command = crate::proc::quiet_command(adb);
    command.args(["-s", serial]).args(args);
    Ok(command)
}

/// [`adb_command`] run with a deadline and a size cap: its output, or why not.
fn read_on(
    road: &Road,
    serial: &str,
    args: &[&str],
    max_bytes: Option<u64>,
) -> Result<Vec<u8>, ProviderError> {
    refusal(serial, args)?;
    // `accessibility::run` asks the list again; the code is kept from here.
    let mut full = vec!["-s", serial];
    full.extend_from_slice(args);
    accessibility::run(&road.sdk.adb, &full, max_bytes).map_err(backend_error)
}

// ---- Before any verb --------------------------------------------------------

/// One `adb devices -l`, the listing every verb reads its phone from.
fn listed(road: &Road) -> Result<Vec<AdbRow>, ProviderError> {
    let out = crate::proc::quiet_command(&road.sdk.adb)
        .args(["devices", "-l"])
        .output()
        .map_err(backend_error)?;
    if !out.status.success() {
        return Err(backend_error("adb did not list the devices; ask again"));
    }
    Ok(parse_adb_rows(&String::from_utf8_lossy(&out.stdout)))
}

/// The row of the phone with this id, whatever its state.
fn row_of(road: &Road, handle: &str) -> Result<AdbRow, ProviderError> {
    listed(road)?
        .into_iter()
        .find(|row| !row.is_emulator() && android_phone_handle(&row.serial) == handle)
        .ok_or_else(|| {
            ProviderError::new(
                PHONE_UNREACHABLE,
                format!(
                    "{PHONE_UNREACHABLE}: no Android phone {handle} is connected now; run list."
                ),
            )
        })
}

/// The serial of a phone `adb` lists as up; a phone that has not allowed this
/// computer, or is offline, is refused with why.
fn serial_of(road: &Road, handle: &str) -> Result<String, ProviderError> {
    let row = row_of(road, handle)?;
    if row.is_up() {
        Ok(row.serial)
    } else if row.is_unauthorized() {
        Err(ProviderError::new(
            PHONE_UNAUTHORIZED,
            words::unauthorized_sentence(),
        ))
    } else {
        Err(ProviderError::new(
            PHONE_UNREACHABLE,
            words::unreachable_sentence(&row.state),
        ))
    }
}

/// What the phone's window manager says of its lock, read now.
fn lock_now(road: &Road, serial: &str) -> LockRead {
    read_on(
        road,
        serial,
        &["shell", "dumpsys", "window", "policy"],
        Some(accessibility::MAX_OUTPUT_BYTES),
    )
    .map_or(LockRead::Unread, |dump| {
        words::lock_in_policy_dump(&String::from_utf8_lossy(&dump))
    })
}

fn refuse_if_locked(road: &Road, serial: &str) -> Result<(), ProviderError> {
    match lock_now(road, serial) {
        LockRead::Open => Ok(()),
        read => Err(ProviderError::new(
            PHONE_LOCKED,
            words::locked_sentence(read),
        )),
    }
}

/// One phone, one verb at a time.
fn gate(handle: &str) -> Arc<Mutex<()>> {
    static GATES: Mutex<Option<HashMap<String, Arc<Mutex<()>>>>> = Mutex::new(None);
    let mut gates = GATES.lock().unwrap_or_else(PoisonError::into_inner);
    Arc::clone(
        gates
            .get_or_insert_with(HashMap::new)
            .entry(handle.to_string())
            .or_default(),
    )
}

/// A verb's first steps, in this order: the person's answer (no process before
/// it), the phone's own gate, its serial and state, its lock. Then the verb.
fn drive<T>(
    road: &Road,
    handle: &str,
    verb: impl FnOnce(&Road, &str) -> Result<T, ProviderError>,
) -> Result<T, ProviderError> {
    if !(road.allowed)(handle) {
        return Err(ProviderError::new(
            PHONE_NOT_ALLOWED,
            words::not_allowed_sentence(handle),
        ));
    }
    let gate = gate(handle);
    let _one_at_a_time = gate.lock().unwrap_or_else(PoisonError::into_inner);
    let serial = serial_of(road, handle)?;
    refuse_if_locked(road, &serial)?;
    verb(road, &serial)
}

/// Run a blocking verb on this machine's road, off the async threads.
async fn blocking<T: Send + 'static>(
    verb: impl FnOnce(&Road) -> Result<T, ProviderError> + Send + 'static,
) -> Result<T, ProviderError> {
    let road = road()?;
    tauri::async_runtime::spawn_blocking(move || verb(road.as_ref()))
        .await
        .map_err(backend_error)?
}

/// The guard a press on a real phone takes: the iPhone road's own.
fn policy() -> crate::computer_use::confirm::Policy {
    crate::emulator::wda::phone_policy()
}

/// The real iPhone's money guard on this phone's tree (`act` at the dump's own
/// pixels): one code and one sentence for both phones.
fn refuse_on_money_screen(snapshot: &Snapshot, act: MoneyAct) -> Result<(), ProviderError> {
    let tree = words::money_tree(&snapshot.raw, snapshot.screen.width, snapshot.screen.height);
    crate::emulator::wda::refuse_money(&tree, act)
}

/// What every act answers: sent, by this road, with no Mac pointer.
fn acted() -> Value {
    json!({ "performed": true, ROAD_KEY: ROAD_ADB, MAC_POINTER_KEY: false,
            LANDED_KEY: LANDED_UNCONFIRMED })
}

fn settled_act(settled: &Settled) -> Value {
    let mut answer = acted();
    answer[LANDED_KEY] = json!(settled.landed());
    answer[zerocode_core::agent_emulator::EMULATOR_SETTLE_KEY] = settled.said();
    answer
}

/// The screen as the marks read it, the phone's id as its identity: a phone
/// has no AVD, and its `emu` console is never asked.
fn snapshot_on(
    road: &Road,
    serial: &str,
    handle: &str,
    pressing: bool,
) -> Result<(Snapshot, (u32, u32)), ProviderError> {
    super::marks_snapshot_as(&road.sdk, serial, pressing, Some(handle))
        .map(|(snapshot, size, _)| (snapshot, size))
        .map_err(backend_error)
}

/// Read the pressed screen until it stops changing, as a simulator's and an
/// iPhone's press does (`Settling`, its quiet window and ceiling): a read that
/// differs from `before` or no longer holds the pressed control (`stands`) has
/// moved. A phone's dump takes about two seconds, so the ceiling often comes with
/// one read; a read that failed decides nothing.
fn settle(
    road: &Road,
    serial: &str,
    handle: &str,
    before: &Snapshot,
    tapped: Instant,
    stands: impl Fn(&[zerocode_core::computer_use_protocol::marks::ElementFace]) -> bool,
) -> Settled {
    use zerocode_core::agent_emulator::{EMULATOR_SETTLE_CEILING_MS, EMULATOR_SETTLE_QUIET_MS};
    let poll = Duration::from_millis(zerocode_core::computer_use::COMPUTER_SETTLE_POLL_MS);
    let mut settling = Settling::new(
        Duration::from_millis(EMULATOR_SETTLE_QUIET_MS),
        Duration::from_millis(EMULATOR_SETTLE_CEILING_MS),
    );
    let found = shape(&before.faces);
    let mut last = None;
    loop {
        let asked = Instant::now();
        let read = snapshot_on(road, serial, handle, false)
            .ok()
            .map(|(snapshot, _)| snapshot);
        let seen = read.as_ref().map(|snapshot| shape(&snapshot.faces));
        if let Some(snapshot) = read {
            if seen.as_ref() != Some(&found) || !stands(&snapshot.faces) {
                settling.moved();
            }
            last = Some(snapshot);
        }
        let settle = settling.read(seen, tapped.elapsed());
        if settle != Settle::Reading {
            return Settled {
                settle,
                reads: settling.reads(),
                ms: u64::try_from(tapped.elapsed().as_millis()).unwrap_or(u64::MAX),
                last,
            };
        }
        std::thread::sleep(poll.saturating_sub(asked.elapsed()));
    }
}

/// The first app package a tree names, in preorder.
fn package_of(tree: &Value) -> Option<String> {
    let mut pending = vec![tree];
    while let Some(node) = pending.pop() {
        if let Some(package) = node.get("packageName").and_then(Value::as_str) {
            return Some(package.to_string());
        }
        if let Some(children) = node.get("children").and_then(Value::as_array) {
            pending.extend(children.iter().rev());
        }
    }
    None
}

// ---- The verbs (blocking halves; a test drives these on a fake adb) ----------

pub(super) fn tree_on(road: &Road, handle: &str) -> Result<Value, ProviderError> {
    drive(road, handle, |road, serial| {
        let tree = accessibility::snapshot(&road.sdk.adb, serial).map_err(backend_error)?;
        let tree = serde_json::to_value(tree).map_err(backend_error)?;
        let package = package_of(&tree);
        Ok(json!({ "tree": tree, "app": { "package": package }, ROAD_KEY: ROAD_ADB }))
    })
}

pub(super) fn marks_on(road: &Road, handle: &str) -> Result<Snapshot, ProviderError> {
    drive(road, handle, |road, serial| {
        snapshot_on(road, serial, handle, false).map(|(snapshot, _)| snapshot)
    })
}

/// A press by number: the screen is read again and must still hold the mark,
/// its app and its words; a payment or a transfer stops here with nobody asked.
pub(super) fn click_on(
    road: &Road,
    handle: &str,
    request: &PinnedTap,
) -> Result<Pressed, ProviderError> {
    drive(road, handle, |road, serial| {
        let (snapshot, size) = snapshot_on(road, serial, handle, true)?;
        request.on_device(handle, || {
            request.perform_on_tree(&snapshot.faces, snapshot.screen, policy(), |x, y| {
                refuse_on_money_screen(
                    &snapshot,
                    MoneyAct::Press(x * snapshot.screen.width, y * snapshot.screen.height),
                )?;
                super::tap_at(&road.sdk, serial, x, y, size).map_err(backend_error)
            })
        })?;
        let settled = settle(road, serial, handle, &snapshot, Instant::now(), |faces| {
            request.stands_in(faces)
        });
        Ok(Pressed {
            proof: Proof::Sent,
            settled: Some(settled),
        })
    })
}

/// A tap by position: the screen is read first, and a guarded control under the
/// point stops it as it stops a press by number.
pub(super) fn tap_on(road: &Road, handle: &str, x: f64, y: f64) -> Result<Value, ProviderError> {
    drive(road, handle, |road, serial| {
        let (snapshot, size) = snapshot_on(road, serial, handle, true)?;
        let (at_x, at_y) = super::device_point(x, y, size);
        refuse_guarded_at(
            &snapshot.faces,
            snapshot.screen,
            f64::from(at_x),
            f64::from(at_y),
            policy(),
        )?;
        refuse_on_money_screen(&snapshot, MoneyAct::Press(f64::from(at_x), f64::from(at_y)))?;
        super::tap_at(&road.sdk, serial, x, y, size).map_err(backend_error)?;
        let settled = settle(road, serial, handle, &snapshot, Instant::now(), |_| true);
        Ok(settled_act(&settled))
    })
}

/// A drag: the screen is read first, and on a screen that states money only a
/// vertical scroll goes.
pub(super) fn swipe_on(
    road: &Road,
    handle: &str,
    from: (f64, f64),
    to: (f64, f64),
    ms: Option<u32>,
) -> Result<Value, ProviderError> {
    drive(road, handle, |road, serial| {
        let (snapshot, _) = snapshot_on(road, serial, handle, true)?;
        let (width, height) = (snapshot.screen.width, snapshot.screen.height);
        refuse_on_money_screen(
            &snapshot,
            MoneyAct::Swipe {
                from: (from.0 * width, from.1 * height),
                to: (to.0 * width, to.1 * height),
            },
        )?;
        super::swipe_normalized(&road.sdk, serial, from, to, ms).map_err(backend_error)?;
        Ok(acted())
    })
}

/// A scroll that stops (`scroll`): read first and judged as a drag is, so on a
/// screen that states money only a vertical one goes; then the finger moves a
/// phase a line and rests before it lifts.
pub(super) fn scroll_on(
    road: &Road,
    handle: &str,
    from: (f64, f64),
    to: (f64, f64),
) -> Result<Value, ProviderError> {
    drive(road, handle, |road, serial| {
        let (snapshot, _) = snapshot_on(road, serial, handle, true)?;
        let (width, height) = (snapshot.screen.width, snapshot.screen.height);
        refuse_on_money_screen(
            &snapshot,
            MoneyAct::Swipe {
                from: (from.0 * width, from.1 * height),
                to: (to.0 * width, to.1 * height),
            },
        )?;
        super::scroll_normalized(&road.sdk, serial, from, to).map_err(backend_error)?;
        Ok(acted())
    })
}

/// Typing, through the text helper, Hangul and all: only into a field the phone
/// says has the focus, and never a password field.
pub(super) fn text_on(road: &Road, handle: &str, text: &str) -> Result<Value, ProviderError> {
    if !crate::emulator::ios::sendable_text(text) {
        return Err(ProviderError::invalid_argument(
            crate::emulator::ios::UNSENDABLE_TEXT,
        ));
    }
    let typed = super::text_road(text).map_err(ProviderError::invalid_argument)?;
    drive(road, handle, |road, serial| {
        let ran = super::type_line(&road.sdk, serial, &typed, true).map_err(backend_error)?;
        match ran.status.code() {
            Some(0) => Ok(acted()),
            Some(SECURE_FIELD_EXIT) => Err(ProviderError::new(
                PHONE_SECURE_FIELD,
                words::secure_field_sentence(),
            )),
            Some(NO_FOCUS_EXIT) => Err(ProviderError::invalid_argument(
                "no field has the focus on the phone; press the field first. Nothing was typed.",
            )),
            code => Err(backend_error(
                super::insert_outcome(code, &ran.stderr, false)
                    .err()
                    .unwrap_or_default(),
            )),
        }
    })
}

/// A button: home, back, enter or delete. Its power, lock and sleep keys are
/// refused before anything is asked.
pub(super) fn button_on(road: &Road, handle: &str, name: &str) -> Result<Value, ProviderError> {
    if words::is_power_button(name) {
        return Err(power_refused());
    }
    let code = words::phone_key(name).ok_or_else(|| {
        let names: Vec<&str> = words::PHONE_KEYS.iter().map(|(name, _)| *name).collect();
        ProviderError::new(
            error_code::UNSUPPORTED_CAPABILITY,
            format!(
                "a real Android phone takes {}, not `{name}`",
                names.join(", ")
            ),
        )
    })?;
    drive(road, handle, |road, serial| {
        // `enter` may send a form; home, back and delete send nothing.
        if name == "enter" {
            let (snapshot, _) = snapshot_on(road, serial, handle, true)?;
            refuse_on_money_screen(&snapshot, MoneyAct::Enter)?;
        }
        super::android_input_unlocked(&road.sdk, serial, &["keyevent", code])
            .map_err(backend_error)?;
        Ok(acted())
    })
}

pub(super) fn screenshot_on(road: &Road, handle: &str) -> Result<Vec<u8>, ProviderError> {
    drive(road, handle, |road, serial| {
        read_on(
            road,
            serial,
            &["exec-out", "screencap", "-p"],
            Some(SCREENSHOT_MAX_BYTES),
        )
    })
}

fn power_refused() -> ProviderError {
    let (code, sentence) = adb_refusal(&["reboot"]).unwrap_or_default();
    ProviderError::new(code, sentence)
}

/// `rotate` on a real phone: a rotation is written into its settings, which are
/// the person's.
pub(crate) fn rotate_refused() -> ProviderError {
    let (code, sentence) = adb_refusal(&["shell", "settings", "put"]).unwrap_or_default();
    ProviderError::new(code, sentence)
}

/// A verb that is an emulator's alone (`open`, a boot, a shutdown).
pub(crate) fn emulator_only() -> ProviderError {
    let (code, sentence) = adb_refusal(&["emu"]).unwrap_or_default();
    ProviderError::new(code, sentence)
}

// ---- The verbs, as the door calls them ----------------------------------------

pub(crate) async fn tree(handle: &str) -> Result<Value, ProviderError> {
    let handle = handle.to_string();
    blocking(move |road| tree_on(road, &handle)).await
}

pub(crate) async fn marks_snapshot(handle: &str) -> Result<Snapshot, ProviderError> {
    let handle = handle.to_string();
    blocking(move |road| marks_on(road, &handle)).await
}

pub(crate) async fn click_mark(handle: &str, request: PinnedTap) -> Result<Pressed, ProviderError> {
    let handle = handle.to_string();
    blocking(move |road| click_on(road, &handle, &request)).await
}

pub(crate) async fn tap(handle: &str, x: f64, y: f64) -> Result<Value, ProviderError> {
    let handle = handle.to_string();
    blocking(move |road| tap_on(road, &handle, x, y)).await
}

pub(crate) async fn swipe(
    handle: &str,
    from: (f64, f64),
    to: (f64, f64),
    ms: Option<u32>,
) -> Result<Value, ProviderError> {
    let handle = handle.to_string();
    blocking(move |road| swipe_on(road, &handle, from, to, ms)).await
}

pub(crate) async fn scroll(
    handle: &str,
    from: (f64, f64),
    to: (f64, f64),
) -> Result<Value, ProviderError> {
    let handle = handle.to_string();
    blocking(move |road| scroll_on(road, &handle, from, to)).await
}

pub(crate) async fn text(handle: &str, text: &str) -> Result<Value, ProviderError> {
    let (handle, text) = (handle.to_string(), text.to_string());
    blocking(move |road| text_on(road, &handle, &text)).await
}

pub(crate) async fn button(handle: &str, name: &str) -> Result<Value, ProviderError> {
    let (handle, name) = (handle.to_string(), name.to_string());
    blocking(move |road| button_on(road, &handle, &name)).await
}

pub(crate) async fn screenshot(handle: &str) -> Result<Vec<u8>, String> {
    let handle = handle.to_string();
    blocking(move |road| screenshot_on(road, &handle))
        .await
        .map_err(|error| error.message)
}

// ---- The person's say ------------------------------------------------------------

/// `phone-install` for an Android phone: no build. The card once, for this phone
/// alone, then this window's text helper copied into the phone's /data/local/tmp.
pub(crate) async fn install(handle: &str) -> Result<Value, ProviderError> {
    let owned = handle.to_string();
    let (row, allowed) = blocking(move |road| {
        let row = row_of(road, &owned)?;
        Ok((row, (road.allowed)(&owned)))
    })
    .await?;
    if row.is_unauthorized() {
        return Err(ProviderError::new(
            PHONE_UNAUTHORIZED,
            words::unauthorized_sentence(),
        ));
    }
    if !row.is_up() {
        return Err(ProviderError::new(
            PHONE_UNREACHABLE,
            words::unreachable_sentence(&row.state),
        ));
    }
    if !allowed {
        let name = row
            .model
            .clone()
            .unwrap_or_else(|| zerocode_core::agent_emulator::physical::UNNAMED_ANDROID.into());
        let sentence = words::consent_sentence(&name);
        let args = json!({ "phone": name });
        let decision = tauri::async_runtime::spawn_blocking(move || {
            crate::computer_use::confirm::handoff_said(
                &sentence,
                words::CONSENT_KEY,
                args,
                crate::emulator::wda::CARD_WAIT_MS,
            )
        })
        .await
        .map_err(backend_error)?;
        crate::emulator::wda::refuse_unless_allowed(decision)?;
        crate::emulator::wda::commit_android(handle).map_err(backend_error)?;
    }
    let serial = row.serial;
    let gated = handle.to_string();
    let helper = blocking(move |road| {
        let gate = gate(&gated);
        let _one_at_a_time = gate.lock().unwrap_or_else(PoisonError::into_inner);
        super::insert_dex_ready(&road.sdk, &serial).map_err(backend_error)
    })
    .await?;
    Ok(json!({ "installed": true, "built": false, ROAD_KEY: ROAD_ADB, "textHelper": helper }))
}

/// The helper off one phone that `adb` reaches; what became of it.
fn take_helper_off(road: &Road, serial: &str) -> &'static str {
    let helper = super::insert_dex_on_device();
    let removed = adb_command(
        &road.sdk.adb,
        serial,
        &["shell", "rm", "-f", helper.as_str()],
    )
    .ok()
    .and_then(|mut command| command.output().ok())
    .is_some_and(|out| out.status.success());
    super::forget_insert_dex(serial);
    if removed {
        "the text helper was removed from the phone"
    } else {
        "the phone did not answer; the text helper stays in its /data/local/tmp until it is removed again"
    }
}

/// `phone-remove` for an Android phone: after the person's answer, the helper off
/// the phone and this phone's answer taken back.
pub(crate) async fn remove(handle: &str) -> Result<Value, ProviderError> {
    let owned = handle.to_string();
    let row = blocking(move |road| row_of(road, &owned)).await?;
    let name = row
        .model
        .clone()
        .unwrap_or_else(|| zerocode_core::agent_emulator::physical::UNNAMED_ANDROID.into());
    let sentence = words::remove_sentence(&name);
    let args = json!({ "phone": name });
    let decision = tauri::async_runtime::spawn_blocking(move || {
        crate::computer_use::confirm::handoff_said(
            &sentence,
            words::REMOVE_KEY,
            args,
            crate::emulator::wda::CARD_WAIT_MS,
        )
    })
    .await
    .map_err(backend_error)?;
    crate::emulator::wda::refuse_unless_allowed(decision)?;
    let note = if row.is_up() {
        let serial = row.serial;
        blocking(move |road| Ok(take_helper_off(road, &serial))).await?
    } else {
        "the phone is not reachable now; the text helper stays in its /data/local/tmp until it is connected and removed again"
    };
    crate::emulator::wda::withdraw_one(handle).map_err(backend_error)?;
    Ok(json!({ "removed": true, "phone": note }))
}

/// The person's 「지우기」: the helper off every allowed Android phone `adb`
/// reaches now. Blocks.
pub(in crate::emulator) fn take_off_all() -> Vec<Value> {
    let Ok(road) = road() else {
        return Vec::new();
    };
    listed(&road)
        .unwrap_or_default()
        .into_iter()
        .filter(|row| !row.is_emulator() && row.is_up())
        .filter_map(|row| {
            let handle = android_phone_handle(&row.serial);
            (road.allowed)(&handle)
                .then(|| json!({ "device": handle, "phone": take_helper_off(&road, &row.serial) }))
        })
        .collect()
}

/// `phone-check` for an Android phone: the person's answer, the cable, the lock.
pub(crate) async fn check(handle: &str) -> Result<Value, ProviderError> {
    let handle = handle.to_string();
    blocking(move |road| Ok(check_on(road, &handle))).await
}

pub(super) fn check_on(road: &Road, handle: &str) -> Value {
    let allowed = (road.allowed)(handle);
    let mut items = vec![CheckItem::new(
        "consent",
        Some(allowed),
        if allowed {
            "the person allowed this phone"
        } else {
            "not allowed: run phone-install, and the person answers a card"
        },
    )];
    match serial_of(road, handle) {
        Ok(_) if !allowed => {
            items.push(CheckItem::new(
                "connected",
                Some(true),
                "adb lists it as up",
            ));
            // Nothing goes to a phone the person has not allowed, a read of
            // its window manager neither.
            items.push(CheckItem::new(
                "phoneLocked",
                None,
                "not read: the phone is asked nothing before the person allows it",
            ));
        }
        Ok(serial) => {
            items.push(CheckItem::new(
                "connected",
                Some(true),
                "adb lists it as up",
            ));
            let read = lock_now(road, &serial);
            items.push(match read {
                LockRead::Open => CheckItem::new("phoneLocked", Some(true), "not locked"),
                LockRead::Locked => CheckItem::new(
                    "phoneLocked",
                    Some(false),
                    "locked: every press is refused until the person unlocks it",
                ),
                LockRead::Unread => CheckItem::new(
                    "phoneLocked",
                    None,
                    "unreadable: refused as locked until it reads",
                ),
            });
        }
        Err(error) => {
            items.push(CheckItem::new("connected", Some(false), error.message));
            items.push(CheckItem::new(
                "phoneLocked",
                None,
                "unreadable without a connection",
            ));
        }
    }
    check_answer(items)
}

/// `list`'s Android rows, told what stands: a phone is drivable once the person
/// allowed it and `adb` lists it as up.
pub(in crate::emulator) fn overlay(rows: &mut [PhysicalDevice]) {
    for row in rows {
        let allowed = row.device.as_deref().is_some_and(|handle| {
            is_android_phone(handle) && crate::emulator::wda::allowed_now(handle)
        });
        if allowed && row.state == LinkState::Connected {
            row.drivable = true;
            row.reason = ANDROID_PHONE_REASON;
        }
    }
}

#[cfg(all(test, unix))]
mod tests;
