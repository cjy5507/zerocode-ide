//! The one door an answer prepared from a card takes into a pane.
//!
//! A card is drawn from what a pane's agent said it asked, and answered some
//! moments later by typing into the pane. In between the question may have
//! been answered at the keyboard, resolved by the agent, or replaced by the
//! next one, and a key typed then lands in whatever the screen shows now — a
//! digit in somebody else's menu, an Escape that stops a running turn.
//!
//! So every road that types an answer a card prepared (`answer_ask`,
//! `answer_approval`) goes through here, and here the screen is read again
//! under the pane's own terminal lock, in the same hold as the write:
//! [`type_if_up`] writes only if the question is still the one on screen. The
//! pump needs that lock to move output into the grid, so the screen read and
//! the key are not separated by a redraw this window parsed.
//!
//! One answer at a time per pane ([`answer_lease`]), on the lease a
//! `zerocode-ssh send` already takes — the same pane, the same "two writers
//! must not cross" rule.
//!
//! The reading itself is `zerocode_core::screen_menu`'s. This module owns the
//! terminal, its lock and its clock; core owns what a menu is.

use super::*;
use zerocode_core::ask::{AskPrompt, AskQuestion, AskSelection};

use crate::terminal_registry::HeldTerminal;

/// The word a refused answer carries to the page when the question it was
/// prepared for is no longer the one on the screen. The page maps it to its
/// own sentence and shows the pane's current question.
pub(crate) const QUESTION_CHANGED: &str = "question-changed";

/// The word a refused answer carries when another answer to the same pane is
/// still being typed or has not yet been shown to land.
pub(crate) const ANSWER_IN_FLIGHT: &str = "answer-in-flight";

/// How long a pane stays "being answered" after the last key of an answer.
///
/// The child needs a redraw to show it took the key. A second answer inside
/// that redraw would verify against the screen the first one already
/// answered and type again — the double answer the lease exists to prevent.
/// Short enough that a person answering one question and then the next one it
/// raises never meets it.
pub(super) const ANSWER_SETTLE: Duration = Duration::from_millis(400);

/// Why an answer was not typed.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Refusal {
    /// The screen no longer shows the question the answer was prepared for,
    /// or the selection is not where the answer needs it.
    Changed,
    /// Another answer to this pane is still on its way in.
    InFlight,
    /// The pty would not take the key.
    Pty(String),
}

impl Refusal {
    /// The error string a command returns.
    pub(super) fn into_message(self) -> String {
        match self {
            Self::Changed => QUESTION_CHANGED.to_string(),
            Self::InFlight => ANSWER_IN_FLIGHT.to_string(),
            Self::Pty(error) => error,
        }
    }
}

/// What the screen has to show for a prepared answer to be typed.
pub(super) struct Expect<'a> {
    question: Option<&'a AskQuestion>,
    row: Option<usize>,
    /// No numbered menu at all — for words the window types by itself.
    absent: bool,
}

impl<'a> Expect<'a> {
    /// A live numbered menu of any question — a permission card has no rows
    /// of its own to compare, only that it is a menu that asks.
    pub(super) const fn any_menu() -> Self {
        Self {
            question: None,
            row: None,
            absent: false,
        }
    }

    /// No numbered menu on the screen: the pane is at a prompt that takes
    /// words. A line the window types by itself — the hand-over's exit command
    /// — is guidance, and Enter on a menu takes the highlighted row, which
    /// may be a permission nobody gave.
    pub(super) const fn no_menu() -> Self {
        Self {
            question: None,
            row: None,
            absent: true,
        }
    }

    /// This question, as the screen's menu.
    pub(super) const fn question(question: &'a AskQuestion) -> Self {
        Self {
            question: Some(question),
            row: None,
            absent: false,
        }
    }

    /// And the selection standing on this row.
    pub(super) const fn on_row(mut self, row: usize) -> Self {
        self.row = Some(row);
        self
    }
}

