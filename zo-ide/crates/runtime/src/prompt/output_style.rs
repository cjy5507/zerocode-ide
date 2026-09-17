//! Output styles — Claude Code parity for `/output-style` + `outputStyle`
//! settings key.
//!
//! A style is a named prompt fragment injected near the top of the **main
//! loop's** system prompt (`SystemPromptBuilder::with_output_style`); sub-agent
//! prompts never carry it. Built-ins ship in the binary; custom styles are
//! markdown files in `<cwd>/.zo/output-styles/` or `~/.zo/output-styles/`
//! (project wins on a name collision), with an optional YAML frontmatter
//! carrying `name:` / `description:`.

use std::fs;
use std::path::{Path, PathBuf};

/// One selectable style for pickers (`/output-style` modal).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputStyleEntry {
    /// The key the user selects (file stem for custom styles).
    pub name: String,
    pub description: String,
}

/// The sentinel that disables styling (the stock system prompt).
pub const DEFAULT_STYLE: &str = "default";

/// The builtin `/brief` toggles. Named so the toggle and the table can never
/// disagree on spelling — they already did once, and the mismatch made `/brief`
/// a one-way switch.
pub const CONCISE_STYLE: &str = "concise";

const BUILTINS: &[(&str, &str, &str)] = &[
    (
        "explanatory",
        "Educational insights about the codebase while working",
        "You are in explanatory mode. While completing the task, teach as you go: \
before or after meaningful actions, add a short `※ Insight` block (1-3 sentences) \
explaining the design decision, codebase pattern, or tradeoff involved — the kind \
of context an experienced colleague would point out. Keep the actual work and its \
quality unchanged; the insights are additive, never a substitute for doing the task.",
    ),
    (
        "learning",
        "Collaborative learn-by-doing: you contribute small pieces",
        "You are in learning mode — collaborative learn-by-doing. Besides explaining \
key decisions with short `※ Insight` blocks, occasionally pause and ask the human to \
contribute a small, strategic piece of the work themselves (2-10 lines): pick a spot \
that carries a real design decision, insert a clearly marked `TODO(human)` describing \
exactly what to write, and wait for their contribution before continuing. Never mark \
the task complete while a `TODO(human)` remains.",
    ),
    (
        CONCISE_STYLE,
        "Terse output: answers first, minimal prose",
        "You are in concise mode. Lead with the answer or outcome in the first \
sentence; cut preamble, recaps, and hedging. Prefer short sentences and skip \
sections that do not change what the reader does next. Code, identifiers, and \
file:line references stay precise — brevity must never cost correctness.",
    ),
];

/// Directories scanned for custom styles, project first (project wins), then
/// every canonical Zo global home in precedence order.
fn style_dirs(cwd: &Path) -> Vec<PathBuf> {
    style_dirs_from(cwd, core_types::paths::zo_global_config_roots())
}

fn style_dirs_from(cwd: &Path, global_roots: impl IntoIterator<Item = PathBuf>) -> Vec<PathBuf> {
    let mut dirs = vec![cwd.join(".zo").join("output-styles")];
    for root in global_roots {
        let dir = root.join("output-styles");
        if !dirs.iter().any(|existing| existing == &dir) {
            dirs.push(dir);
        }
    }
    dirs
}

/// Split an optional `---` YAML frontmatter off a style file, returning
/// `(frontmatter_lines, body)`.
fn split_frontmatter(text: &str) -> (Vec<&str>, &str) {
    let trimmed = text.trim_start_matches('\u{feff}');
    let Some(rest) = trimmed.strip_prefix("---") else {
        return (Vec::new(), trimmed);
    };
    let Some((front, body)) = rest.split_once("\n---") else {
        return (Vec::new(), trimmed);
    };
    let body = body.trim_start_matches(['-']).trim_start_matches('\n');
    (front.lines().collect(), body)
}

fn frontmatter_value<'a>(lines: &[&'a str], key: &str) -> Option<&'a str> {
    lines.iter().find_map(|line| {
        let (k, v) = line.split_once(':')?;
        (k.trim() == key).then(|| v.trim())
    })
}

/// A custom style file resolved from disk.
fn read_custom_style(path: &Path) -> Option<(String, String, String)> {
    let stem = path.file_stem()?.to_str()?.to_string();
    let text = fs::read_to_string(path).ok()?;
    let (front, body) = split_frontmatter(&text);
    let display = frontmatter_value(&front, "name").map_or_else(|| stem.clone(), str::to_string);
    let description = frontmatter_value(&front, "description")
        .unwrap_or("custom style")
        .to_string();
    let prompt = body.trim().to_string();
    if prompt.is_empty() {
        return None;
    }
    Some((display, description, prompt))
}

