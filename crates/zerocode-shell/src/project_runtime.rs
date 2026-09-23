use super::*;

use zerocode_core::pick::{HELPER_NAME, PickRequest};

/* ---- the folder panel's clock, in one table -----------------------------
 *
 * 2026-09-05 02:49:27 the sidebar's 다른 폴더 열기 opened its panel — the
 * system log has AppKit's `openAndSavePanelService` coming up under this
 * window's pid at that second — and at 02:55:14 the Dock recorded a force
 * quit of the same pid, after five minutes in which the window's own log
 * wrote nothing at all (t-2488). Nothing, because this road wrote nothing:
 * not that it asked, not that the panel stood, not that it answered. The
 * first job of everything below is that a folder panel now leaves a trace.
 *
 * The trace then showed what the hang was (09-06 17:15 "reached the main
 * thread after 1,096 ms"; 09-07 11:38 "did not reach the panel's task
 * within 2,000 ms" → the watchdog's `hang 2,027 ms` → loginwindow's AutoFill
 * shield for the panel service → force quit). `NSOpenPanel` is a remote
 * view: the thread that opens it waits synchronously for
 * `openAndSavePanelService`, and that thread was the window's main thread.
 * So the panel is not opened in this process any more (t-2982): the default
 * door is the native picker; the in-window browser remains a fallback. The native
 * panel is the `zerocode-pick` helper's, in its own process, spawned and
 * awaited by `pick_paths` on the async runtime. The main thread is free —
 * which is what lets the overdue toast actually appear.
 *
 * What stays from the sheet era: one panel at a time (a second ask brings
 * the helper forward by pid instead of opening another), the receipt behind
 * the ask on the main thread (the marker guard — now, if it misses its
 * budget, that is another bug and not this road), and the overdue event.
 *
 * Every number the guard and the browser use lives here and nowhere else. */

/// How long a folder panel may stand unanswered before the window is told
/// it might not be seeing it. A person browsing a few folders deep takes
/// seconds; a sheet attached to a window that is minimized or behind
/// another app takes forever, and forever used to look like a hang.
pub(super) const FOLDER_PANEL_OVERDUE: Duration = Duration::from_secs(8);

/// How long the receipt posted to the main thread right behind the ask may
/// take to come back. The ask itself no longer touches that thread, so the
/// receipt now measures only how busy the window's main thread is — past
/// this it is written down as a stall, and a stall here is some other bug.
pub(super) const FOLDER_PANEL_MAIN_THREAD_BUDGET: Duration = Duration::from_secs(2);
/// How long after the helper is spawned the window brings it forward once on
/// its own: the panel opens in another process, so the person's active
/// application (this window) must yield activation to it, and other
/// applications' windows may otherwise stay on top of the panel
/// (2026-09-23, the iPhone Mirroring window covered its buttons). Long
/// enough for the helper to have opened its panel, short enough that the
/// person never looks for it.
pub(super) const FOLDER_PANEL_RAISE_DELAY: Duration = Duration::from_millis(350);

/// How long the helper's panel may stand before the window kills it and
/// answers "no folder". A panel nobody answered in a quarter of an hour is
/// a panel nobody is looking at, and a helper process that stands forever
/// is a Dock full of them after a week.
pub(super) const FOLDER_PANEL_HELPER_WAIT: Duration = Duration::from_secs(15 * 60);

/// A standing panel older than this is presumed lost, and the next ask opens
/// a fresh one instead of pointing at it. The helper is killed at
/// [`FOLDER_PANEL_HELPER_WAIT`] and answers then; this is the desk's own
/// floor under a road that never came back at all.
pub(super) const FOLDER_PANEL_PRESUMED_LOST: Duration = Duration::from_secs(15 * 60);

/// The event the window hears when its panel has stood past
/// [`FOLDER_PANEL_OVERDUE`]. The window answers with a toast that carries
/// the recall door.
pub(super) const FOLDER_PANEL_OVERDUE_EVENT: &str = "project:folder-panel-overdue";

/// What a second asker is told while a panel stands. Korean, like every
/// other refusal this backend speaks; the window shows it as it is.
pub(super) const FOLDER_PANEL_STANDING: &str =
    "파일·폴더 선택 창이 이미 열려 있어 앞으로 가져왔습니다";

/// Entries one answer of the in-window browser carries at most (t-2982). A
/// folder of a hundred thousand files is a folder nobody picks by scrolling;
/// the answer is cut here, says it was, and the person types the rest of the
/// path. The window's harness opens a folder of exactly this many rows and
/// pins the open under its budget.
pub(super) const FOLDER_BROWSE_ENTRY_CAP: usize = 5_000;

/// Recent projects the browser's start view offers, newest first. The
/// sidebar shows the whole catalog; the browser's start view is a handful of
/// likely answers above the home folder and the volumes.
pub(super) const FOLDER_BROWSE_RECENT_CAP: usize = 8;

// The start view is a handful of rows; one folder is thousands. A table whose
// recents outnumbered a folder's entries would be a table nobody read —
// refused at compile time, beside the rows it reads.
const _: () =
    assert!(FOLDER_BROWSE_RECENT_CAP > 0 && FOLDER_BROWSE_RECENT_CAP < FOLDER_BROWSE_ENTRY_CAP);

/// What an ask found on the desk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FolderPanelAsk {
    /// No panel stands: the caller opens one and owns its answer. The
    /// generation is the receipt the answer must carry back.
    Fresh { generation: u64 },
    /// One already stands, this long, and this many times somebody has asked
    /// for it since. The caller brings the window forward and does NOT open
    /// a second panel — AppKit would queue it behind the first, unseen.
    Standing { since: Duration, recalls: u32 },
}

/// What the desk hands back when a panel answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct FolderPanelReceipt {
    pub(super) stood_for: Duration,
    pub(super) recalls: u32,
}

/// The helper process behind a standing panel: its pid, to bring it
/// forward, and the one word that kills it.
#[derive(Debug)]
pub(super) struct HelperHandle {
    pid: u32,
    cancel: Option<tokio::sync::oneshot::Sender<()>>,
}

impl HelperHandle {
    pub(super) fn new(pid: u32, cancel: tokio::sync::oneshot::Sender<()>) -> Self {
        Self {
            pid,
            cancel: Some(cancel),
        }
    }
}

#[derive(Debug)]
struct StandingPanel {
    generation: u64,
    asked_at: Instant,
    recalls: u32,
    helper: Option<HelperHandle>,
    cancel_requested: bool,
}

/// One folder panel at a time, and who asked for it while it stood.
///
/// Pure bookkeeping with the clock passed in, so the rules can be tested
/// without a window: a fresh ask, a recall, an answer, a late answer from a
/// panel already presumed lost.
#[derive(Debug)]
pub(super) struct FolderPanelDesk {
    standing: Option<StandingPanel>,
    generations: u64,
}

impl FolderPanelDesk {
    pub(super) const fn new() -> Self {
        Self {
            standing: None,
            generations: 0,
        }
    }

    /// Somebody wants a folder. Either nothing stands and they open a panel,
    /// or one stands and they are pointed at it.
    pub(super) fn ask(&mut self, now: Instant) -> FolderPanelAsk {
        if let Some(panel) = self.standing.as_mut() {
            let since = now.saturating_duration_since(panel.asked_at);
            if since < FOLDER_PANEL_PRESUMED_LOST {
                panel.recalls += 1;
                return FolderPanelAsk::Standing {
                    since,
                    recalls: panel.recalls,
                };
            }
        }
        self.generations += 1;
        let generation = self.generations;
        self.standing = Some(StandingPanel {
            generation,
            asked_at: now,
            recalls: 0,
            helper: None,
            cancel_requested: false,
        });
        FolderPanelAsk::Fresh { generation }
    }

    /// The helper of this generation was spawned. Ignored for a generation
    /// that no longer stands — its road will be cancelled by the dropped
    /// sender, which is the right end for a helper nobody is waiting on.
    pub(super) fn attach_helper(&mut self, generation: u64, mut helper: HelperHandle) {
        if let Some(panel) = self
            .standing
            .as_mut()
            .filter(|panel| panel.generation == generation)
        {
            if panel.cancel_requested
                && let Some(cancel) = helper.cancel.take()
            {
                let _ = cancel.send(());
            }
            panel.helper = Some(helper);
        }
    }

    /// The pid of the helper standing now, if one stands and is not
    /// presumed lost.
    pub(super) fn helper_pid(&self, now: Instant) -> Option<u32> {
        self.standing_for(now)?;
        self.standing
            .as_ref()?
            .helper
            .as_ref()
            .map(|helper| helper.pid)
    }

    /// Kill the standing helper. Answers whether there was one to kill; the
    /// road answers `Cancelled` and clears the desk itself.
    pub(super) fn cancel_helper(&mut self) -> bool {
        let Some(panel) = self.standing.as_mut() else {
            return false;
        };
        if panel.cancel_requested {
            return false;
        }
        panel.cancel_requested = true;
        match panel.helper.as_mut() {
            Some(helper) => helper
                .cancel
                .take()
                .is_some_and(|cancel| cancel.send(()).is_ok()),
            None => true, // Delivered when this generation's helper attaches.
        }
    }

    /// The ask of this generation never got as far as a panel — no helper to
    /// spawn — so nothing stands.
    pub(super) fn abandon(&mut self, generation: u64) {
        if self
            .standing
            .as_ref()
            .is_some_and(|panel| panel.generation == generation)
        {
            self.standing = None;
        }
    }

    /// The panel of this generation answered — a path or a dismissal, the
    /// desk does not care which. `None` when it was not the panel standing:
    /// nothing stood, or a later ask had already presumed it lost and opened
    /// another, which this must not knock down.
    pub(super) fn answered(&mut self, generation: u64, now: Instant) -> Option<FolderPanelReceipt> {
        let standing = self.standing.as_ref()?;
        if standing.generation != generation {
            return None;
        }
        let receipt = FolderPanelReceipt {
            stood_for: now.saturating_duration_since(standing.asked_at),
            recalls: standing.recalls,
        };
        self.standing = None;
        Some(receipt)
    }

    /// How long the standing panel has stood, if one does and it is not yet
    /// presumed lost.
    pub(super) fn standing_for(&self, now: Instant) -> Option<Duration> {
        let since = now.saturating_duration_since(self.standing.as_ref()?.asked_at);
        (since < FOLDER_PANEL_PRESUMED_LOST).then_some(since)
    }
}

static FOLDER_PANEL_DESK: std::sync::Mutex<FolderPanelDesk> =
    std::sync::Mutex::new(FolderPanelDesk::new());

/// The one desk. A poisoned lock is still a desk — the bookkeeping inside is
/// two plain fields and cannot be half-written.
pub(super) fn folder_panel_desk() -> std::sync::MutexGuard<'static, FolderPanelDesk> {
    FOLDER_PANEL_DESK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// What the window is told when the panel has stood past the bound.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(super) struct FolderPanelOverdue {
    /// How long the panel had stood when this was sent.
    pub(super) after_ms: u64,
    /// Whether the window was brought forward as well: it is when it was
    /// minimized or not visible, and left alone when it was already in
    /// front — a person browsing the panel must not have focus yanked.
    pub(super) brought_forward: bool,
    /// How long the panel's task waited for the main thread, when the
    /// receipt came back within its budget.
    pub(super) main_thread_wait_ms: Option<u64>,
}

