//! What the session vault read of each transcript, kept so a file that has
//! not moved is not read again.
//!
//! A card is made from the head of one file — up to
//! [`crate::vault::MAX_HEAD_BYTES`] of it, parsed line by line — and a file
//! nobody wrote to since gives the same card. Measured on one machine on
//! 2026-11-11 (t-103659): the panel's walk read some 2,800 heads, about 550 MB, to show
//! a list in which a handful of conversations had changed since the last
//! look — 1.7–2.3 s with the files in memory, 10.6 s without. So what
//! each file gave is kept under the file's stamp
//! ([`crate::file_stamp::FileStamp`]: size, times, identity), and the scan
//! ([`crate::vault::scan_homes`]) opens only a file whose stamp differs.
//!
//! This is the pure half: what is kept, and when it is believed. The window
//! keeps it between two runs in its own data folder — never beside a session
//! store, which is a vendor's ([`crate::vault`]).

use std::collections::{HashMap, HashSet};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::file_stamp::FileStamp;
use crate::vault::VaultSession;

/// What reading one session file gave.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Outcome {
    /// A conversation, as its card says it.
    Card(Box<VaultSession>),
    /// Read whole, and no conversation: a log nobody spoke in.
    NotASession,
    /// Not read. The word is the scan issue's reason.
    Unreadable(String),
}

/// One file's outcome, and the file as it lay when that was read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Entry {
    stamp: FileStamp,
    outcome: Outcome,
}

/// Every file the last scan looked at, by path.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Index {
    entries: HashMap<String, Entry>,
    /// Whether the outcomes were made by another build of the reader. They
    /// are still what the list shows until a scan has looked — a title the
    /// old reader made is better than an empty panel — and no stamp is
    /// believed, so that scan reads every file again.
    #[serde(skip)]
    distrusted: bool,
}

impl Index {
    /// What `path` gave when it last lay as `stamp` says it does now, or
    /// nothing when it has to be read: never read, moved since, or read by a
    /// reader this build does not trust ([`Self::distrust`]).
    #[must_use]
    pub fn kept(&self, path: &Path, stamp: FileStamp) -> Option<&Outcome> {
        if self.distrusted {
            return None;
        }
        let entry = self.entries.get(path.to_str()?)?;
        (entry.stamp == stamp).then_some(&entry.outcome)
    }

    /// Keep what `path` gave as `stamp` describes it. A path that is not
    /// text is not kept: it is read again next time, no more.
    pub fn keep(&mut self, path: &Path, stamp: FileStamp, outcome: Outcome) {
        if let Some(key) = path.to_str() {
            self.entries
                .insert(key.to_string(), Entry { stamp, outcome });
        }
    }

    /// Forget every file a scan did not look at — deleted, archived, or
    /// pushed past the scan's cap by newer ones — and believe the rest: a
    /// scan has just read whatever it did not trust.
    pub fn settle(&mut self, looked_at: &HashSet<&Path>) {
        self.entries
            .retain(|path, _| looked_at.contains(Path::new(path)));
        self.distrusted = false;
    }

    /// Show what is kept and believe none of its stamps: the outcomes were
    /// made by a reader whose rules this build may have changed.
    pub fn distrust(&mut self) {
        self.distrusted = true;
    }

    /// Every conversation kept, in no order.
    pub fn cards(&self) -> impl Iterator<Item = &VaultSession> {
        self.entries
            .values()
            .filter_map(|entry| match &entry.outcome {
                Outcome::Card(card) => Some(card.as_ref()),
                Outcome::NotASession | Outcome::Unreadable(_) => None,
            })
    }

    /// How many files are kept.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether nothing is kept: no scan has looked yet.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stamp(len: u64) -> FileStamp {
        FileStamp {
            len,
            modified_ms: 1_794_000_000_000,
            changed_ms: 1_794_000_000_000,
            file: (1, 2),
        }
    }

    #[test]
    fn a_file_that_has_not_moved_gives_what_it_gave() {
        let mut index = Index::default();
        let path = Path::new("/store/a.jsonl");
        index.keep(path, stamp(10), Outcome::NotASession);
        assert_eq!(index.kept(path, stamp(10)), Some(&Outcome::NotASession));
    }

    #[test]
    fn a_file_that_moved_is_read_again() {
        let mut index = Index::default();
        let path = Path::new("/store/a.jsonl");
        index.keep(path, stamp(10), Outcome::NotASession);
        assert_eq!(index.kept(path, stamp(11)), None);
    }

    #[test]
    fn a_distrusted_index_shows_nothing_as_kept_until_a_scan_settles_it() {
        let mut index = Index::default();
        let path = Path::new("/store/a.jsonl");
        index.keep(path, stamp(10), Outcome::Unreadable("unreadable".into()));
        index.distrust();
        assert_eq!(
            index.kept(path, stamp(10)),
            None,
            "an old reader's outcome was believed"
        );
        index.settle(&HashSet::from([path]));
        assert!(
            index.kept(path, stamp(10)).is_some(),
            "a settled index is believed"
        );
    }

    #[test]
    fn a_file_the_scan_did_not_look_at_is_forgotten() {
        let mut index = Index::default();
        let (stays, goes) = (Path::new("/store/a.jsonl"), Path::new("/store/b.jsonl"));
        index.keep(stays, stamp(1), Outcome::NotASession);
        index.keep(goes, stamp(2), Outcome::NotASession);
        index.settle(&HashSet::from([stays]));
        assert_eq!((index.len(), index.kept(goes, stamp(2))), (1, None));
    }

    #[test]
    fn what_is_kept_survives_being_written_down_and_read_back() {
        let mut index = Index::default();
        let path = Path::new("/store/a.jsonl");
        index.keep(path, stamp(10), Outcome::Unreadable("unreadable".into()));
        let written = serde_json::to_vec(&index).expect("an index is plain data");
        let read: Index = serde_json::from_slice(&written).expect("and reads back");
        assert_eq!(read, index);
    }
}
