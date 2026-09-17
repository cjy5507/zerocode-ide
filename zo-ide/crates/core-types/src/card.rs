//! Provider-neutral, pure-data model for a rich command-output **card**.
//!
//! Slash commands (`/status`, `/cost`, `/context`, `/mcp`, …) build a
//! [`CardModel`] of structured elements instead of a pre-formatted string.
//! The TUI (`tui/cards/`) renders it into styled `ratatui` lines via the
//! theme; headless sinks fall back to [`CardModel::plain_text`]. Keeping
//! the model here (no `ratatui` dependency) gives the same source one
//! visual in the TUI and a clean JSON projection for `--output-format`.
//!
//! Tone is *semantic*, never a concrete color — the renderer maps each
//! [`CardTone`] through `Theme` so `NO_COLOR` and every palette degrade
//! consistently (code-rules R9/R10).

use serde::{Deserialize, Serialize};

/// Semantic tone for a card element, mapped to a theme color by the
/// renderer (never a raw color here).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CardTone {
    /// Body foreground.
    #[default]
    Default,
    /// Brand accent.
    Accent,
    /// Healthy / success.
    Ok,
    /// Caution.
    Warn,
    /// Critical / error.
    Crit,
    /// De-emphasised.
    Muted,
}

/// One element inside a [`CardModel`], rendered top-to-bottom.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CardElement {
    /// A section divider/header within the card.
    Section {
        /// Section label.
        label: String,
    },
    /// `label …… value` row with the value right-aligned and toned.
    Metric {
        /// Left label.
        label: String,
        /// Right-aligned value.
        value: String,
        /// Semantic tone for the value.
        tone: CardTone,
    },
    /// A labeled progress gauge. `ratio` is clamped to `0.0..=1.0` by the
    /// renderer; `caption` is the human-readable read-out (e.g.
    /// `"120k / 200k · 60%"`).
    Gauge {
        /// Left label.
        label: String,
        /// Fill ratio (clamped at render time).
        ratio: f64,
        /// Right-aligned caption.
        caption: String,
    },
    /// A table with a header row and aligned body rows.
    Table {
        /// Header cells.
        header: Vec<String>,
        /// Body rows (each a row of cells).
        rows: Vec<Vec<String>>,
    },
    /// A status badge: `✓`/`✗` + text.
    Badge {
        /// Whether the badge reads as healthy.
        ok: bool,
        /// Badge text.
        text: String,
    },
    /// `key: value` line (muted key, body value).
    KeyValue {
        /// Muted key.
        key: String,
        /// Body value.
        value: String,
    },
    /// A free text line with an optional tone.
    Text {
        /// Line text.
        text: String,
        /// Semantic tone.
        tone: CardTone,
    },
    /// A vertical spacer (one blank line).
    Spacer,
}

/// A titled, bordered panel of [`CardElement`]s — the structured form of
/// a slash-command report.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CardModel {
    /// Card title (rendered into the top border, e.g. `" /status "`).
    pub title: String,
    /// Optional one-line subtitle under the title.
    pub subtitle: Option<String>,
    /// Body elements in display order.
    pub elements: Vec<CardElement>,
}

