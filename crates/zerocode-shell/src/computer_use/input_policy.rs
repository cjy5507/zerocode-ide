use std::cell::Cell;
use std::marker::PhantomData;
use std::rc::Rc;

pub(crate) use zerocode_core::computer_use_protocol::execution::InputPolicy;

thread_local! {
    static ACTIVE: Cell<Option<InputPolicy>> = const { Cell::new(None) };
}

pub(crate) struct Scope {
    previous: Option<InputPolicy>,
    thread_bound: PhantomData<Rc<()>>,
}

impl Drop for Scope {
    fn drop(&mut self) {
        ACTIVE.with(|active| active.set(self.previous));
    }
}

#[must_use]
pub(crate) fn current() -> InputPolicy {
    ACTIVE.with(|active| active.get().unwrap_or_default())
}

#[must_use]
pub(crate) fn enter(requested: InputPolicy) -> Scope {
    let previous = ACTIVE.with(|active| {
        let previous = active.get();
        active.set(Some(
            previous.map_or(requested, |parent| requested.within(parent)),
        ));
        previous
    });
    Scope {
        previous,
        thread_bound: PhantomData,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_nested_request_cannot_relax_a_background_run() {
        assert_eq!(current(), InputPolicy::Background);
        let _outer = enter(InputPolicy::Background);
        let _inner = enter(InputPolicy::SharedDesktop);
        assert_eq!(current(), InputPolicy::Background);
    }

    #[test]
    fn shared_access_is_scoped_and_restored_after_unwinding() {
        let _outer = enter(InputPolicy::SharedDesktop);
        assert_eq!(current(), InputPolicy::SharedDesktop);
        let outcome = std::panic::catch_unwind(|| {
            let _inner = enter(InputPolicy::Background);
            assert_eq!(current(), InputPolicy::Background);
            panic!("fixture unwind");
        });
        assert!(outcome.is_err());
        assert_eq!(current(), InputPolicy::SharedDesktop);
    }

    #[test]
    fn shared_access_is_not_inherited_by_another_thread() {
        let _scope = enter(InputPolicy::SharedDesktop);
        assert_eq!(
            std::thread::spawn(current).join().unwrap(),
            InputPolicy::Background
        );
    }
}
