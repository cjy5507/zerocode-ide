//! A `CODEX_HOME` that ZeroCode owns, for the machine where the grant cannot
//! succeed.
//!
//! `codex_grant` asks Codex to trust our hook and `codex_install` takes the hook
//! back out when it will not. That is correct and it is also a dead end: on a
//! Codex too old for `app-server`, the answer is *never*, and the user gets no
//! hook forever.
//!
//! ## Why a second home rather than writing the trust ourselves
//!
//! We know the hash Codex wants — `codex_trust::trusted_hash` computes it. So
//! why not write `[hooks.state."…"] trusted_hash = "…"` into `~/.codex/config.toml`
//! and be done?
//!
//! Because that file is the record of what the **user** agreed to run. A hash we
//! put there is indistinguishable from one they approved, and it would apply to
//! every `codex` they start — in this window, in their own terminal, in CI on
//! that machine. Writing it would not be "installing a hook"; it would be
//! forging a consent decision and then hiding the forgery in the place the user
//! would look to check.
//!
//! So instead: a home ZeroCode created, whose `config.toml` mirrors the user's
//! settings so the agent behaves the same, and whose `[hooks.state]` is ours
//! because we made the home. `codex` run from a terminal is untouched. The
//! property is narrow and it is the whole point: **the mirror changes what
//! ZeroCode's Codex does, and nothing else's.**
//!
//! That boundary is a type here, not a comment. [`Home`] can only be built by
//! appending [`HOME_SEGMENTS`] to a directory, so no value of that type can ever
//! name `~/.codex` — and [`write_trust`] takes nothing else.
//!
//! ## What mirroring costs, and the three rules that pay for it
//!
//! A copied config is wrong in three ways, each measured from
//! `managed-agent-hook-controls-Hy3KYvcp.js` (Orca **1.4.169**):
//!
//! 1. **Relative paths move.** `log_dir = "logs"` means `~/.codex/logs` in the
//!    original and `<mirror>/logs` in the copy — the same text, a different
//!    directory. Every path-valued key is absolutised against the source
//!    (`rewriteRelativePathConfigValues`, :2047-2061).
//! 2. **Trust must not come along.** `[hooks.state.*]` in the user's config is
//!    their decision about their home; copied in, it becomes a decision they
//!    never made about ours (`stripRuntimeOwnedTomlSections`, :2449-2455).
//! 3. **Trust must not be lost either.** Re-mirroring on every launch would wipe
//!    the trust we just wrote. So the merge keeps the runtime's own
//!    `[hooks.state.*]` and `[projects.*]` sections
//!    (`mergeSystemCodexConfigIntoRuntime`, :2931-2938).
//!
//! Rules 2 and 3 collide over one case, and Orca's answer is the right one:
//! **the user's revocation wins.** If they mark a project `trust_level = "untrusted"`
//! in their own config, the mirror's trusted section for that project is dropped
//! — unless their config also records it trusted somewhere, which means they
//! changed their mind back. Preserving our copy instead would let ZeroCode run
//! agents unsandboxed in a directory the user had just locked down.

use std::path::{Path, PathBuf};

use crate::codex_trust::{TrustEntry, trust_key, trusted_hash};
use crate::toml_lines::{self, Scan};

/// Where the mirror lives under the app's data directory
/// (`resolveOrcaManagedCodexHomePath`, :446-448).
///
/// The spelling is `zerocode_core`'s because it crosses the window's edge: a
/// zo started outside a pane reads this same directory to borrow the login
/// the window holds (t-5777).
pub use zerocode_core::codex_account::RUNTIME_HOME_SEGMENTS as HOME_SEGMENTS;

/// A `CODEX_HOME` this product owns.
///
/// The single constructor appends [`HOME_SEGMENTS`], so a `Home` is structurally
/// incapable of pointing at the user's `~/.codex`. Everything that writes trust
/// takes one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Home(PathBuf);

impl Home {
    /// The mirror under a data directory — `<user_data>/codex-runtime-home/home`.
    pub fn under(user_data: &Path) -> Self {
        let mut path = user_data.to_path_buf();
        for segment in HOME_SEGMENTS {
            path.push(segment);
        }
        Home(path)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    pub fn config_path(&self) -> PathBuf {
        self.0.join("config.toml")
    }

    pub fn hooks_path(&self) -> PathBuf {
        self.0.join("hooks.json")
    }
}

/// The path-valued keys, by full dotted path (`EXACT_PATH_CONFIG_KEYS`, :2036-2046).
const EXACT_PATH_KEYS: [&str; 9] = [
    "debug.config_lockfile.export_dir",
    "debug.config_lockfile.load_path",
    "experimental_compact_prompt_file",
    "experimental_instructions_file",
    "log_dir",
    "model_catalog_json",
    "model_instructions_file",
    "skills.config.path",
    "sqlite_home",
];

/// Does `<table>.<key>` name a path?
///
/// The four families beyond the exact list are per-entry tables, so their middle
/// segment is a user-chosen name and cannot be enumerated (`isPathConfigKey`,
/// :2071-2076).
fn is_path_key(table_path: &str, key: &str) -> bool {
    let squeeze =
        |value: &str| -> String { value.chars().filter(|ch| !ch.is_whitespace()).collect() };
    let key = squeeze(key);
    let full = if table_path.is_empty() {
        key
    } else {
        format!("{}.{}", squeeze(table_path), key)
    };
    if EXACT_PATH_KEYS.contains(&full.as_str()) {
        return true;
    }
    // `agents.<name>.config_file`
    if let Some(rest) = full.strip_prefix("agents.")
        && let Some(name) = rest.strip_suffix(".config_file")
    {
        return !name.is_empty();
    }
    // `model_providers.<name>.auth.cwd`
    if let Some(rest) = full.strip_prefix("model_providers.")
        && let Some(name) = rest.strip_suffix(".auth.cwd")
    {
        return !name.is_empty();
    }
    // `profiles.<name>.<one of three>`
    if let Some(rest) = full.strip_prefix("profiles.") {
        for tail in [
            ".experimental_compact_prompt_file",
            ".model_catalog_json",
            ".model_instructions_file",
        ] {
            if let Some(name) = rest.strip_suffix(tail) {
                return !name.is_empty();
            }
        }
    }
    false
}

/// Is this value a relative path that needs absolutising?
///
/// Four kinds of value are left alone, and each for a reason that would break if
/// we rewrote it (`shouldRewriteRelativePath`, :2080-2085): `~` and `$VAR` and
/// `%VAR%` are expanded by somebody else later, and a `scheme:` value is a URL,
/// not a path.
fn should_rewrite(value: &str) -> bool {
    let trimmed = value.trim();
    if trimmed.is_empty()
        || trimmed.starts_with('~')
        || trimmed.starts_with('$')
        || trimmed.starts_with('%')
    {
        return false;
    }
    if trimmed.starts_with('/') || windows_absolute(trimmed) {
        return false;
    }
    // `scheme:` — a letter followed by letters, digits, `+`, `.`, `-`, then `:`.
    // `C:\x` is caught above, so this does not eat drive letters.
    let mut chars = trimmed.chars();
    match chars.next() {
        Some(first) if first.is_ascii_alphabetic() => {
            for ch in chars {
                if ch == ':' {
                    return false;
                }
                if !(ch.is_ascii_alphanumeric() || ch == '+' || ch == '.' || ch == '-') {
                    return true;
                }
            }
            true
        }
        _ => true,
    }
}

fn windows_absolute(value: &str) -> bool {
    if value.starts_with("\\\\") || value.starts_with("//") {
        return true;
    }
    let bytes = value.as_bytes();
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'\\' || bytes[2] == b'/')
}

