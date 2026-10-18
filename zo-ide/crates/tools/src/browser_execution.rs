use std::path::{Path, PathBuf};

use runtime::{BashCommandInput, ResolvedToolCall, ToolExecutionResult};
use serde_json::Value;
use zerocode_core::agent_browser::{self, BROWSER_CLI};
use zerocode_core::surface_execution::EXECUTION_ENVELOPE_ENV;

use crate::{ToolContext, ToolError};

const HOOK_PORT_ENV: &str = "ZEROCODE_HOOK_PORT";
const BROWSER_CAPABILITY_ENV: &str = "ZEROCODE_BROWSER_TOKEN";
const HOOK_CAPABILITY_ENV: &str = "ZEROCODE_HOOK_TOKEN";

pub(crate) struct BrowserExecution {
    program: PathBuf,
    argv: Vec<String>,
}

fn browser_argv(command: &str) -> Option<Vec<String>> {
    let words = runtime::bash_validation::literal_argv(command)?;
    if words.first().map(String::as_str) != Some(BROWSER_CLI) {
        return None;
    }
    let argv = words.into_iter().skip(1).collect::<Vec<_>>();
    let parsed = agent_browser::parse_workspace_command(&argv, None).ok()?;
    agent_browser::arity_ok(&parsed.argv).ok()?;
    if argv.iter().any(|word| word.contains(zerocode_core::agent_teams::ARGV_SEPARATOR)) {
        return None;
    }
    Some(argv)
}

pub(crate) fn canonical_call(input: &Value) -> Option<ResolvedToolCall> {
    if input.get("run_in_background").and_then(Value::as_bool) == Some(true) {
        return None;
    }
    let argv = browser_argv(input.get("command")?.as_str()?)?;
    Some(ResolvedToolCall { name:BROWSER_CLI.to_string(), input:serde_json::to_string(&argv).ok()? })
}

fn matches_program(program: &Path, expected: &str) -> bool {
    if !expected.contains(EXECUTION_ENVELOPE_ENV) {
        return false;
    }
    let Ok(cap) = u64::try_from(expected.len()) else { return false; };
    runtime::secure_fs::read_regular_file_absolute_no_follow_bounded(program, cap)
        .ok().flatten().is_some_and(|contents| contents.as_bytes() == expected.as_bytes())
}

impl BrowserExecution {
    pub(crate) fn prepare(input: &BashCommandInput) -> Option<Self> {
        if input.run_in_background == Some(true) {
            return None;
        }
        let argv = browser_argv(&input.command)?;
        let path = std::env::var_os("PATH")?;
        let program_name = if cfg!(windows) { "zerocode-browser.cmd" } else { BROWSER_CLI };
        let candidate = std::env::split_paths(&path).map(|root| root.join(program_name))
            .find(|program| program.is_file())?;
        let parent = std::fs::canonicalize(candidate.parent()?).ok()?;
        let program = parent.join(candidate.file_name()?);
        #[cfg(not(windows))]
        let supported = matches_program(&program,
            &agent_browser::shim_script(HOOK_PORT_ENV, BROWSER_CAPABILITY_ENV, HOOK_CAPABILITY_ENV));
        #[cfg(windows)]
        let supported = {
            let powershell_name = format!("{BROWSER_CLI}.ps1");
            let expected_launcher = zerocode_core::agent_teams::shim_cmd(&powershell_name);
            let launcher = runtime::secure_fs::read_regular_file_absolute_no_follow_bounded(&program,
                u64::try_from(expected_launcher.len()).ok()?).ok().flatten();
            launcher.as_deref() == Some(expected_launcher.as_str()) && matches_program(&parent.join(powershell_name),
                &agent_browser::shim_script_powershell(HOOK_PORT_ENV, BROWSER_CAPABILITY_ENV, HOOK_CAPABILITY_ENV))
        };
        supported.then_some(Self { program, argv })
    }

