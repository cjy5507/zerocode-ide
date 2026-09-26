//! The value seat's writer over a real socket: what it sends, with which key,
//! and what it refuses. No keychain is asked — every key store here is the
//! test's own map — and every case has an endpoint on a port of its own.

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use zerocode_core::type_value::{
    ANTHROPIC_WIRE, FieldLook, GeneratorRoad, Road, ValueRefusal, asked, chosen, chosen_on, render,
};

use super::*;
use crate::api_routers::{HeldKeys, RouterKeys, RouterRefusal};
use crate::systemone::tests::Endpoint;
use crate::systemone::{SCHEMA, TIMEOUT, UNAUTHORIZED};

/// A key a person put in the window's key store for the chosen row.
pub(in crate::computer_use::errand) const KEY: &str = "a-key-a-person-set";

/// What a machine with a subscription login keeps, where the vendor's own
/// client keeps it — the login this seat never reads, never sends.
pub(in crate::computer_use::errand) const SUBSCRIPTION_ITEM: &str = "Claude Code-credentials";
pub(in crate::computer_use::errand) const SUBSCRIPTION: &str =
    r#"{"claudeAiOauth":{"accessToken":"a-subscription-login","refreshToken":"r"}}"#;

/// A key store holding a subscription login, and — `with_key` — the key a
/// person set for the chosen row where the row says it lives.
pub(in crate::computer_use::errand) fn store(with_key: bool) -> Box<dyn RouterKeys> {
    let keys = HeldKeys::default();
    keys.write(SUBSCRIPTION_ITEM, SUBSCRIPTION)
        .expect("a subscription login held");
    if with_key {
        let service = key_service(chosen().expect("a chosen row")).expect("a row names its key");
        keys.write(&service, KEY).expect("the person's key held");
    }
    Box::new(keys)
}

/// One key store that the Computer Use pane writes and every walk's writer
/// reads, as the window's keychain is one (t-9537): a clone is the same store.
#[derive(Clone)]
pub(in crate::computer_use::errand) struct OneStore(Rc<dyn RouterKeys>);

impl OneStore {
    pub(in crate::computer_use::errand) fn over(keys: Box<dyn RouterKeys>) -> Self {
        Self(Rc::from(keys))
    }
}

impl RouterKeys for OneStore {
    fn read(&self, service: &str) -> Result<Option<String>, RouterRefusal> {
        self.0.read(service)
    }
    fn write(&self, service: &str, secret: &str) -> Result<(), RouterRefusal> {
        self.0.write(service, secret)
    }
    fn delete(&self, service: &str) -> Result<(), RouterRefusal> {
        self.0.delete(service)
    }
}

/// The name of the key the chosen row reads, as the pane names it.
pub(in crate::computer_use::errand) fn chosen_key() -> &'static str {
    chosen()
        .and_then(|row| row.credential_key.as_deref())
        .expect("the chosen row names its key")
}

/// The words around one box, as a walk's look reads them.
fn look() -> FieldLook<'static> {
    FieldLook {
        goal: "Find a flight from Zurich to London",
        label: "To",
        placeholder: "City or airport",
        near: "Arrival airport",
    }
}

/// What the Messages endpoint answers when it wrote `text`.
pub(in crate::computer_use::errand) fn wrote(text: &str) -> String {
    json!({
        "id": "msg_test",
        "type": "message",
        "role": "assistant",
        "model": chosen().expect("a chosen row").model,
        "content": [{ "type": "text", "text": text }],
        "stop_reason": "end_turn",
        "usage": { "input_tokens": 40, "output_tokens": 3 },
    })
    .to_string()
}

fn writer_at(endpoint: &Endpoint, with_key: bool) -> LiveWriter {
    LiveWriter::at(&format!("{}/v1/messages", endpoint.base()), store(with_key))
}

