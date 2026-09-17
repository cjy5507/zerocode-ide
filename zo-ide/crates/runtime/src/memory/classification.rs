use decision_core::dreamer::LessonKind;

use super::model_tag::MemoryModelTag;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemorySource {
    Unknown,
    HandWritten,
    Dreamer,
}

/// How far a memory entry is intended to travel.
///
/// `Local` is the safe default for newly written observations: it lives in the
/// machine-local project overlay, which survives a session (so compaction and a
/// restart can still use it) but is not shared with another machine. `Global`
/// is opt-in durable project knowledge. Entries written before this field
/// existed intentionally read as `Global`, preserving their original store and
/// recall behaviour without a migration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryScope {
    Local,
    Global,
}

impl MemoryScope {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Global => "global",
        }
    }

    #[must_use]
    pub const fn from_local(local: bool) -> Self {
        if local { Self::Local } else { Self::Global }
    }
}

impl MemorySource {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::HandWritten => "hand_written",
            Self::Dreamer => "dreamer",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryKind {
    Unknown,
    Preference,
    Gotcha,
    Workflow,
    Constraint,
    TaskLog,
}

impl MemoryKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Preference => "preference",
            Self::Gotcha => "gotcha",
            Self::Workflow => "workflow",
            Self::Constraint => "constraint",
            Self::TaskLog => "task_log",
        }
    }

    #[must_use]
    pub const fn from_lesson(kind: LessonKind) -> Self {
        match kind {
            LessonKind::Preference => Self::Preference,
            LessonKind::Gotcha => Self::Gotcha,
            LessonKind::Workflow => Self::Workflow,
            LessonKind::Constraint => Self::Constraint,
        }
    }
}

/// Everything the `- memory_metadata:` trailer says about one entry.
///
/// This is both the parse result and the write request: [`Self::metadata_line`]
/// is the only producer of that line and [`classify_memory_body`] the only
/// reader, so the two can never drift apart into two spellings of one format.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryClassification {
    pub source: MemorySource,
    pub kind: MemoryKind,
    pub protected: bool,
    pub resolved_task_log: bool,
    pub written_at: Option<u64>,
    /// The model that authored the body currently on disk, when it is known.
    ///
    /// `None` on every entry written before per-model memory existed, and on
    /// any entry whose writer could not name its model. Those stay fully
    /// recallable — recall only ever *adds* a preference for the running
    /// model's own entries (see `recall::ranking_boost`), so an untagged entry
    /// keeps exactly the rank it had before the field existed. That is what
    /// lets the field ship with no migration.
    pub model: Option<MemoryModelTag>,
    /// Scope is an optional metadata suffix, exactly like `model=`. Missing
    /// suffixes are legacy global entries, not malformed entries.
    pub scope: MemoryScope,
}

impl Default for MemoryClassification {
    fn default() -> Self {
        Self {
            source: MemorySource::Unknown,
            kind: MemoryKind::Unknown,
            protected: true,
            resolved_task_log: false,
            written_at: None,
            model: None,
            scope: MemoryScope::Global,
        }
    }
}

impl MemoryClassification {
    /// A `MemoryWrite` entry: the model wrote this itself, so it is protected
    /// from the decay pass and stamped with the model that wrote it.
    #[must_use]
    pub fn hand_written(
        kind: MemoryKind,
        written_at: Option<u64>,
        model: Option<MemoryModelTag>,
    ) -> Self {
        Self {
            source: MemorySource::HandWritten,
            kind,
            protected: true,
            resolved_task_log: false,
            written_at,
            model,
            scope: MemoryScope::Global,
        }
    }

    /// A Dreamer promotion: decayable, and stamped with the model whose pass
    /// promoted it (the lessons themselves are mined from turn logs that carry
    /// no model of their own).
    #[must_use]
    pub fn dreamer(
        kind: MemoryKind,
        resolved_task_log: bool,
        written_at: Option<u64>,
        model: Option<MemoryModelTag>,
    ) -> Self {
        Self {
            source: MemorySource::Dreamer,
            kind,
            protected: false,
            resolved_task_log,
            written_at,
            model,
            // Constructors remain backward-compatible for callers that render
            // a legacy durable Dreamer entry. Automatic curation explicitly
            // selects Local at its write boundary below.
            scope: MemoryScope::Global,
        }
    }

