//! A person's own decisions asked for a pane's agent (t-106308), end to end
//! on the moments a pane's hooks carry: each case keeps its decisions in a
//! temporary zo home's store, asks a loopback endpoint through the real
//! door, and reads the rows back off the ledger zo keeps for the project in
//! that home — never this machine's keychain, settings or ledgers.

use serde_json::{Map, json};
use zerocode_core::hook_guard::Call;
use zerocode_core::jev::custom::{Answer, Input, NOT_APPLICABLE};
use zerocode_core::jev::{AGENT_TOOL_ASK_OPTIONS, SMART_SETTINGS_KEY};

use super::*;
use crate::systemone::tests::Endpoint;

/// The pane every case asks for: a Claude pane of the person's own — not a
/// summoned worker — in launch `LAUNCH`.
const LAUNCH: &str = "launch-1";

fn pane(project: &Path) -> Pane {
    Pane {
        term: 7,
        agent: AgentKind::Claude,
        session: Some("session-1".to_string()),
        worktree: project.to_path_buf(),
        worker: false,
        prompt_key: None,
        launch: LAUNCH.to_string(),
    }
}

/// What a yes of [`yes_no`]'s decision means, as its line says it
/// ([`custom::said`]).
const YES_MEANS: &str = "A person sees it change.";

/// What its no means.
const NO_MEANS: &str = "Nothing a person sees moves.";

/// A yes-or-no decision named `name` asking at `at`, as a new one starts.
fn yes_no(name: &str, at: Point) -> Definition {
    Definition::new(
        custom::new_id(),
        name.to_string(),
        "Does this change what a person sees?".to_string(),
        Answer::YesNo {
            yes: YES_MEANS.to_string(),
            no: NO_MEANS.to_string(),
        },
        at,
    )
}

/// `line` as a reply or a brief carries it: after the line that says what
/// the lines are ([`custom::introduced`]).
fn briefed(line: String) -> String {
    custom::introduced(&[line]).expect("a line")
}

/// A decision named `name` at `at` that chooses one of `options`.
fn choosing(name: &str, at: Point, options: &[&str]) -> Definition {
    let mut definition = yes_no(name, at);
    definition.answer = Answer::Choice(
        options
            .iter()
            .map(|option| ((*option).to_string(), format!("The change is {option}.")))
            .collect(),
    );
    definition
}

/// A yes-or-no answer: its probability of yes.
fn a_noul(yes: f64) -> Value {
    json!({"type": "noul", "noul": yes})
}

/// A choice of `chosen`, with every option's probability and how sure.
fn a_choice(chosen: &str, spread: &[(&str, f64)], confidence: f64) -> Value {
    let probabilities: Map<String, Value> = spread
        .iter()
        .map(|(option, probability)| ((*option).to_string(), json!(probability)))
        .collect();
    json!({"type": "choice", "choice": chosen, "confidence": confidence, "probabilities": probabilities})
}

/// An endpoint answering each question a request asks with `answer` of its
/// id, and counting what it was asked.
fn replying(answer: impl Fn(&str) -> Value + Send + Sync + 'static) -> Endpoint {
    Endpoint::answering_each(
        "HTTP/1.1 200 OK",
        move |request| {
            let body: Value = request
                .split("\r\n\r\n")
                .nth(1)
                .and_then(|body| serde_json::from_str(body).ok())
                .unwrap_or(Value::Null);
            let answers: Map<String, Value> = body["questions"]
                .as_object()
                .into_iter()
                .flat_map(Map::keys)
                .map(|id| (id.clone(), answer(id)))
                .collect();
            json!({"model": "jev-test", "answers": answers, "usage": {"input_tokens": 20, "output_tokens": 0}})
                .to_string()
        },
        0,
    )
}

/// A temporary zo home: settings with the switch over every decision at
/// `switch`, every pane seat off and consent to `project`; the decisions
/// `kept` in its store, each in its mode. Its wire, and what it reads.
struct Home {
    home: tempfile::TempDir,
    wire: Wire,
    root: PathBuf,
}

impl Home {
    fn new(
        endpoint: &Endpoint,
        project: &Path,
        switch: JevMode,
        kept: &[(&Definition, JevMode)],
    ) -> Self {
        Self::with(endpoint, "test-key", project, switch, kept, &Map::new())
    }

    /// [`Self::new`], sending with `key`, its Jev settings holding `jev`
    /// beside its consent to the project.
    fn with(
        endpoint: &Endpoint,
        key: &str,
        project: &Path,
        switch: JevMode,
        kept: &[(&Definition, JevMode)],
        jev: &Map<String, Value>,
    ) -> Self {
        let home = tempfile::tempdir().expect("a zo home");
        let root = project.canonicalize().expect("the project");
        let mut smart = Map::new();
        for seat in PANE_SEATS {
            smart.insert(seat.setting.to_string(), json!(JevMode::Off.key()));
        }
        smart.insert(CUSTOM_DECISIONS.setting.to_string(), json!(switch.key()));
        let mut door =
            Map::from_iter([("workspaces".to_string(), json!([root.to_string_lossy()]))]);
        door.extend(jev.clone());
        smart.insert("jev".to_string(), Value::Object(door));
        let settings = home.path().join("settings.json");
        std::fs::write(&settings, json!({ SMART_SETTINGS_KEY: smart }).to_string())
            .expect("zo's settings");
        for (definition, mode) in kept {
            store::create(home.path(), definition, *mode, 1).expect("the store keeps it");
        }
        Self {
            wire: Wire::at(&endpoint.base(), key, Some(settings)),
            home,
            root,
        }
    }

    /// The standing a window reads off this home now.
    fn standing(&self) -> Standing {
        let standing = Standing::default();
        standing.read(&self.wire);
        standing
    }

    /// The rows of the decisions' ledger zo keeps for the project.
    fn rows(&self) -> Vec<Value> {
        let ledger = systemone::project_ledger_of(&self.wire, &CUSTOM_DECISIONS, &self.root)
            .expect("the ledger");
        systemone::read_rows(&ledger)
    }
}

/// A book of the case's own, for the life of the process.
fn fresh_guards() -> &'static Mutex<Guards> {
    Box::leak(Box::new(Mutex::default()))
}

fn about(id: Option<&str>, command: &str, project: &Path) -> Moment {
    Moment::CommandAbout(Call {
        id: id.map(str::to_string),
        command: Some(command.to_string()),
        cwd: Some(project.to_path_buf()),
    })
}

fn turn_ended(stopped: bool) -> Moment {
    Moment::TurnEnded {
        stopped,
        said: Some(SaidAt::Words("The release notes are written.".to_string())),
    }
}

