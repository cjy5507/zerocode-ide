//! `zo vault …` — the second brain's graph, from outside a session.
//!
//! `search`: the files a few words point at, each with the passage that
//! holds the most of them and the line it starts on. The answer comes from
//! one index of passages the window's core keeps
//! ([`zerocode_core::second_brain_search`]): the vault's pages first, then
//! its originals and any folder a person added with `source add`, each read
//! where it lies and only when it changed. It is the vault's search for an
//! agent, which cannot read `wiki/index.md` whole once the vault outgrows
//! one read. This door reads the arguments and prints the answer; it ranks
//! nothing itself.
//!
//! Three more verbs. `path`, the question Graphify answers with "how do these two
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
//! zo vault search <words…> [--limit <n>] [--layer <name>] [--json] [--vault <dir>] [--cwd <dir>]
//! zo vault source add <folder> [--layer <name>] | list | remove <folder|layer>
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
use zerocode_core::second_brain_search::{self as vault_search, Answer, Hit, LayerReport};
use runtime::ConfigLoader;
use serde_json::{json, Value};
use zerocode_core::second_brain_code::{
    file_id, symbol_id, CodeEdge, CodeLayer, CodeLimits, CodeNode,
};
use zerocode_core::second_brain_graph::{EdgeKind, GraphCache, NodeKind, VaultGraph};
use zerocode_core::second_brain_paths::{render_chain, report, PathReport, PATH_LIMITS};

pub const USAGE: &str = "\
zo vault search <words…> [--limit <n>] [--layer <name>] [--json] [--vault <dir>] [--cwd <dir>]
zo vault source add <folder> [--layer <name>] | list | remove <folder|layer>
zo vault path <from> <to> [--k <n>] [--json] [--vault <dir>] [--cwd <dir>]
zo vault code [--project <dir>] [--json] [--vault <dir>] [--cwd <dir>]
zo vault pairs [--limit <n>] [--json] [--vault <dir>] [--cwd <dir>]
zo vault mark <left> <right> <merge|related|supersedes|contradicts|none> [--vault <dir>]

  search: the files these words point at, best first — the vault's pages
  (wiki), then its originals (raw) and any folder added with `source add`.
  Each answer names its path and the line its best passage starts on
  (path:line); the first answers of a layer show that passage, the rest the
  one line of it that holds most of the words. A rare word counts for more
  than a common one, a file the words would match by chance is left out, and
  a page another page replaced (supersedes) gives its place to that page.
  The answer comes from an index of passages under the vault's .zerocode/:
  only files that changed are read again. --layer asks one layer alone.
  Open a file before relying on it.
  source: the folders outside the vault the search also reads, each where it
  lies and under its own layer. add lists a folder (its name is the layer
  unless --layer says), list shows what is read, remove takes one out by its
  folder or its layer and drops its index; the folder is never written to.
  An added folder's words, paths and titles are kept in the index under the
  vault's .zerocode/, so a vault that is synced carries them along: add a
  folder only when its owner asks.
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
const VERBS: [&str; 6] = ["search", "source", "path", "code", "pairs", "mark"];

/// What `zo vault source` does, as its first word.
const SOURCE_ADD: &str = "add";
const SOURCE_LIST: &str = "list";
const SOURCE_REMOVE: &str = "remove";

/// Pages `search` names when nobody says, and the most it names: a list an
/// agent reads whole before it opens any of them.
const SEARCH_LIMIT: usize = 8;
const SEARCH_LIMIT_MAX: usize = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verb {
    Search,
    Source,
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
    /// The layer `search` asks alone, or the one `source add` files a folder under.
    layer: Option<String>,
    label: Option<zerocode_core::second_brain_pairs::Suggestion>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub text: String,
}

