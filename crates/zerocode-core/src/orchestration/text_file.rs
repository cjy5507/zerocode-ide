//! Prose that arrives as a file (t-80615).
//!
//! An agent that wrote a briefing inside shell double quotes had the shell
//! run every backtick and `$(…)` in it before `zerocode-orc` saw a word — twice
//! (10-28 03:23 and 21:2x) a `pkill` in a briefing ran and took the window's
//! panes down. A rule for the writer failed twice, so the verb takes the text
//! another way: a file, read here as bytes, which no shell ever expands.
//!
//! The window calls [`expand`] on the whole argv before anything reads it, so
//! the verb's own code, the receipt's fingerprint and the bounds all see
//! `--prompt <text>` exactly as if it had been typed. The plan never opens a
//! file ([`refuse_unexpanded`] stops a request that reached it with one).

use std::borrow::Cow;

use super::Words;

/// Each file flag and the text flag it stands for.
pub const TEXT_FILE_FLAGS: &[(&str, &str)] = &[];

/// What inline text holding one of these may have been through before the
/// window saw it.
pub const SHELL_RUNS: &[&str] = &["`", "$("];

/// The line printed once for an inline text holding [`SHELL_RUNS`].
pub fn inline_warning() -> String {
    String::new()
}

/// Whether inline text holds something a shell runs as a command.
pub fn may_have_been_expanded(_text: &str) -> bool {
    false
}

/// Replace every file flag in `argv` with the text flag it stands for.
pub fn expand(argv: &[String]) -> Result<Cow<'_, [String]>, String> {
    Ok(Cow::Borrowed(argv))
}

