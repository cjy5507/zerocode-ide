//! Every key the window hands a backend command is an argument that command
//! reads, spelled the way tauri reads it (t-84850).
//!
//! Tauri 2 takes a command's arguments by their names in camelCase: a Rust
//! `from_pane` is read from the key `fromPane`, and a key it does not know is
//! dropped without a word. So a window that wrote `from_pane` sent nothing,
//! and the backend's `Option` read `None` — on 10-30 the 「대화」 hand-over
//! had never told a pane's CLI to exit, never written its receipt, and every
//! test passed, because the window's tests answer the invoke themselves and
//! the backend's never see the window. This gate reads both sides from the
//! sources — every command signature in the backend crates, every static
//! `invoke("…", { … })` in `ui/*.js` — and checks them key against key, the
//! way `the_emulator_commands_and_the_windows_calls_agree` does for the
//! emulator's commands alone. A call whose arguments are a variable, or an
//! object carrying a spread or a computed key, is counted and skipped rather
//! than half-checked; the floors keep the gate from quietly going blind.

use std::collections::HashMap;
use std::path::Path;

use super::quiet_children::shell_sources;
use super::support::strip_rust_comments;

/// One argument a command reads from the window: its Rust name, and whether
/// the call must send it (anything but an `Option`).
struct Argument {
    name: String,
    required: bool,
}

/// The key tauri reads a Rust argument from: its words joined in camelCase.
fn tauri_key(argument: &str) -> String {
    let mut words = argument.trim_start_matches('_').split('_');
    let mut key = words.next().unwrap_or_default().to_string();
    for word in words {
        let mut glyphs = word.chars();
        if let Some(first) = glyphs.next() {
            key.extend(first.to_uppercase());
            key.push_str(glyphs.as_str());
        }
    }
    key
}

/// The backend sources the window's commands live in: the shell crate and the
/// command crates beside it, each without its test module and its comments.
/// Strings stay: a quote's own character literal (`'"'`) is a string's start
/// to a reader that also empties strings, and the rest of its file would read
/// as one.
fn backend_sources() -> Vec<(String, String)> {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut sources = shell_sources();
    for name in ["zerocode-shell-cmd-jira", "zerocode-shell-cmd-linear"] {
        let path = crates.join(name).join("src/lib.rs");
        let text = std::fs::read_to_string(&path).unwrap_or_else(|why| panic!("{name}: {why}"));
        sources.push((format!("{name}/src/lib.rs"), text));
    }
    sources
        .into_iter()
        .filter(|(name, _)| !name.contains("tests"))
        .map(|(name, text)| {
            let shipped = text
                .split_once("#[cfg(test)]\nmod tests")
                .map_or(text.as_str(), |(code, _)| code);
            (name, strip_rust_comments(shipped))
        })
        .collect()
}

/// Every command the backend declares, by name, with the arguments the window
/// sends it. The values the runtime hands a command itself (its state, the
/// app, the webview and window, the raw request) are not the window's to send.
fn backend_commands() -> HashMap<String, Vec<Vec<Argument>>> {
    // Assembled rather than written: `nothing_reachable_ships_after_the_tests_begin`
    // counts the spelled attribute, and a literal here would read as a command
    // living inside the tests.
    let door = format!("#[tauri::{}", "command");
    let injected = [
        "State<",
        "AppHandle",
        "Webview",
        "tauri::Window",
        "ipc::Request",
    ];
    let mut commands: HashMap<String, Vec<Vec<Argument>>> = HashMap::new();
    for (file, code) in backend_sources() {
        let mut rest = code.as_str();
        while let Some(at) = rest.find(&door) {
            rest = &rest[at + door.len()..];
            let fn_at = rest
                .find("fn ")
                .unwrap_or_else(|| panic!("{file}: a command without a fn"));
            let signature = &rest[fn_at + 3..];
            let name_end = signature
                .find(['(', '<'])
                .unwrap_or_else(|| panic!("{file}: a fn without parameters"));
            let name = signature[..name_end].trim().to_string();
            // The parameter list to its own closing paren: generics and
            // tuples carry commas, so the split tracks their depth.
            let mut depth = 0i32;
            let mut opened = false;
            let mut piece = String::new();
            let mut arguments = Vec::new();
            for glyph in signature[name_end..].chars() {
                match glyph {
                    '(' if !opened && depth == 0 => {
                        opened = true;
                        continue;
                    }
                    '(' | '<' | '[' => depth += 1,
                    ')' if opened && depth == 0 => {
                        arguments.extend(argument_of(&piece, &injected));
                        break;
                    }
                    ')' | '>' | ']' => depth -= 1,
                    ',' if opened && depth == 0 => {
                        arguments.extend(argument_of(&piece, &injected));
                        piece.clear();
                        continue;
                    }
                    _ => {}
                }
                if opened {
                    piece.push(glyph);
                }
            }
            commands.entry(name).or_default().push(arguments);
        }
    }
    commands
}

