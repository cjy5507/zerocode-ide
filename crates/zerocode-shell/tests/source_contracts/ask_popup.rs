//! Every question the window puts to the person is one popup (t-17514): a tool
//! permission, an agent's own question (zo's model switch), Computer Use's
//! confirm and its hand-over, and the window's own confirms are KINDS of ask
//! that share one frame in the middle of the window, one queue, and one
//! keyboard. The browser suite (`ui/tests/ask-popup.mjs`) drives it; these
//! contracts hold the shapes that suite cannot see from outside — which
//! elements exist, how a kind is told from another, and what no kind may say.

use std::path::Path;

use super::support::{block_after, block_after_css, window_source};

fn ui_file(name: &str) -> String {
    let ui = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ui");
    std::fs::read_to_string(ui.join(name)).unwrap_or_else(|why| panic!("{name}: {why}"))
}

/// One frame. The two pills Computer Use put along the top edge, the tool
/// modal, and the generic confirm were four surfaces in three places; the
/// person asked for one, where the eye already is.
#[test]
fn the_window_has_one_ask_popup_and_it_stands_above_the_other_dialogs() {
    let markup = ui_file("index.html");
    let sheet = ui_file("shell.css");
    let window = window_source();

    assert_eq!(
        markup.matches(r#"id="ask-scrim""#).count(),
        1,
        "the ask popup is not exactly one element"
    );
    for gone in [
        r#"id="perm-scrim""#,
        r#"id="perm-title""#,
        r#"id="computer-confirm""#,
        r#"id="computer-handoff""#,
    ] {
        assert!(
            !markup.contains(gone),
            "{gone} is back: a second surface asks the person a question"
        );
    }
    // Centred, where the permission modal always sat — not the launcher's seat
    // along the top (`scrim--top`), which is where the generic confirm sat.
    let opening = markup
        .find(r#"id="ask-scrim""#)
        .and_then(|at| markup[..at].rfind("<div"))
        .expect("the ask popup's opening tag");
    let tag = &markup[opening..markup[opening..].find('>').map(|at| opening + at).unwrap()];
    assert!(
        tag.contains(r#"class="scrim""#) && !tag.contains("scrim--"),
        "the ask popup left the middle of the window: {tag}"
    );
    // Last in the body, so DOM order puts it over every dialog that shares
    // its layer: an agent that is waiting must never sit behind a wizard.
    let after = &markup[markup.find(r#"id="ask-scrim""#).unwrap()..];
    assert_eq!(
        after.matches(r#"class="scrim"#).count(),
        0,
        "another scrim stands after the ask popup, and covers it"
    );

    // The Computer Use band asks nothing and stays where it is; the pills are gone
    // from the stylesheet with their markup.
    assert!(
        markup.contains(r#"id="computer-band""#) && sheet.contains(".computer-band {"),
        "the status that asks nothing lost its place"
    );
    for gone in [
        ".computer-confirm {",
        ".computer-handoff {",
        ".computer-confirm-clock",
    ] {
        assert!(
            !sheet.contains(gone),
            "{gone} still dresses a surface that is gone"
        );
    }

    // One registry entry for the popup, and none for the modal it replaced.
    assert!(
        !window.contains("perm-scrim") && !window.contains("permScrim"),
        "the window still names the permission modal"
    );
    let specs = block_after(window, "const MODAL_SPECS = Object.freeze({");
    assert_eq!(
        specs.matches("\"ask-scrim\": {").count(),
        1,
        "the modal manager does not know the ask popup exactly once"
    );
}

/// The five kinds, each registered by the file that owns its words.
#[test]
fn every_kind_of_ask_is_registered_once_by_the_file_that_owns_its_words() {
    let window = window_source();
    for kind in [
        "tool",
        "question",
        "computer-confirm",
        "computer-handoff",
        "confirm",
    ] {
        assert_eq!(
            window
                .matches(&format!("registerAskKind(\"{kind}\", {{"))
                .count(),
            1,
            "the `{kind}` kind is not registered exactly once"
        );
    }
    assert!(
        ui_file("shell-computer.js").contains("registerAskKind(\"computer-handoff\", {")
            && ui_file("shell-workspace.js").contains("registerAskKind(\"confirm\", {")
            && ui_file("shell-term.js").contains("registerAskKind(\"question\", {"),
        "a kind is registered by a file that does not own its words"
    );
    // The window's own confirm keeps its promise API and its exit.
    let confirm = block_after(window, "registerAskKind(\"confirm\", {");
    assert!(
        confirm.contains("instant: true") && confirm.contains("escape: () => closeAsk(null)"),
        "the window's confirm lost its instant exit or its Escape:\n{confirm}"
    );
    let asking = block_after(window, "function askConfirm(spec) {");
    assert!(
        asking.contains("raiseAsk({ kind: \"confirm\"") && asking.contains("new Promise("),
        "askConfirm no longer puts its question in line:\n{asking}"
    );
}

/// A question is told from a tool by the `kind` its frame names — never by
/// what the tool is called. zo's refusal ladder once asked its model-switch
/// question as a tool named `safety-classifier decline`, and the window drew
/// it as one: a tool chip, a risk line, and 「이 도구를 실행할까요?」.
#[test]
fn a_question_is_told_from_a_tool_by_its_kind_and_never_by_its_tool_string() {
    let window = window_source();
    let raising = block_after(window, "function raisePermission(session, frame) {");
    assert!(
        raising.contains("frame.kind === \"question\" ? \"question\" : \"tool\""),
        "the frame's kind no longer picks the ask:\n{raising}"
    );
    assert!(
        !raising.contains("tool_name") && !window.contains("safety-classifier"),
        "the window recognises a question by a tool string:\n{raising}"
    );
    // A question names no tool and weighs no risk, whatever else its frame carries.
    let question = block_after(window, "registerAskKind(\"question\", {");
    assert!(
        !question.contains("tool_name") && !question.contains("audit_hint"),
        "a question draws a tool chip or a risk line:\n{question}"
    );
    // Its title is chosen by the topic the frame names, in the window's
    // languages, before it falls back to the title the frame carries.
    let title = block_after(window, "function askQuestionTitle(frame) {");
    assert!(
        title.contains("Object.hasOwn(ASK_TOPICS, frame.topic)")
            && title.contains("frame.title")
            && title.contains("\"ask.question.title\""),
        "a question's title lost the topic, the frame's title, or the generic one:\n{title}"
    );
    let topics = block_after(window, "const ASK_TOPICS = Object.freeze({");
    for topic in ["model_switch", "declined_images"] {
        assert!(
            topics.contains(topic),
            "the topic `{topic}` has no words:\n{topics}"
        );
    }
}

/// One keyboard, whatever the kind: Enter gives the SAFE answer of its kind and
/// never the riskiest, Escape refuses or cancels, and one key press answers one
/// ask — a key held down repeats, and a repeat is not an answer to the next ask.
#[test]
fn enter_is_the_safe_answer_of_its_kind_and_a_held_key_answers_once() {
    let window = window_source();

    // The last step, where a press cannot be taken back: Enter refuses.
    let confirm = block_after(window, "registerAskKind(\"computer-confirm\", {");
    assert!(
        confirm.contains("allow: false, tone: \"primary\"")
            && confirm.contains("initial: 0,")
            && confirm.contains("safe: 0,"),
        "Enter on the last-step question no longer refuses:\n{confirm}"
    );
    // The hand-over: Enter answers nothing — the keyboard starts on the frame.
    let handoff = block_after(window, "registerAskKind(\"computer-handoff\", {");
    assert!(
        handoff.contains("initial: null,") && handoff.contains("safe: 0,"),
        "Enter on the hand-over says something for the person:\n{handoff}"
    );
    let painting = block_after(window, "function paintAsk() {");
    assert!(
        painting.contains("view.initial === null ? askFrame"),
        "a kind that starts on nothing starts on a button:\n{painting}"
    );
    assert!(
        ui_file("index.html").contains(r#"data-kind="" tabindex="-1">"#),
        "the frame cannot hold the keyboard for a kind whose Enter answers nothing"
    );
    // A question: the once-only choice, never the one that stays.
    let initial = block_after(window, "function askQuestionInitial(choices) {");
    assert!(
        initial.contains("\"allow_once\"")
            && initial.contains("\"deny\"")
            && !initial.contains("allow_always"),
        "Enter on a question can pick the choice that stays:\n{initial}"
    );
    // A tool prompt is what it always was: the first choice holds the keyboard.
    let tool = block_after(window, "registerAskKind(\"tool\", {");
    assert!(
        tool.contains("initial: 0,"),
        "the tool prompt's keyboard moved:\n{tool}"
    );
    // Escape refuses with the plain `deny` — the one that is not remembered.
    assert!(
        tool.contains("choice.decision === \"deny\""),
        "Escape on a tool prompt can record a refusal for good:\n{tool}"
    );

    // One key, one answer.
    assert!(
        window.contains("askScrim.addEventListener(\n  \"keydown\",")
            && window.contains(
                "if (!event.repeat || (event.key !== \"Enter\" && event.key !== \" \")) return;"
            ),
        "a repeating Enter or Space can answer the next ask"
    );
    let input = ui_file("shell-input.js");
    assert!(
        input.contains("ignoresRepeat: true")
            && input.contains("if (event.repeat && entry.spec.ignoresRepeat === true) return;"),
        "a repeating Escape can refuse the next ask"
    );
}

/// No agent is named in the popup: who is asking comes from the catalog, by
/// the lane or the pane that holds the session, and a session nobody holds
/// is known by the session alone. The chip once read `ZO · <session>` for every
/// agent's question.
#[test]
fn the_popup_names_no_agent_of_its_own() {
    let window = window_source();
    let popup = {
        let start = window
            .find("/* ---- the ask popup ---- */")
            .expect("the ask popup's section");
        let end = window[start..]
            .find("/* ---- threads ---- */")
            .map(|at| start + at)
            .expect("the section that follows the ask popup");
        &window[start..end]
    };
    let mut spelled: Vec<String> = zerocode_core::AGENT_SPECS
        .iter()
        .map(|spec| spec.id.to_string())
        .collect();
    spelled.push("agy".to_string());
    for id in &spelled {
        assert!(
            !popup.contains(&format!("\"{id}\"")),
            "the ask popup spells the agent `{id}`"
        );
    }
    assert!(
        !popup.contains("`ZO ·") && !popup.contains("\"ZO"),
        "the ask popup writes an agent's name in capitals"
    );
    let asker = block_after(window, "function askerOf(session) {");
    assert!(
        asker.contains("agentName(slug)") && asker.contains("paneTermsBySession(session)"),
        "the asker is not named from the catalog by the lane or the pane:\n{asker}"
    );
    // The Computer Use kinds name the feature through the catalog's own key,
    // not a literal.
    let computer = ui_file("shell-computer.js");
    assert!(
        computer.matches("t(\"computerUse.title\"").count() == 2,
        "a Computer Use ask names its source without the catalog"
    );
}

/// The popup is dressed, once, by kind — and its clock is a fact, not motion.
#[test]
fn the_popup_is_dressed_by_kind_and_carries_no_motion_of_its_own() {
    let sheet = ui_file("shell.css");
    let frame = block_after_css(&sheet, ".permission.ask {");
    assert!(
        frame.contains("--ask-edge") && frame.contains("var(--shadow-float)"),
        "the popup lost its edge or its float shadow:\n{frame}"
    );
    for kind in ["computer-confirm", "confirm"] {
        assert!(
            sheet.contains(&format!(".permission.ask[data-kind=\"{kind}\"]")),
            "the `{kind}` kind is not dressed apart"
        );
    }
    // Reduced motion is the sheet's one global rule; the popup adds no
    // animation of its own that the rule would not reach.
    assert!(
        !frame.contains("animation")
            && !block_after_css(&sheet, ".ask-clock {").contains("animation"),
        "the popup carries motion of its own"
    );
}
