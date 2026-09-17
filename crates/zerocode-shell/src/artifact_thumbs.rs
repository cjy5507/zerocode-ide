//! Rendered thumbnails for the gallery's pages and claude.ai artifacts
//! (t-3233 §3): one hidden pane, one queue, one at a time, one picture per
//! key, no retry.
//!
//! claude.ai's gallery shows each artifact's page as a small picture. This
//! window has the door for that already — the browser panes' native snapshot
//! (`cmd::fs::snapshot_webview_png`, the same WKWebView road
//! `zerocode-browser screenshot` walks) — and what this module adds is the
//! discipline around it:
//!
//! - **A pane of its own, never a person's.** The picture is taken in a
//!   webview labelled [`THUMB_LABEL`], born the first time a card asks and
//!   kept for the window's life, parked outside the window's bounds so it is
//!   never seen and never covers anything. It is not in `browser_panes`: the
//!   agents' `tabs` does not list it, the window's tab strip never draws it,
//!   and no `zerocode-browser` verb can steer it.
//! - **One at a time.** [`RENDER`] is a lock; a second card waits for the
//!   first. The window queues on its side too (bounded by the table's
//!   `thumb_queue_max`), so what reaches here is one ask after another.
//! - **Keyed and cached.** `thumbs/<id>.png` beside `thumbs/<id>.key`; the
//!   key is `mtime:bytes` for a file and `url:updated` for a remote row. A
//!   matching key answers from disk; a changed key renders again.
//! - **A failure is a glyph, once.** A render that fails writes the key with
//!   a failure mark, so the same page is not tried again until it changes.
//! - **The logged-in session.** A remote page is loaded in the default
//!   browser profile's data store — the jar the person's own tabs use — so a
//!   private artifact renders as itself, not as claude.ai's sign-in wall.

use std::path::Path;
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Manager as _};
use zerocode_core::artifact::{Artifact, ArtifactKind, Limits};

use crate::AppState;
use crate::ShellStateExt as _;
use crate::artifact_runtime::{Store, THUMBS_DIR_NAME};
use crate::browser_runtime::{blank_page, browsable, browsable_target};

/// The hidden pane's label. Not a `browser-N`, on purpose: the mint and the
/// agents' door only ever name those.
pub(crate) const THUMB_LABEL: &str = "artifact-thumb";
/// How far outside the window the pane is parked (logical pixels). Far
/// enough that no resize brings it on screen.
const PARKED_AT: f64 = -20_000.0;
/// How often the wait for a page's `Finished` looks, inside the table's
/// timeout.
const LOAD_POLL: Duration = Duration::from_millis(50);
/// The mark a failed render leaves in its key file.
const FAILED_MARK: &str = " !failed";

/// What the window is answered: the picture, or nothing — and nothing is
/// final for this key.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct Thumb {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) data_url: Option<String>,
    /// Whether the picture came from the cache rather than a render.
    pub(crate) cached: bool,
}

/// The one-at-a-time lock over the hidden pane.
fn render_lock() -> &'static tokio::sync::Mutex<()> {
    static RENDER: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    RENDER.get_or_init(|| tokio::sync::Mutex::new(()))
}

/// What the hidden pane last finished loading — written by its own
/// `on_page_load`, read by the wait below. `None` until a Finished arrives.
fn finished_url() -> &'static Mutex<Option<String>> {
    static CELL: OnceLock<Mutex<Option<String>>> = OnceLock::new();
    CELL.get_or_init(|| Mutex::new(None))
}

/// The key a row renders under: a local file's `mtime:bytes`, a remote
/// row's `url:updated`. `None` for a row that has no picture to render.
pub(crate) fn key_of(artifact: &Artifact) -> Option<String> {
    if !artifact.kind.renders_thumbnail() {
        return None;
    }
    if artifact.kind == ArtifactKind::Web {
        let url = artifact.url.as_deref()?;
        return Some(format!("{url}:{}", artifact.modified_ms));
    }
    let meta = std::fs::metadata(&artifact.path).ok()?;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|held| held.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |since| since.as_millis());
    let stamp = format!("{mtime}:{}", meta.len());
    Some(
        artifact
            .version
            .map_or_else(|| stamp.clone(), |version| format!("{stamp}:v{version}")),
    )
}

