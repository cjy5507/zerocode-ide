//! The world a STOPPED walk recovers in: the roads it already drives
//! (docs/design/jev-browser-action-20260917.md §7.1). A walk with no document
//! behind it — a goal walk — has no step to resume and drives
//! [`super::desk`]'s world instead; the two are separate because resuming a
//! document is the only thing that differs, and it is the whole of this file
//! that a goal walk would have had to leave empty.
//!
//! Nothing new reaches the browser from here. A look is the pane's own
//! `marks`, a press is `click --mark <n>`, and walking again is the caller's
//! own round with a start — all three down the same road every step of the
//! recipe took, so the pin, the fingerprint and the door's budget stand in
//! front of a recovery exactly as they stand in front of a step.
//!
//! The page's address is read ONCE, before any judgment, from the desk the
//! walk already holds ([`page_of`]). The `marks` answer does not carry it
//! (`cmd/browser.rs` `automate_marks` answers faces and a viewport), and a
//! second round trip inside the judgment's 1.5 s would be spending the clock
//! the walk still needs. Not knowing it is survivable: the address is context
//! for the question, never a gate — the gates are the closed answer space,
//! the pin and the fingerprint.

use std::time::Instant;

use serde_json::Value;
use zerocode_core::computer_recipe::{RecipeTool, recipe_line_holds_ms};
use zerocode_core::computer_use_protocol::marks::ITEMS_KEY;
use zerocode_hookd::TeamAnswer;

use super::{Screen, Seen, World};

/// The pane's host and path among the pages a desk sees (`Desk::pages`), or
/// two empty strings when no page answers to that label. The query is
/// dropped: it carries tokens. Takes the pages rather than the desk, so the
/// rule is one small pure thing a test holds without a whole desk.
#[must_use]
pub fn page_of(pages: &[(String, String)], pane: &str) -> (String, String) {
    let Some((_, url)) = pages.iter().find(|(label, _)| label == pane) else {
        return (String::new(), String::new());
    };
    let Ok(url) = url::Url::parse(url) else {
        return (String::new(), String::new());
    };
    (
        url.host_str().unwrap_or_default().to_string(),
        url.path().to_string(),
    )
}

/// The roads a stopped walk already drives, as a recovery's world.
pub struct WalkWorld<'a, Road, Walk> {
    road: &'a mut Road,
    walk: &'a mut Walk,
    pane: String,
    host: String,
    path: String,
    /// The call's whole clock, and what the first walk already spent of it.
    deadline_ms: u64,
    spent_ms: u64,
    began: Instant,
}

impl<'a, Road, Walk> WalkWorld<'a, Road, Walk> {
    /// `walk` takes the road, the milliseconds left and the step to start
    /// from — the caller's own round, which is the only thing that can make
    /// one (a closure cannot call itself).
    pub fn new(
        road: &'a mut Road,
        walk: &'a mut Walk,
        pane: &str,
        page: (String, String),
        deadline_ms: u64,
        spent_ms: u64,
    ) -> Self {
        Self {
            road,
            walk,
            pane: pane.to_string(),
            host: page.0,
            path: page.1,
            deadline_ms,
            spent_ms,
            began: Instant::now(),
        }
    }
}

impl<Road, Walk> World for WalkWorld<'_, Road, Walk>
where
    Road: FnMut(RecipeTool, &[String], &[String]) -> TeamAnswer,
    Walk: FnMut(&mut Road, u64, usize) -> Option<Value>,
{
    fn look(&mut self) -> Option<Screen> {
        let argv = vec!["marks".to_string(), self.pane.clone(), "--json".to_string()];
        let answer = (self.road)(RecipeTool::Browser, &argv, &argv);
        if answer.exit_code != 0 {
            return None;
        }
        let said = serde_json::from_str::<Value>(answer.stdout.trim()).ok()?;
        let items = said.get(ITEMS_KEY)?.as_array()?.clone();
        Some(Screen {
            at: Seen::Page {
                host: self.host.clone(),
                path: self.path.clone(),
            },
            items,
            shows: Vec::new(),
        })
    }

    fn press(&mut self, mark: usize) -> bool {
        // By number, never by selector: the pane re-measures the element it
        // handed that number to and refuses a press whose pin no longer holds.
        let argv = vec![
            "click".to_string(),
            self.pane.clone(),
            "--mark".to_string(),
            mark.to_string(),
        ];
        let holds = recipe_line_holds_ms(RecipeTool::Browser, &argv);
        let left = self.left_ms();
        if left == 0 || left < holds {
            return false;
        }
        (self.road)(RecipeTool::Browser, &argv, &argv).exit_code == 0
    }

    fn walk_from(&mut self, step: usize) -> Option<Value> {
        let left = self.left_ms();
        (self.walk)(self.road, left, step)
    }

    fn left_ms(&mut self) -> u64 {
        let spent = self
            .spent_ms
            .saturating_add(u64::try_from(self.began.elapsed().as_millis()).unwrap_or(u64::MAX));
        self.deadline_ms.saturating_sub(spent)
    }
}

#[cfg(test)]
mod tests;
