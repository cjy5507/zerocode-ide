//! The composer a delivery's words were measured landing in, and the
//! pasteboard beside it — a model for tests (t-17274).
//!
//! Measured 2026-09-30 against Claude Code 2.1.285 on macOS, its bundle read
//! for the rules below and each rule then driven in a pty with the CLI under
//! a sandbox that refuses every `com.apple.pasteboard*` lookup — so a reach
//! for the clipboard shows up as a refused lookup in the unified log, and
//! reads nothing. What the pty answered:
//!
//! - a whole paste frame never reached for the pasteboard (0 of 6);
//! - an empty frame did, and so did a paste end with no start — in every
//!   run watched for eight seconds (4 of 4), 7 of 12 in all: the frame holds
//!   nothing, which is what a terminal sends for Cmd+V when the clipboard
//!   holds only a picture, and the picture is attached as `[Image #N]`;
//! - a frame whose end came 2.5 s after its start did (1 of 4): the reader
//!   closes a paste that has been quiet for two seconds, and the end that
//!   follows is a paste end with no start;
//! - typed words, a line break typed as Ctrl+J, never did (0 of 4); a typed
//!   Ctrl+V did (1 of 1) — Ctrl+V is its picture key, and finding no picture
//!   it went on to read the clipboard's text with `pbpaste` (refused too).
//!
//! Read from the bundle, not driven: while the program starts, before its
//! composer stands, it collects what it is sent into a buffer that drops
//! every escape sequence — a frame's start and end among them — and makes a
//! carriage return a line break; that buffer, trimmed, seeds the composer.
//! A frame that straddles the hand-over loses its start to the buffer and
//! brings its end to the composer. Ctrl+C in that buffer ends the program.
//! And a paste (or one typed run longer than 800 characters, which it takes
//! as one) whose line ends in a picture file's relative name makes it read
//! the path of a file copied to the clipboard, to see whether that is the
//! file meant.

use super::*;

use std::time::{Duration, Instant};

use zerocode_pty::ready::{Line, Observed, Step};

/// A picture the person left on the clipboard: a PNG's first bytes.
const PICTURE: &[u8] = b"\x89PNG\r\n\x1a\n";

/// One typed run longer than this is taken as a paste.
const TYPED_RUN_AS_PASTE: usize = 800;

/// The endings of a picture file's name it looks up (`/\.(png|jpe?g|gif|webp)$/i`).
const PICTURE_FILES: [&str; 5] = [".png", ".jpg", ".jpeg", ".gif", ".webp"];

/// The pasteboard beside the composer: what it holds, and what asked it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Pasteboard {
    /// What the person left there. Nothing here ever writes it.
    picture: Vec<u8>,
    /// Reads of the picture: an empty paste, a paste end with no start,
    /// the picture key.
    picture_reads: usize,
    /// Reads of a copied file's path: a pasted line ending in a picture
    /// file's relative name.
    file_reads: usize,
}

impl Pasteboard {
    fn holding(picture: &[u8]) -> Self {
        Self {
            picture: picture.to_vec(),
            picture_reads: 0,
            file_reads: 0,
        }
    }
}

/// The measured composer, fed what a pane is written.
struct Composer {
    /// The start-up buffer, while the composer does not stand yet.
    starting: Option<String>,
    /// The line in the composer.
    line: String,
    /// A paste the reader has opened and not closed, and what it holds.
    paste: Option<String>,
    /// Lines the composer sent.
    sent: Vec<String>,
    /// Ctrl+C reached the start-up buffer: the program ended.
    ended: bool,
    pasteboard: Pasteboard,
    pictures: usize,
}

impl Composer {
    fn starting(pasteboard: Pasteboard) -> Self {
        Self {
            starting: Some(String::new()),
            line: String::new(),
            paste: None,
            sent: Vec::new(),
            ended: false,
            pasteboard,
            pictures: 0,
        }
    }

    /// The composer stands: the start-up buffer, trimmed, is its line.
    fn stand(&mut self) {
        if let Some(buffer) = self.starting.take() {
            self.line = buffer.trim().to_string();
        }
    }

    /// Two seconds without input: an open paste is closed with what it
    /// holds, and one holding nothing is simply dropped.
    fn quiet(&mut self) {
        if let Some(held) = self.paste.take()
            && !held.is_empty()
        {
            self.pasted(&held);
        }
    }

