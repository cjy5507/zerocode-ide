//! The turn's ruler on synthetic hook payloads, held to the t-14656 baseline's
//! definitions: the same turn written as a transcript and counted by that
//! script gives the numbers these tests expect (calls 9, looking around 4,
//! first edit at 7 after 3 looks, 3 repeats, one run of 3, 2 failures of the
//! same call, 1 skill).

use serde_json::{Value, json};

use super::*;

/// A Claude-family tool event, every personal value a placeholder.
fn claude(event: &str, tool: &str, input: &Value, id: usize) -> String {
    json!({
        "session_id": "session-1",
        "cwd": "/w/project",
        "hook_event_name": event,
        "tool_name": tool,
        "tool_input": input,
        "tool_use_id": format!("call-{id}"),
    })
    .to_string()
}

fn told(agent: AgentKind, event: &str, payload: &str) -> Vec<Tallied> {
    tallied_parsed(agent, event, &HookPayload::of(payload))
}

/// The synthetic turn: each call, and whether it failed.
fn the_turn() -> Vec<(&'static str, Value, bool)> {
    vec![
        ("Read", json!({ "file_path": "/w/project/src/a.rs" }), false),
        (
            "Grep",
            json!({ "pattern": "foo", "path": "/w/project" }),
            false,
        ),
        (
            "Bash",
            json!({ "command": "cd /w/project && git status", "description": "see" }),
            false,
        ),
        (
            "Bash",
            json!({ "command": "cargo test", "description": "run" }),
            true,
        ),
        (
            "Bash",
            json!({ "command": "cargo test", "description": "again", "timeout": 100 }),
            true,
        ),
        ("Bash", json!({ "command": "cargo test" }), false),
        ("Skill", json!({ "skill": "example" }), false),
        (
            "Edit",
            json!({ "file_path": "/w/project/src/b.rs", "old_string": "a", "new_string": "b" }),
            false,
        ),
        ("Read", json!({ "file_path": "/w/project/src/a.rs" }), false),
    ]
}

/// A Claude turn counted off its hooks comes to the numbers the baseline
/// script counts off the same turn's transcript.
#[test]
fn a_claude_turn_counts_as_the_baseline_counts_it() {
    let mut tally = TurnTally::default();
    for (id, (tool, input, failed)) in the_turn().into_iter().enumerate() {
        for one in told(
            AgentKind::Claude,
            "PreToolUse",
            &claude("PreToolUse", tool, &input, id),
        ) {
            tally.take(one);
        }
        let after = if failed {
            "PostToolUseFailure"
        } else {
            "PostToolUse"
        };
        for one in told(AgentKind::Claude, after, &claude(after, tool, &input, id)) {
            tally.take(one);
        }
    }
    assert_eq!(tally.explore_before_first_edit(), Some(3));
    let row = tally
        .row(7, AgentKind::Claude.slug(), Some("project".to_string()))
        .expect("a turn with calls has a row");
    assert_eq!(
        row,
        PaneTurnRow {
            at: 7,
            from: "claude".to_string(),
            pane: Some("project".to_string()),
            calls: 9,
            explore_calls: 4,
            calls_before_first_edit: Some(7),
            explore_before_first_edit: Some(3),
            duplicate_calls: 3,
            repeat_runs: 1,
            repeat_calls: 2,
            failed_calls: 2,
            same_failure_again: true,
            skill_loads: 1,
        }
    );
}

/// A call is told once: where it begins when the row sees that, and a
/// finished call adds nothing; a failure is told where it comes back.
#[test]
fn a_call_is_told_where_it_begins_and_a_failure_where_it_comes_back() {
    let input = json!({ "command": "cargo test" });
    let began = told(
        AgentKind::Claude,
        "PreToolUse",
        &claude("PreToolUse", "Bash", &input, 1),
    );
    assert_eq!(
        began,
        [Tallied::Called {
            key: call_key("Bash", Some(&input)),
            kind: CallKind::default(),
        }]
    );
    assert!(
        told(
            AgentKind::Claude,
            "PostToolUse",
            &claude("PostToolUse", "Bash", &input, 1)
        )
        .is_empty()
    );
    assert_eq!(
        told(
            AgentKind::Claude,
            "PostToolUseFailure",
            &claude("PostToolUseFailure", "Bash", &input, 1)
        ),
        [Tallied::Failed {
            key: call_key("Bash", Some(&input)),
        }]
    );
    // The person's prompt and the turn's end are not calls.
    let prompt = json!({ "hook_event_name": "UserPromptSubmit", "prompt": "fix it" }).to_string();
    assert!(told(AgentKind::Claude, "UserPromptSubmit", &prompt).is_empty());
}

