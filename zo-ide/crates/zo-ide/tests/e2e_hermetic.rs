//! Deterministic process-level coverage for the zo IDE front end.
//!
//! Every interactive case runs the real `zo` binary in a 120x40 PTY. The
//! provider is either the workspace's `MockAnthropicService` or a test-local
//! fixed SSE script, and every state directory is temporary. No test can
//! reach a real provider or the developer's credential store.

#![cfg(unix)]

mod e2e;

use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use e2e::contract::{self, Rule};
use e2e::harness::{history_rows_raw, run_pipe, run_pipe_with_env, visible_row_occupancy, PtyRun, Screen};
use e2e::scripted::{user_text_contains, ScriptedAnthropicService};
use mock_anthropic_service::{CapturedRequest, MockAnthropicService};
use tempfile::TempDir;
use zerocode_harness::{method, Client};

const DIRECT_ANSWER: &str = "### Deterministic answer\n\n- first item\n- second item\n";
const TABLE_ANSWER: &str = "| Name | Count | Status |\n| --- | --- | --- |\n| alpha | 1 | ok |\n| beta | 22 | pending |\n| gamma | 333 | failed |\n| delta | 4444 | ok |\n";
/// The same table, 36 body rows instead of 4 — the second point of a slope.
///
/// One capture cannot separate a turn's fixed cost (boot banner, the composer
/// echoing the typed prompt) from what one more line of output actually costs,
/// and dividing total bytes by committed lines silently blames the streaming
/// for the fixed part. Two sizes give the marginal byte per line directly.
const TABLE_ANSWER_BIG: &str = concat!(
    "| Name | Count | Status |\n| --- | --- | --- |\n",
    "| alpha | 1 | ok |\n| beta | 22 | pending |\n| gamma | 333 | failed |\n| delta | 4444 | ok |\n",
    "| epsilon | 5 | ok |\n| zeta | 66 | pending |\n| eta | 777 | failed |\n| theta | 8888 | ok |\n",
    "| iota | 9 | ok |\n| kappa | 10 | pending |\n| lambda | 111 | failed |\n| mu | 1212 | ok |\n",
    "| nu | 13 | ok |\n| xi | 14 | pending |\n| omicron | 151 | failed |\n| pi | 1616 | ok |\n",
    "| rho | 17 | ok |\n| sigma | 18 | pending |\n| tau | 191 | failed |\n| upsilon | 2020 | ok |\n",
    "| phi | 21 | ok |\n| chi | 22 | pending |\n| psi | 231 | failed |\n| omega | 2424 | ok |\n",
    "| alef | 25 | ok |\n| bet | 26 | pending |\n| gimel | 271 | failed |\n| dalet | 2828 | ok |\n",
    "| he | 29 | ok |\n| vav | 30 | pending |\n| zayin | 311 | failed |\n| het | 3232 | ok |\n",
    "| tet | 33 | ok |\n| yod | 34 | pending |\n| kaf | 351 | failed |\n| lamed | 3636 | ok |\n",
);
const TEST_TIMEOUT: Duration = Duration::from_secs(10);

const CONTINUE_ARGS: [&str; 2] = ["--continue", "--permission-mode"];
const PACKAGE_VERSION: &str = env!("CARGO_PKG_VERSION");
const VERSION_TOKEN: &[u8] = b"v{VERSION}";

struct Layout {
    root: TempDir,
    cwd: PathBuf,
    home: PathBuf,
    sessions: PathBuf,
    state: PathBuf,
}

impl Layout {
    fn new() -> Self {
        let root = tempfile::tempdir().expect("temporary e2e root");
        let cwd = root.path().join("workspace");
        let home = root.path().join("home");
        let sessions = root.path().join("sessions");
        let state = root.path().join("state");
        for path in [&cwd, &home, &sessions, &state] {
            fs::create_dir_all(path).expect("e2e directory");
        }
        fs::write(
            home.join("settings.json"),
            br#"{"smart":{"autoClassifier":"off"}}"#,
        )
        .expect("disable routing probe in e2e home");
        Self {
            root,
            cwd,
            home,
            sessions,
            state,
        }
    }

    /// A script that owns the tool sequence must not be consumed by a host
    /// decomposition request before the model turn begins.
    fn model_led(&self) {
        let path = self.home.join("settings.json");
        let mut settings: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        settings["smart"]["orchestration"] = serde_json::json!("model");
        fs::write(path, serde_json::to_vec(&settings).unwrap()).unwrap();
    }

    fn fixture(&self, contents: &str) {
        fs::write(self.cwd.join("fixture.txt"), contents).expect("fixture file");
    }

    /// Rewrite `settings.json` with hooks beside the routing-probe switch.
    ///
    /// The switch has to survive: without it the session opens a classifier
    /// request no scenario scripted, and the hook cases count requests.
    fn with_hooks(&self, hooks: &serde_json::Value) {
        let settings = serde_json::json!({
            "smart": {"autoClassifier": "off"},
            "hooks": hooks.clone(),
        });
        fs::write(
            self.home.join("settings.json"),
            serde_json::to_vec(&settings).expect("serialize hook settings"),
        )
        .expect("rewrite e2e settings with hooks");
    }

    /// Write an executable hook whose whole stdout is `stdout`, and answer with
    /// its path.
    ///
    /// A file rather than an inline `printf`: the command travels through
    /// `settings.json` and then through `sh -lc`, and a JSON payload quoted for
    /// both is unreadable — a test whose fixture cannot be read by eye is a
    /// test nobody can tell is still asking the right question. It lands beside
    /// the temporary root rather than in the workspace, the config home or the
    /// state dir, so nothing under test can mistake it for its own file.
    fn hook_script(&self, name: &str, stdout: &str, exit_code: i32) -> String {
        use std::os::unix::fs::PermissionsExt;

        let dir = self.root.path().join("hooks");
        fs::create_dir_all(&dir).expect("hook script directory");
        let path = dir.join(name);
        fs::write(
            &path,
            format!("#!/bin/sh\ncat <<'ZO_HOOK_JSON'\n{stdout}\nZO_HOOK_JSON\nexit {exit_code}\n"),
        )
        .expect("write hook script");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755))
            .expect("make the hook script executable");
        path.to_string_lossy().into_owned()
    }
}

fn pty(layout: &Layout, base_url: &str, args: &[&str]) -> PtyRun {
    PtyRun::spawn(
        &layout.cwd,
        &layout.home,
        &layout.sessions,
        &layout.state,
        base_url,
        args,
    )
    .expect("spawn zo in PTY")
}

fn interactive_args() -> [&'static str; 2] {
    ["--permission-mode", "danger-full-access"]
}

async fn connect_to_addr_file(path: &std::path::Path, token: &str) -> Client {
    let deadline = Instant::now() + TEST_TIMEOUT;
    loop {
        if let Ok(address) = fs::read_to_string(path) {
            return Client::connect(address.trim(), Some(token.to_string()))
                .await
                .expect("connect to pane events channel");
        }
        assert!(
            Instant::now() < deadline,
            "events address file {} did not appear",
            path.display()
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// 골든 파일의 자리.
fn golden_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/e2e/golden")
        .join(format!("{name}.bin"))
}

/// 실행마다 달라지는 것만 가린다 — 그 밖은 전부 계약이다.
///
/// 스트림 어디에나 안전하게 쓸 수 있는 규칙만 여기 있다: 세션 id(시각에서
/// 나온다)와 임시 디렉터리 경로. 색·간격·순서·문안은 가리지 않는다.
fn mask_volatile(bytes: &[u8], layout: &Layout) -> Vec<u8> {
    let mut masked = mask_session_ids(bytes);
    for path in [&layout.cwd, &layout.home, &layout.sessions, &layout.state] {
        masked = replace_bytes(&masked, path.to_string_lossy().as_bytes(), b"<path>");
    }
    masked
}

/// 히스토리 **한 행**에만 쓰는 규칙. 부팅 카드의 `directory:` 행은 폭에 맞춰
/// 가운데가 접히므로 경로 문자열이 원문으로 남지 않는다 — 라벨 뒤를 통째로
/// 가린다. 행 단위라서 안전하다(스트림 전체에 쓰면 뒤를 전부 잘라 먹는다).
fn mask_history_row(row: &[u8], layout: &Layout) -> Vec<u8> {
    let mut masked = mask_volatile(row, layout);
    if let Some(at) = find(&masked, b"directory:") {
        masked.truncate(at + b"directory:".len());
        masked.extend_from_slice(b" <cwd>");
    }
    masked
}

/// `session-<millis>-<n>` 을 고정 문자열로. 숫자 자리는 시계에서 온다.
fn mask_session_ids(row: &[u8]) -> Vec<u8> {
    const MARKER: &[u8] = b"session-";
    let mut out = Vec::with_capacity(row.len());
    let mut index = 0;
    while index < row.len() {
        if row[index..].starts_with(MARKER) {
            let mut end = index + MARKER.len();
            while row
                .get(end)
                .is_some_and(|byte| byte.is_ascii_digit() || *byte == b'-')
            {
                end += 1;
            }
            out.extend_from_slice(b"session-<id>");
            index = end;
            continue;
        }
        out.push(row[index]);
        index += 1;
    }
    out
}

fn replace_bytes(haystack: &[u8], needle: &[u8], with: &[u8]) -> Vec<u8> {
    if needle.is_empty() {
        return haystack.to_vec();
    }
    let mut out = Vec::with_capacity(haystack.len());
    let mut index = 0;
    while index < haystack.len() {
        if haystack[index..].starts_with(needle) {
            out.extend_from_slice(with);
            index += needle.len();
            continue;
        }
        out.push(haystack[index]);
        index += 1;
    }
    out
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// 골든과 견준다. `ZO_E2E_BLESS=1` 이면 대신 **다시 쓴다** — 기본 실행에서는
/// 파일을 쓰지 않는다(관문이 스스로 정답을 고쳐 쓰면 관문이 아니다).
fn assert_golden_bytes(actual: &[u8], name: &str, scenario: &str) {
    let path = golden_path(name);
    if std::env::var_os("ZO_E2E_BLESS").is_some() {
        fs::write(&path, mask_version(actual, PACKAGE_VERSION)).expect("write golden");
        return;
    }
    let expected = fs::read(&path)
        .unwrap_or_else(|error| panic!("{scenario}: read {}: {error}", path.display()));
    compare_golden_bytes(&expected, actual, PACKAGE_VERSION).unwrap_or_else(|error| {
        panic!(
            "{scenario}: {error}\n\n\
             (`ZO_E2E_BLESS=1 cargo test -p zo-ide --test e2e_hermetic` rewrites the golden \
             once the change is intended)"
        )
    });
}

/// Releases change the boot card and pipe heading, but not the transcript contract.
/// Match only the supplied package version; other version-like text is still evidence.
fn mask_version(bytes: &[u8], version: &str) -> Vec<u8> {
    let tokened = replace_bytes(bytes, format!("v{version}").as_bytes(), VERSION_TOKEN);
    fold_padding_after(&tokened, VERSION_TOKEN)
}

/// After every `token`, the first run of spaces folds to one: that run is the
/// box padding whose length is the version's width in disguise.
fn fold_padding_after(bytes: &[u8], token: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at..].starts_with(token) {
            out.extend_from_slice(token);
            at += token.len();
            // Copy the tail glued to the token (a closing paren, a colour
            // reset) up to the padding, then fold the padding itself.
            while at < bytes.len() && bytes[at] != b' ' && bytes[at] != b'\n' {
                out.push(bytes[at]);
                at += 1;
            }
            let mut spaces = 0;
            while at < bytes.len() && bytes[at] == b' ' {
                spaces += 1;
                at += 1;
            }
            if spaces > 0 {
                out.push(b' ');
            }
            continue;
        }
        out.push(bytes[at]);
        at += 1;
    }
    out
}

/// The version's WIDTH must not leak into the row either: `v1.3.9` and
/// `v1.3.10` pad the boot card's first row differently, so the golden blessed
/// under one width would fail every patch that changes it (v1.3.10 was the
/// first). Both sides of a comparison are masked, so nothing else moves.
#[test]
fn the_version_mask_hides_the_versions_width_too() {
    let five: &[u8] = "\u{1b}[2m│ >_ zo (v1.3.9)      │ next".as_bytes();
    let six: &[u8] = "\u{1b}[2m│ >_ zo (v1.3.10)     │ next".as_bytes();
    assert_eq!(mask_version(five, "1.3.9"), mask_version(six, "1.3.10"));
    // A golden already holding the token compares equal to a fresh capture.
    let golden: &[u8] = "\u{1b}[2m│ >_ zo (v{VERSION})      │ next".as_bytes();
    assert_eq!(mask_version(golden, "1.3.10"), mask_version(six, "1.3.10"));
}

fn compare_golden_bytes(expected: &[u8], actual: &[u8], version: &str) -> Result<(), String> {
    let expected = mask_version(expected, version);
    let actual = mask_version(actual, version);
    if expected == actual {
        return Ok(());
    }
    // Goldens are rows, not a stream protocol. Text patches may leave one
    // record terminator at EOF; accept it only when removing that byte makes
    // the complete golden equal. A real final empty row still compares above.
    if expected
        .strip_suffix(b"\n")
        .is_some_and(|without_terminator| without_terminator == actual.as_slice())
    {
        return Ok(());
    }
    let expected_rows: Vec<&[u8]> = expected.split(|byte| *byte == b'\n').collect();
    let actual_rows: Vec<&[u8]> = actual.split(|byte| *byte == b'\n').collect();
    for (index, (want, got)) in expected_rows.iter().zip(actual_rows.iter()).enumerate() {
        if want != got {
            return Err(format!(
                "row {index} differs\n  want: {}\n  got : {}",
                escape(want),
                escape(got)
            ));
        }
    }
    Err(format!(
        "row count differs — want {}, got {}\n  first extra: {}",
        expected_rows.len(),
        actual_rows.len(),
        escape(
            expected_rows
                .get(actual_rows.len())
                .or_else(|| actual_rows.get(expected_rows.len()))
                .unwrap_or(&&b""[..])
        )
    ))
}

#[test]
fn golden_comparison_masks_the_supplied_version_on_both_sides() {
    let tokenized = [b"\x1b[1mzo (".as_slice(), VERSION_TOKEN, b")\x1b[22m\nzo ", VERSION_TOKEN].concat();
    for version in [PACKAGE_VERSION, "98.76.54", "2.0.0-rc.1+build.42"] {
        let rendered = format!("\x1b[1mzo (v{version})\x1b[22m\nzo v{version}");
        for (expected, actual) in [
            (tokenized.as_slice(), rendered.as_bytes()),
            (rendered.as_bytes(), tokenized.as_slice()),
        ] {
            assert_eq!(compare_golden_bytes(expected, actual, version), Ok(()));
        }
    }
}

#[test]
fn golden_comparison_rejects_a_non_version_byte_difference() {
    let expected = format!("\x1b[1mzo (v{PACKAGE_VERSION})\x1b[22m\nanswer");
    for (from, to) in [(b'z', b'Z'), (b'1', b'2'), (b'(', b' '), (b'a', b'A')] {
        let mut actual = expected.as_bytes().to_vec();
        let index = actual.iter().position(|byte| *byte == from).expect("fixture byte");
        actual[index] = to;
        assert!(compare_golden_bytes(expected.as_bytes(), &actual, PACKAGE_VERSION).is_err());
    }
}

/// 실패 메시지에서 SGR 이 터미널을 물들이지 않도록 이스케이프한다.
fn escape(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).replace('\u{1b}', "ESC")
}

/// 스크롤백에 커밋된 **모든 행**을 바이트 그대로 골든과 견준다.
/// The golden pins the transcript's rows byte for byte, as a PREFIX of what
/// was captured: the test waits until the golden's last row is committed and
/// then finishes, and whatever the TUI commits after that moment — the
/// turn-end rule, the `/exit` echo — is timing, not transcript. Comparing the
/// whole capture made the goldens race the commit animation (red 1 run in 5 on
/// this machine: "row count differs — want 15, got 19").
fn assert_history_golden(capture: &[u8], name: &str, scenario: &str, layout: &Layout) {
    let rows = history_rows_raw(capture);
    assert!(!rows.is_empty(), "{scenario}: no history rows were captured");
    let masked: Vec<Vec<u8>> = rows
        .iter()
        .map(|row| mask_history_row(row, layout))
        .collect();
    if std::env::var_os("ZO_E2E_BLESS").is_some() {
        assert_golden_bytes(&masked.join(&b'\n'), name, scenario);
        return;
    }
    let path = golden_path(name);
    let expected = fs::read(&path)
        .unwrap_or_else(|error| panic!("{scenario}: read {}: {error}", path.display()));
    let expected_rows: Vec<&[u8]> = expected.split(|byte| *byte == b'\n').collect();
    let prefix: Vec<Vec<u8>> = masked.iter().take(expected_rows.len()).cloned().collect();
    assert!(
        prefix.len() == expected_rows.len(),
        "{scenario}: only {} of the golden's {} rows were captured\n  last captured: {}",
        prefix.len(),
        expected_rows.len(),
        prefix.last().map_or_else(String::new, |row| escape(row))
    );
    assert_golden_bytes(&prefix.join(&b'\n'), name, scenario);
}

/// 뷰포트 화면(피커 등)은 프레임이 여러 번 다시 그려지므로 캡처 전체를 그대로
/// 비교할 수 없다. 대신 **자리잡은 마지막 프레임**에서 화면 한 덩어리를 잘라
/// 골든으로 삼고, 그 덩어리가 캡처 안에 바이트 그대로 있는지 본다. 줄 단위
/// 마커보다 훨씬 촘촘하다: 색·간격·행 순서가 모두 골든 안에 있다.
fn assert_block_golden(
    capture: &[u8],
    name: &str,
    scenario: &str,
    layout: &Layout,
    start: &[u8],
    end: &[u8],
) {
    let masked = mask_volatile(capture, layout);
    let block = settled_block(&masked, start, end)
        .unwrap_or_else(|| panic!("{scenario}: neither anchor was drawn in the capture"));
    if std::env::var_os("ZO_E2E_BLESS").is_some() {
        fs::write(golden_path(name), &block).expect("write golden");
        return;
    }
    let expected = fs::read(golden_path(name)).expect("read golden");
    assert_eq!(
        escape(&expected),
        escape(&block),
        "{scenario}: the settled screen differs from the golden \
         (`ZO_E2E_BLESS=1 …` rewrites it once the change is intended)"
    );
}

/// 두 닻 사이의 마지막 화면. 시작 닻은 **마지막** 등장을 쓴다 — 그리는 도중의
/// 반쪽 프레임이 아니라 자리잡은 프레임을 잡기 위해서다.
fn settled_block(capture: &[u8], start: &[u8], end: &[u8]) -> Option<Vec<u8>> {
    let begin = capture
        .windows(start.len())
        .rposition(|window| window == start)?;
    let tail = &capture[begin..];
    let finish = tail.windows(end.len()).position(|window| window == end)? + end.len();
    Some(tail[..finish].to_vec())
}

