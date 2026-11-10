//! Deterministic observations from the same complete mobile trees as marks.

use serde_json::{Value, json};
use zerocode_core::computer_use::{EmulatorMethod, EmulatorPlatform, window_title_matches};
use zerocode_core::computer_use_protocol::marks::ElementFace;
use zerocode_core::computer_use_protocol::{ProviderError, error_code};

use super::marks::{self, Device, Seen, Snapshot};

/// A check as a lone read, for the benches and tests that need no screen
/// handed on. Only the phone road's tests and bench read it, and they are a
/// Mac's (`wda`: unix tests, a macOS bench), so a Windows test build leaves it out.
#[cfg(all(test, unix))]
pub(crate) async fn observe(
    platform: EmulatorPlatform,
    device: Device,
    method: EmulatorMethod,
    subject: &str,
) -> Result<Value, ProviderError> {
    observe_seen(platform, device, method, subject)
        .await
        .map(|(answer, _)| answer)
}

/// A check (`find`, `foreground`) on a screen read now, with that screen for
/// the step after it.
pub(crate) async fn observe_seen(
    platform: EmulatorPlatform,
    device: Device,
    method: EmulatorMethod,
    subject: &str,
) -> Result<(Value, Seen), ProviderError> {
    let seen = Seen::read(platform, device).await?;
    Ok((evaluate(platform, &seen.snapshot, method, subject)?, seen))
}

fn evaluate(
    platform: EmulatorPlatform,
    snapshot: &Snapshot,
    method: EmulatorMethod,
    subject: &str,
) -> Result<Value, ProviderError> {
    let package = package(platform, &snapshot.raw);
    let count = match method {
        EmulatorMethod::Find => found(snapshot, subject),
        EmulatorMethod::Foreground => usize::from(package.ok_or_else(|| ProviderError::new(
            error_code::UNSUPPORTED_CAPABILITY,
            "the accessibility snapshot has no unambiguous foreground app package; iOS exports no bundle metadata",
        ))? == subject),
        _ => return Err(ProviderError::invalid_argument("expected a mobile check")),
    };
    // Both check verbs use the existing find count and Flow app/package keys.
    let mut answer = json!({ COUNT_KEY: count });
    if let Some(package) = package {
        answer["app"] = json!({"package": package});
    }
    // A find that counted nothing says what the screen does show, so the next
    // step is a choice and not another guess (Phone Harness's `tap_text`).
    if method == EmulatorMethod::Find && count == 0 {
        answer[VISIBLE_KEY] = json!(visible(snapshot));
    }
    Ok(answer)
}

/// The key a check's count is answered under — `find`'s, and a look's that
/// was asked to count (`marks --text`).
pub(crate) const COUNT_KEY: &str = "count";

/// The key the screen's own words are answered under when what was asked for
/// is not among them.
pub(crate) const VISIBLE_KEY: &str = "visible";

/// The words the device's screen shows now, read once ([`visible`]): what a
/// batch answers its last screen with.
pub(crate) async fn screen_words(
    platform: EmulatorPlatform,
    device: &Device,
) -> Result<Vec<String>, ProviderError> {
    Ok(visible(&marks::snapshot(platform, device).await?))
}

/// The words a snapshot shows, each once, in reading order — at most
/// [`EMULATOR_VISIBLE_MAX`] of them and none longer than
/// [`EMULATOR_VISIBLE_CHARS_MAX`] characters.
///
/// [`EMULATOR_VISIBLE_MAX`]: zerocode_core::agent_emulator::EMULATOR_VISIBLE_MAX
/// [`EMULATOR_VISIBLE_CHARS_MAX`]: zerocode_core::agent_emulator::EMULATOR_VISIBLE_CHARS_MAX
pub(super) fn visible(snapshot: &Snapshot) -> Vec<String> {
    use zerocode_core::agent_emulator::{EMULATOR_VISIBLE_CHARS_MAX, EMULATOR_VISIBLE_MAX};
    let mut words = words_of(snapshot);
    words.truncate(EMULATOR_VISIBLE_MAX);
    words
        .into_iter()
        .map(|word| word.chars().take(EMULATOR_VISIBLE_CHARS_MAX).collect())
        .collect()
}

/// Every word a snapshot's faces read, each once, in reading order: what a
/// list walk collects and how it tells one screen from the next.
pub(super) fn words_of(snapshot: &Snapshot) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    reading_order(&snapshot.faces)
        .into_iter()
        .filter_map(|face| face.name.as_deref().map(str::trim))
        .filter(|name| !name.is_empty())
        .filter(|name| seen.insert((*name).to_owned()))
        .map(str::to_owned)
        .collect()
}

