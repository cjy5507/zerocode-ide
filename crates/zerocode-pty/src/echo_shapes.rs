//! What an answer written to the pty comes back as, and where it sits in a
//! read that carried other things too.
//!
//! A terminal answers a program's question by writing to the pty master, and
//! whatever line discipline sits in between can copy that write straight back
//! out as ordinary output. Kept apart from the write path because what an echo
//! LOOKS like does not depend on who wrote it
//! (`pty-startup-reply-echo-shapes.ts`, #12112/#13137).
//!
//! **Two sources, and neither can be read off a flag.** Measured on this
//! machine, on a bare pty and at a `bash --norc -i` prompt:
//!
//! 1. **The kernel**, while ECHO is set. With ECHOCTL — the POSIX default —
//!    every control byte returns as a caret pair: `ESC [ 1 ; 1 R` comes back
//!    as `^[[1;1R`, and a colour report's terminator as `^[\`. A
//!    `stty -echoctl` tty echoes the answer verbatim instead.
//! 2. **The line editor**, in software. readline redraws a master write as if
//!    it had been typed, *while the kernel reports ECHO clear* — at that bash
//!    prompt the master's termios reads ECHO off and ICANON off, and writing
//!    `ESC [ 1 ; 1 R` still brings `BEL ; 1 R` back. So a clear bit is proof
//!    about the kernel and about nothing else, which is why the original
//!    stopped reading it to decide when to write (#15578) and recognises the
//!    echo on this side instead.
//!
//! What an editor does to an answer is not a guess here. Measured against
//! `bash --norc -i`, it eats the escape prefix it was reading as a key
//! binding, rings the bell, and types the rest:
//!
//! ```text
//! ESC [ 1 ; 1 R          -> BEL ; 1 R
//! ESC [ 0 n              -> BEL n
//! ESC [ ? 62 ; 22 c      -> BEL 62;22c
//! ESC [ ? 2026 ; 2 $ y   -> BEL 2026;2$y
//! ESC ] 11 ; rgb:… ESC \ -> BEL 11;rgb:…
//! ```
//!
//! Three bytes eaten for a CSI, two for an OSC, and an OSC's terminator
//! dropped with them. `zsh -f -i` answers the same for all but the cursor
//! report, which it redraws with a backspace in the middle — that one shape
//! is not projected here, and is the honest gap in this file.

/// One shape to watch for, and whether a torn one may be held for the rest.
///
/// `hold_partial` is the safety bit. A shape that does NOT begin with ESC can
/// be held across reads as a candidate, because no sequence a program is
/// waiting on shares a prefix with it — holding it steals nothing. A shape
/// that DOES begin with ESC must never be held: a read ending on a bare ESC
/// is a strict prefix of it, and holding that ESC takes it away from the
/// parser, so a sequence torn at its own ESC would never be understood. Such
/// a shape is matched only when it arrives whole.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EchoShape {
    /// The bytes this shape looks like on the way back.
    pub needle: Vec<u8>,
    /// Whether the tail of a read may be withheld as a possible beginning.
    pub hold_partial: bool,
}

/// Where an echo is in a read, if it is there at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EchoMatch {
    /// The whole shape is here, at this offset and this long.
    Whole { at: usize, len: usize },
    /// A read ended part-way into a shape that may be held; it starts here.
    Torn { at: usize },
    /// Nothing of any shape is in this read.
    Absent,
}

/// How the kernel writes one byte back when ECHOCTL is in force.
///
/// Control bytes come back as a caret and the character forty positions up
/// (`ESC` → `^[`, `BEL` → `^G`), which is what the terminal driver has done
/// since v7 and what a bare pty on this machine does. Tab and newline are
/// deliberately absent: the driver echoes those as themselves, and no answer
/// this terminal gives contains either.
fn caret_form(reply: &[u8]) -> Vec<u8> {
    let mut caret = Vec::with_capacity(reply.len() * 2);
    for &byte in reply {
        if byte < 0x20 {
            caret.push(b'^');
            caret.push(byte ^ 0x40);
        } else {
            caret.push(byte);
        }
    }
    caret
}

