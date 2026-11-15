/// Walk the complete profile block, so future rows cannot silently lose sources.
pub fn assert_sources(rendered: &str) {
    let mut pending: Option<&str> = None;
    let mut fields = 0;
    let profile = rendered.lines()
        .skip_while(|row| !row.trim_start().starts_with("Edit tool "))
        .take_while(|row| !row.trim_start().starts_with("Workspace "));
    for row in profile {
        let row = row.trim_start();
        assert!(row.len() >= 18, "malformed profile row: {row}");
        let (label, value) = row.split_at(18);
        let (label, value) = (label.trim(), value.trim());
        if let Some(label) = label.strip_suffix(" source") {
            assert_eq!(pending.take(), Some(label), "source without its value: {row}");
            assert!(valid_source(value), "unknown source: {row}");
            continue;
        }
        assert!(pending.is_none(), "{} has no source", pending.unwrap_or_default());
        fields += 1;
        if let Some((_, source)) = value.rsplit_once(" (") {
            let source = source.strip_suffix(')').expect("source closes with a parenthesis");
            assert!(valid_source(source), "unknown source: {row}");
        } else {
            pending = Some(label);
        }
    }
    assert!(pending.is_none(), "{} has no source", pending.unwrap_or_default());
    assert!(fields > 0, "the report has no profile values");
}

fn valid_source(source: &str) -> bool {
    matches!(source, "measured" | "read" | "declared" | "inherited" | "default")
}
