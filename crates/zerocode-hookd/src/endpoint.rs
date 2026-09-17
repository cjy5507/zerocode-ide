//! The endpoint file: how a hook finds a bridge that moved.
//!
//! The PTY hands every agent the bridge's coordinates at launch — and then the
//! app restarts, and every agent still running holds a port that now belongs to
//! nobody. The agents outlive us on purpose (a `zo` session is the whole point),
//! so the coordinates cannot live only in process environments that were copied
//! once. The token itself stays in this private file; the PTY carries only its
//! path whenever the file can be written.
//!
//! Orca's answer (1.4.169, `writeEndpointFile` out/main/index.js:10756-10800)
//! is a file: the current port and token, at a fixed path, handed to agents as
//! `ORCA_AGENT_HOOK_ENDPOINT`. Every generated script *sources* that file
//! before reading its environment, so the file's values override the stale
//! ones — a hook from an agent launched three app-restarts ago still lands on
//! the bridge that is listening NOW. Ours is the same mechanism under our own
//! names.
//!
//! Because the file is sourced by `/bin/sh`, every value in it is a command
//! injection waiting for a character that ends a word. Orca refuses to write
//! the file at all when a value fails its shell-safety test and falls back to
//! the PTY env; so does this. The check is an allowlist, not an escape —
//! quoting can be gotten wrong, refusing cannot.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// The file's name. Orca uses `endpoint.cmd` on Windows (`set K=V` lines);
/// this app ships on Unix and writes the POSIX form.
pub const ENDPOINT_FILE: &str = "endpoint.env";
/// Names of the private capability files beside the endpoint file. They are
/// deliberately values rather than paths: the caller owns the injected root.
pub const BROWSER_TOKEN_FILE: &str = "browser-token";
pub const COMPUTER_TOKEN_FILE: &str = "computer-token";

/// What the file carries. The same four facts the PTY env carries — the file
/// exists to out-live the PTY env, not to say more than it.
#[derive(Clone)]
pub struct EndpointFields {
    pub port: u16,
    pub token: String,
    /// Which app instance these coordinates belong to (`production`, `dev`) —
    /// so a dev build and a real one running side by side do not steal each
    /// other's hooks.
    pub env: String,
    pub version: String,
}

impl std::fmt::Debug for EndpointFields {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The token is private bridge material. Keep its byte size for
        // diagnostics, matching ProviderSession's redaction contract, but
        // never hand Debug a value that can print it.
        formatter
            .debug_struct("EndpointFields")
            .field("port", &self.port)
            .field("token_bytes", &self.token.len())
            .field("env", &self.env)
            .field("version", &self.version)
            .finish()
    }
}

/// Is this value safe to write into a file `/bin/sh` will source, unquoted?
///
/// The measured allowlist (`isShellSafeEndpointValue`): letters, digits, and
/// `. _ : / -`. Nothing that can end a word, start a substitution, or escape
/// anything. An empty value is refused too — `K=` followed by a hostile next
/// line is exactly the ambiguity this test exists to remove.
pub fn is_shell_safe(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b':' | b'/' | b'-'))
}

/// Write the endpoint file atomically and return its path.
///
/// Refuses — without writing anything — when any value fails the shell-safety
/// test: a missing file degrades to the PTY env, a poisoned file is sourced by
/// every hook on the machine. The write goes through a temp file in the same
/// directory and a rename, because half an endpoint file is a parse error in
/// every script that races the write.
///
/// The directory is created `0o700` and the file `0o600`: the token in here is
/// the only thing standing between another local user and a POST that this
/// bridge will believe.
pub fn write_endpoint_file(dir: &Path, fields: &EndpointFields) -> io::Result<PathBuf> {
    let port = fields.port.to_string();
    for (name, value) in [
        ("port", port.as_str()),
        ("token", fields.token.as_str()),
        ("env", fields.env.as_str()),
        ("version", fields.version.as_str()),
    ] {
        if !is_shell_safe(value) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("endpoint {name} is not shell-safe; leaving the PTY env to carry it"),
            ));
        }
    }

    std::fs::create_dir_all(dir)?;
    set_mode(dir, 0o700)?;
    sweep_stale_temps(dir);

    let lines = format!(
        "{}={}\n{}={}\n{}={}\n{}={}\n",
        crate::env_var::PORT,
        port,
        crate::env_var::TOKEN,
        fields.token,
        crate::env_var::AGENT_ENV,
        fields.env,
        crate::env_var::VERSION,
        fields.version,
    );

    let final_path = dir.join(ENDPOINT_FILE);
    let tmp_path = dir.join(format!(".endpoint-{}.tmp", std::process::id()));
    std::fs::write(&tmp_path, lines)?;
    set_mode(&tmp_path, 0o600)?;
    if let Err(error) = std::fs::rename(&tmp_path, &final_path) {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(error);
    }
    Ok(final_path)
}

