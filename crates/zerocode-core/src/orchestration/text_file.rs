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
use std::path::Path;

use super::{BOOL_FLAGS, MAX_PROSE, Words};

/// Each file flag and the text flag it stands for: every flag a verb reads prose from, except
/// `--result`, which is JSON, and `--title`, which is one line.
pub const TEXT_FILE_FLAGS: &[(&str, &str)] = &[
    ("--prompt-file", "--prompt"),
    ("--body-file", "--body"),
    ("--spec-file", "--spec"),
    ("--question-file", "--question"),
    ("--resolution-file", "--resolution"),
    ("--reason-file", "--reason"),
];

/// What inline text holding one of these may have been through before the window saw it: a shell
/// runs a backtick pair and a `$(` inside double quotes, and hands the verb what they printed.
pub const SHELL_RUNS: &[&str] = &["`", "$("];

/// The sentence the shim says once for an inline text holding [`SHELL_RUNS`]. It carries no
/// apostrophe, so both shims can quote it in single quotes and no shell reads it as a command.
pub fn inline_warning() -> String {
    let files: Vec<&str> = TEXT_FILE_FLAGS.iter().map(|(file, _)| *file).collect();
    format!(
        "warning: a text holds a backtick or $( - the shell may already have run it \
         before this verb saw it; give the text as a file with {} instead",
        files.join(", ")
    )
}

/// What `help` says once about the file forms, beside what every verb shares.
pub fn help_line() -> String {
    let files: Vec<&str> = TEXT_FILE_FLAGS.iter().map(|(file, _)| *file).collect();
    format!(
        "a text flag also comes as a file ({}): an absolute path to a UTF-8 file, \
         read as it lies with no shell in between — give any text holding a backtick \
         or $( that way",
        files.join(", ")
    )
}

/// Whether inline text holds something a shell runs as a command.
pub fn may_have_been_expanded(text: &str) -> bool {
    SHELL_RUNS.iter().any(|part| text.contains(part))
}

/// Replace every file flag in `argv` with the text flag it stands for, holding the file's text.
///
/// The one place a file is opened for a verb. `argv` comes back as it was, without a copy, when
/// it names no file flag — which is every verb but the ones an agent hands a briefing.
///
/// A text comes by one road: a file flag beside the inline flag it stands for, or beside itself,
/// is refused, not resolved by which came last. The words are walked as
/// [`super::split_words`] walks them, so the value of another flag, or anything past `--`, that
/// happens to spell a file flag is left alone.
pub fn expand(argv: &[String]) -> Result<Cow<'_, [String]>, String> {
    let names_a_file = |arg: &String| {
        let name = arg.split_once('=').map_or(arg.as_str(), |(name, _)| name);
        TEXT_FILE_FLAGS.iter().any(|(file, _)| *file == name)
    };
    let Some((verb, rest)) = argv.split_first().filter(|_| argv.iter().any(names_a_file)) else {
        return Ok(Cow::Borrowed(argv));
    };
    let mut expanded = vec![verb.clone()];
    // Each text flag a source was seen for, and whether that source was a file.
    let mut sources: Vec<(&str, bool)> = Vec::new();
    let mut note = |(file, text): (&'static str, &'static str), from_file: bool| {
        if sources
            .iter()
            .any(|(held, was_file)| *held == text && (*was_file || from_file))
        {
            return Err(format!(
                "{text} and {file} were both given, or one of them twice — a text comes one \
                 way, so give one of them"
            ));
        }
        sources.push((text, from_file));
        Ok(())
    };
    let mut index = 0;
    let mut past_terminator = false;
    while index < rest.len() {
        let arg = rest[index].as_str();
        if past_terminator || !arg.starts_with("--") || arg == "--" {
            past_terminator |= arg == "--";
            expanded.push(arg.to_string());
            index += 1;
            continue;
        }
        let (name, joined) = arg
            .split_once('=')
            .map_or((arg, None), |(name, value)| (name, Some(value)));
        let width = if joined.is_some() || BOOL_FLAGS.contains(&arg) {
            1
        } else {
            2
        };
        let value = joined
            .map(str::to_string)
            .or_else(|| (width == 2).then(|| rest.get(index + 1).cloned().unwrap_or_default()));
        if let Some(&pair) = TEXT_FILE_FLAGS.iter().find(|(file, _)| *file == name) {
            note(pair, true)?;
            expanded.push(pair.1.to_string());
            expanded.push(read_text_file(pair.0, &value.unwrap_or_default())?);
        } else {
            if let Some(&pair) = TEXT_FILE_FLAGS.iter().find(|(_, text)| *text == name) {
                note(pair, false)?;
            }
            expanded.extend(rest[index..(index + width).min(rest.len())].iter().cloned());
        }
        index += width;
    }
    Ok(Cow::Owned(expanded))
}

