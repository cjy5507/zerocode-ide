//! The literal gate of the catalog de-hardcoding plan: a version-pinned model
//! id (`gpt-5.6-sol`, `claude-opus-4-8`, `gemini-3.7-flash`, `o3`, …) may live
//! in the shipped catalog, in tests, and in the one grammar seed that turns
//! an id into a provider — nowhere else in non-test source. Everywhere else
//! a model is named by a family alias (`fable`, `openai-latest`) or a role,
//! so a new release is a catalog edit and never a rebuild.

use std::path::{Path, PathBuf};

/// The crates whose non-test source is scanned, relative to this crate.
const SCANNED: [&str; 4] = ["../api/src", "../runtime/src", "../tools/src", "src"];

/// Files that may spell versioned ids: the grammar seed that recognises an
/// OpenAI lineup word (`gpt`, `o1`, `o3`, `o4`, `codex`) before any catalog
/// row exists for it. Everything else reads the catalog.
const ALLOWED: [&str; 1] = ["../api/src/providers/mod.rs"];

/// The lineage and lineup words a policy must not branch on: which family a
/// model belongs to, and what that family can do, are catalog facts
/// (`class`, `capabilities`, `demotes_to`, `refusal_fallback`, `priors`).
/// The routing review's §1.6 named three branches that did — the fan-out
/// decomposition model, the cold-start specialty seed and the refusal
/// fallback's eligibility — and P4 moved each into the catalog.
const FAMILY_WORDS: [&str; 12] = [
    "claude", "fable", "mythos", "opus", "sonnet", "haiku", "gpt", "codex", "gemini", "deepseek",
    "grok", "spark",
];

/// The crates whose non-test source must not branch on a family word. `api`
/// is out: it owns the grammar that turns an id into a provider and a family.
const FAMILY_SCANNED: [&str; 3] = ["../runtime/src", "../tools/src", "src"];

/// The grammar seeds outside `api`, and one provider policy table, that may
/// still name a family word:
/// - `model_inventory.rs` derives family/class labels from an id when no
///   catalog row declares them (the fallback grammar);
/// - `model_catalog.rs` spells each provider's family key;
/// - `runtime_support.rs` maps a typed model word to its provider before the
///   catalog is consulted;
/// - `conversation/api.rs` compares a provider KEY, not a model;
/// - `conversation/compaction.rs` keeps the per-provider context policy
///   (compaction thresholds) — the next table to move into the catalog.
const FAMILY_ALLOWED: [&str; 5] = [
    "../runtime/src/model_inventory.rs",
    "../runtime/src/model_catalog.rs",
    "../runtime/src/conversation/api.rs",
    "../runtime/src/conversation/compaction.rs",
    "src/runtime_support.rs",
];

/// The quoted string literals on one source line, without their quotes.
fn string_literals(line: &str) -> impl Iterator<Item = &str> {
    line.split('"').skip(1).step_by(2)
}

/// Whether a literal spells a version-pinned model id: a lineup word followed
/// by a digit (`gpt-5.6-sol`, `claude-opus-4-8`, `gemini-3.7-flash`,
/// `grok-4-fast`) or an OpenAI o-series name (`o1`, `o3-mini`, `o4`).
/// A family alias (`fable`, `openai-latest`, `gemini-flash`, `gpt`) has no
/// version digit and passes.
fn names_versioned_id(literal: &str) -> bool {
    literal
        .split(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '-' | '.')))
        .any(|word| {
            let word = word.to_ascii_lowercase();
            let after = |prefix: &str| word.strip_prefix(prefix).map(|rest| rest.trim_start_matches('-').to_string());
            let starts_with_digit = |rest: &str| rest.chars().next().is_some_and(|c| c.is_ascii_digit());
            let versioned_family = ["gpt", "gemini", "grok"]
                .iter()
                .any(|lineup| after(lineup).is_some_and(|rest| starts_with_digit(&rest)));
            let versioned_claude = ["claude-opus", "claude-sonnet", "claude-haiku", "claude-fable"]
                .iter()
                .any(|family| after(family).is_some_and(|rest| starts_with_digit(&rest)));
            let o_series = matches!(word.as_str(), "o1" | "o3" | "o4")
                || ["o1-", "o3-", "o4-"].iter().any(|prefix| word.starts_with(prefix));
            versioned_family || versioned_claude || o_series
        })
}

/// Whether a line branches on a family word: compares, prefix/suffix/substring
/// tests or a bare `"word".to_string()` against one of [`FAMILY_WORDS`]. A
/// family word inside a longer literal (`"claude-fable-5"`, a log line) is
/// not a branch and passes; the versioned-id gate above covers ids.
fn branches_on_family_word(line: &str) -> Option<&'static str> {
    FAMILY_WORDS.iter().copied().find(|word| {
        let quoted = format!("\"{word}\"");
        ["contains(", "starts_with(", "ends_with(", "eq_ignore_ascii_case(", "== ", "!= ", "=> "]
            .iter()
            .any(|op| line.contains(&format!("{op}{quoted}")))
            || line.contains(&format!("{quoted}.to_string()"))
    })
}

