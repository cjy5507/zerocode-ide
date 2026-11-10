//! Walking a vendor's transcript tree, and reading it a line at a time.
//!
//! Two vendors keep their records in the same shape of place — a directory of
//! newline-delimited JSON under the person's home — and count entirely
//! differently once a line is in hand. What they share is the walking and the
//! reading, which is what lives here; what they do not share is what a line
//! MEANS, which stays in `claude_tokens` and `codex_tokens`.
//!
//! The bounds are the point of having this at all. A scan walks whatever the
//! vendor laid down, and a corpus is large: 915 MB for one on this machine and
//! 4.4 GB for the other, with a single 504 MB file in it. Unbounded recursion
//! turns a symlink into a hang, and reading a file into a `String` first puts
//! the corpus in memory to count it.

use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use zerocode_core::rollout_digest::{self, Stamp};

/// How deep a walk goes below the root it was given.
///
/// The measured maximum is 5 for Claude's layout and 4 for Codex's. Eight
/// leaves room for a layout the vendors have not shipped yet without letting a
/// loop run forever.
pub(crate) const MAX_DEPTH: u8 = 8;

/// The longest line either reader will hold.
///
/// The largest record measured across both corpora is 1,747,798 bytes — one
/// turn that pasted a file in. Four mebibytes is twice that headroom, and the
/// point of the cap is that a single pathological line cannot make a scan
/// allocate without bound.
pub(crate) const MAX_RECORD_BYTES: u64 = 4 * 1024 * 1024;

/// Every file under `root` whose name ends in `suffix`, to [`MAX_DEPTH`].
///
/// Sorted, because two scans of the same tree should answer in the same order:
/// a rollup keyed by day does not care, but a reader comparing two runs does,
/// and `read_dir` promises nothing.
pub(crate) fn files_under(root: &Path, suffix: &str) -> Vec<PathBuf> {
    let mut found = Vec::new();
    walk(root, &|name| name.ends_with(suffix), MAX_DEPTH, &mut found);
    found.sort();
    found
}

/// [`files_under`], and the records its CLI has since compressed where they
/// lay — `<name>.zst`, [`zerocode_core::record_file`] — for a vendor whose
/// CLI does that: Codex turns a rollout idle for a week into
/// `….jsonl.zst` (0.164). [`for_each_line`] reads either back as lines.
pub(crate) fn records_under(root: &Path, suffix: &str) -> Vec<PathBuf> {
    let mut found = Vec::new();
    walk(
        root,
        &|name| zerocode_core::record_file::names_record(name, suffix),
        MAX_DEPTH,
        &mut found,
    );
    found.sort();
    found
}

fn walk(root: &Path, keep: &dyn Fn(&str) -> bool, depth: u8, found: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            if depth > 0 {
                walk(&path, keep, depth - 1, found);
            }
        } else if path
            .file_name()
            .and_then(std::ffi::OsStr::to_str)
            .is_some_and(keep)
        {
            found.push(path);
        }
    }
}

/// Hand `read` every complete line of `path`, in order — the lines a
/// compressed record decodes to when its CLI compressed it
/// ([`zerocode_core::record_file`]).
///
/// One buffer for the whole file, capped per line: a line at the cap is
/// skipped rather than truncated, because half a JSON record is not a record.
/// A file that will not open is simply not read — a scan of somebody else's
/// directory has no business failing over one unreadable entry — and one
/// whose frames stop decoding is read up to there.
pub(crate) fn for_each_line(path: &Path, mut read: impl FnMut(&str)) {
    let Ok(file) = zerocode_core::record_file::open(path) else {
        return;
    };
    let mut reader = BufReader::new(file);
    let mut line = Vec::new();
    loop {
        line.clear();
        let got = match (&mut reader)
            .take(MAX_RECORD_BYTES)
            .read_until(b'\n', &mut line)
        {
            Ok(0) => break,
            Ok(got) => got,
            Err(_) => break,
        };
        if u64::try_from(got).unwrap_or(u64::MAX) >= MAX_RECORD_BYTES {
            continue;
        }
        if let Ok(text) = std::str::from_utf8(&line) {
            read(text);
        }
    }
}