fn assert_mock_http_contract(requests: &[CapturedRequest], expected: usize) {
    assert_eq!(requests.len(), expected);
    for request in requests {
        assert_eq!(request.method, "POST");
        assert_eq!(request.path, "/v1/messages");
        assert!(request.stream);
        assert_eq!(
            request.headers.get("x-api-key").map(String::as_str),
            Some("test-dummy-key")
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_boot_card_and_direct_heading_answer() {
    let layout = Layout::new();
    let service = ScriptedAnthropicService::text(DIRECT_ANSWER)
        .await
        .expect("start direct-answer script");
    let args = interactive_args();
    let mut run = pty(&layout, service.base_url(), &args);

    run.wait_for("directory:", TEST_TIMEOUT);
    run.send(b"Give me the deterministic heading and two bullets\r")
        .expect("send direct prompt");
    run.wait_for_history_row("second item", TEST_TIMEOUT);
    let capture = run.finish();

    assert_history_golden(&capture, "direct-answer", "boot card + direct heading answer", &layout);
    let requests = service.request_bodies().await;
    assert_eq!(requests.len(), 1, "unexpected requests: {requests:#?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_loop_three_runs_three_iterations_and_completes() {
    let layout = Layout::new();
    let service = MockAnthropicService::spawn()
        .await
        .expect("start loop mock service");
    let args = interactive_args();
    let mut run = pty(&layout, &service.base_url(), &args);
    run.wait_for("directory:", TEST_TIMEOUT);
    run.send(b"/loop 3 PARITY_SCENARIO:streaming_text echo hi\r")
        .expect("start fixed loop");
    run.wait_for("runs 3/50", TEST_TIMEOUT);
    let capture = run.finish();

    assert!(
        String::from_utf8_lossy(&capture).contains("completed"),
        "fixed loop never reported completion"
    );
    let requests = service.captured_requests().await;
    assert_mock_http_contract(&requests, 3);
}

/// A pipe closes stdin immediately after its one prompt, but a bounded
/// headless loop owns the process until the shared loop controller completes.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_plain_headless_loop_runs_twice_one_second_apart_then_exits() {
    const ANSWER: &str = "HEADLESS_LOOP_ITERATION_DONE";
    let layout = Layout::new();
    let service = ScriptedAnthropicService::text(ANSWER)
        .await
        .expect("start headless loop script");
    let started = Instant::now();
    let output = run_pipe(
        &layout.cwd,
        &layout.home,
        &layout.sessions,
        &layout.state,
        service.base_url(),
        &[
            "--plain",
            "--loop-every",
            "1s",
            "--loop-max",
            "2",
            "--permission-mode",
            "danger-full-access",
        ],
        b"repeat this prompt\n",
    )
    .expect("run bounded headless loop");

    assert!(output.status.success(), "headless loop exited with {:?}", output.status);
    assert!(
        started.elapsed() >= Duration::from_secs(1),
        "the second iteration did not wait for the interval"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(stdout.matches(ANSWER).count(), 2, "stdout was:\n{stdout}");
    assert_eq!(service.request_bodies().await.len(), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_json_headless_loop_emits_one_boundary_object_per_iteration() {
    let layout = Layout::new();
    let service = ScriptedAnthropicService::text("json loop answer")
        .await
        .expect("start headless JSON loop script");
    let output = run_pipe(
        &layout.cwd,
        &layout.home,
        &layout.sessions,
        &layout.state,
        service.base_url(),
        &[
            "--json",
            "--loop-every",
            "1s",
            "--loop-max",
            "2",
            "--permission-mode",
            "danger-full-access",
        ],
        b"repeat JSON prompt\n",
    )
    .expect("run JSON headless loop");
    assert!(output.status.success(), "JSON loop exited with {:?}", output.status);

    let loop_rows: Vec<serde_json::Value> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|line| serde_json::from_str(line).expect("every JSON line parses"))
        .filter(|row: &serde_json::Value| row["type"] == "loop")
        .collect();
    assert_eq!(
        loop_rows,
        vec![
            serde_json::json!({
                "type": "loop", "iteration": 1, "of": 2,
                "trigger": "every 1s", "outcome": "continue"
            }),
            serde_json::json!({
                "type": "loop", "iteration": 2, "of": 2,
                "trigger": "every 1s", "outcome": "done"
            }),
        ]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_headless_until_exits_two_when_its_run_limit_wins() {
    let layout = Layout::new();
    let service = ScriptedAnthropicService::text("condition still pending")
        .await
        .expect("start until-loop script");
    let output = run_pipe(
        &layout.cwd,
        &layout.home,
        &layout.sessions,
        &layout.state,
        service.base_url(),
        &[
            "--plain",
            "--loop-until",
            "false",
            "--loop-max",
            "1",
            "--permission-mode",
            "danger-full-access",
        ],
        b"check the condition\n",
    )
    .expect("run bounded until loop");

    assert_eq!(output.status.code(), Some(2), "stderr was {}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(service.request_bodies().await.len(), 1);
}

/// `auth.reload` is received while no turn owns the channel. The TUI paints
/// the shared wording on its status row once; it is neither transcript replay
/// nor a timer-driven notice that can repeat forever.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_auth_reload_shows_the_account_label_once_in_the_idle_tui() {
    const TOKEN: &str = "auth-switch-test-token";
    const LABEL: &str = "work-account";
    let layout = Layout::new();
    let service = ScriptedAnthropicService::text("unused")
        .await
        .expect("start idle provider");
    let addr_file = layout.root.path().join("events.addr");
    let account_home = layout.root.path().join("account");
    fs::create_dir_all(&account_home).expect("account directory");
    let addr_file_text = addr_file.to_string_lossy().into_owned();
    let account_home_text = account_home.to_string_lossy().into_owned();
    let args = interactive_args();
    let mut run = PtyRun::spawn_with_env(
        &layout.cwd,
        &layout.home,
        &layout.sessions,
        &layout.state,
        service.base_url(),
        &args,
        &[
            ("ZO_EVENTS_ADDR_FILE", addr_file_text.as_str()),
            ("ZO_SERVE_TOKEN", TOKEN),
        ],
    )
    .expect("spawn idle TUI");
    run.wait_for("directory:", TEST_TIMEOUT);

    let mut client = connect_to_addr_file(&addr_file, TOKEN).await;
    let response = client
        .call(
            method::AUTH_RELOAD,
            serde_json::json!({
                "provider": "anthropic",
                "label": LABEL,
                "claude_config_dir": account_home_text,
            }),
        )
        .await
        .expect("auth.reload");
    assert_eq!(response["account"]["label"], LABEL);

    let notice = format!("계정 전환 · {LABEL}");
    run.wait_until(0, TEST_TIMEOUT, |bytes| {
        strip_ansi(&String::from_utf8_lossy(bytes)).contains(&notice)
    });
    let capture = run.finish();
    let plain = strip_ansi(&String::from_utf8_lossy(&capture));
    assert_eq!(plain.matches(&notice).count(), 1, "pty was:\n{plain}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_auth_reload_is_one_append_only_line_in_the_plain_frontend() {
    const TOKEN: &str = "plain-auth-switch-token";
    const LABEL: &str = "personal-account";
    let layout = Layout::new();
    let service = ScriptedAnthropicService::text("unused")
        .await
        .expect("start idle provider");
    let addr_file = layout.root.path().join("plain-events.addr");
    let account_home = layout.root.path().join("plain-account");
    fs::create_dir_all(&account_home).expect("account directory");
    let addr_file_text = addr_file.to_string_lossy().into_owned();
    let account_home_text = account_home.to_string_lossy().into_owned();
    let mut run = PtyRun::spawn_with_env(
        &layout.cwd,
        &layout.home,
        &layout.sessions,
        &layout.state,
        service.base_url(),
        &[
            "--plain",
            "--events-bind",
            "127.0.0.1:0",
            "--permission-mode",
            "danger-full-access",
        ],
        &[
            ("ZO_EVENTS_ADDR_FILE", addr_file_text.as_str()),
            ("ZO_SERVE_TOKEN", TOKEN),
        ],
    )
    .expect("spawn idle plain frontend");

    let mut client = connect_to_addr_file(&addr_file, TOKEN).await;
    client
        .call(
            method::AUTH_RELOAD,
            serde_json::json!({
                "provider": "anthropic",
                "label": LABEL,
                "claude_config_dir": account_home_text,
            }),
        )
        .await
        .expect("auth.reload");
    let notice = format!("계정 전환 · {LABEL}");
    run.wait_for(&notice, TEST_TIMEOUT);
    let capture = run.finish();
    let plain = strip_ansi(&String::from_utf8_lossy(&capture));
    assert_eq!(plain.matches(&notice).count(), 1, "pty was:\n{plain}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_interval_loop_wakes_twice_then_stops_from_human_input() {
    let layout = Layout::new();
    let service = MockAnthropicService::spawn()
        .await
        .expect("start interval-loop mock service");
    let args = interactive_args();
    let mut run = pty(&layout, &service.base_url(), &args);
    run.wait_for("directory:", TEST_TIMEOUT);
    run.send(b"/loop every 1s PARITY_SCENARIO:streaming_text poll CI\r")
        .expect("start interval loop");
    let after_first = run.wait_for("Mock streaming says hello", TEST_TIMEOUT);
    run.wait_for_after("Mock streaming says hello", after_first, TEST_TIMEOUT);
    run.send(b"/loop stop\r").expect("stop interval loop");
    run.wait_for("loop-1 stopped", TEST_TIMEOUT);
    let _capture = run.finish();

    let requests = service.captured_requests().await;
    assert!(requests.len() >= 2, "interval did not wake twice: {requests:#?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_goal_false_gate_repairs_once_then_stalls_without_rerunning() {
    let layout = Layout::new();
    let service = MockAnthropicService::spawn()
        .await
        .expect("start goal mock service");
    let args = interactive_args();
    let mut run = pty(&layout, &service.base_url(), &args);
    run.wait_for("directory:", TEST_TIMEOUT);
    run.send(
        b"/goal start PARITY_SCENARIO:bash_stdout_roundtrip repair me --check false --no-plan\r",
    )
    .expect("start goal");
    run.wait_for("workspace unchanged after repair", TEST_TIMEOUT);
    let capture = run.finish();

    assert!(
        String::from_utf8_lossy(&capture).contains("(paused)"),
        "failed repair did not pause the goal"
    );
    let requests = service.captured_requests().await;
    assert!(requests.len() >= 3, "action/gate/repair did not all run: {requests:#?}");
}

/// A goal is judged by its gate command's exit code, never by the Bash tool
/// answering: an obedient model runs the exact gate once — `false` exits 1,
/// and the tool answers that without an error flag — so the controller
/// repairs and stalls instead of completing; `true` completes the same goal
/// with nobody typing after the start.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_goal_gate_is_judged_by_the_commands_exit_code() {
    for (check, ends) in [
        ("false", "workspace unchanged after repair"),
        ("true", "(completed)"),
    ] {
        let layout = Layout::new();
        let service = MockAnthropicService::spawn()
            .await
            .expect("start goal gate mock service");
        let args = interactive_args();
        let mut run = pty(&layout, &service.base_url(), &args);
        run.wait_for("directory:", TEST_TIMEOUT);
        run.send(
            format!("/goal start PARITY_SCENARIO:goal_gate_exact ship it --check {check} --no-plan\r")
                .as_bytes(),
        )
        .expect("start goal");
        run.wait_for(ends, TEST_TIMEOUT);
        let capture = String::from_utf8_lossy(&run.finish()).to_string();
        let requests = service.captured_requests().await;
        let gate_runs = requests
            .iter()
            .filter(|request| request.raw_body.contains("toolu_goal_gate"))
            .count();
        assert!(gate_runs >= 1, "the model never ran the gate for --check {check}: {requests:#?}");
        if check == "false" {
            assert!(
                !capture.contains("(completed)"),
                "a gate that exited 1 completed the goal"
            );
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_goal_resumes_after_provider_retry_after_and_reaches_its_condition() {
    let layout = Layout::new();
    let service = MockAnthropicService::spawn()
        .await
        .expect("start rate-limit mock service");
    let args = interactive_args();
    let mut run = pty(&layout, &service.base_url(), &args);
    run.wait_for("directory:", TEST_TIMEOUT);
    run.send(
        b"/goal start PARITY_SCENARIO:goal_rate_limit_resume deploy --until true --no-plan\r",
    )
    .expect("start goal");
    run.wait_for("(completed)", TEST_TIMEOUT);
    let capture = run.finish();

    assert!(
        String::from_utf8_lossy(&capture).contains("provider reset elapsed"),
        "goal did not resume its action after the provider reset"
    );
    let requests = service.captured_requests().await;
    assert!(
        requests.len() >= 8,
        "rate-limit retries and resumed condition were not all observed: {requests:#?}"
    );
}

/// A teammate pane is one turn under somebody else's brief, and then a file.
///
/// The parent's whole protocol is these two documents: it writes `brief.json`
/// into a directory and waits for `result.json` to appear beside it. So this
/// asks the only questions that matter from the outside — the prompt in the
/// brief really became this run's first turn, the answer really came back in
/// the file, and the process really left. A child that answered on screen and
/// wrote nothing would leave its parent waiting out an hour-long budget for
/// work that was already done.
/// The first segment of the idle teammate's line, from the strings table it
/// is drawn from — the words, not a copy of them — short enough to land whole
/// on a sixty-column pane.
fn teammate_idle_line_head() -> &'static str {
    zo_ide::tui::strings::TEAMMATE_IDLE
        .split(" · ")
        .next()
        .expect("the idle line has a first segment")
}

/// A teammate's brief that reads as broad — it even asks for parallel lanes —
/// is NOT pre-analysed by the host: the child's turn is already a delegation.
/// Before this the child fanned its own brief out to a decomposition helper
/// (a second provider request, and in the window a second pane teammate whose
/// brief read as broad, and so on — thirty-five panes in ninety seconds,
/// 2026-09-07). Exactly one provider request, and the answer written back.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_a_teammate_never_pre_analyses_its_own_brief() {
    use runtime::subagent_panes::{Brief, Exit, PROTOCOL_VERSION};

    let layout = Layout::new();
    let service = ScriptedAnthropicService::text("one lane, one answer\n")
        .await
        .expect("start teammate script");
    let directory = layout.root.path().join("agent-broad-1");
    fs::create_dir_all(&directory).expect("teammate directory");
    Brief {
        version: PROTOCOL_VERSION,
        agent_id: "agent-broad-1".to_string(),
        prompt: "Audit ui/, crates/ and zo-ide/ in parallel: split the review into separate lanes \
                 per directory, run them as a fan-out, and report every finding."
            .to_string(),
        description: "broad review".to_string(),
        subagent_type: Some("general-purpose".to_string()),
        name: Some("broad".to_string()),
        permission_mode: Some("read-only".to_string()),
        parent_session: Some("session-parent-e2e".to_string()),
        tool_call_id: Some("toolu_broad".to_string()),
        ..Brief::default()
    }
    .write(&directory)
    .expect("write brief");

    let brief_arg = directory.to_string_lossy().into_owned();
    let mut run = pty(&layout, service.base_url(), &["--teammate", &brief_arg]);
    run.wait_for("session-parent-e2e", TEST_TIMEOUT);
    let result = wait_for_teammate_result(&directory, Duration::from_secs(20));
    assert_eq!(result.exit, Exit::Ok, "{result:?}");
    assert!(result.final_message.contains("one answer"), "{result:?}");
    // The whole life of this child cost the provider ONE request: no
    // decomposition helper, no fan-out members, before or after the answer.
    tokio::time::sleep(Duration::from_millis(400)).await;
    let requests = service.request_bodies().await;
    assert_eq!(
        requests.len(),
        1,
        "a teammate pre-analysed its own brief: {} provider request(s)",
        requests.len()
    );
    let (mut client, _) = connect_to_teammate(&directory).await;
    let _ = client
        .call(runtime::subagent_panes::channel_method::TEAMMATE_CLOSE, serde_json::json!({}))
        .await;
    let _ = run.finish();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_a_teammate_pane_runs_its_brief_once_and_writes_the_answer_back() {
    use runtime::subagent_panes::{Brief, Exit, PROTOCOL_VERSION};

    let layout = Layout::new();
    let service = ScriptedAnthropicService::text("the map has three edges\n")
        .await
        .expect("start teammate script");

    let directory = layout.root.path().join("agent-e2e-1");
    fs::create_dir_all(&directory).expect("teammate directory");
    Brief {
        version: PROTOCOL_VERSION,
        agent_id: "agent-e2e-1".to_string(),
        prompt: "Count the edges of the map.".to_string(),
        description: "recon".to_string(),
        subagent_type: Some("Explore".to_string()),
        name: Some("recon-graph".to_string()),
        model: None,
        effort: None,
        permission_mode: Some("read-only".to_string()),
        cwd: None,
        parent_session: Some("session-parent-e2e".to_string()),
        tool_call_id: Some("toolu_e2e".to_string()),
        wave_index: 0,
        ..Brief::default()
    }
    .write(&directory)
    .expect("write brief");

    let brief_arg = directory.to_string_lossy().into_owned();
    let mut run = pty(&layout, service.base_url(), &["--teammate", &brief_arg]);

    // Who sent this, before anything else on screen.
    run.wait_for("session-parent-e2e", TEST_TIMEOUT);
    // The brief's prompt is this run's first turn — nobody typed it.
    run.wait_for_history_row("Count the edges of the map.", TEST_TIMEOUT);
    run.wait_for_history_row("three edges", TEST_TIMEOUT);
    // Since t-2513 the pane STAYS after the answer, waiting for its parent;
    // a v1 brief names no parent channel, so the parent's word is the only
    // thing that ends it here. Closed over the channel, it says it handed
    // the answer up before the process leaves.
    let result = wait_for_teammate_result(&directory, Duration::from_secs(20));
    run.wait_for(teammate_idle_line_head(), TEST_TIMEOUT);
    let (mut client, _) = connect_to_teammate(&directory).await;
    let closed = client
        .call(
            runtime::subagent_panes::channel_method::TEAMMATE_CLOSE,
            serde_json::json!({}),
        )
        .await
        .expect("teammate.close");
    assert_eq!(closed["closing"], true);
    run.wait_for("부모에게 돌려줌", TEST_TIMEOUT);
    let closing = wait_for_closing(&directory, Duration::from_secs(10));
    assert_eq!(closing.reason, Some(runtime::subagent_panes::CloseReason::ClosedByParent));
    assert_eq!(result.agent_id, "agent-e2e-1");
    assert_eq!(result.exit, Exit::Ok);
    assert!(
        result.final_message.contains("three edges"),
        "the answer did not reach the parent: {result:?}"
    );
    assert!(result.error.is_none(), "{result:?}");
    assert!(
        result
            .transcript
            .as_deref()
            .is_some_and(std::path::Path::exists),
        "the result points at no transcript: {result:?}"
    );

    // One turn, and out — never a second request, and never a prompt waiting
    // for a person who is not there.
    let requests = service.request_bodies().await;
    assert_eq!(requests.len(), 1, "a teammate ran more than one turn");
    // A teammate never starts teammates: the spawn family is off in the very
    // harness the model is shown.
    for spawn in ["\"Agent\"", "\"SpawnMultiAgent\"", "\"Workflow\"", "\"Task\""] {
        assert!(
            !requests[0].contains(spawn),
            "a teammate was offered {spawn} and could cut a pane out of a pane"
        );
    }
    let _ = run.finish();
}

/// Wait for a teammate's `result.json` the way its parent does.
fn wait_for_teammate_result(
    directory: &std::path::Path,
    timeout: Duration,
) -> runtime::subagent_panes::TeammateResult {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if let Some(result) = runtime::subagent_panes::TeammateResult::read(directory) {
            return result;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the teammate wrote no result in {timeout:?}"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// A fresh pane can receive window-owned control noise in the first rendered
/// frame. Those bytes must not turn an empty composer into a user-authored
/// exit; ten seconds later the same pane is alive and accepts ordinary text.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_fresh_pane_ignores_quit_noise_then_accepts_composer_input() {
    let layout = Layout::new();
    let service = ScriptedAnthropicService::text("unused")
        .await
        .expect("start unused provider");
    let mut run = pty(&layout, service.base_url(), &interactive_args());
    run.wait_for("directory:", TEST_TIMEOUT);

    run.send(&[0x03, 0x03, 0x04])
        .expect("send launch-road quit noise");
    std::thread::sleep(Duration::from_secs(10));
    assert!(run.rss_kib().is_some(), "fresh zo exited during the ten-second survival window");

    run.send(b"launch-road composer live")
        .expect("type after the survival window");
    run.wait_for("launch-road composer live", TEST_TIMEOUT);
    let _ = run.finish();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_tool_turn_runs_bash_and_read_and_compacts_to_two_commands() {
    let layout = Layout::new();
    layout.fixture("fixture content for the deterministic read\n");
    let service = ScriptedAnthropicService::bash_read(
        "### Tool answer\n\n- bash command completed\n- fixture read\n",
    )
    .await
    .expect("start bash/read script");
    let args = interactive_args();
    let mut run = pty(&layout, service.base_url(), &args);

    run.wait_for("directory:", TEST_TIMEOUT);
    run.send(b"Run Bash once and Read fixture.txt once, then summarize\r")
        .expect("send tool prompt");
    run.wait_for("Ran 2 commands", TEST_TIMEOUT);
    run.wait_for_history_row("fixture read", TEST_TIMEOUT);
    let capture = run.finish();

    assert_history_golden(&capture, "tool-turn", "tool turn", &layout);
    let requests = service.request_bodies().await;
    assert_eq!(requests.len(), 2, "one request for tools and one final turn");
    assert!(
        requests[1].contains("tool_result"),
        "second request must carry both tool results"
    );
    assert!(requests[1].contains("fixture.txt"));
}

/// A `ZeroCode` pane is a transport location, not a second TUI theme. The same
/// deterministic tool turn must commit exactly the same styled history bytes
/// as a bare terminal unless the separate fold-marker experiment is enabled.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_pane_key_preserves_bare_terminal_history_bytes() {
    async fn capture(in_pane: bool) -> (Layout, Vec<u8>) {
        let layout = Layout::new();
        layout.fixture("fixture content for the deterministic read\n");
        let service = ScriptedAnthropicService::bash_read(
            "### Tool answer\n\n- bash command completed\n- fixture read\n",
        )
        .await
        .expect("start bash/read script");
        let args = interactive_args();
        let mut run = if in_pane {
            PtyRun::spawn_with_env(
                &layout.cwd,
                &layout.home,
                &layout.sessions,
                &layout.state,
                service.base_url(),
                &args,
                &[("ZEROCODE_PANE_KEY", "e2e-pane")],
            )
        } else {
            PtyRun::spawn(
                &layout.cwd,
                &layout.home,
                &layout.sessions,
                &layout.state,
                service.base_url(),
                &args,
            )
        }
        .expect("spawn zo in PTY");
        run.wait_for("directory:", TEST_TIMEOUT);
        run.send(b"Run Bash once and Read fixture.txt once, then summarize\r")
            .expect("send tool prompt");
        // The composer placeholder can repaint while the final answer is still
        // draining. Compare only after the last expected history row commits.
        run.wait_for_history_row("fixture read", TEST_TIMEOUT);
        wait_until_quiet(&run, Duration::from_millis(500), TEST_TIMEOUT);
        let capture = run.snapshot_output();
        let _ = run.finish();
        (layout, capture)
    }

    let (bare_layout, bare) = capture(false).await;
    let (pane_layout, pane) = capture(true).await;
    let masked = |capture: &[u8], layout: &Layout| {
        history_rows_raw(capture)
            .iter()
            .map(|row| mask_history_row(row, layout))
            .collect::<Vec<_>>()
    };
    assert_eq!(masked(&pane, &pane_layout), masked(&bare, &bare_layout));
}

/// The footer shortens a workspace below HOME to `~/…`, but the resume
/// filter must keep using the real filesystem path. Otherwise the default
/// `[Cwd]` view hides the session that was just written in that directory.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_resume_picker_finds_a_session_below_home() {
    let mut layout = Layout::new();
    // macOS spells the tempfile root through `/var` while `current_dir()`
    // canonicalizes it to `/private/var`. Use one spelling so this scenario
    // exercises HOME shortening rather than that separate symlink case.
    layout.home = layout.home.canonicalize().expect("canonical test home");
    layout.cwd = layout.home.join("workspace");
    fs::create_dir_all(&layout.cwd).expect("home-nested workspace");
    let first_service = ScriptedAnthropicService::text("### Saved\n\n- ready to resume\n")
        .await
        .expect("start first script");
    let args = interactive_args();
    let mut first = pty(&layout, first_service.base_url(), &args);
    first.wait_for("directory:", TEST_TIMEOUT);
    first
        .send(b"remember the home nested resume fixture\r")
        .expect("send first prompt");
    first.wait_for("ready to resume", TEST_TIMEOUT);
    let _ = first.finish();

    let second_service = ScriptedAnthropicService::text("unused\n")
        .await
        .expect("start second script");
    let mut second = pty(&layout, second_service.base_url(), &args);
    second.wait_for("directory:", TEST_TIMEOUT);
    second.send(b"/resume\r").expect("open resume picker");
    second.wait_for("Resume a previous session", TEST_TIMEOUT);
    let capture = second.snapshot_output();
    let _ = second.finish();
    let screen = strip_ansi(&String::from_utf8_lossy(&capture));

    assert!(
        screen.contains("remember the home nested resume fixture"),
        "the Cwd filter hid the session below HOME:\n{screen}"
    );
    assert!(!screen.contains("No sessions yet"), "{screen}");
}

/// Codex `/new [name]` starts a named resumable chat without touching the
/// terminal, while `/clear [name]` takes the same fresh-session path after one
/// atomic visible-screen + scrollback purge.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_new_and_clear_start_named_resumable_chats() {
    const CLEAR_TERMINAL: &[u8] = b"\x1b[r\x1b[0m\x1b[H\x1b[2J\x1b[3J\x1b[H";

    let layout = Layout::new();
    let service = ScriptedAnthropicService::text("unused\n")
        .await
        .expect("start script");
    let args = interactive_args();
    let mut run = pty(&layout, service.base_url(), &args);
    run.wait_for("directory:", TEST_TIMEOUT);

    let new_at = run.output_len();
    run.send(b"/new planning pass\r").expect("start new chat");
    run.wait_for_after("directory:", new_at, TEST_TIMEOUT);

    let clear_at = run.output_len();
    run.send(b"/clear clean slate\r")
        .expect("clear and start new chat");
    run.wait_for_after("directory:", clear_at, TEST_TIMEOUT);
    let capture = run.finish();

    assert!(
        !capture[new_at..clear_at]
            .windows(CLEAR_TERMINAL.len())
            .any(|window| window == CLEAR_TERMINAL),
        "/new must preserve the terminal"
    );
    assert!(
        capture[clear_at..]
            .windows(CLEAR_TERMINAL.len())
            .any(|window| window == CLEAR_TERMINAL),
        "/clear did not emit Codex's terminal purge sequence"
    );

    let session_dir = layout.sessions.join("sessions");
    let sessions: Vec<runtime::Session> = fs::read_dir(&session_dir)
        .expect("read managed sessions")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| runtime::session_control::is_managed_session_file(path))
        .filter_map(|path| runtime::Session::load_from_path(path).ok())
        .collect();
    assert_eq!(sessions.len(), 3, "startup, /new, and /clear need distinct chats");

    let ids: std::collections::BTreeSet<&str> = sessions
        .iter()
        .map(|session| session.session_id.as_str())
        .collect();
    assert_eq!(ids.len(), 3, "fresh chats must have distinct ids");
    let names: std::collections::BTreeSet<&str> = sessions
        .iter()
        .filter_map(|session| session.name.as_deref())
        .collect();
    assert_eq!(
        names,
        std::collections::BTreeSet::from(["clean slate", "planning pass"])
    );
}

/// Both commands replace the active chat, so Codex rejects them until the
/// current turn finishes rather than steering the command into the model.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_new_and_clear_are_disabled_during_an_active_turn() {
    const CLEAR_TERMINAL: &[u8] = b"\x1b[r\x1b[0m\x1b[H\x1b[2J\x1b[3J\x1b[H";

    let layout = Layout::new();
    let service = ScriptedAnthropicService::slow_tool("ACTIVE_TURN_DONE\n")
        .await
        .expect("start slow-tool script");
    let args = interactive_args();
    let mut run = pty(&layout, service.base_url(), &args);

    run.wait_for("directory:", TEST_TIMEOUT);
    run.send(b"Run the slow command\r").expect("send prompt");
    run.wait_for("sleep 3", TEST_TIMEOUT);
    let session_dir = layout.sessions.join("sessions");
    let managed_sessions = || {
        fs::read_dir(&session_dir)
            .expect("read managed sessions")
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| runtime::session_control::is_managed_session_file(path))
            .collect::<std::collections::BTreeSet<_>>()
    };
    let sessions_before_commands = managed_sessions();
    let commands_at = run.output_len();
    run.send(b"/new must not exist\r")
        .expect("submit blocked new command");
    run.wait_for(
        "'/new' is disabled while a task is in progress.",
        TEST_TIMEOUT,
    );
    run.send(b"/clear must not exist\r")
        .expect("submit blocked clear command");
    run.wait_for(
        "'/clear' is disabled while a task is in progress.",
        TEST_TIMEOUT,
    );
    run.wait_for("ACTIVE_TURN_DONE", TEST_TIMEOUT);
    let sessions_after_commands = managed_sessions();
    let capture = run.finish();

    assert!(
        !capture[commands_at..]
            .windows(CLEAR_TERMINAL.len())
            .any(|window| window == CLEAR_TERMINAL),
        "blocked /clear must not touch the terminal"
    );
    assert_eq!(
        sessions_after_commands,
        sessions_before_commands,
        "blocked commands must not create chats"
    );
    let requests = service.request_bodies().await;
    assert_eq!(requests.len(), 2, "tool request plus its final response");
    assert!(!requests.iter().any(|request| request.contains("must not exist")));
}

// ---------------------------------------------------------------------------
// r28 — a hook's `additionalContext` is prompt material, not a screen cell.
//
// Observed in an IDE zo worker pane: two lines of ZeroCode's orchestration
// contract sat between cells, verbatim. That paragraph is what a hook hands
// the MODEL through `hookSpecificOutput.additionalContext`; codex has no
// hooks, and Claude Code never prints one either (a block or a failure is all
// a person sees). The unit pins in `runtime/src/hooks.rs` fix the parse; these
// pin the whole road — a real settings.json hook, a real turn, and the two
// halves of the claim measured at once: NOT on the screen, and still in the
// request body.
// ---------------------------------------------------------------------------

/// What a `UserPromptSubmit` hook contributes. Distinctive on purpose: the
/// assertions are substring scans over an entire capture.
const PROMPT_HOOK_CONTEXT: &str = "PROMPT_HOOK_CONTEXT_BELONGS_TO_THE_MODEL_ONLY";
/// What a `PreToolUse` hook contributes on the same turn.
const TOOL_HOOK_CONTEXT: &str = "TOOL_HOOK_CONTEXT_BELONGS_TO_THE_MODEL_ONLY";
/// What a *blocking* `PreToolUse` hook contributes — the shape the leak was
/// loudest in, because there the refusal a person reads is built from the very
/// vector the context used to share.
const BLOCKING_HOOK_CONTEXT: &str = "BLOCKING_HOOK_CONTEXT_BELONGS_TO_THE_MODEL_ONLY";

/// One JSON object carrying only `additionalContext`, as a hook prints it.
fn context_only_hook_output(event: &str, context: &str) -> String {
    serde_json::json!({
        "hookSpecificOutput": {"hookEventName": event, "additionalContext": context}
    })
    .to_string()
}

/// Every history row of a capture as one lossy string, for a substring scan.
///
/// Committed rows rather than the raw capture: the viewport is redrawn
/// constantly and a match inside a half-painted frame says nothing about what
/// stayed on the screen. This is the same window `assert_history_golden`
/// compares, so a leak either shows in both or in neither.
fn history_text(capture: &[u8]) -> String {
    history_rows_raw(capture)
        .iter()
        .map(|row| String::from_utf8_lossy(row).into_owned())
        .collect::<Vec<_>>()
        .join("\n")
}

fn assert_no_hook_material_on_screen(capture: &[u8], scenario: &str, contexts: &[&str]) {
    let rows = history_text(capture);
    for context in contexts {
        assert!(
            !rows.contains(context),
            "{scenario}: a hook's prompt material reached the screen — {context} is in:\n{rows}"
        );
    }
    // The wrappers too: seeing `Hook feedback:` or a bare `hookSpecificOutput`
    // means the context leaked wearing a different coat (the raw-JSON echo
    // fallback is exactly that second coat).
    for wrapper in ["Hook feedback", "Hook context", "hookSpecificOutput"] {
        assert!(
            !rows.contains(wrapper),
            "{scenario}: {wrapper} was rendered — hook material must not be a cell:\n{rows}"
        );
    }
}

/// A hook's context reaches the model and nothing else.
///
/// Both events at once on one ordinary tool turn, compared byte for byte with
/// the golden of that same turn without hooks. Measured: restoring the
/// `parsed.messages.push(additional_context)` line this replaced leaves THIS
/// screen unchanged — the TUI shows a tool cell's first output line and folds
/// the rest, so a non-blocking hook's paragraph sat one line under the visible
/// edge. The two shapes that did reach a person are pinned below (a blocking
/// hook's banner) and further down (the exec sink, which prints the body
/// whole). What this case pins that neither of those can is the other half of
/// the claim: the context must still ARRIVE — a fix that merely dropped it
/// would pass every screen assertion in this file.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_hook_context_reaches_the_model_and_never_the_screen() {
    let layout = Layout::new();
    layout.fixture("fixture content for the deterministic read\n");
    let prompt_hook = layout.hook_script(
        "prompt-context.sh",
        &context_only_hook_output("UserPromptSubmit", PROMPT_HOOK_CONTEXT),
        0,
    );
    let tool_hook = layout.hook_script(
        "tool-context.sh",
        &context_only_hook_output("PreToolUse", TOOL_HOOK_CONTEXT),
        0,
    );
    layout.with_hooks(&serde_json::json!({
        "UserPromptSubmit": [prompt_hook],
        "PreToolUse": [tool_hook],
    }));
    let service = ScriptedAnthropicService::bash_read(
        "### Tool answer\n\n- bash command completed\n- fixture read\n",
    )
    .await
    .expect("start bash/read script");
    let args = interactive_args();
    let mut run = pty(&layout, service.base_url(), &args);

    run.wait_for("directory:", TEST_TIMEOUT);
    run.send(b"Run Bash once and Read fixture.txt once, then summarize\r")
        .expect("send tool prompt");
    run.wait_for_history_row("fixture read", TEST_TIMEOUT);
    let capture = run.finish();

    assert_no_hook_material_on_screen(
        &capture,
        "hook context on a tool turn",
        &[PROMPT_HOOK_CONTEXT, TOOL_HOOK_CONTEXT],
    );
    // The whole screen, byte for byte, against the golden of the SAME turn
    // without hooks. That is the contract stated positively — two hooks
    // contributing context change nothing a person sees — and it is stricter
    // than any substring scan: a stray row, a lost colour, a reordered cell all
    // fail here. It reuses `tool-turn` rather than adding a golden, which is
    // the point: the two scenarios must draw the same bytes.
    assert_history_golden(&capture, "tool-turn", "tool turn under context hooks", &layout);

    let requests = service.request_bodies().await;
    assert_eq!(requests.len(), 2, "one request for tools and one final turn");
    assert!(
        requests[0].contains(PROMPT_HOOK_CONTEXT),
        "the prompt hook's context must reach the model on the very turn it \
         was contributed for: {}",
        requests[0]
    );
    assert!(
        requests[1].contains(TOOL_HOOK_CONTEXT),
        "the tool hook's context must ride the tool result: {}",
        requests[1]
    );
}

/// A blocking hook's refusal is the runtime's words, not the model's material.
///
/// `decision: block` with no `reason` and only `additionalContext` is the shape
/// a context-contributing gate has when it also says no. The banner a person
/// reads is built from `messages` — which the context was in — so the whole
/// paragraph was printed as `denied '<tool>': …`, and the denied cell repeated
/// it under `└`. That is the leak as it was seen: a wrapped warning row of
/// prompt material sitting between two cells.
///
/// It bites. With the old push restored this fails twice over (measured):
/// `⚠ denied 'bash': BLOCKING_HOOK_CONTEXT… · /permissions` and
/// `└ BLOCKING_HOOK_CONTEXT…`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_a_blocking_hooks_refusal_is_not_its_prompt_material() {
    let layout = Layout::new();
    layout.fixture("fixture content for the deterministic read\n");
    let blocking_hook = layout.hook_script(
        "blocking-context.sh",
        &serde_json::json!({
            "decision": "block",
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "additionalContext": BLOCKING_HOOK_CONTEXT,
            },
        })
        .to_string(),
        0,
    );
    layout.with_hooks(&serde_json::json!({
        "PreToolUse": [{"matcher": "bash", "hooks": [{"type": "command", "command": blocking_hook}]}],
    }));
    let service = ScriptedAnthropicService::bash_read(
        "### Tool answer\n\n- bash command completed\n- fixture read\n",
    )
    .await
    .expect("start bash/read script");
    let args = interactive_args();
    let mut run = pty(&layout, service.base_url(), &args);

    run.wait_for("directory:", TEST_TIMEOUT);
    run.send(b"Run Bash once and Read fixture.txt once, then summarize\r")
        .expect("send tool prompt");
    run.wait_for("fixture read", TEST_TIMEOUT);
    let capture = run.finish();

    let rows = history_text(&capture);
    assert!(
        rows.contains("denied 'bash'"),
        "the block itself must still be visible — a hook that says no is news:\n{rows}"
    );
    assert_no_hook_material_on_screen(
        &capture,
        "a blocking hook that only carries context",
        &[BLOCKING_HOOK_CONTEXT],
    );

    let requests = service.request_bodies().await;
    assert_eq!(requests.len(), 2, "one request for tools and one final turn");
    assert!(
        requests[1].contains(BLOCKING_HOOK_CONTEXT),
        "a blocked call still hands the model what the gate wanted it to know: {}",
        requests[1]
    );
}

/// The pipe (`exec`) side obeys the same rule.
///
/// Two renderers, one contract: `--plain` builds its cells in `sinks/text.rs`
/// from the same `RenderBlock`s the TUI draws — but it prints a tool body
/// WHOLE where the TUI folds it, so this is the path a non-blocking hook's
/// paragraph actually reached a reader through, and a program piping zo would
/// have had hook material in its stdout.
///
/// No fixture on purpose: `read_file` fails, which routes the batch through the
/// error arm where the rendered body IS the model-facing string. With the old
/// push restored the stdout carries `Hook feedback:` and the whole paragraph
/// under the failed read (measured).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_pipe_mode_keeps_hook_context_out_of_its_stdout() {
    let layout = Layout::new();
    let prompt_hook = layout.hook_script(
        "prompt-context.sh",
        &context_only_hook_output("UserPromptSubmit", PROMPT_HOOK_CONTEXT),
        0,
    );
    let tool_hook = layout.hook_script(
        "tool-context.sh",
        &context_only_hook_output("PreToolUse", TOOL_HOOK_CONTEXT),
        0,
    );
    layout.with_hooks(&serde_json::json!({
        "UserPromptSubmit": [prompt_hook],
        "PreToolUse": [tool_hook],
    }));
    let service = ScriptedAnthropicService::bash_read(
        "### Tool answer\n\n- bash command completed\n- fixture read\n",
    )
    .await
    .expect("start bash/read script");
    let output = run_pipe(
        &layout.cwd,
        &layout.home,
        &layout.sessions,
        &layout.state,
        service.base_url(),
        &["--plain", "--permission-mode", "danger-full-access"],
        b"Run Bash once and Read fixture.txt once, then summarize\n/exit\n",
    )
    .expect("run zo in pipe mode");
    assert!(output.status.success(), "pipe exited with {:?}", output.status);

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    for context in [PROMPT_HOOK_CONTEXT, TOOL_HOOK_CONTEXT] {
        assert!(
            !stdout.contains(context),
            "the exec path printed a hook's prompt material — {context} is in:\n{stdout}"
        );
    }
    for wrapper in ["Hook feedback", "Hook context", "hookSpecificOutput"] {
        assert!(
            !stdout.contains(wrapper),
            "the exec path printed {wrapper}:\n{stdout}"
        );
    }

    let requests = service.request_bodies().await;
    assert_eq!(requests.len(), 2, "one request for tools and one final turn");
    assert!(requests[0].contains(PROMPT_HOOK_CONTEXT), "{}", requests[0]);
    assert!(requests[1].contains(TOOL_HOOK_CONTEXT), "{}", requests[1]);
}

/// `--json` is a contract another program drives a long run with.
///
/// `--plain` was already there, and it is the reason automation could not use
/// zo: it is prose made pleasant for a person — folded tool output, summaries,
/// colour — and driving it means parsing that back. This asserts the parts that
/// make the JSON usable rather than merely present: every line parses, the
/// vocabulary covers the turn, the final answer arrives whole (not only as
/// deltas), and no banner or stray prose sits in the stream.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_json_emits_one_parseable_object_per_event() {
    const PROMPT: &str = "Run Bash once and Read fixture.txt once, then summarize";
    let layout = Layout::new();
    layout.fixture("fixture content for the deterministic read\n");
    let service = ScriptedAnthropicService::bash_read(
        "### Tool answer\n\n- bash command completed\n- fixture read\n",
    )
    .await
    .expect("start bash/read script");
    let mut args = interactive_args().to_vec();
    args.push("--json");
    let mut run = pty(&layout, service.base_url(), &args);
    run.send(format!("{PROMPT}\r").as_bytes())
        .expect("send tool prompt");
    run.wait_for("\"type\":\"assistant\"", TEST_TIMEOUT);
    let capture = run.finish();

    let text = String::from_utf8_lossy(&capture);
    let mut kinds: Vec<String> = Vec::new();
    let mut assistant = String::new();
    let mut lines = 0usize;
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        if line.trim().is_empty() {
            continue;
        }
        // The pty itself echoes what was typed into it — the prompt, and the
        // `/exit` this harness sends to close the session. That is the
        // terminal, not zo (a program driving `--json` pipes stdin and sees
        // none of it), but this harness only has a pty, so the echoed lines are
        // skipped by exact match rather than by loosening the parse.
        if line == PROMPT || line == "/exit" {
            continue;
        }
        lines += 1;
        let value: serde_json::Value = serde_json::from_str(line)
            .unwrap_or_else(|error| panic!("every line must be one JSON object: {error} in {line:?}"));
        let kind = value["type"].as_str().expect("every object names its type");
        if kind == "assistant" {
            assistant = value["text"].as_str().unwrap_or_default().to_string();
        }
        kinds.push(kind.to_string());
    }

    assert!(lines >= 4, "a tool turn is more than a couple of events: {kinds:?}");
    for expected in ["user", "tool_call", "tool_result", "assistant"] {
        assert!(
            kinds.iter().any(|kind| kind == expected),
            "the vocabulary must cover a tool turn — missing {expected} in {kinds:?}"
        );
    }
    assert!(
        assistant.contains("bash command completed") && assistant.contains("fixture read"),
        "the final answer arrives whole, not only as deltas: {assistant:?}"
    );
    assert!(
        !text.contains("Welcome") && !text.contains("directory:"),
        "no banner may sit in the stream — the first line has to parse"
    );
}

/// `--last-message` hands the caller one answer without parsing anything.
///
/// Driven through the **interactive** front-end on purpose: the same flag has
/// to mean the same thing on both paths, or it is not a contract but an
/// accident of which renderer happened to run.
///
/// The point of the file is that it appears **after** the session is settled:
/// a caller watching for it starts its next step on that signal, so hooks,
/// the dreamer and the transcript write must already be done.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_last_message_writes_the_settled_answer() {
    let layout = Layout::new();
    let service = ScriptedAnthropicService::text("### Done\n\n- the settled answer\n")
        .await
        .expect("start text script");
    let answer_path = layout.cwd.join("nested").join("answer.txt");
    let path_arg = answer_path.to_string_lossy().into_owned();
    let mut args = interactive_args().to_vec();
    args.push("--last-message");
    args.push(&path_arg);
    let mut run = pty(&layout, service.base_url(), &args);
    run.wait_for("directory:", TEST_TIMEOUT);
    run.send(b"Say the thing\r").expect("send prompt");
    run.wait_for("the settled answer", TEST_TIMEOUT);
    let _ = run.finish();

    let written = std::fs::read_to_string(&answer_path)
        .expect("the file exists once the session has exited — its directory too");
    assert!(
        written.contains("the settled answer"),
        "the whole final answer lands, not a fragment: {written:?}"
    );
    assert!(
        !written.contains('\u{1b}'),
        "no escape byte may reach a file a program reads: {written:?}"
    );
}

/// Ctrl+T shows what the compact committed cell omitted.
///
/// Codex's compact grouped cell does not commit each tool body to scrollback.
/// The transcript overlay is the person's door to the complete source, and it
/// has to work in a real terminal: the overlay takes the whole viewport, so a
/// wiring mistake shows up as a blank screen rather than as a failing unit.
///
/// It bites: with the Ctrl+T press removed the fixture body is nowhere in the
/// capture at all (measured), which is the whole claim — compaction omits it,
/// and this overlay is what opens it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_ctrl_t_unfolds_the_tool_body_the_cell_hid() {
    let layout = Layout::new();
    layout.fixture("FIXTURE_MARKER_IN_TRANSCRIPT\n");
    let service = ScriptedAnthropicService::bash_read(
        "### Tool answer\n\n- bash command completed\n- fixture read\n",
    )
    .await
    .expect("start bash/read script");
    let args = interactive_args();
    let mut run = pty(&layout, service.base_url(), &args);

    run.wait_for("directory:", TEST_TIMEOUT);
    run.send(b"Run Bash once and Read fixture.txt once, then summarize\r")
        .expect("send tool prompt");
    run.wait_for("fixture read", TEST_TIMEOUT);

    // Ctrl+T. Codex's own binding, and the same key closes it again.
    run.send(&[0x14]).expect("send ctrl+t");
    run.wait_for("Transcript", TEST_TIMEOUT);
    run.wait_for("tool output is unfolded here", TEST_TIMEOUT);
    let capture = run.finish();
    let text = String::from_utf8_lossy(&capture);

    assert!(
        text.contains("FIXTURE_MARKER_IN_TRANSCRIPT"),
        "the read body the compact cell omitted must be readable in the transcript"
    );
    assert!(
        text.contains("ctrl+t/esc close"),
        "the overlay names its own way out"
    );
}

/// The delegation cell reads as a task, not as arguments.
///
/// The screen this fixes showed
/// `Called Agent({"allow_cross_provider":true,"description":"zo 오 …` — a
/// truncated JSON blob that spent the whole line on syntax and cut off before
/// the field a watcher wanted. Codex puts a sentence there. Driven through a
/// real PTY because the defect was only ever visible on a screen.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_a_delegation_cell_shows_its_task_not_its_arguments() {
    let layout = Layout::new();
    layout.model_led();
    let service = ScriptedAnthropicService::spawn_agent("### Done\n\n- delegated\n")
        .await
        .expect("start spawn script");
    let args = interactive_args();
    let mut run = pty(&layout, service.base_url(), &args);

    run.wait_for("directory:", TEST_TIMEOUT);
    run.send(b"Delegate the survey\r").expect("send spawn prompt");
    // The humanized detail, straight off the wire's `subagent_type` and
    // `description`.
    run.wait_for("survey the fixture tree", TEST_TIMEOUT);
    let capture = run.finish();
    let screen = String::from_utf8_lossy(&capture);
    let plain_screen = strip_ansi(&screen);

    if std::env::var("SHOW_CELL").is_ok() {
        for line in plain_screen.lines() {
            let line = line.trim_end();
            if line.contains('•') || line.contains('└') || line.contains("Agent") {
                eprintln!("CELL {line:?}");
            }
        }
    }
    assert!(
        plain_screen.contains("general-purpose"),
        "the cell must name the harness the child runs under"
    );
    for syntax in ["allow_cross_provider", "{\"", "\":\""] {
        assert!(
            !plain_screen.contains(syntax),
            "argument syntax reached the screen ({syntax:?}) — this is the defect"
        );
    }

    // A detached spawn commits the collab-shaped completion cell and suppresses
    // the result envelope; the report arrives later as its own AgentResult cell.
    assert!(
        plain_screen.contains("Spawned agent [general-purpose]"),
        "the result cell must use the spawned-agent grammar"
    );
    for syntax in ["agentId", "outputFile", "subagentType"] {
        assert!(
            !plain_screen.contains(syntax),
            "result envelope keys reached the screen ({syntax:?})"
        );
    }
}

/// Anthropic thinking has no bold heading, so the status row said `Working`
/// for the whole block and the body never reached the screen (2026-09-22,
/// "thinking 중에 뭘 하는지 확인이 안 됨"). Now the row says the newest
/// complete sentence and the block commits as a titled thinking cell; a
/// `/thinking` keeps the row's word and drops the cell (t-5872).
///
/// The delta-to-screen latency is measured, not assumed: the script writes
/// one SSE event per sentence with its instant, and the screen is polled
/// until the sentence's word stands on the row.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_headerless_thinking_names_the_status_row_and_commits_a_titled_cell() {
    let layout = Layout::new();
    let sentences = [
        "The person wants the fixture summarised. ",
        "Let me read the fixture before answering. ",
        "It is three lines long, so one sentence will do.",
    ];
    let words = [
        "The person wants the fixture summarised",
        "Let me read the fixture before answering",
        "It is three lines long, so one sentence will do",
    ];
    let gap = Duration::from_millis(400);
    let service = ScriptedAnthropicService::thinking(&sentences, gap, "### Summary\n\n- three lines\n")
        .await
        .expect("start thinking script");
    let mut run = pty(&layout, service.base_url(), &interactive_args());
    let timeout = Duration::from_secs(20);

    run.wait_for("directory:", TEST_TIMEOUT);
    run.send(b"Summarise the fixture\r").expect("send prompt");

    // Each sentence's word on the status row, and how long after its delta
    // left the server. The shimmer styles every glyph, so the row is read
    // through the screen, never as raw bytes.
    let mut latencies_ms = Vec::new();
    for (index, word) in words.iter().enumerate() {
        let deadline = Instant::now() + timeout;
        let seen_at = loop {
            let mut screen = Screen::new(40);
            screen.feed(&run.snapshot_output());
            let visible = screen.visible();
            if visible
                .iter()
                .any(|row| row.contains("esc to interrupt") && row.contains(word))
            {
                break Instant::now();
            }
            assert!(
                Instant::now() < deadline,
                "sentence {index} never reached the status row: {visible:#?}"
            );
            std::thread::sleep(Duration::from_millis(2));
        };
        // Event 0 of request 0 is `message_start`, 1 is the block start, so
        // sentence `index` is event `index + 2`.
        let sent_at = service
            .event_marks()
            .into_iter()
            .find(|mark| mark.request_index == 0 && mark.event_index == index + 2)
            .map(|mark| mark.at)
            .expect("the sentence's event mark");
        latencies_ms.push(seen_at.saturating_duration_since(sent_at).as_secs_f64() * 1000.0);
    }
    eprintln!("thinking delta → status row (ms): {latencies_ms:.1?}");
    // A frame is 32 ms and the poll adds 2 ms; measured alone this is under
    // 50 ms. The bound is a second because this binary's PTY cases run
    // beside one another under `just test`: a stall is what it refuses, not
    // a loaded scheduler.
    assert!(
        latencies_ms.iter().all(|ms| *ms < 1_000.0),
        "the status word lagged its delta: {latencies_ms:?}"
    );

    // The block ends: a titled thinking cell with the sentences under it,
    // then the answer.
    run.wait_for_history_row("Thinking", timeout);
    run.wait_for_history_row("three lines", timeout);
    let mut screen = Screen::new(40);
    screen.feed(&run.snapshot_output());
    let transcript = screen.transcript();
    let title = transcript
        .iter()
        .position(|row| row.trim() == "• Thinking")
        .unwrap_or_else(|| panic!("no thinking title row: {transcript:#?}"));
    assert!(
        transcript[title + 1].contains("The person wants the fixture summarised"),
        "the body follows the title: {transcript:#?}"
    );
    assert!(
        transcript
            .iter()
            .position(|row| row.contains("three lines"))
            .is_some_and(|answer| answer > title),
        "the answer follows the thinking cell: {transcript:#?}"
    );

    // `/thinking` hides the next block's cell; the row still gets its word.
    run.send(b"/thinking\r").expect("send /thinking");
    run.wait_for_history_row("thinking hidden", timeout);
    let cells_before = {
        let mut screen = Screen::new(40);
        screen.feed(&run.snapshot_output());
        screen.transcript().iter().filter(|row| row.trim() == "• Thinking").count()
    };
    run.send(b"Summarise it again\r").expect("send second prompt");
    let deadline = Instant::now() + timeout;
    loop {
        let mut screen = Screen::new(40);
        screen.feed(&run.snapshot_output());
        if screen
            .visible()
            .iter()
            .any(|row| row.contains("esc to interrupt") && row.contains(words[0]))
        {
            break;
        }
        assert!(Instant::now() < deadline, "the hidden block still names the row");
        std::thread::sleep(Duration::from_millis(5));
    }
    run.wait_for_history_row("three lines", timeout);
    let mut screen = Screen::new(40);
    screen.feed(&run.snapshot_output());
    let cells_after = screen.transcript().iter().filter(|row| row.trim() == "• Thinking").count();
    assert_eq!(cells_after, cells_before, "a hidden block commits no thinking cell");
    let _ = run.finish();
}

