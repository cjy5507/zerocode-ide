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

/// How `sample` names the main thread in a call graph header: by the main
/// queue while every sample found it there, and `Main Thread` when it moved
/// between queues.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const MAIN_THREAD_MARKS: [&str; 2] = ["com.apple.main-thread", "Main Thread"];

/// The main thread's call tree, as the lines under its header. A main thread
/// inside another serial queue's `dispatch_sync` for the whole sample carries
/// THAT queue's label and neither mark (measured 2026-09-24 with a probe
/// process), so the root frame decides then: only the main thread grows from
/// dyld's `start`; every other thread grows from `thread_start` or
/// `start_wqthread`. The 2026-09-23 13:58 hang, one of the two with a screen
/// on, ended as `sample_no_main_frames` — what the label alone answers for
/// such a thread.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn main_thread_lines(text: &str) -> Vec<&str> {
    let graph = text
        .split_once("Call graph:")
        .map_or(text, |(_, graph)| graph);
    let mut sections: Vec<(&str, Vec<&str>)> = Vec::new();
    for line in graph.lines() {
        if line.contains("Thread_") {
            sections.push((line, Vec::new()));
        } else if let Some((_, lines)) = sections.last_mut() {
            // The call graph ends at its first blank line; the totals that
            // follow repeat symbols with counts and are not a stack.
            if line.trim().is_empty() {
                break;
            }
            lines.push(line);
        }
    }
    let marked = |header: &str| MAIN_THREAD_MARKS.iter().any(|mark| header.contains(mark));
    let rooted_in_dyld = |lines: &[&str]| {
        lines.first().is_some_and(|line| {
            line.split_once("  (in dyld)").is_some_and(|(tree, _)| {
                matches!(
                    tree.split_whitespace().last(),
                    Some("start" | "_dyld_start")
                )
            })
        })
    };
    sections
        .iter()
        .find(|(header, _)| marked(header))
        .or_else(|| sections.iter().find(|(_, lines)| rooted_in_dyld(lines)))
        .map(|(_, lines)| lines.clone())
        .unwrap_or_default()
}

/// Keep only the main call tree's public symbols. Headers, binary paths,
/// source locations, addresses and other threads never enter crash evidence.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn main_sample_frames(text: &str) -> Vec<String> {
    let mut frames = std::collections::VecDeque::new();
    for line in main_thread_lines(text) {
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

    /// `sample`'s own words for a main thread held inside another serial
    /// queue's `dispatch_sync` the whole second (2026-09-24 probe, addresses
    /// shortened): that queue's label, no main-thread mark — and the tree
    /// still grows from dyld's `start`.
    #[test]
    fn a_main_thread_inside_another_queue_is_still_the_main_thread() {
        let sample = "Call graph:\n    87 Thread_51463556   DispatchQueue_23: com.example.probe.serial  (serial)\n      87 start  (in dyld) + 7184  [0x1]\n        87 main  (in probe) + 36  [0x2]\n          87 _dispatch_lane_barrier_sync_invoke_and_complete  (in libdispatch.dylib) + 56  [0x3]\n            87 _dispatch_client_callout  (in libdispatch.dylib) + 16  [0x4]\n              87 sleep  (in libsystem_c.dylib) + 52  [0x5]\n    87 Thread_51463557: worker\n      87 thread_start  (in libsystem_pthread.dylib) + 8  [0x6]\n        87 work  (in probe) + 4  [0x7]\n\nTotal number in stack (recursive counted multiple, when >=5):\n        87       sleep  (in libsystem_c.dylib) + 52  [0x5]\n";
        assert_eq!(
            main_sample_frames(sample),
            [
                "0: native::samples_87::sleep",
                "1: native::samples_87::_dispatch_client_callout",
                "2: native::samples_87::_dispatch_lane_barrier_sync_invoke_and_complete",
                "3: native::samples_87::main",
                "4: native::samples_87::start",
            ]
        );
    }

    /// The same probe moving between the main queue and another one: the
    /// header says `Main Thread` and `DispatchQueue_<multiple>`. The totals
    /// after the graph's blank line are not frames even when the main
    /// thread is the graph's last section.
    #[test]
    fn a_main_thread_between_queues_is_named_main_thread() {
        let sample = "Call graph:\n    88 Thread_51466000: Main Thread   DispatchQueue_<multiple>\n      88 start  (in dyld) + 7184  [0x1]\n        54 main  (in probe) + 72  [0x2]\n        + 54 _dispatch_lane_barrier_sync_invoke_and_complete  (in libdispatch.dylib) + 56  [0x3]\n        34 main  (in probe) + 60  [0x4]\n          34 usleep  (in libsystem_c.dylib) + 68  [0x5]\n\nTotal number in stack (recursive counted multiple, when >=5):\n        88       __semwait_signal  (in libsystem_kernel.dylib) + 8  [0x6]\n";
        assert_eq!(
            main_sample_frames(sample),
            [
                "0: native::samples_34::usleep",
                "1: native::samples_34::main",
                "2: native::samples_54::_dispatch_lane_barrier_sync_invoke_and_complete",
                "3: native::samples_54::main",
                "4: native::samples_88::start",
            ]
        );
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
