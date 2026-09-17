//! Fixed ledger pointers left where a provider's own hook will collect them.
//!
//! # Why this exists beside the notification hub
//!
//! [`crate::orchestration_notify`] is for a provider the window can TELL
//! something: Codex has `codex queue`, so the window speaks and the provider
//! listens. The installed Claude CLI (2.1.261, measured) has no such surface —
//! every commander subcommand it registers was read, and none of them delivers
//! a message into a running interactive session. What it does have is a hook
//! road that this window already installs, and a documented reply to it: a
//! `Stop` hook may answer `{"decision":"block","reason":…}` and the model
//! continues with that reason instead of stopping.
//!
//! So the direction is inverted. The window cannot call the agent; the agent
//! calls the window, on every turn boundary, and this is the shelf the pointer
//! waits on until it does. No keystroke, no composer draft, and nobody
//! pressing Enter.
//!
//! # What this can and cannot carry
//!
//! A [`PointerNotice`] and nothing else, exactly like the notifier contract:
//! the fixed advice to run `zerocode-orc check`, never a message body, never a
//! receipt, never a `worker_done`. Collecting a pointer is not an
//! acknowledgement of anything; the ledger's own `check`/`--ack` remains the
//! only authority on delivery.
//!
//! # Why an uncollected pointer is safe
//!
//! A parked pointer is a HINT, and the pass that parks it writes no delivery
//! mark. If the turn is interrupted, if the pane exits, if the hook never
//! knocks — nothing was promised, the mail is still `pending` in the ledger,
//! and the ordinary pointer road speaks about it on a later beat exactly as it
//! did before this module existed. The one thing that must never happen is the
//! same pointer being handed over twice, because a continuation that keeps
//! continuing is a loop; so the shelf is a TAKE.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use zerocode_hookd::pointer_mailbox::{PointerMailbox, PointerMoment};
use zerocode_hookd::session_notify::PointerNotice;

/// One pane's waiting pointer.
///
/// Keyed by the window's own term, and looked up from the envelope's
/// `pane_key`, which this window spells `term-<n>` and nothing else
/// ([`crate::hooks::pane_key_of`]).
/// One pointer on a shelf, with the plain ledger identity beside it.
///
/// The notice itself is opaque on purpose — it cannot be taken apart into a
/// message id. The window put it here, so the window may keep its own note of
/// WHICH mail it is about, which is what lets a collected pointer be written
/// down as delivered.
struct Standing {
    notice: PointerNotice,
    run: String,
    address: String,
    newest: String,
    /// Which launch this pointer was parked for.
    ///
    /// A pane id is reused: a worker exits, another opens in the same seat,
    /// and an agent from the earlier launch can still be finishing a turn.
    /// Every other hook road forwards the payload and lets the window check
    /// this afterwards, but a mailbox CONSUMES — so the stale knock would
    /// swallow the live pane's pointer and nothing downstream would ever see
    /// it. Checked here, before anything is taken.
    ///
    /// `None` when the window had no token for the pane, which is the same
    /// posture `report_of` keeps: an identity nobody recorded refuses nobody.
    launch: Option<String>,
    since: Instant,
}

#[derive(Default)]
struct Shelf {
    waiting: Mutex<HashMap<u32, Standing>>,
    /// Pointers handed out but not yet PROVEN to have arrived.
    ///
    /// A take is not a delivery. The reply still has to be composed, written
    /// down the socket and read by the agent, and the window still has to
    /// have been there to consume the event that carried it — none of which
    /// this process can observe. So a taken pointer moves here rather than
    /// being written off, and if the mail is still pending when the
    /// re-notification bound passes, the road that types gets it back.
    ///
    /// In memory on purpose. The durable delivered-mark is what makes a
    /// watermark survive a restart, and writing one for a pointer that may
    /// never have arrived is how mail goes quiet forever.
    offered: Mutex<HashMap<u32, (Standing, Instant)>>,
    /// Terms whose turn a collected pointer has just CONTINUED.
    ///
    /// A `Stop` answered with a continuation is not a turn that ended, and the
    /// window reads the same event a moment later to decide whether to say a
    /// turn ended and ring for it. This is how it knows.
    continued: Mutex<HashMap<u32, Instant>>,
}

/// How long a parked pointer is given to be collected once the turn it was
/// parked for has ENDED.
///
/// The knock is already on its way: the generated hook script dials the bridge
/// with `--connect-timeout 0.5 --max-time 1.5`. Three seconds covers that and
/// a beat, and is short enough that a hook which never came costs one pause
/// before the composer road speaks instead.
pub(crate) const HOOK_COLLECTION_GRACE: Duration = Duration::from_secs(3);

