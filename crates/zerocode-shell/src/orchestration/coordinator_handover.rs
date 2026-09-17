//! Native human declaration and the existing beat's quota-witness adapter.
use super::*;
use zerocode_core::orchestration::coordinator_handover::{self as core, SeatHandoverStatus};

fn leader_seats() -> Vec<(String, zerocode_core::agent_teams::Team)> {
    crate::agent_teams::teams()
        .iter()
        .map(|(id, team)| (id.clone(), team.clone()))
        .collect()
}

fn eligible_panes(host: &dyn Host, ledger: &Ledger, run_id: &str) -> Vec<serde_json::Value> {
    let mut panes: Vec<_> = leader_seats().into_iter().filter_map(|(id, team)| {
        core::eligible(ledger, run_id, &team, &team.leader_pane).ok()?;
        let term = team.term_of(&team.leader_pane)?;
        if !host.pane_exists(term) { return None; }
        host.actor_for(term)?;
        let agent = host.agent_of(term)?;
        Some(serde_json::json!({"term":term,"seat":format!("{id}/{}",team.leader_pane),"agent":agent}))
    }).collect();
    panes.sort_by_key(|pane| pane["term"].as_u64());
    panes
}

pub(crate) fn status(host: &dyn Host, run_id: &str) -> Result<serde_json::Value, String> {
    if let Some(why) = unavailable() {
        return Err(why);
    }
    let held = runtime().ok_or("orchestration runtime unavailable")?;
    let image = held.actor.view().map_err(|error| format!("{error:?}"))?;
    let ledger = cached_ledger(&held, &image).map_err(|error| format!("{error:?}"))?;
    let run = ledger.run(run_id).ok_or("unknown run")?;
    let receipts: Vec<_> = run
        .messages()
        .iter()
        .rev()
        .filter(|message| message.kind == zerocode_core::orchestration::MessageKind::Handoff)
        .take(20)
        .map(|message| serde_json::json!({"id":message.id,"body":message.body.as_str()}))
        .collect();
    let source_agent = run.coordinator_live().and_then(|seat| {
        leader_seats()
            .into_iter()
            .find(|(id, team)| format!("{id}/{}", team.leader_pane) == seat.seat)
            .and_then(|(_, team)| host.agent_of(team.leader_term))
    });
    let quota_supported = source_agent
        .as_deref()
        .and_then(|agent| zerocode_core::orchestration::quota_gauge_for(agent, None))
        .is_some();
    let policy = run
        .coordinator
        .as_ref()
        .and_then(|seat| seat.handover.as_ref());
    Ok(serde_json::json!({
        "runId":run_id,"coordinator":run.coordinator.as_ref().map(|seat|seat.json()),
        "policy":policy.map(core::SeatHandoverPolicy::json),"quotaSupported":quota_supported,"eligiblePanes":eligible_panes(host,&ledger,run_id),"receipts":receipts,
    }))
}

/// Internal native door. Never exposed through the CLI bridge: that bridge
/// supplies a provider session, and cannot declare a human's standing order.
fn command(
    team: &str,
    pane: &str,
    capability: String,
    argv: Vec<String>,
    now_ms: i64,
) -> Result<serde_json::Value, String> {
    if let Some(why) = unavailable() {
        return Err(why);
    }
    let held = runtime().ok_or("orchestration runtime unavailable")?;
    let request = PlanCommand::checked(
        argv,
        team,
        pane,
        capability,
        Some(core::human_principal()),
        now_ms,
    )
    .map_err(|error| format!("{error:?}"))?;
    let (answer, _) = held
        .actor
        .plan(request)
        .map_err(|error| format!("{error:?}"))?;
    rang(answer.requires_durability);
    if answer.reply.exit_code != 0 {
        return Err(answer.reply.stderr);
    }
    serde_json::from_str(&answer.reply.stdout).map_err(|error| error.to_string())
}

struct HumanTarget {
    team: zerocode_core::agent_teams::Team,
    actor: String,
    capability: String,
}

