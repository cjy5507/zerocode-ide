//! The JSON Schema words a tool's input schema may use and still reach every
//! provider zo talks to unchanged (t-96186).
//!
//! zo shows every model one tool list. Each provider carries a tool's schema
//! its own way — Anthropic's `input_schema` and the two OpenAI shapes'
//! `parameters` as written, Gemini Code Assist's through
//! `gemini_parameter_schema`'s rewrite into its protobuf `Schema`, which takes
//! these words as they are (ai.google.dev structured-output and
//! function-calling docs). Each provider's tests carry
//! [`every_portable_keyword`] through its serializer and find it unchanged; the
//! tools crate holds zo's wire schemas to these words.

/// Schema keywords every provider's serializer passes through as written.
pub const PORTABLE_SCHEMA_KEYWORDS: &[&str] = &[
    "type",
    "description",
    "properties",
    "required",
    "additionalProperties",
    "items",
    "minItems",
    "maxItems",
    "enum",
    "minimum",
    "maximum",
];

/// A tool schema that uses every portable keyword, for the providers' tests.
#[cfg(test)]
pub(crate) fn every_portable_keyword() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "description": "Every portable keyword once.",
        "properties": {
            "file_path": { "type": "string", "description": "A file." },
            "limit": { "type": "integer", "minimum": 1, "maximum": 2000 },
            "mode": { "type": "string", "enum": ["content", "count"] },
            "edits": {
                "type": "array",
                "minItems": 1,
                "maxItems": 4,
                "items": {
                    "type": "object",
                    "properties": { "old_string": { "type": "string" } },
                    "required": ["old_string"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["file_path"],
        "additionalProperties": false
    })
}

/// The schema keywords `schema` uses, property names aside.
#[must_use]
pub fn schema_keywords(schema: &serde_json::Value) -> std::collections::BTreeSet<String> {
    let mut keywords = std::collections::BTreeSet::new();
    collect_keywords(schema, &mut keywords);
    keywords
}

fn collect_keywords(schema: &serde_json::Value, keywords: &mut std::collections::BTreeSet<String>) {
    match schema {
        serde_json::Value::Object(fields) => {
            for (key, value) in fields {
                keywords.insert(key.clone());
                match (key.as_str(), value) {
                    // A property map's keys are argument names, not keywords.
                    ("properties", serde_json::Value::Object(properties)) => {
                        for property in properties.values() {
                            collect_keywords(property, keywords);
                        }
                    }
                    // Literal values, not schemas.
                    ("enum" | "required", _) => {}
                    _ => collect_keywords(value, keywords),
                }
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                collect_keywords(item, keywords);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::{every_portable_keyword, schema_keywords, PORTABLE_SCHEMA_KEYWORDS};
    use crate::types::ToolDefinition;

    /// The providers' fixture is the whole list: a keyword added to the list
    /// is proved through every serializer before a wire schema may use it.
    #[test]
    fn the_fixture_uses_every_portable_keyword_and_nothing_else() {
        let listed: std::collections::BTreeSet<String> = PORTABLE_SCHEMA_KEYWORDS
            .iter()
            .map(|keyword| (*keyword).to_string())
            .collect();
        assert_eq!(schema_keywords(&every_portable_keyword()), listed);
    }

    /// Anthropic carries `input_schema` as serialized from the definition.
    #[test]
    fn anthropic_carries_every_portable_keyword_as_written() {
        let tool = ToolDefinition {
            name: "Read".to_string(),
            description: Some("Read a file.".to_string()),
            input_schema: every_portable_keyword(),
        };
        let wire = serde_json::to_value(&tool).expect("a definition serializes");
        assert_eq!(wire["name"], "Read");
        assert_eq!(wire["input_schema"], every_portable_keyword());
    }
}
