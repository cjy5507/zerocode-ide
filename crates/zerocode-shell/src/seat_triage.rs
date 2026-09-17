//! The roads that turn a pending item into a ledger task through the seat
//! (`orchestration::file_task_through_seat`) — a crash incident
//! (`crash_triage`), a failing QA verdict (`qa_triage`), a scoreboard finding
//! (`scoreboard_inbox`) — hear the same answers and keep the same boot
//! memory, so both live here once; each road says only what its item is, how
//! it is stamped, and what its lines and its event are called.
//!
//! A beat asks the door about one item and sorts the answer by what the road
//! does next: nothing to ask; wait a beat for a seat; stop asking for this
//! boot, because the ledger is degraded — decided once by
//! `orchestration::open` before the first beat and never repaired inside a
//! boot; or count a refusal. An item refused past the table is set aside for
//! the rest of the boot with its reason, and the items behind it are still
//! asked about. The request name is the item's own, so every ask is the same
//! request and a lost answer is never a second task.
//!
//! Three sentences reach `window-errors.log`, one line each: filed, degraded,
//! set aside. Waiting for a seat is silent — it is the ordinary state for the
//! first seconds of every boot — and so is a refusal under the table.

use std::collections::BTreeMap;

use tauri::{AppHandle, Emitter as _, Manager as _};

use crate::agent_teams::Host;
use crate::orchestration::SeatFiling;

/// How many refusals one boot spends on one item before setting it aside.
/// The bound is on how long a refusing ledger is asked, not on how many tasks
/// could result.
pub(crate) const REFUSALS_PER_BOOT: u32 = 3;

/// What one beat heard about a road's oldest pending item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Triage {
    /// Nothing wants filing; the door was not asked.
    Nothing,
    /// No leader seated in a run sits in this window yet — nothing was
    /// presented and nothing was stamped. Ask next beat.
    Waiting(String),
    /// The ledger is degraded for this whole boot; the next boot asks again.
    Unavailable(String),
    /// The door answered no, or it cut the task and the stamp was not written.
    Refused { item: String, why: String },
    /// Filed and stamped.
    Filed { item: String, task: String },
    /// A task still open already carries the item: the stamp names it and
    /// nothing new was filed.
    Open { item: String, task: String },
}

/// One item through the door: what the door answered, then the road's stamp
/// of the task it gave (`stamp(item, task)`).
pub(crate) fn triage(
    item: String,
    answered: Result<String, SeatFiling>,
    stamp: impl FnOnce(&str, &str) -> Result<(), String>,
) -> Triage {
    let task = match answered {
        Ok(task) => task,
        Err(SeatFiling::Waiting(why)) => return Triage::Waiting(why),
        Err(SeatFiling::Unavailable(why)) => return Triage::Unavailable(why),
        Err(SeatFiling::Refused(why)) => return Triage::Refused { item, why },
    };
    match stamp(&item, &task) {
        Ok(()) => Triage::Filed { item, task },
        // The task exists; only the stamp is missing. The next ask is the
        // same request name and the ledger answers the same task, so a
        // missing stamp costs a repeated answer, never a second task.
        Err(why) => Triage::Refused {
            item,
            why: format!("filed as {task} but the stamp could not be written: {why}"),
        },
    }
}

/// The ledger as a road sees it.
pub(crate) trait Door {
    /// File one `task-create` verb through the seat: the task id the ledger
    /// gave.
    fn file(&self, argv: &[String]) -> Result<String, SeatFiling>;
    /// A task still open whose title carries this marker, if the ledger has
    /// one.
    fn open_task(&self, title_mark: &str) -> Option<String>;
}

/// This window's ledger: filing through the newest seated coordinator, open
/// tasks read off the window's own view.
pub(crate) struct SeatDoor<'a> {
    pub host: &'a dyn Host,
    pub overrides: &'a [(String, zerocode_core::launch::LaunchOverride)],
    pub now_ms: i64,
}

impl Door for SeatDoor<'_> {
    fn file(&self, argv: &[String]) -> Result<String, SeatFiling> {
        crate::orchestration::file_task_through_seat(self.host, self.overrides, argv, self.now_ms)
    }

    fn open_task(&self, title_mark: &str) -> Option<String> {
        crate::orchestration::open_task_titled(title_mark)
    }
}

