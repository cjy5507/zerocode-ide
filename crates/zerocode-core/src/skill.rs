//! Finding the skills an agent can already use.
//!
//! Orca's `SkillsPage` (SkillsPage-CJSxvNGJ.js, and `discoverSkills` /
//! `buildSkillDiscoverySources` / `scanRoot` in out/main/index.js:98860-99260,
//! 1.4.164). A skill is a directory holding a `SKILL.md`, and every agent family
//! keeps them somewhere different — so the page's whole job is knowing where to
//! look and reading what it finds.
//!
//! ## Reading, never writing
//!
//! Nothing here creates, edits or deletes a skill. These are files the user and
//! their agents own, in eleven directories this product does not manage, and the
//! measured surface is a **list with filters** — Orca's own row offers exactly one
//! action, "reveal the file". A page that could rewrite somebody's `SKILL.md`
//! would be a different and much more dangerous feature than the one measured.
//!
//! ## The name is what the file says it is
//!
//! In order: the frontmatter's `name`, then the body's first `# heading`, then the
//! directory's own name. Orca's fallback chain, and it matters because a skill
//! with no frontmatter is common — the directory name is the one thing that
//! always exists.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The file that makes a directory a skill.
pub const SKILL_FILE: &str = "SKILL.md";

/// How deep a scan goes. Orca's numbers: four levels for an ordinary root, nine
/// inside a plugin cache — a cache nests by plugin, then by version, then by the
/// plugin's own layout, so a shallow walk finds nothing there.
pub const MAX_DEPTH: usize = DEFAULT_POLICY.max_depth;
pub const MAX_PLUGIN_DEPTH: usize = DEFAULT_POLICY.max_plugin_depth;

/// A cap on how much of a `SKILL.md` is read for its summary.
///
/// Only the frontmatter and the first paragraph are wanted, and both are at the
/// top. Without a cap this walks whatever somebody put in a file that happens to
/// be named `SKILL.md` — and these are directories full of arbitrary content.
pub const MAX_SUMMARY_BYTES: usize = DEFAULT_POLICY.max_summary_bytes;

/// Where a skill came from, which is what the row's badge says.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SourceKind {
    /// One of the agent homes under `~`.
    Home,
    /// Shipped with the agent rather than written by anybody — Orca's `.system`
    /// subdirectory of a home root.
    Bundled,
    /// A checkout's own `.agents/` or `.claude/`.
    Repo,
    /// A plugin cache, which nests far deeper than the others.
    Plugin,
}

/// A directory that may hold skills.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SkillSource {
    pub id: String,
    pub label: String,
    pub path: PathBuf,
    pub kind: SourceKind,
    /// Which agent families read this directory. More than one for the shared
    /// `agent-skills` convention.
    pub providers: Vec<String>,
    /// The single agent that owns it, where there is one.
    pub owner: Option<String>,
}

impl SkillSource {
    fn new(
        id: &str,
        label: &str,
        path: PathBuf,
        kind: SourceKind,
        providers: &[&str],
        owner: Option<&str>,
    ) -> Self {
        Self {
            id: id.to_string(),
            label: label.to_string(),
            path,
            kind,
            providers: providers.iter().map(|one| one.to_string()).collect(),
            owner: owner.map(str::to_string),
        }
    }

    fn max_depth(&self, policy: &Policy) -> usize {
        if self.kind == SourceKind::Plugin {
            policy.max_plugin_depth
        } else {
            policy.max_depth
        }
    }
}

/// Every directory to look in, for this machine and these checkouts.
///
/// The eleven home roots are Orca's table verbatim
/// (`buildSkillDiscoverySources`, :98952-98963) — and they can be copied because
/// they are not Orca's own layout, they are **where those tools keep their
/// skills**, which is a fact about each tool. Two more per checkout, because a
/// repository can carry skills for the people working in it.
pub fn discovery_sources(home: &Path, repos: &[PathBuf]) -> Vec<SkillSource> {
    let at = |parts: &[&str]| -> PathBuf {
        let mut path = home.to_path_buf();
        for part in parts {
            path.push(part);
        }
        path
    };
    let mut roots = vec![
        SkillSource::new(
            "home-codex",
            "Codex home",
            at(&[".codex", "skills"]),
            SourceKind::Home,
            &["codex"],
            Some("codex"),
        ),
        SkillSource::new(
            "home-agents",
            "Agent skills home",
            at(&[".agents", "skills"]),
            SourceKind::Home,
            &["agent-skills"],
            None,
        ),
        SkillSource::new(
            "home-claude",
            "Claude home",
            at(&[".claude", "skills"]),
            SourceKind::Home,
            &["claude"],
            Some("claude"),
        ),
        // Ours: zo reads `~/.zo/skills` (its `zo_global_skill_roots`), which
        // Orca's table cannot know about.
        SkillSource::new(
            "home-zo",
            "zo home",
            at(&[".zo", "skills"]),
            SourceKind::Home,
            &["zo"],
            Some("zo"),
        ),
        SkillSource::new(
            "codex-plugin-cache",
            "Codex plugin cache",
            at(&[".codex", "plugins", "cache"]),
            SourceKind::Plugin,
            &["codex", "agent-skills"],
            Some("codex"),
        ),
        SkillSource::new(
            "home-grok",
            "Grok home",
            at(&[".grok", "skills"]),
            SourceKind::Home,
            &["agent-skills"],
            Some("grok"),
        ),
        SkillSource::new(
            "home-opencode",
            "OpenCode home",
            at(&[".config", "opencode", "skills"]),
            SourceKind::Home,
            &["agent-skills"],
            Some("opencode"),
        ),
        SkillSource::new(
            "home-pi",
            "Pi home",
            at(&[".pi", "agent", "skills"]),
            SourceKind::Home,
            &["agent-skills"],
            Some("pi"),
        ),
        SkillSource::new(
            "home-prime-agent",
            "Prime Agent home",
            at(&[".prime", "agent", "skills"]),
            SourceKind::Home,
            &["agent-skills"],
            Some("prime-agent"),
        ),
        SkillSource::new(
            "home-omp",
            "OMP home",
            at(&[".omp", "agent", "skills"]),
            SourceKind::Home,
            &["agent-skills"],
            Some("omp"),
        ),
        SkillSource::new(
            "home-antigravity",
            "Antigravity home",
            at(&[".gemini", "antigravity", "skills"]),
            SourceKind::Home,
            &["agent-skills"],
            Some("antigravity"),
        ),
        SkillSource::new(
            "home-cursor",
            "Cursor home",
            at(&[".cursor", "skills"]),
            SourceKind::Home,
            &["agent-skills"],
            Some("cursor"),
        ),
    ];
    for target in INSTALL_TARGETS {
        let Some(relative) = target.extra_home else {
            continue;
        };
        let path = home.join(relative);
        if roots.iter().any(|root| root.path == path) {
            continue;
        }
        let label = crate::agent::agent_spec(target.agent).map_or(target.agent, |spec| spec.name);
        roots.push(SkillSource::new(
            &format!("home-{}", target.agent),
            label,
            path,
            SourceKind::Home,
            &[target.agent],
            Some(target.agent),
        ));
    }
    for repo in repos {
        let name = repo
            .file_name()
            .map(|held| held.to_string_lossy().into_owned())
            .unwrap_or_else(|| repo.to_string_lossy().into_owned());
        let stamp = path_id(repo);
        roots.push(SkillSource::new(
            &format!("repo-agents-{stamp}"),
            &format!("{name} .agents"),
            repo.join(".agents").join("skills"),
            SourceKind::Repo,
            &["agent-skills"],
            None,
        ));
        roots.push(SkillSource::new(
            &format!("repo-claude-{stamp}"),
            &format!("{name} .claude"),
            repo.join(".claude").join("skills"),
            SourceKind::Repo,
            &["claude"],
            Some("claude"),
        ));
    }
    roots
}

/// A short stable id for a path.
///
/// Orca uses the first 16 hex of a sha1. Ours is a plain FNV-1a, because the only
/// property needed is that the same path yields the same id twice — nothing here
/// is a security boundary, and a hash crate for a DOM key would be weight for
/// nothing.
pub fn path_id(path: &Path) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in path.to_string_lossy().as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// What a `SKILL.md` says about itself.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Summary {
    pub name: Option<String>,
    pub description: Option<String>,
    pub version: Option<String>,
}

