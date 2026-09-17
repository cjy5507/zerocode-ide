use std::path::Path;

use ignore::{DirEntry, WalkBuilder};
use regex::{Regex, RegexBuilder};
use serde::Serialize;

use crate::explorer_policy::ExplorerPolicy;
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use serde::Deserialize;
use std::time::{Duration, Instant};

/// Optional content filters. An omitted object retains the literal search.
#[derive(Clone, Default, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub(crate) struct SearchOptions {
    pub(crate) case_sensitive: bool,
    pub(crate) whole_word: bool,
    pub(crate) regex: bool,
    pub(crate) include: String,
    pub(crate) exclude: String,
}

const PRUNED_DIRECTORIES: [&str; 4] = [".git", "target", "node_modules", ".re-scratch"];

/// One line of a content search, ready for the file panel.
#[derive(Serialize, Debug, PartialEq, Eq)]
pub(crate) struct TextHit {
    pub(crate) path: String,
    pub(crate) line: u32,
    pub(crate) text: String,
}

/// Find project-relative paths whose file name contains `query`.
pub(crate) fn search_names(
    root: &Path,
    query: &str,
    policy: &ExplorerPolicy,
) -> Result<Vec<String>, String> {
    let Some(matcher) = literal_matcher(query)? else {
        return Ok(Vec::new());
    };
    let mut hits = Vec::new();
    for entry in project_files(root, None) {
        if hits.len() >= policy.hit_cap {
            break;
        }
        if !matcher.is_match(&entry.file_name().to_string_lossy()) {
            continue;
        }
        if let Some(relative) = relative_path(root, entry.path()) {
            hits.push(relative);
        }
    }
    hits.sort_unstable();
    Ok(hits)
}

/// Find project-relative lines containing `query` without allocating a
/// lowercased copy of every line.
pub(crate) fn search(
    root: &Path,
    query: &str,
    options: Option<&SearchOptions>,
    policy: &ExplorerPolicy,
) -> Result<Vec<TextHit>, String> {
    let deadline =
        options.map(|_| Instant::now() + Duration::from_millis(policy.search_timeout_ms));
    let defaults = SearchOptions::default();
    let selected = options.unwrap_or(&defaults);
    let include = path_filter(root, &selected.include)?;
    let exclude = path_filter(root, &selected.exclude)?;
    let pattern = if selected.regex {
        query.to_owned()
    } else {
        regex::escape(query)
    };
    let pattern = if selected.whole_word {
        format!(r"\b(?:{pattern})\b")
    } else {
        pattern
    };
    let matcher = if options.is_none() {
        let Some(matcher) = literal_matcher(query)? else {
            return Ok(Vec::new());
        };
        matcher
    } else {
        RegexBuilder::new(&pattern)
            .case_insensitive(!selected.case_sensitive)
            .size_limit(policy.regex_size_limit)
            .build()
            .map_err(|_| "file.searchInvalidRegex".to_string())?
    };
    if query.is_empty() {
        return Ok(Vec::new());
    }
    // Legacy calls must retain their exact answer, including on a slow disk.
    // Opting into filtering also opts into the configured deadline.

    let mut hits = Vec::new();
    for entry in project_files(root, deadline) {
        check_deadline(deadline)?;
        if hits.len() >= policy.hit_cap {
            break;
        }
        if include
            .as_ref()
            .is_some_and(|filter| !filter.matched(entry.path(), false).is_ignore())
            || exclude
                .as_ref()
                .is_some_and(|filter| filter.matched(entry.path(), false).is_ignore())
        {
            continue;
        }
        if entry
            .metadata()
            .map(|metadata| metadata.len())
            .unwrap_or(u64::MAX)
            > policy.max_file_bytes
        {
            continue;
        }
        let Ok(contents) = std::fs::read_to_string(entry.path()) else {
            continue;
        };
        let Some(relative) = relative_path(root, entry.path()) else {
            continue;
        };
        let room = policy.hit_cap - hits.len();
        append_line_hits(
            &mut hits, &relative, &contents, &matcher, room, policy, deadline,
        )?;
    }
    check_deadline(deadline)?;
    hits.sort_by(|left, right| left.path.cmp(&right.path).then(left.line.cmp(&right.line)));
    Ok(hits)
}

fn check_deadline(deadline: Option<Instant>) -> Result<(), String> {
    if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
        Err("file.searchTimeout".into())
    } else {
        Ok(())
    }
}

// ignore already owns globset. Its builder compiles the same glob syntax;
// using it keeps the walker and filters on one dependency and path grammar.
fn path_filter(root: &Path, pattern: &str) -> Result<Option<Gitignore>, String> {
    let pattern = pattern.trim();
    if pattern.is_empty() {
        return Ok(None);
    }
    let mut builder = GitignoreBuilder::new(root);
    builder.allow_unclosed_class(false);
    builder
        .add_line(None, pattern)
        .map_err(|_| "file.searchInvalidGlob".to_string())?;
    builder
        .build()
        .map(Some)
        .map_err(|_| "file.searchInvalidGlob".to_string())
}