/// A person's key is the only key the seat asks with: it rides the row's own
/// key header and nothing else does — no `Authorization`, no subscription
/// beta, no line speaking for another client — and the body is the table's
/// own question: its instructions as the system, the rendered field as the
/// one user line.
#[test]
fn the_value_seat_asks_with_the_key_a_person_set_and_speaks_for_no_client() {
    let endpoint = Endpoint::serving("HTTP/1.1 200 OK", wrote("London"), 0);
    let mut writer = writer_at(&endpoint, true);
    assert!(writer.ready(), "a person set a key");
    let written = writer
        .write(&look(), Duration::from_secs(5))
        .expect("a value");
    assert_eq!(written.value, "London");
    let row = chosen().expect("a chosen row");
    assert_eq!(written.model, row.model);

    let heard = endpoint.asked();
    assert_eq!(heard.len(), 1, "one request for one value");
    let (head, body) = heard[0].split_once("\r\n\r\n").expect("a head and a body");
    let head = head.to_ascii_lowercase();
    assert!(
        head.contains(&format!("{}: {KEY}", ANTHROPIC_WIRE.key_header)),
        "the person's key rides its header:\n{head}"
    );
    assert!(head.contains(&format!("anthropic-version: {}", ANTHROPIC_WIRE.version)));
    for absent in ["authorization:", "anthropic-beta:"] {
        assert!(!head.contains(absent), "`{absent}` was sent:\n{head}");
    }
    assert!(!body.contains(KEY), "the key rode the body");
    let sent: Value = serde_json::from_str(body).expect("a json body");
    assert_eq!(sent["model"], json!(row.model));
    assert_eq!(
        sent["system"],
        json!(asked().instructions),
        "the table's words, nothing else"
    );
    assert_eq!(sent["messages"][0]["role"], json!("user"));
    assert_eq!(sent["messages"][0]["content"], json!(render(&look())));
    assert!(
        sent["max_tokens"].as_u64().is_some_and(|most| most > 0),
        "a bound on the value's length"
    );
    let printed = format!("{written:?}");
    assert!(
        !printed.contains(KEY) && !printed.contains("London"),
        "{printed}"
    );
}

/// A machine that holds only a subscription login has no value seat: the
/// writer is not ready, it asks nothing, and the login is on no request.
/// (The walk's side of it — no entry offered — is the goal world's test.)
#[test]
fn a_subscription_login_alone_sets_up_no_writer() {
    let endpoint = Endpoint::serving("HTTP/1.1 200 OK", wrote("London"), 0);
    let mut writer = writer_at(&endpoint, false);
    assert!(
        !writer.ready(),
        "a subscription login is not a key a person set"
    );
    assert_eq!(
        writer
            .write(&look(), Duration::from_secs(5))
            .map(|written| written.ms)
            .expect_err("no key, no value"),
        NO_KEY
    );
    assert!(endpoint.asked().is_empty(), "a request left without a key");
}

/// The key a person saves in the Computer Use pane (t-9537) is the key the
/// next walk's writer asks with — the same item, named by the same function —
/// and no restart stands between them: the writer a walk made before the save
/// has no key, the one the next walk makes after it types with the key, and
/// after the pane removes it the one after that has none again.
#[test]
fn a_key_saved_in_the_pane_is_the_next_walks_key() {
    let keys = OneStore::over(store(false));
    let endpoint = Endpoint::serving("HTTP/1.1 200 OK", wrote("London"), 0);
    let at = format!("{}/v1/messages", endpoint.base());

    let mut before = LiveWriter::at(&at, Box::new(keys.clone()));
    assert!(!before.ready(), "a walk before the save has a key");
    assert_eq!(
        before
            .write(&look(), Duration::from_secs(5))
            .map(|written| written.ms)
            .expect_err("no key, no value"),
        NO_KEY
    );

    crate::type_value_keys::save(chosen_key(), KEY, &keys).expect("the pane keeps the key");
    let mut after = LiveWriter::at(&at, Box::new(keys.clone()));
    assert!(after.ready(), "the next walk's writer reads the saved key");
    let written = after
        .write(&look(), Duration::from_secs(5))
        .expect("a value");
    assert_eq!(written.value, "London");
    let heard = endpoint.asked();
    assert_eq!(heard.len(), 1, "one request for one value: {heard:?}");
    assert!(
        heard[0]
            .to_ascii_lowercase()
            .contains(&format!("{}: {KEY}", ANTHROPIC_WIRE.key_header)),
        "the pane's key rides the request:\n{}",
        heard[0]
    );

    crate::type_value_keys::remove(chosen_key(), &keys).expect("the pane forgets the key");
    assert!(
        !LiveWriter::at(&at, Box::new(keys)).ready(),
        "a walk after the removal has a key"
    );
}

