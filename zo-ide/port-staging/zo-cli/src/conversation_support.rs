use std::env;
use std::path::Path;

use runtime::{
    ContentBlock, MessageRole, PermissionMode, PermissionPolicy, Session,
};
use tools::GlobalToolRegistry;

use crate::default_prompt_date;

pub(crate) fn render_export_text(session: &Session) -> String {
    let mut lines = vec!["# Conversation Export".to_string(), String::new()];
    for (index, message) in session.messages.iter().enumerate() {
        let role = match message.role {
            MessageRole::System => "system",
            MessageRole::User => "user",
            MessageRole::Assistant => "assistant",
            MessageRole::Tool => "tool",
        };
        lines.push(format!("## {}. {role}", index + 1));
        for block in &message.blocks {
            match block {
                ContentBlock::Text { text } => lines.push(text.clone()),
                ContentBlock::ToolUse { id, name, input } => {
                    lines.push(format!("[tool_use id={id} name={name}] {input}"));
                }
                ContentBlock::ToolResult {
                    tool_use_id,
                    tool_name,
                    output,
                    is_error,
                    ..
                } => {
                    lines.push(format!(
                        "[tool_result id={tool_use_id} name={tool_name} error={is_error}] {output}"
                    ));
                }
                ContentBlock::Image { media_type, .. } => {
                    lines.push(format!("[image: {media_type}]"));
                }
                ContentBlock::Thinking { .. } => lines.push("[thinking]".to_string()),
                ContentBlock::RedactedThinking { .. } => {
                    lines.push("[redacted thinking]".to_string());
                }
            }
        }
        lines.push(String::new());
    }
    lines.join("\n")
}

/// Provider key prefixes whose trailing secret run is masked on the share-gist
/// upload path.
const SECRET_PREFIXES: &[&str] = &["sk-ant-", "ghp_", "gho_", "ghs_", "github_pat_", "AKIA"];
/// Identifier suffixes that mark a `<name>=<value>` assignment as sensitive.
const SECRET_ASSIGN_KEYS: &[&str] = &["KEY", "SECRET", "TOKEN", "PASSWORD"];
const REDACTED: &str = "[REDACTED]";

/// Best-effort secret redaction, applied *only* to the `/share gist` upload
/// path — never to [`render_export_text`] itself, so local exports and every
/// other consumer keep the verbatim transcript. It masks the highest-signal
/// secret shapes: provider key prefixes (`sk-ant-…`, `ghp_…`, `AKIA…`),
/// `KEY=`/`SECRET=`/`TOKEN=`/`PASSWORD=` assignments, and `Authorization:`
/// header credentials. This *reduces* exposure; it does not guarantee removal,
/// and the upload warning says exactly that. No regex / new dependency — a
/// whitespace-token scan keeps non-secret text byte-for-byte intact.
pub(crate) fn redact_for_share(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + REDACTED.len());
    for (index, line) in text.split('\n').enumerate() {
        if index > 0 {
            out.push('\n');
        }
        out.push_str(&redact_line(line));
    }
    out
}

/// Redact one line, preserving its original whitespace separators so untouched
/// text round-trips exactly. Tracks an `Authorization:` header across tokens so
/// an opaque (prefix-less) bearer credential is still masked.
fn redact_line(line: &str) -> String {
    let mut result = String::with_capacity(line.len());
    let mut expect_credential = false;
    let mut rest = line;
    while !rest.is_empty() {
        let ws_end = rest
            .find(|c: char| !c.is_whitespace())
            .unwrap_or(rest.len());
        result.push_str(&rest[..ws_end]);
        rest = &rest[ws_end..];
        if rest.is_empty() {
            break;
        }
        let tok_end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        let token = &rest[..tok_end];
        rest = &rest[tok_end..];

        if expect_credential {
            if is_auth_scheme(token) {
                // Keep the scheme word (Bearer / token / Basic); the credential
                // is the *next* token.
                result.push_str(token);
            } else {
                result.push_str(&mask_credential(token));
                expect_credential = false;
            }
            continue;
        }

        result.push_str(&redact_token(token));
        expect_credential = is_auth_marker(token);
    }
    result
}

