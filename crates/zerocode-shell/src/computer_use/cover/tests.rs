//! The hand between the cover seat and the helper, on a stand-in helper
//! that keeps a window list and moves its windows as the real one does —
//! no real window, pointer or key moves.

use std::time::Duration;

use serde_json::{Value, json};
use zerocode_core::jev::JevMode;
use zerocode_core::jev::cover::{CoverAsk, Held, Move};
use zerocode_core::jev::questions::{COVER_COVERED, COVER_KIND, COVER_MOVE, COVER_REVERSIBLE};
use zerocode_core::jev::reflex_decide::Wired;

use super::*;
use crate::computer_use::ComputerUseError;
use crate::computer_use::confirm::Decision;

const TARGET: u64 = 5;
const OVER: u64 = 7;

/// A window list, front to back, as `listAllWindows` answers it with every
/// layer, and what was asked of it.
struct Desk {
    windows: Vec<Value>,
    acted: Vec<(String, Value)>,
}

fn row(id: u64, pid: i64, app: &str, layer: i64, rect: (f64, f64, f64, f64)) -> Value {
    json!({
        "id": id, "title": "never read", "app": { "name": app, "pid": pid },
        "x": rect.0, "y": rect.1, "width": rect.2, "height": rect.3,
        "own": false, "layer": layer, "alpha": 1.0, "overlay": false,
    })
}

impl Desk {
    /// The target at (100, 100) 400×300 under another app's window at
    /// `layer` over its middle.
    fn with_over(layer: i64) -> Self {
        Self {
            windows: vec![
                row(OVER, 20, "Other", layer, (200.0, 150.0, 300.0, 200.0)),
                row(TARGET, 10, "Target", 0, (100.0, 100.0, 400.0, 300.0)),
            ],
            acted: Vec::new(),
        }
    }

    fn call(&mut self, method: &str, params: Value) -> Result<Value, ComputerUseError> {
        match method {
            "listAllWindows" => Ok(json!({ "windows": self.windows })),
            "displays" => Ok(json!({ "displays": [
                { "index": 0, "bounds": { "x": 0.0, "y": 0.0, "width": 1440.0, "height": 900.0 } }
            ] })),
            "windowAction" => {
                self.acted.push((method.to_string(), params.clone()));
                let id = params["windowId"].as_u64().unwrap_or_default();
                let at = self
                    .windows
                    .iter()
                    .position(|window| window["id"] == id)
                    .expect("a listed window");
                match params["action"].as_str() {
                    // To the front of its own layer: what stays above
                    // ordinary windows stays above it.
                    Some("focus") => {
                        let window = self.windows.remove(at);
                        let layer = window["layer"].as_i64().unwrap_or_default();
                        let under = self
                            .windows
                            .iter()
                            .position(|other| other["layer"].as_i64().unwrap_or_default() <= layer)
                            .unwrap_or(self.windows.len());
                        self.windows.insert(under, window);
                    }
                    Some("move") => {
                        self.windows[at]["x"] = params["x"].clone();
                        self.windows[at]["y"] = params["y"].clone();
                    }
                    other => panic!("no window action {other:?}"),
                }
                Ok(json!({}))
            }
            other => panic!("the hand asked the helper for {other}"),
        }
    }

    fn moved(&self) -> Vec<(u64, String)> {
        self.acted
            .iter()
            .map(|(_, params)| {
                (
                    params["windowId"].as_u64().unwrap_or_default(),
                    params["action"].as_str().unwrap_or_default().to_string(),
                )
            })
            .collect()
    }
}

/// The seat as a test stands it: its word, whether it acts, and one canned
/// wire result.
struct Seat {
    mode: JevMode,
    applies: bool,
    wired: Result<String, String>,
    /// Requests that left: 0 for a question the door refused.
    attempts: u32,
    asked: usize,
    rows: Vec<Value>,
}

impl Seat {
    fn at(mode: JevMode, applies: bool, wired: Result<String, String>) -> Self {
        Self {
            mode,
            applies,
            wired,
            attempts: 1,
            asked: 0,
            rows: Vec::new(),
        }
    }
}

impl Judge for Seat {
    fn standing(&mut self) -> (JevMode, bool, Option<u16>) {
        (self.mode, self.applies, None)
    }

    fn ask(&mut self, _asked: &CoverAsk) -> Wired {
        self.asked += 1;
        Wired {
            answer: self.wired.clone(),
            attempts: self.attempts,
            request_bytes: 100,
            rtt_ms: 300,
        }
    }

