//! `GoalOptions` → `/goal` flag-suffix serialization.
//!
//! The TUI's goal-clarify picker answers the ambiguity hold by RE-SUBMITTING
//! the goal through the normal `/goal` text path (one apply path, no
//! duplicated start logic), so the options the user originally passed must
//! round-trip through text. The flag spellings live in this crate right next
//! to their parser, and [`tests::flags_suffix_round_trips_through_the_parser`]
//! pins the pair — the parser is the oracle, so a renamed flag breaks the
//! build here instead of silently dropping an option on resubmission.

use crate::slash_commands::GoalOptions;

impl GoalOptions {
    /// Render these options back into `/goal` flag syntax (leading space when
    /// non-empty, empty string when default). `checks` are included for
    /// completeness, though the clarify hold only ever fires on a goal with
    /// no objective checks.
    #[must_use]
    pub fn to_flags_suffix(&self) -> String {
        use std::fmt::Write as _;
        let mut out = String::new();
        for check in &self.checks {
            out.push_str(" --check \"");
            out.push_str(check);
            out.push('"');
        }
        if let Some(turns) = self.max_turns {
            let _ = write!(out, " --max-turns {turns}");
        }
        if let Some(budget) = self.token_budget {
            let _ = write!(out, " --token-budget {budget}");
        }
        if self.allow_writes {
            out.push_str(" --allow-writes");
        }
        if self.no_plan {
            out.push_str(" --no-plan");
        }
        if let Some(until) = self.until.as_deref() {
            let _ = write!(out, " --until \"{until}\"");
        }
        if let Some(every) = self.every.as_ref() {
            let _ = write!(out, " --every {}", every.raw);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use crate::slash_commands::{GoalCommand, GoalOptions, SlashCommand};

    /// 직렬화 철자와 파서가 한 소스임을 핀: suffix를 붙여 재파싱하면
    /// 원본 옵션이 그대로 돌아와야 한다(파서가 오라클).
    #[test]
    fn flags_suffix_round_trips_through_the_parser() {
        let options = GoalOptions {
            checks: vec!["cargo:test".to_string()],
            max_turns: Some(7),
            token_budget: Some(50_000),
            allow_writes: true,
            no_plan: true,
            until: None,
            every: Some(crate::slash_commands::DurationSpec {
                raw: "30s".to_string(),
                duration: std::time::Duration::from_secs(30),
            }),
        };
        let input = format!("/goal fix the parser{}", options.to_flags_suffix());
        let parsed = SlashCommand::parse(&input)
            .expect("suffix must parse")
            .expect("input is a slash command");
        let SlashCommand::Goal {
            command: GoalCommand::Start { goal, options: reparsed },
        } = parsed
        else {
            panic!("must parse as a goal start: {input}");
        };
        assert_eq!(goal, "fix the parser");
        assert_eq!(reparsed, options);
    }

    /// 기본 옵션은 빈 suffix — 재제출 텍스트가 원문 그대로 유지된다.
    #[test]
    fn default_options_serialize_to_nothing() {
        assert_eq!(GoalOptions::default().to_flags_suffix(), "");
    }
}