    /// One read's worth of input.
    fn take(&mut self, input: &str) {
        if self.ended {
            return;
        }
        let chars: Vec<char> = input.chars().collect();
        if self.starting.is_some() {
            self.collect(&chars);
        } else {
            self.read(&chars);
        }
    }

    /// The start-up buffer's rules: escape sequences dropped, a carriage
    /// return made a line break, other controls dropped, Ctrl+C the end.
    fn collect(&mut self, chars: &[char]) {
        let Some(buffer) = self.starting.as_mut() else {
            return;
        };
        let mut at = 0;
        while at < chars.len() {
            let one = chars[at];
            match one {
                '\u{3}' => {
                    self.ended = true;
                    return;
                }
                '\u{4}' => return,
                '\u{7f}' | '\u{8}' => {
                    buffer.pop();
                }
                '\u{1b}' => {
                    at += 1;
                    match chars.get(at) {
                        Some('[') => {
                            at += 1;
                            while chars.get(at).is_some_and(|c| u32::from(*c) < 64) {
                                at += 1;
                            }
                            at += 1;
                        }
                        Some(']' | 'P' | 'X' | '^' | '_') => {
                            at += 1;
                            while let Some(c) = chars.get(at) {
                                if *c == '\u{7}' {
                                    at += 1;
                                    break;
                                }
                                if *c == '\u{1b}' && chars.get(at + 1) == Some(&'\\') {
                                    at += 2;
                                    break;
                                }
                                at += 1;
                            }
                        }
                        Some('O') => at += 2,
                        Some('\u{1b}') | None => {}
                        Some(_) => at += 1,
                    }
                    continue;
                }
                '\r' => buffer.push('\n'),
                c if u32::from(c) < 32 && c != '\t' && c != '\n' => {}
                c => buffer.push(c),
            }
            at += 1;
        }
    }

    /// The standing composer's reader: paste frames, keys, typed runs.
    fn read(&mut self, chars: &[char]) {
        let mut run = String::new();
        let mut at = 0;
        while at < chars.len() {
            let one = chars[at];
            if one == '\u{1b}' {
                let end = sequence_end(chars, at);
                let sequence: String = chars[at..end].iter().collect();
                at = end;
                if sequence == "\u{1b}[200~" {
                    self.typed(&mut run);
                    self.paste = Some(String::new());
                } else if sequence == "\u{1b}[201~" {
                    self.typed(&mut run);
                    // The pasteboard of a paste end outside a paste is empty.
                    let held = self.paste.take().unwrap_or_default();
                    self.pasted(&held);
                } else if let Some(held) = self.paste.as_mut() {
                    held.push_str(&sequence);
                }
                continue;
            }
            at += 1;
            if let Some(held) = self.paste.as_mut() {
                held.push(one);
                continue;
            }
            if u32::from(one) >= 32 && one != '\u{7f}' {
                run.push(one);
                continue;
            }
            self.typed(&mut run);
            match one {
                '\r' => {
                    if !self.line.trim().is_empty() {
                        self.sent.push(std::mem::take(&mut self.line));
                    }
                }
                // Ctrl+J: its line-break key.
                '\n' => self.line.push('\n'),
                // Ctrl+V: its picture key.
                '\u{16}' => self.picture(),
                '\u{7f}' | '\u{8}' => {
                    self.line.pop();
                }
                _ => {}
            }
        }
        self.typed(&mut run);
    }

    /// A typed run, as one key: past the threshold it is taken as a paste.
    fn typed(&mut self, run: &mut String) {
        let run = std::mem::take(run);
        if run.chars().count() > TYPED_RUN_AS_PASTE {
            self.pasted(&run);
        } else {
            self.line.push_str(&run);
        }
    }

    /// A paste arrived: an empty one is a picture paste; any other is
    /// looked through for a picture file's name, then inserted.
    fn pasted(&mut self, text: &str) {
        if text.is_empty() || text == "[I" || text == "[O" {
            self.picture();
            return;
        }
        // Its pieces: the lines, and a space before a slash starts one.
        for piece in text.replace(" /", "\n/").split(['\n', '\r']) {
            let name = piece.trim().trim_matches(['"', '\'']).to_ascii_lowercase();
            if PICTURE_FILES.iter().any(|ending| name.ends_with(ending)) && !name.starts_with('/') {
                self.pasteboard.file_reads += 1;
            }
        }
        self.line
            .push_str(&text.replace("\r\n", "\n").replace('\r', "\n"));
    }

