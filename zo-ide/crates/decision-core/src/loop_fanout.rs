//! Fold multiple independent verifier "lens" verdicts into one goal-facing
//! accept/reject signal. Used by the opt-in multi-lens verification fan-out: N
//! adversarial verifiers each judge a change from a different angle, and this
//! pure helper combines them under a configurable consensus policy.

/// One lens's verdict on a change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LensVerdict {
    Accept,
    Reject,
    /// The lens produced no usable verdict (timed out, errored, unparseable).
    Abstain,
}

/// How to combine lens verdicts into one signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsensusPolicy {
    /// Reject if ANY lens rejects (the conservative default): a single credible
    /// objection blocks acceptance; accept only if ≥1 lens accepts and none
    /// reject.
    AnyReject,
    /// Simple majority of non-abstaining lenses.
    Majority,
}

/// Fold lens verdicts into a goal-facing `Option<bool>` matching the
/// `summary.deep_verification` channel: `Some(true)` accept, `Some(false)`
/// reject, `None` no usable signal (all abstained / empty).
#[must_use]
pub fn fold_lens_verdicts(verdicts: &[LensVerdict], policy: ConsensusPolicy) -> Option<bool> {
    let accepts = verdicts
        .iter()
        .filter(|verdict| **verdict == LensVerdict::Accept)
        .count();
    let rejects = verdicts
        .iter()
        .filter(|verdict| **verdict == LensVerdict::Reject)
        .count();
    if accepts + rejects == 0 {
        return None; // all abstained / empty — no usable signal
    }
    let accepted = match policy {
        ConsensusPolicy::AnyReject => rejects == 0,
        ConsensusPolicy::Majority => accepts > rejects,
    };
    Some(accepted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn any_reject_blocks_on_a_single_rejection() {
        let verdicts = [LensVerdict::Accept, LensVerdict::Accept, LensVerdict::Reject];
        assert_eq!(
            fold_lens_verdicts(&verdicts, ConsensusPolicy::AnyReject),
            Some(false)
        );
    }

    #[test]
    fn any_reject_accepts_when_none_reject() {
        let verdicts = [LensVerdict::Accept, LensVerdict::Abstain, LensVerdict::Accept];
        assert_eq!(
            fold_lens_verdicts(&verdicts, ConsensusPolicy::AnyReject),
            Some(true)
        );
    }

    #[test]
    fn all_abstain_or_empty_is_no_signal() {
        assert_eq!(
            fold_lens_verdicts(
                &[LensVerdict::Abstain, LensVerdict::Abstain],
                ConsensusPolicy::AnyReject
            ),
            None
        );
        assert_eq!(fold_lens_verdicts(&[], ConsensusPolicy::AnyReject), None);
    }

    #[test]
    fn majority_needs_more_accepts_than_rejects() {
        assert_eq!(
            fold_lens_verdicts(
                &[LensVerdict::Accept, LensVerdict::Accept, LensVerdict::Reject],
                ConsensusPolicy::Majority
            ),
            Some(true)
        );
        assert_eq!(
            fold_lens_verdicts(
                &[LensVerdict::Accept, LensVerdict::Reject],
                ConsensusPolicy::Majority
            ),
            Some(false),
            "tie is not a majority accept"
        );
    }
}