/// The address the hidden pane loads for a row: the artifact's url, or the
/// page's `file://` — through the same allowlist every browser pane walks.
pub(crate) fn address_of(artifact: &Artifact) -> Result<tauri::Url, String> {
    match artifact.kind {
        ArtifactKind::Web => browsable(artifact.url.as_deref().unwrap_or_default()),
        ArtifactKind::Page => tauri::Url::from_file_path(&artifact.path)
            .map_err(|()| "페이지 경로를 주소로 만들 수 없습니다".to_string())
            .and_then(|url| browsable(url.as_str())),
        _ => Err("이 종류는 썸네일을 그리지 않습니다".to_string()),
    }
}

fn data_url_of(png: &[u8]) -> String {
    use base64::Engine as _;
    format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(png)
    )
}

/// What the cache holds for one key: the picture, a recorded failure, or
/// nothing yet.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Cached {
    Picture(Vec<u8>),
    Failed,
    Nothing,
}

/// Read the cache for `id` under `key` — the pure half of the road, so the
/// tests can pin it without a webview.
pub(crate) fn cached(store_root: &Path, id: &str, key: &str) -> Cached {
    let dir = store_root.join(THUMBS_DIR_NAME);
    let Ok(held) = std::fs::read_to_string(dir.join(format!("{id}.key"))) else {
        return Cached::Nothing;
    };
    if held == format!("{key}{FAILED_MARK}") {
        return Cached::Failed;
    }
    if held != key {
        return Cached::Nothing;
    }
    match std::fs::read(dir.join(format!("{id}.png"))) {
        Ok(png) if !png.is_empty() => Cached::Picture(png),
        _ => Cached::Nothing,
    }
}

/// Write a picture and its key beside each other — the picture first, so a
/// key without a picture (a crash between the two) reads as nothing.
pub(crate) fn remember(store_root: &Path, id: &str, key: &str, png: &[u8]) -> Result<(), String> {
    let dir = store_root.join(THUMBS_DIR_NAME);
    std::fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
    let tmp = dir.join(format!("{id}.png.tmp"));
    std::fs::write(&tmp, png).map_err(|error| error.to_string())?;
    std::fs::rename(&tmp, dir.join(format!("{id}.png"))).map_err(|error| error.to_string())?;
    std::fs::write(dir.join(format!("{id}.key")), key).map_err(|error| error.to_string())
}

/// Record that this key failed to render, so it is not tried again.
pub(crate) fn remember_failure(store_root: &Path, id: &str, key: &str) {
    let dir = store_root.join(THUMBS_DIR_NAME);
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::write(dir.join(format!("{id}.key")), format!("{key}{FAILED_MARK}"));
}

/// The window's ask: the picture for one row, from the cache or from one
/// render of the hidden pane. `Ok(Thumb { data_url: None, .. })` is "a glyph,
/// and do not ask again for this key" — a kind that has no picture, a row
/// whose file is gone, or a render that failed.
pub(crate) async fn thumbnail(app: &AppHandle, store: &Store, id: &str) -> Result<Thumb, String> {
    let artifact = store
        .get(id)
        .ok_or_else(|| "그 아티팩트가 없습니다".to_string())?;
    let Some(key) = key_of(&artifact) else {
        return Ok(Thumb {
            data_url: None,
            cached: true,
        });
    };
    match cached(store.root(), id, &key) {
        Cached::Picture(png) => {
            return Ok(Thumb {
                data_url: Some(data_url_of(&png)),
                cached: true,
            });
        }
        Cached::Failed => {
            return Ok(Thumb {
                data_url: None,
                cached: true,
            });
        }
        Cached::Nothing => {}
    }
    let address = address_of(&artifact)?;
    let limits = store.limits();
    // One at a time: the pane is one, and two navigations in flight would
    // snap one page under the other's name.
    let _turn = render_lock().lock().await;
    // Another ask for the same row may have rendered it while this one
    // waited for the lock.
    match cached(store.root(), id, &key) {
        Cached::Picture(png) => {
            return Ok(Thumb {
                data_url: Some(data_url_of(&png)),
                cached: true,
            });
        }
        Cached::Failed => {
            return Ok(Thumb {
                data_url: None,
                cached: true,
            });
        }
        Cached::Nothing => {}
    }
    match render(app, &address, &limits).await {
        Ok(png) => {
            remember(store.root(), id, &key, &png)?;
            Ok(Thumb {
                data_url: Some(data_url_of(&png)),
                cached: false,
            })
        }
        Err(why) => {
            remember_failure(store.root(), id, &key);
            crate::system_runtime::note_window_event(
                store.local_data_root(),
                &format!("artifacts: thumbnail of {id} failed — {why}"),
            );
            Ok(Thumb {
                data_url: None,
                cached: false,
            })
        }
    }
}