/// Mask provider-key prefixes and sensitive `<name>=<value>` assignments inside
/// a single token. Returns the token unchanged when nothing matches.
fn redact_token(token: &str) -> String {
    for prefix in SECRET_PREFIXES {
        if let Some(pos) = token.find(prefix) {
            return format!("{}{prefix}{REDACTED}", &token[..pos]);
        }
    }
    if let Some(eq) = token.find('=') {
        let name = &token[..eq];
        let has_value = eq + 1 < token.len();
        let ident_start = name
            .rfind(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .map_or(0, |i| i + 1);
        let ident = name[ident_start..].to_ascii_uppercase();
        let sensitive = SECRET_ASSIGN_KEYS
            .iter()
            .any(|key| ident == *key || ident.ends_with(&format!("_{key}")));
        if has_value && sensitive {
            return format!("{name}={REDACTED}");
        }
    }
    token.to_string()
}

/// Mask the leading credential run of a token, preserving leading/trailing
/// punctuation (e.g. surrounding JSON quotes) so structure survives.
fn mask_credential(token: &str) -> String {
    let start = token.find(|c: char| c.is_ascii_alphanumeric()).unwrap_or(0);
    let end = token[start..]
        .find(|c: char| !(c.is_ascii_alphanumeric() || "+/=_-.".contains(c)))
        .map_or(token.len(), |offset| start + offset);
    if start >= end {
        return token.to_string();
    }
    format!("{}{REDACTED}{}", &token[..start], &token[end..])
}

/// Whether `token` opens an `Authorization` header (tolerating leading quotes
/// from a JSON-serialized tool payload, e.g. `"Authorization":`).
fn is_auth_marker(token: &str) -> bool {
    token
        .trim_start_matches(|c: char| !c.is_ascii_alphabetic())
        .to_ascii_lowercase()
        .starts_with("authorization")
}

/// Whether `token` is an auth *scheme* word that precedes the real credential.
fn is_auth_scheme(token: &str) -> bool {
    let word = token.trim_matches(|c: char| !c.is_ascii_alphabetic());
    word.eq_ignore_ascii_case("bearer")
        || word.eq_ignore_ascii_case("token")
        || word.eq_ignore_ascii_case("basic")
}

/// Build the main-loop system prompt for `model`.
///
/// `model` must be the session's *settled* model — the one the runtime is
/// about to be bound to, after preference resolution — because it is what the
/// prompt's `Model family:` line reports back to the model itself.
pub(crate) fn build_system_prompt_for_mode(
    cwd: &Path,
    mode: runtime::PromptMode,
    model: &str,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    // Main-loop prompt: the only surface that carries the configured
    // `outputStyle` (sub-agents and status views stay on the stock prompt).
    let mut sections = runtime::load_system_prompt_for_main_with_mode(
        cwd,
        default_prompt_date(),
        env::consts::OS,
        "unknown",
        Some(model),
        mode,
    )?;
    // Deferred-tool manifest: the model cannot ToolSearch for names it has
    // never seen. Session-stable, so it never disturbs the prompt cache.
    // Appended here rather than in the runtime builder because the deferred
    // set is owned by the tools crate, which depends on runtime — not vice
    // versa.
    sections.push(tools::deferred_tool_manifest_section());
    Ok(sections)
}

pub(crate) use runtime::final_assistant_text;

pub(crate) fn collect_tool_uses(summary: &runtime::TurnSummary) -> Vec<serde_json::Value> {
    summary
        .assistant_messages
        .iter()
        .flat_map(|message| message.blocks.iter())
        .filter_map(|block| match block {
            ContentBlock::ToolUse { id, name, input } => Some(serde_json::json!({
                "id": id,
                "name": name,
                "input": input,
            })),
            _ => None,
        })
        .collect()
}

pub(crate) fn collect_tool_results(summary: &runtime::TurnSummary) -> Vec<serde_json::Value> {
    summary
        .tool_results
        .iter()
        .flat_map(|message| message.blocks.iter())
        .filter_map(|block| match block {
            ContentBlock::ToolResult {
                tool_use_id,
                tool_name,
                output,
                is_error,
                ..
            } => Some(serde_json::json!({
                "tool_use_id": tool_use_id,
                "tool_name": tool_name,
                "output": output,
                "is_error": is_error,
            })),
            _ => None,
        })
        .collect()
}

pub(crate) fn collect_prompt_cache_events(
    summary: &runtime::TurnSummary,
) -> Vec<serde_json::Value> {
    summary
        .prompt_cache_events
        .iter()
        .map(|event| {
            serde_json::json!({
                "unexpected": event.unexpected,
                "reason": event.reason,
                "previous_cache_read_input_tokens": event.previous_cache_read_input_tokens,
                "current_cache_read_input_tokens": event.current_cache_read_input_tokens,
                "token_drop": event.token_drop,
                // Cache-efficiency forensics (prompt-cache diagnostic
                // instrumentation): set on the request where a low-cache-hit
                // streak first reaches its warning threshold. `null` on the
                // overwhelming majority of events — this is the only surface
                // that carries it today (no TUI render path consumes it yet).
                "warning": event.warning,
            })
        })
        .collect()
}

pub(crate) fn permission_policy(
    mode: PermissionMode,
    feature_config: &runtime::RuntimeFeatureConfig,
    tool_registry: &GlobalToolRegistry,
) -> Result<PermissionPolicy, String> {
    Ok(tool_registry
        .permission_specs(None)
        .map_err(|e| e.to_string())?
        .into_iter()
        .fold(
            PermissionPolicy::new(mode).with_permission_rules(feature_config.permission_rules()),
            |policy, (name, required_permission)| {
                policy.with_tool_requirement(name, required_permission)
            },
        ))
}

// `convert_messages` and the cache-breakpoint marking live in the runtime
// crate (the single source of truth, beside the
// `context_compression::wire_tool_output` they depend on) so the sub-agent
// provider clients in `crates/tools` share them. Re-export under the in-crate
// names so existing `pub(crate)` callers stay unchanged.
pub(crate) use runtime::{convert_messages, mark_conversation_cache_breakpoints};

#[cfg(test)]
mod system_prompt_tests {
    use super::build_system_prompt_for_mode;
    use std::fs;
    use std::path::PathBuf;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "zo-system-prompt-{name}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn build_system_prompt_for_reads_supplied_cwd() {
        let root = temp_dir("cwd");
        fs::write(root.join("context.md"), "SUPPLIED_CWD_RULE").expect("write instructions");

        let prompt =
            build_system_prompt_for_mode(&root, runtime::PromptMode::Interactive, "claude-opus-5")
                .expect("system prompt should load")
                .join("\n\n");

        assert!(prompt.contains("SUPPLIED_CWD_RULE"), "{prompt}");
        fs::remove_dir_all(root).ok();
    }

    /// The main-loop prompt reports the SESSION's model back to it. Before this
    /// the `Model family:` line was a hardcoded release name, so every session
    /// — and every sub-agent — was told it was that model no matter what it
    /// actually ran on.
    #[test]
    fn main_prompt_reports_the_bound_model() {
        let root = temp_dir("bound-model");

        let fable =
            build_system_prompt_for_mode(&root, runtime::PromptMode::Interactive, "claude-fable-5")
                .expect("system prompt should load")
                .join("\n\n");
        assert!(fable.contains("Model family: Claude Fable 5"), "{fable}");
        assert!(!fable.contains("Opus"), "fable session must not claim Opus");

        let opus =
            build_system_prompt_for_mode(&root, runtime::PromptMode::Interactive, "claude-opus-5")
                .expect("system prompt should load")
                .join("\n\n");
        assert!(opus.contains("Model family: Claude Opus 5"), "{opus}");

        fs::remove_dir_all(root).ok();
    }
}

