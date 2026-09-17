//! Admit only path changes supported by a hook and filesystem identity.
use crate::file_tree_io::Identity;
use std::path::{Path, PathBuf};

type ObservedMove = (PathBuf, PathBuf, Identity);
fn prepare_move(root: &Path, command: &str) -> Option<Vec<ObservedMove>> {
    // Reuse the hook bridge's quote-aware shell lexer. Expansion, compound
    // scripts and redirection do not identify a single proven path operation.
    if command.contains(['$', '`', '*', '?', '[', ']', '>', '<', '(', ')']) {
        return None;
    }
    let mut segments = zerocode_core::hook::shell_segments(command);
    if segments.len() != 1 {
        return None;
    }
    let mut words = segments
        .pop()?
        .into_iter()
        .map(|word| word.text)
        .collect::<Vec<_>>();
    match words.first()?.as_str() {
        "mv" | "/bin/mv" | "/usr/bin/mv" => {
            words.remove(0);
        }
        "git" if words.get(1).is_some_and(|word| word == "mv") => {
            words.drain(..2);
        }
        _ => return None,
    }
    if words.first().is_some_and(|word| word == "--") {
        words.remove(0);
    }
    if words.len() < 2 || words.iter().any(|word| word.starts_with('-')) {
        return None;
    }
    let target = root.join(words.pop()?);
    let directory = target.is_dir();
    if words.len() > 1 && !directory {
        return None;
    }
    words
        .into_iter()
        .map(|word| {
            let from = crate::file_tree_ops::checked_path(root, &root.join(word)).ok()?;
            let to = if directory {
                target.join(from.file_name()?)
            } else {
                target.clone()
            };
            let to = crate::file_tree_ops::checked_path(root, &to).ok()?;
            crate::file_tree_io::vacant(&to).ok()?;
            Some((from.clone(), to, Identity::read(&from).ok()?))
        })
        .collect()
}

use crate::*;
use zerocode_core::hook::{HookEnvelope, Phase};

struct Candidate {
    key: String,
    root: PathBuf,
    pairs: Vec<ObservedMove>,
    by: file_tree_ops::Origin,
    expires: Instant,
}
#[derive(Default)]
pub(crate) struct Pending {
    candidates: VecDeque<Candidate>,
}
impl Pending {
    fn ready(&mut self) -> Vec<Candidate> {
        let mut ready = Vec::new();
        let mut waiting = VecDeque::new();
        while let Some(candidate) = self.candidates.pop_front() {
            if Instant::now() >= candidate.expires {
                continue;
            }
            if candidate.pairs.iter().all(|(from, to, identity)| {
                file_tree_io::vacant(from).is_ok() && identity.check(to).is_ok()
            }) {
                ready.push(candidate);
            } else {
                waiting.push_back(candidate);
            }
        }
        self.candidates = waiting;
        ready
    }
}

fn candidate(
    envelope: &HookEnvelope,
    policy: &explorer_policy::ExplorerPolicy,
) -> Option<Candidate> {
    let payload: serde_json::Value = serde_json::from_str(&envelope.payload).ok()?;
    let root = PathBuf::from(&envelope.worktree_id).canonicalize().ok()?;
    if envelope.worktree_id.is_empty() {
        return None;
    }
    let cwd = payload
        .get("cwd")
        .and_then(serde_json::Value::as_str)
        .map(PathBuf::from)
        .unwrap_or_else(|| root.clone());
    let cwd = cwd.canonicalize().ok()?;
    if !file_tree_ops::inside_root(&root, &cwd) {
        return None;
    }
    let tool = payload
        .get("tool_name")
        .or_else(|| payload.get("tool"))?
        .as_str()?;
    let pairs = if ["Bash", "shell", "exec_command", "run_shell_command"].contains(&tool) {
        let command = zerocode_core::hook::worker_command(&envelope.payload)?;
        prepare_move(&cwd, &command)?
    } else if ["move_file", "rename_file", "mcp__filesystem__move_file"].contains(&tool) {
        let input = payload.get("tool_input").or_else(|| payload.get("input"))?;
        let from = input
            .get("source")
            .or_else(|| input.get("from"))?
            .as_str()?;
        let to = input
            .get("destination")
            .or_else(|| input.get("to"))?
            .as_str()?;
        let from = file_tree_ops::checked_path(&root, &cwd.join(from)).ok()?;
        let to = file_tree_ops::checked_path(&root, &cwd.join(to)).ok()?;
        file_tree_io::vacant(&to).ok()?;
        vec![(from.clone(), to, Identity::read(&from).ok()?)]
    } else {
        return None;
    };
    if pairs.is_empty() || pairs.len() > policy.move_cap {
        return None;
    }
    // A cwd inside a worktree is still owned by that worktree's stack.
    if pairs.iter().any(|(from, to, _)| {
        !file_tree_ops::inside_root(&root, from) || !file_tree_ops::inside_root(&root, to)
    }) {
        return None;
    }
    Some(Candidate {
        expires: Instant::now() + Duration::from_millis(policy.hook_pending_ms),
        key: call_key(envelope)?,
        root,
        pairs,
        by: file_tree_ops::Origin::Agent {
            agent: envelope.agent.slug().to_string(),
            pane: envelope.pane_key.clone(),
        },
    })
}

fn call_key(envelope: &HookEnvelope) -> Option<String> {
    Some(format!(
        "{}:{}:{}:{}",
        envelope.agent.slug(),
        envelope.pane_key,
        envelope.launch_token,
        zerocode_core::hook::worker_call_id(&envelope.payload)?
    ))
}

