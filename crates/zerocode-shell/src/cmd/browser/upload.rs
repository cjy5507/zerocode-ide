//! Explicit local files into a page's file input or drag-and-drop uploader.
//! File bytes stay out of replies and step logs. Site acceptance is a later wait/read.

use super::*;
use base64::Engine;
use std::io::Read;
use std::path::Path;
use zerocode_core::agent_browser::UploadCommand;

const UPLOAD_BYTE_CAP: u64 = 16 * 1024 * 1024;

#[derive(Debug, Serialize)]
struct UploadFile {
    name: String,
    mime: &'static str,
    data: String,
}

fn file_mime(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "heic" => "image/heic",
        "heif" => "image/heif",
        "pdf" => "application/pdf",
        "txt" => "text/plain",
        "csv" => "text/csv",
        _ => "application/octet-stream",
    }
}

fn read_files(paths: &[String]) -> Result<Vec<UploadFile>, String> {
    let mut left = UPLOAD_BYTE_CAP;
    let mut files = Vec::with_capacity(paths.len());
    for path in paths {
        let path = Path::new(path);
        // No implicit app cwd, globbing or directory traversal. The caller
        // chooses each complete path, just as in the native file picker.
        if !path.is_absolute() {
            return Err("upload_path_invalid: 절대 파일 경로를 주세요".into());
        }
        if !std::fs::metadata(path)
            .map_err(|_| "upload_file_unreadable: 파일 정보를 읽을 수 없습니다")?
            .is_file()
        {
            return Err("upload_not_file: 일반 파일만 올릴 수 있습니다".into());
        }
        let file = std::fs::File::open(path)
            .map_err(|_| "upload_file_unreadable: 파일을 열 수 없습니다")?;
        let metadata = file
            .metadata()
            .map_err(|_| "upload_file_unreadable: 파일 정보를 읽을 수 없습니다")?;
        if !metadata.is_file() {
            return Err("upload_not_file: 일반 파일만 올릴 수 있습니다".into());
        }
        if metadata.len() > left {
            return Err("upload_too_large: 파일 합계는 16 MiB까지입니다".into());
        }
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or("upload_name_invalid: 파일 이름을 읽을 수 없습니다")?;
        // Bound the read itself too: a file may grow after metadata().
        let mut bytes = Vec::new();
        file.take(left + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| "upload_file_unreadable: 파일을 읽을 수 없습니다")?;
        if bytes.len() as u64 > left {
            return Err("upload_too_large: 파일 합계는 16 MiB까지입니다".into());
        }
        left -= bytes.len() as u64;
        files.push(UploadFile {
            name: name.into(),
            mime: file_mime(path),
            data: base64::engine::general_purpose::STANDARD.encode(bytes),
        });
    }
    Ok(files)
}

pub(crate) async fn automate_upload(
    app: &AppHandle,
    state: &AppState,
    command: UploadCommand,
) -> Result<serde_json::Value, BrowserRefusal> {
    checked_selector(&command.selector).map_err(BrowserRefusal::Unsent)?;
    let pane = browser_pane_of(app, state, &command.label).map_err(BrowserRefusal::Unsent)?;
    let expect = form::known_form(&command.label);
    let files = tauri::async_runtime::spawn_blocking(move || read_files(&command.paths))
        .await
        .map_err(|_| {
            BrowserRefusal::Unsent("upload_read_failed: 파일 읽기를 마치지 못했습니다".into())
        })?
        .map_err(BrowserRefusal::Unsent)?;
    let mut request = form::form_request();
    request["selector"] = serde_json::json!(command.selector);
    request["drop"] = serde_json::json!(command.drop);
    request["files"] = serde_json::json!(files);
    request["expect"] = serde_json::json!(expect);
    let reply = page_json(
        &pane,
        form::form_script(&request, BROWSER_UPLOAD_BODY),
        BROWSER_CALLBACK_DEADLINE,
    )
    .await?;
    input_value(reply)
}

/// Fixed platform events; never arbitrary evaluation from a work-tab caller.
/// Refusals in UNSENT_PAGE_CODES precede the first event and file assignment.
pub(crate) const BROWSER_UPLOAD_BODY: &str = r#"
if (request.expect && zcFormFields(request).print !== request.expect) return zcFail("form_stale");
const selected = zcSelect(request.selector);
if (selected.code) return zcFail(selected.code);
const element = selected.element;
const input = element.matches('input[type="file"]');
if (!request.drop && !input) return zcFail("element_not_editable");
if (element.matches(':disabled') || element.getAttribute('aria-disabled') === 'true') return zcFail("element_disabled");
if (input && !element.multiple && request.files.length > 1) return zcFail("element_not_editable");
let target = element;
if (!zcVisible(target) && input && !request.drop) {
  target = Array.from(element.labels || []).find((label) => zcVisible(label));
}
if (!target || !zcVisible(target)) return zcFail("element_not_visible");
target.scrollIntoView({ block: 'center', inline: 'center', behavior: 'auto' });
const rect = target.getBoundingClientRect();
const hit = document.elementFromPoint(rect.left + rect.width / 2, rect.top + rect.height / 2);
if (!hit || (hit !== target && !target.contains(hit))) return zcFail("element_obscured");
const view = element.ownerDocument.defaultView;
const transfer = new view.DataTransfer();
for (const file of request.files) {
  const raw = atob(file.data);
  const bytes = Uint8Array.from(raw, (char) => char.charCodeAt(0));
  transfer.items.add(new view.File([bytes], file.name, { type: file.mime }));
}
if (request.drop) {
  for (const type of ['dragenter', 'dragover', 'drop']) {
    element.dispatchEvent(new view.DragEvent(type, { bubbles: true, cancelable: true, composed: true, dataTransfer: transfer }));
  }
} else {
  element.files = transfer.files;
  element.dispatchEvent(new view.Event('input', { bubbles: true, composed: true }));
  element.dispatchEvent(new view.Event('change', { bubbles: true, composed: true }));
}
// A drop has no standard readback of the site's state. An input can prove
// its FileList still holds the bytes; neither proves a server accepted them.
const verified = !request.drop && element.isConnected && element.files.length === transfer.files.length
  && Array.from(element.files).every((file, index) => file.name === transfer.files[index].name && file.size === transfer.files[index].size);
return zcEncode({ ok: true, value: { method: request.drop ? 'synthetic-drop' : 'file-input', trustedEvents: false, count: transfer.files.length, verified } });
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upload_reads_only_named_regular_files_and_caps_the_actual_bytes() {
        let dir = std::env::temp_dir().join(format!("browser-upload-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("receipt.txt");
        std::fs::write(&path, b"synthetic receipt").unwrap();
        let files = read_files(&[path.to_string_lossy().into_owned()]).unwrap();
        assert_eq!(files[0].name, "receipt.txt");
        assert_eq!(files[0].mime, "text/plain");
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(&files[0].data)
                .unwrap(),
            b"synthetic receipt"
        );
        assert!(read_files(&[dir.to_string_lossy().into_owned()]).is_err());
        assert!(read_files(&["relative.txt".into()]).is_err());
        std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(UPLOAD_BYTE_CAP + 1)
            .unwrap();
        assert!(
            read_files(&[path.to_string_lossy().into_owned()])
                .unwrap_err()
                .contains("upload_too_large")
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}