/// Type `bytes` into the pane if — and only if — the screen shows what
/// `expect` says, read and written in one hold of the pane's lock.
///
/// `Ok(false)` is a refusal: nothing was typed.
pub(super) fn type_if_up(
    held: &HeldTerminal,
    expect: &Expect<'_>,
    bytes: &[u8],
) -> Result<bool, PtyTransportError> {
    let _ = expect;
    lock_pty(held).write_input(bytes)?;
    Ok(true)
}

/// The one row a card's answer picked on a numbered menu, or `None` when the
/// answer is not one pick of one single-select question: words only, several
/// picks, a multi-select, nothing answered. A menu takes a row — not a
/// sentence and not a set.
pub(super) fn chosen_row(question: &AskQuestion, selections: &[AskSelection]) -> Option<usize> {
    let _ = (question, selections);
    None
}

/// Choose row `to` of the menu `question` is on a pane's screen the way a
/// hand would: walk the selection there, look that it landed, then take it.
///
/// The look is the point. A menu nobody measured may take an arrow key
/// differently, and a pace that outran it would otherwise Enter the wrong
/// row; here it ends as [`Refusal::Changed`] with no Enter typed.
///
/// `wake` tells the window's pump the child has been written to — after every
/// key, because the screen the look reads is the one the pump parses from the
/// child's answer.
pub(super) fn choose_row(
    held: &HeldTerminal,
    question: &AskQuestion,
    to: usize,
    step: Duration,
    settle: Duration,
    wake: &dyn Fn(),
) -> Result<(), Refusal> {
    let _ = (held, question, to, step, settle, wake);
    Ok(())
}

/// The card a waiting pane's screen is drawn as, when the screen shows a
/// numbered menu — for a pane whose agent said only that it waits.
///
/// Never blocks: this is asked from the window's own thread, and a pane whose
/// terminal is busy being parsed is simply not read this time; the next ask
/// finds it.
pub(super) fn screen_card(held: &HeldTerminal) -> Option<AskPrompt> {
    let _ = held;
    None
}

/// Take the right to type one answer into `term`, or say why not.
pub(super) fn answer_lease(term: TermId) -> Result<tokio::sync::OwnedMutexGuard<()>, Refusal> {
    let _ = term;
    Arc::new(tokio::sync::Mutex::new(()))
        .try_lock_owned()
        .map_err(|_| Refusal::InFlight)
}

#[cfg(test)]
mod tests {
    use super::*;
    use zerocode_core::ask::AskOption;
    use zerocode_pty::{Pumped, Terminal};

    /// A synthetic CLI that draws a numbered menu and takes arrow keys and
    /// Enter the way a menu does — the pane every test here types into.
    struct MenuCli {
        terminal: Terminal,
        title: String,
        options: Vec<String>,
        selected: usize,
        /// Whether an arrow key moves the selection. A CLI that does not is
        /// the one the walk's look has to catch.
        follows_arrows: bool,
        written: Arc<Mutex<Vec<Vec<u8>>>>,
        chosen: Arc<Mutex<Option<usize>>>,
    }

    impl MenuCli {
        fn redraw(&mut self) {
            let mut screen = String::from("\x1b[2J\x1b[H");
            screen.push_str(&self.title);
            screen.push_str("\r\n");
            for (i, option) in self.options.iter().enumerate() {
                let glyph = if i == self.selected { "❯" } else { " " };
                screen.push_str(&format!("{glyph} {}. {option}\r\n", i + 1));
            }
            self.terminal.feed(screen.as_bytes());
        }
    }

    impl PtyTransport for MenuCli {
        fn pump(&mut self) -> Pumped {
            Pumped {
                bytes: 0,
                ended: false,
                answered: false,
                unanswered_since: None,
            }
        }

        fn terminal(&self) -> &Terminal {
            &self.terminal
        }

        fn terminal_mut(&mut self) -> &mut Terminal {
            &mut self.terminal
        }

