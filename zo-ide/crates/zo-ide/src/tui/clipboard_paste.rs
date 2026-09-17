//! 시스템 클립보드의 이미지를 다음 제출용 PNG 파일로 고정한다.

use std::path::PathBuf;

use tempfile::Builder;

#[derive(Debug, Clone)]
pub enum PasteImageError {
    ClipboardUnavailable(String),
    NoImage(String),
    EncodeFailed(String),
    IoError(String),
}

impl std::fmt::Display for PasteImageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ClipboardUnavailable(msg) => write!(f, "clipboard unavailable: {msg}"),
            Self::NoImage(msg) => write!(f, "no image on clipboard: {msg}"),
            Self::EncodeFailed(msg) => write!(f, "could not encode image: {msg}"),
            Self::IoError(msg) => write!(f, "io error: {msg}"),
        }
    }
}

impl std::error::Error for PasteImageError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncodedImageFormat {
    Png,
    Jpeg,
    Other,
}

impl EncodedImageFormat {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Png => "PNG",
            Self::Jpeg => "JPEG",
            Self::Other => "IMG",
        }
    }
}

#[derive(Debug, Clone)]
pub struct PastedImageInfo {
    pub width: u32,
    pub height: u32,
    pub encoded_format: EncodedImageFormat,
}

/// Capture an image from the system clipboard and encode it as PNG.
pub fn paste_image_as_png() -> Result<(Vec<u8>, PastedImageInfo), PasteImageError> {
    let mut clipboard = arboard::Clipboard::new()
        .map_err(|error| PasteImageError::ClipboardUnavailable(error.to_string()))?;

    // Sometimes images on the clipboard come as files (e.g. when copy/pasting from
    // Finder), sometimes they come as image data (e.g. when pasting from Chrome).
    // Accept both, and prefer files if both are present.
    let files = clipboard
        .get()
        .file_list()
        .map_err(|error| PasteImageError::ClipboardUnavailable(error.to_string()));
    let image = if let Some(image) = files
        .unwrap_or_default()
        .into_iter()
        .find_map(|path| image::open(path).ok())
    {
        image
    } else {
        let image = clipboard
            .get_image()
            .map_err(|error| PasteImageError::NoImage(error.to_string()))?;
        let width = u32::try_from(image.width)
            .map_err(|_| PasteImageError::EncodeFailed("invalid image dimensions".into()))?;
        let height = u32::try_from(image.height)
            .map_err(|_| PasteImageError::EncodeFailed("invalid image dimensions".into()))?;
        let Some(image) = image::RgbaImage::from_raw(width, height, image.bytes.into_owned())
        else {
            return Err(PasteImageError::EncodeFailed("invalid RGBA buffer".into()));
        };
        image::DynamicImage::ImageRgba8(image)
    };

    let mut png = Vec::new();
    let mut cursor = std::io::Cursor::new(&mut png);
    image
        .write_to(&mut cursor, image::ImageFormat::Png)
        .map_err(|error| PasteImageError::EncodeFailed(error.to_string()))?;
    Ok((
        png,
        PastedImageInfo {
            width: image.width(),
            height: image.height(),
            encoded_format: EncodedImageFormat::Png,
        },
    ))
}

/// Write the clipboard image to a persistent temporary PNG file.
pub fn paste_image_to_temp_png() -> Result<(PathBuf, PastedImageInfo), PasteImageError> {
    let (png, info) = paste_image_as_png()?;
    let temporary = Builder::new()
        .prefix("zo-clipboard-")
        .suffix(".png")
        .tempfile()
        .map_err(|error| PasteImageError::IoError(error.to_string()))?;
    std::fs::write(temporary.path(), png)
        .map_err(|error| PasteImageError::IoError(error.to_string()))?;
    let (_file, path) = temporary
        .keep()
        .map_err(|error| PasteImageError::IoError(error.error.to_string()))?;
    Ok((path, info))
}

