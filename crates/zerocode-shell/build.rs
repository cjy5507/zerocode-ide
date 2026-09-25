fn main() {
    build_ios_emulator_helper();
    build_computer_use_helper();
    stamp_identity();
    tauri_build::build();
}

fn build_computer_use_helper() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    let manifest = std::path::PathBuf::from(
        std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"),
    );
    let package = manifest.join("native/computer-use-macos");
    announce_tree(&package);
    let status = std::process::Command::new("swift")
        .args(["build", "-c", "release", "--package-path"])
        .arg(&package)
        .status()
        .expect("launch swift build for Computer Use");
    assert!(status.success(), "Computer Use Swift build failed");

    let release = package.join(".build/release");
    let binary = release.join("zerocode-computer-use-macos");
    assert!(binary.is_file(), "Computer Use helper binary was not built");
    let destination = release.join("ZeroCode Computer Use.app");
    let app = release.join("ZeroCode Computer Use.staging.app");
    let executable = app
        .join("Contents/MacOS")
        .join("zerocode-computer-use-macos");
    let resources = app.join("Contents/Resources");
    if app.exists() {
        std::fs::remove_dir_all(&app).expect("replace generated Computer Use app");
    }
    std::fs::create_dir_all(executable.parent().expect("helper executable parent"))
        .expect("create Computer Use app executable directory");
    std::fs::create_dir_all(&resources).expect("create Computer Use app resources");
    std::fs::copy(&binary, &executable).expect("copy Computer Use executable");
    std::fs::copy(
        manifest.join("icons/icon.icns"),
        resources.join("AppIcon.icns"),
    )
    .expect("copy Computer Use icon");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755))
            .expect("chmod Computer Use executable");
    }
    std::fs::write(app.join("Contents/Info.plist"), computer_use_info_plist())
        .expect("write Computer Use Info.plist");
    let signing = manifest.join("../../tools/signing");
    for script in [
        "ensure-local-identity.sh",
        "sign-computer-use.sh",
        "sign-developer-id.sh",
    ] {
        println!("cargo:rerun-if-changed={}", signing.join(script).display());
    }
    println!("cargo:rerun-if-env-changed=ZEROCODE_SIGNING_KEYCHAIN");
    println!("cargo:rerun-if-env-changed=APPLE_SIGNING_IDENTITY");
    let signed = std::process::Command::new("bash")
        .arg(manifest.join("../../tools/signing/sign-computer-use.sh"))
        .arg(&app)
        .status()
        .expect("launch codesign for Computer Use");
    assert!(signed.success(), "Computer Use helper signing failed");
    // Rename the signed copy into place: never overwrite a live Mach-O inode.
    let previous = release.join("ZeroCode Computer Use.previous.app");
    if previous.exists() {
        std::fs::remove_dir_all(&previous).expect("remove old helper bundle");
    }
    if destination.exists() {
        std::fs::rename(&destination, &previous).expect("move previous helper bundle");
    }
    std::fs::rename(&app, &destination).expect("publish signed helper bundle");
}

fn announce_tree(root: &std::path::Path) {
    println!("cargo:rerun-if-changed={}", root.display());
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.file_name().is_some_and(|name| name == ".build") {
            continue;
        }
        if path.is_dir() {
            announce_tree(&path);
        } else {
            println!("cargo:rerun-if-changed={}", path.display());
        }
    }
}

