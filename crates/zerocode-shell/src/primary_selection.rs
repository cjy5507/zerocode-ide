//! Linux PRIMARY-selection access for middle-click paste.
//!
//! The ordinary clipboard stays owned by `tauri-plugin-clipboard-manager`.
//! PRIMARY is a different Linux protocol, and keeping its native handle here
//! prevents clipboard details from leaking into settings or terminal code.

#[cfg(target_os = "linux")]
use std::sync::Mutex;

/// Orca accepts at most 65,536 JavaScript UTF-16 code units. Four bytes per
/// unit is the matching native safety ceiling for text crossing IPC.
pub(crate) const MAX_BYTES: usize = 262_144;

#[derive(Default)]
pub(crate) struct PrimarySelection {
    #[cfg(target_os = "linux")]
    clipboard: Mutex<Option<arboard::Clipboard>>,
}

impl PrimarySelection {
    #[cfg(target_os = "linux")]
    pub(crate) fn read_text(&self) -> Result<String, String> {
        self.with_clipboard(|clipboard| {
            use arboard::{GetExtLinux as _, LinuxClipboardKind};

            clipboard
                .get()
                .clipboard(LinuxClipboardKind::Primary)
                .text()
        })
        .and_then(validate_text)
    }

    #[cfg(not(target_os = "linux"))]
    pub(crate) fn read_text(&self) -> Result<String, String> {
        Err("primary selection is unavailable on this platform".to_string())
    }

    #[cfg(target_os = "linux")]
    pub(crate) fn write_text(&self, text: String) -> Result<(), String> {
        validate_text(text).and_then(|text| {
            self.with_clipboard(|clipboard| {
                use arboard::{LinuxClipboardKind, SetExtLinux as _};

                clipboard
                    .set()
                    .clipboard(LinuxClipboardKind::Primary)
                    .text(text)
            })
        })
    }

    #[cfg(not(target_os = "linux"))]
    pub(crate) fn write_text(&self, text: String) -> Result<(), String> {
        validate_text(text)?;
        Err("primary selection is unavailable on this platform".to_string())
    }

    #[cfg(target_os = "linux")]
    fn with_clipboard<T>(
        &self,
        operation: impl FnOnce(&mut arboard::Clipboard) -> Result<T, arboard::Error>,
    ) -> Result<T, String> {
        let mut held = self
            .clipboard
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if held.is_none() {
            *held = Some(
                arboard::Clipboard::new()
                    .map_err(|_| "primary selection is unavailable".to_string())?,
            );
        }
        operation(held.as_mut().expect("initialized above"))
            .map_err(|_| "primary selection operation failed".to_string())
    }
}

fn validate_text(text: String) -> Result<String, String> {
    if text.is_empty() || text.len() > MAX_BYTES {
        return Err("primary selection text is outside the supported size".to_string());
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_native_boundary_rejects_empty_and_oversized_text() {
        assert!(validate_text(String::new()).is_err());
        assert!(validate_text("x".repeat(MAX_BYTES)).is_ok());
        assert!(validate_text("x".repeat(MAX_BYTES + 1)).is_err());
    }
}
