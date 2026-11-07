//! The agents the store serves: how each sees the store's skills, where each
//! keeps its MCP list, and where each installs a skill by itself.
//!
//! Which agent reads the shared folder was read off each installed build
//! (2026-11-06), not assumed: Codex lists `.agents/skills` among its skill
//! homes, Gemini CLI has `getUserAgentSkillsDir`, Qwen Code's
//! `SKILL_PROVIDER_CONFIG_DIRS` is `[".qwen", ".agents"]`, Grok says it "also
//! scans `.agents/skills/`", and OpenCode walks `.agents` beside `.claude`.
//! Cursor's agent was not installed on the machine read; its own skills page
//! (cursor.com/docs/context/skills, read 2026-11-07) lists `~/.agents/skills/`
//! beside `~/.cursor/skills/` among the user folders it loads (the review of
//! the skills store, P3-2).
//! zo reads only its own roots (`skill_search_roots`), Claude Code reads its
//! config home's `skills` and nothing like a shared folder (its skills page),
//! and Kiro and Antigravity name no such folder.

use super::mcp::Format;

/// How an agent comes to see the store's skills.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Skills {
    /// It reads `~/.agents/skills` by itself.
    Native,
    /// Claude Code in the window: the `skills` folder of the home the window
    /// launches it with (its own, `CLAUDE_CONFIG_DIR`) is the store.
    WindowHome,
    /// zo: it reads the store while the window's terminals say so
    /// ([`super::ZO_COMMON_SKILLS_ENV`]).
    WindowEnv,
    /// It reads only its own folder, and nothing tells it of another.
    OwnOnly,
}

impl Skills {
    /// Whether the agent reads the store wherever it runs — what lets a skill
    /// leave its own folder for the store, and a shipped skill be written to
    /// the store for it. Claude Code reads the store only through the
    /// window's home and zo only in the window's terminals, so their own
    /// folders keep their copies.
    #[must_use]
    pub fn reads_the_store_everywhere(self) -> bool {
        matches!(self, Self::Native)
    }
}

/// Where an agent keeps its user list of MCP servers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpPlace {
    /// A file under the person's home.
    Home(&'static str),
    /// `.claude.json` in the home the window launches Claude Code with.
    ClaudeWindow,
    /// `opencode.json` under the XDG config home (`~/.config` when unset).
    OpenCode,
}

/// One agent the store serves.
#[derive(Debug, Clone, Copy)]
pub struct LibraryAgent {
    /// The window's agent id where the agent catalog has one (`qwen-code`,
    /// `antigravity`), else the library's own (`gemini`).
    pub id: &'static str,
    pub label: &'static str,
    /// Whether the window's agent catalog knows it; one it does not is found
    /// by its command on the shell's PATH.
    pub in_catalog: bool,
    /// The command its MCP list is written through, and found by.
    pub binary: &'static str,
    pub skills: Skills,
    /// Folders under the home it installs skills into by itself — watched, so
    /// a skill it installs reaches the store. One written under `.config/` is
    /// under the XDG config home, where the agent reads it.
    pub own_skills: &'static [&'static str],
    pub mcp: McpPlace,
    pub format: Format,
}

