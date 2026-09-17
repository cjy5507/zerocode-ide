//! The project's own file: what to run when a workspace appears or goes away.
//!
//! **Not to be confused with [`crate::hook`].** Orca calls both of these
//! "hooks" and they are different things: `hook.rs` is the bridge an AGENT
//! reports events to while it works, and this is a file in the REPOSITORY
//! saying "when you make me a new checkout, install the dependencies". One is
//! about watching, the other about preparing. Conflating them is how a window
//! ends up running `npm install` on a tool-use event.
//!
//! Measured from `parseOrcaYaml`/`getEffectiveHookScript`/`getSetupEnvVars`
//! (out/main/index.js:69152-69396). The schema is small and the whole file is
//! optional:
//!
//! ```yaml
//! scripts:
//!   setup: |            # a new workspace was created
//!     npm install
//!   archive: |          # a workspace is being removed
//!     docker compose down
//! issueCommand: gh issue view "$1"
//! defaultTabs:
//!   - title: dev
//!     command: npm run dev
//! ```
//!
//! **The name is ours.** Orca reads `orca.yaml` and exports `ORCA_*`
//! variables; a white-label differs in branding, so this reads
//! [`PROJECT_FILE`] and exports [`ROOT_PATH_VAR`] and its siblings. Orca also
//! sets two compatibility aliases for other tools' variable names — we do not
//! carry those: a third party's spelling in our product is the one thing the
//! repository rule is about, and a person migrating renames one variable.
//!
//! **A parser that refuses is safe; one that guesses is not.** This reads only
//! the shape above and answers [`ProjectFile::unreadable`] for anything else,
//! rather than reaching for a YAML crate. The failure mode matters: a strict
//! parser can only ever fail to run a script, while a lenient one could run
//! the WRONG string as a shell command. Orca has the same distinction and the
//! same duty to report it (`mayNeedUpdate`: the file is there and did not
//! parse), which is why "could not read this" is a state here and not a
//! silence.

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// The file this reads, at the root of a repository or a worktree.
pub const PROJECT_FILE: &str = "zerocode.yaml";

/// Variables a setup or archive script is given. Orca's three, renamed:
/// `getSetupEnvVars` hands the repository root, the worktree path and the
/// workspace's own name, because a script that prepares a checkout needs to
/// know which checkout and what the main one is.
pub const ROOT_PATH_VAR: &str = "ZEROCODE_ROOT_PATH";
pub const WORKTREE_PATH_VAR: &str = "ZEROCODE_WORKTREE_PATH";
pub const WORKSPACE_NAME_VAR: &str = "ZEROCODE_WORKSPACE_NAME";

/// How long a script may run before it is given up on. Orca's `HOOK_TIMEOUT`
/// (:69197) — two minutes, which is `npm install` on a cold cache and not much
/// more.
pub const SCRIPT_TIMEOUT: Duration = Duration::from_secs(120);

/// Which of the two scripts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectScript {
    /// A workspace was created.
    Setup,
    /// A workspace is being removed.
    Archive,
}

/// Where a script comes from when there is both a shared one in the file and a
/// local one in this window's own settings (`resolveHookCommandSourcePolicy`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ScriptSource {
    /// The file's, and only it. Orca's default.
    #[default]
    SharedOnly,
    /// This machine's, and only it — for somebody who does not want the
    /// repository deciding what runs on their laptop.
    LocalOnly,
    /// Both, shared first. Orca joins them with a newline into ONE script, so
    /// they share a shell and a working directory rather than running as two.
    RunBoth,
}

/// Whether a created workspace runs its setup without being asked
/// (`getEffectiveSetupRunPolicy`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SetupRunPolicy {
    /// Orca's default: run it.
    #[default]
    RunByDefault,
    /// Ask first. Orca *throws* rather than guessing when this is set and no
    /// decision was passed (`shouldRunSetupForCreate`), which is the right
    /// shape — a window that silently picked one would be deciding for the
    /// person who asked to be asked.
    Ask,
    /// Never, unless somebody asks for it by hand.
    Never,
}

/// One tab a new workspace opens with (`defaultTabs`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DefaultTab {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// `#rgb` or `#rrggbb`, and nothing else.
    ///
    /// Orca validates the same shape and DROPS a colour that fails it rather
    /// than refusing the tab (`DEFAULT_TAB_COLOR_RE`, out/main/index.js:69050
    /// and :69065). The strictness earns its place here: this value reaches a
    /// stylesheet, and a repository is not the authority on what this window
    /// may put in one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

/// Is this a colour a repository may name?
///
/// `#rgb` or `#rrggbb`, hex only. Anything else is dropped — the value is
/// written into a style, and a file in somebody's checkout must not be able to
/// put arbitrary text there.
fn tab_color(value: &str) -> Option<String> {
    let said = value.trim();
    let hex = said.strip_prefix('#')?;
    let sane = (hex.len() == 3 || hex.len() == 6) && hex.bytes().all(|one| one.is_ascii_hexdigit());
    sane.then(|| said.to_string())
}

