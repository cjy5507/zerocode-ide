//! 쿠키 가져오기의 시스템 손 (지시서 C2 — `docs/plans/browser-cookie-import.md`).
//!
//! 판단(복호·정책·파서·시각)은 전부 [`crate::browser_cookies`]의 것이고(C1),
//! 여기는 그 판단에 **재료를 대는 손**이다: 설치된 브라우저를 찾고, 잠긴
//! SQLite를 안정된 사본으로 뜨고, 행을 읽고, OS의 열쇠 의식을 치른다.
//! 원본: Orca `browser-cookie-import.ts`(감지 `:141-377`, 읽기 `:1660-1780`,
//! 열쇠 `:974-1105`)와 `chromium-cookie-snapshot.ts`(안정 사본).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::browser_cookies::{self, DecryptOutcome, EncryptionKeys, SameSite, ValidatedCookie};

/* ---- 감지: 어떤 브라우저가 이 기계에 있는가 ---- */

/// Chromium 계열 여섯의 정체 — 열쇠 이름과 세 OS의 뿌리
/// (browser-cookie-import.ts:145-198; Comet은 Linux 빌드가 없고 Helium은
/// macOS만 확인됐다는 원본의 생략까지 그대로).
struct ChromiumBrowserDef {
    family: &'static str,
    label: &'static str,
    keychain_service: &'static str,
    keychain_account: &'static str,
    mac_root: Option<&'static str>,
    // 이 빌드가 아닌 OS의 뿌리는 cfg 갈래 밖에서 죽은 글자로 보인다 —
    // 표는 세 OS 것을 다 들고, 읽는 갈래만 컴파일된다.
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    win_root: Option<&'static str>,
    #[cfg_attr(any(target_os = "macos", target_os = "windows"), allow(dead_code))]
    linux_root: Option<&'static str>,
}

const CHROMIUM_BROWSERS: [ChromiumBrowserDef; 6] = [
    ChromiumBrowserDef {
        family: "chrome",
        label: "Google Chrome",
        keychain_service: "Chrome Safe Storage",
        keychain_account: "Chrome",
        mac_root: Some("Google/Chrome"),
        win_root: Some("Google/Chrome/User Data"),
        linux_root: Some("google-chrome"),
    },
    ChromiumBrowserDef {
        family: "edge",
        label: "Microsoft Edge",
        keychain_service: "Microsoft Edge Safe Storage",
        keychain_account: "Microsoft Edge",
        mac_root: Some("Microsoft Edge"),
        win_root: Some("Microsoft/Edge/User Data"),
        linux_root: Some("microsoft-edge"),
    },
    ChromiumBrowserDef {
        family: "arc",
        label: "Arc",
        keychain_service: "Arc Safe Storage",
        keychain_account: "Arc",
        mac_root: Some("Arc/User Data"),
        win_root: None,
        linux_root: None,
    },
    ChromiumBrowserDef {
        family: "chromium",
        label: "Brave",
        keychain_service: "Brave Safe Storage",
        keychain_account: "Brave",
        mac_root: Some("BraveSoftware/Brave-Browser"),
        win_root: Some("BraveSoftware/Brave-Browser/User Data"),
        linux_root: Some("BraveSoftware/Brave-Browser"),
    },
    ChromiumBrowserDef {
        family: "comet",
        label: "Comet",
        keychain_service: "Comet Safe Storage",
        keychain_account: "Comet",
        mac_root: Some("Comet"),
        win_root: Some("Comet/User Data"),
        linux_root: None,
    },
    ChromiumBrowserDef {
        family: "helium",
        // Helium은 '<Browser> Safe Storage' 관례를 깬다 — 서비스명이
        // 문자 그대로 'Helium Storage Key'다 (원본 주석 그대로).
        label: "Helium",
        keychain_service: "Helium Storage Key",
        keychain_account: "Helium",
        mac_root: Some("net.imput.helium"),
        win_root: None,
        linux_root: None,
    },
];

/// Chromium 표 밖의 둘 — 이름이 두 곳에서 따로 적히지 않도록 상수로.
const FIREFOX_LABEL: &str = "Firefox";
const SAFARI_LABEL: &str = "Safari";

/// 한 가족의 사람이 읽는 이름.
///
/// 감지 결과가 아니라 **표**가 답한다. 어디에서 가져왔는지는 가져온 뒤에도
/// 남아야 하는데, 그때 소스 브라우저는 이미 지워졌을 수 있기 때문이다 —
/// 그러면 `detect_installed_browsers`는 그 가족을 더 이상 세지 않는다.
pub fn family_label(family: &str) -> Option<&'static str> {
    match family {
        "firefox" => Some(FIREFOX_LABEL),
        "safari" => Some(SAFARI_LABEL),
        _ => CHROMIUM_BROWSERS
            .iter()
            .find(|def| def.family == family)
            .map(|def| def.label),
    }
}

/// 창의 가져오기 메뉴가 읽는 한 소스 — 브라우저 하나와 그 프로필들.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CookieSource {
    pub family: String,
    pub label: String,
    pub profiles: Vec<CookieSourceProfile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CookieSourceProfile {
    pub name: String,
    pub directory: String,
}

/// OS별 뿌리 — 환경은 인자로 받아 시험 가능하게 (`browserRootPath`,
/// `:200-224`).
fn chromium_root(def: &ChromiumBrowserDef, home: &Path) -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        def.mac_root
            .map(|tail| home.join("Library/Application Support").join(tail))
    }
    #[cfg(target_os = "windows")]
    {
        let local = std::env::var_os("LOCALAPPDATA")?;
        let _ = home;
        def.win_root.map(|tail| PathBuf::from(local).join(tail))
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let config = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".config"));
        def.linux_root.map(|tail| config.join(tail))
    }
}

/// 프로필 디렉터리 이름은 Local State에서 온 바깥 데이터인데 경로 조각이
/// 된다 — 담장 먼저 (`isSafeBrowserProfileDirectory`, `:226-236`).
fn is_safe_profile_directory(directory: &str) -> bool {
    !directory.is_empty()
        && directory != "."
        && directory != ".."
        && !directory.contains('\u{0}')
        && !directory.contains('/')
        && !directory.contains('\\')
}

