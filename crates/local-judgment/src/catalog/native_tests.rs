use super::*;

#[test]
fn builtin_runtime_profiles_load_with_the_reviewed_native_profile_on_its_platform() {
    let profiles = runtime_profiles();
    assert!(
        profiles.is_ok(),
        "builtin_runtime_profiles_did_not_load: {profiles:?}"
    );
    let profiles = profiles.unwrap();
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        let profile = profiles
            .iter()
            .find(|profile| profile.engine == EngineKind::LlamaCpp)
            .expect("builtin_llama_runtime_profile_was_missing");
        let native = profile
            .native
            .as_ref()
            .expect("builtin_native_files_were_missing");
        assert_eq!(
            native.validate(),
            Ok(()),
            "builtin_native_files_did_not_validate"
        );
        assert!(!native.files.is_empty());
        assert!(
            native
                .files
                .iter()
                .any(|file| file.path == native.executable)
        );
    }
    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
    assert!(
        !profiles
            .iter()
            .any(|profile| profile.engine == EngineKind::LlamaCpp),
        "native_runtime_was_offered_on_an_unreviewed_platform"
    );
}

fn pinned() -> Artifact {
    Artifact {
        path: "runtime.py".to_owned(),
        bytes: 1,
        sha256: "a".repeat(64),
        url: "https://example.invalid/runtime.py".to_owned(),
        package: None,
        version: None,
        license: None,
    }
}

#[test]
fn python_runtime_files_still_require_a_nonempty_pinned_list_and_all_integrity_limits() {
    assert_eq!(validate_files(&[]), Err(Error::Manifest));
    assert_eq!(validate_files(&[pinned()]), Ok(()));
    let mut path = pinned();
    path.path = "../runtime.py".to_owned();
    assert_eq!(validate_files(&[path]), Err(Error::UnsafePath));
    let mut digest = pinned();
    digest.sha256 = "invalid".to_owned();
    assert_eq!(validate_files(&[digest]), Err(Error::Manifest));
    let mut size = pinned();
    size.bytes = 0;
    assert_eq!(validate_files(&[size]), Err(Error::Manifest));
    assert_eq!(validate_files(&[pinned(), pinned()]), Err(Error::Manifest));
    let mut large = pinned();
    large.bytes = 4 * 1024 * 1024 * 1024 + 1;
    assert_eq!(validate_files(&[large]), Err(Error::TooLarge));
}

#[test]
fn native_files_cannot_be_empty_instead_of_the_python_runtime_files() {
    let native = crate::llama::Runtime {
        archive: Artifact {
            path: "runtime.tar.gz".to_owned(),
            ..pinned()
        },
        executable: "package/llama-server".to_owned(),
        files: Vec::new(),
    };
    assert_eq!(native.validate(), Err(Error::Manifest));
}
