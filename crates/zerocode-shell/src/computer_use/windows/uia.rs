//! The UI Automation client: a bounded tree fetch per observation, live
//! elements for the actions that follow.
//!
//! A tree is read one level at a time: a cache request that names every
//! property and pattern the renderer will ask about, scoped to the element
//! itself, is handed to `ElementFromHandleBuildCache` for the root and to
//! `FindAllBuildCache(TreeScope_Children)` for each parent the renderer
//! descends into. Reading a fetched element then touches no other process,
//! and — the point — nothing below a parent is materialised until the
//! renderer asks for it, so a 50,000-node document costs the round trips of
//! the 1,200 nodes the snapshot can show and not the whole page (which a
//! `TreeScope_Subtree` request built in the target's process, memory
//! unbounded, until UI Automation's own timeout gave up on it). The elements
//! come back in `AutomationElementMode_Full`, so each indexed record still
//! holds a live reference an action can invoke a pattern on.
//!
//! Roles are mapped onto the shared AX-style vocabulary once, here, so the
//! renderer in `zerocode_core::computer_use_protocol::render` needs no
//! second opinion about what a `TabItem` is.

use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
use windows::Win32::UI::Accessibility::{
    AutomationElementMode_Full, CUIAutomation8, ExpandCollapseState_Collapsed,
    ExpandCollapseState_Expanded, IUIAutomation, IUIAutomationCacheRequest, IUIAutomationCondition,
    IUIAutomationElement, IUIAutomationElement3, IUIAutomationExpandCollapsePattern,
    IUIAutomationInvokePattern, IUIAutomationRangeValuePattern, IUIAutomationScrollItemPattern,
    IUIAutomationScrollPattern, IUIAutomationSelectionItemPattern, IUIAutomationTextPattern,
    IUIAutomationTogglePattern, IUIAutomationValuePattern, ScrollAmount_LargeDecrement,
    ScrollAmount_LargeIncrement, ScrollAmount_NoAmount, TextPatternRangeEndpoint_End,
    TextPatternRangeEndpoint_Start, TextUnit_Character, ToggleState_On, TreeScope_Children,
    TreeScope_Element, UIA_AutomationIdPropertyId, UIA_BoundingRectanglePropertyId,
    UIA_ClassNamePropertyId, UIA_ControlTypePropertyId, UIA_ExpandCollapsePatternId,
    UIA_HasKeyboardFocusPropertyId, UIA_HelpTextPropertyId, UIA_InvokePatternId,
    UIA_IsControlElementPropertyId, UIA_IsEnabledPropertyId, UIA_IsKeyboardFocusablePropertyId,
    UIA_IsOffscreenPropertyId, UIA_IsPasswordPropertyId, UIA_LocalizedControlTypePropertyId,
    UIA_NamePropertyId, UIA_NativeWindowHandlePropertyId, UIA_ProcessIdPropertyId,
    UIA_RangeValuePatternId, UIA_ScrollItemPatternId, UIA_ScrollPatternId,
    UIA_SelectionItemPatternId, UIA_TextPatternId, UIA_TogglePatternId, UIA_ValuePatternId,
};
use windows::Win32::UI::WindowsAndMessaging::{GA_ROOT, GetAncestor};
use windows::core::{BSTR, IUnknown, Interface};
use zerocode_core::computer_use_protocol::render::{
    Rect, looks_like_secure_text, should_probe_secure_text_metadata,
};
use zerocode_core::computer_use_protocol::text_field::{
    FieldText, ReadFailure, Selection, TextField, WriteFailure,
};
use zerocode_core::computer_use_protocol::validate::ScrollDirection;
use zerocode_core::computer_use_protocol::{ProviderError, error_code};

use super::super::ComputerUseError;

pub(super) struct UiaClient {
    automation: IUIAutomation,
    /// Every property and pattern the renderer reads, for one element.
    element: IUIAutomationCacheRequest,
    /// The control view: the same filter the macOS helper's AX tree walks.
    control_view: IUIAutomationCondition,
}

fn com_error(context: &str, error: windows::core::Error) -> ComputerUseError {
    ComputerUseError::new(
        error_code::ACCESSIBILITY_ERROR,
        format!("{context}: {error}"),
    )
}

