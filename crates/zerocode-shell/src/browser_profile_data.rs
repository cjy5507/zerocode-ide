//! 크롬 프로필의 북마크 — 프로필 가져오기의 둘째 조각 (t-79331).
//!
//! 쿠키는 판이 태어날 때 저장소에 앉아야 해서 스테이징을 지나지만
//! ([`crate::browser_cookie_import`]), 북마크는 창이 직접 읽는 자료다: 주소줄
//! 제안과 북마크 줄이 읽는다. 그래서 프로필마다 파일로 앱 저장소
//! (`config_root`)에 둔다 — 주인만 읽는 권한으로(`durable_file::replace_bytes`).
//! 원본은 프로필 폴더의 `Bookmarks` JSON이다.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Chromium 프로필 폴더 안의 원본.
pub const BOOKMARKS_FILE: &str = "Bookmarks";

/// 한 프로필에 두는 북마크의 상한 — 주소줄 제안이 글자마다 훑는 목록이다.
const BOOKMARKS_KEPT: usize = 20_000;

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
/// 판이 열 수 없어 옮기지 않고 센다.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct BookmarksRead {
    pub bookmarks: Vec<Bookmark>,
    pub skipped: usize,
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
        serde_json::from_str(text).map_err(|_| "북마크 파일을 읽을 수 없습니다".to_string())?;
    let roots = parsed
        .get("roots")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| "북마크 파일에 뿌리가 없습니다".to_string())?;
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
                if !is_web_address(url) || read.bookmarks.len() >= BOOKMARKS_KEPT {
                    read.skipped += 1;
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

/* ---- 프로필 저장소 ---- */

/// 한 프로필로 옮긴 자료의 수 — 가져오기 카드와 설정 카드의 「북마크 J」.
/// 수만 든다.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ImportedData {
    pub bookmarks: usize,
    /// 가져온 시각(unix 초).
    pub at_unix: i64,
}

pub fn bookmarks_file(config_root: &Path, profile_id: &str) -> PathBuf {
    config_root.join(format!("browser-bookmarks-{profile_id}.json"))
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

/// 한 프로필로 마지막에 옮긴 자료의 수. 옮긴 적이 없으면 없다.
pub fn last_imported(config_root: &Path, profile_id: &str) -> Option<ImportedData> {
    let text = std::fs::read_to_string(imported_file(config_root, profile_id)).ok()?;
    serde_json::from_str(&text).ok()
}

/// 프로필을 잊을 때 그 자료를 함께 거둔다.
pub fn discard(config_root: &Path, profile_id: &str) {
    for path in [
        bookmarks_file(config_root, profile_id),
        imported_file(config_root, profile_id),
    ] {
        let _ = std::fs::remove_file(path);
    }
}

/// 읽은 자료를 대상 프로필의 저장소에 둔다. 새 가져오기는 옛 자료를 갈아
/// 끼운다 — 원본 브라우저가 진실이다.
pub fn save_imported(
    config_root: &Path,
    profile_id: &str,
    bookmarks: &BookmarksRead,
    at_unix: i64,
) -> Result<ImportedData, String> {
    save_private(
        &bookmarks_file(config_root, profile_id),
        &bookmarks.bookmarks,
    )?;
    let counts = ImportedData {
        bookmarks: bookmarks.bookmarks.len(),
        at_unix,
    };
    save_private(&imported_file(config_root, profile_id), &counts)?;
    Ok(counts)
}

/// 한 Chromium 프로필의 북마크를 읽는다. 원본이 없으면 빈 몫이다 — 북마크를
/// 하나도 만든 적 없는 프로필도 흔하다.
pub fn read_chromium_profile_data(
    family: &str,
    profile: &str,
    home: &Path,
) -> Result<BookmarksRead, String> {
    match crate::browser_cookie_import::chromium_profile_file(family, profile, home, BOOKMARKS_FILE)
    {
        Some(path) => {
            let text = std::fs::read_to_string(&path).map_err(|error| error.to_string())?;
            parse_chromium_bookmarks(&text)
        }
        None => Ok(BookmarksRead::default()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOOKMARKS_FIXTURE: &str =
        include_str!("../../../tests/fixtures/browser-profile/Bookmarks");

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

    #[test]
    fn a_bookmarks_file_that_is_not_one_is_refused() {
        assert!(parse_chromium_bookmarks("{not json").is_err());
        assert!(parse_chromium_bookmarks("{}").is_err());
        let empty = parse_chromium_bookmarks(r#"{"roots":{}}"#).expect("no roots is empty");
        assert_eq!(empty, BookmarksRead::default());
    }

    /// 가져온 북마크는 프로필마다 저장소에 앉고, 다시 읽으면 같은 목록이며,
    /// 수가 곁파일에 남는다. 프로필을 잊으면 함께 간다.
    #[test]
    fn the_store_keeps_what_was_read_and_forgets_it_with_the_profile() {
        let root = tempfile::tempdir().expect("root");
        let id = "a".repeat(32);
        assert!(load_bookmarks(root.path(), &id).is_empty());
        assert!(last_imported(root.path(), &id).is_none());

        let bookmarks = parse_chromium_bookmarks(BOOKMARKS_FIXTURE).expect("parses");
        let counts = save_imported(root.path(), &id, &bookmarks, 1_793_200_000).expect("saves");
        assert_eq!(
            counts,
            ImportedData {
                bookmarks: 6,
                at_unix: 1_793_200_000,
            }
        );
        assert_eq!(load_bookmarks(root.path(), &id), bookmarks.bookmarks);
        assert_eq!(last_imported(root.path(), &id), Some(counts));
        let wire = serde_json::to_value(load_bookmarks(root.path(), &id)).expect("wire");
        assert_eq!(wire[0]["root"], "bar");
        assert!(
            wire[0].get("folder").is_none(),
            "an unfoldered bookmark wrote a folder"
        );
        assert_eq!(wire[1]["folder"], "[합성-폴더]");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(bookmarks_file(root.path(), &id))
                .expect("meta")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600, "the bookmarks are readable by others");
        }

        discard(root.path(), &id);
        assert!(!bookmarks_file(root.path(), &id).exists());
        assert!(!imported_file(root.path(), &id).exists());
    }

    /// 원본 폴더에서 읽기: 북마크 파일이 없으면 빈 몫이고, 프로필 이름이 길을
    /// 품으면 읽지 않는다.
    #[test]
    fn a_source_profile_reads_from_its_own_folder() {
        let home = tempfile::tempdir().expect("home");
        let read = read_chromium_profile_data("chrome", "Default", home.path())
            .expect("an absent profile reads as empty");
        assert!(read.bookmarks.is_empty());

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
        let read = read_chromium_profile_data("chrome", "Default", home.path()).expect("reads");
        assert_eq!(read.bookmarks.len(), 6);
        assert!(
            read_chromium_profile_data("chrome", "../Default", home.path())
                .expect("fenced")
                .bookmarks
                .is_empty()
        );
        std::fs::write(folder.join(BOOKMARKS_FILE), "{not json").expect("broken");
        assert!(read_chromium_profile_data("chrome", "Default", home.path()).is_err());
    }
}
