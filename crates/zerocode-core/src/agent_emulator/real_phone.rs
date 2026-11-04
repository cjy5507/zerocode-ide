//! The pure half of driving a real iPhone from inside it (t-90894): the words,
//! the pins and the readers. WebDriverAgent runs on the phone as a test runner
//! and answers HTTP over the tunnel the Mac already keeps to the phone, so a
//! press needs no Mac pointer, no front app and no clipboard, and the phone hands
//! back a tree of its controls that iPhone Mirroring does not have.
//!
//! Running anything — `xcodebuild`, `devicectl`, HTTP — is the window's
//! (`emulator::wda`). Nothing here starts a process or reads a clock.
//!
//! What stays the person's, on this road and in this file's words: unlocking the
//! phone, a password or PIN, biometrics, and a payment or transfer press. None of
//! them waits for an answer: the person may be nowhere near the Mac, so the road
//! refuses at once and the refusal says why, for the agent to pass on.

use serde::Serialize;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::computer_use::MARK_ROLES;

/// Where the source comes from. It is built on the person's Mac, and never kept
/// in this repository.
pub const WDA_REPOSITORY: &str = "https://github.com/appium/WebDriverAgent.git";
/// The tag this window was tested with, and what that tag must point at.
pub const WDA_TAG: &str = "v16.15.3";
pub const WDA_COMMIT: &str = "02cb92f0e93c8b021443bcbb8eeb80932c113220";
/// The tree of [`WDA_COMMIT`]: the hash of the files themselves, compared after
/// the fetch and before anything is patched or built.
pub const WDA_TREE: &str = "6094cc6e6d2ae02beef9bfd20e3434ed346ec742";

/// What a real phone's id starts with on `--device`. No simulator's UDID and no
/// Android serial can start with it, so it alone says which road a verb takes.
pub const REAL_PHONE_PREFIX: &str = "phone-";
/// How many hex digits of the hash an id carries: 48 bits tell apart any drawer
/// of phones, and a hardware identifier does not reach the agent's context.
const HANDLE_HEX_DIGITS: usize = 12;

/// The ports a runner listens on. Over the tunnel every phone has its own
/// address, so one port serves them all.
pub const WDA_PORT: u16 = 8100;

/// What `xcodebuild` hands the runner as `USE_IP`: the one address its HTTP server
/// and its screen stream (9100) listen on. Without it both listen on every network
/// the phone has, Wi-Fi included, with no password (the upstream's
/// `FBWebServer.m`), so a runner is never started without it.
pub const RUNNER_BIND_ENV: &str = "TEST_RUNNER_USE_IP";

/// What a runner prints once it listens, around its own address.
pub const SERVER_URL_BEGIN: &str = "ServerURLHere->";
const SERVER_URL_END: &str = "<-ServerURLHere";

/// What an act on a real phone answers about its landing. Its patch answers 0.5 s
/// after the event even when the phone's own answer is late, so a late error is
/// not heard; what the screen did after the press is the only evidence.
pub const LANDED_KEY: &str = "landed";
/// The gesture was sent and the phone took it, and nobody looked after it, or the
/// screen was still changing when the press stopped waiting: look before counting
/// on it.
pub const LANDED_UNCONFIRMED: &str = "unconfirmed";
/// A press that waited for its screen (t-93254): the screen changed after it and
/// then stood still.
pub const LANDED_SCREEN_CHANGED: &str = "screenChanged";
/// A press that waited for its screen: nothing on it changed within the quiet
/// window. A press on an inert spot reads the same as one that missed.
pub const LANDED_SCREEN_UNCHANGED: &str = "screenUnchanged";

/// The refusals of this road, as `code` in the answer.
pub const PHONE_NOT_ALLOWED: &str = "phone_not_allowed";
pub const PHONE_NOT_INSTALLED: &str = "phone_not_installed";
pub const PHONE_LOCKED: &str = "phone_locked";
pub const PHONE_UNREACHABLE: &str = "phone_unreachable";
pub const PHONE_SECURE_FIELD: &str = "phone_secure_field";
pub const PHONE_TEAM_REQUIRED: &str = "phone_team_required";
pub const PHONE_SOURCE_MISMATCH: &str = "phone_source_mismatch";
pub const PHONE_PATCH_MISMATCH: &str = "phone_patch_mismatch";

