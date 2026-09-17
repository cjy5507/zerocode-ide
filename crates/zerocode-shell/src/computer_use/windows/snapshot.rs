//! One observation of one window: the tree an agent reads, the elements the
//! next action needs, the picture, and the JSON it all becomes.

use std::collections::BTreeMap;
use std::rc::Rc;

use windows::Win32::UI::Accessibility::IUIAutomationElement;
use zerocode_core::computer_use_protocol::cache::CachedSnapshot;
use zerocode_core::computer_use_protocol::click_plan::Recipient;
use zerocode_core::computer_use_protocol::marks;
use zerocode_core::computer_use_protocol::render::{
    self, Described, Rect, RenderNode, RenderedRecord, TreeSource,
};
use zerocode_core::computer_use_protocol::{
    MAX_TREE_DEPTH, MAX_TREE_NODES, Observation, ProviderError, ScreenshotMetadata,
    ScreenshotPayload, ScreenshotStatus, SnapshotBody, Truncation, WindowInfo, error_code,
};

use super::super::screenshot_png::bounded_png;
use super::apps::AppDescriptor;
use super::capture;
use super::desktop_windows::{self, WindowCandidate};
use super::uia::{self, ElementFacts, UiaClient};

pub(super) struct Snapshot {
    pub id: String,
    pub app: AppDescriptor,
    pub window: WindowCandidate,
    pub title: String,
    pub tree_text: String,
    pub records: BTreeMap<usize, RenderedRecord<usize>>,
    /// The live elements the records point into, by handle.
    pub elements: Vec<IUIAutomationElement>,
    pub focused_element_id: Option<usize>,
    pub truncated: bool,
    pub max_depth_reached: bool,
    /// Cross-process UI Automation fetches this observation made.
    pub fetches: usize,
    pub screenshot: Option<ScreenshotPayload>,
    pub screenshot_status: ScreenshotStatus,
}

impl Snapshot {
    /// The record and live element at `index`, worded like the helper when
    /// the index is not in this snapshot.
    pub fn element(
        &self,
        index: usize,
    ) -> Result<(&RenderedRecord<usize>, &IUIAutomationElement), ProviderError> {
        let record = self.records.get(&index).ok_or_else(|| {
            ProviderError::new(
                error_code::ELEMENT_NOT_FOUND,
                format!(
                    "element {index} is not in the current cached snapshot for {}; run get-app-state again and use a fresh element index",
                    self.app.name
                ),
            )
        })?;
        let element = self.elements.get(record.handle).ok_or_else(|| {
            ProviderError::new(
                error_code::ELEMENT_NOT_FOUND,
                format!("element {index} is gone"),
            )
        })?;
        Ok((record, element))
    }

    pub fn focused(&self) -> Option<(&RenderedRecord<usize>, &IUIAutomationElement)> {
        self.element(self.focused_element_id?).ok()
    }

    /// The screen point at the middle of a record's frame.
    pub fn center_of(&self, record: &RenderedRecord<usize>) -> Option<(f64, f64)> {
        let frame = record.local_frame?;
        Some((
            self.window.bounds.x + frame.mid_x(),
            self.window.bounds.y + frame.mid_y(),
        ))
    }

    /// A window-local point on the screen.
    pub fn screen_point(&self, x: f64, y: f64) -> (f64, f64) {
        (self.window.bounds.x + x, self.window.bounds.y + y)
    }

    /// Who a synthetic click must land on: the window, by the process that
    /// owns its handle — a packaged app's frame host, not the app.
    pub fn recipient(&self) -> Recipient {
        Recipient {
            owner_pid: self.window.host_pid,
            window_id: self.window.id(),
        }
    }

    fn window_info(&self) -> WindowInfo {
        WindowInfo {
            id: self.window.id(),
            title: self.title.clone(),
            x: self.window.bounds.x.round() as i64,
            y: self.window.bounds.y.round() as i64,
            width: self.window.bounds.width.round() as i64,
            height: self.window.bounds.height.round() as i64,
            is_minimized: false,
            is_offscreen: false,
            screen_index: self.window.screen_index,
            platform: serde_json::json!({ "topmost": self.window.topmost }),
        }
    }