/// The properties and patterns the renderer and the actions read from the
/// cache. Anything not listed here costs a live round trip later.
const CACHED_PROPERTIES: &[windows::Win32::UI::Accessibility::UIA_PROPERTY_ID] = &[
    UIA_NamePropertyId,
    UIA_ControlTypePropertyId,
    UIA_LocalizedControlTypePropertyId,
    UIA_BoundingRectanglePropertyId,
    UIA_IsEnabledPropertyId,
    UIA_IsOffscreenPropertyId,
    UIA_HasKeyboardFocusPropertyId,
    UIA_IsKeyboardFocusablePropertyId,
    UIA_AutomationIdPropertyId,
    UIA_ClassNamePropertyId,
    UIA_HelpTextPropertyId,
    UIA_IsPasswordPropertyId,
    UIA_IsControlElementPropertyId,
    UIA_NativeWindowHandlePropertyId,
    UIA_ProcessIdPropertyId,
];

const CACHED_PATTERNS: &[windows::Win32::UI::Accessibility::UIA_PATTERN_ID] = &[
    UIA_InvokePatternId,
    UIA_ValuePatternId,
    UIA_TogglePatternId,
    UIA_SelectionItemPatternId,
    UIA_ExpandCollapsePatternId,
    UIA_ScrollPatternId,
    UIA_ScrollItemPatternId,
    UIA_TextPatternId,
    UIA_RangeValuePatternId,
];

impl UiaClient {
    pub fn new() -> Result<Self, ComputerUseError> {
        // SAFETY: COM is initialised on this thread by the caller
        // (`ComApartment`); creating the automation object and its cache
        // requests are ordinary in-process COM calls.
        unsafe {
            let automation: IUIAutomation =
                CoCreateInstance(&CUIAutomation8, None::<&IUnknown>, CLSCTX_INPROC_SERVER)
                    .map_err(|error| com_error("UI Automation is unavailable", error))?;
            let element = automation
                .CreateCacheRequest()
                .map_err(|error| com_error("could not create a cache request", error))?;
            for property in CACHED_PROPERTIES {
                element
                    .AddProperty(*property)
                    .map_err(|error| com_error("could not cache a property", error))?;
            }
            for pattern in CACHED_PATTERNS {
                element
                    .AddPattern(*pattern)
                    .map_err(|error| com_error("could not cache a pattern", error))?;
            }
            let control_view = automation
                .ControlViewCondition()
                .map_err(|error| com_error("could not build the control view", error))?;
            element
                .SetTreeFilter(&control_view)
                .map_err(|error| com_error("could not filter the tree", error))?;
            element
                .SetTreeScope(TreeScope_Element)
                .map_err(|error| com_error("could not scope the cache", error))?;
            element
                .SetAutomationElementMode(AutomationElementMode_Full)
                .map_err(|error| com_error("could not keep live elements", error))?;
            Ok(Self {
                automation,
                element,
                control_view,
            })
        }
    }

    /// The window's root element, with its own properties cached; its
    /// children come from [`Self::children_of`] as the renderer descends.
    pub fn window_root(&self, hwnd: HWND) -> Result<IUIAutomationElement, ComputerUseError> {
        // SAFETY: an in-process COM call on the object this client owns.
        unsafe {
            self.automation
                .ElementFromHandleBuildCache(hwnd, &self.element)
        }
        .map_err(|error| com_error("could not read the window's accessibility tree", error))
    }

    /// One level below `parent`, in the control view, each child with its
    /// properties cached — one cross-process round trip per parent, and
    /// never more than `budget` children kept: the rest of a very wide level
    /// is released here, and the caller says the tree was cut.
    pub fn children_of(&self, parent: &IUIAutomationElement, budget: usize) -> Children {
        // SAFETY: a live COM call scoped to direct children; the array and
        // its elements are released as they drop.
        let found = unsafe {
            parent.FindAllBuildCache(TreeScope_Children, &self.control_view, &self.element)
        };
        let Ok(array) = found else {
            return Children::default();
        };
        // SAFETY: reads of the array this call produced.
        let length = unsafe { array.Length() }.unwrap_or(0).max(0) as usize;
        let kept = length.min(budget);
        let elements = (0..kept)
            .filter_map(|index| {
                // SAFETY: `index` is within the array's reported length.
                unsafe { array.GetElement(index as i32) }.ok()
            })
            .collect();
        Children {
            elements,
            cut: length > kept,
        }
    }

    /// The element with keyboard focus RIGHT NOW, with its properties
    /// cached — read live before a keyboard action rather than trusted from
    /// the observation a moment earlier, because focus can move between the
    /// two and text must not land in a field the agent never saw.
    pub fn focused_element(&self) -> Option<IUIAutomationElement> {
        // SAFETY: as above.
        unsafe { self.automation.GetFocusedElementBuildCache(&self.element) }.ok()
    }

