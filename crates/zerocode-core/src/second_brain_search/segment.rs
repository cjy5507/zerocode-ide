//! One index file: the passages of a set of files, by the words they hold.
//!
//! A file is written once and never changed. It holds a row per file and a
//! row per passage, what each file is called and links to, and for every word
//! the passages that hold it. A search reads the rows and then only the words
//! it asks for: each is one probe into a hash table and one read of that
//! word's passages.
//!
//! Nothing here trusts the bytes, and every byte a search reads is summed.
//! The header says how long the file is and where each part lies, and carries
//! its own sum and the rows'; each slot of the word table carries its sum and
//! each word's passages theirs. A file that fails any of it — cut short,
//! zeroed, overwritten — is [`Damaged`] and its caller builds another.
//!
//! ```text
//! header · file rows · passage rows · names and links · replaced rows · word table · passages by word
//! ```

use std::collections::{BTreeSet, HashMap, HashSet};
use std::io::{Read, Seek, SeekFrom};

use crate::file_stamp::FileStamp;
use crate::second_brain_graph::EdgeKind;

/// The layout's number. A file written under another is built again.
/// 2: a file's record holds the stamp every reader in this crate compares
/// (`FileStamp`) in place of a stamp of this module's own.
pub(super) const FORMAT: u32 = 2;

const MAGIC: [u8; 8] = *b"ZCSEARCH";
/// The magic, the format, four spare bytes, fifteen numbers and the sum of
/// all of that.
const HEADER_NUMBERS: usize = 15;
const HEADER_SUMMED: usize = 8 + 4 + 4 + HEADER_NUMBERS * 8;
const HEADER_BYTES: usize = HEADER_SUMMED + 8;
const DOC_ROW_BYTES: usize = 12;
const PASSAGE_ROW_BYTES: usize = 12;
/// A slot of the word table: a tag of the word's hash, where its entry lies
/// and how long it is, and the sum of those three — four bytes each.
const SLOT_BYTES: usize = 16;
const SLOT_SUMMED: usize = 12;
/// A slot no word took. Neither zeroes nor ones: a table that was never
/// written, or was written over, does not read as empty.
const EMPTY_SLOT: [u8; SLOT_BYTES] = [0x5a; SLOT_BYTES];
/// Slots per word: half the table stays empty, so a probe ends within a slot
/// or two of where it began.
const SLOTS_PER_WORD: usize = 2;
/// Slots one probe reads at a time.
const SLOTS_PER_READ: usize = 4;

/// The spellings a relation is stored by: its frontmatter key, and the body's
/// own for a plain link.
const KINDS: [EdgeKind; 6] = [
    EdgeKind::Mentions,
    EdgeKind::Related,
    EdgeKind::Implements,
    EdgeKind::DependsOn,
    EdgeKind::Supersedes,
    EdgeKind::Contradicts,
];

/// The byte a relation is stored as: its place in [`KINDS`]. Every kind is
/// named, so a kind the graph gains does not compile until it has a byte
/// here — give it the next one and add it to the end of [`KINDS`].
pub(super) const fn kind_byte(kind: EdgeKind) -> u8 {
    match kind {
        EdgeKind::Mentions => 0,
        EdgeKind::Related => 1,
        EdgeKind::Implements => 2,
        EdgeKind::DependsOn => 3,
        EdgeKind::Supersedes => 4,
        EdgeKind::Contradicts => 5,
    }
}

/// The relation a stored byte names, or nothing for a byte no kind has.
pub(super) fn kind_of(byte: u8) -> Option<EdgeKind> {
    KINDS.get(usize::from(byte)).copied()
}

/// An index file that is not what its header says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Damaged;

impl From<std::io::Error> for Damaged {
    fn from(_: std::io::Error) -> Self {
        Self
    }
}

/// What a word's place in the table is found by. A parameter so a test can
/// make every word collide.
pub(super) type WordHash = fn(&[u8]) -> u64;

