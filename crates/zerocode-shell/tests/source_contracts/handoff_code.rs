//! The one-time code on the person's-turn card (t-40807): what a test of one
//! function cannot see from inside it. The behaviour is the pending table's
//! and the door's own tests (`computer_use::confirm`, `main_unit_tests`), core's
//! rule (`handoff_code`) and the window suite (`ui/tests/ask-popup.mjs`); these
//! contracts hold where a code may be answered from, where a value may and may
//! not be written, what the page reads of it and keeps of it, and that no agent,
//! no helper and no second road is named on it.

use std::path::{Path, PathBuf};

use super::quiet_children::{code_only, shell_sources};
use super::support::{block_after, block_after_css, shipped_backend};

fn ui_file(name: &str) -> String {
    let ui = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ui");
    std::fs::read_to_string(ui.join(name)).unwrap_or_else(|why| panic!("{name}: {why}"))
}

fn skill(name: &str) -> String {
    let skills = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../skills");
    std::fs::read_to_string(skills.join(name).join("SKILL.md"))
        .unwrap_or_else(|why| panic!("skills/{name}: {why}"))
}

/// The pending table and the dispatch; a sibling module, read through its own
/// `include_str!` like the others.
const CONFIRM: &str = include_str!("../../src/computer_use/confirm.rs");
/// The rule for a code, in core.
const CODE_RULE: &str = include_str!("../../../zerocode-core/src/handoff_code.rs");
const MAIN: &str = include_str!("../../src/main.rs");

/// A file's shipped half: everything before its first test module.
fn shipped(source: &str) -> &str {
    source
        .split_once("#[cfg(test)]")
        .map_or(source, |(code, _)| code)
}

/// The window's shipped Rust, comments and strings off, by file — the
/// tests' own files and test folders left out.
fn shipped_sources() -> Vec<(String, String)> {
    shell_sources()
        .into_iter()
        .filter(|(name, _)| {
            name != "main_unit_tests.rs" && !name.ends_with("tests.rs") && !name.contains("/tests/")
        })
        .map(|(name, text)| (name, code_only(shipped(&text))))
        .collect()
}

/// A code is answered from one place: the window's page, by one command that
/// names the card and hands what was typed to the one function that judges it.
/// An agent, a hook, a second door or a recipe has no road to it — and the
/// plain answer (`confirm::answer`) is the page's command too, never anyone's.
#[test]
fn only_the_windows_page_answers_a_card_and_its_line() {
    let backend = shipped_backend();
    assert_eq!(
        backend.matches("fn computer_handoff_code(").count(),
        1,
        "the code has not exactly one command"
    );
    let command = block_after(backend, "fn computer_handoff_code(");
    assert!(
        command.contains("let _crumb = crate::crumbs::Command::enter(\"computer_handoff_code\");"),
        "the code's command lost its breadcrumb:\n{command}"
    );
    assert!(
        command.contains("computer_use::confirm::answer_code("),
        "the code's command does not hand the code to the one function that judges it:\n{command}"
    );
    // Registered with the commands the page may call, once.
    let handlers = &MAIN[MAIN
        .find("generate_handler![")
        .expect("the window's command list")..];
    let handlers = &handlers[..handlers.find("])").expect("the command list ends")];
    assert_eq!(
        handlers.matches("computer_handoff_code,").count(),
        1,
        "the code's command is not registered once in the window's command list"
    );

    let sources = shipped_sources();
    for (call, only_in) in [
        ("answer_code(", "cmd/settings.rs"),
        ("confirm::answer(", "cmd/settings.rs"),
    ] {
        let callers: Vec<&str> = sources
            .iter()
            .filter(|(name, code)| name != "computer_use/confirm.rs" && code.contains(call))
            .map(|(name, _)| name.as_str())
            .collect();
        assert_eq!(
            callers,
            [only_in],
            "`{call}` — what a person said — is called from somewhere other than the page's command"
        );
    }
}

