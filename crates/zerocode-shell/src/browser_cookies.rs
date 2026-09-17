//! 브라우저 쿠키 가져오기의 순수한 반 (P1 'imported' 프로필, 지시서 C1 —
//! `docs/plans/browser-cookie-import.md`).
//!
//! 원본 계약: Orca `browser-cookie-import.ts`(복호·파서·시각)와
//! `browser-cookie-import-policy.ts`(도메인·구글 규칙). 디스크·키체인·
//! 프로세스는 C2(main.rs)의 손이고, 여기는 **바이트와 문자열만 받는
//! 판정**이다 — last_status·file_tree_ops와 같은 분리, 같은 이유: 계약은
//! 디스크 없이 시험할 수 있어야 한다.

use serde::{Deserialize, Serialize};

/* ---- 상수: 전부 Chromium/Safari가 정한 수 ---- */

/// PBKDF2-SHA1 유도의 세 상수 (browser-cookie-import.ts:827-829). salt와
/// 반복 수는 Chromium os_crypt의 것이라 바꾸는 순간 아무것도 못 연다.
const PBKDF2_SALT: &[u8] = b"saltysalt";
const PBKDF2_ITERATIONS_MAC: u32 = 1_003;
/// Linux v10은 고정 비밀번호 "peanuts"에 **1회** — Chromium이 그렇게 쓴다
/// (`:1018`); v11만 keyring 비밀번호를 같은 1회로 유도한다(`:1049`).
const PBKDF2_ITERATIONS_LINUX: u32 = 1;
// macOS·Windows 빌드에서는 걷지 않는 길이라 죽은 글자로 보인다 — 호출부는
// browser_cookie_import의 Linux `obtain_keys`에 실재한다(보드 메모 20).
#[cfg_attr(any(target_os = "macos", target_os = "windows"), allow(dead_code))]
pub const LINUX_V10_PASSWORD: &[u8] = b"peanuts";
const DERIVED_KEY_LEN: usize = 16;

/// CBC의 IV는 스페이스 16개 — Chromium os_crypt의 관용 그대로 (`:1201`).
const CBC_IV: [u8; 16] = [0x20; 16];

/// Chromium 127+가 값 앞에 붙이는 HMAC의 길이. 해시는 대략 절반이 비인쇄
/// 바이트라, 앞 32바이트 중 비인쇄가 8개 이상이면 접두로 판정한다
/// (`:1107-1126` — 표본이 아니라 원본의 판정식이다).
const HMAC_PREFIX_LEN: usize = 32;
const HMAC_NONPRINTABLE_FLOOR: usize = 8;

/// GCM 레이아웃: `[12바이트 nonce][본문][16바이트 태그]` (`:1211-1219`).
const GCM_NONCE_LEN: usize = 12;
const GCM_TAG_LEN: usize = 16;

/// Safari는 2001-01-01(Mac absolute time) 기준 초를 쓴다 (`:1317`).
const MAC_EPOCH_DELTA: f64 = 978_307_200.0;

/// Chromium 시각은 1601-01-01 기준 마이크로초다 (`:833-866`).
const WINDOWS_TO_UNIX_EPOCH_SECS: i64 = 11_644_473_600;

/// 소스 기기에 결속돼 이식하면 계정이 깨지는 구글 쿠키
/// (browser-cookie-import-policy.ts:7-15).
const GOOGLE_SOURCE_BOUND_COOKIE_NAMES: [&str; 5] = [
    "SIDCC",
    "__Secure-1PSIDCC",
    "__Secure-3PSIDCC",
    "__Secure-STRP",
    "AEC",
];

/// 도메인 전체가 이식 불가인 목록 (`-policy.ts:104`) — google.com의 세션은
/// 소스 브라우저의 지문에 묶여 있어 옮기면 로그아웃보다 나쁘게 깨진다.
const NON_TRANSPLANTABLE_DOMAINS: [&str; 1] = ["google.com"];

/* ---- 모델 ---- */

/// 한 쿠키의 검증된 모양 — Electron `cookies.set`에 건네던 그 필드들이고,
/// 우리 쪽에서는 tauri `Webview::set_cookie`가 읽는다 (`:543-591`).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ValidatedCookie {
    pub url: String,
    pub name: String,
    pub value: String,
    pub domain: String,
    pub path: String,
    pub secure: bool,
    pub http_only: bool,
    pub same_site: SameSite,
    /// unix 초. 없으면 세션 쿠키.
    pub expires_unix: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SameSite {
    Unspecified,
    NoRestriction,
    Lax,
    Strict,
}

