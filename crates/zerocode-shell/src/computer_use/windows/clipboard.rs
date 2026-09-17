//! The clipboard, for `paste-text`: everything that was there, the text to
//! paste, and putting everything back.
//!
//! The macOS helper copies every pasteboard item out and writes every item
//! back; this is the same promise, and the road to it is a type. Nothing here
//! can empty the clipboard without first holding a [`Snapshot`] that is
//! complete — every format on it read into bytes (`super::super::clipboard_formats`
//! says which can be) — so a clipboard carrying something this process could
//! not put back is never touched. A snapshot that is not complete has no
//! road to `EmptyClipboard` at all; `paste-text` types the text instead.
//!
//! Restoration is guarded by the clipboard sequence number: if another
//! program changed the clipboard between our write and our restore, its
//! contents are the person's newer ones and are left alone.

use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, EnumClipboardFormats, GetClipboardData,
    GetClipboardFormatNameW, GetClipboardSequenceNumber, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::Memory::{
    GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock,
};
use zerocode_core::computer_use_protocol::{ProviderError, error_code};

use super::super::clipboard_formats::{FormatClass, UNICODE_TEXT, classify, standard_name};

/// The clipboard, open for the lifetime of this value. Another process may
/// hold it for a moment; a few short retries cover that.
struct Open;

impl Open {
    fn acquire() -> Result<Self, ProviderError> {
        for attempt in 0..10 {
            // SAFETY: opening the clipboard without an owner window.
            if unsafe { OpenClipboard(None) }.is_ok() {
                return Ok(Self);
            }
            std::thread::sleep(std::time::Duration::from_millis(10 * (attempt + 1)));
        }
        Err(ProviderError::new(
            error_code::ACCESSIBILITY_ERROR,
            "the clipboard is held by another application",
        ))
    }
}

impl Drop for Open {
    fn drop(&mut self) {
        // SAFETY: balances the OpenClipboard this value stands for.
        let _ = unsafe { CloseClipboard() };
    }
}

fn clipboard_error(context: &str, error: windows::core::Error) -> ProviderError {
    ProviderError::new(
        error_code::ACCESSIBILITY_ERROR,
        format!("could not {context}: {error}"),
    )
}

/// One format's bytes, as they were.
#[derive(Debug, Clone)]
pub(super) struct Entry {
    pub format: u32,
    pub name: String,
    pub bytes: Vec<u8>,
}

/// The formats a clipboard carries, in the order the system lists them.
fn formats_on(_open: &Open) -> Vec<u32> {
    let mut formats = Vec::new();
    let mut format = 0;
    loop {
        // SAFETY: the clipboard is open; 0 ends the enumeration.
        format = unsafe { EnumClipboardFormats(format) };
        if format == 0 {
            break;
        }
        formats.push(format);
    }
    formats
}

fn format_name(format: u32) -> String {
    if let Some(name) = standard_name(format) {
        return name.to_string();
    }
    let mut buffer = [0u16; 256];
    // SAFETY: the buffer outlives the call; the API bounds itself by it.
    let length = unsafe { GetClipboardFormatNameW(format, &mut buffer) };
    if length > 0 {
        String::from_utf16_lossy(&buffer[..length as usize])
    } else {
        format!("format {format:#x}")
    }
}

/// The bytes of a global-block format, or `None` when the owner would not
/// render it or the block cannot be read.
fn read_global(_open: &Open, format: u32) -> Option<Vec<u8>> {
    // SAFETY: the clipboard is open; the global block is locked, read
    // within its reported size, and unlocked before anything else happens.
    unsafe {
        let handle = GetClipboardData(format).ok()?;
        let global = HGLOBAL(handle.0);
        let size = GlobalSize(global);
        let locked = GlobalLock(global);
        if locked.is_null() {
            return None;
        }
        let bytes = std::slice::from_raw_parts(locked.cast::<u8>(), size).to_vec();
        let _ = GlobalUnlock(global);
        Some(bytes)
    }
}

