//! The fixture is generated here on purpose: no third-party archive is
//! committed as test data.

use std::fs;

#[test]
fn packs_and_reads_back_a_nested_archive() {
    let dir = tempfile::tempdir().expect("tempdir");
    let archive_path = dir.path().join("fixture.asar");
    let bytes = asar_reader::pack(&[
        ("index.js", b"console.log('hi')" as &[u8]),
        ("out/shared/pane-key.js", b"exports.FIRST_PANE_ID = 1;"),
        ("assets/style.css", b":root{--x:1px}"),
    ]);
    fs::write(&archive_path, &bytes).expect("write fixture");

    let (_, archive) = asar_reader::open(&archive_path).expect("open");
    let entries = asar_reader::entries(&archive);
    let paths: Vec<&str> = entries.iter().map(|e| e.path.as_str()).collect();
    assert_eq!(
        paths,
        vec!["assets/style.css", "index.js", "out/shared/pane-key.js"]
    );

    let out = dir.path().join("extracted");
    let written = asar_reader::extract(&archive_path, &out, "").expect("extract");
    assert_eq!(written, 3);
    assert_eq!(
        fs::read_to_string(out.join("out/shared/pane-key.js")).expect("read"),
        "exports.FIRST_PANE_ID = 1;"
    );
}

#[test]
fn filter_extracts_only_matching_entries() {
    let dir = tempfile::tempdir().expect("tempdir");
    let archive_path = dir.path().join("fixture.asar");
    fs::write(
        &archive_path,
        asar_reader::pack(&[("a/keep.css", b"body{}" as &[u8]), ("b/skip.js", b"noop")]),
    )
    .expect("write fixture");

    let out = dir.path().join("extracted");
    let written = asar_reader::extract(&archive_path, &out, ".css").expect("extract");
    assert_eq!(written, 1);
    assert!(out.join("a/keep.css").exists());
    assert!(!out.join("b/skip.js").exists());
}

/// A packer and a reader that share a wrong assumption still round-trip
/// happily — which is exactly how the JSON offset was wrong here at first. So
/// pin the bytes against the layout observed in a real Electron bundle:
/// `[4][header_len][header_len - 4][json_len]`, JSON at offset 16.
#[test]
fn the_packed_header_matches_the_real_electron_byte_layout() {
    let bytes = asar_reader::pack(&[("a.txt", b"hi" as &[u8])]);
    let word = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().expect("word"));

    assert_eq!(word(0), 4, "outer pickle payload size");
    let header_len = word(4);
    assert_eq!(
        word(8),
        header_len - 4,
        "header pickle repeats its own payload size"
    );
    let json_len = word(12) as usize;
    // `size` before `offset`, which is the order a real Electron bundle has
    // (measured off Orca 1.4.169's app.asar). It used to read `offset,"size"`
    // here — alphabetical, which is what serde_json's default BTreeMap gives —
    // and that was an accident of a feature flag rather than the layout this
    // test says it pins. Enabling `preserve_order` for another crate in the
    // workspace changed it, which is how the accident surfaced.
    assert_eq!(
        &bytes[16..16 + json_len],
        &br#"{"files":{"a.txt":{"size":2,"offset":"0"}}}"#[..],
        "json must start at offset 16"
    );
    assert_eq!(
        &bytes[8 + header_len as usize..],
        b"hi",
        "file data starts at 8 + header_len"
    );
}

/// Rejecting `..` in entry paths is not enough. If anything inside the output
/// directory is already a symlink — a stale extraction, a directory the user
/// linked elsewhere — then `create_dir_all` and `fs::write` happily follow it
/// and a hostile archive lands its bytes outside the root we were handed.
#[cfg(unix)]
#[test]
fn extraction_refuses_to_write_through_a_symlink_inside_the_output_directory() {
    let dir = tempfile::tempdir().expect("tempdir");
    let outside = dir.path().join("outside");
    let out = dir.path().join("extracted");
    fs::create_dir_all(&outside).expect("outside");
    fs::create_dir_all(&out).expect("out");
    std::os::unix::fs::symlink(&outside, out.join("link")).expect("symlink");

    let archive_path = dir.path().join("fixture.asar");
    fs::write(
        &archive_path,
        asar_reader::pack(&[("link/escaped.txt", b"escaped" as &[u8])]),
    )
    .expect("write fixture");

    let err = asar_reader::extract(&archive_path, &out, "").expect_err("must refuse");
    assert!(
        matches!(err, asar_reader::AsarError::UnsafePath(_)),
        "{err}"
    );
    assert!(
        !outside.join("escaped.txt").exists(),
        "nothing may be written outside the output directory"
    );
}

/// The same escape, one level down: the symlink is the file itself rather than
/// a directory on the way to it.
#[cfg(unix)]
#[test]
fn extraction_refuses_to_overwrite_a_symlinked_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let target = dir.path().join("secret.txt");
    let out = dir.path().join("extracted");
    fs::write(&target, "original").expect("target");
    fs::create_dir_all(&out).expect("out");
    std::os::unix::fs::symlink(&target, out.join("innocent.txt")).expect("symlink");

    let archive_path = dir.path().join("fixture.asar");
    fs::write(
        &archive_path,
        asar_reader::pack(&[("innocent.txt", b"overwritten" as &[u8])]),
    )
    .expect("write fixture");

    let err = asar_reader::extract(&archive_path, &out, "").expect_err("must refuse");
    assert!(
        matches!(err, asar_reader::AsarError::UnsafePath(_)),
        "{err}"
    );
    assert_eq!(
        fs::read_to_string(&target).expect("read"),
        "original",
        "the linked-to file must be untouched"
    );
}

#[test]
fn rejects_a_file_that_is_not_an_archive() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("not.asar");
    fs::write(&path, b"this is not a pickle header at all").expect("write");
    let err = asar_reader::open(&path).expect_err("must reject");
    assert!(matches!(err, asar_reader::AsarError::BadMagic(_)), "{err}");
}