/// Local State의 `profile.info_cache`에서 프로필들을 (`discoverProfiles`,
/// `:238-263`) — 없으면 Default 하나가 답이다.
fn discover_chromium_profiles(root: &Path) -> Vec<CookieSourceProfile> {
    let fallback = vec![CookieSourceProfile {
        name: "Default".into(),
        directory: "Default".into(),
    }];
    let Ok(raw) = std::fs::read_to_string(root.join("Local State")) else {
        return fallback;
    };
    let Ok(state) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return fallback;
    };
    let Some(cache) = state
        .get("profile")
        .and_then(|profile| profile.get("info_cache"))
        .and_then(|cache| cache.as_object())
    else {
        return fallback;
    };
    let mut profiles: Vec<CookieSourceProfile> = cache
        .iter()
        .filter(|(directory, _)| is_safe_profile_directory(directory))
        .map(|(directory, info)| CookieSourceProfile {
            name: info
                .get("name")
                .and_then(|name| name.as_str())
                .unwrap_or(directory)
                .to_string(),
            directory: directory.clone(),
        })
        .collect();
    if profiles.is_empty() {
        return fallback;
    }
    profiles.sort_by(|a, b| a.directory.cmp(&b.directory));
    profiles
}

/// Firefox 프로필 뿌리 (`firefoxProfilesRoot`, `:269-281`).
fn firefox_profiles_root(home: &Path) -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        home.join("Library/Application Support/Firefox/Profiles")
    }
    #[cfg(target_os = "windows")]
    {
        std::env::var_os("APPDATA")
            .map(|appdata| PathBuf::from(appdata).join("Mozilla/Firefox/Profiles"))
            .unwrap_or_else(|| home.join("Mozilla/Firefox/Profiles"))
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        home.join(".mozilla/firefox")
    }
}

/// Firefox 프로필들 — `<random>.<name>` 꼴, default-release가 앞장선다
/// (`discoverFirefoxProfiles`, `:282-317`).
fn discover_firefox_profiles(profiles_root: &Path) -> Vec<CookieSourceProfile> {
    let Ok(entries) = std::fs::read_dir(profiles_root) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect();
    names.sort_by_key(|name| {
        if name.contains("default-release") {
            0
        } else if name.contains("default") {
            1
        } else {
            2
        }
    });
    names
        .into_iter()
        .map(|directory| CookieSourceProfile {
            name: directory
                .split_once('.')
                .map(|(_, tail)| tail.to_string())
                .unwrap_or_else(|| directory.clone()),
            directory,
        })
        .collect()
}

/// Safari의 두 자리 (`detectSafari`, `:346-377`) — 컨테이너 안이 먼저다.
fn safari_cookie_paths(home: &Path) -> [PathBuf; 2] {
    [
        home.join("Library/Containers/com.apple.Safari/Data/Library/Cookies/Cookies.binarycookies"),
        home.join("Library/Cookies/Cookies.binarycookies"),
    ]
}

/// 이 기계에 서 있는 소스들 (`detectInstalledBrowsers`, `:378-416`).
pub fn detect_installed_browsers(home: &Path) -> Vec<CookieSource> {
    let mut sources = Vec::new();
    for def in &CHROMIUM_BROWSERS {
        let Some(root) = chromium_root(def, home) else {
            continue;
        };
        if !root.is_dir() {
            continue;
        }
        sources.push(CookieSource {
            family: def.family.to_string(),
            label: def.label.to_string(),
            profiles: discover_chromium_profiles(&root),
        });
    }
    let firefox_root = firefox_profiles_root(home);
    let firefox_profiles: Vec<CookieSourceProfile> = discover_firefox_profiles(&firefox_root)
        .into_iter()
        .filter(|profile| {
            firefox_root
                .join(&profile.directory)
                .join("cookies.sqlite")
                .is_file()
        })
        .collect();
    if !firefox_profiles.is_empty() {
        sources.push(CookieSource {
            family: "firefox".into(),
            label: FIREFOX_LABEL.into(),
            profiles: firefox_profiles,
        });
    }
    if safari_cookie_paths(home).iter().any(|path| path.is_file()) {
        sources.push(CookieSource {
            family: "safari".into(),
            label: SAFARI_LABEL.into(),
            profiles: Vec::new(),
        });
    }
    sources
}

/// 한 소스의 쿠키 파일 경로 — 읽기 직전에 다시 푸는 단일 장소.
pub fn cookie_database_path(family: &str, profile: &str, home: &Path) -> Option<PathBuf> {
    match family {
        "firefox" => {
            if !is_safe_profile_directory(profile) {
                return None;
            }
            Some(
                firefox_profiles_root(home)
                    .join(profile)
                    .join("cookies.sqlite"),
            )
        }
        "safari" => safari_cookie_paths(home)
            .into_iter()
            .find(|path| path.is_file()),
        _ => {
            let def = CHROMIUM_BROWSERS.iter().find(|def| def.family == family)?;
            if !is_safe_profile_directory(profile) {
                return None;
            }
            // Chromium은 프로필 바로 아래 또는 Network/ 아래에 둔다 —
            // 버전에 따라 갈리므로 서 있는 쪽이 답이다 (`chromium-cookie-
            // path.ts`).
            let base = chromium_root(def, home)?.join(profile);
            let network = base.join("Network/Cookies");
            if network.is_file() {
                return Some(network);
            }
            let flat = base.join("Cookies");
            flat.is_file().then_some(flat)
        }
    }
}

pub fn keychain_names(family: &str) -> Option<(&'static str, &'static str)> {
    CHROMIUM_BROWSERS
        .iter()
        .find(|def| def.family == family)
        .map(|def| (def.keychain_service, def.keychain_account))
}

/* ---- 사본: 잠긴 SQLite를 안정되게 뜨기 ---- */

/// 복사 전후의 파일 상태 — 같아야 그 사본이 한 시점의 것이다
/// (`chromium-cookie-snapshot.ts`의 FileState, dev/ino/size/mtime).
#[derive(PartialEq)]
struct FileState {
    len: u64,
    modified: Option<std::time::SystemTime>,
}

fn file_state(path: &Path) -> Option<FileState> {
    let meta = std::fs::metadata(path).ok()?;
    Some(FileState {
        len: meta.len(),
        modified: meta.modified().ok(),
    })
}

const SNAPSHOT_ATTEMPTS: usize = 5;

