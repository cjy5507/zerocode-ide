//! From what one look found to what to do: the store kept in step with every
//! agent, without a person doing anything.
//!
//! The rules, the person's (2026-11-06) and the store's own:
//!
//! - **One list.** A server an agent adds, a skill an agent installs, goes
//!   into the store, and from there into every other agent.
//! - **Every change is a change everywhere.** An agent that edits a server
//!   the store gave it edits the store's; an agent that removes it removes it
//!   from every agent. What tells an edit from the window's own write is the
//!   ledger's `seen`: a list that held the store's version and now holds
//!   another was changed there.
//! - **Two versions are a question, never a guess.** Where two lists hold
//!   different servers under one name — the first look on a machine whose
//!   agents were set up one by one — nothing is written over; the person is
//!   asked which ([`Ledger::mcp`]`.pick`).
//! - **What only works for one agent stays with it**
//!   ([`McpDef::bound_to_its_agent`]).
//! - **The person's own Claude is read, never written.** A server or a skill
//!   found there is taken in; its absence there removes nothing, because the
//!   window never put anything there.
//!
//! The plan is a list of steps and the ledger they leave. A step that writes
//! an agent's list records what it wrote only when it succeeds — the runtime
//! does that ([`Ledger`]'s `seen`), because a failed write recorded as done
//! would read, next look, as the agent removing the server.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use super::agents::{AGENTS, MAC_CLAUDE};
use super::ledger::{Candidate, Ledger, McpRecord, SkillCandidate, SkillRecord};
use super::mcp::{McpDef, McpEntry};

/// One agent's MCP list as the look read it.
#[derive(Debug, Clone)]
pub struct AgentList {
    pub agent: String,
    /// False for the person's own Claude.
    pub writable: bool,
    pub servers: BTreeMap<String, McpEntry>,
}

/// What one look found: the store and every list that could be read. A list
/// that could not be read is left out, and so is never written over.
#[derive(Debug, Clone, Default)]
pub struct McpView {
    pub store: BTreeMap<String, McpEntry>,
    pub lists: Vec<AgentList>,
}

/// One thing to do to the MCP lists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpStep {
    /// Put the server into the store, or replace the store's.
    Store { name: String, entry: McpEntry },
    /// Take it out of the store.
    Unstore { name: String },
    /// Write it into an agent's list, replacing what stands there.
    Push {
        agent: String,
        name: String,
        entry: McpEntry,
    },
    /// Take it out of an agent's list.
    Pull { agent: String, name: String },
}

/// What happened, for the person's 「최근 변경」.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Note {
    /// A server or skill found in one agent's list became the store's.
    TakenIn { name: String, from: String },
    /// An agent changed a server the store gave it; the store follows.
    Edited { name: String, at: String },
    /// An agent removed it; it goes from every agent.
    Removed { name: String, at: String },
    /// Two versions under one name: the person is asked.
    Asked { name: String },
    /// A list that lost every server the window wrote there at once was read
    /// as reset, and the store's servers are written back into it: one
    /// removed there on purpose is to be removed from the store.
    Reset { at: String },
}

/// The steps one look calls for, and the ledger they leave.
#[derive(Debug, Clone)]
pub struct McpPlan {
    pub steps: Vec<McpStep>,
    pub ledger: Ledger,
    pub notes: Vec<Note>,
}

fn rank(agent: &str) -> usize {
    AGENTS
        .iter()
        .position(|known| known.id == agent)
        .unwrap_or(AGENTS.len())
}

fn candidate(agents: Vec<String>, def: &McpDef) -> Candidate {
    Candidate {
        agents,
        fingerprint: def.fingerprint(),
        summary: def.summary(),
        remote: def.is_remote(),
    }
}

/// The lists that hold `name`, grouped by version, in agent order.
fn versions<'a>(lists: &'a [AgentList], name: &str) -> Vec<(String, &'a McpEntry, Vec<String>)> {
    let mut groups: Vec<(String, &McpEntry, Vec<String>)> = Vec::new();
    let mut holders: Vec<(&AgentList, &McpEntry)> = lists
        .iter()
        .filter_map(|list| list.servers.get(name).map(|entry| (list, entry)))
        .collect();
    holders.sort_by_key(|(list, _)| rank(&list.agent));
    for (list, entry) in holders {
        let fingerprint = entry.def.fingerprint();
        match groups.iter_mut().find(|(held, _, _)| *held == fingerprint) {
            Some((_, _, agents)) => agents.push(list.agent.clone()),
            None => groups.push((fingerprint, entry, vec![list.agent.clone()])),
        }
    }
    groups
}

/// Every list the store should write `name` into, given the record.
fn wanted(list: &AgentList, record: &McpRecord, entry: &McpEntry) -> bool {
    if !list.writable || entry.disabled || record.off.contains(&list.agent) {
        return false;
    }
    match (&record.own, record.shared) {
        (Some(own), false) => *own == list.agent,
        _ => true,
    }
}

/// The lists that lost, at once, every server the window had seen in them —
/// a file deleted, emptied or rewritten (a home reset, an agent reinstalled).
/// Such a list is read as reset, not as the person removing each server: the
/// window writes the store's servers back into it and takes nothing from the
/// store or the other lists. A removal counts only from a list that still
/// holds another server the window wrote there.
fn reset_lists(view: &McpView, ledger: &Ledger) -> BTreeSet<String> {
    view.lists
        .iter()
        .filter(|list| {
            let mut seen_here = ledger
                .mcp
                .iter()
                .filter(|(_, record)| record.seen.contains_key(&list.agent))
                .peekable();
            seen_here.peek().is_some()
                && seen_here.all(|(name, _)| !list.servers.contains_key(name))
        })
        .map(|list| list.agent.clone())
        .collect()
}

