//! A private ATIF view of the durable session, published at a host save boundary.

use std::fs;
use std::io;

use serde::Serialize;
use serde_json::{json, Value};

use crate::usage::TokenUsage;

use super::{
    acquire_writer_lock, parse_vault_records, read_secure_regular_file, vault_path_for,
    write_atomic_secure, ContentBlock, ConversationMessage, FileFingerprint, MessageRole, Session,
    SessionError, ToolCallPairing, VaultRecord, PROCESS_EVENT_PREFIX,
};

/// Suffix of the ATIF document beside its resumable session transcript.
pub const ATIF_DOCUMENT_SUFFIX: &str = ".atif.json";

impl Session {
    /// Atomically refresh the ATIF document beside this session's persisted transcript.
    ///
    /// The host calls this after saving a turn or changing history. Use the
    /// durable snapshot under the same writer lease, so a partial save, a
    /// stale clone or a transient request message cannot enter the document.
    /// `ZO_DISABLE_ATIF` disables publication for this process.
    pub fn persist_atif(&self, agent_version: &str) -> Result<(), SessionError> {
        self.persist_atif_with_setting(agent_version, None)
    }

    pub(super) fn persist_atif_with_setting(
        &self,
        agent_version: &str,
        _setting: Option<&str>,
    ) -> Result<(), SessionError> {
        let Some(persistence) = self.persistence.as_ref() else { return Ok(()); };
        if std::env::var_os("ZO_DISABLE_ATIF").is_some() {
            return Ok(());
        }
        let path = &persistence.path;
        let mut writer = persistence.writer.lock().map_err(|_| {
            SessionError::Conflict("session writer lease poisoned".to_string())
        })?;
        let Some(expected) = writer.expected.clone() else { return Ok(()); };
        acquire_writer_lock(path, &mut writer)?;
        let target = path.with_extension(ATIF_DOCUMENT_SUFFIX.trim_start_matches('.'));
        if writer.atif_revision.as_ref() == Some(&expected) && target.is_file() {
            return Ok(());
        }

        #[cfg(test)]
        super::FULL_FILE_READS.with(|count| count.set(count.get() + 1));
        let contents = read_secure_regular_file(path)?;
        if FileFingerprint::of_bytes(contents.as_bytes()) != expected {
            return Err(SessionError::Conflict("session changed before ATIF publication".to_string()));
        }
        let snapshot = Self::load_from_contents(&contents, None)?;
        drop(contents);
        let vault = match read_secure_regular_file(&vault_path_for(path)) {
            Ok(text) => parse_vault_records(&text),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Vec::new(),
            Err(error) => return Err(error.into()),
        };
        match project(&snapshot, &vault, agent_version) {
            Some(document) => {
                let json = serde_json::to_string(&document).map_err(io::Error::other)?;
                write_atomic_secure(&target, &json)?;
            }
            None => match fs::remove_file(&target) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            },
        }
        writer.atif_revision = Some(expected);
        Ok(())
    }
}

#[derive(Serialize)]
struct Document<'a> {
    schema_version: &'static str,
    session_id: &'a str,
    agent: Agent<'a>,
    steps: Vec<Step>,
    #[serde(skip_serializing_if = "Option::is_none")]
    final_metrics: Option<Value>,
}

#[derive(Serialize)]
struct Agent<'a> {
    name: &'static str,
    version: &'a str,
}

#[derive(Serialize)]
struct Step {
    #[serde(rename = "step_id")]
    id: usize,
    source: &'static str,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    timestamp: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    model_name: Option<String>,
    #[serde(skip_serializing_if = "String::is_empty")]
    reasoning_content: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tool_calls: Vec<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    observation: Option<Observation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    metrics: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    extra: Option<Value>,
}

#[derive(Default, Serialize)]
struct Observation {
    results: Vec<Value>,
}

impl Step {
    fn new(id: usize, source: &'static str, message: String, at_ms: Option<u64>) -> Self {
        Self {
            id,
            source,
            message,
            timestamp: at_ms.and_then(timestamp),
            model_name: None,
            reasoning_content: String::new(),
            tool_calls: Vec::new(),
            observation: None,
            metrics: None,
            extra: None,
        }
    }

    fn observe(&mut self, result: Value) {
        self.observation.get_or_insert_with(Observation::default).results.push(result);
    }
}

#[derive(Default)]
struct Trajectory<'a> {
    steps: Vec<Step>,
    calls: Vec<(usize, String)>,
    pairing: ToolCallPairing<'a>,
    // ATIF's cached_tokens is a subset of the input, not an extra charge.
    totals: Option<[u64; 3]>,
}

impl<'a> Trajectory<'a> {
    fn push_message(&mut self, message: &'a ConversationMessage) {
        let text = message_text(message);
        if message.role == MessageRole::System && text.starts_with(PROCESS_EVENT_PREFIX) {
            return;
        }
        let source = match message.role {
            MessageRole::Assistant => Some("agent"),
            MessageRole::User if message.origin.is_none() && !message.holds_tool_result() => Some("user"),
            MessageRole::Tool | MessageRole::User if text.is_empty() => None,
            _ => Some("system"),
        };
        if let Some(source) = source {
            let mut step = Step::new(self.steps.len() + 1, source, text, message.updated_at_ms);
            if message.role == MessageRole::Assistant {
                self.fill_agent_step(&mut step, message);
            }
            self.steps.push(step);
        }

        for block in &message.blocks {
            if let ContentBlock::ToolResult { tool_use_id, output, is_error, images, .. } = block {
                let mut result = json!({"content": output});
                if *is_error || !images.is_empty() {
                    result["extra"] = json!({"is_error": is_error, "image_count": images.len()});
                }
                if let Some(occurrence) = self.pairing.result(tool_use_id) {
                    let (step, id) = &self.calls[occurrence];
                    result["source_call_id"] = json!(id);
                    self.steps[*step].observe(result);
                } else {
                    let mut step = Step::new(self.steps.len() + 1, "system", String::new(), message.updated_at_ms);
                    step.extra = Some(json!({"orphan_tool_call_id": tool_use_id}));
                    step.observe(result);
                    self.steps.push(step);
                }
            }
        }
    }

