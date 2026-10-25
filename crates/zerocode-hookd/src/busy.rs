//! Which door of the team road refused, and how long its holders have held on
//! (t-47458).
//!
//! A request from an agent's pane passes up to five doors on its way to an
//! answer: the body permit (a body being read), the wait permit (a long-poll
//! seat), the queue to the window, and — inside the window — the active credit
//! and the window's own wait credit. All five used to refuse with the same
//! sentence, so a "this window is busy" that stood for three hours (t-47141)
//! could not say which door had closed or whether a holder had been sitting on
//! it since the morning.
//!
//! The kind of the refusal is unchanged: status 429, and a body that opens with
//! [`TEAM_BUSY_LEAD`] and closes with [`TEAM_BUSY_TAIL`]. A caller that reads
//! only "busy" reads the same two ends it always read; the entrance and the
//! holders ride between them.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

/// How a busy refusal opens. Callers that read only the busy kind match this.
pub const TEAM_BUSY_LEAD: &str = "tmux: this window is busy";
/// How a busy refusal closes.
pub const TEAM_BUSY_TAIL: &str = "; retry the request\n";

/// The door that said no.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BusyEntry {
    /// Bodies being read before they can contend for the queue.
    BodyPermit,
    /// Long-poll seats (`check --wait`, `ask`, `worker-start`, `delegate`).
    WaitPermit,
    /// Requests waiting between the bridge and the window.
    FullQueue,
    /// The window's cap on team effects running or waiting on an inbox.
    WindowCredit,
    /// The window's cap on long-poll seats, a share of [`Self::WindowCredit`].
    WindowWaitCredit,
}

impl BusyEntry {
    /// The words a refusal and a person use for this door.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::BodyPermit => "body permit",
            Self::WaitPermit => "wait permit",
            Self::FullQueue => "full queue",
            Self::WindowCredit => "window credit",
            Self::WindowWaitCredit => "window wait credit",
        }
    }
}

/// One refusal: the door, how full it was, and — where the door keeps a clock
/// per holder — how long its longest holder has held.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Busy {
    entry: BusyEntry,
    limit: usize,
    held: usize,
    oldest: Option<Duration>,
}

impl Busy {
    /// The queue between the bridge and the window was at capacity. The queue
    /// keeps no clock per request, so this refusal claims no holder age.
    #[must_use]
    pub fn full_queue(capacity: usize) -> Self {
        Self {
            entry: BusyEntry::FullQueue,
            limit: capacity,
            held: capacity,
            oldest: None,
        }
    }

    #[must_use]
    pub fn entry(&self) -> BusyEntry {
        self.entry
    }

    #[must_use]
    pub fn limit(&self) -> usize {
        self.limit
    }

    #[must_use]
    pub fn held(&self) -> usize {
        self.held
    }

    /// How long the longest holder has held, when this door can tell.
    #[must_use]
    pub fn oldest(&self) -> Option<Duration> {
        self.oldest
    }

    /// The refusal as the shim prints it on stderr.
    #[must_use]
    pub fn message(&self) -> String {
        let name = self.entry.name();
        let (held, limit) = (self.held, self.limit);
        let detail = match (self.entry, self.oldest) {
            (BusyEntry::FullQueue, _) => format!("{held} of {limit} waiting"),
            (_, Some(oldest)) => {
                format!("{held} of {limit} held, oldest held {}", age_words(oldest))
            }
            (_, None) => format!("{held} of {limit} held"),
        };
        format!("{TEAM_BUSY_LEAD} (refused at the {name}: {detail}){TEAM_BUSY_TAIL}")
    }
}

/// A duration a person reads at a glance, every number with its unit.
fn age_words(age: Duration) -> String {
    let millis = age.as_millis();
    let seconds = age.as_secs();
    if millis < 1_000 {
        format!("{millis} ms")
    } else if seconds < 60 {
        format!("{seconds} s")
    } else if seconds < 3_600 {
        format!("{} min {} s", seconds / 60, seconds % 60)
    } else {
        format!("{} h {} min", seconds / 3_600, seconds % 3_600 / 60)
    }
}

#[derive(Default)]
struct Held {
    next: u64,
    since: BTreeMap<u64, Instant>,
}

/// A fixed number of seats that never queues a caller: it admits or refuses on
/// the spot, and remembers when each holder sat down so a refusal can say how
/// long the longest one has been there.
#[derive(Clone)]
pub struct Seats {
    entry: BusyEntry,
    limit: usize,
    gate: Arc<tokio::sync::Semaphore>,
    held: Arc<Mutex<Held>>,
}

impl Seats {
    #[must_use]
    pub fn new(entry: BusyEntry, limit: usize) -> Self {
        Self {
            entry,
            limit,
            gate: Arc::new(tokio::sync::Semaphore::new(limit)),
            held: Arc::new(Mutex::new(Held::default())),
        }
    }

    /// Seats nobody holds right now.
    #[must_use]
    pub fn available(&self) -> usize {
        self.gate.available_permits()
    }

    /// Take a seat or learn which door is full. Never waits.
    pub fn try_acquire(&self) -> Result<Seat, Busy> {
        self.try_acquire_at(Instant::now())
    }

    /// [`Self::try_acquire`] with the moment given, so a test reads an age it
    /// chose rather than one the clock produced.
    pub fn try_acquire_at(&self, now: Instant) -> Result<Seat, Busy> {
        match Arc::clone(&self.gate).try_acquire_owned() {
            Ok(permit) => {
                let mut held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
                let id = held.next;
                held.next += 1;
                held.since.insert(id, now);
                Ok(Seat {
                    _permit: permit,
                    held: Arc::clone(&self.held),
                    id,
                })
            }
            Err(_) => Err(self.busy_at(now)),
        }
    }

