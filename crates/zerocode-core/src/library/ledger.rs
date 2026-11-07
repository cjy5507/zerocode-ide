//! What the window remembers between looks: where each item came from, what
//! it last saw in every agent's list, and what was removed.
//!
//! The store's files say what IS; this record says what the window DID, which
//! is what lets one look tell an agent's edit from the window's own write and
//! an agent's removal from a list the window never wrote. It holds no secret:
//! a server is remembered by its fingerprint ([`super::mcp::McpDef::fingerprint`])
//! and a one-line summary with every credential masked.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

/// The record, as `library/ledger.json` under the window's data holds it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Ledger {
    pub version: u32,
    /// Off only by the person's own switch; the store is kept in step
    /// otherwise.
    pub paused: bool,
    /// When the first look took every agent's lists into the store.
    pub merged_at: Option<i64>,
    pub mcp: BTreeMap<String, McpRecord>,
    pub skills: BTreeMap<String, SkillRecord>,
    /// Servers removed, by the person or by an agent, and when. A copy left
    /// in a list the window may not write (the person's own Claude) is not
    /// taken back in while its name stands here.
    pub gone_mcp: BTreeMap<String, i64>,
    /// The same, for skills.
    pub gone_skills: BTreeMap<String, i64>,
    /// What a removed skill left behind: every folder that still held its
    /// name, and its text, at the first look after the removal. Such a copy
    /// stays out of the store; a folder that is not one of them — an agent
    /// installing the skill again, or the person changing a copy — brings
    /// the name back (the review of v1.1.128, P2-4). A removed name missing
    /// here is taken stock of at the next look.
    pub gone_skill_left: BTreeMap<String, Vec<SkillCandidate>>,
    /// Whether v1.1.128's 「가져옴 되돌리기」 — `own` on a server not bound
    /// to its agent — has been moved to [`McpRecord::kept_with`] (the review
    /// of t-98860, P2-1). Absent from a file v1.1.128 wrote, so its first look
    /// here moves it once; a file v1.1.128 writes again drops it, and the next
    /// look moves once more what that build wrote in the old way.
    pub kept_moved: bool,
    /// Servers of that move still waiting for their agent's list to be read —
    /// bound or kept is told only by the version that list holds. The move
    /// applies to these names alone; the rest of the ledger is past it (the
    /// reviews of t-99089 P3-G and t-99137 P3-G2).
    pub kept_waiting: BTreeSet<String>,
}

/// The ledger's shape today; a record written by another shape is read as
/// empty rather than trusted. It stays 1 while v1.1.128 may read the same
/// file: that build reads only 1, and would read another as an empty ledger
/// and write that back (the second review of t-98860, P2-1). A new field
/// rides `#[serde(default)]` instead — see [`Ledger::kept_moved`].
pub const VERSION: u32 = 1;

/// One server.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct McpRecord {
    /// The agent it was first found in, `person` for one added in the
    /// window, or the person's own Claude.
    pub origin: String,
    pub at_ms: i64,
    /// Agents the person switched it off for.
    pub off: BTreeSet<String>,
    /// The agent it only works for ([`super::mcp::McpDef::bound_to_its_agent`]),
    /// while the person has not shared it anyway.
    pub own: Option<String>,
    /// The person shared a server bound to one agent with every agent.
    pub shared: bool,
    /// The agents the person kept a server taken in with (「가져옴
    /// 되돌리기」): every list that held it then. It stays out of the store
    /// and the others; an agent leaves here only once a look read its list,
    /// not reset, without the server. Apart from [`Self::own`], so a list
    /// read for a moment as empty, or a bound server's command changed,
    /// neither undoes the person's choice nor makes one (the review of
    /// v1.1.128 skills-p2, P2②).
    pub kept_with: BTreeSet<String>,
    /// The fingerprint each agent's list held at the window's last look or
    /// write. An agent missing here held nothing the window knows of.
    pub seen: BTreeMap<String, String>,
    /// The versions the person is asked to choose between, while they have
    /// not.
    pub pick: Vec<Candidate>,
}