    /// Whether what is on top at the screen point is `target`, inside it, or
    /// around it — what a press there reaches, `MARK_HIT_DEPTH` parents
    /// either way. A click by a mark's number presses nothing covered.
    pub fn on_top(
        &self,
        target: &IUIAutomationElement,
        (x, y): (f64, f64),
    ) -> Result<bool, ComputerUseError> {
        #[allow(clippy::cast_possible_truncation)]
        let point = POINT {
            x: x.round() as i32,
            y: y.round() as i32,
        };
        // SAFETY: ordinary in-process COM calls on this thread's automation
        // object, as above.
        unsafe {
            let hit = self
                .automation
                .ElementFromPoint(point)
                .map_err(|error| com_error("could not hit-test the point", error))?;
            let walker = self
                .automation
                .RawViewWalker()
                .map_err(|error| com_error("could not walk the tree", error))?;
            let reaches = |from: &IUIAutomationElement, to: &IUIAutomationElement| {
                let mut node = Some(from.clone());
                for _ in 0..zerocode_core::computer_use::MARK_HIT_DEPTH {
                    let Some(current) = node else {
                        return false;
                    };
                    if self
                        .automation
                        .CompareElements(&current, to)
                        .is_ok_and(|same| same.as_bool())
                    {
                        return true;
                    }
                    node = walker.GetParentElement(&current).ok();
                }
                false
            };
            Ok(reaches(&hit, target) || reaches(target, &hit))
        }
    }
}

/// One fetched level: the children kept, and whether the level was wider
/// than the budget allowed.
#[derive(Default)]
pub(super) struct Children {
    pub elements: Vec<IUIAutomationElement>,
    pub cut: bool,
}

/// Whether a cached element is the platform's own secret field — UIA's
/// `IsPassword`, a hard signal, never a label's word.
pub(super) fn cached_is_password(element: &IUIAutomationElement) -> bool {
    // SAFETY: a cache read.
    cached_bool(unsafe { element.CachedIsPassword() }).unwrap_or(false)
}

/// The process a cached element belongs to.
pub(super) fn cached_process_id(element: &IUIAutomationElement) -> Option<u32> {
    // SAFETY: a cache read.
    unsafe { element.CachedProcessId() }
        .ok()
        .and_then(|pid| u32::try_from(pid).ok())
}

/// The top-level window a cached element lives in, by its native handle.
/// `None` for an element with no window handle of its own (most elements
/// inside a framework-drawn surface have only the frame's).
pub(super) fn cached_root_window(element: &IUIAutomationElement) -> Option<HWND> {
    // SAFETY: a cache read, then a plain ancestor walk on the handle it names.
    unsafe {
        let hwnd = element.CachedNativeWindowHandle().ok()?;
        if hwnd.0.is_null() {
            return None;
        }
        let root = GetAncestor(hwnd, GA_ROOT);
        Some(if root.0.is_null() { hwnd } else { root })
    }
}

/// What the renderer wants to know about one cached element.
#[derive(Debug, Clone, Default)]
pub(super) struct ElementFacts {
    pub role: &'static str,
    pub role_description: Option<String>,
    pub name: Option<String>,
    pub help_text: Option<String>,
    pub value: Option<String>,
    pub is_password: bool,
    pub enabled: bool,
    pub has_focus: bool,
    pub frame: Option<Rect>,
    pub actions: Vec<String>,
    pub selected: bool,
    pub expanded: bool,
    pub settable: bool,
    pub class_name: String,
}

