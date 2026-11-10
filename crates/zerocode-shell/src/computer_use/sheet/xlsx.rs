//! An `.xlsx` file's cells, read and changed in place.
//!
//! A workbook is a zip of XML parts. A write changes three of them at most —
//! the one sheet's XML, the workbook's (so the app computes every formula on
//! opening it) and, when it had one, drops the formula order Excel cached
//! (`calcChain`, which names cells a write may have changed). Every other part
//! is copied byte for byte, still compressed: the other sheets, the styles,
//! the shared strings, pictures, charts. Inside the sheet, only the named
//! cells' elements are replaced or inserted; the rest of its bytes stay.
//!
//! Rust crates that write xlsx either cannot edit an existing file
//! (`rust_xlsxwriter`) or rebuild every part from their own model on save
//! (`umya-spreadsheet`, 40-odd dependencies), so a part they do not model
//! is lost. This road uses the `zip` and `quick-xml` already in the lock:
//! no new crate.

use std::collections::BTreeMap;
use std::io::{Cursor, Read as _, Write as _};

use quick_xml::events::{BytesStart, Event};
use quick_xml::{Reader, XmlVersion};
use zerocode_core::computer_sheet::{
    CellAddress, CellRange, FORMULA_MARK, ReadCell, SHEET_MAX_CELLS, SheetValue, number_text,
};

/// The most bytes one part of a workbook may unpack to: a zip that claims
/// more is refused rather than unpacked.
pub(super) const MAX_PART_BYTES: u64 = 256 * 1024 * 1024;

/// The sheet a new workbook gets when no `--sheet` names one.
pub(super) const DEFAULT_SHEET: &str = "Sheet1";

const ROOT_RELS: &str = "_rels/.rels";
const CONTENT_TYPES: &str = "[Content_Types].xml";
const DEFAULT_WORKBOOK: &str = "xl/workbook.xml";
const OFFICE_DOCUMENT: &str = "/officeDocument";
const WORKSHEET: &str = "/worksheet";
const SHARED_STRINGS: &str = "/sharedStrings";
const CALC_CHAIN: &str = "/calcChain";
/// The workbook children that come after `calcPr` (ECMA-376 §18.2.27): a
/// `calcPr` the workbook lacks goes before the first of them.
const AFTER_CALC_PR: [&[u8]; 9] = [
    b"oleSize",
    b"customWorkbookViews",
    b"pivotCaches",
    b"smartTagPr",
    b"smartTagTypes",
    b"webPublishing",
    b"fileRecoveryPr",
    b"webPublishObjects",
    b"extLst",
];

type Archive<'a> = zip::ZipArchive<Cursor<&'a [u8]>>;

/// What a write left: the file's new bytes and the sheet it wrote.
pub(super) struct Written {
    pub bytes: Vec<u8>,
    pub sheet: String,
}

/// What a read found: the sheet read, every sheet's name, and its cells.
pub(super) struct Read {
    pub sheet: String,
    pub sheets: Vec<String>,
    pub cells: Vec<ReadCell>,
    pub truncated: bool,
}

/// The cells of one sheet (the first when none is named), within `range`.
///
/// # Errors
/// Not a workbook, no such sheet, or a part that does not read.
pub(super) fn read(
    bytes: &[u8],
    sheet: Option<&str>,
    range: Option<CellRange>,
) -> Result<Read, String> {
    let mut archive = open(bytes)?;
    let book = book(&mut archive)?;
    let entry = book.pick(sheet)?;
    let xml = part(&mut archive, &entry.part)?.ok_or_else(|| {
        format!(
            "the workbook names sheet `{}` but has no part for it",
            entry.name
        )
    })?;
    let strings = match &book.shared_strings {
        Some(name) => {
            part(&mut archive, name)?.map_or_else(|| Ok(Vec::new()), |xml| shared_strings(&xml))?
        }
        None => Vec::new(),
    };
    let parsed = SheetXml::parse(&xml)?;
    let mut cells = Vec::new();
    let mut truncated = false;
    for cell in parsed.rows.iter().flat_map(|row| &row.cells) {
        if range.is_some_and(|range| !range.contains(cell.at)) {
            continue;
        }
        let Some(read) = cell.read(&strings) else {
            continue;
        };
        if cells.len() == SHEET_MAX_CELLS {
            truncated = true;
            break;
        }
        cells.push(read);
    }
    Ok(Read {
        sheet: entry.name.clone(),
        sheets: book.sheets.iter().map(|sheet| sheet.name.clone()).collect(),
        cells,
        truncated,
    })
}

/// `cells` written into `existing` — or into a new workbook of one sheet when
/// there is no file yet.
///
/// # Errors
/// Not a workbook, no such sheet in an existing one, a cell that heads a
/// shared or array formula, or a sheet whose rows do not name their cells.
pub(super) fn write(
    existing: Option<&[u8]>,
    sheet: Option<&str>,
    cells: &[(CellAddress, SheetValue)],
) -> Result<Written, String> {
    let template;
    let source = match existing {
        Some(bytes) => bytes,
        None => {
            template = new_workbook(sheet.unwrap_or(DEFAULT_SHEET))?;
            template.as_slice()
        }
    };
    let mut archive = open(source)?;
    let book = book(&mut archive)?;
    let entry = book.pick(sheet)?;
    let mut replaced: BTreeMap<String, String> = BTreeMap::new();
    let sheet_xml = part(&mut archive, &entry.part)?.ok_or_else(|| {
        format!(
            "the workbook names sheet `{}` but has no part for it",
            entry.name
        )
    })?;
    let patched = patch_sheet(&sheet_xml, cells)?;
    replaced.insert(
        entry.part.clone(),
        drop_computed_values(&patched)?.unwrap_or(patched),
    );
    for other in book.sheets.iter().filter(|sheet| sheet.part != entry.part) {
        if let Some(xml) = part(&mut archive, &other.part)?
            && let Some(dropped) = drop_computed_values(&xml)?
        {
            replaced.insert(other.part.clone(), dropped);
        }
    }
    let workbook_xml = part(&mut archive, &book.workbook)?.ok_or("the workbook part is missing")?;
    replaced.insert(book.workbook.clone(), patch_workbook(&workbook_xml)?);
    if let Some(chain) = &book.calc_chain {
        let rels = part(&mut archive, &book.workbook_rels)?.unwrap_or_default();
        replaced.insert(
            book.workbook_rels.clone(),
            remove_empty_elements(&rels, |element| {
                attribute(element, b"Type")
                    .ok()
                    .flatten()
                    .is_some_and(|kind| kind.ends_with(CALC_CHAIN))
            })?,
        );
        if let Some(types) = part(&mut archive, CONTENT_TYPES)? {
            let named = format!("/{chain}");
            replaced.insert(
                CONTENT_TYPES.to_string(),
                remove_empty_elements(&types, |element| {
                    attribute(element, b"PartName").ok().flatten().as_deref()
                        == Some(named.as_str())
                })?,
            );
        }
    }

    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let deflated = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for index in 0..archive.len() {
        let file = archive.by_index_raw(index).map_err(zip_error)?;
        let name = file.name().to_string();
        if book.calc_chain.as_deref() == Some(name.as_str()) {
            continue;
        }
        if let Some(text) = replaced.get(&name) {
            drop(file);
            writer.start_file(name, deflated).map_err(zip_error)?;
            writer
                .write_all(text.as_bytes())
                .map_err(|error| error.to_string())?;
        } else {
            writer.raw_copy_file(file).map_err(zip_error)?;
        }
    }
    let bytes = writer.finish().map_err(zip_error)?.into_inner();
    Ok(Written {
        bytes,
        sheet: entry.name.clone(),
    })
}

