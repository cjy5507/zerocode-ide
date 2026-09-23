//! Bounded Android `uiautomator` accessibility snapshots.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;
#[cfg(any(test, not(unix)))]
use std::time::Instant;

use quick_xml::Reader;
use quick_xml::XmlVersion;
use quick_xml::events::{BytesStart, Event};
use serde::Serialize;

const DEVICE_DUMP_PATH: &str = "/sdcard/window_dump.xml";
const COMMAND_TIMEOUT: Duration = Duration::from_secs(6);
pub(super) const MAX_OUTPUT_BYTES: u64 = 4 * 1024 * 1024;
const MAX_ELEMENTS: usize = 500;
const MAX_DEPTH: usize = 80;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct AndroidAxBounds {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct AndroidAxNode {
    #[serde(skip_serializing_if = "Option::is_none")]
    class_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    resource_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    content_desc: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    package_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    clickable: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    focused: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    bounds: Option<AndroidAxBounds>,
    /// The display's rotation the dump was taken in (`<hierarchy
    /// rotation="…">`, 0–3) — only the synthetic root carries it.
    #[serde(skip_serializing_if = "Option::is_none")]
    rotation: Option<u32>,
    children: Vec<AndroidAxNode>,
}

impl AndroidAxNode {
    fn root(children: Vec<Self>) -> Self {
        Self {
            class_name: None,
            text: None,
            resource_id: None,
            content_desc: None,
            package_name: None,
            clickable: None,
            enabled: None,
            focused: None,
            bounds: None,
            rotation: None,
            children,
        }
    }

    /// The display's rotation the dump was taken in, when it said one.
    pub(super) const fn rotation(&self) -> Option<u32> {
        self.rotation
    }
}

pub(super) fn snapshot(adb: &Path, serial: &str) -> Result<AndroidAxNode, String> {
    // Concurrent tree reads must never read or remove another request's dump.
    let dump_path = format!("{DEVICE_DUMP_PATH}.{}", uuid::Uuid::new_v4());
    let dumped = run(
        adb,
        &["-s", serial, "shell", "uiautomator", "dump", &dump_path],
        None,
    );
    let xml = dumped.and_then(|_| {
        run(
            adb,
            &["-s", serial, "exec-out", "cat", &dump_path],
            Some(MAX_OUTPUT_BYTES),
        )
    });
    let _ = run(adb, &["-s", serial, "shell", "rm", "-f", &dump_path], None);
    let text = String::from_utf8(xml?).map_err(|_| "Android 접근성 트리가 UTF-8이 아닙니다")?;
    parse(&text)
}

pub(super) fn run(binary: &Path, args: &[&str], max_bytes: Option<u64>) -> Result<Vec<u8>, String> {
    let output_path = std::env::temp_dir().join(format!(
        "zerocode-android-ax-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    let output = std::fs::File::create(&output_path).map_err(|error| error.to_string())?;
    let child = crate::proc::quiet_command(binary)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::from(output))
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| error.to_string())?;
    let Some(status) = wait_within(child, COMMAND_TIMEOUT) else {
        let _ = std::fs::remove_file(&output_path);
        return Err("Android 접근성 명령이 시간을 초과했습니다".to_string());
    };
    if !status.success() {
        let _ = std::fs::remove_file(&output_path);
        return Err("Android 접근성 명령이 실패했습니다".to_string());
    }
    let size = std::fs::metadata(&output_path)
        .map_err(|error| error.to_string())?
        .len();
    if max_bytes.is_some_and(|maximum| size > maximum) {
        let _ = std::fs::remove_file(&output_path);
        return Err("Android 접근성 트리가 크기 제한을 넘었습니다".to_string());
    }
    let bytes = std::fs::read(&output_path).map_err(|error| error.to_string());
    let _ = std::fs::remove_file(output_path);
    bytes
}

/// Wait for `child` to exit, at most `timeout`: its status, or `None` once
/// the time is up and the child has been killed and reaped.
///
/// A thread waits on the child and hands its status over the moment it
/// exits (t-6385). The 25 ms poll this replaces answered no sooner than its
/// next wake: an adb call that takes 11–24 ms on an emulator waited 25, and
/// a walk step drives a dozen of them — about 200 ms a step spent asleep.
#[cfg(unix)]
fn wait_within(
    mut child: std::process::Child,
    timeout: Duration,
) -> Option<std::process::ExitStatus> {
    let pid = child.id();
    let (done, exited) = std::sync::mpsc::channel();
    let waiter = std::thread::Builder::new()
        .name("android-ax-wait".to_string())
        .spawn(move || {
            let _ = done.send(child.wait());
        });
    if waiter.is_err() {
        return None;
    }
    match exited.recv_timeout(timeout) {
        Ok(Ok(status)) => Some(status),
        Ok(Err(_)) => None,
        Err(_) => {
            // SAFETY: `pid` is this process's own child, not yet reaped —
            // the waiter still holds it — so the id cannot name anyone else.
            let pid = i32::try_from(pid).unwrap_or(i32::MAX);
            unsafe { libc::kill(pid, libc::SIGKILL) };
            // The waiter reaps it; nothing is left behind.
            let _ = exited.recv();
            None
        }
    }
}

/// [`wait_within`] where no signal can end a child by its id: the poll.
#[cfg(not(unix))]
fn wait_within(
    mut child: std::process::Child,
    timeout: Duration,
) -> Option<std::process::ExitStatus> {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status),
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(25));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

