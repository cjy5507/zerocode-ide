//! The output log of a background shell task. Both shell tools capture their
//! streams here, and the task registry retains the formatted log.
//!
//! The log is what the model reads back through `TaskOutput` and what the
//! task's completion re-injects, so the grammar is a wire contract and stays
//! as it is:
//!
//! ```text
//! $ <command, possibly several lines>
//! [pid 4242]
//! <a stdout line, untagged>
//! [stderr] <a stderr line>
//! [output truncated: stream still open 2s after exit]   (older pipe-backed tasks)
//! [exit 1]                                              (or `[exit signal]`)
//! ```
//!
//! A log over the registry's cap keeps only its tail behind
//! [`OLDER_OUTPUT_TRUNCATED`]; the `$ command` head is then gone, and
//! [`parse`] says so by answering `None`.
//!
//! The TUI draws a finished task as the cell the same command would have had
//! in the foreground (t-3177): [`parse`] takes the log apart into the command,
//! the untagged output in the order the streams arrived, the exit code, and
//! whether anything was cut, so the foreground formatter can be handed the
//! same shape it gets from a foreground `bash` result.

use std::fmt::Write as _;
use std::fs::File;
use std::io::{self, Read as _, Seek as _, SeekFrom};
use std::process::Command;

use crate::task_registry::{TaskReach, BACKGROUND_TASK_LIMITS};

/// The name a background bash task carries — its registry description and
/// the `name` of the completion it publishes. The reader that turns that
/// completion back into a command cell keys on it.
pub const BACKGROUND_BASH: &str = "background bash";

/// The tag a stderr line wears in the log, without its brackets.
pub const STDERR_TAG: &str = "stderr";

/// The line the registry leaves at the head of a log it had to cap.
pub const OLDER_OUTPUT_TRUNCATED: &str = "[older task output truncated]\n";

pub const COMPLETION_PREFIX: &str = "background task ";
const NOTIFICATION_PREFIX: &str = "[task notification — background process ";
const COMMAND_PREFIX: &str = "$ ";
const PID_OPEN: &str = "[pid ";
const EXIT_OPEN: &str = "[exit ";
const EXIT_SIGNAL: &str = "signal";
const DRAIN_TRUNCATED_OPEN: &str = "[output truncated: stream still open ";

/// Holds the read handles for a task's two output files. Each writer has
/// its own cursor and remains usable for as long as the child keeps it open.
#[derive(Debug)]
pub(crate) struct Capture {
    stdout: Stream,
    stderr: Stream,
}

impl Capture {
    pub(crate) fn redirect(command: &mut Command) -> io::Result<Self> {
        let (stdout, stdout_writer) = Stream::open()?;
        let (stderr, stderr_writer) = Stream::open()?;
        command.stdout(stdout_writer).stderr(stderr_writer);
        Ok(Self { stdout, stderr })
    }

    pub(crate) fn read(&mut self, finished: bool) -> (String, bool) {
        let mut text = String::new();
        let mut truncated = false;
        for (stream, label) in [
            (&mut self.stdout, None),
            (&mut self.stderr, Some(STDERR_TAG)),
        ] {
            if let Ok(skipped) = stream.read(&mut text, label, finished) {
                truncated |= skipped;
            }
        }
        (text, truncated)
    }
}

#[derive(Debug)]
struct Stream {
    file: File,
    offset: u64,
}

impl Stream {
    fn open() -> io::Result<(Self, File)> {
        let spool = tempfile::Builder::new()
            .append(true)
            .tempfile()?;
        // Reopen for an independent read position, then remove the name.
        // Unix unlinks immediately. Windows releases a deleted file on close.
        let file = spool.reopen()?;
        let (writer, path) = spool.into_parts();
        path.close()?;
        Ok((Self { file, offset: 0 }, writer))
    }

