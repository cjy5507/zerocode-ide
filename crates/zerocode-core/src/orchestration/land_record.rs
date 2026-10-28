//! The landing of a stack written on every member in one call (t-78890, O8; VERDICT F1).
//!
//! On 10-28 no task that reached main that day carried `merged` or `deployed` in the ledger: the
//! coordinator landed stacks by hand, and `land-batch --record-merged` is off until a repository
//! turns the integration queue on. Writing the landing one `task-update` at a time — each naming
//! its own attempt and source — was the step that was skipped. `task-update --members t-1,t-2
//! --result '{"merged":true,"mergeHead":"<sha>"}'` writes it once for the whole stack, and later
//! `{"deployed":true,"release":"<tag>"}` the same way.
//!
//! The ledger writes only what it heard. Each member's record is bound to the attempt and the
//! source its standing verified review already named — the review written one task at a time —
//! and goes through the same seat check every review does (`correction_author`). So this writes
//! a landing on reviewed work and nothing else: no verification, no status, no attempt named by
//! hand. One member that cannot take it refuses the call, one line each, and nothing is written.

use super::*;

/// The flag that names the stack's tasks — the word `land-batch` names its members with.
/// (`--tasks` is `reset`'s standalone switch, so a list after it would be read as a stray word.)
pub const MEMBERS_FLAG: &str = "--members";

/// The key a release tag is written under beside `deployed`: `v1.1.101`.
pub const RELEASE_KEY: &str = "release";

/// The `task-update` flags that speak of one task, refused beside [`MEMBERS_FLAG`].
const ONE_TASK_FLAGS: &[&str] = &[
    "--task",
    "--status",
    "--attempt",
    "--source",
    "--closed-as",
    "--into",
    "--to",
    "--why",
];

/// The keys a landing record may carry: the merge's, the shipping's and the release's.
fn landing_key(key: &str) -> bool {
    let [_, merged_keys, _] = REVIEW_KEY_GROUPS;
    merged_keys.contains(&key) || key == DEPLOYED_KEY || key == RELEASE_KEY
}

/// The landing a `--result` says, as the keys to add — or why it is not one.
fn landing_of(result: &str) -> Result<serde_json::Map<String, serde_json::Value>, String> {
    let Ok(serde_json::Value::Object(map)) = serde_json::from_str::<serde_json::Value>(result)
    else {
        return Err(format!(
            "{MEMBERS_FLAG} takes a --result that is a JSON object of landing keys, like \
             {{\"merged\":true,\"mergeHead\":\"<sha>\"}} or {{\"{DEPLOYED_KEY}\":true,\
             \"{RELEASE_KEY}\":\"<tag>\"}}"
        ));
    };
    if let Some(key) = map.keys().find(|key| !landing_key(key)) {
        return Err(format!(
            "{MEMBERS_FLAG} writes a landing and nothing else, and {key:?} is not a landing key \
             — a review is of one hand-in, written with `task-update --task <id> --attempt \
             <dispatch> --source <commit|report>`"
        ));
    }
    let facts = ReviewFacts::from_result(result);
    let [_, merged_keys, _] = REVIEW_KEY_GROUPS;
    let speaks_of_merge = merged_keys.iter().any(|key| map.contains_key(*key));
    let release = map.get(RELEASE_KEY);
    if (speaks_of_merge && !facts.merged)
        || (map.contains_key(DEPLOYED_KEY) && !facts.deployed)
        || release.is_some_and(|tag| tag.as_str().is_none_or(|tag| tag.trim().is_empty()))
        || (release.is_some() && !facts.deployed)
        || !(facts.merged || facts.deployed)
    {
        return Err(format!(
            "{MEMBERS_FLAG} records what landed: `merged` (true or a commit) with `mergeHead`, \
             or `{DEPLOYED_KEY}: true` with `{RELEASE_KEY}` naming the tag — a key that says \
             NOT, or a release nothing deployed, is written one task at a time"
        ));
    }
    Ok(map)
}

