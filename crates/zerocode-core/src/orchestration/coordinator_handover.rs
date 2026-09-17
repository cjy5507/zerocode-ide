//! Human-declared, one-shot transfer of an existing coordinator seat.
use super::*;

pub const POLICY_VERB: &str = "coordinator-handover-policy";
pub const APPLY_VERB: &str = "coordinator-handover-apply";
pub const CLAIM_VERB: &str = "coordinator-seat-claim";
pub const RECENT_RUNS_DEFAULT: usize = 50;
/// Only the native UI and its standing-order beat supply this principal.
/// The CLI bridge rejects these verbs before planning and derives session actors.
pub fn human_principal() -> String {
    named_actor(digest_of("zerocode.orchestration.human-seat-order.v1", &[]))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SeatHandoverStatus {
    Armed,
    Completed,
    Interrupted,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SeatHandoverPolicy {
    pub target_seat: String,
    pub target_term: u32,
    pub target_actor: String,
    pub source_term: u32,
    pub source_agent: String,
    pub source_model: Option<String>,
    pub generation: u32,
    pub armed_ms: i64,
    pub request: String,
    pub status: SeatHandoverStatus,
}

impl SeatHandoverPolicy {
    /// Public status omits the provider identity used to pin the target.
    pub fn json(&self) -> serde_json::Value {
        serde_json::json!({"targetSeat":self.target_seat,"targetTerm":self.target_term,
            "sourceTerm":self.source_term,"sourceAgent":self.source_agent,"generation":self.generation,
            "armedMs":self.armed_ms,"request":self.request,"status":self.status})
    }
}

/// The same ledger eligibility rule is used for UI listing and execution.
/// Host liveness/session facts are checked separately at the native boundary.
pub fn eligible(ledger: &Ledger, run_id: &str, team: &Team, pane: &str) -> Result<(), String> {
    eligible_to_claim(ledger, run_id, team, pane)?;
    if ledger
        .run(run_id)
        .and_then(Run::coordinator_live)
        .is_some_and(|held| held.seat == caller_of(team, pane))
    {
        return Err("target already holds a coordinator seat".into());
    }
    Ok(())
}

/// Manual entry may name its own current seat; every other eligibility rule
/// is shared with a quota handover, including the whole ledger's worker history.
fn eligible_to_claim(ledger: &Ledger, run_id: &str, team: &Team, pane: &str) -> Result<(), String> {
    if pane != team.leader_pane || team.term_of(pane).is_none() {
        return Err("target must be an existing human leader pane".into());
    }
    if ledger.seat_ever_held_a_worker((&team.id, pane)) {
        return Err("a pane with worker history cannot inherit the coordinator seat".into());
    }
    let seat = caller_of(team, pane);
    if ledger
        .runs()
        .iter()
        .any(|run| run.id != run_id && run.coordinator_live().is_some_and(|held| held.seat == seat))
    {
        return Err("target already holds a coordinator seat".into());
    }
    if ledger.run(run_id).is_none() {
        return Err(unknown_run(run_id));
    }
    Ok(())
}

/// Recent work a person can choose to coordinate. Unfinished work uses the
/// retention rule; a held chair or an unused, uncompacted run is also selectable.
pub fn recent_runs(ledger: &Ledger, requested_limit: Option<usize>) -> serde_json::Value {
    let limit = requested_limit
        .unwrap_or(RECENT_RUNS_DEFAULT)
        .clamp(1, MAX_LIST);
    let mut recent: Vec<_> = ledger
        .runs()
        .iter()
        .filter(|run| {
            run.coordinator_live().is_some()
                || !compactable(run, i64::MAX)
                || (run.summary.is_none() && holds_no_detail(run))
        })
        .map(|run| (run, run_last_activity(run)))
        .collect();
    recent.sort_by(|(left, left_at), (right, right_at)| {
        right_at
            .cmp(left_at)
            .then_with(|| right.created_ms.cmp(&left.created_ms))
            .then_with(|| right.id.cmp(&left.id))
    });
    let runs: Vec<_> = recent
        .iter()
        .take(limit)
        .map(|(run, at)| {
            serde_json::json!({
                "runId":run.id,"name":run.name,"createdMs":run.created_ms,"lastActivityMs":at,
                "generation":run.coordinator.as_ref().map_or(0,|held|held.generation),
                "coordinator":run.coordinator.as_ref().map(CoordinatorSeat::json),
            })
        })
        .collect();
    serde_json::json!({"runs":runs,"limit":limit,"truncated":recent.len()>limit})
}

fn target_identity<'a>(
    team: &Team,
    pane: &str,
    words: &'a Words,
) -> Result<(u32, &'a str), String> {
    let term: u32 = words
        .value("--target-term")
        .ok_or("missing target incarnation")?
        .parse()
        .map_err(|_| "invalid target terminal")?;
    if team.term_of(pane) != Some(term) {
        return Err("target incarnation changed".into());
    }
    let actor = words
        .value("--target-actor")
        .filter(|value| !value.is_empty())
        .ok_or("target has no session identity")?;
    Ok((term, actor))
}

fn claim(
    ledger: &mut Ledger,
    team: &Team,
    run_id: &str,
    words: &Words,
    pane: &str,
    now_ms: i64,
) -> Result<Decided, String> {
    eligible_to_claim(ledger, run_id, team, pane)?;
    let (_, actor) = target_identity(team, pane, words)?;
    let target = caller_of(team, pane);
    let held = ledger.run(run_id).and_then(Run::coordinator_live).cloned();
    let already_here = held.as_ref().is_some_and(|held| held.seat == target);
    if !already_here {
        ledger
            .run(run_id)
            .and_then(|run| run.coordinator.as_ref())
            .map_or(0, |held| held.generation)
            .checked_add(1)
            .ok_or("coordinator generation exhausted")?;
    }
    let seated = match held {
        Some(held) if held.seat != target => ledger.take_over_coordinator(
            run_id,
            &target,
            Some(actor),
            &held.seat,
            "human manual seat claim",
            now_ms,
        )?,
        _ => ledger.seat_coordinator(run_id, &target, Some(actor), now_ms)?,
    };
    ledger.bind(actor, run_id);
    ledger.bind(&target, run_id);
    if seated.moved {
        let run = ledger.run_mut(run_id).ok_or_else(|| unknown_run(run_id))?;
        run.auto = None;
        Ledger::adopt_standing_orphans(run, seated.generation);
        post_receipt(
            ledger,
            run_id,
            &format!("run:{run_id}"),
            serde_json::json!({
                "handoff":"coordinator seat claimed by human", "to":target, "generation":seated.generation,
            }),
            now_ms,
        );
    }
    Ok(said(
        serde_json::json!({"runId":run_id,"seated":true,"moved":seated.moved,"generation":seated.generation,
            "coordinator":ledger.run(run_id).and_then(|run|run.coordinator.as_ref()).map(CoordinatorSeat::json),
        }),
    ))
}

#[allow(clippy::too_many_arguments)]
pub(super) fn decide(
    ledger: &mut Ledger,
    team: &Team,
    launcher: &dyn Launcher,
    verb: &str,
    words: &Words,
    pane: &str,
    actor: &str,
    now_ms: i64,
) -> Result<Decided, String> {
    if actor != human_principal() {
        return Err("coordinator handover requires a human declaration through the window".into());
    }
    let run_id = words.value("--run").ok_or("handover needs --run")?;
    let generation: u32 = words
        .value("--generation")
        .ok_or("handover needs --generation")?
        .parse()
        .map_err(|_| "invalid coordinator generation")?;
    let held = ledger
        .run(run_id)
        .ok_or_else(|| unknown_run(run_id))?
        .coordinator
        .clone();
    if held.as_ref().map_or(0, |held| held.generation) != generation {
        return Err("coordinator generation changed; refresh before declaring".into());
    }
    if verb == CLAIM_VERB {
        return claim(ledger, team, run_id, words, pane, now_ms);
    }
    let held = held.ok_or("run has no coordinator seat")?;
    if verb == POLICY_VERB && words.has("--off") {
        ledger
            .run_mut(run_id)
            .ok_or_else(|| unknown_run(run_id))?
            .coordinator
            .as_mut()
            .ok_or("no coordinator")?
            .handover = None;
        post_receipt(
            ledger,
            run_id,
            &held.seat,
            serde_json::json!({
                "handoff": "coordinator handover policy disabled", "generation": generation,
            }),
            now_ms,
        );
        return Ok(said(serde_json::json!({"runId":run_id,"policy":null})));
    }
    if !held.is_held() {
        return Err("no live coordinator; automatic handover is paused".into());
    }
    eligible(ledger, run_id, team, pane)?;
    let (target_term, target_actor) = target_identity(team, pane, words)?;
    let target_seat = caller_of(team, pane);
    if verb == POLICY_VERB {
        let policy = SeatHandoverPolicy {
            target_seat,
            target_term,
            target_actor: target_actor.into(),
            source_term: words
                .value("--source-term")
                .ok_or("missing source incarnation")?
                .parse()
                .map_err(|_| "invalid source terminal")?,
            source_agent: words.value("--agent").ok_or("missing source agent")?.into(),
            source_model: words.value("--model").map(str::to_string),
            generation,
            armed_ms: now_ms,
            request: words
                .value(RETRY_REQUEST)
                .ok_or("missing declaration receipt")?
                .into(),
            status: SeatHandoverStatus::Armed,
        };
        ledger
            .run_mut(run_id)
            .ok_or_else(|| unknown_run(run_id))?
            .coordinator
            .as_mut()
            .ok_or("no coordinator")?
            .handover = Some(policy.clone());
        post_receipt(
            ledger,
            run_id,
            &held.seat,
            serde_json::json!({
                "handoff":"coordinator handover policy armed", "policy":policy.json(),
            }),
            now_ms,
        );
        return Ok(said(
            serde_json::json!({"runId":run_id,"policy":policy.json()}),
        ));
    }
    let mut policy = held.handover.clone().ok_or("no human standing order")?;
    if policy.status != SeatHandoverStatus::Armed
        || policy.generation != generation
        || policy.target_seat != target_seat
        || policy.target_term != target_term
        || policy.target_actor != target_actor
        || words.value("--declaration") != Some(policy.request.as_str())
    {
        return Err("standing order or target changed; no handover performed".into());
    }
    let marker = words
        .value("--marker")
        .filter(|line| !line.is_empty())
        .map(|line| QuotaWallMarker {
            source: words.value("--marker-source").unwrap_or("screen").into(),
            line: line.into(),
        });
    let headroom = launcher.provider_headroom(&policy.source_agent, policy.source_model.as_deref());
    let witness = quota_wall_witness(&held.seat, marker, headroom.as_ref(), now_ms)
        .ok_or("coordinator handover needs both fresh quota witnesses; quiet is not evidence")?;
    generation
        .checked_add(1)
        .ok_or("coordinator generation exhausted")?;
    let seated = ledger.take_over_coordinator(
        run_id,
        &target_seat,
        Some(target_actor),
        &held.seat,
        "human standing order: witnessed quota wall",
        now_ms,
    )?;
    policy.status = SeatHandoverStatus::Completed;
    let run = ledger.run_mut(run_id).ok_or_else(|| unknown_run(run_id))?;
    run.coordinator.as_mut().ok_or("no coordinator")?.handover = Some(policy.clone());
    run.auto = None;
    Ledger::adopt_standing_orphans(run, seated.generation);
    ledger.bind(target_actor, run_id);
    ledger.bind(&target_seat, run_id);
    let receipt = serde_json::json!({
        "handoff":"coordinator standing order completed", "from":held.seat,
        "to":target_seat,"generation":seated.generation,"declaration":policy.request,
        "provider":witness.headroom.provider,"usedPercent":witness.headroom.used_percent,
        "marker":witness.marker.line.as_str(),
        "next":"Continue this run from its ledger and existing dispatches. Do not repeat carried work. run-auto is off; rearm only if wanted.",
    });
    post_receipt(ledger, run_id, &format!("run:{run_id}"), receipt, now_ms);
    Ok(said(
        serde_json::json!({"runId":run_id,"seated":true,"generation":seated.generation,"policy":policy.json()}),
    ))
}

pub(super) fn interrupted(ledger: &mut Ledger, orders: Vec<(String, String, u32)>, now_ms: i64) {
    for (run, declaration, generation) in orders {
        post_receipt(
            ledger,
            &run,
            &format!("run:{run}"),
            serde_json::json!({
                "handoff":"coordinator standing order interrupted", "declaration":declaration,
                "generation":generation, "reason":"source seat vacated; human must rearm after return",
            }),
            now_ms,
        );
    }
}

fn post_receipt(
    ledger: &mut Ledger,
    run_id: &str,
    recipient: &str,
    body: serde_json::Value,
    now_ms: i64,
) {
    let to = if recipient.starts_with("run:") {
        recipient.into()
    } else {
        format!("pane:{recipient}")
    };
    let _ = ledger.post(
        run_id,
        Draft {
            from: LEDGER_ITSELF.into(),
            to,
            kind: MessageKind::Handoff,
            body: body.to_string().into(),
            subject: Text::default(),
            priority: Priority::High,
            payload: Text::default(),
            thread: None,
            task: None,
            dispatch: None,
        },
        now_ms,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Catalog;
    impl Launcher for Catalog {
        fn command_for(&self, _: &str, _: &str, _: &[String]) -> Result<String, String> {
            Ok("codex".into())
        }
    }
    struct Bench {
        ledger: Ledger,
        team: Team,
        clock: i64,
    }
    impl Bench {
        fn new() -> Self {
            Self {
                ledger: Ledger::new(),
                team: Team::new("human", "token", 7),
                clock: 1000,
            }
        }
        fn run(&mut self, line: &str) -> Decided {
            self.clock += 1;
            let mut argv: Vec<String> = line.split_whitespace().map(str::to_string).collect();
            if needs_a_retry_name(&argv) {
                argv.extend(["--retry-request".into(), format!("r-{}", self.clock)]);
            }
            let answer = plan(
                &mut self.ledger,
                &mut self.team,
                &Catalog,
                &argv,
                "%1",
                self.clock,
                Some("agent-session"),
            );
            self.ledger.file_receipt(&answer, self.clock);
            answer
        }
        fn json(&mut self, line: &str) -> serde_json::Value {
            let answer = self.run(line);
            assert_eq!(answer.reply.exit_code, 0, "{}", answer.reply.stderr);
            serde_json::from_str(&answer.reply.stdout).unwrap()
        }
    }

    #[test]
    fn coordinator_handover_policy_requires_a_human_declaration() {
        let mut bench = Bench::new();
        bench.json("run-create --name seat");
        let reply = bench.run("coordinator-handover-policy --off --generation 1");
        assert_ne!(reply.reply.exit_code, 0);
        assert!(
            reply.reply.stderr.contains("human"),
            "{}",
            reply.reply.stderr
        );
    }

    #[test]
    fn coordinator_handover_no_leader_means_no_automatic_dispatch() {
        let mut bench = Bench::new();
        let id = bench.json("run-create --name seat")["runId"]
            .as_str()
            .unwrap()
            .to_owned();
        bench.json("task-create --spec work");
        bench.json("run-auto --agent codex --max 1");
        let run = bench.ledger.run_mut(&id).unwrap();
        run.coordinator.as_mut().unwrap().vacated_ms = Some(2_000);
        assert!(next_dispatch(run).is_none());
    }

    struct Gauge(Option<Headroom>);
    impl Launcher for Gauge {
        fn command_for(&self, _: &str, _: &str, _: &[String]) -> Result<String, String> {
            Ok("codex".into())
        }
        fn provider_headroom(&self, _: &str, _: Option<&str>) -> Option<Headroom> {
            self.0.clone()
        }
    }
    struct Fixture {
        ledger: Ledger,
        target: Team,
        run: String,
        gauge: Gauge,
    }
    impl Fixture {
        fn new() -> Self {
            let mut ledger = Ledger::new();
            let run = ledger.create_run("handover", 1000);
            ledger
                .seat_coordinator(&run, "source/%1", Some("source-actor"), 1000)
                .unwrap();
            Self {
                ledger,
                target: Team::new("target", "token", 8),
                run,
                gauge: Gauge(Some(Headroom {
                    provider: "codex".into(),
                    used_percent: 98,
                    window: QuotaWindow::Session,
                    resets_at_ms: Some(9_000_000),
                    updated_at_ms: 1000,
                    status: "ok".into(),
                    failure_kind: None,
                })),
            }
        }
        fn native(&mut self, verb: &str, extra: &[&str], request: &str) -> Decided {
            let mut argv = vec![
                verb.into(),
                "--run".into(),
                self.run.clone(),
                "--generation".into(),
                "1".into(),
                "--target-term".into(),
                "8".into(),
                "--target-actor".into(),
                "target-actor".into(),
                "--retry-request".into(),
                request.into(),
            ];
            argv.extend(extra.iter().map(|arg| arg.to_string()));
            let answer = plan(
                &mut self.ledger,
                &mut self.target,
                &self.gauge,
                &argv,
                "%1",
                2000,
                Some(&human_principal()),
            );
            self.ledger.file_receipt(&answer, 2000);
            answer
        }
        fn arm(&mut self) -> Decided {
            self.native(
                POLICY_VERB,
                &["--source-term", "7", "--agent", "codex"],
                "arm",
            )
        }
        fn apply(&mut self, request: &str) -> Decided {
            self.native(
                APPLY_VERB,
                &[
                    "--declaration",
                    "arm",
                    "--marker",
                    "quota wall",
                    "--marker-source",
                    "screen",
                ],
                request,
            )
        }
        fn seat(&self) -> &CoordinatorSeat {
            self.ledger
                .run(&self.run)
                .unwrap()
                .coordinator
                .as_ref()
                .unwrap()
        }
    }

    #[test]
    fn coordinator_handover_is_one_atomic_transfer_with_receipts_and_no_new_work() {
        let mut f = Fixture::new();
        assert_eq!(f.arm().reply.exit_code, 0);
        let task = f
            .ledger
            .create_task(
                &f.run,
                "carried work".into(),
                String::new(),
                Vec::new(),
                None,
                1500,
            )
            .unwrap();
        let worker = f
            .ledger
            .start_worker(&f.run, "codex", ("source", "%2"), Some(&task), 1500)
            .unwrap()
            .worker;
        f.ledger
            .run_mut(&f.run)
            .unwrap()
            .workers
            .iter_mut()
            .find(|held| held.id == worker)
            .unwrap()
            .state = WorkerState::Orphaned;
        let dispatch = f.ledger.run(&f.run).unwrap().workers[0]
            .dispatch
            .clone()
            .unwrap();
        let before = f.ledger.run(&f.run).unwrap().workers.len();
        let applied = f.apply("apply");
        assert_eq!(applied.reply.exit_code, 0, "{}", applied.reply.stderr);
        assert_eq!(f.seat().seat, "target/%1");
        assert_eq!(f.seat().generation, 2);
        assert_eq!(
            f.seat().handover.as_ref().unwrap().status,
            SeatHandoverStatus::Completed
        );
        assert_eq!(f.ledger.bound_run("target-actor"), Some(f.run.as_str()));
        assert_eq!(f.ledger.run(&f.run).unwrap().workers.len(), before);
        let run = f.ledger.run(&f.run).unwrap();
        assert!(run.dispatch(&dispatch).unwrap().is_open());
        assert_eq!(run.task(&task).unwrap().status, TaskStatus::Dispatched);
        assert_eq!(run.workers[0].adopted_by, Some(2));
        assert_eq!(run.workers[0].state, WorkerState::Active);
        let messages = run.messages().len();
        assert_eq!(f.apply("apply").reply.stdout, applied.reply.stdout);
        assert_ne!(f.apply("another-apply").reply.exit_code, 0);
        assert_eq!(f.seat().generation, 2);
        assert_eq!(f.ledger.run(&f.run).unwrap().messages().len(), messages);
        let mail = f.ledger.run(&f.run).unwrap().messages();
        assert!(mail.iter().any(|message| message.to == "pane:source/%1"));
        assert!(
            mail.iter()
                .any(|message| message.to == format!("run:{}", f.run))
        );
    }

    #[test]
    fn coordinator_handover_requires_the_declaration_and_both_fresh_witnesses() {
        let mut f = Fixture::new();
        assert_ne!(f.apply("undeclared").reply.exit_code, 0);
        assert_eq!(f.arm().reply.exit_code, 0);
        assert_ne!(
            f.native(APPLY_VERB, &["--declaration", "arm"], "quiet")
                .reply
                .exit_code,
            0
        );
        let fresh = f.gauge.0.clone();
        for gauge in [
            None,
            fresh.clone().map(|mut g| {
                g.used_percent = 90;
                g
            }),
            fresh.clone().map(|mut g| {
                g.updated_at_ms = -2_000_000;
                g
            }),
            fresh.clone().map(|mut g| {
                g.resets_at_ms = Some(1500);
                g
            }),
            fresh.clone().map(|mut g| {
                g.updated_at_ms = 3000;
                g
            }),
        ] {
            f.gauge.0 = gauge;
            assert_ne!(f.apply("insufficient").reply.exit_code, 0);
            assert_eq!(f.seat().generation, 1);
        }
        f.gauge.0 = fresh;
        assert_eq!(f.apply("fresh").reply.exit_code, 0);
    }

    #[test]
    fn coordinator_handover_revalidates_worker_history_and_target_incarnation() {
        let mut f = Fixture::new();
        assert_eq!(f.arm().reply.exit_code, 0);
        let other = f.ledger.create_run("other", 1500);
        let worker = f
            .ledger
            .start_worker(&other, "codex", ("target", "%1"), None, 1500)
            .unwrap()
            .worker;
        f.ledger
            .run_mut(&other)
            .unwrap()
            .workers
            .iter_mut()
            .find(|w| w.id == worker)
            .unwrap()
            .state = WorkerState::Released;
        assert!(
            eligible(&f.ledger, &f.run, &f.target, "%1")
                .unwrap_err()
                .contains("worker history")
        );
        assert_ne!(f.apply("worker").reply.exit_code, 0);
        let mut f = Fixture::new();
        assert_eq!(f.arm().reply.exit_code, 0);
        f.target.respawn_pane("%1", 9);
        assert_ne!(f.apply("respawn").reply.exit_code, 0);
        assert_eq!(f.seat().generation, 1);
    }

    #[test]
    fn coordinator_handover_refuses_another_runs_coordinator_and_a_child_pane() {
        let mut f = Fixture::new();
        let other = f.ledger.create_run("other", 1500);
        f.ledger
            .seat_coordinator(&other, "target/%1", Some("target-actor"), 1500)
            .unwrap();
        assert_ne!(f.arm().reply.exit_code, 0);
        assert!(eligible(&f.ledger, &f.run, &f.target, "%2").is_err());
    }

    #[test]
    fn coordinator_handover_stale_generation_and_revoked_order_do_nothing() {
        let mut f = Fixture::new();
        assert_eq!(f.arm().reply.exit_code, 0);
        assert_eq!(f.native(POLICY_VERB, &["--off"], "off").reply.exit_code, 0);
        assert_ne!(f.apply("revoked").reply.exit_code, 0);
        f.ledger
            .take_over_coordinator(
                &f.run,
                "third/%1",
                Some("third"),
                "source/%1",
                "manual",
                1700,
            )
            .unwrap();
        assert_eq!(
            f.arm().reply.exit_code,
            0,
            "replay returns the old receipt without rearming"
        );
        assert_ne!(
            f.native(
                POLICY_VERB,
                &["--source-term", "7", "--agent", "codex"],
                "stale-new-order"
            )
            .reply
            .exit_code,
            0
        );
        assert_eq!(f.seat().seat, "third/%1");
    }

    #[test]
    fn coordinator_handover_persists_and_restart_interrupts_an_unspent_order() {
        let mut f = Fixture::new();
        assert_eq!(f.arm().reply.exit_code, 0);
        let encoded = serde_json::to_string(&f.ledger).unwrap();
        f.ledger = serde_json::from_str(&encoded).unwrap();
        assert_eq!(
            f.seat().handover.as_ref().unwrap().status,
            SeatHandoverStatus::Armed
        );
        f.ledger.window_restarted(3000);
        assert!(
            f.ledger
                .run(&f.run)
                .unwrap()
                .messages()
                .iter()
                .any(|message| message.body.contains("standing order interrupted"))
        );
        assert_eq!(
            f.seat().handover.as_ref().unwrap().status,
            SeatHandoverStatus::Interrupted
        );
        assert_ne!(f.apply("after-restart").reply.exit_code, 0);
        f.ledger
            .seat_coordinator(&f.run, "source/%1", Some("source-actor"), 4000)
            .unwrap();
        assert!(f.seat().handover.is_none());
        assert_ne!(f.apply("after-return").reply.exit_code, 0);
    }

    #[test]
    fn coordinator_handover_completed_receipt_survives_rebuild_without_reexecution() {
        let mut f = Fixture::new();
        assert_eq!(f.arm().reply.exit_code, 0);
        let applied = f.apply("apply");
        assert_eq!(applied.reply.exit_code, 0);
        f.ledger = serde_json::from_str(&serde_json::to_string(&f.ledger).unwrap()).unwrap();
        assert_eq!(f.apply("apply").reply.stdout, applied.reply.stdout);
        assert_eq!(f.seat().generation, 2);
    }
    #[test]
    fn coordinator_manual_claim_takes_a_live_seat_and_replays_without_moving_again() {
        let mut f = Fixture::new();
        let answer = f.native("coordinator-seat-claim", &[], "manual");
        assert_eq!(answer.reply.exit_code, 0, "{}", answer.reply.stderr);
        assert_eq!(f.seat().generation, 2);
        assert_eq!(f.seat().seat, "target/%1");
        let rows = f.ledger.run(&f.run).unwrap().messages().len();
        assert_eq!(
            f.native("coordinator-seat-claim", &[], "manual")
                .reply
                .stdout,
            answer.reply.stdout
        );
        assert_eq!(f.ledger.run(&f.run).unwrap().messages().len(), rows);
    }

    #[test]
    fn coordinator_manual_claim_of_the_current_seat_is_a_noop_but_stale_generation_is_refused() {
        let mut f = Fixture::new();
        f.ledger
            .take_over_coordinator(
                &f.run,
                "target/%1",
                Some("target-actor"),
                "source/%1",
                "manual",
                1500,
            )
            .unwrap();
        let before = f.ledger.run(&f.run).unwrap().messages().len();
        let answer = f.native("coordinator-seat-claim", &["--generation", "2"], "same");
        assert_eq!(answer.reply.exit_code, 0, "{}", answer.reply.stderr);
        let json: serde_json::Value = serde_json::from_str(&answer.reply.stdout).unwrap();
        assert_eq!(json["moved"], false);
        assert_eq!(f.seat().generation, 2);
        assert_eq!(f.ledger.run(&f.run).unwrap().messages().len(), before);
        assert_ne!(
            f.native("coordinator-seat-claim", &[], "stale")
                .reply
                .exit_code,
            0
        );
    }
    #[test]
    fn coordinator_manual_claim_fills_empty_and_vacated_seats_without_quota() {
        for vacant in [false, true] {
            let mut f = Fixture::new();
            f.gauge.0 = None;
            if vacant {
                f.ledger
                    .run_mut(&f.run)
                    .unwrap()
                    .coordinator
                    .as_mut()
                    .unwrap()
                    .vacated_ms = Some(1500);
            } else {
                f.ledger.run_mut(&f.run).unwrap().coordinator = None;
            }
            let expected = if vacant { "1" } else { "0" };
            let answer = f.native(CLAIM_VERB, &["--generation", expected], "empty");
            assert_eq!(answer.reply.exit_code, 0, "{}", answer.reply.stderr);
            assert_eq!(f.seat().generation, if vacant { 2 } else { 1 });
            assert_eq!(f.ledger.bound_run("target-actor"), Some(f.run.as_str()));
            assert!(f.seat().is_held());
        }
    }

    #[test]
    fn coordinator_manual_claim_never_promotes_a_worker_even_if_it_already_holds_the_seat() {
        for current in [false, true] {
            let mut f = Fixture::new();
            if current {
                f.ledger
                    .take_over_coordinator(
                        &f.run,
                        "target/%1",
                        Some("target-actor"),
                        "source/%1",
                        "manual",
                        1500,
                    )
                    .unwrap();
            }
            let other = f.ledger.create_run("history", 1500);
            let worker = f
                .ledger
                .start_worker(&other, "codex", ("target", "%1"), None, 1500)
                .unwrap()
                .worker;
            f.ledger
                .run_mut(&other)
                .unwrap()
                .workers
                .iter_mut()
                .find(|w| w.id == worker)
                .unwrap()
                .state = WorkerState::Released;
            let expected = if current { "2" } else { "1" };
            let before = f.ledger.export();
            let answer = f.native(CLAIM_VERB, &["--generation", expected], "worker");
            assert_ne!(answer.reply.exit_code, 0);
            assert!(
                answer.reply.stderr.contains("worker history"),
                "{}",
                answer.reply.stderr
            );
            assert_eq!(f.ledger.export(), before);
        }
    }

    #[test]
    fn coordinator_manual_claim_rejects_unknown_target_other_run_and_agent_declarations() {
        let mut f = Fixture::new();
        assert_ne!(
            f.native(CLAIM_VERB, &["--target-term", "9"], "incarnation")
                .reply
                .exit_code,
            0
        );
        let other = f.ledger.create_run("other", 1500);
        f.ledger
            .seat_coordinator(&other, "target/%1", Some("target-actor"), 1500)
            .unwrap();
        assert_ne!(f.native(CLAIM_VERB, &[], "other-run").reply.exit_code, 0);
        let mut bench = Bench::new();
        bench.json("run-create --name manual");
        let answer = bench.run("coordinator-seat-claim --generation 1");
        assert!(answer.reply.stderr.contains("human declaration"));
    }

    #[test]
    fn coordinator_manual_recent_runs_are_bounded_sorted_and_exclude_settled_history() {
        let mut f = Fixture::new();
        let finished = f.ledger.create_run("settled", 5000);
        let task = f
            .ledger
            .create_task(
                &finished,
                "done".into(),
                String::new(),
                Vec::new(),
                None,
                5000,
            )
            .unwrap();
        f.ledger
            .run_mut(&finished)
            .unwrap()
            .tasks
            .iter_mut()
            .find(|t| t.id == task)
            .unwrap()
            .status = TaskStatus::Completed;
        for i in 0..=MAX_LIST {
            f.ledger
                .create_run(&format!("pending-{i}"), 2000 + i as i64);
        }
        post_receipt(
            &mut f.ledger,
            &f.run,
            "source/%1",
            serde_json::json!({"recent":"activity"}),
            6000,
        );
        let before = f.ledger.export();
        let small = recent_runs(&f.ledger, Some(2));
        assert_eq!(small["runs"].as_array().unwrap().len(), 2);
        assert_eq!(small["runs"][0]["runId"], f.run);
        assert_eq!(small["runs"][0]["generation"], 1);
        assert_eq!(small["runs"][1]["generation"], 0);
        assert_eq!(small["truncated"], true);
        let capped = recent_runs(&f.ledger, Some(usize::MAX));
        assert_eq!(capped["runs"].as_array().unwrap().len(), MAX_LIST);
        assert!(
            !capped["runs"]
                .as_array()
                .unwrap()
                .iter()
                .any(|run| run["runId"] == finished)
        );
        assert_eq!(
            recent_runs(&f.ledger, None)["runs"]
                .as_array()
                .unwrap()
                .len(),
            RECENT_RUNS_DEFAULT
        );
        assert_eq!(
            recent_runs(&f.ledger, Some(0))["runs"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(f.ledger.export(), before);
    }
}