    fn read(&mut self, text: &mut String, label: Option<&str>, finished: bool) -> io::Result<bool> {
        let end = self.file.metadata()?.len();
        let start = self.offset.max(end.saturating_sub(BACKGROUND_TASK_LIMITS.tail_bytes as u64));
        if start >= end {
            return Ok(false);
        }

        // Limit the read to a snapshot of the file. A busy child can keep
        // appending while TaskOutput or the watcher reads.
        self.file.seek(SeekFrom::Start(start))?;
        let mut bytes = Vec::new();
        self.file.by_ref().take(end - start).read_to_end(&mut bytes)?;
        if !finished {
            let last_line_end = bytes.iter().rposition(|&byte| byte == b'\n');
            bytes.truncate(last_line_end.map_or(0, |end| end + 1));
        }
        if bytes.is_empty() {
            return Ok(false);
        }

        for line in String::from_utf8_lossy(&bytes).lines() {
            if let Some(label) = label {
                text.push_str(&tagged_line(label, line));
            } else {
                let _ = writeln!(text, "{line}");
            }
        }
        let skipped = start > self.offset;
        self.offset = start + bytes.len() as u64;
        Ok(skipped)
    }
}

/// The head of a fresh log: the command, then the child's pid.
#[must_use]
pub fn head(command: &str, pid: u32) -> String {
    format!("{COMMAND_PREFIX}{command}\n{PID_OPEN}{pid}]\n")
}

/// One streamed line under a tag: `[stderr] line`.
#[must_use]
pub fn tagged_line(tag: &str, line: &str) -> String {
    format!("[{tag}] {line}\n")
}

/// The closing line of an exited process: `[exit N]`, or `[exit signal]`
/// when the shell died to a signal and has no code.
#[must_use]
pub fn exit_line(code: Option<i32>) -> String {
    let code = code.map_or_else(|| EXIT_SIGNAL.to_string(), |code| code.to_string());
    format!("{EXIT_OPEN}{code}]\n")
}

enum ExitLine { Code(i32), Signal }

impl ExitLine {
    fn code(self) -> Option<i32> {
        match self { Self::Code(code) => Some(code), Self::Signal => None }
    }
}

fn parse_exit_line(line: &str) -> Option<ExitLine> {
    let word = line.strip_prefix(EXIT_OPEN)?.strip_suffix(']')?;
    if word == EXIT_SIGNAL { Some(ExitLine::Signal) } else { word.parse().ok().map(ExitLine::Code) }
}

/// The watcher's closing exit code, even when the bounded log lost its head.
#[must_use]
pub fn exit_code(log: &str) -> Option<i32> {
    log.lines().rev().find_map(parse_exit_line).and_then(ExitLine::code)
}

/// One compact lifecycle fact for the parent transcript and model notification.
#[must_use]
pub fn completion_summary(task_id: &str, log: &str) -> String {
    let code = match log.lines().rev().find_map(parse_exit_line) {
        Some(ExitLine::Code(code)) => format!("code {code}"),
        Some(ExitLine::Signal) => EXIT_SIGNAL.to_string(),
        None => "status unknown".to_string(),
    };
    let parsed = parse(log);
    let output = parsed.as_ref().map_or(log, |entry| entry.output.as_str());
    let last = output.lines().rev().find(|line| !line.trim().is_empty()
        && parse_exit_line(line).is_none() && !line.starts_with(COMMAND_PREFIX)
        && !line.starts_with(PID_OPEN) && !line.starts_with(DRAIN_TRUNCATED_OPEN)
        && *line != OLDER_OUTPUT_TRUNCATED.trim_end()).unwrap_or("(no output)");
    let last = last.strip_prefix(&format!("[{STDERR_TAG}] ")).unwrap_or(last);
    let last = core_types::text::elide_middle(last, crate::task_registry::BACKGROUND_TASK_LIMITS.notice_tail_chars);
    format!("{COMPLETION_PREFIX}{task_id} exited ({code}) — last: {last}")
}

/// Process completions point at retained output, not agent continuation tools:
/// `TaskOutput` where the session reaches it, else the output the notice
/// carries below this line.
#[must_use]
pub fn notification_header(task_id: &str, reach: TaskReach) -> String {
    match reach {
        TaskReach::TaskTools => format!("{NOTIFICATION_PREFIX}{task_id}; use TaskOutput to read its retained output]"),
        TaskReach::Completion => format!("{NOTIFICATION_PREFIX}{task_id}; its retained output follows]"),
    }
}