    /// `renderSnapshot`, without the faces a marked look plans from.
    pub fn observation(&self) -> Observation {
        self.observation_with(false)
    }

    /// `renderSnapshot`; with `elements`, every indexed element with a frame
    /// (`marks::element_faces`), which only a look that asked for them carries.
    pub fn observation_with(&self, elements: bool) -> Observation {
        Observation {
            snapshot: SnapshotBody {
                id: self.id.clone(),
                app: self.app.identity(),
                window: self.window_info(),
                coordinate_space: "window",
                tree_text: self.tree_text.clone(),
                element_count: self.records.len(),
                focused_element_id: self.focused_element_id,
                truncation: Truncation {
                    truncated: self.truncated,
                    max_nodes: MAX_TREE_NODES,
                    max_depth: MAX_TREE_DEPTH,
                    max_depth_reached: self.max_depth_reached,
                },
                elements: elements.then(|| marks::element_faces(&self.records)),
            },
            screenshot: self.screenshot.clone(),
            screenshot_status: self.screenshot_status.clone(),
        }
    }
}

/// A snapshot as the cache keeps it: shared, and without the picture — the
/// cache validates element identity, and a megabyte of base64 per entry
/// would be memory spent on nothing. `Rc`, not `Arc`: the cache and its COM
/// elements never leave the provider thread.
#[derive(Clone)]
pub(super) struct SharedSnapshot(pub Rc<Snapshot>);

impl SharedSnapshot {
    pub fn from_observed(snapshot: &Snapshot) -> Self {
        Self(Rc::new(Snapshot {
            id: snapshot.id.clone(),
            app: snapshot.app.clone(),
            window: snapshot.window.clone(),
            title: snapshot.title.clone(),
            tree_text: String::new(),
            records: snapshot.records.clone(),
            elements: snapshot.elements.clone(),
            focused_element_id: snapshot.focused_element_id,
            truncated: snapshot.truncated,
            max_depth_reached: snapshot.max_depth_reached,
            fetches: snapshot.fetches,
            screenshot: None,
            screenshot_status: ScreenshotStatus::Skipped,
        }))
    }
}

impl CachedSnapshot for SharedSnapshot {
    fn id(&self) -> &str {
        &self.0.id
    }

    fn window_id(&self) -> u64 {
        self.0.window.id()
    }

    fn element_signature(&self, index: usize) -> Option<&str> {
        self.0
            .records
            .get(&index)
            .map(|record| record.signature.as_str())
    }

    fn element_name(&self, index: usize) -> Option<&str> {
        self.0
            .records
            .get(&index)
            .and_then(|record| record.name.as_deref().or(record.placeholder.as_deref()))
    }

    fn element_context(&self, index: usize) -> Option<&str> {
        self.0
            .records
            .get(&index)
            .and_then(|record| record.context.as_deref())
    }

    fn element_frame(&self, index: usize) -> Option<Rect> {
        self.0
            .records
            .get(&index)
            .and_then(|record| record.local_frame)
    }
}

/// What `build` is asked for.
pub(super) struct BuildRequest<'a> {
    pub app: &'a AppDescriptor,
    pub include_screenshot: bool,
    pub window_id: Option<u64>,
    pub window_index: Option<usize>,
    pub restore_window: bool,
}

/// The renderer's view of the UIA tree, fetched one level at a time as the
/// walk descends: handles are indexes into one store of live elements, so
/// comparing two costs nothing, and nothing below a parent exists here until
/// the renderer asks for it.
///
/// Acquisition is bounded twice over. The renderer stops describing at
/// `MAX_TREE_NODES` indexed elements; and no level is kept wider than the
/// store's remaining room under [`ACQUISITION_CEILING`], so a list of fifty
/// thousand rows costs one round trip and a bounded array, never fifty
/// thousand live proxies. Either cut reads as `truncated` in the snapshot.
struct UiaTree<'a> {
    client: &'a UiaClient,
    elements: Vec<IUIAutomationElement>,
    roles: Vec<&'static str>,
    focused: Vec<bool>,
    /// The children of each element, once fetched; `None` until asked.
    children: Vec<Option<Vec<usize>>>,
    window_origin: (f64, f64),
    browser: bool,
    /// A level was wider than the room left: the tree is cut.
    cut: bool,
    /// Cross-process fetches made — the number the performance report prints.
    fetches: usize,
}