fn zip_error(error: zip::result::ZipError) -> String {
    format!("the xlsx file does not read as a zip: {error}")
}

fn open(bytes: &[u8]) -> Result<Archive<'_>, String> {
    zip::ZipArchive::new(Cursor::new(bytes)).map_err(zip_error)
}

/// One part's text, or `None` when the workbook has no such part.
fn part(archive: &mut Archive<'_>, name: &str) -> Result<Option<String>, String> {
    let file = match archive.by_name(name) {
        Ok(file) => file,
        Err(zip::result::ZipError::FileNotFound) => return Ok(None),
        Err(error) => return Err(zip_error(error)),
    };
    let mut bytes = Vec::new();
    file.take(MAX_PART_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("{name} does not unpack: {error}"))?;
    if bytes.len() as u64 > MAX_PART_BYTES {
        return Err(format!("{name} unpacks past {MAX_PART_BYTES} bytes"));
    }
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|_| format!("{name} is not UTF-8 XML"))
}

/// A workbook's sheets in order, and where its other parts are.
struct Book {
    workbook: String,
    workbook_rels: String,
    sheets: Vec<SheetEntry>,
    shared_strings: Option<String>,
    calc_chain: Option<String>,
}

struct SheetEntry {
    name: String,
    part: String,
}

impl Book {
    /// The named sheet (its name in any case, as a workbook compares them),
    /// or the first.
    fn pick(&self, name: Option<&str>) -> Result<&SheetEntry, String> {
        let names = || {
            self.sheets
                .iter()
                .map(|sheet| format!("`{}`", sheet.name))
                .collect::<Vec<_>>()
                .join(", ")
        };
        match name {
            None => self
                .sheets
                .first()
                .ok_or_else(|| "the workbook has no sheet".to_string()),
            Some(name) => self
                .sheets
                .iter()
                .find(|sheet| sheet.name.to_lowercase() == name.to_lowercase())
                .ok_or_else(|| {
                    format!(
                        "the workbook has no sheet `{name}`; its sheets: {}",
                        names()
                    )
                }),
        }
    }
}

fn book(archive: &mut Archive<'_>) -> Result<Book, String> {
    let root =
        part(archive, ROOT_RELS)?.map_or_else(|| Ok(Vec::new()), |xml| relationships(&xml))?;
    let workbook = root
        .iter()
        .find(|rel| rel.kind.ends_with(OFFICE_DOCUMENT))
        .map_or_else(
            || DEFAULT_WORKBOOK.to_string(),
            |rel| resolve("", &rel.target),
        );
    let workbook_xml =
        part(archive, &workbook)?.ok_or("not an xlsx workbook: it has no workbook part")?;
    let (folder, file) = workbook.rsplit_once('/').unwrap_or(("", workbook.as_str()));
    let workbook_rels = if folder.is_empty() {
        format!("_rels/{file}.rels")
    } else {
        format!("{folder}/_rels/{file}.rels")
    };
    let rels =
        part(archive, &workbook_rels)?.map_or_else(|| Ok(Vec::new()), |xml| relationships(&xml))?;
    let target_of = |kind: &str| {
        rels.iter()
            .find(|rel| rel.kind.ends_with(kind))
            .map(|rel| resolve(folder, &rel.target))
    };
    let mut sheets = Vec::new();
    for element in elements(&workbook_xml, b"sheet")? {
        let name = attribute(&element, b"name")?.ok_or("a sheet in the workbook has no name")?;
        let id =
            prefixed_attribute(&element, b"id")?.ok_or("a sheet in the workbook has no part")?;
        let rel = rels
            .iter()
            .find(|rel| rel.id == id && rel.kind.ends_with(WORKSHEET));
        // A chart sheet or a dialog sheet has no cells: it is not offered.
        if let Some(rel) = rel {
            sheets.push(SheetEntry {
                name,
                part: resolve(folder, &rel.target),
            });
        }
    }
    Ok(Book {
        shared_strings: target_of(SHARED_STRINGS),
        calc_chain: target_of(CALC_CHAIN),
        workbook,
        workbook_rels,
        sheets,
    })
}

struct Relationship {
    id: String,
    kind: String,
    target: String,
}

fn relationships(xml: &str) -> Result<Vec<Relationship>, String> {
    elements(xml, b"Relationship")?
        .iter()
        .map(|element| {
            Ok(Relationship {
                id: attribute(element, b"Id")?.unwrap_or_default(),
                kind: attribute(element, b"Type")?.unwrap_or_default(),
                target: attribute(element, b"Target")?.unwrap_or_default(),
            })
        })
        .collect()
}

/// A relationship's target as a part name: from the package root when it
/// starts with `/`, else beside `folder`, with `..` folded.
fn resolve(folder: &str, target: &str) -> String {
    let joined = match target.strip_prefix('/') {
        Some(absolute) => absolute.to_string(),
        None if folder.is_empty() => target.to_string(),
        None => format!("{folder}/{target}"),
    };
    let mut parts: Vec<&str> = Vec::new();
    for piece in joined.split('/') {
        match piece {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            piece => parts.push(piece),
        }
    }
    parts.join("/")
}

