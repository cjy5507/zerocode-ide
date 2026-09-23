//! `zerocode vault-lint` — the second brain's lint table, on a terminal.
//!
//! The table is [`zerocode_core::second_brain_lint::VaultLint`], produced by
//! the same scan the window's knowledge graph and vault health card read. This
//! command prints it and exits with the finding count folded to a status, so
//! `just vault-lint` can sit in a hook, a cron or a CI leg: 0 is a clean
//! vault, 1 is a vault holding a finding, 2 is "no vault to lint".
//!
//! Where the vault is comes from three places, in order, none of them written
//! here: `--vault <dir>`, the pane variable every ZeroCode terminal carries
//! ([`zerocode_core::second_brain::VAULT_ENV`]), and the vault the window
//! saved in its settings document (one key read out of
//! [`zerocode_lane::PREFERENCES_FILE`]).

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use zerocode_core::second_brain::{VAULT_ENV, WIKI_DIR};
use zerocode_core::second_brain_graph::{GraphCache, VaultGraph};
use zerocode_core::second_brain_lint::VaultLint;
use zerocode_lane::{AppPaths, PREFERENCES_FILE, PathClass};

pub const USAGE: &str = "\
zerocode vault-lint [--vault <dir>] [--json]

    Lint the second-brain vault: pages the index does not list, links to
    pages that do not exist, orphans, pages without source/ingested_at,
    prose links no relation key declares, raw items nothing ingested,
    relations whose provenance no road vouches for.
    The vault is --vault, else $ZEROCODE_SECOND_BRAIN, else the one the
    ZeroCode window saved. Exit 0 clean, 1 with findings, 2 with no vault.
";

/// The settings key the window writes the saved vault under
/// (`crates/zerocode-shell/src/settings_runtime.rs`, `setting_key::SECOND_BRAIN_VAULT`)
/// inside the document's `data` envelope (`zerocode-shell-state`'s writer).
const SAVED_VAULT_KEY: &str = "second_brain_vault";
const DOCUMENT_DATA_KEY: &str = "data";

const EXIT_FINDINGS: u8 = 1;
const EXIT_NO_VAULT: u8 = 2;

#[derive(Debug, Default, PartialEq, Eq)]
struct Options {
    vault: Option<PathBuf>,
    json: bool,
}

fn parse(args: &[String]) -> Result<Options, String> {
    let mut options = Options::default();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--vault" => {
                let value = iter.next().ok_or("--vault needs a directory")?;
                options.vault = Some(PathBuf::from(value));
            }
            "--json" => options.json = true,
            "-h" | "--help" => return Err(USAGE.to_string()),
            other => return Err(format!("unknown argument `{other}`\n\n{USAGE}")),
        }
    }
    Ok(options)
}

/// Where the vault is, and which of the three roads named it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Named {
    Argument(PathBuf),
    Environment(PathBuf),
    Settings(PathBuf),
}

impl Named {
    fn path(&self) -> &Path {
        match self {
            Self::Argument(path) | Self::Environment(path) | Self::Settings(path) => path,
        }
    }

    const fn road(&self) -> &'static str {
        match self {
            Self::Argument(_) => "--vault",
            Self::Environment(_) => VAULT_ENV,
            Self::Settings(_) => "window settings",
        }
    }
}

/// The three roads, in order. `env` and `settings` are parameters so a test
/// can hold both without touching the process environment or the machine's
/// settings document.
pub fn resolve(
    argument: Option<PathBuf>,
    env: Option<String>,
    settings: impl FnOnce() -> Option<PathBuf>,
) -> Option<Named> {
    if let Some(path) = argument {
        return Some(Named::Argument(path));
    }
    if let Some(path) = env
        .map(|held| held.trim().to_string())
        .filter(|held| !held.is_empty())
    {
        return Some(Named::Environment(PathBuf::from(path)));
    }
    settings().map(Named::Settings)
}

/// The vault the window saved: one key out of its settings document. Any
/// failure — no document, no key, a blank value — is "nothing saved".
#[must_use]
pub fn saved_vault_in(document: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(document).ok()?;
    let root: serde_json::Value = serde_json::from_str(&text).ok()?;
    let path = root
        .get(DOCUMENT_DATA_KEY)?
        .get(SAVED_VAULT_KEY)?
        .as_str()?
        .trim();
    (!path.is_empty()).then(|| PathBuf::from(path))
}

