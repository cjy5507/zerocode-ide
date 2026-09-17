//! The Windows provider: the app-scoped methods the macOS helper answers,
//! dispatched over the same cache and the same stale-index rule. The
//! desktop, app, window and meaning-layer methods the helper grew later are
//! held here (`zerocode_core::computer_use::WINDOWS_HOLD_METHODS`) and
//! refused as `unsupported_capability` until the parity work lands.

use std::time::Instant;

use serde_json::{Map, Value};
use zerocode_core::computer_use::COMPUTER_USE_PROTOCOL_VERSION;
use zerocode_core::computer_use_protocol::cache::{
    AppKeys, Namespace, SnapshotCache, validate_pinned_element, validate_requested_elements,
};
use zerocode_core::computer_use_protocol::marks::{ELEMENT_FRAMES_KEY, Pin};
use zerocode_core::computer_use_protocol::params;
use zerocode_core::computer_use_protocol::{
    ActionMetadata, ActionResult, AppListing, ListedWindow, ProviderCapabilities, ProviderError,
    SupportMatrix, Verification, WindowInfo, WindowListing, error_code,
};

use super::super::ComputerUseError;
use super::apps::{AppDescriptor, Catalog};
use super::desktop_windows;
use super::snapshot::{self, BuildRequest, SharedSnapshot, Snapshot};
use super::uia::UiaClient;
use super::{PROVIDER_NAME, PROVIDER_VERSION};

pub(super) struct Provider {
    pub(super) client: UiaClient,
    cache: SnapshotCache<SharedSnapshot>,
    /// Process descriptions, kept across calls (`apps::Catalog`).
    pub(super) apps: Catalog,
}

impl Provider {
    pub fn new() -> Result<Self, ComputerUseError> {
        Ok(Self {
            client: UiaClient::new()?,
            cache: SnapshotCache::new(),
            apps: Catalog::default(),
        })
    }

    /// `Provider.handle(method:params:)`.
    pub fn handle(&mut self, method: &str, params: Value) -> Result<Value, ComputerUseError> {
        let params = params.as_object().cloned().unwrap_or_default();
        match method {
            "handshake" => json(capabilities()),
            "listApps" => json(self.list_apps()),
            "listWindows" => json(self.list_windows(&params)?),
            "getAppState" => {
                let include_screenshot = !params::flag(&params, "noScreenshot");
                let faces = params::flag(&params, ELEMENT_FRAMES_KEY);
                json(
                    self.observe(&params, include_screenshot)?
                        .observation_with(faces),
                )
            }
            "click" => json(self.run_action(&params, Self::click)?),
            "performSecondaryAction" => {
                json(self.run_action(&params, Self::perform_secondary_action)?)
            }
            "setValue" => json(self.run_action(&params, Self::set_value)?),
            "typeText" => json(self.run_action(&params, Self::type_text)?),
            "pressKey" => json(self.run_action(&params, Self::press_key)?),
            "hotkey" => json(self.run_action(&params, Self::hotkey)?),
            "pasteText" => json(self.run_action(&params, Self::paste_text)?),
            "scroll" => json(self.run_action(&params, Self::scroll)?),
            "drag" => json(self.run_action(&params, Self::drag)?),
            "terminate" => Ok(serde_json::json!({ "ok": true })),
            other if zerocode_core::computer_use::WINDOWS_HOLD_METHODS.contains(&other) => {
                Err(ProviderError::new(
                    error_code::UNSUPPORTED_CAPABILITY,
                    format!(
                        "'{other}' is held for Windows (docs/design/computer-use-windows-parity.md §1.3)"
                    ),
                ))
            }
            other => Err(ProviderError::invalid_argument(format!(
                "unknown method '{other}'"
            ))),
        }
    }

    fn list_apps(&mut self) -> AppListing {
        AppListing {
            apps: self.apps.list().iter().map(AppDescriptor::listed).collect(),
        }
    }

    fn list_windows(
        &mut self,
        params: &Map<String, Value>,
    ) -> Result<WindowListing, ProviderError> {
        let app = self
            .apps
            .resolve(&params::required_string(params, "app")?)?;
        let windows = desktop_windows::candidates(app.pid)
            .into_iter()
            .enumerate()
            .map(|(index, candidate)| ListedWindow {
                index,
                app: app.identity(),
                window: WindowInfo {
                    id: candidate.id(),
                    title: candidate.title.clone(),
                    x: candidate.bounds.x.round() as i64,
                    y: candidate.bounds.y.round() as i64,
                    width: candidate.bounds.width.round() as i64,
                    height: candidate.bounds.height.round() as i64,
                    is_minimized: candidate.is_minimized,
                    is_offscreen: candidate.is_offscreen,
                    screen_index: candidate.screen_index,
                    platform: serde_json::json!({ "topmost": candidate.topmost }),
                },
                is_main: None,
            })
            .collect();
        Ok(WindowListing {
            app: app.listed(),
            windows,
        })
    }

