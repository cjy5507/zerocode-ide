//! A spreadsheet's cells without the person's pointer, keys or front app.
//!
//! Background keys cannot write a table (t-101933, 11-10): LibreOffice Calc
//! took 0 of 12 cells and Numbers 6 of 12 (Hangul arrived as loose jamo, a
//! formula naming cells could not be typed). A file written directly took
//! 12 of 12 in 1.8 s, and Numbers' own script 12 of 12 in 1.7 s while the
//! front app never changed. `sheet-write` and `sheet-read` are those two
//! roads: the file (`.xlsx`, `.csv`) or an open Numbers or Excel document
//! through the app's script.
//!
//! This module is the pure part both roads share — cell addresses, the cells
//! JSON, the path rule, the CSV text and the script's arguments. Values reach
//! a script only as `osascript` arguments, never spliced into its text.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use serde_json::{Map, Value, json};

/// The flag a `sheet-write` names its cells with; a step log keeps each
/// value's length, never the value.
pub const SHEET_CELLS_FLAG: &str = "--cells";
/// The most cells one `sheet-write` names, and one `sheet-read` answers.
pub const SHEET_MAX_CELLS: usize = 10_000;
/// The longest text one cell holds — Excel's own limit.
pub const SHEET_MAX_TEXT_CHARS: usize = 32_767;
/// The longest formula one cell holds — Excel's own limit.
pub const SHEET_MAX_FORMULA_CHARS: usize = 8_192;
/// The most argument bytes one script call carries; past it the file road
/// writes the table instead.
pub const SHEET_MAX_SCRIPT_ARG_BYTES: usize = 256 * 1024;
/// The last column a sheet has (`XFD`) and its last row.
pub const SHEET_MAX_COLUMN: u32 = 16_384;
pub const SHEET_MAX_ROW: u32 = 1_048_576;
/// The longest sheet name a workbook keeps.
pub const SHEET_NAME_MAX_CHARS: usize = 31;
/// What a text cell starts with to stay text although it reads like a
/// formula: `'=1+1` is the text `=1+1`, as a person types it in a cell.
pub const TEXT_MARK: char = '\'';
/// The first character of a formula.
pub const FORMULA_MARK: char = '=';
/// Characters a CSV reader may run as a formula at the start of a field. A
/// text that starts with one is written behind [`TEXT_MARK`], so opening the
/// file never runs what was meant as words.
const CSV_FORMULA_STARTS: [char; 4] = ['=', '+', '-', '@'];

/// One cell's place: column and row, both counted from 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CellAddress {
    /// Rows first, so a sorted list walks a sheet the way its XML is laid out.
    pub row: u32,
    pub column: u32,
}

impl CellAddress {
    /// `A1`, `xfd1048576`, `$B$2`: letters, then digits.
    ///
    /// # Errors
    /// Anything else, or a place past the last column or row.
    pub fn parse(word: &str) -> Result<Self, String> {
        let plain: String = word.trim().chars().filter(|c| *c != '$').collect();
        let split = plain
            .find(|c: char| c.is_ascii_digit())
            .ok_or_else(|| format!("`{word}` is not a cell address like A1"))?;
        let (letters, digits) = plain.split_at(split);
        if letters.is_empty()
            || !letters.chars().all(|c| c.is_ascii_alphabetic())
            || !digits.chars().all(|c| c.is_ascii_digit())
            || digits.starts_with('0')
        {
            return Err(format!("`{word}` is not a cell address like A1"));
        }
        let mut column: u32 = 0;
        for letter in letters.chars() {
            let digit = u32::from(letter.to_ascii_uppercase()) - u32::from('A') + 1;
            column = column
                .checked_mul(26)
                .and_then(|c| c.checked_add(digit))
                .filter(|c| *c <= SHEET_MAX_COLUMN)
                .ok_or_else(|| format!("`{word}` is past the last column (XFD)"))?;
        }
        let row: u32 = digits
            .parse()
            .ok()
            .filter(|row| *row <= SHEET_MAX_ROW)
            .ok_or_else(|| format!("`{word}` is past the last row ({SHEET_MAX_ROW})"))?;
        Ok(Self { row, column })
    }

    /// A column's letters: 1 is `A`, 27 is `AA`.
    #[must_use]
    pub fn column_letters(column: u32) -> String {
        let mut letters = Vec::new();
        let mut left = column;
        while left > 0 {
            let digit = (left - 1) % 26;
            letters.push(char::from(b'A' + u8::try_from(digit).unwrap_or(0)));
            left = (left - 1) / 26;
        }
        letters.iter().rev().collect()
    }

    #[must_use]
    pub fn name(self) -> String {
        format!("{}{}", Self::column_letters(self.column), self.row)
    }
}

/// A block of cells, both corners inside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellRange {
    pub first: CellAddress,
    pub last: CellAddress,
}

impl CellRange {
    /// `A1:C4`, either corner first, or one cell `B2`.
    ///
    /// # Errors
    /// A corner that is not a cell address.
    pub fn parse(word: &str) -> Result<Self, String> {
        let (from, to) = word.split_once(':').unwrap_or((word, word));
        let (a, b) = (CellAddress::parse(from)?, CellAddress::parse(to)?);
        Ok(Self {
            first: CellAddress {
                row: a.row.min(b.row),
                column: a.column.min(b.column),
            },
            last: CellAddress {
                row: a.row.max(b.row),
                column: a.column.max(b.column),
            },
        })
    }

    #[must_use]
    pub fn contains(self, cell: CellAddress) -> bool {
        (self.first.row..=self.last.row).contains(&cell.row)
            && (self.first.column..=self.last.column).contains(&cell.column)
    }

    #[must_use]
    pub fn name(self) -> String {
        format!("{}:{}", self.first.name(), self.last.name())
    }
}

/// What one cell is asked to hold.
#[derive(Debug, Clone, PartialEq)]
pub enum SheetValue {
    Text(String),
    Number(f64),
    Bool(bool),
    /// The formula without its leading `=`.
    Formula(String),
    /// `null`: the cell's value goes; its format stays.
    Empty,
}