/// One `name: Type` of a command's parameter list, unless the runtime fills it.
fn argument_of(piece: &str, injected: &[&str]) -> Option<Argument> {
    let (name, of_type) = piece.split_once(':')?;
    let of_type = of_type.trim();
    if injected.iter().any(|kind| of_type.contains(kind)) {
        return None;
    }
    let name = name.trim().trim_start_matches("mut ").trim().to_string();
    Some(Argument {
        name,
        required: !of_type.starts_with("Option<"),
    })
}

/// The window's scripts: every `ui/*.js` the page loads, by file name.
fn window_scripts() -> Vec<(String, String)> {
    let ui = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ui");
    let mut found: Vec<(String, String)> = std::fs::read_dir(&ui)
        .expect("the window's folder")
        .map(|entry| entry.expect("a window file").path())
        .filter(|path| path.extension().and_then(|it| it.to_str()) == Some("js"))
        .map(|path| {
            let name = path
                .file_name()
                .expect("a file name")
                .to_string_lossy()
                .into_owned();
            (
                name,
                std::fs::read_to_string(&path).expect("a window script"),
            )
        })
        .collect();
    found.sort();
    found
}

/// What one static call hands its command: the keys of the object it writes,
/// or `None` when the arguments are not an object this walk can read (a
/// variable, a spread, a computed key).
fn sent_keys(after_name: &str) -> Option<Vec<String>> {
    let after = after_name.trim_start();
    let Some(arguments) = after.strip_prefix(',') else {
        // `invoke("name")` or `invoke("name", )`: nothing sent.
        return Some(Vec::new());
    };
    let object = arguments.trim_start().strip_prefix('{')?;
    let mut depth = 1i32;
    let mut quote: Option<char> = None;
    let mut escaped = false;
    let mut top = String::new();
    let mut glyphs = object.chars().peekable();
    while let Some(glyph) = glyphs.next() {
        if let Some(open) = quote {
            if escaped {
                escaped = false;
            } else if glyph == '\\' {
                escaped = true;
            } else if glyph == open {
                quote = None;
            }
            continue;
        }
        match glyph {
            '"' | '\'' | '`' => quote = Some(glyph),
            '/' if glyphs.peek() == Some(&'/') => {
                for skipped in glyphs.by_ref() {
                    if skipped == '\n' {
                        break;
                    }
                }
                continue;
            }
            '/' if glyphs.peek() == Some(&'*') => {
                let mut last = ' ';
                for skipped in glyphs.by_ref() {
                    if last == '*' && skipped == '/' {
                        break;
                    }
                    last = skipped;
                }
                continue;
            }
            '{' | '(' | '[' => depth += 1,
            '}' | ')' | ']' => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            ',' if depth == 1 => {
                top.push('\n');
                continue;
            }
            _ => {}
        }
        if depth == 1 {
            top.push(glyph);
        }
    }
    let mut keys = Vec::new();
    for entry in top.lines().map(str::trim).filter(|entry| !entry.is_empty()) {
        if entry.starts_with("...") || entry.starts_with('[') {
            return None;
        }
        let key = entry
            .split(':')
            .next()
            .unwrap_or(entry)
            .trim()
            .trim_matches(['"', '\'']);
        if !key.is_empty()
            && key
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
        {
            keys.push(key.to_string());
        }
    }
    Some(keys)
}

