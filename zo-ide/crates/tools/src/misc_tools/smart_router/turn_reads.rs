//! What a turn READ, as the seats' hindsight labels read it: the one place
//! a tool call is turned into the path it opened, so the recall seat (which
//! asks whether the note it put first was read) and the compaction seat
//! (which asks whether a dropped block's file was read again) cannot answer
//! the same question two ways.

/// Only the dedicated reader, or a shell command the existing dispatcher
/// proves equivalent to it, witnesses a read. An edit or a path substring
/// says nothing about which note was read.
pub(super) fn read_path(name: &str, input: &str) -> Option<String> {
    let input: serde_json::Value = serde_json::from_str(input).ok()?;
    let name = crate::aliases::canonical_tool_name(name);
    if name == crate::file_tools::READ_FILE_TOOL_NAME {
        return serde_json::from_value::<crate::file_tools::ReadFileInput>(input)
            .ok()
            .map(|read| read.path);
    }
    if name != crate::aliases::canonical_tool_name("Bash") {
        return None;
    }
    match crate::bash_redirect::dedicated_read_for(input.get("command")?.as_str()?) {
        Some(crate::bash_redirect::DedicatedRead::ReadFile { path, .. }) => Some(path),
        _ => None,
    }
}