/// A global block holding `bytes`, handed to the clipboard — which owns it
/// from then on — or freed again when the clipboard refuses it.
fn write_global(_open: &Open, format: u32, bytes: &[u8]) -> Result<(), ProviderError> {
    // SAFETY: a movable global block sized for the bytes is filled while
    // locked, then handed to the clipboard; on refusal it is freed here.
    unsafe {
        let global = GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1))
            .map_err(|error| clipboard_error("allocate clipboard memory", error))?;
        let locked = GlobalLock(global);
        if locked.is_null() {
            let _ = GlobalFree(Some(global));
            return Err(ProviderError::new(
                error_code::ACCESSIBILITY_ERROR,
                "could not lock clipboard memory",
            ));
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), locked.cast::<u8>(), bytes.len());
        let _ = GlobalUnlock(global);
        if let Err(error) = SetClipboardData(format, Some(HANDLE(global.0))) {
            let _ = GlobalFree(Some(global));
            return Err(clipboard_error("set the clipboard", error));
        }
    }
    Ok(())
}

fn sequence_number() -> u32 {
    // SAFETY: a plain read.
    unsafe { GetClipboardSequenceNumber() }
}

/// Everything the clipboard held, read out; and what could not be.
pub(super) struct Snapshot {
    entries: Vec<Entry>,
    unpreserved: Vec<String>,
}

impl Snapshot {
    /// Read the clipboard. A format that is not a global block, or whose
    /// owner would not render it, is named in `unpreserved` — and a snapshot
    /// with any is not complete.
    pub fn take() -> Result<Self, ProviderError> {
        let open = Open::acquire()?;
        let formats = formats_on(&open);
        let mut entries = Vec::new();
        let mut unpreserved = Vec::new();
        for format in &formats {
            match classify(*format) {
                FormatClass::Global => match read_global(&open, *format) {
                    Some(bytes) => entries.push(Entry {
                        format: *format,
                        name: format_name(*format),
                        bytes,
                    }),
                    None => unpreserved.push(format_name(*format)),
                },
                FormatClass::GdiHandle => {
                    if !super::super::clipboard_formats::global_twin(*format)
                        .is_some_and(|twin| formats.contains(&twin))
                    {
                        unpreserved.push(format_name(*format));
                    }
                }
                FormatClass::OwnerOnly => unpreserved.push(format_name(*format)),
            }
        }
        Ok(Self {
            entries,
            unpreserved,
        })
    }

    /// Whether everything on the clipboard can be put back.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.unpreserved.is_empty()
    }

    /// The formats that could not be read, by name — the reason a paste
    /// leaves the clipboard alone.
    #[must_use]
    pub fn unpreserved(&self) -> &[String] {
        &self.unpreserved
    }

    #[must_use]
    pub fn entry(&self, format: u32) -> Option<&Entry> {
        self.entries.iter().find(|entry| entry.format == format)
    }

    #[must_use]
    pub fn formats(&self) -> Vec<u32> {
        self.entries.iter().map(|entry| entry.format).collect()
    }

    /// The Unicode text on the clipboard, if any.
    #[must_use]
    pub fn text(&self) -> Option<String> {
        let entry = self.entry(UNICODE_TEXT)?;
        let units: Vec<u16> = entry
            .bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        let end = units
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(units.len());
        Some(String::from_utf16_lossy(&units[..end]))
    }

    /// Replace the clipboard with `text`. Only a complete snapshot may:
    /// this is the one road to `EmptyClipboard`, and it exists only once
    /// everything that will be emptied has been read.
    pub fn replace_with_text(self, text: &str) -> Result<Replaced, (Self, ProviderError)> {
        if !self.is_complete() {
            let why = format!(
                "the clipboard holds content that could not be put back ({}); it was left untouched",
                self.unpreserved.join(", ")
            );
            return Err((
                self,
                ProviderError::new(error_code::ACCESSIBILITY_ERROR, why),
            ));
        }
        let written = {
            let open = match Open::acquire() {
                Ok(open) => open,
                Err(error) => return Err((self, error)),
            };
            // SAFETY: the clipboard is open.
            if let Err(error) = unsafe { EmptyClipboard() } {
                return Err((self, clipboard_error("clear the clipboard", error)));
            }
            let units: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
            let bytes: Vec<u8> = units.iter().flat_map(|unit| unit.to_le_bytes()).collect();
            write_global(&open, UNICODE_TEXT, &bytes)
        };
        // The clipboard is closed again here; the sequence number after our
        // write is what a restore must still see.
        match written {
            Ok(()) => Ok(Replaced {
                snapshot: self,
                sequence_after_write: sequence_number(),
            }),
            Err(error) => {
                // The clipboard was emptied and the text refused: put the
                // snapshot back at once rather than leave it empty.
                let _ = restore_entries(&self.entries);
                Err((self, error))
            }
        }
    }
}

