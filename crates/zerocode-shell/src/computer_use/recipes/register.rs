//! The Flow card's two hands on a recipe document (t-93871): taking one away,
//! and standing a plain recipe up as a Flow.
//!
//! Both are the person's (`flow_delete` and `flow_register` stand on the
//! window's person-only list), both name a recipe by its slug alone — a slug
//! is the word `recipe_slug` makes of a name, so no separator, no `..` and no
//! drive can ride in it and nothing outside the recipes folder is reachable —
//! and neither keeps a store of its own: the document is the truth, as it is
//! for the roster. An agent's road to the second is `recipe-save --flow-check`
//! (the same registration, given the words the screen shows when it worked).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;
use zerocode_core::computer_flow::{
    Check, CheckKind, Confirm, EvidenceLevel, Fingerprint, FlowSpec, Policy, page_host, parse_flow,
};
use zerocode_core::computer_recipe::{
    RECIPE_CHECKS, RecipeLine, RecipeTool, browser_tab_target, recipe_line_text, recipe_lines,
    recipe_placeholders,
};
use zerocode_core::computer_use::{COMPUTER_USE_PROTOCOL_VERSION, recipe_slug, verb_method};
use zerocode_core::computer_use_protocol::error_code;

use super::{ComputerUseError, recipes_dir, splice_flow_sections, walk_slug};

/// The role every window-bearing app's tree holds, so "the app is open" is a
/// check the helper can answer (`wait-for --role AXWindow --app …`).
const WINDOW_ROLE: &str = "AXWindow";
/// A browser page always has a body: "the page is open" as a `wait` check.
const PAGE_SELECTOR: &str = "body";
/// How long that `wait` holds, in ms — the table's own unit for a `wait`.
const PAGE_WAIT_MS: &str = "3000";
/// What opens the note a starter condition leaves in the document, and the
/// sentence under it. The note sits after the Flow sections, in a section of
/// its own, so `flow_set` keeps it when it rewrites the two it owns.
const STARTER_HEADING: &str = "## Rewrite the check";
const STARTER_NOTE: &str = "The Flow card wrote the check above only to stand this recipe up as a Flow: it holds while the app (or page) is open, which says little. Replace it with what the screen shows when this run has really worked. The `fingerprint` line lists the apps and sites this Flow may act in; if a run stops saying it met one that line does not name, add it there.";

fn refusal(message: impl Into<String>) -> ComputerUseError {
    ComputerUseError::new(error_code::INVALID_ARGUMENT, message)
}

/// The document a slug names, and nothing else. A slug is already the word
/// `recipe_slug` makes of a name — lowercase letters, digits, single hyphens —
/// so the word is refused unless it is its own slug: `../x`, `/etc/x`, `a/b`
/// and `..` all differ from theirs and none reaches outside the folder.
pub fn recipe_file(root: &Path, slug: &str) -> Result<PathBuf, ComputerUseError> {
    if slug.is_empty() || recipe_slug(slug) != slug {
        return Err(refusal(format!(
            "'{slug}' is not a recipe slug — a slug is the lowercase letters, digits and single hyphens of a recipe's file name"
        )));
    }
    Ok(recipes_dir(root).join(format!("{slug}.md")))
}

/// A recipe's document and text, by exact slug.
fn existing(root: &Path, slug: &str) -> Result<(PathBuf, String), ComputerUseError> {
    let file = recipe_file(root, slug)?;
    let text =
        std::fs::read_to_string(&file).map_err(|_| refusal(format!("no recipe named '{slug}'")))?;
    Ok((file, text))
}

/// What deleting a recipe took, and what it left.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FlowDeleted {
    /// The document taken away.
    pub file: String,
    /// The walk records dropped from the session folders — the Flow's last
    /// verdict, which the card reads from the newest folder that walked its slug.
    pub walks: usize,
    /// The money ledger left where the document stood, when it had one. A
    /// ledger is the record that a transaction was acted on, and what stops a
    /// later recipe of the same name from pressing it again: it stays, for the
    /// person to remove by hand if they mean to.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ledger: Option<String>,
}

/// Take a recipe — a Flow or not — away: its document, and the walk records
/// that name its slug in the session folders, so a Flow made again under the
/// same name does not inherit the last verdict of the one that went. The
/// folders' steps, frames and verdicts stay (they are the sessions', pruned by
/// age); the automations that walk the slug are the caller's to name and drop.
/// A slug that is not one, or names no document, is refused by name.
pub fn flow_delete(root: &Path, slug: &str) -> Result<FlowDeleted, ComputerUseError> {
    let file = recipe_file(root, slug)?;
    if !file.is_file() {
        return Err(refusal(format!("no recipe named '{slug}'")));
    }
    std::fs::remove_file(&file).map_err(|error| {
        ComputerUseError::new(
            error_code::ACCESSIBILITY_ERROR,
            format!("could not delete '{slug}': {error}"),
        )
    })?;
    let ledger = crate::computer_use::guarded::Ledger::beside(&file)
        .path()
        .to_path_buf();
    Ok(FlowDeleted {
        file: file.display().to_string(),
        walks: forget_runs_of(root, slug),
        ledger: ledger.is_file().then(|| ledger.display().to_string()),
    })
}