pub(super) fn parse(xml: &str) -> Result<AndroidAxNode, String> {
    if xml.trim().is_empty() {
        return Err("Android 접근성 트리가 비어 있습니다".to_string());
    }
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut stack: Vec<Option<AndroidAxNode>> = Vec::new();
    let mut roots = Vec::new();
    let mut elements = 0usize;
    let mut rotation = None;
    loop {
        match reader.read_event() {
            Ok(Event::Start(start)) => {
                if stack.len() >= MAX_DEPTH {
                    return Err(limit_exceeded("depth", MAX_DEPTH));
                }
                if start.name().as_ref() == b"node" {
                    count_element(&mut elements)?;
                    stack.push(Some(read_node(&start)?));
                } else {
                    if start.name().as_ref() == b"hierarchy" {
                        rotation = hierarchy_rotation(&start)?;
                    }
                    stack.push(None);
                }
            }
            Ok(Event::Empty(start)) if start.name().as_ref() == b"node" => {
                if stack.len() >= MAX_DEPTH {
                    return Err(limit_exceeded("depth", MAX_DEPTH));
                }
                count_element(&mut elements)?;
                attach(read_node(&start)?, &mut stack, &mut roots);
            }
            Ok(Event::End(_)) => {
                let Some(node) = stack.pop() else {
                    return Err("Android 접근성 XML의 닫는 태그가 맞지 않습니다".to_string());
                };
                if let Some(node) = node {
                    attach(node, &mut stack, &mut roots);
                }
            }
            Ok(Event::Eof) => break,
            Err(error) => return Err(format!("Android 접근성 XML을 읽지 못했습니다: {error}")),
            _ => {}
        }
    }
    if !stack.is_empty() || roots.is_empty() {
        return Err("Android 접근성 XML이 완전하지 않습니다".to_string());
    }
    Ok(AndroidAxNode {
        rotation,
        ..AndroidAxNode::root(roots)
    })
}

/// The rotation a dump's `<hierarchy>` says it was taken in (0–3), when it
/// says one; anything else is no rotation.
fn hierarchy_rotation(start: &BytesStart<'_>) -> Result<Option<u32>, String> {
    for attribute in start.attributes() {
        let attribute = attribute.map_err(|error| error.to_string())?;
        if attribute.key.as_ref() == b"rotation" {
            let value = attribute
                .normalized_value(XmlVersion::Implicit1_0)
                .map_err(|error| error.to_string())?;
            return Ok(value
                .trim()
                .parse::<u32>()
                .ok()
                .filter(|rotation| *rotation < 4));
        }
    }
    Ok(None)
}

