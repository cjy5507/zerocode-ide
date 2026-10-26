//! Input policy shared by the recorder and recipe saver. It carries facts,
//! never values. Credential shapes belong to `credential`, not this module.

use super::*;
use crate::credential;

pub const INPUT_KEY: &str = "recipeInput";
pub const SECRET_PREFIX: &str = "$secret_";
/// A saved upload names its files by parameter, never by where they sat.
pub const FILE_PREFIX: &str = "$file_";

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct InputPolicy {
    pub secret: bool,
    pub credential: bool,
}

/// Remove control stamps before deciding which word is the input payload.
pub fn normalized(tool: &str, argv: &[String]) -> Result<Vec<String>, String> {
    if tool == "browser" {
        agent_browser::parse_workspace_command(argv, None).map(|word| word.argv)
    } else {
        Ok(argv.to_vec())
    }
}

/// Payload positions, not selectors or the text a check searches for.
pub fn value_positions(tool: &str, argv: &[String]) -> Vec<usize> {
    let needle = tool == "computer"
        && argv
            .first()
            .and_then(|word| verb_method(word))
            .is_some_and(|method| {
                matches!(
                    method,
                    ComputerMethod::Click | ComputerMethod::Find | ComputerMethod::WaitFor
                )
            });
    let browser_text = tool == "browser"
        && argv
            .first()
            .is_some_and(|word| matches!(word.as_str(), "type" | "fill"));
    (0..argv.len())
        .filter(|&index| {
            browser_text && index + 1 == argv.len()
                || !needle && index > 0 && matches!(argv[index - 1].as_str(), "--text" | "--value")
        })
        .collect()
}

pub fn is_secret_parameter(word: &str) -> bool {
    word.strip_prefix('$').is_some_and(|name| {
        name.starts_with("secret_") && name.len() > "secret_".len() && recipe_param_name(name)
    })
}

/// `$file_<name>`: the file an upload picks, given with `--params` or by the
/// person when a walk reaches the step.
pub fn is_file_parameter(word: &str) -> bool {
    word.strip_prefix('$').is_some_and(|name| {
        name.starts_with("file_") && name.len() > "file_".len() && recipe_param_name(name)
    })
}

/// A `$`-word the person supplies, never a value the recipe keeps: a secret
/// or a file. A walk stops for the person when one has no value.
pub fn is_person_parameter(word: &str) -> bool {
    is_secret_parameter(word) || is_file_parameter(word)
}

/// The words of an upload that name local files: after the selector and an
/// optional `--drop`. Where a file sat is not kept, so these never reach a
/// log or a saved procedure as given.
pub fn file_positions(tool: &str, argv: &[String]) -> Vec<usize> {
    if tool != "browser" || argv.first().is_none_or(|verb| verb != "upload") {
        return Vec::new();
    }
    let first = if argv.get(3).is_some_and(|word| word == "--drop") {
        4
    } else {
        3
    };
    (first..argv.len()).collect()
}

/// A file as a log keeps it: its name, never the folders above it (a home
/// folder carries a person's name).
pub fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\'])
        .find(|part| !part.is_empty())
        .unwrap_or(path)
}

pub fn is_parameter_word(word: &str) -> bool {
    if is_person_parameter(word) {
        return true;
    }
    let names = recipe_placeholders(word);
    names.len() == 1 && word == format!("{{{{{}}}}}", names[0])
}

pub fn is_reference_value(value: &str) -> bool {
    if is_person_parameter(value) || credential::is_reference_value(value) {
        return true;
    }
    value
        .split_once('=')
        .or_else(|| value.split_once(':'))
        .is_some_and(|(key, value)| {
            recipe_param_name(key)
                && !credential::looks_like_a_credential(key)
                && is_person_parameter(value)
        })
}

fn unsafe_text(text: &str) -> bool {
    !is_reference_value(text) && credential::may_carry_a_credential(text)
}

pub fn input_policy(tool: &str, argv: &[String], masked: &[String]) -> InputPolicy {
    let Ok(argv) = normalized(tool, argv) else {
        return InputPolicy {
            secret: true,
            credential: true,
        };
    };
    let mut out = InputPolicy::default();
    let positions = value_positions(tool, &argv);
    if tool == "browser" && argv.first().is_some_and(|verb| verb == "fill") {
        if let Some(value) = positions.first().and_then(|&index| argv.get(index)) {
            if let Ok(entries) = crate::browser_form::fill_entries(value) {
                for entry in entries {
                    let secret = masked.contains(&entry.handle)
                        || credential::names_a_credential(&entry.handle);
                    out.secret |= secret;
                    let unsafe_value = match &entry.value {
                        crate::browser_form::FormValue::Text(text) => unsafe_text(text),
                        crate::browser_form::FormValue::List(items) => {
                            items.iter().any(|text| unsafe_text(text))
                        }
                        crate::browser_form::FormValue::Flag(_) => false,
                    };
                    out.credential |= !secret && unsafe_value;
                }
            } else {
                out.credential = unsafe_text(value);
            }
        }
    } else {
        let target = argv.get(2);
        let known_secret = target.is_some_and(|handle| {
            masked.contains(handle)
                || (credential::names_a_credential(handle) && !is_reference_value(handle))
        });
        out.secret = known_secret
            || positions
                .iter()
                .any(|&index| index > 0 && argv[index - 1] == "--value");
        out.credential = !known_secret && positions.iter().any(|&index| unsafe_text(&argv[index]));
    }
    out
}

