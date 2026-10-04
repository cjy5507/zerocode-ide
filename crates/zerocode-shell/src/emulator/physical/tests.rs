//! The real-device probe: a tool is asked for a file and read from it, a tool
//! that fails or hangs is a reason and not "none", a reading is kept for a short
//! while by one probe at a time, and a caller never waits past its wait. Every
//! name below is made up.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use zerocode_core::agent_emulator::physical::{ANDROID_REASON, LinkState, PhysicalDevice};
use zerocode_core::computer_use::EmulatorPlatform;

use super::kept::{Kept, Reading};
use super::*;

/// A `devicectl` answer cut to what the reader uses: one phone the Mac reaches
/// and one it only knows.
#[cfg(unix)]
const DEVICECTL_JSON: &str = r#"{"info":{"jsonVersion":3,"outcome":"success"},"result":{"devices":[
{"identifier":"00000000-0000-0000-0000-0000000000a1","connectionProperties":{"tunnelState":"connected"},
 "deviceProperties":{"name":"Synthetic Phone A"},"hardwareProperties":{"platform":"iOS","marketingName":"Synthetic iPhone"}},
{"identifier":"00000000-0000-0000-0000-0000000000a2","connectionProperties":{"tunnelState":"unavailable"},
 "deviceProperties":{"name":"Synthetic Tablet B"},"hardwareProperties":{"platform":"iOS","marketingName":"Synthetic iPad"}}
]}}"#;

/// One row to hand a probe back, built by hand field by field: a probe's tests
/// must not stand on the reader of an `adb` listing.
fn phone() -> PhysicalDevice {
    PhysicalDevice {
        platform: EmulatorPlatform::Android,
        name: "Synthetic Pixel".to_string(),
        model: None,
        state: LinkState::Connected,
        drivable: false,
        reason: ANDROID_REASON,
    }
}

/// A leaked `Kept`, because the probe thread wants a `'static` one: the real
/// reading lives in a static, and a test's lives as long as the process.
fn kept() -> &'static Kept {
    Box::leak(Box::new(Kept::new()))
}

/// A probe that counts how often it ran, takes `takes` to answer and answers
/// `answer`.
fn probe(
    runs: &Arc<AtomicUsize>,
    takes: Duration,
    answer: Reading,
) -> impl FnOnce() -> Reading + Send + 'static {
    let runs = Arc::clone(runs);
    move || {
        runs.fetch_add(1, Ordering::SeqCst);
        std::thread::sleep(takes);
        answer
    }
}

/// A script standing in for a tool: it runs `body` and nothing else.
#[cfg(unix)]
fn fake_tool(scratch: &std::path::Path, name: &str, body: &str) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt as _;
    let path = scratch.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("a fake tool");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    path
}

#[cfg(unix)]
#[test]
fn a_devicectl_run_asks_for_a_json_file_and_reads_the_phones_from_it() {
    let scratch = tempfile::tempdir().expect("scratch");
    let asked = scratch.path().join("asked");
    let wrote = scratch.path().join("wrote");
    let fixture = scratch.path().join("fixture.json");
    std::fs::write(&fixture, DEVICECTL_JSON).expect("fixture");
    let tool = fake_tool(
        scratch.path(),
        "devicectl",
        &format!(
            "printf '%s\\n' \"$*\" > '{}'\nfor last; do :; done\ncp '{}' \"$last\"\nprintf '%s' \"$last\" > '{}'",
            asked.display(),
            fixture.display(),
            wrote.display()
        ),
    );
    let reading = ios_devices_with(crate::proc::quiet_command(&tool), Duration::from_secs(5));
    assert!(reading.is_ok(), "{reading:?}");
    let rows = reading.unwrap_or_default();
    let names: Vec<&str> = rows.iter().map(|row| row.name.as_str()).collect();
    assert_eq!(names, ["Synthetic Phone A", "Synthetic Tablet B"]);
    assert!(rows.iter().all(|row| !row.drivable));
    let said = std::fs::read_to_string(&asked).unwrap_or_default();
    assert!(
        said.starts_with("list devices --quiet --timeout 8 --json-output "),
        "devicectl was asked: {said:?}"
    );
    let file = std::fs::read_to_string(&wrote).unwrap_or_default();
    assert!(
        !file.is_empty() && !std::path::Path::new(&file).exists(),
        "the answer file {file:?} was left behind"
    );
}

