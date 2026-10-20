use std::io::Cursor;

use flate2::Compression;
use flate2::write::GzEncoder;
use sha2::{Digest, Sha256};

use super::*;

const URL: &str = "https://example.invalid/native-runtime.tar.gz";

fn artifact(path: &str, bytes: &[u8]) -> Artifact {
    Artifact {
        path: path.to_owned(),
        bytes: bytes.len() as u64,
        sha256: format!("{:x}", Sha256::digest(bytes)),
        url: URL.to_owned(),
        package: None,
        version: None,
        license: None,
    }
}

fn package(entries: &[(&str, Option<&str>, &[u8])]) -> Vec<u8> {
    let mut builder = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::fast()));
    for (name, link, bytes) in entries {
        let mut header = tar::Header::new_gnu();
        header.set_path(name).expect("fixture path");
        header.set_mode(0o755);
        if let Some(link) = link {
            header.set_entry_type(tar::EntryType::Symlink);
            header.set_link_name(link).expect("fixture link");
            header.set_size(0);
        } else {
            header.set_entry_type(tar::EntryType::Regular);
            header.set_size(bytes.len() as u64);
        }
        header.set_cksum();
        builder
            .append(&header, Cursor::new(*bytes))
            .expect("fixture member");
    }
    builder
        .into_inner()
        .expect("archive")
        .finish()
        .expect("gzip")
}

fn runtime(bytes: &[u8], files: Vec<Artifact>) -> Runtime {
    Runtime {
        archive: artifact("native.tar.gz", bytes),
        executable: "package/llama-server".to_owned(),
        files,
    }
}

#[test]
fn the_native_archive_copies_only_pinned_files_and_flattens_internal_library_links() {
    let server = b"owned fixture executable data";
    let library = b"owned fixture library data";
    let bytes = package(&[
        ("package/llama-server", None, server),
        ("package/libimpl.dylib", None, library),
        ("package/libalias.dylib", Some("libimpl.dylib"), b""),
        ("package/other-tool", None, b"unused data"),
    ]);
    let profile = runtime(
        &bytes,
        vec![
            artifact("package/llama-server", server),
            artifact("package/libimpl.dylib", library),
            artifact("package/libalias.dylib", library),
        ],
    );
    let home = tempfile::tempdir().expect("owned stage");
    let archive = home.path().join("native.tar.gz");
    std::fs::write(&archive, bytes).expect("archive bytes");
    let installed =
        crate::archive::native(&archive, home.path(), &profile, &AtomicBool::new(false));
    assert!(
        installed.is_ok(),
        "pinned native archive was not installed: {installed:?}"
    );
    let alias = home.path().join("native/package/libalias.dylib");
    assert_eq!(std::fs::read(&alias).expect("alias data"), library);
    assert!(
        !std::fs::symlink_metadata(alias)
            .expect("alias metadata")
            .file_type()
            .is_symlink()
    );
    assert!(!home.path().join("native/package/other-tool").exists());
}

#[test]
fn native_archive_links_cannot_escape_the_stage_or_form_a_cycle() {
    for target in ["../outside", "/outside", "libalias.dylib"] {
        let bytes = package(&[
            ("package/llama-server", None, b"fixture"),
            ("package/libalias.dylib", Some(target), b""),
        ]);
        let profile = runtime(
            &bytes,
            vec![
                artifact("package/llama-server", b"fixture"),
                artifact("package/libalias.dylib", b"fixture"),
            ],
        );
        let home = tempfile::tempdir().expect("owned stage");
        let archive = home.path().join("native.tar.gz");
        std::fs::write(&archive, bytes).expect("archive");
        assert!(
            crate::archive::native(&archive, home.path(), &profile, &AtomicBool::new(false))
                .is_err(),
            "{target}"
        );
        assert!(!home.path().join("outside").exists());
    }
}

#[test]
fn native_archive_corruption_and_duplicate_paths_do_not_commit() {
    for entries in [
        vec![("package/llama-server", None, b"corrupt".as_slice())],
        vec![
            ("package/llama-server", None, b"fixture".as_slice()),
            ("package/llama-server", None, b"fixture".as_slice()),
        ],
    ] {
        let bytes = package(&entries);
        let profile = runtime(&bytes, vec![artifact("package/llama-server", b"fixture")]);
        let home = tempfile::tempdir().expect("owned stage");
        let archive = home.path().join("native.tar.gz");
        std::fs::write(&archive, bytes).expect("archive");
        assert!(
            crate::archive::native(&archive, home.path(), &profile, &AtomicBool::new(false))
                .is_err()
        );
        assert!(!home.path().join("receipt.json").exists());
    }
}

#[test]
fn a_cancelled_native_archive_does_not_create_runtime_files() {
    let bytes = package(&[("package/llama-server", None, b"fixture")]);
    let profile = runtime(&bytes, vec![artifact("package/llama-server", b"fixture")]);
    let home = tempfile::tempdir().expect("owned stage");
    let archive = home.path().join("native.tar.gz");
    std::fs::write(&archive, bytes).expect("archive");
    assert_eq!(
        crate::archive::native(&archive, home.path(), &profile, &AtomicBool::new(true)),
        Err(Error::Cancelled)
    );
    assert!(!home.path().join("native/package/llama-server").exists());
}

#[test]
fn the_python_supervisor_owns_only_its_pinned_native_child_and_connection() {
    let source = serde_json::to_string(include_str!("../bridge.py")).expect("bridge source");
    let output = std::process::Command::new("python3")
        .args([
            "-B",
            "-c",
            include_str!("../../tests/llama_boundary.py"),
            &source,
        ])
        .output()
        .expect("supervisor boundary suite");
    assert!(
        output.status.success(),
        "native supervisor boundary failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
