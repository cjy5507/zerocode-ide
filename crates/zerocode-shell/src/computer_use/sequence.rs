//! A batch or a recipe owns the apps it walks, and only those.
//!
//! A sequence holding the whole window made every other agent wait or fail while
//! it ran, whatever app they worked in. Numbers now outlive a look and stand for
//! one element, so another app's act cannot disturb a sequence: only acts in the
//! same app are held back. A request waits its turn instead of being refused, up
//! to a bound. Looks, passive sensors and the emergency stop never wait.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::sync::{OwnedRwLockReadGuard, OwnedRwLockWriteGuard, RwLock};
use zerocode_core::computer_recipe::{RecipeTool, recipe_fill, recipe_lines};
use zerocode_core::computer_use::{
    ComputerCommand, ComputerMethod, batch_steps, parallel_steps, parse_command,
};

use super::ComputerUseError;

/// What acts with no app share: the shared pointer and keyboard.
const DESKTOP: &str = "desktop";
/// How long a request waits for a sequence in its app before it is told so.
const TURN_WAIT: Duration = Duration::from_secs(30);

#[derive(Clone, Default)]
pub struct SequenceGate {
    gates: Arc<Mutex<HashMap<String, Arc<RwLock<()>>>>>,
}

#[derive(Debug, Default)]
pub struct SequenceLease {
    _shared: Option<OwnedRwLockReadGuard<()>>,
    _exclusive: Vec<OwnedRwLockWriteGuard<()>>,
    /// How long the request waited for its turn, and in which place it waited
    /// longest — what its answer says when the wait was one a person would
    /// notice ([`QUEUE_NOTE_FROM`]).
    pub waited: Duration,
    pub waited_for: Option<String>,
}

/// How long a wait for a turn must be before the answer says it: shorter is
/// the lock's own cost, longer is another agent's work the caller should know
/// it stood behind.
pub const QUEUE_NOTE_FROM: Duration = Duration::from_millis(100);

/// Whether a request reads its window and then presses where it read: a press
/// by the words the window shows (`click --ocr`). Between its read and its
/// press, another agent's keys into the same app could change the window, and
/// the press would land where the words used to be — a mirrored phone's row
/// replaced under it (2026-11-09). So it holds its app alone, as a sequence
/// does, for the time it takes.
fn reads_then_presses(command: &ComputerCommand) -> bool {
    command.method == ComputerMethod::Click
        && command.params.get("ocr").and_then(Value::as_bool) == Some(true)
}

impl SequenceGate {
    fn gate(&self, target: &str) -> Arc<RwLock<()>> {
        let mut gates = self
            .gates
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Arc::clone(gates.entry(target.to_owned()).or_default())
    }

    /// Wait for the request's turn in the places [`names_of`] named.
    /// `None`: it needs none.
    pub async fn enter(
        &self,
        command: &ComputerCommand,
        names: &BTreeSet<String>,
    ) -> Result<Option<SequenceLease>, ComputerUseError> {
        self.enter_within(command, names, TURN_WAIT).await
    }

    async fn enter_within(
        &self,
        command: &ComputerCommand,
        names: &BTreeSet<String>,
        wait: Duration,
    ) -> Result<Option<SequenceLease>, ComputerUseError> {
        let mut lease = SequenceLease::default();
        let mut longest = Duration::ZERO;
        let mut note = |lease: &mut SequenceLease, target: &str, since: std::time::Instant| {
            let took = since.elapsed();
            if took > longest {
                longest = took;
                lease.waited_for = Some(target.to_owned());
            }
            lease.waited += took;
        };
        if is_sequence(command.method) {
            // Taken in one order, so two sequences cannot hold each other up.
            for target in names {
                let since = std::time::Instant::now();
                let held = tokio::time::timeout(wait, self.gate(target).write_owned())
                    .await
                    .map_err(|_| busy(target))?;
                note(&mut lease, target, since);
                lease._exclusive.push(held);
            }
            return Ok(Some(lease));
        }
        let Some(target) = names.iter().next() else {
            return Ok(None);
        };
        let since = std::time::Instant::now();
        if reads_then_presses(command) {
            let alone = tokio::time::timeout(wait, self.gate(target).write_owned())
                .await
                .map_err(|_| busy(target))?;
            note(&mut lease, target, since);
            lease._exclusive.push(alone);
        } else {
            let shared = tokio::time::timeout(wait, self.gate(target).read_owned())
                .await
                .map_err(|_| busy(target))?;
            note(&mut lease, target, since);
            lease._shared = Some(shared);
        }
        Ok(Some(lease))
    }
}

