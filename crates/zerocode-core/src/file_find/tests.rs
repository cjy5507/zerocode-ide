//! `zerocode-find`'s words on their own: what the window reads off the argv
//! the shim sends, and what the command prints.

use super::*;

fn argv(words: &[&str]) -> Vec<String> {
    words.iter().map(|word| (*word).to_string()).collect()
}

/// The agent's words are the request; the folder and the pane the shim adds
/// at the end are read off the end, so the same flags inside the words stay
/// words. No word is the usage.
#[test]
fn the_request_is_the_agents_words_and_the_shim_adds_the_folder_and_pane() {
    assert_eq!(
        ask_from_argv(&argv(&[
            "fix",
            "the parser crash",
            CWD_FLAG,
            "/w/project",
            PANE_FLAG,
            "term-3"
        ])),
        Ok(FindAsk {
            request: "fix the parser crash".to_string(),
            cwd: Some("/w/project".to_string()),
            pane: Some("term-3".to_string()),
        })
    );
    assert_eq!(
        ask_from_argv(&argv(&["why", PANE_FLAG, "is", "set", CWD_FLAG, "/w"])),
        Ok(FindAsk {
            request: format!("why {PANE_FLAG} is set"),
            cwd: Some("/w".to_string()),
            pane: None,
        })
    );
    assert_eq!(ask_from_argv(&argv(&[CWD_FLAG, "/w"])), Err(USAGE));
    assert_eq!(ask_from_argv(&argv(&["  "])), Err(USAGE));
}

/// One line per file, its first comment line beside it where it has one,
/// and the note that says the list is a suggestion; a line of its own when
/// nothing was found.
#[test]
fn the_listing_names_each_file_with_its_first_line_and_says_what_it_is() {
    let files = [
        FilePickCandidate {
            path: "src/parser.rs".to_string(),
            about: "//! The parser, which turns a line into tokens.".to_string(),
        },
        FilePickCandidate {
            path: "src/lexer.rs".to_string(),
            about: String::new(),
        },
    ];
    assert_eq!(
        listing(&files),
        format!(
            "{FILE_PICK_NOTE_PREFIX} src/parser.rs  //! The parser, which turns a line into tokens.\n{FILE_PICK_NOTE_PREFIX} src/lexer.rs\n{LISTING_NOTE}\n"
        )
    );
    assert_eq!(listing(&[]), format!("{NOTHING_FOUND}\n"));
}

/// The command is the window's bridge door on its own road, carrying the
/// folder and the pane.
#[test]
fn the_shim_is_a_bridge_door_on_its_own_road() {
    let script = shim_script("TEST_PORT", "TEST_TOKEN");
    for part in [ROUTE, SHIM, CWD_FLAG, PANE_FLAG, "TEST_PORT", "TEST_TOKEN"] {
        assert!(script.contains(part), "{part}");
    }
}