impl SameSite {
    /// Chromium cookies.samesite: -1 unspecified / 0 none / 1 lax / 2 strict
    /// (`:482-493`).
    pub fn from_chromium(raw: i64) -> Self {
        match raw {
            0 => Self::NoRestriction,
            1 => Self::Lax,
            2 => Self::Strict,
            _ => Self::Unspecified,
        }
    }

    /// Firefox moz_cookies.sameSite: 0 none / 1 lax / 2 strict (`:495-506`).
    pub fn from_firefox(raw: i64) -> Self {
        match raw {
            0 => Self::NoRestriction,
            1 => Self::Lax,
            2 => Self::Strict,
            _ => Self::Unspecified,
        }
    }
}

/* ---- 정책: 도메인과 구글 규칙 ---- */

/// 선행 점을 벗기고 소문자로 — 쿠키 도메인의 정규형 (`-policy.ts:17-54`).
/// 브래킷 IPv6은 그대로 지나가고, 빈 것과 공백 낀 것은 도메인이 아니다.
pub fn normalize_cookie_domain(domain: &str) -> Option<String> {
    let candidate = domain.trim().trim_start_matches('.');
    if candidate.is_empty() || candidate.chars().any(char::is_whitespace) {
        return None;
    }
    Some(candidate.to_ascii_lowercase())
}

/// 이 도메인의 쿠키는 통째로 이식하지 않는다 (`-policy.ts:104-119`).
pub fn is_non_transplantable_domain(domain: &str) -> bool {
    let Some(normalized) = normalize_cookie_domain(domain) else {
        return false;
    };
    NON_TRANSPLANTABLE_DOMAINS
        .iter()
        .any(|root| normalized == *root || normalized.ends_with(&format!(".{root}")))
}

/// 이름까지 맞아야 하는 소스-결속 구글 쿠키 (`-policy.ts:127-136`).
pub fn is_google_source_bound(name: &str, domain: &str) -> bool {
    if !GOOGLE_SOURCE_BOUND_COOKIE_NAMES.contains(&name) {
        return false;
    }
    let Some(normalized) = normalize_cookie_domain(domain) else {
        return false;
    };
    normalized == "google.com" || normalized.ends_with(".google.com")
}

/// 쿠키의 도메인에서 set_cookie가 요구하는 url을 유도한다 (`:529-543`).
pub fn derive_url(domain: &str, secure: bool) -> Option<String> {
    let host = normalize_cookie_domain(domain)?;
    let scheme = if secure { "https" } else { "http" };
    Some(format!("{scheme}://{host}/"))
}

/* ---- 시각 ---- */

/// Chromium의 1601-epoch 마이크로초를 unix 초로. 0은 세션 쿠키(만료
/// 없음)이고, epoch보다 이른 값은 시계가 아니라 쓰레기다 (`:833-866`).
pub fn chromium_time_to_unix(chromium_micros: i64) -> Option<i64> {
    if chromium_micros <= 0 {
        return None;
    }
    let seconds = chromium_micros / 1_000_000 - WINDOWS_TO_UNIX_EPOCH_SECS;
    (seconds > 0).then_some(seconds)
}

/// Safari의 Mac absolute time(2001 기준 초)을 unix 초로 (`:1317-1339`).
pub fn mac_time_to_unix(mac_seconds: f64) -> Option<i64> {
    (mac_seconds > 0.0).then(|| (mac_seconds + MAC_EPOCH_DELTA).round() as i64)
}

/* ---- Chromium 값 복호 ---- */

/// C2가 OS별 의식으로 얻어 온 열쇠들. CBC 쪽은 버전별 두 벌일 수 있다 —
/// Linux의 v10(고정 비밀번호)과 v11(keyring)이 그 경우다 (`:1012-1051`).
pub enum EncryptionKeys {
    Cbc {
        v10: [u8; DERIVED_KEY_LEN],
        v11: Option<[u8; DERIVED_KEY_LEN]>,
    },
    // Windows의 DPAPI 길에서만 지어진다(browser_cookie_import `obtain_keys`)
    // — 다른 빌드에서는 테스트만 이 갈래를 걷는다.
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    Gcm { key: [u8; 32] },
}

