//! Opt-in frame marks. The harness samples malloc size classes after `turn_end`.
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::time::Instant;

/// Open once at turn entry; no environment lookup belongs in the draw loop.
///
/// `ZO_PROBE_FRAMES` names the file and records frames alone. `ZO_PROBE_PAINT`
/// writes them beside the painter's per-row log, whose writes happen inside
/// the frame and are then part of what a frame measures.
pub(super) fn begin() -> Option<File> {
    if let Some(path) = std::env::var_os("ZO_PROBE_FRAMES").filter(|path| !path.is_empty()) {
        return open(Path::new(&path));
    }
    let mut path = std::env::var_os("ZO_PROBE_PAINT").filter(|path| !path.is_empty())?;
    path.push(".frames.jsonl");
    open(Path::new(&path))
}

fn open(path: &Path) -> Option<File> {
    let mut file = OpenOptions::new().create(true).append(true).open(path).ok()?;
    writeln!(file, "{{\"mark\":\"turn_start\",\"pid\":{}}}", std::process::id()).ok()?;
    Some(file)
}

/// The lazy arguments are the probe-off hot-path contract: neither runs.
#[inline]
pub(super) fn start<T>(enabled: bool, queue: impl FnOnce() -> usize, clock: impl FnOnce() -> T) -> Option<(usize, T)> {
    enabled.then(|| (queue(), clock()))
}

/// One frame's mark. `paint_ms` is the whole draw; the phases split it —
/// the pty's size, the commit animation's lines, and the viewport built and
/// written (`frame_ms`). `sized` and `committed` are read only while a probe
/// is open, like the clock `start` reads.
pub(super) fn frame(
    file: Option<&mut File>,
    sample: Option<(usize, Instant)>,
    sized: Option<Instant>,
    committed: Option<Instant>,
) {
    if let (Some(file), Some((queue, started))) = (file, sample) {
        let ended = Instant::now();
        let ms = |from: Instant, to: Instant| to.saturating_duration_since(from).as_secs_f64() * 1000.0;
        let sized = sized.unwrap_or(started);
        let committed = committed.unwrap_or(sized);
        let _ = writeln!(
            file,
            "{{\"mark\":\"frame\",\"queue\":{queue},\"paint_ms\":{},\"size_ms\":{},\"commit_ms\":{},\"frame_ms\":{}}}",
            ms(started, ended),
            ms(started, sized),
            ms(sized, committed),
            ms(committed, ended),
        );
    }
}

pub(super) fn end(file: Option<File>) {
    if let Some(mut file) = file {
        let _ = writeln!(file, "{{\"mark\":\"turn_end\",\"pid\":{}}}", std::process::id());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_frame_never_reads_queue_or_clock() {
        assert_eq!(start(false, || panic!("queue sampled"), || panic!("clock read")), None::<(usize, ())>);
    }

    #[test]
    fn enabled_marks_bound_frames_and_turn_end() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("marks");
        let mut file = open(&path);
        let sample = start(file.is_some(), || 7, Instant::now);
        frame(file.as_mut(), sample, Some(Instant::now()), Some(Instant::now()));
        end(file);
        let marks: Vec<serde_json::Value> = std::fs::read_to_string(path).unwrap().lines()
            .map(|line| serde_json::from_str(line).unwrap()).collect();
        assert_eq!(marks[0]["mark"], "turn_start");
        assert_eq!(marks[1]["queue"], 7);
        assert!(marks[1]["paint_ms"].as_f64().unwrap() >= 0.0);
        for phase in ["size_ms", "commit_ms", "frame_ms"] {
            assert!(marks[1][phase].as_f64().unwrap() >= 0.0, "{phase} missing");
        }
        assert_eq!(marks[2]["mark"], "turn_end");
    }
}