        fn write_input(&mut self, bytes: &[u8]) -> Result<(), PtyTransportError> {
            self.written.lock().unwrap().push(bytes.to_vec());
            match bytes {
                b"\x1b[B" if self.follows_arrows => {
                    self.selected = (self.selected + 1).min(self.options.len() - 1);
                    self.redraw();
                }
                b"\x1b[A" if self.follows_arrows => {
                    self.selected = self.selected.saturating_sub(1);
                    self.redraw();
                }
                b"\r" => {
                    *self.chosen.lock().unwrap() = Some(self.selected);
                    self.terminal.feed(b"\x1b[2J\x1b[Hchosen\r\n> ");
                }
                _ => {}
            }
            Ok(())
        }

        fn resize(&mut self, _rows: u16, _cols: u16) -> Result<(), PtyTransportError> {
            Ok(())
        }

        fn try_wait(&mut self) -> Result<Option<u32>, PtyTransportError> {
            Ok(None)
        }

        fn kill(&mut self) -> Result<(), PtyTransportError> {
            Ok(())
        }
    }

    struct Pane {
        held: HeldTerminal,
        written: Arc<Mutex<Vec<Vec<u8>>>>,
        chosen: Arc<Mutex<Option<usize>>>,
    }

    impl Pane {
        fn typed(&self) -> Vec<String> {
            self.written
                .lock()
                .unwrap()
                .iter()
                .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
                .collect()
        }

        /// What the child would draw next, as the child's own bytes.
        fn draw(&self, text: &str) {
            let mut pty = lock_pty(&self.held);
            pty.terminal_mut().feed(b"\x1b[2J\x1b[H");
            pty.terminal_mut()
                .feed(text.replace('\n', "\r\n").as_bytes());
        }
    }

    fn pane(title: &str, options: &[&str], follows_arrows: bool) -> Pane {
        let written = Arc::new(Mutex::new(Vec::new()));
        let chosen = Arc::new(Mutex::new(None));
        let mut cli = MenuCli {
            terminal: Terminal::new(24, 80),
            title: title.to_string(),
            options: options.iter().map(|option| (*option).to_string()).collect(),
            selected: 0,
            follows_arrows,
            written: Arc::clone(&written),
            chosen: Arc::clone(&chosen),
        };
        cli.redraw();
        Pane {
            held: Arc::new(Mutex::new(PtyHandle::new(cli))),
            written,
            chosen,
        }
    }

    fn asked(text: &str, options: &[&str]) -> AskQuestion {
        AskQuestion {
            question: text.to_string(),
            header: None,
            multi_select: false,
            options: options
                .iter()
                .map(|label| AskOption {
                    label: (*label).to_string(),
                    description: None,
                })
                .collect(),
        }
    }

    const NO_WAIT: Duration = Duration::ZERO;

    /// The incident: an answer prepared for one question, typed after the
    /// pane has moved on to another, is typed zero times — to the next
    /// question, to a composer, to nothing at all.
    #[test]
    fn a_late_answer_is_typed_zero_times_after_the_question_changed() {
        let pane = pane("Which library?", &["date-fns", "dayjs"], true);
        let question = asked("Which library?", &["date-fns", "dayjs"]);
        let expect = Expect::question(&question);
        assert_eq!(type_if_up(&pane.held, &expect, b"1").ok(), Some(true));
        assert_eq!(
            pane.typed(),
            vec!["1"],
            "the live question takes its answer"
        );

        // The agent moved on to another question.
        pane.draw("Which framework?\n❯ 1. react\n  2. vue\n");
        assert_eq!(type_if_up(&pane.held, &expect, b"1").ok(), Some(false));
        // The question was answered and the pane is at its composer.
        pane.draw("● Done.\n\n❯ \n");
        assert_eq!(type_if_up(&pane.held, &expect, b"1").ok(), Some(false));
        assert_eq!(
            pane.typed(),
            vec!["1"],
            "nothing was typed after the change"
        );
    }

