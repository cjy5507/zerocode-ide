//! Where the runner comes from: the pinned source, this repository's one small
//! patch, a build on the person's own Mac, kept until what it was built for changes.
//!
//! The order is the point. Nothing is patched before the fetched hashes match, and
//! nothing is built before the patch fits. No script is downloaded and run: what
//! `xcodebuild` builds is the source these hashes name, and git runs no hook of it.

use std::path::{Path, PathBuf};

use zerocode_core::agent_emulator::real_phone::{
    BuildStamp, Team, WDA_COMMIT, WDA_REPOSITORY, WDA_TAG, check_fetched, patch_mismatch,
    profile_expiry, runner_bundle_id,
};

/// The patch: `FBXCTestDaemonsProxy.synthesizeEventWithRecord`, one function. One
/// W3C `/actions` request took 6 s and with it 0.8 s on an iPhone (iOS 26.5,
/// 2026-11-03). That is the request alone: the window's `tap` and `click` read the
/// lock and the screen before it, and were not timed on a phone.
pub(super) const TAP_NO_WAIT_PATCH: &str = include_str!("tap-no-wait.patch");

const STAMP_FILE: &str = "stamp.json";
const SOURCE_DIR: &str = "src";
const BUILD_DIR: &str = "build";

/// The one thing this file asks of the machine. Running it is the caller's, so a
/// test sees every command, in order, without git, Xcode or a network.
pub(super) trait Tools {
    /// Run a program in a directory; its output, or why it failed.
    fn run(&mut self, program: &str, args: &[String], cwd: &Path) -> Result<String, String>;
}

fn words(list: &[&str]) -> Vec<String> {
    list.iter().map(|word| (*word).to_string()).collect()
}

/// What git is told for every fetch: a person's own rules do not apply, and nothing
/// from the source runs.
fn git_args(rest: &[&str]) -> Vec<String> {
    let mut args = words(&[
        "-c",
        "core.hooksPath=/dev/null",
        "-c",
        "transfer.fsckObjects=true",
        "-c",
        "protocol.file.allow=never",
    ]);
    args.extend(words(rest));
    args
}

/// Fetch the pinned commit into `source`, and read the three hashes back.
fn fetch(tools: &mut dyn Tools, source: &Path) -> Result<(), String> {
    let git = |tools: &mut dyn Tools, rest: &[&str]| tools.run("git", &git_args(rest), source);
    git(tools, &["init", "-q"])?;
    let tag_ref = format!("refs/tags/{WDA_TAG}");
    let listed = git(tools, &["ls-remote", WDA_REPOSITORY, &tag_ref])?;
    let tag_commit = listed.split_whitespace().next().unwrap_or_default();
    git(
        tools,
        &["fetch", "-q", "--depth", "1", WDA_REPOSITORY, WDA_COMMIT],
    )?;
    let head = git(tools, &["rev-parse", "FETCH_HEAD"])?;
    let tree = git(tools, &["rev-parse", "FETCH_HEAD^{tree}"])?;
    check_fetched(tag_commit, head.trim(), tree.trim())?;
    git(tools, &["checkout", "-q", "FETCH_HEAD"])?;
    Ok(())
}

/// Whether what stands in `dir` was built for this stamp.
#[must_use]
pub(super) fn built_for(dir: &Path, stamp: &BuildStamp) -> Option<PathBuf> {
    (stamp_in(dir)? == *stamp)
        .then(|| xctestrun_in(dir))
        .flatten()
}

fn stamp_in(dir: &Path) -> Option<BuildStamp> {
    let text = std::fs::read_to_string(dir.join(STAMP_FILE)).ok()?;
    serde_json::from_str(&text).ok()
}

/// The team a standing build was signed with, from its own stamp: what takes the
/// runner off the phone after the person's answer is gone.
#[must_use]
pub(super) fn built_team(dir: &Path) -> Option<String> {
    let kept = stamp_in(dir)?;
    (!kept.team.is_empty()).then_some(kept.team)
}