/// What the project's file says.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setup: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archive: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issue_command: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub default_tabs: Vec<DefaultTab>,
    /// Directories the repository wants every workspace to SHARE with the
    /// primary checkout rather than get its own copy of — `node_modules` is
    /// the reason the feature exists (`worktree.sharedDirectories`,
    /// `orca-yaml.ts:234-236`).
    ///
    /// Repo-root-relative and already made safe by [`shared_directories`]:
    /// nothing absolute, nothing that climbs, and never `.git`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shared_directories: Vec<String>,
    /// The file is there and this reader could not make sense of it.
    ///
    /// Reported rather than swallowed: a person whose setup script silently
    /// stopped running has no way to discover why, and the honest answer is
    /// that this window's reader is strict. Orca's `mayNeedUpdate` says the
    /// same thing for the same reason.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unreadable: bool,
}

impl ProjectFile {
    /// Does this file ask for anything at all? Orca returns `null` from its
    /// parser when every recognised key is absent, so an empty file and no
    /// file are the same thing.
    pub fn is_silent(&self) -> bool {
        self.setup.is_none()
            && self.archive.is_none()
            && self.issue_command.is_none()
            && self.default_tabs.is_empty()
            && self.shared_directories.is_empty()
            && !self.unreadable
    }

    fn script(&self, which: ProjectScript) -> Option<&str> {
        match which {
            ProjectScript::Setup => self.setup.as_deref(),
            ProjectScript::Archive => self.archive.as_deref(),
        }
    }
}

/// The script that will actually run, from the file's and this machine's.
///
/// Orca's `getEffectiveHookScript` exactly, including the join: `run-both`
/// produces ONE script with the shared part first, so both halves share a
/// shell and a working directory. Returns `None` when there is nothing to run
/// — an empty string is not a script, and running `bash -c ""` would report a
/// successful setup that did nothing.
pub fn effective_script(
    file: &ProjectFile,
    local: Option<&str>,
    which: ProjectScript,
    source: ScriptSource,
) -> Option<String> {
    let shared = file
        .script(which)
        .map(str::trim)
        .filter(|one| !one.is_empty());
    let mine = local.map(str::trim).filter(|one| !one.is_empty());
    let joined = match source {
        ScriptSource::LocalOnly => mine.map(str::to_string),
        ScriptSource::SharedOnly => shared.map(str::to_string),
        ScriptSource::RunBoth => {
            let both: Vec<&str> = [shared, mine].into_iter().flatten().collect();
            (!both.is_empty()).then(|| both.join("\n"))
        }
    };
    joined.filter(|one| !one.is_empty())
}

/// Whether a freshly created workspace runs its setup.
///
/// `None` means the person has to be asked — the caller's job, and the reason
/// this is a three-way answer rather than a bool. Orca throws here; a `None`
/// carries the same "you must decide" without making a normal configuration
/// into an error.
pub fn runs_setup_on_create(policy: SetupRunPolicy) -> Option<bool> {
    match policy {
        SetupRunPolicy::RunByDefault => Some(true),
        SetupRunPolicy::Never => Some(false),
        SetupRunPolicy::Ask => None,
    }
}

/// The variables one script run is given.
/// The name Orca's "New Markdown" mints: `untitled.md`, then `untitled-2.md`
/// and up — the store's own loop (`createUntitledMarkdownFile`,
/// store-BgJxB0hr.js:40487: baseName "untitled", 100 attempts). The RULE
/// lives here and the filesystem race lives with the caller: `create_new`
/// is what actually claims a name, and a pure function is what a test can
/// hold still.
#[must_use]
pub fn untitled_markdown_name(attempt: u32) -> String {
    if attempt <= 1 {
        "untitled.md".to_string()
    } else {
        format!("untitled-{attempt}.md")
    }
}

pub fn script_env(repo_root: &str, worktree_path: &str) -> Vec<(String, String)> {
    let name = worktree_path
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(worktree_path);
    vec![
        (ROOT_PATH_VAR.to_string(), repo_root.to_string()),
        (WORKTREE_PATH_VAR.to_string(), worktree_path.to_string()),
        (WORKSPACE_NAME_VAR.to_string(), name.to_string()),
    ]
}

/* ---- the reader ----------------------------------------------------------
 *
 * Strict about SHAPE, lenient about VOCABULARY — which is Orca's own split.
 * It understands: top-level `key:` at column zero, one nesting level under
 * `scripts:`, block scalars (`|` and `|-`), plain one-line values, and a
 * list of `- title:`/`command:` maps under `defaultTabs:`.
 *
 * A shape it cannot parse makes the whole file unreadable rather than
 * half-read: a folded scalar or a tab-indented block could be MISREAD into a
 * different command, and running a misreading is the one unforgivable
 * failure here. A KEY it does not know is another matter entirely — Orca
 * ignores those (`parseOrcaYaml` picks its known fields out of the record
 * and leaves the rest), and for a while this reader refused them instead,
 * which meant a file written to Orca's own schema (`worktree:`) never ran
 * its setup (P0-12). Unknown keys are skipped whole; only a file in which
 * nothing at all was recognised answers unreadable, which is Orca's `null`
 * and the panel's "may need update".
 *
 * Comments and blank lines are skipped everywhere. Tabs for indentation are
 * refused, which YAML itself also does — and refusing is what keeps a
 * tab-indented file from being read with the wrong nesting. */

const RECOGNISED_KEYS: &[&str] = &[
    "scripts",
    "issueCommand",
    "defaultTabs",
    "environmentRecipes",
    // Orca's worktree defaults — `worktree.sharedDirectories`
    // (`orca-yaml.ts:233-236`), read below.
    "worktree",
];

/// A cap so a pathological file cannot be walked forever. Orca has its own
/// (`isOrcaYamlTextWithinLimit`); the number is ours because the measurement
/// does not expose it.
const MAX_FILE_BYTES: usize = 256 * 1024;

