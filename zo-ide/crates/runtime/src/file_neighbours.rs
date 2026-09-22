//! What sits next to a file the model just read: the files it imports, the
//! module that declares it and the ones it declares, and its tests — resolved
//! from the file's own text and a few `stat` calls, then completed from the
//! codegraph index when one is already open or on disk.
//!
//! r40 counted 83% of tool batches carrying a single call, and the prompt's
//! "read the files you can already predict together" changing nothing. The
//! model reads one file at a time because it does not know what the next one
//! is until it has read this one; the neighbour list answers that on the
//! result itself, so the batch that follows can be wide.
//!
//! Cheap by construction: the first `IMPORT_SCAN_LINES` lines are scanned
//! for import syntax, each candidate costs a handful of existence checks, and
//! the list is capped at [`MAX_NEIGHBOURS`]. Resolution is heuristic but
//! existence-checked: a path is listed only when the file is really there.
//!
//! The index adds what the text cannot resolve ([`with_index_links`], fed by
//! the tools crate): the file defining a name this one imports, and the
//! tests importing a name this one defines. It was left out while it was a
//! JSON file — 352 MB on this repository, 0.94 s and 791 MB resident to load
//! — and is consulted now that it is `SQLite`: opening an index already on
//! disk costs ~2 ms once per session and one file's links ~0.9 ms (p50, a
//! 1.7k-line file; t-5970). A read never builds, walks or waits for it.
//! Against rust-analyzer on 706 Rust files here, the index took the imports
//! listed from 1,049 to 2,192 at 96.7% precision (the text alone: 100%), the
//! real dependencies found from 17.8% to 36.0%, and the real tests listed
//! from 122 to 412 (precision 61.6% → 83.4%).

use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Ceiling on the whole list; one line of the result, not a second file.
pub const MAX_NEIGHBOURS: usize = 12;
/// Per-relation ceiling, so a file with forty imports still shows its tests.
const MAX_PER_RELATION: usize = 8;
/// Imports live at the top; scanning further pays for nothing.
const IMPORT_SCAN_LINES: usize = 200;
/// How far above a crate's `src` the workspace's other crates are looked for.
const WORKSPACE_SEARCH_DEPTH: usize = 6;

const SCRIPT_EXTENSIONS: &[&str] = &["ts", "tsx", "js", "jsx", "mjs", "cjs"];
/// Rust crates that are never workspace members.
const RUST_BUILTIN_CRATES: &[&str] = &["std", "core", "alloc", "crate", "self", "super"];

/// How a neighbour relates to the file that was read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Relation {
    /// A file this one imports from.
    Imports,
    /// The module file that declares this one, or one this file declares.
    Module,
    /// A test file for this one.
    Tests,
}

impl Relation {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Imports => "imports",
            Self::Module => "module",
            Self::Tests => "tests",
        }
    }
}

/// One file next to the one that was read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Neighbour {
    pub relation: Relation,
    /// Absolute path, as the read's own `filePath` is.
    pub path: String,
}

/// Whether a read of `path` from `offset` carries neighbours: a read from the
/// top of a code file is the moment the model decides what to read next; a
/// window further in is a drill-in that already knows.
#[must_use]
pub fn wants_neighbours(path: &Path, offset: Option<usize>) -> bool {
    offset.unwrap_or(0) == 0 && is_code_path(&path.to_string_lossy())
}

/// Whether `path` names a source file the outline and neighbour passes apply to.
#[must_use]
pub fn is_code_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    [
        ".rs", ".ts", ".tsx", ".js", ".jsx", ".mjs", ".cjs", ".py", ".go", ".java", ".c", ".h",
        ".cpp", ".hpp", ".cs", ".rb", ".swift", ".kt", ".scala", ".php",
    ]
    .iter()
    .any(|ext| lower.ends_with(ext))
}

/// The neighbours of `path`, whose full text is `content`. Empty for a
/// language without rules here and for a file with nothing resolvable.
#[must_use]
pub fn neighbours_of(path: &Path, content: &str) -> Vec<Neighbour> {
    let Some(ext) = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase)
    else {
        return Vec::new();
    };
    let mut found = Found::new(path);
    let head: Vec<&str> = content.lines().take(IMPORT_SCAN_LINES).collect();
    match ext.as_str() {
        "rs" => rust::collect(path, &head, &mut found),
        ext if SCRIPT_EXTENSIONS.contains(&ext) => script::collect(path, &head, &mut found),
        "py" => python::collect(path, &head, &mut found),
        _ => {}
    }
    tests::collect(path, &ext, &mut found);
    found.into_neighbours()
}

