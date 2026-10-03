//! Review and export the evidence the existing Jev settings opted into keeping.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use serde_json::json;
use zerocode_core::jev::learning::{self, Outcome, Reviewer, store::Store};

use super::{Refused, Report};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Request {
    Review { seat: Option<String>, limit: usize, audit: usize, seed: u64, json: bool },
    Outcome { case_id: String, question: String, correct: bool, reviewer: Reviewer, note: String },
    Export { seat: Option<String> },
    Archive { path: PathBuf, seat: Option<String>, rubric: Option<u32>, include_unreviewed: bool },
}

fn refused(message: impl Into<String>) -> Refused {
    Refused { message: message.into(), exit: 2 }
}

pub(super) fn parse(args: &[String]) -> Result<Request, Refused> {
    let verb = args.first().map(String::as_str).unwrap_or_default();
    let mut at = 1;
    let case_id = if verb == "outcome" {
        let id = args.get(at).filter(|id| !id.starts_with('-')).ok_or_else(|| refused("outcome needs a case id"))?;
        at += 1;
        Some(id.clone())
    } else { None };
    let archive = if verb == "archive" {
        let path = args.get(at).filter(|path| !path.starts_with('-')).ok_or_else(|| refused("archive needs a new output file"))?;
        at += 1;
        Some(PathBuf::from(path))
    } else { None };
    let mut fields = std::collections::BTreeMap::new();
    let mut as_json = false;
    let mut include_unreviewed = false;
    while let Some(flag) = args.get(at) {
        if flag == "--include-unreviewed" && verb == "archive" {
            if include_unreviewed { return Err(refused("--include-unreviewed was specified twice")); }
            include_unreviewed = true;
            at += 1;
            continue;
        }
        if flag == "--json" {
            as_json = true;
            at += 1;
            continue;
        }
        let allowed = match verb {
            "review" => ["--seat", "--limit", "--audit", "--seed"].contains(&flag.as_str()),
            "export" => flag == "--seat",
            "archive" => ["--seat", "--rubric-version"].contains(&flag.as_str()),
            "outcome" => ["--question", "--correct", "--reviewer", "--note"].contains(&flag.as_str()),
            _ => false,
        };
        if !allowed { return Err(refused(format!("unknown {verb} argument '{flag}'"))); }
        let value = args.get(at + 1).ok_or_else(|| refused(format!("{flag} needs a value")))?;
        if fields.insert(flag.as_str(), value.as_str()).is_some() {
            return Err(refused(format!("{flag} was specified twice")));
        }
        at += 2;
    }
    let required = |key: &str| fields.get(key).copied().filter(|value| !value.trim().is_empty())
        .ok_or_else(|| refused(format!("{verb} needs {key}")));
    if let Some(case_id) = case_id {
        let correct = match required("--correct")? {
            "true" => true, "false" => false,
            _ => return Err(refused("--correct must be true or false")),
        };
        let reviewer = match required("--reviewer")? {
            "human" => Reviewer::Human, "agent" => Reviewer::Agent, "execution" => Reviewer::Execution,
            _ => return Err(refused("--reviewer must be human, agent or execution")),
        };
        return Ok(Request::Outcome { case_id, question: required("--question")?.into(), correct,
            reviewer, note: required("--note")?.into() });
    }
    let seat = fields.get("--seat").map(std::string::ToString::to_string);
    if seat.as_ref().is_some_and(|id| zerocode_core::jev::jev_use(id).is_none()) {
        return Err(refused("--seat must name an existing Jev feature"));
    }
    if verb == "export" { return Ok(Request::Export { seat }); }
    if let Some(path) = archive {
        let rubric = fields.get("--rubric-version").map(|word| word.parse::<u32>()
            .map_err(|_| refused("--rubric-version needs a positive integer"))).transpose()?;
        if rubric == Some(0) { return Err(refused("--rubric-version needs a positive integer")); }
        return Ok(Request::Archive { path, seat, rubric, include_unreviewed });
    }
    let number = |key: &str, default: u64| -> Result<u64, Refused> {
        fields.get(key).map_or(Ok(default), |text| text.parse().map_err(|_| refused(format!("{key} needs a nonnegative integer"))))
    };
    let limit = usize::try_from(number("--limit", 10)?).map_err(|_| refused("invalid review limit"))?;
    let default_audit = u64::try_from((limit / 5).max(1)).unwrap_or(1);
    let audit = usize::try_from(number("--audit", default_audit)?).map_err(|_| refused("invalid audit count"))?;
    if !(1..=learning::MAX_BATCH).contains(&limit) || audit > limit {
        return Err(refused("--limit must be 1 to 100 and --audit may not exceed it"));
    }
    Ok(Request::Review { seat, limit, audit, seed: number("--seed", 0)?, json: as_json })
}