impl SheetValue {
    /// A JSON value as a cell: a string starting with `=` is a formula, one
    /// starting with `'=` the text after the mark, a number a number.
    ///
    /// # Errors
    /// An array or object, a text or formula past its limit, or a character
    /// a sheet's XML cannot hold.
    pub fn from_json(value: &Value) -> Result<Self, String> {
        match value {
            Value::Null => Ok(Self::Empty),
            Value::Bool(flag) => Ok(Self::Bool(*flag)),
            Value::Number(number) => number
                .as_f64()
                .filter(|n| n.is_finite())
                .map(Self::Number)
                .ok_or_else(|| format!("{number} is not a number a cell holds")),
            Value::String(text) => {
                if let Some(bad) = text.chars().find(|c| !xml_char(*c)) {
                    return Err(format!(
                        "a cell cannot hold the control character U+{:04X}",
                        u32::from(bad)
                    ));
                }
                if let Some(formula) = text.strip_prefix(FORMULA_MARK) {
                    if formula.trim().is_empty() {
                        return Err("a formula needs words after its `=`".into());
                    }
                    if text.chars().count() > SHEET_MAX_FORMULA_CHARS {
                        return Err(format!(
                            "a formula holds at most {SHEET_MAX_FORMULA_CHARS} characters"
                        ));
                    }
                    return Ok(Self::Formula(formula.to_string()));
                }
                let text = text
                    .strip_prefix(TEXT_MARK)
                    .filter(|rest| rest.starts_with(FORMULA_MARK))
                    .unwrap_or(text);
                if text.chars().count() > SHEET_MAX_TEXT_CHARS {
                    return Err(format!(
                        "a cell holds at most {SHEET_MAX_TEXT_CHARS} characters"
                    ));
                }
                Ok(Self::Text(text.to_string()))
            }
            Value::Array(_) | Value::Object(_) => {
                Err("a cell holds a string, a number, true/false or null".into())
            }
        }
    }

    /// How the answer says the value: a formula keeps its `=`.
    #[must_use]
    pub fn to_json(&self) -> Value {
        match self {
            // A text that reads like a formula keeps its mark, so it reads
            // back as text wherever this JSON is parsed again.
            Self::Text(text) if text.starts_with(FORMULA_MARK) => {
                json!(format!("{TEXT_MARK}{text}"))
            }
            Self::Text(text) => json!(text),
            Self::Number(number) => json!(number),
            Self::Bool(flag) => json!(flag),
            Self::Formula(formula) => json!(format!("{FORMULA_MARK}{formula}")),
            Self::Empty => Value::Null,
        }
    }
}

/// Whether XML 1.0 can hold the character — a sheet file's own limit.
#[must_use]
pub fn xml_char(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\r') || (c >= ' ' && c != '\u{FFFE}' && c != '\u{FFFF}')
}

/// A number as a cell's text: the shortest form that reads back the same,
/// `3` rather than `3.0`.
#[must_use]
pub fn number_text(number: f64) -> String {
    format!("{number}")
}

/// `--cells '{"A1":"item","B2":3,"C4":"=SUM(C2:C3)"}'` as cells in sheet
/// order.
///
/// # Errors
/// Not a JSON object, no cells, too many, an address that is not one, two
/// keys naming one cell (`a1` and `A1`), or a value no cell holds.
pub fn parse_cells(raw: &str) -> Result<Vec<(CellAddress, SheetValue)>, String> {
    let value: Value =
        serde_json::from_str(raw).map_err(|error| format!("--cells is not JSON: {error}"))?;
    cells_from_value(&value)
}

/// [`parse_cells`] past the JSON text: the object the parse kept in params.
///
/// # Errors
/// As [`parse_cells`].
pub fn cells_from_value(value: &Value) -> Result<Vec<(CellAddress, SheetValue)>, String> {
    let object = value
        .as_object()
        .ok_or("--cells is a JSON object of cell to value, like {\"A1\":\"item\",\"B2\":3}")?;
    if object.is_empty() {
        return Err("--cells names no cell".into());
    }
    if object.len() > SHEET_MAX_CELLS {
        return Err(format!(
            "--cells names {} cells; one call writes at most {SHEET_MAX_CELLS}",
            object.len()
        ));
    }
    let mut cells = BTreeMap::new();
    for (key, value) in object {
        let address = CellAddress::parse(key)?;
        let value = SheetValue::from_json(value).map_err(|error| format!("{key}: {error}"))?;
        if cells.insert(address, value).is_some() {
            return Err(format!("--cells names {} twice", address.name()));
        }
    }
    Ok(cells.into_iter().collect())
}

/// The cells as params keep them: canonical addresses, the caller's values.
#[must_use]
pub fn cells_to_value(cells: &[(CellAddress, SheetValue)]) -> Value {
    Value::Object(
        cells
            .iter()
            .map(|(address, value)| (address.name(), value.to_json()))
            .collect::<Map<_, _>>(),
    )
}

/// A sheet name a workbook keeps: 1 to 31 characters, none of `[]:*?/\`.
///
/// # Errors
/// Any other name.
pub fn check_sheet_name(name: &str) -> Result<(), String> {
    let count = name.chars().count();
    if count == 0 || count > SHEET_NAME_MAX_CHARS {
        return Err(format!(
            "a sheet name is 1 to {SHEET_NAME_MAX_CHARS} characters"
        ));
    }
    if let Some(bad) = name
        .chars()
        .find(|c| matches!(c, '[' | ']' | ':' | '*' | '?' | '/' | '\\') || !xml_char(*c))
    {
        return Err(format!("a sheet name cannot hold `{bad}`"));
    }
    if name.starts_with('\'') || name.ends_with('\'') {
        return Err("a sheet name cannot start or end with `'`".into());
    }
    Ok(())
}

/// The file kinds the file road writes and reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SheetFileKind {
    Xlsx,
    Csv,
}

impl SheetFileKind {
    /// By the file's extension, in any case.
    #[must_use]
    pub fn of(path: &Path) -> Option<Self> {
        let extension = path.extension()?.to_str()?.to_ascii_lowercase();
        match extension.as_str() {
            "xlsx" => Some(Self::Xlsx),
            "csv" => Some(Self::Csv),
            _ => None,
        }
    }
}

/// The folders a sheet file may be in: the person's home, and their own
/// temporary folder. Everything else — the system's folders, another
/// person's, a shared `/tmp` — is refused before anything is read.
#[derive(Debug, Clone)]
pub struct SheetRoots {
    pub home: PathBuf,
    pub temp: Option<PathBuf>,
}