    /// Change only the destination scope while retaining provenance and all
    /// existing metadata fields. The tool layer supplies author and write time;
    /// the filesystem layer is the single authority for which store receives it.
    #[must_use]
    pub const fn with_scope(mut self, scope: MemoryScope) -> Self {
        self.scope = scope;
        self
    }

    /// Render this classification as the entry-body trailer line.
    ///
    /// `v=1` is deliberately unchanged by the `model=` field: the parser skips
    /// keys it does not know and requires only the pre-existing ones, so a new
    /// line reads correctly in an old build and an old line reads correctly
    /// here. Bumping the version would have made every existing entry
    /// unclassified — the one outcome per-model memory must not cause.
    #[must_use]
    pub fn metadata_line(&self) -> String {
        let written_at = self
            .written_at
            .map_or_else(|| "unknown".to_string(), |secs| secs.to_string());
        let model = self
            .model
            .as_ref()
            .map_or_else(String::new, |model| format!(";model={model}"));
        let scope = format!(";scope={}", self.scope.as_str());
        format!(
            "- memory_metadata: v=1;source={};kind={};protected={};resolved_task_log={};written_at={written_at}{model}{scope}",
            self.source.as_str(),
            self.kind.as_str(),
            self.protected,
            self.resolved_task_log,
        )
    }
}

/// Whether two entry bodies differ only in their metadata `written_at` stamp.
///
/// The `MemoryWrite` tool stamps the write time into every body it persists,
/// so a model re-recording the same lesson produces a byte-different file
/// whose every *meaningful* byte is identical. Comparing with the stamp masked
/// lets the store treat that re-save as the no-op it is — which also keeps the
/// surviving `written_at` meaning "first authored", not "most recently
/// re-said". Everything else on the metadata line (source, kind, protected)
/// still has to match: changing those is a real write — including `model=`,
/// so the tag on disk always names the model whose bytes are actually there.
/// A different model re-recording the same lesson is one `Updated` write that
/// re-points the tag at it, not perpetual churn: the next re-save under that
/// same model is byte-identical again and stays `Unchanged`.
#[must_use]
pub fn bodies_equal_ignoring_written_at(a: &str, b: &str) -> bool {
    a == b || mask_written_at(a) == mask_written_at(b)
}