/// Read the frontmatter keys this page uses, and nothing else.
///
/// A deliberately small reader, for the reason this repo already chose once for
/// `zerocode.yaml`: a strict reader of a measured shape can only fail to find a
/// name, while a general parser that is subtly wrong about somebody's file can
/// report the wrong one. Two forms beyond a plain scalar are handled because
/// skills use them — a block scalar (`|` or `>`) for a long description, and a
/// dash list.
fn frontmatter_value(block: &str, want: &str) -> Option<String> {
    let lines: Vec<&str> = block.lines().collect();
    let mut at = 0;
    while at < lines.len() {
        let line = lines[at];
        let Some((key, rest)) = line.split_once(':') else {
            at += 1;
            continue;
        };
        // Only top-level keys. An indented `name:` belongs to something nested and
        // is not this document's name.
        if key.starts_with(char::is_whitespace) {
            at += 1;
            continue;
        }
        let key = key.trim();
        let value = rest.trim();
        if key != want {
            at += 1;
            continue;
        }
        // A block scalar: the indented lines that follow are the value. `>` folds
        // them with spaces, `|` keeps the newlines — and either way the result is
        // squeezed, because this lands in a one-line row.
        if matches!(value, "|" | "|-" | ">" | ">-") {
            let mut held: Vec<&str> = Vec::new();
            at += 1;
            while at < lines.len() && (lines[at].trim().is_empty() || lines[at].starts_with("  ")) {
                held.push(lines[at].trim());
                at += 1;
            }
            let joined = held.join(" ");
            return Some(squeeze(&joined)).filter(|one| !one.is_empty());
        }
        // A list. The page shows one string, so the items are joined.
        if value.is_empty() {
            let mut items: Vec<String> = Vec::new();
            at += 1;
            while at < lines.len() {
                let Some(item) = lines[at].trim_start().strip_prefix('-') else {
                    break;
                };
                items.push(unquote(item.trim()));
                at += 1;
            }
            return (!items.is_empty()).then(|| items.join(", "));
        }
        return Some(unquote(value)).filter(|one| !one.is_empty());
    }
    None
}

/// Strip one matching pair of quotes.
fn unquote(value: &str) -> String {
    let held = value.trim();
    for quote in ['"', '\''] {
        if held.len() >= 2 && held.starts_with(quote) && held.ends_with(quote) {
            return held[1..held.len() - 1].to_string();
        }
    }
    held.to_string()
}

/// Collapse every run of whitespace to one space.
fn squeeze(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The body's first `# heading`, if it has one.
fn first_heading(body: &str) -> Option<String> {
    body.lines().find_map(|line| {
        let rest = line.strip_prefix("# ")?;
        Some(rest.trim().to_string()).filter(|one| !one.is_empty())
    })
}

/// The body's first paragraph, bounded.
///
/// Headings and fences are skipped BEFORE the paragraph starts and end it once it
/// has — so a document that opens with a title and a code block still yields the
/// prose under them, and one that opens with prose stops at the next block.
fn first_paragraph(body: &str) -> Option<String> {
    let mut held: Vec<&str> = Vec::new();
    // A fence has to be tracked, not just skipped. Skipping only the ``` LINE
    // leaves the code inside it looking like prose — a skill that opens with a
    // title and an example was described by the first line of the example.
    let mut fenced = false;
    // `\r` is trimmed by the `trim` below, so the CRLF form needs no rewrite —
    // which also keeps the borrows pointing into the caller's string.
    for line in body.lines().map(str::trim) {
        if line.starts_with("```") {
            fenced = !fenced;
            if !held.is_empty() {
                break;
            }
            continue;
        }
        if fenced {
            continue;
        }
        if line.is_empty() || line.starts_with('#') {
            if !held.is_empty() {
                break;
            }
            continue;
        }
        held.push(line);
        // Orca's own 240: a row shows two clamped lines, and reading further to
        // throw it away is work for nothing.
        if held.join(" ").len() > SUMMARY_CHARS {
            break;
        }
    }
    (!held.is_empty()).then(|| clamp(&squeeze(&held.join(" "))))
}

/// How long a description is allowed to be. Orca's own threshold for when to stop
/// reading, used here to bound the RESULT as well.
const SUMMARY_CHARS: usize = DEFAULT_POLICY.summary_chars;

/// Cut a description to something a row can hold.
///
/// Orca stops reading past 240 characters but keeps whatever line it was on
/// whole — so one very long line arrives at full length, and a `SKILL.md` whose
/// first paragraph is a thousand characters puts a thousand characters on the
/// wire for a row that clamps to two lines. This cuts at a word break where there
/// is one near the end, and marks that it cut.
fn clamp(value: &str) -> String {
    if value.chars().count() <= SUMMARY_CHARS {
        return value.to_string();
    }
    let held: String = value.chars().take(SUMMARY_CHARS).collect();
    // At a space if one is close to the end; mid-word otherwise, because a single
    // 300-character token has no break to find.
    let cut = held
        .rfind(' ')
        .filter(|at| *at > SUMMARY_CHARS * 3 / 4)
        .unwrap_or(held.len());
    format!("{}…", held[..cut].trim_end())
}

/// Read a `SKILL.md`'s own account of itself.
///
/// The name falls back through frontmatter → first heading → (the caller's)
/// directory name; the description through frontmatter → first paragraph. A file
/// with neither is still a skill — it just has less to say.
pub fn summarize(markdown: &str) -> Summary {
    let (front, body) = document_sections(markdown);
    Summary {
        name: frontmatter_value(front, "name").or_else(|| first_heading(body)),
        description: frontmatter_value(front, "description")
            .or_else(|| first_paragraph(body))
            .map(|value| clamp(&value)),
        version: frontmatter_value(front, "version"),
    }
}

/// A document's own title, using the same frontmatter and Markdown readers as
/// skill summaries. This does not interpret a filename as a description.
pub fn document_title(markdown: &str) -> Option<String> {
    let (front, body) = document_sections(markdown);
    frontmatter_value(front, "title")
        .or_else(|| first_heading(body))
        .or_else(|| first_paragraph(body))
}

fn document_sections(markdown: &str) -> (&str, &str) {
    let text = markdown.strip_prefix('\u{feff}').unwrap_or(markdown);
    // Frontmatter is a `---` fence at the very top and nowhere else. A `---` in
    // the middle of a document is a horizontal rule.
    match text.strip_prefix("---") {
        Some(rest) if rest.starts_with('\n') || rest.starts_with("\r\n") => {
            match rest.split_once("\n---") {
                Some((front, after)) => (
                    front,
                    after
                        .split_once('\n')
                        .map(|(_, body)| body)
                        .unwrap_or_default(),
                ),
                None => ("", text),
            }
        }
        _ => ("", text),
    }
}

/// One skill, as the page lists it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Skill {
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub digest: String,
    #[serde(default)]
    pub roots: Vec<SkillSource>,
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    /// The `SKILL.md` itself, which is what "reveal" opens.
    pub file: PathBuf,
    /// Its directory, which is the skill.
    pub directory: PathBuf,
    pub source_id: String,
    pub source_label: String,
    pub source_kind: SourceKind,
    pub providers: Vec<String>,
    /// How many files the skill is made of. A one-file skill and a forty-file one
    /// are different things to read.
    pub files: usize,
    /// Modified time of the `SKILL.md`, epoch milliseconds; 0 when unknown.
    pub updated_at: i64,
}

/// Whether a skill found under a home root is one the agent shipped.
///
/// Orca's rule (`sourceKindForSkill`): the `.system` directory of a home root
/// holds bundled skills. Worth telling apart — "you wrote this" and "it came with
/// the tool" answer different questions about the same list.
fn kind_of(source: &SkillSource, file: &Path) -> SourceKind {
    if source.kind != SourceKind::Home {
        return source.kind;
    }
    let bundled = file
        .strip_prefix(&source.path)
        .ok()
        .and_then(|rest| rest.components().next())
        .is_some_and(|first| first.as_os_str() == ".system");
    if bundled {
        SourceKind::Bundled
    } else {
        source.kind
    }
}

fn label_of(source: &SkillSource, kind: SourceKind) -> String {
    if kind == SourceKind::Bundled {
        format!("{} bundled", source.label)
    } else {
        source.label.clone()
    }
}

/// Walk one root and read every skill in it.
///
/// Bounded three ways, because these are directories this product does not own:
/// by depth (the source says how deep), by the summary read (only the top of a
/// `SKILL.md` is wanted), and by how many files a skill is counted as. A root that
/// does not exist is not an error — most machines have most of these missing.
///
/// Symlinks are not followed. A skills directory pointing at `/` would otherwise
/// walk the disk, and a loop would never end.
pub fn scan(source: &SkillSource) -> Vec<Skill> {
    scan_incremental(source, &Policy::default(), None).skills
}

/// Cached metadata for every visited directory and skill file. Checking only a
/// root's mtime misses edits and additions inside existing skill directories.
#[derive(Debug, Clone, Default)]
pub struct ScanSnapshot {
    pub skills: Vec<Skill>,
    pub reads: usize,
    pub capped: bool,
    stamps: Vec<(PathBuf, Option<(std::time::SystemTime, u64)>)>,
    policy: Policy,
}

fn stamp(path: &Path) -> Option<(std::time::SystemTime, u64)> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