fn computer_use_info_plist() -> String {
    r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>ZeroCodePermissionTargets</key><string>__PERMISSION_TARGETS__</string>
  <key>CFBundleDevelopmentRegion</key><string>en</string>
  <key>CFBundleExecutable</key><string>zerocode-computer-use-macos</string>
  <key>CFBundleIdentifier</key><string>dev.zerocode.app.computer-use</string>
  <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
  <key>CFBundleIconFile</key><string>AppIcon</string>
  <key>CFBundleName</key><string>ZeroCode Computer Use</string>
  <key>CFBundleDisplayName</key><string>ZeroCode Computer Use</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>1.0</string>
  <key>CFBundleVersion</key><string>1</string>
  <key>LSMinimumSystemVersion</key><string>14.0</string>
  <key>LSUIElement</key><true/>
  <key>NSAccessibilityUsageDescription</key>
  <string>ZeroCode Computer Use needs Accessibility permission to inspect and operate app interfaces only when you ask an agent to use local apps.</string>
  <key>NSScreenCaptureUsageDescription</key>
  <string>ZeroCode Computer Use needs Screen Recording permission to capture app windows only when you ask an agent to inspect visual state.</string>
</dict>
</plist>
"#.replace("__PERMISSION_TARGETS__", &include_str!("native/computer-use-macos/permissions.json").replace('&', "&amp;").replace('<', "&lt;"))
}

/// The nine files that ship, in the one order the digest is taken in.
///
/// The same list `scripts/build-ui-dist.mjs` copies into `ui/dist`, kept
/// here rather than parsed from it: two readers of one list is a list, two
/// spellings of one list is a bug waiting for the day they disagree. The
/// gate below is what makes a disagreement loud instead of shipped.
const UI_FILES: &[&str] = &[
    "favicon.ico",
    "favicon.png",
    "index.html",
    "shell-boot.js",
    "shell-browser.js",
    "shell-computer.js",
    "shell-flow.js",
    "shell-doc.js",
    "shell-i18n.js",
    "shell-input.js",
    "shell-knowledge.js",
    "shell-knowledge-3d.js",
    "shell-knowledge-supply.js",
    "shell-explorer-search.js",
    "shell-explorer-tree.js",
    "shell-attach.js",
    "shell-board.js",
    "shell-board-live.js",
    "shell-composer.js",
    "shell-conversation-view.js",
    "shell-path-browser.js",
    "shell-remote.js",
    "shell-sftp.js",
    "shell-scm.js",
    "shell-settings.js",
    "shell-jev.js",
    "shell-status.js",
    "shell-term.js",
    "shell-term-selection.js",
    "shell-update.js",
    "shell-workspace.js",
    "shell.css",
    "shell.js",
    "tokens.css",
    "vendor/cm6-LICENSE",
    "vendor/cm6.js",
];

/// What this binary IS, written down while the build still knows.
///
/// A release is three layers — the code, the branch it landed on, and the
/// binary a person is actually running — and until this existed the third
/// one could not be ASKED. The UI ships as a curated copy of `ui/` embedded
/// and compressed, so no string in the binary answers "which UI is this":
/// probing a shipped binary for a function name finds nothing whether the
/// UI is current or a year stale, which is the worst kind of answer. And
/// the bundle version is a static `0.1.0`, so it cannot tell two builds
/// apart. Measured 2026-08-24 on a release everyone believed in and nobody
/// could check.
///
/// `-dirty` is the point of the whole thing. "Build only from a committed
/// snapshot" has been our rule since a working-tree build shipped
/// half-edited UI; a rule is a thing people follow, and this makes it a
/// thing the artifact reports.
fn stamp_identity() {
    let manifest = std::path::PathBuf::from(
        std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"),
    );
    let root = manifest
        .parent()
        .and_then(std::path::Path::parent)
        .expect("the workspace root above crates/zerocode-shell")
        .to_path_buf();
    // Tauri's context macro embeds the frontend during the Cargo build. The
    // release overlay points at `ui/dist`, while development points at `ui`;
    // watching only Rust files lets Cargo reuse an older compressed asset set
    // after the HTML/JS/CSS changed. Keep both roads attached to the build
    // graph so an installed app can never show a stale settings pane.
    for name in UI_FILES {
        println!(
            "cargo:rerun-if-changed={}",
            root.join("ui").join(name).display()
        );
        println!(
            "cargo:rerun-if-changed={}",
            root.join("ui/dist").join(name).display()
        );
    }
    println!("cargo:rustc-env=ZEROCODE_COMMIT={}", commit(&manifest));
    println!("cargo:rustc-env=ZEROCODE_UI_DIGEST={}", ui_digest(&root));
    // The list itself, so the gate in `main.rs` can ask what was hashed
    // instead of spelling the list a second time.
    println!("cargo:rustc-env=ZEROCODE_UI_FILES={}", UI_FILES.join(","));
}