/// What the pass should do about a pane that still has something on its shelf.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Parked {
    /// Nothing is waiting here.
    Empty,
    /// A pointer is waiting and its hook may still be on the way.
    Fresh,
    /// A pointer has been waiting longer than any knock should take. It is
    /// gone now, and the ordinary road owns this mail again.
    Abandoned,
}

impl std::fmt::Debug for Shelf {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let held = self
            .waiting
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .len();
        formatter
            .debug_struct("Shelf")
            .field("waiting", &held)
            .finish()
    }
}

impl PointerMailbox for Shelf {
    fn take(
        &self,
        pane_key: &str,
        launch_token: &str,
        moment: PointerMoment,
    ) -> Option<PointerNotice> {
        /* Which moment it is decides only what the window RECORDS. Both
         * moments want the same fixed sentence: a shelf that answered a turn
         * ending and not a session starting would leave a resumed pane's mail
         * sitting behind a turn boundary that is never coming. */
        let term = crate::hooks::term_of_pane_key(pane_key)?;
        let mut held = self
            .waiting
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        /* The identity gate, and it is a LOOK before it is a take: a knock
         * from another launch must leave the shelf exactly as it found it.
         * The rule is `report_of`'s, said in the same order — a token only
         * refuses when both sides have one and they differ. */
        if !launch_matches(held.get(&term)?.launch.as_deref(), launch_token) {
            return None;
        }
        let standing = held.remove(&term)?;
        drop(held);
        if moment == PointerMoment::TurnEnding {
            self.continued
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .insert(term, Instant::now());
        }
        /* OFFERED, which is the strongest word this side of the wire is
         * entitled to.
         *
         * It is enough to keep the two roads from both speaking: while a
         * pointer stands offered, the road that types holds off, so nobody
         * reads the same sentence twice. It is not enough to be written down
         * as delivered, and that distinction is the whole of this bookkeeping
         * — the reply has not been composed yet, the window may already be
         * gone, and the hook's own `--max-time` may cut the answer before the
         * agent ever reads it. A durable mark here would make one lost
         * wake-up into mail nothing mentions again, and a restart would not
         * repair it.
         *
         * So the offer expires. If the mail is still pending when it does,
         * the composer road takes it back and says it out loud. Nothing here
         * is an acknowledgement either way: only the ledger's own
         * `check`/`--ack` retires mail. */
        let notice = standing.notice.clone();
        self.offered
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(term, (standing, Instant::now()));
        Some(notice)
    }

    fn restore(&self, pane_key: &str) {
        /* The answer never went out. Put the pointer back on the shelf so the
         * next knock — or the road that types — still has it, rather than
         * leaving it in an offer nobody was ever made. */
        let Some(term) = crate::hooks::term_of_pane_key(pane_key) else {
            return;
        };
        let taken = self
            .offered
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&term);
        if let Some((standing, _)) = taken {
            self.waiting
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .insert(term, standing);
            self.continued
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .remove(&term);
        }
    }
}

fn shelf() -> &'static Arc<Shelf> {
    static SHELF: OnceLock<Arc<Shelf>> = OnceLock::new();
    SHELF.get_or_init(|| Arc::new(Shelf::default()))
}

/// The mailbox the bridge is handed at startup.
pub(crate) fn mailbox() -> Arc<dyn PointerMailbox> {
    Arc::clone(shelf()) as Arc<dyn PointerMailbox>
}

/// Leave this pane's pointer where its own hook will find it.
///
/// Replaces whatever was standing: a pointer names the newest waiting mail,
/// and a newer watermark says the same thing about more of it. Answers whether
/// the shelf changed, so the pass that parks can say so once rather than every
/// beat.
/// Whether a knock's launch token may collect what this pane parked.
///
/// Refuses only when both sides name a launch and the two disagree. An older
/// script carries no token, and a pane the window recorded no token for has no
/// claim to check — neither absence is evidence of a stale agent, and treating
/// it as one would silently disable the road on every pane a person started
/// by hand.
fn launch_matches(parked_for: Option<&str>, knocking: &str) -> bool {
    match parked_for {
        Some(parked) if !parked.is_empty() && !knocking.is_empty() => parked == knocking,
        _ => true,
    }
}