fn literal_matcher(query: &str) -> Result<Option<Regex>, String> {
    if query.is_empty() {
        return Ok(None);
    }
    RegexBuilder::new(&regex::escape(query))
        .case_insensitive(true)
        .build()
        .map(Some)
        .map_err(|error| format!("invalid search query: {error}"))
}

fn project_files(root: &Path, deadline: Option<Instant>) -> impl Iterator<Item = DirEntry> {
    let mut builder = WalkBuilder::new(root);
    builder
        .hidden(false)
        .follow_links(false)
        .parents(true)
        .ignore(true)
        .git_ignore(true)
        .git_global(false)
        .git_exclude(true)
        .require_git(false)
        .filter_entry(move |entry| {
            deadline.is_none_or(|until| Instant::now() < until) && searchable_entry(entry)
        });
    builder.build().filter_map(Result::ok).filter(|entry| {
        entry
            .file_type()
            .is_some_and(|file_type| file_type.is_file())
    })
}

fn searchable_entry(entry: &DirEntry) -> bool {
    entry.depth() == 0
        || !entry
            .file_type()
            .is_some_and(|file_type| file_type.is_dir())
        || !PRUNED_DIRECTORIES.contains(&entry.file_name().to_string_lossy().as_ref())
}

fn relative_path(root: &Path, path: &Path) -> Option<String> {
    path.strip_prefix(root)
        .ok()
        .map(|relative| relative.to_string_lossy().into_owned())
}