/// The clipboard as `paste-text` left it: our text, with the person's
/// content held until [`Replaced::restore`].
pub(super) struct Replaced {
    snapshot: Snapshot,
    sequence_after_write: u32,
}

/// How a restore ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum RestoreOutcome {
    /// Every format is back.
    Restored,
    /// Another program changed the clipboard after our write; its newer
    /// content was left in place and ours is gone with it.
    Superseded,
    /// The clipboard was emptied and some format could not be written back.
    Failed(String),
}

impl Replaced {
    pub fn restore(self) -> RestoreOutcome {
        if sequence_number() != self.sequence_after_write {
            return RestoreOutcome::Superseded;
        }
        match restore_entries(&self.snapshot.entries) {
            Ok(()) => RestoreOutcome::Restored,
            Err(error) => RestoreOutcome::Failed(error.message),
        }
    }
}

/// Empty the clipboard and write every entry back. A refused entry is
/// named and the rest are still written.
fn restore_entries(entries: &[Entry]) -> Result<(), ProviderError> {
    let open = Open::acquire()?;
    // SAFETY: the clipboard is open.
    unsafe { EmptyClipboard() }.map_err(|error| clipboard_error("clear the clipboard", error))?;
    let mut refused = Vec::new();
    for entry in entries {
        if let Err(error) = write_global(&open, entry.format, &entry.bytes) {
            refused.push(format!("{}: {}", entry.name, error.message));
        }
    }
    if refused.is_empty() {
        Ok(())
    } else {
        Err(ProviderError::new(
            error_code::ACCESSIBILITY_ERROR,
            format!("could not restore {}", refused.join("; ")),
        ))
    }
}

/// The fixture tests' hands on the clipboard: put content there the way an
/// app does, look at what is there, and put the host's own content back
/// when they are done.
#[cfg(test)]
pub(super) mod fixture {
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::DataExchange::{
        EmptyClipboard, IsClipboardFormatAvailable, RegisterClipboardFormatW, SetClipboardData,
    };
    use windows::core::PCWSTR;

    use super::{Open, Snapshot, restore_entries, write_global};

    /// Empty the clipboard and put these global-block formats on it.
    pub fn put(entries: &[(u32, Vec<u8>)]) {
        let open = Open::acquire().expect("open the clipboard");
        // SAFETY: the clipboard is open.
        unsafe { EmptyClipboard() }.expect("empty the clipboard");
        for (format, bytes) in entries {
            write_global(&open, *format, bytes).expect("write a clipboard format");
        }
    }

    /// Empty the clipboard and put one handle-based format on it; the
    /// clipboard owns the handle from then on.
    pub fn put_handle(format: u32, handle: HANDLE) {
        let _open = Open::acquire().expect("open the clipboard");
        // SAFETY: the clipboard is open; the handle is the caller's to give.
        unsafe {
            EmptyClipboard().expect("empty the clipboard");
            SetClipboardData(format, Some(handle)).expect("set a clipboard handle");
        }
    }

    pub fn has_format(format: u32) -> bool {
        let _open = Open::acquire().expect("open the clipboard");
        // SAFETY: a plain query while the clipboard is open.
        unsafe { IsClipboardFormatAvailable(format) }.is_ok()
    }

    pub fn register(name: &str) -> u32 {
        let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
        // SAFETY: the name is NUL-terminated and outlives the call.
        unsafe { RegisterClipboardFormatW(PCWSTR(wide.as_ptr())) }
    }

    impl Snapshot {
        /// Put a snapshot's content back regardless of sequence — for a
        /// test handing the host its clipboard back.
        pub fn put_back(self) {
            let _ = restore_entries(&self.entries);
        }
    }
}
