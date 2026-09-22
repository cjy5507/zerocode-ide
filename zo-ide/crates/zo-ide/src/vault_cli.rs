//! `zo vault …` — the second brain's graph, from outside a session.
//!
//! One verb today: `path`, the question Graphify answers with "how do these
//! two concepts connect" (t-5966 G3). The calculator is the window's own,
//! [`zerocode_core::second_brain_paths::report`] over the same scanner the
//! knowledge graph and `zerocode vault-lint` read — this door prints the
//! answer on a pane and counts nothing itself.
//!
//! ```text
//! zo vault path <from> <to> [--k <n>] [--json] [--vault <dir>] [--cwd <dir>]
//! ```
//!
//! Where the vault is comes from the same roads every zo surface reads:
//! `--vault`, else [`SecondBrain::resolve`] (the pane's
//! `ZEROCODE_SECOND_BRAIN`, else the merged settings' `secondBrain.vault`).
//! No session, no credentials, no workspace trust — `--doctor`'s principle.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::Instant;

use runtime::second_brain::{SecondBrain, WIKI_DIR};
use runtime::ConfigLoader;
use serde_json::{json, Value};
use zerocode_core::second_brain_graph::{GraphCache, VaultGraph};
use zerocode_core::second_brain_paths::{render_chain, report, PathReport, PATH_LIMITS};

pub const USAGE: &str = "\
zo vault path <from> <to> [--k <n>] [--json] [--vault <dir>] [--cwd <dir>]

  Paths between two pages of the second brain, shortest first, each hop
  naming the relation and the road that wrote it (measured · declared ·
  inferred). A page is named by its id (wiki/a/b.md), its path below wiki/,
  its file stem or its title. The vault is --vault, else the pane's
  ZEROCODE_SECOND_BRAIN, else the merged settings' secondBrain.vault.
";

/// The verbs, in a table so the parser names the word that is not one.
const VERBS: [&str; 1] = ["path"];

#[derive(Debug, Clone, PartialEq, Eq)]
struct Request {
    from: String,
    to: String,
    k: usize,
    json: bool,
    vault: Option<PathBuf>,
    cwd: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub text: String,
}

fn parse(args: &[String]) -> Result<Request, String> {
    match args.first().map(String::as_str) {
        Some("path") => {}
        Some("-h" | "--help") | None => return Err(USAGE.to_string()),
        Some(other) => {
            return Err(format!(
                "unknown `zo vault` verb `{other}` — one of {}\n\n{USAGE}",
                VERBS.join(", ")
            ))
        }
    }
    let mut request = Request {
        from: String::new(),
        to: String::new(),
        k: PATH_LIMITS.k_max,
        json: false,
        vault: None,
        cwd: None,
    };
    let mut named: Vec<String> = Vec::new();
    let mut rest = args[1..].iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--k" => {
                let value = rest.next().ok_or("--k needs a number")?;
                request.k = value
                    .parse()
                    .map_err(|_| format!("--k needs a number, not `{value}`"))?;
            }
            "--json" => request.json = true,
            "--vault" => {
                request.vault = Some(PathBuf::from(
                    rest.next().ok_or("--vault needs a directory")?,
                ));
            }
            "--cwd" => {
                request.cwd = Some(PathBuf::from(rest.next().ok_or("--cwd needs a directory")?));
            }
            "-h" | "--help" => return Err(USAGE.to_string()),
            other if other.starts_with("--") => {
                return Err(format!("unknown argument `{other}`\n\n{USAGE}"))
            }
            other => named.push(other.to_string()),
        }
    }
    if named.len() != 2 {
        return Err(format!(
            "`zo vault path` needs exactly two pages, got {}\n\n{USAGE}",
            named.len()
        ));
    }
    request.to = named.pop().unwrap_or_default();
    request.from = named.pop().unwrap_or_default();
    Ok(request)
}

/// The vault, by the roads in the module's words.
fn resolve_vault(request: &Request, cwd: &Path) -> Result<SecondBrain, String> {
    if let Some(root) = &request.vault {
        return Ok(SecondBrain::at(root.clone()));
    }
    let config = ConfigLoader::default_for(cwd)
        .load()
        .map_err(|error| format!("settings could not be read: {error}"))?;
    SecondBrain::resolve(&config).ok_or_else(|| {
        format!(
            "no vault — pass --vault <dir>, export {}, or set secondBrain.vault",
            runtime::second_brain::VAULT_ENV
        )
    })
}

/// # Errors
///
/// The usage, what was wrong with the arguments, a vault nothing names, or a
/// page the graph does not hold.
pub fn run(args: &[String], cwd: &Path) -> Result<Report, String> {
    let request = parse(args)?;
    let cwd = request.cwd.clone().unwrap_or_else(|| cwd.to_path_buf());
    let vault = resolve_vault(&request, &cwd)?;
    if !vault.is_set_up() {
        return Err(format!(
            "{} has no {WIKI_DIR}/ — not a second-brain vault",
            vault.root().display()
        ));
    }
    let began = Instant::now();
    let graph = GraphCache::new().scan(vault.root(), false);
    let scanned_ms = u64::try_from(began.elapsed().as_millis()).unwrap_or(u64::MAX);
    let answer = report(&graph, &request.from, &request.to, request.k).map_err(|refusal| refusal.to_string())?;
    Ok(Report {
        text: if request.json {
            json_receipt(vault.root(), scanned_ms, &graph, &answer).to_string()
        } else {
            text_receipt(vault.root(), scanned_ms, &graph, &answer)
        },
    })
}

