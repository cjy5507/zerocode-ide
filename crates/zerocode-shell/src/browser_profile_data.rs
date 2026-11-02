//! 크롬 프로필의 북마크·주소·입력값 — 프로필 가져오기의 둘째 조각 (t-79331).
//!
//! 쿠키는 판이 태어날 때 저장소에 앉아야 해서 스테이징을 지나지만
//! ([`crate::browser_cookie_import`]), 여기 셋은 창이 직접 읽는 자료다: 북마크는
//! 주소줄 제안과 북마크 줄이, 주소와 입력값은 페이지 폼 채움
//! ([`crate::browser_form_fill`])이 읽는다. 그래서 프로필마다 파일로 앱 저장소
//! (`config_root`)에 둔다 — 주인만 읽는 권한으로(`durable_file::replace_bytes`).
//!
//! 원본은 둘이다. `Bookmarks`는 JSON이고 `Web Data`는 SQLite다. Web Data의
//! 주소 표 이름은 크롬 판마다 다르다(옛 `autofill_profiles`, 그 뒤의
//! `local_addresses`·`contact_info`, 지금의 `addresses`) — 서 있는 표가 답이고,
//! 스키마 버전(`meta` 표)은 로그 한 줄에만 간다. 결제 카드는 옮기지 않는다:
//! 값의 종류로 비밀이라, 몇 건이 남았는지만 센다. 입력값 가운데 값의 종류가
//! 비밀인 것(비밀번호·결제 카드·인증번호 칸의 값, 카드 번호 꼴, 토큰 꼴 낱말)도
//! 같은 이유로 센다.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Chromium 프로필 폴더 안의 두 원본.
pub const BOOKMARKS_FILE: &str = "Bookmarks";
pub const WEB_DATA_FILE: &str = "Web Data";

/// 한 프로필에 두는 북마크의 상한 — 주소줄 제안이 글자마다 훑는 목록이다.
const BOOKMARKS_KEPT: usize = 20_000;
/// 한 프로필에 두는 입력값의 상한 — 많이 쓴 것부터 남긴다.
const ENTRIES_KEPT: usize = 5_000;

/* ---- 북마크 ---- */

/// 북마크가 선 뿌리 — 크롬의 즐겨찾기 바, 기타 북마크, 모바일 북마크.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BookmarkRoot {
    Bar,
    Other,
    Mobile,
}

/// Bookmarks JSON의 `roots` 열쇠와 그 뿌리.
const BOOKMARK_ROOTS: [(&str, BookmarkRoot); 3] = [
    ("bookmark_bar", BookmarkRoot::Bar),
    ("other", BookmarkRoot::Other),
    ("synced", BookmarkRoot::Mobile),
];

/// 북마크 하나. 폴더는 뿌리 바로 밑의 첫 단계 이름만 남는다 — 더 깊은 폴더의
/// 북마크도 그 첫 단계 폴더에 든다.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Bookmark {
    pub title: String,
    pub url: String,
    pub root: BookmarkRoot,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
}

/// Bookmarks를 읽은 결과. 웹 주소가 아닌 북마크(`javascript:`·`chrome://`)는
/// 판이 열 수 없어 옮기지 않고 세고, 상한(`BOOKMARKS_KEPT`)을 넘은 것은
/// 따로 센다.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct BookmarksRead {
    pub bookmarks: Vec<Bookmark>,
    pub skipped: usize,
    pub over_cap: usize,
}

fn is_web_address(address: &str) -> bool {
    address
        .parse::<tauri::Url>()
        .is_ok_and(|parsed| matches!(parsed.scheme(), "http" | "https"))
}

/// 크롬의 Bookmarks JSON을 읽는다. 뿌리 셋을 바·기타·모바일 순으로 걷고,
/// 각 뿌리 안에서는 크롬이 보여 주는 순서 그대로다.
pub fn parse_chromium_bookmarks(text: &str) -> Result<BookmarksRead, String> {
    let parsed: serde_json::Value =
        serde_json::from_str(text).map_err(|_| "the Bookmarks file is not JSON".to_string())?;
    let roots = parsed
        .get("roots")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| "the Bookmarks file has no roots".to_string())?;
    let mut read = BookmarksRead::default();
    for (key, root) in BOOKMARK_ROOTS {
        if let Some(node) = roots.get(key) {
            walk_bookmarks(node, root, None, &mut read);
        }
    }
    Ok(read)
}

fn walk_bookmarks(
    node: &serde_json::Value,
    root: BookmarkRoot,
    folder: Option<&str>,
    read: &mut BookmarksRead,
) {
    let Some(children) = node.get("children").and_then(serde_json::Value::as_array) else {
        return;
    };
    for child in children {
        let name = child
            .get("name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        match child.get("type").and_then(serde_json::Value::as_str) {
            Some("url") => {
                let url = child
                    .get("url")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default();
                if !is_web_address(url) {
                    read.skipped += 1;
                    continue;
                }
                if read.bookmarks.len() >= BOOKMARKS_KEPT {
                    read.over_cap += 1;
                    continue;
                }
                read.bookmarks.push(Bookmark {
                    title: name.to_string(),
                    url: url.to_string(),
                    root,
                    folder: folder.map(str::to_string),
                });
            }
            // 첫 단계 폴더의 이름이 그 밑 모든 북마크의 폴더다.
            Some("folder") => walk_bookmarks(child, root, Some(folder.unwrap_or(name)), read),
            _ => {}
        }
    }
}

/* ---- 주소와 입력값 ---- */

/// Chromium `FieldType`(components/autofill/core/browser/field_types.h)의
/// 번호와 HTML autocomplete 낱말. 페이지 쪽(`ui/browser-form-fill.js`)은 칸을
/// 같은 낱말로 가르므로, 저장된 주소는 처음부터 그 낱말로 둔다. 표에 없는
/// 번호(결제 카드, 쓰이지 않는 옛 전화 번호 갈래 등)는 옮기지 않는다.
const CHROMIUM_FIELD_TYPES: [(i64, &str); 14] = [
    (3, "given-name"),
    (4, "additional-name"),
    (5, "family-name"),
    (7, "name"),
    (9, "email"),
    (14, "tel"),
    (30, "address-line1"),
    (31, "address-line2"),
    (33, "address-level2"),
    (34, "address-level1"),
    (35, "postal-code"),
    (36, "country"),
    (60, "organization"),
    (77, "street-address"),
];

/// 주소가 낱말 줄(type token)로 사는 표들 — 새 판부터. `addresses`는 지금의
/// 표, 나머지 둘은 그 앞의 판들이 쓰던 이름이다.
const TOKEN_ADDRESS_TABLES: [(&str, &str); 3] = [
    ("addresses", "address_type_tokens"),
    ("local_addresses", "local_addresses_type_tokens"),
    ("contact_info", "contact_info_type_tokens"),
];

/// 옛 `autofill_profiles`의 칸과 그 낱말.
const LEGACY_PROFILE_COLUMNS: [(&str, &str); 6] = [
    ("company_name", "organization"),
    ("street_address", "street-address"),
    ("city", "address-level2"),
    ("state", "address-level1"),
    ("zipcode", "postal-code"),
    ("country_code", "country"),
];

