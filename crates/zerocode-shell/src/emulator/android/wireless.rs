//! Connecting a real Android phone without a cable (T6, 2026-11-09): the
//! person opens 「페어링 코드로 기기 페어링」 on the phone, the window finds it
//! with `adb mdns services`, the person types the six digits the phone shows,
//! and the window pairs (`adb pair`) and waits for `adb` to connect the phone
//! by the name its wireless debugging announces — connecting it itself when
//! `adb` does not.
//!
//! Only the person does this, from the window's settings; no verb an agent
//! sends names it. An address is paired only while the phone offers pairing on
//! it, and connected only where the same phone offers its connection. The phone
//! is the phone its cable showed (`real_android::phone_identity`), so the
//! person's answer for it, and its id, do not change with the way it is
//! connected; a different phone is never taken in its place.
//!
//! What fails comes back as a code the window words in the person's language:
//! `wireless_code_shape`, `wireless_not_offered`, `wireless_pair_failed`
//! (with what `adb` said), `wireless_not_connected`, `wireless_timed_out`,
//! `wireless_no_adb`.

use std::path::Path;
use std::time::{Duration, Instant};

use serde::Serialize;
use zerocode_core::agent_emulator::real_android::{
    MdnsService, android_phone_handle, mdns_serial, paired, pairing_code_ok, parse_mdns_services,
    phone_identity,
};

use crate::emulator::capability::{COMMAND_OUTPUT_BYTES, SHORT_COMMAND_TIMEOUT};
use crate::emulator::process::run_bounded;

/// How long `adb pair` may take: the phone answers within seconds or not at all.
/// `adb mdns services` and `adb connect` take the emulator's short command
/// time ([`SHORT_COMMAND_TIMEOUT`]).
const PAIR_WITHIN: Duration = Duration::from_secs(30);
/// How long the window waits for `adb` to connect a phone it just paired.
const CONNECT_WAIT: Duration = Duration::from_secs(10);
/// How often it looks.
const LOOK_EVERY: Duration = Duration::from_millis(500);

/// A phone offering pairing now: where, and which phone (its id).
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub(crate) struct WirelessOffer {
    pub(crate) addr: String,
    pub(crate) device: String,
}

/// A phone the window paired and sees connected.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub(crate) struct WirelessPaired {
    pub(crate) device: String,
    pub(crate) name: String,
    /// Whether the person already allowed this phone (over its cable, say).
    pub(crate) allowed: bool,
}

/// One `adb` command through the emulator's bounded runner: what it said on
/// both streams, whatever its exit — `adb pair` says why it failed on its
/// output.
fn ask(adb: &Path, args: &[&str], within: Duration) -> Result<String, String> {
    let mut command = crate::proc::quiet_command(adb);
    command.args(args);
    let started = Instant::now();
    let output = run_bounded(command, within, COMMAND_OUTPUT_BYTES).map_err(|error| {
        // The runner words its failure for a person; the window wants a code,
        // and a run that used up its time is the phone not answering.
        if started.elapsed() >= within {
            "wireless_timed_out".to_string()
        } else {
            format!("wireless_no_adb: {error}")
        }
    })?;
    let mut said = output.stdout_text();
    said.push_str(&String::from_utf8_lossy(&output.stderr));
    Ok(said)
}

fn services(adb: &Path) -> Result<Vec<MdnsService>, String> {
    Ok(parse_mdns_services(&ask(
        adb,
        &["mdns", "services"],
        SHORT_COMMAND_TIMEOUT,
    )?))
}

/// The phones offering pairing now, by address and id.
pub(super) fn offers_on(adb: &Path) -> Result<Vec<WirelessOffer>, String> {
    Ok(services(adb)?
        .into_iter()
        .filter(|service| service.pairing)
        .filter_map(|service| {
            Some(WirelessOffer {
                device: android_phone_handle(mdns_serial(&service.name)?),
                addr: service.addr,
            })
        })
        .collect())
}

/// The up row of the phone whose own serial is `serial`, over any connection:
/// its id and the model `adb` names.
fn connected(adb: &Path, serial: &str, also: Option<&str>) -> Option<(String, Option<String>)> {
    super::adb_rows(adb)
        .ok()?
        .into_iter()
        .find(|row| {
            row.is_up()
                && !row.is_emulator()
                && (phone_identity(&row.serial) == serial || also == Some(row.serial.as_str()))
        })
        .map(|row| (android_phone_handle(&row.serial), row.model))
}

