//! The AVD's quick-boot snapshot, judged before the SDK launcher reads it.
//!
//! An 11-byte `snapshot.pb` alone in `snapshots/default_boot` is not a
//! half-written save. It is the emulator's OWN record of a load it refused:
//! a `Snapshot` message carrying a version and a `failed_to_load_reason_code`
//! and nothing else. Measured 09-21 on emulator 36.4.10 — planting the August
//! stub `beat_sweep_w012` had been carrying (reason 10001) and booting the AVD
//! gave back the same 11 bytes with reason 10004, written during that boot.
//! So this rule does not second-guess the emulator; it reads back what the
//! emulator already decided, one launch too late to be of use to that launch.
//!
//! What it costs: the boot that meets such a record is cold. Three cold boots
//! of the same AVD with the stub in place measured 13,498 / 15,407 / 19,415 ms
//! to `sys.boot_completed`, against 1,108 / 1,256 / 1,658 ms resuming a real
//! one. (The failed load itself is free: three boots with the directory
//! removed entirely measured 13,747 / 14,440 / 19,411 ms — the same spread.
//! The record's cost is that it SURVIVES every exit that does not save, so
//! every launch pays the cold boot again. `beat_sweep_w012` carried its August
//! one until 09-21.)
//!
//! The rule is read off two real snapshots this AVD wrote, at their measured
//! sizes:
//!
//! | file | first save | a later save |
//! |---|---|---|
//! | `ram.bin` | 2,052,610,126 | 1,001,856,660 |
//! | `textures.bin` | 3,868,922 | 3,367,499 |
//! | `screenshot.png` | 4,767 | 248,895 |
//! | `hardware.ini` | 4,233 | 4,233 |
//! | `snapshot.pb` | 1,003 | 978 |
//! | `compatible.pb` | 0 | 0 |
//!
//! Two of those six decide it: the descriptor has to be longer than a failure
//! record, and the guest RAM image has to be there and to have been written.
//! The other four are deliberately not required — a missing `hardware.ini` or
//! `textures.bin` is the launcher's own compatibility question, while this one
//! only asks whether there is anything here to resume at all, and judging it
//! too eagerly would throw away a snapshot that works.
//!
//! Note what the table rules OUT as well: `ram.bin` is NOT `hw.ramSize`. The
//! same 2 GB AVD wrote 1.0 GB and 2.1 GB images on different exits, so the
//! floor below is a floor and not a ratio, and a RAM image truncated above it
//! is not something sizes can catch. That case is caught the way every other
//! one is — by the record the emulator writes when it fails to load it.

use std::path::{Path, PathBuf};

/// Where an AVD keeps its snapshots, and the one the emulator resumes from
/// when nobody names another.
const SNAPSHOTS_DIRECTORY: &str = "snapshots";
const DEFAULT_BOOT: &str = "default_boot";

/// What a snapshot moved out of the way is called: this, then the local day.
///
/// The prefix is also how the old ones are found again, so a name written by
/// hand in any day format is still pruned by the same rule.
const STALE_PREFIX: &str = "default_boot.stale-";
/// How many moved-aside snapshots survive, newest first.
///
/// They exist to be looked at after the fact — the 11-byte stub above is the
/// whole reason this code exists — and a broken snapshot is small by
/// definition, since what makes it broken is the missing RAM image. Two is
/// one to read and one to compare it against; a third is only disk.
const STALE_KEEP: usize = 2;
/// How many names are tried before giving up on moving one aside. Pruning
/// runs first, so only a snapshot already moved aside TODAY can collide.
const STALE_NAME_ATTEMPTS: u32 = 4;

/// The descriptor every snapshot carries, valid or not.
const SNAPSHOT_DESCRIPTOR: &str = "snapshot.pb";
/// The floor a real descriptor clears.
///
/// A failure record is four varint fields — 11 bytes measured, twice, and one
/// carrying every field the emulator can put in it is still under a hundred.
/// A real descriptor names the system image, the kernel and the disks it was
/// taken from: 978 and 1,003 bytes for the two above, and it cannot be short
/// because those are absolute paths. The floor sits an order of magnitude
/// under the real ones and an order over the record.
const DESCRIPTOR_MIN_BYTES: u64 = 128;

/// The guest RAM image, under both names the emulator has written it as —
/// `ram.bin` today, `ram.img` in the builds before it.
const RAM_IMAGES: [&str; 2] = ["ram.bin", "ram.img"];
/// What a refused `default_boot` costs, said once because both branches of
/// the log line carry it.
const COST_OF_LEAVING_IT: &str =
    "every launch onto it is a cold boot (measured 13.5-19.4 s against 1.1-1.7 s resuming)";