/// The file a `--path` names: taken from `cwd` when relative (the folder the
/// asking shell stands in), with no `..`, ending `.xlsx` or `.csv`, and
/// inside [`SheetRoots`] once every link in its folders is followed. A file
/// that is itself a link is refused: a write would land where it points.
///
/// # Errors
/// Each of those, said in words.
pub fn sheet_path(
    asked: &str,
    cwd: Option<&Path>,
    roots: &SheetRoots,
) -> Result<(PathBuf, SheetFileKind), String> {
    let asked_path = Path::new(asked.trim());
    if asked.trim().is_empty() {
        return Err("--path names no file".into());
    }
    if asked_path
        .components()
        .any(|part| matches!(part, Component::ParentDir))
    {
        return Err("--path may not climb with `..`".into());
    }
    let path = if asked_path.is_absolute() {
        asked_path.to_path_buf()
    } else {
        cwd.ok_or("--path is relative and the caller's folder is unknown; give an absolute path")?
            .join(asked_path)
    };
    let kind = SheetFileKind::of(&path)
        .ok_or("--path must end in .xlsx or .csv (a .numbers or .xls file goes through --app)")?;
    let name = path.file_name().ok_or("--path names no file")?;
    if std::fs::symlink_metadata(&path).is_ok_and(|meta| meta.file_type().is_symlink()) {
        return Err(format!(
            "{} is a link; name the file itself",
            path.display()
        ));
    }
    let folder = path.parent().ok_or("--path names no folder")?;
    let real_folder = existing_ancestor(folder)?;
    let inside = |root: &Path| {
        std::fs::canonicalize(root)
            .ok()
            .is_some_and(|root| real_folder.starts_with(&root))
    };
    if !(inside(roots.home.as_path()) || roots.temp.as_deref().is_some_and(inside)) {
        return Err(format!(
            "{} is outside your home folder; a sheet file is written and read only there",
            path.display()
        ));
    }
    Ok((folder.join(name), kind))
}

/// The real place of `folder`'s nearest existing ancestor, with what is
/// still to be made below it.
fn existing_ancestor(folder: &Path) -> Result<PathBuf, String> {
    let mut missing = Vec::new();
    let mut at = folder;
    loop {
        if let Ok(real) = std::fs::canonicalize(at) {
            return Ok(missing
                .iter()
                .rev()
                .fold(real, |path, part| path.join(part)));
        }
        missing.push(at.file_name().ok_or("--path names no folder that exists")?);
        at = at.parent().ok_or("--path names no folder that exists")?;
    }
}

/// The apps whose open document the app road writes through their script.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SheetApp {
    Numbers,
    Excel,
}

impl SheetApp {
    /// `Numbers` or `Excel`, in any case, or the app's full name or bundle.
    ///
    /// # Errors
    /// Any other app.
    pub fn parse(word: &str) -> Result<Self, String> {
        match word.trim().to_ascii_lowercase().as_str() {
            "numbers" | "com.apple.numbers" => Ok(Self::Numbers),
            "excel" | "microsoft excel" | "com.microsoft.excel" => Ok(Self::Excel),
            _ => Err(format!(
                "--app for a sheet is Numbers or Excel, not `{word}`; any other table goes through --path"
            )),
        }
    }

    /// The name the share and a person know the app by.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Numbers => "Numbers",
            Self::Excel => "Microsoft Excel",
        }
    }

    #[must_use]
    pub const fn bundle_id(self) -> &'static str {
        match self {
            Self::Numbers => "com.apple.Numbers",
            Self::Excel => "com.microsoft.Excel",
        }
    }

    const fn write_script(self) -> &'static str {
        match self {
            Self::Numbers => NUMBERS_WRITE,
            Self::Excel => EXCEL_WRITE,
        }
    }

    const fn read_script(self) -> &'static str {
        match self {
            Self::Numbers => NUMBERS_READ,
            Self::Excel => EXCEL_READ,
        }
    }
}

/// The first script argument: a plain word, so `osascript` stops reading
/// its own options there and a value like `-5` stays a value.
pub const SCRIPT_ARGV_LEAD: &str = "zerocode-sheet";

/// The errors a sheet script raises on purpose, by number.
pub const SCRIPT_NOT_RUNNING: i64 = 4001;
pub const SCRIPT_NO_DOCUMENT: i64 = 4002;
pub const SCRIPT_NO_SHEET: i64 = 4003;
pub const SCRIPT_NO_TABLE: i64 = 4004;
/// AppleScript's own "can't get" for an app that is not installed.
pub const SCRIPT_NO_SUCH_APP: i64 = -1728;

/// Field and record separators in a read script's answer: ASCII's unit
/// and record separators, which no cell value [`xml_char`] admits.
pub const SCRIPT_FIELD: char = '\u{1f}';
pub const SCRIPT_RECORD: char = '\u{1e}';

// Every script reads its words from `argv` alone. Item 1 is
// SCRIPT_ARGV_LEAD; then the document, the sheet ("" for the first) and the
// table ("" for the first). It checks the app is running before it names the
// app's terms, so a script never starts an app, and it never activates one.
const NUMBERS_WRITE: &str = r#"on run argv
	if not (application id "com.apple.Numbers" is running) then error "Numbers is not running" number 4001
	set zcDocName to item 2 of argv
	set zcSheetName to item 3 of argv
	set zcTableName to item 4 of argv
	set zcNeedRows to (item 5 of argv) as integer
	set zcNeedColumns to (item 6 of argv) as integer
	tell application id "com.apple.Numbers"
		if not (exists document zcDocName) then error "no open document" number 4002
		set zcTheDoc to document zcDocName
		if zcSheetName is "" then
			set zcTheSheet to sheet 1 of zcTheDoc
		else
			if not (exists sheet zcSheetName of zcTheDoc) then error "no such sheet" number 4003
			set zcTheSheet to sheet zcSheetName of zcTheDoc
		end if
		if zcTableName is "" then
			if (count of tables of zcTheSheet) is 0 then error "the sheet has no table" number 4004
			set zcTheTable to table 1 of zcTheSheet
		else
			if not (exists table zcTableName of zcTheSheet) then error "no such table" number 4004
			set zcTheTable to table zcTableName of zcTheSheet
		end if
		if (row count of zcTheTable) < zcNeedRows then set row count of zcTheTable to zcNeedRows
		if (column count of zcTheTable) < zcNeedColumns then set column count of zcTheTable to zcNeedColumns
		repeat with i from 7 to (count of argv) by 3
			set zcTheCell to cell (item i of argv) of zcTheTable
			set zcKind to item (i + 1) of argv
			set zcRaw to item (i + 2) of argv
			if zcKind is "e" then
				clear zcTheCell
			else if zcKind is "n" then
				set value of zcTheCell to (zcRaw as number)
			else if zcKind is "b" then
				set value of zcTheCell to (zcRaw is "true")
			else
				set value of zcTheCell to zcRaw
			end if
		end repeat
	end tell
	return ""
end run"#;

const NUMBERS_READ: &str = r#"on run argv
	if not (application id "com.apple.Numbers" is running) then error "Numbers is not running" number 4001
	set zcDocName to item 2 of argv
	set zcSheetName to item 3 of argv
	set zcTableName to item 4 of argv
	set zcRangeName to item 5 of argv
	tell application id "com.apple.Numbers"
		if not (exists document zcDocName) then error "no open document" number 4002
		set zcTheDoc to document zcDocName
		if zcSheetName is "" then
			set zcTheSheet to sheet 1 of zcTheDoc
		else
			if not (exists sheet zcSheetName of zcTheDoc) then error "no such sheet" number 4003
			set zcTheSheet to sheet zcSheetName of zcTheDoc
		end if
		if zcTableName is "" then
			if (count of tables of zcTheSheet) is 0 then error "the sheet has no table" number 4004
			set zcTheTable to table 1 of zcTheSheet
		else
			if not (exists table zcTableName of zcTheSheet) then error "no such table" number 4004
			set zcTheTable to table zcTableName of zcTheSheet
		end if
		if zcRangeName is "" then
			set zcTheRange to cell range of zcTheTable
		else
			set zcTheRange to range zcRangeName of zcTheTable
		end if
		set zcNames to name of every cell of zcTheRange
		set zcValues to value of every cell of zcTheRange
		set zcFormulas to formula of every cell of zcTheRange
	end tell
	return my zcRecords(zcNames, zcValues, zcFormulas)
