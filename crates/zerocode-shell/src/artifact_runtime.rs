//! The artifact store: catalog, bounded scan, previews, retention (t-2720 §2).
//!
//! Everything an agent makes ends up in one of four places today — an
//! automation's evidence folder, a worker's `/tmp/t-<id>-report.md`, a
//! Computer Use or browser screenshot, a workflow's output — and until this
//! module the window knew about the first only. This is the one catalog:
//! `<local data root>/artifacts/<run|automation|manual>/<id>/…` for the
//! files it copies, `index.jsonl` (append-only, one row per line, a tombstone
//! per removal) for what it knows, and one `id → Artifact` map in memory.
//!
//! What this file does NOT do is as deliberate as what it does:
//!
//! - It does not walk the disk on its own clock. A scan is bounded by the
//!   table ([`Limits`]) — depth, files, bytes — and incremental on
//!   modified-time and length, so a folder that has not changed costs one
//!   `stat` per file and no read. The file watcher's artifact lane
//!   (`file_watch`) says WHEN to look; nothing here spawns a thread.
//! - It does not decode an image until the drawer asks for it, and it hands
//!   the window a bounded `data:` URL through an LRU with a byte cap, never a
//!   decoded bitmap held for the life of the window.
//! - It does not know where evidence folders are. The automation runtime
//!   REGISTERS them ([`Store::adopt_source`]) with the origin it holds; this
//!   file only scans what it was handed. The orchestration runtime hands in
//!   worker reports the same way ([`Store::register_kept`]).
//! - It does not delete without being told the person confirmed
//!   ([`Store::delete`] with `confirmed == false` is a no-op), and it never
//!   deletes a file outside the app's own data root.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::SystemTime;

use crate::{AppState, ShellStateExt as _};
use serde::{Deserialize, Serialize};
use zerocode_core::artifact::{
    self, Artifact, ArtifactKind, Limits, Origin, Preview, Source, artifact_id,
    artifact_id_for_url, body_tokens, kind_of, preview_of, query_matches, title_of,
};
use zerocode_core::artifact_publish::{ExportFormat, PageRenderer};
use zerocode_core::artifact_transcript::{PageFact, RemoteFact};

mod kept;

pub(crate) use kept::KeptFile;

/// The store's folder under the local data root.
pub(crate) const STORE_DIR_NAME: &str = "artifacts";
/// The append-only catalog inside it.
pub(crate) const INDEX_FILE: &str = "index.jsonl";
/// The file watcher lane this store's folders ride in.
pub(crate) const WATCH_LANE: &str = "artifacts";
/// The window event that says the catalog moved.
pub(crate) const CHANGED_EVENT: &str = "artifacts:changed";
/// The window event that carries one page the door just published, as the
/// row the catalog now holds (t-11958). The window decides from it alone
/// whether the page opens beside its maker, updates in place or waits
/// behind a 「새 N」 badge; `artifacts:changed` still says the catalog moved.
pub(crate) const PUBLISHED_EVENT: &str = "artifacts:published";
/// Where a page's snapshots live: `<root>/versions/<id>/<n>/<name>` (t-3233 §5).
pub(crate) const VERSIONS_DIR_NAME: &str = "versions";
/// Where a card's rendered thumbnail lives: `<root>/thumbs/<id>.png` (t-3233 §3).
pub(crate) const THUMBS_DIR_NAME: &str = "thumbs";

/// One line of `index.jsonl`: a row, or the removal of one. Untagged so an
/// old build's rows (which know no tombstones) still read, and a tombstone —
/// which has no `id` field — can never parse as a row.
#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum IndexLine {
    Row(Box<Artifact>),
    Tombstone { tombstone: String },
}

/// What a file was when it was last indexed — the cheap question a scan asks
/// before it reads anything, exactly as `file_watch::Snap` asks it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Stamp {
    modified: Option<SystemTime>,
    len: u64,
}

fn stamp_of(path: &Path) -> Option<Stamp> {
    let meta = std::fs::metadata(path).ok()?;
    meta.is_file().then_some(Stamp {
        modified: meta.modified().ok(),
        len: meta.len(),
    })
}

fn epoch_ms(time: Option<SystemTime>) -> i64 {
    time.and_then(|held| held.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |since| since.as_millis() as i64)
}

/// What a row is built from: the file, its identity, its provenance, and
/// the clock — one bundle so the builder reads as one question.
struct RowSeed<'a> {
    path: &'a Path,
    id: String,
    source: Source,
    origin: Origin,
    stamp: Stamp,
    now_ms: i64,
    created_ms: Option<i64>,
}

/// A row as held in memory: the artifact, its search tokens (sorted, bounded
/// by the table), and the stamp the incremental scan compares against.
#[derive(Clone)]
struct Row {
    artifact: Artifact,
    tokens: Vec<String>,
    stamp: Option<Stamp>,
}

/// A folder the scan walks, with the origin every file in it inherits. The
/// caller that knows the folder — the automation runtime for evidence, the
/// settings for an export folder — hands it in; this file never guesses one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ScanSource {
    pub(crate) dir: PathBuf,
    pub(crate) source: Source,
    pub(crate) origin: Origin,
}

/// What one scan pass did, and whether it hit a bound.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct ScanReport {
    pub(crate) seen: usize,
    pub(crate) added: usize,
    pub(crate) updated: usize,
    pub(crate) removed: usize,
    /// A bound of the table was reached before every file was looked at.
    pub(crate) truncated: bool,
}

impl ScanReport {
    pub(crate) fn changed(&self) -> bool {
        self.added + self.updated + self.removed > 0
    }
}

/// What a retention sweep gave up.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Pruned {
    pub(crate) rows: usize,
    pub(crate) files: usize,
}

/// The window's filter, as the view sends it. Every field optional; an empty
/// filter lists everything the table allows.
#[derive(Debug, Default, Clone, Deserialize)]
#[serde(default)]
pub(crate) struct Filter {
    pub(crate) query: String,
    pub(crate) kind: Option<String>,
    pub(crate) run: Option<String>,
    pub(crate) task: Option<String>,
    pub(crate) worker: Option<String>,
    pub(crate) worktree: Option<String>,
    pub(crate) automation: Option<String>,
    pub(crate) agent: Option<String>,
    /// `Some(true)` lists claude.ai artifacts only, `Some(false)` this
    /// machine's only — the gallery's 전체/이 기계/claude.ai segments (t-3233 §4).
    pub(crate) remote: Option<bool>,
    /// `Some(true)` lists publications only — the rows `zerocode-artifact`
    /// can read and export by version (t-3952), exactly what the headless
    /// catalog holds.
    pub(crate) published: Option<bool>,
    /// 갤러리 탭이 묶는 종류. 「페이지·문서」는 page·document·web이고,
    /// 모르는 종류는 `kind`처럼 무시한다. 빈 목록이면 모든 종류를 받는다.
    pub(crate) kinds: Vec<String>,
    /// 창만이 한 프로젝트의 루트와 워크트리를 함께 아니까 그 경로들을 받는다.
    /// 행의 출처 경로가 그 아래에 있으면 속하며, 빈 목록이면 모두 받는다.
    pub(crate) roots: Vec<PathBuf>,
    /// 오늘·이번 주의 경계는 창의 시계와 시간대로 정하고 epoch ms로 건넨다.
    pub(crate) since_ms: Option<i64>,
    /// `Some(true)`는 파일이 있는 행만 받는다. 파일 없는 claude.ai 행은
    /// 늘 살아 있으며, `Some(false)`는 파일이 사라진 행만 받는다.
    pub(crate) present: Option<bool>,
}

/// One snapshot of a page or document (t-3233 §5), or one kept version of a
/// publication (t-3952): the number the picker shows, the immutable file,
/// when it was written, and its SHA-256 — the digest feedback cites so an
/// agent can tell the version it was shown from the source it will edit.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct Version {
    pub(crate) n: u32,
    pub(crate) path: PathBuf,
    pub(crate) bytes: u64,
    pub(crate) modified_ms: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) sha256: Option<String>,
}

/// What a page address names in this store ([`Store::page_at`]): the
/// publication, and the kept version the address is — `None` for the page's
/// current file.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct PageAt {
    pub(crate) artifact: Artifact,
    pub(crate) version: Option<u32>,
}

/// 발행한 페이지에 전달된 주석의 기록(t-11959): `pages/<id>/feedback.jsonl`, 전달
/// 한 번에 한 줄. 덧붙이기만 하고 다시 쓰지 않는다 — 버전 폴더(`v<n>`)가 아니라서
/// 버전 목록·내보내기·주소 판정 어느 것도 이 파일을 판으로 읽지 않는다.
pub(crate) const FEEDBACK_FILE: &str = "feedback.jsonl";
/// 한 페이지의 기록이 자랄 수 있는 끝. 넘으면 초안은 여전히 판에 들어가지만 기록은
/// 거절된다 — 파일을 줄여 자리를 내는 일은 이 파일이 하지 않는다(덧붙이기만).
pub(crate) const FEEDBACK_FILE_MAX_BYTES: u64 = 1024 * 1024;
/// 전달 한 번의 줄이 가질 수 있는 바이트.
pub(crate) const FEEDBACK_LINE_MAX_BYTES: usize = 64 * 1024;
/// 전달 한 번에 실을 수 있는 주석의 수.
pub(crate) const FEEDBACK_ITEMS_MAX: usize = 50;
/// 주석 하나의 선택자와 코멘트가 가질 수 있는 글자 수.
pub(crate) const FEEDBACK_SELECTOR_MAX: usize = 1024;
pub(crate) const FEEDBACK_COMMENT_MAX: usize = 4000;
/// 받는 판의 에이전트 id가 가질 수 있는 글자 수.
pub(crate) const FEEDBACK_AGENT_MAX: usize = 64;
/// 링크를 따라간 페이지의 주소가 가질 수 있는 글자 수(t-14586).
pub(crate) const FEEDBACK_PAGE_URL_MAX: usize = 2048;
/// 내보낼 파일 이름이 이미 있을 때 번호를 붙여 볼 횟수.
const EXPORT_NAME_TRIES: u32 = 100;
/// 내보낼 파일 이름에서 제목이 차지할 수 있는 글자 수.
const EXPORT_STEM_MAX: usize = 60;
/// 이 창이 방금 내보낸 파일을 몇 개까지 기억하는가 — 토스트의 「Finder에서 보기」가 이
/// 목록 안의 파일만 보인다(t-18558).
const EXPORTED_KEPT: usize = 64;

/// 창이 청하는 기록 한 줄: 어느 발행물의 몇 번 판에 단 주석을 어느 판에 넣었는가.
/// 그 판의 SHA와 고칠 원본은 창이 말하지 않는다 — 스토어가 제 기록에서 적는다.
/// 탭이 그 판에서 링크를 따라 발행물이 아닌 페이지로 갔으면 `page_url`이 주석을 단
/// 그 페이지이고, `version`은 탭이 떠날 때 보이던 판이다(t-14586).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FeedbackAsk {
    pub(crate) id: String,
    pub(crate) version: u32,
    pub(crate) items: Vec<FeedbackItem>,
    pub(crate) recipient: FeedbackRecipient,
    #[serde(default)]
    pub(crate) page_url: Option<String>,
}

/// 주석 하나 — 찍은 요소의 선택자와 사람이 쓴 말.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FeedbackItem {
    pub(crate) selector: String,
    pub(crate) comment: String,
}

/// 초안을 받은 판: 창의 판 열쇠(`term-<n>`)와 그 판의 에이전트. 표지일 뿐 무엇도
/// 허락하지 않는다.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FeedbackRecipient {
    pub(crate) pane: String,
    pub(crate) agent: String,
}

/// `feedback.jsonl`의 한 줄. 읽을 때는 모르는 필드를 버린다 — 뒤의 빌드가 보탠
/// 필드가 앞의 줄들을 버리게 하지 않는다.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct FeedbackLine {
    pub(crate) id: String,
    pub(crate) version: u32,
    #[serde(default)]
    pub(crate) sha256: Option<String>,
    #[serde(default)]
    pub(crate) source_path: Option<PathBuf>,
    pub(crate) recipient: FeedbackRecipient,
    pub(crate) items: Vec<FeedbackItem>,
    pub(crate) at_ms: i64,
    /// 판이 아니라 그 판에서 따라간 페이지에 단 주석이면 그 주소. 이 필드가 없던
    /// 빌드의 줄은 없는 채로 읽힌다.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) page_url: Option<String>,
}

/// 한 페이지의 기록을 센 것: 읽히는 줄의 수와 가장 새 줄이 단 판.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub(crate) struct FeedbackSummary {
    pub(crate) count: u32,
    pub(crate) version: Option<u32>,
}

/// 목록 상한이 행을 잘라도 탭과 사라진 파일의 수는 전체를 센다.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct Listing {
    pub(crate) rows: Vec<Artifact>,
    pub(crate) total: usize,
    pub(crate) truncated: bool,
    /// `rows`의 모양은 그대로 두고 사라진 파일의 id만 창에 따로 건넨다.
    /// 창은 디스크에 다시 묻지 않는다.
    pub(crate) missing: Vec<String>,
    /// `present`만 빼고 거르개에 맞는 사라진 파일의 수다.
    pub(crate) missing_total: usize,
    /// 종류만 빼고 거르개에 맞는 전체 행을 종류별로 센다.
    pub(crate) by_kind: BTreeMap<String, usize>,
}

/// Counts by origin, for the chips other surfaces wear — the board card, the
/// task row, the worktree row ask "how many for mine" and nothing else.
#[derive(Debug, Default, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct Counts {
    pub(crate) total: usize,
    pub(crate) by_worker: BTreeMap<String, usize>,
    pub(crate) by_task: BTreeMap<String, usize>,
    pub(crate) by_run: BTreeMap<String, usize>,
    pub(crate) by_worktree: BTreeMap<String, usize>,
    pub(crate) by_automation: BTreeMap<String, usize>,
    /// The newest report per worker — the id 「보고서 보기」 opens from a
    /// ledger row (t-2720 §4), beside the count the chip wears.
    pub(crate) report_by_worker: BTreeMap<String, String>,
}

/// What the drawer draws: the file's text (bounded), or a `data:` URL for an
/// image (bounded), or nothing with a reason.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct PreviewPayload {
    /// `markdown` | `image` | `text` | `none`.
    pub(crate) kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) data_url: Option<String>,
    pub(crate) bytes: u64,
    /// The file was larger than the table allows, and what came back is cut
    /// (text) or absent (image).
    pub(crate) truncated: bool,
    /// What the writing lint counted in a report's or a document's text
    /// (t-32786): worked out once, when the preview is, and kept with it in the
    /// byte-capped cache. Counts only; absent for any other kind and for a
    /// text with nothing to count.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) writing: Option<zerocode_core::plain_text::TextLint>,
}

impl PreviewPayload {
    fn cost(&self) -> u64 {
        self.text.as_ref().map_or(0, |held| held.len() as u64)
            + self.data_url.as_ref().map_or(0, |held| held.len() as u64)
    }
}

/// A document row's text for a tab nobody can write (t-16006): bounded like the
/// drawer's preview, and cut at a whole character when the file is longer than
/// the table allows.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct DocumentText {
    pub(crate) text: String,
    /// The file's size on disk, not the text's — a cut text is shorter.
    pub(crate) bytes: u64,
    pub(crate) truncated: bool,
}

/// The preview LRU: bounded by bytes, not by count — one 4 MB screenshot and
/// a thousand 200-byte texts are not the same memory.
#[derive(Default)]
struct PreviewCache {
    order: VecDeque<String>,
    held: HashMap<String, Arc<PreviewPayload>>,
    bytes: u64,
}

impl PreviewCache {
    fn get(&mut self, id: &str) -> Option<Arc<PreviewPayload>> {
        let found = self.held.get(id)?.clone();
        // Touched: to the back of the line.
        if let Some(at) = self.order.iter().position(|held| held == id) {
            self.order.remove(at);
        }
        self.order.push_back(id.to_string());
        Some(found)
    }

    fn put(&mut self, id: &str, payload: Arc<PreviewPayload>, cap: u64) {
        let cost = payload.cost();
        if cost > cap {
            // Larger than the whole cache: served once, never kept.
            return;
        }
        self.evict(id);
        while self.bytes + cost > cap {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            if let Some(gone) = self.held.remove(&oldest) {
                self.bytes -= gone.cost();
            }
        }
        self.order.push_back(id.to_string());
        self.held.insert(id.to_string(), payload);
        self.bytes += cost;
    }

    fn evict(&mut self, id: &str) {
        if let Some(gone) = self.held.remove(id) {
            self.bytes -= gone.cost();
        }
        if let Some(at) = self.order.iter().position(|held| held == id) {
            self.order.remove(at);
        }
    }

    fn clear(&mut self) {
        self.order.clear();
        self.held.clear();
        self.bytes = 0;
    }
}

struct Index {
    rows: BTreeMap<String, Row>,
    /// Resume bounded page scans after the last visited id, avoiding starvation.
    page_scan_after: Option<String>,
    /// Lines appended since the last compaction. Nothing reads it but the
    /// sweep, which rewrites the file whole when it has removed anything.
    appended: usize,
}

/// The store. One per window, installed at boot ([`install`]) and found by
/// every road through [`store`].
pub(crate) struct Store {
    root: PathBuf,
    local_data_root: PathBuf,
    limits: Mutex<Limits>,
    window: Mutex<Option<tauri::AppHandle>>,
    index: Mutex<Index>,
    previews: Mutex<PreviewCache>,
    sources: Mutex<Vec<ScanSource>>,
    /// Serialize cursor load/read/save across boot, refresh and hook readers.
    pub(crate) transcript_reads: Mutex<()>,
    /// The ledger's `swept_at_ms` this store last swept beside; the artifact
    /// sweep rides the ledger's own hour rather than keeping a second clock.
    swept_beside: Mutex<Option<i64>>,
    /// 페이지마다 마지막으로 센 피드백과 그때 파일의 도장. 목록은 도장이 같으면
    /// 파일을 다시 읽지 않는다.
    feedback: Mutex<HashMap<String, (Stamp, FeedbackSummary)>>,
    /// 「내보내기」와 문이 페이지를 PDF·그림으로 그릴 때 쓰는 렌더러(t-18558). 창이 뜨면
    /// `install`이 창의 숨은 판을 세우고 시험은 가짜를 세운다. 없으면 그려야 하는 형식은
    /// 이유와 함께 거절된다 — HTML 바이트를 .pdf에 쓰지 않는다.
    renderer: Mutex<Option<Arc<dyn PageRenderer>>>,
    /// 이 창이 방금 내보낸 파일들, 오래된 것부터. 토스트의 「Finder에서 보기」는 이 밖의
    /// 경로를 보이지 않는다.
    exported: Mutex<VecDeque<PathBuf>>,
    /// What a worker's hand-in named and the store kept of it, one manifest each
    /// (t-32798, [`kept`]).
    hand_ins: Mutex<kept::HandIns>,
}

fn store_cell() -> &'static Mutex<Option<Arc<Store>>> {
    static CELL: OnceLock<Mutex<Option<Arc<Store>>>> = OnceLock::new();
    CELL.get_or_init(|| Mutex::new(None))
}

/// Put the booted store where every road finds it.
pub(crate) fn install(store: Arc<Store>, app: tauri::AppHandle) {
    store.set_renderer(Arc::new(crate::artifact_render::WindowRenderer::new(
        app.clone(),
    )));
    *store.window.lock().unwrap_or_else(PoisonError::into_inner) = Some(app);
    *store_cell().lock().unwrap_or_else(PoisonError::into_inner) = Some(store);
}

