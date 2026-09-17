//! The second brain: an Obsidian-shaped Markdown vault zo consults before it
//! answers and writes back what a session actually learned.
//!
//! The vault is only a directory of Markdown — `raw/` holds untouched sources,
//! `wiki/` holds short atomic pages linked to each other by `[[wikilinks]]`,
//! and the vault's own `AGENTS.md` carries the house rules. Obsidian is an
//! optional reader, so nothing here depends on the application being installed.
//!
//! Three seams use it, and each pays only for what it needs:
//!
//! * **The system prompt** (`prompt::sections::render_second_brain_section`)
//!   names the vault and the protocol, once per session.
//! * **Recall** ([`corpus`]) merges `wiki/**/*.md` into the existing lexical
//!   memory index as a read-only corpus — no second retriever, no new ranking.
//! * **Session end** ([`promote`]) appends what the Dreamer promoted as new
//!   atomic pages plus one line in the vault's ingest log.
//!
//! Everything is off — at zero cost — when no vault is configured.
//!
//! # Where the path comes from
//!
//! [`VAULT_ENV`] is the single read-time source of truth, exactly as
//! `ZO_CUSTOM_PROVIDERS` is for the provider catalog: recall authorizes a page
//! path per rendered snippet, and a settings load on that path would be a file
//! read per turn. The `secondBrain.vault` setting reaches it through one
//! publish at startup ([`publish_vault_from_config`]), and an operator export
//! is never rewritten.

pub mod corpus;
pub mod promote;

use std::path::{Path, PathBuf};

use crate::config::RuntimeConfig;

/// Environment variable naming the vault. The `ZeroCode` window exports it into
/// every pane it opens; a bare terminal gets it from settings via
/// [`publish_vault_from_config`], or from the operator's own shell.
///
/// The spelling is a contract with the window, which writes the same string.
pub const VAULT_ENV: &str = "ZEROCODE_SECOND_BRAIN";
/// [`VAULT_ENV`]'s one word that is not a path: no vault in this process,
/// whatever settings declare. A blank value is "unset" and settings fill it,
/// so a run that must not read or write the person's vault (the Computer Use
/// bench) needs a word that says so.
pub const VAULT_OFF: &str = "off";

/// Settings object holding the vault path and the write-back switch.
pub const SETTINGS_KEY: &str = "secondBrain";
/// `secondBrain.vault` — absolute path of the vault.
pub const VAULT_SETTINGS_FIELD: &str = "vault";
/// `secondBrain.writeBack` — whether a session may append what it learned.
pub const WRITE_BACK_SETTINGS_FIELD: &str = "writeBack";

/// Untouched sources. Zo reads them and never writes there.
pub const RAW_DIR: &str = "raw";
/// Atomic knowledge pages — the corpus recall indexes and promotion appends to.
pub const WIKI_DIR: &str = "wiki";
/// The vault's own house rules.
///
/// zo does not load it: instruction-file discovery collects `context.md` and
/// its siblings, never `AGENTS.md`. The prompt section therefore names the file
/// and tells the model to read it, rather than assuming it is already there.
///
/// The window depends on this in the other direction, and says so at
/// `zerocode_core::second_brain::global_guide_file`: it writes its standing
/// second-brain block into each agent's global instruction file
/// (`~/.claude/CLAUDE.md`, `~/.codex/AGENTS.md`) and deliberately writes
/// nothing under `~/.zo`, because zo would never open it. zo's half of that
/// contract is [`VAULT_ENV`] plus the bundled skill — so a change here that
/// started reading a global guide file would leave zo reading a file nobody
/// writes. What guards the claim this section's own text makes is the
/// co-located prompt test, which writes a line into a vault `AGENTS.md` and
/// asserts it never reaches the prompt.
pub const AGENTS_FILE: &str = "AGENTS.md";
/// Map of content, relative to [`WIKI_DIR`].
pub const WIKI_INDEX_FILE: &str = "index.md";
/// Ingest log, relative to [`WIKI_DIR`]: one line per promotion.
pub const WIKI_LOG_FILE: &str = "log.md";
/// Subdirectory of [`WIKI_DIR`] that zo's own promotions land in, so a page a
/// person wrote and a page a session left are never confused.
pub const ZO_PAGE_DIR: &str = "zo";
/// The `source:` prefix stamped on a promoted page's frontmatter.
pub const SESSION_SOURCE_PREFIX: &str = "zo:session:";

/// Markdown extension. A wikilink names a page without it.
const PAGE_SUFFIX: &str = ".md";

/// Wikilink target of the vault's map of content, derived from
/// [`WIKI_INDEX_FILE`] so the file name and the link that points at it cannot
/// drift into two spellings.
#[must_use]
pub fn wiki_index_link() -> String {
    format!(
        "{WIKI_DIR}/{}",
        WIKI_INDEX_FILE.trim_end_matches(PAGE_SUFFIX)
    )
}

