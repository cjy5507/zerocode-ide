//! The event plan for a synthetic mouse click, and the fence around it.
//!
//! `SyntheticMouseClickDelivery` from the macOS helper (STA-3433): one move,
//! then a down/up pair per press with a pause between events, and BEFORE
//! every press and AFTER every release a look at who owns the focused window
//! under the pointer. A press that would land in a window that is not the
//! target is not posted, and the refusal says how many presses were already
//! delivered so the agent knows whether the app may have acted. The platform
//! makes and posts the events; this module orders them.

/// Three presses is a triple click; more is not a click.
pub const MAX_CLICK_COUNT: usize = 3;
/// Unpaced posts race the window server's routing and the mouse-up is
/// silently dropped (measured on macOS; kept on every platform because a
/// dropped up is a hover, not a click).
pub const INTER_EVENT_PAUSE_MICROS: u32 = 50_000;
// Unpaced posts race the window server and the mouse-up is dropped, turning
// the click into a hover-only no-op (STA-3433) — so the pause is never zero.
const _: () = assert!(INTER_EVENT_PAUSE_MICROS > 0);

/// The window a click is meant for: the process and the window id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Recipient {
    pub owner_pid: u32,
    pub window_id: u64,
}

/// What the platform saw when asked "who would receive a click right now".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecipientObservation {
    Focused(Recipient),
    /// The target went away — a popup a final release dismissed.
    Dismissed,
    /// The platform could not say. Fail closed.
    Unavailable,
}

impl RecipientObservation {
    fn recipient(self) -> Option<Recipient> {
        match self {
            Self::Focused(recipient) => Some(recipient),
            Self::Dismissed | Self::Unavailable => None,
        }
    }
}

/// Why a click was stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FenceFailure {
    RecipientChanged {
        expected: Recipient,
        actual: Option<Recipient>,
        delivered_presses: usize,
    },
}

/// One event of the plan. `press_index` becomes the event's click state so
/// repeated presses register as double/triple clicks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Move,
    ButtonDown { press_index: usize },
    ButtonUp { press_index: usize },
}

/// The plan for `click_count` presses.
#[must_use]
pub fn steps(click_count: usize) -> Vec<Step> {
    let mut plan = vec![Step::Move];
    for press in 1..=click_count.clamp(1, MAX_CLICK_COUNT) {
        plan.push(Step::ButtonDown { press_index: press });
        plan.push(Step::ButtonUp { press_index: press });
    }
    plan
}

/// The click-state field for a step; 0 leaves it unset.
#[must_use]
pub const fn click_state(step: Step) -> i64 {
    match step {
        Step::Move => 0,
        Step::ButtonDown { press_index } | Step::ButtonUp { press_index } => press_index as i64,
    }
}

/// The one candidate a predicate picks, or `None` when it picks none or more
/// than one — an ambiguous frame match must not choose a window.
pub fn unique_window_candidate<C>(candidates: &[C], predicate: impl Fn(&C) -> bool) -> Option<&C> {
    let mut found = None;
    for candidate in candidates.iter().filter(|candidate| predicate(candidate)) {
        if found.is_some() {
            return None;
        }
        found = Some(candidate);
    }
    found
}

