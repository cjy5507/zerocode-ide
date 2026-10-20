//! Registered Windows browser HTTPS commands, matched against the resolved executable.

use std::ptr::{null, null_mut};

use serde_json::Value;
use windows::Win32::Foundation::POINT;
use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CLASSES_ROOT, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_32KEY,
    KEY_WOW64_64KEY, RRF_RT_REG_EXPAND_SZ, RRF_RT_REG_SZ, RegCloseKey, RegEnumKeyExW, RegGetValueW,
    RegOpenKeyExW,
};
use zerocode_core::computer_use::ComputerMethod;
use zerocode_core::computer_use_protocol::marks::pointed_points;

use super::{apps::Catalog, desktop_windows};

struct Key(HKEY);

impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: this handle was opened by RegOpenKeyExW and belongs to this value.
        unsafe {
            RegCloseKey(self.0);
        }
    }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain([0]).collect()
}

fn string(root: HKEY, key: &str, name: &str) -> Option<String> {
    let key = wide(key);
    let name = wide(name);
    let mut bytes = 0u32;
    // SAFETY: the first call requests the buffer size; all pointers outlive the call.
    if unsafe {
        RegGetValueW(
            root,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ,
            null_mut(),
            null_mut(),
            &mut bytes,
        )
    } != 0
    {
        return None;
    }
    let mut buffer = vec![0u16; (bytes as usize).div_ceil(2)];
    // SAFETY: the allocated buffer has the byte count the first call requested.
    if unsafe {
        RegGetValueW(
            root,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ,
            null_mut(),
            buffer.as_mut_ptr().cast(),
            &mut bytes,
        )
    } != 0
    {
        return None;
    }
    let end = buffer
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(buffer.len());
    Some(String::from_utf16_lossy(&buffer[..end]))
}

fn registered_commands() -> Option<Vec<String>> {
    let mut commands = Vec::new();
    let mut read_any = false;
    for root in [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE] {
        for view in [KEY_WOW64_64KEY, KEY_WOW64_32KEY] {
            let path = wide("Software\\Clients\\StartMenuInternet");
            let mut key = null_mut();
            // SAFETY: the predefined root stays open; the output is owned by `Key`.
            if unsafe { RegOpenKeyExW(root, path.as_ptr(), 0, KEY_READ | view, &mut key) } != 0 {
                continue;
            }
            let key = Key(key);
            read_any = true;
            for index in 0..1024 {
                let mut name = [0u16; 256];
                let mut length = name.len() as u32;
                // SAFETY: the API bounds writes by `length`; optional outputs are null.
                let result = unsafe {
                    RegEnumKeyExW(
                        key.0,
                        index,
                        name.as_mut_ptr(),
                        &mut length,
                        null(),
                        null_mut(),
                        null_mut(),
                        null_mut(),
                    )
                };
                if result != 0 {
                    break;
                }
                let client = String::from_utf16_lossy(&name[..length as usize]);
                let Some(prog_id) = string(
                    key.0,
                    &format!("{client}\\Capabilities\\URLAssociations"),
                    "https",
                ) else {
                    continue;
                };
                if let Some(command) = string(
                    HKEY_CLASSES_ROOT,
                    &format!("{prog_id}\\shell\\open\\command"),
                    "",
                ) {
                    commands.push(command);
                }
            }
        }
    }
    (read_any && !commands.is_empty()).then_some(commands)
}

fn executable_for_pid(catalog: &mut Catalog, pid: u32) -> Option<String> {
    catalog
        .describe(pid)?
        .executable
        .map(|path| path.to_string_lossy().into_owned())
}

pub(in crate::computer_use) fn target_is_browser(
    method: ComputerMethod,
    params: &Value,
) -> Option<bool> {
    let commands = registered_commands()?;
    let mut catalog = Catalog::default();
    let classify =
        |path: &str| super::super::browser_route::registered_executable(path, Some(&commands));
    if let Some(app) = params
        .get("app")
        .or_else(|| params.get("with"))
        .and_then(Value::as_str)
    {
        let app = catalog.resolve(app).ok()?;
        return classify(app.executable.as_ref()?.to_str()?);
    }
    if let Some(window) = params.get("windowId").and_then(Value::as_u64) {
        let (pid, _) = desktop_windows::all_top_level()
            .into_iter()
            .find(|(_, candidate)| candidate.id() == window)?;
        return classify(&executable_for_pid(&mut catalog, pid)?);
    }
    if method == ComputerMethod::Open {
        return None;
    }
    if matches!(
        method,
        ComputerMethod::MouseMove
            | ComputerMethod::MouseClick
            | ComputerMethod::MouseDrag
            | ComputerMethod::MouseScroll
    ) {
        let mut points = pointed_points(method.provider_name()?, params);
        if points.is_empty() {
            let point = if method == ComputerMethod::MouseMove {
                (params.get("x")?.as_f64()?, params.get("y")?.as_f64()?)
            } else {
                let mut point = POINT::default();
                unsafe { GetCursorPos(&mut point) }.ok()?;
                (f64::from(point.x), f64::from(point.y))
            };
            points.push(point);
        }
        let mut unknown = false;
        for (x, y) in points {
            let browser = desktop_windows::root_at(x, y)
                .and_then(|(pid, _)| executable_for_pid(&mut catalog, pid))
                .and_then(|path| classify(&path));
            match browser {
                Some(true) => return Some(true),
                Some(false) => {}
                None => unknown = true,
            }
        }
        return (!unknown).then_some(false);
    }
    let (pid, _) = desktop_windows::foreground()?;
    classify(&executable_for_pid(&mut catalog, pid)?)
}