/// Read the project's file.
///
/// A file that is absent is `None`; a file that is present and says nothing is
/// `Some` with [`ProjectFile::is_silent`]; a file that is present and confusing
/// is `Some` with [`ProjectFile::unreadable`].
pub fn parse_project_file(text: &str) -> ProjectFile {
    if text.len() > MAX_FILE_BYTES || text.contains('\t') {
        return unreadable();
    }
    let mut file = ProjectFile::default();
    let lines: Vec<&str> = text.lines().collect();
    let mut at = 0;
    // The two verdict flags. Orca IGNORES keys it does not know — its parser
    // picks the known fields out of the record and leaves the rest
    // (`parseOrcaYaml`) — so an unknown key must not stop a setup from
    // running (P0-12). But a file in which NOTHING was recognised is Orca's
    // `null`, the answer its panel shows as "may need update", and that is
    // exactly what `unreadable` reports here.
    let mut recognised = false;
    let mut passed_over = false;
    while at < lines.len() {
        let line = lines[at];
        if is_blank(line) {
            at += 1;
            continue;
        }
        // Only a top-level key may sit at column zero; anything else here is a
        // shape this reader does not know.
        if line.starts_with(' ') || line.starts_with('-') {
            return unreadable();
        }
        let Some((key, rest)) = split_key(line) else {
            return unreadable();
        };
        if !RECOGNISED_KEYS.contains(&key) {
            passed_over = true;
            at = skip_block(&lines, at + 1);
            continue;
        }
        match key {
            "scripts" => match read_scripts(&lines, at + 1, &mut file) {
                Some((next, read_a_script)) => {
                    if read_a_script {
                        recognised = true;
                    } else {
                        // `scripts:` that contributed nothing this window
                        // runs — empty, or all foreign keys. Counted with
                        // the passed-over rather than as recognition, so a
                        // file that is ONLY that still reports itself.
                        passed_over = true;
                    }
                    at = next;
                }
                None => return unreadable(),
            },
            "issueCommand" => match read_value(&lines, at, rest) {
                Some((value, next)) => {
                    file.issue_command = value;
                    recognised = true;
                    at = next;
                }
                None => return unreadable(),
            },
            "defaultTabs" => match read_tabs(&lines, at + 1, &mut file) {
                Some(next) => {
                    recognised = true;
                    at = next;
                }
                None => return unreadable(),
            },
            "worktree" => match read_worktree(&lines, at + 1, &mut file) {
                Some((next, read_something)) => {
                    // A `worktree:` block whose only keys are ones this
                    // window does not run counts as passed over, the same
                    // way an all-foreign `scripts:` does.
                    if read_something {
                        recognised = true;
                    } else {
                        passed_over = true;
                    }
                    at = next;
                }
                None => return unreadable(),
            },
            // Known to Orca, not used here. Skipped rather than refused so a
            // file written for a fleet still runs its setup on this machine —
            // and counted as recognition, because Orca's parser answers
            // non-null for them.
            _ => {
                recognised = true;
                at = skip_block(&lines, at + 1);
            }
        }
    }
    if passed_over && !recognised {
        return unreadable();
    }
    file
}

fn unreadable() -> ProjectFile {
    ProjectFile {
        unreadable: true,
        ..ProjectFile::default()
    }
}

fn is_blank(line: &str) -> bool {
    let said = line.trim();
    said.is_empty() || said.starts_with('#')
}

/// `key: rest` at this line's own indentation, or `None` when there is no
/// colon to divide it.
fn split_key(line: &str) -> Option<(&str, &str)> {
    let trimmed = line.trim_start();
    let (key, rest) = trimmed.split_once(':')?;
    let key = key.trim();
    if key.is_empty()
        || !key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return None;
    }
    Some((key, rest.trim()))
}

fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

/// One value: what follows the colon, or the block scalar under it.
///
/// Returns the line to continue from. `|` and `|-` are the same to this reader
/// — the difference is only the trailing newline, and a shell does not care.
fn read_value<'a>(lines: &[&'a str], at: usize, rest: &'a str) -> Option<(Option<String>, usize)> {
    if rest != "|" && rest != "|-" && rest != ">" && rest != ">-" {
        // A one-line value. Quotes are stripped only when they wrap the whole
        // thing, which is the only quoting this reader claims to understand.
        let said = rest.trim();
        let bare = said
            .strip_prefix('"')
            .and_then(|one| one.strip_suffix('"'))
            .or_else(|| {
                said.strip_prefix('\'')
                    .and_then(|one| one.strip_suffix('\''))
            })
            .unwrap_or(said);
        return Some(((!bare.is_empty()).then(|| bare.to_string()), at + 1));
    }
    // A folded scalar (`>`) joins its lines with spaces, which would silently
    // turn two commands into one. Refused rather than guessed.
    if rest.starts_with('>') {
        return None;
    }
    let own = indent_of(lines[at]);
    let mut body: Vec<&str> = Vec::new();
    let mut next = at + 1;
    let mut block_indent = None;
    while next < lines.len() {
        let line = lines[next];
        if line.trim().is_empty() {
            body.push("");
            next += 1;
            continue;
        }
        let here = indent_of(line);
        if here <= own {
            break;
        }
        let deep = *block_indent.get_or_insert(here);
        if here < deep {
            break;
        }
        body.push(&line[deep..]);
        next += 1;
    }
    while body.last().is_some_and(|one| one.is_empty()) {
        body.pop();
    }
    let text = body.join("\n");
    Some(((!text.trim().is_empty()).then_some(text), next))
}