/// The hash the index is written with.
pub(super) fn word_hash(word: &[u8]) -> u64 {
    crate::launch_budget::fnv1a(word.iter().copied())
}

/// Where a passage lies in its file's text and the line it starts on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) struct Place {
    pub start: u32,
    pub len: u32,
    pub line: u32,
}

/// What a file is called and what it says about other files.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(super) struct DocRecord {
    /// Below its source's folder, `/`-separated.
    pub path: String,
    /// What the walk saw of the file when it was read: a walk that sees
    /// another reads it again.
    pub stamp: FileStamp,
    pub title: String,
    pub tags: Vec<String>,
    /// Links as the file wrote them, in the order it wrote them.
    pub links: Vec<(EdgeKind, String)>,
}

/// One file as it goes into an index: its record, the words of its name and
/// the words of each passage.
#[derive(Debug, Clone, Default)]
pub(super) struct DocInput {
    pub record: DocRecord,
    pub name: BTreeSet<String>,
    pub passages: Vec<(Place, BTreeSet<String>)>,
}

/// One file's row: where its record lies, its first passage (the name) and
/// how many different words it holds.
#[derive(Debug, Clone, Copy)]
pub(super) struct DocRow {
    heap_at: u32,
    pub first_passage: u32,
    pub length: u32,
}

/// The bytes of an index over `docs`. `replaced` names rows of the index this
/// one is laid over that no longer stand (see [`Segment::replaced`]).
pub(super) fn write(docs: &[DocInput], replaced: &[u32], hash: WordHash) -> Vec<u8> {
    let mut doc_rows = Vec::with_capacity(docs.len() * DOC_ROW_BYTES);
    let mut passage_rows = Vec::new();
    let mut heap = Vec::new();
    let mut by_word: HashMap<&str, Vec<u32>> = HashMap::new();
    let mut passages = 0_u32;
    let mut total_length = 0_u64;
    for doc in docs {
        let length = doc
            .passages
            .iter()
            .flat_map(|(_, words)| words)
            .chain(&doc.name)
            .map(String::as_str)
            .collect::<HashSet<_>>()
            .len();
        let length = u32::try_from(length).unwrap_or(u32::MAX);
        total_length += u64::from(length);
        put_u32(&mut doc_rows, u32::try_from(heap.len()).unwrap_or(u32::MAX));
        put_u32(&mut doc_rows, passages);
        put_u32(&mut doc_rows, length);
        put_record(&mut heap, &doc.record);
        // The name is the file's first passage and lies nowhere in its text.
        let named = std::iter::once((Place::default(), &doc.name));
        for (place, words) in named.chain(doc.passages.iter().map(|(place, words)| (*place, words)))
        {
            put_u32(&mut passage_rows, place.start);
            put_u32(&mut passage_rows, place.len);
            put_u32(&mut passage_rows, place.line);
            for word in words {
                by_word.entry(word.as_str()).or_default().push(passages);
            }
            passages += 1;
        }
    }

    let slots = if by_word.is_empty() {
        0
    } else {
        (by_word.len() * SLOTS_PER_WORD).next_power_of_two()
    };
    let mut table = EMPTY_SLOT.repeat(slots);
    let mut postings = Vec::new();
    // In the words' own order, so the same files always write the same bytes.
    let mut words: Vec<(&str, Vec<u32>)> = by_word.into_iter().collect();
    words.sort_unstable_by_key(|(word, _)| *word);
    for (word, held) in &words {
        // An entry: its own sum, the word, and the word's passages.
        let at = postings.len();
        put_u32(&mut postings, 0);
        put_str(&mut postings, word);
        put_varint(&mut postings, held.len() as u64);
        let mut last = 0;
        for id in held {
            put_varint(&mut postings, u64::from(id - last));
            last = *id;
        }
        let sum = sum32(&postings[at + 4..]);
        postings[at..at + 4].copy_from_slice(&sum.to_le_bytes());
        let key = hash(word.as_bytes());
        let mut slot = slot_of(key, slots);
        while table[slot * SLOT_BYTES..(slot + 1) * SLOT_BYTES] != EMPTY_SLOT {
            slot = (slot + 1) % slots;
        }
        let row = &mut table[slot * SLOT_BYTES..(slot + 1) * SLOT_BYTES];
        row[..4].copy_from_slice(&tag_of(key).to_le_bytes());
        row[4..8].copy_from_slice(&u32::try_from(at).unwrap_or(u32::MAX).to_le_bytes());
        row[8..12].copy_from_slice(
            &u32::try_from(postings.len() - at)
                .unwrap_or(u32::MAX)
                .to_le_bytes(),
        );
        let sum = sum32(&row[..SLOT_SUMMED]);
        row[SLOT_SUMMED..].copy_from_slice(&sum.to_le_bytes());
    }
    let mut replaced_rows = Vec::with_capacity(replaced.len() * 4);
    for row in replaced {
        put_u32(&mut replaced_rows, *row);
    }

    let doc_rows_at = HEADER_BYTES;
    let passage_rows_at = doc_rows_at + doc_rows.len();
    let heap_at = passage_rows_at + passage_rows.len();
    let replaced_at = heap_at + heap.len();
    let table_at = replaced_at + replaced_rows.len();
    let postings_at = table_at + table.len();
    let file_len = postings_at + postings.len();
    // The rows are read whole when the file is opened, so they are summed
    // whole; the table and the passages are read a word at a time and each
    // word's entry carries its own sum.
    let rows_sum = sum64([&doc_rows, &passage_rows, &heap, &replaced_rows]);
    let mut out = Vec::with_capacity(file_len);
    out.extend_from_slice(&MAGIC);
    put_u32(&mut out, FORMAT);
    put_u32(&mut out, 0);
    for number in [
        file_len,
        docs.len(),
        passages as usize,
        usize::try_from(total_length).unwrap_or(usize::MAX),
        slots,
        doc_rows_at,
        passage_rows_at,
        heap_at,
        heap.len(),
        table_at,
        postings_at,
        postings.len(),
        replaced_at,
        replaced.len(),
    ] {
        out.extend_from_slice(&(number as u64).to_le_bytes());
    }
    out.extend_from_slice(&rows_sum.to_le_bytes());
    let header_sum = sum64([&out]);
    out.extend_from_slice(&header_sum.to_le_bytes());
    for part in [doc_rows, passage_rows, heap, replaced_rows, table, postings] {
        out.extend_from_slice(&part);
    }
    out
}

