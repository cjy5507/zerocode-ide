//! Every harness value shown by `--prompt-input` names its evidence.

#[expect(dead_code, reason = "this contract shares the isolated provider guard, without using its HTTP server")]
#[path = "support/custom_models.rs"]
mod fixture;
#[path = "support/profile_sources.rs"]
mod profile_sources;

use zo_ide::prompt_input::PromptInput;

/// Walk every value in the profile block, including any future capability row.
#[test]
fn every_profile_value_has_a_visible_source() {
    let _own = fixture::OwnProviders::new();
    for model in [
        "claude-opus-5-5",
        "gpt-6.1-sol",
        "gemini-3.6-flash",
        "deepseek-flash",
        "acme-coder-1",
    ] {
        let input = PromptInput::measure(model, "/w", &["# Zo\nfixed".to_string()], &[], &[]);
        profile_sources::assert_sources(&input.render());
    }
}