/// Reuse summaries when every recorded directory/file still has its old stamp.
/// Both traversal and retained metadata have a per-root budget.
pub fn scan_incremental(
    source: &SkillSource,
    policy: &Policy,
    previous: Option<ScanSnapshot>,
) -> ScanSnapshot {
    if let Some(mut held) = previous
        && held.policy == *policy
        && held.stamps.iter().all(|(path, old)| stamp(path) == *old)
    {
        held.reads = 0;
        return held;
    }
    let mut result = ScanSnapshot {
        policy: policy.clone(),
        ..Default::default()
    };
    let depth_limit = source.max_depth(policy);
    let mut remaining = policy.max_entries_per_root;
    walk(
        source,
        &source.path,
        depth_limit,
        policy,
        &mut remaining,
        &mut result,
    );
    result.skills.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.file.cmp(&b.file))
    });
    result
}

fn walk(
    source: &SkillSource,
    at: &Path,
    depth: usize,
    policy: &Policy,
    remaining: &mut usize,
    result: &mut ScanSnapshot,
) {
    if *remaining == 0 || result.skills.len() >= policy.max_skills_per_root {
        result.capped = true;
        return;
    }
    result.stamps.push((at.to_path_buf(), stamp(at)));
    // The root itself may be a symlink; following it bypasses the walk's bounds.
    if std::fs::symlink_metadata(at).is_ok_and(|meta| meta.file_type().is_symlink()) {
        return;
    }
    let Ok(entries) = std::fs::read_dir(at) else {
        return;
    };
    let mut entries: Vec<_> = entries
        .take(remaining.saturating_add(1))
        .flatten()
        .collect();
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        if *remaining == 0 || result.skills.len() >= policy.max_skills_per_root {
            result.capped = true;
            break;
        }
        *remaining -= 1;
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_symlink() {
            continue;
        }
        if kind.is_dir() && depth > 0 {
            walk(source, &path, depth - 1, policy, remaining, result);
        } else if kind.is_file() && entry.file_name() == SKILL_FILE {
            result.stamps.push((path.clone(), stamp(&path)));
            result.reads += 1;
            if let Some(skill) = read_skill(source, &path, policy) {
                result.skills.push(skill);
            }
        }
    }
}

fn read_skill(source: &SkillSource, file: &Path, policy: &Policy) -> Option<Skill> {
    let directory = file.parent()?.to_path_buf();
    let meta = std::fs::metadata(file).ok()?;
    // Only the top of the file. The rest is the skill's instructions, which this
    // page does not show and should not read into memory.
    let text = read_head(file, policy.max_summary_bytes)?;
    let summary = summarize(&text);
    let kind = kind_of(source, file);
    Some(Skill {
        version: summary.version,
        digest: file_digest(file, policy.max_detail_bytes).unwrap_or_default(),
        roots: vec![source.clone()],
        id: path_id(file),
        // The directory's own name is the last fallback, and the one that always
        // exists — a `SKILL.md` with no frontmatter and no heading is common.
        name: summary.name.unwrap_or_else(|| {
            directory
                .file_name()
                .map(|held| held.to_string_lossy().into_owned())
                .unwrap_or_default()
        }),
        description: summary.description,
        file: file.to_path_buf(),
        source_id: source.id.clone(),
        source_label: label_of(source, kind),
        source_kind: kind,
        providers: source.providers.clone(),
        files: count_files(&directory, policy),
        updated_at: meta
            .modified()
            .ok()
            .and_then(|at| at.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |since| since.as_millis() as i64),
        directory,
    })
}

/// The first `limit` bytes of a file, as text.
///
/// Read as bytes and converted lossily rather than `read_to_string`: a `SKILL.md`
/// carrying one invalid byte should still show its name, and cutting at `limit`
/// can land mid-character in any case.
fn read_head(path: &Path, limit: usize) -> Option<String> {
    use std::io::Read as _;
    let mut file = std::fs::File::open(path).ok()?;
    let mut buffer = vec![0u8; limit];
    let read = file.read(&mut buffer).ok()?;
    buffer.truncate(read);
    Some(String::from_utf8_lossy(&buffer).into_owned())
}

/// How many files a skill is made of, bounded.
///
/// The number is a hint about size, not an inventory — so a directory with more
/// than the cap reports the cap rather than being walked to the end. Links are
/// skipped here for the same reason as in `walk` — a private helper, so it is
/// named rather than linked: rustdoc refuses a public item pointing at one.
pub const MAX_COUNTED_FILES: usize = DEFAULT_POLICY.max_counted_files;

fn count_files(at: &Path, policy: &Policy) -> usize {
    let mut total = 0;
    let mut queue = vec![at.to_path_buf()];
    let mut visited = 0;
    while let Some(here) = queue.pop() {
        let Ok(entries) = std::fs::read_dir(&here) else {
            continue;
        };
        for entry in entries.flatten() {
            visited += 1;
            if total >= policy.max_counted_files || visited >= policy.max_entries_per_root {
                return total.min(policy.max_counted_files);
            }
            match entry.file_type() {
                Ok(kind) if kind.is_dir() => queue.push(entry.path()),
                Ok(kind) if kind.is_file() => total += 1,
                _ => {}
            }
        }
    }
    total
}

