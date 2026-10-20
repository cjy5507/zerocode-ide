//! LaunchServices registration and the native receiver, without a Computer helper request.

use std::ffi::c_void;
use std::path::Path;
use std::ptr::NonNull;

use objc2::rc::Retained;
use objc2_app_kit::{NSApplicationActivationPolicy, NSRunningApplication, NSWorkspace};
use objc2_core_foundation::{
    CFArray, CFDictionary, CFNumber, CFNumberType, CFRetained, CFString, CGRect,
};
use objc2_core_graphics::{
    CGEvent, CGRectMakeWithDictionaryRepresentation, CGWindowListCopyWindowInfo, CGWindowListOption,
};
use objc2_foundation::{NSBundle, NSString, NSURL};
use serde_json::Value;
use zerocode_core::computer_use::ComputerMethod;
use zerocode_core::computer_use_protocol::identity::{
    AppQuery, MACOS_BUNDLE_NAME_KEYS, matches_any,
};
use zerocode_core::computer_use_protocol::marks::pointed_points;
use zerocode_core::computer_use_protocol::pointer::is_pointer_picture;

#[link(name = "CoreServices", kind = "framework")]
unsafe extern "C" {
    fn LSCopyAllHandlersForURLScheme(scheme: *const CFString) -> *mut CFArray;
    fn LSCopyAllRoleHandlersForContentType(
        content_type: *const CFString,
        roles: u32,
    ) -> *mut CFArray;
}

fn handlers(name: &str, viewer: bool) -> Option<Vec<String>> {
    let name = CFString::from_str(name);
    // SAFETY: LaunchServices returns a retained array of CFStrings, or null.
    let array = unsafe {
        let raw = if viewer {
            LSCopyAllRoleHandlersForContentType(&*name, 2) // kLSRolesViewer
        } else {
            LSCopyAllHandlersForURLScheme(&*name)
        };
        CFRetained::from_raw(NonNull::new(raw)?)
    };
    (0..CFArray::count(&array))
        .map(|at| {
            // SAFETY: LaunchServices owns string elements for the array's lifetime.
            let value = unsafe { CFArray::value_at_index(&array, at) }.cast::<CFString>();
            Some(unsafe { value.as_ref()? }.to_string())
        })
        .collect()
}

fn browser_bundle(bundle_id: &str) -> Option<bool> {
    let handlers = [
        handlers("http", false)?,
        handlers("https", false)?,
        handlers("public.html", true)?,
        handlers("public.xhtml", true)?,
    ];
    super::registered_browser(bundle_id, &handlers)
}

fn bundle_for_query(query: &str) -> Option<String> {
    let workspace = NSWorkspace::sharedWorkspace();
    let query = AppQuery::parse(query).ok()?;
    if let AppQuery::Pid(pid) = query {
        return NSRunningApplication::runningApplicationWithProcessIdentifier(
            i32::try_from(pid).ok()?,
        )?
        .bundleIdentifier()
        .map(|id| id.to_string());
    }
    let AppQuery::Named(query) = query else {
        return None;
    };
    let mut matched = None;
    for app in workspace.runningApplications().iter().filter(|app| {
        !app.isTerminated() && app.activationPolicy() == NSApplicationActivationPolicy::Regular
    }) {
        let name = app
            .localizedName()
            .map(|name| name.to_string())
            .unwrap_or_default();
        let bundle_id = app.bundleIdentifier().map(|id| id.to_string());
        let mut others = Vec::new();
        if let Some(url) = app.bundleURL()
            && let Some(bundle) = NSBundle::bundleWithURL(&url)
        {
            for key in MACOS_BUNDLE_NAME_KEYS {
                if let Some(value) = bundle.objectForInfoDictionaryKey(&NSString::from_str(key))
                    && let Some(name) = value.downcast_ref::<NSString>()
                {
                    others.push(name.to_string());
                }
            }
        }
        if let Some(path) = app
            .executableURL()
            .and_then(|url| url.path())
            .map(|path| path.to_string())
            && let Some(stem) = Path::new(&path).file_stem().and_then(|stem| stem.to_str())
        {
            others.push(stem.to_string());
        }
        if matches_any(
            &query,
            &name,
            bundle_id.as_deref(),
            others.iter().map(String::as_str),
        ) {
            let bundle_id = bundle_id?;
            if matched
                .as_ref()
                .is_some_and(|previous: &String| !previous.eq_ignore_ascii_case(&bundle_id))
            {
                return None;
            }
            matched = Some(bundle_id);
        }
    }
    matched.or_else(|| {
        let url = workspace.URLForApplicationWithBundleIdentifier(&NSString::from_str(&query))?;
        NSBundle::bundleWithURL(&url)?
            .bundleIdentifier()
            .map(|id| id.to_string())
    })
}

fn value(row: &CFDictionary, key: &str) -> *const c_void {
    let key = CFString::from_str(key);
    // SAFETY: the dictionary remains alive while the returned value is read.
    unsafe { CFDictionary::value(row, (&*key as *const CFString).cast()) }
}

