//! The trust grant, against an app server that speaks the measured protocol.
//!
//! The unit tests in `codex_grant` cover the pieces — key folding, listing
//! parsing, which failures are retryable. These cover the conversation, which is
//! the part that can be right in every piece and still wrong as a whole.
//!
//! The server is a Python script this test writes. Not a fixture file and not a
//! recording: it implements the protocol, so it can also assert what it was
//! *asked* — and the single most important claim in this module is about the
//! request, not the response. The hash written into `hooks.state` has to be the
//! one Codex reported, never one we computed, because that is the difference
//! between asking for consent and forging it.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use zerocode_hookd::codex_grant::{Grant, GrantError, GrantPlan, Invocation, VerifyClass};

const MANAGED: &str = "/bin/sh '/h/.zerocode/agent-hooks/codex-hook.sh'";

/// A fake `codex app-server`.
///
/// `hooks` is the JSON it answers `hooks/list` with the FIRST time; `after` is
/// what it answers once a `config/batchWrite` has landed. Every request it
/// receives is appended to a log the test reads afterwards.
fn server(dir: &Path, hooks: &str, after: &str, extra: &str) -> PathBuf {
    let script = dir.join("fake-app-server.py");
    let body = format!(
        r#"
import json, sys, os
LOG = os.environ["FAKE_LOG"]
FIRST = {hooks}
AFTER = {after}
wrote = False
{extra}
for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    msg = json.loads(line)
    with open(LOG, "a") as log:
        log.write(line + "\n")
    if "id" not in msg:
        continue                      # a notification; nothing to answer
    method = msg.get("method")
    if method == "initialize":
        out = {{}}
    elif method == "hooks/list":
        out = AFTER if wrote else FIRST
    elif method == "config/batchWrite":
        wrote = True
        out = {{}}
    else:
        print(json.dumps({{"id": msg["id"],
                          "error": {{"code": -32601, "message": "method not found"}}}}),
              flush=True)
        continue
    print(json.dumps({{"id": msg["id"], "result": out}}), flush=True)
"#
    );
    std::fs::write(&script, body).expect("write the fake server");
    script
}

fn listing(key: &str, command: &str, hash: &str, status: &str) -> String {
    format!(
        r#"{{"key": "{key}", "command": "{command}", "currentHash": "{hash}", "trustStatus": "{status}"}}"#
    )
}

fn plan(script: &Path, log: &Path, keys: &[&str]) -> GrantPlan {
    // The fake is run through python, and the log path rides in the environment
    // so the script and the assertion agree on one file.
    let mut invocation = Invocation::native("python3", Path::new("/h"), true);
    invocation.args = vec![script.to_string_lossy().into_owned()];
    invocation
        .env
        .push(("FAKE_LOG".to_string(), log.to_string_lossy().into_owned()));
    // `native` cleared CODEX_HOME; the fake does not care, and leaving the clear
    // in keeps the invocation the same shape the real one has.
    GrantPlan {
        invocation,
        hooks_list_cwd: PathBuf::from("/h"),
        expected_keys: keys.iter().map(|key| key.to_string()).collect(),
        managed_command: MANAGED.to_string(),
    }
}

