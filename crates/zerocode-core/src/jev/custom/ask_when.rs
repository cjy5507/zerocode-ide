//! When a decision asks (t-107869): a decision may say it asks only where
//! one of its words stands in what its point is about — the command about to
//! run, the turn's last answer, the person's request — and, at a turn's end,
//! only after a turn that edited files. It says nothing by default, and then
//! it asks wherever it asked before.
//!
//! What it does not ask about sends no request and writes no row: the day's
//! count of those asks is all that is kept (`super::super::count::count_left_out`).
//! The words are the decision's own question in the person's words — a
//! decision about deleting asks about commands that delete — so a command
//! none of them names is one the decision answers `no` to, and paying the
//! wire to hear it is what this saves.
//!
//! It is kept beside the store's index ([`super::store::ask_when`]), never in
//! a definition: a definition and the index are read with
//! `deny_unknown_fields`, so a key added to either would make a window or a
//! zo built before this one lose that decision, or every decision. A program
//! that does not know the file reads the store as it did and asks as it did.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{Input, Lent, Point};

/// The most words one decision keeps.
pub const WORDS_MAX: usize = 40;

/// The most characters one word holds.
pub const WORD_MAX_CHARS: usize = 60;

/// When one decision asks. Neither part set is the default: it asks at every
/// one of its point's moments, as every decision did before this existed.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AskWhen {
    /// It asks only where one of these stands in what its point is about.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub words: Vec<String>,
    /// At a turn's end, it asks only after a turn that edited files.
    #[serde(default, skip_serializing_if = "is_false")]
    pub edited: bool,
}

/// Why a decision cannot be told when to ask this way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unfit {
    /// A decision an agent asks by name is asked on purpose: nothing is
    /// left out of what an agent chose to ask.
    AskedByAnAgent,
    /// Only a turn's end knows which files the turn edited.
    EditedOutsideATurnsEnd,
    /// A word with nothing in it.
    EmptyWord,
    /// A word past [`WORD_MAX_CHARS`].
    LongWord,
    /// A word holding a control character.
    ControlCharacter,
    /// More than [`WORDS_MAX`] words.
    TooManyWords,
}

impl Unfit {
    /// The rule's own word, as a refusal is spelled.
    #[must_use]
    pub const fn word(&self) -> &'static str {
        match self {
            Self::AskedByAnAgent => "asked_by_an_agent",
            Self::EditedOutsideATurnsEnd => "edited_outside_a_turns_end",
            Self::EmptyWord => "empty_word",
            Self::LongWord => "long_word",
            Self::ControlCharacter => "control_character",
            Self::TooManyWords => "too_many_words",
        }
    }
}

impl AskWhen {
    /// Whether it asks at every moment of its point — nothing said.
    #[must_use]
    pub fn always(&self) -> bool {
        self.words.is_empty() && !self.edited
    }

    /// This, kept for a decision asking at `point`: each word trimmed and its
    /// inner spaces made one, the same word twice kept once, in the order
    /// given.
    ///
    /// # Errors
    /// The first rule it breaks.
    pub fn fit_for(&self, point: Point) -> Result<Self, Unfit> {
        if self.always() {
            return Ok(Self::default());
        }
        if point == Point::AgentAsks {
            return Err(Unfit::AskedByAnAgent);
        }
        if self.edited && point != Point::TurnEnd {
            return Err(Unfit::EditedOutsideATurnsEnd);
        }
        let mut words: Vec<String> = Vec::with_capacity(self.words.len());
        for word in &self.words {
            if word.chars().any(char::is_control) {
                return Err(Unfit::ControlCharacter);
            }
            let word = word.split_whitespace().collect::<Vec<_>>().join(" ");
            if word.is_empty() {
                return Err(Unfit::EmptyWord);
            }
            if word.chars().count() > WORD_MAX_CHARS {
                return Err(Unfit::LongWord);
            }
            if !words.contains(&word) {
                words.push(word);
            }
        }
        if words.len() > WORDS_MAX {
            return Err(Unfit::TooManyWords);
        }
        Ok(Self {
            words,
            edited: self.edited,
        })
    }