/// Every skill on this machine, and every place looked in.
///
/// The sources come back too — including the ones that do not exist. A page that
/// listed only what it found could not tell "you have no skills there" from
/// "we never looked".
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SkillReport {
    pub skills: Vec<Skill>,
    pub sources: Vec<ScannedSource>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScannedSource {
    #[serde(flatten)]
    pub source: SkillSource,
    pub exists: bool,
    pub found: usize,
}

pub fn discover(home: &Path, repos: &[PathBuf]) -> SkillReport {
    let mut skills = Vec::new();
    let mut sources = Vec::new();
    for source in discovery_sources(home, repos) {
        let exists = source.path.is_dir();
        let found = if exists { scan(&source) } else { Vec::new() };
        sources.push(ScannedSource {
            exists,
            found: found.len(),
            source,
        });
        skills.extend(found);
    }
    // One order for the whole list, not per root: the page groups by nothing, so
    // the reader's eye needs a single alphabetical run.
    skills.sort_by(|left, right| {
        left.name
            .to_lowercase()
            .cmp(&right.name.to_lowercase())
            .then_with(|| left.source_label.cmp(&right.source_label))
            .then_with(|| left.file.cmp(&right.file))
    });
    SkillReport { skills, sources }
}

/* ---- the orchestration skill ----
 *
 * Orca's agent-orchestration surface (I18nProvider-4EBrmTGg.js:29327-29360 and
 * the Settings OrchestrationPane, Settings-UurIK2fv.js:22861-23400): one named
 * skill, installed globally with a package-runner command, detected by looking
 * in the same home roots `discovery_sources` already knows, and reported per
 * agent — because the point of the feature is "which of my agents can take a
 * handoff", not "does a directory exist".
 *
 * The MECHANISM is Orca's, measured; the NAMES are ours, by the repository
 * rule — Orca installs from its own public repository and so do we. */

/// The skill the orchestration surface is about.
pub const ORCHESTRATION_SKILL_NAME: &str = "orchestration";

/// Where `npx skills add` fetches from — our repository, the white-label
/// counterpart of `ORCA_SKILLS_REPOSITORY_URL`.
pub const SKILLS_REPOSITORY_URL: &str = "https://github.com/cjy5507/zerocode-ide";

/// External CLI capabilities verified with skills@1.5.23. Additional native
/// roots use that catalog's global defaults; roots already in discovery_sources
/// are reused rather than duplicated. No external CLI target exists for zo.
#[derive(Debug, Clone, Copy)]
pub struct InstallTarget {
    pub agent: &'static str,
    pub cli_agent: Option<&'static str>,
    pub extra_home: Option<&'static str>,
}
pub const INSTALL_TARGETS: &[InstallTarget] = &[
    InstallTarget {
        agent: "zo",
        cli_agent: None,
        extra_home: None,
    },
    InstallTarget {
        agent: "claude",
        cli_agent: Some("claude-code"),
        extra_home: None,
    },
    InstallTarget {
        agent: "openclaude",
        cli_agent: Some("claude-code"),
        extra_home: None,
    },
    InstallTarget {
        agent: "codex",
        cli_agent: Some("codex"),
        extra_home: None,
    },
    InstallTarget {
        agent: "devin",
        cli_agent: Some("devin"),
        extra_home: Some(".config/devin/skills"),
    },
    InstallTarget {
        agent: "ante",
        cli_agent: None,
        extra_home: None,
    },
    InstallTarget {
        agent: "trae",
        cli_agent: Some("trae"),
        extra_home: Some(".trae/skills"),
    },
    InstallTarget {
        agent: "autohand",
        cli_agent: Some("autohand-code"),
        extra_home: Some(".autohand/skills"),
    },
    InstallTarget {
        agent: "opencode",
        cli_agent: Some("opencode"),
        extra_home: None,
    },
    InstallTarget {
        agent: "mimo-code",
        cli_agent: None,
        extra_home: None,
    },
    InstallTarget {
        agent: "pi",
        cli_agent: Some("pi"),
        extra_home: None,
    },
    InstallTarget {
        agent: "omp",
        cli_agent: None,
        extra_home: None,
    },
    InstallTarget {
        agent: "prime-agent",
        cli_agent: None,
        extra_home: None,
    },
    InstallTarget {
        agent: "antigravity",
        cli_agent: Some("antigravity"),
        extra_home: None,
    },
    InstallTarget {
        agent: "aider",
        cli_agent: None,
        extra_home: None,
    },
    InstallTarget {
        agent: "goose",
        cli_agent: Some("goose"),
        extra_home: Some(".config/goose/skills"),
    },
    InstallTarget {
        agent: "amp",
        cli_agent: Some("amp"),
        extra_home: Some(".config/agents/skills"),
    },
    InstallTarget {
        agent: "kilo",
        cli_agent: Some("kilo"),
        extra_home: Some(".kilocode/skills"),
    },
    InstallTarget {
        agent: "kiro",
        cli_agent: Some("kiro-cli"),
        extra_home: Some(".kiro/skills"),
    },
    InstallTarget {
        agent: "crush",
        cli_agent: Some("crush"),
        extra_home: Some(".config/crush/skills"),
    },
    InstallTarget {
        agent: "aug",
        cli_agent: Some("augment"),
        extra_home: Some(".augment/skills"),
    },
    InstallTarget {
        agent: "cline",
        cli_agent: Some("cline"),
        extra_home: Some(".agents/skills"),
    },
    InstallTarget {
        agent: "codebuff",
        cli_agent: None,
        extra_home: None,
    },
    InstallTarget {
        agent: "command-code",
        cli_agent: Some("command-code"),
        extra_home: Some(".commandcode/skills"),
    },
    InstallTarget {
        agent: "continue",
        cli_agent: Some("continue"),
        extra_home: Some(".continue/skills"),
    },
    InstallTarget {
        agent: "cursor",
        cli_agent: Some("cursor"),
        extra_home: None,
    },
    InstallTarget {
        agent: "droid",
        cli_agent: Some("droid"),
        extra_home: Some(".factory/skills"),
    },
    InstallTarget {
        agent: "kimi",
        cli_agent: Some("kimi-code-cli"),
        extra_home: Some(".agents/skills"),
    },
    InstallTarget {
        agent: "mistral-vibe",
        cli_agent: Some("mistral-vibe"),
        extra_home: Some(".vibe/skills"),
    },
    InstallTarget {
        agent: "qwen-code",
        cli_agent: Some("qwen-code"),
        extra_home: Some(".qwen/skills"),
    },
    InstallTarget {
        agent: "rovo",
        cli_agent: Some("rovodev"),
        extra_home: Some(".rovodev/skills"),
    },
    InstallTarget {
        agent: "hermes",
        cli_agent: Some("hermes-agent"),
        extra_home: Some(".hermes/skills"),
    },
    InstallTarget {
        agent: "openclaw",
        cli_agent: Some("openclaw"),
        extra_home: Some(".openclaw/skills"),
    },
    InstallTarget {
        agent: "copilot",
        cli_agent: Some("github-copilot"),
        extra_home: Some(".copilot/skills"),
    },
    InstallTarget {
        agent: "grok",
        cli_agent: Some("grok"),
        extra_home: None,
    },
];
pub fn install_target(agent: &str) -> Option<&'static InstallTarget> {
    INSTALL_TARGETS.iter().find(|target| target.agent == agent)
}
pub fn cli_agent(agent: &str) -> Option<&'static str> {
    install_target(agent)
        .and_then(|target| target.cli_agent)
        .or_else(|| {
            INSTALL_TARGETS
                .iter()
                .find_map(|target| target.cli_agent.filter(|cli| *cli == agent))
        })
}
fn detected_install_command(names: &[&str], detected: &[(String, String)]) -> String {
    let agents: Vec<_> = detected
        .iter()
        .filter(|(agent, _)| cli_agent(agent).is_some())
        .map(|(agent, _)| agent.as_str())
        .collect();
    if agents.is_empty() {
        return String::new();
    }
    skill_install_command(names, &agents).unwrap_or_default()
}

/// The install command, Orca's grammar verbatim
/// (`buildAgentFeatureSkillInstallCommand`): one `npx skills add`, the skill
/// names space-joined, `--global` because the surface is about every checkout.
/// No names is a caller error, answered with `None` rather than a broken
/// command somebody might paste.
pub fn skill_install_command(names: &[&str], agents: &[&str]) -> Option<String> {
    skill_install_command_from(SKILLS_REPOSITORY_URL, names, agents)
}

pub fn skill_install_command_from(
    repository: &str,
    names: &[&str],
    agents: &[&str],
) -> Option<String> {
    if names.is_empty()
        || !valid_repository(repository)
        || !names.iter().chain(agents).all(|name| valid_cli_name(name))
    {
        return None;
    }
    // ONE FLAG PER NAME. `--skill a b` reads as one flag whose value is `a`
    // and a stray positional `b`; the installer takes the first and silently
    // ignores the rest. Identical at one name, wrong at two — which is the
    // worst way for it to be wrong, because it works until the day a second
    // skill ships (`agent-feature-install-commands.ts:44`).
    let mut command = format!("npx skills add {repository}");
    for name in names {
        command.push_str(&format!(" --skill {name}"));
    }
    // Named targets, so the installer does not take its zero-detected branch.
    // With no `--agent` it installs into every agent it has ever heard of —
    // about seventy-five directories on a bare machine, none of which the
    // person has. We already know which agents are actually here; saying so is
    // the difference between seeding two directories and littering a home
    // (`agent-feature-install-commands.ts:49-55`).
    let mut targets = Vec::new();
    for agent in agents {
        let target = cli_agent(agent)?;
        if !targets.contains(&target) {
            targets.push(target);
        }
    }
    for agent in targets {
        command.push_str(&format!(" --agent {agent}"));
    }
    command.push_str(" --global");
    Some(command)
}

/// The update command (`buildAgentFeatureSkillUpdateCommand`): by name only —
/// the installer already knows where it came from.
pub fn skill_update_command(name: &str) -> Option<String> {
    let name = name.trim();
    if !valid_cli_name(name) {
        return None;
    }
    Some(format!("npx skills update {name} --global"))
}

/// Orca's name normalisation (`normalizeSkillName`): trimmed, lowered.
fn normalize_name(value: &str) -> String {
    value.trim().to_lowercase()
}

/// Whether a discovered skill IS the orchestration skill (`isOrchestrationSkill`):
/// the read name matches, or the directory's own basename does — a skill with
/// no frontmatter is named by its directory, and the installer names the
/// directory after the skill.
pub fn is_orchestration_skill(skill: &Skill) -> bool {
    let expected = normalize_name(ORCHESTRATION_SKILL_NAME);
    if normalize_name(&skill.name) == expected {
        return true;
    }
    skill
        .directory
        .file_name()
        .is_some_and(|held| normalize_name(&held.to_string_lossy()) == expected)
}

/// Globally installed, which is what the coverage answers about
/// (`isGlobalOrchestrationSkill`): a checkout's own copy travels with the
/// checkout, not with the machine, so it does not count.
pub fn is_global_orchestration_skill(skill: &Skill) -> bool {
    is_orchestration_skill(skill) && skill.source_kind != SourceKind::Repo
}

/// Which discovery roots each agent family actually reads
/// (`ORCHESTRATION_SKILL_LOCATION_IDS_BY_AGENT`) — Orca's table, keyed by our
/// source ids since `discovery_sources` holds the same home roots. Every
/// family also reads the shared `.agents` home; an agent this table does not
/// know reads only that.
pub fn orchestration_source_ids(agent: &str) -> &'static [&'static str] {
    match agent {
        "claude" | "openclaude" => &["home-claude", "home-agents"],
        "codex" => &["home-codex", "codex-plugin-cache", "home-agents"],
        "grok" => &["home-grok", "home-agents"],
        "opencode" => &["home-opencode", "home-agents"],
        "pi" => &["home-pi", "home-agents"],
        "omp" => &["home-omp", "home-agents"],
        "prime-agent" => &["home-prime-agent", "home-agents"],
        "antigravity" => &["home-antigravity", "home-agents"],
        "zo" => &["home-zo", "home-agents"],
        "cursor" => &["home-cursor", "home-agents"],
        _ => &["home-agents"],
    }
}

