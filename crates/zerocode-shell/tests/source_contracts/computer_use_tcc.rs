//! The Computer Use permission card's TCC rows (t-6058): what the TCC
//! database records for this app's own two bundles, read against the
//! signatures they carry now. One table decides a row's grant and its
//! buttons (`COMPUTER_PERMISSION_GRANTS`, zerocode-core); the window only
//! gives those words their sentences. The database is read by one statement
//! bound to this app's bundle ids, the requirement only by the Security
//! framework, Full Disk Access by one open attempt, and `tccutil` runs through
//! one road that always names one of this app's bundles.

use std::path::Path;

use zerocode_core::computer_use::{
    COMPUTER_PERMISSION_GRANTS, ComputerPermissionRowAction, ComputerPermissionUnreadable,
};

use super::support::{block_after, strip_comments, strip_rust_comments, window_source};

fn shell_source(name: &str) -> String {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    std::fs::read_to_string(src.join(name)).unwrap_or_else(|why| panic!("{name}: {why}"))
}

/// A file's code without its test module.
fn shipped(name: &str) -> String {
    let source = shell_source(name);
    source
        .split_once("\n#[cfg(test)]\nmod tests {")
        .map_or(source.clone(), |(code, _)| code.to_string())
}

fn wire(value: impl serde::Serialize) -> String {
    serde_json::to_value(value)
        .expect("a wire word")
        .as_str()
        .expect("a string")
        .to_string()
}

/// Every reason a row can be unread and every button it can carry — a new
/// one fails to compile here until the window has words for it.
fn every_unreadable() -> Vec<String> {
    use ComputerPermissionUnreadable::{Database, NoFullDiskAccess, Requirement, Signature};
    let all = [NoFullDiskAccess, Database, Requirement, Signature];
    for reason in all {
        match reason {
            NoFullDiskAccess | Database | Requirement | Signature => {}
        }
    }
    all.into_iter().map(wire).collect()
}

fn every_action() -> Vec<String> {
    use ComputerPermissionRowAction::{OpenSettings, Reset};
    let all = [Reset, OpenSettings];
    for action in all {
        match action {
            Reset | OpenSettings => {}
        }
    }
    all.into_iter().map(wire).collect()
}

/// The words one section of `COMPUTER_TCC_WORDS` has sentences for, in order.
fn words_in<'a>(table: &'a str, section: &str) -> Vec<&'a str> {
    let opened = format!("  {section}: Object.freeze({{");
    let at = table
        .find(&opened)
        .unwrap_or_else(|| panic!("COMPUTER_TCC_WORDS lost its `{section}` section"));
    let rest = &table[at + opened.len()..];
    let body = &rest[..rest.find("\n  }),").expect("the section closes")];
    body.lines()
        .filter_map(|line| line.trim().split_once(": { key: "))
        .map(|(word, _)| word.trim_matches('"'))
        .collect()
}

/// The window's words for a TCC row are one entry per word the core table
/// and its enums can put on the wire — no more, no fewer — and each is asked
/// by key, so every catalog must carry it.
#[test]
fn the_tcc_rows_words_are_one_sentence_per_word_the_core_can_say() {
    let window = window_source();
    let table = block_after(window, "const COMPUTER_TCC_WORDS = Object.freeze({");
    let grants = COMPUTER_PERMISSION_GRANTS
        .iter()
        .map(|(grant, _)| wire(grant))
        .collect::<Vec<_>>();
    assert_eq!(words_in(table, "grant"), grants, "one sentence per grant");
    assert_eq!(words_in(table, "unreadable"), every_unreadable());
    assert_eq!(words_in(table, "action"), every_action());
    for key in table
        .split("key: \"")
        .skip(1)
        .filter_map(|rest| rest.split('"').next())
    {
        assert!(
            key.starts_with("computerUse.tcc"),
            "`{key}` is not a TCC row sentence"
        );
    }
}