fn note_folder_panel(app: &AppHandle, line: &str) {
    note_window_event(
        app.state::<AppState>().local_data_root(),
        &format!("folder panel: {line}"),
    );
}

/// Whether the main window is somewhere a person could see a sheet on it.
/// `None` when the window is gone.
fn main_window_in_front(app: &AppHandle) -> Option<bool> {
    let window = app.get_webview_window(MAIN_WINDOW_LABEL)?;
    let minimized = window.is_minimized().unwrap_or(false);
    let visible = window.is_visible().unwrap_or(true);
    Some(!minimized && visible)
}

/// Ask the operating system for paths, guarded — the ONE seat every native
/// picker in this window rides (project folder, Warp theme files and folder,
/// cookie file, emulator app).
///
/// The panel is the `zerocode-pick` helper's, in its own process
/// ([`run_pick_helper`]); this awaits it on tauri's runtime and never touches
/// the thread that paints the window. What this adds is the clock above:
///
///   1. one panel at a time — a second ask brings the helper forward by pid
///      (or the window, when the helper has not said its pid yet) and
///      refuses with [`FOLDER_PANEL_STANDING`] rather than opening another;
///   2. a receipt behind the ask on the main thread, so a busy main thread
///      is measured and a stall is written down — the marker guard;
///   3. past [`FOLDER_PANEL_OVERDUE`], the window hears
///      [`FOLDER_PANEL_OVERDUE_EVENT`] and a minimized or hidden window is
///      brought forward;
///   4. past [`FOLDER_PANEL_HELPER_WAIT`], or on a cancel, the helper is
///      killed.
///
/// An empty answer is a dismissal, which is an answer and not an error.
pub(super) async fn pick_paths(
    app: &AppHandle,
    request: PickRequest,
) -> Result<Vec<PathBuf>, String> {
    let asked_at = Instant::now();
    let generation = match folder_panel_desk().ask(asked_at) {
        FolderPanelAsk::Fresh { generation } => generation,
        FolderPanelAsk::Standing { since, recalls } => {
            let helper = folder_panel_desk().helper_pid(asked_at);
            let forwarded = helper.is_some_and(bring_helper_forward);
            if !forwarded {
                bring_main_window_forward(app);
            }
            note_folder_panel(
                app,
                &format!(
                    "asked again after {}ms (recall {recalls}) — the standing {} was brought \
                     forward, no second one opened",
                    since.as_millis(),
                    if forwarded { "helper" } else { "window" }
                ),
            );
            return Err(FOLDER_PANEL_STANDING.to_string());
        }
    };
    note_folder_panel(
        app,
        &format!("asked (generation {generation}, {})", request.kind.as_str()),
    );

    let Some(program) = pick_helper_program() else {
        folder_panel_desk().abandon(generation);
        note_folder_panel(
            app,
            "no helper beside the window or on PATH — nothing opened",
        );
        return Err(format!(
            "파일·폴더 선택 헬퍼({HELPER_NAME})를 찾지 못했습니다"
        ));
    };
    let (cancel, cancelled) = tokio::sync::oneshot::channel();
    let mut helper = Box::pin(run_pick_helper(
        program,
        request,
        FOLDER_PANEL_HELPER_WAIT,
        cancelled,
        {
            let app = app.clone();
            move |pid| {
                folder_panel_desk().attach_helper(generation, HelperHandle::new(pid, cancel));
                // The panel is the helper's window: hand it the activation this
                // window holds, once it has had time to open.
                tokio::spawn(async move {
                    tokio::time::sleep(FOLDER_PANEL_RAISE_DELAY).await;
                    let noted = app.clone();
                    let _ = app.run_on_main_thread(move || {
                        let forwarded = bring_helper_forward(pid);
                        note_folder_panel(
                            &noted,
                            &format!(
                                "helper {} brought forward at spawn (cooperative activation {})",
                                pid,
                                if forwarded { "granted" } else { "refused" }
                            ),
                        );
                    });
                });
            }
        },
    ));

    // The receipt behind the ask. The ask no longer queues anything to the
    // main thread, so this measures only whether that thread is free — and
    // a stall here is some other bug, which is why it stays.
    let (mark, marked) = tokio::sync::oneshot::channel();
    let _ = app.run_on_main_thread(move || {
        let _ = mark.send(Instant::now());
    });
    let (main_thread_wait, early_answer) = match wait_for_pick_start(
        asked_at,
        FOLDER_PANEL_MAIN_THREAD_BUDGET,
        marked,
        &mut helper,
    )
    .await
    {
        PickStart::MainThread(wait) => (wait, None),
        PickStart::Answered(outcome) => (None, Some(outcome)),
    };
    match main_thread_wait {
        Some(wait) => note_folder_panel(
            app,
            &format!("main thread receipt after {}ms", wait.as_millis()),
        ),
        None => note_folder_panel(
            app,
            &format!(
                "no main thread receipt within {}ms (or the helper answered first) — the panel \
                 stands in its own process either way",
                FOLDER_PANEL_MAIN_THREAD_BUDGET.as_millis()
            ),
        ),
    }

    let remaining = FOLDER_PANEL_OVERDUE.saturating_sub(asked_at.elapsed());
    let outcome = if let Some(outcome) = early_answer {
        outcome
    } else {
        match tokio::time::timeout(remaining, &mut helper).await {
            Ok(outcome) => outcome,
            Err(_) => {
                let in_front = main_window_in_front(app).unwrap_or(true);
                if !in_front {
                    bring_main_window_forward(app);
                }
                let overdue = FolderPanelOverdue {
                    after_ms: u64::try_from(asked_at.elapsed().as_millis()).unwrap_or(u64::MAX),
                    brought_forward: !in_front,
                    main_thread_wait_ms: main_thread_wait
                        .map(|wait| u64::try_from(wait.as_millis()).unwrap_or(u64::MAX)),
                };
                note_folder_panel(
                    app,
                    &format!(
                        "standing for {}ms with no answer — the window was told{}",
                        overdue.after_ms,
                        if overdue.brought_forward {
                            " and brought forward"
                        } else {
                            ""
                        }
                    ),
                );
                let _ = app.emit_to(MAIN_WINDOW_LABEL, FOLDER_PANEL_OVERDUE_EVENT, overdue);
                helper.await
            }
        }
    };

    let receipt = folder_panel_desk().answered(generation, Instant::now());
    let said = match &outcome {
        PickOutcome::Chosen(paths) => format!("{} path(s)", paths.len()),
        PickOutcome::Dismissed => "dismissed".to_string(),
        PickOutcome::TimedOut => "killed past its wait".to_string(),
        PickOutcome::Cancelled => "cancelled and killed".to_string(),
        PickOutcome::Failed(why) => format!("failed: {why}"),
    };
    match receipt {
        Some(receipt) => note_folder_panel(
            app,
            &format!(
                "answered {said} after {}ms (recall {})",
                receipt.stood_for.as_millis(),
                receipt.recalls
            ),
        ),
        None => note_folder_panel(
            app,
            &format!("generation {generation} answered {said} after it was presumed lost"),
        ),
    }
    match outcome {
        PickOutcome::Chosen(paths) => Ok(paths),
        PickOutcome::Dismissed | PickOutcome::Cancelled => Ok(Vec::new()),
        PickOutcome::TimedOut => Err(format!(
            "파일·폴더 선택 창이 {}분 동안 답이 없어 닫았습니다",
            FOLDER_PANEL_HELPER_WAIT.as_secs() / 60
        )),
        PickOutcome::Failed(why) => Err(why),
    }
}

/// The person says they cannot see the panel: bring the helper that holds it
/// forward, or the window when no helper has said its pid. Answers whether
/// a panel was standing to be seen.
pub(super) fn recall_folder_panel_window(app: &AppHandle) -> bool {
    let now = Instant::now();
    let standing = folder_panel_desk().standing_for(now);
    let helper = folder_panel_desk().helper_pid(now);
    let forwarded = helper.is_some_and(bring_helper_forward);
    if !forwarded {
        bring_main_window_forward(app);
    }
    match standing {
        Some(since) => note_folder_panel(
            app,
            &format!(
                "recalled after {}ms — the {} was brought forward",
                since.as_millis(),
                if forwarded { "helper" } else { "window" }
            ),
        ),
        None => note_folder_panel(
            app,
            "recalled with no panel standing — the window was brought forward",
        ),
    }
    standing.is_some()
}

/// The person gives up on the panel: kill the helper. Answers whether there
/// was one to kill; the seat above hears `Cancelled` and answers "nothing".
pub(super) fn cancel_folder_panel_window(app: &AppHandle) -> bool {
    let cancelled = folder_panel_desk().cancel_helper();
    note_folder_panel(
        app,
        if cancelled {
            "cancelled — the helper was told to die"
        } else {
            "cancelled with no helper standing"
        },
    );
    cancelled
}

/* ---- the in-window folder browser (t-2982) -------------------------------
 *
 * The default door of 프로젝트 추가 → 폴더 찾아보기 is a modal in the webview,
 * not AppKit's panel: three force-quits on that road (t-2488 09-05 02:49,
 * 09-06 17:15, 09-07 11:38) were NSOpenPanel's remote view service holding
 * the main thread while it came up — see `docs/design/folder-panel-off-the-
 * main-thread.md`. The browser asks two questions of this backend, both
 * plain directory reads on the async runtime: one folder level anywhere
 * ([`browse_folder`]) and where to start ([`browse_places_of`]). The numbers
 * are the two rows above in the folder panel's table. */

/// What one attached path is right now — the word the composer's chip keys
/// its glyph by, and 「없음」 when it is gone (t-2993,
/// docs/design/composer-attachments.md §2.4).
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(super) enum PathKind {
    File,
    Folder,
    Missing,
}

/// One kind per path, in the order asked. `metadata` follows symlinks, so a
/// link is what it points at and a dangling one is missing — which is what
/// the agent reading the path would find. Names only: nothing is read, and
/// nothing is capped here — the window sends at most its table's count.
pub(super) fn path_kinds_of(paths: &[String]) -> Vec<PathKind> {
    paths
        .iter()
        .map(|path| match std::fs::metadata(path) {
            Ok(meta) if meta.is_dir() => PathKind::Folder,
            Ok(_) => PathKind::File,
            Err(_) => PathKind::Missing,
        })
        .collect()
}

/// One folder level, anywhere on the disk.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(super) struct BrowseAnswer {
    /// The folder listed, canonical — what `..` and a symlink resolved to,
    /// so the path field shows where the person actually is.
    pub(super) path: String,
    /// One level up, `None` at the root of a disk; the browser's ← goes to
    /// the start view from there.
    pub(super) parent: Option<String>,
    pub(super) entries: Vec<DirEntry>,
    /// How many entries the folder had before the cap.
    pub(super) total: usize,
    pub(super) truncated: bool,
}

/// One row of the browser's start view.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(super) struct BrowsePlace {
    /// `recent`, `home` or `volume` — the window picks the glyph and the
    /// label by this; the name is what the row prints beside it.
    pub(super) kind: &'static str,
    pub(super) name: String,
    pub(super) path: String,
}

/// Where the browser starts.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(super) struct BrowsePlaces {
    /// The home folder, for `~` in the path field and for the default start.
    pub(super) home: Option<String>,
    pub(super) places: Vec<BrowsePlace>,
}

