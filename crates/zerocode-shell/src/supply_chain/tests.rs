//! The supply-chain lookup over a real loopback socket and a real disk: the
//! batches, the later pages, one read per record, the deadline, the failure
//! words, the day-long cache and its fingerprint, and what reaches the wire.
//! No case here touches the network beyond 127.0.0.1 — except the one
//! `#[ignore]`d measurement at the bottom.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use serde_json::{Value, json};
use zerocode_core::supply_chain::{OSV_BATCH_MAX_QUERIES, Origin};

use super::discover::LOCKFILE_SKIPPED_DIRECTORIES;
use super::osv::{BAD_ANSWER, OFFLINE, TIMEOUT, http_word};
use super::*;

/// A deadline no loopback answer comes near, for the cases that are not about
/// the deadline.
const GENEROUS: Duration = Duration::from_secs(20);

/// Epoch ms the cases are "now" at.
const NOW_MS: i64 = 1_789_600_000_000;

/// One request the fake OSV heard.
#[derive(Debug, Clone)]
struct Heard {
    method: String,
    path: String,
    body: String,
}

impl Heard {
    fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or(Value::Null)
    }
}

/// What the fake OSV answers one request with: status, body, and how long to
/// sit on it first.
type Reply = (u16, String, u64);

/// A loopback OSV answering every request through `answer`, one connection at
/// a time, remembering what it heard.
struct FakeOsv {
    addr: SocketAddr,
    heard: Arc<Mutex<Vec<Heard>>>,
}

impl FakeOsv {
    fn serving(answer: impl Fn(&Heard) -> Reply + Send + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback seat");
        let addr = listener.local_addr().expect("its address");
        let heard = Arc::new(Mutex::new(Vec::new()));
        let record = Arc::clone(&heard);
        thread::spawn(move || {
            for socket in listener.incoming() {
                let Ok(mut socket) = socket else {
                    return;
                };
                let Some(request) = read_request(&mut socket) else {
                    continue;
                };
                let (status, body, hold_ms) = answer(&request);
                record.lock().expect("the record").push(request);
                if hold_ms > 0 {
                    thread::sleep(Duration::from_millis(hold_ms));
                }
                let reply = format!(
                    "HTTP/1.1 {status} Fake\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(reply.as_bytes());
                let _ = socket.flush();
            }
        });
        Self { addr, heard }
    }

    fn wire(&self, deadline: Duration) -> OsvWire {
        OsvWire::at(&format!("http://{}", self.addr), deadline)
    }

    fn heard(&self) -> Vec<Heard> {
        self.heard.lock().expect("the record").clone()
    }

    fn batches(&self) -> Vec<Heard> {
        self.heard()
            .into_iter()
            .filter(|heard| heard.method == "POST" && heard.path == "/v1/querybatch")
            .collect()
    }

    fn record_reads(&self) -> Vec<String> {
        self.heard()
            .into_iter()
            .filter(|heard| heard.method == "GET")
            .map(|heard| heard.path)
            .collect()
    }
}

/// One HTTP/1.1 request off a socket: the head, then as many body bytes as it
/// declares.
fn read_request(socket: &mut TcpStream) -> Option<Heard> {
    let mut raw = Vec::new();
    let mut buffer = [0_u8; 16 * 1024];
    let head_end = loop {
        let read = socket.read(&mut buffer).ok()?;
        if read == 0 {
            return None;
        }
        raw.extend_from_slice(&buffer[..read]);
        if let Some(at) = raw.windows(4).position(|window| window == b"\r\n\r\n") {
            break at + 4;
        }
    };
    let head = String::from_utf8_lossy(&raw[..head_end]).into_owned();
    let length = head
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())
                .flatten()
        })
        .unwrap_or(0);
    while raw.len() < head_end + length {
        let read = socket.read(&mut buffer).ok()?;
        if read == 0 {
            break;
        }
        raw.extend_from_slice(&buffer[..read]);
    }
    let mut request_line = head.lines().next()?.split(' ');
    Some(Heard {
        method: request_line.next()?.to_string(),
        path: request_line.next()?.to_string(),
        body: String::from_utf8_lossy(&raw[head_end..]).into_owned(),
    })
}

