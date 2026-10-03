//! Compile, inspect and roll back source-bound project rule definitions.

use std::io::Read;
use std::path::{Path, PathBuf};

use zerocode_core::jev::project_rules::{Book, MAX_BOOK_BYTES, store};

use super::{Refused, Report};

#[derive(Debug, Clone, PartialEq, Eq)]
enum Operation { Show, Compile(PathBuf), Activate(String) }

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Request { operation: Operation, cwd: Option<PathBuf> }

fn refused(message: impl Into<String>) -> Refused { Refused { message: message.into(), exit: 2 } }

pub(super) fn parse(args: &[String]) -> Result<Request, Refused> {
    let mut rest = args.iter();
    let operation = match rest.next().map(String::as_str) {
        Some("show") => Operation::Show,
        Some("compile") => Operation::Compile(PathBuf::from(rest.next().filter(|word| !word.starts_with('-'))
            .ok_or_else(|| refused("rules compile needs a definition file"))?)),
        Some("activate") => Operation::Activate(rest.next().filter(|word| !word.starts_with('-'))
            .ok_or_else(|| refused("rules activate needs a definition id"))?.clone()),
        _ => return Err(refused("rules needs show, compile or activate")),
    };
    let mut cwd = None;
    while let Some(flag) = rest.next() {
        match flag.as_str() {
            "--json" => {},
            "--cwd" if cwd.is_none() => cwd = Some(PathBuf::from(rest.next()
                .ok_or_else(|| refused("--cwd needs a directory"))?)),
            _ => return Err(refused(format!("unknown or repeated rules argument '{flag}'"))),
        }
    }
    Ok(Request { operation, cwd })
}

pub(super) fn run(request: &Request, cwd: &Path, home: &Path) -> Result<Report, Refused> {
    let workspace = request.cwd.as_deref().unwrap_or(cwd);
    match &request.operation {
        Operation::Show => {},
        Operation::Activate(id) => store::activate(home, workspace, id).map_err(|error| refused(error.to_string()))?,
        Operation::Compile(path) => {
            let mut bytes = Vec::new();
            std::fs::File::open(path).map_err(|error| refused(error.to_string()))?
                .take(u64::try_from(MAX_BOOK_BYTES).unwrap_or(u64::MAX).saturating_add(1))
                .read_to_end(&mut bytes).map_err(|error| refused(error.to_string()))?;
            if bytes.len() > MAX_BOOK_BYTES { return Err(refused("rule definition exceeds its byte limit")); }
            let book: Book = serde_json::from_slice(&bytes).map_err(|error| refused(error.to_string()))?;
            store::compile(home, workspace, book).map_err(|error| refused(error.to_string()))?;
        }
    }
    let book = store::load(home, workspace).map_err(|error| refused(error.to_string()))?;
    let id = book.as_ref().map(Book::identity).transpose().map_err(|error| refused(error.to_string()))?;
    Ok(Report { text: serde_json::json!({"schemaVersion":1,"workspace":store::workspace_root(workspace),
        "id":id,"definition":book,"settingsChanged":false}).to_string(), exit: 0 })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(values: &[&str]) -> Vec<String> { values.iter().map(|word| (*word).into()).collect() }

    #[test]
    fn rule_commands_refuse_missing_or_ambiguous_arguments() {
        for args in [vec!["compile"], vec!["activate"], vec!["show", "--cwd"],
            vec!["show", "--cwd", "/work/one", "--cwd", "/work/two"], vec!["unknown"]] {
            assert!(parse(&words(&args)).is_err());
        }
        assert!(parse(&words(&["compile", "rules.json", "--cwd", "/work/project", "--json"])).is_ok());
    }

    #[test]
    fn showing_an_unconfigured_project_changes_neither_files_nor_settings() {
        let workspace = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let report = run(&parse(&words(&["show"])).unwrap(), workspace.path(), home.path()).unwrap();
        let value: serde_json::Value = serde_json::from_str(&report.text).unwrap();
        assert!(value["definition"].is_null());
        assert_eq!(value["settingsChanged"], false);
        assert!(!home.path().join("jev").exists());
    }
}