fn number(row: &CFDictionary, key: &str) -> Option<i64> {
    let value = value(row, key).cast::<CFNumber>();
    if value.is_null() {
        return None;
    }
    let mut number = 0i64;
    // SAFETY: these WindowServer keys contain CFNumbers; the output is an i64.
    unsafe {
        CFNumber::value(
            &*value,
            CFNumberType::SInt64Type,
            (&mut number as *mut i64).cast(),
        )
    }
    .then_some(number)
}

fn window_pid(window_id: Option<u64>, point: Option<(f64, f64)>) -> Option<i32> {
    let options = if window_id.is_some() {
        CGWindowListOption(0)
    } else {
        CGWindowListOption::OptionOnScreenOnly
    };
    let listed = CGWindowListCopyWindowInfo(options, 0)?;
    for at in 0..CFArray::count(&listed) {
        // SAFETY: CoreGraphics returns dictionaries, retained by `listed`.
        let row = unsafe { &*CFArray::value_at_index(&listed, at).cast::<CFDictionary>() };
        if let Some(wanted) = window_id {
            if number(row, "kCGWindowNumber").and_then(|id| u64::try_from(id).ok()) != Some(wanted)
            {
                continue;
            }
        } else if let Some((x, y)) = point {
            let bounds = value(row, "kCGWindowBounds").cast::<CFDictionary>();
            if bounds.is_null() {
                continue;
            }
            let mut rect = CGRect::default();
            // SAFETY: the bounds key is a CGRect dictionary; `rect` outlives the call.
            if !unsafe { CGRectMakeWithDictionaryRepresentation(Some(&*bounds), &mut rect) } {
                continue;
            }
            let owner = value(row, "kCGWindowOwnerName").cast::<CFString>();
            let owner = unsafe { owner.as_ref() }
                .map(ToString::to_string)
                .unwrap_or_default();
            if is_pointer_picture(
                number(row, "kCGWindowLayer").unwrap_or_default(),
                &owner,
                rect.size.width,
                rect.size.height,
            ) || x < rect.origin.x
                || y < rect.origin.y
                || x >= rect.origin.x + rect.size.width
                || y >= rect.origin.y + rect.size.height
            {
                continue;
            }
        }
        return number(row, "kCGWindowOwnerPID").and_then(|pid| i32::try_from(pid).ok());
    }
    None
}

fn app_bundle(app: Retained<NSRunningApplication>) -> Option<String> {
    app.bundleIdentifier().map(|id| id.to_string())
}

pub(super) fn target_is_browser(method: ComputerMethod, params: &Value) -> Option<bool> {
    objc2::rc::autoreleasepool(|_| {
        if let Some(query) = params
            .get("app")
            .or_else(|| params.get("with"))
            .and_then(Value::as_str)
        {
            return browser_bundle(&bundle_for_query(query)?);
        }
        if let Some(window) = params.get("windowId").and_then(Value::as_u64) {
            let app = NSRunningApplication::runningApplicationWithProcessIdentifier(window_pid(
                Some(window),
                None,
            )?)?;
            return browser_bundle(&app_bundle(app)?);
        }
        if method == ComputerMethod::Open {
            let url = if let Some(url) = params.get("url").and_then(Value::as_str) {
                NSURL::URLWithString(&NSString::from_str(url))?
            } else {
                NSURL::fileURLWithPath(&NSString::from_str(params.get("path")?.as_str()?))
            };
            let app = NSWorkspace::sharedWorkspace().URLForApplicationToOpenURL(&url)?;
            return browser_bundle(
                &NSBundle::bundleWithURL(&app)?
                    .bundleIdentifier()?
                    .to_string(),
            );
        }
        if matches!(
            method,
            ComputerMethod::MouseMove
                | ComputerMethod::MouseClick
                | ComputerMethod::MouseDrag
                | ComputerMethod::MouseScroll
        ) {
            let provider = method.provider_name()?;
            let mut points = pointed_points(provider, params);
            if points.is_empty() {
                let point = if method == ComputerMethod::MouseMove {
                    (params.get("x")?.as_f64()?, params.get("y")?.as_f64()?)
                } else {
                    let event = CGEvent::new(None)?;
                    let point = CGEvent::location(Some(&event));
                    (point.x, point.y)
                };
                points.push(point);
            }
            let mut unknown = false;
            for point in points {
                let browser = window_pid(None, Some(point))
                    .and_then(NSRunningApplication::runningApplicationWithProcessIdentifier)
                    .and_then(app_bundle)
                    .and_then(|bundle| browser_bundle(&bundle));
                match browser {
                    Some(true) => return Some(true),
                    Some(false) => {}
                    None => unknown = true,
                }
            }
            return (!unknown).then_some(false);
        }
        browser_bundle(&app_bundle(
            NSWorkspace::sharedWorkspace().frontmostApplication()?,
        )?)
    })
}
