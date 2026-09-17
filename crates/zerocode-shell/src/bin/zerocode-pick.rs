//! The native file panel, in a process of its own.
//!
//! The window never opens `NSOpenPanel` again. The panel is a remote view:
//! the thread that opens it waits, synchronously, for AppKit's
//! `openAndSavePanelService` to come up and connect, and on macOS 26 that
//! service raises an AutoFill shield on top. Opened on the window's main
//! thread, that wait was the window not painting and not hearing clicks —
//! three force-quits on one road (t-2488 2026-09-05 02:49; 09-06 17:15;
//! 09-07 11:38). Opened here, the same wait is this process's, and the
//! window stays free to paint, to say the panel is late, to bring this
//! process forward, and to kill it.
//!
//! Contract: [`zerocode_core::pick`] — the request on `argv`, the answer on
//! stdout one path per line, nothing at all for a dismissal, exit 0 either
//! way. Exit 2 is an argument the helper did not understand; exit 1 is a
//! panel it could not open. Bundled beside the window's executable, where
//! `zerocode-mirror` already lives.

#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use std::io::Write as _;
use std::process::ExitCode;

use zerocode_core::pick::{PickKind, PickRequest, format_answer};

fn main() -> ExitCode {
    let request = match PickRequest::parse_argv(std::env::args_os().skip(1)) {
        Ok(request) => request,
        Err(said) => {
            eprintln!("zerocode-pick: {said}");
            eprintln!(
                "usage: zerocode-pick --kind folder|folders|file|files [--start <dir>] \
                 [--title <words>] [--filter <name>=<ext>,<ext>]..."
            );
            return ExitCode::from(2);
        }
    };
    come_forward();
    let answer = match ask(&request) {
        Ok(paths) => paths,
        Err(said) => {
            eprintln!("zerocode-pick: {said}");
            return ExitCode::from(1);
        }
    };
    let mut stdout = std::io::stdout().lock();
    if stdout
        .write_all(&answer)
        .and_then(|()| stdout.flush())
        .is_err()
    {
        return ExitCode::from(1);
    }
    ExitCode::SUCCESS
}

/// The panel, blocking this thread — which is the point. `Ok(empty)` is a
/// dismissal.
fn ask(request: &PickRequest) -> Result<Vec<u8>, String> {
    let mut dialog = rfd::FileDialog::new();
    if let Some(start) = &request.start {
        dialog = dialog.set_directory(start);
    }
    if let Some(title) = &request.title {
        dialog = dialog.set_title(title);
    }
    for filter in &request.filters {
        let extensions: Vec<&str> = filter.extensions.iter().map(String::as_str).collect();
        dialog = dialog.add_filter(&filter.name, &extensions);
    }
    let paths = match request.kind {
        PickKind::Folder => dialog.pick_folder().into_iter().collect(),
        PickKind::Folders => dialog.pick_folders().unwrap_or_default(),
        PickKind::File => dialog.pick_file().into_iter().collect(),
        PickKind::Files => dialog.pick_files().unwrap_or_default(),
    };
    format_answer(&paths)
}

/// A bare process has no window to attach the panel to and no Dock tile to
/// click, so it asks to be the active application before the panel opens —
/// otherwise the panel comes up BEHIND the window that asked for it, which
/// is the shape of "hang" this whole road exists to end. Accessory policy:
/// active and key, without a Dock tile that bounces for a file panel.
#[cfg(target_os = "macos")]
fn come_forward() {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};

    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    #[allow(deprecated)]
    app.activateIgnoringOtherApps(true);
}

#[cfg(not(target_os = "macos"))]
fn come_forward() {}
