//! The stand-in against a keyless server written in `sh`: what it is asked
//! and when it starts are read off the server's own log.

use std::ffi::OsString;
use std::io::{BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use serde_json::{Value, json};

use super::{LISTING_STANDS_FOR, Listing, Listings, StandIn, listing_key};

/// A server that logs each method it hears, lists the tools its tools file
/// holds, answers `pong` to a call, and ends without a word when the tool
/// called is `die`. Beside its tools file, a `.slow` file makes it answer its
/// start a second late.
const SERVER: &str = r#"#!/bin/sh
log="$1"; tools="$2"
echo "start" >> "$log"
while IFS= read -r line; do
  method=$(printf '%s\n' "$line" | sed -nE 's/.*"method":"([^"]*)".*/\1/p')
  id=$(printf '%s\n' "$line" | sed -nE 's/.*"id":("[^"]*"|[0-9]+).*/\1/p')
  echo "heard $method" >> "$log"
  [ -z "$id" ] && continue
  case "$method" in
    initialize) [ -f "$tools.slow" ] && sleep 1; printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2025-06-18","capabilities":{"tools":{}},"serverInfo":{"name":"fixture","version":"1"},"instructions":"FIXTURE-INSTRUCTIONS"}}\n' "$id" ;;
    tools/list) printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":%s}}\n' "$id" "$(cat "$tools")" ;;
    tools/call)
      case "$line" in *'"name":"die"'*) echo "died" >> "$log"; exit 0 ;; esac
      printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"pong"}]}}\n' "$id" ;;
    ping) printf '{"jsonrpc":"2.0","id":%s,"result":{}}\n' "$id" ;;
    *) printf '{"jsonrpc":"2.0","id":%s,"error":{"code":-32601,"message":"no such method"}}\n' "$id" ;;
  esac
done
echo "end" >> "$log"
"#;

const SECRET_WORD: &str = "--fixture-secret-looking-word";

struct Fixture {
    dir: PathBuf,
    log: PathBuf,
    tools: PathBuf,
    script: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("zerocode-first-call-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("fixture folder");
        let fixture = Self {
            log: dir.join("server.log"),
            tools: dir.join("tools.json"),
            script: dir.join("server.sh"),
            dir,
        };
        std::fs::write(&fixture.script, SERVER).expect("server script");
        fixture.set_tools(&["ping"]);
        fixture
    }

    fn set_tools(&self, names: &[&str]) {
        let tools: Vec<Value> = names
            .iter()
            .map(|name| {
                json!({"name": name, "description": format!("{name} of the fixture"), "inputSchema": {"type": "object", "properties": {}}})
            })
            .collect();
        std::fs::write(&self.tools, Value::Array(tools).to_string()).expect("tools file");
    }

    fn listings(&self) -> Listings {
        Listings {
            dir: self.dir.join("listings"),
        }
    }

    fn args(&self) -> Vec<OsString> {
        vec![
            self.script.clone().into_os_string(),
            self.log.clone().into_os_string(),
            self.tools.clone().into_os_string(),
            SECRET_WORD.into(),
        ]
    }

    fn key(&self) -> String {
        listing_key("probe", "/bin/sh".as_ref(), &self.args())
    }