/// A code is a length to everything that writes: no log line, no notice to
/// the page, no record, no panic message holds more. The type has no `Clone`,
/// no `Display`, no `Serialize` and a `Debug` that counts; the one place it is
/// read out is the answer built for the agent that asked.
#[test]
fn a_code_is_written_in_no_log_no_notice_and_no_record() {
    let confirm = code_only(shipped(CONFIRM));
    for loud in [
        "tracing::",
        "println!",
        "eprintln!",
        "dbg!",
        "log::",
        ".emit(",
    ] {
        assert!(
            !confirm.contains(loud),
            "the pending table writes somewhere (`{loud}`) where a code could land"
        );
    }

    let rule = shipped(CODE_RULE);
    let at = rule
        .find("pub struct OneTimeCode")
        .expect("the code type is gone");
    let head: Vec<&str> = rule[..at].lines().rev().take(4).collect();
    assert!(
        head.iter().all(|line| !line.contains("derive(")),
        "the code type derives a trait (Clone, Serialize, PartialEq…) that copies or writes it:\n{head:?}"
    );
    for forbidden in [
        "impl Clone for OneTimeCode",
        "impl fmt::Display for OneTimeCode",
        "impl Serialize for OneTimeCode",
        "impl std::fmt::Display for OneTimeCode",
    ] {
        assert!(!rule.contains(forbidden), "the code type has `{forbidden}`");
    }
    let debug = block_after(rule, "impl fmt::Debug for OneTimeCode {");
    assert!(
        debug.contains("self.chars()") && !debug.contains("self.0"),
        "the code's Debug prints more than how long it is:\n{debug}"
    );

    // The window tells the page which card and how it ended — never the answer.
    let backend = shipped_backend();
    let asker = block_after(backend, "fn install_confirm_asker(");
    assert!(
        asker.contains("confirm::closed_said(")
            && !asker.contains(".code")
            && !asker.contains("reveal"),
        "the window's asker touches the code on its way to the page:\n{asker}"
    );

    let mut revealing: Vec<String> = shipped_sources()
        .into_iter()
        .filter(|(_, code)| code.contains(".reveal()"))
        .map(|(name, _)| name)
        .collect();
    revealing.sort();
    assert_eq!(
        revealing,
        ["computer_use/confirm.rs"],
        "a code is read out in more than the one answer built for the agent that asked"
    );
}

/// No agent is named on the code's road — core's rule, the pending table and
/// the page's card read one table and one door, whoever is waiting.
#[test]
fn no_agent_is_named_on_the_code_road() {
    let mut spelled: Vec<String> = zerocode_core::AGENT_SPECS
        .iter()
        .map(|spec| spec.id.to_string())
        .collect();
    spelled.push("agy".to_string());
    let card = ui_file("shell-computer.js");
    for (what, text) in [
        ("computer_use/confirm.rs", shipped(CONFIRM)),
        ("zerocode-core/src/handoff_code.rs", shipped(CODE_RULE)),
        ("ui/shell-computer.js", card.as_str()),
    ] {
        for id in &spelled {
            assert!(
                !text.contains(&format!("\"{id}\"")),
                "{what} spells the agent `{id}` on the code's road"
            );
        }
    }
}

/// The signed helper knows nothing of the card: the line, the code and the
/// refusal of a secret are the window's, so the helper is unchanged and the
/// same on every machine.
#[test]
fn the_signed_helper_knows_nothing_of_the_code_card() {
    let native = Path::new(env!("CARGO_MANIFEST_DIR")).join("native/computer-use-macos");
    let mut pending = vec![native];
    let mut swift = 0;
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(&directory).expect("the helper's folder") {
            let path: PathBuf = entry.expect("a helper entry").path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().and_then(|one| one.to_str()) == Some("swift") {
                swift += 1;
                let text = std::fs::read_to_string(&path).expect("a helper source");
                for word in ["askCode", "ask-code", "codeAsk", "OneTimeCode"] {
                    assert!(
                        !text.contains(word),
                        "{} knows `{word}`: the helper is unchanged by the code card",
                        path.display()
                    );
                }
            }
        }
    }
    assert!(swift > 3, "the helper walk found only {swift} Swift files");
}

/// The page paints what the window sends and holds nothing of its own: the
/// limits come in the card's payload, the field's length from the same table,
/// the verdict on a typed code from the window.
#[test]
fn the_card_reads_its_limits_and_its_verdicts_from_the_window() {
    let card = ui_file("shell-computer.js");
    assert!(
        card.contains("ask.payload.codeAsk"),
        "the card does not read the limits the window sends"
    );
    assert_eq!(
        card.matches("maxLength").count(),
        1,
        "the field's length is set more than once, or from a number of the page's own"
    );
    assert!(
        card.contains("computerCodeInput.maxLength = limits.typedMax;"),
        "the field's room is not the window's typed maximum"
    );
    // The verdict words are the enum's serde names, one for one.
    let confirm = shipped(CONFIRM);
    let verdicts = block_after(confirm, "pub enum CodeVerdict {");
    assert!(
        confirm.contains("#[serde(rename_all = \"lowercase\")]\npub enum CodeVerdict {"),
        "the verdicts are no longer spelled in lowercase on the wire"
    );
    for (variant, word) in [
        ("Delivered", "\"delivered\""),
        ("Invalid", "\"invalid\""),
        ("Gone", "\"gone\""),
    ] {
        assert!(
            verdicts.contains(variant) && card.contains(word),
            "the page and the window disagree on the verdict {word}"
        );
    }
    // A refusal the card painted itself leaves the ask standing: the popup's
    // answer road knows it by its mark, and the card says it with that mark.
    let term = ui_file("shell-term.js");
    assert!(
        block_after(&term, "async function answerAsk(choice) {")
            .contains("error?.askHeld === true")
            && card.contains("askHeld: true"),
        "the popup and the card do not share the mark of a held answer"
    );
    // The keyboard starts where the kind says, not only on a button or the frame.
    assert!(
        block_after(&term, "function paintAsk() {").contains("view.focus"),
        "a kind cannot start the keyboard in its own field"
    );
}

