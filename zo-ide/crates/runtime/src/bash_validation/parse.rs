//! Pure shell-command tokenizing helpers shared by the validation rules and
//! the intent classifier: split a line into operator-separated segments,
//! strip leading wrapper commands (`timeout`, `env`, …), extract the first
//! bare command, and unquote single-quoted spans. All functions are pure
//! (`&str` in, owned/borrowed `str` out) with no validation policy of their
//! own.

/// Commands that wrap and then exec another command. Stripping them
/// exposes the real command for classification: `timeout 5 rm -rf /`
/// must classify as `rm` (Destructive), not as the unknown `timeout`.
const WRAPPER_COMMANDS: &[&str] = &[
    "timeout", "time", "nice", "ionice", "nohup", "stdbuf", "setsid", "env", "xargs", "doas",
];

/// Split a shell command line into the individual commands joined by
/// control operators (`&&`, `||`, `;`, `|`, `&`, newline), respecting
/// single and double quotes so operators inside string literals are not
/// treated as separators.
///
/// Empty segments are dropped and each result is trimmed. A command with
/// no operators yields a single segment (the whole command), so callers
/// stay backward-compatible.
#[must_use]
pub fn split_command_segments(command: &str) -> Vec<&str> {
    let bytes = command.as_bytes();
    let mut segments = Vec::new();
    let mut start = 0usize;
    let mut i = 0usize;
    let mut quote: Option<u8> = None;

    while i < bytes.len() {
        let b = bytes[i];
        if let Some(q) = quote {
            // Inside a quoted span: only the matching unescaped quote ends it.
            if b == q && bytes[i - 1] != b'\\' {
                quote = None;
            }
            i += 1;
            continue;
        }
        match b {
            b'\'' | b'"' => {
                quote = Some(b);
                i += 1;
            }
            // `&&`/`||` (two chars) and `&`/`|` (one char) all separate.
            b'&' | b'|' => {
                segments.push(&command[start..i]);
                let double = i + 1 < bytes.len() && bytes[i + 1] == b;
                i += if double { 2 } else { 1 };
                start = i;
            }
            b';' | b'\n' => {
                segments.push(&command[start..i]);
                i += 1;
                start = i;
            }
            _ => i += 1,
        }
    }
    segments.push(&command[start..]);

    segments
        .into_iter()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect()
}

/// Strip leading command wrappers (`timeout 5`, `nice -n 10`, `env A=b`,
/// `nohup`, …) and structural subshell/group/negation punctuation from a
/// single segment, returning the inner command so it can be classified by its
/// real first token. Conservative: when unsure how many arguments a wrapper
/// consumes it stops early, leaving the remainder to be treated as `Unknown`
/// rather than silently trusted.
pub(super) fn strip_command_wrappers(segment: &str) -> &str {
    let mut rest = segment.trim();
    // Bounded loop: each iteration must consume at least the wrapper token,
    // and `WRAPPER_COMMANDS` is small, so this terminates quickly.
    loop {
        // Structural command-position punctuation hides the real command from
        // the first-token classifier: `! rm -rf /` (pipeline negation) and
        // `( rm -rf / )` / `(rm -rf /)` (subshell) both classify as an inert
        // `!`/`(` token, letting a destructive command slip past read-only and
        // catastrophic checks. Strip the wrappers so the inner command — and its
        // arguments — are exposed as clean tokens.
        //
        // Trailing subshell/group closers first: `(rm -rf /)` glues `)` onto the
        // final argument (`/)`), so strip a trailing `)`/`}` before tokenizing.
        let closed = rest.trim_end_matches([')', '}']).trim_end();
        if closed.len() != rest.len() {
            rest = closed;
            continue;
        }
        let Some(first) = rest.split_whitespace().next() else {
            return rest;
        };
        // `(` opens a subshell with or without a following space, so strip a
        // leading `(` in either form. `!`/`{` are structural only as a
        // standalone word — `!rm`/`{rm` glued are a command name / brace text in
        // a non-interactive shell, so those stay on the fail-closed Unknown path.
        if let Some(after) = rest.strip_prefix('(') {
            rest = after.trim_start();
            continue;
        }
        if matches!(first, "!" | "{") {
            rest = rest[first.len()..].trim_start();
            continue;
        }
        if !WRAPPER_COMMANDS.contains(&first) {
            return rest;
        }
        let after = rest[first.len()..].trim_start();
        let inner = skip_wrapper_args(first, after);
        // No progress (wrapper with no following command) → keep as-is so
        // the caller still sees *something* to classify.
        if inner.is_empty() || inner.len() >= rest.len() {
            return rest;
        }
        rest = inner;
    }
}

