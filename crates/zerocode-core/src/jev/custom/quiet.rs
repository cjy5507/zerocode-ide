//! A person's decisions' quiet marks (t-106308, t-107587): the one book the
//! window's panes and zo both keep of the decisions whose refusal would
//! repeat, and the one fold that counts such a refusal once a span.
//!
//! A program that asks a decision far more often than the door lets through
//! — a pane's agent or zo runs thousands of commands a week — meets the same
//! refusal again and again until something outside it changes: an hour or a
//! day found full, no key, the switch off, a folder not consented, a
//! definition that breaks a rule. Such a refusal writes one row and makes the
//! decision quiet ([`Quiet::refused`], [`Quiet::broke`]): until its span ends
//! or its definition changes, a command costs it nothing more — no door, no
//! count file, no lock, no row. Which refusals repeat, for how long, and
//! which are one folder's are [`Refused::repeats`] and
//! [`Refused::of_the_folder`]; this book only keeps what they said.
//!
//! The marks live as long as what they were read from: each program starts
//! a new book when its settings are read again — and an ask that began
//! before keeps marking the old one, which nothing reads — so a key put in,
//! consent given or a bound raised asks at once; a mark of a decision whose
//! definition changed holds no more; and a mark whose span has passed is
//! dropped when the next is made. So a book stays bounded by the decisions
//! times the folders one program asks them in.
//!
//! Rows written before this book, or by several programs each writing its
//! one row for the same span, are counted once a span by [`counted_once`].

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use serde_json::Value;

use crate::jev::count;
use crate::jev::door::{Refused, Span};
use crate::jev::summary::{AT, OUTCOME};

/// The local hour and day a moment falls in, as numbers
/// ([`count::hour_number`], [`count::day_number`]): what a mark is held
/// against, compared without allocating.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Now {
    hour: i64,
    day: i64,
}

impl Now {
    /// The local hour and day of `epoch_ms`, on a clock `offset_minutes`
    /// ahead of UTC — the hour and day the door counts a request in.
    #[must_use]
    pub fn at(epoch_ms: i64, offset_minutes: i32) -> Self {
        Self {
            hour: count::hour_number(epoch_ms, offset_minutes),
            day: count::day_number(epoch_ms, offset_minutes),
        }
    }

    const fn of(self, span: Span) -> Until {
        match span {
            Span::Hour => Until::Hour(self.hour),
            Span::Day => Until::Day(self.day),
        }
    }
}

/// Where a quiet decision's refusal stops repeating: the local hour or day
/// it was refused in ([`Refused::repeats`]), or — for a definition that
/// breaks a rule — the version that broke it, mended only by a new one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Until {
    Hour(i64),
    Day(i64),
    Mended,
}

impl Until {
    /// Whether the refusal still repeats at `now`.
    const fn holds(self, now: Now) -> bool {
        match self {
            Self::Hour(hour) => hour == now.hour,
            Self::Day(day) => day == now.day,
            Self::Mended => true,
        }
    }

    /// Whether it may hold at `now` or later: a span already past never
    /// will, so its mark is dropped.
    const fn ahead(self, now: Now) -> bool {
        match self {
            Self::Hour(hour) => hour >= now.hour,
            Self::Day(day) => day >= now.day,
            Self::Mended => true,
        }
    }
}

/// One quiet mark: the version of the decision it was refused at, and where
/// the refusal stops.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Mark {
    version: String,
    until: Until,
}

impl Mark {
    fn holds(&self, version: &str, now: Now) -> bool {
        self.version == version && self.until.holds(now)
    }
}

/// The marks, by decision: those for the whole machine, and those for one
/// folder — the folder asked in, resolved — for a refusal that folder owns
/// ([`Refused::of_the_folder`]). Two tables so a mark is looked up by
/// borrowing, allocating nothing.
#[derive(Debug, Default)]
struct Marks {
    everywhere: HashMap<String, Mark>,
    in_folder: HashMap<PathBuf, HashMap<String, Mark>>,
}