/// 브라우저가 쥔 채로 움직이는 DB를, 본체와 -wal을 함께 복사하고 전후
/// 상태가 같을 때만 사본으로 인정한다 — 다섯 번까지
/// (`copyStableAttempt`, SNAPSHOT_ATTEMPTS=5).
pub fn snapshot_sqlite(source: &Path, temp_dir: &Path) -> Result<PathBuf, String> {
    std::fs::create_dir_all(temp_dir).map_err(|error| error.to_string())?;
    let target = temp_dir.join("Cookies");
    for _ in 0..SNAPSHOT_ATTEMPTS {
        let before = file_state(source);
        let before_wal = file_state(&wal_of(source));
        if before.is_none() {
            return Err("쿠키 데이터베이스가 없습니다".into());
        }
        let _ = std::fs::remove_file(&target);
        let _ = std::fs::remove_file(wal_of(&target));
        if std::fs::copy(source, &target).is_err() {
            continue;
        }
        let wal = wal_of(source);
        if wal.is_file() && std::fs::copy(&wal, wal_of(&target)).is_err() {
            continue;
        }
        if file_state(source) == before && file_state(&wal_of(source)) == before_wal {
            return Ok(target);
        }
    }
    Err("브라우저가 쿠키 파일을 계속 고쳐 써서 안정된 사본을 뜨지 못했습니다".into())
}

fn wal_of(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push("-wal");
    PathBuf::from(name)
}

/* ---- 읽기: 두 SQLite와 하나의 바이너리 ---- */

/// 한 소스를 읽은 결과 — 요약 계약(browser-workspace-types.ts:139-160)의
/// 재료. 스킵은 뭉뚱그리지 않는다: 어떤 스킵인지가 경고 문장을 정한다.
#[derive(Debug, Default, Serialize)]
pub struct CookieReadOutcome {
    pub cookies: Vec<ValidatedCookie>,
    pub total: usize,
    pub google_skipped: usize,
    pub non_transplantable_skipped: usize,
    pub partition_skipped: usize,
    pub app_bound: usize,
    pub decrypt_failed: usize,
}

/// Chromium `cookies` 테이블 (`:1660-1780`). 파티션 열은 스키마에 따라
/// 있기도 하다 — PRAGMA로 물어서, 값이 서 있으면 **충실히 못 옮기는
/// 쿠키로 센다**(STA-4300: tauri Cookie에는 파티션 칸이 없다).
pub fn read_chromium_cookies(
    database: &Path,
    keys: &EncryptionKeys,
) -> Result<CookieReadOutcome, String> {
    let db =
        rusqlite::Connection::open_with_flags(database, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|error| error.to_string())?;
    let has_partition_column = {
        let mut columns = db
            .prepare("PRAGMA table_info(cookies)")
            .map_err(|error| error.to_string())?;
        let names = columns
            .query_map([], |row| row.get::<_, String>(1))
            .map_err(|error| error.to_string())?
            .filter_map(Result::ok)
            .collect::<Vec<_>>();
        names.iter().any(|name| name == "top_frame_site_key")
    };
    let select = if has_partition_column {
        "SELECT host_key, name, value, encrypted_value, path, expires_utc, is_secure, \
         is_httponly, samesite, top_frame_site_key FROM cookies ORDER BY rowid"
    } else {
        "SELECT host_key, name, value, encrypted_value, path, expires_utc, is_secure, \
         is_httponly, samesite, '' FROM cookies ORDER BY rowid"
    };
    let mut statement = db.prepare(select).map_err(|error| error.to_string())?;
    let mut out = CookieReadOutcome::default();
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Vec<u8>>(3).unwrap_or_default(),
                row.get::<_, String>(4)?,
                row.get::<_, i64>(5).unwrap_or(0),
                row.get::<_, i64>(6).unwrap_or(0) != 0,
                row.get::<_, i64>(7).unwrap_or(0) != 0,
                row.get::<_, i64>(8).unwrap_or(-1),
                row.get::<_, String>(9).unwrap_or_default(),
            ))
        })
        .map_err(|error| error.to_string())?;
    for row in rows {
        let (
            host,
            name,
            plain_value,
            encrypted,
            path,
            expires,
            secure,
            http_only,
            same_site,
            partition,
        ) = row.map_err(|error| error.to_string())?;
        out.total += 1;
        if browser_cookies::is_google_source_bound(&name, &host) {
            out.google_skipped += 1;
            continue;
        }
        if browser_cookies::is_non_transplantable_domain(&host) {
            out.non_transplantable_skipped += 1;
            continue;
        }
        if !partition.is_empty() {
            out.partition_skipped += 1;
            continue;
        }
        let value = if encrypted.is_empty() {
            plain_value
        } else {
            match browser_cookies::decrypt_cookie_value(&encrypted, keys) {
                DecryptOutcome::Plain(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
                DecryptOutcome::AppBound => {
                    out.app_bound += 1;
                    continue;
                }
                DecryptOutcome::Failed => {
                    out.decrypt_failed += 1;
                    continue;
                }
            }
        };
        let Some(url) = browser_cookies::derive_url(&host, secure) else {
            out.decrypt_failed += 1;
            continue;
        };
        out.cookies.push(ValidatedCookie {
            url,
            name,
            value,
            domain: host,
            path,
            secure,
            http_only,
            same_site: SameSite::from_chromium(same_site),
            expires_unix: browser_cookies::chromium_time_to_unix(expires),
        });
    }
    Ok(out)
}

/// Firefox `moz_cookies` — 평문이고, 파티션 플래그가 있으면 같은 이유로
/// 센다 (`:1425-1460`).
pub fn read_firefox_cookies(database: &Path) -> Result<CookieReadOutcome, String> {
    let db =
        rusqlite::Connection::open_with_flags(database, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|error| error.to_string())?;
    let has_partition_flag = {
        let mut columns = db
            .prepare("PRAGMA table_info(moz_cookies)")
            .map_err(|error| error.to_string())?;
        let names = columns
            .query_map([], |row| row.get::<_, String>(1))
            .map_err(|error| error.to_string())?
            .filter_map(Result::ok)
            .collect::<Vec<_>>();
        names.iter().any(|name| name == "isPartitionedAttributeSet")
    };
    let select = if has_partition_flag {
        "SELECT host, name, value, path, expiry, isSecure, isHttpOnly, sameSite, \
         isPartitionedAttributeSet FROM moz_cookies"
    } else {
        "SELECT host, name, value, path, expiry, isSecure, isHttpOnly, sameSite, 0 \
         FROM moz_cookies"
    };
    let mut statement = db.prepare(select).map_err(|error| error.to_string())?;
    let mut out = CookieReadOutcome::default();
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4).unwrap_or(0),
                row.get::<_, i64>(5).unwrap_or(0) != 0,
                row.get::<_, i64>(6).unwrap_or(0) != 0,
                row.get::<_, i64>(7).unwrap_or(-1),
                row.get::<_, i64>(8).unwrap_or(0) != 0,
            ))
        })
        .map_err(|error| error.to_string())?;
    for row in rows {
        let (host, name, value, path, expiry, secure, http_only, same_site, partitioned) =
            row.map_err(|error| error.to_string())?;
        out.total += 1;
        if browser_cookies::is_google_source_bound(&name, &host) {
            out.google_skipped += 1;
            continue;
        }
        if browser_cookies::is_non_transplantable_domain(&host) {
            out.non_transplantable_skipped += 1;
            continue;
        }
        if partitioned {
            out.partition_skipped += 1;
            continue;
        }
        let Some(url) = browser_cookies::derive_url(&host, secure) else {
            out.decrypt_failed += 1;
            continue;
        };
        out.cookies.push(ValidatedCookie {
            url,
            name,
            value,
            domain: host,
            path,
            secure,
            http_only,
            same_site: SameSite::from_firefox(same_site),
            // Firefox expiry는 이미 unix 초다.
            expires_unix: (expiry > 0).then_some(expiry),
        });
    }
    Ok(out)
}