/// List `target` for the browser: canonical path, its parent, and one level
/// of entries cut at `cap`. A path that is not a folder is refused — the
/// browser asked to enter it, and there is nothing to enter.
pub(super) fn browse_folder(target: &Path, cap: usize) -> Result<BrowseAnswer, String> {
    let canonical = target
        .canonicalize()
        .map_err(|error| format!("{}: {error}", target.display()))?;
    if !canonical.is_dir() {
        return Err(format!("{}: 폴더가 아닙니다", target.display()));
    }
    let (entries, total) = read_dir_entries(&canonical, Some(cap))
        .map_err(|error| format!("{}: {error}", canonical.display()))?;
    Ok(BrowseAnswer {
        path: canonical.to_string_lossy().into_owned(),
        parent: canonical
            .parent()
            .map(|parent| parent.to_string_lossy().into_owned()),
        truncated: entries.len() < total,
        entries,
        total,
    })
}

/// The start view's rows, in the order the browser shows them: the newest
/// `cap` recent projects, then home, then every mounted volume. Pure, so the
/// order and the cap are tested without a disk.
pub(super) fn browse_places_of(
    home: Option<&Path>,
    volumes: &[PathBuf],
    recents: &[String],
    cap: usize,
) -> BrowsePlaces {
    let leaf = |path: &Path| {
        path.file_name().map_or_else(
            || path.to_string_lossy().into_owned(),
            |name| name.to_string_lossy().into_owned(),
        )
    };
    let mut places = Vec::with_capacity(cap.min(recents.len()) + 1 + volumes.len());
    for recent in recents.iter().take(cap) {
        places.push(BrowsePlace {
            kind: "recent",
            name: leaf(Path::new(recent)),
            path: recent.clone(),
        });
    }
    if let Some(home) = home {
        places.push(BrowsePlace {
            kind: "home",
            name: leaf(home),
            path: home.to_string_lossy().into_owned(),
        });
    }
    for volume in volumes {
        places.push(BrowsePlace {
            kind: "volume",
            name: leaf(volume),
            path: volume.to_string_lossy().into_owned(),
        });
    }
    BrowsePlaces {
        home: home.map(|home| home.to_string_lossy().into_owned()),
        places,
    }
}

/// `~` and `~/…` are the home folder; anything else — `~other`, an absolute
/// path, a relative one — is handed back as typed, trimmed.
pub(super) fn expand_home(typed: &str, home: Option<&Path>) -> PathBuf {
    let typed = typed.trim();
    match (typed.strip_prefix('~'), home) {
        (Some(""), Some(home)) => home.to_path_buf(),
        (Some(rest), Some(home)) if rest.starts_with(['/', '\\']) => home.join(&rest[1..]),
        _ => PathBuf::from(typed),
    }
}

/// The disks a person can browse: `/Volumes/*` on macOS (the boot disk is
/// among them, as a link to `/`), the drive roots that exist on Windows, and
/// `/` elsewhere.
pub(super) fn mounted_volumes() -> Vec<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let mut volumes: Vec<PathBuf> = std::fs::read_dir("/Volumes")
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .map(|entry| entry.path())
                    .filter(|path| path.is_dir())
                    .collect()
            })
            .unwrap_or_default();
        volumes.sort();
        volumes
    }
    #[cfg(windows)]
    {
        (b'A'..=b'Z')
            .map(|letter| PathBuf::from(format!("{}:\\", letter as char)))
            .filter(|root| root.is_dir())
            .collect()
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        vec![PathBuf::from("/")]
    }
}

/* ---- 프로젝트를 추가하는 나머지 두 길 ---------------------------------------
 *
 * 사이드바의 ＋는 이제 폴더 선택기로 직행하지 않고 다이얼로그 한 장을 세운다
 * (스펙 §12). 그 다이얼로그의 세 길 중 첫째는 위의 [`choose_project`] 그대로이고,
 * 나머지 둘 — URL에서 복제, 빈 폴더에서 시작 — 이 아래 셋이다.
 *
 * 셋 다 같은 모양이다: **이름을 정하고, 부모 안에 한 마디를 만들고, 만들어진
 * 경로를 돌려준다.** 등록은 하지 않는다 — 창이 받은 경로로 `open_project`를
 * 부르는 것이 폴더를 고른 사람이 지나는 그 길이고, 여기서 한 번 더 등록하면
 * 프로젝트가 카탈로그에 들어가는 문이 둘이 된다.
 */

/// 진행 중인 복제가 도달한 지점. 창은 이것으로 막대 하나를 그린다.
#[derive(Serialize, Clone)]
pub(super) struct CloneProgress {
    pub(super) phase: String,
    pub(super) percent: u8,
}

/// `parent` 안에 `name` 한 마디를 만들 자리를 정한다 — 만들지는 않는다.
///
/// 부모를 **먼저 canonicalize**하는 것이 이 함수의 전부다. `..`를 품은 이름은
/// [`is_dir_name`](zerocode_core::clone::is_dir_name)이 이미 막지만, 부모 쪽
/// 심볼릭 링크는 이름이 아니라 경로의 성질이라 이름 검사로는 안 잡힌다.
pub(super) fn place_inside(parent: &str, name: &str) -> Result<PathBuf, String> {
    if !zerocode_core::clone::is_dir_name(name) {
        return Err("폴더 이름으로 쓸 수 없습니다".to_string());
    }
    let parent = PathBuf::from(parent.trim())
        .canonicalize()
        .map_err(|error| format!("{parent}: {error}"))?;
    if !parent.is_dir() {
        return Err("폴더가 아닙니다".to_string());
    }
    let target = parent.join(name);
    // 이름 검사와 부모 해소를 둘 다 지나고도 부모 밖이면 그것은 우리가 모르는
    // 경로 문법이고, 모르는 것은 거절한다.
    if target.parent() != Some(parent.as_path()) {
        return Err("폴더 이름으로 쓸 수 없습니다".to_string());
    }
    if target.exists() {
        return Err(format!("{name}: 이미 있습니다"));
    }
    Ok(target)
}

/// How many repositories the sidebar catalog keeps.
pub(super) const MAX_PROJECTS: usize = 64;

/// Register `path` without moving any repository the person already placed.
///
/// Repository headers are spatial navigation: activating one of their
/// worktrees must not turn the catalog into an MRU list. New repositories are
/// appended, duplicates are removed without changing the first occurrence,
/// and only the oldest tail entry yields when the cap is full.
pub(super) fn remember_project(known: &[String], path: &str, cap: usize) -> Vec<String> {
    let mut kept = Vec::with_capacity(cap.min(known.len().saturating_add(1)));
    for held in known {
        if kept.len() == cap {
            break;
        }
        if !kept.iter().any(|seen| seen == held) {
            kept.push(held.clone());
        }
    }
    if cap > 0 && !kept.iter().any(|held| held == path) {
        if kept.len() == cap {
            kept.pop();
        }
        kept.push(path.to_string());
    }
    kept
}

/// Whether this path is a whole filesystem rather than a project in one.
///
/// **A GUI launch has no working directory of its own.** macOS hands a
/// Finder- or Dock-started app `/`, so the boot that records "the project
/// somebody opened from a terminal" was writing down the root of the disk —
/// reported as "이걸 연 적이 없는데 계속 생성됨", a `/` row that came back
/// after every removal because every launch put it there again.
///
/// Asked as "has no parent" rather than compared against `"/"`: that is the
/// same question on Windows, where a drive root (`C:\`) is the answer a
/// launch from Explorer would give.
pub(super) fn is_whole_filesystem(path: &Path) -> bool {
    path.parent().is_none()
}

pub(super) fn recent_projects_file(config_root: &Path) -> PathBuf {
    config_root.join(artifact_file::RECENT_PROJECTS)
}

pub(super) fn onboarding_file(config_root: &Path) -> PathBuf {
    config_root.join(artifact_file::ONBOARDING)
}

/// 이 프로세스가 무엇이든 쓰기 **전에** 재는 한 가지 사실: 이 기계가 우리를
/// 이미 알고 있었는가.
///
/// Orca는 데이터 파일 하나를 쓰므로 `existsSync(dataFile)`이면 끝이지만
/// (`fileExistedOnLoad`), 여기 상태는 관심사마다 파일 하나로 흩어져 있어
/// **디렉터리에 뭐라도 들어 있었는가**가 같은 질문이 된다. 재는 시점이 전부다:
/// `note_project`가 최근 프로젝트를 적는 순간 이 답은 영원히 참이 되고, 그러면
/// 신규 설치와 기존 사용자를 다시는 가를 수 없다.
pub(super) fn state_profile_existed(state_root: &Path) -> bool {
    std::fs::read_dir(state_root).is_ok_and(|mut entries| entries.next().is_some())
}

/// 저장된 첫 실행 상태, 이 판의 눈금으로 읽어서.
///
/// 파일이 있는데 통째로 못 읽히는 경우도 **이미 쓰던 사람**으로 읽는다
/// (`Some(default)`). 손상된 한 파일 때문에 마법사를 다시 띄우는 것이 두 실패
/// 중 나쁜 쪽이고, 이 파일은 남이 준 것이 아니라 우리가 쓴 것이다.
pub(super) fn stored_onboarding(config_root: &Path) -> zerocode_core::Onboarding {
    let file = onboarding_file(config_root);
    let Ok(text) = std::fs::read_to_string(&file) else {
        // 파일이 없다 — 이 실행의 시작에서 이미 판정하고 적어 두었으므로
        // (`settle_onboarding`) 여기로 오는 것은 그 뒤에 누가 지운 경우다.
        return zerocode_core::Onboarding::default();
    };
    let stored = serde_json::from_str(&text).unwrap_or_default();
    zerocode_core::onboarding::normalized(stored)
}

pub(super) fn write_onboarding(
    config_root: &Path,
    onboarding: &zerocode_core::Onboarding,
) -> Result<(), String> {
    let file = onboarding_file(config_root);
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let text = serde_json::to_string_pretty(onboarding).map_err(|error| error.to_string())?;
    std::fs::write(&file, text).map_err(|error| error.to_string())
}

/// 첫 실행 판정을 이번 실행에서 딱 한 번, 창이 묻기 전에.
///
/// 기록이 없는 사람에게 답을 **적어 두는** 것까지가 판정이다. 적지 않으면
/// `state_profile_existed`가 다음 실행에도 같은 갈림길을 다시 계산해야 하는데,
/// 그때는 이 실행이 남긴 파일들 때문에 신규 설치가 기존 사용자로 뒤바뀐다.
pub(super) fn settle_onboarding(config_root: &Path, profile_existed: bool) {
    if onboarding_file(config_root).exists() {
        return;
    }
    let settled = zerocode_core::onboarding::on_load(None, profile_existed, now_epoch_ms());
    let _ = write_onboarding(config_root, &settled);
}