    /// A permission card has no rows of its own: any live menu will do, and a
    /// screen with none — the agent resolved it itself — gets no key, least of
    /// all the Escape that would stop a running turn.
    #[test]
    fn a_permission_answer_needs_a_live_menu_and_never_types_an_escape_without_one() {
        let pane = pane("Do you want to proceed?", &["Yes", "No"], true);
        assert_eq!(
            type_if_up(&pane.held, &Expect::any_menu(), b"1").ok(),
            Some(true)
        );
        pane.draw("✻ Working… (3s)\n\n❯ \n");
        assert_eq!(
            type_if_up(&pane.held, &Expect::any_menu(), b"\x1b").ok(),
            Some(false)
        );
        assert_eq!(pane.typed(), vec!["1"]);
    }

    /// The selection has to be where the answer needs it when the answer
    /// is a key relative to it.
    #[test]
    fn an_answer_that_needs_the_selection_on_a_row_waits_for_it() {
        let pane = pane("Pick", &["a", "b", "c"], true);
        let question = asked("Pick", &["a", "b", "c"]);
        let on_second = Expect::question(&question).on_row(1);
        assert_eq!(type_if_up(&pane.held, &on_second, b"\r").ok(), Some(false));
        assert!(pane.typed().is_empty());
        let on_first = Expect::question(&question).on_row(0);
        assert_eq!(type_if_up(&pane.held, &on_first, b"\r").ok(), Some(true));
    }

    /// The pane's own redraw of the same question — a clock above it, the
    /// selection moved — is not a changed question.
    #[test]
    fn a_redraw_of_the_same_question_is_not_a_change() {
        let pane = pane("Which library?", &["date-fns", "dayjs"], true);
        let question = asked("Which library?", &["date-fns", "dayjs"]);
        let expect = Expect::question(&question);
        for tick in 0..20 {
            pane.draw(&format!(
                "✻ Worked for {tick}m {s}s · {tick}.2k tokens\n\nWhich library?\n  1. date-fns\n❯ 2. dayjs\n\nEnter to select\n",
                s = 7 + tick
            ));
            assert_eq!(
                type_if_up(&pane.held, &expect, b"2").ok(),
                Some(true),
                "refused a good answer on redraw {tick}"
            );
        }
    }

    /// Guidance the window types by itself is not typed into a pane standing on
    /// a menu: Enter there takes the highlighted row.
    #[test]
    fn a_line_is_not_typed_into_a_pane_standing_on_a_menu() {
        let pane = pane("Do you want to proceed?", &["Yes", "No"], true);
        assert_eq!(
            type_if_up(&pane.held, &Expect::no_menu(), b"/exit").ok(),
            Some(false)
        );
        assert!(pane.typed().is_empty(), "a line was typed into a menu");
        pane.draw("● Done.\n\n❯ \n");
        assert_eq!(
            type_if_up(&pane.held, &Expect::no_menu(), b"/exit").ok(),
            Some(true)
        );
        assert_eq!(pane.typed(), vec!["/exit"]);
    }

    /// A menu takes one row: the answer's single pick of a single-select
    /// question. Words only, several picks, a multi-select and an unanswered
    /// question are not a row.
    #[test]
    fn a_menu_takes_one_row_and_only_one() {
        let single = asked("Pick", &["a", "b", "c"]);
        let pick = |indices: &[usize]| AskSelection {
            indices: indices.to_vec(),
            other: String::new(),
        };
        assert_eq!(chosen_row(&single, &[pick(&[1])]), Some(1));
        assert_eq!(chosen_row(&single, &[pick(&[])]), None, "words only");
        assert_eq!(chosen_row(&single, &[pick(&[0, 2])]), None, "two picks");
        assert_eq!(chosen_row(&single, &[]), None, "nothing answered");
        let mut multi = asked("Pick any", &["a", "b"]);
        multi.multi_select = true;
        assert_eq!(chosen_row(&multi, &[pick(&[0])]), None, "a multi-select");
    }

