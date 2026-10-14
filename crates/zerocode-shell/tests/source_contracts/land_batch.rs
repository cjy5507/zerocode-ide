//! t-45315, test 16 of the design's table: a person's press on an integration batch is the main
//! window's alone. The core defines the person as a presser once (`person_press`), and the only
//! other shipped source that names it is the board's four commands, each proving the main window
//! first — no verb the CLI plans, no beat, no hook and no worker endpoint can say the person
//! pressed.

use std::path::Path;

fn shell_source(name: &str) -> String {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    std::fs::read_to_string(src.join(name)).unwrap_or_else(|why| panic!("{name}: {why}"))
}

fn core_source(name: &str) -> String {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../zerocode-core/src");
    std::fs::read_to_string(src.join(name)).unwrap_or_else(|why| panic!("{name}: {why}"))
}

/// Every shipped Rust source under `dir`: test folders and test files are left out.
fn shipped_rust_sources(dir: &Path, into: &mut Vec<(String, String)>) {
    let entries = std::fs::read_dir(dir).unwrap_or_else(|why| panic!("{}: {why}", dir.display()));
    for entry in entries {
        let path = entry.expect("a directory entry").path();
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_string();
        if path.is_dir() {
            if name != "tests" && name != "target" {
                shipped_rust_sources(&path, into);
            }
        } else if name.ends_with(".rs") && name != "tests.rs" && !name.ends_with("_tests.rs") {
            let text = std::fs::read_to_string(&path)
                .unwrap_or_else(|why| panic!("{}: {why}", path.display()));
            into.push((path.to_string_lossy().replace('\\', "/"), text));
        }
    }
}

#[test]
fn only_the_window_command_mints_a_person_press() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut sources = Vec::new();
    shipped_rust_sources(&crates, &mut sources);
    assert!(
        sources.len() > 100,
        "the walk read {} sources — it is not reading the workspace",
        sources.len()
    );
    let allowed = [
        "zerocode-core/src/orchestration/land_batch.rs",
        "zerocode-shell/src/cmd/board.rs",
    ];
    let mut minted = Vec::new();
    for (path, text) in &sources {
        let code = super::support::strip_rust_comments(text);
        for name in ["person_press(", "PressedBy::Person"] {
            if !code.contains(name) {
                continue;
            }
            assert!(
                allowed.iter().any(|one| path.ends_with(one)),
                "`{name}` is named in {path}: a person's press is minted by the window's board \
                 command alone"
            );
            minted.push(path.clone());
        }
    }
    assert!(
        minted.iter().any(|path| path.ends_with(allowed[1])),
        "no board command presses a batch as the person"
    );

    let board = super::support::strip_rust_comments(&shell_source("cmd/board.rs"));
    let doors = [
        "pub(crate) async fn land_batch_promote(",
        "pub(crate) async fn land_batch_bisect(",
        "pub(crate) async fn land_batch_restack(",
        "pub(crate) async fn land_batch_cancel(",
    ];
    for door in doors {
        assert!(board.contains(door), "the board has no `{door}`");
        let body = super::support::block_after(&board, door);
        let main_only = body
            .find("crate::from_the_main_webview(&webview)?")
            .unwrap_or_else(|| panic!("`{door}` is open beyond the main window:\n{body}"));
        let pressed = body
            .find("person_press()")
            .unwrap_or_else(|| panic!("`{door}` does not press as the person:\n{body}"));
        assert!(
            main_only < pressed && body.contains("spawn_blocking"),
            "`{door}` presses before it proves the main window, or on the main thread:\n{body}"
        );
    }
    assert_eq!(
        board.matches("person_press(").count(),
        doors.len(),
        "the board names the person's press outside its four commands"
    );

    let core = core_source("orchestration/land_batch.rs");
    let planner = super::support::block_after(&core, "pub(super) fn plan(");
    for forbidden in ["person_press", "PressedBy::Person"] {
        assert!(
            !planner.contains(forbidden),
            "the verb the CLI plans reaches `{forbidden}`:\n{planner}"
        );
    }
}