fn human_target(host: &dyn Host, term: u32) -> Result<HumanTarget, String> {
    let (_, team) = leader_seats()
        .into_iter()
        .find(|(_, team)| team.term_of(&team.leader_pane) == Some(term))
        .ok_or("target is not an existing human leader pane")?;
    let capability = crate::agent_teams::current_pane_capability(&team.id, &team.leader_pane)
        .ok_or("target pane is gone")?;
    if !host.pane_exists(term) {
        return Err("target pane is gone".into());
    }
    let actor = host
        .actor_for(term)
        .ok_or("target has no session identity")?;
    host.agent_of(term).ok_or("target has no agent identity")?;
    Ok(HumanTarget {
        team,
        actor,
        capability,
    })
}

pub(crate) fn recent_runs(limit: Option<usize>) -> Result<serde_json::Value, String> {
    if let Some(why) = unavailable() {
        return Err(why);
    }
    let held = runtime().ok_or("orchestration runtime unavailable")?;
    let image = held.actor.view().map_err(|error| format!("{error:?}"))?;
    let ledger = cached_ledger(&held, &image).map_err(|error| format!("{error:?}"))?;
    Ok(core::recent_runs(&ledger, limit))
}

pub(crate) fn claim_seat(
    host: &dyn Host,
    run_id: &str,
    target_term: u32,
    generation: u32,
    request: &str,
    now_ms: i64,
) -> Result<serde_json::Value, String> {
    if request.trim().is_empty() {
        return Err("retryRequest is required".into());
    }
    let target = human_target(host, target_term)?;
    // The actor checks eligibility after retry replay: the original target
    // holding the seat now must not prevent the same request being replayed.
    let argv = vec![
        core::CLAIM_VERB.into(),
        "--run".into(),
        run_id.into(),
        "--target-term".into(),
        target_term.to_string(),
        "--generation".into(),
        generation.to_string(),
        "--target-actor".into(),
        target.actor,
        "--retry-request".into(),
        request.into(),
    ];
    command(
        &target.team.id,
        &target.team.leader_pane,
        target.capability,
        argv,
        now_ms,
    )
}

pub(crate) fn set_policy(
    host: &dyn Host,
    run_id: &str,
    generation: u32,
    target_term: Option<u32>,
    request: &str,
    now_ms: i64,
) -> Result<serde_json::Value, String> {
    if request.trim().is_empty() {
        return Err("retryRequest is required".into());
    }
    let held = runtime().ok_or("orchestration runtime unavailable")?;
    let image = held.actor.view().map_err(|error| format!("{error:?}"))?;
    let ledger = cached_ledger(&held, &image).map_err(|error| format!("{error:?}"))?;
    let seat = ledger
        .run(run_id)
        .and_then(|run| run.coordinator.as_ref())
        .ok_or("run has no coordinator")?;
    if let Some(policy) = seat
        .handover
        .as_ref()
        .filter(|policy| policy.request == request)
    {
        if target_term != Some(policy.target_term) || generation != policy.generation {
            return Err("retryRequest belongs to a different declaration".into());
        }
        drop(ledger);
        return status(host, run_id);
    }
    let source = leader_seats()
        .into_iter()
        .find(|(id, team)| format!("{id}/{}", team.leader_pane) == seat.seat);
    let mut argv = vec![
        core::POLICY_VERB.into(),
        "--run".into(),
        run_id.into(),
        "--generation".into(),
        generation.to_string(),
        "--retry-request".into(),
        request.into(),
    ];
    let (team, pane, capability) = if let Some(term) = target_term {
        let HumanTarget {
            team: target,
            actor: target_actor,
            capability,
        } = human_target(host, term)?;
        core::eligible(&ledger, run_id, &target, &target.leader_pane)?;
        let (_, source) = source.ok_or("source pane is gone")?;
        if !host.pane_exists(source.leader_term) {
            return Err("source pane is gone".into());
        }
        let source_agent = host
            .agent_of(source.leader_term)
            .ok_or("source agent is unknown")?;
        if host.actor_for(source.leader_term) != seat.actor {
            return Err("source session no longer matches its coordinator seat".into());
        }
        if zerocode_core::orchestration::quota_gauge_for(&source_agent, None).is_none() {
            return Err(
                "source has no verified provider gauge; quota handover cannot be armed".into(),
            );
        }
        // Model-dependent gauges without a measured model remain unknown.
        // Never borrow a different provider's usage for such a pane.
        argv.extend([
            "--target-term".into(),
            term.to_string(),
            "--target-actor".into(),
            target_actor,
            "--source-term".into(),
            source.leader_term.to_string(),
            "--agent".into(),
            source_agent,
        ]);
        (target.id, target.leader_pane, capability)
    } else {
        argv.push("--off".into());
        // A departed source can remain in the team table. Find a live, proven
        // declaration channel instead of selecting an arbitrary stale team.
        let HumanTarget {
            team, capability, ..
        } = source
            .and_then(|(_, team)| human_target(host, team.leader_term).ok())
            .or_else(|| {
                leader_seats()
                    .into_iter()
                    .find_map(|(_, team)| human_target(host, team.leader_term).ok())
            })
            .ok_or("no live human pane for the declaration")?;
        (team.id, team.leader_pane, capability)
    };
    drop(ledger);
    command(&team, &pane, capability, argv, now_ms)?;
    status(host, run_id)
}

