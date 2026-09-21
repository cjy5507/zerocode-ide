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
            avd_home: None,
            android_user_home: None,
            sdk_home: None,
            home: Some(self.directory.path().to_path_buf()),
        }
    }

    /// One AVD directory under `home`, with the pointer beside it that the
    /// SDK writes — optionally pointing somewhere else entirely, which is
    /// what a person who moved an AVD off the boot disk has.
    fn avd(&self, home: &Path, name: &str, contents_at: Option<&Path>) -> PathBuf {
        std::fs::create_dir_all(home).expect("avd home");
        let root = contents_at.map_or_else(
            || home.join(format!("{name}{AVD_DIRECTORY_SUFFIX}")),
            Path::to_path_buf,
        );
        std::fs::create_dir_all(&root).expect("avd directory");
        std::fs::write(
            home.join(format!("{name}{AVD_POINTER_SUFFIX}")),
            format!(
                "avd.ini.encoding=UTF-8\n{AVD_PATH_KEY}={}\npath.rel=avd/{name}{AVD_DIRECTORY_SUFFIX}\n",
                root.display()
            ),
        )
        .expect("avd pointer");
        root
    }

    fn under(&self, name: &str) -> PathBuf {
        self.directory.path().join(name)
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

/// The AVD table, top to bottom: `$ANDROID_AVD_HOME`, then the `avd` inside
/// `$ANDROID_USER_HOME` and inside the legacy `$ANDROID_SDK_HOME/.android`,
/// then `~/.android/avd` — which is the only row a window opened from the Dock
/// has (trap 307), and the row this machine's own AVD sits in.
#[test]
fn each_row_of_the_avd_table_wins_over_the_rows_under_it() {
    let fake = FakeHome::new(&[]);
    let default_home = fake.under(DOT_ANDROID).join(AVD_SUBDIRECTORY);
    let user_home = fake.under("user-home");
    let sdk_home = fake.under("sdk-home");
    let avd_home = fake.under("avd-home");

    let by_default = fake.avd(&default_home, "probe", None);
    let by_user = fake.avd(&user_home.join(AVD_SUBDIRECTORY), "probe", None);
    let by_sdk = fake.avd(
        &sdk_home.join(DOT_ANDROID).join(AVD_SUBDIRECTORY),
        "probe",
        None,
    );
    let by_avd_home = fake.avd(&avd_home, "probe", None);

    let mut environment = fake.environment();
    assert_eq!(
        avd_root(&environment, "probe").as_deref(),
        Some(by_default.as_path()),
        "the home default is the last row and the only one set"
    );
    environment.sdk_home = Some(sdk_home.into_os_string());
    assert_eq!(
        avd_root(&environment, "probe").as_deref(),
        Some(by_sdk.as_path()),
        "$ANDROID_SDK_HOME comes before the home default"
    );
    environment.android_user_home = Some(user_home.into_os_string());
    assert_eq!(
        avd_root(&environment, "probe").as_deref(),
        Some(by_user.as_path()),
        "$ANDROID_USER_HOME comes before $ANDROID_SDK_HOME"
    );
    environment.avd_home = Some(avd_home.into_os_string());
    assert_eq!(
        avd_root(&environment, "probe").as_deref(),
        Some(by_avd_home.as_path()),
        "$ANDROID_AVD_HOME is the first row"
    );
}

/// The pointer beside an AVD is what says where its files are — an AVD moved
/// off the boot disk keeps its name in `~/.android/avd` and nothing else.
#[test]
fn the_pointer_decides_where_the_avd_files_are() {
    let fake = FakeHome::new(&[]);
    let home = fake.under(DOT_ANDROID).join(AVD_SUBDIRECTORY);
    let elsewhere = fake.under("second-disk").join("moved.avd");
    let root = fake.avd(&home, "moved", Some(&elsewhere));
    assert_eq!(root, elsewhere);
    assert_eq!(
        avd_root(&fake.environment(), "moved").as_deref(),
        Some(elsewhere.as_path())
    );

    // A pointer naming a directory that is not there falls back to the
    // `<name>.avd` beside it rather than answering a road nobody can read.
    std::fs::remove_dir_all(&elsewhere).expect("clear");
    let beside = home.join(format!("moved{AVD_DIRECTORY_SUFFIX}"));
    std::fs::create_dir_all(&beside).expect("avd directory");
    assert_eq!(
        avd_root(&fake.environment(), "moved").as_deref(),
        Some(beside.as_path())
    );
}

/// An AVD this machine does not have is not an error anywhere it is asked —
/// and a name that could not go on a command line is never joined into a path.
#[test]
fn an_unknown_or_unusable_avd_name_answers_nothing() {
    let fake = FakeHome::new(&[]);
    fake.avd(
        &fake.under(DOT_ANDROID).join(AVD_SUBDIRECTORY),
        "probe",
        None,
    );
    assert_eq!(avd_root(&fake.environment(), "absent"), None);
    assert_eq!(avd_root(&fake.environment(), "../probe"), None);
    assert_eq!(avd_root(&fake.environment(), ""), None);
}

/// One reader for every AVD ini: answers in the order asked for, ignores what
/// was not asked for, and refuses a file too big to be one.
#[test]
fn the_ini_reader_answers_the_keys_it_was_asked_for() {
    let fake = FakeHome::new(&[]);
    let root = fake.avd(
        &fake.under(DOT_ANDROID).join(AVD_SUBDIRECTORY),
        "probe",
        None,
    );
    let config = root.join(AVD_CONFIG_FILE);
    std::fs::write(
        &config,
        "AvdId=probe\nfastboot.forceColdBoot=no\nfastboot.forceFastBoot = yes\nhw.ramSize=2G\n",
    )
    .expect("config");

    assert_eq!(
        ini_values(&config, &COLD_BOOT_KEYS.map(|(key, _)| key)),
        vec![
            ("fastboot.forceColdBoot".to_string(), "no".to_string()),
            ("fastboot.forceFastBoot".to_string(), "yes".to_string()),
        ],
        "both keys, in the order the table names them, with the spaces trimmed"
    );
    assert_eq!(ini_values(&config, &["hw.cpu"]), Vec::new());
    assert_eq!(ini_values(&root.join("absent.ini"), &["AvdId"]), Vec::new());

    std::fs::write(&config, vec![b'#'; AVD_INI_MAX_BYTES as usize + 1]).expect("oversized");
    assert_eq!(
        ini_values(&config, &["AvdId"]),
        Vec::new(),
        "a file too big to be an ini is not read"
    );
}