    /// Read the pasteboard's picture, and attach it when there is one.
    fn picture(&mut self) {
        self.pasteboard.picture_reads += 1;
        if !self.pasteboard.picture.is_empty() {
            self.pictures += 1;
            self.line.push_str(&format!("[Image #{}]", self.pictures));
        }
    }

    /// Everything the composer sent and still holds.
    fn arrived(&self) -> String {
        let mut all = self.sent.join("\n");
        all.push_str(&self.line);
        all
    }
}

/// Where the escape sequence starting at `at` ends: a CSI through its final
/// byte, any other escape and the character after it.
fn sequence_end(chars: &[char], at: usize) -> usize {
    if chars.get(at + 1) != Some(&'[') {
        return (at + 2).min(chars.len());
    }
    let mut end = at + 2;
    while chars
        .get(end)
        .is_some_and(|c| (0x20..0x40).contains(&u32::from(*c)))
    {
        end += 1;
    }
    (end + 1).min(chars.len())
}

/// The composer after `bytes`, standing once the first `stand` characters
/// were collected by its start-up buffer.
fn handed_over_at(bytes: &[u8], stand: usize) -> Composer {
    let chars: Vec<char> = text_of(bytes).chars().collect();
    let mut composer = Composer::starting(Pasteboard::holding(PICTURE));
    let (early, late) = chars.split_at(stand.min(chars.len()));
    composer.take(&early.iter().collect::<String>());
    composer.stand();
    composer.take(&late.iter().collect::<String>());
    composer
}

/// The composer after `bytes`, standing from the start, its reader quiet
/// for two seconds before character `quiet`.
fn quiet_at(bytes: &[u8], quiet: usize) -> Composer {
    let chars: Vec<char> = text_of(bytes).chars().collect();
    let mut composer = Composer::starting(Pasteboard::holding(PICTURE));
    composer.stand();
    let (before, after) = chars.split_at(quiet.min(chars.len()));
    composer.take(&before.iter().collect::<String>());
    composer.quiet();
    composer.take(&after.iter().collect::<String>());
    composer
}

fn text_of(bytes: &[u8]) -> &str {
    std::str::from_utf8(bytes).expect("a delivery writes text")
}

/// The letters of `text`, whatever spaces and line breaks came between
/// them: a hand-over trims the start-up buffer, and the model is asked
/// only whether the words arrived and nothing else did.
fn letters(text: &str) -> String {
    text.chars().filter(|c| !c.is_whitespace()).collect()
}

/// `bytes` at the measured composer, under every hand-over and every
/// two-second quiet it can meet: the pasteboard is never read nor changed,
/// nothing ends the program, and the words arrive and nothing else does.
fn only_the_words_arrive(bytes: &[u8], words: &str, road: &str) {
    let places = text_of(bytes).chars().count();
    let held = Pasteboard::holding(PICTURE);
    for at in 0..=places {
        for (how, composer) in [
            ("hand-over", handed_over_at(bytes, at)),
            ("quiet", quiet_at(bytes, at)),
        ] {
            let there = format!("{road}, {how} before character {at} of {bytes:?}");
            assert_eq!(
                composer.pasteboard, held,
                "the composer read the pasteboard ({there})"
            );
            assert!(!composer.ended, "the program was ended ({there})");
            assert_eq!(
                letters(&composer.arrived()),
                letters(words),
                "not the words, or not only them ({there})"
            );
        }
    }
}

/// Every byte `delivery` writes, fed the way the pump feeds it: a composer
/// that shook hands, then one showing every sign of readiness there is,
/// round after round on a clock past every wait — at a pane that reports
/// the prompts it takes and never takes this one, so the Enter pressed
/// again is in the stream too.
fn written(mut delivery: PromptDelivery, start: Instant) -> Vec<u8> {
    let line = Line {
        taken: Some(0),
        ..Line::default()
    };
    let handshake = Observed {
        wrote: true,
        bracketed_paste: true,
        ..Observed::default()
    };
    let ready = Observed {
        wrote: false,
        bracketed_paste: true,
        cursor_shows: 1,
        marker_written: true,
        marker_in_alt: true,
        alt_screen: true,
    };
    let mut bytes = Vec::new();
    let mut now = start;
    for round in 0..2_000 {
        let seen = if round == 0 { handshake } else { ready };
        match delivery.poll_line(seen, line, now) {
            Step::Write(more) | Step::Submit(more) => bytes.extend(more),
            Step::Done(_) => return bytes,
            Step::Waiting => {}
        }
        now += Duration::from_millis(50);
    }
    panic!("the delivery never settled: {delivery:?}");
}