/// Join a directory and a relative path with the separator the directory uses.
///
/// Deliberately not `Path::join`: the source directory may be a POSIX path read
/// on Windows or the reverse, and the result goes into a config another process
/// reads. `sourceConfigDir.startsWith("/") ? posix : win32` is Orca's own test
/// (:2069).
fn join_for(dir: &str, relative: &str) -> String {
    if dir.starts_with('/') {
        let base = dir.trim_end_matches('/');
        return format!("{base}/{}", relative.trim_start_matches('/'));
    }
    let base = dir.trim_end_matches(['\\', '/']);
    format!("{base}\\{}", relative.replace('/', "\\"))
}

/// Absolutise every relative path value in a config against where it came from.
pub fn rewrite_relative_paths(config: &str, source_config_dir: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut table_path = String::new();
    let mut scan = Scan::new();
    for line in config.split('\n') {
        if scan.structural() {
            if let Some(header) = toml_lines::table_header(line) {
                table_path = toml_lines::header_path(header).to_string();
                out.push(line.to_string());
            } else {
                out.push(rewrite_line(line, &table_path, source_config_dir));
            }
        } else {
            out.push(line.to_string());
        }
        scan = scan.advance(line);
    }
    out.join("\n")
}

fn rewrite_line(line: &str, table_path: &str, source_config_dir: &str) -> String {
    let Some(equals) = line.find('=') else {
        return line.to_string();
    };
    if !is_path_key(table_path, line[..equals].trim()) {
        return line.to_string();
    }
    let Some(parsed) = toml_lines::single_line_string_value(line, equals + 1) else {
        return line.to_string();
    };
    if !should_rewrite(&parsed.value) {
        return line.to_string();
    }
    let absolute = join_for(source_config_dir, &parsed.value);
    format!(
        "{}{}{}",
        &line[..parsed.start],
        toml_lines::quote_path(&absolute),
        &line[parsed.end..]
    )
}

/// One table and everything under it, as text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    /// The header line, comment excluded.
    pub header: String,
    /// The header line and its body, verbatim.
    pub block: String,
    /// Line index the header sits on.
    pub start: usize,
}

/// Split a config into its tables (`getTomlSections`, :2456-2483).
///
/// Everything before the first header is not a section — it is the preamble, and
/// the caller keeps it separately, because top-level keys belong to no table.
pub fn sections(config: &str) -> Vec<Section> {
    let lines: Vec<&str> = config.split('\n').collect();
    let mut found: Vec<Section> = Vec::new();
    let mut start: Option<(usize, String)> = None;
    let mut scan = Scan::new();

    for (index, line) in lines.iter().enumerate() {
        let header = if scan.structural() {
            toml_lines::table_header(line)
        } else {
            None
        };
        let Some(header) = header else {
            scan = scan.advance(line);
            continue;
        };
        if let Some((open_at, open_header)) = start.take() {
            found.push(Section {
                header: open_header,
                block: lines[open_at..index].join("\n"),
                start: open_at,
            });
        }
        start = Some((index, header.to_string()));
        scan = scan.advance(line);
    }
    if let Some((open_at, open_header)) = start {
        found.push(Section {
            header: open_header,
            block: lines[open_at..].join("\n"),
            start: open_at,
        });
    }
    found
}

/// `[hooks.state]` or anything under it — the runtime's own trust
/// (`isRuntimeHookTrustTomlSection`, :2487-2490).
pub fn is_hook_trust_section(header: &str) -> bool {
    let trimmed = header.trim();
    trimmed == "[hooks.state]" || trimmed.starts_with("[hooks.state.")
}

/// The path a `[projects."…"]` header names, or `None`
/// (`parseCodexProjectHeaderPath`, codex-app-server-client:716-726).
///
/// Strict about the tail: `[projects."/a"] junk` is not a project header, so a
/// malformed line cannot be read as trust for a directory.
pub fn project_header_path(header: &str) -> Option<String> {
    let trimmed = header.trim_end_matches('\r').trim_start();
    let rest = trimmed.strip_prefix('[')?;
    let rest = rest.trim_start().strip_prefix("projects")?;
    let rest = rest.trim_start().strip_prefix('.')?;
    let offset = trimmed.len() - rest.len();
    let parsed = toml_lines::single_line_string_value(trimmed, offset)?;
    let after = trimmed[parsed.end..].trim_start();
    let after = after.strip_prefix(']')?.trim_start();
    if after.is_empty() || after.starts_with('#') {
        Some(parsed.value)
    } else {
        None
    }
}

pub fn is_project_section(header: &str) -> bool {
    project_header_path(header).is_some()
}

/// Does the runtime keep this section across a re-mirror?
///
/// Exactly the two kinds the runtime owns: its hook trust and its project trust
/// (`isRuntimePreservedTomlSection`, :2484-2486). Everything else is the user's
/// and comes fresh from their config every time — which is what makes the mirror
/// a mirror rather than a fork.
pub fn is_runtime_preserved(header: &str) -> bool {
    is_hook_trust_section(header) || is_project_section(header)
}

/// The identity two sections are the same section under.
///
/// A project's identity is its normalised path, so `[projects."/a"]` and
/// `[projects."/a/"]`… are one table on Windows. On POSIX the path is compared
/// as written, because there `/A` and `/a` really are two directories
/// (`getTomlSectionHeaderKey`, :2494-2497).
pub fn section_key(header: &str) -> String {
    match project_header_path(header) {
        Some(path) => format!("project:{}", normalize_project_path(&path)),
        None => header.trim().to_string(),
    }
}

/// The identity a **revocation** is matched under — case-folded on Windows
/// (`getRevocationTomlSectionHeaderKey`, :2498-2501).
///
/// Looser than [`section_key`] on purpose, and the asymmetry is the safety: a
/// revocation should catch more spellings than a grant, never fewer.
pub fn revocation_key(header: &str) -> String {
    match project_header_path(header) {
        Some(path) => {
            let normalized = normalize_project_path(&path);
            let folded = if uses_windows_separators(&path) {
                normalized.to_lowercase()
            } else {
                normalized
            };
            format!("project:{folded}")
        }
        None => header.trim().to_string(),
    }
}

fn uses_windows_separators(path: &str) -> bool {
    windows_absolute(path) || path.starts_with("//")
}

/// `normalizeCodexProjectPathForLookup` (codex-app-server-client:533-537).
///
/// A POSIX path is returned untouched. The Windows branch folds separators and
/// lowercases; the UNC case-folding Orca does through `wsl-paths` is not ported,
/// because this product does not run there yet and a half-folded UNC path would
/// silently merge two directories.
fn normalize_project_path(path: &str) -> String {
    if !uses_windows_separators(path) {
        return path.to_string();
    }
    path.replace('\\', "/").to_lowercase()
}

/// What a project block says about trust, if it says anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectTrust {
    Trusted,
    Untrusted,
}

