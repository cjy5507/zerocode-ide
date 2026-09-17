//! Bounded native main-thread sampling on the existing watchdog lane.

use crate::crash::Limits;
#[cfg(any(target_os = "macos", test))]
use std::time::Instant;

/// The sampler is a bounded child of the existing resume/watchdog lane. It
/// never samples the observer's Rust stack and never starts another thread.
#[cfg(target_os = "macos")]
pub(crate) fn sample_main_thread() -> Result<Vec<String>, &'static str> {
    use std::io::Read as _;
    use std::os::unix::fs::DirBuilderExt as _;
    use std::process::Stdio;
    use std::time::Duration;
    let directory = std::env::temp_dir().join(format!("zerocode-hang-{}", uuid::Uuid::new_v4()));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&directory)
        .map_err(|_| "sample_directory")?;
    let path = directory.join("sample.txt");
    let result = (|| {
        let mut child = crate::proc::quiet_command("/usr/bin/sample")
            .args([
                std::process::id().to_string(),
                Limits::SAMPLE_SECONDS.to_string(),
                Limits::SAMPLE_INTERVAL_MS.to_string(),
            ])
            .arg("-file")
            .arg(&path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| "sample_start")?;
        let started = Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    if !status.success() {
                        return Err("sample_failed");
                    }
                    break;
                }
                Ok(None)
                    if started.elapsed() < Duration::from_millis(Limits::SAMPLE_TIMEOUT_MS) =>
                {
                    std::thread::sleep(Duration::from_millis(Limits::DEFAULT.ping_ms));
                }
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err("sample_timeout");
                }
            }
        }
        let file = crate::durable_file::open_plain_file(&path).map_err(|_| "sample_read")?;
        let mut text = String::new();
        file.take(Limits::SAMPLE_BYTES)
            .read_to_string(&mut text)
            .map_err(|_| "sample_read")?;
        let frames = main_sample_frames(&text);
        if frames.is_empty() {
            Err("sample_no_main_frames")
        } else {
            Ok(frames)
        }
    })();
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_dir(&directory);
    result
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn sample_main_thread() -> Result<Vec<String>, &'static str> {
    Err("sample_unsupported")
}

/// Keep only the main call tree's public symbols. Headers, binary paths,
/// source locations, addresses and other threads never enter crash evidence.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn main_sample_frames(text: &str) -> Vec<String> {
    let mut main = false;
    let mut frames = std::collections::VecDeque::new();
    for line in text.lines() {
        if line.contains("Thread_") {
            if main {
                break;
            }
            main = line.contains("com.apple.main-thread");
            continue;
        }
        if !main {
            continue;
        }
        let Some((tree, _)) = line.split_once("  (in ") else {
            continue;
        };
        let branch = tree.trim_start_matches(|ch: char| ch.is_whitespace() || "+!:|".contains(ch));
        let Some((count, symbol)) = branch.split_once(' ') else {
            continue;
        };
        if !count.bytes().all(|byte| byte.is_ascii_digit())
            || symbol.contains('/')
            || symbol.contains('@')
        {
            continue;
        }
        let symbol: String = symbol
            .trim()
            .chars()
            .take(Limits::TEXT_BYTES / 2)
            .map(|ch| {
                if ch.is_ascii_alphanumeric() || "_:<>{}.".contains(ch) {
                    ch
                } else {
                    '_'
                }
            })
            .collect();
        if symbol.is_empty() {
            continue;
        }
        if frames.len() == Limits::SAMPLE_FRAMES {
            frames.pop_front();
        }
        frames.push_back(format!("native::samples_{count}::{symbol}"));
    }
    // A stalled main thread is one deep stack. Keep its leaf end rather than
    // filling the crash task's small frame budget with start/main wrappers.
    frames
        .into_iter()
        .rev()
        .enumerate()
        .map(|(index, symbol)| format!("{index}: {symbol}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hang_sample_keeps_only_main_thread_symbols() {
        let sample = "Path: /Users/private/window\n    10 Thread_1 DispatchQueue_1: com.apple.main-thread (serial)\n    + 10 start  (in dyld) + 4 [0x123]\n    +   8 __CFRunLoopRun  (in CoreFoundation) + 4 [0x456]\n    +     8 -[NSApplication run]  (in AppKit) + 4 [0x789]\n    + 2 /Users/private/source  (in window)\n    10 Thread_2 worker\n    + 10 observer::sleep  (in window)\n";
        assert_eq!(
            main_sample_frames(sample),
            [
                "0: native::samples_8::__NSApplication_run_",
                "1: native::samples_8::__CFRunLoopRun",
                "2: native::samples_10::start",
            ]
        );
    }

    #[test]
    fn hang_sample_bounds_the_call_tree_and_never_guesses_a_missing_main_thread() {
        let mut sample = "1 Thread_1 com.apple.main-thread\n".to_string();
        for _ in 0..Limits::SAMPLE_FRAMES * 2 {
            sample.push_str(" + 1 public::frame  (in window) + 1\n");
        }
        assert_eq!(main_sample_frames(&sample).len(), Limits::SAMPLE_FRAMES);
        assert!(main_sample_frames("1 Thread_2 background\n + 1 wait  (in window)").is_empty());
    }

    #[test]
    #[ignore = "native sampler smoke measurement"]
    fn measure_native_main_thread_sample() {
        let started = Instant::now();
        let frames = sample_main_thread().expect("sample this process main thread");
        println!(
            "sample_ms={} frames={} first={}",
            started.elapsed().as_millis(),
            frames.len(),
            frames[0]
        );
    }
}
