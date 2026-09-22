//! A page's side of the mention seat (t-6042): what the `@` popup and the
//! `/resume` picker each remember of the question asked about their page,
//! so the two surfaces keep one rule.
//!
//! The rule: the fuzzy page is drawn first; a question about it leaves off
//! the key path; the answer is applied only while the person has not moved
//! the selection and it still sits on the first row; a page that changed
//! under the question drops the answer; and the pick the person makes is
//! read against the page as the judgment saw it. Everything here is
//! bookkeeping — the seat itself is `tools::MentionRerank`.

use tools::{MentionAnswer, MentionCandidate};

/// The page the seat was last asked about.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Asked {
    query: String,
    /// The page's names in fuzzy order — the positions an answer's order and
    /// a pick are read against.
    names: Vec<String>,
    /// The question's ticket; `None` when the seat asked nothing (off, one
    /// row, an empty token).
    ticket: Option<u64>,
}

/// The pick the person made, as the label reads it: the question it
/// answers, the row's position on the page the judgment saw (`None` for a
/// row off that page), and whether the page had taken the judgment's order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pick {
    pub ticket: u64,
    pub position: Option<usize>,
    pub reordered: bool,
}

/// One surface's memory of the seat's question over its page.
#[derive(Debug, Clone, Default)]
pub struct PageRerank {
    asked: Option<Asked>,
    /// The answer for `asked`, once it came: the page's positions in the
    /// judgment's order, and whether the mode acts on it.
    answer: Option<(Vec<usize>, bool)>,
    /// Whether the person moved the selection since the question was asked:
    /// a page they chose from is not moved under them.
    moved: bool,
    /// Whether the page currently shows the judgment's order.
    reordered: bool,
}

impl PageRerank {
    /// The page as it stands after a change. A page that differs from the one
    /// last asked about is asked about — through `ask`, which answers the
    /// ticket the seat gave it — and forgets the old answer; a page that is
    /// the same is not asked again, and an answer already held is re-applied
    /// if it may be. Answers the order to permute the page by now, if any.
    pub fn page_stands(
        &mut self,
        query: &str,
        candidates: Vec<MentionCandidate>,
        selected_first: bool,
        ask: impl FnOnce(&str, Vec<MentionCandidate>) -> Option<u64>,
    ) -> Option<Vec<usize>> {
        let names: Vec<String> = candidates.iter().map(|candidate| candidate.name.clone()).collect();
        if self
            .asked
            .as_ref()
            .is_some_and(|asked| asked.query == query && asked.names == names)
        {
            return self.applicable(selected_first);
        }
        // A page of one row has nothing to reorder, and an empty token
        // nothing to rank by: neither is a question, and neither clones a page.
        let ticket = if names.len() > 1 && !query.trim().is_empty() { ask(query, candidates) } else { None };
        *self = Self {
            asked: Some(Asked { query: query.to_string(), names, ticket }),
            answer: None,
            moved: false,
            reordered: false,
        };
        None
    }

    /// An answer arrived. It is kept only when it names the question asked
    /// about the page as it stands (`names`); answers the order to permute
    /// the page by now, if it may be applied.
    pub fn answered(&mut self, answer: &MentionAnswer, names: &[String], selected_first: bool) -> Option<Vec<usize>> {
        let asked = self.asked.as_ref()?;
        if asked.ticket != Some(answer.ticket) || asked.names != names {
            return None;
        }
        if answer.order.len() != names.len() {
            return None;
        }
        self.answer = Some((answer.order.clone(), answer.applies));
        self.applicable(selected_first)
    }

    /// The person moved the selection: the page is theirs now.
    pub fn moved(&mut self) {
        self.moved = true;
    }

    /// The surface rebuilt its rows in fuzzy order.
    pub fn rows_rebuilt(&mut self) {
        self.reordered = false;
    }

    /// Whether the page shows the judgment's order.
    #[must_use]
    pub const fn reordered(&self) -> bool {
        self.reordered
    }

    /// The pick of the row named `selected`, read against the page the
    /// judgment saw. `None` when no question was asked about this page.
    #[must_use]
    pub fn pick(&self, selected: &str) -> Option<Pick> {
        let asked = self.asked.as_ref()?;
        Some(Pick {
            ticket: asked.ticket?,
            position: asked.names.iter().position(|name| name == selected),
            reordered: self.reordered,
        })
    }

    /// The order to apply now: an answer the mode acts on, on a page the
    /// person has not moved through, with the selection still on the first
    /// row, not already applied.
    fn applicable(&mut self, selected_first: bool) -> Option<Vec<usize>> {
        if self.reordered || self.moved || !selected_first {
            return None;
        }
        let (order, applies) = self.answer.as_ref()?;
        if !applies {
            return None;
        }
        self.reordered = true;
        Some(order.clone())
    }
}

/// `rows[..order.len()]` in `order`, the rest as they were — the one
/// permutation both surfaces apply.
pub fn permute_head<T: Clone>(rows: &mut [T], order: &[usize]) {
    if order.len() > rows.len() || order.iter().any(|position| *position >= order.len()) {
        return;
    }
    let head: Vec<T> = order.iter().map(|position| rows[*position].clone()).collect();
    for (slot, row) in rows.iter_mut().zip(head) {
        *slot = row;
    }
}

#[cfg(test)]
mod tests {
    use tools::MentionSurface;

    use super::*;

    fn candidates(names: &[&str]) -> Vec<MentionCandidate> {
        names
            .iter()
            .map(|name| MentionCandidate {
                name: (*name).to_string(),
                head: String::new(),
            })
            .collect()
    }

