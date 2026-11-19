//! Which shells each screen reads, and the pull that answers a screen — the
//! terminal read path as a host calls it, with no window toolkit in it.
//!
//! A host (the web window today, a native one next) names each of its screens
//! with a label it owns, and calls three things:
//!
//! - [`WatchedTerms::declare`] — a screen's whole current set of shells, every
//!   time it changes. This is the whole of subscribing and unsubscribing.
//! - [`WatchedTerms::pull`] — everything those shells owe that screen, in one
//!   answer: the first pull after a declaration is the whole screen, every
//!   later one what changed since, and `resync` asks for the whole screen
//!   again after a pull the screen lost.
//! - [`WatchedTerms::remove`] — the screen is gone.
//!
//! It hears one thing back, through [`DirtyNotices`]: "this screen is owed
//! frames" ([`tell_dirty`]). What that notice is called on the host's side
//! (`term:dirty:<label>` for a webview) and how an answer crosses to it (JSON
//! bytes for a webview) belong to the host's adapter. The engine answers in
//! Rust types ([`PulledScreens`]), and a native screen reads them as they are.
//!
//! The terminals stay wherever the host keeps them, behind [`TerminalPool`]:
//! one call that runs a piece of work on one shell under that shell's own
//! lock. Each shell's readers are [`crate::FrameReaders`], under that same lock.
//!
//! # Lock order
//!
//! The declaration lock, then (one at a time) a terminal's lock — nothing takes
//! the declaration while holding a terminal. A declaration holds the
//! declaration lock until the new set is visible, so a pull or a pump round
//! that reads the declaration reads it together with the readers it implies. A
//! pull holds it only while it copies one screen's set. No notice is delivered
//! under a lock of this module: [`tell_dirty`] takes none, so a
//! [`DirtyNotices`] may call straight back into [`WatchedTerms`].

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

use serde::Serialize;

use crate::grid::{GridDelta, Terminal};

/// A shell's number in its host's pool.
///
/// The host never hands the same number to a second shell: a number that came
/// back would let a frame of a shell that ended paint into its replacement.
pub type TermId = u32;

/// Where a host keeps its terminals.
///
/// One question: run `work` on the shell numbered `term`, under that shell's
/// own lock, and hand back what it made — `None` when no shell has that number
/// now. An implementation holds no other lock of its own while `work` runs, and
/// the work this module hands it takes none (the lock order above).
pub trait TerminalPool {
    fn with_terminal<R>(&self, term: TermId, work: impl FnOnce(&mut Terminal) -> R) -> Option<R>;
}

/// Where "this screen is owed frames" goes.
///
/// A notice carries nothing but the screen's label and owes nothing to the one
/// before it, so a lost one costs a moment ([`crate::RENOTIFY_AFTER`]), never a
/// screen. It is delivered with no lock of this module held, so an
/// implementation may call straight back into [`WatchedTerms`] — a native
/// screen that pulls the moment it is told.
pub trait DirtyNotices {
    fn dirty(&self, label: &str);
}

/// One notice a screen, however many of its shells have frames: its pull
/// answers for all of them at once.
///
/// `told` is what a pump round's looks collected, one label per shell that owes
/// that screen something ([`crate::FrameReaders::look`]). Sorted before it goes
/// out, so two rounds over the same readers tell them in the same order.
pub fn tell_dirty(mut told: Vec<Arc<str>>, notices: &impl DirtyNotices) {
    told.sort_unstable();
    told.dedup();
    for label in told {
        notices.dirty(&label);
    }
}

/// What one pull answers: the frames each shell owes this screen, in order,
/// and the declared shells that are not there at all.
///
/// The field names and their order are the wire a webview reads
/// (`{"screens":[{"term":…,"frames":[…]}],"missing":[…]}`), so they stay as
/// they are.
#[derive(Debug, Default, Serialize)]
pub struct PulledScreens {
    /// One entry per declared shell that owed frames, ascending by number. A
    /// shell that owed nothing is absent, not empty.
    pub screens: Vec<PulledScreen>,
    /// Declared shells the pool does not hold — for a board card, "no live
    /// terminal".
    pub missing: Vec<TermId>,
}

/// One shell's part of a pull.
#[derive(Debug, Serialize)]
pub struct PulledScreen {
    pub term: TermId,
    /// Oldest first; never empty.
    pub frames: Vec<GridDelta>,
}

/// Which shells each screen is reading, by the screen's label.
///
/// **Declarative, per screen**: a screen says its CURRENT whole set on every
/// visibility change, so its entry is replaced rather than adjusted. A count
/// that goes up and down drifts the first time a screen misses a decrement,
/// and the symptom of that drift is a terminal that silently stops painting.
/// Saying the same set twice is nothing at all, and saying it again always
/// restores the truth.
///
/// A shell's readers are exactly the screens named here
/// ([`crate::FrameReaders`]): a host's pump brings each shell's readers in line
/// with [`Self::readers_by_term`] every round, so a screen's entry is the truth
/// and the readers only its delivery state.
#[derive(Debug, Default)]
pub struct WatchedTerms {
    declared: Mutex<HashMap<String, HashSet<TermId>>>,
}

