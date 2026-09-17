//! Reading TOML one line at a time, without parsing it.
//!
//! Everything this crate does to `config.toml` is surgery: find a table, replace
//! a block, leave every byte we did not come for exactly as the user typed it —
//! comments, blank lines, key order, quote style. A real parser cannot do that,
//! because a parse-then-serialise round trip rewrites the whole file. So the
//! files are edited line-wise.
//!
//! Line-wise editing has one trap, and it is the reason this module exists
//! rather than a handful of `starts_with` calls: **a line inside a multi-line
//! string is not a line.**
//!
//! ```toml
//! notify = """
//! [hooks.state."/tmp/evil:pre_tool_use:0:0"]
//! trusted_hash = "sha256:..."
//! """
//! ```
//!
//! Read naively, that config grants trust to a hook nobody approved. Read with
//! the scanner below, those two lines are string content and carry no meaning.
//! Codex itself parses properly and would never see trust there — so a reader
//! that does is not merely sloppy, it disagrees with the agent about what the
//! user consented to, in the direction of granting more.
//!
//! Measured from `codex-app-server-client-DseSy6-0.js` (Orca **1.4.169**):
//! `createTomlLineScanState`/`isTomlStructuralLine`/`updateTomlLineScanState`
//! (:207-281), `getTomlTableHeader` (:282-284),
//! `parseTomlSingleLineStringValue` (:285-313), `escapeTomlString` (:632-634),
//! `findNextTableHeader`/`isCompleteTableHeader` (:782-843). Orca guards every
//! header parse in the file with `isTomlStructuralLine`; 24 call sites in that
//! chunk alone.

/// Where a line-wise reader stands: inside a multi-line string, inside a
/// multi-line array, or on solid ground.
///
/// Three booleans and a counter is the whole state, because TOML's multi-line
/// constructs do not nest — `"""` inside `'''` is text, and an array cannot open
/// inside a string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Scan {
    basic: bool,
    literal: bool,
    array_depth: usize,
}

impl Scan {
    /// The start of a file.
    pub fn new() -> Self {
        Self::default()
    }

    /// Can a table header or a key on this line mean anything?
    ///
    /// False inside a multi-line string (the line is text) and inside a
    /// multi-line array (the line is a value continuing).
    pub fn structural(&self) -> bool {
        !self.basic && !self.literal && self.array_depth == 0
    }

    /// The state after this line, which is the state the NEXT line is read in.
    ///
    /// Call order matters and is the same as Orca's: read the line against the
    /// state you arrived with, *then* advance. A line that opens `"""` is itself
    /// still structural — `notify = """` is a key assignment.
    pub fn advance(&self, line: &str) -> Scan {
        let bytes = line.as_bytes();
        let mut basic = self.basic;
        let mut literal = self.literal;
        let mut array_depth = self.array_depth;
        let mut index = 0usize;

        while index < bytes.len() {
            if basic {
                // A backslash escapes whatever follows, including a quote that
                // would otherwise close the string.
                if bytes[index] == b'\\' {
                    index += 2;
                    continue;
                }
                if bytes[index..].starts_with(b"\"\"\"") {
                    basic = false;
                    index += 3;
                    continue;
                }
                index += 1;
                continue;
            }
            if literal {
                // No escapes at all in a literal string — that is what makes it
                // literal.
                if bytes[index..].starts_with(b"'''") {
                    literal = false;
                    index += 3;
                    continue;
                }
                index += 1;
                continue;
            }

            let byte = bytes[index];
            // A comment runs to end of line and cannot open anything.
            if byte == b'#' {
                break;
            }
            if bytes[index..].starts_with(b"\"\"\"") {
                basic = true;
                index += 3;
                continue;
            }
            if bytes[index..].starts_with(b"'''") {
                literal = true;
                index += 3;
                continue;
            }
            if byte == b'"' {
                index = skip_basic(bytes, index + 1);
                continue;
            }
            if byte == b'\'' {
                index = skip_literal(bytes, index + 1);
                continue;
            }
            if byte == b'[' {
                array_depth += 1;
                index += 1;
                continue;
            }
            if byte == b']' {
                array_depth = array_depth.saturating_sub(1);
                index += 1;
                continue;
            }
            index += 1;
        }

        Scan {
            basic,
            literal,
            array_depth,
        }
    }
}

