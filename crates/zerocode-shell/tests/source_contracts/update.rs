//! t-3191 (U-B): versioned and automatic updates — the feed the window asks,
//! the key it verifies with, and the roads the window may take
//! (docs/design/versioned-auto-update.md §2.2–2.5, §3).

use crate::ui_source::{block_after, shipped_backend, strip_rust_comments, window_source};

fn tauri_conf() -> serde_json::Value {
    serde_json::from_str(include_str!("../../tauri.conf.json")).expect("tauri.conf.json is JSON")
}

/// The updater's public key is base64 over a minisign public-key file:
/// `untrusted comment: …` and then one 56-character key line beginning
/// `RW`. Anything else in the field is a typo the plugin would only notice
/// at the first download.
fn is_minisign_pubkey(field: &str) -> bool {
    use base64::Engine;
    let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(field) else {
        return false;
    };
    let Ok(text) = std::str::from_utf8(&bytes) else {
        return false;
    };
    let mut lines = text.lines();
    let comment = lines.next().unwrap_or_default();
    let key = lines.next().unwrap_or_default().trim();
    comment.starts_with("untrusted comment:")
        && key.len() == 56
        && key.starts_with("RW")
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/' || b == b'=')
}

/// §2.2: the feed is one signed static JSON at a GitHub release asset, the
/// endpoint is rendered from `update_runtime::FEED` (the unit test beside the
/// table pins the rendered string equals the configured one), and the public
/// key is either a minisign key or the table's `UNSET` word — never empty,
/// never a placeholder that looks like a key.
#[test]
fn the_updater_config_is_the_feed_table_and_a_real_or_unset_key() {
    let conf = tauri_conf();
    let updater = &conf["plugins"]["updater"];
    let endpoints = updater["endpoints"]
        .as_array()
        .expect("plugins.updater.endpoints is a list");
    assert_eq!(
        endpoints.len(),
        1,
        "one endpoint: the stable channel; beta is chosen at check time from the same table"
    );
    let stable = endpoints[0].as_str().expect("endpoint is a string");
    assert!(
        stable.starts_with("https://github.com/")
            && stable.ends_with("/releases/latest/download/latest.json"),
        "the stable endpoint is GitHub's latest-release asset road: {stable}"
    );
    let pubkey = updater["pubkey"]
        .as_str()
        .expect("plugins.updater.pubkey is a string");
    assert!(
        pubkey == "UNSET" || is_minisign_pubkey(pubkey),
        "pubkey is a minisign public key or the word UNSET, not {pubkey:?}"
    );

    // The table itself: owner/repo spelled once, and the word the config uses
    // while the key is not yet generated is the runtime's own constant.
    let runtime = strip_rust_comments(
        include_str!("../../src/update_runtime.rs")
            .split_once("#[cfg(test)]")
            .map_or(
                include_str!("../../src/update_runtime.rs"),
                |(before, _)| before,
            ),
    );
    assert!(
        runtime.contains("pub(crate) const FEED: Feed = Feed {"),
        "the feed table is one constant"
    );
    assert!(runtime.contains("pub(crate) const PUBKEY_UNSET: &str = \"UNSET\";"));
    assert!(
        runtime.contains("pub(crate) fn feed_endpoint("),
        "the endpoint is rendered from the table, not spelled"
    );
    let rendered_owner_repo = format!("https://github.com/{}/{}/", "cjy5507", "zerocode");
    assert!(
        stable.starts_with(&rendered_owner_repo),
        "the decided repository (design §4.1) is the configured one: {stable}"
    );
}

/// §3: both plugins are registered in the shipped backend, and neither is
/// granted to the webview — the window has no `plugin:updater|…` or
/// `plugin:process|…` road, so the one restart road stays `relaunch_window`.
#[test]
fn the_plugins_are_registered_in_the_backend_and_granted_to_no_webview() {
    let main = strip_rust_comments(include_str!("../../src/main.rs"));
    assert!(
        main.contains(".plugin(tauri_plugin_updater::Builder::new().build())"),
        "the updater plugin is registered"
    );
    assert!(
        main.contains(".plugin(tauri_plugin_process::init())"),
        "the process plugin is registered"
    );
    let capabilities = include_str!("../../capabilities/default.json");
    assert!(
        !capabilities.contains("updater:") && !capabilities.contains("process:"),
        "no webview may call the updater or process plugins directly:\n{capabilities}"
    );
    let window = window_source();
    assert!(
        !window.contains("plugin:updater") && !window.contains("plugin:process"),
        "the window reaches the updater only through the five commands"
    );
    let _ = (shipped_backend(), block_after);
}

