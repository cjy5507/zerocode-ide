//! Repository metadata read out of git config rather than asked for.
//!
//! The project catalog draws every repository in the sidebar and the composer,
//! and each row would like to say which repository on the internet it IS. That
//! is one string per project, and asking git for it costs a process per project
//! on a road that runs every time somebody clicks a workspace — the same trade
//! [`crate::git_dir`] exists to refuse. Creation bases use this module's same
//! direct-file boundary, so a catalog refresh does not spawn a separate
//! `git config` process for them either.
//!
//! **What this deliberately does not do is includes.** Git config can pull in
//! another file with `include.path` or `includeIf`, and this reader stops at
//! the file it was given. A remote declared through an include therefore reads
//! as *no remote*, which is the same answer as a repository with no remote at
//! all — a row that says nothing rather than a row that says something wrong.
//! Following includes means resolving `includeIf` conditions (`gitdir:`,
//! `onbranch:`, `hasconfig:`), which is a different piece of work and needs a
//! caller who actually loses something without it.

use std::collections::BTreeMap;
use std::path::Path;

/// Every `[remote "<name>"] url` in this config file, by remote name.
///
/// A [`BTreeMap`] because the one caller picks between them by name and then,
/// failing that, alphabetically — see [`primary_remote`].
#[must_use]
pub fn remote_urls(config: &Path) -> BTreeMap<String, String> {
    let Ok(text) = std::fs::read_to_string(config) else {
        return BTreeMap::new();
    };
    let mut found = BTreeMap::new();
    let mut remote: Option<String> = None;
    for line in joined_lines(&text) {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if let Some(header) = section_of(line) {
            remote = header;
            continue;
        }
        let Some(name) = remote.as_ref() else {
            continue;
        };
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        // Keys are case-insensitive in git config; section names are too, but
        // SUBSECTION names — the remote's own name — are not.
        if !key.trim().eq_ignore_ascii_case("url") {
            continue;
        }
        if let Some(url) = unquote(value.trim())
            && !url.is_empty()
        {
            // Last one wins, which is what git does with a repeated key when
            // the caller asks for a single value.
            found.insert(name.clone(), url);
        }
    }
    found
}

/// The remote this repository is best identified BY.
///
/// `upstream` before `origin` before everything else alphabetically — the
/// original's own order (`primaryRemoteSortKey`,
/// `shared/git-remote-identity.ts:114-122`). `upstream` leads because a fork's
/// identity on the provider is its parent's: the original carries that as
/// `repo.upstream` and shows it in the project row
/// (`getProjectProviderIdentity`, `project-host-setup-projection.ts:17-29`),
/// having learned it from the provider's API. We have no API answer on this
/// road and cannot afford one, and a fork that has been set up to contribute
/// has that same parent standing in its config under exactly this name — so
/// the cheap answer and the expensive one agree wherever the expensive one
/// would have mattered.
#[must_use]
pub fn primary_remote(config: &Path) -> Option<(String, String)> {
    let urls = remote_urls(config);
    let rank = |name: &str| match name {
        "upstream" => 0,
        "origin" => 1,
        _ => 2,
    };
    urls.iter()
        .min_by(|(left, _), (right, _)| rank(left).cmp(&rank(right)).then_with(|| left.cmp(right)))
        .map(|(name, url)| (name.clone(), url.clone()))
}

/// Every `branch.<name>.base` recorded in this config file, by branch name.
///
/// Creation bases are local repository metadata, so the catalog can read them
/// from the shared config it already knows rather than starting `git config`
/// once per repository. This deliberately has the same include boundary as
/// [`remote_urls`]: the caller gives us one file, and an included file is not
/// followed. The values this product writes live in that file.
#[must_use]
pub fn branch_bases(config: &Path) -> BTreeMap<String, String> {
    let Ok(text) = std::fs::read_to_string(config) else {
        return BTreeMap::new();
    };
    let mut branch: Option<String> = None;
    let mut found = BTreeMap::new();
    for line in joined_lines(&text) {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if let Some(header) = branch_section_of(line) {
            branch = header;
            continue;
        }
        let Some(name) = branch.as_ref() else {
            continue;
        };
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if !key.trim().eq_ignore_ascii_case("base") {
            continue;
        }
        if let Some(base) = unquote(value.trim())
            && !base.is_empty()
        {
            // Last one wins, matching git config's single-value lookup.
            found.insert(name.clone(), base);
        }
    }
    found
}