/// The test plan a build wrote, the file `test-without-building` runs.
#[must_use]
pub(super) fn xctestrun_in(dir: &Path) -> Option<PathBuf> {
    std::fs::read_dir(dir.join(BUILD_DIR).join("Build").join("Products"))
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .find(|path| path.extension().is_some_and(|ext| ext == "xctestrun"))
}

/// The build's own signature's end, for `phone-check`.
#[must_use]
pub(super) fn signature_ends(dir: &Path, tools: &mut dyn Tools) -> Option<String> {
    let products = dir.join(BUILD_DIR).join("Build").join("Products");
    let app = products
        .join("Debug-iphoneos")
        .join("WebDriverAgentRunner-Runner.app")
        .join("embedded.mobileprovision");
    let plist = tools
        .run(
            "security",
            &[
                "cms".to_string(),
                "-D".to_string(),
                "-i".to_string(),
                app.to_string_lossy().into_owned(),
            ],
            dir,
        )
        .ok()?;
    profile_expiry(&plist)
}

/// Build for `team` and this phone, unless what stands already is. Returns the plan.
///
/// # Errors
/// The sentence for the person; a mismatch built nothing.
pub(super) fn prepare(
    tools: &mut dyn Tools,
    dir: &Path,
    team: &Team,
    udid: &str,
    device_os: &str,
) -> Result<PathBuf, String> {
    let stamp = BuildStamp::new(TAP_NO_WAIT_PATCH, &team.id, device_os);
    if let Some(plan) = built_for(dir, &stamp) {
        return Ok(plan);
    }
    let source = dir.join(SOURCE_DIR);
    let _ = std::fs::remove_dir_all(&source);
    std::fs::create_dir_all(&source).map_err(|error| error.to_string())?;
    fetch(tools, &source)?;

    let patch_file = dir.join("tap-no-wait.patch");
    std::fs::write(&patch_file, TAP_NO_WAIT_PATCH).map_err(|error| error.to_string())?;
    let patch_path = patch_file.to_string_lossy().into_owned();
    tools
        .run(
            "git",
            &git_args(&["apply", "--check", &patch_path]),
            &source,
        )
        .map_err(|why| patch_mismatch(&why))?;
    tools
        .run("git", &git_args(&["apply", &patch_path]), &source)
        .map_err(|why| patch_mismatch(&why))?;

    let build = dir.join(BUILD_DIR);
    let args = vec![
        "build-for-testing".to_string(),
        "-project".to_string(),
        "WebDriverAgent.xcodeproj".to_string(),
        "-scheme".to_string(),
        "WebDriverAgentRunner".to_string(),
        "-destination".to_string(),
        format!("id={udid}"),
        "-derivedDataPath".to_string(),
        build.to_string_lossy().into_owned(),
        // Only a build signs, and only this step may ask Apple for a profile.
        "-allowProvisioningUpdates".to_string(),
        format!("DEVELOPMENT_TEAM={}", team.id),
        "CODE_SIGN_STYLE=Automatic".to_string(),
        format!("PRODUCT_BUNDLE_IDENTIFIER={}", runner_bundle_id(&team.id)),
    ];
    tools.run("xcodebuild", &args, &source)?;
    let plan = xctestrun_in(dir)
        .ok_or_else(|| "the build ended and wrote no test plan; tell the person".to_string())?;
    let stamp_text = serde_json::to_string(&stamp).map_err(|error| error.to_string())?;
    std::fs::write(dir.join(STAMP_FILE), stamp_text).map_err(|error| error.to_string())?;
    // The product is under `build`; the source and patch are not needed again.
    let _ = std::fs::remove_dir_all(&source);
    let _ = std::fs::remove_file(&patch_file);
    Ok(plan)
}

/// The arguments that run a built plan: no signing, no build, nothing that could
/// stop to ask a person. This is every start after the first.
#[must_use]
pub(super) fn run_args(plan: &Path, udid: &str) -> Vec<String> {
    vec![
        "test-without-building".to_string(),
        "-xctestrun".to_string(),
        plan.to_string_lossy().into_owned(),
        "-destination".to_string(),
        format!("id={udid}"),
    ]
}