/// The folder under the window's own data where digests are kept.
const DIGEST_DIR: &str = "codex-rollout-digests";

/// What a kept digest is named by: these many hex digits of its rollout's
/// path, hashed — enough that two rollouts do not meet, and the header says
/// which rollout a digest is of when they would.
const DIGEST_NAME_HEX: usize = 32;

/// How a kept digest's name ends: a record file compressed where it lies,
/// read back through the door every rollout is read through.
const DIGEST_SUFFIX: &str = ".jsonl.zst";

/// How long a plain rollout has been left alone before it is at rest. Codex
/// appends to a rollout for as long as its session runs; one a scan caught
/// mid-conversation would be filed, changed and filed again on every look.
/// A rollout Codex compressed where it lay is at rest by that fact.
const AT_REST: Duration = Duration::from_secs(10 * 60);

/// The digests of Codex rollouts at rest
/// ([`zerocode_core::rollout_digest`]), kept under the window's own data.
///
/// The usage scans read every rollout on the disk and take three kinds of
/// record from each. Measured on one machine on 2026-11-10 (790 rollouts,
/// 1,178 MB as Codex compressed them, 3,785 MB decoded; `cargo run --release
/// -p zerocode-core --example rollout_digest_cost`): reading them whole cost
/// the ledger's reader 13.3 s and the token panel's 16.6 s a scan, 9–11 s of
/// each spent decompressing; the same two readers over the digests took
/// 0.32 s and 0.22 s, and answered alike — 86,356 events and 92,596 records,
/// every one compared. The digests are 41 MB of lines, 5.8 MB as kept.
///
/// A digest is checked against its rollout's size and modification time
/// every time ([`Stamp`]), so a rollout that changed is read again; one
/// whose rollout has gone is removed by the scan that next files another
/// ([`Digests::sweep`]). A digest that cannot be read is not an error: the
/// rollout is still there to read.
pub(crate) struct Digests {
    dir: PathBuf,
    /// Whether this scan filed a digest — the set moved, so it is swept.
    filed: std::cell::Cell<bool>,
}

impl Digests {
    /// The digests kept under `local_data_root`.
    pub(crate) fn under(local_data_root: &Path) -> Self {
        Self {
            dir: local_data_root.join(DIGEST_DIR),
            filed: std::cell::Cell::new(false),
        }
    }

    /// Where the digest of the rollout at `source` is kept.
    fn entry_of(&self, source: &str) -> PathBuf {
        use sha2::{Digest as _, Sha256};
        let named = format!("{:x}", Sha256::digest(source.as_bytes()));
        self.dir
            .join(format!("{}{DIGEST_SUFFIX}", &named[..DIGEST_NAME_HEX]))
    }

    /// Hand `read` the digest of the rollout at `path`, a line at a time: the
    /// kept one when it was made from the rollout as it lies now, and
    /// otherwise the rollout itself, cut down as it is read — the same lines
    /// either way — and filed when the rollout is at rest.
    fn for_each_line(&self, path: &Path, mut read: impl FnMut(&str)) {
        let stamped = stamp_of(path);
        let Some(source) = path.to_str() else {
            // A path that is not text names no digest; its lines are cut all
            // the same, so a scan reads one kind of line throughout.
            for_each_line(path, |line| {
                if let Some(kept) = rollout_digest::digest_line(line) {
                    read(&kept);
                }
            });
            return;
        };
        let entry = self.entry_of(source);
        if let Some((stamp, _)) = stamped
            && let Some(kept) = kept_digest(&entry, stamp, source)
        {
            kept.lines().skip(1).for_each(read);
            return;
        }
        let files = stamped.is_some_and(|(_, modified)| at_rest(path, modified));
        let mut made = files.then(String::new);
        for_each_line(path, |line| {
            let Some(kept) = rollout_digest::digest_line(line) else {
                return;
            };
            read(&kept);
            if let Some(made) = made.as_mut() {
                made.push_str(&kept);
                made.push('\n');
            }
        });
        // Filed only for a rollout that did not move while it was read: a
        // digest of half a turn under the stamp of the whole would be read as
        // the whole from then on.
        if let (Some(made), Some((stamp, _))) = (made, stamped)
            && stamp_of(path).map(|(now, _)| now) == Some(stamp)
        {
            self.file(&entry, stamp, source, &made);
        }
    }