/// What every act on a phone answers beside `performed`: which road pressed, and
/// that the Mac's pointer did not move. A caller passes them on as they are.
pub const ROAD_KEY: &str = "road";
pub const ROAD_WDA: &str = "webdriveragent";
pub const MAC_POINTER_KEY: &str = "macPointer";

/// How long the runner stays up with nobody using it, when the person has not
/// turned unattended use on. The phone locks itself in about three minutes
/// (measured 2026-11-03), and a locked phone refuses every press, so a runner kept
/// much longer than that serves nothing; a start with nothing to build costs tens
/// of seconds, so a pause between two steps of one job must not pay for one.
pub const IDLE_STOP_MINUTES: u64 = 5;

/// How a runner that died unattended comes back: the wait before each try, and
/// the tries that stand before the window stops trying and says so. A run that
/// stayed up for [`RESTART_FORGIVEN_SECS`] was not a failing one, and starts over.
pub const RESTART_BACKOFF_SECS: [u64; 5] = [5, 15, 45, 120, 300];
pub const RESTART_FORGIVEN_SECS: u64 = 600;

/// What the source call leaves out. `visible` is nearly the whole cost: on an
/// iOS 26.5 simulator (193 elements, three reads each) the whole tree took
/// 2,444–2,550 ms and without `visible` and `accessible` 654–776 ms, and leaving
/// out `enabled`, `name` and `rawIdentifier` as well moved nothing (679–705 ms).
/// On an iPhone the whole tree took 4.8 s and with all of them out 2.0 s.
pub const SOURCE_QUERY: &str = "format=json&excluded_attributes=visible,accessible";
/// How deep a tree is read, against the default of 50. The deepest element of the
/// trees measured stood at 13 and a control at 11; past this a screen's nesting
/// is decoration, and a phone that goes deeper is cut, not refused.
pub const SNAPSHOT_MAX_DEPTH: u32 = 20;

/// The name the marks and the door give the phone's home screen, the one app an
/// agent always finds a phone on after `button --name home`.
pub const SPRINGBOARD: &str = "com.apple.springboard";

/// The id `--device` takes for a phone with this hardware identifier.
#[must_use]
pub fn phone_handle(udid: &str) -> String {
    let digest = Sha256::digest(udid.as_bytes());
    let mut hex = String::with_capacity(HANDLE_HEX_DIGITS);
    for byte in digest.iter().take(HANDLE_HEX_DIGITS.div_ceil(2)) {
        hex.push_str(&format!("{byte:02x}"));
    }
    format!("{REAL_PHONE_PREFIX}{hex}")
}

/// Whether `--device` names a real phone rather than a simulator or an emulator.
#[must_use]
pub fn is_real_phone(device: &str) -> bool {
    device.starts_with(REAL_PHONE_PREFIX)
}

/// The bundle id the runner is signed under. Apple keeps one id to one team, so
/// the upstream's cannot be used, and the team's own id stays out of the name.
#[must_use]
pub fn runner_bundle_id(team: &str) -> String {
    let digest = Sha256::digest(team.as_bytes());
    let mut hex = String::new();
    for byte in digest.iter().take(4) {
        hex.push_str(&format!("{byte:02x}"));
    }
    format!("dev.zerocode.wda{hex}.WebDriverAgentRunner")
}

/// What decides whether a build still stands: only these four. A phone that was
/// replugged, a window that was updated or a runner that died builds nothing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct BuildStamp {
    pub tag: String,
    pub patch: String,
    pub team: String,
    pub device_os: String,
}

impl BuildStamp {
    #[must_use]
    pub fn new(patch_text: &str, team: &str, device_os: &str) -> Self {
        let digest = Sha256::digest(patch_text.as_bytes());
        let mut patch = String::new();
        for byte in digest {
            patch.push_str(&format!("{byte:02x}"));
        }
        Self {
            tag: WDA_TAG.to_string(),
            patch,
            team: team.to_string(),
            device_os: device_os.to_string(),
        }
    }
}