/// A file's text, whole, as it lies: a plain file by its absolute path, UTF-8, not past the prose
/// bound, with something in it.
///
/// Absolute because the window reads it from its own folder, not the caller's. A link is not
/// followed — the window opens what the caller names, and a link is a way to name something
/// else — the rule a worker's receipt file is read by. A file past the bound is refused whole,
/// because a cut briefing is one nobody wrote.
fn read_text_file(flag: &str, path: &str) -> Result<String, String> {
    if path.is_empty() {
        return Err(format!("{flag} needs a path"));
    }
    if !Path::new(path).is_absolute() {
        return Err(format!(
            "{flag} names a file by its absolute path, and {path:?} is not one — the window \
             reads it from its own folder, not yours; write \"$PWD/name\""
        ));
    }
    let unreadable = |why: &dyn std::fmt::Display| format!("{flag} {path:?} cannot be read: {why}");
    let held = std::fs::symlink_metadata(path).map_err(|why| unreadable(&why))?;
    if !held.is_file() {
        return Err(format!(
            "{flag} {path:?} is not a plain file — a link or a folder is not followed"
        ));
    }
    if held.len() > MAX_PROSE as u64 {
        return Err(format!(
            "{flag} {path:?} is {} bytes and a text holds {MAX_PROSE} — nothing was written, \
             so give a shorter file",
            held.len()
        ));
    }
    let bytes = std::fs::read(path).map_err(|why| unreadable(&why))?;
    let text = String::from_utf8(bytes)
        .map_err(|_| format!("{flag} {path:?} is not UTF-8 text — nothing was written"))?;
    if text.trim().is_empty() {
        return Err(format!("{flag} {path:?} is empty — nothing was written"));
    }
    Ok(text)
}

/// Refuse a request that reached the plan with a file flag still in it.
///
/// The window reads the files before the plan, so a file flag here means a road to the plan that
/// skipped the window. Ignored, the verb would write a task with no spec and say it worked.
pub fn refuse_unexpanded(words: &Words) -> Result<(), String> {
    match TEXT_FILE_FLAGS
        .iter()
        .find(|(file, _)| words.value(file).is_some())
    {
        Some((file, text)) => Err(format!(
            "{file} reached the ledger unread — the window reads a file before a verb is planned, \
             so this request came by a road that does not; send {text} with the text itself"
        )),
        None => Ok(()),
    }
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
        for &(file, text) in TEXT_FILE_FLAGS {
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
    fn help_names_every_file_flag_once() {
        let line = help_line();
        for &(file, _) in TEXT_FILE_FLAGS {
            assert_eq!(line.matches(file).count(), 1, "{file}: {line}");
        }
    }

    #[test]
    fn a_file_flag_that_reaches_the_plan_is_refused_not_dropped() {
        let words =
            super::super::split_words(&argv(&["--prompt-file", "/tmp/x.md", "--task", "t-1"]), &[]);
        let why = refuse_unexpanded(&words).expect_err("it would silently lose the text");
        assert!(why.contains("--prompt-file"), "{why}");
        let words = super::super::split_words(&argv(&["--prompt", "x"]), &[]);
        assert!(refuse_unexpanded(&words).is_ok());
    }
}