#[cfg(unix)]
#[test]
fn a_devicectl_that_fails_or_writes_nonsense_is_a_reason_and_not_an_empty_list() {
    let scratch = tempfile::tempdir().expect("scratch");
    let failing = fake_tool(
        scratch.path(),
        "failing",
        "echo 'synthetic failure' >&2\nexit 1",
    );
    let nonsense = fake_tool(
        scratch.path(),
        "nonsense",
        "for last; do :; done\nprintf 'garbage' > \"$last\"",
    );
    let failed = ios_devices_with(crate::proc::quiet_command(&failing), Duration::from_secs(5));
    assert!(
        failed
            .as_ref()
            .is_err_and(|why| why.contains("devicectl") && why.contains("synthetic failure")),
        "{failed:?}"
    );
    let garbled = ios_devices_with(
        crate::proc::quiet_command(&nonsense),
        Duration::from_secs(5),
    );
    assert!(
        garbled
            .as_ref()
            .is_err_and(|why| why.contains("cannot read")),
        "{garbled:?}"
    );
}

#[cfg(unix)]
#[test]
fn a_devicectl_that_hangs_is_killed_at_its_cap() {
    let scratch = tempfile::tempdir().expect("scratch");
    let hanging = fake_tool(scratch.path(), "hanging", "exec sleep 30");
    let began = Instant::now();
    let reading = ios_devices_with(
        crate::proc::quiet_command(&hanging),
        Duration::from_millis(200),
    );
    assert!(reading.is_err(), "a hung tool answered: {reading:?}");
    assert!(
        began.elapsed() < Duration::from_secs(5),
        "it waited {:?}",
        began.elapsed()
    );
}

#[test]
fn a_kept_reading_serves_calls_inside_its_time() {
    let kept = kept();
    let runs = Arc::new(AtomicUsize::new(0));
    // An hour: no stall of a loaded machine makes the reading stale in between.
    let ttl = Duration::from_secs(3_600);
    let wait = Duration::from_secs(2);
    let first = kept.read(ttl, wait, probe(&runs, Duration::ZERO, Ok(vec![phone()])));
    assert_eq!(first, Ok(vec![phone()]));
    let second = kept.read(ttl, wait, probe(&runs, Duration::ZERO, Ok(Vec::new())));
    assert_eq!(
        second, first,
        "a second call inside the time starts no probe"
    );
    assert_eq!(runs.load(Ordering::SeqCst), 1);
}

#[test]
fn a_reading_past_its_time_is_read_again() {
    let kept = kept();
    let runs = Arc::new(AtomicUsize::new(0));
    // A millisecond is gone before the next call can look, however fast the
    // machine is; the call that waited for the probe still gets its answer.
    let ttl = Duration::from_millis(1);
    let wait = Duration::from_secs(2);
    let first = kept.read(ttl, wait, probe(&runs, Duration::ZERO, Ok(vec![phone()])));
    assert_eq!(first, Ok(vec![phone()]));
    std::thread::sleep(Duration::from_millis(50));
    let second = kept.read(ttl, wait, probe(&runs, Duration::ZERO, Ok(Vec::new())));
    assert_eq!(second, Ok(Vec::new()), "a call after the time reads again");
    assert_eq!(runs.load(Ordering::SeqCst), 2);
}

