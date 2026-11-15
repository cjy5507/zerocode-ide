//! Every harness value shown by `--prompt-input` names its evidence.

use zo_ide::prompt_input::PromptInput;

/// Use the public report API, so this also runs against the unchanged resolver.
#[test]
fn every_profile_value_has_a_visible_source() {
    for model in [
        "claude-opus-5-5",
        "gpt-6.1-sol",
        "gemini-3.6-flash",
        "deepseek-flash",
        "acme-coder-1",
    ] {
        let input = PromptInput::measure(model, "/w", &["# Zo\nfixed".to_string()], &[], &[]);
        let rendered = input.render();
        assert!(
            rendered.lines().any(|line| {
                line.trim().strip_prefix("Edit tool source").is_some_and(|source| {
                    matches!(source.trim(), "measured" | "declared" | "inherited" | "default")
                })
            }),
            "profile value has no visible source for {model}:\n{rendered}"
        );
    }
}