/// What a fetch must have brought before anything is patched or built. A tag
/// that moved, or a server that answers other files under this commit, stops here.
///
/// # Errors
/// The three hashes are not the pinned ones; the sentence says which, for the
/// person, and that nothing was built.
pub fn check_fetched(tag_commit: &str, head: &str, tree: &str) -> Result<(), String> {
    let wrong = |what: &str, got: &str, want: &str| {
        Err(format!(
            "{PHONE_SOURCE_MISMATCH}: the WebDriverAgent source this window fetched is not the one it was tested with ({what}: {got}, expected {want}). Nothing was patched or built. Tell the person; do not fetch it another way."
        ))
    };
    if tag_commit != WDA_COMMIT {
        return wrong(&format!("tag {WDA_TAG}"), tag_commit, WDA_COMMIT);
    }
    if head != WDA_COMMIT {
        return wrong("commit", head, WDA_COMMIT);
    }
    if tree != WDA_TREE {
        return wrong("tree", tree, WDA_TREE);
    }
    Ok(())
}

/// The sentence for a patch that does not fit the pinned source.
#[must_use]
pub fn patch_mismatch(diagnostic: &str) -> String {
    format!(
        "{PHONE_PATCH_MISMATCH}: this window's small patch does not fit the WebDriverAgent source ({diagnostic}). Nothing was built and nothing went to the phone. Tell the person."
    )
}

/// A team Xcode knows, as the person sees it on the card.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Team {
    pub id: String,
    pub name: String,
    /// `Individual` or `Company`, Xcode's own word.
    pub kind: String,
    /// A free team's signature lasts seven days.
    pub free: bool,
}

/// `IDEProvisioningTeamByIdentifier` of Xcode's preferences, as JSON: an account
/// per key, its teams under it. A team two accounts share is one team.
#[must_use]
pub fn parse_teams(json: &str) -> Vec<Team> {
    let Ok(accounts) = serde_json::from_str::<Map<String, Value>>(json) else {
        return Vec::new();
    };
    let mut teams: Vec<Team> = Vec::new();
    for team in accounts.values().filter_map(Value::as_array).flatten() {
        let Some(id) = team.get("teamID").and_then(Value::as_str) else {
            continue;
        };
        if teams.iter().any(|known| known.id == id) {
            continue;
        }
        let word = |key: &str| team.get(key).and_then(Value::as_str).unwrap_or_default();
        teams.push(Team {
            id: id.to_string(),
            name: word("teamName").to_string(),
            kind: word("teamType").to_string(),
            free: team
                .get("isFreeProvisioningTeam")
                .and_then(Value::as_i64)
                .is_some_and(|free| free != 0)
                || team.get("isFreeProvisioningTeam").and_then(Value::as_bool) == Some(true),
        });
    }
    teams
}

/// Which team to sign with: only the one the caller named, or the only one Xcode
/// knows. Several and no name is a question for the person, never a guess, and a
/// company team is never taken because it came first.
///
/// # Errors
/// No team, a name Xcode does not know, or several and no name; the sentence lists
/// them for the person to choose.
pub fn choose_team<'a>(teams: &'a [Team], asked: Option<&str>) -> Result<&'a Team, String> {
    let listed = || {
        teams
            .iter()
            .map(|team| format!("{} ({}, {})", team.id, team.name, team.kind))
            .collect::<Vec<_>>()
            .join("; ")
    };
    if let Some(asked) = asked {
        return teams
            .iter()
            .find(|team| team.id == asked)
            .ok_or_else(|| format!("{PHONE_TEAM_REQUIRED}: Xcode knows no team {asked}. Teams: {}. Ask the person which one.", listed()));
    }
    match teams {
        [] => Err(format!(
            "{PHONE_TEAM_REQUIRED}: Xcode knows no signing team on this Mac. The person adds their Apple account in Xcode's settings first."
        )),
        [only] => Ok(only),
        _ => Err(format!(
            "{PHONE_TEAM_REQUIRED}: Xcode knows several signing teams: {}. Ask the person which one and pass it with --team; do not choose for them.",
            listed()
        )),
    }
}

/// The card that gives this road its consent, in the words a person reads: what
/// goes onto the phone, that it is a patched build, under whose name, what it may
/// then do, and that it lasts until they turn it off. The page's own sentence
/// under the same key says the same.
#[must_use]
pub fn consent_sentence(team: &Team, phone: &str) -> String {
    format!(
        "An agent asks to control the real phone \"{phone}\" from inside it. Allowing it builds Appium's WebDriverAgent {WDA_TAG} on this Mac with one small patch of ours (a modified build, not the upstream one) and installs it on the phone, signed by your team \"{name}\" ({kind}). Agents can then read the screen, press, swipe and type on this phone without the Mac's pointer. They can press while you are using the phone too; lock it to stop them. They never unlock it or enter a password, and a payment or a transfer stays yours. Xcode makes an App ID and a provisioning profile for this runner in your Apple account. This lasts for this phone until you turn it off in Settings or run phone-remove.",
        name = team.name,
        kind = team.kind,
    )
}