/// An index file, open for reading.
#[derive(Debug)]
pub(super) struct Segment<R> {
    source: R,
    hash: WordHash,
    docs: Vec<DocRow>,
    passages: Vec<Place>,
    records: Vec<DocRecord>,
    replaced: Vec<u32>,
    total_length: u64,
    table: (u64, usize),
    postings: (u64, u64),
}

impl<R: Read + Seek> Segment<R> {
    /// Reads the header, the rows and the records, and refuses a file its
    /// header does not describe.
    pub(super) fn open(mut source: R, hash: WordHash) -> Result<Self, Damaged> {
        let actual = source.seek(SeekFrom::End(0))?;
        let header = read_at(&mut source, 0, HEADER_BYTES)?;
        let (summed, sum) = header.split_at(HEADER_SUMMED);
        if crate::launch_budget::fnv1a(summed.iter().copied()) != read_u64(sum, 0) {
            return Err(Damaged);
        }
        let mut at = Reader::new(summed);
        if at.bytes(MAGIC.len())? != MAGIC || at.u32()? != FORMAT {
            return Err(Damaged);
        }
        at.u32()?;
        let mut numbers = [0_u64; HEADER_NUMBERS];
        for number in &mut numbers {
            *number = at.u64()?;
        }
        let [
            file_len,
            docs,
            passages,
            total_length,
            slots,
            doc_rows_at,
            passage_rows_at,
            heap_at,
            heap_len,
            table_at,
            postings_at,
            postings_len,
            replaced_at,
            replaced,
            rows_sum,
        ] = numbers;
        let part = |from: u64, count: u64, each: usize| -> Result<(u64, usize), Damaged> {
            let bytes = count.checked_mul(each as u64).ok_or(Damaged)?;
            let end = from.checked_add(bytes).ok_or(Damaged)?;
            if from < HEADER_BYTES as u64 || end > file_len {
                return Err(Damaged);
            }
            Ok((from, usize::try_from(bytes).map_err(|_| Damaged)?))
        };
        if file_len != actual || (slots != 0 && !slots.is_power_of_two()) {
            return Err(Damaged);
        }
        let (rows_at, rows_len) = part(doc_rows_at, docs, DOC_ROW_BYTES)?;
        let (places_at, places_len) = part(passage_rows_at, passages, PASSAGE_ROW_BYTES)?;
        let (heap_at, heap_len) = part(heap_at, heap_len, 1)?;
        let (replaced_at, replaced_len) = part(replaced_at, replaced, 4)?;
        part(table_at, slots, SLOT_BYTES)?;
        part(postings_at, postings_len, 1)?;

        let rows = read_at(&mut source, rows_at, rows_len)?;
        let places = read_at(&mut source, places_at, places_len)?;
        let heap = read_at(&mut source, heap_at, heap_len)?;
        let replaced = read_at(&mut source, replaced_at, replaced_len)?;
        if sum64([&rows, &places, &heap, &replaced]) != rows_sum {
            return Err(Damaged);
        }
        let docs: Vec<DocRow> = rows
            .chunks_exact(DOC_ROW_BYTES)
            .map(|row| DocRow {
                heap_at: read_u32(row, 0),
                first_passage: read_u32(row, 4),
                length: read_u32(row, 8),
            })
            .collect();
        // Every file's passages follow the one before it, and its record lies
        // inside the heap: what the lookups below lean on.
        let ordered = docs.windows(2).all(|pair| {
            pair[0].first_passage < pair[1].first_passage && pair[0].heap_at < pair[1].heap_at
        });
        let inside = docs.last().is_none_or(|last| {
            u64::from(last.first_passage) < passages && (last.heap_at as usize) < heap_len
        });
        if !ordered || !inside || docs.first().is_some_and(|first| first.first_passage != 0) {
            return Err(Damaged);
        }
        let passages = places
            .chunks_exact(PASSAGE_ROW_BYTES)
            .map(|row| Place {
                start: read_u32(row, 0),
                len: read_u32(row, 4),
                line: read_u32(row, 8),
            })
            .collect();
        let records = docs
            .iter()
            .map(|row| {
                read_record(&mut Reader::new(
                    heap.get(row.heap_at as usize..).ok_or(Damaged)?,
                ))
            })
            .collect::<Result<_, _>>()?;
        let replaced = replaced
            .chunks_exact(4)
            .map(|row| read_u32(row, 0))
            .collect();
        Ok(Self {
            source,
            hash,
            docs,
            passages,
            records,
            replaced,
            total_length,
            table: (table_at, usize::try_from(slots).map_err(|_| Damaged)?),
            postings: (postings_at, postings_len),
        })
    }

