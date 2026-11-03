//! Test-only: the small POSIX `sh` hook commands the tests write, as the `cmd`
//! command a Windows hook runs (`cmd /S /C "<command>"`, `hooks.rs`).
//!
//! The tests state a hook the way a Unix person writes one — `printf '<json>';
//! exit 2`. `cmd` has no `printf`, does not split on `;`, and strips the quotes
//! of a JSON payload handed to an outside `printf.exe`, so before this module a
//! hook that should deny or add context printed nothing a hook reader could
//! parse (t-78118). Here the text a `printf` would print is written once to a
//! file named by its contents and the hook `type`s that file: what reaches the
//! hook reader is byte for byte what `sh` would print, whatever quotes,
//! brackets or length (past `cmd`'s 8191-character line) the text has.
//!
//! Only the forms the tests use are translated — `printf`, `exit`, `touch`,
//! `;`, `>`/`>>` and `if [ -e 'f' ]; then …; else …; fi`. A command with
//! anything else is left as it was before this module, with single quotes
//! turned into double ones.

/// The command a hook test runs: the script itself on Unix.
#[cfg(not(windows))]
pub(crate) fn shell_snippet(script: &str) -> String {
    script.to_string()
}

/// The command a hook test runs: the script translated to `cmd` on Windows.
#[cfg(windows)]
pub(crate) fn shell_snippet(script: &str) -> String {
    if let Some(command) = windows_special_case(script) {
        return command;
    }
    translate(script, &mut payload_file).unwrap_or_else(|| script.replace('\'', "\""))
}

/// Commands whose `cmd` form is not a translation of their words.
#[cfg(windows)]
fn windows_special_case(script: &str) -> Option<String> {
    let command = match script {
        "cat" => "more",
        "sleep 5" => "ping -n 6 127.0.0.1 >nul",
        "yes X | head -c 262144" => {
            "powershell -NoProfile -Command \"[Console]::Out.Write('X' * 262144)\""
        }
        "printf 'saw:%s' \"$ZO_HOOK_ENV_PROBE\"" => r#"<nul set /p "=saw:%ZO_HOOK_ENV_PROBE%""#,
        _ => return None,
    };
    Some(command.to_string())
}

/// The path of a file holding `text`, named by its contents so the same text
/// is the same command every time (a test compares the commands it reported).
#[cfg(windows)]
fn payload_file(text: &str) -> String {
    use std::hash::{Hash as _, Hasher as _};

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    let folder = std::env::temp_dir().join("zo-test-hook-payloads");
    std::fs::create_dir_all(&folder).expect("hook payload folder");
    let path = folder.join(format!("{:016x}-{}.txt", hasher.finish(), text.len()));
    if std::fs::read(&path).ok().as_deref() != Some(text.as_bytes()) {
        // Written beside and renamed in, so a hook of a test running at the
        // same moment never `type`s half a file. Another writer may have put
        // the same bytes in place first; then the rename may be refused while
        // that hook reads it, and the file already holds the text.
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let partial = path.with_extension(format!("{}-{n}.partial", std::process::id()));
        std::fs::write(&partial, text).expect("hook payload written");
        if std::fs::rename(&partial, &path).is_err() {
            let _ = std::fs::remove_file(&partial);
            assert_eq!(
                std::fs::read(&path).ok().as_deref(),
                Some(text.as_bytes()),
                "hook payload {} holds the text",
                path.display()
            );
        }
    }
    path.display().to_string()
}

/// `script` as one `cmd` command, or `None` when it uses a form this module
/// does not translate. `payload` turns a text into the path of a file holding it.
fn translate(script: &str, payload: &mut dyn FnMut(&str) -> String) -> Option<String> {
    let statements = split_statements(script)?;
    let mut words = Vec::with_capacity(statements.len());
    for statement in &statements {
        words.push(sh_words(statement)?);
    }
    let mut rest = words.as_slice();
    let command = block(&mut rest, payload, &[])?;
    rest.is_empty().then_some(command)
}