end run

on zcRecords(zcNames, zcValues, zcFormulas)
	set zcOut to {}
	repeat with i from 1 to (count of zcNames)
		set v to item i of zcValues
		set f to item i of zcFormulas
		if v is not missing value or f is not missing value then
			if v is missing value then
				set zcShown to "e"
			else if class of v is in {real, integer} then
				set zcShown to "n" & (v as text)
			else if class of v is boolean then
				set zcShown to "b" & (v as text)
			else
				set zcShown to "s" & (v as text)
			end if
			if f is missing value then set f to ""
			set end of zcOut to (item i of zcNames) & (character id 31) & zcShown & (character id 31) & f
		end if
	end repeat
	set AppleScript's text item delimiters to (character id 30)
	set zcAnswer to zcOut as text
	set AppleScript's text item delimiters to ""
	return zcAnswer
end zcRecords"#;

const EXCEL_WRITE: &str = r#"on run argv
	if not (application id "com.microsoft.Excel" is running) then error "Microsoft Excel is not running" number 4001
	set zcDocName to item 2 of argv
	set zcSheetName to item 3 of argv
	tell application id "com.microsoft.Excel"
		if not (exists workbook zcDocName) then error "no open workbook" number 4002
		set zcTheBook to workbook zcDocName
		if zcSheetName is "" then
			set zcTheSheet to worksheet 1 of zcTheBook
		else
			if not (exists worksheet zcSheetName of zcTheBook) then error "no such sheet" number 4003
			set zcTheSheet to worksheet zcSheetName of zcTheBook
		end if
		repeat with i from 7 to (count of argv) by 3
			set zcTheCell to range (item i of argv) of zcTheSheet
			set zcKind to item (i + 1) of argv
			set zcRaw to item (i + 2) of argv
			if zcKind is "e" then
				clear contents zcTheCell
			else if zcKind is "n" then
				set value of zcTheCell to (zcRaw as number)
			else if zcKind is "b" then
				set value of zcTheCell to (zcRaw is "true")
			else if zcKind is "f" then
				set formula of zcTheCell to zcRaw
			else
				set formula of zcTheCell to "'" & zcRaw
			end if
		end repeat
	end tell
	return ""
end run"#;

const EXCEL_READ: &str = r#"on run argv
	if not (application id "com.microsoft.Excel" is running) then error "Microsoft Excel is not running" number 4001
	set zcDocName to item 2 of argv
	set zcSheetName to item 3 of argv
	set zcRangeName to item 5 of argv
	tell application id "com.microsoft.Excel"
		if not (exists workbook zcDocName) then error "no open workbook" number 4002
		set zcTheBook to workbook zcDocName
		if zcSheetName is "" then
			set zcTheSheet to worksheet 1 of zcTheBook
		else
			if not (exists worksheet zcSheetName of zcTheBook) then error "no such sheet" number 4003
			set zcTheSheet to worksheet zcSheetName of zcTheBook
		end if
		if zcRangeName is "" then
			set zcTheRange to used range of zcTheSheet
		else
			set zcTheRange to range zcRangeName of zcTheSheet
		end if
		set zcFirstRow to first row index of zcTheRange
		set zcFirstColumn to first column index of zcTheRange
		set zcValues to value of zcTheRange
		set zcFormulas to formula of zcTheRange
	end tell
	if class of zcValues is not list then set zcValues to {{zcValues}}
	if class of zcFormulas is not list then set zcFormulas to {{zcFormulas}}
	set zcOut to {}
	repeat with r from 1 to (count of zcValues)
		repeat with c from 1 to (count of item r of zcValues)
			set v to item c of item r of zcValues
			set f to item c of item r of zcFormulas
			if f does not start with "=" then set f to ""
			if v is not "" or f is not "" then
				if class of v is in {real, integer} then
					set zcShown to "n" & (v as text)
				else if class of v is boolean then
					set zcShown to "b" & (v as text)
				else
					set zcShown to "s" & (v as text)
				end if
				set zcPlace to ((zcFirstRow + r - 1) as text) & ":" & ((zcFirstColumn + c - 1) as text)
				set end of zcOut to zcPlace & (character id 31) & zcShown & (character id 31) & f
			end if
		end repeat
	end repeat
	set AppleScript's text item delimiters to (character id 30)
	set zcAnswer to zcOut as text
	set AppleScript's text item delimiters to ""
	return zcAnswer
end run"#;

/// Where an open document's table is: the app, the document by its name as
/// the app shows it, and the sheet and table ("" for the first of each).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SheetInApp {
    pub app: SheetApp,
    pub document: String,
    pub sheet: String,
    pub table: String,
}

impl SheetInApp {
    fn lead(&self) -> Vec<String> {
        vec![
            SCRIPT_ARGV_LEAD.to_string(),
            self.document.clone(),
            self.sheet.clone(),
            self.table.clone(),
        ]
    }
}

/// `osascript`'s arguments for writing `cells`: the app's fixed script, then
/// every word as an argument of its own — an address, a kind (`s`, `n`, `b`,
/// `f`, `e`) and the raw value. Nothing a caller gave enters the script's
/// text.
///
/// # Errors
/// A text Numbers would take for a formula (it has no mark that keeps one
/// text), or more argument bytes than one call carries.
pub fn write_script_argv(
    target: &SheetInApp,
    cells: &[(CellAddress, SheetValue)],
) -> Result<Vec<String>, String> {
    let rows = cells.iter().map(|(at, _)| at.row).max().unwrap_or(1);
    let columns = cells.iter().map(|(at, _)| at.column).max().unwrap_or(1);
    let mut words = target.lead();
    words.push(rows.to_string());
    words.push(columns.to_string());
    for (address, value) in cells {
        let (kind, raw) = match value {
            SheetValue::Text(text) => {
                if target.app == SheetApp::Numbers && text.starts_with(FORMULA_MARK) {
                    return Err(format!(
                        "{}: Numbers' script makes a text starting with `=` a formula; write it through --path",
                        address.name()
                    ));
                }
                ("s", text.clone())
            }
            SheetValue::Number(number) => ("n", number_text(*number)),
            SheetValue::Bool(flag) => ("b", flag.to_string()),
            SheetValue::Formula(formula) => ("f", format!("{FORMULA_MARK}{formula}")),
            SheetValue::Empty => ("e", String::new()),
        };
        words.push(address.name());
        words.push(kind.to_string());
        words.push(raw);
    }
    script_argv(target.app.write_script(), words)
}