/// Every shape one answer can return as, most likely first.
///
/// The original projects only ESC into its caret pair
/// (`pty-startup-reply-echo-shapes.ts`, `replyEchoProjections`); this projects
/// every control byte, because a colour report ended with BEL comes back as
/// `^G` and an ESC-only rule walks straight past it. Measured, not reasoned:
/// writing `ESC ] 11 ; rgb:… BEL` to a cooked pty here returns
/// `^[]11;rgb:…^G`.
pub fn echo_shapes(reply: &[u8]) -> Vec<EchoShape> {
    let mut shapes = vec![EchoShape {
        needle: caret_form(reply),
        hold_partial: true,
    }];
    // readline rewrites an OSC answer and echoes it even where the kernel is
    // quiet, so this one is not an alternative to the caret form — it is the
    // other source, and both are armed at once.
    if reply.windows(2).any(|pair| pair == b"\x1b]") {
        let mut rewritten = Vec::with_capacity(reply.len());
        let mut at = 0;
        while at < reply.len() {
            if reply[at..].starts_with(b"\x1b]") {
                rewritten.push(0x07);
                at += 2;
            } else if reply[at..].starts_with(b"\x1b\\") {
                at += 2;
            } else {
                rewritten.push(reply[at]);
                at += 1;
            }
        }
        shapes.push(EchoShape {
            needle: rewritten,
            hold_partial: true,
        });
    }
    // The same editor, on the answers that are not OSC. The original projects
    // only its OSC rewrite; this projects the CSI one too, on the measurement
    // above — but only where what is left is long enough to be nobody else's.
    // `BEL n`, all that survives of `ESC [ 0 n`, is two bytes a child could
    // plausibly write; `BEL ;1R` is four that it could not.
    if reply.starts_with(b"\x1b[") && reply.len() > EDITOR_ATE_A_CSI {
        let mut rung = vec![0x07];
        rung.extend_from_slice(&reply[EDITOR_ATE_A_CSI..]);
        if rung.len() >= EDITOR_SHAPE_FLOOR {
            shapes.push(EchoShape {
                needle: rung,
                hold_partial: true,
            });
        }
    }
    // A `stty -echoctl` tty echoes the answer as it was written. Whole matches
    // only, because it begins with ESC — see [`EchoShape::hold_partial`]. We
    // wrote these exact bytes and we are the terminal, so a child emitting the
    // identical span in the same window is not worth trading a stolen
    // sequence for.
    shapes.push(EchoShape {
        needle: reply.to_vec(),
        hold_partial: false,
    });
    shapes
}

/// How much of a CSI answer a line editor swallows before it gives up.
///
/// Measured, not reasoned: `ESC [` and one byte more, whether that byte is a
/// parameter digit or the private `?`. The editor is reading a key binding and
/// stops at the first byte that cannot continue one.
const EDITOR_ATE_A_CSI: usize = 3;

/// The shortest editor-shaped needle worth watching for.
///
/// What is left of a short answer is a bell and a byte or two — a span a busy
/// child could write on its own, and cutting it would take real output off the
/// screen. Four is the length of the cursor report's remainder, which is the
/// shortest one measured that no child plausibly emits.
const EDITOR_SHAPE_FLOOR: usize = 4;

/// The earliest offset whose remainder of `data` begins `needle`, else `None`.
fn torn_at(needle: &[u8], data: &[u8]) -> Option<usize> {
    let first = data.len().saturating_sub(needle.len() - 1);
    (first..data.len()).find(|&at| needle.starts_with(&data[at..]))
}