/// Safari — 파일을 통째로 읽어 C1의 파서에 넘긴다. 구글 규칙은 여기서도
/// 선다: 파서는 정책을 모른다.
pub fn read_safari_cookies(path: &Path) -> Result<CookieReadOutcome, String> {
    let bytes = std::fs::read(path).map_err(|error| error.to_string())?;
    let decoded = browser_cookies::decode_binarycookies(&bytes);
    let mut out = CookieReadOutcome {
        total: decoded.len(),
        ..CookieReadOutcome::default()
    };
    for cookie in decoded {
        if browser_cookies::is_google_source_bound(&cookie.name, &cookie.domain) {
            out.google_skipped += 1;
            continue;
        }
        if browser_cookies::is_non_transplantable_domain(&cookie.domain) {
            out.non_transplantable_skipped += 1;
            continue;
        }
        out.cookies.push(cookie);
    }
    Ok(out)
}

/// Netscape cookies.txt — 원본이 파일에서 가져올 때 받는 유일한 포맷
/// (`extensions: [".txt"]`, `importCookiesFromFile`). 탭 일곱 칸이고 평문이라
/// 열쇠 의식이 없다: `domain · includeSub · path · secure · expiry · name · value`.
/// `#HttpOnly_` 접두는 httponly의 관례 표식이고, 그 밖의 `#`은 주석이다.
/// 정책(구글·비이식 도메인·url 파생)은 다른 소스와 똑같이 여기서도 선다 —
/// 파서는 판단하지 않는다.
pub fn read_netscape_cookies(path: &Path) -> Result<CookieReadOutcome, String> {
    let text = std::fs::read_to_string(path).map_err(|error| error.to_string())?;
    let mut out = CookieReadOutcome::default();
    for line in text.lines() {
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.trim().is_empty() {
            continue;
        }
        // `#HttpOnly_`는 값이 있는 줄이고, 그 밖의 `#`은 주석이다.
        let (body, http_only) = match trimmed.strip_prefix("#HttpOnly_") {
            Some(rest) => (rest, true),
            None if trimmed.starts_with('#') => continue,
            None => (trimmed, false),
        };
        let fields: Vec<&str> = body.split('\t').collect();
        if fields.len() < 7 || fields[0].is_empty() || fields[5].is_empty() {
            continue;
        }
        out.total += 1;
        let host = fields[0].to_string();
        let secure = fields[3].eq_ignore_ascii_case("TRUE");
        let expiry = fields[4].parse::<i64>().unwrap_or(0);
        let name = fields[5].to_string();
        // value는 마지막 칸 — 표준 포맷엔 탭이 없지만, 있더라도 값의 일부다.
        let value = fields[6..].join("\t");
        if browser_cookies::is_google_source_bound(&name, &host) {
            out.google_skipped += 1;
            continue;
        }
        if browser_cookies::is_non_transplantable_domain(&host) {
            out.non_transplantable_skipped += 1;
            continue;
        }
        let Some(url) = browser_cookies::derive_url(&host, secure) else {
            out.decrypt_failed += 1;
            continue;
        };
        out.cookies.push(ValidatedCookie {
            url,
            name,
            value,
            domain: host,
            path: if fields[2].is_empty() {
                "/".to_string()
            } else {
                fields[2].to_string()
            },
            secure,
            http_only,
            // Netscape 포맷엔 same-site 칸이 없다 — 주입은 어차피 이 칸을 읽지
            // 않으므로(StagedCookie엔 없다) 미지정으로 둔다.
            same_site: SameSite::Unspecified,
            expires_unix: (expiry > 0).then_some(expiry),
        });
    }
    Ok(out)
}

/* ---- 열쇠 의식 ---- */

/// macOS: 키체인의 Safe Storage 비밀번호를 `security`에게 묻는다
/// (`getMacEncryptionKey`, `:991-1010`). 사용자가 거절하면 그 사실이 답이다.
#[cfg(target_os = "macos")]
pub fn obtain_keys(family: &str) -> Result<EncryptionKeys, String> {
    let (service, account) =
        keychain_names(family).ok_or_else(|| "모르는 브라우저입니다".to_string())?;
    let output = crate::proc::quiet_command("security")
        .args(["find-generic-password", "-s", service, "-a", account, "-w"])
        .output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(
            "브라우저 암호화 키에 접근하지 못했습니다 — 키체인이 거절했을 수 있습니다".into(),
        );
    }
    let password = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok(EncryptionKeys::Cbc {
        v10: browser_cookies::mac_cbc_key(password.as_bytes()),
        v11: None,
    })
}

/// Linux: v10은 고정 비밀번호, v11은 keyring(secret-tool → kwallet 폴백)
/// (`getLinuxEncryptionKey`, `:1012-1051`). keyring이 없어도 v10은 선다.
#[cfg(all(unix, not(target_os = "macos")))]
pub fn obtain_keys(family: &str) -> Result<EncryptionKeys, String> {
    let (_, account) = keychain_names(family).ok_or_else(|| "모르는 브라우저입니다".to_string())?;
    let keyring = crate::proc::quiet_command("secret-tool")
        .args(["lookup", "application", account])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|password| !password.is_empty());
    Ok(EncryptionKeys::Cbc {
        v10: browser_cookies::linux_cbc_key(browser_cookies::LINUX_V10_PASSWORD),
        v11: keyring.map(|password| browser_cookies::linux_cbc_key(password.as_bytes())),
    })
}