fn cached_string(value: windows::core::Result<BSTR>) -> Option<String> {
    let text = value.ok()?.to_string();
    let trimmed = text.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn cached_bool(value: windows::core::Result<windows::core::BOOL>) -> Option<bool> {
    value.ok().map(|held| held.as_bool())
}

/// The UIA control type → the shared role vocabulary. A `Document` is web
/// content inside a known browser and a text area anywhere else; a `Pane`
/// that scrolls is a scroll area.
pub(super) fn role_for(control_type: i32, scrolls: bool, browser: bool) -> &'static str {
    match control_type {
        50000 => "AXButton",
        50001 => "AXGroup",
        50002 => "AXCheckBox",
        50003 => "AXComboBox",
        50004 => "AXTextField",
        50005 => "AXLink",
        50006 => "AXImage",
        50007 => "AXRow",
        50008 => "AXList",
        50009 => "AXMenu",
        50010 => "AXMenuBar",
        50011 => "AXMenuItem",
        50012 => "AXProgressIndicator",
        50013 => "AXRadioButton",
        50014 => "AXScrollBar",
        50015 => "AXSlider",
        50016 => "AXIncrementor",
        50017 => "AXGroup",
        50018 => "AXTabGroup",
        50019 => "AXTab",
        50020 => "AXStaticText",
        50021 => "AXToolbar",
        50022 => "AXGroup",
        50023 => "AXOutline",
        50024 => "AXOutlineRow",
        50025 => "AXUnknown",
        50026 => "AXGroup",
        50027 => "AXUnknown",
        50028 => "AXTable",
        50029 => "AXRow",
        50030 if browser => "AXWebArea",
        50030 => "AXTextArea",
        50031 => "AXPopUpButton",
        50032 => "AXWindow",
        50033 if scrolls => "AXScrollArea",
        50033 => "AXGroup",
        50034 => "AXGroup",
        50035 => "AXButton",
        50036 => "AXTable",
        50037 => "AXGroup",
        50038 => "AXUnknown",
        50039 => "AXGroup",
        50040 => "AXToolbar",
        _ => "AXUnknown",
    }
}

fn cached_pattern<T: Interface>(
    element: &IUIAutomationElement,
    pattern: windows::Win32::UI::Accessibility::UIA_PATTERN_ID,
) -> Option<T> {
    // SAFETY: an in-process COM call reading the cache built for this element.
    unsafe { element.GetCachedPattern(pattern) }
        .ok()
        .and_then(|unknown| unknown.cast::<T>().ok())
}

fn current_pattern<T: Interface>(
    element: &IUIAutomationElement,
    pattern: windows::Win32::UI::Accessibility::UIA_PATTERN_ID,
) -> Option<T> {
    // SAFETY: a live COM call to the element's provider.
    unsafe { element.GetCurrentPattern(pattern) }
        .ok()
        .and_then(|unknown| unknown.cast::<T>().ok())
}

/// Read one cached element. Every read is against the cache; nothing here
/// crosses into the target process.
pub(super) fn describe(element: &IUIAutomationElement, browser: bool) -> ElementFacts {
    // SAFETY: every call below reads the cache built for this element.
    unsafe {
        let control_type = element
            .CachedControlType()
            .map(|held| held.0)
            .unwrap_or(50025);
        let class_name = cached_string(element.CachedClassName()).unwrap_or_default();
        let scroll = cached_pattern::<IUIAutomationScrollPattern>(element, UIA_ScrollPatternId);
        let vertical = scroll
            .as_ref()
            .and_then(|pattern| cached_bool(pattern.CachedVerticallyScrollable()))
            .unwrap_or(false);
        let horizontal = scroll
            .as_ref()
            .and_then(|pattern| cached_bool(pattern.CachedHorizontallyScrollable()))
            .unwrap_or(false);
        let role = role_for(control_type, vertical || horizontal, browser);
        let name = cached_string(element.CachedName());
        let help_text = cached_string(element.CachedHelpText());
        let is_password = cached_bool(element.CachedIsPassword()).unwrap_or(false);

        let mut actions = Vec::new();
        let mut value = None;
        let mut settable = false;
        if cached_pattern::<IUIAutomationInvokePattern>(element, UIA_InvokePatternId).is_some() {
            actions.push("Invoke".to_string());
        }
        if let Some(pattern) =
            cached_pattern::<IUIAutomationTogglePattern>(element, UIA_TogglePatternId)
        {
            actions.push("Toggle".to_string());
            value = pattern.CachedToggleState().ok().map(|state| {
                if state == ToggleState_On {
                    "1".to_string()
                } else if state == windows::Win32::UI::Accessibility::ToggleState_Off {
                    "0".to_string()
                } else {
                    "2".to_string()
                }
            });
        }
        let mut selected = false;
        if let Some(pattern) =
            cached_pattern::<IUIAutomationSelectionItemPattern>(element, UIA_SelectionItemPatternId)
        {
            actions.push("Select".to_string());
            selected = cached_bool(pattern.CachedIsSelected()).unwrap_or(false);
        }
        let mut expanded = false;
        if let Some(pattern) = cached_pattern::<IUIAutomationExpandCollapsePattern>(
            element,
            UIA_ExpandCollapsePatternId,
        ) {
            match pattern.CachedExpandCollapseState() {
                Ok(state) if state == ExpandCollapseState_Expanded => {
                    expanded = true;
                    actions.push("Collapse".to_string());
                }
                Ok(state) if state == ExpandCollapseState_Collapsed => {
                    actions.push("Expand".to_string());
                }
                _ => {}
            }
        }
        if vertical {
            actions.push("ScrollUpByPage".to_string());
            actions.push("ScrollDownByPage".to_string());
        }
        if horizontal {
            actions.push("ScrollLeftByPage".to_string());
            actions.push("ScrollRightByPage".to_string());
        }
        if cached_pattern::<IUIAutomationScrollItemPattern>(element, UIA_ScrollItemPatternId)
            .is_some()
        {
            actions.push("ScrollIntoView".to_string());
        }
        if let Some(pattern) =
            cached_pattern::<IUIAutomationValuePattern>(element, UIA_ValuePatternId)
        {
            let read_only = cached_bool(pattern.CachedIsReadOnly()).unwrap_or(true);
            settable = !read_only;
            if value.is_none() {
                value = pattern.CachedValue().ok().map(|held| held.to_string());
            }
        } else if let Some(pattern) =
            cached_pattern::<IUIAutomationRangeValuePattern>(element, UIA_RangeValuePatternId)
        {
            let read_only = cached_bool(pattern.CachedIsReadOnly()).unwrap_or(true);
            settable = !read_only;
            if value.is_none() {
                value = pattern.CachedValue().ok().map(format_number);
            }
        }
        let secure = is_password
            || (should_probe_secure_text_metadata(role)
                && looks_like_secure_text(&format!(
                    "{role} {} {} {}",
                    class_name,
                    name.clone().unwrap_or_default(),
                    help_text.clone().unwrap_or_default()
                )));
        if secure && value.is_some() {
            value = Some("[redacted]".to_string());
        }
        let frame = element.CachedBoundingRectangle().ok().and_then(|rect| {
            let width = f64::from(rect.right - rect.left);
            let height = f64::from(rect.bottom - rect.top);
            (width > 0.0 && height > 0.0)
                .then(|| Rect::new(f64::from(rect.left), f64::from(rect.top), width, height))
        });
        ElementFacts {
            role,
            role_description: cached_string(element.CachedLocalizedControlType()),
            name,
            help_text,
            value,
            is_password,
            enabled: cached_bool(element.CachedIsEnabled()).unwrap_or(true),
            has_focus: cached_bool(element.CachedHasKeyboardFocus()).unwrap_or(false),
            frame,
            actions,
            selected,
            expanded,
            settable,
            class_name,
        }
    }
}

