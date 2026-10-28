//! Every pane's terminal history, held to one budget for the window (t-80604).
//!
//! The scrollback a person chooses is a depth for each pane, and the window
//! held it for every pane at once: at the 50,000-line ceiling a pane that has
//! filled its history keeps about 33 MiB of packed rows (5,000 lines measured
//! 3.3 MiB, t-54317), so ten agent panes left open for a working day were
//! about 330 MiB of the window's own memory — most of it the history of panes
//! nobody had looked at for hours.
//!
//! The budget is the person's depth for one pane and the grid's own default
//! for each of the others, summed: `chosen + DEFAULT_SCROLLBACK_LINES ×
//! (panes − 1)`. Under it nothing is touched, which is every window whose
//! setting is at or under the default. Over it, the panes quiet longest give
//! up their oldest lines first, and none below the default — so the pane in
//! use keeps the depth that was chosen, and a pane left alone still holds
//! what the default would have kept. A pane somebody is reading back, and
//! then a pane a window shows, gives up lines only once every other pane is
//! down to the default. The cap itself is never lowered: a pane that starts
//! printing again grows back toward it.
use super::*;
use std::collections::HashMap;
use std::time::Instant;

/// One pane as the budget reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PaneHistory {
    pub(crate) term: TermId,
    /// History lines it holds now.
    pub(crate) lines: usize,
    /// The depth the person chose, as the pane's grid holds it.
    pub(crate) chosen: usize,
    /// Scrolled back: somebody is reading this history right now.
    pub(crate) reading: bool,
    /// A window shows it (`readers_by_term`).
    pub(crate) watched: bool,
    /// When it last printed or was read back; `None` when not since the
    /// window has been counting.
    pub(crate) moved: Option<Instant>,
}

/// How many lines each pane over its share keeps, quietest pane first.
/// Empty when the window is within its budget.
pub(crate) fn shares(panes: &mut [PaneHistory]) -> Vec<(TermId, usize)> {
    let Some(chosen) = panes.iter().map(|pane| pane.chosen).max() else {
        return Vec::new();
    };
    let floor = zerocode_pty::DEFAULT_SCROLLBACK_LINES.min(chosen);
    let budget = chosen.saturating_add(floor.saturating_mul(panes.len() - 1));
    let mut total = panes
        .iter()
        .fold(0_usize, |sum, pane| sum.saturating_add(pane.lines));
    if total <= budget {
        return Vec::new();
    }
    // A pane being read goes last and a pane on screen next to last, whatever
    // their clocks say: a clock alone ties a reader with a pane that printed
    // in the same round, and a tie fell to the pool's hash order. Among the
    // rest `None` orders first — a pane that has not moved since the window
    // began counting is the quietest there is.
    panes.sort_by_key(|pane| (pane.reading, pane.watched, pane.moved));
    let mut kept = Vec::new();
    for pane in panes.iter() {
        if total <= budget {
            break;
        }
        let give = pane.lines.saturating_sub(floor).min(total - budget);
        if give > 0 {
            kept.push((pane.term, pane.lines - give));
            total -= give;
        }
    }
    kept
}