pub(crate) fn park(
    term: u32,
    run: &str,
    address: &str,
    newest: &str,
    launch: Option<String>,
    notice: PointerNotice,
) -> bool {
    let mut held = shelf()
        .waiting
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    match held.get(&term) {
        Some(standing) if standing.notice.id() == notice.id() => false,
        _ => {
            /* A newer watermark supersedes an offer nobody proved. The mail
             * it names includes everything the old one did, so holding the
             * old offer open would only delay the road that types. */
            shelf()
                .offered
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .remove(&term);
            held.insert(
                term,
                Standing {
                    notice,
                    run: run.to_string(),
                    address: address.to_string(),
                    newest: newest.to_string(),
                    launch,
                    since: Instant::now(),
                },
            );
            true
        }
    }
}

/// Whether this term's last turn was CONTINUED by a collected pointer, within
/// the window a hook's own round trip takes.
///
/// Asked by the report road, which sees the same `Stop` the bridge just
/// answered and would otherwise call it the end of a turn. Consumed on the
/// way out: one continuation answers for one event, and a second `Stop` is a
/// second question.
pub(crate) fn turn_was_continued(term: u32, within: Duration) -> bool {
    let mut held = shelf()
        .continued
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    match held.get(&term) {
        Some(at) if at.elapsed() < within => {
            held.remove(&term);
            true
        }
        Some(_) => {
            held.remove(&term);
            false
        }
        None => false,
    }
}

/// How long after a continuation the window still recognises the `Stop` it
/// belongs to. The bridge answers the hook and the envelope is already on its
/// way to the window's own loop; a second is orders of magnitude more than
/// that hand-off needs and far less than a turn.
pub(crate) const CONTINUATION_RECOGNITION: Duration = Duration::from_secs(1);

/// What is on this pane's shelf, and — if it has been there too long — take
/// it away.
///
/// Asked by the pass the moment a turn ENDS, which is the moment the hook
/// either knocks or never will. A pointer still standing after the grace is a
/// hook road that did not carry this one, so it is dropped here and the
/// composer road below owns the mail again. Dropping is the whole reason this
/// is not a plain read: two roads must never both be holding the same
/// pointer, or the person gets the advice twice.
pub(crate) fn collect_stale(
    term: u32,
    run: &str,
    address: &str,
    newest: &str,
    grace: Duration,
) -> Parked {
    /* About THIS mail, or about none. A shelf entry for an older watermark,
     * or for another of this pane's inboxes, says nothing about the mail the
     * pass is asking after — and holding off on its account would be one
     * address's pointer silencing another's. */
    let names_this_mail = |standing: &Standing| {
        standing.run == run && standing.address == address && standing.newest == newest
    };
    {
        let mut held = shelf()
            .waiting
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match held.get(&term) {
            Some(standing) if !names_this_mail(standing) => {}
            Some(standing) if standing.since.elapsed() < grace => return Parked::Fresh,
            Some(_) => {
                held.remove(&term);
                return Parked::Abandoned;
            }
            None => {}
        }
    }
    /* Nothing is waiting, but something may be OFFERED — handed to a hook
     * whose answer this process cannot watch land. While that offer is young
     * the agent is very likely acting on it, and saying the same thing into
     * its composer would be the window stuttering. Once the bound passes with
     * the mail still pending, the offer was not a delivery, and the road that
     * types owns it again. */
    let mut offered = shelf()
        .offered
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    match offered.get(&term) {
        Some((standing, _)) if !names_this_mail(standing) => Parked::Empty,
        Some((_, at)) if at.elapsed() < RENOTIFY_AFTER => Parked::Fresh,
        Some(_) => {
            offered.remove(&term);
            Parked::Abandoned
        }
        None => Parked::Empty,
    }
}

/// How long an offered pointer stands before the road that types takes it
/// back.
///
/// The measure is "long enough that an agent which GOT it has had its say".
/// A tool-boundary pointer lands mid-turn and the agent finishes what it is
/// doing first; a continuation lands at a turn's end and is acted on at once.
/// A minute covers both with room, and it bounds the cost of a lost wake-up
/// to a minute rather than to forever — which is what a durable mark on an
/// unproven delivery would have cost.
pub(crate) const RENOTIFY_AFTER: Duration = Duration::from_secs(60);

/// Forget a pane's shelf. Every road out of a terminal reaches this.
pub(crate) fn forget_term(term: u32) {
    shelf()
        .waiting
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove(&term);
    shelf()
        .offered
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove(&term);
    shelf()
        .continued
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove(&term);
}