#[test]
fn a_caller_waits_no_longer_than_its_wait_and_the_next_call_has_the_answer() {
    let kept = kept();
    let runs = Arc::new(AtomicUsize::new(0));
    let ttl = Duration::from_secs(3_600);
    // The probe stays inside its tool until the test lets it go, so the wait
    // below can end only by running out.
    let (release, gate) = std::sync::mpsc::channel::<()>();
    let held = {
        let runs = Arc::clone(&runs);
        move || -> Reading {
            runs.fetch_add(1, Ordering::SeqCst);
            let _ = gate.recv_timeout(Duration::from_secs(30));
            Ok(vec![phone()])
        }
    };
    let began = Instant::now();
    let early = kept.read(ttl, Duration::from_millis(50), held);
    assert!(
        began.elapsed() < Duration::from_secs(10),
        "it waited {:?}",
        began.elapsed()
    );
    assert_eq!(early, Err(STILL_LOOKING.to_string()));
    let _ = release.send(());
    let later = loop {
        let answer = kept.read(
            ttl,
            Duration::from_millis(200),
            probe(&runs, Duration::ZERO, Ok(Vec::new())),
        );
        if answer != Err(STILL_LOOKING.to_string()) || began.elapsed() > Duration::from_secs(20) {
            break answer;
        }
    };
    assert_eq!(
        later,
        Ok(vec![phone()]),
        "the probe left running finished for this call"
    );
    assert_eq!(
        runs.load(Ordering::SeqCst),
        1,
        "no second probe was started"
    );
}

#[test]
fn many_calls_at_once_start_one_probe() {
    let kept = kept();
    let runs = Arc::new(AtomicUsize::new(0));
    let callers: Vec<_> = (0..8)
        .map(|_| {
            let runs = Arc::clone(&runs);
            std::thread::spawn(move || {
                kept.read(
                    Duration::from_secs(30),
                    Duration::from_secs(5),
                    probe(&runs, Duration::from_millis(150), Ok(vec![phone()])),
                )
            })
        })
        .collect();
    let answers: Vec<Reading> = callers
        .into_iter()
        .map(|caller| {
            caller
                .join()
                .unwrap_or_else(|_| Err("a caller panicked".to_string()))
        })
        .collect();
    assert_eq!(
        runs.load(Ordering::SeqCst),
        1,
        "eight callers started more than one probe"
    );
    assert!(
        answers.iter().all(|answer| answer == &Ok(vec![phone()])),
        "{answers:?}"
    );
}

#[test]
fn a_failed_probe_is_kept_for_its_time_too_and_a_panicking_one_leaves_a_reason() {
    let kept = kept();
    let runs = Arc::new(AtomicUsize::new(0));
    let ttl = Duration::from_secs(30);
    let wait = Duration::from_secs(2);
    let failure = Err("synthetic failure".to_string());
    assert_eq!(
        kept.read(ttl, wait, probe(&runs, Duration::ZERO, failure.clone())),
        failure
    );
    assert_eq!(
        kept.read(ttl, wait, probe(&runs, Duration::ZERO, Ok(Vec::new()))),
        failure
    );
    assert_eq!(
        runs.load(Ordering::SeqCst),
        1,
        "a tool that failed was asked again at once"
    );

    let panicking = self::kept();
    let said = panicking.read(ttl, wait, || -> Reading {
        panic!("a synthetic panic in a probe")
    });
    assert!(
        said.as_ref()
            .is_err_and(|why| why.contains("stopped unexpectedly")),
        "{said:?}"
    );
}

/// How long each of `runs` took, as the middle, the 90th percentile and the
/// worst, in milliseconds.
fn spread(runs: usize, mut each: impl FnMut()) -> [f64; 3] {
    let mut took: Vec<f64> = (0..runs)
        .map(|_| {
            let began = Instant::now();
            each();
            began.elapsed().as_secs_f64() * 1_000.0
        })
        .collect();
    took.sort_by(f64::total_cmp);
    [took[runs / 2], took[runs * 9 / 10], took[runs - 1]]
}

/// Start the next `list` from a cold reading, where there is a reading to forget.
fn forget_the_reading() {
    #[cfg(target_os = "macos")]
    IOS_DEVICES.forget();
}

/// This process's resident memory in KiB and its thread count, from `ps`.
fn resident_and_threads() -> (Option<u64>, Option<usize>) {
    let pid = std::process::id().to_string();
    let ps = |args: &[&str]| {
        crate::proc::quiet_command("ps")
            .args(args)
            .args(["-p", &pid])
            .output()
            .ok()
            .map(|out| String::from_utf8_lossy(&out.stdout).into_owned())
    };
    let resident = ps(&["-o", "rss="]).and_then(|text| text.trim().parse().ok());
    // `ps -M` prints one line for each thread under a header line.
    let threads = ps(&["-M"]).map(|text| text.lines().count().saturating_sub(1));
    (resident, threads)
}

