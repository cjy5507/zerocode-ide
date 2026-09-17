//! Deterministic skill auto-routing.
//!
//! Zo lists discovered skills in the system prompt (`# Available skills`) and
//! lets the model call the `Skill` tool when it judges one relevant. That is
//! Claude Code's "load when relevant" behavior, but it depends entirely on the
//! model noticing. This module adds Codex-style *implicit invocation*: a cheap,
//! model-free matcher scores each active skill's trigger metadata
//! (`SkillTriggers`) against the current turn and, when a skill clearly fits,
//! injects an advisory recommendation reminder nudging the model to load it.
//!
//! The recommendation is always advisory. It never force-loads a skill body and
//! never bypasses the `Skill` tool's existing `state: proposed` gate (proposed
//! skills are excluded from the index before they ever reach this matcher). The
//! result is the union of both products' strengths: Codex trigger-word matching,
//! Claude Code relevance-gated loading, and Zo's safe lazy `Skill` execution.
//!
//! Matching signals, strongest first: explicit skill-name mention → curated
//! trigger keywords/paths → description-derived salient tokens (the fallback
//! for the majority of skills that ship no `triggers:` frontmatter). Every
//! signal derives from the skill files themselves — no hardcoded domain
//! vocabulary. Cross-language relevance (e.g. a Korean design request hitting
//! an English-described design skill) is deliberately NOT this matcher's job:
//! that is semantic judgment, which the model-led path owns — the prompt's
//! `# Available skills` index instructs the model to load a matching skill in
//! any language before planning.

use crate::prompt::{SkillIndexEntry, SkillInvocationMode};

/// Reminder prefix so the live turn loop can replace a prior turn's
/// recommendation with [`crate::ConversationRuntime::replace_transient_system_reminder_by_prefix`]
/// (no stale skill nudge lingers across turns).
pub const SKILL_RECOMMENDATION_REMINDER_PREFIX: &str = "[zo:skill-routing]";

/// Score at/above which an `Auto`-mode skill is recommended to *load*.
const AUTO_THRESHOLD: f32 = 0.75;
/// Score at/above which any matching skill is *suggested*. Set to a single
/// keyword's weight so one curated trigger word surfaces the skill as an option
/// (Codex's "a description/keyword match is worth considering"), while loading
/// still requires the stronger auto threshold or an explicit mention.
const SUGGEST_THRESHOLD: f32 = 0.30;

/// Per-category score weights. Kept small and additive so several weak signals
/// combine into a suggestion while one strong signal (explicit mention, or a
/// path plus keyword) reaches the auto threshold.
const EXPLICIT_MENTION_SCORE: f32 = 1.0;
const KEYWORD_SCORE: f32 = 0.30;
const PATH_SCORE: f32 = 0.45;
const EXCLUDE_PENALTY: f32 = 0.60;
/// Caps so a skill that lists many keywords/paths cannot dominate purely by
/// listing more triggers than its peers.
const KEYWORD_CAP: f32 = 0.60;
const PATH_CAP: f32 = 0.90;
/// Description-derived token match: the weak fallback signal for the majority
/// of skills that ship a rich `description` but no curated `triggers`
/// frontmatter — before this, such skills were unreachable by auto-routing
/// (keyword score 0) unless the user typed the skill name verbatim. Two
/// description-word hits reach [`SUGGEST_THRESHOLD`]; the cap keeps a long
/// keyword-stuffed description from ever outranking curated triggers.
const DESCRIPTION_TOKEN_SCORE: f32 = 0.15;
const DESCRIPTION_CAP: f32 = 0.45;
/// Minimum length for a description token to be considered salient.
const MIN_DESCRIPTION_TOKEN_LEN: usize = 4;
/// English function words common in skill descriptions; matching these would
/// be noise, not signal.
const DESCRIPTION_STOPWORDS: &[&str] = &[
    "this", "that", "with", "when", "user", "users", "uses", "use", "skill", "skills", "from",
    "into", "your", "them", "then", "than", "also", "have", "will", "should", "would", "about",
    "every", "each", "other", "only", "more", "most", "some", "such", "very", "like", "just",
    "over", "under", "before", "after", "include", "includes", "including", "invoke", "invoked",
    "trigger", "triggers", "asks", "want", "wants", "whenever", "always", "never", "their",
    "these", "those", "what", "which", "auto", "loads",
];