    fn record(&mut self, rows: Vec<Value>) {
        self.rows.extend(rows);
    }
}

fn choice(options: &[(&str, f64)], confidence: f64) -> Value {
    let chosen = options
        .iter()
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map_or("", |(word, _)| *word);
    json!({
        "type": "choice", "choice": chosen, "confidence": confidence,
        "probabilities": options.iter().map(|(word, share)| ((*word).to_string(), json!(share))).collect::<serde_json::Map<_, _>>(),
    })
}

/// An endpoint body: `kind` for what stands in front, `moves` ranked.
fn body(kind: &str, moves: &[(&str, f64)]) -> Result<String, String> {
    let kinds: Vec<(&str, f64)> = ["window", "panel", "system_dialog", "modal", "unknown"]
        .into_iter()
        .map(|word| (word, if word == kind { 0.96 } else { 0.01 }))
        .collect();
    Ok(json!({
        "model": "jev-test",
        "answers": {
            (COVER_COVERED): { "type": "noul", "noul": 0.97 },
            (COVER_KIND): choice(&kinds, 0.9),
            (COVER_MOVE): choice(moves, 0.8),
            (COVER_REVERSIBLE): { "type": "noul", "noul": 0.95 },
        },
    })
    .to_string())
}

const PRESS_AT_THE_MIDDLE: Place = Place {
    window: TARGET,
    local: Some(Rect::new(190.0, 140.0, 20.0, 20.0)),
    needs: Needs::Centre,
};

/// Run the hand on `desk` under `seat`, answering the person with
/// `person` (and, when they say they are done, taking `cleared` off the
/// desk first); the lines the person was shown.
fn run(
    desk: &mut Desk,
    seat: &mut Seat,
    person: Decision,
    cleared: Option<u64>,
) -> (Result<Uncovered, ComputerUseError>, Vec<Said>) {
    let mut shown = Vec::new();
    let desk_cell = std::cell::RefCell::new(desk);
    let ended = {
        let mut call = |method: &str, params: Value| desk_cell.borrow_mut().call(method, params);
        let mut pause = |_wait: Duration| {};
        let mut ask_person = |line: &Said| {
            shown.push(line.clone());
            if person == Decision::Allowed
                && let Some(gone) = cleared
            {
                desk_cell
                    .borrow_mut()
                    .windows
                    .retain(|window| window["id"] != gone);
            }
            person
        };
        let wall = || 1_790_000_000_000_i64;
        uncover(
            PRESS_AT_THE_MIDDLE,
            &mut Hand {
                call: &mut call,
                pause: &mut pause,
                person: &mut ask_person,
                wall_ms: &wall,
            },
            seat,
        )
    };
    (ended, shown)
}

/// Another app's ordinary window over the place, and no seat: the target's
/// window comes to the front, the list is read again, and the place shows —
/// the window in front was never moved, closed or read.
#[test]
fn another_apps_window_over_the_place_is_raised_checked_and_left_clear() {
    let mut desk = Desk::with_over(0);
    let mut seat = Seat::at(JevMode::Off, false, Err("off".into()));
    let (ended, shown) = run(&mut desk, &mut seat, Decision::Refused, None);
    assert_eq!(
        ended.expect("listed"),
        Uncovered::Clear {
            moves: vec![Move::RaiseTarget],
            by_person: false
        }
    );
    assert_eq!(desk.moved(), [(TARGET, "focus".to_string())]);
    assert!(shown.is_empty());
    assert_eq!(seat.asked, 0, "an off seat is asked nothing");
    assert_eq!(desk.windows[0]["id"], TARGET);
}