/// `neighbours` — the text's own candidates for `path` — followed by what the
/// codegraph index links the file to: the files defining names it uses, as
/// imports, and the test files using names it defines, as tests. The index's
/// candidates come after the text's, under the same caps, and like them are
/// listed only when the file is on disk. Paths are absolute.
#[must_use]
pub fn with_index_links(
    path: &Path,
    neighbours: Vec<Neighbour>,
    uses: &[PathBuf],
    tested_by: &[PathBuf],
) -> Vec<Neighbour> {
    let mut found = Found::new(path);
    for neighbour in neighbours {
        found.keep(neighbour);
    }
    for file in uses {
        found.take(Relation::Imports, file);
    }
    for file in tested_by {
        found.take(Relation::Tests, file);
    }
    found.into_neighbours()
}

/// Accumulates candidates: deduplicated, never the file itself, capped per
/// relation and in total, in insertion order.
struct Found {
    own: PathBuf,
    seen: BTreeSet<PathBuf>,
    per_relation: [usize; 3],
    neighbours: Vec<Neighbour>,
}

impl Found {
    fn new(own: &Path) -> Self {
        Self {
            own: own.to_path_buf(),
            seen: BTreeSet::new(),
            per_relation: [0; 3],
            neighbours: Vec::new(),
        }
    }

    /// Record `candidate` under `relation` when it exists and is new. Returns
    /// whether it was taken, so a resolver can stop at the first hit.
    fn take(&mut self, relation: Relation, candidate: &Path) -> bool {
        let candidate = normalize(candidate);
        let slot = relation as usize;
        if self.per_relation[slot] >= MAX_PER_RELATION
            || self.neighbours.len() >= MAX_NEIGHBOURS
            || candidate == self.own
            || !candidate.is_file()
            || self.seen.contains(&candidate)
        {
            return false;
        }
        self.seen.insert(candidate.clone());
        self.per_relation[slot] += 1;
        self.neighbours.push(Neighbour {
            relation,
            path: candidate.to_string_lossy().into_owned(),
        });
        true
    }

    /// Count a neighbour an earlier pass already checked, without asking the
    /// disk again.
    fn keep(&mut self, neighbour: Neighbour) {
        let slot = neighbour.relation as usize;
        self.seen.insert(PathBuf::from(&neighbour.path));
        self.per_relation[slot] += 1;
        self.neighbours.push(neighbour);
    }

    /// The first existing candidate wins; the rest are not tried.
    fn take_first(&mut self, relation: Relation, candidates: impl IntoIterator<Item = PathBuf>) {
        for candidate in candidates {
            if self.take(relation, &candidate) {
                return;
            }
        }
    }

    fn into_neighbours(self) -> Vec<Neighbour> {
        self.neighbours
    }
}

/// Resolve `.` and `..` lexically, so `src/app/../lib/y` reads as
/// `src/lib/y` in the list and two spellings of one file cannot both appear.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// The text between the first pair of `open`/`close` on `line`, or `None`.
fn between(line: &str, open: char, close: char) -> Option<&str> {
    let start = line.find(open)? + open.len_utf8();
    let end = line[start..].find(close)? + start;
    Some(&line[start..end])
}

fn file_stem(path: &Path) -> Option<&str> {
    path.file_stem().and_then(|stem| stem.to_str())
}

mod rust {
    use std::path::{Path, PathBuf};

    use super::{Found, Relation, RUST_BUILTIN_CRATES, WORKSPACE_SEARCH_DEPTH};

    /// The directory a file's child modules live in: `foo.rs` → `foo/`;
    /// `mod.rs`/`lib.rs`/`main.rs` → their own directory.
    fn module_dir(file: &Path) -> Option<PathBuf> {
        let parent = file.parent()?;
        let stem = super::file_stem(file)?;
        Some(if matches!(stem, "mod" | "lib" | "main") {
            parent.to_path_buf()
        } else {
            parent.join(stem)
        })
    }

