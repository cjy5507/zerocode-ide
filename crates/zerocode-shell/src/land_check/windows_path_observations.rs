//! Native observations of the lexical classifier and the compatibility adapter.
//! These fixtures never remove a worktree through a link. A successful probe
//! describes only that fixture; unsupported aliases remain observations.

use super::*;
use std::io;
use std::os::windows::ffi::OsStrExt;
use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_SHARE_DELETE,
    FILE_SHARE_READ, FILE_SHARE_WRITE, GetFileInformationByHandle, OPEN_EXISTING,
};
use zerocode_core::{Ownership, WorktreeFacts, WorktreeOurs};

/// Use the exact Win32 spelling. Rust's filesystem adapter may itself add an
/// extended prefix, which would hide the alias difference being measured.
fn identity(path: &Path) -> io::Result<(u32, u32, u32)> {
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    // SAFETY: wide is terminated and lives through the call. No access or
    // inheritance is requested; the handle is closed on both result paths.
    unsafe {
        let handle = CreateFileW(
            wide.as_ptr(),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            std::ptr::null_mut(),
        );
        if handle == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        let mut info: BY_HANDLE_FILE_INFORMATION = std::mem::zeroed();
        let result = if GetFileInformationByHandle(handle, &mut info) == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok((
                info.dwVolumeSerialNumber,
                info.nFileIndexHigh,
                info.nFileIndexLow,
            ))
        };
        CloseHandle(handle);
        result
    }
}

fn classify_path(path: &Path, root: &Path) -> Ownership {
    zerocode_core::classify(
        WorktreeFacts {
            path: path.to_str().expect("fixture path"),
            branch: None,
        },
        WorktreeOurs {
            worktree_root: root.to_str().expect("fixture root"),
            past_roots: &[],
            branch_prefix: "",
        },
    )
}

fn replacement_observation(junction: bool) {
    let temp = tempfile::tempdir().expect("fixture");
    let base = without_verbatim_prefix(fs::canonicalize(temp.path()).expect("canonical fixture"));
    let managed = base.join("managed");
    let targets = [base.join("outside-a"), base.join("outside-b")];
    fs::create_dir(&managed).expect("managed root");
    for target in &targets {
        fs::create_dir(target).expect("outside target");
    }
    let link = managed.join("door");
    let mut observed = Vec::new();
    for (index, target) in targets.iter().enumerate() {
        if junction {
            let output = crate::proc::quiet_command("cmd")
                .args(["/d", "/c", "mklink", "/J"])
                .arg(&link)
                .arg(target)
                .output()
                .expect("mklink");
            assert!(
                output.status.success(),
                "junction creation failed: {output:?}"
            );
        } else if let Err(error) = std::os::windows::fs::symlink_dir(target, &link) {
            // Developer Mode / SeCreateSymbolicLinkPrivilege is host policy.
            // A permission refusal is a recorded capability, not race evidence.
            assert_eq!(
                index, 0,
                "a previously available symlink capability disappeared"
            );
            assert!(matches!(error.raw_os_error(), Some(5 | 1314)), "{error}");
            println!(
                "{}",
                json!({"probe":"symlink-replacement", "available":false,
                "win32_error":error.raw_os_error(), "replacement_observed":false})
            );
            return;
        }
        let actual = identity(&link).expect("link target identity");
        assert_eq!(actual, identity(target).expect("outside identity"));
        assert!(
            !fs::canonicalize(&link)
                .unwrap()
                .starts_with(fs::canonicalize(&managed).unwrap())
        );
        assert_eq!(classify_path(&link, &managed), Ownership::ZerocodeManaged);
        observed.push(actual);
        // Remove just the reparse point, never its target or a worktree.
        fs::remove_dir(&link).expect("remove fixture reparse point");
    }
    assert_ne!(
        observed[0], observed[1],
        "the fixture did not change identity"
    );
    println!(
        "{}",
        json!({"probe":if junction {"junction-replacement"} else {"symlink-replacement"},
        "available":true, "target_changed":true, "lexical_classification_unchanged":true,
        "race_protection_proven":false})
    );
}

#[test]
fn a_symlink_replacement_does_not_change_lexical_ownership() {
    replacement_observation(false);
}

#[test]
fn a_junction_replacement_does_not_change_lexical_ownership() {
    replacement_observation(true);
}

