//! The fixed-harness budget gate (`harness-diet-r49.md`).
//!
//! r15 sets a cold fixed harness target of 8,700 tokens: system core 5,500,
//! tool schemas 2,000, skill index 900, idle reminders 300. r49 reaches those
//! ceilings and makes them the release contract rather than an approach
//! ratchet. The attainable wire ceiling is twelve; r15's eight-schema sketch
//! remains an experiment because the pinned coding surface does not fit it.
//!
//! What is measured here is the SHIPPED harness — the built-in system sections
//! and the wire tool advertisement, in an empty workspace with provider skill
//! discovery neutralized. A user's skills, CLAUDE.md and memory index are
//! theirs, not the product's, so they are held out — every home the global
//! config chain reads (`ZO_CONFIG_HOME`, `ZO_HOME`, `HOME`) points at one empty
//! temp dir; `zo --prompt-input` in a real repo reports the whole bill.
//!
//! One measurement, shared: every test here would otherwise set and clear the
//! same process-global env vars while the others read them.

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::OnceLock;

use zo_ide::prompt_input::{Bucket, PromptInput};

const SHIPPED_PROVIDER_FAMILIES: [api::ProviderKind; 3] = [
    api::ProviderKind::Anthropic,
    api::ProviderKind::OpenAi,
    api::ProviderKind::Google,
];

/// One representative per first-party provider family, derived from the
/// catalog shipped in this binary rather than copied model ids.
fn shipped_by_provider() -> &'static [(api::ProviderKind, PromptInput)] {
    static MEASURED: OnceLock<Vec<(api::ProviderKind, PromptInput)>> = OnceLock::new();
    MEASURED.get_or_init(|| {
        let home = tempfile::tempdir().expect("temp home");
        let workspace = tempfile::tempdir().expect("temp workspace");
        // A bare workspace AND a neutralized provider catalog: what the product
        // ships, with nothing of the developer's machine in it. Without the
        // provider vars the skill index reads external user catalogs, which do
        // not belong in this product-owned gate.
        std::env::set_var(core_types::paths::ZO_CONFIG_HOME_ENV, home.path());
        std::env::set_var("ZO_CLAUDE_HOME", "");
        std::env::set_var("ZO_CODEX_HOME", "");
        // The global config chain is ALL of `ZO_CONFIG_HOME` → `ZO_HOME` →
        // `~/.zo` (`core_types::paths::zo_global_config_roots`), not the first
        // hit, so a temp `ZO_CONFIG_HOME` alone still leaves the developer's
        // `~/.zo/skills` in the index. `HOME` and `ZO_HOME` point at the same
        // empty temp home for the measurement and are restored after it.
        let previous_home = std::env::var_os("HOME");
        let previous_zo_home = std::env::var_os(core_types::paths::ZO_HOME_ENV);
        std::env::set_var("HOME", home.path());
        std::env::remove_var(core_types::paths::ZO_HOME_ENV);
        // The window plants the second-brain vault into every pane it opens,
        // and the prompt names a configured vault in a section of its own —
        // 25 tokens the shipped product does not carry until somebody sets
        // one up. A run from inside a pane measured the developer's vault
        // (system core 6,425 against a 6,400 ratchet, 2026-09-03).
        std::env::remove_var(runtime::second_brain::VAULT_ENV);
        let measured = SHIPPED_PROVIDER_FAMILIES
            .into_iter()
            .map(|provider| {
                let model = api::builtin_provider_catalog()
                    .iter()
                    .find(|entry| entry.provider == provider)
                    .unwrap_or_else(|| panic!("shipped catalog has no {provider:?} model"))
                    .canonical_model_id;
                (provider, measure_harness(workspace.path(), model))
            })
            .collect();
        std::env::remove_var(core_types::paths::ZO_CONFIG_HOME_ENV);
        std::env::remove_var("ZO_CLAUDE_HOME");
        std::env::remove_var("ZO_CODEX_HOME");
        restore_env("HOME", previous_home);
        restore_env(core_types::paths::ZO_HOME_ENV, previous_zo_home);
        measured
    })
}

