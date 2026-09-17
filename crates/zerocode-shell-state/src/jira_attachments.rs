//! Jira 첨부의 로컬 실체화 — 경로와 한도, 바이트의 검문.
//!
//! 네트워크는 여기 없다(`main.rs`의 한 문이 그 일이다). 이 모듈이 아는 것은
//! 셋뿐이다: 첨부가 local_data 아래 **어디에** 앉는가, **무엇이** 앉을 수
//! 있는가(MIME·크기·개수의 이름 붙은 한도), 그리고 **어떻게** 찢기지 않게
//! 앉는가(durable_file의 스테이징+rename). 순수한 판단들이라 사이트도 토큰도
//! 없이 표 하나로 시험된다 — `zerocode-shell`의 `icon`이 제 캐시에 대해 하는
//! 것과 같은 자리다(저쪽은 바이너리 크레이트라 링크로는 못 짚는다).

use std::io;
use std::path::{Path, PathBuf};

/// local_data 아래 이 캐시의 방 이름 — `app_paths`의 이주 목록이 같은 값을
/// 든다(재생성 가능한 캐시라 `Recreate`).
pub const DIR_NAME: &str = "jira-attachments";

/// 첨부 하나가 실체화될 수 있는 최대 크기. 그 너머는 카드와 프롬프트의
/// 물건이 아니라 브라우저의 몫이다 — 댓글 200과 같은 자리의 같은 판단.
pub const FILE_LIMIT_BYTES: usize = 8 * 1024 * 1024;
/// 이슈 하나가 한 번의 실체화에서 차지할 수 있는 총량. 첨부 스무 장이
/// 각각 한도 안이어도 디스크와 시간은 합으로 겪는다.
pub const TOTAL_LIMIT_BYTES: usize = 24 * 1024 * 1024;
/// 이슈 하나에서 실체화하는 첨부 수의 상한. 그 밖의 첨부는 manifest에
/// 생략 사유로 남는다 — 조용히 사라지는 첨부는 없다.
pub const COUNT_MAX: usize = 12;
/// 카드 미리보기(데이터 URI)가 허용하는 원본 크기. base64는 4/3으로
/// 부풀므로 이 값이 곧 렌더러가 드는 문자열의 상한이다.
pub const PREVIEW_LIMIT_BYTES: usize = 256 * 1024;

/// 실체화가 허용되는 MIME. 접두사 둘과 정확한 값 셋 — 에이전트가 경로로
/// 읽어 쓸모 있는 갈래만. 실행 파일·아카이브는 여기 없다: 목록에 없는
/// 것은 내려받지 않는 것이지, 내려받고 숨기는 것이 아니다.
pub const MIME_ALLOW_PREFIXES: [&str; 2] = ["image/", "text/"];
pub const MIME_ALLOW_EXACT: [&str; 3] = ["application/pdf", "application/json", "application/xml"];

/// 이 MIME이 실체화될 수 있는가. `; charset=` 같은 매개변수는 갈래가 아니라
/// 표기라 판단 밖이다.
#[must_use]
pub fn mime_allowed(mime: &str) -> bool {
    let mime = mime
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    MIME_ALLOW_EXACT.iter().any(|one| *one == mime)
        || MIME_ALLOW_PREFIXES
            .iter()
            .any(|prefix| mime.starts_with(prefix))
}

/// 경로의 한 칸이 될 수 있는 모양 — 사이트 id는 URL-safe base64 24자다.
/// 그 밖의 글자는 id가 아니라 경로 조작이고, 인코딩이 아니라 거절이 답이다.
fn safe_segment(segment: &str) -> bool {
    !segment.is_empty()
        && segment.len() <= 64
        && segment
            .chars()
            .all(|one| one.is_ascii_alphanumeric() || one == '-' || one == '_')
}

/// 이 이슈의 캐시 방 — site id와 키가 경로에 설 수 있는 모양일 때만.
#[must_use]
pub fn issue_dir(local_data_root: &Path, site_id: &str, key: &str) -> Option<PathBuf> {
    (safe_segment(site_id) && zerocode_core::jira::valid_issue_key(key))
        .then(|| local_data_root.join(DIR_NAME).join(site_id).join(key))
}

/// 첨부 하나가 앉는 파일 — 이름의 안전한 모양은 core가 정한다.
#[must_use]
pub fn content_file(
    local_data_root: &Path,
    site_id: &str,
    key: &str,
    attachment_id: &str,
    name: &str,
) -> Option<PathBuf> {
    if !zerocode_core::jira::valid_attachment_id(attachment_id) {
        return None;
    }
    Some(
        issue_dir(local_data_root, site_id, key)?.join(zerocode_core::jira::attachment_file_name(
            attachment_id,
            name,
        )),
    )
}

/// 미리보기(썸네일)가 앉는 파일 — 같은 방의 `{id}.thumb`.
/// 본문 파일은 언제나 `{id}-…`라 이 이름과 충돌할 수 없다.
#[must_use]
pub fn thumbnail_file(
    local_data_root: &Path,
    site_id: &str,
    key: &str,
    attachment_id: &str,
) -> Option<PathBuf> {
    if !zerocode_core::jira::valid_attachment_id(attachment_id) {
        return None;
    }
    Some(issue_dir(local_data_root, site_id, key)?.join(format!("{attachment_id}.thumb")))
}

/// 찢기지 않게 놓기 — 방을 만들고(0700), 스테이징에 다 쓴 뒤 rename.
/// 반쯤 내려받힌 첨부가 캐시 히트로 읽히는 일이 없도록, 보이는 파일은
/// 언제나 온전한 파일이다.
pub fn store_bytes(target: &Path, bytes: &[u8]) -> io::Result<()> {
    crate::durable_file::replace_bytes(target, bytes).map(|_| ())
}