/// Plan one look over the MCP lists.
#[must_use]
pub fn plan_mcp(view: &McpView, ledger: &Ledger, now: i64) -> McpPlan {
    let mut next = ledger.clone();
    let mut steps = Vec::new();
    let mut notes = Vec::new();
    let mut names: BTreeSet<String> = view.store.keys().cloned().collect();
    for list in &view.lists {
        names.extend(list.servers.keys().cloned());
    }
    names.extend(ledger.mcp.keys().cloned());
    let reset = reset_lists(view, ledger);
    // A ledger of v1.1.128 wrote the person keeping a server with its agent
    // as `own` on a server not bound to it; this look moves that to
    // `kept_with` once (the review of t-98860, P2-1) — and later only for a
    // name still waiting for its agent's list.
    let moving_all = !ledger.kept_moved;
    next.kept_moved = true;
    next.kept_waiting.clear();

    for name in names {
        let mut record = next.mcp.remove(&name).unwrap_or_default();
        // A reset list holds nothing the window wrote: forget that it did, so
        // a write back that fails for one server is tried again on the next
        // look instead of reading as the person removing it there.
        for agent in &reset {
            record.seen.remove(agent);
        }
        let mut stored = view.store.get(&name).cloned();

        // A. Removed: a copy the window left in a list it writes goes; one an
        // agent put back (a version the window never saw there) brings the
        // name back.
        if next.gone_mcp.contains_key(&name) {
            let mut back = false;
            for list in view.lists.iter().filter(|list| list.writable) {
                let Some(entry) = list.servers.get(&name) else {
                    continue;
                };
                let fingerprint = entry.def.fingerprint();
                if record.seen.get(&list.agent) == Some(&fingerprint) {
                    steps.push(McpStep::Pull {
                        agent: list.agent.clone(),
                        name: name.clone(),
                    });
                } else {
                    back = true;
                }
            }
            if !back {
                if stored.is_some() {
                    steps.push(McpStep::Unstore { name: name.clone() });
                }
                if !record.seen.is_empty() {
                    next.mcp.insert(name, record);
                }
                continue;
            }
            next.gone_mcp.remove(&name);
            steps.retain(
                |step| !matches!(step, McpStep::Pull { name: pulled, .. } if *pulled == name),
            );
            record = McpRecord::default();
            stored = None;
        }

        let groups = versions(&view.lists, &name);

        // B. Not in the store yet.
        if stored.is_none() {
            // Every version bound to its agent — none held is no version
            // that is not.
            let all_bound = groups
                .iter()
                .all(|(_, entry, _)| entry.def.bound_to_its_agent());
            let from_own = moving_all || ledger.kept_waiting.contains(&name);
            // Its agent's list unread: bound or kept is told only by the
            // version that list holds, so the record waits, `own` and all, by
            // its own name (the reviews of t-98860 P3-1, t-99089 P3-G and
            // t-99137 P3-G2).
            if from_own
                && !record.shared
                && record.kept_with.is_empty()
                && record
                    .own
                    .as_ref()
                    .is_some_and(|own| !view.lists.iter().any(|list| list.agent == *own))
            {
                next.kept_waiting.insert(name.clone());
                next.mcp.insert(name, record);
                continue;
            }
            if from_own
                && !all_bound
                && !record.shared
                && record.kept_with.is_empty()
                && let Some(own) = record.own.take()
            {
                record.kept_with.insert(own);
            }
            // Kept with its agents by the person (「가져옴 되돌리기」): the
            // copies the window wrote into the others go, and it is not taken
            // in again. A version an agent holds that the window never wrote
            // there is that agent's own and stays. An agent lets it go only
            // when its list was read, not reset, without it; once all have,
            // it is an ordinary server again.
            if !record.kept_with.is_empty() {
                record.kept_with.retain(|agent| {
                    match view.lists.iter().find(|list| list.agent == *agent) {
                        Some(list) if !reset.contains(agent) => list.servers.contains_key(&name),
                        _ => true,
                    }
                });
                for list in view.lists.iter().filter(|list| list.writable) {
                    let Some(held) = list.servers.get(&name) else {
                        continue;
                    };
                    let theirs = held.def.fingerprint();
                    if record.kept_with.contains(&list.agent) {
                        record.seen.insert(list.agent.clone(), theirs);
                    } else if record.seen.get(&list.agent) == Some(&theirs) {
                        steps.push(McpStep::Pull {
                            agent: list.agent.clone(),
                            name: name.clone(),
                        });
                    }
                }
                next.mcp.insert(name, record);
                continue;
            }
            if groups.is_empty() {
                // Nothing holds it any more: a question nobody can answer goes.
                continue;
            }
            if !record.pick.is_empty() {
                // Waiting for the person: the versions are refreshed, nothing
                // is written.
                record.pick = groups
                    .iter()
                    .map(|(_, entry, agents)| candidate(agents.clone(), &entry.def))
                    .collect();
                next.mcp.insert(name, record);
                continue;
            }
            if all_bound && !record.shared {
                let owner = groups[0].2[0].clone();
                if record.own.as_deref() != Some(owner.as_str()) {
                    record.origin.clone_from(&owner);
                    record.at_ms = now;
                }
                record.own = Some(owner);
                for (fingerprint, _, agents) in &groups {
                    for agent in agents {
                        record.seen.insert(agent.clone(), fingerprint.clone());
                    }
                }
                next.mcp.insert(name, record);
                continue;
            }
            if groups.len() > 1 {
                record.pick = groups
                    .iter()
                    .map(|(_, entry, agents)| candidate(agents.clone(), &entry.def))
                    .collect();
                if record.origin.is_empty() {
                    record.origin.clone_from(&groups[0].2[0]);
                    record.at_ms = now;
                }
                notes.push(Note::Asked { name: name.clone() });
                next.mcp.insert(name, record);
                continue;
            }
            let (fingerprint, entry, agents) = &groups[0];
            let holders_disabled = view
                .lists
                .iter()
                .filter_map(|list| list.servers.get(&name))
                .all(|held| held.disabled);
            let taken = McpEntry {
                def: entry.def.clone(),
                disabled: holders_disabled,
            };
            steps.push(McpStep::Store {
                name: name.clone(),
                entry: taken.clone(),
            });
            record.origin.clone_from(&agents[0]);
            record.at_ms = now;
            record.own = None;
            for agent in agents {
                record.seen.insert(agent.clone(), fingerprint.clone());
            }
            notes.push(Note::TakenIn {
                name: name.clone(),
                from: agents[0].clone(),
            });
            stored = Some(taken);
        }

        let Some(mut entry) = stored else {
            continue;
        };
        record.pick.clear();
        let mut fingerprint = entry.def.fingerprint();

        // C1. Edits: a list that held the store's version and holds another.
        let mut edits: Vec<(String, McpEntry)> = Vec::new();
        for list in &view.lists {
            if let Some(held) = list.servers.get(&name) {
                let theirs = held.def.fingerprint();
                if theirs != fingerprint && record.seen.get(&list.agent) == Some(&fingerprint) {
                    edits.push((list.agent.clone(), held.clone()));
                }
            }
        }
        edits.sort_by_key(|(agent, _)| rank(agent));
        if !edits.is_empty() {
            let first = edits[0].1.def.fingerprint();
            if edits
                .iter()
                .all(|(_, held)| held.def.fingerprint() == first)
            {
                let at = edits[0].0.clone();
                entry = McpEntry {
                    def: edits[0].1.def.clone(),
                    disabled: entry.disabled,
                };
                fingerprint = first;
                steps.push(McpStep::Store {
                    name: name.clone(),
                    entry: entry.clone(),
                });
                for (agent, _) in &edits {
                    record.seen.insert(agent.clone(), fingerprint.clone());
                }
                notes.push(Note::Edited {
                    name: name.clone(),
                    at,
                });
            } else {
                let mut asked = vec![candidate(vec!["store".to_string()], &entry.def)];
                for (agent, held) in &edits {
                    let theirs = held.def.fingerprint();
                    match asked.iter_mut().find(|known| known.fingerprint == theirs) {
                        Some(known) => known.agents.push(agent.clone()),
                        None => asked.push(candidate(vec![agent.clone()], &held.def)),
                    }
                }
                record.pick = asked;
                notes.push(Note::Asked { name: name.clone() });
                next.mcp.insert(name, record);
                continue;
            }
        }

        // C2. Removals: a list the store wrote, that held its version, and
        // holds nothing now. Only lists the window writes count, and a list
        // that lost every server the window saw in it is a reset (C3 writes
        // them back), not a removal.
        if !entry.disabled && edits.is_empty() {
            let removed_at = view
                .lists
                .iter()
                .filter(|list| wanted(list, &record, &entry))
                .filter(|list| !reset.contains(&list.agent))
                .filter(|list| !list.servers.contains_key(&name))
                .filter(|list| record.seen.get(&list.agent) == Some(&fingerprint))
                .map(|list| list.agent.clone())
                .min_by_key(|agent| rank(agent));
            if let Some(at) = removed_at {
                steps.push(McpStep::Unstore { name: name.clone() });
                for list in view.lists.iter().filter(|list| list.writable) {
                    if let Some(held) = list.servers.get(&name) {
                        let theirs = held.def.fingerprint();
                        if theirs == fingerprint || record.seen.get(&list.agent) == Some(&theirs) {
                            steps.push(McpStep::Pull {
                                agent: list.agent.clone(),
                                name: name.clone(),
                            });
                        }
                    }
                }
                record.seen.remove(&at);
                record.pick.clear();
                next.gone_mcp.insert(name.clone(), now);
                notes.push(Note::Removed {
                    name: name.clone(),
                    at,
                });
                next.mcp.insert(name, record);
                continue;
            }
        }

        // C3. In step: every list gets the store's version or loses the
        // window's copy; a version the window never saw there is asked about.
        let mut diverged: Vec<(String, McpEntry)> = Vec::new();
        for list in &view.lists {
            let held = list.servers.get(&name);
            let theirs = held.map(|held| held.def.fingerprint());
            if !list.writable {
                if theirs.as_ref() == Some(&fingerprint) {
                    record.seen.insert(list.agent.clone(), fingerprint.clone());
                }
                continue;
            }
            if wanted(list, &record, &entry) {
                match (held, theirs) {
                    (None, _) => steps.push(McpStep::Push {
                        agent: list.agent.clone(),
                        name: name.clone(),
                        entry: entry.clone(),
                    }),
                    (Some(_), Some(theirs)) if theirs == fingerprint => {
                        record.seen.insert(list.agent.clone(), fingerprint.clone());
                    }
                    (Some(_), Some(theirs)) if record.seen.get(&list.agent) == Some(&theirs) => {
                        steps.push(McpStep::Push {
                            agent: list.agent.clone(),
                            name: name.clone(),
                            entry: entry.clone(),
                        });
                    }
                    (Some(held), _) => diverged.push((list.agent.clone(), held.clone())),
                }
            } else if let (Some(_), Some(theirs)) = (held, theirs)
                && (theirs == fingerprint || record.seen.get(&list.agent) == Some(&theirs))
                && (record.own.as_deref() != Some(list.agent.as_str()) || record.shared)
            {
                steps.push(McpStep::Pull {
                    agent: list.agent.clone(),
                    name: name.clone(),
                });
            }
        }
        if !diverged.is_empty() {
            let mut asked = vec![candidate(vec!["store".to_string()], &entry.def)];
            for (agent, held) in &diverged {
                let theirs = held.def.fingerprint();
                match asked.iter_mut().find(|known| known.fingerprint == theirs) {
                    Some(known) => known.agents.push(agent.clone()),
                    None => asked.push(candidate(vec![agent.clone()], &held.def)),
                }
            }
            record.pick = asked;
            notes.push(Note::Asked { name: name.clone() });
        }
        next.mcp.insert(name, record);
    }
    // A reset list written back says so once: the person who removed every
    // server there on purpose sees why they came back (the review of the
    // v1.1.128 integration, P3).
    for agent in reset {
        if steps
            .iter()
            .any(|step| matches!(step, McpStep::Push { agent: to, .. } if *to == agent))
        {
            notes.push(Note::Reset { at: agent });
        }
    }
    McpPlan {
        steps,
        ledger: next,
        notes,
    }
}

/// One skill folder as the look found it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillFolder {
    pub name: String,
    pub path: PathBuf,
    /// The digest of its `SKILL.md` and the names and sizes beside it.
    pub digest: String,
    /// A folder that is the store's own skill seen through a link.
    pub into_store: bool,
    /// A copy of a skill this build ships, in a text this build or an
    /// earlier release wrote — the window's own, never a question.
    pub ours: bool,
    /// Another place reads this folder — a link elsewhere reads it, two
    /// places see it as one (`~/.codex/skills -> ~/.claude/skills`), or it
    /// stands in the person's own Claude home. It is copied into the store
    /// and followed, never moved or sent to the Trash.
    pub shared: bool,
    /// The checkout a linked folder reads into: the nearest folder holding
    /// `.git` at or above the link's target. The store's own copy has the
    /// checkout of the folder it was copied from.
    pub checkout: Option<PathBuf>,
}

/// One folder of skills an agent installs into by itself.
#[derive(Debug, Clone)]
pub struct SkillPlace {
    pub agent: String,
    /// The agent reads the store too, so its skill may be moved there.
    pub sees_store: bool,
    /// False for the person's own Claude.
    pub writable: bool,
    pub folders: Vec<SkillFolder>,
}

/// What one look found of skills.
#[derive(Debug, Clone, Default)]
pub struct SkillsView {
    pub store: BTreeMap<String, SkillFolder>,
    pub places: Vec<SkillPlace>,
}

/// One thing to do to the skill folders.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillStep {
    /// Move the folder into the store under its name.
    Move {
        agent: String,
        from: PathBuf,
        name: String,
    },
    /// Copy the folder into the store under its name, replacing the store's.
    Copy {
        agent: String,
        from: PathBuf,
        name: String,
    },
    /// The folder is the store's skill again, held by an agent that reads
    /// the store: to the Trash, so the agent does not list it twice.
    Trash {
        agent: String,
        path: PathBuf,
        name: String,
    },
}

/// The steps one look calls for, and the ledger they leave.
#[derive(Debug, Clone)]
pub struct SkillsPlan {
    pub steps: Vec<SkillStep>,
    pub ledger: Ledger,
    pub notes: Vec<Note>,
}

