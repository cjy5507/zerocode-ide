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
use zerocode_hookd::codex_install::{Landed, land};

const MANAGED: &str = "/bin/sh '/h/.zerocode/agent-hooks/codex-hook.sh'";

/// The silent server's deadline must outlast the interpreter's own start, or
/// the grant gives up before the fake has run a line and the test proves
/// nothing — a Windows CI runner took longer than 600 ms to start python
/// (t-21270, run 36946561997). So the deadline is this machine's measured
/// start times [`START_MARGIN`], never under the 600 ms the test was written
/// with, and never over a ceiling that keeps "gave up near the deadline"
/// (under [`GAVE_UP_WITHIN`]) a claim worth checking.
const SILENT_DEADLINE_FLOOR: Duration = Duration::from_millis(600);
const SILENT_DEADLINE_CEILING: Duration = Duration::from_secs(10);
const START_MARGIN: u32 = 5;
const GAVE_UP_WITHIN: Duration = Duration::from_secs(20);

/// How long this machine's python takes from spawn to the end of a one-line
/// script, measured with the same interpreter and environment the fake runs in.
fn interpreter_start() -> Duration {
    let started = std::time::Instant::now();
    let status = std::process::Command::new("python3")
        .args(["-c", "pass"])
        .status()
        .expect("python3 runs");
    assert!(status.success(), "python3 -c pass: {status}");
    started.elapsed()
}

fn silent_deadline() -> Duration {
    (interpreter_start() * START_MARGIN).clamp(SILENT_DEADLINE_FLOOR, SILENT_DEADLINE_CEILING)
}

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
#[cfg_attr(
    not(unix),
    ignore = "the fake app server and its keys are POSIX paths; Codex's Windows key spelling is not implemented (codex_trust::normalize_source_path)"
)]
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
#[cfg_attr(
    not(unix),
    ignore = "the fake app server and its keys are POSIX paths; Codex's Windows key spelling is not implemented (codex_trust::normalize_source_path)"
)]
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
#[cfg_attr(
    not(unix),
    ignore = "the fake app server and its keys are POSIX paths; Codex's Windows key spelling is not implemented (codex_trust::normalize_source_path)"
)]
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
#[cfg_attr(
    not(unix),
    ignore = "the fake app server and its keys are POSIX paths; Codex's Windows key spelling is not implemented (codex_trust::normalize_source_path)"
)]
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

/// The Python of a fake `codex` with no `app-server` that says why it died only
/// [`LATE_VOICE_PAUSE`] after the stream the parent is waiting on has gone quiet.
///
/// `@QUIET@` is what it does first to silence that stream and `@PAUSE@` is the
/// pause in seconds. It writes `said` to the log once it has spoken, so a test can
/// tell a fake that ran to its end from one that never ran. `os._exit` rather than
/// a normal exit: the interpreter prints to stderr at shutdown when a standard
/// stream was closed under it, and that is not what this fake is here to say.
const LATE_VOICE: &str = r#"
import os, sys, time
@QUIET@
time.sleep(@PAUSE@)
sys.stderr.write("error: unrecognized subcommand 'app-server'\n")
sys.stderr.flush()
with open(os.environ["FAKE_LOG"], "w") as log:
    log.write("said")
os._exit(1)
"#;

/// Goes quiet by closing the input after answering `initialize`: the parent's next
/// write finds nobody reading it.
const DEAF_AFTER_ITS_ANSWER: &str = r#"import json
request = json.loads(sys.stdin.readline())
os.close(0)
print(json.dumps({"id": request["id"], "result": {}}), flush=True)"#;

/// How long after going quiet the late-voiced fake gives its reason. Long enough
/// that a parent which judges at once has judged by then, and a small part of the
/// wait the grant gives a dead child's last words.
const LATE_VOICE_PAUSE: Duration = Duration::from_millis(250);

/// Write the late-voiced old `codex` into `dir`; it goes quiet the way `quiet` says.
fn late_voiced_old_codex(dir: &Path, quiet: &str) -> PathBuf {
    let script = dir.join("late-voiced-old-codex.py");
    let pause = LATE_VOICE_PAUSE.as_secs_f64().to_string();
    let body = LATE_VOICE
        .replace("@QUIET@", quiet)
        .replace("@PAUSE@", &pause);
    std::fs::write(&script, body).expect("write the late-voiced old codex");
    script
}

/// Grant against an old `codex` that goes quiet the way `quiet` says and gives its
/// reason a moment later; with the log the fake wrote when it had.
fn grant_against_late_voice(quiet: &str) -> (Result<Grant, GrantError>, String) {
    let dir = tempfile::tempdir().expect("a temp dir");
    let script = late_voiced_old_codex(dir.path(), quiet);
    let log = dir.path().join("asked.jsonl");
    let outcome = zerocode_hookd::codex_grant::grant(&plan(&script, &log, &[]));
    (outcome, std::fs::read_to_string(&log).unwrap_or_default())
}