/// A querybatch answer with no findings, sized to the batch it answers.
fn nothing_found(heard: &Heard) -> String {
    let asked = heard.json()["queries"].as_array().map_or(0, Vec::len);
    json!({ "results": vec![json!({}); asked] }).to_string()
}

/// An OSV record for `id`, rated by its CVSS v3 vector.
fn record_body(id: &str, aliases: &[&str]) -> String {
    json!({
        "id": id,
        "summary": format!("{id} summary"),
        "aliases": aliases,
        "severity": [{ "type": "CVSS_V3", "score": "CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:N/I:N/A:H" }],
        "affected": [{
            "package": { "ecosystem": "crates.io", "name": "time" },
            "ranges": [{ "type": "SEMVER", "events": [{ "introduced": "0" }, { "fixed": "0.2.23" }] }]
        }]
    })
    .to_string()
}

/// A workspace on disk holding `files`.
fn workspace(files: &[(&str, &str)]) -> tempfile::TempDir {
    let root = tempfile::tempdir().expect("a workspace");
    for (relative, text) in files {
        let path = root.path().join(relative);
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("its directory");
        std::fs::write(&path, text).expect("the lockfile");
    }
    root
}

/// A Cargo.lock of a member depending on `registry` crates.io packages, a git
/// dependency and a private-registry one.
fn cargo_lock(registry: &[(&str, &str)]) -> String {
    let mut text = String::from(
        "version = 4\n\n[[package]]\nname = \"member-app\"\nversion = \"0.1.0\"\ndependencies = [\n \"git-helper\",\n \"secret-internal\",\n",
    );
    for (name, version) in registry {
        text.push_str(&format!(" \"{name} {version}\",\n"));
    }
    text.push_str("]\n\n[[package]]\nname = \"git-helper\"\nversion = \"0.2.0\"\nsource = \"git+https://git.corp.example/helper#abc\"\n\n[[package]]\nname = \"secret-internal\"\nversion = \"9.9.9\"\nsource = \"registry+https://crates.corp.example/index\"\n");
    for (name, version) in registry {
        text.push_str(&format!(
            "\n[[package]]\nname = \"{name}\"\nversion = \"{version}\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\n"
        ));
    }
    text
}

fn answer_at(
    root: &Path,
    cache: &Path,
    osv: &FakeOsv,
    refresh: bool,
    now_ms: i64,
) -> SupplyChainAnswer {
    supply_chain_answer(root, cache, cache, &osv.wire(GENEROUS), refresh, now_ms)
}