/// The key of the page's sentence for [`consent_sentence`].
pub const CONSENT_KEY: &str = "emulator.realPhone.consent";
/// The card for taking it off again.
pub const REMOVE_KEY: &str = "emulator.realPhone.remove";

/// The sentence for [`REMOVE_KEY`].
#[must_use]
pub fn remove_sentence(phone: &str) -> String {
    format!(
        "An agent asks to remove the WebDriverAgent runner from \"{phone}\" and its build from this Mac, and to turn off controlling real phones from inside. Nothing else on the phone changes. The App ID and profile Xcode made in your Apple account stay there."
    )
}

/// What the agent hears when the road has no consent. There is no flag and no
/// file for an agent to turn it on with: the card is the person's.
#[must_use]
pub fn not_allowed_sentence(handle: &str) -> String {
    format!(
        "{PHONE_NOT_ALLOWED}: the person has not allowed driving this phone from inside. Run `zerocode-emulator phone-install --device {handle}`: the window puts a card to the person, and only their answer turns it on. If nobody answers, tell the person and stop; iPhone Mirroring is a road only when the person asks for it."
    )
}

/// The road a locked phone leaves: what the person can do, and what an agent can
/// do now. iPhone Mirroring needs the phone locked, so a locked phone is that
/// road's condition; its pointer is the Mac's, so pressing by mark or by letter
/// takes the shared desktop setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LockedRoads {
    /// The Mac has the iPhone Mirroring app.
    pub mirroring_installed: bool,
    /// The app is running. Whether its window shows the phone only a look tells.
    pub mirroring_running: bool,
    /// `computer_input_policy` is the shared desktop.
    pub shared_desktop: bool,
}

/// Why a locked phone was refused, with what is possible right now.
#[must_use]
pub fn locked_sentence(roads: LockedRoads) -> String {
    let first = format!(
        "{PHONE_LOCKED}: the phone is locked and nothing was pressed. Only the person can unlock it: this window never unlocks a phone, and there is no way around that. Pass this on to the person and stop; the mirroring road below is open only if the person asks for it."
    );
    let mirroring = if !roads.mirroring_installed {
        " iPhone Mirroring: not on this Mac, so no road is open now.".to_string()
    } else {
        let running = if roads.mirroring_running {
            "the app is running; look at its window with `zerocode-computer list-windows --app com.apple.ScreenContinuity`"
        } else {
            "the app is not running, and it is the person's to open"
        };
        let pressing = if roads.shared_desktop {
            "the input setting is the shared desktop, so it may borrow the Mac's pointer to press"
        } else {
            "the input setting is background only, so it can read the phone and send keys but pressing by pointer is refused until the person sets the shared desktop"
        };
        format!(" iPhone Mirroring works on a locked phone (it needs one): {running}; {pressing}.")
    };
    format!("{first}{mirroring}")
}

/// A phone's short element type as the marks read it: the iOS exporter's word
/// where the phone's differs, so the shared numbering takes it unchanged.
fn exporter_type(wda: &str) -> &str {
    let short = wda.strip_prefix("XCUIElementType").unwrap_or(wda);
    match short {
        "Icon" => "Button",
        "Switch" | "Toggle" => "CheckBox",
        "Stepper" => "Incrementor",
        other => other,
    }
}

/// Whether a phone's element type is a secret field: the tree's own word, never a
/// label's, so a field called anything is known, and a label cannot hide one.
#[must_use]
pub fn is_secure_type(wda: &str) -> bool {
    wda.strip_prefix("XCUIElementType").unwrap_or(wda) == "SecureTextField"
}

/// The first secret field a phone's tree shows, for a refusal that says what
/// it found.
#[must_use]
pub fn has_visible_secure_field(source: &Value) -> bool {
    let mut pending = vec![source];
    while let Some(node) = pending.pop() {
        let sized = rect_of(node).is_some_and(|rect| rect[2] > 0.0 && rect[3] > 0.0);
        let kind = node.get("type").and_then(Value::as_str).unwrap_or_default();
        if sized && is_secure_type(kind) {
            return true;
        }
        if let Some(children) = node.get("children").and_then(Value::as_array) {
            pending.extend(children);
        }
    }
    false
}