/// Locate a custom style file by stem (case-insensitive) in the style dirs.
fn find_custom_style(cwd: &Path, name: &str) -> Option<PathBuf> {
    for dir in style_dirs(cwd) {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            if stem.eq_ignore_ascii_case(name) {
                return Some(path);
            }
        }
    }
    None
}

/// Resolve a style name to `(display_name, prompt_text)`. `default` (or an
/// empty name) resolves to `None` — the stock prompt. Custom files shadow
/// built-ins of the same name.
#[must_use]
pub fn resolve(cwd: &Path, name: &str) -> Option<(String, String)> {
    let name = name.trim();
    if name.is_empty() || name.eq_ignore_ascii_case(DEFAULT_STYLE) {
        return None;
    }
    if let Some(path) = find_custom_style(cwd, name) {
        if let Some((display, _description, prompt)) = read_custom_style(&path) {
            return Some((display, prompt));
        }
    }
    BUILTINS
        .iter()
        .find(|(builtin, _, _)| builtin.eq_ignore_ascii_case(name))
        .map(|(builtin, _, prompt)| ((*builtin).to_string(), (*prompt).to_string()))
}

/// Every selectable style: `default` first, then built-ins, then custom files
/// (deduplicated by name, project before user).
#[must_use]
pub fn list(cwd: &Path) -> Vec<OutputStyleEntry> {
    let mut entries = vec![OutputStyleEntry {
        name: DEFAULT_STYLE.to_string(),
        description: "Stock system prompt (no style)".to_string(),
    }];
    for (name, description, _) in BUILTINS {
        entries.push(OutputStyleEntry {
            name: (*name).to_string(),
            description: (*description).to_string(),
        });
    }
    for dir in style_dirs(cwd) {
        let Ok(dir_entries) = fs::read_dir(&dir) else {
            continue;
        };
        let mut customs: Vec<_> = dir_entries
            .flatten()
            .filter_map(|entry| {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("md") {
                    return None;
                }
                let stem = path.file_stem()?.to_str()?.to_string();
                let (_, description, _) = read_custom_style(&path)?;
                Some((stem, description))
            })
            .collect();
        customs.sort();
        for (stem, description) in customs {
            if entries
                .iter()
                .any(|entry| entry.name.eq_ignore_ascii_case(&stem))
            {
                continue;
            }
            entries.push(OutputStyleEntry {
                name: stem,
                description,
            });
        }
    }
    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_cwd(tag: &str) -> PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("zo-style-{tag}-{unique}"));
        fs::create_dir_all(dir.join(".zo").join("output-styles")).expect("mkdir");
        dir
    }

    #[test]
    fn resolves_builtin_styles_case_insensitively() {
        let cwd = temp_cwd("builtin");
        let (name, prompt) = resolve(&cwd, "Explanatory").expect("builtin resolves");
        assert_eq!(name, "explanatory");
        assert!(prompt.contains("Insight"));
        assert!(resolve(&cwd, "default").is_none(), "default = no style");
        assert!(resolve(&cwd, "no-such-style").is_none());
    }

    #[test]
    fn custom_file_with_frontmatter_shadows_builtin() {
        let cwd = temp_cwd("custom");
        fs::write(
            cwd.join(".zo/output-styles/concise.md"),
            "---\nname: Team Concise\ndescription: our concise\n---\nAlways answer in one line.\n",
        )
        .expect("write style");
        let (name, prompt) = resolve(&cwd, "concise").expect("custom resolves");
        assert_eq!(name, "Team Concise");
        assert_eq!(prompt, "Always answer in one line.");
    }

    #[test]
    fn list_contains_default_builtins_and_customs() {
        let cwd = temp_cwd("list");
        fs::write(
            cwd.join(".zo/output-styles/pirate.md"),
            "Talk like a pirate while staying technically precise.\n",
        )
        .expect("write style");
        let names: Vec<String> = list(&cwd).into_iter().map(|entry| entry.name).collect();
        assert_eq!(names[0], "default");
        assert!(names.contains(&"explanatory".to_string()));
        assert!(names.contains(&"pirate".to_string()));
    }

    #[test]
    fn style_dirs_include_every_canonical_global_root_in_order() {
        let cwd = temp_cwd("roots");
        let config_home = cwd.join("config-home");
        let zo_home = cwd.join("zo-home");
        let user_zo = cwd.join("user-home").join(".zo");

        let dirs = style_dirs_from(
            &cwd,
            [config_home.clone(), zo_home.clone(), user_zo.clone()],
        );

        assert_eq!(
            dirs,
            vec![
                cwd.join(".zo").join("output-styles"),
                config_home.join("output-styles"),
                zo_home.join("output-styles"),
                user_zo.join("output-styles"),
            ]
        );
    }
}