/// What the matcher decided for one skill.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillDecision {
    /// Strong match — recommend the model load it before planning.
    Load,
    /// Weak/medium match — surface it as an option.
    Suggest,
    /// No actionable match.
    Ignore,
}

/// A scored recommendation for a single skill.
#[derive(Debug, Clone, PartialEq)]
pub struct SkillRecommendation {
    pub name: String,
    pub score: f32,
    pub decision: SkillDecision,
    /// Human-readable match reasons, for the reminder and for `ZO_*_DEBUG`.
    pub reasons: Vec<String>,
}

/// Read-only inputs for one turn's match pass.
#[derive(Debug, Clone, Copy)]
pub struct SkillMatchInput<'a> {
    /// The current user turn text (the request itself).
    pub user_text: &'a str,
    /// Paths the turn is known to touch (e.g. from the request or recent edits).
    /// May be empty; trigger paths are also matched against `user_text`.
    pub touched_paths: &'a [String],
}

/// Score every skill and return the actionable recommendations (decision other
/// than [`SkillDecision::Ignore`]), highest score first. Deterministic and
/// allocation-light; safe to call on every turn.
#[must_use]
pub fn recommend_skills(
    input: &SkillMatchInput<'_>,
    skills: &[SkillIndexEntry],
) -> Vec<SkillRecommendation> {
    let user_lc = input.user_text.to_lowercase();
    let user_tokens = salient_tokens(&user_lc);
    let touched_lc: Vec<String> = input
        .touched_paths
        .iter()
        .map(|path| path.to_lowercase())
        .collect();

    let mut recommendations: Vec<SkillRecommendation> = skills
        .iter()
        .filter_map(|skill| score_skill(skill, &user_lc, &touched_lc, &user_tokens))
        .collect();

    // Highest score first; ties broken by name for stable, testable ordering.
    recommendations.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.name.cmp(&b.name))
    });
    recommendations
}

/// Build an advisory system-prompt reminder from the matcher output, or `None`
/// when nothing actionable matched. Surfaces at most `MAX_SURFACED` skills so
/// the reminder stays compact, and always leads with the strongest match.
#[must_use]
pub fn build_skill_recommendation_reminder(
    recommendations: &[SkillRecommendation],
) -> Option<String> {
    /// Cap on skills named in a single reminder (keeps the nudge terse).
    const MAX_SURFACED: usize = 2;

    let actionable: Vec<&SkillRecommendation> = recommendations
        .iter()
        .filter(|rec| rec.decision != SkillDecision::Ignore)
        .take(MAX_SURFACED)
        .collect();
    if actionable.is_empty() {
        return None;
    }

    let any_load = actionable
        .iter()
        .any(|rec| rec.decision == SkillDecision::Load);
    let mut lines = vec![format!(
        "{SKILL_RECOMMENDATION_REMINDER_PREFIX} This turn matches {} discovered skill(s) by trigger words/paths. {}",
        actionable.len(),
        if any_load {
            "Call the `Skill` tool to load the strongest match before planning, unless the user opted out or it is clearly irrelevant."
        } else {
            "Consider loading one with the `Skill` tool if it fits; otherwise proceed normally."
        }
    )];
    for rec in actionable {
        let verb = match rec.decision {
            SkillDecision::Load => "load",
            SkillDecision::Suggest => "consider",
            SkillDecision::Ignore => continue,
        };
        lines.push(format!(
            " - `{}` ({}, score {:.2}): {}",
            rec.name,
            verb,
            rec.score,
            rec.reasons.join("; ")
        ));
    }
    Some(lines.join("\n"))
}

/// Whether `haystack` contains `needle` on word boundaries (non-alphanumeric
/// or string edge on both sides). Plain substring `contains` made a
/// single-letter skill name like `x` match ANY text containing that letter
/// ("index.html" → recommend the `x` debate skill), and a keyword like `test`
/// match "latest".
fn contains_word(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return false;
    }
    let mut search_from = 0;
    while let Some(found) = haystack[search_from..].find(needle) {
        let start = search_from + found;
        let end = start + needle.len();
        let before_ok = haystack[..start]
            .chars()
            .next_back()
            .is_none_or(|c| !c.is_alphanumeric());
        let after_ok = haystack[end..]
            .chars()
            .next()
            .is_none_or(|c| !c.is_alphanumeric());
        if before_ok && after_ok {
            return true;
        }
        search_from = end;
    }
    false
}

