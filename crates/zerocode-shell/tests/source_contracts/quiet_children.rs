//! AO #5309: 창이 띄우는 자식은 콘솔 창을 깜빡이지 않는다.
//!
//! `main.rs` 는 release 에서 `windows_subsystem = "windows"` 로 서므로 창에는
//! 콘솔이 없다. 그 아래에서 콘솔 자식(`git`, `gh`, `security`, `adb`,
//! `powershell`…)을 맨 `Command::new` 로 띄우면 윈도우가 자식마다 새 콘솔
//! 창을 만들어 화면 한가운데에서 깜빡인다. 답은 문 하나뿐:
//! `proc::quiet_command`(와 tokio 판)가 `CREATE_NO_WINDOW` 를 다는 유일한
//! 자리이고, 이 크레이트의 나머지에는 맨 `Command::new(` 가 한 곳도 없다.
//!
//! 예외는 하나뿐이고 이유가 있다 — `src/bin/zerocode-mirror.rs` 는 터미널
//! 안에서 `PATH` 위에 서는 콘솔 바이너리다. 그 자식은 사람이 보고 있는
//! 콘솔을 **물려받아야** 하므로 `CREATE_NO_WINDOW` 는 정확히 틀린 답이고,
//! 별개의 바이너리 타깃이라 `crate::proc` 가 닿지도 않는다.

use std::path::{Path, PathBuf};

use crate::support::strip_rust_comments;

/// 이 계약이 읽지 않는 두 파일과, 읽지 않는 이유.
const EXEMPT: &[(&str, &str)] = &[
    ("proc.rs", "the door itself"),
    (
        "bin/zerocode-mirror.rs",
        "a console binary whose child must inherit the person's console",
    ),
];

/// 창 크레이트의 모든 `.rs` — 목록을 손으로 들지 않는다. 손으로 든 목록은
/// 새로 생긴 파일을 놓치는데, 그게 바로 이 계약이 막으려는 실패다.
fn shell_sources() -> Vec<(String, String)> {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut found = Vec::new();
    let mut pending = vec![src.clone()];
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(&directory).expect("shell source directory") {
            let path: PathBuf = entry.expect("shell source entry").path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().and_then(|it| it.to_str()) == Some("rs") {
                let name = path
                    .strip_prefix(&src)
                    .expect("shell source stays in src")
                    .to_string_lossy()
                    .replace('\\', "/");
                found.push((name, std::fs::read_to_string(&path).expect("shell source")));
            }
        }
    }
    found.sort();
    assert!(
        found.len() > 100,
        "the shell source walk found only {} files",
        found.len()
    );
    found
}

/// 주석도 문자열도 없앤 코드.
///
/// 문자열까지 비우는 것이 요점이다: 이 크레이트의 시험들은 자기 계약을 적을
/// 때 `"Command::new(\"gh\")"` 처럼 그 철자를 **인용**하는데, 인용은 자식을
/// 띄우지 않는다. 인용을 코드로 세면 계약이 제 시험을 고발한다.
fn code_only(source: &str) -> String {
    let without_prose = strip_rust_comments(source);
    let bytes: Vec<char> = without_prose.chars().collect();
    let mut out = String::with_capacity(without_prose.len());
    let mut at = 0;
    while at < bytes.len() {
        let glyph = bytes[at];
        // 원시 문자열 `r"…"` / `r#"…"#` 은 제 울타리까지 통째로 지운다.
        if glyph == 'r' && bytes.get(at + 1).is_some_and(|it| *it == '"' || *it == '#') {
            let mut hashes = 0;
            while bytes.get(at + 1 + hashes) == Some(&'#') {
                hashes += 1;
            }
            if bytes.get(at + 1 + hashes) == Some(&'"') {
                let fence: String = std::iter::once('"')
                    .chain(std::iter::repeat_n('#', hashes))
                    .collect();
                let rest: String = bytes[at + 2 + hashes..].iter().collect();
                let end = rest.find(&fence).map_or(bytes.len(), |to| {
                    at + 2 + hashes + rest[..to].chars().count() + fence.chars().count()
                });
                out.push_str("\"\"");
                at = end;
                continue;
            }
        }
        if glyph == '"' {
            at += 1;
            while at < bytes.len() && bytes[at] != '"' {
                if bytes[at] == '\\' {
                    at += 1;
                }
                at += 1;
            }
            at += 1;
            out.push_str("\"\"");
            continue;
        }
        out.push(glyph);
        at += 1;
    }
    out
}

#[test]
fn nothing_in_the_window_starts_a_child_but_the_one_door() {
    let mut offenders = Vec::new();
    for (name, source) in shell_sources() {
        if EXEMPT.iter().any(|(exempt, _)| *exempt == name) {
            continue;
        }
        let count = code_only(&source).matches("Command::new(").count();
        if count > 0 {
            offenders.push(format!("{name}: {count}"));
        }
    }
    assert!(
        offenders.is_empty(),
        "these sources start a child without `proc::quiet_command`, so the release \
window flashes a console window on Windows (AO #5309):\n  {}",
        offenders.join("\n  ")
    );
}

#[test]
fn the_flag_is_spelled_once_and_only_the_door_reaches_for_it() {
    let sources = shell_sources();
    let door = sources
        .iter()
        .find(|(name, _)| name == "proc.rs")
        .map(|(_, source)| code_only(source))
        .expect("crates/zerocode-shell/src/proc.rs is the one door");

    assert_eq!(
        door.matches("0x0800_0000").count(),
        1,
        "CREATE_NO_WINDOW is spelled once, in the door:\n{door}"
    );
    for one in ["fn quiet_command(", "fn quiet_tokio_command("] {
        assert_eq!(door.matches(one).count(), 1, "one `{one}`");
    }

    for (name, source) in &sources {
        if name == "proc.rs" {
            continue;
        }
        let code = code_only(source);
        assert!(
            !code.contains("creation_flags("),
            "{name} sets creation flags behind the window's back; the door owns them"
        );
        assert!(
            !code.contains("CREATE_NO_WINDOW"),
            "{name} names CREATE_NO_WINDOW; the door owns that constant"
        );
    }
}

/// 116 곳을 한 문으로 모으는 일이 **동작을 바꾸지 않는다**는 것이 이 과업의
/// 값이다. 문이 플래그 말고 아무것도 건드리지 않으면, 자식의 인자도 환경도
/// cwd 도 stdio 도 부르는 쪽이 쓴 그대로다.
#[test]
fn the_door_only_adds_a_flag() {
    let door = code_only(
        &std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/proc.rs"))
            .expect("the door"),
    );
    let (shipped, _) = door.split_once("#[cfg(test)]").unwrap_or((&door, ""));
    for forbidden in [
        ".arg(",
        ".args(",
        ".env(",
        ".envs(",
        ".env_clear(",
        ".env_remove(",
        ".current_dir(",
        ".stdin(",
        ".stdout(",
        ".stderr(",
    ] {
        assert!(
            !shipped.contains(forbidden),
            "the door touched `{forbidden}`: it may add a flag and nothing else, \
or every call site quietly changed what it runs"
        );
    }
}