/// An answer that waited for its turn says so: how long, and behind which
/// place, under `queue` — only a JSON answer that succeeded, and only a wait a
/// person would notice ([`QUEUE_NOTE_FROM`]).
#[must_use]
pub fn noting_the_wait(
    answer: zerocode_hookd::TeamAnswer,
    waited: Duration,
    behind: Option<&str>,
) -> zerocode_hookd::TeamAnswer {
    if waited < QUEUE_NOTE_FROM || answer.exit_code != 0 {
        return answer;
    }
    let Ok(mut said) = serde_json::from_str::<Value>(answer.stdout.trim_end()) else {
        return answer;
    };
    if said.get("ok") != Some(&Value::Bool(true)) || !said.is_object() {
        return answer;
    }
    said["queue"] = json!({
        "waitedMs": u64::try_from(waited.as_millis()).unwrap_or(u64::MAX),
        "behind": behind,
    });
    zerocode_hookd::TeamAnswer {
        stdout: format!("{said}\n"),
        ..answer
    }
}

const fn is_sequence(method: ComputerMethod) -> bool {
    matches!(
        method,
        ComputerMethod::Batch | ComputerMethod::Parallel | ComputerMethod::RecipeRun
    )
}

/// Whether a request waits for a sequence at all. Looks and the stop never do.
#[must_use]
pub const fn takes_a_turn(method: ComputerMethod) -> bool {
    is_sequence(method) || method.acts()
}

fn busy(target: &str) -> ComputerUseError {
    ComputerUseError::new(
        zerocode_core::computer_use_protocol::error_code::SEQUENCE_BUSY,
        format!(
            "another agent's sequence has held {target} for {} s; nothing was done. Try again when it ends, or work elsewhere",
            TURN_WAIT.as_secs()
        ),
    )
}

/// The places a request acts in. It asks the helper what each app is called and
/// may read a saved recipe, so it runs off the async loop.
#[must_use]
pub fn names_of(command: &ComputerCommand, root: &Path) -> BTreeSet<String> {
    named_by(command, &the_helpers_name, &|recipe| {
        apps_a_recipe_names(recipe, root)
    })
}

fn named_by(
    command: &ComputerCommand,
    name: &dyn Fn(&Value) -> String,
    recipe_apps: &dyn Fn(&ComputerCommand) -> Vec<String>,
) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    let steps = match command.method {
        ComputerMethod::Batch => batch_steps(command),
        ComputerMethod::Parallel => parallel_steps(command).into_iter().flatten().collect(),
        ComputerMethod::RecipeRun => {
            // A recipe may use the pointer between its apps, so it holds the desk too.
            names.insert(name(&command.params));
            names.insert(DESKTOP.to_owned());
            for app in recipe_apps(command) {
                names.insert(name(&json!({ "app": app })));
            }
            return names;
        }
        _ => {
            names.insert(name(&command.params));
            return names;
        }
    };
    for step in steps {
        if let Ok(step) = parse_command(&step)
            && step.method.acts()
        {
            names.insert(name(&step.params));
        }
    }
    names
}

/// The app a request names, by its own word or by the marked look it presses in.
fn named_app(params: &Value) -> Option<String> {
    params
        .get("app")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| {
            params
                .get("look")
                .and_then(Value::as_str)
                .and_then(super::marks::app_of_look)
        })
}

/// The name as it was written, for when the helper cannot say.
fn written_name(params: &Value) -> String {
    named_app(params).map_or_else(|| DESKTOP.to_owned(), |app| app.trim().to_lowercase())
}

/// The one name an app goes by however it was spelled (its name, its bundle's or
/// `pid:`): the lane the helper runs it in. A window names its owner.
#[cfg(all(target_os = "macos", not(test)))]
fn the_helpers_name(params: &Value) -> String {
    use super::session::ProviderSession as _;
    let asked = named_app(params)
        .map(|app| json!({ "app": app }))
        .or_else(|| {
            params
                .get("windowId")
                .map(|window| json!({ "windowId": window }))
        });
    let Some(asked) = asked else {
        return DESKTOP.to_owned();
    };
    super::standing_session()
        .ok()
        .and_then(|session| session.request("appKey", asked).ok())
        .and_then(|said| said.get("key")?.as_str().map(str::to_owned))
        .unwrap_or_else(|| written_name(params))
}

#[cfg(not(all(target_os = "macos", not(test))))]
fn the_helpers_name(params: &Value) -> String {
    written_name(params)
}

