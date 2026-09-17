use std::collections::BTreeMap;

use crate::ToolError;
use runtime::CouncilOutcome;
use serde::{Deserialize, Serialize};

pub(crate) const MAX_COUNCIL_CANDIDATES: usize = 7;
pub(crate) const MAX_COUNCIL_CANDIDATE_CHARS: usize = 8_000;
/// A verdict is one line, not an essay — the point of a verdict is to be
/// comparable, and a long one is just the answer again under another name.
/// A declaration past this length is not treated as a verdict at all.
pub(crate) const MAX_COUNCIL_VERDICT_CHARS: usize = 400;
pub(crate) const MAX_COUNCIL_LLM_JUDGE_CALLS: usize = 1;

/// The marker a candidate uses to declare its comparable one-line conclusion,
/// on the final line of its own answer.
pub(crate) const VERDICT_MARKER: &str = "VERDICT:";

/// The verdict a candidate declared at the end of its OWN answer.
///
/// Read out of the answer rather than accepted as a separate input, because a
/// verdict supplied beside the answer is a claim nothing can check: an
/// orchestrator could hand three disagreeing answers three matching verdicts
/// and manufacture a majority the candidates never reached. Derived this way,
/// declaring agreement requires actually writing it.
///
/// Only the LAST non-empty line counts. A candidate that declares and then
/// keeps talking has not committed, and honouring an earlier marker would let
/// an echoed or intermediate line outvote the conclusion it landed on.
/// Matching is case-insensitive — models are inconsistent about shouting, and
/// that is not a disagreement.
pub(crate) fn verdict_from_answer(text: &str) -> Option<&str> {
    let last = text.lines().rev().map(str::trim).find(|line| !line.is_empty())?;
    // `get` rather than a byte range: a line starting with a multi-byte
    // character would panic on a slice that lands mid-codepoint.
    if !last.get(..VERDICT_MARKER.len())?.eq_ignore_ascii_case(VERDICT_MARKER) {
        return None;
    }
    let verdict = last[VERDICT_MARKER.len()..].trim();
    // A bare marker declares nothing — treating it as an empty verdict would
    // let two silent candidates "agree".
    (!verdict.is_empty() && verdict.chars().count() <= MAX_COUNCIL_VERDICT_CHARS).then_some(verdict)
}