/// An answer that is no value says why in a word of its own — the wire's
/// for a refusal or a body nothing reads, the seat's rule for a value it
/// would not type — and a writer that has not answered by the wall the walk
/// gave it is no value either; a walk with no time left asks nothing.
#[test]
fn a_refused_or_misshapen_answer_is_no_value_and_says_why() {
    for (status, body, token) in [
        (
            "HTTP/1.1 401 Unauthorized",
            "{}".to_string(),
            UNAUTHORIZED.to_string(),
        ),
        (
            "HTTP/1.1 200 OK",
            "not json".to_string(),
            SCHEMA.to_string(),
        ),
        (
            "HTTP/1.1 200 OK",
            json!({ "content": [] }).to_string(),
            SCHEMA.to_string(),
        ),
        (
            "HTTP/1.1 200 OK",
            wrote("London\nThat is the arrival city."),
            ValueRefusal::NotOneLine.token().to_string(),
        ),
        (
            "HTTP/1.1 200 OK",
            wrote("   "),
            ValueRefusal::Empty.token().to_string(),
        ),
    ] {
        let endpoint = Endpoint::serving(status, body.clone(), 0);
        let refused = writer_at(&endpoint, true)
            .write(&look(), Duration::from_secs(5))
            .map(|written| written.ms)
            .expect_err("no value");
        assert_eq!(refused, token, "{status} {body}");
    }

    let slow = Endpoint::serving("HTTP/1.1 200 OK", wrote("London"), 800);
    let began = Instant::now();
    let refused = writer_at(&slow, true)
        .write(&look(), Duration::from_millis(200))
        .map(|written| written.ms)
        .expect_err("past its wall");
    assert_eq!(refused, TIMEOUT);
    assert!(
        began.elapsed() < Duration::from_millis(700),
        "{:?}",
        began.elapsed()
    );

    let idle = Endpoint::serving("HTTP/1.1 200 OK", wrote("London"), 0);
    let refused = writer_at(&idle, true)
        .write(&look(), Duration::ZERO)
        .map(|written| written.ms)
        .expect_err("no time left");
    assert_eq!(refused, TIMEOUT);
    assert!(
        idle.asked().is_empty(),
        "a walk with no time left asked anyway"
    );
}

/// A row this product does not take is no writer however its key is set: a
/// road that would have the request speak as another client, and one not
/// built here.
#[test]
fn a_road_that_speaks_for_another_client_is_never_taken() {
    for row in zerocode_core::type_value::rows() {
        let taken = endpoint_of(row).is_some();
        if row.client_fingerprint.is_some() {
            assert!(!taken, "{} speaks as another client", row.id);
        }
        if row.road == Road::CodeAssist {
            assert!(!taken, "{} is not built here", row.id);
        }
    }
    let chosen = chosen().expect("a chosen row");
    assert!(
        endpoint_of(chosen).is_some(),
        "the chosen row's road is taken"
    );
    assert!(
        key_service(chosen).is_some(),
        "the chosen row names its key"
    );
}

