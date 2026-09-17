//! A screenshot leaves the bridge as a file, not as a megabyte of base64.

use std::path::PathBuf;

use serde_json::Value;

/// Where exported frames go: a private directory the window owns. `/tmp`
/// where a mode can be set on it; the user's temp directory on Windows,
/// whose default ACL is already the user's own.
fn export_dir() -> PathBuf {
    #[cfg(unix)]
    {
        PathBuf::from("/tmp/zerocode-computer-use")
    }
    #[cfg(not(unix))]
    {
        std::env::temp_dir().join("zerocode-computer-use")
    }
}

/// The PNG a provider answer carries (`screenshot.data`, base64) — the one
/// reader every consumer of a frame uses.
#[must_use]
pub fn screenshot_png(value: &Value) -> Option<Vec<u8>> {
    use base64::Engine as _;
    let data = value.pointer("/screenshot/data").and_then(Value::as_str)?;
    base64::engine::general_purpose::STANDARD.decode(data).ok()
}

/// Export screenshot bytes out of the bridge response so JSON stays small and
/// local file permissions protect the image, and hand the bytes back to a
/// caller that keeps the frame too. A response without a screenshot, or one
/// whose bytes cannot be written, is left as it is.
pub fn export_screenshot(value: &mut Value) -> Option<Vec<u8>> {
    let bytes = screenshot_png(value)?;
    let directory = export_dir();
    if std::fs::create_dir_all(&directory).is_err() {
        return None;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let _ = std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700));
    }
    let file = directory.join(format!("{}-screenshot.png", uuid::Uuid::new_v4()));
    if std::fs::write(&file, &bytes).is_err() {
        return None;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let _ = std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600));
    }
    if let Some(screenshot) = value.get_mut("screenshot").and_then(Value::as_object_mut) {
        screenshot.remove("data");
        screenshot.insert(
            "path".into(),
            Value::String(file.to_string_lossy().into_owned()),
        );
        screenshot.insert("dataOmitted".into(), Value::Bool(true));
    }
    Some(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_bytes_become_a_private_file_and_the_json_names_it() {
        use base64::Engine as _;
        let png = b"\x89PNG\r\n\x1a\nnot really";
        let mut answer = json!({
            "snapshot": { "id": "S" },
            "screenshot": {
                "data": base64::engine::general_purpose::STANDARD.encode(png),
                "format": "png",
                "width": 4,
                "height": 4,
                "scale": 1.0
            }
        });
        assert_eq!(
            export_screenshot(&mut answer).as_deref(),
            Some(&png[..]),
            "the bytes written come back to a caller that keeps the frame"
        );
        let shot = answer["screenshot"].as_object().unwrap();
        assert!(shot.get("data").is_none());
        assert_eq!(shot["dataOmitted"], true);
        assert_eq!(shot["format"], "png", "the other keys stay");
        let path = PathBuf::from(shot["path"].as_str().unwrap());
        assert!(path.starts_with(export_dir()));
        assert_eq!(std::fs::read(&path).unwrap(), png);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        let _ = std::fs::remove_file(path);

        let mut plain = json!({ "snapshot": { "id": "S" }, "screenshot": null });
        let before = plain.clone();
        assert!(export_screenshot(&mut plain).is_none());
        assert_eq!(plain, before, "nothing to export leaves the answer alone");
    }
}
