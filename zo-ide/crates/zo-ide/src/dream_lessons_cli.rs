//! `zo dream-lessons` — what the dreamer would ask about a session, and what
//! it would take from the answer, shown and not kept.
//!
//! The dreamer may ask a model one question about a session: of what the
//! person said, what will still hold in the next one
//! (`decision_core::said_lessons`). That asking is off unless the person
//! turns it on. This command is how they see what it would do first: it shows
//! one session's words to the fast model of the provider that session ran on,
//! reads the reply by the dreamer's own rules, and prints what was taken. It
//! writes nothing — no candidate, no memory, no vault page, no cursor — and
//! `--dry-run` sends nothing either: it says which model would be asked and
//! how much it would be shown.
//!
//! A lesson is kept only when a second session shows a candidate an earlier
//! one left, which a model says by answering with the candidate's id. So a
//! preview can be handed candidates (`--candidates <file>`, read and never
//! written): the lessons earlier previews took, as a runner carries them
//! from one session of a project to the next. Its output says which of them
//! this session showed again.
//!
//! It sends a session's words only where a person is at the terminal that
//! ran it. Run by a program — a session's own shell among them — it answers
//! as `--dry-run` does and says so, unless `--send` says the sending is
//! meant: the switch a pass needs (`dreamLessons`) is not asked of a preview,
//! so a preview a model could run would be a way round it.
//!
//! ```text
//! zo dream-lessons preview <session id|name|path> [--candidates <file>] [--dry-run] [--send] [--json]
//! ```

use std::fmt::Write as _;

use core_types::Session;
use decision_core::dreamer::LessonKind;
use decision_core::said_lessons::{line_allowed, Digest, Offered, Pending, NEW_SAID_TO_ASK};
use serde_json::{json, Value};
use tools::fast_model::FastModel;
use tools::said_lessons::{ask, digest_of, model_of, ASK_WITHIN};

pub const USAGE: &str = "\
zo dream-lessons preview <session id|name|path> [--candidates <file>] [--dry-run] [--send] [--json]

  What the dreamer would ask about one session and take from the answer,
  shown and not kept: nothing is written to memory, the vault or the
  session. It sends what the person said in that session (the beginning of
  each message, and the end of what the assistant had said before it) to the
  fast model of the provider the session ran on — one request. --dry-run
  sends nothing: it names the model and the size of what would be shown.
  --candidates is a JSON list of {\"id\", \"kind\", \"text\"} — lessons earlier
  sessions left — shown beside the session; the file is read, never written.
  Where no person is at the terminal it sends nothing either, unless --send.
";

/// What became of one preview.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Outcome {
    /// Nothing was sent, and why.
    NotAsked(&'static str),
    /// The reply was read: the lessons taken, and how many it offered that
    /// were not.
    Asked { taken: Vec<Offered>, dropped: usize },
    /// The request or the reading of its reply failed, in its words.
    Failed(String),
}

/// One session's preview: what would be shown to which model, and what came
/// of asking.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Preview {
    /// The model the session ran on, when one answered in it.
    session_model: Option<String>,
    /// The fast model that is asked: the same provider's.
    asked_model: Option<String>,
    /// How many things the person said the digest shows, and its size.
    shown: usize,
    chars: usize,
    outcome: Outcome,
}

/// Why a session with fewer things said than a pass asks about is not asked.
const TOO_LITTLE_SAID: &str = "the person said fewer things in it than a pass asks about";
const NO_MODEL: &str = "no model answered in this session";
const DRY_RUN: &str = "a dry run sends nothing";
const NOBODY_AT_THE_TERMINAL: &str = "nobody is at the terminal that ran this, so nothing is sent; --send sends it";

/// Why this run sends nothing, when it does not: it was asked not to, or a
/// program ran it and did not say the sending is meant.
fn held_back(dry_run: bool, send: bool, at_a_terminal: bool) -> Option<&'static str> {
    if dry_run {
        Some(DRY_RUN)
    } else if !send && !at_a_terminal {
        Some(NOBODY_AT_THE_TERMINAL)
    } else {
        None
    }
}