#[derive(Debug, Deserialize)]
pub(crate) struct CouncilInput {
    pub candidates: Vec<CouncilCandidateInput>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct CouncilCandidateInput {
    pub text: String,
    #[serde(default)]
    pub status: Option<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct CouncilOutput {
    pub outcome: CouncilOutcome,
    pub candidate_count: usize,
    pub successful_count: usize,
    pub source_hidden: bool,
    /// Which surface the vote was decided on — `"verdict"` or `"text"`. A
    /// reader cannot otherwise tell whether an outcome reflects declared
    /// conclusions or raw prose equality, and those mean different things.
    pub voted_on: &'static str,
    pub llm_judge_allowed: bool,
    pub llm_judge_call_limit: usize,
}

pub(crate) fn execute_council(input: &CouncilInput) -> Result<CouncilOutput, ToolError> {
    validate_council_input(input)?;

    let candidate_count = input.candidates.len();
    // Every ballot is derived from the candidate's own answer, so nothing
    // outside the candidate can claim agreement on its behalf.
    let declared: Vec<Option<&str>> = input
        .candidates
        .iter()
        .map(|candidate| verdict_from_answer(&candidate.text))
        .collect();
    let vote_on_verdicts = every_successful_candidate_declared(input, &declared);
    let mut groups: BTreeMap<String, Vec<usize>> = BTreeMap::new();

    for (index, candidate) in input.candidates.iter().enumerate() {
        if !is_success_status(candidate.status.as_deref()) {
            continue;
        }
        let ballot = if vote_on_verdicts {
            declared[index].unwrap_or_default()
        } else {
            candidate.text.as_str()
        };
        let normalized = normalize_candidate_text(ballot);
        if normalized.is_empty() {
            continue;
        }
        groups.entry(normalized).or_default().push(index);
    }

    let successful_count = groups.values().map(Vec::len).sum();
    let outcome = select_self_consistency(&groups, successful_count);
    let llm_judge_allowed = should_allow_llm_judge(&outcome, successful_count);
    let llm_judge_call_limit = if llm_judge_allowed {
        MAX_COUNCIL_LLM_JUDGE_CALLS
    } else {
        0
    };
    Ok(CouncilOutput {
        outcome,
        candidate_count,
        successful_count,
        source_hidden: true,
        voted_on: if vote_on_verdicts { "verdict" } else { "text" },
        llm_judge_allowed,
        llm_judge_call_limit,
    })
}

/// Whether the vote runs on declared verdicts rather than full answers.
///
/// All-or-nothing, and deliberately so: if one candidate declared a verdict and
/// another did not, comparing the first's conclusion against the second's whole
/// answer is not a comparison at all. Requiring every successful candidate to
/// have declared one keeps both surfaces internally consistent, and an
/// incomplete set simply votes the way it did before verdicts existed.
fn every_successful_candidate_declared(input: &CouncilInput, declared: &[Option<&str>]) -> bool {
    let mut any = false;
    for (index, candidate) in input.candidates.iter().enumerate() {
        if !is_success_status(candidate.status.as_deref()) {
            continue;
        }
        if declared[index].is_none() {
            return false;
        }
        any = true;
    }
    any
}

fn validate_council_input(input: &CouncilInput) -> Result<(), ToolError> {
    if input.candidates.len() > MAX_COUNCIL_CANDIDATES {
        return Err(ToolError::InvalidInput(format!(
            "Council accepts at most {MAX_COUNCIL_CANDIDATES} candidates (got {})",
            input.candidates.len()
        )));
    }

    for (index, candidate) in input.candidates.iter().enumerate() {
        let char_count = candidate.text.chars().count();
        if char_count > MAX_COUNCIL_CANDIDATE_CHARS {
            return Err(ToolError::InvalidInput(format!(
                "Council candidate {index} text must be at most {MAX_COUNCIL_CANDIDATE_CHARS} characters (got {char_count})"
            )));
        }
    }

    Ok(())
}

fn select_self_consistency(
    groups: &BTreeMap<String, Vec<usize>>,
    successful_count: usize,
) -> CouncilOutcome {
    let Some(max_support) = groups.values().map(Vec::len).max() else {
        return CouncilOutcome::Tie {
            reason: "no successful candidates".to_string(),
        };
    };
    if max_support < 2 || max_support * 2 <= successful_count {
        return CouncilOutcome::Tie {
            reason: "no self-consistency majority".to_string(),
        };
    }

    let winners = groups
        .values()
        .filter(|indices| indices.len() == max_support)
        .collect::<Vec<_>>();
    if winners.len() != 1 {
        return CouncilOutcome::Tie {
            reason: "multiple candidate answers tied".to_string(),
        };
    }

    let supporting_indices = winners[0].clone();
    CouncilOutcome::BestOf {
        winner_index: supporting_indices[0],
        supporting_indices,
        rationale: "selected by self-consistency majority".to_string(),
    }
}

fn should_allow_llm_judge(outcome: &CouncilOutcome, successful_count: usize) -> bool {
    matches!(outcome, CouncilOutcome::Tie { .. }) && successful_count >= 2
}

fn is_success_status(status: Option<&str>) -> bool {
    status.is_none_or(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "" | "ok" | "success" | "succeeded" | "completed"
        )
    })
}