/// 복호의 세 갈래 답. AppBound(v20)는 실패가 아니라 **셈해야 하는 스킵**
/// 이다 — Orca도 세어서 `cookies-undecryptable` 경고로 보고한다 (`:1138`).
#[derive(Debug, PartialEq)]
pub enum DecryptOutcome {
    Plain(Vec<u8>),
    AppBound,
    Failed,
}

/// Chromium Safe Storage 비밀번호에서 CBC 키를 유도한다.
pub fn chromium_cbc_key(password: &[u8], iterations: u32) -> [u8; DERIVED_KEY_LEN] {
    let mut key = [0u8; DERIVED_KEY_LEN];
    pbkdf2::pbkdf2_hmac::<sha1::Sha1>(password, PBKDF2_SALT, iterations, &mut key);
    key
}

/// macOS keychain 비밀번호용 (1003회).
pub fn mac_cbc_key(password: &[u8]) -> [u8; DERIVED_KEY_LEN] {
    chromium_cbc_key(password, PBKDF2_ITERATIONS_MAC)
}

/// Linux v10/v11용 (1회). LINUX_V10_PASSWORD와 같은 이유로, 이 빌드가
/// 아닌 곳에서만 불린다.
#[cfg_attr(any(target_os = "macos", target_os = "windows"), allow(dead_code))]
pub fn linux_cbc_key(password: &[u8]) -> [u8; DERIVED_KEY_LEN] {
    chromium_cbc_key(password, PBKDF2_ITERATIONS_LINUX)
}

fn nonprintable_count(bytes: &[u8]) -> usize {
    bytes
        .iter()
        .filter(|byte| !(0x20..=0x7e).contains(*byte))
        .count()
}

/// Chromium 127+의 HMAC 접두 판정 (`:1107-1120`). 길이는 `>=`다 — 값이
/// 빈 쿠키는 접두 32바이트가 전부라, `>`로 재면 그 해시가 값으로 남는다.
fn has_hmac_prefix(decrypted: &[u8]) -> bool {
    decrypted.len() >= HMAC_PREFIX_LEN
        && nonprintable_count(&decrypted[..HMAC_PREFIX_LEN]) >= HMAC_NONPRINTABLE_FLOOR
}

fn strip_hmac(decrypted: Vec<u8>) -> Vec<u8> {
    if has_hmac_prefix(&decrypted) {
        decrypted[HMAC_PREFIX_LEN..].to_vec()
    } else {
        decrypted
    }
}

fn encryption_version(encrypted: &[u8]) -> Option<String> {
    if encrypted.len() < 3 {
        return None;
    }
    let head = &encrypted[..3];
    if head[0] == b'v' && head[1].is_ascii_digit() && head[2].is_ascii_digit() {
        Some(String::from_utf8_lossy(head).into_owned())
    } else {
        None
    }
}

/// 한 값의 복호 (`decryptCookieValueRaw`, `:1173-1231`). 접두가 버전이
/// 아니면 실패이고, 실패는 조용한 빈 문자열이 아니라 실패다.
pub fn decrypt_cookie_value(encrypted: &[u8], keys: &EncryptionKeys) -> DecryptOutcome {
    let Some(version) = encryption_version(encrypted) else {
        return DecryptOutcome::Failed;
    };
    if version == "v20" {
        return DecryptOutcome::AppBound;
    }
    let payload = &encrypted[3..];
    if payload.is_empty() {
        return DecryptOutcome::Failed;
    }
    match keys {
        EncryptionKeys::Gcm { key } => decrypt_gcm(payload, key),
        EncryptionKeys::Cbc { v10, v11 } => {
            let key = match version.as_str() {
                "v10" => Some(v10),
                "v11" => v11.as_ref(),
                _ => None,
            };
            match key {
                Some(key) => decrypt_cbc(payload, key),
                None => DecryptOutcome::Failed,
            }
        }
    }
}

fn decrypt_cbc(ciphertext: &[u8], key: &[u8; DERIVED_KEY_LEN]) -> DecryptOutcome {
    use aes::cipher::{BlockModeDecrypt, KeyIvInit, block_padding::Pkcs7};
    let mut buf = ciphertext.to_vec();
    let decryptor = cbc::Decryptor::<aes::Aes128>::new(key.into(), (&CBC_IV).into());
    match decryptor.decrypt_padded::<Pkcs7>(&mut buf) {
        Ok(plain) => DecryptOutcome::Plain(strip_hmac(plain.to_vec())),
        Err(_) => DecryptOutcome::Failed,
    }
}