/// # Errors
/// The usage for words it does not know, or why the session could not be read.
pub fn run(args: &[String]) -> Result<String, String> {
    let (mut dry_run, mut send, mut as_json, mut candidates, mut words) = (false, false, false, None, Vec::new());
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" | "-h" => return Ok(USAGE.to_string()),
            "--dry-run" => dry_run = true,
            "--send" => send = true,
            "--json" => as_json = true,
            "--candidates" => candidates = Some(args.next().ok_or("--candidates needs a file")?),
            flag if flag.starts_with("--") => return Err(format!("unknown word {flag:?}\n{USAGE}")),
            word => words.push(word),
        }
    }
    let ["preview", reference] = words.as_slice() else {
        return Err(USAGE.to_string());
    };
    let pending = match candidates {
        Some(file) => candidates_in(&std::fs::read_to_string(file).map_err(|error| format!("{file}: {error}"))?)?,
        None => Vec::new(),
    };
    let handle = crate::session_registry::resolve_session_reference(reference).map_err(|error| error.to_string())?;
    let session = Session::load_from_path(&handle.path).map_err(|error| error.to_string())?;
    let digest = digest_of(&session.messages);
    let session_model = model_of(&session.messages).map(str::to_string);
    let held_back = held_back(dry_run, send, tools::stdin_is_terminal());
    let preview = preview(&digest, session_model, held_back, |digest, model| {
        ask(digest, &pending, model, ASK_WITHIN).map(|asked| (asked.read.taken, asked.read.dropped))
    });
    Ok(if as_json {
        preview.json(&session.session_id).to_string()
    } else {
        preview.text(&session.session_id)
    })
}

/// The candidates a `--candidates` file holds: a JSON list of
/// `{"id", "kind", "text"}`, each of a kind a reply may offer and a line the
/// store would take.
///
/// # Errors
/// Which entry does not read, and why.
fn candidates_in(file: &str) -> Result<Vec<Pending>, String> {
    let list: Vec<Value> = serde_json::from_str(file).map_err(|error| format!("the candidates do not read: {error}"))?;
    list.iter()
        .enumerate()
        .map(|(index, entry)| {
            let word = |key: &str| entry.get(key).and_then(Value::as_str).map(str::trim).filter(|word| !word.is_empty());
            let kind = word("kind").and_then(|name| {
                [LessonKind::Preference, LessonKind::Constraint, LessonKind::Workflow]
                    .into_iter()
                    .find(|kind| kind.as_str() == name)
            });
            match (word("id"), kind, word("text").filter(|text| line_allowed(text))) {
                (Some(id), Some(kind), Some(text)) => Ok(Pending {
                    id: id.to_string(),
                    kind,
                    text: text.to_string(),
                }),
                _ => Err(format!("candidate {} needs an id, a kind (preference, constraint or workflow) and one line of text", index + 1)),
            }
        })
        .collect()
}

/// The preview of a session whose digest is `digest`: asked through `ask`
/// only when a pass would ask about it, a model answered in it, and nothing
/// holds the sending back (`held_back`: why, when something does).
fn preview(
    digest: &Digest,
    session_model: Option<String>,
    held_back: Option<&'static str>,
    ask: impl FnOnce(&Digest, &str) -> Result<(Vec<Offered>, usize), String>,
) -> Preview {
    let asked_model = session_model.as_deref().and_then(|model| FastModel::fast_tier_named_for(Some(model)));
    let outcome = match session_model.as_deref() {
        _ if digest.len() < NEW_SAID_TO_ASK => Outcome::NotAsked(TOO_LITTLE_SAID),
        None => Outcome::NotAsked(NO_MODEL),
        // Only a fast model is asked, as a pass asks: nothing goes to the
        // session's own model where its provider has no fast one.
        Some(_) if asked_model.is_none() => Outcome::NotAsked(tools::fast_model::NO_FAST_TIER),
        Some(model) => match held_back {
            Some(why) => Outcome::NotAsked(why),
            None => match ask(digest, model) {
                Ok((taken, dropped)) => Outcome::Asked { taken, dropped },
                Err(error) => Outcome::Failed(error),
            },
        },
    };
    Preview {
        session_model,
        asked_model,
        shown: digest.len(),
        chars: digest.chars(),
        outcome,
    }
}

impl Preview {
    fn json(&self, session_id: &str) -> Value {
        let (asked, taken, dropped, not_asked, error) = match &self.outcome {
            Outcome::NotAsked(why) => (false, Vec::new(), 0, Some(*why), None),
            Outcome::Asked { taken, dropped } => (true, taken.clone(), *dropped, None, None),
            Outcome::Failed(error) => (true, Vec::new(), 0, None, Some(error.as_str())),
        };
        json!({
            "session": session_id,
            "session_model": self.session_model,
            "asked_model": self.asked_model,
            "shown": self.shown,
            "chars": self.chars,
            "asked": asked,
            "not_asked": not_asked,
            "error": error,
            "dropped": dropped,
            "taken": taken.iter().map(|lesson| json!({
                "kind": lesson.kind.as_str(),
                "text": lesson.text,
                "said": lesson.said,
                "same_as": lesson.same_as,
            })).collect::<Vec<_>>(),
            "written": "nothing",
        })
    }