#[cfg(test)]
mod redaction_tests {
    use super::redact_for_share;

    #[test]
    fn masks_provider_key_prefixes() {
        let out = redact_for_share("key sk-ant-api03-SECRETVALUE end");
        assert!(out.contains("sk-ant-[REDACTED]"), "{out}");
        assert!(!out.contains("SECRETVALUE"), "{out}");
        // Surrounding non-secret words are untouched.
        assert!(out.starts_with("key ") && out.ends_with(" end"), "{out}");

        for token in [
            "ghp_abcDEF123",
            "gho_xyz",
            "github_pat_11ABC",
            "AKIAIOSFODNN7EXAMPLE",
        ] {
            let line = format!("token={token}");
            assert!(
                !redact_for_share(&line).contains(token),
                "expected {token} masked"
            );
        }
    }

    #[test]
    fn masks_secret_assignments_only_for_sensitive_names() {
        let out =
            redact_for_share("AWS_SECRET=abc123 API_KEY=def GITHUB_TOKEN=ghi DB_PASSWORD=jkl");
        for leaked in ["abc123", "=def", "ghi", "jkl"] {
            assert!(!out.contains(leaked), "leaked {leaked} in {out}");
        }
        assert_eq!(out.matches("=[REDACTED]").count(), 4, "{out}");
        // A non-secret name ending in similar letters is NOT masked.
        let monkey = redact_for_share("MONKEY=banana");
        assert!(monkey.contains("banana"), "{monkey}");
    }

    #[test]
    fn masks_authorization_credentials_with_and_without_scheme() {
        let bearer = redact_for_share("Authorization: Bearer opaqueTOKEN123");
        assert!(bearer.contains("Bearer"), "scheme kept: {bearer}");
        assert!(!bearer.contains("opaqueTOKEN123"), "{bearer}");

        let json = redact_for_share(r#"[tool_result] {"Authorization":"Bearer abc.def.ghi"}"#);
        assert!(!json.contains("abc.def.ghi"), "{json}");
    }

    #[test]
    fn leaves_clean_text_byte_identical() {
        let clean = "## 1. user\nHello, please refactor the parser.\n\n## 2. assistant\nDone.";
        assert_eq!(redact_for_share(clean), clean);
    }
}