fn wait_connected(
    adb: &Path,
    serial: &str,
    also: Option<&str>,
    within: Duration,
) -> Option<(String, Option<String>)> {
    let started = Instant::now();
    loop {
        if let Some(found) = connected(adb, serial, also) {
            return Some(found);
        }
        if started.elapsed() >= within {
            return None;
        }
        std::thread::sleep(LOOK_EVERY);
    }
}

/// Pair the phone offering pairing at `addr` with `code`, then wait for it to
/// be connected — by `adb` on its own, or by this window on the connection the
/// same phone offers.
pub(super) fn pair_on(
    adb: &Path,
    addr: &str,
    code: &str,
    connect_wait: Duration,
) -> Result<(String, Option<String>), String> {
    if !pairing_code_ok(code) {
        return Err("wireless_code_shape".to_string());
    }
    let offered = services(adb)?;
    let serial = offered
        .iter()
        .find(|service| service.pairing && service.addr == addr)
        .and_then(|service| mdns_serial(&service.name))
        .ok_or_else(|| "wireless_not_offered".to_string())?;
    let said = ask(adb, &["pair", addr, code], PAIR_WITHIN)?;
    if !paired(&said) {
        let why = said
            .lines()
            .find(|line| line.contains("Failed") || line.contains("error"))
            .unwrap_or_default()
            .trim()
            .to_string();
        return Err(format!("wireless_pair_failed: {why}"));
    }
    if let Some(found) = wait_connected(adb, serial, None, connect_wait) {
        return Ok(found);
    }
    // `adb` did not connect it on its own: the connection the same phone offers.
    let connect = services(adb)?
        .into_iter()
        .find(|service| !service.pairing && mdns_serial(&service.name) == Some(serial))
        .map(|service| service.addr)
        .ok_or_else(|| "wireless_not_connected".to_string())?;
    let _ = ask(adb, &["connect", &connect], SHORT_COMMAND_TIMEOUT)?;
    wait_connected(adb, serial, Some(&connect), connect_wait)
        .ok_or_else(|| "wireless_not_connected".to_string())
}

fn adb() -> Result<std::path::PathBuf, String> {
    super::android_sdk()
        .map(|sdk| sdk.adb)
        .map_err(|_| "wireless_no_adb".to_string())
}

/// The phones offering pairing now (the person opened 「페어링 코드로 기기
/// 페어링」 on them).
#[tauri::command(async)]
pub(crate) async fn phone_wireless_offers(
    webview: tauri::Webview,
) -> Result<Vec<WirelessOffer>, String> {
    crate::from_the_main_webview(&webview)?;
    let adb = adb()?;
    tauri::async_runtime::spawn_blocking(move || offers_on(&adb))
        .await
        .map_err(|error| error.to_string())?
}

/// Pair the phone offering pairing at `addr` with the code the person read off
/// it, and connect it.
#[tauri::command(async)]
pub(crate) async fn phone_wireless_pair(
    webview: tauri::Webview,
    addr: String,
    code: String,
) -> Result<WirelessPaired, String> {
    crate::from_the_main_webview(&webview)?;
    let adb = adb()?;
    let (device, model) =
        tauri::async_runtime::spawn_blocking(move || pair_on(&adb, &addr, &code, CONNECT_WAIT))
            .await
            .map_err(|error| error.to_string())??;
    Ok(WirelessPaired {
        allowed: crate::emulator::wda::allowed_now(&device),
        name: model.unwrap_or_else(|| device.clone()),
        device,
    })
}

