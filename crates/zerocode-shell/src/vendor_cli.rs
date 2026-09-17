//! One execution of a vendor CLI, and nothing more.
//!
//! This module knows which binary to run, where to run it, which arguments and
//! stdin to give it, and how long it may wait. It deliberately does not know
//! what a refusal means: GitHub and GitLab keep their own error vocabularies.

use std::io::Write as _;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// How often bounded waits inspect the child again.
///
/// `gh.rs` and `glab.rs` separately used the same interval. It belongs here
/// because changing it must affect every vendor CLI process boundary together.
const POLL_EVERY: Duration = Duration::from_millis(10);

/// One vendor CLI. Its binary name is declared once in the vendor module.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct VendorCli {
    binary: &'static str,
}

impl VendorCli {
    pub(crate) const fn new(binary: &'static str) -> Self {
        Self { binary }
    }
}

/// One completed vendor CLI process.
///
/// This stays independent of [`std::process::Output`] so queue-backed fakes
/// can exercise account and authorization behavior without touching a real
/// credential or installed CLI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CliOutput {
    pub(crate) success: bool,
    pub(crate) stdout: String,
    pub(crate) stderr: String,
}

/// Failures to transport one call, before a vendor interprets the answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CliError {
    /// The binary was not found on the hydrated PATH.
    Missing,
    /// The process started but the call could not be completed.
    Refused(String),
}

/// Everything needed for one call.
pub(crate) struct CliCall<'a> {
    pub(crate) cwd: Option<&'a Path>,
    pub(crate) args: &'a [String],
    /// User-authored text travels through the pipe, never argv where another
    /// user on the machine could read it from the process list.
    pub(crate) stdin: Option<&'a [u8]>,
    /// `None` preserves the unbounded `output()` behavior used by GitHub.
    pub(crate) budget: Option<Duration>,
}

/// The only injectable process boundary for vendor CLIs.
pub(crate) trait CliRunner: Send + Sync {
    fn run(&self, cli: VendorCli, call: &CliCall<'_>) -> Result<CliOutput, CliError>;
}

/// The production process runner.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ProcessRunner;

impl ProcessRunner {
    fn command(cli: VendorCli, call: &CliCall<'_>) -> Command {
        let mut command = crate::proc::quiet_command(cli.binary);
        command.args(call.args);
        if let Some(cwd) = call.cwd {
            command.current_dir(cwd);
        }
        // GUI apps inherit launchd/Explorer's small PATH. Every vendor CLI
        // road uses the login shell's hydrated value, including availability
        // probes, so "installed" has one meaning throughout the window.
        if let Some(path) = crate::shell_path::hydrated() {
            command.env("PATH", path);
        }
        command
    }

    fn output(output: std::process::Output) -> CliOutput {
        CliOutput {
            success: output.status.success(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        }
    }

    fn run_unbounded(mut command: Command, stdin: Option<&[u8]>) -> Result<CliOutput, CliError> {
        let output = match stdin {
            None => command.output().map_err(|_| CliError::Missing)?,
            Some(input) => {
                let mut child = command
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn()
                    .map_err(|_| CliError::Missing)?;
                if let Some(mut pipe) = child.stdin.take() {
                    pipe.write_all(input)
                        .map_err(|error| CliError::Refused(error.to_string()))?;
                }
                child
                    .wait_with_output()
                    .map_err(|error| CliError::Refused(error.to_string()))?
            }
        };
        Ok(Self::output(output))
    }

    fn run_bounded(
        mut command: Command,
        stdin: Option<&[u8]>,
        budget: Duration,
    ) -> Result<CliOutput, CliError> {
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().map_err(|_| CliError::Missing)?;
        if let Some(input) = stdin
            && let Some(mut pipe) = child.stdin.take()
        {
            pipe.write_all(input)
                .map_err(|error| CliError::Refused(error.to_string()))?;
        }
        // Close unused stdin too: a CLI waiting for input would defeat a
        // bounded health probe.
        drop(child.stdin.take());
        let deadline = Instant::now() + budget;
        loop {
            if child
                .try_wait()
                .map_err(|error| CliError::Refused(error.to_string()))?
                .is_some()
            {
                let output = child
                    .wait_with_output()
                    .map_err(|error| CliError::Refused(error.to_string()))?;
                return Ok(Self::output(output));
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                return Err(CliError::Refused(
                    "the vendor CLI ran past its budget".into(),
                ));
            }
            std::thread::sleep(POLL_EVERY);
        }
    }
}

impl CliRunner for ProcessRunner {
    fn run(&self, cli: VendorCli, call: &CliCall<'_>) -> Result<CliOutput, CliError> {
        let command = Self::command(cli, call);
        match call.budget {
            Some(budget) => Self::run_bounded(command, call.stdin, budget),
            None => Self::run_unbounded(command, call.stdin),
        }
    }
}

/// JSON helpers whose meaning is independent of a particular vendor.
pub(crate) mod json {
    use serde_json::Value;