/// `osascript`'s arguments for reading `range` (the whole table when none).
///
/// # Errors
/// More argument bytes than one call carries — never, for a read.
pub fn read_script_argv(
    target: &SheetInApp,
    range: Option<CellRange>,
) -> Result<Vec<String>, String> {
    let mut words = target.lead();
    words.push(range.map(CellRange::name).unwrap_or_default());
    script_argv(target.app.read_script(), words)
}

fn script_argv(script: &str, words: Vec<String>) -> Result<Vec<String>, String> {
    let bytes: usize = words.iter().map(String::len).sum();
    if bytes > SHEET_MAX_SCRIPT_ARG_BYTES {
        return Err(format!(
            "the cells come to {bytes} bytes; the app's script takes at most {SHEET_MAX_SCRIPT_ARG_BYTES} — write the table to a file with --path"
        ));
    }
    let mut argv = vec!["-e".to_string(), script.to_string()];
    argv.extend(words);
    Ok(argv)
}

/// A cell as a read answers it.
#[derive(Debug, Clone, PartialEq)]
pub struct ReadCell {
    pub address: CellAddress,
    /// The value shown: for a formula, its last computed result when the
    /// file or app keeps one.
    pub value: Value,
    /// The formula with its `=`, when the cell has one.
    pub formula: Option<String>,
    /// An error value such as `#DIV/0!`.
    pub error: bool,
}

impl ReadCell {
    #[must_use]
    pub fn to_json(&self) -> Value {
        let mut cell = json!({ "cell": self.address.name(), "value": self.value });
        if let Some(formula) = &self.formula {
            cell["formula"] = json!(formula);
        }
        if self.error {
            cell["error"] = json!(true);
        }
        cell
    }
}

/// A read script's answer as cells: records of an address (`B2`, or Excel's
/// `row:column`), a kind-led value and a formula.
///
/// # Errors
/// A record that does not have those three fields.
pub fn parse_script_cells(answer: &str) -> Result<Vec<ReadCell>, String> {
    let answer = answer.strip_suffix('\n').unwrap_or(answer);
    if answer.is_empty() {
        return Ok(Vec::new());
    }
    answer
        .split(SCRIPT_RECORD)
        .map(|record| {
            let mut fields = record.splitn(3, SCRIPT_FIELD);
            let (Some(place), Some(shown), Some(formula)) =
                (fields.next(), fields.next(), fields.next())
            else {
                return Err(format!(
                    "the app's script answered an unread record `{record}`"
                ));
            };
            let address = match place.split_once(':') {
                Some((row, column)) => CellAddress {
                    row: row.trim().parse().map_err(|_| format!("bad row `{row}`"))?,
                    column: column
                        .trim()
                        .parse()
                        .map_err(|_| format!("bad column `{column}`"))?,
                },
                None => CellAddress::parse(place)?,
            };
            let mut kind = shown.chars();
            let value = match (kind.next(), kind.as_str()) {
                (Some('n'), text) => script_number(text).map_or_else(|| json!(text), |n| json!(n)),
                (Some('b'), text) => json!(text == "true"),
                (Some('s'), text) => json!(text),
                _ => Value::Null,
            };
            Ok(ReadCell {
                address,
                value,
                formula: (!formula.is_empty()).then(|| formula.to_string()),
                error: false,
            })
        })
        .collect()
}

/// AppleScript's text of a number: `3.0`, `1.4481E+6`, or a comma for the
/// decimal point under a locale that writes one.
fn script_number(text: &str) -> Option<f64> {
    text.parse()
        .ok()
        .or_else(|| text.replacen(',', ".", 1).parse().ok())
        .filter(|n: &f64| n.is_finite())
}

/// One cell written against the same cell read back: the words, the number,
/// the flag and the formula a person would see. A formula is compared
/// without case or spaces, the way the app rewrites one.
#[must_use]
pub fn read_back_matches(wanted: &SheetValue, read: Option<&ReadCell>) -> bool {
    let squeezed = |text: &str| -> String {
        text.chars()
            .filter(|c| !c.is_whitespace())
            .flat_map(char::to_uppercase)
            .collect()
    };
    match (wanted, read) {
        (SheetValue::Empty, None) => true,
        (SheetValue::Empty, Some(cell)) => cell.value.is_null() && cell.formula.is_none(),
        (_, None) => false,
        (SheetValue::Text(text), Some(cell)) => {
            cell.formula.is_none() && cell.value.as_str() == Some(text.as_str())
        }
        (SheetValue::Number(number), Some(cell)) => {
            cell.formula.is_none()
                && cell
                    .value
                    .as_f64()
                    .is_some_and(|read| (read - number).abs() <= number.abs().max(1.0) * 1e-9)
        }
        (SheetValue::Bool(flag), Some(cell)) => {
            cell.formula.is_none() && cell.value.as_bool() == Some(*flag)
        }
        (SheetValue::Formula(formula), Some(cell)) => cell.formula.as_deref().is_some_and(|read| {
            squeezed(read.strip_prefix(FORMULA_MARK).unwrap_or(read)) == squeezed(formula)
        }),
    }
}

/// The verification an answer carries: each written cell as it reads back,
/// and the ones that did not.
#[must_use]
pub fn verification(cells: &[(CellAddress, SheetValue)], read: &[ReadCell]) -> Value {
    let by_address: BTreeMap<CellAddress, &ReadCell> =
        read.iter().map(|cell| (cell.address, cell)).collect();
    let mut checked = Vec::new();
    let mut mismatched = Vec::new();
    for (address, wanted) in cells {
        let found = by_address.get(address).copied();
        let mut row = found.map_or_else(
            || json!({ "cell": address.name(), "value": Value::Null }),
            ReadCell::to_json,
        );
        if !read_back_matches(wanted, found) {
            row["wanted"] = wanted.to_json();
            mismatched.push(address.name());
        }
        checked.push(row);
    }
    json!({
        "verified": mismatched.is_empty(),
        "readBack": checked,
        "mismatched": mismatched,
    })
}

/// A CSV file as rows of fields (RFC 4180): quotes around a field that holds
/// a comma, a quote or a line break, a doubled quote inside. A byte-order
/// mark is dropped; how the file ended its lines is kept to write it back.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CsvTable {
    pub rows: Vec<Vec<String>>,
    pub crlf: bool,
    pub bom: bool,
}

const BOM: char = '\u{feff}';

