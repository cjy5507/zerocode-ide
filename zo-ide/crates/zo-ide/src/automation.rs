//! Per-invocation input, output, and runtime limits. No global defaults change.
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Clone, Default)]
pub struct Invocation {
    pub prompt: Option<String>,
    pub schema: Option<Arc<OutputSchema>>,
    /// Provider iterations across foreground turns; helpers have independent accounting.
    pub max_turns: Option<usize>,
    pub max_output_tokens: Option<u32>,
    pub additional_roots: Vec<PathBuf>,
    pub review: bool,
    pub used_turns: usize,
    pub used_output_tokens: u32,
    pub failure: Option<String>,
}

pub struct OutputSchema {
    validator: jsonschema::Validator,
    raw: String,
}

struct NoRetrieval;
impl jsonschema::Retrieve for NoRetrieval {
    fn retrieve(&self, _: &jsonschema::Uri<String>) -> Result<serde_json::Value, Box<dyn std::error::Error + Send + Sync>> {
        Err("external JSON Schema references are disabled; use references within this schema".into())
    }
}

impl OutputSchema {
    pub fn load(path: &Path) -> Result<Self, String> {
        let raw = std::fs::read_to_string(path).map_err(|e| format!("cannot read output schema {}: {e}", path.display()))?;
        let schema: serde_json::Value = serde_json::from_str(&raw).map_err(|e| format!("invalid output schema JSON: {e}"))?;
        let validator = jsonschema::options().with_retriever(NoRetrieval).build(&schema)
            .map_err(|e| format!("invalid output schema: {e}"))?;
        Ok(Self { validator, raw })
    }

    #[must_use]
    pub fn instruction(&self, input: &str) -> String {
        format!("{input}\n\nReturn your final answer as a single JSON value, without Markdown fences, satisfying this JSON Schema:\n{}", self.raw)
    }

    pub fn validate(&self, answer: &str) -> Result<(), String> {
        let value: serde_json::Value = serde_json::from_str(answer).map_err(|e| format!("final output is not JSON: {e}"))?;
        self.validator.validate(&value).map_err(|e| format!("final output does not match output schema at {}: {e}", e.instance_path))
    }
}

impl Invocation {
    pub fn check_remaining(&self) -> Result<(), String> {
        if let Some(failure) = &self.failure { return Err(failure.clone()); }
        if self.max_turns.is_some_and(|max| self.used_turns >= max) { return Err("invocation turn budget exhausted".into()); }
        if self.max_output_tokens.is_some_and(|max| self.used_output_tokens >= max) { return Err("invocation output-token budget exhausted".into()); }
        Ok(())
    }

    pub fn record(&mut self, summary: &runtime::TurnSummary) -> Result<(), String> {
        self.used_turns = self.used_turns.saturating_add(summary.iterations);
        self.used_output_tokens = self.used_output_tokens.saturating_add(summary.turn_output_tokens);
        let failure = if let Some(kind) = summary.budget_exhausted {
            Some(format!("runtime budget exhausted: {kind:?}"))
        } else if self.max_turns.is_some_and(|max| self.used_turns > max) {
            Some("invocation turn budget exhausted".to_string())
        } else if self.max_output_tokens.is_some_and(|max| self.used_output_tokens > max) {
            Some("invocation output-token budget exhausted".to_string())
        } else { None };
        if let Some(failure) = failure { self.failure = Some(failure.clone()); return Err(failure); }
        Ok(())
    }
}

pub fn canonical_roots(cwd: &Path, roots: &[PathBuf]) -> Result<Vec<PathBuf>, String> {
    let mut canonical = Vec::new();
    for path in roots {
        let path = if path.is_absolute() { path.clone() } else { cwd.join(path) };
        let path = path.canonicalize().map_err(|e| format!("cannot open --add-dir {}: {e}", path.display()))?;
        if !path.is_dir() { return Err(format!("--add-dir {} is not a directory", path.display())); }
        if !canonical.contains(&path) { canonical.push(path); }
    }
    Ok(canonical)
}