fn saved_vault() -> Option<PathBuf> {
    let paths = AppPaths::from_platform().ok()?;
    saved_vault_in(&paths.active_root(PathClass::Config).join(PREFERENCES_FILE))
}

pub fn run(args: &[String]) -> ExitCode {
    let options = match parse(args) {
        Ok(options) => options,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::from(EXIT_NO_VAULT);
        }
    };
    let Some(named) = resolve(options.vault, std::env::var(VAULT_ENV).ok(), saved_vault) else {
        eprintln!(
            "zerocode: no vault to lint — pass --vault <dir>, export {VAULT_ENV}, or set up the second brain in the window"
        );
        return ExitCode::from(EXIT_NO_VAULT);
    };
    let root = named.path();
    if !root.join(WIKI_DIR).is_dir() {
        eprintln!(
            "zerocode: {} ({}) has no {WIKI_DIR}/ — not a second-brain vault",
            root.display(),
            named.road()
        );
        return ExitCode::from(EXIT_NO_VAULT);
    }
    let began = Instant::now();
    let graph = GraphCache::new().scan(root, false);
    let scanned_ms = began.elapsed().as_millis();
    if options.json {
        match serde_json::to_string_pretty(&graph.lint) {
            Ok(text) => println!("{text}"),
            Err(error) => {
                eprintln!("zerocode: {error}");
                return ExitCode::from(EXIT_NO_VAULT);
            }
        }
    } else {
        print!("{}", render(root, named.road(), &graph, scanned_ms));
    }
    if graph.lint.is_clean() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(EXIT_FINDINGS)
    }
}