/// One turn, one spawn, four ledgers — joined by equality on the attempt key.
///
/// This is the case the attempt-key contract exists for
/// (`docs/design/zo-attempt-key-contract-20260915.md` §3.4). Before it, a
/// turn's request rows, its timing rows and the record of what it decided
/// shared no column at all, so "what did this task cost, and was it verified"
/// could not be asked of the disk. The assertions below are equalities, not
/// shapes: every reader joins on `==` and none parses a key.
///
/// The provider is the local SSE script, so the run spends nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_one_attempt_key_joins_the_request_timing_and_outcome_ledgers() {
    let layout = Layout::new();
    layout.model_led();
    let service = ScriptedAnthropicService::spawn_agent("### Done\n\n- delegated\n")
        .await
        .expect("start spawn script");
    let args = interactive_args();
    let mut run = pty(&layout, service.base_url(), &args);

    run.wait_for("directory:", TEST_TIMEOUT);
    run.send(b"Delegate the survey\r").expect("send spawn prompt");
    // The delegation cell's detail, the one needle this screen paints without
    // an escape sequence through the middle of it.
    run.wait_for("survey the fixture tree", TEST_TIMEOUT);
    // Then wait for the EFFECT this test is about, not for a wall clock: the
    // rows land while the run is still alive, so poll the ledgers rather than
    // guessing how long a spawn takes on a loaded machine.
    let requests = await_request_rows(&layout, TEST_TIMEOUT);
    let _ = run.finish();

    assert!(
        !requests.is_empty(),
        "the run recorded no request rows at all under {}",
        layout.home.display()
    );

    let parent_attempts = attempts_under(&requests, "session-");
    let child_attempts = attempts_under(&requests, "agent-");
    let parent_attempt = assert_turn_keys(&parent_attempts);
    let child_attempt = assert_spawn_key(&child_attempts);

    // Every request row carries the whole bill, not just the input side.
    assert!(
        requests
            .iter()
            .any(|(_, row)| row["output"].as_u64().unwrap_or(0) > 0),
        "no request row recorded its output tokens: {requests:?}"
    );

    // The outcome ledger's spawn row IS the child's attempt, and it names the
    // turn it served.
    let outcomes = read_outcome_rows(&layout);
    let spawn_row = outcomes
        .iter()
        .find(|row| row["runId"].as_str() == Some(child_attempt.as_str()))
        .unwrap_or_else(|| panic!("no outcome row names {child_attempt}: {outcomes:?}"));
    assert_eq!(
        spawn_row["parentAttempt"].as_str(),
        Some(parent_attempt.as_str()),
        "the spawn must name the turn that paid for it"
    );
    assert_eq!(
        spawn_row["shape"].as_str(),
        Some("delegate"),
        "one `Agent` call is one delegation"
    );

    // The timing ledger joins on the same key. Only the streaming turn loop
    // feeds it — a row is written from the probe-channel stamps — and a
    // spawned agent runs the sync loop, which mirrors no blocks
    // (`StreamStamps::left_the_runtime`). So the child's waits are on its
    // manifest, not here; what this ledger must never do is write a row that
    // names no attempt, because such a row joins to nothing.
    let timings = read_timing_rows(&layout);
    assert!(
        timings
            .iter()
            .any(|row| row["attempt"].as_str() == Some(parent_attempt.as_str())),
        "no timing row carries {parent_attempt}: {timings:?}"
    );
    for row in &timings {
        assert!(
            row["attempt"].as_str().is_some_and(|key| !key.is_empty()),
            "a timing row with no attempt joins to nothing: {row:?}"
        );
    }

    // The join this test exists to prove, printed so a report can quote the
    // real keys rather than describe them (`--nocapture`).
    println!(
        "ATTEMPT-JOIN parent={parent_attempt} child={child_attempt} \
         spawn.runId={:?} spawn.parentAttempt={:?} spawn.shape={:?} \
         parent_requests={} child_requests={} timings={}",
        spawn_row["runId"].as_str(),
        spawn_row["parentAttempt"].as_str(),
        spawn_row["shape"].as_str(),
        parent_attempts.len(),
        child_attempts.len(),
        timings.len(),
    );

    // Hermetic: the script provider billed nothing real.
    let [input, _cache_read, _cache_write] = service.usage().await;
    assert!(input > 0, "the scripted provider answered at least one request");
}

/// Every attempt named by a request row written under a prompt-cache directory
/// whose name starts with `prefix` (`session-` for the turn, `agent-` for a
/// spawn), in the order the rows were written.
fn attempts_under(rows: &[(String, serde_json::Value)], prefix: &str) -> Vec<String> {
    rows.iter()
        .filter(|(dir, _)| dir.starts_with(prefix))
        .filter_map(|(_, row)| row["attempt"].as_str().map(str::to_string))
        .collect()
}

/// Check the session's own keys and answer with the FIRST — the turn that made
/// the spawn.
///
/// A detached agent's result re-enters the session as a follow-up turn of its
/// own, so a run like this legitimately spans more than one attempt. That is
/// the point: each turn pays for its own requests, and the spawn belongs to
/// the turn that asked for it. What must hold is that the ordinals a session
/// hands out run 1, 2, … without a gap or a repeat, so two turns can never
/// collide on one key.
fn assert_turn_keys(attempts: &[String]) -> String {
    let first = attempts
        .first()
        .cloned()
        .expect("no request row of the session carried an attempt");
    let session_of = |attempt: &str| {
        attempt
            .rsplit_once('@')
            .unwrap_or_else(|| panic!("a main-turn attempt is `<sessionId>@<turn>`: {attempt}"))
            .0
            .to_string()
    };
    let mut ordinals = std::collections::BTreeSet::new();
    for attempt in attempts {
        assert!(
            !attempt.contains('#'),
            "a main-turn attempt is not a spawn key: {attempt}"
        );
        assert_eq!(
            session_of(attempt),
            session_of(&first),
            "every turn of one session shares its session id: {attempts:?}"
        );
        let ordinal: u32 = attempt
            .rsplit_once('@')
            .expect("checked above")
            .1
            .parse()
            .unwrap_or_else(|_| panic!("a turn ordinal is a number: {attempt}"));
        assert!(ordinal >= 1, "a turn ordinal is 1-based: {attempt}");
        ordinals.insert(ordinal);
    }
    assert_eq!(
        ordinals.iter().copied().collect::<Vec<_>>(),
        (1..=u32::try_from(ordinals.len()).expect("small")).collect::<Vec<_>>(),
        "turn ordinals must run 1..n without a gap: {attempts:?}"
    );
    first
}

/// Check the spawned agent's keys and answer with the one they all name — the
/// equality that lets a spawn's tokens be added to the turn that paid.
fn assert_spawn_key(attempts: &[String]) -> String {
    let first = attempts
        .first()
        .cloned()
        .expect("the spawned agent recorded no request rows — its cost is off the disk");
    assert!(
        attempts.iter().all(|attempt| *attempt == first),
        "one run spends on one attempt: {attempts:?}"
    );
    assert!(
        first.contains('#') && !first.contains('@'),
        "a spawn attempt is `<agentId>#<generation>`: {first}"
    );
    first
}

/// Poll until both the session's and the spawned agent's request rows carry an
/// attempt, or the deadline passes — then answer with whatever is on disk, so
/// the assertions (not this helper) say what was missing.
fn await_request_rows(layout: &Layout, within: Duration) -> Vec<(String, serde_json::Value)> {
    let deadline = Instant::now() + within;
    loop {
        let rows = read_request_rows(layout);
        let attempted = |prefix: &str| {
            rows.iter().any(|(dir, row)| {
                dir.starts_with(prefix)
                    && row["attempt"].as_str().is_some_and(|key| !key.is_empty())
            })
        };
        if attempted("session-") && attempted("agent-") {
            return rows;
        }
        if Instant::now() >= deadline {
            return rows;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// `(<prompt-cache dir name>, row)` for every request row the run wrote.
fn read_request_rows(layout: &Layout) -> Vec<(String, serde_json::Value)> {
    let root = layout.home.join("cache").join("prompt-cache");
    let mut rows = Vec::new();
    let Ok(entries) = fs::read_dir(&root) else {
        return rows;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = entry.path().join("requests.jsonl");
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        for line in text.lines() {
            if let Ok(row) = serde_json::from_str::<serde_json::Value>(line) {
                rows.push((name.clone(), row));
            }
        }
    }
    rows
}

fn read_project_state_rows(layout: &Layout, relative: &[&str]) -> Vec<serde_json::Value> {
    let projects = layout.state.join("projects");
    let mut rows = Vec::new();
    let Ok(entries) = fs::read_dir(&projects) else {
        return rows;
    };
    for entry in entries.flatten() {
        let mut path = entry.path().join("state");
        for part in relative {
            path = path.join(part);
        }
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        rows.extend(
            text.lines()
                .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok()),
        );
    }
    rows
}

fn read_outcome_rows(layout: &Layout) -> Vec<serde_json::Value> {
    read_project_state_rows(layout, &["smart-router", "route-outcomes.jsonl"])
}

fn read_timing_rows(layout: &Layout) -> Vec<serde_json::Value> {
    read_project_state_rows(layout, &["request-timings", "timings.jsonl"])
}

/// A Workflow owns one outer tool call while two real helpers execute below
/// it. The first frame that exposes their tool activity must already contain
/// the complete helper rows and wave tally; a later reconciliation is too
/// late for the person deciding whether the run is hung.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_workflow_first_tool_frame_shows_both_live_helpers() {
    let layout = Layout::new();
    let service = ScriptedAnthropicService::workflow_two_helpers(
        "### Workflow done\n\n- both helpers returned\n",
    )
    .await
    .expect("start workflow script");
    let mut run = pty(&layout, service.base_url(), &interactive_args());
    let timeout = Duration::from_secs(20);

    run.wait_for("directory:", TEST_TIMEOUT);
    run.send(b"Run the two-helper workflow\r")
        .expect("send workflow prompt");
    run.wait_for("agents 2 · running 2 · done 0", timeout);
    run.wait_for("Bash · sleep", timeout);

    let mut screen = Screen::new(40);
    screen.feed(&run.snapshot_output());
    let visible = screen.visible();
    let live_helpers = visible
        .iter()
        .filter(|row| row.contains("tool use") && row.contains("Bash · sleep"))
        .count();
    assert_eq!(
        live_helpers, 2,
        "the first visible helper-tool frame did not carry both rows: {visible:#?}"
    );
    assert!(
        visible
            .iter()
            .any(|row| row.contains("agents 2 · running 2 · done 0")),
        "the helper rows arrived without their wave summary: {visible:#?}"
    );

    run.wait_for_history_row("both helpers returned", timeout);
    let _ = run.finish();
}

/// A `Large` turn is pre-analysed by the host before the model sees it: the
/// harness splits it, runs the lanes read-only, seats their findings as a
/// `SpawnMultiAgent` result, and the main turn's request carries them — and
/// the screen says what the harness is doing before the model says anything.
/// The prompt is the one `tools` pins as `Large` with independent lanes
/// (`a_whole_repo_migration_across_named_lanes_classifies_large_and_fans_out`);
/// difficulty alone no longer buys the split (t-3854).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_a_large_turn_is_pre_analysed_by_the_host_before_the_model_turn() {
    let layout = Layout::new();
    let service = ScriptedAnthropicService::host_prelude_two_lanes(
        "### Migrated\n\n- built on both lanes\n",
    )
    .await
    .expect("start prelude script");
    let mut run = pty(&layout, service.base_url(), &interactive_args());

    run.wait_for("directory:", TEST_TIMEOUT);
    run.send("레포 전체를 훑어서 parser/lexer 모듈을 모두 마이그레이션해줘. 여러 단계에 걸쳐 진행해야 한다.\r".as_bytes())
        .expect("send the large prompt");
    run.wait_for("pre-analysis fan-out", TEST_TIMEOUT);
    run.wait_for("2 agents completed", TEST_TIMEOUT);
    run.wait_for_history_row("built on both lanes", TEST_TIMEOUT);
    let _ = run.finish();

    let bodies = service.request_bodies().await;
    let main_turn = bodies
        .iter()
        .find(|body| body.contains("Smart pre-analysis"))
        .expect("the main turn carries the seated pre-analysis");
    for (_, finding) in e2e::scripted::PRELUDE_LANES {
        let (label, _) = finding.split_once(':').expect("finding label");
        assert!(main_turn.contains(label), "{label} did not reach the main turn");
    }
    assert!(
        main_turn.contains("Pre-analysis already ran this turn"),
        "the fanned-out reminder must ride the main turn"
    );
    assert!(
        !bodies.iter().any(|body| body.contains("UNEXPECTED")),
        "a request reached the provider that the prelude script did not expect"
    );
}