/// A road's words: `name` opens every line it writes to `window-errors.log`,
/// `item` comes before an item's id there ("report ", "run ", or nothing for
/// a key that reads on its own), and `event` is what the open window hears
/// when an item is filed, naming it under `id_field`.
pub(crate) struct Road {
    pub name: &'static str,
    pub item: &'static str,
    pub event: &'static str,
    pub id_field: &'static str,
}

/// One boot's memory of a road.
#[derive(Debug, Default)]
pub(crate) struct Sweep {
    /// Refusals spent per item since it was last filed or set aside.
    refusals: BTreeMap<String, u32>,
    /// Items set aside for the rest of this boot, with the refusal that did it.
    set_aside: BTreeMap<String, String>,
    /// The door said the ledger is degraded: said once, and this boot asks
    /// nothing more.
    closed: bool,
}

impl Sweep {
    pub(crate) const fn new() -> Self {
        Self {
            refusals: BTreeMap::new(),
            set_aside: BTreeMap::new(),
            closed: false,
        }
    }

    /// Whether this boot still asks the door at all.
    pub(crate) fn asks(&self) -> bool {
        !self.closed
    }

    /// The items this boot no longer asks about, each with the refusal that
    /// set it aside.
    pub(crate) fn set_aside(&self) -> &BTreeMap<String, String> {
        &self.set_aside
    }

    /// What one beat does with what the door said: the line to log, if any.
    pub(crate) fn step(&mut self, said: &Triage, road: &Road) -> Option<String> {
        match said {
            Triage::Nothing | Triage::Waiting(_) => None,
            Triage::Unavailable(why) => (!std::mem::replace(&mut self.closed, true)).then(|| {
                format!(
                    "{}: not filed — {why} — the next boot asks again",
                    road.name
                )
            }),
            Triage::Refused { item, why } => {
                let count = self.refusals.entry(item.clone()).or_insert(0);
                *count += 1;
                if *count < REFUSALS_PER_BOOT {
                    return None;
                }
                let count = *count;
                self.refusals.remove(item);
                self.set_aside.insert(item.clone(), why.clone());
                Some(format!(
                    "{}: {}{item} not filed — refused {count} times ({why}) — set aside until the next boot",
                    road.name, road.item
                ))
            }
            Triage::Filed { item, task } => {
                self.refusals.remove(item);
                Some(format!(
                    "{}: {}{item} filed as {task}",
                    road.name, road.item
                ))
            }
            // Not news for the log: the ledger already shows the open task.
            Triage::Open { item, .. } => {
                self.refusals.remove(item);
                None
            }
        }
    }

    /// One beat over this memory: nothing once the boot has stopped asking;
    /// otherwise what `ask` heard (it is handed the items set aside, so the
    /// oldest one left is the one asked about) and the line to log.
    pub(crate) fn beat(
        &mut self,
        road: &Road,
        ask: impl FnOnce(&BTreeMap<String, String>) -> Triage,
    ) -> (Triage, Option<String>) {
        if self.closed {
            return (Triage::Nothing, None);
        }
        let said = ask(&self.set_aside);
        let line = self.step(&said, road);
        (said, line)
    }
}

