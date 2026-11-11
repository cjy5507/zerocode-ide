//! Lessons a session's own words teach.
//!
//! Between sessions the dreamer may ask a model one question about a session:
//! of what the person said, what will still hold in the next session? This
//! module is everything about that question that is not IO — what the model is
//! shown ([`Digest`]), what it is asked ([`INSTRUCTIONS`], [`request`]), when it
//! is worth asking ([`worth_asking`]) and the reading of its answer
//! ([`read_reply`]). Reading the session, dialling a model and writing a
//! candidate down are the host's.
//!
//! Three rules keep a lesson honest.
//! - **Anchored or nothing.** A lesson names one thing the person said and
//!   quotes it. A quote that is not in that message, letter for letter, drops
//!   the lesson: what a model inferred from its own work, or made up, has no
//!   message to quote. The quote is checked and thrown away; only the model's
//!   one line is kept.
//! - **One line, of three kinds.** A preference, a constraint or a way of
//!   working. How a tool behaves is none of them: the memory protocol tells a
//!   model never to record it, and one sentence per failed tool is what this
//!   replaced.
//! - **Nothing is the usual answer.** Most sessions teach nothing that
//!   outlasts them, and a lesson offered once is only a candidate. The gate
//!   keeps it when a second session shows the same one, which the model says
//!   by answering with a candidate's id in place of new words.

use std::fmt::Write as _;

use serde_json::Value;

use crate::dreamer::LessonKind;

/// The most lessons one reply may offer. A reply with more keeps its first
/// ones: a session that teaches four durable things is rarer than a model
/// that lists.
pub const MAX_LESSONS: usize = 3;

/// The longest a lesson's one line may be, in bytes: what the store of the
/// person's patterns takes.
pub const LESSON_MAX_BYTES: usize = 512;

/// The shortest quote that anchors a lesson, in bytes after its whitespace is
/// squeezed. Two or three Korean words or one short English phrase; a word
/// like "ok" is in every session and shows nothing.
pub const QUOTE_MIN_BYTES: usize = 8;

/// How many new things the person must have said before a session is asked
/// about. A session of one or two messages is a single task: it has an
/// instruction, not a habit.
pub const NEW_SAID_TO_ASK: usize = 3;

/// How long a session waits after it was asked about before it is asked
/// again, unless it is ending.
pub const SECS_BETWEEN_ASKS: u64 = 30 * 60;

/// What the model is told, ahead of the request about one session.
pub const INSTRUCTIONS: &str = "\
You read what one person said to a coding assistant in one session, and say what in it will \
still hold in their next session on this project. Usually nothing will: answer \
{\"lessons\":[]} then.

A lesson is one of three kinds:
- preference: how this person wants the work done or answered (language, style, format, tools \
they favour).
- constraint: something that must or must not be done in this project, as the person stated it.
- workflow: a way of working the person asked for that would apply again.

Rules:
- Only what the person said counts. Quote the words that show it, copied exactly from one \
numbered message. A lesson without such a quote is discarded.
- Not a lesson: what this one task was about, a one-off instruction, how a tool or a command \
behaves, anything the repository already records, praise or complaint with no instruction in it.
- Write each lesson as one line of at most 200 characters, in the language the person wrote \
in, as a standing instruction (\"Answer in Korean\", not \"the person asked for Korean today\").
- If the session shows the same thing as a listed candidate, answer with that candidate's id \
in \"same_as\" and do not rewrite it.
- At most 3 lessons.

Answer with JSON only:
{\"lessons\":[{\"kind\":\"preference|constraint|workflow\",\"text\":\"...\",\"same_as\":null,\
\"said\":1,\"quote\":\"...\"}]}";

/// One thing the person said, with the end of what the assistant had said
/// just before — the thing a "no, not that" answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Said {
    /// What the person said.
    pub words: String,
    /// What the assistant had last said before it; empty when nothing was.
    pub before: String,
}

/// How much of a session a digest carries, in characters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DigestLimits {
    /// Of one thing the person said: its beginning.
    pub words: usize,
    /// Of what the assistant said before it: its end.
    pub before: usize,
    /// Of the whole digest. The newest things said are the ones kept.
    pub total: usize,
}