/// The value seat's own sources — and the generator's that shares its ask,
/// its table and the two probes that measured such roads — never speak for a
/// client they are not and never read or send a login (t-10372, the
/// coordinator's scope m-10383): none of the words such a request or such a
/// read is made of is in them. A login is spent by the vendor's own CLI.
#[test]
fn the_value_seats_sources_speak_for_no_other_client() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    for file in [
        "crates/zerocode-core/src/type_value.rs",
        "crates/zerocode-core/fixtures/type-value/models.json",
        "crates/zerocode-core/fixtures/type-value/question.json",
        "crates/zerocode-shell/src/computer_use/errand.rs",
        "crates/zerocode-shell/src/computer_use/errand/desk.rs",
        "crates/zerocode-shell/src/computer_use/errand/value.rs",
        "crates/zerocode-shell/src/computer_use/reflex/plan.rs",
        "crates/zerocode-shell/src/type_value_keys.rs",
        "tools/type_value_latency.py",
        "tools/speed-probe/image_loop.py",
        "tools/speed-probe/web_probe.py",
    ] {
        let source = std::fs::read_to_string(root.join(file)).expect("the seat's source");
        for word in [
            "You are Claude Code",
            "official CLI",
            "claudeAiOauth",
            "accessToken",
            "access_token",
            "oauth-2025",
            "anthropic-beta",
            "claude-cli/",
            "usage_login",
            "claude_access_token",
            "credentials.json",
            ".credentials",
        ] {
            assert!(!source.contains(word), "{file} spells `{word}`");
        }
    }
}

/// A vendor's CLI as a script: it writes its arguments, its environment and
/// its stdin down beside itself, prints what `answer` says and exits `rc` —
/// after `pause` seconds, spent in a child of its own whose pid it writes
/// down, when `pause` is more than none.
pub(in crate::computer_use::errand) fn fake_cli(
    dir: &Path,
    name: &str,
    answer: &str,
    rc: i32,
    pause: u32,
) -> String {
    let at = dir.join(name);
    std::fs::write(at.with_extension("answer"), answer).expect("the answer");
    let script = format!(
        "#!/bin/sh\n\
         here=\"{here}\"\n\
         printf '%s\\n' \"$@\" > \"$here.argv\"\n\
         env > \"$here.env\"\n\
         cat > \"$here.stdin\"\n\
         if [ {pause} -gt 0 ]; then sleep {pause} & echo $! > \"$here.child\"; wait; fi\n\
         cat \"$here.answer\"\n\
         exit {rc}\n",
        here = at.display(),
    );
    std::fs::write(&at, script).expect("the script");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&at, std::fs::Permissions::from_mode(0o755)).expect("runnable");
    }
    at.to_string_lossy().into_owned()
}

/// What one fake CLI wrote down about the run it was: `argv`, `env`, `stdin`.
pub(in crate::computer_use::errand) fn heard(dir: &Path, name: &str, what: &str) -> String {
    std::fs::read_to_string(dir.join(format!("{name}.{what}"))).unwrap_or_default()
}

/// Claude Code's one JSON result, as `-p --output-format json` prints it.
pub(in crate::computer_use::errand) fn claude_said(result: &str, is_error: bool) -> String {
    json!({
        "type": "result",
        "subtype": "success",
        "is_error": is_error,
        "result": result,
        "usage": {
            "input_tokens": 40,
            "cache_creation_input_tokens": 5,
            "cache_read_input_tokens": 7,
            "output_tokens": 3,
        },
    })
    .to_string()
}

/// Codex's JSON events for a turn that wrote `text`.
pub(in crate::computer_use::errand) fn codex_wrote(text: &str) -> String {
    [
        json!({ "type": "thread.started", "thread_id": "t" }),
        json!({ "type": "turn.started" }),
        json!({ "type": "item.completed", "item": { "id": "item_0", "type": "agent_message", "text": text } }),
        json!({ "type": "turn.completed", "usage": { "input_tokens": 900, "cached_input_tokens": 800, "output_tokens": 4 } }),
    ]
    .iter()
    .map(Value::to_string)
    .collect::<Vec<_>>()
    .join("\n")
}

/// Codex's JSON events for a turn that failed saying `message`.
pub(in crate::computer_use::errand) fn codex_failed(message: &str) -> String {
    [
        json!({ "type": "thread.started", "thread_id": "t" }),
        json!({ "type": "turn.started" }),
        json!({ "type": "error", "message": format!("Reconnecting... 1/5 ({message})") }),
        json!({ "type": "turn.failed", "error": { "message": message } }),
    ]
    .iter()
    .map(Value::to_string)
    .collect::<Vec<_>>()
    .join("\n")
}