    fn log(&self) -> Vec<String> {
        std::fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    fn starts(&self) -> usize {
        self.log().iter().filter(|line| *line == "start").count()
    }

    /// A session of the stand-in in front of this server.
    fn session(&self, idle: Duration) -> Session {
        self.session_of("/bin/sh", &self.args(), idle)
    }

    fn session_of(&self, real: &str, args: &[OsString], idle: Duration) -> Session {
        let out = Arc::new(Mutex::new(Vec::new()));
        let stand_in = Arc::new(
            StandIn::new(
                "probe",
                real.as_ref(),
                args,
                Some(self.listings()),
                Box::new(Shared(Arc::clone(&out))),
            )
            .with_idle_after_learning(idle),
        );
        let (reader, writer) = std::io::pipe().expect("a pipe");
        let running = Arc::clone(&stand_in);
        let thread = std::thread::spawn(move || running.run(BufReader::new(reader)));
        Session {
            input: Some(writer),
            out,
            thread: Some(thread),
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

struct Shared(Arc<Mutex<Vec<u8>>>);

impl Write for Shared {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().expect("out").extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

struct Session {
    input: Option<std::io::PipeWriter>,
    out: Arc<Mutex<Vec<u8>>>,
    thread: Option<std::thread::JoinHandle<i32>>,
}

impl Session {
    fn send(&mut self, message: &Value) {
        let input = self.input.as_mut().expect("the session is open");
        writeln!(input, "{message}").expect("write to the stand-in");
    }

    fn request(&mut self, id: u64, method: &str, params: Value) -> Value {
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        self.answer(id)
    }

    fn answers(&self) -> Vec<Value> {
        let bytes = self.out.lock().expect("out").clone();
        String::from_utf8_lossy(&bytes)
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect()
    }

    /// The answer to `id`, waited for.
    fn answer(&self, id: u64) -> Value {
        let until = Instant::now() + Duration::from_secs(20);
        loop {
            if let Some(found) = self.answers().into_iter().find(|answer| answer["id"] == id) {
                return found;
            }
            assert!(
                Instant::now() < until,
                "no answer to {id}: {:?}",
                self.answers()
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// The client hangs up; the stand-in's exit code.
    fn close(&mut self) -> i32 {
        self.input.take();
        self.thread
            .take()
            .expect("running")
            .join()
            .expect("the stand-in's thread")
    }

    /// A session opening: what Claude Code 2.1.322 sent a server (2026-11-12).
    fn open(&mut self) -> (Value, Value) {
        self.request(0, "server/discover", json!({}));
        let initialize = self.request(
            1,
            "initialize",
            json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "claude-code", "version": "2.1.322"}}),
        );
        self.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
        let tools = self.request(2, "tools/list", json!({}));
        (initialize, tools)
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if self.thread.is_some() {
            let _ = self.close();
        }
    }
}

fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    let until = Instant::now() + Duration::from_secs(20);
    while !done() {
        assert!(Instant::now() < until, "{what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn tool_names(answer: &Value) -> Vec<String> {
    answer["result"]["tools"]
        .as_array()
        .map(|tools| {
            tools
                .iter()
                .filter_map(|tool| tool["name"].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// A server seen for the first time starts at once and its answers are kept —
/// what it answered and nothing of how it was started. The next session is
/// answered from them without a start, and its first call starts the server,
/// which is handed the client's own `initialize` first; the client is answered
/// once.
#[test]
fn a_listed_server_starts_on_its_first_call_and_ends_with_the_session() {
    let fixture = Fixture::new("listed");
    let mut first = fixture.session(Duration::from_secs(600));
    let (cold_initialize, cold_tools) = first.open();
    assert_eq!(fixture.starts(), 1, "a server never seen starts at once");
    assert_eq!(first.close(), 0);
    wait_until("the learning session's server ends with it", || {
        fixture.log().last().map(String::as_str) == Some("end")
    });

    let file = fixture
        .listings()
        .dir
        .join(format!("{}.json", fixture.key()));
    let text = std::fs::read_to_string(&file).expect("the listing was kept");
    for told in [
        SECRET_WORD,
        fixture.script.to_str().expect("path"),
        fixture.tools.to_str().expect("path"),
    ] {
        assert!(!text.contains(told), "the listing holds {told}: {text}");
    }
    let kept: Value = serde_json::from_str(&text).expect("a listing");
    let mut fields: Vec<&str> = kept
        .as_object()
        .expect("an object")
        .keys()
        .map(String::as_str)
        .collect();
    fields.sort_unstable();
    assert_eq!(
        fields,
        ["answers", "listed_unix", "protocol", "server", "version"]
    );
    let methods: Vec<&String> = kept["answers"]
        .as_object()
        .expect("answers")
        .keys()
        .collect();
    assert_eq!(methods, ["initialize", "server/discover", "tools/list"]);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(&file).expect("file").permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "the listing is the owner's alone");
    }

    let mut second = fixture.session(Duration::from_secs(600));
    let (warm_initialize, warm_tools) = second.open();
    assert_eq!(
        fixture.starts(),
        1,
        "a listed server is not started by a session opening"
    );
    assert_eq!(warm_initialize["result"], cold_initialize["result"]);
    assert_eq!(
        warm_initialize["result"]["instructions"], "FIXTURE-INSTRUCTIONS",
        "the server's instructions reach the client"
    );
    assert_eq!(warm_tools["result"], cold_tools["result"]);

    let call = second.request(3, "tools/call", json!({"name": "ping", "arguments": {}}));
    assert_eq!(call["result"]["content"][0]["text"], "pong", "{call}");
    assert_eq!(fixture.starts(), 2, "the first call started it");
    let heard: Vec<String> = fixture
        .log()
        .into_iter()
        .skip_while(|line| line != "end")
        .skip(1)
        .collect();
    assert_eq!(
        heard[..4],
        [
            "start",
            "heard initialize",
            "heard notifications/initialized",
            "heard tools/list",
        ],
        "started, it is handed the client's start first: {heard:?}"
    );
    assert!(heard.contains(&"heard tools/call".to_string()), "{heard:?}");
    let initialize_answers = second
        .answers()
        .into_iter()
        .filter(|answer| answer["id"] == 1)
        .count();
    assert_eq!(initialize_answers, 1, "the replayed start is not passed on");
    assert!(
        second.answers().iter().all(|answer| !answer["id"]
            .as_str()
            .is_some_and(|id| id.starts_with("zerocode-first-call-"))),
        "no answer to the stand-in's own requests reaches the client"
    );
    assert_eq!(second.close(), 0);
    wait_until("the server ends with the session", || {
        fixture.log().last().map(String::as_str) == Some("end")
    });
}

/// A session that calls nothing of a listed server never starts it.
#[test]
fn a_session_that_calls_nothing_starts_nothing() {
    let fixture = Fixture::new("idle");
    let mut first = fixture.session(Duration::from_secs(600));
    first.open();
    first.close();
    let mut second = fixture.session(Duration::from_secs(600));
    second.open();
    let ping = second.request(5, "ping", json!({}));
    assert_eq!(
        ping["result"],
        json!({}),
        "a ping is answered without the server"
    );
    second.close();
    assert_eq!(fixture.starts(), 1);
}

/// The tools a server lists when a call starts it replace a listing that
/// differs, for the next session.
#[test]
fn a_changed_tool_list_is_kept_for_the_next_session() {
    let fixture = Fixture::new("changed");
    let mut first = fixture.session(Duration::from_secs(600));
    first.open();
    first.close();
    fixture.set_tools(&["ping", "pong"]);

    let mut second = fixture.session(Duration::from_secs(600));
    let (_, tools) = second.open();
    assert_eq!(tool_names(&tools), ["ping"], "answered from the listing");
    second.request(3, "tools/call", json!({"name": "ping", "arguments": {}}));
    wait_until("the new listing is kept", || {
        fixture
            .listings()
            .recall(&fixture.key())
            .is_some_and(|listing| tool_names(&listing.answers["tools/list"]) == ["ping", "pong"])
    });
    second.close();

    let mut third = fixture.session(Duration::from_secs(600));
    let (_, tools) = third.open();
    assert_eq!(tool_names(&tools), ["ping", "pong"]);
    third.close();
}

/// A server started only to learn what it offers is stopped once the session
/// has asked it nothing else for a while, and the next call starts it again.
#[test]
fn a_server_started_to_learn_is_stopped_unused_and_started_again_by_a_call() {
    let fixture = Fixture::new("learn");
    let mut session = fixture.session(Duration::from_millis(300));
    session.open();
    wait_until("the unused server is stopped", || {
        fixture.log().last().map(String::as_str) == Some("end")
    });
    let call = session.request(3, "tools/call", json!({"name": "ping", "arguments": {}}));
    assert_eq!(call["result"]["content"][0]["text"], "pong", "{call}");
    assert_eq!(fixture.starts(), 2);
    session.close();
}

/// A call whose server is slow to answer its start is not cut short by the
/// stop of a server left unused: the request is out while the server starts,
/// so the start is given its whole limit.
#[test]
fn a_server_slow_to_start_for_a_call_is_not_taken_for_one_left_unused() {
    let fixture = Fixture::new("slow");
    let mut first = fixture.session(Duration::from_secs(600));
    first.open();
    first.close();
    std::fs::write(fixture.dir.join("tools.json.slow"), "").expect("the slow mark");

    let mut second = fixture.session(Duration::from_millis(200));
    second.open();
    let call = second.request(3, "tools/call", json!({"name": "ping", "arguments": {}}));
    assert_eq!(call["result"]["content"][0]["text"], "pong", "{call}");
    second.close();
}

/// A call to a listed server that does not start is answered with an error
/// that names the server and holds nothing of how it is started, and the
/// listing is forgotten: the next session starts the server at once.
#[test]
fn a_server_that_does_not_start_fails_the_call_by_name() {
    let fixture = Fixture::new("missing");
    let real = fixture.dir.join("no-such-server");
    let args = vec![OsString::from(SECRET_WORD)];
    let key = listing_key("probe", real.as_os_str(), &args);
    let mut listing = Listing::new("probe");
    listing.protocol = Some("2025-06-18".into());
    listing.listed_unix = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("now")
        .as_secs();
    listing.answers.insert(
        "initialize".into(),
        json!({"result": {"protocolVersion": "2025-06-18", "capabilities": {"tools": {}}, "serverInfo": {"name": "gone", "version": "1"}}}),
    );
    listing.answers.insert(
        "tools/list".into(),
        json!({"result": {"tools": [{"name": "ping"}]}}),
    );
    listing.answers.insert(
        "server/discover".into(),
        json!({"error": {"code": -32601, "message": "no such method"}}),
    );
    fixture
        .listings()
        .remember(&key, &listing)
        .expect("listing");

    let mut session = fixture.session_of(
        real.to_str().expect("path"),
        &args,
        Duration::from_secs(600),
    );
    let (initialize, tools) = session.open();
    assert!(
        initialize["result"].is_object() && tools["result"].is_object(),
        "the session opens on the listing: {initialize} {tools}"
    );
    let call = session.request(3, "tools/call", json!({"name": "ping", "arguments": {}}));
    let message = call["error"]["message"]
        .as_str()
        .expect("an error")
        .to_string();
    assert_eq!(call["error"]["code"], super::NOT_STARTED);
    assert!(message.contains("\"probe\""), "{message}");
    assert!(message.contains("could not be started"), "{message}");
    assert!(
        !message.contains(SECRET_WORD) && !message.contains("no-such-server"),
        "{message}"
    );
    assert!(
        fixture.listings().recall(&key).is_none(),
        "the listing is forgotten"
    );
    session.close();
}

/// A call the server ends on is answered, and so is a call to a server that
/// ends before it answers its start.
#[test]
fn a_server_that_ends_before_answering_fails_what_it_was_handed() {
    let fixture = Fixture::new("dies");
    let mut first = fixture.session(Duration::from_secs(600));
    first.open();
    let died = first.request(3, "tools/call", json!({"name": "die", "arguments": {}}));
    let message = died["error"]["message"].as_str().expect("an error");
    assert!(message.contains("ended before it answered"), "{message}");
    first.close();

    let args = vec![OsString::from("-c"), OsString::from("exit 3")];
    let key = listing_key("probe", "/bin/sh".as_ref(), &args);
    let mut listing = fixture.listings().recall(&fixture.key()).expect("listing");
    listing.listed_unix = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("now")
        .as_secs();
    fixture
        .listings()
        .remember(&key, &listing)
        .expect("listing");
    let mut second = fixture.session_of("/bin/sh", &args, Duration::from_secs(600));
    second.open();
    let call = second.request(3, "tools/call", json!({"name": "ping", "arguments": {}}));
    let message = call["error"]["message"].as_str().expect("an error");
    assert!(message.contains("ended before it answered"), "{message}");
    second.close();
}

/// A listing answers for a day, of its own shape, once it holds the server's
/// start and its tools; one dated ahead of now does not.
#[test]
fn a_listing_stands_for_a_day_when_whole() {
    let now = SystemTime::now();
    let at = now
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("now")
        .as_secs();
    let mut listing = Listing::new("probe");
    listing.listed_unix = at;
    assert!(!listing.stands(now, LISTING_STANDS_FOR), "an empty listing");
    listing
        .answers
        .insert("initialize".into(), json!({"result": {}}));
    listing
        .answers
        .insert("tools/list".into(), json!({"result": {"tools": []}}));
    assert!(listing.stands(now, LISTING_STANDS_FOR));
    assert!(
        !listing.stands(now + LISTING_STANDS_FOR, LISTING_STANDS_FOR),
        "a day old"
    );
    assert!(
        !listing.stands(now - Duration::from_secs(60), LISTING_STANDS_FOR),
        "dated ahead"
    );
    listing.version = 2;
    assert!(!listing.stands(now, LISTING_STANDS_FOR), "another shape");
}

/// The key is the name and a hash of the command line: the same line gives
/// the same key, another argument another, and no argument can be read in it.
#[test]
fn a_listing_is_filed_by_name_and_a_hash_of_the_command_line() {
    let line = |args: &[&str]| -> Vec<OsString> { args.iter().map(OsString::from).collect() };
    let key = listing_key(
        "lokalise-sd",
        "npx".as_ref(),
        &line(&["-y", "pkg", "--token=abc123"]),
    );
    assert_eq!(
        key,
        listing_key(
            "lokalise-sd",
            "npx".as_ref(),
            &line(&["-y", "pkg", "--token=abc123"])
        )
    );
    assert_ne!(
        key,
        listing_key(
            "lokalise-sd",
            "npx".as_ref(),
            &line(&["-y", "pkg", "--token=xyz789"])
        )
    );
    assert!(
        key.starts_with("lokalise-sd-") && key.len() == "lokalise-sd-".len() + 16,
        "{key}"
    );
    assert!(!key.contains("abc123"));
    assert_eq!(
        &listing_key("../a b", "x".as_ref(), &[])[..7],
        ".._a_b-",
        "a name is kept to a file name's characters"
    );
}

/// Where the listings are: the folder a test names, else the window's.
#[test]
fn listings_live_under_the_window_data_folder() {
    let listings = Listings::for_this_machine();
    if std::env::var_os(super::LISTINGS_ENV).is_none()
        && let Some(listings) = listings
    {
        assert!(
            listings
                .dir
                .ends_with(Path::new(crate::app::IDENTIFIER).join("mcp-listings")),
            "{}",
            listings.dir.display()
        );
    }
}