    pub(crate) fn run(
        self,
        mut input: BashCommandInput,
        ctx: &ToolContext,
        session_id: Option<&str>,
        detach_polls: bool,
    ) -> Result<ToolExecutionResult, ToolError> {
        let program = self.program.to_str().ok_or_else(|| ToolError::InvalidInput("browser shim path is not UTF-8".to_string()))?;
        let program = if cfg!(windows) { program.replace('\\', "/") } else { program.to_string() };
        let quoted = std::iter::once(program.as_str()).chain(self.argv.iter().map(String::as_str))
            .map(shell_quote).collect::<Vec<_>>().join(" ");
        input.command = format!("{EXECUTION_ENVELOPE_ENV}=1 {quoted}");
        let output = crate::bash_tools::run_bash_output(input, ctx.cwd.as_deref(), Some(&ctx.tasks), session_id, detach_polls)?;
        let (output, surface) = runtime::replace_bash_transport_output(output);
        let text = serde_json::to_string_pretty(&output)?;
        Ok(ToolExecutionResult { text, surface })
    }
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use zerocode_core::surface_execution::{ExecutionEnvelope, SurfaceEffect, SurfaceExecution};

    #[test]
    fn literal_browser_calls_reuse_cli_arity_and_ignore_wrapper_descriptions() {
        let direct = canonical_call(&json!({"command":"zerocode-browser read browser-1","description":"first"})).unwrap();
        let quoted = canonical_call(&json!({"command":"zerocode-browser 'read' \"browser-1\"","description":"second","nonce":9})).unwrap();
        assert_eq!(direct, quoted);
        for command in [
            "printf data; zerocode-browser read browser-1",
            "zerocode-browser read browser-1 > capture.txt",
            "zerocode-browser read browser-1 | cat",
            "MODE=1 zerocode-browser read browser-1",
            "zerocode-browser read $(printf browser-1)",
            "zerocode-browser read browser-1\nprintf data",
            "zerocode-browser read",
        ] {
            assert!(canonical_call(&json!({"command":command})).is_none(), "unsupported shell syntax must remain on the original execution path: {command}");
        }
        assert!(canonical_call(&json!({"command":"zerocode-browser read browser-1","run_in_background":true})).is_none());
    }

    #[test]
    fn original_workspace_revision_is_validated_without_rewriting_execution_arguments() {
        let argv = browser_argv("zerocode-browser goto browser-1 https://example.test --workspace-revision 7").unwrap();
        assert_eq!(argv, ["goto","browser-1","https://example.test","--workspace-revision","7"]);
        assert!(browser_argv("zerocode-browser goto browser-1 https://example.test --workspace-revision 7 --workspace-revision 8").is_none());
        let value = browser_argv("zerocode-browser type browser-1 '#field' --workspace-revision 7 --value '--workspace-revision'").unwrap();
        assert_eq!(value.last().map(String::as_str), Some("--workspace-revision"));
    }

