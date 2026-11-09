//! A record file a CLI may have compressed where it lies.
//!
//! Codex 0.164 (2026-10-23, openai/codex#54008) compresses a rollout nobody
//! has touched for seven days in place: `rollout-<stamp>-<id>.jsonl` becomes
//! `rollout-<stamp>-<id>.jsonl.zst`, a zstd frame of the same lines, and its
//! own `codex resume` reads it back. Every reader in this window took
//! `.jsonl` alone, so on 2026-11-10 this machine's `~/.codex` held 783
//! compressed rollouts beside 7 plain ones, and the AI vault, the token totals
//! and the usage page saw the 7 — a person's Codex history shrank to its last
//! week without a word.
//!
//! This module is the one place that knows the spelling. A name with the
//! compressed tail still names its record ([`names_record`], [`record_stem`]),
//! and [`open`] reads either kind as the same bytes, so a reader that walked
//! `.jsonl` files keeps reading lines and never learns which kind it had.

use std::fs::File;
use std::io::{self, BufRead, BufReader, Read};
use std::path::Path;

use ruzstd::decoding::errors::{FrameDecoderError, ReadFrameHeaderError};
use ruzstd::decoding::{BlockDecodingStrategy, FrameDecoder};

/// The tail a compressed record carries after the name it was written under.
pub const COMPRESSED_SUFFIX: &str = ".zst";

/// `name` without the compressed tail: the name the record was written under.
#[must_use]
pub fn record_name(name: &str) -> &str {
    name.strip_suffix(COMPRESSED_SUFFIX).unwrap_or(name)
}

/// Whether `name` is a record written as `…{suffix}`, plain or compressed.
#[must_use]
pub fn names_record(name: &str, suffix: &str) -> bool {
    record_name(name).ends_with(suffix)
}

/// Whether the file at `path` is a compressed record.
#[must_use]
pub fn is_compressed(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with(COMPRESSED_SUFFIX))
}

/// The stem of the name a record was written under — `rollout-<stamp>-<id>`
/// for `….jsonl` and for `….jsonl.zst` alike, where `Path::file_stem` would
/// keep the `.jsonl` of the compressed one and name a different session.
#[must_use]
pub fn record_stem(path: &Path) -> Option<&str> {
    let name = record_name(path.file_name()?.to_str()?);
    Path::new(name).file_stem()?.to_str()
}

/// The bytes of the record at `path`: the file's own, or what its zstd frames
/// decode to when it is compressed.
///
/// # Errors
///
/// When the file cannot be opened; reading a compressed one fails where its
/// frames do not decode.
pub fn open(path: &Path) -> io::Result<RecordReader> {
    let file = File::open(path)?;
    Ok(if is_compressed(path) {
        RecordReader::Compressed(Box::new(Frames::new(file)))
    } else {
        RecordReader::Plain(file)
    })
}

/// The whole record at `path` as text, decompressed when it is compressed.
///
/// # Errors
///
/// As [`open`], and when the record is not UTF-8.
pub fn read_to_string(path: &Path) -> io::Result<String> {
    let mut text = String::new();
    open(path)?.read_to_string(&mut text)?;
    Ok(text)
}

/// A record being read, plain or through its frames — boxed, since a frame
/// decoder's tables are kilobytes and a plain file is a descriptor.
pub enum RecordReader {
    Plain(File),
    Compressed(Box<Frames>),
}

impl Read for RecordReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::Plain(file) => file.read(buf),
            Self::Compressed(frames) => frames.read(buf),
        }
    }
}

/// Every zstd frame of a file, one after another.
///
/// The decoder decodes one frame and stops, and the format lets a file hold
/// several — `zstd -d` reads them all, and appending to a compressed file is
/// how one gets a second. So between frames the reader looks for another, and
/// steps over a skippable frame (metadata a writer may leave) by its length.
/// Codex writes one frame today; the loop costs nothing there.
pub struct Frames {
    source: BufReader<File>,
    frame: FrameDecoder,
    /// Whether `frame` holds a frame not yet read to its end.
    open: bool,
}

impl Frames {
    fn new(file: File) -> Self {
        Self {
            source: BufReader::new(file),
            frame: FrameDecoder::new(),
            open: false,
        }
    }
}

impl Read for Frames {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        loop {
            if self.open {
                while self.frame.can_collect() == 0 && !self.frame.is_finished() {
                    self.frame
                        .decode_blocks(&mut self.source, BlockDecodingStrategy::UptoBlocks(1))
                        .map_err(io::Error::other)?;
                }
                let got = self.frame.read(buf)?;
                if got > 0 {
                    return Ok(got);
                }
                self.open = false;
            }
            if self.source.fill_buf()?.is_empty() {
                return Ok(0);
            }
            match self.frame.init(&mut self.source) {
                Ok(()) => self.open = true,
                Err(FrameDecoderError::ReadFrameHeaderError(ReadFrameHeaderError::SkipFrame {
                    length,
                    ..
                })) => {
                    let skipped = io::copy(
                        &mut (&mut self.source).take(u64::from(length)),
                        &mut io::sink(),
                    )?;
                    if skipped < u64::from(length) {
                        return Err(io::Error::new(
                            io::ErrorKind::UnexpectedEof,
                            "a skippable zstd frame ends past the file",
                        ));
                    }
                }
                Err(error) => return Err(io::Error::other(error)),
            }
        }
    }
}