/// Windows: `Local State`의 DPAPI 봉인 키를 PowerShell로 푼다 — stdin으로
/// 건네 주입을 막는 원본의 길 그대로 (`getWindowsEncryptionKey`,
/// `:1053-1105`).
#[cfg(target_os = "windows")]
pub fn obtain_keys(family: &str) -> Result<EncryptionKeys, String> {
    use base64::Engine as _;
    use std::io::Write as _;
    let def = CHROMIUM_BROWSERS
        .iter()
        .find(|def| def.family == family)
        .ok_or_else(|| "모르는 브라우저입니다".to_string())?;
    let home = dirs::home_dir().ok_or_else(|| "홈 디렉터리가 없습니다".to_string())?;
    let root = chromium_root(def, &home).ok_or_else(|| "이 OS 빌드가 없습니다".to_string())?;
    let raw =
        std::fs::read_to_string(root.join("Local State")).map_err(|error| error.to_string())?;
    let state: serde_json::Value = serde_json::from_str(&raw).map_err(|error| error.to_string())?;
    let sealed_b64 = state
        .get("os_crypt")
        .and_then(|section| section.get("encrypted_key"))
        .and_then(|key| key.as_str())
        .ok_or_else(|| "Local State에 암호화 키가 없습니다".to_string())?;
    let sealed = base64::engine::general_purpose::STANDARD
        .decode(sealed_b64)
        .map_err(|error| error.to_string())?;
    let Some(dpapi_payload) = sealed.strip_prefix(b"DPAPI") else {
        return Err("키가 DPAPI 봉인이 아닙니다".into());
    };
    let script = concat!(
        "try { Add-Type -AssemblyName System.Security.Cryptography.ProtectedData -ErrorAction Stop }",
        "catch { try { Add-Type -AssemblyName System.Security -ErrorAction Stop } catch {} };",
        "$in=[Convert]::FromBase64String([Console]::In.ReadLine());",
        "$out=[System.Security.Cryptography.ProtectedData]::Unprotect($in,$null,",
        "[System.Security.Cryptography.DataProtectionScope]::CurrentUser);",
        "[Convert]::ToBase64String($out)",
    );
    let mut child = crate::proc::quiet_command("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .map_err(|error| error.to_string())?;
    let line = format!(
        "{}\n",
        base64::engine::general_purpose::STANDARD.encode(dpapi_payload)
    );
    child
        .stdin
        .take()
        .ok_or_else(|| "powershell stdin".to_string())?
        .write_all(line.as_bytes())
        .map_err(|error| error.to_string())?;
    let output = child
        .wait_with_output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err("DPAPI 복호에 실패했습니다".into());
    }
    let key_bytes = base64::engine::general_purpose::STANDARD
        .decode(String::from_utf8_lossy(&output.stdout).trim())
        .map_err(|error| error.to_string())?;
    let key: [u8; 32] = key_bytes
        .try_into()
        .map_err(|_| "키 길이가 32바이트가 아닙니다".to_string())?;
    Ok(EncryptionKeys::Gcm { key })
}

/* ---- 스테이징: 판이 태어날 때 주입될 쿠키들 ---- */

/// 프로필 하나의 스테이징 파일 — 가져오기는 여기 내려앉고, 그 프로필의
/// 판이 처음 서는 순간 `set_cookie`로 주입된 뒤 소거된다. Electron처럼
/// 파티션 DB에 직접 쓰는 길은 없다(WKWebsiteDataStore 포맷은 비공개) —
/// 살아 있는 판이 유일한 문이고, 그래서 지연이 정직한 모양이다.
pub fn staging_file(config_root: &Path, profile_id: &str) -> PathBuf {
    config_root.join(format!("browser-cookie-staging-{profile_id}.json"))
}

pub fn save_staged_cookies(
    config_root: &Path,
    profile_id: &str,
    cookies: &[ValidatedCookie],
) -> Result<(), String> {
    let text = serde_json::to_vec(cookies).map_err(|error| error.to_string())?;
    std::fs::write(staging_file(config_root, profile_id), text).map_err(|error| error.to_string())
}

/// 스테이징을 읽는다 — 소거는 하지 않는다. 판을 짓기 **전에** 읽어야
/// 하는 값이다: 기다리는 쿠키가 있는 판은 about:blank로 태어나 쿠키를 다
/// 앉힌 뒤 목적지로 떠난다(main.rs `open_browser_pane`). 소거는 주입이
/// 끝난 뒤 `discard_staged_cookies`의 일이다 — 읽는 순간 지우면 주입이
/// 죽었을 때 그 쿠키들은 영영 잃는다.
pub fn peek_staged_cookies(config_root: &Path, profile_id: &str) -> Vec<StagedCookie> {
    let path = staging_file(config_root, profile_id);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    serde_json::from_str(&text).unwrap_or_default()
}

/// 스테이징 쿠키가 **몇 번을 시도해도** 앉지 못하는 이유.
///
/// 이런 쿠키를 거절한 저장소는 실패한 게 아니라 답한 것이다. 그래서
/// 재시도 목록에 남기지 않고 사람에게 알리지도 않는다 — 알려 봤자 손쓸
/// 것이 없고, 남겨 두면 판이 설 때마다 같은 알림이 돌아온다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unacceptable {
    /// 만료가 이미 지났다 — 앉히는 일이 곧 지우는 일이다.
    Expired,
    /// 이름이나 도메인이 없다 — 쓸 것이 없다.
    Nameless,
    /// `__Host-` 는 출처 하나를 약속한다: secure, path `/`, 그리고 Domain
    /// 속성 없음(우리 모양에서는 앞에 점이 붙지 않은 호스트 하나).
    HostPrefix,
    /// `__Secure-` 는 https 를 약속한다.
    SecurePrefix,
}

/// 접두사가 약속을 거는 두 이름 — 규칙은 RFC 6265bis §4.1.3.
const HOST_PREFIX: &str = "__Host-";
const SECURE_PREFIX: &str = "__Secure-";

