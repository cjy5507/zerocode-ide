//! The Simulator's own window, read straight off the window server.
//!
//! Why this exists at all: the frame pump's other road asks CoreSimulator for
//! a still picture, and that request costs 130-230ms on this machine whatever
//! format it is asked for — jpeg 180, png 215, bmp 231, tiff 193 — so the
//! encode is not the cost and no choice of format escapes it. Five frames a
//! second is the ceiling of that road. Reading the window the Simulator is
//! already drawing costs 11-14ms for a 3008x1585 window (measured against
//! this application's own window, which is far larger than a phone), and a
//! phone-shaped one is smaller still.
//!
//! The original does neither: it runs a resident helper that owns the
//! simulator's framebuffer and serves it as MJPEG at up to thirty frames a
//! second (`MAX_FPS = 30`, src/main/emulator/mjpeg-frame-stream.ts), which is
//! why it can hide the Simulator application entirely. This road cannot —
//! measured: a hidden application's window is still LISTED, and capturing it
//! answers no image at all. So this is offered, never assumed, and the pump
//! keeps its still pictures for every machine where the window is not there.
//!
//! We now have that helper too (`emulator/ios_hid.rs`, 1.92ms a frame), which
//! settles the question this file used to answer the other way: the pane is no
//! longer worth a Simulator on the person's desktop, and a pane opening
//! [`hide_simulator`]s it instead. The road below survives because it is still
//! the cheap one for a machine whose helper cannot start AND whose person has
//! the Simulator open anyway — it is read when it is there, and asked for
//! never.

#[cfg(target_os = "macos")]
mod mac {
    use std::ffi::c_void;