/// The snapshot this build stood on, or the honest absence of one.
///
/// `unknown` rather than a fabricated value: a stamp that guesses is worse
/// than a stamp that says it does not know, because only the second one
/// makes anybody go look.
fn commit(manifest: &std::path::Path) -> String {
    let git = |args: &[&str]| -> Option<String> {
        let output = std::process::Command::new("git")
            .current_dir(manifest)
            .args(args)
            .output()
            .ok()?;
        output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
    };
    /* Three files, because one is not enough and the shortfall is silent.
     *
     * `HEAD` holds `ref: refs/heads/<branch>` on an attached checkout, and
     * committing does not change those bytes — watching it alone means the
     * ordinary case (pull, rebuild, ship) rebuilds nothing and stamps the
     * commit the branch used to be on. `logs/HEAD` appends on every HEAD
     * movement, so it moves where `HEAD` does not; the resolved ref file
     * covers a repository with reflogs turned off. A worktree keeps all
     * three outside `.git/`, so ask git where they are rather than
     * assuming. */
    let mut watch = vec!["HEAD".to_string(), "logs/HEAD".to_string()];
    if let Some(name) = git(&["rev-parse", "--symbolic-full-name", "HEAD"])
        && !name.is_empty()
    {
        watch.push(name);
    }
    for name in watch {
        if let Some(path) = git(&["rev-parse", "--git-path", &name]) {
            let path = manifest.join(path);
            // Never announce a path that is not there: cargo reads a missing
            // watch as "changed" and rebuilds on every invocation.
            if path.is_file() {
                println!("cargo:rerun-if-changed={}", path.display());
            }
        }
    }
    let Some(sha) = git(&["rev-parse", "HEAD"]).filter(|sha| !sha.is_empty()) else {
        return "unknown".to_string();
    };
    match git(&["status", "--porcelain"]) {
        Some(dirt) if dirt.is_empty() => sha,
        Some(_) => format!("{sha}-dirty"),
        // `rev-parse` answered and `status` did not: something is wrong with
        // the tree this was built from, and saying so beats a clean-looking
        // sha that was never checked.
        None => format!("{sha}-unverified"),
    }
}

/// The exact bytes of the UI this binary embeds.
///
/// The same chain `scripts/build-ui-dist.mjs` hashes — `name\0bytes\0` over
/// the sorted allowlist — so the two answers are comparable by eye and by
/// script. `unknown` if any file is missing: a stamp that guesses is worse
/// than one that admits.
///
/// `-distdrift` is the other half, and it is the one that would have made
/// this whole stamp a liar. A release does not embed `ui/`; it embeds
/// `ui/dist` (`tauri.release.conf.json`), a copy `scripts/build-ui-dist.mjs`
/// makes. That directory is in `.gitignore`, so a stale one never shows in
/// `git status` and never earns `-dirty` — a build that skipped the npm
/// chain would ship last week's UI and answer with a clean, confident
/// digest of this week's source. So when a dist is present it is measured
/// too, and a disagreement is SAID rather than averaged away.
fn ui_digest(root: &std::path::Path) -> String {
    /* The directory, not just the files under it.
     *
     * Measured 2026-08-24, twice, and both readings mattered. Watching only
     * the eight source files meant the build that ran before a dist existed
     * installed no watch for one — so making a dist, and then editing it,
     * rebuilt nothing and the marker never fired: a drift detector that
     * needs something else to trigger it is not a detector. Announcing the
     * dist path unconditionally fixed that and cost a full recompile of
     * this crate on EVERY cargo invocation (6-11s measured, no changes),
     * which is a tax the whole team pays for a marker almost nobody trips.
     *
     * `ui/` is the cheap answer: it always exists, so it forces nothing,
     * and cargo rescans it. `scripts/build-ui-dist.mjs` replaces the dist
     * by `rename`, which moves this directory's own mtime, so the release
     * road — regenerate the dist, then build — always re-runs this script
     * with the dist it is about to embed. */
    let ui = root.join("ui");
    println!("cargo:rerun-if-changed={}", ui.display());
    let source = digest_of(&ui, true);
    if source == "unknown" {
        return source;
    }
    let dist = root.join("ui").join("dist");
    // A dist that is absent is not a drift: a dev build embeds `ui/` itself
    // and has no dist to disagree with. A release always makes one first,
    // so at the moment that matters this branch is taken.
    if !dist.is_dir() {
        return source;
    }
    if digest_of(&dist, true) == source {
        source
    } else {
        format!("{source}-distdrift")
    }
}

