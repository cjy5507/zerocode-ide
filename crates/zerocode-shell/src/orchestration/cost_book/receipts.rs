use std::hash::{Hash, Hasher};
use std::io::Read;
use std::path::Path;

use serde::{Deserialize, Serialize};
use zerocode_core::orchestration::task_cost::{GenerationCost, UsdReason};

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct Receipt {
    version: u8,
    #[serde(default)]
    revoked: bool,
    run: String,
    task: String,
    generation: u64,
    spend: GenerationCost,
}

pub(super) fn preserve(
    root: &Path,
    run: &str,
    task: &str,
    generation: Option<u64>,
    spend: &mut GenerationCost,
) {
    let Some(generation) = generation else {
        return;
    };
    let mut name = std::hash::DefaultHasher::new();
    (run, task).hash(&mut name);
    let file = root.join(format!("{:016x}.json", name.finish()));
    if crate::durable_file::require_plain_directory_if_present(root).is_err() {
        return;
    }
    if crate::durable_file::ensure_private_directory(root).is_err() {
        return;
    }
    let Ok(lock) = crate::durable_file::private_lock_file(&root.join(".receipts.lock")) else {
        return;
    };
    if lock.try_lock().is_err() {
        return;
    }
    let previous = read(&file).filter(|receipt| {
        receipt.version == 1
            && receipt.run == run
            && receipt.task == task
            && receipt.generation == generation
            && receipt.spend.measured_tokens().is_some()
            && receipt
                .spend
                .usd
                .is_some_and(|usd| usd.is_finite() && usd >= 0.0)
    });
    if spend.measured_tokens().is_some()
        && spend.usd.is_some_and(|usd| usd.is_finite() && usd >= 0.0)
    {
        if previous.as_ref().is_some_and(|receipt| {
            spend.input_tokens < receipt.spend.input_tokens
                || spend.output_tokens < receipt.spend.output_tokens
                || spend.cache_read_tokens < receipt.spend.cache_read_tokens
                || spend.cache_write_tokens < receipt.spend.cache_write_tokens
        }) {
            spend.usd = None;
            spend.usd_reason = Some(UsdReason::InvalidUsage);
            if let Some(mut receipt) = previous {
                receipt.revoked = true;
                if let Ok(bytes) = serde_json::to_vec(&receipt) {
                    let _ = crate::durable_file::replace_bytes(&file, &bytes);
                }
            }
            return;
        }
        let receipt = Receipt {
            version: 1,
            revoked: false,
            run: run.into(),
            task: task.into(),
            generation,
            spend: spend.clone(),
        };
        if previous.as_ref() == Some(&receipt) {
            return;
        }
        if crate::durable_file::ensure_private_directory(root).is_err() {
            return;
        }
        if let Ok(bytes) = serde_json::to_vec(&receipt) {
            let _ = crate::durable_file::replace_bytes(&file, &bytes);
        }
    } else if matches!(
        spend.usd_reason,
        Some(UsdReason::Unlinked | UsdReason::Unscanned)
    ) {
        if let Some(receipt) = previous.filter(|receipt| !receipt.revoked) {
            *spend = receipt.spend;
        }
    } else {
        let _ = crate::durable_file::remove_file(&file);
    }
}

fn read(file: &Path) -> Option<Receipt> {
    let file = crate::durable_file::open_plain_file(file).ok()?;
    if file.metadata().ok()?.len() > 65_536 {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(65_537).read_to_end(&mut bytes).ok()?;
    if bytes.len() > 65_536 {
        return None;
    }
    serde_json::from_slice(&bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn known() -> GenerationCost {
        GenerationCost {
            history_complete: true,
            sessions_known: 1,
            sessions_linked: 1,
            input_tokens: 100,
            output_tokens: 10,
            usd: Some(0.01),
            ..GenerationCost::default()
        }
    }

    #[test]
    fn a_restart_keeps_measured_spend_but_changed_attribution_does_not() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("receipts");
        preserve(&root, "run", "task", Some(1), &mut known());
        let mut missing = GenerationCost {
            usd_reason: Some(UsdReason::Unlinked),
            ..GenerationCost::default()
        };
        preserve(&root, "run", "task", Some(1), &mut missing);
        assert_eq!(missing, known());
        let mut changed = GenerationCost {
            usd_reason: Some(UsdReason::Unlinked),
            ..GenerationCost::default()
        };
        preserve(&root, "run", "task", Some(2), &mut changed);
        assert_eq!(changed.usd, None);
    }

    #[test]
    fn later_out_of_scope_usage_revokes_the_saved_bill() {
        let directory = tempfile::tempdir().unwrap();
        preserve(directory.path(), "run", "task", Some(1), &mut known());
        let mut outside = GenerationCost {
            usd_reason: Some(UsdReason::OutsideAttempt),
            ..GenerationCost::default()
        };
        preserve(directory.path(), "run", "task", Some(1), &mut outside);
        let mut missing = GenerationCost {
            usd_reason: Some(UsdReason::Unlinked),
            ..GenerationCost::default()
        };
        preserve(directory.path(), "run", "task", Some(1), &mut missing);
        assert_eq!(missing.usd, None);
    }

    #[test]
    fn decreasing_counters_remain_revoked_after_another_scan_or_restart() {
        let directory = tempfile::tempdir().unwrap();
        preserve(directory.path(), "run", "task", Some(1), &mut known());
        for _ in 0..2 {
            let mut decreased = known();
            decreased.input_tokens -= 1;
            preserve(directory.path(), "run", "task", Some(1), &mut decreased);
            assert_eq!(decreased.usd_reason, Some(UsdReason::InvalidUsage));
        }
        let mut missing = GenerationCost {
            usd_reason: Some(UsdReason::Unlinked),
            ..GenerationCost::default()
        };
        preserve(directory.path(), "run", "task", Some(1), &mut missing);
        assert_eq!(missing.usd, None);
        preserve(directory.path(), "run", "task", Some(1), &mut known());
        preserve(directory.path(), "run", "task", Some(1), &mut missing);
        assert_eq!(missing, known());
    }
}
