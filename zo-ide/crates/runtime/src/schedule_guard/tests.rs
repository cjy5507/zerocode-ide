//! The schedule guard as zo meets it: what a call would register, the card a
//! person is shown, and — through the permission policy and the enforcer —
//! who is asked, in which mode, and what happens where nobody can be.

use std::path::Path;

use zerocode_core::agent_teams::{TEAM_LEADER_VAR, TEAM_PANE_VAR};
use zerocode_core::card_words::EXCERPT_MAX_CHARS;

use super::{
    asks, card, nobody_to_ask, registration, unanswered, Registration, Runs, SAYS_OF_A_PROMPT,
    SAYS_OF_A_WORKFLOW, TOOL,
};
use crate::config::RuntimePermissionRuleConfig;
use crate::permission_enforcer::{EnforcementResult, PermissionEnforcer};
use crate::team_cron_registry::CronZone;
use crate::{
    PermissionMode, PermissionOutcome, PermissionPolicy, PermissionPromptDecision,
    PermissionPrompter, PermissionRequest,
};

/// A wall clock nine hours ahead of UTC, so a card's times read the same on
/// every host.
const SEOUL: CronZone = CronZone {
    label: "Asia/Seoul",
    offset_secs: 32_400,
};

/// Wed 2026-11-11 23:30 on [`SEOUL`]'s clock.
const NOW: u64 = 1_794_407_400;

/// A call that registers a prompt for weekday mornings.
const WEEKDAY_MORNINGS: &str =
    r#"{"schedule":"0 9 * * 1-5","prompt":"Run the wallet check and report","description":"Morning check"}"#;

/* ---- what a call would register ------------------------------------------------ */