/// Put an env var back the way the measurement found it.
fn restore_env(key: &str, previous: Option<std::ffi::OsString>) {
    match previous {
        Some(value) => std::env::set_var(key, value),
        None => std::env::remove_var(key),
    }
}

fn shipped() -> &'static PromptInput {
    &shipped_by_provider()
        .first()
        .expect("Anthropic is the first shipped provider family")
        .1
}

fn measure_harness(workspace: &Path, model: &str) -> PromptInput {
    let mut system = runtime::load_system_prompt_for_main_with_mode(
        workspace,
        "2026-01-01",
        std::env::consts::OS,
        "unknown",
        Some(model),
        runtime::PromptMode::Interactive,
    )
    .expect("system prompt should build");
    system.push(tools::deferred_tool_manifest_section());
    let registry = tools::GlobalToolRegistry::builtin();
    registry.context().set_active_model(model);
    let definitions = registry.definitions(None);
    PromptInput::measure(
        model,
        &workspace.display().to_string(),
        &system,
        &definitions,
        &[],
    )
}

/// No prompt bullet may restate what a tool's own description already says.
///
/// Both ship on every request, so a sentence in both is paid twice. This is the
/// measurement that killed the "just de-duplicate the prompt" plan, kept as a
/// ratchet: run over the whole harness it found **one** offender — a bullet
/// 58% contained in the `Skill` tool description, saying nothing beyond "you
/// have a `Skill` tool", which the advertised tool already says. It is gone;
/// everything else scored under 0.10.
///
/// Coverage, not Jaccard: the question is "is this sentence already carried
/// elsewhere", which is asymmetric — a short bullet fully contained in a long
/// description is redundant even though their Jaccard is small.
#[test]
fn no_prompt_bullet_restates_a_tool_description() {
    /// Comfortably above the 0.10 noise floor every surviving pair sits under,
    /// and below the 0.58 the one real offender scored.
    const MAX_COVERAGE: f64 = 0.40;

    let sections = include_str!("../../runtime/src/prompt/sections.rs");
    let specs = include_str!("../../tools/src/misc_tools/specs.rs");

    let bullets = quoted_after(sections, ".to_string(),");
    let descriptions = quoted_after(specs, "");
    assert!(
        bullets.len() > 40,
        "the bullet scan stopped seeing the prompt: {} found",
        bullets.len()
    );
    assert!(
        descriptions.len() > 20,
        "the description scan stopped seeing the specs: {} found",
        descriptions.len()
    );

    let mut worst: Option<(f64, &str)> = None;
    for bullet in &bullets {
        let shingles = shingles(bullet);
        if shingles.len() < 8 {
            continue; // A heading or one-liner has no room to be redundant.
        }
        for description in &descriptions {
            let other = self::shingles(description);
            let shared = shingles.intersection(&other).count();
            #[allow(clippy::cast_precision_loss)] // counts, not measurements
            let coverage = shared as f64 / shingles.len() as f64;
            if worst.is_none_or(|(seen, _)| coverage > seen) {
                worst = Some((coverage, bullet));
            }
        }
    }
    let (coverage, bullet) = worst.expect("some pair was scored");
    assert!(
        coverage <= MAX_COVERAGE,
        "a prompt bullet is {coverage:.0} contained in a tool description — say it \
         in one place, and let the tool's own description be the one: {bullet:.160}"
    );
}

/// Every string literal in `source`, optionally only those followed by `suffix`.
fn quoted_after<'a>(source: &'a str, suffix: &str) -> Vec<&'a str> {
    let mut out = Vec::new();
    let bytes = source.as_bytes();
    let mut index = 0;
    while let Some(open) = source[index..].find('"') {
        let start = index + open + 1;
        let mut end = start;
        while end < bytes.len() {
            match bytes[end] {
                b'\\' => end += 2,
                b'"' => break,
                _ => end += 1,
            }
        }
        if end >= bytes.len() {
            break;
        }
        let literal = &source[start..end];
        if literal.len() > 60 && (suffix.is_empty() || source[end + 1..].starts_with(suffix)) {
            out.push(literal);
        }
        index = end + 1;
    }
    out
}