/// Read `trust_level` out of a project block (`getProjectTrustLevel`, :2522-2526).
///
/// First match wins, both quote styles, comment allowed. Anything else — a
/// value we do not know, no line at all — is `None`, which is neither a grant nor
/// a revocation.
pub fn project_trust(block: &str) -> Option<ProjectTrust> {
    let mut scan = Scan::new();
    for line in block.split('\n') {
        if scan.structural()
            && let Some(word) = trust_level_word(line)
        {
            return match word.as_str() {
                "trusted" => Some(ProjectTrust::Trusted),
                "untrusted" => Some(ProjectTrust::Untrusted),
                _ => None,
            };
        }
        scan = scan.advance(line);
    }
    None
}

fn trust_level_word(line: &str) -> Option<String> {
    let rest = line
        .strip_prefix(|ch| ch == ' ' || ch == '\t')
        .unwrap_or(line);
    let rest = rest.trim_start_matches([' ', '\t']);
    let rest = rest.strip_prefix("trust_level")?;
    let rest = rest.trim_start_matches([' ', '\t']).strip_prefix('=')?;
    let rest = rest.trim_start_matches([' ', '\t']);
    let quote = rest.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let inner = &rest[1..];
    let end = inner.find(quote)?;
    let tail = inner[end + 1..].trim_start_matches([' ', '\t', '\r']);
    if !tail.is_empty() && !tail.starts_with('#') {
        return None;
    }
    Some(inner[..end].to_string())
}

/// Collapse repeated `[projects."…"]` tables to one, keeping the untrusted one
/// (`deduplicateProjectTomlSections`, :2502-2521).
///
/// A config can name the same project twice — hand editing, a merge, a tool.
/// TOML says that is an error; Codex reads it anyway, and the tie-break is what
/// matters: if either copy says untrusted, that is the one that survives. The
/// alternative — first or last wins — would make whether a directory is
/// sandboxed depend on line order.
pub fn dedupe_project_sections(input: Vec<Section>) -> Vec<Section> {
    let mut out: Vec<Section> = Vec::new();
    let mut seen: Vec<(String, usize)> = Vec::new();
    for section in input {
        if !is_project_section(&section.header) {
            out.push(section);
            continue;
        }
        let key = section_key(&section.header);
        match seen
            .iter()
            .find(|(known, _)| known == &key)
            .map(|(_, at)| *at)
        {
            None => {
                seen.push((key, out.len()));
                out.push(section);
            }
            Some(at) => {
                let existing_trusted =
                    project_trust(&out[at].block) != Some(ProjectTrust::Untrusted);
                if existing_trusted
                    && project_trust(&section.block) == Some(ProjectTrust::Untrusted)
                {
                    out[at] = section;
                }
            }
        }
    }
    out
}

/// Put `trust_level` under this project's own table, touching nothing else
/// (`upsertProjectTrustLevelInContent`, config-toml-trust.ts:383-426).
///
/// Byte-preserving on purpose: this edits a file Codex also writes, so the
/// file's own EOL convention is kept, a BOM is shed the way Orca sheds it,
/// and an existing `trust_level` line is replaced in place rather than the
/// block being reprinted. The caller hands the path Codex will look the
/// project up under — canonical, a worktree already resolved to its
/// repository root.
pub fn upserted_project_trust(content: &str, project_path: &str, trust: ProjectTrust) -> String {
    let existing = content.strip_prefix('\u{feff}').unwrap_or(content);
    let eol = if existing.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let word = match trust {
        ProjectTrust::Trusted => "trusted",
        ProjectTrust::Untrusted => "untrusted",
    };
    let trust_line = format!("trust_level = \"{word}\"");

    let Some(header_line_end) = project_header_line_end(existing, project_path) else {
        let block = format!(
            "[projects.\"{}\"]{eol}{trust_line}",
            toml_lines::escape_string(project_path)
        );
        if existing.is_empty() {
            return format!("{block}{eol}");
        }
        let separator = if existing.ends_with(&format!("{eol}{eol}")) {
            ""
        } else if existing.ends_with(eol) {
            eol
        } else {
            // One join for the missing line end, one for the blank line
            // between blocks — the same two Orca appends.
            return format!("{existing}{eol}{eol}{block}{eol}");
        };
        return format!("{existing}{separator}{block}{eol}");
    };

    let block_end = next_table_header_at(existing, header_line_end).unwrap_or(existing.len());
    let block = &existing[header_line_end..block_end];
    if let Some((line_start, line_len)) = existing_trust_line(block) {
        let at = header_line_end + line_start;
        return format!(
            "{}{}{}",
            &existing[..at],
            trust_line,
            &existing[at + line_len..]
        );
    }
    format!(
        "{}{eol}{trust_line}{}",
        &existing[..header_line_end],
        &existing[header_line_end..]
    )
}

/// Read-modify-write the file itself; a missing file is an empty one.
pub fn upsert_project_trust_level(
    config_path: &Path,
    project_path: &str,
    trust: ProjectTrust,
) -> std::io::Result<()> {
    let existing = match std::fs::read_to_string(config_path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error),
    };
    let updated = upserted_project_trust(&existing, project_path, trust);
    if updated == existing {
        return Ok(());
    }
    crate::codex_install::write_atomically(config_path, &updated, None)
}

/// Byte offset just past the header line's text (before its own line end) of
/// the table that names this project, or `None` when no table does.
///
/// Walked with the scan state so a header-shaped line inside a multi-line
/// string cannot be edited as if it were structure — the discipline every
/// reader in this file already keeps.
fn project_header_line_end(content: &str, project_path: &str) -> Option<usize> {
    let wanted = normalize_project_path(project_path);
    let mut scan = Scan::new();
    let mut at = 0usize;
    for line in content.split_inclusive('\n') {
        let text = line.strip_suffix('\n').unwrap_or(line);
        if scan.structural()
            && project_header_path(text)
                .is_some_and(|named| normalize_project_path(&named) == wanted)
        {
            return Some(at + text.trim_end_matches('\r').len());
        }
        scan = scan.advance(text);
        at += line.len();
    }
    None
}

/// Byte offset where the next table header at or after `from` starts.
fn next_table_header_at(content: &str, from: usize) -> Option<usize> {
    let mut scan = Scan::new();
    let mut at = 0usize;
    for line in content.split_inclusive('\n') {
        let text = line.strip_suffix('\n').unwrap_or(line);
        if at >= from && scan.structural() && toml_lines::table_header(text).is_some() {
            return Some(at);
        }
        scan = scan.advance(text);
        at += line.len();
    }
    None
}

/// The first structural `trust_level = "trusted"|"untrusted"` line inside a
/// block, as `(offset, length)` of the replaceable text — line end excluded,
/// so the replacement keeps the file's own terminator. A `trust_level` with a
/// value this reader does not know is left standing, and the fresh line goes
/// in above it — first match wins on Codex's side, so the fresh answer is the
/// one that counts (Orca's regex draws the same boundary).
fn existing_trust_line(block: &str) -> Option<(usize, usize)> {
    let mut scan = Scan::new();
    let mut at = 0usize;
    for line in block.split_inclusive('\n') {
        let text = line.strip_suffix('\n').unwrap_or(line);
        let bare = text.strip_suffix('\r').unwrap_or(text);
        if scan.structural()
            && trust_level_word(bare).is_some_and(|word| word == "trusted" || word == "untrusted")
        {
            return Some((at, bare.len()));
        }
        scan = scan.advance(text);
        at += line.len();
    }
    None
}