/// Plan one look over the skill folders.
#[must_use]
pub fn plan_skills(view: &SkillsView, ledger: &Ledger, now: i64) -> SkillsPlan {
    let mut next = ledger.clone();
    let mut steps = Vec::new();
    let mut notes = Vec::new();
    // What the store will hold once the steps run: name → digest.
    let mut holding: BTreeMap<String, String> = view
        .store
        .iter()
        .map(|(name, folder)| (name.clone(), folder.digest.clone()))
        .collect();
    // The checkout each text the store will hold came from, where one did.
    let mut checkouts: BTreeMap<&str, &Path> = view
        .store
        .iter()
        .filter_map(|(name, folder)| Some((name.as_str(), folder.checkout.as_deref()?)))
        .collect();
    // A skill the window remembered in the store and the store no longer
    // holds was removed there — by the person, by an agent, by `npx skills
    // remove` — and a copy left elsewhere does not bring it back. The runtime
    // keeps a record only for an import that succeeded, so a record without
    // its folder is a removal, never a failed import.
    let remembered: Vec<String> = next.skills.keys().cloned().collect();
    for name in remembered {
        if !view.store.contains_key(&name) {
            next.skill_removed(&name, now);
            notes.push(Note::Removed {
                name,
                at: "store".to_string(),
            });
        }
    }
    // What a removal left behind, taken stock of at the first look after it:
    // every folder still holding the name is a copy left there, not an
    // install. A folder that appears later is one.
    let unstocked: Vec<String> = next
        .gone_skills
        .keys()
        .filter(|name| !next.gone_skill_left.contains_key(*name))
        .cloned()
        .collect();
    for name in unstocked {
        let left = view
            .places
            .iter()
            .flat_map(|place| {
                place
                    .folders
                    .iter()
                    .filter(|folder| folder.name == name)
                    .map(|folder| SkillCandidate {
                        agent: place.agent.clone(),
                        path: folder.path.to_string_lossy().into_owned(),
                        digest: folder.digest.clone(),
                    })
            })
            .collect();
        next.gone_skill_left.insert(name, left);
    }
    let mut places: Vec<&SkillPlace> = view.places.iter().collect();
    // The person's own Claude last: an agent that may be moved from is
    // preferred as the origin of a skill both hold.
    places.sort_by_key(|place| (place.agent == MAC_CLAUDE, rank(&place.agent)));

    for place in places {
        for folder in &place.folders {
            let name = &folder.name;
            if !super::is_skill_name(name) || folder.into_store {
                continue;
            }
            if next.gone_skills.contains_key(name) {
                let source = folder.path.to_string_lossy();
                let left_behind = folder.ours
                    || next.gone_skill_left.get(name).is_some_and(|left| {
                        left.iter()
                            .any(|copy| copy.path == source && copy.digest == folder.digest)
                    });
                if left_behind {
                    continue;
                }
                // Installed again since the removal: the name is back.
                next.skill_back(name);
            }
            // A folder another place reads is copied and followed, never
            // moved or trashed: the run would only copy it, and a record of
            // a move would read the person's next edit there as a question
            // (the review of 94e1edaf7, P2-1).
            let removable = place.writable && place.sees_store && !folder.shared;
            match holding.get(name).cloned() {
                Some(digest) if digest == folder.digest => {
                    if removable {
                        steps.push(SkillStep::Trash {
                            agent: place.agent.clone(),
                            path: folder.path.clone(),
                            name: name.clone(),
                        });
                    }
                    if let Some(record) = next.skills.get_mut(name) {
                        let source = folder.path.to_string_lossy();
                        // A folder another place reads, holding the store's
                        // text, that the record does not name — v1.1.128
                        // recorded it as moved — is where the store's copy
                        // came from (the review of t-98860, P3-1).
                        // Not a skill this build ships: its copy in the
                        // person's own Claude is not where the store's came
                        // from (the second review of t-98860, P3-2).
                        if folder.shared && !folder.ours && record.copied_from.is_none() {
                            record.copied_from = Some(source.clone().into_owned());
                        }
                        if record.copied_from.as_deref() == Some(&*source) {
                            record.source_digest = Some(folder.digest.clone());
                        }
                    }
                }
                Some(digest) => {
                    if folder.ours {
                        // An earlier release of a skill this build ships: the
                        // store's copy is the one kept.
                        if removable {
                            steps.push(SkillStep::Trash {
                                agent: place.agent.clone(),
                                path: folder.path.clone(),
                                name: name.clone(),
                            });
                        }
                        continue;
                    }
                    let source = folder.path.to_string_lossy().into_owned();
                    let record = next.skills.entry(name.clone()).or_default();
                    if record.kept_over.contains(&folder.digest) {
                        continue;
                    }
                    // The folder the store's copy came from, unchanged since:
                    // the store moved on (the person edited it there), and
                    // the folder it left behind is no question.
                    if record.copied_from.as_deref() == Some(source.as_str())
                        && record.source_digest.as_deref() == Some(folder.digest.as_str())
                    {
                        continue;
                    }
                    // A copy leaves the store holding what the folder held,
                    // so the store is unchanged since the copy while it still
                    // holds the folder's digest of then. Only then may a
                    // changed folder be copied over it; a store the person
                    // edited meanwhile is asked about instead.
                    let followed = record.copied_from.as_deref() == Some(source.as_str())
                        && record.source_digest.as_deref() == Some(digest.as_str())
                        && record.source_digest.as_deref() != Some(folder.digest.as_str());
                    if followed {
                        steps.push(SkillStep::Copy {
                            agent: place.agent.clone(),
                            from: folder.path.clone(),
                            name: name.clone(),
                        });
                        record.digest.clone_from(&folder.digest);
                        record.source_digest = Some(folder.digest.clone());
                        holding.insert(name.clone(), folder.digest.clone());
                        take_checkout(&mut checkouts, folder);
                        notes.push(Note::Edited {
                            name: name.clone(),
                            at: place.agent.clone(),
                        });
                    } else if folder
                        .checkout
                        .as_deref()
                        .is_some_and(|checkout| checkouts.get(name.as_str()) == Some(&checkout))
                    {
                        // One checkout's own text for another agent — a
                        // package that ships a Claude Code text and a Codex
                        // text of one skill: each agent keeps the text it
                        // reads, and no one is asked which stays.
                        record.pick.retain(|known| known.path != source);
                    } else if !record.pick.iter().any(|known| known.path == source) {
                        let was_asked = !record.pick.is_empty();
                        record.pick.push(SkillCandidate {
                            agent: place.agent.clone(),
                            path: source,
                            digest: folder.digest.clone(),
                        });
                        if !was_asked {
                            notes.push(Note::Asked { name: name.clone() });
                        }
                    }
                }
                None => {
                    let source = folder.path.to_string_lossy().into_owned();
                    if removable {
                        steps.push(SkillStep::Move {
                            agent: place.agent.clone(),
                            from: folder.path.clone(),
                            name: name.clone(),
                        });
                    } else {
                        steps.push(SkillStep::Copy {
                            agent: place.agent.clone(),
                            from: folder.path.clone(),
                            name: name.clone(),
                        });
                    }
                    next.skills.insert(
                        name.clone(),
                        SkillRecord {
                            origin: place.agent.clone(),
                            at_ms: now,
                            digest: folder.digest.clone(),
                            copied_from: (!removable).then(|| source.clone()),
                            source_digest: (!removable).then(|| folder.digest.clone()),
                            pick: Vec::new(),
                            kept_over: Vec::new(),
                        },
                    );
                    holding.insert(name.clone(), folder.digest.clone());
                    take_checkout(&mut checkouts, folder);
                    notes.push(Note::TakenIn {
                        name: name.clone(),
                        from: place.agent.clone(),
                    });
                }
            }
        }
    }

    // The store's own skills are remembered, so one removed from it is not
    // taken back in from a copy left elsewhere.
    for (name, folder) in &view.store {
        let record = next
            .skills
            .entry(name.clone())
            .or_insert_with(|| SkillRecord {
                origin: "store".to_string(),
                at_ms: now,
                ..SkillRecord::default()
            });
        if !steps
            .iter()
            .any(|step| matches!(step, SkillStep::Copy { name: copied, .. } if copied == name))
        {
            record.digest.clone_from(&folder.digest);
        }
        // A question about a skill whose versions are all one again is gone.
        record
            .pick
            .retain(|candidate| candidate.digest != folder.digest);
    }
    SkillsPlan {
        steps,
        ledger: next,
        notes,
    }
}

