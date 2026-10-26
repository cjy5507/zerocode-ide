//! `zo settings secrets …` — the credentials written out in the person's own
//! MCP settings, found, checked and moved to a `${NAME}` reference, from
//! outside a session.
//!
//! The window calls these three in order: list what is written out, store the
//! value in its keychain item once the person agrees, ask whether the name now
//! resolves, then have the slot rewritten. Each prints one JSON object on
//! stdout and exits 0 when it did what it was asked, 1 when it did not.
//!
//! **A value is never printed.** The answers name files, keys and roads; a
//! value is compared in memory by [`runtime::move_mcp_literal_to_reference`]
//! and never reaches stdout, stderr or a test's output.

use std::path::{Path, PathBuf};

use runtime::{ConfigLoader, ReferenceKeys};
use serde_json::{json, Value};

pub const USAGE: &str = "\
zo settings secrets [--json] [--cwd <dir>]
zo settings secrets check --name <NAME> [--json]
zo settings secrets move --file <abs path> --path '<json array of keys>' --name <NAME> [--json]

  The credentials written out in the env and headers of the MCP servers in
  your own settings file (~/.zo/settings.json), by name — never by value.
  `check` says whether ${NAME} resolves, from zo's environment or the window's
  keychain item dev.zerocode.key.<NAME>. `move` rewrites one slot to ${NAME}
  only when NAME resolves to exactly the value written there and the file did
  not change meanwhile; the old file is kept beside it as a timestamped backup.
";

/// What one command answered: the JSON it prints and its exit code.
#[derive(Debug, Clone, PartialEq)]
pub struct Answer {
    pub json: Value,
    pub code: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Request {
    List,
    Check { name: String },
    Move { file: String, path: String, name: String },
}

/// # Errors
///
/// The usage, or what was wrong with the arguments.
pub fn run(args: &[String], cwd: &Path) -> Result<Answer, String> {
    let (request, cwd_flag) = parse(args)?;
    let cwd = cwd_flag.unwrap_or_else(|| cwd.to_path_buf());
    Ok(answer(&request, &ConfigLoader::default_for(&cwd), &runtime::LaunchReferenceKeys))
}

fn parse(args: &[String]) -> Result<(Request, Option<PathBuf>), String> {
    let mut words = args.iter().map(String::as_str);
    if words.next() != Some("secrets") {
        return Err(USAGE.to_string());
    }
    let mut verb = None;
    let mut name = None;
    let mut file = None;
    let mut path = None;
    let mut cwd = None;
    while let Some(word) = words.next() {
        let mut value = |flag: &str| {
            words
                .next()
                .map(str::to_string)
                .ok_or_else(|| format!("{flag} requires a value\n\n{USAGE}"))
        };
        match word {
            "--json" => {}
            "--name" => name = Some(value("--name")?),
            "--file" => file = Some(value("--file")?),
            "--path" => path = Some(value("--path")?),
            "--cwd" => cwd = Some(PathBuf::from(value("--cwd")?)),
            "check" | "move" if verb.is_none() => verb = Some(word),
            "--help" | "-h" | "help" => return Err(USAGE.to_string()),
            other => return Err(format!("unexpected argument {other}\n\n{USAGE}")),
        }
    }
    let missing = |flag: &str| format!("{flag} is required\n\n{USAGE}");
    let request = match verb {
        None => Request::List,
        Some("check") => Request::Check {
            name: name.ok_or_else(|| missing("--name"))?,
        },
        Some(_) => Request::Move {
            file: file.ok_or_else(|| missing("--file"))?,
            path: path.ok_or_else(|| missing("--path"))?,
            name: name.ok_or_else(|| missing("--name"))?,
        },
    };
    Ok((request, cwd))
}

fn answer(request: &Request, loader: &ConfigLoader, keys: &dyn ReferenceKeys) -> Answer {
    match request {
        Request::List => list(loader),
        Request::Check { name } => check(name, keys),
        Request::Move { file, path, name } => move_slot(loader, keys, file, path, name),
    }
}

/// The person's own settings files: every user root the loader reads, in the
/// order it merges them, each once, as the loader names them — links not
/// followed, so a link into a workspace never makes its target one of them.
fn user_settings_files(loader: &ConfigLoader) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = Vec::new();
    for root in loader.config_roots().iter().rev() {
        let file = lexical(&root.join("settings.json"));
        if !files.contains(&file) {
            files.push(file);
        }
    }
    files
}

/// `path` made absolute and its `.` and `..` folded, without asking the disk.
fn lexical(path: &Path) -> PathBuf {
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    core_types::paths::normalize_path_components(&absolute)
}

