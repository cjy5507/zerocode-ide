//! Opt-in frame marks. The harness samples malloc size classes after `turn_end`.
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::time::Instant;

/// Open once at turn entry; no environment lookup belongs in the draw loop.
pub(super) fn begin() -> Option<File> {
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

pub(super) fn frame(file: Option<&mut File>, sample: Option<(usize, Instant)>) {
    if let (Some(file), Some((queue, started))) = (file, sample) {
        let ms = started.elapsed().as_secs_f64() * 1000.0;
        let _ = writeln!(file, "{{\"mark\":\"frame\",\"queue\":{queue},\"paint_ms\":{ms}}}");
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
        frame(file.as_mut(), sample);
        end(file);
        let marks: Vec<serde_json::Value> = std::fs::read_to_string(path).unwrap().lines()
            .map(|line| serde_json::from_str(line).unwrap()).collect();
        assert_eq!(marks[0]["mark"], "turn_start");
        assert_eq!(marks[1]["queue"], 7);
        assert!(marks[1]["paint_ms"].as_f64().unwrap() >= 0.0);
        assert_eq!(marks[2]["mark"], "turn_end");
    }
}