/// Salient lowercase tokens of `text`: alphanumeric words at least
/// [`MIN_DESCRIPTION_TOKEN_LEN`] long that are not stopwords.
fn salient_tokens(text: &str) -> std::collections::HashSet<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|token| token.len() >= MIN_DESCRIPTION_TOKEN_LEN)
        .filter(|token| !DESCRIPTION_STOPWORDS.contains(token))
        .map(str::to_string)
        .collect()
}

/// Score one skill against the (already lowercased) turn signals. Returns `None`
/// when the skill is not actionable (no triggers, manual-only without an
/// explicit mention, excluded, or below the suggest threshold).
fn score_skill(
    skill: &SkillIndexEntry,
    user_lc: &str,
    touched_lc: &[String],
    user_tokens: &std::collections::HashSet<String>,
) -> Option<SkillRecommendation> {
    let mut reasons = Vec::new();

    // Explicit mention: the user named the skill (or its directory slug). Match
    // both the kebab/snake form and a space-normalized form so a natural-language
    // mention ("deep research") still hits a `deep-research` skill — previously
    // only the exact hyphenated token matched, so trigger-less skills were
    // effectively unreachable unless the user typed the slug verbatim.
    let name_lc = skill.name.to_lowercase();
    let name_spaced = name_lc.replace(['-', '_'], " ");
    let explicit = !name_lc.trim().is_empty()
        && (contains_word(user_lc, &name_lc) || contains_word(user_lc, &name_spaced));
    if explicit {
        reasons.push(format!("named `{}`", skill.name));
    }

    // Manual skills are recommended ONLY on an explicit mention.
    if skill.invocation_mode == SkillInvocationMode::Manual && !explicit {
        return None;
    }

    // Negative triggers veto the match regardless of other signals.
    for exclude in &skill.triggers.excludes {
        let exclude_lc = exclude.to_lowercase();
        if !exclude_lc.is_empty() && contains_word(user_lc, &exclude_lc) && !explicit {
            return None;
        }
    }

    let mut score = if explicit {
        EXPLICIT_MENTION_SCORE
    } else {
        0.0
    };

    // Keyword triggers (Codex-style trigger words).
    let mut keyword_score = 0.0;
    for keyword in &skill.triggers.keywords {
        let keyword_lc = keyword.to_lowercase();
        if !keyword_lc.is_empty() && contains_word(user_lc, &keyword_lc) {
            keyword_score += KEYWORD_SCORE;
            reasons.push(format!("keyword `{keyword}`"));
        }
    }
    score += keyword_score.min(KEYWORD_CAP);

    // Path triggers: match against both the declared touched paths and the
    // request text (requests frequently name the file directly).
    let mut path_score = 0.0;
    for trigger_path in &skill.triggers.paths {
        let trigger_lc = trigger_path.to_lowercase();
        if trigger_lc.is_empty() {
            continue;
        }
        let in_touched = touched_lc
            .iter()
            .any(|touched| touched.contains(&trigger_lc) || trigger_lc.contains(touched.as_str()));
        if in_touched || user_lc.contains(&trigger_lc) {
            path_score += PATH_SCORE;
            reasons.push(format!("path `{trigger_path}`"));
        }
    }
    score += path_score.min(PATH_CAP);

    // Description-derived tokens: the fallback signal for skills without
    // curated triggers (the common case — e.g. a design skill whose rich
    // description names "design", "frontend", "chart" but declares no
    // `triggers:` frontmatter). Whole-token equality, not substring, so short
    // incidental overlaps don't fire. Derived purely from the skill's own
    // file — no hardcoded domain vocabulary; cross-language matching is the
    // model-led path's job.
    if let Some(description) = skill.description.as_deref() {
        let mut description_score = 0.0;
        let mut matched = 0usize;
        for token in salient_tokens(description) {
            if user_tokens.contains(&token) {
                description_score += DESCRIPTION_TOKEN_SCORE;
                matched += 1;
                if matched <= 3 {
                    reasons.push(format!("description `{token}`"));
                }
            }
        }
        score += description_score.min(DESCRIPTION_CAP);
    }

    // Apply any exclude penalty that did not fully veto. Skipped for an
    // explicit mention: naming the skill is an unambiguous request and must not
    // be cancelled out by exclude words (and several excludes must never sum to
    // a negative score that drops a named skill).
    if !explicit {
        for exclude in &skill.triggers.excludes {
            let exclude_lc = exclude.to_lowercase();
            if !exclude_lc.is_empty() && contains_word(user_lc, &exclude_lc) {
                score -= EXCLUDE_PENALTY;
                reasons.push(format!("excluded by `{exclude}`"));
            }
        }
    }

    if score <= 0.0 {
        return None;
    }

    // `Load` when the user named the skill, or an `Auto`-mode skill cleared the
    // auto threshold; otherwise `Suggest` when it cleared the suggest threshold.
    let auto_load = skill.invocation_mode == SkillInvocationMode::Auto && score >= AUTO_THRESHOLD;
    let decision = if explicit || auto_load {
        SkillDecision::Load
    } else if score >= SUGGEST_THRESHOLD {
        SkillDecision::Suggest
    } else {
        return None;
    };

    Some(SkillRecommendation {
        name: skill.name.clone(),
        score,
        decision,
        reasons,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prompt::SkillTriggers;
    use std::path::PathBuf;

    fn skill(
        name: &str,
        mode: SkillInvocationMode,
        keywords: &[&str],
        paths: &[&str],
        excludes: &[&str],
    ) -> SkillIndexEntry {
        SkillIndexEntry {
            name: name.to_string(),
            description: Some(format!("{name} description")),
            path: PathBuf::from(format!("/repo/.zo/skills/{name}/SKILL.md")),
            invocation_mode: mode,
            triggers: SkillTriggers {
                keywords: keywords.iter().map(|s| (*s).to_string()).collect(),
                paths: paths.iter().map(|s| (*s).to_string()).collect(),
                excludes: excludes.iter().map(|s| (*s).to_string()).collect(),
            },
        }
    }

    fn input<'a>(user_text: &'a str, touched: &'a [String]) -> SkillMatchInput<'a> {
        SkillMatchInput {
            user_text,
            touched_paths: touched,
        }
    }

    #[test]
    fn keyword_plus_path_reaches_auto_load_for_auto_skill() {
        let skills = vec![skill(
            "zo-tui-render-performance",
            SkillInvocationMode::Auto,
            &["smooth rendering", "reveal"],
            &["crates/zo-cli/src/tui/app/reveal.rs"],
            &["react"],
        )];
        let recs = recommend_skills(
            &input(
                "smooth rendering 병목을 reveal.rs 의 crates/zo-cli/src/tui/app/reveal.rs 에서 고쳐줘",
                &[],
            ),
            &skills,
        );
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].decision, SkillDecision::Load);
        assert!(recs[0].score >= AUTO_THRESHOLD, "score {}", recs[0].score);
    }

    #[test]
    fn single_letter_skill_name_needs_a_word_boundary_mention() {
        // Live false positive: a skill literally named `x` was recommended for
        // "index.html ..." because substring `contains` matched the letter.
        let skills = vec![skill("x", SkillInvocationMode::Suggest, &[], &[], &[])];
        assert!(
            recommend_skills(&input("index.html 메인 페이지 개선해줘", &[]), &skills).is_empty()
        );
        // A word-bounded mention still hits (and an explicit mention loads).
        let recs = recommend_skills(&input("run the x skill on this", &[]), &skills);
        assert_eq!(recs.len(), 1, "{recs:?}");
        assert_eq!(recs[0].decision, SkillDecision::Load);
    }

    #[test]
    fn keywords_and_excludes_match_on_word_boundaries() {
        // Keyword `test` must not fire inside "latest"; exclude `react` must
        // not veto "reactive".
        let skills = vec![skill(
            "unit-runner",
            SkillInvocationMode::Suggest,
            &["test", "coverage"],
            &[],
            &["react"],
        )];
        assert!(
            recommend_skills(&input("show me the latest coverage darling", &[]), &skills).len()
                == 1,
            "coverage matches; 'latest' must not count as `test`"
        );
        let recs = recommend_skills(&input("reactive test coverage please", &[]), &skills);
        assert_eq!(recs.len(), 1, "'reactive' must not veto via `react`: {recs:?}");
    }

    #[test]
    fn cross_language_requests_are_left_to_the_model_led_path() {
        // No hardcoded translation table: a purely-Korean request produces no
        // deterministic match against an English description. Cross-language
        // relevance is semantic judgment, which the model-led path owns (the
        // `# Available skills` prompt index instructs the model to load a
        // matching skill in any language). Mixed-language requests still match
        // on their English tokens organically.
        let mut design = skill("ui-ux-pro-max", SkillInvocationMode::Suggest, &[], &[], &[]);
        design.description = Some(
            "UI/UX design intelligence for web and mobile. Styles, color palettes, \
             font pairings, frontend components and accessibility guidelines."
                .to_string(),
        );
        let korean_only = recommend_skills(
            &input("메인 페이지 디자인 좀 개선해줘", &[]),
            std::slice::from_ref(&design),
        );
        assert!(korean_only.is_empty(), "{korean_only:?}");

        let mixed = recommend_skills(
            &input("frontend design 스타일 개선해줘", &[]),
            std::slice::from_ref(&design),
        );
        assert_eq!(mixed.len(), 1, "{mixed:?}");
        assert_ne!(mixed[0].decision, SkillDecision::Ignore);
    }

    #[test]
    fn description_tokens_alone_can_suggest_but_are_capped() {
        // Organic (non-alias) description overlap is a weak signal: two salient
        // shared tokens reach Suggest, and stopword overlap contributes nothing.
        let mut research = skill("deep-dive", SkillInvocationMode::Suggest, &[], &[], &[]);
        research.description =
            Some("Fan-out web searches, adversarially verify claims, cited report".to_string());
        let recs = recommend_skills(
            &input("run web searches and give me a cited report", &[]),
            &[research],
        );
        assert_eq!(recs.len(), 1, "{recs:?}");
        assert_eq!(recs[0].decision, SkillDecision::Suggest);
        // Capped: even many shared tokens cannot exceed the description cap
        // (score stays below what curated keyword+path stacking can reach).
        assert!(
            recs[0].score <= DESCRIPTION_CAP + f32::EPSILON,
            "{recs:?}"
        );
    }

    #[test]
    fn unrelated_korean_request_matches_nothing() {
        let mut design = skill("ui-ux-pro-max", SkillInvocationMode::Suggest, &[], &[], &[]);
        design.description = Some("UI/UX design intelligence for web and mobile".to_string());
        let recs = recommend_skills(&input("커밋 로그 정리해줘", &[]), &[design]);
        assert!(recs.is_empty(), "{recs:?}");
    }

    #[test]
    fn natural_language_mention_matches_kebab_skill_name() {
        // A trigger-less skill is only reachable by name, so a natural-language
        // mention with spaces must still hit the kebab-cased skill — previously
        // only the exact `deep-research` token matched and "deep research" missed.
        let skills = vec![skill("deep-research", SkillInvocationMode::Manual, &[], &[], &[])];

        let spaced = recommend_skills(
            &input("can you do a deep research report on rust async", &[]),
            &skills,
        );
        assert_eq!(spaced.len(), 1, "spaced mention should match: {spaced:?}");
        assert_eq!(spaced[0].decision, SkillDecision::Load);

        // The exact slug still matches.
        let exact = recommend_skills(&input("please run deep-research now", &[]), &skills);
        assert_eq!(exact.len(), 1);
        assert_eq!(exact[0].decision, SkillDecision::Load);

        // An unrelated request still does not match the trigger-less skill.
        let miss = recommend_skills(&input("fix the failing build", &[]), &skills);
        assert!(miss.is_empty(), "unrelated request must not match: {miss:?}");
    }

    #[test]
    fn single_keyword_only_suggests_not_loads() {
        let skills = vec![skill(
            "zo-tui-render-performance",
            SkillInvocationMode::Auto,
            &["smooth rendering", "reveal"],
            &["crates/zo-cli/src/tui/app/reveal.rs"],
            &[],
        )];
        let recs = recommend_skills(&input("help me with smooth rendering", &[]), &skills);
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].decision, SkillDecision::Suggest);
    }

    #[test]
    fn exclude_word_vetoes_a_non_explicit_match() {
        let skills = vec![skill(
            "zo-tui-render-performance",
            SkillInvocationMode::Auto,
            &["rendering"],
            &[],
            &["react"],
        )];
        let recs = recommend_skills(
            &input("improve rendering in my react web dashboard", &[]),
            &skills,
        );
        assert!(recs.is_empty(), "exclude should veto: {recs:?}");
    }

    #[test]
    fn manual_skill_requires_explicit_mention() {
        let skills = vec![skill(
            "deep-research",
            SkillInvocationMode::Manual,
            &["research", "investigate"],
            &[],
            &[],
        )];
        // Keyword present but manual → not recommended.
        let recs = recommend_skills(&input("please research this topic", &[]), &skills);
        assert!(recs.is_empty(), "manual without mention: {recs:?}");

        // Explicit mention → loaded.
        let recs = recommend_skills(&input("use the deep-research skill", &[]), &skills);
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].decision, SkillDecision::Load);
    }

    #[test]
    fn explicit_mention_overrides_exclude() {
        let skills = vec![skill(
            "frontend-design",
            SkillInvocationMode::Suggest,
            &["dashboard"],
            &[],
            &["backend"],
        )];
        // Even though "backend" excludes, naming the skill forces a load.
        let recs = recommend_skills(
            &input("use frontend-design for my backend admin dashboard", &[]),
            &skills,
        );
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].decision, SkillDecision::Load);
    }

    #[test]
    fn explicit_mention_survives_multiple_exclude_matches() {
        // Regression: penalties from 2+ excludes must not sum below zero and
        // drop a skill the user named explicitly.
        let skills = vec![skill(
            "zo-tui-render-performance",
            SkillInvocationMode::Auto,
            &[],
            &[],
            &["react", "tailwind", "shadcn"],
        )];
        let recs = recommend_skills(
            &input(
                "use zo-tui-render-performance for my react tailwind shadcn wrapper",
                &[],
            ),
            &skills,
        );
        assert_eq!(recs.len(), 1, "explicit mention must survive: {recs:?}");
        assert_eq!(recs[0].decision, SkillDecision::Load);
        assert!(recs[0].score >= EXPLICIT_MENTION_SCORE);
    }

    #[test]
    fn touched_paths_match_even_without_text_mention() {
        let skills = vec![skill(
            "zo-tui-render-performance",
            SkillInvocationMode::Auto,
            &["reveal"],
            &["crates/zo-cli/src/tui/app/reveal.rs"],
            &[],
        )];
        let touched = vec!["crates/zo-cli/src/tui/app/reveal.rs".to_string()];
        let recs = recommend_skills(
            &input("fix the streaming reveal stutter", &touched),
            &skills,
        );
        assert_eq!(recs.len(), 1);
        // keyword (reveal) + path = 0.30 + 0.45 = 0.75 → auto load.
        assert_eq!(recs[0].decision, SkillDecision::Load);
    }

    #[test]
    fn skill_without_triggers_is_never_auto_recommended() {
        let skills = vec![skill(
            "legacy-skill",
            SkillInvocationMode::Suggest,
            &[],
            &[],
            &[],
        )];
        let recs = recommend_skills(&input("do something rendering related", &[]), &skills);
        assert!(recs.is_empty());
    }

    #[test]
    fn higher_score_is_ordered_first() {
        let skills = vec![
            skill(
                "weak-skill",
                SkillInvocationMode::Suggest,
                &["rendering"],
                &[],
                &[],
            ),
            skill(
                "strong-skill",
                SkillInvocationMode::Auto,
                &["rendering", "reveal"],
                &["src/tui/app/reveal.rs"],
                &[],
            ),
        ];
        let recs = recommend_skills(
            &input("rendering and reveal work in src/tui/app/reveal.rs", &[]),
            &skills,
        );
        assert_eq!(recs.len(), 2);
        assert_eq!(recs[0].name, "strong-skill");
        assert!(recs[0].score > recs[1].score);
    }

    #[test]
    fn reminder_is_built_only_when_actionable() {
        assert!(build_skill_recommendation_reminder(&[]).is_none());

        let recs = vec![SkillRecommendation {
            name: "zo-tui-render-performance".to_string(),
            score: 0.80,
            decision: SkillDecision::Load,
            reasons: vec!["keyword `reveal`".to_string()],
        }];
        let reminder = build_skill_recommendation_reminder(&recs).expect("reminder");
        assert!(reminder.starts_with(SKILL_RECOMMENDATION_REMINDER_PREFIX));
        assert!(reminder.contains("zo-tui-render-performance"));
        assert!(reminder.contains("Skill"));
    }

    #[test]
    fn reminder_caps_surfaced_skills() {
        let recs: Vec<SkillRecommendation> = (0..5)
            .map(|i| SkillRecommendation {
                name: format!("skill-{i}"),
                score: 0.5,
                decision: SkillDecision::Suggest,
                reasons: vec!["keyword `x`".to_string()],
            })
            .collect();
        let reminder = build_skill_recommendation_reminder(&recs).expect("reminder");
        // Header + at most 2 skill lines.
        assert_eq!(reminder.lines().count(), 3);
    }
}
