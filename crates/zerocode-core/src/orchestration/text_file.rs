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
    ("--why-file", "--why"),
];

/// What inline text holding one of these may have been through before the window saw it: a shell
/// runs a backtick pair and a `$(` inside double quotes, and hands the verb what they printed.
pub const SHELL_RUNS: &[&str] = &["`", "$("];

/// The sentence the shim says once for an inline text holding [`SHELL_RUNS`]. It carries no
/// apostrophe, so both shims can quote it in single quotes and no shell reads it as a command.
///
/// It does not say the shell ran the text: a shim sees only the words the shell had already made,
/// so a backtick that survived to here sat in single quotes or was escaped, and one that sat in
/// double quotes was run and is gone. The shim cannot tell the two apart, so the sentence names the
/// one road no shell reads — the file — instead of guessing which this was (t-80980 6).
pub fn inline_warning() -> String {
    let files: Vec<&str> = TEXT_FILE_FLAGS.iter().map(|(file, _)| *file).collect();
    format!(
        "warning: a text holds a backtick or $( - inside double quotes the shell has already run \
         it, and this shim cannot tell whether that happened; to be sure no shell reads a text, \
         give it as a file with {} instead",
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

/// A file's text, whole: a plain file by its absolute path, UTF-8, not past the prose bound, with
/// something in it. A leading byte-order mark is not part of the text and a Windows line end
/// (`\r\n`) arrives as `\n`; nothing else is changed.
///
/// Absolute because the window reads it from its own folder, not the caller's. A link at the end
/// of the path is not followed — the window opens what the caller names, and a link is a way to
/// name something else — the rule a worker's receipt file is read by. A link in a folder above
/// the file is followed: that folder is part of where the caller says the file is. A file past
/// the bound is refused whole, because a cut briefing is one nobody wrote.
///
/// The name is opened once and everything after asks the open file, not the name, so a file
/// swapped between a look and the read — for a link, a pipe, or a file that grew — is the file or
/// the refusal, never a half of each. No folder scope is checked: the caller is an agent that can
/// already read any file it names, so a list of folders would stop no read and block the
/// legitimate ones outside it (scratch, reports, another data root) (t-80980 9, decided
/// 2026-11-02 by the coordinator).
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
    let not_plain = || {
        format!(
            "{flag} {path:?} is not a plain file — a link or a folder is not followed (a link as \
             the last part of the path is refused; one in a folder above it is read)"
        )
    };
    if std::fs::symlink_metadata(path).is_ok_and(|named| !named.is_file()) {
        return Err(not_plain());
    }
    let file = open_plain(Path::new(path)).map_err(|why| {
        // The name may have become a link since the look above; the open refuses it.
        match std::fs::symlink_metadata(path) {
            Ok(named) if !named.is_file() => not_plain(),
            _ => unreadable(&why),
        }
    })?;
    let held = file.metadata().map_err(|why| unreadable(&why))?;
    if !held.is_file() {
        return Err(not_plain());
    }
    let past = |size: &dyn std::fmt::Display| {
        format!(
            "{flag} {path:?} is {size} bytes and a text holds {MAX_PROSE} — nothing was written, \
             so give a shorter file"
        )
    };
    if held.len() > MAX_PROSE as u64 {
        return Err(past(&held.len()));
    }
    let bytes = read_capped(file)
        .map_err(|why| unreadable(&why))?
        .ok_or_else(|| past(&format!("more than {MAX_PROSE}")))?;
    let utf16 = matches!(bytes.as_slice(), [0xff, 0xfe, ..] | [0xfe, 0xff, ..]);
    let text = String::from_utf8(bytes).map_err(|_| {
        let hint = if utf16 {
            " (it starts like UTF-16, which Windows editors write; save it as UTF-8)"
        } else {
            ""
        };
        format!("{flag} {path:?} is not UTF-8 text{hint} — nothing was written")
    })?;
    if text.contains('\0') {
        return Err(format!(
            "{flag} {path:?} holds a NUL byte, so it is not text — nothing was written"
        ));
    }
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    if text.trim().is_empty() {
        return Err(format!("{flag} {path:?} is empty — nothing was written"));
    }
    Ok(text.replace("\r\n", "\n"))
}

/// The file at `path`, opened for reading when it is not a link at its last part and not a pipe
/// that nobody writes (`O_NOFOLLOW`, `O_NONBLOCK` on Unix). Elsewhere the open follows a link and
/// the caller's look at the open file is the only check. Every road that reads a file a person or
/// an agent named opens it this way, then looks at what it opened (t-84800 r2 P3-6).
#[cfg(unix)]
pub fn open_plain(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::File::options()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
}

#[cfg(not(unix))]
pub fn open_plain(path: &Path) -> std::io::Result<std::fs::File> {
    std::fs::File::open(path)
}

/// At most [`MAX_PROSE`] bytes of `reader`, or `None` when it holds more — read one byte past the
/// bound, so a file that grew since it was measured is refused and never cut.
fn read_capped(reader: impl std::io::Read) -> std::io::Result<Option<Vec<u8>>> {
    use std::io::Read;
    let mut bytes = Vec::new();
    reader.take(MAX_PROSE as u64 + 1).read_to_end(&mut bytes)?;
    Ok((bytes.len() <= MAX_PROSE).then_some(bytes))
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
                ("--why-file", "--why"),
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

    #[test]
    fn a_read_stops_one_byte_past_the_bound_whatever_the_file_grew_to() {
        use std::io::Read;
        let endless = std::io::repeat(b'a').take(10 * MAX_PROSE as u64);
        assert!(read_capped(endless).expect("reads").is_none());
        let exact = read_capped(std::io::repeat(b'a').take(MAX_PROSE as u64))
            .expect("reads")
            .expect("the bound itself fits");
        assert_eq!(exact.len(), MAX_PROSE);
        let past = std::io::repeat(b'a').take(MAX_PROSE as u64 + 1);
        assert!(read_capped(past).expect("reads").is_none());
    }

    #[test]
    fn the_bound_is_in_bytes_the_last_one_in_and_the_next_one_out() {
        let (_in_dir, exact) = written(&vec![b'a'; MAX_PROSE]);
        let read = expand(&argv(&["send", "--body-file", &exact]))
            .expect("262,144 B fits")
            .into_owned();
        assert_eq!(read[2].len(), MAX_PROSE);
        let (_out_dir, past) = written(&vec![b'a'; MAX_PROSE + 1]);
        let why = expand(&argv(&["send", "--body-file", &past])).expect_err("262,145 B is past it");
        assert!(
            why.contains("--body-file") && why.contains("262145"),
            "{why}"
        );
    }

    #[test]
    fn a_nul_byte_means_it_is_not_text() {
        let (_dir, path) = written(b"before\0after");
        let why = expand(&argv(&["send", "--body-file", &path])).expect_err("NUL is not text");
        assert!(why.contains("NUL"), "{why}");
    }

    #[test]
    fn a_byte_order_mark_is_not_part_of_the_text() {
        let (_dir, path) = written("\u{feff}첫 줄\n둘째".as_bytes());
        let read = expand(&argv(&["send", "--body-file", &path]))
            .expect("reads")
            .into_owned();
        assert_eq!(read[2], "첫 줄\n둘째");
        let (_blank_dir, only_mark) = written("\u{feff}\n".as_bytes());
        let why = expand(&argv(&["send", "--body-file", &only_mark])).expect_err("only a mark");
        assert!(why.contains("empty"), "{why}");
    }

    #[test]
    fn a_windows_line_end_arrives_as_a_line_end() {
        let (_dir, path) = written(b"one\r\ntwo\r\n\r\nlone\rcarriage");
        let read = expand(&argv(&["send", "--body-file", &path]))
            .expect("reads")
            .into_owned();
        assert_eq!(read[2], "one\ntwo\n\nlone\rcarriage");
    }

    #[test]
    fn a_utf16_file_is_named_as_that_not_just_as_not_utf8() {
        let (_dir, path) = written(&[0xff, 0xfe, b'a', 0]);
        let why = expand(&argv(&["send", "--body-file", &path])).expect_err("UTF-16");
        assert!(why.contains("UTF-16"), "{why}");
    }

    #[cfg(unix)]
    #[test]
    fn a_link_is_named_as_the_end_of_the_path_and_a_folder_link_above_it_is_read() {
        let (dir, path) = written(b"text");
        let link = dir.path().join("link.md");
        std::os::unix::fs::symlink(&path, &link).expect("link");
        let why = expand(&argv(&["send", "--body-file", &link.display().to_string()]))
            .expect_err("a link is refused");
        assert!(why.contains("last part of the path"), "{why}");
        let folder = dir.path().join("folder-link");
        std::os::unix::fs::symlink(dir.path(), &folder).expect("folder link");
        let through = folder.join("text.md").display().to_string();
        let read = expand(&argv(&["send", "--body-file", &through]))
            .expect("reads")
            .into_owned();
        assert_eq!(read[2], "text");
    }

    #[test]
    fn the_why_of_a_take_comes_as_a_file_too() {
        let (_dir, path) = written("`pkill` $(x)".as_bytes());
        let read = expand(&argv(&["task-take", "--task", "t-1", "--why-file", &path]))
            .expect("reads")
            .into_owned();
        assert_eq!(
            read.as_ref(),
            argv(&["task-take", "--task", "t-1", "--why", "`pkill` $(x)"])
        );
    }

    #[test]
    fn the_warning_does_not_claim_the_shell_ran_what_it_cannot_tell() {
        let warning = inline_warning();
        assert!(warning.contains("double quotes"), "{warning}");
        assert!(warning.contains("cannot tell"), "{warning}");
        assert!(warning.contains("--body-file"), "{warning}");
        assert!(!warning.contains("may already have run"), "{warning}");
        assert!(
            !warning.contains('\''),
            "both shims quote it in single quotes: {warning}"
        );
    }
}