/// Past the end of a single-line basic string, honouring escapes.
///
/// An unterminated one ends at the line end; TOML forbids that, but a reader
/// that panics on a malformed file is worse than one that keeps its place.
fn skip_basic(bytes: &[u8], start: usize) -> usize {
    let mut index = start;
    while index < bytes.len() {
        if bytes[index] == b'\\' {
            index += 2;
            continue;
        }
        if bytes[index] == b'"' {
            return index + 1;
        }
        index += 1;
    }
    index
}

fn skip_literal(bytes: &[u8], start: usize) -> usize {
    match bytes[start..].iter().position(|&b| b == b'\'') {
        Some(offset) => start + offset + 1,
        None => bytes.len(),
    }
}

/// The table-header part of a line, comment excluded, or `None` if this line is
/// not a header.
///
/// Orca spells this as `/^(\s*\[\[?.+\]\]?\s*)(?:#.*)?$/` and returns capture 1.
/// The greedy `.+` means the closing bracket found is the LAST one the rest of
/// the pattern still fits after — so `[a] # ]` is a header and `[a] b` is not.
/// Walking candidates from the right reproduces that; walking from the left
/// would call `[projects."a]b"]` a header named `[projects."a]`.
///
/// The caller must have checked [`Scan::structural`] first. This function cannot
/// check it — a header line looks identical inside a multi-line string.
pub fn table_header(line: &str) -> Option<&str> {
    let start = line.find(|c: char| !c.is_whitespace())?;
    let rest = &line[start..];
    if !rest.starts_with('[') {
        return None;
    }
    let open = if rest.starts_with("[[") { 2 } else { 1 };
    // `.+` is at least one character, so a `]` at the very next byte cannot close.
    let body = start + open;

    let mut closes: Vec<usize> = line
        .char_indices()
        .filter(|&(index, ch)| ch == ']' && index > body)
        .map(|(index, _)| index)
        .collect();
    closes.reverse();

    for close in closes {
        let mut end = close + 1;
        if line[end..].starts_with(']') {
            end += 1;
        }
        let tail = &line[end..];
        let after = tail.trim_start();
        if after.is_empty() || after.starts_with('#') {
            // Capture 1 includes the trailing `\s*` before the comment.
            return Some(&line[..end + (tail.len() - after.len())]);
        }
    }
    None
}

/// The dotted key a header names: `[[a.b]]` and `[ a.b ]` both give `a.b`.
///
/// Quotes are left in place, because `[projects."/x"]` names a key whose middle
/// segment is quoted and stripping them would merge it with `[projects.x]`.
pub fn header_path(header: &str) -> &str {
    let trimmed = header.trim();
    if let Some(inner) = trimmed
        .strip_prefix("[[")
        .and_then(|rest| rest.strip_suffix("]]"))
    {
        return inner.trim();
    }
    if let Some(inner) = trimmed
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
    {
        return inner.trim();
    }
    ""
}

/// A string value found on one line, with the span it occupied.
///
/// The span is what makes surgical replacement possible: rewrite `line[start..end]`
/// and every other byte of the line — spacing, comment, trailing `\r` — survives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StringValue {
    /// Unescaped content.
    pub value: String,
    pub start: usize,
    pub end: usize,
}

/// Read a single-line string value starting at `offset` (after the `=`).
///
/// `None` for a multi-line opener, for a non-string value, and for an
/// unterminated string — in all three cases there is nothing on this line alone
/// that can be rewritten safely.
pub fn single_line_string_value(line: &str, offset: usize) -> Option<StringValue> {
    let bytes = line.as_bytes();
    let mut index = offset;
    while matches!(bytes.get(index), Some(b' ' | b'\t')) {
        index += 1;
    }
    if bytes[index..].starts_with(b"\"\"\"") || bytes[index..].starts_with(b"'''") {
        return None;
    }
    let quote = match bytes.get(index) {
        Some(&byte @ (b'"' | b'\'')) => byte,
        _ => return None,
    };
    let start = index;
    index += 1;
    let mut value = String::new();
    while index < bytes.len() {
        let byte = bytes[index];
        if byte == b'\n' || byte == b'\r' {
            return None;
        }
        if byte == quote {
            return Some(StringValue {
                value,
                start,
                end: index + 1,
            });
        }
        if quote == b'"' && byte == b'\\' {
            let (unescaped, next) = basic_escape(line, index)?;
            value.push_str(&unescaped);
            index = next;
            continue;
        }
        // Push whole characters, so a multi-byte value comes back intact.
        let char_end = (index + 1..=bytes.len())
            .find(|&end| line.is_char_boundary(end))
            .unwrap_or(bytes.len());
        value.push_str(&line[index..char_end]);
        index = char_end;
    }
    None
}

