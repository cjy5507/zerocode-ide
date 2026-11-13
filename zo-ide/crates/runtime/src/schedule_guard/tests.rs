//! The schedule guard as zo meets it: what a call would register, the card a
//! person is shown, and — through the permission policy and the enforcer —
//! who is asked, in which mode, and what happens where nobody can be.

use std::path::Path;

use zerocode_core::agent_teams::{TEAM_LEADER_VAR, TEAM_PANE_VAR};
use zerocode_core::card_words::EXCERPT_MAX_CHARS;

use super::{
    asks, card, deletion_card, refused_in_a_shell, registration, Act, Registration, Runs, DELETE_TOOL,
    LIST_TOOL, NOT_FOUND_HERE, SAYS_OF_A_DELETION, SAYS_OF_A_PROMPT, SAYS_OF_A_WORKFLOW, STORE_DID_NOT_READ, TOOL,
};
use crate::config::RuntimePermissionRuleConfig;
use crate::permission_enforcer::{EnforcementResult, PermissionEnforcer};
use crate::team_cron_registry::{CronEntry, CronPeek, CronRegistry, CronZone};
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
    for other in [LIST_TOOL, "CronRunDue", "bash", "ScheduleWakeup"] {
        assert!(!asks(other, WEEKDAY_MORNINGS), "{other}");
    }
    // A deletion is asked about by the id it names; one that names none is
    // the tool's to refuse ([`a_deletion_is_asked_about_by_the_id_it_names`]).
    assert!(!asks(DELETE_TOOL, WEEKDAY_MORNINGS));
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