/// 옛 판의 곁표들: 이름·이메일·전화가 주소 밖에 따로 선다.
const LEGACY_SIDE_TABLES: [(&str, &[(&str, &str)]); 3] = [
    (
        "autofill_profile_names",
        &[
            ("first_name", "given-name"),
            ("middle_name", "additional-name"),
            ("last_name", "family-name"),
            ("full_name", "name"),
        ],
    ),
    ("autofill_profile_emails", &[("email", "email")]),
    ("autofill_profile_phones", &[("number", "tel")]),
];

/// 저장된 주소 하나. `fields`의 열쇠는 HTML autocomplete 낱말이다.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedAddress {
    pub id: String,
    pub fields: BTreeMap<String, String>,
}

/// 입력값 하나 — 페이지의 칸 이름(`name` 속성)과 그 칸에 넣었던 값.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AutofillEntry {
    pub name: String,
    pub value: String,
    pub count: u64,
}

/// 한 프로필의 주소와 입력값. 둘 다 많이 쓴 것이 앞이다.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AddressBook {
    pub addresses: Vec<SavedAddress>,
    pub entries: Vec<AutofillEntry>,
}

/// Web Data를 읽은 결과.
#[derive(Debug, Default)]
pub struct WebDataRead {
    pub book: AddressBook,
    /// 옮기지 않은 결제 카드와 비밀로 보이는 입력값의 수.
    pub held_back: usize,
    /// `meta` 표의 스키마 버전 — 로그 한 줄의 재료.
    pub schema_version: Option<i64>,
}

/// 읽는 동안의 주소 — 많이 쓴 순서로 줄 세우려고 쓴 횟수와 때를 든다.
#[derive(Default)]
struct AddressDraft {
    fields: BTreeMap<String, String>,
    use_count: i64,
    use_date: i64,
}

fn table_names(db: &rusqlite::Connection) -> Result<BTreeSet<String>, String> {
    let mut statement = db
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table'")
        .map_err(|error| error.to_string())?;
    let names = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|error| error.to_string())?
        .filter_map(Result::ok)
        .collect();
    Ok(names)
}

fn column_names(db: &rusqlite::Connection, table: &str) -> Result<BTreeSet<String>, String> {
    let mut statement = db
        .prepare(&format!("PRAGMA table_info(\"{table}\")"))
        .map_err(|error| error.to_string())?;
    let names = statement
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|error| error.to_string())?
        .filter_map(Result::ok)
        .collect();
    Ok(names)
}

/// 표의 칸 하나를 고르는 SQL 조각 — 그 판에 칸이 없으면 0이다.
fn column_or_zero(columns: &BTreeSet<String>, column: &str) -> String {
    if columns.contains(column) {
        format!("\"{column}\"")
    } else {
        "0".to_string()
    }
}

fn keep_value(fields: &mut BTreeMap<String, String>, kind: &str, value: String) {
    let value = value.trim();
    if !value.is_empty() {
        fields.insert(kind.to_string(), value.to_string());
    }
}

/// 낱말 줄 표 한 쌍: 주소 행이 쓴 횟수를, 낱말 행이 칸마다 값을 든다.
fn read_token_addresses(
    db: &rusqlite::Connection,
    main: &str,
    tokens: &str,
    drafts: &mut BTreeMap<String, AddressDraft>,
) -> Result<(), String> {
    let columns = column_names(db, main)?;
    let mut rows = db
        .prepare(&format!(
            "SELECT guid, {}, {} FROM \"{main}\"",
            column_or_zero(&columns, "use_count"),
            column_or_zero(&columns, "use_date"),
        ))
        .map_err(|error| error.to_string())?;
    let found = rows
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1).unwrap_or(0),
                row.get::<_, i64>(2).unwrap_or(0),
            ))
        })
        .map_err(|error| error.to_string())?;
    for (guid, use_count, use_date) in found.filter_map(Result::ok) {
        let draft = drafts.entry(guid).or_default();
        draft.use_count = draft.use_count.max(use_count);
        draft.use_date = draft.use_date.max(use_date);
    }
    let mut values = db
        .prepare(&format!("SELECT guid, type, value FROM \"{tokens}\""))
        .map_err(|error| error.to_string())?;
    let found = values
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2).unwrap_or_default(),
            ))
        })
        .map_err(|error| error.to_string())?;
    for (guid, kind, value) in found.filter_map(Result::ok) {
        let Some((_, word)) = CHROMIUM_FIELD_TYPES.iter().find(|(id, _)| *id == kind) else {
            continue;
        };
        // 주소 행이 없는 낱말은 지워진 주소의 찌꺼기다.
        if let Some(draft) = drafts.get_mut(&guid) {
            keep_value(&mut draft.fields, word, value);
        }
    }
    Ok(())
}

/// 옛 판: 주소 칸은 `autofill_profiles`에, 이름·이메일·전화는 곁표에 선다.
fn read_legacy_addresses(
    db: &rusqlite::Connection,
    tables: &BTreeSet<String>,
    drafts: &mut BTreeMap<String, AddressDraft>,
) -> Result<(), String> {
    let columns = column_names(db, "autofill_profiles")?;
    let present: Vec<(&str, &str)> = LEGACY_PROFILE_COLUMNS
        .iter()
        .copied()
        .filter(|(column, _)| columns.contains(*column))
        .collect();
    let select = present
        .iter()
        .map(|(column, _)| format!(", \"{column}\""))
        .collect::<String>();
    let mut rows = db
        .prepare(&format!(
            "SELECT guid, {}, {}{select} FROM autofill_profiles",
            column_or_zero(&columns, "use_count"),
            column_or_zero(&columns, "use_date"),
        ))
        .map_err(|error| error.to_string())?;
    let found = rows
        .query_map([], |row| {
            let mut draft = AddressDraft {
                use_count: row.get::<_, i64>(1).unwrap_or(0),
                use_date: row.get::<_, i64>(2).unwrap_or(0),
                ..AddressDraft::default()
            };
            for (at, (_, word)) in present.iter().enumerate() {
                let value = row.get::<_, String>(3 + at).unwrap_or_default();
                keep_value(&mut draft.fields, word, value);
            }
            Ok((row.get::<_, String>(0)?, draft))
        })
        .map_err(|error| error.to_string())?;
    for (guid, draft) in found.filter_map(Result::ok) {
        drafts.entry(guid).or_insert(draft);
    }
    for (table, pairs) in LEGACY_SIDE_TABLES {
        if !tables.contains(table) {
            continue;
        }
        let side = column_names(db, table)?;
        for (column, word) in pairs {
            if !side.contains(*column) {
                continue;
            }
            let mut rows = db
                .prepare(&format!("SELECT guid, \"{column}\" FROM \"{table}\""))
                .map_err(|error| error.to_string())?;
            let found = rows
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1).unwrap_or_default(),
                    ))
                })
                .map_err(|error| error.to_string())?;
            for (guid, value) in found.filter_map(Result::ok) {
                if let Some(draft) = drafts.get_mut(&guid) {
                    // 곁표는 한 주소에 여러 행을 둘 수 있다 — 첫 값이 그 주소의 것이다.
                    if !draft.fields.contains_key(*word) {
                        keep_value(&mut draft.fields, word, value);
                    }
                }
            }
        }
    }
    Ok(())
}