/// A writer on `road` whose login roads run the scripts in `dir`, under an
/// account store of the test's own.
pub(in crate::computer_use::errand) fn login_writer(
    dir: &Path,
    road: GeneratorRoad,
    claude: &str,
    codex: &str,
) -> LiveWriter {
    let mut setup = Setup::new(road, dir.join("config"), dir.join("data"));
    setup.programs = vec![
        (Road::ClaudeCli, claude.to_string()),
        (Road::CodexCli, codex.to_string()),
    ];
    LiveWriter::over(setup)
}

/// The setup a measurement runs on: the window's own roots and the vendors'
/// own CLIs, as a person hands them to this command's environment
/// (`ZEROCODE_PROBE_ROOT`, the window's config and data root;
/// `ZEROCODE_PROBE_CLAUDE` and `ZEROCODE_PROBE_CODEX`, the CLIs — named, so a
/// pane's orchestration shims are not what runs).
pub(in crate::computer_use) fn probe_setup(road: GeneratorRoad) -> Setup {
    let root = PathBuf::from(std::env::var("ZEROCODE_PROBE_ROOT").expect("the window's root"));
    let mut setup = Setup::new(road, root.clone(), root);
    for (cli, var) in [
        (Road::ClaudeCli, "ZEROCODE_PROBE_CLAUDE"),
        (Road::CodexCli, "ZEROCODE_PROBE_CODEX"),
    ] {
        if let Ok(program) = std::env::var(var) {
            setup.programs.push((cli, program));
        }
    }
    setup
}

/// Each login road timed on its real CLI, as the window asks it (t-10372
/// §2.6): five values each, the table's own cases in turn, under the account
/// the window's panes run as. Printed, one line a call and the percentiles
/// after; a failure is printed as it came.
#[test]
#[ignore = "spends the person's own logins on the value seat's login roads; a measurement"]
fn the_login_roads_timed_on_their_real_clis() {
    let cases = &asked().cases;
    for road in [GeneratorRoad::ClaudeLogin, GeneratorRoad::CodexLogin] {
        let writer = LiveWriter::window(probe_setup(road));
        let mut took = Vec::new();
        for (at, case) in cases.iter().cycle().take(5).enumerate() {
            let look = FieldLook {
                goal: &case.goal,
                label: &case.field.label,
                placeholder: &case.field.placeholder,
                near: &case.field.near,
            };
            let question = asked();
            let began = Instant::now();
            let said = writer.ask_text(
                &question.instructions,
                &render(&look),
                question.value_char_cap,
                writer.wall(),
            );
            let ms = began.elapsed().as_millis();
            let value = said
                .as_ref()
                .ok()
                .map(|said| zerocode_core::type_value::read(&said.text));
            let accepted = value
                .as_ref()
                .and_then(|value| value.as_ref().ok())
                .is_some_and(|value| {
                    case.accepts
                        .iter()
                        .any(|word| value.to_lowercase().contains(word))
                });
            println!(
                "{}",
                json!({
                    "road": road.word(),
                    "call": at + 1,
                    "case": case.id,
                    "ms": ms,
                    "accepted": accepted,
                    "value": value.map(|value| value.map_err(|refusal| refusal.token())),
                    "tokens": said.as_ref().ok().and_then(|said| said.tokens).map(|tokens| json!({ "input": tokens.input, "output": tokens.output })),
                    "answered": said.as_ref().map(|said| said.answered.note()).map_err(Clone::clone),
                })
            );
            took.push(ms);
        }
        took.sort_unstable();
        let rank = |share: f64| {
            let at = ((share * took.len() as f64).ceil() as usize).clamp(1, took.len());
            took[at - 1]
        };
        println!(
            "{}",
            json!({ "road": road.word(), "n": took.len(), "p50Ms": rank(0.5), "p90Ms": rank(0.9) })
        );
    }
}