    pub(super) fn docs(&self) -> &[DocRow] {
        &self.docs
    }

    pub(super) fn passages(&self) -> &[Place] {
        &self.passages
    }

    /// The different words of every file here, summed.
    pub(super) fn total_length(&self) -> u64 {
        self.total_length
    }

    /// Rows of the index this one is laid over that no longer stand: their
    /// files changed, and stand here, or are gone.
    pub(super) fn replaced(&self) -> &[u32] {
        &self.replaced
    }

    /// The passages one file owns after its name, as a range of passage rows.
    pub(super) fn body_of(&self, doc: usize) -> std::ops::Range<u32> {
        let first = self.docs[doc].first_passage;
        let end = self.docs.get(doc + 1).map_or(
            u32::try_from(self.passages.len()).unwrap_or(u32::MAX),
            |next| next.first_passage,
        );
        first + 1..end
    }

    /// Every file's record, in row order.
    #[cfg(test)]
    pub(super) fn records(&self) -> &[DocRecord] {
        &self.records
    }

    /// The records, handed to a caller that keeps them: this file answers
    /// with none afterwards.
    pub(super) fn take_records(&mut self) -> Vec<DocRecord> {
        std::mem::take(&mut self.records)
    }

    /// The passages that hold `word`, rising. A word the table does not hold
    /// answers nothing, and so does one whose hash another word shares: the
    /// word itself is written beside its passages and read back.
    pub(super) fn holders(&mut self, word: &str) -> Result<Vec<u32>, Damaged> {
        let (table_at, slots) = self.table;
        if slots == 0 {
            return Ok(Vec::new());
        }
        let key = (self.hash)(word.as_bytes());
        let mut slot = slot_of(key, slots);
        let mut looked = 0;
        while looked < slots {
            // To the table's end at most; the probe wraps on its next read.
            let count = SLOTS_PER_READ.min(slots - slot);
            let rows = read_at(
                &mut self.source,
                table_at + (slot * SLOT_BYTES) as u64,
                count * SLOT_BYTES,
            )?;
            for row in rows.chunks_exact(SLOT_BYTES) {
                if row == EMPTY_SLOT {
                    return Ok(Vec::new());
                }
                // A taken slot is whole, whoever's it is: a probe that walked
                // past a slot it could not read would call a word absent.
                let (at, len) = (read_u32(row, 4), read_u32(row, 8));
                if sum32(&row[..SLOT_SUMMED]) != read_u32(row, SLOT_SUMMED)
                    || u64::from(at) + u64::from(len) > self.postings.1
                {
                    return Err(Damaged);
                }
                if read_u32(row, 0) == tag_of(key)
                    && let Some(held) = self.passages_at(at, len, word)?
                {
                    return Ok(held);
                }
            }
            looked += count;
            slot = (slot + count) % slots;
        }
        Ok(Vec::new())
    }