/// Type into a BUSY turn and the model actually receives it.
///
/// The complaint was "말을 걸어도 대답이 없다": a line typed while a tool ran
/// showed `steer queued` and then nothing. Nothing was lost — the wire shape
/// puts a `tool_result` after a `tool_use`, so the steer can only land at the
/// tool-result boundary — but until that boundary arrives the screen looks
/// ignored, and if the tool runs for minutes it IS ignored for minutes.
///
/// This drives the real thing: a tool that takes seconds, a line typed while
/// it runs, and then the NEXT provider request inspected for the words. The
/// request body is the only place that can prove delivery; the screen can only
/// show that something was queued.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_a_line_typed_during_a_running_tool_reaches_the_model() {
    let layout = Layout::new();
    let service = ScriptedAnthropicService::slow_tool("### Answered\n\n- yes, still running\n")
        .await
        .expect("start slow-tool script");
    let args = interactive_args();
    let mut run = pty(&layout, service.base_url(), &args);

    run.wait_for("directory:", TEST_TIMEOUT);
    run.send(b"Run the slow command\r").expect("send prompt");
    // The tool is now running for ~3s. Type into it, the way a person does.
    run.wait_for("sleep 3", TEST_TIMEOUT);
    run.send("아직 도는중?\r".as_bytes()).expect("steer mid-tool");
    run.wait_for(
        "Messages to be submitted after next tool call",
        TEST_TIMEOUT,
    );
    let preview = run.snapshot_output();
    assert!(
        !history_rows_raw(&preview)
            .iter()
            .any(|row| find(row, "아직 도는중?".as_bytes()).is_some()),
        "a pending steer was committed before the runtime consumed it"
    );
    run.wait_for("yes, still running", TEST_TIMEOUT);
    run.wait_for_history_row("yes, still running", TEST_TIMEOUT);
    let capture = run.finish();

    let requests = service.request_bodies().await;
    assert_eq!(requests.len(), 2, "one tool turn and one final turn");
    assert!(
        requests[1].contains("도는중"),
        "the steer never reached the model — request was {}",
        &requests[1][..requests[1].len().min(2000)]
    );
    assert!(
        requests[1].contains("tool_result"),
        "the steer must ride WITH the tool result, not as a second user turn"
    );
    let screen = String::from_utf8_lossy(&capture);
    assert!(
        screen.contains("yes, still running"),
        "the model's answer to the steer must reach the screen"
    );
    assert!(!screen.contains("↳ steer queued"));
    assert!(!screen.contains("⤷ steering:"));
    let steer_rows = history_rows_raw(&capture)
        .into_iter()
        .filter(|row| find(row, "아직 도는중?".as_bytes()).is_some())
        .count();
    assert_eq!(steer_rows, 1, "the consumed steer must be one user row");
    assert_history_golden(
        &capture,
        "steer-preview",
        "pending steer preview and consumed user cell",
        &layout,
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_tab_queues_a_follow_up_turn_without_optimistic_history() {
    let layout = Layout::new();
    let service = ScriptedAnthropicService::slow_tool("R34_FOLLOW_UP_DONE\n")
        .await
        .expect("start slow-tool script");
    let args = interactive_args();
    let mut run = pty(&layout, service.base_url(), &args);

    run.wait_for("directory:", TEST_TIMEOUT);
    run.send(b"Run the slow command\r").expect("send prompt");
    run.wait_for("sleep 3", TEST_TIMEOUT);
    run.send(b"R34_QUEUED_FOLLOW_UP\t")
        .expect("queue follow-up with Tab");
    run.wait_for("Queued follow-up inputs", TEST_TIMEOUT);
    let preview = run.snapshot_output();
    assert!(
        !history_rows_raw(&preview)
            .iter()
            .any(|row| find(row, b"R34_QUEUED_FOLLOW_UP").is_some()),
        "a Tab-queued draft was committed before its turn started"
    );

    // The queued turn starts on the child's clock: its answer can be in the
    // capture before this thread reads the length, so the second wait
    // resumes from the first match's end, not from a fresh length.
    let after_first_answer = run.wait_for("R34_FOLLOW_UP_DONE", TEST_TIMEOUT);
    run.wait_for_after("R34_FOLLOW_UP_DONE", after_first_answer, TEST_TIMEOUT);
    let capture = run.finish();
    let requests = service.request_bodies().await;
    assert_eq!(
        requests.len(),
        3,
        "tool request, its final response, then one queued follow-up turn"
    );
    assert!(requests[2].contains("R34_QUEUED_FOLLOW_UP"));
    assert!(
        !requests[1].contains("R34_QUEUED_FOLLOW_UP"),
        "Tab must queue a new turn, not steer the running one"
    );
    let queued_rows = history_rows_raw(&capture)
        .into_iter()
        .filter(|row| find(row, b"R34_QUEUED_FOLLOW_UP").is_some())
        .count();
    assert_eq!(queued_rows, 1, "the queued turn must commit one user row");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_alt_up_edits_the_latest_queued_message() {
    let layout = Layout::new();
    let service = ScriptedAnthropicService::slow_tool("R34_EDITED_DONE\n")
        .await
        .expect("start slow-tool script");
    let args = interactive_args();
    let mut run = pty(&layout, service.base_url(), &args);

    run.wait_for("directory:", TEST_TIMEOUT);
    run.send(b"Run the slow command\r").expect("send prompt");
    run.wait_for("sleep 3", TEST_TIMEOUT);
    run.send(b"R34_QUEUED_DRAFT\t")
        .expect("queue draft with Tab");
    run.wait_for("Queued follow-up inputs", TEST_TIMEOUT);
    let before_edit = run.output_len();
    run.send(b"\x1b[1;3A").expect("Alt+Up queued edit");
    run.wait_for_after("R34_QUEUED_DRAFT", before_edit, TEST_TIMEOUT);
    run.send(b"\r").expect("submit restored draft as steer");
    run.wait_for(
        "Messages to be submitted after next tool call",
        TEST_TIMEOUT,
    );
    run.wait_for("R34_EDITED_DONE", TEST_TIMEOUT);
    let capture = run.finish();

    let requests = service.request_bodies().await;
    assert_eq!(requests.len(), 2, "the edited draft must not start a third turn");
    assert!(requests[1].contains("R34_QUEUED_DRAFT"));
    let rows = history_rows_raw(&capture);
    assert_eq!(
        rows.iter()
            .filter(|row| find(row, b"R34_QUEUED_DRAFT").is_some())
            .count(),
        1,
        "the edited draft must commit only when its steer is consumed"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_esc_with_pending_steer_resubmits_it_as_a_fresh_turn() {
    let layout = Layout::new();
    let service = ScriptedAnthropicService::slow_tool("R34_STEERED_AFTER_ESC\n")
        .await
        .expect("start slow-tool script");
    let args = interactive_args();
    let mut run = pty(&layout, service.base_url(), &args);

    run.wait_for("directory:", TEST_TIMEOUT);
    run.send(b"Run the slow command\r").expect("send prompt");
    run.wait_for("sleep 3", TEST_TIMEOUT);
    run.send(b"R34_SEND_NOW\r").expect("submit pending steer");
    run.wait_for(
        "Messages to be submitted after next tool call",
        TEST_TIMEOUT,
    );
    run.send(b"\x1b").expect("Esc send immediately");
    run.wait_for(
        "Model interrupted to submit steer instructions.",
        TEST_TIMEOUT,
    );
    run.wait_for("R34_STEERED_AFTER_ESC", TEST_TIMEOUT);
    let capture = run.finish();

    let requests = service.request_bodies().await;
    assert_eq!(requests.len(), 2, "cancelled tool turn plus one fresh steer turn");
    assert!(requests[1].contains("R34_SEND_NOW"));
    let screen = String::from_utf8_lossy(&capture);
    assert!(!screen.contains("↳ steer queued"));
    assert!(!screen.contains("⤷ steering:"));
    assert_eq!(
        history_rows_raw(&capture)
            .iter()
            .filter(|row| find(row, b"R34_SEND_NOW").is_some())
            .count(),
        1,
        "fresh steer turn must have exactly one user row"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_table_answer_is_held_until_the_final_commit() {
    let layout = Layout::new();
    let service = ScriptedAnthropicService::text(TABLE_ANSWER)
        .await
        .expect("start table script");
    let args = interactive_args();
    let mut run = pty(&layout, service.base_url(), &args);

    run.wait_for("directory:", TEST_TIMEOUT);
    run.send(b"Print the deterministic table exactly\r")
        .expect("send table prompt");
    run.wait_for_history_row("4444", TEST_TIMEOUT);
    let capture = run.finish();

    assert_history_golden(&capture, "table", "table answer", &layout);
    let requests = service.request_bodies().await;
    assert_eq!(requests.len(), 1, "unexpected requests: {requests:#?}");
}

/// Repaint cost of one turn, measured on today's bytes.
///
/// Not a contract — a MEASUREMENT, so it is `#[ignore]`d and prints instead of
/// asserting. It exists because the archived zo captures are days old and this
/// repo has already been fooled once by reading a stale one as a live defect
/// (the bare-truecolor SGR).
///
/// Run it against either scenario:
///
/// ```text
/// MEASURE=table cargo test -p zo-ide --test e2e_hermetic repaint_cost -- --ignored --nocapture
/// MEASURE=slow  cargo test -p zo-ide --test e2e_hermetic repaint_cost -- --ignored --nocapture
/// MEASURE=table-big cargo test -p zo-ide --test e2e_hermetic repaint_cost -- --ignored --nocapture
/// ```
///
/// **Read two sizes, never one.** A single capture cannot separate a turn's
/// fixed cost — the boot banner, and the composer echoing every character of
/// the typed prompt — from what one more line of output costs. Dividing total
/// bytes by committed lines blames the streaming for the fixed part, and the
/// smaller the answer the louder that lie gets:
///
/// ```text
/// MEASURE=table      19 lines   21,639 bytes   1,139 B/line   6.10 EL/line
/// MEASURE=table-big  83 lines   27,831 bytes     335 B/line   2.43 EL/line
/// marginal                       6,192 / 64        97 B/line   1.34 EL/line
/// ```
///
/// That is the correction to `docs/captures/frame-diff-r9.md`, which read the
/// 19-line capture against codex's 54-line one and concluded zo repaints twice
/// per line. At comparable output size zo is **cheaper on both axes** than
/// codex's own table capture (734 B/line, 3.06 EL/line), and codex's captures
/// spread 734–3,389 B/line among themselves — the ratio is a property of how
/// much the answer said, not of the renderer.
///
/// EL per FRAME is no better: zo batches more work into fewer frames (83 lines
/// and 19 lines both took 49), so it *rises* as the renderer does better.
/// `docs/captures/frame-diff-r10.md` carries the full re-measurement.
/// One measurement scenario: the scripted answer, the prompt to type, and the
/// text whose arrival means the turn finished.
async fn repaint_scenario(which: &str) -> (ScriptedAnthropicService, &'static str, &'static str) {
    match which {
        "table-big" => (
            ScriptedAnthropicService::text(TABLE_ANSWER_BIG)
                .await
                .expect("big table"),
            "Print the deterministic table exactly",
            "lamed",
        ),
        "slow" => (
            ScriptedAnthropicService::slow_tool("### Done\n\n- slow finished\n")
                .await
                .expect("slow"),
            "Run the slow command",
            "slow finished",
        ),
        _ => (
            ScriptedAnthropicService::text(TABLE_ANSWER)
                .await
                .expect("table"),
            "Print the deterministic table exactly",
            "delta",
        ),
    }
}

#[ignore = "measurement, not a contract"]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn repaint_cost() {
    let layout = Layout::new();
    let which = std::env::var("MEASURE").unwrap_or_else(|_| "table".to_string());
    let (service, prompt, marker) = repaint_scenario(&which).await;
    let args = interactive_args();
    let mut run = pty(&layout, service.base_url(), &args);
    run.wait_for("directory:", TEST_TIMEOUT);
    let mut line = prompt.as_bytes().to_vec();
    line.push(b'\r');
    run.send(&line).expect("send");
    run.wait_for(marker, TEST_TIMEOUT);
    let capture = run.finish();

    let needle = b"\r\n\x1b[39;49m\x1b[K";
    let hist = capture.windows(needle.len()).filter(|w| *w == needle).count();
    let el = capture.windows(3).filter(|w| *w == b"\x1b[K").count();
    let frames = capture.windows(8).filter(|w| *w == b"\x1b[?2026h").count();
    // Ratios in hundredths, kept integer: these are counts of escape
    // sequences, and a float here buys nothing but a clippy exemption.
    let per = |numerator: usize, denominator: usize| {
        let denominator = denominator.max(1);
        format!("{}.{:02}", numerator / denominator, numerator * 100 / denominator % 100)
    };
    eprintln!(
        "[{which}] bytes={} committed_lines={hist} EL={el} frames={frames} \
         EL/line={} EL/frame={}",
        capture.len(),
        per(el, hist),
        per(el, frames)
    );

    // Where the repaints land. A CUP followed by the viewport payload
    // (`ESC[0m ESC[K`) is one repainted row, so the histogram says which part
    // of the screen is doing the work.
    let text = String::from_utf8_lossy(&capture);
    let mut rows: std::collections::BTreeMap<u32, usize> = std::collections::BTreeMap::new();
    let mut rest = text.as_ref();
    while let Some(at) = rest.find("\u{1b}[") {
        let after = &rest[at + 2..];
        if let Some(end) = after.find('H') {
            if let Some((row, _)) = after[..end].split_once(';') {
                if let Ok(row) = row.parse::<u32>() {
                    if after[end + 1..].starts_with("\u{1b}[0m\u{1b}[K") {
                        *rows.entry(row).or_default() += 1;
                    }
                }
            }
        }
        rest = &rest[at + 2..];
    }
    eprintln!("[{which}] repainted rows: {rows:?}");

    if std::env::var("MEASURE_ROW").is_ok() {
        let hottest = rows.iter().max_by_key(|(_, count)| **count).map(|(row, _)| *row);
        if let Some(hot) = hottest {
            let mut payloads: Vec<String> = Vec::new();
            let mut rest = text.as_ref();
            while let Some(at) = rest.find("\u{1b}[") {
                let after = &rest[at + 2..];
                if let Some(end) = after.find('H') {
                    if let Some((row, _)) = after[..end].split_once(';') {
                        if row.parse::<u32>() == Ok(hot) {
                            let body = &after[end + 1..];
                            if body.starts_with("\u{1b}[0m\u{1b}[K") {
                                let cut = body[2..]
                                    .find("\u{1b}[")
                                    .map_or(body.len(), |next| next + 2);
                                let mut piece = body[..cut.min(body.len())].to_string();
                                if let Some(stop) = piece.find("\u{1b}[?25") {
                                    piece.truncate(stop);
                                }
                                payloads.push(piece.replace('\u{1b}', "^"));
                            }
                        }
                    }
                }
                rest = &rest[at + 2..];
            }
            eprintln!("[{which}] row {hot}: {} repaints", payloads.len());
            let mut previous: Option<&String> = None;
            let mut shown = 0;
            for payload in &payloads {
                if previous.is_some_and(|old| old == payload) {
                    eprintln!("   IDENTICAL repaint (the dirty check should have skipped this)");
                } else if shown < 6 {
                    eprintln!("   {}", &payload[..payload.len().min(150)]);
                    shown += 1;
                }
                previous = Some(payload);
            }
        }
    }
}

/// Does a long session grow without bound?
///
/// The claim this repo needs to be able to make is "usable for hours", and the
/// honest test of it is a real process under a real terminal answering turn
/// after turn while its resident set is sampled. A unit test cannot see the
/// allocator's high-water mark; `ps` can.
///
/// Measurement, not a contract — `#[ignore]`d, prints, asserts only the shape
/// that would make the numbers meaningless (a child that died mid-run).
///
/// ```text
/// cargo test -p zo-ide --test e2e_hermetic long_session_rss -- --ignored --nocapture
/// ```
#[ignore = "measurement, not a contract"]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn long_session_rss() {
    let layout = Layout::new();
    let service = ScriptedAnthropicService::text(TABLE_ANSWER)
        .await
        .expect("table script");
    let args = interactive_args();
    let mut run = pty(&layout, service.base_url(), &args);
    run.wait_for("directory:", TEST_TIMEOUT);

    let mut samples = Vec::new();
    for turn in 1..=12u32 {
        run.send(b"Print the deterministic table exactly\r").expect("send");
        run.wait_for("delta", TEST_TIMEOUT);
        let rss = run.rss_kib().expect("the child is still alive");
        samples.push(rss);
        eprintln!("turn {turn:2}: rss={rss} KiB");
    }
    // The average hides the shape. What decides "usable for hours" is whether
    // the curve FLATTENS: early growth is warm-up (allocator arenas, lazily
    // built caches), and a leak keeps the late half rising at the early rate.
    let half = samples.len() / 2;
    let early = samples[half].saturating_sub(samples[0]);
    let late = samples[samples.len() - 1].saturating_sub(samples[half]);
    eprintln!(
        "RSS {} -> {} KiB over {} turns · first half +{early} KiB · second half +{late} KiB",
        samples[0],
        samples[samples.len() - 1],
        samples.len()
    );
    let _ = run.finish();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_model_picker_opens_and_esc_returns_to_the_composer() {
    let layout = Layout::new();
    let service = ScriptedAnthropicService::text("unused\n")
        .await
        .expect("start picker provider");
    let args = interactive_args();
    let mut run = pty(&layout, service.base_url(), &args);

    run.wait_for("directory:", TEST_TIMEOUT);
    run.send(b"/model\r").expect("open model picker");
    run.wait_for("Select Model and Effort", TEST_TIMEOUT);
    run.wait_for("Press enter to confirm or esc to go back", TEST_TIMEOUT);
    run.send(b"\x1b").expect("close model picker");
    run.wait_for("Ask zo to do anything", TEST_TIMEOUT);
    let capture = run.finish();

    assert_block_golden(
        &capture,
        "model-picker",
        "model picker",
        &layout,
        b"Select Model and Effort",
        b"Press enter to confirm or esc to go back",
    );
    assert_eq!(service.request_bodies().await.len(), 0);
}

/// The inline viewport may use three blank rows between the submitted slash
/// cell and the composer (one cell tail plus the bottom-pane top padding).
/// Anything larger is unused screen, not UI chrome.
const MAX_INLINE_BLANK_ROWS: usize = 3;

/// Replay a capture around the same row growth the PTY underwent. The final
/// row must be the footer: locating a cursor near the bottom is weaker because
/// a later frame can still leave the visible footer parked above blank rows.
fn assert_footer_on_last_row_after_grow(capture: &[u8], grow_at: usize, scenario: &str) {
    let grow_at = grow_at.min(capture.len());
    let mut screen = Screen::new(40);
    screen.feed(&capture[..grow_at]);
    screen.resize(60);
    screen.feed(&capture[grow_at..]);
    let visible = screen.visible();
    if let Ok(path) = std::env::var("ZO_E2E_DUMP") {
        fs::write(format!("{path}.bin"), capture).expect("dump grow capture");
        fs::write(format!("{path}.screen.txt"), visible.join("\n"))
            .expect("dump settled grow screen");
    }
    let footer_row = visible
        .iter()
        .position(|row| row.contains("? for shortcuts"))
        .unwrap_or_else(|| panic!("{scenario}: no footer on the settled screen: {visible:#?}"));
    assert_eq!(
        footer_row,
        visible.len() - 1,
        "{scenario}: footer stopped on row {} of {}; blank rows below it: {visible:#?}",
        footer_row + 1,
        visible.len(),
    );
}

fn growing_pty(layout: &Layout, base_url: &str, cursor_row: u16) -> PtyRun {
    let args = interactive_args();
    let mut run = PtyRun::spawn_controlling_sized(
        40,
        120,
        &layout.cwd,
        &layout.home,
        &layout.sessions,
        &layout.state,
        base_url,
        &args,
        &[],
    )
    .expect("spawn growing PTY");
    // Queue the DSR reply before crossterm asks. Zed may answer with the cursor
    // in the middle of its grid; that is the state the old grow rule preserved.
    run.send(format!("\x1b[{cursor_row};1R").as_bytes())
        .expect("answer cursor position query");
    run
}

fn wait_for_bottom_anchored_frame(run: &mut PtyRun, grow_at: usize) {
    const LAST_ROW_CUP: &[u8] = b"\x1b[60;1H";
    const SYNC_END: &[u8] = b"\x1b[?2026l";
    run.wait_until(grow_at, TEST_TIMEOUT, |bytes| {
        find(bytes, LAST_ROW_CUP)
            .is_some_and(|last_row| find(&bytes[last_row..], SYNC_END).is_some())
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_grow_during_a_live_spinner_pins_the_footer_to_the_last_row() {
    let layout = Layout::new();
    let service = ScriptedAnthropicService::slow_tool("### Done\n\n- settled\n")
        .await
        .expect("start slow-tool provider");
    let mut run = growing_pty(&layout, service.base_url(), 18);
    run.wait_for("directory:", TEST_TIMEOUT);
    run.send(b"Run the slow command\r").expect("send prompt");
    run.wait_for("sleep 3", TEST_TIMEOUT);

    let grow_at = run.output_len();
    run.resize(60, 120).expect("grow PTY");
    wait_for_bottom_anchored_frame(&mut run, grow_at);
    let capture = run.snapshot_output();
    run.kill9();
    let _ = run.finish();

    assert_footer_on_last_row_after_grow(&capture, grow_at, "live spinner grow");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_grow_after_context_trim_with_a_live_spinner_pins_the_footer() {
    let layout = Layout::new();
    let service = ScriptedAnthropicService::context_trim_then_slow_tool()
        .await
        .expect("start context-trim provider");
    let mut run = growing_pty(&layout, service.base_url(), 12);
    run.wait_for("directory:", TEST_TIMEOUT);
    run.send(b"Trim old context\r").expect("send trim prompt");
    run.wait_for_history_row("Context trim", TEST_TIMEOUT);
    wait_until_quiet(&run, Duration::from_millis(250), TEST_TIMEOUT);
    run.send(b"Keep working while the pane grows\r")
        .expect("send slow turn");
    run.wait_for("sleep 3", TEST_TIMEOUT);

    let grow_at = run.output_len();
    run.resize(60, 120).expect("grow PTY after trim");
    wait_for_bottom_anchored_frame(&mut run, grow_at);
    let capture = run.snapshot_output();
    run.kill9();
    let _ = run.finish();

    let screen_text = String::from_utf8_lossy(&capture);
    assert!(screen_text.contains("Context trim"), "the trim line never landed");
    assert_footer_on_last_row_after_grow(&capture, grow_at, "post-trim live spinner grow");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_a_late_silent_size_poll_pins_the_footer_to_the_last_row() {
    let layout = Layout::new();
    let service = ScriptedAnthropicService::text("unused\n")
        .await
        .expect("start idle provider");
    let mut run = growing_pty(&layout, service.base_url(), 18);
    run.wait_for("directory:", TEST_TIMEOUT);

    let grow_at = run.output_len();
    run.resize_silently(60, 120).expect("grow PTY without SIGWINCH");
    // No input and no signal can wake the UI; only the once-a-second size poll
    // notices this. Wait for that complete bottom-anchored frame, not a spinner
    // frame that happened to cross the byte offset just before the resize.
    wait_for_bottom_anchored_frame(&mut run, grow_at);
    let capture = run.snapshot_output();
    let _ = run.finish();

    assert_footer_on_last_row_after_grow(&capture, grow_at, "silent size-poll grow");
}

/// The screen a capture leaves on a pane of `rows` rows.
fn settled_screen(capture: &[u8], rows: u16) -> Screen {
    let mut screen = Screen::new(usize::from(rows));
    screen.feed(capture);
    screen
}

/// The row the footer stands on — it must be the last one.
fn assert_footer_on_the_last_row(visible: &[String], scenario: &str, moment: &str) {
    let footer_row = visible
        .iter()
        .rposition(|row| row.contains("? for shortcuts"))
        .unwrap_or_else(|| panic!("{scenario} ({moment}): no footer on the settled screen: {visible:#?}"));
    assert_eq!(
        footer_row,
        visible.len() - 1,
        "{scenario} ({moment}): footer on row {} of {}, blank rows below it: {visible:#?}",
        footer_row + 1,
        visible.len(),
    );
}

/// The longest run of blank rows strictly inside `rows`.
fn longest_blank_run(rows: &[String]) -> usize {
    let mut longest = 0;
    let mut run = 0;
    for row in rows {
        if row.trim().is_empty() {
            run += 1;
            longest = longest.max(run);
        } else {
            run = 0;
        }
    }
    longest
}

/// The head sits on the floor from the first frame and the transcript fills
/// the pane from the top — like Claude Code, not codex's inline head that
/// opened at the cursor and followed the transcript down, leaving a short
/// one mid-pane with blank rows under the composer (the 2026-09-06
/// screenshot). Fresh: the boot card on the top rows, the footer on the last.
/// After three short turns the transcript is one unbroken column from the
/// top, in order, and the footer is still on the last row. The blank rows
/// between the two are the band the next turn fills.
///
/// With `ZO_E2E_DUMP=<path>` the fresh and the settled captures are written
/// to `<path>.<scenario>.{fresh,settled}.bin` before anything is asserted,
/// so a red run still yields the bytes for a terminal-emulator replay.
async fn a_fresh_session_fills_the_pane_from_the_top(rows: u16, scenario: &str) {
    let layout = Layout::new();
    let service = ScriptedAnthropicService::text("Settled.\n")
        .await
        .expect("start provider");
    let args = interactive_args();
    let mut run = PtyRun::spawn_controlling_sized(
        rows,
        120,
        &layout.cwd,
        &layout.home,
        &layout.sessions,
        &layout.state,
        service.base_url(),
        &args,
        &[],
    )
    .expect("spawn zo in PTY");
    // A fresh pane: the cursor on the first row, as the window's own pane
    // hands it over.
    run.send(b"\x1b[1;1R").expect("answer cursor position query");
    run.wait_for("? for shortcuts", TEST_TIMEOUT);
    wait_until_quiet(&run, Duration::from_millis(250), TEST_TIMEOUT);
    let fresh = run.snapshot_output();
    let dump = std::env::var("ZO_E2E_DUMP").ok();
    if let Some(path) = &dump {
        fs::write(format!("{path}.{scenario}.fresh.bin"), &fresh).expect("dump the fresh capture");
    }

    for turn in 1..=3u32 {
        let at = run.output_len();
        run.send(format!("turn {turn}\r").as_bytes()).expect("send prompt");
        run.wait_for_after("Settled.", at, TEST_TIMEOUT);
        wait_until_quiet(&run, Duration::from_millis(250), TEST_TIMEOUT);
    }
    let settled = run.snapshot_output();
    let _ = run.finish();
    if let Some(path) = &dump {
        fs::write(format!("{path}.{scenario}.settled.bin"), &settled).expect("dump the settled capture");
    }

    let visible = settled_screen(&fresh, rows).visible();
    assert_footer_on_the_last_row(&visible, scenario, "fresh");
    // The card's title row is the second or third row: row one holds the
    // harness's echoed cursor reply (the pty echoes it before raw mode), and
    // the band starts under the cursor.
    let card = visible
        .iter()
        .position(|row| row.contains(">_ zo"))
        .unwrap_or_else(|| panic!("{scenario}: no boot card on the fresh screen: {visible:#?}"));
    assert!(
        card <= 2,
        "{scenario}: the boot card is not on the top rows (title on row {}): {visible:#?}",
        card + 1
    );

    let screen = settled_screen(&settled, rows);
    let visible = screen.visible();
    assert_footer_on_the_last_row(&visible, scenario, "after three turns");
    let first_content = visible
        .iter()
        .position(|row| !row.trim().is_empty())
        .expect("something on screen");
    // Row one is the band's anchor row; on a short pane the transcript has
    // scrolled and the top rows can be a cell's two-row margin.
    assert!(
        first_content <= 2,
        "{scenario}: the transcript does not start at the top (first content on row {}): {visible:#?}",
        first_content + 1
    );
    // The six cells stand in order — on a short pane the first of them have
    // scrolled into scrollback, which is where a person finds them.
    let transcript = screen.transcript();
    let mut from = 0;
    for needle in ["turn 1", "Settled.", "turn 2", "Settled.", "turn 3", "Settled."] {
        let at = transcript[from..]
            .iter()
            .position(|row| row.contains(needle))
            .unwrap_or_else(|| panic!("{scenario}: {needle:?} is missing or out of order after transcript row {from}: {transcript:#?}"));
        from += at + 1;
    }
    let last_answer = visible
        .iter()
        .rposition(|row| row.contains("Settled."))
        .expect("the last answer is on screen");
    let void = longest_blank_run(&visible[first_content..=last_answer]);
    assert!(
        void <= 2,
        "{scenario}: a void of {void} blank rows opened inside the transcript: {visible:#?}"
    );
}

/// With `ZO_E2E_DUMP=<path>` a capture is written to `<path>.<scenario>.bin`
/// before anything is asserted — the bytes for a terminal-emulator replay on
/// the other side of the pty (`python3 -c 'import pyte'`).
fn dump_capture(capture: &[u8], scenario: &str) {
    if let Ok(path) = std::env::var("ZO_E2E_DUMP") {
        fs::write(format!("{path}.{scenario}.bin"), capture).expect("dump the capture");
    }
}

/// A background command's completion is the cell the same command draws in
/// the foreground. It used to come back as an agent card — `Failed background
/// bash`, twenty rows of `[stderr]`-tagged stack and an `[exit 1]` closer
/// (07:10 screenshot, t-3177) — while codex closes the exec cell in place.
/// Two sessions run the same failing command, once in the foreground and once
/// with `run_in_background`; the background completion must be the foreground
/// cell plus one dim word, on the screen and in the scrollback alike, and its
/// header must carry the command sentence the call announced with.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_a_background_bash_completes_as_the_foreground_ran_cell() {
    const COMMAND: &str = "sh -c 'echo x >&2; exit 1'";
    const BACKGROUND_WORD: &str = "· background";

    // The control: the same command, run in the foreground.
    let layout = Layout::new();
    let service = ScriptedAnthropicService::bash(COMMAND, "### Done\n\n- foreground settled\n")
        .await
        .expect("start foreground bash script");
    let args = interactive_args();
    let mut run = pty(&layout, service.base_url(), &args);
    run.wait_for("directory:", TEST_TIMEOUT);
    run.send(b"Run the command\r").expect("send foreground prompt");
    run.wait_for_history_row("foreground settled", TEST_TIMEOUT);
    wait_until_quiet(&run, Duration::from_millis(300), TEST_TIMEOUT);
    let capture = run.finish();
    dump_capture(&capture, "background-bash-foreground-control");
    let foreground = settled_screen(&capture, 40).transcript();
    let control = command_cell(&foreground, "• Ran sh -c")
        .unwrap_or_else(|| panic!("no foreground Ran cell: {foreground:#?}"));
    assert_eq!(control[1], "  └ x", "the control cell is not the one expected: {control:#?}");

    // The same command in the background: the start call answers at once,
    // the turn settles, and the task's completion re-enters as its own cell.
    let layout = Layout::new();
    let service = ScriptedAnthropicService::background_bash(
        COMMAND,
        "### Done\n\n- background settled\n",
    )
    .await
    .expect("start background bash script");
    let mut run = pty(&layout, service.base_url(), &args);
    run.wait_for("directory:", TEST_TIMEOUT);
    run.send(b"Run the command in the background\r").expect("send background prompt");
    run.wait_for(BACKGROUND_WORD, TEST_TIMEOUT);
    wait_until_quiet(&run, Duration::from_millis(300), TEST_TIMEOUT);
    let capture = run.finish();
    dump_capture(&capture, "background-bash");
    let screen = settled_screen(&capture, 40);

    for (name, rows) in [("visible", screen.visible()), ("transcript", screen.transcript())] {
        for word in ["[stderr]", "[exit", "background bash", "Failed"] {
            assert!(
                !rows.iter().any(|row| row.contains(word)),
                "`{word}` reached the {name} rows: {rows:#?}"
            );
        }
    }
    let transcript = screen.transcript();
    let completion_at = transcript
        .iter()
        .position(|row| row.contains(BACKGROUND_WORD))
        .unwrap_or_else(|| panic!("no completion header: {transcript:#?}"));
    assert_eq!(
        transcript[completion_at],
        format!("{} {BACKGROUND_WORD}", control[0]),
        "the header is the foreground header plus one dim word: {transcript:#?}"
    );
    let completion = command_cell(&transcript[completion_at..], "• Ran sh -c")
        .unwrap_or_else(|| panic!("no completion cell: {transcript:#?}"));
    assert!(completion.iter().any(|row| row.trim() == "x"), "original command output remains: {completion:?}");
    assert_eq!(completion.iter().filter(|row| row.contains("background task") && row.contains("exited (code 1)")).count(), 1);
    let started_at = transcript
        .iter()
        .position(|row| *row == control[0])
        .unwrap_or_else(|| panic!("the call that started the task has no cell: {transcript:#?}"));
    assert!(
        started_at < completion_at,
        "the completion stands before the call that started it: {transcript:#?}"
    );
    assert!(
        screen.visible().iter().any(|row| row.contains(BACKGROUND_WORD)),
        "the completion cell is not on the settled screen: {:#?}",
        screen.visible()
    );
}

/// The rows of one command cell: the first row at or after the start of
/// `rows` that begins with `head`, then every row until the next blank one.
fn command_cell(rows: &[String], head: &str) -> Option<Vec<String>> {
    let start = rows.iter().position(|row| row.starts_with(head))?;
    let body = rows[start + 1..]
        .iter()
        .take_while(|row| !row.trim().is_empty())
        .cloned();
    Some(std::iter::once(rows[start].clone()).chain(body).collect())
}

/// The runtime's housekeeping — `Context trim · cleared N old tool
/// result(s) …` — used to be a transcript cell (07:20 screenshot, t-3177).
/// Codex leaves no such line in history. A turn long enough for a real
/// microcompact to fire (a small compaction window, one sizeable `bash`
/// result per request, every request after the first held a moment) must
/// show the trim on the status row while the next request is out, and the
/// settled transcript must not carry a `Context trim` row.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_a_context_trim_stands_on_the_status_row_and_never_in_the_transcript() {
    const TRIM: &str = "Context trim";
    // The microcompact tier is 64% of the compaction window; the trim keeps
    // the 24 newest results (the model's own window is large) and needs at
    // least 4k tokens of older ones to clear. Sixty results of ~4.8 KiB
    // (~1.2k tokens) cross the tier near the fiftieth with some 25 older
    // results to clear, and stay under the 80% full-compaction ceiling
    // before and after.
    const WINDOW: &str = "100000";
    const COMMAND: &str = "seq -f 'payload line %g of request {n}' 1 150";
    const HOLD: Duration = Duration::from_millis(250);

    let layout = Layout::new();
    let service = ScriptedAnthropicService::many_bash(
        60,
        COMMAND,
        HOLD,
        "### Done\n\n- the ladder ran\n",
    )
    .await
    .expect("start many-bash script");
    let args = interactive_args();
    let mut run = PtyRun::spawn_with_env(
        &layout.cwd,
        &layout.home,
        &layout.sessions,
        &layout.state,
        service.base_url(),
        &args,
        &[("CLAUDE_CODE_AUTO_COMPACT_WINDOW", WINDOW)],
    )
    .expect("spawn zo in PTY");
    run.wait_for("directory:", TEST_TIMEOUT);
    run.send(b"Run the payload command many times\r").expect("send prompt");

    // The trim is painted while the next request is held: on the status row.
    let long = Duration::from_secs(60);
    run.wait_for(TRIM, long);
    let painted = String::from_utf8_lossy(&run.snapshot_output()).into_owned();
    let trim_rows: Vec<String> = plain_rows(&painted)
        .into_iter()
        .filter(|row| row.contains(TRIM))
        .collect();
    assert!(!trim_rows.is_empty(), "the trim was matched but not on a row");
    for row in &trim_rows {
        assert!(
            row.contains("Working") && row.contains("to interrupt"),
            "the trim stands somewhere other than the status row: {row:?}"
        );
    }

    run.wait_for_history_row("the ladder ran", long);
    wait_until_quiet(&run, Duration::from_millis(300), TEST_TIMEOUT);
    let capture = run.finish();
    dump_capture(&capture, "context-trim-status-row");
    let screen = settled_screen(&capture, 40);
    for (name, rows) in [("visible", screen.visible()), ("transcript", screen.transcript())] {
        assert!(
            !rows.iter().any(|row| row.contains(TRIM)),
            "the trim grew the {name} rows: {rows:#?}"
        );
    }
    assert!(
        screen.transcript().iter().filter(|row| row.starts_with("• Ran ")).count() >= 1,
        "the commands themselves are still in the transcript: {:#?}",
        screen.transcript()
    );
}

/// The capture's rows with their escape sequences stripped — what a person
/// would have seen painted, frame after frame, not the settled screen.
fn plain_rows(painted: &str) -> Vec<String> {
    let mut rows = Vec::new();
    let mut row = String::new();
    let mut chars = painted.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\u{1b}' => {
                if chars.peek() == Some(&'[') {
                    chars.next();
                    while let Some(&next) = chars.peek() {
                        chars.next();
                        if ('\u{40}'..='\u{7e}').contains(&next) {
                            break;
                        }
                    }
                } else if chars.peek() == Some(&']') {
                    while let Some(next) = chars.next() {
                        if next == '\u{07}' {
                            break;
                        }
                        if next == '\u{1b}' && chars.peek() == Some(&'\\') {
                            chars.next();
                            break;
                        }
                    }
                }
            }
            '\n' => rows.push(std::mem::take(&mut row)),
            '\r' => {}
            ch if ch.is_control() => {}
            ch => row.push(ch),
        }
    }
    if !row.is_empty() {
        rows.push(row);
    }
    rows
}

/// Two files written and a `bash` announced in one batch. zo announces the
/// whole batch before any result, and the command's announce used to take
/// the viewport's live cell from the writes while they ran, committing them
/// as `✘ Failed to apply patch` with a bare `✘ path` per file — the lotto
/// session's four edits, every result `is_error: false` (2026-09-07 23:43,
/// t-3063). The screen must read `Edited 2 files`, then the command.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_writes_announced_beside_a_bash_are_edited_files_never_a_failed_patch() {
    let layout = Layout::new();
    let service = ScriptedAnthropicService::writes_then_bash(
        vec![
            ("alpha.txt".to_string(), "alpha one\nalpha two\n".to_string()),
            ("beta.txt".to_string(), "beta one\n".to_string()),
        ],
        Some("printf 'command ran'"),
        Duration::ZERO,
        "### Done\n\n- both files written\n",
    )
    .await
    .expect("start writes/bash script");
    let args = interactive_args();
    let mut run = pty(&layout, service.base_url(), &args);
    run.wait_for("directory:", TEST_TIMEOUT);
    run.send(b"Write two files, then run a command\r").expect("send prompt");
    run.wait_for_history_row("both files written", TEST_TIMEOUT);
    wait_until_quiet(&run, Duration::from_millis(300), TEST_TIMEOUT);
    let capture = run.finish();
    dump_capture(&capture, "writes-then-bash");

    let transcript = settled_screen(&capture, 40).transcript();
    assert!(
        !transcript.iter().any(|row| row.contains("Failed to apply patch") || row.contains('✘')),
        "a successful write was drawn as a failure: {transcript:#?}"
    );
    let edited = transcript
        .iter()
        .position(|row| row.contains("Edited 2 files"))
        .unwrap_or_else(|| panic!("the writes are not one edited group: {transcript:#?}"));
    let ran = transcript
        .iter()
        .position(|row| row.contains("Ran printf"))
        .unwrap_or_else(|| panic!("the command that followed is missing: {transcript:#?}"));
    assert!(edited < ran, "the command stands before the writes it followed: {transcript:#?}");
    assert_eq!(
        fs::read_to_string(layout.cwd.join("alpha.txt")).expect("alpha.txt was written"),
        "alpha one\nalpha two\n"
    );
}

/// Five files of 22 lines written in one batch on a 24-row pane. The finished
/// `Edited 5 files` cell with its five diffs is taller than the pane, and it
/// pushed the composer off the bottom while it stood live (2026-09-07 23:35
/// screenshot, t-3063). The model is held for a while before its answer so
/// the cell stays live: the footer must be on the last row, the composer on
/// screen, and the cell must say how many of its lines it does not show.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_a_tall_edit_cell_keeps_the_composer_on_the_floor() {
    const ROWS: u16 = 24;
    let layout = Layout::new();
    let files: Vec<(String, String)> = (1..=5)
        .map(|file| {
            use std::fmt::Write as _;
            let content = (1..=22).fold(String::new(), |mut content, line| {
                let _ = writeln!(content, "line {line} of file {file}");
                content
            });
            (format!("file{file}.txt"), content)
        })
        .collect();
    let service = ScriptedAnthropicService::writes_then_bash(
        files,
        None,
        Duration::from_secs(4),
        "### Done\n\n- five files written\n",
    )
    .await
    .expect("start tall writes script");
    let args = interactive_args();
    let mut run = PtyRun::spawn_sized(
        ROWS,
        120,
        &layout.cwd,
        &layout.home,
        &layout.sessions,
        &layout.state,
        service.base_url(),
        &args,
        &[],
    )
    .expect("spawn zo in a short PTY");
    run.wait_for("directory:", TEST_TIMEOUT);
    run.send(b"Write five files\r").expect("send prompt");
    run.wait_until(0, TEST_TIMEOUT, |bytes| {
        settled_screen(bytes, ROWS)
            .visible()
            .iter()
            .any(|row| row.contains("Edited 5 files"))
    });
    wait_until_quiet(&run, Duration::from_millis(250), TEST_TIMEOUT);
    let live = run.snapshot_output();
    dump_capture(&live, "tall-cell-live");
    let visible = settled_screen(&live, ROWS).visible();
    assert_footer_on_the_last_row(&visible, "tall-cell", "live");
    assert!(
        visible.iter().any(|row| row.contains("Ask zo to do anything")),
        "tall-cell (live): the composer is off the pane: {visible:#?}"
    );
    assert!(
        visible.iter().any(|row| row.contains("Edited 5 files")),
        "tall-cell (live): the cell's header is not on screen: {visible:#?}"
    );
    let folded = visible
        .iter()
        .position(|row| row.trim_start().starts_with("… ") && row.contains("more lines"))
        .unwrap_or_else(|| panic!("tall-cell (live): the cell does not say what it hides: {visible:#?}"));
    let composer = visible
        .iter()
        .position(|row| row.contains("Ask zo to do anything"))
        .expect("composer row");
    assert!(folded < composer, "the fold marker stands under the cell, above the composer: {visible:#?}");

    run.wait_for_history_row("five files written", TEST_TIMEOUT);
    wait_until_quiet(&run, Duration::from_millis(300), TEST_TIMEOUT);
    let settled = run.snapshot_output();
    let _ = run.finish();
    dump_capture(&settled, "tall-cell-settled");
    let screen = settled_screen(&settled, ROWS);
    assert_footer_on_the_last_row(&screen.visible(), "tall-cell", "settled");
    let transcript = screen.transcript();
    for file in 1..=5 {
        assert!(
            transcript.iter().any(|row| row.contains(&format!("file{file}.txt"))),
            "file{file}.txt is missing from the committed transcript: {transcript:#?}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_a_fresh_session_at_sixty_rows_keeps_the_composer_on_the_floor() {
    a_fresh_session_fills_the_pane_from_the_top(60, "sixty-rows").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_a_fresh_session_at_twenty_four_rows_keeps_the_composer_on_the_floor() {
    a_fresh_session_fills_the_pane_from_the_top(24, "twenty-four-rows").await;
}

fn longest_blank_run_after_content(rows: &[bool]) -> usize {
    let Some(first_content) = rows.iter().position(|occupied| *occupied) else {
        return rows.len();
    };
    let mut longest = 0;
    let mut current = 0;
    for occupied in &rows[first_content..] {
        if *occupied {
            current = 0;
        } else {
            current += 1;
            longest = longest.max(current);
        }
    }
    longest
}

/// SIGTERM — a pane closing, a supervisor giving up, a person's `kill` —
/// ends the session the way exit does: the process leaves promptly, and the
/// terminal is handed back with bracketed paste off and the cursor shown,
/// not abandoned in raw mode ("cc처럼 완벽하게 종료되야함", 2026-09-02).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_sigterm_ends_the_session_and_hands_the_terminal_back() {
    let layout = Layout::new();
    let service = ScriptedAnthropicService::text("unused\n")
        .await
        .expect("start provider");
    let args = interactive_args();
    let mut run = PtyRun::spawn_with_env(
        &layout.cwd,
        &layout.home,
        &layout.sessions,
        &layout.state,
        service.base_url(),
        &args,
        &[],
    )
    .expect("spawn zo in PTY");
    run.send(b"\x1b[35;1R").expect("answer cursor position query");
    run.wait_for("? for shortcuts", TEST_TIMEOUT);
    let signalled_at = run.output_len();
    let pid = nix::unistd::Pid::from_raw(i32::try_from(run.pid()).expect("pid fits"));
    nix::sys::signal::kill(pid, nix::sys::signal::Signal::SIGTERM).expect("send SIGTERM");
    let started = std::time::Instant::now();
    let capture = run.finish();
    assert!(
        started.elapsed() < TEST_TIMEOUT,
        "zo did not leave within the timeout after SIGTERM"
    );
    let after = &capture[signalled_at.min(capture.len())..];
    assert!(
        after.windows(8).any(|w| w == b"\x1b[?2004l"),
        "bracketed paste was not switched off on the way out: {}",
        String::from_utf8_lossy(after)
    );
    assert!(
        after.windows(6).any(|w| w == b"\x1b[?25h"),
        "the cursor was not shown on the way out: {}",
        String::from_utf8_lossy(after)
    );
}

/// (e) t-3054: a session whose selected model the discovery list no longer
/// carries still offers it in `/model` — dimmed, with the reason and the
/// local clock of the first refresh that missed it — instead of the row
/// vanishing under the person ("astra 모델이 갑자기 안 보임", 09-07 23:20).
///
/// The discovery cache is the one the connection rule would have written:
/// the OpenAI row for `gpt-6-astra` stamped `unlisted_since`. Discovery
/// itself stays off (the harness pins `ZO_DISABLE_MODEL_DISCOVERY`), so the
/// picker shows exactly what the cache says.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_model_picker_keeps_the_selected_model_its_source_stopped_listing() {
    let layout = Layout::new();
    let service = ScriptedAnthropicService::text("unused\n")
        .await
        .expect("start picker provider");
    // 2026-09-07T14:25:08Z — 23:25 in Seoul, 14:25 in London; the clock the
    // row shows is the local one, so only the date-and-reason prefix is pinned.
    let unlisted_since = 1_788_791_108_u64;
    let cache_dir = layout.home.join("cache").join("model-catalog");
    fs::create_dir_all(&cache_dir).expect("discovery cache dir");
    fs::write(
        cache_dir.join("discovered.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "fetched_at": unlisted_since + 3600,
            "reports": [{
                "provider": "openai",
                "source": "chatgpt-backend",
                "ok": true,
                "detail": "1 model(s)",
                "count": 1,
                "fetched_at": unlisted_since + 3600,
                "origin": "live"
            }],
            "models": [
                {
                    "provider": "openai",
                    "id": "gpt-6-astra",
                    "display_name": "GPT-6-Astra",
                    "class": "frontier",
                    "context_window": 400_000,
                    "effort_levels": ["low", "medium", "high", "xhigh", "max", "ultra"],
                    "prominence": 1,
                    "source": "chatgpt-backend",
                    "unlisted_since": unlisted_since
                },
                {
                    "provider": "openai",
                    "id": "gpt-5.6-sol",
                    "display_name": "GPT-5.6-Sol",
                    "prominence": 2,
                    "source": "chatgpt-backend"
                }
            ]
        }))
        .expect("serialize the discovery cache"),
    )
    .expect("write the discovery cache");
    let args = ["--permission-mode", "danger-full-access", "--model", "gpt-6-astra"];
    let mut run = PtyRun::spawn_with_env(
        &layout.cwd,
        &layout.home,
        &layout.sessions,
        &layout.state,
        service.base_url(),
        &args,
        &[("OPENAI_API_KEY", "openai-picker-test-key")],
    )
    .expect("spawn zo in PTY");

    run.wait_for("directory:", TEST_TIMEOUT);
    let picker_at = run.output_len();
    run.send(b"/model\r").expect("open model picker");
    run.wait_for_after("Select Model and Effort", picker_at, TEST_TIMEOUT);
    run.wait_for_after("gpt-6-astra (current)", picker_at, TEST_TIMEOUT);
    run.wait_for_after("공급자 목록에서 빠짐 · 09-0", picker_at, TEST_TIMEOUT);
    run.wait_for_after("Press enter to confirm or esc to go back", picker_at, TEST_TIMEOUT);
    run.send(b"\x1b").expect("close model picker");
    run.wait_for_after("Ask zo to do anything", picker_at, TEST_TIMEOUT);
    let capture = run.finish();

    // The picker paints one row per cursor move (`ESC[<row>;1H`) and closes
    // its styling with `ESC[0m`; that pair bounds the row's own bytes.
    let screen = String::from_utf8_lossy(&capture[picker_at..]);
    let label_at = screen.find("gpt-6-astra (current)").expect("the unlisted row is offered");
    let row_start = screen[..label_at].rfind(";1H").map_or(0, |at| at + 3);
    let row_end = screen[label_at..].find("\x1b[0m").map_or(screen.len(), |at| label_at + at);
    let row = &screen[row_start..row_end];
    assert!(
        row.contains("공급자 목록에서 빠짐"),
        "the reason sits on the row itself: {row:?}"
    );
    assert!(
        row.contains("\x1b[2m") && !row.contains("\x1b[1m"),
        "the selected unlisted row is dim, not bold: {row:?}"
    );
    assert_eq!(service.request_bodies().await.len(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_model_picker_does_not_leave_a_vertical_void_after_it_closes() {
    let layout = Layout::new();
    let service = ScriptedAnthropicService::text("unused\n")
        .await
        .expect("start picker provider");
    let args = interactive_args();
    let mut run = PtyRun::spawn_with_env(
        &layout.cwd,
        &layout.home,
        &layout.sessions,
        &layout.state,
        service.base_url(),
        &args,
        &[
            ("OPENAI_API_KEY", "openai-picker-test-key"),
            ("GOOGLE_API_KEY", "google-picker-test-key"),
            ("XAI_API_KEY", "xai-picker-test-key"),
            ("OLLAMA_BASE_URL", "http://127.0.0.1:11434"),
        ],
    )
    .expect("spawn zo in PTY");
    // A real terminal answers crossterm's cursor-position query. Start the
    // inline viewport five rows from the bottom, as a busy pane does, instead
    // of exercising the harness's two-second no-terminal fallback at row zero.
    run.send(b"\x1b[35;1R").expect("answer cursor position query");

    run.wait_for("? for shortcuts", TEST_TIMEOUT);
    let before = run.snapshot_output();
    let before_blank = longest_blank_run_after_content(&visible_row_occupancy(&before, 40));

    let picker_at = run.output_len();
    run.send(b"/model\r").expect("open model picker");
    run.wait_for_after("Press enter to confirm or esc to go back", picker_at, TEST_TIMEOUT);
    let close_at = run.output_len();
    run.send(b"\x1b").expect("close model picker");
    run.wait_for_after("? for shortcuts", close_at, TEST_TIMEOUT);
    let after = run.snapshot_output();
    let after_rows = visible_row_occupancy(&after, 40);
    let after_blank = longest_blank_run_after_content(&after_rows);
    let _ = run.finish();
    eprintln!("model picker blank-run rows: before={before_blank}, after={after_blank}");
    // The raw bytes, for replaying through a terminal emulator when the
    // occupancy alone does not say what went where.
    if let Ok(path) = std::env::var("ZO_E2E_DUMP") {
        fs::write(&path, &after).expect("dump the capture");
    }

    assert!(
        before_blank <= MAX_INLINE_BLANK_ROWS && after_blank <= MAX_INLINE_BLANK_ROWS,
        "the 40-row PTY grew a vertical void: before={before_blank}, after={after_blank}, \
         occupied-after={after_rows:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_continue_replays_two_completed_turns_from_the_same_session() {
    let layout = Layout::new();
    let service = MockAnthropicService::spawn()
        .await
        .expect("start shared mock service");
    let args = interactive_args();

    let mut first = pty(&layout, &service.base_url(), &args);
    first.wait_for("directory:", TEST_TIMEOUT);
    let first_turn_start = first.output_len();
    first
        .send(b"PARITY_SCENARIO:streaming_text first deterministic turn\r")
        .expect("send first turn");
    first.wait_for("Mock streaming says hello from the parity harness.", TEST_TIMEOUT);
    // The answer and composer can arrive in the same PTY read. Taking a
    // fresh offset after observing the answer would discard that prompt.
    first.wait_for_after("Ask zo to do anything", first_turn_start, TEST_TIMEOUT);
    let second_turn_start = first.output_len();
    first
        .send(b"PARITY_SCENARIO:streaming_text second deterministic turn\r")
        .expect("send second turn");
    first.wait_for_after(
        "Mock streaming says hello from the parity harness.",
        second_turn_start,
        TEST_TIMEOUT,
    );
    let _first_capture = first.finish();

    let resumed_args = [
        CONTINUE_ARGS[0],
        CONTINUE_ARGS[1],
        "danger-full-access",
    ];
    let mut resumed = pty(&layout, &service.base_url(), &resumed_args);
    resumed.wait_for("first deterministic turn", TEST_TIMEOUT);
    resumed.wait_for("second deterministic turn", TEST_TIMEOUT);
    resumed.wait_for_history_row("Mock streaming says hello from the parity harness.", TEST_TIMEOUT);
    let capture = resumed.finish();

    assert_history_golden(&capture, "continue", "continue replay", &layout);
    let requests = service.captured_requests().await;
    assert_mock_http_contract(&requests, 2);
    assert!(requests.iter().all(|request| request.scenario == "streaming_text"));
}

/// r24b — a session killed **between** a `tool_use` and its result must come
/// back on a request the Anthropic API would accept.
///
/// The window restores its panes with `zo --resume <id>` after a
/// restart, and a restart during a long autonomous run lands wherever the turn
/// happened to be. The worst landing spot is this one: the assistant's
/// `tool_use` is already appended to the transcript (zo persists it *before* it
/// runs the tool, so the record survives even a SIGKILL) while the result never
/// existed. Replay that verbatim and every later request carries a `tool_use`
/// with no `tool_result` — `400 invalid_request_error`, forever, because the
/// offending pair is in history. A night's work bricked by one restart.
///
/// The mock accepts anything, so the assertion is on the BODY, checked against
/// the API's own rules by [`e2e::contract`]; no real model is called, and no
/// real 400 needs to be provoked to know the body would earn one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_resume_after_a_tool_turn_died_mid_flight_sends_a_valid_request() {
    let layout = Layout::new();
    let service = ScriptedAnthropicService::parked_tool(
        PARKED_TOOL_SECONDS,
        "### Resumed\n\n- the session picked up after the kill\n",
    )
    .await
    .expect("start parked-tool script");
    let args = interactive_args();

    let mut doomed = pty(&layout, service.base_url(), &args);
    doomed.wait_for("directory:", TEST_TIMEOUT);
    doomed
        .send(b"Run the long command and tell me when it lands\r")
        .expect("send the doomed tool prompt");
    // The window restart lands *here*: the tool_use is on disk, the tool is
    // still running, and nothing will ever write its result.
    let transcript = wait_for_transcript(&layout, "toolu_parked_e2e", TEST_TIMEOUT);
    doomed.kill9();
    let _ = doomed.finish();

    let killed = fs::read_to_string(&transcript).expect("read the killed transcript");
    assert!(
        killed.contains("toolu_parked_e2e"),
        "the killed transcript should keep the tool_use it had already settled",
    );
    assert!(
        !killed.contains("tool_result"),
        "the killed transcript must NOT have a result — that is the whole scenario:\n{killed}",
    );

    let session_id = session_id_of(&transcript);
    let resumed_args = [
        "--resume",
        session_id.as_str(),
        "--permission-mode",
        "danger-full-access",
    ];
    let mut resumed = pty(&layout, service.base_url(), &resumed_args);
    resumed.wait_for("Run the long command", TEST_TIMEOUT);
    let before_prompt = resumed.output_len();
    resumed
        .send(b"Never mind the command; just say you are back\r")
        .expect("send the prompt that has to survive");
    resumed.wait_for_after("picked up after the kill", before_prompt, TEST_TIMEOUT);
    let resumed_capture = resumed.finish();
    if std::env::var_os("ZO_E2E_DUMP_SHAPE").is_some() {
        for row in history_rows_raw(&resumed_capture) {
            eprintln!("[screen] {}", strip_ansi(&String::from_utf8_lossy(&row)));
        }
    }

    let bodies = service.request_bodies().await;
    assert_eq!(bodies.len(), 2, "one doomed turn, one resumed turn");
    assert_wire_contract(&bodies[1], "the first request after a mid-tool death");
    assert!(
        bodies[1].contains("toolu_parked_e2e"),
        "the resumed request should still carry the interrupted call, sealed — not silently dropped:\n{}",
        contract::message_shape(&bodies[1]),
    );
}

/// r24b variant — the same restart, but during the assistant's *text*.
///
/// zo appends an assistant message only once the stream settles, so a death
/// here loses the partial answer entirely and leaves the user's prompt as the
/// last record. That is a different shape from the tool case (nothing to pair)
/// and it has its own way to earn a 400: the resumed turn appends a second user
/// message right behind the first.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_resume_after_the_answer_died_mid_stream_sends_a_valid_request() {
    let layout = Layout::new();
    let service = ScriptedAnthropicService::dribbled_text(
        dribbled_answer(),
        DRIBBLE_PIECES,
        Duration::from_millis(DRIBBLE_GAP_MS),
    )
    .await
    .expect("start dribbled-text script");
    let args = interactive_args();

    let mut doomed = pty(&layout, service.base_url(), &args);
    doomed.wait_for("directory:", TEST_TIMEOUT);
    doomed
        .send(b"Answer at length so the restart can catch you talking\r")
        .expect("send the doomed text prompt");
    doomed.wait_for(DRIBBLE_HEAD, TEST_TIMEOUT);
    doomed.kill9();
    let capture = doomed.finish();
    assert!(
        find(&capture, DRIBBLE_TAIL.as_bytes()).is_none(),
        "the kill was too late — the whole answer had already landed",
    );

    let transcript = wait_for_transcript(&layout, "Answer at length", TEST_TIMEOUT);
    let killed = fs::read_to_string(&transcript).expect("read the killed transcript");
    let partial_text_persisted = killed.contains(DRIBBLE_HEAD);

    let session_id = session_id_of(&transcript);
    let resumed_args = [
        "--resume",
        session_id.as_str(),
        "--permission-mode",
        "danger-full-access",
    ];
    let mut resumed = pty(&layout, service.base_url(), &resumed_args);
    resumed.wait_for("Answer at length", TEST_TIMEOUT);
    let before_prompt = resumed.output_len();
    resumed
        .send(b"Say it again, shorter\r")
        .expect("send the prompt that has to survive");
    resumed.wait_for_after(DRIBBLE_TAIL, before_prompt, TEST_TIMEOUT);
    let _ = resumed.finish();

    let bodies = service.request_bodies().await;
    assert_eq!(bodies.len(), 2, "one doomed turn, one resumed turn");
    assert_wire_contract(
        &bodies[1],
        &format!(
            "the first request after a mid-stream death (partial text persisted: {partial_text_persisted})"
        ),
    );
}

/// `--continue` must ride the same recovery as `--resume <id>`.
///
/// The window uses the explicit id, but a person at a terminal reaches for
/// `--continue`, and the two resolve the session through different code. If
/// only one of them seals the interrupted call, half the users of a killed
/// session still get the 400.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_continue_after_a_tool_turn_died_mid_flight_sends_a_valid_request() {
    let layout = Layout::new();
    let service = ScriptedAnthropicService::parked_tool(
        PARKED_TOOL_SECONDS,
        "### Continued\n\n- the newest session picked up after the kill\n",
    )
    .await
    .expect("start parked-tool script");
    let args = interactive_args();

    let mut doomed = pty(&layout, service.base_url(), &args);
    doomed.wait_for("directory:", TEST_TIMEOUT);
    doomed
        .send(b"Run the long command and tell me when it lands\r")
        .expect("send the doomed tool prompt");
    wait_for_transcript(&layout, "toolu_parked_e2e", TEST_TIMEOUT);
    doomed.kill9();
    let _ = doomed.finish();

    let resumed_args = [CONTINUE_ARGS[0], CONTINUE_ARGS[1], "danger-full-access"];
    let mut resumed = pty(&layout, service.base_url(), &resumed_args);
    resumed.wait_for("Run the long command", TEST_TIMEOUT);
    let before_prompt = resumed.output_len();
    resumed
        .send(b"Never mind the command; just say you are back\r")
        .expect("send the prompt that has to survive");
    resumed.wait_for_after("picked up after the kill", before_prompt, TEST_TIMEOUT);
    let _ = resumed.finish();

    let bodies = service.request_bodies().await;
    assert_eq!(bodies.len(), 2, "one doomed turn, one continued turn");
    assert_wire_contract(&bodies[1], "the first request after `--continue`");
}

/// r26 — the compaction SUMMARY request is a second door to the provider, and
/// a session killed mid-tool has to survive it too.
///
/// r24b proved the live turn is sealed: `Session::tool_consistent_messages`
/// answers the orphan `tool_use` a SIGKILL left behind, so ordinary requests
/// stay legal forever. The transcript is append-only, though, so the orphan
/// itself is permanent — and the compaction summary round-trip builds its body
/// from `CompactionPlan` slices, not from that sealed view. Before r26 it
/// carried the orphan straight out: an unattended run answered turn after turn
/// and then bricked at its first FULL compaction, hours later, with `400
/// tool_use ids were found without tool_result blocks`.
///
/// Driven at the real seam and read off the real wire: a killed tool turn, a
/// resume, a `/compact`, and the recorded summary BODY checked by the same
/// `e2e::contract` rules the r24b cases use. `/compact` rather than a shrunken
/// auto-compaction window because it fires on a threshold of its own
/// (`CompactionConfig::default`, 10k estimated tokens) — so exactly one summary
/// request exists to read, and the auto ladder is not part of the question.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_compaction_after_a_tool_turn_died_sends_a_valid_summary_request() {
    let layout = Layout::new();
    let service = e2e::measure::MeasureService::start(e2e::measure::Plan {
        payload_lines: R26_PAYLOAD_LINES,
        // Long enough that "kill it while the tool runs" is not a race.
        child_sleep_secs: R26_PARKED_SECONDS,
        ..e2e::measure::Plan::default()
    })
    .await
    .expect("start r26 provider");
    let session_id = r26_kill_a_session_mid_tool(&layout, service.base_url());
    r26_resume_and_compact(&layout, service.base_url(), &session_id);

    let recorded = service.recorded().await;
    let summaries: Vec<&e2e::measure::Recorded> = recorded
        .iter()
        .filter(|row| row.compaction_summary)
        .collect();
    assert_eq!(
        summaries.len(),
        1,
        "exactly one summary round-trip: /compact fired once and the auto ladder \
         (default window) never did — got {} of {} requests",
        summaries.len(),
        recorded.len(),
    );
    let summary = summaries[0];

    // What the seal costs P5. The live request already carried it (r24b), so an
    // unsealed summary request diverged from the cached prefix at the orphan;
    // sealing both sides is what puts the shared prefix back. Printed, not
    // asserted: a mock invents its own cache accounting, but the SERIALIZED
    // prefix the provider matches on is real in every captured body.
    if let Some(live) = recorded
        .iter()
        .rev()
        .find(|row| !row.compaction_summary && row.at_ms < summary.at_ms)
    {
        let before = live.messages_json();
        let during = summary.messages_json();
        eprintln!(
            "[r26] summary request: {}B/{} messages · live request before it: {}B/{} messages \
             · shared serialized prefix {}B",
            during.len(),
            summary.message_count(),
            before.len(),
            live.message_count(),
            e2e::measure::shared_prefix_len(&before, &during),
        );
    }
    // The pin. Before the seal this body carried `toolu_r23_sleep` with nothing
    // answering it — the 400 the real API would have returned.
    assert_wire_contract(&summary.body, "the compaction summary request");
    assert!(
        summary.body.contains("toolu_r23_sleep"),
        "the interrupted call must reach the summarizer SEALED, not dropped — a \
         summary that never heard of it is a worse repair:\n{}",
        contract::message_shape(&summary.body),
    );

    if std::env::var_os("ZO_E2E_DUMP_SHAPE").is_some() {
        eprintln!(
            "[r26] summary body shape:\n{}",
            contract::message_shape(&summary.body)
        );
    }
}

/// r26 phase one — grow a session, then SIGKILL it between a `tool_use` and its
/// result. Returns the id `--resume` takes.
///
/// Asserts its own premises before handing the id on: a scenario that failed to
/// leave an orphan would make every later assertion vacuous.
fn r26_kill_a_session_mid_tool(layout: &Layout, base_url: &str) -> String {
    let args = interactive_args();
    let mut doomed = pty(layout, base_url, &args);
    doomed.wait_for("directory:", TEST_TIMEOUT);
    // `measured_turn` waits for the turn AND for the screen to go quiet: a
    // prompt sent before that lands as a steer inside the running turn instead
    // of starting a new one, and the scenario needs a turn of its own to kill.
    for turn in 0..R26_TURNS_BEFORE_THE_KILL {
        measured_turn(
            &mut doomed,
            &format!("R23_BIG emit the payload ({turn})"),
            R26_TURN_TIMEOUT,
        );
    }
    // The restart lands HERE: the `tool_use` is on disk, the tool is still
    // sleeping, and nothing will ever write its result.
    doomed
        .send(b"R23_SLEEP while the window restarts\r")
        .expect("send the doomed tool prompt");
    let transcript = wait_for_transcript(layout, "toolu_r23_sleep", R26_TURN_TIMEOUT);
    doomed.kill9();
    let _ = doomed.finish();

    let killed = fs::read_to_string(&transcript).expect("read the killed transcript");
    assert!(
        killed.contains("toolu_r23_sleep"),
        "premise: the killed transcript keeps the tool_use it had already settled",
    );
    assert!(
        !killed
            .lines()
            .any(|line| line.contains("toolu_r23_sleep") && line.contains("tool_result")),
        "premise: the parked call has no result — that is the whole scenario:\n{killed:.4000}",
    );
    session_id_of(&transcript)
}

/// r26 phase two — resume the killed session, push the orphan out of the
/// preserved tail, and run `/compact`.
fn r26_resume_and_compact(layout: &Layout, base_url: &str, session_id: &str) {
    let resumed_args = [
        "--resume",
        session_id,
        "--permission-mode",
        "danger-full-access",
    ];
    let mut resumed = pty(layout, base_url, &resumed_args);
    resumed.wait_for("R23_BIG emit the payload (0)", R26_TURN_TIMEOUT);
    // More full turns, so the orphan is no longer inside the preserved tail
    // (`CompactionConfig::default` keeps the last four messages) and is
    // actually part of what gets summarized.
    for turn in 0..R26_TURNS_AFTER_THE_RESUME {
        measured_turn(
            &mut resumed,
            &format!("R23_BIG emit the payload again ({turn})"),
            R26_TURN_TIMEOUT,
        );
    }

    let before_compact = resumed.output_len();
    resumed.send(b"/compact\r").expect("run /compact");
    resumed.wait_for_after("compact:", before_compact, R26_TURN_TIMEOUT);
    let capture = resumed.finish();
    let screen = strip_ansi(&String::from_utf8_lossy(&capture));
    assert!(
        !screen.contains("compact failed"),
        "the compaction itself must finish: {}",
        screen
            .lines()
            .filter(|line| line.contains("compact"))
            .collect::<Vec<_>>()
            .join(" / "),
    );
}

/// Lines of tool output per `R23_BIG` turn.
///
/// Well past the 160 lines the wire compressor keeps of a bounded tool output
/// (`context_compression`: 120 head + 40 tail), so each turn contributes the
/// most a turn can — measured at ~17 kB of messages, ~4.3k estimated tokens.
const R26_PAYLOAD_LINES: usize = 400;
/// How many big turns land before and after the kill.
///
/// Four of them put the session past the 10k estimated tokens
/// `CompactionConfig::default` gates on, so `/compact` really summarizes
/// instead of answering "nothing to compact" — which would leave the test
/// green while measuring nothing. Split across the kill so the orphan sits in
/// the middle: past the four-message preserved tail (or it would never be
/// summarized) and past the head (or the shape would be degenerate).
const R26_TURNS_BEFORE_THE_KILL: usize = 2;
const R26_TURNS_AFTER_THE_RESUME: usize = 2;
/// Seconds the doomed tool parks for — long enough that the kill is not a race
/// on a fast machine, short enough that the stray `sleep` is gone before the
/// suite ends.
const R26_PARKED_SECONDS: u64 = 20;
/// A `R23_BIG` turn writes 400 lines through a real `bash` and streams them
/// back; the boot + replay of a resumed session is slower still. The shared
/// 10s [`TEST_TIMEOUT`] is for screens, not for these.
const R26_TURN_TIMEOUT: Duration = Duration::from_secs(60);

/// Seconds the doomed tool parks for. Long enough that "kill it while the tool
/// runs" is not a race against a fast machine; short enough that the orphaned
/// `sleep` the kill leaves behind is gone before the suite ends.
const PARKED_TOOL_SECONDS: u32 = 12;
const DRIBBLE_PIECES: usize = 10;
/// Wide enough that "the head is on screen but the tail is not" survives a
/// loaded machine: the whole answer takes ~2.2s to arrive, so the kill has
/// most of a second of slack rather than a few frames of it.
const DRIBBLE_GAP_MS: u64 = 250;
const DRIBBLE_HEAD: &str = "opening fragment of the dying answer";
const DRIBBLE_TAIL: &str = "closing fragment nobody was supposed to reach";

/// A long answer whose first and last fragments are separately greppable, so a
/// test can say "the head arrived, the tail did not" without guessing at
/// timing.
fn dribbled_answer() -> String {
    let filler = (0..24)
        .map(|index| format!("- middle line {index} of the dying answer"))
        .collect::<Vec<_>>()
        .join("\n");
    format!("{DRIBBLE_HEAD}\n\n{filler}\n\n{DRIBBLE_TAIL}\n")
}

/// Fail with the body's shape when a request breaks the Anthropic contract.
///
/// The pairing rules are the pin. Role alternation is *reported* rather than
/// asserted: a killed session can leave two user messages in a row, and whether
/// that is worth changing is a separate question from whether the interrupted
/// tool call was sealed.
fn assert_wire_contract(body: &str, scenario: &str) {
    let found = contract::violations(body);
    let (fatal, reported): (Vec<_>, Vec<_>) = found
        .into_iter()
        .partition(|violation| violation.rule != Rule::RoleAlternation);
    for violation in &reported {
        eprintln!("[contract] {scenario}: {violation}");
    }
    // The measurement wants the shape even when the body is legal — "it passes"
    // and "here is what passed" are different findings.
    if std::env::var_os("ZO_E2E_DUMP_SHAPE").is_some() {
        eprintln!(
            "[contract] {scenario} shape:\n{}",
            contract::message_shape(body)
        );
    }
    assert!(
        fatal.is_empty(),
        "{scenario} breaks the Anthropic message contract:\n{}\nshape:\n{}",
        fatal
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n"),
        contract::message_shape(body),
    );
}

/// Wait until some session transcript under the private session root contains
/// `needle`, and return its path.
///
/// Polling the file rather than the screen is what makes the kill deterministic:
/// the question is whether the record is DURABLE, and only the file can answer
/// that.
fn wait_for_transcript(layout: &Layout, needle: &str, timeout: Duration) -> PathBuf {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        for path in transcripts(&layout.sessions) {
            if fs::read_to_string(&path).is_ok_and(|body| body.contains(needle)) {
                return path;
            }
        }
        assert!(
            std::time::Instant::now() < deadline,
            "no session transcript under {} ever contained {needle:?}",
            layout.sessions.display(),
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Every `.jsonl` transcript under the session root, whatever nesting the
/// registry chose.
fn transcripts(root: &std::path::Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let Ok(entries) = fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|extension| extension == "jsonl") {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

/// The id `--resume` takes, read back from the transcript's file name.
fn session_id_of(transcript: &std::path::Path) -> String {
    transcript
        .file_stem()
        .and_then(|stem| stem.to_str())
        .expect("transcript file name is UTF-8")
        .to_string()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_pipe_mode_keeps_the_append_only_exec_contract() {
    let layout = Layout::new();
    let service = MockAnthropicService::spawn()
        .await
        .expect("start pipe mock service");
    let output = run_pipe(
        &layout.cwd,
        &layout.home,
        &layout.sessions,
        &layout.state,
        &service.base_url(),
        &["--plain", "--permission-mode", "danger-full-access"],
        b"PARITY_SCENARIO:streaming_text pipe deterministic turn\n/exit\n",
    )
    .expect("run zo in pipe mode");
    assert!(output.status.success(), "pipe exited with {:?}", output.status);
    assert_golden_bytes(&mask_volatile(&output.stdout, &layout), "pipe", "pipe mode");
    assert!(!output.stdout.windows(7).any(|window| window == b"?2026h"));
    let requests = service.captured_requests().await;
    assert_mock_http_contract(&requests, 1);
}

/// r49 — a lesson this project promoted comes back as a reminder on the turn
/// that needs it, and costs the cache nothing.
///
/// The claim has three halves and a mock provider can check all three, because
/// what reaches the wire is the whole question:
///
/// 1. a query that touches what a lesson is about puts a `reminder:` line in
///    the request body, and an unrelated query does not;
/// 2. the reminder rides as a transient block behind the newest user message,
///    never as a system section — the system blocks of two turns in one session
///    are byte-identical;
/// 3. so the prefix the provider caches never moves, which is the only reason
///    a per-turn reminder is affordable at all.
///
/// (2) is the half that is easy to get wrong and expensive to get wrong. The
/// system blocks sit in front of every message cache breakpoint, so a reminder
/// written into the system prompt re-bills the entire transcript on every turn
/// it changes — a regression this runtime already paid for once
/// (`ConversationRuntime::transient_reminders`).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_promoted_lesson_returns_as_a_reminder_without_moving_the_cached_prefix() {
    let layout = Layout::new();
    plant_promoted_lessons(&layout);
    let service = MockAnthropicService::spawn()
        .await
        .expect("spawn mock anthropic service");

    // Two turns of ONE session. Two processes rather than two lines down one
    // pipe: a second line arriving while the first turn is still streaming is
    // steering, and joins that turn instead of opening another. `--continue`
    // reopens the same session, and reopening replays its system prompt
    // verbatim — which is the invariant under test, so replay is the right
    // second reading of it.
    let first = run_pipe(
        &layout.cwd,
        &layout.home,
        &layout.sessions,
        &layout.state,
        &service.base_url(),
        &["--plain", "--permission-mode", "danger-full-access"],
        b"PARITY_SCENARIO:streaming_text what breaks in crates/runtime/src/memory/recall.rs\n/exit\n",
    )
    .expect("run zo in pipe mode");
    assert!(first.status.success(), "first turn exited with {:?}", first.status);
    let second = run_pipe(
        &layout.cwd,
        &layout.home,
        &layout.sessions,
        &layout.state,
        &service.base_url(),
        &[
            "--plain",
            CONTINUE_ARGS[0],
            CONTINUE_ARGS[1],
            "danger-full-access",
        ],
        b"PARITY_SCENARIO:streaming_text write a haiku about the sea\n/exit\n",
    )
    .expect("resume zo in pipe mode");
    assert!(second.status.success(), "second turn exited with {:?}", second.status);

    let requests = service.captured_requests().await;
    assert_eq!(requests.len(), 2, "one request per turn");
    let bodies: Vec<serde_json::Value> = requests
        .iter()
        .map(|request| {
            serde_json::from_str(&request.raw_body).expect("the request body is JSON")
        })
        .collect();

    // (1) the reminder is there when the turn is about the lessons, and gone
    //     when it is not.
    let reminded = body_text(&bodies[0]);
    assert!(
        reminded.contains(runtime::REMINDER_LINE_PREFIX),
        "a query touching what the lessons are about must carry a reminder:\n{reminded}"
    );
    assert!(
        reminded.contains(runtime::REMINDERS_SECTION_HEADING),
        "and the block must be the one `render_reminders` writes:\n{reminded}"
    );
    // The second request carries the whole transcript, and turn one's reminder
    // is a persisted message in it — that is the design (a reminder is absorbed
    // once and never re-billed). So the question is only about what THIS turn
    // appended: everything from its own user message onward.
    let unrelated = body_tail_after(&bodies[1], "write a haiku about the sea");
    assert!(
        !unrelated.contains(runtime::REMINDER_LINE_PREFIX),
        "an unrelated query must append no reminder:\n{unrelated}"
    );

    // The reminder never repeats an entry the recalled-memory section already
    // carries — a duplicate is the block's whole budget spent on nothing.
    for slug in reminder_slugs(&reminded) {
        assert!(
            !reminded.contains(&format!("- [{slug}](")),
            "{slug} is both recalled and reminded in one request:\n{reminded}"
        );
    }

    // The block stays inside its own ceiling, in the estimator every budget in
    // this repo is written in.
    for line in reminded.lines().filter(|line| line.contains(runtime::REMINDER_LINE_PREFIX)) {
        assert!(
            line.chars().count() / 4 < runtime::MAX_REMINDER_TOKENS,
            "one reminder line over the whole block's budget: {line}"
        );
    }
    assert!(
        reminded.matches(runtime::REMINDER_LINE_PREFIX).count() <= runtime::MAX_REMINDER_LINES,
        "at most {} lines:\n{reminded}",
        runtime::MAX_REMINDER_LINES
    );

    // (2) and (3) — the system blocks did not move between the two turns, so
    // nothing behind them was re-billed.
    assert_eq!(
        bodies[0].get("system"),
        bodies[1].get("system"),
        "a reminder must never reach the system blocks: they sit in front of \
         every message cache breakpoint, and moving them re-bills the whole \
         transcript"
    );
    let system = serde_json::to_string(bodies[0].get("system").expect("system blocks"))
        .expect("serialize system blocks");
    assert!(
        !system.contains(runtime::REMINDER_LINE_PREFIX),
        "the reminder rides as a transient block, not as a system section:\n{system}"
    );
}

/// Plant six promoted lessons in this workspace's global project memory, all
/// about one file, so a query naming that file recalls more of them than the
/// recall section renders — which is exactly the surplus the reminder block is
/// for.
fn plant_promoted_lessons(layout: &Layout) {
    use std::fmt::Write as _;

    // Canonicalized first: on macOS the temp root is `/var/...` here and
    // `/private/var/...` inside the child, and the project slug is a hash of
    // the path — plant under the wrong one and the store is simply never read.
    let cwd = fs::canonicalize(&layout.cwd).expect("canonical workspace");
    let root = runtime::memory::paths::memory_project_root(&cwd);
    let memory = layout
        .home
        .join("projects")
        .join(runtime::project_slug(&root))
        .join("memory");
    fs::create_dir_all(&memory).expect("memory store");
    let lessons = [
        ("gotcha-recall-locale-grep", "grep over crates/runtime/src/memory/recall.rs needs LC_ALL=C"),
        ("gotcha-recall-index-stale", "the recall index in crates/runtime/src/memory/recall.rs goes stale after a write"),
        ("workflow-recall-verify", "verify crates/runtime/src/memory/recall.rs with cargo test -p runtime --lib"),
        ("gotcha-recall-snippet-cap", "snippets from crates/runtime/src/memory/recall.rs are byte-capped, not line-capped"),
        ("constraint-recall-reserve", "crates/runtime/src/memory/recall.rs must keep the compaction reserve"),
        ("preference-recall-tokens", "prefer token overlap over embeddings in crates/runtime/src/memory/recall.rs"),
    ];
    let mut index = String::from("# Zo memory\n\n");
    for (slug, summary) in lessons {
        let _ = writeln!(index, "- [{slug}]({slug}.md) — {summary}");
        fs::write(
            memory.join(format!("{slug}.md")),
            format!("# {summary}\n\n{summary}. Recorded by the dreamer.\n"),
        )
        .expect("memory entry");
    }
    fs::write(memory.join("MEMORY.md"), index).expect("memory index");
}

/// Every `text` in a request body's messages, joined — where a transient
/// reminder lands.
fn body_text(body: &serde_json::Value) -> String {
    body_messages(body).join("")
}

/// What one turn appended: the messages from the one carrying `marker` onward.
fn body_tail_after(body: &serde_json::Value, marker: &str) -> String {
    let messages = body_messages(body);
    let start = messages
        .iter()
        .rposition(|message| message.contains(marker))
        .unwrap_or(0);
    messages[start..].join("")
}

fn body_messages(body: &serde_json::Value) -> Vec<String> {
    body.get("messages")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .map(|message| {
            let mut text = String::new();
            collect_text(message, &mut text);
            text
        })
        .collect()
}

fn collect_text(value: &serde_json::Value, out: &mut String) {
    match value {
        serde_json::Value::String(string) => {
            out.push_str(string);
            out.push('\n');
        }
        serde_json::Value::Array(items) => {
            for item in items {
                collect_text(item, out);
            }
        }
        serde_json::Value::Object(fields) => {
            for (key, field) in fields {
                if key == "text" || key == "content" {
                    collect_text(field, out);
                }
            }
        }
        _ => {}
    }
}

/// The slugs a rendered reminder block names, from `… (slug)` at line end.
fn reminder_slugs(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| {
            let rest = line.trim().strip_prefix(runtime::REMINDER_LINE_PREFIX)?;
            let open = rest.rfind('(')?;
            Some(rest[open + 1..].trim_end_matches(')').to_string())
        })
        .collect()
}

/// Visible text of a captured frame, for eyeballing a cell in a failure.
fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\u{1b}' {
            out.push(ch);
            continue;
        }
        if chars.peek() == Some(&'[') {
            chars.next();
            for next in chars.by_ref() {
                if next.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            chars.next();
        }
    }
    out
}

// ---------------------------------------------------------------------------
// r23 — the four things `docs/analysis/loop-prompt.md` queue C never measured:
// a real turn's CPU, a session long enough to compact more than once, several
// children at once, and the first-run LSP probe.  All four are MEASUREMENTS:
// `#[ignore]`d, they print and assert only the shape that would make the
// numbers meaningless.  `docs/analysis/perf-r23.md` carries the readings.
// ---------------------------------------------------------------------------

fn measure_env<T: std::str::FromStr>(name: &str, fallback: T) -> T {
    std::env::var(name)
        .ok()
        .and_then(|raw| raw.trim().parse().ok())
        .unwrap_or(fallback)
}

/// The repaint ruler of `docs/captures/frame-diff-r10.md`, as a function.
///
/// Returns `(bytes, committed_lines, erase_line, frames)`.  Two calls at two
/// sizes give the marginal cost; ONE call gives a ratio dominated by the turn's
/// fixed cost, which is how `frame-diff-r9.md` came to the wrong conclusion.
fn repaint_ruler(capture: &[u8]) -> (usize, usize, usize, usize) {
    let needle = b"\r\n\x1b[39;49m\x1b[K";
    (
        capture.len(),
        capture.windows(needle.len()).filter(|w| *w == needle).count(),
        capture.windows(3).filter(|w| *w == b"\x1b[K").count(),
        capture.windows(8).filter(|w| *w == b"\x1b[?2026h").count(),
    )
}

/// Send one prompt, wait for the scripted answer, and return its wall clock.
///
/// The answer text landing on screen is NOT the end of the turn: the REPL is
/// still inside it for a while afterwards, and a prompt typed in that window
/// becomes a **steer** folded into the running turn instead of a new one. That
/// is not a defect — it is the mid-turn steering path — but a measurement that
/// walks into it counts N prompts and gets far fewer turns, with the wall clock
/// of the ones that did run silently averaged over all of them.
///
/// So the driver waits for the marker (that is the turn's wall clock) and then
/// waits for the terminal to go QUIET before typing again. Quiet is a sound
/// idle test here because an idle zo frame emits zero bytes
/// (`perf-r11.md` §1) while a live turn keeps redrawing its spinner.
fn measured_turn(run: &mut PtyRun, prompt: &str, timeout: Duration) -> Duration {
    let offset = run.output_len();
    let mut line = prompt.as_bytes().to_vec();
    line.push(b'\r');
    let started = std::time::Instant::now();
    run.send(&line).expect("send measured prompt");
    run.wait_for_after(e2e::measure::TURN_SETTLED, offset, timeout);
    let elapsed = started.elapsed();
    wait_until_quiet(run, Duration::from_millis(500), timeout);
    elapsed
}

/// Block until the child has emitted nothing for `quiet_for`.
fn wait_until_quiet(run: &PtyRun, quiet_for: Duration, timeout: Duration) {
    let deadline = std::time::Instant::now() + timeout;
    let mut last = run.output_len();
    let mut since = std::time::Instant::now();
    while since.elapsed() < quiet_for {
        assert!(
            std::time::Instant::now() < deadline,
            "the session never went quiet — every prompt after this one would be a steer"
        );
        std::thread::sleep(Duration::from_millis(25));
        let now = run.output_len();
        if now != last {
            last = now;
            since = std::time::Instant::now();
        }
    }
}

/// r23 (1) — how much of a real tool turn is zo COMPUTING?
///
/// `perf-r9.md` left "CPU profile" unmeasured and `perf-r11.md` only ever
/// profiled *startup*.  The question a long autonomous session actually asks is
/// what a turn costs while it runs, and the first thing to establish is the
/// split: a turn that is 95% waiting has no frame worth optimizing, however
/// tall it looks in a profiler that counts wall clock.
///
/// `cpu / wall` from `ps` answers that without any attribution at all, so it is
/// the number this prints first.  For attribution, set `MEASURE_SAMPLE` to a
/// path and macOS `sample` runs against the same window — but note the release
/// profile sets `strip = "symbols"`, so a stripped build profiles as addresses.
/// Build with `CARGO_PROFILE_RELEASE_STRIP=none` for a readable report; the
/// stripped and unstripped binaries are otherwise identical (strip is a
/// post-link step, not a codegen change).
///
/// ```text
/// MEASURE_SECONDS=8 cargo test --release -p zo-ide --test e2e_hermetic \
///     r23_turn_cpu -- --ignored --nocapture
/// ```
#[ignore = "measurement, not a contract"]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn r23_turn_cpu() {
    let plan = e2e::measure::Plan {
        tools: measure_env("MEASURE_TOOLS", 8),
        ..e2e::measure::Plan::default()
    };
    let seconds: u64 = measure_env("MEASURE_SECONDS", 8);
    let layout = Layout::new();
    layout.fixture("r23 fixture\n");
    let service = e2e::measure::MeasureService::start(plan)
        .await
        .expect("start r23 provider");
    let args = interactive_args();
    let mut run = pty(&layout, service.base_url(), &args);
    run.wait_for("directory:", TEST_TIMEOUT);

    // One warm-up turn first: allocator arenas, the lazily built caches, and
    // the first provider connection all land on turn 1, and folding them into
    // the average is how a warm-up gets reported as a steady-state cost.
    let turn_timeout = Duration::from_secs(measure_env("MEASURE_TURN_TIMEOUT", 60_u64));
    let warm_up = measured_turn(&mut run, "R23_TOOLS warm up the turn path", turn_timeout);

    // An IDLE control first. The frame loop runs whether or not a turn does,
    // and without this reading its cost would be folded into the turn's and
    // reported as work the turn caused.
    let idle_seconds = 3_u64;
    let idle_before = run.cpu_ms().expect("the child is still alive");
    let idle_started = std::time::Instant::now();
    while idle_started.elapsed() < Duration::from_secs(idle_seconds) {
        std::thread::sleep(Duration::from_millis(50));
    }
    let idle_wall = idle_started.elapsed();
    let idle_cpu = run
        .cpu_ms()
        .expect("the child is still alive")
        .saturating_sub(idle_before);

    let sampler = std::env::var("MEASURE_SAMPLE").ok().map(|path| {
        std::process::Command::new("sample")
            .args([
                run.pid().to_string(),
                seconds.to_string(),
                "-file".to_string(),
                path.clone(),
            ])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn sample")
    });

    let cpu_before = run.cpu_ms().expect("the child is still alive");
    let wall_started = std::time::Instant::now();
    let mut turns = Vec::new();
    while wall_started.elapsed() < Duration::from_secs(seconds) {
        turns.push(measured_turn(
            &mut run,
            "R23_TOOLS run the probe set",
            turn_timeout,
        ));
    }
    let wall = wall_started.elapsed();
    let cpu_after = run.cpu_ms().expect("the child is still alive");
    let cpu = cpu_after.saturating_sub(cpu_before);

    // The turn's own wall clock is prompt → answer; the rest of the window is
    // this driver's quiet gate and belongs to neither zo nor the turn.
    let turn_wall: u128 = turns.iter().map(Duration::as_millis).sum();
    let idle_share = u128::from(idle_cpu) * wall.as_millis() / (idle_wall.as_millis().max(1));
    let turn_cpu = u128::from(cpu).saturating_sub(idle_share);
    let turns_count = turns.len().max(1) as u128;
    let fastest = turns.iter().min().copied().unwrap_or_default();
    let slowest = turns.iter().max().copied().unwrap_or_default();
    eprintln!(
        "[r23-cpu] tools={} turns={} warm_up={}ms",
        plan.tools,
        turns.len(),
        warm_up.as_millis(),
    );
    eprintln!(
        "[r23-cpu] idle control: {idle_cpu}ms cpu over {}ms wall = {}% of one core",
        idle_wall.as_millis(),
        u128::from(idle_cpu) * 100 / idle_wall.as_millis().max(1),
    );
    eprintln!(
        "[r23-cpu] window: wall={}ms (turns {turn_wall}ms + driver gate {}ms) cpu={cpu}ms \
         (idle share {idle_share}ms, turns {turn_cpu}ms)",
        wall.as_millis(),
        wall.as_millis().saturating_sub(turn_wall),
    );
    eprintln!(
        "[r23-cpu] per turn: wall={}ms cpu={}ms → zo COMPUTES {}% of a turn, waits {}%  \
         (spread {}..{}ms)",
        turn_wall / turns_count,
        turn_cpu / turns_count,
        turn_cpu * 100 / turn_wall.max(1),
        100_u128.saturating_sub(turn_cpu * 100 / turn_wall.max(1)),
        fastest.as_millis(),
        slowest.as_millis(),
    );
    if let Some(mut sampler) = sampler {
        let _ = sampler.wait();
        eprintln!("[r23-cpu] sample report written");
    }
    let capture = run.finish();
    let (bytes, lines, el, frames) = repaint_ruler(&capture);
    eprintln!("[r23-cpu] repaint bytes={bytes} committed_lines={lines} EL={el} frames={frames}");
    assert!(!turns.is_empty(), "no turn completed inside the window");
}

/// r23 (2) — a session long enough for compaction to fire more than once.
///
/// `long_session_rss` runs twelve turns, which is enough to show the resident
/// set flattening and not nearly enough to compact.  This one drives the
/// session at a deliberately small compaction window
/// (`CLAUDE_CODE_AUTO_COMPACT_WINDOW`) so the ladder — microcompact, then the
/// full summarize-and-replace round — fires repeatedly inside a test's patience.
///
/// Shrinking the WINDOW rather than the transcript is what keeps this honest:
/// every tier is a percentage of that window, so the ladder's *shape* is the
/// shipped one and only its scale moves.
///
/// ```text
/// MEASURE_TURNS=40 MEASURE_WINDOW=60000 cargo test --release -p zo-ide \
///     --test e2e_hermetic r23_long_session -- --ignored --nocapture
/// ```
#[ignore = "measurement, not a contract"]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn r23_long_session() {
    let plan = e2e::measure::Plan {
        payload_lines: measure_env("MEASURE_PAYLOAD_LINES", 90),
        ..e2e::measure::Plan::default()
    };
    let turns: usize = measure_env("MEASURE_TURNS", 40);
    let window: u64 = measure_env("MEASURE_WINDOW", 60_000);
    let layout = Layout::new();
    let service = e2e::measure::MeasureService::start(plan)
        .await
        .expect("start r23 provider");
    let args = interactive_args();
    let window_text = window.to_string();
    let mut run = PtyRun::spawn_with_env(
        &layout.cwd,
        &layout.home,
        &layout.sessions,
        &layout.state,
        service.base_url(),
        &args,
        &[("CLAUDE_CODE_AUTO_COMPACT_WINDOW", window_text.as_str())],
    )
    .expect("spawn zo in PTY");
    run.wait_for("directory:", TEST_TIMEOUT);

    let turn_timeout = Duration::from_secs(60);
    let mut samples = Vec::new();
    for turn in 1..=turns {
        let elapsed = measured_turn(&mut run, "R23_BIG emit the payload", turn_timeout);
        let rss = run.rss_kib().expect("the child is still alive");
        // The footer already carries the occupancy the ladder gates on, so the
        // measurement reads zo's own number instead of re-deriving one that
        // could disagree with it.
        let left = context_left_percent(&run.snapshot_output()).unwrap_or(0);
        samples.push((elapsed.as_millis(), rss));
        eprintln!(
            "[r23-long] turn {turn:3}: wall={}ms rss={rss} KiB context_used={}%",
            elapsed.as_millis(),
            100_u32.saturating_sub(left),
        );
    }
    let capture = run.finish();

    let half = samples.len() / 2;
    let first_rss = samples[0].1;
    let mid_rss = samples[half].1;
    let last_rss = samples[samples.len() - 1].1;
    eprintln!(
        "[r23-long] RSS {first_rss} -> {last_rss} KiB over {} turns · first half +{} · second half +{}",
        samples.len(),
        mid_rss.saturating_sub(first_rss),
        last_rss.saturating_sub(mid_rss),
    );
    let wall_early: u128 = samples[..half].iter().map(|(wall, _)| wall).sum();
    let wall_late: u128 = samples[half..].iter().map(|(wall, _)| wall).sum();
    eprintln!(
        "[r23-long] wall per turn: first half {}ms · second half {}ms",
        wall_early / half.max(1) as u128,
        wall_late / (samples.len() - half).max(1) as u128,
    );

    eprintln!(
        "[r23-long] footer 'context left' readings, in order: {:?}",
        context_left_readings(&capture)
    );

    // Microcompaction is only visible on the screen; full compaction is only
    // visible on the wire.  Count both from the place that actually sees them.
    let screen = String::from_utf8_lossy(&capture);
    let trims = screen.matches("Context trim").count();
    let recorded = service.recorded().await;
    let summaries: Vec<usize> = recorded
        .iter()
        .enumerate()
        .filter(|(_, row)| row.compaction_summary)
        .map(|(index, _)| index)
        .collect();
    eprintln!(
        "[r23-long] window={window} requests={} microcompact_notices={trims} full_compactions={}",
        recorded.len(),
        summaries.len(),
    );

    report_compaction_prefixes(&recorded, &summaries);

    dump_request_bodies(&recorded);

    report_request_ledger(&layout.home);

    assert!(
        samples.len() == turns,
        "a turn was lost — the numbers above describe a different session"
    );
}