fn limit_exceeded(kind: &str, maximum: usize) -> String {
    format!(
        "Android accessibility tree exceeds {kind} limit {maximum}; refused a truncated tree (no partial marks)"
    )
}

fn count_element(elements: &mut usize) -> Result<(), String> {
    *elements += 1;
    if *elements > MAX_ELEMENTS {
        Err(limit_exceeded("elements", MAX_ELEMENTS))
    } else {
        Ok(())
    }
}

fn attach(
    node: AndroidAxNode,
    stack: &mut [Option<AndroidAxNode>],
    roots: &mut Vec<AndroidAxNode>,
) {
    if let Some(parent) = stack.iter_mut().rev().flatten().next() {
        parent.children.push(node);
    } else {
        roots.push(node);
    }
}

fn read_node(start: &BytesStart<'_>) -> Result<AndroidAxNode, String> {
    let mut node = AndroidAxNode::root(Vec::new());
    for attribute in start.attributes() {
        let attribute = attribute.map_err(|error| error.to_string())?;
        let value = attribute
            .normalized_value(XmlVersion::Implicit1_0)
            .map_err(|error| error.to_string())?
            .into_owned();
        let present = (!value.is_empty()).then_some(value.as_str());
        match attribute.key.as_ref() {
            b"class" => node.class_name = present.map(str::to_string),
            b"text" => node.text = present.map(str::to_string),
            b"resource-id" => node.resource_id = present.map(str::to_string),
            b"content-desc" => node.content_desc = present.map(str::to_string),
            b"package" => node.package_name = present.map(str::to_string),
            b"clickable" => node.clickable = bool_value(present),
            b"enabled" => node.enabled = bool_value(present),
            b"focused" => node.focused = bool_value(present),
            b"bounds" => node.bounds = present.and_then(parse_bounds),
            _ => {}
        }
    }
    Ok(node)
}

fn bool_value(value: Option<&str>) -> Option<bool> {
    match value {
        Some("true") => Some(true),
        Some("false") => Some(false),
        _ => None,
    }
}