/// The window the booted store answers to, for a caller outside this file
/// that published through the store and must say the catalog moved.
pub(crate) fn window_handle() -> Option<tauri::AppHandle> {
    store()?
        .window
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

/// The window's store, or `None` before boot — every caller reads that as
/// "nothing to register", never as an error.
pub(crate) fn store() -> Option<Arc<Store>> {
    store_cell()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

impl Store {
    /// Open the store under the local data root and read its catalog. A
    /// missing folder is a fresh store; a catalog line that does not parse
    /// is skipped, because one bad line must not cost the rows around it.
    pub(crate) fn open(local_data_root: &Path, limits: Limits) -> Self {
        let root = local_data_root.join(STORE_DIR_NAME);
        let mut rows = BTreeMap::new();
        if let Ok(text) = std::fs::read_to_string(root.join(INDEX_FILE)) {
            for line in text.lines() {
                match serde_json::from_str::<IndexLine>(line) {
                    Ok(IndexLine::Row(artifact)) => {
                        rows.insert(
                            artifact.id.clone(),
                            Row {
                                artifact: *artifact,
                                tokens: Vec::new(),
                                stamp: None,
                            },
                        );
                    }
                    Ok(IndexLine::Tombstone { tombstone }) => {
                        rows.remove(&tombstone);
                    }
                    Err(_) => {}
                }
            }
        }
        // The cap holds at boot too: the newest rows win, the oldest are
        // forgotten rather than held past the table.
        while rows.len() > limits.index_rows_max {
            let Some(oldest) = rows
                .values()
                .min_by_key(|row| row.artifact.modified_ms)
                .map(|row| row.artifact.id.clone())
            else {
                break;
            };
            rows.remove(&oldest);
        }
        // A publication catalogued before rows named their editable source
        // (t-3952) reads it from its own metadata — one bounded read per such
        // row, and only until the next compaction writes it down.
        for row in rows.values_mut() {
            if is_publication(&row.artifact) && row.artifact.source_path.is_none() {
                row.artifact.source_path =
                    zerocode_core::artifact_publish::read_meta(&root, &row.artifact.id)
                        .ok()
                        .map(|meta| meta.source_path);
            }
        }
        // Tokens are rebuilt from disk lazily by the first scan; the rows the
        // catalog named are re-stamped there too.
        let hand_ins = kept::HandIns::load(&root);
        Self {
            root,
            local_data_root: local_data_root.to_path_buf(),
            limits: Mutex::new(limits),
            window: Mutex::new(None),
            index: Mutex::new(Index {
                rows,
                appended: 0,
                page_scan_after: None,
            }),
            previews: Mutex::new(PreviewCache::default()),
            sources: Mutex::new(Vec::new()),
            transcript_reads: Mutex::new(()),
            swept_beside: Mutex::new(None),
            feedback: Mutex::new(HashMap::new()),
            renderer: Mutex::new(None),
            exported: Mutex::new(VecDeque::new()),
            hand_ins: Mutex::new(hand_ins),
        }
    }

    /// Publish into this catalog under its existing lock; no watcher or timer of its own.
    /// The row wears `origin` whole — a republish records its own publisher,
    /// not the first one's.
    pub(crate) fn publish_page(
        &self,
        input: &zerocode_core::artifact_publish::PublishInput,
        origin: Origin,
    ) -> Result<zerocode_core::artifact_publish::PageMeta, String> {
        let limits = self.limits();
        let mut index = self.index.lock().unwrap_or_else(PoisonError::into_inner);
        let _lock = zerocode_core::artifact_publish::lock_store(&self.root)?;
        let meta = zerocode_core::artifact_publish::publish(&self.root, input, &limits)?;
        let artifact = meta.artifact(origin);
        let row = Row {
            tokens: Self::tokens_of_words(
                &[&meta.title, meta.description.as_deref().unwrap_or_default()],
                &limits,
            ),
            stamp: stamp_of(&artifact.path),
            artifact,
        };
        self.insert_row(&mut index, row, &limits)?;
        Ok(meta)
    }

    pub(crate) fn limits(&self) -> Limits {
        *self.limits.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The store's folder — where its sidecars (versions, thumbnails, the
    /// transcript stamps) live beside the catalog.
    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    /// The app's data root the store lives under — where the window's event
    /// log is written.
    pub(crate) fn local_data_root(&self) -> &Path {
        &self.local_data_root
    }

    /// Replace the table — the settings overlay's road in. A smaller preview
    /// cap takes effect by dropping what the cache holds.
    pub(crate) fn set_limits(&self, limits: Limits) {
        *self.limits.lock().unwrap_or_else(PoisonError::into_inner) = limits;
        self.previews
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
    }

    fn append_line(&self, line: &IndexLine) -> Result<(), String> {
        use std::io::Write as _;
        std::fs::create_dir_all(&self.root).map_err(|error| error.to_string())?;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.root.join(INDEX_FILE))
            .map_err(|error| error.to_string())?;
        let mut text = serde_json::to_string(line).map_err(|error| error.to_string())?;
        text.push('\n');
        file.write_all(text.as_bytes())
            .map_err(|error| error.to_string())
    }

    /// Rewrite the catalog from the rows in memory: the compaction that
    /// follows a sweep. Written beside and renamed over, like every durable
    /// file this window keeps.
    fn compact(&self, index: &mut Index) -> Result<(), String> {
        std::fs::create_dir_all(&self.root).map_err(|error| error.to_string())?;
        let mut text = String::new();
        for row in index.rows.values() {
            text.push_str(
                &serde_json::to_string(&IndexLine::Row(Box::new(row.artifact.clone())))
                    .map_err(|error| error.to_string())?,
            );
            text.push('\n');
        }
        let target = self.root.join(INDEX_FILE);
        let tmp = self.root.join(format!("{INDEX_FILE}.tmp"));
        std::fs::write(&tmp, text).map_err(|error| error.to_string())?;
        std::fs::rename(&tmp, &target).map_err(|error| error.to_string())?;
        index.appended = 0;
        Ok(())
    }

    /// Read the front of a file for its preview and its search tokens —
    /// bounded by the table, never the whole of a large file.
    fn read_front(path: &Path, limits: &Limits) -> Vec<u8> {
        use std::io::Read as _;
        let Ok(file) = std::fs::File::open(path) else {
            return Vec::new();
        };
        let mut held = Vec::new();
        let _ = file
            .take(limits.body_index_bytes_max)
            .read_to_end(&mut held);
        held
    }

    fn build_row(seed: RowSeed<'_>, limits: &Limits) -> Row {
        let RowSeed {
            path,
            id,
            source,
            origin,
            stamp,
            now_ms,
            created_ms,
        } = seed;
        let kind = kind_of(path, source);
        let front = Self::read_front(path, limits);
        let preview = preview_of(kind, &front, limits);
        let title =
            artifact::descriptive_title(path, &front, origin.work_summary.as_deref(), limits);
        let mut tokens = zerocode_core::second_brain_related::tokenize(&title)
            .into_iter()
            .collect::<Vec<_>>();
        if matches!(
            kind,
            ArtifactKind::Report
                | ArtifactKind::Evidence
                | ArtifactKind::Transcript
                | ArtifactKind::Other
        ) && let Ok(text) = std::str::from_utf8(&front)
        {
            tokens.extend(body_tokens(text, limits));
        }
        tokens.sort();
        tokens.dedup();
        let modified_ms = epoch_ms(stamp.modified).max(0);
        Row {
            artifact: Artifact {
                id,
                kind,
                title,
                path: path.to_path_buf(),
                bytes: stamp.len,
                created_ms: created_ms.unwrap_or(if modified_ms > 0 {
                    modified_ms
                } else {
                    now_ms
                }),
                modified_ms: if modified_ms > 0 { modified_ms } else { now_ms },
                url: None,
                favicon: None,
                description: None,
                version: None,
                source_path: None,
                feedback_count: None,
                feedback_version: None,
                origin,
                tags: Vec::new(),
                preview,
                source,
            },
            tokens,
            stamp: Some(stamp),
        }
    }

    /// The search tokens of a row that has no file: its title, its
    /// description and its url, sorted and deduplicated like every other row's.
    fn tokens_of_words(words: &[&str], limits: &Limits) -> Vec<String> {
        let mut tokens: Vec<String> = Vec::new();
        for word in words {
            tokens.extend(body_tokens(word, limits));
        }
        tokens.sort();
        tokens.dedup();
        tokens
    }

    fn insert_row(&self, index: &mut Index, row: Row, limits: &Limits) -> Result<(), String> {
        // The cap: the oldest row gives its seat to the new one.
        while index.rows.len() >= limits.index_rows_max
            && !index.rows.contains_key(&row.artifact.id)
        {
            let Some(oldest) = index
                .rows
                .values()
                .min_by_key(|held| held.artifact.modified_ms)
                .map(|held| held.artifact.id.clone())
            else {
                break;
            };
            self.remove_row(index, &oldest)?;
        }
        self.append_line(&IndexLine::Row(Box::new(row.artifact.clone())))?;
        index.appended += 1;
        self.previews
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .evict(&row.artifact.id);
        index.rows.insert(row.artifact.id.clone(), row);
        Ok(())
    }

    /// Copy any file into the store under its source's bucket — the hand-
    /// registration road (`manual`), and, in the store's own tests, a report
    /// copied verbatim (a worker's report is kept by [`Store::register_kept`],
    /// which masks, caps and writes a manifest).
    pub(crate) fn register_copy(
        &self,
        source_path: &Path,
        source: Source,
        origin: Origin,
        now_ms: i64,
    ) -> Result<Artifact, String> {
        let stamp = stamp_of(source_path)
            .ok_or_else(|| format!("report file is not there: {}", source_path.display()))?;
        let limits = self.limits();
        if stamp.len > limits.scan_bytes_max {
            return Err(format!(
                "report is larger than the table allows ({} bytes)",
                stamp.len
            ));
        }
        let id = artifact_id(&origin, source_path);
        let name = source_path
            .file_name()
            .map(|held| held.to_string_lossy().into_owned())
            .unwrap_or_else(|| "report.md".to_string());
        let seat = self.root.join(source.bucket()).join(&id);
        let target = seat.join(&name);
        let mut index = self.index.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(held) = index.rows.get(&id)
            && held.artifact.path == target
            && held.artifact.bytes == stamp.len
            && held.artifact.modified_ms == epoch_ms(stamp.modified).max(0)
            && target.is_file()
        {
            return Ok(held.artifact.clone());
        }
        std::fs::create_dir_all(&seat).map_err(|error| error.to_string())?;
        // Beside and renamed over: a copy that fails halfway leaves no
        // half-report under the artifact's name.
        let tmp = seat.join(format!("{name}.tmp"));
        std::fs::copy(source_path, &tmp).map_err(|error| error.to_string())?;
        std::fs::rename(&tmp, &target).map_err(|error| error.to_string())?;
        // The copy keeps the source's stamp in the row so "unchanged" can be
        // answered against the source next time, not against the copy.
        let created = index.rows.get(&id).map(|held| held.artifact.created_ms);
        let mut row = Self::build_row(
            RowSeed {
                path: &target,
                id,
                source,
                origin,
                stamp,
                now_ms,
                created_ms: created,
            },
            &limits,
        );
        row.stamp = stamp_of(&target);
        let artifact = row.artifact.clone();
        self.insert_row(&mut index, row, &limits)?;
        Ok(artifact)
    }

    /// The in-place road under the caller's lock; answers whether the row is
    /// new, updated or unchanged so the scan can count.
    fn register_in_place_locked(
        &self,
        index: &mut Index,
        path: &Path,
        source: Source,
        origin: Origin,
        now_ms: i64,
        limits: &Limits,
    ) -> Result<(Artifact, Option<bool>), String> {
        let stamp =
            stamp_of(path).ok_or_else(|| format!("file is not there: {}", path.display()))?;
        let id = artifact_id(&origin, path);
        if let Some(held) = index.rows.get_mut(&id) {
            if held.stamp == Some(stamp) && held.artifact.path == path {
                return Ok((held.artifact.clone(), None));
            }
            // Read back from the catalog at boot: the file is what the row
            // says, so re-stamp without re-reading.
            if held.stamp.is_none()
                && held.artifact.path == path
                && held.artifact.bytes == stamp.len
                && held.artifact.modified_ms == epoch_ms(stamp.modified).max(0)
            {
                held.stamp = Some(stamp);
                // Tokens were not persisted; rebuild them from the front of
                // the file once, bounded like everything else.
                if held.tokens.is_empty() {
                    let front = Self::read_front(path, limits);
                    let mut tokens =
                        zerocode_core::second_brain_related::tokenize(&held.artifact.title)
                            .into_iter()
                            .collect::<Vec<_>>();
                    if let Ok(text) = std::str::from_utf8(&front) {
                        tokens.extend(body_tokens(text, limits));
                    }
                    tokens.sort();
                    tokens.dedup();
                    held.tokens = tokens;
                }
                return Ok((held.artifact.clone(), None));
            }
        }
        let existed = index.rows.contains_key(&id);
        let created = index.rows.get(&id).map(|held| held.artifact.created_ms);
        let row = Self::build_row(
            RowSeed {
                path,
                id,
                source,
                origin,
                stamp,
                now_ms,
                created_ms: created,
            },
            limits,
        );
        let artifact = row.artifact.clone();
        // A page or document's every version is kept (t-3233 §5): the first
        // registration is V1, and each stamp the scan sees move is the next.
        if source == Source::AgentPage
            && matches!(artifact.kind, ArtifactKind::Page | ArtifactKind::Document)
        {
            let _ = self.snapshot_version(&artifact, limits);
        }
        self.insert_row(index, row, limits)?;
        Ok((artifact, Some(!existed)))
    }

    /// Copy the file as it is now into `versions/<id>/<n>/<name>`, `n` one
    /// past the newest kept, and drop the oldest past the table's count.
    /// Beside and renamed over, like every durable file this window keeps.
    fn snapshot_version(&self, artifact: &Artifact, limits: &Limits) -> Result<u32, String> {
        let seat = self.root.join(VERSIONS_DIR_NAME).join(&artifact.id);
        std::fs::create_dir_all(&seat).map_err(|error| error.to_string())?;
        let mut held = Self::version_numbers(&seat);
        let next = held.last().map_or(1, |newest| newest + 1);
        let name = artifact
            .path
            .file_name()
            .map(|held| held.to_string_lossy().into_owned())
            .unwrap_or_else(|| "page".to_string());
        let folder = seat.join(next.to_string());
        std::fs::create_dir_all(&folder).map_err(|error| error.to_string())?;
        let tmp = folder.join(format!("{name}.tmp"));
        // The file may grow after stat. Copy at most the observed bytes,
        // within the page cap; a changing file must not become a truncated
        // snapshot with a valid version number.
        let copy = (|| {
            use std::io::Read as _;
            let source = std::fs::File::open(&artifact.path)?;
            let mut target = std::fs::File::create(&tmp)?;
            let cap = artifact.bytes.min(limits.page_bytes_max);
            let copied = std::io::copy(&mut source.take(cap.saturating_add(1)), &mut target)?;
            if copied != artifact.bytes || copied > cap {
                return Err(std::io::Error::other(
                    "page changed while saving its version",
                ));
            }
            std::fs::rename(&tmp, folder.join(&name))
        })();
        if let Err(error) = copy {
            let _ = std::fs::remove_dir_all(&folder);
            return Err(error.to_string());
        }
        held.push(next);
        while held.len() > limits.versions_per_artifact_max.max(1) {
            let oldest = held.remove(0);
            let _ = std::fs::remove_dir_all(seat.join(oldest.to_string()));
        }
        Ok(next)
    }

    /// The version numbers under one seat, ascending.
    fn version_numbers(seat: &Path) -> Vec<u32> {
        let mut numbers: Vec<u32> = std::fs::read_dir(seat)
            .map(|entries| {
                entries
                    .flatten()
                    .filter_map(|entry| entry.file_name().to_string_lossy().parse::<u32>().ok())
                    .collect()
            })
            .unwrap_or_default();
        numbers.sort_unstable();
        numbers
    }

    /// Every immutable version kept for one row, oldest first — what the
    /// page's version picker lists (t-3233 §5). A publication's are the
    /// versions its own store keeps (`pages/<id>/v<n>`, t-3952), with the
    /// digest each was written with; a page or document the transcript road
    /// follows has the scan's snapshots, digested here. Empty for a row that
    /// has none.
    pub(crate) fn versions(&self, id: &str) -> Vec<Version> {
        let published = zerocode_core::artifact_publish::versions(&self.root, id);
        if !published.is_empty() {
            return published
                .into_iter()
                .filter_map(|kept| {
                    let stamp = stamp_of(&kept.path)?;
                    Some(Version {
                        n: kept.n,
                        path: kept.path,
                        bytes: stamp.len,
                        modified_ms: epoch_ms(stamp.modified),
                        sha256: kept.sha256,
                    })
                })
                .collect();
        }
        let cap = self.limits().page_bytes_max;
        self.snapshot_versions(id)
            .into_iter()
            .map(|mut version| {
                version.sha256 = file_sha256(&version.path, cap);
                version
            })
            .collect()
    }

    /// The publication a path names: its current file under `pages/<id>/`,
    /// or one of its kept versions. Judged on resolved paths under this
    /// store's own folder, so a link into the store opens as the artifact it
    /// is and nothing outside it is taken for one; any other file — outside
    /// the store, a page's metadata, a row since deleted — is `None`.
    pub(crate) fn page_at(&self, path: &Path) -> Option<PageAt> {
        let pages = self
            .root
            .join(zerocode_core::artifact_publish::PAGES_DIR)
            .canonicalize()
            .ok()?;
        let asked = path.canonicalize().ok()?;
        let id = asked
            .strip_prefix(&pages)
            .ok()?
            .components()
            .next()?
            .as_os_str()
            .to_str()?
            .to_string();
        let mut artifact = self.get(&id).filter(is_publication)?;
        self.fill_feedback(&mut artifact);
        if artifact
            .path
            .canonicalize()
            .is_ok_and(|current| current == asked)
        {
            return Some(PageAt {
                artifact,
                version: None,
            });
        }
        let version = zerocode_core::artifact_publish::versions(&self.root, &id)
            .into_iter()
            .find(|kept| kept.path.canonicalize().is_ok_and(|kept| kept == asked))?
            .n;
        Some(PageAt {
            artifact,
            version: Some(version),
        })
    }

    /// 전달된 주석 한 묶음을 그 페이지의 기록에 한 줄로 덧붙인다(t-11959). 판의 SHA와
    /// 고칠 원본은 스토어가 제 기록에서 적는다 — 창이 말한 것을 믿지 않는다. 틀린
    /// 청, 발행물이 아닌 행, 아직 없는 판, 상한에 닿은 기록은 한 바이트도 쓰지 않고
    /// 거절한다. 카탈로그의 자물쇠 아래에서 쓰므로 두 전달도, 전달과 삭제도 섞이지
    /// 않는다.
    pub(crate) fn record_feedback(
        &self,
        ask: FeedbackAsk,
        at_ms: i64,
    ) -> Result<FeedbackSummary, String> {
        use std::io::{Read as _, Seek as _, Write as _};
        validate_feedback(&ask)?;
        let index = self.index.lock().unwrap_or_else(PoisonError::into_inner);
        let row = index
            .rows
            .get(&ask.id)
            .map(|row| row.artifact.clone())
            .filter(is_publication)
            .ok_or("피드백은 이 창이 발행한 페이지에만 남깁니다")?;
        if ask.version > row.version.unwrap_or(0) {
            return Err(format!(
                "{}의 버전 {}은 발행된 적이 없습니다",
                ask.id, ask.version
            ));
        }
        // 그 판이 보관에서 밀려났어도 주석은 이미 판에 들어갔다 — 기록은 남기되 SHA는 모른다.
        let sha256 = zerocode_core::artifact_publish::versions(&self.root, &ask.id)
            .into_iter()
            .find(|kept| kept.n == ask.version)
            .and_then(|kept| kept.sha256);
        let line = FeedbackLine {
            id: ask.id,
            version: ask.version,
            sha256,
            source_path: row.source_path,
            recipient: ask.recipient,
            items: ask.items,
            at_ms,
            page_url: ask.page_url,
        };
        let encoded = serde_json::to_vec(&line).map_err(|error| error.to_string())?;
        if encoded.len() > FEEDBACK_LINE_MAX_BYTES {
            return Err(format!(
                "피드백 한 줄은 {FEEDBACK_LINE_MAX_BYTES}바이트를 넘을 수 없습니다"
            ));
        }
        let path = self.feedback_path(&line.id);
        let held = match std::fs::symlink_metadata(&path) {
            Ok(meta) if !meta.is_file() => {
                return Err("피드백 기록이 보통 파일이 아닙니다".into());
            }
            Ok(meta) => meta.len(),
            Err(_) => 0,
        };
        // 끝이 잘린 줄 뒤에 붙이면 두 줄이 한 줄로 읽힌다 — 새 줄은 제 줄에서 시작한다.
        let torn = held > 0
            && std::fs::File::open(&path)
                .and_then(|mut file| {
                    file.seek(std::io::SeekFrom::End(-1))?;
                    let mut last = [0u8; 1];
                    file.read_exact(&mut last)?;
                    Ok(last[0] != b'\n')
                })
                .map_err(|error| error.to_string())?;
        let mut bytes = Vec::with_capacity(encoded.len() + 2);
        if torn {
            bytes.push(b'\n');
        }
        bytes.extend_from_slice(&encoded);
        bytes.push(b'\n');
        if held + bytes.len() as u64 > FEEDBACK_FILE_MAX_BYTES {
            return Err(format!(
                "이 페이지의 피드백 기록이 상한({FEEDBACK_FILE_MAX_BYTES}바이트)에 닿았습니다"
            ));
        }
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .and_then(|mut file| file.write_all(&bytes))
            .map_err(|error| error.to_string())?;
        drop(index);
        Ok(self.feedback_summary(&line.id))
    }

    pub(crate) fn feedback_origin(
        &self,
        ask: &FeedbackAsk,
    ) -> Result<zerocode_core::user_preferences::FeedbackOrigin, String> {
        use std::io::Read as _;
        validate_feedback(ask)?;
        let index = self.index.lock().unwrap_or_else(PoisonError::into_inner);
        if !index
            .rows
            .get(&ask.id)
            .is_some_and(|row| is_publication(&row.artifact))
        {
            return Err("preferences require a recorded publication feedback".into());
        }
        let file = crate::durable_file::open_plain_file(&self.feedback_path(&ask.id))
            .map_err(|_| "recorded feedback could not be read safely")?;
        if file
            .metadata()
            .map_err(|_| "feedback metadata is unavailable")?
            .len()
            > FEEDBACK_FILE_MAX_BYTES
        {
            return Err("recorded feedback exceeds its size limit".into());
        }
        let mut text = String::new();
        file.take(FEEDBACK_FILE_MAX_BYTES + 1)
            .read_to_string(&mut text)
            .map_err(|_| "recorded feedback could not be read")?;
        if u64::try_from(text.len()).unwrap_or(u64::MAX) > FEEDBACK_FILE_MAX_BYTES {
            return Err("recorded feedback exceeds its size limit".into());
        }
        for raw in text.lines().rev() {
            let Ok(line) = serde_json::from_str::<FeedbackLine>(raw) else {
                continue;
            };
            if line.id == ask.id
                && line.version == ask.version
                && line.items == ask.items
                && line.recipient == ask.recipient
                && line.page_url == ask.page_url
            {
                return Ok(zerocode_core::user_preferences::FeedbackOrigin {
                    artifact_id: line.id,
                    version: line.version,
                    sha256: line.sha256,
                    feedback_key: zerocode_core::user_preferences::content_key(raw.as_bytes()),
                    followed_link: line.page_url.is_some(),
                });
            }
        }
        Err("the feedback must be recorded before it becomes a saved preference".into())
    }

    fn feedback_path(&self, id: &str) -> PathBuf {
        self.root
            .join(zerocode_core::artifact_publish::PAGES_DIR)
            .join(id)
            .join(FEEDBACK_FILE)
    }

    /// 한 페이지의 기록을 센다: 읽히고 그 페이지를 말하는 줄의 수와, 그중 가장 새
    /// 줄의 판. 파일의 도장이 지난번과 같으면 다시 읽지 않고, 상한 너머는 읽지 않는다.
    pub(crate) fn feedback_summary(&self, id: &str) -> FeedbackSummary {
        use std::io::Read as _;
        let path = self.feedback_path(id);
        let mut cache = self.feedback.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(stamp) = stamp_of(&path) else {
            cache.remove(id);
            return FeedbackSummary::default();
        };
        if let Some((seen, summary)) = cache.get(id)
            && *seen == stamp
        {
            return *summary;
        }
        let mut bytes = Vec::new();
        let read = std::fs::File::open(&path)
            .and_then(|file| file.take(FEEDBACK_FILE_MAX_BYTES).read_to_end(&mut bytes));
        let mut summary = FeedbackSummary::default();
        if read.is_ok() {
            for line in String::from_utf8_lossy(&bytes).lines() {
                if let Ok(line) = serde_json::from_str::<FeedbackLine>(line)
                    && line.id == id
                {
                    summary.count = summary.count.saturating_add(1);
                    summary.version = Some(line.version);
                }
            }
        }
        cache.insert(id.to_string(), (stamp, summary));
        summary
    }

    /// 발행물 행에 그 기록의 수와 판을 입힌다 — 창에 답하는 사본에만. 다른 행은
    /// 두 필드가 없는 채로 둔다.
    pub(crate) fn fill_feedback(&self, artifact: &mut Artifact) {
        if !is_publication(artifact) {
            return;
        }
        let summary = self.feedback_summary(&artifact.id);
        artifact.feedback_count = Some(summary.count);
        artifact.feedback_version = summary.version;
    }

    /// 페이지를 그릴 렌더러 — 창의 숨은 판, 시험의 가짜, 아니면 없음.
    pub(crate) fn renderer(&self) -> Option<Arc<dyn PageRenderer>> {
        self.renderer
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub(crate) fn set_renderer(&self, renderer: Arc<dyn PageRenderer>) {
        *self.renderer.lock().unwrap_or_else(PoisonError::into_inner) = Some(renderer);
    }

    /// 이 창이 방금 내보낸 파일인가 — 토스트의 「Finder에서 보기」가 묻는다.
    pub(crate) fn was_exported(&self, path: &Path) -> bool {
        self.exported
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .any(|held| held == path)
    }

    fn note_exported(&self, path: &Path) {
        let mut held = self.exported.lock().unwrap_or_else(PoisonError::into_inner);
        held.push_back(path.to_path_buf());
        while held.len() > EXPORTED_KEPT {
            held.pop_front();
        }
    }

    /// 발행물의 한 판을 사람이 고른 폴더에 새 파일(`<제목>-v<n>.<형식>`)로 내보낸다 —
    /// HTML은 그대로 복사하고, PDF와 PNG는 창의 WebKit이 그린다(t-18558). 쓰는 것은
    /// 문의 내보내기(`artifact_publish::export_with`)와 같은 불변 스냅샷이다. 그리는
    /// 일은 이름을 찾기 전에 한 번만 하고, 이름이 있으면 번호를 붙여 새 이름을 찾는다
    /// — 있는 파일은 덮지 않는다. 스토어 안으로는 내보내지 않는다. 그릴 렌더러를
    /// 기다리므로 창의 스레드나 비동기 일꾼이 아니라 막아도 되는 스레드에서 부른다.
    pub(crate) fn export_into(
        &self,
        id: &str,
        version: u32,
        folder: &Path,
        format: ExportFormat,
    ) -> Result<zerocode_core::artifact_publish::ExportedFile, String> {
        let row = self
            .get(id)
            .filter(is_publication)
            .ok_or("내보낼 수 있는 것은 이 창이 발행한 페이지뿐입니다")?;
        if !folder.is_absolute() {
            return Err("내보낼 폴더는 절대 경로여야 합니다".into());
        }
        let folder = folder.canonicalize().map_err(|error| error.to_string())?;
        if !folder.is_dir() {
            return Err("내보낼 곳이 폴더가 아닙니다".into());
        }
        if self
            .root
            .canonicalize()
            .is_ok_and(|store| folder.starts_with(store))
        {
            return Err("아티팩트 스토어 안으로는 내보내지 않습니다".into());
        }
        let renderer = self.renderer();
        let prepared = zerocode_core::artifact_publish::prepare_export(
            &self.root,
            id,
            Some(version),
            format,
            renderer.as_deref(),
        )?;
        let stem = export_stem(&row.title, id);
        let extension = format.word();
        for n in 1..=EXPORT_NAME_TRIES {
            let name = if n == 1 {
                format!("{stem}-v{version}.{extension}")
            } else {
                format!("{stem}-v{version} ({n}).{extension}")
            };
            let out = folder.join(name);
            if out.symlink_metadata().is_ok() {
                continue;
            }
            match zerocode_core::artifact_publish::write_export(&out, &prepared) {
                Ok(done) => {
                    self.note_exported(&done.path);
                    return Ok(done);
                }
                // 물은 사이에 누가 그 이름을 썼다 — 다음 번호로.
                Err(_) if out.symlink_metadata().is_ok() => {}
                Err(error) => return Err(error),
            }
        }
        Err(format!(
            "{stem}-v{version}.{extension} 이름의 파일이 이미 {EXPORT_NAME_TRIES}개 있습니다"
        ))
    }

    /// The scan's snapshots of one page or document, oldest first, without
    /// digests — what retention walks.
    fn snapshot_versions(&self, id: &str) -> Vec<Version> {
        let seat = self.root.join(VERSIONS_DIR_NAME).join(id);
        Self::version_numbers(&seat)
            .into_iter()
            .filter_map(|n| {
                let folder = seat.join(n.to_string());
                let file = std::fs::read_dir(&folder)
                    .ok()?
                    .flatten()
                    .find_map(|entry| {
                        let path = entry.path();
                        (path.is_file() && !path.to_string_lossy().ends_with(".tmp"))
                            .then_some(path)
                    })?;
                let stamp = stamp_of(&file)?;
                Some(Version {
                    n,
                    path: file,
                    bytes: stamp.len,
                    modified_ms: epoch_ms(stamp.modified),
                    sha256: None,
                })
            })
            .collect()
    }

    /// A claude.ai artifact a transcript saw published (t-3233 §2a): one row
    /// per url, refreshed with the newer title and time when the fact is
    /// newer than the row, left alone when it is older. Idempotent.
    pub(crate) fn register_remote(
        &self,
        fact: &RemoteFact,
        agent: &str,
        now_ms: i64,
    ) -> Result<Artifact, String> {
        let url = fact.url.trim();
        if !url.starts_with("https://") {
            return Err(format!("not an artifact url: {url}"));
        }
        let limits = self.limits();
        let id = artifact_id_for_url(url);
        let at_ms = fact.at_ms.unwrap_or(now_ms);
        let mut index = self.index.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(held) = index.rows.get(&id)
            && held.artifact.modified_ms >= at_ms
        {
            return Ok(held.artifact.clone());
        }
        let title = fact
            .title
            .clone()
            .filter(|held| !held.trim().is_empty())
            .unwrap_or_else(|| {
                url.rsplit('/')
                    .next()
                    .filter(|tail| !tail.is_empty())
                    .unwrap_or(url)
                    .to_string()
            });
        let title = artifact::truncate_chars(&title, limits.title_chars);
        let description = fact.description.clone().unwrap_or_default();
        let created_ms = index
            .rows
            .get(&id)
            .map_or(at_ms, |held| held.artifact.created_ms.min(at_ms));
        let tokens = Self::tokens_of_words(&[&title, &description, url], &limits);
        let row = Row {
            artifact: Artifact {
                id: id.clone(),
                kind: ArtifactKind::Web,
                title,
                path: PathBuf::new(),
                bytes: 0,
                created_ms,
                modified_ms: at_ms,
                url: Some(url.to_string()),
                favicon: fact.favicon.clone(),
                description: None,
                version: None,
                source_path: None,
                feedback_count: None,
                feedback_version: None,
                origin: Origin {
                    agent: Some(agent.to_string()),
                    session: fact.session.clone(),
                    project: fact.project.clone(),
                    ..Origin::default()
                },
                tags: Vec::new(),
                preview: if description.is_empty() {
                    Preview::None
                } else {
                    Preview::Text {
                        text: artifact::truncate_chars(&description, limits.preview_chars),
                    }
                },
                source: Source::Remote,
            },
            tokens,
            stamp: None,
        };
        let artifact = row.artifact.clone();
        self.insert_row(&mut index, row, &limits)?;
        Ok(artifact)
    }

    /// A page an agent wrote into its project (t-3233 §2b): registered where
    /// it is, never copied. `Ok(None)` when the file is not there, is larger
    /// than the table allows, or is not a page at all — none of which is an
    /// error, a transcript names files that have since moved on. Bounded per
    /// session: past `pages_per_session` the session's oldest row gives way.
    pub(crate) fn register_page(
        &self,
        fact: &PageFact,
        agent: &str,
        now_ms: i64,
    ) -> Result<Option<Artifact>, String> {
        if !artifact::is_page_extension(&fact.path) || !fact.path.is_absolute() {
            return Ok(None);
        }
        // Writers follow symlinks before ..; normalize on disk before the
        // final containment check, then key aliases as one physical page.
        let Some((path, project)) = fact
            .project
            .as_deref()
            .and_then(|root| Some((fact.path.canonicalize().ok()?, root.canonicalize().ok()?)))
        else {
            return Ok(None);
        };
        if !path.starts_with(&project) {
            return Ok(None);
        }
        let Some(stamp) = stamp_of(&path) else {
            return Ok(None);
        };
        let limits = self.limits();
        if stamp.len > limits.page_bytes_max {
            return Ok(None);
        }
        let origin = Origin {
            agent: Some(agent.to_string()),
            session: fact.session.clone(),
            project: Some(project),
            ..Origin::default()
        };
        let id = artifact_id(&Origin::default(), &path);
        let mut index = self.index.lock().unwrap_or_else(PoisonError::into_inner);
        if !index.rows.contains_key(&id)
            && let Some(session) = fact.session.as_deref()
        {
            let mut same: Vec<(i64, String)> = index
                .rows
                .values()
                .filter(|row| {
                    row.artifact.source == Source::AgentPage
                        && row.artifact.origin.session.as_deref() == Some(session)
                })
                .map(|row| (row.artifact.modified_ms, row.artifact.id.clone()))
                .collect();
            same.sort();
            while same.len() >= limits.pages_per_session.max(1) {
                let (_, oldest) = same.remove(0);
                self.remove_row(&mut index, &oldest)?;
            }
        }
        let (artifact, _) = self.register_in_place_locked(
            &mut index,
            &path,
            Source::AgentPage,
            origin,
            now_ms,
            &limits,
        )?;
        Ok(Some(artifact))
    }

    /// Every row the transcript road registered (t-3233 §2b) — what a change
    /// of that road's rule must judge again (t-3952).
    pub(crate) fn agent_pages(&self) -> Vec<Artifact> {
        self.index
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .rows
            .values()
            .filter(|row| row.artifact.source == Source::AgentPage)
            .map(|row| row.artifact.clone())
            .collect()
    }

    /// Drop transcript-road rows from the catalog with their sidecars (their
    /// snapshots and thumbnails). The files they point at are the project's
    /// and are never touched; any other row is refused. Answers how many went.
    pub(crate) fn forget_agent_pages(&self, ids: &[String]) -> usize {
        let mut index = self.index.lock().unwrap_or_else(PoisonError::into_inner);
        let mut gone = 0;
        for id in ids {
            let agent_page = index
                .rows
                .get(id)
                .is_some_and(|row| row.artifact.source == Source::AgentPage);
            if agent_page && matches!(self.remove_row(&mut index, id), Ok(Some(_))) {
                gone += 1;
            }
        }
        gone
    }

    /// Register a folder the scan walks. The newest registrations win the
    /// table's seats; a folder registered twice keeps one seat.
    pub(crate) fn adopt_source(&self, source: ScanSource) {
        let limits = self.limits();
        let mut sources = self.sources.lock().unwrap_or_else(PoisonError::into_inner);
        sources.retain(|held| held.dir != source.dir);
        sources.push(source);
        while sources.len() > limits.scan_sources_max {
            sources.remove(0);
        }
    }

    /// The folders the file watcher's artifact lane follows: the store's own
    /// root and the newest registered sources, bounded by the table.
    pub(crate) fn watch_targets(&self) -> Vec<(String, Option<PathBuf>)> {
        let limits = self.limits();
        let sources = self.sources.lock().unwrap_or_else(PoisonError::into_inner);
        // The root and its three buckets: a copied report lands as a new
        // folder under a bucket, and that is the folder whose stamp moves.
        let mut targets = vec![(format!("{WATCH_LANE}:root"), Some(self.root.clone()))];
        for bucket in [Source::WorkerReport, Source::Evidence, Source::Manual] {
            targets.push((
                format!("{WATCH_LANE}:{}", bucket.bucket()),
                Some(self.root.join(bucket.bucket())),
            ));
        }
        for source in sources
            .iter()
            .rev()
            .take(limits.watch_dirs_max.saturating_sub(targets.len()))
        {
            targets.push((
                format!("{WATCH_LANE}:{}", source.dir.display()),
                Some(source.dir.clone()),
            ));
        }
        // The pages registered one by one (t-3233 §2b): the newest keep the
        // seats the table has left, so a page an agent is rewriting now is
        // heard and re-versioned while an old one waits for the next scan.
        let index = self.index.lock().unwrap_or_else(PoisonError::into_inner);
        let mut pages: Vec<&Row> = index
            .rows
            .values()
            .filter(|row| row.artifact.source == Source::AgentPage)
            .collect();
        pages.sort_by_key(|row| std::cmp::Reverse(row.artifact.modified_ms));
        for row in pages
            .into_iter()
            .take(limits.watch_dirs_max.saturating_sub(targets.len()))
        {
            targets.push((
                format!("{WATCH_LANE}:{}", row.artifact.path.display()),
                Some(row.artifact.path.clone()),
            ));
        }
        targets
    }

    /// One bounded, incremental pass over every registered folder and the
    /// store's own files.
    ///
    /// Bounded: depth, files and bytes from the table, and `truncated` says
    /// which pass hit one. Incremental: a file whose stamp matches its row is
    /// skipped without a read. A row whose file is gone is removed — for the
    /// in-place sources only; a copied report is the store's own file and its
    /// absence is a bug to notice, not a row to drop silently.
    pub(crate) fn scan(&self, now_ms: i64) -> ScanReport {
        let limits = self.limits();
        let sources = self
            .sources
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let mut report = ScanReport::default();
        let mut budget = ScanBudget {
            files: limits.scan_files_max,
            bytes: limits.scan_bytes_max,
        };
        let mut index = self.index.lock().unwrap_or_else(PoisonError::into_inner);
        let mut seen_paths: Vec<PathBuf> = Vec::new();
        // Upgrade filename-only catalogs on their first bounded scan. Explicit
        // published titles stay authoritative; no extra watcher or UI reads.
        let legacy: Vec<String> = index
            .rows
            .iter()
            .filter(|(_, row)| {
                row.stamp.is_none()
                    && row.artifact.url.is_none()
                    && row.artifact.title == title_of(&row.artifact.path, &limits)
            })
            .map(|(id, _)| id.clone())
            .collect();
        for id in legacy {
            let Some(row) = index.rows.get(&id) else {
                continue;
            };
            if !budget.visit_file()
                || !budget.read_bytes(row.artifact.bytes.min(limits.body_index_bytes_max))
            {
                report.truncated = true;
                break;
            }
            let front = Self::read_front(&row.artifact.path, &limits);
            let title = artifact::descriptive_title(
                &row.artifact.path,
                &front,
                row.artifact.origin.work_summary.as_deref(),
                &limits,
            );
            if title != row.artifact.title {
                let mut row = row.clone();
                row.artifact.title = title;
                row.tokens = Self::tokens_of_words(
                    &[
                        &row.artifact.title,
                        std::str::from_utf8(&front).unwrap_or_default(),
                    ],
                    &limits,
                );
                if self.insert_row(&mut index, row, &limits).is_ok() {
                    report.updated += 1;
                }
            }
        }
        for source in &sources {
            let (mut found, cut) = Self::walk(&source.dir, limits.scan_depth, &mut budget);
            report.truncated |= cut;
            found.sort();
            for path in found {
                report.seen += 1;
                match self.register_in_place_locked(
                    &mut index,
                    &path,
                    source.source,
                    source.origin.clone(),
                    now_ms,
                    &limits,
                ) {
                    Ok((_, Some(true))) => report.added += 1,
                    Ok((_, Some(false))) => report.updated += 1,
                    Ok((_, None)) | Err(_) => {}
                }
                seen_paths.push(path);
            }
        }
        // The store's own copies: re-stamped, never dropped.
        let own: Vec<(String, PathBuf)> = index
            .rows
            .values()
            .filter(|row| row.artifact.path.starts_with(&self.root))
            .map(|row| (row.artifact.id.clone(), row.artifact.path.clone()))
            .collect();
        for (id, path) in own {
            if let Some(row) = index.rows.get_mut(&id)
                && row.stamp.is_none()
            {
                row.stamp = stamp_of(&path);
                if row.tokens.is_empty() {
                    let front = Self::read_front(&path, &limits);
                    let mut tokens =
                        zerocode_core::second_brain_related::tokenize(&row.artifact.title)
                            .into_iter()
                            .collect::<Vec<_>>();
                    if let Ok(text) = std::str::from_utf8(&front) {
                        tokens.extend(body_tokens(text, &limits));
                    }
                    tokens.sort();
                    tokens.dedup();
                    row.tokens = tokens;
                }
            }
        }
        // Pages registered one by one (t-3233 §2b) have no source folder:
        // each is asked its stamp — one `stat` — and the row goes when the
        // file is gone, moves (and keeps a version) when the file did.
        let mut pages: Vec<(String, PathBuf, Option<Stamp>, Origin)> = index
            .rows
            .values()
            .filter(|row| row.artifact.source == Source::AgentPage)
            .map(|row| {
                (
                    row.artifact.id.clone(),
                    row.artifact.path.clone(),
                    row.stamp,
                    row.artifact.origin.clone(),
                )
            })
            .collect();
        if let Some(after) = &index.page_scan_after {
            let start = pages.partition_point(|(id, _, _, _)| id <= after);
            pages.rotate_left(start);
        }
        for (id, path, stamp, origin) in pages {
            if !budget.visit_file() {
                report.truncated = true;
                break;
            }
            index.page_scan_after = Some(id.clone());
            match stamp_of(&path) {
                None => {
                    if self.remove_row(&mut index, &id).is_ok() {
                        report.removed += 1;
                    }
                }
                Some(now) if stamp != Some(now) => {
                    report.seen += 1;
                    if now.len > limits.page_bytes_max || !budget.read_bytes(now.len) {
                        report.truncated = true;
                        continue;
                    }
                    if let Ok((_, Some(_))) = self.register_in_place_locked(
                        &mut index,
                        &path,
                        Source::AgentPage,
                        origin,
                        now_ms,
                        &limits,
                    ) {
                        report.updated += 1;
                    }
                }
                Some(_) => {}
            }
        }
        // Rows of in-place files that are no longer there — only under
        // folders this pass actually finished walking.
        if !report.truncated {
            let gone: Vec<String> = index
                .rows
                .values()
                .filter(|row| {
                    row.artifact.source != Source::WorkerReport
                        && !row.artifact.path.starts_with(&self.root)
                        && sources
                            .iter()
                            .any(|source| row.artifact.path.starts_with(&source.dir))
                        && !row.artifact.path.is_file()
                })
                .map(|row| row.artifact.id.clone())
                .collect();
            for id in gone {
                if self.remove_row(&mut index, &id).is_ok() {
                    report.removed += 1;
                }
            }
        }
        report
    }

    /// Walk one folder to the table's depth, spending the shared budget.
    fn walk(dir: &Path, depth: usize, budget: &mut ScanBudget) -> (Vec<PathBuf>, bool) {
        let mut found = Vec::new();
        let mut cut = false;
        let mut pending: Vec<(PathBuf, usize)> = vec![(dir.to_path_buf(), 0)];
        while let Some((folder, level)) = pending.pop() {
            let Ok(entries) = std::fs::read_dir(&folder) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let Ok(kind) = entry.file_type() else {
                    continue;
                };
                if kind.is_dir() {
                    if level + 1 < depth {
                        pending.push((path, level + 1));
                    } else {
                        cut = true;
                    }
                    continue;
                }
                if !kind.is_file() {
                    continue;
                }
                // The catalog's own files are not artifacts.
                if path.file_name().is_some_and(|name| {
                    name == INDEX_FILE || name.to_string_lossy().ends_with(".tmp")
                }) {
                    continue;
                }
                if !budget.visit_file() {
                    cut = true;
                    return (found, cut);
                }
                let len = entry.metadata().map(|meta| meta.len()).unwrap_or(0);
                if !budget.read_bytes(len) {
                    cut = true;
                    return (found, cut);
                }
                found.push(path);
            }
        }
        (found, cut)
    }

    fn remove_row(&self, index: &mut Index, id: &str) -> Result<Option<Artifact>, String> {
        let Some(row) = index.rows.remove(id) else {
            return Ok(None);
        };
        self.append_line(&IndexLine::Tombstone {
            tombstone: id.to_string(),
        })?;
        index.appended += 1;
        self.previews
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .evict(id);
        self.forget_sidecars(id);
        Ok(Some(row.artifact))
    }

    /// Whether a file is the store's to delete: under the app's own data
    /// root. A person's export folder is theirs; a row can go, the file stays.
    fn owns(&self, path: &Path) -> bool {
        path.starts_with(&self.local_data_root)
    }

    /// Remove one artifact — only when the person confirmed. Without the
    /// confirmation this is a no-op that answers `false`, so a caller that
    /// forgot to ask cannot delete by accident. The copied file (or the
    /// in-place file, when it is under the app's data root) goes with the row.
    pub(crate) fn delete(&self, id: &str, confirmed: bool) -> Result<bool, String> {
        if !confirmed {
            return Ok(false);
        }
        let mut index = self.index.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(artifact) = self.remove_row(&mut index, id)? else {
            return Ok(false);
        };
        if self.owns(&artifact.path) {
            let seat = self.root.join(artifact.source.bucket()).join(&artifact.id);
            if artifact.path.starts_with(&seat) {
                let _ = std::fs::remove_dir_all(&seat);
            } else {
                let _ = std::fs::remove_file(&artifact.path);
            }
        }
        self.forget_sidecars(&artifact.id);
        Ok(true)
    }

    /// The store's own files beside a row — its versions and its rendered
    /// thumbnail — go with the row, whoever owned the file itself.
    fn forget_sidecars(&self, id: &str) {
        self.feedback
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(id);
        let _ = std::fs::remove_dir_all(
            self.root
                .join(zerocode_core::artifact_publish::PAGES_DIR)
                .join(id),
        );
        let _ = std::fs::remove_dir_all(self.root.join(VERSIONS_DIR_NAME).join(id));
        let _ = std::fs::remove_file(self.root.join(THUMBS_DIR_NAME).join(format!("{id}.png")));
        let _ = std::fs::remove_file(self.root.join(THUMBS_DIR_NAME).join(format!("{id}.key")));
    }

    /// Drop every row whose file lived under one of the given folders — the
    /// automation runtime's road when it sweeps evidence folders. Files are
    /// the caller's; only the catalog moves here.
    pub(crate) fn forget_under(&self, dirs: &[PathBuf]) -> usize {
        let mut index = self.index.lock().unwrap_or_else(PoisonError::into_inner);
        let gone: Vec<String> = index
            .rows
            .values()
            .filter(|row| dirs.iter().any(|dir| row.artifact.path.starts_with(dir)))
            .map(|row| row.artifact.id.clone())
            .collect();
        let mut count = 0;
        for id in gone {
            if self.remove_row(&mut index, &id).is_ok() {
                count += 1;
            }
        }
        let mut sources = self.sources.lock().unwrap_or_else(PoisonError::into_inner);
        sources.retain(|source| !dirs.iter().any(|dir| source.dir.starts_with(dir)));
        count
    }

    /// The retention sweep: rows older than `days` go, and the store's own
    /// files with them; the catalog is compacted afterwards so the tombstones
    /// do not outlive what they buried.
    pub(crate) fn prune(&self, now_ms: i64, days: u32) -> Pruned {
        let horizon = now_ms - i64::from(days.max(1)) * 24 * 60 * 60 * 1000;
        let mut index = self.index.lock().unwrap_or_else(PoisonError::into_inner);
        let old: Vec<Artifact> = index
            .rows
            .values()
            .filter(|row| row.artifact.modified_ms < horizon)
            .map(|row| row.artifact.clone())
            .collect();
        let mut pruned = Pruned::default();
        // Active pages also shed old versions on the ledger's retention
        // beat. Keep the newest snapshot so version numbers never restart.
        // A publication's own versions are its store's (bounded by
        // `versions_per_artifact_max` at publish), never retention's.
        for row in index.rows.values() {
            let mut versions = self.snapshot_versions(&row.artifact.id);
            versions.pop();
            for version in versions {
                if version.modified_ms < horizon
                    && let Some(folder) = version.path.parent()
                    && std::fs::remove_dir_all(folder).is_ok()
                {
                    pruned.files += 1;
                }
            }
        }
        for artifact in old {
            index.rows.remove(&artifact.id);
            pruned.rows += 1;
            if self.owns(&artifact.path) {
                let seat = self.root.join(artifact.source.bucket()).join(&artifact.id);
                let removed = if artifact.path.starts_with(&seat) {
                    std::fs::remove_dir_all(&seat).is_ok()
                } else {
                    std::fs::remove_file(&artifact.path).is_ok()
                };
                if removed {
                    pruned.files += 1;
                }
            }
            self.forget_sidecars(&artifact.id);
            self.previews
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .evict(&artifact.id);
        }
        // The manifests of what a worker handed in go on the same age as the rows
        // they point at (t-32798).
        self.prune_hand_ins(horizon);
        if pruned.rows > 0 || index.appended > 0 {
            let _ = self.compact(&mut index);
        }
        pruned
    }

    /// Sweep beside the ledger: once per ledger sweep, with the days the
    /// table (or its settings overlay) says. `None` when the ledger has not
    /// swept since the last look — the ordinary answer on every beat but one.
    pub(crate) fn sweep_beside_ledger(
        &self,
        ledger_swept_at_ms: Option<i64>,
        ledger_days: u32,
        now_ms: i64,
    ) -> Option<Pruned> {
        let mut beside = self
            .swept_beside
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if *beside == ledger_swept_at_ms {
            return None;
        }
        *beside = ledger_swept_at_ms;
        drop(beside);
        let days = self.limits().effective_retention_days(ledger_days);
        let pruned = self.prune(now_ms, days);
        if pruned.rows > 0 {
            crate::system_runtime::note_window_event(
                &self.local_data_root,
                &format!(
                    "artifacts: retention swept {} rows and {} files older than {days} days",
                    pruned.rows, pruned.files
                ),
            );
        }
        Some(pruned)
    }

    /// List rows the filter admits, newest first, bounded by the table.
    ///
    /// 파일 존재는 저렴한 거르개를 지난 행마다 목록을 만들 때 한 번만 묻는다.
    /// 지워진 Computer Use 세션 폴더처럼 감시가 끝난 경로도 있으므로
    /// 스캔만으로는 알 수 없고, 창은 그릴 때마다 디스크에 묻지 않는다.
    pub(crate) fn list(&self, filter: &Filter) -> Listing {
        let limits = self.limits();
        let kind = filter.kind.as_deref().and_then(ArtifactKind::parse);
        let kinds: Vec<ArtifactKind> = filter
            .kinds
            .iter()
            .filter_map(|word| ArtifactKind::parse(word))
            .collect();
        let of_kind = |artifact: &Artifact| {
            kind.is_none_or(|wanted| artifact.kind == wanted)
                && (kinds.is_empty() || kinds.contains(&artifact.kind))
        };
        let present = |gone: bool| filter.present.is_none_or(|wanted| wanted != gone);
        let index = self.index.lock().unwrap_or_else(PoisonError::into_inner);
        let held: Vec<(&Row, bool)> = index
            .rows
            .values()
            .filter(|row| {
                filter
                    .remote
                    .is_none_or(|remote| (row.artifact.kind == ArtifactKind::Web) == remote)
            })
            .filter(|row| {
                filter
                    .published
                    .is_none_or(|published| is_publication(&row.artifact) == published)
            })
            .filter(|row| {
                let origin = &row.artifact.origin;
                let same = |asked: &Option<String>, held: Option<&str>| {
                    asked.as_deref().is_none_or(|wanted| held == Some(wanted))
                };
                same(&filter.run, origin.run.as_deref())
                    && same(&filter.task, origin.task.as_deref())
                    && same(&filter.worker, origin.worker.as_deref())
                    && same(&filter.automation, origin.automation.as_deref())
                    && same(&filter.agent, origin.agent.as_deref())
                    && filter.worktree.as_deref().is_none_or(|wanted| {
                        origin
                            .worktree
                            .as_deref()
                            .is_some_and(|held| held == Path::new(wanted))
                    })
            })
            .filter(|row| {
                let origin = &row.artifact.origin;
                filter.roots.is_empty()
                    || [origin.project.as_deref(), origin.worktree.as_deref()]
                        .into_iter()
                        .flatten()
                        .any(|path| filter.roots.iter().any(|root| path.starts_with(root)))
            })
            .filter(|row| {
                filter
                    .since_ms
                    .is_none_or(|since| row.artifact.modified_ms >= since)
            })
            .filter(|row| {
                filter.query.trim().is_empty() || query_matches(&filter.query, &row.tokens)
            })
            .map(|row| (row, is_gone(&row.artifact)))
            .collect();
        let mut by_kind: BTreeMap<String, usize> = BTreeMap::new();
        let mut missing_total = 0;
        for (row, gone) in &held {
            if present(*gone) {
                *by_kind
                    .entry(row.artifact.kind.as_str().to_string())
                    .or_insert(0) += 1;
            }
            if *gone && of_kind(&row.artifact) {
                missing_total += 1;
            }
        }
        let mut rows: Vec<(&Row, bool)> = held
            .into_iter()
            .filter(|(row, gone)| of_kind(&row.artifact) && present(*gone))
            .collect();
        rows.sort_by(|(a, _), (b, _)| {
            b.artifact
                .modified_ms
                .cmp(&a.artifact.modified_ms)
                .then_with(|| a.artifact.id.cmp(&b.artifact.id))
        });
        let total = rows.len();
        let truncated = total > limits.list_rows_max;
        rows.truncate(limits.list_rows_max);
        let mut listing = Listing {
            missing: rows
                .iter()
                .filter(|(_, gone)| *gone)
                .map(|(row, _)| row.artifact.id.clone())
                .collect(),
            rows: rows
                .into_iter()
                .map(|(row, _)| row.artifact.clone())
                .collect(),
            total,
            truncated,
            missing_total,
            by_kind,
        };
        // 기록을 세는 읽기는 카탈로그의 자물쇠 밖에서 — 행은 이미 사본이다.
        drop(index);
        for row in &mut listing.rows {
            self.fill_feedback(row);
        }
        listing
    }

    /// Counts by origin, for the chips.
    pub(crate) fn counts(&self) -> Counts {
        let index = self.index.lock().unwrap_or_else(PoisonError::into_inner);
        let mut counts = Counts::default();
        for row in index.rows.values() {
            counts.total += 1;
            let origin = &row.artifact.origin;
            let bump = |map: &mut BTreeMap<String, usize>, key: Option<String>| {
                if let Some(key) = key {
                    *map.entry(key).or_insert(0) += 1;
                }
            };
            bump(&mut counts.by_worker, origin.worker.clone());
            bump(&mut counts.by_task, origin.task.clone());
            bump(&mut counts.by_run, origin.run.clone());
            bump(
                &mut counts.by_worktree,
                origin
                    .worktree
                    .as_ref()
                    .map(|held| held.display().to_string()),
            );
            bump(&mut counts.by_automation, origin.automation.clone());
            if row.artifact.kind == ArtifactKind::Report
                && let Some(worker) = origin.worker.clone()
            {
                let newer = counts
                    .report_by_worker
                    .get(&worker)
                    .and_then(|held| index.rows.get(held))
                    .is_none_or(|held| held.artifact.modified_ms < row.artifact.modified_ms);
                if newer {
                    counts
                        .report_by_worker
                        .insert(worker, row.artifact.id.clone());
                }
            }
        }
        counts
    }

    /// One row.
    pub(crate) fn get(&self, id: &str) -> Option<Artifact> {
        self.index
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .rows
            .get(id)
            .map(|row| row.artifact.clone())
    }

    /// The artifacts one worker left, newest first — what `worker-show` is
    /// garnished with.
    pub(crate) fn ids_for_worker(&self, worker: &str) -> Vec<(String, ArtifactKind)> {
        let index = self.index.lock().unwrap_or_else(PoisonError::into_inner);
        let mut rows: Vec<&Row> = index
            .rows
            .values()
            .filter(|row| row.artifact.origin.worker.as_deref() == Some(worker))
            .collect();
        rows.sort_by_key(|row| std::cmp::Reverse(row.artifact.modified_ms));
        rows.iter()
            .map(|row| (row.artifact.id.clone(), row.artifact.kind))
            .collect()
    }

    /// What the drawer draws for one row, through the byte-capped LRU.
    /// Images are read and encoded only here — on request — and never for a
    /// card that is merely listed.
    pub(crate) fn preview(&self, id: &str) -> Result<Arc<PreviewPayload>, String> {
        if let Some(held) = self
            .previews
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(id)
        {
            return Ok(held);
        }
        let artifact = self
            .get(id)
            .ok_or_else(|| format!("unknown artifact: {id}"))?;
        let limits = self.limits();
        // A claude.ai artifact has no file: its description is the drawer's
        // text, and the browser door draws the rest (t-3233 §3).
        if artifact.kind == ArtifactKind::Web {
            let text = match &artifact.preview {
                Preview::Text { text } => Some(text.clone()),
                _ => None,
            };
            let payload = Arc::new(PreviewPayload {
                kind: if text.is_some() { "text" } else { "none" },
                text,
                data_url: None,
                bytes: 0,
                truncated: false,
                writing: None,
            });
            return Ok(payload);
        }
        let meta = std::fs::metadata(&artifact.path).map_err(|error| error.to_string())?;
        let bytes = meta.len();
        // 페이지 그림은 현재 행 키로 숨은 판이 만든 썸네일이다. 아직 없다는
        // 답은 기억하지 않는다 — 카드가 뒤에 그린 뒤 다시 물으면 찾아야 한다.
        if artifact.kind == ArtifactKind::Page {
            use crate::artifact_thumbs::{Cached, cached, data_url_of, key_of};
            let png = key_of(&artifact).and_then(|key| match cached(&self.root, id, &key) {
                Cached::Picture(png) => Some(png),
                Cached::Failed | Cached::Nothing => None,
            });
            return Ok(Arc::new(PreviewPayload {
                kind: if png.is_some() { "image" } else { "none" },
                text: None,
                data_url: png.as_deref().map(data_url_of),
                bytes,
                truncated: false,
                writing: None,
            }));
        }
        let payload = match artifact.kind {
            ArtifactKind::Screenshot => {
                if bytes > limits.preview_image_bytes_max {
                    PreviewPayload {
                        kind: "image",
                        text: None,
                        data_url: None,
                        bytes,
                        truncated: true,
                        writing: None,
                    }
                } else {
                    use base64::Engine as _;
                    let data = std::fs::read(&artifact.path).map_err(|error| error.to_string())?;
                    let mime = image_mime(&artifact.path);
                    PreviewPayload {
                        kind: "image",
                        text: None,
                        data_url: Some(format!(
                            "data:{mime};base64,{}",
                            base64::engine::general_purpose::STANDARD.encode(data)
                        )),
                        bytes,
                        truncated: false,
                        writing: None,
                    }
                }
            }
            ArtifactKind::Export | ArtifactKind::Page | ArtifactKind::Web => PreviewPayload {
                kind: "none",
                text: None,
                data_url: None,
                bytes,
                truncated: false,
                writing: None,
            },
            // A PDF document is not text; its bytes fall to `none` below.
            ArtifactKind::Document if !is_utf8_document(&artifact.path) => PreviewPayload {
                kind: "none",
                text: None,
                data_url: None,
                bytes,
                truncated: false,
                writing: None,
            },
            ArtifactKind::Report
            | ArtifactKind::Document
            | ArtifactKind::Evidence
            | ArtifactKind::Transcript
            | ArtifactKind::Other => {
                use std::io::Read as _;
                let file =
                    std::fs::File::open(&artifact.path).map_err(|error| error.to_string())?;
                let mut held = Vec::new();
                file.take(limits.preview_text_bytes_max)
                    .read_to_end(&mut held)
                    .map_err(|error| error.to_string())?;
                let text = String::from_utf8_lossy(&held).into_owned();
                let prose = matches!(artifact.kind, ArtifactKind::Report | ArtifactKind::Document);
                // A report's words are linted here, once, and the counts ride the
                // cached preview (t-32786). What is not prose, and a text with
                // nothing to count, carry none.
                let writing = prose
                    .then(|| zerocode_core::plain_text::lint(&text))
                    .filter(|found| found.sentences > 0);
                PreviewPayload {
                    kind: if prose { "markdown" } else { "text" },
                    text: Some(text),
                    data_url: None,
                    bytes,
                    truncated: bytes > limits.preview_text_bytes_max,
                    writing,
                }
            }
        };
        let payload = Arc::new(payload);
        self.previews
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .put(id, Arc::clone(&payload), limits.preview_cache_bytes);
        Ok(payload)
    }

    /// A document row's text, by id (t-16006). The gallery lists every project's
    /// documents and the project's own file door refuses a path outside its
    /// root, as it must; a document outside the open project is read here
    /// instead, out of the catalog's own row, so nothing about the path comes
    /// from the window. The bounds are the drawer preview's (`preview`): a row
    /// of kind document that is text, a regular file, and the table's text cap
    /// — a longer file is cut on a whole character and says so, because the
    /// person asked to read it. `version` names one of the row's kept snapshots,
    /// found in the store's own list (`versions`) by number. Every refusal names
    /// the row, so the window can say it in one sentence.
    pub(crate) fn document_text(
        &self,
        id: &str,
        version: Option<u32>,
    ) -> Result<DocumentText, String> {
        use std::io::Read as _;
        let artifact = self
            .get(id)
            .ok_or_else(|| format!("그 아티팩트가 없습니다: {id}"))?;
        if artifact.kind != ArtifactKind::Document {
            return Err(format!(
                "문서가 아닌 아티팩트입니다: {id} ({})",
                artifact.kind.as_str()
            ));
        }
        if !is_utf8_document(&artifact.path) {
            return Err(format!("텍스트 문서가 아닙니다: {id}"));
        }
        let path = match version {
            None => artifact.path,
            Some(n) => self
                .versions(id)
                .into_iter()
                .find(|kept| kept.n == n)
                .map(|kept| kept.path)
                .ok_or_else(|| format!("그 버전은 더 이상 보관되지 않습니다: {id} 버전 {n}"))?,
        };
        let cap = self.limits().preview_text_bytes_max;
        // A row outlives its file until the next scan; say so in words rather than
        // in the operating system's.
        let meta = std::fs::metadata(&path).map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => format!("아티팩트 파일이 사라졌습니다: {id}"),
            _ => error.to_string(),
        })?;
        if !meta.is_file() {
            return Err(format!("일반 파일이 아닙니다: {id}"));
        }
        let mut held = Vec::new();
        std::fs::File::open(&path)
            .and_then(|file| file.take(cap).read_to_end(&mut held))
            .map_err(|error| error.to_string())?;
        if held.contains(&0) {
            return Err(format!("바이너리 파일입니다: {id}"));
        }
        Ok(DocumentText {
            text: text_of_whole_characters(held),
            bytes: meta.len(),
            truncated: meta.len() > cap,
        })
    }
}

