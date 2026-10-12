//! `zerocode vault-types` — give every wiki page that names no kind a `type:`
//! key (t-43992 unit 2), through
//! [`zerocode_core::second_brain_kinds::backfill`]. A dry run unless
//! `--write`; the vault comes by the same three roads as `vault-lint`'s.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use zerocode_core::second_brain::{VAULT_ENV, WIKI_DIR, WIKI_LOG_FILE};
use zerocode_core::second_brain_kinds::{Backfill, backfill};

use crate::vault_lint;

pub const USAGE: &str = "\
zerocode vault-types [--vault <dir>] [--write]

    Give every wiki page that names no kind a type: key - source, entity,
    concept, comparison, overview or synthesis - guessed from its file name,
    title and tags. Prints the plan and changes nothing unless --write;
    with --write it adds the key as the first frontmatter line, keeps a
    type: someone wrote as written, never opens raw/, and appends one line
    to wiki/log.md.
    The vault is --vault, else $ZEROCODE_SECOND_BRAIN, else the one the
    ZeroCode window saved. Exit 0 done, 1 when a page could not be read or
    written, 2 with no vault.
";

const EXIT_FAILED: u8 = 1;
const EXIT_NO_VAULT: u8 = 2;

#[derive(Debug, Default, PartialEq, Eq)]
struct Options {
    vault: Option<PathBuf>,
    write: bool,
}

fn parse(args: &[String]) -> Result<Options, String> {
    let mut options = Options::default();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--vault" => {
                let value = iter.next().ok_or("--vault needs a directory")?;
                options.vault = Some(PathBuf::from(value));
            }
            "--write" => options.write = true,
            "-h" | "--help" => return Err(USAGE.to_string()),
            other => return Err(format!("unknown argument `{other}`\n\n{USAGE}")),
        }
    }
    Ok(options)
}

pub fn run(args: &[String]) -> ExitCode {
    let options = match parse(args) {
        Ok(options) => options,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::from(EXIT_NO_VAULT);
        }
    };
    let Some(named) = vault_lint::resolve(
        options.vault,
        std::env::var(VAULT_ENV).ok(),
        vault_lint::saved_vault,
    ) else {
        eprintln!(
            "zerocode: no vault to type — pass --vault <dir>, export {VAULT_ENV}, or set up the second brain in the window"
        );
        return ExitCode::from(EXIT_NO_VAULT);
    };
    let root = named.path();
    if !root.join(WIKI_DIR).is_dir() {
        eprintln!(
            "zerocode: {} ({}) has no {WIKI_DIR}/ — not a second-brain vault",
            root.display(),
            named.road()
        );
        return ExitCode::from(EXIT_NO_VAULT);
    }
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|since| i64::try_from(since.as_millis()).ok())
        .unwrap_or_default();
    match backfill(root, options.write, now_ms) {
        Ok(done) => {
            print!("{}", render(root, named.road(), &done, options.write));
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("zerocode: {error}");
            ExitCode::from(EXIT_FAILED)
        }
    }
}

/// The plan, one page a line, then what was left alone and what happened.
#[must_use]
pub fn render(root: &Path, road: &str, done: &Backfill, wrote: bool) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "vault: {} ({road})", root.display());
    for (page, kind) in &done.planned {
        let _ = writeln!(out, "{page} → {}", kind.as_str());
    }
    if !done.kept.is_empty() {
        let _ = writeln!(out, "\nkept as written — a type outside the six kinds:");
        for (page, value) in &done.kept {
            let _ = writeln!(out, "    {page} ({value})");
        }
    }
    if !done.no_frontmatter.is_empty() {
        let _ = writeln!(out, "\nleft as is — no frontmatter to hold the key:");
        for page in &done.no_frontmatter {
            let _ = writeln!(out, "    {page}");
        }
    }
    if !done.unreadable.is_empty() {
        let _ = writeln!(out, "\nleft as is — not UTF-8 text:");
        for page in &done.unreadable {
            let _ = writeln!(out, "    {page}");
        }
    }
    let planned = done.planned.len();
    let _ = if planned == 0 {
        writeln!(out, "\nevery page names a kind — nothing to do")
    } else if wrote {
        writeln!(
            out,
            "\n{} pages typed · one line added to {WIKI_LOG_FILE}",
            done.written
        )
    } else {
        writeln!(
            out,
            "\n{planned} pages would get a type · a dry run, nothing changed — run again with --write to apply"
        )
    };
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(words: &[&str]) -> Vec<String> {
        words.iter().map(|word| (*word).to_string()).collect()
    }

    #[test]
    fn arguments_parse_and_anything_else_is_refused_with_the_usage() {
        assert_eq!(parse(&[]), Ok(Options::default()));
        assert_eq!(
            parse(&args(&["--write", "--vault", "/v"])),
            Ok(Options {
                vault: Some(PathBuf::from("/v")),
                write: true,
            })
        );
        assert!(parse(&args(&["--vault"])).is_err());
        let refused = parse(&args(&["--json"])).unwrap_err();
        assert!(refused.contains("zerocode vault-types [--vault <dir>] [--write]"));
    }
}
