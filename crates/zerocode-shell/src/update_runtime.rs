//! 「새 빌드 준비됨」 — the window's half of the release lane.
//!
//! The lane (`tools/release/lane.sh`, docs/design/release-lane-off-the-window.md
//! §2.1–2.3) runs under launchd, outside this process, and leaves two files
//! behind in `~/.local/share/zerocode/release/`: `status.json`, overwritten
//! at every phase, and `installed.json`, written only when a swap finished.
//! This module READS them — on the status bar's poll period, never through a
//! watcher, never writing — and judges one thing: is the build this process
//! is running the build that is installed? The answer is a [`Notice`] the
//! window surfaces twice (a sticky toast and the settings notice) and acts on
//! never: the restart is a person's hand, through `relaunch_window`, the one
//! restart road this window has.
//!
//! The file name is the one the gap map (B8, docs/plans/orca-gap-reaudit-
//! 20260905.md) reserved for automatic updates. Feed, signature and download
//! come after the release decision; this is the front half — the machine's
//! own lane and the 「준비됨」 surface.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Where the lane leaves its files, relative to the home directory. Spelled
/// once here and once in `tools/release/lane.sh`'s table; the two agree by
/// the design (§3 rule 6) rather than by a shared constant, because the lane
/// is a shell script launchd runs without this binary.
const RELEASE_DIR: &[&str] = &[".local", "share", "zerocode", "release"];
const STATUS_FILE: &str = "status.json";
const INSTALLED_FILE: &str = "installed.json";

/// A sha shorter than this is a word, not an identity. Seven is git's own
/// abbreviation floor; both producers write the full forty, so this only
/// matters when one side is hand-typed.
const SHA_MIN_CHARS: usize = 7;

// ---- the feed (t-3191, docs/design/versioned-auto-update.md §2.2) ---------

/// Where the public releases live, spelled once. The stable channel reads
/// GitHub's 「latest release」 asset road; the beta channel reads the asset of
/// one moving prerelease tag. The repository was decided 2026-09-08 07:45
/// (design §4.1): the existing public `cjy5507/zerocode`, which also carries
/// an older CLI's releases — ours are told apart by their assets
/// ([`ASSET_PREFIX`]), never by tag or by 「latest」 alone (§2.2 공존 규칙).
pub(crate) struct Feed {
    pub(crate) owner: &'static str,
    pub(crate) repo: &'static str,
    /// The prerelease tag the lane moves for the beta channel.
    pub(crate) beta_tag: &'static str,
    /// The updater manifest at the top of every one of our releases.
    pub(crate) manifest: &'static str,
}

pub(crate) const FEED: Feed = Feed {
    owner: "cjy5507",
    repo: "zerocode",
    beta_tag: "beta",
    manifest: "latest.json",
};

/// What every asset of ours begins with (`ZeroCode_<version>_<arch>.app.tar.gz`
/// and its `.sig`). A release in the same repository without such an asset
/// is the older CLI's, and a `latest.json` whose platform asset is not ours
/// is read as 「이 플랫폼의 자산이 아직 없음」 before any signature is looked at.
pub(crate) const ASSET_PREFIX: &str = "ZeroCode_";

/// The word `tauri.conf.json` carries in `plugins.updater.pubkey` until the
/// lane machine has generated a key (U-A's procedure). Downloads refuse with
/// [`FailureWord::SigningKeyUnset`] while it stands; checks still work, since
/// the plugin reads the key only at verification.
pub(crate) const PUBKEY_UNSET: &str = "UNSET";

/// The channel a window follows. Serialized as the settings row spells it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum UpdateChannel {
    #[default]
    Stable,
    Beta,
}

/// What the window does with a newer version (design §2.3). `Ask` is Orca's
/// `autoDownload = false`: say so, and let a person choose to download.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum UpdatePolicy {
    #[default]
    Ask,
    Auto,
    Off,
}

/// The feed URL for a channel, rendered from [`FEED`]. The stable one is what
/// `tauri.conf.json` carries verbatim (the test below pins the two equal);
/// the beta one is chosen at check time and never configured.
pub(crate) fn feed_endpoint(channel: UpdateChannel) -> String {
    match channel {
        UpdateChannel::Stable => format!(
            "https://github.com/{}/{}/releases/latest/download/{}",
            FEED.owner, FEED.repo, FEED.manifest
        ),
        UpdateChannel::Beta => format!(
            "https://github.com/{}/{}/releases/download/{}/{}",
            FEED.owner, FEED.repo, FEED.beta_tag, FEED.manifest
        ),
    }
}

/// The release-list road of the same repository (GitHub's REST API, public,
/// unauthenticated). `api_base` is a parameter so a test can point it at a
/// server of its own; the window passes [`GITHUB_API`].
pub(crate) fn releases_api_url(api_base: &str, per_page: usize) -> String {
    format!(
        "{}/repos/{}/{}/releases?per_page={per_page}",
        api_base.trim_end_matches('/'),
        FEED.owner,
        FEED.repo
    )
}

pub(crate) const GITHUB_API: &str = "https://api.github.com";

// ---- the clock and the cache, as a table (design §2.3, §2.2) --------------

/// The first feed check after boot, in seconds. One minute: after the window
/// has painted and the agents have started, before anyone has waited.
pub(crate) const CHECK_AFTER_BOOT_SECS: u64 = 60;
/// Then every six hours. The window knocks on the status bar's own period
/// (`USAGE_AMBIENT_MS`, fifteen minutes) and this table decides whether a
/// knock is a check — no thread and no timer of its own.
pub(crate) const CHECK_EVERY_SECS: u64 = 6 * 60 * 60;
/// How long the cached release list stands before the API is asked again.
pub(crate) const HISTORY_TTL_SECS: u64 = 6 * 60 * 60;
/// How many releases one history page asks for. GitHub's default page is
/// thirty; ours are the newest thirty *of ours*, after the older CLI's rows
/// are filtered out, so the ask is wider than the show.
pub(crate) const HISTORY_PER_PAGE: usize = 30;
/// Where the release list is cached, relative to the home directory. Beside
/// the lane's directory, not inside it (§3 rule 7: the window never writes
/// under `release/`).
pub(crate) const UPDATE_DIR: &[&str] = &[".local", "share", "zerocode", "update"];
pub(crate) const RELEASES_CACHE_FILE: &str = "releases.json";

/// What this process was built from: the three stamps `build.rs` fixes at
/// compile time (`stamp_identity`), given to the window as one object. The
/// commit is the raw stamp — `<sha>`, `<sha>-dirty` or `<sha>-unverified` —
/// so the window can show exactly what `--version` prints.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct BuildStamp {
    pub(crate) version: String,
    pub(crate) commit: String,
    pub(crate) ui_digest: String,
}

pub(crate) fn build_stamp() -> BuildStamp {
    BuildStamp {
        version: env!("CARGO_PKG_VERSION").into(),
        commit: env!("ZEROCODE_COMMIT").into(),
        ui_digest: env!("ZEROCODE_UI_DIGEST").into(),
    }
}

/// One half of `installed.json`: `{sha, at, version}`. The app and zo
/// halves are written separately by the lane (an app build can be red while
/// zo was swapped), so each is optional on its own.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct InstalledBuild {
    pub(crate) sha: String,
    #[serde(default)]
    pub(crate) at: Option<String>,
    /// The version the lane built the sha from (t-3237): the scratch's
    /// `[workspace.package] version`, the letters `status.json` carries.
    /// `None` for a file from before the field, or the empty string the
    /// lane writes for a half it did not swap since — the window's
    /// 「unknown」, which falls back to the sha sentence.
    #[serde(default)]
    pub(crate) version: Option<String>,
}