fn list(loader: &ConfigLoader) -> Answer {
    // Where the person's own file cannot be told from a workspace's, an empty
    // list would say there is nothing to move; say why instead.
    if !runtime::can_tell_persons_own_settings_file() {
        return Answer {
            json: json!({
                "literals": [],
                "supported": false,
                "why": "this system cannot tell your own settings file from a workspace's.",
            }),
            code: 0,
        };
    }
    let mut literals = Vec::new();
    for file in user_settings_files(loader) {
        // Only the file opened and judged to be the person's own is read: a
        // link, a file in the workspace or one with a second name lists none.
        match runtime::persons_own_literal_secrets(&file, loader.cwd()) {
            Ok(found) => literals.extend(found.iter().map(|slot| {
                json!({
                    "file": file.display().to_string(),
                    "path": slot.key_path(),
                    "name": slot.name,
                })
            })),
            Err(why) => eprintln!("zo settings: {why}"),
        }
    }
    Answer {
        json: json!({ "literals": literals }),
        code: 0,
    }
}

fn check(name: &str, keys: &dyn ReferenceKeys) -> Answer {
    // Where the person's own file cannot be told, nothing can be moved, so a
    // check says that rather than which road would answer.
    if !runtime::can_tell_persons_own_settings_file() {
        return Answer {
            json: json!({
                "name": name,
                "resolves": false,
                "why": "this system cannot tell your own settings file from a workspace's.",
            }),
            code: 1,
        };
    }
    match runtime::resolve_reference_name(name, keys, true) {
        Ok((_, from)) => Answer {
            json: json!({ "name": name, "resolves": true, "from": from.token() }),
            code: 0,
        },
        Err(miss) => Answer {
            json: json!({ "name": name, "resolves": false, "why": miss.why() }),
            code: 1,
        },
    }
}

fn move_slot(
    loader: &ConfigLoader,
    keys: &dyn ReferenceKeys,
    file: &str,
    key_path: &str,
    name: &str,
) -> Answer {
    let refused = |why: String| Answer {
        json: json!({ "moved": false, "why": why }),
        code: 1,
    };
    let given = Path::new(file);
    if !given.is_absolute() {
        return refused("--file must be an absolute path.".to_string());
    }
    // The boundary is the path as given, before any link is followed; the
    // move itself then checks that the file there is the person's own and
    // keeps it the same file from its read to its replacement.
    let file = lexical(given);
    if !user_settings_files(loader).contains(&file) {
        return refused(format!(
            "{} is not your user settings file; only that file is rewritten.",
            given.display()
        ));
    }
    let Ok(key_path) = serde_json::from_str::<Vec<String>>(key_path) else {
        return refused("--path must be a JSON array of keys.".to_string());
    };
    let resolve = |name: &str| {
        runtime::resolve_reference_name(name, keys, true)
            .map(|(value, _)| value)
            .map_err(|miss| miss.why().to_string())
    };
    match runtime::move_mcp_literal_to_reference(&file, loader.cwd(), &key_path, name, &resolve) {
        Ok(backup) => Answer {
            json: json!({ "moved": true, "backup": backup.display().to_string() }),
            code: 0,
        },
        Err(why) => refused(why),
    }
}

#[cfg(test)]
mod tests {
    // Most tests need a system that tells the person's own file; elsewhere
    // only the refusals run.
    #![cfg_attr(not(any(target_os = "macos", target_os = "linux")), allow(dead_code, unused_imports))]

