//! Bounded Android `uiautomator` accessibility snapshots.

use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, Instant};

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
            children,
        }
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
    let mut child = crate::proc::quiet_command(binary)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::from(output))
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| error.to_string())?;
    let deadline = Instant::now() + COMMAND_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(25));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = std::fs::remove_file(&output_path);
                return Err("Android 접근성 명령이 시간을 초과했습니다".to_string());
            }
        }
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

pub(super) fn parse(xml: &str) -> Result<AndroidAxNode, String> {
    if xml.trim().is_empty() {
        return Err("Android 접근성 트리가 비어 있습니다".to_string());
    }
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut stack: Vec<Option<AndroidAxNode>> = Vec::new();
    let mut roots = Vec::new();
    let mut elements = 0usize;
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
    Ok(AndroidAxNode::root(roots))
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