fn normalize_candidate_text(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::{
        execute_council, normalize_candidate_text, verdict_from_answer, CouncilCandidateInput,
        CouncilInput, MAX_COUNCIL_CANDIDATES, MAX_COUNCIL_CANDIDATE_CHARS,
        MAX_COUNCIL_LLM_JUDGE_CALLS, MAX_COUNCIL_VERDICT_CHARS,
    };
    use crate::ToolError;
    use runtime::CouncilOutcome;

    #[test]
    fn self_consistency_selects_majority_candidate() {
        let output = execute_council(&CouncilInput {
            candidates: vec![
                CouncilCandidateInput {
                    text: "Use ProviderClient routing".to_string(),
                    status: None,
                },
                CouncilCandidateInput {
                    text: "Use providerclient routing".to_string(),
                    status: Some("completed".to_string()),
                },
                CouncilCandidateInput {
                    text: "Rewrite the runtime".to_string(),
                    status: None,
                },
            ],
        })
        .expect("valid council input");

        assert_eq!(output.candidate_count, 3);
        assert_eq!(output.successful_count, 3);
        assert!(output.source_hidden);
        assert!(!output.llm_judge_allowed);
        assert_eq!(output.llm_judge_call_limit, 0);
        assert_eq!(
            output.outcome,
            CouncilOutcome::BestOf {
                winner_index: 0,
                supporting_indices: vec![0, 1],
                rationale: "selected by self-consistency majority".to_string(),
            }
        );
    }

    #[test]
    fn all_failed_candidates_return_honest_tie() {
        let output = execute_council(&CouncilInput {
            candidates: vec![CouncilCandidateInput {
                text: "ignored".to_string(),
                status: Some("failed".to_string()),
            }],
        })
        .expect("valid council input");

        assert_eq!(output.successful_count, 0);
        assert!(!output.llm_judge_allowed);
        assert_eq!(output.llm_judge_call_limit, 0);
        assert_eq!(
            output.outcome,
            CouncilOutcome::Tie {
                reason: "no successful candidates".to_string(),
            }
        );
    }

    #[test]
    fn plurality_without_majority_does_not_fake_a_winner() {
        let output = execute_council(&CouncilInput {
            candidates: vec![
                CouncilCandidateInput {
                    text: "A".to_string(),
                    status: None,
                },
                CouncilCandidateInput {
                    text: "A".to_string(),
                    status: None,
                },
                CouncilCandidateInput {
                    text: "B".to_string(),
                    status: None,
                },
                CouncilCandidateInput {
                    text: "C".to_string(),
                    status: None,
                },
                CouncilCandidateInput {
                    text: "D".to_string(),
                    status: None,
                },
            ],
        })
        .expect("valid council input");

        assert_eq!(output.successful_count, 5);
        assert!(output.llm_judge_allowed);
        assert_eq!(
            output.outcome,
            CouncilOutcome::Tie {
                reason: "no self-consistency majority".to_string(),
            }
        );
    }

    #[test]
    fn unique_answers_do_not_fake_a_winner() {
        let output = execute_council(&CouncilInput {
            candidates: vec![
                CouncilCandidateInput {
                    text: "A".to_string(),
                    status: None,
                },
                CouncilCandidateInput {
                    text: "B".to_string(),
                    status: None,
                },
                CouncilCandidateInput {
                    text: "C".to_string(),
                    status: None,
                },
            ],
        })
        .expect("valid council input");

        assert!(output.llm_judge_allowed);
        assert_eq!(output.llm_judge_call_limit, MAX_COUNCIL_LLM_JUDGE_CALLS);
        assert_eq!(
            output.outcome,
            CouncilOutcome::Tie {
                reason: "no self-consistency majority".to_string(),
            }
        );
    }

    #[test]
    fn the_vote_reads_the_verdict_each_candidate_declared_in_its_own_answer() {
        // The declaration must come from the candidate's ANSWER, never from a
        // field the caller fills in beside it: an orchestrator that could hand
        // three disagreeing answers three matching "verdicts" could manufacture
        // a winner the candidates never agreed on.
        let output = execute_council(&CouncilInput {
            candidates: vec![
                CouncilCandidateInput {
                    text: "Long reasoning, phrased one way.\nVERDICT: fix ProviderClient routing"
                        .to_string(),
                    status: None,
                },
                CouncilCandidateInput {
                    text: "Entirely different prose, same destination.\nverdict:  Fix providerclient Routing  "
                        .to_string(),
                    status: Some("completed".to_string()),
                },
                CouncilCandidateInput {
                    text: "A third opinion.\nVERDICT: rewrite the runtime".to_string(),
                    status: None,
                },
            ],
        })
        .expect("valid council input");

        assert_eq!(output.voted_on, "verdict");
        assert_eq!(
            output.outcome,
            CouncilOutcome::BestOf {
                winner_index: 0,
                supporting_indices: vec![0, 1],
                rationale: "selected by self-consistency majority".to_string(),
            }
        );
    }

    #[test]
    fn a_verdict_must_be_the_final_line_not_merely_present_somewhere() {
        // A candidate that declares and then keeps talking has not committed.
        // Accepting a marker anywhere lets an echoed or intermediate line
        // outvote the conclusion the candidate actually landed on.
        let output = execute_council(&CouncilInput {
            candidates: vec![
                CouncilCandidateInput {
                    text: "VERDICT: approve\nActually, on reflection, reject it.".to_string(),
                    status: None,
                },
                CouncilCandidateInput {
                    text: "Some other answer.\nVERDICT: approve".to_string(),
                    status: None,
                },
            ],
        })
        .expect("valid council input");

        assert_eq!(
            output.voted_on, "text",
            "one candidate never committed, so the whole vote falls back to text"
        );
        assert_eq!(
            output.outcome,
            CouncilOutcome::Tie {
                reason: "no self-consistency majority".to_string(),
            }
        );
    }

    #[test]
    fn a_candidate_that_declares_nothing_puts_the_whole_vote_back_on_text() {
        // All-or-nothing: comparing one candidate's conclusion against
        // another's whole answer is not a comparison, so an incomplete set
        // votes exactly the way it did before verdicts existed.
        let output = execute_council(&CouncilInput {
            candidates: vec![
                CouncilCandidateInput {
                    text: "Same answer\nVERDICT: alpha".to_string(),
                    status: None,
                },
                CouncilCandidateInput {
                    text: "same answer".to_string(),
                    status: None,
                },
                CouncilCandidateInput {
                    text: "Different answer\nVERDICT: alpha".to_string(),
                    status: None,
                },
            ],
        })
        .expect("valid council input");

        assert_eq!(output.voted_on, "text");
        assert_eq!(
            output.outcome,
            CouncilOutcome::Tie {
                reason: "no self-consistency majority".to_string(),
            },
            "on text the three answers are all distinct, so there is no majority"
        );
    }

    #[test]
    fn a_bare_marker_counts_as_undeclared_rather_than_as_agreement() {
        // Two candidates that declared nothing must not be read as agreeing on
        // nothing — that would be a fabricated majority.
        let output = execute_council(&CouncilInput {
            candidates: vec![
                CouncilCandidateInput {
                    text: "Answer A\nVERDICT:   ".to_string(),
                    status: None,
                },
                CouncilCandidateInput {
                    text: "Answer B\nVERDICT:".to_string(),
                    status: None,
                },
            ],
        })
        .expect("valid council input");

        assert_eq!(output.voted_on, "text");
        assert_eq!(
            output.outcome,
            CouncilOutcome::Tie {
                reason: "no self-consistency majority".to_string(),
            }
        );
    }

    #[test]
    fn disagreeing_verdicts_still_return_an_honest_tie() {
        let output = execute_council(&CouncilInput {
            candidates: vec![
                CouncilCandidateInput {
                    text: "Answer A\nVERDICT: alpha".to_string(),
                    status: None,
                },
                CouncilCandidateInput {
                    text: "Answer B\nVERDICT: beta".to_string(),
                    status: None,
                },
                CouncilCandidateInput {
                    text: "Answer C\nVERDICT: gamma".to_string(),
                    status: None,
                },
            ],
        })
        .expect("valid council input");

        assert_eq!(output.voted_on, "verdict");
        assert_eq!(
            output.outcome,
            CouncilOutcome::Tie {
                reason: "no self-consistency majority".to_string(),
            }
        );
        assert!(
            output.llm_judge_allowed,
            "a real disagreement may be adjudicated"
        );
    }

    #[test]
    fn a_failed_candidate_without_a_verdict_does_not_drop_the_vote_to_text() {
        // Only SUCCESSFUL candidates have to declare one — a failed candidate
        // contributes no ballot either way.
        let output = execute_council(&CouncilInput {
            candidates: vec![
                CouncilCandidateInput {
                    text: "Answer A\nVERDICT: alpha".to_string(),
                    status: Some("completed".to_string()),
                },
                CouncilCandidateInput {
                    text: "Answer B\nVERDICT: alpha".to_string(),
                    status: Some("completed".to_string()),
                },
                CouncilCandidateInput {
                    text: "crashed".to_string(),
                    status: Some("failed".to_string()),
                },
            ],
        })
        .expect("valid council input");

        assert_eq!(output.voted_on, "verdict");
        assert_eq!(output.successful_count, 2);
        assert_eq!(
            output.outcome,
            CouncilOutcome::BestOf {
                winner_index: 0,
                supporting_indices: vec![0, 1],
                rationale: "selected by self-consistency majority".to_string(),
            }
        );
    }

    #[test]
    fn verdict_from_answer_reads_only_the_final_committed_line() {
        assert_eq!(verdict_from_answer("reasoning\nVERDICT: alpha"), Some("alpha"));
        assert_eq!(verdict_from_answer("no marker here"), None);
        assert_eq!(
            verdict_from_answer("VERDICT: first\nmiddle\nverdict:   last  "),
            Some("last"),
            "the final line is the commitment, matched case-insensitively"
        );
        assert_eq!(
            verdict_from_answer("trailing blank\nVERDICT: beta\n\n"),
            Some("beta"),
            "trailing blank lines must not hide the declaration"
        );
        assert_eq!(
            verdict_from_answer("VERDICT: approve\nActually, on reflection, reject it."),
            None,
            "a candidate that declares and then keeps talking has not committed"
        );
        assert_eq!(
            verdict_from_answer("declared nothing\nVERDICT:   "),
            None,
            "a bare marker declares nothing, so two silent candidates cannot 'agree'"
        );
        assert_eq!(
            verdict_from_answer("한국어 답변입니다\nVERDICT: 감마"),
            Some("감마"),
            "an answer whose lines start with multi-byte characters must not panic"
        );
    }

    #[test]
    fn an_essay_length_declaration_is_not_a_verdict() {
        // A verdict past the cap is the answer again under another name, and
        // comparing those is what whole-text voting already does.
        let long = "x".repeat(MAX_COUNCIL_VERDICT_CHARS + 1);
        assert_eq!(verdict_from_answer(&format!("body\nVERDICT: {long}")), None);
        let at_cap = "y".repeat(MAX_COUNCIL_VERDICT_CHARS);
        assert_eq!(
            verdict_from_answer(&format!("body\nVERDICT: {at_cap}")),
            Some(at_cap.as_str())
        );
    }

    #[test]
    fn normalization_ignores_case_and_whitespace() {
        assert_eq!(
            normalize_candidate_text("  Same\n answer\tagain "),
            "same answer again"
        );
    }

    #[test]
    fn rejects_too_many_candidates() {
        let candidates = (0..=MAX_COUNCIL_CANDIDATES)
            .map(|index| CouncilCandidateInput {
                text: format!("candidate {index}"),
                status: None,
            })
            .collect();
        let error = execute_council(&CouncilInput { candidates })
            .expect_err("candidate count should be bounded");

        assert!(matches!(
            error,
            ToolError::InvalidInput(message)
                if message.contains("at most")
                    && message.contains(&MAX_COUNCIL_CANDIDATES.to_string())
        ));
    }

    #[test]
    fn rejects_oversized_candidate_text() {
        let error = execute_council(&CouncilInput {
            candidates: vec![CouncilCandidateInput {
                text: "x".repeat(MAX_COUNCIL_CANDIDATE_CHARS + 1),
                status: None,
            }],
        })
        .expect_err("candidate text should be bounded");

        assert!(matches!(
            error,
            ToolError::InvalidInput(message)
                if message.contains("candidate 0")
                    && message.contains(&MAX_COUNCIL_CANDIDATE_CHARS.to_string())
        ));
    }
}