/// 칸 이름이 이 낱말을 품으면 그 입력값은 결제 카드나 일회용 인증번호의
/// 것이다. 비밀번호·토큰을 말하는 낱말은 core의 표가 든다
/// (`names_a_credential`).
const SECRET_ENTRY_NAME_WORDS: [&str; 12] = [
    "card", "cc-", "cc_num", "ccnum", "cvc", "cvv", "csc", "otp", "one-time", "onetime", "verif",
    "인증",
];

/// 이 입력값은 비밀인가 — 값의 종류로 가른다. 칸 이름이 비밀번호·토큰·결제
/// 카드·인증번호를 말하거나, 값이 카드 번호 꼴이거나, 값의 어느 조각(공백으로
/// 나눈 것)이 토큰 표식을 달았거나, 값이 띄어 쓰지 않은 한 ASCII 낱말로 긴 혼합
/// 토큰 꼴일 때다 — 토큰 표식은 낱말의 가장자리에 서고 그 뒤에 토큰이 이어질
/// 때만 센다(「Asia」는 ASIA 표식이 아니다). 주소·메모처럼 띄어 쓴 글과 한글이
/// 든 값은 길고 숫자와 글자가 섞여도 토큰이 아니다 — core의 「긴 혼합 낱말」
/// 규칙은 한 낱말에만 건다. 그래도 `Bearer eyJ…`나 `키:AKIA…`처럼 조각 하나가
/// 키면 값 전체가 남는다.
fn entry_is_secret(name: &str, value: &str) -> bool {
    use zerocode_core::credential;
    let one_word = value.is_ascii() && !value.bytes().any(|byte| byte.is_ascii_whitespace());
    credential::names_a_credential(name)
        || SECRET_ENTRY_NAME_WORDS
            .iter()
            .any(|word| credential::contains_ascii_case(name, word))
        || credential::looks_like_a_payment_card(value)
        || value
            .split_whitespace()
            .any(credential::carries_a_value_mark_at_a_word_edge)
        || (one_word && credential::looks_like_an_opaque_token(value))
}

fn read_entries(
    db: &rusqlite::Connection,
    tables: &BTreeSet<String>,
    read: &mut WebDataRead,
) -> Result<(), String> {
    if !tables.contains("autofill") {
        return Ok(());
    }
    let columns = column_names(db, "autofill")?;
    let order = if columns.contains("date_last_used") {
        "ORDER BY count DESC, date_last_used DESC"
    } else {
        "ORDER BY count DESC"
    };
    let mut rows = db
        .prepare(&format!(
            "SELECT name, value, {} FROM autofill {order}",
            column_or_zero(&columns, "count"),
        ))
        .map_err(|error| error.to_string())?;
    let found = rows
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0).unwrap_or_default(),
                row.get::<_, String>(1).unwrap_or_default(),
                row.get::<_, i64>(2).unwrap_or(0),
            ))
        })
        .map_err(|error| error.to_string())?;
    for (name, value, count) in found.filter_map(Result::ok) {
        let (name, value) = (name.trim(), value.trim());
        if name.is_empty() || value.is_empty() {
            continue;
        }
        if entry_is_secret(name, value) {
            read.held_back += 1;
            continue;
        }
        if read.book.entries.len() >= ENTRIES_KEPT {
            break;
        }
        read.book.entries.push(AutofillEntry {
            name: name.to_string(),
            value: value.to_string(),
            count: u64::try_from(count).unwrap_or(0),
        });
    }
    Ok(())
}

/// `meta` 표의 스키마 버전. 크롬은 글자 칸에 수를 적는다.
fn schema_version(db: &rusqlite::Connection, tables: &BTreeSet<String>) -> Option<i64> {
    if !tables.contains("meta") {
        return None;
    }
    db.query_row("SELECT value FROM meta WHERE key = 'version'", [], |row| {
        row.get::<_, String>(0)
            .map(|text| text.trim().parse::<i64>().unwrap_or_default())
            .or_else(|_| row.get::<_, i64>(0))
    })
    .ok()
}

/// 크롬의 Web Data(사본)에서 주소와 입력값을 읽는다. 결제 카드는 읽지 않고
/// 행 수만 센다.
pub fn read_chromium_web_data(database: &Path) -> Result<WebDataRead, String> {
    let db =
        rusqlite::Connection::open_with_flags(database, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|error| error.to_string())?;
    let tables = table_names(&db)?;
    let mut read = WebDataRead {
        schema_version: schema_version(&db, &tables),
        ..WebDataRead::default()
    };
    let mut drafts = BTreeMap::new();
    for (main, tokens) in TOKEN_ADDRESS_TABLES {
        if tables.contains(main) && tables.contains(tokens) {
            read_token_addresses(&db, main, tokens, &mut drafts)?;
        }
    }
    if tables.contains("autofill_profiles") {
        read_legacy_addresses(&db, &tables, &mut drafts)?;
    }
    let mut drafts: Vec<(String, AddressDraft)> = drafts
        .into_iter()
        .filter(|(_, draft)| !draft.fields.is_empty())
        .collect();
    drafts.sort_by(|(_, a), (_, b)| {
        b.use_count
            .cmp(&a.use_count)
            .then(b.use_date.cmp(&a.use_date))
    });
    read.book.addresses = drafts
        .into_iter()
        .map(|(id, draft)| SavedAddress {
            id,
            fields: draft.fields,
        })
        .collect();
    read_entries(&db, &tables, &mut read)?;
    if tables.contains("credit_cards") {
        let cards = db
            .query_row("SELECT COUNT(*) FROM credit_cards", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap_or(0);
        read.held_back += usize::try_from(cards).unwrap_or(0);
    }
    Ok(read)
}

/* ---- 프로필 저장소 ---- */

/// 한 프로필로 옮긴 자료의 수 — 가져오기 카드와 설정 카드의 「북마크 J · 주소 K」.
/// 수만 든다.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ImportedData {
    pub bookmarks: usize,
    pub addresses: usize,
    pub autofill_entries: usize,
    /// 옮기지 않은 결제 카드와 비밀로 보이는 입력값.
    pub held_back: usize,
    /// 가져온 시각(unix 초).
    pub at_unix: i64,
}

pub fn bookmarks_file(config_root: &Path, profile_id: &str) -> PathBuf {
    config_root.join(format!("browser-bookmarks-{profile_id}.json"))
}

pub fn address_book_file(config_root: &Path, profile_id: &str) -> PathBuf {
    config_root.join(format!("browser-addresses-{profile_id}.json"))
}

pub fn imported_file(config_root: &Path, profile_id: &str) -> PathBuf {
    config_root.join(format!("browser-profile-imported-{profile_id}.json"))
}