/// One atomic ledger transfer per beat. There are no external effects to
/// resume: the existing transaction persists seat, bindings and receipts.
pub(super) fn walk(host: &dyn Host, now_ms: i64) -> bool {
    let Some(held) = runtime() else {
        return false;
    };
    let Ok(image) = held.actor.view() else {
        return false;
    };
    let Ok(ledger) = cached_ledger(&held, &image) else {
        return false;
    };
    let candidates: Vec<_> = ledger
        .runs()
        .iter()
        .filter_map(|run| {
            let seat = run.coordinator_live()?;
            let policy = seat
                .handover
                .as_ref()
                .filter(|policy| policy.status == SeatHandoverStatus::Armed)?;
            Some((run.id.clone(), seat.clone(), policy.clone()))
        })
        .collect();
    drop(ledger);
    for (run, seat, policy) in candidates {
        let Some((source_team, source_pane)) = seat.seat.split_once('/') else {
            continue;
        };
        let Some((target_team, target_pane)) = policy.target_seat.split_once('/') else {
            continue;
        };
        let Some(capability) =
            crate::agent_teams::current_pane_capability(target_team, target_pane)
        else {
            continue;
        };
        let live = {
            let teams = crate::agent_teams::teams();
            teams
                .get(source_team)
                .and_then(|team| team.term_of(source_pane))
                == Some(policy.source_term)
                && teams
                    .get(target_team)
                    .and_then(|team| team.term_of(target_pane))
                    == Some(policy.target_term)
        };
        if !live
            || !host.pane_exists(policy.source_term)
            || !host.pane_exists(policy.target_term)
            || host.actor_for(policy.target_term).as_deref() != Some(policy.target_actor.as_str())
            || host.agent_of(policy.source_term).as_deref() != Some(policy.source_agent.as_str())
            || host.actor_for(policy.source_term) != seat.actor
        {
            continue;
        }
        let marker = host.quota_wall_marker(policy.source_term, &policy.source_agent);
        let headroom = usage_headroom(
            &held.usage,
            &policy.source_agent,
            policy.source_model.as_deref(),
        );
        let Some(witness) = zerocode_core::orchestration::quota_wall_witness(
            &seat.seat,
            marker,
            headroom.as_ref(),
            now_ms,
        ) else {
            continue;
        };
        let request = format!(
            "seat-handover-{}",
            digest(
                b"zerocode.coordinator-handover.v1",
                &[
                    run.as_bytes(),
                    policy.request.as_bytes(),
                    &policy.generation.to_le_bytes(),
                ]
            )
        );
        let argv = vec![
            core::APPLY_VERB.into(),
            "--run".into(),
            run,
            "--generation".into(),
            seat.generation.to_string(),
            "--target-term".into(),
            policy.target_term.to_string(),
            "--target-actor".into(),
            policy.target_actor.clone(),
            "--declaration".into(),
            policy.request.clone(),
            "--marker".into(),
            witness.marker.line.as_str().into(),
            "--marker-source".into(),
            witness.marker.source,
            "--retry-request".into(),
            request,
        ];
        if command(target_team, target_pane, capability, argv, now_ms).is_ok() {
            return true;
        }
    }
    false
}