#[test]
fn a_thousand_and_one_questions_are_two_batches_and_each_record_is_read_once() {
    let names: Vec<String> = (0..=OSV_BATCH_MAX_QUERIES)
        .map(|at| format!("crate-{at:04}"))
        .collect();
    let registry: Vec<(&str, &str)> = names.iter().map(|name| (name.as_str(), "1.0.0")).collect();
    let root = workspace(&[("Cargo.lock", &cargo_lock(&registry))]);
    let cache = tempfile::tempdir().expect("a cache");
    let osv = FakeOsv::serving(|heard| {
        if heard.method == "GET" {
            return (
                200,
                record_body("RUSTSEC-2020-0071", &["GHSA-wcg3-cvx6-7396"]),
                0,
            );
        }
        let queries = heard.json()["queries"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let results: Vec<Value> = queries
            .iter()
            .map(|query| match query["package"]["name"].as_str() {
                Some("crate-0000" | "crate-1000") => {
                    json!({ "vulns": [{ "id": "RUSTSEC-2020-0071", "modified": "x" }] })
                }
                _ => json!({}),
            })
            .collect();
        (200, json!({ "results": results }).to_string(), 0)
    });

    let answer = answer_at(root.path(), cache.path(), &osv, false, NOW_MS);

    let sizes: Vec<usize> = osv
        .batches()
        .iter()
        .map(|heard| heard.json()["queries"].as_array().map_or(0, Vec::len))
        .collect();
    assert_eq!(
        sizes,
        vec![OSV_BATCH_MAX_QUERIES, 1],
        "one request per thousand questions"
    );
    assert_eq!(
        osv.record_reads(),
        vec!["/v1/vulns/RUSTSEC-2020-0071".to_string()],
        "one read per record, however many packages name it"
    );
    assert_eq!(answer.lookup.state, LookupState::Fresh);
    assert_eq!(
        (
            answer.lookup.asked,
            answer.lookup.batches,
            answer.lookup.records
        ),
        (1_001, 2, 1)
    );
    assert_eq!(answer.lookup.checked_at, Some(NOW_MS));
    assert_eq!(answer.graph.vulnerabilities.len(), 1);
    let affects = answer
        .graph
        .edges
        .iter()
        .filter(|edge| edge.kind == zerocode_core::supply_chain::SupplyEdgeKind::Affects)
        .count();
    assert_eq!(
        affects, 2,
        "the one advisory reaches both packages OSV named it for"
    );
    assert_eq!(answer.lockfiles.len(), 1);
    assert_eq!(answer.lockfiles[0].component_count, 1_004);
}

#[test]
fn a_later_page_is_asked_only_for_the_questions_that_have_one() {
    let root = workspace(&[(
        "Cargo.lock",
        &cargo_lock(&[("time", "0.1.45"), ("serde", "1.0.219")]),
    )]);
    let cache = tempfile::tempdir().expect("a cache");
    let osv = FakeOsv::serving(|heard| {
        if heard.method == "GET" {
            let id = heard
                .path
                .rsplit('/')
                .next()
                .unwrap_or_default()
                .to_string();
            return (200, record_body(&id, &[]), 0);
        }
        let queries = heard.json()["queries"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let results: Vec<Value> = queries
            .iter()
            .map(|query| match (query["package"]["name"].as_str(), query["page_token"].as_str()) {
                (Some("time"), None) => json!({ "vulns": [{ "id": "RUSTSEC-2020-0071" }], "next_page_token": "time-page-2" }),
                (Some("time"), Some("time-page-2")) => json!({ "vulns": [{ "id": "RUSTSEC-2020-0159" }] }),
                _ => json!({}),
            })
            .collect();
        (200, json!({ "results": results }).to_string(), 0)
    });

    let answer = answer_at(root.path(), cache.path(), &osv, false, NOW_MS);

    let batches = osv.batches();
    assert_eq!(batches.len(), 2);
    assert_eq!(
        batches[1].json()["queries"],
        json!([{ "package": { "ecosystem": "crates.io", "name": "time" }, "version": "0.1.45", "page_token": "time-page-2" }]),
        "only the question with a later page is asked again, with its token"
    );
    assert_eq!(
        osv.record_reads(),
        vec![
            "/v1/vulns/RUSTSEC-2020-0071".to_string(),
            "/v1/vulns/RUSTSEC-2020-0159".to_string()
        ]
    );
    assert_eq!(
        answer.graph.vulnerabilities.len(),
        2,
        "both pages' advisories stand"
    );
}

#[test]
fn a_page_that_names_itself_again_is_a_bad_answer_not_a_loop() {
    let root = workspace(&[("Cargo.lock", &cargo_lock(&[("time", "0.1.45")]))]);
    let cache = tempfile::tempdir().expect("a cache");
    let osv = FakeOsv::serving(|_| {
        (
            200,
            json!({ "results": [{ "vulns": [], "next_page_token": "same" }] }).to_string(),
            0,
        )
    });

    let answer = answer_at(root.path(), cache.path(), &osv, false, NOW_MS);

    assert_eq!(answer.lookup.state, LookupState::Failed);
    assert_eq!(answer.lookup.reason.as_deref(), Some(BAD_ANSWER));
    assert_eq!(
        answer.lookup.batches, 2,
        "the first page, then the page that named itself"
    );
}

#[test]
fn a_server_slower_than_the_deadline_is_a_timeout_and_the_components_still_stand() {
    let root = workspace(&[("Cargo.lock", &cargo_lock(&[("time", "0.1.45")]))]);
    let cache = tempfile::tempdir().expect("a cache");
    let deadline = Duration::from_millis(300);
    let osv = FakeOsv::serving(move |heard| (200, nothing_found(heard), 1_500));

    let began = std::time::Instant::now();
    let answer = supply_chain_answer(
        root.path(),
        cache.path(),
        cache.path(),
        &osv.wire(deadline),
        false,
        NOW_MS,
    );

    assert!(
        began.elapsed() < deadline + Duration::from_millis(700),
        "the deadline bounds the whole lookup: {:?}",
        began.elapsed()
    );
    assert_eq!(answer.lookup.state, LookupState::Failed);
    assert_eq!(answer.lookup.reason.as_deref(), Some(TIMEOUT));
    assert_eq!(answer.lookup.checked_at, None);
    assert_eq!(
        answer.graph.components.len(),
        4,
        "a failed lookup still draws every component"
    );
    assert!(answer.graph.vulnerabilities.is_empty());
    assert!(
        super::cache::read(&super::cache::file(cache.path(), root.path())).is_none(),
        "a failure is not cached"
    );
}

#[test]
fn a_refusal_is_its_status_word_and_a_non_json_answer_is_a_bad_answer() {
    let root = workspace(&[("Cargo.lock", &cargo_lock(&[("time", "0.1.45")]))]);
    for (reply, word) in [
        ((503, "{}".to_string(), 0), http_word(503)),
        ((429, "{}".to_string(), 0), http_word(429)),
        (
            (200, "<html>not json</html>".to_string(), 0),
            BAD_ANSWER.to_string(),
        ),
        (
            (200, json!({ "results": [] }).to_string(), 0),
            BAD_ANSWER.to_string(),
        ),
    ] {
        let cache = tempfile::tempdir().expect("a cache");
        let osv = FakeOsv::serving(move |_| reply.clone());
        let answer = answer_at(root.path(), cache.path(), &osv, false, NOW_MS);
        assert_eq!(answer.lookup.state, LookupState::Failed, "{word}");
        assert_eq!(answer.lookup.reason.as_deref(), Some(word.as_str()));
        assert_eq!(osv.batches().len(), 1, "one request, no retry ({word})");
        assert_eq!(answer.graph.components.len(), 4);
    }
    assert_eq!(http_word(503), "http_503");
}

#[test]
fn a_seat_nobody_answers_is_offline() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback seat");
    let addr = listener.local_addr().expect("its address");
    drop(listener);
    let root = workspace(&[("Cargo.lock", &cargo_lock(&[("time", "0.1.45")]))]);
    let cache = tempfile::tempdir().expect("a cache");

    let answer = supply_chain_answer(
        root.path(),
        cache.path(),
        cache.path(),
        &OsvWire::at(&format!("http://{addr}"), GENEROUS),
        false,
        NOW_MS,
    );

    assert_eq!(answer.lookup.reason.as_deref(), Some(OFFLINE));
    assert_eq!(answer.lookup.batches, 1);
}

#[test]
fn a_day_fresh_answer_about_the_same_lockfiles_sends_nothing() {
    let root = workspace(&[("Cargo.lock", &cargo_lock(&[("time", "0.1.45")]))]);
    let cache = tempfile::tempdir().expect("a cache");
    let osv = FakeOsv::serving(|heard| {
        if heard.method == "GET" {
            return (200, record_body("RUSTSEC-2020-0071", &[]), 0);
        }
        (
            200,
            json!({ "results": [{ "vulns": [{ "id": "RUSTSEC-2020-0071" }] }] }).to_string(),
            0,
        )
    });

    let first = answer_at(root.path(), cache.path(), &osv, false, NOW_MS);
    let heard_after_first = osv.heard().len();
    let later = NOW_MS + super::cache::FRESH_FOR_MS - 1;
    let second = answer_at(root.path(), cache.path(), &osv, false, later);

    assert_eq!(first.lookup.state, LookupState::Fresh);
    assert_eq!(heard_after_first, 2);
    assert_eq!(second.lookup.state, LookupState::Cached);
    assert_eq!(
        osv.heard().len(),
        heard_after_first,
        "a cache hit sends nothing"
    );
    assert_eq!((second.lookup.batches, second.lookup.records), (0, 0));
    assert_eq!(
        second.lookup.checked_at,
        Some(NOW_MS),
        "the card says when OSV answered, not when it was read"
    );
    assert_eq!(second.graph.vulnerabilities, first.graph.vulnerabilities);
    assert_eq!(second.graph.edges, first.graph.edges);
    let file = super::cache::file(cache.path(), root.path());
    assert!(file.starts_with(cache.path().join(super::cache::DIR_NAME)));
    assert_eq!(
        file.file_name()
            .and_then(|name| name.to_str())
            .map(str::len),
        Some(64 + ".json".len())
    );

    let day_later = answer_at(
        root.path(),
        cache.path(),
        &osv,
        false,
        NOW_MS + super::cache::FRESH_FOR_MS,
    );
    assert_eq!(
        day_later.lookup.state,
        LookupState::Fresh,
        "a day-old answer is asked again"
    );
    assert_eq!(osv.batches().len(), 2);

    let refreshed = answer_at(
        root.path(),
        cache.path(),
        &osv,
        true,
        NOW_MS + super::cache::FRESH_FOR_MS + 1,
    );
    assert_eq!(
        refreshed.lookup.state,
        LookupState::Fresh,
        "refresh asks inside the day"
    );
    assert_eq!(osv.batches().len(), 3);
}

#[test]
fn a_changed_lockfile_is_asked_again_and_a_failure_keeps_the_last_answer_about_the_same_lockfiles()
{
    let root = workspace(&[("Cargo.lock", &cargo_lock(&[("time", "0.1.45")]))]);
    let cache = tempfile::tempdir().expect("a cache");
    let failing = Arc::new(Mutex::new(false));
    let fail = Arc::clone(&failing);
    let osv = FakeOsv::serving(move |heard| {
        if *fail.lock().expect("the switch") {
            return (503, "{}".to_string(), 0);
        }
        if heard.method == "GET" {
            return (200, record_body("RUSTSEC-2020-0071", &[]), 0);
        }
        let asked = heard.json()["queries"].as_array().map_or(0, Vec::len);
        let results: Vec<Value> = (0..asked)
            .map(|_| json!({ "vulns": [{ "id": "RUSTSEC-2020-0071" }] }))
            .collect();
        (200, json!({ "results": results }).to_string(), 0)
    });

    let first = answer_at(root.path(), cache.path(), &osv, false, NOW_MS);
    assert_eq!(first.lookup.state, LookupState::Fresh);

    std::fs::write(
        root.path().join("Cargo.lock"),
        cargo_lock(&[("time", "0.1.45"), ("serde", "1.0.219")]),
    )
    .expect("the lockfile moves");
    let moved = answer_at(root.path(), cache.path(), &osv, false, NOW_MS + 1);
    assert_eq!(
        moved.lookup.state,
        LookupState::Fresh,
        "new lockfile bytes are a new question"
    );
    assert_eq!(moved.lookup.asked, 2);
    assert_eq!(osv.batches().len(), 2);

    *failing.lock().expect("the switch") = true;
    let expired = NOW_MS + 1 + super::cache::FRESH_FOR_MS;
    let failed = answer_at(root.path(), cache.path(), &osv, false, expired);
    assert_eq!(failed.lookup.state, LookupState::Failed);
    assert_eq!(failed.lookup.reason.as_deref(), Some("http_503"));
    assert_eq!(
        failed.lookup.checked_at,
        Some(NOW_MS + 1),
        "the last answer, with the time it was given"
    );
    assert_eq!(failed.graph.vulnerabilities, moved.graph.vulnerabilities);

    std::fs::write(
        root.path().join("Cargo.lock"),
        cargo_lock(&[("time", "0.1.45")]),
    )
    .expect("moves back");
    let other = answer_at(root.path(), cache.path(), &osv, false, expired);
    assert_eq!(other.lookup.state, LookupState::Failed);
    assert_eq!(
        other.lookup.checked_at, None,
        "an answer about other lockfiles is not this one's"
    );
    assert!(other.graph.vulnerabilities.is_empty());
}

#[test]
fn nothing_but_public_registry_names_reaches_the_socket() {
    let npm = json!({
        "name": "private-site", "lockfileVersion": 3,
        "packages": {
            "": { "name": "private-site", "dependencies": { "left-pad": "^1.3.0", "@corp/secret-ui": "^1.0.0", "linked-tool": "*" } },
            "node_modules/left-pad": { "version": "1.3.0", "resolved": "https://registry.npmjs.org/left-pad/-/left-pad-1.3.0.tgz" },
            "node_modules/@corp/secret-ui": { "version": "1.0.0", "resolved": "https://npm.corp.example/@corp/secret-ui/-/secret-ui-1.0.0.tgz" },
            "node_modules/git-thing": { "version": "0.0.1", "resolved": "git+ssh://git@git.corp.example/git-thing.git#abc" },
            "node_modules/tarball-thing": { "version": "0.0.2", "resolved": "file:vendor/tarball-thing-0.0.2.tgz" },
            "tools/linked-tool": { "name": "linked-tool", "version": "0.0.3" },
            "node_modules/linked-tool": { "resolved": "tools/linked-tool", "link": true }
        }
    });
    let root = workspace(&[
        ("Cargo.lock", &cargo_lock(&[("time", "0.1.45")])),
        ("web/package-lock.json", &npm.to_string()),
    ]);
    let cache = tempfile::tempdir().expect("a cache");
    let osv = FakeOsv::serving(|heard| (200, nothing_found(heard), 0));

    let answer = answer_at(root.path(), cache.path(), &osv, false, NOW_MS);

    let wire: String = osv
        .heard()
        .iter()
        .map(|heard| format!("{} {}\n{}\n", heard.method, heard.path, heard.body))
        .collect();
    let root_text = root.path().to_string_lossy().into_owned();
    for private in [
        "member-app",
        "git-helper",
        "secret-internal",
        "corp",
        "private-site",
        "secret-ui",
        "git-thing",
        "tarball-thing",
        "linked-tool",
        "vendor",
        "web/",
        "Cargo.lock",
        root_text.as_str(),
    ] {
        assert!(
            !wire.contains(private),
            "`{private}` reached the socket:\n{wire}"
        );
    }
    let asked: Vec<Value> = osv.batches()[0].json()["queries"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        asked,
        vec![
            json!({ "package": { "ecosystem": "crates.io", "name": "time" }, "version": "0.1.45" }),
            json!({ "package": { "ecosystem": "npm", "name": "left-pad" }, "version": "1.3.0" }),
        ]
    );
    assert_eq!(answer.lookup.asked, 2);
    let origins: Vec<Origin> = answer
        .graph
        .components
        .iter()
        .map(|component| component.origin)
        .collect();
    assert!(
        origins.contains(&Origin::Git)
            && origins.contains(&Origin::Private)
            && origins.contains(&Origin::Path)
    );
}

#[test]
fn the_walk_goes_three_directories_deep_and_never_into_installed_or_built_trees() {
    // `apps/web/package-lock.json` is depth 3 (`LOCKFILE_SEARCH_DEPTH`); the
    // fixture's is depth 4.
    let npm = json!({ "lockfileVersion": 3, "packages": { "": { "name": "x" } } }).to_string();
    let cargo = cargo_lock(&[]);
    let mut files: Vec<(String, &str)> = vec![
        ("Cargo.lock".to_string(), cargo.as_str()),
        ("apps/web/package-lock.json".to_string(), npm.as_str()),
        ("fixtures/deep/repo/Cargo.lock".to_string(), cargo.as_str()),
        ("yarn.lock".to_string(), "not read"),
    ];
    for skipped in LOCKFILE_SKIPPED_DIRECTORIES {
        files.push((format!("{skipped}/Cargo.lock"), cargo.as_str()));
        files.push((format!("apps/{skipped}/package-lock.json"), npm.as_str()));
    }
    let borrowed: Vec<(&str, &str)> = files
        .iter()
        .map(|(path, text)| (path.as_str(), *text))
        .collect();
    let root = workspace(&borrowed);

    let found: Vec<String> = super::discover::lockfiles(root.path())
        .into_iter()
        .map(|lockfile| lockfile.relative)
        .collect();

    assert_eq!(
        found,
        vec![
            "Cargo.lock".to_string(),
            "apps/web/package-lock.json".to_string()
        ]
    );
}

#[test]
fn an_unreadable_lockfile_is_named_and_the_others_still_stand() {
    let npm = json!({ "lockfileVersion": 3, "packages": {
        "": { "name": "site" },
        "node_modules/left-pad": { "version": "1.3.0", "resolved": "https://registry.npmjs.org/left-pad/-/left-pad-1.3.0.tgz" }
    } })
    .to_string();
    let root = workspace(&[
        ("Cargo.lock", "[[package]\nname ="),
        ("package-lock.json", &npm),
    ]);
    let cache = tempfile::tempdir().expect("a cache");
    let osv = FakeOsv::serving(|heard| (200, nothing_found(heard), 0));

    let answer = answer_at(root.path(), cache.path(), &osv, false, NOW_MS);

    assert_eq!(answer.lockfiles.len(), 2);
    assert_eq!(answer.lockfiles[0].path, "Cargo.lock");
    assert_eq!(answer.lockfiles[0].component_count, 0);
    assert!(answer.lockfiles[0].unreadable.is_some());
    assert_eq!(
        (
            answer.lockfiles[1].component_count,
            answer.lockfiles[1].unreadable.as_deref()
        ),
        (2, None)
    );
    assert_eq!(answer.graph.components.len(), 2);
    assert_eq!(answer.lookup.state, LookupState::Fresh);
}

#[test]
fn a_workspace_with_nothing_to_ask_sends_nothing() {
    let npm = json!({ "lockfileVersion": 3, "packages": { "": { "name": "site" } } }).to_string();
    let root = workspace(&[("package-lock.json", &npm)]);
    let cache = tempfile::tempdir().expect("a cache");
    let osv = FakeOsv::serving(|heard| (200, nothing_found(heard), 0));

    let answer = answer_at(root.path(), cache.path(), &osv, false, NOW_MS);

    assert_eq!(answer.lookup.state, LookupState::Fresh);
    assert_eq!((answer.lookup.asked, answer.lookup.batches), (0, 0));
    assert!(osv.heard().is_empty());
}

#[test]
fn the_answer_is_camel_case_for_the_window() {
    let root = workspace(&[("Cargo.lock", &cargo_lock(&[("time", "0.1.45")]))]);
    let cache = tempfile::tempdir().expect("a cache");
    let osv = FakeOsv::serving(|heard| (200, nothing_found(heard), 0));

    let wire = serde_json::to_value(answer_at(root.path(), cache.path(), &osv, false, NOW_MS))
        .expect("an answer");

    let mut keys: Vec<&str> = wire
        .as_object()
        .expect("an object")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        vec![
            "components",
            "edges",
            "lockfiles",
            "lookup",
            "root",
            "vulnerabilities"
        ]
    );
    let mut lookup: Vec<&str> = wire["lookup"]
        .as_object()
        .expect("lookup")
        .keys()
        .map(String::as_str)
        .collect();
    lookup.sort_unstable();
    assert_eq!(
        lookup,
        vec![
            "asked",
            "batches",
            "checkedAt",
            "elapsedMs",
            "reason",
            "records",
            "state"
        ]
    );
    assert_eq!(wire["lookup"]["state"], json!("fresh"));
    assert_eq!(
        wire["lockfiles"][0],
        json!({ "path": "Cargo.lock", "ecosystem": "cargo", "componentCount": 4, "unreadable": null })
    );
}