/// Say what one beat did: its line to `window-errors.log`, and a filed item to
/// the open window.
pub(crate) fn tell(app: &AppHandle, road: &Road, said: &Triage, line: Option<&str>) {
    if let Some(line) = line {
        crate::note_window_event(app.state::<crate::AppState>().local_data_root(), line);
    }
    if let Triage::Filed { item, task } = said {
        let mut payload = serde_json::Map::new();
        payload.insert(road.id_field.to_string(), item.as_str().into());
        payload.insert("taskId".to_string(), task.as_str().into());
        let _ = app.emit_to(crate::MAIN_WINDOW_LABEL, road.event, payload);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROAD: Road = Road {
        name: "test task",
        item: "item ",
        event: "test:filed",
        id_field: "itemId",
    };

    fn refused(item: &str) -> Triage {
        Triage::Refused {
            item: item.to_string(),
            why: "already answered".to_string(),
        }
    }

    #[test]
    fn the_doors_answer_and_the_stamp_decide_what_was_heard() {
        let stamped = |_: &str, _: &str| Ok(());
        assert_eq!(
            triage("a".into(), Ok("t-1".into()), stamped),
            Triage::Filed {
                item: "a".into(),
                task: "t-1".into()
            }
        );
        assert_eq!(
            triage("a".into(), Ok("t-2".into()), |item, task| {
                assert_eq!((item, task), ("a", "t-2"), "the stamp names both");
                Err("disk full".to_string())
            }),
            Triage::Refused {
                item: "a".into(),
                why: "filed as t-2 but the stamp could not be written: disk full".into()
            },
            "a task without its stamp is asked again under the same request name"
        );
        let never = |_: &str, _: &str| -> Result<(), String> { panic!("nothing to stamp") };
        assert_eq!(
            triage(
                "a".into(),
                Err(SeatFiling::Waiting("no seat".into())),
                never
            ),
            Triage::Waiting("no seat".into())
        );
        assert_eq!(
            triage(
                "a".into(),
                Err(SeatFiling::Unavailable("gone".into())),
                never
            ),
            Triage::Unavailable("gone".into())
        );
        assert_eq!(
            triage("a".into(), Err(SeatFiling::Refused("no".into())), never),
            Triage::Refused {
                item: "a".into(),
                why: "no".into()
            }
        );
    }

    #[test]
    fn a_degraded_ledger_is_one_line_and_this_boot_asks_nothing_more() {
        let mut sweep = Sweep::new();
        let mut asked = 0;
        let (_, line) = sweep.beat(&ROAD, |_| {
            asked += 1;
            Triage::Unavailable("the ledger is gone".into())
        });
        assert_eq!(
            line.as_deref(),
            Some("test task: not filed — the ledger is gone — the next boot asks again")
        );
        assert!(!sweep.asks());
        for _ in 0..5 {
            let (said, line) = sweep.beat(&ROAD, |_| {
                asked += 1;
                Triage::Unavailable("the ledger is gone".into())
            });
            assert_eq!((said, line), (Triage::Nothing, None));
        }
        assert_eq!(asked, 1, "the door is not asked again this boot");
        assert_eq!(
            sweep.step(&Triage::Unavailable("gone".into()), &ROAD),
            None,
            "and a repeat is never a second line"
        );
    }

    #[test]
    fn refusals_are_counted_per_item_and_the_third_sets_it_aside_with_its_reason() {
        let mut sweep = Sweep::new();
        for _ in 1..REFUSALS_PER_BOOT {
            assert_eq!(sweep.step(&refused("disk"), &ROAD), None);
        }
        assert_eq!(
            sweep.step(&refused("flakes"), &ROAD),
            None,
            "counted per item"
        );
        assert_eq!(
            sweep.step(&refused("disk"), &ROAD).as_deref(),
            Some(
                "test task: item disk not filed — refused 3 times (already answered) — set aside until the next boot"
            )
        );
        assert_eq!(
            sweep.set_aside().get("disk").map(String::as_str),
            Some("already answered")
        );
        assert!(!sweep.set_aside().contains_key("flakes"));
        assert!(sweep.asks(), "one item set aside is not the road stopped");
        let (said, _) = sweep.beat(&ROAD, |set_aside| {
            assert!(set_aside.contains_key("disk"), "the ask skips it");
            Triage::Nothing
        });
        assert_eq!(said, Triage::Nothing);
    }

    #[test]
    fn a_filed_or_open_item_starts_its_refusal_count_over() {
        let mut sweep = Sweep::new();
        sweep.step(&refused("disk"), &ROAD);
        sweep.step(&refused("disk"), &ROAD);
        assert_eq!(
            sweep
                .step(
                    &Triage::Filed {
                        item: "disk".into(),
                        task: "t-1".into()
                    },
                    &ROAD
                )
                .as_deref(),
            Some("test task: item disk filed as t-1")
        );
        sweep.step(&refused("disk"), &ROAD);
        sweep.step(&refused("disk"), &ROAD);
        assert_eq!(
            sweep.step(
                &Triage::Open {
                    item: "disk".into(),
                    task: "t-1".into()
                },
                &ROAD
            ),
            None,
            "an open task is not news for the log"
        );
        sweep.step(&refused("disk"), &ROAD);
        sweep.step(&refused("disk"), &ROAD);
        assert!(sweep.set_aside().is_empty(), "{sweep:?}");
    }

    #[test]
    fn waiting_for_a_seat_and_nothing_pending_are_silent_and_keep_asking() {
        let mut sweep = Sweep::new();
        assert_eq!(sweep.step(&Triage::Waiting("no seat".into()), &ROAD), None);
        assert_eq!(sweep.step(&Triage::Nothing, &ROAD), None);
        assert!(sweep.asks());
    }
}
