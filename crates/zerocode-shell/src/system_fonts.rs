//! Installed font-family discovery for Settings.
//!
//! The editor accepts any family name, so discovery is only a convenience and
//! must never make Settings unavailable.  Each platform has one fixed,
//! argument-separated command, a bounded stdout pipe, and a deadline; failure
//! falls back to the same short platform list Orca shows before discovery.

use std::io;
use std::process::Stdio;
use std::time::Duration;

use serde::Deserialize;
use tokio::io::AsyncReadExt;
use tokio::sync::OnceCell;
use tokio::time::timeout;

#[cfg(not(target_os = "macos"))]
const FONT_LIST_TIMEOUT: Duration = Duration::from_secs(15);
#[cfg(target_os = "macos")]
const MAC_FONT_LIST_TIMEOUT: Duration = Duration::from_secs(45);
#[cfg(not(target_os = "macos"))]
const FONT_LIST_MAX_BYTES: usize = 8 * 1024 * 1024;
#[cfg(target_os = "macos")]
const MAC_FONT_LIST_MAX_BYTES: usize = 32 * 1024 * 1024;

static FONTS: OnceCell<Vec<String>> = OnceCell::const_new();

struct FontCommand {
    program: &'static str,
    args: &'static [&'static str],
    max_bytes: usize,
    deadline: Duration,
}

/// Return installed families, or a small useful platform list if discovery is
/// unavailable. The result is cached because macOS font inventory is costly.
pub(crate) async fn list() -> Vec<String> {
    FONTS
        .get_or_init(|| async {
            match load().await {
                Ok(fonts) if !fonts.is_empty() => fonts,
                Ok(_) | Err(_) => fallback(),
            }
        })
        .await
        .clone()
}

async fn load() -> io::Result<Vec<String>> {
    let command = platform_command();
    let output = run_bounded(&command).await?;
    let families = if cfg!(target_os = "macos") {
        parse_macos(&output)?
    } else if cfg!(windows) {
        parse_lines(&output, false)
    } else {
        parse_lines(&output, true)
    };
    Ok(unique_sorted(families))
}

#[cfg(target_os = "macos")]
fn platform_command() -> FontCommand {
    FontCommand {
        program: "system_profiler",
        args: &["SPFontsDataType", "-json"],
        max_bytes: MAC_FONT_LIST_MAX_BYTES,
        deadline: MAC_FONT_LIST_TIMEOUT,
    }
}

#[cfg(windows)]
fn platform_command() -> FontCommand {
    FontCommand {
        program: "powershell.exe",
        args: &[
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            "Add-Type -AssemblyName System.Drawing; $fonts = New-Object System.Drawing.Text.InstalledFontCollection; $fonts.Families | ForEach-Object { $_.Name }",
        ],
        max_bytes: FONT_LIST_MAX_BYTES,
        deadline: FONT_LIST_TIMEOUT,
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
fn platform_command() -> FontCommand {
    FontCommand {
        program: "fc-list",
        args: &[":", "family"],
        max_bytes: FONT_LIST_MAX_BYTES,
        deadline: FONT_LIST_TIMEOUT,
    }
}

async fn run_bounded(spec: &FontCommand) -> io::Result<String> {
    let mut command = crate::proc::quiet_tokio_command(spec.program);
    command
        .args(spec.args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);

    let mut child = command.spawn()?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("font inventory stdout was not piped"))?;
    let mut limited = stdout.take(spec.max_bytes as u64 + 1);
    let result = timeout(spec.deadline, async {
        let mut bytes = Vec::new();
        limited.read_to_end(&mut bytes).await?;
        if bytes.len() > spec.max_bytes {
            let _ = child.kill().await;
            let _ = child.wait().await;
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "font inventory exceeded its output limit",
            ));
        }
        let status = child.wait().await?;
        if !status.success() {
            return Err(io::Error::other("font inventory command failed"));
        }
        String::from_utf8(bytes)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "font inventory was not UTF-8"))
    })
    .await;

    match result {
        Ok(output) => output,
        Err(_) => {
            drop(limited);
            let _ = child.kill().await;
            let _ = child.wait().await;
            Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "font inventory timed out",
            ))
        }
    }
}

#[derive(Deserialize)]
struct MacFontReport {
    #[serde(rename = "SPFontsDataType", default)]
    fonts: Vec<MacFont>,
}

#[derive(Deserialize)]
struct MacFont {
    #[serde(default)]
    typefaces: Vec<MacTypeface>,
}

#[derive(Deserialize)]
struct MacTypeface {
    family: Option<String>,
}

fn parse_macos(output: &str) -> io::Result<Vec<String>> {
    let report: MacFontReport = serde_json::from_str(output)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    Ok(report
        .fonts
        .into_iter()
        .flat_map(|font| font.typefaces)
        .filter_map(|face| face.family)
        .collect())
}

fn parse_lines(output: &str, comma_separated: bool) -> Vec<String> {
    output
        .lines()
        .flat_map(|line| {
            if comma_separated {
                line.split(',').collect::<Vec<_>>()
            } else {
                vec![line]
            }
        })
        .map(str::to_string)
        .collect()
}

fn unique_sorted(values: Vec<String>) -> Vec<String> {
    let mut values = values
        .into_iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty() && !value.starts_with('.'))
        .collect::<Vec<_>>();
    values.sort_by_key(|value| value.to_lowercase());
    values.dedup();
    values
}

#[cfg(target_os = "macos")]
fn fallback() -> Vec<String> {
    ["SF Mono", "Menlo", "Monaco", "JetBrains Mono", "Fira Code"]
        .map(str::to_string)
        .to_vec()
}

#[cfg(windows)]
fn fallback() -> Vec<String> {
    [
        "Cascadia Mono",
        "Consolas",
        "Lucida Console",
        "JetBrains Mono",
        "Fira Code",
    ]
    .map(str::to_string)
    .to_vec()
}

#[cfg(not(any(target_os = "macos", windows)))]
fn fallback() -> Vec<String> {
    [
        "JetBrains Mono",
        "Fira Code",
        "DejaVu Sans Mono",
        "Liberation Mono",
        "Ubuntu Mono",
        "Noto Sans Mono",
    ]
    .map(str::to_string)
    .to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mac_inventory_reads_families_not_face_names() {
        let parsed = parse_macos(
            r#"{"SPFontsDataType":[{"typefaces":[{"family":"SF Mono"},{"family":"Menlo"}]},{"typefaces":[{"family":"SF Mono"},{"family":null}]}]}"#,
        )
        .expect("valid profiler report");
        assert_eq!(
            unique_sorted(parsed),
            vec!["Menlo".to_string(), "SF Mono".to_string()]
        );
    }

    #[test]
    fn linux_inventory_splits_every_family_alias() {
        assert_eq!(
            unique_sorted(parse_lines(
                "JetBrains Mono,JetBrains Mono NL\n.hidden\n Fira Code \n",
                true,
            )),
            vec![
                "Fira Code".to_string(),
                "JetBrains Mono".to_string(),
                "JetBrains Mono NL".to_string(),
            ]
        );
    }

    #[test]
    fn windows_inventory_keeps_a_family_with_spaces_whole() {
        assert_eq!(
            unique_sorted(parse_lines("Cascadia Mono\r\nConsolas\r\n", false)),
            vec!["Cascadia Mono".to_string(), "Consolas".to_string()]
        );
    }
}
