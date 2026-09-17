//! What a save is being asked to DO, decided before anything is written.
//!
//! Until this rule a save meant exactly one thing — put these bytes over the
//! file they came out of — and a file deleted from disk while its tab stayed
//! open was refused with "파일이 아닙니다". The buffer in that tab is the last
//! copy of the file anywhere (the window keeps it rather than closing the tab,
//! 1-bk), so the one keystroke that could rescue it was the one keystroke that
//! did nothing. Every editor a person has used makes the file again.
//!
//! Making it again is not the same permission as writing over it, and this
//! module exists to keep the two apart. "The file is gone, put it back" and
//! "the file is not where I last saw it, write anyway" are indistinguishable
//! at the disk — both find nothing at the path — and only the second one is a
//! blind overwrite. What tells them apart is that the window was TOLD: the
//! watcher reported the delete, the tab wears the mark (`tab.gone`), and the
//! save carries that mark with it. A save that arrives without it and finds
//! nothing there is still refused, because nothing has established that the
//! absence is the one the person is looking at.
//!
//! The window spells the mark `createIfMissing`, which is the request; what it
//! MEANS is the tab's `gone` mark, which is why the argument is named for the
//! knowledge here rather than for the effect.
//!
//! Pure on purpose. The disk is looked at once, by the caller, and turned into
//! [`OnDisk`]; everything after that is a table small enough to read. A rule
//! that stats for itself can only be tested by arranging a filesystem, and the
//! arrangement that matters most here — something that is neither a file nor
//! absent — is the one a test is least likely to build.

/// What is at the path a save is aimed at.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OnDisk {
    /// A regular file. The ordinary case, and the only one a save can be
    /// written over.
    AFile,
    /// Nothing at that path at all.
    Absent,
    /// Something that is not a regular file — a directory, a socket, a
    /// symlink pointing at nothing. A save cannot be written into any of
    /// them, and a save that MADE something here would first have to remove
    /// whatever is there, which is a deletion nobody asked for.
    Other,
}

/// Which road a save takes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SaveIntent {
    /// Write over what is there — the road that has always existed, with its
    /// version check unchanged.
    Overwrite,
    /// Make the file again, at the address it had.
    Recreate,
    /// Neither. Nothing is written and nothing is created.
    Refuse,
}

/// The whole rule.
///
/// `told_deleted` is the window saying it knows this file was deleted while
/// the tab stayed open — not a general licence to create. It is only ever
/// consulted for a path with nothing at it, and it is the ONLY thing that
/// separates making a file again from writing into the dark.
///
/// A file that is there takes the ordinary road whatever the window believes,
/// and that is deliberate: a file deleted and then put back by something else
/// is a file this window has never seen. The version check on that road is
/// what says so, and being told about the deletion must not switch it off —
/// the mark would then be a way to overwrite a stranger's file without being
/// asked, which is the thing the version check exists to prevent.
pub fn intended(found: OnDisk, told_deleted: bool) -> SaveIntent {
    match (found, told_deleted) {
        (OnDisk::AFile, _) => SaveIntent::Overwrite,
        (OnDisk::Absent, true) => SaveIntent::Recreate,
        (OnDisk::Absent, false) => SaveIntent::Refuse,
        (OnDisk::Other, _) => SaveIntent::Refuse,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The file is there: nothing about this slice touches that road.
    ///
    /// Including when the window believes it was deleted. That belief is a
    /// moment old at best — the watcher polls twice a second — and a file
    /// standing at the path is the newer fact. It goes to the version check,
    /// which is what tells the person the file is not the one they read.
    #[test]
    fn a_file_that_is_there_is_saved_the_way_it_always_was() {
        assert_eq!(intended(OnDisk::AFile, false), SaveIntent::Overwrite);
        assert_eq!(
            intended(OnDisk::AFile, true),
            SaveIntent::Overwrite,
            "being told about a deletion overrode the file that is actually \
             there, so a save can bypass the version check by claiming the \
             file is gone"
        );
    }

    /// The slice itself: gone, and the window says so, so saving puts it back.
    #[test]
    fn a_file_the_window_knows_is_gone_is_made_again() {
        assert_eq!(intended(OnDisk::Absent, true), SaveIntent::Recreate);
    }

    /// And nothing else creates a file.
    ///
    /// Same disk, same absence, no mark — the two are told apart by the
    /// window having been told, and by nothing else. Without this arm the
    /// save command becomes "write these bytes to any path in the project",
    /// which is a different and much larger promise.
    #[test]
    fn a_file_that_is_simply_not_there_is_refused() {
        assert_eq!(
            intended(OnDisk::Absent, false),
            SaveIntent::Refuse,
            "a save with no knowledge of a deletion created a file anyway"
        );
    }

    /// A directory at the path is refused however it is asked.
    ///
    /// This is the case the old `!target.is_file()` refusal was written for,
    /// and it survives the slice intact: recreating "over" a directory would
    /// mean removing it first.
    #[test]
    fn something_that_is_not_a_file_is_refused_however_it_is_asked() {
        assert_eq!(intended(OnDisk::Other, false), SaveIntent::Refuse);
        assert_eq!(
            intended(OnDisk::Other, true),
            SaveIntent::Refuse,
            "a deletion mark turned a save into the removal of whatever was \
             standing at that path"
        );
    }
}