/// Load `address` in the hidden pane, wait for its Finished inside the
/// table's timeout, and take the picture at the table's width.
async fn render(app: &AppHandle, address: &tauri::Url, limits: &Limits) -> Result<Vec<u8>, String> {
    let pane = pane(app, limits)?;
    *finished_url()
        .lock()
        .unwrap_or_else(PoisonError::into_inner) = None;
    pane.navigate(address.clone())
        .map_err(|error| error.to_string())?;
    let deadline = std::time::Instant::now() + Duration::from_millis(limits.thumb_timeout_ms);
    loop {
        if finished_url()
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_deref()
            .is_some_and(|held| held != blank_page().as_str())
        {
            break;
        }
        if std::time::Instant::now() >= deadline {
            // Leave the pane on the blank page: a page still loading must
            // not keep spending the person's network after its picture was
            // given up on.
            let _ = pane.navigate(blank_page());
            return Err(format!(
                "페이지가 {} ms 안에 뜨지 않았습니다",
                limits.thumb_timeout_ms
            ));
        }
        tokio::time::sleep(LOAD_POLL).await;
    }
    // One more beat for the first paint: Finished is the document, not
    // the pixels, and a snapshot taken on the same tick is often blank.
    tokio::time::sleep(LOAD_POLL).await;
    let png =
        crate::cmd::fs::snapshot_webview_png(pane.clone(), Some(f64::from(limits.thumb_width)))
            .await;
    let _ = pane.navigate(blank_page());
    png
}

/// The hidden pane, born on the first ask and kept. Sized for reading
/// (`thumb_viewport_width`, 4:3) so a page lays itself out as a page; the
/// snapshot is asked for at card width. Parked outside the window's bounds.
fn pane(app: &AppHandle, limits: &Limits) -> Result<tauri::Webview, String> {
    if let Some(held) = app.get_webview(THUMB_LABEL) {
        return Ok(held);
    }
    let window = app
        .get_window("main")
        .ok_or_else(|| "창이 없습니다".to_string())?;
    let state = app.state::<AppState>();
    let width = f64::from(limits.thumb_viewport_width.max(1));
    let height =
        width * f64::from(limits.thumb_height.max(1)) / f64::from(limits.thumb_width.max(1));
    let builder =
        tauri::webview::WebviewBuilder::new(THUMB_LABEL, tauri::WebviewUrl::External(blank_page()))
            // The same allowlist every browser pane walks: a page cannot send the
            // thumbnail pane somewhere the address bar would have refused.
            .on_navigation(browsable_target)
            // A popup from a thumbnail is nobody's business — denied, not routed.
            .on_new_window(|_, _| tauri::webview::NewWindowResponse::Deny)
            .on_page_load(move |_, payload| {
                if matches!(payload.event(), tauri::webview::PageLoadEvent::Finished) {
                    *finished_url()
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner) = Some(payload.url().to_string());
                }
            });
    // The person's own browser session: the default profile's jar, so a
    // private claude.ai artifact renders as itself.
    #[cfg(target_os = "macos")]
    let builder = match crate::browser_runtime::load_default_browser_profile(state.config_root())
        .as_deref()
        .and_then(crate::browser_runtime::browser_profile_store)
    {
        Some(bytes) => builder.data_store_identifier(bytes),
        None => builder,
    };
    let builder = match crate::browser_user_agent::desktop_user_agent_for_this_machine() {
        Some(name) => builder.user_agent(&name),
        None => builder,
    };
    let pane = window
        .add_child(
            builder,
            tauri::LogicalPosition::new(PARKED_AT, PARKED_AT),
            tauri::LogicalSize::new(width, height),
        )
        .map_err(|error| error.to_string())?;
    // A native pane being born can take the window's first responder with
    // it; the keyboard goes back to the main webview (the same mercy
    // `browser_place` shows when it hides a pane).
    if let Some(main) = app.get_webview("main") {
        let _ = main.set_focus();
    }
    Ok(pane)
}