    fn busy_at(&self, now: Instant) -> Busy {
        let held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
        let oldest = held
            .since
            .values()
            .min()
            .map(|since| now.saturating_duration_since(*since));
        Busy {
            entry: self.entry,
            limit: self.limit,
            held: self.limit.saturating_sub(self.available()),
            oldest,
        }
    }
}

/// One seat held. Dropping it — on every road out, unwinding included — gives
/// the seat back and forgets the holder.
pub struct Seat {
    _permit: tokio::sync::OwnedSemaphorePermit,
    held: Arc<Mutex<Held>>,
    id: u64,
}

impl Drop for Seat {
    fn drop(&mut self) {
        self.held
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .since
            .remove(&self.id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [BusyEntry; 5] = [
        BusyEntry::BodyPermit,
        BusyEntry::WaitPermit,
        BusyEntry::FullQueue,
        BusyEntry::WindowCredit,
        BusyEntry::WindowWaitCredit,
    ];

    fn seconds(count: u64) -> Duration {
        Duration::from_secs(count)
    }

    #[test]
    fn a_full_door_reports_its_longest_holder_not_its_latest() {
        let base = Instant::now();
        let seats = Seats::new(BusyEntry::WaitPermit, 3);
        let first = seats.try_acquire_at(base).expect("first seat");
        let _second = seats
            .try_acquire_at(base + seconds(40))
            .expect("second seat");
        let _third = seats
            .try_acquire_at(base + seconds(70))
            .expect("third seat");

        let busy = seats
            .try_acquire_at(base + seconds(100))
            .err()
            .expect("the door is full");
        assert_eq!(busy.entry(), BusyEntry::WaitPermit);
        assert_eq!((busy.held(), busy.limit()), (3, 3));
        assert_eq!(busy.oldest(), Some(seconds(100)));

        // The longest holder leaves: the next longest is now the answer.
        drop(first);
        let _fourth = seats
            .try_acquire_at(base + seconds(110))
            .expect("seat freed");
        let busy = seats
            .try_acquire_at(base + seconds(120))
            .err()
            .expect("full again");
        assert_eq!(busy.oldest(), Some(seconds(80)));
    }

    #[test]
    fn a_seat_returns_on_drop_and_on_unwind_and_leaves_no_holder_behind() {
        let base = Instant::now();
        let seats = Seats::new(BusyEntry::BodyPermit, 2);
        let seat = seats.try_acquire_at(base).expect("seat");
        assert_eq!(seats.available(), 1);
        drop(seat);
        assert_eq!(seats.available(), 2);

        let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _seat = seats.try_acquire_at(base).expect("seat");
            panic!("injected holder panic");
        }));
        assert!(unwound.is_err());
        assert_eq!(seats.available(), 2);

        let _a = seats.try_acquire_at(base).expect("a");
        let _b = seats.try_acquire_at(base).expect("b");
        let busy = seats.try_acquire_at(base + seconds(5)).err().expect("full");
        assert_eq!(busy.held(), 2, "a dropped holder was still counted");
        assert_eq!(busy.oldest(), Some(seconds(5)));
    }

    #[test]
    fn every_door_has_its_own_name_and_the_busy_kind_keeps_its_two_ends() {
        let names: Vec<&str> = ALL.iter().map(|entry| entry.name()).collect();
        for (index, name) in names.iter().enumerate() {
            assert!(!name.is_empty());
            assert_eq!(
                names.iter().filter(|other| *other == name).count(),
                1,
                "two doors share the name `{name}`"
            );
            assert!(
                names
                    .iter()
                    .enumerate()
                    .all(|(other, longer)| other == index || !longer.contains(name)),
                "the name `{name}` is inside another door's name, so a reader cannot tell them apart"
            );
        }

        let base = Instant::now();
        for entry in [
            BusyEntry::BodyPermit,
            BusyEntry::WaitPermit,
            BusyEntry::WindowCredit,
            BusyEntry::WindowWaitCredit,
        ] {
            let seats = Seats::new(entry, 1);
            let _held = seats.try_acquire_at(base).expect("seat");
            let words = seats
                .try_acquire_at(base + seconds(3_725))
                .err()
                .expect("full")
                .message();
            assert!(words.starts_with(TEAM_BUSY_LEAD), "{words:?}");
            assert!(words.ends_with(TEAM_BUSY_TAIL), "{words:?}");
            assert!(
                words.contains(&format!(
                    "(refused at the {}: 1 of 1 held, oldest held 1 h 2 min)",
                    entry.name()
                )),
                "{words:?}"
            );
        }
        let words = Busy::full_queue(16).message();
        assert_eq!(
            words,
            "tmux: this window is busy (refused at the full queue: 16 of 16 waiting); retry the request\n"
        );
    }

    #[test]
    fn an_age_is_worded_with_a_unit_on_every_number() {
        let cases = [
            (Duration::from_millis(0), "0 ms"),
            (Duration::from_millis(999), "999 ms"),
            (seconds(1), "1 s"),
            (seconds(59), "59 s"),
            (seconds(60), "1 min 0 s"),
            (seconds(3_599), "59 min 59 s"),
            (seconds(3_600), "1 h 0 min"),
            (seconds(3 * 3_600 + 12 * 60 + 59), "3 h 12 min"),
        ];
        for (age, words) in cases {
            assert_eq!(age_words(age), words, "{age:?}");
        }
    }
}
