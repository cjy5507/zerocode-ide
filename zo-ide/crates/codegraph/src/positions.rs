//! Where one name occurs in one file, packed into the `refs.positions` blob.
//!
//! A reference row used to be five integers per occurrence — 1.97 million
//! rows and 65 MB of table and index for this repository. One row per name
//! and file now carries every occurrence as LEB128 varints: the start byte
//! and the row as deltas from the previous occurrence (both only grow along a
//! file), the column as it is. The table has one row per distinct name in a
//! file — 0.30 million here — and an occurrence costs a few bytes.

use std::collections::BTreeMap;

use crate::model::Reference;

/// One occurrence of a name: where its first byte is, and that byte's row and
/// byte column.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct Occurrence {
    pub(crate) start_byte: usize,
    pub(crate) row: usize,
    pub(crate) column: usize,
}

/// One file's references to one name, packed: a future `refs` row less its
/// ids.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PackedName {
    pub(crate) name: String,
    pub(crate) occurrences: usize,
    pub(crate) positions: Vec<u8>,
}

/// Group one file's references by name and pack each group, names in
/// lexical order. Pure and per file, so it runs on the extraction threads
/// and the writer resolves one name id per group instead of one per
/// reference.
pub(crate) fn pack_by_name(references: &[Reference]) -> Vec<PackedName> {
    let mut by_name = BTreeMap::<&str, Vec<Occurrence>>::new();
    for reference in references {
        by_name
            .entry(reference.name.as_str())
            .or_default()
            .push(Occurrence {
                start_byte: reference.range.start_byte,
                row: reference.range.start.row,
                column: reference.range.start.column,
            });
    }
    by_name
        .into_iter()
        .map(|(name, mut occurrences)| {
            occurrences.sort_unstable();
            PackedName {
                name: name.to_owned(),
                occurrences: occurrences.len(),
                positions: encode(&occurrences),
            }
        })
        .collect()
}

const CONTINUATION: u8 = 0x80;
const PAYLOAD_BITS: u32 = 7;
const PAYLOAD_MASK: usize = 0x7f;

/// Pack `occurrences`, which must be in source order (the writer sorts them).
pub(crate) fn encode(occurrences: &[Occurrence]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(occurrences.len() * 4);
    let (mut previous_byte, mut previous_row) = (0, 0);
    for occurrence in occurrences {
        put(&mut bytes, occurrence.start_byte.saturating_sub(previous_byte));
        put(&mut bytes, occurrence.row.saturating_sub(previous_row));
        put(&mut bytes, occurrence.column);
        previous_byte = occurrence.start_byte;
        previous_row = occurrence.row;
    }
    bytes
}

/// Hand each occurrence [`encode`] packed to `each`, in source order; `None`
/// for bytes it cannot have written. Nothing is allocated: a caller filling
/// one answer from many rows decodes straight into it.
pub(crate) fn decode_each(mut bytes: &[u8], mut each: impl FnMut(Occurrence)) -> Option<()> {
    let (mut start_byte, mut row) = (0_usize, 0_usize);
    while !bytes.is_empty() {
        start_byte = start_byte.checked_add(take(&mut bytes)?)?;
        row = row.checked_add(take(&mut bytes)?)?;
        let column = take(&mut bytes)?;
        each(Occurrence {
            start_byte,
            row,
            column,
        });
    }
    Some(())
}

fn put(bytes: &mut Vec<u8>, mut value: usize) {
    loop {
        let low = u8::try_from(value & PAYLOAD_MASK).unwrap_or_default();
        value >>= PAYLOAD_BITS;
        if value == 0 {
            bytes.push(low);
            return;
        }
        bytes.push(low | CONTINUATION);
    }
}

fn take(bytes: &mut &[u8]) -> Option<usize> {
    let mut value = 0_usize;
    let mut shift = 0_u32;
    loop {
        let (&byte, rest) = bytes.split_first()?;
        *bytes = rest;
        let payload = usize::from(byte & !CONTINUATION);
        if shift >= usize::BITS || (payload << shift) >> shift != payload {
            return None;
        }
        value |= payload << shift;
        if byte & CONTINUATION == 0 {
            return Some(value);
        }
        shift += PAYLOAD_BITS;
    }
}

#[cfg(test)]
mod tests {
    use super::{decode_each, encode, Occurrence};

    fn decode(bytes: &[u8]) -> Option<Vec<Occurrence>> {
        let mut occurrences = Vec::new();
        decode_each(bytes, |occurrence| occurrences.push(occurrence)).map(|()| occurrences)
    }

    fn at(start_byte: usize, row: usize, column: usize) -> Occurrence {
        Occurrence {
            start_byte,
            row,
            column,
        }
    }

    #[test]
    fn occurrences_round_trip_through_the_blob() {
        let occurrences = vec![
            at(0, 0, 0),
            at(7, 0, 7),
            at(300, 12, 4),
            at(5 * 1024 * 1024 - 9, 150_000, 1_023),
        ];
        let bytes = encode(&occurrences);
        assert_eq!(decode(&bytes), Some(occurrences));
        assert_eq!(decode(&[]), Some(Vec::new()));
    }

    #[test]
    fn a_small_occurrence_costs_three_bytes() {
        assert_eq!(encode(&[at(40, 2, 17), at(90, 3, 5)]).len(), 6);
    }

    #[test]
    fn bytes_the_encoder_cannot_have_written_are_refused() {
        // A continuation bit with nothing after it, and a value wider than a
        // machine word.
        assert_eq!(decode(&[0x80]), None);
        assert_eq!(decode(&[0xff; 11]), None);
        // Two of an occurrence's three numbers.
        assert_eq!(decode(&[1, 2]), None);
    }
}
