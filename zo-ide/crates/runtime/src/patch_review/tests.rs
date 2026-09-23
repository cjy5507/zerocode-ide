//! The pure half, held to its contract: what a review is asked about and
//! what leaves the machine, how a reply is read and judged, the one line an
//! acting seat adds, and what hindsight makes of a patch. No wire anywhere.

use std::collections::BTreeMap;

use api::{SystemOneResponse, SystemOneUsage};
use serde_json::json;
use zerocode_core::jev::{rubric_fingerprint, CUT_MARK};

use super::*;

fn user_text(text: &str) -> ConversationMessage {
    ConversationMessage {
        role: MessageRole::User,
        blocks: vec![ContentBlock::Text { text: text.to_string() }],
        usage: None,
        thought_signature: None,
        reasoning_replay: None,
        model: None,
    }
}

fn call(id: &str, tool: &str, input: &str) -> ConversationMessage {
    ConversationMessage::assistant(vec![ContentBlock::ToolUse {
        id: id.to_string(),
        name: tool.to_string(),
        input: input.to_string(),
    }])
}

fn result(id: &str, tool: &str, output: &str) -> ConversationMessage {
    ConversationMessage::tool_result(id, tool, output, false)
}

/// An edit's result in the shape `edit_file` writes it.
fn edit_output(path: &str, hunks: &[(usize, usize, usize, usize, &[&str])]) -> String {
    let structured: Vec<serde_json::Value> = hunks
        .iter()
        .map(|(old_start, old_lines, new_start, new_lines, lines)| {
            json!({
                "oldStart": old_start,
                "oldLines": old_lines,
                "newStart": new_start,
                "newLines": new_lines,
                "lines": lines,
            })
        })
        .collect();
    serde_json::to_string_pretty(&json!({
        "filePath": path,
        "oldString": "b",
        "newString": "c",
        "structuredPatch": structured,
        "userModified": false,
        "replaceAll": false,
        "gitDiff": null,
    }))
    .expect("an envelope")
}

/// A green `cargo test` as the bash tool reports one.
fn green_bash() -> String {
    json!({"stdout": "test result: ok. 3 passed", "stderr": "", "interrupted": false}).to_string()
}