    /// Keep `lines` as the digest of `source` as `stamp` describes it. A
    /// digest that cannot be written is one read again next time, no more.
    fn file(&self, entry: &Path, stamp: Stamp, source: &str, lines: &str) {
        if !private_dir(&self.dir) {
            return;
        }
        let mut text = rollout_digest::header_line(stamp, source);
        text.push('\n');
        text.push_str(lines);
        let squeezed = zerocode_core::record_file::compressed(text.as_bytes());
        if zerocode_core::atomic_file::write_atomically(entry, &squeezed).is_ok() {
            self.filed.set(true);
        }
    }

    /// Remove what is in the folder and is not the digest of a rollout still
    /// there — once a scan, and only by a scan that filed one: a set nobody
    /// added to has nothing new to have outlived.
    ///
    /// A rollout leaves its path when Codex archives it, when it is resumed
    /// and written plain again, and when a person deletes it; its digest
    /// would otherwise stay for good. The folder is this window's alone, so
    /// whatever else is in it is a write on its way — left alone while it is
    /// fresh, removed once it has lain there as long as a rollout at rest.
    pub(crate) fn sweep(&self) {
        if !self.filed.replace(false) {
            return;
        }
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let digest = path
                .file_name()
                .and_then(std::ffi::OsStr::to_str)
                .is_some_and(|name| name.ends_with(DIGEST_SUFFIX));
            let stands = if digest {
                first_line(&path)
                    .and_then(|line| rollout_digest::read_header(&line))
                    .is_some_and(|(_, source)| Path::new(&source).is_file())
            } else {
                // Not a digest: a write on its way to being one. Another
                // scan's, while it is fresh — the token panel and the ledger
                // file into this folder side by side — and nobody's once it
                // has lain there.
                stamp_of(&path).is_some_and(|(_, modified)| !at_rest(&path, modified))
            };
            if !stands {
                let _ = std::fs::remove_file(&path);
            }
        }
    }
}

/// [`for_each_line`] for a scan of Codex rollouts: through `digests` when the
/// scan keeps them, and the rollout's own lines when it does not.
pub(crate) fn for_each_rollout_line(
    path: &Path,
    digests: Option<&Digests>,
    read: impl FnMut(&str),
) {
    match digests {
        Some(digests) => digests.for_each_line(path, read),
        None => for_each_line(path, read),
    }
}

/// The folder digests are kept in, made for its owner alone where the
/// platform has such a thing — a digest holds the folders a person worked in.
fn private_dir(dir: &Path) -> bool {
    if dir.is_dir() {
        return true;
    }
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        builder.mode(0o700);
    }
    builder.create(dir).is_ok()
}

/// What the rollout at `path` is now, and when it was last written.
fn stamp_of(path: &Path) -> Option<(Stamp, SystemTime)> {
    let held = std::fs::metadata(path).ok()?;
    let modified = held.modified().ok()?;
    let since = modified.duration_since(UNIX_EPOCH).ok()?;
    Some((
        Stamp {
            len: held.len(),
            modified_ns: u64::try_from(since.as_nanos()).ok()?,
        },
        modified,
    ))
}

/// Whether a rollout last written at `modified` has been left alone: Codex
/// compressed it, or [`AT_REST`] has passed. A time ahead of the clock is
/// not a time that has passed.
fn at_rest(path: &Path, modified: SystemTime) -> bool {
    zerocode_core::record_file::is_compressed(path)
        || SystemTime::now()
            .duration_since(modified)
            .is_ok_and(|age| age >= AT_REST)
}

