use std::collections::BTreeSet;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use zerocode_core::orchestration::gate_reuse::LibraryIdentity;

pub(super) fn pth_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push("._pth");
    PathBuf::from(name)
}

pub(super) fn verify(
    python: &Path,
    runtime: &Path,
    libraries: &[LibraryIdentity],
) -> Result<Vec<PathBuf>, ()> {
    if !runtime.is_dir() || python.parent() != Some(runtime.join("bin").as_path()) {
        return Err(());
    }
    let approved = pth_path(python);
    if super::canonical(approved.to_str().ok_or(())?)? != approved
        || !fs::symlink_metadata(&approved).map_err(|_| ())?.is_file()
    {
        return Err(());
    }
    let mut bytes = Vec::new();
    fs::File::open(&approved)
        .map_err(|_| ())?
        .take(16 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ())?;
    if bytes.len() > 16 * 1024 {
        return Err(());
    }
    let text = std::str::from_utf8(&bytes).map_err(|_| ())?;
    let lines = text
        .strip_suffix('\n')
        .ok_or(())?
        .split('\n')
        .collect::<Vec<_>>();
    if lines.len() != 2
        || lines.iter().any(|line| {
            line.is_empty()
                || line.trim() != *line
                || line.chars().any(|value| {
                    value.is_control() || matches!(value, '#' | '\u{2028}' | '\u{2029}')
                })
        })
    {
        return Err(());
    }
    let paths = lines
        .iter()
        .map(|line| super::canonical(line))
        .collect::<Result<Vec<_>, _>>()?;
    let stdlib = &paths[0];
    let version = stdlib
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_prefix("python"))
        .and_then(|version| version.split_once('.'))
        .ok_or(())?;
    if [version.0, version.1]
        .iter()
        .any(|part| part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()))
        || stdlib.parent() != Some(runtime.join("lib").as_path())
        || paths[1] != stdlib.join("lib-dynload")
        || paths.iter().any(|path| !path.is_dir())
        || !fs::symlink_metadata(stdlib.join("os.py"))
            .map_err(|_| ())?
            .is_file()
        || !fs::symlink_metadata(stdlib.join("encodings/__init__.py"))
            .map_err(|_| ())?
            .is_file()
    {
        return Err(());
    }
    let mut redirects = BTreeSet::from([runtime.join("pyvenv.cfg")]);
    if let Some(parent) = runtime.parent() {
        redirects.insert(parent.join("pyvenv.cfg"));
    }
    for path in std::iter::once(python).chain(
        libraries
            .iter()
            .flat_map(|library| [Path::new(&library.path), Path::new(&library.resolved_path)]),
    ) {
        redirects.insert(pth_path(path));
        redirects.insert(path.with_extension("_pth"));
        let parent = path.parent().ok_or(())?;
        redirects.insert(parent.join("python._pth"));
        redirects.insert(parent.join("pyvenv.cfg"));
        if let Some(directory) = parent.parent() {
            redirects.insert(directory.join("pyvenv.cfg"));
        }
    }
    for (index, entry) in fs::read_dir(python.parent().ok_or(())?)
        .map_err(|_| ())?
        .enumerate()
    {
        if index >= super::ENTRY_CAP {
            return Err(());
        }
        let path = entry.map_err(|_| ())?.path();
        if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.ends_with("._pth"))
        {
            redirects.insert(path);
        }
    }
    redirects.remove(&approved);
    for path in redirects {
        match fs::symlink_metadata(path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            _ => return Err(()),
        }
    }
    Ok(paths)
}
