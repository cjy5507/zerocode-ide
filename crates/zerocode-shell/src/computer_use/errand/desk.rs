//! The world a goal walk presses in: the roads it already drives.
//!
//! Nothing new reaches a screen from here, and that is the whole point. A
//! look is the surface's own marked look, a press is `click --mark <n>`, and
//! the success check is the caller's own words put through the check verb the
//! guarded money path already uses as its witness — so the pin, the
//! fingerprint, the person's stop and the door's budget stand in front of a
//! goal walk exactly as they stand in front of a step somebody typed.
//!
//! What a goal walk may NOT do is as load-bearing as what it may:
//!
//! - It presses. It does not type, navigate, run a script or set a value —
//!   the answer space is a number and nothing else, so there is no road from
//!   a judgment to a value, a selector or an address.
//! - It stays where it was aimed. A desktop walk names one app and looks only
//!   at that app's tree; a pane walk names one pane. A press that carries the
//!   screen somewhere else shows up as a screen that changed, which the walk
//!   reads, and never as a second surface quietly joining the walk.
//! - It verifies with the caller's words, never its own. [`Aim::reached`]
//!   asks the check verb for a piece of text the CALLER wrote down before the
//!   walk started; no screen's text is compiled in here, and the judgment's
//!   own `done` is recorded as the weaker end that it is.

use std::time::Instant;

use serde_json::{Value, json};
use zerocode_core::computer_recipe::{RecipeTool, recipe_line_holds_ms};
use zerocode_core::computer_use::{EmulatorPlatform, FLOW_BASELINE_PROBE_MS};
use zerocode_core::computer_use_protocol::marks::{ITEMS_KEY, LOOK_ID_KEY};
use zerocode_hookd::TeamAnswer;

use super::{Saved, Screen, Seen, Surface, World};

/// The flag every road here asks its answer as JSON with — the word both
/// CLIs already take, spelled once.
const JSON_FLAG: &str = "--json";

/// What a forked step's saved state is called on the device: this, then a
/// short random tail, so two forks of one device never share a name and a
/// state left behind by a window that died is recognisable for what it is.
const FORK_SNAPSHOT_PREFIX: &str = "zerocode-fork-";

/// A device's saved states, as a forked step drives them (t-6044) — the seam
/// the Android AVD snapshot road sits behind, and a test replaces.
///
/// Only Android has one: an AVD saves and loads a named snapshot through the
/// emulator's own console, an iOS simulator has no such road, and a page or
/// the desktop is not a device. A world with none of these answers
/// [`World::save`] with `None`, and the step is taken once, as today.
pub trait Snapshots {
    /// Save the device as `name`; answer how long it took.
    ///
    /// # Errors
    /// The device's own words for why it could not.
    fn save(&mut self, name: &str) -> Result<u64, String>;
    /// Put the device back to `name`.
    ///
    /// # Errors
    /// The device's own words for why it could not.
    fn load(&mut self, name: &str) -> Result<u64, String>;
    /// Delete `name` from the device. Best effort: a state that stays costs
    /// disk, not correctness.
    fn delete(&mut self, name: &str);
}

/// The flag a look takes when nobody will open its picture.
const NO_PICTURE_FLAG: &str = "--no-screenshot";

/// What a goal walk is aimed at — one surface, named once, for the whole walk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Aim {
    /// A browser pane, by the label the pane answers to.
    Pane { label: String },
    /// One app on the desktop, by its name.
    App { name: String },
    /// One mobile device; every look, press and check retains this target.
    Phone {
        platform: EmulatorPlatform,
        device: String,
    },
}