/// File `moments` for the pane as the hook loop does, with the standing
/// `home` reads now, and wait for every question they asked: how many
/// threads they started.
fn noted(
    guards: &'static Mutex<Guards>,
    home: &Home,
    standing: &Standing,
    moments: Vec<Moment>,
) -> usize {
    noted_with(
        guards,
        home,
        (standing.asking(), &standing.decided()),
        moments,
        crate::usage_runtime::epoch_ms_now(),
    )
}

/// [`noted`], with the decisions `decided` holds, at `at`.
fn noted_with(
    guards: &'static Mutex<Guards>,
    home: &Home,
    (asking, decided): (Asking, &Decided),
    moments: Vec<Moment>,
    at: i64,
) -> usize {
    noted_in(
        guards,
        home,
        &pane(&home.root),
        (asking, decided),
        moments,
        at,
    )
}

/// [`noted_with`], for `pane`.
fn noted_in(
    guards: &'static Mutex<Guards>,
    home: &Home,
    pane: &Pane,
    (asking, decided): (Asking, &Decided),
    moments: Vec<Moment>,
    at: i64,
) -> usize {
    let handles = note_deciding(guards, &home.wire, pane, asking, decided, moments, at);
    let started = handles.len();
    for handle in handles {
        handle.join().expect("a question's thread");
    }
    started
}

/// The bridge's ask for the pane's next turn, from `launch`.
fn brief_ask(project: &Path, launch: &str) -> zerocode_hookd::TurnBriefAsk {
    zerocode_hookd::TurnBriefAsk {
        agent: AgentKind::Claude,
        pane_key: crate::hooks::pane_key_of(7),
        launch_token: launch.to_string(),
        session_id: Some("session-1".to_string()),
        supports_receipts: true,
        worktree: project.to_string_lossy().into_owned(),
        prompt: "Continue.".to_string(),
        wall: Duration::from_millis(100),
    }
}

fn brief(
    guards: &'static Mutex<Guards>,
    home: &Home,
    standing: &Standing,
    launch: &str,
) -> Option<String> {
    brief_in(
        guards,
        &standing.decided(),
        &brief_ask(&home.root, launch),
        zerocode_hookd::TURN_BRIEF_CHAR_CAP,
    )
}

/// With the switch over every decision off — or on, with no decision asking
/// at a pane's point — the standing asks nothing, so a pane's event is not
/// read for decisions at all: no moment, no store read, no thread.
#[test]
fn off_reads_nothing_for_a_decision() {
    let endpoint = replying(|_| a_noul(0.9));
    let project = tempfile::tempdir().expect("a project");
    let before = yes_no("Before", Point::BeforeCommand);
    let end = yes_no("End", Point::TurnEnd);
    let off = Home::new(
        &endpoint,
        project.path(),
        JevMode::Off,
        &[(&before, JevMode::On), (&end, JevMode::On)],
    );
    let standing = off.standing();
    assert!(standing.asking().nothing(), "{:?}", standing.asking());
    assert!(standing.decided().decisions.kept.is_empty());
    let by_name = yes_no("By name", Point::AgentAsks);
    let elsewhere = Home::new(
        &endpoint,
        project.path(),
        JevMode::On,
        &[(&by_name, JevMode::On)],
    );
    assert!(elsewhere.standing().asking().nothing());
    let payload = json!({
        "session_id": "session-1", "cwd": project.path(), "hook_event_name": "PreToolUse",
        "tool_name": "Bash", "tool_use_id": "call-1", "tool_input": {"command": "rm -rf build"},
    })
    .to_string();
    assert!(
        hook_guard::moments_parsed(
            AgentKind::Claude,
            "PreToolUse",
            &zerocode_core::payload::HookPayload::of(&payload),
            standing.asking(),
        )
        .is_empty()
    );
    let guards = fresh_guards();
    assert_eq!(
        noted(
            guards,
            &off,
            &standing,
            vec![about(Some("call-1"), "rm -rf build", project.path())]
        ),
        0
    );
    assert!(endpoint.asked().is_empty());
}

/// The standing asks at each point a decision that is not off asks at, and
/// switches no seat on.
#[test]
fn the_standing_asks_at_the_points_its_decisions_ask_at() {
    let endpoint = replying(|_| a_noul(0.9));
    let project = tempfile::tempdir().expect("a project");
    let before = yes_no("Before", Point::BeforeCommand);
    let end = yes_no("End", Point::TurnEnd);
    let resting = yes_no("Resting", Point::TurnEnd);
    let home = Home::new(
        &endpoint,
        project.path(),
        JevMode::On,
        &[(&before, JevMode::Shadow), (&resting, JevMode::Off)],
    );
    let asking = home.standing().asking();
    assert!(
        asking.decision_command && !asking.decision_turn_end,
        "{asking:?}"
    );
    assert_eq!(
        Asking {
            decision_command: false,
            ..asking
        },
        Asking::default(),
        "no seat is switched on"
    );
    store::create(home.home.path(), &end, JevMode::On, 2).expect("the store keeps it");
    assert!(home.standing().asking().decision_turn_end);
}

/// A decision that records, before a pane's command: asked through the
/// door, its row in the ledger zo keeps for the folder in zo's shape, with
/// `from` naming the pane's agent — and nothing offered.
#[test]
fn shadow_writes_its_row_and_offers_nothing() {
    let endpoint = replying(|_| a_noul(0.95));
    let project = tempfile::tempdir().expect("a project");
    let decision = yes_no("Visible", Point::BeforeCommand);
    let home = Home::new(
        &endpoint,
        project.path(),
        JevMode::On,
        &[(&decision, JevMode::Shadow)],
    );
    let standing = home.standing();
    let guards = fresh_guards();
    assert_eq!(
        noted(
            guards,
            &home,
            &standing,
            vec![about(Some("call-1"), "rm -rf build", project.path())]
        ),
        1
    );
    let rows = home.rows();
    assert_eq!(rows.len(), 1, "{rows:?}");
    let row = &rows[0];
    for key in [
        "at",
        "from",
        "decision",
        "definition",
        "point",
        "mode",
        "rubricVersion",
        "outcome",
        "chosen",
        "confidence",
        "applied",
        "elapsedMs",
        "requests",
        "retries",
        "redactedLines",
        "inputTokens",
        "model",
    ] {
        assert!(row.get(key).is_some(), "{key}: {row}");
    }
    assert_eq!(row["from"], AgentKind::Claude.slug());
    assert_eq!(row["decision"], decision.id.as_str());
    assert_eq!(row["definition"], decision.version().expect("its version"));
    assert_eq!(row["point"], Point::BeforeCommand.key());
    assert_eq!(row["mode"], JevMode::Shadow.key());
    assert_eq!(row["outcome"], ANSWERED_OUTCOME);
    assert_eq!(
        (&row["chosen"], &row["applied"], &row["requests"]),
        (&json!("yes"), &json!(false), &json!(1))
    );
    let body: Value = endpoint.asked()[0]
        .split("\r\n\r\n")
        .nth(1)
        .and_then(|body| serde_json::from_str(body).ok())
        .expect("a JSON body");
    assert_eq!(body["state"][Input::Command.key()], "rm -rf build");
    assert!(brief(guards, &home, &standing, LAUNCH).is_none());
}