/// 자동 코치마크를 받을 프로필인지 **이번 실행에서 딱 한 번**, 그리고 영구히.
///
/// `settle_onboarding` 바로 뒤에 서야 한다: 마법사를 볼 사람인지가 곧 자동
/// 투어를 받을 사람인지의 근거이고(Orca의 `setContextualToursAutoEligible(
/// shouldShowOnboarding(onboarding))`), 그 판정은 마법사 상태가 확정된 다음에만
/// 옳다. 매 실행 다시 계산하면 마법사를 끝낸 다음 실행부터 모두가 "자격 없음"이
/// 되어 투어는 아무에게도 뜨지 않는다.
pub(super) fn settle_tour_eligibility(config_root: &Path) {
    let Some(next) =
        zerocode_core::onboarding::settled_tour_eligibility(&stored_onboarding(config_root))
    else {
        return;
    };
    let _ = write_onboarding(config_root, &next);
}

/// 시작 체크리스트의 현재 상태.
///
/// 창이 재는 넷 — 목록을 읽었나, 프로젝트 수, 곁가지 체크아웃 수, GitHub 연결
/// — 만 인자로 받는다. 첫 번째가 있는 이유는 **아직 읽지 않은 빈 목록과 읽고
/// 나서 빈 목록이 다른 상태**이기 때문이다: 재지 않은 상태를 그리면 창은 매
/// 부팅 첫 프레임에 "아무것도 안 했군요"를 띄운다. 나머지는 백엔드가 이미 들고 있고, **완료 여부는 어디에도 저장되지
/// 않는다**(Orca의 `use-setup-guide-progress`가 제일 잘한 것): 매번 다시
/// 파생하므로 프로젝트를 지운 사람의 목록은 스스로 되돌아간다.
#[derive(Serialize)]
pub(super) struct GuideReport {
    pub(super) steps: Vec<zerocode_core::GuideStep>,
    pub(super) complete: bool,
    pub(super) dismissed: bool,
    /// 사이드바에 그 한 줄이 서야 하나.
    pub(super) entry: bool,
}

/// 코치마크 하나를 지금 띄울 수 있나 — 그리고 못 띄운다면 왜.
///
/// 창이 재는 사실은 인자로 오고, 저장된 둘(`tours_auto`·`tours_seen`)은 여기서
/// 읽는다. 규칙 자체는 코어의 순수 함수 하나이고 창은 그 답을 그대로 따른다 —
/// 창이 자기 판단을 하나라도 더하면 두 판정이 갈라지고, 갈라진 쪽은 "왜 안
/// 뜨지"라는 질문에 아무도 답할 수 없게 된다.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TourAsk {
    pub(super) id: String,
    pub(super) here: bool,
    pub(super) ready: bool,
    pub(super) modal_open: bool,
    pub(super) active_tour: bool,
    pub(super) spent_this_session: bool,
    pub(super) has_target: bool,
}

/// 이번 오픈에 팁을 띄울지, 그리고 보여 주지 않고 끝낼 것들.
#[derive(Serialize)]
pub(super) struct TipReport {
    /// `suppress` / `skip` / `show` 셋 중 하나. `suppress`는 마법사가 떠 있어서
    /// 이 실행 전체를 봉인해야 한다는 뜻이다.
    pub(super) verdict: &'static str,
    pub(super) id: Option<&'static str>,
    pub(super) settle: Vec<&'static str>,
}

/// The active checkout and the remote whose host selects an account in `gh`.
/// The webview supplies neither path nor host: both are process-owned facts,
/// and a stale integration card cannot redirect an auth mutation elsewhere.
pub(super) fn github_context(state: &AppState) -> (PathBuf, Option<String>) {
    let root = state.active_root();
    let remote = git_text(&root, &["remote"])
        .ok()
        .and_then(|listed| gh::pick_remote(&listed).ok())
        .and_then(|remote| git_text(&root, &["remote", "get-url", &remote]).ok());
    (root, remote)
}

/// Credential commands never forward CLI stderr. A provider can echo an
/// environment credential in an error, and renderer IPC is not a secret
/// transport.
pub(super) fn github_lifecycle_error(error: gh::GhError) -> String {
    match error {
        gh::GhError::Missing => "GitHub CLI(gh)를 찾지 못했습니다".to_string(),
        gh::GhError::Refused(_) => "GitHub CLI가 계정 작업을 완료하지 못했습니다".to_string(),
        gh::GhError::Unreadable(_) => "GitHub CLI의 계정 상태를 읽지 못했습니다".to_string(),
    }
}

/// GitLab의 실패도 CLI가 말한 문장을 그대로 옮기지 않는다 — `glab`은 인증
/// 상태를 출력하면서 토큰 줄을 함께 찍고, 렌더러 IPC는 비밀 통로가 아니다.
pub(super) fn gitlab_lifecycle_error(error: glab::GlabError) -> String {
    match error {
        glab::GlabError::Missing => "GitLab CLI(glab)를 찾지 못했습니다".to_string(),
        // 분류된 거절은 목록 읽기의 것이고 상태 확인에는 오지 않는다. 그래도
        // 여기 함께 적는 것은, 이 길이 카드의 한 문장으로 끝나기 때문이다 —
        // 갈래를 골라 쓰는 자리는 `GlabFailure` 쪽이다.
        glab::GlabError::Refused(_) | glab::GlabError::Denied(_) => {
            "GitLab CLI의 인증 상태를 읽지 못했습니다".to_string()
        }
    }
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum NotificationPreferenceKind {
    Enabled,
    AgentAttention,
    AgentCompletion,
}

pub(super) fn pane_layouts_file(config_root: &Path) -> PathBuf {
    config_root.join(artifact_file::PANE_LAYOUTS)
}

pub(super) fn automations_file(config_root: &Path) -> PathBuf {
    config_root.join(artifact_file::AUTOMATIONS)
}

/// One in-process serialization domain for both automation documents.
///
/// Tauri can dispatch commands from several windows while the scheduler and
/// terminal reaper mutate the same files on their own threads. Atomic rename
/// prevents partial JSON, but only this lock prevents two complete
/// read-modify-write transactions from publishing over one another.
#[derive(Default)]
pub(super) struct AutomationStoreDomain {
    pub(super) next_incarnation: u64,
    pub(super) incarnations: HashMap<String, u64>,
}

impl AutomationStoreDomain {
    pub(super) fn mint(&mut self) -> u64 {
        self.next_incarnation = self.next_incarnation.wrapping_add(1).max(1);
        self.next_incarnation
    }

    pub(super) fn current_or_mint(&mut self, id: &str) -> u64 {
        if let Some(incarnation) = self.incarnations.get(id) {
            return *incarnation;
        }
        let incarnation = self.mint();
        self.incarnations.insert(id.to_string(), incarnation);
        incarnation
    }

    pub(super) fn replace_incarnation(&mut self, id: &str) -> u64 {
        let incarnation = self.mint();
        self.incarnations.insert(id.to_string(), incarnation);
        incarnation
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct AutomationRunClaim {
    pub(super) automation_id: String,
    pub(super) incarnation: u64,
}

pub(super) fn automation_store_domain() -> MutexGuard<'static, AutomationStoreDomain> {
    static DOMAIN: OnceLock<Mutex<AutomationStoreDomain>> = OnceLock::new();
    DOMAIN
        .get_or_init(|| Mutex::new(AutomationStoreDomain::default()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Re-read a definition and mint the short-lived identity a long run carries.
/// The guard is gone before this returns; no precheck, git command, or spawn
/// happens while the storage domain is locked.
pub(super) fn claim_automation_run(
    config_root: &Path,
    id: &str,
) -> Result<(Automation, AutomationRunClaim), String> {
    let mut domain = automation_store_domain();
    let automation = stored_automations(config_root)
        .into_iter()
        .find(|automation| automation.id == id)
        .ok_or_else(|| format!("자동화 {id}을(를) 찾지 못했습니다"))?;
    let incarnation = domain.current_or_mint(id);
    Ok((
        automation,
        AutomationRunClaim {
            automation_id: id.to_string(),
            incarnation,
        },
    ))
}

/// The scheduled jobs, as stored.
///
/// A file that will not parse answers empty rather than failing the window:
/// automations are one screen of it, and a window that refuses to open because
/// a schedule is malformed is worse than one that opens with the list blank
/// and lets the person write it again.
pub(super) fn stored_automations(config_root: &Path) -> Vec<Automation> {
    std::fs::read_to_string(automations_file(config_root))
        .ok()
        .and_then(|text| serde_json::from_str::<Vec<Automation>>(&text).ok())
        .unwrap_or_default()
}

pub(super) fn write_automations(config_root: &Path, rows: &[Automation]) -> Result<(), String> {
    let file = automations_file(config_root);
    let text = serde_json::to_string_pretty(rows).map_err(|error| error.to_string())?;
    durable_file::replace_bytes(&file, text.as_bytes())
        .map(|_| ())
        .map_err(|error| error.to_string())
}

pub(super) fn automation_runs_file(local_data_root: &Path) -> PathBuf {
    local_data_root.join(artifact_file::AUTOMATION_RUNS)
}

/// Every run this window has ever started, oldest first.
///
/// Its own file rather than a field on the schedule, for the reason the
/// schedules have their own: a history that grows without bound inside the
/// record the editor rewrites on every save is a record that gets larger
/// every time somebody fixes a typo, and one bad write would take both.
/// Unreadable answers empty, the same way the schedules do.
pub(super) fn stored_automation_runs(local_data_root: &Path) -> Vec<AutomationRun> {
    std::fs::read_to_string(automation_runs_file(local_data_root))
        .ok()
        .and_then(|text| serde_json::from_str::<Vec<AutomationRun>>(&text).ok())
        .unwrap_or_default()
}

pub(super) fn write_automation_runs(
    local_data_root: &Path,
    rows: &[AutomationRun],
) -> Result<(), String> {
    let file = automation_runs_file(local_data_root);
    let text = serde_json::to_string_pretty(rows).map_err(|error| error.to_string())?;
    durable_file::replace_bytes(&file, text.as_bytes())
        .map(|_| ())
        .map_err(|error| error.to_string())
}

/// This instant, in milliseconds since the epoch.
///
/// A run is reported to a reader, and a reader's locale turns a moment into
/// words — so the ledger keeps the moment, not the scheduler's local minute.
pub(crate) fn now_epoch_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_millis() as i64)
}

/* ---- quick commands --------------------------------------------------------
 *
 * Saved keystrokes for the terminal's context menu. Orca's model
 * (`normalizeTerminalQuickCommands`, store-BgJxB0hr.js:1578-1637): a label
 * and a body, both capped at 80/4000, scoped to one repository or to every
 * window, and one of two actions — text typed at the terminal (with the
 * Enter optional: a command left uncommitted is a template, and the menu
 * marks it `Insert`), or a prompt handed to an agent. An agent that only
 * accepts prompts by being typed at cannot carry a quick command
 * (`supportsTerminalAgentQuickCommand`) — there is no terminal of its own
 * to type into yet, so the prompt rides the launch, and stdin-after-start
 * has nothing to ride. */

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(super) struct QuickCommand {
    pub(super) id: String,
    pub(super) label: String,
    /// `None` is Orca's `global` scope; a workspace root is its repo scope.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) workspace: Option<String>,
    /// The text — a command for the terminal, or a prompt when `agent` says
    /// which agent should hear it.
    pub(super) body: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) agent: Option<String>,
    #[serde(default = "enter_by_default")]
    pub(super) append_enter: bool,
}

pub(super) fn enter_by_default() -> bool {
    true
}

/* ---- the project's own scripts --------------------------------------------
 *
 * `zerocode.yaml` at a checkout's root says what to run when a workspace is
 * created and when one is removed. The schema and the rules are in
 * `zerocode-core::project`; the runner is in `script.rs`; what lives here is
 * this machine's half — the local script and the two policies, which are a
 * per-repository setting in Orca and are kept per-repository here too.
 *
 * Why a local half exists at all: the shared file belongs to the repository
 * and everybody who clones it, and not everybody wants a repository deciding
 * what runs on their laptop. `ScriptSource` is that choice. */

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(super) struct ProjectSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) local_setup: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) local_archive: Option<String>,
    #[serde(default)]
    pub(super) source: zerocode_core::ScriptSource,
    #[serde(default)]
    pub(super) setup_run_policy: zerocode_core::SetupRunPolicy,
    /// The repository's colour mark, and `None` for one nobody has marked.
    ///
    /// It rides here rather than in a file of its own because this map is
    /// already "what THIS MACHINE says about that repository, keyed by its
    /// root" — the same sentence a colour is. Absent rather than null for an
    /// unmarked repository (`skip_serializing_if`), so the file grows a key
    /// only for a repository somebody actually marked, and a settings file
    /// written before this field existed reads back as no mark at all rather
    /// than failing to parse (`serde(default)`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) mark_color: Option<String>,
    /// When this project first entered the list, epoch milliseconds.
    ///
    /// Written once, by [`note_project`], and only for a project the recent
    /// list has never held. It exists for exactly one question — which side of
    /// [`zerocode_core::VISIBILITY_ROLLOUT_MS`] this project is on — and
    /// `None` is the answer for every project that was already here, which is
    /// the answer that keeps their sidebars full.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) added_at: Option<i64>,
    /// Whether this project lets in worktrees this window did not make, and
    /// the four records the surfaces around that switch keep.
    ///
    /// Nested under one key rather than spread across six top-level fields, so
    /// the file reads as "this repository's answer about external worktrees"
    /// and an untouched project writes nothing at all (the record serialises to
    /// `{}` and is skipped whole).
    #[serde(default, skip_serializing_if = "untouched_visibility")]
    pub(super) external_worktrees: zerocode_core::ExternalVisibility,
    /// What this repository's new worktrees are cut from when nobody says —
    /// Orca's `repo.worktreeBaseRef`.
    ///
    /// Written by USE rather than by a settings row: the create dialog's base
    /// field remembers what was typed there last for this repository, which is
    /// the only way a per-repository default gets set without adding a screen
    /// nobody visits. Still only a default — the ladder in
    /// [`resolve_create_base`] drops it the moment the ref stops existing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) worktree_base_ref: Option<String>,
}