    /// The nearest ancestor holding `lib.rs` or `main.rs`: the crate's `src`.
    fn crate_src_root(file: &Path) -> Option<PathBuf> {
        file.ancestors()
            .skip(1)
            .find(|dir| dir.join("lib.rs").is_file() || dir.join("main.rs").is_file())
            .map(Path::to_path_buf)
    }

    /// `a::b::c` under `base`: the longest prefix that is a file, as `x.rs` or
    /// `x/mod.rs`; the trailing segments are items inside it.
    fn resolve_module_path(base: &Path, segments: &[&str]) -> Vec<PathBuf> {
        let mut candidates = Vec::new();
        for take in (1..=segments.len()).rev() {
            let mut path = base.to_path_buf();
            for segment in &segments[..take] {
                path.push(segment);
            }
            candidates.push(path.with_extension("rs"));
            candidates.push(path.join("mod.rs"));
        }
        candidates
    }

    /// Expand one `use` path list into its leaf paths: `a::{b, c::d}` →
    /// `a::b`, `a::c::d`. One level of braces; `self`, `*`, and `as` aliases
    /// resolve to the prefix.
    fn use_paths(spec: &str) -> Vec<Vec<String>> {
        let spec = spec.trim().trim_end_matches(';').trim();
        let (prefix, group) = match spec.find('{') {
            Some(open) => (
                spec[..open].trim().trim_end_matches("::"),
                spec[open + 1..].trim_end_matches('}').trim_end_matches(';'),
            ),
            None => (spec, ""),
        };
        let prefix: Vec<String> = prefix
            .split("::")
            .map(str::trim)
            .filter(|segment| !segment.is_empty())
            .map(str::to_string)
            .collect();
        if group.is_empty() {
            return vec![strip_alias(prefix)];
        }
        group
            .split(',')
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .map(|item| {
                let mut path = prefix.clone();
                let item = item.split(" as ").next().unwrap_or(item).trim();
                path.extend(
                    item.split("::")
                        .map(str::trim)
                        .filter(|segment| !segment.is_empty() && *segment != "self" && *segment != "*")
                        .map(str::to_string),
                );
                path
            })
            .collect()
    }

    fn strip_alias(mut path: Vec<String>) -> Vec<String> {
        if let Some(last) = path.last_mut() {
            if let Some(name) = last.split(" as ").next() {
                *last = name.trim().to_string();
            }
        }
        path.retain(|segment| segment != "self" && segment != "*");
        path
    }

    /// `crates/<name>/src/lib.rs` or `<name>/src/lib.rs` a few levels above
    /// this crate's `src`; `_` and `-` are both tried.
    fn workspace_crate(src_root: &Path, name: &str) -> Vec<PathBuf> {
        let dashed = name.replace('_', "-");
        let mut candidates = Vec::new();
        for ancestor in src_root.ancestors().skip(1).take(WORKSPACE_SEARCH_DEPTH) {
            for name in [name, dashed.as_str()] {
                candidates.push(ancestor.join("crates").join(name).join("src").join("lib.rs"));
                candidates.push(ancestor.join(name).join("src").join("lib.rs"));
            }
        }
        candidates
    }