impl Default for DigestLimits {
    fn default() -> Self {
        // One machine's month of sessions (2026-11-11): under these a
        // session's digest is 5,185 characters at the median and 15,187 at
        // the ninth decile, and eight sessions in 315 reach the total.
        Self {
            words: 1_200,
            before: 400,
            total: 24_000,
        }
    }
}

/// What the model is shown of a session: the newest things the person said
/// that fit, oldest first, each on one line and cut to its limit. A reply
/// names one by its place in this list, from 1, and is checked against these
/// very words — a model can only quote what it was shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Digest {
    shown: Vec<Said>,
}

impl Digest {
    /// The digest of `said`, which is in the order it was said.
    #[must_use]
    pub fn of(said: &[Said], limits: DigestLimits) -> Self {
        let mut shown = Vec::new();
        let mut left = limits.total;
        for one in said.iter().rev() {
            let cut = Said {
                words: beginning(&squeezed(&one.words), limits.words),
                before: ending(&squeezed(&one.before), limits.before),
            };
            if cut.words.is_empty() {
                continue;
            }
            let size = cut.words.chars().count() + cut.before.chars().count();
            if size > left && !shown.is_empty() {
                break;
            }
            left = left.saturating_sub(size);
            shown.push(cut);
        }
        shown.reverse();
        Self { shown }
    }

    /// How many things the person said it shows.
    #[must_use]
    pub fn len(&self) -> usize {
        self.shown.len()
    }

    /// Whether it shows nothing — a session to ask nothing about.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.shown.is_empty()
    }

    /// Its size in characters, as a request carries it.
    #[must_use]
    pub fn chars(&self) -> usize {
        self.text().chars().count()
    }

    /// The numbered lines a request carries.
    #[must_use]
    pub fn text(&self) -> String {
        let mut text = String::new();
        for (index, one) in self.shown.iter().enumerate() {
            let number = index + 1;
            if one.before.is_empty() {
                let _ = writeln!(text, "[{number}] the person: {}", one.words);
            } else {
                let _ = writeln!(text, "[{number}] the assistant had said: {}", one.before);
                let _ = writeln!(text, "    the person: {}", one.words);
            }
        }
        text
    }

    /// The words shown at place `number`, from 1.
    fn words_at(&self, number: usize) -> Option<&str> {
        let index = number.checked_sub(1)?;
        self.shown.get(index).map(|one| one.words.as_str())
    }
}

/// A candidate the gate has not kept yet, as a request shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pending {
    /// The short name the request gives it (`c1`), which a reply answers with.
    pub id: String,
    pub kind: LessonKind,
    /// Its one line.
    pub text: String,
}

/// What the model is asked about one session: the candidates earlier sessions
/// left, then what the person said in this one.
#[must_use]
pub fn request(pending: &[Pending], digest: &Digest) -> String {
    let mut text = String::new();
    if pending.is_empty() {
        text.push_str("Candidates from earlier sessions: none.\n");
    } else {
        text.push_str("Candidates from earlier sessions:\n");
        for candidate in pending {
            let _ = writeln!(text, "{} [{}] {}", candidate.id, candidate.kind, candidate.text);
        }
    }
    text.push_str("\nWhat the person said in this session:\n");
    text.push_str(&digest.text());
    text
}

/// Whether a session is worth asking about now: it has enough that is new,
/// and it is ending or was not asked about a moment ago.
#[must_use]
pub fn worth_asking(new_said: usize, secs_since_asked: Option<u64>, ending: bool) -> bool {
    new_said >= NEW_SAID_TO_ASK
        && (ending || secs_since_asked.is_none_or(|secs| secs >= SECS_BETWEEN_ASKS))
}