/// Every distinct occupancy the footer showed, in order.
///
/// A measurement that derives its own percentage can disagree with the one on
/// the user's screen; this reads zo's own number, and if it never moves that is
/// itself the finding — the footer takes the PROVIDER's reported usage
/// (`RenderBlock::Usage.ctx_tokens`), so a mock that answers with a constant
/// makes a 70-turn session read `100% context left` forever.
fn context_left_readings(capture: &[u8]) -> Vec<u32> {
    let text = String::from_utf8_lossy(capture);
    let mut seen: Vec<u32> = Vec::new();
    let mut rest = text.as_ref();
    while let Some(at) = rest.find("% context left") {
        let digits: String = rest[..at]
            .chars()
            .rev()
            .take_while(char::is_ascii_digit)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        if let Ok(value) = digits.parse::<u32>() {
            if seen.last() != Some(&value) {
                seen.push(value);
            }
        }
        rest = &rest[at + "% context left".len()..];
    }
    seen
}

/// What one compaction costs the PREFIX.
///
/// A mock invents its own `cache_creation` / `cache_read`, so reading those
/// back would be circular; the provider matches serialized prefixes, and those
/// are real in every captured body.
fn report_compaction_prefixes(recorded: &[e2e::measure::Recorded], summaries: &[usize]) {
    for index in summaries {
        let summary = &recorded[*index];
        let Some(previous) = recorded[..*index].iter().rev().find(|row| !row.compaction_summary)
        else {
            continue;
        };
        let before = previous.messages_json();
        let during = summary.messages_json();
        let shared_summary = e2e::measure::shared_prefix_len(&before, &during);
        eprintln!(
            "[r23-long] compaction at request {index} (t={}ms, {}ms after the live request before it): \
             live_before={}B/{}msg summary_request={}B/{}msg shared_prefix={shared_summary}B ({}%)",
            summary.at_ms,
            summary.at_ms.saturating_sub(previous.at_ms),
            before.len(),
            previous.message_count(),
            during.len(),
            summary.message_count(),
            shared_summary * 100 / before.len().max(1),
        );
        if let Some(after) = recorded[*index + 1..].iter().find(|row| !row.compaction_summary) {
            let after_json = after.messages_json();
            let shared_after = e2e::measure::shared_prefix_len(&before, &after_json);
            eprintln!(
                "[r23-long]   next live request={}B/{}msg shared_prefix_with_before={shared_after}B ({}%)",
                after_json.len(),
                after.message_count(),
                shared_after * 100 / before.len().max(1),
            );
        }
    }
}

