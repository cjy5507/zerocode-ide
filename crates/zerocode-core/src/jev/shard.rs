//! How a batch of questions is split across requests.
//!
//! One Jev request carries one state and a map of questions over it, and every
//! seat but one asks few enough that the map is the whole batch: recall's cap
//! is twelve notes ([`crate::jev::RECALL_NOTE_CAP`]) and a screen's is twelve
//! controls ([`crate::jev::SCREEN_CANDIDATE_CAP`]), so those seats ask once and
//! never reach this module. The skill seat is the first that can be handed more
//! questions than one request should carry — a machine with a hundred skills
//! installed has a hundred of them — and a list that long has to be cut.
//!
//! It is cut EVENLY rather than into full shards and a remainder, because the
//! shards travel at the same time and the slowest of them is what the caller
//! waits for: 101 questions at a target of 50 is 34/34/33 and not 50/50/1, and
//! the second of those two makes the caller wait out a request twice the size
//! of the one that decides the answer for nothing.
//!
//! It lives here, in the crate both programs read, for the reason the rest of
//! this module's facts do: a second way of counting the same split is a second
//! answer to how many requests a day was billed for.

use std::ops::Range;

/// The shards `total` questions are asked in: none larger than `target`, none
/// more than one question larger than another, and every question in exactly
/// one of them.
///
/// `total` of zero is no shards at all — a seat with nothing to ask sends
/// nothing — and a `target` of zero is read as one, so a caller's arithmetic
/// slip asks a question per request rather than dividing by zero.
#[must_use]
pub fn even_shards(total: usize, target: usize) -> Vec<Range<usize>> {
    if total == 0 {
        return Vec::new();
    }
    let shards = total.div_ceil(target.max(1));
    let base = total / shards;
    // The first `wider` shards carry one question more than the rest, which is
    // what makes the largest and the smallest differ by at most one.
    let wider = total % shards;
    let mut cut = Vec::with_capacity(shards);
    let mut at = 0;
    for shard in 0..shards {
        let len = base + usize::from(shard < wider);
        cut.push(at..at + len);
        at += len;
    }
    cut
}

#[cfg(test)]
mod tests {
    use super::even_shards;

    /// The example the design names: a hundred and one questions at a target
    /// of fifty are three requests of 34, 34 and 33 — not 50, 50 and 1.
    #[test]
    fn a_hundred_and_one_questions_at_fifty_are_three_even_requests() {
        let shards = even_shards(101, 50);
        let sizes: Vec<usize> = shards.iter().map(ExactSizeIterator::len).collect();
        assert_eq!(sizes, vec![34, 34, 33]);
        assert_eq!(shards[0], 0..34);
        assert_eq!(shards[1], 34..68);
        assert_eq!(shards[2], 68..101);
    }

    #[test]
    fn every_question_is_asked_exactly_once_and_no_shard_is_over_the_target() {
        for target in [1_usize, 2, 7, 50] {
            for total in 0..200_usize {
                let shards = even_shards(total, target);
                let mut seen: Vec<usize> = Vec::new();
                for shard in &shards {
                    assert!(shard.len() <= target, "{total} at {target}: {shard:?}");
                    assert!(!shard.is_empty(), "{total} at {target}: an empty request");
                    seen.extend(shard.clone());
                }
                assert_eq!(seen, (0..total).collect::<Vec<_>>(), "{total} at {target}");
            }
        }
    }

    #[test]
    fn the_largest_shard_and_the_smallest_differ_by_at_most_one() {
        for target in [3_usize, 8, 50] {
            for total in 1..200_usize {
                let shards = even_shards(total, target);
                let sizes: Vec<usize> = shards.iter().map(ExactSizeIterator::len).collect();
                let widest = sizes.iter().max().copied().unwrap_or(0);
                let narrowest = sizes.iter().min().copied().unwrap_or(0);
                assert!(widest - narrowest <= 1, "{total} at {target}: {sizes:?}");
            }
        }
    }

    /// A seat whose cap already sits under the target asks once — which is why
    /// recall and the screen seats never reach this module.
    #[test]
    fn a_batch_under_the_target_is_one_request() {
        assert_eq!(even_shards(crate::jev::RECALL_NOTE_CAP, 50), vec![0..12]);
        assert_eq!(
            even_shards(crate::jev::SCREEN_CANDIDATE_CAP, 50),
            vec![0..12]
        );
        assert_eq!(even_shards(50, 50), vec![0..50]);
        assert_eq!(even_shards(0, 50), Vec::<std::ops::Range<usize>>::new());
    }

    /// A slip in the caller's arithmetic asks one question per request rather
    /// than dividing by zero.
    #[test]
    fn a_target_of_nothing_is_read_as_one() {
        assert_eq!(even_shards(3, 0), vec![0..1, 1..2, 2..3]);
    }
}