/// Values that will remain in a procedure. Locations are parsed as locations,
/// so a long checkout path is not mistaken for an opaque token. Query values,
/// userinfo and credential-shaped path segments are still checked centrally.
pub fn unsafe_location_or_value(value: &str) -> bool {
    if is_reference_value(value) {
        return false;
    }
    if let Ok(url) = url::Url::parse(value)
        && (url.has_host() || url.scheme() == "file")
    {
        if !url.username().is_empty() || url.password().is_some() {
            return true;
        }
        if url.query_pairs().any(|(key, value)| {
            !is_reference_value(&value)
                && (credential::names_a_credential(&key)
                    || credential::may_carry_a_credential(&value))
        }) {
            return true;
        }
        return url
            .path_segments()
            .into_iter()
            .flatten()
            .any(credential::location_segment_may_carry_a_credential)
            || url
                .fragment()
                .is_some_and(credential::location_segment_may_carry_a_credential);
    }
    if std::path::Path::new(value).is_absolute() {
        return value
            .split(['/', '\\'])
            .any(credential::location_segment_may_carry_a_credential);
    }
    credential::may_carry_a_credential(value)
}

pub fn unsafe_stored_argument(tool: &str, argv: &[String], index: usize) -> bool {
    let value = &argv[index];
    let selector = tool == "browser"
        && index == 2
        && argv.first().is_some_and(|verb| {
            matches!(
                verb.as_str(),
                "type" | "click" | "wait" | "read" | "scroll" | "upload"
            )
        });
    if selector {
        credential::looks_like_a_credential(value)
    } else {
        unsafe_location_or_value(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whole_references_pass_while_credential_values_are_refused() {
        for value in [
            "$secret_password",
            "{{password}}",
            "password=$secret_password",
            "password={{password}}",
            "password=${TOKEN}",
            "Bearer ${TOKEN}",
            "https://example.test/?password=$secret_password",
        ] {
            assert!(
                !unsafe_location_or_value(value),
                "a reference is not a stored value"
            );
        }
        let unsafe_values = [
            "password=SYNTHETIC_INPUT_ONLY",
            "password=$(printf hunter2)",
            "password=$(gh auth token)",
            "password=$(printf '%s' \"$(gh auth token)\")",
            "https://example.test/?password=SYNTHETIC_INPUT_ONLY",
        ];
        let failures: Vec<_> = unsafe_values
            .into_iter()
            .filter(|value| !unsafe_location_or_value(value))
            .collect();
        assert!(failures.is_empty(), "unsafe values accepted: {failures:?}");
    }

    #[test]
    fn an_upload_names_its_files_after_the_selector_and_a_log_keeps_only_their_names() {
        let argv = |list: &[&str]| list.iter().map(|w| (*w).to_string()).collect::<Vec<_>>();
        assert_eq!(
            file_positions(
                "browser",
                &argv(&[
                    "upload",
                    "browser-1",
                    "#file",
                    "/Users/dev/a.pdf",
                    "/Users/dev/b.png"
                ])
            ),
            [3, 4]
        );
        assert_eq!(
            file_positions(
                "browser",
                &argv(&["upload", "browser-1", "#drop", "--drop", "/Users/dev/a.pdf"])
            ),
            [4]
        );
        assert!(file_positions("browser", &argv(&["click", "browser-1", "#file"])).is_empty());
        assert!(file_positions("computer", &argv(&["upload", "x", "y", "/a"])).is_empty());
        assert_eq!(
            file_name("/Users/dev/Documents/receipt 1.pdf"),
            "receipt 1.pdf"
        );
        assert_eq!(file_name("C:\\Users\\dev\\receipt.pdf"), "receipt.pdf");
        assert_eq!(file_name("receipt.pdf"), "receipt.pdf");
        for word in ["$file_3", "$file_receipt"] {
            assert!(is_file_parameter(word) && is_parameter_word(word), "{word}");
        }
        for word in ["$file_", "$file", "file_3", "$files_3", "$file_a b"] {
            assert!(!is_file_parameter(word), "{word}");
        }
    }

    #[test]
    fn masked_input_facts_survive_without_the_value_and_general_cards_are_refused() {
        let text = "SYNTHETIC_INPUT_ONLY";
        let argv = vec![
            "fill".into(),
            "browser-1".into(),
            serde_json::json!({"#opaque": text}).to_string(),
        ];
        let policy = input_policy("browser", &argv, &["#opaque".into()]);
        assert!(policy.secret);
        assert!(!serde_json::to_string(&policy).unwrap().contains(text));
        let card = "4242".repeat(4);
        let argv = vec![
            "fill".into(),
            "browser-1".into(),
            serde_json::json!([{"handle":"#reference","value":card}]).to_string(),
        ];
        assert!(input_policy("browser", &argv, &[]).credential);
    }
}
