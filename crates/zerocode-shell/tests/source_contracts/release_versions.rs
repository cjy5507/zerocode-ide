//! §2.1–2.2 of `docs/design/versioned-auto-update.md`: the version is said
//! in one place and followed by two, and the updater's private key never
//! enters the tree.

use std::path::{Path, PathBuf};
use std::process::Command;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(path: impl AsRef<Path>) -> String {
    let path = path.as_ref();
    std::fs::read_to_string(path).unwrap_or_else(|why| panic!("{}: {why}", path.display()))
}

/// The `version = "…"` line under `[workspace.package]` of a Cargo manifest —
/// that section's own line, not a dependency's.
fn workspace_package_version(manifest: &str) -> &str {
    let (_, section) = manifest
        .split_once("[workspace.package]")
        .expect("a [workspace.package] section");
    let section = section.split("\n[").next().unwrap_or(section);
    let line = section
        .lines()
        .find(|line| line.starts_with("version"))
        .expect("a version line under [workspace.package]");
    line.split('"').nth(1).expect("a quoted version")
}

/// The top-level `"version"` of tauri.conf.json — the one at two-space depth.
fn tauri_conf_version(conf: &str) -> &str {
    let line = conf
        .lines()
        .find(|line| line.starts_with("  \"version\":"))
        .expect("a top-level \"version\" in tauri.conf.json");
    line.split('"').nth(3).expect("a quoted version")
}

/// The truth is the root `Cargo.toml` `[workspace.package] version`; the
/// window's `tauri.conf.json` and `zo-ide/Cargo.toml` say the same letters,
/// and the crate compiled here inherits it (`version.workspace = true`).
/// `tools/release/bump.sh` is the one hand that moves all three.
#[test]
fn the_three_versions_agree() {
    let root = repo_root();
    let workspace = read(root.join("Cargo.toml"));
    let tauri = read(root.join("crates/zerocode-shell/tauri.conf.json"));
    let zo = read(root.join("zo-ide/Cargo.toml"));
    let truth = workspace_package_version(&workspace);
    assert!(
        truth.split('.').count() == 3 && truth.split('.').all(|part| part.parse::<u64>().is_ok()),
        "the workspace version is a plain semver triple, not {truth:?}"
    );
    assert_eq!(
        tauri_conf_version(&tauri),
        truth,
        "crates/zerocode-shell/tauri.conf.json \"version\" follows the root Cargo.toml"
    );
    assert_eq!(
        workspace_package_version(&zo),
        truth,
        "zo-ide/Cargo.toml [workspace.package] version follows the root Cargo.toml"
    );
    assert_eq!(
        env!("CARGO_PKG_VERSION"),
        truth,
        "the shell crate inherits the workspace version"
    );
}

/// Standard base64 of `bytes`, whose length is a multiple of three here, so
/// the encoding of a header prefix is a fixed string any longer box starts with.
fn base64_of(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    assert_eq!(bytes.len() % 3, 0, "a prefix aligned to three bytes");
    bytes
        .chunks(3)
        .flat_map(|chunk| {
            let n = (u32::from(chunk[0]) << 16) | (u32::from(chunk[1]) << 8) | u32::from(chunk[2]);
            (0..4).map(move |i| ALPHABET[((n >> (18 - 6 * i)) & 63) as usize] as char)
        })
        .collect()
}

/// The updater's private key lives at
/// `~/.local/share/zerocode/release/keys/updater.key` on the lane machine
/// only. No tracked file carries a minisign secret-key box — plain (an
/// `untrusted comment` line naming an encrypted secret key) or base64 of it,
/// which is how `tauri signer generate` writes the file — and `.gitignore`
/// refuses every `*.key` so a stray `git add -A` cannot bring one in.
#[test]
fn no_private_key_in_the_tree() {
    let root = repo_root();
    let ignore = read(root.join(".gitignore"));
    assert!(
        ignore.lines().any(|line| line.trim() == "*.key"),
        ".gitignore refuses *.key"
    );
    let listed = Command::new("git")
        .arg("-C")
        .arg(&root)
        .args(["ls-files", "-z"])
        .output()
        .expect("git ls-files runs");
    assert!(listed.status.success(), "git ls-files failed");
    // Built from parts so this file's own text does not carry the header;
    // the base64 prefixes cover rsign (tauri) and minisign boxes, each
    // aligned so the encoding is one fixed string.
    let header = ["untrusted", " comment:"].concat();
    let secret = ["secret", " key"].concat();
    let encoded: Vec<String> = ["rsign", "minisign"]
        .iter()
        .map(|tool| {
            let plain = format!("{header} {tool} encrypted secret ");
            base64_of(plain.as_bytes())
        })
        .collect();
    let mut offenders = Vec::new();
    for name in String::from_utf8_lossy(&listed.stdout).split('\0') {
        if name.is_empty() {
            continue;
        }
        let path = root.join(name);
        let Ok(bytes) = std::fs::read(&path) else {
            continue; // a submodule or a directory entry
        };
        let text = String::from_utf8_lossy(&bytes);
        let plain = text
            .lines()
            .any(|line| line.contains(&header) && line.contains(&secret));
        let boxed = encoded.iter().any(|prefix| text.contains(prefix.as_str()));
        if plain || boxed {
            offenders.push(name.to_string());
        }
    }
    assert!(
        offenders.is_empty(),
        "tracked files carrying a minisign secret-key header: {offenders:?}"
    );
}