// The fake `adb` below is a `#!/bin/sh` script made executable, so these tests
// build only where a shell script can run — the Windows cross-check
// (`just win-check`) refused `std::os::unix` here.
#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    const SERIAL: &str = "R5CTSYNTH01";
    const PAIRING: &str = "192.0.2.10:37123";
    const CONNECT: &str = "192.0.2.10:41001";

    /// A fake `adb` whose answers are files beside it; `auto` makes `devices -l`
    /// list the phone by its mDNS name once it is paired, as `adb` does on its own.
    fn fake(auto: bool, pair_ok: bool) -> (tempfile::TempDir, std::path::PathBuf) {
        let directory = tempfile::tempdir().unwrap();
        let adb = directory.path().join("adb");
        std::fs::write(
            &adb,
            format!(
                r#"#!/bin/sh
cd "$(dirname "$0")" || exit 1
printf '%s\n' "$*" >> commands
case "$*" in
  'mdns services') printf 'List of discovered mdns services\nadb-{SERIAL}-AbC123\t_adb-tls-pairing._tcp.\t{PAIRING}\nadb-{SERIAL}-AbC123\t_adb-tls-connect._tcp.\t{CONNECT}\n' ;;
  'pair {PAIRING} '*) if [ {pair_ok} = true ]; then touch paired; printf 'Successfully paired to {PAIRING} [guid=adb-{SERIAL}-AbC123]\n'; else printf 'Failed: Wrong password or connection was dropped.\n'; exit 1; fi ;;
  'connect {CONNECT}') touch connected; printf 'connected to {CONNECT}\n' ;;
  'devices -l')
    printf 'List of devices attached\n'
    if [ -f paired ] && [ {auto} = true ]; then printf 'adb-{SERIAL}-AbC123._adb-tls-connect._tcp device product:synth model:Synthetic_Pixel transport_id:3\n'; fi
    if [ -f connected ]; then printf '{CONNECT} device product:synth model:Synthetic_Pixel transport_id:4\n'; fi ;;
  *) exit 1 ;;
esac
"#
            ),
        )
        .unwrap();
        std::fs::set_permissions(&adb, std::fs::Permissions::from_mode(0o700)).unwrap();
        (directory, adb)
    }

    fn commands(directory: &tempfile::TempDir) -> Vec<String> {
        std::fs::read_to_string(directory.path().join("commands"))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    #[test]
    fn a_phone_offering_pairing_is_found_by_its_own_id() {
        let (_directory, adb) = fake(true, true);
        assert_eq!(
            offers_on(&adb).expect("found"),
            [WirelessOffer {
                addr: PAIRING.to_string(),
                device: android_phone_handle(SERIAL),
            }],
            "one offer — the connection service is no pairing offer"
        );
    }

    #[test]
    fn a_paired_phone_is_the_phone_its_cable_showed_and_adb_connects_it_by_name() {
        let (directory, adb) = fake(true, true);
        let (device, model) =
            pair_on(&adb, PAIRING, "042519", Duration::from_secs(2)).expect("paired");
        assert_eq!(
            device,
            android_phone_handle(SERIAL),
            "the cable's id: one phone"
        );
        assert_eq!(model.as_deref(), Some("Synthetic Pixel"));
        assert!(
            !commands(&directory)
                .iter()
                .any(|line| line.starts_with("connect")),
            "adb connected it by name; the window connected nothing: {:?}",
            commands(&directory)
        );
    }

    #[test]
    fn a_phone_adb_does_not_connect_is_connected_on_its_own_offer() {
        let (directory, adb) = fake(false, true);
        let (device, _) =
            pair_on(&adb, PAIRING, "042519", Duration::from_millis(600)).expect("connected");
        assert!(commands(&directory).contains(&format!("connect {CONNECT}")));
        // Connected by address: an address alone names no phone, so it is its own id.
        assert_eq!(device, android_phone_handle(CONNECT));
    }

    #[test]
    fn a_wrong_code_an_odd_code_and_an_address_offering_nothing_pair_nothing() {
        let (directory, adb) = fake(true, false);
        assert_eq!(
            pair_on(&adb, PAIRING, "04251", Duration::from_millis(100)),
            Err("wireless_code_shape".to_string())
        );
        assert!(
            commands(&directory).is_empty(),
            "an odd code asks adb nothing"
        );
        assert_eq!(
            pair_on(
                &adb,
                "192.0.2.99:37123",
                "042519",
                Duration::from_millis(100)
            ),
            Err("wireless_not_offered".to_string())
        );
        assert!(
            !commands(&directory)
                .iter()
                .any(|line| line.starts_with("pair"))
        );
        let failed = pair_on(&adb, PAIRING, "000000", Duration::from_millis(100)).unwrap_err();
        assert!(
            failed.starts_with("wireless_pair_failed") && failed.contains("Wrong password"),
            "{failed}"
        );
    }
}
