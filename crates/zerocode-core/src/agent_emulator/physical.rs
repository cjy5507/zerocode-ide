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
//! `drivable: false` and the reason in plain words. The reason also says the
//! road a device takes instead when it has one: a real iPhone is driven through
//! the Mac's iPhone Mirroring window with `zerocode-computer`, so `list` is the
//! one step that tells an agent which road a device takes.

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
/// is long enough for a name such as "Synthetic Phone Max" and too short to
/// carry a paragraph.
pub const DEVICE_NAME_CHARS_MAX: usize = 48;

/// The name a row carries when the tool gave none: an Android device that
/// reports no model.
pub const UNNAMED_ANDROID: &str = "Android device";

/// The id of the Mac's iPhone Mirroring app, the window a real iPhone is driven
/// through. An id is the same in every language; its name is not ("iPhone
/// Mirroring", "iPhone 미러링"), and an agent that called the app by three names
/// in one day found it by none of them.
pub const IPHONE_MIRRORING_APP: &str = "com.apple.ScreenContinuity";

/// Why a real iPhone is not drivable by `zerocode-emulator`, and the road it
/// takes instead, in the words an agent acts on: which window, through which
/// tool, the app by its id, and what to do when the window is not open (ask the
/// person once; the app is theirs to open and the phone theirs to unlock). It is
/// in the row itself so that `list` is the one step that tells which road a
/// device takes. It must carry [`IPHONE_MIRRORING_APP`]; a test holds it to that.
pub const IPHONE_REASON: &str = "A real iPhone. zerocode-emulator cannot drive it; the Mac's iPhone Mirroring window can, through zerocode-computer (--app com.apple.ScreenContinuity). If that window is not open, ask the person to open it once; do not open the app or unlock anything yourself.";
/// Why a real iPad is not drivable, and that no road reaches it from here:
/// iPhone Mirroring shows iPhones only.
pub const IPAD_REASON: &str = "A real iPad. zerocode-emulator cannot drive it, and iPhone Mirroring shows iPhones only; tell the person so and offer a simulator, or work on the app's code.";
/// Why a real Android phone or tablet is not drivable.
pub const ANDROID_REASON: &str =
    "A real Android phone or tablet. This window drives only Android emulators.";

/// What `adb` calls the serial of an emulator the window drives. Every other
/// serial is a device on a cable or a network. Spelled here once: the
/// window's emulator road and the reader below must tell the two apart the
/// same way.
pub const ANDROID_EMULATOR_SERIAL_PREFIX: &str = "emulator-";

/// The platforms `devicectl` reports as an iPhone and as an iPad. It also lists
/// watches, TVs and headsets, which are not the person's phone.
const DEVICECTL_IPHONE_PLATFORM: &str = "iOS";
const DEVICECTL_IPAD_PLATFORM: &str = "iPadOS";
const DEVICECTL_PHONE_PLATFORMS: [&str; 2] = [DEVICECTL_IPHONE_PLATFORM, DEVICECTL_IPAD_PLATFORM];

/// Where a device's facts sit in `devicectl list devices --json-output` (JSON
/// pointers). Its own table is the one place that says what each means.
const DEVICECTL_DEVICES_AT: &str = "/result/devices";
const DEVICECTL_PLATFORM_AT: &str = "/hardwareProperties/platform";
const DEVICECTL_MODEL_AT: &str = "/hardwareProperties/marketingName";
const DEVICECTL_NAME_AT: &str = "/deviceProperties/name";
const DEVICECTL_TUNNEL_AT: &str = "/connectionProperties/tunnelState";

/// `devicectl`'s words for the tunnel to a device.
const TUNNEL_CONNECTED: &str = "connected";
const TUNNEL_DISCONNECTED: &str = "disconnected";
const TUNNEL_UNAVAILABLE: &str = "unavailable";

/// `adb`'s words for the state of a listed device, as `adb devices -l` prints
/// them. Only `device` is a device that can be used.
const ADB_STATE_UP: &str = "device";
const ADB_STATE_UNAUTHORIZED: &str = "unauthorized";
const ADB_STATE_OFFLINE: &str = "offline";

/// What a line of `adb devices -l` that is not a device starts with: the header
/// of the list, and the notices the `adb` daemon writes about itself
/// (`* daemon started successfully`).
const ADB_HEADER_PREFIX: &str = "List of devices";
const ADB_NOTICE_PREFIX: char = '*';