/// Bytes as text, with a character the cut split in two left out rather than
/// shown as a replacement mark: a Korean document cut at the byte cap ends in
/// half of a three-byte character two times in three. Anything else that is not
/// UTF-8 is replaced, as the drawer's preview does.
fn text_of_whole_characters(held: Vec<u8>) -> String {
    match String::from_utf8(held) {
        Ok(text) => text,
        Err(error) => {
            let invalid = error.utf8_error();
            let bytes = error.into_bytes();
            let whole = if invalid.error_len().is_none() {
                &bytes[..invalid.valid_up_to()]
            } else {
                &bytes[..]
            };
            String::from_utf8_lossy(whole).into_owned()
        }
    }
}

struct ScanBudget {
    files: usize,
    bytes: u64,
}

impl ScanBudget {
    fn visit_file(&mut self) -> bool {
        let Some(left) = self.files.checked_sub(1) else {
            return false;
        };
        self.files = left;
        true
    }

    fn read_bytes(&mut self, bytes: u64) -> bool {
        let Some(left) = self.bytes.checked_sub(bytes) else {
            return false;
        };
        self.bytes = left;
        true
    }
}

/// Whether a row is a publication (t-3952): the only rows that carry a
/// version number of their own, written by `artifact_publish::PageMeta`.
/// 창이 청한 피드백 한 줄의 모양: 판 번호, 주석의 수와 길이, 받는 판의 열쇠와
/// 에이전트 id, 따라간 페이지의 주소. 판에 그대로 붙여 넣은 말이라도 기록에는 터미널
/// 제어 문자를 받지 않는다.
fn validate_feedback(ask: &FeedbackAsk) -> Result<(), String> {
    if ask.version == 0 {
        return Err("피드백의 버전은 1부터입니다".into());
    }
    if ask.items.is_empty() || ask.items.len() > FEEDBACK_ITEMS_MAX {
        return Err(format!("주석은 1–{FEEDBACK_ITEMS_MAX}개여야 합니다"));
    }
    let plain = |text: &str, lines: bool| {
        !text
            .chars()
            .any(|c| c.is_control() && !(lines && (c == '\n' || c == '\t')))
    };
    for item in &ask.items {
        if item.selector.chars().count() > FEEDBACK_SELECTOR_MAX
            || item.comment.chars().count() > FEEDBACK_COMMENT_MAX
        {
            return Err(format!(
                "선택자는 {FEEDBACK_SELECTOR_MAX}자, 코멘트는 {FEEDBACK_COMMENT_MAX}자까지입니다"
            ));
        }
        if !plain(&item.selector, false) || !plain(&item.comment, true) {
            return Err("주석에 제어 문자가 들어 있습니다".into());
        }
        if item.selector.trim().is_empty() && item.comment.trim().is_empty() {
            return Err("아무것도 말하지 않는 주석입니다".into());
        }
    }
    let pane = &ask.recipient.pane;
    let digits = pane.strip_prefix("term-").unwrap_or_default();
    if digits.is_empty() || digits.len() > 10 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err("받는 판은 창의 판 열쇠(term-<n>)여야 합니다".into());
    }
    let agent = &ask.recipient.agent;
    if agent.is_empty()
        || agent.len() > FEEDBACK_AGENT_MAX
        || !agent
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
    {
        return Err("받는 판의 에이전트 id가 아닙니다".into());
    }
    if let Some(url) = &ask.page_url
        && (url.trim().is_empty()
            || url.chars().count() > FEEDBACK_PAGE_URL_MAX
            || !plain(url, false))
    {
        return Err(format!(
            "따라간 페이지의 주소는 비지 않고 제어 문자 없이 {FEEDBACK_PAGE_URL_MAX}자까지입니다"
        ));
    }
    Ok(())
}