    /// Whether a decision at `point` asks about `about` — the command about to
    /// run, the turn's last answer, or the person's request — after a turn
    /// that `edited` files or not. What an agent asks by name is always asked.
    #[must_use]
    pub fn admits(&self, point: Point, about: &str, edited: bool) -> bool {
        if point == Point::AgentAsks || self.always() {
            return true;
        }
        if self.edited && point == Point::TurnEnd && !edited {
            return false;
        }
        if self.words.is_empty() {
            return true;
        }
        let about = folded(about);
        self.words
            .iter()
            .any(|word| stands_in(&about, &folded(word)))
    }
}

impl AskWhen {
    /// [`Self::admits`] with what `point` lent: the text its words are looked
    /// for in ([`about_key`]), and whether the turn edited files. Where the
    /// asking program cannot see an agent's edits (`edits_seen` false) the
    /// turn counts as one that may have: a decision is never left out on a
    /// turn nobody could tell about.
    #[must_use]
    pub fn admits_lent(&self, point: Point, lent: &Lent, edits_seen: bool) -> bool {
        let about = about_key(point)
            .and_then(|key| lent.get(key))
            .and_then(Value::as_str)
            .unwrap_or_default();
        let edited = !edits_seen
            || lent
                .get(Input::EditedPaths.key())
                .and_then(Value::as_array)
                .is_some_and(|paths| !paths.is_empty());
        self.admits(point, about, edited)
    }
}

/// What `point` lends that a decision's words are looked for in.
#[must_use]
pub const fn about_key(point: Point) -> Option<&'static str> {
    match point {
        Point::BeforeCommand => Some(Input::Command.key()),
        Point::TurnEnd => Some(Input::FinalText.key()),
        Point::TurnStart => Some(Input::Task.key()),
        Point::AgentAsks => None,
    }
}

/// Whether `word` stands in `text`, both already [`folded`]: somewhere it
/// begins where a word may begin and ends where a word may end. Only an edge
/// of the word that is a letter, a digit or `_` needs a boundary beside it —
/// `-delete` stands in `--delete`, `checkout .` in `checkout .gitignore` — so
/// `rm` stands in `rm -rf x` and `/bin/rm x` and not in `format` or `rmdir`,
/// and a Korean word stands in a sentence whatever particle follows it.
fn stands_in(text: &str, word: &str) -> bool {
    let (Some(first), Some(last)) = (word.chars().next(), word.chars().next_back()) else {
        return false;
    };
    let (left, right) = (is_word_char(first), is_word_char(last));
    let mut from = 0;
    while let Some(found) = text[from..].find(word) {
        let at = from + found;
        let end = at + word.len();
        let starts = !left
            || text[..at]
                .chars()
                .next_back()
                .is_none_or(|before| !is_word_char(before));
        let ends = !right
            || text[end..]
                .chars()
                .next()
                .is_none_or(|after| !is_word_char(after));
        if starts && ends {
            return true;
        }
        from = at + text[at..].chars().next().map_or(1, char::len_utf8);
    }
    false
}

/// A character a word is made of, for the boundary rule: ASCII letters,
/// digits and `_`. Every other character is a boundary — a space, a slash, a
/// quote, a dash, and every letter outside ASCII.
const fn is_word_char(character: char) -> bool {
    character.is_ascii_alphanumeric() || character == '_'
}

/// `text` with every run of white space made one space and ASCII letters
/// made small: what a word is looked for in, and how a word is looked for.
fn folded(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for (at, piece) in text.split_whitespace().enumerate() {
        if at > 0 {
            out.push(' ');
        }
        out.push_str(&piece.to_ascii_lowercase());
    }
    out
}