/// The allowlist, hashed under one root.
///
/// `watch` announces each file so an edit under either root is seen. A
/// directory's mtime reports an entry appearing or leaving and says nothing
/// about a byte changing inside one, which is exactly the shape a stale
/// dist has.
fn digest_of(root: &std::path::Path, watch: bool) -> String {
    use sha2::Digest as _;
    let mut names = UI_FILES.to_vec();
    names.sort_unstable();
    let mut digest = sha2::Sha256::new();
    for name in names {
        let path = root.join(name);
        if watch {
            println!("cargo:rerun-if-changed={}", path.display());
        }
        let Ok(bytes) = std::fs::read(&path) else {
            return "unknown".to_string();
        };
        digest.update(name.as_bytes());
        digest.update([0]);
        digest.update(&bytes);
        digest.update([0]);
    }
    format!("{:x}", digest.finalize())
}

fn build_ios_emulator_helper() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    let manifest = std::path::PathBuf::from(
        std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"),
    );
    // One module: the two files at the helper's root, plus the part of it
    // `swift test` builds on its own (`just swift-test`, Package.swift there).
    let helper = manifest.join("native/ios-emulator-helper");
    let inputs = [
        helper.join("main.swift"),
        helper.join("AccessibilityBridge.swift"),
        helper.join("Sources/ZeroCodeIosEmulatorHelperCore/AccessibilityChildTally.swift"),
        helper.join("Sources/ZeroCodeIosEmulatorHelperCore/AccessibilityLeaf.swift"),
    ];
    for input in &inputs {
        println!("cargo:rerun-if-changed={}", input.display());
    }
    let architecture = match std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("aarch64") => "arm64",
        Ok("x86_64") => "x86_64",
        Ok(other) => panic!("unsupported macOS architecture for iOS helper: {other}"),
        Err(error) => panic!("CARGO_CFG_TARGET_ARCH is missing: {error}"),
    };
    let target = format!("{architecture}-apple-macosx14.0");
    use std::hash::{Hash as _, Hasher as _};
    let mut fingerprint = std::collections::hash_map::DefaultHasher::new();
    for input in &inputs {
        std::fs::read(input)
            .expect("read iOS emulator helper source")
            .hash(&mut fingerprint);
    }
    target.hash(&mut fingerprint);
    let output = std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR")).join(
        format!("zerocode-ios-emulator-input-{:016x}", fingerprint.finish()),
    );
    if !output.is_file() {
        let status = std::process::Command::new("xcrun")
            .args(["--sdk", "macosx", "swiftc"])
            .args(&inputs)
            .args(["-O", "-whole-module-optimization", "-target", &target, "-o"])
            .arg(&output)
            .status()
            .expect("failed to launch Swift compiler for iOS emulator input helper");
        assert!(
            status.success(),
            "iOS emulator input helper did not compile"
        );

        // The helper is embedded as bytes and materialized at runtime. An ad-hoc
        // signature keeps the embedded Mach-O acceptable to macOS after that copy.
        let signed = std::process::Command::new("codesign")
            .args(["-s", "-", "-f"])
            .arg(&output)
            .status()
            .expect("failed to launch codesign for iOS emulator input helper");
        assert!(signed.success(), "iOS emulator input helper was not signed");
    }
    println!(
        "cargo:rustc-env=ZEROCODE_IOS_HID_HELPER={}",
        output.display()
    );
}
