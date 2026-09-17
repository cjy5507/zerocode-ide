//! The current macOS keyboard input source, reduced to the one fact the
//! terminal needs for Orca's `Option as Alt` automatic mode.
//!
//! The renderer must not guess this from the UI locale: a person can write a
//! Korean document with a US layout selected, or use a non-US layout in an
//! English UI. InputSource Services is the native authority and this module is
//! the only FFI boundary that reads it.

use serde::Serialize;

const US_INPUT_SOURCE_IDS: [&str; 2] = [
    "com.apple.keylayout.us",
    "com.apple.keylayout.usinternational-pc",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum KeyboardLayoutCategory {
    Us,
    NonUs,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct KeyboardLayoutReport {
    category: KeyboardLayoutCategory,
}

fn classify_input_source(identifier: Option<&str>) -> KeyboardLayoutCategory {
    let Some(identifier) = identifier.map(str::trim).filter(|value| !value.is_empty()) else {
        return KeyboardLayoutCategory::Unknown;
    };
    if US_INPUT_SOURCE_IDS
        .iter()
        .any(|known| identifier.eq_ignore_ascii_case(known))
    {
        KeyboardLayoutCategory::Us
    } else {
        KeyboardLayoutCategory::NonUs
    }
}

#[tauri::command]
pub(crate) fn terminal_keyboard_layout() -> KeyboardLayoutReport {
    let _crumb = crate::crumbs::Command::enter("terminal_keyboard_layout");
    KeyboardLayoutReport {
        category: classify_input_source(current_input_source_id().as_deref()),
    }
}

#[cfg(not(target_os = "macos"))]
fn current_input_source_id() -> Option<String> {
    None
}

#[cfg(target_os = "macos")]
fn current_input_source_id() -> Option<String> {
    use core::ffi::c_void;
    use core_foundation::base::{CFType, CFTypeRef, TCFType};
    use core_foundation::string::{CFString, CFStringRef};

    #[link(name = "Carbon", kind = "framework")]
    unsafe extern "C" {
        #[link_name = "kTISPropertyInputSourceID"]
        static K_TIS_PROPERTY_INPUT_SOURCE_ID: CFStringRef;
        #[link_name = "TISCopyCurrentKeyboardInputSource"]
        fn copy_current_keyboard_input_source() -> *const c_void;
        #[link_name = "TISGetInputSourceProperty"]
        fn get_input_source_property(
            input_source: *const c_void,
            property_key: CFStringRef,
        ) -> *const c_void;
    }

    // SAFETY: `TISCopyCurrentKeyboardInputSource` returns a retained CF object
    // under the Create rule. The property behind the documented
    // `kTISPropertyInputSourceID` key is either null or a borrowed CFString.
    // The wrappers below apply those two ownership rules exactly.
    unsafe {
        let source = copy_current_keyboard_input_source();
        if source.is_null() {
            return None;
        }
        let _source = CFType::wrap_under_create_rule(source.cast::<c_void>() as CFTypeRef);
        let identifier = get_input_source_property(source, K_TIS_PROPERTY_INPUT_SOURCE_ID);
        if identifier.is_null() {
            return None;
        }
        Some(CFString::wrap_under_get_rule(identifier.cast()).to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::{KeyboardLayoutCategory, US_INPUT_SOURCE_IDS, classify_input_source};

    #[test]
    fn only_orcas_two_native_us_sources_enable_automatic_alt() {
        for identifier in [
            "com.apple.keylayout.US",
            "com.apple.keylayout.USInternational-PC",
            "COM.APPLE.KEYLAYOUT.US",
        ] {
            assert_eq!(
                classify_input_source(Some(identifier)),
                KeyboardLayoutCategory::Us
            );
        }
        for identifier in [
            "com.apple.keylayout.ABC",
            "com.apple.inputmethod.Korean.2SetKorean",
            "com.apple.inputmethod.Kotoeri.Japanese",
        ] {
            assert_eq!(
                classify_input_source(Some(identifier)),
                KeyboardLayoutCategory::NonUs
            );
        }
    }

    #[test]
    fn an_unreadable_input_source_is_unknown_and_never_assumed_us() {
        assert_eq!(classify_input_source(None), KeyboardLayoutCategory::Unknown);
        assert_eq!(
            classify_input_source(Some("  ")),
            KeyboardLayoutCategory::Unknown
        );
    }

    #[test]
    fn the_native_us_inventory_matches_the_versioned_orca_contract() {
        let contract: serde_json::Value =
            serde_json::from_str(include_str!("../../../ui/tests/orca-settings-1.4.180.json"))
                .expect("Orca settings contract is JSON");
        let measured = contract["terminal_option_as_alt_contract"]["auto_us_input_source_ids"]
            .as_array()
            .expect("Option as Alt contract owns a US input source list")
            .iter()
            .map(|value| value.as_str().expect("input source ID is a string"))
            .collect::<Vec<_>>();

        assert_eq!(measured, US_INPUT_SOURCE_IDS);
    }
}