fn format_number(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 1e15 {
        format!("{}", value as i64)
    } else {
        value.to_string()
    }
}

/// A live pattern call, worded like `AXUIElementPerformAction` failures.
fn perform<T: Interface>(
    element: &IUIAutomationElement,
    pattern: windows::Win32::UI::Accessibility::UIA_PATTERN_ID,
    name: &str,
    call: impl FnOnce(&T) -> windows::core::Result<()>,
) -> Result<(), ProviderError> {
    let Some(pattern) = current_pattern::<T>(element, pattern) else {
        return Err(ProviderError::new(
            error_code::ACTION_NOT_SUPPORTED,
            format!("the element no longer supports {name}"),
        ));
    };
    call(&pattern).map_err(|error| {
        ProviderError::new(
            error_code::ACCESSIBILITY_ERROR,
            format!("UI Automation {name} failed: {error}"),
        )
    })
}

/// Perform one advertised action by its wire name.
pub(super) fn perform_action(
    element: &IUIAutomationElement,
    action: &str,
) -> Result<(), ProviderError> {
    // SAFETY (each arm): a live COM call on a pattern the element advertised.
    match action {
        "Invoke" => perform::<IUIAutomationInvokePattern>(
            element,
            UIA_InvokePatternId,
            "Invoke",
            |pattern| unsafe { pattern.Invoke() },
        ),
        "Toggle" => perform::<IUIAutomationTogglePattern>(
            element,
            UIA_TogglePatternId,
            "Toggle",
            |pattern| unsafe { pattern.Toggle() },
        ),
        "Select" => perform::<IUIAutomationSelectionItemPattern>(
            element,
            UIA_SelectionItemPatternId,
            "Select",
            |pattern| unsafe { pattern.Select() },
        ),
        "Expand" => perform::<IUIAutomationExpandCollapsePattern>(
            element,
            UIA_ExpandCollapsePatternId,
            "Expand",
            |pattern| unsafe { pattern.Expand() },
        ),
        "Collapse" => perform::<IUIAutomationExpandCollapsePattern>(
            element,
            UIA_ExpandCollapsePatternId,
            "Collapse",
            |pattern| unsafe { pattern.Collapse() },
        ),
        "ScrollIntoView" => perform::<IUIAutomationScrollItemPattern>(
            element,
            UIA_ScrollItemPatternId,
            "ScrollIntoView",
            |pattern| unsafe { pattern.ScrollIntoView() },
        ),
        "ScrollUpByPage" | "ScrollDownByPage" | "ScrollLeftByPage" | "ScrollRightByPage" => {
            let direction = match action {
                "ScrollUpByPage" => ScrollDirection::Up,
                "ScrollDownByPage" => ScrollDirection::Down,
                "ScrollLeftByPage" => ScrollDirection::Left,
                _ => ScrollDirection::Right,
            };
            scroll_page(element, direction)
        }
        other => Err(ProviderError::new(
            error_code::ACTION_NOT_SUPPORTED,
            format!("'{other}' is not an action this provider performs"),
        )),
    }
}