/// The floor a real RAM image clears — a megabyte, three orders of magnitude
/// under the 1.0-2.1 GB the measured saves wrote. It is here to catch an
/// image that was never written, not to second-guess a size: see the module
/// note on why a ratio against `hw.ramSize` would be wrong.
const RAM_MIN_BYTES: u64 = 1024 * 1024;

/// What the quick-boot snapshot of one AVD turned out to be.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum Verdict {
    /// Nothing there yet. A first boot is cold whatever anybody does, and the
    /// exit that follows it writes the snapshot the boot after resumes.
    Absent,
    Resumable,
    /// There is a `default_boot`, and it cannot be resumed. The sentence says
    /// why, in the words the window log carries.
    Broken(String),
}

/// `<avd>/snapshots/default_boot`.
pub(super) fn default_boot_of(avd_root: &Path) -> PathBuf {
    avd_root.join(SNAPSHOTS_DIRECTORY).join(DEFAULT_BOOT)
}

/// Judge one `default_boot` directory by the rule documented above.
pub(super) fn inspect(default_boot: &Path) -> Verdict {
    if !default_boot.exists() {
        return Verdict::Absent;
    }
    if !default_boot.is_dir() {
        return Verdict::Broken(format!("{DEFAULT_BOOT} is not a directory"));
    }
    let descriptor = file_bytes(&default_boot.join(SNAPSHOT_DESCRIPTOR));
    let Some(descriptor) = descriptor else {
        return Verdict::Broken(format!("no {SNAPSHOT_DESCRIPTOR}"));
    };
    if descriptor < DESCRIPTOR_MIN_BYTES {
        return Verdict::Broken(format!(
            "{SNAPSHOT_DESCRIPTOR} is {descriptor} bytes, under the {DESCRIPTOR_MIN_BYTES} a loadable one clears"
        ));
    }
    let ram = RAM_IMAGES
        .iter()
        .find_map(|name| file_bytes(&default_boot.join(name)).map(|bytes| (*name, bytes)));
    match ram {
        None => Verdict::Broken(format!("no guest RAM image ({})", RAM_IMAGES.join(" or "))),
        Some((name, bytes)) if bytes < RAM_MIN_BYTES => Verdict::Broken(format!(
            "{name} is {bytes} bytes, under the {RAM_MIN_BYTES} a written one clears"
        )),
        Some(_) => Verdict::Resumable,
    }
}

/// Move an unusable `default_boot` out of the launcher's way, and answer with
/// the line the window log should carry — or `None` when there was nothing to
/// do, which is every ordinary launch.
///
/// `today` is the local `YYYY-MM-DD` the moved directory is named after; it is
/// passed in rather than read here so the clock stays in one place and a test
/// can name a day.
pub(super) fn tidy(avd_root: &Path, today: &str) -> Option<String> {
    let default_boot = default_boot_of(avd_root);
    let Verdict::Broken(reason) = inspect(&default_boot) else {
        return None;
    };
    let snapshots = avd_root.join(SNAPSHOTS_DIRECTORY);
    let pruned = prune(&snapshots, STALE_KEEP.saturating_sub(1));
    let moved = (1..=STALE_NAME_ATTEMPTS).find_map(|attempt| {
        let name = if attempt == 1 {
            format!("{STALE_PREFIX}{today}")
        } else {
            format!("{STALE_PREFIX}{today}.{attempt}")
        };
        let target = snapshots.join(name);
        (!target.exists() && std::fs::rename(&default_boot, &target).is_ok()).then_some(target)
    });
    Some(match moved {
        Some(target) => format!(
            "emulator android stale snapshot moved aside: {} — {reason}; {COST_OF_LEAVING_IT}; moved to {} (pruned {pruned})",
            default_boot.display(),
            target.display()
        ),
        None => format!(
            "emulator android stale snapshot could not be moved aside: {} — {reason}; {COST_OF_LEAVING_IT}",
            default_boot.display()
        ),
    })
}

/// Keep the `keep` newest moved-aside snapshots and remove the rest, oldest
/// first. Answers how many went.
fn prune(snapshots: &Path, keep: usize) -> usize {
    let Ok(entries) = std::fs::read_dir(snapshots) else {
        return 0;
    };
    let mut aside = entries
        .flatten()
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with(STALE_PREFIX))
        })
        .map(|entry| {
            let modified = entry
                .metadata()
                .and_then(|meta| meta.modified())
                .unwrap_or(std::time::UNIX_EPOCH);
            (modified, entry.path())
        })
        .collect::<Vec<_>>();
    // Newest first, so everything past `keep` is the older half.
    aside.sort_by(|left, right| right.0.cmp(&left.0));
    aside
        .into_iter()
        .skip(keep)
        .filter(|(_, path)| remove(path))
        .count()
}