/// One member as its standing review names it: the attempt, the source and the result the
/// landing is added to — or why it cannot take one.
fn member(
    run: &Run,
    id: &str,
    landing: &serde_json::Map<String, serde_json::Value>,
) -> Result<(Option<String>, String, String), String> {
    let task = run.task(id).ok_or("unknown task")?;
    if let Some(newest) = run.newest_attempt(id)
        && newest.source.is_none()
    {
        let state = match newest.is_open() {
            true => "is still open and",
            false => "ended and",
        };
        return Err(format!(
            "attempt {} {state} has handed nothing in, so no review can stand on it",
            newest.id
        ));
    }
    let review = run.review_of(task);
    if !matches!(review.author, ReviewAuthor::Coordinator) || !review.verified {
        let moved = match (&review.superseded_by, &review.source_now) {
            (Some(attempt), _) => format!(" (the review was of an attempt before {attempt})"),
            (None, Some(source)) => format!(" (the review was of a source before {source})"),
            (None, None) => String::new(),
        };
        return Err(format!(
            "no verified review stands on its hand-in{moved} — review it first with \
             `task-update --task {id} --attempt <dispatch> --source <commit|report> --result \
             '{{\"verified\":true}}'`"
        ));
    }
    let [_, merged_keys, _] = REVIEW_KEY_GROUPS;
    if review.nothing_to_land
        && landing
            .keys()
            .any(|key| merged_keys.contains(&key.as_str()))
    {
        return Err("its review says there is nothing to land".to_string());
    }
    let source = review.source.clone().ok_or("its review names no source")?;
    let mut result = match serde_json::from_str::<serde_json::Value>(task.result.as_str()) {
        Ok(serde_json::Value::Object(map)) => map,
        _ => return Err("its review is not a JSON object to add the landing to".to_string()),
    };
    result.extend(landing.clone());
    Ok((
        review.attempt.clone(),
        source,
        serde_json::Value::Object(result).to_string(),
    ))
}

/// `task-update --members <t-1,t-2,…> --result <landing>`: the landing written on every member,
/// each bound to its own review — or refused whole, one line for each member that cannot take it.
pub(super) fn record(
    ledger: &mut Ledger,
    team: &Team,
    pane: &str,
    words: &Words,
    run_id: &str,
    now_ms: i64,
) -> Result<Decided, String> {
    if let Some(flag) = ONE_TASK_FLAGS
        .iter()
        .find(|flag| words.value(flag).is_some())
    {
        return Err(format!(
            "task-update {MEMBERS_FLAG} names its tasks, and each member's attempt and source \
             come from its own review — {flag} speaks of one task and is refused beside it"
        ));
    }
    let landing = landing_of(
        words
            .value("--result")
            .ok_or(format!("task-update {MEMBERS_FLAG} needs --result"))?,
    )?;
    let mut members: Vec<String> = Vec::new();
    for id in words.list(MEMBERS_FLAG) {
        if !members.contains(&id) {
            members.push(id);
        }
    }
    if members.is_empty() {
        return Err(format!(
            "{MEMBERS_FLAG} names the tasks, like {MEMBERS_FLAG} t-1,t-2"
        ));
    }
    let run = ledger.run(run_id).ok_or_else(|| unknown_run(run_id))?;
    let mut bound = Vec::new();
    let mut refused = Vec::new();
    for id in &members {
        match member(run, id, &landing) {
            Ok(held) => bound.push((id.clone(), held)),
            Err(why) => refused.push(format!("{id}: {why}")),
        }
    }
    if !refused.is_empty() {
        return Err(format!(
            "task-update {MEMBERS_FLAG} writes the whole stack or nothing, and {} of {} \
             members cannot take this record — nothing was written:\n{}",
            refused.len(),
            members.len(),
            refused.join("\n")
        ));
    }
    // Every author is judged before anything is written, the way `land-batch --record-merged`
    // judges its members, so a refusal leaves every task as it was.
    let mut writing = Vec::new();
    for (id, (attempt, source, result)) in bound {
        let author = correction_author(
            ledger,
            &team.leader_pane,
            run_id,
            &id,
            (&team.id, pane),
            Observed {
                attempt: Some(attempt.as_deref().unwrap_or(NO_ATTEMPT)),
                source: Some(&source),
                review: true,
                tested_head: None,
            },
        )?;
        writing.push((id, attempt, source, result, author));
    }
    let mut recorded = Vec::new();
    for (id, attempt, source, result, author) in writing {
        ledger.update_task(run_id, &id, None, Some(result), author, now_ms)?;
        recorded.push(serde_json::json!({
            "taskId": id,
            "attempt": attempt,
            "source": source,
        }));
    }
    Ok(said(serde_json::json!({
        "recorded": recorded,
        "landing": landing,
    })))
}