/// What a command started in the background answers: its task and pid, and
/// how the session's model reaches it — the task tools, or, in a session
/// given neither, the completion that brings its output back. Nothing else is
/// promised there: a `kill` of the pid can meet the shell's kill guard.
#[must_use]
pub fn started_line(task_id: &str, pid: u32, reach: TaskReach) -> String {
    match reach {
        TaskReach::TaskTools => format!(
            "Started background task {task_id} (pid {pid}). Poll output with TaskOutput(task_id), stop with TaskStop."
        ),
        TaskReach::Completion => format!(
            "Started background task {task_id} (pid {pid}). Its output comes back to you when it exits."
        ),
    }
}

#[must_use]
pub fn is_notification_header(line: &str) -> bool {
    line.starts_with(NOTIFICATION_PREFIX) && line.ends_with(']')
}

/// Separate the host-appended lifecycle line from the original process log.
/// Older completions without the line keep their original representation.
#[must_use]
pub fn split_completion_notice(body: &str) -> (&str, Option<&str>) {
    let trimmed = body.trim_end();
    if let Some((log, notice)) = trimmed.rsplit_once('\n') {
        if notice.starts_with(COMPLETION_PREFIX) { return (log, Some(notice)); }
    }
    (body, None)
}

/// The truncation notice used by older pipe-backed logs.
#[must_use]
pub fn drain_truncation_notice(grace_secs: u64) -> String {
    format!("{DRAIN_TRUNCATED_OPEN}{grace_secs}s after exit]\n")
}

/// A log taken apart for the reader that draws it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackgroundLog {
    /// The command the task ran, as the model wrote it.
    pub command: String,
    /// The `[exit N]` line's code. `None` when the process died to a signal
    /// or the log has no closing line (a watch error, a log cut before it).
    pub exit_code: Option<i32>,
    /// Every captured line in log order with its stream tag taken off.
    pub output: String,
    /// Whether any notice in the log says output was cut.
    pub truncated: bool,
}