/// Whether nobody has said anything about this project's external worktrees.
///
/// The `skip_serializing_if` for the nested record: a project that has never
/// been asked must leave no key behind, because a key in the file reads as an
/// answer somebody gave.
pub(super) fn untouched_visibility(held: &zerocode_core::ExternalVisibility) -> bool {
    *held == zerocode_core::ExternalVisibility::default()
}

impl ProjectSettings {
    /// Whether this entry says anything about SCRIPTS.
    ///
    /// A repository mark can create an entry that has no script opinion. This
    /// predicate lets the keyed reader distinguish that from a legacy
    /// worktree entry that still owns the script fields during migration.
    pub(super) fn speaks_about_scripts(&self) -> bool {
        self.local_setup.is_some()
            || self.local_archive.is_some()
            || self.source != zerocode_core::ScriptSource::default()
            || self.setup_run_policy != zerocode_core::SetupRunPolicy::default()
    }

    /// Whether this entry says anything AT ALL.
    ///
    /// What the erase paths check before they take a key out of the file. It
    /// names every field rather than the scripts alone: taking the entry out
    /// because its colour was removed would also throw away when this project
    /// was added and whether its external worktrees are let in — and the second
    /// of those, silently reset, empties somebody's sidebar.
    pub(super) fn says_nothing(&self) -> bool {
        !self.speaks_about_scripts()
            && self.mark_color.is_none()
            && self.added_at.is_none()
            && self.worktree_base_ref.is_none()
            && untouched_visibility(&self.external_worktrees)
    }
}

/// One project's entry, or the default for a project nobody has said anything
/// about.
pub(super) fn stored_project_settings_at(
    repository: &settings::SettingsRepository,
    key: &str,
) -> Result<ProjectSettings, String> {
    Ok(stored_project_settings_all(repository)?
        .remove(key)
        .unwrap_or_default())
}

/// This machine's script settings for the active repository, keyed by its root.
///
/// Keyed rather than global because the setting is about one repository's file:
/// "run both" means something different for a repo whose setup is `npm ci` than
/// for one whose setup starts a database.
pub(super) fn legacy_project_settings_all(root: &Path) -> BTreeMap<String, ProjectSettings> {
    LegacySettings::new(root)
        .read(legacy_settings_file::PROJECT_SETTINGS)
        .unwrap_or_default()
}

/// Import the old raw map once during startup. The legacy file remains
/// untouched as a rollback source, and tests never need to mutate HOME merely
/// to inject a repository root.
pub(super) fn migrate_legacy_project_settings(
    repository: &settings::SettingsRepository,
) -> Result<(), String> {
    let read = repository
        .read_json::<BTreeMap<String, ProjectSettings>>(PROJECT_SETTINGS_DOCUMENT_FILE)
        .map_err(|error| error.to_string())?;
    if read.value.is_some() {
        return complete_legacy_migration(repository, LegacyMigration::ProjectSettings);
    }

    let seed = if legacy_migration_completed(repository, LegacyMigration::ProjectSettings)? {
        BTreeMap::new()
    } else {
        legacy_project_settings_all(repository.root())
    };
    repository
        .mutate_json(
            PROJECT_SETTINGS_DOCUMENT_FILE,
            || seed,
            |_fresh: &mut BTreeMap<String, ProjectSettings>| {},
        )
        .map_err(|error| error.to_string())?;
    complete_legacy_migration(repository, LegacyMigration::ProjectSettings)
}

/// Load the versioned project-settings document. Startup owns legacy import;
/// callers see an empty map only for a deliberately empty injected store.
pub(super) fn stored_project_settings_all(
    repository: &settings::SettingsRepository,
) -> Result<BTreeMap<String, ProjectSettings>, String> {
    repository
        .read_json::<BTreeMap<String, ProjectSettings>>(PROJECT_SETTINGS_DOCUMENT_FILE)
        .map(|read| read.value.unwrap_or_default())
        .map_err(|error| error.to_string())
}

pub(super) fn mutate_project_settings<R>(
    repository: &settings::SettingsRepository,
    change: impl FnOnce(&mut BTreeMap<String, ProjectSettings>) -> Result<R, String>,
) -> Result<R, String> {
    repository
        .try_mutate_json(
            PROJECT_SETTINGS_DOCUMENT_FILE,
            BTreeMap::<String, ProjectSettings>::new,
            change,
        )
        .map_err(|error| error.to_string())?
        .map(|mutation| mutation.result)
}

/// Resolve the script settings owned by `repo_root`.
///
/// Old builds accidentally stored the active worktree path. That key is a
/// migration fallback only for the same checkout; no reader may take an
/// arbitrary first map entry, because it may belong to a different project.
pub(super) fn project_script_settings_from(
    held: &BTreeMap<String, ProjectSettings>,
    repo_root: &Path,
    checkout: Option<&Path>,
) -> ProjectSettings {
    let root_key = project_settings_key(repo_root);
    let raw_root = repo_root.to_string_lossy();
    if let Some(settings) = held
        .get(&root_key)
        .filter(|settings| settings.speaks_about_scripts())
        .or_else(|| {
            held.get(raw_root.as_ref())
                .filter(|settings| settings.speaks_about_scripts())
        })
    {
        return settings.clone();
    }

    checkout
        .filter(|path| *path != repo_root)
        .and_then(|path| {
            let key = project_settings_key(path);
            let raw = path.to_string_lossy();
            held.get(&key)
                .filter(|settings| settings.speaks_about_scripts())
                .or_else(|| {
                    held.get(raw.as_ref())
                        .filter(|settings| settings.speaks_about_scripts())
                })
        })
        .cloned()
        .unwrap_or_else(|| {
            held.get(&root_key)
                .or_else(|| held.get(raw_root.as_ref()))
                .cloned()
                .unwrap_or_default()
        })
}

pub(super) fn stored_project_script_settings(
    repository: &settings::SettingsRepository,
    repo_root: &Path,
    checkout: Option<&Path>,
) -> Result<ProjectSettings, String> {
    Ok(project_script_settings_from(
        &stored_project_settings_all(repository)?,
        repo_root,
        checkout,
    ))
}

/// The colour mark stored for each repository root, ready to read.
///
/// Normalised on the way out, so a value typed into the file by hand reaches
/// the window in the one spelling the picker can compare against its palette —
/// and an unreadable one reaches it as no mark rather than as itself.
pub(super) fn stored_repo_marks(
    repository: &settings::SettingsRepository,
) -> Result<BTreeMap<String, String>, String> {
    Ok(stored_project_settings_all(repository)?
        .into_iter()
        .filter_map(|(root, held)| {
            zerocode_core::normalize_repo_mark(held.mark_color.as_deref())
                .map(|colour| (root, colour))
        })
        .collect())
}

/// What each repository root has said about external worktrees, and when it
/// joined the list.
///
/// A reader of its own beside [`stored_repo_marks`], for the reason that one is
/// separate from [`stored_project_settings_at`]: one question, one answer, and the
/// caller cannot pick up the wrong half of an entry. The pair travels together
/// because neither half means anything alone — the switch's default is decided
/// by the date ([`zerocode_core::effective_visibility`]).
pub(super) fn stored_external_visibility(
    repository: &settings::SettingsRepository,
) -> Result<BTreeMap<String, (zerocode_core::ExternalVisibility, Option<i64>)>, String> {
    Ok(stored_project_settings_all(repository)?
        .into_iter()
        .map(|(root, held)| (root, (held.external_worktrees, held.added_at)))
        .collect())
}

/// Change one project's entry in place, then write the file.
///
/// In place, never by replacing the record: this map holds five unrelated facts
/// about a repository and a writer that builds a whole `ProjectSettings` erases
/// whichever of them it has not heard of. That is not hypothetical — the colour
/// mark was already being carried across one such write by hand, and each new
/// field would need another line nobody would remember to add.
///
/// An entry left saying nothing is taken out rather than stored as `{}`, so an
/// erase leaves no trace and the file only ever names repositories somebody has
/// an opinion about.
pub(super) fn update_project_settings(
    repository: &settings::SettingsRepository,
    root: &str,
    change: impl FnOnce(&mut ProjectSettings),
) -> Result<(), String> {
    mutate_project_settings(repository, |held| {
        let entry = held.entry(root.to_string()).or_default();
        change(entry);
        if entry.says_nothing() {
            held.remove(root);
        }
        Ok(())
    })
}

/// The project root as the catalog spells it.
///
/// Every per-project key goes through this. `project_catalog` canonicalizes
/// each path it lists, so a key stored uncanonically is a key nothing ever
/// finds again — a setting that appears to save and then does nothing.
pub(super) fn project_settings_key(path: impl AsRef<Path>) -> String {
    let path = path.as_ref();
    path.canonicalize().map_or_else(
        |_| path.to_string_lossy().into_owned(),
        |resolved| resolved.to_string_lossy().into_owned(),
    )
}

