use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use super::{Run, UsageSource, reported_sessions};
use crate::ProviderSession;

#[derive(Hash)]
pub(super) struct SessionOwner {
    pub run: String,
    pub dispatch: String,
    pub ended_ms: Option<i64>,
    pub started_ms: i64,
}

#[derive(Default)]
pub struct SessionAttribution {
    owners: HashMap<UsageSource, HashMap<String, Vec<SessionOwner>>>,
    fingerprints: HashMap<String, u64>,
}

impl SessionAttribution {
    #[must_use]
    pub fn new<'run>(runs: impl IntoIterator<Item = &'run Run>) -> Self {
        let runs: Vec<_> = runs.into_iter().collect();
        let mut attribution = Self::default();
        for run in &runs {
            for attempt in &run.dispatches {
                let Some(source) = run
                    .worker(&attempt.worker)
                    .and_then(|worker| UsageSource::of_agent(&worker.agent))
                else {
                    continue;
                };
                for session in reported_sessions(run, attempt) {
                    let owners = attribution
                        .owners
                        .entry(source)
                        .or_default()
                        .entry(session.id.clone())
                        .or_default();
                    if owners
                        .last()
                        .is_some_and(|owner| owner.run == run.id && owner.dispatch == attempt.id)
                    {
                        continue;
                    }
                    owners.push(SessionOwner {
                        run: run.id.clone(),
                        dispatch: attempt.id.clone(),
                        ended_ms: attempt.ended_ms,
                        started_ms: attempt.started_ms,
                    });
                }
            }
        }
        for run in runs {
            let fingerprint = attribution.fingerprint_of(run);
            attribution.fingerprints.insert(run.id.clone(), fingerprint);
        }
        attribution
    }

    pub(super) fn owners(&self, source: UsageSource, session: &ProviderSession) -> &[SessionOwner] {
        self.owners
            .get(&source)
            .and_then(|sessions| sessions.get(session.id.as_str()))
            .map_or(&[], Vec::as_slice)
    }

    #[must_use]
    pub fn generation_fingerprint(&self, run_id: &str) -> Option<u64> {
        self.fingerprints.get(run_id).copied()
    }

    fn fingerprint_of(&self, run: &Run) -> u64 {
        let mut digest = std::hash::DefaultHasher::new();
        super::generation_fingerprint(run).hash(&mut digest);
        for attempt in &run.dispatches {
            let Some(source) = run
                .worker(&attempt.worker)
                .and_then(|worker| UsageSource::of_agent(&worker.agent))
            else {
                continue;
            };
            for session in reported_sessions(run, attempt) {
                self.owners(source, session).hash(&mut digest);
            }
        }
        digest.finish()
    }
}

pub(super) fn covers(owners: &[SessionOwner], bounds: Option<(i64, i64)>) -> bool {
    let Some((first, last)) = bounds else {
        return false;
    };
    if first < 0 || first > last {
        return false;
    }
    let mut ranges: Vec<_> = owners
        .iter()
        .filter_map(|owner| owner.ended_ms.map(|end| (owner.started_ms, end)))
        .collect();
    ranges.sort_unstable();
    let mut covered = None;
    for (start, end) in ranges {
        if start < 0 || end < start {
            return false;
        }
        if covered.is_none() && start <= first && end >= first {
            covered = Some(end);
        } else if let Some(previous) = covered
            && start <= previous
        {
            covered = Some(previous.max(end));
        }
    }
    covered.is_some_and(|end| end >= last)
}