/// `None` is structural failure only. The bool says whether a script this
/// window RUNS was read — Orca ignores `scripts` keys it does not know
/// (`parseOrcaYaml` reads `setup` and `archive` out of the record), and a
/// `postCreate:` somebody wrote for another tool must not take the setup
/// beside it down with it. The caller decides what a block that contributed
/// nothing means.
fn read_scripts(lines: &[&str], from: usize, file: &mut ProjectFile) -> Option<(usize, bool)> {
    let mut at = from;
    let mut read_a_script = false;
    while at < lines.len() {
        let line = lines[at];
        if is_blank(line) {
            at += 1;
            continue;
        }
        if indent_of(line) == 0 {
            break;
        }
        let (key, rest) = split_key(line)?;
        let (value, next) = read_value(lines, at, rest)?;
        match key {
            "setup" => {
                file.setup = value;
                read_a_script = true;
            }
            "archive" => {
                file.archive = value;
                read_a_script = true;
            }
            // Read for its shape, kept for nobody: the value was consumed so
            // the walk stays aligned, and nothing of it is run.
            _ => {}
        }
        at = next;
    }
    Some((at, read_a_script))
}

/// Orca's own bound on the list (`normalizeDefaultTabs`, out/main/index.js
/// :69032): entries past it are dropped, not refused. The whole project file
/// is capped at 256KB, and minimal `- title:` entries fit that budget tens of
/// thousands of times — a loop that opens a pty per entry needs a smaller
/// number than "whatever fits in the file".
const MAX_DEFAULT_TABS: usize = 256;

fn read_tabs(lines: &[&str], from: usize, file: &mut ProjectFile) -> Option<usize> {
    let mut at = from;
    let mut tabs: Vec<DefaultTab> = Vec::new();
    while at < lines.len() {
        let line = lines[at];
        if is_blank(line) {
            at += 1;
            continue;
        }
        if indent_of(line) == 0 {
            break;
        }
        let said = line.trim_start();
        if let Some(first) = said.strip_prefix("- ") {
            tabs.push(DefaultTab::default());
            let (key, rest) = split_key(first)?;
            // The continuation index is honoured here exactly as it is for the
            // later fields. Discarding it and stepping one line — which this
            // did — walked INTO a block scalar's body when the dash line's own
            // field carried one, and read its text as keys: the natural
            // orderings parsed and the unusual one refused the whole file.
            let (value, next) = read_value(lines, at, rest)?;
            put_tab_field(tabs.last_mut()?, key, value)?;
            at = next;
            continue;
        }
        let (key, rest) = split_key(said)?;
        let (value, next) = read_value(lines, at, rest)?;
        put_tab_field(tabs.last_mut()?, key, value)?;
        at = next;
    }
    if tabs.is_empty() {
        return None;
    }
    tabs.truncate(MAX_DEFAULT_TABS);
    file.default_tabs = tabs;
    Some(at)
}

/// The most entries one repository may ask to share.
///
/// Orca's own bound, and for the reason it gives: a repository file is
/// somebody else's input and the work it can request has to stop somewhere
/// (`MAX_SHARED_DIRECTORIES`, `orca-yaml.ts:35`).
const MAX_SHARED_DIRECTORIES: usize = 100;

/// Read the `worktree:` block. Answers `(next line, read something we run)`.
fn read_worktree(lines: &[&str], from: usize, file: &mut ProjectFile) -> Option<(usize, bool)> {
    let mut at = from;
    let mut read_something = false;
    while at < lines.len() {
        let line = lines[at];
        if is_blank(line) {
            at += 1;
            continue;
        }
        if indent_of(line) == 0 {
            break;
        }
        let (key, rest) = split_key(line.trim_start())?;
        if key != "sharedDirectories" {
            // Another of Orca's worktree keys. Skipped whole, the same way an
            // unknown top-level key is.
            at = skip_block(lines, at + 1);
            continue;
        }
        if !rest.is_empty() {
            // `sharedDirectories: [a, b]` — a flow sequence, which this reader
            // does not parse. Refusing the FILE over it would stop the setup
            // script too, so the block is skipped and the file still runs.
            at = skip_block(lines, at + 1);
            continue;
        }
        let (entries, next) = read_string_list(lines, at + 1)?;
        file.shared_directories = shared_directories(&entries);
        read_something = !file.shared_directories.is_empty();
        at = next;
    }
    Some((at, read_something))
}

/// A `- value` list under a key, at any deeper indentation.
fn read_string_list(lines: &[&str], from: usize) -> Option<(Vec<String>, usize)> {
    let mut at = from;
    let mut found: Vec<String> = Vec::new();
    while at < lines.len() {
        let line = lines[at];
        if is_blank(line) {
            at += 1;
            continue;
        }
        let said = line.trim_start();
        if indent_of(line) == 0 || !said.starts_with("- ") {
            break;
        }
        let value = said[2..].trim();
        // Quotes are the one decoration this reader takes off — a path is a
        // path whether or not somebody quoted it.
        let value = value
            .strip_prefix('"')
            .and_then(|held| held.strip_suffix('"'))
            .or_else(|| {
                value
                    .strip_prefix('\'')
                    .and_then(|held| held.strip_suffix('\''))
            })
            .unwrap_or(value);
        found.push(value.to_string());
        at += 1;
    }
    Some((found, at))
}