/// The Claude login is spent the one way this product spends one (t-10372):
/// Claude Code's own CLI, once, headless, with the row's argv — the table's
/// instructions as the system and nothing of the field on argv — the field's
/// question on stdin, under the account environment the window's panes read
/// (`CLAUDE_CONFIG_DIR` of the window's own home), and with none of the
/// coordinates that would tie it to a pane. The answer is the result's text,
/// its tokens what the request carried and wrote, and it says which road
/// wrote it.
#[test]
fn the_claude_login_is_asked_through_its_own_cli_with_the_question_on_stdin() {
    let dir = tempfile::tempdir().expect("a root");
    let claude = fake_cli(dir.path(), "claude", &claude_said("London", false), 0, 0);
    let mut writer = login_writer(dir.path(), GeneratorRoad::ClaudeLogin, &claude, "/nowhere");
    assert!(
        writer.ready(),
        "a CLI on the machine is a road that can be asked"
    );
    let written = writer
        .write(&look(), Duration::from_secs(10))
        .expect("a value");
    assert_eq!(written.value, "London");
    let row = chosen_on(GeneratorRoad::ClaudeLogin).expect("the Claude login row");
    assert_eq!(written.model, row.model);
    assert_eq!(
        written.answered,
        Answered {
            road: "claude_login",
            model: row.model.clone(),
            passed: Vec::new(),
        }
    );

    let argv: Vec<String> = heard(dir.path(), "claude", "argv")
        .lines()
        .map(str::to_string)
        .collect();
    assert_eq!(
        argv,
        zerocode_core::type_value::claude_cli_argv(row, &asked().instructions),
        "the row's own argv, nothing else"
    );
    let stdin = heard(dir.path(), "claude", "stdin");
    assert_eq!(stdin, render(&look()), "the question rides stdin");
    assert!(
        !argv
            .iter()
            .any(|word| word.contains(look().label) || word.contains(look().goal)),
        "the field's words reached argv: {argv:?}"
    );
    let env = heard(dir.path(), "claude", "env");
    for (name, value) in zerocode_core::type_value::claude_cli_env(row) {
        assert!(
            env.lines().any(|line| line == format!("{name}={value}")),
            "the row's {name} did not ride along"
        );
    }
    assert!(
        env.lines()
            .any(|line| line.starts_with("PWD=") && line.ends_with("zerocode-one-shot")),
        "the CLI did not start in a directory of its own:\n{env}"
    );
    let home = dir.path().join("config");
    assert!(
        env.lines()
            .any(|line| line.starts_with("CLAUDE_CONFIG_DIR=")
                && line.contains(home.to_string_lossy().as_ref())),
        "the window's own account home is the one read:\n{env}"
    );
    for name in crate::hooks::PANE_COORDINATES {
        assert!(
            !env.lines()
                .any(|line| line.starts_with(&format!("{name}="))),
            "{name} reached a run that is no pane's"
        );
    }
    let said = writer
        .ask_text("S", "U", 10, Duration::from_secs(10))
        .expect("an answer");
    assert_eq!(
        said.tokens,
        Some(Tokens {
            input: 52,
            output: 3
        }),
        "what the request carried, cached or not, and what it wrote"
    );
}