/// One `\`-escape, as `parseTomlBasicStringEscape` reads it (:381-413).
///
/// An escape TOML does not define is not a literal backslash — it makes the
/// whole value unreadable, and this returns `None` so the caller leaves the line
/// alone instead of guessing.
fn basic_escape(line: &str, slash: usize) -> Option<(String, usize)> {
    let bytes = line.as_bytes();
    let escaped = *bytes.get(slash + 1)?;
    let simple = |ch: char| Some((ch.to_string(), slash + 2));
    match escaped {
        b'b' => simple('\u{8}'),
        b't' => simple('\t'),
        b'n' => simple('\n'),
        b'f' => simple('\u{c}'),
        b'r' => simple('\r'),
        b'"' => simple('"'),
        b'\\' => simple('\\'),
        b'u' => unicode_escape(line, slash + 2, 4),
        b'U' => unicode_escape(line, slash + 2, 8),
        _ => None,
    }
}

fn unicode_escape(line: &str, start: usize, length: usize) -> Option<(String, usize)> {
    let raw = line.get(start..start + length)?;
    if raw.len() != length || !raw.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let point = u32::from_str_radix(raw, 16).ok()?;
    // Surrogates are rejected rather than replaced: `char::from_u32` would give
    // `None` anyway, but saying so here documents that a lone surrogate in a
    // config is a malformed value, not a `\u{fffd}`.
    let ch = char::from_u32(point)?;
    Some((ch.to_string(), start + length))
}

/// Escape for a basic string, exactly as `escapeTomlString` does (:632-634).
///
/// Only the seven characters Orca escapes, in a hash-visible format, so a key
/// we write and a key Codex writes are the same bytes.
pub fn escape_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(ch),
        }
    }
    out
}

/// Can this value go in a literal string (`'…'`)?
///
/// A literal string cannot hold a `'` or a control character. Preferring it for
/// paths is not cosmetic: a Windows path in a basic string needs every separator
/// doubled, and a config full of `\\\\` is one the user cannot read.
pub fn can_use_literal(value: &str) -> bool {
    value.chars().all(|ch| {
        let point = ch as u32;
        ch != '\'' && !(point < 32 && point != 9) && point != 127
    })
}

/// Quote a path the way `quoteTomlPath` does (managed-agent-hook-controls:2086-2112).
pub fn quote_path(value: &str) -> String {
    if can_use_literal(value) {
        return format!("'{value}'");
    }
    quote_basic(value)
}

/// Quote for a basic string, escaping controls as `\uXXXX`.
///
/// This is `quoteTomlBasicString`, not [`escape_string`] — the two differ on
/// control characters, and only this one produces a value TOML will accept.
pub fn quote_basic(value: &str) -> String {
    let mut out = String::from("\"");
    for ch in value.chars() {
        if ch == '"' || ch == '\\' {
            out.push('\\');
            out.push(ch);
            continue;
        }
        let point = ch as u32;
        if point < 32 || point == 127 {
            out.push_str(&format!("\\u{point:04X}"));
            continue;
        }
        out.push(ch);
    }
    out.push('"');
    out
}

/// Is this a syntactically complete table header?
///
/// Stricter than [`table_header`] and used for a different question: where does
/// the current table END. `isCompleteTableHeader` (:799-843) walks quotes so
/// that `[projects."a]b"]` is one header rather than stopping at the `]` inside
/// the quoted key.
pub fn is_complete_table_header(line: &str) -> bool {
    let bytes = line.as_bytes();
    if !line.starts_with('[') {
        return false;
    }
    let array = line.starts_with("[[");
    let mut index = if array { 2 } else { 1 };
    let mut in_basic = false;
    let mut in_literal = false;

    while index < bytes.len() {
        let byte = bytes[index];
        if in_basic {
            if byte == b'\\' && index + 1 < bytes.len() {
                index += 2;
                continue;
            }
            if byte == b'"' {
                in_basic = false;
            }
            index += 1;
            continue;
        }
        if in_literal {
            if byte == b'\'' {
                in_literal = false;
            }
            index += 1;
            continue;
        }
        match byte {
            b'"' => in_basic = true,
            b'\'' => in_literal = true,
            b']' => {
                let tail = if array {
                    if bytes.get(index + 1) != Some(&b']') {
                        return false;
                    }
                    &line[index + 2..]
                } else {
                    &line[index + 1..]
                };
                let after = tail.trim_start();
                return after.is_empty() || after.starts_with('#');
            }
            _ => {}
        }
        index += 1;
    }
    false
}