fn parse(args: &[String]) -> Result<Request, String> {
    let verb = match args.first().map(String::as_str) {
        Some("search") => Verb::Search,
        Some("source") => Verb::Source,
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
        limit: if verb == Verb::Search {
            SEARCH_LIMIT
        } else {
            zerocode_core::second_brain_pairs::PAIR_LIMIT
        },
        layer: None,
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
            "--layer" if matches!(verb, Verb::Search | Verb::Source) => {
                request.layer = Some(rest.next().ok_or("--layer needs a name")?.clone());
            }
            "--limit" if verb == Verb::Search => {
                let value = rest.next().ok_or("--limit needs a number")?;
                request.limit = value
                    .parse()
                    .map_err(|_| format!("--limit needs a number, not `{value}`"))?;
                if request.limit == 0 || request.limit > SEARCH_LIMIT_MAX {
                    return Err(format!("--limit is 1 to {SEARCH_LIMIT_MAX}"));
                }
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
    take_pages(request, named)
}

/// The words that are not flags, by what the verb takes: the words to look
/// for, two pages, two pages and a decision, or nothing.
fn take_pages(mut request: Request, mut named: Vec<String>) -> Result<Request, String> {
    let verb = request.verb;
    if verb == Verb::Search {
        if named.is_empty() {
            return Err(format!("`zo vault search` needs the words to look for\n\n{USAGE}"));
        }
        request.from = named.join(" ");
        return Ok(request);
    }
    if verb == Verb::Source {
        // `from` is what to do, `to` the folder or layer it is done to.
        let wanted = match named.first().map(String::as_str) {
            Some(SOURCE_LIST) => Some(1),
            Some(SOURCE_ADD | SOURCE_REMOVE) => Some(2),
            _ => None,
        };
        if wanted != Some(named.len()) {
            return Err(format!(
                "`zo vault source` takes `{SOURCE_ADD} <folder>`, `{SOURCE_LIST}` or `{SOURCE_REMOVE} <folder|layer>`\n\n{USAGE}"
            ));
        }
        let mut named = named.into_iter();
        request.from = named.next().unwrap_or_default();
        request.to = named.next().unwrap_or_default();
        return Ok(request);
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
    if request.verb == Verb::Search {
        return run_search(&request, &vault);
    }
    if request.verb == Verb::Source {
        return run_source(&request, &cwd, vault.root());
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

/// The files the words point at, from the core's index of passages: no
/// project memory, no session, and nothing written but that index.
fn run_search(request: &Request, vault: &SecondBrain) -> Result<Report, String> {
    let began = Instant::now();
    let answer = vault_search::search(
        vault.root(),
        &vault_search::Request {
            words: &request.from,
            limit: request.limit,
            layer: request.layer.as_deref(),
        },
    )?;
    let took_ms = u64::try_from(began.elapsed().as_millis()).unwrap_or(u64::MAX);
    let text = if request.json {
        search_json(vault.root(), &request.from, took_ms, &answer).to_string()
    } else {
        search_text(vault.root(), &request.from, took_ms, &answer)
    };
    Ok(Report { text })
}

/// `path:line` for an answer whose passage has a line, else the path alone.
fn place_of(hit: &Hit) -> String {
    match hit.line_number {
        Some(line) => format!("{}:{line}", hit.path.display()),
        None => hit.path.display().to_string(),
    }
}

/// The vault's own pages in the index — what `pages` has always counted.
fn pages_of(answer: &Answer) -> usize {
    answer
        .layers
        .iter()
        .find(|layer| layer.layer == WIKI_DIR)
        .map_or(0, |layer| layer.files)
}

/// The answer on the wire. `found[].slug · summary · line · path`, `pages`,
/// `tookMs`, `vault` and `words` are what scripts and skills already read;
/// everything else was added beside them.
fn search_json(vault: &Path, words: &str, took_ms: u64, answer: &Answer) -> Value {
    json!({
        "vault": vault.display().to_string(),
        "words": words,
        "pages": pages_of(answer),
        "tookMs": took_ms,
        "found": answer.hits.iter().map(|hit| json!({
            "slug": hit.slug,
            "path": hit.path.display().to_string(),
            "summary": hit.summary(),
            "line": hit.line,
            "layer": hit.layer,
            "lineNumber": hit.line_number,
            "at": place_of(hit),
            "passage": hit.passage,
            "score": hit.score,
            "via": hit.via.as_ref().map(|via| json!({
                "kind": via.kind.as_str(),
                "page": via.seed,
                "declaredByThatPage": via.from_seed,
            })),
        })).collect::<Vec<_>>(),
        "layers": answer.layers.iter().map(|layer| json!({
            "layer": layer.layer,
            "files": layer.files,
            "passages": layer.passages,
            "index": layer.freshness.as_str(),
            "read": layer.read,
            "found": layer.found,
            "tookMs": layer.took_ms,
        })).collect::<Vec<_>>(),
        "notices": answer.notices,
    })
}

/// What each layer's index holds, on one line: `wiki 3 files · raw 1 file`.
fn layer_words(layers: &[LayerReport]) -> String {
    layers
        .iter()
        .map(|layer| {
            format!(
                "{} {} {}",
                layer.layer,
                layer.files,
                if layer.files == 1 { "file" } else { "files" }
            )
        })
        .collect::<Vec<_>>()
        .join(" · ")
}

fn search_text(vault: &Path, words: &str, took_ms: u64, answer: &Answer) -> String {
    let mut text = String::new();
    let indexed = layer_words(&answer.layers);
    if answer.hits.is_empty() {
        let _ = writeln!(
            text,
            "Nothing in {} holds \"{words}\" beyond what chance would match ({indexed} · {took_ms} ms).",
            vault.display()
        );
        let _ = write!(
            text,
            "Try a page's own words: a title word, a Latin name (Windows, not 윈도우 alone), a task id, or fewer words."
        );
    } else {
        let _ = writeln!(
            text,
            "{} {} in {} for \"{words}\" ({indexed} · {took_ms} ms):",
            answer.hits.len(),
            if answer.hits.len() == 1 { "answer" } else { "answers" },
            vault.display()
        );
        let mut layer = "";
        for (at, hit) in answer.hits.iter().enumerate() {
            if hit.layer != layer {
                layer = &hit.layer;
                let _ = writeln!(text, "[{layer}]");
            }
            let _ = writeln!(text, "{}. {} — {}", at + 1, hit.slug, hit.summary());
            // The passage when the answer carries it, else its one best line.
            let shown = hit.passage.as_deref().or(hit.line.as_deref()).unwrap_or_default();
            for line in shown.lines().filter(|line| !line.trim().is_empty()) {
                let _ = writeln!(text, "   > {line}");
            }
            let _ = writeln!(text, "   {}", place_of(hit));
        }
        let _ = write!(
            text,
            "Open a file before relying on it. A page another page replaced gives its place to that page."
        );
    }
    for notice in &answer.notices {
        let _ = write!(text, "\nnote: {notice}");
    }
    text
}

/// `zo vault source`: the folders outside the vault the search also reads.
fn run_source(request: &Request, cwd: &Path, vault: &Path) -> Result<Report, String> {
    let folder = || {
        let named = Path::new(&request.to);
        if named.is_absolute() {
            named.to_path_buf()
        } else {
            cwd.join(named)
        }
    };
    let said = match request.from.as_str() {
        SOURCE_ADD => {
            let source = vault_search::add_source(vault, &folder(), request.layer.as_deref())?;
            format!(
                "added {} as layer {} — the next search reads it where it lies\nnote: {}",
                source.root.display(),
                source.layer,
                vault_search::ADDED_FOLDER_NOTICE
            )
        }
        SOURCE_REMOVE => {
            // By its layer first; a path is tried as written and from the cwd.
            let gone = vault_search::remove_source(vault, &request.to)
                .or_else(|refused| vault_search::remove_source(vault, &folder().display().to_string()).map_err(|_| refused))?;
            gone.iter()
                .map(|source| {
                    format!(
                        "removed {} (layer {}) — its index is gone, the folder is untouched",
                        source.root.display(),
                        source.layer
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")
        }
        _ => String::new(),
    };
    let sources = vault_search::read_settings(vault).sources;
    let kind = |source: &vault_search::Source| match source.kind {
        vault_search::Kind::Wiki => "pages",
        vault_search::Kind::Raw => "originals",
        vault_search::Kind::Added => "added",
    };
    // Said wherever an added folder is listed: what of it the vault now holds.
    let notice = sources
        .iter()
        .any(|source| source.kind == vault_search::Kind::Added)
        .then_some(vault_search::ADDED_FOLDER_NOTICE);
    let text = if request.json {
        json!({
            "vault": vault.display().to_string(),
            "sources": sources.iter().map(|source| json!({
                "layer": source.layer,
                "path": source.root.display().to_string(),
                "kind": kind(source),
            })).collect::<Vec<_>>(),
            "notice": notice,
        })
        .to_string()
    } else {
        let mut text = said;
        for source in &sources {
            if !text.is_empty() {
                text.push('\n');
            }
            let _ = write!(text, "{} · {} · {}", source.layer, kind(source), source.root.display());
        }
        // `add` has said it already, on its own second line.
        if let Some(notice) = notice.filter(|_| request.from == SOURCE_LIST) {
            let _ = write!(text, "\nnote: {notice}");
        }
        text
    };
    Ok(Report { text })
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
        assert_eq!(parse(&args(&[])).unwrap_err(), USAGE);
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

    fn vault_with(pages: &[(&str, &str)]) -> tempfile::TempDir {
        let vault = tempfile::tempdir().expect("vault");
        std::fs::create_dir_all(vault.path().join("wiki")).expect("wiki");
        std::fs::create_dir_all(vault.path().join("raw")).expect("raw");
        for (name, text) in pages {
            std::fs::write(vault.path().join("wiki").join(format!("{name}.md")), text)
                .expect("page");
        }
        vault
    }

    #[test]
    fn search_takes_its_words_whole_and_bounds_its_list() {
        let request = parse(&args(&["search", "hook", "bridge"])).unwrap();
        assert_eq!(request.verb, Verb::Search);
        assert_eq!(request.from, "hook bridge");
        assert_eq!(request.limit, SEARCH_LIMIT);
        let request = parse(&args(&["search", "hook", "--limit", "3", "--layer", "raw"])).unwrap();
        assert_eq!(request.limit, 3);
        assert_eq!(request.layer.as_deref(), Some("raw"));
        assert!(parse(&args(&["path", "A", "B", "--layer", "raw"])).unwrap_err().contains("unknown argument"));
        assert!(parse(&args(&["search"])).unwrap_err().starts_with("`zo vault search` needs"));
        assert!(parse(&args(&["search", "hook", "--limit", "0"])).is_err());
        assert!(parse(&args(&["search", "hook", "--limit", "21"])).is_err());
    }

    /// The page the words name comes first, with its path and the line of it
    /// that holds the words; the frontmatter, the title heading and a section
    /// heading holding no more words than a body line are not that line.
    #[test]
    fn search_names_the_page_its_path_and_the_line_that_holds_the_words() {
        let vault = vault_with(&[
            (
                "hook-bridge",
                "---\ntitle: \"Hook Bridge\"\ntags: [hooks]\n---\n\n# Hook Bridge\n\n## Hook bridge wiring\n\nThe bridge carries every hook to the window.\n",
            ),
            ("cooking", "# Cooking\n\nUnrelated.\n"),
        ]);
        let root = vault.path().display().to_string();
        let report = run(&args(&["search", "hook", "bridge", "--vault", &root]), vault.path()).unwrap();
        let lines: Vec<&str> = report.text.lines().collect();
        assert!(lines[0].starts_with("1 answer in "), "{}", report.text);
        assert!(lines[0].contains("(wiki 2 files · raw 0 files · "), "{}", report.text);
        assert_eq!(lines[1], "[wiki]", "{}", report.text);
        assert_eq!(lines[2], "1. wiki/hook-bridge — Hook Bridge · tags: hooks", "{}", report.text);
        // The first answers carry their passage: the headings and the line under them.
        assert_eq!(
            lines[3..6],
            ["   > # Hook Bridge", "   > ## Hook bridge wiring", "   > The bridge carries every hook to the window."],
            "{}",
            report.text
        );
        let page = vault.path().join("wiki").join("hook-bridge.md");
        assert_eq!(lines[6], format!("   {}:6", page.display()), "{}", report.text);
        assert!(!report.text.contains("cooking"), "{}", report.text);

        let report = run(&args(&["search", "hook", "--json", "--vault", &root]), vault.path()).unwrap();
        let value: Value = serde_json::from_str(&report.text).unwrap();
        // What scripts already read, unchanged in name and meaning.
        assert_eq!(value["found"][0]["slug"], "wiki/hook-bridge");
        assert_eq!(value["found"][0]["path"], page.display().to_string());
        assert_eq!(value["found"][0]["summary"], "Hook Bridge · tags: hooks");
        assert_eq!(value["found"][0]["line"], "The bridge carries every hook to the window.");
        assert_eq!(value["pages"], 2);
        assert_eq!(value["words"], "hook");
        assert_eq!(value["vault"], root);
        assert!(value["tookMs"].is_u64());
        // What was added beside them.
        assert_eq!(value["found"][0]["layer"], "wiki");
        assert_eq!(value["found"][0]["lineNumber"], 6);
        assert_eq!(value["found"][0]["at"], format!("{}:6", page.display()));
        assert!(value["found"][0]["passage"].as_str().unwrap().ends_with("every hook to the window."));
        assert_eq!(value["layers"][0]["layer"], "wiki");
        assert_eq!(value["layers"][0]["index"], "fresh");
        assert_eq!(value["layers"][0]["files"], 2);
    }

    /// An original under `raw/` answers after the pages, under its own layer,
    /// and `--layer` asks one layer alone.
    #[test]
    fn search_shows_the_pages_first_and_the_originals_under_their_own_layer() {
        let vault = vault_with(&[("hook-bridge", "# Hook Bridge\n\nThe bridge carries every hook to the window.\n")]);
        std::fs::write(vault.path().join("raw").join("call-notes.txt"), "the hook fired twice on the bridge\n").unwrap();
        let root = vault.path().display().to_string();
        let report = run(&args(&["search", "hook", "bridge", "--vault", &root]), vault.path()).unwrap();
        let lines: Vec<&str> = report.text.lines().collect();
        let wiki = lines.iter().position(|line| *line == "[wiki]").expect("the pages' layer");
        let raw = lines.iter().position(|line| *line == "[raw]").expect("the originals' layer");
        assert!(wiki < raw, "{}", report.text);
        assert_eq!(lines[raw + 1], "2. raw/call-notes.txt — call-notes", "{}", report.text);
        assert_eq!(lines[raw + 2], "   > the hook fired twice on the bridge", "{}", report.text);
        let original = vault.path().join("raw").join("call-notes.txt");
        assert_eq!(lines[raw + 3], format!("   {}:1", original.display()), "{}", report.text);

        let only = run(&args(&["search", "hook", "--layer", "raw", "--json", "--vault", &root]), vault.path()).unwrap();
        let value: Value = serde_json::from_str(&only.text).unwrap();
        assert_eq!(value["found"].as_array().unwrap().len(), 1);
        assert_eq!(value["found"][0]["layer"], "raw");
        assert_eq!(value["pages"], 0, "no page was asked");
        let refused = run(&args(&["search", "hook", "--layer", "nope", "--vault", &root]), vault.path()).unwrap_err();
        assert!(refused.starts_with("no layer named `nope`"), "{refused}");
    }

    /// `source` lists what the search reads, adds a folder outside the vault
    /// under a layer and takes it out again; the search follows.
    #[test]
    fn source_adds_lists_and_removes_a_folder_outside_the_vault() {
        let vault = vault_with(&[("hook-bridge", "# Hook Bridge\n\nThe bridge carries every hook.\n")]);
        let root = vault.path().display().to_string();
        let outside = tempfile::tempdir().expect("outside");
        let reports = outside.path().join("reports");
        std::fs::create_dir_all(&reports).unwrap();
        std::fs::write(reports.join("week.md"), "The hook bridge passed its soak test.\n").unwrap();

        assert_eq!(parse(&args(&["source", "list"])).unwrap().from, SOURCE_LIST);
        assert!(parse(&args(&["source"])).unwrap_err().starts_with("`zo vault source` takes"));
        assert!(parse(&args(&["source", "add"])).unwrap_err().starts_with("`zo vault source` takes"));
        assert!(parse(&args(&["source", "drop", "x"])).unwrap_err().starts_with("`zo vault source` takes"));

        let listed = run(&args(&["source", "list", "--vault", &root]), vault.path()).unwrap().text;
        assert_eq!(listed.lines().count(), 2, "{listed}");
        assert!(listed.starts_with("wiki · pages · "), "{listed}");

        let added = run(
            &args(&["source", "add", &reports.display().to_string(), "--layer", "weekly", "--vault", &root]),
            vault.path(),
        )
        .unwrap()
        .text;
        assert!(added.starts_with("added "), "{added}");
        assert_eq!(added.lines().nth(1), Some(format!("note: {}", vault_search::ADDED_FOLDER_NOTICE).as_str()), "{added}");
        assert!(added.lines().last().unwrap().starts_with("weekly · added · "), "{added}");
        // The list says the same wherever an added folder is on it, and in --json.
        let listed = run(&args(&["source", "list", "--vault", &root]), vault.path()).unwrap().text;
        assert_eq!(listed.lines().count(), 4, "{listed}");
        assert!(listed.ends_with(vault_search::ADDED_FOLDER_NOTICE), "{listed}");
        let listed = run(&args(&["source", "list", "--json", "--vault", &root]), vault.path()).unwrap().text;
        let value: Value = serde_json::from_str(&listed).unwrap();
        assert_eq!(value["notice"], vault_search::ADDED_FOLDER_NOTICE);
        let found = run(&args(&["search", "soak", "test", "--json", "--vault", &root]), vault.path()).unwrap();
        let value: Value = serde_json::from_str(&found.text).unwrap();
        assert_eq!(value["found"][0]["slug"], "weekly/week.md");
        assert_eq!(value["found"][0]["layer"], "weekly");

        let removed = run(&args(&["source", "remove", "weekly", "--vault", &root]), vault.path()).unwrap().text;
        assert!(removed.starts_with("removed "), "{removed}");
        assert_eq!(removed.lines().count(), 3, "{removed}");
        assert!(!removed.contains("note:"), "no added folder is left to speak of: {removed}");
        let found = run(&args(&["search", "soak", "test", "--json", "--vault", &root]), vault.path()).unwrap();
        let value: Value = serde_json::from_str(&found.text).unwrap();
        assert!(value["found"].as_array().unwrap().is_empty(), "{}", found.text);
        assert!(reports.join("week.md").is_file(), "the folder is the person's");
    }

    /// Words no page holds get a sentence saying so and what to try, not an
    /// empty list an agent could read as "searched and fine".
    #[test]
    fn search_says_when_nothing_answers_and_what_to_try() {
        let vault = vault_with(&[("cooking", "# Cooking\n\nUnrelated.\n")]);
        let root = vault.path().display().to_string();
        let report = run(&args(&["search", "parser", "--vault", &root]), vault.path()).unwrap();
        assert!(report.text.starts_with("Nothing in "), "{}", report.text);
        assert!(report.text.contains("Try a page's own words"), "{}", report.text);
    }
}