/// Every element of `local` name in `xml`, start or empty, in order.
fn elements(xml: &str, local: &[u8]) -> Result<Vec<BytesStart<'static>>, String> {
    let mut found = Vec::new();
    for event in Events::new(xml) {
        let (_, _, event) = event?;
        if let Event::Start(element) | Event::Empty(element) = event
            && element.local_name().as_ref() == local
        {
            found.push(element.into_owned());
        }
    }
    Ok(found)
}

fn attribute(element: &BytesStart<'_>, key: &[u8]) -> Result<Option<String>, String> {
    find_attribute(element, |name| name == key)
}

/// An attribute in a namespace, by its local name — `r:id`, whatever the
/// prefix.
fn prefixed_attribute(element: &BytesStart<'_>, local: &[u8]) -> Result<Option<String>, String> {
    find_attribute(element, |name| {
        name.iter()
            .position(|byte| *byte == b':')
            .is_some_and(|colon| &name[colon + 1..] == local)
    })
}

fn find_attribute(
    element: &BytesStart<'_>,
    wanted: impl Fn(&[u8]) -> bool,
) -> Result<Option<String>, String> {
    for attribute in element.attributes().with_checks(false) {
        let attribute =
            attribute.map_err(|error| format!("an attribute does not read: {error}"))?;
        if wanted(attribute.key.as_ref()) {
            return attribute
                .normalized_value(XmlVersion::Implicit1_0)
                .map(|value| Some(value.into_owned()))
                .map_err(|error| format!("an attribute does not read: {error}"));
        }
    }
    Ok(None)
}

/// The XML events of a part, each with the byte span it was read from.
struct Events<'a> {
    reader: Reader<&'a [u8]>,
    done: bool,
}

impl<'a> Events<'a> {
    fn new(xml: &'a str) -> Self {
        Self {
            reader: Reader::from_str(xml),
            done: false,
        }
    }

    fn at(&self) -> usize {
        usize::try_from(self.reader.buffer_position()).unwrap_or(usize::MAX)
    }
}

impl<'a> Iterator for Events<'a> {
    type Item = Result<(usize, usize, Event<'a>), String>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        let start = self.at();
        match self.reader.read_event() {
            Ok(Event::Eof) => {
                self.done = true;
                None
            }
            Ok(event) => Some(Ok((start, self.at(), event))),
            Err(error) => {
                self.done = true;
                Some(Err(format!("the workbook's XML does not read: {error}")))
            }
        }
    }
}

/// The words an event adds to a text being read: its text, a CDATA block,
/// or a character or predefined entity.
fn push_text(out: &mut String, event: &Event<'_>) -> Result<(), String> {
    let unread = |error: &dyn std::fmt::Display| format!("a cell's text does not read: {error}");
    match event {
        Event::Text(text) => out.push_str(
            &text
                .xml_content(XmlVersion::Implicit1_0)
                .map_err(|e| unread(&e))?,
        ),
        Event::CData(data) => out.push_str(&data.decode().map_err(|e| unread(&e))?),
        Event::GeneralRef(reference) => {
            match reference.resolve_char_ref().map_err(|e| unread(&e))? {
                Some(c) => out.push(c),
                None => {
                    let name = reference.decode().map_err(|e| unread(&e))?;
                    out.push_str(
                        quick_xml::escape::resolve_predefined_entity(&name).ok_or_else(|| {
                            format!("a cell's text names an unknown entity `{name}`")
                        })?,
                    );
                }
            }
        }
        _ => {}
    }
    Ok(())
}

/// The shared strings, each item's text (its runs joined, its phonetic
/// reading left out).
fn shared_strings(xml: &str) -> Result<Vec<String>, String> {
    let mut strings = Vec::new();
    let mut item: Option<String> = None;
    let (mut in_text, mut phonetic) = (false, 0_usize);
    for event in Events::new(xml) {
        let (_, _, event) = event?;
        match &event {
            Event::Start(element) => match element.local_name().as_ref() {
                b"si" => item = Some(String::new()),
                b"rPh" => phonetic += 1,
                b"t" if phonetic == 0 => in_text = true,
                _ => {}
            },
            Event::Empty(element) if element.local_name().as_ref() == b"si" => {
                strings.push(String::new())
            }
            Event::End(element) => match element.local_name().as_ref() {
                b"si" => strings.push(item.take().unwrap_or_default()),
                b"rPh" => phonetic = phonetic.saturating_sub(1),
                b"t" => in_text = false,
                _ => {}
            },
            _ if in_text => {
                if let Some(text) = item.as_mut() {
                    push_text(text, &event)?;
                }
            }
            _ => {}
        }
    }
    Ok(strings)
}

/// One `<c>` element: where its bytes are, and what it holds.
#[derive(Debug)]
struct CellSpan {
    at: CellAddress,
    start: usize,
    end: usize,
    style: Option<String>,
    kind: Option<String>,
    value: Option<String>,
    formula: Option<String>,
    inline: Option<String>,
    /// It heads a shared or an array formula other cells read (`<f ref=…>`).
    heads_formula: bool,
    /// It has a formula, its own text or a share of another's.
    has_formula: bool,
    /// Where its `<v>` — a formula's last computed value — sits.
    value_span: Option<(usize, usize)>,
}

impl CellSpan {
    /// The cell as a read answers it, or `None` when it holds nothing (a
    /// styled blank).
    fn read(&self, strings: &[String]) -> Option<ReadCell> {
        let formula = self
            .formula
            .as_deref()
            .filter(|formula| !formula.is_empty())
            .map(|formula| format!("{FORMULA_MARK}{formula}"));
        let mut error = false;
        let value = match (self.kind.as_deref(), self.value.as_deref()) {
            (Some("inlineStr"), _) => self.inline.clone().map(serde_json::Value::String),
            (_, None) => None,
            (Some("s"), Some(index)) => index
                .trim()
                .parse::<usize>()
                .ok()
                .and_then(|index| strings.get(index))
                .map(|text| serde_json::json!(text)),
            (Some("b"), Some(flag)) => Some(serde_json::json!(flag.trim() == "1")),
            (Some("e"), Some(text)) => {
                error = true;
                Some(serde_json::json!(text))
            }
            (Some("str" | "d"), Some(text)) => Some(serde_json::json!(text)),
            (_, Some(number)) => Some(
                number
                    .trim()
                    .parse::<f64>()
                    .ok()
                    .filter(|n| n.is_finite())
                    .map_or_else(|| serde_json::json!(number), |n| serde_json::json!(n)),
            ),
        };
        if value.is_none() && formula.is_none() {
            return None;
        }
        Some(ReadCell {
            address: self.at,
            value: value.unwrap_or(serde_json::Value::Null),
            formula,
            error,
        })
    }
}