impl Marks {
    /// Drop every mark whose span has passed at `now`, and every folder left
    /// with none.
    fn forget_past(&mut self, now: Now) {
        self.everywhere.retain(|_, mark| mark.until.ahead(now));
        self.in_folder.retain(|_, marks| {
            marks.retain(|_, mark| mark.until.ahead(now));
            !marks.is_empty()
        });
    }
}

/// The decisions whose refusal would repeat, each with where it stops: for
/// the machine, or for the one folder whose refusal it is.
#[derive(Debug, Default)]
pub struct Quiet {
    marks: Mutex<Marks>,
}

impl Quiet {
    /// Whether the decision `id`, at the version `version`, is quiet at
    /// `now`: everywhere, or — when the folder asked in is known — in
    /// `folder`. Borrows and compares; allocates nothing and reads no file.
    #[must_use]
    pub fn is(&self, id: &str, version: &str, folder: Option<&Path>, now: Now) -> bool {
        let marks = self.marks.lock().unwrap_or_else(PoisonError::into_inner);
        marks
            .everywhere
            .get(id)
            .is_some_and(|mark| mark.holds(version, now))
            || folder
                .and_then(|folder| marks.in_folder.get(folder))
                .and_then(|marked| marked.get(id))
                .is_some_and(|mark| mark.holds(version, now))
    }

    /// The decision `id`, at `version`, asked in `folder` at `now`, was
    /// refused with `token`: whether its row is written. A refusal that
    /// repeats ([`Refused::repeats`]) marks the decision quiet — in `folder`
    /// alone when it is the folder's own ([`Refused::of_the_folder`]) — and
    /// writes its row only when that mark is new, so of every ask that meets
    /// it in its span exactly one says so. An hourly refusal repeats only
    /// when the hour is truly full (`hour_is_full`, [`crate::jev::door::Hour::is_full`]):
    /// a lock that stayed busy is a moment's. Anything else — a wall, a
    /// malformed reply, a server's wait — writes its row each time.
    pub fn refused(
        &self,
        (id, version): (&str, &str),
        folder: &Path,
        token: &str,
        now: Now,
        hour_is_full: impl FnOnce() -> bool,
    ) -> bool {
        let Some(refused) = Refused::from_token(token) else {
            return true;
        };
        let Some(span) = refused.repeats() else {
            return true;
        };
        if refused == Refused::HourlyBudget && !hour_is_full() {
            return true;
        }
        let folder = refused.of_the_folder().then_some(folder);
        self.mark((id, version), folder, now.of(span), now)
    }

    /// The decision `id`'s definition at `version` breaks a rule it was kept
    /// under ([`super::check`]): whether its row is written — once, until the
    /// definition changes.
    pub fn broke(&self, (id, version): (&str, &str), now: Now) -> bool {
        self.mark((id, version), None, Until::Mended, now)
    }

    /// Mark `id` at `version` quiet until `until` — in `folder` alone, or
    /// everywhere: whether this mark is new.
    fn mark(
        &self,
        (id, version): (&str, &str),
        folder: Option<&Path>,
        until: Until,
        now: Now,
    ) -> bool {
        let mut marks = self.marks.lock().unwrap_or_else(PoisonError::into_inner);
        marks.forget_past(now);
        let marked = match folder {
            Some(folder) => marks.in_folder.entry(folder.to_path_buf()).or_default(),
            None => &mut marks.everywhere,
        };
        if marked
            .get(id)
            .is_some_and(|mark| mark.version == version && mark.until == until)
        {
            return false;
        }
        marked.insert(
            id.to_string(),
            Mark {
                version: version.to_string(),
                until,
            },
        );
        true
    }
}

