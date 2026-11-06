//! The common store: one place for the skills and MCP servers every agent in
//! the window reads (2026-11-06, the person: 「codex에서 a 스킬 설치하고 cc에서
//! b 스킬 설치시 gemini는 ab를 다 봐야함 cc도 ab를 다 보고」, 「링크를 거는 게
//! 아닌 공통 스킬 mcp 저장소를 보게」).
//!
//! Before this module each agent kept its own list: a skill Codex installed
//! lived in `~/.codex/skills` and no other agent saw it, and the same MCP server
//! stood in seven configuration files under three spellings. The store is two
//! places under the person's home:
//!
//! - `~/.agents/skills/` — the shared skills folder the open `skills` tool
//!   installs into, and the one Codex, Gemini CLI, Qwen Code, Grok and OpenCode
//!   already read without being told ([`agents::Skills::Native`]).
//! - `~/.agents/mcp.json` — `{"mcpServers": {…}}` in Claude's spelling, which
//!   no agent reads by itself; the window writes each server into every
//!   agent's own list through that agent's own command ([`agents::LibraryAgent::binary`]).
//!
//! What this module is: the shapes (`mcp`), the agent table (`agents`), the
//! record the window keeps of what it did (`ledger`) and the pure planner that
//! turns what it sees on disk into what to do (`plan`). Nothing here touches
//! the disk; `library_runtime` in the shell reads, runs the plan and writes.

use std::path::{Path, PathBuf};

pub mod agents;
pub mod ledger;
pub mod mcp;
pub mod plan;

/// The folder under the person's home the store lives in.
pub const STORE_DIR: &str = ".agents";
/// The skills folder inside [`STORE_DIR`].
pub const STORE_SKILLS: &str = "skills";
/// The MCP list inside [`STORE_DIR`].
pub const STORE_MCP: &str = "mcp.json";

/// The environment variable the window sets in its terminals so that zo also
/// reads the store's skills. zo reads only its own roots by default — a
/// prompt that grows with whatever another product installed is not one
/// anybody can reason about (`skill_sources::PROVIDER_SKILLS_ENV`) — and the
/// store is borrowed only while zo runs inside the window, which is the
/// person's own rule: 「zerocode ide를 사용하는 이상」.
pub const ZO_COMMON_SKILLS_ENV: &str = "ZO_COMMON_SKILLS";

/// Where the store keeps its skills.
#[must_use]
pub fn store_skills_dir(home: &Path) -> PathBuf {
    home.join(STORE_DIR).join(STORE_SKILLS)
}

/// Where the store keeps its MCP list.
#[must_use]
pub fn store_mcp_file(home: &Path) -> PathBuf {
    home.join(STORE_DIR).join(STORE_MCP)
}

/// Whether a folder name inside a skills folder is one the store takes.
///
/// A dot folder is an agent's own (`.system` holds the skills an agent ships
/// with), and `synced` is where Claude Code keeps the skills of the person's
/// claude.ai account — managed by that account, never by this store.
#[must_use]
pub fn is_skill_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && name != CLAUDE_SYNCED
        && !name.contains('/')
        && !name.contains('\\')
        && name != ".."
}

/// The folder Claude Code writes the claude.ai account's skills into, inside
/// its skills folder.
pub const CLAUDE_SYNCED: &str = "synced";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_store_is_two_places_under_the_home() {
        let home = Path::new("/Users/dev");
        assert_eq!(
            store_skills_dir(home),
            PathBuf::from("/Users/dev/.agents/skills")
        );
        assert_eq!(
            store_mcp_file(home),
            PathBuf::from("/Users/dev/.agents/mcp.json")
        );
    }

    #[test]
    fn hidden_folders_and_claudes_account_skills_are_not_store_skills() {
        assert!(is_skill_name("pdf"));
        assert!(is_skill_name("release-notes"));
        for refused in [".system", ".git", "synced", "", "a/b", ".."] {
            assert!(!is_skill_name(refused), "{refused} was taken");
        }
    }
}