impl CardModel {
    /// Start a new card with the given title.
    #[must_use]
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            subtitle: None,
            elements: Vec::new(),
        }
    }

    /// Set the subtitle (builder).
    #[must_use]
    pub fn subtitle(mut self, subtitle: impl Into<String>) -> Self {
        self.subtitle = Some(subtitle.into());
        self
    }

    /// Append a section header (builder).
    #[must_use]
    pub fn section(mut self, label: impl Into<String>) -> Self {
        self.elements.push(CardElement::Section {
            label: label.into(),
        });
        self
    }

    /// Append a toned metric row (builder).
    #[must_use]
    pub fn metric(
        mut self,
        label: impl Into<String>,
        value: impl Into<String>,
        tone: CardTone,
    ) -> Self {
        self.elements.push(CardElement::Metric {
            label: label.into(),
            value: value.into(),
            tone,
        });
        self
    }

    /// Append a gauge row (builder).
    #[must_use]
    pub fn gauge(
        mut self,
        label: impl Into<String>,
        ratio: f64,
        caption: impl Into<String>,
    ) -> Self {
        self.elements.push(CardElement::Gauge {
            label: label.into(),
            ratio,
            caption: caption.into(),
        });
        self
    }

    /// Append a table (builder).
    #[must_use]
    pub fn table(mut self, header: Vec<String>, rows: Vec<Vec<String>>) -> Self {
        self.elements.push(CardElement::Table { header, rows });
        self
    }

    /// Append a status badge (builder).
    #[must_use]
    pub fn badge(mut self, ok: bool, text: impl Into<String>) -> Self {
        self.elements.push(CardElement::Badge {
            ok,
            text: text.into(),
        });
        self
    }

    /// Append a `key: value` line (builder).
    #[must_use]
    pub fn key_value(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.elements.push(CardElement::KeyValue {
            key: key.into(),
            value: value.into(),
        });
        self
    }

    /// Append a toned text line (builder).
    #[must_use]
    pub fn text(mut self, text: impl Into<String>, tone: CardTone) -> Self {
        self.elements.push(CardElement::Text {
            text: text.into(),
            tone,
        });
        self
    }

    /// Append a blank spacer (builder).
    #[must_use]
    pub fn spacer(mut self) -> Self {
        self.elements.push(CardElement::Spacer);
        self
    }

    /// Parse a conventional report string into a card.
    ///
    /// zo's text reports follow a stable shape: a flush-left line is a
    /// **section header**, an indented `  Label    Value` line is a
    /// **key/value** (split on the first run of two spaces), a blank line
    /// is a spacer, and anything else is plain text. This upgrades every
    /// string-report slash command to a bordered card with one call,
    /// without restructuring each report's internals.
    #[must_use]
    pub fn from_report_text(title: impl Into<String>, text: &str) -> Self {
        let mut card = Self::new(title);
        for line in text.lines() {
            if line.trim().is_empty() {
                card.elements.push(CardElement::Spacer);
            } else if line.starts_with(char::is_whitespace) {
                let trimmed = line.trim_start();
                if let Some((key, value)) = split_label_value(trimmed) {
                    card.elements.push(CardElement::KeyValue { key, value });
                } else {
                    card.elements.push(CardElement::Text {
                        text: trimmed.to_string(),
                        tone: CardTone::Default,
                    });
                }
            } else {
                card.elements.push(CardElement::Section {
                    label: line.trim_end().to_string(),
                });
            }
        }
        // Drop a trailing spacer so the card border hugs the content.
        if matches!(card.elements.last(), Some(CardElement::Spacer)) {
            card.elements.pop();
        }
        card
    }

    /// Flatten the card into a plain-text block for non-TUI sinks and the
    /// headless `--output-format` paths. No ANSI, no borders — just the
    /// content so logs and pipes stay readable.
    #[must_use]
    pub fn plain_text(&self) -> String {
        use core::fmt::Write as _;
        let mut out = String::new();
        // Writing to a `String` is infallible, so the `Result` is ignored.
        let _ = writeln!(out, "{}", self.title.trim());
        if let Some(sub) = &self.subtitle {
            let _ = writeln!(out, "{sub}");
        }
        for el in &self.elements {
            match el {
                CardElement::Section { label } => {
                    let _ = write!(out, "\n{label}\n");
                }
                CardElement::Metric { label, value, .. } => {
                    let _ = writeln!(out, "  {label}: {value}");
                }
                CardElement::Gauge { label, caption, .. } => {
                    let _ = writeln!(out, "  {label}: {caption}");
                }
                CardElement::Table { header, rows } => {
                    let _ = writeln!(out, "  {}", header.join(" | "));
                    for row in rows {
                        let _ = writeln!(out, "  {}", row.join(" | "));
                    }
                }
                CardElement::Badge { ok, text } => {
                    let mark = if *ok { "ok" } else { "x" };
                    let _ = writeln!(out, "  [{mark}] {text}");
                }
                CardElement::KeyValue { key, value } => {
                    let _ = writeln!(out, "  {key}: {value}");
                }
                CardElement::Text { text, .. } => {
                    let _ = writeln!(out, "  {text}");
                }
                CardElement::Spacer => out.push('\n'),
            }
        }
        out.trim_end().to_string()
    }
}

/// Split `  Label    Value` on the first run of two-or-more spaces. Byte
/// indices from `find("  ")` are always valid UTF-8 boundaries, so this is
/// safe for multibyte (e.g. Korean) labels.
fn split_label_value(s: &str) -> Option<(String, String)> {
    let idx = s.find("  ")?;
    let label = s[..idx].trim_end();
    let value = s[idx..].trim_start();
    if label.is_empty() || value.is_empty() {
        return None;
    }
    Some((label.to_string(), value.to_string()))
}

#[cfg(test)]
mod tests {
    use super::{CardElement, CardModel, CardTone};

    #[test]
    fn builder_assembles_elements_in_order() {
        let card = CardModel::new(" /status ")
            .section("Session")
            .key_value("model", "Opus 4.8")
            .gauge("context", 0.6, "120k / 200k · 60%")
            .metric("cost", "$1.23", CardTone::Warn)
            .badge(true, "API key set");
        assert_eq!(card.elements.len(), 5);
        assert_eq!(card.title, " /status ");
    }

    #[test]
    fn plain_text_is_ansi_free_and_readable() {
        let card = CardModel::new(" /cost ")
            .metric("total", "$0.42", CardTone::Default)
            .badge(false, "rate limited");
        let txt = card.plain_text();
        assert!(txt.contains("/cost"));
        assert!(txt.contains("total: $0.42"));
        assert!(txt.contains("[x] rate limited"));
        assert!(!txt.contains('\u{1b}'), "no ANSI escapes in plain text");
    }

    #[test]
    fn from_report_text_parses_sections_and_key_values() {
        let report = "Status\n  Model            opus\n  Permission mode  workspace-write\n\nUsage\n  Cumulative total 1234\n  플랫폼            한국어 값";
        let card = CardModel::from_report_text(" /status ", report);
        // Two section headers.
        let sections = card
            .elements
            .iter()
            .filter(|e| matches!(e, CardElement::Section { .. }))
            .count();
        assert_eq!(sections, 2);
        // Key/value split survives multibyte labels/values.
        let has_kv = card.elements.iter().any(|e| {
            matches!(e, CardElement::KeyValue { key, value } if key == "플랫폼" && value == "한국어 값")
        });
        assert!(has_kv, "multibyte key/value must parse: {card:?}");
        // No trailing spacer.
        assert!(!matches!(card.elements.last(), Some(CardElement::Spacer)));
    }

    #[test]
    fn roundtrips_through_serde_json() {
        let card = CardModel::new("t").gauge("g", 0.5, "half");
        let json = serde_json::to_string(&card).expect("serialize");
        let back: CardModel = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(card, back);
    }
}
