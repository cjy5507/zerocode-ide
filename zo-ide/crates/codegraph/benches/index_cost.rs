//! What the codegraph index costs on a real workspace (t-5970): the first
//! build, the cache on disk, a load and what it keeps resident, one saved
//! file, and the queries the zo tools make.
//!
//! One phase per process, so a load's resident size is the load's and not the
//! build's. `tools/codegraph-bench/run.py` drives the phases against a
//! snapshot of a checkout and prints the table; one phase by hand:
//!
//! ```text
//! cargo bench -p codegraph --bench index_cost -- build --workspace <dir> --cache-dir <dir>
//! ```
//!
//! Every phase goes through the crate's public API only — the same calls the
//! zo tools make — so one harness measures any index behind that API. Two
//! phases only dump what the index answers (`links`, `sample`), for
//! `tools/codegraph-bench/truth.py` to hold against rust-analyzer's LSIF.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use codegraph::{CodeGraph, CodeGraphError, Symbol, SymbolKind, DEFAULT_CACHE_FILE_NAME};
use serde_json::{json, Value};

/// Timed repetitions of one query when the driver names none.
const DEFAULT_REPETITIONS: usize = 20;
/// Saves the edit phase times when the driver names none.
const DEFAULT_EDIT_SAMPLES: usize = 5;
/// The definition the edit phase appends, so the refresh it times is proven
/// to have indexed the save. Rust syntax: the phase refuses other files.
const EDIT_PROBE_PREFIX: &str = "codegraph_bench_probe_";
const EDIT_PROBE_EXTENSION: &str = "rs";
/// Definitions the `sample` phase draws when the driver names no count.
const DEFAULT_SAMPLE_SIZE: usize = 100;
/// Seed for the `sample` phase's draw; any fixed value makes it repeatable.
const DEFAULT_SAMPLE_SEED: u64 = 5_970;
const MILLIS_PER_SECOND: f64 = 1_000.0;
const KIB_PER_MIB: f64 = 1_024.0;
const BYTES_PER_MIB: f64 = 1_024.0 * 1_024.0;

type PhaseResult = Result<Value, String>;

fn main() {
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    let outcome = Options::parse(&arguments).and_then(|options| match options.phase.as_str() {
        "build" => build(&options),
        "load" => load(&options),
        "query" => query(&options),
        "edit" => edit(&options),
        "links" => links(&options),
        "sample" => sample(&options),
        other => Err(format!(
            "unknown phase `{other}`; expected build, load, query, edit, links or sample"
        )),
    });
    match outcome {
        Ok(report) => println!("{report}"),
        Err(message) => {
            eprintln!("index_cost: {message}");
            std::process::exit(2);
        }
    }
}

struct Options {
    phase: String,
    values: BTreeMap<String, String>,
}

impl Options {
    fn parse(arguments: &[String]) -> Result<Self, String> {
        let mut phase = None;
        let mut values = BTreeMap::new();
        let mut rest = arguments.iter();
        while let Some(argument) = rest.next() {
            // `cargo bench` appends `--bench` to every harness-less target.
            if argument == "--bench" {
                continue;
            }
            if let Some(key) = argument.strip_prefix("--") {
                let value = rest
                    .next()
                    .ok_or_else(|| format!("`--{key}` needs a value"))?;
                values.insert(key.to_string(), value.clone());
            } else if phase.is_none() {
                phase = Some(argument.clone());
            } else {
                return Err(format!("unexpected argument `{argument}`"));
            }
        }
        let phase = phase.ok_or("name a phase: build, load, query, edit, links or sample")?;
        Ok(Self { phase, values })
    }

    fn path(&self, key: &str) -> Result<PathBuf, String> {
        self.values
            .get(key)
            .map(PathBuf::from)
            .ok_or_else(|| format!("`--{key}` is required"))
    }

    fn count(&self, key: &str, default: usize) -> Result<usize, String> {
        self.number(key, default)
    }

    fn number<T>(&self, key: &str, default: T) -> Result<T, String>
    where
        T: std::str::FromStr,
        T::Err: std::fmt::Display,
    {
        self.values.get(key).map_or(Ok(default), |value| {
            value
                .parse()
                .map_err(|error| format!("`--{key} {value}`: {error}"))
        })
    }

