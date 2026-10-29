//! A session's own folder is read through a narrow door of its own (t-83039).
//!
//! The person pressed a path an agent's answer named in that agent's scratch
//! folder, outside the project. The project's file door refuses it, rightly.
//! These hold that it still does, that the narrow door beside it asks the
//! session table and judges the path before it opens anything, and that the
//! window opens what it hands back in a tab nobody can write.
//!
//! The judgement itself — which folders a session names, a key or a keychain
//! refused before it is opened, a link out of the folder followed to where it
//! leads — is `session_scratch`'s own unit tests.

use super::support::{block_after, strip_rust_comments, window_source};

const FS: &str = include_str!("../../src/cmd/fs.rs");

/// The door's source, read through its own `include_str!`: a module that owns
/// its tests beside it stays out of the shipped-backend haystack.
const DOOR: &str = include_str!("../../src/session_scratch.rs");

#[test]
fn the_project_door_is_unchanged_and_the_scratch_door_asks_the_session_table() {
    let fs = strip_rust_comments(FS);
    let project = block_after(&fs, "pub(crate) async fn read_text_file(");
    assert!(
        project.contains("root_of(") && project.contains("read_text_in_project(&root, &path"),
        "the project's file door no longer reads through its own root:\n{project}"
    );
    let scratch = block_after(&fs, "pub(crate) fn session_scratch_document(");
    for needed in [
        "state.pane_sessions()",
        "session_folders(",
        "read_scratch(&path, &folders, cap)",
    ] {
        assert!(
            scratch.contains(needed),
            "the scratch door lost `{needed}`:\n{scratch}"
        );
    }
    assert!(
        !scratch.contains("read_text_in_project") && !scratch.contains("std::fs::"),
        "the scratch door reads past its own judgement:\n{scratch}"
    );
    let door = strip_rust_comments(
        DOOR.split_once("#[cfg(test)]")
            .map_or(DOOR, |(code, _)| code),
    );
    let read = block_after(&door, "pub(crate) fn read_scratch(");
    let judged = read
        .find("real.starts_with(folder)")
        .expect("the scratch door no longer judges the path against the session folders");
    let opened = read
        .find("bounded_text(")
        .expect("the scratch door no longer reads through the bounded text read");
    assert!(
        judged < opened,
        "the scratch door opens a file before it knows the file is in a session's folder:\n{read}"
    );
}

#[test]
fn the_window_opens_a_scratch_file_in_a_tab_nobody_can_write() {
    let opening = block_after(window_source(), "async function openOutsideDocLink(");
    assert!(
        opening.contains("invoke(\"session_scratch_document\"")
            && opening.contains("readOnly: true")
            && opening.contains("t(\"file.outsideProject\""),
        "a pressed path outside the project lost its narrow door, its read-only tab or its sentence:\n{opening}"
    );
}