#[cfg(test)]
mod tests {
    use super::*;
    use zerocode_core::artifact::{Origin, Preview, Source};

    fn row(kind: ArtifactKind, path: &Path, url: Option<&str>) -> Artifact {
        Artifact {
            id: "art".into(),
            kind,
            title: "t".into(),
            path: path.to_path_buf(),
            bytes: 0,
            created_ms: 1,
            modified_ms: 2,
            url: url.map(str::to_string),
            favicon: None,
            description: None,
            version: None,
            source_path: None,
            origin: Origin::default(),
            tags: Vec::new(),
            preview: Preview::None,
            source: Source::Manual,
        }
    }

    /// The key: a file's stamp, a remote row's url and time, nothing for a
    /// kind that has no picture; the address walks the browser allowlist.
    #[test]
    fn artifact_publication_version_invalidates_a_thumbnail_even_with_the_same_file_stamp() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.html");
        std::fs::write(&path, "<main>hi</main>").unwrap();
        let mut artifact = row(ArtifactKind::Page, &path, None);
        artifact.version = Some(1);
        let first = key_of(&artifact);
        artifact.version = Some(2);
        assert_ne!(first, key_of(&artifact));
    }

    #[test]
    fn the_key_is_the_files_stamp_or_the_urls_time_and_the_address_is_allowlisted() {
        let dir = tempfile::tempdir().expect("tempdir");
        let page = dir.path().join("index.html");
        std::fs::write(&page, "<p>hi</p>").expect("write");
        let local = key_of(&row(ArtifactKind::Page, &page, None)).expect("a key");
        assert!(local.ends_with(":9"), "{local}");
        std::fs::write(&page, "<p>hi there</p>").expect("write");
        let moved = key_of(&row(ArtifactKind::Page, &page, None)).expect("a key");
        assert_ne!(local, moved, "a rewritten file kept its key");
        assert_eq!(
            key_of(&row(
                ArtifactKind::Web,
                Path::new(""),
                Some("https://claude.ai/code/artifact/x")
            )),
            Some("https://claude.ai/code/artifact/x:2".into())
        );
        assert_eq!(key_of(&row(ArtifactKind::Document, &page, None)), None);
        assert_eq!(
            key_of(&row(ArtifactKind::Page, Path::new("/nowhere.html"), None)),
            None
        );
        assert_eq!(
            address_of(&row(ArtifactKind::Page, &page, None))
                .expect("a file url")
                .scheme(),
            "file"
        );
        assert!(address_of(&row(ArtifactKind::Web, Path::new(""), Some("javascript:1"))).is_err());
        assert!(address_of(&row(ArtifactKind::Document, &page, None)).is_err());
    }

    /// The cache answers only under its key, a failure is remembered under
    /// its key, and a new key reads as nothing — the render happens once
    /// per key, whichever way it went.
    #[test]
    fn the_cache_answers_by_key_and_remembers_a_failure_once() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        assert_eq!(cached(root, "a", "k1"), Cached::Nothing);
        remember(root, "a", "k1", b"png-bytes").expect("remembers");
        assert_eq!(
            cached(root, "a", "k1"),
            Cached::Picture(b"png-bytes".to_vec())
        );
        assert_eq!(
            cached(root, "a", "k2"),
            Cached::Nothing,
            "a new key reads the old picture"
        );
        remember_failure(root, "a", "k2");
        assert_eq!(cached(root, "a", "k2"), Cached::Failed);
        assert_eq!(
            cached(root, "a", "k1"),
            Cached::Nothing,
            "the failure did not unseat the key"
        );
        remember(root, "a", "k3", b"new").expect("remembers");
        assert_eq!(cached(root, "a", "k3"), Cached::Picture(b"new".to_vec()));
        assert!(root.join(THUMBS_DIR_NAME).join("a.png").is_file());
        assert!(!root.join(THUMBS_DIR_NAME).join("a.png.tmp").exists());
    }
}