fn decrypt_gcm(payload: &[u8], key: &[u8; 32]) -> DecryptOutcome {
    use aes_gcm::KeyInit;
    use aes_gcm::aead::Aead;
    if payload.len() < GCM_NONCE_LEN + GCM_TAG_LEN {
        return DecryptOutcome::Failed;
    }
    let Ok(cipher) = aes_gcm::Aes256Gcm::new_from_slice(key) else {
        return DecryptOutcome::Failed;
    };
    let Ok(nonce) = aes_gcm::Nonce::try_from(&payload[..GCM_NONCE_LEN]) else {
        return DecryptOutcome::Failed;
    };
    // aead의 decrypt는 [본문||태그] 붙은 모양을 받는다 — Chromium 레이아웃의
    // nonce 뒤가 정확히 그것이다.
    match cipher.decrypt(&nonce, &payload[GCM_NONCE_LEN..]) {
        Ok(plain) => DecryptOutcome::Plain(strip_hmac(plain)),
        Err(_) => DecryptOutcome::Failed,
    }
}

/* ---- Safari binarycookies ---- */

/// `Cookies.binarycookies` 전체를 푼다 (`decodeSafariBinaryCookies`,
/// `:1233-1260`): 매직 `cook`, BE 페이지 수, BE 페이지 크기 표, 페이지들.
pub fn decode_binarycookies(buffer: &[u8]) -> Vec<ValidatedCookie> {
    if buffer.len() < 8 || &buffer[..4] != b"cook" {
        return Vec::new();
    }
    let page_count = read_u32_be(buffer, 4) as usize;
    let mut cursor = 8usize;
    let Some(table_end) = cursor.checked_add(page_count * 4) else {
        return Vec::new();
    };
    if table_end > buffer.len() {
        return Vec::new();
    }
    let mut sizes = Vec::with_capacity(page_count);
    for _ in 0..page_count {
        sizes.push(read_u32_be(buffer, cursor) as usize);
        cursor += 4;
    }
    let mut cookies = Vec::new();
    for size in sizes {
        let Some(end) = cursor.checked_add(size) else {
            break;
        };
        if end > buffer.len() {
            break;
        }
        decode_safari_page(&buffer[cursor..end], &mut cookies);
        cursor = end;
    }
    cookies
}

/// 한 페이지: LE 매직 0x00000100, LE 쿠키 수, LE 오프셋 표 (`:1268-1295`).
fn decode_safari_page(page: &[u8], out: &mut Vec<ValidatedCookie>) {
    if page.len() < 16 || read_u32_be(page, 0) != 0x0000_0100 {
        return;
    }
    let cookie_count = read_u32_le(page, 4) as usize;
    if 8 + cookie_count * 4 > page.len() {
        return;
    }
    for at in 0..cookie_count {
        let offset = read_u32_le(page, 8 + at * 4) as usize;
        if offset >= page.len() {
            continue;
        }
        if let Some(cookie) = decode_safari_cookie(&page[offset..]) {
            out.push(cookie);
        }
    }
}

/// 한 쿠키 레코드 (`decodeSafariCookie`, `:1297-1355`): LE 크기, 플래그
/// (1=secure, 4=httpOnly), 오프셋 4종, LE double 만료(Mac absolute).
/// 크기는 파일에서 온 수라 먼저 잘라 — C 문자열 읽기가 레코드 밖으로
/// 나가지 못하게 한다.
fn decode_safari_cookie(buf: &[u8]) -> Option<ValidatedCookie> {
    if buf.len() < 48 {
        return None;
    }
    let size = (read_u32_le(buf, 0) as usize).min(buf.len());
    if size < 48 {
        return None;
    }
    let flags = read_u32_le(buf, 8);
    let secure = flags & 1 != 0;
    let http_only = flags & 4 != 0;
    let url_offset = read_u32_le(buf, 16) as usize;
    let name_offset = read_u32_le(buf, 20) as usize;
    let path_offset = read_u32_le(buf, 24) as usize;
    let value_offset = read_u32_le(buf, 28) as usize;
    let expiration = read_f64_le(buf, 40);
    let name = read_cstring(buf, name_offset, size)?;
    let value = read_cstring(buf, value_offset, size).unwrap_or_default();
    let path = read_cstring(buf, path_offset, size).unwrap_or_else(|| "/".to_string());
    // Safari는 도메인을 URL 칸에 둔다 — 별도 도메인 컬럼이 없다 (`:1327`).
    let domain = read_cstring(buf, url_offset, size).filter(|raw| !raw.is_empty())?;
    let url = derive_url(&domain, secure)?;
    Some(ValidatedCookie {
        url,
        name,
        value,
        domain,
        path,
        secure,
        http_only,
        // binarycookies는 CHIPS 이전 포맷 — sameSite 칸 자체가 없다.
        same_site: SameSite::Unspecified,
        expires_unix: mac_time_to_unix(expiration),
    })
}

