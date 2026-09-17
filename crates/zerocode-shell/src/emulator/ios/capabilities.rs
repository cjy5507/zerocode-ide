//! iOS Simulator app, privacy, and one-shot unified-log operations.

use std::process::Command;

use crate::emulator::capability::{
    APP_COMMAND_TIMEOUT, COMMAND_OUTPUT_BYTES, EmulatorLogBatch, LOG_COMMAND_TIMEOUT,
    LOG_OUTPUT_BYTES, SHORT_COMMAND_TIMEOUT, app_path, identifier, ios_log_batch, log_line_limit,
};
use crate::emulator::process::run_bounded;

const DEFAULT_LOG_SECONDS: u32 = 60;
const MAX_LOG_SECONDS: u32 = 3_600;
const PRIVACY_SERVICES: &[&str] = &[
    "all",
    "calendar",
    "contacts-limited",
    "contacts",
    "location",
    "location-always",
    "photos-add",
    "photos",
    "media-library",
    "microphone",
    "motion",
    "reminders",
    "siri",
];

pub(super) fn install_app(udid: &str, path: &str) -> Result<(), String> {
    let path = app_path(path, "app", true)?;
    let mut command = simctl();
    command.args(["install", udid]).arg(path);
    run_bounded(command, APP_COMMAND_TIMEOUT, COMMAND_OUTPUT_BYTES)?
        .ensure_success("iOS 앱 설치에 실패했습니다")?;
    Ok(())
}

pub(super) fn launch_app(udid: &str, bundle: &str) -> Result<(), String> {
    let bundle = identifier(bundle, "iOS 번들 식별자")?;
    let mut command = simctl();
    command.args(["launch", "--terminate-running-process", udid, &bundle]);
    run_bounded(command, SHORT_COMMAND_TIMEOUT, COMMAND_OUTPUT_BYTES)?
        .ensure_success("iOS 앱 실행에 실패했습니다")?;
    Ok(())
}

pub(super) fn set_permission(
    udid: &str,
    operation: &str,
    bundle: &str,
    service: &str,
) -> Result<(), String> {
    if !matches!(operation, "grant" | "revoke" | "reset") {
        return Err("권한 작업은 grant, revoke 또는 reset이어야 합니다".to_string());
    }
    let bundle = identifier(bundle, "iOS 번들 식별자")?;
    let service = service.trim().to_ascii_lowercase();
    if !PRIVACY_SERVICES.contains(&service.as_str()) {
        return Err("지원하지 않는 iOS 개인정보 서비스입니다".to_string());
    }
    if service == "all" && operation != "reset" {
        return Err("iOS의 all 서비스는 reset에만 사용할 수 있습니다".to_string());
    }
    let mut command = simctl();
    command.args(["privacy", udid, operation, &service, &bundle]);
    run_bounded(command, SHORT_COMMAND_TIMEOUT, COMMAND_OUTPUT_BYTES)?
        .ensure_success("iOS 권한 변경에 실패했습니다")?;
    Ok(())
}

pub(super) fn logs(
    udid: &str,
    lines: Option<usize>,
    seconds: Option<u32>,
    process: Option<String>,
) -> Result<EmulatorLogBatch, String> {
    let lines = log_line_limit(lines)?;
    let seconds = seconds.unwrap_or(DEFAULT_LOG_SECONDS);
    if !(1..=MAX_LOG_SECONDS).contains(&seconds) {
        return Err(format!(
            "iOS 로그 범위는 1..={MAX_LOG_SECONDS}초여야 합니다"
        ));
    }
    let process = process
        .map(|value| identifier(&value, "iOS 로그 프로세스 이름"))
        .transpose()?;
    let mut command = simctl();
    command.args([
        "spawn",
        udid,
        "log",
        "show",
        "--style",
        "ndjson",
        "--last",
        &format!("{seconds}s"),
    ]);
    if let Some(process) = process {
        command.args(["--predicate", &format!("process == \"{process}\"")]);
    }
    let output = run_bounded(command, LOG_COMMAND_TIMEOUT, LOG_OUTPUT_BYTES)?
        .ensure_success("iOS 로그를 읽지 못했습니다")?;
    Ok(ios_log_batch(&output.stdout, lines, output.truncated))
}

fn simctl() -> Command {
    // The one cached resolution every simctl door shares — see
    // `ios::simctl_command`.
    super::simctl_command()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn privacy_service_contract_matches_simctl_and_rejects_unknown_values() {
        assert!(PRIVACY_SERVICES.contains(&"location-always"));
        assert!(!PRIVACY_SERVICES.contains(&"camera"));
    }
}