/// Antigravity's hooks say nothing before a tool runs (t-10461): its calls
/// are counted where they come back, once each.
#[test]
fn an_agent_with_nothing_before_a_tool_is_counted_where_it_comes_back() {
    let input = json!({ "command": "ls src" });
    let payload = claude("PostToolUse", "run_command", &input, 1);
    let after = told(AgentKind::Antigravity, "PostToolUse", &payload);
    assert_eq!(
        after,
        [Tallied::Called {
            key: call_key("run_command", Some(&input)),
            kind: CallKind {
                explore: true,
                ..CallKind::default()
            },
        }]
    );
    // zo asks its own seats and counts its own turns: the window reads none.
    assert!(told(AgentKind::Zo, "PostToolUse", &payload).is_empty());
}

/// Codex and the agents like it load a skill by reading its file with the
/// shell; Claude has a tool for it, and reading the file there is only a
/// read.
#[test]
fn a_skill_load_is_read_off_the_agents_row() {
    let read_skill = json!({ "command": "cat /Users/dev/.codex/skills/example/SKILL.md" });
    assert_eq!(
        told(
            AgentKind::Codex,
            "PreToolUse",
            &claude("PreToolUse", "Bash", &read_skill, 1)
        ),
        [Tallied::Called {
            key: call_key("Bash", Some(&read_skill)),
            kind: CallKind {
                explore: true,
                skill: true,
                ..CallKind::default()
            },
        }]
    );
    assert_eq!(
        told(
            AgentKind::Claude,
            "PreToolUse",
            &claude("PreToolUse", "Bash", &read_skill, 1)
        ),
        [Tallied::Called {
            key: call_key("Bash", Some(&read_skill)),
            kind: CallKind {
                explore: true,
                ..CallKind::default()
            },
        }]
    );
    let skill = json!({ "skill": "example" });
    assert_eq!(
        told(
            AgentKind::Claude,
            "PreToolUse",
            &claude("PreToolUse", "Skill", &skill, 2)
        ),
        [Tallied::Called {
            key: call_key("Skill", Some(&skill)),
            kind: CallKind {
                skill: true,
                ..CallKind::default()
            },
        }]
    );
}

/// A shell command looks around by its program, past `cd … &&` and the
/// variables set before it — the baseline's reading.
#[test]
fn a_shell_command_looks_around_by_its_program() {
    for command in [
        "cat src/a.rs",
        "/usr/bin/sed -n 1,20p src/a.rs",
        "cd /w/project && rg foo",
        "cd /w/project; ls -la",
        "LANG=C grep -n foo src",
        "git log --oneline -3",
        "git diff",
        "zerocode-find the parser crash",
    ] {
        assert!(explore_command(command), "{command}");
    }
    for command in [
        "cargo test",
        "git commit -m x",
        "cd /w/project && cargo build",
        "",
    ] {
        assert!(!explore_command(command), "{command}");
    }
}

/// Two calls that differ only in how to run them, or in their keys' order,
/// are one call; a different input is another.
#[test]
fn a_calls_name_is_its_tool_and_what_it_does() {
    let one = call_key(
        "Bash",
        Some(&json!({ "command": "cargo test", "description": "a" })),
    );
    assert_eq!(
        one,
        call_key(
            "Bash",
            Some(&json!({ "timeout": 5, "command": "cargo test", "run_in_background": false }))
        )
    );
    assert_ne!(
        one,
        call_key("Bash", Some(&json!({ "command": "cargo build" })))
    );
    assert_ne!(
        one,
        call_key("Shell", Some(&json!({ "command": "cargo test" })))
    );
    assert_eq!(
        call_key("Read", Some(&json!({ "b": 1, "a": { "y": 2, "x": 1 } }))),
        call_key("Read", Some(&json!({ "a": { "x": 1, "y": 2 }, "b": 1 })))
    );
    assert!(!one.is_empty());
}

/// A turn that made no call has no row; one that edited nothing says
/// nothing of a first edit; a row carries counts and the agent's name, and
/// nothing the agent ran.
#[test]
fn a_row_carries_counts_only() {
    let empty = TurnTally::default();
    assert!(empty.is_empty());
    assert_eq!(empty.row(1, "codex", None), None);
    let mut tally = TurnTally::default();
    tally.take(Tallied::Called {
        key: call_key("Bash", Some(&json!({ "command": "cat secret.txt" }))),
        kind: CallKind {
            explore: true,
            ..CallKind::default()
        },
    });
    assert_eq!(tally.explore_before_first_edit(), None);
    let row = tally.row(1, "codex", None).expect("one call");
    assert_eq!(row.calls_before_first_edit, None);
    let written = serde_json::to_value(&row).expect("a row serializes");
    let keys: std::collections::BTreeSet<&str> = written
        .as_object()
        .expect("an object")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        std::collections::BTreeSet::from([
            "at",
            "from",
            "calls",
            "exploreCalls",
            "duplicateCalls",
            "repeatRuns",
            "repeatCalls",
            "failedCalls",
            "sameFailureAgain",
            "skillLoads"
        ])
    );
    assert!(!written.to_string().contains("secret"));
}