/// Every app a saved recipe's desktop steps name, its placeholders filled.
fn apps_a_recipe_names(command: &ComputerCommand, root: &Path) -> Vec<String> {
    let name = command
        .params
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let Ok((_, text)) = super::recipes::show(root, name) else {
        return Vec::new();
    };
    let values = command
        .params
        .get("recipeParams")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    recipe_lines(&text)
        .unwrap_or_default()
        .into_iter()
        .filter(|line| line.tool == RecipeTool::Computer)
        .filter_map(|line| {
            let filled = recipe_fill(&line.argv, &values).unwrap_or(line.argv);
            let at = filled.iter().position(|word| word == "--app")?;
            filled.get(at + 1).cloned()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn command(method: ComputerMethod, params: Value) -> ComputerCommand {
        ComputerCommand {
            method,
            params,
            json: true,
        }
    }

    /// What a request holds, with a helper that knows every app by its lowercase word.
    fn names(command: &ComputerCommand) -> BTreeSet<String> {
        named_by(command, &written_name, &|_| Vec::new())
    }

    async fn turn(
        gate: &SequenceGate,
        command: &ComputerCommand,
    ) -> Result<Option<SequenceLease>, ComputerUseError> {
        let names = if takes_a_turn(command.method) {
            names(command)
        } else {
            BTreeSet::new()
        };
        gate.enter_within(command, &names, SHORT).await
    }

    fn batch(steps: &[&[&str]]) -> ComputerCommand {
        let lines: Vec<Vec<String>> = steps
            .iter()
            .map(|step| step.iter().map(|word| (*word).to_owned()).collect())
            .collect();
        let argv: Vec<String> = ["batch", "--commands", &json!(lines).to_string()]
            .iter()
            .map(|word| (*word).to_owned())
            .collect();
        parse_command(&argv).expect("a batch")
    }

    const SHORT: Duration = Duration::from_millis(20);

    /// A press by the window's words reads and then presses, so it holds its
    /// app alone: another agent's keys into that app wait until it pressed,
    /// and it waits for keys already going in. Plain presses still share, and
    /// another app is never held up.
    #[tokio::test]
    async fn a_press_by_the_windows_words_holds_its_app_alone_while_it_reads_and_presses() {
        let gate = SequenceGate::default();
        let by_words = command(
            ComputerMethod::Click,
            json!({"app": "iPhone Mirroring", "ocr": true, "text": "Weather"}),
        );
        let keys = command(
            ComputerMethod::TypeText,
            json!({"app": "iPhone Mirroring", "text": "seoul"}),
        );
        let pressing = turn(&gate, &by_words).await.unwrap();
        assert_eq!(
            turn(&gate, &keys).await.unwrap_err().code,
            zerocode_core::computer_use_protocol::error_code::SEQUENCE_BUSY
        );
        let elsewhere = command(
            ComputerMethod::TypeText,
            json!({"app": "Notes", "text": "x"}),
        );
        assert!(turn(&gate, &elsewhere).await.is_ok());
        drop(pressing);
        let typing = turn(&gate, &keys).await.unwrap();
        assert_eq!(
            turn(&gate, &by_words).await.unwrap_err().code,
            zerocode_core::computer_use_protocol::error_code::SEQUENCE_BUSY,
            "it waits for keys already going in"
        );
        drop(typing);
        let plain = command(
            ComputerMethod::Click,
            json!({"app": "Notes", "x": 1, "y": 1}),
        );
        let _first = turn(&gate, &plain).await.unwrap();
        assert!(turn(&gate, &plain).await.is_ok(), "plain presses share");
    }

    /// An answer that stood behind another agent's work says how long and
    /// behind what; a quick turn, a refusal or a plain-text answer is left as it
    /// was.
    #[test]
    fn an_answer_that_waited_for_its_turn_says_how_long_and_behind_what() {
        let answer = |stdout: &str, exit_code| zerocode_hookd::TeamAnswer {
            stdout: stdout.to_owned(),
            stderr: String::new(),
            exit_code,
        };
        let waited = Duration::from_millis(1_250);
        let noted = noting_the_wait(
            answer("{\"ok\":true,\"result\":{}}\n", 0),
            waited,
            Some("pid:42"),
        );
        let said: Value = serde_json::from_str(noted.stdout.trim()).unwrap();
        assert_eq!(said["queue"]["waitedMs"], 1_250);
        assert_eq!(said["queue"]["behind"], "pid:42");
        assert!(noted.stdout.ends_with('\n'));

        let quick = Duration::from_millis(3);
        for (stdout, code, took) in [
            ("{\"ok\":true,\"result\":{}}\n", 0, quick),
            ("{\"ok\":false,\"error\":{}}\n", 1, waited),
            ("Done.\n", 0, waited),
        ] {
            assert_eq!(
                noting_the_wait(answer(stdout, code), took, Some("pid:42")).stdout,
                stdout
            );
        }
    }

    #[tokio::test]
    async fn a_sequence_holds_up_nothing_in_another_app() {
        let gate = SequenceGate::default();
        let _owner = turn(
            &gate,
            &batch(&[&["click", "--app", "Notes", "--x", "1", "--y", "1"]]),
        )
        .await
        .unwrap();
        let other = command(ComputerMethod::Click, json!({"app": "KakaoTalk"}));
        assert!(turn(&gate, &other).await.is_ok());
    }

    #[tokio::test]
    async fn a_sequence_holds_up_acts_in_its_own_app_until_it_ends() {
        let gate = SequenceGate::default();
        let owner = turn(
            &gate,
            &batch(&[&["click", "--app", "Notes", "--x", "1", "--y", "1"]]),
        )
        .await
        .unwrap();
        let same = command(ComputerMethod::Click, json!({"app": "notes"}));
        assert_eq!(
            turn(&gate, &same).await.unwrap_err().code,
            zerocode_core::computer_use_protocol::error_code::SEQUENCE_BUSY
        );
        drop(owner);
        assert!(turn(&gate, &same).await.is_ok());
    }

    #[tokio::test]
    async fn looks_sensors_and_the_stop_never_wait() {
        let gate = SequenceGate::default();
        let _owner = turn(
            &gate,
            &batch(&[&["click", "--app", "Notes", "--x", "1", "--y", "1"]]),
        )
        .await
        .unwrap();
        for method in [
            ComputerMethod::GetAppState,
            ComputerMethod::Observe,
            ComputerMethod::Stop,
            ComputerMethod::Status,
            ComputerMethod::Watch,
            ComputerMethod::SoundWait,
            ComputerMethod::SoundRead,
            ComputerMethod::Wait,
            ComputerMethod::WaitFor,
        ] {
            let look = command(method, json!({"app": "Notes"}));
            assert!(
                turn(&gate, &look).await.unwrap().is_none(),
                "{method:?} takes no turn"
            );
        }
    }

    #[tokio::test]
    async fn two_lone_acts_in_one_app_do_not_wait_for_each_other() {
        let gate = SequenceGate::default();
        let first = command(ComputerMethod::Click, json!({"app": "Notes"}));
        let _one = turn(&gate, &first).await.unwrap();
        assert!(turn(&gate, &first).await.is_ok());
    }

    #[tokio::test]
    async fn a_desktop_sequence_holds_up_other_desktop_acts_only() {
        let gate = SequenceGate::default();
        let _owner = turn(&gate, &batch(&[&["mouse-click", "--x", "1", "--y", "1"]]))
            .await
            .unwrap();
        let desktop = command(ComputerMethod::MouseClick, json!({"x": 1, "y": 1}));
        assert!(turn(&gate, &desktop).await.is_err());
        let scoped = command(ComputerMethod::Click, json!({"app": "Notes"}));
        assert!(turn(&gate, &scoped).await.is_ok());
    }

    #[tokio::test]
    async fn two_spellings_of_one_app_are_one_place() {
        let one_name = |params: &Value| -> String {
            match params["app"].as_str() {
                Some("Notes" | "com.apple.Notes" | "pid:42") => "pid:42".to_owned(),
                _ => written_name(params),
            }
        };
        let none = |_: &ComputerCommand| Vec::new();
        let gate = SequenceGate::default();
        let owner = batch(&[&["click", "--app", "Notes", "--x", "1", "--y", "1"]]);
        let _owner = gate
            .enter_within(&owner, &named_by(&owner, &one_name, &none), SHORT)
            .await
            .unwrap();
        for spelling in ["com.apple.Notes", "pid:42"] {
            let same = command(ComputerMethod::Click, json!({ "app": spelling }));
            let names = named_by(&same, &one_name, &none);
            assert_eq!(
                gate.enter_within(&same, &names, SHORT)
                    .await
                    .unwrap_err()
                    .code,
                zerocode_core::computer_use_protocol::error_code::SEQUENCE_BUSY,
                "{spelling}"
            );
        }
    }

    #[test]
    fn a_recipe_holds_every_app_its_steps_name() {
        let recipe = command(ComputerMethod::RecipeRun, json!({ "name": "errand" }));
        let held = named_by(&recipe, &written_name, &|_| {
            vec!["Notes".to_owned(), "KakaoTalk".to_owned()]
        });
        assert_eq!(
            held.into_iter().collect::<Vec<_>>(),
            ["desktop", "kakaotalk", "notes"]
        );
    }
}