    use std::cell::RefCell;
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};

    use runtime::{ConfigLoader, ReferenceKeys};
    use serde_json::{json, Value};
    use tempfile::TempDir;

    use super::{answer, parse, Answer};

    /// Values no real key has, so a leak is a string a test can look for.
    const LITERAL: &str = "fixture-literal-token-7a1b2c3d4e5f60718293a4b5c6d7e8f9";
    const ENV_VALUE: &str = "fixture-env-value-3141";

    /// An environment and a keychain of the test's own: the person's keychain
    /// and environment are never read.
    #[derive(Default)]
    struct Keys {
        environment: BTreeMap<String, String>,
        keychain: BTreeMap<String, String>,
        asked: RefCell<Vec<String>>,
    }

    impl ReferenceKeys for Keys {
        fn environment(&self, name: &str) -> Option<String> {
            self.environment.get(name).cloned()
        }

        fn keychain(&self, name: &str) -> api::ReferenceKey {
            self.asked.borrow_mut().push(name.to_string());
            self.keychain
                .get(name)
                .cloned()
                .map_or(api::ReferenceKey::Absent, api::ReferenceKey::Found)
        }
    }

    /// A config home and a workspace nobody else shares.
    struct Scratch {
        home: TempDir,
        cwd: TempDir,
    }

    impl Scratch {
        fn new() -> Self {
            let scratch = Self {
                home: tempfile::tempdir().expect("config home"),
                cwd: tempfile::tempdir().expect("workspace"),
            };
            let document = json!({
                "model": "opus",
                "mcpServers": {
                    "github": {
                        "command": "npx",
                        "env": {
                            "GITHUB_PERSONAL_ACCESS_TOKEN": LITERAL,
                            "GITHUB_HOST": "github.example",
                        },
                    },
                },
            });
            std::fs::write(scratch.home_settings(), document.to_string()).expect("home settings");
            std::fs::create_dir_all(scratch.cwd.path().join(".zo")).expect(".zo");
            std::fs::write(scratch.project_settings(), document.to_string()).expect("project settings");
            scratch
        }

        fn loader(&self) -> ConfigLoader {
            ConfigLoader::new(self.cwd.path(), self.home.path())
        }

        fn home_settings(&self) -> PathBuf {
            self.home.path().join("settings.json")
        }

        fn project_settings(&self) -> PathBuf {
            self.cwd.path().join(".zo").join("settings.json")
        }

        fn run(&self, line: &[&str], keys: &Keys) -> Answer {
            let args: Vec<String> = line.iter().map(|word| (*word).to_string()).collect();
            let (request, _) = parse(&args).expect("parses");
            let answered = answer(&request, &self.loader(), keys);
            let printed = answered.json.to_string();
            // Said without the answer, which would carry the value.
            assert!(!printed.contains(LITERAL), "a value was printed");
            assert!(!printed.contains(ENV_VALUE), "a value was printed");
            answered
        }
    }


    const TOKEN_PATH: &str = r#"["mcpServers","github","env","GITHUB_PERSONAL_ACCESS_TOKEN"]"#;

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn secrets_lists_the_user_files_literal_slots_by_name_only() {
        let scratch = Scratch::new();
        let answered = scratch.run(&["secrets", "--json"], &Keys::default());
        assert_eq!(answered.code, 0);
        assert_eq!(
            answered.json,
            json!({"literals": [{
                "file": scratch.home_settings().display().to_string(),
                "path": ["mcpServers", "github", "env", "GITHUB_PERSONAL_ACCESS_TOKEN"],
                "name": "GITHUB_PERSONAL_ACCESS_TOKEN",
            }]}),
            "the project file's slot is not the person's own and is not listed"
        );
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn check_says_which_road_answers_and_never_the_value() {
        let scratch = Scratch::new();
        let keys = Keys {
            environment: BTreeMap::from([("ON_ENV".to_string(), ENV_VALUE.to_string())]),
            keychain: BTreeMap::from([("ON_KEYCHAIN".to_string(), LITERAL.to_string())]),
            ..Keys::default()
        };
        let env = scratch.run(&["secrets", "check", "--name", "ON_ENV", "--json"], &keys);
        assert_eq!((env.json, env.code), (json!({"name": "ON_ENV", "resolves": true, "from": "env"}), 0));
        let keychain = scratch.run(&["secrets", "check", "--name", "ON_KEYCHAIN", "--json"], &keys);
        assert_eq!(
            (keychain.json, keychain.code),
            (json!({"name": "ON_KEYCHAIN", "resolves": true, "from": "keychain"}), 0)
        );
        let absent = scratch.run(&["secrets", "check", "--name", "NOWHERE", "--json"], &keys);
        assert_eq!(absent.code, 1);
        assert_eq!(absent.json["resolves"], json!(false));
        assert!(absent.json["why"].as_str().is_some_and(|why| !why.is_empty()), "{}", absent.json);
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn move_rewrites_the_user_file_once_the_name_resolves_to_the_same_value() {
        let scratch = Scratch::new();
        let file = scratch.home_settings().display().to_string();
        let mismatch = Keys {
            keychain: BTreeMap::from([(
                "GITHUB_PERSONAL_ACCESS_TOKEN".to_string(),
                ENV_VALUE.to_string(),
            )]),
            ..Keys::default()
        };
        let line = [
            "secrets", "move", "--file", &file, "--path", TOKEN_PATH, "--name",
            "GITHUB_PERSONAL_ACCESS_TOKEN", "--json",
        ];
        let refused = scratch.run(&line, &mismatch);
        assert_eq!((refused.json["moved"].clone(), refused.code), (json!(false), 1), "{}", refused.json);

        let project = scratch.project_settings().display().to_string();
        let mut project_line = line;
        project_line[3] = &project;
        let matching = Keys {
            keychain: BTreeMap::from([(
                "GITHUB_PERSONAL_ACCESS_TOKEN".to_string(),
                LITERAL.to_string(),
            )]),
            ..Keys::default()
        };
        let refused = scratch.run(&project_line, &matching);
        assert_eq!((refused.json["moved"].clone(), refused.code), (json!(false), 1), "{}", refused.json);
        assert!(
            refused.json["why"].as_str().is_some_and(|why| why.contains("not your user settings file")),
            "{}",
            refused.json
        );
        assert!(std::fs::read_to_string(scratch.project_settings()).expect("project").contains(LITERAL));

        let moved = scratch.run(&line, &matching);
        assert_eq!((moved.json["moved"].clone(), moved.code), (json!(true), 0), "{}", moved.json);
        let backup = moved.json["backup"].as_str().expect("backup path");
        assert!(Path::new(backup).is_file(), "{backup}");
        let after: Value = serde_json::from_str(&std::fs::read_to_string(scratch.home_settings()).expect("home"))
            .expect("json");
        assert_eq!(
            after["mcpServers"]["github"]["env"]["GITHUB_PERSONAL_ACCESS_TOKEN"],
            json!("${GITHUB_PERSONAL_ACCESS_TOKEN}")
        );
        assert_eq!(after["model"], json!("opus"));

        let again = scratch.run(&line, &matching);
        assert_eq!((again.json["moved"].clone(), again.code), (json!(false), 1), "a reference is not moved twice");
        assert!(
            scratch.run(&["secrets"], &matching).json["literals"]
                .as_array()
                .is_some_and(Vec::is_empty),
            "nothing written out is left"
        );
    }

    /// Where the person's own file cannot be told from a workspace's, the
    /// list says so instead of listing nothing.
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    #[test]
    fn secrets_says_it_cannot_tell_the_persons_own_file_rather_than_listing_nothing() {
        let scratch = Scratch::new();
        let answered = scratch.run(&["secrets", "--json"], &Keys::default());
        assert_eq!(answered.code, 0);
        assert_eq!(answered.json["supported"], json!(false), "{}", answered.json);
        assert_eq!(answered.json["literals"], json!([]), "{}", answered.json);
        assert!(answered.json["why"].as_str().is_some_and(|why| !why.is_empty()), "{}", answered.json);
    }

    /// The person's settings file is a link to the workspace's own
    /// `.zo/settings.json`: neither path is the person's to list or rewrite.
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn a_user_settings_link_into_the_workspace_is_neither_listed_nor_moved() {
        let scratch = Scratch::new();
        std::fs::remove_file(scratch.home_settings()).expect("drop the home file");
        std::os::unix::fs::symlink(scratch.project_settings(), scratch.home_settings()).expect("link");
        let matching = Keys {
            keychain: BTreeMap::from([(
                "GITHUB_PERSONAL_ACCESS_TOKEN".to_string(),
                LITERAL.to_string(),
            )]),
            ..Keys::default()
        };
        let listed = scratch.run(&["secrets", "--json"], &matching);
        assert_eq!(listed.json, json!({"literals": []}), "the workspace's file is not listed as the person's");

        let link = scratch.home_settings().display().to_string();
        let project = scratch.project_settings().display().to_string();
        let canonical_project = std::fs::canonicalize(scratch.project_settings())
            .expect("canonical")
            .display()
            .to_string();
        for file in [&link, &project, &canonical_project] {
            let words = [
                "secrets", "move", "--file", file, "--path", TOKEN_PATH, "--name",
                "GITHUB_PERSONAL_ACCESS_TOKEN", "--json",
            ];
            let refused = scratch.run(&words, &matching);
            assert_eq!((refused.json["moved"].clone(), refused.code), (json!(false), 1), "{file}: {}", refused.json);
        }
        assert!(std::fs::read_to_string(scratch.project_settings()).expect("project").contains(LITERAL));
        let backups: Vec<_> = std::fs::read_dir(scratch.cwd.path().join(".zo"))
            .expect(".zo")
            .map(|entry| entry.expect("entry").file_name().to_string_lossy().into_owned())
            .filter(|name| name.contains(".bak-"))
            .collect();
        assert!(backups.is_empty(), "no backup in the project folder: {backups:?}");
    }
}
