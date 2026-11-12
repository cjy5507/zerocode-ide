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
//! ```text
//! zo dream-lessons preview <session id|name|path> [--dry-run] [--json]
//! ```

use core_types::Session;
use decision_core::said_lessons::{Digest, Offered, NEW_SAID_TO_ASK};
use serde_json::{json, Value};
use tools::fast_model::FastModel;
use tools::said_lessons::{ask, digest_of, model_of};

pub const USAGE: &str = "\
zo dream-lessons preview <session id|name|path> [--dry-run] [--json]

  What the dreamer would ask about one session and take from the answer,
  shown and not kept: nothing is written to memory, the vault or the
  session. It sends what the person said in that session (the beginning of
  each message, and the end of what the assistant had said before it) to the
  fast model of the provider the session ran on — one request. --dry-run
  sends nothing: it names the model and the size of what would be shown.
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

/// # Errors
/// The usage for words it does not know, or why the session could not be read.
pub fn run(args: &[String]) -> Result<String, String> {
    let (mut dry_run, mut as_json, mut words) = (false, false, Vec::new());
    for arg in args {
        match arg.as_str() {
            "--help" | "-h" => return Ok(USAGE.to_string()),
            "--dry-run" => dry_run = true,
            "--json" => as_json = true,
            flag if flag.starts_with("--") => return Err(format!("unknown word {flag:?}\n{USAGE}")),
            word => words.push(word),
        }
    }
    let ["preview", reference] = words.as_slice() else {
        return Err(USAGE.to_string());
    };
    let handle = crate::session_registry::resolve_session_reference(reference).map_err(|error| error.to_string())?;
    let session = Session::load_from_path(&handle.path).map_err(|error| error.to_string())?;
    let digest = digest_of(&session.messages);
    let session_model = model_of(&session.messages).map(str::to_string);
    let preview = preview(&digest, session_model, dry_run, |digest, model| {
        ask(digest, &[], model).map(|asked| (asked.read.taken, asked.read.dropped))
    });
    Ok(if as_json {
        preview.json(&session.session_id).to_string()
    } else {
        preview.text(&session.session_id)
    })
}

/// The preview of a session whose digest is `digest`: asked through `ask`
/// only when a pass would ask about it, a model answered in it, and this is
/// not a dry run.
fn preview(
    digest: &Digest,
    session_model: Option<String>,
    dry_run: bool,
    ask: impl FnOnce(&Digest, &str) -> Result<(Vec<Offered>, usize), String>,
) -> Preview {
    let asked_model = session_model.as_deref().map(|model| FastModel::named_for(Some(model)));
    let outcome = match session_model.as_deref() {
        _ if digest.len() < NEW_SAID_TO_ASK => Outcome::NotAsked(TOO_LITTLE_SAID),
        None => Outcome::NotAsked(NO_MODEL),
        Some(_) if dry_run => Outcome::NotAsked(DRY_RUN),
        Some(model) => match ask(digest, model) {
            Ok((taken, dropped)) => Outcome::Asked { taken, dropped },
            Err(error) => Outcome::Failed(error),
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
            })).collect::<Vec<_>>(),
            "written": "nothing",
        })
    }

    fn text(&self, session_id: &str) -> String {
        let mut text = format!(
            "session {session_id}: the person said {} thing(s) a pass would show, {} characters\n",
            self.shown, self.chars
        );
        match (&self.session_model, &self.asked_model) {
            (Some(ran_on), Some(asked)) => text.push_str(&format!("ran on {ran_on}; the fast model asked is {asked}\n")),
            _ => text.push_str("no model answered in it\n"),
        }
        match &self.outcome {
            Outcome::NotAsked(why) => text.push_str(&format!("not asked: {why}\n")),
            Outcome::Failed(error) => text.push_str(&format!("asked, and it failed: {error}\n")),
            Outcome::Asked { taken, dropped } => {
                text.push_str(&format!("asked: {} lesson(s) taken, {dropped} offered and not taken\n", taken.len()));
                for lesson in taken {
                    text.push_str(&format!("- [{}] {} (from [{}])\n", lesson.kind.as_str(), lesson.text, lesson.said));
                }
            }
        }
        text.push_str("nothing was written");
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use decision_core::dreamer::LessonKind;
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

    fn never_asked(_: &Digest, _: &str) -> Result<(Vec<Offered>, usize), String> {
        panic!("a request was made")
    }

    /// Nothing is sent for a session a pass would not ask about, for one no
    /// model answered in, or on a dry run — and the dry run still names the
    /// model and the size.
    #[test]
    fn a_preview_sends_nothing_where_a_pass_would_not_ask_or_on_a_dry_run() {
        let short = preview(&digest(&["one", "two"]), Some("claude-opus-5-5".into()), false, never_asked);
        assert_eq!(short.outcome, Outcome::NotAsked(TOO_LITTLE_SAID));
        let three = digest(&["Answer in Korean", "Never push to main", "Run the gate first"]);
        let no_model = preview(&three, None, false, never_asked);
        assert_eq!(no_model.outcome, Outcome::NotAsked(NO_MODEL));
        assert_eq!(no_model.asked_model, None);
        let dry = preview(&three, Some("claude-opus-5-5".into()), true, never_asked);
        assert_eq!(dry.outcome, Outcome::NotAsked(DRY_RUN));
        assert_eq!(dry.shown, 3);
        assert_eq!(dry.chars, three.chars());
        let asked_model = dry.asked_model.clone().expect("the model that would be asked");
        assert_eq!(
            api::detect_provider_kind(&asked_model),
            api::detect_provider_kind("claude-opus-5-5"),
            "the same provider's: {asked_model}"
        );
        let json = dry.json("s-1");
        assert_eq!(json["asked"], false);
        assert_eq!(json["not_asked"], DRY_RUN);
        assert_eq!(json["written"], "nothing");
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
        let shown = preview(&three, Some("model-x".into()), false, |digest, model| {
            asked_with.push((digest.len(), model.to_string()));
            Ok((taken.clone(), 1))
        });
        assert_eq!(asked_with, [(3, "model-x".to_string())]);
        assert_eq!(shown.outcome, Outcome::Asked { taken, dropped: 1 });
        let text = shown.text("s-1");
        assert!(text.contains("asked: 1 lesson(s) taken, 1 offered and not taken"), "{text}");
        assert!(text.contains("- [constraint] Never push to main (from [2])"), "{text}");
        assert!(text.ends_with("nothing was written"), "{text}");
        let json = shown.json("s-1");
        assert_eq!(json["taken"][0]["kind"], "constraint");
        assert_eq!(json["dropped"], 1);

        let failed = preview(&three, Some("model-x".into()), false, |_, _| Err("timed out after 45s".to_string()));
        assert_eq!(failed.outcome, Outcome::Failed("timed out after 45s".to_string()));
        assert_eq!(failed.json("s-1")["error"], "timed out after 45s");
    }

    #[test]
    fn the_words_it_does_not_know_get_the_usage() {
        let args = |list: &[&str]| list.iter().map(ToString::to_string).collect::<Vec<_>>();
        assert!(run(&args(&[])).unwrap_err().contains("zo dream-lessons preview"));
        assert!(run(&args(&["preview"])).unwrap_err().contains("zo dream-lessons preview"));
        assert!(run(&args(&["preview", "x", "--keep"])).unwrap_err().contains("unknown word"));
        assert_eq!(run(&args(&["--help"])).as_deref(), Ok(USAGE));
    }
}
