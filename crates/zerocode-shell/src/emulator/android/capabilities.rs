//! Android app, permission, and one-shot log operations.

use std::collections::BTreeSet;
use std::process::Command;

use super::AndroidSdk;
use crate::emulator::capability::{
    APP_COMMAND_TIMEOUT, COMMAND_OUTPUT_BYTES, EmulatorLogBatch, LOG_COMMAND_TIMEOUT,
    LOG_OUTPUT_BYTES, SHORT_COMMAND_TIMEOUT, android_activity, android_log_batch,
    android_log_filters, app_path, identifier, log_line_limit,
};
use crate::emulator::process::run_bounded;

const MAX_PACKAGE_PERMISSIONS: usize = 256;

pub(super) fn install_app(
    sdk: &AndroidSdk,
    serial: &str,
    path: &str,
    reinstall: bool,
) -> Result<(), String> {
    let path = app_path(path, "apk", false)?;
    let mut command = adb(sdk, serial);
    command.arg("install");
    if reinstall {
        command.arg("-r");
    }
    command.arg(path);
    let output = run_bounded(command, APP_COMMAND_TIMEOUT, COMMAND_OUTPUT_BYTES)?;
    let text = format!(
        "{}\n{}",
        output.stdout_text(),
        String::from_utf8_lossy(&output.stderr)
    );
    if !output.status.success()
        || text.lines().any(|line| {
            let line = line.trim_start();
            line.starts_with("Failure [") || line.starts_with("Error:")
        })
    {
        return Err(format!("APK 설치에 실패했습니다: {}", output.diagnostic()));
    }
    Ok(())
}

pub(super) fn launch_app(
    sdk: &AndroidSdk,
    serial: &str,
    package: &str,
    activity: Option<String>,
) -> Result<(), String> {
    let package = identifier(package, "Android 패키지 이름")?;
    let activity = android_activity(activity)?;
    let component = if let Some(activity) = activity {
        format!("{package}/{activity}")
    } else {
        resolve_launch_component(sdk, serial, &package)?
    };
    let mut command = adb(sdk, serial);
    command.args(["shell", "am", "start", "-n", &component]);
    let output = run_bounded(command, SHORT_COMMAND_TIMEOUT, COMMAND_OUTPUT_BYTES)?;
    let stdout = output.stdout_text();
    if !output.status.success()
        || stdout.lines().any(|line| {
            let line = line.trim_start();
            line.starts_with("Error:")
                || line.contains("No activities found")
                || line.contains("monkey aborted")
        })
    {
        return Err(format!(
            "Android 앱 실행에 실패했습니다: {}",
            output.diagnostic()
        ));
    }
    Ok(())
}

fn resolve_launch_component(
    sdk: &AndroidSdk,
    serial: &str,
    package: &str,
) -> Result<String, String> {
    let mut command = adb(sdk, serial);
    command.args([
        "shell",
        "cmd",
        "package",
        "resolve-activity",
        "--brief",
        "-a",
        "android.intent.action.MAIN",
        "-c",
        "android.intent.category.LAUNCHER",
        package,
    ]);
    let output = run_bounded(command, SHORT_COMMAND_TIMEOUT, COMMAND_OUTPUT_BYTES)?
        .ensure_success("Android 시작 액티비티를 찾지 못했습니다")?;
    parse_launch_component(&output.stdout_text(), package)
}

fn parse_launch_component(output: &str, package: &str) -> Result<String, String> {
    output
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| {
            line.strip_prefix(package)
                .and_then(|suffix| suffix.strip_prefix('/'))
                .is_some_and(|activity| !activity.is_empty())
        })
        .map(str::to_string)
        .ok_or_else(|| format!("{package}에 실행 가능한 LAUNCHER 액티비티가 없습니다"))
}

pub(super) fn set_permission(
    sdk: &AndroidSdk,
    serial: &str,
    operation: &str,
    package: &str,
    permission: Option<String>,
) -> Result<(), String> {
    let package = identifier(package, "Android 패키지 이름")?;
    match operation {
        "grant" | "revoke" => {
            let permission = permission
                .as_deref()
                .ok_or("grant/revoke에는 Android 권한 이름이 필요합니다")?;
            let permission = identifier(permission, "Android 권한 이름")?;
            let mut command = adb(sdk, serial);
            command.args(["shell", "pm", operation, &package, &permission]);
            run_bounded(command, SHORT_COMMAND_TIMEOUT, COMMAND_OUTPUT_BYTES)?
                .ensure_success("Android 권한 변경에 실패했습니다")?;
            Ok(())
        }
        "reset" => reset_permissions(sdk, serial, &package, permission.as_deref()),
        _ => Err("권한 작업은 grant, revoke 또는 reset이어야 합니다".to_string()),
    }
}

