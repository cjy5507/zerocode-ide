//! Every road out of this window, by name (t-6428).
//!
//! Leaving the window cuts what runs in its panes — a worker's turn, the
//! gate it left running in the background — so each road is named where it
//! begins, and the goodbye says which one it was
//! (`orchestration::window_exiting`). Measured before this module existed,
//! over 37 hours of the window's own log (2026-09-22 12:13 – 09-24 01:32):
//! nine exits with workers seated, twenty sleeping panes, and every one of
//! the nine the main window closing — `window destroyed: main`, then tauri's
//! `ExitRequested` without a code — none of them through a restart button.

use std::fmt;
use std::sync::Mutex;

/// A road out of this window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExitRoad {
    /// A restart button (`relaunch_window`), by the door it stands in when
    /// the window named one.
    Restart(Option<RestartDoor>),
    /// The last window closed — its red button, ⌘W. tauri says so only
    /// once the window is gone (`ExitRequested` without a code).
    Close,
    /// `terminate:` — ⌘Q, the Dock's Quit, a logout or a shutdown. The app
    /// hears it only as it ends (`RunEvent::Exit`), so nothing can ask first.
    Terminate,
    /// The menu-bar icon's Quit.
    Tray,
    /// The window's own `app.exit(code)`, with no road named before it.
    App,
}

/// The four restart buttons: the lane's 「새 빌드 준비됨」 toast (t-3005),
/// the update feed's install (t-3191), the settings notice beside them, and
/// the window material's relaunch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RestartDoor {
    UpdateToast,
    UpdateInstall,
    SettingsNotice,
    WindowMaterial,
}

/// Each door's word, in one table: the window names its door with it, and
/// the goodbye's line says it back.
const DOORS: [(RestartDoor, &str); 4] = [
    (RestartDoor::UpdateToast, "update-toast"),
    (RestartDoor::UpdateInstall, "update-install"),
    (RestartDoor::SettingsNotice, "settings-notice"),
    (RestartDoor::WindowMaterial, "window-material"),
];

impl RestartDoor {
    /// The door a word names, or `None` for a word no door wears.
    pub(crate) fn named(word: &str) -> Option<Self> {
        DOORS
            .iter()
            .find(|(_, held)| *held == word)
            .map(|(door, _)| *door)
    }

    fn word(self) -> &'static str {
        DOORS
            .iter()
            .find(|(door, _)| *door == self)
            .map_or("", |(_, word)| word)
    }
}

impl fmt::Display for ExitRoad {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Restart(Some(door)) => write!(out, "restart:{}", door.word()),
            Self::Restart(None) => out.write_str("restart"),
            Self::Close => out.write_str("close"),
            Self::Terminate => out.write_str("terminate"),
            Self::Tray => out.write_str("tray"),
            Self::App => out.write_str("app"),
        }
    }
}

/// The road this window is leaving by, once one is named.
static LEAVING: Mutex<Option<ExitRoad>> = Mutex::new(None);

/// Name the road this window is leaving by, and answer the road it is.
///
/// The first road named is the road. A close is followed by the window's
/// own `app.exit(0)` once the embedded browser has shut down, and then by
/// tauri's `Exit`: all three reach the goodbye, and the person closed the
/// window.
pub(crate) fn begin(road: ExitRoad) -> ExitRoad {
    first(
        &mut LEAVING.lock().unwrap_or_else(|held| held.into_inner()),
        road,
    )
}

fn first(held: &mut Option<ExitRoad>, road: ExitRoad) -> ExitRoad {
    *held.get_or_insert(road)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_road_named_is_the_road_the_window_left_by() {
        let mut held = None;
        assert_eq!(first(&mut held, ExitRoad::Close), ExitRoad::Close);
        // The browser's own `app.exit(0)` and tauri's `Exit` come after.
        assert_eq!(first(&mut held, ExitRoad::App), ExitRoad::Close);
        assert_eq!(first(&mut held, ExitRoad::Terminate), ExitRoad::Close);
    }

    #[test]
    fn every_door_is_named_by_one_word_and_said_back_by_it() {
        for (door, word) in DOORS {
            assert_eq!(RestartDoor::named(word), Some(door));
            assert_eq!(
                ExitRoad::Restart(Some(door)).to_string(),
                format!("restart:{word}")
            );
        }
        assert_eq!(RestartDoor::named("a-door-nobody-built"), None);
        assert_eq!(ExitRoad::Restart(None).to_string(), "restart");
        for (road, word) in [
            (ExitRoad::Close, "close"),
            (ExitRoad::Terminate, "terminate"),
            (ExitRoad::Tray, "tray"),
            (ExitRoad::App, "app"),
        ] {
            assert_eq!(road.to_string(), word);
        }
    }
}