/// A restart's words, a mail pointer's, and a line naming a picture file.
const WORDS: &str = "The window restarted and cut your last turn short. Continue exactly \
                     where you left off.\nYou have 1 orchestration message. Run `zerocode-orc \
                     check`.\nThe screenshot it asked about is before.png";

/// The model against its measurements: a whole frame is words, and a frame
/// that comes apart — at the hand-over, or at the reader's quiet — or holds
/// nothing is a picture paste, as is a typed Ctrl+V; typed words are words.
#[test]
fn the_model_answers_what_the_pty_answered() {
    let framed = zerocode_pty::encode_paste(WORDS, true);
    let places = text_of(&framed).chars().count();

    let whole = handed_over_at(&framed, 0);
    assert_eq!(
        whole.pasteboard.picture_reads, 0,
        "a whole frame read the picture"
    );
    assert_eq!(letters(&whole.arrived()), letters(WORDS));
    assert!(
        (0..=places).any(|at| handed_over_at(&framed, at).pasteboard.picture_reads > 0),
        "no hand-over inside the frame made a paste end with no start"
    );
    assert!(
        (0..=places).any(|at| quiet_at(&framed, at).pasteboard.picture_reads > 0),
        "no quiet inside the frame made a paste end with no start"
    );
    assert_eq!(
        whole.pasteboard.file_reads, 1,
        "a pasted line ending in a picture file's name did not look up a copied file"
    );

    let empty = handed_over_at(b"\x1b[200~\x1b[201~", 0);
    assert_eq!(empty.pasteboard.picture_reads, 1);
    assert_eq!(empty.arrived(), "[Image #1]");
    let stray = handed_over_at(b"\x1b[201~", 0);
    assert_eq!(stray.pasteboard.picture_reads, 1);

    let typed = handed_over_at(b"first line\nsecond line", 0);
    assert_eq!(typed.pasteboard, Pasteboard::holding(PICTURE));
    assert_eq!(typed.arrived(), "first line\nsecond line");
    let ctrl_v = handed_over_at(b"before\x16after", 0);
    assert_eq!(ctrl_v.pasteboard.picture_reads, 1);
    assert!(handed_over_at(b"no\x03more", 5).ended);
}

/// The measured row, every road that hands its composer words: whatever
/// the program is doing when they land — still starting, or with its
/// reader gone quiet mid-write — the words arrive and the pasteboard is
/// never read. The restoring worker's wait (the worker split) carries no
/// words at all and must write none.
#[test]
fn every_road_hands_the_measured_composer_its_words_and_never_its_pasteboard() {
    let row = "claude";
    let start = Instant::now();
    for readiness in [
        crate::cmd::terminal::PromptReadiness::Mounting,
        crate::cmd::terminal::PromptReadiness::Resting,
        crate::cmd::terminal::PromptReadiness::RestingBesideADraft,
    ] {
        let delivery = crate::cmd::terminal::prompt_delivery_for(
            WORDS.to_string(),
            true,
            Some(row),
            readiness,
            None,
            start,
        );
        only_the_words_arrive(&written(delivery, start), WORDS, &format!("{readiness:?}"));
    }
    let enter = crate::cmd::terminal::prompt_delivery_for(
        String::new(),
        true,
        Some(row),
        crate::cmd::terminal::PromptReadiness::EnterAgain(None),
        None,
        start,
    );
    only_the_words_arrive(&written(enter, start), "", "the Enter alone");
    let restoring = PromptDelivery::with_deadlines(
        String::new(),
        false,
        ready_signal_for(Some(row)),
        start,
        ready_quiet_for(Some(row)),
        ready_timeout_for(Some(row)),
    )
    .clearing(false)
    .guarded(zerocode_pty::ready::Guard::for_its_own_line(None));
    let waited = written(restoring, start);
    assert!(
        waited.is_empty(),
        "the restoring worker's wait wrote {waited:?} to the composer it waited for"
    );
}