/// Faces in the order a person reads the screen: rows from the top, and
/// along a row from the left. The tree's own order is not that — a web
/// page's text is hit-tested after the browser's bars, and a bold label can
/// follow the value beside it ("Hours" after ": Mon–Fri 7:30 AM", Phone
/// Harness 2026-11-10), which sent the model to a screenshot to be sure. Two
/// faces share a row when their middles are closer than half the shorter
/// one's height, so a container that spans many rows never swallows them.
/// Ties keep the tree's order.
fn reading_order(faces: &[ElementFace]) -> Vec<&ElementFace> {
    let middle = |face: &ElementFace| face.y + face.height / 2.0;
    let same_row = |a: &ElementFace, b: &ElementFace| {
        (middle(a) - middle(b)).abs() <= a.height.min(b.height) / 2.0
    };
    let mut by_top: Vec<&ElementFace> = faces.iter().collect();
    by_top.sort_by(|a, b| a.y.total_cmp(&b.y));
    let mut rows: Vec<Vec<&ElementFace>> = Vec::new();
    for face in by_top {
        match rows.last_mut() {
            Some(row) if same_row(row[0], face) => row.push(face),
            _ => rows.push(vec![face]),
        }
    }
    for row in &mut rows {
        row.sort_by(|a, b| a.x.total_cmp(&b.x));
    }
    rows.into_iter().flatten().collect()
}

/// `wait-for`: read the screen until `subject` is on it — or, `absent`, until it
/// is not — or the time has gone by, and answer whether it was met, the last
/// count, how long it waited and in how many reads. Not met, the answer carries
/// what the screen did show. A read the door refuses (a locked phone) ends the
/// wait with that refusal: waiting does not unlock anything.
///
/// `timeout_ms` left out is the default time, and then a wait for words that
/// are not there also ends once the screen has stood still
/// [`EMULATOR_WAIT_STILL_MS`] ([`Stillness`]), answering how long under
/// [`EMULATOR_STOOD_STILL_KEY`]. A given time, and a wait for words to go,
/// are waited whole.
///
/// [`EMULATOR_WAIT_STILL_MS`]: zerocode_core::agent_emulator::EMULATOR_WAIT_STILL_MS
/// [`EMULATOR_STOOD_STILL_KEY`]: zerocode_core::agent_emulator::EMULATOR_STOOD_STILL_KEY
pub(crate) async fn wait_for(
    platform: EmulatorPlatform,
    device: Device,
    subject: &str,
    absent: bool,
    timeout_ms: Option<u64>,
) -> Result<(Value, Seen), ProviderError> {
    use zerocode_core::agent_emulator::{
        EMULATOR_STOOD_STILL_KEY, EMULATOR_WAIT_MS_DEFAULT, EMULATOR_WAIT_POLL_MS,
        EMULATOR_WAIT_STILL_MS,
    };
    let started = std::time::Instant::now();
    let limit = std::time::Duration::from_millis(timeout_ms.unwrap_or(EMULATOR_WAIT_MS_DEFAULT));
    let mut stillness = (timeout_ms.is_none() && !absent)
        .then(|| Stillness::new(std::time::Duration::from_millis(EMULATOR_WAIT_STILL_MS)));
    let mut reads = 0_u32;
    loop {
        let read = std::time::Instant::now();
        let snapshot = marks::snapshot(platform, &device).await?;
        reads += 1;
        let count = found(&snapshot, subject);
        let met = (count > 0) != absent;
        let waited = started.elapsed();
        let stood_still = stillness
            .as_mut()
            .and_then(|still| still.stood(&snapshot, read));
        if met || waited >= limit || stood_still.is_some() {
            let mut answer = json!({
                MET_KEY: met,
                COUNT_KEY: count,
                "waitedMs": millis(waited),
                "reads": reads,
            });
            if !met {
                answer[VISIBLE_KEY] = json!(visible(&snapshot));
                if let Some(still) = stood_still {
                    answer[EMULATOR_STOOD_STILL_KEY] = json!(millis(still));
                }
            }
            return Ok((answer, Seen::of(snapshot, device, read)));
        }
        tokio::time::sleep(std::time::Duration::from_millis(EMULATOR_WAIT_POLL_MS)).await;
    }
}

fn millis(span: std::time::Duration) -> u64 {
    u64::try_from(span.as_millis()).unwrap_or(u64::MAX)
}

/// Whether the screen a wait reads has stood still long enough to stop
/// waiting for words that are not on it: every read since the first of a
/// run read the same — each element's role, words and frame to the point,
/// as a settling press compares two reads ([`marks::shape`]) — and the reads
/// span `quiet`, measured from when the first of them was asked for to when
/// the last was. Any change starts the run again, so a page loading in, a
/// list filling or anything that moves keeps the wait going to its time.
struct Stillness {
    quiet: std::time::Duration,
    run: Option<(Vec<marks::FaceShape>, std::time::Instant)>,
}

impl Stillness {
    fn new(quiet: std::time::Duration) -> Self {
        Self { quiet, run: None }
    }