/// Whether this agent has a MEASURED hook road for a fixed pointer.
///
/// Two questions, both asked of the catalog rather than of a name: the
/// provider must run this window's hooks at all, and it must have been
/// measured to honour a turn-end continuation. A provider that fails either is
/// not offered the route and is not reported as having one — an honest
/// `false` here is what keeps the PTY road from being skipped for a pane that
/// nothing can reach any other way.
pub(crate) fn agent_has_hook_route(agent: Option<&str>) -> bool {
    agent
        .and_then(zerocode_core::AgentKind::from_slug)
        .is_some_and(|kind| {
            kind.hook_additional_context().is_some()
                && zerocode_core::hook_continuation::stop_continuation(kind).is_some()
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notice(newest: &str, pending: usize) -> PointerNotice {
        PointerNotice::new("run-1", "run:run-1", newest, pending).expect("a pointer")
    }

    fn standing(notice: PointerNotice) -> Standing {
        Standing {
            notice,
            run: "run-1".to_string(),
            address: "run:run-1".to_string(),
            newest: "m-1".to_string(),
            launch: Some("launch-1".to_string()),
            since: Instant::now(),
        }
    }

    /// The whole life of one shelf: parked, collected once, and gone.
    #[test]
    fn a_parked_pointer_is_collected_exactly_once() {
        let shelf = Shelf::default();
        shelf
            .waiting
            .lock()
            .expect("a fresh shelf")
            .insert(41, standing(notice("m-1", 2)));

        let collected = shelf.take("term-41", "launch-1", PointerMoment::TurnEnding);
        assert_eq!(
            collected.as_ref().map(PointerNotice::text),
            Some(zerocode_core::orchestration::pointer_text(2).as_str())
        );
        assert!(
            shelf
                .take("term-41", "launch-1", PointerMoment::TurnEnding)
                .is_none(),
            "a second collection is a continuation that continues itself"
        );
    }

    /// A pane key this window did not mint names no pane, and answering
    /// anything for it would be answering for a terminal nobody can see.
    #[test]
    fn a_pane_key_from_nowhere_collects_nothing() {
        let shelf = Shelf::default();
        shelf
            .waiting
            .lock()
            .expect("a fresh shelf")
            .insert(42, standing(notice("m-1", 1)));

        assert!(
            shelf
                .take("someone-elses-pane", "launch-1", PointerMoment::TurnEnding)
                .is_none()
        );
        assert!(
            shelf
                .take("term-", "launch-1", PointerMoment::TurnEnding)
                .is_none()
        );
        assert!(
            shelf
                .take("term-43", "launch-1", PointerMoment::TurnEnding)
                .is_none()
        );
        /* A knock from a PREVIOUS launch of this reused pane takes nothing,
         * and — the half that matters — leaves the pointer on the shelf for
         * the agent actually living here. */
        assert!(
            shelf
                .take("term-42", "launch-0", PointerMoment::TurnEnding)
                .is_none(),
            "a stale launch collected a live pane's pointer"
        );
        assert!(
            shelf
                .take("term-42", "launch-1", PointerMoment::SessionStarting)
                .is_some(),
            "the stale knock swallowed the pointer on its way past"
        );
    }

    /// A pointer nobody collected is taken away, so the composer road can own
    /// the same mail without the two of them both holding it.
    #[test]
    fn a_pointer_no_hook_collected_is_abandoned_after_its_grace() {
        let waiting =
            |term, newest, grace| collect_stale(term, "run-1", "run:run-1", newest, grace);
        assert_eq!(waiting(4_401, "m-1", HOOK_COLLECTION_GRACE), Parked::Empty);
        assert!(park(
            4_401,
            "run-1",
            "run:run-1",
            "m-1",
            None,
            notice("m-1", 1)
        ));
        assert_eq!(waiting(4_401, "m-1", HOOK_COLLECTION_GRACE), Parked::Fresh);
        // An entry about other mail says nothing about this mail.
        assert_eq!(waiting(4_401, "m-9", HOOK_COLLECTION_GRACE), Parked::Empty);
        assert_eq!(waiting(4_401, "m-1", Duration::ZERO), Parked::Abandoned);
        assert_eq!(waiting(4_401, "m-1", HOOK_COLLECTION_GRACE), Parked::Empty);

        // The same watermark twice is one park; a newer one replaces it.
        assert!(park(
            4_402,
            "run-1",
            "run:run-1",
            "m-1",
            None,
            notice("m-1", 1)
        ));
        assert!(!park(
            4_402,
            "run-1",
            "run:run-1",
            "m-1",
            None,
            notice("m-1", 1)
        ));
        assert!(park(
            4_402,
            "run-1",
            "run:run-1",
            "m-2",
            None,
            notice("m-2", 2)
        ));
        forget_term(4_402);
        assert_eq!(waiting(4_402, "m-2", HOOK_COLLECTION_GRACE), Parked::Empty);
    }

    /// A take is an OFFER, and an offer that was never made comes back.
    ///
    /// This is the lost-wake-up case: the reply is composed after the take,
    /// written down a socket, and read by an agent — none of which this
    /// process can watch. The offer therefore expires, and a `restore` from a
    /// bridge that could not answer at all puts the pointer straight back.
    /// Neither road may write the durable delivered-mark, because a watermark
    /// on a pointer that never arrived is mail no restart repairs.
    #[test]
    fn an_offer_expires_and_a_failed_answer_hands_the_pointer_straight_back() {
        let shelf = Shelf::default();
        shelf
            .waiting
            .lock()
            .expect("a fresh shelf")
            .insert(44, standing(notice("m-1", 1)));

        // Taken, and now merely offered — not written off.
        assert!(
            shelf
                .take("term-44", "launch-1", PointerMoment::ToolBoundary)
                .is_some()
        );
        assert!(shelf.waiting.lock().expect("the shelf").is_empty());
        assert_eq!(shelf.offered.lock().expect("the offers").len(), 1);

        // The answer never went out: back on the shelf, and the turn is no
        // longer counted as continued.
        shelf.restore("term-44");
        assert_eq!(shelf.waiting.lock().expect("the shelf").len(), 1);
        assert!(shelf.offered.lock().expect("the offers").is_empty());
        assert!(
            shelf
                .take("term-44", "launch-1", PointerMoment::TurnEnding)
                .is_some(),
            "the restored pointer could not be offered again"
        );
        assert!(
            shelf.continued.lock().expect("the marks").contains_key(&44),
            "a turn-end offer left no continuation mark"
        );
        shelf.restore("term-44");
        assert!(
            !shelf.continued.lock().expect("the marks").contains_key(&44),
            "a continuation stood for an answer that never went out"
        );
    }

    /// While an offer is young the road that types holds off; once the bound
    /// passes with the mail still pending, it takes it back and says it.
    #[test]
    fn an_unproven_offer_is_taken_back_by_the_road_that_types() {
        let about = |newest, grace| collect_stale(4_403, "run-1", "run:run-1", newest, grace);
        assert!(park(
            4_403,
            "run-1",
            "run:run-1",
            "m-4",
            None,
            notice("m-4", 1)
        ));
        let handed = mailbox().take(
            &crate::hooks::pane_key_of(4_403),
            "",
            PointerMoment::ToolBoundary,
        );
        assert!(handed.is_some());

        assert_eq!(about("m-4", HOOK_COLLECTION_GRACE), Parked::Fresh);
        // An offer about other mail silences nothing.
        assert_eq!(about("m-5", HOOK_COLLECTION_GRACE), Parked::Empty);

        // Age it past the bound by hand: the clock is the point, not the wait.
        {
            let mut offered = shelf().offered.lock().expect("the offers");
            let (standing, _) = offered.remove(&4_403).expect("the offer");
            offered.insert(4_403, (standing, Instant::now() - RENOTIFY_AFTER));
        }
        assert_eq!(about("m-4", HOOK_COLLECTION_GRACE), Parked::Abandoned);
        assert_eq!(about("m-4", HOOK_COLLECTION_GRACE), Parked::Empty);
        forget_term(4_403);
    }

    /// Neither side naming a launch refuses nobody; two that disagree do.
    #[test]
    fn a_launch_token_refuses_only_a_knock_that_contradicts_the_park() {
        assert!(launch_matches(Some("launch-1"), "launch-1"));
        assert!(!launch_matches(Some("launch-1"), "launch-2"));
        // An older script carries none, and a pane the window recorded none
        // for has no claim to check. Absence is not evidence of staleness.
        assert!(launch_matches(Some("launch-1"), ""));
        assert!(launch_matches(None, "launch-2"));
        assert!(launch_matches(None, ""));
        assert!(launch_matches(Some(""), "launch-2"));
    }

    /// Only a provider measured to honour a continuation is offered one.
    #[test]
    fn the_hook_route_is_claimed_only_for_a_measured_provider() {
        assert!(agent_has_hook_route(Some("claude")));
        // Codex runs our hooks and has a native queue of its own; what it must
        // not get is another provider's continuation shape guessed for it.
        assert!(!agent_has_hook_route(Some("codex")));
        assert!(!agent_has_hook_route(Some("zo")));
        assert!(!agent_has_hook_route(None));
        assert!(!agent_has_hook_route(Some("a-name-nobody-ships")));
    }
}
