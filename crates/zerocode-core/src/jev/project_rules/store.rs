//! Private, versioned rule definitions shared by a project and its registered worktrees.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use serde_json::json;

use super::{Advice, Book, Invalid, MAX_BOOK_BYTES, MAX_SOURCE_BYTES, source_hash};

#[must_use]
pub fn workspace_root(workspace: &Path) -> PathBuf {
    let root = workspace
        .ancestors()
        .find(|root| crate::git_dir::of(root).is_some())
        .unwrap_or(workspace);
    root.canonicalize().unwrap_or_else(|_| root.to_path_buf())
}

/// Resolve a tool's edited path from its working directory into the rule
/// definition's repository coordinates, including panes opened in a subfolder.
///
/// # Errors
/// Paths outside the project or paths that cannot be represented unambiguously.
pub fn edit_path(workspace: &Path, path: &str) -> Result<String, Invalid> {
    let cwd = workspace.canonicalize().map_err(|_| Invalid::Source)?;
    let root = workspace_root(&cwd);
    let absolute = cwd.join(path);
    let absolute = absolute.canonicalize().unwrap_or(absolute);
    let path = absolute
        .strip_prefix(root)
        .map_err(|_| Invalid::Outside)?
        .to_str()
        .ok_or(Invalid::Source)?
        .replace(std::path::MAIN_SEPARATOR, "/");
    if !super::relative(&path) {
        return Err(Invalid::Source);
    }
    Ok(path)
}

fn directory(home: &Path, workspace: &Path) -> PathBuf {
    let root = workspace_root(workspace);
    let owner = crate::git_dir::owning_checkout_of(&root).unwrap_or(root);
    home.join("jev/project-rules")
        .join(source_hash(owner.to_string_lossy().as_bytes()))
}

fn bounded(path: &Path, cap: usize) -> io::Result<Vec<u8>> {
    let mut data = Vec::new();
    File::open(path)?
        .take(u64::try_from(cap).unwrap_or(u64::MAX).saturating_add(1))
        .read_to_end(&mut data)?;
    if data.len() > cap {
        return Err(io::Error::other(Invalid::Size));
    }
    Ok(data)
}

fn sources(workspace: &Path, book: &Book) -> io::Result<BTreeMap<String, Vec<u8>>> {
    let root = workspace_root(workspace).canonicalize()?;
    let mut sources = BTreeMap::new();
    for rule in &book.rules {
        if sources.contains_key(&rule.source.path) {
            continue;
        }
        let path = root.join(&rule.source.path).canonicalize()?;
        if !path.starts_with(&root) || !path.is_file() {
            return Err(io::Error::other(Invalid::Source));
        }
        sources.insert(rule.source.path.clone(), bounded(&path, MAX_SOURCE_BYTES)?);
    }
    Ok(sources)
}

/// Refuse a turn whose applicable instruction files are absent from the
/// reviewed definition. Only ancestors of actual edits are visited, never
/// the whole repository. This also detects a new nested override between
/// judging a turn and delivering its advisory.
///
/// # Errors
/// Unbounded/escaping edit paths, an uncompiled instruction file, changed
/// sources, or a filesystem error. Missing evidence is not compliance.
pub fn verify_scope(workspace: &Path, book: &Book, edited: &[String]) -> io::Result<()> {
    if edited.len() > super::MAX_EDITED_PATHS || edited.iter().any(|path| !super::relative(path)) {
        return Err(io::Error::other(Invalid::Size));
    }
    let root = workspace_root(workspace).canonicalize()?;
    let mut directories = BTreeSet::from([PathBuf::new()]);
    for path in edited {
        for directory in Path::new(path)
            .parent()
            .into_iter()
            .flat_map(Path::ancestors)
        {
            directories.insert(directory.to_path_buf());
            if directories.len() > 128 {
                return Err(io::Error::other(Invalid::Size));
            }
        }
    }
    for directory in directories {
        for name in super::INSTRUCTION_FILES {
            let relative = directory.join(name);
            let path = root.join(&relative);
            match fs::symlink_metadata(&path) {
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error),
                Ok(_) => {}
            }
            if !book
                .rules
                .iter()
                .any(|rule| Path::new(&rule.source.path) == relative)
            {
                return Err(io::Error::other(Invalid::Changed));
            }
        }
    }
    book.verify_sources(&sources(workspace, book)?)
        .map_err(io::Error::other)
}