    /// How long the screen has stood still, once that is `quiet` or more.
    fn stood(
        &mut self,
        snapshot: &Snapshot,
        read: std::time::Instant,
    ) -> Option<std::time::Duration> {
        let shape = marks::shape(&snapshot.faces);
        let since = match &self.run {
            Some((before, since)) if *before == shape => *since,
            _ => read,
        };
        self.run = Some((shape, since));
        let still = read.saturating_duration_since(since);
        (still >= self.quiet).then_some(still)
    }
}

/// The key a wait answers whether it was met under — a batch stops at a wait
/// that was not.
pub(crate) const MET_KEY: &str = "met";

/// How many of a snapshot's faces read `subject` — `find`'s count, and a
/// look's own when it is asked to count (`marks --text`, t-6385): the same
/// case-insensitive fragment policy as desktop window checks.
pub(super) fn found(snapshot: &Snapshot, subject: &str) -> usize {
    snapshot
        .faces
        .iter()
        .filter(|face| {
            face.name
                .as_deref()
                .is_some_and(|name| window_title_matches(name, subject))
        })
        .count()
}

/// Android's synthetic root wraps the foreground window's AX roots. Require
/// every root to name the same package, and reject contradictory descendants.
/// AXUniqueId on iOS identifies an element, never an exported bundle id.
fn package(platform: EmulatorPlatform, tree: &Value) -> Option<&str> {
    if platform != EmulatorPlatform::Android {
        return None;
    }
    let roots = tree.get("children")?.as_array()?;
    fn named(node: &Value) -> Option<&str> {
        node.get("packageName").and_then(Value::as_str)
    }
    let package = roots.first()?.get("packageName")?.as_str()?;
    if package.trim().is_empty() || roots.iter().any(|root| named(root) != Some(package)) {
        return None;
    }
    let mut pending: Vec<_> = roots.iter().collect();
    while let Some(node) = pending.pop() {
        if node.get("packageName").is_some() && named(node) != Some(package) {
            return None;
        }
        if let Some(children) = node.get("children").and_then(Value::as_array) {
            pending.extend(children);
        }
    }
    Some(package)
}

#[cfg(test)]
mod tests {
    use super::*;
    use zerocode_core::computer_use_protocol::render::Rect;

    fn snapshot(platform: EmulatorPlatform, tree: &Value) -> Snapshot {
        Snapshot::new(platform, tree, Rect::new(0.0, 0.0, 390.0, 844.0)).unwrap()
    }

    fn android_tree() -> Value {
        json!({"children": [{"className": "android.widget.TextView", "text": "송금 완료 Done", "enabled": false, "clickable": false,
            "packageName": "com.example.wallet", "bounds": {"left": 0, "top": 0, "right": 100, "bottom": 30}}]})
    }

    #[test]
    fn emulator_checks_find_noninteractive_text_on_both_platforms() {
        let ios: Value = serde_json::from_str(include_str!("marks/fixtures/ios.json")).unwrap();
        for (platform, tree, text) in [
            (EmulatorPlatform::Ios, ios, "일반"),
            (EmulatorPlatform::Android, android_tree(), "완료"),
        ] {
            let snapshot = snapshot(platform, &tree);
            assert_eq!(
                evaluate(platform, &snapshot, EmulatorMethod::Find, text).unwrap()["count"],
                1
            );
            assert_eq!(
                evaluate(platform, &snapshot, EmulatorMethod::Find, "없는 글자").unwrap()["count"],
                0
            );
        }
        let snapshot = snapshot(EmulatorPlatform::Android, &android_tree());
        assert_eq!(
            evaluate(
                EmulatorPlatform::Android,
                &snapshot,
                EmulatorMethod::Find,
                " dOnE "
            )
            .unwrap()["count"],
            1
        );
    }

    /// The screen's words come in reading order: rows from the top, along a
    /// row from the left — a bold label before the value beside it though
    /// the tree lists the value first, and a container that spans the rows
    /// swallows none of them.
    #[test]
    fn the_screens_words_read_top_down_and_left_to_right() {
        let text = |words: &str, left: i64, top: i64, bottom: i64| {
            json!({"className": "android.widget.TextView", "text": words, "enabled": true,
                "clickable": false, "packageName": "com.example.shop",
                "bounds": {"left": left, "top": top, "right": left + 60, "bottom": bottom}})
        };
        let tree = json!({"children": [
            text("Toolbar", 0, 780, 820),
            text(": Mon–Fri 7:30 AM – 6:00 PM", 70, 301, 321),
            text("Page", 0, 0, 840),
            text("Hours", 10, 300, 322),
            text("Blue Lantern Cafe", 10, 100, 130),
            text("12 Harbor Street", 10, 200, 220),
        ]});
        let snapshot = snapshot(EmulatorPlatform::Android, &tree);
        assert_eq!(
            words_of(&snapshot),
            [
                "Page",
                "Blue Lantern Cafe",
                "12 Harbor Street",
                "Hours",
                ": Mon–Fri 7:30 AM – 6:00 PM",
                "Toolbar"
            ]
        );
    }