/// A panel that stays above ordinary windows: the answer put the front
/// first and the move second; the front leaves it covered, so the hand makes
/// the runner-up with no second question and the place shows. The label
/// says the first move was wrong.
#[test]
fn a_panel_that_stays_on_top_takes_the_runner_up_its_answer_ranked() {
    let mut desk = Desk::with_over(3);
    let mut seat = Seat::at(
        JevMode::Auto,
        true,
        body(
            "panel",
            &[
                ("raise_target", 0.6),
                ("move_target", 0.3),
                ("look_again", 0.05),
                ("ask_person", 0.05),
            ],
        ),
    );
    let (ended, shown) = run(&mut desk, &mut seat, Decision::Refused, None);
    assert_eq!(
        ended.expect("listed"),
        Uncovered::Clear {
            moves: vec![Move::RaiseTarget, Move::MoveTarget],
            by_person: false
        }
    );
    assert!(shown.is_empty());
    assert_eq!(seat.asked, 1, "one question for the whole scene");
    assert_eq!(
        desk.moved(),
        [(TARGET, "focus".to_string()), (TARGET, "move".to_string())]
    );
    assert!(desk.moved().iter().all(|(id, _)| *id != OVER));
    let label = seat.rows.last().expect("a label");
    assert_eq!(label["agreed"], false, "{label}");
    assert_eq!(label["clearedBy"], "move_target");
    assert_eq!(seat.rows[0]["applied"], true);
    assert!(
        !seat.rows[0].to_string().contains("never read"),
        "no title reaches a row"
    );
}

/// The seat acts and its answer does not come inside the wall: the hand
/// stays still — no window moves — and the person is asked once, in the
/// page's words; the press does not go on under the old plan.
#[test]
fn a_silent_seat_holds_the_hand_and_asks_the_person_once() {
    let mut desk = Desk::with_over(0);
    let mut seat = Seat::at(JevMode::Auto, true, Err("timeout".into()));
    let (ended, shown) = run(&mut desk, &mut seat, Decision::TimedOut, None);
    assert_eq!(
        ended.expect("listed"),
        Uncovered::Held {
            held: Held::Unanswered,
            over: "Other".into()
        }
    );
    assert!(desk.acted.is_empty(), "{:?}", desk.acted);
    assert_eq!(shown.len(), 1);
    assert_eq!(shown[0].key, "computer.cover.unanswered");
    assert_eq!(shown[0].args, json!({ "app": "Other" }));
    assert_eq!(seat.rows[0]["held"], "unanswered");
    assert_eq!(seat.rows[0]["outcome"], "timeout");
}

/// The system's window over the place is never acted on: nothing moves,
/// the person is asked, and once they say they are done the list is read
/// again and the place shows.
#[test]
fn the_systems_window_over_the_place_is_left_to_the_person() {
    let mut desk = Desk::with_over(0);
    let mut seat = Seat::at(
        JevMode::On,
        true,
        body(
            "system_dialog",
            &[
                ("raise_target", 0.7),
                ("move_target", 0.2),
                ("look_again", 0.05),
                ("ask_person", 0.05),
            ],
        ),
    );
    let (ended, shown) = run(&mut desk, &mut seat, Decision::Allowed, Some(OVER));
    assert_eq!(
        ended.expect("listed"),
        Uncovered::Clear {
            moves: Vec::new(),
            by_person: true
        }
    );
    assert!(desk.acted.is_empty(), "{:?}", desk.acted);
    assert_eq!(shown.len(), 1);
    assert_eq!(shown[0].key, "computer.cover.ask");
    assert!(!shown[0].reason.contains("never read"));
}

/// A place whose centre shows needs nothing: no question, no move.
#[test]
fn a_place_whose_centre_shows_asks_nothing() {
    let mut desk = Desk::with_over(0);
    desk.windows[0] = row(OVER, 20, "Other", 0, (100.0, 100.0, 50.0, 50.0));
    let mut seat = Seat::at(JevMode::Auto, true, Err("unused".into()));
    let (ended, shown) = run(&mut desk, &mut seat, Decision::Refused, None);
    assert_eq!(
        ended.expect("listed"),
        Uncovered::Clear {
            moves: Vec::new(),
            by_person: false
        }
    );
    assert_eq!(seat.asked, 0);
    assert!(desk.acted.is_empty() && shown.is_empty() && seat.rows.is_empty());
}

/// Under `shadow` the answer is kept and the hand makes today's rule's
/// moves: here it asked for the person where the front cleared the place,
/// and its label says so.
#[test]
fn shadow_keeps_its_answer_and_the_hand_makes_todays_moves() {
    let mut desk = Desk::with_over(0);
    let mut seat = Seat::at(
        JevMode::Shadow,
        false,
        body(
            "window",
            &[
                ("ask_person", 0.6),
                ("raise_target", 0.3),
                ("move_target", 0.05),
                ("look_again", 0.05),
            ],
        ),
    );
    let (ended, shown) = run(&mut desk, &mut seat, Decision::Refused, None);
    assert_eq!(
        ended.expect("listed"),
        Uncovered::Clear {
            moves: vec![Move::RaiseTarget],
            by_person: false
        }
    );
    assert!(shown.is_empty());
    assert_eq!(seat.rows[0]["applied"], false);
    let label = &seat.rows[1];
    assert_eq!(
        (label["agreed"].clone(), label["baselineAgreed"].clone()),
        (json!(false), json!(true)),
        "{label}"
    );
}