/// The digest kept at `entry`, whole, when it was made from `source` as
/// `stamp` describes it. Read whole before a line of it is handed on: a
/// digest that stops decoding half way must not be half of an answer.
fn kept_digest(entry: &Path, stamp: Stamp, source: &str) -> Option<String> {
    let text = zerocode_core::record_file::read_to_string(entry).ok()?;
    let made_from = rollout_digest::read_header(text.lines().next()?)?;
    (made_from.0 == stamp && made_from.1 == source).then_some(text)
}

/// The first line of the record at `path`, without reading the rest.
fn first_line(path: &Path) -> Option<String> {
    let mut line = String::new();
    BufReader::new(zerocode_core::record_file::open(path).ok()?)
        .read_line(&mut line)
        .ok()?;
    Some(line)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A record its CLI compressed in place is found by the walk that asks for
    /// them and read back as its lines; the plain walk does not see it.
    #[test]
    fn a_compressed_record_is_walked_when_asked_for_and_read_as_lines() {
        let root = tempfile::tempdir().expect("tempdir");
        let squeezed = ruzstd::encoding::compress_to_vec(
            &b"{\"a\":1}\n{\"b\":2}\n"[..],
            ruzstd::encoding::CompressionLevel::Fastest,
        );
        std::fs::write(root.path().join("rollout-1.jsonl.zst"), squeezed).expect("write");
        std::fs::write(root.path().join("rollout-2.jsonl"), "{\"c\":3}\n").expect("write");
        std::fs::write(root.path().join("rollout-3.json.zst"), b"x").expect("write");
        assert_eq!(files_under(root.path(), ".jsonl").len(), 1);
        let records = records_under(root.path(), ".jsonl");
        assert_eq!(records.len(), 2, "{records:?}");
        let mut seen = Vec::new();
        for path in &records {
            for_each_line(path, |line| seen.push(line.trim_end().to_string()));
        }
        assert_eq!(seen, ["{\"a\":1}", "{\"b\":2}", "{\"c\":3}"]);
        // A file that only wears the name is read as far as it decodes: no line.
        std::fs::write(root.path().join("rollout-0.jsonl.zst"), b"not zstd").expect("write");
        let mut none = 0;
        for_each_line(&root.path().join("rollout-0.jsonl.zst"), |_| none += 1);
        assert_eq!(none, 0);
    }

    /// One `token_count` record: a running total after `turns` turns.
    fn spent(second: u32, turns: u64) -> String {
        let usage = |times: u64| {
            serde_json::json!({
                "input_tokens": 1200 * times, "cached_input_tokens": 1000 * times,
                "output_tokens": 80 * times, "reasoning_output_tokens": 30 * times,
                "total_tokens": 1280 * times,
            })
        };
        serde_json::json!({
            "timestamp": format!("2026-05-30T01:00:{second:02}.000Z"),
            "type": "event_msg",
            "payload": {
                "type": "token_count",
                "info": { "total_token_usage": usage(turns), "last_token_usage": usage(1) },
                "rate_limits": { "primary": { "used_percent": 12.5 } },
            },
        })
        .to_string()
    }

    /// A rollout of `turns` turns: its header, a turn header, the turns —
    /// and, between them, the records a scan reads nothing of.
    fn rollout(turns: u64) -> String {
        let mut lines = vec![
            serde_json::json!({
                "timestamp": "2026-05-30T01:00:00.000Z", "type": "session_meta",
                "payload": { "id": "s-1", "cwd": "/work/app", "instructions": "x".repeat(400) },
            })
            .to_string(),
            r#"{"type":"response_item","payload":{"type":"message","content":"hello"}}"#
                .to_string(),
            r#"{"timestamp":"2026-05-30T01:00:01.000Z","type":"turn_context","payload":{"cwd":"/work/app","model":"gpt-5.5","summary":"auto"}}"#
                .to_string(),
        ];
        for turn in 1..=turns {
            lines.push(spent(u32::try_from(turn).unwrap_or(0) + 1, turn));
            lines.push(
                r#"{"type":"response_item","payload":{"type":"message","content":"…"}}"#
                    .to_string(),
            );
        }
        lines.join("\n") + "\n"
    }

    fn squeeze(text: &str) -> Vec<u8> {
        ruzstd::encoding::compress_to_vec(
            text.as_bytes(),
            ruzstd::encoding::CompressionLevel::Fastest,
        )
    }

    /// What a reader is handed for `path`, whole or through `digests`.
    fn handed(digests: Option<&Digests>, path: &Path) -> Vec<String> {
        let mut seen = Vec::new();
        for_each_rollout_line(path, digests, |line| {
            seen.push(line.trim_end_matches(['\n', '\r']).to_string());
        });
        seen
    }

    fn cut(text: &str) -> Vec<String> {
        text.lines()
            .filter_map(rollout_digest::digest_line)
            .collect()
    }

    fn kept_under(data: &Path) -> Vec<PathBuf> {
        let mut held: Vec<PathBuf> = std::fs::read_dir(data.join(DIGEST_DIR))
            .map(|entries| entries.flatten().map(|entry| entry.path()).collect())
            .unwrap_or_default();
        held.sort();
        held
    }

    fn set_modified(path: &Path, to: SystemTime) {
        std::fs::OpenOptions::new()
            .write(true)
            .open(path)
            .and_then(|file| file.set_modified(to))
            .expect("set the modification time");
    }

    /// A rollout at rest is cut down once, filed, and read from its digest
    /// after — the rollout itself not opened again until it changes.
    #[test]
    fn a_rollout_at_rest_is_cut_down_once_and_read_from_its_digest_after() {
        let root = tempfile::tempdir().expect("tempdir");
        let data = tempfile::tempdir().expect("tempdir");
        let path = root.path().join("rollout-a.jsonl.zst");
        let text = rollout(3);
        std::fs::write(&path, squeeze(&text)).expect("write");

        // No digests asked for: the rollout's own lines, as before.
        assert_eq!(handed(None, &path), text.lines().collect::<Vec<_>>());
        assert!(kept_under(data.path()).is_empty());

        let digests = Digests::under(data.path());
        let first = handed(Some(&digests), &path);
        assert_eq!(first, cut(&text));
        assert_eq!(first.len(), 5, "a header, a turn header, three turns");
        let kept = kept_under(data.path());
        assert_eq!(kept.len(), 1, "{kept:?}");
        let name = kept[0]
            .file_name()
            .and_then(|name| name.to_str())
            .expect("a name");
        assert_eq!(name.len(), DIGEST_NAME_HEX + DIGEST_SUFFIX.len(), "{name}");
        assert!(
            !name.contains("rollout"),
            "a digest is not named after its rollout"
        );
        let (stamp, source) = first_line(&kept[0])
            .and_then(|line| rollout_digest::read_header(&line))
            .expect("a header");
        assert_eq!(source, path.to_string_lossy());
        assert_eq!(Some(stamp), stamp_of(&path).map(|(stamp, _)| stamp));

        // Other bytes of the same length under the same time: a reader that
        // opened the rollout would be handed nothing, and this one is handed
        // the digest.
        let (_, written) = stamp_of(&path).expect("a stamp");
        let length = usize::try_from(stamp.len).expect("a length");
        std::fs::write(&path, vec![b'x'; length]).expect("write");
        set_modified(&path, written);
        assert!(handed(None, &path).is_empty());
        assert_eq!(handed(Some(&Digests::under(data.path())), &path), first);

        // The rollout changed: read again, and the digest made again in place.
        let longer = rollout(4);
        std::fs::write(&path, squeeze(&longer)).expect("write");
        let again = handed(Some(&Digests::under(data.path())), &path);
        assert_eq!(again, cut(&longer));
        assert_eq!(again.len(), first.len() + 1);
        assert_eq!(kept_under(data.path()), kept);
        assert_eq!(handed(Some(&Digests::under(data.path())), &path), again);
    }

    /// A plain rollout still being written is cut down like any other and
    /// not filed; left alone long enough, it is.
    #[test]
    fn a_rollout_still_being_written_is_not_filed_until_it_rests() {
        let root = tempfile::tempdir().expect("tempdir");
        let data = tempfile::tempdir().expect("tempdir");
        let path = root.path().join("rollout-live.jsonl");
        let text = rollout(2);
        std::fs::write(&path, &text).expect("write");
        let digests = Digests::under(data.path());
        assert_eq!(handed(Some(&digests), &path), cut(&text));
        assert!(
            kept_under(data.path()).is_empty(),
            "a live rollout was filed"
        );

        // A clock ahead of the file is not rest either.
        set_modified(&path, SystemTime::now() + Duration::from_secs(3600));
        assert_eq!(handed(Some(&digests), &path), cut(&text));
        assert!(kept_under(data.path()).is_empty());

        set_modified(&path, SystemTime::now() - AT_REST - Duration::from_secs(5));
        assert_eq!(handed(Some(&digests), &path), cut(&text));
        assert_eq!(kept_under(data.path()).len(), 1);
        assert_eq!(handed(Some(&digests), &path), cut(&text));
    }

    /// A digest that cannot be trusted — cut short, not a record at all, of
    /// another format, of another rollout — is not read: the rollout is, each
    /// line handed once, and the digest is made whole again.
    #[test]
    fn a_digest_that_cannot_be_trusted_is_made_again_and_no_line_is_handed_twice() {
        let root = tempfile::tempdir().expect("tempdir");
        let data = tempfile::tempdir().expect("tempdir");
        let path = root.path().join("rollout-a.jsonl.zst");
        let text = rollout(3);
        std::fs::write(&path, squeeze(&text)).expect("write");
        let expected = cut(&text);
        assert_eq!(handed(Some(&Digests::under(data.path())), &path), expected);
        let entry = kept_under(data.path()).remove(0);
        let whole = std::fs::read(&entry).expect("the digest");
        let (stamp, source) = first_line(&entry)
            .and_then(|line| rollout_digest::read_header(&line))
            .expect("a header");
        let body = expected.join("\n") + "\n";
        let other_format = rollout_digest::header_line(stamp, &source).replace(
            &format!("\"zerocode_rollout_digest\":{}", rollout_digest::VERSION),
            "\"zerocode_rollout_digest\":0",
        );
        let spoiled: [(&str, Vec<u8>); 5] = [
            ("cut short", whole[..whole.len() / 2].to_vec()),
            ("not a record", b"not zstd at all".to_vec()),
            (
                "another format",
                squeeze(&format!("{other_format}\n{body}")),
            ),
            (
                "another rollout",
                squeeze(&format!(
                    "{}\n{body}",
                    rollout_digest::header_line(stamp, "/elsewhere/rollout-z.jsonl.zst")
                )),
            ),
            ("no header", squeeze(&body)),
        ];
        for (what, bytes) in spoiled {
            std::fs::write(&entry, bytes).expect("spoil");
            assert_eq!(
                handed(Some(&Digests::under(data.path())), &path),
                expected,
                "{what}"
            );
            assert_eq!(std::fs::read(&entry).expect("the digest"), whole, "{what}");
        }
    }

    /// The digest of a rollout that has gone is removed, with whatever else
    /// lies in the folder — by the scan that files another, and not by one
    /// that only read.
    #[test]
    fn the_digest_of_a_rollout_that_has_gone_is_swept_by_the_scan_that_files_another() {
        let root = tempfile::tempdir().expect("tempdir");
        let data = tempfile::tempdir().expect("tempdir");
        let write = |name: &str, turns: u64| {
            let path = root.path().join(name);
            std::fs::write(&path, squeeze(&rollout(turns))).expect("write");
            path
        };
        let (gone, stays) = (
            write("rollout-a.jsonl.zst", 1),
            write("rollout-b.jsonl.zst", 2),
        );
        let first = Digests::under(data.path());
        handed(Some(&first), &gone);
        handed(Some(&first), &stays);
        first.sweep();
        assert_eq!(kept_under(data.path()).len(), 2);

        std::fs::remove_file(&gone).expect("remove");
        // Two writes that never became digests: one another scan is still
        // making, one left behind long ago.
        let stray = data.path().join(DIGEST_DIR).join("left-behind.tmp");
        std::fs::write(&stray, b"half a write").expect("write");
        set_modified(&stray, SystemTime::now() - AT_REST - Duration::from_secs(5));
        let in_flight = data.path().join(DIGEST_DIR).join("being-written.tmp");
        std::fs::write(&in_flight, b"half a write").expect("write");
        let reading = Digests::under(data.path());
        handed(Some(&reading), &stays);
        reading.sweep();
        assert_eq!(
            kept_under(data.path()).len(),
            4,
            "a scan that filed nothing swept"
        );

        let filing = Digests::under(data.path());
        handed(Some(&filing), &stays);
        handed(Some(&filing), &write("rollout-c.jsonl.zst", 3));
        filing.sweep();
        assert!(!stray.exists(), "a write left behind was kept");
        assert!(
            in_flight.exists(),
            "another scan's write in flight was removed"
        );
        std::fs::remove_file(&in_flight).expect("remove");
        let kept = kept_under(data.path());
        assert_eq!(kept.len(), 2, "{kept:?}");
        let sources: Vec<String> = kept
            .iter()
            .filter_map(|entry| first_line(entry))
            .filter_map(|line| rollout_digest::read_header(&line))
            .map(|(_, source)| source)
            .collect();
        assert!(
            sources
                .iter()
                .all(|source| !source.ends_with("rollout-a.jsonl.zst"))
                && sources.len() == 2,
            "{sources:?}"
        );
    }

    /// The walk finds what it should, stops where it should, and answers in
    /// the same order twice.
    #[test]
    fn the_walk_is_bounded_suffixed_and_ordered() {
        let root = tempfile::tempdir().expect("tempdir");
        let deep = root.path().join("a").join("b").join("c");
        std::fs::create_dir_all(&deep).expect("mkdir");
        std::fs::write(root.path().join("top.jsonl"), "").expect("write");
        std::fs::write(deep.join("rollout-1.jsonl"), "").expect("write");
        std::fs::write(deep.join("notes.txt"), "").expect("write");
        std::fs::write(deep.join("rollout-2.jsonl"), "").expect("write");

        let held = files_under(root.path(), ".jsonl");
        assert_eq!(held.len(), 3, "the walk missed or invented a file");
        assert!(
            held.iter()
                .all(|path| path.extension().is_some_and(|held| held == "jsonl"))
        );
        assert_eq!(
            held,
            files_under(root.path(), ".jsonl"),
            "two walks disagreed"
        );

        // A narrower suffix is a narrower answer.
        assert_eq!(files_under(root.path(), "rollout-2.jsonl").len(), 1);
        // A root that is not there is no files, not a failure.
        assert!(files_under(&root.path().join("nowhere"), ".jsonl").is_empty());
    }

    /// Complete lines only, and one absurd line cannot take the file down.
    #[test]
    fn a_line_past_the_cap_is_skipped_and_the_rest_still_read() {
        let root = tempfile::tempdir().expect("tempdir");
        let path = root.path().join("one.jsonl");
        let huge = "x".repeat(usize::try_from(MAX_RECORD_BYTES).unwrap_or(0) + 16);
        std::fs::write(&path, format!("first\n{huge}\nlast\n")).expect("write");

        let mut seen = Vec::new();
        for_each_line(&path, |line| seen.push(line.trim_end().to_string()));
        assert!(
            seen.contains(&"first".to_string()),
            "the first line was lost"
        );
        assert!(
            seen.contains(&"last".to_string()),
            "the reader stopped at the huge line"
        );
        assert!(
            !seen.iter().any(|line| line.len() > 1000),
            "a line past the cap was handed over anyway"
        );

        // A file that is not there is quiet.
        let mut none = 0;
        for_each_line(&root.path().join("missing.jsonl"), |_| none += 1);
        assert_eq!(none, 0);
    }
}
