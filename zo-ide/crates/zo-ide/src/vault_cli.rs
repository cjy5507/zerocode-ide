//! `zo vault …` — the second brain's graph, from outside a session.
//!
//! Three verbs. `path`, the question Graphify answers with "how do these two
//! concepts connect" (t-5966 G3): the calculator is the window's own,
//! [`zerocode_core::second_brain_paths::report`] over the same scanner the
//! knowledge graph and `zerocode vault-lint` read — this door prints the
//! answer on a pane and counts nothing itself. And `code` (t-5970 G2): the
//! code the vault's pages name, resolved against a project's codegraph index
//! into the [`CodeLayer`] the window grafts onto its picture
//! ([`zerocode_core::second_brain_code::graft`]) — the index is read here,
//! once, and nowhere in the window.
//!
//! ```text
//! zo vault path <from> <to> [--k <n>] [--json] [--vault <dir>] [--cwd <dir>]
//! zo vault code [--project <dir>] [--json] [--vault <dir>] [--cwd <dir>]
//! ```
//!
//! Where the vault is comes from the same roads every zo surface reads:
//! `--vault`, else [`SecondBrain::resolve`] (the pane's
//! `ZEROCODE_SECOND_BRAIN`, else the merged settings' `secondBrain.vault`).
//! No session, no credentials, no workspace trust — `--doctor`'s principle.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::Instant;

use codegraph::{CodeGraph, Resolved};

use runtime::second_brain::{SecondBrain, WIKI_DIR};
use runtime::ConfigLoader;
use serde_json::{json, Value};
use zerocode_core::second_brain_code::{
    file_id, symbol_id, CodeEdge, CodeLayer, CodeLimits, CodeNode,
};
use zerocode_core::second_brain_graph::{EdgeKind, GraphCache, NodeKind, VaultGraph};
use zerocode_core::second_brain_paths::{render_chain, report, PathReport, PATH_LIMITS};

pub const USAGE: &str = "\
zo vault path <from> <to> [--k <n>] [--json] [--vault <dir>] [--cwd <dir>]
zo vault code [--project <dir>] [--json] [--vault <dir>] [--cwd <dir>]
zo vault pairs [--limit <n>] [--json] [--vault <dir>] [--cwd <dir>]
zo vault mark <left> <right> <merge|related|supersedes|contradicts|none> [--vault <dir>]

  path: paths between two pages of the second brain, shortest first, each
  hop naming the relation and the road that wrote it (measured · declared ·
  inferred). A page is named by its id (wiki/a/b.md), its path below wiki/,
  its file stem or its title.
  code: the files and definitions of a project (--project, else the working
  directory) that the pages name in their source: and backtick spans, as the
  project's codegraph index places them — each joined to the pages naming it,
  and the files to the files they import from. Builds the index if the
  project has none.
  The vault is --vault, else the pane's ZEROCODE_SECOND_BRAIN, else the
  merged settings' secondBrain.vault.
";