fn rect_of(node: &Value) -> Option<[f64; 4]> {
    let rect = node.get("rect")?;
    let side = |key: &str| rect.get(key)?.as_f64().filter(|value| value.is_finite());
    Some([side("x")?, side("y")?, side("width")?, side("height")?])
}

/// WebDriverAgent's tree in the shape the simulator's exporter writes
/// (`emulator::marks::faces` reads it, numbering included), so a phone's look is
/// numbered exactly as a simulator's is, with no text recognition.
///
/// An element with no size is dropped with what hangs under it only when nothing
/// under it has a size: a home screen carries a size-zero copy of each icon that
/// a search also finds, and pressing one does nothing (2026-11-03). The `bundle`
/// goes on the root as its identity, so a look taken in one app breaks when
/// another comes to the front whatever the two screens have in common.
#[must_use]
pub fn tree_from_source(source: &Value, bundle: Option<&str>) -> Value {
    let Some(mut root) = convert(source) else {
        return json!([]);
    };
    if let Some(bundle) = bundle {
        root["AXUniqueId"] = json!(bundle);
    }
    answer_centres(&mut root);
    json!([root])
}

/// What can lie over a control beside another control.
const OVERLAY_TYPES: [&str; 5] = ["Keyboard", "Alert", "Sheet", "Popover", "Dialog"];

fn presses_or_covers(kind: &str) -> bool {
    OVERLAY_TYPES.contains(&kind) || MARK_ROLES.contains(&format!("AX{kind}").as_str())
}

/// The bars UIKit draws above the content that scrolls under them. A long
/// conversation's rows pass under the navigation bar; without this its back
/// button lost its number to a row behind it (2026-11-03, the Messages bench).
const BAR_TYPES: [&str; 3] = ["NavigationBar", "TabBar", "Toolbar"];

/// One node of a tree in preorder, for [`answer_centres`]: the first index past
/// its subtree says who its descendants and ancestors are.
struct Placed {
    centre: (f64, f64),
    frame: [f64; 4],
    past: usize,
    covers: bool,
    /// It is a bar ([`BAR_TYPES`]).
    bar: bool,
    /// It or a parent is a bar.
    in_bar: bool,
    /// Its nearest overlay parent (or itself), the layer it is drawn in.
    layer: Option<usize>,
}

fn frame_of(node: &Value) -> [f64; 4] {
    let side = |key: &str| node["frame"][key].as_f64().unwrap_or_default();
    [side("x"), side("y"), side("width"), side("height")]
}

fn place(node: &Value, out: &mut Vec<Placed>, in_bar: bool, layer: Option<usize>) {
    let frame = frame_of(node);
    let at = out.len();
    let kind = node["type"].as_str().unwrap_or_default();
    let bar = BAR_TYPES.contains(&kind);
    let in_bar = in_bar || bar;
    let layer = if OVERLAY_TYPES.contains(&kind) {
        Some(at)
    } else {
        layer
    };
    out.push(Placed {
        centre: (frame[0] + frame[2] / 2.0, frame[1] + frame[3] / 2.0),
        frame,
        past: 0,
        covers: presses_or_covers(kind),
        bar,
        in_bar,
        layer,
    });
    if let Some(children) = node.get("children").and_then(Value::as_array) {
        for child in children {
            place(child, out, in_bar, layer);
        }
    }
    out[at].past = out.len();
}

fn write_answers(node: &mut Value, answers: &[bool], at: &mut usize) {
    node[HIT_AT_CENTRE] = json!(answers[*at]);
    *at += 1;
    if let Some(children) = node.get_mut("children").and_then(Value::as_array_mut) {
        for child in children {
            write_answers(child, answers, at);
        }
    }
}

/// The key the simulator's exporter answers an element's centre under, read by
/// `emulator::marks`.
const HIT_AT_CENTRE: &str = "hit_at_centre";