/// `body` with the `written_at=` value on its metadata line replaced by `*`.
fn mask_written_at(body: &str) -> String {
    body.lines()
        .map(|line| {
            if !line.trim_start().starts_with("- memory_metadata:") {
                return line.to_string();
            }
            match line.find("written_at=") {
                None => line.to_string(),
                Some(at) => {
                    let value_start = at + "written_at=".len();
                    let value_end = line[value_start..]
                        .find(';')
                        .map_or(line.len(), |semi| value_start + semi);
                    format!("{}*{}", &line[..value_start], &line[value_end..])
                }
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[must_use]
pub fn memory_body_has_classification_metadata(body: &str) -> bool {
    body.lines()
        .any(|line| line.trim().starts_with("- memory_metadata:"))
}

#[must_use]
pub fn classify_memory_body(body: &str) -> MemoryClassification {
    let Some(line) = body
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with("- memory_metadata:"))
    else {
        return MemoryClassification::default();
    };
    parse_memory_metadata_line(line).unwrap_or_default()
}

/// Replace a valid metadata trailer with the same trailer plus this entry's
/// scope. This is deliberately a write-path helper, not a migration: untouched
/// legacy files keep their bytes and their implicit [`MemoryScope::Global`].
#[must_use]
pub fn with_memory_scope_metadata(body: &str, scope: MemoryScope) -> String {
    let mut changed = false;
    let mut rendered = body
        .lines()
        .map(|line| {
            let trimmed = line.trim();
            let Some(classification) = parse_memory_metadata_line(trimmed) else {
                return line.to_string();
            };
            let indent = &line[..line.len() - line.trim_start().len()];
            changed = true;
            format!("{indent}{}", classification.with_scope(scope).metadata_line())
        })
        .collect::<Vec<_>>()
        .join("\n");
    if changed && body.ends_with('\n') {
        rendered.push('\n');
    }
    if changed { rendered } else { body.to_string() }
}

fn parse_memory_metadata_line(line: &str) -> Option<MemoryClassification> {
    let metadata = line.strip_prefix("- memory_metadata:")?.trim();
    let mut version_ok = false;
    let mut source = None;
    let mut kind = None;
    let mut protected = None;
    let mut resolved_task_log = None;
    let mut resolved_task_log_seen = false;
    let mut written_at = None;
    let mut written_at_seen = false;
    let mut model = None;
    let mut scope = MemoryScope::Global;

    for part in metadata.split(';') {
        let (key, value) = part.trim().split_once('=')?;
        match key.trim() {
            "v" => version_ok = value.trim() == "1",
            "source" => source = parse_source(value.trim()),
            "kind" => kind = parse_kind(value.trim()),
            "protected" => protected = parse_bool(value.trim()),
            "resolved_task_log" => {
                resolved_task_log = parse_bool(value.trim());
                resolved_task_log_seen = true;
            }
            "written_at" => {
                written_at_seen = true;
                let value = value.trim();
                written_at = if value == "unknown" {
                    None
                } else {
                    Some(value.parse::<u64>().ok()?)
                };
            }
            // Optional by design: entries written before per-model memory have
            // no `model=` at all, and an unusable id leaves the entry untagged
            // rather than dropping its whole classification.
            "model" => model = MemoryModelTag::new(value),
            // Optional by design. Pre-scope entries were written to the durable
            // project store, so `global` exactly preserves their old meaning.
            "scope" => scope = parse_scope(value).unwrap_or(MemoryScope::Global),
            _ => {}
        }
    }

    (version_ok && resolved_task_log_seen && written_at_seen).then_some(MemoryClassification {
        source: source?,
        kind: kind?,
        protected: protected?,
        resolved_task_log: resolved_task_log?,
        written_at,
        model,
        scope,
    })
}

fn parse_source(value: &str) -> Option<MemorySource> {
    match value {
        "unknown" => Some(MemorySource::Unknown),
        "hand_written" => Some(MemorySource::HandWritten),
        "dreamer" => Some(MemorySource::Dreamer),
        _ => None,
    }
}

fn parse_kind(value: &str) -> Option<MemoryKind> {
    match value {
        "unknown" => Some(MemoryKind::Unknown),
        "preference" => Some(MemoryKind::Preference),
        "gotcha" => Some(MemoryKind::Gotcha),
        "workflow" => Some(MemoryKind::Workflow),
        "constraint" => Some(MemoryKind::Constraint),
        "task_log" => Some(MemoryKind::TaskLog),
        _ => None,
    }
}

fn parse_scope(value: &str) -> Option<MemoryScope> {
    match value {
        "local" => Some(MemoryScope::Local),
        "global" => Some(MemoryScope::Global),
        _ => None,
    }
}

fn parse_bool(value: &str) -> Option<bool> {
    match value {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tag(id: &str) -> MemoryModelTag {
        MemoryModelTag::new(id).expect("test model id should be taggable")
    }

    #[test]
    fn missing_or_malformed_metadata_is_protected_unknown() {
        assert_eq!(classify_memory_body("plain note"), MemoryClassification::default());
        assert_eq!(
            classify_memory_body(
                "---\n- memory_metadata: v=1;source=dreamer;kind=gotcha;protected=false;resolved_task_log=false;written_at=not-a-number"
            ),
            MemoryClassification::default()
        );
    }

    #[test]
    fn parses_dreamer_metadata_line() {
        let body = format!(
            "body\n\n---\n{}\n",
            MemoryClassification::dreamer(MemoryKind::Gotcha, true, Some(42), None).metadata_line()
        );
        let classification = classify_memory_body(&body);
        assert_eq!(classification.source, MemorySource::Dreamer);
        assert_eq!(classification.kind, MemoryKind::Gotcha);
        assert!(!classification.protected);
        assert!(classification.resolved_task_log);
        assert_eq!(classification.written_at, Some(42));
        assert_eq!(classification.model, None);
    }

    #[test]
    fn metadata_line_round_trips_the_authoring_model() {
        let written = MemoryClassification::hand_written(
            MemoryKind::Preference,
            Some(7),
            Some(tag("claude-opus-5")),
        );
        let line = written.metadata_line();
        assert!(
            line.ends_with(";model=claude-opus-5;scope=global"),
            "optional metadata remains append-only: {line}"
        );
        assert_eq!(classify_memory_body(&format!("body\n\n---\n{line}\n")), written);
    }

    #[test]
    fn scope_is_an_optional_append_only_suffix_and_legacy_entries_stay_global() {
        let legacy = "- memory_metadata: v=1;source=hand_written;kind=unknown;protected=true;resolved_task_log=false;written_at=7;model=claude-opus-5";
        assert_eq!(
            classify_memory_body(&format!("body\n\n---\n{legacy}\n")).scope,
            MemoryScope::Global,
            "a missing scope preserves the old durable-store meaning"
        );

        let local = MemoryClassification::hand_written(MemoryKind::Gotcha, Some(7), None)
            .with_scope(MemoryScope::Local);
        let rewritten = with_memory_scope_metadata(
            &format!("body\n\n---\n{}\n", local.metadata_line()),
            MemoryScope::Global,
        );
        assert!(rewritten.ends_with(";scope=global\n"), "{rewritten}");
        assert_eq!(classify_memory_body(&rewritten).scope, MemoryScope::Global);
    }

    #[test]
    fn a_line_written_before_per_model_memory_still_classifies_in_full() {
        // The exact bytes on disk today, from entries written before `model=`
        // existed. They must keep every field they had and simply read as
        // untagged — this is the no-migration guarantee.
        let legacy = "- memory_metadata: v=1;source=hand_written;kind=unknown;protected=true;resolved_task_log=false;written_at=1784489881";
        assert_eq!(
            classify_memory_body(&format!("body\n\n---\n{legacy}\n")),
            MemoryClassification::hand_written(MemoryKind::Unknown, Some(1_784_489_881), None)
        );
    }

    #[test]
    fn an_unusable_model_id_costs_the_tag_and_nothing_else() {
        // A forged/broken `model=` value must not take the rest of the
        // classification down with it, and an unknown key stays ignored so a
        // future field is readable by this build.
        let line = "- memory_metadata: v=1;source=dreamer;kind=gotcha;protected=false;resolved_task_log=false;written_at=9;model=   ;future_field=whatever";
        assert_eq!(
            classify_memory_body(&format!("body\n\n---\n{line}\n")),
            MemoryClassification::dreamer(MemoryKind::Gotcha, false, Some(9), None)
        );
    }

    #[test]
    fn re_recording_under_another_model_is_a_real_write() {
        let opus = MemoryClassification::hand_written(MemoryKind::Unknown, Some(1), Some(tag("claude-opus-5")));
        let opus_later =
            MemoryClassification::hand_written(MemoryKind::Unknown, Some(2), Some(tag("claude-opus-5")));
        let sol = MemoryClassification::hand_written(MemoryKind::Unknown, Some(2), Some(tag("gpt-5.6-sol")));
        let body = |classification: &MemoryClassification| {
            format!("same lesson\n\n---\n{}\n", classification.metadata_line())
        };

        assert!(
            bodies_equal_ignoring_written_at(&body(&opus), &body(&opus_later)),
            "the same model re-saying the same lesson stays a no-op"
        );
        assert!(
            !bodies_equal_ignoring_written_at(&body(&opus), &body(&sol)),
            "another model re-recording it re-points the tag at the bytes on disk"
        );
    }
}
