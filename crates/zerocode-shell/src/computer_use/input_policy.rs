use std::cell::Cell;
use std::marker::PhantomData;
use std::rc::Rc;

pub(crate) use zerocode_core::computer_use_protocol::execution::InputPolicy;

thread_local! {
    static ACTIVE: Cell<Option<RequestPolicy>> = const { Cell::new(None) };
}

#[derive(Clone, Copy)]
pub(crate) struct RequestPolicy {
    policy: InputPolicy,
    shared_generation: Option<u64>,
}

impl RequestPolicy {
    fn revoked(self) -> bool {
        self.shared_generation
            .is_some_and(|generation| generation != super::guard::stop_generation())
    }
}

pub(crate) struct Scope {
    previous: Option<RequestPolicy>,
    thread_bound: PhantomData<Rc<()>>,
}

impl Drop for Scope {
    fn drop(&mut self) {
        ACTIVE.with(|active| active.set(self.previous));
    }
}

#[must_use]
pub(crate) fn current() -> InputPolicy {
    let captured = capture();
    if captured.revoked() {
        InputPolicy::Background
    } else {
        captured.policy
    }
}

#[must_use]
pub(crate) fn capture() -> RequestPolicy {
    ACTIVE.with(|active| {
        active.get().unwrap_or(RequestPolicy {
            policy: InputPolicy::Background,
            shared_generation: None,
        })
    })
}

pub(crate) fn ensure_current() -> Result<(), super::ComputerUseError> {
    if capture().revoked() {
        Err(super::ComputerUseError::new(
            zerocode_core::computer_use_protocol::error_code::STOPPED,
            "this request's shared input permission was revoked; start a new request under the person's current policy",
        ))
    } else {
        Ok(())
    }
}

#[cfg(test)]
#[must_use]
pub(crate) fn enter(requested: InputPolicy) -> Scope {
    enter_with(|| requested)
}

#[must_use]
pub(crate) fn enter_with(read: impl FnOnce() -> InputPolicy) -> Scope {
    let generation = super::guard::stop_generation();
    let policy = read();
    enter_captured(RequestPolicy {
        policy,
        shared_generation: (policy == InputPolicy::SharedDesktop).then_some(generation),
    })
}

#[must_use]
pub(crate) fn enter_captured(mut requested: RequestPolicy) -> Scope {
    let previous = ACTIVE.with(|active| {
        let previous = active.get();
        if let Some(parent) = previous {
            requested.policy = requested.policy.within(parent.policy);
            requested.shared_generation =
                match (requested.shared_generation, parent.shared_generation) {
                    (Some(child), Some(parent)) => Some(child.min(parent)),
                    (child, parent) => child.or(parent),
                };
        }
        active.set(Some(requested));
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
        let _hand = crate::tests::computer_desktop_wait::ONE_HAND
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(current(), InputPolicy::Background);
        let _outer = enter(InputPolicy::Background);
        let _inner = enter(InputPolicy::SharedDesktop);
        assert_eq!(current(), InputPolicy::Background);
    }

    #[test]
    fn shared_access_is_scoped_and_restored_after_unwinding() {
        let _hand = crate::tests::computer_desktop_wait::ONE_HAND
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
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
        let _hand = crate::tests::computer_desktop_wait::ONE_HAND
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _scope = enter(InputPolicy::SharedDesktop);
        assert_eq!(
            std::thread::spawn(current).join().unwrap(),
            InputPolicy::Background
        );
    }

    #[test]
    fn a_resume_cannot_revive_a_revoked_shared_request_or_its_children() {
        let _hand = crate::tests::computer_desktop_wait::ONE_HAND
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _scope = enter(InputPolicy::SharedDesktop);
        let captured = capture();
        super::super::guard::stop("fixture policy change");
        super::super::guard::lift();
        assert_eq!(current(), InputPolicy::Background);
        assert!(ensure_current().is_err());
        {
            let _nested = enter(InputPolicy::SharedDesktop);
            assert!(ensure_current().is_err());
        }
        assert!(
            std::thread::spawn(move || {
                let _captured = enter_captured(captured);
                ensure_current().is_err()
            })
            .join()
            .unwrap()
        );
    }

    #[test]
    fn a_policy_read_racing_a_revocation_does_not_grant_a_new_shared_request() {
        let _hand = crate::tests::computer_desktop_wait::ONE_HAND
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _scope = enter_with(|| {
            super::super::guard::stop("fixture policy change");
            super::super::guard::lift();
            InputPolicy::SharedDesktop
        });
        assert!(ensure_current().is_err());
    }

    #[test]
    fn a_new_request_reads_the_new_grant_after_an_older_one_was_revoked() {
        let _hand = crate::tests::computer_desktop_wait::ONE_HAND
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        {
            let _old = enter(InputPolicy::SharedDesktop);
            super::super::guard::stop("fixture policy change");
            super::super::guard::lift();
            assert!(ensure_current().is_err());
        }
        let _new = enter(InputPolicy::SharedDesktop);
        assert!(ensure_current().is_ok());
    }
}