    pub(super) fn collect(file: &Path, head: &[&str], found: &mut Found) {
        let Some(module_dir) = module_dir(file) else {
            return;
        };
        // The file that declares this module, when this is not a crate root.
        let declared_by = file
            .parent()
            .filter(|_| !matches!(super::file_stem(file), Some("lib" | "main")));
        if let Some(parent) = declared_by {
            let parent_module = if super::file_stem(file) == Some("mod") {
                parent
                    .parent()
                    .map(|grand| {
                        let name = parent.file_name().map(|n| n.to_string_lossy().into_owned());
                        name.map_or(grand.join("mod.rs"), |name| grand.join(format!("{name}.rs")))
                    })
                    .into_iter()
                    .chain(parent.parent().map(|grand| grand.join("mod.rs")))
                    .chain(parent.parent().map(|grand| grand.join("lib.rs")))
                    .collect::<Vec<_>>()
            } else {
                vec![parent.join("mod.rs"), parent.join("lib.rs"), parent.join("main.rs")]
            };
            found.take_first(Relation::Module, parent_module);
        }
        let src_root = crate_src_root(file);
        for line in head {
            let line = line.trim_start();
            let line = line.strip_prefix("pub ").unwrap_or(line);
            let line = line.strip_prefix("pub(crate) ").unwrap_or(line);
            if let Some(rest) = line.strip_prefix("mod ") {
                if let Some(name) = rest.strip_suffix(';') {
                    let name = name.trim();
                    found.take_first(
                        Relation::Module,
                        [module_dir.join(format!("{name}.rs")), module_dir.join(name).join("mod.rs")],
                    );
                }
                continue;
            }
            let Some(spec) = line.strip_prefix("use ") else {
                continue;
            };
            for path in use_paths(spec) {
                let Some((head_segment, rest)) = path.split_first() else {
                    continue;
                };
                let rest: Vec<&str> = rest.iter().map(String::as_str).collect();
                match head_segment.as_str() {
                    "crate" => {
                        if let Some(root) = &src_root {
                            found.take_first(Relation::Imports, resolve_module_path(root, &rest));
                        }
                    }
                    "super" => {
                        // Leading `super`s climb one module each.
                        let climbs = 1 + rest.iter().take_while(|s| **s == "super").count();
                        let remaining: Vec<&str> =
                            rest.iter().copied().filter(|s| *s != "super").collect();
                        let mut base = module_dir.clone();
                        for _ in 0..climbs {
                            if let Some(up) = base.parent() {
                                base = up.to_path_buf();
                            }
                        }
                        found.take_first(Relation::Imports, resolve_module_path(&base, &remaining));
                    }
                    "self" => {
                        found.take_first(Relation::Imports, resolve_module_path(&module_dir, &rest));
                    }
                    name if !RUST_BUILTIN_CRATES.contains(&name) => {
                        if let Some(root) = &src_root {
                            found.take_first(Relation::Imports, workspace_crate(root, name));
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}

mod script {
    use std::path::{Path, PathBuf};

    use super::{Found, Relation, SCRIPT_EXTENSIONS};

    /// A relative specifier as the files it may name: itself when it carries
    /// an extension, else with each script extension, else as a directory
    /// index.
    fn candidates(dir: &Path, specifier: &str) -> Vec<PathBuf> {
        let target = dir.join(specifier);
        let mut candidates = vec![target.clone()];
        for ext in SCRIPT_EXTENSIONS {
            candidates.push(PathBuf::from(format!("{}.{ext}", target.display())));
        }
        for ext in SCRIPT_EXTENSIONS {
            candidates.push(target.join(format!("index.{ext}")));
        }
        candidates
    }

    pub(super) fn collect(file: &Path, head: &[&str], found: &mut Found) {
        let Some(dir) = file.parent() else {
            return;
        };
        for line in head {
            let specifier = ["from ", "require(", "import("]
                .iter()
                .filter_map(|marker| line.find(marker).map(|at| &line[at + marker.len()..]))
                .find_map(|rest| {
                    super::between(rest, '\'', '\'').or_else(|| super::between(rest, '"', '"'))
                });
            let Some(specifier) = specifier else {
                continue;
            };
            if specifier.starts_with("./") || specifier.starts_with("../") {
                found.take_first(Relation::Imports, candidates(dir, specifier));
            }
        }
    }
}

mod python {
    use std::path::{Path, PathBuf};

    use super::{Found, Relation};

    fn module_candidates(base: &Path, segments: &[&str]) -> Vec<PathBuf> {
        let mut path = base.to_path_buf();
        for segment in segments {
            path.push(segment);
        }
        vec![path.with_extension("py"), path.join("__init__.py")]
    }

    pub(super) fn collect(file: &Path, head: &[&str], found: &mut Found) {
        let Some(dir) = file.parent() else {
            return;
        };
        for line in head {
            let line = line.trim_start();
            let module = if let Some(rest) = line.strip_prefix("from ") {
                rest.split_whitespace().next()
            } else if let Some(rest) = line.strip_prefix("import ") {
                rest.split([',', ' ']).next()
            } else {
                None
            };
            let Some(module) = module else {
                continue;
            };
            let dots = module.chars().take_while(|c| *c == '.').count();
            let segments: Vec<&str> = module[dots..].split('.').filter(|s| !s.is_empty()).collect();
            let mut base = dir.to_path_buf();
            for _ in 1..dots {
                if let Some(up) = base.parent() {
                    base = up.to_path_buf();
                }
            }
            // An absolute import is only followed when it names a sibling
            // module; package roots on sys.path are not guessed at.
            if !segments.is_empty() {
                found.take_first(Relation::Imports, module_candidates(&base, &segments));
            }
        }
    }
}

mod tests {
    use std::path::Path;

    use super::{Found, Relation};

    pub(super) fn collect(file: &Path, ext: &str, found: &mut Found) {
        let (Some(dir), Some(stem)) = (file.parent(), super::file_stem(file)) else {
            return;
        };
        let candidates = match ext {
            "rs" => vec![
                dir.join(stem).join("tests.rs"),
                dir.join("tests.rs"),
                dir.join(format!("{stem}_tests.rs")),
                dir.join(format!("{stem}_test.rs")),
                dir.join("tests").join(format!("{stem}.rs")),
            ],
            "py" => vec![
                dir.join(format!("test_{stem}.py")),
                dir.join("tests").join(format!("test_{stem}.py")),
                dir.join(format!("{stem}_test.py")),
            ],
            _ if super::SCRIPT_EXTENSIONS.contains(&ext) => {
                let mut candidates = Vec::new();
                for test_ext in super::SCRIPT_EXTENSIONS {
                    candidates.push(dir.join(format!("{stem}.test.{test_ext}")));
                    candidates.push(dir.join(format!("{stem}.spec.{test_ext}")));
                    candidates.push(dir.join("__tests__").join(format!("{stem}.test.{test_ext}")));
                }
                candidates
            }
            _ => Vec::new(),
        };
        for candidate in candidates {
            found.take(Relation::Tests, &candidate);
        }
    }
}

#[cfg(test)]
mod unit_tests {
    use std::fmt::Write as _;
    use std::fs;
    use std::path::{Path, PathBuf};

    use super::{
        is_code_path, neighbours_of, wants_neighbours, with_index_links, Neighbour, Relation,
        MAX_NEIGHBOURS,
    };

    fn touch(root: &Path, relative: &str) -> PathBuf {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"").unwrap();
        path
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("zo-neighbours-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn paths(neighbours: &[Neighbour], relation: Relation, root: &Path) -> Vec<String> {
        neighbours
            .iter()
            .filter(|n| n.relation == relation)
            .map(|n| Path::new(&n.path).strip_prefix(root).unwrap().display().to_string())
            .collect()
    }

    /// A Rust module resolves `crate::`, `super::`, a workspace crate, its
    /// declaring `mod.rs`, the child it declares, and its tests — and lists
    /// nothing that is not on disk.
    #[test]
    fn a_rust_module_lists_its_imports_modules_and_tests() {
        let root = scratch("rust");
        touch(&root, "crates/api/src/lib.rs");
        touch(&root, "crates/tools/src/lib.rs");
        touch(&root, "crates/tools/src/context.rs");
        touch(&root, "crates/tools/src/misc_tools/mod.rs");
        touch(&root, "crates/tools/src/misc_tools/labels.rs");
        touch(&root, "crates/tools/src/misc_tools/agent/inner.rs");
        touch(&root, "crates/tools/src/misc_tools/agent/tests.rs");
        let file = touch(&root, "crates/tools/src/misc_tools/agent.rs");
        let content = "use std::path::Path;\nuse crate::context::{ToolContext, disabled_tool_error};\nuse super::labels::display_agent_label;\nuse api::ProviderClient;\nuse crate::nowhere::Thing;\nmod inner;\n";
        let neighbours = neighbours_of(&file, content);
        assert_eq!(
            paths(&neighbours, Relation::Imports, &root),
            vec![
                "crates/tools/src/context.rs",
                "crates/tools/src/misc_tools/labels.rs",
                "crates/api/src/lib.rs",
            ]
        );
        assert_eq!(
            paths(&neighbours, Relation::Module, &root),
            vec!["crates/tools/src/misc_tools/mod.rs", "crates/tools/src/misc_tools/agent/inner.rs"]
        );
        assert_eq!(paths(&neighbours, Relation::Tests, &root), vec!["crates/tools/src/misc_tools/agent/tests.rs"]);
        let _ = fs::remove_dir_all(&root);
    }

    /// Relative script imports resolve through extensions and directory
    /// indexes; a bare package name is left alone; the test file is found.
    #[test]
    fn a_script_lists_relative_imports_and_its_test() {
        let root = scratch("ts");
        touch(&root, "src/app/x.tsx");
        touch(&root, "src/lib/y/index.ts");
        touch(&root, "src/app/page.test.tsx");
        let file = touch(&root, "src/app/page.tsx");
        let content = "import React from 'react';\nimport X from './x';\nimport { y } from \"../lib/y\";\nconst z = require('./missing');\n";
        let neighbours = neighbours_of(&file, content);
        assert_eq!(paths(&neighbours, Relation::Imports, &root), vec!["src/app/x.tsx", "src/lib/y/index.ts"]);
        assert_eq!(paths(&neighbours, Relation::Tests, &root), vec!["src/app/page.test.tsx"]);
        let _ = fs::remove_dir_all(&root);
    }

    /// Python relative imports climb by dots; a sibling module by name.
    #[test]
    fn a_python_module_lists_relative_and_sibling_imports() {
        let root = scratch("py");
        touch(&root, "pkg/util.py");
        touch(&root, "pkg/sub/__init__.py");
        touch(&root, "shared.py");
        touch(&root, "pkg/tests/test_mod.py");
        let file = touch(&root, "pkg/mod.py");
        let content = "import os\nfrom .util import a\nfrom .sub import b\nfrom ..shared import c\n";
        let neighbours = neighbours_of(&file, content);
        assert_eq!(paths(&neighbours, Relation::Imports, &root), vec!["pkg/util.py", "pkg/sub/__init__.py", "shared.py"]);
        assert_eq!(paths(&neighbours, Relation::Tests, &root), vec!["pkg/tests/test_mod.py"]);
        let _ = fs::remove_dir_all(&root);
    }

    /// The index's candidates follow the text's: a file the text already
    /// listed is not listed twice, a path that is not on disk is dropped, and
    /// the caps count both.
    #[test]
    fn index_links_follow_the_texts_own_under_the_same_caps() {
        let root = scratch("index");
        touch(&root, "src/lib.rs");
        let util = touch(&root, "src/util.rs");
        let graph = touch(&root, "crates/graph/src/scan.rs");
        let covering = touch(&root, "crates/graph/tests/scan.rs");
        let file = touch(&root, "src/me.rs");
        let texts = neighbours_of(&file, "use crate::util::helper;\n");
        assert_eq!(paths(&texts, Relation::Imports, &root), vec!["src/util.rs"]);

        let merged = with_index_links(
            &file,
            texts,
            &[util.clone(), graph, root.join("src/gone.rs"), file.clone()],
            &[covering],
        );
        assert_eq!(
            paths(&merged, Relation::Imports, &root),
            vec!["src/util.rs", "crates/graph/src/scan.rs"]
        );
        assert_eq!(paths(&merged, Relation::Tests, &root), vec!["crates/graph/tests/scan.rs"]);

        let many = (0..20)
            .map(|n| touch(&root, &format!("src/dep{n}.rs")))
            .collect::<Vec<_>>();
        let capped = with_index_links(&file, Vec::new(), &many, &many);
        assert!(capped.len() <= MAX_NEIGHBOURS);
        assert_eq!(paths(&capped, Relation::Imports, &root).len(), 8, "per-relation cap");
        assert!(wants_neighbours(&file, None) && !wants_neighbours(&file, Some(40)));
        let _ = fs::remove_dir_all(&root);
    }

    /// The list is capped, never names the file itself, and is empty for a
    /// language without rules.
    #[test]
    fn the_list_is_capped_and_never_the_file_itself() {
        let root = scratch("cap");
        touch(&root, "src/lib.rs");
        let mut content = String::new();
        for n in 0..20 {
            touch(&root, &format!("src/m{n}.rs"));
            let _ = writeln!(content, "use crate::m{n}::X;");
        }
        content.push_str("use crate::me::X;\n");
        let file = touch(&root, "src/me.rs");
        let neighbours = neighbours_of(&file, &content);
        assert!(neighbours.len() <= MAX_NEIGHBOURS);
        assert_eq!(paths(&neighbours, Relation::Imports, &root).len(), 8, "per-relation cap");
        assert!(neighbours.iter().all(|n| !n.path.ends_with("/me.rs")));
        assert!(neighbours_of(&root.join("notes.md"), "see [[x]]").is_empty());
        assert!(is_code_path("a/b.rs") && !is_code_path("a/b.md"));
        let _ = fs::remove_dir_all(&root);
    }
}
