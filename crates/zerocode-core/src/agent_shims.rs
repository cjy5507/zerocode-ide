//! Installed surface shims, including the window's private-token loaders.
//! Installers and surface metadata consumers share these exact bytes.

mod pane_grant;

pub use pane_grant::{PaneTeamGrant, orchestration_program, read_pane_team_grant};

const PANE_GRANTS_DIR_NAME: &str = "panes";

/// The installed ledger command has its own directory, separate from tmux.
pub const ORCHESTRATION_DIR_NAME: &str = "orchestration-bin";
/// The ledger command's name before a platform launcher suffix.
pub const ORCHESTRATION_COMMAND: &str = "zerocode-orc";

/// Surface command whose installed bytes can attest host metadata.
#[derive(Clone, Copy, Debug)]
pub enum SurfaceShim {
    /// The window's managed browser command.
    Browser,
    /// The window's scoped desktop command.
    Computer,
}

impl SurfaceShim {
    /// Command name without the Windows launcher suffix.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Browser => crate::agent_browser::BROWSER_CLI,
            Self::Computer => crate::computer_use::COMPUTER_CLI,
        }
    }
}

/// Complete installed POSIX or PowerShell script. PowerShell includes its BOM.
#[must_use]
pub fn installed_script(shim: SurfaceShim, windows: bool) -> String {
    let port = "ZEROCODE_HOOK_PORT";
    let hook_token = "ZEROCODE_HOOK_TOKEN";
    let (token, token_file) = match shim {
        SurfaceShim::Browser => ("ZEROCODE_BROWSER_TOKEN", "ZEROCODE_BROWSER_TOKEN_FILE"),
        SurfaceShim::Computer => ("ZEROCODE_COMPUTER_TOKEN", "ZEROCODE_COMPUTER_TOKEN_FILE"),
    };
    let script = match (shim, windows) {
        (SurfaceShim::Browser, false) => crate::agent_browser::shim_script(port, token, hook_token),
        (SurfaceShim::Browser, true) => {
            crate::agent_browser::shim_script_powershell(port, token, hook_token)
        }
        (SurfaceShim::Computer, false) => crate::computer_use::shim_script(port, token, hook_token),
        (SurfaceShim::Computer, true) => {
            crate::computer_use::shim_script_powershell(port, token, hook_token)
        }
    };
    if windows {
        format!(
            "\u{FEFF}{}",
            powershell_shim_script_with_private_tokens(script, &[(token, token_file)])
        )
    } else {
        shim_script_with_private_tokens(script, &[(token, token_file)])
    }
}

/// Insert the POSIX loader used by the window's installed agent commands.
#[must_use]
pub fn shim_script_with_private_tokens(script: String, private_tokens: &[(&str, &str)]) -> String {
    let endpoint = "ZEROCODE_HOOK_ENDPOINT";
    let pane_key = crate::hook::PANE_KEY_ENV;
    // A Windows window spells the endpoint path with backslashes; Git Bash
    // reads such a path but `${endpoint%/*}` below cannot cut a directory
    // off it. One spelling before anything looks at it.
    let mut loader = format!(
        r#"case "${{{endpoint}:-}}" in
  *\\*) {endpoint}=$(printf '%s' "${{{endpoint}}}" | tr '\\' '/') ;;
esac
if [ -n "${{{endpoint}:-}}" ] && [ -r "${{{endpoint}}}" ]; then
  . "${{{endpoint}}}" 2>/dev/null || :
fi
"#
    );
    let missing = private_tokens
        .iter()
        .map(|(_, file_var)| format!(r#"[ -z "${{{file_var}:-}}" ]"#))
        .collect::<Vec<_>>()
        .join(" || ");
    if !missing.is_empty() {
        loader.push_str(&format!(
            r#"if {{ {missing}; }} && [ -n "${{{endpoint}:-}}" ] && [ -n "${{{pane_key}:-}}" ]; then
  case "${{{pane_key}}}" in
    term-[0-9]*)
      grant="${{{endpoint}%/*}}/{PANE_GRANTS_DIR_NAME}/${{{pane_key}}}.env"
      if [ -r "$grant" ]; then
        . "$grant" 2>/dev/null || :
      fi
      ;;
  esac
fi
"#
        ));
    }
    for (token_var, file_var) in private_tokens {
        loader.push_str(&format!(
            r#"if [ -n "${{{file_var}:-}}" ] && [ -r "${{{file_var}}}" ]; then
  {token_var}=$({{ command -p cat "${{{file_var}}}" 2>/dev/null || cat "${{{file_var}}}"; }}) || :
  export {token_var}
fi
"#
        ));
    }
    let Some((_, rest)) = script.split_once('\n') else {
        return script;
    };
    format!(
        "{}\n{}{}",
        script.lines().next().unwrap_or_default(),
        loader,
        rest
    )
}

/// Prefix the PowerShell loader used by installed agent commands.
#[must_use]
pub fn powershell_shim_script_with_private_tokens(
    script: String,
    private_tokens: &[(&str, &str)],
) -> String {
    let endpoint = "ZEROCODE_HOOK_ENDPOINT";
    let hook_token = "ZEROCODE_HOOK_TOKEN";
    let mut loader = format!(
        r#"$endpoint = $env:{endpoint}
if ($endpoint -and (Test-Path -LiteralPath $endpoint -PathType Leaf)) {{
  foreach ($line in (Get-Content -LiteralPath $endpoint -ErrorAction SilentlyContinue)) {{
    if ($line.StartsWith('{hook_token}=')) {{
      $env:{hook_token} = $line.Substring('{hook_token}='.Length)
    }}
  }}
}}
"#
    );
    for (token_var, file_var) in private_tokens {
        loader.push_str(&format!(
            r#"$tokenFile = $env:{file_var}
if ($tokenFile -and (Test-Path -LiteralPath $tokenFile -PathType Leaf)) {{
  try {{ $env:{token_var} = [System.IO.File]::ReadAllText($tokenFile).Trim() }} catch {{}}
}}
"#
        ));
    }
    format!("{loader}{script}")
}