fn requests(log: &Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

/// The whole conversation, and the claim the module stands on: the hash written
/// is the one CODEX reported.
#[test]
fn an_untrusted_hook_is_granted_with_codexs_own_hash() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let log = dir.path().join("asked.jsonl");
    let key = "/h/hooks.json:pre_tool_use:0:0";
    let script = server(
        dir.path(),
        &format!(
            "[{}]",
            listing(key, MANAGED, "sha256:codexsaysthis", "untrusted")
        ),
        &format!(
            "[{}]",
            listing(key, MANAGED, "sha256:codexsaysthis", "trusted")
        ),
        "",
    );
    let granted = zerocode_hookd::codex_grant::grant(&plan(&script, &log, &[key]))
        .expect("the fake server answers");
    let Grant::Granted {
        wrote_trust,
        entries,
    } = granted
    else {
        panic!("not granted: {granted:?}");
    };
    assert!(wrote_trust, "an untrusted hook was not granted");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].trusted_hash, "sha256:codexsaysthis");

    let asked = requests(&log);
    // The handshake, in order, and `initialized` as a notification with no id.
    assert_eq!(asked[0]["method"], "initialize");
    assert_eq!(asked[0]["params"]["clientInfo"]["name"], "zerocode");
    assert_eq!(asked[1]["method"], "initialized");
    assert!(
        asked[1].get("id").is_none(),
        "`initialized` was sent as a request rather than a notification: {:?}",
        asked[1]
    );
    // No `jsonrpc` field anywhere: this is not JSON-RPC 2.0, and a server that
    // validates would reject it.
    assert!(
        asked.iter().all(|one| one.get("jsonrpc").is_none()),
        "a jsonrpc field was sent: {asked:?}"
    );

    let write = asked
        .iter()
        .find(|one| one["method"] == "config/batchWrite")
        .expect("the grant wrote trust");
    let edit = &write["params"]["edits"][0];
    assert_eq!(edit["keyPath"], "hooks.state");
    assert_eq!(edit["mergeStrategy"], "upsert");
    assert_eq!(write["params"]["reloadUserConfig"], true);
    // THE assertion. Codex's hash, under the key Codex reported.
    assert_eq!(edit["value"][key]["trusted_hash"], "sha256:codexsaysthis");

    // And it listed again after writing, because a write that returned success is
    // not the same as a hook that will run.
    assert_eq!(
        asked
            .iter()
            .filter(|one| one["method"] == "hooks/list")
            .count(),
        2,
        "the grant did not verify after writing: {asked:?}"
    );
    // The list is asked about the home we named.
    assert_eq!(asked[2]["params"]["cwds"][0], "/h");
}

/// A hook already trusted is not written again — the common case after the first
/// time, and writing anyway would touch the user's config for nothing.
#[test]
fn an_already_trusted_hook_is_not_written_again() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let log = dir.path().join("asked.jsonl");
    let key = "/h/hooks.json:stop:0:0";
    let both = format!("[{}]", listing(key, MANAGED, "sha256:a", "trusted"));
    let script = server(dir.path(), &both, &both, "");
    let granted = zerocode_hookd::codex_grant::grant(&plan(&script, &log, &[key]))
        .expect("the fake server answers");
    assert!(
        matches!(
            granted,
            Grant::Granted {
                wrote_trust: false,
                ..
            }
        ),
        "{granted:?}"
    );
    assert!(
        !requests(&log)
            .iter()
            .any(|one| one["method"] == "config/batchWrite"),
        "trust was rewritten over an already-trusted hook"
    );
}

/// Somebody else's hooks are not granted trust, and their presence does not make
/// ours look covered.
#[test]
fn another_products_hooks_are_neither_granted_nor_counted() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let log = dir.path().join("asked.jsonl");
    let ours = "/h/hooks.json:stop:0:0";
    let theirs = "/h/hooks.json:stop:0:1";
    let both = format!(
        "[{}, {}]",
        listing(ours, MANAGED, "sha256:a", "trusted"),
        listing(
            theirs,
            "/bin/sh '/other/product.sh'",
            "sha256:b",
            "untrusted"
        )
    );
    let script = server(dir.path(), &both, &both, "");
    let granted = zerocode_hookd::codex_grant::grant(&plan(&script, &log, &[ours]))
        .expect("the fake server answers");
    let Grant::Granted { entries, .. } = granted else {
        panic!("not granted: {granted:?}");
    };
    assert_eq!(entries.len(), 1, "{entries:?}");
    assert_eq!(entries[0].key, ours);
    assert!(
        !requests(&log)
            .iter()
            .any(|one| one["method"] == "config/batchWrite"),
        "the other product's untrusted hook triggered a write"
    );
}