/// `[remote "origin"]` → `Some(Some("origin"))`, any other section header →
/// `Some(None)`, and a line that is not a header at all → [`None`].
///
/// Both spellings of a subsection are read. `[remote "origin"]` is what git
/// writes; `[remote.origin]` is the deprecated form it still accepts, and its
/// name is case-INSENSITIVE where the quoted form's is not — a difference this
/// keeps because git keeps it.
fn section_of(line: &str) -> Option<Option<String>> {
    section_named(line, "remote", true)
}

/// Read one config section's optional subsection. Unquoted subsection names
/// have the legacy case-folding Git applies to `[remote.origin]`; branch names
/// stay case-sensitive because they are ref names.
fn section_named(
    line: &str,
    wanted: &str,
    fold_unquoted_subsection: bool,
) -> Option<Option<String>> {
    let inside = line.strip_prefix('[')?.split(']').next()?.trim();
    if let Some((head, tail)) = inside.split_once('"') {
        let name = tail.strip_suffix('"')?;
        return Some(
            head.trim()
                .eq_ignore_ascii_case(wanted)
                .then(|| name.to_string()),
        );
    }
    let Some((head, tail)) = inside.split_once('.') else {
        return Some(None);
    };
    let name = tail.trim();
    Some(head.trim().eq_ignore_ascii_case(wanted).then(|| {
        if fold_unquoted_subsection {
            name.to_ascii_lowercase()
        } else {
            name.to_string()
        }
    }))
}

/// `[branch "main"]` → `Some(Some("main"))`, another section header →
/// `Some(None)`, and a non-header → `None`.
fn branch_section_of(line: &str) -> Option<Option<String>> {
    section_named(line, "branch", false)
}

/// A config value, with the two things that can wrap one taken off.
///
/// A trailing `#` or `;` starts a comment OUTSIDE quotes only, which is the
/// whole reason a URL ever gets quoted in this file. Inside quotes git honours
/// `\"` and `\\`; the other escapes it knows (`\n`, `\t`, `\b`) cannot appear
/// in a remote URL, so they are left as written rather than half-decoded.
fn unquote(value: &str) -> Option<String> {
    let mut out = String::with_capacity(value.len());
    let mut quoted = false;
    let mut chars = value.chars();
    while let Some(one) = chars.next() {
        match one {
            '"' => quoted = !quoted,
            '\\' => match chars.next() {
                Some(escaped @ ('"' | '\\')) => out.push(escaped),
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => return None,
            },
            '#' | ';' if !quoted => break,
            _ => out.push(one),
        }
    }
    if quoted {
        return None;
    }
    Some(out.trim().to_string())
}

