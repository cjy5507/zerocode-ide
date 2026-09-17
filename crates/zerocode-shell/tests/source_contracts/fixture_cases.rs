//! Shared by the bin and standalone source-contract registries.
/// Count static string labels per switch, including fixture invoke/mutate
/// switches. Tokenize first so comments, strings and nested switches cannot
/// masquerade as a second branch. This is a fixture-source contract, not a JS evaluator.
fn duplicate_switch_cases(source: &str) -> Vec<String> {
    use std::collections::{BTreeMap, BTreeSet};
    let tokens = regex::Regex::new(
        r#"(?s)//[^\n]*|/\*.*?\*/|"(?:\\.|[^"\\])*"|'(?:\\.|[^'\\])*'|`(?:\\.|[^`\\])*`|/(?:\\.|\[(?:\\.|[^\]\\])*\]|[^/\\\r\n])+/[a-z]*|[\w$]+|[^\s]"#,
    ).unwrap();
    let tokens: Vec<_> = tokens
        .find_iter(source)
        .map(|m| m.as_str())
        .filter(|token| !token.starts_with("//") && !token.starts_with("/*"))
        .collect();
    let mut depth = 0usize;
    let mut pending_switch = false;
    let mut switches: Vec<(usize, BTreeMap<String, usize>)> = Vec::new();
    let mut duplicates = BTreeSet::new();
    for (i, token) in tokens.iter().enumerate() {
        match *token {
            "switch" => pending_switch = true,
            "{" => {
                depth += 1;
                if pending_switch {
                    switches.push((depth, BTreeMap::new()));
                    pending_switch = false;
                }
            }
            "}" => {
                if switches.last().is_some_and(|(at, _)| *at == depth) {
                    switches.pop();
                }
                depth = depth.saturating_sub(1);
            }
            "case" if tokens.get(i + 2) == Some(&":") => {
                if let Some((at, labels)) = switches.last_mut()
                    && *at == depth
                    && let Some(literal) = tokens.get(i + 1)
                    && let Some(label) = literal
                        .strip_prefix(['\'', '"'])
                        .and_then(|s| s.strip_suffix(['\'', '"']))
                {
                    let count = labels.entry(label.to_owned()).or_default();
                    *count += 1;
                    if *count > 1 {
                        duplicates.insert(label.to_owned());
                    }
                }
            }
            _ => {}
        }
    }
    duplicates.into_iter().collect()
}

#[test]
fn fixture_case_counter_rejects_duplicate_invoke_labels() {
    let source = r#"async invoke(command) { switch (command) {
        case "choose_paths": return [];
        case 'choose_paths': throw new Error("picker unavailable");
    } }"#;
    assert_eq!(duplicate_switch_cases(source), ["choose_paths"]);
}

#[test]
fn fixture_case_counter_scopes_switches_and_ignores_quoted_cases() {
    let source = r#"switch (command) {
        case "choose_paths": {
            // case "choose_paths":
            const text = `case "choose_paths":`;
            const quoted = 'case "choose_paths":';
            /* case "choose_paths": */
            switch (kind) { case "choose_paths": return 1; }
        }
    }
    switch (command) { case 'choose_paths': return 2; }"#;
    assert!(duplicate_switch_cases(source).is_empty());
}

#[test]
fn browser_fixtures_have_no_duplicate_switch_cases() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ui/tests");
    for entry in std::fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|ext| ext == "mjs") {
            let source = std::fs::read_to_string(&path).unwrap();
            let duplicates = duplicate_switch_cases(&source);
            assert!(
                duplicates.is_empty(),
                "{}: duplicate fixture case labels: {duplicates:?}",
                path.display()
            );
        }
    }
}