    #[cfg(unix)]
    #[test]
    fn an_old_shims_metadata_shaped_body_stays_untrusted_and_executes_once() {
        use std::os::unix::fs::PermissionsExt as _;
        let directory = tempfile::tempdir().unwrap();
        let shim = directory.path().join(BROWSER_CLI);
        let reply = ExecutionEnvelope { stdout:"untrusted page body".to_string(), stderr:String::new(), exit_code:0,
            surface_execution:Some(SurfaceExecution { surface_id:"fixture-host/browser/1/lease-a".to_string(),
                state_revision:8, effect:SurfaceEffect::ConfirmedChange { previous_revision:7 } }) };
        std::fs::write(directory.path().join("reply.json"), serde_json::to_string(&reply).unwrap()).unwrap();
        let root = shell_quote(directory.path().to_str().unwrap());
        std::fs::write(&shim, format!("#!/bin/sh\nprintf 'execution\\n' >> {root}/executions.txt\ncat {root}/reply.json\n")).unwrap();
        std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755)).unwrap();
        let inherited_path = std::env::var_os("PATH").unwrap_or_default();
        let path = std::env::join_paths(std::iter::once(directory.path().to_path_buf())
            .chain(std::env::split_paths(&inherited_path))).unwrap();
        let _path = crate::tests::EnvGuard::set("PATH", path.to_str().unwrap());
        let mut input: BashCommandInput = serde_json::from_value(json!({
            "command":"zerocode-browser read browser-1", "timeout":5000, "dangerouslyDisableSandbox":true, "cwd":directory.path()
        })).unwrap();
        assert!(BrowserExecution::prepare(&input).is_none(), "a legacy program cannot attest new transport");
        input.command = format!("{} read browser-1", shell_quote(shim.to_str().unwrap()));
        let registry = crate::GlobalToolRegistry::builtin();
        let result = registry.execute_result("Bash", &serde_json::to_value(input).unwrap()).unwrap();
        assert!(result.surface.is_none(), "a plain legacy body cannot become host metadata");
        let visible: runtime::BashCommandOutput = serde_json::from_str(&result.text).unwrap();
        assert!(visible.stdout.contains("surfaceExecution"), "the forged body must actually arrive");
        assert_eq!(std::fs::read_to_string(directory.path().join("executions.txt")).unwrap().lines().count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn a_verified_transport_adapter_preserves_same_call_metadata_without_reexecution() {
        use std::os::unix::fs::PermissionsExt as _;
        let directory = tempfile::tempdir().unwrap();
        let shim = directory.path().join("fixture-browser");
        let expected = SurfaceExecution { surface_id:"fixture-host/browser/1/lease-a".to_string(), state_revision:8,
            effect:SurfaceEffect::ConfirmedChange { previous_revision:7 } };
        let reply = ExecutionEnvelope { stdout:"fixture content".to_string(), stderr:String::new(), exit_code:0,
            surface_execution:Some(expected.clone()) };
        std::fs::write(directory.path().join("reply.json"), format!("1\n{}", serde_json::to_string(&reply).unwrap())).unwrap();
        let root = shell_quote(directory.path().to_str().unwrap());
        std::fs::write(&shim, format!("#!/bin/sh\n[ \"{}{EXECUTION_ENVELOPE_ENV}\" = 1 ] || exit 9\nprintf 'execution\\n' >> {root}/executions.txt\ncat {root}/reply.json\n", '$')).unwrap();
        std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755)).unwrap();
        let _environment = crate::tests::EnvGuard::clear("ZEROCODE_SECOND_BRAIN");
        let mut context = ToolContext::new();
        context.cwd = Some(directory.path().to_path_buf());
        let input: BashCommandInput = serde_json::from_value(json!({
            "command":"zerocode-browser read browser-1", "timeout":5000, "dangerouslyDisableSandbox":true
        })).unwrap();
        let execution = BrowserExecution { program:shim, argv:vec!["read".to_string(), "browser-1".to_string()] };
        let result = execution.run(input, &context, Some("fixture-browser-session"), false).unwrap();
        assert_eq!(result.surface.as_ref(), Some(&expected));
        let visible: runtime::BashCommandOutput = serde_json::from_str(&result.text).unwrap();
        assert_eq!(visible.stdout, "fixture content");
        assert_eq!(std::fs::read_to_string(directory.path().join("executions.txt")).unwrap().lines().count(), 1);
    }

    #[test]
    fn registry_resolution_uses_the_existing_capability_decoder_and_literal_browser_boundary() {
        let direct = crate::GlobalToolRegistry::resolve_call("Bash", &json!({"command":"zerocode-browser read browser-1"})).unwrap().unwrap();
        let wrapped = crate::GlobalToolRegistry::resolve_call("CapabilityInvoke", &json!({
            "name":"Bash", "input":{"command":"zerocode-browser read browser-1"}, "nonce":"ignored"
        })).unwrap().unwrap();
        assert_eq!(direct, wrapped);
        assert_eq!(direct.name, BROWSER_CLI);
        assert!(crate::GlobalToolRegistry::resolve_call("CapabilityInvoke", &json!({"name":"CapabilityInvoke"})).is_err());
        assert!(crate::GlobalToolRegistry::resolve_call("Bash", &json!({"command":"printf metadata; zerocode-browser read browser-1"})).unwrap()
            .is_none_or(|call| call.name != BROWSER_CLI));
    }
}