/// Where the best of these shapes sits in this read.
///
/// The whole span is searched rather than only its head: the tty coalesces an
/// echo with whatever the shell and the program wrote around it, so anchoring
/// at the start recognises almost no real echo. A whole match beats a torn
/// one, and the earlier of two of a kind wins.
pub fn locate_echo(shapes: &[EchoShape], data: &[u8]) -> EchoMatch {
    let mut whole: Option<(usize, usize)> = None;
    let mut torn: Option<usize> = None;
    for shape in shapes {
        if shape.needle.is_empty() {
            continue;
        }
        if let Some(at) = data
            .windows(shape.needle.len())
            .position(|window| window == shape.needle.as_slice())
        {
            if whole.is_none_or(|(best, _)| at < best) {
                whole = Some((at, shape.needle.len()));
            }
            continue;
        }
        if !shape.hold_partial {
            continue;
        }
        if let Some(at) = torn_at(&shape.needle, data)
            && torn.is_none_or(|best| at < best)
        {
            torn = Some(at);
        }
    }
    match (whole, torn) {
        (Some((at, len)), _) => EchoMatch::Whole { at, len },
        (None, Some(at)) => EchoMatch::Torn { at },
        (None, None) => EchoMatch::Absent,
    }
}

/// How far past an answer its own echo is still worth looking for.
///
/// Bytes rather than reads: an echo is thirty-odd bytes, but nothing bounds
/// how the tty chunks them, and a per-read budget would be spent inside the
/// echo itself. This is a backstop against a stream that never carries the
/// echo at all — a program that asked, got its answer, and then printed a
/// splash — set far above anything an echo could arrive behind
/// (`pty-startup-reply-echo-shapes.ts`'s caller, `ECHO_SEARCH_BUDGET_BYTES`).
const ECHO_SEARCH_BUDGET: usize = 256 * 1024;

/// How many answers may be waiting for their echoes at once.
///
/// A program can ask faster than the tty returns, and every unanswered shape
/// is memory that outlives its read. The original caps the same queue at the
/// same number (`MAX_TRACKED_ECHOES`); past it the oldest is forgotten, which
/// costs one echo drawn on the screen rather than unbounded growth.
const MAX_EXPECTED_ECHOES: usize = 64;

/// One answer's shapes, and how much output is left to find them in.
#[derive(Debug)]
struct Expected {
    shapes: Vec<EchoShape>,
    budget: usize,
}

/// The echoes a lane is still expecting to have handed back to it.
///
/// Answers go out the moment they are owed — waiting for a quiet tty is what
/// the original tried first and removed, because the wait made the write
/// asynchronous and never removed the echo anyway (#15578). Recognising the
/// echo on the way back is the half that does the work, and it is the only
/// half that can cover a line editor echoing in software.
#[derive(Debug, Default)]
pub struct ExpectedEchoes {
    waiting: Vec<Expected>,
    /// The tail of a read withheld because it may be an echo beginning.
    held: Vec<u8>,
}

impl ExpectedEchoes {
    /// Watch for what this answer will look like coming back.
    pub fn expect(&mut self, reply: &[u8]) {
        if reply.is_empty() {
            return;
        }
        if self.waiting.len() >= MAX_EXPECTED_ECHOES {
            self.waiting.remove(0);
        }
        self.waiting.push(Expected {
            shapes: echo_shapes(reply),
            budget: ECHO_SEARCH_BUDGET,
        });
    }

    /// Whether anything is being watched for or withheld.
    pub fn is_quiet(&self) -> bool {
        self.waiting.is_empty() && self.held.is_empty()
    }