/// Every rule of `styles` with a selector that opens with one of `prefixes`,
/// as (selectors, body) — innermost rules only, read the way
/// `a_state_change_is_eased_not_snapped` reads the sheet. Chosen by name, not
/// by line: the router's table is the next rule down and has pixels of its
/// own to answer for.
fn rules_named<'a>(styles: &'a str, prefixes: &[&str]) -> Vec<(&'a str, &'a str)> {
    let mut rules = Vec::new();
    let mut opened: Vec<&str> = Vec::new();
    let mut from = 0;
    for (at, glyph) in styles.char_indices() {
        match glyph {
            '{' => {
                opened.push(styles[from..at].trim());
                from = at + 1;
            }
            '}' => {
                let body = styles[from..at].trim();
                from = at + 1;
                if let Some(selectors) = opened.pop()
                    && !body.is_empty()
                    && selectors
                        .split(',')
                        .any(|one| prefixes.iter().any(|prefix| one.trim().starts_with(prefix)))
                {
                    rules.push((selectors, body));
                }
            }
            _ => {}
        }
    }
    rules
}

/// The card paints what the backend read: a row's buttons are the ones the
/// row carries (`row.actions`, the core table's), and nothing in the window
/// decides a grant or a button by the grant's name — so no row but a stale
/// one can ever show a reset. And it paints in the palette's measures: every
/// rule named for the rows or the permission cards draws its lines and
/// corners from tokens, never a pixel of its own (t-9719).
#[test]
fn a_tcc_rows_buttons_are_the_ones_the_row_carries() {
    let window = window_source();
    let node = block_after(window, "function computerUseTccRowNode(row) {");
    assert!(
        node.contains("for (const action of row.actions ?? [])"),
        "the row's buttons are not its own:\n{node}"
    );
    assert!(
        node.contains("invoke(\"computer_use_tcc_row_action\", { id: row.id, bundleId: row.bundle_id, action })"),
        "a row's button names another row:\n{node}"
    );
    for code in [
        node,
        block_after(window, "function paintComputerUseTccRows(id) {"),
        block_after(window, "function computerTccGrantWords(row) {"),
    ] {
        assert!(
            !code.contains("\"stale\"") && !code.contains("grant ===") && !code.contains("tccutil"),
            "the window judges a grant of its own:\n{code}"
        );
    }
    let main = shell_source("main.rs");
    assert!(
        main.contains("            computer_use_tcc_row_action,"),
        "the Tauri handler does not expose the row's door"
    );

    let styles = strip_comments(include_str!("../../../../ui/shell.css"));
    let card = rules_named(&styles, &[".computer-use-tcc-", ".computer-permission-"]);
    for (rule, tokens) in [
        (
            ".computer-use-tcc-judged",
            &["var(--rule-width)", "var(--radius-pill)"][..],
        ),
        (".computer-permission-recovery", &["var(--rule-width)"][..]),
    ] {
        let (_, body) = card
            .iter()
            .find(|(selectors, _)| *selectors == rule)
            .unwrap_or_else(|| panic!("`{rule}` is gone from ui/shell.css"));
        for token in tokens {
            assert!(
                body.contains(token),
                "`{rule}` no longer reads `{token}`:\n{body}"
            );
        }
    }
    for (selectors, body) in &card {
        let pixel = body
            .match_indices("px")
            .any(|(at, _)| at > 0 && body.as_bytes()[at - 1].is_ascii_digit());
        assert!(
            !pixel,
            "`{selectors}` measures with a pixel of its own — the card's lines, \
             corners and gaps are the palette's (`--rule-width`, `--radius-*`, \
             `--space-*`):\n{body}"
        );
    }
}