impl WatchedTerms {
    fn declared(&self) -> MutexGuard<'_, HashMap<String, HashSet<TermId>>> {
        self.declared
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The screen `label` reads exactly `terms` from now on.
    ///
    /// A shell it starts reading gets it as a reader that is owed a snapshot,
    /// and one it stops reading loses it — both under that shell's own lock and
    /// before the new set is visible, so the screen's first pull after this
    /// returns is paid its floor ([`crate::FrameReaders::declare`]) and a shell
    /// it just hid holds no share for it.
    pub fn declare(
        &self,
        pool: &impl TerminalPool,
        label: &str,
        terms: impl IntoIterator<Item = TermId>,
    ) {
        let next: HashSet<TermId> = terms.into_iter().collect();
        // Held until the replacement is visible, so a pull or a pump round
        // that reads the declaration reads it together with the readers it
        // implies. Lock order: the declaration, then (one at a time) a
        // terminal's lock — nothing takes the declaration while holding a
        // terminal.
        let mut watched = self.declared();
        let previous = watched.get(label).cloned().unwrap_or_default();
        for term in next.difference(&previous) {
            pool.with_terminal(*term, |terminal| terminal.readers_mut().0.declare(label));
        }
        for term in previous.difference(&next) {
            pool.with_terminal(*term, |terminal| terminal.readers_mut().0.leave(label));
        }
        watched.insert(label.to_string(), next);
    }

    /// Everything the screen `label`'s shells owe it, in one answer.
    ///
    /// The screen asks when it is ready — on a notice while it was quiet, then
    /// on each display frame while frames keep coming — so a screen that paints
    /// slower than the pump reads is handed one frame per paint, folded by the
    /// grid, and never a queue of screens that are already stale.
    ///
    /// Each shell is locked on its own, one at a time, in ascending order: the
    /// pull that pays a debt takes the delta for the other readers first and
    /// the snapshot after, in one lock ([`crate::FrameReaders::pull`]). A
    /// declared shell that is not in the pool is named in `missing`.
    ///
    /// `resync` is the screen saying its last pull failed. The frames that pull
    /// took are gone with it, and a model with a hole in it is wrong from then
    /// on; so every screen it reads is owed whole again, and this pull pays.
    pub fn pull(&self, pool: &impl TerminalPool, label: &str, resync: bool) -> PulledScreens {
        let mut terms: Vec<TermId> = self
            .declared()
            .get(label)
            .map(|terms| terms.iter().copied().collect())
            .unwrap_or_default();
        terms.sort_unstable();
        let now = Instant::now();
        let mut answer = PulledScreens::default();
        for term in terms {
            let Some(frames) = pool.with_terminal(term, |terminal| {
                let (readers, grid) = terminal.readers_mut();
                if resync {
                    readers.declare(label);
                }
                readers.pull(label, grid, now)
            }) else {
                answer.missing.push(term);
                continue;
            };
            if !frames.is_empty() {
                answer.screens.push(PulledScreen { term, frames });
            }
        }
        answer
    }

    /// The screen `label` is gone, and its declaration with it.
    ///
    /// Only the declaration: the shells it read lose it as a reader on the
    /// host's next pump round ([`crate::FrameReaders::reconcile`]), shares and
    /// all — the one place a shell's readers are brought in line with what the
    /// screens declared.
    pub fn remove(&self, label: &str) {
        self.declared().remove(label);
    }

    /// The shells the screen `label` declared last — empty for a screen that
    /// has declared nothing.
    #[must_use]
    pub fn terms_of(&self, label: &str) -> HashSet<TermId> {
        self.declared().get(label).cloned().unwrap_or_default()
    }

    /// Which screens read each shell, as of now ([`readers_by_term`]).
    #[must_use]
    pub fn readers_by_term(&self) -> HashMap<TermId, Vec<String>> {
        readers_by_term(&self.declared())
    }
}

/// Which screens asked for each shell, inverted from the per-screen sets.
///
/// Delivery needs the addresses: every screen that declared a shell is one of
/// its readers ([`crate::FrameReaders`]), and no other screen is. A shell with
/// no entry is read by nobody — frames stay in its grid, and a screen that
/// starts reading it later is owed a snapshot on its first pull. An empty map
/// means nobody has declared anything yet and reads as "nobody reads
/// anything", which is right: the first thing every surface does on the way to
/// visible is declare itself. Sorted so two rounds over the same declaration
/// keep the same readers in the same order — a board pop-out and the main
/// window must not swap places between rounds for reasons nobody can name.
#[must_use]
pub fn readers_by_term(screens: &HashMap<String, HashSet<TermId>>) -> HashMap<TermId, Vec<String>> {
    let mut readers: HashMap<TermId, Vec<String>> = HashMap::new();
    for (label, terms) in screens {
        for term in terms {
            readers.entry(*term).or_default().push(label.clone());
        }
    }
    for labels in readers.values_mut() {
        labels.sort();
    }
    readers
}