impl CsvTable {
    /// # Errors
    /// A quoted field that never closes.
    pub fn parse(text: &str) -> Result<Self, String> {
        let bom = text.starts_with(BOM);
        let text = text.strip_prefix(BOM).unwrap_or(text);
        let crlf = text.contains("\r\n");
        let mut rows = Vec::new();
        let mut row = Vec::new();
        let mut field = String::new();
        let mut quoted = false;
        let mut chars = text.chars().peekable();
        let mut line_has_content = false;
        while let Some(c) = chars.next() {
            if quoted {
                match c {
                    '"' if chars.peek() == Some(&'"') => {
                        field.push('"');
                        chars.next();
                    }
                    '"' => quoted = false,
                    _ => field.push(c),
                }
                continue;
            }
            match c {
                '"' if field.is_empty() => {
                    quoted = true;
                    line_has_content = true;
                }
                ',' => {
                    row.push(std::mem::take(&mut field));
                    line_has_content = true;
                }
                '\r' if chars.peek() == Some(&'\n') => {}
                '\n' => {
                    row.push(std::mem::take(&mut field));
                    rows.push(std::mem::take(&mut row));
                    line_has_content = false;
                }
                _ => {
                    field.push(c);
                    line_has_content = true;
                }
            }
        }
        if quoted {
            return Err("the CSV file has a quoted field that never closes".into());
        }
        if line_has_content {
            row.push(field);
            rows.push(row);
        }
        Ok(Self { rows, crlf, bom })
    }

    #[must_use]
    pub fn render(&self) -> String {
        let newline = if self.crlf { "\r\n" } else { "\n" };
        let mut out = String::new();
        if self.bom {
            out.push(BOM);
        }
        for row in &self.rows {
            let line: Vec<String> = row.iter().map(|field| csv_field(field)).collect();
            out.push_str(&line.join(","));
            out.push_str(newline);
        }
        out
    }

    /// Writes each cell into its row and column, growing a row only as far
    /// as a written cell. Rows and fields not named stay as they were.
    pub fn apply(&mut self, cells: &[(CellAddress, SheetValue)]) {
        for (address, value) in cells {
            let (row, column) = (address.row as usize - 1, address.column as usize - 1);
            if matches!(value, SheetValue::Empty)
                && self.rows.get(row).is_none_or(|r| r.len() <= column)
            {
                continue;
            }
            if self.rows.len() <= row {
                self.rows.resize_with(row + 1, Vec::new);
            }
            let line = &mut self.rows[row];
            if line.len() <= column {
                line.resize(column + 1, String::new());
            }
            line[column] = csv_cell_text(value);
        }
    }

    /// The cells with something in them, within `range` when one is named.
    #[must_use]
    pub fn cells(&self, range: Option<CellRange>) -> Vec<ReadCell> {
        let mut cells = Vec::new();
        for (r, row) in self.rows.iter().enumerate() {
            for (c, field) in row.iter().enumerate() {
                let (Ok(row), Ok(column)) = (u32::try_from(r + 1), u32::try_from(c + 1)) else {
                    continue;
                };
                let address = CellAddress { row, column };
                if field.is_empty() || range.is_some_and(|range| !range.contains(address)) {
                    continue;
                }
                cells.push(csv_read_cell(address, field));
            }
        }
        cells
    }
}

/// A CSV field as written: quoted only when it must be.
fn csv_field(field: &str) -> String {
    if field.contains([',', '"', '\n', '\r']) || field.starts_with(' ') || field.ends_with(' ') {
        format!("\"{}\"", field.replace('"', "\"\""))
    } else {
        field.to_string()
    }
}

/// A cell's value as CSV text: a formula keeps its `=`; a text a CSV reader
/// would run as a formula goes behind [`TEXT_MARK`].
fn csv_cell_text(value: &SheetValue) -> String {
    match value {
        SheetValue::Text(text) if text.starts_with(CSV_FORMULA_STARTS) => {
            format!("{TEXT_MARK}{text}")
        }
        SheetValue::Text(text) => text.clone(),
        SheetValue::Number(number) => number_text(*number),
        SheetValue::Bool(true) => "TRUE".into(),
        SheetValue::Bool(false) => "FALSE".into(),
        SheetValue::Formula(formula) => format!("{FORMULA_MARK}{formula}"),
        SheetValue::Empty => String::new(),
    }
}