    /// One word's passages, or nothing when the entry is another word's.
    fn passages_at(&mut self, at: u32, len: u32, word: &str) -> Result<Option<Vec<u32>>, Damaged> {
        let bytes = read_at(
            &mut self.source,
            self.postings.0 + u64::from(at),
            len as usize,
        )?;
        let mut entry = Reader::new(&bytes);
        if entry.u32()? != sum32(entry.rest) {
            return Err(Damaged);
        }
        if entry.str()? != word {
            return Ok(None);
        }
        let count = usize::try_from(entry.varint()?).map_err(|_| Damaged)?;
        // Each passage takes a byte at least: a count the entry could not
        // hold is not a count.
        if count > bytes.len() {
            return Err(Damaged);
        }
        let mut held = Vec::with_capacity(count);
        let mut last = 0_u64;
        for _ in 0..count {
            last = last.checked_add(entry.varint()?).ok_or(Damaged)?;
            if last >= self.passages.len() as u64 {
                return Err(Damaged);
            }
            held.push(u32::try_from(last).map_err(|_| Damaged)?);
        }
        Ok(Some(held))
    }
}

/// Where a key's probe begins: its low bits.
fn slot_of(key: u64, slots: usize) -> usize {
    // `slots` is a power of two that fits a `usize`, so the mask does too.
    (key & (slots as u64 - 1)) as usize
}

/// What a slot remembers of the key that took it: its high bits, which the
/// slot's own place does not already say.
fn tag_of(key: u64) -> u32 {
    (key >> 32) as u32
}

/// What a run of parts sums to.
fn sum64<const PARTS: usize>(parts: [&Vec<u8>; PARTS]) -> u64 {
    crate::launch_budget::fnv1a(parts.into_iter().flatten().copied())
}

/// What a slot or one word's entry sums to, folded to four bytes.
fn sum32(bytes: &[u8]) -> u32 {
    let sum = crate::launch_budget::fnv1a(bytes.iter().copied());
    (sum ^ (sum >> 32)) as u32
}