/// A configured vault. Holding one is the whole feature gate: every seam takes
/// `Option<SecondBrain>` and does nothing when it is `None`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecondBrain {
    root: PathBuf,
}

impl SecondBrain {
    /// A vault at an explicit root, for tests and for callers that already
    /// resolved the path themselves.
    #[must_use]
    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The vault named by [`VAULT_ENV`], if any. This is the read-time
    /// resolution every hot path uses: one `getenv`, no file IO.
    ///
    /// A relative path is refused rather than joined onto the current
    /// directory — the variable crosses a process boundary, so "relative to
    /// what" has no answer the reader can trust.
    #[must_use]
    pub fn from_env() -> Option<Self> {
        let raw = std::env::var(VAULT_ENV).ok()?;
        let root = PathBuf::from(raw.trim());
        (root.is_absolute()).then_some(Self { root })
    }

    /// The environment's vault, else the one merged settings declare — unless
    /// the environment says [`VAULT_OFF`]. Used by the surfaces that already
    /// hold a loaded config (prompt build, doctor, the startup publish) so
    /// none of them reads settings a second time.
    #[must_use]
    pub fn resolve(config: &RuntimeConfig) -> Option<Self> {
        if turned_off() {
            return None;
        }
        Self::from_env().or_else(|| {
            let root = PathBuf::from(config.second_brain().vault()?.trim());
            (root.is_absolute()).then_some(Self { root })
        })
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Where the atomic pages live.
    #[must_use]
    pub fn wiki_dir(&self) -> PathBuf {
        self.root.join(WIKI_DIR)
    }

    /// Where untouched sources live. Named so the prompt section and the
    /// promotion guard agree on the directory nothing may write to.
    #[must_use]
    pub fn raw_dir(&self) -> PathBuf {
        self.root.join(RAW_DIR)
    }

    /// The ingest log a promotion appends its one line to.
    #[must_use]
    pub fn log_path(&self) -> PathBuf {
        self.wiki_dir().join(WIKI_LOG_FILE)
    }

    /// Where this session's own promoted pages land.
    #[must_use]
    pub fn zo_pages_dir(&self) -> PathBuf {
        self.wiki_dir().join(ZO_PAGE_DIR)
    }

    /// Whether the vault has been set up — the `wiki/` directory exists. A
    /// configured path that was never created (or has been moved) is worth
    /// saying out loud in `--doctor` rather than silently doing nothing.
    #[must_use]
    pub fn is_set_up(&self) -> bool {
        self.wiki_dir().is_dir()
    }
}

/// Whether [`VAULT_ENV`] turns the vault off for this process.
fn turned_off() -> bool {
    std::env::var(VAULT_ENV).is_ok_and(|value| value.trim().eq_ignore_ascii_case(VAULT_OFF))
}

/// Mirror the settings-declared vault into [`VAULT_ENV`] so every read-time
/// resolution is a `getenv`.
///
/// An operator export always wins: the variable is only written when it is
/// absent or blank. That makes the bridge idempotent across the several
/// runtime rebuilds one session performs (`/model`, `/resume`, each headless
/// turn) — unlike the provider catalog, a vault path is a location rather than
/// a cache of a mutable list, so re-publishing a changed setting mid-session is
/// not worth the risk of stomping an export.
pub fn publish_vault_from_config(config: &RuntimeConfig) {
    // `off` is non-blank, so it is kept like any other export.
    if std::env::var(VAULT_ENV).is_ok_and(|value| !value.trim().is_empty()) {
        return;
    }
    let Some(vault) = config.second_brain().vault() else {
        return;
    };
    let vault = vault.trim();
    if Path::new(vault).is_absolute() {
        std::env::set_var(VAULT_ENV, vault);
    }
}

/// What `zo --doctor` (and any other status surface) needs to say one honest
/// line about the vault.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VaultStatus {
    /// The configured root, as configured — not canonicalized, so the line
    /// shows the reader the path they typed.
    pub root: String,
    /// Whether the vault has a `wiki/` directory for recall to read.
    pub ready: bool,
    /// Pages the recall corpus can see. Zero on a vault that is not ready.
    pub pages: usize,
    /// Whether the scan stopped at [`corpus::MAX_INDEXED_PAGES`].
    pub capped: bool,
}

/// The vault's status, or `None` when none is configured — an absent row says
/// "off" more honestly than a row saying "not configured".
#[must_use]
pub fn status(config: &RuntimeConfig) -> Option<VaultStatus> {
    let vault = SecondBrain::resolve(config)?;
    let root = vault.root().display().to_string();
    if !vault.is_set_up() {
        return Some(VaultStatus {
            root,
            ready: false,
            pages: 0,
            capped: false,
        });
    }
    let scan = corpus::scan(&vault);
    Some(VaultStatus {
        root,
        ready: true,
        pages: scan.pages.len(),
        capped: scan.capped,
    })
}

#[cfg(test)]
mod tests;