/// Statements up to one whose first word is in `stop` (left in `rest`),
/// joined with `&` — `cmd`'s `;`.
fn block(
    rest: &mut &[Vec<String>],
    payload: &mut dyn FnMut(&str) -> String,
    stop: &[&str],
) -> Option<String> {
    let mut commands = Vec::new();
    loop {
        let current = *rest;
        let Some((first, tail)) = current.split_first() else {
            break;
        };
        if first.first().is_some_and(|word| stop.contains(&word.as_str())) {
            break;
        }
        *rest = tail;
        if let ["if", "[", "-e", file, "]"] = strs(first).as_slice() {
            commands.push(if_exists(file, rest, payload)?);
        } else {
            commands.push(statement(first, payload)?);
        }
    }
    (!commands.is_empty()).then(|| commands.join(" & "))
}

/// `if [ -e 'file' ]; then …; else …; fi` as `if exist "file" (…) else (…)`.
fn if_exists(
    file: &str,
    rest: &mut &[Vec<String>],
    payload: &mut dyn FnMut(&str) -> String,
) -> Option<String> {
    let then = branch(rest, "then", payload, &["else", "fi"])?;
    let otherwise = if rest.first()?.first()? == "else" {
        Some(branch(rest, "else", payload, &["fi"])?)
    } else {
        None
    };
    let current = *rest;
    let (fi, tail) = current.split_first()?;
    if strs(fi) != ["fi"] {
        return None;
    }
    *rest = tail;
    Some(match otherwise {
        Some(otherwise) => format!("if exist \"{file}\" ({then}) else ({otherwise})"),
        None => format!("if exist \"{file}\" ({then})"),
    })
}

/// The statements after `keyword` (which opens the first of them) up to `stop`.
fn branch(
    rest: &mut &[Vec<String>],
    keyword: &str,
    payload: &mut dyn FnMut(&str) -> String,
    stop: &[&str],
) -> Option<String> {
    let current = *rest;
    let (first, tail) = current.split_first()?;
    if first.first()? != keyword {
        return None;
    }
    let mut opened: Vec<Vec<String>> = vec![first[1..].to_vec()];
    opened.extend_from_slice(tail);
    let mut cursor = opened.as_slice();
    let command = block(&mut cursor, payload, stop)?;
    let consumed = opened.len() - cursor.len();
    *rest = &current[consumed..];
    Some(command)
}

/// One simple statement: `exit N`, `touch 'f'`, or `printf` with an optional
/// `>`/`>>` redirect.
fn statement(words: &[String], payload: &mut dyn FnMut(&str) -> String) -> Option<String> {
    match strs(words).as_slice() {
        ["exit", status] if status.parse::<i32>().is_ok() => Some(format!("exit {status}")),
        ["touch", file] => Some(format!("type nul >> \"{file}\"")),
        ["printf", format, rest @ ..] => {
            let (args, redirect) = match rest {
                [args @ .., op @ (">" | ">>"), file] => (args, Some(format!(" {op} \"{file}\""))),
                args => (args, None),
            };
            let text = printf(format, args)?;
            Some(format!(
                "type \"{}\"{}",
                payload(&text),
                redirect.unwrap_or_default()
            ))
        }
        _ => None,
    }
}

fn strs(words: &[String]) -> Vec<&str> {
    words.iter().map(String::as_str).collect()
}

/// What `printf FORMAT ARGS…` prints, for the escapes and the `%s`/`%%`
/// directives the tests use.
fn printf(format: &str, args: &[&str]) -> Option<String> {
    let mut out = String::new();
    let mut args = args.iter();
    let mut chars = format.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => out.push(match chars.next()? {
                'n' => '\n',
                't' => '\t',
                'r' => '\r',
                other @ ('\\' | '"' | '\'') => other,
                _ => return None,
            }),
            '%' => match chars.next()? {
                's' => out.push_str(args.next().copied().unwrap_or_default()),
                '%' => out.push('%'),
                _ => return None,
            },
            c => out.push(c),
        }
    }
    args.next().is_none().then_some(out)
}

/// `script` cut at each `;` outside single quotes.
fn split_statements(script: &str) -> Option<Vec<String>> {
    let mut statements = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut chars = script.chars();
    while let Some(c) = chars.next() {
        match c {
            '\'' => quoted = !quoted,
            '\\' if !quoted => {
                current.push(c);
                current.push(chars.next()?);
                continue;
            }
            ';' if !quoted => {
                statements.push(std::mem::take(&mut current));
                continue;
            }
            _ => {}
        }
        current.push(c);
    }
    if quoted {
        return None;
    }
    statements.push(current);
    let statements: Vec<String> = statements
        .into_iter()
        .map(|statement| statement.trim().to_string())
        .collect();
    statements.iter().all(|statement| !statement.is_empty()).then_some(statements)
}