/// The typed code is read in one place, sent by one door, and gone from the
/// field the moment it has gone — the page keeps it in no store, no attribute,
/// no console and no window log.
#[test]
fn the_typed_code_is_read_once_sent_by_one_door_and_kept_nowhere() {
    let card = ui_file("shell-computer.js");
    assert_eq!(
        card.matches("= computerCodeInput.value;").count(),
        1,
        "the field is read in more than one place"
    );
    assert!(
        card.matches("computerCodeInput.value = \"\";").count() >= 2,
        "the field is not emptied where the code leaves and where the card does"
    );
    assert_eq!(
        card.matches("invoke(\"computer_handoff_code\", { id: ask.id, code: typed })")
            .count(),
        1,
        "the code leaves by more than the one door, or with more than its card's id"
    );
    for kept in [
        "localStorage",
        "sessionStorage",
        "console.",
        "tellWindowLog(",
        "setAttribute(\"value\"",
        "dataset.code",
    ] {
        assert!(
            !card.contains(kept),
            "the card's file touches `{kept}`, where a typed code could stay"
        );
    }
}

/// The field is labelled, described, wired for a one-time code and for nothing
/// else — not a password field, not an autofilled one — and drawn from the
/// window's tokens with a focus ring of its own.
#[test]
fn the_field_is_labelled_wired_for_a_code_and_drawn_from_the_tokens() {
    let markup = ui_file("index.html");
    let sheet = ui_file("shell.css");
    assert!(
        markup.contains("id=\"ask-code\" hidden>"),
        "the line is not hidden until a card asks for it"
    );
    assert!(
        markup.contains("for=\"ask-code-input\""),
        "the field has no label of its own"
    );
    let start = markup
        .find("<input class=\"ask-code-input\"")
        .expect("the code field");
    let tag = &markup[start..start + markup[start..].find('>').expect("the field's tag ends")];
    for needed in [
        "id=\"ask-code-input\"",
        "autocomplete=\"one-time-code\"",
        "spellcheck=\"false\"",
        "aria-describedby=\"ask-code-hint ask-code-error\"",
    ] {
        assert!(
            tag.contains(needed),
            "the code field lost `{needed}`: {tag}"
        );
    }
    assert!(
        !tag.contains("type=\"password\""),
        "the code field is a password field: a code is read back by the person who types it"
    );
    assert!(
        markup.contains("id=\"ask-code-error\"") && markup.contains("role=\"alert\""),
        "a refusal of a typed code is not announced"
    );
    let field = block_after_css(&sheet, ".ask-code-input {");
    assert!(
        !field.contains('#') && !field.contains("rgb(") && field.contains("var(--"),
        "the field is painted from something other than the window's tokens:\n{field}"
    );
    assert!(
        sheet.contains(".ask-code-input:focus-visible"),
        "the field has no focus ring of its own"
    );
}

/// The manuals teach the road: ask with `--ask-code`, take the value from the
/// answer into a `--value-stdin`/`--text-stdin` writer, and a password or a
/// card number is the person's, never asked this way.
#[test]
fn the_manuals_teach_the_code_road_and_keep_a_password_the_persons() {
    for (name, stdin) in [
        ("computer-use", "--value-stdin"),
        ("mobile-device", "--text-stdin"),
    ] {
        let manual = skill(name);
        assert!(
            manual.contains("zerocode-computer handoff --ask-code"),
            "{name} does not teach `handoff --ask-code`"
        );
        assert!(
            manual.contains(stdin),
            "{name} does not put the code into the field by `{stdin}`"
        );
        assert!(
            manual.contains("password") && manual.contains("card number"),
            "{name} does not say that a password or a card number is the person's"
        );
    }
}