/// Normalize pasted text that may represent one filesystem path.
///
/// The terminal can supply a quoted path, a `file://` URL, or a shell-escaped
/// path. More than one shell token is intentionally not a path: its caller
/// must preserve that paste as normal text.
#[must_use]
pub fn normalize_pasted_path(pasted: &str) -> Option<PathBuf> {
    let pasted = pasted.trim();
    let unquoted = pasted
        .strip_prefix('"')
        .and_then(|path| path.strip_suffix('"'))
        .or_else(|| pasted.strip_prefix('\'').and_then(|path| path.strip_suffix('\'')))
        .unwrap_or(pasted);

    if let Ok(url) = url::Url::parse(unquoted) {
        if url.scheme() == "file" {
            return url.to_file_path().ok();
        }
    }

    if let Some(path) = normalize_windows_path(unquoted) {
        return Some(path);
    }

    let parts: Vec<String> = shlex::Shlex::new(pasted).collect();
    if parts.len() == 1 {
        let path = parts.into_iter().next()?;
        return Some(normalize_windows_path(&path).unwrap_or_else(|| PathBuf::from(path)));
    }

    None
}

#[cfg(target_os = "linux")]
fn is_probably_wsl() -> bool {
    std::fs::read_to_string("/proc/version")
        .ok()
        .is_some_and(|version| {
            let version = version.to_lowercase();
            version.contains("microsoft") || version.contains("wsl")
        })
        || std::env::var_os("WSL_DISTRO_NAME").is_some()
        || std::env::var_os("WSL_INTEROP").is_some()
}

#[cfg(target_os = "linux")]
fn convert_windows_path_to_wsl(input: &str) -> Option<PathBuf> {
    if input.starts_with("\\\\") {
        return None;
    }

    let drive_letter = input.chars().next()?.to_ascii_lowercase();
    if !drive_letter.is_ascii_lowercase() || input.get(1..2) != Some(":") {
        return None;
    }

    let mut result = PathBuf::from(format!("/mnt/{drive_letter}"));
    for component in input
        .get(2..)?
        .trim_start_matches(['\\', '/'])
        .split(['\\', '/'])
        .filter(|component| !component.is_empty())
    {
        result.push(component);
    }
    Some(result)
}

fn normalize_windows_path(input: &str) -> Option<PathBuf> {
    let drive = input.chars().next().is_some_and(|ch| ch.is_ascii_alphabetic())
        && input.get(1..2) == Some(":")
        && input.get(2..3).is_some_and(|separator| separator == "\\" || separator == "/");
    let unc = input.starts_with("\\\\");
    if !drive && !unc {
        return None;
    }

    #[cfg(target_os = "linux")]
    if is_probably_wsl() {
        if let Some(path) = convert_windows_path_to_wsl(input) {
            return Some(path);
        }
    }

    Some(PathBuf::from(input))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{normalize_pasted_path, EncodedImageFormat, PasteImageError};

    #[test]
    fn codex_error_messages_are_human_readable() {
        assert_eq!(
            PasteImageError::ClipboardUnavailable("offline".into()).to_string(),
            "clipboard unavailable: offline"
        );
        assert_eq!(
            PasteImageError::NoImage("empty".into()).to_string(),
            "no image on clipboard: empty"
        );
        assert_eq!(
            PasteImageError::EncodeFailed("bad rgba".into()).to_string(),
            "could not encode image: bad rgba"
        );
        assert_eq!(
            PasteImageError::IoError("disk full".into()).to_string(),
            "io error: disk full"
        );
    }

    #[test]
    fn png_is_the_encoded_format() {
        assert_eq!(EncodedImageFormat::Png.label(), "PNG");
    }

    #[test]
    fn normalizes_a_quoted_path_with_spaces() {
        assert_eq!(
            normalize_pasted_path("  \"/tmp/an image.png\"  "),
            Some(PathBuf::from("/tmp/an image.png"))
        );
    }

    #[test]
    fn normalizes_a_file_url() {
        assert_eq!(
            normalize_pasted_path("file:///tmp/an%20image.png"),
            Some(PathBuf::from("/tmp/an image.png"))
        );
    }

    #[test]
    fn leaves_multiple_tokens_for_normal_text_pasting() {
        assert_eq!(normalize_pasted_path("/tmp/one.png /tmp/two.png"), None);
    }
}