/// The words of one statement as `sh` reads them: single-quoted text kept
/// whole, `\x` outside quotes read as `x`, `>`/`>>` their own words. `None` for
/// anything `sh` would expand or a `cmd` command would read differently.
fn sh_words(statement: &str) -> Option<Vec<String>> {
    let mut words = Vec::new();
    let mut current: Option<String> = None;
    let mut chars = statement.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                let word = current.get_or_insert_with(String::new);
                loop {
                    match chars.next()? {
                        '\'' => break,
                        inner => word.push(inner),
                    }
                }
            }
            '\\' => current.get_or_insert_with(String::new).push(chars.next()?),
            '>' => {
                words.extend(current.take());
                if chars.peek() == Some(&'>') {
                    chars.next();
                    words.push(">>".to_string());
                } else {
                    words.push(">".to_string());
                }
            }
            c if c.is_whitespace() => words.extend(current.take()),
            '"' | '$' | '`' | '|' | '&' | '<' | '(' | ')' | '*' | '?' => return None,
            c => current.get_or_insert_with(String::new).push(c),
        }
    }
    words.extend(current.take());
    (!words.is_empty()).then_some(words)
}

#[cfg(test)]
mod tests {
    use super::translate;

    /// Translates with payload files named `P<n>` and returns the texts too.
    fn translated(script: &str) -> (Option<String>, Vec<String>) {
        let mut texts = Vec::new();
        let command = translate(script, &mut |text: &str| {
            texts.push(text.to_string());
            format!("P{}", texts.len())
        });
        (command, texts)
    }

    #[test]
    fn a_printf_is_the_text_it_prints_and_an_exit_keeps_its_status() {
        assert_eq!(
            translated(r#"printf '{"decision":"block","reason":"x y"}'"#),
            (
                Some(r#"type "P1""#.to_string()),
                vec![r#"{"decision":"block","reason":"x y"}"#.to_string()]
            )
        );
        assert_eq!(
            translated("printf 'blocked by hook'; exit 2"),
            (
                Some(r#"type "P1" & exit 2"#.to_string()),
                vec!["blocked by hook".to_string()]
            )
        );
        assert_eq!(
            translated(r#"printf '%s' '{"a":"b"}'"#),
            (Some(r#"type "P1""#.to_string()), vec![r#"{"a":"b"}"#.to_string()])
        );
    }

    #[test]
    fn printf_escapes_and_sh_quote_splices_read_as_sh_reads_them() {
        let (command, texts) = translated(r"printf 'it'\''s \\ a\n' >> 'C:\m f.txt'");
        assert_eq!(command.as_deref(), Some(r#"type "P1" >> "C:\m f.txt""#));
        assert_eq!(texts, ["it's \\ a\n"]);
        let (command, texts) = translated("printf 'hit\n' >> 'm'");
        assert_eq!(command.as_deref(), Some(r#"type "P1" >> "m""#));
        assert_eq!(texts, ["hit\n"]);
    }

    #[test]
    fn touch_then_printf_and_an_if_exists_branch_translate_whole() {
        assert_eq!(
            translated(r#"touch 'm'; printf '{"k":1}'"#).0.as_deref(),
            Some(r#"type nul >> "m" & type "P1""#)
        );
        assert_eq!(
            translated(r#"if [ -e 'm' ]; then exit 0; else touch 'm'; printf '{"k":1}'; fi"#)
                .0
                .as_deref(),
            Some(r#"if exist "m" (exit 0) else (type nul >> "m" & type "P1")"#)
        );
    }

    #[test]
    fn a_form_it_does_not_know_is_left_alone() {
        for script in ["sleep 0.5", "cat > 'm'", "printf '%d' 1", "printf \"$X\"", "if [ -e 'm' ]; then exit 0"] {
            assert_eq!(translated(script).0, None, "{script}");
        }
    }
}
