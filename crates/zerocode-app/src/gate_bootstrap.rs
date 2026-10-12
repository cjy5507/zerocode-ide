use std::collections::BTreeMap;
use std::fs::{self, Metadata};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

use serde_json::{Value, json};
use zerocode_core::user_preferences::content_key;

const LAUNCHER_CAP: u64 = 8 * 1024 * 1024;
const FILE_CAP: usize = 128 * 1024 * 1024;
const ENTRY_CAP: usize = 50_000;
const TREE_CAP: u64 = 2 * 1024 * 1024 * 1024;
const FLAGS: [&str; 6] = [
    "--launcher",
    "--launcher-sha256",
    "--python",
    "--python-sha256",
    "--stdlib",
    "--stdlib-sha256",
];

pub(crate) fn run(args: &[String]) -> ExitCode {
    match execute(args) {
        Ok(code) => code,
        Err(()) => {
            println!(
                "{}",
                json!({"decision":"miss","reason":"native bootstrap identity unavailable",
                    "executed":false,"newExecutions":0})
            );
            ExitCode::FAILURE
        }
    }
}

fn parse(args: &[String]) -> Result<(BTreeMap<&str, &str>, &[String]), ()> {
    let split = args.iter().position(|word| word == "--").ok_or(())?;
    if split % 2 != 0 || split + 1 == args.len() {
        return Err(());
    }
    let mut options = BTreeMap::new();
    for pair in args[..split].chunks_exact(2) {
        if !FLAGS.contains(&pair[0].as_str())
            || options.insert(pair[0].as_str(), pair[1].as_str()).is_some()
        {
            return Err(());
        }
    }
    if FLAGS.iter().any(|flag| !options.contains_key(flag)) {
        return Err(());
    }
    for flag in ["--launcher-sha256", "--python-sha256", "--stdlib-sha256"] {
        let value = options[flag];
        if value.len() != 64
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(());
        }
    }
    Ok((options, &args[split + 1..]))
}

fn canonical(path: &str) -> Result<PathBuf, ()> {
    let named = Path::new(path);
    let resolved = named.canonicalize().map_err(|_| ())?;
    if !named.is_absolute() || named != resolved {
        return Err(());
    }
    for parent in named.ancestors() {
        if fs::symlink_metadata(parent)
            .map_err(|_| ())?
            .file_type()
            .is_symlink()
        {
            return Err(());
        }
    }
    Ok(resolved)
}

fn stamp(value: &Metadata) -> (u64, Option<std::time::SystemTime>, bool, u32) {
    (
        value.len(),
        value.modified().ok(),
        value.is_file(),
        mode(value),
    )
}

#[cfg(unix)]
fn mode(value: &Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    value.permissions().mode() & 0o7777
}

#[cfg(not(unix))]
fn mode(value: &Metadata) -> u32 {
    u32::from(value.permissions().readonly())
}

fn file_hash(path: &Path) -> Result<String, ()> {
    let before = fs::symlink_metadata(path).map_err(|_| ())?;
    if !before.is_file() {
        return Err(());
    }
    let digest = zerocode_core::skill::file_digest(path, FILE_CAP).ok_or(())?;
    if stamp(&before) != stamp(&fs::symlink_metadata(path).map_err(|_| ())?) {
        return Err(());
    }
    Ok(digest)
}

fn tree_hash(root: &Path) -> Result<String, ()> {
    let mut rows = Vec::<Value>::new();
    let mut pending = vec![root.to_path_buf()];
    let mut bytes = 0_u64;
    while let Some(path) = pending.pop() {
        if rows.len() >= ENTRY_CAP {
            return Err(());
        }
        let meta = fs::symlink_metadata(&path).map_err(|_| ())?;
        let relative = path
            .strip_prefix(root)
            .map_err(|_| ())?
            .to_str()
            .ok_or(())?;
        let relative = if relative.is_empty() { "." } else { relative };
        if meta.is_dir() {
            rows.push(json!([relative, "directory", mode(&meta)]));
            let mut children = fs::read_dir(&path)
                .map_err(|_| ())?
                .map(|entry| entry.map(|entry| entry.path()))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| ())?;
            children.sort();
            pending.extend(children.into_iter().rev());
        } else if meta.file_type().is_symlink() {
            let target = path.canonicalize().map_err(|_| ())?;
            if !target.starts_with(root) {
                return Err(());
            }
            rows.push(json!([
                relative,
                "link",
                mode(&meta),
                fs::read_link(&path).map_err(|_| ())?.to_str().ok_or(())?
            ]));
        } else if meta.is_file() {
            bytes = bytes.checked_add(meta.len()).ok_or(())?;
            if bytes > TREE_CAP {
                return Err(());
            }
            rows.push(json!([relative, "file", mode(&meta), file_hash(&path)?]));
        } else {
            return Err(());
        }
    }
    Ok(content_key(&serde_json::to_vec(&rows).map_err(|_| ())?))
}