/// One `<row>` element.
#[derive(Debug)]
struct RowSpan {
    row: u32,
    start: usize,
    /// Just past its start tag (its end, for an empty `<row/>`).
    open_end: usize,
    /// Where `</row>` begins (its end, for an empty one).
    close_start: usize,
    end: usize,
    empty: bool,
    cells: Vec<CellSpan>,
}

/// A worksheet part read for where its rows and cells are.
#[derive(Debug, Default)]
struct SheetXml {
    /// The prefix its elements carry (`x:`), or none.
    prefix: String,
    /// `<sheetData>`'s start tag, or the whole `<sheetData/>`.
    data_open: (usize, usize),
    data_empty: bool,
    data_close_start: usize,
    rows: Vec<RowSpan>,
    dimension: Option<(usize, usize, String)>,
    /// A row or a cell that does not say where it is.
    unaddressed: bool,
}

impl SheetXml {
    fn parse(xml: &str) -> Result<Self, String> {
        let mut sheet = Self::default();
        let mut row: Option<RowSpan> = None;
        let mut cell: Option<CellSpan> = None;
        let mut reading: Option<&'static str> = None;
        let (mut in_inline, mut phonetic, mut in_data) = (false, 0_usize, false);
        let mut previous_row = 0;
        for event in Events::new(xml) {
            let (start, end, event) = event?;
            match &event {
                Event::Start(element) | Event::Empty(element) => {
                    let empty = matches!(event, Event::Empty(_));
                    match element.local_name().as_ref() {
                        b"dimension" => {
                            let range = attribute(element, b"ref")?.unwrap_or_default();
                            sheet.dimension = Some((start, end, range));
                        }
                        b"sheetData" => {
                            let name = element.name();
                            let qualified = String::from_utf8_lossy(name.as_ref()).into_owned();
                            sheet.prefix = qualified
                                .strip_suffix("sheetData")
                                .unwrap_or_default()
                                .to_string();
                            sheet.data_open = (start, end);
                            sheet.data_empty = empty;
                            sheet.data_close_start = end;
                            in_data = !empty;
                        }
                        b"row" if in_data => {
                            let number = match attribute(element, b"r")? {
                                Some(r) => r
                                    .trim()
                                    .parse::<u32>()
                                    .map_err(|_| format!("a row says it is row `{r}`"))?,
                                None => {
                                    sheet.unaddressed = true;
                                    previous_row + 1
                                }
                            };
                            previous_row = number;
                            let span = RowSpan {
                                row: number,
                                start,
                                open_end: end,
                                close_start: end,
                                end,
                                empty,
                                cells: Vec::new(),
                            };
                            if empty {
                                sheet.rows.push(span);
                            } else {
                                row = Some(span);
                            }
                        }
                        b"c" if row.is_some() => {
                            let Some(current) = row.as_ref() else {
                                continue;
                            };
                            let at = match attribute(element, b"r")? {
                                Some(r) => CellAddress::parse(&r)?,
                                None => {
                                    sheet.unaddressed = true;
                                    CellAddress {
                                        row: current.row,
                                        column: current.cells.last().map_or(1, |c| c.at.column + 1),
                                    }
                                }
                            };
                            let span = CellSpan {
                                at,
                                start,
                                end,
                                style: attribute(element, b"s")?,
                                kind: attribute(element, b"t")?,
                                value: None,
                                formula: None,
                                inline: None,
                                heads_formula: false,
                                has_formula: false,
                                value_span: None,
                            };
                            if empty {
                                if let Some(current) = row.as_mut() {
                                    current.cells.push(span);
                                }
                            } else {
                                cell = Some(span);
                            }
                        }
                        b"f" if cell.is_some() => {
                            if let Some(current) = cell.as_mut() {
                                current.has_formula = true;
                                current.heads_formula |= attribute(element, b"ref")?.is_some();
                                if !empty {
                                    current.formula = Some(String::new());
                                    reading = Some("f");
                                }
                            }
                        }
                        b"v" if cell.is_some() => {
                            if let Some(current) = cell.as_mut() {
                                current.value_span = Some((start, end));
                                if !empty {
                                    current.value = Some(String::new());
                                    reading = Some("v");
                                }
                            }
                        }
                        b"is" if cell.is_some() && !empty => {
                            in_inline = true;
                            if let Some(current) = cell.as_mut() {
                                current.inline = Some(String::new());
                            }
                        }
                        b"rPh" if in_inline && !empty => phonetic += 1,
                        b"t" if in_inline && phonetic == 0 && !empty => reading = Some("t"),
                        _ => {}
                    }
                }
                Event::End(element) => match element.local_name().as_ref() {
                    b"v" => {
                        reading = None;
                        if let Some((_, close)) = cell.as_mut().and_then(|c| c.value_span.as_mut())
                        {
                            *close = end;
                        }
                    }
                    b"f" | b"t" => reading = None,
                    b"rPh" => phonetic = phonetic.saturating_sub(1),
                    b"is" => in_inline = false,
                    b"c" => {
                        if let (Some(mut done), Some(current)) = (cell.take(), row.as_mut()) {
                            done.end = end;
                            current.cells.push(done);
                        }
                    }
                    b"row" => {
                        if let Some(mut done) = row.take() {
                            done.close_start = start;
                            done.end = end;
                            sheet.rows.push(done);
                        }
                    }
                    b"sheetData" => {
                        sheet.data_close_start = start;
                        in_data = false;
                    }
                    _ => {}
                },
                _ => {
                    if let (Some(field), Some(current)) = (reading, cell.as_mut()) {
                        let text = match field {
                            "f" => current.formula.as_mut(),
                            "v" => current.value.as_mut(),
                            _ => current.inline.as_mut(),
                        };
                        if let Some(text) = text {
                            push_text(text, &event)?;
                        }
                    }
                }
            }
        }
        if sheet.data_open == (0, 0) {
            return Err("the sheet has no cell data (`sheetData`)".into());
        }
        Ok(sheet)
    }
}

/// XML text as an element's content or attribute holds it; a carriage
/// return is written as a reference, which a reader keeps.
fn escaped(text: &str) -> String {
    quick_xml::escape::escape(text).replace('\r', "&#13;")
}