/// The `key:value` column of an `adb devices -l` line that carries the model.
const ADB_MODEL_PREFIX: &str = "model:";

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

/// The kinds of real device a row can be. What a device is decides the road it
/// takes, so it decides the words of the row's `reason`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Hardware {
    /// An iPhone: the Mac's iPhone Mirroring window drives it.
    IPhone,
    /// An iPad: iPhone Mirroring shows iPhones only.
    IPad,
    /// An Android phone or tablet on a cable or a network.
    AndroidDevice,
}

impl Hardware {
    fn platform(self) -> EmulatorPlatform {
        match self {
            Self::IPhone | Self::IPad => EmulatorPlatform::Ios,
            Self::AndroidDevice => EmulatorPlatform::Android,
        }
    }

    fn reason(self) -> &'static str {
        match self {
            Self::IPhone => IPHONE_REASON,
            Self::IPad => IPAD_REASON,
            Self::AndroidDevice => ANDROID_REASON,
        }
    }
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
    fn new(hardware: Hardware, name: &str, model: Option<String>, state: LinkState) -> Self {
        Self {
            platform: hardware.platform(),
            name: plain_name(name),
            model: model.map(|model| plain_name(&model)),
            state,
            drivable: false,
            reason: hardware.reason(),
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

/// A device's name as the answer carries it: one line, no control characters,
/// cut at [`DEVICE_NAME_CHARS_MAX`].
fn plain_name(raw: &str) -> String {
    let one_line = raw
        .split_whitespace()
        .map(|word| {
            word.chars()
                .filter(|ch| !ch.is_control())
                .collect::<String>()
        })
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    one_line.chars().take(DEVICE_NAME_CHARS_MAX).collect()
}

/// `devicectl list devices --json-output` → the iPhones and iPads in it.
///
/// # Errors
/// The text is not JSON, or has no device list: the tool's answer changed shape
/// or something else wrote the file.
pub fn parse_devicectl(json: &str) -> Result<Vec<PhysicalDevice>, String> {
    let document: Value = serde_json::from_str(json)
        .map_err(|error| format!("devicectl wrote an answer this window cannot read: {error}"))?;
    let devices = document
        .pointer(DEVICECTL_DEVICES_AT)
        .and_then(Value::as_array)
        .ok_or("devicectl's answer has no device list")?;
    Ok(devices.iter().filter_map(ios_row).collect())
}

fn ios_row(device: &Value) -> Option<PhysicalDevice> {
    let text = |at: &str| {
        device
            .pointer(at)
            .and_then(Value::as_str)
            .map(str::to_string)
    };
    let platform = text(DEVICECTL_PLATFORM_AT)?;
    if !DEVICECTL_PHONE_PLATFORMS.contains(&platform.as_str()) {
        return None;
    }
    let model = text(DEVICECTL_MODEL_AT);
    let name = text(DEVICECTL_NAME_AT).or_else(|| model.clone())?;
    let state = tunnel_state(text(DEVICECTL_TUNNEL_AT).as_deref());
    let hardware = if platform == DEVICECTL_IPAD_PLATFORM {
        Hardware::IPad
    } else {
        Hardware::IPhone
    };
    Some(PhysicalDevice::new(hardware, &name, model, state))
}

/// What `devicectl`'s tunnel word means for the person. Its own table calls a
/// `disconnected` tunnel "available (paired)": the Mac reaches the phone and
/// has not opened a tunnel yet. Only `unavailable` is a phone it cannot reach.
fn tunnel_state(word: Option<&str>) -> LinkState {
    match word {
        Some(TUNNEL_CONNECTED | TUNNEL_DISCONNECTED) => LinkState::Connected,
        Some(TUNNEL_UNAVAILABLE) => LinkState::Unavailable,
        _ => LinkState::Unknown,
    }
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
        self.state == ADB_STATE_UP
    }
}

/// The device lines of `adb devices -l`: the header, the daemon's own notices
/// (`* daemon started …`) and blank lines are not devices.
#[must_use]
pub fn parse_adb_rows(listing: &str) -> Vec<AdbRow> {
    listing
        .lines()
        .map(str::trim)
        .filter(|line| {
            !line.is_empty()
                && !line.starts_with(ADB_NOTICE_PREFIX)
                && !line.starts_with(ADB_HEADER_PREFIX)
        })
        .filter_map(|line| {
            let mut columns = line.split_whitespace();
            let serial = columns.next()?.to_string();
            let state = columns.next()?.to_string();
            let model = columns
                .find_map(|column| column.strip_prefix(ADB_MODEL_PREFIX))
                .map(|model| model.replace('_', " "));
            Some(AdbRow {
                serial,
                state,
                model,
            })
        })
        .collect()
}

/// The real Android devices among the rows of one `adb devices -l`: every row
/// that is not an emulator. The window reads the listing once and hands the same
/// rows to this and to its emulator reader.
#[must_use]
pub fn physical_android_rows(rows: &[AdbRow]) -> Vec<PhysicalDevice> {
    rows.iter()
        .filter(|row| !row.is_emulator())
        .map(|row| {
            let state = match row.state.as_str() {
                ADB_STATE_UP => LinkState::Connected,
                ADB_STATE_UNAUTHORIZED => LinkState::Unauthorized,
                ADB_STATE_OFFLINE => LinkState::Offline,
                _ => LinkState::Unknown,
            };
            let name = row.model.clone().unwrap_or_else(|| UNNAMED_ANDROID.into());
            PhysicalDevice::new(Hardware::AndroidDevice, &name, row.model.clone(), state)
        })
        .collect()
}

/// The rows an answer carries and how many were left out: the ones the Mac
/// reaches first, then by platform and name (the order the tool listed them in
/// settles the rest), cut at [`PHYSICAL_ROWS_MAX`].
#[must_use]
pub fn bounded(mut rows: Vec<PhysicalDevice>) -> (Vec<PhysicalDevice>, usize) {
    rows.sort_by(|left, right| {
        (left.state != LinkState::Connected)
            .cmp(&(right.state != LinkState::Connected))
            .then_with(|| left.platform.as_str().cmp(right.platform.as_str()))
            .then_with(|| left.name.cmp(&right.name))
    });
    let omitted = rows.len().saturating_sub(PHYSICAL_ROWS_MAX);
    rows.truncate(PHYSICAL_ROWS_MAX);
    (rows, omitted)
}

/// What one platform's tools said to `list`.
pub struct PlatformReading {
    pub platform: EmulatorPlatform,
    /// The simulators or emulators: the rows `list` always carried.
    pub simulated: Result<Value, String>,
    /// The real devices; the error is why they were not read.
    pub physical: Result<Vec<PhysicalDevice>, String>,
}

/// The `list` answer: what the window can drive, what it sees and cannot, and
/// what nobody read.
///
/// One platform's missing tool does not hide the other platform's answer — a
/// Mac with Xcode and no Android SDK still has simulators and phones to
/// report. Only when neither platform answered is there no answer, and then it
/// is the first platform's error, as it always was.
///
/// # Errors
/// Neither platform's simulators or emulators could be read.
pub fn list_answer(ios: PlatformReading, android: PlatformReading) -> Result<Value, String> {
    if let (Err(first), Err(_)) = (&ios.simulated, &android.simulated) {
        return Err(first.clone());
    }
    let mut unchecked = Vec::new();
    let mut real = Vec::new();
    let mut take = |reading: PlatformReading| {
        let PlatformReading {
            platform,
            simulated,
            physical,
        } = reading;
        let mut gaps = Vec::new();
        let simulated = simulated.unwrap_or_else(|why| {
            gaps.push(why);
            json!([])
        });
        match physical {
            Ok(found) => real.extend(found),
            // One missing SDK stops both readers with the same sentence: that
            // is one gap, said once.
            Err(why) if !gaps.contains(&why) => gaps.push(why),
            Err(_) => {}
        }
        unchecked.extend(gaps.into_iter().map(|why| Unchecked { platform, why }));
        simulated
    };
    let simulators = take(ios);
    let emulators = take(android);
    let (real, omitted) = bounded(real);
    let mut answer = json!({
        "surface": BUILT_IN_SURFACE,
        "ios": simulators,
        "android": emulators,
        PHYSICAL_KEY: real,
    });
    if omitted > 0 {
        answer[OMITTED_KEY] = json!(omitted);
    }
    if !unchecked.is_empty() {
        answer[UNCHECKED_KEY] = json!(unchecked);
    }
    Ok(answer)
}

#[cfg(test)]
mod tests;