fn is_test_source(path: &Path) -> bool {
    // `tests.rs`, anything under a `tests` directory, and a `*_tests.rs`
    // module file — the crates' convention for a test module too large for
    // the bottom of its file (`roads_tests.rs`, `label_audit_tests.rs`),
    // declared under `#[cfg(test)]` beside `tests.rs`.
    path.file_name().is_some_and(|name| {
        name == "tests.rs" || name.to_str().is_some_and(|name| name.ends_with("_tests.rs"))
    }) || path.components().any(|part| part.as_os_str() == "tests")
}

/// Source lines before the file's first `#[cfg(test)]` — test modules sit at
/// the bottom by convention, and a literal in a test is allowed anyway.
fn non_test_lines(source: &str) -> impl Iterator<Item = (usize, &str)> {
    source
        .lines()
        .enumerate()
        .take_while(|(_, line)| !line.trim_start().starts_with("#[cfg(test)]"))
        .filter(|(_, line)| {
            let trimmed = line.trim_start();
            !(trimmed.starts_with("//") || trimmed.starts_with('*'))
        })
        .map(|(index, line)| (index + 1, line))
}

fn rust_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|ext| ext == "rs") && !is_test_source(&path) {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

#[test]
fn non_test_source_names_no_versioned_model_id() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let allowed: Vec<PathBuf> = ALLOWED.iter().map(|rel| manifest.join(rel)).collect();
    let mut offenders = Vec::new();
    for root in SCANNED {
        for file in rust_files(&manifest.join(root)) {
            if allowed.iter().any(|allow| same_file(allow, &file)) {
                continue;
            }
            let source = std::fs::read_to_string(&file).expect("read source");
            for (line_no, line) in non_test_lines(&source) {
                if let Some(hit) = string_literals(line).find(|literal| names_versioned_id(literal)) {
                    offenders.push(format!(
                        "{}:{line_no}: \"{hit}\"",
                        file.strip_prefix(manifest).unwrap_or(&file).display(),
                    ));
                }
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "version-pinned model ids outside the catalog, tests, and the grammar seed \
         (name a family alias or a role, or add a catalog row):\n  {}",
        offenders.join("\n  ")
    );
}

/// Non-test source outside the grammar seeds branches on no family word.
#[test]
fn non_test_source_outside_the_grammar_branches_on_no_family_word() {
    let here = Path::new(env!("CARGO_MANIFEST_DIR"));
    let allowed: Vec<PathBuf> = FAMILY_ALLOWED.iter().map(|rel| here.join(rel)).collect();
    let mut offenders = Vec::new();
    for root in FAMILY_SCANNED {
        for path in rust_files(&here.join(root)) {
            if allowed.iter().any(|allow| same_file(allow, &path)) {
                continue;
            }
            let source = std::fs::read_to_string(&path).expect("read source");
            for (line_no, line) in non_test_lines(&source) {
                if let Some(word) = branches_on_family_word(line) {
                    offenders.push(format!("{}:{line_no}: branches on \"{word}\" — {}", path.display(), line.trim()));
                }
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "a policy branches on a family word; which family a model is and what it can do are catalog facts:\n{}",
        offenders.join("\n")
    );
}

fn same_file(left: &Path, right: &Path) -> bool {
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => left == right,
    }
}

/// The gate must see through the shapes it is meant to catch, and only those.
#[test]
fn the_family_gate_matches_branches_and_not_longer_literals() {
    assert_eq!(branches_on_family_word(r#"lower.contains("fable") || x"#), Some("fable"));
    assert_eq!(branches_on_family_word(r#"if raw == "opus" {"#), Some("opus"));
    assert_eq!(branches_on_family_word(r#"None => "haiku".to_string(),"#), Some("haiku"));
    assert_eq!(branches_on_family_word(r#"Self::Anthropic => "claude","#), Some("claude"));
    assert_eq!(branches_on_family_word(r#"set_context_model("claude-fable-5")"#), None);
    assert_eq!(branches_on_family_word(r#"let words = ["gpt", "claude"];"#), None);
    assert_eq!(branches_on_family_word(r#"eprintln!("fable refused")"#), None);
}

#[test]
fn the_gate_matches_versioned_ids_and_not_family_aliases() {
    for versioned in [
        r#""gpt-5.6-sol""#,
        r#""claude-opus-4-8""#,
        r#""gemini-3.7-flash""#,
        r#"model.starts_with("o3")"#,
        r#""grok-4-fast""#,
        r#"["gpt-5.5-fast", "x"]"#,
        r#""o1-preview""#,
        r#""openai/gpt-5""#,
    ] {
        assert!(
            string_literals(versioned).any(names_versioned_id),
            "{versioned} names a versioned id"
        );
    }
    for alias in [
        r#""fable""#,
        r#""openai-latest""#,
        r#""gemini-flash""#,
        r#""opus[1m]""#,
        r#""gpt""#,
        r#""codex""#,
        r#""claude-haiku""#,
        r#""ollama""#,
        r#""oz-not-a-model""#,
        "let x = 1; // gpt-5.6-sol in a trailing comment is not a literal",
    ] {
        assert!(
            !string_literals(alias).any(names_versioned_id),
            "{alias} is a family alias, not a version"
        );
    }
}
