//! Bounded child-process execution for emulator capability commands.
//!
//! Streaming has its own lifetime owner in `session`; short-lived app, permission,
//! and log commands come through here instead. Both pipes are drained concurrently
//! so a noisy tool cannot deadlock, while only the newest diagnostic bytes remain
//! resident.

use std::collections::VecDeque;
use std::io::Read;
use std::process::{Command, ExitStatus, Stdio};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const PROCESS_POLL_INTERVAL: Duration = Duration::from_millis(20);
const READ_CHUNK_BYTES: usize = 8 * 1024;

#[derive(Debug)]
pub(super) struct BoundedOutput {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub truncated: bool,
}

impl BoundedOutput {
    pub(super) fn stdout_text(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }

    pub(super) fn diagnostic(&self) -> String {
        let stderr = String::from_utf8_lossy(&self.stderr);
        let stdout = String::from_utf8_lossy(&self.stdout);
        let detail = if stderr.trim().is_empty() {
            stdout.trim()
        } else {
            stderr.trim()
        };
        if detail.is_empty() {
            self.status.code().map_or_else(
                || "프로세스가 종료됐습니다".to_string(),
                |code| format!("종료 코드 {code}"),
            )
        } else {
            detail.to_string()
        }
    }

    pub(super) fn ensure_success(self, label: &str) -> Result<Self, String> {
        if self.status.success() {
            Ok(self)
        } else {
            Err(format!("{label}: {}", self.diagnostic()))
        }
    }
}

struct Captured {
    bytes: Vec<u8>,
    truncated: bool,
}

/// Run one already-argument-separated command with a wall-clock and retained
/// output ceiling. The full stream is consumed, but only a tail is kept so the
/// most useful error/log lines survive a cap.
pub(super) fn run_bounded(
    mut command: Command,
    timeout: Duration,
    max_output_bytes: usize,
) -> Result<BoundedOutput, String> {
    let limit = max_output_bytes.max(1);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|error| format!("에뮬레이터 명령을 시작하지 못했습니다: {error}"))?;
    let Some(stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Err("에뮬레이터 명령의 표준 출력이 없습니다".to_string());
    };
    let Some(stderr) = child.stderr.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Err("에뮬레이터 명령의 진단 출력이 없습니다".to_string());
    };
    let stdout_reader = match spawn_reader("emulator-command-stdout", stdout, limit) {
        Ok(reader) => reader,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
    };
    let stderr_reader = match spawn_reader("emulator-command-stderr", stderr, limit) {
        Ok(reader) => reader,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            let _ = stdout_reader.join();
            return Err(error);
        }
    };

    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(PROCESS_POLL_INTERVAL);
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err(format!(
                    "에뮬레이터 명령이 제한 시간 {}초를 넘었습니다",
                    timeout.as_secs()
                ));
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err(format!("에뮬레이터 명령을 기다리지 못했습니다: {error}"));
            }
        }
    };

    let stdout = join_reader(stdout_reader)?;
    let stderr = join_reader(stderr_reader)?;
    Ok(BoundedOutput {
        status,
        truncated: stdout.truncated || stderr.truncated,
        stdout: stdout.bytes,
        stderr: stderr.bytes,
    })
}

fn spawn_reader<R>(
    name: &str,
    reader: R,
    limit: usize,
) -> Result<JoinHandle<std::io::Result<Captured>>, String>
where
    R: Read + Send + 'static,
{
    std::thread::Builder::new()
        .name(name.to_string())
        .spawn(move || read_tail(reader, limit))
        .map_err(|error| format!("에뮬레이터 출력 회수기를 시작하지 못했습니다: {error}"))
}

fn join_reader(reader: JoinHandle<std::io::Result<Captured>>) -> Result<Captured, String> {
    reader
        .join()
        .map_err(|_| "에뮬레이터 출력 회수기가 중단됐습니다".to_string())?
        .map_err(|error| format!("에뮬레이터 출력을 읽지 못했습니다: {error}"))
}

fn read_tail(mut reader: impl Read, limit: usize) -> std::io::Result<Captured> {
    let mut kept = VecDeque::with_capacity(limit.min(READ_CHUNK_BYTES));
    let mut chunk = [0_u8; READ_CHUNK_BYTES];
    let mut truncated = false;
    loop {
        let read = reader.read(&mut chunk)?;
        if read == 0 {
            break;
        }
        retain_tail(&mut kept, &chunk[..read], limit, &mut truncated);
    }
    Ok(Captured {
        bytes: kept.into_iter().collect(),
        truncated,
    })
}

fn retain_tail(kept: &mut VecDeque<u8>, incoming: &[u8], limit: usize, truncated: &mut bool) {
    if incoming.len() >= limit {
        *truncated |= !kept.is_empty() || incoming.len() > limit;
        kept.clear();
        kept.extend(&incoming[incoming.len() - limit..]);
        return;
    }
    let overflow = kept
        .len()
        .saturating_add(incoming.len())
        .saturating_sub(limit);
    if overflow > 0 {
        *truncated = true;
        kept.drain(..overflow);
    }
    kept.extend(incoming);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capped_reader_keeps_the_newest_bytes_without_growing() {
        let mut kept = VecDeque::from(b"123456".to_vec());
        let mut truncated = false;
        retain_tail(&mut kept, b"789", 6, &mut truncated);
        assert_eq!(kept.iter().copied().collect::<Vec<_>>(), b"456789");
        assert!(truncated);

        retain_tail(&mut kept, b"abcdefgh", 6, &mut truncated);
        assert_eq!(kept.iter().copied().collect::<Vec<_>>(), b"cdefgh");
        assert!(kept.capacity() >= kept.len());
    }

    #[cfg(unix)]
    #[test]
    fn a_noisy_child_is_killed_and_collected_at_its_deadline() {
        let mut command = crate::proc::quiet_command("yes");
        command.arg("bounded-emulator-output");
        let error = run_bounded(command, Duration::from_millis(40), 1024).unwrap_err();
        assert!(error.contains("제한 시간"));
    }
}