/// `auto` asks Claude's login first and, when it cannot answer, Codex's — and
/// says so (t-10372): the value is Codex's, the row names the road that wrote
/// it and the one passed over with its reason, and Codex was asked its own
/// way — `exec` on its row, the instructions and the question on stdin.
#[test]
fn auto_passes_a_walled_claude_to_codex_and_says_so() {
    let dir = tempfile::tempdir().expect("a root");
    let claude = fake_cli(
        dir.path(),
        "claude",
        &claude_said(
            "You've hit your session limit · resets 4:10am (Asia/Seoul)",
            true,
        ),
        1,
        0,
    );
    let codex = fake_cli(dir.path(), "codex", &codex_wrote("London"), 0, 0);
    let mut writer = login_writer(dir.path(), GeneratorRoad::Auto, &claude, &codex);
    let written = writer
        .write(&look(), Duration::from_secs(10))
        .expect("Codex wrote it");
    assert_eq!(written.value, "London");
    let row = chosen_on(GeneratorRoad::CodexLogin).expect("the Codex login row");
    assert_eq!(
        written.answered,
        Answered {
            road: "codex_login",
            model: row.model.clone(),
            passed: vec![format!("claude_login={QUOTA_WALL}")],
        }
    );
    assert_eq!(
        written.answered.note(),
        json!({ "road": "codex_login", "passedOver": [format!("claude_login={QUOTA_WALL}")] })
    );
    let argv: Vec<String> = heard(dir.path(), "codex", "argv")
        .lines()
        .map(str::to_string)
        .collect();
    assert_eq!(
        argv,
        zerocode_core::type_value::codex_cli_argv(
            row,
            &zerocode_core::launch::compat_launch_args("codex")
        )
    );
    let stdin = heard(dir.path(), "codex", "stdin");
    assert!(stdin.starts_with(&asked().instructions) && stdin.ends_with(&render(&look())));
    assert_eq!(heard(dir.path(), "claude", "stdin"), render(&look()));
}

/// A road set aside — its answers came, but could not be used — is not asked
/// again by this writer, and every later answer carries it and its reason
/// (t-10372); with no road left after it, nothing is set aside.
#[test]
fn a_road_set_aside_is_not_asked_again_and_every_answer_says_so() {
    let dir = tempfile::tempdir().expect("a root");
    let claude = fake_cli(dir.path(), "claude", &claude_said("{}", false), 0, 0);
    let codex = fake_cli(dir.path(), "codex", &codex_wrote("{}"), 0, 0);
    let writer = login_writer(dir.path(), GeneratorRoad::Auto, &claude, &codex);
    assert!(!writer.set_aside("plan_refused"), "nothing answered yet");
    let first = writer
        .ask_text("S", "U", 10, Duration::from_secs(10))
        .expect("Claude answers");
    assert_eq!(first.answered.road, "claude_login");
    assert!(writer.set_aside("plan_refused"), "Codex is left to ask");
    std::fs::remove_file(dir.path().join("claude.stdin")).expect("Claude ran");
    let second = writer
        .ask_text("S", "U", 10, Duration::from_secs(10))
        .expect("Codex answers");
    assert_eq!(second.answered.road, "codex_login");
    assert_eq!(
        second.answered.passed,
        vec!["claude_login=plan_refused".to_string()]
    );
    assert!(
        heard(dir.path(), "claude", "stdin").is_empty(),
        "a road set aside was asked again"
    );
    assert!(
        !writer.set_aside("plan_refused"),
        "no road is left after Codex"
    );
}

/// With neither login able to answer, nothing is typed and the refusal names
/// each road's own reason (t-10372) — the words each CLI was measured to say
/// when it holds no login — so the walk hands the field back saying why.
#[test]
fn with_no_login_on_either_road_the_refusal_names_each_roads_reason() {
    let dir = tempfile::tempdir().expect("a root");
    let claude = fake_cli(
        dir.path(),
        "claude",
        &claude_said("Not logged in · Please run /login", true),
        1,
        0,
    );
    let codex = fake_cli(
        dir.path(),
        "codex",
        &codex_failed(
            "unexpected status 401 Unauthorized: Missing bearer or basic authentication in header",
        ),
        1,
        0,
    );
    let mut writer = login_writer(dir.path(), GeneratorRoad::Auto, &claude, &codex);
    assert_eq!(
        writer
            .write(&look(), Duration::from_secs(10))
            .map(|written| written.ms)
            .expect_err("nobody can write"),
        format!("claude_login={NO_LOGIN},codex_login={NO_LOGIN}")
    );
    // One road chosen says its one reason, as a key's road always has.
    let mut claude_only = login_writer(dir.path(), GeneratorRoad::ClaudeLogin, &claude, &codex);
    assert_eq!(
        claude_only
            .write(&look(), Duration::from_secs(10))
            .map(|written| written.ms)
            .expect_err("no login"),
        NO_LOGIN
    );
    // And `off` asks nobody at all.
    let mut off = login_writer(dir.path(), GeneratorRoad::Off, &claude, &codex);
    assert_eq!(off.unready().as_deref(), Some(GENERATOR_OFF));
    assert!(!off.ready());
    std::fs::remove_file(dir.path().join("claude.stdin")).expect("the first runs");
    assert_eq!(
        off.write(&look(), Duration::from_secs(10))
            .map(|written| written.ms)
            .expect_err("off"),
        GENERATOR_OFF
    );
    assert!(
        heard(dir.path(), "claude", "stdin").is_empty(),
        "off ran a CLI"
    );
}