/// Whether one agent can already use the skill: any global orchestration
/// skill sitting in a root that agent reads.
pub fn agent_has_orchestration_skill(agent: &str, skills: &[Skill]) -> bool {
    skills
        .iter()
        .any(|skill| is_global_orchestration_skill(skill) && agent_reads_skill(agent, skill))
}

/// One agent's row in the coverage strip.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OrchestrationAgent {
    pub agent: String,
    pub label: String,
    pub installed: bool,
}

/// The whole report the settings pane draws.
#[derive(Debug, Clone, Serialize)]
pub struct OrchestrationReport {
    /// The skill is installed somewhere global — the card's own pill.
    pub installed: bool,
    pub install_command: String,
    pub update_command: String,
    /// Detected agents in the picker's own order, each with its answer.
    pub agents: Vec<OrchestrationAgent>,
}

/// Build the report from what discovery found and which agents are installed.
///
/// `detected` arrives in the caller's picker order — the same order the agent
/// list shows — and keeps it, the way Orca sorts by its auto-pick order: the
/// chips read in the order the person knows their agents by.
pub fn orchestration_report(
    skills: &[Skill],
    detected: &[(String, String)],
) -> OrchestrationReport {
    // The card's own pill counts HOME roots only (Orca's
    // `GLOBAL_AGENT_SKILL_SOURCE_KINDS = ["home"]`), while the per-agent
    // chips count anything non-repo — so a copy living only in a plugin
    // cache marks the one agent that reads the cache without letting the
    // card claim the machine is set up. The asymmetry is Orca's, kept.
    OrchestrationReport {
        installed: skills.iter().any(|skill| {
            is_orchestration_skill(skill)
                && matches!(skill.source_kind, SourceKind::Home | SourceKind::Bundled)
        }),
        install_command: detected_install_command(&[ORCHESTRATION_SKILL_NAME], detected),
        update_command: skill_update_command(ORCHESTRATION_SKILL_NAME).expect("a literal name"),
        agents: detected
            .iter()
            .map(|(agent, label)| OrchestrationAgent {
                agent: agent.clone(),
                label: label.clone(),
                installed: agent_has_orchestration_skill(agent, skills),
            })
            .collect(),
    }
}

/* ---- the Computer Use skill ---------------------------------------------
 *
 * The native provider makes the capability real; this report answers which
 * installed agents have the instructions that teach them to use its guarded
 * CLI. It deliberately follows the orchestration card's measured discovery
 * rules so two feature cards cannot disagree about the same skill roots. */

pub const COMPUTER_USE_SKILL_NAME: &str = "computer-use";

#[must_use]
pub fn is_computer_use_skill(skill: &Skill) -> bool {
    let expected = normalize_name(COMPUTER_USE_SKILL_NAME);
    normalize_name(&skill.name) == expected
        || skill
            .directory
            .file_name()
            .is_some_and(|held| normalize_name(&held.to_string_lossy()) == expected)
}

#[must_use]
pub fn is_global_computer_use_skill(skill: &Skill) -> bool {
    is_computer_use_skill(skill) && skill.source_kind != SourceKind::Repo
}