    /// A wait gives up on a still screen only once every read across the
    /// quiet span was the same; any change — a word, an element that moved
    /// a point — starts the span again.
    #[test]
    fn a_screen_stands_still_only_when_every_read_across_the_span_agrees() {
        use std::time::{Duration, Instant};
        let row = |words: &str, top: i64| {
            json!({"children": [{"className": "android.widget.TextView", "text": words,
                "enabled": true, "clickable": false, "packageName": "com.example.shop",
                "bounds": {"left": 0, "top": top, "right": 200, "bottom": top + 40}}]})
        };
        let at = |ms: u64, tree: &Value| (snapshot(EmulatorPlatform::Android, tree), ms);
        let t0 = Instant::now();
        let quiet = Duration::from_millis(2_000);
        let mut still = Stillness::new(quiet);
        let loading = row("Loading", 100);
        let shown = row("Blue Lantern Cafe", 100);
        let moved = row("Blue Lantern Cafe", 101);
        for (read, ms, expected) in [
            (at(0, &loading), 0, None),
            (at(850, &loading), 850, None),
            (at(1_700, &shown), 1_700, None),
            (at(2_550, &shown), 2_550, None),
            (at(3_400, &moved), 3_400, None),
            (at(4_250, &moved), 4_250, None),
            (at(5_400, &moved), 5_400, Some(2_000)),
        ] {
            let (snapshot, _) = read;
            let stood = still.stood(&snapshot, t0 + Duration::from_millis(ms));
            assert_eq!(
                stood.map(|span| span.as_millis()),
                expected,
                "read at {ms} ms"
            );
        }
    }

    #[test]
    fn emulator_checks_foreground_names_the_observed_package_even_on_mismatch() {
        let snapshot = snapshot(EmulatorPlatform::Android, &android_tree());
        for (expected, count) in [
            ("com.example.wallet", 1),
            ("com.example", 0),
            ("com.example.Wallet", 0),
        ] {
            let result = evaluate(
                EmulatorPlatform::Android,
                &snapshot,
                EmulatorMethod::Foreground,
                expected,
            )
            .unwrap();
            assert_eq!(
                result,
                json!({"count": count, "app": {"package": "com.example.wallet"}})
            );
            let fingerprint = zerocode_core::computer_flow::Fingerprint::observe(&[
                json!({"ok": true, "result": result}),
            ]);
            assert!(fingerprint.apps.contains("com.example.wallet"));
        }
    }

    #[test]
    fn emulator_checks_missing_or_conflicting_packages_are_not_absence() {
        for tree in [
            json!({"children": []}),
            json!({"children": [{"children": [{"packageName": "com.example.wallet"}]}]}),
            json!({"children": [{"packageName": "com.example.wallet"}, {}]}),
            json!({"children": [{"packageName": "com.example.wallet"}, {"packageName": "com.other"}]}),
            json!({"children": [{"packageName": "com.example.wallet", "children": [{"packageName": "com.other"}]}]}),
        ] {
            let snapshot = snapshot(EmulatorPlatform::Android, &tree);
            let error = evaluate(
                EmulatorPlatform::Android,
                &snapshot,
                EmulatorMethod::Foreground,
                "com.example.wallet",
            )
            .unwrap_err();
            assert_eq!(error.code, error_code::UNSUPPORTED_CAPABILITY);
        }
    }

    #[test]
    fn emulator_checks_ios_does_not_invent_a_bundle_from_ax_unique_id() {
        let tree: Value = serde_json::from_str(include_str!("marks/fixtures/ios.json")).unwrap();
        let snapshot = snapshot(EmulatorPlatform::Ios, &tree);
        assert!(
            evaluate(
                EmulatorPlatform::Ios,
                &snapshot,
                EmulatorMethod::Find,
                "일반"
            )
            .unwrap()
            .get("app")
            .is_none()
        );
        assert_eq!(
            evaluate(
                EmulatorPlatform::Ios,
                &snapshot,
                EmulatorMethod::Foreground,
                "com.example.settings"
            )
            .unwrap_err()
            .code,
            error_code::UNSUPPORTED_CAPABILITY
        );
    }

    #[test]
    fn emulator_checks_cannot_observe_absence_from_a_truncated_tree() {
        let mut tree = android_tree();
        tree["children"][0]["truncated"] = json!(true);
        assert!(
            Snapshot::new(
                EmulatorPlatform::Android,
                &tree,
                Rect::new(0.0, 0.0, 390.0, 844.0)
            )
            .is_err()
        );
    }
}