pub(super) fn run(request: &Request, home: &Path, now_ms: i64) -> Result<Report, Refused> {
    let store = Store::at(home);
    if let Request::Archive { path, seat, rubric, include_unreviewed } = request {
        let archived = store.archive(path, seat.as_deref(), *rubric, *include_unreviewed)
            .map_err(|error| refused(error.to_string()))?;
        return Ok(Report { text: json!({"archive":path,"result":archived,"settingsChanged":false}).to_string(), exit: 0 });
    }
    if let Request::Outcome { case_id, question, correct, reviewer, note } = request {
        let outcome = Outcome { case_id: case_id.clone(), question: question.clone(), at: now_ms,
            correct: *correct, reviewer: *reviewer, note: note.clone() };
        store.outcome(outcome).map_err(|error| refused(error.to_string()))?;
        return Ok(Report { text: json!({"recorded":true,"caseId":case_id,"question":question}).to_string(), exit: 0 });
    }
    if let Request::Review { seat, limit, audit, seed, json } = request {
        let review = store.review(seat.as_deref(), None, *limit, *audit, *seed)
            .map_err(|error| refused(error.to_string()))?;
        let text = if *json {
            json!({"schemaVersion":1,"cases":review.cases,"invalid":review.invalid,
                "capacity":learning::store::MAX_CASES,"samples":review.samples}).to_string()
        } else {
            let mut text = format!("{} cases selected; {} invalid records. Retention uses smart.jev.labelDrafts.\n",
                review.samples.len(), review.invalid);
            for sample in &review.samples {
                let case = &sample.case;
                let _ = writeln!(text, "\n{} · {} · rubric {} · {} · {:?}\nQuestions: {}\nRequest: {}\nAnswers: {}",
                    case.id, case.seat, case.rubric_version, case.model, sample.reason,
                    sample.remaining.join(", "), case.request, case.answers);
            }
            text
        };
        return Ok(Report { text, exit: 0 });
    }
    let mut snapshot = store.snapshot().map_err(|error| refused(error.to_string()))?;
    let seat = match request {
        Request::Review { seat, .. } | Request::Export { seat } => seat.as_deref(),
        Request::Outcome { .. } | Request::Archive { .. } => None,
    };
    if let Some(seat) = seat { snapshot.cases.retain(|case| case.seat == seat); }
    let selected_ids = snapshot.cases.iter().map(|case| case.id.as_str()).collect::<std::collections::HashSet<_>>();
    snapshot.outcomes.retain(|outcome| selected_ids.contains(outcome.case_id.as_str()));
    match request {
        Request::Review { .. } => Err(refused("review was not selected")),
        Request::Export { .. } => Ok(Report { text: json!({"schemaVersion":1,
            "cases":snapshot.cases,"outcomes":snapshot.outcomes,"invalid":snapshot.invalid}).to_string(), exit: 0 }),
        Request::Outcome { .. } => Err(refused("outcome was not recorded")),
        Request::Archive { .. } => Err(refused("archive was not recorded")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(args: &[&str]) -> Vec<String> { args.iter().map(|arg| (*arg).to_string()).collect() }

    #[test]
    fn review_reads_existing_seats_and_refuses_ambiguous_or_unbounded_arguments() {
        assert!(parse(&words(&["review", "--seat", "skills", "--limit", "5", "--audit", "1"])).is_ok());
        for args in [vec!["review", "--limit", "0"], vec!["review", "--limit", "101"],
            vec!["review", "--limit", "1", "--audit", "2"], vec!["review", "--seat", "unknown"],
            vec!["review", "--seed", "1", "--seed", "2"]] {
            assert!(parse(&words(&args)).is_err(), "{args:?}");
        }
    }

    #[test]
    fn outcomes_require_an_explicit_reviewer_correctness_and_evidence() {
        let request = parse(&words(&["outcome", "case", "--question", "q", "--correct", "false",
            "--reviewer", "agent", "--note", "Execution contradicted the answer."])).unwrap();
        assert!(matches!(request, Request::Outcome { reviewer: Reviewer::Agent, correct: false, .. }));
        assert!(parse(&words(&["outcome", "case", "--question", "q", "--correct", "true"])).is_err());
    }

    #[test]
    fn archives_name_a_new_destination_and_explicitly_include_unreviewed_evidence() {
        assert!(parse(&words(&["archive"])).is_err());
        assert!(parse(&words(&["archive", "saved.json", "--rubric-version", "0"])).is_err());
        assert!(matches!(parse(&words(&["archive", "saved.json", "--seat", "skills",
            "--rubric-version", "1", "--include-unreviewed"])).unwrap(),
            Request::Archive { include_unreviewed: true, rubric: Some(1), .. }));
    }

    #[test]
    fn a_read_of_an_empty_store_creates_nothing() {
        let home = tempfile::tempdir().unwrap();
        let request = parse(&words(&["review", "--json"])).unwrap();
        let result = run(&request, home.path(), 10).unwrap();
        let value: serde_json::Value = serde_json::from_str(&result.text).unwrap();
        assert_eq!(value["samples"], json!([]));
        assert!(!home.path().join("jev").exists());
    }
}