/// The database is read by one statement, bound to the service and one of
/// this app's bundle ids: no other app's row is read, counted or logged, and
/// no statement is spelled from a value.
#[test]
fn the_tcc_reader_reads_this_apps_rows_by_one_bound_statement() {
    let reader = shipped("computer_use/macos/tcc.rs");
    assert_eq!(reader.matches("FROM access").count(), 1, "one statement");
    let statement = &reader[reader.find("\"SELECT").expect("the statement")..];
    let statement = &statement[..statement.find(')').expect("its end")];
    for bound in ["service = ?1", "client = ?2", "client_type = ?3"] {
        assert!(
            statement.contains(bound),
            "the statement is not bound by `{bound}`:\n{statement}"
        );
    }
    assert!(
        !reader.contains("prepare(&format!") && !reader.contains("query_row(&format!"),
        "a statement spelled from a value"
    );
    // The database is opened read-only by the crate's one road for somebody
    // else's database, after Full Disk Access is judged by its one open.
    assert!(reader.contains("crate::sqlite_read::open_read_only(database)"));
    assert!(reader.contains("match tcc_database_access(database) {"));
    let macos = shipped("computer_use/macos.rs");
    for bundle_ids in ["HELPER_BUNDLE_ID,", "APP_BUNDLE_ID,"] {
        assert_eq!(
            block_after(&macos, "    const fn bundle_id(self) -> &'static str {")
                .matches(bundle_ids)
                .count(),
            1,
            "the rows read are this app's two bundles"
        );
    }
}

/// The recorded requirement is the Security framework's to read — its text
/// and whether the bundle as signed now satisfies it — and no byte of the
/// blob is parsed here.
#[test]
fn a_recorded_requirement_is_read_only_by_the_security_framework() {
    let reader = shipped("computer_use/macos/tcc.rs");
    for call in [
        "SecRequirementCreateWithData(",
        "SecRequirementCopyString(",
        "SecStaticCodeCreateWithPath(",
        "SecStaticCodeCheckValidity(",
    ] {
        assert!(reader.contains(call), "the reader lost `{call}`");
    }
    for parsing in ["fade0c", "0xfade", "from_be_bytes", "blob["] {
        assert!(
            !reader.to_lowercase().contains(parsing),
            "the reader parses a requirement's bytes itself (`{parsing}`)"
        );
    }
}

/// Full Disk Access is judged by one open attempt on a TCC database — the
/// person's for the developer permission row, the system's for Computer
/// Use's rows — and the database's place is spelled once.
#[test]
fn full_disk_access_is_one_open_and_the_database_is_spelled_once() {
    let permissions = shipped("developer_permissions.rs");
    let place = "\"Library/Application Support/com.apple.TCC/TCC.db\"";
    assert_eq!(permissions.matches(place).count(), 1);
    let open = block_after(
        &permissions,
        "pub(crate) fn tcc_database_access(database: &std::path::Path) -> PermissionStatus {",
    );
    assert!(open.contains("std::fs::File::open(database)"));
    assert!(
        block_after(
            &permissions,
            "fn full_disk_access_status() -> PermissionStatus {"
        )
        .contains("tcc_database_access(&home.join(TCC_DATABASE))"),
        "the developer row judges Full Disk Access some other way"
    );
    for name in [
        "computer_use/macos.rs",
        "computer_use/macos/tcc.rs",
        "computer_use/mod.rs",
    ] {
        assert!(
            !strip_rust_comments(&shipped(name)).contains("com.apple.TCC"),
            "{name} spells a TCC database of its own"
        );
    }
}

/// `tccutil` runs through one road, three words long, the last one of this
/// app's own bundle ids — a missing bundle would reset every app's row for
/// the service — and the row door lets a reset through only where the table
/// gives the row's grant one.
#[test]
fn tccutil_runs_through_one_three_word_road_and_the_row_door_asks_the_table() {
    let macos = shipped("computer_use/macos.rs");
    assert_eq!(macos.matches("/usr/bin/tccutil").count(), 1, "one road");
    let road = block_after(&macos, "fn reset_row(");
    assert!(road.contains(".args(reset_args(target, &row.bundle_id)?)"));
    let args = block_after(&macos, "fn reset_args<'a>(");
    assert!(
        args.contains("Result<[&'a str; 3], ComputerUseError>")
            && args.contains("PermissionSubject::BOTH"),
        "reset_args lost its three-word contract:\n{args}"
    );
    let door = block_after(&macos, "pub(super) fn tcc_row_action(");
    let asks = door
        .find("if !row_offers(read, action)")
        .expect("the door asks the table");
    let resets = door
        .find("reset_row(&target, &row)")
        .expect("the door resets the row");
    assert!(asks < resets, "the door resets before it asks:\n{door}");
    assert!(
        block_after(&macos, "fn row_offers(").contains(".actions().contains(&action)"),
        "the door's question is not the table's"
    );
}