fn phone_argv(verb: &str, platform: EmulatorPlatform, device: &str) -> Vec<String> {
    [
        verb,
        "--platform",
        platform.as_str(),
        "--device",
        device,
        JSON_FLAG,
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

/// The lines of a surface's text answer, as the screen carries them —
/// trimmed, blanks dropped; the question does the cutting.
fn shows_of(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

fn answer_value(answer: &TeamAnswer) -> Option<Value> {
    if answer.exit_code != 0 {
        return None;
    }
    let said: Value = serde_json::from_str(answer.stdout.trim()).ok()?;
    if said.get("ok") == Some(&Value::Bool(false)) {
        return None;
    }
    Some(said.get("result").cloned().unwrap_or(said))
}

fn read_unjudged(
    road: &mut impl FnMut(RecipeTool, &[String], &[String]) -> TeamAnswer,
    tool: RecipeTool,
    argv: &[String],
) -> TeamAnswer {
    crate::run_evidence::observing(json!({"judgment": {"asked": false}}), || {
        road(tool, argv, argv)
    })
}

impl Aim {
    /// Which seat's consent this walk is under.
    #[must_use]
    pub const fn surface(&self) -> Surface {
        match self {
            Self::Pane { .. } => Surface::Page,
            Self::App { .. } => Surface::Desk,
            Self::Phone { .. } => Surface::Phone,
        }
    }

    const fn tool(&self) -> RecipeTool {
        match self {
            Self::Pane { .. } => RecipeTool::Browser,
            Self::App { .. } => RecipeTool::Computer,
            Self::Phone { .. } => RecipeTool::Emulator,
        }
    }

    fn look_argv(&self) -> Vec<String> {
        match self {
            Self::Pane { label } => vec!["marks".to_string(), label.clone(), JSON_FLAG.to_string()],
            Self::Phone { platform, device } => phone_argv("marks", *platform, device),
            // `--no-screenshot`: a walk presses by number and never opens
            // the picture, and drawing ninety-nine badges on the frame and
            // encoding a PNG is 262 ms of a 645 ms look (measured 2026-09-18,
            // `marks::draw_answer`).
            Self::App { name } => vec![
                "observe".to_string(),
                "--app".to_string(),
                name.clone(),
                "--marks".to_string(),
                NO_PICTURE_FLAG.to_string(),
                JSON_FLAG.to_string(),
            ],
        }
    }

    /// `look` is the id the marked look handed out: the desktop door refuses a
    /// number without the look it was read from, because a number means
    /// nothing apart from the table that drew it. A pane's label IS that
    /// identity, so the browser door asks for no id.
    /// The verb that reads the screen's own text beside its controls, when
    /// this surface has one worth a round trip. The desktop reads its
    /// window's accessibility text (`read --app`): a calculator's display,
    /// a dialog's message — what a walk pressed the wrong key for want of
    /// (t-5497, 3/4 wrong on "7×6=42"). A page's legend already names
    /// its controls and the browser walk read 10/10 without it (2026-09-20),
    /// and a phone's `tree` is a heavier answer than its marks — both are
    /// `None` until a measurement says otherwise.
    fn shows_argv(&self) -> Option<Vec<String>> {
        match self {
            Self::App { name } => Some(vec![
                "read".to_string(),
                "--app".to_string(),
                name.clone(),
                JSON_FLAG.to_string(),
            ]),
            Self::Pane { .. } | Self::Phone { .. } => None,
        }
    }

    fn press_argv(&self, mark: usize, look: &str) -> Vec<String> {
        match self {
            Self::Phone { platform, device } => {
                let mut argv = phone_argv("click", *platform, device);
                argv.extend([
                    "--mark".into(),
                    mark.to_string(),
                    "--look".into(),
                    look.into(),
                ]);
                argv
            }
            Self::Pane { label } => vec![
                "click".to_string(),
                label.clone(),
                "--mark".to_string(),
                mark.to_string(),
            ],
            // No `--app`: a mark already names its own app, window and
            // element, and the door refuses a press that names one again.
            Self::App { .. } => vec![
                "click".to_string(),
                "--mark".to_string(),
                mark.to_string(),
                "--look".to_string(),
                look.to_string(),
            ],
        }
    }

    /// The check verb that says whether the caller's own condition holds, at
    /// no wait — the guarded money path's witness, asked the same way.
    fn reached_argv(&self, until: &str) -> Vec<String> {
        match self {
            Self::Phone { platform, device } => {
                let mut argv = phone_argv("find", *platform, device);
                argv.extend(["--text".into(), until.into()]);
                argv
            }
            Self::Pane { label } => vec!["find".to_string(), label.clone(), until.to_string()],
            Self::App { name } => vec![
                "wait-for".to_string(),
                "--app".to_string(),
                name.clone(),
                "--text".to_string(),
                until.to_string(),
                "--timeout-ms".to_string(),
                FLOW_BASELINE_PROBE_MS.to_string(),
            ],
        }
    }
}

/// The screen a marked look answered with, and the id that look handed out.
/// Pure: the two surfaces' answers differ in shape and this is the one place
/// that knows how.
#[must_use]
pub fn screen_of(aim: &Aim, said: &Value) -> Option<(Screen, String)> {
    match aim {
        Aim::Phone { platform, device } => {
            let look = said.get(LOOK_ID_KEY)?.as_str()?.trim();
            if look.is_empty() {
                return None;
            }
            Some((
                Screen {
                    at: Seen::Phone {
                        platform: *platform,
                        device: device.clone(),
                    },
                    items: said.get(ITEMS_KEY)?.as_array()?.clone(),
                    shows: Vec::new(),
                },
                look.to_string(),
            ))
        }
        Aim::Pane { .. } => Some((
            Screen {
                // A pane's address is not in its `marks` answer, and a second
                // round trip for it would spend the clock the walk still needs;
                // the caller reads it once before the walk and hands it in.
                at: Seen::default(),
                items: said.get(ITEMS_KEY)?.as_array()?.clone(),
                shows: Vec::new(),
            },
            String::new(),
        )),
        Aim::App { name } => {
            let marks = said.get("marks")?;
            Some((
                Screen {
                    at: Seen::Desk {
                        app: marks
                            .get("app")
                            .and_then(Value::as_str)
                            .unwrap_or(name)
                            .to_string(),
                        window: said
                            .pointer("/tree/window/title")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                    },
                    items: marks.get(ITEMS_KEY)?.as_array()?.clone(),
                    shows: Vec::new(),
                },
                marks
                    .get(LOOK_ID_KEY)
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            ))
        }
    }
}

/// The roads a goal walk drives, as a world.
pub struct GoalWorld<'a, Road> {
    road: &'a mut Road,
    aim: Aim,
    /// The pane's address, read once before the walk (a `marks` answer does
    /// not carry it). Empty for a desktop walk, which reads its own.
    page: Seen,
    /// What the caller wrote down as "this worked", if they wrote one.
    until: Option<String>,
    /// The id the last look handed out — what a press by number is read
    /// against. A press before any look has none, and the door refuses it.
    look: String,
    /// The call's whole clock, and what was spent before the walk began.
    deadline_ms: u64,
    spent_ms: u64,
    began: Instant,
    /// The device's saved states, when this surface has them (t-6044).
    snapshots: Option<Box<dyn Snapshots>>,
}

impl<'a, Road> GoalWorld<'a, Road> {
    pub fn new(
        road: &'a mut Road,
        aim: Aim,
        page: Seen,
        until: Option<String>,
        deadline_ms: u64,
        spent_ms: u64,
    ) -> Self {
        Self {
            road,
            aim,
            page,
            until,
            look: String::new(),
            deadline_ms,
            spent_ms,
            began: Instant::now(),
            snapshots: None,
        }
    }

    /// The same world, able to save and load the device it is aimed at — an
    /// Android AVD's snapshot road, or a test's stand-in. A world aimed at
    /// anything but an Android device keeps no road however it is built: a
    /// forked step never saves what it cannot load.
    #[must_use]
    pub fn with_snapshots(mut self, snapshots: Box<dyn Snapshots>) -> Self {
        if matches!(
            self.aim,
            Aim::Phone {
                platform: EmulatorPlatform::Android,
                ..
            }
        ) {
            self.snapshots = Some(snapshots);
        }
        self
    }
}

impl<Road> World for GoalWorld<'_, Road>
where
    Road: FnMut(RecipeTool, &[String], &[String]) -> TeamAnswer,
{
    fn look(&mut self) -> Option<Screen> {
        let argv = self.aim.look_argv();
        let answer = read_unjudged(self.road, self.aim.tool(), &argv);
        let said = answer_value(&answer)?;
        // A CLI answer is `{ ok, result }` when it went through the door's
        // envelope and the bare answer when it did not; both are read here so
        // the walk is not one envelope's prisoner.
        let (mut screen, look) = screen_of(&self.aim, &said)?;
        if matches!(self.aim, Aim::Pane { .. }) {
            screen.at = self.page.clone();
        }
        if let Some(argv) = self.aim.shows_argv() {
            let answer = read_unjudged(self.road, self.aim.tool(), &argv);
            screen.shows = answer_value(&answer)
                .and_then(|read| Some(shows_of(read.get("text")?.as_str()?)))
                .unwrap_or_default();
        }
        self.look = look;
        Some(screen)
    }

    fn press(&mut self, mark: usize) -> bool {
        // By number, never by selector or a point: the surface re-measures the
        // element it handed that number to and refuses a press whose pin no
        // longer holds.
        let argv = self.aim.press_argv(mark, &self.look);
        let holds = recipe_line_holds_ms(self.aim.tool(), &argv);
        let left = self.left_ms();
        if left == 0 || left < holds {
            return false;
        }
        (self.road)(self.aim.tool(), &argv, &argv).exit_code == 0
    }

    fn reached(&mut self) -> Option<bool> {
        let until = self.until.clone()?;
        let argv = self.aim.reached_argv(&until);
        let answer = read_unjudged(self.road, self.aim.tool(), &argv);
        if matches!(self.aim, Aim::App { .. }) {
            Some(answer.exit_code == 0)
        } else {
            Some(
                answer_value(&answer)
                    .and_then(|value| value.get("count").and_then(Value::as_u64))
                    .is_some_and(|count| count > 0),
            )
        }
    }

    fn left_ms(&mut self) -> u64 {
        let spent = self
            .spent_ms
            .saturating_add(u64::try_from(self.began.elapsed().as_millis()).unwrap_or(u64::MAX));
        self.deadline_ms.saturating_sub(spent)
    }

    fn save(&mut self) -> Option<Saved> {
        let snapshots = self.snapshots.as_mut()?;
        let name = format!(
            "{FORK_SNAPSHOT_PREFIX}{}",
            &uuid::Uuid::new_v4().simple().to_string()[..8]
        );
        match snapshots.save(&name) {
            Ok(took_ms) => Some(Saved { name, took_ms }),
            Err(why) => {
                eprintln!("walk: the device was not saved for a fork: {why}");
                None
            }
        }
    }

    fn restore(&mut self, saved: &Saved) -> bool {
        let Some(snapshots) = self.snapshots.as_mut() else {
            return false;
        };
        match snapshots.load(&saved.name) {
            Ok(_) => true,
            Err(why) => {
                eprintln!("walk: the device was not put back after a fork: {why}");
                false
            }
        }
    }

    fn forget(&mut self, saved: &Saved) {
        if let Some(snapshots) = self.snapshots.as_mut() {
            snapshots.delete(&saved.name);
        }
    }
}

#[cfg(test)]
mod tests;