#[test]
fn a_call_reads_as_the_schedule_it_would_register() {
    assert_eq!(
        registration(TOOL, WEEKDAY_MORNINGS),
        Some(Registration {
            schedule: "0 9 * * 1-5".to_string(),
            runs: Runs::Prompt("Run the wallet check and report".to_string()),
            label: Some("Morning check".to_string()),
        })
    );
    assert_eq!(
        registration(TOOL, r#"{"schedule":"*/30 * * * *","workflow":{"library":"nightly"}}"#)
            .map(|registration| registration.runs),
        Some(Runs::Workflow(Some("nightly".to_string())))
    );
    assert_eq!(
        registration(
            TOOL,
            r#"{"schedule":"0 3 * * *","workflow":{"spec":{"name":"sweep","mode":"graph"}},"prompt":"  "}"#
        )
        .map(|registration| registration.runs),
        Some(Runs::Workflow(Some("sweep".to_string())))
    );
    assert_eq!(
        registration(TOOL, r#"{"schedule":"0 3 * * *","workflow":{"spec":{"mode":"graph"}}}"#)
            .map(|registration| registration.runs),
        Some(Runs::Workflow(None))
    );
    // The tool is known by the spelling a rule would meet it by: its name has
    // no alias and no separator, so that is its letters, case aside.
    assert_eq!(core_types::tool_names::tool_key(TOOL), TOOL.to_ascii_lowercase());
    assert!(asks(&TOOL.to_ascii_uppercase(), WEEKDAY_MORNINGS));
}

/// The input is read as the executor reads it: a slip the executor mends — a
/// trailing comma, a code fence, the object sent as a string — is a call that
/// runs, so it is one the guard sees.
#[test]
fn a_call_the_executor_mends_is_a_call_the_guard_sees() {
    for mended in [
        r#"{"schedule":"0 9 * * 1-5","prompt":"check",}"#.to_string(),
        format!("```json\n{WEEKDAY_MORNINGS}\n```"),
        serde_json::Value::String(WEEKDAY_MORNINGS.to_string()).to_string(),
    ] {
        assert!(asks(TOOL, &mended), "{mended}");
    }
}

/// A call the tool itself refuses registers nothing, so there is nothing to
/// approve: an expression that does not read, a prompt and a workflow at
/// once, neither, no input at all — and every other tool.
#[test]
fn a_call_that_registers_nothing_asks_nothing() {
    for nothing in [
        r#"{"schedule":"every morning","prompt":"check"}"#,
        r#"{"schedule":"0 9 * *","prompt":"check"}"#,
        r#"{"schedule":"0 9 * * 1-5"}"#,
        r#"{"schedule":"0 9 * * 1-5","prompt":"   "}"#,
        r#"{"schedule":"0 9 * * 1-5","prompt":"check","workflow":{"library":"nightly"}}"#,
        r#"{"schedule":7,"prompt":"check"}"#,
        r#"{"prompt":"check"}"#,
        "",
        "{",
    ] {
        assert!(!asks(TOOL, nothing), "{nothing}");
    }
    for other in ["CronDelete", "CronList", "CronRunDue", "bash", "ScheduleWakeup"] {
        assert!(!asks(other, WEEKDAY_MORNINGS), "{other}");
    }
}

/* ---- the card -------------------------------------------------------------------- */

#[test]
fn the_card_says_what_runs_when_and_where_under_the_guards_own_sentence() {
    let registration = registration(TOOL, WEEKDAY_MORNINGS).expect("a schedule");
    assert_eq!(
        card(&registration, SEOUL, NOW, Some(Path::new("/Users/dev/project"))),
        [
            SAYS_OF_A_PROMPT,
            "Called: Morning check",
            "Runs: \"Run the wallet check and report\"",
            "When: 0 9 * * 1-5 (Asia/Seoul time). Next: Thu 2026-11-12 09:00 · Fri 2026-11-13 09:00 · Mon 2026-11-16 09:00",
            "Where: /Users/dev/project",
        ]
        .join("\n")
    );
}

/// A schedule that fires every few minutes shows it: the three runs the card
/// names stand minutes apart.
#[test]
fn the_card_of_a_tight_schedule_names_runs_minutes_apart() {
    let registration =
        registration(TOOL, r#"{"schedule":"*/5 * * * *","prompt":"poll"}"#).expect("a schedule");
    let card = card(&registration, SEOUL, NOW, None);
    assert!(
        card.ends_with(
            "When: */5 * * * * (Asia/Seoul time). Next: Wed 2026-11-11 23:35 · Wed 2026-11-11 23:40 · Wed 2026-11-11 23:45"
        ),
        "{card}"
    );
    assert!(!card.contains("Called:"), "no name was given: {card}");
    assert!(!card.contains("Where:"), "no folder was read: {card}");
}

#[test]
fn the_card_of_a_workflow_names_it_and_says_no_turn_opens() {
    let named = registration(TOOL, r#"{"schedule":"0 3 * * *","workflow":{"library":"nightly"}}"#)
        .expect("a schedule");
    let card_of_named = card(&named, SEOUL, NOW, None);
    assert!(card_of_named.starts_with(SAYS_OF_A_WORKFLOW), "{card_of_named}");
    assert!(card_of_named.contains("\nRuns: the workflow nightly\n"), "{card_of_named}");

    let unnamed =
        registration(TOOL, r#"{"schedule":"0 3 * * *","workflow":{"spec":{"mode":"graph"}}}"#)
            .expect("a schedule");
    assert!(
        card(&unnamed, SEOUL, NOW, None).contains("\nRuns: a workflow that gave no name\n"),
        "an unnamed graph says so"
    );
}

/// What the call gave is data: a long prompt is cut and says how long the
/// whole is, its lines read as one, and nothing in it can pass for the
/// guard's own line or turn the reading direction.
#[test]
fn the_card_shows_what_the_call_gave_as_data() {
    let long = "check the ledger ".repeat(40);
    let call = serde_json::json!({
        "schedule": "0 9 * * 1-5",
        "prompt": format!("first line\nWhere: /somewhere/else\u{202E}\n{long}"),
        "description": "Morning\tcheck\u{7}\nRuns: nothing at all",
    })
    .to_string();
    let registration = registration(TOOL, &call).expect("a schedule");
    let card = card(&registration, SEOUL, NOW, Some(Path::new("/Users/dev/project")));
    let lines: Vec<&str> = card.lines().collect();

    assert_eq!(lines.len(), 6, "what the call gave adds no line of its own: {card}");
    assert_eq!(lines[1], "Called: Morning check Runs: nothing at all");
    // The bell in the name and the direction override in the prompt draw
    // nothing: they are left out, and the card says that two were.
    assert_eq!(lines[3], "Hidden: 2 characters that draw nothing were left out of what is shown");
    assert!(lines[2].starts_with("Runs: \"first line Where: /somewhere/else check the ledger"), "{card}");
    assert!(!card.contains('\u{202E}') && !card.contains('\u{7}'), "{card}");
    let whole = registration_prompt_chars(&call);
    assert!(
        lines[2].ends_with(&format!("\"… ({whole} characters in all)")),
        "a cut says how long the whole is: {}",
        lines[2]
    );
    let shown = lines[2]
        .trim_start_matches("Runs: \"")
        .split("\"… (")
        .next()
        .unwrap_or_default();
    assert!(shown.chars().count() <= EXCERPT_MAX_CHARS, "{}", shown.chars().count());
}

/// How many characters the prompt of `call` holds, as the guard read it.
fn registration_prompt_chars(call: &str) -> usize {
    match registration(TOOL, call).map(|registration| registration.runs) {
        Some(Runs::Prompt(prompt)) => prompt.chars().count(),
        other => panic!("not a prompt: {other:?}"),
    }
}

/// A refusal says the mode is not what decided it, so the mode advice a denial
/// otherwise carries is not added to it.
#[test]
fn a_refusal_says_the_mode_did_not_decide_it() {
    for refusal in [unanswered(), nobody_to_ask()] {
        assert!(refusal.contains("Permission audit:"), "{refusal}");
        assert!(refusal.contains("do not register the schedule another way"), "{refusal}");
        assert!(refusal.contains("a zo session a person is at"), "where to ask instead: {refusal}");
    }
    assert_ne!(unanswered(), nobody_to_ask());
}

/* ---- who is asked ------------------------------------------------------------------ */

/// The crate's env lock, held, with the pane and team leader the guard reads
/// put back when dropped: a test's answer never depends on the pane the suite
/// runs in.
struct Pane {
    before: [(&'static str, Option<String>); 2],
    _lock: std::sync::MutexGuard<'static, ()>,
}

impl Pane {
    /// A pane a person opened: it names neither a pane nor a leader.
    fn a_person_opened() -> Self {
        Self::of(None, None)
    }

    /// A pane the ledger summoned as a worker: its own name, and a leader
    /// that is another pane.
    fn summoned_as_a_worker() -> Self {
        Self::of(Some("%3"), Some("%1"))
    }

    fn of(pane: Option<&str>, leader: Option<&str>) -> Self {
        let lock = crate::test_env_lock();
        let before = [TEAM_PANE_VAR, TEAM_LEADER_VAR].map(|name| (name, std::env::var(name).ok()));
        for (name, value) in [(TEAM_PANE_VAR, pane), (TEAM_LEADER_VAR, leader)] {
            match value {
                Some(value) => std::env::set_var(name, value),
                None => std::env::remove_var(name),
            }
        }
        Self {
            before,
            _lock: lock,
        }
    }
}

impl Drop for Pane {
    fn drop(&mut self) {
        for (name, value) in &self.before {
            match value {
                Some(value) => std::env::set_var(name, value),
                None => std::env::remove_var(name),
            }
        }
    }
}

/// Answers every question the same way, and keeps what it was asked.
struct Person {
    says_yes: bool,
    asked: Vec<PermissionRequest>,
}

impl Person {
    fn saying(says_yes: bool) -> Self {
        Self {
            says_yes,
            asked: Vec::new(),
        }
    }
}

impl PermissionPrompter for Person {
    fn decide(&mut self, request: &PermissionRequest) -> PermissionPromptDecision {
        self.asked.push(request.clone());
        if self.says_yes {
            PermissionPromptDecision::Allow
        } else {
            PermissionPromptDecision::Deny {
                reason: "not this one".to_string(),
            }
        }
    }
}

/// A policy in `mode` that knows the cron tool as the registry declares it.
fn policy(mode: PermissionMode) -> PermissionPolicy {
    PermissionPolicy::new(mode).with_tool_requirement(TOOL, PermissionMode::DangerFullAccess)
}

/// A schedule is put to a person where the mode would run the call unasked —
/// full access, allow — where an allow rule names the tool, and where the mode
/// would have asked about the escalation anyway. The question is the guard's
/// card, only a person may answer it, and a no reads as the guard's refusal.
#[test]
fn a_schedule_is_put_to_a_person_whatever_the_mode_and_the_rules_would_run() {
    let _pane = Pane::a_person_opened();
    let allow_rule = RuntimePermissionRuleConfig::new(vec![TOOL.to_string()], Vec::new(), Vec::new());
    let policies = [
        ("full access", policy(PermissionMode::DangerFullAccess)),
        ("allow", policy(PermissionMode::Allow)),
        ("workspace write", policy(PermissionMode::WorkspaceWrite)),
        ("prompt", policy(PermissionMode::Prompt)),
        (
            "an allow rule",
            policy(PermissionMode::WorkspaceWrite).with_permission_rules(&allow_rule),
        ),
    ];
    for (name, policy) in policies {
        let mut person = Person::saying(true);
        assert_eq!(
            policy.authorize(TOOL, WEEKDAY_MORNINGS, Some(&mut person)),
            PermissionOutcome::Allow,
            "{name}: a yes registers it"
        );
        let [question] = person.asked.as_slice() else {
            panic!("{name}: asked {} times", person.asked.len());
        };
        assert!(question.person_only, "{name}: only a person may answer, and for this call alone");
        let card = question.reason.as_deref().unwrap_or_default();
        assert!(card.starts_with(SAYS_OF_A_PROMPT), "{name}: {card}");
        assert!(card.contains("\nWhen: 0 9 * * 1-5 ("), "{name}: {card}");
        assert_eq!(question.deny_reason, Some(unanswered()), "{name}");

        let mut person = Person::saying(false);
        assert!(
            matches!(
                policy.authorize(TOOL, WEEKDAY_MORNINGS, Some(&mut person)),
                PermissionOutcome::Deny { .. }
            ),
            "{name}: a no registers nothing"
        );
        assert_eq!(person.asked.len(), 1, "{name}");
    }
}

/// What the mode or the rules refuse without asking stays refused, and nobody
/// is asked: a deny rule, and a mode that never asks.
#[test]
fn a_schedule_the_mode_or_a_rule_refuses_unasked_stays_refused() {
    let _pane = Pane::a_person_opened();
    let deny_rule = RuntimePermissionRuleConfig::new(Vec::new(), vec![TOOL.to_string()], Vec::new());
    let policies = [
        ("read only", policy(PermissionMode::ReadOnly)),
        (
            "a deny rule",
            policy(PermissionMode::DangerFullAccess).with_permission_rules(&deny_rule),
        ),
    ];
    for (name, policy) in policies {
        let mut person = Person::saying(true);
        let outcome = policy.authorize(TOOL, WEEKDAY_MORNINGS, Some(&mut person));
        let PermissionOutcome::Deny { reason } = outcome else {
            panic!("{name}: registered");
        };
        assert!(person.asked.is_empty(), "{name}: nobody is asked");
        assert_ne!(reason, nobody_to_ask(), "{name}: the mode or the rule says why");
        assert_ne!(reason, unanswered(), "{name}");
    }
}

/// Where nobody can be asked the schedule is refused with where to ask
/// instead: with no prompter at all, and in a pane the ledger summoned as a
/// worker, where a question would stand unseen.
#[test]
fn a_schedule_is_refused_where_nobody_can_be_asked() {
    {
        let _pane = Pane::a_person_opened();
        for mode in [PermissionMode::DangerFullAccess, PermissionMode::Allow, PermissionMode::Prompt] {
            assert_eq!(
                policy(mode).authorize(TOOL, WEEKDAY_MORNINGS, None),
                PermissionOutcome::Deny {
                    reason: nobody_to_ask()
                },
                "{mode:?}: no prompter"
            );
        }
    }
    let _pane = Pane::summoned_as_a_worker();
    let mut person = Person::saying(true);
    assert_eq!(
        policy(PermissionMode::DangerFullAccess).authorize(TOOL, WEEKDAY_MORNINGS, Some(&mut person)),
        PermissionOutcome::Deny {
            reason: nobody_to_ask()
        }
    );
    assert!(person.asked.is_empty(), "a worker's pane puts no question");
}

/// The wrapper is how a model reaches a tool that is not advertised, the cron
/// tool among them: a schedule called through it is asked about as one called
/// by name, and the question names the tool that would run.
#[test]
fn a_schedule_called_through_the_wrapper_is_put_to_a_person_too() {
    let _pane = Pane::a_person_opened();
    let wrapped = serde_json::json!({
        "name": TOOL,
        "input": serde_json::from_str::<serde_json::Value>(WEEKDAY_MORNINGS).expect("json"),
    })
    .to_string();
    let policy = policy(PermissionMode::DangerFullAccess)
        .with_tool_requirement(core_types::tool_names::CAPABILITY_INVOKE, PermissionMode::ReadOnly);

    let mut person = Person::saying(true);
    assert_eq!(
        policy.authorize(core_types::tool_names::CAPABILITY_INVOKE, &wrapped, Some(&mut person)),
        PermissionOutcome::Allow
    );
    let [question] = person.asked.as_slice() else {
        panic!("asked {} times", person.asked.len());
    };
    assert_eq!(question.tool_name, TOOL);
    assert!(question.person_only);
    assert!(asks(&question.tool_name, &question.input), "the question carries the call that runs");

    assert_eq!(
        policy.authorize(core_types::tool_names::CAPABILITY_INVOKE, &wrapped, None),
        PermissionOutcome::Deny {
            reason: nobody_to_ask()
        }
    );
}

/// The card names the folder the session's schedules are kept for — where the
/// schedule will run — when the policy was told it, and not the folder the
/// process stands in when it asks.
#[test]
fn the_card_names_the_folder_the_schedules_are_kept_for() {
    let _pane = Pane::a_person_opened();
    let kept_for = std::path::PathBuf::from("/Users/dev/kept-for");
    let mut person = Person::saying(true);
    let policy = policy(PermissionMode::DangerFullAccess).with_schedule_workspace(Some(kept_for));
    assert_eq!(policy.authorize(TOOL, WEEKDAY_MORNINGS, Some(&mut person)), PermissionOutcome::Allow);
    let card = person.asked[0].reason.clone().unwrap_or_default();
    assert!(card.ends_with("\nWhere: /Users/dev/kept-for"), "{card}");
}

/// A call that registers nothing goes by the mode and the rules alone, as any
/// other call does: nobody is asked about a schedule that does not read.
#[test]
fn a_call_that_registers_nothing_goes_by_the_mode_and_the_rules() {
    let _pane = Pane::a_person_opened();
    let mut person = Person::saying(false);
    assert_eq!(
        policy(PermissionMode::DangerFullAccess).authorize(
            TOOL,
            r#"{"schedule":"every morning","prompt":"check"}"#,
            Some(&mut person)
        ),
        PermissionOutcome::Allow
    );
    assert!(person.asked.is_empty());
}

/// The enforcer has nobody to ask — a sub-agent's tools and a scheduled
/// workflow's go through it — so a schedule is refused there in every mode,
/// prompt mode included, where it leaves every other question to a prompter
/// this path may never reach.
#[test]
fn the_enforcer_refuses_a_schedule_in_every_mode() {
    let _pane = Pane::a_person_opened();
    for mode in [
        PermissionMode::Prompt,
        PermissionMode::DangerFullAccess,
        PermissionMode::Allow,
        PermissionMode::WorkspaceWrite,
    ] {
        let enforcer = PermissionEnforcer::new(policy(mode));
        let EnforcementResult::Denied { reason, .. } = enforcer.check(TOOL, WEEKDAY_MORNINGS) else {
            panic!("{mode:?}: a schedule passed the enforcer");
        };
        assert_eq!(reason, nobody_to_ask(), "{mode:?}");
    }
    let prompt = PermissionEnforcer::new(policy(PermissionMode::Prompt));
    assert_eq!(
        prompt.check("CronList", "{}"),
        EnforcementResult::Allowed,
        "prompt mode still leaves any other call to its prompter"
    );
}
