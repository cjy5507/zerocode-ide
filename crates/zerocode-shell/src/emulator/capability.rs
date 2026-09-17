//! Shared validation and bounded log values for mobile-emulator capabilities.
//!
//! Platform modules own their tools (`adb`/`simctl`); this module owns the wire
//! contract and the input limits both implementations must obey.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use serde::Serialize;

use super::EmulatorPlatform;

pub(super) const APP_COMMAND_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(180);
pub(super) const SHORT_COMMAND_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);
pub(super) const LOG_COMMAND_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);
pub(super) const COMMAND_OUTPUT_BYTES: usize = 256 * 1024;
pub(super) const LOG_OUTPUT_BYTES: usize = 1024 * 1024;
pub(super) const DEFAULT_LOG_LINES: usize = 200;
pub(super) const MAX_LOG_LINES: usize = 2_000;
const MAX_IDENTIFIER_BYTES: usize = 255;
const MAX_LOG_FILTERS: usize = 16;
const MAX_FILTER_BYTES: usize = 96;
const MAX_LOG_MESSAGE_BYTES: usize = 16 * 1024;
const MAX_LOG_RECORD_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EmulatorLogEntry {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub level: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub process: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    pub message: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EmulatorLogBatch {
    pub platform: EmulatorPlatform,
    pub entries: Vec<EmulatorLogEntry>,
    pub truncated: bool,
}

pub(super) fn app_path(path: &str, extension: &str, directory: bool) -> Result<PathBuf, String> {
    if path.trim().is_empty() || path.as_bytes().contains(&0) {
        return Err("앱 경로가 비어 있거나 올바르지 않습니다".to_string());
    }
    let canonical = std::fs::canonicalize(path)
        .map_err(|error| format!("앱 경로를 열 수 없습니다: {error}"))?;
    let metadata = std::fs::metadata(&canonical).map_err(|error| error.to_string())?;
    if directory != metadata.is_dir() || (!directory && !metadata.is_file()) {
        return Err(if directory {
            "iOS Simulator 앱은 .app 번들이어야 합니다".to_string()
        } else {
            "Android 앱은 APK 파일이어야 합니다".to_string()
        });
    }
    let actual = canonical
        .extension()
        .and_then(std::ffi::OsStr::to_str)
        .unwrap_or_default();
    if !actual.eq_ignore_ascii_case(extension) {
        return Err(format!(".{extension} 앱만 설치할 수 있습니다"));
    }
    if !directory && metadata.len() == 0 {
        return Err("빈 APK 파일은 설치할 수 없습니다".to_string());
    }
    Ok(canonical)
}

pub(super) fn identifier(value: &str, label: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > MAX_IDENTIFIER_BYTES
        || !value.is_ascii()
        || !value
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        || value.starts_with('.')
        || value.ends_with('.')
        || value.split('.').any(str::is_empty)
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'$'))
    {
        return Err(format!("올바르지 않은 {label}입니다"));
    }
    Ok(value.to_string())
}

pub(super) fn android_activity(value: Option<String>) -> Result<Option<String>, String> {
    value
        .map(|value| {
            let value = value.trim();
            if value.is_empty() {
                return Ok(None);
            }
            let class = value.strip_prefix('.').unwrap_or(value);
            let valid = value.len() <= MAX_IDENTIFIER_BYTES
                && value.is_ascii()
                && !class.is_empty()
                && class.split('.').all(|segment| {
                    !segment.is_empty()
                        && segment.as_bytes().first().is_some_and(|byte| {
                            byte.is_ascii_alphabetic() || matches!(byte, b'_' | b'$')
                        })
                })
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'$'));
            valid
                .then(|| value.to_string())
                .map(Some)
                .ok_or_else(|| "올바르지 않은 Android 액티비티입니다".to_string())
        })
        .transpose()
        .map(Option::flatten)
}

pub(super) fn log_line_limit(lines: Option<usize>) -> Result<usize, String> {
    let lines = lines.unwrap_or(DEFAULT_LOG_LINES);
    (1..=MAX_LOG_LINES)
        .contains(&lines)
        .then_some(lines)
        .ok_or_else(|| format!("로그 줄 수는 1..={MAX_LOG_LINES}여야 합니다"))
}

pub(super) fn android_log_filters(filters: Option<Vec<String>>) -> Result<Vec<String>, String> {
    let filters = filters.unwrap_or_default();
    if filters.len() > MAX_LOG_FILTERS {
        return Err(format!(
            "로그 필터는 {MAX_LOG_FILTERS}개까지 사용할 수 있습니다"
        ));
    }
    filters
        .into_iter()
        .map(|filter| {
            let filter = filter.trim();
            let Some((tag, level)) = filter.rsplit_once(':') else {
                return Err("Android 로그 필터는 TAG:LEVEL 형식이어야 합니다".to_string());
            };
            let valid_tag = !tag.is_empty()
                && tag.len() <= MAX_FILTER_BYTES
                && tag.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b'*' | b'.' | b'_' | b'-')
                });
            let valid_level = matches!(
                level,
                "V" | "D" | "I" | "W" | "E" | "F" | "S" | "v" | "d" | "i" | "w" | "e" | "f" | "s"
            );
            if !valid_tag || !valid_level {
                return Err("올바르지 않은 Android 로그 필터입니다".to_string());
            }
            Ok(format!("{}:{}", tag, level.to_ascii_uppercase()))
        })
        .collect()
}

