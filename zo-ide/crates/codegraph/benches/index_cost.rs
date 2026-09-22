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
//! zo tools make — so one harness measures any index behind that API.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use codegraph::{CodeGraph, CodeGraphError, SymbolKind, DEFAULT_CACHE_FILE_NAME};
use serde_json::{json, Value};

/// Timed repetitions of one query when the driver names none.
const DEFAULT_REPETITIONS: usize = 20;
/// Saves the edit phase times when the driver names none.
const DEFAULT_EDIT_SAMPLES: usize = 5;
/// The definition the edit phase appends, so the refresh it times is proven
/// to have indexed the save. Rust syntax: the phase refuses other files.
const EDIT_PROBE_PREFIX: &str = "codegraph_bench_probe_";
const EDIT_PROBE_EXTENSION: &str = "rs";
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
        other => Err(format!(
            "unknown phase `{other}`; expected build, load, query or edit"
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
        let phase = phase.ok_or("name a phase: build, load, query or edit")?;
        Ok(Self { phase, values })
    }

    fn path(&self, key: &str) -> Result<PathBuf, String> {
        self.values
            .get(key)
            .map(PathBuf::from)
            .ok_or_else(|| format!("`--{key}` is required"))
    }

    fn count(&self, key: &str, default: usize) -> Result<usize, String> {
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
    Ok(json!({
        "phase": "load",
        "load_ms": load_ms,
        "resident_mb": resident_after_load,
        "first_query_ms": first_query_ms,
        "first_query_matches": matches,
        "resident_after_query_mb": resident_mb(),
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
    let mut refresh_samples = Vec::new();
    for _ in 0..repetitions {
        let started = Instant::now();
        graph.refresh().map_err(describe)?;
        refresh_samples.push(elapsed_ms(started));
    }
    Ok(json!({
        "phase": "query",
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