    fn names(&self, key: &str) -> Result<Vec<String>, String> {
        let names = self
            .values
            .get(key)
            .map(|value| {
                value
                    .split(',')
                    .map(str::trim)
                    .filter(|name| !name.is_empty())
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if names.is_empty() {
            return Err(format!("`--{key}` must name at least one identifier"));
        }
        Ok(names)
    }

    fn workspace(&self) -> Result<PathBuf, String> {
        self.path("workspace")
    }

    fn cache(&self) -> Result<(PathBuf, PathBuf), String> {
        let directory = self.path("cache-dir")?;
        let file = directory.join(DEFAULT_CACHE_FILE_NAME);
        Ok((directory, file))
    }
}

/// The first index of a workspace: the driver hands over an empty cache
/// directory, so this is the cost a fresh project pays.
fn build(options: &Options) -> PhaseResult {
    let workspace = options.workspace()?;
    let (directory, cache) = options.cache()?;
    let started = Instant::now();
    let graph = CodeGraph::load_or_build(&workspace, &cache).map_err(describe)?;
    let build_ms = elapsed_ms(started);
    let status = graph.status();
    Ok(json!({
        "phase": "build",
        "build_ms": build_ms,
        "indexed_files": status.indexed_files,
        "skipped_files": status.skipped_files,
        "cache_mb": mebibytes(directory_bytes(&directory)),
        "resident_mb": resident_mb(),
    }))
}

/// A session's first touch of an existing cache: the load, what it keeps
/// resident, and the first answer a tool call would wait for.
fn load(options: &Options) -> PhaseResult {
    let workspace = options.workspace()?;
    let (_, cache) = options.cache()?;
    let names = options.names("references")?;
    let started = Instant::now();
    let mut graph = CodeGraph::load_or_build(&workspace, &cache).map_err(describe)?;
    let load_ms = elapsed_ms(started);
    let resident_after_load = resident_mb();
    let started = Instant::now();
    let matches = graph.find_references(&names[0]).map_err(describe)?.len();
    let first_query_ms = elapsed_ms(started);
    let resident_after_query = resident_mb();
    drop(graph);
    // What a file read pays for an index it did not open: open what is on
    // disk, never build.
    let started = Instant::now();
    let opened = CodeGraph::open_existing(&workspace, &cache)
        .map_err(describe)?
        .is_some();
    let open_existing_ms = elapsed_ms(started);
    Ok(json!({
        "phase": "load",
        "load_ms": load_ms,
        "resident_mb": resident_after_load,
        "first_query_ms": first_query_ms,
        "first_query_matches": matches,
        "resident_after_query_mb": resident_after_query,
        "open_existing_ms": open_existing_ms,
        "open_existing_found": opened,
    }))
}

/// The three tool queries, repeated on a loaded index. Each call pays what a
/// tool call pays — its own freshness check included.
fn query(options: &Options) -> PhaseResult {
    let workspace = options.workspace()?;
    let (_, cache) = options.cache()?;
    let reference_names = options.names("references")?;
    let symbol_names = options.names("symbols")?;
    let outline_file = options.path("outline")?;
    let repetitions = options.count("repetitions", DEFAULT_REPETITIONS)?;
    let mut graph = CodeGraph::load_or_build(&workspace, &cache).map_err(describe)?;
    graph.refresh().map_err(describe)?;

    let mut reference_samples = Vec::new();
    let mut reference_counts = BTreeMap::new();
    for name in &reference_names {
        for _ in 0..repetitions {
            let started = Instant::now();
            let found = graph.find_references(name).map_err(describe)?.len();
            reference_samples.push(elapsed_ms(started));
            reference_counts.insert(name.clone(), found);
        }
    }
    let mut symbol_samples = Vec::new();
    let mut symbol_counts = BTreeMap::new();
    for name in &symbol_names {
        for _ in 0..repetitions {
            let started = Instant::now();
            let found = graph.find_symbols(name, None).map_err(describe)?.len();
            symbol_samples.push(elapsed_ms(started));
            symbol_counts.insert(name.clone(), found);
        }
    }
    let mut outline_samples = Vec::new();
    let mut outline_definitions = 0;
    for _ in 0..repetitions {
        let started = Instant::now();
        outline_definitions = graph
            .file_outline(&outline_file)
            .map_err(describe)?
            .map_or(0, |symbols| symbols.len());
        outline_samples.push(elapsed_ms(started));
    }
    let definition_queries =
        definition_queries(&mut graph, &outline_file, &symbol_names, repetitions)?;
    // The neighbour list's question, every link kept: the cost does not
    // depend on how many the caller shows.
    let mut links_samples = Vec::new();
    let mut links_found = (0, 0);
    for _ in 0..repetitions {
        let started = Instant::now();
        let links = graph
            .file_links(&outline_file, usize::MAX)
            .map_err(describe)?
            .unwrap_or_default();
        links_samples.push(elapsed_ms(started));
        links_found = (links.uses.len(), links.used_by.len());
    }
    let mut refresh_samples = Vec::new();
    for _ in 0..repetitions {
        let started = Instant::now();
        graph.refresh().map_err(describe)?;
        refresh_samples.push(elapsed_ms(started));
    }
    Ok(json!({
        "phase": "query",
        "definition_queries": definition_queries,
        "file_links_ms": percentiles(&mut links_samples),
        "file_links_found": { "uses": links_found.0, "used_by": links_found.1 },
        "repetitions": repetitions,
        "find_references_ms": percentiles(&mut reference_samples),
        "find_references_matches": reference_counts,
        "find_symbol_ms": percentiles(&mut symbol_samples),
        "find_symbol_matches": symbol_counts,
        "file_outline_ms": percentiles(&mut outline_samples),
        "file_outline_definitions": outline_definitions,
        "unchanged_refresh_ms": percentiles(&mut refresh_samples),
        "resident_mb": resident_mb(),
    }))
}

/// One file saved under a loaded index: each sample appends a definition,
/// times the refresh that follows, and proves the refresh saw it. The file is
/// put back afterwards so the next run starts from the same bytes. Then the
/// same for a file created beside it and removed again — the change an agent
/// makes as often as a save, and the one a whole-index cache pays most for.
fn edit(options: &Options) -> PhaseResult {
    let workspace = options.workspace()?;
    let (_, cache) = options.cache()?;
    let relative = options.path("file")?;
    let samples = options.count("samples", DEFAULT_EDIT_SAMPLES)?;
    if relative.extension().and_then(|extension| extension.to_str()) != Some(EDIT_PROBE_EXTENSION)
    {
        return Err(format!(
            "`--file` must be a .{EDIT_PROBE_EXTENSION} file: the probe is a Rust definition"
        ));
    }
    let path = workspace.join(&relative);
    let original = fs::read(&path).map_err(|error| format!("{}: {error}", path.display()))?;
    let mut graph = CodeGraph::load_or_build(&workspace, &cache).map_err(describe)?;
    graph.refresh().map_err(describe)?;

    let mut refresh_samples = Vec::new();
    let mut query_samples = Vec::new();
    for sample in 0..samples {
        let probe = format!("{EDIT_PROBE_PREFIX}{sample}");
        let mut edited = original.clone();
        edited.extend_from_slice(format!("\nfn {probe}() {{}}\n").as_bytes());
        fs::write(&path, &edited).map_err(|error| format!("{}: {error}", path.display()))?;
        let started = Instant::now();
        graph.refresh().map_err(describe)?;
        refresh_samples.push(elapsed_ms(started));
        let started = Instant::now();
        let seen = count_functions(&mut graph, &probe)?;
        query_samples.push(elapsed_ms(started));
        if seen != 1 {
            return Err(format!(
                "the refresh after save {sample} indexed {seen} `{probe}` definitions, not 1"
            ));
        }
    }
    fs::write(&path, &original).map_err(|error| format!("{}: {error}", path.display()))?;
    graph.refresh().map_err(describe)?;

    let created = path.with_file_name(format!("{EDIT_PROBE_PREFIX}file.{EDIT_PROBE_EXTENSION}"));
    let mut create_samples = Vec::new();
    let mut delete_samples = Vec::new();
    for sample in 0..samples {
        let probe = format!("{EDIT_PROBE_PREFIX}created_{sample}");
        fs::write(&created, format!("fn {probe}() {{}}\n"))
            .map_err(|error| format!("{}: {error}", created.display()))?;
        let started = Instant::now();
        graph.refresh().map_err(describe)?;
        create_samples.push(elapsed_ms(started));
        let seen = count_functions(&mut graph, &probe)?;
        if seen != 1 {
            return Err(format!("the refresh after creating a file indexed {seen} `{probe}`"));
        }
        fs::remove_file(&created).map_err(|error| format!("{}: {error}", created.display()))?;
        let started = Instant::now();
        graph.refresh().map_err(describe)?;
        delete_samples.push(elapsed_ms(started));
        let seen = count_functions(&mut graph, &probe)?;
        if seen != 0 {
            return Err(format!("the refresh after removing a file kept {seen} `{probe}`"));
        }
    }
    Ok(json!({
        "phase": "edit",
        "file": relative,
        "file_bytes": original.len(),
        "refresh_after_save_ms": percentiles(&mut refresh_samples),
        "query_after_refresh_ms": percentiles(&mut query_samples),
        "refresh_after_create_ms": percentiles(&mut create_samples),
        "refresh_after_delete_ms": percentiles(&mut delete_samples),
        "resident_mb": resident_mb(),
    }))
}

fn count_functions(graph: &mut CodeGraph, name: &str) -> Result<usize, String> {
    graph
        .find_symbols(name, Some(SymbolKind::Function))
        .map(|symbols| symbols.len())
        .map_err(describe)
}

/// The two questions asked of one definition — `find_references` narrowed
/// to it and `impact` — for each of `names` that `file` defines.
fn definition_queries(
    graph: &mut CodeGraph,
    file: &Path,
    names: &[String],
    repetitions: usize,
) -> PhaseResult {
    // `find_references` narrowed to a definition: the symbol names the
    // outline file itself defines.
    let mut narrowed_samples = Vec::new();
    let mut narrowed_counts = BTreeMap::new();
    for name in names {
        for _ in 0..repetitions {
            let started = Instant::now();
            let Some(found) = graph
                .references_to(file, name)
                .map_err(describe)?
            else {
                break;
            };
            narrowed_samples.push(elapsed_ms(started));
            narrowed_counts.insert(name.clone(), found.len());
        }
    }
    // The `impact` tool's question for the same names.
    let mut impact_samples = Vec::new();
    let mut impact_found = BTreeMap::new();
    for name in names {
        for _ in 0..repetitions {
            let started = Instant::now();
            let Some(impact) = graph.impact(file, name).map_err(describe)? else {
                break;
            };
            impact_samples.push(elapsed_ms(started));
            impact_found.insert(
                name.clone(),
                json!({
                    "references": impact.references,
                    "callers": impact.callers(),
                    "tests": impact.tests(),
                }),
            );
        }
    }
    Ok(json!({
        "references_to_ms": percentiles(&mut narrowed_samples),
        "references_to_matches": narrowed_counts,
        "impact_ms": percentiles(&mut impact_samples),
        "impact_found": impact_found,
    }))
}

/// Every indexed file's links, every link kept — the neighbour list's source,
/// dumped for the truth script to check file by file.
fn links(options: &Options) -> PhaseResult {
    let workspace = options.workspace()?;
    let (_, cache) = options.cache()?;
    let mut graph = CodeGraph::load_or_build(&workspace, &cache).map_err(describe)?;
    graph.refresh().map_err(describe)?;
    let mut files = Vec::new();
    for file in graph.indexed_files() {
        if let Some(links) = graph.file_links(&file, usize::MAX).map_err(describe)? {
            files.push(json!({
                "file": file,
                "uses": links.uses,
                "used_by": links.used_by,
            }));
        }
    }
    Ok(json!({ "phase": "links", "files": files }))
}

/// A seeded draw of definitions under `--under` in `.rs` files, each with
/// every occurrence of its name `find_references` answers — the exact-name
/// answer whose precision the truth script measures. Each occurrence says
/// whether its file's imports spell the name, and each definition how many
/// files define its name: the two cheap filters that precision is weighed
/// against.
fn sample(options: &Options) -> PhaseResult {
    let workspace = options.workspace()?;
    let (_, cache) = options.cache()?;
    let under = options.path("under")?;
    let count = options.count("count", DEFAULT_SAMPLE_SIZE)?;
    let seed = options.number("seed", DEFAULT_SAMPLE_SEED)?;
    let mut graph = CodeGraph::load_or_build(&workspace, &cache).map_err(describe)?;
    graph.refresh().map_err(describe)?;
    let mut definitions = Vec::<Symbol>::new();
    for file in graph.indexed_files() {
        let rust = file.extension().and_then(|extension| extension.to_str())
            == Some(EDIT_PROBE_EXTENSION);
        if rust && file.starts_with(&under) {
            let outline = graph.file_outline(&file).map_err(describe)?;
            definitions.extend(outline.unwrap_or_default());
        }
    }
    let population = definitions.len();
    let drawn = draw(&mut definitions, count, seed);
    // Each file's imports, read once: every `file_imports` call pays the
    // freshness walk, and the heaviest names occur in most files.
    let mut imports = BTreeMap::<PathBuf, Vec<codegraph::Import>>::new();
    let mut samples = Vec::new();
    for definition in drawn {
        let definers = graph
            .find_symbols(&definition.name, None)
            .map_err(describe)?
            .iter()
            .map(|symbol| symbol.file.clone())
            .collect::<std::collections::BTreeSet<_>>()
            .len();
        let mut occurrences = Vec::new();
        for reference in graph.find_references(&definition.name).map_err(describe)? {
            if !imports.contains_key(&reference.file) {
                let read = graph.file_imports(&reference.file).map_err(describe)?;
                imports.insert(reference.file.clone(), read.unwrap_or_default());
            }
            let spelled_by = |name: &str| {
                imports[&reference.file]
                    .iter()
                    .any(|import| import.spells(name))
            };
            occurrences.push(json!({
                "file": reference.file,
                "row": reference.range.start.row,
                "column": reference.range.start.column,
                "imports_spell": spelled_by(&definition.name),
                "imports_spell_container": definition.container.as_deref().is_some_and(spelled_by),
            }));
        }
        samples.push(json!({
            "definition": definition,
            "definers": definers,
            "occurrences": occurrences,
        }));
    }
    Ok(json!({
        "phase": "sample",
        "population": population,
        "seed": seed,
        "samples": samples,
    }))
}

/// `count` items drawn without replacement by a seeded xorshift64* — the
/// same draw for the same seed and the same population order.
fn draw<T>(items: &mut [T], count: usize, seed: u64) -> Vec<T>
where
    T: Clone,
{
    let mut state = seed.max(1);
    let mut next = || {
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        state.wrapping_mul(0x2545_f491_4f6c_dd1d)
    };
    let count = count.min(items.len());
    for position in 0..count {
        let remaining = u64::try_from(items.len() - position).unwrap_or(u64::MAX);
        let offset = usize::try_from(next() % remaining).unwrap_or_default();
        items.swap(position, position + offset);
    }
    items[..count].to_vec()
}

#[allow(clippy::needless_pass_by_value)] // the shape `map_err` hands over
fn describe(error: CodeGraphError) -> String {
    error.to_string()
}

fn elapsed_ms(started: Instant) -> f64 {
    started.elapsed().as_secs_f64() * MILLIS_PER_SECOND
}

/// Nearest-rank p50/p95 and the extremes, in milliseconds.
fn percentiles(samples: &mut [f64]) -> Value {
    if samples.is_empty() {
        return Value::Null;
    }
    samples.sort_by(f64::total_cmp);
    let rank = |percent: usize| {
        let position = (percent * samples.len()).div_ceil(100);
        samples[position.clamp(1, samples.len()) - 1]
    };
    json!({
        "p50": rank(50),
        "p95": rank(95),
        "min": samples[0],
        "max": samples[samples.len() - 1],
        "samples": samples.len(),
    })
}

#[allow(clippy::cast_precision_loss)] // a table cell: 52 bits of mantissa is 4 PiB
fn mebibytes(bytes: u64) -> f64 {
    bytes as f64 / BYTES_PER_MIB
}

/// Bytes of every file directly in `directory` — the cache file and whatever
/// sidecars its format keeps beside it.
fn directory_bytes(directory: &Path) -> u64 {
    fs::read_dir(directory)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|entry| entry.metadata().ok())
        .filter(fs::Metadata::is_file)
        .map(|metadata| metadata.len())
        .sum()
}

/// This process's resident set, as `ps` reports it; `None` where `ps` is not
/// there to ask.
fn resident_mb() -> Option<f64> {
    let output = Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .ok()?;
    let kib = String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse::<f64>()
        .ok()?;
    Some(kib / KIB_PER_MIB)
}
