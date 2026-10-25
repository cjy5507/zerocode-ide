use std::collections::BTreeMap;
use std::fs::{self, Metadata};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

use serde_json::{Value, json};
use zerocode_core::orchestration::gate_reuse::{
    BootstrapIdentity, FileIdentity, JobIntent, job_lifetime_ms,
};
use zerocode_core::user_preferences::content_key;

// The private descriptor road; its one caller is unix-only, as the
// descriptor is.
#[cfg(unix)]
mod gate_channel;
mod runtime_libraries;
mod startup_layout;

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
        if (!FLAGS.contains(&pair[0].as_str()) && pair[0] != "--gate-job")
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

fn execute(args: &[String]) -> Result<ExitCode, ()> {
    let wall_started = std::time::Instant::now();
    let (options, forwarded) = parse(args)?;
    if cfg!(not(unix)) && options.contains_key("--gate-job") {
        println!(
            "{}",
            json!({"decision":"miss","code":"unsupported_platform","executed":false,"newExecutions":0})
        );
        return Ok(ExitCode::FAILURE);
    }
    let launcher = canonical(options["--launcher"])?;
    let python = canonical(options["--python"])?;
    let runtime = canonical(options["--stdlib"])?;
    startup_layout::verify(&python, &runtime, &[])?;
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
    let libraries = match runtime_libraries::read_and_verify(&python, &runtime) {
        Ok(value) => value,
        Err(code) => {
            println!(
                "{}",
                json!({"decision":"miss","code":code,"executed":false,"newExecutions":0})
            );
            return Ok(ExitCode::FAILURE);
        }
    };
    startup_layout::verify(&python, &runtime, &libraries)?;
    let prehash_ms = wall_started.elapsed().as_millis();
    let open_ms;
    let mut command = Command::new(&python);
    command.env_clear();
    for key in ["PATH", "HOME", "SystemRoot", "TMPDIR", "TMP", "TEMP"] {
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
        .stdin(Stdio::piped());
    let mut child = if let Some(job) = options.get("--gate-job") {
        let arguments = match launcher_arguments(forwarded) {
            Ok(value) => value,
            Err(code) => {
                println!(
                    "{}",
                    json!({"decision":"miss","code":code,"executed":false,"newExecutions":0})
                );
                return Ok(ExitCode::FAILURE);
            }
        };
        #[cfg(unix)]
        let opening = std::time::Instant::now();
        let identity = BootstrapIdentity {
            launcher: FileIdentity {
                path: launcher.to_str().ok_or(())?.into(),
                sha256: options["--launcher-sha256"].into(),
            },
            python: FileIdentity {
                path: python.to_str().ok_or(())?.into(),
                sha256: options["--python-sha256"].into(),
            },
            stdlib: FileIdentity {
                path: runtime.to_str().ok_or(())?.into(),
                sha256: options["--stdlib-sha256"].into(),
            },
            libraries,
        };
        let channel = match open_authority(job, &arguments, &identity) {
            Ok(value) => value,
            Err(code) => {
                println!(
                    "{}",
                    json!({"decision":"miss","code":code,"executed":false,"newExecutions":0})
                );
                return Ok(ExitCode::FAILURE);
            }
        };
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            open_ms = opening.elapsed().as_millis();
            command
                .arg("--authority-fd")
                .arg(channel.as_raw_fd().to_string())
                .arg("--bound-policy-sha256")
                .arg(arguments.policy_sha)
                .args(forwarded);
            gate_channel::spawn(command, channel).map_err(|_| ())?
        }
        #[cfg(not(unix))]
        {
            drop(channel);
            return Err(());
        }
    } else {
        open_ms = 0;
        command.args(forwarded).spawn().map_err(|_| ())?
    };
    let sent = child.stdin.take().ok_or(())?.write_all(&retained);
    if sent.is_err() {
        let _ = child.kill();
        let _ = child.wait();
        return Err(());
    }
    let status = child.wait().map_err(|_| ())?;
    if std::env::var("ZEROCODE_GATE_TIMING").as_deref() == Ok("1") {
        eprintln!(
            "{}",
            json!({"nativeTiming":{"preHashMs":prehash_ms,"openMs":open_ms,"totalMs":wall_started.elapsed().as_millis()}})
        );
    }
    Ok(status
        .code()
        .and_then(|code| u8::try_from(code).ok())
        .map_or(ExitCode::FAILURE, ExitCode::from))
}