    /// `observe`: resolve the app, build the snapshot, file it.
    pub(super) fn observe(
        &mut self,
        params: &Map<String, Value>,
        include_screenshot: bool,
    ) -> Result<Snapshot, ProviderError> {
        let query = params::required_string(params, "app")?;
        let window_id = params::optional_window_id(params)?;
        let window_index = params::optional_index(params, "windowIndex")?;
        let app = self.apps.resolve(&query)?;
        let snapshot = snapshot::build(
            &self.client,
            BuildRequest {
                app: &app,
                include_screenshot,
                window_id,
                window_index,
                restore_window: params::flag(params, "restoreWindow"),
            },
        )?;
        self.cache.remember(
            &query,
            &AppKeys {
                name: app.name.clone(),
                bundle_id: app.aumid.clone(),
                pid: app.pid,
            },
            &namespace_of(params),
            window_index,
            SharedSnapshot::from_observed(&snapshot),
            Instant::now(),
        );
        Ok(snapshot)
    }

    /// `currentSnapshot`: a fresh, screenshot-less observation whose
    /// requested indexes still name what the cached one indexed — or, for a
    /// click by a mark's number, whose element is still the one its pin names.
    pub(super) fn current_snapshot(
        &mut self,
        params: &Map<String, Value>,
    ) -> Result<Snapshot, ProviderError> {
        if let Some(pin) = Pin::from_params(params)? {
            let current = self.observe(params, false)?;
            let index = params::optional_index(params, "elementIndex")?.ok_or_else(|| {
                ProviderError::invalid_argument("a mark's pin needs its elementIndex")
            })?;
            validate_pinned_element(&SharedSnapshot::from_observed(&current), index, &pin)?;
            return Ok(current);
        }
        self.cache.prune(Instant::now());
        let cached = self
            .cache
            .lookup(
                params::optional_str(params, "app").unwrap_or_default(),
                &namespace_of(params),
                params::optional_window_id(params)?,
                params::optional_index(params, "windowIndex")?,
            )
            .cloned();
        let current = self.observe(params, false)?;
        let requested = requested_indexes(params)?;
        validate_requested_elements(
            cached.as_ref(),
            &SharedSnapshot::from_observed(&current),
            &requested,
        )?;
        Ok(current)
    }

    /// `actionResult`: run the action, observe after it, and when the
    /// requested window is gone observe without the selector and say so.
    fn run_action(
        &mut self,
        params: &Map<String, Value>,
        action: fn(&mut Self, &Map<String, Value>) -> Result<ActionMetadata, ProviderError>,
    ) -> Result<ActionResult, ProviderError> {
        let mut metadata = action(self, params)?;
        let include_screenshot = !params::flag(params, "noScreenshot");
        let has_selector = params.contains_key("windowId") || params.contains_key("windowIndex");
        let observed = match self.observe(params, include_screenshot) {
            Ok(snapshot) => snapshot,
            Err(error)
                if has_selector
                    && matches!(
                        error.code.as_str(),
                        error_code::WINDOW_NOT_FOUND | error_code::WINDOW_STALE
                    ) =>
            {
                let mut fallback = params.clone();
                fallback.remove("windowId");
                fallback.remove("windowIndex");
                if metadata.verification.is_none() {
                    metadata.verification = Some(Verification::unverified(
                        zerocode_core::computer_use_protocol::unverified_reason::WINDOW_CHANGED,
                        None,
                        None,
                    ));
                }
                self.observe(&fallback, include_screenshot)?
            }
            Err(error) => return Err(error),
        };
        metadata.target_window_id = observed.window.id();
        Ok(ActionResult {
            observation: observed.observation(),
            action: metadata,
        })
    }
}

/// An answer as JSON; a value that cannot be serialized is the provider's
/// own fault and says so.
fn json<T: serde::Serialize>(value: T) -> Result<Value, ProviderError> {
    serde_json::to_value(value)
        .map_err(|error| ProviderError::new(error_code::ACCESSIBILITY_ERROR, error.to_string()))
}

/// `providerHandshake`.
fn capabilities() -> ProviderCapabilities {
    ProviderCapabilities {
        platform: "windows".into(),
        provider: PROVIDER_NAME.into(),
        provider_version: PROVIDER_VERSION.into(),
        protocol_version: COMPUTER_USE_PROTOCOL_VERSION,
        supports: SupportMatrix::full_desktop(),
    }
}

pub(super) fn namespace_of(params: &Map<String, Value>) -> Namespace {
    Namespace::from_request(
        params::optional_str(params, "session"),
        params::optional_str(params, "worktree"),
    )
}

/// Every element index a request names.
pub(super) fn requested_indexes(params: &Map<String, Value>) -> Result<Vec<usize>, ProviderError> {
    let mut indexes = Vec::new();
    for key in ["elementIndex", "fromElementIndex", "toElementIndex"] {
        if let Some(index) = params::optional_index(params, key)? {
            indexes.push(index);
        }
    }
    Ok(indexes)
}