fn read_at(source: &mut (impl Read + Seek), at: u64, len: usize) -> Result<Vec<u8>, Damaged> {
    source.seek(SeekFrom::Start(at))?;
    let mut bytes = vec![0_u8; len];
    source.read_exact(&mut bytes)?;
    Ok(bytes)
}

fn put_record(heap: &mut Vec<u8>, record: &DocRecord) {
    put_str(heap, &record.path);
    let FileStamp {
        len,
        modified_ms,
        changed_ms,
        file: (device, inode),
    } = record.stamp;
    heap.extend_from_slice(&len.to_le_bytes());
    heap.extend_from_slice(&modified_ms.to_le_bytes());
    heap.extend_from_slice(&changed_ms.to_le_bytes());
    heap.extend_from_slice(&device.to_le_bytes());
    heap.extend_from_slice(&inode.to_le_bytes());
    put_str(heap, &record.title);
    put_varint(heap, record.tags.len() as u64);
    for tag in &record.tags {
        put_str(heap, tag);
    }
    put_varint(heap, record.links.len() as u64);
    for (kind, target) in &record.links {
        heap.push(kind_byte(*kind));
        put_str(heap, target);
    }
}

fn read_record(at: &mut Reader<'_>) -> Result<DocRecord, Damaged> {
    let path = at.str()?.to_string();
    let stamp = FileStamp {
        len: at.u64()?,
        modified_ms: at.i64()?,
        changed_ms: at.i64()?,
        file: (at.u64()?, at.u64()?),
    };
    let title = at.str()?.to_string();
    let tags = (0..at.count()?)
        .map(|_| at.str().map(str::to_string))
        .collect::<Result<_, _>>()?;
    let links = (0..at.count()?)
        .map(|_| {
            let kind = kind_of(at.bytes(1)?[0]).ok_or(Damaged)?;
            Ok((kind, at.str()?.to_string()))
        })
        .collect::<Result<_, Damaged>>()?;
    Ok(DocRecord {
        path,
        stamp,
        title,
        tags,
        links,
    })
}

fn put_u32(into: &mut Vec<u8>, value: u32) {
    into.extend_from_slice(&value.to_le_bytes());
}

fn put_varint(into: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        into.push((value & 0x7f) as u8 | 0x80);
        value >>= 7;
    }
    into.push(value as u8);
}

fn put_str(into: &mut Vec<u8>, text: &str) {
    put_varint(into, text.len() as u64);
    into.extend_from_slice(text.as_bytes());
}

/// A row's number at `at`. Rows are cut to their width before this reads
/// them, so the four bytes are there.
fn read_u32(row: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([row[at], row[at + 1], row[at + 2], row[at + 3]])
}

fn read_u64(row: &[u8], at: usize) -> u64 {
    u64::from(read_u32(row, at)) | u64::from(read_u32(row, at + 4)) << 32
}