/// What is on top at each element's centre, as far as a phone's tree can prove:
/// a press lands on the element or on something inside it, unless a control or an
/// overlay outside its own line of parents and children also holds that point, or
/// the centre is off the screen. Then no number is given, so nothing is pressed on
/// a guess about which of two lies on top. One order is not a guess: in one layer,
/// a bar ([`BAR_TYPES`]) and its controls lie over the content beside them, so a
/// bar's control keeps its number over a row scrolled under it, and the row loses
/// its number where the bar lies over its centre.
///
/// The simulator's exporter asks the screen; asking a phone the same costs the
/// `visible` attribute, the most expensive of all ([`SOURCE_QUERY`]), and that
/// attribute does not say what covers what either.
fn answer_centres(root: &mut Value) {
    let mut placed = Vec::new();
    place(root, &mut placed, false, None);
    let screen = placed[0].frame;
    let inside = |frame: [f64; 4], (x, y): (f64, f64)| {
        x >= frame[0] && y >= frame[1] && x <= frame[0] + frame[2] && y <= frame[1] + frame[3]
    };
    let answers: Vec<bool> = (0..placed.len())
        .map(|at| {
            let centre = placed[at].centre;
            inside(screen, centre)
                && !placed.iter().enumerate().any(|(other, over)| {
                    let parent = other < at && at < over.past;
                    let child = at < other && other < placed[at].past;
                    let me = &placed[at];
                    let same_layer = me.layer == over.layer;
                    // In one layer a bar is drawn over what is not in a bar.
                    let under_me = same_layer && me.in_bar && !over.in_bar;
                    let over_me = same_layer && over.bar && !me.in_bar;
                    other != at
                        && !parent
                        && !child
                        && inside(over.frame, centre)
                        && ((over.covers && !under_me) || over_me)
                })
        })
        .collect();
    let mut at = 0;
    write_answers(root, &answers, &mut at);
}

fn convert(node: &Value) -> Option<Value> {
    let rect = rect_of(node).filter(|rect| rect[2] > 0.0 && rect[3] > 0.0);
    let kind = node.get("type").and_then(Value::as_str).unwrap_or_default();
    let word = |key: &str| {
        node.get(key)
            .and_then(Value::as_str)
            .filter(|word| !word.is_empty())
    };
    let mut children: Vec<Value> = Vec::new();
    for child in node
        .get("children")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        // A list's row answers twice: a cell holding a cell of the same words and
        // frame. One control, so its inside takes its place, and two exact
        // copies side by side (a scroll bar, 2026-11-03) are one too.
        if child["type"] == node["type"]
            && child["label"] == node["label"]
            && rect_of(child) == rect
            && let Some(inner) = convert(child)
        {
            for grand in inner["children"].as_array().into_iter().flatten() {
                children.push(grand.clone());
            }
        } else if let Some(converted) = convert(child)
            && !children.contains(&converted)
        {
            children.push(converted);
        }
    }
    if rect.is_none() && children.is_empty() {
        return None;
    }
    let rect = rect.unwrap_or_default();
    let mut out = Map::new();
    out.insert("type".into(), json!(exporter_type(kind)));
    if let Some(id) = word("rawIdentifier") {
        out.insert("AXUniqueId".into(), json!(id));
    }
    if let Some(label) = word("label") {
        out.insert("AXLabel".into(), json!(label));
    }
    // A secret field's value is never carried, whatever the phone answers.
    if !is_secure_type(kind)
        && let Some(value) = word("value")
    {
        out.insert("AXValue".into(), json!(value));
    }
    // The phone answers "1" or "0"; an answer without the word is not a refusal.
    let enabled = node.get("isEnabled").and_then(Value::as_str) != Some("0");
    out.insert("enabled".into(), json!(enabled));
    out.insert(
        "frame".into(),
        json!({ "x": rect[0], "y": rect[1], "width": rect[2], "height": rect[3] }),
    );
    if !children.is_empty() {
        out.insert("children".into(), Value::Array(children));
    }
    Some(Value::Object(out))
}

/// One finger's press, as a W3C action. Coordinates are the phone's points.
#[must_use]
pub fn tap_actions(x: f64, y: f64) -> Value {
    finger(&[
        json!({ "type": "pointerMove", "duration": 0, "x": x, "y": y }),
        json!({ "type": "pointerDown" }),
        json!({ "type": "pause", "duration": 50 }),
        json!({ "type": "pointerUp" }),
    ])
}