/// Every agent the store serves, in the order a person reads them and the
/// order a new server's origin is credited in.
pub const AGENTS: [LibraryAgent; 10] = [
    LibraryAgent {
        id: "claude",
        label: "Claude Code",
        in_catalog: true,
        binary: "claude",
        skills: Skills::WindowHome,
        own_skills: &[],
        mcp: McpPlace::ClaudeWindow,
        format: Format::Claude,
    },
    LibraryAgent {
        id: "codex",
        label: "Codex",
        in_catalog: true,
        binary: "codex",
        skills: Skills::Native,
        own_skills: &[".codex/skills"],
        mcp: McpPlace::Home(".codex/config.toml"),
        format: Format::Codex,
    },
    LibraryAgent {
        id: "zo",
        label: "zo",
        in_catalog: true,
        binary: "zo",
        skills: Skills::WindowEnv,
        own_skills: &[".zo/skills"],
        mcp: McpPlace::Home(".zo/settings.json"),
        format: Format::Zo,
    },
    LibraryAgent {
        id: "gemini",
        label: "Gemini CLI",
        in_catalog: false,
        binary: "gemini",
        skills: Skills::Native,
        own_skills: &[".gemini/skills"],
        mcp: McpPlace::Home(".gemini/settings.json"),
        format: Format::Gemini,
    },
    LibraryAgent {
        id: "qwen-code",
        label: "Qwen Code",
        in_catalog: true,
        binary: "qwen",
        skills: Skills::Native,
        own_skills: &[".qwen/skills"],
        mcp: McpPlace::Home(".qwen/settings.json"),
        format: Format::Qwen,
    },
    LibraryAgent {
        id: "grok",
        label: "Grok",
        in_catalog: true,
        binary: "grok",
        skills: Skills::Native,
        own_skills: &[".grok/skills"],
        mcp: McpPlace::Home(".grok/config.toml"),
        format: Format::Grok,
    },
    LibraryAgent {
        id: "antigravity",
        label: "Antigravity",
        in_catalog: true,
        binary: "agy",
        skills: Skills::OwnOnly,
        own_skills: &[".gemini/antigravity/skills"],
        mcp: McpPlace::Home(".gemini/config/mcp_config.json"),
        format: Format::Antigravity,
    },
    LibraryAgent {
        id: "opencode",
        label: "OpenCode",
        in_catalog: true,
        binary: "opencode",
        skills: Skills::Native,
        own_skills: &[".config/opencode/skills"],
        mcp: McpPlace::OpenCode,
        format: Format::OpenCode,
    },
    LibraryAgent {
        id: "kiro",
        label: "Kiro",
        in_catalog: true,
        binary: "kiro-cli",
        skills: Skills::OwnOnly,
        own_skills: &[".kiro/skills"],
        mcp: McpPlace::Home(".kiro/settings/mcp.json"),
        format: Format::Kiro,
    },
    LibraryAgent {
        id: "cursor",
        label: "Cursor",
        in_catalog: true,
        binary: "cursor-agent",
        skills: Skills::Native,
        own_skills: &[".cursor/skills"],
        mcp: McpPlace::Home(".cursor/mcp.json"),
        format: Format::Cursor,
    },
];

/// The id the person's own Claude Code — the one in their terminal, outside
/// the window — is read under. Read only: the window never writes the
/// person's Claude home (`accounts`), so a skill or a server found there is
/// copied into the store and the home stays as it was.
pub const MAC_CLAUDE: &str = "claude-mac";
/// Its skills folder, under the home.
pub const MAC_CLAUDE_SKILLS: &str = ".claude/skills";
/// Its user list, under the home.
pub const MAC_CLAUDE_MCP: &str = ".claude.json";

/// The agent by id.
#[must_use]
pub fn agent(id: &str) -> Option<&'static LibraryAgent> {
    AGENTS.iter().find(|agent| agent.id == id)
}

/// Where an origin is said to have come from, for a person: an agent's
/// label, the person's own Claude, or the window.
#[must_use]
pub fn origin_label(id: &str) -> &'static str {
    match id {
        MAC_CLAUDE => "Claude Code (Mac)",
        "person" => "person",
        "bundle" => "ZeroCode",
        other => agent(other).map_or("agent", |agent| agent.label),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_agent_is_one_id_and_one_list() {
        let mut ids: Vec<&str> = AGENTS.iter().map(|agent| agent.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), AGENTS.len());
        assert!(agent("claude").is_some() && agent(MAC_CLAUDE).is_none());
        assert_eq!(origin_label("qwen-code"), "Qwen Code");
        assert_eq!(origin_label(MAC_CLAUDE), "Claude Code (Mac)");
    }

    /// The catalog ids are the window's own, so the library's rows and the
    /// agent picker name one agent the same way.
    #[test]
    fn a_catalog_agent_carries_the_catalogs_id() {
        for library in AGENTS.iter().filter(|agent| agent.in_catalog) {
            assert!(
                crate::agent::agent_spec(library.id).is_some(),
                "{} is not a catalog id",
                library.id
            );
        }
    }

    #[test]
    fn only_an_agent_that_reads_the_store_everywhere_lets_a_skill_leave_its_folder() {
        assert!(Skills::Native.reads_the_store_everywhere());
        assert!(!Skills::WindowHome.reads_the_store_everywhere());
        assert!(!Skills::WindowEnv.reads_the_store_everywhere());
        assert!(!Skills::OwnOnly.reads_the_store_everywhere());
    }
}