/// One page in `direction` through the Scroll pattern.
pub(super) fn scroll_page(
    element: &IUIAutomationElement,
    direction: ScrollDirection,
) -> Result<(), ProviderError> {
    let (horizontal, vertical) = match direction {
        ScrollDirection::Up => (ScrollAmount_NoAmount, ScrollAmount_LargeDecrement),
        ScrollDirection::Down => (ScrollAmount_NoAmount, ScrollAmount_LargeIncrement),
        ScrollDirection::Left => (ScrollAmount_LargeDecrement, ScrollAmount_NoAmount),
        ScrollDirection::Right => (ScrollAmount_LargeIncrement, ScrollAmount_NoAmount),
    };
    perform::<IUIAutomationScrollPattern>(
        element,
        UIA_ScrollPatternId,
        "Scroll",
        |pattern| unsafe { pattern.Scroll(horizontal, vertical) },
    )
}

/// The primary press: `Invoke`, else `Toggle`, else `Select` — the order a
/// click means on a button, a checkbox, a list item (the core's table, which
/// the marks read too).
pub(super) fn primary_click_action(actions: &[String]) -> Option<&'static str> {
    zerocode_core::computer_use::WINDOWS_PRESS_PATTERNS
        .iter()
        .copied()
        .find(|candidate| actions.iter().any(|action| action == candidate))
}

/// What a value-bearing element holds right now.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum LiveValue {
    Text(String),
    Range(f64),
    Toggle(bool),
}

/// Which pattern owns an element's value, and so which setter writes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ValueKind {
    Text,
    Range,
    Toggle,
}

/// One live look at a value-bearing element: the pattern that owns the
/// value, whether it may be written, and the value — or the provider's
/// refusal to say. A refused read (a password edit answers
/// `E_ACCESSDENIED` through the MSAA proxy) is kept apart from an empty
/// value, because overwriting a field on the strength of "" would erase
/// what the person typed.
pub(super) struct ValueProbe {
    pub kind: ValueKind,
    pub read_only: bool,
    pub value: Result<LiveValue, ProviderError>,
}

fn read_error(what: &str, error: windows::core::Error) -> ProviderError {
    ProviderError::new(
        error_code::ACCESSIBILITY_ERROR,
        format!("UI Automation could not read the {what}: {error}"),
    )
}

/// `None` when the element carries no value pattern at all.
pub(super) fn probe_value(element: &IUIAutomationElement) -> Option<ValueProbe> {
    // SAFETY: live COM calls on patterns the element supports.
    unsafe {
        if let Some(pattern) =
            current_pattern::<IUIAutomationValuePattern>(element, UIA_ValuePatternId)
        {
            return Some(ValueProbe {
                kind: ValueKind::Text,
                read_only: cached_bool(pattern.CurrentIsReadOnly()).unwrap_or(true),
                value: pattern
                    .CurrentValue()
                    .map(|held| LiveValue::Text(held.to_string()))
                    .map_err(|error| read_error("value", error)),
            });
        }
        if let Some(pattern) =
            current_pattern::<IUIAutomationRangeValuePattern>(element, UIA_RangeValuePatternId)
        {
            return Some(ValueProbe {
                kind: ValueKind::Range,
                read_only: cached_bool(pattern.CurrentIsReadOnly()).unwrap_or(true),
                value: pattern
                    .CurrentValue()
                    .map(LiveValue::Range)
                    .map_err(|error| read_error("range value", error)),
            });
        }
        if let Some(pattern) =
            current_pattern::<IUIAutomationTogglePattern>(element, UIA_TogglePatternId)
        {
            return Some(ValueProbe {
                kind: ValueKind::Toggle,
                read_only: false,
                value: pattern
                    .CurrentToggleState()
                    .map(|state| LiveValue::Toggle(state == ToggleState_On))
                    .map_err(|error| read_error("toggle state", error)),
            });
        }
        None
    }
}