/// Deliver the plan. `observe` is asked before each press and after each
/// release; `make_event` builds the platform event for a step (it may fail,
/// in which case nothing of that pair is posted); `post` posts it and says
/// whether the platform accepted it — a refused event ends the plan with the
/// truth of what went before it, never with a success; `pause` sleeps the
/// given microseconds.
pub fn deliver<Event, E>(
    click_count: usize,
    target: Recipient,
    mut observe: impl FnMut() -> RecipientObservation,
    mut make_event: impl FnMut(Step) -> Result<Event, E>,
    mut post: impl FnMut(Event) -> Result<(), E>,
    mut pause: impl FnMut(u32),
) -> Result<(), DeliveryFailure<E>> {
    let not_posted = |error: E| DeliveryFailure::Event {
        error,
        delivered_presses: 0,
        button_held: false,
    };
    post(make_event(Step::Move).map_err(not_posted)?).map_err(not_posted)?;
    pause(INTER_EVENT_PAUSE_MICROS);
    let press_count = click_count.clamp(1, MAX_CLICK_COUNT);
    for press_index in 1..=press_count {
        let before_down = observe();
        if before_down != RecipientObservation::Focused(target) {
            return Err(DeliveryFailure::Fence(FenceFailure::RecipientChanged {
                expected: target,
                actual: before_down.recipient(),
                delivered_presses: press_index - 1,
            }));
        }
        let before_this_press = |error: E| DeliveryFailure::Event {
            error,
            delivered_presses: press_index - 1,
            button_held: false,
        };
        let down = make_event(Step::ButtonDown { press_index }).map_err(before_this_press)?;
        let up = make_event(Step::ButtonUp { press_index }).map_err(before_this_press)?;
        post(down).map_err(before_this_press)?;
        pause(INTER_EVENT_PAUSE_MICROS);
        // A refused release leaves the button down in the app's eyes: the
        // platform must release it by hand, and the press is not a click.
        post(up).map_err(|error| DeliveryFailure::Event {
            error,
            delivered_presses: press_index - 1,
            button_held: true,
        })?;
        let after_up = observe();
        // A final release may dismiss the target; an unavailable probe is unsafe.
        let final_dismissal =
            press_index == press_count && after_up == RecipientObservation::Dismissed;
        if after_up != RecipientObservation::Focused(target) && !final_dismissal {
            return Err(DeliveryFailure::Fence(FenceFailure::RecipientChanged {
                expected: target,
                actual: after_up.recipient(),
                delivered_presses: press_index,
            }));
        }
        pause(INTER_EVENT_PAUSE_MICROS);
    }
    Ok(())
}

/// Either the fence stopped the click or the platform could not make or
/// post an event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeliveryFailure<E> {
    Fence(FenceFailure),
    Event {
        error: E,
        /// Presses whose down AND up were accepted before this refusal.
        delivered_presses: usize,
        /// The down was accepted and the up was refused: the button is
        /// still held as far as the app knows, and the platform must
        /// release it.
        button_held: bool,
    },
}

/// The recovery sentence a partial delivery carries, shared by the fence's
/// message and the platform's wording of a refused event.
#[must_use]
pub fn partial_delivery_recovery(delivered_presses: usize) -> String {
    if delivered_presses == 0 {
        "bring the target window forward, run get-app-state again, and retry".to_string()
    } else {
        format!(
            "{delivered_presses} press(es) may already have been delivered; run get-app-state and verify state before retrying"
        )
    }
}