    /// Read one non-empty, trimmed string field.
    pub(crate) fn text(value: &Value, key: &str) -> Option<String> {
        let found = value.get(key)?.as_str()?.trim();
        (!found.is_empty()).then(|| found.to_string())
    }

    /// Read one unsigned integer field.
    pub(crate) fn number(value: &Value, key: &str) -> Option<u64> {
        value.get(key)?.as_u64()
    }

    /// Read one array field, treating a missing or differently typed field as
    /// an empty list.
    pub(crate) fn array<'a>(value: &'a Value, key: &str) -> &'a [Value] {
        value
            .get(key)
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    /// Read one boolean field.
    pub(crate) fn boolean(value: &Value, key: &str) -> Option<bool> {
        value.get(key)?.as_bool()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::json::{array, boolean, number, text};

    #[test]
    fn json_helpers_keep_type_and_text_rules_in_one_place() {
        let value = json!({
            "text": "  kept  ",
            "empty": "   ",
            "number": 7,
            "array": [1, 2],
            "boolean": true,
            "wrong": "not the requested type"
        });

        assert_eq!(text(&value, "text").as_deref(), Some("kept"));
        assert_eq!(text(&value, "empty"), None);
        assert_eq!(number(&value, "number"), Some(7));
        assert_eq!(number(&value, "wrong"), None);
        assert_eq!(array(&value, "array"), &[json!(1), json!(2)]);
        assert!(array(&value, "wrong").is_empty());
        assert_eq!(boolean(&value, "boolean"), Some(true));
        assert_eq!(boolean(&value, "wrong"), None);
    }
}

#[cfg(test)]
use std::collections::VecDeque;
#[cfg(test)]
use std::path::PathBuf;
#[cfg(test)]
use std::sync::Mutex;

/// One call recorded by [`FakeRunner`].
#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FakeCall {
    pub(crate) cli: VendorCli,
    pub(crate) cwd: Option<PathBuf>,
    pub(crate) args: Vec<String>,
    pub(crate) stdin: Option<Vec<u8>>,
    pub(crate) budget: Option<Duration>,
}

/// One call's answer read off the call itself — [`FakeRunner::answering`].
#[cfg(test)]
pub(crate) type FakeAnswer = fn(&FakeCall) -> Result<CliOutput, CliError>;

/// A deterministic runner shared by vendor module tests: replies queued in
/// call order, or one answer read off each call when what a call asked is the
/// point and not the order the calls came in (several checkouts in one tick).
#[cfg(test)]
#[derive(Debug, Default)]
pub(crate) struct FakeRunner {
    replies: Mutex<VecDeque<Result<CliOutput, CliError>>>,
    answer: Option<FakeAnswer>,
    calls: Mutex<Vec<FakeCall>>,
}

#[cfg(test)]
impl FakeRunner {
    pub(crate) fn with(replies: impl IntoIterator<Item = Result<CliOutput, CliError>>) -> Self {
        Self {
            replies: Mutex::new(replies.into_iter().collect()),
            ..Self::default()
        }
    }

    pub(crate) fn answering(answer: FakeAnswer) -> Self {
        Self {
            answer: Some(answer),
            ..Self::default()
        }
    }

    pub(crate) fn calls(&self) -> Vec<FakeCall> {
        self.calls.lock().expect("fake calls").clone()
    }
}

#[cfg(test)]
impl CliRunner for FakeRunner {
    fn run(&self, cli: VendorCli, call: &CliCall<'_>) -> Result<CliOutput, CliError> {
        let asked = FakeCall {
            cli,
            cwd: call.cwd.map(Path::to_path_buf),
            args: call.args.to_vec(),
            stdin: call.stdin.map(<[u8]>::to_vec),
            budget: call.budget,
        };
        self.calls.lock().expect("fake calls").push(asked.clone());
        if let Some(answer) = self.answer {
            return answer(&asked);
        }
        self.replies
            .lock()
            .expect("fake replies")
            .pop_front()
            .expect("unexpected vendor CLI call")
    }
}
