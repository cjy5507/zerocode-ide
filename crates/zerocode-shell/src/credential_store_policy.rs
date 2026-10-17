use std::ffi::OsStr;

pub(crate) const DISABLE_ENV: &str = "ZEROCODE_DISABLE_CREDENTIAL_STORE";
pub(crate) const DISABLED_MESSAGE: &str =
    "credential_store_disabled: credential-store access is disabled for this process";

pub(crate) fn disabled() -> bool {
    disabled_value(std::env::var_os(DISABLE_ENV).as_deref())
}

fn disabled_value(value: Option<&OsStr>) -> bool {
    value == Some(OsStr::new("1"))
}

#[cfg(any(target_os = "macos", test))]
pub(crate) fn ensure_access() -> Result<(), String> {
    ensure_access_for(disabled())
}

#[cfg(any(target_os = "macos", test))]
fn ensure_access_for(disabled: bool) -> Result<(), String> {
    if disabled {
        Err(DISABLED_MESSAGE.to_owned())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn credential_access_is_not_disabled_by_default() {
        assert!(!disabled_value(None));
    }

    #[test]
    fn an_explicit_one_disables_credential_access() {
        assert!(disabled_value(Some(OsStr::new("1"))));
    }

    #[test]
    fn other_values_do_not_disable_credential_access() {
        assert!(
            ["", "0", "true", " 1", "1 "]
                .into_iter()
                .all(|value| !disabled_value(Some(OsStr::new(value))))
        );
    }

    #[test]
    fn a_disabled_store_refuses_before_the_operation() {
        let invoked = Cell::new(false);
        let result = ensure_access_for(true).map(|()| invoked.set(true));
        assert_eq!(
            (result, invoked.get()),
            (Err(DISABLED_MESSAGE.to_owned()), false)
        );
    }

    #[test]
    fn an_enabled_store_keeps_the_operation() {
        assert_eq!(ensure_access_for(false).map(|()| "allowed"), Ok("allowed"));
    }

    #[test]
    fn the_process_guard_matches_its_explicit_policy() {
        assert_eq!(ensure_access().is_err(), disabled());
    }
}
