//! Artifact identity is captured while compiling, never from a running pane's cwd.
use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    output.status.success().then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn main() {
    for key in ["ZO_BUILD_ID", "SOURCE_DATE_EPOCH"] {
        println!("cargo:rerun-if-env-changed={key}");
    }
    // Include HEAD, the linked worktree's index, loose ref and packed refs.
    // Cargo otherwise cannot see a commit or an index-only change.
    for name in ["HEAD", "index", "packed-refs"] {
        if let Some(path) = git(&["rev-parse", "--git-path", name]) {
            println!("cargo:rerun-if-changed={path}");
        }
    }
    if let Some(reference) = git(&["symbolic-ref", "-q", "HEAD"]) {
        if let Some(path) = git(&["rev-parse", "--git-path", &reference]) {
            println!("cargo:rerun-if-changed={path}");
        }
    }
    // Once explicit rerun paths exist Cargo's default source scan is disabled.
    // The workspace also owns the linked libraries that form this artifact.
    println!("cargo:rerun-if-changed=../../Cargo.toml");
    println!("cargo:rerun-if-changed=../../Cargo.lock");
    println!("cargo:rerun-if-changed=..");
    let sha = git(&["rev-parse", "--verify", "HEAD"]);
    let dirty = sha.as_ref().and_then(|_| git(&["status", "--porcelain", "--untracked-files=normal", "--", "../.."]))
        .map(|changes| (!changes.is_empty()).to_string());
    let id = std::env::var("ZO_BUILD_ID").ok().filter(|value| !value.is_empty()).or_else(|| {
        sha.as_ref().map(|sha| {
            let time = std::env::var("SOURCE_DATE_EPOCH").unwrap_or_else(|_| {
                std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
                    .map_or_else(|_| "unknown".into(), |elapsed| elapsed.as_nanos().to_string())
            });
            format!("{sha}-{time}")
        })
    });
    for (key, value) in [("ZO_BUILD_GIT_SHA", sha), ("ZO_BUILD_DIRTY", dirty), ("ZO_BUILD_ID", id)] {
        // A newline must not become a second Cargo directive.
        let value = value.filter(|value| value.len() <= 256 && !value.chars().any(char::is_control));
        println!("cargo:rustc-env={key}={}", value.as_deref().unwrap_or(""));
    }
}
