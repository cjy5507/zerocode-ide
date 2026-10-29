//! Rows that remember how they last changed (t-82988).
//!
//! A ledger write used to learn what had changed by serializing the whole
//! ledger and hashing it — 44 MB on the person's machine on 2026-10-29 — and
//! then rewrote every table whose hash had moved: all 43,540 letters, for one
//! new letter. The store cannot tell an untouched run from a changed one
//! without reading every byte of it, but the ledger can.
//!
//! Every list of a run's rows is a [`Marked`]. Anything that could change a
//! row already there — `retain`, `iter_mut`, an assignment through an index,
//! `&mut` handed to a helper — reaches the rows through `DerefMut`, and that
//! stamps the list with an edit nobody else has had. Only [`Marked::push`]
//! keeps the stamp, because an append leaves every row before it as it was.
//! A copy keeps it too: the copy holds the same rows.
//!
//! The store keeps the stamp it last wrote beside what it wrote, so a list
//! whose stamp has not moved still begins with the rows the disk holds, and
//! only the rows after them are new.

use std::sync::atomic::{AtomicU64, Ordering};

use super::LedgerProjectionV1;

/// An edit stamp no other list in this process has had. Never zero, so a
/// default [`Share`] names no list.
fn fresh() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// Rows, and the stamp of their last change other than an append.
#[derive(Clone)]
pub struct Marked<T> {
    rows: Vec<T>,
    edit: u64,
}

impl<T> Marked<T> {
    /// Append one row. The stamp stays: every row before it is unchanged.
    pub fn push(&mut self, row: T) {
        self.rows.push(row);
    }

    /// The stamp of the last change that was not an append.
    #[must_use]
    pub const fn edit(&self) -> u64 {
        self.edit
    }

    /// For serde's `skip_serializing_if`, which names a function by path.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

impl<T> Default for Marked<T> {
    fn default() -> Self {
        Self::from(Vec::new())
    }
}

impl<T> From<Vec<T>> for Marked<T> {
    fn from(rows: Vec<T>) -> Self {
        Self {
            rows,
            edit: fresh(),
        }
    }
}

impl<T> FromIterator<T> for Marked<T> {
    fn from_iter<I: IntoIterator<Item = T>>(rows: I) -> Self {
        Self::from(rows.into_iter().collect::<Vec<_>>())
    }
}

impl<T> std::ops::Deref for Marked<T> {
    type Target = Vec<T>;

    fn deref(&self) -> &Vec<T> {
        &self.rows
    }
}

impl<T> std::ops::DerefMut for Marked<T> {
    /// Whoever borrows the rows mutably may change one in place, so the
    /// borrow is the edit — whether or not anything moves.
    fn deref_mut(&mut self) -> &mut Vec<T> {
        self.edit = fresh();
        &mut self.rows
    }
}

impl<'a, T> IntoIterator for &'a Marked<T> {
    type Item = &'a T;
    type IntoIter = std::slice::Iter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.rows.iter()
    }
}

impl<'a, T> IntoIterator for &'a mut Marked<T> {
    type Item = &'a mut T;
    type IntoIter = std::slice::IterMut<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        std::ops::DerefMut::deref_mut(self).iter_mut()
    }
}

impl<T> IntoIterator for Marked<T> {
    type Item = T;
    type IntoIter = std::vec::IntoIter<T>;

    fn into_iter(self) -> Self::IntoIter {
        self.rows.into_iter()
    }
}

/// Printed as the rows alone: the stamp is this process's bookkeeping, and
/// a `dbg!` of a run reads the same as it did before the stamp existed.
impl<T: std::fmt::Debug> std::fmt::Debug for Marked<T> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.rows.fmt(formatter)
    }
}

/// Written as the rows alone, so a ledger file keeps its shape.
impl<T: serde::Serialize> serde::Serialize for Marked<T> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.rows.serialize(serializer)
    }
}

/// Read back with a fresh stamp: rows from a file are rows no store has
/// been told about by this process.
impl<'de, T: serde::Deserialize<'de>> serde::Deserialize<'de> for Marked<T> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Vec::<T>::deserialize(deserializer).map(Self::from)
    }
}

/// One table's rows from one list in an export: how many, and the stamp of
/// the list they were made from. The default names no list, and so matches
/// nothing a store remembers.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Share {
    pub rows: usize,
    pub edit: u64,
}

/// One run's share of each table that holds a run's rows, in the order
/// [`super::Ledger::export_marked`] writes them. Inboxes and acknowledgements
/// are both made from the run's inboxes, and carry that list's stamp.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RunShare {
    pub tasks: Share,
    pub dispatches: Share,
    pub workers: Share,
    pub attachments: Share,
    pub gates: Share,
    pub messages: Share,
    pub inboxes: Share,
    pub acked: Share,
}

/// A ledger's tables, and how each run's rows in them last changed.
///
/// Built only by [`super::Ledger::export_marked`], and never opened for
/// writing: the shares describe the lists the rows were made from, and a
/// projection edited after the fact would no longer be what they describe.
pub struct MarkedExport {
    pub(super) projection: LedgerProjectionV1,
    pub(super) runs: Vec<RunShare>,
    pub(super) served: Share,
}

impl MarkedExport {
    #[must_use]
    pub const fn projection(&self) -> &LedgerProjectionV1 {
        &self.projection
    }

    /// One share per run, in the order of `projection().runs`.
    #[must_use]
    pub fn runs(&self) -> &[RunShare] {
        &self.runs
    }

    #[must_use]
    pub const fn served(&self) -> Share {
        self.served
    }

    #[must_use]
    pub fn into_projection(self) -> LedgerProjectionV1 {
        self.projection
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_append_keeps_the_stamp_and_every_other_borrow_moves_it() {
        let mut rows: Marked<u32> = vec![1, 2].into();
        let first = rows.edit();
        rows.push(3);
        assert_eq!(rows.edit(), first, "an append leaves the rows before it");

        let read: u32 = rows.iter().sum();
        assert_eq!((read, rows.edit()), (6, first), "a read is not an edit");

        rows.retain(|row| *row != 2);
        let retained = rows.edit();
        assert_ne!(retained, first, "a retain may drop a row");

        for row in &mut rows {
            *row += 1;
        }
        assert_ne!(rows.edit(), retained, "an iteration by `&mut` may edit");

        let indexed = rows.edit();
        rows[0] = 9;
        assert_ne!(rows.edit(), indexed, "an assignment through an index edits");
        assert_eq!(*rows, vec![9, 4]);
    }

    #[test]
    fn a_copy_keeps_the_stamp_and_a_new_list_never_shares_one() {
        let rows: Marked<u32> = vec![1].into();
        assert_eq!(rows.clone().edit(), rows.edit());
        assert_ne!(Marked::<u32>::default().edit(), rows.edit());
        assert_ne!(Share::default().edit, rows.edit());
    }

    #[test]
    fn the_rows_cross_serde_as_a_plain_list() {
        let rows: Marked<u32> = vec![1, 2].into();
        let written = serde_json::to_string(&rows).expect("rows serialize");
        assert_eq!(written, "[1,2]");
        let read: Marked<u32> = serde_json::from_str(&written).expect("rows deserialize");
        assert_eq!(*read, vec![1, 2]);
        assert_ne!(read.edit(), rows.edit());
        assert_eq!(format!("{rows:?}"), "[1, 2]");
    }
}
