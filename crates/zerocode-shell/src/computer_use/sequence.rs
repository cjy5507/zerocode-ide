//! Whole batches/recipes exclude other requests that could disturb a step.
//! Lone requests keep the provider's existing per-call serialization. Passive
//! sensors and the emergency stop never wait for a sequence owner.

use std::sync::Arc;

use tokio::sync::{OwnedRwLockReadGuard, OwnedRwLockWriteGuard, RwLock};
use zerocode_core::computer_use::{ComputerCommand, ComputerMethod, rebuilds_the_tree};

use super::ComputerUseError;

#[derive(Clone, Default)]
pub struct SequenceGate(Arc<RwLock<()>>);

#[derive(Debug)]
pub enum SequenceLease {
    Shared { _guard: OwnedRwLockReadGuard<()> },
    Exclusive { _guard: OwnedRwLockWriteGuard<()> },
}

impl SequenceGate {
    pub fn enter(
        &self,
        command: &ComputerCommand,
    ) -> Result<Option<SequenceLease>, ComputerUseError> {
        let method = command.method;
        if matches!(
            method,
            ComputerMethod::Stop
                | ComputerMethod::Status
                | ComputerMethod::Watch
                | ComputerMethod::SoundWait
                | ComputerMethod::SoundRead
                | ComputerMethod::Wait
        ) || (method == ComputerMethod::WaitFor && !rebuilds_the_tree(command))
        {
            return Ok(None);
        }
        let lease = if matches!(method, ComputerMethod::Batch | ComputerMethod::RecipeRun) {
            self.0
                .clone()
                .try_write_owned()
                .map(|guard| SequenceLease::Exclusive { _guard: guard })
        } else {
            self.0
                .clone()
                .try_read_owned()
                .map(|guard| SequenceLease::Shared { _guard: guard })
        };
        lease.map(Some).map_err(|_| ComputerUseError::new(
            zerocode_core::computer_use_protocol::error_code::SEQUENCE_BUSY,
            "another Computer Use request is in flight; wait for its result, then observe again before starting a sequence or acting",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn command(method: ComputerMethod) -> ComputerCommand {
        ComputerCommand {
            method,
            params: json!({}),
            json: true,
        }
    }

    #[test]
    fn a_batch_excludes_input_cache_and_listener_changes_but_not_passive_sensors_or_stop() {
        let gate = SequenceGate::default();
        let owner = gate.enter(&command(ComputerMethod::Batch)).unwrap();
        let other = gate.clone();
        for method in [
            ComputerMethod::Click,
            ComputerMethod::Observe,
            ComputerMethod::Batch,
            ComputerMethod::Resume,
            ComputerMethod::ListenStart,
            ComputerMethod::ListenStop,
            ComputerMethod::WaitFor,
        ] {
            assert_eq!(
                other.enter(&command(method)).unwrap_err().code,
                zerocode_core::computer_use_protocol::error_code::SEQUENCE_BUSY
            );
        }
        for method in [
            ComputerMethod::Stop,
            ComputerMethod::Status,
            ComputerMethod::Watch,
            ComputerMethod::SoundWait,
            ComputerMethod::SoundRead,
            ComputerMethod::Wait,
        ] {
            assert!(other.enter(&command(method)).unwrap().is_none());
        }
        for params in [json!({"window":"owned"}), json!({"ocr":true})] {
            let mut passive = command(ComputerMethod::WaitFor);
            passive.params = params;
            assert!(other.enter(&passive).unwrap().is_none());
        }
        drop(owner);
        assert!(
            other
                .enter(&command(ComputerMethod::Observe))
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn lone_requests_share_the_existing_provider_queue_and_exclude_new_walks() {
        let gate = SequenceGate::default();
        let first = gate.enter(&command(ComputerMethod::Observe)).unwrap();
        let second = gate.enter(&command(ComputerMethod::Click)).unwrap();
        assert!(gate.enter(&command(ComputerMethod::RecipeRun)).is_err());
        drop(first);
        assert!(gate.enter(&command(ComputerMethod::RecipeRun)).is_err());
        drop(second);
        assert!(gate.enter(&command(ComputerMethod::RecipeRun)).is_ok());
    }

    #[tokio::test]
    async fn cancelling_an_owner_releases_its_sequence_without_resuming_an_input() {
        let gate = SequenceGate::default();
        let owner = gate.enter(&command(ComputerMethod::RecipeRun)).unwrap();
        let pending = tokio::spawn(async move {
            let _owner = owner;
            std::future::pending::<()>().await;
        });
        pending.abort();
        assert!(pending.await.unwrap_err().is_cancelled());
        assert!(
            gate.enter(&command(ComputerMethod::Batch))
                .unwrap()
                .is_some()
        );
    }
}
