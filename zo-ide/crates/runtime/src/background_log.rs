//! The output log of a background bash task — its grammar, written once here
//! for the three writers (the bash tool, the `PowerShell` tool, the task
//! registry's cap) and read once here for the one reader that draws it.
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
//! [output truncated: stream still open 2s after exit]   (only when a drain outlived the grace)
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

/// Process completions point at retained output, not agent continuation tools.
#[must_use]
pub fn notification_header(task_id: &str) -> String {
    format!("{NOTIFICATION_PREFIX}{task_id}; use TaskOutput to read its retained output]")
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

/// What the watcher appends when it sealed the log while a pipe was still
/// open, so a truncated tail is visible instead of silently missing.
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
    /// Every streamed line with its tag taken off, in the order the streams
    /// arrived — one interleaved stream, as the person's own terminal would
    /// have shown it.
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

    fn log(command: &str, body: &[String], closer: &str) -> String {
        let mut log = head(command, 4242);
        for line in body {
            log.push_str(line);
        }
        log.push_str(closer);
        log
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
