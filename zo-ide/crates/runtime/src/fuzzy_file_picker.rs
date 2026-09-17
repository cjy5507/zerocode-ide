use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct FileMatch {
    pub relative_path: String,
    pub score: i64,
}

#[must_use]
pub fn fuzzy_find_files(root: &Path, query: &str, max_results: usize) -> Vec<FileMatch> {
    let query_lower = query.to_lowercase();
    let query_chars: Vec<char> = query_lower.chars().collect();

    let mut matches = Vec::new();
    collect_files(root, root, &query_chars, &mut matches);

    matches.sort_by(|a, b| b.score.cmp(&a.score));
    matches.truncate(max_results);
    matches
}

fn collect_files(root: &Path, dir: &Path, query_chars: &[char], matches: &mut Vec<FileMatch>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        if name_str.starts_with('.') || name_str == "node_modules" || name_str == "target" {
            continue;
        }

        if path.is_dir() {
            collect_files(root, &path, query_chars, matches);
        } else if let Some(score) = fuzzy_score(&name_str, query_chars) {
            let relative = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .to_string();
            matches.push(FileMatch {
                relative_path: relative,
                score,
            });
        }
    }
}

fn fuzzy_score(haystack: &str, needle_chars: &[char]) -> Option<i64> {
    if needle_chars.is_empty() {
        return Some(0);
    }

    let hay_lower = haystack.to_lowercase();
    let hay_chars: Vec<char> = hay_lower.chars().collect();

    let mut needle_idx = 0;
    let mut score: i64 = 0;
    let mut prev_match = false;

    for (i, &h) in hay_chars.iter().enumerate() {
        if needle_idx < needle_chars.len() && h == needle_chars[needle_idx] {
            score += 10;
            if prev_match {
                score += 5;
            }
            if i == 0
                || matches!(
                    hay_chars.get(i.wrapping_sub(1)),
                    Some('/' | '_' | '-' | '.')
                )
            {
                score += 20;
            }
            needle_idx += 1;
            prev_match = true;
        } else {
            prev_match = false;
        }
    }

    if needle_idx == needle_chars.len() {
        #[allow(clippy::cast_possible_wrap)]
        let length_penalty = hay_chars.len() as i64;
        Some(score - length_penalty)
    } else {
        None
    }
}

#[must_use]
pub fn parse_file_reference(input: &str) -> Option<FileReference> {
    if !input.starts_with('@') {
        return None;
    }

    let rest = &input[1..];
    if let Some((path, range)) = rest.split_once('#') {
        let line_range = parse_line_range(range);
        Some(FileReference {
            path: PathBuf::from(path),
            line_range,
        })
    } else {
        Some(FileReference {
            path: PathBuf::from(rest),
            line_range: None,
        })
    }
}

fn parse_line_range(s: &str) -> Option<(usize, Option<usize>)> {
    let s = s
        .strip_prefix('L')
        .or_else(|| s.strip_prefix('l'))
        .unwrap_or(s);
    if let Some((start, end)) = s.split_once('-') {
        let start: usize = start.parse().ok()?;
        let end: usize = end.parse().ok()?;
        Some((start, Some(end)))
    } else {
        let line: usize = s.parse().ok()?;
        Some((line, None))
    }
}

#[derive(Debug, Clone)]
pub struct FileReference {
    pub path: PathBuf,
    pub line_range: Option<(usize, Option<usize>)>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzzy_score_matches_exact() {
        let chars: Vec<char> = "main".chars().collect();
        assert!(fuzzy_score("main.rs", &chars).is_some());
    }

    #[test]
    fn fuzzy_score_rejects_nonmatch() {
        let chars: Vec<char> = "xyz".chars().collect();
        assert!(fuzzy_score("main.rs", &chars).is_none());
    }

    #[test]
    fn parse_file_reference_with_line_range() {
        let r = parse_file_reference("@src/main.rs#L10-20").unwrap();
        assert_eq!(r.path, PathBuf::from("src/main.rs"));
        assert_eq!(r.line_range, Some((10, Some(20))));
    }

    #[test]
    fn parse_file_reference_without_range() {
        let r = parse_file_reference("@Cargo.toml").unwrap();
        assert_eq!(r.path, PathBuf::from("Cargo.toml"));
        assert!(r.line_range.is_none());
    }

    #[test]
    fn parse_returns_none_for_non_at() {
        assert!(parse_file_reference("src/main.rs").is_none());
    }
}