/// 지금 이 쿠키를 앉힐 수 없는 이유 — 없으면 앉힐 수 있다.
///
/// `now_unix` 를 받는 것은 만료 판정이 시계에 달려 있기 때문이다: 시험은
/// 제 시각을 주고, 부르는 쪽은 진짜 시계를 준다.
#[must_use]
pub fn unacceptable(cookie: &StagedCookie, now_unix: i64) -> Option<Unacceptable> {
    if cookie.name.is_empty() || cookie.domain.is_empty() {
        return Some(Unacceptable::Nameless);
    }
    // 만료 0(또는 없음)은 세션 쿠키다 — 지난 시각이 아니다.
    if cookie
        .expires_unix
        .is_some_and(|at| at > 0 && at <= now_unix)
    {
        return Some(Unacceptable::Expired);
    }
    let path_is_root = cookie.path.is_empty() || cookie.path == "/";
    if cookie.name.starts_with(HOST_PREFIX)
        && (!cookie.secure || !path_is_root || cookie.domain.starts_with('.'))
    {
        return Some(Unacceptable::HostPrefix);
    }
    if cookie.name.starts_with(SECURE_PREFIX) && !cookie.secure {
        return Some(Unacceptable::SecurePrefix);
    }
    None
}

/// 한 번의 주입에서 버려진 쿠키의 수 — 로그 한 줄이 말하는 숫자들이다.
/// 이름도 값도 도메인도 세지 않는다.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Dropped {
    pub expired: usize,
    pub nameless: usize,
    pub host_prefix: usize,
    pub secure_prefix: usize,
}

impl Dropped {
    /// 버려진 전부.
    #[must_use]
    pub fn total(&self) -> usize {
        self.expired + self.nameless + self.host_prefix + self.secure_prefix
    }

    /// 아무것도 버리지 않았다.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.total() == 0
    }

    fn count(&mut self, reason: Unacceptable) {
        match reason {
            Unacceptable::Expired => self.expired += 1,
            Unacceptable::Nameless => self.nameless += 1,
            Unacceptable::HostPrefix => self.host_prefix += 1,
            Unacceptable::SecurePrefix => self.secure_prefix += 1,
        }
    }
}

impl std::fmt::Display for Dropped {
    /// 로그 한 줄에 들어가는 모양 — 0인 이유는 적지 않는다.
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut first = true;
        for (count, word) in [
            (self.expired, "expired"),
            (self.nameless, "nameless"),
            (self.host_prefix, "__Host- rule"),
            (self.secure_prefix, "__Secure- rule"),
        ] {
            if count == 0 {
                continue;
            }
            if !first {
                write!(out, ", ")?;
            }
            write!(out, "{count} {word}")?;
            first = false;
        }
        if first {
            write!(out, "none")?;
        }
        Ok(())
    }
}

/// 만료 판정이 묻는 시계. 부르는 쪽이 저마다 시각을 만들지 않도록 여기
/// 하나만 둔다 — 시계가 에포크 이전이면 0을 답하고, 그러면 아무것도
/// 만료로 버리지 않는다(잘못 버리느니 한 번 더 시도한다).
#[must_use]
pub fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| {
            i64::try_from(since.as_secs()).unwrap_or(i64::MAX)
        })
}

/// 지금 앉힐 수 있는 쿠키들과, 버린 것의 수.
///
/// 버려진 쿠키는 다시 시도되지 않는다: 만료는 시간이 지나도 돌아오지 않고,
/// 접두사 규칙은 다음 판에서도 같은 답을 받는다. 스테이징을 통째로 붙들고
/// 재시도하던 길이 알림 하나를 영영 되풀이하게 만든 자리다.
#[must_use]
pub fn keep_acceptable(cookies: Vec<StagedCookie>, now_unix: i64) -> (Vec<StagedCookie>, Dropped) {
    let mut dropped = Dropped::default();
    let kept = cookies
        .into_iter()
        .filter(|cookie| match unacceptable(cookie, now_unix) {
            Some(reason) => {
                dropped.count(reason);
                false
            }
            None => true,
        })
        .collect();
    (kept, dropped)
}

/// 주입이 끝난 스테이징을 소거한다 — 두 번 주입되는 쿠키는 한 번의 사실이
/// 아니다. 프로필의 저장소가 이미 쿠키를 들었으니 다음 판은 저장소에서
/// 그대로 만난다.
pub fn discard_staged_cookies(config_root: &Path, profile_id: &str) {
    let _ = std::fs::remove_file(staging_file(config_root, profile_id));
}

/// 스테이징에서 돌아오는 모양 — 저장은 ValidatedCookie로 하고, 주입은
/// 이 완화된 모양으로 읽는다: 낡은 스테이징 파일의 낯선 필드가 전체
/// 주입을 죽이면 안 된다.
#[derive(Debug, Default, Deserialize)]
pub struct StagedCookie {
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub value: String,
    #[serde(default)]
    pub domain: String,
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub secure: bool,
    #[serde(default)]
    pub http_only: bool,
    #[serde(default)]
    pub same_site: Option<SameSite>,
    #[serde(default)]
    pub expires_unix: Option<i64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 감지가 세지 않는 가족도 이름을 댄다.
    ///
    /// 어디에서 가져왔는지는 가져온 뒤에 읽히고, 그때 그 브라우저는 이미
    /// 지워졌을 수 있다 — 그러면 `detect_installed_browsers`의 답에는 그
    /// 가족이 없다. 표가 답해야 하는 이유가 그것이다.
    #[test]
    fn every_family_can_say_its_name_without_being_installed() {
        for def in &CHROMIUM_BROWSERS {
            assert_eq!(
                family_label(def.family),
                Some(def.label),
                "{} could not say its name",
                def.family
            );
        }
        assert_eq!(family_label("firefox"), Some(FIREFOX_LABEL));
        assert_eq!(family_label("safari"), Some(SAFARI_LABEL));
        assert_eq!(
            family_label("netscape"),
            None,
            "a stranger was given a name"
        );
    }

    /// 프로필 디렉터리 담장: Local State는 바깥 데이터다.
    #[test]
    fn a_profile_directory_carries_no_road() {
        for good in ["Default", "Profile 1", "프로필"] {
            assert!(is_safe_profile_directory(good), "{good}");
        }
        for bad in ["", ".", "..", "a/b", "a\\b", "a\u{0}b"] {
            assert!(!is_safe_profile_directory(bad), "{bad:?}");
        }
    }