/// A CLI that outlives its wall is ended with every process it started, and
/// the road says it ran out of time.
#[cfg(unix)]
#[test]
fn a_cli_that_outlives_its_wall_is_ended_with_everything_it_started() {
    let dir = tempfile::tempdir().expect("a root");
    let claude = fake_cli(dir.path(), "claude", &claude_said("late", false), 0, 30);
    let writer = login_writer(dir.path(), GeneratorRoad::ClaudeLogin, &claude, "/nowhere");
    let began = Instant::now();
    assert_eq!(
        writer
            .ask_text("S", "U", 10, Duration::from_secs(3))
            .map(|said| said.text)
            .expect_err("too late"),
        TIMEOUT
    );
    assert!(began.elapsed() < Duration::from_secs(8), "the wall held");
    let child: i32 = heard(dir.path(), "claude", "child")
        .trim()
        .parse()
        .expect("the run started a child of its own");
    let gone = (0..100).any(|_| {
        // SAFETY: signal 0 only asks whether the pid is there.
        let there = unsafe { libc::kill(child, 0) } == 0;
        if there {
            std::thread::sleep(Duration::from_millis(20));
        }
        !there
    });
    assert!(gone, "the run's child outlived it");
}

/// The window's memory keeps a value per identity, as many as the longest
/// walk the verb allows can write, the oldest out first; keeping an identity
/// again replaces its value without growing.
#[test]
fn the_memory_keeps_the_longest_walks_values_oldest_out_first() {
    let mut values = Values::new(2);
    values.keep("a".to_string(), "1".to_string());
    values.keep("b".to_string(), "2".to_string());
    values.keep("c".to_string(), "3".to_string());
    assert_eq!(values.recall("a"), None, "the oldest went first");
    assert_eq!(values.recall("b").as_deref(), Some("2"));
    assert_eq!(values.recall("c").as_deref(), Some("3"));
    values.keep("c".to_string(), "4".to_string());
    assert_eq!(values.recall("c").as_deref(), Some("4"));
    assert_eq!(
        values.recall("b").as_deref(),
        Some("2"),
        "a replaced value grows nothing"
    );
    assert_eq!(
        held(&window_values()).cap(),
        zerocode_core::computer_use::WALK_STEPS_MAX
    );
}

/// The value seat timed on its real road, one process, three writes: the
/// first pays for the client and the connection, the rest ride its socket.
/// A measurement, printed; the key is the one a person hands this command's
/// environment, never read from a keychain and never printed.
#[test]
#[ignore = "spends a person's own API key on the value seat's real road; a measurement"]
fn the_value_seat_timed_on_its_real_road() {
    let key = std::env::var("ZEROCODE_VALUE_PROBE_KEY").expect("a key a person set");
    let keys = HeldKeys::default();
    let service = key_service(chosen().expect("a chosen row")).expect("its key's name");
    keys.write(&service, &key).expect("held");
    let mut writer = LiveWriter::at(ANTHROPIC_WIRE.url, Box::new(keys));
    for pass in 1..=3 {
        let began = Instant::now();
        let written = writer.write(&look(), Duration::from_secs(10));
        println!(
            "pass {pass}: {:?} in {} ms",
            written.as_ref().map(|written| written.value.as_str()),
            began.elapsed().as_millis()
        );
    }
}