fn append_line_hits(
    hits: &mut Vec<TextHit>,
    path: &str,
    contents: &str,
    matcher: &Regex,
    room: usize,
    policy: &ExplorerPolicy,
    deadline: Option<Instant>,
) -> Result<(), String> {
    if room == 0 || contents.contains('\0') {
        return Ok(());
    }
    let limit = hits.len().saturating_add(room).min(policy.hit_cap);
    for (index, line) in contents.lines().enumerate() {
        check_deadline(deadline)?;
        if hits.len() >= limit {
            break;
        }
        if !matcher.is_match(line) {
            continue;
        }
        hits.push(TextHit {
            path: path.to_string(),
            line: u32::try_from(index).unwrap_or(u32::MAX).saturating_add(1),
            text: line
                .trim_start()
                .chars()
                .take(policy.max_hit_line_chars)
                .collect(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn find_text_with_options(
        root: &Path,
        query: &str,
        options: Option<&SearchOptions>,
    ) -> Result<Vec<TextHit>, String> {
        search(root, query, options, &ExplorerPolicy::default())
    }
    fn find_files(root: &Path, query: &str) -> Result<Vec<String>, String> {
        search_names(root, query, &ExplorerPolicy::default())
    }
    fn find_text(root: &Path, query: &str) -> Result<Vec<TextHit>, String> {
        search(root, query, None, &ExplorerPolicy::default())
    }

    #[test]
    fn search_options_include_exclude_case_and_words_are_composed() {
        let temp = tempfile::tempdir().expect("project");
        write(temp.path(), "src/keep.rs", "Needle\nneedle\nNeedles\n");
        write(temp.path(), "src/generated.rs", "Needle");
        write(temp.path(), "tests/test.rs", "Needle");
        let options = SearchOptions {
            include: "src/**".into(),
            exclude: "**/generated.*".into(),
            case_sensitive: true,
            whole_word: true,
            ..Default::default()
        };
        let hits = find_text_with_options(temp.path(), "Needle", Some(&options)).unwrap();
        assert_eq!(
            hits,
            [TextHit {
                path: "src/keep.rs".into(),
                line: 1,
                text: "Needle".into()
            }]
        );
    }

    #[test]
    fn search_options_invalid_regex_and_glob_are_errors() {
        let temp = tempfile::tempdir().unwrap();
        let options = SearchOptions {
            regex: true,
            ..Default::default()
        };
        assert!(find_text_with_options(temp.path(), "[", Some(&options)).is_err());
        let options = SearchOptions {
            include: "[".into(),
            ..Default::default()
        };
        assert!(find_text_with_options(temp.path(), "a", Some(&options)).is_err());
    }

    #[test]
    fn search_options_omitted_and_default_keep_the_serialized_pin() {
        let temp = tempfile::tempdir().unwrap();
        write(temp.path(), "src/lib.rs", "first\n  ÉTÉ a+b\nlast");
        let pin = r#"[{"path":"src/lib.rs","line":2,"text":"ÉTÉ a+b"}]"#;
        for options in [None, Some(&SearchOptions::default())] {
            assert_eq!(
                serde_json::to_string(
                    &find_text_with_options(temp.path(), "a+b", options).unwrap()
                )
                .unwrap(),
                pin
            );
        }
    }

    fn write(root: &Path, relative: &str, contents: impl AsRef<[u8]>) {
        let path = root.join(relative);
        std::fs::create_dir_all(path.parent().expect("test file parent")).expect("create parent");
        std::fs::write(path, contents).expect("write test file");
    }

    #[test]
    fn search_options_unicode_regex_and_budget_are_bounded() {
        let temp = tempfile::tempdir().unwrap();
        write(temp.path(), "words.rs", "ÉTÉ\na+b\nété\nétéx\n");
        let options = SearchOptions {
            whole_word: true,
            regex: true,
            ..Default::default()
        };
        let hits = find_text_with_options(temp.path(), "été|a\\+b", Some(&options)).unwrap();
        assert_eq!(
            hits.iter().map(|hit| hit.line).collect::<Vec<_>>(),
            [1, 2, 3]
        );
        let policy = ExplorerPolicy {
            hit_cap: 1,
            max_hit_line_chars: 2,
            ..Default::default()
        };
        let hits = search(temp.path(), "été", Some(&options), &policy).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].text, "ÉT");
        assert_eq!(
            check_deadline(Some(Instant::now())).unwrap_err(),
            "file.searchTimeout"
        );
    }

    #[test]
    #[ignore = "diagnostic timing table; run explicitly with --ignored --nocapture"]
    fn explorer_search_timing_table() {
        let temp = tempfile::tempdir().unwrap();
        for index in 0..600 {
            write(
                temp.path(),
                &format!(
                    "{}/file{index}.rs",
                    if index % 2 == 0 { "src" } else { "tests" }
                ),
                "some source line\n".repeat(40) + "needle42\n",
            );
        }
        let options = SearchOptions {
            regex: true,
            include: "src/**".into(),
            ..Default::default()
        };
        for (label, options) in [("omitted", None), ("regex + include", Some(&options))] {
            for sample in 0..5 {
                let started = Instant::now();
                let hits = find_text_with_options(temp.path(), "needle42", options).unwrap();
                eprintln!(
                    "SEARCH_TIMING,{label},{sample},{:.3},{}",
                    started.elapsed().as_secs_f64() * 1000.0,
                    hits.len()
                );
            }
        }
    }

    #[test]
    fn name_search_is_literal_case_insensitive_and_pruned_once() {
        let temp = tempfile::tempdir().expect("temp project");
        write(temp.path(), "src/Main.rs", "main");
        write(temp.path(), ".hidden/tool.RS", "hidden");
        write(temp.path(), "target/generated.rs", "noise");
        write(temp.path(), "ignored/generated.rs", "noise");
        write(temp.path(), ".gitignore", "ignored/\n");

        assert_eq!(
            find_files(temp.path(), ".rs").expect("name search"),
            [".hidden/tool.RS", "src/Main.rs"]
        );
        assert_eq!(
            find_files(temp.path(), "Main.rs").expect("literal name search"),
            ["src/Main.rs"]
        );
    }

    #[test]
    fn text_search_reports_unicode_literal_matches_and_human_line_numbers() {
        let temp = tempfile::tempdir().expect("temp project");
        write(temp.path(), "src/lib.rs", "first\n  ÉTÉ a+b\nlast");

        let folded = find_text(temp.path(), "été").expect("unicode search");
        assert_eq!(folded.len(), 1);
        assert_eq!((folded[0].line, folded[0].text.as_str()), (2, "ÉTÉ a+b"));
        assert_eq!(
            find_text(temp.path(), "a+b").expect("literal search").len(),
            1
        );
    }

    #[test]
    fn binary_and_oversized_lines_stay_bounded() {
        let temp = tempfile::tempdir().expect("temp project");
        write(temp.path(), "binary.bin", b"needle\0needle");
        write(
            temp.path(),
            "bundle.js",
            format!("  needle{}", "x".repeat(5_000)),
        );

        let hits = find_text(temp.path(), "needle").expect("bounded search");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].path, "bundle.js");
        assert_eq!(
            hits[0].text.chars().count(),
            ExplorerPolicy::default().max_hit_line_chars
        );
        assert!(hits[0].text.starts_with("needle"));
    }

    #[test]
    fn line_collection_stops_at_its_remaining_room() {
        let matcher = literal_matcher("e")
            .expect("matcher")
            .expect("non-empty matcher");
        let mut hits = Vec::new();
        append_line_hits(
            &mut hits,
            "a",
            &"e\n".repeat(1_000),
            &matcher,
            3,
            &ExplorerPolicy::default(),
            None,
        )
        .unwrap();

        assert_eq!(hits.len(), 3);
    }
}