/// One cell's element: the value written, the cell's style kept.
fn cell_xml(prefix: &str, at: CellAddress, value: &SheetValue, style: Option<&str>) -> String {
    let name = at.name();
    let style = style
        .map(|s| format!(" s=\"{}\"", escaped(s)))
        .unwrap_or_default();
    let p = prefix;
    match value {
        SheetValue::Text(text) => format!(
            "<{p}c r=\"{name}\"{style} t=\"inlineStr\"><{p}is><{p}t xml:space=\"preserve\">{}</{p}t></{p}is></{p}c>",
            escaped(text)
        ),
        SheetValue::Number(number) => {
            format!(
                "<{p}c r=\"{name}\"{style}><{p}v>{}</{p}v></{p}c>",
                number_text(*number)
            )
        }
        SheetValue::Bool(flag) => format!(
            "<{p}c r=\"{name}\"{style} t=\"b\"><{p}v>{}</{p}v></{p}c>",
            u8::from(*flag)
        ),
        SheetValue::Formula(formula) => {
            format!(
                "<{p}c r=\"{name}\"{style}><{p}f>{}</{p}f></{p}c>",
                escaped(formula)
            )
        }
        SheetValue::Empty if !style.is_empty() => format!("<{p}c r=\"{name}\"{style}/>"),
        SheetValue::Empty => String::new(),
    }
}

/// A start tag again, with `drop` left out and as a start (never empty):
/// the attributes keep their own bytes.
fn start_tag_without(xml: &str, start: usize, end: usize, drop: &[u8]) -> Result<String, String> {
    let mut events = Events::new(&xml[start..end]);
    let Some(Ok((_, _, Event::Start(element) | Event::Empty(element)))) = events.next() else {
        return Err("a row's tag does not read".into());
    };
    let mut tag = format!("<{}", String::from_utf8_lossy(element.name().as_ref()));
    for attribute in element.attributes().with_checks(false) {
        let attribute =
            attribute.map_err(|error| format!("a row's attribute does not read: {error}"))?;
        if attribute.key.as_ref() == drop {
            continue;
        }
        tag.push_str(&format!(
            " {}=\"{}\"",
            String::from_utf8_lossy(attribute.key.as_ref()),
            String::from_utf8_lossy(&attribute.value).replace('"', "&quot;")
        ));
    }
    tag.push('>');
    Ok(tag)
}

/// The sheet's XML with `cells` written: each named cell replaced or put in
/// its place in its row, each missing row put in its place, the sheet's
/// stated extent grown to hold them. Nothing else in it moves.
fn patch_sheet(xml: &str, cells: &[(CellAddress, SheetValue)]) -> Result<String, String> {
    let sheet = SheetXml::parse(xml)?;
    if sheet.unaddressed {
        return Err(
            "this sheet's rows do not say which cells they hold; open and save the file in a spreadsheet app once, then write it"
                .into(),
        );
    }
    let p = sheet.prefix.as_str();
    let mut by_row: BTreeMap<u32, Vec<(CellAddress, &SheetValue)>> = BTreeMap::new();
    for (at, value) in cells {
        by_row.entry(at.row).or_default().push((*at, value));
    }
    let rows: BTreeMap<u32, &RowSpan> = sheet.rows.iter().map(|row| (row.row, row)).collect();
    // (start, end, text): a replacement of bytes, or an insertion where
    // start == end. A stable sort by (start, end) keeps insertions at one
    // place in the order made, before a replacement that starts there.
    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    let mut new_rows: Vec<(u32, String)> = Vec::new();
    for (number, written) in &by_row {
        let Some(row) = rows.get(number) else {
            let inner: String = written
                .iter()
                .map(|(at, value)| cell_xml(p, *at, value, None))
                .collect();
            if !inner.is_empty() {
                new_rows.push((*number, format!("<{p}row r=\"{number}\">{inner}</{p}row>")));
            }
            continue;
        };
        let mut inserted = Vec::new();
        for (at, value) in written {
            if let Some(cell) = row.cells.iter().find(|cell| cell.at == *at) {
                if cell.heads_formula {
                    return Err(format!(
                        "{} heads a formula other cells share; change it in the app",
                        at.name()
                    ));
                }
                edits.push((
                    cell.start,
                    cell.end,
                    cell_xml(p, *at, value, cell.style.as_deref()),
                ));
                continue;
            }
            let text = cell_xml(p, *at, value, None);
            if text.is_empty() {
                continue;
            }
            let place = row
                .cells
                .iter()
                .find(|cell| cell.at.column > at.column)
                .map_or(row.close_start, |cell| cell.start);
            inserted.push((place, text));
        }
        if inserted.is_empty() {
            continue;
        }
        // A row's `spans` is a hint of its first and last cell; one that a
        // new cell falls outside of is dropped rather than left wrong.
        let tag = start_tag_without(xml, row.start, row.open_end, b"spans")?;
        if row.empty {
            let inner: String = inserted.into_iter().map(|(_, text)| text).collect();
            edits.push((row.start, row.end, format!("{tag}{inner}</{p}row>")));
        } else {
            edits.push((row.start, row.open_end, tag));
            edits.extend(
                inserted
                    .into_iter()
                    .map(|(place, text)| (place, place, text)),
            );
        }
    }
    if sheet.data_empty {
        let inner: String = new_rows.into_iter().map(|(_, text)| text).collect();
        edits.push((
            sheet.data_open.0,
            sheet.data_open.1,
            format!("<{p}sheetData>{inner}</{p}sheetData>"),
        ));
    } else {
        for (number, text) in new_rows {
            let place = sheet
                .rows
                .iter()
                .find(|row| row.row > number)
                .map_or(sheet.data_close_start, |row| row.start);
            edits.push((place, place, text));
        }
    }
    if let Some((start, end, stated)) = &sheet.dimension {
        let written: Vec<CellAddress> = cells
            .iter()
            .filter(|(_, value)| !matches!(value, SheetValue::Empty))
            .map(|(at, _)| *at)
            .collect();
        if let Some(grown) = grown_extent(stated, &written) {
            edits.push((*start, *end, format!("<{p}dimension ref=\"{grown}\"/>")));
        }
    }
    apply(xml, edits)
}

/// The sheet without its formulas' last computed values, or `None` when it
/// keeps none. A value a written cell feeds would otherwise be shown as it
/// was: LibreOffice shows a workbook's stored results and computes only
/// what has none (11-10: a sum stayed 5 after one of its cells went from 3
/// to 4), and Excel computes everything anyway (`calcPr fullCalcOnLoad`).
/// The formulas and every other byte stay; a sheet with no formula is not
/// touched.
fn drop_computed_values(xml: &str) -> Result<Option<String>, String> {
    let sheet = SheetXml::parse(xml)?;
    let edits: Vec<(usize, usize, String)> = sheet
        .rows
        .iter()
        .flat_map(|row| &row.cells)
        .filter(|cell| cell.has_formula)
        .filter_map(|cell| cell.value_span)
        .map(|(start, end)| (start, end, String::new()))
        .collect();
    if edits.is_empty() {
        return Ok(None);
    }
    apply(xml, edits).map(Some)
}