struct LauncherArguments<'a> {
    policy: &'a str,
    policy_sha: &'a str,
}

fn launcher_arguments(words: &[String]) -> Result<LauncherArguments<'_>, &'static str> {
    let end = words
        .iter()
        .position(|word| word == "--")
        .ok_or("invalid_request")?;
    if end != 6 || end + 1 == words.len() {
        return Err("invalid_request");
    }
    let mut options = BTreeMap::new();
    for pair in words[..end].chunks_exact(2) {
        if !["--policy", "--policy-sha256", "--repo"].contains(&pair[0].as_str())
            || pair[1].is_empty()
            || pair[1].starts_with("--")
            || options.insert(pair[0].as_str(), pair[1].as_str()).is_some()
        {
            return Err("invalid_request");
        }
    }
    Ok(LauncherArguments {
        policy: options["--policy"],
        policy_sha: options["--policy-sha256"],
    })
}

fn capability(name: &str, file_name: &str) -> Result<String, &'static str> {
    let value = if let Ok(value) = std::env::var(name) {
        value
    } else {
        let file = std::env::var(file_name).map_err(|_| "authority_unavailable")?;
        let mut bytes = Vec::new();
        fs::File::open(file)
            .map_err(|_| "authority_unavailable")?
            .take(4097)
            .read_to_end(&mut bytes)
            .map_err(|_| "authority_unavailable")?;
        String::from_utf8(bytes).map_err(|_| "authority_unavailable")?
    };
    let value = value.trim();
    if value.is_empty()
        || value.len() > 4096
        || value
            .bytes()
            .any(|byte| byte == b'\r' || byte == b'\n' || byte == 0)
    {
        return Err("authority_unavailable");
    }
    Ok(value.into())
}