#[test]
fn namespace_aliases_record_identity_and_git_compatibility() {
    let temp = tempfile::tempdir().expect("fixture");
    let extended = fs::canonicalize(temp.path()).expect("extended fixture root");
    let plain = without_verbatim_prefix(extended.clone());
    let repo = plain.join("repo");
    fs::create_dir(&repo).expect("repository");
    for args in [
        vec!["init", "-q", "-b", "main"],
        vec![
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--allow-empty",
            "-qm",
            "fixture",
        ],
    ] {
        let output = crate::proc::quiet_command("git")
            .arg("-C")
            .arg(&repo)
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success(), "fixture git failed: {output:?}");
    }
    // The last three names are outside dunce 1.0.5's finite DOS-name list.
    // They are observations, not declarations that those aliases are safe.
    for (name, protected) in [
        ("plain", false),
        ("trailing.", true),
        ("trailing ", true),
        ("CON", true),
        ("CONIN$", false),
        ("COM¹", false),
        ("LPT²", false),
    ] {
        let path = extended.join(name);
        if let Err(error) = fs::create_dir(&path) {
            assert_ne!(name, "plain", "the plain control must work: {error}");
            println!(
                "{}",
                json!({"probe":"namespace", "case":name,
                "created":false, "win32_error":error.raw_os_error()})
            );
            continue;
        }
        let original = identity(&path).expect("created directory identity");
        let adapted = without_verbatim_prefix(path.clone());
        let adapted_identity = identity(&adapted);
        let same_identity = adapted_identity.as_ref().is_ok_and(|id| *id == original);
        if name == "plain" || protected {
            assert!(
                same_identity,
                "the supported adapter case changed identity: {name}"
            );
        }
        if protected {
            assert_eq!(adapted, path, "sensitive spelling lost its prefix: {name}");
        }
        let mut git_results = Vec::new();
        // Use distinct empty destinations so one successful add cannot make
        // the second invocation fail merely because a checkout already exists.
        for (spelling, destination) in [
            ("extended", path.join("extended-child")),
            ("adapted", adapted.join("adapted-child")),
        ] {
            let output = crate::proc::quiet_command("git")
                .arg("-C")
                .arg(&repo)
                .args(["worktree", "add", "--detach"])
                .arg(&destination)
                .output()
                .expect("git observation");
            let listed = crate::proc::quiet_command("git")
                .arg("-C")
                .arg(&repo)
                .args(["worktree", "list", "--porcelain", "-z"])
                .output()
                .unwrap();
            assert!(listed.status.success(), "worktree listing failed");
            let mut listed_same = 0;
            for row in listed.stdout.split(|byte| *byte == 0) {
                if let Some(path) = row.strip_prefix(b"worktree ") {
                    let path = PathBuf::from(String::from_utf8_lossy(path).into_owned());
                    if identity(&path).ok() == identity(&destination).ok()
                        && identity(&destination).is_ok()
                    {
                        listed_same += 1;
                    }
                }
            }
            // The ordinary spelling is the required Git control. Whether
            // this Git also accepts an extended spelling is an observation.
            if name == "plain" && spelling == "adapted" {
                assert!(
                    output.status.success(),
                    "plain git control failed: {output:?}"
                );
                assert_eq!(listed_same, 1, "Git's listed path must retain identity");
            }
            git_results.push(json!({"spelling":spelling, "rc":output.status.code(),
                "listed_same_identity":listed_same}));
        }
        println!(
            "{}",
            json!({"probe":"namespace", "case":name, "created":true,
            "adapter_changed_spelling":adapted != path, "adapter_same_identity":same_identity,
            "adapter_error":adapted_identity.err().and_then(|error| error.raw_os_error()),
            "git":git_results})
        );
    }
    // These are local disk namespace spellings; no network share is contacted.
    let suffix = plain.to_str().unwrap();
    let device = PathBuf::from(format!(r"\\.\{suffix}"));
    assert_eq!(without_verbatim_prefix(device.clone()), device);
    println!(
        "{}",
        json!({"probe":"device-disk-namespace",
        "same_identity":identity(&device).ok() == Some(identity(&extended).unwrap()),
        "win32_error":identity(&device).err().and_then(|error| error.raw_os_error())})
    );
}