fn startup_layout(python: &Path, runtime: &Path) -> Result<(), ()> {
    if !runtime.is_dir() || !python.starts_with(runtime) {
        return Err(());
    }
    let directory = python.parent().ok_or(())?;
    let mut redirects = vec![directory.join("pyvenv.cfg"), directory.join("python._pth")];
    redirects.push(python.with_extension("_pth"));
    for parent in [directory.parent(), runtime.parent(), Some(runtime)]
        .into_iter()
        .flatten()
    {
        redirects.push(parent.join("pyvenv.cfg"));
    }
    for path in redirects {
        match fs::symlink_metadata(path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            _ => return Err(()),
        }
    }
    Ok(())
}

fn execute(args: &[String]) -> Result<ExitCode, ()> {
    let (options, forwarded) = parse(args)?;
    let launcher = canonical(options["--launcher"])?;
    let python = canonical(options["--python"])?;
    let runtime = canonical(options["--stdlib"])?;
    startup_layout(&python, &runtime)?;
    let mut file = fs::File::open(&launcher).map_err(|_| ())?;
    let before = file.metadata().map_err(|_| ())?;
    if !before.is_file() || before.len() > LAUNCHER_CAP {
        return Err(());
    }
    let mut retained = Vec::new();
    Read::by_ref(&mut file)
        .take(LAUNCHER_CAP + 1)
        .read_to_end(&mut retained)
        .map_err(|_| ())?;
    if retained.len() as u64 > LAUNCHER_CAP
        || stamp(&before) != stamp(&file.metadata().map_err(|_| ())?)
        || content_key(&retained) != options["--launcher-sha256"]
        || file_hash(&python)? != options["--python-sha256"]
        || tree_hash(&runtime)? != options["--stdlib-sha256"]
        || file_hash(&python)? != options["--python-sha256"]
    {
        return Err(());
    }
    let mut command = Command::new(&python);
    command.env_clear();
    for key in [
        "PATH",
        "HOME",
        "SystemRoot",
        "TMPDIR",
        "TMP",
        "TEMP",
        "ZEROCODE_AGENT_TEAM_ID",
        "ZEROCODE_AGENT_TEAM_PANE",
        "ZEROCODE_AGENT_TEAM_TOKEN",
        "ZEROCODE_AGENT_TEAM_TOKEN_FILE",
        "ZEROCODE_ORCHESTRATION_RUN",
        "ZEROCODE_HOOK_PORT",
        "ZEROCODE_HOOK_TOKEN",
    ] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    command
        .env("LANG", "C")
        .env("LC_ALL", "C")
        .env("TZ", "UTC")
        .args(["-I", "-S", "-B", "-"])
        .arg("--bootstrap-launcher")
        .arg(&launcher)
        .args(forwarded)
        .stdin(Stdio::piped());
    let mut child = command.spawn().map_err(|_| ())?;
    let sent = child.stdin.take().ok_or(())?.write_all(&retained);
    if sent.is_err() {
        let _ = child.kill();
        let _ = child.wait();
        return Err(());
    }
    let status = child.wait().map_err(|_| ())?;
    Ok(status
        .code()
        .and_then(|code| u8::try_from(code).ok())
        .map_or(ExitCode::FAILURE, ExitCode::from))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bootstrap_rejects_missing_duplicate_unknown_and_partial_options() {
        for words in [
            vec![],
            vec!["--"],
            vec!["--launcher", "file", "--launcher", "other", "--", "run"],
            vec!["--unexpected", "file", "--", "run"],
        ] {
            let words = words.into_iter().map(str::to_string).collect::<Vec<_>>();
            assert!(parse(&words).is_err());
        }
    }

    #[test]
    fn bootstrap_tree_identity_reacts_to_standard_library_bytes_and_modes() {
        let root = tempfile::tempdir().unwrap();
        let module = root.path().join("module.py");
        fs::write(&module, "VALUE = 1\n").unwrap();
        let before = tree_hash(root.path()).unwrap();
        fs::write(&module, "VALUE = 2\n").unwrap();
        assert_ne!(tree_hash(root.path()).unwrap(), before);
        fs::write(&module, "VALUE = 1\n").unwrap();
        assert_eq!(tree_hash(root.path()).unwrap(), before);
        let mut permissions = fs::metadata(&module).unwrap().permissions();
        permissions.set_readonly(true);
        fs::set_permissions(&module, permissions).unwrap();
        assert_ne!(tree_hash(root.path()).unwrap(), before);
    }
}