/// 바이트가 실제로 그리는 그림의 MIME — 확장자도 헤더도 아닌 매직 바이트로.
/// 오류 페이지 HTML이 캐시에 앉아도 미리보기가 되지 못하는 이유다
/// (`icon.rs`의 그 규칙).
#[must_use]
pub fn sniff_image(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        Some("image/png")
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.len() > 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

/// 미리보기 데이터 URI — CSP의 `img-src`가 이미 허락한 그 모양(`data:`)으로,
/// 실측된 그림일 때만, [`PREVIEW_LIMIT_BYTES`] 안에서.
#[must_use]
pub fn image_data_uri(bytes: &[u8]) -> Option<String> {
    use base64::Engine as _;
    if bytes.len() > PREVIEW_LIMIT_BYTES {
        return None;
    }
    let mime = sniff_image(bytes)?;
    Some(format!(
        "data:{mime};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 방을 벗어날 수 있는 어떤 조각도 경로가 되지 못한다 — 사이트 id, 키,
    /// 첨부 id, 이름 넷 다.
    #[test]
    fn a_piece_that_could_leave_the_room_never_becomes_a_path() {
        let root = Path::new("/data");
        for bad_site in ["", "..", "a/b", "a\\b", "site id", &"x".repeat(65)] {
            assert!(
                issue_dir(root, bad_site, "ABC-9").is_none(),
                "site `{bad_site}` stood as a path"
            );
        }
        for bad_key in ["", "ABC-9/../up", "ABC 9"] {
            assert!(
                issue_dir(root, "site24", bad_key).is_none(),
                "key `{bad_key}` stood as a path"
            );
        }
        for bad_id in ["", "10/1", "abc", "-1"] {
            assert!(
                content_file(root, "site24", "ABC-9", bad_id, "a.png").is_none(),
                "id `{bad_id}` stood as a path"
            );
            assert!(thumbnail_file(root, "site24", "ABC-9", bad_id).is_none());
        }
        // 적대적 이름은 거절이 아니라 안전한 모양으로 앉는다 — 이름은
        // Jira가 지어 준 것이고, 위험한 것은 이름이 아니라 경로다.
        let stood = content_file(root, "site24", "ABC-9", "10001", "../../etc/passwd")
            .expect("a hostile name still lands inside the room");
        assert_eq!(
            stood,
            Path::new("/data/jira-attachments/site24/ABC-9/10001-passwd")
        );
        assert!(stood.starts_with(root.join(DIR_NAME)));
    }

    /// 썸네일 파일은 본문 파일과 절대 겹치지 않는다 — `thumb`라는 이름의
    /// 첨부가 와도.
    #[test]
    fn a_thumbnail_never_collides_with_a_content_file() {
        let root = Path::new("/data");
        let thumb = thumbnail_file(root, "site24", "ABC-9", "7").expect("a thumb");
        let content = content_file(root, "site24", "ABC-9", "7", "thumb").expect("a content file");
        assert_ne!(thumb, content);
        assert!(thumb.ends_with("7.thumb"));
    }

    /// 허용 목록에 없는 것은 내려받지 않는다 — 접두사와 정확한 값.
    #[test]
    fn only_the_listed_kinds_are_materialized() {
        assert!(mime_allowed("image/png"));
        assert!(mime_allowed("IMAGE/PNG"));
        assert!(mime_allowed("text/plain; charset=utf-8"));
        assert!(mime_allowed("text/plain"));
        assert!(mime_allowed("application/pdf"));
        assert!(!mime_allowed("application/zip"));
        assert!(!mime_allowed("application/x-executable"));
        assert!(!mime_allowed(""));
    }

    /// 보이는 파일은 언제나 온전한 파일이다 — 쓰기는 스테이징을 지나 rename
    /// 으로 착지하고, 같은 자리에 다시 쓰면 마지막 바이트들이 남는다.
    #[test]
    fn a_stored_attachment_lands_whole_and_replaceable() {
        let root = tempfile::tempdir().expect("a room");
        let target =
            content_file(root.path(), "site24", "ABC-9", "10001", "note.txt").expect("a target");
        store_bytes(&target, b"first").expect("stored");
        assert_eq!(std::fs::read(&target).expect("read back"), b"first");
        store_bytes(&target, b"second body").expect("replaced");
        assert_eq!(std::fs::read(&target).expect("read back"), b"second body");
        // 스테이징 잔해가 방에 남지 않는다.
        let siblings: Vec<_> = std::fs::read_dir(target.parent().expect("a parent"))
            .expect("listable")
            .filter_map(Result::ok)
            .map(|entry| entry.file_name())
            .collect();
        assert_eq!(siblings.len(), 1, "{siblings:?}");
    }

    /// 그림이 아닌 바이트는 미리보기가 되지 못한다 — 오류 페이지도, 허용
    /// 크기를 넘는 진짜 그림도.
    #[test]
    fn only_a_measured_image_becomes_a_preview() {
        assert!(image_data_uri(b"<html>404</html>").is_none());
        assert!(sniff_image(b"GIF89a....").is_some());
        let png = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 1, 2, 3];
        let uri = image_data_uri(&png).expect("a data uri");
        assert!(uri.starts_with("data:image/png;base64,"), "{uri}");
        let mut huge = vec![0u8; PREVIEW_LIMIT_BYTES + 1];
        huge[..8].copy_from_slice(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
        assert!(
            image_data_uri(&huge).is_none(),
            "an oversized preview shipped"
        );
    }
}