/// Drop every walk record that names `slug` from every folder a walk writes
/// to — the session folders and the scheduled runs' — so a recipe saved later
/// under the same name does not inherit a verdict it never earned.
fn forget_runs_of(root: &Path, slug: &str) -> usize {
    crate::run_evidence::walk_folders(root)
        .into_iter()
        .map(|dir| {
            crate::run_evidence::forget_walks(&dir, |record| {
                walk_slug(record).as_deref() == Some(slug)
            })
        })
        .sum()
}

/// What the window can see right now, which names the identity a recipe's
/// words stand for: the browser panes `(label, url)` and the apps the helper
/// lists. A Flow is bound to the identities a run's answers carry (an app's
/// bundle id, a page's host), and a recipe often names an app by the name a
/// person types — so the words are read against what is open.
#[derive(Debug, Clone, Default)]
pub struct Seen {
    pub pages: Vec<(String, String)>,
    pub apps: Vec<SeenApp>,
}

/// One app the helper listed.
#[derive(Debug, Clone)]
pub struct SeenApp {
    pub name: String,
    pub bundle_id: Option<String>,
}

impl Seen {
    /// What the window sees: its browser panes, and the helper's `listApps`
    /// answer when it gave one (a helper that did not answer leaves the words
    /// as they were typed).
    #[must_use]
    pub fn of(pages: Vec<(String, String)>, listed_apps: Option<&Value>) -> Self {
        let apps = listed_apps
            .and_then(|listed| listed.get("apps")?.as_array())
            .into_iter()
            .flatten()
            .filter_map(|row| {
                Some(SeenApp {
                    name: row.get("name")?.as_str()?.to_string(),
                    bundle_id: row
                        .get("bundleId")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                })
            })
            .collect();
        Self { pages, apps }
    }
}

/// What registering did: the recipe now a Flow (or one already), and the check
/// it stands on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Registered {
    pub slug: String,
    pub file: String,
    /// The document was a Flow already and is as it was.
    pub already: bool,
    /// The check the Flow stands on, as its line reads. Absent for a Flow that
    /// was one already.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub check: Option<String>,
    /// The check is the card's starting point (the app or page is open), not
    /// words anyone chose: the person is told to rewrite it.
    pub starter: bool,
}

/// Stand a plain recipe up as a Flow: append its `## Flow` and `## Checks`
/// sections — `dry`, full evidence, the fingerprint its steps name (read
/// against what the window `seen`), and one required `state` check — and write
/// the document with one atomic write, its steps untouched. `given` is the
/// words the screen shows when the run has worked (`recipe-save --flow-check`);
/// without them the check is the recipe's own last check step, or else the
/// starting point "the app (or page) is open", which the document and the card
/// tell the person to rewrite. A document that is a Flow already is answered
/// as it is. Fail closed: what would be written must read back as a Flow, or
/// nothing is written.
pub fn flow_register(
    root: &Path,
    slug: &str,
    given: Option<&str>,
    seen: &Seen,
) -> Result<Registered, ComputerUseError> {
    let (file, text) = existing(root, slug)?;
    match parse_flow(&text) {
        Ok(Some(_)) => {
            return Ok(Registered {
                slug: slug.to_string(),
                file: file.display().to_string(),
                already: true,
                check: None,
                starter: false,
            });
        }
        Ok(None) => {}
        Err(why) => {
            return Err(refusal(format!(
                "'{slug}' has Flow sections that do not read ({why}) — fix them in the document"
            )));
        }
    }
    let lines = recipe_lines(&text)
        .map_err(|why| refusal(format!("'{slug}' does not read as a recipe: {why}")))?;
    if lines.is_empty() {
        return Err(refusal(format!("'{slug}' has no steps to stand a Flow on")));
    }
    let fingerprint = fingerprint_of(&lines, seen)?;
    let (tool, argv, starter) = condition_of(&lines, given.map(str::trim))?;
    let shown = recipe_line_text(tool, &argv, &[]);
    let spec = FlowSpec {
        policy: Policy::Dry,
        evidence: EvidenceLevel::Full,
        fingerprint,
        checks: vec![Check {
            id: 1,
            kind: CheckKind::State,
            required: true,
            tool,
            argv,
        }],
        money: None,
        confirm: Confirm::default(),
        trigger: None,
    };
    let mut next = splice_flow_sections(&text, &spec);
    if starter {
        next.push_str(&format!("\n{STARTER_HEADING}\n\n{STARTER_NOTE}\n"));
    }
    parse_flow(&next)
        .map_err(|why| refusal(format!("'{slug}' would not read back as a Flow: {why}")))?
        .ok_or_else(|| refusal(format!("'{slug}' would not read back as a Flow")))?;
    crate::update_store::write_then_rename(&file, next.as_bytes()).map_err(|error| {
        ComputerUseError::new(
            error_code::ACCESSIBILITY_ERROR,
            format!("could not write {}: {error}", file.display()),
        )
    })?;
    Ok(Registered {
        slug: slug.to_string(),
        file: file.display().to_string(),
        already: false,
        check: Some(shown),
        starter,
    })
}

/// The value of `flag` among a line's words, unless it is a placeholder or
/// another flag.
fn flag_value<'a>(argv: &'a [String], flag: &str) -> Option<&'a str> {
    let at = argv.iter().position(|word| word == flag)?;
    let value = argv.get(at + 1)?.as_str();
    (!value.starts_with("--") && recipe_placeholders(value).is_empty()).then_some(value)
}

