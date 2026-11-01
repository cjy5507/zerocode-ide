//! A batch or a recipe owns the apps it walks, and only those.
//!
//! A sequence holding the whole window made every other agent wait or fail while
//! it ran, whatever app they worked in. Numbers now outlive a look and stand for
//! one element, so another app's act cannot disturb a sequence: only acts in the
//! same app are held back. A request waits its turn instead of being refused, up
//! to a bound. Looks, passive sensors and the emergency stop never wait.

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::Value;
use tokio::sync::{OwnedRwLockReadGuard, OwnedRwLockWriteGuard, RwLock};
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
}

impl SequenceGate {
    fn gate(&self, target: &str) -> Arc<RwLock<()>> {
        let mut gates = self
            .gates
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Arc::clone(gates.entry(target.to_owned()).or_default())
    }

    /// Wait for the request's turn. `None`: it needs none.
    pub async fn enter(
        &self,
        command: &ComputerCommand,
    ) -> Result<Option<SequenceLease>, ComputerUseError> {
        self.enter_within(command, TURN_WAIT).await
    }

    async fn enter_within(
        &self,
        command: &ComputerCommand,
        wait: Duration,
    ) -> Result<Option<SequenceLease>, ComputerUseError> {
        if matches!(
            command.method,
            ComputerMethod::Batch | ComputerMethod::Parallel | ComputerMethod::RecipeRun
        ) {
            // Taken in one order, so two sequences cannot hold each other up.
            let mut exclusive = Vec::new();
            for target in sequence_targets(command) {
                let held = tokio::time::timeout(wait, self.gate(&target).write_owned())
                    .await
                    .map_err(|_| busy(&target))?;
                exclusive.push(held);
            }
            return Ok(Some(SequenceLease {
                _shared: None,
                _exclusive: exclusive,
            }));
        }
        if !(command.method.acts() || command.method == ComputerMethod::Walk) {
            return Ok(None);
        }
        let target = lone_target(&command.params);
        let shared = tokio::time::timeout(wait, self.gate(&target).read_owned())
            .await
            .map_err(|_| busy(&target))?;
        Ok(Some(SequenceLease {
            _shared: Some(shared),
            _exclusive: Vec::new(),
        }))
    }
}

fn busy(target: &str) -> ComputerUseError {
    ComputerUseError::new(
        zerocode_core::computer_use_protocol::error_code::SEQUENCE_BUSY,
        format!(
            "another agent's sequence has held {target} for {} s; nothing was done. Try again when it ends, or work in another app",
            TURN_WAIT.as_secs()
        ),
    )
}

/// The app a request acts in, or the shared desktop when it names none.
fn lone_target(params: &Value) -> String {
    params
        .get("app")
        .and_then(Value::as_str)
        .map_or_else(|| DESKTOP.to_owned(), |app| app.trim().to_lowercase())
}

/// Every app a sequence walks. A recipe is read when it runs, so all of it is
/// held to the shared desktop and the app it names, if any.
fn sequence_targets(command: &ComputerCommand) -> BTreeSet<String> {
    let mut targets = BTreeSet::new();
    let steps = match command.method {
        ComputerMethod::Batch => batch_steps(command),
        ComputerMethod::Parallel => parallel_steps(command).into_iter().flatten().collect(),
        _ => Vec::new(),
    };
    if matches!(
        command.method,
        ComputerMethod::Batch | ComputerMethod::Parallel
    ) {
        for step in steps {
            if let Ok(step) = parse_command(&step)
                && step.method.acts()
            {
                targets.insert(lone_target(&step.params));
            }
        }
    } else {
        targets.insert(lone_target(&command.params));
        targets.insert(DESKTOP.to_owned());
    }
    targets
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

    #[tokio::test]
    async fn a_sequence_holds_up_nothing_in_another_app() {
        let gate = SequenceGate::default();
        let _owner = gate
            .enter_within(
                &batch(&[&["click", "--app", "Notes", "--x", "1", "--y", "1"]]),
                SHORT,
            )
            .await
            .unwrap();
        let other = command(ComputerMethod::Click, json!({"app": "KakaoTalk"}));
        assert!(gate.enter_within(&other, SHORT).await.is_ok());
    }

    #[tokio::test]
    async fn a_sequence_holds_up_acts_in_its_own_app_until_it_ends() {
        let gate = SequenceGate::default();
        let owner = gate
            .enter_within(
                &batch(&[&["click", "--app", "Notes", "--x", "1", "--y", "1"]]),
                SHORT,
            )
            .await
            .unwrap();
        let same = command(ComputerMethod::Click, json!({"app": "notes"}));
        assert_eq!(
            gate.enter_within(&same, SHORT).await.unwrap_err().code,
            zerocode_core::computer_use_protocol::error_code::SEQUENCE_BUSY
        );
        drop(owner);
        assert!(gate.enter_within(&same, SHORT).await.is_ok());
    }

    #[tokio::test]
    async fn looks_sensors_and_the_stop_never_wait() {
        let gate = SequenceGate::default();
        let _owner = gate
            .enter_within(
                &batch(&[&["click", "--app", "Notes", "--x", "1", "--y", "1"]]),
                SHORT,
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
                gate.enter_within(&look, SHORT).await.unwrap().is_none(),
                "{method:?} takes no turn"
            );
        }
    }

    #[tokio::test]
    async fn two_lone_acts_in_one_app_do_not_wait_for_each_other() {
        let gate = SequenceGate::default();
        let first = command(ComputerMethod::Click, json!({"app": "Notes"}));
        let _one = gate.enter_within(&first, SHORT).await.unwrap();
        assert!(gate.enter_within(&first, SHORT).await.is_ok());
    }

    #[tokio::test]
    async fn a_desktop_sequence_holds_up_other_desktop_acts_only() {
        let gate = SequenceGate::default();
        let _owner = gate
            .enter_within(&batch(&[&["mouse-click", "--x", "1", "--y", "1"]]), SHORT)
            .await
            .unwrap();
        let desktop = command(ComputerMethod::MouseClick, json!({"x": 1, "y": 1}));
        assert!(gate.enter_within(&desktop, SHORT).await.is_err());
        let scoped = command(ComputerMethod::Click, json!({"app": "Notes"}));
        assert!(gate.enter_within(&scoped, SHORT).await.is_ok());
    }
}
