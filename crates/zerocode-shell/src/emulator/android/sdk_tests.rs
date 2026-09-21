//! The one table that says where `adb` and `emulator` live.
//!
//! A window opened from the Dock inherits `PATH=/usr/bin:/bin:/usr/sbin:/sbin`
//! and no `ANDROID_*` at all (trap 307), so the first row of that table is
//! empty on the very machine the person is sitting at. These exercise the rows
//! below it — and the sentence that names every road when none of them holds
//! the binary.

use super::*;

/// A home with whichever of the two packages the caller names, built under a
/// temporary directory so no test reads this machine's real SDK.
struct FakeHome {
    directory: tempfile::TempDir,
}

impl FakeHome {
    fn new(packages: &[AndroidTool]) -> Self {
        let home = FakeHome {
            directory: tempfile::tempdir().expect("fake home"),
        };
        for tool in packages {
            install(&home.sdk_root(), *tool);
        }
        home
    }

    /// The host default this machine looks at last.
    fn sdk_root(&self) -> PathBuf {
        host_default_sdk_root(self.directory.path())
    }

    /// A second SDK somewhere else entirely, for the `$ANDROID_*` rows.
    fn elsewhere(&self, name: &str, packages: &[AndroidTool]) -> PathBuf {
        let root = self.directory.path().join(name);
        for tool in packages {
            install(&root, *tool);
        }
        root
    }

    /// A loose directory of binaries, the shape `$PATH` entries have.
    fn loose(&self, name: &str, tools: &[AndroidTool]) -> PathBuf {
        let directory = self.directory.path().join(name);
        std::fs::create_dir_all(&directory).expect("loose directory");
        for tool in tools {
            write_binary(&directory.join(executable(tool.binary)));
        }
        directory
    }

    fn environment(&self) -> SdkEnvironment {
        SdkEnvironment {
            path: Some(OsString::from("/usr/bin:/bin:/usr/sbin:/sbin")),
            sdk_root: None,
            android_home: None,
            home: Some(self.directory.path().to_path_buf()),
        }
    }
}

/// One tool in its package under `root`.
fn install(root: &Path, tool: AndroidTool) {
    let package = root.join(tool.package);
    std::fs::create_dir_all(&package).expect("package directory");
    write_binary(&package.join(executable(tool.binary)));
}

fn write_binary(at: &Path) {
    std::fs::write(at, "#!/bin/sh\nexit 0\n").expect("fixture binary");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(at, std::fs::Permissions::from_mode(0o700))
            .expect("fixture binary is executable");
    }
}

/// The failing window's own environment: the minimal Dock `PATH`, no
/// `ANDROID_*`, and an SDK sitting where the installer put it.
#[test]
fn a_minimal_path_still_finds_the_sdk_under_the_home_default() {
    let home = FakeHome::new(&[ADB, EMULATOR]);
    let root = home.sdk_root();
    let sdk = resolve_android_sdk(&home.environment()).expect("the home default SDK");
    assert_eq!(sdk.adb, root.join(ADB.package).join(executable(ADB.binary)));
    assert_eq!(
        sdk.emulator.as_deref().ok(),
        Some(
            root.join(EMULATOR.package)
                .join(executable(EMULATOR.binary))
        )
        .as_deref()
    );
    assert_eq!(sdk.root, root);
}

/// `adb` alone is the whole road for screen, input and tree. A machine without
/// the `emulator` package still talks to a device that is already up, so the
/// missing package may not take the binary that is right there.
#[test]
fn adb_resolves_even_when_the_emulator_package_is_missing() {
    let home = FakeHome::new(&[ADB]);
    let sdk = resolve_android_sdk(&home.environment()).expect("adb alone is enough");
    assert_eq!(
        sdk.adb,
        home.sdk_root()
            .join(ADB.package)
            .join(executable(ADB.binary))
    );
    let why = sdk.emulator.unwrap_err().to_string();
    assert!(why.contains("emulator"), "the reason never names it: {why}");
}