/// Word 4-grams, lowercased — the unit both texts are compared in.
fn shingles(text: &str) -> std::collections::BTreeSet<Vec<String>> {
    let words: Vec<String> = text
        .split(|c: char| !c.is_ascii_alphabetic())
        .filter(|word| !word.is_empty())
        .map(str::to_ascii_lowercase)
        .collect();
    words.windows(4).map(<[String]>::to_vec).collect()
}

/// The gate measures the product, not the machine: with every user root held
/// out, the shipped harness advertises no skills at all, so the skill bucket
/// reads zero. A non-zero value here means a developer's own `~/.zo/skills`
/// (or another home) leaked into the measurement — the 2026-09-10 false red,
/// when three skills installed on this machine took a gate that had passed
/// at 08:25 to 922/900 with no product change.
#[test]
fn the_shipped_harness_carries_no_machine_skills() {
    for (provider, measured) in shipped_by_provider() {
        assert_eq!(
            measured.bucket_tokens(Bucket::Skills),
            0,
            "{provider:?} indexed skills from this machine, not the product:\n{}",
            measured.render()
        );
    }
}

/// Every provider family's product-owned fixed-harness buckets must remain
/// inside the r49 budget.
/// A schema added to the wire is paid by every turn, including ones that never
/// call it, so any future growth must first free room elsewhere.
#[test]
fn every_shipped_provider_family_meets_the_r49_budget() {
    let failures = shipped_by_provider()
        .iter()
        .filter_map(|(provider, measured)| {
            eprintln!("{provider:?}: system={}, tools={}, skills={}, reminders={}",
                measured.bucket_tokens(Bucket::System), measured.bucket_tokens(Bucket::Tools),
                measured.bucket_tokens(Bucket::Skills), measured.bucket_tokens(Bucket::Reminders));
            let overruns = measured.overruns();
            (!overruns.is_empty()).then(|| {
                format!(
                    "{provider:?} / {}:\n  {}\n{}",
                    measured.model,
                    overruns.join("\n  "),
                    measured.render()
                )
            })
        })
        .collect::<Vec<_>>();
    assert!(
        failures.is_empty(),
        "a shipped provider family's fixed harness broke the r49 budget:\n{}",
        failures.join("\n")
    );
}

/// Name what is left on the wire, so the next cut is chosen from measurement.
///
/// `SpawnMultiAgent`, the plan-settings pair, `Agent` (1,168) and
/// `AskUserQuestion` (597) went behind `CapabilityInvoke` in r49. With the two
/// largest line items off, the budget no longer had a head: the top three fell
/// from 42% of the bucket to 31.9%. t-2903 brought `Agent` back in
/// `ExitPlanModeV2`'s seat — as a schema cut to what a call needs, not the
/// 1,200-token routing essay — and this ratchet is what proves the return did
/// not re-concentrate the bucket.
///
/// So this test flipped direction. It used to demand concentration ("the next
/// cut is a few schemas"); that conclusion is spent, and it now guards against
/// re-concentration — a new schema large enough to move the top three back over
/// 42% owes the same per-request arithmetic `Agent` was given.
#[test]
fn the_wire_budget_has_no_head_left_to_aim_at() {
    let measured = shipped();

    let mut schemas: Vec<_> = measured
        .pieces
        .iter()
        .filter(|piece| piece.bucket == Bucket::Tools && piece.label.starts_with("tool "))
        .collect();
    schemas.sort_by(|left, right| right.est_tokens.cmp(&left.est_tokens));
    let names: BTreeSet<&str> = schemas
        .iter()
        .take(3)
        .map(|piece| piece.label.trim_start_matches("tool "))
        .collect();
    // The assertion names no tools so description edits cannot freeze a stale
    // ranking into the contract.
    assert!(
        !names.is_empty() && names.len() == 3,
        "the wire budget must still have a measurable top: {names:?}"
    );
    // The surface has FLATTENED, and that is the finding.
    //
    // It was 42% in the top three (2,388 of 5,654) before deferral flattened
    // the surface. r49 then shortened descriptions across the remaining
    // schemas; no single tool now closes a meaningful future overrun alone.
    //
    // Kept as a ratchet in the other direction. If the top three climb back
    // over 42%, something large has been added to the wire and deserves the
    // same per-request arithmetic `Agent` got.
    let top_three: u64 = schemas.iter().take(3).map(|piece| piece.est_tokens).sum();
    let total = measured.bucket_tokens(Bucket::Tools);
    assert!(
        top_three * 100 <= total * 42,
        "the wire budget re-concentrated ({top_three} of {total} in the top \
         three) — a new schema is large enough to need its own justification"
    );
}