#[must_use]
pub fn agent_has_computer_use_skill(agent: &str, skills: &[Skill]) -> bool {
    skills
        .iter()
        .any(|skill| is_global_computer_use_skill(skill) && agent_reads_skill(agent, skill))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ComputerUseAgent {
    pub agent: String,
    pub label: String,
    pub installed: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ComputerUseSkillReport {
    pub installed: bool,
    pub install_command: String,
    pub update_command: String,
    pub agents: Vec<ComputerUseAgent>,
}

#[must_use]
pub fn computer_use_skill_report(
    skills: &[Skill],
    detected: &[(String, String)],
) -> ComputerUseSkillReport {
    ComputerUseSkillReport {
        installed: skills.iter().any(|skill| {
            is_computer_use_skill(skill)
                && matches!(skill.source_kind, SourceKind::Home | SourceKind::Bundled)
        }),
        install_command: detected_install_command(&[COMPUTER_USE_SKILL_NAME], detected),
        update_command: skill_update_command(COMPUTER_USE_SKILL_NAME)
            .expect("a literal skill name"),
        agents: detected
            .iter()
            .map(|(agent, label)| ComputerUseAgent {
                agent: agent.clone(),
                label: label.clone(),
                installed: agent_has_computer_use_skill(agent, skills),
            })
            .collect(),
    }
}

/// Whether this agent can read a discovered variant. Home coverage uses the
/// existing per-agent roots table; project skills name their provider family.
pub fn agent_reads_skill(agent: &str, skill: &Skill) -> bool {
    let roots = orchestration_source_ids(agent);
    roots.contains(&skill.source_id.as_str())
        || skill
            .roots
            .iter()
            .any(|root| root.owner.as_deref() == Some(agent))
        || (skill.source_kind == SourceKind::Repo
            && skill.providers.iter().any(|provider| {
                provider == "agent-skills"
                    || provider == agent
                    || roots
                        .iter()
                        .any(|root| root.strip_prefix("home-") == Some(provider.as_str()))
            }))
}

/// Every skill resource budget lives here. Settings may lower a limit; invalid
/// values use the default, and an overlay cannot remove the memory ceiling.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Policy {
    pub max_depth: usize,
    pub max_plugin_depth: usize,
    pub max_roots: usize,
    pub max_skills_per_root: usize,
    pub max_entries_per_root: usize,
    pub max_summary_bytes: usize,
    pub summary_chars: usize,
    pub max_detail_bytes: usize,
    pub max_counted_files: usize,
    pub usage_window_ms: i64,
    pub max_usage_events: usize,
    pub max_evidence_bytes: usize,
    pub max_bundle_names: usize,
    pub max_plans: usize,
    pub max_plan_bytes: usize,
    pub install_poll_ms: u64,
    pub install_watch_ms: u64,
    pub terminal_rows: u16,
    pub terminal_cols: u16,
}
pub const DEFAULT_POLICY: Policy = Policy {
    max_depth: 4,
    max_plugin_depth: 9,
    max_roots: 64,
    max_skills_per_root: 512,
    max_entries_per_root: 8192,
    max_summary_bytes: 64 * 1024,
    summary_chars: 240,
    max_detail_bytes: 2 * 1024 * 1024,
    max_counted_files: 500,
    usage_window_ms: 7 * 24 * 60 * 60 * 1000,
    max_usage_events: 4096,
    max_evidence_bytes: 2 * 1024 * 1024,
    max_bundle_names: 512,
    max_plans: 32,
    max_plan_bytes: 256 * 1024,
    install_poll_ms: 1000,
    install_watch_ms: 30 * 60 * 1000,
    terminal_rows: 24,
    terminal_cols: 96,
};
impl Default for Policy {
    fn default() -> Self {
        DEFAULT_POLICY
    }
}
impl Policy {
    pub fn overlay(value: &serde_json::Value) -> Self {
        let mut base = serde_json::to_value(DEFAULT_POLICY).unwrap_or_default();
        if let (Some(base), Some(patch)) = (base.as_object_mut(), value.as_object()) {
            for (key, limit) in base {
                if let (Some(max), Some(wanted)) = (
                    limit.as_u64(),
                    patch.get(key).and_then(serde_json::Value::as_u64),
                ) && wanted > 0
                {
                    *limit = serde_json::json!(if key == "install_poll_ms" {
                        wanted.clamp(
                            DEFAULT_POLICY.install_poll_ms,
                            DEFAULT_POLICY.install_watch_ms,
                        )
                    } else {
                        wanted.min(max)
                    });
                }
            }
        }
        serde_json::from_value(base).unwrap_or_default()
    }
    pub fn required_names(&self) -> Vec<String> {
        crate::skill_install::BUNDLED_SKILLS
            .iter()
            .map(|skill| skill.name.to_string())
            .collect()
    }
}

/// SHA-256 of the complete file. Oversize files remain discoverable, but carry
/// no digest rather than pretending a prefix hash proves they are identical.
pub fn file_digest(path: &Path, max_bytes: usize) -> Option<String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let file = std::fs::File::open(path).ok()?;
    if file.metadata().ok()?.len() > max_bytes as u64 {
        return None;
    }
    let mut hash = Sha256::new();
    let mut bounded = file.take(max_bytes as u64 + 1);
    let read = std::io::copy(&mut bounded, &mut hash).ok()?;
    (read <= max_bytes as u64).then(|| format!("{:x}", hash.finalize()))
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    pub last_used_ms: Option<i64>,
    pub count_7d: usize,
    pub agents: Vec<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SkillFamily {
    pub id: String,
    pub name: String,
    pub variants: Vec<Skill>,
    pub conflict: bool,
    pub latest_version: Option<String>,
    pub outdated_ids: Vec<String>,
    pub bundled_name: Option<String>,
    pub usage: Usage,
}
/// Numeric version components compare numerically; unknown formats stay
/// unordered. A release sorts after its prerelease of the same version.
pub fn compare_versions(left: &str, right: &str) -> Option<std::cmp::Ordering> {
    fn parts(value: &str) -> Option<(Vec<u64>, Option<&str>)> {
        let value = value.strip_prefix('v').unwrap_or(value).split('+').next()?;
        let (base, pre) = value
            .split_once('-')
            .map_or((value, None), |(a, b)| (a, Some(b)));
        let numbers = base
            .split('.')
            .map(str::parse)
            .collect::<Result<Vec<u64>, _>>()
            .ok()?;
        Some((numbers, pre))
    }
    let (mut a, ap) = parts(left)?;
    let (mut b, bp) = parts(right)?;
    let len = a.len().max(b.len());
    a.resize(len, 0);
    b.resize(len, 0);
    Some(a.cmp(&b).then_with(|| match (ap, bp) {
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (Some(_), None) => std::cmp::Ordering::Less,
        (Some(a), Some(b)) => compare_prerelease(a, b),
        (None, None) => std::cmp::Ordering::Equal,
    }))
}
fn compare_prerelease(left: &str, right: &str) -> std::cmp::Ordering {
    let mut left = left.split('.');
    let mut right = right.split('.');
    loop {
        match (left.next(), right.next()) {
            (None, None) => return std::cmp::Ordering::Equal,
            (None, Some(_)) => return std::cmp::Ordering::Less,
            (Some(_), None) => return std::cmp::Ordering::Greater,
            (Some(a), Some(b)) => {
                let order = match (a.parse::<u64>(), b.parse::<u64>()) {
                    (Ok(a), Ok(b)) => a.cmp(&b),
                    (Ok(_), Err(_)) => std::cmp::Ordering::Less,
                    (Err(_), Ok(_)) => std::cmp::Ordering::Greater,
                    _ => a.cmp(b),
                };
                if !order.is_eq() {
                    return order;
                }
            }
        }
    }
}
pub fn families(skills: Vec<Skill>) -> Vec<SkillFamily> {
    let mut groups: std::collections::BTreeMap<String, Vec<Skill>> = Default::default();
    for skill in skills {
        groups
            .entry(normalize_name(&skill.name))
            .or_default()
            .push(skill);
    }
    groups
        .into_iter()
        .map(|(name, mut variants)| {
            variants.sort_by(|a, b| a.file.cmp(&b.file));
            let hashes: std::collections::BTreeSet<_> = variants
                .iter()
                .filter(|v| !v.digest.is_empty())
                .map(|v| &v.digest)
                .collect();
            let latest_version = variants
                .iter()
                .filter_map(|v| v.version.as_deref())
                .filter(|version| compare_versions(version, version).is_some())
                .max_by(|a, b| compare_versions(a, b).unwrap_or(std::cmp::Ordering::Equal))
                .map(str::to_string);
            let outdated_ids = variants
                .iter()
                .filter(|variant| {
                    variant
                        .version
                        .as_deref()
                        .zip(latest_version.as_deref())
                        .is_some_and(|(version, latest)| {
                            compare_versions(version, latest) == Some(std::cmp::Ordering::Less)
                        })
                })
                .map(|variant| variant.id.clone())
                .collect();
            let bundled_name =
                crate::skill_install::bundled_skill(&name).map(|skill| skill.name.to_string());
            SkillFamily {
                bundled_name,
                id: path_id(Path::new(&name)),
                name: variants[0].name.clone(),
                conflict: hashes.len() > 1,
                latest_version,
                outdated_ids,
                variants,
                usage: Usage::default(),
            }
        })
        .collect()
}

fn valid_cli_name(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('-')
        && value.len() <= SUMMARY_CHARS
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.:@/".contains(&b))
}
fn valid_repository(value: &str) -> bool {
    url::Url::parse(value).is_ok_and(|url| {
        url.scheme() == "https"
            && url.host_str().is_some()
            && url.username().is_empty()
            && url.password().is_none()
    }) && value
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b"-_.:/".contains(&b))
}
pub fn bundle_list_command(repository: &str) -> Option<String> {
    valid_repository(repository).then(|| format!("npx skills add {repository} --list"))
}
#[derive(Debug, Clone, Default, Serialize)]
pub struct BundleListing {
    pub names: Vec<String>,
    pub error: Option<String>,
}
/// Parse the CLI's Available Skills section, never its echoed shell command or
/// prose. ANSI and tree decorations are presentation, not skill names.
pub fn parse_bundle_output(output: &str, policy: &Policy) -> BundleListing {
    let mut plain = String::new();
    let mut escape = false;
    for (at, ch) in output.char_indices() {
        if at + ch.len_utf8() > policy.max_evidence_bytes {
            break;
        }
        if ch == '\u{1b}' {
            escape = true;
            continue;
        }
        if escape {
            if ch.is_ascii_alphabetic() {
                escape = false;
            }
            continue;
        }
        plain.push(ch);
    }
    let mut result = BundleListing::default();
    let mut listing = false;
    let mut name_indent = None;
    let is_header = |line: &str| {
        line.trim()
            .trim_start_matches(['│', '┃', '|', '◇', 'o', ' '])
            .eq_ignore_ascii_case("available skills")
    };
    let listing_at = plain.lines().position(is_header);
    for (line_index, line) in plain.lines().enumerate() {
        if listing_at.is_some_and(|start| line_index < start) {
            continue;
        }
        let trimmed = line.trim_start();
        let decorated = trimmed.trim_start_matches(['│', '┃', '|', ' ']);
        let lower = decorated.to_lowercase();
        let clack_error = trimmed
            .strip_prefix("■  ")
            .or_else(|| trimmed.strip_prefix("x  "));
        if let Some(error) = clack_error {
            result.error = Some(clamp(error));
            break;
        }
        if !listing
            && !trimmed.starts_with(['│', '┃', '|'])
            && (lower.starts_with("npm error")
                || lower.starts_with("npm err!")
                || lower.starts_with("error:")
                || lower.starts_with("failed "))
        {
            result.error = Some(clamp(decorated));
            break;
        }
        if is_header(line) {
            listing = true;
            name_indent = None;
            result.names.clear();
            continue;
        }
        if !listing {
            continue;
        }
        if trimmed.starts_with(['└', '—']) {
            break;
        }
        let content = trimmed.strip_prefix(['│', '┃', '|']).unwrap_or(line);
        let indent = content.len() - content.trim_start().len();
        // Clack adds its own padding. Plain plugin headings have no indent;
        // descriptions have more indentation than a skill's name.
        if indent == 0 || content.trim().is_empty() {
            continue;
        }
        let expected = *name_indent.get_or_insert(indent);
        let name = content.trim();
        if indent == expected && valid_cli_name(name) && !result.names.iter().any(|n| n == name) {
            result.names.push(name.to_string());
            if result.names.len() >= policy.max_bundle_names {
                break;
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The name falls back the way Orca's does, and the last fallback is the one
    /// that always exists.
    #[test]
    fn a_skill_is_named_by_the_best_thing_that_names_it() {
        // Frontmatter wins.
        let front = summarize("---\nname: Deploy\ndescription: Ship it\n---\n# Other\n\nWords.\n");
        assert_eq!(front.name.as_deref(), Some("Deploy"));
        assert_eq!(front.description.as_deref(), Some("Ship it"));

        // No frontmatter: the first heading, and the first paragraph under it.
        let heading = summarize("# Review a PR\n\nWalks the diff and comments.\n");
        assert_eq!(heading.name.as_deref(), Some("Review a PR"));
        assert_eq!(
            heading.description.as_deref(),
            Some("Walks the diff and comments.")
        );

        // Neither: the caller falls back to the directory name, so both are None
        // rather than invented here.
        let bare = summarize("");
        assert_eq!(bare, Summary::default());
    }

    /// A block scalar description is read and squeezed onto one line — a row shows
    /// two clamped lines, and the newlines the author wrote are not part of it.
    #[test]
    fn a_block_description_arrives_as_one_line() {
        for marker in ["|", "|-", ">", ">-"] {
            let held = summarize(&format!(
                "---\nname: N\ndescription: {marker}\n  first line\n  second line\n---\nbody\n"
            ));
            assert_eq!(
                held.description.as_deref(),
                Some("first line second line"),
                "{marker}"
            );
        }
        // And a list becomes a readable string rather than nothing.
        let listed = summarize("---\nname: N\ndescription:\n  - one\n  - \"two\"\n---\n");
        assert_eq!(listed.description.as_deref(), Some("one, two"));
    }

    /// Frontmatter is a fence at the TOP. A `---` in the middle of a document is a
    /// horizontal rule, and reading it as frontmatter would take a heading from
    /// the wrong half of the file.
    #[test]
    fn a_rule_in_the_body_is_not_frontmatter() {
        let held = summarize("# Real name\n\nThe prose.\n\n---\n\nname: Not this\n");
        assert_eq!(held.name.as_deref(), Some("Real name"));
        assert_eq!(held.description.as_deref(), Some("The prose."));

        // An unterminated fence is not frontmatter either — the whole file is body.
        let broken = summarize("---\nname: Never closed\n\n# Heading\n");
        assert_eq!(broken.name.as_deref(), Some("Heading"));
    }

    /// A nested key is not this document's. An indented `name:` inside some other
    /// mapping used to be read as the skill's name.
    #[test]
    fn an_indented_key_belongs_to_something_else() {
        let held = summarize("---\ntool:\n  name: inner\nname: outer\n---\n");
        assert_eq!(held.name.as_deref(), Some("outer"));

        let only_nested = summarize("---\ntool:\n  name: inner\n---\n# Heading\n");
        assert_eq!(
            only_nested.name.as_deref(),
            Some("Heading"),
            "a nested name was taken as the document's"
        );
    }

    /// The first paragraph skips a leading heading and a fence, and stops at the
    /// next block — so a document that opens with a title and an example still
    /// describes itself.
    #[test]
    fn the_first_paragraph_is_prose_and_not_a_code_block() {
        let held = summarize("# Title\n\n```sh\nnot this\n```\n\nThis is the summary.\n\nMore.\n");
        assert_eq!(held.description.as_deref(), Some("This is the summary."));
    }

    /// A wall of text is cut, and says it was.
    ///
    /// Orca stops READING past 240 characters but keeps the line it was on whole,
    /// so a thousand-character first line arrives at full length for a row that
    /// clamps to two lines. This bounds the result too.
    #[test]
    fn a_long_description_is_cut_at_a_word_and_marked() {
        let held = summarize(&format!("# T\n\n{}\n", "word ".repeat(200)))
            .description
            .expect("a description");
        assert!(held.chars().count() <= SUMMARY_CHARS + 1, "{}", held.len());
        assert!(held.ends_with('…'), "the cut is not marked: {held}");
        // Cut at a word break, not mid-word.
        assert!(held.ends_with("word…"), "{held}");

        // A single unbreakable token has no break to find and is cut anyway.
        let solid = summarize(&format!("# T\n\n{}\n", "x".repeat(400)))
            .description
            .expect("a description");
        assert_eq!(solid.chars().count(), SUMMARY_CHARS + 1);

        // And something that fits is untouched — no stray ellipsis on a short one.
        let short = summarize("# T\n\nBrief.\n")
            .description
            .expect("a description");
        assert_eq!(short, "Brief.");
    }

    /// Every measured root is looked in, each repo adds its two, and a plugin
    /// cache is scanned deeper than the rest.
    #[test]
    fn every_measured_root_is_looked_in() {
        let home = PathBuf::from("/home/j");
        let sources = discovery_sources(&home, &[PathBuf::from("/repos/zerocode")]);
        // Named, not counted — a count passes when one root is swapped for
        // another, and each of these is a place a real tool keeps skills.
        for wanted in [
            "home-codex",
            "home-agents",
            "home-claude",
            "codex-plugin-cache",
            "home-grok",
            "home-opencode",
            "home-pi",
            "home-omp",
            "home-prime-agent",
            "home-antigravity",
            "home-cursor",
        ] {
            assert!(
                sources.iter().any(|one| one.id == wanted),
                "`{wanted}` is no longer looked in, so those skills are invisible"
            );
        }
        // The paths are those tools' own, not a guess.
        let claude = sources
            .iter()
            .find(|one| one.id == "home-claude")
            .expect("claude");
        assert_eq!(claude.path, home.join(".claude").join("skills"));
        let opencode = sources
            .iter()
            .find(|one| one.id == "home-opencode")
            .expect("opencode");
        assert_eq!(
            opencode.path,
            home.join(".config").join("opencode").join("skills")
        );

        // Two per checkout, labelled by the directory somebody would recognise.
        let repo: Vec<&SkillSource> = sources
            .iter()
            .filter(|one| one.kind == SourceKind::Repo)
            .collect();
        assert_eq!(repo.len(), 2);
        assert!(repo.iter().all(|one| one.label.starts_with("zerocode ")));
        assert!(
            repo.iter()
                .any(|one| one.path.ends_with("zerocode/.agents/skills"))
        );

        // A plugin cache nests by plugin, then version, then its own layout.
        let cache = sources
            .iter()
            .find(|one| one.id == "codex-plugin-cache")
            .expect("cache");
        assert_eq!(cache.max_depth(&Policy::default()), MAX_PLUGIN_DEPTH);
        assert_eq!(claude.max_depth(&Policy::default()), MAX_DEPTH);
        // A compile-time truth, so it is checked at compile time: a plugin cache
        // nests deeper than an ordinary root and a shallower bound would find
        // nothing in it.
        const { assert!(MAX_PLUGIN_DEPTH > MAX_DEPTH) };
    }

    /// Two checkouts do not collide, and the same checkout yields the same ids
    /// twice — these are DOM keys as well as identities.
    #[test]
    fn each_checkout_gets_its_own_stable_ids() {
        let home = PathBuf::from("/home/j");
        let ids = |repos: &[PathBuf]| -> Vec<String> {
            discovery_sources(&home, repos)
                .into_iter()
                .filter(|one| one.kind == SourceKind::Repo)
                .map(|one| one.id)
                .collect()
        };
        let first = ids(&[PathBuf::from("/a/proj")]);
        let second = ids(&[PathBuf::from("/b/proj")]);
        assert_eq!(
            first,
            ids(&[PathBuf::from("/a/proj")]),
            "ids are not stable"
        );
        assert_ne!(first, second, "two checkouts named `proj` collided");
        // And both are listed when both are open.
        let both = ids(&[PathBuf::from("/a/proj"), PathBuf::from("/b/proj")]);
        assert_eq!(both.len(), 4);
    }

    /// A real tree, scanned. The unit tests above cover the reading; this covers
    /// the walking, which is where the bounds live.
    #[test]
    fn a_real_tree_is_walked_within_its_bounds() {
        let dir = tempfile::tempdir().expect("temp");
        let home = dir.path();
        let claude = home.join(".claude").join("skills");
        let write = |at: &Path, body: &str| {
            std::fs::create_dir_all(at.parent().expect("parent")).expect("mkdir");
            std::fs::write(at, body).expect("write");
        };

        // An ordinary skill, one written by the user.
        write(
            &claude.join("deploy").join(SKILL_FILE),
            "---\nname: Deploy\ndescription: Ships it\n---\n# x\n",
        );
        // A second file in the same skill, so the count means something.
        write(&claude.join("deploy").join("notes.md"), "more");
        // One that came with the tool.
        write(
            &claude.join(".system").join("review").join(SKILL_FILE),
            "# Review\n\nReads a diff.\n",
        );
        // One with nothing to say: named by its directory.
        write(&claude.join("quiet").join(SKILL_FILE), "");
        // Past the depth bound, so it must NOT be found.
        write(
            &claude
                .join("a")
                .join("b")
                .join("c")
                .join("d")
                .join("e")
                .join(SKILL_FILE),
            "# Too deep\n",
        );
        // A file that is not a SKILL.md is not a skill.
        write(&claude.join("other").join("README.md"), "# Not a skill\n");

        let report = discover(home, &[]);
        let names: Vec<&str> = report.skills.iter().map(|one| one.name.as_str()).collect();
        assert_eq!(names, vec!["Deploy", "quiet", "Review"], "{names:?}");

        let deploy = &report.skills[0];
        assert_eq!(deploy.description.as_deref(), Some("Ships it"));
        assert_eq!(deploy.files, 2);
        assert_eq!(deploy.source_kind, SourceKind::Home);
        assert_eq!(deploy.source_label, "Claude home");
        assert_eq!(deploy.providers, vec!["claude"]);
        assert!(deploy.updated_at > 0, "no modified time");

        // The bundled one says so, and carries the heading as its name.
        let review = report
            .skills
            .iter()
            .find(|one| one.name == "Review")
            .expect("review");
        assert_eq!(review.source_kind, SourceKind::Bundled);
        assert_eq!(review.source_label, "Claude home bundled");
        assert_eq!(review.description.as_deref(), Some("Reads a diff."));

        // The empty one is named by its directory rather than left blank.
        assert_eq!(
            report
                .skills
                .iter()
                .find(|one| one.name == "quiet")
                .map(|one| &one.description),
            Some(&None)
        );

        // Every root is reported, missing ones included.
        assert_eq!(report.sources.len(), discovery_sources(home, &[]).len());
        let claude_source = report
            .sources
            .iter()
            .find(|one| one.source.id == "home-claude")
            .expect("claude source");
        assert!(claude_source.exists);
        assert_eq!(claude_source.found, 3);
    }

    /// A link is not followed. A skills directory pointing at `/` would walk the
    /// disk; one pointing at its own parent would never end.
    #[cfg(unix)]
    #[test]
    fn a_symlink_is_not_walked() {
        let dir = tempfile::tempdir().expect("temp");
        let home = dir.path();
        let claude = home.join(".claude").join("skills");
        std::fs::create_dir_all(claude.join("real")).expect("mkdir");
        std::fs::write(claude.join("real").join(SKILL_FILE), "# Real\n").expect("write");
        // A loop, and a link to somewhere with a skill in it.
        std::os::unix::fs::symlink(&claude, claude.join("loop")).expect("symlink");
        std::os::unix::fs::symlink(claude.join("real"), claude.join("alias")).expect("symlink");

        let report = discover(home, &[]);
        assert_eq!(
            report.skills.len(),
            1,
            "a link was followed: {:?}",
            report
                .skills
                .iter()
                .map(|one| &one.file)
                .collect::<Vec<_>>()
        );
        assert_eq!(report.skills[0].name, "Real");
    }

    /// A skill under `.system` came with the tool, and says so. "You wrote this"
    /// and "it shipped" are different answers about the same list.
    #[test]
    fn a_bundled_skill_is_told_from_one_somebody_wrote() {
        let home = PathBuf::from("/home/j");
        let sources = discovery_sources(&home, &[]);
        let claude = sources
            .iter()
            .find(|one| one.id == "home-claude")
            .expect("claude");

        let bundled = claude.path.join(".system").join("review").join(SKILL_FILE);
        assert_eq!(kind_of(claude, &bundled), SourceKind::Bundled);
        assert_eq!(label_of(claude, SourceKind::Bundled), "Claude home bundled");

        let mine = claude.path.join("deploy").join(SKILL_FILE);
        assert_eq!(kind_of(claude, &mine), SourceKind::Home);
        assert_eq!(label_of(claude, SourceKind::Home), "Claude home");

        // A directory merely STARTING with `.system` is not it.
        let near = claude.path.join(".systemd").join("x").join(SKILL_FILE);
        assert_eq!(kind_of(claude, &near), SourceKind::Home);

        // And a repo root is never bundled, however it is laid out.
        let repo = SkillSource::new(
            "r",
            "R",
            PathBuf::from("/r/.agents/skills"),
            SourceKind::Repo,
            &["agent-skills"],
            None,
        );
        assert_eq!(
            kind_of(&repo, &repo.path.join(".system").join(SKILL_FILE)),
            SourceKind::Repo
        );
    }

    fn held_skill(name: &str, directory: &str, source_id: &str, kind: SourceKind) -> Skill {
        Skill {
            version: None,
            digest: String::new(),
            roots: Vec::new(),
            id: format!("{source_id}:{name}"),
            name: name.to_string(),
            description: None,
            file: PathBuf::from(directory).join(SKILL_FILE),
            directory: PathBuf::from(directory),
            source_id: source_id.to_string(),
            source_label: source_id.to_string(),
            source_kind: kind,
            providers: Vec::new(),
            files: 1,
            updated_at: 0,
        }
    }

    /// The two commands keep Orca's grammar — one `--skill` per name, one
    /// `--agent` per detected agent — and refuse to build a broken command
    /// from nothing.
    #[test]
    fn the_skill_commands_keep_the_measured_grammar() {
        assert_eq!(
            skill_install_command(&[ORCHESTRATION_SKILL_NAME], &[]).as_deref(),
            Some(
                "npx skills add https://github.com/cjy5507/zerocode-ide \
                 --skill orchestration --global"
            )
        );
        // Two names is where the old space-joined spelling went wrong: the
        // installer would have read `orchestration` as a positional and
        // installed only the first.
        assert_eq!(
            skill_install_command(&["zerocode-cli", "orchestration"], &[]).as_deref(),
            Some(
                "npx skills add https://github.com/cjy5507/zerocode-ide \
                 --skill zerocode-cli --skill orchestration --global"
            )
        );
        // And the agents we actually found, so the installer does not take its
        // install-into-everything branch.
        assert_eq!(
            skill_install_command(&[ORCHESTRATION_SKILL_NAME], &["claude", "codex"]).as_deref(),
            Some(
                "npx skills add https://github.com/cjy5507/zerocode-ide \
                 --skill orchestration --agent claude-code --agent codex --global"
            )
        );
        assert_eq!(skill_install_command(&[], &["claude"]), None);
        assert_eq!(
            skill_update_command(" orchestration ").as_deref(),
            Some("npx skills update orchestration --global")
        );
        assert_eq!(skill_update_command("   "), None);
    }

    /// A skill is the orchestration skill by its read name OR its directory's
    /// basename, case-folded — and only a global copy counts for coverage.
    #[test]
    fn the_orchestration_skill_is_matched_the_measured_way() {
        let by_name = held_skill(
            "Orchestration",
            "/h/.claude/skills/x",
            "home-claude",
            SourceKind::Home,
        );
        let by_dir = held_skill(
            "Coordinating agents",
            "/h/.claude/skills/orchestration",
            "home-claude",
            SourceKind::Home,
        );
        let other = held_skill(
            "review",
            "/h/.claude/skills/review",
            "home-claude",
            SourceKind::Home,
        );
        let repo_copy = held_skill(
            "orchestration",
            "/r/.claude/skills/orchestration",
            "repo-claude-1",
            SourceKind::Repo,
        );
        assert!(is_orchestration_skill(&by_name));
        assert!(is_orchestration_skill(&by_dir));
        assert!(!is_orchestration_skill(&other));
        assert!(is_orchestration_skill(&repo_copy));
        assert!(!is_global_orchestration_skill(&repo_copy));
    }

    /// Coverage answers per agent, through the roots that agent reads: a copy
    /// in the Claude home marks claude but not codex; a copy in the shared
    /// `.agents` home marks everybody, including agents the table cannot name.
    #[test]
    fn coverage_reads_each_agents_own_roots() {
        let claude_copy = held_skill(
            "orchestration",
            "/h/.claude/skills/orchestration",
            "home-claude",
            SourceKind::Home,
        );
        let skills = vec![claude_copy];
        assert!(agent_has_orchestration_skill("claude", &skills));
        assert!(agent_has_orchestration_skill("openclaude", &skills));
        assert!(!agent_has_orchestration_skill("codex", &skills));
        assert!(!agent_has_orchestration_skill("some-new-agent", &skills));

        let shared = vec![held_skill(
            "orchestration",
            "/h/.agents/skills/orchestration",
            "home-agents",
            SourceKind::Home,
        )];
        assert!(agent_has_orchestration_skill("codex", &shared));
        assert!(agent_has_orchestration_skill("some-new-agent", &shared));

        let report = orchestration_report(
            &skills,
            &[
                ("claude".to_string(), "Claude".to_string()),
                ("codex".to_string(), "Codex".to_string()),
            ],
        );
        assert!(report.installed);
        assert!(report.install_command.contains("--skill orchestration"));
        assert_eq!(
            report
                .agents
                .iter()
                .map(|row| (row.agent.as_str(), row.installed))
                .collect::<Vec<_>>(),
            vec![("claude", true), ("codex", false)]
        );
        // Nothing anywhere: the report says so instead of guessing.
        let empty = orchestration_report(&[], &[]);
        assert!(!empty.installed && empty.agents.is_empty());

        // A copy living only in the codex plugin cache: the codex chip says
        // Ready, but the card's pill does not claim the machine is set up —
        // Orca's own asymmetry (pill counts home roots only).
        let cached = vec![held_skill(
            "orchestration",
            "/h/.codex/plugins/cache/p/1/orchestration",
            "codex-plugin-cache",
            SourceKind::Plugin,
        )];
        let report = orchestration_report(&cached, &[("codex".to_string(), "Codex".to_string())]);
        assert!(!report.installed);
        assert!(report.agents[0].installed);
    }

    #[test]
    fn computer_use_coverage_uses_the_same_real_agent_roots() {
        let shared = vec![held_skill(
            "Computer Use",
            "/h/.agents/skills/computer-use",
            "home-agents",
            SourceKind::Home,
        )];
        let repo = held_skill(
            "computer-use",
            "/r/.agents/skills/computer-use",
            "repo-agents-0",
            SourceKind::Repo,
        );
        assert!(is_computer_use_skill(&shared[0]));
        assert!(is_computer_use_skill(&repo));
        assert!(!is_global_computer_use_skill(&repo));

        let report = computer_use_skill_report(
            &shared,
            &[
                ("claude".to_string(), "Claude".to_string()),
                ("codex".to_string(), "Codex".to_string()),
            ],
        );
        assert!(report.installed);
        assert!(report.install_command.contains("--skill computer-use"));
        assert_eq!(
            report.update_command,
            "npx skills update computer-use --global"
        );
        assert!(report.agents.iter().all(|agent| agent.installed));
    }
}