    fn fill_agent_step(&mut self, step: &mut Step, message: &'a ConversationMessage) {
        step.model_name.clone_from(&message.model);
        for block in &message.blocks {
            match block {
                ContentBlock::Thinking { thinking, .. } => step.reasoning_content.push_str(thinking),
                ContentBlock::ToolUse { id, name, input } => {
                    // Some providers reuse ids. ATIF identifies each occurrence.
                    let atif_id = format!("{}:{id}", self.calls.len() + 1);
                    let step_index = self.steps.len();
                    self.pairing.call(step_index, id, self.calls.len());
                    let arguments = serde_json::from_str::<serde_json::Map<String, Value>>(input);
                    let mut call = json!({"tool_call_id": atif_id, "function_name": name, "arguments": {}});
                    match arguments {
                        Ok(arguments) => call["arguments"] = Value::Object(arguments),
                        Err(_) => call["extra"] = json!({"raw_arguments": input}),
                    }
                    self.calls.push((step_index, atif_id));
                    step.tool_calls.push(call);
                }
                _ => {}
            }
        }
        if let Some(usage) = message.usage {
            step.metrics = Some(self.add_usage(usage));
        }
    }

    fn add_usage(&mut self, usage: TokenUsage) -> Value {
        let cached = u64::from(usage.cache_read_input_tokens);
        let created = u64::from(usage.cache_creation_input_tokens);
        let prompt = u64::from(usage.input_tokens) + cached + created;
        let completion = u64::from(usage.output_tokens);
        let totals = self.totals.get_or_insert([0; 3]);
        for (total, count) in totals.iter_mut().zip([prompt, completion, cached]) {
            *total = total.saturating_add(count);
        }
        let mut metrics = json!({"prompt_tokens": prompt, "completion_tokens": completion, "cached_tokens": cached});
        let mut extra = serde_json::Map::new();
        if created > 0 {
            extra.insert("cache_creation_input_tokens".to_string(), json!(created));
        }
        if let Some(details) = usage.output_tokens_details {
            extra.insert("reasoning_tokens".to_string(), json!(details.thinking_tokens));
        }
        if !extra.is_empty() {
            metrics["extra"] = Value::Object(extra);
        }
        metrics
    }
}

fn project<'a>(snapshot: &'a Session, vault: &[VaultRecord], agent_version: &'a str) -> Option<Document<'a>> {
    let mut trajectory = Trajectory::default();
    for record in vault.iter().take_while(|record| record.vault_seq < snapshot.first_message_index()) {
        trajectory.push_message(&record.message);
    }
    let mut live = snapshot.messages.as_slice();
    if let Some(compaction) = &snapshot.compaction {
        // The JSONL header also exists in older sessions without a stored
        // continuation message. Keep the summary in either form.
        let continuation = live.first().filter(|message| {
            message.role == MessageRole::System && matches!(
                message.blocks.as_slice(),
                [ContentBlock::Text { text }] if text.starts_with("This session is being continued from a previous conversation")
            )
        });
        let mut step = Step::new(
            trajectory.steps.len() + 1, "system", "Context compacted.".to_string(),
            continuation.and_then(|message| message.updated_at_ms),
        );
        step.extra = Some(json!({"context_management": {
            "type": "compaction",
            "count": compaction.count,
            "removed_message_count": compaction.removed_message_count,
        }}));
        let summary = continuation.map_or_else(|| compaction.summary.clone(), message_text);
        step.observe(json!({"content": summary}));
        trajectory.steps.push(step);
        if continuation.is_some() {
            live = &live[1..];
        }
    }
    for message in live {
        trajectory.push_message(message);
    }
    if trajectory.steps.is_empty() {
        return None;
    }
    let final_metrics = trajectory.totals.map(|[prompt, completion, cached]| {
        json!({
            "total_prompt_tokens": prompt,
            "total_completion_tokens": completion,
            "total_cached_tokens": cached,
        })
    });
    Some(Document {
        schema_version: "ATIF-v1.7",
        session_id: &snapshot.session_id,
        agent: Agent { name: "zo", version: agent_version },
        steps: trajectory.steps,
        final_metrics,
    })
}

fn message_text(message: &ConversationMessage) -> String {
    message.blocks.iter().filter_map(|block| match block {
        ContentBlock::Text { text } => Some(text.as_str()),
        ContentBlock::Image { .. } => Some("[image]"),
        _ => None,
    }).collect::<Vec<_>>().join("\n")
}

fn timestamp(millis: u64) -> Option<String> {
    // Older or imported sessions can carry arbitrary epoch values. ISO 8601
    // timestamps in Harbor's published validator have a four-digit year.
    if millis > 253_402_300_799_999 {
        return None;
    }
    let seconds = millis / 1_000;
    let date = crate::date::utc_date_from_unix_secs(seconds);
    let time = seconds % 86_400;
    Some(format!("{date}T{:02}:{:02}:{:02}.{:03}Z", time / 3_600, time / 60 % 60, time % 60, millis % 1_000))
}
