//! `zerocode vault-types` — the one-shot backfill of the vault's `type:` key
//! (t-43992 unit 2): a dry run by default that changes nothing, `--write` to
//! add the key, never a file under `raw/`, and the usage on anything else.

use std::path::Path;
use std::process::Command;

fn zerocode() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_zerocode"));
    // Neither the developer's pane variable nor their saved vault may leak
    // into a test that names its own.
    command.env_remove(zerocode_core::second_brain::VAULT_ENV);
    command
}

const TOOL: &str = "---\ntags: [tool]\n---\nAn editor.\n";
const RAW: &str = "---\ntags: [tool]\n---\nraw text\n";

fn vault() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    zerocode_core::second_brain::setup(root.path()).unwrap();
    std::fs::write(root.path().join("wiki/obsidian.md"), TOOL).unwrap();
    std::fs::write(root.path().join("raw/clip.md"), RAW).unwrap();
    root
}

fn read(root: &Path, relative: &str) -> String {
    std::fs::read_to_string(root.join(relative)).unwrap()
}

#[test]
fn a_dry_run_prints_the_plan_and_changes_nothing() {
    let root = vault();
    let log = read(root.path(), "wiki/log.md");
    let output = zerocode()
        .args(["vault-types", "--vault"])
        .arg(root.path())
        .output()
        .expect("run zerocode vault-types");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(output.status.code(), Some(0), "{stdout}");
    assert!(stdout.contains("wiki/obsidian.md → entity"), "{stdout}");
    assert!(
        stdout.contains("--write"),
        "the dry run does not say how to apply it:\n{stdout}"
    );
    assert_eq!(read(root.path(), "wiki/obsidian.md"), TOOL);
    assert_eq!(read(root.path(), "wiki/log.md"), log);
}

#[test]
fn write_adds_the_key_logs_it_and_leaves_raw_alone() {
    let root = vault();
    let output = zerocode()
        .args(["vault-types", "--write", "--vault"])
        .arg(root.path())
        .output()
        .expect("run zerocode vault-types --write");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(output.status.code(), Some(0), "{stdout}");
    assert_eq!(
        read(root.path(), "wiki/obsidian.md"),
        format!("---\ntype: entity\n{}", &TOOL[4..])
    );
    assert_eq!(read(root.path(), "raw/clip.md"), RAW);
    assert!(read(root.path(), "wiki/log.md").contains("zerocode vault-types"));
}

#[test]
fn anything_else_is_refused_with_the_usage() {
    let output = zerocode()
        .args(["vault-types", "--nope"])
        .output()
        .expect("run zerocode vault-types --nope");
    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("zerocode vault-types [--vault <dir>] [--write]")
    );
}