/// 내보낼 파일 이름의 앞부분: 제목에서 경로와 셸이 달리 읽는 글자를 빼고, 비면 id.
fn export_stem(title: &str, id: &str) -> String {
    let cleaned: String = title
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                '-'
            } else {
                c
            }
        })
        .collect();
    let stem = artifact::truncate_chars(
        cleaned.trim().trim_matches(['.', '-', ' ']),
        EXPORT_STEM_MAX,
    );
    if stem.is_empty() {
        id.to_string()
    } else {
        stem
    }
}

fn is_publication(artifact: &Artifact) -> bool {
    artifact.kind == ArtifactKind::Page && artifact.version.is_some()
}

/// 파일을 이름 붙인 행만 존재를 묻는다. claude.ai 행에는 파일이 없다.
fn is_gone(artifact: &Artifact) -> bool {
    !artifact.path.as_os_str().is_empty() && !artifact.path.is_file()
}

/// Whether a document is Markdown rather than a PDF — by extension, the way
/// the kind table judged it.
fn is_utf8_document(path: &Path) -> bool {
    !path
        .extension()
        .is_some_and(|held| held.to_string_lossy().eq_ignore_ascii_case("pdf"))
}

/// The SHA-256 of bytes in hand, hex — the same digest `file_sha256` answers
/// for a file, for a picture already read.
pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest as _, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}

/// The SHA-256 of a whole file, hex — `None` when it cannot be read or is
/// larger than `cap`, because a digest of part of a file is not that file's.
pub(crate) fn file_sha256(path: &Path, cap: u64) -> Option<String> {
    use sha2::{Digest as _, Sha256};
    use std::io::Read as _;
    let file = std::fs::File::open(path).ok()?;
    if file.metadata().ok()?.len() > cap {
        return None;
    }
    let mut reader = file.take(cap.saturating_add(1));
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        let read = reader.read(&mut buffer).ok()?;
        if read == 0 {
            break;
        }
        total += read as u64;
        if total > cap {
            return None;
        }
        hasher.update(&buffer[..read]);
    }
    Some(format!("{:x}", hasher.finalize()))
}

fn image_mime(path: &Path) -> &'static str {
    match path
        .extension()
        .map(|held| held.to_string_lossy().to_ascii_lowercase())
        .as_deref()
    {
        Some("png") => "image/png",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("bmp") => "image/bmp",
        _ => "image/jpeg",
    }
}

/// The origin a ledger row vouches for. Only what the row holds: the seat,
/// the agent and model it was launched as, the checkout it was placed in, the
/// task its dispatch carries. No commit — the ledger records none.
pub(crate) fn origin_of_worker(
    run: &zerocode_core::orchestration::Run,
    worker: &zerocode_core::orchestration::Worker,
) -> Origin {
    let task =
        worker_task_id(&run.dispatches, &worker.id, worker.dispatch.as_deref()).map(str::to_string);
    Origin {
        work_summary: task
            .as_deref()
            .and_then(|id| run.task(id))
            .map(|task| task.display_name().to_string()),
        run: Some(run.id.clone()),
        task,
        worker: Some(worker.id.clone()),
        pane: Some(format!("{}/{}", worker.team, worker.pane)),
        agent: Some(worker.agent.clone()),
        model: worker.model.clone(),
        commit: None,
        worktree: worker.checkout.as_deref().map(PathBuf::from),
        automation: None,
        session: None,
        project: None,
    }
}

// Completing a dispatch clears the worker's active pointer before its report
// is registered. The immutable dispatch history still names the work it did.
fn worker_task_id<'a>(
    dispatches: &'a [zerocode_core::orchestration::Dispatch],
    worker: &str,
    active: Option<&str>,
) -> Option<&'a str> {
    active
        .and_then(|id| dispatches.iter().find(|d| d.id == id && d.worker == worker))
        .or_else(|| {
            dispatches
                .iter()
                .filter(|d| d.worker == worker)
                .max_by_key(|d| d.started_ms)
        })
        .map(|dispatch| dispatch.task.as_str())
}

/// The window's road after the file watcher's lane reported movement, or
/// after boot: one bounded pass, then the watcher's lane is re-aimed at the
/// folders the store now knows, and the window hears about any change.
pub(crate) fn rescan_and_tell(app: &tauri::AppHandle, now_ms: i64) -> Option<ScanReport> {
    use tauri::{Emitter as _, Manager as _};
    let store = store()?;
    let report = store.scan(now_ms);
    app.state::<AppState>()
        .watched()
        .replace_lane(WATCH_LANE, store.watch_targets(), true);
    if report.changed() {
        let _ = app.emit(CHANGED_EVENT, ());
    }
    Some(report)
}

/// The hook road (t-3233 §2): a Claude or zo pane's `Stop`/`PostToolUse`
/// names its transcript, and the tail past the remembered stamp is read for
/// the artifacts it published and the pages it wrote. The window hears
/// `artifacts:changed` only when a row landed. Cheap when the envelope is
/// not one of those — a string look, no disk.
pub(crate) fn note_hook(app: &tauri::AppHandle, envelope: &zerocode_core::HookEnvelope) {
    use tauri::Emitter as _;
    if !envelope.payload.contains("transcript_path") && !envelope.payload.contains("cwd") {
        return;
    }
    let Some(store) = store() else {
        return;
    };
    if crate::artifact_transcripts::note_hook(&store, envelope, crate::now_epoch_ms()) {
        let _ = app.emit(CHANGED_EVENT, ());
    }
}

/// The boot road runs bounded passes on the existing reconcile thread until
/// legacy history is judged. Refresh uses the same road; no manual refresh
/// loop or additional thread is needed to finish a long migration.
pub(crate) fn backfill_transcripts(
    app: &tauri::AppHandle,
) -> Option<crate::artifact_transcripts::BackfillReport> {
    use tauri::{Emitter as _, Manager as _};
    let store = store()?;
    let home = dirs::home_dir()?;
    let window_home = app.state::<AppState>().config_root().to_path_buf();
    let roots = crate::artifact_transcripts::roots(&home, &window_home);
    let mut total = crate::artifact_transcripts::BackfillReport::default();
    loop {
        let report = crate::artifact_transcripts::backfill(&store, &roots, crate::now_epoch_ms());
        if report.changed() {
            app.state::<AppState>()
                .watched()
                .replace_lane(WATCH_LANE, store.watch_targets(), true);
            let _ = app.emit(CHANGED_EVENT, ());
        }
        total.files += report.files;
        total.bytes += report.bytes;
        total.remote += report.remote;
        total.pages += report.pages;
        total.forgotten += report.forgotten;
        total.truncated |= report.truncated;
        total.rejudging = report.rejudging;
        if report.rejudging == 0 {
            return Some(total);
        }
        // Each pass releases the transcript lock, allowing hooks to add
        // durable creation evidence before the next cleanup decision.
        std::thread::yield_now();
    }
}

/// The automation runtime's registration road: one run's evidence folder,
/// with the origin the run row vouches for. The folder is walked by the next
/// scan; nothing is read here.
pub(crate) fn adopt_evidence(dir: &str, automation_id: &str, worktree: Option<&str>) {
    let Some(store) = store() else {
        return;
    };
    store.adopt_source(ScanSource {
        dir: PathBuf::from(dir),
        source: Source::Evidence,
        origin: Origin {
            automation: Some(automation_id.to_string()),
            worktree: worktree.map(PathBuf::from),
            ..Origin::default()
        },
    });
}

/// The automation runtime's other registration road: these evidence folders
/// were swept, so their rows go too. Only the catalog moves; the files were
/// the caller's to remove.
pub(crate) fn forget_evidence(dirs: &[PathBuf]) {
    if let Some(store) = store()
        && !dirs.is_empty()
    {
        store.forget_under(dirs);
    }
}

/// The table's cap on retention days — the one number the settings field
/// shows as its `max`, read from here so settings never spell it.
pub(crate) fn retention_days_max() -> u32 {
    Limits::default().retention_days_max
}

/// The settings overlay's road: the person changed the retention days.
/// Answers the days as the table admits them (clamped), which is what the
/// settings document stores.
pub(crate) fn note_retention_days(days: u32) -> u32 {
    let days = days.min(retention_days_max());
    let Some(store) = store() else {
        return days;
    };
    let overlay: BTreeMap<String, u64> = [(
        format!("{}retention_days", artifact::OVERLAY_PREFIX),
        u64::from(days),
    )]
    .into_iter()
    .collect();
    store.set_limits(Limits::default().overlaid(&overlay));
    days
}

/// The authenticated hook route uses the same store that the gallery reads.
pub(crate) struct ArtifactDoor;
impl zerocode_hookd::ArtifactCommands for ArtifactDoor {
    fn execute(&self, request: serde_json::Value) -> Result<serde_json::Value, String> {
        let store = store().ok_or("artifact store is not ready")?;
        let app = store
            .window
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        artifact_request(&store, request, &|caller| {
            app.as_ref()
                .map_or_else(Origin::default, |app| window_origin(app, caller))
        })
    }
}

/// What the window knows about the pane a publication came from. The run,
/// worker and task are the ledger's seat in that pane, when it holds one.
#[derive(Default)]
struct PaneFacts {
    agent: Option<String>,
    model: Option<String>,
    run: Option<String>,
    worker: Option<String>,
    task: Option<String>,
}

/// A project this window keeps, and the workspaces it knows for it.
struct KnownProject {
    root: PathBuf,
    workspaces: Vec<PathBuf>,
}

/// The origin a publication's pane vouches for: the pane under the window's
/// own spelling of its key, the project and workspace this window keeps that
/// hold the folder the publish was asked from, and the agent, model and ledger
/// seat the window already knows for that pane. A publish from no pane this window
/// holds — a shell outside it, a key of another spelling, a pane since
/// closed — records nothing at all; a folder no known workspace holds names
/// no project rather than being promoted to one. A label, like every origin:
/// nothing is authorized by it.
fn publication_origin(
    caller: &zerocode_core::artifact_publish::Caller,
    pane: impl FnOnce(crate::TermId) -> Option<PaneFacts>,
    known: impl FnOnce(&Path) -> Vec<KnownProject>,
) -> Origin {
    let Some(term) = caller
        .pane
        .as_deref()
        .and_then(crate::hooks::term_of_pane_key)
    else {
        return Origin::default();
    };
    let Some(facts) = pane(term) else {
        return Origin::default();
    };
    let (project, worktree) = caller
        .cwd
        .as_deref()
        .filter(|cwd| cwd.is_absolute())
        .map_or((None, None), |cwd| place_of(cwd, &known(cwd)));
    Origin {
        pane: Some(crate::hooks::pane_key_of(term)),
        agent: facts.agent,
        model: facts.model,
        run: facts.run,
        worker: facts.worker,
        task: facts.task,
        worktree,
        project,
        ..Origin::default()
    }
}