/// Row order, top to bottom: `$PATH`, `$ANDROID_SDK_ROOT`, `$ANDROID_HOME`,
/// then this machine's default. Each row wins over every row under it.
#[test]
fn each_row_of_the_table_wins_over_the_rows_under_it() {
    let home = FakeHome::new(&[ADB, EMULATOR]);
    let sdk_root = home.elsewhere("sdk-root", &[ADB, EMULATOR]);
    let android_home = home.elsewhere("android-home", &[ADB, EMULATOR]);
    let on_path = home.loose("on-path", &[ADB, EMULATOR]);

    let mut environment = home.environment();
    environment.sdk_root = Some(sdk_root.clone().into_os_string());
    environment.android_home = Some(android_home.clone().into_os_string());
    let mut with_path = environment.clone();
    with_path.path = Some(std::env::join_paths([&on_path]).expect("a PATH"));

    assert_eq!(
        resolve_android_sdk(&with_path)
            .expect("the PATH binary")
            .adb,
        on_path.join(executable(ADB.binary)),
        "$PATH is the first row"
    );
    assert_eq!(
        resolve_android_sdk(&environment)
            .expect("the configured root")
            .adb,
        sdk_root.join(ADB.package).join(executable(ADB.binary)),
        "$ANDROID_SDK_ROOT comes before $ANDROID_HOME"
    );
    let mut home_only = environment.clone();
    home_only.sdk_root = None;
    assert_eq!(
        resolve_android_sdk(&home_only).expect("$ANDROID_HOME").adb,
        android_home.join(ADB.package).join(executable(ADB.binary)),
        "$ANDROID_HOME comes before the host default"
    );
}

/// A `$PATH` that holds one package still reaches the other: `platform-tools`
/// on the path is how a shell is usually set up, and the emulator sits in its
/// sibling directory under the same root.
#[test]
fn a_path_entry_reaches_its_sibling_package() {
    let home = FakeHome::new(&[]);
    let root = home.elsewhere("sdk", &[ADB, EMULATOR]);
    let mut environment = home.environment();
    environment.path = Some(std::env::join_paths([root.join(ADB.package)]).expect("a PATH"));

    let sdk = resolve_android_sdk(&environment).expect("both tools under one root");
    assert_eq!(sdk.adb, root.join(ADB.package).join(executable(ADB.binary)));
    assert_eq!(
        sdk.emulator.as_deref().ok(),
        Some(
            root.join(EMULATOR.package)
                .join(executable(EMULATOR.binary))
        )
        .as_deref()
    );
}

/// When nothing holds the binary the person is told which roads were walked —
/// every row of the table, named the way they would set it.
#[test]
fn a_missing_sdk_names_every_road_it_looked_down() {
    let home = FakeHome::new(&[]);
    let configured = home.elsewhere("configured", &[]);
    let mut environment = home.environment();
    environment.sdk_root = Some(configured.clone().into_os_string());

    let why = resolve_android_sdk(&environment)
        .expect_err("an empty home has no SDK")
        .to_string();
    for road in [
        "adb",
        "/usr/bin",
        "$ANDROID_SDK_ROOT",
        &configured.display().to_string(),
        "$ANDROID_HOME",
        &home.sdk_root().display().to_string(),
    ] {
        assert!(why.contains(road), "the reason never names {road}: {why}");
    }
}

/// A root that is not absolute is a typo, not a road: it would otherwise be
/// joined against whatever directory the window happens to be sitting in.
#[test]
fn a_relative_configured_root_is_not_walked() {
    let home = FakeHome::new(&[]);
    let mut environment = home.environment();
    environment.sdk_root = Some(OsString::from("relative/sdk"));
    let why = resolve_android_sdk(&environment)
        .expect_err("nothing is installed")
        .to_string();
    assert!(
        !why.contains("relative/sdk"),
        "a relative root was walked anyway: {why}"
    );
}