/// A conversation up to an edit: the person's words, a failing test, and a
/// harness reminder after them that must not be read as the task.
fn before_the_edit() -> Vec<ConversationMessage> {
    vec![
        user_text("fix the parser so `1+2` reads as 3"),
        call("t1", "bash", r#"{"command":"cargo test -p parser"}"#),
        result(
            "t1",
            "bash",
            &json!({
                "stdout": "running 3 tests\ntest adds ... FAILED\n\nfailures:\n    expected 3, got 12",
                "stderr": "",
                "returnCodeInterpretation": "exit_code:101",
            })
            .to_string(),
        ),
        user_text("[zo:turn-end-gate] <system-reminder>keep going</system-reminder>"),
        call("t2", "edit_file", r#"{"path":"src/parse.rs","old_string":"b","new_string":"c"}"#),
    ]
}

fn one_hunk() -> String {
    edit_output("/work/src/parse.rs", &[(10, 3, 10, 3, &[" a", "-b", "+c", " d"])])
}

fn answers(yes: [f64; 4]) -> Answers {
    Answers { yes }
}

/// Two readings of four answers, equal as the wire's numbers are.
fn same_answers(read: [f64; 4], expected: [f64; 4]) -> bool {
    read.iter().zip(expected).all(|(got, want)| (got - want).abs() < f64::EPSILON)
}

fn response(answers: serde_json::Value) -> SystemOneResponse {
    SystemOneResponse {
        model: "jev-1.13.0".to_string(),
        answers: serde_json::from_value::<BTreeMap<String, serde_json::Value>>(answers).expect("answers"),
        usage: SystemOneUsage { input_tokens: 900, output_tokens: 0 },
    }
}

/// zo's client and the window's wire build a Noul each on their own — the two
/// workspaces cannot share a crate for it — so the object they put on the
/// wire is held to be one shape.
#[test]
fn a_noul_on_zos_wire_is_the_one_the_window_builds() {
    for question in REVIEW_QUESTIONS {
        let zo = serde_json::to_value(SystemOneQuestion::noul(question.instructions, question.yes, question.no))
            .expect("a question serializes");
        assert_eq!(
            zo,
            zerocode_core::jev::noul::question(question.instructions, question.yes, question.no),
            "{}",
            question.id
        );
    }
}

/// The four questions are the reference harness's four, under its ids, as
/// Nouls; their words are pinned to the rubric version, so a word changed
/// without a bump is a red test rather than a quiet drift.
#[test]
fn four_nouls_under_the_harness_ids_and_a_pinned_rubric() {
    let asked = questions();
    assert_eq!(
        asked.keys().map(String::as_str).collect::<Vec<_>>(),
        ["addresses_task", "evidence_supports", "needs_clarification", "unrelated_changes"]
    );
    for question in asked.values() {
        assert_eq!(question.kind, api::SystemOneQuestionKind::Noul);
    }
    assert_eq!(
        REVIEW_QUESTIONS.iter().map(|question| question.yes_stands).collect::<Vec<_>>(),
        [true, true, false, false],
        "two questions a patch must pass, two it must not trip"
    );
    assert_eq!(PATCH_REVIEW_RUBRIC_VERSION, 1);
    assert_eq!(rubric_fingerprint(rubric_words), "3cdbf92b665d7854", "{}", rubric_words());
    // The note speaks as the harness speaks, so the person's words are never
    // mistaken for it — and it is never mistaken for the person's.
    assert!(PATCH_REVIEW_NOTE_PREFIX.starts_with(HARNESS_TAG_OPEN));
}

#[test]
fn an_ask_reads_the_patch_the_persons_words_and_the_evidences_tail() {
    let messages = before_the_edit();
    let ask = ask_for(&messages, "turn-1", "t2", "edit_file", &one_hunk()).expect("an edit asks");
    assert_eq!(ask.task, "fix the parser so `1+2` reads as 3", "the person's words, not the harness's");
    assert_eq!(ask.path, "/work/src/parse.rs");
    assert_eq!((ask.tool_use_id.as_str(), ask.tool_name.as_str(), ask.attempt.as_str()), ("t2", "edit_file", "turn-1"));
    assert_eq!(ask.hunks.len(), 1);
    // The test's own lines, unescaped, and the exit code beside them.
    assert!(ask.evidence.contains("test adds ... FAILED\n\nfailures:\n"), "{}", ask.evidence);
    assert!(ask.evidence.contains("expected 3, got 12"), "{}", ask.evidence);
    assert!(ask.evidence.contains("exit_code:101"), "{}", ask.evidence);
    assert!(!ask.evidence.contains("\\n"), "no escaped newline survives: {}", ask.evidence);

    // A line appended to the result never hides the patch.
    let noted = format!("{}\n\n[zo:patch-review] unrelated changes 0.82 — narrow it.", one_hunk());
    assert_eq!(ask_for(&messages, "turn-1", "t2", "edit_file", &noted), Some(ask.clone()));

    // Nothing to ask about.
    assert_eq!(ask_for(&messages, "turn-1", "t2", "bash", &one_hunk()), None, "not a mutation");
    assert_eq!(ask_for(&messages, "turn-1", "t2", "edit_file", "old_string not found in file"), None);
    assert_eq!(
        ask_for(&messages, "turn-1", "t2", "write_file", &edit_output("/work/same.rs", &[])),
        None,
        "a write that changed nothing"
    );
    assert_eq!(
        ask_for(&messages, "turn-1", "t2", "NotebookEdit", r#"{"cell":1,"notebook":"/w/a.ipynb"}"#),
        None,
        "a result of another shape"
    );
    let only_the_harness = vec![user_text("[zo:goal-plan] next step")];
    assert_eq!(ask_for(&only_the_harness, "turn-1", "t2", "edit_file", &one_hunk()), None, "no words of the person's");
    // No tool ran before: the evidence is empty, and the patch is still asked about.
    let first = vec![user_text("rename it")];
    assert_eq!(ask_for(&first, "turn-1", "t2", "edit_file", &one_hunk()).expect("asked").evidence, "");
}

/* ---- the task readings (t-6232) ---------------------------------------------- */

fn said(text: &str) -> ConversationMessage {
    ConversationMessage::assistant(vec![ContentBlock::Text { text: text.to_string() }])
}

/// The model's words and its edit call in one message, as a model writes them.
fn said_then_edits(text: &str, id: &str) -> ConversationMessage {
    ConversationMessage::assistant(vec![
        ContentBlock::Text { text: text.to_string() },
        ContentBlock::ToolUse {
            id: id.to_string(),
            name: "edit_file".to_string(),
            input: "{}".to_string(),
        },
    ])
}

/// A `TodoWrite` result in the shape the tool writes it.
fn todo_result(id: &str, todos: &[(&str, &str, &str)]) -> ConversationMessage {
    let new_todos: Vec<serde_json::Value> = todos
        .iter()
        .map(|(content, active, status)| json!({"content": content, "activeForm": active, "status": status}))
        .collect();
    result(
        id,
        "TodoWrite",
        &serde_json::to_string_pretty(&json!({"oldTodos": [], "newTodos": new_todos, "verificationNudgeNeeded": null}))
            .expect("an envelope"),
    )
}

/// Three turns before an edit: the person's words in each, the harness
/// speaking between, the model planning in writing and in words.
fn three_turns() -> Vec<ConversationMessage> {
    vec![
        user_text("the parser reads `1+2` as 12"),
        said("It concatenates digits across the operator."),
        user_text("fix it so it reads 3"),
        todo_result("p0", &[("Fix the tokenizer", "Fixing the tokenizer", "in_progress")]),
        user_text("[zo:turn-end-gate] <system-reminder>keep going</system-reminder>"),
        user_text("go on"),
        todo_result(
            "p1",
            &[
                ("Read the parser", "Reading the parser", "completed"),
                ("Split tokens at operators", "Splitting tokens at operators", "in_progress"),
                ("Add a test for 1+2", "Adding a test", "pending"),
            ],
        ),
        said_then_edits("The tokenizer never ends a number at `+`; I'll end it there.", "t2"),
    ]
}

#[test]
fn every_reading_asks_about_the_same_patches_and_the_seats_own_reads_as_before() {
    assert_eq!(TaskReading::ALL.map(TaskReading::word), ["v1", "v2a", "v2b", "v2c"]);
    for reading in TaskReading::ALL {
        assert_eq!(TaskReading::named(reading.word()), Some(reading));
    }
    assert_eq!(TaskReading::named("v3"), None);

    let conversations = [
        before_the_edit(),
        three_turns(),
        vec![user_text("rename it")],
        vec![user_text("[zo:goal-plan] next step"), said("On it.")],
        Vec::new(),
    ];
    for messages in &conversations {
        let seats = ask_for(messages, "turn-1", "t2", "edit_file", &one_hunk());
        assert_eq!(
            ask_reading(messages, "turn-1", "t2", "edit_file", &one_hunk(), TaskReading::PersonsNewest),
            seats,
            "the seat's reading is v1"
        );
        for reading in TaskReading::ALL {
            let asked = ask_reading(messages, "turn-1", "t2", "edit_file", &one_hunk(), reading);
            assert_eq!(asked.is_some(), seats.is_some(), "{} asks about the same patches", reading.word());
            if let (Some(asked), Some(seats)) = (asked, &seats) {
                // Only the task moves.
                assert_eq!(PatchAsk { task: seats.task.clone(), ..asked }, *seats, "{}", reading.word());
            }
        }
        assert_eq!(
            task_by(messages, TaskReading::PersonsNewest),
            persons_words(messages),
            "v1 is the person's newest words, uncut"
        );
    }
}

#[test]
fn the_recent_reading_is_the_persons_last_three_messages_in_the_order_said() {
    assert_eq!(
        task_by(&three_turns(), TaskReading::PersonsRecent),
        "the parser reads `1+2` as 12\n\nfix it so it reads 3\n\ngo on",
        "the harness's words are not the person's"
    );
    let mut four = three_turns();
    four.insert(0, user_text("hello"));
    assert_eq!(
        task_by(&four, TaskReading::PersonsRecent),
        task_by(&three_turns(), TaskReading::PersonsRecent),
        "three, the newest"
    );
    assert_eq!(task_by(&[user_text("rename it")], TaskReading::PersonsRecent), "rename it");
}

#[test]
fn the_plan_reading_is_the_persons_words_then_the_models_since_them() {
    assert_eq!(
        task_by(&three_turns(), TaskReading::ModelsPlan),
        "go on\n\nThe tokenizer never ends a number at `+`; I'll end it there."
    );
    // The model's words from before the person last spoke are not its plan
    // for this edit.
    let silent = vec![user_text("fix it"), said("I will fix it."), user_text("and rename it")];
    assert_eq!(task_by(&silent, TaskReading::ModelsPlan), "and rename it");
    // The newest words the model said since, wherever the call came after them.
    let mut earlier = silent.clone();
    earlier.extend([said("Renaming `old` to `new`."), call("t1", "edit_file", "{}")]);
    assert_eq!(task_by(&earlier, TaskReading::ModelsPlan), "and rename it\n\nRenaming `old` to `new`.");
}

#[test]
fn the_todo_reading_is_the_turns_open_plan_or_else_the_models_words() {
    // The newest plan of the turn, what is open first; the one before the
    // person last spoke is not this turn's.
    assert_eq!(
        task_by(&three_turns(), TaskReading::TodoPlan),
        "[~] Splitting tokens at operators\n[ ] Add a test for 1+2\n[x] Read the parser"
    );
    // A turn without a plan of its own reads as v2b.
    let mut unplanned = three_turns();
    unplanned.retain(|message| {
        !message.blocks.iter().any(|block| matches!(block, ContentBlock::ToolResult { tool_use_id, .. } if tool_use_id == "p1"))
    });
    assert_eq!(
        task_by(&unplanned, TaskReading::TodoPlan),
        task_by(&unplanned, TaskReading::ModelsPlan)
    );
    // A plan with nothing open says nothing about the next patch.
    let mut done = three_turns();
    done.insert(7, todo_result("p2", &[("Split tokens at operators", "Splitting tokens", "completed")]));
    assert_eq!(task_by(&done, TaskReading::TodoPlan), task_by(&done, TaskReading::ModelsPlan));
    // A failed write is no plan, and text a hook appended hides none.
    let mut failed = unplanned.clone();
    failed.insert(6, ConversationMessage::tool_result("p3", "TodoWrite", "todos must not be empty", true));
    assert_eq!(task_by(&failed, TaskReading::TodoPlan), task_by(&failed, TaskReading::ModelsPlan));
    let mut hooked = unplanned;
    let ContentBlock::ToolResult { output, .. } = &todo_result("p4", &[("Split", "Splitting", "in_progress")]).blocks[0] else {
        unreachable!("a tool result");
    };
    hooked.insert(6, result("p4", "todo_write", &format!("{output}\n\nhook: noted")));
    assert_eq!(task_by(&hooked, TaskReading::TodoPlan), "[~] Splitting");
}

/// A joined task fits the row's cap itself, the newest words first: the door
/// cuts a task to its head, and a pasted brief said before must not push
/// "go on" off its end.
#[test]
fn a_joined_task_keeps_the_newest_words_within_the_cap() {
    let brief = "b".repeat(PATCH_REVIEW_TASK_CHAR_CAP * 3);
    let messages = vec![user_text(&brief), said("Read it."), user_text("go on")];
    let recent = task_by(&messages, TaskReading::PersonsRecent);
    assert_eq!(recent.chars().count(), PATCH_REVIEW_TASK_CHAR_CAP);
    assert!(recent.starts_with("bbb") && recent.ends_with("bb\n\ngo on"), "the brief's head, then the newest whole");
    let ask = ask_reading(&messages, "turn-1", "t2", "edit_file", &one_hunk(), TaskReading::PersonsRecent).expect("asked");
    assert_eq!(state(&ask)["task"], recent, "the state carries it as fitted");
    // The model's words are the newest: past the whole cap, the person's go.
    let long = vec![user_text("go on"), said_then_edits(&"m".repeat(PATCH_REVIEW_TASK_CHAR_CAP + 5), "t2")];
    assert_eq!(task_by(&long, TaskReading::ModelsPlan), "m".repeat(PATCH_REVIEW_TASK_CHAR_CAP));
    // Short of it, the person's words keep the head of the room left.
    let tight = vec![user_text("go on"), said_then_edits(&"m".repeat(PATCH_REVIEW_TASK_CHAR_CAP - 5), "t2")];
    assert_eq!(
        task_by(&tight, TaskReading::ModelsPlan),
        format!("go \n\n{}", "m".repeat(PATCH_REVIEW_TASK_CHAR_CAP - 5))
    );
}

/// A mutation's own result is not evidence for the next patch: the evidence
/// is the newest result that is not one.
#[test]
fn the_evidence_is_the_newest_result_that_is_not_itself_an_edit() {
    let mut messages = before_the_edit();
    messages.push(result("t2", "edit_file", &one_hunk()));
    messages.push(call("t3", "edit_file", "{}"));
    let evidence = evidence_before(&messages);
    assert!(evidence.contains("expected 3, got 12"), "{evidence}");
    assert!(!evidence.contains("structuredPatch"), "{evidence}");
}

#[test]
fn the_evidence_keeps_the_newest_lines_within_its_cap() {
    let long = (0..2_000).map(|at| format!("line {at}")).collect::<Vec<_>>().join("\n");
    let messages = vec![
        user_text("look"),
        result("t1", "bash", &json!({"stdout": long, "stderr": ""}).to_string()),
    ];
    let evidence = evidence_before(&messages);
    assert!(evidence.len() <= PATCH_REVIEW_EVIDENCE_BYTE_CAP, "{}", evidence.len());
    assert!(evidence.ends_with("line 1999"), "the newest lines: {}", &evidence[evidence.len() - 20..]);
    assert!(!evidence.contains("line 0\n"));
    // One line longer than the whole cap keeps its own end.
    let one = "x".repeat(PATCH_REVIEW_EVIDENCE_BYTE_CAP * 2) + "END";
    let messages = vec![user_text("look"), result("t1", "read_file", &one)];
    let evidence = evidence_before(&messages);
    assert_eq!(evidence.len(), PATCH_REVIEW_EVIDENCE_BYTE_CAP);
    assert!(evidence.ends_with("END"));
}

/// What leaves the machine: the person's words, the hunks, the evidence and
/// the path's fingerprint — never the path, never a file header.
#[test]
fn the_state_carries_hunks_a_tail_and_a_fingerprint_never_the_path() {
    let ask = ask_for(&before_the_edit(), "turn-1", "t2", "edit_file", &one_hunk()).expect("asked");
    let state = state(&ask);
    let keys: Vec<&str> = state.as_object().expect("an object").keys().map(String::as_str).collect();
    assert_eq!(keys, ["evidence", "patch", "path", "task"]);
    assert_eq!(state["patch"], "@@ -10,3 +10,3 @@\n a\n-b\n+c\n d");
    assert_eq!(state["path"], zerocode_core::jev::fingerprint_of("/work/src/parse.rs"));
    assert!(!state.to_string().contains("/work/src/parse.rs"), "{state}");
    assert_eq!(state["task"], ask.task);
}

#[test]
fn the_state_is_cut_to_the_rows_caps() {
    let mut ask = ask_for(&before_the_edit(), "turn-1", "t2", "edit_file", &one_hunk()).expect("asked");
    ask.task = "t".repeat(PATCH_REVIEW_TASK_CHAR_CAP + 50);
    let long_lines: Vec<String> = (0..2_000).map(|at| format!("+added line {at}")).collect();
    ask.hunks = vec![StructuredPatchHunk {
        old_start: 1,
        old_lines: 0,
        new_start: 1,
        new_lines: long_lines.len(),
        lines: long_lines,
    }];
    let state = state(&ask);
    assert_eq!(state["task"].as_str().expect("words").chars().count(), PATCH_REVIEW_TASK_CHAR_CAP);
    let patch = state["patch"].as_str().expect("a patch");
    assert!(patch.len() <= PATCH_REVIEW_PATCH_BYTE_CAP + CUT_MARK.len(), "{}", patch.len());
    assert!(patch.starts_with("@@ -1,0 +1,2000 @@\n+added line 0"), "the head of the patch");
    assert!(patch.ends_with(CUT_MARK));
}

#[test]
fn a_reply_reads_as_four_probabilities_or_not_at_all() {
    let good = json!({
        "addresses_task": {"type": "noul", "noul": 0.93},
        "evidence_supports": {"type": "noul", "noul": 0.81},
        "unrelated_changes": {"type": "noul", "noul": 0.12},
        "needs_clarification": {"type": "noul", "noul": 0.05},
    });
    let read_good = read(&response(good.clone())).expect("four answers");
    assert!(same_answers(read_good.yes, [0.93, 0.81, 0.12, 0.05]), "{:?}", read_good.yes);
    assert_eq!(
        read_good.by_id(),
        BTreeMap::from([
            ("addresses_task".to_string(), 0.93),
            ("evidence_supports".to_string(), 0.81),
            ("needs_clarification".to_string(), 0.05),
            ("unrelated_changes".to_string(), 0.12),
        ])
    );

    let mut stray = good.clone();
    stray["extra"] = json!({"type": "noul", "noul": 0.5});
    assert_eq!(read(&response(stray)), Err(ReviewRejection::UnknownAnswer("extra".to_string())));

    let mut missing = good.clone();
    missing.as_object_mut().expect("answers").remove("needs_clarification");
    let refused = read(&response(missing)).expect_err("three answers are not a review");
    assert_eq!(refused.rule(), NoulRefusal::NoAnswer.token());

    let mut wide = good.clone();
    wide["unrelated_changes"] = json!({"type": "noul", "noul": 1.2});
    assert_eq!(read(&response(wide)).expect_err("out of range").rule(), NoulRefusal::OutOfRange.token());

    let mut choice = good;
    choice["addresses_task"] = json!({"type": "choice", "choice": "yes"});
    assert_eq!(read(&response(choice)).expect_err("not a noul").rule(), NoulRefusal::NotANoul.token());
}

/// The one code judgment, at its line: every favourable lean at least the
/// permit line, every unfavourable one at most one minus it.
#[test]
fn the_verdict_is_one_rule_at_the_permit_line() {
    let line = f64::from(PATCH_REVIEW_PERMIT_FLOOR_PERMILLE) / 1000.0;
    assert_eq!(verdict(Some(&answers([line, line, 1.0 - line, 1.0 - line]))), Verdict::Permit);
    assert_eq!(verdict(Some(&answers([0.99, 0.99, 0.0, 0.0]))), Verdict::Permit);
    for (at, leaning) in [(0, 0.79), (1, 0.79), (2, 0.21), (3, 0.21)] {
        let mut yes = [0.99, 0.99, 0.01, 0.01];
        yes[at] = leaning;
        assert_eq!(verdict(Some(&answers(yes))), Verdict::ProposalOnly, "{}", REVIEW_QUESTIONS[at].id);
    }
    // The wire's hundredths are read in permille exactly: 0.57 is 570, not 569.
    assert_eq!(standing_permille(&REVIEW_QUESTIONS[0], 0.57), 570);
    assert_eq!(standing_permille(&REVIEW_QUESTIONS[2], 0.2), 800);
    assert_eq!(verdict(None), Verdict::Unavailable);
    assert_eq!(
        [Verdict::Permit, Verdict::ProposalOnly, Verdict::Unavailable].map(Verdict::word),
        ["permit", "proposal_only", "unavailable"]
    );
}

#[test]
fn a_note_names_what_leaned_the_wrong_way_furthest_first() {
    assert_eq!(
        note(&answers([0.95, 0.9, 0.82, 0.05])).as_deref(),
        Some("[zo:patch-review] unrelated changes 0.82 — narrow the edit to the task or confirm the extra change is wanted.")
    );
    assert_eq!(
        note(&answers([0.6, 0.2, 0.3, 0.05])).as_deref(),
        Some("[zo:patch-review] evidence supports it 0.20, addresses the task 0.60, unrelated changes 0.30 — run the check or read the code that shows the change is needed."),
        "furthest first, with the furthest's ask"
    );
    assert_eq!(note(&answers([0.95, 0.9, 0.1, 0.05])), None, "a permit says nothing");
}

/* ---- hindsight -------------------------------------------------------------- */

fn watched(tool_use_id: &str, path: &str, output: &str) -> Watched {
    let ask = ask_for(&[user_text("go")], "turn", tool_use_id, "edit_file", output).expect("an edit");
    assert_eq!(ask.path, path);
    Watched::of(&ask)
}

/// The patch at lines 10–11 of `/w/a.rs`: two lines added after line 9.
fn two_lines_at_ten() -> String {
    edit_output("/w/a.rs", &[(7, 6, 7, 8, &[" 7", " 8", " 9", "+ten", "+eleven", " 10", " 11", " 12"])])
}

#[test]
fn an_edit_of_the_same_lines_regrets_a_patch_and_an_edit_above_moves_it() {
    let mut book = vec![watched("p1", "/w/a.rs", &two_lines_at_ten())];
    // The patch's own result, then three lines inserted at the top of the
    // file: the patch's lines are now 13–14, and nothing touched them.
    let turn = vec![
        call("p1", "edit_file", "{}"),
        result("p1", "edit_file", &two_lines_at_ten()),
        call("e1", "edit_file", "{}"),
        result("e1", "edit_file", &edit_output("/w/a.rs", &[(1, 1, 1, 4, &["+x", "+y", "+z", " 1"])])),
    ];
    assert!(hindsight_of_turn(&mut book, &turn).is_empty());
    assert_eq!(book[0].regions, vec![Region { start: 13, end: 15 }]);
    // An edit of line 14 in the next turn: a fix of the fix.
    let turn = vec![
        user_text("that broke the build"),
        call("e2", "edit_file", "{}"),
        result("e2", "edit_file", &edit_output("/w/a.rs", &[(13, 3, 13, 3, &[" ten", "-eleven", "+eleven!", " 10"])])),
    ];
    let decided = hindsight_of_turn(&mut book, &turn);
    assert_eq!(decided.len(), 1);
    assert_eq!(decided[0].1, Hindsight::Regretted);
    assert_eq!(decided[0].0.turns, 1, "decided inside the second turn it waited");
    assert!(book.is_empty());
}

/// An edit below the patch moves nothing; an edit of another file is not
/// the patch's hindsight; an edit before the patch's own result is not
/// either.
#[test]
fn only_a_later_edit_of_the_same_lines_of_the_same_file_counts() {
    let mut book = vec![watched("p1", "/w/a.rs", &two_lines_at_ten())];
    let turn = vec![
        // Before the patch's own result: not its hindsight.
        result("e0", "edit_file", &edit_output("/w/a.rs", &[(10, 1, 10, 1, &["-ten", "+TEN"])])),
        result("p1", "edit_file", &two_lines_at_ten()),
        // Another file, the same line numbers.
        result("e1", "edit_file", &edit_output("/w/b.rs", &[(10, 1, 10, 1, &["-ten", "+TEN"])])),
        // Below the patch.
        result("e2", "edit_file", &edit_output("/w/a.rs", &[(20, 1, 20, 1, &["-x", "+y"])])),
        // A failed edit of the same lines changed nothing.
        ConversationMessage::tool_result("e3", "edit_file", two_lines_at_ten(), true),
    ];
    assert!(hindsight_of_turn(&mut book, &turn).is_empty());
    assert_eq!(book[0].regions, vec![Region { start: 10, end: 12 }]);
}

/// A replacement is the lines it took away: one that ends on the line before
/// the patch, or starts on the line after it, is beside it; an addition
/// right before or right after is beside it too — but one between two of
/// its lines, or one that takes away a line it wrote, is in it.
#[test]
fn beside_the_patch_is_not_in_it() {
    let beside: [&[&str]; 4] = [
        &["-nine", "+NINE"],     // line 9, just above
        &[" 11", "-12", "+TWELVE"], // line 12, just below
        &["+before"],            // put in before line 10
        &["+after"],             // put in before line 12, right after 11
    ];
    let starts = [9, 11, 10, 12];
    for (lines, start) in beside.iter().zip(starts) {
        let mut book = vec![watched("p1", "/w/a.rs", &two_lines_at_ten())];
        let hunk = (start, lines.iter().filter(|line| !line.starts_with('+')).count(), start, lines.iter().filter(|line| !line.starts_with('-')).count(), *lines);
        let turn = vec![
            result("p1", "edit_file", &two_lines_at_ten()),
            result("e1", "edit_file", &edit_output("/w/a.rs", &[hunk])),
        ];
        assert!(hindsight_of_turn(&mut book, &turn).is_empty(), "{lines:?} at {start}");
    }
    let inside: [(usize, &[&str]); 2] = [(11, &["+between"]), (11, &["-eleven"])];
    for (start, lines) in inside {
        let mut book = vec![watched("p1", "/w/a.rs", &two_lines_at_ten())];
        let hunk = (start, lines.iter().filter(|line| !line.starts_with('+')).count(), start, lines.iter().filter(|line| !line.starts_with('-')).count(), lines);
        let turn = vec![
            result("p1", "edit_file", &two_lines_at_ten()),
            result("e1", "edit_file", &edit_output("/w/a.rs", &[hunk])),
        ];
        let decided = hindsight_of_turn(&mut book, &turn);
        assert_eq!(decided.len(), 1, "{lines:?} at {start}");
        assert_eq!(decided[0].1, Hindsight::Regretted);
    }
}

/// The receipt settles a patch first: a check green after the turn's last
/// edit, and the patch stood — whether it was written this turn or before.
#[test]
fn a_check_green_after_the_turns_last_edit_settles_a_patch_as_standing() {
    let mut book = vec![watched("p1", "/w/a.rs", &two_lines_at_ten())];
    let turn = vec![
        user_text("fix it"),
        call("p1", "edit_file", "{}"),
        result("p1", "edit_file", &two_lines_at_ten()),
        call("c1", "bash", r#"{"command":"cargo test -p a"}"#),
        result("c1", "bash", &green_bash()),
    ];
    let decided = hindsight_of_turn(&mut book, &turn);
    assert_eq!(decided.len(), 1);
    assert_eq!(decided[0].1, Hindsight::Stood { receipt: true });
    assert_eq!(decided[0].0.turns, 1);

    // A check that ran before the turn's last edit is no receipt.
    let mut book = vec![watched("p1", "/w/a.rs", &two_lines_at_ten())];
    let turn = vec![
        user_text("fix it"),
        call("c1", "bash", r#"{"command":"cargo test -p a"}"#),
        result("c1", "bash", &green_bash()),
        call("p1", "edit_file", "{}"),
        result("p1", "edit_file", &two_lines_at_ten()),
    ];
    assert!(hindsight_of_turn(&mut book, &turn).is_empty());
    // Nor is a red one, or a green command that checks nothing.
    let turn = vec![
        user_text("again"),
        call("e1", "edit_file", "{}"),
        result("e1", "edit_file", &edit_output("/w/c.rs", &[(1, 1, 1, 1, &["-a", "+b"])])),
        call("c2", "bash", r#"{"command":"ls -la"}"#),
        result("c2", "bash", &green_bash()),
        call("c3", "bash", r#"{"command":"cargo test"}"#),
        result(
            "c3",
            "bash",
            &json!({"stdout": "FAILED", "stderr": "", "returnCodeInterpretation": "exit_code:101"}).to_string(),
        ),
    ];
    assert!(hindsight_of_turn(&mut book, &turn).is_empty());
}

#[test]
fn a_window_of_quiet_turns_lets_a_patch_stand() {
    let mut book = vec![watched("p1", "/w/a.rs", &two_lines_at_ten())];
    let quiet = vec![user_text("and now the docs"), ConversationMessage::assistant(vec![ContentBlock::Text {
        text: "done".to_string(),
    }])];
    for _ in 1..PATCH_REVIEW_REGRET_TURNS {
        assert!(hindsight_of_turn(&mut book, &quiet).is_empty());
    }
    let decided = hindsight_of_turn(&mut book, &quiet);
    assert_eq!(decided.len(), 1);
    assert_eq!(decided[0].1, Hindsight::Stood { receipt: false });
    assert_eq!(decided[0].0.turns, PATCH_REVIEW_REGRET_TURNS);
}

#[test]
fn a_verdict_agrees_with_hindsight_when_it_called_what_became_of_the_patch() {
    let stood = Hindsight::Stood { receipt: false };
    assert_eq!(stood.agrees_with(Verdict::Permit), Some(true));
    assert_eq!(stood.agrees_with(Verdict::ProposalOnly), Some(false));
    assert_eq!(Hindsight::Regretted.agrees_with(Verdict::ProposalOnly), Some(true));
    assert_eq!(Hindsight::Regretted.agrees_with(Verdict::Permit), Some(false));
    assert_eq!(stood.agrees_with(Verdict::Unavailable), None);
}

/// The turns of a transcript begin at the person's words; what the harness
/// writes in their place continues the turn it was written in.
#[test]
fn a_turn_begins_at_the_persons_words() {
    let messages = vec![
        user_text("first"),
        call("t1", "bash", "{}"),
        result("t1", "bash", "ok"),
        user_text("[zo:turn-end-gate] <system-reminder>go on</system-reminder>"),
        ConversationMessage::assistant(vec![ContentBlock::Text { text: "done".to_string() }]),
        user_text("second"),
        ConversationMessage::assistant(vec![ContentBlock::Text { text: "ok".to_string() }]),
    ];
    let turns = persons_turns(&messages);
    assert_eq!(turns.iter().map(|turn| turn.len()).collect::<Vec<_>>(), [5, 2]);
}