/// §2.3, §3: the five commands are registered; the swap of a prepared bundle
/// happens in `relaunch_window` — the one restart road — right before the
/// restart and nowhere else; the plugin's own `install` (which deletes the
/// running app's directory) is never called; and the judgement the commands
/// act on is `update_runtime`'s pure functions, not a second one at the edge.
#[test]
fn the_five_commands_are_registered_and_the_swap_is_the_restart_roads_alone() {
    let main = strip_rust_comments(include_str!("../../src/main.rs"));
    for command in [
        "update_check",
        "update_download",
        "update_install",
        "update_history",
        "patch_update_prefs",
    ] {
        assert!(
            main.contains(&format!("            {command},\n")),
            "{command} is registered in generate_handler!"
        );
    }
    let backend = shipped_backend();
    let road = block_after(backend, "pub(crate) fn relaunch_window(");
    let swap = road
        .find("update_store::swap_in(")
        .expect("relaunch_window swaps the staged bundle");
    let restart = road.find("app.restart()").expect("and restarts");
    assert!(
        swap < restart,
        "the swap comes right before the restart:\n{road}"
    );
    assert_eq!(
        backend.matches("update_store::swap_in(").count(),
        1,
        "one swap site in the shipped backend: relaunch_window"
    );

    let commands = strip_rust_comments(
        include_str!("../../src/cmd/update.rs")
            .split_once("#[cfg(test)]")
            .map_or(include_str!("../../src/cmd/update.rs"), |(before, _)| {
                before
            }),
    );
    assert!(
        !commands.contains(".install(") && !commands.contains("download_and_install("),
        "the plugin's install is never the road — it deletes the running bundle"
    );
    for judgement in [
        "feed::dev_build(",
        "feed::check_is_due(",
        "feed::judge_found(",
        "feed::asset_is_ours(",
        "feed::failure_word(",
        "feed::cache_is_fresh(",
        "feed::read_releases(",
    ] {
        assert!(
            commands.contains(judgement),
            "the commands act on the runtime's judgement `{judgement}`"
        );
    }
    assert!(
        commands.contains("feed::feed_endpoint(channel)"),
        "the channel's endpoint comes from the table at check time"
    );
    assert!(
        commands.contains("== feed::PUBKEY_UNSET"),
        "a download without a key is refused by the table's word"
    );
    // The store is the one file that writes under the update directory and
    // beside the bundle; the runtime stays read-only (t-3005's contract).
    let store = strip_rust_comments(
        include_str!("../../src/update_store.rs")
            .split_once("#[cfg(test)]")
            .map_or(include_str!("../../src/update_store.rs"), |(before, _)| {
                before
            }),
    );
    assert!(
        store.contains("fn stage_beside(") && store.contains("fn swap_in("),
        "staging and swapping live in update_store"
    );
    assert!(
        !store.contains("remove_dir_all(bundle)") && !store.contains("remove_dir_all(&bundle)"),
        "the running bundle is renamed aside, never deleted"
    );
    let _ = window_source();
}

