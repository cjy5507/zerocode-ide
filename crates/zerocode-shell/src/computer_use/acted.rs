//! What a step's act did, heard as the helper answered it (I-t123731-rc1):
//! whether it went no further than its window, and which window that was
//! when it acted — its number and its owner. The step's evidence is framed
//! by it: the act that ran, not the step's words read again (a mark click
//! asked `--foreground` goes to the background), nor the look after the act
//! (which may land on another window of the app) — whether the step's
//! answer reached the agent as words or as JSON.

use std::cell::Cell;

use serde_json::Value;
use zerocode_core::computer_use_protocol::execution::{self, Acted};

thread_local! {
    /// The last act heard on this thread while a step listens (`during`):
    /// `None` while none listens, `Some(None)` before its first act.
    static HEARD: Cell<Option<Option<Acted>>> = const { Cell::new(None) };
}

/// Run a step, and say what its last act did as the helper answered it;
/// `None` when it made no act the helper answered — ZeroCode's own page
/// pressed it, or it pressed nothing.
pub(crate) fn during<T>(step: impl FnOnce() -> T) -> (T, Option<Acted>) {
    /// Puts back what an outer step had heard, however this one ends.
    struct Restore(Option<Option<Acted>>);
    impl Drop for Restore {
        fn drop(&mut self) {
            HEARD.set(self.0);
        }
    }
    let _restore = Restore(HEARD.replace(Some(None)));
    let ran = step();
    (ran, HEARD.get().flatten())
}

/// A request the helper answered: the listening step's last act, when it
/// acts. `background`: whether it was sent asking for the background road.
pub(super) fn heard(method: &str, background: bool, answer: &Value) {
    let Some(acted) = execution::acted(method, background, answer) else {
        return;
    };
    if HEARD.get().is_some() {
        HEARD.set(Some(Some(acted)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use zerocode_core::computer_use_protocol::execution::ActedWindow;

    fn pressed(id: u64) -> Value {
        json!({ "action": { "actedWindow": { "id": id, "pid": 501 } } })
    }

    fn window(acted: Option<Acted>) -> Option<u64> {
        acted?.window.map(|window| window.id)
    }

    /// A step hears its own last act: a look after it changes nothing, an
    /// act outside every step is kept by none, a step inside another hears
    /// its own and leaves the outer one's as it was, and a step that acts
    /// on nothing hears nothing.
    #[test]
    fn a_step_hears_its_own_last_act_and_nothing_outside_it() {
        heard("click", true, &pressed(3));
        let ((), acted) = during(|| {
            heard("click", true, &pressed(5));
            heard("click", true, &pressed(7));
            heard(
                "getAppState",
                true,
                &json!({ "snapshot": { "window": { "id": 19 } } }),
            );
        });
        assert_eq!(
            acted,
            Some(Acted {
                in_its_window: true,
                window: Some(ActedWindow { id: 7, pid: 501 }),
            }),
            "the last act, never the look after it"
        );
        let (inner, outer) = during(|| {
            heard("click", true, &pressed(5));
            let ((), inner) = during(|| heard("click", true, &pressed(9)));
            inner
        });
        assert_eq!(window(inner), Some(9), "the inner step's own act");
        assert_eq!(window(outer), Some(5), "the outer step's act stands");
        assert_eq!(during(|| ()).1, None, "a step that acts on nothing");
    }
}