    fn text(&self, session_id: &str) -> String {
        let mut text = format!(
            "session {session_id}: the person said {} thing(s) a pass would show, {} characters\n",
            self.shown, self.chars
        );
        let _ = match (&self.session_model, &self.asked_model) {
            (Some(ran_on), Some(asked)) => writeln!(text, "ran on {ran_on}; the fast model asked is {asked}"),
            (Some(ran_on), None) => writeln!(text, "ran on {ran_on}; its provider has no fast model"),
            (None, _) => writeln!(text, "no model answered in it"),
        };
        let _ = match &self.outcome {
            Outcome::NotAsked(why) => writeln!(text, "not asked: {why}"),
            Outcome::Failed(error) => writeln!(text, "asked, and it failed: {error}"),
            Outcome::Asked { taken, dropped } => {
                writeln!(text, "asked: {} lesson(s) taken, {dropped} offered and not taken", taken.len())
            }
        };
        if let Outcome::Asked { taken, .. } = &self.outcome {
            for lesson in taken {
                let again = lesson.same_as.as_deref().map_or(String::new(), |id| format!(", candidate {id} shown again"));
                let _ = writeln!(text, "- [{}] {} (from [{}]{again})", lesson.kind.as_str(), lesson.text, lesson.said);
            }
        }
        text.push_str("nothing was written");
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use decision_core::said_lessons::{DigestLimits, Said};

    fn digest(said: &[&str]) -> Digest {
        let said: Vec<Said> = said
            .iter()
            .map(|words| Said {
                words: (*words).to_string(),
                before: String::new(),
            })
            .collect();
        Digest::of(&said, DigestLimits::default())
    }

    /// A model the catalog knows by name, so its provider and that provider's
    /// fast tier are the catalog's answer and not this machine's logins.
    const A_CATALOG_MODEL: &str = "claude-sonnet-4-6";

    fn never_asked(_: &Digest, _: &str) -> Result<(Vec<Offered>, usize), String> {
        panic!("a request was made")
    }

    /// Nothing is sent for a session a pass would not ask about, for one no
    /// model answered in, or on a dry run — and the dry run still names the
    /// model and the size.
    #[test]
    fn a_preview_sends_nothing_where_a_pass_would_not_ask_or_on_a_dry_run() {
        let short = preview(&digest(&["one", "two"]), Some(A_CATALOG_MODEL.into()), None, never_asked);
        assert_eq!(short.outcome, Outcome::NotAsked(TOO_LITTLE_SAID));
        let three = digest(&["Answer in Korean", "Never push to main", "Run the gate first"]);
        let no_model = preview(&three, None, None, never_asked);
        assert_eq!(no_model.outcome, Outcome::NotAsked(NO_MODEL));
        assert_eq!(no_model.asked_model, None);
        let dry = preview(&three, Some(A_CATALOG_MODEL.into()), Some(DRY_RUN), never_asked);
        assert_eq!(dry.outcome, Outcome::NotAsked(DRY_RUN));
        assert_eq!(dry.shown, 3);
        assert_eq!(dry.chars, three.chars());
        let asked_model = dry.asked_model.clone().expect("the model that would be asked");
        assert_eq!(
            api::detect_provider_kind(&asked_model),
            api::detect_provider_kind(A_CATALOG_MODEL),
            "the same provider's: {asked_model}"
        );
        let json = dry.json("s-1");
        assert_eq!(json["asked"], false);
        assert_eq!(json["not_asked"], DRY_RUN);
        assert_eq!(json["written"], "nothing");
    }

    /// A session whose provider has no fast model is not asked: the question
    /// goes to a fast model or nowhere.
    #[test]
    fn a_preview_sends_nothing_to_a_provider_with_no_fast_model() {
        let three = digest(&["Answer in Korean", "Never push to main", "Run the gate first"]);
        // The catalog's xAI row has no fast tier (`tools::fanout`'s own test).
        let model = "grok-3";
        if FastModel::fast_tier_named_for(Some(model)).is_some() {
            // The model routes elsewhere on this machine: nothing to show here.
            return;
        }
        let shown = preview(&three, Some(model.into()), None, never_asked);
        assert_eq!(shown.outcome, Outcome::NotAsked(tools::fast_model::NO_FAST_TIER));
        assert_eq!(shown.asked_model, None);
        assert!(shown.text("s-1").contains("its provider has no fast model"), "{}", shown.text("s-1"));
    }

    /// A session a pass would ask about is asked once, on the session's own
    /// model's provider, and what the reading took is what is printed.
    #[test]
    fn a_preview_asks_once_and_prints_what_was_taken() {
        let three = digest(&["Answer in Korean", "Never push to main", "Run the gate first"]);
        let mut asked_with = Vec::new();
        let taken = vec![Offered {
            same_as: None,
            kind: LessonKind::Constraint,
            text: "Never push to main".to_string(),
            said: 2,
        }];
        let shown = preview(&three, Some(A_CATALOG_MODEL.into()), None, |digest, model| {
            asked_with.push((digest.len(), model.to_string()));
            Ok((taken.clone(), 1))
        });
        assert_eq!(asked_with, [(3, A_CATALOG_MODEL.to_string())]);
        assert_eq!(shown.outcome, Outcome::Asked { taken, dropped: 1 });
        let text = shown.text("s-1");
        assert!(text.contains("asked: 1 lesson(s) taken, 1 offered and not taken"), "{text}");
        assert!(text.contains("- [constraint] Never push to main (from [2])"), "{text}");
        assert!(text.ends_with("nothing was written"), "{text}");
        let json = shown.json("s-1");
        assert_eq!(json["taken"][0]["kind"], "constraint");
        assert_eq!(json["dropped"], 1);

        let failed = preview(&three, Some(A_CATALOG_MODEL.into()), None, |_, _| Err("timed out after 45s".to_string()));
        assert_eq!(failed.outcome, Outcome::Failed("timed out after 45s".to_string()));
        assert_eq!(failed.json("s-1")["error"], "timed out after 45s");
    }

    /// Candidates are read from a file's JSON, each of a kind a reply may
    /// offer; a lesson that names one says so in what is printed.
    #[test]
    fn candidates_read_from_json_and_a_candidate_shown_again_is_said() {
        let read = candidates_in(
            r#"[{"id":"c1","kind":"constraint","text":"Never push to main"},
                {"id":"c2","kind":"preference","text":" Answer in Korean "}]"#,
        )
        .expect("two candidates");
        assert_eq!(read.len(), 2);
        assert_eq!((read[1].id.as_str(), read[1].kind, read[1].text.as_str()), ("c2", LessonKind::Preference, "Answer in Korean"));
        for bad in [
            r#"[{"id":"c1","kind":"gotcha","text":"bash is error-prone"}]"#,
            r#"[{"id":"c1","kind":"preference"}]"#,
            r#"[{"kind":"preference","text":"x"}]"#,
            r#"{"id":"c1"}"#,
        ] {
            assert!(candidates_in(bad).is_err(), "{bad}");
        }
        assert_eq!(candidates_in("[]"), Ok(Vec::new()));

        let three = digest(&["Work on a branch", "main is protected here", "Open a hand-in"]);
        let again = vec![Offered {
            same_as: Some("c1".to_string()),
            kind: LessonKind::Constraint,
            text: "Never push to main".to_string(),
            said: 2,
        }];
        let shown = preview(&three, Some(A_CATALOG_MODEL.into()), None, |_, _| Ok((again.clone(), 0)));
        assert!(
            shown.text("s-2").contains("- [constraint] Never push to main (from [2], candidate c1 shown again)"),
            "{}",
            shown.text("s-2")
        );
        assert_eq!(shown.json("s-2")["taken"][0]["same_as"], "c1");
    }