/// The target's window is not on the screen: nothing to move, the person is
/// asked.
#[test]
fn a_window_not_on_the_screen_is_the_persons() {
    let mut desk = Desk::with_over(0);
    desk.windows.retain(|window| window["id"] != TARGET);
    let mut seat = Seat::at(JevMode::Auto, true, Err("unused".into()));
    let (ended, shown) = run(&mut desk, &mut seat, Decision::Refused, None);
    assert_eq!(shown.len(), 1, "the person is asked");
    assert_eq!(
        ended.expect("listed"),
        Uncovered::Held {
            held: Held::Gone,
            over: String::new()
        }
    );
    assert_eq!(shown[0].key, "computer.cover.gone");
    assert_eq!(seat.asked, 0);
}

/// The door refusing the question — the switch, the folder, the day's count
/// — is the seat not asked here: the hand makes today's rule's moves, and the
/// row says why nothing came back.
#[test]
fn a_door_that_refuses_leaves_the_hand_to_todays_rule() {
    let mut desk = Desk::with_over(0);
    let mut seat = Seat::at(JevMode::Auto, true, Err("workspace".into()));
    seat.attempts = 0;
    let (ended, shown) = run(&mut desk, &mut seat, Decision::Refused, None);
    assert_eq!(
        ended.expect("listed"),
        Uncovered::Clear {
            moves: vec![Move::RaiseTarget],
            by_person: false
        }
    );
    assert!(shown.is_empty());
    assert_eq!(seat.rows[0]["outcome"], "workspace");
    assert_eq!(seat.rows[0]["applied"], false);
}

// ---- a press by a look's mark (the road 26,124 steps a week take) ----------

/// What the helper says when a mark's centre is under something: its words
/// may name what the covering window shows, and none of them may travel.
const HELPERS_WORDS: &str =
    "element 3 is covered at its centre by AXWindow Secret Title; look again with --marks";

fn mark() -> PinnedClick {
    PinnedClick {
        params: serde_json::Map::new(),
        mark: 3,
        look: "1.1".into(),
        role: "button".into(),
        label: Some("Save".into()),
        element_index: 12,
        app: "Target".into(),
        frame: Rect::new(290.0, 240.0, 20.0, 20.0),
        window_id: TARGET,
        local: Rect::new(190.0, 140.0, 20.0, 20.0),
    }
}

/// Press `mark()` on `desk` as the helper would: refused while its centre is
/// under another window, pressed once it shows. Returns the answer, how many
/// presses went, and the lines the person saw.
fn press_on(
    desk: &mut Desk,
    seat: &mut Seat,
    person: Decision,
) -> (Result<Value, ComputerUseError>, usize, Vec<Said>) {
    let desk_cell = std::cell::RefCell::new(desk);
    let mut presses = 0;
    let mut shown = Vec::new();
    let answered = {
        let covered = || {
            let listed: Vec<zerocode_core::computer_use_protocol::marks::DesktopWindow> = desk_cell
                .borrow()
                .windows
                .iter()
                .filter_map(zerocode_core::computer_use_protocol::marks::DesktopWindow::from_row)
                .collect();
            zerocode_core::computer_use_protocol::cover::cover_of(
                &listed,
                TARGET,
                Some(mark().local),
            )
            .is_some_and(|cover| cover.blocks_a_press())
        };
        let mut press = || {
            presses += 1;
            if covered() {
                Err(ComputerUseError::new(
                    zerocode_core::computer_use_protocol::error_code::ELEMENT_NOT_FOUND,
                    HELPERS_WORDS,
                ))
            } else {
                Ok(json!({ "action": { "path": "accessibility" } }))
            }
        };
        let mut call = |method: &str, params: Value| desk_cell.borrow_mut().call(method, params);
        let mut pause = |_wait: Duration| {};
        let mut ask_person = |line: &Said| {
            shown.push(line.clone());
            person
        };
        let wall = || 1_790_000_000_000_i64;
        press_mark(
            &mark(),
            &mut press,
            &mut Hand {
                call: &mut call,
                pause: &mut pause,
                person: &mut ask_person,
                wall_ms: &wall,
            },
            seat,
        )
    };
    (answered, presses, shown)
}

