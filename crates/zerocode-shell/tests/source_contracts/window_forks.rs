//! The window process starts no child by a fork (t-73907, t-74976).
//!
//! Measured 2026-10-25 04:01–04:06: a launch the window forked stood forever
//! in a libSystem fork handler (`xpc_atfork_child`, waiting inside CEF)
//! before its exec, with a copy of every descriptor the window had open, and
//! three new windows stood behind the settings lock it kept. std's `spawn`
//! takes `posix_spawn`, which runs no fork handler, unless the command asks
//! for something `posix_spawn` cannot do: a `pre_exec` closure, a uid, gid or
//! group change, a chroot, or a bare program name under a changed `PATH`
//! (`env_clear` counts as one). Then it forks
//! (`library/std/src/sys/process/unix/unix.rs`, `posix_spawn`).
//!
//! So the code the window ships — this crate and every workspace crate it
//! links, test code left out — holds none of those, and every whole-env
//! change goes through the door in `proc.rs` that resolves the name first.
//! The few that stay say why below.
//!
//! The crates and files are read off the manifests and the `mod` tree, not
//! listed by hand: a hand-made list misses the next crate or file, which is
//! the failure this contract is for.

use std::path::{Path, PathBuf};

use crate::support::strip_rust_comments;

/// Whole-env changes outside the door, with how many each file has and why
/// its program cannot be a bare name under a changed `PATH`.
const ENV_CHANGES_ALLOWED: &[(&str, &str, usize, &str)] = &[(
    "zerocode-shell/src/accounts.rs",
    ".envs(",
    2,
    "both blank OVERRIDING_AUTH_VARS, which name no PATH, on a command \
     `quiet_command_on_path` built",
)];

/// Lock sites that hold their file lock through
/// `zerocode_core::held_file_lock::HeldFileLock`, which ends the lock on drop:
/// a close ends a flock only when every copy of its descriptor is gone, and a
/// launch that forked keeps one (t-73907). `(file, fn)`.
const GUARDED_LOCKS: &[(&str, &str)] = &[
    ("zerocode-shell/src/google_login.rs", "fn hold_lock("),
    (
        "zerocode-shell/src/orchestration/cost_book/receipts.rs",
        "fn take_lock(",
    ),
    (
        "zerocode-core/src/artifact_publish.rs",
        "fn lock_store_with_clock(",
    ),
    ("zerocode-core/src/jev/custom/store.rs", "fn lock("),
    (
        "zerocode-core/src/jev/project_rules/store.rs",
        "fn lock_file(",
    ),
    ("zerocode-core/src/jev/count.rs", "fn lock_hours("),
    (
        "zerocode-core/src/settings_file_lock.rs",
        "fn reclaim_from_a_dead_owner(",
    ),
];