    /// Local State가 없거나 깨져도 Default 하나는 선다.
    #[test]
    fn discovery_falls_back_to_default() {
        let yard = tempfile::tempdir().expect("root");
        assert_eq!(discover_chromium_profiles(yard.path()).len(), 1);
        std::fs::write(yard.path().join("Local State"), "{not json").expect("write");
        assert_eq!(
            discover_chromium_profiles(yard.path())[0].directory,
            "Default"
        );
        std::fs::write(
            yard.path().join("Local State"),
            r#"{"profile":{"info_cache":{"Default":{"name":"나"},"Profile 1":{"name":"일"},"../x":{"name":"악"}}}}"#,
        )
        .expect("write");
        let found = discover_chromium_profiles(yard.path());
        assert_eq!(found.len(), 2, "the traversal name was fenced out");
        assert_eq!(found[0].name, "나");
    }

    /// 스냅숏: 안정된 원본은 한 번에 뜨고, 사본은 원본과 같은 바이트다.
    #[test]
    fn a_quiet_database_snapshots_first_try() {
        let yard = tempfile::tempdir().expect("root");
        let source = yard.path().join("Cookies");
        std::fs::write(&source, b"sqlite-bytes").expect("write");
        let copy_dir = yard.path().join("snap");
        let copy = snapshot_sqlite(&source, &copy_dir).expect("snapshot");
        assert_eq!(std::fs::read(&copy).expect("read"), b"sqlite-bytes");
        assert!(snapshot_sqlite(&yard.path().join("absent"), &copy_dir).is_err());
    }

    /// Chromium 사본 읽기: 평문·구글·파티션·앱바운드가 각자의 칸으로 센다.
    #[test]
    fn chromium_rows_sort_into_their_bins() {
        let yard = tempfile::tempdir().expect("root");
        let database = yard.path().join("Cookies");
        let db = rusqlite::Connection::open(&database).expect("create");
        db.execute_batch(
            "CREATE TABLE cookies (host_key TEXT, name TEXT, value TEXT, \
             encrypted_value BLOB, path TEXT, expires_utc INTEGER, is_secure INTEGER, \
             is_httponly INTEGER, samesite INTEGER, top_frame_site_key TEXT);",
        )
        .expect("schema");
        let mut insert = db
            .prepare("INSERT INTO cookies VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)")
            .expect("prepare");
        let plain: (&str, &str, &str, &[u8], &str, i64, i64, i64, i64, &str) = (
            ".example.com",
            "sid",
            "plain-value",
            b"",
            "/",
            0,
            1,
            1,
            1,
            "",
        );
        insert
            .execute(rusqlite::params![
                plain.0, plain.1, plain.2, plain.3, plain.4, plain.5, plain.6, plain.7, plain.8,
                plain.9
            ])
            .expect("plain");
        insert
            .execute(rusqlite::params![
                ".google.com",
                "SIDCC",
                "x",
                b"".as_slice(),
                "/",
                0,
                1,
                1,
                1,
                ""
            ])
            .expect("google");
        insert
            .execute(rusqlite::params![
                ".partitioned.com",
                "a",
                "x",
                b"".as_slice(),
                "/",
                0,
                1,
                0,
                0,
                "https://top.example"
            ])
            .expect("partitioned");
        insert
            .execute(rusqlite::params![
                ".sealed.com",
                "b",
                "",
                b"v20sealed".as_slice(),
                "/",
                0,
                0,
                0,
                -1,
                ""
            ])
            .expect("app-bound");
        drop(insert);
        drop(db);
        let keys = EncryptionKeys::Gcm { key: [0; 32] };
        let out = read_chromium_cookies(&database, &keys).expect("read");
        assert_eq!(out.total, 4);
        assert_eq!(out.cookies.len(), 1);
        assert_eq!(out.cookies[0].value, "plain-value");
        assert_eq!(out.cookies[0].url, "https://example.com/");
        assert_eq!(out.google_skipped, 1);
        assert_eq!(out.partition_skipped, 1);
        assert_eq!(out.app_bound, 1);
    }

    /// Firefox 읽기: 평문과 파티션 플래그.
    #[test]
    fn firefox_rows_read_plain_and_flagged() {
        let yard = tempfile::tempdir().expect("root");
        let database = yard.path().join("cookies.sqlite");
        let db = rusqlite::Connection::open(&database).expect("create");
        db.execute_batch(
            "CREATE TABLE moz_cookies (host TEXT, name TEXT, value TEXT, path TEXT, \
             expiry INTEGER, isSecure INTEGER, isHttpOnly INTEGER, sameSite INTEGER, \
             isPartitionedAttributeSet INTEGER);",
        )
        .expect("schema");
        db.execute(
            "INSERT INTO moz_cookies VALUES ('.rust-lang.org', 'ff', 'v', '/', 1893456000, 1, 0, 1, 0)",
            [],
        )
        .expect("plain");
        db.execute(
            "INSERT INTO moz_cookies VALUES ('.chips.dev', 'p', 'v', '/', 0, 1, 0, 0, 1)",
            [],
        )
        .expect("flagged");
        drop(db);
        let out = read_firefox_cookies(&database).expect("read");
        assert_eq!(out.total, 2);
        assert_eq!(out.cookies.len(), 1);
        assert_eq!(out.cookies[0].expires_unix, Some(1_893_456_000));
        assert_eq!(out.partition_skipped, 1);
    }

    /// 한 벌의 쿠키 — 이름만 바꾸며 쓴다.
    fn staged(name: &str) -> StagedCookie {
        StagedCookie {
            url: "https://example.com/".into(),
            name: name.into(),
            value: "v".into(),
            domain: "example.com".into(),
            path: "/".into(),
            secure: true,
            http_only: false,
            same_site: None,
            expires_unix: None,
        }
    }