/// A lesson a reply offered and the reading took.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Offered {
    /// The candidate the session showed again, by the id the request gave it;
    /// `None` for a lesson in new words.
    pub same_as: Option<String>,
    pub kind: LessonKind,
    /// The lesson's one line: the candidate's own when it is one shown again.
    pub text: String,
    /// The place in the digest, from 1, of the thing said that it quoted.
    pub said: usize,
}

/// What a reply came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadReply {
    /// The lessons taken, in the reply's order.
    pub taken: Vec<Offered>,
    /// How many the reply offered that were not taken: no quote in the
    /// message it named, a kind or a line outside the rules, one past the
    /// third, a second saying of the same thing.
    pub dropped: usize,
}

/// A reply with no answer of the asked shape in it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Unreadable;

/// Read a model's `reply` against the `digest` it was asked about and the
/// `pending` candidates it was shown.
///
/// A lesson is taken when its kind is one of the three, its line fits the
/// store, and its quote stands — whitespace aside — in the message it names.
/// A lesson that answers with a candidate's id is that candidate shown again
/// and carries the candidate's own kind and line; an id the request never
/// gave is read as no id.
///
/// # Errors
/// [`Unreadable`] when the reply holds no JSON object with a `lessons` list:
/// the whole reply is set aside.
pub fn read_reply(reply: &str, digest: &Digest, pending: &[Pending]) -> Result<ReadReply, Unreadable> {
    let offered = offered_lessons(reply).ok_or(Unreadable)?;
    let mut taken: Vec<Offered> = Vec::new();
    for lesson in &offered {
        if taken.len() == MAX_LESSONS {
            break;
        }
        let Some(read) = read_lesson(lesson, digest, pending) else {
            continue;
        };
        let said_before = taken
            .iter()
            .any(|earlier| earlier.text == read.text || (read.same_as.is_some() && earlier.same_as == read.same_as));
        if !said_before {
            taken.push(read);
        }
    }
    let dropped = offered.len() - taken.len();
    Ok(ReadReply { taken, dropped })
}

/// The `lessons` list of the JSON object in `reply`, which may stand in a
/// fence or after a sentence.
fn offered_lessons(reply: &str) -> Option<Vec<Value>> {
    let object = reply.get(reply.find('{')?..=reply.rfind('}')?)?;
    let json: Value = serde_json::from_str(object).ok()?;
    json.get("lessons")?.as_array().cloned()
}

fn read_lesson(lesson: &Value, digest: &Digest, pending: &[Pending]) -> Option<Offered> {
    let said = lesson.get("said").and_then(place)?;
    let quote = squeezed(lesson.get("quote")?.as_str()?);
    if quote.len() < QUOTE_MIN_BYTES || !digest.words_at(said)?.contains(&quote) {
        return None;
    }
    let again = lesson
        .get("same_as")
        .and_then(Value::as_str)
        .and_then(|id| pending.iter().find(|candidate| candidate.id == id.trim()));
    if let Some(candidate) = again {
        return Some(Offered {
            same_as: Some(candidate.id.clone()),
            kind: candidate.kind,
            text: candidate.text.clone(),
            said,
        });
    }
    let kind = kind_named(lesson.get("kind")?.as_str()?)?;
    let text = lesson.get("text")?.as_str()?.trim();
    line_allowed(text).then(|| Offered {
        same_as: None,
        kind,
        text: text.to_string(),
        said,
    })
}

/// A place in the digest as a reply writes it: a number, or a number in quotes.
fn place(value: &Value) -> Option<usize> {
    let number = match value {
        Value::Number(number) => number.as_u64()?,
        Value::String(text) => text.trim().parse().ok()?,
        _ => return None,
    };
    usize::try_from(number).ok()
}