/// A CSV field read as a cell, the way [`csv_cell_text`] wrote it: a number
/// only when it is written as one (`007` stays text), `TRUE`/`FALSE` a flag.
fn csv_read_cell(address: CellAddress, field: &str) -> ReadCell {
    let plain = |value: Value| ReadCell {
        address,
        value,
        formula: None,
        error: false,
    };
    if let Some(text) = field
        .strip_prefix(TEXT_MARK)
        .filter(|rest| rest.starts_with(CSV_FORMULA_STARTS))
    {
        return plain(json!(text));
    }
    if field.starts_with(FORMULA_MARK) && field.len() > 1 {
        return ReadCell {
            address,
            value: Value::Null,
            formula: Some(field.to_string()),
            error: false,
        };
    }
    match field {
        "TRUE" => return plain(json!(true)),
        "FALSE" => return plain(json!(false)),
        _ => {}
    }
    match field.parse::<f64>() {
        Ok(number) if number.is_finite() && number_text(number) == field => plain(json!(number)),
        _ => plain(json!(field)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell(word: &str) -> CellAddress {
        CellAddress::parse(word).expect(word)
    }

    #[test]
    fn addresses_read_letters_then_digits_to_the_last_column_and_row() {
        assert_eq!(cell("A1"), CellAddress { row: 1, column: 1 });
        assert_eq!(cell("b2"), CellAddress { row: 2, column: 2 });
        assert_eq!(
            cell("$AA$10"),
            CellAddress {
                row: 10,
                column: 27
            }
        );
        assert_eq!(cell("XFD1048576").name(), "XFD1048576");
        for bad in [
            "", "A", "1", "A0", "A01", "1A", "A1B", "XFE1", "A1048577", "가1", "A-1",
        ] {
            assert!(CellAddress::parse(bad).is_err(), "{bad:?}");
        }
        for column in [1, 26, 27, 52, 53, 702, 703, 16_384] {
            let name = format!("{}1", CellAddress::column_letters(column));
            assert_eq!(cell(&name).column, column, "{name}");
        }
    }

    #[test]
    fn a_range_takes_either_corner_first_or_one_cell() {
        let range = CellRange::parse("C4:A1").expect("a range");
        assert_eq!(range.name(), "A1:C4");
        assert!(range.contains(cell("B2")));
        assert!(!range.contains(cell("D1")));
        assert_eq!(CellRange::parse("B2").expect("one cell").name(), "B2:B2");
        assert!(CellRange::parse("A1:").is_err());
    }

    #[test]
    fn cells_json_keeps_formulas_numbers_hangul_and_refuses_what_no_cell_holds() {
        let cells = parse_cells(
            r#"{"b2":3,"A1":"품목","C4":"=SUM(C2:C3)","D1":"'=1+1","E1":true,"F1":null,"G1":1200.5}"#,
        )
        .expect("cells");
        let named: Vec<String> = cells.iter().map(|(at, _)| at.name()).collect();
        assert_eq!(
            named,
            ["A1", "D1", "E1", "F1", "G1", "B2", "C4"],
            "sheet order"
        );
        assert_eq!(cells[0].1, SheetValue::Text("품목".into()));
        assert_eq!(cells[1].1, SheetValue::Text("=1+1".into()));
        assert_eq!(cells[2].1, SheetValue::Bool(true));
        assert_eq!(cells[3].1, SheetValue::Empty);
        assert_eq!(cells[4].1, SheetValue::Number(1200.5));
        assert_eq!(cells[5].1, SheetValue::Number(3.0));
        assert_eq!(cells[6].1, SheetValue::Formula("SUM(C2:C3)".into()));
        // Round trip through params.
        assert_eq!(
            cells_from_value(&cells_to_value(&cells)).expect("again"),
            cells
        );

        for (raw, says) in [
            ("[]", "JSON object"),
            ("{}", "no cell"),
            ("not json", "not JSON"),
            (r#"{"A1":1,"a1":2}"#, "twice"),
            (r#"{"A0":1}"#, "cell address"),
            (r#"{"A1":[1]}"#, "holds a string"),
            (r#"{"A1":{"x":1}}"#, "holds a string"),
            (r#"{"A1":"="}"#, "after its"),
            ("{\"A1\":\"bell\\u0007\"}", "control character"),
        ] {
            let refused = parse_cells(raw).expect_err(raw);
            assert!(refused.contains(says), "{raw}: {refused}");
        }
        let long = format!(r#"{{"A1":"{}"}}"#, "x".repeat(SHEET_MAX_TEXT_CHARS + 1));
        assert!(parse_cells(&long).is_err());
    }

    #[test]
    fn sheet_names_follow_the_workbooks_rule() {
        assert!(check_sheet_name("매출 2026").is_ok());
        for bad in ["", "a/b", "a[1]", "x:y", "'quoted'", &"x".repeat(32)] {
            assert!(check_sheet_name(bad).is_err(), "{bad:?}");
        }
    }

    fn roots(home: &Path) -> SheetRoots {
        SheetRoots {
            home: home.to_path_buf(),
            temp: None,
        }
    }

    #[test]
    fn a_path_stays_in_the_home_folder_and_names_a_sheet_file() {
        let home = tempfile::tempdir().expect("home");
        let outside = tempfile::tempdir().expect("outside");
        let roots = roots(home.path());
        let real_home = std::fs::canonicalize(home.path()).expect("real");

        let (path, kind) =
            sheet_path(&home.path().join("t.xlsx").to_string_lossy(), None, &roots).expect("home");
        assert_eq!(kind, SheetFileKind::Xlsx);
        assert_eq!(path, home.path().join("t.xlsx"));
        // A folder still to be made is inside when its nearest real folder is.
        assert!(
            sheet_path(
                &home.path().join("new/deeper/t.CSV").to_string_lossy(),
                None,
                &roots
            )
            .is_ok()
        );
        // Relative paths start where the caller stands, and nowhere else.
        let (relative, kind) = sheet_path("sub/t.csv", Some(&real_home), &roots).expect("relative");
        assert_eq!(
            (relative, kind),
            (real_home.join("sub/t.csv"), SheetFileKind::Csv)
        );
        assert!(
            sheet_path("t.csv", None, &roots)
                .unwrap_err()
                .contains("absolute")
        );

        let refused = |asked: String| sheet_path(&asked, None, &roots).expect_err(&asked);
        assert!(
            refused(outside.path().join("t.xlsx").to_string_lossy().into()).contains("outside")
        );
        #[cfg(unix)]
        {
            assert!(refused("/etc/t.csv".into()).contains("outside"));
            assert!(refused("/System/t.xlsx".into()).contains("outside"));
        }
        assert!(refused(format!("{}/../t.xlsx", home.path().display())).contains(".."));
        assert!(refused(home.path().join("t.numbers").to_string_lossy().into()).contains(".xlsx"));
        assert!(refused(home.path().join("t.xls").to_string_lossy().into()).contains(".xlsx"));
        assert!(refused(String::new()).contains("no file"));

        #[cfg(unix)]
        {
            // A link out of the home folder is followed before it is judged,
            // and a file that is itself a link is never written through.
            std::os::unix::fs::symlink(outside.path(), home.path().join("away")).expect("link");
            assert!(
                refused(home.path().join("away/t.xlsx").to_string_lossy().into())
                    .contains("outside")
            );
            std::fs::write(outside.path().join("real.csv"), "a").expect("file");
            std::os::unix::fs::symlink(outside.path().join("real.csv"), home.path().join("l.csv"))
                .expect("link");
            assert!(refused(home.path().join("l.csv").to_string_lossy().into()).contains("link"));
        }
        // The person's own temp folder is a root too, when named.
        let with_temp = SheetRoots {
            home: home.path().to_path_buf(),
            temp: Some(outside.path().to_path_buf()),
        };
        assert!(
            sheet_path(
                &outside.path().join("t.csv").to_string_lossy(),
                None,
                &with_temp
            )
            .is_ok()
        );
    }

    #[test]
    fn apps_are_numbers_or_excel_by_any_of_their_names() {
        assert_eq!(SheetApp::parse("numbers"), Ok(SheetApp::Numbers));
        assert_eq!(SheetApp::parse("com.apple.Numbers"), Ok(SheetApp::Numbers));
        assert_eq!(SheetApp::parse("Microsoft Excel"), Ok(SheetApp::Excel));
        assert_eq!(SheetApp::parse("EXCEL"), Ok(SheetApp::Excel));
        assert!(
            SheetApp::parse("LibreOffice")
                .unwrap_err()
                .contains("--path")
        );
    }

    fn in_app(app: SheetApp) -> SheetInApp {
        SheetInApp {
            app,
            document: "-e \" & quit".into(),
            sheet: String::new(),
            table: String::new(),
        }
    }

    #[test]
    fn a_script_carries_every_value_as_an_argument_and_never_in_its_text() {
        let hostile = "\" & (do shell script \"touch /tmp/x\") & \"";
        let cells = parse_cells(
            &serde_json::to_string(&json!({
                "A1": hostile, "B2": -5, "C3": "=SUM(B2:B3)", "D4": null, "E5": false,
            }))
            .expect("json"),
        )
        .expect("cells");
        for app in [SheetApp::Numbers, SheetApp::Excel] {
            let argv = write_script_argv(&in_app(app), &cells).expect("argv");
            assert_eq!(argv[0], "-e");
            assert_eq!(argv[1], app.write_script(), "the script is the fixed text");
            assert!(!argv[1].contains("touch"));
            assert!(
                !argv[1].contains("activate"),
                "a script never brings the app forward"
            );
            assert!(argv[1].contains(&format!(
                "application id \"{}\" is running",
                app.bundle_id()
            )));
            assert_eq!(
                &argv[2..],
                [
                    SCRIPT_ARGV_LEAD,
                    "-e \" & quit",
                    "",
                    "",
                    "5",
                    "5",
                    "A1",
                    "s",
                    hostile,
                    "B2",
                    "n",
                    "-5",
                    "C3",
                    "f",
                    "=SUM(B2:B3)",
                    "D4",
                    "e",
                    "",
                    "E5",
                    "b",
                    "false",
                ],
                "{app:?}"
            );
            let read = read_script_argv(&in_app(app), Some(CellRange::parse("A1:C4").expect("r")))
                .expect("read");
            assert_eq!(read[1], app.read_script());
            assert!(!read[1].contains("activate"));
            assert_eq!(
                &read[2..],
                [SCRIPT_ARGV_LEAD, "-e \" & quit", "", "", "A1:C4"]
            );
        }
        // The script reads its words where the arguments put them.
        for script in [NUMBERS_WRITE, EXCEL_WRITE] {
            assert!(
                script.contains("item 2 of argv")
                    && script.contains("from 7 to (count of argv) by 3")
            );
        }
    }

    #[test]
    fn numbers_refuses_a_text_it_would_make_a_formula_and_a_call_too_big_to_carry() {
        let cells = vec![(cell("A1"), SheetValue::Text("=not a formula".into()))];
        assert!(
            write_script_argv(&in_app(SheetApp::Numbers), &cells)
                .unwrap_err()
                .contains("--path")
        );
        assert!(write_script_argv(&in_app(SheetApp::Excel), &cells).is_ok());
        let wide: Vec<_> = (1..=10)
            .map(|row| {
                (
                    CellAddress { row, column: 1 },
                    SheetValue::Text("x".repeat(30_000)),
                )
            })
            .collect();
        assert!(
            write_script_argv(&in_app(SheetApp::Excel), &wide)
                .unwrap_err()
                .contains("--path")
        );
    }

    #[test]
    fn a_read_scripts_records_come_back_as_cells() {
        let answer = format!(
            "A1\u{1f}s품목\u{1f}{r}B2\u{1f}n1200.5\u{1f}{r}B4\u{1f}n5.0\u{1f}=SUM(B2:B3){r}C4\u{1f}n1.4481E+6\u{1f}=SUMPRODUCT(B2:B3,C2:C3){r}3:5\u{1f}btrue\u{1f}{r}A9\u{1f}e\u{1f}=1/0\n",
            r = SCRIPT_RECORD
        );
        let cells = parse_script_cells(&answer).expect("cells");
        let json: Vec<Value> = cells.iter().map(ReadCell::to_json).collect();
        assert_eq!(
            json,
            vec![
                json!({"cell":"A1","value":"품목"}),
                json!({"cell":"B2","value":1200.5}),
                json!({"cell":"B4","value":5.0,"formula":"=SUM(B2:B3)"}),
                json!({"cell":"C4","value":1_448_100.0,"formula":"=SUMPRODUCT(B2:B3,C2:C3)"}),
                json!({"cell":"E3","value":true}),
                json!({"cell":"A9","value":null,"formula":"=1/0"}),
            ]
        );
        assert!(parse_script_cells("").expect("none").is_empty());
        assert!(parse_script_cells("A1\u{1f}s").is_err());
    }

    #[test]
    fn verification_reads_each_written_cell_back() {
        let cells = parse_cells(r#"{"A1":"품목","B2":3,"B4":"=sum(B2:B3)","C1":null,"D1":true}"#)
            .expect("cells");
        let read = vec![
            ReadCell {
                address: cell("A1"),
                value: json!("품목"),
                formula: None,
                error: false,
            },
            ReadCell {
                address: cell("B2"),
                value: json!(3.0000000001),
                formula: None,
                error: false,
            },
            ReadCell {
                address: cell("B4"),
                value: json!(3),
                formula: Some("=SUM(B2:B3)".into()),
                error: false,
            },
            ReadCell {
                address: cell("D1"),
                value: json!(false),
                formula: None,
                error: false,
            },
        ];
        let said = verification(&cells, &read);
        assert_eq!(said["verified"], json!(false));
        assert_eq!(said["mismatched"], json!(["D1"]));
        assert_eq!(said["readBack"].as_array().map(Vec::len), Some(5));
        assert_eq!(said["readBack"][2]["wanted"], json!(true));
        // A text that reads back as a number is not the text written.
        assert!(!read_back_matches(
            &SheetValue::Text("007".into()),
            Some(&ReadCell {
                address: cell("A1"),
                value: json!(7),
                formula: None,
                error: false
            })
        ));
    }

    #[test]
    fn csv_keeps_quotes_line_breaks_hangul_and_its_line_endings() {
        let text = "\u{feff}품목,\"a, b\",\"say \"\"hi\"\"\"\r\n\"two\nlines\",,007\r\n";
        let table = CsvTable::parse(text).expect("csv");
        assert!(table.crlf && table.bom);
        assert_eq!(
            table.rows,
            vec![
                vec!["품목".to_string(), "a, b".into(), "say \"hi\"".into()],
                vec!["two\nlines".to_string(), String::new(), "007".into()],
            ]
        );
        assert_eq!(table.render(), text);
        assert!(CsvTable::parse("\"open").is_err());
        assert_eq!(CsvTable::parse("a\n\nb").expect("blank").rows.len(), 3);
    }

    #[test]
    fn csv_writes_only_the_named_cells_and_reads_them_back_the_same() {
        let mut table = CsvTable::parse("a,b\nc\n").expect("csv");
        let cells = parse_cells(
            r#"{"C3":"=SUM(A1:A2)","A2":3,"B1":"-1 is text","D1":"'=x","C1":true,"A4":null,"E2":"007"}"#,
        )
        .expect("cells");
        table.apply(&cells);
        assert_eq!(
            table.render(),
            "a,'-1 is text,TRUE,'=x\n3,,,,007\n,,=SUM(A1:A2)\n"
        );
        let read = CsvTable::parse(&table.render()).expect("again").cells(None);
        assert_eq!(
            verification(&cells, &read)["verified"],
            json!(true),
            "{read:?}"
        );
        let ranged = table.cells(Some(CellRange::parse("A1:B2").expect("r")));
        let names: Vec<String> = ranged.iter().map(|c| c.address.name()).collect();
        assert_eq!(names, ["A1", "B1", "A2"]);
    }
}