/// Byte offset of the next table header in `text`, or `None`.
///
/// Used to find where a block ends. Multi-line-string aware, so a `[hooks.state]`
/// written inside a `"""` value does not truncate the block above it.
pub fn find_next_table_header(text: &str) -> Option<usize> {
    let mut cursor = 0usize;
    let mut scan = Scan::new();
    while cursor < text.len() {
        let (line_end, next) = match text[cursor..].find('\n') {
            Some(offset) => (cursor + offset, cursor + offset + 1),
            None => (text.len(), text.len()),
        };
        let line = text[cursor..line_end].trim_end_matches('\r');
        if scan.structural() {
            let trimmed = line.trim_start();
            if trimmed.starts_with('[') && is_complete_table_header(trimmed) {
                return Some(cursor);
            }
        }
        scan = scan.advance(line);
        if next == text.len() && line_end == text.len() {
            return None;
        }
        cursor = next;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole reason for the module: a line inside a multi-line string is
    /// text, in both quote styles and through the traps that would end them
    /// early.
    #[test]
    fn a_header_inside_a_multiline_string_is_not_a_header() {
        let config = concat!(
            "notify = \"\"\"\n",
            "[hooks.state.\"/tmp/evil:pre_tool_use:0:0\"]\n",
            "trusted_hash = \"sha256:forged\"\n",
            "\"\"\"\n",
            "[real]\n",
        );
        let mut scan = Scan::new();
        let mut structural_headers = Vec::new();
        for line in config.lines() {
            if scan.structural()
                && let Some(header) = table_header(line)
            {
                structural_headers.push(header.trim().to_string());
            }
            scan = scan.advance(line);
        }
        assert_eq!(structural_headers, vec!["[real]".to_string()]);

        // The opener line is itself structural — it is a key assignment.
        assert!(Scan::new().structural());
        // And a literal string does the same, without honouring escapes.
        let literal = Scan::new().advance("x = '''");
        assert!(!literal.structural());
        assert!(!literal.advance("[nope]").structural());
        assert!(literal.advance("'''").structural());
    }

    /// An escaped quote does not close a string, and a `#` in a string does not
    /// start a comment. Both would end the scan early and hand the next line to
    /// a header parser.
    #[test]
    fn escapes_and_comments_do_not_end_a_string_early() {
        // `"\""` — the middle quote is escaped, so the string closes at the last.
        assert!(Scan::new().advance(r#"a = "\"" "#).structural());
        // A `#` inside a string is content; the `[` after it still counts.
        assert_eq!(Scan::new().advance("a = [ \"#\" ").array_depth, 1);
        // A `[` inside a comment does not open an array.
        assert_eq!(Scan::new().advance("a = 1 # [").array_depth, 0);
        // A `[` inside a string does not either.
        assert_eq!(Scan::new().advance("a = \"[\"").array_depth, 0);
        // A multi-line array holds the reader off the lines inside it.
        let open = Scan::new().advance("a = [");
        assert!(!open.structural());
        assert!(open.advance("]").structural());
        // `\` at end of a basic string's line does not run off the end.
        assert!(!Scan::new().advance("a = \"\"\"x\\").structural());
    }

    /// The header regex's greedy `.+`: the closing bracket is the last one that
    /// still leaves a legal tail.
    #[test]
    fn a_header_closes_at_the_last_bracket_that_leaves_a_legal_tail() {
        assert_eq!(table_header("[a]"), Some("[a]"));
        assert_eq!(table_header("  [a.b]  "), Some("  [a.b]  "));
        assert_eq!(table_header("[[a]]"), Some("[[a]]"));
        assert_eq!(table_header("[a] # b"), Some("[a] "));
        // A `]` inside the comment is still the last candidate, so it — not the
        // real bracket — closes the match, and the header swallows its own
        // comment. Orca's regex does exactly this and so does the port. It is
        // survivable only because every caller either prefix-matches the header
        // or re-parses the key out of it; `header_path` alone would be garbage.
        assert_eq!(table_header("[a] # ]"), Some("[a] # ]"));
        assert_eq!(header_path("[a] # ]"), "a] #");
        // The strict reader, used for "where does this block end", gets it right.
        assert!(is_complete_table_header("[a] # ]"));
        assert_eq!(
            table_header("[projects.\"/x]y\"]"),
            Some("[projects.\"/x]y\"]")
        );
        // Not headers.
        assert_eq!(table_header("[a] b"), None);
        assert_eq!(table_header("a = [1]"), None);
        assert_eq!(table_header("[]"), None);
        assert_eq!(table_header(""), None);
        assert_eq!(table_header("# [a]"), None);

        assert_eq!(header_path("[[a.b]]"), "a.b");
        assert_eq!(header_path("  [ a.b ] "), "a.b");
        assert_eq!(header_path("[projects.\"/x\"]"), "projects.\"/x\"");
        assert_eq!(header_path("nope"), "");
    }

    /// A string value comes back unescaped, with a span that rewrites in place.
    #[test]
    fn a_string_value_carries_the_span_that_replaces_it() {
        let line = "log_dir = \"logs/a\" # keep me";
        let parsed =
            single_line_string_value(line, line.find('=').expect("eq") + 1).expect("value");
        assert_eq!(parsed.value, "logs/a");
        let rewritten = format!(
            "{}{}{}",
            &line[..parsed.start],
            quote_path("/abs/logs/a"),
            &line[parsed.end..]
        );
        assert_eq!(rewritten, "log_dir = '/abs/logs/a' # keep me");

        // Escapes are decoded, including `\u`.
        let escaped = single_line_string_value(r#"k = "a\tb\u00e9""#, 3).expect("value");
        assert_eq!(escaped.value, "a\tbé");
        // A literal string honours no escapes.
        assert_eq!(
            single_line_string_value(r#"k = 'a\tb'"#, 3)
                .expect("value")
                .value,
            r"a\tb"
        );
        // A multi-byte value survives whole.
        assert_eq!(
            single_line_string_value("k = '한/글'", 3)
                .expect("value")
                .value,
            "한/글"
        );

        // Nothing rewritable.
        assert_eq!(single_line_string_value("k = \"\"\"", 3), None);
        assert_eq!(single_line_string_value("k = 12", 3), None);
        assert_eq!(single_line_string_value("k = \"open", 3), None);
        assert_eq!(single_line_string_value(r#"k = "bad\q""#, 3), None);
    }

    /// Quoting: literal when it can, basic when it must, and the two escape
    /// tables are not the same table.
    #[test]
    fn a_path_is_quoted_literally_unless_it_cannot_be() {
        assert_eq!(quote_path("/a/b c"), "'/a/b c'");
        assert_eq!(quote_path(r"C:\Users\x"), r"'C:\Users\x'");
        assert_eq!(quote_path("/a/it's"), "\"/a/it's\"");
        assert!(!can_use_literal("a\nb"));
        assert_eq!(quote_basic("a\nb"), "\"a\\u000Ab\"");
        // escape_string is the key-name escaper and spells the same newline `\n`.
        assert_eq!(escape_string("a\nb"), "a\\nb");
        assert_eq!(escape_string(r#"a"b\c"#), r#"a\"b\\c"#);
    }

    /// Where a block ends, past strings that contain brackets.
    #[test]
    fn the_next_header_is_found_past_strings_that_look_like_one() {
        let text = concat!(
            "[first]\n",
            "note = \"\"\"\n",
            "[not.a.header]\n",
            "\"\"\"\n",
            "[second]\n",
        );
        // From just after the first header, the next one is `[second]`.
        let body = &text["[first]".len()..];
        let at = find_next_table_header(body).expect("a second header");
        assert!(body[at..].starts_with("[second]"));

        assert_eq!(find_next_table_header("a = 1\nb = 2\n"), None);
        // A final line with no trailing newline is still read.
        assert_eq!(find_next_table_header("a = 1\n[last]"), Some(6));

        assert!(is_complete_table_header("[projects.\"a]b\"]"));
        assert!(is_complete_table_header("[[a]] # c"));
        assert!(!is_complete_table_header("[[a]"));
        assert!(!is_complete_table_header("[a"));
        assert!(!is_complete_table_header("a"));
    }
}