/// `installed.json` as the lane writes it: `{app:{sha, at, version},
/// zo:{sha, at, version}}`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Installed {
    #[serde(default)]
    pub(crate) app: Option<InstalledBuild>,
    #[serde(default)]
    pub(crate) zo: Option<InstalledBuild>,
}

impl Installed {
    /// Read leniently: a half whose `sha` is not a string is a half that is
    /// not there. The window never refuses the file — a lane mid-write or an
    /// older shape is silence, not an error toast.
    pub(crate) fn from_value(value: Option<&Value>) -> Self {
        let half = |name: &str| -> Option<InstalledBuild> {
            let entry = value?.get(name)?;
            let sha = entry.get("sha")?.as_str()?.trim();
            (!sha.is_empty()).then(|| InstalledBuild {
                sha: sha.to_string(),
                at: entry.get("at").and_then(Value::as_str).map(str::to_string),
                version: entry
                    .get("version")
                    .and_then(Value::as_str)
                    .and_then(version_word),
            })
        };
        Self {
            app: half("app"),
            zo: half("zo"),
        }
    }
}

/// A version word the sentence can carry: trimmed, non-empty.
fn version_word(text: &str) -> Option<String> {
    let word = text.trim();
    (!word.is_empty()).then(|| word.to_string())
}

/// One zo pane's build as its handshake receipt said it: `build.git_sha`
/// and `build.version` (an older zo, or a receipt without one, says no
/// version).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct RunningZo {
    pub(crate) sha: String,
    pub(crate) version: Option<String>,
}

/// What the sentence says moved (t-3237). The judgement is the sha's either
/// way; this only picks the words: `Version` when the lane wrote a version
/// beside the installed sha and something running is not it — 「새 버전
/// {{version}}이(가) 설치되었습니다」 — and `Build` when the version is the
/// running one, or the lane did not say — 「새 빌드({{sha}})가
/// 설치되었습니다」. Serialized as the word the window switches on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Change {
    Version,
    Build,
}

pub(crate) fn change_of<'a>(
    installed: Option<&str>,
    running: impl IntoIterator<Item = &'a str>,
) -> Change {
    match installed {
        Some(installed) if running.into_iter().any(|version| version != installed) => {
            Change::Version
        }
        _ => Change::Build,
    }
}

/// The app half of a notice: the sha the lane installed and the sha this
/// process runs, both plain (`-dirty` off), with the version beside each —
/// the lane's word for the installed one (`None` when it did not say) and
/// the compiled `CARGO_PKG_VERSION` for the running one — and the change
/// the words name.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct AppDrift {
    pub(crate) installed: String,
    pub(crate) running: String,
    pub(crate) installed_version: Option<String>,
    pub(crate) running_version: String,
    pub(crate) change: Change,
}

/// The zo half: the installed sha and every distinct sha a running zo pane
/// reported through its handshake that is not it. A pane opened after the
/// swap already runs the new binary and is not listed. `running_versions`
/// are the versions those panes said, distinct, in the same order, for the
/// panes that said one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ZoDrift {
    pub(crate) installed: String,
    pub(crate) running: Vec<String>,
    pub(crate) installed_version: Option<String>,
    pub(crate) running_versions: Vec<String>,
    pub(crate) change: Change,
}

/// What the window says. At least one half is present, or there is no
/// notice at all — [`update_notice`] returns `None` rather than an empty
/// notice, so the callers' 「once per sha」 memory has a sha to remember.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Notice {
    pub(crate) app: Option<AppDrift>,
    pub(crate) zo: Option<ZoDrift>,
}

/// A sha the judgement can stand on: the `-dirty` mark off (the crash
/// report's rule, [`crate::crash::plain_sha`]), lowercased, hex, at least
/// seven characters. `unknown` — what `build.rs` stamps when there is no git
/// under it — and `<sha>-unverified` both fail this, and a build whose
/// identity is not known is not a build that differs from anything: saying
/// 「새 빌드 준비됨」 on every boot of an unstamped binary would be a nag
/// that teaches people to dismiss the real one.
fn judged_sha(stamp: &str) -> Option<String> {
    let plain = crate::crash::plain_sha(stamp.trim()).to_ascii_lowercase();
    (plain.len() >= SHA_MIN_CHARS && plain.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then_some(plain)
}

/// Two judged shas name the same commit when one is a prefix of the other.
/// Both producers write the full sha today; the tolerance is for the day one
/// of them abbreviates, so that a shortened stamp does not read as a new
/// build forever.
fn same_commit(left: &str, right: &str) -> bool {
    left.starts_with(right) || right.starts_with(left)
}

/// The judgement, pure: this process against what the lane installed.
///
/// - `app`: [`BuildStamp::commit`] against `installed.app.sha`.
/// - `zo`: every sha in `zo_running` (the `build.git_sha` of each pane's
///   handshake receipt) against `installed.zo.sha`; the ones that differ
///   are listed, distinct, in the order given.
///
/// The sha judges; the versions only ride along for the words (t-3237): a
/// sha the lane installed under a version nobody runs is 「새 버전」, the
/// same version again is 「새 빌드」, and the same sha under any version
/// word is no notice at all.
///
/// A missing half of `installed.json` judges nothing for that half. A stamp
/// that cannot be judged (see [`judged_sha`]) judges nothing either. Equal is
/// `None`: no notice is the ordinary state of a window, and every caller
/// treats `None` as 「the condition is gone」.
pub(crate) fn update_notice(
    running: &BuildStamp,
    installed: &Installed,
    zo_running: &[RunningZo],
) -> Option<Notice> {
    let app = installed.app.as_ref().and_then(|build| {
        let installed = judged_sha(&build.sha)?;
        let running_sha = judged_sha(&running.commit)?;
        (!same_commit(&installed, &running_sha)).then(|| AppDrift {
            installed,
            running: running_sha,
            installed_version: build.version.clone(),
            running_version: running.version.clone(),
            change: change_of(build.version.as_deref(), [running.version.as_str()]),
        })
    });
    let zo = installed.zo.as_ref().and_then(|build| {
        let installed = judged_sha(&build.sha)?;
        let mut running = Vec::new();
        let mut running_versions: Vec<String> = Vec::new();
        for pane in zo_running {
            let Some(sha) = judged_sha(&pane.sha) else {
                continue;
            };
            if same_commit(&installed, &sha) {
                continue;
            }
            if !running.contains(&sha) {
                running.push(sha);
            }
            if let Some(version) = pane.version.as_deref().and_then(version_word)
                && !running_versions.contains(&version)
            {
                running_versions.push(version);
            }
        }
        (!running.is_empty()).then(|| ZoDrift {
            installed,
            change: change_of(
                build.version.as_deref(),
                running_versions.iter().map(String::as_str),
            ),
            running,
            installed_version: build.version.clone(),
            running_versions,
        })
    });
    (app.is_some() || zo.is_some()).then_some(Notice { app, zo })
}

// ---- the judgement (t-3191, design §2.3) ------------------------------------

/// Why this machine's window does not ask the feed at all: the build has no
/// judged sha (`-dirty`, `-unverified`, `unknown`), or the sha is the one the
/// local release lane installed — on that machine the t-3005 road
/// (「새 빌드 준비됨」) speaks instead, from the same `build_stamp`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DevBuild {
    Unstamped,
    LaneInstalled,
}