/// Write one private token value beside the endpoint file and return its path.
///
/// Capability shims receive this path, not the value. The file is replaced
/// atomically and kept at 0600 in the same 0700 directory as the endpoint, so
/// a reader sees either the old complete token or the new complete token.
pub fn write_private_token_file(dir: &Path, file_name: &str, token: &str) -> io::Result<PathBuf> {
    if file_name.is_empty()
        || file_name == "."
        || file_name == ".."
        || !file_name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        || !is_shell_safe(token)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "private token file name or value is not safe",
        ));
    }

    std::fs::create_dir_all(dir)?;
    set_mode(dir, 0o700)?;

    let final_path = dir.join(file_name);
    // A thread-local sequence keeps concurrent pane launches from sharing a
    // temporary name. Private temps are swept by the next endpoint write;
    // sweeping them here would race a different pane's in-flight write.
    static PRIVATE_TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let sequence = PRIVATE_TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let tmp_path = dir.join(format!(
        ".private-{file_name}-{}-{sequence}.tmp",
        std::process::id()
    ));
    std::fs::write(&tmp_path, token)?;
    set_mode(&tmp_path, 0o600)?;
    if let Err(error) = std::fs::rename(&tmp_path, &final_path) {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(error);
    }
    Ok(final_path)
}

/// A crashed writer leaves its temp file behind; the next writer takes it out.
/// Only files matching our own temp pattern — this directory is ours, but a
/// sweep that deletes by directory rather than by name is one bad path from a
/// disaster.
fn sweep_stale_temps(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if (name.starts_with(".endpoint-") || name.starts_with(".private-"))
            && name.ends_with(".tmp")
        {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

fn set_mode(path: &Path, mode: u32) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(path)?.permissions();
        perms.set_mode(mode);
        std::fs::set_permissions(path, perms)?;
    }
    #[cfg(not(unix))]
    let _ = (path, mode);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields(token: &str) -> EndpointFields {
        EndpointFields {
            port: 4021,
            token: token.to_string(),
            env: "production".to_string(),
            version: "1".to_string(),
        }
    }

    #[test]
    fn the_file_is_written_sourceable_and_private() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path =
            write_endpoint_file(dir.path(), &fields("tok-1234")).expect("write endpoint file");
        let text = std::fs::read_to_string(&path).expect("read back");
        assert_eq!(
            text,
            format!(
                "{}=4021\n{}=tok-1234\n{}=production\n{}=1\n",
                crate::env_var::PORT,
                crate::env_var::TOKEN,
                crate::env_var::AGENT_ENV,
                crate::env_var::VERSION,
            )
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let file_mode = std::fs::metadata(&path).expect("meta").permissions().mode();
            assert_eq!(file_mode & 0o777, 0o600, "the token is readable by others");
            let dir_mode = std::fs::metadata(dir.path())
                .expect("meta")
                .permissions()
                .mode();
            assert_eq!(dir_mode & 0o777, 0o700);
        }
    }

    #[test]
    fn endpoint_debug_does_not_print_the_secret() {
        let secret = "tok-debug-must-not-escape";
        let printed = format!("{:?}", fields(secret));
        assert!(
            !printed.contains(secret),
            "EndpointFields Debug disclosed its token: {printed}"
        );
        assert!(printed.contains("token_bytes"));
        assert!(printed.contains(&secret.len().to_string()));
    }

    #[test]
    fn a_private_token_file_is_atomic_and_mode_six_hundred() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write_private_token_file(dir.path(), BROWSER_TOKEN_FILE, "browser-secret")
            .expect("write private token");
        assert_eq!(
            std::fs::read_to_string(&path).expect("read token"),
            "browser-secret"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path)
                    .expect("token metadata")
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
            assert_eq!(
                std::fs::metadata(dir.path())
                    .expect("directory metadata")
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
        }
    }

    /// The refusal is the security property: a value that could break out of a
    /// sourced assignment must prevent the WRITE, not get quoted.
    #[test]
    fn a_hostile_value_prevents_the_file_not_just_the_line() {
        let dir = tempfile::tempdir().expect("tempdir");
        for hostile in [
            "tok; rm -rf /",
            "tok$(id)",
            "tok`id`",
            "tok tok",
            "tok\nX=1",
            "",
        ] {
            let refused = write_endpoint_file(dir.path(), &fields(hostile));
            assert!(
                refused.is_err(),
                "`{hostile}` was written into a sourced file"
            );
            assert!(
                !dir.path().join(ENDPOINT_FILE).exists(),
                "`{hostile}` left a file behind"
            );
        }
        // And the ordinary token shape passes.
        assert!(is_shell_safe("0198c0d9-7000-8000-abcdef012345"));
    }

    #[test]
    fn a_stale_temp_from_a_crashed_writer_is_swept() {
        let dir = tempfile::tempdir().expect("tempdir");
        let stale = dir.path().join(".endpoint-99999.tmp");
        std::fs::write(&stale, "half a file").expect("plant stale temp");
        // An unrelated file is not ours to delete, even here.
        let bystander = dir.path().join("last-status.json");
        std::fs::write(&bystander, "{}").expect("plant bystander");
        write_endpoint_file(dir.path(), &fields("tok-1")).expect("write");
        assert!(!stale.exists(), "the stale temp survived the sweep");
        assert!(bystander.exists(), "the sweep deleted a bystander");
    }
}