/// The known project and workspace that hold `cwd` — the deepest workspace
/// wins, so a checkout nested inside another names itself. The spellings are
/// the catalog's and git's own, which is what the worktree chip keys on; only
/// the comparison is made on resolved paths.
fn place_of(cwd: &Path, known: &[KnownProject]) -> (Option<PathBuf>, Option<PathBuf>) {
    let Ok(cwd) = cwd.canonicalize() else {
        return (None, None);
    };
    let mut best: Option<(usize, &KnownProject, &PathBuf)> = None;
    for project in known {
        for workspace in &project.workspaces {
            let Ok(held) = workspace.canonicalize() else {
                continue;
            };
            let depth = held.components().count();
            if cwd.starts_with(&held) && best.is_none_or(|(deepest, ..)| depth > deepest) {
                best = Some((depth, project, workspace));
            }
        }
    }
    best.map_or((None, None), |(_, project, workspace)| {
        (Some(project.root.clone()), Some(workspace.clone()))
    })
}

/// The projects this window keeps, each with the workspaces it knows for it.
/// Only the repository `cwd` stands in is asked for its worktree list — one
/// `rev-parse` and one `worktree list`, however many projects the catalog
/// holds; every other project is its own folder (its main checkout, or a
/// folder workspace). A repository no stored project belongs to lends its
/// list to nobody.
fn known_projects(config_root: &Path, cwd: &Path) -> Vec<KnownProject> {
    let listed: Vec<PathBuf> = zerocode_orchestrator::Orchestrator::open(cwd)
        .and_then(|repository| repository.list())
        .map(|worktrees| {
            worktrees
                .into_iter()
                .map(|worktree| worktree.path)
                .collect()
        })
        .unwrap_or_default();
    let resolved = |path: &Path| path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let listed_resolved: Vec<PathBuf> = listed.iter().map(|path| resolved(path)).collect();
    crate::shell_runtime::stored_projects(config_root)
        .into_iter()
        .map(PathBuf::from)
        .map(|root| {
            let workspaces = if listed_resolved.contains(&resolved(&root)) {
                listed.clone()
            } else {
                vec![root.clone()]
            };
            KnownProject { root, workspaces }
        })
        .collect()
}

/// The ledger's seat in a pane, as [`pane_facts`] reads it.
struct Seat<'a> {
    agent: &'a str,
    model: Option<&'a str>,
    run: &'a str,
    worker: &'a str,
    task: &'a str,
}

/// A pane's facts: the agent the window seated or heard there, else the
/// seat's; the seat's model, run, worker and task only while the seat's agent
/// is the one the pane runs — a seat another agent now sits over vouches for
/// nothing. The ledger's empty word is absence (`LedgerAgent::task_id`).
fn pane_facts(heard: Option<&str>, seat: Option<Seat<'_>>) -> PaneFacts {
    let agent = heard.or(seat.as_ref().map(|seat| seat.agent));
    let Some(seat) = seat.filter(|seat| agent == Some(seat.agent)) else {
        return PaneFacts {
            agent: agent.map(str::to_string),
            ..PaneFacts::default()
        };
    };
    let named = |value: &str| (!value.is_empty()).then(|| value.to_string());
    PaneFacts {
        agent: Some(seat.agent.to_string()),
        model: seat.model.map(str::to_string),
        run: named(seat.run),
        worker: named(seat.worker),
        task: named(seat.task),
    }
}

/// This window's answer for [`publication_origin`]: a pane it holds, what
/// [`pane_facts`] makes of the agent it seated or heard there and of the
/// ledger's seat in it.
fn window_origin(
    app: &tauri::AppHandle,
    caller: &zerocode_core::artifact_publish::Caller,
) -> Origin {
    use tauri::Manager as _;
    let state = app.state::<AppState>();
    publication_origin(
        caller,
        |term| {
            if !state.terminals().contains_key(&term) {
                return None;
            }
            let heard = state.agent_terms().get(&term).copied();
            let ledger = crate::orchestration::board_ledger_snapshot();
            let seat = ledger
                .agents
                .iter()
                .find(|row| row.term == Some(term))
                .map(|row| Seat {
                    agent: &row.agent,
                    model: row.model.as_deref(),
                    run: &row.run,
                    worker: &row.worker,
                    task: &row.task_id,
                });
            Some(pane_facts(heard, seat))
        },
        |cwd| known_projects(state.config_root(), cwd),
    )
}