/// The cost of one `list` with and without the real-device tools, on this
/// machine's own `simctl`, `adb` and `devicectl`: what it was (the simulator
/// and emulator readers alone), what it is with a cold reading and with a kept
/// one, what each tool costs alone, what a machine without `devicectl` pays, how
/// long a caller waits for a tool that takes three seconds, and the process's
/// memory and threads over a long run. Prints counts and times, never a name or
/// an id. Run on purpose, normal and under `taskpolicy -b`:
/// `cargo test -p zerocode-shell --bin zerocode-shell -- --ignored --nocapture the_cost_of_one_list`.
#[test]
#[ignore = "a measurement: it runs this machine's simctl, adb and devicectl"]
fn the_cost_of_one_list() {
    const RUNS: usize = 20;
    const LONG_RUN: usize = 300;
    const SLOW_TOOL: Duration = Duration::from_secs(3);
    let list = || {
        let _ = tauri::async_runtime::block_on(list_answer_now());
    };
    let was = spread(RUNS, || {
        tauri::async_runtime::block_on(async {
            let _ = tokio::join!(
                mobile_emulators_direct(),
                super::super::android_emulators_direct()
            );
        });
    });
    let cold = spread(RUNS, || {
        forget_the_reading();
        list();
    });
    list();
    let warm = spread(RUNS, list);
    #[cfg(target_os = "macos")]
    let devicectl = spread(RUNS, || {
        let _ = ios_devices_with(super::super::ios::devicectl_command(), PROBE_TOOL_CAP);
    });
    #[cfg(not(target_os = "macos"))]
    let devicectl = [0.0; 3];
    let missing = spread(RUNS, || {
        let _ = ios_devices_with(
            crate::proc::quiet_command("/nonexistent/devicectl"),
            PROBE_TOOL_CAP,
        );
    });
    let android = spread(RUNS, || {
        let _ = super::super::android::android_listing();
    });
    let slow = {
        let kept = kept();
        let began = Instant::now();
        let first = kept.read(PROBE_TTL, PROBE_WAIT, || {
            std::thread::sleep(SLOW_TOOL);
            Ok(Vec::new())
        });
        let waited = began.elapsed().as_secs_f64() * 1_000.0;
        std::thread::sleep(SLOW_TOOL);
        let began = Instant::now();
        let second = kept.read(PROBE_TTL, PROBE_WAIT, || Ok(Vec::new()));
        [
            waited,
            began.elapsed().as_secs_f64() * 1_000.0,
            f64::from(u8::from(first.is_err() && second.is_ok())),
        ]
    };
    let (rss_before, threads_before) = resident_and_threads();
    for round in 0..LONG_RUN {
        if round % 10 == 0 {
            forget_the_reading();
        }
        list();
    }
    let (rss_after, threads_after) = resident_and_threads();
    let answer = tauri::async_runtime::block_on(list_answer_now()).unwrap_or_default();
    let keys: Vec<&str> = answer.as_object().map_or_else(Vec::new, |object| {
        object.keys().map(String::as_str).collect()
    });
    let counts =
        ["ios", "android", "physical"].map(|key| answer[key].as_array().map_or(0, Vec::len));
    println!(
        "PHYSICAL_LIST_NUMBERS {}",
        serde_json::json!({
            "runs": RUNS,
            "unit": "ms [p50, p90, max]",
            "list_before": was,
            "list_after_cold": cold,
            "list_after_warm": warm,
            "devicectl_alone": devicectl,
            "devicectl_missing": missing,
            "android_half_alone (one adb, both readings)": android,
            "slow_tool_3s_ms [caller_waited, second_call, as_expected]": slow,
            "answer_keys": keys,
            "rows_ios_android_physical": counts,
            "long_run": {
                "lists": LONG_RUN,
                "rss_kib_before": rss_before,
                "rss_kib_after": rss_after,
                "threads_before": threads_before,
                "threads_after": threads_after,
            },
        })
    );
}