/// The identities an `--app` word stands for: the word as typed, and — when the
/// helper lists an app it names (by name or by bundle id) — the identity a run's
/// answers will carry for it, its bundle id.
fn app_identities(app: &str, seen: &Seen) -> BTreeSet<String> {
    let mut identities = BTreeSet::from([app.to_string()]);
    let wanted = app.trim().to_lowercase();
    let listed = seen.apps.iter().find(|row| {
        row.name.to_lowercase() == wanted
            || row
                .bundle_id
                .as_deref()
                .is_some_and(|id| id.to_lowercase() == wanted)
    });
    if let Some(row) = listed {
        identities.insert(row.bundle_id.clone().unwrap_or_else(|| row.name.clone()));
    }
    identities
}

/// The host a browser step's page comes from: the URL a `goto` or an `open`
/// names, or the host of the pane a step on a labelled tab aims at, read from
/// what is open now. A page with no host (a local file) and a tab an alias
/// stands for have none to give.
fn browser_host(line: &RecipeLine, seen: &Seen) -> Option<String> {
    let argv = &line.argv;
    let host = match argv.first()?.as_str() {
        "goto" => page_host(argv.get(2)?),
        "open" => page_host(argv.get(1)?),
        _ => {
            let label = browser_tab_target(argv)?;
            seen.pages
                .iter()
                .find(|(shown, _)| shown == label)
                .and_then(|(_, url)| page_host(url))
        }
    };
    host.filter(|host| !host.is_empty())
}

/// The fingerprint a recipe's own words name: every `--app` its desktop and
/// phone steps act in and every host its browser steps open or sit on, on this
/// build's protocol. A recipe that names neither has nothing to bind a Flow to —
/// a Flow acts only in the apps and on the sites it was recorded with — and is
/// refused, saying how to give it one.
fn fingerprint_of(lines: &[RecipeLine], seen: &Seen) -> Result<Fingerprint, ComputerUseError> {
    let mut print = Fingerprint {
        protocol: COMPUTER_USE_PROTOCOL_VERSION,
        ..Fingerprint::default()
    };
    for line in lines {
        match line.tool {
            RecipeTool::Computer | RecipeTool::Emulator => {
                if let Some(app) =
                    flag_value(&line.argv, "--app").filter(|app| !app.starts_with("pid:"))
                {
                    print.apps.extend(app_identities(app, seen));
                }
            }
            RecipeTool::Browser => print.hosts.extend(browser_host(line, seen)),
        }
    }
    if print.apps.is_empty() && print.hosts.is_empty() {
        return Err(refusal(
            "no step names an app or a website, so there is nothing to bind a Flow to — name the app with --app in a step, or open the page in a browser tab and register again",
        ));
    }
    Ok(print)
}

/// Where the steps ended, and so what a check may look at.
enum Aim {
    App(String),
    Tab(String),
    Phone { platform: String, device: String },
}

/// What the recipe's last aimed step aims at: an app, a browser tab or a phone.
fn aim_of(lines: &[RecipeLine]) -> Option<Aim> {
    lines.iter().rev().find_map(|line| match line.tool {
        RecipeTool::Computer => flag_value(&line.argv, "--app")
            .filter(|app| !app.starts_with("pid:"))
            .map(|app| Aim::App(app.to_string())),
        RecipeTool::Browser => browser_tab_target(&line.argv)
            .map(str::to_string)
            .or_else(|| line.opens_tab.clone())
            .map(Aim::Tab),
        RecipeTool::Emulator => Some(Aim::Phone {
            platform: flag_value(&line.argv, "--platform")?.to_string(),
            device: flag_value(&line.argv, "--device")?.to_string(),
        }),
    })
}

impl Aim {
    fn tool(&self) -> RecipeTool {
        match self {
            Self::App(_) => RecipeTool::Computer,
            Self::Tab(_) => RecipeTool::Browser,
            Self::Phone { .. } => RecipeTool::Emulator,
        }
    }

    /// The check "this text is on the screen" there.
    fn shows(&self, text: &str) -> Vec<String> {
        let words: Vec<&str> = match self {
            Self::App(app) => vec!["wait-for", "--app", app.as_str(), "--text", text],
            Self::Tab(tab) => vec!["find", tab.as_str(), text],
            Self::Phone { platform, device } => vec![
                "find",
                "--platform",
                platform.as_str(),
                "--device",
                device.as_str(),
                "--text",
                text,
            ],
        };
        words.into_iter().map(str::to_string).collect()
    }

    /// The starting point "it is open": all a recipe's words can say about
    /// where it ended. A phone has none to give.
    fn is_open(&self) -> Option<Vec<String>> {
        let words: Vec<&str> = match self {
            Self::App(app) => vec!["wait-for", "--app", app.as_str(), "--role", WINDOW_ROLE],
            Self::Tab(tab) => vec!["wait", tab.as_str(), PAGE_SELECTOR, PAGE_WAIT_MS],
            Self::Phone { .. } => return None,
        };
        Some(words.into_iter().map(str::to_string).collect())
    }
}