fn open_authority(
    job: &str,
    arguments: &LauncherArguments<'_>,
    bootstrap: &BootstrapIdentity,
) -> Result<std::net::TcpStream, &'static str> {
    use std::net::{SocketAddr, TcpStream};
    use std::time::Duration;
    if job.len() > 16 * 1024 {
        return Err("invalid_request");
    }
    let intent: JobIntent = serde_json::from_str(job).map_err(|error| {
        if error.to_string().contains("unknown field") {
            "unknown_field"
        } else {
            "invalid_request"
        }
    })?;
    let path = arguments.policy;
    let expected = arguments.policy_sha;
    let mut manifest = Vec::new();
    fs::File::open(path)
        .map_err(|_| "identity_unavailable")?
        .take(1024 * 1024 + 1)
        .read_to_end(&mut manifest)
        .map_err(|_| "identity_unavailable")?;
    if manifest.len() > 1024 * 1024 || content_key(&manifest) != expected {
        return Err("digest_mismatch");
    }
    let manifest = String::from_utf8(manifest).map_err(|_| "invalid_request")?;
    let declaration: Value = serde_json::from_str(&manifest).map_err(|_| "invalid_request")?;
    if declaration["version"] != 3
        || BootstrapIdentity::from_manifest(&declaration).map_err(|_| "digest_mismatch")?
            != *bootstrap
    {
        return Err("digest_mismatch");
    }
    job_lifetime_ms(&declaration["gates"][&intent.gate], &intent.mode)
        .map_err(|_| "invalid_request")?;
    let port = std::env::var("ZEROCODE_HOOK_PORT")
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
        .filter(|value| *value != 0)
        .ok_or("authority_unavailable")?;
    let token = capability("ZEROCODE_HOOK_TOKEN", "ZEROCODE_HOOK_TOKEN_FILE")?;
    let pane_token = capability(
        "ZEROCODE_AGENT_TEAM_TOKEN",
        "ZEROCODE_AGENT_TEAM_TOKEN_FILE",
    )?;
    let team = capability("ZEROCODE_AGENT_TEAM_ID", "ZEROCODE_AGENT_TEAM_ID_FILE")?;
    let pane = capability("ZEROCODE_AGENT_TEAM_PANE", "ZEROCODE_AGENT_TEAM_PANE_FILE")?;
    let body =
        serde_json::to_vec(&json!({"intent":intent,"manifest":manifest,"bootstrap":bootstrap}))
            .map_err(|_| "invalid_request")?;
    let address = SocketAddr::from(([127, 0, 0, 1], port));
    let mut control = TcpStream::connect_timeout(&address, Duration::from_secs(3))
        .map_err(|_| "authority_unavailable")?;
    control
        .set_read_timeout(Some(Duration::from_secs(10)))
        .map_err(|_| "authority_unavailable")?;
    control
        .set_write_timeout(Some(Duration::from_secs(10)))
        .map_err(|_| "authority_unavailable")?;
    write!(control, "POST /gate-sessions/open HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\nx-zerocode-hook-token: {token}\r\nx-zerocode-team-id: {team}\r\nx-zerocode-team-pane: {pane}\r\nx-zerocode-team-token: {pane_token}\r\n\r\n", body.len()).map_err(|_| "authority_unavailable")?;
    control
        .write_all(&body)
        .map_err(|_| "authority_unavailable")?;
    let mut header = Vec::new();
    while !header.ends_with(b"\r\n\r\n") {
        if header.len() >= 8192 {
            return Err("invalid_request");
        }
        let mut byte = [0];
        control
            .read_exact(&mut byte)
            .map_err(|_| "authority_unavailable")?;
        header.push(byte[0]);
    }
    let header = std::str::from_utf8(&header).map_err(|_| "invalid_request")?;
    let length = header
        .lines()
        .filter_map(|line| line.split_once(':'))
        .find(|(key, _)| key.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.trim().parse::<usize>().ok())
        .filter(|value| *value <= 16 * 1024)
        .ok_or("invalid_request")?;
    let mut result = vec![0; length];
    control
        .read_exact(&mut result)
        .map_err(|_| "authority_unavailable")?;
    if header
        .lines()
        .next()
        .is_none_or(|line| line.split_whitespace().nth(1) != Some("200"))
    {
        let code = std::str::from_utf8(&result)
            .unwrap_or_default()
            .split(':')
            .next()
            .unwrap_or_default();
        return Err(match code {
            "unpinned" => "unpinned",
            "digest_mismatch" => "digest_mismatch",
            "stale_pin" => "stale_pin",
            "unknown_field" => "unknown_field",
            "expired" => "expired",
            "out_of_scope" => "out_of_scope",
            _ => "authority_unavailable",
        });
    }
    let result: Value = serde_json::from_slice(&result).map_err(|_| "invalid_request")?;
    let port = result["port"]
        .as_u64()
        .and_then(|value| u16::try_from(value).ok())
        .filter(|value| *value != 0)
        .ok_or("invalid_request")?;
    let mut channel = TcpStream::connect_timeout(
        &SocketAddr::from(([127, 0, 0, 1], port)),
        Duration::from_secs(3),
    )
    .map_err(|_| "authority_unavailable")?;
    channel
        .set_read_timeout(Some(Duration::from_secs(10)))
        .map_err(|_| "authority_unavailable")?;
    channel
        .set_write_timeout(Some(Duration::from_secs(10)))
        .map_err(|_| "authority_unavailable")?;
    let authentication =
        serde_json::to_vec(&json!({"capability":pane_token})).map_err(|_| "invalid_request")?;
    channel
        .write_all(&(authentication.len() as u32).to_be_bytes())
        .and_then(|_| channel.write_all(&authentication))
        .map_err(|_| "authority_unavailable")?;
    let mut length = [0; 4];
    channel
        .read_exact(&mut length)
        .map_err(|_| "authority_unavailable")?;
    let length = u32::from_be_bytes(length) as usize;
    if length > 32 * 1024 {
        return Err("invalid_request");
    }
    let mut bound = vec![0; length];
    channel
        .read_exact(&mut bound)
        .map_err(|_| "authority_unavailable")?;
    let bound: Value = serde_json::from_slice(&bound).map_err(|_| "invalid_request")?;
    if bound["ok"] != true {
        return Err("authority_unavailable");
    }
    if bound["value"]["policy"].as_str() != Some(expected) {
        return Err("digest_mismatch");
    }
    Ok(channel)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn an_unsupported_runtime_is_refused_before_any_image_is_loaded() {
        assert_eq!(
            runtime_libraries::verify_closure(
                Path::new("absent-python"),
                Path::new("absent-runtime"),
                &[]
            ),
            Err("unsupported_runtime")
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn bootstrap_library_closure_rejects_missing_transitive_and_unresolved_loads() {
        use zerocode_core::orchestration::gate_reuse::LibraryIdentity;
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().canonicalize().unwrap();
        let root = directory.join("runtime");
        let external = directory.join("libraries");
        fs::create_dir(&root).unwrap();
        fs::create_dir(&external).unwrap();
        let write_image = |path: &Path, dependency: &str| {
            let length = (24 + dependency.len() + 1).div_ceil(8) * 8;
            let mut bytes = Vec::new();
            for word in [
                0xfeed_facf_u32,
                0x0100_000c,
                0,
                8,
                1,
                length as u32,
                0,
                0,
                12,
                length as u32,
                24,
                0,
                0,
                0,
            ] {
                bytes.extend_from_slice(&word.to_le_bytes());
            }
            bytes.extend_from_slice(dependency.as_bytes());
            bytes.resize(32 + length, 0);
            fs::write(path, bytes).unwrap();
        };
        let image = root.join("interpreter");
        let first = external.join("library-a");
        let second = external.join("library-b");
        write_image(&second, "/usr/lib/libSystem.B.dylib");
        write_image(&first, second.to_str().unwrap());
        write_image(&image, first.to_str().unwrap());
        let entry = |path: &Path| LibraryIdentity {
            path: path.to_str().unwrap().into(),
            resolved_path: path.to_str().unwrap().into(),
            sha256: file_hash(path).unwrap(),
        };
        let libraries = vec![entry(&first), entry(&second)];
        assert!(runtime_libraries::verify_entries(&libraries).is_ok());
        assert!(runtime_libraries::verify_closure(&image, &root, &libraries).is_ok());
        assert_eq!(
            runtime_libraries::verify_closure(&image, &root, &libraries[..1]),
            Err("unknown_runtime_dependency")
        );
        write_image(&image, "@rpath/library-a");
        assert_eq!(
            runtime_libraries::verify_closure(&image, &root, &libraries),
            Err("unknown_runtime_dependency")
        );
    }

    #[cfg(unix)]
    #[test]
    fn bootstrap_runtime_libraries_bind_loader_targets_bytes_and_limits() {
        use zerocode_core::orchestration::gate_reuse::LibraryIdentity;
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let target = root.join("library-a");
        let other = root.join("library-b");
        let loader = root.join("loader-name");
        fs::write(&target, b"fixture library bytes").unwrap();
        fs::write(&other, b"other fixture library bytes").unwrap();
        std::os::unix::fs::symlink(&target, &loader).unwrap();
        let entry = LibraryIdentity {
            path: loader.to_str().unwrap().into(),
            resolved_path: target.to_str().unwrap().into(),
            sha256: file_hash(&target).unwrap(),
        };
        let alias = root.join("other-loader-name");
        std::os::unix::fs::symlink(&target, &alias).unwrap();
        let other_name = LibraryIdentity {
            path: alias.to_str().unwrap().into(),
            ..entry.clone()
        };
        assert!(runtime_libraries::verify_entries(&[entry.clone(), other_name.clone()]).is_ok());
        let conflicting = LibraryIdentity {
            sha256: "b".repeat(64),
            ..other_name
        };
        assert_eq!(
            runtime_libraries::verify_entries(&[entry.clone(), conflicting]),
            Err("digest_mismatch")
        );
        assert!(runtime_libraries::verify_entries(std::slice::from_ref(&entry)).is_ok());
        fs::write(&target, b"changed fixture library bytes").unwrap();
        assert_eq!(
            runtime_libraries::verify_entries(std::slice::from_ref(&entry)),
            Err("digest_mismatch")
        );
        fs::write(&target, b"fixture library bytes").unwrap();
        fs::remove_file(&loader).unwrap();
        std::os::unix::fs::symlink(&other, &loader).unwrap();
        assert_eq!(
            runtime_libraries::verify_entries(std::slice::from_ref(&entry)),
            Err("digest_mismatch")
        );
        fs::remove_file(&loader).unwrap();
        std::os::unix::fs::symlink(&target, &loader).unwrap();
        assert_eq!(
            runtime_libraries::verify_entries(&[entry.clone(), entry.clone()]),
            Err("digest_mismatch")
        );
        assert_eq!(
            runtime_libraries::verify_entries(&vec![entry.clone(); 17]),
            Err("digest_mismatch")
        );
        let many = (0..17)
            .map(|index| {
                let path = root.join(format!("count-loader-{index}"));
                std::os::unix::fs::symlink(&target, &path).unwrap();
                LibraryIdentity {
                    path: path.to_str().unwrap().into(),
                    ..entry.clone()
                }
            })
            .collect::<Vec<_>>();
        assert_eq!(
            runtime_libraries::verify_entries(&many),
            Err("digest_mismatch")
        );
        let shared = root.join("shared-budget-library");
        fs::File::create(&shared)
            .unwrap()
            .set_len(90 * 1024 * 1024)
            .unwrap();
        let digest = file_hash(&shared).unwrap();
        let shared_names = (0..3)
            .map(|index| {
                let path = root.join(format!("budget-loader-{index}"));
                std::os::unix::fs::symlink(&shared, &path).unwrap();
                LibraryIdentity {
                    path: path.to_str().unwrap().into(),
                    resolved_path: shared.to_str().unwrap().into(),
                    sha256: digest.clone(),
                }
            })
            .collect::<Vec<_>>();
        assert!(runtime_libraries::verify_entries(&shared_names).is_ok());
        let directory = LibraryIdentity {
            path: root.to_str().unwrap().into(),
            resolved_path: root.to_str().unwrap().into(),
            sha256: "b".repeat(64),
        };
        assert_eq!(
            runtime_libraries::verify_entries(&[directory]),
            Err("digest_mismatch")
        );
        let sparse = root.join("oversized-library");
        fs::File::create(&sparse)
            .unwrap()
            .set_len(256 * 1024 * 1024 + 1)
            .unwrap();
        let oversized = LibraryIdentity {
            path: sparse.to_str().unwrap().into(),
            resolved_path: sparse.to_str().unwrap().into(),
            sha256: "b".repeat(64),
        };
        assert_eq!(
            runtime_libraries::verify_entries(&[oversized]),
            Err("invalid_request")
        );
        assert!(runtime_libraries::verify_entries(&[entry]).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn bootstrap_startup_file_binds_exact_paths_and_rejects_prior_redirects() {
        use zerocode_core::orchestration::gate_reuse::LibraryIdentity;
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().canonicalize().unwrap();
        let runtime = directory.join("runtime");
        let python = runtime.join("bin/python3.14");
        let stdlib = runtime.join("lib/python3.14");
        let dynload = stdlib.join("lib-dynload");
        fs::create_dir_all(python.parent().unwrap()).unwrap();
        fs::create_dir_all(&dynload).unwrap();
        fs::create_dir(stdlib.join("encodings")).unwrap();
        fs::write(&python, b"fixture interpreter bytes").unwrap();
        fs::write(stdlib.join("os.py"), b"").unwrap();
        fs::write(stdlib.join("encodings/__init__.py"), b"").unwrap();
        let approved = startup_layout::pth_path(&python);
        let text = format!("{}\n{}\n", stdlib.display(), dynload.display());
        fs::write(&approved, &text).unwrap();
        assert_eq!(
            startup_layout::verify(&python, &runtime, &[]).unwrap(),
            vec![stdlib.clone(), dynload.clone()]
        );
        for invalid in [
            String::new(),
            format!("{text}\n"),
            format!("{text}import site\n"),
            format!("{text}import os\n"),
            format!("{}#comment\n{}\n", stdlib.display(), dynload.display()),
            format!("../lib/python3.14\n{}\n", dynload.display()),
            format!("{}\n{}\n", directory.display(), dynload.display()),
            format!("{}\n{}\n", stdlib.display(), directory.display()),
        ] {
            fs::write(&approved, invalid).unwrap();
            assert!(startup_layout::verify(&python, &runtime, &[]).is_err());
        }
        fs::write(&approved, &text).unwrap();
        let legacy = python.with_extension("_pth");
        fs::rename(&approved, &legacy).unwrap();
        assert!(startup_layout::verify(&python, &runtime, &[]).is_err());
        fs::rename(&legacy, &approved).unwrap();
        let external = directory.join("libraries");
        fs::create_dir(&external).unwrap();
        let target = external.join("runtime-library");
        let alias = external.join("runtime-alias");
        fs::write(&target, b"fixture library bytes").unwrap();
        std::os::unix::fs::symlink(&target, &alias).unwrap();
        let libraries = vec![LibraryIdentity {
            path: alias.to_str().unwrap().into(),
            resolved_path: target.to_str().unwrap().into(),
            sha256: file_hash(&target).unwrap(),
        }];
        assert!(startup_layout::verify(&python, &runtime, &libraries).is_ok());
        for prior in [
            startup_layout::pth_path(&alias),
            startup_layout::pth_path(&target),
            external.join("pyvenv.cfg"),
            python.parent().unwrap().join("python._pth"),
            runtime.parent().unwrap().join("pyvenv.cfg"),
        ] {
            fs::write(&prior, &text).unwrap();
            assert!(startup_layout::verify(&python, &runtime, &libraries).is_err());
            fs::remove_file(&prior).unwrap();
        }
        assert!(startup_layout::verify(&python, &runtime, &libraries).is_ok());
    }

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
        let good = vec![
            "--policy".into(),
            "/fixture/policy".into(),
            "--policy-sha256".into(),
            "b".repeat(64),
            "--repo".into(),
            "/fixture/repo".into(),
            "--".into(),
            "run".into(),
        ];
        assert!(launcher_arguments(&good).is_ok());
        for key in [
            "--pol",
            "--policy-sha256=value",
            "--bootstrap-launcher",
            "--authority-fd",
            "--bound-policy-sha256",
        ] {
            let mut bad = good.clone();
            bad[0] = key.into();
            assert!(launcher_arguments(&bad).is_err());
        }
        let mut duplicate = good.clone();
        duplicate.splice(6..6, ["--policy".into(), "/fixture/other".into()]);
        assert!(launcher_arguments(&duplicate).is_err());
        let mut disguised = good;
        disguised[1] = "--policy-sha256".into();
        assert!(launcher_arguments(&disguised).is_err());
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