impl FenceFailure {
    /// The `window_not_focused` sentence the helper writes for this failure.
    #[must_use]
    pub fn message(&self) -> String {
        match self {
            Self::RecipientChanged {
                expected,
                actual,
                delivered_presses,
            } => {
                let actual_description = actual.map_or_else(
                    || "no focused window".to_string(),
                    |actual| format!("pid {} window {}", actual.owner_pid, actual.window_id),
                );
                let recovery = partial_delivery_recovery(*delivered_presses);
                format!(
                    "coordinate click aborted because target pid {} window {} is no longer the focused topmost recipient (current: {actual_description}); {recovery}",
                    expected.owner_pid, expected.window_id
                )
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TARGET: Recipient = Recipient {
        owner_pid: 41,
        window_id: 101,
    };
    const INTRUDER: Recipient = Recipient {
        owner_pid: 52,
        window_id: 202,
    };

    fn run(
        click_count: usize,
        mut observations: Vec<RecipientObservation>,
    ) -> (Result<(), DeliveryFailure<()>>, Vec<Step>) {
        let mut posted = Vec::new();
        let result = deliver(
            click_count,
            TARGET,
            || observations.remove(0),
            Ok::<Step, ()>,
            |event| {
                posted.push(event);
                Ok(())
            },
            |_| {},
        );
        (result, posted)
    }

    // ---- SyntheticMouseClickDeliveryTests.swift, case for case ----

    #[test]
    fn plans_pair_down_and_up_after_move_and_cap_at_triple() {
        assert_eq!(
            steps(1),
            vec![
                Step::Move,
                Step::ButtonDown { press_index: 1 },
                Step::ButtonUp { press_index: 1 }
            ]
        );
        assert_eq!(
            steps(2),
            vec![
                Step::Move,
                Step::ButtonDown { press_index: 1 },
                Step::ButtonUp { press_index: 1 },
                Step::ButtonDown { press_index: 2 },
                Step::ButtonUp { press_index: 2 },
            ]
        );
        assert_eq!(steps(0), steps(1));
        assert_eq!(steps(usize::MAX).len(), 7);
        assert_eq!(click_state(Step::Move), 0);
        assert_eq!(click_state(Step::ButtonDown { press_index: 1 }), 1);
        assert_eq!(click_state(Step::ButtonUp { press_index: 2 }), 2);
    }

    #[test]
    fn an_ambiguous_window_frame_fallback_selects_nothing() {
        let candidates = [(101, 7), (202, 7)];
        assert!(unique_window_candidate(&candidates, |c| c.1 == 7).is_none());
        assert!(unique_window_candidate(&candidates, |c| c.1 == 8).is_none());
        assert_eq!(
            unique_window_candidate(&candidates, |c| c.0 == 101),
            Some(&(101, 7))
        );
    }

    #[test]
    fn a_recipient_change_before_mouse_down_posts_no_click() {
        let (result, posted) = run(1, vec![RecipientObservation::Focused(INTRUDER)]);
        assert_eq!(
            result,
            Err(DeliveryFailure::Fence(FenceFailure::RecipientChanged {
                expected: TARGET,
                actual: Some(INTRUDER),
                delivered_presses: 0
            }))
        );
        assert_eq!(posted, vec![Step::Move]);
    }

    #[test]
    fn a_final_mouse_up_may_dismiss_the_target() {
        let (result, posted) = run(
            1,
            vec![
                RecipientObservation::Focused(TARGET),
                RecipientObservation::Dismissed,
            ],
        );
        assert!(result.is_ok());
        assert_eq!(posted, steps(1));
    }

    #[test]
    fn a_final_unavailable_observation_fails_closed() {
        let (result, posted) = run(
            1,
            vec![
                RecipientObservation::Focused(TARGET),
                RecipientObservation::Unavailable,
            ],
        );
        assert_eq!(
            result,
            Err(DeliveryFailure::Fence(FenceFailure::RecipientChanged {
                expected: TARGET,
                actual: None,
                delivered_presses: 1
            }))
        );
        assert_eq!(posted, steps(1));
    }

    #[test]
    fn a_final_mouse_up_rejects_a_different_focused_recipient() {
        let (result, posted) = run(
            1,
            vec![
                RecipientObservation::Focused(TARGET),
                RecipientObservation::Focused(INTRUDER),
            ],
        );
        assert_eq!(
            result,
            Err(DeliveryFailure::Fence(FenceFailure::RecipientChanged {
                expected: TARGET,
                actual: Some(INTRUDER),
                delivered_presses: 1
            }))
        );
        assert_eq!(posted, steps(1));
    }

    #[test]
    fn mouse_up_posts_before_the_recipient_check_for_the_next_press() {
        let trace = std::cell::RefCell::new(Vec::new());
        let result = deliver(
            2,
            TARGET,
            || {
                trace.borrow_mut().push("recipient");
                RecipientObservation::Focused(TARGET)
            },
            Ok::<Step, ()>,
            |event| {
                trace.borrow_mut().push(match event {
                    Step::Move => "move",
                    Step::ButtonDown { .. } => "down",
                    Step::ButtonUp { .. } => "up",
                });
                Ok(())
            },
            |_| {},
        );
        assert!(result.is_ok());
        assert_eq!(
            trace.into_inner(),
            vec![
                "move",
                "recipient",
                "down",
                "up",
                "recipient",
                "recipient",
                "down",
                "up",
                "recipient"
            ]
        );
    }

    #[test]
    fn a_recipient_change_before_a_later_press_reports_completed_presses() {
        let (result, posted) = run(
            2,
            vec![
                RecipientObservation::Focused(TARGET),
                RecipientObservation::Focused(TARGET),
                RecipientObservation::Focused(INTRUDER),
            ],
        );
        assert_eq!(
            result,
            Err(DeliveryFailure::Fence(FenceFailure::RecipientChanged {
                expected: TARGET,
                actual: Some(INTRUDER),
                delivered_presses: 1
            }))
        );
        assert_eq!(posted, steps(1));

        let (result, posted) = run(
            2,
            vec![
                RecipientObservation::Focused(TARGET),
                RecipientObservation::Focused(INTRUDER),
            ],
        );
        assert!(matches!(
            result,
            Err(DeliveryFailure::Fence(FenceFailure::RecipientChanged {
                delivered_presses: 1,
                ..
            }))
        ));
        assert_eq!(posted, steps(1));

        let (result, posted) = run(
            2,
            vec![
                RecipientObservation::Focused(TARGET),
                RecipientObservation::Dismissed,
            ],
        );
        assert_eq!(
            result,
            Err(DeliveryFailure::Fence(FenceFailure::RecipientChanged {
                expected: TARGET,
                actual: None,
                delivered_presses: 1
            }))
        );
        assert_eq!(posted, steps(1));
    }

    #[test]
    fn a_multi_click_revalidates_before_every_press_and_between_releases() {
        let mut validations = 0;
        let mut posted = Vec::new();
        let result = deliver(
            2,
            TARGET,
            || {
                validations += 1;
                RecipientObservation::Focused(TARGET)
            },
            Ok::<Step, ()>,
            |event| {
                posted.push(event);
                Ok(())
            },
            |_| {},
        );
        assert!(result.is_ok());
        assert_eq!(validations, 4);
        assert_eq!(posted, steps(2));
    }

    #[test]
    fn a_button_pair_is_prepared_before_mouse_down_posts() {
        let mut posted = Vec::new();
        let result = deliver(
            1,
            TARGET,
            || RecipientObservation::Focused(TARGET),
            |step| match step {
                Step::ButtonUp { .. } => Err("mouse up"),
                other => Ok(other),
            },
            |event| {
                posted.push(event);
                Ok(())
            },
            |_| {},
        );
        assert_eq!(
            result,
            Err(DeliveryFailure::Event {
                error: "mouse up",
                delivered_presses: 0,
                button_held: false
            })
        );
        assert_eq!(posted, vec![Step::Move]);
    }

    /// A platform that refuses an event (SendInput inserted nothing: another
    /// process holds the input queue, the target runs elevated) ends the plan
    /// with what was delivered and whether a button is still down — never
    /// with a success the app did not see.
    #[test]
    fn a_refused_post_is_a_failure_that_says_what_was_delivered() {
        let refuse = |which: Step| {
            move |event: Step| {
                if event == which {
                    Err("blocked")
                } else {
                    Ok(())
                }
            }
        };
        let observe = || RecipientObservation::Focused(TARGET);
        assert_eq!(
            deliver(
                1,
                TARGET,
                observe,
                Ok::<Step, &str>,
                refuse(Step::Move),
                |_| {}
            ),
            Err(DeliveryFailure::Event {
                error: "blocked",
                delivered_presses: 0,
                button_held: false
            })
        );
        assert_eq!(
            deliver(
                2,
                TARGET,
                observe,
                Ok::<Step, &str>,
                refuse(Step::ButtonDown { press_index: 2 }),
                |_| {}
            ),
            Err(DeliveryFailure::Event {
                error: "blocked",
                delivered_presses: 1,
                button_held: false
            })
        );
        assert_eq!(
            deliver(
                1,
                TARGET,
                observe,
                Ok::<Step, &str>,
                refuse(Step::ButtonUp { press_index: 1 }),
                |_| {}
            ),
            Err(DeliveryFailure::Event {
                error: "blocked",
                delivered_presses: 0,
                button_held: true
            })
        );
        assert_eq!(
            partial_delivery_recovery(0),
            "bring the target window forward, run get-app-state again, and retry"
        );
        assert!(partial_delivery_recovery(2).starts_with("2 press(es) may already"));
    }

    #[test]
    fn the_fence_failure_reads_like_the_helper() {
        let none = FenceFailure::RecipientChanged {
            expected: TARGET,
            actual: None,
            delivered_presses: 0,
        };
        assert_eq!(
            none.message(),
            "coordinate click aborted because target pid 41 window 101 is no longer the focused topmost recipient (current: no focused window); bring the target window forward, run get-app-state again, and retry"
        );
        let some = FenceFailure::RecipientChanged {
            expected: TARGET,
            actual: Some(INTRUDER),
            delivered_presses: 2,
        };
        assert!(
            some.message().contains(
                "(current: pid 52 window 202); 2 press(es) may already have been delivered"
            )
        );
    }
}
