//! The second rung of the ladder (t-6132 S3): when the screen seat's judgment
//! is under its press floor, the same closed choice is put to the frontier —
//! zo, headless, under the window's own login — before the walk steps back
//! to the person.
//!
//! What it asks is exactly what the seat asked: the question's `state` and
//! `questions` as [`zerocode_core::screen_action::ask`] rendered them, with
//! the options listed once more and one contract for the answer — a single
//! JSON line naming one offered option and a confidence. What comes back is
//! read through the question that was asked ([`ActionAsk::choice_of`]): an
//! option nobody offered, a confidence that is not a share, or no JSON at
//! all is a refusal in the closed choice's own words, and the walk steps back
//! to the person exactly as it would have without this rung.
//!
//! The reader is `zo -p`: the person's own zo, found where the release lane
//! puts it (`zo_companion::zo_path_under`), started in a session of its own
//! (never `--resume`), with no tools to spawn, read-only, `--effort low`, and
//! the model the person pinned for the router's `fast` role when they pinned
//! one — otherwise zo's own router picks, which for a one-line closed choice
//! is its easy rung (`model_router::tiering`). No model is named here. The
//! login is the window's: the launch environment every named agent launch
//! gets (`usage_runtime::account_env_for`), so a walk started from the window
//! reads the account the window's picker names, not whatever `~/.claude`
//! holds.
//!
//! The wall is one number ([`TEAM_DEADLINE`]) bounded by what the walk has
//! left: a reader that has not answered by then is killed, and its silence is
//! the wire's own word for it.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use serde_json::Value;
use zerocode_core::screen_action::ActionAsk;

use super::{ActionJudge, Judged};
use crate::systemone::{SCHEMA, TIMEOUT, TRANSPORT};

/// The most one second-reader question may hold a walk. A frontier turn on
/// a one-line closed choice is seconds, not a wire's milliseconds; twenty is
/// what the coordinator set (m-6143) and what a walk's own budget can spare
/// before the person would be asked anyway.
pub const TEAM_DEADLINE: Duration = Duration::from_secs(20);

/// How often a running reader is looked at for its exit.
const POLL: Duration = Duration::from_millis(20);

/// The effort the reader is asked at — the cheapest zo offers.
pub const EFFORT: &str = "low";

/// The router role whose pinned model, when the person pinned one, the
/// reader runs on (`modelRouter.roles.<role>` in zo's settings).
pub const FAST_ROLE: &str = "fast";

/// The words around the question. They are ours; the screen's words reach
/// the model only inside the JSON the seat already rendered.
pub const PROMPT_HEAD: &str = "You are the second reader of a closed choice that a screen walk's first reader could not decide with confidence. Read the state and the question below exactly as the first reader did.";
pub const ANSWER_CONTRACT: &str = "Answer with ONE line of JSON and nothing else, of the shape {\"choice\":\"<one of the options>\",\"confidence\":<a number from 0 to 1>}. The choice must be one of the options listed; do not explain.";

/// The second reader over zo, headless.
pub struct TeamJudge {
    program: PathBuf,
    env: Vec<(String, String)>,
    cwd: Option<PathBuf>,
    model: Option<String>,
    /// What the last question cost in wall time and bytes, for a caller
    /// that measures ([`Self::last`]).
    last: Option<Asked>,
}

/// What one second-reader question came to, apart from the answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asked {
    pub elapsed_ms: u64,
    pub prompt_bytes: usize,
    pub answer_bytes: usize,
    pub model: Option<String>,
}

impl TeamJudge {
    /// The reader as the window starts it: the person's zo, the window's
    /// login for `zo`, the walk's workspace as the session's directory, and
    /// the `fast` role's pinned model if any. `None` when there is no zo to
    /// run or no login the window may launch with — then there is no second
    /// rung, and the walk steps back to the person as it does today.
    #[must_use]
    pub fn new(config_root: &Path, workspace: Option<&Path>) -> Option<Self> {
        let home = dirs::home_dir()?;
        let program = crate::zo_companion::zo_path_under(&home);
        if !program.exists() {
            return None;
        }
        let env = crate::usage_runtime::account_env_for(config_root, "zo").ok()?;
        let settings = crate::api_routers::zo_settings_path()
            .and_then(|path| crate::api_routers::read_zo_settings_root(&path).ok())
            .map_or(Value::Null, Value::Object);
        Some(Self::at(
            program,
            env,
            workspace.map(Path::to_path_buf),
            pinned_fast_model(&settings),
        ))
    }

    /// A reader over `program` — how a test hands in a script that answers
    /// like zo does.
    #[must_use]
    pub fn at(
        program: PathBuf,
        env: Vec<(String, String)>,
        cwd: Option<PathBuf>,
        model: Option<String>,
    ) -> Self {
        Self {
            program,
            env,
            cwd,
            model,
            last: None,
        }
    }

    /// The last question's cost, if one was asked — what the measurement
    /// reads.
    #[cfg(test)]
    #[must_use]
    pub fn last(&self) -> Option<&Asked> {
        self.last.as_ref()
    }

    /// The whole prompt: the head, the state and the question as the seat
    /// rendered them, the options once more, and the answer's contract.
    #[must_use]
    pub fn prompt(ask: &ActionAsk) -> String {
        format!(
            "{PROMPT_HEAD}\n\nstate:\n{}\n\nquestion:\n{}\n\noptions: {}\n\n{ANSWER_CONTRACT}\n",
            ask.state,
            ask.questions,
            ask.press_options().join(", ")
        )
    }