pub(super) fn android_log_batch(
    output: &[u8],
    lines: usize,
    process_truncated: bool,
) -> EmulatorLogBatch {
    let text = String::from_utf8_lossy(output);
    let mut entries = VecDeque::with_capacity(lines.min(DEFAULT_LOG_LINES));
    let mut overflow = false;
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        if entries.len() == lines {
            entries.pop_front();
            overflow = true;
        }
        entries.push_back(parse_android_log_line(line));
    }
    EmulatorLogBatch {
        platform: EmulatorPlatform::Android,
        entries: entries.into(),
        truncated: process_truncated || overflow,
    }
}

pub(super) fn ios_log_batch(
    output: &[u8],
    lines: usize,
    process_truncated: bool,
) -> EmulatorLogBatch {
    let text = String::from_utf8_lossy(output);
    let mut entries = VecDeque::with_capacity(lines.min(DEFAULT_LOG_LINES));
    let mut overflow = false;
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        let Some(entry) = parse_ios_log_line(line) else {
            continue;
        };
        if entries.len() == lines {
            entries.pop_front();
            overflow = true;
        }
        entries.push_back(entry);
    }
    EmulatorLogBatch {
        platform: EmulatorPlatform::Ios,
        entries: entries.into(),
        truncated: process_truncated || overflow,
    }
}

fn parse_android_log_line(line: &str) -> EmulatorLogEntry {
    let fields = line.split_whitespace().collect::<Vec<_>>();
    if fields.len() >= 6
        && fields[0].len() == 5
        && fields[1].contains(':')
        && fields[2].bytes().all(|byte| byte.is_ascii_digit())
        && fields[3].bytes().all(|byte| byte.is_ascii_digit())
    {
        let tag = fields[5].trim_end_matches(':');
        let message_start = if fields.get(6) == Some(&":") { 7 } else { 6 }.min(fields.len());
        return EmulatorLogEntry {
            timestamp: Some(format!("{} {}", fields[0], fields[1])),
            level: Some(fields[4].to_string()),
            process: Some(fields[2].to_string()),
            tag: (!tag.is_empty()).then(|| tag.to_string()),
            message: bounded_message(&fields[message_start..].join(" ")),
        };
    }
    EmulatorLogEntry {
        timestamp: None,
        level: None,
        process: None,
        tag: None,
        message: bounded_message(line.trim()),
    }
}

fn parse_ios_log_line(line: &str) -> Option<EmulatorLogEntry> {
    if line.len() > MAX_LOG_RECORD_BYTES {
        return Some(EmulatorLogEntry {
            timestamp: None,
            level: None,
            process: None,
            tag: None,
            message: bounded_message(line.trim()),
        });
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
        return Some(EmulatorLogEntry {
            timestamp: None,
            level: None,
            process: None,
            tag: None,
            message: bounded_message(line.trim()),
        });
    };
    if value.get("finished").is_some() && value.get("count").is_some() {
        return None;
    }
    let string = |key: &str| value.get(key).and_then(serde_json::Value::as_str);
    let process = string("processImagePath")
        .and_then(|path| Path::new(path).file_name())
        .and_then(std::ffi::OsStr::to_str)
        .or_else(|| string("process"))
        .map(str::to_string);
    let tag = match (string("subsystem"), string("category")) {
        (Some(subsystem), Some(category)) => Some(format!("{subsystem}:{category}")),
        (Some(subsystem), None) => Some(subsystem.to_string()),
        _ => None,
    };
    Some(EmulatorLogEntry {
        timestamp: string("timestamp").map(str::to_string),
        level: string("messageType").map(str::to_string),
        process,
        tag,
        message: bounded_message(string("eventMessage").unwrap_or(line.trim())),
    })
}

fn bounded_message(message: &str) -> String {
    if message.len() <= MAX_LOG_MESSAGE_BYTES {
        return message.to_string();
    }
    let mut boundary = MAX_LOG_MESSAGE_BYTES;
    while !message.is_char_boundary(boundary) {
        boundary -= 1;
    }
    format!("{}…", &message[..boundary])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_and_filters_reject_argument_shaped_input() {
        assert!(identifier("com.example.app", "패키지").is_ok());
        assert!(identifier("--device", "패키지").is_err());
        assert!(android_log_filters(Some(vec!["App:D".to_string(), "*:S".to_string()])).is_ok());
        assert!(android_log_filters(Some(vec!["--help".to_string()])).is_err());
    }

    #[test]
    fn android_logs_are_parsed_and_keep_only_the_requested_tail() {
        let batch = android_log_batch(
            b"08-20 12:00:00.000  10  11 I First: one\n08-20 12:00:01.000  12  13 E Last: two\n",
            1,
            false,
        );
        assert_eq!(batch.entries.len(), 1);
        assert_eq!(batch.entries[0].tag.as_deref(), Some("Last"));
        assert_eq!(batch.entries[0].message, "two");
        assert!(batch.truncated);

        let padded = parse_android_log_line("08-20 12:00:02.000  12  13 W Padded : detail");
        assert_eq!(padded.tag.as_deref(), Some("Padded"));
        assert_eq!(padded.message, "detail");
    }

    #[test]
    fn ios_ndjson_maps_to_the_same_wire_shape() {
        let batch = ios_log_batch(
            br#"{"timestamp":"now","messageType":"Info","process":"Demo","eventMessage":"ready"}"#,
            10,
            false,
        );
        assert_eq!(batch.entries[0].process.as_deref(), Some("Demo"));
        assert_eq!(batch.entries[0].message, "ready");

        let summary = ios_log_batch(br#"{"count":1,"finished":1}"#, 10, false);
        assert!(summary.entries.is_empty());
    }
}