/// Make a repository's shared-directory list safe to act on.
///
/// Every rule here is Orca's (`normalizeSharedDirectories`,
/// `orca-yaml.ts:46-72`), and each drops rather than repairs:
///
/// * `\` becomes `/`, a leading `./` and trailing `/` come off.
/// * Absolute paths, Windows drive letters and `..` are dropped — this list
///   comes from a file in somebody else's repository and it decides what gets
///   linked into a checkout.
/// * `.git` is dropped at any depth. Sharing it would make two worktrees one.
/// * An entry that still needs collapsing (`apps/./web`) is **dropped, not
///   rewritten**, and the reason is subtle enough that Orca writes it down:
///   the link would be created at the collapsed path, git would report the
///   collapsed path, and every later comparison against the stored entry would
///   miss — so the link would look like permanent untracked work.
/// * Duplicates fold, order is kept, and the count is bounded.
#[must_use]
pub fn shared_directories(entries: &[String]) -> Vec<String> {
    let mut kept: Vec<String> = Vec::new();
    for raw in entries.iter().take(MAX_SHARED_DIRECTORIES) {
        let raw = raw.trim();
        if raw.is_empty() {
            continue;
        }
        let normalized = raw
            .replace('\\', "/")
            .trim_end_matches('/')
            .trim_start_matches("./")
            .to_string();
        if normalized.is_empty() || normalized.starts_with('/') {
            continue;
        }
        // `C:` and friends.
        if normalized
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphabetic)
            && normalized.as_bytes().get(1) == Some(&b':')
        {
            continue;
        }
        if normalized
            .split('/')
            .any(|part| matches!(part, ".." | "." | "" | ".git"))
        {
            continue;
        }
        if !kept.iter().any(|held| held == &normalized) {
            kept.push(normalized);
        }
    }
    kept
}

fn put_tab_field(tab: &mut DefaultTab, key: &str, value: Option<String>) -> Option<()> {
    match key {
        "title" => tab.title = value.filter(|one| is_one_line(one)),
        // REFUSED, not trimmed, when the command holds a control character.
        // The unrun command's whole promise is "typed and waiting, the Enter
        // is yours" — and a pty makes no distinction between an embedded
        // newline and that Enter. A block scalar joins its lines with `\n`, so
        // `command: |` followed by two lines would execute the first the
        // moment it was "typed", policy or no policy. A carriage return
        // smuggled inside a quoted value is the same key in different
        // clothes. The strict parser can only fail to RUN something; the
        // loose one runs what nobody agreed to. (Found by review, verified
        // live against this parser before the refusal existed.)
        "command" => match value {
            Some(one) if !is_one_line(&one) => return None,
            other => tab.command = other,
        },
        // A colour that is not one is dropped rather than refused, which is
        // Orca's own choice: the tab is still a tab, and a typo in a decoration
        // should not stop a workspace opening.
        "color" => tab.color = value.as_deref().and_then(tab_color),
        _ => return None,
    }
    Some(())
}

/// No control characters — which covers `\n`, `\r`, and every escape a
/// terminal would interpret as something other than text.
fn is_one_line(value: &str) -> bool {
    !value.chars().any(char::is_control)
}