    /// The reader's words read through the question: the first JSON object
    /// in them, its `choice` and `confidence`, judged by the closed choice's
    /// rules. Anything else is [`SCHEMA`].
    #[must_use]
    pub fn read_answer(ask: &ActionAsk, text: &str) -> Judged {
        let Some(object) = first_json_object(text) else {
            return Judged::Refused(SCHEMA.to_string());
        };
        let (Some(choice), Some(confidence)) = (
            object.get("choice").and_then(Value::as_str),
            object.get("confidence").and_then(Value::as_f64),
        ) else {
            return Judged::Refused(SCHEMA.to_string());
        };
        match ask.choice_of(choice, confidence) {
            Ok(choice) => Judged::Chose(choice.into()),
            Err(why) => Judged::Refused(why.token().to_string()),
        }
    }

    /// The arguments zo is started with — one session of its own, headless,
    /// no tools to spawn, read-only, the cheapest effort, the answer written
    /// to `last_message`, and the pinned model when there is one.
    fn argv(&self, last_message: &Path) -> Vec<String> {
        let mut argv = vec![
            "-p".to_string(),
            "--no-spawn".to_string(),
            "--permission-mode".to_string(),
            "read-only".to_string(),
            "--effort".to_string(),
            EFFORT.to_string(),
            "--last-message".to_string(),
            last_message.to_string_lossy().into_owned(),
        ];
        if let Some(model) = &self.model {
            argv.push("--model".to_string());
            argv.push(model.clone());
        }
        if let Some(cwd) = &self.cwd {
            argv.push("--cwd".to_string());
            argv.push(cwd.to_string_lossy().into_owned());
        }
        argv
    }

    /// One question, bounded by `deadline`: the prompt on the reader's
    /// stdin, the answer read from the file it was told to write, the reader
    /// killed if it outlives the wall.
    fn run(&self, prompt: &str, deadline: Duration) -> Result<String, String> {
        let last_message = std::env::temp_dir().join(format!(
            "zerocode-team-answer-{}-{}.txt",
            std::process::id(),
            Instant::now().elapsed().as_nanos()
        ));
        let _ = std::fs::remove_file(&last_message);
        let mut command = crate::proc::quiet_command(&self.program);
        command
            .args(self.argv(&last_message))
            .envs(self.env.iter().map(|(name, value)| (name, value)))
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if let Some(cwd) = &self.cwd {
            command.current_dir(cwd);
        }
        let mut child = command.spawn().map_err(|_| TRANSPORT.to_string())?;
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(prompt.as_bytes());
        }
        let began = Instant::now();
        let answered = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status.success(),
                Ok(None) if began.elapsed() < deadline => std::thread::sleep(POLL),
                Ok(None) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = std::fs::remove_file(&last_message);
                    return Err(TIMEOUT.to_string());
                }
                Err(_) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = std::fs::remove_file(&last_message);
                    return Err(TRANSPORT.to_string());
                }
            }
        };
        let text = std::fs::read_to_string(&last_message);
        let _ = std::fs::remove_file(&last_message);
        match (answered, text) {
            (true, Ok(text)) => Ok(text),
            _ => Err(TRANSPORT.to_string()),
        }
    }
}

/// The model the person pinned for `modelRouter.roles.fast` in zo's settings,
/// if they pinned one (`{"mode":"pinned","model":"…"}`, zo's own shape).
#[must_use]
pub fn pinned_fast_model(settings: &Value) -> Option<String> {
    let role = settings.get("modelRouter")?.get("roles")?.get(FAST_ROLE)?;
    if role.get("mode").and_then(Value::as_str) != Some("pinned") {
        return None;
    }
    role.get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .map(str::to_string)
}

/// The first JSON object in `text`, parsed — a reader that wrapped its line
/// in a sentence or a fence is still read; one that wrote no object is not.
fn first_json_object(text: &str) -> Option<Value> {
    let start = text.find('{')?;
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (offset, ch) in text[start..].char_indices() {
        if in_string {
            match ch {
                '\\' if !escaped => escaped = true,
                '"' if !escaped => in_string = false,
                _ => escaped = false,
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return serde_json::from_str(&text[start..=start + offset]).ok();
                }
            }
            _ => {}
        }
    }
    None
}

impl ActionJudge for TeamJudge {
    fn choose(&mut self, ask: &ActionAsk) -> Judged {
        self.choose_within(ask, TEAM_DEADLINE)
    }

    fn choose_within(&mut self, ask: &ActionAsk, left: Duration) -> Judged {
        let deadline = TEAM_DEADLINE.min(left);
        if deadline.is_zero() {
            return Judged::Refused(TIMEOUT.to_string());
        }
        let prompt = Self::prompt(ask);
        let began = Instant::now();
        let answered = self.run(&prompt, deadline);
        self.last = Some(Asked {
            elapsed_ms: u64::try_from(began.elapsed().as_millis()).unwrap_or(u64::MAX),
            prompt_bytes: prompt.len(),
            answer_bytes: answered.as_ref().map_or(0, String::len),
            model: self.model.clone(),
        });
        match answered {
            Ok(text) => Self::read_answer(ask, &text),
            Err(token) => Judged::Refused(token),
        }
    }
}

#[cfg(test)]
pub(super) mod tests;