#[test]
fn a_root_that_is_not_a_directory_is_refused() {
    let root = workspace(&[("Cargo.lock", "")]);
    assert!(workspace_root(&root.path().join("Cargo.lock")).is_err());
    assert!(workspace_root(&root.path().join("missing")).is_err());
    assert_eq!(
        workspace_root(root.path()).expect("a directory"),
        std::fs::canonicalize(root.path()).expect("canonical")
    );
}

/// G6 (docs/design/knowledge-supply-chain-20260917.md §2): this repository's
/// supply chain against OSV itself — the one case that leaves the machine,
/// carrying only public-registry names. Prints the counts, the first lookup's
/// and the cache hit's wall time, and the RustSec ids for `cargo audit` to be
/// held against. The cache goes to a scratch directory, never the window's.
#[test]
#[ignore = "a measurement against api.osv.dev, printed; not a check"]
fn supply_chain_live_osv_measurement() {
    let root = workspace_root(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
        .expect("this repository");
    let cache = tempfile::tempdir().expect("a cache");
    let now = crate::project_runtime::now_epoch_ms();

    let first = supply_chain_answer(
        &root,
        cache.path(),
        cache.path(),
        &OsvWire::public(),
        false,
        now,
    );
    let second = supply_chain_answer(
        &root,
        cache.path(),
        cache.path(),
        &OsvWire::public(),
        false,
        now + 1,
    );

    for lockfile in &first.lockfiles {
        println!(
            "lockfile {}: {} components {:?}",
            lockfile.path, lockfile.component_count, lockfile.unreadable
        );
    }
    println!(
        "components {} · asked {} · batches {} · records {} · vulnerabilities {} · state {:?} {:?}",
        first.graph.components.len(),
        first.lookup.asked,
        first.lookup.batches,
        first.lookup.records,
        first.graph.vulnerabilities.len(),
        first.lookup.state,
        first.lookup.reason
    );
    println!(
        "first lookup {} ms · cache hit {} ms ({:?})",
        first.lookup.elapsed_ms, second.lookup.elapsed_ms, second.lookup.state
    );
    let mut rustsec: Vec<&str> = first
        .graph
        .vulnerabilities
        .iter()
        .flat_map(|vulnerability| std::iter::once(&vulnerability.id).chain(&vulnerability.aliases))
        .map(String::as_str)
        .filter(|id| id.starts_with("RUSTSEC-"))
        .collect();
    rustsec.sort_unstable();
    rustsec.dedup();
    println!("rustsec {} {}", rustsec.len(), rustsec.join(" "));
    for vulnerability in &first.graph.vulnerabilities {
        println!(
            "vulnerability {} {:?} {:?} informational={:?} aliases={}",
            vulnerability.id,
            vulnerability.severity,
            vulnerability.score,
            vulnerability.informational,
            vulnerability.aliases.join(",")
        );
    }
}