/// A hook Codex does not report is a list mismatch — and it must be reported as
/// such rather than granted, because the caller has to roll its hook back.
#[test]
fn a_hook_codex_cannot_see_is_a_list_mismatch() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let log = dir.path().join("asked.jsonl");
    let script = server(dir.path(), "[]", "[]", "");
    let outcome =
        zerocode_hookd::codex_grant::grant(&plan(&script, &log, &["/h/hooks.json:stop:0:0"]))
            .expect("the fake server answers");
    let Grant::VerifyFailed { class, reason } = outcome else {
        panic!("a hook that is not there was granted: {outcome:?}");
    };
    assert_eq!(class, VerifyClass::ListMismatch);
    assert!(reason.contains("0 of 1"), "{reason}");
}

/// Trust written and still not trusted. This is the state that stalls an agent,
/// so it is a failure with its own class and never a success.
#[test]
fn a_write_that_did_not_take_is_reported_untrusted() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let log = dir.path().join("asked.jsonl");
    let key = "/h/hooks.json:stop:0:0";
    let stubborn = format!("[{}]", listing(key, MANAGED, "sha256:a", "untrusted"));
    let script = server(dir.path(), &stubborn, &stubborn, "");
    let outcome = zerocode_hookd::codex_grant::grant(&plan(&script, &log, &[key]))
        .expect("the fake server answers");
    let Grant::VerifyFailed { class, reason } = outcome else {
        panic!("an untrusted hook was reported granted: {outcome:?}");
    };
    assert_eq!(class, VerifyClass::PostGrantUntrusted);
    assert!(reason.contains("untrusted"), "{reason}");
}

/// A Codex that does not know the method is UNSUPPORTED — remember it and stop
/// asking, rather than retrying a capability that will never appear.
#[test]
fn a_method_this_codex_lacks_is_unsupported() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let log = dir.path().join("asked.jsonl");
    // The fake answers `hooks/list` with method-not-found by never matching it.
    let script = server(
        dir.path(),
        "[]",
        "[]",
        "sys.argv.append('--no-hooks-list')\n",
    );
    // Rewrite the script so `hooks/list` falls through to the error branch.
    let text = std::fs::read_to_string(&script).expect("read the fake");
    std::fs::write(
        &script,
        text.replace(r#"elif method == "hooks/list":"#, r#"elif False:"#),
    )
    .expect("rewrite the fake");
    match zerocode_hookd::codex_grant::grant(&plan(&script, &log, &[])) {
        Err(GrantError::Unsupported(why)) => assert!(why.contains("hooks/list"), "{why}"),
        other => panic!("method-not-found was not reported unsupported: {other:?}"),
    }
}

/// A `codex` with no `app-server` subcommand is unsupported, read from what it
/// prints — the exit code for "unknown subcommand" and for "crashed" is the same
/// 1, so treating this as transient means retrying forever.
#[test]
fn a_codex_without_app_server_is_unsupported() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let script = dir.path().join("old-codex.py");
    std::fs::write(
        &script,
        "import sys\nsys.stderr.write(\"error: unrecognized subcommand 'app-server'\\n\")\nsys.exit(1)\n",
    )
    .expect("write the old codex");
    let log = dir.path().join("asked.jsonl");
    match zerocode_hookd::codex_grant::grant(&plan(&script, &log, &[])) {
        Err(GrantError::Unsupported(why)) => {
            assert!(why.contains("app-server"), "{why}");
        }
        other => panic!("an old codex was not reported unsupported: {other:?}"),
    }
}

/// A server that never answers is a timeout, and the process does not survive
/// it. Stopping the wait without killing would leave an app server holding the
/// config file the caller is about to read.
#[test]
fn a_server_that_never_answers_times_out_and_is_killed() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let script = dir.path().join("silent.py");
    // Reads nothing, answers nothing, and writes a file so the test can tell it
    // actually started rather than failing to spawn.
    std::fs::write(
        &script,
        "import time, os, sys\nopen(os.environ['FAKE_LOG'], 'w').write('started')\ntime.sleep(300)\n",
    )
    .expect("write the silent server");
    let log = dir.path().join("asked.jsonl");
    let mut held = plan(&script, &log, &[]);
    held.invocation.timeout = Duration::from_millis(600);
    let started = std::time::Instant::now();
    let outcome = zerocode_hookd::codex_grant::grant(&held);
    let took = started.elapsed();
    assert!(
        matches!(outcome, Err(GrantError::Timeout(_))),
        "a silent server was not a timeout: {outcome:?}"
    );
    // It gave up near the deadline rather than at the 300s the child asked for.
    assert!(took < Duration::from_secs(20), "took {took:?}");
    assert_eq!(
        std::fs::read_to_string(&log).unwrap_or_default(),
        "started",
        "the silent server never ran, so this proved nothing"
    );
}