/// One finger's drag, as a W3C action.
#[must_use]
pub fn swipe_actions(from: (f64, f64), to: (f64, f64), ms: u32) -> Value {
    finger(&[
        json!({ "type": "pointerMove", "duration": 0, "x": from.0, "y": from.1 }),
        json!({ "type": "pointerDown" }),
        json!({ "type": "pointerMove", "duration": ms, "x": to.0, "y": to.1 }),
        json!({ "type": "pointerUp" }),
    ])
}

fn finger(steps: &[Value]) -> Value {
    json!({ "actions": [{
        "type": "pointer",
        "id": "finger",
        "parameters": { "pointerType": "touch" },
        "actions": steps,
    }]})
}

/// The tunnel address of one phone in `devicectl list devices --json-output`,
/// with what the rest of the road needs of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhoneFacts {
    pub handle: String,
    /// The hardware identifier `xcodebuild` takes. It goes no further than the
    /// window's own processes.
    pub udid: String,
    pub name: String,
    pub os: String,
    pub address: Option<String>,
    pub developer_mode: Option<String>,
    pub tunnel: Option<String>,
}

/// The iPhones and iPads in a `devicectl` answer, for the window's own use.
#[must_use]
pub fn phones_in_devicectl(json: &str) -> Vec<PhoneFacts> {
    let Ok(document) = serde_json::from_str::<Value>(json) else {
        return Vec::new();
    };
    let Some(devices) = document
        .pointer("/result/devices")
        .and_then(Value::as_array)
    else {
        return Vec::new();
    };
    devices
        .iter()
        .filter_map(|device| {
            let text = |at: &str| {
                device
                    .pointer(at)
                    .and_then(Value::as_str)
                    .map(str::to_string)
            };
            let platform = text("/hardwareProperties/platform")?;
            if platform != "iOS" && platform != "iPadOS" {
                return None;
            }
            let udid = text("/hardwareProperties/udid")?;
            Some(PhoneFacts {
                handle: phone_handle(&udid),
                name: text("/deviceProperties/name").unwrap_or_default(),
                os: text("/deviceProperties/osVersionNumber").unwrap_or_default(),
                address: text("/connectionProperties/tunnelIPAddress"),
                developer_mode: text("/deviceProperties/developerModeStatus"),
                tunnel: text("/connectionProperties/tunnelState"),
                udid,
            })
        })
        .collect()
}

/// Whether `devicectl device info lockState --json-output` says the phone is
/// locked now (t-93254): `passcodeRequired` is what a person must enter before the
/// phone opens — false on an open iPhone, true on the same one locked (2026-11-04);
/// `unlockedSinceBoot` was true both times and says nothing of now. It is read
/// without a runner in 333–1,477 ms (the first call the slowest), so a locked phone
/// is refused before one is started for the tens of seconds a start fails in
/// (20.65 s measured the same day). Anything but a plain true or false is unread,
/// and an unread lock refuses nothing: the runner's own answer still comes after it.
#[must_use]
pub fn locked_in_devicectl(json: &str) -> Option<bool> {
    serde_json::from_str::<Value>(json)
        .ok()?
        .pointer("/result/passcodeRequired")?
        .as_bool()
}

/// The runner's own HTTP base, read off the line it prints once it listens. With
/// `USE_IP` it prints that address as it is, an IPv6 one without brackets
/// (`http://fd00::1:8100`), so the port is what follows the last colon. A line
/// that names no port names no runner.
#[must_use]
pub fn announced_base(line: &str) -> Option<String> {
    let (_, rest) = line.split_once(SERVER_URL_BEGIN)?;
    let url = rest.split(SERVER_URL_END).next()?.trim();
    let host_port = url.strip_prefix("http://")?;
    let (host, port) = host_port.rsplit_once(':')?;
    let port: u16 = port.parse().ok()?;
    let host = host.trim_start_matches('[').trim_end_matches(']');
    (!host.is_empty()).then(|| runner_base(host, port))
}

/// The base of a runner's HTTP address: an IPv6 tunnel address in its brackets.
#[must_use]
pub fn runner_base(address: &str, port: u16) -> String {
    if address.contains(':') {
        format!("http://[{address}]:{port}")
    } else {
        format!("http://{address}:{port}")
    }
}

/// How one readiness item stands. `None` is "this window cannot read that", which
/// is said, and never counted as good.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CheckItem {
    pub name: &'static str,
    pub ok: Option<bool>,
    pub said: String,
}