/// The most live elements one observation will hold. Elided groups do not
/// count against the renderer's cap but do occupy the store, so the store
/// is allowed more than the cap; past this, a level is cut short.
const ACQUISITION_CEILING: usize = MAX_TREE_NODES * 2;

impl UiaTree<'_> {
    fn push(&mut self, element: IUIAutomationElement) -> usize {
        self.elements.push(element);
        self.roles.push("AXUnknown");
        self.focused.push(false);
        self.children.push(None);
        self.elements.len() - 1
    }

    /// The children of `handle`, fetched on first ask, within the room left.
    fn children_of(&mut self, handle: usize) -> Vec<usize> {
        if let Some(children) = &self.children[handle] {
            return children.clone();
        }
        let budget = ACQUISITION_CEILING.saturating_sub(self.elements.len());
        let fetched = self.client.children_of(&self.elements[handle], budget);
        self.fetches += 1;
        if fetched.cut {
            self.cut = true;
        }
        let children: Vec<usize> = fetched
            .elements
            .into_iter()
            .map(|child| self.push(child))
            .collect();
        self.children[handle] = Some(children.clone());
        children
    }

    fn node_of(
        facts: &ElementFacts,
        child_count: usize,
        web_area_depth: Option<usize>,
    ) -> RenderNode {
        let text_like = matches!(
            facts.role,
            "AXTextField" | "AXTextArea" | "AXComboBox" | "AXSearchField"
        );
        let mut traits = Vec::new();
        if facts.selected {
            traits.push("selected".to_string());
        }
        if facts.expanded {
            traits.push("expanded".to_string());
        }
        if !facts.enabled {
            traits.push("disabled".to_string());
        }
        if facts.settable
            && matches!(
                facts.role,
                "AXCheckBox"
                    | "AXComboBox"
                    | "AXRadioButton"
                    | "AXSlider"
                    | "AXTextArea"
                    | "AXTextField"
                    | "AXIncrementor"
            )
        {
            traits.push("settable".to_string());
        }
        RenderNode {
            role: facts.role.to_string(),
            role_description: facts.role_description.clone(),
            title: facts.name.clone(),
            label: if text_like {
                None
            } else {
                facts.help_text.clone()
            },
            link_text: None,
            value: facts.value.clone(),
            placeholder: if text_like {
                facts.help_text.clone()
            } else {
                None
            },
            url: None,
            traits,
            raw_actions: facts.actions.clone(),
            child_count,
            summary: None,
            row_summary: None,
            web_area_depth,
        }
    }
}

impl TreeSource for UiaTree<'_> {
    type Handle = usize;

    fn describe(&mut self, handle: &usize, ancestors: &[usize]) -> Described<usize> {
        let element = self.elements[*handle].clone();
        let facts = uia::describe(&element, self.browser);
        self.roles[*handle] = facts.role;
        self.focused[*handle] = facts.has_focus;
        let children = self.children_of(*handle);
        let web_area_depth = if facts.role == "AXWebArea" {
            Some(0)
        } else {
            ancestors
                .iter()
                .rev()
                .position(|ancestor| self.roles[*ancestor] == "AXWebArea")
                .map(|distance| distance + 1)
        };
        let local_frame = facts.frame.map(|frame| {
            Rect::new(
                frame.x - self.window_origin.0,
                frame.y - self.window_origin.1,
                frame.width,
                frame.height,
            )
        });
        Described {
            node: Self::node_of(&facts, children.len(), web_area_depth),
            children,
            local_frame,
        }
    }

    fn peek(&mut self, handle: &usize) -> RenderNode {
        let facts = uia::describe(&self.elements[*handle], self.browser);
        let child_count = self.children_of(*handle).len();
        Self::node_of(&facts, child_count, None)
    }

    fn is_focused(&mut self, handle: &usize) -> bool {
        self.focused[*handle]
    }
}

fn no_window(app: &AppDescriptor) -> ProviderError {
    ProviderError::new(
        error_code::WINDOW_NOT_FOUND,
        format!("app '{}' has no on-screen window", app.name),
    )
}