fn remove(path: &Path) -> bool {
    if path.is_dir() {
        std::fs::remove_dir_all(path).is_ok()
    } else {
        std::fs::remove_file(path).is_ok()
    }
}

/// The size of one file, or `None` when it is missing or is not a file.
fn file_bytes(path: &Path) -> Option<u64> {
    let meta = std::fs::metadata(path).ok()?;
    meta.is_file().then_some(meta.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two directories the 09-21 measurement left on disk, rebuilt: the
    /// real snapshot's file list and sizes, and the 11-byte stub.
    fn write_snapshot(at: &Path, descriptor: u64, ram: Option<(&str, u64)>) {
        std::fs::create_dir_all(at).expect("snapshot directory");
        write_file(&at.join(SNAPSHOT_DESCRIPTOR), descriptor);
        if let Some((name, bytes)) = ram {
            write_file(&at.join(name), bytes);
        }
    }

    fn write_file(at: &Path, bytes: u64) {
        std::fs::write(
            at,
            vec![0_u8; usize::try_from(bytes).expect("fixture size")],
        )
        .expect("fixture file");
    }

    fn avd(root: &Path) -> PathBuf {
        let avd = root.join("probe.avd");
        std::fs::create_dir_all(avd.join(SNAPSHOTS_DIRECTORY)).expect("snapshots directory");
        avd
    }

    #[test]
    fn the_measured_snapshot_is_resumable_and_the_measured_stub_is_not() {
        let home = tempfile::tempdir().expect("home");
        let avd = avd(home.path());
        let default_boot = default_boot_of(&avd);

        assert_eq!(inspect(&default_boot), Verdict::Absent, "a first boot");

        // The one the 09-21 exit wrote, at its measured sizes.
        write_snapshot(&default_boot, 1_013, Some(("ram.bin", 4 * RAM_MIN_BYTES)));
        assert_eq!(inspect(&default_boot), Verdict::Resumable);

        // The August one: an 11-byte failure stub and nothing else.
        std::fs::remove_dir_all(&default_boot).expect("clear");
        write_snapshot(&default_boot, 11, None);
        let Verdict::Broken(reason) = inspect(&default_boot) else {
            panic!("the 11-byte stub must not read as resumable");
        };
        assert!(reason.contains("11 bytes"), "the reason names it: {reason}");
    }

    #[test]
    fn a_descriptor_without_its_ram_image_is_broken_and_ram_img_still_counts() {
        let home = tempfile::tempdir().expect("home");
        let avd = avd(home.path());
        let default_boot = default_boot_of(&avd);

        write_snapshot(&default_boot, 1_013, None);
        assert!(
            matches!(inspect(&default_boot), Verdict::Broken(reason) if reason.contains("RAM")),
            "a descriptor alone cannot be resumed"
        );

        // A save killed part-way leaves the image there and empty.
        write_file(&default_boot.join("ram.bin"), 0);
        assert!(matches!(inspect(&default_boot), Verdict::Broken(_)));

        // The older builds' name is the same image.
        std::fs::remove_file(default_boot.join("ram.bin")).expect("clear");
        write_file(&default_boot.join("ram.img"), 4 * RAM_MIN_BYTES);
        assert_eq!(inspect(&default_boot), Verdict::Resumable);
    }

    #[test]
    fn a_broken_snapshot_is_moved_aside_and_a_resumable_one_is_left_alone() {
        let home = tempfile::tempdir().expect("home");
        let avd = avd(home.path());
        let default_boot = default_boot_of(&avd);

        assert_eq!(
            tidy(&avd, "2026-09-21"),
            None,
            "nothing to move on a first boot"
        );

        write_snapshot(&default_boot, 1_013, Some(("ram.bin", 4 * RAM_MIN_BYTES)));
        assert_eq!(tidy(&avd, "2026-09-21"), None, "a good snapshot stays");
        assert!(default_boot.is_dir());

        std::fs::remove_dir_all(&default_boot).expect("clear");
        write_snapshot(&default_boot, 11, None);
        let note = tidy(&avd, "2026-09-21").expect("the broken one is moved");
        assert!(!default_boot.exists(), "the launcher must not find it");
        let moved = avd
            .join(SNAPSHOTS_DIRECTORY)
            .join(format!("{STALE_PREFIX}2026-09-21"));
        assert!(moved.is_dir(), "moved to {}", moved.display());
        assert!(
            note.contains("11 bytes") && note.contains(&moved.display().to_string()),
            "the log line says what and where: {note}"
        );
    }

    #[test]
    fn the_moved_aside_ones_do_not_pile_up_and_the_oldest_goes_first() {
        let home = tempfile::tempdir().expect("home");
        let avd = avd(home.path());
        let snapshots = avd.join(SNAPSHOTS_DIRECTORY);
        // More already aside than the rule keeps, oldest to newest by mtime.
        for day in 1..=4 {
            let old = snapshots.join(format!("{STALE_PREFIX}2026-09-0{day}"));
            write_snapshot(&old, 11, None);
            filetime(&old, 1_000 + i64::from(day));
        }
        write_snapshot(&default_boot_of(&avd), 11, None);

        let note = tidy(&avd, "2026-09-21").expect("the broken one is moved");
        let left = std::fs::read_dir(&snapshots)
            .expect("snapshots")
            .flatten()
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter(|name| name.starts_with(STALE_PREFIX))
            .collect::<Vec<_>>();
        assert_eq!(left.len(), STALE_KEEP, "kept {left:?}");
        assert!(
            left.contains(&format!("{STALE_PREFIX}2026-09-21"))
                && left.contains(&format!("{STALE_PREFIX}2026-09-04")),
            "the newest survivor and today's, not the oldest: {left:?}"
        );
        assert!(note.contains("pruned 3"), "the line counts them: {note}");
    }

    #[test]
    fn a_second_break_on_the_same_day_gets_its_own_name() {
        let home = tempfile::tempdir().expect("home");
        let avd = avd(home.path());
        for _ in 0..2 {
            write_snapshot(&default_boot_of(&avd), 11, None);
            assert!(tidy(&avd, "2026-09-21").is_some());
        }
        let snapshots = avd.join(SNAPSHOTS_DIRECTORY);
        assert!(snapshots.join(format!("{STALE_PREFIX}2026-09-21")).is_dir());
        assert!(
            snapshots
                .join(format!("{STALE_PREFIX}2026-09-21.2"))
                .is_dir()
        );
    }

    /// The same judgement, run against a real AVD on this machine — the road
    /// the 09-21 measurement took, so the numbers in
    /// `docs/design/emulator-first-second-20260921.md` are this code's and
    /// not a hand-moved folder's.
    ///
    /// ```text
    /// ZEROCODE_LIVE_AVD_ROOT=<…/name.avd> \
    ///   cargo test -p zerocode-shell --bin zerocode-shell -- --ignored \
    ///   --nocapture a_live_avds_snapshot
    /// ```
    ///
    /// It moves a broken snapshot aside exactly as a launch would, and leaves
    /// a resumable one where it is.
    #[test]
    #[ignore = "reads and can move a real AVD's snapshot directory"]
    fn a_live_avds_snapshot_is_judged_and_cleared() {
        let Ok(root) = std::env::var("ZEROCODE_LIVE_AVD_ROOT") else {
            println!("LIVE: no AVD named; nothing judged");
            return;
        };
        let root = PathBuf::from(root);
        let default_boot = default_boot_of(&root);
        println!("LIVE: {}", default_boot.display());
        for name in [SNAPSHOT_DESCRIPTOR, RAM_IMAGES[0], RAM_IMAGES[1]] {
            println!(
                "LIVE:   {name} = {:?} bytes",
                file_bytes(&default_boot.join(name))
            );
        }
        println!("LIVE: verdict = {:?}", inspect(&default_boot));
        match tidy(&root, &super::super::today_local()) {
            Some(note) => println!("LIVE: {note}"),
            None => println!("LIVE: nothing moved"),
        }
    }

    /// Give a directory a modified time, so the prune order is the test's and
    /// not the filesystem's resolution.
    #[cfg(unix)]
    fn filetime(path: &Path, epoch_secs: i64) {
        let times = [libc::timeval {
            tv_sec: libc::time_t::try_from(epoch_secs).expect("fixture stamp"),
            tv_usec: 0,
        }; 2];
        let path = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).expect("path");
        // SAFETY: both pointers are valid for the call — a NUL-terminated
        // path and a two-element array of the struct `utimes` expects.
        let set = unsafe { libc::utimes(path.as_ptr(), times.as_ptr()) };
        assert_eq!(set, 0, "utimes");
    }

    #[cfg(not(unix))]
    fn filetime(_path: &Path, _epoch_secs: i64) {
        // The prune order falls back to the filesystem's own stamps, which are
        // written oldest-first by the loop above.
    }
}