fn read_u32_be(buf: &[u8], at: usize) -> u32 {
    buf.get(at..at + 4)
        .map(|bytes| u32::from_be_bytes(bytes.try_into().expect("four bytes")))
        .unwrap_or(0)
}

fn read_u32_le(buf: &[u8], at: usize) -> u32 {
    buf.get(at..at + 4)
        .map(|bytes| u32::from_le_bytes(bytes.try_into().expect("four bytes")))
        .unwrap_or(0)
}

fn read_f64_le(buf: &[u8], at: usize) -> f64 {
    buf.get(at..at + 8)
        .map(|bytes| f64::from_le_bytes(bytes.try_into().expect("eight bytes")))
        .unwrap_or(0.0)
}

fn read_cstring(buf: &[u8], offset: usize, end: usize) -> Option<String> {
    if offset == 0 || offset >= end || end > buf.len() {
        return None;
    }
    let tail = &buf[offset..end];
    let nul = tail.iter().position(|byte| *byte == 0)?;
    Some(String::from_utf8_lossy(&tail[..nul]).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cbc_encrypt(plain: &[u8], key: &[u8; 16]) -> Vec<u8> {
        use aes::cipher::{BlockModeEncrypt, KeyIvInit, block_padding::Pkcs7};
        let mut buf = vec![0u8; plain.len() + 16];
        buf[..plain.len()].copy_from_slice(plain);
        cbc::Encryptor::<aes::Aes128>::new(key.into(), (&CBC_IV).into())
            .encrypt_padded::<Pkcs7>(&mut buf, plain.len())
            .expect("a block of headroom is enough for pkcs7")
            .to_vec()
    }

    /// CBC 왕복: v10으로 잠근 것은 v10 키가 열고, v11 표기는 v11 키만 연다.
    #[test]
    fn cbc_roundtrip_honours_the_version_prefix() {
        let v10 = mac_cbc_key(b"from-the-keychain");
        let v11 = linux_cbc_key(b"from-the-keyring");
        let mut sealed = b"v10".to_vec();
        sealed.extend(cbc_encrypt(b"session=abc123", &v10));
        let keys = EncryptionKeys::Cbc {
            v10,
            v11: Some(v11),
        };
        assert_eq!(
            decrypt_cookie_value(&sealed, &keys),
            DecryptOutcome::Plain(b"session=abc123".to_vec())
        );
        let mut wrong = b"v11".to_vec();
        wrong.extend(cbc_encrypt(b"session=abc123", &v10));
        // v11 표기인데 v10 키로 잠근 것 — 열리면 그게 버그다. PKCS7 패딩이
        // 우연히 성립할 확률로만 살아남으므로 Plain이 아닌지로 단언한다.
        assert_ne!(
            decrypt_cookie_value(&wrong, &keys),
            DecryptOutcome::Plain(b"session=abc123".to_vec())
        );
    }

    /// Chromium 127+의 HMAC 접두는 벗겨지고, 안 붙은 값은 그대로다.
    #[test]
    fn the_hmac_prefix_comes_off_and_only_when_it_is_one() {
        let key = mac_cbc_key(b"pw");
        let mut hashed = vec![0u8; HMAC_PREFIX_LEN];
        hashed.extend_from_slice(b"value-after-hash");
        let mut sealed = b"v10".to_vec();
        sealed.extend(cbc_encrypt(&hashed, &key));
        let keys = EncryptionKeys::Cbc {
            v10: key,
            v11: None,
        };
        assert_eq!(
            decrypt_cookie_value(&sealed, &keys),
            DecryptOutcome::Plain(b"value-after-hash".to_vec())
        );
        // 32바이트가 전부 인쇄 가능한 짧은 평문은 접두가 아니다.
        let mut plain_sealed = b"v10".to_vec();
        plain_sealed.extend(cbc_encrypt(
            b"all printable ascii, well over thirty-two bytes long",
            &key,
        ));
        assert_eq!(
            decrypt_cookie_value(&plain_sealed, &keys),
            DecryptOutcome::Plain(b"all printable ascii, well over thirty-two bytes long".to_vec())
        );
        // 값이 빈 쿠키: 접두 32바이트가 전부다 — 해시가 값으로 남으면 안 된다.
        let mut empty_sealed = b"v10".to_vec();
        empty_sealed.extend(cbc_encrypt(&[0u8; HMAC_PREFIX_LEN], &key));
        assert_eq!(
            decrypt_cookie_value(&empty_sealed, &keys),
            DecryptOutcome::Plain(Vec::new())
        );
        // 정확히 32바이트인데 전부 인쇄 가능한 평문은 여전히 값이다.
        let printable = b"exactly-thirty-two-printable-byt";
        assert_eq!(printable.len(), HMAC_PREFIX_LEN);
        let mut printable_sealed = b"v10".to_vec();
        printable_sealed.extend(cbc_encrypt(printable, &key));
        assert_eq!(
            decrypt_cookie_value(&printable_sealed, &keys),
            DecryptOutcome::Plain(printable.to_vec())
        );
    }

    /// GCM 왕복 — Windows 레이아웃 [nonce][본문][태그] 그대로.
    #[test]
    fn gcm_roundtrip_in_the_windows_layout() {
        use aes_gcm::KeyInit;
        use aes_gcm::aead::Aead;
        let key = [7u8; 32];
        let cipher = aes_gcm::Aes256Gcm::new_from_slice(&key).expect("key");
        let nonce = [9u8; GCM_NONCE_LEN];
        let sealed_tail = cipher
            .encrypt(
                &aes_gcm::Nonce::try_from(&nonce[..]).expect("twelve bytes"),
                b"gcm-value".as_ref(),
            )
            .expect("seal");
        let mut sealed = b"v10".to_vec();
        sealed.extend_from_slice(&nonce);
        sealed.extend(sealed_tail);
        assert_eq!(
            decrypt_cookie_value(&sealed, &EncryptionKeys::Gcm { key }),
            DecryptOutcome::Plain(b"gcm-value".to_vec())
        );
    }

    /// v20은 실패가 아니라 App-Bound다 — 셈해서 경고할 수 있게.
    #[test]
    fn app_bound_is_counted_not_failed() {
        let sealed = b"v20whatever".to_vec();
        assert_eq!(
            decrypt_cookie_value(&sealed, &EncryptionKeys::Gcm { key: [0; 32] }),
            DecryptOutcome::AppBound
        );
        assert_eq!(
            decrypt_cookie_value(b"not-a-version", &EncryptionKeys::Gcm { key: [0; 32] }),
            DecryptOutcome::Failed
        );
    }

    /// 시각: Chromium 1601-epoch 마이크로초와 Safari Mac 초 — 0은 세션.
    #[test]
    fn the_two_foreign_clocks_read_as_unix() {
        // 2020-01-01T00:00:00Z = unix 1577836800
        let unix_2020 = 1_577_836_800i64;
        let chromium = (unix_2020 + WINDOWS_TO_UNIX_EPOCH_SECS) * 1_000_000;
        assert_eq!(chromium_time_to_unix(chromium), Some(unix_2020));
        assert_eq!(chromium_time_to_unix(0), None);
        assert_eq!(
            mac_time_to_unix(unix_2020 as f64 - MAC_EPOCH_DELTA),
            Some(unix_2020)
        );
        assert_eq!(mac_time_to_unix(0.0), None);
    }

    /// 두 브라우저의 sameSite 숫자는 다르게 센다 — Chromium은 -1이
    /// unspecified, Firefox는 0부터가 none이다.
    #[test]
    fn the_two_same_site_scales_map_apart() {
        assert_eq!(SameSite::from_chromium(-1), SameSite::Unspecified);
        assert_eq!(SameSite::from_chromium(0), SameSite::NoRestriction);
        assert_eq!(SameSite::from_chromium(1), SameSite::Lax);
        assert_eq!(SameSite::from_chromium(2), SameSite::Strict);
        assert_eq!(SameSite::from_firefox(0), SameSite::NoRestriction);
        assert_eq!(SameSite::from_firefox(2), SameSite::Strict);
        assert_eq!(SameSite::from_firefox(9), SameSite::Unspecified);
    }

    /// Linux v10의 고정 비밀번호는 Chromium의 것("peanuts", 1회)이다 —
    /// 이 유도가 흔들리면 keyring 없는 리눅스의 모든 쿠키가 안 열린다.
    #[test]
    fn the_linux_fixed_password_derives_the_v10_key() {
        let fixed = linux_cbc_key(LINUX_V10_PASSWORD);
        assert_eq!(fixed, chromium_cbc_key(b"peanuts", 1));
        assert_ne!(
            fixed,
            mac_cbc_key(LINUX_V10_PASSWORD),
            "iteration counts differ"
        );
    }

    /// 구글 규칙: 결속 이름은 구글 도메인에서만 걸리고, google.com 나무는
    /// 통째로 이식 불가다.
    #[test]
    fn google_rules_hold_the_line() {
        assert!(is_google_source_bound("SIDCC", ".google.com"));
        assert!(is_google_source_bound("AEC", "accounts.google.com"));
        assert!(!is_google_source_bound("SIDCC", "example.com"));
        assert!(!is_google_source_bound("plain", "google.com"));
        assert!(is_non_transplantable_domain(".google.com"));
        assert!(is_non_transplantable_domain("mail.google.com"));
        assert!(!is_non_transplantable_domain("notgoogle.com"));
        assert_eq!(
            derive_url(".GitHub.com", true).as_deref(),
            Some("https://github.com/")
        );
    }

    /// 손으로 지은 최소 binarycookies: 1페이지 1쿠키가 그대로 나온다.
    #[test]
    fn a_hand_built_binarycookies_file_decodes() {
        // 레코드: [size][?][flags][?][urlOff][nameOff][pathOff][valueOff]
        //         [?8][expiry f64][creation f64][strings…]
        let url = b".example.com\0";
        let name = b"sid\0";
        let path = b"/\0";
        let value = b"v-1\0";
        let head_len = 56usize;
        let size = head_len + url.len() + name.len() + path.len() + value.len();
        let mut record = Vec::new();
        record.extend((size as u32).to_le_bytes());
        record.extend(0u32.to_le_bytes());
        record.extend(5u32.to_le_bytes()); // secure|httpOnly
        record.extend(0u32.to_le_bytes());
        let url_off = head_len;
        let name_off = url_off + url.len();
        let path_off = name_off + name.len();
        let value_off = path_off + path.len();
        for offset in [url_off, name_off, path_off, value_off] {
            record.extend((offset as u32).to_le_bytes());
        }
        record.extend(0u64.to_le_bytes());
        // 2030-01-01 = unix 1893456000 → mac = unix - delta
        record.extend((1_893_456_000.0f64 - MAC_EPOCH_DELTA).to_le_bytes());
        record.extend(0.0f64.to_le_bytes());
        record.extend_from_slice(url);
        record.extend_from_slice(name);
        record.extend_from_slice(path);
        record.extend_from_slice(value);

        let mut page = Vec::new();
        page.extend(0x0000_0100u32.to_be_bytes());
        page.extend(1u32.to_le_bytes());
        page.extend(12u32.to_le_bytes()); // 레코드 시작(헤더 8 + 표 4)
        page.extend_from_slice(&record);

        let mut file = Vec::new();
        file.extend_from_slice(b"cook");
        file.extend(1u32.to_be_bytes());
        file.extend((page.len() as u32).to_be_bytes());
        file.extend_from_slice(&page);

        let out = decode_binarycookies(&file);
        assert_eq!(out.len(), 1, "one page, one cookie");
        let cookie = &out[0];
        assert_eq!(cookie.name, "sid");
        assert_eq!(cookie.value, "v-1");
        assert_eq!(cookie.domain, ".example.com");
        assert_eq!(cookie.path, "/");
        assert!(cookie.secure && cookie.http_only);
        assert_eq!(cookie.url, "https://example.com/");
        assert_eq!(cookie.expires_unix, Some(1_893_456_000));
        // 매직이 아니면 침묵.
        assert!(decode_binarycookies(b"nope").is_empty());
    }
}