/// The value alone, when it can be read.
pub(super) fn live_value(element: &IUIAutomationElement) -> Option<LiveValue> {
    probe_value(element)?.value.ok()
}

pub(super) fn set_text_value(
    element: &IUIAutomationElement,
    value: &str,
) -> Result<(), ProviderError> {
    perform::<IUIAutomationValuePattern>(
        element,
        UIA_ValuePatternId,
        "SetValue",
        |pattern| unsafe { pattern.SetValue(&BSTR::from(value)) },
    )
}

pub(super) fn set_range_value(
    element: &IUIAutomationElement,
    value: f64,
) -> Result<(), ProviderError> {
    perform::<IUIAutomationRangeValuePattern>(
        element,
        UIA_RangeValuePatternId,
        "SetValue",
        |pattern| unsafe { pattern.SetValue(value) },
    )
}

/// Bring a toggle to `wanted`, toggling at most twice (indeterminate first).
pub(super) fn toggle_to(element: &IUIAutomationElement, wanted: bool) -> Result<(), ProviderError> {
    for _ in 0..2 {
        match live_value(element) {
            Some(LiveValue::Toggle(current)) if current == wanted => return Ok(()),
            Some(LiveValue::Toggle(_)) => {
                perform::<IUIAutomationTogglePattern>(
                    element,
                    UIA_TogglePatternId,
                    "Toggle",
                    |pattern| unsafe { pattern.Toggle() },
                )?;
            }
            _ => break,
        }
    }
    Ok(())
}

/// The text before, inside and after the current selection, through the
/// Text pattern — `None` when the element has no text pattern or no
/// selection to speak of, which is the cue to append instead of replace.
pub(super) fn text_around_selection(element: &IUIAutomationElement) -> Option<Selection> {
    let pattern = current_pattern::<IUIAutomationTextPattern>(element, UIA_TextPatternId)?;
    // SAFETY: live COM calls on the pattern and the ranges it hands out.
    unsafe {
        let document = pattern.DocumentRange().ok()?;
        let selection = pattern.GetSelection().ok()?;
        if selection.Length().ok()? < 1 {
            return None;
        }
        let selected = selection.GetElement(0).ok()?;
        let prefix = document.Clone().ok()?;
        prefix
            .MoveEndpointByRange(
                TextPatternRangeEndpoint_End,
                &selected,
                TextPatternRangeEndpoint_Start,
            )
            .ok()?;
        let suffix = document.Clone().ok()?;
        suffix
            .MoveEndpointByRange(
                TextPatternRangeEndpoint_Start,
                &selected,
                TextPatternRangeEndpoint_End,
            )
            .ok()?;
        Some(Selection {
            prefix: prefix.GetText(-1).ok()?.to_string(),
            selected: selected.GetText(-1).ok()?.to_string(),
            suffix: suffix.GetText(-1).ok()?.to_string(),
        })
    }
}

/// Collapse the selection to a caret `utf16_offset` characters into the
/// document — where the helper's `setSelectedTextRange` puts it after a
/// replacement. Best effort: a provider that cannot is not an error.
pub(super) fn place_caret(element: &IUIAutomationElement, utf16_offset: usize) {
    let Some(pattern) = current_pattern::<IUIAutomationTextPattern>(element, UIA_TextPatternId)
    else {
        return;
    };
    // SAFETY: live COM calls on the pattern and one range cloned from it.
    unsafe {
        let Ok(range) = pattern
            .DocumentRange()
            .and_then(|document| document.Clone())
        else {
            return;
        };
        let count = i32::try_from(utf16_offset).unwrap_or(i32::MAX);
        if range
            .MoveEndpointByUnit(TextPatternRangeEndpoint_Start, TextUnit_Character, count)
            .is_err()
        {
            return;
        }
        if range
            .MoveEndpointByRange(
                TextPatternRangeEndpoint_End,
                &range,
                TextPatternRangeEndpoint_Start,
            )
            .is_err()
        {
            return;
        }
        let _ = range.Select();
    }
}