    fn names(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| (*name).to_string()).collect()
    }

    fn answer(ticket: u64, order: &[usize], applies: bool) -> MentionAnswer {
        MentionAnswer {
            ticket,
            surface: MentionSurface::Mention,
            order: order.to_vec(),
            applies,
        }
    }

    #[test]
    fn a_changed_page_is_asked_once_and_an_unchanged_page_is_not_asked_again() {
        let mut page = PageRerank::default();
        let asked = std::cell::Cell::new(0);
        let ask = |_: &str, _: Vec<MentionCandidate>| {
            asked.set(asked.get() + 1);
            Some(7)
        };
        assert_eq!(page.page_stands("co", candidates(&["a", "b"]), true, ask), None);
        assert_eq!(page.page_stands("co", candidates(&["a", "b"]), true, ask), None);
        assert_eq!(page.page_stands("com", candidates(&["a", "b"]), true, ask), None);
        assert_eq!(asked.get(), 2, "the same page is not asked twice");
        assert_eq!(page.page_stands("com", candidates(&["a"]), true, ask), None);
        assert_eq!(asked.get(), 2, "one row is not a question");
        assert_eq!(page.page_stands("", candidates(&["a", "b"]), true, ask), None);
        assert_eq!(asked.get(), 2, "an empty token is not a question");
    }

    #[test]
    fn a_late_answer_applies_only_on_the_first_row_of_an_unmoved_page() {
        let mut page = PageRerank::default();
        let ask = |_: &str, _: Vec<MentionCandidate>| Some(1);
        assert_eq!(page.page_stands("co", candidates(&["a", "b", "c"]), true, ask), None);
        let order = page.answered(&answer(1, &[2, 0, 1], true), &names(&["a", "b", "c"]), true);
        assert_eq!(order, Some(vec![2, 0, 1]));
        assert!(page.reordered());
        // Applied once: the same answer is not applied over itself.
        assert_eq!(page.answered(&answer(1, &[2, 0, 1], true), &names(&["a", "b", "c"]), true), None);

        let mut moved = PageRerank::default();
        assert_eq!(moved.page_stands("co", candidates(&["a", "b", "c"]), true, ask), None);
        moved.moved();
        assert_eq!(moved.answered(&answer(1, &[2, 0, 1], true), &names(&["a", "b", "c"]), true), None);
        assert!(!moved.reordered());

        let mut second_row = PageRerank::default();
        assert_eq!(second_row.page_stands("co", candidates(&["a", "b", "c"]), true, ask), None);
        assert_eq!(second_row.answered(&answer(1, &[2, 0, 1], true), &names(&["a", "b", "c"]), false), None);
    }

    #[test]
    fn an_answer_for_another_ticket_or_another_page_is_dropped() {
        let mut page = PageRerank::default();
        let ask = |_: &str, _: Vec<MentionCandidate>| Some(3);
        assert_eq!(page.page_stands("co", candidates(&["a", "b"]), true, ask), None);
        assert_eq!(page.answered(&answer(2, &[1, 0], true), &names(&["a", "b"]), true), None, "an older ticket");
        assert_eq!(page.answered(&answer(3, &[1, 0], true), &names(&["a", "z"]), true), None, "another page");
        assert_eq!(page.answered(&answer(3, &[1], true), &names(&["a", "b"]), true), None, "a short order");
        assert_eq!(page.answered(&answer(3, &[1, 0], true), &names(&["a", "b"]), true), Some(vec![1, 0]));
    }

    #[test]
    fn a_recording_answer_is_remembered_for_the_pick_and_never_applied() {
        let mut page = PageRerank::default();
        let ask = |_: &str, _: Vec<MentionCandidate>| Some(4);
        assert_eq!(page.page_stands("co", candidates(&["a", "b"]), true, ask), None);
        assert_eq!(page.answered(&answer(4, &[1, 0], false), &names(&["a", "b"]), true), None);
        assert!(!page.reordered());
        assert_eq!(page.pick("b"), Some(Pick { ticket: 4, position: Some(1), reordered: false }));
        assert_eq!(page.pick("zzz"), Some(Pick { ticket: 4, position: None, reordered: false }));
    }

    #[test]
    fn a_rebuilt_page_that_did_not_change_takes_the_remembered_order_again() {
        let mut page = PageRerank::default();
        let ask = |_: &str, _: Vec<MentionCandidate>| Some(5);
        assert_eq!(page.page_stands("co", candidates(&["a", "b"]), true, ask), None);
        assert_eq!(page.answered(&answer(5, &[1, 0], true), &names(&["a", "b"]), true), Some(vec![1, 0]));
        page.rows_rebuilt();
        assert!(!page.reordered());
        assert_eq!(page.page_stands("co", candidates(&["a", "b"]), true, ask), Some(vec![1, 0]));
        assert!(page.reordered());
    }

    #[test]
    fn a_page_nobody_asked_about_has_no_pick_to_label() {
        let mut page = PageRerank::default();
        let ask = |_: &str, _: Vec<MentionCandidate>| None;
        assert_eq!(page.page_stands("co", candidates(&["a", "b"]), true, ask), None);
        assert_eq!(page.pick("a"), None);
        assert_eq!(page.answered(&answer(1, &[1, 0], true), &names(&["a", "b"]), true), None);
    }

    #[test]
    fn the_head_is_permuted_and_the_tail_stands() {
        let mut rows = vec!["a", "b", "c", "d"];
        permute_head(&mut rows, &[2, 0, 1]);
        assert_eq!(rows, ["c", "a", "b", "d"]);
        permute_head(&mut rows, &[9, 0]);
        assert_eq!(rows, ["c", "a", "b", "d"], "a position off the page permutes nothing");
        permute_head(&mut rows, &[0, 1, 2, 3, 4]);
        assert_eq!(rows, ["c", "a", "b", "d"], "an order longer than the page permutes nothing");
    }
}
