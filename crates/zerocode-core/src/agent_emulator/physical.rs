//! The devices a person can have connected that this window cannot drive
//! (t-36920): a real iPhone or iPad that Xcode's `devicectl` lists, a real
//! Android phone that `adb` lists.
//!
//! `zerocode-emulator list` answered simulators and emulators only, so an agent
//! that had found the road still could not tell "nothing is connected" from
//! "this window cannot drive that device". This module is the pure half of the
//! answer that tells them apart: the words (keys, states, reasons, the row
//! cap), the two readers over what the tools print, and the one function that
//! puts the platforms' readings into the `list` answer. Running the tools —
//! with a deadline and a short cache — is the window's (`emulator::physical`),
//! because that needs processes and a clock; nothing here does.
//!
//! A real device is never drivable here: the window's input roads are the
//! simulator's and the emulator's (`emulator::android` keeps `emulator-`
//! serials only, and says why in `device_presence`), so every row carries
//! `drivable: false` and the reason in plain words.

use serde::Serialize;
use serde_json::{Value, json};

use crate::computer_use::EmulatorPlatform;

/// What `list` says about itself: the window's own surface, not a desktop app.
pub const BUILT_IN_SURFACE: &str = "zerocode-built-in";

/// The key of the answer that carries real devices.
pub const PHYSICAL_KEY: &str = "physical";
/// The key that names what a tool did not answer, with the reason.
pub const UNCHECKED_KEY: &str = "unchecked";
/// The key that counts the real devices left out past [`PHYSICAL_ROWS_MAX`].
pub const OMITTED_KEY: &str = "omitted";

/// How many real devices one answer carries. A row costs about fifty tokens and
/// `devicectl` lists every phone ever paired, so a drawer of test phones would
/// be paid for on every `list`; the connected ones come first and the count of
/// the rest is said.
pub const PHYSICAL_ROWS_MAX: usize = 8;

/// The longest device name an answer carries. The name is the owner's own
/// word, or a device's own property, and it reaches the agent's context: this
/// is long enough for "<person>'s iPhone Air" and too short to carry a
/// paragraph.
pub const DEVICE_NAME_CHARS_MAX: usize = 48;

/// The name a row carries when the tool gave none: an Android device that
/// reports no model.
pub const UNNAMED_ANDROID: &str = "Android device";

/// Why a real iPhone or iPad is not drivable, in the words an agent hands the
/// person.
pub const IOS_REASON: &str = "A real iPhone or iPad. This window drives only the iOS Simulator.";
/// Why a real Android phone or tablet is not drivable.
pub const ANDROID_REASON: &str =
    "A real Android phone or tablet. This window drives only Android emulators.";

/// What `adb` calls the serial of an emulator the window drives. Every other
/// serial is a device on a cable or a network. Spelled here once: the
/// window's emulator road and the reader below must tell the two apart the
/// same way.
pub const ANDROID_EMULATOR_SERIAL_PREFIX: &str = "emulator-";

/// How the Mac stands with one real device, in plain words.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum LinkState {
    /// The Mac reaches it now.
    Connected,
    /// On a cable, but the phone has not allowed this computer (`adb`).
    Unauthorized,
    /// On the bridge, but not answering (`adb`).
    Offline,
    /// The Mac knows the phone and cannot reach it now (`devicectl`).
    Unavailable,
    /// The tool said a word this reader does not know.
    Unknown,
}

/// One real device this window cannot drive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PhysicalDevice {
    pub platform: EmulatorPlatform,
    /// The name the device carries. There is no id: no command can use one, and
    /// a hardware identifier is not the agent's to carry.
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub state: LinkState,
    /// Always false: it is a field so an agent reads the answer instead of
    /// assuming it, and so a road added later says `true` where it exists.
    pub drivable: bool,
    pub reason: &'static str,
}

impl PhysicalDevice {
    fn new(
        platform: EmulatorPlatform,
        name: &str,
        model: Option<String>,
        state: LinkState,
    ) -> Self {
        Self {
            platform,
            name: plain_name(name),
            model: model.map(|model| plain_name(&model)),
            state,
            drivable: false,
            reason: match platform {
                EmulatorPlatform::Ios => IOS_REASON,
                EmulatorPlatform::Android => ANDROID_REASON,
            },
        }
    }
}

/// A platform's tool that did not answer, and why — a gap, which is not the
/// same as "nothing is connected".
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Unchecked {
    pub platform: EmulatorPlatform,
    pub why: String,
}

/// A device's name as the answer carries it.
fn plain_name(raw: &str) -> String {
    raw.to_string()
}

/// `devicectl list devices --json-output` → the iPhones and iPads in it.
///
/// # Errors
/// The text is not JSON, or has no device list.
pub fn parse_devicectl(_json: &str) -> Result<Vec<PhysicalDevice>, String> {
    Ok(Vec::new())
}

/// One device line of `adb devices -l`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdbRow {
    pub serial: String,
    /// `adb`'s own word: `device`, `offline`, `unauthorized`, …
    pub state: String,
    /// The `model:` property, with its underscores read as spaces.
    pub model: Option<String>,
}

impl AdbRow {
    /// Whether this serial is an emulator the window drives.
    #[must_use]
    pub fn is_emulator(&self) -> bool {
        self.serial.starts_with(ANDROID_EMULATOR_SERIAL_PREFIX)
    }

    /// Whether `adb` lists it as up: the only state a device can be used in.
    #[must_use]
    pub fn is_up(&self) -> bool {
        self.state == "device"
    }
}

/// The device lines of `adb devices -l`.
#[must_use]
pub fn parse_adb_rows(_listing: &str) -> Vec<AdbRow> {
    Vec::new()
}

/// The real Android devices in an `adb devices -l` listing.
#[must_use]
pub fn parse_adb_physical(_listing: &str) -> Vec<PhysicalDevice> {
    Vec::new()
}

/// The rows an answer carries and how many were left out.
#[must_use]
pub fn bounded(rows: Vec<PhysicalDevice>) -> (Vec<PhysicalDevice>, usize) {
    (rows, 0)
}

/// What one platform's tools said to `list`.
pub struct PlatformReading {
    pub platform: EmulatorPlatform,
    /// The simulators or emulators: the rows `list` always carried.
    pub simulated: Result<Value, String>,
    /// The real devices; the error is why they were not read.
    pub physical: Result<Vec<PhysicalDevice>, String>,
}

/// The `list` answer, as it was before real devices were named: the
/// simulators and the emulators, or the first error.
///
/// # Errors
/// Either platform's simulators or emulators could not be read.
pub fn list_answer(ios: PlatformReading, android: PlatformReading) -> Result<Value, String> {
    Ok(json!({
        "surface": BUILT_IN_SURFACE,
        "ios": ios.simulated?,
        "android": android.simulated?,
    }))
}

#[cfg(test)]
mod tests;