/* Whether a repository's own code may run on this machine.
 *
 * The setup script and the `defaultTabs:` commands both arrive by `git clone`,
 * and until this gate existed both ran for any repository somebody opened. A
 * stranger's checkout could declare either. Orca asks once per repository, per
 * VERSION of what it asks to run (`OrcaYamlTrustDialog`) — see
 * `zerocode_core::repo_trust` for what "trusted" means; what is here is where
 * the answer is kept, and the check that does not believe the window.
 *
 * The dialog in the webview is the user experience. This is the enforcement,
 * and they are deliberately not the same code: a webview bug — a gate that
 * throws, a call site added later that forgets to ask — must not be able to run
 * a script nobody approved. The window asks; Rust decides. */

/// One repository's answer, as stored.
///
/// Tolerant on the way in (`serde(default)` on every field) because this file
/// is read on a path where failing would mean failing OPEN — an entry that will
/// not parse has to read as "nobody has approved anything", which is the safe
/// direction and also the true one.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(super) struct StoredRepoTrust {
    /// When "always trust this repository" was ticked. Hash-independent by
    /// design: it is the answer to "stop asking me about my own repository".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) all: Option<i64>,
    /// The content hash that was approved, from [`zerocode_core::content_hash`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) setup_hash: Option<String>,
    /// The archive script's own approval, hashed apart from the setup's.
    ///
    /// A separate slot because it is a separate QUESTION asked at a separate
    /// moment: the setup bundle runs at creation, the archive at removal, and
    /// Orca splits the kinds the same way (`orca-hook-trust.ts:3` —
    /// setup/archive/issueCommand each carry their own confirmation). One
    /// shared hash would let a repository approved for its setup swap its
    /// archive afterwards and keep running it — the map's P0-6.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) archive_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) approved_at: Option<i64>,
}

pub(super) fn repo_trust_file(config_root: &Path) -> PathBuf {
    config_root.join(artifact_file::REPO_TRUST)
}