/// The store takes its text from `folder`: that text came from the folder's
/// checkout, or from none.
fn take_checkout<'a>(checkouts: &mut BTreeMap<&'a str, &'a Path>, folder: &'a SkillFolder) {
    match folder.checkout.as_deref() {
        Some(checkout) => {
            checkouts.insert(&folder.name, checkout);
        }
        None => {
            checkouts.remove(folder.name.as_str());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stdio(command: &str, args: &[&str]) -> McpEntry {
        McpEntry {
            def: McpDef::Stdio {
                command: command.into(),
                args: args.iter().map(|a| (*a).to_string()).collect(),
                env: BTreeMap::new(),
            },
            disabled: false,
        }
    }

    fn list(agent: &str, servers: &[(&str, &McpEntry)]) -> AgentList {
        AgentList {
            agent: agent.into(),
            writable: agent != MAC_CLAUDE,
            servers: servers
                .iter()
                .map(|(name, entry)| ((*name).to_string(), (*entry).clone()))
                .collect(),
        }
    }

    fn view(store: &[(&str, &McpEntry)], lists: Vec<AgentList>) -> McpView {
        McpView {
            store: store
                .iter()
                .map(|(name, entry)| ((*name).to_string(), (*entry).clone()))
                .collect(),
            lists,
        }
    }

    /// The runtime's half: a successful write is recorded as seen.
    fn apply(plan: &McpPlan) -> Ledger {
        let mut ledger = plan.ledger.clone();
        for step in &plan.steps {
            match step {
                McpStep::Push { agent, name, entry } => {
                    ledger
                        .mcp
                        .entry(name.clone())
                        .or_default()
                        .seen
                        .insert(agent.clone(), entry.def.fingerprint());
                }
                McpStep::Pull { agent, name } => {
                    if let Some(record) = ledger.mcp.get_mut(name) {
                        record.seen.remove(agent);
                    }
                }
                _ => {}
            }
        }
        ledger
    }

    fn pushes(plan: &McpPlan) -> Vec<String> {
        plan.steps
            .iter()
            .filter_map(|step| match step {
                McpStep::Push { agent, name, .. } => Some(format!("{agent}:{name}")),
                _ => None,
            })
            .collect()
    }

    fn pulls(plan: &McpPlan) -> Vec<String> {
        plan.steps
            .iter()
            .filter_map(|step| match step {
                McpStep::Pull { agent, name } => Some(format!("{agent}:{name}")),
                _ => None,
            })
            .collect()
    }

    /// The person's own example: Codex adds a, Claude Code adds b — both go
    /// into the store and from there to every agent.
    #[test]
    fn a_server_one_agent_adds_reaches_every_agent() {
        let a = stdio("a-mcp", &[]);
        let b = stdio("b-mcp", &["--x"]);
        let looked = view(
            &[],
            vec![
                list("claude", &[("b", &b)]),
                list("codex", &[("a", &a)]),
                list("gemini", &[]),
            ],
        );
        let plan = plan_mcp(&looked, &Ledger::new(), 1);
        assert!(plan.steps.contains(&McpStep::Store {
            name: "a".into(),
            entry: a.clone()
        }));
        assert!(plan.steps.contains(&McpStep::Store {
            name: "b".into(),
            entry: b.clone()
        }));
        let mut pushed = pushes(&plan);
        pushed.sort();
        assert_eq!(pushed, vec!["claude:a", "codex:b", "gemini:a", "gemini:b"]);
        assert_eq!(plan.ledger.mcp["a"].origin, "codex");
        assert_eq!(plan.ledger.mcp["b"].origin, "claude");

        // The next look, with every write done, has nothing to do.
        let ledger = apply(&plan);
        let settled = view(
            &[("a", &a), ("b", &b)],
            vec![
                list("claude", &[("a", &a), ("b", &b)]),
                list("codex", &[("a", &a), ("b", &b)]),
                list("gemini", &[("a", &a), ("b", &b)]),
            ],
        );
        let again = plan_mcp(&settled, &ledger, 2);
        assert!(again.steps.is_empty(), "{:?}", again.steps);
        assert!(again.notes.is_empty());
    }

    #[test]
    fn two_versions_under_one_name_are_asked_and_nothing_is_written() {
        let local = stdio("context7-mcp", &[]);
        let remote = McpEntry {
            def: McpDef::Http {
                url: "https://mcp.context7.com/mcp".into(),
                headers: BTreeMap::new(),
            },
            disabled: false,
        };
        let looked = view(
            &[],
            vec![
                list("claude", &[("context7", &local)]),
                list("codex", &[("context7", &remote)]),
                list("zo", &[("context7", &local)]),
                list("gemini", &[]),
            ],
        );
        let plan = plan_mcp(&looked, &Ledger::new(), 1);
        assert!(plan.steps.is_empty(), "{:?}", plan.steps);
        let pick = &plan.ledger.mcp["context7"].pick;
        assert_eq!(pick.len(), 2);
        assert_eq!(pick[0].agents, vec!["claude", "zo"]);
        assert_eq!(pick[1].agents, vec!["codex"]);
        assert!(pick[1].remote);
        assert_eq!(
            plan.notes,
            vec![Note::Asked {
                name: "context7".into()
            }]
        );
        // Still waiting on the next look: no write, no second note.
        let again = plan_mcp(&looked, &plan.ledger, 2);
        assert!(again.steps.is_empty() && again.notes.is_empty());
        assert_eq!(again.ledger.mcp["context7"].pick.len(), 2);
    }

    /// The person chose: the runtime stores the choice and marks what every
    /// list held as seen, so the other version is replaced, not asked again.
    #[test]
    fn a_chosen_version_replaces_the_others() {
        let local = stdio("context7-mcp", &[]);
        let remote = McpEntry {
            def: McpDef::Http {
                url: "https://mcp.context7.com/mcp".into(),
                headers: BTreeMap::new(),
            },
            disabled: false,
        };
        let mut ledger = Ledger::new();
        let record = ledger.mcp.entry("context7".into()).or_default();
        record.seen.insert("claude".into(), local.def.fingerprint());
        record.seen.insert("codex".into(), remote.def.fingerprint());
        let looked = view(
            &[("context7", &local)],
            vec![
                list("claude", &[("context7", &local)]),
                list("codex", &[("context7", &remote)]),
            ],
        );
        let plan = plan_mcp(&looked, &ledger, 1);
        assert_eq!(pushes(&plan), vec!["codex:context7"]);
        assert!(plan.ledger.mcp["context7"].pick.is_empty());
    }

    #[test]
    fn a_server_bound_to_its_agent_stays_with_it() {
        let helper = stdio(
            "/Applications/ChatGPT.app/Contents/Resources/cua_node/bin/node_repl",
            &[],
        );
        let looked = view(
            &[],
            vec![
                list("codex", &[("node_repl", &helper)]),
                list("gemini", &[]),
            ],
        );
        let plan = plan_mcp(&looked, &Ledger::new(), 1);
        assert!(plan.steps.is_empty(), "{:?}", plan.steps);
        assert_eq!(plan.ledger.mcp["node_repl"].own.as_deref(), Some("codex"));
        // Shared by the person: into the store, and out to the others.
        let mut shared = plan.ledger.clone();
        shared.mcp.get_mut("node_repl").unwrap().shared = true;
        let looked = view(
            &[("node_repl", &helper)],
            vec![
                list("codex", &[("node_repl", &helper)]),
                list("gemini", &[]),
            ],
        );
        let plan = plan_mcp(&looked, &shared, 2);
        assert_eq!(pushes(&plan), vec!["gemini:node_repl"]);
    }

    /// The review of v1.1.128 (P2-1): 「가져옴 되돌리기」 keeps an ordinary
    /// server with the agent it came from. The next look does not take it in
    /// again, and the copies the window wrote into the other agents go; an
    /// agent that holds a version the window never wrote keeps it.
    #[test]
    fn a_server_kept_with_its_agent_is_not_taken_in_again() {
        let x = stdio("x-mcp", &[]);
        let theirs = stdio("x-mcp", &["--their-own"]);
        let mut ledger = Ledger::new();
        ledger.mcp.insert(
            "x".into(),
            McpRecord {
                origin: "codex".into(),
                kept_with: ["codex".to_string()].into(),
                seen: [
                    ("codex".to_string(), x.def.fingerprint()),
                    ("gemini".to_string(), x.def.fingerprint()),
                ]
                .into(),
                ..McpRecord::default()
            },
        );
        let looked = view(
            &[],
            vec![
                list("codex", &[("x", &x)]),
                list("gemini", &[("x", &x)]),
                list("kiro", &[("x", &theirs)]),
                list("cursor", &[]),
            ],
        );
        let plan = plan_mcp(&looked, &ledger, 2);
        assert_eq!(
            plan.steps,
            vec![McpStep::Pull {
                agent: "gemini".into(),
                name: "x".into()
            }]
        );
        assert!(plan.ledger.mcp["x"].kept_with.contains("codex"));
        assert!(plan.notes.is_empty(), "{:?}", plan.notes);
        let settled = view(
            &[],
            vec![
                list("codex", &[("x", &x)]),
                list("gemini", &[]),
                list("kiro", &[("x", &theirs)]),
                list("cursor", &[]),
            ],
        );
        let again = plan_mcp(&settled, &apply(&plan), 3);
        assert!(again.steps.is_empty(), "{:?}", again.steps);
        assert!(again.ledger.mcp["x"].kept_with.contains("codex"));
    }

    /// The review of v1.1.128 skills-p2 (P2② a): a server the person kept
    /// with Codex stays kept while Codex's list cannot be read, or reads as
    /// reset — neither is Codex letting it go — and is not taken in once the
    /// list reads again.
    #[test]
    fn a_kept_server_stays_kept_while_its_agents_list_is_unread_or_reset() {
        let x = stdio("x-mcp", &[]);
        let mut ledger = Ledger::new();
        ledger.mcp.insert(
            "x".into(),
            McpRecord {
                origin: "codex".into(),
                kept_with: ["codex".to_string()].into(),
                seen: [("codex".to_string(), x.def.fingerprint())].into(),
                ..McpRecord::default()
            },
        );
        let unread = view(&[], vec![list("gemini", &[])]);
        let reset = view(&[], vec![list("codex", &[]), list("gemini", &[])]);
        let back = view(&[], vec![list("codex", &[("x", &x)]), list("gemini", &[])]);
        for first in [unread, reset] {
            let plan = plan_mcp(&first, &ledger, 2);
            assert!(
                plan.ledger.mcp["x"].kept_with.contains("codex"),
                "{:?}",
                plan.ledger.mcp.get("x")
            );
            let again = plan_mcp(&back, &apply(&plan), 3);
            assert!(again.steps.is_empty(), "{:?}", again.steps);
            assert!(again.notes.is_empty(), "{:?}", again.notes);
        }
    }

    /// The review of v1.1.128 skills-p2 (P2② b): a server bound to Codex
    /// whose command becomes a whole path is an ordinary server now, and is
    /// taken in — its old binding is not read as the person keeping it.
    #[test]
    fn a_bound_server_that_is_bound_no_more_is_taken_in() {
        let bound = stdio("./bin/x-mcp", &[]);
        let plain = stdio("/usr/local/bin/x-mcp", &[]);
        let looked = view(
            &[],
            vec![list("codex", &[("x", &bound)]), list("gemini", &[])],
        );
        let plan = plan_mcp(&looked, &Ledger::new(), 1);
        assert_eq!(plan.ledger.mcp["x"].own.as_deref(), Some("codex"));
        let moved = view(
            &[],
            vec![list("codex", &[("x", &plain)]), list("gemini", &[])],
        );
        let again = plan_mcp(&moved, &apply(&plan), 2);
        assert!(
            again.steps.contains(&McpStep::Store {
                name: "x".into(),
                entry: plain.clone()
            }),
            "{:?}",
            again.steps
        );
    }

    /// The review of t-98860 (P2-1): v1.1.128 wrote 「가져옴 되돌리기」 as
    /// `own` on a server not bound to its agent, and its ledger has no
    /// `kept_with`. The first look of this build keeps that choice — nothing
    /// is taken into the store or written into another agent — while a
    /// server that is bound to its agent stays merely bound.
    #[test]
    fn a_server_kept_by_the_ledger_of_v1_1_128_stays_kept() {
        let x = stdio("x-mcp", &[]);
        let b = stdio("./bin/b-mcp", &[]);
        let old = serde_json::json!({
            "version": 1,
            "merged_at": 1,
            "mcp": {
                "x": {
                    "origin": "codex",
                    "at_ms": 1,
                    "own": "codex",
                    "seen": {
                        "codex": x.def.fingerprint(),
                        "gemini": x.def.fingerprint(),
                    },
                },
                "b": {
                    "origin": "codex",
                    "at_ms": 1,
                    "own": "codex",
                    "seen": { "codex": b.def.fingerprint() },
                },
            },
        })
        .to_string();
        let ledger = Ledger::read(&old);
        let looked = view(
            &[],
            vec![
                list("codex", &[("x", &x), ("b", &b)]),
                list("gemini", &[("x", &x)]),
                list("kiro", &[]),
                list("cursor", &[]),
            ],
        );
        let plan = plan_mcp(&looked, &ledger, 2);
        assert_eq!(
            plan.steps,
            vec![McpStep::Pull {
                agent: "gemini".into(),
                name: "x".into()
            }]
        );
        assert!(plan.notes.is_empty(), "{:?}", plan.notes);
        // Still the shape v1.1.128 reads: a version it did not know would read
        // as an empty ledger there and be written back (the second review of
        // t-98860, P2-1).
        assert_eq!(
            serde_json::to_value(&plan.ledger).expect("json")["version"],
            1
        );
        assert!(plan.ledger.kept_moved);
        let kept = &plan.ledger.mcp["x"];
        assert_eq!(kept.kept_with, ["codex".to_string()].into());
        assert_eq!(kept.own, None);
        let bound = &plan.ledger.mcp["b"];
        assert_eq!(bound.own.as_deref(), Some("codex"));
        assert!(bound.kept_with.is_empty());

        let settled = view(
            &[],
            vec![
                list("codex", &[("x", &x), ("b", &b)]),
                list("gemini", &[]),
                list("kiro", &[]),
                list("cursor", &[]),
            ],
        );
        let again = plan_mcp(&settled, &apply(&plan), 3);
        assert!(again.steps.is_empty(), "{:?}", again.steps);
        assert!(again.notes.is_empty(), "{:?}", again.notes);
    }

    /// The reviews of t-98860 (P3-1) and t-99089 (P3-G): a server v1.1.128
    /// wrote as `own` whose agent's list the first look could not read waits,
    /// `own` and all, for a look that reads it — then a kept one is kept and
    /// a bound one stays bound, and the list read later takes neither in.
    #[test]
    fn a_server_of_v1_1_128_whose_list_was_not_read_waits_for_it() {
        let x = stdio("x-mcp", &[]);
        let b = stdio("./bin/b-mcp", &[]);
        let old = serde_json::json!({
            "version": 1,
            "merged_at": 1,
            "mcp": {
                "x": {
                    "origin": "codex",
                    "at_ms": 1,
                    "own": "codex",
                    "seen": { "codex": x.def.fingerprint() },
                },
                "b": {
                    "origin": "codex",
                    "at_ms": 1,
                    "own": "codex",
                    "seen": { "codex": b.def.fingerprint() },
                },
            },
        })
        .to_string();
        let ledger = Ledger::read(&old);
        // Codex's list could not be read: the view leaves it out.
        let unread = view(&[], vec![list("gemini", &[]), list("kiro", &[])]);
        let plan = plan_mcp(&unread, &ledger, 2);
        assert!(plan.steps.is_empty(), "{:?}", plan.steps);
        assert_eq!(
            plan.ledger.kept_waiting,
            ["b".to_string(), "x".to_string()].into()
        );
        for name in ["x", "b"] {
            let record = &plan.ledger.mcp[name];
            assert_eq!(record.own.as_deref(), Some("codex"), "{name}");
            assert!(record.kept_with.is_empty(), "{name}");
        }
        let read = view(
            &[],
            vec![
                list("codex", &[("x", &x), ("b", &b)]),
                list("gemini", &[]),
                list("kiro", &[]),
            ],
        );
        let again = plan_mcp(&read, &apply(&plan), 3);
        assert!(again.steps.is_empty(), "{:?}", again.steps);
        assert!(again.ledger.kept_waiting.is_empty());
        assert_eq!(
            again.ledger.mcp["x"].kept_with,
            ["codex".to_string()].into()
        );
        let bound = &again.ledger.mcp["b"];
        assert_eq!(bound.own.as_deref(), Some("codex"));
        assert!(bound.kept_with.is_empty(), "a bound server became kept");
    }

    /// The review of t-99137 (P3-G2): a v1.1.128 server whose agent's list is
    /// never read again (the agent is gone) waits alone — the rest of the
    /// ledger is past the move, so a bound server that loses its binding later
    /// is taken into the store, not read as kept by the person.
    #[test]
    fn a_server_waiting_for_a_gone_agent_keeps_the_move_from_the_others() {
        let g = stdio("g-mcp", &[]);
        let bound = stdio("./bin/b-mcp", &[]);
        let loose = stdio("b-mcp", &[]);
        let old = serde_json::json!({
            "version": 1,
            "merged_at": 1,
            "mcp": {
                "g": {
                    "origin": "gemini",
                    "at_ms": 1,
                    "own": "gemini",
                    "seen": { "gemini": g.def.fingerprint() },
                },
                "b": {
                    "origin": "codex",
                    "at_ms": 1,
                    "own": "codex",
                    "seen": { "codex": bound.def.fingerprint() },
                },
            },
        })
        .to_string();
        let ledger = Ledger::read(&old);
        // Gemini is gone: its list is never in the view.
        let first = view(
            &[],
            vec![list("codex", &[("b", &bound)]), list("kiro", &[])],
        );
        let plan = plan_mcp(&first, &ledger, 2);
        assert!(plan.steps.is_empty(), "{:?}", plan.steps);
        assert!(
            plan.ledger.kept_moved,
            "one waiting server held the whole ledger"
        );
        assert_eq!(plan.ledger.mcp["g"].own.as_deref(), Some("gemini"));
        let changed = view(
            &[],
            vec![list("codex", &[("b", &loose)]), list("kiro", &[])],
        );
        let again = plan_mcp(&changed, &apply(&plan), 3);
        assert!(
            again
                .steps
                .iter()
                .any(|step| matches!(step, McpStep::Store { name, .. } if name == "b")),
            "{:?}",
            again.steps
        );
        assert!(again.ledger.mcp["b"].kept_with.is_empty());
        assert_eq!(again.ledger.mcp["g"].own.as_deref(), Some("gemini"));
    }

    #[test]
    fn an_agent_that_removes_a_server_removes_it_everywhere() {
        // Codex keeps b, which the window wrote too: its list was not reset,
        // so a missing from it is the person's removal.
        let a = stdio("a-mcp", &[]);
        let b = stdio("b-mcp", &["--x"]);
        let mut ledger = Ledger::new();
        for (name, entry) in [("a", &a), ("b", &b)] {
            let record = ledger.mcp.entry(name.into()).or_default();
            for agent in ["claude", "codex", "gemini"] {
                record.seen.insert(agent.into(), entry.def.fingerprint());
            }
        }
        let looked = view(
            &[("a", &a), ("b", &b)],
            vec![
                list("claude", &[("a", &a), ("b", &b)]),
                list("codex", &[("b", &b)]),
                list("gemini", &[("a", &a), ("b", &b)]),
            ],
        );
        let plan = plan_mcp(&looked, &ledger, 5);
        assert!(plan.steps.contains(&McpStep::Unstore { name: "a".into() }));
        let mut pulled = pulls(&plan);
        pulled.sort();
        assert_eq!(pulled, vec!["claude:a", "gemini:a"]);
        assert_eq!(plan.ledger.gone_mcp.get("a"), Some(&5));
        assert_eq!(
            plan.notes,
            vec![Note::Removed {
                name: "a".into(),
                at: "codex".into()
            }]
        );

        // A copy a failed pull left behind goes on the next look; nothing
        // comes back from it.
        let ledger = plan.ledger.clone();
        let looked = view(
            &[("b", &b)],
            vec![
                list("claude", &[("a", &a), ("b", &b)]),
                list("codex", &[("b", &b)]),
                list("gemini", &[("b", &b)]),
            ],
        );
        let plan = plan_mcp(&looked, &ledger, 6);
        assert_eq!(pulls(&plan), vec!["claude:a"]);
        assert!(
            !plan
                .steps
                .iter()
                .any(|s| matches!(s, McpStep::Store { .. }))
        );
    }

    /// The review of v1.1.128 (t-97098 P1): a list that lost every server the
    /// window had seen in it at once — its file deleted, emptied or rewritten by
    /// a reinstall — is a reset, not the person removing each server. Read as
    /// removals, one missing `~/.kiro/settings/mcp.json` took every server out of
    /// the store and out of every other agent, with nothing left to undo from.
    #[test]
    fn a_list_that_lost_every_server_the_window_saw_is_reset_not_removed() {
        let a = stdio("a-mcp", &[]);
        let b = stdio("b-mcp", &["--x"]);
        let mut ledger = Ledger::new();
        for (name, entry) in [("a", &a), ("b", &b)] {
            let record = ledger.mcp.entry(name.into()).or_default();
            for agent in ["claude", "codex", "gemini"] {
                record.seen.insert(agent.into(), entry.def.fingerprint());
            }
        }
        let looked = view(
            &[("a", &a), ("b", &b)],
            vec![
                list("claude", &[("a", &a), ("b", &b)]),
                list("codex", &[]),
                list("gemini", &[("a", &a), ("b", &b)]),
            ],
        );
        let plan = plan_mcp(&looked, &ledger, 5);
        assert!(
            !plan
                .steps
                .iter()
                .any(|step| matches!(step, McpStep::Unstore { .. })),
            "{:?}",
            plan.steps
        );
        assert!(pulls(&plan).is_empty(), "{:?}", plan.steps);
        let mut pushed = pushes(&plan);
        pushed.sort();
        assert_eq!(pushed, vec!["codex:a", "codex:b"]);
        assert!(plan.ledger.gone_mcp.is_empty());
        // The person who removed them on purpose sees why they came back
        // (the review of the v1.1.128 integration, P3).
        assert_eq!(plan.notes, vec![Note::Reset { at: "codex".into() }]);
    }

    /// The write back into a reset list can fail for one server (review of
    /// v1.1.128, t-97756 P2): the next look must try it again, not read the
    /// list that now holds the others as the person removing that one.
    #[test]
    fn a_reset_list_whose_write_back_half_failed_is_written_again_not_removed() {
        let a = stdio("a-mcp", &[]);
        let b = stdio("b-mcp", &["--x"]);
        let mut ledger = Ledger::new();
        for (name, entry) in [("a", &a), ("b", &b)] {
            let record = ledger.mcp.entry(name.into()).or_default();
            for agent in ["claude", "codex", "gemini"] {
                record.seen.insert(agent.into(), entry.def.fingerprint());
            }
        }
        let reset = view(
            &[("a", &a), ("b", &b)],
            vec![
                list("claude", &[("a", &a), ("b", &b)]),
                list("codex", &[]),
                list("gemini", &[("a", &a), ("b", &b)]),
            ],
        );
        let plan = plan_mcp(&reset, &ledger, 5);
        // Only a's write lands; b's fails and records nothing.
        let mut ledger = plan.ledger.clone();
        ledger
            .mcp
            .get_mut("a")
            .unwrap()
            .seen
            .insert("codex".into(), a.def.fingerprint());
        let half = view(
            &[("a", &a), ("b", &b)],
            vec![
                list("claude", &[("a", &a), ("b", &b)]),
                list("codex", &[("a", &a)]),
                list("gemini", &[("a", &a), ("b", &b)]),
            ],
        );
        let again = plan_mcp(&half, &ledger, 6);
        assert!(
            !again
                .steps
                .iter()
                .any(|step| matches!(step, McpStep::Unstore { .. })),
            "{:?}",
            again.steps
        );
        assert!(pulls(&again).is_empty(), "{:?}", again.steps);
        assert_eq!(pushes(&again), vec!["codex:b"]);
        assert!(again.ledger.gone_mcp.is_empty());
    }

    #[test]
    fn a_server_put_back_after_removal_is_taken_in_again() {
        let a = stdio("a-mcp", &[]);
        let fresh = stdio("a-mcp", &["--v2"]);
        let mut ledger = Ledger::new();
        ledger.gone_mcp.insert("a".into(), 1);
        let looked = view(
            &[],
            vec![list("codex", &[("a", &fresh)]), list("gemini", &[])],
        );
        let plan = plan_mcp(&looked, &ledger, 2);
        assert!(!plan.ledger.gone_mcp.contains_key("a"));
        assert!(plan.steps.contains(&McpStep::Store {
            name: "a".into(),
            entry: fresh.clone()
        }));
        assert_eq!(pushes(&plan), vec!["gemini:a"]);
        let _ = a;
    }

    #[test]
    fn an_agents_edit_becomes_every_agents() {
        let old = stdio("a-mcp", &[]);
        let new = stdio("a-mcp", &["--fast"]);
        let mut ledger = Ledger::new();
        let record = ledger.mcp.entry("a".into()).or_default();
        for agent in ["claude", "codex"] {
            record.seen.insert(agent.into(), old.def.fingerprint());
        }
        let looked = view(
            &[("a", &old)],
            vec![
                list("claude", &[("a", &old)]),
                list("codex", &[("a", &new)]),
            ],
        );
        let plan = plan_mcp(&looked, &ledger, 3);
        assert!(plan.steps.contains(&McpStep::Store {
            name: "a".into(),
            entry: new.clone()
        }));
        assert_eq!(pushes(&plan), vec!["claude:a"]);
        assert_eq!(
            plan.notes,
            vec![Note::Edited {
                name: "a".into(),
                at: "codex".into()
            }]
        );
    }

    #[test]
    fn two_agents_editing_one_server_two_ways_is_asked() {
        let old = stdio("a-mcp", &[]);
        let one = stdio("a-mcp", &["--1"]);
        let two = stdio("a-mcp", &["--2"]);
        let mut ledger = Ledger::new();
        let record = ledger.mcp.entry("a".into()).or_default();
        for agent in ["claude", "codex"] {
            record.seen.insert(agent.into(), old.def.fingerprint());
        }
        let looked = view(
            &[("a", &old)],
            vec![
                list("claude", &[("a", &one)]),
                list("codex", &[("a", &two)]),
            ],
        );
        let plan = plan_mcp(&looked, &ledger, 3);
        assert!(plan.steps.is_empty(), "{:?}", plan.steps);
        assert_eq!(plan.ledger.mcp["a"].pick.len(), 3);
    }

    #[test]
    fn a_switched_off_server_leaves_every_list_the_store_wrote() {
        let a = stdio("a-mcp", &[]);
        let off = McpEntry {
            disabled: true,
            ..a.clone()
        };
        let mut ledger = Ledger::new();
        let record = ledger.mcp.entry("a".into()).or_default();
        for agent in ["claude", "codex"] {
            record.seen.insert(agent.into(), a.def.fingerprint());
        }
        let looked = view(
            &[("a", &off)],
            vec![list("claude", &[("a", &a)]), list("codex", &[("a", &a)])],
        );
        let plan = plan_mcp(&looked, &ledger, 3);
        let mut pulled = pulls(&plan);
        pulled.sort();
        assert_eq!(pulled, vec!["claude:a", "codex:a"]);
        assert!(
            plan.ledger.gone_mcp.is_empty(),
            "switching off is not removing"
        );
        // On again: back into both.
        let ledger = apply(&plan);
        let looked = view(&[("a", &a)], vec![list("claude", &[]), list("codex", &[])]);
        let plan = plan_mcp(&looked, &ledger, 4);
        assert_eq!(pushes(&plan).len(), 2);
    }

    #[test]
    fn switched_off_for_one_agent_leaves_only_that_agent() {
        let a = stdio("a-mcp", &[]);
        let mut ledger = Ledger::new();
        let record = ledger.mcp.entry("a".into()).or_default();
        for agent in ["claude", "codex"] {
            record.seen.insert(agent.into(), a.def.fingerprint());
        }
        record.off.insert("codex".into());
        let looked = view(
            &[("a", &a)],
            vec![list("claude", &[("a", &a)]), list("codex", &[("a", &a)])],
        );
        let plan = plan_mcp(&looked, &ledger, 3);
        assert_eq!(pulls(&plan), vec!["codex:a"]);
        assert!(plan.ledger.gone_mcp.is_empty());
        // And the pull, done, is not read as Codex removing it.
        let ledger = apply(&plan);
        let looked = view(
            &[("a", &a)],
            vec![list("claude", &[("a", &a)]), list("codex", &[])],
        );
        assert!(plan_mcp(&looked, &ledger, 4).steps.is_empty());
    }

    #[test]
    fn the_persons_own_claude_is_read_and_never_written() {
        let a = stdio("a-mcp", &[]);
        let b = stdio("b-mcp", &[]);
        let looked = view(
            &[("b", &b)],
            vec![list(MAC_CLAUDE, &[("a", &a)]), list("codex", &[("b", &b)])],
        );
        let plan = plan_mcp(&looked, &Ledger::new(), 1);
        assert!(plan.steps.contains(&McpStep::Store {
            name: "a".into(),
            entry: a.clone()
        }));
        assert_eq!(pushes(&plan), vec!["codex:a"]);
        assert!(plan.steps.iter().all(|step| !matches!(step, McpStep::Push { agent, .. } | McpStep::Pull { agent, .. } if agent == MAC_CLAUDE)));
        // Gone from the person's own Claude: nothing is removed anywhere.
        let ledger = apply(&plan);
        let looked = view(
            &[("a", &a), ("b", &b)],
            vec![
                list(MAC_CLAUDE, &[]),
                list("codex", &[("a", &a), ("b", &b)]),
            ],
        );
        let plan = plan_mcp(&looked, &ledger, 2);
        assert!(plan.steps.is_empty(), "{:?}", plan.steps);
    }

    #[test]
    fn a_failed_write_is_tried_again_and_never_read_as_a_removal() {
        let a = stdio("a-mcp", &[]);
        let looked = view(
            &[("a", &a)],
            vec![list("claude", &[("a", &a)]), list("gemini", &[])],
        );
        let mut ledger = Ledger::new();
        ledger
            .mcp
            .entry("a".into())
            .or_default()
            .seen
            .insert("claude".into(), a.def.fingerprint());
        let plan = plan_mcp(&looked, &ledger, 1);
        assert_eq!(pushes(&plan), vec!["gemini:a"]);
        // The push failed: the runtime records nothing, and the next look
        // pushes again instead of removing `a` everywhere.
        let again = plan_mcp(&looked, &plan.ledger, 2);
        assert_eq!(pushes(&again), vec!["gemini:a"]);
        assert!(again.ledger.gone_mcp.is_empty());
    }

    #[test]
    fn a_version_the_window_never_saw_in_an_agent_is_asked_not_overwritten() {
        let ours = stdio("a-mcp", &[]);
        let theirs = stdio("a-mcp", &["--their-own"]);
        let looked = view(&[("a", &ours)], vec![list("codex", &[("a", &theirs)])]);
        let plan = plan_mcp(&looked, &Ledger::new(), 1);
        assert!(pushes(&plan).is_empty());
        assert_eq!(plan.ledger.mcp["a"].pick.len(), 2);
    }

    #[test]
    fn a_store_edit_reaches_every_list_that_held_the_old_version() {
        let old = stdio("a-mcp", &[]);
        let new = stdio("a-mcp", &["--new"]);
        let mut ledger = Ledger::new();
        let record = ledger.mcp.entry("a".into()).or_default();
        for agent in ["claude", "codex"] {
            record.seen.insert(agent.into(), old.def.fingerprint());
        }
        let looked = view(
            &[("a", &new)],
            vec![
                list("claude", &[("a", &old)]),
                list("codex", &[("a", &old)]),
            ],
        );
        let plan = plan_mcp(&looked, &ledger, 2);
        let mut pushed = pushes(&plan);
        pushed.sort();
        assert_eq!(pushed, vec!["claude:a", "codex:a"]);
    }

    fn folder(name: &str, digest: &str) -> SkillFolder {
        SkillFolder {
            name: name.into(),
            path: PathBuf::from(format!("/Users/dev/x/{name}")),
            digest: digest.into(),
            into_store: false,
            ours: false,
            shared: false,
            checkout: None,
        }
    }

    fn from_checkout(mut folder: SkillFolder, checkout: &str) -> SkillFolder {
        folder.checkout = Some(PathBuf::from(checkout));
        folder
    }

    fn place(agent: &str, sees_store: bool, folders: Vec<SkillFolder>) -> SkillPlace {
        SkillPlace {
            agent: agent.into(),
            sees_store,
            writable: agent != MAC_CLAUDE,
            folders: folders
                .into_iter()
                .map(|mut f| {
                    f.path = PathBuf::from(format!("/Users/dev/{agent}/{}", f.name));
                    f
                })
                .collect(),
        }
    }

    /// The person's example, for skills: Codex installs a, Claude Code (the
    /// person's own) installs b. a is moved — Codex reads the store; b is
    /// copied — the person's Claude home is never changed.
    #[test]
    fn a_skill_an_agent_installs_goes_into_the_store() {
        let looked = SkillsView {
            store: BTreeMap::new(),
            places: vec![
                place("codex", true, vec![folder("a", "da")]),
                place(MAC_CLAUDE, false, vec![folder("b", "db")]),
                place("kiro", false, vec![folder("k", "dk")]),
            ],
        };
        let plan = plan_skills(&looked, &Ledger::new(), 1);
        assert!(plan.steps.contains(&SkillStep::Move {
            agent: "codex".into(),
            from: "/Users/dev/codex/a".into(),
            name: "a".into()
        }));
        assert!(plan.steps.contains(&SkillStep::Copy {
            agent: MAC_CLAUDE.into(),
            from: "/Users/dev/claude-mac/b".into(),
            name: "b".into()
        }));
        assert!(plan.steps.contains(&SkillStep::Copy {
            agent: "kiro".into(),
            from: "/Users/dev/kiro/k".into(),
            name: "k".into()
        }));
        assert_eq!(plan.ledger.skills["a"].origin, "codex");
        assert_eq!(
            plan.ledger.skills["b"].copied_from.as_deref(),
            Some("/Users/dev/claude-mac/b")
        );
        // Next look: a is in the store and nowhere else; b's copy is the
        // store's; nothing to do.
        let settled = SkillsView {
            store: [("a", "da"), ("b", "db"), ("k", "dk")]
                .iter()
                .map(|(n, d)| ((*n).to_string(), folder(n, d)))
                .collect(),
            places: vec![
                place("codex", true, vec![]),
                place(MAC_CLAUDE, false, vec![folder("b", "db")]),
                place("kiro", false, vec![folder("k", "dk")]),
            ],
        };
        let again = plan_skills(&settled, &plan.ledger, 2);
        assert!(again.steps.is_empty(), "{:?}", again.steps);
    }

    #[test]
    fn the_same_skill_twice_is_one_store_skill() {
        let looked = SkillsView {
            store: [("a".to_string(), folder("a", "da"))].into_iter().collect(),
            places: vec![
                place("codex", true, vec![folder("a", "da")]),
                place("kiro", false, vec![folder("a", "da")]),
            ],
        };
        let plan = plan_skills(&looked, &Ledger::new(), 1);
        assert_eq!(
            plan.steps,
            vec![SkillStep::Trash {
                agent: "codex".into(),
                path: "/Users/dev/codex/a".into(),
                name: "a".into()
            }]
        );
    }

    #[test]
    fn another_text_under_a_store_skills_name_is_asked() {
        let looked = SkillsView {
            store: [("a".to_string(), folder("a", "da"))].into_iter().collect(),
            places: vec![place("codex", true, vec![folder("a", "OTHER")])],
        };
        let plan = plan_skills(&looked, &Ledger::new(), 1);
        assert!(plan.steps.is_empty());
        assert_eq!(plan.ledger.skills["a"].pick.len(), 1);
        assert_eq!(plan.notes, vec![Note::Asked { name: "a".into() }]);
        let again = plan_skills(&looked, &plan.ledger, 2);
        assert!(again.notes.is_empty(), "asked twice");
    }

    #[test]
    fn a_text_the_person_decided_against_is_not_asked_again() {
        let mut ledger = Ledger::new();
        ledger.skills.insert(
            "a".into(),
            SkillRecord {
                origin: "store".into(),
                digest: "da".into(),
                kept_over: vec!["OTHER".into()],
                ..SkillRecord::default()
            },
        );
        let looked = SkillsView {
            store: [("a".to_string(), folder("a", "da"))].into_iter().collect(),
            places: vec![place(MAC_CLAUDE, false, vec![folder("a", "OTHER")])],
        };
        let plan = plan_skills(&looked, &ledger, 1);
        assert!(
            plan.steps.is_empty() && plan.notes.is_empty(),
            "{:?} {:?}",
            plan.steps,
            plan.notes
        );
        assert!(plan.ledger.skills["a"].pick.is_empty());
    }

    const PACKAGE: &str = "/Users/dev/src/im-not-ai";

    /// The store's text and the person's Claude's, one package's two texts
    /// of a skill — a Claude Code one and a Codex one, linked from one
    /// checkout (im-not-ai's humanize-korean).
    fn one_checkouts_two_texts() -> SkillsView {
        SkillsView {
            store: [(
                "a".to_string(),
                from_checkout(folder("a", "codex"), PACKAGE),
            )]
            .into_iter()
            .collect(),
            places: vec![place(
                MAC_CLAUDE,
                false,
                vec![from_checkout(folder("a", "claude"), PACKAGE)],
            )],
        }
    }

    #[test]
    fn one_checkouts_texts_of_a_skill_are_not_asked_about() {
        let plan = plan_skills(&one_checkouts_two_texts(), &Ledger::new(), 1);
        assert!(
            plan.steps.is_empty()
                && plan.notes.is_empty()
                && plan.ledger.skills["a"].pick.is_empty(),
            "{:?} {:?} {:?}",
            plan.steps,
            plan.notes,
            plan.ledger.skills["a"]
        );
    }

    /// A ledger an earlier build wrote holds the question; the next look
    /// drops it.
    #[test]
    fn a_saved_question_about_one_checkouts_other_text_is_dropped() {
        let mut ledger = Ledger::new();
        ledger.skills.insert(
            "a".into(),
            SkillRecord {
                origin: "codex".into(),
                digest: "codex".into(),
                pick: vec![SkillCandidate {
                    agent: MAC_CLAUDE.into(),
                    path: "/Users/dev/claude-mac/a".into(),
                    digest: "claude".into(),
                }],
                ..SkillRecord::default()
            },
        );
        let plan = plan_skills(&one_checkouts_two_texts(), &ledger, 2);
        assert!(
            plan.ledger.skills["a"].pick.is_empty(),
            "{:?}",
            plan.ledger.skills["a"].pick
        );
    }

    #[test]
    fn a_text_from_another_checkout_or_from_none_is_still_asked() {
        let looked = SkillsView {
            store: [(
                "a".to_string(),
                from_checkout(folder("a", "codex"), PACKAGE),
            )]
            .into_iter()
            .collect(),
            places: vec![
                place(
                    "codex",
                    true,
                    vec![from_checkout(folder("a", "fork"), "/Users/dev/src/fork")],
                ),
                place(MAC_CLAUDE, false, vec![folder("a", "mine")]),
            ],
        };
        let plan = plan_skills(&looked, &Ledger::new(), 1);
        assert_eq!(
            plan.ledger.skills["a"].pick.len(),
            2,
            "{:?}",
            plan.ledger.skills["a"].pick
        );
    }

    /// The store takes a skill in this look from one agent's link: the other
    /// text from that checkout is no question in the same look either.
    #[test]
    fn a_skill_taken_in_brings_its_checkout_along() {
        let mut linked = from_checkout(folder("a", "codex"), PACKAGE);
        linked.shared = true;
        let looked = SkillsView {
            store: BTreeMap::new(),
            places: vec![
                place("codex", true, vec![linked]),
                place(
                    MAC_CLAUDE,
                    false,
                    vec![from_checkout(folder("a", "claude"), PACKAGE)],
                ),
            ],
        };
        let plan = plan_skills(&looked, &Ledger::new(), 1);
        assert_eq!(
            plan.notes,
            vec![Note::TakenIn {
                name: "a".into(),
                from: "codex".into()
            }]
        );
    }

    /// Reading one checkout spares a question only: a copy still follows the
    /// folder it came from.
    #[test]
    fn a_copy_still_follows_its_source_within_one_checkout() {
        let mut ledger = Ledger::new();
        ledger.skills.insert(
            "a".into(),
            SkillRecord {
                origin: MAC_CLAUDE.into(),
                digest: "v1".into(),
                copied_from: Some("/Users/dev/claude-mac/a".into()),
                source_digest: Some("v1".into()),
                ..SkillRecord::default()
            },
        );
        let looked = SkillsView {
            store: [("a".to_string(), from_checkout(folder("a", "v1"), PACKAGE))]
                .into_iter()
                .collect(),
            places: vec![place(
                MAC_CLAUDE,
                false,
                vec![from_checkout(folder("a", "v2"), PACKAGE)],
            )],
        };
        let plan = plan_skills(&looked, &ledger, 3);
        assert_eq!(plan.ledger.skills["a"].digest, "v2");
    }

    #[test]
    fn a_copied_skill_follows_its_source() {
        let mut ledger = Ledger::new();
        ledger.skills.insert(
            "b".into(),
            SkillRecord {
                origin: MAC_CLAUDE.into(),
                digest: "v1".into(),
                copied_from: Some("/Users/dev/claude-mac/b".into()),
                source_digest: Some("v1".into()),
                ..SkillRecord::default()
            },
        );
        let looked = SkillsView {
            store: [("b".to_string(), folder("b", "v1"))].into_iter().collect(),
            places: vec![place(MAC_CLAUDE, false, vec![folder("b", "v2")])],
        };
        let plan = plan_skills(&looked, &ledger, 3);
        assert_eq!(
            plan.steps,
            vec![SkillStep::Copy {
                agent: MAC_CLAUDE.into(),
                from: "/Users/dev/claude-mac/b".into(),
                name: "b".into()
            }]
        );
        assert_eq!(plan.ledger.skills["b"].digest, "v2");
    }

    /// The person edits a copied skill in the store: the folder it came from
    /// has not changed, so nothing is copied back and nothing is asked. When
    /// that folder changes too, both moved — that one is asked.
    #[test]
    fn a_store_edit_does_not_ask_about_the_unchanged_folder_it_came_from() {
        let mut ledger = Ledger::new();
        ledger.skills.insert(
            "b".into(),
            SkillRecord {
                origin: MAC_CLAUDE.into(),
                digest: "v1".into(),
                copied_from: Some("/Users/dev/claude-mac/b".into()),
                source_digest: Some("v1".into()),
                ..SkillRecord::default()
            },
        );
        let edited = SkillsView {
            store: [("b".to_string(), folder("b", "mine"))]
                .into_iter()
                .collect(),
            places: vec![place(MAC_CLAUDE, false, vec![folder("b", "v1")])],
        };
        let plan = plan_skills(&edited, &ledger, 3);
        assert!(plan.steps.is_empty(), "{:?}", plan.steps);
        assert!(plan.notes.is_empty(), "{:?}", plan.notes);
        assert!(plan.ledger.skills["b"].pick.is_empty());
        assert_eq!(plan.ledger.skills["b"].digest, "mine");

        let both = SkillsView {
            store: [("b".to_string(), folder("b", "mine"))]
                .into_iter()
                .collect(),
            places: vec![place(MAC_CLAUDE, false, vec![folder("b", "v2")])],
        };
        let asked = plan_skills(&both, &plan.ledger, 4);
        assert!(asked.steps.is_empty(), "{:?}", asked.steps);
        assert_eq!(asked.ledger.skills["b"].pick.len(), 1);
        assert_eq!(asked.notes, vec![Note::Asked { name: "b".into() }]);
    }

    #[test]
    fn a_skill_removed_from_the_store_is_not_taken_back_from_a_copy() {
        let mut ledger = Ledger::new();
        ledger.skills.insert(
            "b".into(),
            SkillRecord {
                origin: MAC_CLAUDE.into(),
                digest: "v1".into(),
                ..SkillRecord::default()
            },
        );
        let looked = SkillsView {
            store: BTreeMap::new(),
            places: vec![place(MAC_CLAUDE, false, vec![folder("b", "v1")])],
        };
        let plan = plan_skills(&looked, &ledger, 4);
        assert!(plan.steps.is_empty(), "{:?}", plan.steps);
        assert_eq!(plan.ledger.gone_skills.get("b"), Some(&4));
        let again = plan_skills(&looked, &plan.ledger, 5);
        assert!(again.steps.is_empty());
    }

    /// The review of v1.1.128 (P2-4): a removed skill's name is not barred
    /// for good. The copy left behind stays out; a folder that appears after
    /// the removal — an agent installing the skill again, the same text or
    /// another — brings the name back, and every agent sees it again.
    #[test]
    fn a_skill_installed_again_after_its_removal_comes_back() {
        let mut ledger = Ledger::new();
        ledger.skills.insert(
            "b".into(),
            SkillRecord {
                origin: "zo".into(),
                digest: "v1".into(),
                copied_from: Some("/Users/dev/zo/b".into()),
                source_digest: Some("v1".into()),
                ..SkillRecord::default()
            },
        );
        let removed = SkillsView {
            store: BTreeMap::new(),
            places: vec![
                place("codex", true, vec![]),
                place("zo", false, vec![folder("b", "v1")]),
            ],
        };
        let plan = plan_skills(&removed, &ledger, 4);
        assert!(plan.steps.is_empty(), "{:?}", plan.steps);
        assert!(plan.ledger.gone_skills.contains_key("b"));
        let left = plan_skills(&removed, &plan.ledger, 5);
        assert!(left.steps.is_empty(), "the copy left behind came back");

        let installed = SkillsView {
            store: BTreeMap::new(),
            places: vec![
                place("codex", true, vec![folder("b", "v1")]),
                place("zo", false, vec![folder("b", "v1")]),
            ],
        };
        let back = plan_skills(&installed, &left.ledger, 6);
        assert!(
            back.steps.contains(&SkillStep::Move {
                agent: "codex".into(),
                from: "/Users/dev/codex/b".into(),
                name: "b".into()
            }),
            "{:?}",
            back.steps
        );
        assert!(!back.ledger.gone_skills.contains_key("b"));
        assert!(back.notes.contains(&Note::TakenIn {
            name: "b".into(),
            from: "codex".into()
        }));
    }

    /// The review of 94e1edaf7 (t-98010, P2-1): a folder another place reads
    /// is copied, and the record says where from, so an edit there is
    /// followed — never asked about — and no Trash step is planned for it.
    #[test]
    fn a_folder_another_place_reads_is_copied_and_followed() {
        let mut read = folder("foo", "v1");
        read.shared = true;
        let looked = SkillsView {
            store: BTreeMap::new(),
            places: vec![place("codex", true, vec![read.clone()])],
        };
        let plan = plan_skills(&looked, &Ledger::new(), 1);
        assert_eq!(
            plan.steps,
            vec![SkillStep::Copy {
                agent: "codex".into(),
                from: "/Users/dev/codex/foo".into(),
                name: "foo".into()
            }]
        );
        assert_eq!(
            plan.ledger.skills["foo"].copied_from.as_deref(),
            Some("/Users/dev/codex/foo")
        );
        let held = SkillsView {
            store: [("foo".to_string(), folder("foo", "v1"))].into(),
            places: looked.places.clone(),
        };
        let again = plan_skills(&held, &plan.ledger, 2);
        assert!(again.steps.is_empty(), "{:?}", again.steps);
        let mut edited = read;
        edited.digest = "v2".into();
        let follow = plan_skills(
            &SkillsView {
                store: held.store.clone(),
                places: vec![place("codex", true, vec![edited])],
            },
            &again.ledger,
            3,
        );
        assert_eq!(
            follow.steps,
            vec![SkillStep::Copy {
                agent: "codex".into(),
                from: "/Users/dev/codex/foo".into(),
                name: "foo".into()
            }]
        );
        assert!(
            follow.ledger.skills["foo"].pick.is_empty(),
            "an edit was asked about"
        );
    }

    /// The review of t-98860 (P3-1): v1.1.128 recorded a folder another
    /// place reads as moved, without where it came from. Once a look finds
    /// that folder holding the store's text, the record says where from, so
    /// an edit there is followed, not asked about.
    #[test]
    fn a_record_of_v1_1_128_learns_the_folder_another_place_reads() {
        let mut read = folder("foo", "v1");
        read.shared = true;
        let mut ledger = Ledger::new();
        ledger.skills.insert(
            "foo".into(),
            SkillRecord {
                origin: "codex".into(),
                at_ms: 1,
                digest: "v1".into(),
                ..SkillRecord::default()
            },
        );
        let held = SkillsView {
            store: [("foo".to_string(), folder("foo", "v1"))].into(),
            places: vec![place("codex", true, vec![read.clone()])],
        };
        let plan = plan_skills(&held, &ledger, 2);
        assert!(plan.steps.is_empty(), "{:?}", plan.steps);
        let record = &plan.ledger.skills["foo"];
        assert_eq!(record.copied_from.as_deref(), Some("/Users/dev/codex/foo"));
        assert_eq!(record.source_digest.as_deref(), Some("v1"));
        let mut edited = read;
        edited.digest = "v2".into();
        let follow = plan_skills(
            &SkillsView {
                store: held.store.clone(),
                places: vec![place("codex", true, vec![edited])],
            },
            &plan.ledger,
            3,
        );
        assert_eq!(
            follow.steps,
            vec![SkillStep::Copy {
                agent: "codex".into(),
                from: "/Users/dev/codex/foo".into(),
                name: "foo".into()
            }]
        );
        assert!(
            follow.ledger.skills["foo"].pick.is_empty(),
            "an edit was asked about"
        );
    }

    /// The second review of t-98860 (P3-2): a skill this build ships, whose
    /// copy stands in a folder another place reads (the person's own Claude),
    /// does not take that copy as where the store's came from — an edit there
    /// is not carried into every agent unasked.
    #[test]
    fn a_shipped_skill_does_not_learn_a_shared_copy_as_its_source() {
        let mut copy = folder("design", "v1");
        copy.shared = true;
        copy.ours = true;
        let mut ledger = Ledger::new();
        ledger.skills.insert(
            "design".into(),
            SkillRecord {
                origin: "codex".into(),
                at_ms: 1,
                digest: "v1".into(),
                ..SkillRecord::default()
            },
        );
        let held = SkillsView {
            store: [("design".to_string(), folder("design", "v1"))].into(),
            places: vec![place("codex", true, vec![copy])],
        };
        let plan = plan_skills(&held, &ledger, 2);
        assert_eq!(plan.ledger.skills["design"].copied_from, None);
    }

    #[test]
    fn the_windows_own_shipped_skill_is_never_a_question() {
        let mut stale = folder("design", "OLD-RELEASE");
        stale.ours = true;
        let looked = SkillsView {
            store: [("design".to_string(), folder("design", "CURRENT"))]
                .into_iter()
                .collect(),
            places: vec![
                place("codex", true, vec![stale.clone()]),
                place("kiro", false, vec![stale]),
            ],
        };
        let plan = plan_skills(&looked, &Ledger::new(), 1);
        assert_eq!(
            plan.steps,
            vec![SkillStep::Trash {
                agent: "codex".into(),
                path: "/Users/dev/codex/design".into(),
                name: "design".into()
            }]
        );
        assert!(plan.ledger.skills["design"].pick.is_empty());
    }

    /// Gemini CLI reads the store by itself (2026-11-13: an old copy of
    /// `computer-use` in its own folder showed beside the store's). A copy
    /// of ours there — the whole folder, as the keeper reads it
    /// (`skill_install::ships_this_folder`) — goes to the Trash; anything
    /// else is the person's, and is asked about — never taken away unasked.
    #[test]
    fn a_shipped_skill_in_a_native_agents_folder_goes_if_ours_and_is_asked_if_edited() {
        let mut ours = folder("computer-use", "OLD-RELEASE");
        ours.ours = true;
        let edited = folder("computer-use", "PERSONS-EDIT");
        let store = || -> BTreeMap<String, SkillFolder> {
            [(
                "computer-use".to_string(),
                folder("computer-use", "CURRENT"),
            )]
            .into_iter()
            .collect()
        };
        let plan = plan_skills(
            &SkillsView {
                store: store(),
                places: vec![place("gemini", true, vec![ours])],
            },
            &Ledger::new(),
            1,
        );
        assert!(
            matches!(&plan.steps[..], [SkillStep::Trash { agent, .. }] if agent == "gemini"),
            "{:?}",
            plan.steps
        );
        assert!(plan.ledger.skills["computer-use"].pick.is_empty());
        let plan = plan_skills(
            &SkillsView {
                store: store(),
                places: vec![place("gemini", true, vec![edited])],
            },
            &Ledger::new(),
            1,
        );
        assert!(plan.steps.is_empty(), "{:?}", plan.steps);
        let pick = &plan.ledger.skills["computer-use"].pick;
        assert_eq!(pick.len(), 1);
        assert_eq!(pick[0].agent, "gemini");
    }

    #[test]
    fn hidden_folders_links_into_the_store_and_claude_account_skills_are_left() {
        let mut through = folder("a", "da");
        through.into_store = true;
        let looked = SkillsView {
            store: BTreeMap::new(),
            places: vec![place(
                "codex",
                true,
                vec![folder(".system", "x"), folder("synced", "y"), through],
            )],
        };
        let plan = plan_skills(&looked, &Ledger::new(), 1);
        assert!(plan.steps.is_empty(), "{:?}", plan.steps);
    }
}
