//! Measurement harness: what a file's own text lists as its neighbours
//! (`file_neighbours::neighbours_of`), for every file of a snapshot — the
//! "before" that `tools/codegraph-bench/truth.py` holds against
//! rust-analyzer next to what the codegraph index adds (t-5970).
//!
//! Ignored by default: it reads a tree from disk and writes numbers' inputs
//! rather than asserting a contract. Run it with
//!
//! ```text
//! ZO_MEASURE_NEIGHBOURS_ROOT=<snapshot> \
//! ZO_MEASURE_NEIGHBOURS_FILES=<file: one snapshot-relative path per line> \
//! ZO_MEASURE_NEIGHBOURS_OUT=<neighbours.json> \
//!   cargo test -p runtime --test neighbours_measure -- --ignored --nocapture
//! ```

use std::fs;
use std::path::{Path, PathBuf};

use runtime::file_neighbours::neighbours_of;
use serde_json::json;

const ROOT_VAR: &str = "ZO_MEASURE_NEIGHBOURS_ROOT";
const FILES_VAR: &str = "ZO_MEASURE_NEIGHBOURS_FILES";
const OUT_VAR: &str = "ZO_MEASURE_NEIGHBOURS_OUT";

fn required(var: &str) -> PathBuf {
    PathBuf::from(std::env::var_os(var).unwrap_or_else(|| panic!("{var} is required")))
}

fn relative(path: &str, root: &Path) -> String {
    Path::new(path)
        .strip_prefix(root)
        .map_or_else(|_| path.to_string(), |path| path.to_string_lossy().into_owned())
}

#[test]
#[ignore = "measurement: reads a snapshot named by ZO_MEASURE_NEIGHBOURS_ROOT"]
fn dump_the_texts_own_neighbours_of_every_file() {
    let root = fs::canonicalize(required(ROOT_VAR)).expect("snapshot root");
    let list = fs::read_to_string(required(FILES_VAR)).expect("file list");
    let mut files = Vec::new();
    for file in list.lines().filter(|line| !line.trim().is_empty()) {
        let path = root.join(file);
        let Ok(content) = fs::read_to_string(&path) else {
            continue;
        };
        let neighbours = neighbours_of(&path, &content)
            .into_iter()
            .map(|neighbour| {
                json!({
                    "relation": neighbour.relation.label(),
                    "path": relative(&neighbour.path, &root),
                })
            })
            .collect::<Vec<_>>();
        files.push(json!({ "file": file, "neighbours": neighbours }));
    }
    let count = files.len();
    fs::write(
        required(OUT_VAR),
        serde_json::to_vec(&json!({ "files": files })).expect("encode"),
    )
    .expect("write the dump");
    println!("neighbours of {count} files written");
}