/// Whether a step line is a check the walk can judge, with no placeholder a
/// walk would have to be given.
fn is_own_check(line: &RecipeLine) -> bool {
    let verb = line.argv.first().map_or("", String::as_str);
    let check = match line.tool {
        RecipeTool::Computer => {
            verb_method(verb).is_some_and(|method| RECIPE_CHECKS.contains(&method))
        }
        RecipeTool::Browser => zerocode_core::agent_browser::is_check(verb),
        RecipeTool::Emulator => zerocode_core::agent_emulator::is_check(verb),
    };
    check
        && line
            .argv
            .iter()
            .all(|word| recipe_placeholders(word).is_empty())
}

/// The check a registered Flow stands on, as `(door, words, starter)`: the
/// words the screen shows (`given`) looked for where the steps ended; else the
/// recipe's own last check step, which its author already used to see it work;
/// else the starting point — the app or page is open — which says so
/// (`starter`) and asks to be rewritten.
fn condition_of(
    lines: &[RecipeLine],
    given: Option<&str>,
) -> Result<(RecipeTool, Vec<String>, bool), ComputerUseError> {
    let aim = aim_of(lines);
    if let Some(text) = given {
        if text.is_empty() {
            return Err(refusal(
                "the success condition is empty — say what the screen shows when the run has worked",
            ));
        }
        let aim = aim.ok_or_else(|| {
            refusal("no step names an app, a browser tab or a phone to look for the condition in")
        })?;
        return Ok((aim.tool(), aim.shows(text), false));
    }
    if let Some(line) = lines.iter().rev().find(|line| is_own_check(line)) {
        return Ok((line.tool, line.argv.clone(), false));
    }
    let started = aim
        .as_ref()
        .and_then(|aim| aim.is_open().map(|argv| (aim.tool(), argv)));
    started
        .map(|(tool, argv)| (tool, argv, true))
        .ok_or_else(|| {
            refusal(
                "the recipe ends in no app or browser tab a starting condition can look at — register it with the words the screen shows when it has worked (recipe-save --flow-check), or write the Checks section in the document",
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::computer_use::evidence::SESSIONS_DIR;
    use crate::computer_use::recipes::{FlowJob, FlowTrigger, RECIPES_DIR, flow_list};
    use zerocode_core::computer_flow::{FLOW_HEADING, FLOW_HEADING_CHECKS};
    use zerocode_core::computer_recipe::RECIPE_HEADING_STEPS;

    /// A recipes folder in a temp root; the real store is never named.
    fn folder() -> (tempfile::TempDir, PathBuf) {
        let root = tempfile::tempdir().expect("tempdir");
        let recipes = root.path().join(RECIPES_DIR);
        std::fs::create_dir_all(&recipes).expect("recipes folder");
        (root, recipes)
    }

    fn plain(title: &str, steps: &[&str]) -> String {
        let steps: String = steps
            .iter()
            .enumerate()
            .map(|(at, step)| format!("{}. `{step}`\n", at + 1))
            .collect();
        format!("# {title}\n\n{RECIPE_HEADING_STEPS}\n\n{steps}")
    }

    fn flow_doc(title: &str) -> String {
        format!(
            "{}\n{FLOW_HEADING}\n\n- policy: dry\n- evidence: full\n- fingerprint: apps=com.example.app protocol={COMPUTER_USE_PROTOCOL_VERSION}\n\n{FLOW_HEADING_CHECKS}\n\n1. `zerocode-computer wait-for --app com.example.app --text done` — state, required\n",
            plain(title, &["zerocode-computer launch --app com.example.app"])
        )
    }

    fn seen_safari() -> Seen {
        Seen::of(
            vec![("browser-1".into(), "https://admin.example.com/cards".into())],
            Some(&serde_json::json!({ "apps": [
                { "name": "Safari", "bundleId": "com.apple.Safari", "pid": 7 },
                { "name": "Calculator", "bundleId": "com.apple.calculator", "pid": 8 }
            ]})),
        )
    }

    /// A walk record the way a recipe-run leaves it, with a verdict beside it.
    fn walked(root: &Path, session: &str, recipe: &str, at: i64) -> PathBuf {
        walked_in(root.join(SESSIONS_DIR).join(session), recipe, at)
    }

    /// The same record in the folder it is given — a scheduled run's, say.
    fn walked_in(dir: PathBuf, recipe: &str, at: i64) -> PathBuf {
        std::fs::create_dir_all(&dir).unwrap();
        crate::run_evidence::record_walk(
            &dir,
            &serde_json::json!({ "kind": "recipe-run", "name": recipe, "at_epoch_ms": at }),
        )
        .unwrap();
        crate::computer_use::evidence::write_verdict(
            &dir,
            &["verdict".to_string(), "--pass".to_string()],
            true,
            None,
            at,
        )
        .unwrap();
        dir
    }

    /// Deleting a Flow takes its document and the walk records that name its
    /// slug — so the roster no longer shows it and a Flow made again under the
    /// name inherits no verdict — and nothing else: another recipe's document
    /// and walks, and the sessions' own steps and verdicts, stay.
    #[test]
    fn deleting_a_flow_takes_its_document_and_its_last_verdict_and_nothing_else() {
        let (root, recipes) = folder();
        std::fs::write(recipes.join("wallet-transfer.md"), flow_doc("Wallet")).unwrap();
        std::fs::write(recipes.join("other.md"), flow_doc("Other")).unwrap();
        let both = walked(root.path(), "20260914-020000-1", "Wallet transfer", 5_000);
        crate::run_evidence::record_walk(
            &both,
            &serde_json::json!({ "kind": "recipe-run", "name": "other", "at_epoch_ms": 5_100 }),
        )
        .unwrap();
        let only_other = walked(root.path(), "20260914-030000-1", "other", 6_000);

        let before = flow_list(root.path(), |_| None);
        assert_eq!(before.flows.len(), 2);
        assert_eq!(
            before
                .flows
                .iter()
                .find(|flow| flow.slug == "wallet-transfer")
                .and_then(|flow| flow.last.as_ref())
                .map(|last| last.at),
            Some(5_000)
        );

        let deleted = flow_delete(root.path(), "wallet-transfer").expect("deleted");
        assert!(!recipes.join("wallet-transfer.md").exists(), "the document");
        assert_eq!(deleted.walks, 1, "the one walk that named the slug");
        assert_eq!(deleted.ledger, None);

        let after = flow_list(root.path(), |_| None);
        assert_eq!(
            after
                .flows
                .iter()
                .map(|flow| flow.slug.as_str())
                .collect::<Vec<_>>(),
            ["other"],
            "the roster no longer shows it, and the other stays"
        );
        assert_eq!(
            after.flows[0].last.as_ref().map(|last| last.at),
            Some(6_000),
            "the other recipe's walks, in the shared session too, are untouched"
        );
        assert_eq!(crate::run_evidence::walks_in(&both).len(), 1);
        assert_eq!(crate::run_evidence::walks_in(&only_other).len(), 1);
        assert!(
            crate::computer_use::evidence::read_verdict(&both).is_some(),
            "the session's own verdict stays — it is the session's, not the recipe's"
        );

        // Made again under the same name, it has run nowhere.
        std::fs::write(recipes.join("wallet-transfer.md"), flow_doc("Wallet")).unwrap();
        let again = flow_list(root.path(), |_| None);
        let reborn = again
            .flows
            .iter()
            .find(|flow| flow.slug == "wallet-transfer");
        assert!(
            reborn.is_some_and(|flow| flow.last.is_none()),
            "a Flow made again must not inherit the last verdict of the one deleted"
        );
    }

    /// Deleting names a recipe by slug alone. Anything that is not its own slug
    /// — a path out of the folder, an absolute path, a separator, a name with
    /// capitals — is refused before any file is touched, and a slug that names
    /// no document is refused by name.
    #[test]
    fn deleting_refuses_a_path_a_stranger_and_a_missing_recipe() {
        let (root, recipes) = folder();
        std::fs::write(
            recipes.join("keep.md"),
            plain("Keep", &["zerocode-computer key --key tab"]),
        )
        .unwrap();
        let outside = root.path().join("outside.md");
        std::fs::write(&outside, "not a recipe").unwrap();
        std::fs::write(root.path().join("computer-use").join("sibling.md"), "x").unwrap();

        let dangerous = [
            "../../outside",
            "../outside",
            "..",
            ".",
            "",
            "/etc/hosts",
            "keep/../keep",
            "sub/keep",
            "sub\\keep",
            "Keep",
            "keep.md",
            "keep ",
            "../sibling",
        ];
        for word in dangerous {
            let refused = flow_delete(root.path(), word).expect_err(word);
            assert_eq!(refused.code, error_code::INVALID_ARGUMENT, "{word:?}");
        }
        assert!(
            recipes.join("keep.md").exists(),
            "a refusal touches nothing"
        );
        assert!(outside.exists());
        assert!(root.path().join("computer-use").join("sibling.md").exists());

        let missing = flow_delete(root.path(), "nothing-here").expect_err("no such recipe");
        assert!(missing.message.contains("nothing-here"), "{missing:?}");
        // A plain recipe is deletable too: the card's second list reaches it.
        flow_delete(root.path(), "keep").expect("a plain recipe goes the same way");
        assert!(!recipes.join("keep.md").exists());
    }

    /// A money Flow's ledger is the record a transaction was acted on, and what
    /// stops a later recipe of the same name from pressing it again: deleting
    /// the Flow leaves it, and says so.
    #[test]
    fn deleting_a_money_flow_leaves_its_ledger_and_says_so() {
        let (root, recipes) = folder();
        std::fs::write(recipes.join("payout.md"), flow_doc("Payout")).unwrap();
        let ledger = recipes.join("payout.ledger.jsonl");
        std::fs::write(&ledger, "{\"txn\":\"t-1\"}\n").unwrap();
        let deleted = flow_delete(root.path(), "payout").expect("deleted");
        assert_eq!(
            deleted.ledger.as_deref(),
            Some(ledger.display().to_string().as_str())
        );
        assert!(ledger.exists(), "the ledger is not this card's to remove");
        assert!(!recipes.join("payout.md").exists());
    }

    /// The card lists the recipes that are not Flows apart from the Flows, each
    /// with its steps — and a document whose sections do not read is listed
    /// there with the reason, never lost.
    #[test]
    fn the_roster_keeps_the_recipes_that_are_not_flows_in_a_second_list() {
        let (root, recipes) = folder();
        std::fs::write(recipes.join("a-flow.md"), flow_doc("A flow")).unwrap();
        std::fs::write(
            recipes.join("open-mail.md"),
            plain(
                "메일 열기",
                &[
                    "zerocode-computer key --key tab",
                    "zerocode-computer key --key return",
                ],
            ),
        )
        .unwrap();
        std::fs::write(
            recipes.join("broken.md"),
            format!(
                "{}\n{FLOW_HEADING}\n\n- policy: wet\n",
                plain("Broken", &["zerocode-computer key --key a"])
            ),
        )
        .unwrap();
        std::fs::write(recipes.join("empty.md"), "# Empty\n").unwrap();

        let listing = flow_list(root.path(), |_| None);
        assert_eq!(
            listing
                .flows
                .iter()
                .map(|flow| flow.slug.as_str())
                .collect::<Vec<_>>(),
            ["a-flow"]
        );
        let row = |slug: &str| {
            listing
                .recipes
                .iter()
                .find(|row| row.slug == slug)
                .unwrap_or_else(|| {
                    panic!("{slug} must be on the second list: {:?}", listing.recipes)
                })
        };
        assert_eq!(
            (row("open-mail").name.as_str(), row("open-mail").steps),
            ("메일 열기", 2)
        );
        assert_eq!(row("open-mail").problem, None);
        assert!(
            row("broken")
                .problem
                .as_deref()
                .is_some_and(|why| why.contains("Flow") || why.contains("policy")),
            "a Flow section that does not read says why: {:?}",
            row("broken")
        );
        assert!(row("empty").problem.is_some(), "no steps is said");
        assert_eq!(listing.recipes.len(), 3, "{:?}", listing.recipes);
    }

    /// A recipe of the second list is deleted with the automations that walk it
    /// and the verdict of its last walk (the same command takes both), so its row
    /// says them as a Flow's row does — the question the person answers names what
    /// goes — and says nothing when there is nothing.
    #[test]
    fn a_recipe_on_the_second_list_names_the_automations_and_the_verdict_deleting_it_takes() {
        let (root, recipes) = folder();
        for slug in ["nightly-probe", "quiet"] {
            std::fs::write(
                recipes.join(format!("{slug}.md")),
                plain(slug, &["zerocode-computer key --key tab"]),
            )
            .unwrap();
        }
        walked(root.path(), "20260914-020000-1", "Nightly probe", 5_000);

        let listing = flow_list(root.path(), |slug| {
            (slug == "nightly-probe").then(|| FlowTrigger {
                cron: Some("0 3 * * *".to_string()),
                automations: vec!["Night job".to_string()],
                jobs: vec![FlowJob {
                    id: "night".to_string(),
                    name: "Night job".to_string(),
                    enabled: false,
                }],
            })
        });
        let row = |slug: &str| {
            listing
                .recipes
                .iter()
                .find(|row| row.slug == slug)
                .unwrap_or_else(|| panic!("{slug} must be on the second list"))
        };
        assert_eq!(row("nightly-probe").automations, ["Night job"]);
        assert_eq!(
            row("nightly-probe")
                .schedules
                .iter()
                .map(|job| (job.id.as_str(), job.enabled))
                .collect::<Vec<_>>(),
            [("night", false)],
            "a paused job is still listed under the recipe it runs"
        );
        assert_eq!(
            row("nightly-probe").last.as_ref().map(|last| last.at),
            Some(5_000),
            "the walk the delete would forget"
        );
        assert!(row("quiet").automations.is_empty() && row("quiet").last.is_none());

        // What the page reads: a key with nothing to say is not there.
        let json = serde_json::to_value(&listing).unwrap();
        let quiet = json["recipes"]
            .as_array()
            .and_then(|rows| rows.iter().find(|row| row["slug"] == "quiet"))
            .expect("the quiet recipe is listed");
        assert!(quiet.get("automations").is_none() && quiet.get("last").is_none());
    }

    /// A walk a scheduled run made is a walk like any other: its folder under
    /// `automations/<job>/runs/` gives the recipe its last verdict when it is the
    /// newest, and deleting the recipe forgets that record with the sessions'.
    #[test]
    fn a_scheduled_runs_folder_gives_the_recipe_its_last_verdict_and_is_forgotten_with_it() {
        let (root, recipes) = folder();
        std::fs::write(
            recipes.join("nightly-probe.md"),
            plain("Nightly probe", &["zerocode-computer key --key tab"]),
        )
        .unwrap();
        walked(root.path(), "20260914-020000-1", "Nightly probe", 5_000);
        let run = walked_in(
            root.path()
                .join(crate::run_evidence::AUTOMATIONS_DIR)
                .join("job-1")
                .join(crate::run_evidence::AUTOMATION_RUNS_DIR)
                .join("9000"),
            "Nightly probe",
            9_000,
        );

        let listing = flow_list(root.path(), |_| None);
        let last = listing.recipes[0]
            .last
            .as_ref()
            .expect("the recipe has been walked");
        assert_eq!(last.at, 9_000, "the scheduled run is the newer walk");
        assert_eq!(last.dir, run.display().to_string());
        assert_eq!(last.pass, Some(true));

        let deleted = flow_delete(root.path(), "nightly-probe").expect("deleted");
        assert_eq!(deleted.walks, 2, "the session's record and the run's");
        assert!(
            crate::run_evidence::walks_in(&run).is_empty(),
            "the run's folder still names the recipe"
        );
    }

    /// Registering appends the two Flow sections to a plain recipe, leaves its
    /// steps word for word, and the recipe then stands on the roster — `dry`,
    /// full evidence, the identities its steps name read against what is open.
    #[test]
    fn registering_a_plain_recipe_stands_it_up_as_a_flow_on_the_roster() {
        let (root, recipes) = folder();
        let text = plain(
            "Open Safari",
            &[
                "zerocode-computer launch --app Safari",
                "zerocode-computer click --app Safari --role button --text Go",
                "zerocode-computer wait-for --app Safari --text 완료",
            ],
        );
        std::fs::write(recipes.join("open-safari.md"), &text).unwrap();
        let steps_before = recipe_lines(&text).unwrap();

        let done =
            flow_register(root.path(), "open-safari", None, &seen_safari()).expect("registered");
        assert!(!done.already);
        assert!(
            !done.starter,
            "the recipe's own check step is a condition its author used"
        );
        assert!(
            done.check
                .as_deref()
                .is_some_and(|check| check.contains("wait-for") && check.contains("완료")),
            "{done:?}"
        );

        let after = std::fs::read_to_string(recipes.join("open-safari.md")).unwrap();
        assert!(
            after.starts_with(text.trim_end()),
            "the steps stay word for word"
        );
        let steps_after = recipe_lines(&after).unwrap();
        assert_eq!(
            steps_after
                .iter()
                .map(|line| &line.argv)
                .collect::<Vec<_>>(),
            steps_before
                .iter()
                .map(|line| &line.argv)
                .collect::<Vec<_>>()
        );
        let spec = parse_flow(&after).expect("reads").expect("is a Flow");
        assert_eq!(
            (spec.policy, spec.evidence),
            (Policy::Dry, EvidenceLevel::Full)
        );
        assert_eq!(
            spec.fingerprint
                .apps
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["Safari", "com.apple.Safari"],
            "the name as typed, and the identity a run's answers carry for it"
        );
        assert_eq!(spec.fingerprint.protocol, COMPUTER_USE_PROTOCOL_VERSION);
        assert_eq!((spec.checks.len(), spec.checks[0].required), (1, true));

        let listing = flow_list(root.path(), |_| None);
        assert_eq!(listing.flows.len(), 1, "it stands on the roster now");
        assert!(listing.recipes.is_empty(), "and leaves the second list");
        assert_eq!(listing.flows[0].slug, "open-safari");
    }

    /// A recipe with no check step of its own gets the starting point — the app
    /// is open — and the document says to rewrite it; the note survives a policy
    /// change through `flow_set`.
    #[test]
    fn a_recipe_without_a_check_gets_a_starting_condition_that_asks_to_be_rewritten() {
        let (root, recipes) = folder();
        std::fs::write(
            recipes.join("calc.md"),
            plain("Calc", &["zerocode-computer launch --app Calculator"]),
        )
        .unwrap();
        let done = flow_register(root.path(), "calc", None, &seen_safari()).expect("registered");
        assert!(done.starter);
        let after = std::fs::read_to_string(recipes.join("calc.md")).unwrap();
        let spec = parse_flow(&after).unwrap().unwrap();
        assert_eq!(
            spec.checks[0].argv,
            ["wait-for", "--app", "Calculator", "--role", "AXWindow"]
        );
        assert!(
            after.contains(STARTER_HEADING) && after.contains("Replace it with"),
            "{after}"
        );
        assert_eq!(
            spec.fingerprint
                .apps
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["Calculator", "com.apple.calculator"]
        );

        super::super::flow_set(root.path(), "calc", None, Some("verdict-only")).expect("set");
        let kept = std::fs::read_to_string(recipes.join("calc.md")).unwrap();
        assert!(
            kept.contains(STARTER_HEADING),
            "the note stays when the card rewrites its sections"
        );
    }

    /// Registering a Flow that already is one changes nothing — the bytes, not
    /// just the meaning — and says so.
    #[test]
    fn registering_a_flow_again_leaves_the_document_exactly_as_it_was() {
        let (root, recipes) = folder();
        let doc = flow_doc("Already");
        std::fs::write(recipes.join("already.md"), &doc).unwrap();
        let done = flow_register(
            root.path(),
            "already",
            Some("something else"),
            &Seen::default(),
        )
        .expect("answered");
        assert!(done.already && done.check.is_none() && !done.starter);
        assert_eq!(
            std::fs::read_to_string(recipes.join("already.md")).unwrap(),
            doc
        );
    }

    /// `recipe-save --flow-check`: the words the screen shows become the
    /// required check, looked for where the steps ended — the app of a desktop
    /// recipe, the tab of a browser one.
    #[test]
    fn the_given_words_become_the_required_check_where_the_steps_ended() {
        let (root, recipes) = folder();
        std::fs::write(
            recipes.join("desk.md"),
            plain(
                "Desk",
                &["zerocode-computer click --app com.example.notes --role button --text Save"],
            ),
        )
        .unwrap();
        std::fs::write(
            recipes.join("web.md"),
            plain(
                "Web",
                &[
                    "zerocode-browser goto browser-1 https://admin.example.com/cards",
                    "zerocode-browser click browser-1 #pay",
                ],
            ),
        )
        .unwrap();
        let desk =
            flow_register(root.path(), "desk", Some("  Saved  "), &Seen::default()).expect("desk");
        assert!(
            !desk.starter,
            "words someone chose are not a starting point"
        );
        let spec = parse_flow(&std::fs::read_to_string(recipes.join("desk.md")).unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(spec.checks[0].tool, RecipeTool::Computer);
        assert_eq!(
            spec.checks[0].argv,
            ["wait-for", "--app", "com.example.notes", "--text", "Saved"]
        );
        assert_eq!(spec.checks[0].kind, CheckKind::State);

        flow_register(root.path(), "web", Some("Order placed"), &Seen::default()).expect("web");
        let spec = parse_flow(&std::fs::read_to_string(recipes.join("web.md")).unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(spec.checks[0].tool, RecipeTool::Browser);
        assert_eq!(spec.checks[0].argv, ["find", "browser-1", "Order placed"]);
        assert_eq!(
            spec.fingerprint
                .hosts
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["admin.example.com"],
            "a goto's host is the page the Flow is bound to"
        );
        assert!(spec.fingerprint.apps.is_empty());
    }

    /// A browser step on an open tab reads that tab's page host from what the
    /// window shows now; a recipe that names no app or site, a phone with no
    /// words, an empty condition and a Flow section that does not read are each
    /// refused by name, and the document is left as it was.
    #[test]
    fn registering_refuses_what_it_cannot_bind_and_leaves_the_document_alone() {
        let (root, recipes) = folder();
        let on_a_tab = plain("Tab", &["zerocode-browser click browser-1 #go"]);
        std::fs::write(recipes.join("tab.md"), &on_a_tab).unwrap();
        flow_register(root.path(), "tab", None, &seen_safari()).expect("the tab's page is open");
        let spec = parse_flow(&std::fs::read_to_string(recipes.join("tab.md")).unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(
            spec.fingerprint
                .hosts
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["admin.example.com"]
        );
        assert_eq!(spec.checks[0].argv, ["wait", "browser-1", "body", "3000"]);

        let untouched = |name: &str, text: &str| {
            std::fs::write(recipes.join(format!("{name}.md")), text).unwrap();
            text.to_string()
        };
        let keyless = untouched("keys", &plain("Keys", &["zerocode-computer key --key tab"]));
        let local = untouched(
            "local",
            &plain(
                "Local",
                &["zerocode-browser goto browser-1 file:///Users/dev/page.html"],
            ),
        );
        let phone = untouched(
            "phone",
            &plain(
                "Phone",
                &["zerocode-emulator launch --platform ios --device phone --app com.example.app"],
            ),
        );
        let named_app = untouched(
            "app",
            &plain("App", &["zerocode-computer launch --app com.example.app"]),
        );
        let broken = untouched(
            "broken",
            &format!(
                "{}\n{FLOW_HEADING}\n\n- policy: wet\n",
                plain("Broken", &["zerocode-computer key --key a"])
            ),
        );

        let refusals = [
            ("keys", None, "names an app or a website"),
            ("local", None, "names an app or a website"),
            ("phone", None, "starting condition"),
            ("app", Some("   "), "empty"),
            ("broken", None, "do not read"),
            ("nothing-here", None, "no recipe named"),
            ("../keys", None, "not a recipe slug"),
        ];
        for (slug, words, said) in refusals {
            let refused =
                flow_register(root.path(), slug, words, &Seen::default()).expect_err(slug);
            assert!(refused.message.contains(said), "{slug}: {refused:?}");
        }
        for (name, text) in [
            ("keys", keyless),
            ("local", local),
            ("phone", phone),
            ("app", named_app),
            ("broken", broken),
        ] {
            assert_eq!(
                std::fs::read_to_string(recipes.join(format!("{name}.md"))).unwrap(),
                text,
                "{name} was written"
            );
        }
        // A phone is given words, and looks for them on its own screen.
        flow_register(root.path(), "phone", Some("Welcome"), &Seen::default())
            .expect("a phone with words");
        let spec = parse_flow(&std::fs::read_to_string(recipes.join("phone.md")).unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(spec.checks[0].tool, RecipeTool::Emulator);
    }

    /// The identity of an app a recipe types by name is read against what the
    /// helper lists, by name or by bundle id, in any case; an unlisted app, a
    /// placeholder and a `pid:` stay as typed or are left out.
    #[test]
    fn an_apps_name_is_read_against_what_is_open_and_stays_as_typed_when_nothing_answers() {
        let seen = seen_safari();
        assert_eq!(
            app_identities("safari", &seen)
                .into_iter()
                .collect::<Vec<_>>(),
            ["com.apple.Safari", "safari"]
        );
        assert_eq!(
            app_identities("COM.APPLE.CALCULATOR", &seen)
                .into_iter()
                .collect::<Vec<_>>(),
            ["COM.APPLE.CALCULATOR", "com.apple.calculator"]
        );
        assert_eq!(
            app_identities("Notes", &seen)
                .into_iter()
                .collect::<Vec<_>>(),
            ["Notes"],
            "an app nothing lists is bound by the word typed"
        );
        let argv = |words: &[&str]| {
            words
                .iter()
                .map(|word| (*word).to_string())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            flag_value(&argv(&["click", "--app", "{{app}}"]), "--app"),
            None
        );
        assert_eq!(
            flag_value(&argv(&["click", "--app", "--text"]), "--app"),
            None
        );
        assert_eq!(
            flag_value(&argv(&["click", "--app", "Mail"]), "--app"),
            Some("Mail")
        );
        assert!(
            Seen::of(Vec::new(), None).apps.is_empty(),
            "a helper that did not answer"
        );
    }
}