/// Take a log apart. `None` when it does not open with the `$ command` /
/// `[pid N]` head — a capped log, or something that is not a task log.
#[must_use]
pub fn parse(log: &str) -> Option<BackgroundLog> {
    let mut lines = log.trim_start_matches(['\r', '\n']).lines();
    let first = lines.next()?.strip_prefix(COMMAND_PREFIX)?;
    let mut command = vec![first];
    loop {
        let line = lines.next()?;
        if line.starts_with(PID_OPEN) && line.ends_with(']') {
            break;
        }
        command.push(line);
    }
    let stderr_tag = format!("[{STDERR_TAG}] ");
    let mut exit_code = None;
    let mut truncated = false;
    let mut output = Vec::new();
    for line in lines {
        if let Some(code) = parse_exit_line(line) {
            exit_code = code.code();
            continue;
        }
        if line.starts_with(DRAIN_TRUNCATED_OPEN)
            || line == OLDER_OUTPUT_TRUNCATED.trim_end_matches('\n')
        {
            truncated = true;
            continue;
        }
        output.push(line.strip_prefix(&stderr_tag).unwrap_or(line));
    }
    Some(BackgroundLog {
        command: command.join("\n"),
        exit_code,
        output: output.join("\n"),
        truncated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reading_task_output_keeps_each_line_and_finalization_closes_the_log() {
        use crate::task_registry::{TaskRegistry, TaskStatus};
        use std::io::Write as _;

        let registry = TaskRegistry::new_in_memory();
        let task_id = registry.create_background_process("command", None, None).task_id;
        let (stdout, mut stdout_writer) = Stream::open().unwrap();
        let (stderr, mut stderr_writer) = Stream::open().unwrap();
        registry.capture_background_output(&task_id, Capture { stdout, stderr });

        stdout_writer.write_all(b"first\npart").unwrap();
        stderr_writer.write_all(b"warning\r\n").unwrap();
        assert_eq!(registry.output(&task_id).unwrap(), "first\n[stderr] warning\n");

        // Reading has its own cursor. The writer can finish its partial line.
        stdout_writer.write_all(b"ial").unwrap();
        assert_eq!(registry.output(&task_id).unwrap(), "first\n[stderr] warning\n");
        registry.refresh_background_output(&task_id, true);
        registry.append_output(&task_id, &exit_line(Some(0))).unwrap();
        registry.set_status(&task_id, TaskStatus::Completed).unwrap();

        // A descendant may still have the write handle when its leader exits.
        stdout_writer.write_all(b"late\n").unwrap();
        assert_eq!(
            registry.output(&task_id).unwrap(),
            "first\n[stderr] warning\npartial\n[exit 0]\n",
        );
    }

    fn log(command: &str, body: &[String], closer: &str) -> String {
        let mut log = head(command, 4242);
        for line in body {
            log.push_str(line);
        }
        log.push_str(closer);
        log
    }

    /// The start line and the completion notice name the task tools only to a
    /// session that reaches them; one that does not is told what it does
    /// reach — the output that comes back when the command exits — and the
    /// notice keeps the shape the screen finds it by.
    #[test]
    fn the_start_and_the_notice_name_only_the_road_the_session_reaches() {
        assert_eq!(
            started_line("task-1", 4242, TaskReach::TaskTools),
            "Started background task task-1 (pid 4242). Poll output with TaskOutput(task_id), stop with TaskStop."
        );
        let started = started_line("task-1", 4242, TaskReach::Completion);
        assert!(started.starts_with("Started background task task-1 (pid 4242). "), "{started}");
        assert_eq!(started, "Started background task task-1 (pid 4242). Its output comes back to you when it exits.");
        let notice = notification_header("task-1", TaskReach::Completion);
        assert!(is_notification_header(&notice), "{notice}");
        assert!(is_notification_header(&notification_header("task-1", TaskReach::TaskTools)));
        for text in [started, notice] {
            assert!(!text.contains("TaskOutput") && !text.contains("TaskStop"), "{text}");
        }
    }

    #[test]
    fn completion_does_not_report_a_multiline_command_as_process_output() {
        let source = log("python3 <<'PY'\npass\nPY", &[], &exit_line(Some(0)));
        let summary = completion_summary("task-empty", &source);
        assert!(summary.ends_with("last: (no output)"), "{summary}");
    }

    #[test]
    fn a_log_comes_apart_into_command_untagged_output_and_exit_code() {
        let body = [
            "building\n".to_string(),
            tagged_line(STDERR_TAG, "warning: unused"),
            "done\n".to_string(),
        ];
        let parsed = parse(&log("cargo build", &body, &exit_line(Some(1)))).expect("a task log");
        assert_eq!(
            parsed,
            BackgroundLog {
                command: "cargo build".to_string(),
                exit_code: Some(1),
                output: "building\nwarning: unused\ndone".to_string(),
                truncated: false,
            }
        );
    }

    #[test]
    fn a_command_of_several_lines_ends_where_the_pid_line_begins() {
        let parsed = parse(&log("python3 <<'PY'\nprint(1)\nPY", &[], &exit_line(Some(0))))
            .expect("a task log");
        assert_eq!(parsed.command, "python3 <<'PY'\nprint(1)\nPY");
        assert_eq!(parsed.exit_code, Some(0));
        assert_eq!(parsed.output, "");
    }

    #[test]
    fn a_signal_has_no_code_and_a_stop_stays_in_the_body() {
        let parsed = parse(&log("sleep 9", &[], &exit_line(None))).expect("a task log");
        assert_eq!(parsed.exit_code, None);

        let stopped = parse(&log("sleep 9", &[], "[killed by TaskStop]\n")).expect("a task log");
        assert_eq!(stopped.exit_code, None);
        assert_eq!(stopped.output, "[killed by TaskStop]");
    }

    #[test]
    fn truncation_notices_are_a_flag_not_a_line() {
        let body = ["x\n".to_string(), drain_truncation_notice(2)];
        let parsed = parse(&log("sh -c x", &body, &exit_line(Some(0)))).expect("a task log");
        assert!(parsed.truncated);
        assert_eq!(parsed.output, "x");

        let capped = format!("{OLDER_OUTPUT_TRUNCATED}tail\n{}", exit_line(Some(0)));
        assert_eq!(parse(&capped), None, "a capped log lost its command head");
    }

    #[test]
    fn something_that_is_not_a_task_log_is_none() {
        assert_eq!(parse("The reviewer found nothing.\n"), None);
        assert_eq!(parse("$ ls\nno pid line ever\n"), None);
        assert_eq!(parse(""), None);
    }
}