fn save_private(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let bytes = serde_json::to_vec(value).map_err(|error| error.to_string())?;
    crate::durable_file::replace_bytes(path, &bytes)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

fn load<T: serde::de::DeserializeOwned + Default>(path: &Path) -> T {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/// 한 프로필의 북마크. 가져온 적이 없으면 빈 목록이다.
pub fn load_bookmarks(config_root: &Path, profile_id: &str) -> Vec<Bookmark> {
    load(&bookmarks_file(config_root, profile_id))
}

/// 한 프로필의 주소와 입력값. 가져온 적이 없으면 비어 있다.
pub fn load_address_book(config_root: &Path, profile_id: &str) -> AddressBook {
    load(&address_book_file(config_root, profile_id))
}

/// 한 프로필로 마지막에 옮긴 자료의 수. 옮긴 적이 없으면 없다.
pub fn last_imported(config_root: &Path, profile_id: &str) -> Option<ImportedData> {
    let text = std::fs::read_to_string(imported_file(config_root, profile_id)).ok()?;
    serde_json::from_str(&text).ok()
}

/// 프로필을 잊을 때 그 자료 셋을 함께 거둔다.
pub fn discard(config_root: &Path, profile_id: &str) {
    for path in [
        bookmarks_file(config_root, profile_id),
        address_book_file(config_root, profile_id),
        imported_file(config_root, profile_id),
    ] {
        let _ = std::fs::remove_file(path);
    }
}

/// 읽은 자료를 대상 프로필의 저장소에 둔다. 새 가져오기는 옛 자료를 갈아
/// 끼운다 — 원본 브라우저가 진실이다. 읽지 못한 몫(`None`)은 건드리지 않아
/// 지난번에 옮긴 자료가 그대로 남고, 그 수도 지난 수 그대로 센다.
pub fn save_imported(
    config_root: &Path,
    profile_id: &str,
    bookmarks: Option<&BookmarksRead>,
    web_data: Option<&WebDataRead>,
    at_unix: i64,
) -> Result<ImportedData, String> {
    let before = last_imported(config_root, profile_id).unwrap_or_default();
    let mut counts = ImportedData { at_unix, ..before };
    if let Some(read) = bookmarks {
        save_private(&bookmarks_file(config_root, profile_id), &read.bookmarks)?;
        counts.bookmarks = read.bookmarks.len();
    }
    if let Some(read) = web_data {
        save_private(&address_book_file(config_root, profile_id), &read.book)?;
        counts.addresses = read.book.addresses.len();
        counts.autofill_entries = read.book.entries.len();
        counts.held_back = read.held_back;
    }
    save_private(&imported_file(config_root, profile_id), &counts)?;
    Ok(counts)
}

/// 프로필 자료를 옮기지 못한 까닭의 종류 — 가져오기 카드가 이 값으로 사람
/// 말을 고른다(i18n 키). 자세한 글자는 창 로그 한 줄에만 간다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ProfileDataSkipped {
    /// 브라우저가 Web Data를 계속 고쳐 써서 안정된 사본을 뜨지 못했다 —
    /// 브라우저가 켜져 일하는 동안 흔하다.
    Locked,
    /// 북마크나 Web Data 파일을 읽거나 풀지 못했다.
    Unreadable,
    /// 읽은 자료를 프로필 저장소에 두지 못했다.
    Unsaved,
}

/// 프로필 자료를 옮기지 못한 일 — 종류는 카드로, 글자는 창 로그로 간다.
#[derive(Debug)]
pub struct ProfileDataError {
    pub kind: ProfileDataSkipped,
    pub detail: String,
}

impl ProfileDataError {
    fn of(kind: ProfileDataSkipped) -> impl FnOnce(String) -> Self {
        move |detail| Self { kind, detail }
    }
}

impl std::fmt::Display for ProfileDataError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}: {}", self.kind, self.detail)
    }
}

/// 옮기지 못한 몫과 그 까닭 — 북마크와 주소(Web Data)가 각자 까닭을 말한다.
/// 가져오기 카드가 이 값으로 몫마다 사람 말을 고른다.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileDataSkippedParts {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bookmarks: Option<ProfileDataSkipped>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub addresses: Option<ProfileDataSkipped>,
}

/// 읽은 두 몫을 대상 프로필에 두고, 읽지 못한 몫은 까닭과 함께 돌려준다. 한
/// 몫만 읽었어도 그 몫은 옮겨지고(수는 `Some`), 둘 다 못 읽었으면 수가 없다.
/// 저장이 실패하면 읽은 몫이 못 옮긴 것이라 그 몫은 `Unsaved`고, 못 읽은 몫은
/// 제 까닭을 둔다. 돌려주는 수는 이번에 옮긴 몫만 센다(못 읽은 몫은 0 —
/// 저장 파일과 `last_imported`는 지난 수를 그대로 둔다). 자세한 글자는 `log`
/// 한 줄씩으로만 간다.
pub fn import_profile_data(
    read: ProfileDataRead,
    config_root: &Path,
    profile_id: &str,
    at_unix: i64,
    mut log: impl FnMut(&str),
) -> (Option<ImportedData>, Option<ProfileDataSkippedParts>) {
    let mut skip = |error: ProfileDataError| {
        log(&format!("browser profile data import skipped: {error}"));
        error.kind
    };
    let bookmarks = read.bookmarks.map_err(&mut skip);
    let web_data = read.web_data.map_err(&mut skip);
    let skipped = ProfileDataSkippedParts {
        bookmarks: bookmarks.as_ref().err().copied(),
        addresses: web_data.as_ref().err().copied(),
    };
    let (kept_bookmarks, kept_web_data) = (bookmarks.as_ref().ok(), web_data.as_ref().ok());
    if kept_bookmarks.is_none() && kept_web_data.is_none() {
        return (None, Some(skipped));
    }
    log(&import_log_line(kept_bookmarks, kept_web_data));
    match save_imported(
        config_root,
        profile_id,
        kept_bookmarks,
        kept_web_data,
        at_unix,
    ) {
        Ok(counts) => (
            Some(counts_carried_now(counts, skipped)),
            (skipped != ProfileDataSkippedParts::default()).then_some(skipped),
        ),
        Err(detail) => {
            log(&format!("browser profile data import skipped: {detail}"));
            // 저장이 실패하면 읽은 몫이 못 옮긴 것이다 — 못 읽은 몫은 제 까닭을 둔다.
            let unsaved = Some(ProfileDataSkipped::Unsaved);
            (
                None,
                Some(ProfileDataSkippedParts {
                    bookmarks: skipped.bookmarks.or(unsaved),
                    addresses: skipped.addresses.or(unsaved),
                }),
            )
        }
    }
}

/// 가져오기 카드로 가는 수 — 이번 누름에 옮긴 몫만 센다. `save_imported`의 수는
/// 못 읽은 몫에 지난번 값을 두는데(저장 파일과 설정 화면은 저장된 양을 보인다),
/// 카드의 「가져옴」에 그 몫이 들면 같은 카드가 못 읽었다고 말하는 몫을 가져왔다고
/// 말하게 된다.
fn counts_carried_now(saved: ImportedData, skipped: ProfileDataSkippedParts) -> ImportedData {
    let mut now = saved;
    if skipped.bookmarks.is_some() {
        now.bookmarks = 0;
    }
    if skipped.addresses.is_some() {
        now.addresses = 0;
        now.autofill_entries = 0;
        now.held_back = 0;
    }
    now
}

/// 창 로그 한 줄 — 수와 Web Data의 스키마 버전만 적는다. 값·이름·주소는
/// 어느 것도 적지 않는다. 읽지 못한 몫은 「not read」다(까닭은 따로 한 줄).
pub fn import_log_line(
    bookmarks: Option<&BookmarksRead>,
    web_data: Option<&WebDataRead>,
) -> String {
    let count =
        |read: Option<usize>| read.map_or_else(|| "not read".to_string(), |n| n.to_string());
    format!(
        "browser profile data imported: bookmarks {} (not web addresses {}, over the cap {}), \
         addresses {}, entries {}, held back {}, web data schema {}",
        count(bookmarks.map(|read| read.bookmarks.len())),
        count(bookmarks.map(|read| read.skipped)),
        count(bookmarks.map(|read| read.over_cap)),
        count(web_data.map(|read| read.book.addresses.len())),
        count(web_data.map(|read| read.book.entries.len())),
        count(web_data.map(|read| read.held_back)),
        web_data
            .and_then(|read| read.schema_version)
            .map_or_else(|| "none".to_string(), |version| version.to_string()),
    )
}