/// A schedule as tight as one zo may ask for shows it: the three runs the
/// card names stand a quarter of an hour apart, into the next day.
#[test]
fn the_card_of_a_tight_schedule_names_runs_minutes_apart() {
    let registration =
        registration(TOOL, r#"{"schedule":"*/15 * * * *","prompt":"poll"}"#).expect("a schedule");
    let card = card(&registration, SEOUL, NOW, None);
    assert!(
        card.ends_with(
            "When: */15 * * * * (Asia/Seoul time). Next: Wed 2026-11-11 23:45 · Thu 2026-11-12 00:00 · Thu 2026-11-12 00:15"
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
    for (act, verb) in [(Act::Registers, "register"), (Act::Deletes, "delete")] {
        for refusal in [act.unanswered(), act.nobody_to_ask()] {
            assert!(refusal.contains("Permission audit:"), "{refusal}");
            assert!(refusal.contains(&format!("do not {verb} the schedule another way")), "{refusal}");
            assert!(refusal.contains("a zo session a person is at"), "where to ask instead: {refusal}");
            assert!(!refusal.contains("  "), "the sentence runs on, with no gap in it: {refusal:?}");
        }
        assert_ne!(act.unanswered(), act.nobody_to_ask());
    }
    assert_ne!(Act::Registers.unanswered(), Act::Deletes.unanswered(), "each says what it refuses");
    assert_ne!(Act::Registers.nobody_to_ask(), Act::Deletes.nobody_to_ask());
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
        assert_eq!(question.deny_reason, Some(Act::Registers.unanswered()), "{name}");

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
        assert_ne!(reason, Act::Registers.nobody_to_ask(), "{name}: the mode or the rule says why");
        assert_ne!(reason, Act::Registers.unanswered(), "{name}");
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
                    reason: Act::Registers.nobody_to_ask()
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
            reason: Act::Registers.nobody_to_ask()
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
            reason: Act::Registers.nobody_to_ask()
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

/// A call that would register nothing — an expression that does not read, a
/// prompt and a workflow at once, runs closer together than the floor — is
/// refused with why before the mode and the rules are asked: in a mode that
/// would have asked about the tool, a person is not put a question whose yes
/// registers nothing. With a prompter, without one, and through the wrapper.
#[test]
fn a_call_that_would_register_nothing_is_refused_with_why_and_nobody_is_asked() {
    let _pane = Pane::a_person_opened();
    let nothing = [
        r#"{"schedule":"every morning","prompt":"check"}"#,
        r#"{"schedule":"0 9 * * 1-5","prompt":"check","workflow":{"library":"nightly"}}"#,
        r#"{"schedule":"0 9 * * 1-5"}"#,
        r#"{"schedule":"*/5 * * * *","prompt":"poll"}"#,
    ];
    for mode in [
        PermissionMode::Prompt,
        PermissionMode::WorkspaceWrite,
        PermissionMode::DangerFullAccess,
    ] {
        let policy = policy(mode)
            .with_tool_requirement(core_types::tool_names::CAPABILITY_INVOKE, PermissionMode::ReadOnly);
        for input in nothing {
            let why = serde_json::from_str::<crate::team_cron_registry::CronCreateInput>(input)
                .expect("the call's shape")
                .work()
                .expect_err("registers nothing");
            let refusal = format!("refused, since it would register nothing: {why}. {}", Act::Registers.not_the_mode());
            let wrapped = serde_json::json!({
                "name": TOOL,
                "input": serde_json::from_str::<serde_json::Value>(input).expect("json"),
            })
            .to_string();
            for (tool, input) in [(TOOL, input), (core_types::tool_names::CAPABILITY_INVOKE, wrapped.as_str())] {
                let mut person = Person::saying(true);
                assert_eq!(
                    policy.authorize(tool, input, Some(&mut person)),
                    PermissionOutcome::Deny {
                        reason: refusal.clone()
                    },
                    "{mode:?} {tool} {input}"
                );
                assert!(person.asked.is_empty(), "{mode:?} {tool}: a yes would register nothing, so nobody is asked");
                assert_eq!(
                    policy.authorize(tool, input, None),
                    PermissionOutcome::Deny {
                        reason: refusal.clone()
                    },
                    "{mode:?} {tool}: with nobody to ask"
                );
            }
            assert!(!refusal.contains("  "), "{refusal:?}");
        }
    }
    // An input the tool cannot read at all is the tool's to answer: it goes
    // by the mode and the rules, as any other call does.
    let mut person = Person::saying(false);
    assert_eq!(
        policy(PermissionMode::DangerFullAccess).authorize(TOOL, r#"{"prompt":"check"}"#, Some(&mut person)),
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
        assert_eq!(reason, Act::Registers.nobody_to_ask(), "{mode:?}");
    }
    let prompt = PermissionEnforcer::new(policy(PermissionMode::Prompt));
    assert_eq!(
        prompt.check(LIST_TOOL, "{}"),
        EnforcementResult::Allowed,
        "prompt mode still leaves any other call to its prompter"
    );
}

/* ---- the floor --------------------------------------------------------------------- */

/// A schedule whose runs come closer together than the floor registers
/// nothing, so the guard has no card for it — and the policy refuses it in
/// the floor's words, whatever the mode. One at the floor is a person's to
/// approve.
#[test]
fn a_schedule_under_the_floor_registers_nothing_and_is_refused_in_the_floors_words() {
    let _pane = Pane::a_person_opened();
    for (tight, gap) in [
        (r#"{"schedule":"* * * * *","prompt":"poll"}"#, 1),
        (r#"{"schedule":"*/5 * * * *","prompt":"poll"}"#, 5),
        (r#"{"schedule":"0,1 9 * * *","prompt":"poll"}"#, 1),
        (r#"{"schedule":"*/10 * * * *","workflow":{"library":"nightly"}}"#, 10),
    ] {
        assert_eq!(registration(TOOL, tight), None, "{tight}");
        assert!(!asks(TOOL, tight), "{tight}");
        let mut person = Person::saying(true);
        let PermissionOutcome::Deny { reason } =
            policy(PermissionMode::DangerFullAccess).authorize(TOOL, tight, Some(&mut person))
        else {
            panic!("{tight}: a schedule under the floor was let through");
        };
        assert!(
            reason.contains(&format!(
                "runs no closer together than 15 minutes; this one has runs {gap} minutes apart."
            )),
            "{reason}"
        );
        assert!(person.asked.is_empty(), "{tight}: nothing would be registered");
    }
    assert!(
        asks(TOOL, r#"{"schedule":"*/15 * * * *","prompt":"poll"}"#),
        "a schedule at the floor is a person's to approve"
    );
}

/* ---- the shell is not a second road -------------------------------------------------- */

/// What `zo cron ensure` is handed, after the verb.
const ENSURED: &str = "--description nightly --schedule '0 3 * * *' --prompt 'sweep the logs'";

/// A shell command that would register a schedule, in the ways a shell runs
/// one: by name, by path, behind assignments and wrappers, in a chain, in a
/// string handed to a shell, fed its prompt on a pipe.
fn registering_commands() -> Vec<String> {
    vec![
        format!("zo cron ensure {ENSURED}"),
        format!("/Users/dev/.local/bin/zo cron ensure {ENSURED}"),
        format!("ZO cron ensure {ENSURED}"),
        format!("zo.exe cron ensure {ENSURED}"),
        format!("ZO_STATE_DIR=/tmp/elsewhere zo cron ensure {ENSURED}"),
        format!("env -u ZO_SESSION nice -n 5 zo cron ensure {ENSURED}"),
        format!("cd /Users/dev/work && zo cron ensure {ENSURED} --cwd /Users/dev/work"),
        format!("echo start; zo cron ensure {ENSURED} | tee receipt.json"),
        "bash -c \"zo cron ensure --description nightly --schedule '0 3 * * *' --prompt sweep\"".to_string(),
        "eval zo cron ensure --description nightly --schedule \"'0 3 * * *'\" --prompt sweep".to_string(),
        "echo sweep | zo cron ensure --description nightly --schedule '0 3 * * *' --prompt-file -".to_string(),
        "zo cron ensure --description nightly --schedule '0 3 * * *' --prompt-file plan.md".to_string(),
        "echo \"$(zo cron ensure --description nightly --schedule '0 3 * * *' --prompt sweep)\"".to_string(),
    ]
}

/// The refusal reads the command by the kill guard's reading, so it finds the
/// call wherever the shell would run it, and says the tool to ask through.
#[test]
fn a_shell_command_that_registers_a_schedule_is_refused_wherever_the_shell_would_run_it() {
    for command in registering_commands() {
        let refused = refused_in_a_shell(&command).unwrap_or_else(|| panic!("passed: {command}"));
        assert!(
            refused.starts_with(
                "refused to run: this command registers a schedule from a shell (`zo cron ensure`), past the question a person answers. Ask for the schedule with the CronCreate tool (`schedule`, `prompt` or `workflow`, `description`): it puts the schedule to a person. Permission audit:"
            ),
            "{refused}"
        );
        assert!(!refused.contains("  "), "the sentence runs on, with no gap in it: {refused:?}");
        assert!(refused.contains(&format!("with the {TOOL} tool")), "{refused}");
        assert!(refused.contains("Permission audit:"), "{refused}");
        assert!(refused.contains("do not register the schedule another way"), "{refused}");
    }
}

/// Under `zo cron`, only the verbs that register nothing pass: a verb the
/// command leaves to run time, or one this guard does not know, is refused
/// as the one that registers — and the refusal says so, not `ensure`.
#[test]
fn a_cron_verb_the_command_does_not_spell_out_is_refused_as_the_one_that_registers() {
    for command in [
        "zo cron \"$VERB\" --description nightly --schedule '0 3 * * *' --prompt sweep",
        "zo cron $(cat verb.txt) --description nightly",
        "zo cron --description nightly --schedule '0 3 * * *' --prompt sweep",
        "zo cron replace --description nightly",
    ] {
        let refused = refused_in_a_shell(command).unwrap_or_else(|| panic!("passed: {command}"));
        assert!(
            refused.contains("(`zo cron` with a verb the command does not spell out as `show`, which only reads)"),
            "{refused}"
        );
    }
}

/// What the reading does not reach passes, as the module says. A verb the
/// shell fills in is refused at the top of a command; inside a string handed
/// to a shell the reading drops it and takes the next word for the verb, so
/// it is refused when a word follows that registers something to the guard's
/// eye and passes when none follows. The word `cron` split by quotes or a
/// backslash, and zo under a name the shell fills in, pass.
#[test]
fn the_kinds_the_reading_does_not_reach_pass_as_the_module_says() {
    let rest = "--description nightly --schedule '0 3 * * *' --prompt sweep";
    assert!(refused_in_a_shell(&format!("zo cron \"$VERB\" {rest}")).is_some(), "at the top of a command");
    assert!(
        refused_in_a_shell(&format!("bash -c \"zo cron $VERB {rest}\"")).is_some(),
        "in a shell's string, the word after the gap is read as the verb"
    );
    for passes in [
        "bash -c \"zo cron $VERB\"".to_string(),
        format!("zo cr\"on\" ensure {rest}"),
        format!("zo c\\ron ensure {rest}"),
        format!("\"$ZO\" cron ensure {rest}"),
    ] {
        assert_eq!(refused_in_a_shell(&passes), None, "{passes}");
    }
}

/// A command that writes nothing is not the guard's: the verb that reads the
/// store, `zo cron` alone, another `zo` verb, the
/// words as an argument of something else, and a program that only shares
/// the word.
#[test]
fn a_shell_command_that_registers_nothing_passes() {
    for command in [
        "zo cron show --description nightly",
        "zo cron",
        "zo cron --help",
        "zo cron -h",
        "zo vault search cron ensure",
        "zo line add -- cargo test cron",
        "echo zo cron ensure --description nightly",
        "grep -rn 'zo cron ensure' README.md",
        "git commit -m 'zo cron ensure asks a person'",
        "crontab -l",
        "cat .zo/registries/crons.json",
        "ls -la",
    ] {
        assert_eq!(refused_in_a_shell(command), None, "{command}");
    }
}

/// The policy refuses such a command in every mode, under an allow rule, and
/// whoever could be asked: nobody is, since the question a schedule is put to
/// a person by is the tool's. The other `zo cron` verbs go by the mode.
#[test]
fn the_policy_refuses_a_shell_command_that_registers_a_schedule_in_every_mode_and_asks_nobody() {
    let _pane = Pane::a_person_opened();
    let command = format!("zo cron ensure {ENSURED}");
    let call = serde_json::json!({ "command": command }).to_string();
    let refusal = refused_in_a_shell(&command).expect("a refusal");
    let allow_rule = RuntimePermissionRuleConfig::new(vec!["bash".to_string()], Vec::new(), Vec::new());
    let policies = [
        ("full access", PermissionPolicy::new(PermissionMode::DangerFullAccess)),
        ("allow", PermissionPolicy::new(PermissionMode::Allow)),
        ("workspace write", PermissionPolicy::new(PermissionMode::WorkspaceWrite)),
        ("prompt", PermissionPolicy::new(PermissionMode::Prompt)),
        ("read only", PermissionPolicy::new(PermissionMode::ReadOnly)),
        (
            "an allow rule",
            PermissionPolicy::new(PermissionMode::WorkspaceWrite).with_permission_rules(&allow_rule),
        ),
    ];
    for (name, policy) in policies {
        for tool in ["bash", "Bash", "PowerShell"] {
            let mut person = Person::saying(true);
            assert_eq!(
                policy.authorize(tool, &call, Some(&mut person)),
                PermissionOutcome::Deny {
                    reason: refusal.clone()
                },
                "{name}, {tool}"
            );
            assert!(person.asked.is_empty(), "{name}, {tool}: a refusal is not a question");
            assert_eq!(
                policy.authorize(tool, &call, None),
                PermissionOutcome::Deny {
                    reason: refusal.clone()
                },
                "{name}, {tool}: with nobody to ask"
            );
        }
    }
    let shown = serde_json::json!({ "command": "zo cron show --description nightly" }).to_string();
    assert_eq!(
        PermissionPolicy::new(PermissionMode::DangerFullAccess).authorize("bash", &shown, None),
        PermissionOutcome::Allow,
        "reading the store is the mode's to decide"
    );
}

/// The wrapper grants nothing: a shell command it carries is refused as one
/// called by name.
#[test]
fn a_shell_command_that_registers_a_schedule_is_refused_through_the_wrapper_too() {
    let _pane = Pane::a_person_opened();
    let command = format!("zo cron ensure {ENSURED}");
    let wrapped = serde_json::json!({ "name": "Bash", "input": { "command": command } }).to_string();
    let policy = PermissionPolicy::new(PermissionMode::DangerFullAccess)
        .with_tool_requirement(core_types::tool_names::CAPABILITY_INVOKE, PermissionMode::ReadOnly);
    let mut person = Person::saying(true);
    assert_eq!(
        policy.authorize(core_types::tool_names::CAPABILITY_INVOKE, &wrapped, Some(&mut person)),
        PermissionOutcome::Deny {
            reason: refused_in_a_shell(&command).expect("a refusal")
        }
    );
    assert!(person.asked.is_empty());
}

/// The enforcer — a sub-agent's commands, a scheduled workflow's, a command
/// file's `!` lines — refuses it in every mode, and in a pane the ledger
/// summoned as a worker.
#[test]
fn the_enforcer_refuses_a_shell_command_that_registers_a_schedule_in_every_mode() {
    let command = format!("zo cron ensure {ENSURED}");
    let refusal = refused_in_a_shell(&command).expect("a refusal");
    for pane in [Pane::a_person_opened, Pane::summoned_as_a_worker] {
        let _pane = pane();
        for mode in [
            PermissionMode::Prompt,
            PermissionMode::DangerFullAccess,
            PermissionMode::Allow,
            PermissionMode::WorkspaceWrite,
            PermissionMode::ReadOnly,
        ] {
            let enforcer = PermissionEnforcer::new(PermissionPolicy::new(mode));
            let EnforcementResult::Denied { reason, .. } = enforcer.check_bash(&command) else {
                panic!("{mode:?}: the command passed the enforcer");
            };
            assert_eq!(reason, refusal, "{mode:?}");
        }
    }
    let _pane = Pane::a_person_opened();
    assert_eq!(
        PermissionEnforcer::new(PermissionPolicy::new(PermissionMode::DangerFullAccess))
            .check_bash("zo cron show --description nightly"),
        EnforcementResult::Allowed,
        "reading the store writes nothing"
    );
}

/* ---- deleting a schedule ------------------------------------------------------------ */

/// A `CronDelete` call for `cron_id`.
fn deleting(cron_id: &str) -> String {
    serde_json::json!({ "cron_id": cron_id }).to_string()
}

/// A folder whose store holds one schedule, put there as the person's road
/// puts one, and its id.
fn a_folder_with_a_schedule() -> (tempfile::TempDir, String) {
    let folder = tempfile::tempdir().expect("tempdir");
    let entry = CronRegistry::for_workspace(folder.path())
        .create("0 9 * * 1", "say hello to the team", Some("Monday hello"))
        .expect("a schedule");
    (folder, entry.cron_id)
}

/// A policy in `mode` that knows the deleting tool as the registry declares
/// it, told the folder the session's schedules are kept for.
fn deleting_policy(mode: PermissionMode, folder: &Path) -> PermissionPolicy {
    PermissionPolicy::new(mode)
        .with_tool_requirement(DELETE_TOOL, PermissionMode::DangerFullAccess)
        .with_schedule_workspace(Some(folder.to_path_buf()))
}

/// A deletion is a person's to approve by the id it names, read as the
/// executor reads it; one that names none is the tool's to refuse.
#[test]
fn a_deletion_is_asked_about_by_the_id_it_names() {
    let call = deleting("cron_1");
    assert_eq!(Act::of(DELETE_TOOL, &call), Some(Act::Deletes));
    assert!(asks(&DELETE_TOOL.to_ascii_lowercase(), &call), "case aside");
    for mended in [
        r#"{"cron_id":"cron_1",}"#.to_string(),
        format!("```json\n{call}\n```"),
        serde_json::Value::String(call.clone()).to_string(),
    ] {
        assert!(asks(DELETE_TOOL, &mended), "{mended}");
    }
    for nothing in ["{}", r#"{"cron_id":7}"#, "", "{"] {
        assert_eq!(Act::of(DELETE_TOOL, nothing), None, "{nothing}");
    }
    assert_eq!(Act::of(TOOL, WEEKDAY_MORNINGS), Some(Act::Registers));
    assert_eq!(Act::of(LIST_TOOL, &call), None);
}

/// The card says, under the guard's sentence, the schedule as the store holds
/// it: its name, what it runs, when, how many times it ran, and where.
#[test]
fn the_deletion_card_says_what_the_schedule_runs_when_how_often_and_where() {
    let mut entry = CronEntry {
        cron_id: "cron_1".to_string(),
        schedule: "0 9 * * 1-5".to_string(),
        prompt: "Run the wallet check and report".to_string(),
        workflow: None,
        workflow_active_at: None,
        workflow_blocked_reason: None,
        description: Some("Morning check".to_string()),
        enabled: true,
        created_at: NOW - 3 * 86_400,
        updated_at: NOW,
        // Wed 2026-11-11 09:00 on SEOUL's clock.
        last_run_at: Some(NOW - 52_200),
        run_count: 2,
        rev: 1,
    };
    assert_eq!(
        deletion_card("cron_1", &CronPeek::Stands(Box::new(entry.clone())), SEOUL, NOW, Some(Path::new("/Users/dev/project"))),
        [
            SAYS_OF_A_DELETION,
            "Schedule: cron_1",
            "Called: Morning check",
            "Runs: \"Run the wallet check and report\"",
            "When: 0 9 * * 1-5 (Asia/Seoul time). Next: Thu 2026-11-12 09:00 · Fri 2026-11-13 09:00 · Mon 2026-11-16 09:00",
            "Ran: 2 times, last at Wed 2026-11-11 09:00",
            "Where: /Users/dev/project",
        ]
        .join("\n")
    );
    let stands = |entry: &CronEntry| CronPeek::Stands(Box::new(entry.clone()));
    entry.run_count = 1;
    assert!(deletion_card("cron_1", &stands(&entry), SEOUL, NOW, None).contains("\nRan: once, at Wed 2026-11-11 09:00"));
    entry.last_run_at = None;
    assert!(deletion_card("cron_1", &stands(&entry), SEOUL, NOW, None).ends_with("\nRan: once"));
    entry.run_count = 0;
    entry.last_run_at = None;
    entry.prompt = String::new();
    entry.workflow = Some(serde_json::json!({ "library": "nightly" }));
    let workflow = deletion_card("cron_1", &stands(&entry), SEOUL, NOW, None);
    assert!(workflow.contains("\nRuns: the workflow nightly\n"), "{workflow}");
    assert!(workflow.ends_with("\nRan: never"), "{workflow}");
}

/// An id the guard's look did not find is said as data, with that it was not
/// found here and that the card cannot show what a yes deletes: what the call
/// gave adds no line of its own.
#[test]
fn the_deletion_card_of_an_id_not_found_says_so() {
    let card = deletion_card("cron_9\u{202E}\nWhere: /elsewhere", &CronPeek::NotHere, SEOUL, NOW, Some(Path::new("/Users/dev/project")));
    assert_eq!(
        card,
        [
            SAYS_OF_A_DELETION,
            "Schedule: cron_9 Where: /elsewhere",
            "Hidden: 1 character that draws nothing was left out of what is shown",
            NOT_FOUND_HERE,
            "Where: /Users/dev/project",
        ]
        .join("\n")
    );
}

/// A deletion is put to a person where the mode would run the call unasked,
/// where an allow rule names the tool, and where the mode would have asked
/// anyway: the question is the deletion's card, only a person may answer it,
/// and a no reads as the guard's refusal.
#[test]
fn a_deletion_is_put_to_a_person_whatever_the_mode_and_the_rules_would_run() {
    let _pane = Pane::a_person_opened();
    let (folder, cron_id) = a_folder_with_a_schedule();
    let call = deleting(&cron_id);
    let allow_rule = RuntimePermissionRuleConfig::new(vec![DELETE_TOOL.to_string()], Vec::new(), Vec::new());
    let policies = [
        ("full access", deleting_policy(PermissionMode::DangerFullAccess, folder.path())),
        ("allow", deleting_policy(PermissionMode::Allow, folder.path())),
        ("workspace write", deleting_policy(PermissionMode::WorkspaceWrite, folder.path())),
        ("prompt", deleting_policy(PermissionMode::Prompt, folder.path())),
        (
            "an allow rule",
            deleting_policy(PermissionMode::WorkspaceWrite, folder.path()).with_permission_rules(&allow_rule),
        ),
    ];
    for (name, policy) in policies {
        let mut person = Person::saying(true);
        assert_eq!(policy.authorize(DELETE_TOOL, &call, Some(&mut person)), PermissionOutcome::Allow, "{name}");
        let [question] = person.asked.as_slice() else {
            panic!("{name}: asked {} times", person.asked.len());
        };
        assert!(question.person_only, "{name}: only a person may answer, and for this call alone");
        let card = question.reason.as_deref().unwrap_or_default();
        assert!(card.starts_with(&format!("{SAYS_OF_A_DELETION}\nSchedule: {cron_id}\n")), "{name}: {card}");
        assert!(card.contains("\nCalled: Monday hello\nRuns: \"say hello to the team\"\n"), "{name}: {card}");
        assert!(card.contains("\nRan: never\n"), "{name}: {card}");
        assert_eq!(question.deny_reason, Some(Act::Deletes.unanswered()), "{name}");

        let mut person = Person::saying(false);
        assert!(
            matches!(policy.authorize(DELETE_TOOL, &call, Some(&mut person)), PermissionOutcome::Deny { .. }),
            "{name}: a no deletes nothing"
        );
        assert_eq!(person.asked.len(), 1, "{name}");
    }
}

/// Whether the guard finds the id is not what decides the asking: the tool
/// deletes from its own store, which a policy told no folder may not be the
/// one the guard read. An id found nowhere is put to a person all the same,
/// saying so, and refused where nobody can be asked.
#[test]
fn a_deletion_the_guard_does_not_find_is_asked_about_all_the_same() {
    let _pane = Pane::a_person_opened();
    let empty = tempfile::tempdir().expect("tempdir");
    let call = deleting("cron_elsewhere_1");
    for mode in [
        PermissionMode::DangerFullAccess,
        PermissionMode::Allow,
        PermissionMode::WorkspaceWrite,
        PermissionMode::Prompt,
    ] {
        let policy = deleting_policy(mode, empty.path());
        let mut person = Person::saying(false);
        assert!(matches!(policy.authorize(DELETE_TOOL, &call, Some(&mut person)), PermissionOutcome::Deny { .. }));
        let [question] = person.asked.as_slice() else {
            panic!("{mode:?}: asked {} times", person.asked.len());
        };
        let card = question.reason.as_deref().unwrap_or_default();
        assert!(card.contains(&format!("\nSchedule: cron_elsewhere_1\n{NOT_FOUND_HERE}\n")), "{mode:?}: {card}");
        assert_eq!(
            policy.authorize(DELETE_TOOL, &call, None),
            PermissionOutcome::Deny {
                reason: Act::Deletes.nobody_to_ask()
            },
            "{mode:?}"
        );
    }
}

/// A store whose file is there and does not read says nothing of what it
/// holds: the card says the store did not read and that it cannot show what
/// a yes deletes — not that no schedule stands — while a folder with no store
/// file holds none.
#[test]
fn a_store_that_does_not_read_is_said_so_on_the_card() {
    let _pane = Pane::a_person_opened();
    let (folder, cron_id) = a_folder_with_a_schedule();
    let store = folder.path().join(".zo").join("registries").join("crons.json");
    std::fs::write(&store, "{ \"entries\": { not json").expect("a store that does not read");
    let mut person = Person::saying(false);
    let _ = deleting_policy(PermissionMode::DangerFullAccess, folder.path())
        .authorize(DELETE_TOOL, &deleting(&cron_id), Some(&mut person));
    let card = person.asked.first().and_then(|question| question.reason.clone()).unwrap_or_default();
    assert!(card.contains(&format!("\n{STORE_DID_NOT_READ}\n")), "{card}");
    assert!(!card.contains(NOT_FOUND_HERE), "{card}");
    assert!(matches!(CronRegistry::peek_in_workspace(folder.path(), &cron_id), CronPeek::DidNotRead));
    let empty = tempfile::tempdir().expect("tempdir");
    assert!(matches!(CronRegistry::peek_in_workspace(empty.path(), &cron_id), CronPeek::NotHere));
}

/// The guard's look at the store writes nothing: a store holding an entry
/// that does not read — one opening the registry would switch off and save —
/// is the same bytes after the card is made.
#[test]
fn the_guards_look_at_the_store_writes_nothing() {
    let _pane = Pane::a_person_opened();
    let folder = tempfile::tempdir().expect("tempdir");
    let store = folder.path().join(".zo").join("registries").join("crons.json");
    std::fs::create_dir_all(store.parent().expect("a parent")).expect("the store's folder");
    let entry = |id: &str, schedule: &str, name: &str| {
        serde_json::json!({
            "cron_id": id, "schedule": schedule, "prompt": "say hello", "description": name,
            "enabled": true, "created_at": NOW, "updated_at": NOW, "last_run_at": null, "run_count": 0, "rev": 1,
        })
    };
    let held = serde_json::json!({
        "entries": { "cron_ok": entry("cron_ok", "0 9 * * 1", "kept"), "cron_bad": entry("cron_bad", "not a schedule", "broken") },
        "counter": 2, "revision": 2, "tombstones": {},
    })
    .to_string();
    std::fs::write(&store, &held).expect("the store");

    let mut person = Person::saying(false);
    let _ = deleting_policy(PermissionMode::DangerFullAccess, folder.path())
        .authorize(DELETE_TOOL, &deleting("cron_ok"), Some(&mut person));
    let card = person.asked.first().and_then(|question| question.reason.clone()).unwrap_or_default();
    assert!(card.contains("\nCalled: kept\n"), "the card read the store: {card}");
    assert_eq!(std::fs::read_to_string(&store).expect("the store"), held, "the look wrote nothing");

    // The store is one that opening the registry rewrites, so the bytes above
    // are a look that did not open it.
    let _ = CronRegistry::for_workspace(folder.path());
    assert_ne!(std::fs::read_to_string(&store).expect("the store"), held);
}

/// What the mode or the rules refuse without asking stays refused, and nobody
/// is asked; where nobody can be asked — no prompter, a pane the ledger
/// summoned as a worker, the enforcer a sub-agent's and a workflow's tools go
/// through — a deletion is refused in its own words. That a refusal deletes
/// nothing is the turn's to show (`conversation::tests`): no tool runs here.
#[test]
fn a_deletion_is_refused_where_nobody_can_be_asked() {
    let (folder, cron_id) = a_folder_with_a_schedule();
    let call = deleting(&cron_id);
    let refused = PermissionOutcome::Deny {
        reason: Act::Deletes.nobody_to_ask(),
    };
    {
        let _pane = Pane::a_person_opened();
        let deny_rule = RuntimePermissionRuleConfig::new(Vec::new(), vec![DELETE_TOOL.to_string()], Vec::new());
        for policy in [
            deleting_policy(PermissionMode::ReadOnly, folder.path()),
            deleting_policy(PermissionMode::DangerFullAccess, folder.path()).with_permission_rules(&deny_rule),
        ] {
            let mut person = Person::saying(true);
            assert!(matches!(policy.authorize(DELETE_TOOL, &call, Some(&mut person)), PermissionOutcome::Deny { .. }));
            assert!(person.asked.is_empty(), "the mode or the rule refuses it unasked");
        }
        for mode in [
            PermissionMode::DangerFullAccess,
            PermissionMode::Allow,
            PermissionMode::WorkspaceWrite,
            PermissionMode::Prompt,
        ] {
            assert_eq!(deleting_policy(mode, folder.path()).authorize(DELETE_TOOL, &call, None), refused, "{mode:?}");
            let EnforcementResult::Denied { reason, .. } =
                PermissionEnforcer::new(deleting_policy(mode, folder.path())).check(DELETE_TOOL, &call)
            else {
                panic!("{mode:?}: a deletion passed the enforcer");
            };
            assert_eq!(reason, Act::Deletes.nobody_to_ask(), "{mode:?}");
        }
    }
    let _pane = Pane::summoned_as_a_worker();
    let mut person = Person::saying(true);
    assert_eq!(
        deleting_policy(PermissionMode::DangerFullAccess, folder.path()).authorize(DELETE_TOOL, &call, Some(&mut person)),
        refused
    );
    assert!(person.asked.is_empty(), "a worker's pane puts no question");
}

/// A deletion called through the wrapper is asked about as one called by
/// name, and the question names the tool that would run.
#[test]
fn a_deletion_called_through_the_wrapper_is_put_to_a_person_too() {
    let _pane = Pane::a_person_opened();
    let (folder, cron_id) = a_folder_with_a_schedule();
    let wrapped = serde_json::json!({ "name": DELETE_TOOL, "input": { "cron_id": cron_id } }).to_string();
    let policy = deleting_policy(PermissionMode::DangerFullAccess, folder.path())
        .with_tool_requirement(core_types::tool_names::CAPABILITY_INVOKE, PermissionMode::ReadOnly);

    let mut person = Person::saying(true);
    assert_eq!(
        policy.authorize(core_types::tool_names::CAPABILITY_INVOKE, &wrapped, Some(&mut person)),
        PermissionOutcome::Allow
    );
    let [question] = person.asked.as_slice() else {
        panic!("asked {} times", person.asked.len());
    };
    assert_eq!(question.tool_name, DELETE_TOOL);
    assert!(question.person_only);
    assert!(question.reason.as_deref().unwrap_or_default().starts_with(SAYS_OF_A_DELETION));
    assert_eq!(
        policy.authorize(core_types::tool_names::CAPABILITY_INVOKE, &wrapped, None),
        PermissionOutcome::Deny {
            reason: Act::Deletes.nobody_to_ask()
        }
    );
}

/// What `zo cron remove` is handed, after the verb.
const REMOVED: &str = "--description nightly";

/// A shell command that would delete a schedule is refused in its own words —
/// the tool to delete through, how to find the id, and that another folder's
/// store is out of reach — wherever the shell would run it.
#[test]
fn a_shell_command_that_deletes_a_schedule_is_refused_in_its_own_words() {
    for command in [
        format!("zo cron remove {REMOVED}"),
        format!("zo cron remove {REMOVED} --cwd /Users/dev/work"),
        format!("/Users/dev/.local/bin/zo cron remove {REMOVED}"),
        format!("zo.exe cron remove {REMOVED}"),
        format!("cd /Users/dev/work && zo cron remove {REMOVED}"),
        format!("bash -c \"zo cron remove {REMOVED}\""),
    ] {
        let refused = refused_in_a_shell(&command).unwrap_or_else(|| panic!("passed: {command}"));
        assert!(
            refused.starts_with(
                "refused to run: this command deletes a schedule from a shell (`zo cron remove`), past the question a person answers. Delete a schedule with the CronDelete tool: find its `cron_id` with the CronList tool, then call CronDelete with it, and it puts the deletion to a person. A schedule kept for another folder (`--cwd`) cannot be deleted from this session. Permission audit:"
            ),
            "{refused}"
        );
        assert!(refused.contains("do not delete the schedule another way"), "{refused}");
        assert!(!refused.contains("  "), "the sentence runs on, with no gap in it: {refused:?}");
    }
}

/// The policy refuses it in every mode and asks nobody, through the wrapper
/// too; the enforcer refuses it in every mode, in a pane the ledger summoned
/// as a worker as well.
#[test]
fn every_layer_refuses_a_shell_command_that_deletes_a_schedule() {
    let command = format!("zo cron remove {REMOVED}");
    let refusal = refused_in_a_shell(&command).expect("a refusal");
    let call = serde_json::json!({ "command": command }).to_string();
    let wrapped = serde_json::json!({ "name": "Bash", "input": { "command": command } }).to_string();
    let modes = [
        PermissionMode::DangerFullAccess,
        PermissionMode::Allow,
        PermissionMode::WorkspaceWrite,
        PermissionMode::Prompt,
        PermissionMode::ReadOnly,
    ];
    for pane in [Pane::a_person_opened, Pane::summoned_as_a_worker] {
        let _pane = pane();
        for mode in modes {
            let policy = PermissionPolicy::new(mode)
                .with_tool_requirement(core_types::tool_names::CAPABILITY_INVOKE, PermissionMode::ReadOnly);
            for (tool, input) in [("bash", &call), ("PowerShell", &call), (core_types::tool_names::CAPABILITY_INVOKE, &wrapped)] {
                let mut person = Person::saying(true);
                assert_eq!(
                    policy.authorize(tool, input, Some(&mut person)),
                    PermissionOutcome::Deny {
                        reason: refusal.clone()
                    },
                    "{mode:?}, {tool}"
                );
                assert!(person.asked.is_empty(), "{mode:?}, {tool}: a refusal is not a question");
            }
            let EnforcementResult::Denied { reason, .. } = PermissionEnforcer::new(PermissionPolicy::new(mode)).check_bash(&command)
            else {
                panic!("{mode:?}: the command passed the enforcer");
            };
            assert_eq!(reason, refusal, "{mode:?}");
        }
    }
}
