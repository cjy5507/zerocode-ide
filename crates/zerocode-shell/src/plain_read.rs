//! Reading another process's file without trusting it (t-69106 ④): the build
//! line's and the release lane's files are written by processes this window
//! does not own, so a reader of them opens a plain file and nothing else —
//! never through a link, never a pipe it could stand waiting on, never more
//! bytes than the caller said it can use.
//!
//! The one place that opens such a file for reading. It only reads: every
//! open here is `read(true)`, and the contract over this file
//! (`source_contracts/board_schedule.rs`) holds it to that.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// The file at `path` opened for reading, when it is a plain file: not a link
/// to one (a link is another process's choice of where we read), not a pipe, a
/// device or a folder. The open itself does not follow a link or wait on a
/// pipe that nobody writes (`O_NOFOLLOW`, `O_NONBLOCK` on Unix; open the
/// reparse point itself and require a disk file on Windows). A name swapped
/// between the look and the open still comes back `None`, and the kind is
/// asked of the open file, not of the name.
pub(crate) fn open_plain(path: &Path) -> Option<File> {
    if !std::fs::symlink_metadata(path).ok()?.is_file() {
        return None;
    }
    let file = open_reading(path)?;
    file.metadata().ok()?.is_file().then_some(file)
}

#[cfg(unix)]
fn open_reading(path: &Path) -> Option<File> {
    use std::os::unix::fs::OpenOptionsExt;
    File::options()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW)
        .open(path)
        .ok()
}

#[cfg(windows)]
fn open_reading(path: &Path) -> Option<File> {
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_OPEN_REPARSE_POINT, FILE_TYPE_DISK, GetFileType,
    };

    let file = File::options()
        .read(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
        .ok()?;
    let metadata = file.metadata().ok()?;
    // Ask the opened handle: the earlier path lookup cannot rule out a swap.
    if !metadata.is_file() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return None;
    }
    // SAFETY: file owns this live handle for the duration of the query.
    (unsafe { GetFileType(file.as_raw_handle()) } == FILE_TYPE_DISK).then_some(file)
}

#[cfg(not(any(unix, windows)))]
fn open_reading(path: &Path) -> Option<File> {
    File::open(path).ok()
}

/// The whole of a plain file, or nothing when it holds more than `max` bytes:
/// a file that size is not the small document the caller means to read.
pub(crate) fn read_plain(path: &Path, max: u64) -> Option<Vec<u8>> {
    let file = open_plain(path)?;
    let mut bytes = Vec::new();
    file.take(max + 1).read_to_end(&mut bytes).ok()?;
    (bytes.len() as u64 <= max).then_some(bytes)
}

/// The last `max` bytes of a plain file, and whether it holds more than that:
/// a long log's last stretch is the part a reader wants.
pub(crate) fn read_plain_tail(path: &Path, max: u64) -> Option<(Vec<u8>, bool)> {
    let mut file = open_plain(path)?;
    let len = file.metadata().ok()?.len();
    let start = len.saturating_sub(max);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut bytes = Vec::new();
    file.take(max).read_to_end(&mut bytes).ok()?;
    Some((bytes, start > 0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_file_is_read_and_one_over_the_cap_is_not() {
        let scratch = tempfile::tempdir().expect("a scratch directory");
        let path = scratch.path().join("small.json");
        std::fs::write(&path, b"{\"a\":1}").expect("a file");
        assert_eq!(read_plain(&path, 7).as_deref(), Some(&b"{\"a\":1}"[..]));
        assert_eq!(read_plain(&path, 6), None, "one byte over the cap");
        assert_eq!(read_plain(&scratch.path().join("gone"), 7), None);
    }

    #[test]
    fn the_tail_is_the_last_bytes_and_says_it_is_cut() {
        let scratch = tempfile::tempdir().expect("a scratch directory");
        let path = scratch.path().join("long.log");
        std::fs::write(&path, b"0123456789").expect("a file");
        assert_eq!(read_plain_tail(&path, 4), Some((b"6789".to_vec(), true)));
        assert_eq!(
            read_plain_tail(&path, 10),
            Some((b"0123456789".to_vec(), false))
        );
    }

    /// Exercise the open after the caller's metadata check, with the name
    /// deterministically replaced by a link in between. Windows CI runs this
    /// with the same symlink permission its other filesystem tests require.
    #[cfg(windows)]
    #[test]
    fn a_file_replaced_by_a_link_after_the_look_is_not_followed() {
        let scratch = tempfile::tempdir().expect("a scratch directory");
        let path = scratch.path().join("status.json");
        let elsewhere = scratch.path().join("elsewhere.json");
        std::fs::write(&path, b"original").expect("a plain file");
        std::fs::write(&elsewhere, b"replacement").expect("a link target");
        assert!(open_plain(&path).is_some(), "a plain disk file must open");
        assert!(
            std::fs::symlink_metadata(&path)
                .expect("the look")
                .is_file()
        );
        std::fs::remove_file(&path).expect("replace after the look");
        std::os::windows::fs::symlink_file(&elsewhere, &path).expect("a replacement symlink");
        assert!(
            open_reading(&path).is_none(),
            "the open followed the replacement link"
        );
        assert_eq!(read_plain(&path, 100), None);
        assert_eq!(read_plain_tail(&path, 100), None);
        assert!(
            open_plain(scratch.path()).is_none(),
            "a directory must not open"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_link_is_not_followed_and_a_pipe_is_not_waited_on() {
        let scratch = tempfile::tempdir().expect("a scratch directory");
        let real = scratch.path().join("real.log");
        std::fs::write(&real, b"line\n").expect("a file");
        let link = scratch.path().join("link.log");
        std::os::unix::fs::symlink(&real, &link).expect("a link");
        assert_eq!(read_plain(&link, 100), None, "a link to a plain file");
        assert_eq!(read_plain_tail(&link, 100), None);
        let pipe = scratch.path().join("pipe.log");
        let name = std::ffi::CString::new(std::os::unix::ffi::OsStrExt::as_bytes(pipe.as_os_str()))
            .expect("a name");
        // SAFETY: `name` is a NUL-terminated path that outlives the call.
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0, "mkfifo");
        // A reader on a pipe with no writer waits for ever; this one is told no, at once.
        let began = std::time::Instant::now();
        assert_eq!(read_plain(&pipe, 100), None);
        assert_eq!(read_plain_tail(&pipe, 100), None);
        // The open alone, as if the name had been swapped after the look: it must
        // not follow the link, nor wait on the pipe, and the open file says what it is.
        assert!(open_reading(&link).is_none(), "the open followed a link");
        let opened = open_reading(&pipe).expect("a pipe opens without waiting");
        assert!(!opened.metadata().expect("a kind").is_file());
        assert!(open_plain(scratch.path()).is_none(), "a folder");
        assert!(began.elapsed().as_secs() < 5, "the pipe held the reader");
    }
}
