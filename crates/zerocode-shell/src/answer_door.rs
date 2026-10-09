//! The one door an answer prepared from a card takes into a pane.
//!
//! A card is drawn from what a pane's agent said it asked, and answered some
//! moments later by typing into the pane. In between the question may have
//! been answered at the keyboard, resolved by the agent, or replaced by the
//! next one, and a key typed then lands in whatever the screen shows now — a
//! digit in somebody else's menu, an Escape that stops a running turn.
//!
//! So every road that types an answer a card prepared (`answer_ask`,
//! `answer_approval`) — and the one line the window types by itself that a
//! menu would take for an answer (the hand-over's exit command) — goes
//! through here, and here the screen is read again under the pane's own
//! terminal lock, in the same hold as the write: [`type_if_up`] writes only if
//! the screen shows what the caller needs. The pump needs that lock to move
//! output into the grid, so the screen read and the key are not separated by a
//! redraw this window parsed.
//!
//! One answer at a time per pane ([`answer_lease`]), on the lease a
//! `zerocode-ssh send` already takes — the same pane, the same "two writers
//! must not cross" rule.
//!
//! The reading itself is `zerocode_core::screen_menu`'s. This module owns the
//! terminal, its lock and its clock; core owns what a menu is.
//!
//! **What does not come through here, and why.** A person's own keys, paste
//! and wheel (`term_key`, `term_paste`, `term_scroll`): the person is looking
//! at the screen that takes them. A queued prompt (`PromptDelivery`): it has
//! its own guard on the composer line it writes to. An agent team's
//! `send-keys`, the restart nudge and the usage probes: nothing was prepared
//! from an earlier read of the screen, so there is nothing here to be late.

use super::*;
use zerocode_core::ask::{AskPrompt, AskQuestion, AskSelection};
use zerocode_core::screen_menu::{self, ScreenMenu};
use zerocode_core::secret_prompt::SecretPrompt;

use crate::terminal_registry::HeldTerminal;

/// The word a refused answer carries to the page when the question it was
/// prepared for is no longer the one on the screen. The page maps it to its
/// own sentence and shows the pane's question as it is now.
pub(crate) const QUESTION_CHANGED: &str = "question-changed";

/// The word a refused answer carries when another answer to the same pane is
/// still being typed or has not yet been shown to land.
pub(crate) const ANSWER_IN_FLIGHT: &str = "answer-in-flight";

/// The word a refused answer carries when a numbered menu was answered with
/// only words: a menu takes a row.
pub(crate) const MENU_NEEDS_A_ROW: &str = "menu-needs-a-row";

/// The word a refused secret carries when its value cannot be typed as one
/// line of visible text (`zerocode_core::secret_prompt::value_is_typable`).
pub(crate) const SECRET_INVALID: &str = "secret-invalid";

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

/// The visible rows of a pane's screen and where its cursor stands — what
/// `screen_menu` reads. Copied out so the lock is held for the copy and not
/// for the reading.
fn screen_of(pty: &PtyHandle) -> (Vec<String>, Option<usize>) {
    let grid = pty.terminal().grid();
    let rows = (0..grid.screen_rows()).map(|row| grid.line(row)).collect();
    (rows, Some(grid.cursor().0))
}