/// Compressed records as the reference encoder writes them, for every test
/// in this crate that reads one.
#[cfg(test)]
pub(crate) mod fixture {
    /// `zstd --no-check` (CLI 1.5.7) of two rollout lines — the reference
    /// encoder, the library Codex compresses with, not this crate's decoder
    /// checking its own output.
    pub(crate) const FRAME_A: &str = "28b52ffd209ee50300c2c71a1c60c7e680cd8f250bf649e849443fe1924054088a48f4951902a70503c49c96d0101f077e10a5602347c24f27e6f4a347d9f4c7b6f00db0c3e3c2879168e28e04694f598a9e59866bcb7a536429806539e39a1b3fc855e3a79140a9e4c22a1f393042bb5f37be44495701040009cb001af25c8386085a2a0a";
    /// The same encoder, one more line, stored raw (too short to compress).
    pub(crate) const FRAME_B: &str = "28b52ffd2034a101007b2274797065223a227475726e5f636f6e74657874222c227061796c6f6164223a7b226d6f64656c223a226770742d35227d7d0a";
    /// What [`FRAME_A`] decodes to: a Codex `session_meta` and a token event.
    pub(crate) const LINES_A: &str = "{\"type\":\"session_meta\",\"payload\":{\"id\":\"019e7786-4a0e-7012-9ea6-6d045315a523\",\"cwd\":\"/Users/dev/acme\"}}\n{\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\"}}\n";
    /// What [`FRAME_B`] decodes to.
    pub(crate) const LINES_B: &str =
        "{\"type\":\"turn_context\",\"payload\":{\"model\":\"gpt-5\"}}\n";
    /// The session [`FRAME_A`]'s `session_meta` names.
    pub(crate) const SESSION_A: &str = "019e7786-4a0e-7012-9ea6-6d045315a523";

    /// A frame's bytes from its hex.
    pub(crate) fn bytes(hex: &str) -> Vec<u8> {
        (0..hex.len())
            .step_by(2)
            .map(|at| u8::from_str_radix(&hex[at..at + 2], 16).expect("hex"))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::fixture::{FRAME_A, FRAME_B, LINES_A, LINES_B, bytes};
    use super::*;

    /// A skippable frame (RFC 8878 §3.1.2): one of the sixteen magic numbers,
    /// a little-endian length, then that many bytes a decoder steps over.
    fn skippable(payload: &[u8]) -> Vec<u8> {
        let mut frame = 0x184D_2A53_u32.to_le_bytes().to_vec();
        frame.extend_from_slice(&u32::try_from(payload.len()).expect("short").to_le_bytes());
        frame.extend_from_slice(payload);
        frame
    }

    #[test]
    fn a_compressed_record_reads_as_the_lines_it_was_written_with() {
        let dir = tempfile::tempdir().expect("tempdir");
        let compressed = dir.path().join("rollout-a.jsonl.zst");
        std::fs::write(&compressed, bytes(FRAME_A)).expect("write");
        let plain = dir.path().join("rollout-a.jsonl");
        std::fs::write(&plain, LINES_A).expect("write");
        assert_eq!(read_to_string(&compressed).expect("decodes"), LINES_A);
        assert_eq!(read_to_string(&plain).expect("reads"), LINES_A);
        let lines: Vec<String> = BufReader::new(open(&compressed).expect("opens"))
            .lines()
            .map(|line| line.expect("a line"))
            .collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("session_meta"));
    }

    #[test]
    fn every_frame_is_read_and_a_skippable_one_is_stepped_over() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("rollout-ab.jsonl.zst");
        let mut file = bytes(FRAME_A);
        file.extend(skippable(b"writer's own note"));
        file.extend(bytes(FRAME_B));
        file.extend(skippable(b""));
        std::fs::write(&path, file).expect("write");
        assert_eq!(
            read_to_string(&path).expect("decodes"),
            format!("{LINES_A}{LINES_B}")
        );
    }

    #[test]
    fn a_damaged_frame_is_an_error_not_a_short_record() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("rollout-cut.jsonl.zst");
        let whole = bytes(FRAME_A);
        std::fs::write(&path, &whole[..whole.len() / 2]).expect("write");
        assert!(read_to_string(&path).is_err(), "half a frame");
        std::fs::write(&path, b"not zstd at all").expect("write");
        assert!(read_to_string(&path).is_err(), "no frame");
        let truncated_skip = &skippable(b"0123456789")[..12];
        std::fs::write(&path, truncated_skip).expect("write");
        assert!(
            read_to_string(&path).is_err(),
            "a skippable frame cut short"
        );
    }

    #[test]
    fn a_compressed_name_still_names_its_record() {
        let zst = Path::new(
            "/h/.codex/sessions/2026/05/30/rollout-2026-05-30T15-15-50-019e7786-4a0e-7012-9ea6-6d045315a523.jsonl.zst",
        );
        let plain = Path::new(
            "/h/.codex/sessions/2026/05/30/rollout-2026-05-30T15-15-50-019e7786-4a0e-7012-9ea6-6d045315a523.jsonl",
        );
        assert!(is_compressed(zst) && !is_compressed(plain));
        assert_eq!(record_stem(zst), record_stem(plain));
        assert_eq!(
            record_stem(zst),
            Some("rollout-2026-05-30T15-15-50-019e7786-4a0e-7012-9ea6-6d045315a523")
        );
        assert!(names_record("rollout-x.jsonl.zst", ".jsonl"));
        assert!(names_record("rollout-x.jsonl", ".jsonl"));
        assert!(!names_record("rollout-x.json.zst", ".jsonl"));
        assert!(!names_record("history.zst", ".jsonl"));
        assert_eq!(record_name("a.jsonl.zst"), "a.jsonl");
        assert_eq!(record_name("a.jsonl"), "a.jsonl");
    }
}