/// The picture's size line — the same three counts the report's table reads.
fn provenance_words(graph: &VaultGraph) -> String {
    graph
        .provenances
        .iter()
        .map(|row| format!("{} {}", row.provenance.as_str(), row.count))
        .collect::<Vec<_>>()
        .join(" · ")
}

fn text_receipt(root: &Path, scanned_ms: u64, graph: &VaultGraph, answer: &PathReport) -> String {
    let mut out = format!("vault: {}\n", root.display());
    let _ = writeln!(
        out,
        "pages {} · relations {} ({}) · scanned in {scanned_ms} ms",
        graph.pages,
        graph.edges.len(),
        provenance_words(graph)
    );
    let shortest = answer
        .shortest
        .map_or_else(|| "not connected".to_string(), |hops| format!("shortest {hops} hops"));
    let _ = writeln!(
        out,
        "{} → {}: {} path{} ({shortest}, k {}, walk {} µs{})",
        answer.from.title,
        answer.to.title,
        answer.paths.len(),
        if answer.paths.len() == 1 { "" } else { "s" },
        answer.k,
        answer.elapsed_us,
        if answer.capped { ", cut at the budget" } else { "" }
    );
    for (at, path) in answer.paths.iter().enumerate() {
        let _ = writeln!(out, "  {}. {}", at + 1, render_chain(path));
    }
    out
}

fn json_receipt(root: &Path, scanned_ms: u64, graph: &VaultGraph, answer: &PathReport) -> Value {
    json!({
        "vault": root.display().to_string(),
        "scannedMs": scanned_ms,
        "graph": {
            "pages": graph.pages,
            "edges": graph.edges.len(),
            "provenances": graph.provenances,
        },
        "report": answer,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(words: &[&str]) -> Vec<String> {
        words.iter().map(|word| (*word).to_string()).collect()
    }

    /// A vault of three pages: A links B in prose, B declares it implements C.
    fn vault() -> tempfile::TempDir {
        let root = tempfile::tempdir().expect("tempdir");
        let wiki = root.path().join(WIKI_DIR);
        std::fs::create_dir_all(&wiki).unwrap();
        std::fs::write(wiki.join("A.md"), "---\ntitle: 알파\n---\nsee [[B]]\n").unwrap();
        std::fs::write(wiki.join("B.md"), "---\nimplements: [[C]]\n---\nplain\n").unwrap();
        std::fs::write(wiki.join("C.md"), "plain\n").unwrap();
        root
    }

    #[test]
    fn the_verb_parses_its_two_pages_and_refuses_anything_else() {
        let request = parse(&args(&["path", "알파", "C", "--k", "3", "--json", "--vault", "/v"])).unwrap();
        assert_eq!(request.from, "알파");
        assert_eq!(request.to, "C");
        assert_eq!(request.k, 3);
        assert!(request.json);
        assert_eq!(request.vault, Some(PathBuf::from("/v")));
        assert_eq!(parse(&args(&["path", "A"])).unwrap_err().lines().next().unwrap(), "`zo vault path` needs exactly two pages, got 1");
        assert!(parse(&args(&["path", "A", "B", "--k", "x"])).unwrap_err().contains("--k needs a number"));
        assert!(parse(&args(&["frobnicate"])).unwrap_err().starts_with("unknown `zo vault` verb"));
        assert!(parse(&args(&[])).unwrap_err().starts_with("zo vault path"));
        assert!(parse(&args(&["path", "A", "B", "--nope"])).unwrap_err().contains("unknown argument"));
    }

    #[test]
    fn a_path_is_printed_as_a_chain_with_each_hops_kind_and_road_and_json_carries_the_report() {
        let root = vault();
        let vault = root.path().to_string_lossy().into_owned();
        let text = run(&args(&["path", "알파", "C", "--vault", &vault]), Path::new("/")).unwrap().text;
        assert!(text.contains("pages 3 · relations 2 (measured 0 · declared 1 · inferred 1)"), "{text}");
        assert!(text.contains("알파 → C: 1 path (shortest 2 hops"), "{text}");
        assert!(text.contains("  1. 알파 →(mentions·inferred) B →(implements·declared) C\n"), "{text}");
        let json: Value = serde_json::from_str(
            &run(&args(&["path", "C", "wiki/A.md", "--json", "--vault", &vault]), Path::new("/")).unwrap().text,
        )
        .unwrap();
        assert_eq!(json["graph"]["pages"], 3);
        assert_eq!(json["report"]["from"]["id"], "wiki/C.md");
        assert_eq!(json["report"]["paths"][0]["hops"][0]["kind"], "implements");
        assert_eq!(json["report"]["paths"][0]["hops"][0]["provenance"], "declared");
        assert_eq!(json["report"]["paths"][0]["hops"][0]["reversed"], true);
        assert_eq!(json["report"]["shortest"], 2);
        assert!(json["report"]["elapsedUs"].is_null(), "serde keeps snake_case on the wire");
        assert!(json["report"]["elapsed_us"].is_u64());
    }

    #[test]
    fn an_unknown_page_and_a_folder_without_a_wiki_are_refused_in_one_sentence() {
        let root = vault();
        let vault = root.path().to_string_lossy().into_owned();
        let refused = run(&args(&["path", "A", "nothing", "--vault", &vault]), Path::new("/")).unwrap_err();
        assert_eq!(refused, "no page named `nothing` (to)");
        let bare = tempfile::tempdir().expect("tempdir");
        let refused = run(
            &args(&["path", "A", "B", "--vault", &bare.path().to_string_lossy()]),
            Path::new("/"),
        )
        .unwrap_err();
        assert!(refused.ends_with("not a second-brain vault"), "{refused}");
    }
}