#[test]
fn nothing_the_window_ships_asks_std_to_fork() {
    let mut offenders = Vec::new();
    for file in window_files().iter().filter(|file| file.shipped) {
        for (spelling, why) in [
            ("pre_exec(", "a pre_exec closure"),
            ("::fork(", "a fork"),
            ("::vfork(", "a vfork"),
            ("chroot(", "a chroot"),
        ] {
            let count = file.code.matches(spelling).count();
            if count > 0 {
                offenders.push(format!("{}: {count} × {why} (`{spelling}`)", file.name));
            }
        }
        for change in [".uid(", ".gid(", ".groups("] {
            // `MetadataExt::uid()` reads a file's owner; only a call with an
            // argument changes a child's.
            let count = file
                .code
                .match_indices(change)
                .filter(|(at, _)| !file.code[at + change.len()..].starts_with(')'))
                .count();
            if count > 0 {
                offenders.push(format!("{}: {count} × `{change}`", file.name));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "the window ships a child start that std runs by fork, and a fork of the window can \
stand forever before its exec holding every descriptor the window has (t-73907):\n  {}",
        offenders.join("\n  ")
    );
}

#[test]
fn a_whole_env_change_in_the_window_goes_through_the_door_that_resolves_the_name() {
    let mut offenders = Vec::new();
    for file in window_files().iter().filter(|file| file.shipped) {
        if file.name == "zerocode-shell/src/proc.rs" {
            continue;
        }
        let changes = [
            (".env_clear(", file.code.matches(".env_clear(").count()),
            (".envs(", file.code.matches(".envs(").count()),
            (".env(\"PATH\"", file.text.matches(".env(\"PATH\"").count()),
            (
                ".env_remove(\"PATH\"",
                file.text.matches(".env_remove(\"PATH\"").count(),
            ),
        ];
        for (spelling, count) in changes {
            let allowed = ENV_CHANGES_ALLOWED
                .iter()
                .find(|(name, allowed, _, _)| *name == file.name && *allowed == spelling)
                .map_or(0, |(_, _, count, _)| *count);
            if count != allowed {
                offenders.push(format!(
                    "{}: {count} × `{spelling}`, {allowed} allowed",
                    file.name
                ));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "these sources change a child's whole env or its PATH outside `proc.rs`, and a bare \
program name there makes std fork the window (t-47141, t-74976) — use \
`proc::quiet_command_in_env` or `proc::quiet_command_on_path`, or say why in \
ENV_CHANGES_ALLOWED:\n  {}",
        offenders.join("\n  ")
    );
}

/// What an allowance leans on is checked, not just said.
#[test]
fn the_allowed_env_changes_still_name_no_path() {
    assert!(
        !zerocode_core::OVERRIDING_AUTH_VARS.contains(&"PATH"),
        "accounts.rs blanks OVERRIDING_AUTH_VARS on a bare name; with PATH among them std forks"
    );
    let files = window_files();
    let accounts = files
        .iter()
        .find(|file| file.name == "zerocode-shell/src/accounts.rs")
        .expect("accounts.rs ships");
    assert_eq!(
        accounts.code.matches("quiet_command_on_path(").count(),
        accounts.code.matches(".envs(").count(),
        "each `.envs(` in accounts.rs rides a command the PATH door built"
    );
}

#[test]
fn every_lock_a_launch_could_inherit_ends_with_its_guard() {
    let files = window_files();
    let mut offenders = Vec::new();
    for (name, function) in GUARDED_LOCKS {
        let file = files
            .iter()
            .find(|file| file.name == *name && file.shipped)
            .unwrap_or_else(|| panic!("{name} is not among the window's shipped sources"));
        let Some(at) = file.code.find(function) else {
            panic!("{name} has no `{function}`");
        };
        let body = braced_after(&file.code, at);
        if !body.contains("HeldFileLock") {
            offenders.push(format!("{name} {function}…)"));
        }
    }
    assert!(
        offenders.is_empty(),
        "these locks end with a close, which a forked launch's copy of the descriptor \
outlives (t-73907):\n  {}",
        offenders.join("\n  ")
    );
}

/// The walk below must find what it says it finds, or every rule above
/// passes over nothing.
#[test]
fn the_walk_reaches_every_source_of_every_linked_crate() {
    let crates = window_crates();
    for expected in [
        "zerocode-shell",
        "zerocode-core",
        "zerocode-pty",
        "zerocode-shell-state",
    ] {
        assert!(
            crates.iter().any(|name| name == expected),
            "{expected} is not among {crates:?}"
        );
    }
    let files = window_files();
    let reached: std::collections::BTreeSet<&str> =
        files.iter().map(|file| file.name.as_str()).collect();
    let mut unreached = Vec::new();
    for name in &crates {
        let src = crates_dir().join(name).join("src");
        for path in rust_files_under(&src) {
            let relative = relative_name(&path);
            let separate_binary = path.starts_with(src.join("bin"));
            if !separate_binary && !reached.contains(relative.as_str()) {
                unreached.push(relative);
            }
        }
    }
    assert!(
        unreached.is_empty(),
        "the module walk missed these sources:\n  {}",
        unreached.join("\n  ")
    );
    let shipped = files.iter().filter(|file| file.shipped).count();
    assert!(
        shipped > 500 && shipped < files.len(),
        "{shipped} shipped of {} files: the test split is not working",
        files.len()
    );
}

/// One source the window's build reads.
struct WindowFile {
    /// `crate/src/…`, `/`-separated.
    name: String,
    /// Whether any of it ships: false for a file only a test module reaches.
    shipped: bool,
    /// The shipped part with comments gone and literals kept.
    text: String,
    /// The same with every literal's inside blanked, so a quoted spelling is
    /// not code.
    code: String,
}

fn crates_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates/")
        .to_path_buf()
}

/// A source's name under `crates/`, or under the repository for a file a
/// `#[path]` reaches outside it (tools/test-support's shared test helpers).
fn relative_name(path: &Path) -> String {
    let crates = crates_dir();
    let repository = crates.parent().expect("the repository");
    path.strip_prefix(&crates)
        .or_else(|_| path.strip_prefix(repository))
        .expect("a source in the repository")
        .to_string_lossy()
        .replace('\\', "/")
}

/// This crate and every workspace crate its `[dependencies]` name, on any
/// target, followed through their own manifests. Dev and build dependencies
/// are not in the window.
fn window_crates() -> Vec<String> {
    let mut found = Vec::new();
    let mut pending = vec!["zerocode-shell".to_string()];
    while let Some(name) = pending.pop() {
        if found.contains(&name) {
            continue;
        }
        let manifest = std::fs::read_to_string(crates_dir().join(&name).join("Cargo.toml"))
            .unwrap_or_else(|error| panic!("{name}/Cargo.toml: {error}"));
        let mut linked = false;
        for line in manifest.lines().map(str::trim) {
            if line.starts_with('[') {
                linked = line == "[dependencies]"
                    || (line.starts_with("[target.") && line.ends_with(".dependencies]"));
                continue;
            }
            if !linked {
                continue;
            }
            if let Some((dependency, _)) = line.split_once('=') {
                let dependency = dependency.trim();
                if dependency.starts_with("zerocode-") {
                    pending.push(dependency.to_string());
                }
            }
        }
        found.push(name);
    }
    found.sort();
    found
}

/// `path` with its `..` steps taken, as rustc reads a `#[path]`.
fn normalized(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

fn rust_files_under(directory: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut pending = vec![directory.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(&directory).expect("a source directory") {
            let path = entry.expect("a source entry").path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().and_then(|it| it.to_str()) == Some("rs") {
                found.push(path);
            }
        }
    }
    found
}

/// Every source of every window crate, reached from its root through `mod`
/// and `include!` the way rustc reaches it. A module declared under a test
/// `cfg` and all it declares in turn is test-only.
fn window_files() -> Vec<WindowFile> {
    let mut files: Vec<WindowFile> = Vec::new();
    for name in window_crates() {
        let src = crates_dir().join(&name).join("src");
        let root = if name == "zerocode-shell" {
            "main.rs"
        } else {
            "lib.rs"
        };
        let mut pending = vec![(src.join(root), src.clone(), false)];
        while let Some((path, module_dir, test_only)) = pending.pop() {
            let path = normalized(&path);
            let relative = relative_name(&path);
            // A file two modules name ships if either of them does.
            if let Some(seen) = files.iter().position(|file| file.name == relative) {
                if files[seen].shipped || test_only {
                    continue;
                }
                files.remove(seen);
            }
            let source = std::fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("{relative}: {error}"));
            let text = strip_rust_comments(&source);
            let code = blank_literals(&text);
            let tests = if test_only {
                vec![(0, code.len())]
            } else {
                test_ranges(&code)
            };
            let in_test = |at: usize| tests.iter().any(|(from, to)| (*from..*to).contains(&at));
            let here = path.parent().expect("a source's folder").to_path_buf();
            let inline = inline_modules(&code);
            for (at, module) in module_declarations(&code) {
                let attributes = &text[item_start(&code, at)..at];
                let (child, child_dir) = if let Some(given) = path_attribute(attributes) {
                    let child = here.join(given);
                    let dir = child.parent().expect("a module's folder").to_path_buf();
                    (child, dir)
                } else {
                    let mut dir = module_dir.clone();
                    for (_, _, outer) in
                        inline.iter().filter(|(from, to, _)| *from < at && at < *to)
                    {
                        dir.push(outer);
                    }
                    let flat = dir.join(format!("{module}.rs"));
                    let child = if flat.exists() {
                        flat
                    } else {
                        dir.join(&module).join("mod.rs")
                    };
                    (child, dir.join(&module))
                };
                assert!(
                    child.exists(),
                    "{relative}: `mod {module};` names no file at {}",
                    child.display()
                );
                pending.push((child, child_dir, test_only || in_test(at)));
            }
            for (at, included) in includes(&code, &text) {
                let child = here.join(included);
                let dir = child
                    .parent()
                    .expect("an included file's folder")
                    .to_path_buf();
                pending.push((child, dir, test_only || in_test(at)));
            }
            files.push(WindowFile {
                name: relative,
                shipped: !test_only,
                text: blank_ranges(&text, &tests),
                code: blank_ranges(&code, &tests),
            });
        }
    }
    files
}

/// `text` with the inside of every string, raw string and char literal
/// turned to spaces, byte for byte, so offsets into one are offsets into the
/// other and a brace or a `;` inside a literal is not code.
fn blank_literals(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = bytes.to_vec();
    let mut at = 0;
    while at < bytes.len() {
        let previous_is_word =
            at > 0 && (bytes[at - 1].is_ascii_alphanumeric() || bytes[at - 1] == b'_');
        if bytes[at] == b'r' && (!previous_is_word || bytes[at - 1] == b'b') {
            let hashes = bytes[at + 1..].iter().take_while(|it| **it == b'#').count();
            if bytes.get(at + 1 + hashes) == Some(&b'"') {
                let body = at + 2 + hashes;
                let fence: Vec<u8> = std::iter::once(b'"')
                    .chain(std::iter::repeat_n(b'#', hashes))
                    .collect();
                let end = bytes[body..]
                    .windows(fence.len())
                    .position(|window| window == fence.as_slice())
                    .map_or(bytes.len(), |to| body + to);
                out[body..end].fill(b' ');
                at = end + fence.len();
                continue;
            }
        }
        match bytes[at] {
            b'"' => {
                let mut end = at + 1;
                while end < bytes.len() && bytes[end] != b'"' {
                    end += if bytes[end] == b'\\' { 2 } else { 1 };
                }
                let end = end.min(bytes.len());
                out[at + 1..end].fill(b' ');
                at = end + 1;
            }
            b'\'' => {
                // `'x'` or `'\x'`; anything else is a lifetime or a label.
                let rest = &text[at + 1..];
                let mut glyphs = rest.char_indices();
                let close = match glyphs.next() {
                    Some((_, '\\')) => glyphs.nth(1),
                    Some(_) => glyphs.next(),
                    None => None,
                }
                .filter(|(_, glyph)| *glyph == '\'')
                .map(|(to, _)| at + 1 + to);
                match close {
                    Some(close) => {
                        out[at + 1..close].fill(b' ');
                        at = close + 1;
                    }
                    None => at += 1,
                }
            }
            _ => at += 1,
        }
    }
    String::from_utf8(out).expect("only whole literals were blanked")
}

/// The byte ranges of `code` under a test-only `cfg`: `#[cfg(test)]` and
/// `#[cfg(all(…, test, …))]` with the item after them. `any(…, test)` ships.
fn test_ranges(code: &str) -> Vec<(usize, usize)> {
    if code.contains("#![cfg(test)]") {
        return vec![(0, code.len())];
    }
    let bytes = code.as_bytes();
    let mut ranges = Vec::new();
    for (start, _) in code.match_indices("#[cfg(") {
        let open = start + "#[cfg".len();
        let close = closing(bytes, open, b'(', b')');
        if !test_only_predicate(&code[open + 1..close]) {
            continue;
        }
        let mut at = close + 2;
        loop {
            while at < bytes.len() && bytes[at].is_ascii_whitespace() {
                at += 1;
            }
            if code[at..].starts_with("#[") {
                at = closing(bytes, at + 1, b'[', b']') + 1;
            } else {
                break;
            }
        }
        ranges.push((start, item_end(bytes, at)));
    }
    ranges
}

fn test_only_predicate(predicate: &str) -> bool {
    let predicate = predicate.trim();
    if predicate == "test" {
        return true;
    }
    let Some(inner) = predicate
        .strip_prefix("all(")
        .and_then(|rest| rest.strip_suffix(')'))
    else {
        return false;
    };
    let mut depth = 0;
    let mut argument = String::new();
    let mut arguments = Vec::new();
    for glyph in inner.chars() {
        match glyph {
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth == 0 => {
                arguments.push(std::mem::take(&mut argument));
                continue;
            }
            _ => {}
        }
        argument.push(glyph);
    }
    arguments.push(argument);
    arguments.iter().any(|argument| argument.trim() == "test")
}

/// Where the item starting at `at` ends: its first `;` outside brackets, or
/// its braced body and a `;` right after it. A field or an arm ends at its
/// `,`, or at the `}` that closes what holds it.
fn item_end(bytes: &[u8], mut at: usize) -> usize {
    let mut depth = 0i32;
    while at < bytes.len() {
        match bytes[at] {
            b'(' | b'[' => depth += 1,
            b')' | b']' => depth -= 1,
            b';' | b',' if depth == 0 => return at + 1,
            b'}' if depth == 0 => return at,
            b'{' if depth == 0 => {
                let mut end = closing(bytes, at, b'{', b'}') + 1;
                let mut after = end;
                while after < bytes.len() && bytes[after].is_ascii_whitespace() {
                    after += 1;
                }
                if bytes.get(after) == Some(&b';') {
                    end = after + 1;
                }
                return end;
            }
            _ => {}
        }
        at += 1;
    }
    bytes.len()
}

/// The index of the bracket that closes the one at `open`.
fn closing(bytes: &[u8], open: usize, opens: u8, closes: u8) -> usize {
    let mut depth = 0;
    for (at, byte) in bytes.iter().enumerate().skip(open) {
        if *byte == opens {
            depth += 1;
        } else if *byte == closes {
            depth -= 1;
            if depth == 0 {
                return at;
            }
        }
    }
    bytes.len() - 1
}

/// The braced body that follows `at` in `code`.
fn braced_after(code: &str, at: usize) -> &str {
    let open = at + code[at..].find('{').expect("a body");
    &code[open..=closing(code.as_bytes(), open, b'{', b'}')]
}

/// Whether the word ending just before `at` stands alone.
fn word_starts_at(code: &str, at: usize) -> bool {
    code[..at]
        .chars()
        .next_back()
        .is_none_or(|glyph| !(glyph.is_alphanumeric() || glyph == '_'))
}

/// `mod name;` declarations: where each starts, and the name.
fn module_declarations(code: &str) -> Vec<(usize, String)> {
    let mut found = Vec::new();
    for (at, _) in code.match_indices("mod ") {
        if !word_starts_at(code, at) {
            continue;
        }
        let rest = code[at + 4..].trim_start();
        let name: String = rest
            .chars()
            .take_while(|glyph| glyph.is_alphanumeric() || *glyph == '_')
            .collect();
        if !name.is_empty() && rest[name.len()..].trim_start().starts_with(';') {
            found.push((at, name));
        }
    }
    found
}

/// `mod name { … }` blocks: their braces, and the name.
fn inline_modules(code: &str) -> Vec<(usize, usize, String)> {
    let mut found = Vec::new();
    for (at, _) in code.match_indices("mod ") {
        if !word_starts_at(code, at) {
            continue;
        }
        let rest = code[at + 4..].trim_start();
        let name: String = rest
            .chars()
            .take_while(|glyph| glyph.is_alphanumeric() || *glyph == '_')
            .collect();
        if name.is_empty() || !rest[name.len()..].trim_start().starts_with('{') {
            continue;
        }
        let open = at + code[at..].find('{').expect("a body");
        found.push((open, closing(code.as_bytes(), open, b'{', b'}'), name));
    }
    found
}

/// Where the item holding `at` began: just past the `;`, `{` or `}` before.
fn item_start(code: &str, at: usize) -> usize {
    code[..at].rfind([';', '{', '}']).map_or(0, |to| to + 1)
}

/// The path a `#[path = "…"]` among `attributes` names.
fn path_attribute(attributes: &str) -> Option<&str> {
    let at = attributes.find("#[path")?;
    let rest = &attributes[at..];
    let open = rest.find('"')? + 1;
    let close = open + rest[open..].find('"')?;
    Some(&rest[open..close])
}

/// `include!("…")` with a literal path: where each is, and the path.
fn includes<'a>(code: &str, text: &'a str) -> Vec<(usize, &'a str)> {
    code.match_indices("include!(")
        .filter_map(|(at, spelling)| {
            let rest = &text[at + spelling.len()..];
            let literal = rest.trim_start().strip_prefix('"')?;
            Some((at, &literal[..literal.find('"')?]))
        })
        .collect()
}

/// `text` with every range turned to spaces, newlines kept.
fn blank_ranges(text: &str, ranges: &[(usize, usize)]) -> String {
    let mut out = text.as_bytes().to_vec();
    for (from, to) in ranges {
        for byte in &mut out[*from..(*to).min(text.len())] {
            if *byte != b'\n' {
                *byte = b' ';
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}
