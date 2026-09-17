//! Durable PR facts and pure reactions. Design adapted from AO's
//! docs/scm-observer.md and backend/internal/lifecycle/reactions.go (Apache-2.0).

use crate::checks::{CheckRun, Limits, checks_mail_line};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FactKind {
    Ci,
    Reviews,
    Mergeable,
    State,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrState {
    Open,
    Merged,
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mergeability {
    Conflicting,
    Blocked,
    Mergeable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ci {
    pub head: String,
    pub checks: Vec<CheckRun>,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewItem {
    pub id: String,
    pub author: String,
    pub body: String,
    pub path: Option<String>,
    pub line: Option<u64>,
    pub state: String,
    pub resolved: bool,
}

/// None means not fetched (including a failed fetch), never an empty answer.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Observation {
    pub ci: Option<Ci>,
    pub reviews: Option<Vec<ReviewItem>>,
    pub mergeable: Option<Mergeability>,
    pub stacked: Option<bool>,
    pub state: Option<PrState>,
}

impl Observation {
    pub fn merge(&mut self, fetched: &Self) {
        if let Some(ci) = &fetched.ci {
            self.ci = Some(ci.clone());
        }
        if let Some(reviews) = &fetched.reviews {
            self.reviews = Some(reviews.clone());
        }
        if let Some(state) = fetched.state {
            self.state = Some(state);
        }
        // A conflict without a successful stack lookup is not a judgment.
        if let Some(mergeable) = fetched.mergeable {
            self.mergeable = Some(mergeable);
            self.stacked = fetched.stacked;
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Notice {
    pub opened_at: i64,
    pub resolved_at: Option<i64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    pub facts: Observation,
    pub hashes: BTreeMap<FactKind, String>,
    pub revisions: BTreeMap<FactKind, u64>,
    pub sent_review_ids: BTreeSet<String>,
    pub ci_attempts: BTreeMap<String, u64>,
    /// The failure set actually delivered, separate from the complete CI fact hash.
    #[serde(default)]
    pub ci_nudged_hash: Option<String>,
    pub notices: BTreeMap<FactKind, Notice>,
    pub stopped: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reaction {
    pub kind: FactKind,
    pub hash: String,
    pub body: Option<String>,
    pub review_ids: Vec<String>,
    pub ci_head: Option<String>,
    pub ci_signature: Option<String>,
    pub active: bool,
    pub terminal: bool,
}

impl Record {
    /// Call only after durable mail acceptance (or a successfully consumed silent resolution).
    pub fn acknowledge(&mut self, reaction: &Reaction, now_ms: i64) {
        self.hashes.insert(reaction.kind, reaction.hash.clone());
        *self.revisions.entry(reaction.kind).or_default() += 1;
        self.sent_review_ids
            .extend(reaction.review_ids.iter().cloned());
        if let Some(head) = &reaction.ci_head {
            *self.ci_attempts.entry(head.clone()).or_default() += 1;
        }
        if reaction.kind == FactKind::Ci && (!reaction.active || reaction.body.is_some()) {
            self.ci_nudged_hash = reaction.ci_signature.clone();
        }
        if reaction.terminal {
            self.stopped = true;
            for notice in self.notices.values_mut() {
                notice.resolved_at.get_or_insert(now_ms);
            }
        } else if reaction.active {
            let notice = self.notices.entry(reaction.kind).or_insert(Notice {
                opened_at: now_ms,
                resolved_at: None,
            });
            if notice.resolved_at.take().is_some() {
                notice.opened_at = now_ms;
            }
        } else if let Some(notice) = self.notices.get_mut(&reaction.kind) {
            notice.resolved_at.get_or_insert(now_ms);
        }
    }

    /// The revision distinguishes a rearmed condition from its earlier occurrence.
    pub fn delivery_key(&self, subject: &str, reaction: &Reaction) -> String {
        // A comment's ID is its receipt even if another comment gets ACKed
        // first after restart. Whole-review hashes and revisions can both move.
        if !reaction.review_ids.is_empty() {
            return format!(
                "scm:{}",
                semantic_hash(&(subject, reaction.kind, &reaction.review_ids))
            );
        }
        let fact = reaction.ci_signature.as_ref().unwrap_or(&reaction.hash);
        format!(
            "scm:{}",
            semantic_hash(&(
                subject,
                reaction.kind,
                self.revisions
                    .get(&reaction.kind)
                    .copied()
                    .unwrap_or_default(),
                fact
            ))
        )
    }
}

pub fn semantic_hash(value: &impl Serialize) -> String {
    // These types have no fallible map keys or custom serializers.
    let bytes = serde_json::to_vec(value).unwrap_or_default();
    format!("{:x}", Sha256::digest(bytes))
}

pub fn ci_hash(ci: &Ci) -> String {
    hash_checks(&ci.head, ci.checks.iter())
}

fn hash_checks<'a>(head: &str, checks: impl Iterator<Item = &'a CheckRun>) -> String {
    let mut rows: Vec<_> = checks
        .map(|check| {
            (
                &check.name,
                format!("{:?}", check.status),
                format!("{:?}", check.effective_conclusion()),
            )
        })
        .collect();
    rows.sort();
    semantic_hash(&(head, rows))
}

fn reaction(kind: FactKind, hash: String) -> Reaction {
    Reaction {
        kind,
        hash,
        body: None,
        review_ids: Vec::new(),
        ci_head: None,
        ci_signature: None,
        active: false,
        terminal: false,
    }
}

/// Pure reactions over independent successful facts. API failures never enter this function.
pub fn react(previous: &Record, observed: &Observation, limits: &Limits) -> Vec<Reaction> {
    if previous.stopped {
        return Vec::new();
    }
    let mut out = Vec::new();
    if let Some(state) = observed.state {
        let mut r = reaction(FactKind::State, semantic_hash(&state));
        if previous.hashes.get(&r.kind) != Some(&r.hash) {
            if state != PrState::Open {
                r.terminal = true;
                r.body = Some(
                    match state {
                        PrState::Merged => "PR merged.",
                        _ => "PR closed without merging.",
                    }
                    .into(),
                );
                // A terminal PR cannot acquire new actionable notices in this batch.
                return vec![r];
            }
            out.push(r);
        }
    }
    if let Some(ci) = &observed.ci {
        let mut r = reaction(FactKind::Ci, ci_hash(ci));
        if previous.hashes.get(&r.kind) != Some(&r.hash) {
            r.active = ci
                .checks
                .iter()
                .any(|c| c.effective_conclusion().reads_as_failed());
            if r.active {
                r.ci_signature = Some(hash_checks(
                    &ci.head,
                    ci.checks
                        .iter()
                        .filter(|check| check.effective_conclusion().reads_as_failed()),
                ));
            }
            if r.active
                && previous.ci_nudged_hash != r.ci_signature
                && previous
                    .ci_attempts
                    .get(&ci.head)
                    .copied()
                    .unwrap_or_default()
                    < limits.head_nudges_max
            {
                r.body = Some(crate::untrusted::fence(
                    "GitHub PR",
                    &format!("{}\n{}", checks_mail_line(&ci.checks), ci.detail),
                    limits.mail_bytes_max,
                ));
                r.ci_head = Some(ci.head.clone());
            }
            out.push(r);
        }
    }
    if let Some(reviews) = &observed.reviews {
        let mut rows: Vec<_> = reviews.iter().collect();
        rows.sort_by(|a, b| a.id.cmp(&b.id));
        let mut r = reaction(FactKind::Reviews, semantic_hash(&rows));
        let unsent: Vec<_> = rows
            .iter()
            .filter(|item| {
                !item.resolved
                    && !previous.sent_review_ids.contains(&item.id)
                    && item.state != "dismissed"
            })
            .collect();
        if previous.hashes.get(&r.kind) != Some(&r.hash) || !unsent.is_empty() {
            // One bounded item per receipt: never acknowledge comments whose bodies were omitted.
            if let Some(item) = unsent.first() {
                let location = match (&item.path, item.line) {
                    (Some(path), Some(line)) => format!("{path}:{line}"),
                    (Some(path), None) => path.clone(),
                    _ => "general".into(),
                };
                r.body = Some(crate::untrusted::fence(
                    "GitHub PR",
                    &format!(
                        "Review {} by @{} ({location})\n{}",
                        item.state, item.author, item.body
                    ),
                    limits.mail_bytes_max,
                ));
                r.review_ids.push(item.id.clone());
                // Delivery keys must distinguish successive comments in an unchanged review snapshot.
                r.hash = semantic_hash(&(&r.hash, &item.id));
            }
            out.push(r);
        }
    }
    if let Some(mergeable) = observed.mergeable {
        let stacked = match (mergeable, observed.stacked) {
            (Mergeability::Conflicting, None) => return out,
            (_, value) => value.unwrap_or(false),
        };
        let mut r = reaction(FactKind::Mergeable, semantic_hash(&(mergeable, stacked)));
        if previous.hashes.get(&r.kind) != Some(&r.hash) {
            r.active = mergeable == Mergeability::Conflicting && !stacked;
            if r.active {
                r.body = Some(
                    "PR has merge conflicts. Update its base and resolve the conflicts.".into(),
                );
            }
            out.push(r);
        }
    }
    out
}

/// Checkout + hosted PR, persisted before any detailed refresh.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Subject {
    pub root: String,
    pub repo: String,
    pub number: u64,
    pub url: String,
    pub head: String,
    pub branch: String,
}
impl Subject {
    pub fn key(&self) -> String {
        semantic_hash(&(&self.root, &self.url))
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Book {
    pub subjects: BTreeMap<String, Tracked>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tracked {
    pub subject: Subject,
    pub record: Record,
}

/// The shell owns I/O. Tests can fail either write boundary or the mail ACK.
pub trait Effects {
    fn save(&mut self, book: &Book) -> Result<(), String>;
    fn mail(&mut self, subject: &Subject, receipt: &str, body: &str) -> Result<(), String>;
}

impl Book {
    pub fn discover(
        &mut self,
        subjects: &[Subject],
        effects: &mut impl Effects,
    ) -> Result<(), String> {
        let mut staged = self.clone();
        for subject in subjects {
            staged
                .subjects
                .entry(subject.key())
                .and_modify(|held| held.subject = subject.clone())
                .or_insert_with(|| Tracked {
                    subject: subject.clone(),
                    record: Record::default(),
                });
        }
        self.commit(staged, effects)
    }

    fn commit(&mut self, staged: Self, effects: &mut impl Effects) -> Result<(), String> {
        if *self != staged {
            effects.save(&staged)?;
            *self = staged;
        }
        Ok(())
    }

    pub fn apply(
        &mut self,
        key: &str,
        observed: &Observation,
        limits: &Limits,
        now_ms: i64,
        effects: &mut impl Effects,
    ) -> Result<(), String> {
        let mut staged = self.clone();
        let tracked = staged
            .subjects
            .get_mut(key)
            .ok_or("subject was not discovered")?;
        tracked.record.facts.merge(observed);
        self.commit(staged, effects)?;
        // Drain bounded review items individually; no omitted body is ACKed.
        for _ in 0..limits.conversation_max.saturating_add(4) {
            let held = self.subjects.get(key).ok_or("subject disappeared")?;
            let reactions = react(&held.record, &held.record.facts, limits);
            if reactions.is_empty() {
                return Ok(());
            }
            let mut failure = None;
            for reaction in reactions {
                let held = self.subjects.get(key).ok_or("subject disappeared")?;
                if let Some(body) = &reaction.body
                    && let Err(error) = effects.mail(
                        &held.subject,
                        &held.record.delivery_key(key, &reaction),
                        body,
                    )
                {
                    failure = Some(error);
                    continue;
                }
                let mut acknowledged = self.clone();
                if let Some(held) = acknowledged.subjects.get_mut(key) {
                    held.record.acknowledge(&reaction, now_ms);
                }
                if let Err(error) = self.commit(acknowledged, effects) {
                    failure = Some(error);
                }
            }
            if let Some(error) = failure {
                return Err(error);
            }
        }
        Err("review delivery remains pending".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::checks::{CheckConclusion, CheckStatus};

    fn red(head: &str) -> Observation {
        Observation {
            ci: Some(Ci {
                head: head.into(),
                checks: vec![CheckRun {
                    name: "ci/test".into(),
                    status: CheckStatus::Completed,
                    conclusion: Some(CheckConclusion::Failure),
                    url: None,
                    check_run_id: None,
                    workflow_run_id: None,
                }],
                detail: "src/lib.rs:3: assertion failed".into(),
            }),
            ..Observation::default()
        }
    }

    fn ci_reaction(record: &Record, observation: &Observation) -> Reaction {
        react(record, observation, &Limits::default())
            .into_iter()
            .find(|r| r.kind == FactKind::Ci)
            .unwrap()
    }

    #[test]
    fn acknowledgement_is_the_only_cursor_and_failed_mail_retries() {
        let mut record = Record::default();
        let observation = red("a");
        let reaction = ci_reaction(&record, &observation);
        assert!(reaction.body.as_ref().unwrap().contains("ci/test"));
        assert_eq!(
            ci_reaction(&record, &observation),
            reaction,
            "mail failure must retry"
        );
        record.acknowledge(&reaction, 20);
        assert!(react(&record, &observation, &Limits::default()).is_empty());
        assert!(ci_reaction(&record, &red("b")).body.is_some());
    }

    #[test]
    fn unfetched_placeholders_are_not_data() {
        let record = Record::default();
        assert!(react(&record, &Observation::default(), &Limits::default()).is_empty());
    }

    #[test]
    fn slower_or_failed_review_fetch_preserves_review_facts() {
        let mut held = Observation {
            reviews: Some(vec![ReviewItem {
                id: "comment:1".into(),
                author: "reviewer".into(),
                body: "fix this".into(),
                path: Some("lib.rs".into()),
                line: Some(4),
                state: "commented".into(),
                resolved: false,
            }]),
            ..Observation::default()
        };
        held.merge(&red("a"));
        assert_eq!(held.reviews.as_ref().unwrap()[0].id, "comment:1");
        let reactions = react(&Record::default(), &held, &Limits::default());
        assert!(reactions.iter().any(|r| r.kind == FactKind::Ci));
        let review = reactions
            .iter()
            .find(|r| r.kind == FactKind::Reviews)
            .unwrap();
        let body = review.body.as_ref().unwrap();
        assert!(body.contains("lib.rs:4"));
        assert!(body.contains(&crate::untrusted::close_marker("GitHub PR")));
    }

    #[test]
    fn conflict_resolution_and_stack_exemption_rearm_the_next_conflict() {
        let mut record = Record::default();
        let mut observation = Observation {
            mergeable: Some(Mergeability::Conflicting),
            stacked: Some(false),
            ..Observation::default()
        };
        let first = react(&record, &observation, &Limits::default()).remove(0);
        record.acknowledge(&first, 1);
        assert!(record.notices[&FactKind::Mergeable].resolved_at.is_none());
        assert!(react(&record, &observation, &Limits::default()).is_empty());
        observation.stacked = Some(true);
        let exempt = react(&record, &observation, &Limits::default()).remove(0);
        assert!(exempt.body.is_none());
        record.acknowledge(&exempt, 2);
        assert_eq!(record.notices[&FactKind::Mergeable].resolved_at, Some(2));
        observation.stacked = Some(false);
        assert!(
            react(&record, &observation, &Limits::default())[0]
                .body
                .is_some()
        );
        observation.mergeable = Some(Mergeability::Mergeable);
        let resolved = react(&record, &observation, &Limits::default()).remove(0);
        record.acknowledge(&resolved, 3);
        observation.mergeable = Some(Mergeability::Conflicting);
        assert!(
            react(&record, &observation, &Limits::default())[0]
                .body
                .is_some()
        );
    }

    #[test]
    fn same_head_has_three_ci_nudges_and_review_ids_are_sent_once() {
        let mut record = Record::default();
        for i in 0..5 {
            let mut observation = red("head");
            observation.ci.as_mut().unwrap().checks[0].name = format!("job-{i}");
            let reaction = ci_reaction(&record, &observation);
            assert_eq!(reaction.body.is_some(), i < 3);
            record.acknowledge(&reaction, i);
        }
        assert!(ci_reaction(&record, &red("new-head")).body.is_some());
    }

    #[test]
    fn terminal_transition_resolves_every_notice_and_stops() {
        let mut record = Record::default();
        record.acknowledge(&ci_reaction(&record, &red("a")), 1);
        let terminal = Observation {
            state: Some(PrState::Merged),
            ..Observation::default()
        };
        let reaction = react(&record, &terminal, &Limits::default()).remove(0);
        record.acknowledge(&reaction, 2);
        assert!(record.stopped);
        assert!(record.notices.values().all(|n| n.resolved_at == Some(2)));
        assert!(react(&record, &terminal, &Limits::default()).is_empty());
    }

    #[test]
    fn check_order_and_transient_ids_do_not_change_the_semantic_hash() {
        let mut observation = red("a");
        let first = ci_reaction(&Record::default(), &observation);
        observation.ci.as_mut().unwrap().checks[0].check_run_id = Some(123);
        assert_eq!(
            first.hash,
            ci_reaction(&Record::default(), &observation).hash
        );
    }
    #[derive(Default)]
    struct Host {
        snapshots: Vec<Book>,
        mail: BTreeSet<String>,
        fail_save: Option<usize>,
        saves: usize,
        fail_mail: bool,
    }
    impl Effects for Host {
        fn save(&mut self, book: &Book) -> Result<(), String> {
            self.saves += 1;
            if self.fail_save == Some(self.saves) {
                return Err("disk".into());
            }
            self.snapshots.push(book.clone());
            Ok(())
        }
        fn mail(&mut self, _: &Subject, key: &str, _: &str) -> Result<(), String> {
            if self.fail_mail {
                return Err("mail".into());
            }
            self.mail.insert(key.into());
            Ok(())
        }
    }
    fn subject(number: u64) -> Subject {
        Subject {
            root: "/wt".into(),
            repo: "o/r".into(),
            number,
            url: format!("https://github.com/o/r/pull/{number}"),
            head: "a".into(),
            branch: "topic".into(),
        }
    }
    #[test]
    fn failed_persistence_never_advances_facts_or_ack_and_replay_is_idempotent() {
        let mut book = Book::default();
        let mut host = Host::default();
        let subject = subject(1);
        let key = subject.key();
        book.discover(&[subject], &mut host).unwrap();
        host.fail_save = Some(2);
        assert!(
            book.apply(&key, &red("a"), &Limits::default(), 1, &mut host)
                .is_err()
        );
        assert!(book.subjects[&key].record.hashes.is_empty());
        assert!(book.subjects[&key].record.facts.ci.is_none());
        assert!(host.mail.is_empty());
        host.fail_save = Some(4); // facts stored, mail accepted, ACK persistence fails
        assert!(
            book.apply(&key, &red("a"), &Limits::default(), 2, &mut host)
                .is_err()
        );
        assert!(book.subjects[&key].record.hashes.is_empty());
        let mut restarted = host.snapshots.last().unwrap().clone();
        restarted
            .apply(&key, &red("a"), &Limits::default(), 3, &mut host)
            .unwrap();
        assert_eq!(host.mail.len(), 1);
        assert!(!restarted.subjects[&key].record.hashes.is_empty());
    }
    #[test]
    fn discovery_persists_every_sibling_baseline_before_refresh() {
        let mut book = Book::default();
        let mut host = Host::default();
        let first = subject(1);
        let second = subject(2);
        book.discover(&[first.clone(), second.clone()], &mut host)
            .unwrap();
        assert_eq!(host.snapshots[0].subjects.len(), 2);
        host.fail_mail = true;
        assert!(
            book.apply(&first.key(), &red("a"), &Limits::default(), 1, &mut host)
                .is_err()
        );
        assert!(book.subjects[&first.key()].record.hashes.is_empty());
        assert!(book.subjects[&second.key()].record.facts.ci.is_none());
        host.fail_mail = false;
        book.apply(&first.key(), &red("a"), &Limits::default(), 2, &mut host)
            .unwrap();
        assert_eq!(host.mail.len(), 1);
    }
    #[test]
    fn a_failed_ci_letter_does_not_prevent_review_acknowledgement() {
        struct Selective {
            keys: Vec<String>,
        }
        impl Effects for Selective {
            fn save(&mut self, _: &Book) -> Result<(), String> {
                Ok(())
            }
            fn mail(&mut self, _: &Subject, key: &str, body: &str) -> Result<(), String> {
                if body.contains("ci/test") {
                    Err("CI delivery failed".into())
                } else {
                    self.keys.push(key.into());
                    Ok(())
                }
            }
        }
        let mut host = Selective { keys: Vec::new() };
        let mut book = Book::default();
        let subject = subject(1);
        let key = subject.key();
        book.discover(&[subject], &mut host).unwrap();
        let mut observation = red("a");
        observation.reviews = Some(vec![ReviewItem {
            id: "comment:1".into(),
            author: "r".into(),
            body: "fix this".into(),
            path: None,
            line: None,
            state: "commented".into(),
            resolved: false,
        }]);
        assert!(
            book.apply(&key, &observation, &Limits::default(), 1, &mut host)
                .is_err()
        );
        assert!(
            book.subjects[&key]
                .record
                .sent_review_ids
                .contains("comment:1")
        );
        assert!(
            !book.subjects[&key]
                .record
                .hashes
                .contains_key(&FactKind::Ci)
        );
    }
    #[test]
    fn unrelated_pending_checks_do_not_repeat_a_failure_nudge() {
        let mut record = Record::default();
        let mut observation = red("a");
        let mut pending = observation.ci.as_ref().unwrap().checks[0].clone();
        pending.name = "slow-job".into();
        pending.status = CheckStatus::Queued;
        pending.conclusion = Some(CheckConclusion::Pending);
        observation.ci.as_mut().unwrap().checks.push(pending);
        record.acknowledge(&ci_reaction(&record, &observation), 1);
        observation.ci.as_mut().unwrap().checks[1].status = CheckStatus::InProgress;
        let moved = ci_reaction(&record, &observation);
        assert!(
            moved.body.is_none(),
            "the same failing job was mailed again when another job started"
        );
        record.acknowledge(&moved, 2);
        assert_eq!(record.ci_attempts["a"], 1);
        observation.ci.as_mut().unwrap().checks[0].conclusion = Some(CheckConclusion::Success);
        record.acknowledge(&ci_reaction(&record, &observation), 3);
        observation.ci.as_mut().unwrap().checks[0].conclusion = Some(CheckConclusion::Failure);
        assert!(
            ci_reaction(&record, &observation).body.is_some(),
            "resolution must rearm an actual recurrence"
        );
    }

    #[test]
    fn lost_ack_replays_ci_even_when_an_unrelated_check_moves() {
        let mut host = Host::default();
        let mut book = Book::default();
        let subject = subject(1);
        let key = subject.key();
        book.discover(&[subject], &mut host).unwrap();
        host.fail_save = Some(3);
        assert!(
            book.apply(&key, &red("a"), &Limits::default(), 1, &mut host)
                .is_err()
        );
        let mut restarted = host.snapshots.last().unwrap().clone();
        let mut next = red("a");
        let mut passed = next.ci.as_ref().unwrap().checks[0].clone();
        passed.name = "other".into();
        passed.conclusion = Some(CheckConclusion::Success);
        next.ci.as_mut().unwrap().checks.push(passed);
        restarted
            .apply(&key, &next, &Limits::default(), 2, &mut host)
            .unwrap();
        assert_eq!(
            host.mail.len(),
            1,
            "a lost ACK must replay the same failure receipt across unrelated CI changes"
        );
    }

    #[test]
    fn lost_review_ack_keeps_its_receipt_when_another_comment_arrives_first() {
        let item = |id: &str| ReviewItem {
            id: id.into(),
            author: "r".into(),
            body: "fix".into(),
            path: None,
            line: None,
            state: "commented".into(),
            resolved: false,
        };
        let mut host = Host::default();
        let mut book = Book::default();
        let subject = subject(1);
        let key = subject.key();
        book.discover(&[subject], &mut host).unwrap();
        host.fail_save = Some(3);
        let first = Observation {
            reviews: Some(vec![item("comment:2")]),
            ..Observation::default()
        };
        assert!(
            book.apply(&key, &first, &Limits::default(), 1, &mut host)
                .is_err()
        );
        let mut restarted = host.snapshots.last().unwrap().clone();
        let next = Observation {
            reviews: Some(vec![item("comment:10"), item("comment:2")]),
            ..Observation::default()
        };
        restarted
            .apply(&key, &next, &Limits::default(), 2, &mut host)
            .unwrap();
        assert_eq!(
            host.mail.len(),
            2,
            "the already accepted comment was filed twice under different cursor revisions"
        );
    }
}
