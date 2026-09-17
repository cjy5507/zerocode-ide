//! `zerocode vault-lint` against the fixture vault: red with the exact counts
//! `fixtures/vault-lint/expected.json` pins, green on a vault holding nothing,
//! and named by the three roads a vault reaches it on.

use std::path::{Path, PathBuf};
use std::process::Command;

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/vault-lint")
}

fn zerocode() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_zerocode"));
    // Neither the developer's pane variable nor their saved vault may leak
    // into a test that names its own.
    command.env_remove(zerocode_core::second_brain::VAULT_ENV);
    command
}

#[test]
fn the_fixture_vault_is_red_with_exactly_its_expected_counts() {
    let output = zerocode()
        .args(["vault-lint", "--vault"])
        .arg(fixture().join("vault"))
        .output()
        .expect("run zerocode vault-lint");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        output.status.code(),
        Some(1),
        "a vault with findings must exit 1:\n{stdout}"
    );
    for row in [
        "index_gaps                2   pages not in wiki/index.md",
        "ghost_links               1   link targets without a page",
        "orphans                   1   pages no page links to",
        "missing_frontmatter       1   pages without source/ingested_at",
        "undeclared_relations      1   prose links no relation key declares",
        "unlogged_raw              1   raw items without an ingestion record",
        "contradictions            1   declared `contradicts`",
        "superseded                1   pages a `supersedes`",
        // Seated by the scan since 09-07: the fixture has no two pages that look
        // like one, and a zero is not a dash — a dash says nobody computed it.
        "merge_candidates          0   pairs that look like one page (dedupe lens, t-2931",
        "    [[never-written]] ← wiki/hub.md",
        "    wiki/no-source.md (source)",
        "    wiki/hub.md → wiki/linked-target.md",
        "    raw/unlogged.md",
        "findings: 7 → exit 1",
    ] {
        assert!(stdout.contains(row), "missing row `{row}` in:\n{stdout}");
    }
}

#[test]
fn json_is_the_table_the_producer_pinned() {
    let output = zerocode()
        .args(["vault-lint", "--json", "--vault"])
        .arg(fixture().join("vault"))
        .output()
        .expect("run zerocode vault-lint --json");
    assert_eq!(output.status.code(), Some(1));
    let printed: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("--json prints one JSON document");
    let expected: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(fixture().join("expected.json")).expect("expected.json"),
    )
    .expect("expected.json parses");
    assert_eq!(printed, expected);
}

#[test]
fn a_vault_holding_nothing_is_green_and_the_pane_variable_names_it() {
    let root = tempfile::tempdir().unwrap();
    zerocode_core::second_brain::setup(root.path()).unwrap();
    let output = zerocode()
        .arg("vault-lint")
        .env(zerocode_core::second_brain::VAULT_ENV, root.path())
        .output()
        .expect("run zerocode vault-lint");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(output.status.code(), Some(0), "{stdout}");
    assert!(
        stdout.contains(&format!("({})", zerocode_core::second_brain::VAULT_ENV)),
        "the road the vault came by is not named:\n{stdout}"
    );
    assert!(stdout.contains("findings: 0 → exit 0"), "{stdout}");
}

#[test]
fn no_vault_anywhere_is_exit_two_and_so_is_a_folder_without_a_wiki() {
    // The saved-settings road is the machine's own document, which this test
    // cannot blank; point the argument at a folder that is not a vault, which
    // is refused before anything is read out of settings.
    let not_a_vault = tempfile::tempdir().unwrap();
    let output = zerocode()
        .args(["vault-lint", "--vault"])
        .arg(not_a_vault.path())
        .output()
        .expect("run zerocode vault-lint");
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("has no wiki/"), "{stderr}");

    // `--help` is the CLI's own door, answered before any command runs, and
    // its usage names this command.
    let output = zerocode()
        .args(["vault-lint", "--help"])
        .output()
        .expect("run zerocode vault-lint --help");
    assert_eq!(output.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&output.stdout).contains("zerocode vault-lint"));

    let output = zerocode()
        .args(["vault-lint", "--nope"])
        .output()
        .expect("run zerocode vault-lint --nope");
    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("zerocode vault-lint [--vault <dir>]")
    );
}