/// The kind a reply names. A gotcha is not one a reply may offer: that kind is
/// how a tool misbehaves.
fn kind_named(name: &str) -> Option<LessonKind> {
    [LessonKind::Preference, LessonKind::Constraint, LessonKind::Workflow]
        .into_iter()
        .find(|kind| kind.as_str() == name.trim())
}

/// Whether `text` can be a lesson's one line: something, on one line, with no
/// control character, within [`LESSON_MAX_BYTES`].
#[must_use]
pub fn line_allowed(text: &str) -> bool {
    !text.trim().is_empty() && text.len() <= LESSON_MAX_BYTES && !text.chars().any(char::is_control)
}

/// `text` with every run of whitespace — a line break among them — as one
/// space, and none at its ends.
fn squeezed(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The first `limit` characters of `text`, marked when more followed.
fn beginning(text: &str, limit: usize) -> String {
    match text.char_indices().nth(limit) {
        Some((end, _)) => format!("{}…", &text[..end]),
        None => text.to_string(),
    }
}

/// The last `limit` characters of `text`, marked when more came before; none
/// of it when the limit is none.
fn ending(text: &str, limit: usize) -> String {
    let count = text.chars().count();
    if count <= limit {
        return text.to_string();
    }
    if limit == 0 {
        return String::new();
    }
    let start = text.char_indices().nth(count - limit).map_or(text.len(), |(start, _)| start);
    format!("…{}", &text[start..])
}

#[cfg(test)]
mod tests {
    use super::{
        line_allowed, read_reply, request, worth_asking, Digest, DigestLimits, Offered, Pending,
        ReadReply, Said, Unreadable, INSTRUCTIONS, LESSON_MAX_BYTES, MAX_LESSONS,
        NEW_SAID_TO_ASK, SECS_BETWEEN_ASKS,
    };
    use crate::dreamer::LessonKind;

    fn said(words: &str) -> Said {
        Said {
            words: words.to_string(),
            before: String::new(),
        }
    }

    fn session() -> Digest {
        Digest::of(
            &[
                said("이 저장소의 테스트를 돌려 줘"),
                Said {
                    words: "아니 영어 말고 한국어로 답해.\n그리고 커밋은 내가 할게".to_string(),
                    before: "I ran the tests and all of them pass. Shall I commit?".to_string(),
                },
                said("use pnpm here, never npm"),
            ],
            DigestLimits::default(),
        )
    }

    fn pending() -> Vec<Pending> {
        vec![Pending {
            id: "c1".to_string(),
            kind: LessonKind::Constraint,
            text: "Use pnpm in this project, never npm".to_string(),
        }]
    }

    fn taken(reply: &str) -> Vec<Offered> {
        read_reply(reply, &session(), &pending()).expect("a readable reply").taken
    }

    /// The digest shows each thing said on one line under its number, with
    /// what the assistant had said before it, and a request carries the
    /// candidates ahead of it.
    #[test]
    fn a_request_shows_the_candidates_and_the_numbered_things_said() {
        let digest = session();
        assert_eq!(digest.len(), 3);
        let text = request(&pending(), &digest);
        assert!(text.contains("c1 [constraint] Use pnpm in this project, never npm"), "{text}");
        assert!(text.contains("[1] the person: 이 저장소의 테스트를 돌려 줘\n"), "{text}");
        assert!(
            text.contains(
                "[2] the assistant had said: I ran the tests and all of them pass. Shall I commit?\n    \
                 the person: 아니 영어 말고 한국어로 답해. 그리고 커밋은 내가 할게\n"
            ),
            "a line break in what was said is one space: {text}"
        );
        assert!(text.ends_with("[3] the person: use pnpm here, never npm\n"), "{text}");
        assert_eq!(digest.chars(), digest.text().chars().count());

        let alone = request(&[], &digest);
        assert!(alone.starts_with("Candidates from earlier sessions: none.\n"), "{alone}");
        assert!(INSTRUCTIONS.contains("{\"lessons\":[]}"));
    }

    /// A long message keeps its beginning and what came before it keeps its
    /// end, each marked; over the total, the newest things said are the ones
    /// shown, and they keep the order they were said in.
    #[test]
    fn a_digest_cuts_each_thing_said_and_keeps_the_newest_within_its_total() {
        let limits = DigestLimits {
            words: 5,
            before: 4,
            total: 100,
        };
        let cut = Digest::of(
            &[Said {
                words: "가나다라마바사".to_string(),
                before: "abcdefgh".to_string(),
            }],
            limits,
        );
        assert_eq!(cut.text(), "[1] the assistant had said: …efgh\n    the person: 가나다라마…\n");

        let tight = DigestLimits {
            words: 10,
            before: 0,
            total: 12,
        };
        let newest = Digest::of(&[said("first one"), said("second"), said("third")], tight);
        assert_eq!(newest.text(), "[1] the person: second\n[2] the person: third\n");

        let one = Digest::of(&[said("only this, and it is past the total")], tight);
        assert_eq!(one.len(), 1, "the newest thing said is shown whatever the total");
        assert!(Digest::of(&[said("  \n ")], tight).is_empty(), "nothing said is nothing shown");
    }

    /// A lesson is taken when its quote stands in the message it names; the
    /// quote's own line breaks and spacing do not matter.
    #[test]
    fn a_lesson_is_taken_when_its_quote_stands_in_the_message_it_names() {
        let reply = r#"{"lessons":[{"kind":"preference","text":"한국어로 답한다","same_as":null,"said":2,"quote":"한국어로   답해.\n그리고"}]}"#;
        assert_eq!(
            taken(reply),
            [Offered {
                same_as: None,
                kind: LessonKind::Preference,
                text: "한국어로 답한다".to_string(),
                said: 2,
            }]
        );
        let fenced = format!("Here is what I found:\n```json\n{reply}\n```\n");
        assert_eq!(taken(&fenced).len(), 1, "a fence and a sentence around the object are read past");
        let quoted_number = reply.replace("\"said\":2", "\"said\":\"2\"");
        assert_eq!(taken(&quoted_number).len(), 1);
    }

    /// What the person did not say is no lesson: a quote from another
    /// message, from the assistant's words, from nowhere, or too short to
    /// show anything.
    #[test]
    fn a_lesson_whose_quote_is_not_in_the_message_it_names_is_dropped() {
        for (said, quote) in [
            (1, "한국어로 답해"),
            (2, "Shall I commit?"),
            (2, "always write tests first"),
            (3, "pnpm"),
            (4, "use pnpm here"),
            (0, "use pnpm here"),
        ] {
            let reply = format!(
                r#"{{"lessons":[{{"kind":"preference","text":"a line","said":{said},"quote":"{quote}"}}]}}"#
            );
            let read = read_reply(&reply, &session(), &pending()).expect("readable");
            assert_eq!(read, ReadReply { taken: Vec::new(), dropped: 1 }, "{said} {quote}");
        }
    }

    /// A lesson's kind is one of the three and its line is one line the store
    /// takes; a gotcha — how a tool misbehaves — is not offered.
    #[test]
    fn a_lesson_outside_the_three_kinds_or_the_one_line_is_dropped() {
        let offer = |kind: &str, text: &str| {
            let text = serde_json::to_string(text).expect("a JSON string");
            format!(r#"{{"lessons":[{{"kind":"{kind}","text":{text},"said":3,"quote":"use pnpm here"}}]}}"#)
        };
        assert_eq!(taken(&offer("constraint", "Use pnpm, never npm")).len(), 1);
        assert_eq!(taken(&offer("workflow", " Use pnpm, never npm "))[0].text, "Use pnpm, never npm");
        let too_long = "가".repeat(LESSON_MAX_BYTES / 3 + 1);
        for (kind, text) in [
            ("gotcha", "npm is error-prone here"),
            ("habit", "Use pnpm"),
            ("constraint", ""),
            ("constraint", "two\nlines"),
            ("constraint", too_long.as_str()),
        ] {
            assert!(taken(&offer(kind, text)).is_empty(), "{kind} {text:.20}");
        }
        assert!(line_allowed(&"a".repeat(LESSON_MAX_BYTES)));
        assert!(!line_allowed(&"a".repeat(LESSON_MAX_BYTES + 1)));
    }

    /// A lesson that answers with a candidate's id is that candidate shown
    /// again, in the candidate's own words whatever the reply wrote; an id the
    /// request never gave is no id.
    #[test]
    fn a_candidate_shown_again_is_answered_by_its_id_and_keeps_its_own_words() {
        let again = r#"{"lessons":[{"kind":"preference","text":"rewritten","same_as":"c1","said":3,"quote":"use pnpm here, never npm"}]}"#;
        assert_eq!(
            taken(again),
            [Offered {
                same_as: Some("c1".to_string()),
                kind: LessonKind::Constraint,
                text: "Use pnpm in this project, never npm".to_string(),
                said: 3,
            }]
        );
        let unknown = again.replace("\"c1\"", "\"c9\"");
        assert_eq!(
            taken(&unknown),
            [Offered {
                same_as: None,
                kind: LessonKind::Preference,
                text: "rewritten".to_string(),
                said: 3,
            }]
        );
        let without_quote = again.replace("use pnpm here, never npm", "pnpm is the package manager");
        assert!(taken(&without_quote).is_empty(), "a candidate is shown again only by the person's words");
    }

    /// A reply keeps its first three lessons and one saying of each; the rest
    /// are counted as dropped.
    #[test]
    fn a_reply_keeps_its_first_three_and_one_saying_of_each() {
        let lesson = |text: &str| format!(r#"{{"kind":"preference","text":"{text}","said":3,"quote":"use pnpm here"}}"#);
        let many = format!(
            r#"{{"lessons":[{},{},{},{},{}]}}"#,
            lesson("one"),
            lesson("one"),
            lesson("two"),
            lesson("three"),
            lesson("four")
        );
        let read = read_reply(&many, &session(), &pending()).expect("readable");
        let lines: Vec<&str> = read.taken.iter().map(|offered| offered.text.as_str()).collect();
        assert_eq!(lines, ["one", "two", "three"]);
        assert_eq!(read.taken.len(), MAX_LESSONS);
        assert_eq!(read.dropped, 2);
    }

    /// Nothing is an answer; no object, or one with no list, is not.
    #[test]
    fn nothing_is_an_answer_and_no_object_is_not() {
        let nothing = read_reply(r#"{"lessons":[]}"#, &session(), &pending());
        assert_eq!(nothing, Ok(ReadReply { taken: Vec::new(), dropped: 0 }));
        for reply in ["", "I found no lessons.", "{\"lessons\":", "{\"answer\":[]}", "[1,2]"] {
            assert_eq!(read_reply(reply, &session(), &pending()), Err(Unreadable), "{reply}");
        }
    }

    /// A session is asked about when it has three new things said, and not
    /// again within the half hour unless it is ending.
    #[test]
    fn a_session_is_asked_about_when_it_has_enough_new_and_was_not_just_asked() {
        assert!(!worth_asking(NEW_SAID_TO_ASK - 1, None, true));
        assert!(worth_asking(NEW_SAID_TO_ASK, None, false));
        assert!(!worth_asking(NEW_SAID_TO_ASK, Some(SECS_BETWEEN_ASKS - 1), false));
        assert!(worth_asking(NEW_SAID_TO_ASK, Some(SECS_BETWEEN_ASKS), false));
        assert!(worth_asking(NEW_SAID_TO_ASK, Some(1), true), "an ending session is its last chance");
    }
}