/// Physical lines folded into logical ones on a trailing backslash.
///
/// Git's own parser does this, and a URL long enough to be wrapped is exactly
/// the value somebody would wrap.
fn joined_lines(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut held: Option<String> = None;
    for raw in text.lines() {
        let raw = raw.strip_suffix('\r').unwrap_or(raw);
        let (piece, more) = raw
            .strip_suffix('\\')
            .map_or((raw, false), |trimmed| (trimmed, true));
        let mut line = held.take().unwrap_or_default();
        line.push_str(piece);
        if more {
            held = Some(line);
        } else {
            lines.push(line);
        }
    }
    if let Some(last) = held {
        lines.push(last);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, text: &str) -> std::path::PathBuf {
        let at = dir.join("config");
        std::fs::write(&at, text).expect("write config");
        at
    }

    #[test]
    fn the_remotes_are_read_the_way_git_writes_them() {
        let dir = tempfile::tempdir().expect("tempdir");
        let at = write(
            dir.path(),
            concat!(
                "[core]\n",
                "\trepositoryformatversion = 0\n",
                "[remote \"origin\"]\n",
                "\turl = https://github.com/cjy5507/zerocode-ide.git\n",
                "\tfetch = +refs/heads/*:refs/remotes/origin/*\n",
                "[branch \"main\"]\n",
                "\tremote = origin\n",
            ),
        );
        let urls = remote_urls(&at);
        assert_eq!(
            urls.get("origin").map(String::as_str),
            Some("https://github.com/cjy5507/zerocode-ide.git")
        );
        // `[branch "main"]`도 `remote =`를 갖고 있다 — 구획을 안 보면 그것이
        // 원격의 주소로 읽힌다.
        assert_eq!(urls.len(), 1);
        assert_eq!(
            primary_remote(&at),
            Some((
                "origin".to_string(),
                "https://github.com/cjy5507/zerocode-ide.git".to_string()
            ))
        );
    }

    #[test]
    fn upstream_leads_origin_leads_the_alphabet() {
        let dir = tempfile::tempdir().expect("tempdir");
        let at = write(
            dir.path(),
            concat!(
                "[remote \"zed\"]\n\turl = https://example.test/z.git\n",
                "[remote \"origin\"]\n\turl = https://example.test/mine.git\n",
                "[remote \"upstream\"]\n\turl = https://example.test/parent.git\n",
            ),
        );
        assert_eq!(
            primary_remote(&at).map(|(name, _)| name),
            Some("upstream".to_string())
        );

        let without = write(
            dir.path(),
            concat!(
                "[remote \"zed\"]\n\turl = https://example.test/z.git\n",
                "[remote \"aardvark\"]\n\turl = https://example.test/a.git\n",
                "[remote \"origin\"]\n\turl = https://example.test/mine.git\n",
            ),
        );
        assert_eq!(
            primary_remote(&without).map(|(name, _)| name),
            Some("origin".to_string())
        );

        // 이름난 둘이 다 없으면 알파벳이 답한다 — 임의로 하나를 고르지 않는다.
        let neither = write(
            dir.path(),
            concat!(
                "[remote \"zed\"]\n\turl = https://example.test/z.git\n",
                "[remote \"aardvark\"]\n\turl = https://example.test/a.git\n",
            ),
        );
        assert_eq!(
            primary_remote(&neither).map(|(name, _)| name),
            Some("aardvark".to_string())
        );
    }

    #[test]
    fn the_awkward_spellings_still_answer() {
        let dir = tempfile::tempdir().expect("tempdir");
        // 접힌 줄, 따옴표 안의 `#`, 줄 끝 주석, 예전 구획 철자, 그리고
        // 대소문자가 다른 열쇠.
        let at = write(
            dir.path(),
            concat!(
                "[remote \"origin\"]\n",
                "\tURL = https://example.test/\\\n",
                "long/path.git\t; where it lives\n",
                "[remote.legacy]\n",
                "\turl = \"https://example.test/has#hash.git\"\n",
                "[remote \"empty\"]\n",
                "\turl =\n",
            ),
        );
        let urls = remote_urls(&at);
        assert_eq!(
            urls.get("origin").map(String::as_str),
            Some("https://example.test/long/path.git")
        );
        assert_eq!(
            urls.get("legacy").map(String::as_str),
            Some("https://example.test/has#hash.git")
        );
        // 비어 있는 값은 원격이 아니다.
        assert!(!urls.contains_key("empty"));
    }

    #[test]
    fn branch_bases_read_the_local_creation_metadata() {
        let dir = tempfile::tempdir().expect("tempdir");
        let at = write(
            dir.path(),
            concat!(
                "[branch \"main\"]\n",
                "\tbase = origin/main ; comment\n",
                "[branch \"wt/t-3\"]\n",
                "\tBASE = \"main\"\n",
                "[branch.feature]\n",
                "\tbase = release\\\n",
                "next\n",
                "[remote \"origin\"]\n",
                "\tbase = ignored\n",
                "[branch \"empty\"]\n",
                "\tbase =\n",
            ),
        );
        assert_eq!(
            branch_bases(&at),
            BTreeMap::from([
                ("main".to_string(), "origin/main".to_string()),
                ("wt/t-3".to_string(), "main".to_string()),
                ("feature".to_string(), "releasenext".to_string()),
            ])
        );
    }

    #[test]
    fn a_file_that_is_not_there_is_not_a_failure() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(remote_urls(&dir.path().join("nowhere")).is_empty());
        assert_eq!(primary_remote(&dir.path().join("nowhere")), None);

        // 그리고 include로 들여온 원격은 **없는 것으로 읽힌다** — 틀린 말을
        // 하는 행보다 아무 말도 안 하는 행이 낫다는 판단이고, 주석이 그
        // 판단을 말한다.
        let at = write(
            dir.path(),
            "[include]\n\tpath = ../elsewhere\n[core]\n\tbare = false\n",
        );
        assert_eq!(primary_remote(&at), None);
    }
}