/// Skip a wrapper's option flags and its positional argument(s), returning
/// the slice that begins at the wrapped command.
fn skip_wrapper_args<'a>(wrapper: &str, after: &'a str) -> &'a str {
    let mut rest = after.trim_start();
    // Skip leading option flags (`-n`, `--signal=TERM`, `-oL`, …). A flag
    // that takes a separate value (`-n 10`, `--signal TERM`) also consumes
    // the following non-flag token.
    while let Some(tok) = rest.split_whitespace().next() {
        if tok.len() > 1 && tok.starts_with('-') {
            rest = rest[tok.len()..].trim_start();
            let takes_value = !tok.contains('=')
                && matches!(wrapper, "nice" | "ionice" | "timeout" | "stdbuf")
                && matches!(
                    tok,
                    "-n" | "-s" | "--signal" | "-k" | "-c" | "-o" | "-e" | "-i"
                );
            if takes_value {
                if let Some(val) = rest.split_whitespace().next() {
                    if !val.starts_with('-') {
                        rest = rest[val.len()..].trim_start();
                    }
                }
            }
        } else {
            break;
        }
    }
    // Wrapper-specific positional arguments.
    match wrapper {
        // `timeout DURATION cmd` / `time` (bash builtin form has none).
        "timeout" => {
            if let Some(tok) = rest.split_whitespace().next() {
                if tok.chars().next().is_some_and(|c| c.is_ascii_digit()) {
                    rest = rest[tok.len()..].trim_start();
                }
            }
        }
        // `env KEY=VAL... cmd`.
        "env" => {
            while let Some(tok) = rest.split_whitespace().next() {
                let is_assignment = tok.split_once('=').is_some_and(|(k, _)| {
                    !k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                });
                if is_assignment {
                    rest = rest[tok.len()..].trim_start();
                } else {
                    break;
                }
            }
        }
        _ => {}
    }
    rest
}

/// Extract the first bare command from a pipeline/chain, stripping env vars and sudo.
pub(super) fn extract_first_command(command: &str) -> String {
    let trimmed = command.trim();

    // Skip leading environment variable assignments (KEY=val cmd ...).
    let mut remaining = trimmed;
    loop {
        let next = remaining.trim_start();
        if let Some(eq_pos) = next.find('=') {
            let before_eq = &next[..eq_pos];
            // Valid env var name: alphanumeric + underscore, no spaces.
            if !before_eq.is_empty()
                && before_eq
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_')
            {
                // Skip past the value (might be quoted).
                let after_eq = &next[eq_pos + 1..];
                if let Some(space) = find_end_of_value(after_eq) {
                    remaining = &after_eq[space..];
                    continue;
                }
                // No space found means value goes to end of string — no actual command.
                return String::new();
            }
        }
        break;
    }

    remaining
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_string()
}

/// Extract the command following "sudo" (skip sudo flags).
pub(super) fn extract_sudo_inner(command: &str) -> &str {
    let parts: Vec<&str> = command.split_whitespace().collect();
    let sudo_idx = parts.iter().position(|&p| p == "sudo");
    match sudo_idx {
        Some(idx) => {
            // Skip flags after sudo.
            let rest = &parts[idx + 1..];
            for &part in rest {
                if !part.starts_with('-') {
                    // Found the inner command — return from here to end.
                    let offset = command.find(part).unwrap_or(0);
                    return &command[offset..];
                }
            }
            ""
        }
        None => "",
    }
}

/// Find the end of a value in `KEY=value rest` (handles basic quoting).
fn find_end_of_value(s: &str) -> Option<usize> {
    let s = s.trim_start();
    if s.is_empty() {
        return None;
    }

    let first = s.as_bytes()[0];
    if first == b'"' || first == b'\'' {
        let quote = first;
        let mut i = 1;
        while i < s.len() {
            if s.as_bytes()[i] == quote && (i == 0 || s.as_bytes()[i - 1] != b'\\') {
                // Skip past quote.
                i += 1;
                // Find next whitespace.
                while i < s.len() && !s.as_bytes()[i].is_ascii_whitespace() {
                    i += 1;
                }
                return if i < s.len() { Some(i) } else { None };
            }
            i += 1;
        }
        None
    } else {
        s.find(char::is_whitespace)
    }
}

/// Remove single-quoted spans so a literal operator inside an argument
/// (e.g. `grep -F '|' file`) does not trip the escape checks. Double
/// quotes are preserved because the shell still expands `$(…)` and
/// backticks inside them.
pub(super) fn strip_single_quoted(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut in_single = false;
    for ch in input.chars() {
        match ch {
            '\'' => in_single = !in_single,
            _ if in_single => {}
            _ => output.push(ch),
        }
    }
    output
}