pub(crate) fn dev_build(running: &BuildStamp, installed: &Installed) -> Option<DevBuild> {
    // `judged_sha` reads a `-dirty` build as its commit, which is right for
    // the lane's drift (the crash report's rule) and wrong here: a build with
    // uncommitted changes is nobody's release, whatever sha it started from.
    if running.commit.trim().ends_with("-dirty") {
        return Some(DevBuild::Unstamped);
    }
    let Some(sha) = judged_sha(&running.commit) else {
        return Some(DevBuild::Unstamped);
    };
    installed
        .app
        .as_ref()
        .and_then(|build| judged_sha(&build.sha))
        .filter(|lane| same_commit(lane, &sha))
        .map(|_| DevBuild::LaneInstalled)
}

/// Who knocked. The status bar's clock knocks every fifteen minutes and this
/// table decides; the pane's arrival asks unless the policy is off; the
/// button always asks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Knock {
    Clock,
    PaneOpened,
    Button,
}

/// The two numbers the clock judgement needs, both in seconds: how long this
/// process has been up, and how long since the last check — `None` when the
/// feed was never asked (or the stored clock word does not parse).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Clock {
    pub(crate) boot_secs: u64,
    pub(crate) since_last_check_secs: Option<u64>,
}

pub(crate) fn check_is_due(policy: UpdatePolicy, knock: Knock, clock: Clock) -> bool {
    match (knock, policy) {
        (Knock::Button, _) => true,
        (_, UpdatePolicy::Off) => false,
        (Knock::PaneOpened, _) => true,
        (Knock::Clock, _) => {
            clock.boot_secs >= CHECK_AFTER_BOOT_SECS
                && clock
                    .since_last_check_secs
                    .is_none_or(|since| since >= CHECK_EVERY_SECS)
        }
    }
}

/// What a check found, once the plugin's answer is read: the version it
/// announced (with the notes the lane wrote from the CHANGELOG and the file
/// name of our platform's asset), or one of the four words.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Announced {
    pub(crate) version: String,
    pub(crate) notes: Option<String>,
    pub(crate) pub_date: Option<String>,
    pub(crate) asset: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Found {
    UpToDate,
    Newer(Announced),
    NothingPublished,
    NoAssetForPlatform,
    Failed(FailureWord),
}

/// What the window should do next with a verdict. `Announce` is the ask
/// policy's toast (「내려받기」 is a person's hand); `Download` is the auto
/// policy's quiet road. The window follows; it never decides.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum NextStep {
    Announce,
    Download,
}

/// The words a failure leaves in the pane until the next check. The plugin's
/// many variants fold to these; the 403 word belongs to the history road
/// (GitHub's API rate limit), the signature words to the download.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum FailureWord {
    Network,
    Forbidden,
    Signature,
    SigningKeyUnset,
    Disk,
    Malformed,
    NoAssetForPlatform,
    NothingPublished,
    NotABundle,
    Unsupported,
}

/// Where the update stands, as one tagged value the pane paints from.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "phase")]
pub(crate) enum Phase {
    #[default]
    Idle,
    DevBuild {
        reason: DevBuild,
    },
    Checking,
    UpToDate,
    Available {
        announced: Announced,
    },
    Skipped {
        announced: Announced,
    },
    Downloading {
        version: String,
        received_bytes: u64,
        total_bytes: Option<u64>,
        percent: Option<u8>,
    },
    Downloaded {
        version: String,
    },
    Ready {
        version: String,
    },
    NothingPublished,
    NoAssetForPlatform,
    Failed {
        word: FailureWord,
        detail: String,
    },
}

/// The verdict over a check: the phase to stand in, what the window does
/// next, and whether the skipped version is released (a newer one arrived).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Verdict {
    pub(crate) phase: Phase,
    pub(crate) next: Option<NextStep>,
    pub(crate) clear_skip: bool,
}

pub(crate) fn judge_found(policy: UpdatePolicy, found: Found, skipped: Option<&str>) -> Verdict {
    match found {
        Found::Newer(announced) => {
            if skipped.is_some_and(|held| held == announced.version) {
                return Verdict {
                    phase: Phase::Skipped { announced },
                    next: None,
                    clear_skip: false,
                };
            }
            let next = match policy {
                UpdatePolicy::Auto => NextStep::Download,
                UpdatePolicy::Ask | UpdatePolicy::Off => NextStep::Announce,
            };
            Verdict {
                phase: Phase::Available { announced },
                next: Some(next),
                clear_skip: skipped.is_some(),
            }
        }
        Found::UpToDate => silent(Phase::UpToDate),
        Found::NothingPublished => silent(Phase::NothingPublished),
        Found::NoAssetForPlatform => silent(Phase::NoAssetForPlatform),
        Found::Failed(word) => silent(Phase::Failed {
            word,
            detail: String::new(),
        }),
    }
}

fn silent(phase: Phase) -> Verdict {
    Verdict {
        phase,
        next: None,
        clear_skip: false,
    }
}

/// The file name at the end of a URL's path, without its query.
pub(crate) fn url_file_name(url: &str) -> Option<&str> {
    let path = url.split(['?', '#']).next()?;
    let (scheme, rest) = path.split_once("://")?;
    if scheme.is_empty() || rest.is_empty() {
        return None;
    }
    let name = rest.rsplit('/').next()?;
    (!name.is_empty() && rest.contains('/')).then_some(name)
}

/// §2.2 공존 규칙: the platform asset the feed points at is ours when its
/// file name carries [`ASSET_PREFIX`]. Anything else — the older CLI's
/// binaries under the same 「latest」, the bundler's unversioned name — is
/// 「이 플랫폼의 자산이 아직 없음」, judged before any signature is looked at.
pub(crate) fn asset_is_ours(url: &str) -> bool {
    url_file_name(url).is_some_and(|name| name.starts_with(ASSET_PREFIX))
}

/// The plugin's error, folded to a word. Pure over the variants: `Network`
/// carries the status text, which is how a 403 is told apart.
pub(crate) fn failure_word(error: &tauri_plugin_updater::Error) -> FailureWord {
    use tauri_plugin_updater::Error as E;
    match error {
        E::ReleaseNotFound => FailureWord::NothingPublished,
        E::TargetNotFound(_) | E::TargetsNotFound(_) => FailureWord::NoAssetForPlatform,
        E::Network(text) => history_failure(status_in(text)),
        E::Reqwest(_) => FailureWord::Network,
        E::Io(_) | E::TempDirNotFound | E::TempDirNotOnSameMountPoint => FailureWord::Disk,
        E::Minisign(_) | E::SignatureUtf8(_) | E::Base64(_) => FailureWord::Signature,
        E::Semver(_) | E::Serialization(_) | E::UrlParse(_) | E::InvalidUpdaterFormat => {
            FailureWord::Malformed
        }
        _ => FailureWord::Unsupported,
    }
}

/// The HTTP status a `Network` text names, if any (`… status: 403 Forbidden`).
fn status_in(text: &str) -> Option<u16> {
    text.split(|c: char| !c.is_ascii_digit())
        .filter(|word| word.len() == 3)
        .find_map(|word| word.parse::<u16>().ok())
        .filter(|status| (100..600).contains(status))
}

/// The history road's failure word from its HTTP status: GitHub answers a
/// spent unauthenticated quota with 403 (and 429), which the pane names so
/// nobody reads a rate limit as a dead network.
pub(crate) fn history_failure(status: Option<u16>) -> FailureWord {
    match status {
        Some(403 | 429) => FailureWord::Forbidden,
        _ => FailureWord::Network,
    }
}