/// Refuse a request that reached the plan with a file flag still in it.
pub fn refuse_unexpanded(_words: &Words) -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(words: &[&str]) -> Vec<String> {
        words.iter().map(|word| (*word).to_string()).collect()
    }

    /// A file on disk named `name`, holding `text`, in a folder that lives as long as the
    /// returned guard.
    fn written(text: &[u8]) -> (tempfile::TempDir, String) {
        let dir = tempfile::tempdir().expect("scratch folder");
        let path = dir.path().join("text.md");
        std::fs::write(&path, text).expect("write the file");
        (dir, path.display().to_string())
    }

    /// What the two accidents had in common: a command substitution inside a briefing.
    const DANGEROUS: &str = "첫 줄 `pkill -f zerocode` 그리고 $(rm -rf ~/x) 와 $HOME \"따옴표\" 'a'\n\
                             \\n 끝줄";

    #[test]
    fn a_file_flag_stands_for_every_text_flag_the_verbs_read() {
        let flags = TEXT_FILE_FLAGS.to_vec();
        assert_eq!(
            flags,
            vec![
                ("--prompt-file", "--prompt"),
                ("--body-file", "--body"),
                ("--spec-file", "--spec"),
                ("--question-file", "--question"),
                ("--resolution-file", "--resolution"),
                ("--reason-file", "--reason"),
            ]
        );
    }

    #[test]
    fn a_file_arrives_letter_for_letter_whatever_a_shell_would_have_made_of_it() {
        let (_dir, path) = written(DANGEROUS.as_bytes());
        for (file, text) in TEXT_FILE_FLAGS {
            let given = argv(&["worker-start", "--task", "t-1", file, &path, "--worktree"]);
            let read = expand(&given).expect("a plain file reads");
            assert_eq!(
                read.as_ref(),
                argv(&[
                    "worker-start",
                    "--task",
                    "t-1",
                    text,
                    DANGEROUS,
                    "--worktree"
                ]),
                "{file}"
            );
        }
    }

    #[test]
    fn the_equals_form_reads_the_same_file() {
        let (_dir, path) = written(b"one\ntwo\n");
        let given = argv(&["send", "--type", "note", &format!("--body-file={path}")]);
        assert_eq!(
            expand(&given).expect("reads").as_ref(),
            argv(&["send", "--type", "note", "--body", "one\ntwo\n"])
        );
    }

    #[test]
    fn a_request_without_a_file_flag_is_handed_back_untouched_and_unallocated() {
        let given = argv(&["worker-start", "--prompt", "do it", "--task", "t-1"]);
        assert!(matches!(expand(&given), Ok(Cow::Borrowed(_))));
    }

    #[test]
    fn a_file_flag_named_as_another_flags_value_is_that_flags_value() {
        let given = argv(&["task-create", "--title", "--spec-file", "--spec", "x"]);
        assert_eq!(expand(&given).expect("untouched").as_ref(), given);
        let given = argv(&["task-create", "--spec", "x", "--", "--spec-file", "/nope"]);
        assert_eq!(expand(&given).expect("untouched").as_ref(), given);
    }

    #[test]
    fn a_text_given_twice_is_refused_by_name() {
        let (_dir, path) = written(b"text");
        for given in [
            argv(&["send", "--body", "inline", "--body-file", &path]),
            argv(&["send", "--body-file", &path, "--body", "inline"]),
            argv(&["send", "--body-file", &path, "--body-file", &path]),
            argv(&["send", "--body=inline", "--body-file", &path]),
        ] {
            let why = expand(&given).expect_err("two roads for one text");
            assert!(
                why.contains("--body") && why.contains("--body-file"),
                "{why}"
            );
        }
    }

    #[test]
    fn a_missing_empty_blank_or_unreadable_file_is_refused() {
        let (_dir, empty) = written(b"");
        let (_blank_dir, blank) = written(b" \n\t\n");
        let (_bad_dir, not_utf8) = written(&[0xff, 0xfe, 0x41]);
        let folder = tempfile::tempdir().expect("folder");
        let missing = folder.path().join("nope.md").display().to_string();
        let folder = folder.path().display().to_string();
        for (path, wanted) in [
            (empty.as_str(), "empty"),
            (blank.as_str(), "empty"),
            (missing.as_str(), "--spec-file"),
            (folder.as_str(), "plain file"),
            (not_utf8.as_str(), "UTF-8"),
            ("relative/path.md", "absolute"),
            ("", "needs a path"),
        ] {
            let why = expand(&argv(&["task-create", "--spec-file", path]))
                .expect_err("that file cannot be the text");
            assert!(why.contains(wanted), "{path:?}: {why}");
        }
        let why = expand(&argv(&["task-create", "--spec-file"])).expect_err("no path at all");
        assert!(why.contains("needs a path"), "{why}");
    }

    #[cfg(unix)]
    #[test]
    fn a_link_is_not_followed() {
        let (dir, path) = written(b"text");
        let link = dir.path().join("link.md");
        std::os::unix::fs::symlink(&path, &link).expect("link");
        let why = expand(&argv(&["send", "--body-file", &link.display().to_string()]))
            .expect_err("a link is refused");
        assert!(why.contains("plain file"), "{why}");
    }

    #[test]
    fn a_file_past_the_prose_bound_is_refused_whole_not_cut() {
        let (_dir, path) = written("가".repeat(super::super::MAX_PROSE).as_bytes());
        let why = expand(&argv(&["send", "--body-file", &path])).expect_err("too big");
        assert!(why.contains("--body-file"), "{why}");
    }

    #[test]
    fn inline_text_with_something_a_shell_runs_is_marked() {
        for risky in ["a `b` c", "$(x)", "ok $(y) ok", DANGEROUS] {
            assert!(may_have_been_expanded(risky), "{risky}");
        }
        for plain in ["", "plain text", "$HOME and ${X}", "(parens) $ alone"] {
            assert!(!may_have_been_expanded(plain), "{plain}");
        }
        let warning = inline_warning();
        assert!(warning.contains("--prompt-file"), "{warning}");
        assert!(warning.contains("shell"), "{warning}");
    }

    #[test]
    fn a_file_flag_that_reaches_the_plan_is_refused_not_dropped() {
        let words = super::super::split_words(
            &argv(&["--prompt-file", "/tmp/x.md", "--task", "t-1"]),
            &[],
        );
        let why = refuse_unexpanded(&words).expect_err("it would silently lose the text");
        assert!(why.contains("--prompt-file"), "{why}");
        let words = super::super::split_words(&argv(&["--prompt", "x"]), &[]);
        assert!(refuse_unexpanded(&words).is_ok());
    }
}