/// The verbs, in a table so the parser names the word that is not one.
const VERBS: [&str; 4] = ["path", "code", "pairs", "mark"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verb {
    Path,
    Code,
    Pairs,
    Mark,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Request {
    verb: Verb,
    from: String,
    to: String,
    k: usize,
    json: bool,
    vault: Option<PathBuf>,
    cwd: Option<PathBuf>,
    project: Option<PathBuf>,
    limit: usize,
    label: Option<zerocode_core::second_brain_pairs::Suggestion>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub text: String,
}

fn parse(args: &[String]) -> Result<Request, String> {
    let verb = match args.first().map(String::as_str) {
        Some("path") => Verb::Path,
        Some("code") => Verb::Code,
        Some("pairs") => Verb::Pairs,
        Some("mark") => Verb::Mark,
        Some("-h" | "--help") | None => return Err(USAGE.to_string()),
        Some(other) => {
            return Err(format!(
                "unknown `zo vault` verb `{other}` — one of {}\n\n{USAGE}",
                VERBS.join(", ")
            ))
        }
    };
    let mut request = Request {
        verb,
        from: String::new(),
        to: String::new(),
        k: PATH_LIMITS.k_max,
        json: false,
        vault: None,
        cwd: None,
        project: None,
        limit: zerocode_core::second_brain_pairs::PAIR_LIMIT,
        label: None,
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
            "--project" if verb == Verb::Code => {
                request.project = Some(PathBuf::from(
                    rest.next().ok_or("--project needs a directory")?,
                ));
            }
            "--limit" if verb == Verb::Pairs => {
                let value = rest.next().ok_or("--limit needs a number")?;
                request.limit = value.parse().map_err(|_| format!("--limit needs a number, not `{value}`"))?;
                if request.limit > zerocode_core::second_brain_pairs::PAIR_LIMIT {
                    return Err(format!("--limit exceeds {}", zerocode_core::second_brain_pairs::PAIR_LIMIT));
                }
            }
            "-h" | "--help" => return Err(USAGE.to_string()),
            other if other.starts_with("--") => {
                return Err(format!("unknown argument `{other}`\n\n{USAGE}"))
            }
            other => named.push(other.to_string()),
        }
    }
    if matches!(verb, Verb::Code | Verb::Pairs) {
        if let Some(stray) = named.first() {
            return Err(format!("`zo vault {}` takes no pages, got `{stray}`\n\n{USAGE}", if verb == Verb::Code { "code" } else { "pairs" }));
        }
        return Ok(request);
    }
    if verb == Verb::Mark {
        if named.len() != 3 {
            return Err(format!("`zo vault mark` needs two pages and one review decision\n\n{USAGE}"));
        }
        request.label = zerocode_core::second_brain_pairs::Suggestion::from_word(&named[2]);
        if request.label.is_none() {
            return Err(format!("unknown review decision `{}`\n\n{USAGE}", named[2]));
        }
        request.from.clone_from(&named[0]);
        request.to.clone_from(&named[1]);
        return Ok(request);
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
    if request.verb == Verb::Code {
        return run_code(&request, &cwd, vault.root());
    }
    if request.verb == Verb::Pairs {
        return run_pairs(&request, vault.root());
    }
    if request.verb == Verb::Mark {
        return run_mark(&request, vault.root());
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

fn run_mark(request: &Request, vault: &Path) -> Result<Report, String> {
    let graph = GraphCache::new().scan(vault, false);
    let actual = request.label.ok_or("missing review decision")?;
    let now_ms = i64::try_from(std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis()).unwrap_or(i64::MAX);
    let label = tools::mark_vault_pair(&graph, vault, &request.from, &request.to, actual, now_ms)?;
    let text = if request.json {
        serde_json::to_string_pretty(&label).map_err(|error| error.to_string())?
    } else {
        format!("{} ≈ {}: {} · agreed {} · baselineAgreed {}",
            label.left, label.right, label.actual.word(), label.agreed, label.baseline_agreed)
    };
    Ok(Report { text })
}

fn run_pairs(request: &Request, vault: &Path) -> Result<Report, String> {
    let graph = GraphCache::new().scan(vault, false);
    let mut old = zerocode_core::second_brain_pairs::valid_proposals(vault, &graph);
    let now_ms = i64::try_from(std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis()).unwrap_or(i64::MAX);
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()
        .map_err(|error| error.to_string())?;
    let report = runtime.block_on(tools::judge_vault_pairs(&graph, vault, request.limit, now_ms));
    old.extend(tools::recorded_vault_pair_proposals(&graph, vault));
    for row in &report.rows {
        if row.outcome != "answered" { continue; }
        let left = graph.nodes.iter().find(|node| node.id == row.left);
        let right = graph.nodes.iter().find(|node| node.id == row.right);
        if let (Some(left), Some(right)) = (left, right) {
            old.push(zerocode_core::second_brain_pairs::ProposalRecord {
                left: row.left.clone(), right: row.right.clone(),
                reason: row.reason.clone(),
                left_modified_ms: left.modified_ms, right_modified_ms: right.modified_ms,
                suggestion: row.proposal,
            });
        }
    }
    if !old.is_empty() && report.mode != "off" {
        let mut unique = BTreeMap::new();
        for row in old {
            unique.insert((row.left.clone(), row.right.clone()), row);
        }
        let rows = unique.into_values().collect::<Vec<_>>();
        zerocode_core::second_brain_pairs::save_proposals(vault, &rows)
            .map_err(|error| format!("pair proposals could not be saved: {error}"))?;
    }
    let text = if request.json {
        serde_json::to_string_pretty(&report).map_err(|error| error.to_string())?
    } else {
        format!("status {} · mode {} · pages {} · candidates {} · asked {} · answered {} · proposals {} · input tokens {} · cost USD {:?} · requests {}",
            report.status, report.mode, report.pages, report.candidates, report.asked, report.answered,
            report.proposals, report.input_tokens, report.cost_usd, report.requests)
    };
    Ok(Report { text })
}

/// `zo vault code`: the vault's code mentions over the project's index.
fn run_code(request: &Request, cwd: &Path, vault: &Path) -> Result<Report, String> {
    let project = request.project.clone().unwrap_or_else(|| cwd.to_path_buf());
    let project = project
        .canonicalize()
        .map_err(|error| format!("{}: {error}", project.display()))?;
    let began = Instant::now();
    let mut pages = GraphCache::new();
    pages.scan(vault, false);
    let mut graph = CodeGraph::load_or_build(&project, tools::codegraph_cache_path(&project))
        .map_err(|error| error.to_string())?;
    let layer = code_layer(
        &pages.code_mentions(),
        &mut graph,
        &project.display().to_string(),
        &CodeLimits::default(),
    )?;
    let elapsed_ms = u64::try_from(began.elapsed().as_millis()).unwrap_or(u64::MAX);
    Ok(Report {
        text: if request.json {
            serde_json::to_string(&layer).map_err(|error| error.to_string())?
        } else {
            code_receipt(vault, elapsed_ms, &layer)
        },
    })
}

/// The code layer for a vault's pages over one project's index: every
/// page's code mentions resolved in one batch (`CodeGraph::resolve_mentions`),
/// a node per file or definition named, an `implements` line from it to each
/// page naming it, and the index's links (`CodeGraph::file_links`) between
/// the files the layer holds as `depends_on`. Bounded by `limits`.
///
/// # Errors
///
/// The index could not be read.
pub fn code_layer(
    pages: &BTreeMap<&str, &[String]>,
    graph: &mut CodeGraph,
    project: &str,
    limits: &CodeLimits,
) -> Result<CodeLayer, String> {
    let distinct = pages
        .values()
        .flat_map(|mentions| mentions.iter().cloned())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let resolved = graph
        .resolve_mentions(&distinct)
        .map_err(|error| error.to_string())?;
    let placed = distinct
        .iter()
        .zip(&resolved)
        .filter_map(|(mention, answer)| Some((mention.as_str(), code_node(answer.as_ref()?))))
        .collect::<HashMap<_, _>>();
    let mut layer = CodeLayer {
        project: project.to_string(),
        mentions: distinct.len(),
        resolved: placed.len(),
        ..CodeLayer::default()
    };
    // Every placed node with the pages naming it; past the bound, the nodes
    // the most pages name stay — a picture keeps what the vault leans on,
    // not what the first pages in id order happened to name.
    let mut named = BTreeMap::<String, (CodeNode, BTreeSet<&str>)>::new();
    for (page, mentions) in pages {
        for node in mentions.iter().filter_map(|mention| placed.get(mention.as_str())) {
            named
                .entry(node.id.clone())
                .or_insert_with(|| (node.clone(), BTreeSet::new()))
                .1
                .insert(page);
        }
    }
    let mut ranked = named.into_values().collect::<Vec<_>>();
    // Stable: equals keep the id order the map gave them.
    ranked.sort_by(|left, right| right.1.len().cmp(&left.1.len()));
    if ranked.len() > limits.layer_nodes_max {
        ranked.truncate(limits.layer_nodes_max);
        layer.capped = true;
    }
    let mut nodes = BTreeMap::<String, CodeNode>::new();
    for (node, naming) in ranked {
        for page in naming {
            layer.edges.push(CodeEdge {
                from: node.id.clone(),
                to: page.to_string(),
                kind: EdgeKind::Implements,
            });
        }
        nodes.insert(node.id.clone(), node);
    }
    let files = nodes
        .values()
        .filter(|node| node.kind == NodeKind::CodeFile)
        .map(|node| PathBuf::from(&node.file))
        .collect::<BTreeSet<_>>();
    for file in &files {
        let links = graph
            .file_links(file, limits.layer_edges_max)
            .map_err(|error| error.to_string())?
            .unwrap_or_default();
        for used in links.uses.iter().filter(|used| files.contains(&used.file)) {
            layer.edges.push(CodeEdge {
                from: file_id(&wire_path(file)),
                to: file_id(&wire_path(&used.file)),
                kind: EdgeKind::DependsOn,
            });
        }
    }
    if layer.edges.len() > limits.layer_edges_max {
        layer.edges.truncate(limits.layer_edges_max);
        layer.capped = true;
    }
    layer.nodes = nodes.into_values().collect();
    Ok(layer)
}

/// The node a resolved mention stands for.
fn code_node(resolved: &Resolved) -> CodeNode {
    match resolved {
        Resolved::File(file) => {
            let file = wire_path(file);
            CodeNode {
                id: file_id(&file),
                kind: NodeKind::CodeFile,
                title: file.rsplit('/').next().unwrap_or(&file).to_string(),
                detail: file.rsplit_once('/').map_or_else(String::new, |(folder, _)| folder.to_string()),
                file,
                line: None,
            }
        }
        Resolved::Symbol(symbol) => {
            let file = wire_path(&symbol.file);
            let line = u32::try_from(symbol.range.start.row + 1).unwrap_or(u32::MAX);
            CodeNode {
                id: symbol_id(&file, &symbol.name, line),
                kind: NodeKind::CodeSymbol,
                title: symbol
                    .container
                    .as_ref()
                    .map_or_else(|| symbol.name.clone(), |container| format!("{container}::{}", symbol.name)),
                detail: symbol.kind.as_str().to_string(),
                file,
                line: Some(line),
            }
        }
    }
}

/// A project path as the layer spells it: `/`-separated on every platform.
fn wire_path(path: &Path) -> String {
    path.components()
        .map(|part| part.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn code_receipt(vault: &Path, elapsed_ms: u64, layer: &CodeLayer) -> String {
    let files = layer.nodes.iter().filter(|node| node.kind == NodeKind::CodeFile).count();
    let implements = layer.edges.iter().filter(|edge| edge.kind == EdgeKind::Implements).count();
    let mut out = format!("vault: {}\nproject: {}\n", vault.display(), layer.project);
    let _ = writeln!(
        out,
        "mentions {} · placed {} · files {files} · definitions {} · implements {implements} · depends_on {} · {elapsed_ms} ms{}",
        layer.mentions,
        layer.resolved,
        layer.nodes.len() - files,
        layer.edges.len() - implements,
        if layer.capped { " · cut at a bound" } else { "" }
    );
    out
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
    fn the_code_verb_takes_a_project_and_no_pages() {
        let request = parse(&args(&["code", "--project", "/p", "--json"])).unwrap();
        assert_eq!(request.verb, Verb::Code);
        assert_eq!(request.project, Some(PathBuf::from("/p")));
        assert!(parse(&args(&["code", "A"])).unwrap_err().starts_with("`zo vault code` takes no pages"));
        assert!(parse(&args(&["path", "A", "B", "--project", "/p"])).unwrap_err().contains("unknown argument"));
    }

    #[test]
    fn pair_run_is_bounded_and_a_review_names_an_explicit_outcome() {
        let pairs = parse(&args(&["pairs", "--limit", "50", "--vault", "/v"])).expect("pair run");
        assert_eq!(pairs.verb, Verb::Pairs);
        assert_eq!(pairs.limit, 50);
        let over = (zerocode_core::second_brain_pairs::PAIR_LIMIT + 1).to_string();
        assert!(parse(&args(&["pairs", "--limit", &over])).is_err());
        let review = parse(&args(&["mark", "wiki/a.md", "wiki/b.md", "none"]))
            .expect("review mark");
        assert_eq!(review.verb, Verb::Mark);
        assert_eq!(review.label, Some(zerocode_core::second_brain_pairs::Suggestion::None));
        assert!(parse(&args(&["mark", "wiki/a.md", "wiki/b.md", "maybe"])).is_err());
    }

    /// The pages name a file, a definition, a word and a file the project
    /// does not have; the layer holds what the index placed, each joined to
    /// the pages naming it, and the one import between the two files.
    #[test]
    fn the_code_layer_joins_what_the_index_places_to_the_pages_naming_it() {
        let project = tempfile::tempdir().expect("project");
        let src = project.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("scan.rs"), "pub fn scan_workspace() {}\n").unwrap();
        std::fs::write(src.join("graph.rs"), "use crate::scan::scan_workspace;\nfn graph() { scan_workspace(); }\n").unwrap();
        let mut graph = CodeGraph::load_or_build(project.path(), project.path().join("state/index.sqlite")).unwrap();
        let graph_page = vec!["src/graph.rs".to_string(), "missing.rs".to_string(), "words_only".to_string()];
        let scan_page = vec!["scan_workspace".to_string(), "src/graph.rs".to_string()];
        let named_pages = BTreeMap::from([("wiki/Graph.md", graph_page.as_slice()), ("wiki/Scan.md", scan_page.as_slice())]);
        let layer = code_layer(&named_pages, &mut graph, "acme", &CodeLimits::default()).unwrap();
        assert_eq!((layer.mentions, layer.resolved, layer.capped), (4, 2, false));
        let ids = layer.nodes.iter().map(|node| node.id.as_str()).collect::<Vec<_>>();
        assert_eq!(ids, ["code:src/graph.rs", "code:src/scan.rs#scan_workspace@1"]);
        let edges = layer
            .edges
            .iter()
            .map(|edge| (edge.from.as_str(), edge.to.as_str(), edge.kind))
            .collect::<Vec<_>>();
        // Most named first: graph.rs by two pages, then the definition by one.
        assert_eq!(
            edges,
            [
                ("code:src/graph.rs", "wiki/Graph.md", EdgeKind::Implements),
                ("code:src/graph.rs", "wiki/Scan.md", EdgeKind::Implements),
                ("code:src/scan.rs#scan_workspace@1", "wiki/Scan.md", EdgeKind::Implements),
            ]
        );

        // Past the bound, the most named node is the one kept.
        let one = CodeLimits {
            layer_nodes_max: 1,
            ..CodeLimits::default()
        };
        let cut = code_layer(&named_pages, &mut graph, "acme", &one).unwrap();
        assert!(cut.capped);
        assert_eq!(cut.nodes.len(), 1);
        assert_eq!(cut.nodes[0].id, "code:src/graph.rs");
        // With the scanned file named too, the import between them is a line.
        let both = vec!["src/graph.rs".to_string(), "src/scan.rs".to_string()];
        let pages = BTreeMap::from([("wiki/Graph.md", both.as_slice())]);
        let layer = code_layer(&pages, &mut graph, "acme", &CodeLimits::default()).unwrap();
        assert!(layer.edges.contains(&CodeEdge {
            from: "code:src/graph.rs".to_string(),
            to: "code:src/scan.rs".to_string(),
            kind: EdgeKind::DependsOn,
        }));
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
