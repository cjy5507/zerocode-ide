//! The crash that becomes a task, on the beat that already exists (t-3014 §2.4).
//!
//! The boot report consumes the incident and shows the sheet; this module
//! files that incident as a ledger task through the one argv door
//! ([`crate::orchestration::file_crash_task`]) and tells the open sheet by
//! event. It cannot file at the boot site itself: a `task-create` lands in
//! the run its caller stands in, and at `boot_report` no leader pane has
//! been cut yet — the coordinator tab mounts seconds later. So the boot
//! site ARMS the sweep after consuming, and the standing-order beat carries
//! it until a seated leader appears, the ledger answers, or this boot gives
//! up and leaves the next one to ask again.
//!
//! What the door says, how often a refusal is asked again, and the one line
//! each outcome writes are the three seat roads' shared vocabulary
//! (`seat_triage`); this road adds only the arming, and that one incident is
//! the whole of its work for a boot.

use std::sync::Mutex;

use tauri::{AppHandle, Manager as _};

use crate::agent_teams::Host;
use crate::seat_triage::{Road, Sweep, Triage, tell};

const ROAD: Road = Road {
    name: "crash task",
    item: "report ",
    event: "crash:triaged",
    id_field: "reportId",
};

#[derive(Debug, Default)]
struct Boot {
    /// The boot site consumed an incident (or found none) and asked for a
    /// sweep. Before that, `presented.json` still names the LAST boot's
    /// incident and a sweep would judge the wrong one.
    armed: bool,
    /// This boot is done with the incident: filed, or nothing to file.
    settled: bool,
    /// The incident's refusals, and whether the ledger is degraded.
    sweep: Sweep,
}

impl Boot {
    const fn new() -> Self {
        Self {
            armed: false,
            settled: false,
            sweep: Sweep::new(),
        }
    }

    /// Whether this beat asks the door: armed, the incident neither settled
    /// nor set aside, and the ledger not degraded.
    fn asks(&self) -> bool {
        self.armed && !self.settled && self.sweep.asks() && self.sweep.set_aside().is_empty()
    }

    /// What one beat does with what the door said: the one line to log.
    fn step(&mut self, said: &Triage) -> Option<String> {
        self.settled |= matches!(said, Triage::Nothing | Triage::Filed { .. });
        self.sweep.step(said, &ROAD)
    }
}

static BOOT: Mutex<Boot> = Mutex::new(Boot::new());

/// The boot site, after `crash::consume`: the incident the sheet is about
/// to show is the one the beat files. Re-arming on a webview reload is
/// harmless — a stamped incident answers `Nothing` on the first beat.
pub(crate) fn arm() {
    *BOOT.lock().unwrap_or_else(|held| held.into_inner()) = Boot {
        armed: true,
        ..Boot::new()
    };
}

/// One beat: ask the door once if this boot is armed and not yet done.
pub(crate) fn sweep(
    app: &AppHandle,
    host: &dyn Host,
    overrides: &[(String, zerocode_core::launch::LaunchOverride)],
    now_ms: i64,
) {
    if !BOOT.lock().unwrap_or_else(|held| held.into_inner()).asks() {
        return;
    }
    let root = app
        .state::<crate::AppState>()
        .local_data_root()
        .to_path_buf();
    let said = crate::orchestration::file_crash_task(host, overrides, &root, now_ms);
    let line = BOOT
        .lock()
        .unwrap_or_else(|held| held.into_inner())
        .step(&said);
    tell(app, &ROAD, &said, line.as_deref());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::seat_triage::REFUSALS_PER_BOOT;

    fn armed() -> Boot {
        Boot {
            armed: true,
            ..Boot::new()
        }
    }

    #[test]
    fn a_boot_asks_until_filed_or_settled_and_logs_one_line_per_outcome() {
        assert!(
            !Boot::new().asks(),
            "a boot the site never armed asks nothing"
        );

        let mut boot = armed();
        assert_eq!(boot.step(&Triage::Waiting("no seat".into())), None);
        assert!(boot.asks(), "waiting for a seat is not the end of a boot");
        let refused = Triage::Refused {
            item: "r".into(),
            why: "no run in use".into(),
        };
        for _ in 1..REFUSALS_PER_BOOT {
            assert_eq!(boot.step(&refused), None);
            assert!(boot.asks());
        }
        let said = boot.step(&refused).expect("the last refusal is said");
        assert_eq!(
            said,
            "crash task: report r not filed — refused 3 times (no run in use) — set aside until the next boot"
        );
        assert!(!boot.asks());

        let mut boot = armed();
        let said = boot.step(&Triage::Filed {
            item: "r".into(),
            task: "t-7".into(),
        });
        assert_eq!(said.as_deref(), Some("crash task: report r filed as t-7"));
        assert!(!boot.asks());

        let mut boot = armed();
        let said = boot
            .step(&Triage::Unavailable("the ledger is gone".into()))
            .expect("a degraded ledger is said");
        assert!(
            said.starts_with("crash task: not filed — the ledger is gone"),
            "{said}"
        );
        assert!(!boot.asks());

        let mut boot = armed();
        assert_eq!(boot.step(&Triage::Nothing), None);
        assert!(!boot.asks());
    }
}