/// §2.5: the 「업데이트」 pane is one module (`shell-update.js`) in the four
/// registries, its markup carries every control of the design, its words are
/// `t()` in four catalogs (Korean at the call site), the clock rides the
/// status bar's period plus one boot timer read from `update_spec`, and its
/// restart goes through `relaunch_window` — the one road.
#[test]
fn the_update_pane_is_one_module_with_the_designs_controls_and_words() {
    use crate::ui_source::markup_between;
    let window = window_source();
    for needle in [
        "function paintUpdatePane(",
        "function askUpdateCheck(",
        "function runUpdateDownload(",
        "function refreshUpdateHistory(",
        "function paintUpdateHistory(",
        "function applyUpdateSettingsSnapshot(",
        "function updateReadyVersion(",
        "invoke(\"update_check\"",
        "invoke(\"update_download\"",
        "invoke(\"update_install\"",
        "invoke(\"update_history\"",
        "\"patch_update_prefs\"",
        "listen(\"update:progress\"",
    ] {
        assert!(
            window.contains(needle),
            "the update module lacks `{needle}`"
        );
    }
    let poll = block_after(window, "const updateAmbient = idlePoller({");
    assert!(
        poll.contains("every: USAGE_AMBIENT_MS"),
        "the update clock rides the status bar's period, no timer of its own:\n{poll}"
    );
    let boot = block_after(window, "function armUpdateBootCheck(");
    assert!(
        boot.contains("updateSpec.check_after_boot_secs"),
        "the one boot timer reads the table through update_spec:\n{boot}"
    );
    assert!(
        !window.contains("6 * 60 * 60") && !window.contains("21600"),
        "the six-hour number is Rust's, never the window's"
    );
    let restart = block_after(window, "function restartToInstall(");
    assert!(
        restart.contains("invoke(\"relaunch_window\")"),
        "the pane's restart is the one road:\n{restart}"
    );

    let markup = include_str!("../../../../ui/index.html");
    assert_eq!(
        markup.matches("data-pane=\"update\"").count(),
        2,
        "one rail item and one section"
    );
    let section = markup_between(
        markup,
        "<section class=\"settings-pane\" data-pane=\"update\"",
        "</section>",
    );
    for id in [
        "update-running",
        "update-status",
        "update-last-checked",
        "update-check-now",
        "update-progress",
        "update-download",
        "update-restart",
        "update-skip",
        "update-policy",
        "update-channel",
        "update-history",
        "update-history-empty",
        "update-history-refresh",
        "update-zo-note",
    ] {
        assert!(
            section.contains(&format!("id=\"{id}\"")),
            "the pane lacks #{id}"
        );
    }
    assert_eq!(
        section
            .matches("type=\"radio\" name=\"update-policy\"")
            .count(),
        3,
        "three policy radios"
    );
    assert!(section.contains("role=\"radiogroup\""));
    assert!(!section.contains(" title="), "no native title attributes");

    let i18n = include_str!("../../../../ui/shell-i18n.js");
    for key in [
        "settings.update.pane",
        "settings.update.about",
        "settings.update.checkNow",
        "settings.update.download",
        "settings.update.restartInstall",
        "settings.update.skip",
        "settings.update.policy",
        "settings.update.policyAsk",
        "settings.update.policyAuto",
        "settings.update.policyOff",
        "settings.update.channel",
        "settings.update.channelStable",
        "settings.update.channelBeta",
        "settings.update.history",
        "settings.update.historyEmpty",
        "settings.update.status.available",
        "settings.update.status.downloading",
        "settings.update.status.ready",
        "settings.update.status.failed",
        "settings.update.fail.noAssetForPlatform",
        "settings.update.fail.nothingPublished",
        "settings.update.readyVersion",
        "update.available",
        "update.readyVersion",
    ] {
        assert_eq!(
            i18n.matches(&format!("\"{key}\"")).count(),
            4,
            "{key} must be in en/ja/zh/es"
        );
    }

    // Four registries, one file (the dist copier and the build stamp agree by
    // their own gate; here: the file is in all four at all).
    assert!(include_str!("../../build.rs").contains("\"shell-update.js\","));
    assert!(include_str!("../../../../scripts/build-ui-dist.mjs").contains("\"shell-update.js\","));
    assert!(include_str!("../../src/ui_source.rs").contains("\"shell-update.js\","));
    assert!(include_str!("support.rs").contains("\"shell-update.js\","));
    assert!(markup.contains("<script defer src=\"./shell-update.js\"></script>"));
}

/// The window's parts are classic scripts sharing one global lexical scope:
/// a top-level `let`/`const`/`class` declared twice across two files is a
/// `SyntaxError` at load that silently drops the whole later file — every
/// function in it undefined, every snapshot chain that reaches one broken.
/// `node --check` reads one file and cannot see it (t-3191: `updateToastKey`
/// stood in shell-status.js and in the new shell-update.js). One name, one
/// declaration, across every part.
#[test]
fn no_top_level_name_is_declared_twice_across_the_window_parts() {
    use crate::ui_source::WINDOW_PARTS;
    let mut owners: std::collections::BTreeMap<String, Vec<&str>> = Default::default();
    for (name, text) in WINDOW_PARTS {
        for line in text.lines() {
            let Some(rest) = [
                "let ",
                "const ",
                "var ",
                "class ",
                "function ",
                "async function ",
            ]
            .iter()
            .find_map(|keyword| line.strip_prefix(keyword)) else {
                continue;
            };
            if let Some(inner) = rest.strip_prefix('{') {
                // `const { a, b: c } = …` — every bound name.
                let Some(close) = inner.find('}') else {
                    continue;
                };
                for part in inner[..close].split(',') {
                    let bound = part.rsplit(':').next().unwrap_or_default().trim();
                    if !bound.is_empty() {
                        owners.entry(bound.to_string()).or_default().push(name);
                    }
                }
                continue;
            }
            let ident: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '$')
                .collect();
            if !ident.is_empty() {
                owners.entry(ident).or_default().push(name);
            }
        }
    }
    let twice: Vec<String> = owners
        .iter()
        .filter(|(_, files)| files.len() > 1)
        .map(|(ident, files)| format!("{ident} in {}", files.join(", ")))
        .collect();
    assert!(
        twice.is_empty(),
        "a top-level name is declared in more than one window part — the later \
         file fails to load whole:\n  {}",
        twice.join("\n  ")
    );
}