    use objc2::AnyThread;
    use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep, NSRunningApplication};
    use objc2_core_foundation::{CFArray, CFDictionary, CFNumber, CFNumberType, CFString};
    use objc2_core_graphics::{
        CGRectNull, CGWindowImageOption, CGWindowListCopyWindowInfo, CGWindowListOption,
    };
    use objc2_foundation::{NSDictionary, NSString};

    /// The application that draws every iOS simulator screen on this machine.
    const SIMULATOR_OWNER: &str = "Simulator";

    /// One window, as the window server describes it.
    ///
    /// Kept as plain data so the rule that picks one is a pure function with
    /// a test, rather than something only a booted device can exercise.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct WindowRow {
        pub owner: String,
        pub name: String,
        pub number: u32,
        /// 0 is the ordinary document layer. Menus, tooltips and the status
        /// bar live above it and are never the screen we mean.
        pub layer: i64,
        pub onscreen: bool,
    }

    /// Which listed window is this device's screen.
    ///
    /// The window server names a simulator window by the DEVICE's own name and
    /// nothing else — measured: AppleScript reports `"<device> – iOS 26.5"` while
    /// `kCGWindowName` gives `"<device>"`. Both spellings are accepted here so
    /// the answer does not depend on which of them a future macOS hands over.
    ///
    /// A window that is not on screen is refused rather than returned, because
    /// capturing one answers nothing: a hidden application keeps its window in
    /// the list and gives no picture for it.
    pub fn pick_window(rows: &[WindowRow], device: &str) -> Option<u32> {
        let device = device.trim();
        if device.is_empty() {
            return None;
        }
        rows.iter()
            .filter(|row| row.owner == SIMULATOR_OWNER && row.layer == 0 && row.onscreen)
            .find(|row| {
                let name = row.name.trim();
                name == device || name.starts_with(&format!("{device} "))
            })
            .map(|row| row.number)
    }

    unsafe fn said(row: *const CFDictionary, key: &str) -> Option<String> {
        let key = CFString::from_str(key);
        let value =
            unsafe { CFDictionary::value(&*row, (&*key as *const CFString).cast::<c_void>()) };
        if value.is_null() {
            return None;
        }
        Some(unsafe { &*value.cast::<CFString>() }.to_string())
    }

    unsafe fn number(row: *const CFDictionary, key: &str) -> Option<i64> {
        let key = CFString::from_str(key);
        let value =
            unsafe { CFDictionary::value(&*row, (&*key as *const CFString).cast::<c_void>()) };
        if value.is_null() {
            return None;
        }
        let mut held: i64 = 0;
        let got = unsafe {
            CFNumber::value(
                &*value.cast::<CFNumber>(),
                CFNumberType::SInt64Type,
                (&raw mut held).cast::<c_void>(),
            )
        };
        got.then_some(held)
    }

    /// Every window this process is allowed to know about.
    ///
    /// `OptionAll` rather than on-screen only, because the on-screen answer is
    /// what `pick_window` decides from `kCGWindowIsOnscreen` — asking the list
    /// to pre-filter would hide the difference between "no such window" and
    /// "that window is hidden", and those two want different words.
    pub fn windows() -> Vec<WindowRow> {
        let Some(listed) = CGWindowListCopyWindowInfo(
            CGWindowListOption::OptionAll | CGWindowListOption::ExcludeDesktopElements,
            0,
        ) else {
            return Vec::new();
        };
        let mut rows = Vec::new();
        for at in 0..CFArray::count(&listed) {
            let row = unsafe { CFArray::value_at_index(&listed, at) }.cast::<CFDictionary>();
            if row.is_null() {
                continue;
            }
            let Some(numbered) = (unsafe { number(row, "kCGWindowNumber") }) else {
                continue;
            };
            rows.push(WindowRow {
                owner: unsafe { said(row, "kCGWindowOwnerName") }.unwrap_or_default(),
                name: unsafe { said(row, "kCGWindowName") }.unwrap_or_default(),
                number: numbered as u32,
                layer: unsafe { number(row, "kCGWindowLayer") }.unwrap_or(-1),
                onscreen: unsafe { number(row, "kCGWindowIsOnscreen") }.unwrap_or(0) == 1,
            });
        }
        rows
    }

    /// Apple's own identifier for the application that draws simulators.
    /// Not a guess and not ours to choose — it is the string Launch Services
    /// answers to, and the same application the original's own launch shim
    /// names (`open -gj -a Simulator`).
    const SIMULATOR_BUNDLE_ID: &str = "com.apple.iphonesimulator";

    /// Put the Simulator back out of sight.
    ///
    /// The opposite of what this file used to offer, and for the reason the
    /// header now records: the helper reads the device's framebuffer with no
    /// window at all, so a pane that opens has nothing to gain from a
    /// Simulator on the desktop and the person has a second application to
    /// close for nothing.
    ///
    /// `simctl boot` is what draws that window — the Simulator opens one for
    /// any device that boots under it, whoever asked — so the launch shim's
    /// `-j` cannot cover this on its own: it only keeps an application we
    /// STARTED hidden, and says nothing about one the person already had
    /// open. Hiding is therefore said here, after the boot, rather than
    /// wished for at the launch.
    ///
    /// Hidden rather than closed, and never activated: the Simulator may be
    /// holding other devices for the person, and `open_mobile_emulator` — the
    /// "Open in Simulator" button, the one door where showing it is what was
    /// asked for — brings all of them back with one click.
    ///
    /// `hide` answers a BOOL that is measured unreliable here (false while the
    /// application does go away), so nothing reads it.
    pub fn hide_simulator() {
        if let Some(running) = simulator_application()
            && !running.isHidden()
        {
            let _ = running.hide();
        }
    }

    fn simulator_application() -> Option<objc2::rc::Retained<NSRunningApplication>> {
        let id = NSString::from_str(SIMULATOR_BUNDLE_ID);
        NSRunningApplication::runningApplicationsWithBundleIdentifier(&id).firstObject()
    }

    /// This device's screen as JPEG bytes, or nothing at all.
    ///
    /// Nothing means the window went away, the application was hidden, or this
    /// machine has not granted screen recording — three states that all read
    /// the same from here and that all want the same answer from the caller,
    /// which is to go back to asking CoreSimulator.
    ///
    /// `NominalResolution` asks for the window in POINTS rather than backing
    /// pixels: the pane it lands in is a few hundred points wide, so the
    /// retina copy would be four times the bytes for a picture nobody can see
    /// the extra detail in.
    // `CGWindowListCreateImage` is deprecated in favour of ScreenCaptureKit,
    // which is an asynchronous stream with a delegate, an entitlement and a
    // frame callback — a shape this pump has no use for, since it already owns
    // a thread and wants ONE picture when it asks. The call still works, it is
    // the only synchronous way to read a window, and the day it stops the
    // still-picture road below is already there to catch it.
    ///
    /// The body runs inside an autorelease pool because this is called from the
    /// pump's own thread, up to thirty times a second, and
    /// `representationUsingType_properties` is outside the alloc/init/new/copy
    /// rule — so the `NSData` it answers is autoreleased. A thread that never
    /// drains a pool holds every one of those until the thread itself ends,
    /// which for this thread means until the pane closes: tens of kilobytes a
    /// frame, at thirty frames a second, with nothing asking for them.
    #[allow(deprecated)]
    pub fn capture_jpeg(window: u32) -> Option<Vec<u8>> {
        objc2::rc::autoreleasepool(|_| {
            let image = unsafe {
                objc2_core_graphics::CGWindowListCreateImage(
                    CGRectNull,
                    CGWindowListOption::OptionIncludingWindow,
                    window,
                    CGWindowImageOption::BoundsIgnoreFraming
                        | CGWindowImageOption::NominalResolution,
                )
            }?;
            // The same encoder the browser pane's snapshot already goes through —
            // AppKit's bitmap rep, which takes a CGImage as it stands and writes
            // whatever file type is asked for. A second encoder would be a second
            // set of colour-space decisions for one picture.
            let rep = NSBitmapImageRep::initWithCGImage(NSBitmapImageRep::alloc(), &image);
            let bytes = unsafe {
                rep.representationUsingType_properties(
                    NSBitmapImageFileType::JPEG,
                    &NSDictionary::new(),
                )
            }?;
            // `to_vec` copies out before the pool drains — the bytes leaving this
            // function are ours, not the pool's.
            Some(bytes.to_vec())
        })
    }

    #[cfg(test)]
    mod tests {
        use super::{WindowRow, pick_window};

        fn row(owner: &str, name: &str, number: u32, layer: i64, onscreen: bool) -> WindowRow {
            WindowRow {
                owner: owner.to_string(),
                name: name.to_string(),
                number,
                layer,
                onscreen,
            }
        }

        /// The device's own name is the window's name, and both spellings the
        /// system has been seen to use answer to it.
        #[test]
        fn a_device_is_found_by_the_name_its_window_wears() {
            let rows = vec![
                row("Finder", "BeatSweep", 1, 0, true),
                row("Simulator", "Other Phone", 2, 0, true),
                row("Simulator", "BeatSweep iPhone Air Layout QA", 3, 0, true),
            ];
            assert_eq!(
                pick_window(&rows, "BeatSweep iPhone Air Layout QA"),
                Some(3)
            );
            // The AppleScript spelling, with the runtime after the name.
            let suffixed = vec![row(
                "Simulator",
                "BeatSweep iPhone Air Layout QA – iOS 26.5",
                4,
                0,
                true,
            )];
            assert_eq!(
                pick_window(&suffixed, "BeatSweep iPhone Air Layout QA"),
                Some(4)
            );
        }

        /// A hidden application keeps its window in the list and gives no
        /// picture for it — measured, and the reason the pump keeps its other
        /// road. A menu or tooltip is not a device screen either.
        #[test]
        fn a_window_that_could_not_be_captured_is_not_offered() {
            let hidden = vec![row("Simulator", "BeatSweep", 5, 0, false)];
            assert_eq!(pick_window(&hidden, "BeatSweep"), None);
            let floating = vec![row("Simulator", "BeatSweep", 6, 25, true)];
            assert_eq!(pick_window(&floating, "BeatSweep"), None);
            assert_eq!(pick_window(&[], "BeatSweep"), None);
        }

        /// A device with no name matches nothing rather than the first window
        /// on the desktop.
        #[test]
        fn a_nameless_device_matches_nothing() {
            let rows = vec![row("Simulator", "BeatSweep", 7, 0, true)];
            assert_eq!(pick_window(&rows, "   "), None);
        }
    }
}

#[cfg(target_os = "macos")]
pub use mac::{capture_jpeg, hide_simulator, pick_window, windows};

/// Off macOS there is no Simulator and no window to read: the caller keeps
/// whatever road it already had, and says so in the same words either way.
#[cfg(not(target_os = "macos"))]
pub fn pick_window(_rows: &[()], _device: &str) -> Option<u32> {
    None
}

#[cfg(not(target_os = "macos"))]
#[must_use]
pub fn windows() -> Vec<()> {
    Vec::new()
}

#[cfg(not(target_os = "macos"))]
pub fn capture_jpeg(_window: u32) -> Option<Vec<u8>> {
    None
}

#[cfg(not(target_os = "macos"))]
pub fn hide_simulator() {}