/// 한 Chromium 프로필에서 읽은 두 몫 — 북마크와 Web Data(주소·입력값). 한
/// 몫이 깨져도 다른 몫은 읽힌다: 각자 읽고 각자 까닭을 말한다.
#[derive(Debug)]
pub struct ProfileDataRead {
    pub bookmarks: Result<BookmarksRead, ProfileDataError>,
    pub web_data: Result<WebDataRead, ProfileDataError>,
}

/// 한 Chromium 프로필의 북마크와 Web Data를 따로 읽는다. 원본이 없으면 그 몫은
/// 비어 있다 — 북마크를 하나도 만든 적 없는 프로필도 흔하다. Web Data는
/// 브라우저가 쥐고 고쳐 쓰는 DB라 쿠키처럼 안정된 사본에서 읽는다. 사본을
/// 뜨지 못하면 [`ProfileDataSkipped::Locked`], 그 밖에 읽지 못하면
/// [`ProfileDataSkipped::Unreadable`]이다.
pub fn read_chromium_profile_data(
    family: &str,
    profile: &str,
    home: &Path,
    scratch: &Path,
) -> ProfileDataRead {
    let unreadable = || ProfileDataError::of(ProfileDataSkipped::Unreadable);
    let bookmarks = match crate::browser_cookie_import::chromium_profile_file(
        family,
        profile,
        home,
        BOOKMARKS_FILE,
    ) {
        Some(path) => std::fs::read_to_string(&path)
            .map_err(|error| error.to_string())
            .and_then(|text| parse_chromium_bookmarks(&text))
            .map_err(unreadable()),
        None => Ok(BookmarksRead::default()),
    };
    let web_data = match crate::browser_cookie_import::chromium_profile_file(
        family,
        profile,
        home,
        WEB_DATA_FILE,
    ) {
        Some(path) => {
            let read = crate::browser_cookie_import::snapshot_sqlite(&path, scratch)
                .map_err(ProfileDataError::of(ProfileDataSkipped::Locked))
                .and_then(|copy| read_chromium_web_data(&copy).map_err(unreadable()));
            // 사본은 읽었든 못 읽었든 남지 않는다 — 사람의 주소가 든 파일이다.
            let _ = std::fs::remove_dir_all(scratch);
            read
        }
        None => Ok(WebDataRead::default()),
    };
    ProfileDataRead {
        bookmarks,
        web_data,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOOKMARKS_FIXTURE: &str =
        include_str!("../../../tests/fixtures/browser-profile/Bookmarks");
    const WEB_DATA_FIXTURE: &str =
        include_str!("../../../tests/fixtures/browser-profile/web-data.sql");
    const WEB_DATA_LEGACY_FIXTURE: &str =
        include_str!("../../../tests/fixtures/browser-profile/web-data-legacy.sql");

    fn web_data(sql: &str) -> (tempfile::TempDir, PathBuf) {
        let yard = tempfile::tempdir().expect("yard");
        let path = yard.path().join(WEB_DATA_FILE);
        rusqlite::Connection::open(&path)
            .expect("create")
            .execute_batch(sql)
            .expect("fixture schema");
        (yard, path)
    }

    /// 지금 판의 fixture에, 카드 번호 꼴 입력값 하나를 더한 것. 그 값은 시험이
    /// 실행 중에 짓는다 — 저장소의 어느 파일에도 글자로 서지 않는다.
    fn current_web_data() -> (tempfile::TempDir, PathBuf) {
        let (yard, path) = web_data(WEB_DATA_FIXTURE);
        let card_shaped = ["4111", "1111", "1111", "1111"].join(" ");
        rusqlite::Connection::open(&path)
            .expect("open")
            .execute(
                "INSERT INTO autofill VALUES('cardnumber', ?1, ?1, 1, 40, 5)",
                [&card_shaped],
            )
            .expect("card-shaped entry");
        (yard, path)
    }

    /// 즐겨찾기 바의 첫 단계 폴더만 남고, 더 깊은 폴더의 북마크는 그 첫 단계에
    /// 든다. 웹 주소가 아닌 북마크는 옮기지 않고 센다.
    #[test]
    fn bookmarks_keep_one_folder_level_and_web_addresses_only() {
        let read = parse_chromium_bookmarks(BOOKMARKS_FIXTURE).expect("fixture parses");
        let bar: Vec<&Bookmark> = read
            .bookmarks
            .iter()
            .filter(|one| one.root == BookmarkRoot::Bar)
            .collect();
        assert_eq!(bar.len(), 4, "{bar:?}");
        assert_eq!(bar[0].folder, None, "a bar item stays on the bar");
        assert_eq!(bar[0].title, "[합성-북마크-가]");
        assert!(
            bar[1..]
                .iter()
                .all(|one| one.folder.as_deref() == Some("[합성-폴더]")),
            "a deeper folder's bookmark did not fold into its first-level folder: {bar:?}"
        );
        assert_eq!(
            read.bookmarks
                .iter()
                .filter(|one| one.root == BookmarkRoot::Other)
                .count(),
            1
        );
        assert_eq!(
            read.bookmarks
                .iter()
                .filter(|one| one.root == BookmarkRoot::Mobile)
                .count(),
            1
        );
        assert_eq!(read.bookmarks.len(), 6);
        assert_eq!(read.skipped, 2, "javascript: and chrome:// were moved");
        assert!(
            read.bookmarks
                .iter()
                .all(|one| one.url.starts_with("https://") || one.url.starts_with("http://"))
        );
    }

    /// 상한을 넘은 북마크는 웹 주소가 아닌 것과 따로 센다 — 로그를 읽는 사람이
    /// 둘을 헷갈리지 않게.
    #[test]
    fn bookmarks_over_the_cap_are_counted_apart() {
        let node = |url: &str| serde_json::json!({ "type": "url", "name": "[합성]", "url": url });
        let mut children: Vec<serde_json::Value> = (0..BOOKMARKS_KEPT + 2)
            .map(|at| node(&format!("https://bookmark-{at}.example.com/")))
            .collect();
        children.push(node("javascript:void(0)"));
        let text = serde_json::json!({ "roots": { "bookmark_bar": { "children": children } } })
            .to_string();
        let read = parse_chromium_bookmarks(&text).expect("parses");
        assert_eq!(read.bookmarks.len(), BOOKMARKS_KEPT);
        assert_eq!(read.over_cap, 2);
        assert_eq!(
            read.skipped, 1,
            "a bookmark over the cap counted as not a web address"
        );
    }

    #[test]
    fn a_bookmarks_file_that_is_not_one_is_refused() {
        assert!(parse_chromium_bookmarks("{not json").is_err());
        assert!(parse_chromium_bookmarks("{}").is_err());
        let empty = parse_chromium_bookmarks(r#"{"roots":{}}"#).expect("no roots is empty");
        assert_eq!(empty, BookmarksRead::default());
    }

    /// 지금 판의 Web Data: 낱말 줄 표에서 주소를 읽고, 많이 쓴 주소가 앞에 선다.
    /// 결제 카드와 비밀로 보이는 입력값은 옮기지 않고 센다.
    #[test]
    fn the_current_web_data_reads_addresses_entries_and_holds_cards_back() {
        let (_yard, path) = current_web_data();
        let read = read_chromium_web_data(&path).expect("reads");
        assert_eq!(read.schema_version, Some(134));
        assert_eq!(read.book.addresses.len(), 2, "{:?}", read.book.addresses);
        let first = &read.book.addresses[0];
        assert_eq!(first.id, "00000000-0000-4000-8000-000000000002");
        assert_eq!(
            first.fields.get("name").map(String::as_str),
            Some("[합성-이름-나]")
        );
        assert_eq!(
            first.fields.get("postal-code").map(String::as_str),
            Some("[합성-우편번호]")
        );
        assert_eq!(
            first.fields.get("street-address").map(String::as_str),
            Some("[합성-주소-1줄]\n[합성-주소-2줄]")
        );
        assert!(
            first.fields.keys().all(|kind| CHROMIUM_FIELD_TYPES
                .iter()
                .any(|(_, word)| *word == kind.as_str())),
            "a field outside the table was moved: {:?}",
            first.fields.keys()
        );
        // 지워진 주소의 찌꺼기 낱말은 주소를 세우지 않는다.
        assert!(
            read.book
                .addresses
                .iter()
                .all(|one| one.id != "00000000-0000-4000-8000-0000000000ff")
        );
        // 입력값: 많이 쓴 것부터, 비밀로 보이는 셋은 빠진다.
        let names: Vec<&str> = read
            .book
            .entries
            .iter()
            .map(|one| one.name.as_str())
            .collect();
        assert_eq!(names, ["nickname", "company"], "{names:?}");
        // 카드 두 장 + 비밀 입력값 셋(카드 번호 꼴, cvc, password).
        assert_eq!(read.held_back, 5);
    }

    /// 입력값의 비밀은 값의 종류로 가른다: 띄어 쓴 영문 주소와 건물 동이 든
    /// 한글 주소는 길고 숫자와 글자가 섞여도 옮겨지고, 비밀번호·카드·인증번호
    /// 칸의 값과 토큰 꼴 한 낱말은 남는다.
    #[test]
    fn an_entry_is_secret_by_its_kind_and_a_written_address_is_not() {
        let written = [
            ("address", "[합성] 1200 Example Parkway Building 43"),
            ("addr2", "[합성-도로명] 123 A동 1203호"),
            ("street", "[합성]테헤란로123A동"),
            ("nickname", "[합성-별명]"),
        ];
        for (name, value) in written {
            assert!(!entry_is_secret(name, value), "{name} was held back");
        }
        // 토큰 꼴 값은 시험이 실행 중에 짓는다 — 저장소에 글자로 서지 않게.
        let vendor_shaped = format!("{}{}", "ghp_", "a1".repeat(18));
        let opaque = "a1b2c3d4e5f6g7h8i9j0k1l2m3".to_string();
        let secret = [
            ("password", "[합성-비밀번호]".to_string()),
            ("card_cvc", "[합성-보안코드]".to_string()),
            ("cardNumber", "[합성-카드]".to_string()),
            ("cc-exp", "[합성-만료]".to_string()),
            ("otp", "[합성-코드]".to_string()),
            ("sms_verification", "[합성-코드]".to_string()),
            ("인증번호", "[합성-코드]".to_string()),
            ("q", vendor_shaped),
            ("q", opaque),
        ];
        for (name, value) in &secret {
            assert!(entry_is_secret(name, value), "{name} was moved");
        }

        // 읽는 길 전체: 띄어 쓴 주소 줄 한 행은 입력값으로 옮겨지고 남는 수는
        // 그대로다(카드 두 장 + cvc + password).
        let (_yard, path) = web_data(WEB_DATA_FIXTURE);
        rusqlite::Connection::open(&path)
            .expect("open")
            .execute(
                "INSERT INTO autofill VALUES('address', ?1, ?1, 1, 50, 2)",
                [written[0].1],
            )
            .expect("written address entry");
        let read = read_chromium_web_data(&path).expect("reads");
        assert!(
            read.book
                .entries
                .iter()
                .any(|entry| entry.name == "address" && entry.value == written[0].1),
            "{:?}",
            read.book.entries
        );
        assert_eq!(read.held_back, 4);
    }

    /// 한 낱말 값의 비밀 표식(`sk-`·`AKIA`·`eyJ` …)은 낱말의 가장자리에 설 때
    /// 만 센다. 대륙 이름 「Asia」(ASIA)나 「desk-lamp」(sk-)처럼 낱말 가운데
    /// 표식이 우연히 든 값은 옮겨지고, 표식 뒤에 토큰 꼴이 이어지는 값은 남는다
    /// (t-84200). 토큰 꼴 값은 시험이 실행 중에 짓는다.
    #[test]
    fn a_one_word_value_is_secret_by_a_mark_at_its_edge_not_one_inside_a_word() {
        for value in [
            "Malaysia",
            "Asia",
            "Asian",
            "Asia-Pacific",
            "desk-lamp",
            "task_sk-list",
            "xoxo",
        ] {
            assert!(!entry_is_secret("q", value), "{value} was held back");
        }
        let held = [
            format!("AKIA{}", "Q7".repeat(8)),
            format!("akia{}", "q7".repeat(8)),
            format!("sk-{}", "a1".repeat(12)),
            format!("sk_{}", "a1".repeat(12)),
            format!("eyJ{}", "hGci".repeat(5)),
            format!("xoxb-{}", "12".repeat(6)),
            format!("key=sk-{}", "a1".repeat(12)),
        ];
        for value in &held {
            assert!(entry_is_secret("q", value), "{value} was moved");
        }
    }

    /// 띄어 쓰거나 한글이 든 값 안의 토큰도 남는다: 값을 공백으로 나눠 조각마다
    /// 표식 규칙을 본다. 띄어 쓴 주소 속 「Asia」는 여전히 옮겨진다 (t-84200 r2).
    #[test]
    fn a_key_inside_a_spaced_or_korean_value_is_held_back_and_an_address_word_is_not() {
        let aws = format!("AKIA{}", "Q7".repeat(8));
        let jwt = format!("eyJ{}", "hGci".repeat(5));
        let held = [
            format!("Bearer {jwt}"),
            format!("키:{aws}"),
            format!("[합성] {aws} 끝"),
        ];
        for value in &held {
            assert!(entry_is_secret("q", value), "{value} was moved");
        }
        for value in [
            "[합성] 12 Asia-Pacific Avenue",
            "[합성] Asian Tower 43",
            "[합성-주소] 책상 desk-lamp 3층",
        ] {
            assert!(!entry_is_secret("q", value), "{value} was held back");
        }
    }

    /// 옛 판의 Web Data: 주소 칸은 autofill_profiles에, 이름·이메일·전화는 곁표에.
    #[test]
    fn a_legacy_web_data_reads_its_side_tables() {
        let (_yard, path) = web_data(WEB_DATA_LEGACY_FIXTURE);
        let read = read_chromium_web_data(&path).expect("reads");
        assert_eq!(read.book.addresses.len(), 1);
        let only = &read.book.addresses[0];
        for kind in [
            "given-name",
            "family-name",
            "email",
            "tel",
            "street-address",
            "address-level2",
            "postal-code",
            "country",
        ] {
            assert!(only.fields.contains_key(kind), "{kind} was not read");
        }
        assert_eq!(read.held_back, 0);
        assert!(read.book.entries.is_empty());
    }

    /// 가져온 자료는 프로필마다 저장소에 앉고, 다시 읽으면 같은 자료이며, 수가
    /// 곁파일에 남는다. 프로필을 잊으면 셋이 함께 간다.
    #[test]
    fn the_store_keeps_what_was_read_and_forgets_it_with_the_profile() {
        let root = tempfile::tempdir().expect("root");
        let id = "a".repeat(32);
        assert!(load_bookmarks(root.path(), &id).is_empty());
        assert_eq!(load_address_book(root.path(), &id), AddressBook::default());
        assert!(last_imported(root.path(), &id).is_none());

        let bookmarks = parse_chromium_bookmarks(BOOKMARKS_FIXTURE).expect("parses");
        let (_yard, path) = current_web_data();
        let read = read_chromium_web_data(&path).expect("reads");
        let counts = save_imported(
            root.path(),
            &id,
            Some(&bookmarks),
            Some(&read),
            1_793_200_000,
        )
        .expect("saves");
        assert_eq!(
            counts,
            ImportedData {
                bookmarks: 6,
                addresses: 2,
                autofill_entries: 2,
                held_back: 5,
                at_unix: 1_793_200_000,
            }
        );
        let line = import_log_line(Some(&bookmarks), Some(&read));
        assert!(
            line.contains("bookmarks 6 (not web addresses 2, over the cap 0)")
                && line.contains("addresses 2")
                && line.contains("held back 5")
                && line.contains("schema 134"),
            "{line}"
        );
        assert!(
            !line.contains("합성") && !line.contains("example"),
            "{line}"
        );
        assert_eq!(load_bookmarks(root.path(), &id), bookmarks.bookmarks);
        assert_eq!(load_address_book(root.path(), &id), read.book);
        assert_eq!(last_imported(root.path(), &id), Some(counts.clone()));

        // 한 몫을 읽지 못한 가져오기(None)는 그 몫의 저장된 자료와 수를 건드리지
        // 않고, 읽은 몫만 갈아 끼운다.
        let fewer = BookmarksRead::default();
        let partial = save_imported(root.path(), &id, Some(&fewer), None, 1_793_200_001)
            .expect("saves a part");
        assert_eq!(
            partial,
            ImportedData {
                bookmarks: 0,
                at_unix: 1_793_200_001,
                ..counts
            }
        );
        assert_eq!(load_address_book(root.path(), &id), read.book);
        assert!(load_bookmarks(root.path(), &id).is_empty());
        let line = import_log_line(None, Some(&read));
        assert!(
            line.contains("bookmarks not read") && line.contains("addresses 2"),
            "{line}"
        );
        save_imported(
            root.path(),
            &id,
            Some(&bookmarks),
            Some(&read),
            1_793_200_000,
        )
        .expect("restored");
        let wire = serde_json::to_value(load_bookmarks(root.path(), &id)).expect("wire");
        assert_eq!(wire[0]["root"], "bar");
        assert!(
            wire[0].get("folder").is_none(),
            "an unfoldered bookmark wrote a folder"
        );
        assert_eq!(wire[1]["folder"], "[합성-폴더]");
        let wire = serde_json::to_value(last_imported(root.path(), &id)).expect("wire");
        for key in [
            "bookmarks",
            "addresses",
            "autofillEntries",
            "heldBack",
            "atUnix",
        ] {
            assert!(wire.get(key).is_some(), "{key} is not on the wire: {wire}");
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(address_book_file(root.path(), &id))
                .expect("meta")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600, "the address book is readable by others");
        }

        discard(root.path(), &id);
        assert!(!bookmarks_file(root.path(), &id).exists());
        assert!(!address_book_file(root.path(), &id).exists());
        assert!(!imported_file(root.path(), &id).exists());
    }

    /// 한 몫만 읽힌 가져오기: 읽은 몫은 옮겨지고 수가 서며, 못 읽은 몫만 까닭이
    /// 카드로 간다. 둘 다 못 읽으면 수가 없다. 로그에는 값이 없다(t-84200).
    #[test]
    fn each_part_of_an_import_says_its_own_reason() {
        let root = tempfile::tempdir().expect("root");
        let id = "b".repeat(32);
        let (_yard, path) = current_web_data();
        let web = read_chromium_web_data(&path).expect("reads");
        let mut lines = Vec::new();
        let (counts, skipped) = import_profile_data(
            ProfileDataRead {
                bookmarks: Err(ProfileDataError {
                    kind: ProfileDataSkipped::Unreadable,
                    detail: "bad json".into(),
                }),
                web_data: Ok(web),
            },
            root.path(),
            &id,
            7,
            |line| lines.push(line.to_string()),
        );
        let counts = counts.expect("the addresses were carried");
        assert_eq!((counts.bookmarks, counts.addresses), (0, 2));
        assert_eq!(
            skipped,
            Some(ProfileDataSkippedParts {
                bookmarks: Some(ProfileDataSkipped::Unreadable),
                addresses: None
            })
        );
        assert_eq!(
            serde_json::to_value(skipped).expect("wire"),
            serde_json::json!({ "bookmarks": "unreadable" })
        );
        assert_eq!(load_address_book(root.path(), &id).addresses.len(), 2);
        assert!(lines.iter().all(|line| !line.contains("합성")), "{lines:?}");

        let (counts, skipped) = import_profile_data(
            ProfileDataRead {
                bookmarks: Err(ProfileDataError {
                    kind: ProfileDataSkipped::Unreadable,
                    detail: "bad json".into(),
                }),
                web_data: Err(ProfileDataError {
                    kind: ProfileDataSkipped::Locked,
                    detail: "busy".into(),
                }),
            },
            root.path(),
            &id,
            8,
            |_| {},
        );
        assert!(counts.is_none());
        assert_eq!(
            skipped,
            Some(ProfileDataSkippedParts {
                bookmarks: Some(ProfileDataSkipped::Unreadable),
                addresses: Some(ProfileDataSkipped::Locked)
            })
        );
        // 못 읽은 가져오기는 지난번 자료를 지우지 않는다.
        assert_eq!(load_address_book(root.path(), &id).addresses.len(), 2);
    }

    /// 다시 가져올 때 못 읽은 몫의 수는 카드에 0으로 간다 — 이번 누름에 옮긴
    /// 것이 아니다. 저장 파일과 `last_imported`는 지난 수를 그대로 둔다 (t-84200 r2).
    #[test]
    fn a_part_not_read_this_time_counts_zero_on_the_card_and_keeps_its_last_count() {
        let root = tempfile::tempdir().expect("root");
        let id = "c".repeat(32);
        let (_yard, path) = current_web_data();
        let bookmarks = || parse_chromium_bookmarks(BOOKMARKS_FIXTURE).expect("fixture parses");
        let (first, none) = import_profile_data(
            ProfileDataRead {
                bookmarks: Ok(bookmarks()),
                web_data: Ok(read_chromium_web_data(&path).expect("reads")),
            },
            root.path(),
            &id,
            1,
            |_| {},
        );
        let first = first.expect("both parts were carried");
        assert!(none.is_none());
        assert_eq!((first.bookmarks, first.addresses), (6, 2));
        assert!(first.held_back > 0, "the fixture holds a card back");

        // 북마크가 깨진 채 다시: 카드의 북마크는 0, 주소는 이번 수.
        let (counts, skipped) = import_profile_data(
            ProfileDataRead {
                bookmarks: Err(ProfileDataError {
                    kind: ProfileDataSkipped::Unreadable,
                    detail: "bad json".into(),
                }),
                web_data: Ok(read_chromium_web_data(&path).expect("reads")),
            },
            root.path(),
            &id,
            2,
            |_| {},
        );
        let counts = counts.expect("the addresses were carried");
        assert_eq!(counts.bookmarks, 0, "no bookmark was carried this time");
        assert_eq!(counts.addresses, 2);
        assert_eq!(
            skipped.and_then(|parts| parts.bookmarks),
            Some(ProfileDataSkipped::Unreadable)
        );
        let kept = last_imported(root.path(), &id).expect("kept");
        assert_eq!((kept.bookmarks, kept.addresses), (6, 2));

        // Web Data가 잠긴 채 다시: 주소·입력값·보류는 0, 북마크는 이번 수.
        let (counts, skipped) = import_profile_data(
            ProfileDataRead {
                bookmarks: Ok(bookmarks()),
                web_data: Err(ProfileDataError {
                    kind: ProfileDataSkipped::Locked,
                    detail: "busy".into(),
                }),
            },
            root.path(),
            &id,
            3,
            |_| {},
        );
        let counts = counts.expect("the bookmarks were carried");
        assert_eq!(counts.bookmarks, 6);
        assert_eq!(
            (counts.addresses, counts.autofill_entries, counts.held_back),
            (0, 0, 0),
            "nothing of Web Data was read this time"
        );
        assert_eq!(
            skipped.and_then(|parts| parts.addresses),
            Some(ProfileDataSkipped::Locked)
        );
        let kept = last_imported(root.path(), &id).expect("kept");
        assert_eq!((kept.addresses, kept.held_back), (2, first.held_back));
        assert_eq!(kept.at_unix, 3);
    }

    /// 저장이 실패하면 읽은 몫만 `Unsaved`이고, 못 읽은 몫은 제 까닭을 둔다.
    #[test]
    fn a_failed_save_blames_only_the_parts_that_were_read() {
        let root = tempfile::tempdir().expect("root");
        let not_a_folder = root.path().join("file");
        std::fs::write(&not_a_folder, b"x").expect("file");
        let (_yard, path) = current_web_data();
        let (counts, skipped) = import_profile_data(
            ProfileDataRead {
                bookmarks: Err(ProfileDataError {
                    kind: ProfileDataSkipped::Unreadable,
                    detail: "bad json".into(),
                }),
                web_data: Ok(read_chromium_web_data(&path).expect("reads")),
            },
            &not_a_folder,
            &"d".repeat(32),
            1,
            |_| {},
        );
        assert!(counts.is_none());
        assert_eq!(
            skipped,
            Some(ProfileDataSkippedParts {
                bookmarks: Some(ProfileDataSkipped::Unreadable),
                addresses: Some(ProfileDataSkipped::Unsaved)
            })
        );
    }

    /// 원본 폴더에서 읽기: 북마크와 Web Data가 없으면 빈 몫이고, 있으면
    /// Web Data는 사본에서 읽혀 사본 폴더는 남지 않는다.
    #[test]
    fn a_source_profile_reads_from_its_own_folder_through_a_snapshot() {
        let home = tempfile::tempdir().expect("home");
        let scratch = home.path().join("scratch");
        let absent = read_chromium_profile_data("chrome", "Default", home.path(), &scratch);
        let (bookmarks, web) = (
            absent.bookmarks.expect("an absent profile reads as empty"),
            absent.web_data.expect("an absent profile reads as empty"),
        );
        assert!(bookmarks.bookmarks.is_empty() && web.book.addresses.is_empty());

        // 뿌리가 환경(LOCALAPPDATA·XDG_CONFIG_HOME)에서 오는 OS에서는 시험이
        // 임시 홈 밖에 쓰지 않는다.
        let Some(folder) =
            crate::browser_cookie_import::chromium_profile_dir("chrome", "Default", home.path())
                .filter(|folder| folder.starts_with(home.path()))
        else {
            return;
        };
        std::fs::create_dir_all(&folder).expect("folder");
        std::fs::write(folder.join(BOOKMARKS_FILE), BOOKMARKS_FIXTURE).expect("bookmarks");
        rusqlite::Connection::open(folder.join(WEB_DATA_FILE))
            .expect("create")
            .execute_batch(WEB_DATA_FIXTURE)
            .expect("fixture schema");
        let read = read_chromium_profile_data("chrome", "Default", home.path(), &scratch);
        let (bookmarks, web) = (
            read.bookmarks.expect("reads"),
            read.web_data.expect("reads"),
        );
        assert_eq!(bookmarks.bookmarks.len(), 6);
        assert_eq!(web.book.addresses.len(), 2);
        assert!(!scratch.exists(), "the snapshot outlived the read");
        // 프로필 이름은 바깥 데이터다 — 길을 품은 이름은 읽지 않는다.
        assert!(
            read_chromium_profile_data("chrome", "../Default", home.path(), &scratch)
                .bookmarks
                .expect("fenced")
                .bookmarks
                .is_empty()
        );
        // 두 몫은 따로 읽는다: 깨진 북마크 파일은 북마크만 못 읽게 하고 주소는
        // 그대로 읽힌다(t-84200).
        std::fs::write(folder.join(BOOKMARKS_FILE), "{not json").expect("broken");
        let broken = read_chromium_profile_data("chrome", "Default", home.path(), &scratch);
        let skipped = broken.bookmarks.expect_err("a broken Bookmarks file reads");
        assert_eq!(skipped.kind, ProfileDataSkipped::Unreadable);
        assert_eq!(
            broken
                .web_data
                .expect("the addresses still read")
                .book
                .addresses
                .len(),
            2
        );
        // 카드에는 종류 낱말 하나만 간다 — 글자는 로그 몫이다.
        assert_eq!(
            serde_json::to_value(skipped.kind).expect("wire"),
            "unreadable"
        );
        // 거꾸로 Web Data가 깨져도 북마크는 읽힌다.
        std::fs::write(folder.join(BOOKMARKS_FILE), BOOKMARKS_FIXTURE).expect("bookmarks");
        std::fs::write(folder.join(WEB_DATA_FILE), "not a database").expect("broken");
        let broken = read_chromium_profile_data("chrome", "Default", home.path(), &scratch);
        assert_eq!(
            broken
                .bookmarks
                .expect("the bookmarks still read")
                .bookmarks
                .len(),
            6
        );
        assert!(broken.web_data.is_err());
        assert!(!scratch.exists(), "the snapshot outlived the failed read");
        assert_eq!(
            serde_json::to_value(ProfileDataSkipped::Locked).expect("wire"),
            "locked"
        );
    }
}