/// The rows of a decision's ledger from `since_ms` on, as they count: a
/// refusal that repeats ([`Refused::repeats`]) once per decision per span —
/// the hour for an hour's bound, the day for the day's — whoever wrote its
/// rows and however many did, on a clock `offset_minutes` ahead of UTC.
/// Answers and one-off failures pass as they are. One lazy pass, remembering
/// a span by borrowing its row's words.
pub fn counted_once<'row>(
    rows: impl IntoIterator<Item = &'row Value>,
    since_ms: i64,
    offset_minutes: i32,
) -> impl Iterator<Item = &'row Value> {
    let mut seen: HashSet<(&'row str, &'row str, Until)> = HashSet::new();
    rows.into_iter().filter(move |row| {
        let at = AT.read(row).and_then(Value::as_i64).unwrap_or(0);
        if at < since_ms {
            return false;
        }
        let Some(token) = OUTCOME.read(row).and_then(Value::as_str) else {
            return true;
        };
        let Some(span) = Refused::from_token(token).and_then(Refused::repeats) else {
            return true;
        };
        let decision = row
            .get("decision")
            .and_then(Value::as_str)
            .unwrap_or_default();
        seen.insert((decision, token, Now::at(at, offset_minutes).of(span)))
    })
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use serde_json::{Value, json};

    use super::{Now, Quiet, counted_once};
    use crate::civil::{MS_PER_DAY, MS_PER_HOUR};
    use crate::jev::count;
    use crate::jev::door::{ANSWERED_OUTCOME, Refused};

    const ID: &str = "d-00000001";
    const VERSION: &str = "v1";

    fn now(at: i64) -> Now {
        Now::at(at, 0)
    }

    /// A moment well inside an hour, so `+ MS_PER_HOUR` is the next one.
    const AT: i64 = 20_000 * MS_PER_DAY + 10 * MS_PER_HOUR + 60_000;

    #[test]
    fn the_hours_number_and_its_name_agree() {
        for offset in [-570, -60, 0, 330, 540, 765] {
            for at in [AT, AT + MS_PER_HOUR - 1, AT - 60_000, -1, 0] {
                for other in [at + 1, at + 59 * 60_000, at + MS_PER_HOUR, at - MS_PER_HOUR] {
                    assert_eq!(
                        count::hour_of(at, offset) == count::hour_of(other, offset),
                        count::hour_number(at, offset) == count::hour_number(other, offset),
                        "{at} {other} {offset}"
                    );
                    assert_eq!(
                        count::day_of(at, offset) == count::day_of(other, offset),
                        count::day_number(at, offset) == count::day_number(other, offset),
                        "{at} {other} {offset}"
                    );
                }
            }
        }
    }

    #[test]
    fn a_full_hour_writes_once_and_the_next_hour_asks() {
        let quiet = Quiet::default();
        let folder = Path::new("/work");
        let full = Refused::HourlyBudget.token();
        assert!(quiet.refused((ID, VERSION), folder, full, now(AT), || true));
        for _ in 0..99 {
            assert!(quiet.is(ID, VERSION, Some(folder), now(AT)));
            assert!(!quiet.refused((ID, VERSION), folder, full, now(AT), || true));
        }
        assert!(
            !quiet.is(ID, VERSION, None, now(AT + MS_PER_HOUR)),
            "the next hour asks"
        );
        assert!(
            !quiet.is(ID, "v2", None, now(AT)),
            "a changed definition asks"
        );
        assert!(quiet.refused((ID, VERSION), folder, full, now(AT + MS_PER_HOUR), || true));
    }

    #[test]
    fn a_busy_lock_and_a_one_off_failure_write_each_time() {
        let quiet = Quiet::default();
        let folder = Path::new("/work");
        for _ in 0..3 {
            assert!(quiet.refused(
                (ID, VERSION),
                folder,
                Refused::HourlyBudget.token(),
                now(AT),
                || false
            ));
            assert!(quiet.refused((ID, VERSION), folder, "timeout", now(AT), || true));
            assert!(quiet.refused(
                (ID, VERSION),
                folder,
                Refused::Resting.token(),
                now(AT),
                || true
            ));
        }
        assert!(!quiet.is(ID, VERSION, Some(folder), now(AT)));
    }

    #[test]
    fn a_spent_day_holds_all_day() {
        let quiet = Quiet::default();
        let folder = Path::new("/work");
        assert!(quiet.refused(
            (ID, VERSION),
            folder,
            Refused::Budget.token(),
            now(AT),
            || false
        ));
        assert!(
            quiet.is(ID, VERSION, None, now(AT + MS_PER_HOUR)),
            "the next hour of the day"
        );
        assert!(
            !quiet.is(ID, VERSION, None, now(AT + MS_PER_DAY)),
            "the next day asks"
        );
    }

    #[test]
    fn a_folder_not_consented_is_quiet_there_alone() {
        let quiet = Quiet::default();
        let (shut, open) = (Path::new("/shut"), Path::new("/open"));
        let token = Refused::NotConsented.token();
        assert!(quiet.refused((ID, VERSION), shut, token, now(AT), || false));
        assert!(!quiet.refused((ID, VERSION), shut, token, now(AT), || false));
        assert!(quiet.is(ID, VERSION, Some(shut), now(AT)));
        assert!(!quiet.is(ID, VERSION, Some(open), now(AT)));
        assert!(!quiet.is(ID, VERSION, None, now(AT)));
    }

    #[test]
    fn a_broken_definition_writes_once_a_version() {
        let quiet = Quiet::default();
        assert!(quiet.broke((ID, VERSION), now(AT)));
        assert!(!quiet.broke((ID, VERSION), now(AT + MS_PER_DAY)));
        assert!(quiet.is(ID, VERSION, None, now(AT + 2 * MS_PER_DAY)));
        assert!(!quiet.is(ID, "v2", None, now(AT)));
    }

    #[test]
    fn a_past_span_is_dropped_when_the_next_mark_is_made() {
        let quiet = Quiet::default();
        let full = Refused::HourlyBudget.token();
        for folder in ["/a", "/b", "/c"] {
            let folder = Path::new(folder);
            assert!(quiet.refused(
                (ID, VERSION),
                folder,
                Refused::NotConsented.token(),
                now(AT),
                || false
            ));
        }
        assert!(quiet.refused(("d-2", VERSION), Path::new("/a"), full, now(AT), || true));
        assert!(quiet.refused(
            ("d-3", VERSION),
            Path::new("/a"),
            full,
            now(AT + MS_PER_HOUR),
            || true
        ));
        let marks = quiet.marks.lock().expect("the marks");
        assert_eq!(marks.everywhere.len(), 1, "{marks:?}");
        assert!(marks.in_folder.is_empty(), "{marks:?}");
    }

    fn row(decision: &str, at: i64, outcome: &str) -> Value {
        json!({"at": at, "decision": decision, "outcome": outcome, "requests": 0})
    }

    #[test]
    fn a_repeating_refusal_counts_once_a_span_and_answers_as_they_are() {
        let full = Refused::HourlyBudget.token();
        let mut rows: Vec<Value> = (0..57)
            .map(|minute| row(ID, AT + minute * 1_000, full))
            .collect();
        rows.extend((0..3).map(|second| row(ID, AT + MS_PER_HOUR + second, full)));
        rows.extend((0..2).map(|second| row(ID, AT + second, ANSWERED_OUTCOME)));
        rows.extend((0..2).map(|second| row(ID, AT + second, "timeout")));
        rows.extend((0..4).map(|hour| row(ID, AT + hour * MS_PER_HOUR, Refused::Budget.token())));
        let counted: Vec<&Value> = counted_once(&rows, i64::MIN, 0).collect();
        let of = |outcome: &str| {
            counted
                .iter()
                .filter(|row| row["outcome"] == outcome)
                .count()
        };
        assert_eq!(of(full), 2, "two hours");
        assert_eq!(of(ANSWERED_OUTCOME), 2);
        assert_eq!(of("timeout"), 2);
        assert_eq!(of(Refused::Budget.token()), 1, "one day");
        let later: Vec<&Value> = counted_once(&rows, AT + 30_000, 0).collect();
        assert_eq!(
            later.iter().filter(|row| row["outcome"] == full).count(),
            2,
            "an hour cut by the window counts from its first row inside it"
        );
    }
}