/// One row per finding kind, its count, a sentence, and the ids under it.
/// The row keys are the table's own field names, so the text and `--json`
/// name the same things.
#[must_use]
pub fn render(root: &Path, road: &str, graph: &VaultGraph, scanned_ms: u128) -> String {
    let lint: &VaultLint = &graph.lint;
    let mut out = String::new();
    out.push_str(&format!("vault: {} ({road})\n", root.display()));
    out.push_str(&format!(
        "pages {} · links {} · ghosts {} · scanned in {scanned_ms} ms{}\n\n",
        graph.pages,
        graph.edges.len(),
        graph.ghosts,
        if graph.capped { " · CAPPED" } else { "" }
    ));
    let row = |out: &mut String, key: &str, count: String, said: &str| {
        out.push_str(&format!("{key:<22}{count:>5}   {said}\n"));
    };
    let under = |out: &mut String, line: String| out.push_str(&format!("    {line}\n"));

    row(
        &mut out,
        "index_gaps",
        lint.counts.index_gaps.to_string(),
        "pages not in wiki/index.md",
    );
    for page in &lint.index_gaps {
        under(&mut out, page.clone());
    }
    row(
        &mut out,
        "ghost_links",
        lint.counts.ghost_links.to_string(),
        "link targets without a page",
    );
    for ghost in &lint.ghost_links {
        under(
            &mut out,
            format!("[[{}]] ← {}", ghost.target, ghost.from.join(", ")),
        );
    }
    row(
        &mut out,
        "orphans",
        lint.counts.orphans.to_string(),
        "pages no page links to and the index does not list",
    );
    for page in &lint.orphans {
        under(&mut out, page.clone());
    }
    row(
        &mut out,
        "missing_frontmatter",
        lint.counts.missing_frontmatter.to_string(),
        "pages without source/ingested_at",
    );
    for held in &lint.missing_frontmatter {
        under(
            &mut out,
            format!("{} ({})", held.page, held.missing.join(", ")),
        );
    }
    row(
        &mut out,
        "undeclared_relations",
        lint.counts.undeclared_relations.to_string(),
        "prose links no relation key declares",
    );
    for held in &lint.undeclared_relations {
        under(
            &mut out,
            format!("{} → {}", held.page, held.targets.join(", ")),
        );
    }
    row(
        &mut out,
        "unlogged_raw",
        lint.counts.unlogged_raw.to_string(),
        "raw items without an ingestion record",
    );
    for item in &lint.unlogged_raw {
        under(&mut out, item.clone());
    }
    row(
        &mut out,
        "unsourced_edges",
        lint.counts.unsourced_edges.to_string(),
        "relations whose provenance no road vouches for",
    );
    for held in &lint.unsourced_edges {
        under(
            &mut out,
            format!(
                "{} → {} ({} · {})",
                held.from,
                held.to,
                held.kind
                    .map_or("?", zerocode_core::second_brain_graph::EdgeKind::as_str),
                held.provenance.map_or(
                    "?",
                    zerocode_core::second_brain_graph::EdgeProvenance::as_str
                ),
            ),
        );
    }
    row(
        &mut out,
        "contradictions",
        lint.counts.contradictions.to_string(),
        "declared `contradicts` relations (a fact, not a finding)",
    );
    row(
        &mut out,
        "superseded",
        lint.counts.superseded.to_string(),
        "pages a `supersedes` relation points at (a fact, not a finding)",
    );
    for page in &lint.superseded {
        under(&mut out, page.clone());
    }
    row(
        &mut out,
        "merge_candidates",
        lint.counts
            .merge_candidates
            .map_or_else(|| "—".to_string(), |held| held.to_string()),
        "pairs that look like one page (dedupe lens, t-2931; — = not computed)",
    );
    if let Some(pairs) = &lint.merge_candidates {
        for pair in pairs {
            under(
                &mut out,
                format!(
                    "{} ≈ {} ({}){}",
                    pair.left,
                    pair.right,
                    pair.reason,
                    pair.proposal
                        .as_deref()
                        .map_or(String::new(), |word| format!(" → {word}"))
                ),
            );
        }
    }
    out.push_str(&format!(
        "\nfindings: {} → exit {}\n",
        lint.findings,
        if lint.is_clean() { 0 } else { EXIT_FINDINGS }
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_argument_wins_then_the_environment_then_the_saved_setting() {
        let settings = || Some(PathBuf::from("/saved"));
        assert_eq!(
            resolve(Some(PathBuf::from("/arg")), Some("/env".into()), settings),
            Some(Named::Argument(PathBuf::from("/arg")))
        );
        assert_eq!(
            resolve(None, Some("/env".into()), settings),
            Some(Named::Environment(PathBuf::from("/env")))
        );
        // A blank variable is no vault, not a vault at "".
        assert_eq!(
            resolve(None, Some("  ".into()), settings),
            Some(Named::Settings(PathBuf::from("/saved")))
        );
        assert_eq!(resolve(None, None, || None), None);
    }

    #[test]
    fn the_saved_vault_is_one_key_inside_the_document_s_data_envelope() {
        let dir = tempfile::tempdir().unwrap();
        let document = dir.path().join(PREFERENCES_FILE);
        std::fs::write(
            &document,
            r#"{"_meta":{"format":1,"revision":3},"data":{"second_brain_vault":" /Users/me/Knowledge "}}"#,
        )
        .unwrap();
        assert_eq!(
            saved_vault_in(&document),
            Some(PathBuf::from("/Users/me/Knowledge"))
        );
        std::fs::write(&document, r#"{"data":{"second_brain_vault":""}}"#).unwrap();
        assert_eq!(saved_vault_in(&document), None);
        std::fs::write(&document, "not json").unwrap();
        assert_eq!(saved_vault_in(&document), None);
        assert_eq!(saved_vault_in(&dir.path().join("absent.json")), None);
    }

    /// The envelope key and the setting key are spelled by the window's
    /// settings store; this command reads them and must not drift. Pinned
    /// against the source rather than linked, because the store crate pulls
    /// a keychain and an HTTP client a lint CLI has no business loading.
    #[test]
    fn the_keys_this_command_reads_are_the_ones_the_window_writes() {
        let store = include_str!("../../zerocode-shell-state/src/settings.rs");
        assert!(
            store.contains(&format!(
                "root.insert(\"{DOCUMENT_DATA_KEY}\".into(), data);"
            )),
            "the settings store no longer writes its data under `{DOCUMENT_DATA_KEY}`"
        );
        let runtime = include_str!("../../zerocode-shell/src/settings_runtime.rs");
        assert!(
            runtime.contains(&format!(
                "pub const SECOND_BRAIN_VAULT: &str = \"{SAVED_VAULT_KEY}\";"
            )),
            "the window's saved-vault key is no longer `{SAVED_VAULT_KEY}`"
        );
    }

    #[test]
    fn arguments_parse_and_anything_else_is_refused_with_the_usage() {
        assert_eq!(
            parse(&["--vault".into(), "/v".into(), "--json".into()]).unwrap(),
            Options {
                vault: Some(PathBuf::from("/v")),
                json: true
            }
        );
        assert!(
            parse(&["--vault".into()])
                .unwrap_err()
                .contains("--vault needs")
        );
        assert!(
            parse(&["--nope".into()])
                .unwrap_err()
                .contains("zerocode vault-lint")
        );
    }
}