fn artifact_request(
    store: &Store,
    mut request: serde_json::Value,
    origin_of: &dyn Fn(&zerocode_core::artifact_publish::Caller) -> Origin,
) -> Result<serde_json::Value, String> {
    let action = request
        .get("action")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string();
    match action.as_str() {
        "publish" => {
            let (input, caller) = zerocode_core::artifact_publish::publish_parts(request)?;
            let origin = origin_of(&caller);
            let meta = store.publish_page(&input, origin.clone())?;
            if let Some(app) = store
                .window
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .as_ref()
            {
                use tauri::Emitter as _;
                let _ = app.emit(CHANGED_EVENT, ());
                let mut row = meta.artifact(origin);
                store.fill_feedback(&mut row);
                let _ = app.emit(PUBLISHED_EVENT, row);
            }
            serde_json::to_value(meta).map_err(|e| e.to_string())
        }
        // Publications only, as the headless catalog answers: a page an agent
        // wrote into its project is the gallery's, not this door's to read
        // or export by version (t-3952).
        "list" => serde_json::to_value(store.list(&Filter {
            kind: Some("page".into()),
            published: Some(true),
            ..Filter::default()
        }))
        .map_err(|e| e.to_string()),
        "read" => {
            let id = request
                .get("id")
                .and_then(serde_json::Value::as_str)
                .ok_or("read needs id")?;
            let row = store.get(id).ok_or("artifact not found")?;
            let html = zerocode_core::artifact_publish::read_page(store.root(), &row)?;
            Ok(serde_json::json!({ "artifact": row, "html": html }))
        }
        "export" => {
            request
                .as_object_mut()
                .ok_or("expected object")?
                .remove("action");
            let input: zerocode_core::artifact_publish::ExportInput =
                serde_json::from_value(request).map_err(|e| e.to_string())?;
            store.get(&input.id).ok_or("artifact not found")?;
            // `--out`의 확장자가 파일의 종류를 가른다: .html·.htm은 복사, .pdf·.png는
            // 창의 렌더러가 그린다(t-18558). 렌더러가 없으면 이유와 함께 거절한다.
            let renderer = store.renderer();
            serde_json::to_value(zerocode_core::artifact_publish::export_with(
                store.root(),
                &input,
                renderer.as_deref(),
            )?)
            .map_err(|e| e.to_string())
        }
        _ => Err("Artifact action must be publish, list, read or export".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test-only doors onto the store, beside the tests so the shipped half
    /// of this file ends where they begin.
    impl Store {
        /// Catalog a file where it is — an evidence frame, an export — without
        /// copying it. The row is refreshed when the file changed, kept when not.
        /// The scan is the shipped road onto `register_in_place_locked`; this
        /// door is the tests' way of placing one file without a source folder.
        pub(crate) fn register_in_place(
            &self,
            path: &Path,
            source: Source,
            origin: Origin,
            now_ms: i64,
        ) -> Result<Artifact, String> {
            let limits = self.limits();
            let mut index = self.index.lock().unwrap_or_else(PoisonError::into_inner);
            self.register_in_place_locked(&mut index, path, source, origin, now_ms, &limits)
                .map(|(artifact, _)| artifact)
        }

        /// Copy a report into the store verbatim and catalog it — the road a
        /// worker's report took before the keeping, and the shortest way to a
        /// report row in these tests. Nothing shipped calls it.
        pub(crate) fn register_report(
            &self,
            source_path: &Path,
            origin: Origin,
            now_ms: i64,
        ) -> Result<Artifact, String> {
            self.register_copy(source_path, Source::WorkerReport, origin, now_ms)
        }

        /// Bytes the preview cache holds right now — the number the tests pin.
        pub(crate) fn preview_cache_bytes(&self) -> u64 {
            self.previews
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .bytes
        }

        /// Rows held — the other number the tests pin.
        pub(crate) fn len(&self) -> usize {
            self.index
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .rows
                .len()
        }
    }
    use std::time::Duration;
    use zerocode_core::artifact::Preview;

    fn touch(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("mkdir");
        std::fs::write(path, text).expect("write");
    }

    /// A publish asked from no pane this window holds.
    fn nobody(_: &zerocode_core::artifact_publish::Caller) -> Origin {
        Origin::default()
    }

    fn origin(worker: &str) -> Origin {
        Origin {
            run: Some("run-1".into()),
            task: Some("t-1".into()),
            worker: Some(worker.into()),
            worktree: Some(PathBuf::from("/wt/a")),
            ..Origin::default()
        }
    }

    /// Two scans of an unchanged folder: the second reads nothing and changes
    /// nothing. A rewritten file is the one row that moves.
    #[test]
    fn completed_workers_keep_the_task_their_report_describes() {
        let dispatch =
            |id: &str, worker: &str, task: &str, at| zerocode_core::orchestration::Dispatch {
                id: id.to_string(),
                worker: worker.to_string(),
                task: task.to_string(),
                started_ms: at,
                ended_ms: Some(at + 1),
                succeeded: Some(true),
                retry_of: None,
                remote: None,
                source: None,
                session_history: None,
            };
        let history = vec![
            dispatch("d1", "w1", "old", 1),
            dispatch("d2", "w1", "current", 2),
            dispatch("d3", "w2", "someone-else", 3),
        ];
        assert_eq!(worker_task_id(&history, "w1", None), Some("current"));
        assert_eq!(worker_task_id(&history, "w1", Some("d1")), Some("old"));
        assert_eq!(worker_task_id(&history, "unknown", None), None);
    }

    #[test]
    fn reopening_a_filename_catalog_upgrades_titles_from_content() {
        let dir = tempfile::tempdir().expect("tempdir");
        let report = dir.path().join("t-42-report.md");
        touch(&report, "# SFTP home mapping and transfer queue");
        let data = dir.path().join("data");
        let store = Store::open(&data, Limits::default());
        let mut artifact = store
            .register_report(&report, origin("worker"), 1000)
            .expect("report");
        artifact.title = artifact
            .path
            .file_name()
            .expect("name")
            .to_string_lossy()
            .into_owned();
        store
            .append_line(&IndexLine::Row(Box::new(artifact)))
            .expect("legacy row");
        drop(store);
        let reopened = Store::open(&data, Limits::default());
        reopened.scan(2000);
        assert_eq!(
            reopened.list(&Filter::default()).rows[0].title,
            "SFTP home mapping and transfer queue"
        );
    }

    /// t-18558: the window's door names the file's kind by `--out`'s extension.
    /// A store with no renderer behind it (this test, a build without the
    /// window's WebKit) refuses `.pdf` and `.png` — and any other name that is
    /// not `.html` or `.htm` — instead of writing the page's HTML bytes into a
    /// file that claims to be a PDF or a picture.
    #[test]
    fn the_door_never_writes_html_bytes_into_a_pdf_or_png() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("store"), Limits::default());
        let source = dir.path().join("source.html");
        touch(&source, "<title>Report</title><main>hello</main>");
        let published = artifact_request(
            &store,
            serde_json::json!({"action":"publish", "file_path":source}),
            &nobody,
        )
        .unwrap();
        for name in ["report.pdf", "picture.png", "notes.txt"] {
            let out = dir.path().join(name);
            let answer = artifact_request(
                &store,
                serde_json::json!({"action":"export", "id":published["id"], "out":out}),
                &nobody,
            );
            assert!(answer.is_err(), "{name}: the door answered {answer:?}");
            assert!(!out.exists(), "{name}: a refused export left a file behind");
        }
    }

    /// One published page in a scratch store, with a fake renderer behind it.
    fn store_drawing(
        dir: &Path,
        drawn: &[u8],
    ) -> (
        Store,
        String,
        Arc<crate::artifact_render::fake::FakeRenderer>,
    ) {
        let store = Store::open(&dir.join("store"), Limits::default());
        let source = dir.join("source.html");
        touch(&source, "<title>Card sketch</title><main>hello</main>");
        let published = artifact_request(
            &store,
            serde_json::json!({"action":"publish", "file_path":source}),
            &nobody,
        )
        .unwrap();
        let renderer = Arc::new(crate::artifact_render::fake::FakeRenderer::drawing(drawn));
        store.set_renderer(renderer.clone());
        let id = published["id"].as_str().unwrap().to_string();
        (store, id, renderer)
    }

    /// t-18558: with a renderer behind the store the door writes a PDF or a
    /// picture where `--out`'s extension says so — drawn from the version's own
    /// immutable file — and a copy still asks nobody.
    #[test]
    fn the_door_draws_a_pdf_or_png_where_the_extension_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let (store, id, renderer) = store_drawing(dir.path(), b"%PDF-1.7 drawn");
        let out = dir.path().join("report.pdf");
        let answer = artifact_request(
            &store,
            serde_json::json!({"action":"export", "id":id, "out":out}),
            &nobody,
        )
        .unwrap();
        assert_eq!(std::fs::read(&out).unwrap(), b"%PDF-1.7 drawn");
        assert_eq!(
            (answer["format"].as_str(), answer["pages"].as_u64()),
            (Some("pdf"), Some(2))
        );
        let snapshot = store.root().join("pages").join(&id).join("v1/index.html");
        assert_eq!(renderer.asked(), vec![(snapshot, ExportFormat::Pdf)]);

        let copy = dir.path().join("copy.html");
        artifact_request(
            &store,
            serde_json::json!({"action":"export", "id":id, "out":copy}),
            &nobody,
        )
        .unwrap();
        assert_eq!(renderer.asked().len(), 1, "a copy asked the renderer");

        let bad = dir.path().join("notes.txt");
        let refused = artifact_request(
            &store,
            serde_json::json!({"action":"export", "id":id, "out":bad}),
            &nobody,
        )
        .unwrap_err();
        assert!(
            refused.contains(".pdf") && refused.contains(".png"),
            "{refused}"
        );
        assert!(!bad.exists());
    }

    /// t-18558: the strip's 「내보내기」 names its file `<제목>-v<n>.<형식>` in the
    /// folder it was given, draws once whatever the numbering costs, never
    /// overwrites, refuses a folder inside the store before drawing anything, and
    /// remembers the files it wrote so that the notice can reveal exactly those.
    #[test]
    fn export_into_names_a_drawn_file_by_its_format_and_draws_it_once() {
        let dir = tempfile::tempdir().unwrap();
        let (store, id, renderer) = store_drawing(dir.path(), b"%PDF-1.7 drawn");
        let folder = dir.path().join("shared");
        std::fs::create_dir_all(&folder).unwrap();
        let first = store
            .export_into(&id, 1, &folder, ExportFormat::Pdf)
            .unwrap();
        let second = store
            .export_into(&id, 1, &folder, ExportFormat::Pdf)
            .unwrap();
        assert_eq!(first.path.file_name().unwrap(), "Card sketch-v1.pdf");
        assert_eq!(second.path.file_name().unwrap(), "Card sketch-v1 (2).pdf");
        assert_eq!(std::fs::read(&first.path).unwrap(), b"%PDF-1.7 drawn");
        assert_eq!((first.format, first.pages), (ExportFormat::Pdf, Some(2)));
        // A taken name costs a probe, never a second drawing: two exports, two drawings.
        assert_eq!(renderer.asked().len(), 2);

        let picture = store
            .export_into(&id, 1, &folder, ExportFormat::Png)
            .unwrap();
        assert_eq!(picture.path.file_name().unwrap(), "Card sketch-v1.png");
        assert_eq!(picture.format, ExportFormat::Png);
        let copy = store
            .export_into(&id, 1, &folder, ExportFormat::Html)
            .unwrap();
        assert_eq!(copy.path.file_name().unwrap(), "Card sketch-v1.html");
        assert_eq!(renderer.asked().len(), 3, "a copy asked the renderer");
        assert_eq!(std::fs::read_dir(&folder).unwrap().count(), 4);

        for one in [&first, &second, &picture, &copy] {
            assert!(store.was_exported(&one.path), "{:?}", one.path);
        }
        assert!(!store.was_exported(&folder.join("elsewhere.pdf")));

        let before = renderer.asked().len();
        assert!(
            store
                .export_into(&id, 1, store.root(), ExportFormat::Pdf)
                .is_err()
        );
        assert!(
            store
                .export_into(&id, 9, &folder, ExportFormat::Pdf)
                .is_err()
        );
        assert_eq!(
            renderer.asked().len(),
            before,
            "a refused export drew a page"
        );
    }

    /// With no renderer behind the store, a drawn kind is refused with its
    /// reason and a copy still works; the menu's answer says the same.
    #[test]
    fn export_into_without_a_renderer_refuses_a_drawing_and_still_copies() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("store"), Limits::default());
        let source = dir.path().join("source.html");
        touch(&source, "<title>Card sketch</title><main>hello</main>");
        let published = artifact_request(
            &store,
            serde_json::json!({"action":"publish", "file_path":source}),
            &nobody,
        )
        .unwrap();
        let id = published["id"].as_str().unwrap();
        let folder = dir.path().join("shared");
        std::fs::create_dir_all(&folder).unwrap();
        let refused = store
            .export_into(id, 1, &folder, ExportFormat::Pdf)
            .unwrap_err();
        assert!(refused.contains("WebKit"), "{refused}");
        assert_eq!(std::fs::read_dir(&folder).unwrap().count(), 0);
        assert!(
            store
                .export_into(id, 1, &folder, ExportFormat::Html)
                .is_ok()
        );
        let choices = crate::artifact_render::format_choices(store.renderer().as_deref());
        assert_eq!(
            choices.iter().map(|one| one.available).collect::<Vec<_>>(),
            [true, false, false]
        );
    }

    #[test]
    fn artifact_window_export_uses_the_requested_snapshot_without_changing_the_catalog() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("store"), Limits::default());
        let source = dir.path().join("source.html");
        let out = dir.path().join("share.html");
        touch(&source, "<main>first</main>");
        let request = serde_json::json!({"action":"publish", "file_path":source});
        let first = artifact_request(&store, request.clone(), &nobody).unwrap();
        touch(&source, "<main>second</main>");
        artifact_request(&store, request, &nobody).unwrap();
        std::fs::remove_file(source).unwrap();
        let answer = artifact_request(
            &store,
            serde_json::json!({"action":"export", "id":first["id"], "version":1, "out":out}),
            &nobody,
        )
        .unwrap();
        assert_eq!(answer["version"], 1);
        let snapshot = store
            .root()
            .join("pages")
            .join(first["id"].as_str().unwrap())
            .join("v1/index.html");
        assert_eq!(
            std::fs::read(out).unwrap(),
            std::fs::read(snapshot).unwrap()
        );
        assert_eq!(store.list(&Filter::default()).total, 1);
        assert_eq!(
            store.get(first["id"].as_str().unwrap()).unwrap().version,
            Some(2)
        );
    }

    #[test]
    fn artifact_publication_is_one_gallery_row_after_reopen_and_delete_cleans_versions() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("report.html");
        touch(&source, "<main>original</main>");
        let store = Store::open(dir.path(), Limits::default());
        let input = zerocode_core::artifact_publish::PublishInput {
            file_path: source.clone(),
            title: Some("Orbit Review".into()),
            description: Some("A finished report".into()),
            favicon: Some("🌿".into()),
            ..Default::default()
        };
        let one = store.publish_page(&input, Origin::default()).unwrap();
        touch(&source, "<main>revised</main>");
        let two = store.publish_page(&input, Origin::default()).unwrap();
        assert_eq!(one.url, two.url);
        assert_eq!(two.version, 2);
        let store = Store::open(dir.path(), Limits::default());
        assert_eq!(store.list(&Filter::default()).total, 1);
        let row = store.get(&one.id).unwrap();
        assert_eq!(row.kind, ArtifactKind::Page);
        assert_eq!(row.title, "Orbit Review");
        assert_eq!(row.description.as_deref(), Some("A finished report"));
        assert_eq!(row.favicon.as_deref(), Some("🌿"));
        assert_eq!(row.version, Some(2));
        assert!(store.delete(&row.id, true).unwrap());
        assert!(!store.root().join("pages").join(&row.id).exists());
        assert!(source.exists());
    }

    /// The version picker of a publication lists its store's own immutable
    /// versions with their digests; the publication door lists publications
    /// only (an agent's project page stays the gallery's); the snapshot road
    /// digests what it kept; and a publication catalogued before rows named
    /// their editable source reads it back at open (t-3952).
    #[test]
    fn a_publication_lists_its_kept_versions_and_the_door_lists_only_publications() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        let store = Store::open(&data, Limits::default());
        let source = dir.path().join("deck.html");
        touch(&source, "<main>first</main>");
        let request = serde_json::json!({"action":"publish", "file_path":source});
        let first = artifact_request(&store, request.clone(), &nobody).unwrap();
        touch(&source, "<main>second</main>");
        let second = artifact_request(&store, request, &nobody).unwrap();
        let id = first["id"].as_str().unwrap().to_string();
        let versions = store.versions(&id);
        assert_eq!(
            versions.iter().map(|one| one.n).collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert!(versions[0].path.ends_with("v1/index.html"));
        assert_eq!(versions[1].sha256.as_deref(), second["sha256"].as_str());
        assert_ne!(versions[0].sha256, versions[1].sha256);

        let project = dir.path().join("project");
        touch(&project.join("site.html"), "<title>Site</title>");
        store
            .register_page(
                &PageFact {
                    path: project.join("site.html"),
                    at_ms: Some(1),
                    session: Some("s-1".into()),
                    project: Some(project.clone()),
                },
                "claude",
                1,
            )
            .unwrap()
            .expect("the created page is a row");
        let pages = store.list(&Filter {
            kind: Some("page".into()),
            ..Filter::default()
        });
        assert_eq!(pages.total, 2);
        let listed =
            artifact_request(&store, serde_json::json!({"action":"list"}), &nobody).unwrap();
        assert_eq!(listed["total"], 1, "{listed}");
        assert_eq!(listed["rows"][0]["id"], id.as_str());
        let page = store.agent_pages().remove(0);
        let snapshots = store.versions(&page.id);
        assert_eq!(snapshots.len(), 1);
        assert_eq!(
            snapshots[0].sha256,
            file_sha256(&snapshots[0].path, u64::MAX),
            "{snapshots:?}"
        );
        assert!(snapshots[0].sha256.is_some());

        let mut legacy = store.get(&id).unwrap();
        assert_eq!(legacy.source_path, Some(source.canonicalize().unwrap()));
        legacy.source_path = None;
        store
            .append_line(&IndexLine::Row(Box::new(legacy)))
            .unwrap();
        drop(store);
        let reopened = Store::open(&data, Limits::default());
        assert_eq!(
            reopened.get(&id).unwrap().source_path,
            Some(source.canonicalize().unwrap())
        );
    }

    /// A publication from a pane records the pane under the window's own
    /// spelling, the project and workspace the window keeps that hold its
    /// folder — the deepest one, a linked checkout outside the project
    /// included — and the agent, model and ledger seat the window knows for
    /// that pane. An unknown folder names no project; a pane the window does
    /// not hold, or a publish with no pane at all, records nothing and asks
    /// nothing.
    #[test]
    fn a_publication_origin_is_what_the_window_knows_about_its_pane_and_folder() {
        use zerocode_core::artifact_publish::Caller;
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("project");
        let nested = project.join(".worktrees").join("side");
        let linked = dir.path().join("workspaces").join("feature");
        let elsewhere = dir.path().join("elsewhere");
        for folder in [
            project.join("src"),
            nested.join("src"),
            linked.join("ui"),
            elsewhere.clone(),
        ] {
            std::fs::create_dir_all(folder).unwrap();
        }
        let known = || {
            vec![KnownProject {
                root: project.clone(),
                workspaces: vec![project.clone(), nested.clone(), linked.clone()],
            }]
        };
        let facts = |term| {
            (term == 7).then(|| PaneFacts {
                agent: Some("claude".into()),
                model: Some("claude-opus-5".into()),
                run: Some("run-1".into()),
                worker: Some("w-1".into()),
                task: Some("t-1".into()),
            })
        };
        let from = |pane: &str, cwd: &Path| {
            publication_origin(
                &Caller {
                    pane: Some(pane.to_string()),
                    cwd: Some(cwd.to_path_buf()),
                },
                facts,
                |_| known(),
            )
        };
        assert_eq!(
            from("term-7", &linked.join("ui")),
            Origin {
                pane: Some("term-7".into()),
                agent: Some("claude".into()),
                model: Some("claude-opus-5".into()),
                run: Some("run-1".into()),
                worker: Some("w-1".into()),
                task: Some("t-1".into()),
                worktree: Some(linked.clone()),
                project: Some(project.clone()),
                ..Origin::default()
            }
        );
        let in_project = from("term-7", &project.join("src"));
        assert_eq!(
            (in_project.project, in_project.worktree),
            (Some(project.clone()), Some(project.clone()))
        );
        assert_eq!(
            from("term-7", &nested.join("src")).worktree,
            Some(nested.clone()),
            "the deepest workspace names itself"
        );
        let unknown = from("term-7", &elsewhere);
        assert_eq!(
            (unknown.pane.as_deref(), unknown.project, unknown.worktree),
            (Some("term-7"), None, None)
        );
        assert_eq!(from("term-7", Path::new("project/src")).project, None);
        for pane in [None, Some("term-9"), Some("tab-1/leaf-2")] {
            let asked = std::cell::Cell::new(false);
            let origin = publication_origin(
                &Caller {
                    pane: pane.map(str::to_string),
                    cwd: Some(linked.clone()),
                },
                facts,
                |_| {
                    asked.set(true);
                    known()
                },
            );
            assert!(origin.is_empty(), "{pane:?}: {origin:?}");
            assert!(!asked.get(), "{pane:?} asked the catalog");
        }
    }

    /// A pane's facts take the ledger's seat only while the seat's agent is the
    /// one the pane runs: the seat's model, run, worker and task then ride
    /// with it, an empty ledger word is no fact, and a seat another agent now
    /// sits over lends nothing but leaves the heard agent standing. With
    /// nothing heard, the seat's own agent is the pane's.
    #[test]
    fn a_pane_takes_its_ledger_seat_only_while_the_seat_is_its_agent() {
        let seat = |agent| Seat {
            agent,
            model: Some("claude-opus-5"),
            run: "run-1",
            worker: "w-1",
            task: "",
        };
        let facts = pane_facts(Some("claude"), Some(seat("claude")));
        assert_eq!(
            (
                facts.agent.as_deref(),
                facts.model.as_deref(),
                facts.run.as_deref(),
                facts.worker.as_deref(),
                facts.task.as_deref()
            ),
            (
                Some("claude"),
                Some("claude-opus-5"),
                Some("run-1"),
                Some("w-1"),
                None
            )
        );
        let seated = pane_facts(None, Some(seat("codex")));
        assert_eq!(
            (seated.agent.as_deref(), seated.worker.as_deref()),
            (Some("codex"), Some("w-1"))
        );
        let other = pane_facts(Some("claude"), Some(seat("codex")));
        assert_eq!(
            (
                other.agent.as_deref(),
                other.model,
                other.run,
                other.worker,
                other.task
            ),
            (Some("claude"), None, None, None, None)
        );
        let alone = pane_facts(None, None);
        assert_eq!((alone.agent, alone.worker), (None, None));
    }

    /// Through the door: a publication from a pane in a known workspace is
    /// counted on that workspace's chip; a republish from another pane and
    /// folder records the newer publisher and the chip follows it; a publish
    /// from no pane keeps an empty origin; the rows read back as written.
    #[test]
    fn a_publication_carries_its_publishers_origin_onto_the_worktree_chip() {
        use zerocode_core::artifact_publish::Caller;
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        let project = dir.path().join("project");
        let linked = dir.path().join("feature");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(&linked).unwrap();
        let store = Store::open(&data, Limits::default());
        let window = |caller: &Caller| {
            publication_origin(
                caller,
                |term| {
                    Some(PaneFacts {
                        agent: Some(if term == 7 { "claude" } else { "codex" }.into()),
                        ..PaneFacts::default()
                    })
                },
                |_| {
                    vec![KnownProject {
                        root: project.clone(),
                        workspaces: vec![project.clone(), linked.clone()],
                    }]
                },
            )
        };
        let source = dir.path().join("deck.html");
        touch(&source, "<main>first</main>");
        let publish = |pane: &str, cwd: &Path| {
            artifact_request(
                &store,
                serde_json::json!({"action":"publish", "file_path":source, "pane":pane, "cwd":cwd}),
                &window,
            )
            .unwrap()
        };
        let chip = |store: &Store, path: &Path| {
            store
                .counts()
                .by_worktree
                .get(&path.display().to_string())
                .copied()
        };
        let first = publish("term-7", &linked);
        let id = first["id"].as_str().unwrap().to_string();
        let row = store.get(&id).unwrap();
        assert_eq!(row.origin.pane.as_deref(), Some("term-7"));
        assert_eq!(row.origin.worktree.as_deref(), Some(linked.as_path()));
        assert_eq!(chip(&store, &linked), Some(1));
        touch(&source, "<main>second</main>");
        publish("term-8", &project);
        let row = store.get(&id).unwrap();
        assert_eq!(
            (
                row.version,
                row.origin.pane.as_deref(),
                row.origin.agent.as_deref()
            ),
            (Some(2), Some("term-8"), Some("codex"))
        );
        assert_eq!(
            (chip(&store, &linked), chip(&store, &project)),
            (None, Some(1))
        );
        let other = dir.path().join("other.html");
        touch(&other, "<main>plain</main>");
        let plain = artifact_request(
            &store,
            serde_json::json!({"action":"publish", "file_path":other, "cwd":linked}),
            &window,
        )
        .unwrap();
        assert!(
            store
                .get(plain["id"].as_str().unwrap())
                .unwrap()
                .origin
                .is_empty()
        );
        drop(store);
        let reopened = Store::open(&data, Limits::default());
        assert_eq!(reopened.get(&id).unwrap().origin, row.origin);
        assert_eq!(chip(&reopened, &project), Some(1));
    }

    /// End to end on a real repository: the argv the door sends from a
    /// subfolder of a linked checkout, read the way the bridge reads it, into
    /// the store. The project is the one the window keeps, spelled as it keeps
    /// it; the worktree is git's own spelling of the linked checkout — the one
    /// the sidebar's worktree row carries and its chip is keyed on — and the
    /// chip counts the publication. The catalog is asked about the folder only
    /// through `known_projects`, as the window asks it.
    #[test]
    fn a_publication_from_a_linked_checkout_names_the_kept_project_and_gits_worktree() {
        let git = |repo: &Path, args: &[&str]| {
            let output = crate::proc::quiet_command("git")
                .arg("-C")
                .arg(repo)
                .args(args)
                .output()
                .expect("git");
            assert!(
                output.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config");
        let repo = dir.path().join("repo");
        let linked = dir.path().join("wt-feature");
        std::fs::create_dir_all(&config).unwrap();
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        git(&repo, &["config", "user.name", "ZeroCode Test"]);
        git(&repo, &["config", "user.email", "test@zerocode"]);
        git(&repo, &["commit", "--allow-empty", "-q", "-m", "first"]);
        git(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "feature",
                &linked.to_string_lossy(),
            ],
        );
        std::fs::write(
            crate::project_runtime::recent_projects_file(&config),
            serde_json::to_string(&[repo.display().to_string()]).unwrap(),
        )
        .unwrap();
        let spelled = zerocode_orchestrator::Orchestrator::open(&repo)
            .unwrap()
            .list()
            .unwrap()
            .into_iter()
            .map(|worktree| worktree.path)
            .find(|path| path.ends_with("wt-feature"))
            .expect("git lists the linked checkout");
        let folder = linked.join("ui");
        std::fs::create_dir_all(&folder).unwrap();
        let source = folder.join("deck.html");
        touch(&source, "<main>deck</main>");
        let argv: Vec<String> = [
            "publish",
            "--file-path",
            &source.to_string_lossy(),
            "--cwd",
            &folder.to_string_lossy(),
            "--pane",
            "term-7",
        ]
        .iter()
        .map(|word| (*word).to_string())
        .collect();
        let request = zerocode_core::artifact_publish::request_from_argv(&argv).unwrap();
        let store = Store::open(&dir.path().join("data"), Limits::default());
        let answer = artifact_request(&store, request, &|caller| {
            publication_origin(
                caller,
                |_| {
                    Some(PaneFacts {
                        agent: Some("claude".into()),
                        model: Some("claude-opus-5".into()),
                        ..PaneFacts::default()
                    })
                },
                |cwd| known_projects(&config, cwd),
            )
        })
        .unwrap();
        let origin = store.get(answer["id"].as_str().unwrap()).unwrap().origin;
        println!(
            "recorded origin: {}",
            serde_json::to_string(&origin).unwrap()
        );
        assert_eq!(
            origin,
            Origin {
                pane: Some("term-7".into()),
                agent: Some("claude".into()),
                model: Some("claude-opus-5".into()),
                worktree: Some(spelled.clone()),
                project: Some(repo.clone()),
                ..Origin::default()
            }
        );
        assert_eq!(
            store
                .counts()
                .by_worktree
                .get(&spelled.display().to_string()),
            Some(&1)
        );
    }

    /// A path into the store's `pages/<id>/` names that publication: its
    /// current file, or a kept version by number. Its metadata, a file
    /// outside the store, and a deleted page name nothing.
    #[test]
    fn a_store_page_address_names_its_publication_and_version() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("data"), Limits::default());
        let source = dir.path().join("deck.html");
        touch(&source, "<main>first</main>");
        let request = serde_json::json!({"action":"publish", "file_path":source});
        let first = artifact_request(&store, request.clone(), &nobody).unwrap();
        touch(&source, "<main>second</main>");
        artifact_request(&store, request, &nobody).unwrap();
        let id = first["id"].as_str().unwrap();
        let seat = store.root().join("pages").join(id);
        let current = store
            .page_at(&seat.join("index.html"))
            .expect("the current file");
        assert_eq!((current.artifact.id.as_str(), current.version), (id, None));
        let kept = store
            .page_at(&seat.join("v1").join("index.html"))
            .expect("a kept version");
        assert_eq!(kept.version, Some(1));
        assert_eq!(
            store
                .page_at(&seat.join("v1").join("..").join("index.html"))
                .map(|at| at.version),
            Some(None),
            "the same file by a longer road is the same file"
        );
        assert_eq!(store.page_at(&seat.join("meta.json")), None);
        assert_eq!(store.page_at(&source), None);
        assert!(store.delete(id, true).unwrap());
        assert_eq!(store.page_at(&seat.join("index.html")), None);
    }

    #[test]
    fn a_scan_is_incremental_on_modified_time_and_length() {
        let dir = tempfile::tempdir().expect("tempdir");
        let data = dir.path().join("data");
        let evidence = dir.path().join("evidence");
        touch(&evidence.join("steps.jsonl"), "{\"n\":1}\n");
        touch(&evidence.join("shot-1.png"), "not really a png");
        let store = Store::open(&data, Limits::default());
        store.adopt_source(ScanSource {
            dir: evidence.clone(),
            source: Source::Evidence,
            origin: Origin {
                automation: Some("auto-1".into()),
                ..Origin::default()
            },
        });
        let first = store.scan(1_000);
        assert_eq!(
            (first.seen, first.added, first.updated),
            (2, 2, 0),
            "{first:?}"
        );
        let second = store.scan(2_000);
        assert_eq!(
            (second.seen, second.added, second.updated),
            (2, 0, 0),
            "{second:?}"
        );
        assert!(!second.changed());

        std::thread::sleep(Duration::from_millis(20));
        touch(&evidence.join("steps.jsonl"), "{\"n\":1}\n{\"n\":2}\n");
        let third = store.scan(3_000);
        assert_eq!((third.added, third.updated), (0, 1), "{third:?}");
        let rows = store.list(&Filter::default());
        assert_eq!(rows.rows.len(), 2);
        let steps = rows
            .rows
            .iter()
            .find(|row| row.title == "steps.jsonl")
            .expect("the step log is a row");
        assert_eq!(steps.kind, ArtifactKind::Evidence);
        assert_eq!(steps.origin.automation.as_deref(), Some("auto-1"));
        assert!(matches!(&steps.preview, Preview::Text { text } if text.contains("\"n\":2")));

        std::fs::remove_file(evidence.join("shot-1.png")).expect("remove");
        let fourth = store.scan(4_000);
        assert_eq!(fourth.removed, 1, "{fourth:?}");
        assert_eq!(store.len(), 1);
    }

    /// The bounds are the table's, and reaching one is reported rather than
    /// silently walked past.
    #[test]
    fn a_scan_past_the_table_reports_truncation() {
        let dir = tempfile::tempdir().expect("tempdir");
        let folder = dir.path().join("exports");
        for n in 0..5 {
            touch(&folder.join(format!("row-{n}.csv")), "a,b\n");
        }
        touch(&folder.join("deep/deeper/deepest/leaf.csv"), "a,b\n");
        let limits = Limits {
            scan_files_max: 3,
            ..Limits::default()
        };
        let store = Store::open(&dir.path().join("data"), limits);
        store.adopt_source(ScanSource {
            dir: folder.clone(),
            source: Source::Export,
            origin: Origin::default(),
        });
        let report = store.scan(1_000);
        assert!(report.truncated, "{report:?}");
        assert_eq!(report.added, 3);

        let shallow = Store::open(
            &dir.path().join("data2"),
            Limits {
                scan_depth: 2,
                ..Limits::default()
            },
        );
        shallow.adopt_source(ScanSource {
            dir: folder,
            source: Source::Export,
            origin: Origin::default(),
        });
        let report = shallow.scan(1_000);
        assert!(report.truncated, "depth was not reported: {report:?}");
        assert_eq!(report.added, 5, "the shallow rows still landed");
    }

    /// Registering the same report twice under the same origin is one
    /// artifact and one copy; a changed report is copied again under the
    /// same id; another worker's identical path is another artifact.
    #[test]
    fn a_report_copy_is_idempotent_and_survives_reopening() {
        let dir = tempfile::tempdir().expect("tempdir");
        let data = dir.path().join("data");
        let report = dir.path().join("tmp/t-1-report.md");
        touch(&report, "# report\n\nlanded 아티팩트 view\n");
        let store = Store::open(&data, Limits::default());
        let first = store
            .register_report(&report, origin("w-1"), 1_000)
            .expect("registers");
        let again = store
            .register_report(&report, origin("w-1"), 2_000)
            .expect("registers again");
        assert_eq!(first, again, "the same report became two artifacts");
        assert!(
            first
                .path
                .starts_with(data.join(STORE_DIR_NAME).join("run"))
        );
        assert_eq!(first.kind, ArtifactKind::Report);
        assert!(
            matches!(&first.preview, Preview::Markdown { text } if text.starts_with("# report"))
        );
        let lines = std::fs::read_to_string(data.join(STORE_DIR_NAME).join(INDEX_FILE))
            .expect("index")
            .lines()
            .count();
        assert_eq!(lines, 1, "an unchanged report appended a second line");

        std::thread::sleep(Duration::from_millis(20));
        touch(&report, "# report v2\n");
        let changed = store
            .register_report(&report, origin("w-1"), 3_000)
            .expect("re-registers");
        assert_eq!(changed.id, first.id);
        assert_eq!(
            std::fs::read_to_string(&changed.path).expect("copy"),
            "# report v2\n"
        );
        let other = store
            .register_report(&report, origin("w-2"), 3_000)
            .expect("another worker");
        assert_ne!(other.id, first.id);
        assert_eq!(store.counts().by_worker.get("w-1"), Some(&1));
        assert_eq!(store.counts().by_task.get("t-1"), Some(&2));
        assert_eq!(
            store.counts().report_by_worker.get("w-1"),
            Some(&changed.id)
        );
        assert_eq!(store.counts().report_by_worker.get("w-2"), Some(&other.id));

        // Reopened: the rows are still there, and search finds the body.
        let reopened = Store::open(&data, Limits::default());
        assert_eq!(reopened.len(), 2);
        reopened.scan(4_000);
        let found = reopened.list(&Filter {
            query: "v2 report".into(),
            ..Filter::default()
        });
        assert_eq!(found.rows.len(), 2, "{found:?}");
        let none = reopened.list(&Filter {
            query: "아티팩트".into(),
            ..Filter::default()
        });
        assert_eq!(none.rows.len(), 0, "the old body still matched: {none:?}");
        assert_eq!(reopened.ids_for_worker("w-2").len(), 1);
    }

    /// Pruning takes the row AND the store's copy, compacts the catalog, and
    /// leaves a file outside the app's data root alone.
    #[test]
    fn pruning_takes_the_row_and_the_file_together() {
        let dir = tempfile::tempdir().expect("tempdir");
        let data = dir.path().join("data");
        let report = dir.path().join("tmp/t-1-report.md");
        touch(&report, "# old\n");
        let export = dir.path().join("exports/rows.csv");
        touch(&export, "a,b\n");
        let store = Store::open(&data, Limits::default());
        let copied = store
            .register_report(&report, origin("w-1"), 1_000)
            .expect("registers");
        let outside = store
            .register_in_place(&export, Source::Export, Origin::default(), 1_000)
            .expect("registers in place");
        let day = 24 * 60 * 60 * 1000;
        let now = copied.modified_ms + 40 * day;
        let pruned = store.prune(now, 30);
        assert_eq!(pruned, Pruned { rows: 2, files: 1 }, "{pruned:?}");
        assert!(!copied.path.exists(), "the store's copy survived the sweep");
        assert!(export.exists(), "a file outside the data root was deleted");
        assert_eq!(store.len(), 0);
        let index =
            std::fs::read_to_string(data.join(STORE_DIR_NAME).join(INDEX_FILE)).expect("index");
        assert_eq!(
            index, "",
            "the catalog kept tombstones after compaction: {index}"
        );
        let _ = outside;

        // The ledger's clock: no sweep until the ledger swept, one per sweep.
        touch(&report, "# new\n");
        store
            .register_report(&report, origin("w-1"), 1_000)
            .expect("registers");
        assert_eq!(store.sweep_beside_ledger(None, 30, now), None);
        assert!(store.sweep_beside_ledger(Some(5), 30, now).is_some());
        assert_eq!(store.sweep_beside_ledger(Some(5), 30, now), None);
    }

    /// Without the person's confirmation nothing moves; with it, the row and
    /// the copy go.
    #[test]
    fn deleting_without_confirmation_is_a_no_op() {
        let dir = tempfile::tempdir().expect("tempdir");
        let data = dir.path().join("data");
        let report = dir.path().join("tmp/t-1-report.md");
        touch(&report, "# report\n");
        let store = Store::open(&data, Limits::default());
        let copied = store
            .register_report(&report, origin("w-1"), 1_000)
            .expect("registers");
        assert_eq!(store.delete(&copied.id, false), Ok(false));
        assert!(copied.path.exists());
        assert_eq!(store.len(), 1);
        assert_eq!(store.delete(&copied.id, true), Ok(true));
        assert!(!copied.path.exists());
        assert_eq!(store.len(), 0);
        assert_eq!(
            store.delete(&copied.id, true),
            Ok(false),
            "a second delete finds nothing"
        );
    }

    /// The preview cache is bounded by bytes, evicts the least recently used
    /// entry first, and forgets a row that was deleted.
    #[test]
    fn the_preview_cache_is_bounded_by_bytes_and_least_recently_used() {
        let dir = tempfile::tempdir().expect("tempdir");
        let data = dir.path().join("data");
        let limits = Limits {
            preview_cache_bytes: 40,
            ..Limits::default()
        };
        let store = Store::open(&data, limits);
        let mut ids = Vec::new();
        for n in 0..3 {
            let report = dir.path().join(format!("tmp/t-{n}-report.md"));
            touch(&report, "0123456789012345");
            ids.push(
                store
                    .register_report(&report, origin(&format!("w-{n}")), 1_000)
                    .expect("registers")
                    .id,
            );
        }
        for id in &ids[..2] {
            assert_eq!(store.preview(id).expect("preview").kind, "markdown");
        }
        assert_eq!(store.preview_cache_bytes(), 32);
        // Touch the first so the second is the one to go.
        store.preview(&ids[0]).expect("preview");
        store.preview(&ids[2]).expect("preview");
        assert_eq!(
            store.preview_cache_bytes(),
            32,
            "the cache grew past its cap"
        );
        let held = store.previews.lock().unwrap();
        assert!(
            held.held.contains_key(&ids[0]),
            "the recently used entry was evicted"
        );
        assert!(
            !held.held.contains_key(&ids[1]),
            "the least recently used entry stayed"
        );
        drop(held);
        store.delete(&ids[0], true).expect("deletes");
        assert_eq!(store.preview_cache_bytes(), 16);
    }

    /// A report's preview carries the writing lint of its text (t-32786), worked
    /// out once with the preview and kept with it in the cache. A text with
    /// nothing to count, and a kind that is not prose, carry none.
    #[test]
    fn a_reports_preview_carries_the_lint_of_its_text_and_the_cache_keeps_it() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = Store::open(&dir.path().join("data"), Limits::default());
        let report = dir.path().join("tmp/t-1-report.md");
        touch(
            &report,
            "Jev 자리의 판정이 느려서 스윕 박자를 조정함으로써 재시도가 줄어들게 되는 것이다.\n",
        );
        let id = store
            .register_report(&report, origin("w-1"), 1_000)
            .expect("registers")
            .id;
        let first = store.preview(&id).expect("preview");
        assert_eq!(first.kind, "markdown");
        assert_eq!(
            first
                .writing
                .as_ref()
                .map(|found| (found.sentences, found.words, found.patterns)),
            Some((1, 2, 1)),
            "the report's preview carried no lint: {first:?}"
        );
        let again = store.preview(&id).expect("preview");
        assert!(
            Arc::ptr_eq(&first, &again),
            "the cached preview was built again"
        );

        let numbers = dir.path().join("tmp/t-2-report.md");
        touch(&numbers, "0123456789012345");
        let id = store
            .register_report(&numbers, origin("w-2"), 1_000)
            .expect("registers")
            .id;
        assert!(
            store.preview(&id).expect("preview").writing.is_none(),
            "a text with no words carried a lint"
        );
    }

    /// Listing filters by kind and by each origin field, newest first, and
    /// says when the table cut the answer.
    #[test]
    fn listing_filters_by_kind_and_origin_and_is_bounded() {
        let dir = tempfile::tempdir().expect("tempdir");
        let data = dir.path().join("data");
        let limits = Limits {
            list_rows_max: 2,
            ..Limits::default()
        };
        let store = Store::open(&data, limits);
        for n in 0..3 {
            let report = dir.path().join(format!("tmp/t-{n}-report.md"));
            touch(&report, "# r\n");
            store
                .register_report(&report, origin(&format!("w-{n}")), 1_000 + n)
                .expect("registers");
        }
        let shot = dir.path().join("evidence/shot.png");
        touch(&shot, "png?");
        store
            .register_in_place(
                &shot,
                Source::Evidence,
                Origin {
                    automation: Some("auto-1".into()),
                    worktree: Some(PathBuf::from("/wt/b")),
                    ..Origin::default()
                },
                1_000,
            )
            .expect("registers");
        let all = store.list(&Filter::default());
        assert!(all.truncated);
        assert_eq!(all.total, 4);
        assert_eq!(all.rows.len(), 2);
        let shots = store.list(&Filter {
            kind: Some("screenshot".into()),
            ..Filter::default()
        });
        assert_eq!(shots.rows.len(), 1);
        assert_eq!(shots.rows[0].title, "shot.png");
        let worker = store.list(&Filter {
            worker: Some("w-1".into()),
            ..Filter::default()
        });
        assert_eq!(worker.rows.len(), 1);
        let worktree = store.list(&Filter {
            worktree: Some("/wt/b".into()),
            ..Filter::default()
        });
        assert_eq!(worktree.rows.len(), 1);
        let unknown_kind = store.list(&Filter {
            kind: Some("picture".into()),
            ..Filter::default()
        });
        assert_eq!(
            unknown_kind.total, 4,
            "an unknown kind word filtered rather than being ignored"
        );
        assert_eq!(store.counts().by_automation.get("auto-1"), Some(&1));
        assert_eq!(store.counts().by_worktree.get("/wt/a"), Some(&3));

        let forgotten = store.forget_under(&[dir.path().join("evidence")]);
        assert_eq!(forgotten, 1);
        assert_eq!(store.len(), 3);
    }

    /// 런타임이 탭·프로젝트·기간·파일 존재를 함께 거르고, 목록 상한 뒤의
    /// 탭 수와 사라진 파일 수도 센다. 창은 디스크에 묻지 않는다.
    #[test]
    fn listing_answers_the_gallerys_tabs_projects_periods_and_missing_files_past_the_cap() {
        let dir = tempfile::tempdir().expect("tempdir");
        let data = dir.path().join("data");
        let store = Store::open(
            &data,
            Limits {
                list_rows_max: 2,
                ..Limits::default()
            },
        );
        let limits = store.limits();
        let project = dir.path().join("acme");
        let worktree = dir.path().join("acme-wt");
        let file = |name: &str| {
            let path = dir.path().join("files").join(name);
            touch(&path, "x");
            path
        };
        let gone = dir.path().join("sessions/gone/0.png");
        let rows = [
            (
                "page",
                ArtifactKind::Page,
                file("site.html"),
                5_000,
                Origin {
                    project: Some(project.join("sub")),
                    agent: Some("claude".into()),
                    ..Origin::default()
                },
            ),
            (
                "doc",
                ArtifactKind::Document,
                file("plan.md"),
                1_000,
                Origin {
                    worktree: Some(worktree.clone()),
                    ..Origin::default()
                },
            ),
            (
                "web",
                ArtifactKind::Web,
                PathBuf::new(),
                4_000,
                Origin {
                    agent: Some("claude".into()),
                    ..Origin::default()
                },
            ),
            (
                "report",
                ArtifactKind::Report,
                file("report.md"),
                3_000,
                Origin {
                    worktree: Some(dir.path().join("elsewhere")),
                    agent: Some("codex".into()),
                    ..Origin::default()
                },
            ),
            (
                "shot-a",
                ArtifactKind::Screenshot,
                gone.clone(),
                6_000,
                Origin::default(),
            ),
            (
                "shot-b",
                ArtifactKind::Screenshot,
                gone.with_file_name("1.png"),
                2_000,
                Origin::default(),
            ),
            (
                "steps",
                ArtifactKind::Evidence,
                file("steps.jsonl"),
                7_000,
                Origin::default(),
            ),
        ];
        {
            let mut index = store.index.lock().unwrap();
            for (id, kind, path, modified_ms, origin) in rows {
                let artifact = Artifact {
                    id: id.into(),
                    kind,
                    title: id.into(),
                    path,
                    bytes: 1,
                    created_ms: modified_ms,
                    modified_ms,
                    url: (kind == ArtifactKind::Web).then(|| "https://example.com/a".into()),
                    favicon: None,
                    description: None,
                    version: None,
                    source_path: None,
                    feedback_count: None,
                    feedback_version: None,
                    origin,
                    tags: Vec::new(),
                    preview: Preview::default(),
                    source: Source::default(),
                };
                let row = Row {
                    artifact,
                    tokens: Vec::new(),
                    stamp: None,
                };
                store.insert_row(&mut index, row, &limits).unwrap();
            }
        }
        let words = |kinds: &[&str]| kinds.iter().map(|kind| (*kind).to_string()).collect();
        let ids = |listing: &Listing| {
            listing
                .rows
                .iter()
                .map(|row| row.id.clone())
                .collect::<Vec<_>>()
        };

        let pages = store.list(&Filter {
            kinds: words(&["page", "document", "web"]),
            present: Some(true),
            ..Filter::default()
        });
        assert_eq!((pages.total, pages.truncated), (3, true));
        assert_eq!(ids(&pages), ["page", "web"]);
        assert_eq!(pages.missing_total, 0);
        assert_eq!(
            pages.by_kind,
            BTreeMap::from(
                [
                    ("document", 1),
                    ("evidence", 1),
                    ("page", 1),
                    ("report", 1),
                    ("web", 1)
                ]
                .map(|(kind, n)| (kind.to_string(), n))
            ),
            "the tabs' counts are whole past the cap and leave the dead screenshots out"
        );

        let evidence = store.list(&Filter {
            kinds: words(&["screenshot", "evidence"]),
            present: Some(true),
            ..Filter::default()
        });
        assert_eq!(ids(&evidence), ["steps"]);
        assert_eq!(evidence.missing_total, 2);
        assert!(evidence.missing.is_empty());

        let shown = store.list(&Filter {
            kinds: words(&["screenshot", "evidence"]),
            ..Filter::default()
        });
        assert_eq!(
            (shown.total, ids(&shown)),
            (3, vec!["steps".into(), "shot-a".into()])
        );
        assert_eq!(shown.missing, ["shot-a"]);
        assert_eq!(shown.missing_total, 2);
        assert_eq!(
            store
                .list(&Filter {
                    present: Some(false),
                    ..Filter::default()
                })
                .total,
            2
        );

        let mine = store.list(&Filter {
            roots: vec![project.clone(), worktree.clone()],
            ..Filter::default()
        });
        assert_eq!(
            (mine.total, ids(&mine)),
            (2, vec!["page".into(), "doc".into()])
        );
        assert_eq!(
            store
                .list(&Filter {
                    roots: vec![dir.path().join("acme-w")],
                    ..Filter::default()
                })
                .total,
            0,
            "a root is a path, not a prefix of a name"
        );

        let recent = store.list(&Filter {
            kinds: words(&["page", "document", "web"]),
            since_ms: Some(4_000),
            ..Filter::default()
        });
        assert_eq!(ids(&recent), ["page", "web"]);
        let codex = store.list(&Filter {
            agent: Some("codex".into()),
            ..Filter::default()
        });
        assert_eq!(ids(&codex), ["report"]);
        assert_eq!(
            store
                .list(&Filter {
                    kinds: words(&["picture"]),
                    ..Filter::default()
                })
                .total,
            7,
            "an unknown kind word filtered rather than being ignored"
        );
    }

    /// 페이지 서랍은 현재 행 키의 썸네일을 읽는다. 아직 없다는 답은
    /// 기억하지 않으므로 카드가 그린 뒤 다시 물으면 그림을 찾는다.
    #[test]
    fn a_pages_preview_is_its_rendered_thumbnail_once_there_is_one() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = Store::open(&dir.path().join("data"), Limits::default());
        let project = dir.path().join("project");
        touch(&project.join("site.html"), "<title>Site</title>");
        let page = store
            .register_page(
                &PageFact {
                    path: project.join("site.html"),
                    at_ms: Some(1),
                    session: Some("s-1".into()),
                    project: Some(project.clone()),
                },
                "claude",
                1,
            )
            .unwrap()
            .expect("the created page is a row");
        let before = store.preview(&page.id).unwrap();
        assert_eq!((before.kind, before.data_url.is_none()), ("none", true));
        assert_eq!(store.preview_cache_bytes(), 0);
        let key = crate::artifact_thumbs::key_of(&page).expect("a page renders a thumbnail");
        crate::artifact_thumbs::remember(store.root(), &page.id, &key, b"\x89PNG").unwrap();
        let after = store.preview(&page.id).unwrap();
        assert_eq!(after.kind, "image");
        assert_eq!(
            after.data_url.as_deref(),
            Some(crate::artifact_thumbs::data_url_of(b"\x89PNG").as_str())
        );
        crate::artifact_thumbs::remember(store.root(), &page.id, "an older key", b"old").unwrap();
        assert_eq!(
            store.preview(&page.id).unwrap().kind,
            "none",
            "a picture under another key is not this page's"
        );
    }

    /// A claude.ai artifact is one row per url (t-3233 §2a): the same url
    /// twice is one row, a newer fact refreshes the title and time, an older
    /// one is ignored, and the row lists as `web` with its url and no file.
    #[test]
    fn a_remote_row_is_one_per_url_and_takes_the_newer_title() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = Store::open(&dir.path().join("data"), Limits::default());
        let fact = |title: &str, at_ms: i64| RemoteFact {
            url: "https://claude.ai/code/artifact/abc".into(),
            title: Some(title.into()),
            description: Some("한 장짜리 결정 문서 landed".into()),
            favicon: Some("🧱".into()),
            source_path: Some(PathBuf::from("/tmp/x.html")),
            at_ms: Some(at_ms),
            session: Some("s-1".into()),
            project: Some(PathBuf::from("/p")),
        };
        let first = store
            .register_remote(&fact("첫 제목", 1_000), "claude", 5_000)
            .expect("registers");
        assert_eq!(first.kind, ArtifactKind::Web);
        assert_eq!(
            first.url.as_deref(),
            Some("https://claude.ai/code/artifact/abc")
        );
        assert_eq!(first.path, PathBuf::new());
        assert_eq!(first.modified_ms, 1_000);
        assert_eq!(first.favicon.as_deref(), Some("🧱"));
        assert_eq!(first.source, Source::Remote);
        let again = store
            .register_remote(&fact("첫 제목", 1_000), "claude", 6_000)
            .expect("registers again");
        assert_eq!(again, first, "the same fact made a second row");
        assert_eq!(store.len(), 1);
        let older = store
            .register_remote(&fact("옛 제목", 500), "claude", 6_000)
            .expect("registers");
        assert_eq!(older.title, "첫 제목", "an older fact overwrote the row");
        let newer = store
            .register_remote(&fact("새 제목", 2_000), "claude", 6_000)
            .expect("registers");
        assert_eq!(newer.title, "새 제목");
        assert_eq!(newer.id, first.id);
        assert_eq!(
            newer.created_ms, 1_000,
            "the first sighting stays the birth"
        );
        assert_eq!(store.len(), 1);
        let lines = std::fs::read_to_string(
            dir.path()
                .join("data")
                .join(STORE_DIR_NAME)
                .join(INDEX_FILE),
        )
        .expect("index")
        .lines()
        .count();
        assert_eq!(lines, 2, "one line per change, none for the repeats");
        // Search reaches the title, the description and the url.
        for query in ["새 제목", "결정 문서", "artifact"] {
            let found = store.list(&Filter {
                query: query.into(),
                ..Filter::default()
            });
            assert_eq!(found.rows.len(), 1, "{query}: {found:?}");
        }
        assert_eq!(store.preview(&first.id).expect("preview").kind, "text");
        assert!(
            store
                .register_remote(
                    &RemoteFact {
                        url: "javascript:alert(1)".into(),
                        ..fact("x", 1)
                    },
                    "claude",
                    1
                )
                .is_err(),
            "only an https url is an artifact"
        );
        // Reopened, the url is still there.
        let reopened = Store::open(&dir.path().join("data"), Limits::default());
        assert_eq!(reopened.get(&first.id).and_then(|row| row.url), first.url);
    }

    /// A page is pointed at, never copied (t-3233 §2b): the same path twice
    /// is one row, a session past the table's count loses its oldest page,
    /// a file past the byte cap or not there is `None`, and the scan drops
    /// the row when the file goes.
    #[test]
    fn a_page_is_registered_in_place_bounded_per_session_and_gone_with_its_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let project = dir.path().join("project");
        let store = Store::open(
            &dir.path().join("data"),
            Limits {
                pages_per_session: 2,
                page_bytes_max: 64,
                ..Limits::default()
            },
        );
        let fact = |name: &str| PageFact {
            path: project.join(name),
            at_ms: Some(1_000),
            session: Some("s-1".into()),
            project: Some(project.clone()),
        };
        touch(
            &project.join("index.html"),
            "<!doctype html><title>a</title>",
        );
        let page = store
            .register_page(&fact("index.html"), "claude", 1_000)
            .expect("registers")
            .expect("a row");
        assert_eq!(page.kind, ArtifactKind::Page);
        assert_eq!(page.source, Source::AgentPage);
        assert_eq!(
            page.path,
            project.join("index.html").canonicalize().unwrap()
        );
        assert_eq!(page.origin.agent.as_deref(), Some("claude"));
        assert_eq!(page.origin.session.as_deref(), Some("s-1"));
        assert_eq!(page.origin.project, Some(project.canonicalize().unwrap()));
        assert!(
            !page.path.starts_with(dir.path().join("data")),
            "the page was copied into the store"
        );
        let again = store
            .register_page(&fact("index.html"), "claude", 2_000)
            .expect("registers")
            .expect("a row");
        assert_eq!(again.id, page.id);
        assert_eq!(store.len(), 1);
        assert_eq!(
            store.register_page(&fact("missing.html"), "claude", 1_000),
            Ok(None),
            "a file that is not there is not an error"
        );
        touch(&project.join("big.html"), &"x".repeat(100));
        assert_eq!(
            store.register_page(&fact("big.html"), "claude", 1_000),
            Ok(None)
        );
        touch(&project.join("main.rs"), "fn main() {}");
        assert_eq!(
            store.register_page(&fact("main.rs"), "claude", 1_000),
            Ok(None)
        );

        touch(
            &project.join("notes.md"),
            "# notes
",
        );
        touch(&project.join("paper.pdf"), "%PDF-1.4");
        let notes = store
            .register_page(&fact("notes.md"), "claude", 1_100)
            .expect("registers")
            .expect("a row");
        assert_eq!(notes.kind, ArtifactKind::Document);
        assert!(
            matches!(&notes.preview, Preview::Markdown { text } if text.starts_with("# notes"))
        );
        assert_eq!(store.preview(&notes.id).expect("preview").kind, "markdown");
        let pdf = store
            .register_page(&fact("paper.pdf"), "claude", 1_200)
            .expect("registers")
            .expect("a row");
        assert_eq!(pdf.kind, ArtifactKind::Document);
        assert_eq!(store.preview(&pdf.id).expect("preview").kind, "none");
        assert_eq!(store.len(), 2, "the session's oldest page gave its seat");
        assert!(
            store.get(&page.id).is_none(),
            "the oldest page kept its row"
        );

        std::fs::remove_file(project.join("notes.md")).expect("remove");
        let report = store.scan(3_000);
        assert_eq!(report.removed, 1, "{report:?}");
        assert_eq!(store.len(), 1);
        assert!(
            store.watch_targets().iter().any(|(_, path)| path.as_deref()
                == Some(project.join("paper.pdf").canonicalize().unwrap().as_path())),
            "the page's file does not ride the watcher's lane"
        );
    }

    /// A document that lives outside the open project opens from the gallery as
    /// a tab nobody can write, and its text comes from here, by id, out of the
    /// catalog's own row: the project's file door refuses any path outside its
    /// root and goes on refusing. This door answers for a text document row that
    /// is a regular file, cut at the preview's own byte cap; anything else is
    /// refused, and the refusal names the row (t-16006).
    #[test]
    fn a_document_row_is_read_by_id_within_the_previews_bounds_and_nothing_else_is() {
        let dir = tempfile::tempdir().expect("tempdir");
        let project = dir.path().join("project");
        let store = Store::open(
            &dir.path().join("data"),
            Limits {
                preview_text_bytes_max: 16,
                ..Limits::default()
            },
        );
        let register = |name: &str, at: i64| {
            let fact = PageFact {
                path: project.join(name),
                at_ms: Some(at),
                session: Some("s-1".into()),
                project: Some(project.clone()),
            };
            store
                .register_page(&fact, "claude", at)
                .expect("registers")
                .expect("a row")
        };

        touch(&project.join("short.md"), "# short\n");
        let short = register("short.md", 1_000);
        assert_eq!(short.kind, ArtifactKind::Document);
        assert_eq!(
            store.document_text(&short.id, None),
            Ok(DocumentText {
                text: "# short\n".into(),
                bytes: 8,
                truncated: false,
            })
        );

        // Longer than the cap: cut and said so, never refused — the person asked
        // to read it. The cut lands on a whole character.
        touch(&project.join("long.md"), &"a".repeat(40));
        let long = register("long.md", 1_100);
        let cut = store
            .document_text(&long.id, None)
            .expect("a long document is cut, not refused");
        assert_eq!((cut.text.len(), cut.bytes, cut.truncated), (16, 40, true));
        touch(&project.join("korean.md"), "가나다라마바");
        let korean = register("korean.md", 1_150);
        let cut = store
            .document_text(&korean.id, None)
            .expect("a Korean document is cut too");
        assert_eq!(
            (cut.text.as_str(), cut.bytes, cut.truncated),
            ("가나다라마", 18, true),
            "the cut must not leave half a character behind"
        );

        // A page is not a document, and neither is a worker's report; the
        // refusal says what the row is.
        touch(
            &project.join("index.html"),
            "<!doctype html><title>a</title>",
        );
        let page = register("index.html", 1_200);
        let refused = store
            .document_text(&page.id, None)
            .expect_err("a page is refused");
        assert!(
            refused.contains(&page.id) && refused.contains("page"),
            "{refused}"
        );
        let report_file = dir.path().join("t-9-report.md");
        touch(&report_file, "# report");
        let report = store
            .register_report(&report_file, origin("w-1"), 1_300)
            .expect("report");
        let refused = store
            .document_text(&report.id, None)
            .expect_err("a report is refused");
        assert!(refused.contains("report"), "{refused}");

        // A PDF is a document row and not text; binary bytes are not text either.
        touch(&project.join("paper.pdf"), "%PDF-1.4");
        let pdf = register("paper.pdf", 1_400);
        assert_eq!(pdf.kind, ArtifactKind::Document);
        let refused = store
            .document_text(&pdf.id, None)
            .expect_err("a PDF is refused");
        assert!(refused.contains(&pdf.id), "{refused}");
        std::fs::write(project.join("binary.md"), b"# a\0b").expect("write");
        let binary = register("binary.md", 1_500);
        assert_eq!(binary.kind, ArtifactKind::Document);
        assert!(
            store
                .document_text(&binary.id, None)
                .expect_err("binary bytes are refused")
                .contains("바이너리")
        );

        // Not a regular file: the row's path became a folder after it was
        // catalogued, and a folder is not read.
        std::fs::remove_file(project.join("short.md")).expect("remove");
        std::fs::create_dir(project.join("short.md")).expect("mkdir");
        assert!(
            store
                .document_text(&short.id, None)
                .expect_err("a folder is refused")
                .contains("일반 파일")
        );
        assert!(
            store
                .document_text("no-such-row", None)
                .expect_err("an unknown row is refused")
                .contains("no-such-row")
        );
        // The file went away while its row stayed: one sentence, not an OS error.
        std::fs::remove_file(project.join("long.md")).expect("remove");
        assert!(
            store
                .document_text(&long.id, None)
                .expect_err("a vanished file is refused")
                .contains("사라졌습니다")
        );

        // A kept version is read from the store's own list of them — the caller
        // names a number, never a path.
        touch(&project.join("history.md"), "first\n");
        let history = register("history.md", 2_000);
        touch(&project.join("history.md"), "second\n");
        register("history.md", 3_000);
        assert_eq!(store.versions(&history.id).len(), 2);
        assert_eq!(
            store
                .document_text(&history.id, Some(1))
                .expect("version 1")
                .text,
            "first\n"
        );
        assert_eq!(
            store
                .document_text(&history.id, None)
                .expect("the current file")
                .text,
            "second\n"
        );
        assert!(
            store
                .document_text(&history.id, Some(9))
                .expect_err("no such version")
                .contains("버전 9")
        );
    }

    #[cfg(unix)]
    #[test]
    fn page_registration_resolves_aliases_and_rejects_symlink_traversal() {
        let dir = tempfile::tempdir().expect("tempdir");
        let project = dir.path().join("project");
        touch(&project.join("docs/page.html"), "inside");
        touch(&dir.path().join("outside/page.html"), "outside");
        std::os::unix::fs::symlink(project.join("docs"), project.join("alias")).expect("alias");
        std::os::unix::fs::symlink(dir.path().join("outside"), project.join("escape"))
            .expect("escape");
        let store = Store::open(&dir.path().join("data"), Limits::default());
        let fact = |path: &str| PageFact {
            path: project.join(path),
            at_ms: None,
            session: None,
            project: Some(project.clone()),
        };
        let original = store
            .register_page(&fact("docs/page.html"), "zo", 1)
            .unwrap()
            .unwrap();
        let alias = store
            .register_page(&fact("alias/page.html"), "zo", 2)
            .unwrap()
            .unwrap();
        assert_eq!(alias.id, original.id, "one physical page is one row");
        assert_eq!(
            store.register_page(&fact("escape/page.html"), "zo", 3),
            Ok(None)
        );
        // Lexically this is project/outside/page.html, but resolving the
        // symlink before .. reaches the sibling outside/page.html.
        assert_eq!(
            store.register_page(&fact("escape/../outside/page.html"), "zo", 4),
            Ok(None)
        );
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn rescanning_pages_obeys_the_file_byte_and_page_limits() {
        for limits in [
            Limits {
                page_bytes_max: 4,
                ..Limits::default()
            },
            Limits {
                scan_bytes_max: 4,
                ..Limits::default()
            },
            Limits {
                scan_files_max: 1,
                ..Limits::default()
            },
        ] {
            let dir = tempfile::tempdir().expect("tempdir");
            let project = dir.path().join("project");
            let store = Store::open(&dir.path().join("data"), limits);
            let mut ids = Vec::new();
            for name in ["a.html", "b.md"] {
                let path = project.join(name);
                touch(&path, "v1");
                let row = store
                    .register_page(
                        &PageFact {
                            path: path.clone(),
                            project: Some(project.clone()),
                            at_ms: None,
                            session: None,
                        },
                        "zo",
                        1,
                    )
                    .unwrap()
                    .unwrap();
                ids.push(row.id);
                touch(&path, "v2222");
            }
            let report = store.scan(2);
            assert!(report.truncated, "{limits:?}: {report:?}");
            let new_versions: usize = ids.iter().map(|id| store.versions(id).len() - 1).sum();
            assert_eq!(
                new_versions,
                usize::from(limits.scan_files_max == 1),
                "{limits:?}: {report:?}"
            );
            if limits.scan_files_max == 1 {
                store.scan(3);
                assert!(
                    ids.iter().all(|id| store.versions(id).len() == 2),
                    "a bounded scan must reach the deferred page"
                );
            }
        }
    }

    /// Every change to a page keeps a version (t-3233 §5): V1 at registration,
    /// V2 when the scan sees the stamp move, bounded by the table, and the
    /// versions go with the row.
    #[test]
    fn a_page_keeps_its_versions_bounded_and_loses_them_with_the_row() {
        let dir = tempfile::tempdir().expect("tempdir");
        let project = dir.path().join("project");
        let store = Store::open(
            &dir.path().join("data"),
            Limits {
                versions_per_artifact_max: 2,
                ..Limits::default()
            },
        );
        let fact = PageFact {
            path: project.join("index.html"),
            at_ms: None,
            session: Some("s-1".into()),
            project: Some(project.clone()),
        };
        touch(&project.join("index.html"), "v1");
        let page = store
            .register_page(&fact, "zo", 1_000)
            .expect("registers")
            .expect("a row");
        let versions = store.versions(&page.id);
        assert_eq!(versions.len(), 1, "{versions:?}");
        assert_eq!(versions[0].n, 1);
        assert_eq!(
            std::fs::read_to_string(&versions[0].path).expect("v1"),
            "v1"
        );
        assert!(
            versions[0]
                .path
                .starts_with(store.root().join(VERSIONS_DIR_NAME))
        );
        std::thread::sleep(Duration::from_millis(20));
        touch(&project.join("index.html"), "v2 longer");
        let report = store.scan(2_000);
        assert_eq!(report.updated, 1, "{report:?}");
        let versions = store.versions(&page.id);
        assert_eq!(versions.iter().map(|v| v.n).collect::<Vec<_>>(), vec![1, 2]);
        assert_eq!(
            std::fs::read_to_string(&versions[1].path).expect("v2"),
            "v2 longer"
        );
        std::thread::sleep(Duration::from_millis(20));
        touch(&project.join("index.html"), "v3");
        store.scan(3_000);
        let versions = store.versions(&page.id);
        assert_eq!(
            versions.iter().map(|v| v.n).collect::<Vec<_>>(),
            vec![2, 3],
            "the oldest went first"
        );
        assert_eq!(
            store.scan(4_000).updated,
            0,
            "an unchanged page was re-versioned"
        );
        assert_eq!(store.versions(&page.id).len(), 2);
        assert!(store.delete(&page.id, true).expect("deletes"));
        assert!(store.versions(&page.id).is_empty());
        assert!(!store.root().join(VERSIONS_DIR_NAME).join(&page.id).exists());
        assert!(
            project.join("index.html").exists(),
            "the project's file was deleted"
        );
        assert!(store.versions("nobody").is_empty());
    }

    #[test]
    fn retention_removes_old_snapshots_of_a_still_active_page() {
        let dir = tempfile::tempdir().expect("tempdir");
        let project = dir.path().join("project");
        let store = Store::open(&dir.path().join("data"), Limits::default());
        let fact = PageFact {
            path: project.join("index.html"),
            at_ms: None,
            session: Some("s-1".into()),
            project: Some(project),
        };
        touch(&fact.path, "v1");
        let page = store
            .register_page(&fact, "zo", 1)
            .expect("register")
            .expect("row");
        let first = store.versions(&page.id).remove(0);
        std::fs::File::options()
            .write(true)
            .open(&first.path)
            .expect("snapshot")
            .set_times(std::fs::FileTimes::new().set_modified(SystemTime::UNIX_EPOCH))
            .expect("age snapshot");
        touch(&fact.path, "v2 longer");
        store.scan(2);
        store.prune(epoch_ms(Some(SystemTime::now())), 1);
        assert!(store.get(&page.id).is_some());
        assert!(!first.path.exists());
        assert_eq!(
            store
                .versions(&page.id)
                .iter()
                .map(|v| v.n)
                .collect::<Vec<_>>(),
            vec![2]
        );
        touch(&fact.path, "v3");
        store.scan(3);
        assert_eq!(
            store
                .versions(&page.id)
                .iter()
                .map(|v| v.n)
                .collect::<Vec<_>>(),
            vec![2, 3]
        );
    }

    #[test]
    fn page_removal_cleans_snapshots_and_thumbnails_on_every_catalog_exit() {
        for reason in ["missing", "session_cap", "index_cap"] {
            let dir = tempfile::tempdir().expect("tempdir");
            let project = dir.path().join("project");
            let store = Store::open(
                &dir.path().join("data"),
                Limits {
                    pages_per_session: if reason == "session_cap" { 1 } else { 10 },
                    index_rows_max: if reason == "index_cap" { 1 } else { 10 },
                    ..Limits::default()
                },
            );
            let mut fact = PageFact {
                path: project.join("first.html"),
                at_ms: None,
                session: Some("s-1".into()),
                project: Some(project.clone()),
            };
            touch(&fact.path, "first");
            let page = store
                .register_page(&fact, "zo", 1)
                .expect("register")
                .expect("row");
            let snapshot = store.versions(&page.id).remove(0).path;
            let thumbnail = store
                .root()
                .join(THUMBS_DIR_NAME)
                .join(format!("{}.png", page.id));
            touch(&thumbnail, "picture");
            if reason == "missing" {
                std::fs::remove_file(&fact.path).expect("remove source");
                store.scan(2);
            } else {
                fact.path = project.join("second.html");
                touch(&fact.path, "second");
                store.register_page(&fact, "zo", 2).expect("register");
            }
            assert!(store.get(&page.id).is_none(), "{reason}");
            assert!(!snapshot.exists(), "{reason}: orphan snapshot");
            assert!(!thumbnail.exists(), "{reason}: orphan thumbnail");
        }
    }

    /// 두 판을 발행한 페이지 하나와 그 id, 고칠 원본 — 피드백 시험의 바탕.
    fn published_twice(dir: &Path) -> (Store, String, PathBuf) {
        let store = Store::open(&dir.join("store"), Limits::default());
        let source = dir.join("card.html");
        touch(&source, "<main>one</main>");
        let request = serde_json::json!({"action":"publish", "file_path":source});
        let first = artifact_request(&store, request.clone(), &nobody).unwrap();
        touch(&source, "<main>two</main>");
        artifact_request(&store, request, &nobody).unwrap();
        let id = first["id"].as_str().unwrap().to_string();
        (store, id, source.canonicalize().unwrap())
    }

    fn feedback_ask(id: &str, version: u32, comment: &str) -> FeedbackAsk {
        FeedbackAsk {
            id: id.to_string(),
            version,
            items: vec![FeedbackItem {
                selector: ".pin".into(),
                comment: comment.into(),
            }],
            recipient: FeedbackRecipient {
                pane: "term-4".into(),
                agent: "claude".into(),
            },
            page_url: None,
        }
    }

    fn feedback_file(store: &Store, id: &str) -> PathBuf {
        store.root().join("pages").join(id).join(FEEDBACK_FILE)
    }

    #[test]
    fn a_preference_origin_requires_the_actual_recorded_feedback_and_version() {
        let directory = tempfile::tempdir().unwrap();
        let (store, id, _) = published_twice(directory.path());
        let ask = feedback_ask(&id, 1, "Keep headings concise");
        assert!(store.feedback_origin(&ask).is_err());
        store.record_feedback(ask.clone(), 10).unwrap();
        let origin = store.feedback_origin(&ask).unwrap();
        let recorded = std::fs::read_to_string(feedback_file(&store, &id)).unwrap();
        assert_eq!(
            origin.feedback_key,
            zerocode_core::user_preferences::content_key(recorded.trim_end().as_bytes())
        );
        assert_eq!(origin.version, 1);
        assert_eq!(
            origin.sha256,
            zerocode_core::artifact_publish::versions(store.root(), &id)[0].sha256
        );
        assert!(!origin.followed_link);
        assert!(
            store
                .feedback_origin(&feedback_ask(&id, 2, "Keep headings concise"))
                .is_err()
        );
        assert!(
            store
                .feedback_origin(&feedback_ask(&id, 1, "Unrecorded preference"))
                .is_err()
        );
        let mut followed = ask;
        followed.page_url = Some("https://example.com/review".into());
        store.record_feedback(followed.clone(), 20).unwrap();
        assert!(store.feedback_origin(&followed).unwrap().followed_link);
        let exposed = serde_json::to_string(&origin).unwrap();
        assert!(!exposed.contains(&directory.path().to_string_lossy().into_owned()));
    }

    /// 전달된 주석은 그 페이지의 `feedback.jsonl`에 한 줄씩 덧붙는다. 줄은 판의
    /// 번호와 그 판의 SHA, 고칠 원본, 받은 판과 에이전트, 주석을 적고, SHA와 원본은
    /// 창이 아니라 스토어가 적는다. 틀린 청은 파일을 한 바이트도 바꾸지 않고
    /// 거절되며, 상한에 닿은 기록은 더 받지 않는다.
    #[test]
    fn delivered_feedback_is_validated_bounded_and_appended_line_by_line() {
        let dir = tempfile::tempdir().unwrap();
        let (store, id, source) = published_twice(dir.path());
        let file = feedback_file(&store, &id);

        let one = store
            .record_feedback(feedback_ask(&id, 1, "핀 번호가 글자를 가립니다"), 10)
            .unwrap();
        assert_eq!(
            one,
            FeedbackSummary {
                count: 1,
                version: Some(1)
            }
        );
        let after_one = std::fs::read(&file).unwrap();
        let two = store
            .record_feedback(feedback_ask(&id, 2, "답하기 단추를 조금 더 크게"), 20)
            .unwrap();
        assert_eq!(
            two,
            FeedbackSummary {
                count: 2,
                version: Some(2)
            }
        );
        let after_two = std::fs::read(&file).unwrap();
        assert!(
            after_two.starts_with(&after_one),
            "the second delivery rewrote the first line"
        );
        let text = String::from_utf8(after_two.clone()).unwrap();
        let lines: Vec<FeedbackLine> = text
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let kept = zerocode_core::artifact_publish::versions(store.root(), &id);
        assert_eq!(lines.len(), 2);
        assert_eq!((lines[0].id.as_str(), lines[0].version), (id.as_str(), 1));
        assert_eq!(lines[0].sha256, kept[0].sha256);
        assert!(lines[0].sha256.is_some());
        assert_eq!(lines[1].sha256, kept[1].sha256);
        assert_ne!(lines[0].sha256, lines[1].sha256);
        assert_eq!(lines[0].source_path.as_deref(), Some(source.as_path()));
        assert_eq!(
            lines[0].recipient,
            FeedbackRecipient {
                pane: "term-4".into(),
                agent: "claude".into()
            }
        );
        assert_eq!(lines[0].items[0].comment, "핀 번호가 글자를 가립니다");
        assert_eq!((lines[0].at_ms, lines[1].at_ms), (10, 20));

        let report = dir.path().join("report.md");
        touch(&report, "# report");
        let copied = store
            .register_copy(&report, Source::Manual, Origin::default(), 1)
            .unwrap();
        let with = |edit: &dyn Fn(&mut FeedbackAsk)| {
            let mut ask = feedback_ask(&id, 2, "좋습니다");
            edit(&mut ask);
            ask
        };
        let refused: Vec<(&str, FeedbackAsk)> = vec![
            ("an unknown id", with(&|ask| ask.id = "p-nothere".into())),
            (
                "a row that is no publication",
                with(&|ask| ask.id.clone_from(&copied.id)),
            ),
            ("version zero", with(&|ask| ask.version = 0)),
            ("a version not yet published", with(&|ask| ask.version = 3)),
            ("no items", with(&|ask| ask.items.clear())),
            (
                "too many items",
                with(&|ask| ask.items = vec![ask.items[0].clone(); FEEDBACK_ITEMS_MAX + 1]),
            ),
            (
                "a comment past its bound",
                with(&|ask| ask.items[0].comment = "가".repeat(FEEDBACK_COMMENT_MAX + 1)),
            ),
            (
                "a selector past its bound",
                with(&|ask| ask.items[0].selector = "a".repeat(FEEDBACK_SELECTOR_MAX + 1)),
            ),
            (
                "a terminal escape in a comment",
                with(&|ask| ask.items[0].comment = "\u{1b}[2J 지워라".into()),
            ),
            (
                "an item that says nothing",
                with(&|ask| {
                    ask.items[0] = FeedbackItem {
                        selector: String::new(),
                        comment: "  ".into(),
                    };
                }),
            ),
            (
                "a pane key of another shape",
                with(&|ask| ask.recipient.pane = "../term-4".into()),
            ),
            (
                "a pane key without a number",
                with(&|ask| ask.recipient.pane = "term-".into()),
            ),
            (
                "an agent id with words in it",
                with(&|ask| ask.recipient.agent = "claude; rm".into()),
            ),
            ("no agent", with(&|ask| ask.recipient.agent = String::new())),
            (
                "an agent id past its bound",
                with(&|ask| ask.recipient.agent = "a".repeat(FEEDBACK_AGENT_MAX + 1)),
            ),
        ];
        for (why, ask) in refused {
            assert!(store.record_feedback(ask, 30).is_err(), "took {why}");
        }
        assert_eq!(std::fs::read(&file).unwrap(), after_two, "a refusal wrote");
        assert!(
            !store.root().join("pages").join(&copied.id).exists(),
            "a refusal made a page seat"
        );
        let unknown_field = serde_json::from_value::<FeedbackAsk>(serde_json::json!({
            "id": id, "version": 1, "submit": true,
            "items": [{"selector": ".pin", "comment": "x"}],
            "recipient": {"pane": "term-4", "agent": "claude"},
        }));
        assert!(
            unknown_field.is_err(),
            "the ask took a field it does not know"
        );

        // 상한에 닿은 기록: 더 받지 않고, 있던 것은 그대로다.
        let mut full = after_two.clone();
        let filler = usize::try_from(FEEDBACK_FILE_MAX_BYTES).unwrap() - full.len() - 64;
        full.extend(std::iter::repeat_n(b'x', filler));
        full.push(b'\n');
        std::fs::write(&file, &full).unwrap();
        assert!(
            store
                .record_feedback(feedback_ask(&id, 2, "하나 더"), 40)
                .is_err()
        );
        assert_eq!(std::fs::read(&file).unwrap(), full);
        assert_eq!(store.feedback_summary(&id).count, 2);
    }

    /// 창이 보내는 그대로의 청(t-14586): 링크를 따라간 페이지에 단 주석은 떠난 판의
    /// 번호에 그 페이지의 주소(`page_url`)를 붙여 기록되고, 그 페이지의 수에 든다.
    /// R3가 쓴 줄(주소 없음)도 함께 세이고, 판에서 단 주석의 줄은 주소를 쓰지 않는다.
    /// 청은 여전히 모르는 필드를 받지 않는다. 창이 쓰는 입구(JSON)로만 청한다.
    #[test]
    fn an_annotation_on_a_followed_page_is_recorded_with_its_address() {
        let dir = tempfile::tempdir().unwrap();
        let (store, id, _) = published_twice(dir.path());
        let file = feedback_file(&store, &id);
        let old = format!(
            r#"{{"id":"{id}","version":1,"sha256":null,"source_path":null,"recipient":{{"pane":"term-2","agent":"claude"}},"items":[{{"selector":".pin","comment":"옛 줄"}}],"at_ms":3}}"#
        );
        std::fs::write(&file, format!("{old}\n")).unwrap();
        let followed = "file:///tmp/zerocode-test/notes/plain.html";
        let ask = |extra: serde_json::Value| {
            let mut body = serde_json::json!({
                "id": id, "version": 2,
                "items": [{"selector": "main h1", "comment": "따라간 노트의 표가 넘칩니다"}],
                "recipient": {"pane": "term-4", "agent": "claude"},
            });
            body.as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            serde_json::from_value::<FeedbackAsk>(body)
        };

        let asked = ask(serde_json::json!({"page_url": followed}));
        assert!(
            asked.is_ok(),
            "the ask refused a followed page's address: {asked:?}"
        );
        let recorded = asked
            .map_err(|error| error.to_string())
            .and_then(|ask| store.record_feedback(ask, 7));
        assert_eq!(
            recorded,
            Ok(FeedbackSummary {
                count: 2,
                version: Some(2)
            })
        );
        let on_version = ask(serde_json::json!({})).unwrap();
        assert_eq!(store.record_feedback(on_version, 8).unwrap().count, 3);

        let text = std::fs::read_to_string(&file).unwrap();
        let lines: Vec<serde_json::Value> = text
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(lines.len(), 3, "{text}");
        assert_eq!(lines[0].get("page_url"), None);
        assert_eq!(lines[1]["page_url"], followed);
        assert_eq!(lines[1]["version"], 2);
        assert_eq!(lines[1]["items"][0]["selector"], "main h1");
        assert_eq!(
            lines[2].get("page_url"),
            None,
            "a delivery on the version wrote an address"
        );
        assert_eq!(
            store.list(&Filter::default()).rows[0].feedback_count,
            Some(3)
        );
        assert!(
            ask(serde_json::json!({"page": followed})).is_err(),
            "the ask took a field it does not know"
        );
    }

    /// 링크를 따라간 페이지에 단 주석(t-14586): 줄은 떠난 판의 번호와 그 페이지의
    /// 주소를 함께 적고, 주소는 다른 문자열처럼 길이와 제어 문자로 막힌다. 주소가
    /// 없던 빌드의 줄은 그대로 읽혀 함께 세이고, 주소 없는 전달은 그 필드를 쓰지
    /// 않는다. 청은 여전히 모르는 필드를 받지 않는다.
    #[test]
    fn a_followed_page_address_is_recorded_bounded_and_old_lines_are_still_read() {
        let dir = tempfile::tempdir().unwrap();
        let (store, id, _) = published_twice(dir.path());
        let file = feedback_file(&store, &id);
        // R3가 쓴 모양 그대로의 줄 — `page_url`이 없다.
        let old = format!(
            r#"{{"id":"{id}","version":1,"sha256":null,"source_path":null,"recipient":{{"pane":"term-2","agent":"claude"}},"items":[{{"selector":".pin","comment":"옛 줄"}}],"at_ms":3}}"#
        );
        std::fs::write(&file, format!("{old}\n")).unwrap();
        assert_eq!(
            store.feedback_summary(&id),
            FeedbackSummary {
                count: 1,
                version: Some(1)
            }
        );

        let followed = "file:///tmp/zerocode-test/notes/plain.html";
        let mut ask = feedback_ask(&id, 2, "따라간 페이지의 제목이 잘립니다");
        ask.page_url = Some(followed.into());
        let summary = store.record_feedback(ask, 7).unwrap();
        assert_eq!(
            summary,
            FeedbackSummary {
                count: 2,
                version: Some(2)
            }
        );
        let plain = store
            .record_feedback(feedback_ask(&id, 2, "판에서 단 주석"), 8)
            .unwrap();
        assert_eq!(plain.count, 3);
        let text = std::fs::read_to_string(&file).unwrap();
        let raw: Vec<&str> = text.lines().collect();
        assert_eq!(raw.len(), 3, "{text}");
        let lines: Vec<FeedbackLine> = raw
            .iter()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(lines[0].page_url, None);
        assert_eq!(lines[1].page_url.as_deref(), Some(followed));
        assert_eq!(lines[1].version, 2);
        assert!(lines[1].sha256.is_some());
        assert_eq!(lines[2].page_url, None);
        assert!(
            !raw[2].contains("page_url"),
            "a delivery on the version itself wrote the followed-page field: {}",
            raw[2]
        );
        let listed = store.list(&Filter::default());
        assert_eq!(listed.rows[0].feedback_count, Some(3));

        let with_url = |url: String| {
            let mut ask = feedback_ask(&id, 2, "좋습니다");
            ask.page_url = Some(url);
            ask
        };
        let at_bound = format!(
            "https://example.com/{}",
            "a".repeat(FEEDBACK_PAGE_URL_MAX - "https://example.com/".len())
        );
        assert_eq!(at_bound.chars().count(), FEEDBACK_PAGE_URL_MAX);
        let before = std::fs::read(&file).unwrap();
        let refused = [
            ("an empty address", String::new()),
            ("a blank address", "   ".to_string()),
            ("an address past its bound", format!("{at_bound}a")),
            (
                "a line break in the address",
                "https://example.com/\nx".into(),
            ),
            (
                "a terminal escape in the address",
                "https://example.com/\u{1b}[2J".into(),
            ),
        ];
        for (why, url) in refused {
            assert!(
                store.record_feedback(with_url(url), 9).is_err(),
                "took {why}"
            );
        }
        assert_eq!(std::fs::read(&file).unwrap(), before, "a refusal wrote");
        assert_eq!(
            store.record_feedback(with_url(at_bound), 9).unwrap().count,
            4
        );

        let asked = serde_json::from_value::<FeedbackAsk>(serde_json::json!({
            "id": id, "version": 1, "page_url": followed,
            "items": [{"selector": ".pin", "comment": "x"}],
            "recipient": {"pane": "term-4", "agent": "claude"},
        }))
        .unwrap();
        assert_eq!(asked.page_url.as_deref(), Some(followed));
        let unknown_field = serde_json::from_value::<FeedbackAsk>(serde_json::json!({
            "id": id, "version": 1, "page": followed,
            "items": [{"selector": ".pin", "comment": "x"}],
            "recipient": {"pane": "term-4", "agent": "claude"},
        }));
        assert!(
            unknown_field.is_err(),
            "the ask took a field it does not know"
        );
    }

    /// 한 줄이 깨져도 기록은 선다: 읽히지 않는 줄과 다른 페이지를 말하는 줄은 세지
    /// 않고, 끝이 잘린 줄 뒤의 다음 전달은 제 줄에서 시작한다 — 깨진 조각은 지우지
    /// 않는다(덧붙이기만).
    #[test]
    fn a_corrupt_feedback_line_is_skipped_and_the_next_delivery_starts_its_own_line() {
        let dir = tempfile::tempdir().unwrap();
        let (store, id, _) = published_twice(dir.path());
        let file = feedback_file(&store, &id);
        let stranger = serde_json::to_string(&FeedbackLine {
            id: "p-other".into(),
            version: 1,
            sha256: None,
            source_path: None,
            recipient: FeedbackRecipient {
                pane: "term-1".into(),
                agent: "codex".into(),
            },
            items: Vec::new(),
            at_ms: 1,
            page_url: None,
        })
        .unwrap();
        std::fs::write(&file, format!("not json\n{stranger}\n{{\"id\":")).unwrap();
        assert_eq!(store.feedback_summary(&id), FeedbackSummary::default());
        let listed = store.list(&Filter::default());
        assert_eq!(listed.rows[0].feedback_count, Some(0));
        assert_eq!(listed.rows[0].feedback_version, None);

        let summary = store
            .record_feedback(feedback_ask(&id, 1, "색이 너무 옅습니다"), 5)
            .unwrap();
        assert_eq!(
            summary,
            FeedbackSummary {
                count: 1,
                version: Some(1)
            }
        );
        let text = std::fs::read_to_string(&file).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 4, "{text}");
        assert_eq!(lines[..3], ["not json", stranger.as_str(), "{\"id\":"]);
        let parsed: FeedbackLine = serde_json::from_str(lines[3]).unwrap();
        assert_eq!(
            (parsed.version, parsed.items[0].comment.as_str()),
            (1, "색이 너무 옅습니다")
        );
        assert!(text.ends_with('\n'));
    }

    /// 기록 파일은 판이 아니다: 버전 목록, 주소 판정, 문의 내보내기와 목록은 그 파일이
    /// 없던 때와 같게 답하고, 목록의 발행물 행만 기록의 수와 판을 입는다 — 그 수는
    /// 카탈로그에 적히지 않고 다시 열어도 파일에서 다시 센다. 창의 내보내기는 고른
    /// 폴더에 그 판을 새 파일로 쓰고, 있는 파일은 덮지 않는다.
    #[test]
    fn the_feedback_file_is_no_version_and_leaves_listing_versions_and_export_alone() {
        let dir = tempfile::tempdir().unwrap();
        let (store, id, _) = published_twice(dir.path());
        let report = dir.path().join("report.md");
        touch(&report, "# report");
        let copied = store
            .register_copy(&report, Source::Manual, Origin::default(), 1)
            .unwrap();
        store
            .record_feedback(feedback_ask(&id, 1, "첫 판에"), 1)
            .unwrap();
        store
            .record_feedback(feedback_ask(&id, 2, "둘째 판에"), 2)
            .unwrap();
        let file = feedback_file(&store, &id);
        assert!(file.is_file());

        let numbers = |kept: Vec<u32>| kept;
        assert_eq!(
            numbers(store.versions(&id).iter().map(|one| one.n).collect()),
            [1, 2]
        );
        assert_eq!(
            numbers(
                zerocode_core::artifact_publish::versions(store.root(), &id)
                    .iter()
                    .map(|one| one.n)
                    .collect()
            ),
            [1, 2]
        );
        assert_eq!(store.page_at(&file), None);
        let current = store.get(&id).unwrap().path;
        let at = store.page_at(&current).unwrap();
        assert_eq!(
            (at.artifact.feedback_count, at.artifact.feedback_version),
            (Some(2), Some(2))
        );

        let listing = store.list(&Filter::default());
        assert_eq!(listing.total, 2);
        let page = listing.rows.iter().find(|row| row.id == id).unwrap();
        assert_eq!(page.version, Some(2));
        assert_eq!(
            (page.feedback_count, page.feedback_version),
            (Some(2), Some(2))
        );
        let other = listing.rows.iter().find(|row| row.id == copied.id).unwrap();
        assert_eq!((other.feedback_count, other.feedback_version), (None, None));
        let door = artifact_request(&store, serde_json::json!({"action":"list"}), &nobody).unwrap();
        assert_eq!(door["total"], 1);
        let catalog = std::fs::read_to_string(store.root().join(INDEX_FILE)).unwrap();
        assert!(
            !catalog.contains("feedback"),
            "the count was written down: {catalog}"
        );

        let snapshot =
            std::fs::read(store.root().join("pages").join(&id).join("v1/index.html")).unwrap();
        let out = dir.path().join("door.html");
        artifact_request(
            &store,
            serde_json::json!({"action":"export", "id":id, "version":1, "out":out}),
            &nobody,
        )
        .unwrap();
        assert_eq!(std::fs::read(&out).unwrap(), snapshot);

        let folder = dir.path().join("shared");
        std::fs::create_dir_all(&folder).unwrap();
        let first = store
            .export_into(&id, 1, &folder, ExportFormat::Html)
            .unwrap();
        let second = store
            .export_into(&id, 1, &folder, ExportFormat::Html)
            .unwrap();
        assert_eq!(first.path.file_name().unwrap(), "card-v1.html");
        assert_eq!(second.path.file_name().unwrap(), "card-v1 (2).html");
        assert_eq!(std::fs::read(&first.path).unwrap(), snapshot);
        assert_eq!(std::fs::read(&second.path).unwrap(), snapshot);
        assert_eq!((first.version, second.version), (1, 1));
        assert!(
            store
                .export_into(&id, 1, Path::new("shared"), ExportFormat::Html)
                .is_err()
        );
        assert!(
            store
                .export_into(&id, 1, store.root(), ExportFormat::Html)
                .is_err()
        );
        assert!(
            store
                .export_into(&id, 9, &folder, ExportFormat::Html)
                .is_err()
        );
        assert!(
            store
                .export_into(&copied.id, 1, &folder, ExportFormat::Html)
                .is_err()
        );
        assert_eq!(std::fs::read_dir(&folder).unwrap().count(), 2);

        let reopened = Store::open(&dir.path().join("store"), Limits::default());
        let again = reopened.list(&Filter::default());
        let page = again.rows.iter().find(|row| row.id == id).unwrap();
        assert_eq!(
            (page.feedback_count, page.feedback_version),
            (Some(2), Some(2))
        );
        assert!(reopened.delete(&id, true).unwrap());
        assert!(!file.exists());
    }
}