impl CheckItem {
    #[must_use]
    pub fn new(name: &'static str, ok: Option<bool>, said: impl Into<String>) -> Self {
        Self {
            name,
            ok,
            said: said.into(),
        }
    }
}

/// What a person reads before leaving the Mac: every item and the one word on top.
/// `ready` is true only when nothing is known bad; the unknown are listed apart so
/// a person sees what the word does not cover.
#[must_use]
pub fn check_answer(items: Vec<CheckItem>) -> Value {
    let bad: Vec<&str> = items
        .iter()
        .filter(|item| item.ok == Some(false))
        .map(|item| item.name)
        .collect();
    let unknown: Vec<&str> = items
        .iter()
        .filter(|item| item.ok.is_none())
        .map(|item| item.name)
        .collect();
    json!({
        "ready": bad.is_empty(),
        "bad": bad,
        "unknown": unknown,
        "items": items,
    })
}

/// `pmset -g` and whether the Mac will fall asleep with nobody at it: `sleep 0`
/// is no idle sleep. A display that sleeps does not stop a tunnel, so it is
/// reported apart and is not bad by itself.
#[must_use]
pub fn sleep_minutes(pmset: &str, word: &str) -> Option<u32> {
    pmset.lines().find_map(|line| {
        let mut columns = line.split_whitespace();
        (columns.next()? == word).then(|| columns.next()?.parse().ok())?
    })
}

/// Days since 1970 of an ISO date (`2027-01-02T…`), for how long a signature has
/// left, by the civil-date formula: nothing else here reads a calendar.
#[must_use]
pub fn iso_days(iso: &str) -> Option<i64> {
    let mut parts = iso.get(..10)?.split('-');
    let year: i64 = parts.next()?.parse().ok()?;
    let month: i64 = parts.next()?.parse().ok()?;
    let day: i64 = parts.next()?.parse().ok()?;
    let shifted = if month <= 2 { year - 1 } else { year };
    let era = shifted.div_euclid(400);
    let of_era = shifted.rem_euclid(400);
    let of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let of_cycle = of_era * 365 + of_era / 4 - of_era / 100 + of_year;
    Some(era * 146_097 + of_cycle - 719_468)
}

/// Why `xcodebuild` ended without a runner, in the only words that reach an agent:
/// its log names paths and hardware ids, and stays in the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunnerFailure {
    /// A locked phone cannot start a test runner (LocalAuthentication -4).
    Locked,
    /// The phone has not trusted this developer, or a signature has run out.
    Signature,
    /// Developer Mode is off, or a disk image would not mount.
    DeveloperMode,
    Other,
}

/// Which failure a run's log tail shows.
#[must_use]
pub fn runner_failure(log: &str) -> RunnerFailure {
    if log.contains("LocalAuthentication") || log.contains("Failed to initialize for UI testing") {
        RunnerFailure::Locked
    } else if log.contains("Untrusted Developer")
        || log.contains("not been explicitly trusted")
        || log.contains("provisioning profile")
        || log.contains("expired")
    {
        RunnerFailure::Signature
    } else if log.contains("Developer Mode") || log.contains("Developer disk image") {
        RunnerFailure::DeveloperMode
    } else {
        RunnerFailure::Other
    }
}

/// What a failure asks of the person.
#[must_use]
pub const fn runner_failure_said(failure: RunnerFailure) -> &'static str {
    match failure {
        RunnerFailure::Locked => "the phone is locked; only the person can unlock it",
        RunnerFailure::Signature => {
            "the phone does not trust this build's signature or it ran out: the person trusts the developer in the phone's Settings, or runs phone-install again"
        }
        RunnerFailure::DeveloperMode => {
            "Developer Mode is off on the phone; only the person can turn it on"
        }
        RunnerFailure::Other => "xcodebuild ended before the runner came up",
    }
}

/// The date a provisioning profile ends, from `security cms -D` of it: the plist's
/// `ExpirationDate` as an ISO time, compared as text by the caller's clock.
#[must_use]
pub fn profile_expiry(plist_xml: &str) -> Option<String> {
    let after = plist_xml.split("<key>ExpirationDate</key>").nth(1)?;
    let open = after.find("<date>")? + "<date>".len();
    let close = after.find("</date>")?;
    after.get(open..close).map(str::to_string)
}

#[cfg(test)]
mod tests;