/// Walk past a block this reader does not use, stopping at the next top-level
/// key.
fn skip_block(lines: &[&str], from: usize) -> usize {
    let mut at = from;
    while at < lines.len() {
        let line = lines[at];
        if !is_blank(line) && indent_of(line) == 0 {
            break;
        }
        at += 1;
    }
    at
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 저장소가 공유하겠다고 적은 디렉터리는 **안전해진 뒤에만** 목록에 든다.
    #[test]
    fn a_shared_directory_list_drops_everything_it_cannot_act_on_safely() {
        let asked: Vec<String> = [
            "node_modules",
            "./apps/web/.env",   // `./` 는 벗겨진다
            "vendor/",           // 꼬리 슬래시도
            "node_modules",      // 중복은 접힌다
            "/etc/passwd",       // 절대 경로 — 남의 파일
            "C:/Windows",        // 윈도 드라이브
            "../outside",        // 기어오르기
            "apps/../etc",       // 안쪽에 숨은 기어오르기
            "apps/./web",        // 접어야 하는 것 — 고치지 않고 버린다
            ".git",              // 두 워크트리를 하나로 만든다
            "nested/.git/hooks", // 깊은 곳의 .git 도
            "  ",                // 빈 것
        ]
        .iter()
        .map(|held| (*held).to_string())
        .collect();

        assert_eq!(
            shared_directories(&asked),
            vec![
                "node_modules".to_string(),
                "apps/web/.env".to_string(),
                "vendor".to_string(),
            ],
            "저장소 파일이 요구한 위험한 경로가 목록에 남았다"
        );

        // 역슬래시는 슬래시로 — 같은 파일을 두 이름으로 부르지 않는다.
        assert_eq!(
            shared_directories(&["apps\\web".to_string()]),
            vec!["apps/web".to_string()]
        );

        // 그리고 경계가 있다: 저장소 파일은 남의 입력이다.
        let many: Vec<String> = (0..500).map(|n| format!("dir{n}")).collect();
        assert_eq!(shared_directories(&many).len(), MAX_SHARED_DIRECTORIES);
    }

    /// `worktree:` 블록이 읽히고, 그 안의 모르는 키는 파일을 거절하지 않는다.
    #[test]
    fn the_worktree_block_is_read_without_refusing_the_file_over_a_foreign_key() {
        let read = parse_project_file(
            "worktree:\n  sharedDirectories:\n    - node_modules\n    - \"apps/web/.env\"\n",
        );
        assert!(!read.unreadable);
        assert_eq!(
            read.shared_directories,
            vec!["node_modules".to_string(), "apps/web/.env".to_string()]
        );

        // 설정 스크립트와 나란히 있어도 둘 다 읽힌다.
        let both = parse_project_file(
            "scripts:\n  setup: npm ci\nworktree:\n  sharedDirectories:\n    - node_modules\n",
        );
        assert_eq!(both.setup.as_deref(), Some("npm ci"));
        assert_eq!(both.shared_directories, vec!["node_modules".to_string()]);

        // Orca 의 다른 worktree 키는 건너뛰되 파일은 산다 — 설정 스크립트가
        // 남의 키 하나 때문에 안 도는 것이 P0-12 의 병이었다.
        let foreign =
            parse_project_file("worktree:\n  somethingElse: yes\nscripts:\n  setup: npm ci\n");
        assert!(!foreign.unreadable);
        assert_eq!(foreign.setup.as_deref(), Some("npm ci"));
        assert!(foreign.shared_directories.is_empty());

        // 흐름 시퀀스는 이 리더가 못 읽지만, 그 때문에 파일을 거절하지는
        // 않는다 — 거절하면 설정 스크립트까지 같이 죽는다.
        let flow = parse_project_file(
            "worktree:\n  sharedDirectories: [a, b]\nscripts:\n  setup: npm ci\n",
        );
        assert!(!flow.unreadable);
        assert_eq!(flow.setup.as_deref(), Some("npm ci"));
        assert!(flow.shared_directories.is_empty());

        // 공유 디렉터리만 있는 파일도 말한 것이 있는 파일이다.
        assert!(
            !parse_project_file("worktree:\n  sharedDirectories:\n    - node_modules\n")
                .is_silent()
        );
    }

    /// Orca's own ladder: the first name is bare, every retry counts.
    #[test]
    fn untitled_names_climb_the_way_orcas_do() {
        assert_eq!(untitled_markdown_name(0), "untitled.md");
        assert_eq!(untitled_markdown_name(1), "untitled.md");
        assert_eq!(untitled_markdown_name(2), "untitled-2.md");
        assert_eq!(untitled_markdown_name(37), "untitled-37.md");
    }

    #[test]
    fn the_ordinary_file_reads() {
        let file = parse_project_file(
            "# what a new checkout needs\nscripts:\n  setup: |\n    npm install\n    npm run build\n  archive: |-\n    docker compose down\nissueCommand: gh issue view \"$1\"\n",
        );
        assert!(!file.unreadable);
        assert_eq!(file.setup.as_deref(), Some("npm install\nnpm run build"));
        assert_eq!(file.archive.as_deref(), Some("docker compose down"));
        assert_eq!(file.issue_command.as_deref(), Some("gh issue view \"$1\""));
        assert!(!file.is_silent());
    }

    #[test]
    fn a_file_written_by_hand_reads_the_way_it_was_written() {
        // Copied from a real file typed into a scratch repository rather than
        // composed for the test: a leading comment, two statements in the
        // setup, a variable reference in the archive, and the blank line a
        // person leaves between the two blocks.
        let file = parse_project_file(
            "# what a new checkout needs\nscripts:\n  setup: |\n    printf 'prepared %s\\n' \"$ZEROCODE_WORKSPACE_NAME\" > prepared.txt\n    echo \"root is $ZEROCODE_ROOT_PATH\" >> prepared.txt\n\n  archive: |\n    echo torn-down >> \"$ZEROCODE_ROOT_PATH/archived.txt\"\n",
        );
        assert!(!file.unreadable, "a hand-written file was refused");
        let setup = file.setup.expect("no setup script");
        assert_eq!(
            setup.lines().count(),
            2,
            "the two statements did not survive: {setup}"
        );
        assert!(setup.starts_with("printf 'prepared"));
        assert!(setup.ends_with(">> prepared.txt"));
        assert_eq!(
            file.archive.as_deref(),
            Some("echo torn-down >> \"$ZEROCODE_ROOT_PATH/archived.txt\"")
        );
    }

    #[test]
    fn an_empty_file_and_no_file_say_the_same_thing() {
        assert!(parse_project_file("").is_silent());
        assert!(parse_project_file("# nothing but a comment\n").is_silent());
        // A `scripts:` with nothing under it is not a script.
        assert!(parse_project_file("scripts:\n").unreadable);
    }

    #[test]
    fn a_shape_this_reader_does_not_know_is_refused_whole() {
        // Each of these could be MISREAD into a different command, which is
        // the failure this strictness exists to prevent.
        for confusing in [
            // A folded scalar joins lines with spaces: two commands become one.
            "scripts:\n  setup: >\n    npm install\n    npm test\n",
            // A key under `scripts` we do not run, but the author expects to.
            "scripts:\n  postCreate: npm i\n",
            // A top-level key nobody here knows.
            "beforeEverything: rm -rf /\n",
            // Tabs make the nesting ambiguous.
            "scripts:\n\tsetup: npm i\n",
            // A list where a map belongs.
            "- setup: npm i\n",
        ] {
            assert!(
                parse_project_file(confusing).unreadable,
                "read something it should have refused:\n{confusing}"
            );
        }
        // And "unreadable" is not silent — the window has something to say.
        assert!(!parse_project_file("beforeEverything: x\n").is_silent());
    }

    #[test]
    fn a_key_orca_knows_and_this_window_does_not_is_skipped_not_refused() {
        // A file written for a fleet still runs its setup here.
        let file = parse_project_file(
            "environmentRecipes:\n  - name: vm\n    image: ubuntu\nscripts:\n  setup: npm i\n",
        );
        assert!(!file.unreadable);
        assert_eq!(file.setup.as_deref(), Some("npm i"));
    }

    /// Orca's leniency, ported whole (P0-12): its parser picks the fields it
    /// knows out of the record and IGNORES the rest (`parseOrcaYaml`), so no
    /// unknown vocabulary — Orca's own `worktree:`, a future key, a fleet's
    /// extension, a foreign script name — may stop the setup beside it.
    #[test]
    fn unknown_vocabulary_never_takes_the_setup_down_with_it() {
        // The headline case: a file written to Orca's OWN schema.
        let orca_spec = parse_project_file(
            "worktree:\n  sharedDirectories:\n    - node_modules\nscripts:\n  setup: npm i\n",
        );
        assert!(!orca_spec.unreadable, "Orca's own worktree key was refused");
        assert_eq!(orca_spec.setup.as_deref(), Some("npm i"));

        // A top-level key nobody knows, beside a setup somebody needs.
        let extended = parse_project_file(
            "companyPolicy: strict\nscripts:\n  setup: make ready\nissueCommand: gh issue view \"$1\"\n",
        );
        assert!(!extended.unreadable);
        assert_eq!(extended.setup.as_deref(), Some("make ready"));
        assert_eq!(
            extended.issue_command.as_deref(),
            Some("gh issue view \"$1\"")
        );

        // A foreign script name under `scripts`, beside the two that run —
        // block value and all.
        let foreign = parse_project_file(
            "scripts:\n  postCreate: |\n    echo other tool\n  setup: npm i\n  archive: docker compose down\n",
        );
        assert!(!foreign.unreadable);
        assert_eq!(foreign.setup.as_deref(), Some("npm i"));
        assert_eq!(foreign.archive.as_deref(), Some("docker compose down"));

        // A worktree-only file is recognised — Orca parses it non-null. It
        // is no longer SILENT either: the shared directories are something
        // this window now acts on, so a file that asks only for those is a
        // file that asked for something.
        let worktree_only =
            parse_project_file("worktree:\n  sharedDirectories:\n    - node_modules\n");
        assert!(!worktree_only.unreadable);
        assert!(!worktree_only.is_silent());
        // And a worktree block this window runs nothing out of is Orca's
        // `null` when it is the only thing in the file: the shared list is
        // empty, so every field of the answer is empty, and Orca returns null
        // exactly there (`orca-yaml.ts:238-247`).
        assert!(parse_project_file("worktree:\n  somethingElse: yes\n").unreadable);

        // But a file in which NOTHING was recognised still reports itself:
        // that is Orca's `null`, the panel's "may need update" — and the
        // shape refusals above it have not moved an inch.
        assert!(parse_project_file("beforeEverything: x\nafterEverything: y\n").unreadable);
        assert!(parse_project_file("scripts:\n  postCreate: npm i\n").unreadable);
    }

    /// A colour is written into a style, so a repository does not get to say
    /// what goes there. Orca validates the same shape and drops what fails it.
    #[test]
    fn a_tab_colour_is_hex_or_it_is_nothing() {
        let file = parse_project_file(
            "defaultTabs:\n  - title: dev\n    color: \"#0af\"\n  - title: logs\n    color: \"#00AAFF\"\n  - title: bad\n    color: red\n  - title: worse\n    color: \"#0af; background: url(x)\"\n",
        );
        assert_eq!(file.default_tabs.len(), 4);
        assert_eq!(file.default_tabs[0].color.as_deref(), Some("#0af"));
        assert_eq!(file.default_tabs[1].color.as_deref(), Some("#00AAFF"));
        // Dropped, not refused: the tab is still a tab, and a typo in a
        // decoration must not stop a workspace opening.
        assert_eq!(file.default_tabs[2].color, None);
        assert_eq!(file.default_tabs[2].title.as_deref(), Some("bad"));
        assert_eq!(file.default_tabs[3].color, None);
    }

    /// A pty makes no distinction between an embedded newline and the Enter
    /// key. The unrun command's promise is "typed and waiting, the Enter is
    /// yours" — a command that carries its own `\n` breaks that promise the
    /// moment it is typed, whatever the policy said. Verified live before the
    /// refusal existed: the block-scalar body reached the shell and every line
    /// but the last EXECUTED.
    #[test]
    fn a_tab_command_with_its_own_enter_refuses_the_whole_file() {
        // The natural ordering — title first, command as a later field — with
        // a block scalar carrying two lines.
        let file = parse_project_file(
            "defaultTabs:\n  - title: dev\n    command: |\n      echo one\n      rm -rf demo\n",
        );
        assert!(file.unreadable, "the two-line command was accepted");
        assert!(file.default_tabs.is_empty());

        // A carriage return smuggled inside a quoted value is the same key in
        // different clothes.
        let file =
            parse_project_file("defaultTabs:\n  - title: dev\n    command: \"echo a\rrm -rf b\"\n");
        assert!(file.unreadable, "the embedded CR was accepted");

        // A one-line block scalar carries no newline and is an ordinary
        // command.
        let file = parse_project_file("defaultTabs:\n  - title: dev\n    command: |\n      make\n");
        assert!(!file.unreadable);
        assert_eq!(file.default_tabs[0].command.as_deref(), Some("make"));

        // A title is decoration: one with control characters is dropped like a
        // bad colour, and the workspace still opens.
        let file = parse_project_file("defaultTabs:\n  - title: \"a\rb\"\n    command: make\n");
        assert!(!file.unreadable);
        assert_eq!(file.default_tabs[0].title, None);
        assert_eq!(file.default_tabs[0].command.as_deref(), Some("make"));
    }

    /// The dash line's own field can carry a block scalar, and the reader used
    /// to step one line instead of honouring the block's end — walking into
    /// the body and reading its text as keys, so the unusual ordering refused
    /// a file the natural ordering accepted.
    #[test]
    fn a_block_scalar_on_the_dash_line_is_walked_past_not_into() {
        let file = parse_project_file(
            "defaultTabs:\n  - title: |\n      first\n      second\n    command: make\n  - title: logs\n",
        );
        assert!(
            !file.unreadable,
            "the dash-line block scalar broke the file"
        );
        assert_eq!(file.default_tabs.len(), 2);
        // The two-line title is decoration and drops; the command survives.
        assert_eq!(file.default_tabs[0].title, None);
        assert_eq!(file.default_tabs[0].command.as_deref(), Some("make"));
        assert_eq!(file.default_tabs[1].title.as_deref(), Some("logs"));
    }

    /// Orca bounds the list at 256 and drops the rest. The file itself is
    /// capped at 256KB, which fits minimal entries tens of thousands of times
    /// — and the window opens a pty per entry.
    #[test]
    fn the_tab_list_is_bounded_at_orcas_own_number() {
        let mut text = String::from("defaultTabs:\n");
        for index in 0..300 {
            text.push_str(&format!("  - title: t{index}\n"));
        }
        let file = parse_project_file(&text);
        assert!(!file.unreadable);
        assert_eq!(file.default_tabs.len(), 256);
        assert_eq!(file.default_tabs[255].title.as_deref(), Some("t255"));
    }

    #[test]
    fn default_tabs_read_as_a_list_of_two_fields() {
        let file = parse_project_file(
            "defaultTabs:\n  - title: dev\n    command: npm run dev\n  - title: logs\n    command: tail -f log\n",
        );
        assert!(!file.unreadable);
        assert_eq!(file.default_tabs.len(), 2);
        assert_eq!(file.default_tabs[0].title.as_deref(), Some("dev"));
        assert_eq!(file.default_tabs[1].command.as_deref(), Some("tail -f log"));
    }

    #[test]
    fn run_both_is_one_script_rather_than_two() {
        // They share a shell and a working directory, which is what makes
        // `cd build` in the shared half affect the local half.
        let file = ProjectFile {
            setup: Some("npm install".into()),
            ..ProjectFile::default()
        };
        assert_eq!(
            effective_script(
                &file,
                Some("cp .env.example .env"),
                ProjectScript::Setup,
                ScriptSource::RunBoth
            ),
            Some("npm install\ncp .env.example .env".to_string())
        );
        assert_eq!(
            effective_script(
                &file,
                Some("x"),
                ProjectScript::Setup,
                ScriptSource::SharedOnly
            ),
            Some("npm install".to_string())
        );
        assert_eq!(
            effective_script(
                &file,
                Some("x"),
                ProjectScript::Setup,
                ScriptSource::LocalOnly
            ),
            Some("x".to_string())
        );
        // Nothing to run is None, not an empty command — `bash -c ""` would
        // report a successful setup that did nothing.
        assert_eq!(
            effective_script(
                &file,
                None,
                ProjectScript::Archive,
                ScriptSource::SharedOnly
            ),
            None
        );
        assert_eq!(
            effective_script(
                &file,
                Some("   "),
                ProjectScript::Setup,
                ScriptSource::LocalOnly
            ),
            None
        );
    }

    #[test]
    fn asking_is_a_third_answer_rather_than_a_guess() {
        assert_eq!(
            runs_setup_on_create(SetupRunPolicy::RunByDefault),
            Some(true)
        );
        assert_eq!(runs_setup_on_create(SetupRunPolicy::Never), Some(false));
        assert_eq!(runs_setup_on_create(SetupRunPolicy::Ask), None);
        // The default is Orca's: run it.
        assert_eq!(SetupRunPolicy::default(), SetupRunPolicy::RunByDefault);
        assert_eq!(ScriptSource::default(), ScriptSource::SharedOnly);
    }

    #[test]
    fn a_script_is_told_which_checkout_it_is_preparing() {
        let env = script_env("/repo", "/repo/.worktrees/fix-login/");
        assert_eq!(env[0], (ROOT_PATH_VAR.into(), "/repo".into()));
        assert_eq!(
            env[1],
            (
                WORKTREE_PATH_VAR.into(),
                "/repo/.worktrees/fix-login/".into()
            )
        );
        // The name is the leaf, with a trailing slash not turning it into "".
        assert_eq!(env[2], (WORKSPACE_NAME_VAR.into(), "fix-login".into()));
        // Nothing in here carries another product's spelling.
        for (name, _) in &env {
            assert!(
                name.starts_with("ZEROCODE_"),
                "`{name}` is not this product's variable"
            );
        }
    }
}