/// A percent, only when the total is known and non-zero; never past 100.
pub(crate) fn progress(received: u64, total: Option<u64>) -> Option<u8> {
    let total = total.filter(|total| *total > 0)?;
    let percent = received.saturating_mul(100) / total;
    Some(u8::try_from(percent.min(100)).unwrap_or(100))
}

// ---- the history (design §2.2) ----------------------------------------------

/// One release of ours, as the pane lists it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Release {
    pub(crate) tag: String,
    pub(crate) name: String,
    pub(crate) body: String,
    pub(crate) published_at: String,
    pub(crate) prerelease: bool,
}

/// §2.2 공존 규칙: a release is ours when it carries both the manifest and an
/// asset of ours — a `latest.json` alone or an archive alone is a half
/// publish, and the older CLI's rows carry neither.
pub(crate) fn release_is_ours<'a>(asset_names: impl Iterator<Item = &'a str>) -> bool {
    let (mut manifest, mut archive) = (false, false);
    for name in asset_names {
        manifest |= name == FEED.manifest;
        archive |= name.starts_with(ASSET_PREFIX) && name.ends_with(".app.tar.gz");
    }
    manifest && archive
}

/// GitHub's release list, read leniently and filtered to ours, in the API's
/// order (newest first). Anything that is not an object, or not ours, is
/// not a row.
pub(crate) fn read_releases(list: &Value) -> Vec<Release> {
    let text = |value: &Value, key: &str| {
        value
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    list.as_array()
        .map(|rows| {
            rows.iter()
                .filter(|row| row.is_object())
                .filter(|row| {
                    let names = row
                        .get("assets")
                        .and_then(Value::as_array)
                        .map(|assets| {
                            assets
                                .iter()
                                .filter_map(|asset| asset.get("name").and_then(Value::as_str))
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default();
                    release_is_ours(names.into_iter())
                })
                .map(|row| Release {
                    tag: text(row, "tag_name"),
                    name: text(row, "name"),
                    body: text(row, "body"),
                    published_at: text(row, "published_at"),
                    prerelease: row
                        .get("prerelease")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// `~/.local/share/zerocode/update/releases.json`: when the list was fetched
/// (the tree's ISO shape, `civil::iso_utc_of`) and the rows of ours.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct HistoryCache {
    pub(crate) fetched_at: String,
    pub(crate) releases: Vec<Release>,
}

/// Fresh while `fetched_at + HISTORY_TTL_SECS` is still ahead of `now`. A
/// stamp that does not parse, or one from the future (a clock that walked
/// backwards), is stale — the API is asked again, which is the safe side.
pub(crate) fn cache_is_fresh(cache: &HistoryCache, now_ms: i64) -> bool {
    let Some(fetched) = zerocode_core::civil::epoch_ms_of_iso(&cache.fetched_at) else {
        return false;
    };
    let ttl_ms = i64::try_from(HISTORY_TTL_SECS).unwrap_or(i64::MAX) * 1_000;
    fetched <= now_ms && now_ms < fetched.saturating_add(ttl_ms)
}

pub(crate) fn update_dir_under(home: &Path) -> PathBuf {
    UPDATE_DIR
        .iter()
        .fold(home.to_path_buf(), |dir, part| dir.join(part))
}

/// `~/.local/share/zerocode/update`, or `None` on a machine with no home.
pub(crate) fn update_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|home| update_dir_under(&home))
}

/// The verified archive's name on disk, beside the cache: the lane's asset
/// name without the arch (one machine downloads one arch).
pub(crate) fn archive_file_name(version: &str) -> String {
    format!("{ASSET_PREFIX}{version}.app.tar.gz")
}

// ---- the bundle (design §2.3 내려받기) --------------------------------------

/// The `.app` this executable runs inside — the nearest ancestor with that
/// extension. `None` for a bare binary (`target/debug/…`), which is a dev
/// build and never asks the feed anyway.
pub(crate) fn app_bundle_of(exe: &Path) -> Option<PathBuf> {
    exe.ancestors()
        .find(|dir| dir.extension().is_some_and(|ext| ext == "app"))
        .map(Path::to_path_buf)
}

fn beside(bundle: &Path, shape: impl Fn(&str) -> String) -> PathBuf {
    let name = bundle
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("ZeroCode.app");
    bundle.parent().unwrap_or(Path::new("/")).join(shape(name))
}

/// Where the update is unpacked, beside the bundle: `.ZeroCode.app.new`.
/// Hidden by the dot, so Finder and Spotlight do not offer a half-unpacked
/// app; on the same volume, so the swap is one rename.
pub(crate) fn staged_bundle_path(bundle: &Path) -> PathBuf {
    beside(bundle, |name| format!(".{name}.new"))
}

/// Where the running bundle goes when the swap happens: `ZeroCode.app.old`,
/// kept — the running process still maps its binary from there, and a
/// person can drag it back.
pub(crate) fn retired_bundle_path(bundle: &Path) -> PathBuf {
    beside(bundle, |name| format!("{name}.old"))
}

/// The lane's directory under a given home. Separate from
/// [`release_dir`] so a test can point it at a temporary directory without
/// touching `$HOME`.
pub(crate) fn release_dir_under(home: &Path) -> PathBuf {
    RELEASE_DIR
        .iter()
        .fold(home.to_path_buf(), |dir, part| dir.join(part))
}

/// What the local lane installed, read now (read-only, lenient): the app
/// half is what tells a lane-installed build from a public one.
pub(crate) fn lane_installed() -> Installed {
    let value = release_dir().and_then(|dir| read_json(&dir.join(INSTALLED_FILE)));
    Installed::from_value(value.as_ref())
}

/// `~/.local/share/zerocode/release`, or `None` on a machine with no home.
/// Deliberately NOT the app's own local-data root (`~/Library/Application
/// Support/dev.zerocode.app`): the lane is a shell script under launchd that
/// knows nothing of Tauri's path resolver, and its files live where the
/// coordinator's release scripts always have.
pub(crate) fn release_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|home| release_dir_under(&home))
}

/// One JSON file, or nothing. Missing, unreadable, not UTF-8, not JSON —
/// every one of those is `None`, because the window is a reader of another
/// process's files and none of those is the window's error to raise. A file
/// the lane is halfway through writing is the ordinary case, not a fault.
pub(crate) fn read_json(path: &Path) -> Option<Value> {
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// What `release_status` hands the window: both files as read (`null` when
/// absent or malformed) and the judgement over the installed one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ReleaseStatus {
    pub(crate) status: Value,
    pub(crate) installed: Value,
    pub(crate) notice: Option<Notice>,
    /// Who a restart would cut, read by the command from the one census
    /// (t-3058, t-6428) — what the notice says beside its button. Nobody for
    /// the pure judgement over the files.
    #[serde(default)]
    pub(crate) busy: crate::orchestration::restart_census::Busy,
}

/// Read the lane's two files under `dir` and judge them against `running`
/// and the zo panes. Pure over the file system: no state, no caching — two
/// small reads on a fifteen-minute clock.
pub(crate) fn release_status_in(
    dir: &Path,
    running: &BuildStamp,
    zo_running: &[RunningZo],
) -> ReleaseStatus {
    let status = read_json(&dir.join(STATUS_FILE));
    let installed = read_json(&dir.join(INSTALLED_FILE));
    let notice = update_notice(
        running,
        &Installed::from_value(installed.as_ref()),
        zo_running,
    );
    ReleaseStatus {
        status: status.unwrap_or(Value::Null),
        installed: installed.unwrap_or(Value::Null),
        notice,
        busy: crate::orchestration::restart_census::Busy::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// §2.2: the endpoint `tauri.conf.json` carries is the one the table
    /// renders — two spellings of one URL would drift on the day the
    /// repository moves. The beta endpoint is the moving tag's asset road.
    #[test]
    fn the_configured_endpoint_is_rendered_from_the_feed_table() {
        let conf: Value = serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        let endpoints = conf["plugins"]["updater"]["endpoints"].as_array().unwrap();
        assert_eq!(endpoints.len(), 1);
        assert_eq!(endpoints[0], json!(feed_endpoint(UpdateChannel::Stable)));
        assert_eq!(
            feed_endpoint(UpdateChannel::Stable),
            "https://github.com/cjy5507/zerocode/releases/latest/download/latest.json"
        );
        assert_eq!(
            feed_endpoint(UpdateChannel::Beta),
            "https://github.com/cjy5507/zerocode/releases/download/beta/latest.json"
        );
        let pubkey = conf["plugins"]["updater"]["pubkey"].as_str().unwrap();
        assert!(!pubkey.trim().is_empty(), "never an empty key field");
        assert_eq!(
            releases_api_url(GITHUB_API, 30),
            "https://api.github.com/repos/cjy5507/zerocode/releases?per_page=30"
        );
        assert_eq!(
            releases_api_url("http://127.0.0.1:9/", 2),
            "http://127.0.0.1:9/repos/cjy5507/zerocode/releases?per_page=2"
        );
    }

    const OLD: &str = "9b576e43aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const NEW: &str = "abc1234bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const ZO_OLD: &str = "5205ab00cccccccccccccccccccccccccccccccc";
    const ZO_NEW: &str = "e2371aabdddddddddddddddddddddddddddddddd";

    fn running(commit: &str) -> BuildStamp {
        BuildStamp {
            version: "0.1.0".into(),
            commit: commit.into(),
            ui_digest: "ui".into(),
        }
    }

    fn half(sha: &str, version: Option<&str>) -> InstalledBuild {
        InstalledBuild {
            sha: sha.into(),
            at: Some("2026-09-07T17:40:00+09:00".into()),
            version: version.map(str::to_string),
        }
    }

    /// The file as a lane from before t-3237 wrote it: shas, no versions.
    fn installed(app: Option<&str>, zo: Option<&str>) -> Installed {
        Installed {
            app: app.map(|sha| half(sha, None)),
            zo: zo.map(|sha| half(sha, None)),
        }
    }

    /// The file as the lane writes it now: a version beside each sha.
    fn versioned(app: Option<(&str, &str)>, zo: Option<(&str, &str)>) -> Installed {
        Installed {
            app: app.map(|(sha, version)| half(sha, Some(version))),
            zo: zo.map(|(sha, version)| half(sha, Some(version))),
        }
    }

    /// zo panes whose receipts said a sha and no version (an older zo).
    fn shas(list: &[&str]) -> Vec<RunningZo> {
        list.iter()
            .map(|sha| RunningZo {
                sha: (*sha).to_string(),
                version: None,
            })
            .collect()
    }

    /// zo panes whose receipts said both.
    fn panes(list: &[(&str, &str)]) -> Vec<RunningZo> {
        list.iter()
            .map(|(sha, version)| RunningZo {
                sha: (*sha).to_string(),
                version: Some((*version).to_string()),
            })
            .collect()
    }

    #[test]
    fn app_only_drift_names_installed_and_running() {
        let notice = update_notice(&running(OLD), &installed(Some(NEW), None), &[]);
        assert_eq!(
            notice,
            Some(Notice {
                app: Some(AppDrift {
                    installed: NEW.into(),
                    running: OLD.into(),
                    installed_version: None,
                    running_version: "0.1.0".into(),
                    change: Change::Build,
                }),
                zo: None,
            })
        );
    }

    /// t-3237: the judgement is the sha's; the version the lane wrote
    /// beside it rides along and picks the sentence — a version that is
    /// not the running one is 「새 버전 {{version}}」, the same version (or a
    /// lane that did not say) is 「새 빌드({{sha}})」.
    #[test]
    fn the_version_the_lane_wrote_rides_the_app_notice_and_names_the_change() {
        let moved = update_notice(&running(OLD), &versioned(Some((NEW, "0.2.0")), None), &[])
            .expect("a new sha")
            .app
            .expect("the app half");
        assert_eq!(moved.installed_version.as_deref(), Some("0.2.0"));
        assert_eq!(moved.running_version, "0.1.0");
        assert_eq!(moved.change, Change::Version);
        assert_eq!(
            (moved.installed.as_str(), moved.running.as_str()),
            (NEW, OLD)
        );

        let same = update_notice(&running(OLD), &versioned(Some((NEW, "0.1.0")), None), &[])
            .expect("a new sha of the same version is still a new build")
            .app
            .unwrap();
        assert_eq!(same.installed_version.as_deref(), Some("0.1.0"));
        assert_eq!(same.change, Change::Build);

        // The sha judges, not the version: the same commit under another
        // version word (a hand-edited file) is no notice at all.
        assert_eq!(
            update_notice(&running(OLD), &versioned(Some((OLD, "0.2.0")), None), &[]),
            None
        );
        // And a version the lane did not write is the sha sentence.
        let unsaid = update_notice(&running(OLD), &installed(Some(NEW), None), &[])
            .unwrap()
            .app
            .unwrap();
        assert_eq!(
            (unsaid.installed_version, unsaid.change),
            (None, Change::Build)
        );
    }

    /// The zo half says the versions of the panes that differ, distinct and
    /// in pane order; the change is `Version` as soon as one of them runs a
    /// version that is not the installed one.
    #[test]
    fn the_zo_notice_carries_the_versions_of_the_panes_that_differ() {
        let zo = update_notice(
            &running(OLD),
            &versioned(None, Some((ZO_NEW, "0.2.0"))),
            &panes(&[
                (ZO_OLD, "0.1.0"),
                (ZO_OLD, "0.1.0"),
                (ZO_NEW, "0.2.0"),
                (NEW, "0.2.0"),
            ]),
        )
        .expect("two panes differ")
        .zo
        .expect("the zo half");
        assert_eq!(zo.installed_version.as_deref(), Some("0.2.0"));
        assert_eq!(zo.running, vec![ZO_OLD.to_string(), NEW.to_string()]);
        assert_eq!(
            zo.running_versions,
            vec!["0.1.0".to_string(), "0.2.0".to_string()]
        );
        assert_eq!(zo.change, Change::Version);

        // Every differing pane already runs the installed version: a new
        // build of it.
        let same = update_notice(
            &running(OLD),
            &versioned(None, Some((ZO_NEW, "0.2.0"))),
            &panes(&[(ZO_OLD, "0.2.0")]),
        )
        .unwrap()
        .zo
        .unwrap();
        assert_eq!(same.running_versions, vec!["0.2.0".to_string()]);
        assert_eq!(same.change, Change::Build);

        // Panes that said no version: the sentence cannot claim a move.
        let unsaid = update_notice(
            &running(OLD),
            &versioned(None, Some((ZO_NEW, "0.2.0"))),
            &shas(&[ZO_OLD]),
        )
        .unwrap()
        .zo
        .unwrap();
        assert_eq!(unsaid.running_versions, Vec::<String>::new());
        assert_eq!(unsaid.change, Change::Build);
    }

    #[test]
    fn the_change_is_a_version_only_when_something_running_is_not_it() {
        assert_eq!(change_of(Some("0.2.0"), ["0.1.0"]), Change::Version);
        assert_eq!(
            change_of(Some("0.2.0"), ["0.2.0", "0.1.0"]),
            Change::Version
        );
        assert_eq!(change_of(Some("0.2.0"), ["0.2.0"]), Change::Build);
        assert_eq!(change_of(Some("0.2.0"), []), Change::Build);
        assert_eq!(change_of(None, ["0.1.0"]), Change::Build);
        assert_eq!(
            serde_json::to_value(Change::Version).unwrap(),
            json!("version"),
            "the window reads the word"
        );
    }

    #[test]
    fn zo_only_drift_lists_each_differing_pane_once() {
        let notice = update_notice(
            &running(OLD),
            &installed(Some(OLD), Some(ZO_NEW)),
            &shas(&[ZO_OLD, ZO_OLD, ZO_NEW, "unknown"]),
        );
        assert_eq!(
            notice,
            Some(Notice {
                app: None,
                zo: Some(ZoDrift {
                    installed: ZO_NEW.into(),
                    running: vec![ZO_OLD.into()],
                    installed_version: None,
                    running_versions: vec![],
                    change: Change::Build,
                }),
            })
        );
    }

    #[test]
    fn both_halves_drift_together() {
        let notice = update_notice(
            &running(OLD),
            &installed(Some(NEW), Some(ZO_NEW)),
            &shas(&[ZO_OLD]),
        )
        .expect("both halves differ");
        assert_eq!(notice.app.as_ref().map(|a| a.installed.as_str()), Some(NEW));
        assert_eq!(
            notice.zo.as_ref().map(|z| z.installed.as_str()),
            Some(ZO_NEW)
        );
    }

    #[test]
    fn equal_builds_are_no_notice_at_all() {
        assert_eq!(
            update_notice(
                &running(OLD),
                &installed(Some(OLD), Some(ZO_NEW)),
                &shas(&[ZO_NEW])
            ),
            None
        );
        // No zo pane is running: nothing differs, whatever is installed.
        assert_eq!(
            update_notice(&running(OLD), &installed(Some(OLD), Some(ZO_NEW)), &[]),
            None
        );
    }

    #[test]
    fn a_dirty_running_commit_is_judged_by_its_sha_like_the_crash_report() {
        let dirty = format!("{OLD}-dirty");
        assert_eq!(
            update_notice(&running(&dirty), &installed(Some(OLD), None), &[]),
            None
        );
        let notice = update_notice(&running(&dirty), &installed(Some(NEW), None), &[])
            .expect("a dirty build of the old sha is still the old sha");
        assert_eq!(notice.app.unwrap().running, OLD);
        // The same rule on a zo pane's receipt.
        let zo_dirty = format!("{ZO_NEW}-dirty");
        assert_eq!(
            update_notice(
                &running(OLD),
                &installed(None, Some(ZO_NEW)),
                &shas(&[&zo_dirty])
            ),
            None
        );
        assert_eq!(crate::crash::plain_sha(&dirty), OLD);
    }

    #[test]
    fn a_missing_installed_half_judges_nothing() {
        assert_eq!(
            update_notice(&running(OLD), &Installed::default(), &shas(&[ZO_OLD])),
            None
        );
        // app absent, zo present and equal
        assert_eq!(
            update_notice(
                &running(OLD),
                &installed(None, Some(ZO_OLD)),
                &shas(&[ZO_OLD])
            ),
            None
        );
    }

    #[test]
    fn an_unstamped_or_unverified_build_is_not_a_different_build() {
        for stamp in ["unknown", "", &format!("{OLD}-unverified"), "abc12"] {
            assert_eq!(
                update_notice(&running(stamp), &installed(Some(NEW), None), &[]),
                None,
                "{stamp:?} cannot be judged, so it must not read as drift"
            );
        }
    }

    #[test]
    fn an_abbreviated_sha_on_either_side_still_matches() {
        assert_eq!(
            update_notice(&running(&OLD[..7]), &installed(Some(OLD), None), &[]),
            None
        );
        assert_eq!(
            update_notice(&running(OLD), &installed(Some(&OLD[..12]), None), &[]),
            None
        );
        assert!(update_notice(&running(&NEW[..7]), &installed(Some(OLD), None), &[]).is_some());
    }

    // ---- the back half (t-3191) ------------------------------------------

    fn announced(version: &str) -> Announced {
        Announced {
            version: version.into(),
            notes: Some("## [1.3.0]".into()),
            pub_date: Some("2026-09-09T00:00:00Z".into()),
            asset: format!("ZeroCode_{version}_aarch64.app.tar.gz"),
        }
    }

    /// §2.3: a build the feed cannot speak for — no judged sha, or the sha
    /// the local lane installed — never asks the feed; the t-3005 road says
    /// 「새 빌드 준비됨」 on that machine instead.
    #[test]
    fn a_dev_build_or_a_lane_installed_build_does_not_ask_the_feed() {
        for stamp in [
            "unknown",
            "",
            &format!("{OLD}-unverified"),
            &format!("{OLD}-dirty"),
        ] {
            assert_eq!(
                dev_build(&running(stamp), &Installed::default()),
                Some(DevBuild::Unstamped),
                "{stamp:?}"
            );
        }
        assert_eq!(
            dev_build(&running(OLD), &installed(Some(OLD), None)),
            Some(DevBuild::LaneInstalled)
        );
        assert_eq!(
            dev_build(&running(OLD), &installed(Some(&OLD[..12]), None)),
            Some(DevBuild::LaneInstalled),
            "an abbreviated lane sha is the same commit"
        );
        assert_eq!(dev_build(&running(OLD), &Installed::default()), None);
        assert_eq!(dev_build(&running(OLD), &installed(Some(NEW), None)), None);
        assert_eq!(
            dev_build(&running(OLD), &installed(None, Some(ZO_NEW))),
            None,
            "a zo swap says nothing about the app"
        );
    }

    /// §2.3 the clock: off never ticks and only the button asks; the pane's
    /// arrival asks unless off; the clock waits a minute after boot and then
    /// six hours since the last check.
    #[test]
    fn the_clock_is_the_table_and_the_policy() {
        let fresh = Clock {
            boot_secs: 0,
            since_last_check_secs: None,
        };
        let settled = Clock {
            boot_secs: CHECK_AFTER_BOOT_SECS,
            since_last_check_secs: None,
        };
        let recent = Clock {
            boot_secs: 10_000,
            since_last_check_secs: Some(CHECK_EVERY_SECS - 1),
        };
        let stale = Clock {
            boot_secs: 10_000,
            since_last_check_secs: Some(CHECK_EVERY_SECS),
        };
        for policy in [UpdatePolicy::Ask, UpdatePolicy::Auto] {
            assert!(
                !check_is_due(policy, Knock::Clock, fresh),
                "{policy:?} waits a minute"
            );
            assert!(
                check_is_due(policy, Knock::Clock, settled),
                "{policy:?} first check"
            );
            assert!(
                !check_is_due(policy, Knock::Clock, recent),
                "{policy:?} six hours"
            );
            assert!(check_is_due(policy, Knock::Clock, stale));
            assert!(check_is_due(policy, Knock::PaneOpened, fresh));
            assert!(check_is_due(policy, Knock::Button, fresh));
        }
        assert!(!check_is_due(UpdatePolicy::Off, Knock::Clock, stale));
        assert!(!check_is_due(UpdatePolicy::Off, Knock::PaneOpened, stale));
        assert!(
            check_is_due(UpdatePolicy::Off, Knock::Button, fresh),
            "「지금 확인」 alone lives"
        );
    }

    /// §2.3 after the feed answered: ask announces, auto downloads, off
    /// announces what the button found; the skipped version is silent until
    /// a newer one arrives, which also clears the skip.
    #[test]
    fn the_verdict_follows_the_policy_and_the_skipped_version() {
        let newer = Found::Newer(announced("1.3.0"));
        let ask = judge_found(UpdatePolicy::Ask, newer.clone(), None);
        assert_eq!(ask.next, Some(NextStep::Announce));
        assert!(matches!(ask.phase, Phase::Available { .. }));
        assert!(!ask.clear_skip);
        let auto = judge_found(UpdatePolicy::Auto, newer.clone(), None);
        assert_eq!(auto.next, Some(NextStep::Download));
        let off = judge_found(UpdatePolicy::Off, newer.clone(), None);
        assert_eq!(off.next, Some(NextStep::Announce));

        let skipped = judge_found(UpdatePolicy::Auto, newer.clone(), Some("1.3.0"));
        assert_eq!(
            skipped.next, None,
            "a skipped version is silent even under auto"
        );
        assert!(matches!(skipped.phase, Phase::Skipped { .. }));
        assert!(!skipped.clear_skip);
        let past_skip = judge_found(UpdatePolicy::Ask, newer, Some("1.2.9"));
        assert_eq!(past_skip.next, Some(NextStep::Announce));
        assert!(past_skip.clear_skip, "a newer version releases the skip");

        for (found, phase) in [
            (Found::UpToDate, Phase::UpToDate),
            (Found::NothingPublished, Phase::NothingPublished),
            (Found::NoAssetForPlatform, Phase::NoAssetForPlatform),
        ] {
            let verdict = judge_found(UpdatePolicy::Auto, found, Some("1.3.0"));
            assert_eq!(verdict.phase, phase);
            assert_eq!(verdict.next, None);
            assert!(!verdict.clear_skip, "nothing newer, the skip stands");
        }
        let failed = judge_found(
            UpdatePolicy::Auto,
            Found::Failed(FailureWord::Network),
            None,
        );
        assert!(matches!(
            failed.phase,
            Phase::Failed {
                word: FailureWord::Network,
                ..
            }
        ));
        assert_eq!(failed.next, None);
    }

    /// §2.2 공존 규칙: a platform asset that is not ours is 「이 플랫폼의 자산이
    /// 아직 없음」 before any signature is looked at.
    #[test]
    fn a_platform_asset_is_ours_by_its_name() {
        assert!(asset_is_ours(
            "https://github.com/cjy5507/zerocode/releases/download/v1.3.0/ZeroCode_1.3.0_aarch64.app.tar.gz"
        ));
        assert!(asset_is_ours(
            "https://x.test/ZeroCode_1.3.0_x64.app.tar.gz?token=1"
        ));
        assert!(!asset_is_ours(
            "https://github.com/cjy5507/zerocode/releases/download/v1.2.7/zo-v1.2.7-aarch64-apple-darwin"
        ));
        assert!(
            !asset_is_ours("https://x.test/ZeroCode.app.tar.gz"),
            "the unversioned name is not the lane's"
        );
        assert!(!asset_is_ours("not a url"));
        assert!(!asset_is_ours("https://x.test/"));
    }

    /// The failure words the pane shows: the plugin's variants fold to a
    /// short table, 404 (`ReleaseNotFound`) is 「아직 공개된 버전 없음」 and not
    /// an error (§2.7), and the history road tells 403 from the rest.
    #[test]
    fn failures_fold_to_words() {
        use tauri_plugin_updater::Error as E;
        assert_eq!(
            failure_word(&E::ReleaseNotFound),
            FailureWord::NothingPublished
        );
        assert_eq!(
            failure_word(&E::TargetNotFound("darwin-aarch64".into())),
            FailureWord::NoAssetForPlatform
        );
        assert_eq!(
            failure_word(&E::TargetsNotFound(vec!["a".into()])),
            FailureWord::NoAssetForPlatform
        );
        assert_eq!(
            failure_word(&E::Network(
                "Download request failed with status: 403 Forbidden".into()
            )),
            FailureWord::Forbidden
        );
        assert_eq!(
            failure_word(&E::Network("timed out".into())),
            FailureWord::Network
        );
        assert_eq!(
            failure_word(&E::Io(std::io::Error::other("disk full"))),
            FailureWord::Disk
        );
        assert_eq!(
            failure_word(&E::SignatureUtf8("x".into())),
            FailureWord::Signature
        );
        assert_eq!(failure_word(&E::EmptyEndpoints), FailureWord::Unsupported);
        assert_eq!(
            failure_word(&E::Serialization(
                serde_json::from_str::<Value>("{").unwrap_err()
            )),
            FailureWord::Malformed
        );
        assert_eq!(history_failure(Some(403)), FailureWord::Forbidden);
        assert_eq!(history_failure(Some(429)), FailureWord::Forbidden);
        assert_eq!(history_failure(Some(500)), FailureWord::Network);
        assert_eq!(history_failure(None), FailureWord::Network);
        assert_eq!(
            serde_json::to_value(FailureWord::NoAssetForPlatform).unwrap(),
            json!("no_asset_for_platform")
        );
    }

    #[test]
    fn progress_is_a_percent_when_the_total_is_known() {
        assert_eq!(progress(0, Some(200)), Some(0));
        assert_eq!(progress(50, Some(200)), Some(25));
        assert_eq!(progress(200, Some(200)), Some(100));
        assert_eq!(progress(300, Some(200)), Some(100), "never past the end");
        assert_eq!(progress(10, Some(0)), None);
        assert_eq!(progress(10, None), None);
    }

    /// §2.2: the release list is filtered to ours by assets — `latest.json`
    /// and a `ZeroCode_…app.tar.gz` together — so the older CLI's rows in
    /// the same repository never show; the cache stands for the TTL.
    #[test]
    fn the_history_is_ours_by_assets_and_the_cache_has_a_ttl() {
        let list = json!([
            {"tag_name": "v1.3.1", "name": "ZeroCode 1.3.1", "body": "notes", "published_at": "2026-09-10T00:00:00Z", "prerelease": true,
             "assets": [{"name": "latest.json"}, {"name": "ZeroCode_1.3.1_aarch64.app.tar.gz"}, {"name": "ZeroCode_1.3.1_aarch64.app.tar.gz.sig"}]},
            {"tag_name": "v1.3.0", "name": null, "body": null, "published_at": "2026-09-09T00:00:00Z", "prerelease": false,
             "assets": [{"name": "latest.json"}, {"name": "ZeroCode_1.3.0_aarch64.app.tar.gz"}]},
            {"tag_name": "v1.2.7", "name": "zo 1.2.7", "body": "Rust-native coding agent CLI", "published_at": "2026-08-13T00:00:00Z", "prerelease": false,
             "assets": [{"name": "manifest.txt"}, {"name": "zo-v1.2.7-aarch64-apple-darwin"}]},
            {"tag_name": "v9.9.9", "name": "half", "body": "", "published_at": "2026-09-11T00:00:00Z", "prerelease": false,
             "assets": [{"name": "latest.json"}]},
            {"tag_name": "v9.9.8", "name": "half", "body": "", "published_at": "2026-09-11T00:00:00Z", "prerelease": false,
             "assets": [{"name": "ZeroCode_9.9.8_aarch64.app.tar.gz"}]},
            "not an object"
        ]);
        let releases = read_releases(&list);
        assert_eq!(
            releases.iter().map(|r| r.tag.as_str()).collect::<Vec<_>>(),
            vec!["v1.3.1", "v1.3.0"],
            "the older CLI's rows and half-published rows are not ours"
        );
        assert!(releases[0].prerelease);
        assert_eq!(
            releases[1].name, "",
            "a null name is empty, never the word null"
        );
        assert_eq!(releases[1].body, "");
        assert_eq!(releases[0].published_at, "2026-09-10T00:00:00Z");
        assert_eq!(
            read_releases(&json!({"message": "rate limited"})),
            Vec::<Release>::new()
        );

        let fetched = zerocode_core::civil::iso_utc_of(1_000_000_000_000);
        let cache = HistoryCache {
            fetched_at: fetched,
            releases: releases.clone(),
        };
        let ttl_ms = i64::try_from(HISTORY_TTL_SECS).unwrap() * 1_000;
        assert!(cache_is_fresh(&cache, 1_000_000_000_000 + ttl_ms - 1));
        assert!(!cache_is_fresh(&cache, 1_000_000_000_000 + ttl_ms));
        assert!(
            !cache_is_fresh(&cache, 999_999_999_999),
            "a clock that walked backwards is stale"
        );
        let torn = HistoryCache {
            fetched_at: "yesterday".into(),
            releases: Vec::new(),
        };
        assert!(!cache_is_fresh(&torn, 1_000_000_000_000));
        assert!(update_dir_under(Path::new("/h")).ends_with(".local/share/zerocode/update"));
        assert_eq!(archive_file_name("1.3.0"), "ZeroCode_1.3.0.app.tar.gz");
    }

    /// The bundle beside which the update is staged, and the two names the
    /// swap uses: `.ZeroCode.app.new` beside it, `ZeroCode.app.old` after.
    #[test]
    fn the_bundle_is_the_nearest_app_ancestor() {
        let exe = Path::new("/Applications/ZeroCode.app/Contents/MacOS/zerocode-shell");
        let bundle = app_bundle_of(exe).expect("a bundle");
        assert_eq!(bundle, Path::new("/Applications/ZeroCode.app"));
        assert_eq!(
            staged_bundle_path(&bundle),
            Path::new("/Applications/.ZeroCode.app.new")
        );
        assert_eq!(
            retired_bundle_path(&bundle),
            Path::new("/Applications/ZeroCode.app.old")
        );
        assert_eq!(
            app_bundle_of(Path::new("/tmp/target/debug/zerocode-shell")),
            None
        );
        assert_eq!(
            app_bundle_of(Path::new(
                "/Users/j/Nested.app/Contents/Helpers/Inner.app/Contents/MacOS/x"
            )),
            Some(PathBuf::from(
                "/Users/j/Nested.app/Contents/Helpers/Inner.app"
            )),
            "the nearest, not the outermost"
        );
    }

    #[test]
    fn build_stamp_is_the_compiled_identity_verbatim() {
        let stamp = build_stamp();
        assert_eq!(stamp.commit, env!("ZEROCODE_COMMIT"));
        assert_eq!(stamp.ui_digest, env!("ZEROCODE_UI_DIGEST"));
        assert_eq!(stamp.version, env!("CARGO_PKG_VERSION"));
        // And it is the crash report's identity, seen from the other side.
        assert_eq!(
            serde_json::to_value(crate::crash::build_identity()).unwrap()["id"],
            json!(stamp.commit)
        );
    }

    #[test]
    fn installed_json_is_read_leniently() {
        let value = json!({"app": {"sha": NEW, "at": "t"}, "zo": {"sha": 7}});
        let read = Installed::from_value(Some(&value));
        assert_eq!(read.app.as_ref().map(|a| a.sha.as_str()), Some(NEW));
        assert_eq!(read.app.as_ref().and_then(|a| a.at.as_deref()), Some("t"));
        assert_eq!(read.zo, None, "a sha that is not a string is no half");
        // The version (t-3237): the lane's word, trimmed; an older file
        // without it, an empty one (the lane's 「unknown」) or a number is None.
        let versions = |value: Value| {
            let read = Installed::from_value(Some(&value));
            (
                read.app.and_then(|a| a.version),
                read.zo.and_then(|z| z.version),
            )
        };
        assert_eq!(
            versions(json!({"app": {"sha": NEW, "at": "t", "version": " 0.2.0 "},
                             "zo": {"sha": ZO_NEW, "at": "t", "version": ""}})),
            (Some("0.2.0".into()), None)
        );
        assert_eq!(
            versions(json!({"app": {"sha": NEW, "at": "t", "version": 2}, "zo": {"sha": ZO_NEW}})),
            (None, None)
        );
        assert_eq!(Installed::from_value(None), Installed::default());
        assert_eq!(
            Installed::from_value(Some(&json!("not an object"))),
            Installed::default()
        );
    }

    #[test]
    fn release_status_reads_both_files_and_never_errs() {
        let home = tempfile::tempdir().unwrap();
        let dir = release_dir_under(home.path());
        assert!(dir.ends_with(".local/share/zerocode/release"));

        // Nothing there yet: two nulls, no notice.
        let empty = release_status_in(&dir, &running(OLD), &[]);
        assert_eq!(empty.status, Value::Null);
        assert_eq!(empty.installed, Value::Null);
        assert_eq!(empty.notice, None);

        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(STATUS_FILE),
            json!({"sha": NEW, "phase": "swap-app", "outcome": null}).to_string(),
        )
        .unwrap();
        std::fs::write(dir.join(INSTALLED_FILE), "{\"app\": {\"sha\": \"").unwrap();
        let torn = release_status_in(&dir, &running(OLD), &[]);
        assert_eq!(torn.status["phase"], json!("swap-app"));
        assert_eq!(torn.installed, Value::Null, "a torn write is silence");
        assert_eq!(torn.notice, None);

        std::fs::write(
            dir.join(INSTALLED_FILE),
            json!({"app": {"sha": NEW, "at": "t"}, "zo": {"sha": ZO_NEW, "at": "t"}}).to_string(),
        )
        .unwrap();
        let swapped = release_status_in(&dir, &running(OLD), &shas(&[ZO_OLD]));
        assert_eq!(swapped.installed["zo"]["sha"], json!(ZO_NEW));
        let notice = swapped.notice.expect("both halves drifted");
        assert_eq!(notice.app.unwrap().installed, NEW);
        assert_eq!(notice.zo.unwrap().running, vec![ZO_OLD.to_string()]);

        // The file as the lane writes it now (t-3237): the version reaches
        // the notice, and the JSON the window reads spells the change.
        std::fs::write(
            dir.join(INSTALLED_FILE),
            json!({"app": {"sha": NEW, "at": "t", "version": "0.2.0"},
                   "zo": {"sha": ZO_NEW, "at": "t", "version": "0.2.0"}})
            .to_string(),
        )
        .unwrap();
        let versioned = release_status_in(&dir, &running(OLD), &panes(&[(ZO_OLD, "0.1.0")]));
        let wire = serde_json::to_value(&versioned).unwrap();
        assert_eq!(wire["notice"]["app"]["installed_version"], json!("0.2.0"));
        assert_eq!(wire["notice"]["app"]["running_version"], json!("0.1.0"));
        assert_eq!(wire["notice"]["app"]["change"], json!("version"));
        assert_eq!(wire["notice"]["zo"]["installed_version"], json!("0.2.0"));
        assert_eq!(wire["notice"]["zo"]["running_versions"], json!(["0.1.0"]));
        assert_eq!(wire["notice"]["zo"]["change"], json!("version"));
    }
}