    /// The part of this read that is the child's own output.
    ///
    /// Borrowed and untouched when nothing is expected, which is what a lane
    /// spends almost all of its life doing. Otherwise the echoes are cut out,
    /// and a read that ended part-way into one keeps that tail back until the
    /// next read says what it was.
    pub fn sift<'a>(&mut self, chunk: &'a [u8]) -> std::borrow::Cow<'a, [u8]> {
        if self.is_quiet() {
            return std::borrow::Cow::Borrowed(chunk);
        }
        let mut data = std::mem::take(&mut self.held);
        data.extend_from_slice(chunk);
        loop {
            let found = self
                .waiting
                .iter()
                .enumerate()
                .map(|(which, expected)| (which, locate_echo(&expected.shapes, &data)))
                .min_by_key(|(_, found)| match found {
                    // A whole match anywhere beats a torn one, and the earlier
                    // of two of a kind wins.
                    EchoMatch::Whole { at, .. } => (0, *at),
                    EchoMatch::Torn { at } => (1, *at),
                    EchoMatch::Absent => (2, 0),
                });
            match found {
                Some((which, EchoMatch::Whole { at, len })) => {
                    self.waiting.remove(which);
                    data.drain(at..at + len);
                }
                Some((_, EchoMatch::Torn { at })) => {
                    self.held = data.split_off(at);
                    break;
                }
                _ => break,
            }
        }
        // Every byte that arrived counts against the wait, echo or not.
        for expected in &mut self.waiting {
            expected.budget = expected.budget.saturating_sub(chunk.len());
        }
        self.waiting.retain(|expected| expected.budget > 0);
        // Nothing left to wait for means nothing left to withhold: whatever is
        // held was ordinary output that merely began like an echo.
        if self.waiting.is_empty() && !self.held.is_empty() {
            let released = std::mem::take(&mut self.held);
            data.extend_from_slice(&released);
        }
        std::borrow::Cow::Owned(data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shape a cooked pty hands back, measured on a bare pty on this
    /// machine: `ESC [ 1 ; 1 R` in, `^[[1;1R` out.
    #[test]
    fn the_caret_form_is_what_a_cooked_pty_hands_back() {
        let shapes = echo_shapes(b"\x1b[1;1R");
        assert_eq!(shapes[0].needle, b"^[[1;1R");
        assert!(shapes[0].hold_partial);
    }

    /// And every control byte, not only the ESC. A colour report ended with
    /// BEL comes back with `^G` on the end — the original projects the ESC
    /// alone and walks past this one.
    #[test]
    fn a_report_ended_with_bel_projects_that_bel_too() {
        let shapes = echo_shapes(b"\x1b]11;rgb:00/00/00\x07");
        assert_eq!(shapes[0].needle, b"^[]11;rgb:00/00/00^G");
    }

    /// The line editor's rewrite is armed alongside, not instead: they are two
    /// sources and either may be the one that speaks.
    #[test]
    fn an_osc_answer_also_wears_the_line_editors_rewrite() {
        let shapes = echo_shapes(b"\x1b]11;rgb:00/00/00\x1b\\");
        let needles: Vec<&[u8]> = shapes.iter().map(|shape| shape.needle.as_slice()).collect();
        assert!(needles.contains(&b"\x0711;rgb:00/00/00".as_slice()));
        assert!(needles.contains(&b"^[]11;rgb:00/00/00^[\\".as_slice()));
    }

    /// The verbatim shape — what a `stty -echoctl` tty returns — begins with
    /// ESC, so it is never held half finished. Holding it would take a bare
    /// ESC away from a program's own sequence.
    #[test]
    fn the_verbatim_shape_is_never_held_half_finished() {
        let shapes = echo_shapes(b"\x1b[1;1R");
        let verbatim = shapes
            .iter()
            .find(|shape| shape.needle == b"\x1b[1;1R")
            .expect("the verbatim shape is armed");
        assert!(!verbatim.hold_partial);
    }

    #[test]
    fn a_read_nobody_is_watching_is_handed_back_untouched() {
        let mut echoes = ExpectedEchoes::default();
        assert!(matches!(
            echoes.sift(b"ordinary output"),
            std::borrow::Cow::Borrowed(b"ordinary output")
        ));
    }

    #[test]
    fn an_echo_is_taken_out_of_the_middle_of_a_read() {
        let mut echoes = ExpectedEchoes::default();
        echoes.expect(b"\x1b[1;1R");
        assert_eq!(echoes.sift(b"before^[[1;1Rafter").as_ref(), b"beforeafter");
    }

    /// Nothing bounds where the tty cuts, so an echo arrives in halves. The
    /// first half is withheld rather than drawn, and rejoined by the next read.
    #[test]
    fn an_echo_split_across_two_reads_is_still_taken() {
        let mut echoes = ExpectedEchoes::default();
        echoes.expect(b"\x1b[1;1R");
        assert_eq!(echoes.sift(b"hi^[[1;").as_ref(), b"hi");
        assert_eq!(echoes.sift(b"1Rthere").as_ref(), b"there");
    }

    /// And output that merely begins like an echo is held for one read and
    /// then handed over whole, in the order it arrived.
    #[test]
    fn output_that_only_begins_like_an_echo_is_released() {
        let mut echoes = ExpectedEchoes::default();
        echoes.expect(b"\x1b[1;1R");
        assert_eq!(echoes.sift(b"abc^[[").as_ref(), b"abc");
        assert_eq!(echoes.sift(b"xyz").as_ref(), b"^[[xyz");
    }

    /// An echo that never comes stops being waited for, and the bytes held
    /// against it are released rather than lost.
    #[test]
    fn an_echo_that_never_comes_stops_being_waited_for() {
        let mut echoes = ExpectedEchoes::default();
        echoes.expect(b"\x1b[1;1R");
        let flood = vec![b'x'; ECHO_SEARCH_BUDGET];
        assert_eq!(echoes.sift(&flood).len(), ECHO_SEARCH_BUDGET);
        assert!(echoes.is_quiet(), "the wait outlived its own budget");
        assert!(matches!(
            echoes.sift(b"^[[1;1R"),
            std::borrow::Cow::Borrowed(b"^[[1;1R")
        ));
    }

    /// A program can ask faster than the tty answers, and every unanswered
    /// shape is memory. Past the cap the oldest is forgotten.
    #[test]
    fn the_queue_of_unanswered_shapes_is_capped() {
        let mut echoes = ExpectedEchoes::default();
        for _ in 0..MAX_EXPECTED_ECHOES + 8 {
            echoes.expect(b"\x1b[1;1R");
        }
        assert_eq!(echoes.waiting.len(), MAX_EXPECTED_ECHOES);
    }

    /// Two answers in one turn are two shapes, and taking one leaves the other
    /// armed for the read it arrives in.
    #[test]
    fn two_answers_are_two_shapes() {
        let mut echoes = ExpectedEchoes::default();
        echoes.expect(b"\x1b[1;1R");
        echoes.expect(b"\x1b[0n");
        assert_eq!(echoes.sift(b"^[[1;1R").as_ref(), b"");
        assert_eq!(echoes.sift(b"a^[[0nb").as_ref(), b"ab");
        assert!(echoes.is_quiet());
    }
}