/// Write every captured body out, so a divergence between a live request and
/// the summary request that follows it can be read byte by byte instead of
/// guessed at from a percentage.
fn dump_request_bodies(recorded: &[e2e::measure::Recorded]) {
    if let Ok(dir) = std::env::var("MEASURE_DUMP") {
        let dir = PathBuf::from(dir);
        fs::create_dir_all(&dir).expect("dump directory");
        for (index, row) in recorded.iter().enumerate() {
            let tag = if row.compaction_summary { "summary" } else { "live" };
            fs::write(dir.join(format!("{index:03}-{tag}.json")), &row.body)
                .expect("dump request body");
        }
        eprintln!("[r23-long] dumped {} request bodies to {}", recorded.len(), dir.display());
    }
}

/// The per-request ledger this build gained (edc9f92), from the private config
/// home — so it describes this run and nothing else.
fn report_request_ledger(home: &std::path::Path) {
    let ledger = find_requests_ledger(home);
    match ledger {
        Some(path) => {
            let rows = fs::read_to_string(&path).unwrap_or_default();
            eprintln!(
                "[r23-long] requests.jsonl rows={} at {}",
                rows.lines().filter(|line| !line.trim().is_empty()).count(),
                path.display()
            );
            for line in rows.lines().take(4) {
                eprintln!("[r23-long]   {line}");
            }
        }
        None => eprintln!("[r23-long] requests.jsonl absent under the private config home"),
    }
}

/// Last `N% context left` the footer printed, as zo itself computed it.
fn context_left_percent(capture: &[u8]) -> Option<u32> {
    let text = String::from_utf8_lossy(capture);
    let at = text.rfind("% context left")?;
    let head = &text[..at];
    let digits: String = head
        .chars()
        .rev()
        .take_while(char::is_ascii_digit)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    digits.parse().ok()
}

/// Find this run's `requests.jsonl` under the private config home.
fn find_requests_ledger(home: &std::path::Path) -> Option<PathBuf> {
    let root = home.join("cache").join("prompt-cache");
    let mut newest: Option<(std::time::SystemTime, PathBuf)> = None;
    for entry in fs::read_dir(root).ok()?.flatten() {
        let candidate = entry.path().join("requests.jsonl");
        let Ok(meta) = fs::metadata(&candidate) else {
            continue;
        };
        let Ok(modified) = meta.modified() else {
            continue;
        };
        if newest.as_ref().is_none_or(|(best, _)| modified > *best) {
            newest = Some((modified, candidate));
        }
    }
    newest.map(|(_, path)| path)
}

/// r23 (3) — several children at once.
///
/// Sub-agents run on detached OS threads inside the same zo process, so the
/// parent's resident set and CPU already contain them; what the measurement has
/// to establish first is whether three children actually run *at once*, because
/// a serialized fan-out would make every other number here describe a different
/// machine than the one the roadmap is asking about.
///
/// Read TWO sizes.  One child and three children give the marginal cost of one
/// more; a single reading cannot separate it from the turn's fixed cost — the
/// same correction `frame-diff-r10.md` had to make for the repaint ruler.
///
/// ```text
/// MEASURE_CHILDREN=1 cargo test --release -p zo-ide --test e2e_hermetic \
///     r23_concurrent_children -- --ignored --nocapture
/// MEASURE_CHILDREN=3 ...
/// ```
#[ignore = "measurement, not a contract"]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn r23_concurrent_children() {
    let plan = e2e::measure::Plan {
        children: measure_env("MEASURE_CHILDREN", 3),
        child_sleep_secs: measure_env("MEASURE_CHILD_SLEEP", 3),
        background: measure_env("MEASURE_BACKGROUND", false),
        ..e2e::measure::Plan::default()
    };
    let layout = Layout::new();
    layout.model_led();
    let service = e2e::measure::MeasureService::start(plan)
        .await
        .expect("start r23 provider");
    let args = interactive_args();
    let mut run = pty(&layout, service.base_url(), &args);
    run.wait_for("directory:", TEST_TIMEOUT);

    let idle_rss = run.rss_kib().expect("alive");
    let idle_threads = run.thread_count().unwrap_or_default();
    let cpu_before = run.cpu_ms().expect("alive");

    run.send(b"R23_SPAWN delegate the survey\r").expect("send");
    let started = std::time::Instant::now();
    let deadline = started + Duration::from_secs(180);
    let mut peak_rss = idle_rss;
    let mut peak_threads = idle_threads;
    // Watch the WIRE, not the screen. A child's report is rendered into the
    // parent's transcript, so a screen marker cannot say whether the third
    // child has finished or the first one merely printed.
    let wanted = plan.children * 2;
    loop {
        let child_requests = service
            .recorded()
            .await
            .iter()
            .filter(|row| row.opens_with("R23_SLEEP"))
            .count();
        if child_requests >= wanted {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "only {child_requests} of {wanted} child requests arrived"
        );
        peak_rss = peak_rss.max(run.rss_kib().unwrap_or(peak_rss));
        peak_threads = peak_threads.max(run.thread_count().unwrap_or(peak_threads));
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let wall = started.elapsed();
    let cpu = run.cpu_ms().expect("alive").saturating_sub(cpu_before);
    let settled_rss = run.rss_kib().expect("alive");
    let recorded = service.recorded().await;
    let capture = run.finish();
    let (bytes, lines, el, frames) = repaint_ruler(&capture);

    // When each child STARTED, from the request that carried its sleep. Three
    // starts inside a few milliseconds is concurrency; three starts a sleep
    // apart is a queue, and the difference is invisible in a total.
    let mut starts: Vec<u128> = recorded
        .iter()
        .filter(|row| row.opens_with("R23_SLEEP") && row.message_count() == 1)
        .map(|row| row.at_ms)
        .collect();
    starts.sort_unstable();
    let spread = starts
        .last()
        .zip(starts.first())
        .map_or(0, |(last, first)| last - first);

    eprintln!(
        "[r23-children] children={} background={} sleep={}s → wall={}ms cpu={cpu}ms cpu/wall={}%",
        plan.children,
        plan.background,
        plan.child_sleep_secs,
        wall.as_millis(),
        u128::from(cpu) * 100 / wall.as_millis().max(1),
    );
    eprintln!(
        "[r23-children] child starts at {starts:?} ms · spread={spread}ms → {}",
        if spread < u128::from(plan.child_sleep_secs) * 500 {
            "CONCURRENT"
        } else {
            "SERIALIZED"
        }
    );
    eprintln!(
        "[r23-children] rss idle={idle_rss} peak={peak_rss} settled={settled_rss} KiB \
         (+{} KiB peak over idle) · threads idle={idle_threads} peak={peak_threads} \
         · requests={}",
        peak_rss.saturating_sub(idle_rss),
        recorded.len(),
    );
    eprintln!(
        "[r23-children] repaint bytes={bytes} committed_lines={lines} EL={el} frames={frames}"
    );
    assert!(
        starts.len() == plan.children,
        "expected {} children to start, saw {}",
        plan.children,
        starts.len()
    );
}

/// A command that makes its own process group the terminal's foreground group
/// and exits — what an interactive shell does when it comes up under a test.
/// Only a process that shares zo's controlling terminal can; `/dev/tty` is
/// that terminal. SIGTTOU is ignored so the call is allowed from a background
/// group, as a job-control shell arranges for itself.
const TERMINAL_THIEF: &str = "perl -e '$SIG{TTOU} = \"IGNORE\"; use POSIX (); \
open(my $tty, \"+<\", \"/dev/tty\") or exit 0; \
POSIX::tcsetpgrp(fileno($tty), getpgrp()) or exit 3; exit 0'";

/// A tool that takes the terminal must not take the pane's input with it.
///
/// Before: the thief's group died with the tool, every read of the terminal
/// failed with `EIO`, crossterm's event thread retried the read forever
/// without waking the app, and the pane sat with a composer that echoed
/// nothing while one core spun ("zo 지금 인풋창에 아무것도 입력안됨",
/// 2026-09-03). zo takes the terminal back on the size tick — so a key typed
/// after the turn shows up in the composer as it always did.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_a_tool_that_takes_the_terminal_does_not_take_the_pane_input() {
    let layout = Layout::new();
    let service = ScriptedAnthropicService::bash(TERMINAL_THIEF, "the thief is gone\n")
        .await
        .expect("start bash script");
    let args = interactive_args();
    let mut run = PtyRun::spawn_controlling(
        &layout.cwd,
        &layout.home,
        &layout.sessions,
        &layout.state,
        service.base_url(),
        &args,
    )
    .expect("spawn zo on its own terminal");

    run.wait_for("directory:", TEST_TIMEOUT);
    run.send(b"Run the thief once, then say it is gone\r")
        .expect("send tool prompt");
    run.wait_for("the thief is gone", TEST_TIMEOUT);

    // The thief is gone and so, until the next size tick, is the terminal.
    // Typing must still reach the composer.
    run.send(b"still typing here").expect("type after the tool");
    run.wait_for("still typing here", TEST_TIMEOUT);
    let _ = run.finish();
}

/// A pane the window closes under zo ends the way `/exit` ends it — never as a
/// panic.
///
/// Closing a pane closes the terminal's master and hangs the session up; zo
/// ends the conversation on that signal, puts the screen back, and then wrote
/// its exit summary with `println!` onto a terminal that was gone. The write
/// answered EIO and `println!` panics on EIO: 129 of the last 300 zo panes the
/// window launched ended as `process exit reason=panic` this way
/// (`~/.zo/logs/zo-ide.log`, 2026-09-17). A panic is exit code 101.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_a_pane_closed_under_zo_ends_without_a_panic() {
    let layout = Layout::new();
    let service = ScriptedAnthropicService::text("the summary is owed")
        .await
        .expect("start text script");
    let args = interactive_args();
    let mut run = PtyRun::spawn_controlling(
        &layout.cwd,
        &layout.home,
        &layout.sessions,
        &layout.state,
        service.base_url(),
        &args,
    )
    .expect("spawn zo on its own terminal");

    run.wait_for("directory:", TEST_TIMEOUT);
    run.send(b"answer once\r").expect("send prompt");
    run.wait_for("the summary is owed", TEST_TIMEOUT);
    // A written transcript is what puts a summary on the way out; a session
    // that never spoke leaves without one, and there is nothing to write.
    let _ = wait_for_transcript(&layout, "the summary is owed", TEST_TIMEOUT);

    let code = run.hang_up(TEST_TIMEOUT);
    assert_eq!(
        code,
        Some(0),
        "zo left a pane closed under it with {code:?} instead of the way /exit leaves"
    );
}