/// A decision that is on, as sure as its act-from line, in the person's own
/// pane: its row says its line was handed, and the pane's next turn's brief
/// says it once — not again, and not to another launch of the pane.
#[test]
fn on_and_sure_hands_its_line_to_the_persons_own_pane_once() {
    let endpoint = replying(|_| a_noul(0.9));
    let project = tempfile::tempdir().expect("a project");
    let decision = yes_no("Visible", Point::BeforeCommand);
    let home = Home::new(
        &endpoint,
        project.path(),
        JevMode::On,
        &[(&decision, JevMode::On)],
    );
    let standing = home.standing();
    let guards = fresh_guards();
    noted(
        guards,
        &home,
        &standing,
        vec![about(Some("call-1"), "rm -rf build", project.path())],
    );
    let rows = home.rows();
    assert_eq!(
        (rows.len(), &rows[0]["applied"]),
        (1, &json!(true)),
        "{rows:?}"
    );
    assert!(!pane(&home.root).worker);
    assert!(
        brief(guards, &home, &standing, "launch-2").is_none(),
        "another launch of the pane takes nothing"
    );
    assert_eq!(
        brief(guards, &home, &standing, LAUNCH),
        Some(briefed(custom::line("Visible", YES_MEANS, 0.9)))
    );
    assert!(
        brief(guards, &home, &standing, LAUNCH).is_none(),
        "handed once"
    );
}

/// An answer under the act-from line, a tie, and an option that does not
/// apply each write their row and earn no line.
#[test]
fn unsure_tied_or_not_applicable_writes_its_row_and_no_line() {
    let mut unsure = yes_no("Unsure", Point::BeforeCommand);
    unsure.act_from = 0.8;
    let mut tied = yes_no("Tied", Point::BeforeCommand);
    tied.act_from = *custom::ACT_FROM.start();
    let scope = choosing("Scope", Point::BeforeCommand, &["docs", "code"]);
    let answers = [
        (unsure.id.clone(), a_noul(0.7)),
        (tied.id.clone(), a_noul(0.5)),
        (
            scope.id.clone(),
            a_choice(
                NOT_APPLICABLE,
                &[("docs", 0.05), ("code", 0.05), (NOT_APPLICABLE, 0.9)],
                0.9,
            ),
        ),
    ];
    let endpoint = replying(move |id| {
        answers
            .iter()
            .find(|(asked, _)| asked == id)
            .map_or(Value::Null, |(_, answer)| answer.clone())
    });
    let project = tempfile::tempdir().expect("a project");
    let home = Home::new(
        &endpoint,
        project.path(),
        JevMode::On,
        &[
            (&unsure, JevMode::On),
            (&tied, JevMode::On),
            (&scope, JevMode::On),
        ],
    );
    let standing = home.standing();
    let guards = fresh_guards();
    noted(
        guards,
        &home,
        &standing,
        vec![about(Some("call-1"), "rm -rf build", project.path())],
    );
    let rows = home.rows();
    assert_eq!(rows.len(), 3, "{rows:?}");
    assert!(
        rows.iter()
            .all(|row| row["outcome"] == ANSWERED_OUTCOME && row["applied"] == false),
        "{rows:?}"
    );
    assert!(brief(guards, &home, &standing, LAUNCH).is_none());
}

/// A command the read-only rule clears asks nothing and writes no row; one
/// call reported twice before it runs — once by an event that names no call
/// — is asked once.
#[test]
fn a_read_only_command_asks_nothing_and_a_call_reported_twice_asks_once() {
    let endpoint = replying(|_| a_noul(0.9));
    let project = tempfile::tempdir().expect("a project");
    let decision = yes_no("Visible", Point::BeforeCommand);
    let home = Home::new(
        &endpoint,
        project.path(),
        JevMode::On,
        &[(&decision, JevMode::Shadow)],
    );
    let standing = home.standing();
    let guards = fresh_guards();
    assert_eq!(
        noted(
            guards,
            &home,
            &standing,
            vec![about(Some("call-1"), "git status", project.path())]
        ),
        0
    );
    assert!(home.rows().is_empty());
    assert!(endpoint.asked().is_empty());
    let twice = vec![
        about(None, "rm -rf build", project.path()),
        about(Some("call-2"), "rm -rf build", project.path()),
    ];
    assert_eq!(noted(guards, &home, &standing, twice), 1);
    assert_eq!(home.rows().len(), 1);
}

/// A turn the person stopped asks no decision at a turn's end; a turn that
/// ended with its answer does, and what an acting one earned is offered to
/// the next turn's brief. A later turn's end that earns nothing takes the
/// earlier offer back.
#[test]
fn a_turns_end_asks_unless_stopped_and_its_offer_is_replaced_by_a_later_one() {
    let sure = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let answering = std::sync::Arc::clone(&sure);
    let endpoint = replying(move |_| {
        a_noul(if answering.load(std::sync::atomic::Ordering::SeqCst) {
            0.9
        } else {
            0.6
        })
    });
    let project = tempfile::tempdir().expect("a project");
    let decision = yes_no("Done", Point::TurnEnd);
    let home = Home::new(
        &endpoint,
        project.path(),
        JevMode::On,
        &[(&decision, JevMode::On)],
    );
    let standing = home.standing();
    let guards = fresh_guards();
    assert_eq!(noted(guards, &home, &standing, vec![turn_ended(true)]), 0);
    assert!(home.rows().is_empty());
    assert_eq!(noted(guards, &home, &standing, vec![turn_ended(false)]), 1);
    let rows = home.rows();
    assert_eq!(
        (&rows[0]["point"], &rows[0]["applied"]),
        (&json!(Point::TurnEnd.key()), &json!(true))
    );
    let body: Value = endpoint.asked()[0]
        .split("\r\n\r\n")
        .nth(1)
        .and_then(|body| serde_json::from_str(body).ok())
        .expect("a JSON body");
    assert_eq!(
        body["state"][Input::FinalText.key()],
        "The release notes are written."
    );
    assert_eq!(
        brief(guards, &home, &standing, LAUNCH),
        Some(briefed(custom::line("Done", YES_MEANS, 0.9)))
    );
    noted(guards, &home, &standing, vec![turn_ended(false)]);
    sure.store(false, std::sync::atomic::Ordering::SeqCst);
    noted(guards, &home, &standing, vec![turn_ended(false)]);
    assert_eq!(home.rows().len(), 3);
    assert!(
        brief(guards, &home, &standing, LAUNCH).is_none(),
        "a later turn's end that earned nothing replaced the earlier offer"
    );
}