/// Join blocks with one blank line, dropping the empty ones
/// (`joinTomlBlocks`, :2527-2530).
///
/// The dropping is why this is a function and not a `join`: strip a section out
/// of the middle of a config and the naive join leaves the hole behind as
/// stacking blank lines, which grows on every re-mirror.
pub fn join_blocks(blocks: &[String]) -> String {
    let kept: Vec<&str> = blocks
        .iter()
        .map(|block| block.trim())
        .filter(|block| !block.is_empty())
        .collect();
    if kept.is_empty() {
        return String::new();
    }
    format!("{}\n", kept.join("\n\n"))
}

/// Drop the sections the runtime owns, so a copy of the user's config carries no
/// trust decision into a home they have never seen
/// (`stripRuntimeOwnedTomlSections`, :2449-2455).
///
/// `runtime_project_headers` are the projects the runtime already has a section
/// for; the system's copy of those is dropped as redundant — **unless** it says
/// untrusted, in which case it is kept, because that is the revocation and it has
/// to reach the merge.
pub fn strip_runtime_owned(config: &str, runtime_project_headers: &[String]) -> String {
    let lines: Vec<&str> = config.split('\n').collect();
    let source = sections(config);
    let first = source.first().map(|section| section.start);
    let deduped = dedupe_project_sections(source);

    let mut blocks: Vec<String> = Vec::new();
    blocks.push(match first {
        None => config.to_string(),
        Some(at) => lines[..at].join("\n"),
    });
    for section in deduped {
        if is_hook_trust_section(&section.header) {
            continue;
        }
        if is_project_section(&section.header)
            && runtime_project_headers.contains(&section_key(&section.header))
            && project_trust(&section.block) != Some(ProjectTrust::Untrusted)
        {
            continue;
        }
        blocks.push(section.block);
    }
    join_blocks(&blocks)
}

/// Prepare the user's config to be read as the mirror's source.
///
/// Path rewriting only — the trust stripping is the caller's, because a fresh
/// mirror and a re-merge strip against different sets.
pub fn prepare(config: &str, source_config_dir: &str) -> String {
    normalize_deprecated_hook_flag(&rewrite_relative_paths(config, source_config_dir))
}

/// A mirror written for the first time: the user's settings, none of their trust.
pub fn seed(config: &str, source_config_dir: &str) -> String {
    strip_runtime_owned(&prepare(config, source_config_dir), &[])
}

/// Re-mirror: the user's settings again, plus the trust the runtime owns
/// (`mergeSystemCodexConfigIntoRuntime`, :2931-2938).
///
/// `system_config` must already have been through [`prepare`].
///
/// The one hard case is a project the runtime trusts and the user has since
/// revoked. **The revocation wins.** Keeping our section would mean ZeroCode
/// running agents unsandboxed in a directory the user locked down — the exact
/// decision they made in their own config, reversed by a cache. The escape hatch
/// is symmetric: a config that records the project trusted *as well* means the
/// user changed their mind back, and then ours stands.
pub fn merge(runtime_config: &str, system_config: &str) -> String {
    let runtime = dedupe_project_sections(sections(runtime_config));
    let runtime_projects: Vec<String> = runtime
        .iter()
        .filter(|section| is_project_section(&section.header))
        .map(|section| section_key(&section.header))
        .collect();

    let system_projects: Vec<Section> = dedupe_project_sections(sections(system_config))
        .into_iter()
        .filter(|section| is_project_section(&section.header))
        .collect();
    let revoked: Vec<String> = system_projects
        .iter()
        .filter(|section| project_trust(&section.block) == Some(ProjectTrust::Untrusted))
        .map(|section| revocation_key(&section.header))
        .collect();
    let regranted: Vec<String> = system_projects
        .iter()
        .filter(|section| project_trust(&section.block) == Some(ProjectTrust::Trusted))
        .map(|section| section_key(&section.header))
        .collect();

    let mut blocks = vec![strip_runtime_owned(system_config, &runtime_projects)];
    for section in runtime {
        if !is_runtime_preserved(&section.header) {
            continue;
        }
        if is_project_section(&section.header)
            && revoked.contains(&revocation_key(&section.header))
            && !regranted.contains(&section_key(&section.header))
        {
            continue;
        }
        blocks.push(section.block);
    }
    join_blocks(&blocks)
}

/// Rename Codex's old `features.codex_hooks` key to `features.hooks`
/// (`normalizeDeprecatedCodexHookFeatureFlag`, :2119-2153).
///
/// Without this, a user whose config still carries the old spelling gets a
/// mirror where hooks are off, and the hook we then write never runs — a silent
/// no-op rather than an error. A duplicate is dropped rather than renamed twice,
/// and an existing `hooks` key wins over both.
fn normalize_deprecated_hook_flag(config: &str) -> String {
    if !config.contains("codex_hooks") {
        return config.to_string();
    }
    let mut lines: Vec<String> = config.split('\n').map(str::to_string).collect();
    // Section bounds are found with the same loose header test Orca uses here —
    // this pass predates the scanner and runs before any trust is read.
    let is_header = |line: &str| {
        let trimmed = line.trim_start();
        trimmed.starts_with('[')
            && trimmed
                .trim_end_matches('\r')
                .split('#')
                .next()
                .is_some_and(|head| head.trim_end().ends_with(']'))
    };
    let is_features = |line: &str| {
        let head = line.trim_start().trim_end_matches('\r');
        let head = head.split('#').next().unwrap_or_default().trim_end();
        head == "[features]"
    };

    let mut ranges: Vec<(usize, usize)> = Vec::new();
    let mut open: Option<usize> = None;
    for index in 0..=lines.len() {
        let line = lines.get(index);
        if let Some(line) = line
            && !is_header(line)
        {
            continue;
        }
        if let Some(start) = open.take() {
            ranges.push((start, index));
        }
        if line.is_some_and(|line| is_features(line)) {
            open = Some(index);
        }
    }

    for (start, end) in ranges.into_iter().rev() {
        normalize_features_block(&mut lines, start + 1, end);
    }
    lines.join("\n")
}

fn normalize_features_block(lines: &mut Vec<String>, start: usize, end: usize) {
    let key_at = |line: &str, want: &str| -> bool {
        let trimmed = line.trim_start_matches([' ', '\t']);
        trimmed
            .strip_prefix(want)
            .is_some_and(|rest| rest.trim_start_matches([' ', '\t']).starts_with('='))
    };
    let mut deprecated: Vec<usize> = Vec::new();
    let mut has_hooks = false;
    for (index, line) in lines
        .iter()
        .enumerate()
        .take(end.min(lines.len()))
        .skip(start)
    {
        if key_at(line, "hooks") {
            has_hooks = true;
        }
        if key_at(line, "codex_hooks") {
            deprecated.push(index);
        }
    }
    if deprecated.is_empty() {
        return;
    }
    if !has_hooks {
        let first = deprecated.remove(0);
        let line = &lines[first];
        let indent: String = line
            .chars()
            .take_while(|ch| *ch == ' ' || *ch == '\t')
            .collect();
        let rest = line[indent.len()..]
            .strip_prefix("codex_hooks")
            .unwrap_or_default();
        lines[first] = format!("{indent}hooks{rest}");
    }
    for index in deprecated.into_iter().rev() {
        lines.remove(index);
    }
}