/// `buildSnapshot`.
pub(super) fn build(
    client: &UiaClient,
    request: BuildRequest<'_>,
) -> Result<Snapshot, ProviderError> {
    let app = request.app;
    let mut candidates = desktop_windows::candidates(app.pid);
    if candidates.is_empty() {
        return Err(no_window(app));
    }
    let foreground_title = desktop_windows::foreground()
        .filter(|(pid, _)| *pid == app.pid)
        .and_then(|(_, root)| {
            candidates
                .iter()
                .find(|candidate| candidate.hwnd == root)
                .map(|candidate| candidate.title.clone())
        });
    let mut window = desktop_windows::resolve(
        &candidates,
        foreground_title.as_deref(),
        request.window_id,
        request.window_index,
    )
    .ok_or_else(|| {
        if request.window_id.is_some() || request.window_index.is_some() {
            ProviderError::new(
                error_code::WINDOW_NOT_FOUND,
                "could not match accessibility window to requested window; run get-app-state again or retry without a window selector",
            )
        } else {
            no_window(app)
        }
    })?;
    if request.restore_window {
        desktop_windows::bring_to_front(&window);
        // Restoring moves a minimized window; read it again where it is now.
        candidates = desktop_windows::candidates(app.pid);
        if let Some(moved) = candidates
            .iter()
            .find(|candidate| candidate.hwnd == window.hwnd)
        {
            window = moved.clone();
        }
    }
    let root = client.window_root(window.handle()).map_err(|error| {
        if error.message.contains("0x80070005") {
            ProviderError::new(
                error_code::PERMISSION_DENIED,
                format!(
                    "app '{}' refused accessibility access (it runs elevated above ZeroCode)",
                    app.name
                ),
            )
        } else {
            error
        }
    })?;
    let mut tree = UiaTree {
        client,
        elements: Vec::with_capacity(MAX_TREE_NODES),
        roles: Vec::with_capacity(MAX_TREE_NODES),
        focused: Vec::with_capacity(MAX_TREE_NODES),
        children: Vec::with_capacity(MAX_TREE_NODES),
        window_origin: (window.bounds.x, window.bounds.y),
        browser: app.is_browser(),
        cut: false,
        fetches: 0,
    };
    let root_handle = tree.push(root);
    let browser = tree.browser;
    let rendered = render::walk(&mut tree, &root_handle, browser);
    let title = if window.title.is_empty() {
        app.name.clone()
    } else {
        window.title.clone()
    };
    let tree_text = render::render_tree_text(
        &app.identity(),
        &title,
        &rendered.lines,
        rendered.focused_summary.as_deref(),
    );

    let (screenshot, screenshot_status) = if request.include_screenshot {
        match capture::capture(window.handle(), &window.bounds).and_then(|captured| {
            bounded_png(&captured.image).map(|png| (png, captured.engine))
        }) {
            Some((png, engine)) => {
                use base64::Engine as _;
                let payload = ScreenshotPayload {
                    data: base64::engine::general_purpose::STANDARD.encode(&png.data),
                    format: "png",
                    width: png.width,
                    height: png.height,
                    scale: f64::from(png.width) / window.bounds.width.max(1.0),
                };
                (
                    Some(payload),
                    ScreenshotStatus::Captured(ScreenshotMetadata {
                        engine: engine.to_string(),
                        window_id: window.id(),
                    }),
                )
            }
            None => (
                None,
                ScreenshotStatus::Failed {
                    message: "window screenshot capture returned no image; retry with --no-screenshot if accessibility state is sufficient.".into(),
                    metadata: ScreenshotMetadata {
                        engine: "unknown".into(),
                        window_id: window.id(),
                    },
                },
            ),
        }
    } else {
        (None, ScreenshotStatus::Skipped)
    };

    Ok(Snapshot {
        id: uuid::Uuid::new_v4().to_string(),
        app: app.clone(),
        window,
        title,
        tree_text,
        records: rendered.records,
        elements: tree.elements,
        focused_element_id: rendered.focused_element_id,
        truncated: rendered.truncated || tree.cut,
        max_depth_reached: rendered.max_depth_reached,
        fetches: tree.fetches,
        screenshot,
        screenshot_status,
    })
}