/// A decision edited after it answered, or switched to recording, no
/// longer hands the line its earlier version earned.
#[test]
fn a_decision_edited_after_it_answered_drops_its_waiting_line() {
    let endpoint = replying(|_| a_noul(0.9));
    let project = tempfile::tempdir().expect("a project");
    let decision = yes_no("Visible", Point::BeforeCommand);
    let home = Home::new(
        &endpoint,
        project.path(),
        JevMode::On,
        &[(&decision, JevMode::On)],
    );
    let guards = fresh_guards();
    noted(
        guards,
        &home,
        &home.standing(),
        vec![about(Some("call-1"), "rm -rf build", project.path())],
    );
    let mut edited = decision.clone();
    edited.question = "Does this change what a reader sees?".to_string();
    store::edit(home.home.path(), &edited, 2).expect("the next version");
    assert!(brief(guards, &home, &home.standing(), LAUNCH).is_none());
    noted(
        guards,
        &home,
        &home.standing(),
        vec![about(Some("call-2"), "rm -rf dist", project.path())],
    );
    store::set_mode(home.home.path(), &decision.id, JevMode::Shadow, 3).expect("recording");
    assert!(brief(guards, &home, &home.standing(), LAUNCH).is_none());
}

/// One hour, in milliseconds.
const HOUR_MS: i64 = 3_600_000;

/// One day, in milliseconds.
const DAY_MS: i64 = 24 * HOUR_MS;

/// A command for each of `calls`, each asked at `at` with the decisions
/// `decided` holds: how many threads they started together.
fn commands(
    guards: &'static Mutex<Guards>,
    home: &Home,
    (asking, decided): (Asking, &Decided),
    calls: std::ops::RangeInclusive<u32>,
    at: i64,
) -> usize {
    calls
        .map(|call| {
            noted_with(
                guards,
                home,
                (asking, decided),
                vec![about(
                    Some(&format!("call-{call}")),
                    &format!("rm -rf build-{call}"),
                    &home.root,
                )],
                at,
            )
        })
        .sum()
}

/// The first command of a span writes the refusal's one row; the hundred
/// after it in the same span start no thread — so no store read, no hour
/// file opened and no row: the outcomes of the rows written, in order.
fn one_row_then_quiet(
    home: &Home,
    standing: &Standing,
    at: i64,
) -> (&'static Mutex<Guards>, Vec<String>) {
    let guards = fresh_guards();
    let decided = standing.decided();
    assert_eq!(
        commands(guards, home, (standing.asking(), &decided), 0..=0, at),
        1
    );
    assert_eq!(
        commands(guards, home, (standing.asking(), &decided), 1..=100, at),
        0,
        "a quiet decision starts no thread"
    );
    (guards, outcomes(home))
}

