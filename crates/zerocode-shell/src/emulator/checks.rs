//! Deterministic observations from the same complete mobile trees as marks.

use serde_json::{Value, json};
use zerocode_core::computer_use::{EmulatorMethod, EmulatorPlatform, window_title_matches};
use zerocode_core::computer_use_protocol::{ProviderError, error_code};

use super::marks::{self, Device, Snapshot};

pub(crate) async fn observe(
    platform: EmulatorPlatform,
    device: Device,
    method: EmulatorMethod,
    subject: &str,
) -> Result<Value, ProviderError> {
    let snapshot = marks::snapshot(platform, &device).await?;
    evaluate(platform, &snapshot, method, subject)
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

/// The words a snapshot shows, each once, in the tree's order — at most
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

/// Every word a snapshot's faces read, each once, in the tree's order: what a
/// list walk collects and how it tells one screen from the next.
pub(super) fn words_of(snapshot: &Snapshot) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    snapshot
        .faces
        .iter()
        .filter_map(|face| face.name.as_deref().map(str::trim))
        .filter(|name| !name.is_empty())
        .filter(|name| seen.insert((*name).to_owned()))
        .map(str::to_owned)
        .collect()
}

/// `wait-for`: read the screen until `subject` is on it — or, `absent`, until it
/// is not — or `timeout_ms` has gone by, and answer whether it was met, the last
/// count, how long it waited and in how many reads. Not met, the answer carries
/// what the screen did show. A read the door refuses (a locked phone) ends the
/// wait with that refusal: waiting does not unlock anything.
pub(crate) async fn wait_for(
    platform: EmulatorPlatform,
    device: Device,
    subject: &str,
    absent: bool,
    timeout_ms: u64,
) -> Result<Value, ProviderError> {
    use zerocode_core::agent_emulator::EMULATOR_WAIT_POLL_MS;
    let started = std::time::Instant::now();
    let limit = std::time::Duration::from_millis(timeout_ms);
    let mut reads = 0_u32;
    loop {
        let snapshot = marks::snapshot(platform, &device).await?;
        reads += 1;
        let count = found(&snapshot, subject);
        let met = (count > 0) != absent;
        let waited = started.elapsed();
        if met || waited >= limit {
            let mut answer = json!({
                MET_KEY: met,
                COUNT_KEY: count,
                "waitedMs": u64::try_from(waited.as_millis()).unwrap_or(u64::MAX),
                "reads": reads,
            });
            if !met {
                answer[VISIBLE_KEY] = json!(visible(&snapshot));
            }
            return Ok(answer);
        }
        tokio::time::sleep(std::time::Duration::from_millis(EMULATOR_WAIT_POLL_MS)).await;
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
