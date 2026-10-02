//! Bounded native main-thread sampling on the existing watchdog lane.

use crate::crash::Limits;
#[cfg(any(target_os = "macos", test))]
use std::time::Instant;

/// The crumb kind the watchdog writes for every frame of the sample. The crash
/// task carries those frames as its `frames:` already, so it leaves them out of
/// its `crumbs:` (`crash::task_body`): the last crumbs are the story before and
/// during the hang, and 64 sample lines written after it must not push that
/// story out of them.
pub(crate) const SAMPLE_CRUMB: &str = "main_sample_after_detection";

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
                "0: native::self_8::__NSApplication_run_",
                "1: native::self_2::start",
                "2: native::samples_8::__NSApplication_run_",
                "3: native::samples_8::__CFRunLoopRun",
                "4: native::samples_10::start",
            ]
        );
    }

    #[test]
    fn hang_sample_bounds_the_call_tree_and_never_guesses_a_missing_main_thread() {
        // One deep stack, twice the budget: the leaf end is what is kept.
        let mut sample = "1 Thread_1 com.apple.main-thread\n".to_string();
        let depth = Limits::SAMPLE_FRAMES * 2;
        for level in 0..depth {
            let indent = " ".repeat(2 * level);
            sample.push_str(&format!(
                " +{indent} 1 public::frame_{level}  (in window) + 1\n"
            ));
        }
        // One sample each is inside the sampler's own noise, so no frame leads:
        // the path itself is the report, leaf first.
        let frames = main_sample_frames(&sample);
        assert_eq!(frames.len(), Limits::SAMPLE_FRAMES);
        assert!(
            frames[0].ends_with(&format!("public::frame_{}", depth - 1)),
            "{frames:?}"
        );
        assert!(
            frames
                .last()
                .unwrap()
                .ends_with(&format!("public::frame_{}", depth - frames.len())),
            "{frames:?}"
        );
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
                "0: native::self_87::sleep",
                "1: native::samples_87::sleep",
                "2: native::samples_87::_dispatch_client_callout",
                "3: native::samples_87::_dispatch_lane_barrier_sync_invoke_and_complete",
                "4: native::samples_87::main",
                "5: native::samples_87::start",
            ]
        );
    }

    /// The same probe moving between the main queue and another one: the
    /// header says `Main Thread` and `DispatchQueue_<multiple>`. The totals
    /// after the graph's blank line are not frames even when the main
    /// thread is the graph's last section. The thread stood in two stacks, and
    /// the heavier one (54 samples) is named before the lighter (34).
    #[test]
    fn a_main_thread_between_queues_is_named_main_thread() {
        let sample = "Call graph:\n    88 Thread_51466000: Main Thread   DispatchQueue_<multiple>\n      88 start  (in dyld) + 7184  [0x1]\n        54 main  (in probe) + 72  [0x2]\n        + 54 _dispatch_lane_barrier_sync_invoke_and_complete  (in libdispatch.dylib) + 56  [0x3]\n        34 main  (in probe) + 60  [0x4]\n          34 usleep  (in libsystem_c.dylib) + 68  [0x5]\n\nTotal number in stack (recursive counted multiple, when >=5):\n        88       __semwait_signal  (in libsystem_kernel.dylib) + 8  [0x6]\n";
        assert_eq!(
            main_sample_frames(sample),
            [
                "0: native::self_54::_dispatch_lane_barrier_sync_invoke_and_complete",
                "1: native::self_34::usleep",
                "2: native::samples_54::_dispatch_lane_barrier_sync_invoke_and_complete",
                "3: native::samples_54::main",
                "4: native::samples_88::start",
            ]
        );
    }

    /// The shape of the 2026-10-01 17:28 hang's sample (symbols made generic):
    /// one heavy branch, which `sample` prints first, and then a tail of
    /// one-sample branches longer than the frame budget. The report kept the
    /// last `SAMPLE_FRAMES` lines of the tree — the tail — so a busy main
    /// thread was reported by the scatter it stood in once each, and the
    /// branch that held the second was not in the report at all.
    #[test]
    fn a_busy_main_thread_names_what_cost_the_most_not_the_scatter_at_the_tail() {
        let mut sample = String::from(
            "Call graph:\n    100 Thread_1   DispatchQueue_1: com.apple.main-thread  (serial)\n    + 100 start  (in dyld) + 1  [0x1]\n    +   100 main  (in window) + 1  [0x2]\n    +     30 heavy_pump  (in window) + 1  [0x3]\n    +       30 heavy_leaf  (in window) + 1  [0x4]\n",
        );
        // The tail outruns the frame budget, so a keep-the-last-lines rule
        // cannot have the heavy branch in what it keeps.
        const TAIL: usize = 70;
        const _: () = assert!(TAIL > Limits::SAMPLE_FRAMES);
        for at in 0..TAIL {
            sample.push_str(&format!(
                "    +     1 one_sample_{at}  (in window) + 1  [0x5]\n"
            ));
        }
        let frames = main_sample_frames(&sample);
        assert_eq!(
            frames,
            [
                "0: native::self_30::heavy_leaf",
                "1: native::samples_30::heavy_leaf",
                "2: native::samples_30::heavy_pump",
                "3: native::samples_100::main",
                "4: native::samples_100::start",
            ]
        );
    }

    /// A symbol the thread stood in under several callers is one leader, with
    /// the samples of all of them — the main thread's time in `memmove` is not
    /// the largest single stack it was in.
    #[test]
    fn a_symbol_under_several_callers_is_ranked_by_all_its_samples() {
        let sample = "Call graph:\n    60 Thread_1   DispatchQueue_1: com.apple.main-thread  (serial)\n    + 60 start  (in dyld) + 1  [0x1]\n    +   35 a  (in window) + 1  [0x2]\n    +     35 memmove  (in libsystem_platform.dylib) + 1  [0x3]\n    +   25 b  (in window) + 1  [0x4]\n    +     25 memmove  (in libsystem_platform.dylib) + 1  [0x5]\n";
        let frames = main_sample_frames(sample);
        assert_eq!(frames[0], "0: native::self_60::memmove");
        assert_eq!(frames[1], "1: native::samples_35::memmove");
    }

    /// A main thread that had recovered by the time the sample began (the
    /// 2026-09-24 06:09 report: 79 of 82 samples waiting in the run loop) says
    /// so in its first frame, instead of leaving a reader to find the wait among
    /// one-sample lines.
    #[test]
    fn an_idle_main_thread_says_so_first() {
        let sample = "Call graph:\n    82 Thread_1   DispatchQueue_1: com.apple.main-thread  (serial)\n    + 82 start  (in dyld) + 1  [0x1]\n    +   82 CFRunLoopRun  (in CoreFoundation) + 1  [0x2]\n    +     79 mach_msg2_trap  (in libsystem_kernel.dylib) + 8  [0x3]\n    +     3 handle_event  (in window) + 1  [0x4]\n";
        let frames = main_sample_frames(sample);
        assert_eq!(frames[0], "0: native::self_79::mach_msg2_trap");
        assert_eq!(frames[1], "1: native::self_3::handle_event");
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