#[cfg(test)]
mod editor_shapes {
    use super::*;

    /// What `bash --norc -i` hands back, measured reply by reply.
    #[test]
    fn a_line_editor_rings_and_types_what_it_could_not_read() {
        for (reply, rung) in [
            (b"\x1b[1;1R".as_slice(), b"\x07;1R".as_slice()),
            (b"\x1b[?62;22c".as_slice(), b"\x0762;22c".as_slice()),
            (b"\x1b[?2026;2$y".as_slice(), b"\x072026;2$y".as_slice()),
        ] {
            let shapes = echo_shapes(reply);
            assert!(
                shapes.iter().any(|shape| shape.needle == rung),
                "the editor's shape for {reply:?} is missing: {shapes:?}"
            );
        }
    }

    /// And what is too short to be anyone else's is left alone.
    ///
    /// All that survives `ESC [ 0 n` is a bell and an `n`. Watching for that
    /// would cut two bytes out of any child that rang a bell before printing
    /// a word beginning with n — a trade this is not worth.
    #[test]
    fn a_remainder_too_short_to_be_nobodys_else_is_not_watched_for() {
        let shapes = echo_shapes(b"\x1b[0n");
        assert!(
            !shapes.iter().any(|shape| shape.needle == b"\x07n"),
            "a two byte needle was armed: {shapes:?}"
        );
    }
}