fn parse_bounds(value: &str) -> Option<AndroidAxBounds> {
    let values = value
        .trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .split("][")
        .flat_map(|pair| pair.split(','))
        .map(str::parse::<i32>)
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    (values.len() == 4).then(|| AndroidAxBounds {
        left: values[0],
        top: values[1],
        right: values[2],
        bottom: values[3],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn android_xml_faces_preserve_every_supported_field() {
        use zerocode_core::computer_use::EmulatorPlatform;
        let tree = parse(include_str!("../marks/fixtures/android.xml")).unwrap();
        let faces = crate::emulator::marks::faces(
            EmulatorPlatform::Android,
            &serde_json::to_value(tree).unwrap(),
        );
        let face = &faces[1];
        assert_eq!(face.index, 2);
        assert_eq!(face.role, "android.widget.Button");
        assert_eq!(face.name.as_deref(), Some("일반 설정"));
        assert_eq!(face.placeholder, None);
        assert_eq!(face.traits, Vec::<String>::new());
        assert_eq!(face.actions, vec!["AXPress"]);
        assert_eq!(face.x, 20.0);
        assert_eq!(face.y, 80.0);
        assert_eq!(face.width, 120.0);
        assert_eq!(face.height, 44.0);
        assert!(face.signature.contains("settings.general"));
        assert_eq!(face.visible, None);
        assert_eq!(face.context.as_deref(), Some("설정"));
    }

    #[test]
    fn realistic_uiautomator_xml_maps_fields_entities_bounds_and_children() {
        let tree = parse(
            r#"<?xml version='1.0'?><hierarchy><node class="android.widget.FrameLayout" enabled="true" bounds="[0,0][1080,2340]"><node text="Tom &amp; Jerry" content-desc="Search" clickable="true" /></node></hierarchy>"#,
        )
        .unwrap();
        let frame = &tree.children[0];
        assert_eq!(
            frame.class_name.as_deref(),
            Some("android.widget.FrameLayout")
        );
        assert_eq!(frame.enabled, Some(true));
        assert_eq!(frame.bounds.as_ref().unwrap().right, 1080);
        assert_eq!(frame.children[0].text.as_deref(), Some("Tom & Jerry"));
        assert_eq!(frame.children[0].clickable, Some(true));
    }

    #[test]
    fn malformed_empty_and_overdeep_trees_fail_closed() {
        assert!(parse("").is_err());
        assert!(parse("<hierarchy><node>").is_err());
        let deep = format!(
            "<hierarchy>{}{}</hierarchy>",
            "<node>".repeat(MAX_DEPTH + 1),
            "</node>".repeat(MAX_DEPTH + 1)
        );
        assert!(parse(&deep).unwrap_err().contains("truncated"));
        let wide = format!(
            "<hierarchy>{}</hierarchy>",
            "<node/>".repeat(MAX_ELEMENTS + 1)
        );
        assert!(
            parse(&wide)
                .unwrap_err()
                .contains(&format!("elements limit {MAX_ELEMENTS}"))
        );
    }

    /// A dump says the display rotation it was taken in, and nothing else is
    /// read as one (t-6385).
    #[test]
    fn a_dump_says_the_rotation_it_was_taken_in() {
        let dump = |rotation: &str| {
            parse(&format!(
                r#"<?xml version='1.0'?><hierarchy rotation="{rotation}"><node text="설정" bounds="[0,0][10,10]"/></hierarchy>"#
            ))
            .unwrap()
            .rotation()
        };
        assert_eq!(dump("1"), Some(1));
        assert_eq!(dump("0"), Some(0));
        assert_eq!(dump("7"), None);
        assert_eq!(dump("sideways"), None);
        let bare =
            parse(r#"<hierarchy><node text="설정" bounds="[0,0][10,10]"/></hierarchy>"#).unwrap();
        assert_eq!(bare.rotation(), None);
        assert!(
            !serde_json::to_value(&bare.children[0])
                .unwrap()
                .as_object()
                .unwrap()
                .contains_key("rotation"),
            "only the dump's own root says a rotation"
        );
    }

    /// A command is answered the moment it exits (t-6385): the 25 ms poll it
    /// replaced answered a 30 ms command at its second wake, never before
    /// 50 ms from its start. The clock starts once the command has started,
    /// and the quickest of five is held to that, so a busy machine that slows
    /// every start cannot turn a waiter into a poll.
    #[cfg(unix)]
    #[test]
    fn a_finished_command_is_answered_without_waiting_for_a_poll() {
        let millis: Vec<u128> = (0..5)
            .map(|_| {
                let child = crate::proc::quiet_command("/bin/sleep")
                    .arg("0.03")
                    .spawn()
                    .expect("sleep starts");
                let began = Instant::now();
                wait_within(child, COMMAND_TIMEOUT).expect("sleep answers");
                began.elapsed().as_millis()
            })
            .collect();
        let quickest = millis.iter().min().copied().unwrap_or(u128::MAX);
        assert!(quickest < 45, "a 30 ms command answered in {millis:?} ms");
    }

    /// A command past its time is ended and answered with nothing, however
    /// long it meant to run.
    #[cfg(unix)]
    #[test]
    fn a_command_past_its_time_is_ended_and_answers_nothing() {
        let child = crate::proc::quiet_command("/bin/sleep")
            .arg("5")
            .spawn()
            .expect("sleep starts");
        let began = Instant::now();
        assert!(wait_within(child, Duration::from_millis(100)).is_none());
        assert!(
            began.elapsed() < Duration::from_secs(2),
            "{:?}",
            began.elapsed()
        );
    }

    #[test]
    fn android_bounds_accept_offscreen_coordinates_and_reject_junk() {
        assert_eq!(
            parse_bounds("[-5,10][1080,2340]"),
            Some(AndroidAxBounds {
                left: -5,
                top: 10,
                right: 1080,
                bottom: 2340,
            })
        );
        assert!(parse_bounds("[0,0]").is_none());
    }
}