/// Read every pane, and shed where the window is over its budget. Each
/// pane's own lock, one at a time and nothing else under it — the pool's
/// lock order (`terminal_registry`). `moved` is the pump's record of when
/// each pane last printed; a pane read back is written into it at `now`, and
/// panes that left the pool leave it here. `entries` is the caller's buffer,
/// handed back empty.
pub(crate) fn hold(
    terminals: &TerminalRegistry,
    entries: &mut Vec<(TermId, terminal_registry::HeldTerminal)>,
    watched: &HashMap<TermId, Vec<String>>,
    moved: &mut HashMap<TermId, Instant>,
    now: Instant,
) {
    terminals.entries_into(entries);
    moved.retain(|term, _| entries.iter().any(|(held, _)| held == term));
    let mut panes: Vec<PaneHistory> = entries
        .iter()
        .map(|(term, held)| {
            let pty = lock_pty(held);
            let grid = pty.terminal().grid();
            let reading = grid.view_offset() > 0;
            if reading {
                moved.insert(*term, now);
            }
            PaneHistory {
                term: *term,
                lines: grid.scrollback_len(),
                chosen: grid.scrollback_cap(),
                reading,
                watched: watched.contains_key(term),
                moved: moved.get(term).copied(),
            }
        })
        .collect();
    for (term, keep) in shares(&mut panes) {
        if let Some((_, held)) = entries.iter().find(|(held, _)| *held == term) {
            lock_pty(held).terminal_mut().grid_mut().shed_history(keep);
        }
    }
    entries.clear();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use zerocode_lane::{PtyHandle, PtyTransport, PtyTransportError};
    use zerocode_pty::{DEFAULT_SCROLLBACK_LINES, MAX_SCROLLBACK_LINES, Pumped, Terminal};

    fn pane(term: TermId, lines: usize, chosen: usize, moved: Option<Instant>) -> PaneHistory {
        PaneHistory {
            term,
            lines,
            chosen,
            reading: false,
            watched: false,
            moved,
        }
    }

    /// A pane with a screen and a history and no child behind it.
    struct Screen(Terminal);

    impl PtyTransport for Screen {
        fn pump(&mut self) -> Pumped {
            Pumped {
                bytes: 0,
                ended: false,
                answered: false,
                unanswered_since: None,
            }
        }
        fn terminal(&self) -> &Terminal {
            &self.0
        }
        fn terminal_mut(&mut self) -> &mut Terminal {
            &mut self.0
        }
        fn write_input(&mut self, _: &[u8]) -> Result<(), PtyTransportError> {
            Ok(())
        }
        fn resize(&mut self, _: u16, _: u16) -> Result<(), PtyTransportError> {
            Ok(())
        }
        fn try_wait(&mut self) -> Result<Option<u32>, PtyTransportError> {
            Ok(None)
        }
        fn kill(&mut self) -> Result<(), PtyTransportError> {
            Ok(())
        }
    }

    /// Into the pool: a pane at the depth `chosen` holding `lines` of
    /// history, scrolled back `reading` lines.
    fn seat(pool: &TerminalRegistry, term: TermId, chosen: usize, lines: usize, reading: usize) {
        let mut screen = Terminal::new(4, 20);
        screen.grid_mut().set_scrollback_cap(chosen);
        let mut text = String::new();
        for n in 0..lines + 4 {
            text.push_str(&format!("{term}:{n}\r\n"));
        }
        screen.feed(text.as_bytes());
        screen
            .grid_mut()
            .scroll_view(isize::try_from(reading).expect("a small offset"));
        pool.insert(term, PtyHandle::new(Screen(screen)));
    }

    /// `(history lines, view offset)` of one pane in the pool.
    fn held(pool: &TerminalRegistry, term: TermId) -> (usize, usize) {
        let held = pool.handle(term).expect("the pane is in the pool");
        let pty = lock_pty(&held);
        let grid = pty.terminal().grid();
        (grid.scrollback_len(), grid.view_offset())
    }

    #[test]
    fn a_window_at_the_default_depth_is_never_touched() {
        let now = Instant::now();
        let mut panes: Vec<PaneHistory> = (0..10)
            .map(|term| {
                pane(
                    term,
                    DEFAULT_SCROLLBACK_LINES,
                    DEFAULT_SCROLLBACK_LINES,
                    None,
                )
            })
            .collect();
        assert!(shares(&mut panes).is_empty());
        let mut alone = [pane(
            1,
            MAX_SCROLLBACK_LINES,
            MAX_SCROLLBACK_LINES,
            Some(now),
        )];
        assert!(
            shares(&mut alone).is_empty(),
            "one pane keeps the whole depth chosen"
        );
    }

    #[test]
    fn the_quietest_panes_give_up_lines_first_and_never_below_the_default() {
        let now = Instant::now();
        let ago = |secs| now.checked_sub(Duration::from_secs(secs));
        let deep = MAX_SCROLLBACK_LINES;
        let mut panes = vec![
            pane(1, deep, deep, ago(10)),
            pane(2, deep, deep, None),
            pane(3, deep, deep, ago(3_600)),
            pane(4, 200, deep, ago(7_200)),
        ];
        let kept = shares(&mut panes);
        // Budget: 50,000 + 3 × 5,000 = 65,000 of the 150,200 held. Pane 2
        // (never moved) goes first, to the default; pane 4 is under the
        // default and gives nothing; pane 3 (an hour quiet) gives the 40,200
        // still over; pane 1, the one in use, keeps the whole depth.
        assert_eq!(
            kept,
            vec![(2, DEFAULT_SCROLLBACK_LINES), (3, deep - 40_200)]
        );
        let total: usize = panes
            .iter()
            .map(|pane| {
                kept.iter()
                    .find(|(term, _)| *term == pane.term)
                    .map_or(pane.lines, |(_, keep)| *keep)
            })
            .sum();
        assert_eq!(total, 65_000);
    }

    #[test]
    fn a_small_overrun_comes_from_the_quietest_pane_alone() {
        let now = Instant::now();
        let deep = MAX_SCROLLBACK_LINES;
        let mut panes = vec![
            pane(1, deep, deep, Some(now)),
            pane(
                2,
                DEFAULT_SCROLLBACK_LINES + 300,
                deep,
                now.checked_sub(Duration::from_secs(60)),
            ),
        ];
        assert_eq!(shares(&mut panes), vec![(2, DEFAULT_SCROLLBACK_LINES)]);
    }

    #[test]
    fn a_pane_being_read_and_then_a_pane_on_screen_give_up_lines_last() {
        let now = Instant::now();
        let deep = MAX_SCROLLBACK_LINES;
        let ago = now.checked_sub(Duration::from_secs(3_600));
        let mut panes = vec![
            PaneHistory {
                reading: true,
                ..pane(1, deep, deep, Some(now))
            },
            PaneHistory {
                watched: true,
                ..pane(2, deep, deep, ago)
            },
            pane(3, deep, deep, Some(now)),
        ];
        // Budget 60,000 of the 150,000 held: pane 3 printed this very round
        // and still goes first, to the default; the 45,000 still over come
        // from the pane on screen, quiet an hour; the reader keeps it all.
        assert_eq!(
            shares(&mut panes),
            vec![(3, DEFAULT_SCROLLBACK_LINES), (2, DEFAULT_SCROLLBACK_LINES)]
        );
    }

    /// The pool's own walk, where the tie was: the pump stamps a pane that
    /// printed with the round's clock, and a pane scrolled back used to read
    /// as moved at that same instant — so which of the two went fell to the
    /// pool's hash order. Many fresh pools, so a coin cannot pass it.
    #[test]
    fn hold_sheds_the_quiet_pane_before_the_one_being_read_at_the_same_instant() {
        const CHOSEN: usize = 12_000;
        let now = Instant::now();
        for round in 0..16 {
            let pool = TerminalRegistry::default();
            let (read, quiet) = if round % 2 == 0 { (1, 2) } else { (2, 1) };
            seat(&pool, read, CHOSEN, CHOSEN, 7_000);
            seat(&pool, quiet, CHOSEN, CHOSEN, 0);
            let mut moved = HashMap::from([(read, now), (quiet, now)]);
            let mut entries = Vec::new();
            hold(&pool, &mut entries, &HashMap::new(), &mut moved, now);
            assert_eq!(held(&pool, read), (CHOSEN, 7_000), "round {round}");
            assert_eq!(
                held(&pool, quiet),
                (DEFAULT_SCROLLBACK_LINES, 0),
                "round {round}"
            );
            assert!(entries.is_empty(), "the pool's handles outlived the beat");
            assert_eq!(moved.get(&read), Some(&now));
        }
    }

    #[test]
    fn hold_keeps_the_pane_a_window_shows_while_another_has_lines_to_give() {
        const CHOSEN: usize = 12_000;
        let pool = TerminalRegistry::default();
        seat(&pool, 1, CHOSEN, CHOSEN, 0);
        seat(&pool, 2, CHOSEN, CHOSEN, 0);
        seat(&pool, 3, CHOSEN, CHOSEN, 0);
        let now = Instant::now();
        // The shown pane has been quiet longest; pane 9 has left the pool,
        // and the walk forgets it.
        let mut moved = HashMap::from([
            (2, now),
            (9, now),
            (
                1,
                now.checked_sub(Duration::from_secs(3_600))
                    .expect("an hour ago"),
            ),
        ]);
        let watched = HashMap::from([(1, vec!["main".to_owned()])]);
        let mut entries = Vec::new();
        hold(&pool, &mut entries, &watched, &mut moved, now);
        // Budget 22,000 of 36,000: panes 3 and 2 give 7,000 each.
        assert_eq!(held(&pool, 1), (CHOSEN, 0));
        assert_eq!(held(&pool, 2).0, DEFAULT_SCROLLBACK_LINES);
        assert_eq!(held(&pool, 3).0, DEFAULT_SCROLLBACK_LINES);
        assert!(!moved.contains_key(&9), "a pane gone from the pool stayed");

        // Two panes on screen, one quiet an hour: the pane off screen goes to
        // the default first, and only then the shown pane quiet longest.
        let pool = TerminalRegistry::default();
        seat(&pool, 1, CHOSEN, CHOSEN, 0);
        seat(&pool, 2, CHOSEN, CHOSEN, 0);
        seat(&pool, 3, CHOSEN, CHOSEN, 0);
        let watched = HashMap::from([(1, vec!["main".to_owned()]), (2, vec!["main".to_owned()])]);
        hold(&pool, &mut entries, &watched, &mut moved, now);
        assert_eq!(held(&pool, 3).0, DEFAULT_SCROLLBACK_LINES);
        assert_eq!(held(&pool, 1).0, DEFAULT_SCROLLBACK_LINES);
        assert_eq!(held(&pool, 2).0, CHOSEN);
    }
}