/// The sheet's stated extent (`A1:C4`) grown to hold `written`, or `None`
/// when it already does.
fn grown_extent(stated: &str, written: &[CellAddress]) -> Option<String> {
    let first = *written.first()?;
    let stated = CellRange::parse(stated).ok();
    let mut grown = stated.unwrap_or(CellRange { first, last: first });
    for at in written {
        grown.first.row = grown.first.row.min(at.row);
        grown.first.column = grown.first.column.min(at.column);
        grown.last.row = grown.last.row.max(at.row);
        grown.last.column = grown.last.column.max(at.column);
    }
    (Some(grown) != stated).then(|| {
        if grown.first == grown.last {
            grown.first.name()
        } else {
            grown.name()
        }
    })
}

fn apply(xml: &str, mut edits: Vec<(usize, usize, String)>) -> Result<String, String> {
    edits.sort_by_key(|(start, end, _)| (*start, *end));
    let mut out =
        String::with_capacity(xml.len() + edits.iter().map(|(_, _, t)| t.len()).sum::<usize>());
    let mut at = 0;
    for (start, end, text) in edits {
        if start < at || end > xml.len() {
            return Err("two changes to the sheet overlap".into());
        }
        out.push_str(&xml[at..start]);
        out.push_str(&text);
        at = end;
    }
    out.push_str(&xml[at..]);
    Ok(out)
}

/// The workbook's XML set to compute every formula when an app opens it
/// (`calcPr fullCalcOnLoad`): a written formula carries no value yet, and a
/// value other formulas read may have changed.
fn patch_workbook(xml: &str) -> Result<String, String> {
    let mut depth = 0_usize;
    let mut prefix = String::new();
    let mut insert_at = None;
    for event in Events::new(xml) {
        let (start, end, event) = event?;
        match &event {
            Event::Start(element) | Event::Empty(element) => {
                let local = element.local_name();
                if depth == 0 {
                    let name = element.name();
                    let qualified = String::from_utf8_lossy(name.as_ref()).into_owned();
                    prefix = qualified
                        .strip_suffix("workbook")
                        .unwrap_or_default()
                        .to_string();
                } else if depth == 1 && local.as_ref() == b"calcPr" {
                    let kept = start_tag_without(xml, start, end, b"fullCalcOnLoad")?;
                    let kept = kept.strip_suffix('>').unwrap_or(&kept);
                    let close = if matches!(event, Event::Empty(_)) {
                        "/>"
                    } else {
                        ">"
                    };
                    return apply(
                        xml,
                        vec![(start, end, format!("{kept} fullCalcOnLoad=\"1\"{close}"))],
                    );
                } else if depth == 1
                    && insert_at.is_none()
                    && AFTER_CALC_PR.contains(&local.as_ref())
                {
                    insert_at = Some(start);
                }
                if matches!(event, Event::Start(_)) {
                    depth += 1;
                }
            }
            Event::End(_) => {
                depth = depth.saturating_sub(1);
                if depth == 0 && insert_at.is_none() {
                    insert_at = Some(start);
                }
            }
            _ => {}
        }
    }
    let place = insert_at.ok_or("the workbook part has no end")?;
    apply(
        xml,
        vec![(
            place,
            place,
            format!("<{prefix}calcPr fullCalcOnLoad=\"1\"/>"),
        )],
    )
}