/// A mark under a panel that stays on top: the helper refuses it as not
/// found; the hand reads the window list, stops, asks once, brings the
/// target's window to the front and then moves it clear, reads the list
/// again, and presses the mark — and says what it moved.
#[test]
fn a_mark_under_a_panel_is_uncovered_and_pressed() {
    let mut desk = Desk::with_over(3);
    let mut seat = Seat::at(
        JevMode::Auto,
        true,
        body(
            "panel",
            &[
                ("raise_target", 0.6),
                ("move_target", 0.3),
                ("look_again", 0.05),
                ("ask_person", 0.05),
            ],
        ),
    );
    let (answered, presses, shown) = press_on(&mut desk, &mut seat, Decision::Refused);
    assert_eq!(presses, 2, "refused, then pressed");
    let answer = answered.expect("pressed once uncovered");
    assert_eq!(
        answer["uncovered"],
        json!({ "moves": ["raise_target", "move_target"], "byPerson": false })
    );
    assert!(shown.is_empty());
    assert!(desk.moved().iter().all(|(id, _)| *id != OVER));
}

/// The system's window over the mark: nothing moves, the person is asked,
/// and the press is refused as covered — naming whose window it is, never a
/// word of the helper's refusal, in the answer, the rows or the person's line.
#[test]
fn a_covered_mark_is_refused_as_covered_in_words_of_whose_window_alone() {
    // Above ordinary windows, as the system's alerts stand: the helper's own
    // bringing-forward before a press does not clear it.
    let mut desk = Desk::with_over(3);
    let mut seat = Seat::at(
        JevMode::Auto,
        true,
        body(
            "system_dialog",
            &[
                ("ask_person", 0.7),
                ("raise_target", 0.2),
                ("move_target", 0.05),
                ("look_again", 0.05),
            ],
        ),
    );
    let (answered, presses, shown) = press_on(&mut desk, &mut seat, Decision::TimedOut);
    let refused = answered.expect_err("the person did not clear it");
    assert_eq!(
        refused.code,
        zerocode_core::computer_use_protocol::error_code::COVERED
    );
    assert!(
        refused.message.contains("a window of Other"),
        "{}",
        refused.message
    );
    assert!(!refused.message.contains("moved or changed"));
    assert_eq!(presses, 1);
    assert!(desk.acted.is_empty());
    let everything = format!(
        "{} {:?} {}",
        refused.message,
        shown,
        Value::Array(seat.rows.clone())
    );
    assert!(!everything.contains("Secret Title"), "{everything}");
}

/// A refusal with nothing over the mark's centre was the pin's own: it
/// stands as the helper said it, nothing is asked and nothing moves.
#[test]
fn a_refusal_with_nothing_over_the_mark_stands() {
    let mut desk = Desk::with_over(0);
    let mut seat = Seat::at(JevMode::Auto, true, Err("unused".into()));
    let mut presses = 0;
    let answered = {
        let mut press = || {
            presses += 1;
            Err(ComputerUseError::new(
                zerocode_core::computer_use_protocol::error_code::ELEMENT_NOT_FOUND,
                "element 3 moved",
            ))
        };
        desk.windows.remove(0);
        let desk_cell = std::cell::RefCell::new(&mut desk);
        let mut call = |method: &str, params: Value| desk_cell.borrow_mut().call(method, params);
        let mut pause = |_wait: Duration| {};
        let mut ask_person = |_line: &Said| Decision::Refused;
        let wall = || 1_790_000_000_000_i64;
        press_mark(
            &mark(),
            &mut press,
            &mut Hand {
                call: &mut call,
                pause: &mut pause,
                person: &mut ask_person,
                wall_ms: &wall,
            },
            &mut seat,
        )
    };
    let refused = answered.expect_err("stands");
    assert_eq!(refused.message, "element 3 moved");
    assert_eq!(presses, 1);
    assert_eq!(seat.asked, 0);
    assert!(desk.acted.is_empty());
}

// ---- scenes nobody wrote the code for ------------------------------------

/// A small deterministic stream for scenes drawn from a seed: xorshift64*.
struct Draw(u64);

impl Draw {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// A whole number in `low..=high`.
    fn int(&mut self, low: u64, high: u64) -> u64 {
        low + self.next() % (high - low + 1)
    }

