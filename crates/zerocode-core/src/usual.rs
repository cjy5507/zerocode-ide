//! What a recurring thing usually takes, read off its past alone: the median
//! of its last [`SAMPLES`] runs that passed.

use serde::Serialize;

/// An estimate is the median of the last five runs that passed.
pub const SAMPLES: usize = 5;

/// What a running or waiting thing is expected to take: the median of
/// `samples` past runs that passed — or, with none, the cap that bounds it
/// (a thing with no cap says 0).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct Expected {
    pub secs: u32,
    pub samples: u8,
}

/// The middle of `secs` — the mean of the two middles for an even count.
#[must_use]
pub fn median(secs: &[u32]) -> Option<u32> {
    if secs.is_empty() {
        return None;
    }
    let mut sorted = secs.to_vec();
    sorted.sort_unstable();
    let middle = sorted.len() / 2;
    Some(if sorted.len().is_multiple_of(2) {
        ((u64::from(sorted[middle - 1]) + u64::from(sorted[middle])) / 2) as u32
    } else {
        sorted[middle]
    })
}

/// What a thing whose passing runs took `past` (oldest first) is expected to
/// take: the median of the newest [`SAMPLES`] of them.
#[must_use]
pub fn expected_of(past: Option<&[u32]>) -> Expected {
    let all = past.unwrap_or_default();
    let recent = &all[all.len().saturating_sub(SAMPLES)..];
    Expected {
        secs: median(recent).unwrap_or(0),
        samples: recent.len() as u8,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn median_takes_the_middle_and_the_mean_of_two_middles() {
        assert_eq!(median(&[3, 1, 2]), Some(2));
        assert_eq!(median(&[1, 2, 3, 10]), Some(2));
        assert_eq!(median(&[]), None);
    }

    #[test]
    fn expected_reads_only_the_newest_samples() {
        let past = [100, 1, 2, 3, 4, 5];
        assert_eq!(
            expected_of(Some(&past)),
            Expected {
                secs: 3,
                samples: SAMPLES as u8
            }
        );
        assert_eq!(
            expected_of(None),
            Expected {
                secs: 0,
                samples: 0
            }
        );
    }
}
