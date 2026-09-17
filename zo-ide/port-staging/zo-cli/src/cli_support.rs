use std::io::{self, Write};

use commands::{render_slash_command_help, resume_supported_slash_commands, slash_command_usage};

use crate::{
    BUILD_DATE, BUILD_TARGET, GIT_SHA, LATEST_SESSION_REFERENCE, PRIMARY_SESSION_EXTENSION, VERSION,
};

pub(crate) fn render_version_report() -> String {
    let git_sha = GIT_SHA.unwrap_or("unknown");
    let target = BUILD_TARGET.unwrap_or("unknown");
    let build_date = BUILD_DATE.unwrap_or("unknown");
    format!(
        "zo\n  Version          {VERSION}\n  Git SHA          {git_sha}\n  Target           {target}\n  Build date       {build_date}"
    )
}

/// One-line build identity: `zo 0.1.0 (0e71f613b84e, 2026-07-10)`.
///
/// Unlike the first line of [`render_version_report`] (a bare `zo`), this
/// carries the SHA and build date so a reader can tell *which commit* the
/// running binary is — the answer to "did my rebuild actually land?". Used by
/// `/doctor` and the boot banner.
pub(crate) fn render_version_line() -> String {
    let git_sha = GIT_SHA.unwrap_or("unknown");
    let build_date = BUILD_DATE.unwrap_or("unknown");
    format!("zo {VERSION} ({git_sha}, {build_date})")
}

fn write_lines(out: &mut impl Write, lines: &[&str]) -> io::Result<()> {
    for line in lines {
        writeln!(out, "{line}")?;
    }
    Ok(())
}

fn render_resume_safe_commands() -> String {
    resume_supported_slash_commands()
        .into_iter()
        .map(slash_command_usage)
        .collect::<Vec<_>>()
        .join(", ")
}

pub(crate) fn print_help_to(out: &mut impl Write) -> io::Result<()> {
    write_lines(
        out,
        &[
            &format!("zo v{VERSION}"),
            "",
            "Usage:",
            "  zo [--fullscreen] [--model MODEL] [--allowedTools TOOL[,TOOL...]]",
            "      Start the interactive REPL",
            "  zo [--model MODEL] [--output-format text|json|stream-json] prompt TEXT",
            "      Send one prompt and exit",
            "  zo [--model MODEL] [--output-format text|json|stream-json] TEXT",
            "      Shorthand non-interactive prompt mode",
            "  zo --resume [SESSION.jsonl|session-id|latest] [/status] [/compact] [...]",
            "      Inspect or maintain a saved session without entering the REPL",
            "  zo help",
            "      Alias for --help",
            "  zo version",
            "      Alias for --version",
            "  zo status",
            "      Show the current local workspace status snapshot",
            "  zo sandbox",
            "      Show the current sandbox isolation snapshot",
            "  zo doctor [--check]",
            "      Diagnose the local setup and apply safe repairs (--check: read-only)",
            "  zo update [--check]",
            "      Update an installer-managed official stable release",
            "  zo dump-manifests",
            "  zo bootstrap-plan",
            "  zo agents",
            "  zo mcp",
            "  zo skills",
            "  zo system-prompt [--cwd PATH] [--date YYYY-MM-DD]",
            "  zo login",
            "  zo logout",
            "  zo init",
            "  zo serve [--bind ADDR] [--port N]",
            "      Run a persistent session server (sessions survive client disconnects)",
            "  zo acp",
            "      Run as an Agent Client Protocol agent over stdio",
            "  zo attach [SESSION_ID] [--plain] [--bind ADDR] [--port N]",
            "      Attach to a `zo serve` server (rich TUI; --plain for the line client);",
            "      omit SESSION_ID to start a new session",
            "",
            "Flags:",
            "  --model MODEL              Override the active model",
            "  --fullscreen               Use the alternate screen with the right-hand panel",
            "  --inline                   Use native scrollback (the default; kept for scripts)",
            "  --output-format FORMAT     Non-interactive output: text, json, or stream-json (alias: ndjson)",
            "  --input-format FORMAT      Non-interactive input: text or stream-json (aliases: json, ndjson)",
            "  --permission-mode MODE     Set read-only, workspace-write, or danger-full-access",
            "  --dangerously-skip-permissions  Skip all permission checks",
            "  --allowedTools TOOLS       Restrict enabled tools (repeatable; comma-separated aliases supported)",
            "  --max-turns N              Cap the non-interactive agentic loop",
            "  --check CMD                Objective check for this run: edits are auto-verified",
            "                             against CMD exiting 0 (e.g. --check \"npm test\")",
            "  --max-tool-calls N         Cap model-requested tool calls per non-interactive turn",
            "  --settings FILE            Merge an extra settings document with highest precedence",
            "  --strict-mcp-config        Use MCP servers only from --mcp-config (ignore settings)",
            "  --add-dir PATH             Grant an additional workspace root (repeatable)",
            "  --session-id ID            Explicit session id for a non-interactive run",
            "  --fallback-model MODEL     Retry a failed -p run once on this model (overload/429)",
            "  --version, -V              Print version and build information locally",
            "",
            "Interactive slash commands:",
        ],
    )?;
    writeln!(out, "{}", render_slash_command_help())?;
    writeln!(out)?;
    writeln!(
        out,
        "Resume-safe commands: {}",
        render_resume_safe_commands()
    )?;
    writeln!(out)?;
    write_lines(
        out,
        &[
            "Session shortcuts:",
            &format!(
                "  REPL turns auto-save to ~/.zo/projects/<project>/sessions/<session-id>.{PRIMARY_SESSION_EXTENSION}"
            ),
            &format!(
                "  Use `{LATEST_SESSION_REFERENCE}` with --resume, /resume, or /session switch to target the newest saved session"
            ),
            "  Use /session list in the REPL to browse managed sessions",
            "Examples:",
            "  zo --model claude-opus \"summarize this repo\"",
            "  zo --output-format json prompt \"explain src/main.rs\"",
            "  zo --allowedTools read,glob \"summarize Cargo.toml\"",
            &format!("  zo --resume {LATEST_SESSION_REFERENCE}"),
            &format!("  zo --resume {LATEST_SESSION_REFERENCE} /status /diff /export notes.txt"),
            "  zo agents",
            "  zo mcp show my-server",
            "  zo /skills",
            "  zo login",
            "  zo init",
        ],
    )?;
    Ok(())
}

pub(crate) fn print_help() {
    let _ = print_help_to(&mut io::stdout());
}