pub(super) fn logs(
    sdk: &AndroidSdk,
    serial: &str,
    lines: Option<usize>,
    filters: Option<Vec<String>>,
) -> Result<EmulatorLogBatch, String> {
    let lines = log_line_limit(lines)?;
    let filters = android_log_filters(filters)?;
    let mut command = adb(sdk, serial);
    command.args(["logcat", "-d", "-v", "threadtime", "-t", &lines.to_string()]);
    command.args(filters);
    let output = run_bounded(command, LOG_COMMAND_TIMEOUT, LOG_OUTPUT_BYTES)?
        .ensure_success("Android 로그를 읽지 못했습니다")?;
    Ok(android_log_batch(&output.stdout, lines, output.truncated))
}

/// Android has no package-scoped `pm reset-permissions`. Discover this app's
/// runtime permission rows, then revoke and clear decision flags one by one;
/// never fall back to the global command that changes every installed app.
fn reset_permissions(
    sdk: &AndroidSdk,
    serial: &str,
    package: &str,
    requested: Option<&str>,
) -> Result<(), String> {
    let permissions = if let Some(permission) = requested {
        vec![identifier(permission, "Android 권한 이름")?]
    } else {
        package_runtime_permissions(sdk, serial, package)?
    };
    for permission in permissions {
        let mut revoke = adb(sdk, serial);
        revoke.args(["shell", "pm", "revoke", package, &permission]);
        // Already-revoked permissions can make `pm revoke` non-zero. Clearing
        // the choice flags is the reset contract, so continue to that command.
        let _ = run_bounded(revoke, SHORT_COMMAND_TIMEOUT, COMMAND_OUTPUT_BYTES);

        let mut clear = adb(sdk, serial);
        clear.args([
            "shell",
            "pm",
            "clear-permission-flags",
            package,
            &permission,
            "user-set",
            "user-fixed",
        ]);
        run_bounded(clear, SHORT_COMMAND_TIMEOUT, COMMAND_OUTPUT_BYTES)?
            .ensure_success("Android 권한 결정을 초기화하지 못했습니다")?;
    }
    Ok(())
}

fn package_runtime_permissions(
    sdk: &AndroidSdk,
    serial: &str,
    package: &str,
) -> Result<Vec<String>, String> {
    let mut command = adb(sdk, serial);
    command.args(["shell", "dumpsys", "package", package]);
    let output = run_bounded(command, SHORT_COMMAND_TIMEOUT, LOG_OUTPUT_BYTES)?
        .ensure_success("Android 앱 권한을 읽지 못했습니다")?;
    if output.truncated {
        return Err("Android 앱 권한 목록이 안전한 출력 제한을 넘었습니다".to_string());
    }
    parse_runtime_permissions(&output.stdout_text())
}

fn parse_runtime_permissions(output: &str) -> Result<Vec<String>, String> {
    let mut permissions = BTreeSet::new();
    let mut section_indent = None;
    for line in output.lines() {
        let trimmed = line.trim();
        let indent = line.len().saturating_sub(line.trim_start().len());
        if trimmed == "runtime permissions:" {
            section_indent = Some(indent);
            continue;
        }
        let Some(header_indent) = section_indent else {
            continue;
        };
        if !trimmed.is_empty() && indent <= header_indent {
            section_indent = None;
            continue;
        }
        let Some((name, state)) = trimmed.split_once(':') else {
            continue;
        };
        if !state.contains("granted=") || identifier(name, "Android 권한 이름").is_err() {
            continue;
        }
        permissions.insert(name.to_string());
        if permissions.len() > MAX_PACKAGE_PERMISSIONS {
            return Err("Android 앱 권한 수가 안전한 제한을 넘었습니다".to_string());
        }
    }
    Ok(permissions.into_iter().collect())
}

fn adb(sdk: &AndroidSdk, serial: &str) -> Command {
    let mut command = crate::proc::quiet_command(&sdk.adb);
    command.args(["-s", serial]);
    command
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_permission_reset_parser_is_scoped_and_deduplicated() {
        let parsed = parse_runtime_permissions(
            "install permissions:\n  android.permission.INTERNET: granted=true\n\
             runtime permissions:\n  android.permission.CAMERA: granted=false, flags=[ USER_SET ]\n\
             android.permission.CAMERA: granted=true, flags=[]\n",
        )
        .unwrap();
        assert_eq!(parsed, vec!["android.permission.CAMERA".to_string()]);
    }

    #[test]
    fn apk_paths_are_passed_as_process_arguments_not_shell_text() {
        let path = std::path::Path::new("folder with spaces/application.apk");
        assert_eq!(path.extension().and_then(|one| one.to_str()), Some("apk"));
    }

    #[test]
    fn launch_component_parser_ignores_resolver_metadata() {
        let output = "priority=0 match=0x108000\ncom.example.app/.MainActivity\n";
        assert_eq!(
            parse_launch_component(output, "com.example.app").unwrap(),
            "com.example.app/.MainActivity"
        );
        assert!(parse_launch_component("No activity found", "com.example.app").is_err());
    }
}