/// §2.4: zo rides along. The release overlay — and only it — names `bin/zo`
/// as a resource (tauri-build copies resources at compile time and refuses a
/// missing path, so a dev build must not ask for one); the overlay keeps
/// every resource the base names; `npm run tauri:release` stages the binary
/// before `tauri build`; the staged path is gitignored; the boot reads the
/// same `bin/zo` under the resource directory, on the existing boot thread,
/// and reports to the update state.
#[test]
fn zo_rides_in_the_release_bundle_and_the_boot_installs_it_beside_the_old_inode() {
    let base: serde_json::Value = tauri_conf();
    let overlay: serde_json::Value =
        serde_json::from_str(include_str!("../../tauri.release.conf.json")).expect("overlay JSON");
    let base_resources = base["bundle"]["resources"]
        .as_array()
        .expect("base resources");
    let overlay_resources = overlay["bundle"]["resources"]
        .as_object()
        .expect("overlay resource map preserves platform resource mappings");
    for resource in base_resources {
        let path = resource.as_str().expect("base resource path");
        assert_eq!(
            overlay_resources
                .get(path)
                .and_then(serde_json::Value::as_str),
            Some(path),
            "the release overlay must preserve the base resource destination: {path}"
        );
    }
    assert_eq!(
        overlay_resources.get("bin/zo"),
        Some(&serde_json::json!("bin/zo"))
    );
    assert!(
        !base_resources.contains(&serde_json::json!("bin/zo")),
        "a dev build carries no zo and must not ask tauri-build for one"
    );

    let package = include_str!("../../../../package.json");
    let release = package
        .lines()
        .find(|line| line.contains("\"tauri:release\""))
        .expect("the tauri:release script");
    let stage = release.find("scripts/stage-zo.mjs").expect("zo is staged");
    let build = release.find("tauri build").expect("then built");
    assert!(stage < build, "stage before build: {release}");
    assert!(
        include_str!("../../../../.gitignore").contains("crates/zerocode-shell/bin/"),
        "the staged binary is never committed"
    );

    let companion = strip_rust_comments(
        include_str!("../../src/zo_companion.rs")
            .split_once("#[cfg(test)]")
            .map_or(include_str!("../../src/zo_companion.rs"), |(before, _)| {
                before
            }),
    );
    assert!(companion.contains("const BUNDLED_ZO: &[&str] = &[\"bin\", \"zo\"];"));
    assert!(companion.contains("const ZO_BIN: &[&str] = &[\".local\", \"bin\", \"zo\"];"));
    assert!(companion.contains("const ZO_NEW: &str = \".zo.new\";"));
    assert!(
        companion.contains("std::fs::rename(&staged, target)"),
        "the swap is one rename; the old inode lives on under a running zo"
    );
    assert!(
        !companion.contains("fs::write(") && !companion.contains("remove_file("),
        "the companion never writes over or deletes the installed zo"
    );
    let main = strip_rust_comments(include_str!("../../src/main.rs"));
    assert!(
        main.contains("zo_companion::run_at_boot(zo_resources.as_deref(), &home, &app_version)"),
        "the boot runs the companion once"
    );
    assert!(
        main.contains(".note_zo(report)"),
        "and the report reaches the update state for the pane"
    );
    assert_eq!(
        main.matches("zo_companion::run_at_boot(").count(),
        1,
        "once, on the boot thread that already exists"
    );
    let _ = shipped_backend();
}