/// One version of a server the person is asked to choose.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Candidate {
    /// The lists that hold this version (`store` for the store's own).
    pub agents: Vec<String>,
    pub fingerprint: String,
    /// Credential-masked, for the person to tell the versions apart.
    pub summary: String,
    pub remote: bool,
}

/// One skill.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SkillRecord {
    /// The agent whose folder it was found in, `person`, `store` for one
    /// that was in the store before the window looked, or `bundle`.
    pub origin: String,
    pub at_ms: i64,
    /// The digest the store's copy had at the window's last look.
    pub digest: String,
    /// The folder a copy was taken from, for a folder the window may not
    /// move (the person's own Claude, an agent that reads only its own).
    pub copied_from: Option<String>,
    /// The digest that folder had when it was copied — a later digest there
    /// is the source updated, and the copy follows.
    pub source_digest: Option<String>,
    /// Folders holding another text under the same name, the person asked
    /// which stays.
    pub pick: Vec<SkillCandidate>,
    /// Texts the person decided against: a folder still holding one is not
    /// asked about again.
    pub kept_over: Vec<String>,
}

/// One version of a skill the person is asked to choose.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SkillCandidate {
    pub agent: String,
    pub path: String,
    pub digest: String,
}

impl Ledger {
    /// A record in the shape this build reads, or an empty one.
    #[must_use]
    pub fn read(text: &str) -> Self {
        match serde_json::from_str::<Self>(text) {
            Ok(ledger) if ledger.version == VERSION => ledger,
            _ => Self::new(),
        }
    }

    /// A fresh record.
    #[must_use]
    pub fn new() -> Self {
        Self {
            version: VERSION,
            kept_moved: true,
            ..Self::default()
        }
    }

    /// A skill the store no longer holds: its record goes and its name is
    /// remembered as removed. What it left behind is taken stock of by the
    /// next look ([`Self::gone_skill_left`]).
    pub fn skill_removed(&mut self, name: &str, now: i64) {
        self.skills.remove(name);
        self.gone_skills.insert(name.to_string(), now);
        self.gone_skill_left.remove(name);
    }

    /// The copies a removed skill left behind that still stand, by
    /// `stands(path)`. One the person deleted is no longer a copy left
    /// behind: a folder installed there later, the same text too, is an
    /// install (the review of v1.1.128 skills-p2, P3-4). A name stays
    /// stocked even when nothing it left stands.
    pub fn keep_left_copies_that_stand(&mut self, stands: impl Fn(&str) -> bool) {
        for left in self.gone_skill_left.values_mut() {
            left.retain(|copy| stands(copy.path.as_str()));
        }
    }

    /// A removed skill's name is free again: the person put it back, or an
    /// agent installed it anew.
    pub fn skill_back(&mut self, name: &str) {
        self.gone_skills.remove(name);
        self.gone_skill_left.remove(name);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_record_of_another_shape_reads_as_empty() {
        assert_eq!(Ledger::read("not json"), Ledger::new());
        assert_eq!(
            Ledger::read(r#"{"version":99,"paused":true}"#),
            Ledger::new()
        );
        let mut kept = Ledger::new();
        kept.paused = true;
        kept.gone_mcp.insert("x".into(), 1);
        let text = serde_json::to_string(&kept).unwrap();
        assert_eq!(Ledger::read(&text), kept);
    }

    /// A name whose every copy is gone stays stocked, so the next look does
    /// not take the folders standing then for copies left behind.
    #[test]
    fn a_name_whose_copies_are_all_gone_stays_stocked() {
        let mut ledger = Ledger::new();
        let copy = |path: &str| SkillCandidate {
            agent: "codex".into(),
            path: path.into(),
            digest: "v1".into(),
        };
        ledger.gone_skill_left.insert(
            "b".into(),
            vec![copy("/Users/dev/a/b"), copy("/Users/dev/c/b")],
        );
        ledger.keep_left_copies_that_stand(|path| path == "/Users/dev/c/b");
        assert_eq!(ledger.gone_skill_left["b"], vec![copy("/Users/dev/c/b")]);
        ledger.keep_left_copies_that_stand(|_| false);
        assert_eq!(ledger.gone_skill_left.get("b"), Some(&Vec::new()));
    }
}