/// Write trust for our own hooks into a home we own.
///
/// Takes a [`Home`] rather than a path, which is the whole enforcement: this
/// function cannot be pointed at `~/.codex/config.toml`, so it cannot record a
/// consent decision as the user's.
///
/// Follows `buildTrustBlock`/`upsertTrustBlocks`
/// (codex-app-server-client:614-657): `enabled` first, then `trusted_hash`,
/// blocks separated by a blank line, an existing block replaced in place. The
/// replacement is in place rather than appended because the trust key carries the
/// handler's index — appending would leave the stale block above ours claiming
/// the same key, which is [`crate::codex_trust::CodexTrust::Conflicted`] and
/// trusts nothing.
pub fn write_trust(home: &Home, entries: &[TrustEntry]) -> std::io::Result<bool> {
    let path = home.config_path();
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    let mut next = existing.clone();
    for entry in entries {
        next = upsert_trust_block(&next, &trust_key(entry), &trusted_hash(entry));
    }
    if next == existing {
        return Ok(false);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, next)?;
    Ok(true)
}

/// Take our trust blocks back out, by key.
///
/// Uninstalling has to remove the trust as well as the hook. A trust entry left
/// behind for a `hooks.json` entry that is gone is dead text today and a silently
/// pre-approved hook the moment anything writes that key again.
pub fn remove_trust(home: &Home, keys: &[String]) -> std::io::Result<bool> {
    remove_trust_at(&home.config_path(), keys)
}

/// The same sweep against any `config.toml` — the system home's carries the
/// residue of a landing that planted trust and then retreated, and only its
/// path differs from the mirror's.
pub fn remove_trust_at(path: &std::path::Path, keys: &[String]) -> std::io::Result<bool> {
    let Ok(existing) = std::fs::read_to_string(path) else {
        return Ok(false);
    };
    let mut next = existing.clone();
    for key in keys {
        while let Some((start, _, end)) = find_trust_block(&next, key) {
            let tail = next[end..].trim_start_matches('\n');
            next = format!("{}{}", &next[..start], tail);
        }
    }
    if next == existing {
        return Ok(false);
    }
    std::fs::write(path, next)?;
    Ok(true)
}

/// One trust block, as Codex writes it.
pub fn trust_block(key: &str, hash: &str, enabled: bool) -> String {
    format!(
        "[hooks.state.\"{}\"]\nenabled = {enabled}\ntrusted_hash = \"{}\"",
        toml_lines::escape_string(key),
        toml_lines::escape_string(hash)
    )
}

/// Replace this key's block, or append one.
///
/// An existing `enabled = false` is carried forward: the user switched this hook
/// off in a home we own, and re-writing trust is not a reason to switch it back
/// on (`upsertTrustBlocks`'s `explicitEnabled ?? !ranges.some(…false)`, :635-654).
fn upsert_trust_block(content: &str, key: &str, hash: &str) -> String {
    let Some((start, header_end, end)) = find_trust_block(content, key) else {
        let block = trust_block(key, hash, true);
        if content.is_empty() {
            return format!("{block}\n");
        }
        let gap = if content.ends_with("\n\n") {
            ""
        } else if content.ends_with('\n') {
            "\n"
        } else {
            "\n\n"
        };
        return format!("{content}{gap}{block}\n");
    };
    let enabled = !body_says_disabled(&content[header_end..end]);
    // Replace the block's TEXT and keep the whitespace that separated it from
    // what follows. Splicing at `end` instead would swallow the blank line, so
    // re-writing the same trust would keep changing the file — and a launch that
    // rewrites config.toml every time is one that looks edited to anything
    // watching it.
    let content_end = content[..end].trim_end().len();
    let tail = &content[content_end..];
    format!(
        "{}{}{}",
        &content[..start],
        trust_block(key, hash, enabled),
        if tail.is_empty() { "\n" } else { tail }
    )
}

fn body_says_disabled(body: &str) -> bool {
    let mut scan = Scan::new();
    for line in body.split('\n') {
        if scan.structural() {
            let trimmed = line.trim_start_matches([' ', '\t']);
            if let Some(rest) = trimmed.strip_prefix("enabled") {
                let rest = rest.trim_start_matches([' ', '\t']);
                if let Some(rest) = rest.strip_prefix('=') {
                    let rest = rest.trim_start_matches([' ', '\t']);
                    if let Some(tail) = rest.strip_prefix("false") {
                        let tail = tail.trim_start_matches([' ', '\t', '\r']);
                        if tail.is_empty() || tail.starts_with('#') {
                            return true;
                        }
                    }
                }
            }
        }
        scan = scan.advance(line);
    }
    false
}

/// `(block start, end of header line, block end)` for this trust key.
fn find_trust_block(content: &str, key: &str) -> Option<(usize, usize, usize)> {
    let mut cursor = 0usize;
    let mut scan = Scan::new();
    while cursor < content.len() {
        let (line_end, next) = match content[cursor..].find('\n') {
            Some(offset) => (cursor + offset, cursor + offset + 1),
            None => (content.len(), content.len()),
        };
        let raw = &content[cursor..line_end];
        let line = raw.trim_end_matches('\r');
        let line = if cursor == 0 {
            line.trim_start_matches('\u{feff}')
        } else {
            line
        };
        if scan.structural()
            && let Some(found) = crate::codex_trust::hook_state_header(line)
            && found == key
        {
            let header_end = line_end - (raw.len() - line.trim_end_matches('\r').len());
            let end = match toml_lines::find_next_table_header(&content[header_end..]) {
                Some(offset) => header_end + offset,
                None => content.len(),
            };
            return Some((cursor, header_end, end));
        }
        scan = scan.advance(line);
        if next == content.len() && line_end == content.len() {
            return None;
        }
        cursor = next;
    }
    None
}

/// What a mirror sync did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sync {
    /// Written for the first time: the user's settings, no trust.
    Seeded,
    /// Re-mirrored over an existing one, keeping the runtime's trust.
    Merged,
    /// Already matched what a mirror of the current system config would be.
    Unchanged,
    /// The user has no `config.toml` at all, so there is nothing to mirror. The
    /// mirror's own config is left as it is — an empty source is not a reason to
    /// throw away the trust we wrote.
    NoSource,
}