/// A server that talks before answering — notifications, other ids — is not a
/// failure. It is entitled to.
#[test]
fn chatter_before_the_answer_is_ignored() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let log = dir.path().join("asked.jsonl");
    let key = "/h/hooks.json:stop:0:0";
    let both = format!("[{}]", listing(key, MANAGED, "sha256:a", "trusted"));
    let script = server(dir.path(), &both, &both, "");
    // Every answer is preceded by a notification and a response to an id nobody
    // is waiting for.
    let text = std::fs::read_to_string(&script).expect("read the fake");
    let answer = r#"    print(json.dumps({"id": msg["id"], "result": out}), flush=True)"#;
    // The rewrite has to land, or this test asserts a plain grant and calls it
    // proof that chatter is tolerated.
    assert!(
        text.contains(answer),
        "the fake server changed shape: {text}"
    );
    std::fs::write(
        &script,
        text.replace(
            answer,
            concat!(
                "    print(json.dumps({\"method\": \"log\", \"params\": {\"m\": \"w\"}}), flush=True)\n",
                "    print(json.dumps({\"id\": 9999, \"result\": {}}), flush=True)\n",
                "    print(json.dumps({\"id\": msg[\"id\"], \"result\": out}), flush=True)"
            ),
        ),
    )
    .expect("rewrite the fake");
    let granted = zerocode_hookd::codex_grant::grant(&plan(&script, &log, &[key]))
        .expect("chatter broke the session");
    assert!(matches!(granted, Grant::Granted { .. }), "{granted:?}");
}

/// The keys are compared folded, so a Codex that reports a path spelled
/// differently than we planned still matches. Unfolded, the grant would report a
/// mismatch for a hook that is right there.
#[test]
fn a_differently_spelled_path_still_matches() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let log = dir.path().join("asked.jsonl");
    let reported = "/h/./sub/../hooks.json:stop:0:0";
    let both = format!("[{}]", listing(reported, MANAGED, "sha256:a", "trusted"));
    let script = server(dir.path(), &both, &both, "");
    let granted =
        zerocode_hookd::codex_grant::grant(&plan(&script, &log, &["/h/hooks.json:stop:0:0"]))
            .expect("the fake server answers");
    let Grant::Granted { entries, .. } = granted else {
        panic!("a folded path did not match: {granted:?}");
    };
    // The entry keeps the key Codex used, and carries the folded one beside it.
    assert_eq!(entries[0].key, reported);
    assert_eq!(entries[0].normalized_key, "/h/hooks.json:stop:0:0");
}

/// Every expected key has to be covered, not just the count matched. Two hooks
/// reported where one of them is a key we did not plan is a mismatch, even though
/// the numbers agree.
#[test]
fn matching_counts_with_the_wrong_keys_is_still_a_mismatch() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let log = dir.path().join("asked.jsonl");
    let both = format!(
        "[{}, {}]",
        listing("/h/hooks.json:stop:0:0", MANAGED, "sha256:a", "trusted"),
        listing("/h/hooks.json:stop:0:1", MANAGED, "sha256:b", "trusted")
    );
    let script = server(dir.path(), &both, &both, "");
    let wanted: BTreeSet<String> = ["/h/hooks.json:stop:0:0", "/h/hooks.json:pre_tool_use:0:0"]
        .iter()
        .map(|key| key.to_string())
        .collect();
    let mut held = plan(&script, &log, &[]);
    held.expected_keys = wanted;
    let outcome = zerocode_hookd::codex_grant::grant(&held).expect("the fake server answers");
    assert!(
        matches!(
            outcome,
            Grant::VerifyFailed {
                class: VerifyClass::ListMismatch,
                ..
            }
        ),
        "two-for-two with a wrong key was accepted: {outcome:?}"
    );
}