    /// A whole number of points in `low..=high`.
    fn pt(&mut self, low: u64, high: u64) -> f64 {
        // Screen points, far inside f64's whole numbers.
        #[allow(clippy::cast_precision_loss)]
        let drawn = self.int(low, high) as f64;
        drawn
    }
}

/// Two hundred scenes drawn from seeds — the target anywhere on a
/// 1440 × 900 screen, one to three windows of this app or others at the
/// ordinary layer or above it, anywhere — walked by today's rule: no move
/// ever names a window but the target's, a place said to be clear is clear
/// on the list the stand-in helper holds, a hold for the person is one the
/// facts call for, and the ordinary layer alone is never left to the person.
/// The code carries no scene: it holds on every one of them.
#[test]
fn the_hand_holds_its_lines_on_scenes_drawn_from_seeds() {
    use zerocode_core::computer_use_protocol::cover::{Owner, cover_of};
    use zerocode_core::computer_use_protocol::marks::DesktopWindow;
    let listed = |desk: &Desk| -> Vec<DesktopWindow> {
        desk.windows
            .iter()
            .filter_map(DesktopWindow::from_row)
            .collect()
    };
    let (mut cleared, mut held) = (0, 0);
    for seed in 1..=200_u64 {
        let mut draw = Draw(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
        let (width, height) = (draw.int(200, 600), draw.int(150, 500));
        let (x, y) = (draw.pt(0, 1440 - 600), draw.pt(0, 900 - 500));
        let local = Rect::new(draw.pt(0, width - 20), draw.pt(0, height - 20), 20.0, 20.0);
        let mut windows = Vec::new();
        for over in 0..draw.int(1, 3) {
            let pid = if draw.next().is_multiple_of(3) {
                10
            } else {
                20 + i64::try_from(over).unwrap_or_default()
            };
            let layer = if draw.next().is_multiple_of(2) { 0 } else { 3 };
            windows.push(row(
                OVER + over,
                pid,
                "Other",
                layer,
                (
                    draw.pt(0, 1300),
                    draw.pt(0, 800),
                    draw.pt(60, 700),
                    draw.pt(60, 500),
                ),
            ));
        }
        let (width, height) = (length_of(width), length_of(height));
        windows.push(row(TARGET, 10, "Target", 0, (x, y, width, height)));
        let mut desk = Desk {
            windows,
            acted: Vec::new(),
        };
        let before = cover_of(&listed(&desk), TARGET, Some(local)).expect("listed");
        let theirs = before
            .coverers
            .iter()
            .any(|over| over.owner == Owner::OtherApp && over.layer > 0);
        let mut seat = Seat::at(JevMode::Off, false, Err("off".into()));
        let place = Place {
            window: TARGET,
            local: Some(local),
            needs: Needs::Centre,
        };
        let ended = {
            let desk_cell = std::cell::RefCell::new(&mut desk);
            let mut call =
                |method: &str, params: Value| desk_cell.borrow_mut().call(method, params);
            let mut pause = |_wait: Duration| {};
            let mut refuse = |_line: &Said| Decision::Refused;
            let wall = || 1_790_000_000_000_i64;
            uncover(
                place,
                &mut Hand {
                    call: &mut call,
                    pause: &mut pause,
                    person: &mut refuse,
                    wall_ms: &wall,
                },
                &mut seat,
            )
            .expect("listed")
        };
        assert!(
            desk.moved().iter().all(|(id, _)| *id == TARGET),
            "seed {seed}: {:?}",
            desk.acted
        );
        let after = cover_of(&listed(&desk), TARGET, Some(local)).expect("listed");
        match ended {
            Uncovered::Clear { .. } => {
                assert!(!after.blocks_a_press(), "seed {seed}");
                cleared += 1;
            }
            Uncovered::Held { held: why, .. } => {
                match why {
                    Held::Theirs => assert!(theirs, "seed {seed}"),
                    Held::NothingCleared => assert!(after.blocks_a_press(), "seed {seed}"),
                    other => panic!("seed {seed}: {other:?}"),
                }
                assert!(before.blocks_a_press(), "seed {seed}");
                held += 1;
            }
        }
    }
    // The seeds reach both ends, or the test proves nothing.
    assert!(cleared > 0 && held > 0, "cleared {cleared}, held {held}");
}

/// A drawn whole number of points as a length.
fn length_of(points: u64) -> f64 {
    // Screen points, far inside f64's whole numbers.
    #[allow(clippy::cast_precision_loss)]
    let length = points as f64;
    length
}