/// Deferring a tool must never make it unreachable.
///
/// The whole trade is "off the wire, still callable": if a deferred name
/// stopped resolving, the cut would be a capability loss wearing a token win.
#[test]
fn every_tool_taken_off_the_wire_is_still_reachable_by_name() {
    let registry = tools::GlobalToolRegistry::builtin();
    let advertised: BTreeSet<String> = registry
        .definitions(None)
        .into_iter()
        .map(|definition| definition.name)
        .collect();
    for name in [
        "SpawnMultiAgent",
        "ListAgents",
        "PushNotification",
        "EnterPlanMode",
        "ExitPlanMode",
        "ExitPlanModeV2",
        "Workflow",
        "session_recall",
        "MemoryWrite",
        "retrieve_tool_output",
        "send_to_user",
        "read_image",
    ] {
        assert!(!advertised.contains(name), "{name} is still advertised");
        let found = registry.search(&format!("select:{name}"), 3, None, None);
        assert!(
            found.schemas.iter().any(|schema| schema.name == name),
            "{name} left the wire and cannot be found again — that is a capability loss"
        );
    }
}

/// The fixed harness has two ways to promise a tool to the model: a schema on
/// the wire, or a name in the deferred manifest. Both promises must terminate
/// at the registry built from those same tool specs. A wire name that falls
/// through to `NotFound` is worse than a missing schema: the model is explicitly
/// invited to call an address that the live executor rejects.
#[test]
fn every_fixed_harness_tool_name_is_registered_or_deferred_reachable() {
    let registry = tools::GlobalToolRegistry::builtin();
    let wire: BTreeSet<String> = registry
        .definitions(None)
        .into_iter()
        .map(|definition| definition.name)
        .collect();
    let resolvable = registry.resolvable_tool_names();

    for name in &wire {
        let outcome = registry.execute(name, &serde_json::Value::Null);
        assert!(
            !matches!(outcome, Err(tools::ToolError::NotFound(_))),
            "fixed harness advertises `{name}`, but the registry rejects that exact name"
        );
    }

    let manifest = tools::deferred_tool_manifest_section();
    for name in resolvable.difference(&wire) {
        assert!(
            manifest.contains(name),
            "registry defers `{name}`, but the fixed-harness manifest never promises it"
        );
        let found = registry.search(&format!("select:{name}"), 1, None, None);
        assert!(
            found.schemas.iter().any(|schema| schema.name == *name),
            "fixed-harness deferred name `{name}` has no registry schema"
        );
    }
}

#[test]
fn artifact_delivery_is_part_of_the_built_system_prompt() {
    let sections = runtime::SystemPromptBuilder::new().build();
    let style = sections.iter().find(|section| section.starts_with("# Response Style Contract")).unwrap();
    assert!(style.contains("Artifact") && style.contains("link"));
    assert!(style.contains("unrequested") && style.contains("one line"));
}