/// Wait until the scripted service has recorded exactly `expected` request
/// bodies, then pin the count. A prompt whose echo already contains the
/// answer text makes `wait_for` return before the provider is asked, so the
/// count is a deadline, not a snapshot.
async fn wait_for_request_count(service: &ScriptedAnthropicService, expected: usize) {
    let deadline = Instant::now() + TEST_TIMEOUT;
    loop {
        let seen = service.request_bodies().await.len();
        if seen >= expected {
            assert_eq!(seen, expected, "more provider requests than the scenario sends");
            return;
        }
        assert!(
            Instant::now() < deadline,
            "the provider saw {seen} request(s), expected {expected}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_capabilities_report_real_build_and_keep_launch_immutable_across_session_changes() {
    let layout = Layout::new();
    let seed_service = ScriptedAnthropicService::text("saved capability fixture").await.unwrap();
    let mut seed = pty(&layout, seed_service.base_url(), &interactive_args());
    seed.wait_for("directory:", TEST_TIMEOUT);
    seed.send(b"save the capability resume fixture\r").unwrap();
    seed.wait_for("saved capability fixture", TEST_TIMEOUT);
    let transcript = wait_for_transcript(&layout, "saved capability fixture", TEST_TIMEOUT);
    let resume_target = session_id_of(&transcript);
    let _ = seed.finish();
    let service = ScriptedAnthropicService::text("capability answer").await.unwrap();
    let address = layout.root.path().join("capabilities.addr");
    let address_text = address.to_string_lossy().into_owned();
    let mut run = PtyRun::spawn_with_env(&layout.cwd,&layout.home,&layout.sessions,&layout.state,
        service.base_url(), &["--model","claude-fable-5-1","--effort","high","--permission-mode","danger-full-access"],
        &[("ZO_EVENTS_ADDR_FILE",&address_text),("ZO_SERVE_TOKEN","capability-canary-token")]).unwrap();
    run.wait_for("directory:", TEST_TIMEOUT);
    let mut client = connect_to_addr_file(&address,"capability-canary-token").await;
    let initial = client.call("session.capabilities",serde_json::json!({})).await.unwrap();
    assert_eq!(initial["process"]["pid"],run.pid());
    assert_eq!(initial["process"]["build"]["git_sha"],env!("ZO_BUILD_GIT_SHA"));
    assert_eq!(initial["launch"]["requested"]["model"],"claude-fable-5-1");
    assert_eq!(initial["launch"]["effective"]["wire_effort"],"high");
    assert_eq!(initial["mode"]["effective"],"inline");
    assert_eq!(initial["mode"]["reason"],"missing-team-binding");
    assert!(initial["identity"]["pane"].is_null());
    assert!(!initial.to_string().contains("capability-canary"));
    assert!(initial.to_string().len() < 32*1024);
    if let Some(path) = std::env::var_os("ZO_CAPABILITIES_EVIDENCE") {
        let mut redacted = initial.clone();
        redacted["process"]["executable"] = serde_json::json!("<built-test-binary>");
        fs::write(path,serde_json::to_vec_pretty(&redacted).unwrap()).unwrap();
    }
    let endpoint = initial["identity"]["channel_session_id"].as_str().unwrap();
    let original_session = initial["identity"]["session_id"].as_str().unwrap().to_string();
    // Resume is a transcript operation: persist a real turn before switching
    // away, since an untouched empty conversation has nothing durable to load.
    // "capability answer" is also a substring of the typed prompt's echo, so
    // the screen alone cannot say the model was asked: wait for the request.
    run.send(b"give the capability answer\r").unwrap();
    run.wait_for("capability answer",TEST_TIMEOUT);
    wait_for_request_count(&service,1).await;
    let hydrated = client.subscribe(endpoint,true).await.unwrap();
    assert_eq!(hydrated["history"].as_array().unwrap().last().unwrap()["revision"],initial["revision"]);
    run.send(b"/model\r").unwrap();
    run.wait_for("Select Model and Effort",TEST_TIMEOUT);
    run.send(b"\r").unwrap();
    run.wait_for("Select Reasoning Effort",TEST_TIMEOUT);
    run.send(b"\x1b[A\r").unwrap();
    let deadline = Instant::now()+TEST_TIMEOUT;
    let changed = loop {
        let value = client.call("session.capabilities",serde_json::json!({})).await.unwrap();
        if value["current"]["effective"]["effort"] == "medium" { break value; }
        assert!(Instant::now()<deadline,"effort change did not reach capability snapshot");
        tokio::time::sleep(Duration::from_millis(25)).await;
    };
    assert_eq!(changed["launch"],initial["launch"]);
    assert!(changed["revision"].as_u64()>initial["revision"].as_u64());
    run.send(b"/new\r").unwrap();
    let deadline = Instant::now()+TEST_TIMEOUT;
    loop {
        let value = client.call("session.capabilities",serde_json::json!({})).await.unwrap();
        if value["identity"]["session_id"] != original_session { break; }
        assert!(Instant::now()<deadline,"new active session was not published");
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    run.send(format!("/resume {resume_target}\r").as_bytes()).unwrap();
    let deadline = Instant::now()+TEST_TIMEOUT;
    loop {
        let value = client.call("session.capabilities",serde_json::json!({})).await.unwrap();
        if value["identity"]["session_id"] == resume_target {
            assert_eq!(value["identity"]["channel_session_id"],endpoint);
            assert_eq!(value["launch"],initial["launch"]);
            break;
        }
        assert!(Instant::now()<deadline,"in-process resume was not published");
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert_eq!(service.request_bodies().await.len(),1,"read-only operations sent provider input");
    let output_start = run.output_len();
    run.send(b"give the capability answer again\r").unwrap();
    run.wait_for_after("capability answer",output_start,TEST_TIMEOUT);
    wait_for_request_count(&service,2).await;
    let capture = run.finish();
    assert!(!String::from_utf8_lossy(&capture).contains("session_capabilities"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_old_client_ignores_capability_frames_and_headless_token_remains_optional() {
    let layout = Layout::new();
    let service = ScriptedAnthropicService::text("old client answer").await.unwrap();
    let address = layout.root.path().join("old-client.addr");
    let address_text = address.to_string_lossy().into_owned();
    let mut run = PtyRun::spawn_with_env(&layout.cwd,&layout.home,&layout.sessions,&layout.state,
        service.base_url(), &["--plain","--events-bind","127.0.0.1:0","--permission-mode","danger-full-access"],
        &[("ZO_EVENTS_ADDR_FILE",&address_text)]).unwrap();
    // This client deliberately never asks session.capabilities.
    let mut client = connect_to_addr_file(&address,"").await;
    let info = client.call(method::INFO,serde_json::json!({})).await.unwrap();
    assert_eq!(info.as_object().unwrap().len(),8);
    let session = info["id"].as_str().unwrap();
    let hydrated = client.subscribe(session,true).await.unwrap();
    assert!(hydrated["history"].as_array().unwrap().iter().any(|v|v["type"]=="session_capabilities"));
    let stopped = client.call(method::CANCEL_TURN,serde_json::json!({})).await.unwrap();
    assert_eq!(stopped["cancelled"],false);
    run.send(b"answer the old client\n").unwrap();
    run.wait_for("old client answer",TEST_TIMEOUT);
    assert_eq!(service.request_bodies().await.len(),1);
    assert_eq!(client.call(method::INFO,serde_json::json!({})).await.unwrap()["id"],session);
    let capture = run.finish();
    assert!(!String::from_utf8_lossy(&capture).contains("session_capabilities"));
    assert!(!layout.state.join(format!("zo-events-{}.addr",info["pid"])).exists());
}

/// The exact-launch fixture, read from the shipped catalog rather than spelled
/// here: the first Anthropic row (the hermetic provider) whose declared effort
/// levels stop short of a rung zo's `--effort` accepts. Returns the row's id,
/// a level the row declares, and one it does not.
fn a_catalog_row_with_an_undeclared_effort() -> (String, String, String) {
    let catalog: serde_json::Value = serde_json::from_str(api::builtin_model_catalog_json()).unwrap();
    for row in catalog["models"].as_array().into_iter().flatten() {
        if row["provider"] != "anthropic" { continue; }
        let Some(levels) = row["effort_levels"].as_array() else { continue; };
        let declared: Vec<&str> = levels.iter().filter_map(serde_json::Value::as_str).collect();
        let Some(missing) = ["ultra", "max", "xhigh"].into_iter().find(|rung| !declared.contains(rung)) else { continue; };
        let Some(id) = row["ids"][0].as_str() else { continue; };
        let accepted = declared.last().copied().unwrap_or("high");
        return (id.to_string(), accepted.to_string(), missing.to_string());
    }
    panic!("the shipped catalog declares every rung for every Anthropic row; the fixture needs a ceiling");
}

const LAUNCH_REFUSED_EXIT: i32 = 4;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_launch_contract_refuses_before_any_provider_request_and_keeps_the_reason_on_the_channel() {
    let layout = Layout::new();
    let service = ScriptedAnthropicService::text("must never be asked").await.unwrap();
    let (model, _accepted, refused) = a_catalog_row_with_an_undeclared_effort();
    let address = layout.root.path().join("contract.addr");
    let address_text = address.to_string_lossy().into_owned();
    let mut run = PtyRun::spawn_with_env(&layout.cwd, &layout.home, &layout.sessions, &layout.state,
        service.base_url(),
        &["--launch-contract", "1", "--model", &model, "--effort", &refused,
          "--events-bind", "127.0.0.1:0", "--permission-mode", "danger-full-access"],
        &[("ZO_EVENTS_ADDR_FILE", &address_text), ("ZO_SERVE_TOKEN", "contract-canary-token"),
          ("ZO_LAUNCH_REQUEST", r#"{"request_id":"req-t2773","agent":"zo"}"#)]).unwrap();
    let mut client = connect_to_addr_file(&address, "contract-canary-token").await;
    let cap = client.call("session.capabilities", serde_json::json!({})).await.unwrap();
    assert_eq!(cap["launch"]["policy"], "exact", "{cap}");
    assert_eq!(cap["launch"]["request_id"], "req-t2773");
    assert_eq!(cap["launch"]["validation"], serde_json::json!({"state":"refused","reason":"unsupported-effort"}));
    let contract = &cap["launch"]["contract"];
    assert_eq!(contract["version"], 1);
    assert_eq!(contract["verdict"], "refused");
    assert_eq!(contract["reason"], "unsupported-effort");
    assert_eq!(contract["requested"], serde_json::json!({"agent":"zo","model":model,"effort":refused}));
    assert!(contract["effective"].is_null(), "{contract}");
    assert_eq!(cap["support"]["exact_launch"], serde_json::json!({"supported":true,"version":1}));
    assert!(!cap.to_string().contains("contract-canary"));
    // The verdict was read; the process leaves on its own, nonzero, having
    // read no task input and asked the provider nothing.
    assert_eq!(run.exit_code(TEST_TIMEOUT), Some(LAUNCH_REFUSED_EXIT));
    let output = String::from_utf8_lossy(&run.finish()).into_owned();
    assert!(output.contains("unsupported-effort"), "{output}");
    assert!(output.contains(&refused), "{output}");
    assert!(service.request_bodies().await.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_launch_contract_accepts_an_exact_selection_and_reports_it_before_the_first_turn() {
    let layout = Layout::new();
    let service = ScriptedAnthropicService::text("exact answer").await.unwrap();
    let (model, accepted, _refused) = a_catalog_row_with_an_undeclared_effort();
    let address = layout.root.path().join("accepted.addr");
    let address_text = address.to_string_lossy().into_owned();
    let mut run = PtyRun::spawn_with_env(&layout.cwd, &layout.home, &layout.sessions, &layout.state,
        service.base_url(),
        &["--launch-contract", "1", "--model", &model, "--effort", &accepted, "--permission-mode", "danger-full-access"],
        &[("ZO_EVENTS_ADDR_FILE", &address_text), ("ZO_SERVE_TOKEN", "accepted-canary-token")]).unwrap();
    run.wait_for("directory:", TEST_TIMEOUT);
    let mut client = connect_to_addr_file(&address, "accepted-canary-token").await;
    let cap = client.call("session.capabilities", serde_json::json!({})).await.unwrap();
    assert_eq!(cap["launch"]["policy"], "exact", "{cap}");
    assert_eq!(cap["launch"]["validation"], serde_json::json!({"state":"accepted","reason":null}));
    let contract = &cap["launch"]["contract"];
    assert_eq!(contract["verdict"], "accepted");
    assert!(contract["reason"].is_null());
    assert_eq!(contract["requested"]["model"], model);
    assert_eq!(contract["requested"]["effort"], accepted);
    assert_eq!(contract["effective"]["model"], model);
    assert_eq!(contract["effective"]["effort"], accepted);
    assert_eq!(contract["effective"]["provider"], "anthropic");
    assert_eq!(cap["launch"]["effective"]["model"], model);
    assert_eq!(cap["launch"]["effective"]["effort"], accepted);
    assert!(service.request_bodies().await.is_empty(), "validation asked the provider");
    // "exact answer" is also a substring of the typed prompt's echo, so the
    // screen cannot say the provider was asked: wait for the request itself.
    run.send(b"give the exact answer\r").unwrap();
    run.wait_for("exact answer", TEST_TIMEOUT);
    wait_for_request_count(&service, 1).await;
    let again = client.call("session.capabilities", serde_json::json!({})).await.unwrap();
    assert_eq!(again["launch"], cap["launch"], "a turn rewrote the launch receipt");
    let _ = run.finish();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_launch_contract_matrix_over_flag_env_settings_and_legacy() {
    let layout = Layout::new();
    let service = ScriptedAnthropicService::text("legacy answer").await.unwrap();
    let (model, accepted, refused) = a_catalog_row_with_an_undeclared_effort();
    let pipe = |args: &[&str], env: &[(&str, &str)]| {
        run_pipe_with_env(&layout.cwd, &layout.home, &layout.sessions, &layout.state,
            service.base_url(), args, b"say the legacy answer\n", env).unwrap()
    };
    let permission = ["--permission-mode", "danger-full-access", "--plain"];

    // No flag: the same selection zo cannot honour exactly proceeds as today.
    let legacy = pipe(&[&permission[..], &["--model", &model, "--effort", &refused]].concat(), &[]);
    assert!(legacy.status.success(), "{}", String::from_utf8_lossy(&legacy.stderr));
    assert!(String::from_utf8_lossy(&legacy.stdout).contains("legacy answer"));
    assert_eq!(service.request_bodies().await.len(), 1);

    // The flag: refused by name, nonzero, and the provider is not asked.
    let flagged = pipe(&[&permission[..], &["--launch-contract", "1", "--model", &model, "--effort", &refused]].concat(), &[]);
    assert_eq!(flagged.status.code(), Some(LAUNCH_REFUSED_EXIT), "{}", String::from_utf8_lossy(&flagged.stderr));
    let stderr = String::from_utf8_lossy(&flagged.stderr);
    assert!(stderr.contains("unsupported-effort") && stderr.contains(&refused) && stderr.contains(&model), "{stderr}");
    assert_eq!(service.request_bodies().await.len(), 1, "a refused launch asked the provider");

    // A literal the catalog has no facts for is never snapped to a neighbour.
    let unknown = pipe(&[&permission[..], &["--launch-contract", "1", "--model", "no-such-model-t2773"]].concat(), &[]);
    assert_eq!(unknown.status.code(), Some(LAUNCH_REFUSED_EXIT));
    assert!(String::from_utf8_lossy(&unknown.stderr).contains("unsupported-model"));

    // The request envelope must agree with argv.
    let conflicting = pipe(&[&permission[..], &["--launch-contract", "1", "--model", &model]].concat(),
        &[("ZO_LAUNCH_REQUEST", r#"{"agent":"codex"}"#)]);
    assert_eq!(conflicting.status.code(), Some(LAUNCH_REFUSED_EXIT));
    assert!(String::from_utf8_lossy(&conflicting.stderr).contains("agent-mismatch"));

    // An unknown guard version fails explicitly instead of running unguarded.
    let future = pipe(&[&permission[..], &["--launch-contract", "2", "--model", &model]].concat(), &[]);
    assert!(!future.status.success());
    assert!(String::from_utf8_lossy(&future.stderr).contains("exact-launch-unavailable"));

    // The env door and the settings door open the same guard.
    let by_env = pipe(&[&permission[..], &["--model", &model, "--effort", &refused]].concat(), &[("ZO_LAUNCH_CONTRACT", "1")]);
    assert_eq!(by_env.status.code(), Some(LAUNCH_REFUSED_EXIT));
    fs::write(layout.home.join("settings.json"), br#"{"smart":{"autoClassifier":"off"},"launchContract":1}"#).unwrap();
    let by_settings = pipe(&[&permission[..], &["--model", &model, "--effort", &refused]].concat(), &[]);
    assert_eq!(by_settings.status.code(), Some(LAUNCH_REFUSED_EXIT));
    let by_settings_off = pipe(&[&permission[..], &["--launch-contract", "0", "--model", &model, "--effort", &refused]].concat(), &[]);
    assert!(by_settings_off.status.success(), "the flag could not turn the settings door off");
    fs::write(layout.home.join("settings.json"), br#"{"smart":{"autoClassifier":"off"}}"#).unwrap();
    assert_eq!(service.request_bodies().await.len(), 2);

    // JSON consumers get the verdict as one typed line, on stdout, before anything else.
    let json = pipe(&[&permission[..], &["--json", "--launch-contract", "1", "--model", &model, "--effort", &refused]].concat(), &[]);
    assert_eq!(json.status.code(), Some(LAUNCH_REFUSED_EXIT));
    let first: serde_json::Value = serde_json::from_str(String::from_utf8_lossy(&json.stdout).lines().next().unwrap_or("{}")).unwrap();
    assert_eq!(first["type"], "launch_refused");
    assert_eq!(first["reason"], "unsupported-effort");
    assert_eq!(first["requested"]["model"], model);
    assert_eq!(first["requested"]["effort"], refused);

    // And an accepted exact launch through the pipe runs one ordinary turn.
    let exact = pipe(&[&permission[..], &["--launch-contract", "1", "--model", &model, "--effort", &accepted]].concat(), &[]);
    assert!(exact.status.success(), "{}", String::from_utf8_lossy(&exact.stderr));
    assert_eq!(service.request_bodies().await.len(), 3);
}

/* ---- t-2513: the teammate lifecycle contract ---- */

/// The role prompt a parent carries in a v2 brief. Spelled so a request body
/// can be searched for it — and so the child's OWN root prompt, which never
/// contains it, is provably not what the model was shown.
const TEAMMATE_HARNESS_CANARY: &str =
    "TEAMMATE-HARNESS-CANARY: you are the survey helper; answer briefly and stop.";
const TEAMMATE_PARENT_SESSION: &str = "session-parent-e2e";

/// A parent's events channel as a teammate sees it: a discovery file and a
/// listener that answers `session.list`. [`Self::vanish`] closes the port and
/// removes the file — the parent is gone, both ways a child can tell.
struct FakeParent {
    discovery: PathBuf,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    /// Every request the parent was asked, in order — the `mcp.call`s a
    /// child routes to it among them.
    asked: std::sync::Arc<std::sync::Mutex<Vec<serde_json::Value>>>,
}

impl FakeParent {
    fn start(discovery: PathBuf) -> Self {
        use std::io::{BufRead as _, Write as _};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind fake parent");
        listener.set_nonblocking(true).expect("nonblocking");
        let addr = listener.local_addr().expect("addr");
        fs::write(
            &discovery,
            format!("{addr}\nparent-token\n{TEAMMATE_PARENT_SESSION}\n"),
        )
        .expect("write parent discovery file");
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let asked: std::sync::Arc<std::sync::Mutex<Vec<serde_json::Value>>> = std::sync::Arc::default();
        let flag = std::sync::Arc::clone(&stop);
        let seen = std::sync::Arc::clone(&asked);
        let thread = std::thread::spawn(move || {
            while !flag.load(std::sync::atomic::Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let _ = stream.set_nonblocking(false);
                        let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                        let mut reader = std::io::BufReader::new(stream.try_clone().expect("clone"));
                        let mut line = String::new();
                        if reader.read_line(&mut line).is_ok() {
                            let request = serde_json::from_str::<serde_json::Value>(line.trim())
                                .unwrap_or_default();
                            seen.lock().unwrap().push(request.clone());
                            let id = request["id"].as_u64().unwrap_or_default();
                            // The parent answers `mcp.call` as its MCP runtime
                            // would — the answer names the input it was given —
                            // and everything else as the identity handshake.
                            let result = if request["method"] == "mcp.call" {
                                let name = request["params"]["name"].as_str().unwrap_or_default();
                                let known = name == "mcp__ctx7__query";
                                serde_json::json!({
                                    "name": name,
                                    "content": if known {
                                        format!("PARENT-MCP-ANSWER for {}", request["params"]["input"]["q"].as_str().unwrap_or(""))
                                    } else {
                                        format!("unknown MCP tool `{name}`")
                                    },
                                    "is_error": !known,
                                })
                            } else {
                                serde_json::json!([{"id": TEAMMATE_PARENT_SESSION, "messages": 0}])
                            };
                            let mut writer = stream;
                            let _ = writeln!(
                                writer,
                                "{}",
                                serde_json::json!({"jsonrpc": "2.0", "id": id, "result": result})
                            );
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(20));
                    }
                    Err(_) => break,
                }
            }
        });
        Self {
            discovery,
            stop,
            thread: Some(thread),
            asked,
        }
    }

    /// The requests this parent answered so far.
    fn asked(&self) -> Vec<serde_json::Value> {
        self.asked.lock().unwrap().clone()
    }

    /// The parent dies: nobody answers, and its exit guard took the file.
    fn vanish(mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let _ = fs::remove_file(&self.discovery);
    }
}

impl Drop for FakeParent {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Layout {
    /// The lifecycle numbers, small enough for a test to wait out
    /// (`runtime::subagent_panes::Limits`, settings `subagents.*`), beside
    /// the routing-probe switch every scenario needs.
    fn with_subagent_limits(&self, limits: &serde_json::Value) {
        let settings = serde_json::json!({
            "smart": {"autoClassifier": "off"},
            "subagents": limits,
        });
        fs::write(
            self.home.join("settings.json"),
            serde_json::to_vec(&settings).expect("serialize limits"),
        )
        .expect("rewrite e2e settings with limits");
    }
}

/// A v2 brief with the harness a parent would carry for `role`.
fn teammate_brief_v2(
    agent_id: &str,
    prompt: &str,
    role: &str,
    permission_mode: &str,
    parent_channel: Option<PathBuf>,
    idle_budget_ms: Option<u64>,
) -> runtime::subagent_panes::Brief {
    use runtime::subagent_panes::{Brief, ModelSelection, ResolvedHarness, PROTOCOL_VERSION};
    Brief {
        version: PROTOCOL_VERSION,
        agent_id: agent_id.to_string(),
        prompt: prompt.to_string(),
        description: "survey".to_string(),
        subagent_type: Some(role.to_string()),
        name: Some("survey".to_string()),
        permission_mode: Some(permission_mode.to_string()),
        parent_session: Some(TEAMMATE_PARENT_SESSION.to_string()),
        tool_call_id: Some("toolu_life".to_string()),
        harness: Some(ResolvedHarness {
            system_prompt: vec![TEAMMATE_HARNESS_CANARY.to_string()],
            allowed_tools: tools::allowed_tools_for_subagent(role),
            permission_mode: Some(permission_mode.to_string()),
            model: ModelSelection::default(),
            ..ResolvedHarness::default()
        }),
        parent_channel,
        idle_budget_ms,
        ..Brief::default()
    }
}

/// The teammate's channel, found the way its parent finds it: the copy of
/// its discovery record beside the brief.
async fn connect_to_teammate(directory: &std::path::Path) -> (Client, String) {
    let channel = directory.join(runtime::subagent_panes::CHANNEL_FILE);
    let deadline = Instant::now() + TEST_TIMEOUT;
    loop {
        if let Ok(coordinates) = runtime::subagent_panes::ChannelCoordinates::read(&channel) {
            if let Ok(client) = Client::connect(&coordinates.addr, coordinates.token.clone()).await {
                return (client, coordinates.session_id);
            }
        }
        assert!(
            Instant::now() < deadline,
            "the teammate never published its channel file at {}",
            channel.display()
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Wait for the result of turn `turn` the way a parent's watcher does.
fn wait_for_turn_result(
    directory: &std::path::Path,
    turn: u32,
    timeout: Duration,
) -> runtime::subagent_panes::TeammateResult {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(result) = runtime::subagent_panes::TeammateResult::read_turn(directory, turn) {
            return result;
        }
        assert!(
            Instant::now() < deadline,
            "the teammate wrote no result for turn {turn} in {timeout:?}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Wait for the closing document.
fn wait_for_closing(
    directory: &std::path::Path,
    timeout: Duration,
) -> runtime::subagent_panes::TeammateResult {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(result) = runtime::subagent_panes::TeammateResult::read_final(directory) {
            return result;
        }
        assert!(
            Instant::now() < deadline,
            "the teammate wrote no closing document in {timeout:?}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The `system` blocks of one captured request, flattened to text.
fn system_prompt_of(body: &str) -> String {
    let value: serde_json::Value = serde_json::from_str(body).expect("request body is JSON");
    match &value["system"] {
        serde_json::Value::String(text) => text.clone(),
        serde_json::Value::Array(blocks) => blocks
            .iter()
            .filter_map(|block| block["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// The whole life of a teammate pane (design t-2513 §4 tests 2·3·4):
///
/// 1. the brief's turn runs with the CARRIED harness (the request's system
///    prompt is the canary, not this binary's root prompt);
/// 2. a `session.steer` during that turn's long tool is `consumed` with the
///    turn id, and the words reach the model at the tool boundary;
/// 3. `result.json` appears and the process STAYS, idle, saying so;
/// 4. a `session.steer` while idle is `queued` and opens turn 2, whose
///    `result-2.json` appears and whose request carries turn 1's context;
/// 5. the parent's channel vanishing ends the child within the grace, with
///    `result-final.json{exit: closed, reason: parent_lost}` and a clean exit;
/// 6. a steer at the dead channel is `rejected` with the reason.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)] // one life, six numbered steps — split, the timeline loses its order
async fn e2e_a_teammate_stays_idle_takes_the_parents_next_word_and_closes_when_the_parent_is_gone() {
    use runtime::subagent_panes::{steer_over_channel, CloseReason, Exit, SteerReceipt, CHANNEL_FILE};

    let layout = Layout::new();
    layout.with_subagent_limits(&serde_json::json!({
        "parentLivenessPollMs": 200,
        "parentLivenessGraceMs": 1000,
        "channelTimeoutMs": 500,
    }));
    let service = ScriptedAnthropicService::slow_tool("slow answer done\n")
        .await
        .expect("start slow-tool script");
    let directory = layout.root.path().join("agent-life-1");
    fs::create_dir_all(&directory).expect("teammate directory");
    let parent = FakeParent::start(layout.root.path().join("zo-events-parent.addr"));
    teammate_brief_v2(
        "agent-life-1",
        "Run the slow survey.",
        "general-purpose",
        "danger-full-access",
        Some(parent.discovery.clone()),
        Some(60_000),
    )
    .write(&directory)
    .expect("write brief");

    let started = Instant::now();
    let mut timeline: Vec<(&str, Duration)> = Vec::new();
    let dir_arg = directory.to_string_lossy().into_owned();
    let mut run = pty(&layout, service.base_url(), &["--teammate", &dir_arg]);
    run.wait_for(TEAMMATE_PARENT_SESSION, TEST_TIMEOUT);
    let (mut client, child_session) = connect_to_teammate(&directory).await;
    timeline.push(("channel file published", started.elapsed()));

    let capabilities = client.call("session.capabilities", serde_json::json!({})).await.unwrap();
    assert_eq!(capabilities["identity"]["execution"], "pane");
    assert_eq!(capabilities["identity"]["parent_session_id"], TEAMMATE_PARENT_SESSION);
    assert_eq!(capabilities["launch"]["harness"], "carried");
    assert!(
        capabilities["support"]["steer"]["scope"]
            .as_str()
            .is_some_and(|scope| scope.contains("idle-next-turn")),
        "{capabilities}"
    );
    assert_eq!(capabilities["support"]["subagent_steer"]["supported"], true);

    // 2. The first turn is inside its slow bash: steer lands in it.
    wait_for_request_count(&service, 1).await;
    let consumed = client
        .call(
            method::STEER,
            serde_json::json!({"id": child_session, "text": "[message via SendMessage] also count the nodes"}),
        )
        .await
        .expect("steer a running turn");
    assert_eq!(consumed["receipt"], SteerReceipt::Consumed.as_str(), "{consumed}");
    assert!(consumed["turn_id"].is_u64(), "{consumed}");
    timeline.push(("steer during turn 1 → consumed", started.elapsed()));

    // 3. The first answer, with the carried harness and the folded steer.
    let first = wait_for_turn_result(&directory, 1, Duration::from_secs(30));
    timeline.push(("result.json", started.elapsed()));
    assert_eq!(first.exit, Exit::Ok, "{first:?}");
    assert_eq!(first.turn, Some(1));
    assert!(first.final_message.contains("slow answer done"), "{first:?}");
    wait_for_request_count(&service, 2).await;
    let bodies = service.request_bodies().await;
    let system = system_prompt_of(&bodies[0]);
    assert!(system.contains(TEAMMATE_HARNESS_CANARY), "the carried harness was not the system prompt: {system}");
    assert!(
        !system.contains("You are Claude Code") || system.starts_with(TEAMMATE_HARNESS_CANARY),
        "the child re-derived a root prompt instead of running the carried one"
    );
    assert!(bodies[1].contains("also count the nodes"), "the consumed steer never reached the model");
    run.wait_for(teammate_idle_line_head(), TEST_TIMEOUT);
    assert!(run.rss_kib().is_some(), "the teammate left after one turn");

    // 4. Idle: the parent's next word is queued and opens turn 2.
    let queued = client
        .call(
            method::STEER,
            serde_json::json!({"id": child_session, "text": "[message via SendMessage] second question"}),
        )
        .await
        .expect("steer an idle teammate");
    assert_eq!(queued["receipt"], SteerReceipt::Queued.as_str(), "{queued}");
    assert!(queued["turn_id"].is_null(), "{queued}");
    timeline.push(("steer while idle → queued", started.elapsed()));
    let second = wait_for_turn_result(&directory, 2, Duration::from_secs(30));
    timeline.push(("result-2.json", started.elapsed()));
    assert_eq!(second.exit, Exit::Ok, "{second:?}");
    assert_eq!(second.turn, Some(2));
    wait_for_request_count(&service, 3).await;
    let bodies = service.request_bodies().await;
    assert!(bodies[2].contains("second question"), "turn 2 did not carry the parent's words");
    assert!(
        bodies[2].contains("Run the slow survey."),
        "turn 2 lost turn 1's context — it is a different conversation"
    );
    assert!(run.rss_kib().is_some(), "the teammate left after its second turn");

    // 5. The parent is gone: closed within the grace, cleanly.
    parent.vanish();
    timeline.push(("parent channel gone", started.elapsed()));
    let closing = wait_for_closing(&directory, Duration::from_secs(15));
    timeline.push(("result-final.json", started.elapsed()));
    assert_eq!(closing.exit, Exit::Closed);
    assert_eq!(closing.reason, Some(CloseReason::ParentLost));
    assert_eq!(closing.turn, Some(2), "two turns were answered");
    assert!(run.exit_code(Duration::from_secs(10)).is_some(), "the closed teammate did not exit");
    timeline.push(("process exit", started.elapsed()));
    assert!(!directory.join(CHANNEL_FILE).exists(), "a closed teammate left its channel file");

    // 6. Nobody home: the parent's steer is rejected, with the reason.
    let rejected = steer_over_channel(&directory.join(CHANNEL_FILE), "anyone?", Duration::from_millis(500));
    assert_eq!(rejected.receipt, SteerReceipt::Rejected);
    assert!(rejected.reason.is_some());
    for (label, at) in &timeline {
        println!("lifecycle-timeline | {label} | {:.2}s", at.as_secs_f64());
    }
    let _ = run.finish();
}

/// The idle budget ends a forgotten teammate (design §4 test 3, second half).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_a_teammate_closes_itself_when_its_idle_budget_runs_out() {
    use runtime::subagent_panes::{CloseReason, Exit};

    let layout = Layout::new();
    let service = ScriptedAnthropicService::text("quick answer\n").await.expect("start script");
    let directory = layout.root.path().join("agent-idle-1");
    fs::create_dir_all(&directory).expect("teammate directory");
    let parent = FakeParent::start(layout.root.path().join("zo-events-parent.addr"));
    teammate_brief_v2(
        "agent-idle-1",
        "Answer quickly.",
        "Explore",
        "read-only",
        Some(parent.discovery.clone()),
        Some(1_500),
    )
    .write(&directory)
    .expect("write brief");
    let dir_arg = directory.to_string_lossy().into_owned();
    let mut run = pty(&layout, service.base_url(), &["--teammate", &dir_arg]);
    let first = wait_for_turn_result(&directory, 1, Duration::from_secs(20));
    assert_eq!(first.exit, Exit::Ok);
    let idle_since = Instant::now();
    let closing = wait_for_closing(&directory, Duration::from_secs(15));
    let waited = idle_since.elapsed();
    assert_eq!(closing.exit, Exit::Closed);
    assert_eq!(closing.reason, Some(CloseReason::IdleBudget));
    assert!(waited >= Duration::from_millis(1_000), "closed before the budget: {waited:?}");
    assert!(run.exit_code(Duration::from_secs(10)).is_some());
    println!("idle-budget | closed after {:.2}s of idling (budget 1.5s)", waited.as_secs_f64());
    drop(parent);
    let _ = run.finish();
}

/// The parent closes an idle teammate over its channel — the polite road a
/// `StopAgent` takes before `kill-pane` (design §2.2).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_a_parent_closes_an_idle_teammate_over_its_channel() {
    use runtime::subagent_panes::{channel_method, CloseReason, Exit};

    let layout = Layout::new();
    let service = ScriptedAnthropicService::text("quick answer\n").await.expect("start script");
    let directory = layout.root.path().join("agent-close-1");
    fs::create_dir_all(&directory).expect("teammate directory");
    let parent = FakeParent::start(layout.root.path().join("zo-events-parent.addr"));
    teammate_brief_v2(
        "agent-close-1",
        "Answer quickly.",
        "Explore",
        "read-only",
        Some(parent.discovery.clone()),
        Some(60_000),
    )
    .write(&directory)
    .expect("write brief");
    let dir_arg = directory.to_string_lossy().into_owned();
    let mut run = pty(&layout, service.base_url(), &["--teammate", &dir_arg]);
    let first = wait_for_turn_result(&directory, 1, Duration::from_secs(20));
    assert_eq!(first.exit, Exit::Ok);
    let (mut client, _child_session) = connect_to_teammate(&directory).await;
    // A Stop with nothing running is not an error …
    let stopped = client.call(method::CANCEL_TURN, serde_json::json!({})).await.unwrap();
    assert_eq!(stopped["cancelled"], false);
    // … and the door is answered.
    let closed = client
        .call(channel_method::TEAMMATE_CLOSE, serde_json::json!({"reason": "closed_by_parent"}))
        .await
        .expect("teammate.close");
    assert_eq!(closed["closing"], true, "{closed}");
    let closing = wait_for_closing(&directory, Duration::from_secs(10));
    assert_eq!(closing.exit, Exit::Closed);
    assert_eq!(closing.reason, Some(CloseReason::ClosedByParent));
    assert!(run.exit_code(Duration::from_secs(10)).is_some());
    drop(parent);
    let _ = run.finish();
}

/// A re-cut teammate continues the transcript its parent named (design §4
/// test 4): the second turn's request carries the first turn's words, the
/// system prompt is the carried harness (re-resolved zero times), and the
/// result is numbered where the parent's count left off.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_a_re_cut_teammate_continues_the_transcript_it_was_given() {
    use runtime::subagent_panes::{Brief, CloseReason, Exit};

    let layout = Layout::new();
    let service = ScriptedAnthropicService::text("the survey says yes\n").await.expect("start script");
    let directory = layout.root.path().join("agent-resume-1");
    fs::create_dir_all(&directory).expect("teammate directory");
    let parent = FakeParent::start(layout.root.path().join("zo-events-parent.addr"));
    // Life 1: one turn, then the idle budget closes the pane.
    teammate_brief_v2(
        "agent-resume-1",
        "First question: how many edges?",
        "Explore",
        "read-only",
        Some(parent.discovery.clone()),
        Some(800),
    )
    .write(&directory)
    .expect("write brief");
    let dir_arg = directory.to_string_lossy().into_owned();
    let mut first_life = pty(&layout, service.base_url(), &["--teammate", &dir_arg]);
    let first = wait_for_turn_result(&directory, 1, Duration::from_secs(20));
    assert_eq!(first.exit, Exit::Ok);
    let transcript = first.transcript.clone().expect("the result names the child's transcript");
    let closing = wait_for_closing(&directory, Duration::from_secs(15));
    assert_eq!(closing.reason, Some(CloseReason::IdleBudget));
    assert!(first_life.exit_code(Duration::from_secs(10)).is_some());
    let _ = first_life.finish();

    // Life 2: the parent's resume re-cuts a pane on the SAME transcript,
    // numbering the next turn where its count left off.
    let mut brief = teammate_brief_v2(
        "agent-resume-1",
        "Second question: and nodes?",
        "Explore",
        "read-only",
        Some(parent.discovery.clone()),
        Some(60_000),
    );
    brief.resume_transcript = Some(transcript.clone());
    brief.first_turn = Some(2);
    let _ = fs::remove_file(directory.join(runtime::subagent_panes::RESULT_FINAL_FILE));
    brief.write(&directory).expect("write resume brief");
    assert_eq!(Brief::read(&directory).unwrap().first_turn(), 2);
    let transcript_arg = transcript.to_string_lossy().into_owned();
    let second_life = pty(
        &layout,
        service.base_url(),
        &["--teammate", &dir_arg, "--resume-transcript", &transcript_arg],
    );
    let second = wait_for_turn_result(&directory, 2, Duration::from_secs(20));
    assert_eq!(second.exit, Exit::Ok, "{second:?}");
    assert_eq!(second.turn, Some(2));
    assert_eq!(
        second.transcript.as_deref(),
        Some(transcript.as_path()),
        "the re-cut child wrote a different transcript"
    );
    wait_for_request_count(&service, 2).await;
    let bodies = service.request_bodies().await;
    assert!(
        bodies[1].contains("First question: how many edges?"),
        "the re-cut child did not continue the transcript: {}",
        &bodies[1][..bodies[1].len().min(400)]
    );
    assert!(bodies[1].contains("Second question: and nodes?"));
    // Zero re-resolution: both lives were shown the same carried harness.
    assert_eq!(system_prompt_of(&bodies[0]), system_prompt_of(&bodies[1]));
    assert!(system_prompt_of(&bodies[1]).contains(TEAMMATE_HARNESS_CANARY));
    let (mut client, _) = connect_to_teammate(&directory).await;
    let capabilities = client.call("session.capabilities", serde_json::json!({})).await.unwrap();
    assert_eq!(capabilities["launch"]["harness"], "carried");
    drop(parent);
    let _ = second_life.finish();
}

/// A pane child opens its PARENT's registry (design §4 test 5): the record
/// the brief names, under the parent's session id, so its boot-time reap
/// settles a dead-owner manifest in the parent's root and the capability
/// snapshot names that locator.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_a_teammate_opens_its_parents_registry_and_reaps_into_the_parents_root() {
    use runtime::subagent_panes::Exit;

    let layout = Layout::new();
    let service = ScriptedAnthropicService::text("registry answer\n").await.expect("start script");
    let store = layout.root.path().join("agents");
    fs::create_dir_all(store.join("registries")).expect("store");
    let store = fs::canonicalize(&store).expect("canonical store");
    // The parent's record, as `AgentRegistry::open_for_session` persists it.
    let locator = tools::registry_locator_for(&store, TEAMMATE_PARENT_SESSION);
    let record = tools::RegistryRecord {
        session_id: TEAMMATE_PARENT_SESSION.to_string(),
        origin_cwd: layout.cwd.display().to_string(),
        root: store.display().to_string(),
        mirrors: Vec::new(),
        generation: 3,
        created_at: 1,
        last_execution_cwd: None,
        adoption_incomplete: false,
    };
    fs::write(&locator, serde_json::to_vec_pretty(&record).unwrap()).expect("write record");
    // A dead-owner running manifest of the parent's — the reap's quarry.
    let orphan = store.join("agent-orphan-1.json");
    fs::write(store.join("agent-orphan-1.md"), "# Agent\n").expect("orphan output");
    fs::write(
        &orphan,
        serde_json::json!({
            "agentId": "agent-orphan-1",
            "parentSessionId": TEAMMATE_PARENT_SESSION,
            "name": "orphan",
            "description": "left behind",
            "subagentType": "Explore",
            "model": null,
            "status": "running",
            "outputFile": store.join("agent-orphan-1.md"),
            "manifestFile": orphan,
            "createdAt": "100",
            "ownerPid": u32::MAX - 9,
            "startedAt": "100"
        })
        .to_string(),
    )
    .expect("write orphan manifest");

    let directory = store.join("agent-registry-1");
    fs::create_dir_all(&directory).expect("teammate directory");
    let parent = FakeParent::start(layout.root.path().join("zo-events-parent.addr"));
    let mut brief = teammate_brief_v2(
        "agent-registry-1",
        "Answer.",
        "Explore",
        "read-only",
        Some(parent.discovery.clone()),
        Some(60_000),
    );
    brief.registry_locator = Some(locator.clone());
    brief.write(&directory).expect("write brief");
    let dir_arg = directory.to_string_lossy().into_owned();
    let store_arg = store.to_string_lossy().into_owned();
    let run = PtyRun::spawn_with_env(
        &layout.cwd,
        &layout.home,
        &layout.sessions,
        &layout.state,
        service.base_url(),
        &["--teammate", &dir_arg],
        &[("ZO_AGENT_STORE", &store_arg)],
    )
    .expect("spawn teammate");
    let first = wait_for_turn_result(&directory, 1, Duration::from_secs(20));
    assert_eq!(first.exit, Exit::Ok);
    let (mut client, _) = connect_to_teammate(&directory).await;
    let capabilities = client.call("session.capabilities", serde_json::json!({})).await.unwrap();
    assert_eq!(
        capabilities["identity"]["registry_locator"],
        serde_json::json!(locator.display().to_string())
    );
    // The child's reap reached the PARENT's root.
    let deadline = Instant::now() + TEST_TIMEOUT;
    loop {
        let stored: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&orphan).unwrap()).unwrap();
        if stored["status"] == "stopped" {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the teammate never reaped the parent's dead-owner manifest: {stored}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    // And the record it opened is the parent's, untouched in identity.
    let record: tools::RegistryRecord =
        serde_json::from_str(&fs::read_to_string(&locator).unwrap()).unwrap();
    assert_eq!(record.session_id, TEAMMATE_PARENT_SESSION);
    assert_eq!(record.root, store.display().to_string());
    drop(parent);
    let _ = run.finish();
}

/// A pane child's approval rides ITS channel and the answer continues its
/// turn; the parent's `auth.reload` fan-out reaches the child over the same
/// channel and the child says which account it now speaks for (design §4
/// test 6).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_a_teammates_approval_rides_its_own_channel_and_auth_reload_reaches_it() {
    use runtime::subagent_panes::{channel_method, Exit, CHANNEL_FILE};
    use zerocode_harness::Incoming;

    let layout = Layout::new();
    // Turn 1 answers clean; turn 2 — opened over the channel after the test
    // has subscribed — runs a write-shaped bash that parks on an approval.
    let service = ScriptedAnthropicService::text_then_bash(
        "ready for the command\n",
        "touch approval-canary.txt",
        "bash ran\n",
    )
    .await
    .expect("start text-then-bash script");
    let directory = layout.root.path().join("agent-approve-1");
    fs::create_dir_all(&directory).expect("teammate directory");
    let parent = FakeParent::start(layout.root.path().join("zo-events-parent.addr"));
    // `prompt` mode — a real mode a custom role may carry: every tool asks.
    teammate_brief_v2(
        "agent-approve-1",
        "Say you are ready.",
        "general-purpose",
        "prompt",
        Some(parent.discovery.clone()),
        Some(60_000),
    )
    .write(&directory)
    .expect("write brief");
    let dir_arg = directory.to_string_lossy().into_owned();
    let mut run = pty(&layout, service.base_url(), &["--teammate", &dir_arg]);
    let ready = wait_for_turn_result(&directory, 1, Duration::from_secs(20));
    assert_eq!(ready.exit, Exit::Ok, "{ready:?}");
    let (mut client, child_session) = connect_to_teammate(&directory).await;
    client.subscribe(&child_session, true).await.expect("subscribe to the child");
    let opened = client
        .call(
            method::STEER,
            serde_json::json!({"id": child_session, "text": "[message via SendMessage] Run the command."}),
        )
        .await
        .expect("open turn 2");
    assert_eq!(opened["receipt"], "queued", "{opened}");
    // The child's own channel carries the approval.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    let prompt = loop {
        let incoming = tokio::time::timeout_at(deadline, client.next_incoming())
            .await
            .expect("a permission_prompt frame within the deadline")
            .expect("read")
            .expect("stream stayed open");
        if let Incoming::Frame(frame) = incoming {
            if frame["type"] == "permission_prompt" {
                break frame;
            }
        }
    };
    assert_eq!(prompt["tool_name"].as_str().map(str::to_ascii_lowercase).as_deref(), Some("bash"), "{prompt}");
    let prompt_id = prompt["prompt_id"].as_u64().expect("prompt id");
    // Answered on a second connection, as the window does.
    let (mut answerer, _) = connect_to_teammate(&directory).await;
    answerer
        .respond_to_permission(prompt_id, "allow_once")
        .await
        .expect("permission.respond");
    let second = wait_for_turn_result(&directory, 2, Duration::from_secs(30));
    assert_eq!(second.exit, Exit::Ok, "{second:?}");
    assert!(second.final_message.contains("bash ran"), "{second:?}");
    assert_eq!(second.usage.tool_calls, 1, "the approved tool ran once");
    assert!(
        layout.cwd.join("approval-canary.txt").exists(),
        "the approved command did not run in the child's cwd"
    );

    // The parent's fan-out road: the child's channel file, as
    // `tools::live_pane_children` hands it over, and one `auth.reload` call.
    let child = tools::PaneChildChannel {
        agent_id: "agent-approve-1".to_string(),
        channel: directory.join(CHANNEL_FILE),
    };
    let reloaded = child
        .call(
            channel_method::AUTH_RELOAD,
            serde_json::json!({"provider": "anthropic", "label": "work-account"}),
        )
        .expect("auth.reload reaches the child");
    assert_eq!(reloaded["reloaded"], serde_json::json!(["anthropic"]));
    assert_eq!(reloaded["account"]["label"], "work-account");
    // The child says so on its screen — the same notice a root pane shows.
    run.wait_for("계정 전환 · work-account", TEST_TIMEOUT);
    drop(parent);
    let _ = run.finish();
}

/// A pane child's MCP call goes to its PARENT's runtime over the parent's
/// channel (design §2.1: `mcp` is a routing coordinate, and the inline
/// passthrough's road with a socket in the middle): the harness advertises
/// the parent's tool, the model calls it, the parent hears `mcp.call` with
/// the tool's name and input, and the answer is the tool result the child's
/// next request carries.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_a_teammates_mcp_call_is_answered_by_its_parent_over_the_channel() {
    use runtime::subagent_panes::{channel_method, Exit, McpRoute, McpTool};

    let layout = Layout::new();
    let service = ScriptedAnthropicService::mcp_tool("mcp__ctx7__query", "the docs say yes\n")
        .await
        .expect("start mcp script");
    let directory = layout.root.path().join("agent-mcp-1");
    fs::create_dir_all(&directory).expect("teammate directory");
    let parent = FakeParent::start(layout.root.path().join("zo-events-parent.addr"));
    let mut brief = teammate_brief_v2(
        "agent-mcp-1",
        "Look tokio up in the docs.",
        "general-purpose",
        "danger-full-access",
        Some(parent.discovery.clone()),
        Some(60_000),
    );
    {
        let harness = brief.harness.as_mut().expect("harness");
        harness.allowed_tools.insert("mcp__ctx7__query".to_string());
        harness.mcp = Some(McpRoute {
            tools: vec![McpTool {
                name: "mcp__ctx7__query".to_string(),
                description: Some("docs lookup".to_string()),
                input_schema: serde_json::json!({"type": "object", "properties": {"q": {"type": "string"}}}),
                required_permission: Some("read-only".to_string()),
            }],
            channel: Some(parent.discovery.clone()),
            method: channel_method::MCP_CALL.to_string(),
        });
    }
    brief.write(&directory).expect("write brief");
    let dir_arg = directory.to_string_lossy().into_owned();
    let run = pty(&layout, service.base_url(), &["--teammate", &dir_arg]);
    let first = wait_for_turn_result(&directory, 1, Duration::from_secs(30));
    assert_eq!(first.exit, Exit::Ok, "{first:?}");
    assert!(first.final_message.contains("the docs say yes"), "{first:?}");
    assert_eq!(first.usage.tool_calls, 1, "the MCP tool ran once");
    // The parent heard the call, with the tool's name and input …
    let calls: Vec<serde_json::Value> = parent
        .asked()
        .into_iter()
        .filter(|request| request["method"] == "mcp.call")
        .collect();
    assert_eq!(calls.len(), 1, "the parent heard {} mcp.call(s)", calls.len());
    assert_eq!(calls[0]["params"]["name"], "mcp__ctx7__query");
    assert_eq!(calls[0]["params"]["input"]["q"], "tokio");
    assert_eq!(calls[0]["token"], "parent-token", "the parent's token rides the call");
    // … the model was offered the parent's tool, and its answer came back as
    // the tool result.
    wait_for_request_count(&service, 2).await;
    let bodies = service.request_bodies().await;
    assert!(bodies[0].contains("\"mcp__ctx7__query\""), "the parent's MCP tool was not advertised");
    assert!(
        bodies[1].contains("PARENT-MCP-ANSWER for tokio"),
        "the parent's answer did not reach the model as the tool result"
    );
    drop(parent);
    let _ = run.finish();
}

/// `Agent{subagent_type: "fork"}` inherits the PARENT's conversation (t-2875,
/// Claude Code's fork contract). The parent reads `fixture.txt`, then asks
/// the same question of a fork and of a plain agent, blocking on each:
///
/// - the fork answers from the inherited `read_file` result with ZERO tool
///   calls — one provider request, carrying the parent's typed prompt, the
///   parent's read and the parent's system prompt verbatim;
/// - it runs on the parent's model although the call named another one;
/// - the plain agent, given the same question in a fresh context, has to
///   call `read_file` first — two requests.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_a_fork_answers_from_the_parents_context_while_a_plain_agent_reads_again() {
    use e2e::scripted::{
        body_text_contains, has_tool_result_id, user_text_contains, CHILD_READ_ID,
        FORK_ANSWER_AFTER_READ, FORK_ANSWER_FROM_CONTEXT, FORK_FIXTURE_MARKER, FORK_IGNORED_MODEL,
        FORK_QUESTION, FORK_SPAWN_ID, PARENT_READ_ID,
    };

    let layout = Layout::new();
    layout.model_led();
    layout.fixture(&format!("first line\n{FORK_FIXTURE_MARKER}\nlast line\n"));
    let service = ScriptedAnthropicService::fork_inherits_parent_reading(
        "### Fork scenario done\n\n- both children answered\n",
    )
    .await
    .expect("start fork script");
    let mut run = pty(&layout, service.base_url(), &interactive_args());

    run.wait_for("directory:", TEST_TIMEOUT);
    run.send(b"Read fixture.txt, then delegate the marker question twice\r")
        .expect("send fork prompt");
    run.wait_for("both children answered", Duration::from_secs(45));

    let bodies = service.request_bodies().await;
    let parsed: Vec<serde_json::Value> = bodies
        .iter()
        .map(|body| serde_json::from_str(body).expect("request body is JSON"))
        .collect();
    let is_child = |body: &serde_json::Value| user_text_contains(body, FORK_QUESTION);
    let parent: Vec<usize> = (0..parsed.len()).filter(|&i| !is_child(&parsed[i])).collect();
    let fork: Vec<usize> = (0..parsed.len())
        .filter(|&i| is_child(&parsed[i]) && has_tool_result_id(&parsed[i], FORK_SPAWN_ID))
        .collect();
    let plain: Vec<usize> = (0..parsed.len())
        .filter(|&i| is_child(&parsed[i]) && !has_tool_result_id(&parsed[i], FORK_SPAWN_ID))
        .collect();
    assert_eq!(
        parent.len(),
        4,
        "the parent runs read → fork → plain → final; saw {} requests in all",
        parsed.len()
    );
    assert_eq!(
        fork.len(),
        1,
        "a fork inherits the parent's read and answers at once — every extra request is a tool \
         call it should not have needed ({} fork requests, {} in all)",
        fork.len(),
        parsed.len()
    );
    let fork_body = &parsed[fork[0]];
    assert!(
        has_tool_result_id(fork_body, PARENT_READ_ID)
            && body_text_contains(fork_body, FORK_FIXTURE_MARKER),
        "the fork's request does not carry the parent's read_file result"
    );
    assert!(
        user_text_contains(fork_body, "Read fixture.txt, then delegate"),
        "the fork's request does not carry the parent's typed prompt"
    );
    assert_eq!(
        system_prompt_of(&bodies[fork[0]]),
        system_prompt_of(&bodies[parent[1]]),
        "the fork runs under a system prompt other than its parent's"
    );
    assert_eq!(
        fork_body["model"], parsed[parent[1]]["model"],
        "a fork runs on the parent's model — the call's `model` is ignored"
    );
    assert_ne!(fork_body["model"].as_str(), Some(FORK_IGNORED_MODEL));

    assert_eq!(
        plain.len(),
        2,
        "a plain agent with the same question reads the file: one request to call read_file, \
         one carrying its result"
    );
    assert!(
        !body_text_contains(&parsed[plain[0]], FORK_FIXTURE_MARKER),
        "a plain agent starts without the parent's context"
    );
    assert!(has_tool_result_id(&parsed[plain[1]], CHILD_READ_ID));
    // The parent heard both answers, each from its own road.
    assert!(body_text_contains(&parsed[parent[2]], FORK_ANSWER_FROM_CONTEXT));
    assert!(body_text_contains(&parsed[parent[3]], FORK_ANSWER_AFTER_READ));
    let _ = run.finish();
}

// ── t-2901: the two scheduling tools open a turn when the session is idle ────
//
// Before this wave `CronCreate` answered `automatic_scheduler_status:
// not_wired` and `ScheduleWakeup` wrote a file nobody read — both promised a
// future turn that never came (t-2877 axes H1/H2). These three scenarios run
// the real binary against the scripted provider and wait, on the wall clock,
// for the turn the tool promised.

fn pty_with_env(layout: &Layout, base_url: &str, args: &[&str], env: &[(&str, &str)]) -> PtyRun {
    PtyRun::spawn_with_env(
        &layout.cwd,
        &layout.home,
        &layout.sessions,
        &layout.state,
        base_url,
        args,
        env,
    )
    .expect("spawn zo in PTY")
}

fn unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock after the epoch")
        .as_secs()
}

fn parse_body(body: &str) -> serde_json::Value {
    serde_json::from_str(body).expect("request body is JSON")
}

/// Poll the scripted provider until `count` requests have arrived.
async fn wait_for_requests(
    service: &ScriptedAnthropicService,
    count: usize,
    deadline: Instant,
) -> Vec<String> {
    loop {
        let bodies = service.request_bodies().await;
        if bodies.len() >= count {
            return bodies;
        }
        assert!(
            Instant::now() < deadline,
            "only {} of {count} requests arrived before the deadline",
            bodies.len()
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// The five-field expression that names exactly the local minute `secs` falls
/// in — zo's cron registry reads a schedule against the session's own wall
/// clock (`cron_matching_reads_the_zones_wall_clock_with_vixie_semantics`).
fn cron_expression_for_local_minute(secs: u64) -> String {
    // Howard Hinnant's civil-from-days, the same arithmetic the registry uses,
    // over the same wall-clock shift the registry applies before it.
    let secs = secs.saturating_add_signed(core_types::date::local_utc_offset_secs());
    let days = i64::try_from(secs / 86_400).expect("day count fits");
    let z = days + 719_468;
    // The year is not needed for a five-field expression, only the era's day.
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let minute = (secs / 60) % 60;
    let hour = (secs / 3_600) % 24;
    format!("{minute} {hour} {day} {month} *")
}

/// H₁: a cron aimed at the next minute fires within five seconds of it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_cron_aimed_at_the_next_minute_fires_within_five_seconds() {
    const MARKER: &str = "CRON_TURN_MARKER_T2901";
    let now = unix_secs();
    // The boot card and the registering turn must land before the minute the
    // cron names; near the boundary aim one minute further.
    let mut boundary = now - now % 60 + 60;
    if now % 60 >= 45 {
        boundary += 60;
    }
    let schedule = cron_expression_for_local_minute(boundary);
    let service = ScriptedAnthropicService::tool_calls(
        &[(
            "CronCreate",
            serde_json::json!({
                "schedule": schedule,
                "prompt": format!("{MARKER} report that the cron fired"),
                "description": "t-2901 one-shot"
            }),
        )],
        "cron registered",
    )
    .await
    .expect("start cron script");
    let layout = Layout::new();
    let args = interactive_args();
    let mut run = pty(&layout, service.base_url(), &args);
    run.wait_for("directory:", TEST_TIMEOUT);
    run.send(b"register the cron\r").expect("send prompt");
    run.wait_for("cron registered", TEST_TIMEOUT);

    let bodies = service.request_bodies().await;
    assert_eq!(bodies.len(), 2, "the registering turn is one tool call and one answer");
    let receipt = parse_body(&bodies[1]);
    assert!(
        !e2e::scripted::body_text_contains(&receipt, "not_wired"),
        "the CronCreate receipt still claims the scheduler is not wired:\n{}",
        bodies[1]
    );
    assert!(
        e2e::scripted::body_text_contains(&receipt, "session_idle"),
        "the CronCreate receipt does not name the session scheduler:\n{}",
        bodies[1]
    );

    let deadline = Instant::now()
        + Duration::from_secs((boundary + 5).saturating_sub(unix_secs()));
    let bodies = wait_for_requests(&service, 3, deadline).await;
    let fired_at = unix_secs();
    assert!(
        e2e::scripted::user_text_contains(&parse_body(&bodies[2]), MARKER),
        "the fired turn does not carry the cron's prompt:\n{}",
        bodies[2]
    );
    assert!(
        fired_at >= boundary,
        "the cron fired at {fired_at}, before its minute {boundary}"
    );
    run.wait_for("autonomous → cron", TEST_TIMEOUT);
    let _ = run.finish();
}

/// H₂: a 45-second `ScheduleWakeup` fires once, inside a 90-second window.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_schedule_wakeup_fires_once_inside_ninety_seconds() {
    const MARKER: &str = "WAKEUP_TURN_MARKER_T2901";
    let service = ScriptedAnthropicService::tool_calls(
        &[(
            "ScheduleWakeup",
            serde_json::json!({
                "delaySeconds": 45,
                "reason": "poll the build",
                "prompt": format!("{MARKER} the build should be done by now")
            }),
        )],
        "wakeup scheduled",
    )
    .await
    .expect("start wakeup script");
    let layout = Layout::new();
    let args = interactive_args();
    // The limits table's floor is a minute; the scenario asks for 45 s and
    // wants that honoured, so lower the floor through the table's own knob.
    let mut run = pty_with_env(
        &layout,
        service.base_url(),
        &args,
        &[("ZO_AUTONOMY_MIN_MODEL_WAKEUP_SECS", "1")],
    );
    run.wait_for("directory:", TEST_TIMEOUT);
    let asked = Instant::now();
    run.send(b"check back on the build later\r").expect("send prompt");
    run.wait_for("wakeup scheduled", TEST_TIMEOUT);

    let bodies = service.request_bodies().await;
    assert_eq!(bodies.len(), 2, "the scheduling turn is one tool call and one answer");
    assert!(
        e2e::scripted::body_text_contains(&parse_body(&bodies[1]), "session_idle"),
        "the ScheduleWakeup receipt does not name the session scheduler:\n{}",
        bodies[1]
    );
    // The scheduler adopted the record and says when it fires.
    run.wait_for("wakeup: loop-1 armed", TEST_TIMEOUT);

    let bodies = wait_for_requests(&service, 3, asked + Duration::from_secs(90)).await;
    let elapsed = asked.elapsed();
    assert!(
        elapsed >= Duration::from_secs(44),
        "the wakeup fired after {elapsed:?}, before the 45 s it asked for"
    );
    assert!(
        e2e::scripted::user_text_contains(&parse_body(&bodies[2]), MARKER),
        "the fired turn does not carry the stored prompt:\n{}",
        bodies[2]
    );
    // The fired turn scheduled nothing, so the loop ends and nothing else fires.
    run.wait_for("no further wakeup", TEST_TIMEOUT);
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(
        service.request_bodies().await.len(),
        3,
        "a single ScheduleWakeup fired more than once"
    );
    let _ = run.finish();
}

/// A wakeup the model stops in the same turn never fires.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_stopped_wakeup_never_fires() {
    const MARKER: &str = "WAKEUP_NEVER_MARKER_T2901";
    let service = ScriptedAnthropicService::tool_calls(
        &[
            (
                "ScheduleWakeup",
                serde_json::json!({
                    "delaySeconds": 2,
                    "reason": "look again shortly",
                    "prompt": format!("{MARKER} this must never open a turn")
                }),
            ),
            ("ScheduleWakeup", serde_json::json!({"stop": true})),
        ],
        "wakeup cancelled",
    )
    .await
    .expect("start stop script");
    let layout = Layout::new();
    let args = interactive_args();
    let mut run = pty_with_env(
        &layout,
        service.base_url(),
        &args,
        &[("ZO_AUTONOMY_MIN_MODEL_WAKEUP_SECS", "1")],
    );
    run.wait_for("directory:", TEST_TIMEOUT);
    run.send(b"schedule then cancel\r").expect("send prompt");
    run.wait_for("wakeup cancelled", TEST_TIMEOUT);

    let bodies = service.request_bodies().await;
    assert_eq!(bodies.len(), 3, "two tool calls and one answer");
    assert!(
        e2e::scripted::body_text_contains(&parse_body(&bodies[2]), "\"stopped\": true"),
        "the stop call was not honoured:\n{}",
        bodies[2]
    );
    // The scheduler saw both records in order: armed, then stopped.
    run.wait_for("wakeup: loop-1 stopped", TEST_TIMEOUT);

    tokio::time::sleep(Duration::from_secs(8)).await;
    let bodies = service.request_bodies().await;
    assert_eq!(bodies.len(), 3, "a stopped wakeup opened a turn");
    assert!(
        !bodies
            .iter()
            .any(|body| e2e::scripted::user_text_contains(&parse_body(body), MARKER)),
        "the stopped wakeup's prompt reached the provider"
    );
    let _ = run.finish();
}

/// The append-only frontend runs the same driver: a wakeup scheduled from a
/// `--plain` session fires there too, and the notices are its system lines.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_plain_frontend_fires_a_wakeup_from_the_same_engine() {
    const MARKER: &str = "WAKEUP_PLAIN_MARKER_T2901";
    let service = ScriptedAnthropicService::tool_calls(
        &[(
            "ScheduleWakeup",
            serde_json::json!({
                "delaySeconds": 2,
                "reason": "look again shortly",
                "prompt": format!("{MARKER} the plain frontend woke up")
            }),
        )],
        "plain wakeup scheduled",
    )
    .await
    .expect("start plain wakeup script");
    let layout = Layout::new();
    // A pty, not a pipe: a pipe's EOF ends the plain session before any
    // wakeup could fire — the same way a `claude -p` run does not linger.
    let mut run = pty_with_env(
        &layout,
        service.base_url(),
        &["--plain", "--permission-mode", "danger-full-access"],
        &[("ZO_AUTONOMY_MIN_MODEL_WAKEUP_SECS", "1")],
    );
    run.send(b"check back shortly\r").expect("send prompt");
    run.wait_for("plain wakeup scheduled", TEST_TIMEOUT);
    run.wait_for("wakeup: loop-1 armed", TEST_TIMEOUT);

    let bodies = wait_for_requests(&service, 3, Instant::now() + TEST_TIMEOUT).await;
    assert!(
        e2e::scripted::user_text_contains(&parse_body(&bodies[2]), MARKER),
        "the plain frontend's fired turn does not carry the stored prompt:\n{}",
        bodies[2]
    );
    run.wait_for("no further wakeup", TEST_TIMEOUT);
    let _ = run.finish();
}

/* ---- PushNotification (t-2943): the receipt names the road ---- */

/// The scripted turn every push scenario runs: one `PushNotification` call in
/// Claude Code's shape, then a final answer.
async fn push_notification_script(message: &str) -> ScriptedAnthropicService {
    ScriptedAnthropicService::tool_calls(
        &[(
            "PushNotification",
            serde_json::json!({"message": message, "status": "proactive"}),
        )],
        "push handled",
    )
    .await
    .expect("start push script")
}

/// The words the receipt says for the two roads a bare pty can take, pinned
/// where the design table pins them.
const PUSH_ROAD_ATTENDED: &str = "skipped: attended";
const PUSH_ROAD_TERMINAL: &str = "\"road\": \"terminal\"";

/// A person who just typed the prompt is at the keyboard: the tool skips,
/// the receipt says so, and nothing rings — Claude Code's own rule.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_push_notification_is_skipped_while_the_person_is_at_the_keyboard() {
    let service = push_notification_script("Build is green; merge when you are back").await;
    let layout = Layout::new();
    let args = interactive_args();
    let mut run = pty(&layout, service.base_url(), &args);
    run.wait_for("directory:", TEST_TIMEOUT);
    run.send(b"tell me when the build is done\r").expect("send prompt");
    run.wait_for("push handled", TEST_TIMEOUT);
    let capture = run.finish();

    let bodies = service.request_bodies().await;
    assert_eq!(bodies.len(), 2, "one tool call and one answer");
    let receipt = parse_body(&bodies[1]);
    assert!(
        e2e::scripted::body_text_contains(&receipt, PUSH_ROAD_ATTENDED),
        "a person who just typed must read as attended:\n{}",
        bodies[1]
    );
    assert!(
        find(&capture, b"\x1b]777;notify;").is_none(),
        "a skipped push must not ring the terminal"
    );
}

/// With the attended window closed through the table's own knob, the same
/// turn takes the terminal road: the receipt says `terminal`, and the pty
/// sees the bell and the OSC 777 notification carrying the message.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_push_notification_rings_the_bare_terminal_when_nobody_is_at_the_keyboard() {
    let service = push_notification_script("Build is green; merge when you are back").await;
    let layout = Layout::new();
    let args = interactive_args();
    let mut run = pty_with_env(
        &layout,
        service.base_url(),
        &args,
        &[("ZO_PUSH_ATTENDED_WINDOW_MS", "0")],
    );
    run.wait_for("directory:", TEST_TIMEOUT);
    run.send(b"tell me when the build is done\r").expect("send prompt");
    run.wait_for("push handled", TEST_TIMEOUT);
    let capture = run.finish();

    let bodies = service.request_bodies().await;
    assert_eq!(bodies.len(), 2, "one tool call and one answer");
    assert!(
        e2e::scripted::body_text_contains(&parse_body(&bodies[1]), PUSH_ROAD_TERMINAL),
        "the receipt must name the terminal road:\n{}",
        bodies[1]
    );
    let osc_777 = b"\x1b]777;notify;";
    let at = capture
        .windows(osc_777.len())
        .position(|window| window == osc_777)
        .expect("the pty saw the OSC 777 notification");
    let rest = &capture[at + osc_777.len()..];
    let end = rest.iter().position(|byte| *byte == 0x07).expect("BEL terminates the OSC");
    let payload = String::from_utf8_lossy(&rest[..end]).into_owned();
    assert!(
        payload.ends_with(";Build is green; merge when you are back"),
        "the notification carries the message: {payload:?}"
    );
    assert!(
        find(&capture, b"\x1b]9;Build is green; merge when you are back\x07").is_some(),
        "iTerm2's OSC 9 carries the same body"
    );
    // `split` yields one more piece than there are BELs.
    assert!(
        capture.split(|byte| *byte == 0x07).count() > 3,
        "one BEL to ring and one to close each OSC"
    );
}

/// t-2998: the model tries the incident's one-command delegation, receives a
/// refusal, and completes the housekeeping inline without creating a child.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_housekeeping_refuses_small_spawn_and_continues_inline() {
    let layout = Layout::new();
    let service = ScriptedAnthropicService::tool_calls(&[
        ("Agent", serde_json::json!({"description":"remote lookup", "subagent_type":"Explore", "prompt":"Check the git remote.", "background":false})),
        ("write_file", serde_json::json!({"path":"inline-result.txt", "content":"housekeeping finished inline"})),
        ("bash", serde_json::json!({"command":"python3 - <<'PY'\nfrom pathlib import Path\nassert Path('inline-result.txt').read_text().strip()\nPY"})),
    ], "housekeeping-finished-marker").await.unwrap();
    let mut run = pty(&layout, service.base_url(), &interactive_args());
    run.wait_for("directory:", TEST_TIMEOUT);
    run.send("커밋 푸시 후 정리해\r".as_bytes()).unwrap();
    run.wait_for_history_row("housekeeping-finished-marker", TEST_TIMEOUT);
    let capture = run.finish();

    assert_eq!(fs::read_to_string(layout.cwd.join("inline-result.txt")).unwrap(), "housekeeping finished inline");
    let requests = service.request_bodies().await;
    assert_eq!(requests.len(), 4, "a refused child must make no provider request");
    assert!(requests[1].contains("Small delegated task: do it inline"));
    let screen = strip_ansi(&String::from_utf8_lossy(&capture));
    assert!(screen.contains("Spawn declined") && screen.contains("Small delegated task"), "{screen}");
    assert!(!screen.contains("Agent spawn failed"), "a policy verdict is not drawn as a failure:\n{screen}");
}

/// t-2999: a model-authored reserved implementation model is refused; the
/// next scripted step writes inline and there is no child provider request.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_reserved_implementation_refusal_is_followed_by_inline_work() {
    let layout = Layout::new();
    layout.model_led();

    let service = ScriptedAnthropicService::tool_calls(&[
        ("Agent", serde_json::json!({"description":"implement copy", "subagent_type":"general-purpose", "prompt":"Write the new copy in index.html.", "model":"claude-fable-5-1", "allow_cross_provider":true, "background":false})),
        ("write_file", serde_json::json!({"path":"inline-result.txt", "content":"implemented inline after refusal"})),
        ("bash", serde_json::json!({"command":"python3 - <<'PY'\nfrom pathlib import Path\nassert Path('inline-result.txt').read_text().strip()\nPY"})),
    ], "implementation-finished-marker").await.unwrap();
    let mut run = pty(&layout, service.base_url(), &interactive_args());
    run.wait_for("directory:", TEST_TIMEOUT);
    // Authorizes delegation, but never names or authorizes a model exception.
    run.send(b"Use one specialist to implement the copy.\r").unwrap();
    run.wait_for_history_row("implementation-finished-marker", TEST_TIMEOUT);
    let capture = run.finish();

    assert_eq!(fs::read_to_string(layout.cwd.join("inline-result.txt")).unwrap(), "implemented inline after refusal");
    let requests = service.request_bodies().await;
    assert_eq!(requests.len(), 4, "no child or pinned retry request");
    assert!(requests[1].contains("reserved for orchestration") && requests[1].contains("Do this work inline"));
    let screen = strip_ansi(&String::from_utf8_lossy(&capture));
    assert!(screen.contains("Spawn declined") && screen.contains("reserved for orchestration"), "{screen}");
}

/// t-3000: a server dies while another tool is running. Its next request,
/// transcript, session state and a reopened `TaskOutput` all retain the fact.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_background_exit_reaches_next_boundary_and_persisted_task_output() {
    let layout = Layout::new();
    let service = ScriptedAnthropicService::tool_calls(&[
        ("bash", serde_json::json!({"command":"sleep 0.15; printf 'server-last-diagnostic\\n'; exit 7", "run_in_background":true})),
        ("bash", serde_json::json!({"command":"sleep 1; printf boundary-ready"})),
    ], "background-boundary-finished-marker").await.unwrap();
    let mut run = pty(&layout, service.base_url(), &interactive_args());
    run.wait_for("directory:", TEST_TIMEOUT);
    run.send(b"Run the server in background and check it.\r").unwrap();
    run.wait_for_history_row("background-boundary-finished-marker", TEST_TIMEOUT);
    let capture = run.finish();

    let requests = service.request_bodies().await;
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[2].matches("exited (code 7)").count(), 1, "one notice at next tool boundary");
    assert!(requests[2].contains("exited (code 7)") && requests[2].contains("server-last-diagnostic"));
    let transcript = transcripts(&layout.sessions).into_iter().next().unwrap();
    let session_id = session_id_of(&transcript);
    let path = layout.state.join("projects").join(runtime::project_slug(&fs::canonicalize(&layout.cwd).unwrap())).join("state")
        .join(runtime::task_registry::BACKGROUND_TASK_STORE_DIR).join(format!("{session_id}.json"));
    let registry = runtime::task_registry::TaskRegistry::with_persistence_path(Some(path.clone()));
    let tasks = registry.list(None);
    assert_eq!(tasks.len(), 1, "{}", path.display());
    let task = &tasks[0];
    assert_eq!(task.exit_code, Some(7));
    let context = tools::ToolContext::new().with_tasks(registry);
    context.set_session_id(&session_id);
    let output = tools::execute_tool(&context, "TaskOutput", &serde_json::json!({"task_id":task.task_id})).unwrap();
    assert!(output.contains("server-last-diagnostic") && output.contains("failed"));
    let persisted = fs::read_to_string(transcript).unwrap();
    assert_eq!(persisted.matches("exited (code 7)").count(), 1);
    let screen = strip_ansi(&String::from_utf8_lossy(&capture));
    assert!(screen.contains("exited (code 7)"), "{screen}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_headless_background_exit_reaches_the_model_at_a_tool_boundary() {
    let layout = Layout::new();
    let service = ScriptedAnthropicService::tool_calls(&[
        ("bash", serde_json::json!({"command":"sleep 0.15; printf headless-tail; exit 9", "run_in_background":true})),
        ("bash", serde_json::json!({"command":"sleep 1; printf boundary-ready"})),
    ], "headless-background-finished").await.unwrap();
    let output = run_pipe(&layout.cwd, &layout.home, &layout.sessions, &layout.state, service.base_url(),
        &["--plain", "--permission-mode", "danger-full-access"], b"Run a background command and check it.\n/exit\n").unwrap();
    assert!(output.status.success());
    let requests = service.request_bodies().await;
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[2].matches("exited (code 9)").count(), 1);
    assert!(requests[2].contains("headless-tail") && requests[2].contains("use TaskOutput"));
}

/// The plan scorer's shadow files one row per turn under the project's state
/// dir: the alternatives it priced beside the plan the turn ran. A plain
/// text turn is a solo plan on the current model, and every candidate the
/// shared price table names carries a dollar figure — the harness authorizes
/// Anthropic alone, so that is all of them.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_a_turn_files_one_plan_shadow_row_beside_what_it_ran() {
    let layout = Layout::new();
    let service = ScriptedAnthropicService::text(DIRECT_ANSWER)
        .await
        .expect("start direct-answer script");
    let args = interactive_args();
    let mut run = pty(&layout, service.base_url(), &args);

    run.wait_for("directory:", TEST_TIMEOUT);
    run.send(b"Give me the deterministic heading and two bullets\r")
        .expect("send direct prompt");
    run.wait_for_history_row("second item", TEST_TIMEOUT);
    let _capture = run.finish();

    let ledger = find_file_named(layout.root.path(), "plan-shadow.jsonl")
        .expect("one plan-shadow ledger under the private root");
    let text = std::fs::read_to_string(&ledger).expect("read the shadow ledger");
    let rows: Vec<serde_json::Value> = text
        .lines()
        .map(|line| serde_json::from_str(line).expect("a JSON row"))
        .collect();
    assert_eq!(rows.len(), 1, "one turn, one row: {text}");
    let row = &rows[0];
    assert_eq!(row["actual"]["shape"], "solo", "{row}");
    assert_eq!(row["currentModel"], row["actual"]["model"], "{row}");
    assert!(row["candidateCount"].as_u64().is_some_and(|n| n >= 1), "{row}");
    let candidates = row["candidates"].as_array().expect("candidates");
    assert!(!candidates.is_empty(), "{row}");
    // A candidate is priced exactly when the shared table names its model.
    // Not "every candidate is priced": a connected provider the table has no
    // row for (Gemini, xAI, a custom endpoint) must stay unpriced rather than
    // be folded in at zero.
    for candidate in candidates {
        let model = candidate["model"].as_str().expect("a candidate model");
        assert_eq!(
            candidate["costUsd"].is_null(),
            api::model_price(model).is_none(),
            "the row and the table disagree about whether {model} is priced — {row}"
        );
    }
    assert!(
        candidates
            .iter()
            .any(|candidate| candidate["costUsd"].as_f64().is_some_and(|usd| usd > 0.0)),
        "the Anthropic models this harness connects are all in the table — {row}"
    );
    assert!(row["chosen"].is_number(), "the scorer still picks one — {row}");
    // The row names the same attempt the turn's request rows billed to, so
    // the shadow joins the four ledgers on the one key.
    let attempt = row["attempt"].as_str().expect("the shadow row names its attempt");
    assert!(attempt.contains('@'), "a main-turn key: {attempt}");
    let requests = find_file_named(layout.root.path(), "requests.jsonl")
        .expect("the session's request ledger under the private root");
    let billed: Vec<String> = std::fs::read_to_string(&requests)
        .expect("read the request ledger")
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter_map(|req| req["attempt"].as_str().map(str::to_string))
        .collect();
    assert!(!billed.is_empty(), "the turn's request row carries an attempt");
    assert!(
        billed.iter().all(|key| key == attempt),
        "shadow {attempt} vs requests {billed:?}"
    );
}

fn find_file_named(root: &std::path::Path, name: &str) -> Option<PathBuf> {
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).ok()?.flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if path.file_name().is_some_and(|file| file == name) {
                return Some(path);
            }
        }
    }
    None
}

/// The whole `zo mcp` road, end to end in the real binary: `add` writes a
/// server into the config home, `list --json` reads it back, `--doctor` — the
/// diagnosis a person actually runs after adding one — names it, and `remove`
/// takes it out again.
///
/// The three verbs and the doctor are separate processes on purpose: the round
/// trip this proves is a file, not a data structure that never left memory.
#[test]
fn e2e_mcp_add_reaches_the_doctor_and_remove_takes_it_back_out() {
    const SERVER: &str = "e2e-mcp-server-t5552";
    let layout = Layout::new();
    let mcp = |args: &[&str]| {
        run_pipe(
            &layout.cwd,
            &layout.home,
            &layout.sessions,
            &layout.state,
            // No session opens for any of these verbs, so nothing is dialled.
            "http://127.0.0.1:1",
            args,
            b"",
        )
        .expect("run zo")
    };

    let added = mcp(&["mcp", "add", SERVER, "--", "npx", "-y", "server-everything"]);
    assert!(added.status.success(), "add exited with {:?}", added.status);

    let listed = mcp(&["mcp", "list", "--json"]);
    assert!(listed.status.success(), "list exited with {:?}", listed.status);
    let listed: serde_json::Value =
        serde_json::from_slice(&listed.stdout).expect("list --json is JSON");
    let servers = listed["servers"].as_array().expect("servers");
    assert_eq!(servers.len(), 1, "{listed}");
    assert_eq!(servers[0]["name"], SERVER);
    assert_eq!(servers[0]["transport"], "stdio");
    assert_eq!(servers[0]["endpoint"], "npx -y server-everything");

    let doctor = mcp(&["--doctor"]);
    let report = strip_ansi(&String::from_utf8_lossy(&doctor.stdout));
    assert!(
        report.contains(SERVER),
        "the doctor does not name the server that was just added:\n{report}"
    );

    let removed = mcp(&["mcp", "remove", SERVER]);
    assert!(removed.status.success(), "remove exited with {:?}", removed.status);
    let doctor = mcp(&["--doctor"]);
    let report = strip_ansi(&String::from_utf8_lossy(&doctor.stdout));
    assert!(
        !report.contains(SERVER),
        "the doctor still names a removed server:\n{report}"
    );

    // A name nobody configured is a non-zero exit, and one sentence on stderr.
    let missing = mcp(&["mcp", "remove", SERVER]);
    assert_eq!(missing.status.code(), Some(1), "{:?}", missing.status);
    let stderr = String::from_utf8_lossy(&missing.stderr);
    assert!(stderr.contains("no MCP server named"), "stderr was:\n{stderr}");
}

/// Files a test's workspace holds, written before zo starts so the walker
/// finds them on its first pass.
fn write_workspace_files(layout: &Layout, files: &[(&str, &str)]) {
    for (path, body) in files {
        let path = layout.cwd.join(path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("workspace directory");
        }
        fs::write(path, body).expect("workspace file");
    }
}

/// The user text of the one request the provider saw.
async fn only_request_body(service: &ScriptedAnthropicService) -> serde_json::Value {
    let bodies = service.request_bodies().await;
    assert_eq!(bodies.len(), 1, "one submitted line is one request");
    serde_json::from_str(&bodies[0]).expect("the request body is JSON")
}

/// The `@` popup — codex `mentions_v2`: `@comp` lists the files the fuzzy
/// search found under the composer (name, dim directory, `File` tag, the key
/// footer), Tab puts the selected path into the line over the token, and
/// the submitted prompt carries that path as text.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_at_popup_lists_files_and_tab_inserts_the_selected_path() {
    let layout = Layout::new();
    write_workspace_files(
        &layout,
        &[
            ("src/composer.rs", "fn composer() {}\n"),
            ("src/compose_tests.rs", "#[test]\nfn t() {}\n"),
            ("README.md", "# readme\n"),
        ],
    );
    let service = ScriptedAnthropicService::text("### Seen\n\n- the path arrived\n")
        .await
        .expect("start provider");
    let args = interactive_args();
    // A controlling pty, so the test can resize it: the painter repaints only
    // the rows that changed, which spreads a popup whose rows arrive after
    // its footer over several frames; a resize repaints the whole viewport
    // in one frame, and that frame is the golden.
    let mut run = PtyRun::spawn_controlling_sized(
        40,
        120,
        &layout.cwd,
        &layout.home,
        &layout.sessions,
        &layout.state,
        service.base_url(),
        &args,
        &[],
    )
    .expect("spawn zo on a controlling pty");

    run.wait_for("directory:", TEST_TIMEOUT);
    run.send(b"look at @comp").expect("type a mention");
    // Both files match `comp` at the same place and score alike; codex
    // `filter.rs::sort_rows` then orders by name, so `compose_tests.rs` is
    // first. The name column is as wide as the widest visible name (16), so
    // `composer.rs` is padded to it before the two-column gap.
    run.wait_for("> compose_tests.rs  src/", TEST_TIMEOUT);
    run.wait_for("switch search modes", TEST_TIMEOUT);
    // An unselected row changes style between its name and its path, so it
    // is never one contiguous run of bytes — read it off the screen model.
    run.wait_until(0, TEST_TIMEOUT, |bytes| {
        let mut screen = Screen::new(40);
        screen.feed(bytes);
        screen
            .visible()
            .iter()
            .any(|row| row.starts_with("  composer.rs       src/"))
    });
    run.send(b"\x1b[B").expect("move the selection down");
    let popup_seen = run.wait_for("> composer.rs       src/", TEST_TIMEOUT);
    run.resize(41, 120).expect("resize the pty by one row");
    let repainted = run.wait_for_after("Filesystem Only    Skills", popup_seen, TEST_TIMEOUT);
    run.send(b"\t").expect("accept the selected file");
    run.wait_for_after("look at src/composer.rs", repainted, TEST_TIMEOUT);
    run.send(b"\r").expect("submit");
    run.wait_for_history_row("the path arrived", TEST_TIMEOUT);
    let capture = run.finish();
    if let Ok(path) = std::env::var("ZO_E2E_DUMP") {
        fs::write(format!("{path}.bin"), &capture).expect("dump the @ popup capture");
    }

    // From the composer text of the repainted popup frame (its border, both
    // rows, the blank, the footer with its mode indicator) — the footer slot
    // holds no cwd. The caret and the border are styled runs of their own,
    // so the anchor is the typed text, which is one run.
    assert_block_golden(
        &capture,
        "at-popup",
        "@ popup",
        &layout,
        b"look at @comp",
        b"Filesystem Only    Skills",
    );
    let body = only_request_body(&service).await;
    assert!(
        user_text_contains(&body, "look at src/composer.rs"),
        "the completed path reaches the model as text: {body}"
    );
}

/// zo's own row: with a second brain configured, `@` also finds the vault's
/// `wiki/` pages (`Vault` tag), inserts one as `@wiki/<page>`, and the
/// submitted prompt carries the page's body under the mention.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_at_popup_offers_vault_pages_and_the_submitted_mention_carries_the_page_body() {
    let layout = Layout::new();
    write_workspace_files(&layout, &[("main.rs", "fn main() {}\n")]);
    let vault = layout.root.path().join("vault");
    fs::create_dir_all(vault.join("wiki")).expect("vault wiki");
    fs::write(
        vault.join("wiki/alpha-notes.md"),
        "# Alpha\n\nzo vault body line\n",
    )
    .expect("vault page");
    let service = ScriptedAnthropicService::text("### Seen\n\n- the page arrived\n")
        .await
        .expect("start provider");
    let args = interactive_args();
    let vault_text = vault.to_string_lossy().into_owned();
    let mut run = PtyRun::spawn_with_env(
        &layout.cwd,
        &layout.home,
        &layout.sessions,
        &layout.state,
        service.base_url(),
        &args,
        &[("ZEROCODE_SECOND_BRAIN", vault_text.as_str())],
    )
    .expect("spawn zo with a vault");

    run.wait_for("directory:", TEST_TIMEOUT);
    run.send(b"@alpha-n").expect("type a page mention");
    let row_seen = run.wait_for("> alpha-notes.md  wiki/", TEST_TIMEOUT);
    // The popup is drawn over the rows above the composer and given back
    // when it closes, so the tag is read while the popup stands.
    run.wait_until(0, TEST_TIMEOUT, |bytes| {
        let mut screen = Screen::new(40);
        screen.feed(bytes);
        screen
            .visible()
            .iter()
            .any(|row| row.starts_with("> alpha-notes.md  wiki/") && row.ends_with("Vault"))
    });
    run.send(b"\t").expect("accept the page");
    run.wait_for_after("@wiki/alpha-notes.md", row_seen, TEST_TIMEOUT);
    run.send(b"\r").expect("submit");
    run.wait_for_history_row("the page arrived", TEST_TIMEOUT);
    let _ = run.finish();

    let body = only_request_body(&service).await;
    for needle in [
        "@wiki/alpha-notes.md",
        "[second brain page: wiki/alpha-notes.md]",
        "zo vault body line",
    ] {
        assert!(user_text_contains(&body, needle), "{needle:?} missing from {body}");
    }
}

/// zo's own order: a file a tool read in this conversation stands first in
/// the `@` popup, above a file the matcher alone would have put first.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_a_file_the_turn_read_stands_first_in_the_at_popup() {
    let layout = Layout::new();
    layout.fixture("fixture content for the deterministic read\n");
    write_workspace_files(&layout, &[("fix.txt", "the closer match nobody touched\n")]);
    let service = ScriptedAnthropicService::bash_read("### Tool answer\n\n- fixture read\n")
        .await
        .expect("start bash/read script");
    let args = interactive_args();
    let mut run = pty(&layout, service.base_url(), &args);

    run.wait_for("directory:", TEST_TIMEOUT);
    run.send(b"Run Bash once and Read fixture.txt once, then summarize\r")
        .expect("send tool prompt");
    run.wait_for_history_row("fixture read", TEST_TIMEOUT);
    run.send(b"@fix.t").expect("type a mention both files match");
    run.wait_until(0, TEST_TIMEOUT, |bytes| {
        let mut screen = Screen::new(40);
        screen.feed(bytes);
        let visible = screen.visible();
        visible.iter().any(|row| row.starts_with("> fixture.txt  ./"))
            && visible.iter().any(|row| row.starts_with("  fix.txt") && row.contains("  ./"))
    });
    // Esc closes the popup and Ctrl-U clears the line — so `/exit` below
    // lands on an empty composer and nothing else is submitted. (Two Esc
    // bytes back to back read as one escape sequence, not two keys.) The
    // placeholder was on screen at boot too, so the wait starts here.
    let before_esc = run.output_len();
    run.send(b"\x1b").expect("close the popup");
    run.send(b"\x15").expect("clear the composer");
    run.wait_for_after("Ask zo to do anything", before_esc, TEST_TIMEOUT);
    let _ = run.finish();
    assert_eq!(service.request_bodies().await.len(), 2, "the tool turn only");
}