    /// A preview sends only where a person is at its terminal, or where
    /// `--send` says the sending is meant; a dry run sends nothing wherever
    /// it is run.
    #[test]
    fn a_preview_a_program_runs_sends_nothing_unless_it_says_send() {
        assert_eq!(held_back(false, false, true), None, "a person at the terminal");
        assert_eq!(held_back(false, false, false), Some(NOBODY_AT_THE_TERMINAL), "a program ran it");
        assert_eq!(held_back(false, true, false), None, "a program that says --send");
        for (send, at_a_terminal) in [(false, false), (false, true), (true, false), (true, true)] {
            assert_eq!(held_back(true, send, at_a_terminal), Some(DRY_RUN));
        }
        let three = digest(&["Answer in Korean", "Never push to main", "Run the gate first"]);
        let held = preview(&three, Some(A_CATALOG_MODEL.into()), Some(NOBODY_AT_THE_TERMINAL), never_asked);
        assert_eq!(held.outcome, Outcome::NotAsked(NOBODY_AT_THE_TERMINAL));
        assert!(held.text("s-1").contains("--send sends it"), "{}", held.text("s-1"));
    }

    #[test]
    fn the_words_it_does_not_know_get_the_usage() {
        let args = |list: &[&str]| list.iter().map(ToString::to_string).collect::<Vec<_>>();
        assert!(run(&args(&[])).unwrap_err().contains("zo dream-lessons preview"));
        assert!(run(&args(&["preview"])).unwrap_err().contains("zo dream-lessons preview"));
        assert!(run(&args(&["preview", "x", "--keep"])).unwrap_err().contains("unknown word"));
        assert!(run(&args(&["preview", "x", "--candidates"])).unwrap_err().contains("needs a file"));
        assert_eq!(run(&args(&["--help"])).as_deref(), Ok(USAGE));
    }
}