    /// 저장소가 **몇 번을 물어도** 같은 답을 주는 쿠키는 실패가 아니라
    /// 답이다. 2026-09-15 의 스테이징 파일(쿠키 2,245개)이 그 증거다:
    /// 그중 519개는 만료가 지나 있었고, 하나라도 거절되면 스테이징이
    /// 보관되므로 브라우저 판이 설 때마다 같은 알림이 돌아왔다.
    #[test]
    fn a_cookie_that_can_never_be_laid_down_is_not_kept_for_retry() {
        let now = 1_700_000_000;

        // 만료가 지난 것 — 앉히는 일이 곧 지우는 일이다.
        let mut expired = staged("old");
        expired.expires_unix = Some(now - 1);
        assert_eq!(unacceptable(&expired, now), Some(Unacceptable::Expired));
        // 만료가 앞으로면 앉힌다. 0과 없음은 세션 쿠키다.
        let mut live = staged("live");
        live.expires_unix = Some(now + 1);
        assert_eq!(unacceptable(&live, now), None);
        let mut zero = staged("session");
        zero.expires_unix = Some(0);
        assert_eq!(unacceptable(&zero, now), None);
        assert_eq!(unacceptable(&staged("plain"), now), None);

        // 이름이나 도메인이 없으면 쓸 것이 없다.
        let mut nameless = staged("");
        nameless.name = String::new();
        assert_eq!(unacceptable(&nameless, now), Some(Unacceptable::Nameless));
        let mut homeless = staged("x");
        homeless.domain = String::new();
        assert_eq!(unacceptable(&homeless, now), Some(Unacceptable::Nameless));

        // `__Host-` 는 출처 하나를 약속한다: 앞에 점 붙은 도메인, 하위
        // 경로, secure 아님은 모두 저장소가 영영 거절한다. 반면 점 없는
        // 호스트 하나에 path `/`, secure 면 정상이다 — 2026-09-15 표본의
        // 접두사 쿠키 31개가 모두 이 모양이었다.
        assert_eq!(unacceptable(&staged("__Host-ok"), now), None);
        let mut dotted = staged("__Host-dotted");
        dotted.domain = ".example.com".into();
        assert_eq!(unacceptable(&dotted, now), Some(Unacceptable::HostPrefix));
        let mut deep = staged("__Host-deep");
        deep.path = "/app".into();
        assert_eq!(unacceptable(&deep, now), Some(Unacceptable::HostPrefix));
        let mut plain = staged("__Host-plain");
        plain.secure = false;
        assert_eq!(unacceptable(&plain, now), Some(Unacceptable::HostPrefix));

        // `__Secure-` 는 https 하나만 약속한다.
        let mut insecure = staged("__Secure-plain");
        insecure.secure = false;
        assert_eq!(
            unacceptable(&insecure, now),
            Some(Unacceptable::SecurePrefix)
        );
        assert_eq!(unacceptable(&staged("__Secure-ok"), now), None);
    }

    /// 거르기는 남길 것과 버린 수를 함께 답한다. 버린 것만 남은 스테이징은
    /// 앉힐 것이 없다 — 부르는 쪽은 그걸 보고 파일을 지운다.
    #[test]
    fn the_filter_says_what_it_kept_and_what_it_dropped() {
        let now = 1_700_000_000;
        let mut expired = staged("old");
        expired.expires_unix = Some(now - 60);
        let mut dotted = staged("__Host-dotted");
        dotted.domain = ".example.com".into();
        let (kept, dropped) = keep_acceptable(vec![staged("a"), expired, dotted, staged("b")], now);
        assert_eq!(kept.len(), 2);
        assert_eq!(kept[0].name, "a");
        assert_eq!(kept[1].name, "b");
        assert_eq!(dropped.expired, 1);
        assert_eq!(dropped.host_prefix, 1);
        assert_eq!(dropped.total(), 2);
        assert!(!dropped.is_empty());
        assert_eq!(dropped.to_string(), "1 expired, 1 __Host- rule");

        let mut only_dead = staged("dead");
        only_dead.expires_unix = Some(now - 1);
        let (kept, dropped) = keep_acceptable(vec![only_dead], now);
        assert!(kept.is_empty(), "앉힐 것이 없다");
        assert_eq!(dropped.total(), 1);

        let (kept, dropped) = keep_acceptable(vec![staged("a")], now);
        assert_eq!(kept.len(), 1);
        assert!(dropped.is_empty());
        assert_eq!(dropped.to_string(), "none");
    }

    /// 스테이징 왕복 — 읽는 순간 파일이 사라진다(한 번의 주입).
    #[test]
    fn staged_cookies_wait_until_discarded() {
        let yard = tempfile::tempdir().expect("root");
        let cookie = ValidatedCookie {
            url: "https://example.com/".into(),
            name: "sid".into(),
            value: "v".into(),
            domain: ".example.com".into(),
            path: "/".into(),
            secure: true,
            http_only: false,
            same_site: SameSite::Lax,
            expires_unix: None,
        };
        save_staged_cookies(yard.path(), "abc123", &[cookie]).expect("save");
        // 읽기는 소거가 아니다: 판이 태어나기 전에 읽고, 주입이 끝난 뒤에
        // 지운다 — 사이에서 죽어도 스테이징은 남는다.
        let staged = peek_staged_cookies(yard.path(), "abc123");
        assert_eq!(staged.len(), 1);
        assert_eq!(staged[0].name, "sid");
        assert_eq!(peek_staged_cookies(yard.path(), "abc123").len(), 1);
        discard_staged_cookies(yard.path(), "abc123");
        assert!(peek_staged_cookies(yard.path(), "abc123").is_empty());
        // 없는 스테이징의 소거는 조용하다.
        discard_staged_cookies(yard.path(), "abc123");
        assert!(peek_staged_cookies(yard.path(), "nobody").is_empty());
    }

    /// Netscape cookies.txt — 탭 일곱 칸, `#HttpOnly_` 표식, `#` 주석은 건너뛰고,
    /// 만료 0은 세션, 칸이 모자란 줄은 세지 않는다.
    #[test]
    fn netscape_cookies_txt_reads_flags_and_skips_comments() {
        let yard = tempfile::tempdir().expect("root");
        let file = yard.path().join("cookies.txt");
        std::fs::write(
            &file,
            "# Netscape HTTP Cookie File\n\
             # a comment line\n\
             \n\
             .example.com\tTRUE\t/\tTRUE\t1893456000\tsid\tabc123\n\
             #HttpOnly_.example.com\tTRUE\t/app\tFALSE\t0\ttoken\txyz\n\
             short\tline\tnot\tenough\n",
        )
        .expect("write");

        let out = read_netscape_cookies(&file).expect("read");
        // 주석·빈 줄·필드 부족 줄은 세지 않는다 — 쿠키 둘.
        assert_eq!(out.total, 2, "counted comment/blank/short lines");
        assert_eq!(out.cookies.len(), 2);

        let sid = out.cookies.iter().find(|c| c.name == "sid").expect("sid");
        assert!(sid.secure);
        assert!(!sid.http_only);
        assert_eq!(sid.expires_unix, Some(1_893_456_000));
        assert_eq!(sid.value, "abc123");

        let token = out
            .cookies
            .iter()
            .find(|c| c.name == "token")
            .expect("token");
        assert!(token.http_only, "the #HttpOnly_ marker was not read");
        assert!(!token.secure);
        assert_eq!(token.expires_unix, None, "expiry 0 is a session cookie");
        assert_eq!(token.path, "/app");
    }
}