pub(crate) fn note_hook(app: &AppHandle, state: &AppState, envelope: &HookEnvelope, phase: Phase) {
    if phase == Phase::Failed {
        if let Some(key) = call_key(envelope) {
            state
                .shell_runtime()
                .tree_hooks
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .candidates
                .retain(|held| held.key != key);
        }
        return;
    }
    reconcile(app, state);
    if phase != Phase::Started {
        if let Some(key) = call_key(envelope) {
            state
                .shell_runtime()
                .tree_hooks
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .candidates
                .retain(|held| held.key != key);
        }
        return;
    }
    if !envelope.payload.contains("mv")
        && !envelope.payload.contains("move_file")
        && !envelope.payload.contains("rename_file")
    {
        return;
    }
    let Ok(policy) = explorer_runtime::policy(state) else {
        return;
    };
    let Some(candidate) = candidate(envelope, &policy) else {
        return;
    };
    let mut pending = state
        .shell_runtime()
        .tree_hooks
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if pending
        .candidates
        .iter()
        .any(|held| held.key == candidate.key)
    {
        return;
    }
    pending.candidates.push_back(candidate);
    while pending.candidates.len() > policy.hook_pending_cap {
        pending.candidates.pop_front();
    }
}

/// The existing worktree poll and the next hook reconcile start-only providers
/// too. No new poller or invoke: an empty pending ring does no filesystem work.
pub(crate) fn reconcile(app: &AppHandle, state: &AppState) {
    let ready = {
        let mut pending = state
            .shell_runtime()
            .tree_hooks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if pending.candidates.is_empty() {
            return;
        }
        pending.ready()
    };
    if ready.is_empty() {
        return;
    }
    let Ok(policy) = explorer_runtime::policy(state) else {
        return;
    };
    for candidate in ready {
        if let Ok(operation) = explorer_runtime::history(state).observed(
            &candidate.root,
            &candidate.pairs,
            candidate.by,
            &policy,
        ) {
            explorer_runtime::announce(app, &candidate.root, &operation);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tree_hook_simple_quoted_move_has_precise_paths_and_identity() {
        let yard = tempfile::tempdir().unwrap();
        let root = yard.path();
        std::fs::write(root.join("before name"), "content").unwrap();
        let pairs = prepare_move(root, "mv -- 'before name' 'after name'").expect("move evidence");
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].0.file_name().unwrap(), "before name");
        assert_eq!(pairs[0].1.file_name().unwrap(), "after name");
        assert_eq!(
            pairs[0].2,
            Identity::read(&root.join("before name")).unwrap()
        );
    }
    #[test]
    fn tree_hook_proven_agent_move_is_group_undoable_and_deduplicated() {
        let yard = tempfile::tempdir().unwrap();
        let root = yard.path();
        std::fs::write(root.join("a"), "mine").unwrap();
        let envelope: HookEnvelope = serde_json::from_value(json!({"agent":"claude", "pane_key":"term-8", "worktree_id":root, "payload":r#"{"tool_name":"Bash","tool_use_id":"move-1","tool_input":{"command":"mv a b"}}"#})).unwrap();
        let policy = explorer_policy::ExplorerPolicy::default();
        let mut pending = Pending::default();
        pending
            .candidates
            .push_back(candidate(&envelope, &policy).unwrap());
        assert!(pending.ready().is_empty());
        std::fs::rename(root.join("a"), root.join("b")).unwrap();
        let observed = pending.ready().pop().unwrap();
        assert!(pending.ready().is_empty());
        let mut history = file_tree_ops::History::default();
        let operation = history
            .observed(root, &observed.pairs, observed.by, &policy)
            .unwrap();
        assert!(
            matches!(operation.by, file_tree_ops::Origin::Agent { ref pane, .. } if pane == "term-8")
        );
        history.undo(root).unwrap();
        assert_eq!(std::fs::read_to_string(root.join("a")).unwrap(), "mine");
    }

    #[test]
    fn tree_hook_expired_and_replaced_candidates_cannot_claim_files() {
        let yard = tempfile::tempdir().unwrap();
        let root = yard.path();
        std::fs::write(root.join("a"), "mine").unwrap();
        let envelope: HookEnvelope = serde_json::from_value(json!({"agent":"claude", "pane_key":"term-8", "worktree_id":root, "payload":r#"{"tool_name":"Bash","tool_use_id":"move-1","tool_input":{"command":"mv a b"}}"#})).unwrap();
        let policy = explorer_policy::ExplorerPolicy::default();
        let mut held = candidate(&envelope, &policy).unwrap();
        held.expires = Instant::now();
        let mut pending = Pending::default();
        pending.candidates.push_back(held);
        std::fs::rename(root.join("a"), root.join("b")).unwrap();
        assert!(pending.ready().is_empty() && pending.candidates.is_empty());
    }

    #[test]
    fn tree_hook_ambiguous_shell_or_content_write_is_never_an_operation() {
        let yard = tempfile::tempdir().unwrap();
        for command in [
            "echo mv a b",
            "mv a b && false",
            "mv $SOURCE b",
            "mv *.rs dest",
            "git mv -f a b",
            "cp a b",
            "mv a b > receipt",
        ] {
            assert!(prepare_move(yard.path(), command).is_none(), "{command}");
        }
    }
}