/// `xml` without the empty elements `drop` picks.
fn remove_empty_elements(
    xml: &str,
    drop: impl Fn(&BytesStart<'_>) -> bool,
) -> Result<String, String> {
    let mut edits = Vec::new();
    for event in Events::new(xml) {
        let (start, end, event) = event?;
        if let Event::Empty(element) = &event
            && drop(element)
        {
            edits.push((start, end, String::new()));
        }
    }
    apply(xml, edits)
}

/// A workbook of one empty sheet named `sheet`, as a zip.
fn new_workbook(sheet: &str) -> Result<Vec<u8>, String> {
    const MAIN: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";
    const PACKAGE_RELS: &str = "http://schemas.openxmlformats.org/package/2006/relationships";
    const DOCUMENT_RELS: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
    const TYPE: &str = "application/vnd.openxmlformats-officedocument.spreadsheetml";
    const DECLARATION: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n";
    let parts = [
        (
            CONTENT_TYPES.to_string(),
            format!(
                "{DECLARATION}<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"><Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/><Default Extension=\"xml\" ContentType=\"application/xml\"/><Override PartName=\"/xl/workbook.xml\" ContentType=\"{TYPE}.sheet.main+xml\"/><Override PartName=\"/xl/worksheets/sheet1.xml\" ContentType=\"{TYPE}.worksheet+xml\"/><Override PartName=\"/xl/styles.xml\" ContentType=\"{TYPE}.styles+xml\"/></Types>"
            ),
        ),
        (
            ROOT_RELS.to_string(),
            format!(
                "{DECLARATION}<Relationships xmlns=\"{PACKAGE_RELS}\"><Relationship Id=\"rId1\" Type=\"{DOCUMENT_RELS}/officeDocument\" Target=\"xl/workbook.xml\"/></Relationships>"
            ),
        ),
        (
            DEFAULT_WORKBOOK.to_string(),
            format!(
                "{DECLARATION}<workbook xmlns=\"{MAIN}\" xmlns:r=\"{DOCUMENT_RELS}\"><sheets><sheet name=\"{}\" sheetId=\"1\" r:id=\"rId1\"/></sheets><calcPr fullCalcOnLoad=\"1\"/></workbook>",
                escaped(sheet)
            ),
        ),
        (
            "xl/_rels/workbook.xml.rels".to_string(),
            format!(
                "{DECLARATION}<Relationships xmlns=\"{PACKAGE_RELS}\"><Relationship Id=\"rId1\" Type=\"{DOCUMENT_RELS}/worksheet\" Target=\"worksheets/sheet1.xml\"/><Relationship Id=\"rId2\" Type=\"{DOCUMENT_RELS}/styles\" Target=\"styles.xml\"/></Relationships>"
            ),
        ),
        (
            "xl/styles.xml".to_string(),
            format!(
                "{DECLARATION}<styleSheet xmlns=\"{MAIN}\"><fonts count=\"1\"><font><sz val=\"11\"/><name val=\"Calibri\"/></font></fonts><fills count=\"2\"><fill><patternFill patternType=\"none\"/></fill><fill><patternFill patternType=\"gray125\"/></fill></fills><borders count=\"1\"><border><left/><right/><top/><bottom/><diagonal/></border></borders><cellStyleXfs count=\"1\"><xf numFmtId=\"0\" fontId=\"0\" fillId=\"0\" borderId=\"0\"/></cellStyleXfs><cellXfs count=\"1\"><xf numFmtId=\"0\" fontId=\"0\" fillId=\"0\" borderId=\"0\" xfId=\"0\"/></cellXfs><cellStyles count=\"1\"><cellStyle name=\"Normal\" xfId=\"0\" builtinId=\"0\"/></cellStyles></styleSheet>"
            ),
        ),
        (
            "xl/worksheets/sheet1.xml".to_string(),
            format!(
                "{DECLARATION}<worksheet xmlns=\"{MAIN}\"><dimension ref=\"A1\"/><sheetData/></worksheet>"
            ),
        ),
    ];
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let deflated = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for (name, text) in parts {
        writer.start_file(name, deflated).map_err(zip_error)?;
        writer
            .write_all(text.as_bytes())
            .map_err(|error| error.to_string())?;
    }
    Ok(writer.finish().map_err(zip_error)?.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use zerocode_core::computer_sheet::{parse_cells, verification};

    fn cells(raw: &str) -> Vec<(CellAddress, SheetValue)> {
        parse_cells(raw).expect("cells")
    }

    /// A workbook as LibreOffice and Excel write one: shared strings, a
    /// styled cell, a shared formula, two sheets, a cached formula order and
    /// a part this road knows nothing of.
    fn existing_workbook() -> Vec<u8> {
        let sheet1 = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><dimension ref="A1:C3"/><sheetViews><sheetView workbookViewId="0"/></sheetViews><sheetData><row r="1" spans="1:3"><c r="A1" t="s"><v>0</v></c><c r="B1" s="1" t="s"><v>1</v></c></row><row r="3" spans="1:3"><c r="A3"><f t="shared" ref="A3:B3" si="0">B1&amp;"x"</f><v>0</v></c><c r="B3"><f t="shared" si="0"/><v>0</v></c><c r="C3" s="1"><v>7</v></c></row></sheetData><mergeCells count="1"><mergeCell ref="A5:B5"/></mergeCells></worksheet>"#;
        let sheet2 = r#"<?xml version="1.0" encoding="UTF-8"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1"><v>42</v></c></row></sheetData></worksheet>"#;
        let strings = r#"<?xml version="1.0"?><sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" count="2" uniqueCount="2"><si><t>품목</t></si><si><r><t>Mixed </t></r><r><rPr><b/></rPr><t>&amp; bold</t></r><rPh><t>ふりがな</t></rPh></si></sst>"#;
        let workbook = r#"<?xml version="1.0"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="매출" sheetId="1" r:id="rId1"/><sheet name="Other" sheetId="2" r:id="rId2"/></sheets><extLst><ext uri="x"/></extLst></workbook>"#;
        let rels = r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="/xl/worksheets/sheet2.xml"/><Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/sharedStrings" Target="sharedStrings.xml"/><Relationship Id="rId4" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/calcChain" Target="calcChain.xml"/></Relationships>"#;
        let types = r#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Override PartName="/xl/calcChain.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.calcChain+xml"/><Override PartName="/xl/workbook.xml" ContentType="x"/></Types>"#;
        let root = r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#;
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for (name, text) in [
            (CONTENT_TYPES, types),
            (ROOT_RELS, root),
            ("xl/workbook.xml", workbook),
            ("xl/_rels/workbook.xml.rels", rels),
            ("xl/worksheets/sheet1.xml", sheet1),
            ("xl/worksheets/sheet2.xml", sheet2),
            ("xl/sharedStrings.xml", strings),
            ("xl/calcChain.xml", "<calcChain/>"),
            ("xl/media/unknown.bin", "\u{0}opaque bytes"),
        ] {
            writer.start_file(name, options).expect("start");
            writer.write_all(text.as_bytes()).expect("write");
        }
        writer.finish().expect("finish").into_inner()
    }

    fn part_text(bytes: &[u8], name: &str) -> Option<String> {
        part(&mut open(bytes).expect("zip"), name).expect("part")
    }

    #[test]
    fn a_new_workbook_holds_text_numbers_flags_and_formulas_as_written() {
        let written = cells(
            r#"{"A1":"품목","B1":"수량","C1":"단가","A2":"사과","B2":3,"C2":1200,"A3":"배","B3":2,"C3":2500,"A4":"합계 <&>","B4":"=SUM(B2:B3)","C4":"=SUMPRODUCT(B2:B3,C2:C3)","D1":true}"#,
        );
        let book = write(None, Some("표 1"), &written).expect("write");
        assert_eq!(book.sheet, "표 1");
        let read = read(&book.bytes, None, None).expect("read");
        assert_eq!(read.sheets, ["표 1"]);
        assert_eq!(
            verification(&written, &read.cells)["verified"],
            serde_json::json!(true),
            "{:?}",
            read.cells
        );
        let sheet = part_text(&book.bytes, "xl/worksheets/sheet1.xml").expect("sheet");
        assert!(
            sheet.contains(r#"<c r="B4"><f>SUM(B2:B3)</f></c>"#),
            "{sheet}"
        );
        assert!(sheet.contains(r#"<c r="C2"><v>1200</v></c>"#), "{sheet}");
        assert!(sheet.contains("합계 &lt;&amp;&gt;"), "{sheet}");
        assert!(sheet.contains(r#"<dimension ref="A1:D4"/>"#), "{sheet}");
        let workbook = part_text(&book.bytes, "xl/workbook.xml").expect("workbook");
        assert!(workbook.contains(r#"fullCalcOnLoad="1""#));
        // A range reads only its block.
        let block = read_cells(&book.bytes, "B2:C3");
        assert_eq!(block, ["B2", "C2", "B3", "C3"]);
    }

    fn read_cells(bytes: &[u8], range: &str) -> Vec<String> {
        read(bytes, None, Some(CellRange::parse(range).expect("range")))
            .expect("read")
            .cells
            .iter()
            .map(|cell| cell.address.name())
            .collect()
    }

    #[test]
    fn an_existing_workbook_keeps_every_other_cell_sheet_style_and_part() {
        let before = existing_workbook();
        let original = read(&before, None, None).expect("read");
        assert_eq!(original.sheet, "매출");
        assert_eq!(
            original.cells[1].value,
            serde_json::json!("Mixed & bold"),
            "runs joined, reading left out"
        );

        let written = cells(r#"{"B1":"새 이름","A2":5,"D3":"=C3*2","C3":null,"E1":"끝"}"#);
        let after = write(Some(&before), None, &written).expect("write");
        let read_after = read(&after.bytes, None, None).expect("read");
        assert_eq!(
            verification(&written, &read_after.cells)["verified"],
            serde_json::json!(true)
        );

        let sheet = part_text(&after.bytes, "xl/worksheets/sheet1.xml").expect("sheet");
        // The styled cell keeps its style; the emptied one keeps its format.
        assert!(
            sheet.contains(r#"<c r="B1" s="1" t="inlineStr">"#),
            "{sheet}"
        );
        assert!(sheet.contains(r#"<c r="C3" s="1"/>"#), "{sheet}");
        // Cells never named keep their bytes, and so does everything after
        // the cells.
        assert!(sheet.contains(r#"<c r="A1" t="s"><v>0</v></c>"#), "{sheet}");
        // Formulas keep their words and lose their stale results.
        assert!(sheet.contains(r#"<c r="A3"><f t="shared" ref="A3:B3" si="0">B1&amp;"x"</f></c><c r="B3"><f t="shared" si="0"/></c>"#), "{sheet}");
        assert!(sheet.contains(r#"<mergeCells count="1"><mergeCell ref="A5:B5"/></mergeCells>"#));
        // New cells and rows stand in sheet order; spans that no longer hold go.
        let order: Vec<usize> = [
            "<row r=\"1\">",
            "r=\"E1\"",
            "<row r=\"2\">",
            "r=\"A2\"",
            "<row r=\"3\"",
            "r=\"C3\"",
            "r=\"D3\"",
        ]
        .iter()
        .map(|needle| {
            sheet
                .find(needle)
                .unwrap_or_else(|| panic!("{needle} in {sheet}"))
        })
        .collect();
        assert!(
            order.windows(2).all(|pair| pair[0] < pair[1]),
            "{order:?} {sheet}"
        );
        assert!(sheet.contains(r#"<dimension ref="A1:E3"/>"#), "{sheet}");

        // Every other part is the same bytes; the formula order is gone.
        for name in [
            "xl/worksheets/sheet2.xml",
            "xl/sharedStrings.xml",
            "xl/media/unknown.bin",
        ] {
            assert_eq!(
                part_text(&after.bytes, name),
                part_text(&before, name),
                "{name}"
            );
        }
        assert_eq!(part_text(&after.bytes, "xl/calcChain.xml"), None);
        let rels = part_text(&after.bytes, "xl/_rels/workbook.xml.rels").expect("rels");
        assert!(
            !rels.contains("calcChain") && rels.contains("sharedStrings"),
            "{rels}"
        );
        let types = part_text(&after.bytes, CONTENT_TYPES).expect("types");
        assert!(
            !types.contains("calcChain") && types.contains("/xl/workbook.xml"),
            "{types}"
        );
        let workbook = part_text(&after.bytes, "xl/workbook.xml").expect("workbook");
        assert!(
            workbook.contains(r#"<calcPr fullCalcOnLoad="1"/><extLst>"#),
            "before extLst: {workbook}"
        );

        // The other sheet is written by its name, in any case, and alone.
        let other = write(Some(&after.bytes), Some("other"), &cells(r#"{"B1":1}"#)).expect("other");
        assert_eq!(other.sheet, "Other");
        assert_eq!(
            part_text(&other.bytes, "xl/worksheets/sheet1.xml"),
            part_text(&after.bytes, "xl/worksheets/sheet1.xml")
        );
        let missing = write(Some(&before), Some("없음"), &written)
            .err()
            .expect("no such sheet");
        assert!(missing.contains("`매출`, `Other`"), "{missing}");
    }

    #[test]
    fn a_cell_heading_a_shared_formula_and_an_unaddressed_sheet_are_refused() {
        let before = existing_workbook();
        let refused = write(Some(&before), None, &cells(r#"{"A3":1}"#))
            .err()
            .expect("heads");
        assert!(refused.contains("A3"), "{refused}");
        let unaddressed =
            "<worksheet><sheetData><row><c><v>1</v></c></row></sheetData></worksheet>";
        assert!(
            patch_sheet(unaddressed, &cells(r#"{"A1":2}"#))
                .unwrap_err()
                .contains("save the file")
        );
        assert!(write(Some(b"not a zip"), None, &cells(r#"{"A1":2}"#)).is_err());
    }

    #[test]
    fn a_prefixed_sheet_gets_prefixed_cells_and_an_empty_row_opens() {
        let xml = r#"<x:worksheet xmlns:x="m"><x:sheetData><x:row r="2" ht="20"/></x:sheetData></x:worksheet>"#;
        let patched = patch_sheet(xml, &cells(r#"{"B2":1,"A4":"t"}"#)).expect("patch");
        assert_eq!(
            patched,
            r#"<x:worksheet xmlns:x="m"><x:sheetData><x:row r="2" ht="20"><x:c r="B2"><x:v>1</x:v></x:c></x:row><x:row r="4"><x:c r="A4" t="inlineStr"><x:is><x:t xml:space="preserve">t</x:t></x:is></x:c></x:row></x:sheetData></x:worksheet>"#
        );
        let wb = patch_workbook(
            r#"<x:workbook xmlns:x="m"><x:calcPr calcId="1" fullCalcOnLoad="0"/></x:workbook>"#,
        )
        .expect("wb");
        assert_eq!(
            wb,
            r#"<x:workbook xmlns:x="m"><x:calcPr calcId="1" fullCalcOnLoad="1"/></x:workbook>"#
        );
    }

    #[test]
    fn relationship_targets_resolve_from_their_folder() {
        assert_eq!(
            resolve("xl", "worksheets/sheet1.xml"),
            "xl/worksheets/sheet1.xml"
        );
        assert_eq!(resolve("xl", "/xl/worksheets/a.xml"), "xl/worksheets/a.xml");
        assert_eq!(resolve("xl/sub", "../styles.xml"), "xl/styles.xml");
        assert_eq!(resolve("", "xl/workbook.xml"), "xl/workbook.xml");
    }
}