#[test]
fn every_invoke_key_is_an_argument_its_command_reads_in_camel_case() {
    let commands = backend_commands();
    assert!(
        commands.len() >= 600,
        "the backend command parse collapsed ({} found) — the gate is guarding a stub",
        commands.len()
    );
    let mut wrong = Vec::new();
    let mut checked = 0usize;
    let mut opaque = 0usize;
    for (file, script) in window_scripts() {
        let mut rest = script.as_str();
        let mut line = 1usize;
        while let Some(at) = rest.find("invoke(") {
            let before = rest[..at].chars().next_back();
            line += rest[..at].matches('\n').count();
            rest = &rest[at + "invoke(".len()..];
            if before.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$') {
                continue;
            }
            let call = rest.trim_start();
            let Some(open) = call.chars().next().filter(|c| *c == '"' || *c == '\'') else {
                continue;
            };
            let Some(end) = call[1..].find(open) else {
                continue;
            };
            let name = &call[1..=end];
            let Some(declared) = commands.get(name) else {
                wrong.push(format!(
                    "{file}:{line} calls `{name}`, which no backend command declares"
                ));
                continue;
            };
            checked += 1;
            let Some(keys) = sent_keys(&call[end + 2..]) else {
                opaque += 1;
                continue;
            };
            // A name declared more than once (one body per platform) is met
            // when any of its declarations reads the call as written.
            let mismatch = declared.iter().map(|arguments| {
                let mut said = Vec::new();
                for key in &keys {
                    if key.contains('_') {
                        let read = arguments
                            .iter()
                            .find(|argument| argument.name == *key)
                            .map_or_else(|| tauri_key(key), |argument| tauri_key(&argument.name));
                        said.push(format!(
                            "sends `{key}` in snake case — tauri reads `{read}` and drops `{key}`"
                        ));
                    } else if !arguments.iter().any(|argument| tauri_key(&argument.name) == *key) {
                        said.push(format!(
                            "sends `{key}`, which `{name}` does not read — the key lands on the floor"
                        ));
                    }
                }
                for argument in arguments.iter().filter(|argument| argument.required) {
                    let read = tauri_key(&argument.name);
                    if !keys.contains(&read) {
                        said.push(format!(
                            "omits `{read}`, which `{name}` requires — that invoke rejects at runtime"
                        ));
                    }
                }
                said
            });
            let fewest = mismatch.min_by_key(Vec::len).unwrap_or_default();
            wrong.extend(
                fewest
                    .into_iter()
                    .map(|said| format!("{file}:{line} `{name}` {said}")),
            );
        }
    }
    assert!(
        wrong.is_empty(),
        "the window's invokes and the backend's arguments disagree:\n{}",
        wrong.join("\n")
    );
    assert!(
        checked >= 700,
        "only {checked} static invoke sites were found — the scan has gone blind, not the window quiet"
    );
    assert!(
        opaque <= checked / 5,
        "{opaque} of {checked} invoke sites hide their keys behind variables or spreads — teach this \
         walk to follow them before trusting it further"
    );
}

#[test]
fn tauri_reads_a_rust_argument_from_its_camel_case_key() {
    assert_eq!(tauri_key("from_pane"), "fromPane");
    assert_eq!(tauri_key("deadline_ms"), "deadlineMs");
    assert_eq!(tauri_key("term"), "term");
    assert_eq!(
        tauri_key("expected_provider_identity"),
        "expectedProviderIdentity"
    );
    assert_eq!(
        sent_keys(r#", { agent, cwd, resume, from_pane: fromTerm, deadlineMs: deadline });"#),
        Some(vec![
            "agent".to_string(),
            "cwd".to_string(),
            "resume".to_string(),
            "from_pane".to_string(),
            "deadlineMs".to_string(),
        ])
    );
    assert_eq!(
        sent_keys(r#", { text: "a, b { c }", /* x: 1 */ at: [1, 2] }, { hand });"#),
        Some(vec!["text".to_string(), "at".to_string()])
    );
    assert_eq!(sent_keys(r#", { ...rest, term });"#), None);
    assert_eq!(sent_keys(r#", args);"#), None);
    assert_eq!(sent_keys(r#");"#), Some(Vec::new()));
}