/// Bytes read front to back, where running out is [`Damaged`].
struct Reader<'a> {
    rest: &'a [u8],
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { rest: bytes }
    }

    fn bytes(&mut self, len: usize) -> Result<&'a [u8], Damaged> {
        if len > self.rest.len() {
            return Err(Damaged);
        }
        let (head, rest) = self.rest.split_at(len);
        self.rest = rest;
        Ok(head)
    }

    fn u32(&mut self) -> Result<u32, Damaged> {
        Ok(read_u32(self.bytes(4)?, 0))
    }

    fn u64(&mut self) -> Result<u64, Damaged> {
        Ok(read_u64(self.bytes(8)?, 0))
    }

    fn i64(&mut self) -> Result<i64, Damaged> {
        Ok(i64::from_le_bytes(self.u64()?.to_le_bytes()))
    }

    fn varint(&mut self) -> Result<u64, Damaged> {
        let mut value = 0_u64;
        for shift in (0..64).step_by(7) {
            let byte = self.bytes(1)?[0];
            value |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return Ok(value);
            }
        }
        Err(Damaged)
    }

    /// A count of things that follow, each a byte at least.
    fn count(&mut self) -> Result<usize, Damaged> {
        let count = usize::try_from(self.varint()?).map_err(|_| Damaged)?;
        if count > self.rest.len() {
            return Err(Damaged);
        }
        Ok(count)
    }

    fn str(&mut self) -> Result<&'a str, Damaged> {
        let len = usize::try_from(self.varint()?).map_err(|_| Damaged)?;
        std::str::from_utf8(self.bytes(len)?).map_err(|_| Damaged)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn words(text: &str) -> BTreeSet<String> {
        text.split_whitespace().map(str::to_string).collect()
    }

    fn doc(path: &str, name: &str, passages: &[&str]) -> DocInput {
        DocInput {
            record: DocRecord {
                path: path.to_string(),
                stamp: FileStamp {
                    len: 11,
                    modified_ms: -7,
                    changed_ms: 9,
                    file: (13, 17),
                },
                title: format!("title of {path}"),
                tags: vec!["one".to_string(), "two".to_string()],
                links: vec![
                    (EdgeKind::Mentions, "other".to_string()),
                    (EdgeKind::Supersedes, "older".to_string()),
                ],
            },
            name: words(name),
            passages: passages
                .iter()
                .enumerate()
                .map(|(at, text)| {
                    let at = u32::try_from(at).unwrap();
                    (
                        Place {
                            start: at * 10,
                            len: 9,
                            line: at + 3,
                        },
                        words(text),
                    )
                })
                .collect(),
        }
    }

    fn sample() -> Vec<DocInput> {
        vec![
            doc("a.md", "alpha", &["harbor crane", "crane lifts cargo"]),
            doc("b.md", "beta crane", &["tide table"]),
            doc("c.md", "gamma", &[]),
        ]
    }

    fn open(bytes: Vec<u8>, hash: WordHash) -> Result<Segment<Cursor<Vec<u8>>>, Damaged> {
        Segment::open(Cursor::new(bytes), hash)
    }

    /// What goes in comes out: each word's passages, each file's rows and
    /// record, and the rows a later index replaced.
    #[test]
    fn an_index_answers_with_the_passages_the_files_and_the_records_it_was_written_from() {
        let mut held = open(write(&sample(), &[4, 2], word_hash), word_hash).unwrap();
        assert_eq!(held.docs().len(), 3);
        // a.md: name 0, passages 1 and 2; b.md: name 3, passage 4; c.md: name 5.
        assert_eq!(held.holders("crane").unwrap(), [1, 2, 3]);
        assert_eq!(held.holders("alpha").unwrap(), [0]);
        assert_eq!(held.holders("tide").unwrap(), [4]);
        assert!(held.holders("absent").unwrap().is_empty());
        assert_eq!(held.body_of(0), 1..3);
        assert_eq!(held.body_of(1), 4..5);
        assert_eq!(held.body_of(2), 6..6);
        assert_eq!(
            held.passages()[2],
            Place {
                start: 10,
                len: 9,
                line: 4
            }
        );
        // a.md: alpha harbor crane lifts cargo; b.md: beta crane tide table; c.md: gamma.
        assert_eq!(held.docs()[0].length, 5);
        assert_eq!(held.total_length(), 5 + 4 + 1);
        assert_eq!(held.replaced(), [4, 2]);
        assert_eq!(held.records().len(), 3);
        assert_eq!(held.records()[1], sample()[1].record);
    }

    /// Two words with one hash share a probe run and never each other's
    /// passages; a word that was never written answers nothing even though
    /// every slot's hash is its own.
    #[test]
    fn words_whose_hashes_collide_keep_their_own_passages() {
        fn same(_: &[u8]) -> u64 {
            7
        }
        let mut held = open(write(&sample(), &[], same), same).unwrap();
        assert_eq!(held.holders("crane").unwrap(), [1, 2, 3]);
        assert_eq!(held.holders("cargo").unwrap(), [2]);
        assert_eq!(held.holders("gamma").unwrap(), [5]);
        assert!(held.holders("absent").unwrap().is_empty());
    }

    /// Every relation reads back from the byte it is stored as, and a byte
    /// no relation has names none.
    #[test]
    fn every_relation_reads_back_from_its_byte() {
        for (at, kind) in KINDS.into_iter().enumerate() {
            assert_eq!(usize::from(kind_byte(kind)), at, "{kind:?}");
            assert_eq!(kind_of(kind_byte(kind)), Some(kind));
        }
        assert_eq!(kind_of(u8::try_from(KINDS.len()).unwrap()), None);
    }

    /// An index of nothing opens and answers nothing.
    #[test]
    fn an_index_of_no_files_answers_nothing() {
        let mut held = open(write(&[], &[], word_hash), word_hash).unwrap();
        assert!(held.docs().is_empty());
        assert!(held.holders("crane").unwrap().is_empty());
        assert!(held.records().is_empty());
    }

    /// Cut anywhere, the file is refused when it is opened: its header says
    /// how long it was.
    #[test]
    fn a_file_cut_short_is_refused_wherever_the_cut_falls() {
        let whole = write(&sample(), &[1], word_hash);
        for len in 0..whole.len() {
            assert_eq!(
                open(whole[..len].to_vec(), word_hash).err(),
                Some(Damaged),
                "cut at {len} of {}",
                whole.len()
            );
        }
    }

    /// The answers a whole file gives, by word.
    fn answers(held: &mut Segment<Cursor<Vec<u8>>>) -> Vec<Result<Vec<u32>, Damaged>> {
        ["crane", "alpha", "tide", "cargo", "absent"]
            .into_iter()
            .map(|word| held.holders(word))
            .collect()
    }

    /// A byte changed, zeroed or set anywhere in the file never gives another
    /// answer: the file is refused when it is opened, or the word that reads
    /// the changed byte is refused, or the byte was one no answer reads.
    #[test]
    fn a_changed_byte_is_refused_or_changes_no_answer() {
        let whole = write(&sample(), &[1], word_hash);
        let true_answers = answers(&mut open(whole.clone(), word_hash).unwrap());
        let true_records = sample()
            .into_iter()
            .map(|doc| doc.record)
            .collect::<Vec<_>>();
        for at in 0..whole.len() {
            for change in [0xff_u8, 0x00, 0x01] {
                let mut bytes = whole.clone();
                bytes[at] = if change == 0x01 {
                    bytes[at] ^ 1
                } else {
                    change
                };
                if bytes == whole {
                    continue;
                }
                let Ok(mut held) = open(bytes, word_hash) else {
                    continue;
                };
                assert_eq!(held.records(), true_records, "byte {at} set to {change:#x}");
                for (found, wanted) in answers(&mut held).into_iter().zip(&true_answers) {
                    assert!(
                        found == *wanted || found == Err(Damaged),
                        "byte {at} set to {change:#x}: {found:?}"
                    );
                }
            }
        }
    }

    /// A stretch of the file zeroed — what a crash can leave of a file that
    /// was never flushed — is refused, wherever the stretch falls.
    #[test]
    fn a_zeroed_stretch_is_refused_and_never_reads_as_no_passages() {
        let whole = write(&sample(), &[1], word_hash);
        let true_answers = answers(&mut open(whole.clone(), word_hash).unwrap());
        for from in (0..whole.len()).step_by(7) {
            let mut bytes = whole.clone();
            let to = (from + 64).min(bytes.len());
            bytes[from..to].fill(0);
            let Ok(mut held) = open(bytes, word_hash) else {
                continue;
            };
            for (found, wanted) in answers(&mut held).into_iter().zip(&true_answers) {
                assert!(
                    found == *wanted || found == Err(Damaged),
                    "zeroed from {from}: {found:?}"
                );
            }
        }
    }
}
