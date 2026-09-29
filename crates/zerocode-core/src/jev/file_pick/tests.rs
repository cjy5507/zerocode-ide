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

/// The file pick seat's label, golden (t-11349) — the one zo and a window
/// pane both write ([`label_row`] over [`edited_fingerprints`]). The judgment
/// is the ranked top three; today's rule is the session's recent-edit order
/// (the seat's baseline); hindsight is the files the turn's edits wrote.
#[test]
fn the_file_pick_label_golden_table() {
    struct Case {
        ranked: &'static [&'static str],
        baseline: &'static [&'static str],
        edited: &'static [&'static str],
        agreed: Option<bool>,
        baseline_agreed: Option<bool>,
        not_compared: Option<&'static str>,
    }
    let cases = [
        // The judgment found the file the turn edited; today's recent-edit
        // order did not — the rule wrong, the judgment right.
        Case {
            ranked: &["src/a.rs"],
            baseline: &["src/old.rs"],
            edited: &["/w/p/src/a.rs"],
            agreed: Some(true),
            baseline_agreed: Some(false),
            not_compared: None,
        },
        // The recent-edit order found it and the judgment did not.
        Case {
            ranked: &["src/b.rs"],
            baseline: &["src/a.rs"],
            edited: &["/w/p/src/a.rs"],
            agreed: Some(false),
            baseline_agreed: Some(true),
            not_compared: None,
        },
        // Both found it; neither did.
        Case {
            ranked: &["src/a.rs"],
            baseline: &["src/a.rs"],
            edited: &["/w/p/src/a.rs"],
            agreed: Some(true),
            baseline_agreed: Some(true),
            not_compared: None,
        },
        Case {
            ranked: &["src/b.rs"],
            baseline: &["src/c.rs"],
            edited: &["/w/p/src/a.rs"],
            agreed: Some(false),
            baseline_agreed: Some(false),
            not_compared: None,
        },
        // The file named by the pane's own spelling of its folder.
        Case {
            ranked: &["src/a.rs"],
            baseline: &[],
            edited: &["/link/p/src/a.rs"],
            agreed: Some(true),
            baseline_agreed: Some(false),
            not_compared: None,
        },
        // A turn that edited no file of the project is no comparison.
        Case {
            ranked: &["src/a.rs"],
            baseline: &["src/a.rs"],
            edited: &[],
            agreed: None,
            baseline_agreed: None,
            not_compared: Some(FILE_PICK_NO_EDIT_LABEL),
        },
        Case {
            ranked: &["src/a.rs"],
            baseline: &["src/a.rs"],
            edited: &["/elsewhere/a.rs"],
            agreed: None,
            baseline_agreed: None,
            not_compared: Some(FILE_PICK_NO_EDIT_LABEL),
        },
    ];
    let roots = [Path::new("/w/p"), Path::new("/link/p")];
    for case in cases {
        let fingerprints = |paths: &[&str]| {
            paths
                .iter()
                .map(|path| fingerprint_of(path))
                .collect::<Vec<_>>()
        };
        let request = json!({
            "rankedPaths": fingerprints(case.ranked),
            "baselineCandidates": fingerprints(case.baseline),
            "candidatePaths": fingerprints(case.ranked),
            "selectedPaths": [],
            "applied": false,
        });
        let edited: Vec<String> = case.edited.iter().map(|path| (*path).to_string()).collect();
        let label = label_row(
            &request,
            9,
            &edited_fingerprints(&roots, &edited),
            Some(2),
            1,
        );
        assert_eq!(
            (
                label.agreed,
                label.baseline_agreed,
                label.not_compared.as_deref()
            ),
            (case.agreed, case.baseline_agreed, case.not_compared),
            "{:?} {:?} {:?}",
            case.ranked,
            case.baseline,
            case.edited
        );
        assert_eq!(
            (
                label.kind.as_str(),
                label.label.as_str(),
                label.hindsight.as_str()
            ),
            ("label", "9", FILE_PICK_LABEL_HINDSIGHT)
        );
    }
}

/// A worker's prompt opens with the window's own briefing (t-14869): the
/// words a search looks for are the task's, after the briefing hands over —
/// the same boilerplate every time found the same few files, and a pane's
/// candidates held a file its turn edited 0 times in 7. Words that look like
/// code — a name with `_` or an inner capital — are looked for first.
#[test]
fn a_search_looks_for_the_tasks_own_words_and_names_that_look_like_code_first() {
    let briefing = crate::orchestration::worker_briefing("t-1", "the parser crash");
    let prompt = format!(
        "{briefing}Please fix the crash when an empty line reaches lexer_state in ParserTable"
    );
    let terms = search_terms(&prompt);
    assert_eq!(
        terms,
        [
            "lexer_state",
            "parsertable",
            "crash",
            "empty",
            "line",
            "reaches"
        ],
        "{terms:?}"
    );
    // Pasted as a block, the frame around it is no word of the task's.
    let pasted = format!("<pasted_content id=\"8a76\">\n{prompt}\n</pasted_content id=\"8a76\">");
    assert_eq!(search_terms(&pasted), terms);
    // A person's own words, with no briefing, are all the task there is.
    assert_eq!(
        search_terms("fix the crash in lexer_state"),
        ["lexer_state", "crash"]
    );
}