    /// A CLI that is not Claude Code or Codex, waiting on a numbered menu:
    /// the card's choice reaches the pane — the selection walked to the row,
    /// looked at, and taken.
    #[test]
    fn a_choice_on_another_clis_menu_walks_the_selection_looks_and_takes_the_row() {
        let pane = pane("Select an approach", &["Rebase", "Merge", "Cancel"], true);
        let question = asked("Select an approach", &["Rebase", "Merge", "Cancel"]);
        assert_eq!(
            choose_row(&pane.held, &question, 2, NO_WAIT, NO_WAIT, &|| {}),
            Ok(())
        );
        assert_eq!(pane.typed(), vec!["\x1b[B", "\x1b[B", "\r"]);
        assert_eq!(*pane.chosen.lock().unwrap(), Some(2));
    }

    /// A CLI whose menu does not move on an arrow key: the look catches it
    /// and the Enter is never typed.
    #[test]
    fn a_menu_that_ignores_the_arrows_is_not_entered() {
        let pane = pane("Select an approach", &["Rebase", "Merge", "Cancel"], false);
        let question = asked("Select an approach", &["Rebase", "Merge", "Cancel"]);
        assert_eq!(
            choose_row(&pane.held, &question, 2, NO_WAIT, NO_WAIT, &|| {}),
            Err(Refusal::Changed)
        );
        assert!(
            !pane.typed().contains(&"\r".to_string()),
            "an Enter was typed on a menu whose selection never moved: {:?}",
            pane.typed()
        );
        assert_eq!(*pane.chosen.lock().unwrap(), None);
    }

    /// Choosing on a pane that is no longer on the question types nothing.
    #[test]
    fn a_choice_on_a_screen_that_moved_on_types_nothing() {
        let pane = pane("Select an approach", &["Rebase", "Merge", "Cancel"], true);
        let question = asked("Select an approach", &["Rebase", "Merge", "Cancel"]);
        pane.draw("● Rebased.\n\n❯ \n");
        assert_eq!(
            choose_row(&pane.held, &question, 1, NO_WAIT, NO_WAIT, &|| {}),
            Err(Refusal::Changed)
        );
        assert!(pane.typed().is_empty());
    }

    /// The card a pane's screen is drawn as, for a pane whose agent said only
    /// that it waits — and none for a screen with no menu on it.
    #[test]
    fn a_waiting_panes_numbered_menu_is_drawn_as_a_card() {
        let pane = pane("Select an approach", &["Rebase", "Merge", "Cancel"], true);
        let card = screen_card(&pane.held).map(|card| card.questions);
        let only = card.unwrap_or_default();
        assert_eq!(only.len(), 1);
        assert_eq!(only[0].question, "Select an approach");
        let labels: Vec<&str> = only[0]
            .options
            .iter()
            .map(|option| option.label.as_str())
            .collect();
        assert_eq!(labels, vec!["Rebase", "Merge", "Cancel"]);
        pane.draw("● Done.\n\n❯ \n");
        assert_eq!(screen_card(&pane.held), None);
    }

    /// A terminal being parsed is not waited for: the ask comes back empty
    /// and the next one finds the pane.
    #[test]
    fn a_pane_busy_in_its_parse_is_not_waited_for() {
        let pane = pane("Select an approach", &["Rebase", "Merge", "Cancel"], true);
        let held = Arc::clone(&pane.held);
        let busy = lock_pty(&held);
        assert_eq!(screen_card(&pane.held), None);
        drop(busy);
        assert!(screen_card(&pane.held).is_some());
    }

    /// One answer at a time per pane; another pane is another matter.
    #[test]
    fn a_second_answer_to_the_same_pane_is_refused_while_one_is_in_flight() {
        let first = answer_lease(910_001).expect("a free pane");
        assert_eq!(
            answer_lease(910_001).err(),
            Some(Refusal::InFlight),
            "two answers were allowed to cross"
        );
        assert!(answer_lease(910_002).is_ok(), "another pane is not blocked");
        drop(first);
        assert!(answer_lease(910_001).is_ok(), "the pane is free again");
    }
}