/// `AXShowMenu`: ask the element to open its own context menu, through
/// `IUIAutomationElement3::ShowContextMenu`. The menu opens in the app's
/// own way and no pointer is moved, so the click fence has nothing to judge.
pub(super) fn show_context_menu(element: &IUIAutomationElement) -> Result<(), ProviderError> {
    let Ok(element) = element.cast::<IUIAutomationElement3>() else {
        return Err(ProviderError::new(
            error_code::ACTION_NOT_SUPPORTED,
            "this UI Automation client cannot show context menus",
        ));
    };
    // SAFETY: a live COM call on the element.
    unsafe { element.ShowContextMenu() }.map_err(|error| {
        ProviderError::new(
            error_code::ACTION_NOT_SUPPORTED,
            format!("the element did not show a context menu: {error}"),
        )
    })
}

/// A UI Automation call that came back without an answer: the provider may
/// have acted before the connection or the deadline gave out, so the write
/// is unconfirmed rather than refused.
fn write_may_have_applied(error: &windows::core::Error) -> bool {
    const UIA_E_TIMEOUT: i32 = 0x8013_1505_u32 as i32;
    const RPC_E_TIMEOUT: i32 = 0x8001_011F_u32 as i32;
    const RPC_S_CALL_FAILED: i32 = 0x8007_06BE_u32 as i32;
    const RPC_S_CALL_FAILED_DNE: i32 = 0x8007_06BF_u32 as i32;
    matches!(
        error.code().0,
        UIA_E_TIMEOUT | RPC_E_TIMEOUT | RPC_S_CALL_FAILED | RPC_S_CALL_FAILED_DNE
    )
}

/// The focused element as the shared `replace_selection` sees it: the Value
/// pattern for the value and the write, the Text pattern for where the
/// selection is and where the caret goes after.
pub(super) struct UiaTextField<'a>(pub &'a IUIAutomationElement);

impl TextField for UiaTextField<'_> {
    fn read(&self) -> Result<FieldText, ReadFailure> {
        match probe_value(self.0) {
            Some(ValueProbe {
                kind: ValueKind::Text,
                read_only,
                value,
            }) => match value {
                Ok(LiveValue::Text(value)) => Ok(FieldText { value, read_only }),
                Ok(_) => Err(ReadFailure::Unsupported),
                Err(error) => Err(ReadFailure::Denied(error.message)),
            },
            _ => Err(ReadFailure::Unsupported),
        }
    }

    fn around_selection(&self) -> Option<Selection> {
        text_around_selection(self.0)
    }

    fn write(&self, text: &str) -> Result<(), WriteFailure> {
        let Some(pattern) =
            current_pattern::<IUIAutomationValuePattern>(self.0, UIA_ValuePatternId)
        else {
            return Err(WriteFailure::Refused(
                "the element no longer supports SetValue".into(),
            ));
        };
        // SAFETY: a live COM call on a pattern the element advertised.
        unsafe { pattern.SetValue(&BSTR::from(text)) }.map_err(|error| {
            if write_may_have_applied(&error) {
                WriteFailure::Unconfirmed(format!("UI Automation SetValue did not answer: {error}"))
            } else {
                WriteFailure::Refused(format!("UI Automation SetValue failed: {error}"))
            }
        })
    }

    fn place_caret(&self, utf16_offset: usize) {
        place_caret(self.0, utf16_offset);
    }
}

/// Select the whole document through the Text pattern; `true` when the
/// selection then spans it end to end.
pub(super) fn select_all_text(element: &IUIAutomationElement) -> Option<bool> {
    let pattern = current_pattern::<IUIAutomationTextPattern>(element, UIA_TextPatternId)?;
    // SAFETY: live COM calls on the pattern and its ranges.
    unsafe {
        let document = pattern.DocumentRange().ok()?;
        document.Select().ok()?;
        let selection = pattern.GetSelection().ok()?;
        if selection.Length().ok()? < 1 {
            return Some(false);
        }
        let selected = selection.GetElement(0).ok()?;
        let starts = selected
            .CompareEndpoints(
                TextPatternRangeEndpoint_Start,
                &document,
                TextPatternRangeEndpoint_Start,
            )
            .ok()?;
        let ends = selected
            .CompareEndpoints(
                TextPatternRangeEndpoint_End,
                &document,
                TextPatternRangeEndpoint_End,
            )
            .ok()?;
        Some(starts == 0 && ends == 0)
    }
}

/// The whole document's text, for a selection preview.
pub(super) fn document_text(element: &IUIAutomationElement) -> Option<String> {
    let pattern = current_pattern::<IUIAutomationTextPattern>(element, UIA_TextPatternId)?;
    // SAFETY: live COM calls on the pattern and its range.
    unsafe { pattern.DocumentRange().ok()?.GetText(-1).ok() }.map(|held| held.to_string())
}
