//! The file pick seat's shared half on its own: the words its version is
//! pinned to, what a reply ranks and selects, and what a label makes of the
//! files a turn edited (t-11349 moved it here; zo's own cases still run on it
//! through zo's seat).

use serde_json::json;

use super::*;

fn candidate(path: &str) -> FilePickCandidate {
    FilePickCandidate {
        path: path.to_string(),
        about: String::new(),
    }
}

/// The version is pinned to the words zo pinned it to (t-9469): moving the
/// words moved no word.
#[test]
fn the_version_is_pinned_to_the_same_words() {
    assert_eq!(FILE_PICK_RUBRIC_VERSION, 1);
    assert_eq!(
        crate::jev::rubric_fingerprint(rubric_words),
        "5132dc056a853c32"
    );
}

/// A reply ranks every candidate and selects only the confident ones, and a
/// label reads the ranked top three against the files the turn edited.
#[test]
fn a_reply_ranks_and_a_label_reads_the_edited_files() {
    let files = vec![candidate("src/a.rs"), candidate("src/b.rs")];
    let answers = json!({
        "any": { "type": "noul", "noul": 0.95 },
        "F01": { "type": "noul", "noul": 0.72 },
        "F02": { "type": "noul", "noul": 0.93 },
    });
    assert_eq!(rank_candidates(&files, &answers), ["src/b.rs", "src/a.rs"]);
    let request = json!({
        "rankedPaths": [fingerprint_of("src/b.rs")],
        "baselineCandidates": [fingerprint_of("src/a.rs")],
        "candidatePaths": [fingerprint_of("src/a.rs"), fingerprint_of("src/b.rs")],
        "selectedPaths": [],
    });
    let root = Path::new("/w/project");
    let edited = edited_fingerprints(&[root], &["/w/project/src/b.rs".to_string()]);
    let label = label_row(&request, 7, &edited, None, 1);
    assert_eq!(
        (label.agreed, label.baseline_agreed, label.label.as_str()),
        (Some(true), Some(false), "7")
    );
    let none = label_row(&request, 7, &[], None, 1);
    assert_eq!(none.not_compared.as_deref(), Some(FILE_PICK_NO_EDIT_LABEL));
}