#[allow(clippy::trivially_copy_pass_by_ref)]
const fn is_false(value: &bool) -> bool {
    !*value
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(words: &[&str]) -> AskWhen {
        AskWhen {
            words: words.iter().map(|word| (*word).to_string()).collect(),
            edited: false,
        }
    }

    #[test]
    fn nothing_said_asks_at_every_moment_as_before() {
        let always = AskWhen::default();
        for point in Point::ALL {
            assert!(always.admits(point, "", false));
            assert!(always.admits(point, "ls -la", true));
        }
    }

    #[test]
    fn a_command_no_word_names_is_not_asked_and_one_that_names_one_is() {
        let deleting = words(&["rm", "reset --hard", "branch -D", "-delete", "checkout ."]);
        let at = Point::BeforeCommand;
        for asked in [
            "rm -rf target",
            "cd x && /bin/rm a",
            "ls | xargs rm",
            "git reset  --hard HEAD~1",
            "git branch -D wt/x",
            "find . -name '*.tmp' -delete",
            "git push origin --delete wt/x",
            "git checkout .gitignore",
            "RM -RF X",
        ] {
            assert!(deleting.admits(at, asked, false), "{asked}");
        }
        for left_out in [
            "cargo fmt --all",
            "rmdir empty",
            "git status --porcelain",
            "echo perform format",
            "git reset --soft HEAD~1",
            "git checkout main",
        ] {
            assert!(!deleting.admits(at, left_out, false), "{left_out}");
        }
    }

    #[test]
    fn a_korean_word_stands_whatever_particle_follows_it() {
        let finished = words(&["완료"]);
        assert!(finished.admits(Point::TurnEnd, "작업을 완료했습니다.", false));
        assert!(!finished.admits(Point::TurnEnd, "아직 진행 중입니다.", false));
    }

    #[test]
    fn a_turns_end_that_edited_nothing_is_not_asked_when_the_decision_says_so() {
        let edits = AskWhen {
            words: Vec::new(),
            edited: true,
        };
        assert!(!edits.admits(Point::TurnEnd, "done", false));
        assert!(edits.admits(Point::TurnEnd, "done", true));
        let both = AskWhen {
            words: vec!["done".to_string()],
            edited: true,
        };
        assert!(both.admits(Point::TurnEnd, "all done", true));
        assert!(!both.admits(Point::TurnEnd, "all done", false));
        assert!(!both.admits(Point::TurnEnd, "still going", true));
    }

    #[test]
    fn what_an_agent_asks_by_name_is_always_asked() {
        assert!(words(&["rm"]).admits(Point::AgentAsks, "nothing named", false));
        assert_eq!(
            words(&["rm"]).fit_for(Point::AgentAsks),
            Err(Unfit::AskedByAnAgent)
        );
    }

    #[test]
    fn words_are_kept_trimmed_once_each_and_within_the_rules() {
        let kept = words(&["  reset   --hard ", "rm", "rm"]).fit_for(Point::BeforeCommand);
        assert_eq!(kept, Ok(words(&["reset --hard", "rm"])));
        assert_eq!(
            words(&["  "]).fit_for(Point::BeforeCommand),
            Err(Unfit::EmptyWord)
        );
        assert_eq!(
            words(&["a\u{7}b"]).fit_for(Point::BeforeCommand),
            Err(Unfit::ControlCharacter)
        );
        let long = "x".repeat(WORD_MAX_CHARS + 1);
        assert_eq!(
            words(&[long.as_str()]).fit_for(Point::BeforeCommand),
            Err(Unfit::LongWord)
        );
        let many: Vec<String> = (0..=WORDS_MAX).map(|at| format!("w{at}")).collect();
        let many: Vec<&str> = many.iter().map(String::as_str).collect();
        assert_eq!(
            words(&many).fit_for(Point::BeforeCommand),
            Err(Unfit::TooManyWords)
        );
        let edited = AskWhen {
            words: Vec::new(),
            edited: true,
        };
        assert_eq!(
            edited.fit_for(Point::BeforeCommand),
            Err(Unfit::EditedOutsideATurnsEnd)
        );
        assert_eq!(edited.fit_for(Point::TurnEnd), Ok(edited.clone()));
        assert_eq!(
            AskWhen::default().fit_for(Point::AgentAsks),
            Ok(AskWhen::default())
        );
    }

    #[test]
    fn it_is_written_with_only_what_was_said() {
        assert_eq!(serde_json::to_string(&AskWhen::default()).unwrap(), "{}");
        assert_eq!(
            serde_json::to_string(&words(&["rm"])).unwrap(),
            r#"{"words":["rm"]}"#
        );
        let read: AskWhen = serde_json::from_str(r#"{"edited":true,"later":1}"#).unwrap();
        assert!(
            read.edited && read.words.is_empty(),
            "a key a later build adds is read past"
        );
    }
}

/// The words' reach on the answers a decision already gave (t-107869, the
/// acceptance's first check), run by hand and never a gate: no request, no
/// key. `JEV_ASK_WHEN_SEED` names the decision rows of this machine joined to
/// the command each was asked about (`{"cases":[{"decision","chosen",
/// "source","command"}]}`, built by reading the ledgers and transcripts —
/// kept in a scratch folder, never committed), and `JEV_ASK_WHEN_WORDS` the
/// words each decision would be told (`{"<id>":{"words":[…]}}`). For every
/// decision it prints the commands its words would leave out, and how many
/// of those it answered yes — the asks the words would have lost.
#[cfg(test)]
mod reach {
    use std::collections::BTreeMap;

    use serde::Deserialize;

    use super::*;

    #[derive(Deserialize)]
    struct Seed {
        cases: Vec<Case>,
    }

    #[derive(Deserialize)]
    struct Case {
        decision: String,
        chosen: Option<String>,
        source: String,
        command: String,
    }

    #[test]
    #[ignore = "reads this machine's joined decision rows; run by hand"]
    fn the_words_leave_out_only_commands_the_decision_answered_no_to() {
        let read = |name: &str| {
            std::fs::read_to_string(
                std::env::var(name).unwrap_or_else(|_| panic!("{name} names a file")),
            )
            .unwrap_or_else(|error| panic!("{name}: {error}"))
        };
        let seed: Seed = serde_json::from_str(&read("JEV_ASK_WHEN_SEED")).expect("a seed");
        let words: BTreeMap<String, AskWhen> =
            serde_json::from_str(&read("JEV_ASK_WHEN_WORDS")).expect("words");
        let mut report = serde_json::Map::new();
        for (decision, said) in &words {
            let said = said
                .fit_for(Point::BeforeCommand)
                .expect("words a command takes");
            let mut counted: BTreeMap<&str, u64> = BTreeMap::new();
            let mut lost: Vec<String> = Vec::new();
            for case in seed.cases.iter().filter(|case| case.decision == *decision) {
                let asked = said.admits(Point::BeforeCommand, &case.command, false);
                let yes = case.chosen.as_deref() == Some("yes");
                *counted.entry("answered").or_default() += 1;
                *counted
                    .entry(if asked { "asked" } else { "leftOut" })
                    .or_default() += 1;
                if !asked {
                    *counted
                        .entry(if case.source == "zo" {
                            "leftOutZo"
                        } else {
                            "leftOutPanes"
                        })
                        .or_default() += 1;
                }
                if yes {
                    *counted
                        .entry(if asked { "yesAsked" } else { "yesLeftOut" })
                        .or_default() += 1;
                    if !asked {
                        lost.push(
                            super::super::version_of(case.command.as_bytes())[..16].to_string(),
                        );
                    }
                }
            }
            report.insert(
                decision.clone(),
                serde_json::json!({ "counted": counted, "lostYesFingerprints": lost }),
            );
        }
        println!(
            "{}",
            serde_json::to_string_pretty(&report).expect("a report")
        );
    }
}