/// What a dead `codex` said last decides what it was. One whose output closes
/// before its reason arrives is still a Codex with no `app-server`: a busy machine
/// puts the reason second by itself (the thread that reads it runs after the one
/// that sees the output end), and this fake does the same on purpose. Judged at
/// once, it reads as one that broke, and the window retries a capability that will
/// never appear.
#[cfg_attr(
    not(unix),
    ignore = "the fake closes a descriptor it inherited, which is not measured on Windows"
)]
#[test]
fn a_codex_that_says_why_after_its_output_closed_is_still_unsupported() {
    let (outcome, said) = grant_against_late_voice("os.close(1)");
    match outcome {
        Err(GrantError::Unsupported(why)) => {
            assert!(why.contains("unrecognized subcommand"), "{why}");
        }
        other => panic!(
            "an old codex that closed its output before saying why was not reported unsupported: {other:?}"
        ),
    }
    // A fake that never ran (no python3 on the PATH is "unsupported" too) would pass
    // the match above and prove nothing.
    assert_eq!(said, "said", "the fake never said why");
}

/// The same claim on the other road out of a dead child: the parent's write to its
/// input fails because the child stopped reading, and the reason follows a moment
/// later. Both roads end in one function, and a wait added to one of them only
/// would leave the other judging at once.
#[cfg_attr(
    not(unix),
    ignore = "the fake closes a descriptor it inherited, which is not measured on Windows"
)]
#[test]
fn a_codex_that_says_why_after_its_input_closed_is_still_unsupported() {
    let (outcome, said) = grant_against_late_voice(DEAF_AFTER_ITS_ANSWER);
    match outcome {
        Err(GrantError::Unsupported(why)) => {
            assert!(why.contains("unrecognized subcommand"), "{why}");
        }
        other => panic!(
            "an old codex that stopped reading before saying why was not reported unsupported: {other:?}"
        ),
    }
    // A fake that never ran (no python3 on the PATH is "unsupported" too) would pass
    // the match above and prove nothing.
    assert_eq!(said, "said", "the fake never said why");
}

/// What a person is told, and how often Codex is asked, when the old `codex` gives
/// its reason a moment after its output closed. The install road asks once, lands
/// the hook in the home this app owns, and says the grant will never work. A window
/// that read the same Codex as one that broke would ask a second time at once, put
/// a retry on it, and show "exited: before answering" where the reason belongs.
#[cfg_attr(
    not(unix),
    ignore = "the fake closes a descriptor it inherited, which is not measured on Windows"
)]
#[test]
fn an_old_codex_that_says_why_late_is_asked_once_and_remembered_as_unsupported() {
    let dir = tempfile::tempdir().expect("a temp dir");
    // A Codex home the way a person has one: the hooks file and the settings.
    let system_home = dir.path().join("dot-codex");
    std::fs::create_dir_all(&system_home).expect("mkdir");
    let hooks = system_home.join("hooks.json");
    std::fs::write(&hooks, "{\n  \"hooks\": {}\n}\n").expect("write the hooks file");
    std::fs::write(system_home.join("config.toml"), "model = \"gpt-5\"\n")
        .expect("write the settings");
    let script = late_voiced_old_codex(dir.path(), "os.close(1)");
    let log = dir.path().join("asked.jsonl");
    let grant_plan = plan(&script, &log, &[]);

    let asked = std::cell::Cell::new(0u32);
    let landed = land(
        &hooks,
        &dir.path().join("state"),
        &dir.path().join("app-data"),
        &system_home,
        "codex-hook.sh",
        "/bin/sh '/h/.zerocode/agent-hooks/codex-hook.sh'",
        |_| {
            asked.set(asked.get() + 1);
            zerocode_hookd::codex_grant::grant(&grant_plan)
        },
    )
    .expect("land ran");
    let Landed::Mirror { why, retryable, .. } = landed else {
        panic!("an old codex was not landed in the home this app owns: {landed:?}");
    };
    let asked = asked.get();
    assert!(
        asked == 1 && !retryable && why.starts_with("codex app-server unsupported: "),
        "an old codex that says why late was asked {asked} time(s), retryable: {retryable}; the window says: {why}"
    );
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
    held.invocation.timeout = silent_deadline();
    let started = std::time::Instant::now();
    let outcome = zerocode_hookd::codex_grant::grant(&held);
    let took = started.elapsed();
    assert!(
        matches!(outcome, Err(GrantError::Timeout(_))),
        "a silent server was not a timeout: {outcome:?}"
    );
    // It gave up near the deadline rather than at the 300s the child asked for.
    assert!(took < GAVE_UP_WITHIN, "took {took:?}");
    assert_eq!(
        std::fs::read_to_string(&log).unwrap_or_default(),
        "started",
        "the silent server never ran, so this proved nothing"
    );
}

/// A server that talks before answering — notifications, other ids — is not a
/// failure. It is entitled to.
#[cfg_attr(
    not(unix),
    ignore = "the fake app server and its keys are POSIX paths; Codex's Windows key spelling is not implemented (codex_trust::normalize_source_path)"
)]
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
#[cfg_attr(
    not(unix),
    ignore = "the fake app server and its keys are POSIX paths; Codex's Windows key spelling is not implemented (codex_trust::normalize_source_path)"
)]
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