/// Bring the mirror's `config.toml` up to date with the user's.
///
/// Called before every launch that uses the mirror, because the user may have
/// changed a setting since the last one. Idempotent, and reports [`Sync::Unchanged`]
/// when it was — a write on every launch would touch mtime and make the file look
/// edited to anything watching it.
pub fn sync(home: &Home, system_home: &Path) -> std::io::Result<Sync> {
    let system_config = system_home.join("config.toml");
    let raw = std::fs::read_to_string(&system_config).unwrap_or_default();
    if raw.trim().is_empty() {
        return Ok(Sync::NoSource);
    }
    let source_dir = system_home.to_string_lossy().to_string();
    let prepared = prepare(&raw, &source_dir);

    let target = home.config_path();
    let existing = std::fs::read_to_string(&target).ok();
    let (next, how) = match &existing {
        None => (seed(&raw, &source_dir), Sync::Seeded),
        Some(current) => (merge(current, &prepared), Sync::Merged),
    };
    if existing.as_deref() == Some(next.as_str()) {
        return Ok(Sync::Unchanged);
    }
    std::fs::create_dir_all(home.path())?;
    std::fs::write(&target, next)?;
    Ok(how)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(event_label: &str) -> TrustEntry {
        TrustEntry {
            source_path: PathBuf::from("/data/codex-runtime-home/home/hooks.json"),
            event_label: event_label.into(),
            group_index: 0,
            handler_index: 0,
            command: "/bin/sh '/h/.zerocode/agent-hooks/codex-hook.sh'".into(),
            timeout_seconds: Some(10),
            async_hook: false,
            matcher: None,
            status_message: None,
        }
    }

    /// The type is the boundary: a `Home` cannot name the user's `~/.codex`, and
    /// `write_trust` takes nothing else.
    #[test]
    fn a_home_can_only_ever_be_one_we_made() {
        let home = Home::under(Path::new("/data/zerocode"));
        assert_eq!(
            home.path(),
            Path::new("/data/zerocode/codex-runtime-home/home")
        );
        assert!(
            home.config_path()
                .ends_with("codex-runtime-home/home/config.toml")
        );
        // Even handed the user's own Codex home, the constructor descends into a
        // subdirectory of it rather than pointing at it.
        let confused = Home::under(Path::new("/h/.codex"));
        assert_ne!(confused.config_path(), Path::new("/h/.codex/config.toml"));
        assert!(confused.path().starts_with("/h/.codex/codex-runtime-home"));
    }

    /// A relative path means a different directory in the copy, so every
    /// path-valued key is absolutised — and only those keys.
    #[test]
    fn a_relative_path_is_pinned_to_where_it_came_from() {
        let config = concat!(
            "log_dir = \"logs\"\n",
            "model = \"gpt-5\"\n",
            "notify_relative = \"logs\"\n",
            "[agents.mine]\n",
            "config_file = \"a/b.toml\"\n",
            "[model_providers.p.auth]\n",
            "cwd = \"work\"\n",
            "[profiles.fast]\n",
            "model_catalog_json = \"cat.json\"\n",
            "model = \"x\"\n",
        );
        let out = rewrite_relative_paths(config, "/h/.codex");
        assert!(out.contains("log_dir = '/h/.codex/logs'"), "{out}");
        assert!(out.contains("config_file = '/h/.codex/a/b.toml'"), "{out}");
        assert!(out.contains("cwd = '/h/.codex/work'"), "{out}");
        assert!(
            out.contains("model_catalog_json = '/h/.codex/cat.json'"),
            "{out}"
        );
        // Not a path key, so not touched even though the value looks like one.
        assert!(out.contains("notify_relative = \"logs\""), "{out}");
        assert!(out.contains("model = \"gpt-5\""), "{out}");

        // The four value shapes somebody else expands, plus one already absolute.
        for value in ["~/logs", "$HOME/logs", "%APPDATA%/logs", "/var/logs"] {
            let line = format!("log_dir = \"{value}\"\n");
            assert_eq!(rewrite_relative_paths(&line, "/h/.codex"), line, "{value}");
        }
        // A URL is not a path.
        let url = "log_dir = \"https://example.test/x\"\n";
        assert_eq!(rewrite_relative_paths(url, "/h/.codex"), url);
        // A Windows source directory joins with backslashes.
        assert!(
            rewrite_relative_paths("log_dir = \"logs\"\n", "C:\\Users\\j\\.codex")
                .contains(r"log_dir = 'C:\Users\j\.codex\logs'")
        );
        // A comment on the line survives the rewrite.
        assert!(
            rewrite_relative_paths("log_dir = \"logs\" # mine\n", "/h/.codex")
                .contains("log_dir = '/h/.codex/logs' # mine")
        );
        // A path key inside a multi-line string is not a key.
        let quoted = "note = \"\"\"\nlog_dir = \"logs\"\n\"\"\"\n";
        assert_eq!(rewrite_relative_paths(quoted, "/h/.codex"), quoted);
    }

    /// Splitting into tables, and what counts as one.
    #[test]
    fn a_config_splits_into_the_tables_a_reader_would_see() {
        let config = concat!(
            "model = \"gpt-5\"\n",
            "[hooks.state]\n",
            "[hooks.state.\"/a:stop:0:0\"]\n",
            "trusted_hash = \"sha256:x\"\n",
            "[projects.\"/w\"]\n",
            "trust_level = \"trusted\"\n",
            "[other]\n",
            "k = 1\n",
        );
        let found = sections(config);
        assert_eq!(
            found
                .iter()
                .map(|one| one.header.as_str())
                .collect::<Vec<_>>(),
            vec![
                "[hooks.state]",
                "[hooks.state.\"/a:stop:0:0\"]",
                "[projects.\"/w\"]",
                "[other]"
            ]
        );
        // The preamble is not a section.
        assert_eq!(found[0].start, 1);
        assert!(found[1].block.contains("trusted_hash"));

        assert!(is_hook_trust_section("[hooks.state]"));
        assert!(is_hook_trust_section(" [hooks.state.\"k\"] "));
        assert!(!is_hook_trust_section("[hooks]"));
        assert!(!is_hook_trust_section("[hooks.statement]"));

        assert_eq!(project_header_path("[projects.\"/w\"]"), Some("/w".into()));
        assert_eq!(
            project_header_path("[ projects . '/w' ] # c"),
            Some("/w".into())
        );
        // Strict tail: junk after the bracket is not a project header, so it
        // cannot be read as a trust decision about a directory.
        assert_eq!(project_header_path("[projects.\"/w\"] junk"), None);
        assert_eq!(project_header_path("[projectsx.\"/w\"]"), None);
        assert_eq!(project_header_path("[other]"), None);
    }

    /// The trust read out of a project block, and the tie-break when a project
    /// is named twice.
    #[test]
    fn a_project_named_twice_keeps_the_untrusted_one() {
        assert_eq!(
            project_trust("[projects.\"/w\"]\ntrust_level = \"trusted\"\n"),
            Some(ProjectTrust::Trusted)
        );
        assert_eq!(
            project_trust("[projects.\"/w\"]\ntrust_level = 'untrusted' # locked\n"),
            Some(ProjectTrust::Untrusted)
        );
        assert_eq!(project_trust("[projects.\"/w\"]\nk = 1\n"), None);
        assert_eq!(
            project_trust("[projects.\"/w\"]\ntrust_level = \"maybe\"\n"),
            None
        );
        // Inside a string it says nothing.
        assert_eq!(
            project_trust("[projects.\"/w\"]\nnote = \"\"\"\ntrust_level = \"trusted\"\n\"\"\"\n"),
            None
        );

        // Order must not decide whether a directory is sandboxed.
        let trusted_first = concat!(
            "[projects.\"/w\"]\ntrust_level = \"trusted\"\n",
            "[projects.\"/w\"]\ntrust_level = \"untrusted\"\n",
        );
        let untrusted_first = concat!(
            "[projects.\"/w\"]\ntrust_level = \"untrusted\"\n",
            "[projects.\"/w\"]\ntrust_level = \"trusted\"\n",
        );
        for config in [trusted_first, untrusted_first] {
            let kept = dedupe_project_sections(sections(config));
            assert_eq!(kept.len(), 1, "{config}");
            assert_eq!(
                project_trust(&kept[0].block),
                Some(ProjectTrust::Untrusted),
                "{config}"
            );
        }
    }

    /// A fresh mirror carries the settings and none of the trust.
    #[test]
    fn a_fresh_mirror_takes_the_settings_and_leaves_the_trust() {
        let system = concat!(
            "model = \"gpt-5\"\n",
            "log_dir = \"logs\"\n",
            "\n",
            "[hooks.state]\n",
            "\n",
            "[hooks.state.\"/h/.codex/hooks.json:stop:0:0\"]\n",
            "enabled = true\n",
            "trusted_hash = \"sha256:theirs\"\n",
            "\n",
            "[projects.\"/w\"]\n",
            "trust_level = \"trusted\"\n",
        );
        let seeded = seed(system, "/h/.codex");
        assert!(seeded.contains("model = \"gpt-5\""));
        assert!(seeded.contains("log_dir = '/h/.codex/logs'"));
        // Their hook trust does not become ours.
        assert!(
            !seeded.contains("hooks.state"),
            "the user's trust was copied into a home they have never seen: {seeded}"
        );
        assert!(!seeded.contains("sha256:theirs"));
        // Their project trust DOES come along — it is a decision about a
        // directory, and the mirror runs agents in the same directories.
        assert!(seeded.contains("[projects.\"/w\"]"), "{seeded}");
        // No stacking blank lines where the sections were removed.
        assert!(!seeded.contains("\n\n\n"), "{seeded:?}");
    }

    /// Re-mirroring keeps the trust we wrote, and takes the user's settings
    /// again.
    #[test]
    fn a_remirror_keeps_our_trust_and_retakes_their_settings() {
        let runtime = concat!(
            "model = \"gpt-5\"\n",
            "\n",
            "[hooks.state.\"/m/hooks.json:stop:0:0\"]\n",
            "enabled = true\n",
            "trusted_hash = \"sha256:ours\"\n",
            "\n",
            "[projects.\"/w\"]\n",
            "trust_level = \"trusted\"\n",
        );
        let system = "model = \"gpt-5-codex\"\napproval_policy = \"never\"\n";
        let merged = merge(runtime, &prepare(system, "/h/.codex"));

        // Their setting change arrives, and the old value does not linger.
        assert!(merged.contains("model = \"gpt-5-codex\""), "{merged}");
        assert!(!merged.contains("\"gpt-5\"\n"), "{merged}");
        assert!(merged.contains("approval_policy = \"never\""));
        // Our trust survives, so the hook does not stall on the next launch.
        assert!(merged.contains("sha256:ours"), "{merged}");
        assert!(merged.contains("[projects.\"/w\"]"), "{merged}");
    }

    /// The one case where the two rules collide. The user's revocation wins —
    /// and their re-grant wins over that.
    #[test]
    fn a_revoked_project_loses_the_trust_the_mirror_was_holding() {
        let runtime = concat!(
            "[projects.\"/w\"]\n",
            "trust_level = \"trusted\"\n",
            "\n",
            "[hooks.state.\"/m/hooks.json:stop:0:0\"]\n",
            "trusted_hash = \"sha256:ours\"\n",
        );

        let revoked = merge(runtime, "[projects.\"/w\"]\ntrust_level = \"untrusted\"\n");
        assert!(
            revoked.contains("trust_level = \"untrusted\""),
            "the revocation did not survive: {revoked}"
        );
        assert!(
            !revoked.contains("trust_level = \"trusted\""),
            "a revoked project kept the mirror's trust: {revoked}"
        );
        // Hook trust is a different decision and is not collateral.
        assert!(revoked.contains("sha256:ours"), "{revoked}");

        // Named untrusted and trusted both: they changed their mind back, so the
        // mirror's section stands.
        let regranted = merge(
            runtime,
            concat!(
                "[projects.\"/w\"]\ntrust_level = \"untrusted\"\n",
                "[projects.\"/w/\"]\ntrust_level = \"trusted\"\n",
            ),
        );
        assert!(regranted.contains("sha256:ours"));

        // A project the user says nothing about keeps the mirror's answer.
        let quiet = merge(runtime, "model = \"gpt-5\"\n");
        assert!(quiet.contains("trust_level = \"trusted\""), "{quiet}");
    }

    /// The old feature-flag spelling, or the hook silently never runs.
    #[test]
    fn the_deprecated_hook_flag_is_renamed_not_duplicated() {
        let renamed = prepare("[features]\ncodex_hooks = true\n", "/h/.codex");
        assert!(renamed.contains("hooks = true"), "{renamed}");
        assert!(!renamed.contains("codex_hooks"), "{renamed}");

        // An existing `hooks` key wins; the deprecated line is dropped.
        let both = prepare(
            "[features]\nhooks = false\ncodex_hooks = true\n",
            "/h/.codex",
        );
        assert!(both.contains("hooks = false"), "{both}");
        assert!(!both.contains("codex_hooks"), "{both}");

        // Only inside `[features]`.
        let elsewhere = prepare("[other]\ncodex_hooks = true\n", "/h/.codex");
        assert!(elsewhere.contains("codex_hooks = true"), "{elsewhere}");

        // Indentation is kept.
        let indented = prepare("[features]\n  codex_hooks = true\n", "/h/.codex");
        assert!(indented.contains("  hooks = true"), "{indented:?}");
    }

    /// Writing trust: appended once, replaced in place after, and an existing
    /// `enabled = false` is not overruled.
    #[test]
    fn trust_is_written_once_and_replaced_in_place() {
        let dir = tempfile::tempdir().expect("temp");
        let home = Home::under(dir.path());
        let entries = [entry("stop"), entry("pre_tool_use")];

        assert!(write_trust(&home, &entries).expect("write"));
        let first = std::fs::read_to_string(home.config_path()).expect("read");
        assert_eq!(first.matches("[hooks.state.").count(), 2, "{first}");
        assert!(first.contains(&trusted_hash(&entries[0])), "{first}");
        assert!(first.contains("enabled = true"), "{first}");

        // Writing the same thing again changes nothing, so mtime does not move.
        assert!(!write_trust(&home, &entries).expect("write"));
        assert_eq!(
            std::fs::read_to_string(home.config_path()).expect("read"),
            first
        );

        // A changed command re-hashes: the block is replaced, not stacked.
        let mut moved = entries[0].clone();
        moved.command = "/bin/sh '/elsewhere.sh'".into();
        assert!(write_trust(&home, &[moved.clone()]).expect("write"));
        let second = std::fs::read_to_string(home.config_path()).expect("read");
        assert_eq!(second.matches("[hooks.state.").count(), 2, "{second}");
        assert!(second.contains(&trusted_hash(&moved)), "{second}");
        assert!(
            !second.contains(&trusted_hash(&entries[0])),
            "the stale hash stayed behind and now the key has two: {second}"
        );
        // Which is the thing that matters: the reader still calls it trusted.
        let states = crate::codex_trust::read_trust_states(&home.config_path());
        assert_eq!(
            crate::codex_trust::trust_of(&moved, &states),
            crate::codex_trust::CodexTrust::Trusted,
            "{second}"
        );

        // A hook switched off stays off through a re-write.
        let off = second.replace("enabled = true", "enabled = false");
        std::fs::write(home.config_path(), &off).expect("write");
        write_trust(&home, &[moved.clone()]).expect("write");
        let third = std::fs::read_to_string(home.config_path()).expect("read");
        assert!(third.contains("enabled = false"), "{third}");
    }

    /// A trust write must not disturb the settings around it.
    #[test]
    fn writing_trust_leaves_the_rest_of_the_config_alone() {
        let dir = tempfile::tempdir().expect("temp");
        let home = Home::under(dir.path());
        std::fs::create_dir_all(home.path()).expect("mkdir");
        let before = concat!(
            "# my settings\n",
            "model = \"gpt-5\"   # deliberate spacing\n",
            "\n",
            "[projects.\"/w\"]\n",
            "trust_level = \"trusted\"\n",
        );
        std::fs::write(home.config_path(), before).expect("write");

        write_trust(&home, &[entry("stop")]).expect("write");
        let after = std::fs::read_to_string(home.config_path()).expect("read");
        assert!(after.starts_with(before), "{after}");
        assert!(after.contains("[hooks.state."), "{after}");
    }

    /// Syncing: seed, then merge, then notice there is nothing to do.
    #[test]
    fn a_sync_seeds_then_merges_then_stops_writing() {
        let dir = tempfile::tempdir().expect("temp");
        let system = dir.path().join("dot-codex");
        std::fs::create_dir_all(&system).expect("mkdir");
        let home = Home::under(dir.path());

        // Nothing to mirror yet.
        assert_eq!(sync(&home, &system).expect("sync"), Sync::NoSource);
        assert!(!home.config_path().exists());

        std::fs::write(
            system.join("config.toml"),
            "model = \"gpt-5\"\nlog_dir = \"logs\"\n",
        )
        .expect("write");
        assert_eq!(sync(&home, &system).expect("sync"), Sync::Seeded);
        let seeded = std::fs::read_to_string(home.config_path()).expect("read");
        assert!(seeded.contains("log_dir = '"), "{seeded}");
        assert!(
            seeded.contains(system.to_string_lossy().as_ref()),
            "{seeded}"
        );

        // Nothing changed on either side.
        assert_eq!(sync(&home, &system).expect("sync"), Sync::Unchanged);

        // Our trust, then their setting change.
        write_trust(&home, &[entry("stop")]).expect("write");
        std::fs::write(system.join("config.toml"), "model = \"gpt-5-codex\"\n").expect("write");
        assert_eq!(sync(&home, &system).expect("sync"), Sync::Merged);
        let merged = std::fs::read_to_string(home.config_path()).expect("read");
        assert!(merged.contains("gpt-5-codex"), "{merged}");
        assert!(merged.contains(&trusted_hash(&entry("stop"))), "{merged}");
        // And the trust still reads as trust after a round trip through merge.
        let states = crate::codex_trust::read_trust_states(&home.config_path());
        assert!(
            crate::codex_trust::is_trusted(&entry("stop"), &states),
            "{merged}"
        );

        // A user who deletes their config does not lose the mirror's trust.
        std::fs::write(system.join("config.toml"), "\n").expect("write");
        assert_eq!(sync(&home, &system).expect("sync"), Sync::NoSource);
        let kept = std::fs::read_to_string(home.config_path()).expect("read");
        assert!(kept.contains(&trusted_hash(&entry("stop"))), "{kept}");
    }

    /// The project-trust upsert, against the shapes a real config.toml takes
    /// (`upsertProjectTrustLevelInContent`'s own contract).
    #[test]
    fn project_trust_lands_in_its_own_table_and_nowhere_else() {
        let put = |content: &str, path: &str| {
            upserted_project_trust(content, path, ProjectTrust::Trusted)
        };

        // An empty file becomes exactly the one block.
        assert_eq!(
            put("", "/w/repo"),
            "[projects.\"/w/repo\"]\ntrust_level = \"trusted\"\n"
        );

        // Appending keeps the user's text byte-for-byte and separates blocks
        // with one blank line — with or without their trailing newline.
        assert_eq!(
            put("model = \"gpt-5\"\n", "/w/repo"),
            "model = \"gpt-5\"\n\n[projects.\"/w/repo\"]\ntrust_level = \"trusted\"\n"
        );
        assert_eq!(
            put("model = \"gpt-5\"", "/w/repo"),
            "model = \"gpt-5\"\n\n[projects.\"/w/repo\"]\ntrust_level = \"trusted\"\n"
        );

        // An existing table gains the line right under its header; its other
        // keys and its neighbours stay untouched.
        let grown = put(
            "[projects.\"/w/repo\"]\nnote = \"kept\"\n\n[projects.\"/other\"]\ntrust_level = \"untrusted\"\n",
            "/w/repo",
        );
        assert_eq!(
            grown,
            "[projects.\"/w/repo\"]\ntrust_level = \"trusted\"\nnote = \"kept\"\n\n[projects.\"/other\"]\ntrust_level = \"untrusted\"\n"
        );

        // An existing answer is replaced in place, indentation and all gone
        // the way Orca's regex takes the whole line.
        assert_eq!(
            put(
                "[projects.\"/w/repo\"]\n  trust_level = \"untrusted\"\nnote = \"kept\"\n",
                "/w/repo"
            ),
            "[projects.\"/w/repo\"]\ntrust_level = \"trusted\"\nnote = \"kept\"\n"
        );

        // A value this reader does not know is left standing; the fresh line
        // goes in above it, where Codex's first-match read finds it first.
        let over = put(
            "[projects.\"/w/repo\"]\ntrust_level = \"weird\"\n",
            "/w/repo",
        );
        assert_eq!(
            over,
            "[projects.\"/w/repo\"]\ntrust_level = \"trusted\"\ntrust_level = \"weird\"\n"
        );

        // The file's own conventions survive: CRLF stays CRLF, a BOM is shed.
        assert_eq!(
            put("model = \"gpt-5\"\r\n", "/w/repo"),
            "model = \"gpt-5\"\r\n\r\n[projects.\"/w/repo\"]\r\ntrust_level = \"trusted\"\r\n"
        );
        assert_eq!(
            put(
                "\u{feff}[projects.\"/w/repo\"]\ntrust_level = \"untrusted\"\n",
                "/w/repo"
            ),
            "[projects.\"/w/repo\"]\ntrust_level = \"trusted\"\n"
        );

        // A header-shaped line inside a multi-line string is text, not
        // structure — the same discipline the readers here keep.
        let fooled = put("note = \"\"\"\n[projects.\"/w/repo\"]\n\"\"\"\n", "/w/repo");
        assert!(
            fooled.ends_with("[projects.\"/w/repo\"]\ntrust_level = \"trusted\"\n")
                && fooled.starts_with("note = \"\"\"\n"),
            "{fooled}"
        );

        // A path with TOML-hostile characters is escaped Codex's way.
        assert_eq!(
            put("", "/w/it\"s\\here"),
            "[projects.\"/w/it\\\"s\\\\here\"]\ntrust_level = \"trusted\"\n"
        );

        // Idempotent through the file door: the second write is no write.
        let dir = tempfile::tempdir().expect("temp");
        let config = dir.path().join("config.toml");
        upsert_project_trust_level(&config, "/w/repo", ProjectTrust::Trusted).expect("first");
        let first = std::fs::read_to_string(&config).expect("read");
        let stamp = std::fs::metadata(&config).expect("meta").modified().ok();
        upsert_project_trust_level(&config, "/w/repo", ProjectTrust::Trusted).expect("second");
        assert_eq!(std::fs::read_to_string(&config).expect("read"), first);
        if let (Some(before), Ok(meta)) = (stamp, std::fs::metadata(&config)) {
            assert_eq!(
                meta.modified().ok(),
                Some(before),
                "an unchanged upsert rewrote the file"
            );
        }
    }
}