/// Whether the screen shows what `expect` says.
fn shows(pty: &PtyHandle, expect: &Expect<'_>) -> bool {
    let (rows, cursor) = screen_of(pty);
    if expect.absent {
        return screen_menu::read_menu(&rows, cursor).is_none();
    }
    let on_row = |menu: &ScreenMenu| expect.row.is_none_or(|row| menu.selected == row);
    match expect.question {
        // A question with no rows to compare: its words on the last rows.
        Some(question) if question.options.is_empty() => {
            screen_menu::words_are_up(&rows, &question.question)
        }
        Some(question) => screen_menu::read_menu(&rows, cursor)
            .is_some_and(|menu| menu.shows(question) && on_row(&menu)),
        None => screen_menu::read_menu(&rows, cursor).is_some_and(|menu| on_row(&menu)),
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
    let mut pty = lock_pty(held);
    if !shows(&pty, expect) {
        return Ok(false);
    }
    pty.write_input(bytes)?;
    Ok(true)
}

/// Type one key group of a walk, refusing if the question is gone.
fn press(
    held: &HeldTerminal,
    expect: &Expect<'_>,
    group: &zerocode_core::ask::KeyGroup,
    wake: &dyn Fn(),
) -> Result<(), Refusal> {
    let typed = type_if_up(held, expect, &crate::cmd::terminal::key_group_bytes(group))
        .map_err(|error| Refusal::Pty(error.to_string()))?;
    wake();
    if typed { Ok(()) } else { Err(Refusal::Changed) }
}

/// The one row a card's answer picked on a numbered menu, or `None` when the
/// answer is not one pick of one single-select question: words only, several
/// picks, a multi-select, nothing answered. A menu takes a row — not a
/// sentence and not a set.
pub(super) fn chosen_row(question: &AskQuestion, selections: &[AskSelection]) -> Option<usize> {
    match selections.first()?.indices.as_slice() {
        [row] if !question.multi_select => Some(*row),
        _ => None,
    }
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
    let (rows, cursor) = screen_of(&lock_pty(held));
    let from = screen_menu::read_menu(&rows, cursor)
        .filter(|menu| menu.shows(question) && to < menu.options.len())
        .map(|menu| menu.selected)
        .ok_or(Refusal::Changed)?;
    let on_question = Expect::question(question);
    let walk = zerocode_core::ask::walk_to_row(from, to);
    for group in &walk {
        press(held, &on_question, group, wake)?;
        std::thread::sleep(step);
    }
    // Nothing moved when the selection was already on the row, so there is
    // nothing for the screen to settle after.
    if !walk.is_empty() {
        std::thread::sleep(settle);
    }
    // The look: the selection has to be on the chosen row, and it is checked
    // in the same hold as the Enter.
    let on_the_row = Expect::question(question).on_row(to);
    for group in zerocode_core::ask::press_enter() {
        press(held, &on_the_row, &group, wake)?;
    }
    Ok(())
}

/// The card a waiting pane's screen is drawn as, when the screen shows a
/// numbered menu — for a pane whose agent said only that it waits.
///
/// Never blocks: this is asked from the window's own thread, and a pane whose
/// terminal is busy being parsed is simply not read this time; the next ask
/// finds it.
pub(super) fn screen_card(held: &HeldTerminal) -> Option<AskPrompt> {
    let (rows, cursor) = {
        let pty = match held.try_lock() {
            Ok(pty) => pty,
            Err(std::sync::TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => return None,
        };
        screen_of(&pty)
    };
    screen_menu::read_menu(&rows, cursor)?.card()
}

/// How long a pane's output must have been silent before its last line is
/// read as a question the pane waits on. A line still being printed is not
/// yet a question.
pub(super) const SECRET_QUIET: Duration = Duration::from_millis(300);

/// The secret the pane's question asks for, once the pane has been quiet for
/// [`SECRET_QUIET`]. `None` while the pane is still printing, and `None` for any
/// last line that is not such a question.
///
/// Never blocks: a pane whose terminal is being parsed this instant is simply
/// not read this time, as a numbered menu's card is not either.
pub(super) fn secret_prompt(held: &HeldTerminal) -> Option<SecretPrompt> {
    let pty = match held.try_lock() {
        Ok(pty) => pty,
        Err(std::sync::TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
        Err(std::sync::TryLockError::WouldBlock) => return None,
    };
    // A transport that does not say when it last wrote is read as quiet; the
    // reading itself still needs the question on the cursor's last row.
    let quiet = pty
        .last_output_at()
        .is_none_or(|at| at.elapsed() >= SECRET_QUIET);
    if !quiet {
        return None;
    }
    let (rows, cursor) = screen_of(&pty);
    zerocode_core::secret_prompt::read_secret_prompt(&rows, cursor?)
}

/// When the pane's output last moved, in epoch milliseconds — the key a
/// question is listed under, so the same question read again is the same
/// question. `0` when the transport does not say.
pub(super) fn quiet_since(held: &HeldTerminal) -> i64 {
    lock_pty(held).last_output_epoch_ms().unwrap_or(0)
}

/// When the pane's output last moved, as the watcher reads it, without waiting
/// for the pane's lock: `None` while the pane is being parsed this instant (the
/// next look reads it), and `Some(None)` for a transport that does not say.
pub(super) fn output_clock(held: &HeldTerminal) -> Option<Option<i64>> {
    let pty = match held.try_lock() {
        Ok(pty) => pty,
        Err(std::sync::TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
        Err(std::sync::TryLockError::WouldBlock) => return None,
    };
    Some(pty.last_output_epoch_ms())
}

/// Type a secret the person gave a card, and the return that sends it, into the
/// pane — if and only if the pane still shows `expect`'s question. The screen is
/// read and the value written in one hold of the pane's lock, as
/// [`type_if_up`] does. `Ok(false)` is a refusal: nothing was typed.
///
/// The value and its return go out as one write, in a buffer that is wiped when
/// this returns, so the return is never a separate key a changed screen could
/// take.
pub(super) fn type_secret_if_up(
    held: &HeldTerminal,
    expect: &SecretPrompt,
    value: &[u8],
) -> Result<bool, PtyTransportError> {
    let mut pty = lock_pty(held);
    let (rows, cursor) = screen_of(&pty);
    let shown = cursor.and_then(|row| zerocode_core::secret_prompt::read_secret_prompt(&rows, row));
    if shown.as_ref() != Some(expect) {
        return Ok(false);
    }
    let mut typed = zeroize::Zeroizing::new(Vec::with_capacity(value.len() + 1));
    typed.extend_from_slice(value);
    typed.push(b'\r');
    pty.write_input(&typed)?;
    Ok(true)
}

/// Take the right to type one answer into `term`, or say why not.
pub(super) fn answer_lease(term: TermId) -> Result<tokio::sync::OwnedMutexGuard<()>, Refusal> {
    crate::ssh_send_guard::send_lease(term)
        .try_lock_owned()
        .map_err(|_| Refusal::InFlight)
}

/// Keep the pane "being answered" for [`ANSWER_SETTLE`] after its last key,
/// then let go. Blocks; a caller already on a thread of its own calls this.
pub(super) fn settle(lease: tokio::sync::OwnedMutexGuard<()>) {
    std::thread::sleep(ANSWER_SETTLE);
    drop(lease);
}

/// [`settle`] for a caller that has nothing more to do and must not wait.
pub(super) fn settle_in_background(lease: tokio::sync::OwnedMutexGuard<()>) {
    std::thread::spawn(move || settle(lease));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;
    use zerocode_core::ask::AskOption;
    use zerocode_core::secret_prompt::SecretKind;
    use zerocode_pty::{Pumped, Terminal};

    /// What a fake pane's terminal says about who holds it, and whether it echoes.
    #[derive(Clone)]
    struct Holder {
        child: Option<u32>,
        holder: Option<u32>,
        names: Vec<String>,
        echo: Option<bool>,
    }

    impl Holder {
        /// A job the pane's shell started holds the terminal, and it does not echo.
        fn job() -> Self {
            Self {
                child: Some(100),
                holder: Some(200),
                names: vec!["ssh".to_string()],
                echo: Some(false),
            }
        }
    }

    /// A synthetic program that asks for a secret the way sudo does: it draws
    /// its question and waits, and it says when its output last moved.
    struct SecretCli {
        terminal: Terminal,
        last_output: Option<Instant>,
        written: Arc<Mutex<Vec<Vec<u8>>>>,
        holder: Holder,
    }

    impl PtyTransport for SecretCli {
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

        fn last_output_at(&self) -> Option<Instant> {
            self.last_output
        }

        fn pid(&self) -> Option<u32> {
            self.holder.child
        }

        fn foreground_process_id(&self) -> Option<u32> {
            self.holder.holder
        }

        fn foreground_programs(&self) -> Vec<String> {
            self.holder.names.clone()
        }

        fn echo_on(&self) -> Option<bool> {
            self.holder.echo
        }
    }

    struct SecretPane {
        held: HeldTerminal,
        written: Arc<Mutex<Vec<Vec<u8>>>>,
    }

    impl SecretPane {
        fn typed(&self) -> Vec<String> {
            self.written
                .lock()
                .unwrap()
                .iter()
                .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
                .collect()
        }
    }

    /// A pane whose screen shows `question` on its first row, with the cursor
    /// at its end, and whose output last moved `silent_for` ago. A job the
    /// shell started holds its terminal ([`Holder::job`]).
    fn secret_pane(question: &str, silent_for: Duration) -> SecretPane {
        secret_pane_with(question, silent_for, Holder::job())
    }

    fn secret_pane_with(question: &str, silent_for: Duration, holder: Holder) -> SecretPane {
        let written = Arc::new(Mutex::new(Vec::new()));
        let mut terminal = Terminal::new(24, 80);
        terminal.feed(format!("\x1b[2J\x1b[H{question}").as_bytes());
        let cli = SecretCli {
            terminal,
            last_output: Instant::now().checked_sub(silent_for),
            written: Arc::clone(&written),
            holder,
        };
        SecretPane {
            held: Arc::new(Mutex::new(PtyHandle::new(cli))),
            written,
        }
    }

    #[test]
    fn a_question_for_a_secret_is_read_only_once_its_pane_has_been_quiet() {
        let talking = secret_pane("[sudo] password for dev:", Duration::ZERO);
        assert_eq!(
            secret_prompt(&talking.held),
            None,
            "a line still being printed is not yet a question"
        );
        let waiting = secret_pane("[sudo] password for dev:", SECRET_QUIET * 2);
        assert_eq!(
            secret_prompt(&waiting.held),
            Some(SecretPrompt {
                kind: SecretKind::Password,
                line: "[sudo] password for dev:".to_string(),
            }),
        );
    }

    #[test]
    fn a_secret_is_typed_with_its_return_once_when_the_same_question_still_shows() {
        let pane = secret_pane("Password:", SECRET_QUIET * 2);
        let expect = SecretPrompt {
            kind: SecretKind::Password,
            line: "Password:".to_string(),
        };
        assert!(type_secret_if_up(&pane.held, &expect, b"SENTINEL-not-a-secret").unwrap());
        assert_eq!(pane.typed(), vec!["SENTINEL-not-a-secret\r".to_string()]);
    }

    #[test]
    #[ignore = "measurement: cargo test -p zerocode-shell --bin zerocode-shell answer_door::tests::measure_the_secret_door -- --ignored --nocapture"]
    fn measure_the_secret_door() {
        const ITERATIONS: u32 = 2_000;
        let pane = secret_pane("[sudo] password for dev:", SECRET_QUIET * 2);
        let started = Instant::now();
        let mut found = 0_u32;
        for _ in 0..ITERATIONS {
            found += u32::from(secret_prompt(&pane.held).is_some());
        }
        let per_look_us = started.elapsed().as_secs_f64() * 1e6 / f64::from(ITERATIONS);
        eprintln!(
            "MEASURE secret door: {per_look_us:.2} us per look at a quiet pane ({found} of {ITERATIONS} looks read the question)"
        );
    }

    #[test]
    fn a_secret_is_not_typed_into_a_pane_that_asks_something_else() {
        let pane = secret_pane("Enter passphrase:", SECRET_QUIET * 2);
        let expect = SecretPrompt {
            kind: SecretKind::Password,
            line: "Password:".to_string(),
        };
        assert!(matches!(
            type_secret_if_up(&pane.held, &expect, b"SENTINEL-not-a-secret"),
            Ok(false)
        ));
        assert!(pane.typed().is_empty());
    }

    #[test]
    fn a_secret_is_not_offered_while_the_pane_itself_holds_its_terminal() {
        let mut holder = Holder::job();
        holder.holder = holder.child;
        let pane = secret_pane_with("Password:", SECRET_QUIET * 2, holder);
        assert_eq!(
            secret_prompt(&pane.held),
            None,
            "a shell at its own prompt, or an agent with no job in front, asked for nothing a value answers"
        );
    }

    #[test]
    fn a_secret_is_not_offered_while_an_agent_is_the_job_in_front() {
        let mut holder = Holder::job();
        holder.names = vec!["claude".to_string(), "bin".to_string()];
        let pane = secret_pane_with("Password:", SECRET_QUIET * 2, holder);
        assert_eq!(
            secret_prompt(&pane.held),
            None,
            "a tool's prompt on an agent's terminal is read by the agent, not by the tool"
        );
    }

    #[test]
    fn a_secret_is_not_typed_into_a_question_that_echoes() {
        let mut holder = Holder::job();
        holder.echo = Some(true);
        let pane = secret_pane_with("Password:", SECRET_QUIET * 2, holder);
        let expect = SecretPrompt {
            kind: SecretKind::Password,
            line: "Password:".to_string(),
        };
        assert!(matches!(
            type_secret_if_up(&pane.held, &expect, b"SENTINEL-not-a-secret"),
            Ok(false)
        ));
        assert!(
            pane.typed().is_empty(),
            "an echoing terminal keeps the typed value on the screen and in scrollback"
        );
    }

    #[test]
    fn a_secret_is_not_typed_when_the_terminal_cannot_say_who_holds_it() {
        let mut holder = Holder::job();
        holder.holder = None;
        let pane = secret_pane_with("Password:", SECRET_QUIET * 2, holder);
        let expect = SecretPrompt {
            kind: SecretKind::Password,
            line: "Password:".to_string(),
        };
        assert!(matches!(
            type_secret_if_up(&pane.held, &expect, b"SENTINEL-not-a-secret"),
            Ok(false)
        ));
        assert!(pane.typed().is_empty());
    }

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

    /// MEASUREMENT (ignored): what the door costs a pane and what it keeps. The
    /// reading is paid when a pane that waits is listed for the board and when
    /// an answer is about to be typed — never per output chunk — so this is the
    /// whole bill: the card's screen read (lock, copy, reading), the typed
    /// answer's look-and-write, and the memory after a long run of both.
    /// Run on purpose, normally and under `taskpolicy -b`:
    /// `cargo test -p zerocode-shell --bin zerocode-shell answer_door::tests::measure -- --ignored --nocapture`.
    #[test]
    #[ignore = "a measurement, run on purpose: -- --ignored --nocapture"]
    fn measure_the_door_and_what_it_keeps() {
        use std::time::Instant;
        const ITERATIONS: u32 = 50_000;
        const LEASE_CYCLES: u32 = 20_000;
        // The loops run in this many equal parts and the resident size is read
        // after each, so a plateau and a leak look different.
        const PARTS: u32 = 5;
        let rss_kib = || -> u64 {
            let out = crate::proc::quiet_command("ps")
                .args(["-o", "rss=", "-p", &std::process::id().to_string()])
                .output()
                .expect("ps");
            String::from_utf8_lossy(&out.stdout)
                .trim()
                .parse()
                .unwrap_or(0)
        };
        let written = Arc::new(Mutex::new(Vec::new()));
        let mut cli = MenuCli {
            terminal: Terminal::new(40, 120),
            title: "Select an approach".to_string(),
            options: vec!["Rebase".into(), "Merge".into(), "Cancel".into()],
            selected: 0,
            follows_arrows: true,
            written: Arc::clone(&written),
            chosen: Arc::new(Mutex::new(None)),
        };
        cli.redraw();
        let held: HeldTerminal = Arc::new(Mutex::new(PtyHandle::new(cli)));
        let question = asked("Select an approach", &["Rebase", "Merge", "Cancel"]);
        let before = rss_kib();

        let mut cards = 0_u32;
        let mut card_time = Duration::ZERO;
        let mut card_rss = Vec::new();
        for _ in 0..PARTS {
            let began = Instant::now();
            for _ in 0..ITERATIONS / PARTS {
                cards += u32::from(screen_card(&held).is_some());
            }
            card_time += began.elapsed();
            card_rss.push(rss_kib());
        }
        let card_nanos = card_time.as_nanos() as f64 / f64::from(ITERATIONS);

        let mut typed = 0_u32;
        let mut type_time = Duration::ZERO;
        let mut type_rss = Vec::new();
        for _ in 0..PARTS {
            let began = Instant::now();
            for round in 0..ITERATIONS / PARTS {
                typed += u32::from(
                    type_if_up(&held, &Expect::question(&question), b"x").unwrap_or(false),
                );
                if round.is_multiple_of(1_000) {
                    written.lock().unwrap().clear();
                }
            }
            type_time += began.elapsed();
            type_rss.push(rss_kib());
        }
        let type_nanos = type_time.as_nanos() as f64 / f64::from(ITERATIONS);

        let mut lease_rss = Vec::new();
        for part in 0..PARTS {
            for term in 0..LEASE_CYCLES / PARTS {
                let pane = 920_000 + part * (LEASE_CYCLES / PARTS) + term;
                drop(answer_lease(pane).expect("a free pane"));
            }
            lease_rss.push(rss_kib());
        }
        println!(
            "MEASURE door: screen_card {:.1} us/call ({cards} cards of {ITERATIONS}), type_if_up {:.1} us/call ({typed} typed of {ITERATIONS}) — look + write under one hold of the pane's lock",
            card_nanos / 1_000.0,
            type_nanos / 1_000.0
        );
        println!(
            "MEASURE memory: rss {before} KiB at the start; after each fifth of {ITERATIONS} card reads {card_rss:?}; of {ITERATIONS} typed answers {type_rss:?}; of {LEASE_CYCLES} lease cycles on distinct panes {lease_rss:?} (a plateau is no growth)"
        );
    }
}