/// Every repository's answer, keyed by repository root.
///
/// A `BTreeMap` rather than a hash map for the reason the settings map is one:
/// this file is rewritten every time somebody approves something, and a stable
/// key order keeps the diff between two writes to the line that changed.
pub(super) fn stored_repo_trust(config_root: &Path) -> BTreeMap<String, StoredRepoTrust> {
    std::fs::read_to_string(repo_trust_file(config_root))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

pub(super) fn write_repo_trust(
    config_root: &Path,
    held: &BTreeMap<String, StoredRepoTrust>,
) -> Result<(), String> {
    let file = repo_trust_file(config_root);
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let text = serde_json::to_string_pretty(held).map_err(|error| error.to_string())?;
    std::fs::write(&file, text).map_err(|error| error.to_string())
}

/// The key one repository is filed under.
///
/// Resolved, so the same repository reached by a symlink and by its real path
/// is one entry — otherwise approving it once would leave it asking again from
/// the other door. An unresolvable path is kept as written rather than dropped:
/// a repository on a volume that has gone away should not silently become a
/// different, untrusted one when it comes back.
pub(super) fn repo_trust_key(root: &Path) -> String {
    root.canonicalize()
        .unwrap_or_else(|_| root.to_path_buf())
        .to_string_lossy()
        .into_owned()
}

/// Everything the repository at this checkout has asked to run, or `None` when
/// its file is there and could not be read.
///
/// `None` is the fail-closed answer and it means "approve nothing, run
/// nothing". It is a narrow case by construction: a file this reader cannot
/// parse also yields no setup script and no tabs, so nothing repo-supplied can
/// run from it anyway — but the caller is told rather than shown an empty
/// string it would read as consent already given.
pub(super) fn repo_trust_content(checkout: &Path) -> Option<String> {
    // No file at all is not a failure to read one: most repositories declare
    // nothing, and they must never produce a dialog.
    let Some(file) = script::read_project_file(checkout) else {
        return Some(String::new());
    };
    if file.unreadable {
        return None;
    }
    let tabs: Vec<(String, String)> = file
        .default_tabs
        .iter()
        .map(|tab| {
            (
                tab.title.clone().unwrap_or_default(),
                tab.command.clone().unwrap_or_default(),
            )
        })
        .collect();
    Some(zerocode_core::trust_content(file.setup.as_deref(), &tabs))
}

/// Where this repository stands, for a checkout of it.
pub(super) fn repo_trust_standing_of(
    config_root: &Path,
    repo_root: &Path,
    content: &str,
) -> zerocode_core::TrustStanding {
    let held = stored_repo_trust(config_root);
    let entry = held.get(&repo_trust_key(repo_root));
    zerocode_core::standing(
        entry.is_some_and(|one| one.all.is_some()),
        entry.and_then(|one| one.setup_hash.as_deref()),
        content,
    )
}

/// May the repository's own half of the setup script run in this checkout?
///
/// The check Rust makes for itself, against the content it is about to run
/// rather than against anything the window said about it.
pub(super) fn repo_trust_allows(config_root: &Path, repo_root: &Path, checkout: &Path) -> bool {
    let Some(content) = repo_trust_content(checkout) else {
        return false;
    };
    matches!(
        repo_trust_standing_of(config_root, repo_root, &content),
        zerocode_core::TrustStanding::Trusted | zerocode_core::TrustStanding::NothingToRun
    )
}

/// The repository's OWN half of the archive script — what would run at
/// removal that arrived by `git clone`.
///
/// `None` means the repo has no say: the policy is local-only, there is no
/// file, or the file names no archive — all of them "nothing to consent to".
/// A person's `local_archive` is their own words and never needs their
/// permission; asking would train them to click through. An unreadable file
/// parses to no archive at all, so nothing repo-supplied can run from it —
/// the same fail-closed shape `repo_trust_content` keeps.
pub(super) fn repo_archive_half(
    repository: &settings::SettingsRepository,
    repo_root: &Path,
    worktree: &Path,
) -> Result<Option<String>, String> {
    let settings = stored_project_script_settings(repository, repo_root, Some(worktree))?;
    let Some(file) = script::read_project_file(worktree) else {
        return Ok(None);
    };
    Ok(zerocode_core::effective_script(
        &file,
        None,
        zerocode_core::ProjectScript::Archive,
        settings.source,
    ))
}

/// Where this repository stands on its ARCHIVE script, against the archive's
/// own stored hash. `all` still wins — "stop asking me about my own
/// repository" was said about the repository, not about one of its scripts.
pub(super) fn archive_trust_standing_of(
    config_root: &Path,
    repo_root: &Path,
    content: &str,
) -> zerocode_core::TrustStanding {
    let held = stored_repo_trust(config_root);
    let entry = held.get(&repo_trust_key(repo_root));
    zerocode_core::standing(
        entry.is_some_and(|one| one.all.is_some()),
        entry.and_then(|one| one.archive_hash.as_deref()),
        content,
    )
}

/// Forget a repository's answer.
///
/// Called when the project leaves the list: the next person to open that path
/// is asked again. Trust is a thing somebody said about a repository they had
/// chosen to keep, and taking the repository away takes the sentence with it.
pub(super) fn forget_repo_trust(config_root: &Path, repo_root: &Path) {
    let mut held = stored_repo_trust(config_root);
    if held.remove(&repo_trust_key(repo_root)).is_none() {
        return;
    }
    // Best effort: a project that left the list has left it either way, and
    // failing the removal over a state file the window could not write would
    // be reporting the wrong failure to the person.
    let _ = write_repo_trust(config_root, &held);
}

/// The repository a trust question is about, and the checkout its file is read
/// from.
///
/// `project` mirrors the one `create_worktree` takes, and for the same reason:
/// the worktree form can target a repository other than the one the window is
/// standing in. A gate that always asked about the active project would put one
/// repository's script in front of somebody and store their answer under
/// another's key — the dialog would be about the wrong thing and the approval
/// would not take. A named project is read at its ROOT, which is where a
/// worktree made from it gets its file.
pub(super) fn repo_trust_target(
    state: &State<'_, AppState>,
    project: Option<String>,
) -> (PathBuf, PathBuf) {
    if let Some(orchestrator) = project
        .as_deref()
        .and_then(|path| known_project_orchestrator(state.config_root(), path).ok())
    {
        let root = orchestrator.repo_root().to_path_buf();
        return (root.clone(), root);
    }
    let here = state.active_project_context();
    (here.repo_root, here.checkout)
}

/// What the window needs to ask — or the news that it does not have to.
#[derive(Serialize)]
pub(super) struct RepoTrustReport {
    /// `trusted` run it, `nothing` there is nothing to consent to, `ask` put
    /// the dialog up.
    pub(super) standing: &'static str,
    /// Whether this repository was approved for DIFFERENT content before, which
    /// is the difference between "run this?" and "this changed since you said
    /// yes — run the new one?".
    pub(super) changed: bool,
    /// The full text the approval is over, shown verbatim in the dialog. The
    /// same string the hash is taken from, so what is read is what is approved.
    pub(super) content: String,
}

/* ---- worktrees this window did not make ------------------------------------
 *
 * Four commands, one per thing a person can say about a repository's external
 * worktrees: put them away or let them in, stop being asked, take one in by
 * name, and go quiet for good. Every one of them writes through
 * `update_project_settings` into the entry the colour mark and the scripts
 * already share, because "what this machine says about that repository" is one
 * sentence and a fifth file would be a fifth thing to keep in step.
 *
 * The rules are all in `zerocode_core::worktree_ownership`. What is here is the
 * disk and the clock. */

/// Every external worktree of `root` a person would be offered, by path.
///
/// Derived the same way the catalog derives it, from the same rules, so the
/// baseline a "keep hidden" writes down cannot disagree with the list the inbox
/// was drawn from. Answers empty when git will not list the repository — a
/// baseline of "nothing was hidden" written from a failed scan would make every
/// existing worktree look new the next morning.
pub(super) fn hidden_external_paths(
    repository: &settings::SettingsRepository,
    root: &str,
    active: &Path,
) -> Result<Vec<String>, String> {
    let Ok(open) = Orchestrator::open(root) else {
        return Ok(Vec::new());
    };
    let Ok(listed) = open.list() else {
        return Ok(Vec::new());
    };
    let prefs = load_settings_resilient(repository)
        .document
        .workspace_creation_prefs;
    let ours = our_worktree_marks(Some(&open), &prefs);
    let (stored, added_at) = stored_external_visibility(repository)?
        .get(root)
        .cloned()
        .unwrap_or_default();
    let mut hidden = Vec::new();
    for worktree in listed {
        let path = worktree.path.to_string_lossy().into_owned();
        let facts = zerocode_core::WorktreeFacts {
            path: &path,
            branch: worktree.branch.as_deref(),
        };
        // The same three clauses the catalog's walk uses, including the checkout
        // the window is standing in — a baseline that called the active row
        // "hidden" would answer for a row nobody was ever offered.
        let itself = worktree.path == active || worktree.is_main || path == root;
        if !zerocode_core::is_user_facing(facts, ours_of(&ours), itself) {
            continue;
        }
        if !zerocode_core::should_show(facts, ours_of(&ours), &stored, added_at, itself) {
            hidden.push(path);
        }
    }
    Ok(hidden)
}

/// Run the archive script for a checkout that is about to be removed.
pub(super) fn archive_worktree(
    repository: &settings::SettingsRepository,
    config_root: &Path,
    repo_root: &Path,
    worktree: &Path,
) -> Result<script::ScriptOutcome, String> {
    let settings = stored_project_script_settings(repository, repo_root, Some(worktree))?;
    let Some(file) = script::read_project_file(worktree) else {
        return Ok(script::ScriptOutcome::Nothing);
    };
    let Some(said) = zerocode_core::effective_script(
        &file,
        settings.local_archive.as_deref(),
        zerocode_core::ProjectScript::Archive,
        settings.source,
    ) else {
        return Ok(script::ScriptOutcome::Nothing);
    };
    // The archive arrives by `git clone` like the setup does, but it runs at
    // REMOVAL — a moment the creation dialog never covered, and this was the
    // one repo-supplied script with no gate at all: deleting a workspace of a
    // cloned repository ran a stranger's commands unasked (the map's P0-6).
    // Orca keys a separate confirmation to it (`ensureHooksConfirmed(…,
    // 'archive')`, remove-worktree.ts:77-83). Enforcement mirrors the
    // setup's: the window asks, Rust decides, and an unapproved repo half is
    // simply not run — while the person's own `local_archive` still is,
    // because their words need nobody's permission.
    let repo_half = zerocode_core::effective_script(
        &file,
        None,
        zerocode_core::ProjectScript::Archive,
        settings.source,
    );
    let repo_allowed = repo_half.as_deref().is_none_or(|half| {
        matches!(
            archive_trust_standing_of(config_root, repo_root, half),
            zerocode_core::TrustStanding::Trusted | zerocode_core::TrustStanding::NothingToRun
        )
    });
    if repo_allowed {
        return Ok(script::run_script(&said, repo_root, worktree));
    }
    // What may still run is the person's own half — and only where their
    // composition already includes it. Under `SharedOnly` they said the local
    // never runs, and a trust refusal must not widen that into running it.
    // (`LocalOnly` cannot reach here: it makes the repo half `None` above.)
    if settings.source != zerocode_core::ScriptSource::RunBoth {
        return Ok(script::ScriptOutcome::Nothing);
    }
    let Some(mine) = zerocode_core::effective_script(
        &file,
        settings.local_archive.as_deref(),
        zerocode_core::ProjectScript::Archive,
        zerocode_core::ScriptSource::LocalOnly,
    ) else {
        return Ok(script::ScriptOutcome::Nothing);
    };
    Ok(script::run_script(&mine, repo_root, worktree))
}

/// What the project's file asks of a checkout, for the window to show.
#[derive(Serialize)]
pub(super) struct ProjectScriptsReport {
    pub(super) root: String,
    /// Absolute path of the file, so the window can name it.
    pub(super) file: String,
    pub(super) exists: bool,
    /// True when the file is there and this window's reader could not make
    /// sense of it — the state Orca calls `mayNeedUpdate`. A person whose setup
    /// stopped running has no other way to find out why.
    pub(super) unreadable: bool,
    /// The scripts as they will actually run, after the source policy.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) setup: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) archive: Option<String>,
    /// `None` when the policy is to ask.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) runs_setup: Option<bool>,
    pub(super) source: zerocode_core::ScriptSource,
    pub(super) setup_run_policy: zerocode_core::SetupRunPolicy,
    pub(super) local_setup: Option<String>,
    pub(super) local_archive: Option<String>,
    pub(super) default_tabs: usize,
    pub(super) trust: &'static str,
}

pub(super) fn project_scripts_for(
    repository: &settings::SettingsRepository,
    config_root: &Path,
    context: &ActiveProjectContext,
) -> Result<ProjectScriptsReport, String> {
    let settings = stored_project_script_settings(
        repository,
        &context.repo_root,
        Some(context.checkout.as_path()),
    )?;
    let file = script::read_project_file(&context.checkout);
    let held = file.clone().unwrap_or_default();
    Ok(ProjectScriptsReport {
        root: context.repo_root.to_string_lossy().into_owned(),
        file: script::project_file_path(&context.checkout)
            .to_string_lossy()
            .into_owned(),
        exists: file.is_some(),
        unreadable: held.unreadable,
        setup: zerocode_core::effective_script(
            &held,
            settings.local_setup.as_deref(),
            zerocode_core::ProjectScript::Setup,
            settings.source,
        ),
        archive: zerocode_core::effective_script(
            &held,
            settings.local_archive.as_deref(),
            zerocode_core::ProjectScript::Archive,
            settings.source,
        ),
        runs_setup: zerocode_core::runs_setup_on_create(settings.setup_run_policy),
        source: settings.source,
        setup_run_policy: settings.setup_run_policy,
        local_setup: settings.local_setup,
        local_archive: settings.local_archive,
        default_tabs: held.default_tabs.len(),
        trust: match repo_trust_content(&context.checkout) {
            Some(content) => {
                match repo_trust_standing_of(config_root, &context.repo_root, &content) {
                    zerocode_core::TrustStanding::Trusted => "trusted",
                    zerocode_core::TrustStanding::NothingToRun => "nothing",
                    zerocode_core::TrustStanding::Ask { .. } => "ask",
                }
            }
            None => "unreadable",
        },
    })
}

/// What a workspace opens with, and whether its commands may run.
#[derive(Serialize)]
pub(super) struct DefaultTabsReport {
    pub(super) tabs: Vec<zerocode_core::project::DefaultTab>,
    /// `Some(true)` run them, `Some(false)` open the tabs and leave them empty,
    /// `None` the policy is to ask and nobody has been asked yet.
    ///
    /// The same three answers the setup script gets, from the same setting.
    /// Orca ties them together too (`getDefaultTabsLaunch`,
    /// out/main/index.js:69371, resolves `runCommands` through
    /// `shouldRunSetupForCreate` and the shared command-source policy) — and
    /// the tie is the point rather than a convenience. A repository declaring
    /// `defaultTabs` is a file in a checkout asking this machine to run
    /// commands, which is exactly the thing the setup policy already exists to
    /// answer. A second door with its own default would let a repo route
    /// around the answer the person already gave.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) runs_commands: Option<bool>,
    /// Whether this workspace has already been opened with them. Applied once,
    /// as Orca applies them once (`defaultTerminalTabsAppliedByWorktreeId`) —
    /// otherwise every visit to a workspace stacks another set of tabs.
    pub(super) applied: bool,
}

/// The workspaces that have already been opened with their default tabs.
pub(super) fn applied_default_tabs_file(config_root: &Path) -> PathBuf {
    config_root.join(artifact_file::DEFAULT_TABS_APPLIED)
}

pub(super) fn stored_applied_default_tabs(config_root: &Path) -> Vec<String> {
    std::fs::read_to_string(applied_default_tabs_file(config_root))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

pub(super) fn clear_project_script_fields(settings: &mut ProjectSettings) {
    settings.local_setup = None;
    settings.local_archive = None;
    settings.source = zerocode_core::ScriptSource::default();
    settings.setup_run_policy = zerocode_core::SetupRunPolicy::default();
}

/// Apply a policy-only edit and promote the one legacy checkout record that
/// older versions could have written.
///
/// Local setup/archive scripts are intentionally absent from this patch. A
/// policy picker cannot erase text it never displayed. The pure map transform
/// is also the migration seam used by tests; disk I/O stays in the command.
pub(super) fn apply_project_script_policy(
    held: &mut BTreeMap<String, ProjectSettings>,
    repo_root: &Path,
    checkout: &Path,
    source: zerocode_core::ScriptSource,
    setup_run_policy: zerocode_core::SetupRunPolicy,
) {
    let root_key = project_settings_key(repo_root);
    let mut legacy_keys = Vec::new();
    for path in [repo_root, checkout] {
        for candidate in [
            path.to_string_lossy().into_owned(),
            project_settings_key(path),
        ] {
            if candidate != root_key && !legacy_keys.contains(&candidate) {
                legacy_keys.push(candidate);
            }
        }
    }

    let root_speaks = held
        .get(&root_key)
        .is_some_and(ProjectSettings::speaks_about_scripts);
    let inherited = (!root_speaks)
        .then(|| {
            legacy_keys
                .iter()
                .find_map(|key| held.get(key).filter(|row| row.speaks_about_scripts()))
                .cloned()
        })
        .flatten();

    let entry = held.entry(root_key.clone()).or_default();
    if let Some(legacy) = inherited {
        entry.local_setup = legacy.local_setup;
        entry.local_archive = legacy.local_archive;
        entry.source = legacy.source;
        entry.setup_run_policy = legacy.setup_run_policy;
    }
    entry.source = source;
    entry.setup_run_policy = setup_run_policy;

    for key in legacy_keys {
        if let Some(entry) = held.get_mut(&key) {
            clear_project_script_fields(entry);
        }
        if held.get(&key).is_some_and(ProjectSettings::says_nothing) {
            held.remove(&key);
        }
    }
}

pub(super) fn update_project_script_policy(
    repository: &settings::SettingsRepository,
    repo_root: &Path,
    checkout: &Path,
    source: zerocode_core::ScriptSource,
    setup_run_policy: zerocode_core::SetupRunPolicy,
) -> Result<(), String> {
    mutate_project_settings(repository, |held| {
        apply_project_script_policy(held, repo_root, checkout, source, setup_run_policy);
        Ok(())
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum ProjectScriptField {
    Source,
    SetupRunPolicy,
    LocalSetup,
    LocalArchive,
}

/* ---- how each agent is launched ------------------------------------------
 *
 * The per-agent argument and environment overrides. The table of defaults and
 * the judgement about what a launch adds up to are in
 * `zerocode-core::launch`; what lives here is where a person's edit is kept
 * and the pair of commands that read and write it.
 *
 * Keyed by agent id, and only the agents somebody has actually edited are in
 * the file — an absent key is "no opinion", which is a different thing from an
 * emptied field and the reason the override's members are `Option`. */

pub(super) const AGENT_LAUNCH_DOCUMENT_FILE: &str = settings_file::AGENT_LAUNCH;
pub(super) type AgentLaunchOverrides = BTreeMap<String, zerocode_core::LaunchOverride>;

/// Import the raw pre-repository map once, from this repository's own root.
///
/// The old file stays untouched as a rollback source. A malformed legacy file
/// is reported and retried at the next boot instead of being silently replaced
/// by an empty canonical document.
pub(super) fn migrate_legacy_agent_launches(
    repository: &settings::SettingsRepository,
) -> Result<(), String> {
    let read = repository
        .read_json::<AgentLaunchOverrides>(AGENT_LAUNCH_DOCUMENT_FILE)
        .map_err(|error| error.to_string())?;
    if read.value.is_some() {
        return complete_legacy_migration(repository, LegacyMigration::AgentLaunch);
    }

    let legacy = if legacy_migration_completed(repository, LegacyMigration::AgentLaunch)? {
        AgentLaunchOverrides::new()
    } else {
        let legacy_file = repository.root().join(legacy_settings_file::AGENT_LAUNCH);
        match std::fs::read_to_string(&legacy_file) {
            Ok(text) => serde_json::from_str(&text).map_err(|error| {
                format!(
                    "could not parse legacy agent launch settings at {}: {error}",
                    legacy_file.display()
                )
            })?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                AgentLaunchOverrides::new()
            }
            Err(error) => {
                return Err(format!(
                    "could not read legacy agent launch settings at {}: {error}",
                    legacy_file.display()
                ));
            }
        }
    };
    repository
        .mutate_json(
            AGENT_LAUNCH_DOCUMENT_FILE,
            || legacy,
            |_current: &mut AgentLaunchOverrides| {},
        )
        .map_err(|error| error.to_string())?;
    complete_legacy_migration(repository, LegacyMigration::AgentLaunch)
}

pub(super) fn stored_launch_overrides(
    repository: &settings::SettingsRepository,
) -> Result<AgentLaunchOverrides, String> {
    Ok(repository
        .read_json(AGENT_LAUNCH_DOCUMENT_FILE)
        .map_err(|error| error.to_string())?
        .value
        .unwrap_or_default())
}

pub(super) fn stored_launch_override(
    repository: &settings::SettingsRepository,
    agent: &str,
) -> Result<Option<zerocode_core::LaunchOverride>, String> {
    Ok(stored_launch_overrides(repository)?.remove(agent))
}

/// Every agent's plan as it stands, for the settings pane.
///
/// The plan rather than the override: what the pane has to show is what will
/// actually happen, and for an agent nobody has touched that is the default.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct AgentLaunchRow {
    pub(super) agent: String,
    /// Written as one line the way a person types it, not as the split argv —
    /// the field they edit is a command line.
    pub(super) args: String,
    /// The environment override as the LINE a person edits, for the same
    /// reason `args` is a line: the field is a line, and rendering it from the
    /// resolved pairs is what lets the field show what is stored.
    pub(super) env_line: String,
    pub(super) env: Vec<(String, String)>,
    pub(super) permission: zerocode_core::PermissionMode,
    /// False for an agent with no way to be told; the pane then says nothing
    /// rather than implying a setting exists.
    pub(super) has_switch: bool,
    /// True when this row is the measured default rather than somebody's edit.
    pub(super) is_default: bool,
}

pub(super) fn agent_launch_rows(held: &AgentLaunchOverrides) -> Vec<AgentLaunchRow> {
    zerocode_core::AGENT_SPECS
        .iter()
        .map(|spec| {
            let mine = held.get(spec.id);
            let plan = zerocode_core::launch_plan(spec.id, mine);
            AgentLaunchRow {
                agent: spec.id.to_string(),
                // The LINE as stored, never the split words re-joined: a
                // re-join strips the quotes that made `"path with spaces"`
                // one argument, and the next save of this row would then
                // store the corrupted spelling (P0-9).
                args: zerocode_core::launch_args_line(spec.id, mine),
                env_line: zerocode_core::launch_env_line(spec.id, mine),
                env: plan.env,
                permission: plan.permission,
                has_switch: zerocode_core::has_permission_switch(spec.id),
                is_default: mine.is_none(),
            }
        })
        .collect()
}

/// Save one agent's launch line, or clear it back to the default.
///
/// `None` for a field means "no opinion, use the default"; `Some("")` means
/// "ask me". Both reach here, and conflating them is how a person who cleared
/// the field gets the bypass flag back on the next launch.
/// One half of a launch edit — the three things a caller can mean about a
/// field it did or did not touch.
///
/// The distinction is why this is not a plain `Option`. Editing the args field
/// says nothing about env, and must LEAVE it; the reset control clears BOTH;
/// and a value SETS one. A single command whose `null` meant "clear" would
/// erase whatever the other field had put there — the exact data loss a
/// second field makes reachable. Over the wire, `null` and "key absent" are
/// one `None` to serde, so the three states cannot ride one optional — each
/// half gets its own command instead, and the command's name is what scopes
/// the `None`.
pub(super) enum LaunchEdit<T> {
    /// This call is not about this half. Leave whatever is stored.
    Keep,
    /// Put this half back to the measured default.
    Clear,
    /// Set this half to a value.
    Set(T),
}

impl<T> LaunchEdit<T> {
    /// The two states one half-scoped command can send: a value sets, its
    /// absence clears. `Keep` never rides the wire — it is what the OTHER
    /// half's command means for this one.
    pub(super) fn from_half(field: Option<T>) -> Self {
        match field {
            None => Self::Clear,
            Some(value) => Self::Set(value),
        }
    }
}

pub(super) fn save_agent_launch_in(
    repository: &settings::SettingsRepository,
    agent: String,
    args: LaunchEdit<String>,
    env: LaunchEdit<Vec<(String, String)>>,
) -> Result<Vec<AgentLaunchRow>, String> {
    agent_spec(&agent).ok_or_else(|| format!("{agent}은(는) 이 창이 모르는 에이전트입니다"))?;
    // Refused at the door, the way Orca refuses the plan built from it
    // (`planAgentCliArgsSuffix` — "CLI arguments are invalid"): a stored
    // unclosed quote would launch with silently different flags every time.
    if let LaunchEdit::Set(written) = &args
        && let Err(error) = zerocode_core::split_command_line(written)
    {
        return Err(format!("CLI 인자가 올바르지 않습니다: {error}"));
    }
    let committed = repository
        .mutate_json(
            AGENT_LAUNCH_DOCUMENT_FILE,
            AgentLaunchOverrides::new,
            |held: &mut AgentLaunchOverrides| {
                // Merge, not replace. Each half is touched only when the edit
                // was ABOUT that half, so editing args from one field cannot
                // erase an env override set from the other.
                // `apply_agent_permission_mode` already treats this document
                // per-half; this matches it.
                let mut over = held.get(&agent).cloned().unwrap_or_default();
                match &args {
                    LaunchEdit::Keep => {}
                    LaunchEdit::Clear => over.args = None,
                    // Sanitised at the door rather than at launch: an agent
                    // that refuses a flag should never have it in the file
                    // either, or the pane would show a line that is not the
                    // one that runs.
                    LaunchEdit::Set(written) => {
                        over.args = Some(zerocode_core::sanitize_launch_args(&agent, written));
                    }
                }
                match env {
                    LaunchEdit::Keep => {}
                    LaunchEdit::Clear => over.env = None,
                    LaunchEdit::Set(pairs) => over.env = Some(pairs),
                }
                // Both halves gone is the reset — the row goes back to the
                // measured default rather than lingering as an empty opinion.
                if over.args.is_none() && over.env.is_none() {
                    held.remove(&agent);
                } else {
                    held.insert(agent, over);
                }
            },
        )
        .map_err(|error| error.to_string())?;
    Ok(agent_launch_rows(&committed.value))
}

/// The env line's grammar cap. Orca caps its draft at the same figure
/// (`AGENT_DEFAULT_ENV_DRAFT_MAX_BYTES`) — a "variable" past this size is a
/// file that took a wrong turn.
pub(super) const ENV_LINE_MAX_BYTES: usize = 8 * 1024;

/// `KEY=VALUE` words out of one typed line, quote-aware.
///
/// The same splitter the args line goes through, so `TOKEN="a b"` is one
/// pair — and the same refusal shape: a line this door cannot read is refused
/// with the reason, never stored half-parsed. Names take the POSIX letter:
/// `[A-Za-z_][A-Za-z0-9_]*`, because a name outside it cannot reach a child
/// process through `command.env` on every platform we run on.
pub(super) fn parse_env_line(line: &str) -> Result<Vec<(String, String)>, String> {
    if line.len() > ENV_LINE_MAX_BYTES {
        return Err(format!(
            "환경 변수 줄이 너무 깁니다 ({}바이트, 최대 {}바이트)",
            line.len(),
            ENV_LINE_MAX_BYTES
        ));
    }
    let words = zerocode_core::split_command_line(line)
        .map_err(|error| format!("환경 변수 줄이 올바르지 않습니다: {error}"))?;
    let mut pairs = Vec::new();
    for word in words {
        let Some((name, value)) = word.split_once('=') else {
            return Err(format!("KEY=VALUE 꼴이 아닙니다: {word}"));
        };
        let valid = !name.is_empty()
            && name
                .chars()
                .next()
                .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
            && name
                .chars()
                .all(|held| held.is_ascii_alphanumeric() || held == '_');
        if !valid {
            return Err(format!("환경 변수 이름이 올바르지 않습니다: {name}"));
        }
        pairs.push((name.to_string(), value.to_string()));
    }
    Ok(pairs)
}

/// Apply Orca's global Yolo/Manual choice in one document transaction.
///
/// The core owns which fields are permission switches and which values count
/// as custom. This boundary owns only persistence: all known terminal agents
/// move together, or none of them do.
pub(super) fn set_agent_permission_mode_in(
    repository: &settings::SettingsRepository,
    mode: zerocode_core::AgentPermissionMode,
) -> Result<Vec<AgentLaunchRow>, String> {
    let committed = repository
        .mutate_json(
            AGENT_LAUNCH_DOCUMENT_FILE,
            AgentLaunchOverrides::new,
            |held: &mut AgentLaunchOverrides| {
                for spec in &zerocode_core::AGENT_SPECS {
                    if !zerocode_core::has_permission_switch(spec.id) {
                        continue;
                    }
                    let current = held.get(spec.id).cloned();
                    match zerocode_core::apply_agent_permission_mode(
                        spec.id,
                        current.as_ref(),
                        mode,
                    ) {
                        Some(next) => {
                            held.insert(spec.id.to_string(), next);
                        }
                        None => {
                            held.remove(spec.id);
                        }
                    }
                }
            },
        )
        .map_err(|error| error.to_string())?;
    Ok(agent_launch_rows(&committed.value))
}

pub(super) fn quick_commands_file(config_root: &Path) -> PathBuf {
    config_root.join(artifact_file::QUICK_COMMANDS)
}

pub(super) fn stored_quick_commands(config_root: &Path) -> Vec<QuickCommand> {
    std::fs::read_to_string(quick_commands_file(config_root))
        .ok()
        .and_then(|text| serde_json::from_str::<Vec<QuickCommand>>(&text).ok())
        .unwrap_or_default()
}

pub(super) fn write_quick_commands(
    config_root: &Path,
    rows: &[QuickCommand],
) -> Result<(), String> {
    let file = quick_commands_file(config_root);
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let text = serde_json::to_string_pretty(rows).map_err(|error| error.to_string())?;
    std::fs::write(&file, text).map_err(|error| error.to_string())
}

/// Save one, by id — with Orca's own refusals, at the door.
pub(super) fn validate_quick_command(command: &QuickCommand) -> Result<(), String> {
    if command.label.trim().is_empty() || command.body.trim().is_empty() {
        return Err("라벨과 내용이 모두 있어야 합니다".to_string());
    }
    if command.label.chars().count() > 80 {
        return Err("라벨은 80자까지입니다".to_string());
    }
    if let Some(agent) = command.agent.as_deref() {
        let spec = agent_spec(agent)
            .ok_or_else(|| format!("{agent}은(는) 이 창이 모르는 에이전트입니다"))?;
        if !spec.capabilities().takes_prompt_at_start() {
            return Err(format!(
                "{}은(는) 시작 시 프롬프트를 받을 수 없어 빠른 명령이 되지 않습니다",
                spec.name
            ));
        }
    }
    Ok(())
}

/* ---- plan usage ------------------------------------------------------------
 *
 * The status bar's provider segment. The scan itself — the hidden terminal,
 * the `/usage` keystrokes, the parse — lives in `usage.rs` with the
 * measurements that justify it; this block owns the cache, the clock, and
 * the command. */