fn private_file(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

fn replace(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let temp = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut out = private_file(&temp)?;
        out.write_all(bytes)?;
        out.sync_all()?;
        fs::rename(&temp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}

fn lock(root: &Path) -> io::Result<File> {
    lock_file(root, ".lock")
}

fn lock_file(root: &Path, name: &str) -> io::Result<File> {
    fs::create_dir_all(root)?;
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(root.join(name))?;
    let until = std::time::Instant::now() + std::time::Duration::from_millis(25);
    loop {
        match file.try_lock() {
            Ok(()) => break,
            Err(std::fs::TryLockError::WouldBlock) if std::time::Instant::now() < until => {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            Err(error) => return Err(io::Error::other(error)),
        }
    }
    Ok(file)
}

fn lock_recipient(root: &Path, recipient: &str) -> io::Result<File> {
    lock_file(
        root,
        &format!(".lock-{}", source_hash(recipient.as_bytes())),
    )
}

fn identity(word: &str) -> bool {
    word.len() == 64
        && word
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

/// Compile exact source excerpts and activate an immutable definition.
/// An empty source hash requests compilation against the current source;
/// an already-bound hash must still match. No question is generated here.
///
/// # Errors
/// Invalid definitions, stale/escaping sources or failure to persist atomically.
pub fn compile(home: &Path, workspace: &Path, mut book: Book) -> io::Result<String> {
    let unbound = book
        .rules
        .iter()
        .map(|rule| rule.source.sha256.is_empty())
        .collect::<Vec<_>>();
    for rule in &mut book.rules {
        if rule.source.sha256.is_empty() {
            rule.source.sha256 = "0".repeat(64);
        }
    }
    book.validate().map_err(io::Error::other)?;
    let sources = sources(workspace, &book)?;
    for (rule, unbound) in book.rules.iter_mut().zip(unbound) {
        if unbound {
            rule.source.sha256 = source_hash(&sources[&rule.source.path]);
        }
    }
    book.verify_sources(&sources).map_err(io::Error::other)?;
    let id = book.identity().map_err(io::Error::other)?;
    let root = directory(home, workspace);
    let _lock = lock(&root)?;
    let bytes = serde_json::to_vec(&book).map_err(io::Error::other)?;
    let destination = root.join(format!("{id}.json"));
    if destination.exists() {
        if bounded(&destination, MAX_BOOK_BYTES)? != bytes {
            return Err(io::Error::other("rule definition identity collision"));
        }
    } else {
        replace(&destination, &bytes)?;
    }
    // Check again immediately before making a delayed compilation active.
    book.verify_sources(&self::sources(workspace, &book)?)
        .map_err(io::Error::other)?;
    replace(
        &root.join("active.json"),
        &serde_json::to_vec(&json!({"id":id})).map_err(io::Error::other)?,
    )?;
    Ok(id)
}

/// Read the active definition only while its source still matches.
///
/// # Errors
/// Corrupt, changed or escaping definitions/sources. A missing selection is empty.
pub fn load(home: &Path, workspace: &Path) -> io::Result<Option<Book>> {
    let root = directory(home, workspace);
    let selection = match bounded(&root.join("active.json"), 1024) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let selection: serde_json::Value =
        serde_json::from_slice(&selection).map_err(io::Error::other)?;
    let id = selection["id"]
        .as_str()
        .filter(|id| identity(id))
        .ok_or_else(|| io::Error::other(Invalid::Definition))?;
    load_id(&root, workspace, id).map(Some)
}

fn load_id(root: &Path, workspace: &Path, id: &str) -> io::Result<Book> {
    let book: Book =
        serde_json::from_slice(&bounded(&root.join(format!("{id}.json")), MAX_BOOK_BYTES)?)
            .map_err(io::Error::other)?;
    if book.identity().map_err(io::Error::other)? != id {
        return Err(io::Error::other(Invalid::Definition));
    }
    book.verify_sources(&sources(workspace, &book)?)
        .map_err(io::Error::other)?;
    Ok(book)
}

/// Select a previous immutable definition, including a rollback, after checking
/// the current instructions. The global Jev switch and feature modes are untouched.
///
/// # Errors
/// Missing, changed or invalid definitions, a busy selector, or an I/O failure.
pub fn activate(home: &Path, workspace: &Path, id: &str) -> io::Result<()> {
    if !identity(id) {
        return Err(io::Error::other(Invalid::Definition));
    }
    let root = directory(home, workspace);
    let _lock = lock(&root)?;
    load_id(&root, workspace, id)?;
    replace(
        &root.join("active.json"),
        &serde_json::to_vec(&json!({"id":id})).map_err(io::Error::other)?,
    )
}

const HISTORY_LIMIT: usize = 128;

fn advice_path(root: &Path, recipient: &str) -> PathBuf {
    root.join(format!("advice-{}.json", source_hash(recipient.as_bytes())))
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct CheckPoint {
    at: u64,
    /// None means two distinct checks shared a timestamp; neither may apply.
    turn: Option<String>,
}

fn checkpoint(path: &Path) -> io::Result<Option<CheckPoint>> {
    match bounded(&path.with_extension("latest"), 512) {
        Ok(bytes) => {
            let point: CheckPoint = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
            if point.turn.as_deref().is_some_and(|turn| !identity(turn)) {
                return Err(io::Error::other(Invalid::Definition));
            }
            Ok(Some(point))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// Announce every check before awaiting a model, including checks that later
/// find no violation. A newer turn retires the previous observation, and a
/// delayed older answer can no longer re-create it.
///
/// # Errors
/// Busy recipient storage, corrupt state or an I/O error.
pub fn begin_check(
    home: &Path,
    workspace: &Path,
    recipient: &str,
    at: u64,
    turn: &str,
) -> io::Result<bool> {
    let root = directory(home, workspace);
    // Unconfigured projects remain completely untouched. A configured
    // project's turn watermark also retires old work while its mode is Off.
    if !root.join("active.json").try_exists()? {
        return Ok(false);
    }
    let _lock = lock_recipient(&root, recipient)?;
    let path = advice_path(&root, recipient);
    let turn = source_hash(turn.as_bytes());
    let held = checkpoint(&path)?;
    if let Some(point) = &held {
        if point.at > at {
            return Ok(false);
        }
        if point.at == at && point.turn.as_deref() == Some(turn.as_str()) {
            return Ok(true);
        }
    }
    if read_advice(&path)?.is_some_and(|advice| advice.at > at) {
        return Ok(false);
    }
    let ordered = held.is_none_or(|point| point.at < at);
    let next = CheckPoint {
        at,
        turn: ordered.then_some(turn),
    };
    replace(
        &path.with_extension("latest"),
        &serde_json::to_vec(&next).map_err(io::Error::other)?,
    )?;
    match fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    Ok(ordered)
}

fn current(path: &Path, advice: &Advice) -> io::Result<bool> {
    Ok(checkpoint(path)?.is_some_and(|point| {
        point.at == advice.at && point.turn.as_deref() == Some(advice.turn.as_str())
    }))
}

fn read_advice(path: &Path) -> io::Result<Option<Advice>> {
    match bounded(path, MAX_BOOK_BYTES) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(io::Error::other),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn history(path: &Path) -> io::Result<Vec<String>> {
    match bounded(path, 16 * 1024) {
        Ok(bytes) => {
            let ids: Vec<String> = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
            if ids.len() > HISTORY_LIMIT || ids.iter().any(|id| !identity(id)) {
                return Err(io::Error::other(Invalid::Definition));
            }
            Ok(ids)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error),
    }
}

/// Queue one observation after the host's On-mode and freshness checks.
/// Older asynchronous replies cannot replace a newer turn's advisory.
///
/// # Errors
/// Stale definitions, invalid advice, busy storage or an I/O failure.
pub fn offer(home: &Path, workspace: &Path, recipient: &str, advice: &Advice) -> io::Result<bool> {
    if !identity(&advice.key)
        || !identity(&advice.turn)
        || advice.text.chars().count() > super::ADVICE_CHAR_CAP
        || advice.rules.is_empty()
        || advice.rules.len() > super::ADVICE_RULE_CAP
    {
        return Err(io::Error::other(Invalid::Definition));
    }
    let Some(book) = load(home, workspace)? else {
        return Ok(false);
    };
    if book.identity().map_err(io::Error::other)? != advice.definition {
        return Ok(false);
    }
    verify_scope(workspace, &book, &advice.edited)?;
    let root = directory(home, workspace);
    let _lock = lock_recipient(&root, recipient)?;
    let path = advice_path(&root, recipient);
    if !current(&path, advice)?
        || history(&path.with_extension("seen"))?.contains(&advice.key)
        || read_advice(&path)?.is_some_and(|old| old.at >= advice.at)
    {
        return Ok(false);
    }
    replace(
        &path,
        &serde_json::to_vec(advice).map_err(io::Error::other)?,
    )?;
    Ok(true)
}

/// Inspect without consuming. A host can withhold a contribution when its
/// remaining prompt budget cannot carry it, then try at a later boundary.
///
/// # Errors
/// Invalid/stale definitions or stored advice, or an I/O failure.
pub fn pending(
    home: &Path,
    workspace: &Path,
    recipient: &str,
    room: usize,
) -> io::Result<Option<Advice>> {
    let Some(book) = load(home, workspace)? else {
        return Ok(None);
    };
    let path = advice_path(&directory(home, workspace), recipient);
    let Some(advice) = read_advice(&path)? else {
        return Ok(None);
    };
    if !identity(&advice.key)
        || !identity(&advice.turn)
        || advice.rules.is_empty()
        || advice.rules.len() > super::ADVICE_RULE_CAP
        || advice.text.chars().count() > super::ADVICE_CHAR_CAP
        || !advice
            .text
            .starts_with(&format!("[zo:project-rules:{}]\n", advice.key))
        || advice
            .rules
            .iter()
            .any(|id| !book.rules.iter().any(|rule| rule.id == *id))
    {
        return Err(io::Error::other(Invalid::Definition));
    }
    if !current(&path, &advice)?
        || advice.definition != book.identity().map_err(io::Error::other)?
        || advice.text.chars().count() > room
        || history(&path.with_extension("seen"))?.contains(&advice.key)
    {
        return Ok(None);
    }
    verify_scope(workspace, &book, &advice.edited)?;
    Ok(Some(advice))
}

/// Acknowledge only after a host actually includes the advisory. A newer
/// queued turn is preserved, and retries never duplicate the receipt.
///
/// # Errors
/// Invalid identities, busy storage or an I/O failure.
pub fn delivered(home: &Path, workspace: &Path, recipient: &str, key: &str) -> io::Result<()> {
    if !identity(key) {
        return Err(io::Error::other(Invalid::Definition));
    }
    let root = directory(home, workspace);
    let _lock = lock_recipient(&root, recipient)?;
    let path = advice_path(&root, recipient);
    let seen = path.with_extension("seen");
    let mut ids = history(&seen)?;
    if !ids.iter().any(|id| id == key) {
        ids.push(key.to_string());
        if ids.len() > HISTORY_LIMIT {
            ids.remove(0);
        }
        replace(&seen, &serde_json::to_vec(&ids).map_err(io::Error::other)?)?;
    }
    if read_advice(&path)?.is_some_and(|advice| advice.key == key) {
        fs::remove_file(path)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jev::project_rules::{Check, Rule, Source};

    fn draft() -> Book {
        Book {
            schema_version: 1,
            rules: vec![Rule {
                id: "shared_logic".into(),
                paths: Vec::new(),
                source: Source {
                    path: "AGENTS.md".into(),
                    sha256: String::new(),
                    first_line: 2,
                    last_line: 2,
                    text: "Avoid duplicating existing business logic.".into(),
                },
                check: Check::Model {
                    question: "Is any new business logic duplicated in the supplied code?".into(),
                },
            }],
        }
    }

    fn instructions(root: &Path) {
        fs::create_dir_all(root).unwrap();
        fs::write(
            root.join("AGENTS.md"),
            "# Instructions\nAvoid duplicating existing business logic.\n",
        )
        .unwrap();
    }

    #[test]
    fn an_edit_from_a_subfolder_keeps_its_repository_instruction_scope() {
        let workspace = tempfile::tempdir().unwrap();
        let root = workspace.path().canonicalize().unwrap();
        fs::create_dir(root.join(".git")).unwrap();
        fs::create_dir(root.join("src")).unwrap();
        let cwd = root.join("src");
        assert_eq!(edit_path(&cwd, "lib.rs").unwrap(), "src/lib.rs");
        assert_eq!(
            edit_path(&cwd, root.join("src/lib.rs").to_str().unwrap()).unwrap(),
            "src/lib.rs"
        );
        assert!(edit_path(&cwd, "../../outside.rs").is_err());
    }

    #[test]
    fn a_new_nested_instruction_withholds_an_already_queued_advisory() {
        let home = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        instructions(workspace.path());
        compile(home.path(), workspace.path(), draft()).unwrap();
        let book = load(home.path(), workspace.path()).unwrap().unwrap();
        let edited = vec!["src/lib.rs".to_string()];
        verify_scope(workspace.path(), &book, &edited).unwrap();
        let readings = [crate::jev::project_rules::Reading {
            rule_id: "shared_logic".into(),
            verdict: "violated".into(),
            violation_probability: 1.0,
            advisory: true,
            actionable: true,
        }];
        let advice =
            crate::jev::project_rules::advice(&book, &readings, "changed code", &edited, 1, "one")
                .unwrap()
                .unwrap();
        assert!(begin_check(home.path(), workspace.path(), "session", 1, "one").unwrap());
        assert!(offer(home.path(), workspace.path(), "session", &advice).unwrap());
        assert!(
            pending(home.path(), workspace.path(), "session", 1200)
                .unwrap()
                .is_some()
        );
        fs::create_dir(workspace.path().join("src")).unwrap();
        fs::write(
            workspace.path().join("src/AGENTS.md"),
            "A scoped exception.\n",
        )
        .unwrap();
        assert!(verify_scope(workspace.path(), &book, &edited).is_err());
        assert!(pending(home.path(), workspace.path(), "session", 1200).is_err());
        assert!(offer(home.path(), workspace.path(), "other-session", &advice).is_err());
        // The new instruction cannot poison an unrelated directory.
        verify_scope(workspace.path(), &book, &["tests/check.rs".into()]).unwrap();
        assert!(verify_scope(workspace.path(), &book, &["../outside.rs".into()]).is_err());
    }

    #[test]
    fn compiling_and_rolling_back_keep_immutable_definitions_and_require_current_sources() {
        let home = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        instructions(workspace.path());
        assert_eq!(load(home.path(), workspace.path()).unwrap(), None);
        assert!(
            !home.path().join("jev").exists(),
            "an empty read creates nothing"
        );
        let first = compile(home.path(), workspace.path(), draft()).unwrap();
        let original = load(home.path(), workspace.path()).unwrap().unwrap();
        let mut revised = draft();
        revised.rules[0].check = Check::Model {
            question: "Does the whole turn duplicate an existing implementation?".into(),
        };
        let second = compile(home.path(), workspace.path(), revised).unwrap();
        assert_ne!(first, second);
        activate(home.path(), workspace.path(), &first).unwrap();
        assert_eq!(load(home.path(), workspace.path()).unwrap(), Some(original));
        fs::write(
            workspace.path().join("AGENTS.md"),
            "A changed instruction.\n",
        )
        .unwrap();
        assert!(load(home.path(), workspace.path()).is_err());
        assert!(activate(home.path(), workspace.path(), &second).is_err());
        assert!(
            directory(home.path(), workspace.path())
                .join(format!("{first}.json"))
                .exists()
        );
    }

    #[test]
    fn registered_worktrees_share_definitions_and_a_forged_git_pointer_does_not() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let main = temp.path().join("main");
        let linked = temp.path().join("linked");
        let forged = temp.path().join("forged");
        for root in [&main, &linked, &forged] {
            instructions(root);
        }
        let registration = main.join(".git/worktrees/linked");
        fs::create_dir_all(&registration).unwrap();
        fs::write(registration.join("commondir"), "../..\n").unwrap();
        fs::write(
            registration.join("gitdir"),
            linked.join(".git").to_string_lossy().as_bytes(),
        )
        .unwrap();
        for root in [&linked, &forged] {
            fs::write(
                root.join(".git"),
                format!("gitdir: {}\n", registration.display()),
            )
            .unwrap();
        }
        compile(&home, &main, draft()).unwrap();
        assert!(load(&home, &linked).unwrap().is_some());
        assert_eq!(load(&home, &forged).unwrap(), None);
    }

    #[test]
    fn advice_waits_for_space_is_delivered_once_and_cannot_erase_a_newer_turn() {
        let home = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        instructions(workspace.path());
        compile(home.path(), workspace.path(), draft()).unwrap();
        let book = load(home.path(), workspace.path()).unwrap().unwrap();
        let readings = [crate::jev::project_rules::Reading {
            rule_id: "shared_logic".into(),
            verdict: "violated".into(),
            violation_probability: 1.0,
            advisory: true,
            actionable: true,
        }];
        let first =
            crate::jev::project_rules::advice(&book, &readings, "first diff", &[], 1, "one")
                .unwrap()
                .unwrap();
        let second =
            crate::jev::project_rules::advice(&book, &readings, "second diff", &[], 2, "two")
                .unwrap()
                .unwrap();
        assert!(begin_check(home.path(), workspace.path(), "session", 1, "one").unwrap());
        assert!(offer(home.path(), workspace.path(), "session", &first).unwrap());
        assert!(
            pending(home.path(), workspace.path(), "session", 1)
                .unwrap()
                .is_none()
        );
        assert_eq!(
            pending(home.path(), workspace.path(), "session", 1200).unwrap(),
            Some(first.clone())
        );
        assert!(begin_check(home.path(), workspace.path(), "session", 2, "two").unwrap());
        assert!(offer(home.path(), workspace.path(), "session", &second).unwrap());
        assert!(!offer(home.path(), workspace.path(), "session", &first).unwrap());
        delivered(home.path(), workspace.path(), "session", &first.key).unwrap();
        assert_eq!(
            pending(home.path(), workspace.path(), "session", 1200).unwrap(),
            Some(second.clone())
        );
        delivered(home.path(), workspace.path(), "session", &second.key).unwrap();
        delivered(home.path(), workspace.path(), "session", &second.key).unwrap();
        assert!(
            pending(home.path(), workspace.path(), "session", 1200)
                .unwrap()
                .is_none()
        );
        let mut repeated = second;
        repeated.at = 10;
        repeated.turn = source_hash(b"ten");
        assert!(begin_check(home.path(), workspace.path(), "session", 10, "ten").unwrap());
        assert!(!offer(home.path(), workspace.path(), "session", &repeated).unwrap());
    }

    #[test]
    fn a_newer_clean_check_retires_pending_advice_and_refuses_an_older_late_answer() {
        let home = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        instructions(workspace.path());
        compile(home.path(), workspace.path(), draft()).unwrap();
        let book = load(home.path(), workspace.path()).unwrap().unwrap();
        let readings = [crate::jev::project_rules::Reading {
            rule_id: "shared_logic".into(),
            verdict: "violated".into(),
            violation_probability: 1.0,
            advisory: true,
            actionable: true,
        }];
        let old = crate::jev::project_rules::advice(&book, &readings, "old issue", &[], 1, "old")
            .unwrap()
            .unwrap();
        assert!(begin_check(home.path(), workspace.path(), "session", 1, "old").unwrap());
        assert!(offer(home.path(), workspace.path(), "session", &old).unwrap());
        // The newer clean turn has no advisory to offer. Announcing it alone
        // must clear the previous one and stop its in-flight reply returning.
        assert!(begin_check(home.path(), workspace.path(), "session", 2, "clean").unwrap());
        assert!(
            pending(home.path(), workspace.path(), "session", 1200)
                .unwrap()
                .is_none()
        );
        assert!(!offer(home.path(), workspace.path(), "session", &old).unwrap());
        assert!(!begin_check(home.path(), workspace.path(), "session", 1, "old").unwrap());
        // Equal timestamps do not prove the order of different turns.
        assert!(
            begin_check(
                home.path(),
                workspace.path(),
                "session",
                3,
                "first-at-three"
            )
            .unwrap()
        );
        assert!(
            !begin_check(
                home.path(),
                workspace.path(),
                "session",
                3,
                "second-at-three"
            )
            .unwrap()
        );
        let tied =
            crate::jev::project_rules::advice(&book, &readings, "issue", &[], 3, "first-at-three")
                .unwrap()
                .unwrap();
        assert!(!offer(home.path(), workspace.path(), "session", &tied).unwrap());
    }

    #[test]
    fn one_recipients_write_does_not_lock_another_panes_check() {
        let home = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        instructions(workspace.path());
        compile(home.path(), workspace.path(), draft()).unwrap();
        let root = directory(home.path(), workspace.path());
        let _held = lock_recipient(&root, "one").unwrap();
        assert!(begin_check(home.path(), workspace.path(), "two", 1, "turn").unwrap());
        assert!(begin_check(home.path(), workspace.path(), "one", 1, "turn").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn a_source_link_outside_the_workspace_cannot_be_compiled() {
        let home = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let external = tempfile::tempdir().unwrap();
        instructions(external.path());
        std::os::unix::fs::symlink(
            external.path().join("AGENTS.md"),
            workspace.path().join("AGENTS.md"),
        )
        .unwrap();
        assert!(compile(home.path(), workspace.path(), draft()).is_err());
        assert!(!home.path().join("jev").exists());
    }
}