fn outcomes(home: &Home) -> Vec<String> {
    home.rows()
        .iter()
        .map(|row| row["outcome"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// An hour found full — filled here through the core's own counter, in the
/// files zo counts in — writes one row with zo's word for it and sends
/// nothing; the hundred commands after it cost the decision nothing, and
/// the next hour asks again.
#[test]
fn a_full_hour_writes_one_refused_row_and_the_next_hour_asks_again() {
    let endpoint = replying(|_| a_noul(0.9));
    let project = tempfile::tempdir().expect("a project");
    let mut decision = yes_no("Hourly", Point::BeforeCommand);
    decision.max_per_hour = *custom::HOURLY_REQUESTS.start();
    let home = Home::new(
        &endpoint,
        project.path(),
        JevMode::On,
        &[(&decision, JevMode::On)],
    );
    let at = crate::usage_runtime::epoch_ms_now();
    let own = count::hour_path(home.home.path(), &decision.id, &systemone::hour_at(at));
    for _ in 0..decision.max_per_hour {
        count::count_one_in_the_hour(&own).expect("an hour counted");
    }
    let standing = home.standing();
    let (guards, written) = one_row_then_quiet(&home, &standing, at);
    assert_eq!(written, [door::Refused::HourlyBudget.token()]);
    let rows = home.rows();
    assert_eq!(
        (&rows[0]["requests"], &rows[0]["applied"]),
        (&json!(0), &json!(false))
    );
    assert_eq!(
        std::fs::metadata(&own).map(|held| held.len()).ok(),
        Some(decision.max_per_hour)
    );
    assert!(endpoint.asked().is_empty());
    let next = commands(
        guards,
        &home,
        (standing.asking(), &standing.decided()),
        101..=101,
        at + HOUR_MS,
    );
    assert_eq!(
        (next, outcomes(&home).len()),
        (1, 2),
        "the next hour asks again"
    );
}

/// A day's budget spent writes one row a day; the next day asks again.
#[test]
fn a_spent_day_writes_one_refused_row_and_the_next_day_asks_again() {
    let endpoint = replying(|_| a_noul(0.9));
    let project = tempfile::tempdir().expect("a project");
    let decision = yes_no("Daily", Point::BeforeCommand);
    let home = Home::with(
        &endpoint,
        "test-key",
        project.path(),
        JevMode::On,
        &[(&decision, JevMode::On)],
        &Map::from_iter([(door::DAILY_REQUESTS_SETTING.to_string(), json!(1))]),
    );
    count::count_one(&count::requests_path(home.home.path(), &systemone::today()))
        .expect("a day counted");
    let standing = home.standing();
    let at = crate::usage_runtime::epoch_ms_now();
    let (guards, written) = one_row_then_quiet(&home, &standing, at);
    assert_eq!(written, [door::Refused::Budget.token()]);
    assert!(endpoint.asked().is_empty());
    let next = commands(
        guards,
        &home,
        (standing.asking(), &standing.decided()),
        101..=101,
        at + DAY_MS,
    );
    assert_eq!(
        (next, outcomes(&home).len()),
        (1, 2),
        "the next day asks again"
    );
}

/// A folder the person has not consented to writes one row; consent given —
/// the settings read again — asks at once.
#[test]
fn a_folder_not_consented_writes_one_row_and_consent_asks_again() {
    let endpoint = replying(|_| a_noul(0.9));
    let project = tempfile::tempdir().expect("a project");
    let elsewhere = tempfile::tempdir().expect("another folder");
    let decision = yes_no("Consent", Point::BeforeCommand);
    let home = Home::with(
        &endpoint,
        "test-key",
        project.path(),
        JevMode::On,
        &[(&decision, JevMode::On)],
        &Map::from_iter([(
            "workspaces".to_string(),
            json!([elsewhere.path().canonicalize().expect("a folder")]),
        )]),
    );
    let at = crate::usage_runtime::epoch_ms_now();
    let (guards, written) = one_row_then_quiet(&home, &home.standing(), at);
    assert_eq!(written, [door::Refused::NotConsented.token()]);
    assert!(endpoint.asked().is_empty());
    let settings = home.home.path().join("settings.json");
    let mut written: Value =
        serde_json::from_str(&std::fs::read_to_string(&settings).expect("the settings"))
            .expect("JSON");
    written[SMART_SETTINGS_KEY]["jev"]["workspaces"] = json!([home.root]);
    std::fs::write(&settings, written.to_string()).expect("consent given");
    let standing = home.standing();
    let next = commands(
        guards,
        &home,
        (standing.asking(), &standing.decided()),
        101..=101,
        at,
    );
    assert_eq!(next, 1, "the settings read again ask at once");
    assert_eq!(
        outcomes(&home),
        [door::Refused::NotConsented.token(), ANSWERED_OUTCOME]
    );
}

/// The person's pane in another folder, the one `home` did not consent to:
/// its own pane id, under the same launch.
fn pane_elsewhere(folder: &Path) -> Pane {
    Pane {
        term: 8,
        session: Some("session-2".to_string()),
        ..pane(folder)
    }
}

/// The outcomes of the rows zo's ledger keeps for `folder`, in order.
fn outcomes_in(home: &Home, folder: &Path) -> Vec<String> {
    let ledger =
        systemone::project_ledger_of(&home.wire, &CUSTOM_DECISIONS, folder).expect("the ledger");
    systemone::read_rows(&ledger)
        .iter()
        .map(|row| row["outcome"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// A folder not consented is that folder's refusal alone: a pane there
/// writes its one row and goes quiet, and in the same hour a pane in the
/// folder the person consented to still asks the same decision — before a
/// command and at a turn's end alike.
#[test]
fn a_folder_not_consented_quiets_the_decision_there_alone() {
    for at_point in [Point::BeforeCommand, Point::TurnEnd] {
        let endpoint = replying(|_| a_noul(0.9));
        let project = tempfile::tempdir().expect("a project");
        let other = tempfile::tempdir().expect("a folder not consented");
        let elsewhere = other.path().canonicalize().expect("a folder");
        let decision = yes_no("Everywhere", at_point);
        let home = Home::new(
            &endpoint,
            project.path(),
            JevMode::On,
            &[(&decision, JevMode::On)],
        );
        let standing = home.standing();
        let decided = standing.decided();
        let guards = fresh_guards();
        let at = crate::usage_runtime::epoch_ms_now();
        let moment = |call: u32, folder: &Path| match at_point {
            Point::BeforeCommand => about(
                Some(&format!("call-{call}")),
                &format!("rm -rf build-{call}"),
                folder,
            ),
            _ => turn_ended(false),
        };
        let in_folder = |pane: &Pane, folder: &Path, calls: std::ops::RangeInclusive<u32>| {
            calls
                .map(|call| {
                    noted_in(
                        guards,
                        &home,
                        pane,
                        (standing.asking(), &decided),
                        vec![moment(call, folder)],
                        at,
                    )
                })
                .sum::<usize>()
        };
        let there = pane_elsewhere(&elsewhere);
        assert_eq!(in_folder(&there, &elsewhere, 0..=0), 1, "{at_point:?}");
        assert_eq!(
            in_folder(&there, &elsewhere, 1..=100),
            0,
            "{at_point:?}: quiet in the folder not consented"
        );
        assert_eq!(
            outcomes_in(&home, &elsewhere),
            [door::Refused::NotConsented.token()],
            "{at_point:?}"
        );
        assert!(endpoint.asked().is_empty(), "{at_point:?}");
        assert_eq!(
            in_folder(&pane(&home.root), &home.root, 101..=101),
            1,
            "{at_point:?}: the consented folder's pane still asks"
        );
        assert_eq!(outcomes(&home), [ANSWERED_OUTCOME], "{at_point:?}");
        assert_eq!(endpoint.asked().len(), 1, "{at_point:?}");
    }
}

/// A decision that chose its folders: a pane outside them asks it on a
/// thread once — which learns the pane's folder — and after that its
/// commands start no thread at all, while a pane inside them asks.
#[test]
fn a_pane_outside_a_decisions_chosen_folders_starts_no_thread_after_its_first() {
    let endpoint = replying(|_| a_noul(0.9));
    let project = tempfile::tempdir().expect("a project");
    let other = tempfile::tempdir().expect("another folder");
    let elsewhere = other.path().canonicalize().expect("a folder");
    let mut decision = yes_no("Here only", Point::BeforeCommand);
    let home = Home::with(
        &endpoint,
        "test-key",
        project.path(),
        JevMode::On,
        &[],
        &Map::from_iter([(
            "workspaces".to_string(),
            json!([project.path().canonicalize().expect("a project"), elsewhere]),
        )]),
    );
    decision.folders = custom::Folders::Chosen(
        std::iter::once(home.root.to_string_lossy().into_owned()).collect(),
    );
    store::create(home.home.path(), &decision, JevMode::On, 1).expect("the store keeps it");
    let standing = home.standing();
    let decided = standing.decided();
    let guards = fresh_guards();
    let at = crate::usage_runtime::epoch_ms_now();
    let there = pane_elsewhere(&elsewhere);
    let started: Vec<usize> = (0..=100)
        .map(|call| {
            noted_in(
                guards,
                &home,
                &there,
                (standing.asking(), &decided),
                vec![about(
                    Some(&format!("call-{call}")),
                    &format!("rm -rf build-{call}"),
                    &elsewhere,
                )],
                at,
            )
        })
        .collect();
    assert_eq!(started.iter().sum::<usize>(), 1, "{started:?}");
    assert!(outcomes_in(&home, &elsewhere).is_empty());
    assert!(endpoint.asked().is_empty());
    assert_eq!(
        commands(guards, &home, (standing.asking(), &decided), 101..=101, at),
        1
    );
    assert_eq!(outcomes(&home), [ANSWERED_OUTCOME]);
}

/// No key writes one row; the next hour asks again.
#[test]
fn no_key_writes_one_row_and_the_next_hour_asks_again() {
    let endpoint = replying(|_| a_noul(0.9));
    let project = tempfile::tempdir().expect("a project");
    let decision = yes_no("Keyless", Point::BeforeCommand);
    let home = Home::with(
        &endpoint,
        "",
        project.path(),
        JevMode::On,
        &[(&decision, JevMode::On)],
        &Map::new(),
    );
    let standing = home.standing();
    let at = crate::usage_runtime::epoch_ms_now();
    let (guards, written) = one_row_then_quiet(&home, &standing, at);
    assert_eq!(written, [door::Refused::NoKey.token()]);
    let next = commands(
        guards,
        &home,
        (standing.asking(), &standing.decided()),
        101..=101,
        at + HOUR_MS,
    );
    assert_eq!(
        (next, outcomes(&home).len()),
        (1, 2),
        "the next hour asks again"
    );
}

/// A definition that breaks a rule writes its rule's word once and sends
/// nothing; another version of it asks again.
#[test]
fn a_definition_that_breaks_a_rule_writes_one_row_and_its_next_version_asks_again() {
    let endpoint = replying(|_| a_noul(0.9));
    let project = tempfile::tempdir().expect("a project");
    let decision = yes_no("Broken", Point::BeforeCommand);
    let home = Home::new(
        &endpoint,
        project.path(),
        JevMode::On,
        &[(&decision, JevMode::On)],
    );
    let standing = home.standing();
    let mut broken = standing.decided().decisions.as_ref().clone();
    broken.kept[0].definition.act_from = 0.1;
    let decided = Decided {
        decisions: std::sync::Arc::new(broken.clone()),
        quiet: std::sync::Arc::default(),
    };
    let guards = fresh_guards();
    let at = crate::usage_runtime::epoch_ms_now();
    assert_eq!(
        commands(guards, &home, (standing.asking(), &decided), 0..=0, at),
        1
    );
    assert_eq!(
        commands(guards, &home, (standing.asking(), &decided), 1..=100, at),
        0
    );
    assert_eq!(outcomes(&home), [custom::Refusal::ActFrom.word()]);
    assert!(endpoint.asked().is_empty());
    let mut next = broken;
    next.kept[0].entry.definition = "another-version".to_string();
    let moved = Decided {
        decisions: std::sync::Arc::new(next),
        quiet: std::sync::Arc::clone(&decided.quiet),
    };
    assert_eq!(
        commands(guards, &home, (standing.asking(), &moved), 101..=101, at),
        1
    );
    assert_eq!(outcomes(&home).len(), 2, "another version asks again");
}

/// The rows land in the very file zo writes for the same folder: under zo's
/// home, the project's state by the name zo gives the folder, the folder
/// every zo seat appends in, and the provider's evidence folder — zo's
/// `shadow_ledger_path(cwd, CUSTOM_DECISIONS.ledger)`, whose layout
/// `jev_scope` pins against zo's own source.
#[test]
fn the_rows_land_in_the_file_zo_writes_for_the_folder() {
    let endpoint = replying(|_| a_noul(0.9));
    let project = tempfile::tempdir().expect("a project");
    let home = Home::new(&endpoint, project.path(), JevMode::On, &[]);
    let settings = home.home.path().join("settings.json");
    let provider = judgment_provider::Provider::read(&settings).expect("the provider");
    let zo_writes = provider
        .evidence_dir(
            &home
                .home
                .path()
                .join(crate::jev_scope::ZO_PROJECTS_DIR)
                .join(zerocode_core::zo_project::project_slug(&home.root))
                .join(crate::jev_scope::ZO_PROJECT_STATE_DIR)
                .join(crate::jev_scope::JEV_LEDGER_DIR),
        )
        .join(CUSTOM_DECISIONS.ledger);
    assert_eq!(
        systemone::project_ledger_of(&home.wire, &CUSTOM_DECISIONS, &home.root),
        Some(zo_writes)
    );
}

/// Before a command, a line waits for the agent's next tool boundary: taken
/// there once — not again, not at the next turn's brief, and not by another
/// launch of the pane.
#[test]
fn a_commands_line_is_taken_at_the_next_tool_boundary_once() {
    let endpoint = replying(|_| a_noul(0.9));
    let project = tempfile::tempdir().expect("a project");
    let decision = yes_no("Visible", Point::BeforeCommand);
    let home = Home::new(
        &endpoint,
        project.path(),
        JevMode::On,
        &[(&decision, JevMode::On)],
    );
    let standing = home.standing();
    let guards = fresh_guards();
    noted(
        guards,
        &home,
        &standing,
        vec![about(Some("call-1"), "rm -rf build", project.path())],
    );
    let key = crate::hooks::pane_key_of(7);
    let decided = standing.decided();
    assert!(boundary_in(guards, &decided, &key, "launch-2").is_none());
    assert_eq!(
        boundary_in(guards, &decided, &key, LAUNCH),
        Some(briefed(custom::line("Visible", YES_MEANS, 0.9)))
    );
    assert!(boundary_in(guards, &decided, &key, LAUNCH).is_none());
    assert!(brief(guards, &home, &standing, LAUNCH).is_none());
}

/// What a turn's end offered waits for the next turn's brief: a tool
/// boundary in between takes none of it.
#[test]
fn a_turns_offer_is_not_taken_at_a_tool_boundary() {
    let endpoint = replying(|_| a_noul(0.9));
    let project = tempfile::tempdir().expect("a project");
    let decision = yes_no("Done", Point::TurnEnd);
    let home = Home::new(
        &endpoint,
        project.path(),
        JevMode::On,
        &[(&decision, JevMode::On)],
    );
    let standing = home.standing();
    let guards = fresh_guards();
    noted(guards, &home, &standing, vec![turn_ended(false)]);
    let key = crate::hooks::pane_key_of(7);
    assert!(boundary_in(guards, &standing.decided(), &key, LAUNCH).is_none());
    assert_eq!(
        brief(guards, &home, &standing, LAUNCH),
        Some(briefed(custom::line("Done", YES_MEANS, 0.9)))
    );
}

/// zo's settings as `home` keeps them, `change`d and written back.
fn settings_changed(home: &Home, change: impl FnOnce(&mut Value)) {
    let settings = home.home.path().join("settings.json");
    let mut written: Value =
        serde_json::from_str(&std::fs::read_to_string(&settings).expect("the settings"))
            .expect("JSON");
    change(&mut written);
    std::fs::write(&settings, written.to_string()).expect("the settings written");
}

/// A decision that stands on and earns its line before a command, its line
/// left waiting in the pane.
fn a_line_waiting(project: &Path) -> (Endpoint, Home, Standing, &'static Mutex<Guards>) {
    let endpoint = replying(|_| a_noul(0.9));
    let decision = yes_no("Visible", Point::BeforeCommand);
    let home = Home::new(&endpoint, project, JevMode::On, &[(&decision, JevMode::On)]);
    let standing = home.standing();
    let guards = fresh_guards();
    noted(
        guards,
        &home,
        &standing,
        vec![about(Some("call-1"), "rm -rf build", project)],
    );
    (endpoint, home, standing, guards)
}

/// Taking a waiting line reads the snapshot alone, never zo's settings: a
/// settings file that no longer reads still lets the tool boundary's reply
/// carry the line the door let act when it was placed.
#[test]
fn a_waiting_line_is_taken_without_reading_the_settings() {
    let project = tempfile::tempdir().expect("a project");
    let (_endpoint, home, standing, guards) = a_line_waiting(project.path());
    std::fs::write(home.home.path().join("settings.json"), "not JSON").expect("unreadable");
    assert_eq!(
        boundary_in(
            guards,
            &standing.decided(),
            &crate::hooks::pane_key_of(7),
            LAUNCH
        ),
        Some(briefed(custom::line("Visible", YES_MEANS, 0.9)))
    );
}

/// Consent withdrawn: when the settings are read again, the lines waiting in
/// that folder's panes are dropped, so no reply carries one.
#[test]
fn consent_withdrawn_drops_the_waiting_lines_when_the_settings_are_read_again() {
    let project = tempfile::tempdir().expect("a project");
    let (_endpoint, home, standing, guards) = a_line_waiting(project.path());
    settings_changed(&home, |written| {
        written[SMART_SETTINGS_KEY]["jev"]["workspaces"] = json!([]);
    });
    drop_lines_the_door_refuses(guards, &home.wire);
    let key = crate::hooks::pane_key_of(7);
    assert!(boundary_in(guards, &standing.decided(), &key, LAUNCH).is_none());
    assert!(brief(guards, &home, &standing, LAUNCH).is_none());
}

/// A turn that ends, not stopped, replaces what the turn before it offered
/// even when no decision asks at its end — the offer was for the turn that
/// just ended. A stopped turn replaces nothing, as zo's asks nothing.
#[test]
fn a_later_turns_end_replaces_the_offer_even_when_nothing_asks_there() {
    let endpoint = replying(|_| a_noul(0.9));
    let project = tempfile::tempdir().expect("a project");
    let decision = yes_no("Done", Point::TurnEnd);
    let home = Home::new(
        &endpoint,
        project.path(),
        JevMode::On,
        &[(&decision, JevMode::On)],
    );
    let standing = home.standing();
    let guards = fresh_guards();
    let at = crate::usage_runtime::epoch_ms_now();
    let ended = |stopped: bool, asking: Asking, decided: &Decided, at: i64| {
        noted_with(
            guards,
            &home,
            (asking, decided),
            vec![turn_ended(stopped)],
            at,
        )
    };
    let nothing_asks = (Asking::default(), Decided::default());
    assert_eq!(ended(false, standing.asking(), &standing.decided(), at), 1);
    assert_eq!(ended(true, nothing_asks.0, &nothing_asks.1, at + 1), 0);
    assert_eq!(
        brief(guards, &home, &standing, LAUNCH),
        Some(briefed(custom::line("Done", YES_MEANS, 0.9))),
        "a stopped turn left the offer standing"
    );
    assert_eq!(
        ended(false, standing.asking(), &standing.decided(), at + 2),
        1
    );
    assert_eq!(ended(false, nothing_asks.0, &nothing_asks.1, at + 3), 0);
    assert!(
        brief(guards, &home, &standing, LAUNCH).is_none(),
        "the later turn's end replaced the offer with nothing"
    );
}

/// With Jev itself off, a pane's decisions ask nothing at all: no thread and
/// no row — not one `off` row an hour.
#[test]
fn with_jev_off_a_panes_decisions_start_no_thread_and_write_no_row() {
    let endpoint = replying(|_| a_noul(0.9));
    let project = tempfile::tempdir().expect("a project");
    let decision = yes_no("Visible", Point::BeforeCommand);
    let home = Home::with(
        &endpoint,
        "test-key",
        project.path(),
        JevMode::On,
        &[(&decision, JevMode::On)],
        &Map::from_iter([("enabled".to_string(), json!(false))]),
    );
    let standing = home.standing();
    assert!(standing.asking().nothing(), "{:?}", standing.asking());
    let guards = fresh_guards();
    assert_eq!(
        commands(
            guards,
            &home,
            (standing.asking(), &standing.decided()),
            0..=100,
            0
        ),
        0
    );
    assert!(home.rows().is_empty());
    settings_changed(&home, |written| {
        written[SMART_SETTINGS_KEY]["jev"]["enabled"] = json!(true);
    });
    assert!(home.standing().asking().decision_command, "turned back on");
}

/// No ledger to file a row under — zo's provider settings that do not read —
/// quiets the decision for the hour as no key does: one thread, then none.
#[test]
fn a_provider_that_does_not_read_quiets_the_decision_for_the_hour() {
    let endpoint = replying(|_| a_noul(0.9));
    let project = tempfile::tempdir().expect("a project");
    let decision = yes_no("Visible", Point::BeforeCommand);
    let home = Home::new(
        &endpoint,
        project.path(),
        JevMode::On,
        &[(&decision, JevMode::On)],
    );
    settings_changed(&home, |written| {
        written[SMART_SETTINGS_KEY][judgment_provider::SETTINGS_KEY] = json!("not a provider");
    });
    let standing = home.standing();
    let decided = standing.decided();
    let guards = fresh_guards();
    let at = crate::usage_runtime::epoch_ms_now();
    assert_eq!(
        commands(guards, &home, (standing.asking(), &decided), 0..=0, at),
        1
    );
    assert_eq!(
        commands(guards, &home, (standing.asking(), &decided), 1..=100, at),
        0,
        "quiet for the hour"
    );
    assert!(endpoint.asked().is_empty());
}

/// The ledger a pane's decisions write keeps the bound zo keeps on it: past
/// it, the newer half stays and the pane's row lands after it.
#[test]
fn the_ledger_a_pane_writes_is_cut_where_zo_cuts_it() {
    let endpoint = replying(|_| a_noul(0.9));
    let project = tempfile::tempdir().expect("a project");
    let decision = yes_no("Visible", Point::BeforeCommand);
    let home = Home::new(
        &endpoint,
        project.path(),
        JevMode::Shadow,
        &[(&decision, JevMode::Shadow)],
    );
    let ledger = systemone::project_ledger_of(&home.wire, &CUSTOM_DECISIONS, &home.root)
        .expect("the ledger");
    std::fs::create_dir_all(ledger.parent().expect("its folder")).expect("its folder");
    let line = format!("{}\n", json!({"at": 0, "pad": "x".repeat(200)}));
    let lines = zerocode_core::jev::journal::LEDGER_MAX_BYTES / line.len() as u64 + 1;
    std::fs::write(&ledger, line.repeat(usize::try_from(lines).expect("lines"))).expect("full");
    let standing = home.standing();
    noted(
        fresh_guards(),
        &home,
        &standing,
        vec![about(Some("call-1"), "rm -rf build", project.path())],
    );
    let size = std::fs::metadata(&ledger).expect("the ledger").len();
    assert!(
        size <= zerocode_core::jev::journal::LEDGER_MAX_BYTES,
        "{size} bytes"
    );
    let rows = home.rows();
    assert_eq!(rows.len() as u64, lines - lines / 2 + 1);
    assert_eq!(
        rows.last().expect("a row")["outcome"],
        json!(ANSWERED_OUTCOME)
    );
}

/// Where no agent asked — before a command, at a turn's end — a yes-or-no
/// decision speaks only of its yes: a no as sure as 0.99 keeps its row, says
/// it was not applied and hands no line, in no wording. A yes is handed in
/// the person's own words for it ([`custom::said`]), never the bare word.
#[test]
fn an_unasked_no_hands_nothing_and_a_yes_says_what_it_means() {
    for at_point in [Point::BeforeCommand, Point::TurnEnd] {
        for (yes, handed) in [(0.01, false), (0.99, true)] {
            let endpoint = replying(move |_| a_noul(yes));
            let project = tempfile::tempdir().expect("a project");
            let decision = yes_no("Seen", at_point);
            let home = Home::new(
                &endpoint,
                project.path(),
                JevMode::On,
                &[(&decision, JevMode::On)],
            );
            let standing = home.standing();
            let guards = fresh_guards();
            let moment = match at_point {
                Point::BeforeCommand => about(Some("call-1"), "rm -rf build", project.path()),
                _ => turn_ended(false),
            };
            assert_eq!(noted(guards, &home, &standing, vec![moment]), 1);
            let rows = home.rows();
            let chosen = if handed {
                AGENT_TOOL_ASK_OPTIONS[0]
            } else {
                AGENT_TOOL_ASK_OPTIONS[1]
            };
            assert_eq!(
                (&rows[0]["chosen"], &rows[0]["applied"]),
                (&json!(chosen), &json!(handed)),
                "{at_point:?} {yes}"
            );
            let said = brief(guards, &home, &standing, LAUNCH);
            assert_eq!(
                said,
                handed.then(|| briefed(custom::line("Seen", YES_MEANS, 0.99))),
                "{at_point:?} {yes}"
            );
            assert!(
                !said.unwrap_or_default().contains(NO_MEANS),
                "{at_point:?} {yes}"
            );
        }
    }
}

/// One decision answering yes before three commands leaves one line waiting
/// — its newest — so the brief of an agent that took none at a tool
/// boundary carries it once, not once a command.
#[test]
fn a_decision_leaves_one_line_waiting_however_many_commands_it_answered() {
    let endpoint = replying(|_| a_noul(0.9));
    let project = tempfile::tempdir().expect("a project");
    let decision = yes_no("Visible", Point::BeforeCommand);
    let home = Home::new(
        &endpoint,
        project.path(),
        JevMode::On,
        &[(&decision, JevMode::On)],
    );
    let standing = home.standing();
    let guards = fresh_guards();
    let at = crate::usage_runtime::epoch_ms_now();
    assert_eq!(
        commands(
            guards,
            &home,
            (standing.asking(), &standing.decided()),
            1..=3,
            at
        ),
        3
    );
    assert_eq!(home.rows().len(), 3);
    assert_eq!(
        brief(guards, &home, &standing, LAUNCH),
        Some(briefed(custom::line("Visible", YES_MEANS, 0.9)))
    );
}

/// An ask that began before zo's settings were read again places nothing
/// after that reading: the door it asked may no longer let the folder act.
/// One that began after places its line.
#[test]
fn an_ask_begun_before_the_settings_were_read_again_places_nothing() {
    let endpoint = replying(|_| a_noul(0.9));
    let project = tempfile::tempdir().expect("a project");
    let home = Home::new(&endpoint, project.path(), JevMode::On, &[]);
    let standing = home.standing();
    let guards = fresh_guards();
    noted(
        guards,
        &home,
        &standing,
        vec![about(Some("call-1"), "ls", project.path())],
    );
    let here = pane(&home.root);
    let begun = keep_the_folder(guards, &here, &home.root);
    let handed = || {
        vec![Handed {
            decision: "d-0000000a".to_string(),
            version: "v".to_string(),
            line: custom::line("Visible", YES_MEANS, 0.9),
        }]
    };
    drop_lines_the_door_refuses(guards, &home.wire);
    let at = u64::try_from(crate::usage_runtime::epoch_ms_now()).expect("now");
    assert!(!place(
        guards,
        &here,
        (Point::BeforeCommand, at),
        handed(),
        begun
    ));
    assert!(
        guards.lock().expect("the books").panes[&here.term]
            .waiting
            .command
            .is_empty()
    );
    let now = keep_the_folder(guards, &here, &home.root);
    assert!(place(
        guards,
        &here,
        (Point::BeforeCommand, at),
        handed(),
        now
    ));
}

/// A room the line saying what they are fills alone hands nothing and takes
/// nothing: the line still waits for a room it fits.
#[test]
fn a_room_too_small_for_the_opening_takes_nothing_and_the_line_waits() {
    let project = tempfile::tempdir().expect("a project");
    let (_endpoint, home, standing, guards) = a_line_waiting(project.path());
    let tight = custom::LINES_INTRO.chars().count() + 1;
    assert!(
        brief_in(
            guards,
            &standing.decided(),
            &brief_ask(&home.root, LAUNCH),
            tight
        )
        .is_none()
    );
    assert_eq!(
        brief(guards, &home, &standing, LAUNCH),
        Some(briefed(custom::line("Visible", YES_MEANS, 0.9)))
    );
}
