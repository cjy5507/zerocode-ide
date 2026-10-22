use std::fs;
use std::net::{Ipv4Addr, Ipv6Addr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use judgment_provider::{Protocol, Provider, ProviderKind};
use serde_json::{Value, json};

use crate::engine::Engine;
use crate::manager::{Clock, EngineFactory, test_support};
use crate::{Binding, Error, Manager, Model, Snapshot};

const SETTLE: Duration = Duration::from_secs(10);
const HOUR: Duration = Duration::from_secs(60 * 60);
const ANSWERING: &str = r#"import json,os,sys,threading,time
root,number=sys.argv[1],sys.argv[2]
def at(name): return os.path.join(root,name+'-'+number)
def watch():
 while not os.path.exists(at('end')): time.sleep(0.005)
 how,_,value=open(at('end')).read().partition(' ')
 if how=='code': os._exit(int(value))
 os.kill(os.getpid(),int(value))
threading.Thread(target=watch,daemon=True).start()
if os.path.exists(at('fail')): os._exit(3)
if os.path.exists(at('hold')):
 while not os.path.exists(at('go')): time.sleep(0.005)
print('{"ready":true}',flush=True)
for line in sys.stdin:
 request=json.loads(line);answers={}
 with open(at('requests'),'a') as receipt: receipt.write(line)
 if os.path.exists(at('hold-check')):
  while not os.path.exists(at('go-check')): time.sleep(0.005)
 for name,question in request['questions'].items():
  if question['type']=='noul': answers[name]={'type':'noul','noul':0.9}
  elif question['type']=='choice':
   keys=list(question['criteria']);probabilities={key:(1.0 if index==0 else 0.0) for index,key in enumerate(keys)}
   answers[name]={'type':'choice','choice':keys[0],'confidence':1.0,'probabilities':probabilities}
  else:
   count=len(question['criteria']);probabilities={str(index):(1.0 if index==0 else 0.0) for index in range(count)}
   answers[name]={'type':'score','score':0.0,'confidence':1.0,'probabilities':probabilities,'legend':{str(index):value for index,value in enumerate(question['criteria'])}}
 print(json.dumps({'model':request['model'],'answers':answers,'usage':{'input_tokens':1,'output_tokens':0}}),flush=True)
"#;

type Waiting = Vec<(Duration, Box<dyn FnOnce() + Send>)>;

#[derive(Clone)]
struct Manual(Arc<Mutex<Hands>>);

struct Hands {
    start: Instant,
    moved: Duration,
    due: Waiting,
}

impl Manual {
    fn new() -> Self {
        Self(Arc::new(Mutex::new(Hands {
            start: Instant::now(),
            moved: Duration::ZERO,
            due: Vec::new(),
        })))
    }

    fn advance(&self, elapsed: Duration) {
        let ready = {
            let mut hands = self.0.lock().unwrap();
            hands.moved += elapsed;
            let now = hands.moved;
            let (ready, waiting): (Waiting, Waiting) = std::mem::take(&mut hands.due)
                .into_iter()
                .partition(|(due, _)| *due <= now);
            hands.due = waiting;
            ready
        };
        for (_, action) in ready {
            action();
        }
    }
}

impl Clock for Manual {
    fn now(&self) -> Instant {
        let hands = self.0.lock().unwrap();
        hands.start + hands.moved
    }

    fn after(
        &self,
        _runtime: &tokio::runtime::Handle,
        wait: Duration,
        action: Box<dyn FnOnce() + Send>,
    ) {
        let mut hands = self.0.lock().unwrap();
        let due = hands.moved + wait;
        hands.due.push((due, action));
    }
}

fn factory(control: PathBuf, starts: Arc<AtomicUsize>) -> EngineFactory {
    Arc::new(move |_| {
        let number = starts.fetch_add(1, Ordering::SeqCst) + 1;
        Ok(Engine::from_command(
            Command::new("python3")
                .args(["-I", "-u", "-c", ANSWERING])
                .arg(&control)
                .arg(number.to_string()),
        ))
    })
}

fn save(path: &Path, provider: &Provider, expected: &str, binding: &Binding) -> crate::Result<()> {
    if Provider::read(path).map_err(|_| Error::Storage)?.identity() != expected {
        return Err(Error::SettingsChanged);
    }
    select(path, provider, binding);
    Ok(())
}

fn select(path: &Path, provider: &Provider, binding: &Binding) {
    let mut root: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    root["smart"] = json!({"judgmentProvider": provider, (crate::MANAGED_MODEL_KEY): binding});
    root["preserved"] = json!("unchanged");
    fs::write(path, serde_json::to_vec_pretty(&root).unwrap()).unwrap();
}

fn port(provider: &Provider) -> u16 {
    url::Url::parse(&provider.base_url).unwrap().port().unwrap()
}

/// A status read as JSON, without what the supervision says of it: the state
/// a start that restores nothing leaves as it found it.
fn state_of(snapshot: Snapshot) -> Value {
    let mut state = serde_json::to_value(snapshot).unwrap();
    state.as_object_mut().unwrap().remove("supervision");
    state
}

/// Why a start restored nothing, as a status read says it.
fn reason_of(snapshot: &Snapshot) -> Value {
    serde_json::to_value(snapshot).unwrap()["supervision"]["reason"].clone()
}

struct Restoring {
    _home: tempfile::TempDir,
    control: tempfile::TempDir,
    settings: PathBuf,
    data: PathBuf,
    manager: Arc<Manager>,
    models: Vec<Model>,
    starts: Arc<AtomicUsize>,
    writes: Arc<AtomicUsize>,
    swap_calls: Arc<AtomicUsize>,
    before_swap: Arc<Mutex<Option<Vec<u8>>>>,
    transaction: Arc<Mutex<()>>,
    clock: Manual,
}

impl Drop for Restoring {
    fn drop(&mut self) {
        self.manager.shutdown();
    }
}

impl Restoring {
    fn new() -> Self {
        let home = tempfile::tempdir().unwrap();
        let control = tempfile::tempdir().unwrap();
        let settings = home.path().join("settings.json");
        fs::write(&settings, b"{}").unwrap();
        let data = home.path().join("data");
        let model = crate::tests::fixture_model();
        let mut other = model.clone();
        other.id = format!("{}-other", model.id);
        let models = vec![model, other];
        let starts = Arc::new(AtomicUsize::new(0));
        let manager = test_support::installed_manager(
            &data,
            &settings,
            models.clone(),
            factory(control.path().to_path_buf(), starts.clone()),
            Arc::new(|_| {}),
        );
        let clock = Manual::new();
        test_support::use_clock(&manager, Arc::new(clock.clone()));
        Self {
            _home: home,
            control,
            settings,
            data,
            manager,
            models,
            starts,
            writes: Arc::new(AtomicUsize::new(0)),
            swap_calls: Arc::new(AtomicUsize::new(0)),
            before_swap: Arc::new(Mutex::new(None)),
            transaction: Arc::new(Mutex::new(())),
            clock,
        }
    }

    fn starts(&self) -> usize {
        self.starts.load(Ordering::SeqCst)
    }

    fn intent_path(&self) -> PathBuf {
        self.data.join("managed-judgment/run-intent.json")
    }

    fn intent(&self) -> Value {
        serde_json::from_slice(&fs::read(self.intent_path()).unwrap()).unwrap()
    }

    fn keep_intent(&self, provider: &Provider) {
        let model = &self.models[0];
        fs::write(
            self.intent_path(),
            serde_json::to_vec(&json!({
                "version": 1,
                "modelId": model.id,
                "revision": model.revision,
                "manifestFingerprint": model.fingerprint().unwrap(),
                "providerIdentity": provider.identity(),
                "port": port(provider),
                "atMs": 1,
            }))
            .unwrap(),
        )
        .unwrap();
    }

    fn seed(&self) {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let provider = Provider {
            kind: ProviderKind::Local,
            protocol: Protocol::SystemOne,
            base_url: format!("http://{}", listener.local_addr().unwrap()),
            model: self.models[0].id.clone(),
            requires_auth: false,
        };
        select(
            &self.settings,
            &provider,
            &Binding {
                model_id: self.models[0].id.clone(),
                revision: self.models[0].revision.clone(),
                provider_identity: provider.identity(),
            },
        );
        self.keep_intent(&provider);
    }

    fn reopen_with_runtime(&mut self, verified: bool) {
        self.manager.shutdown();
        self.manager = test_support::reopened_manager(
            &self.data,
            &self.settings,
            self.models.clone(),
            factory(self.control.path().to_path_buf(), self.starts.clone()),
            Arc::new(|_| {}),
            verified,
        );
        test_support::use_clock(&self.manager, Arc::new(self.clock.clone()));
    }

    async fn reopen(&mut self) {
        let provider = self.manager.snapshot().server.provider;
        self.manager.shutdown();
        if let Some(provider) = provider {
            let began = Instant::now();
            loop {
                if TcpListener::bind((Ipv4Addr::LOCALHOST, port(&provider))).is_ok() {
                    break;
                }
                assert!(began.elapsed() < SETTLE, "owned listener did not close");
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        }
        self.reopen_with_runtime(true);
    }

    fn restore(&self, allowed: bool) -> crate::Result<Snapshot> {
        let path = self.settings.clone();
        let writes = self.writes.clone();
        let swap_calls = self.swap_calls.clone();
        let before_swap = self.before_swap.clone();
        let writer = self.transaction.clone();
        let guard = self.transaction.clone();
        self.manager.restore_guarded(
            allowed,
            (
                move |provider, expected, binding| {
                    let _writing = writer.try_lock().map_err(|_| Error::Busy)?;
                    swap_calls.fetch_add(1, Ordering::SeqCst);
                    if let Some(newer) = before_swap.lock().unwrap().take() {
                        fs::write(&path, newer).map_err(|_| Error::Storage)?;
                    }
                    save(&path, provider, expected, binding)?;
                    writes.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                },
                move |action: &mut dyn FnMut() -> crate::Result<()>| {
                    let _guarded = guard.try_lock().map_err(|_| Error::Busy)?;
                    action()
                },
            ),
        )
    }

    async fn start(&self, model: &Model) -> Snapshot {
        self.manager.start(&model.id, &model.revision).unwrap();
        let snapshot = crate::tests::settled_snapshot(&self.manager).await;
        assert_eq!(snapshot.server.state, "ready");
        snapshot
    }

    async fn apply(&self, server: &str) -> Provider {
        self.manager.check(server).unwrap();
        let checked = crate::tests::settled_snapshot(&self.manager).await;
        assert!(checked.server.checked);
        self.manager
            .apply(
                server,
                checked.provider_identity.as_deref().unwrap(),
                |provider, expected, binding| save(&self.settings, provider, expected, binding),
            )
            .unwrap()
            .server
            .provider
            .unwrap()
    }

    async fn press(&self) -> (String, Provider) {
        let server = self.start(&self.models[0]).await.server.id.unwrap();
        let provider = self.apply(&server).await;
        (server, provider)
    }

    async fn checking(&self, server: &str) -> String {
        let number = self.starts();
        let requests = self.control.path().join(format!("requests-{number}"));
        let previous = fs::read_to_string(&requests).unwrap().lines().count();
        self.tell("hold-check", number);
        let checking = self.manager.check(server).unwrap();
        let operation = checking.operation.unwrap();
        assert_eq!(operation.kind, "check");
        let began = Instant::now();
        while fs::read_to_string(&requests).unwrap().lines().count() <= previous {
            assert!(
                began.elapsed() < SETTLE,
                "held check did not reach the runtime"
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        operation.id
    }

    fn tell(&self, name: &str, number: usize) {
        fs::write(self.control.path().join(format!("{name}-{number}")), b"").unwrap();
    }

    async fn started(&self, count: usize) {
        let began = Instant::now();
        while self.starts() < count {
            assert!(began.elapsed() < SETTLE, "restore runtime did not start");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    fn attempts(&self) -> Vec<Value> {
        self.kept("restore")
    }

    /// The store's kept records of `kind`.
    fn kept(&self, kind: &str) -> Vec<Value> {
        fs::read_to_string(self.data.join("managed-judgment/server-exits.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .filter(|record| record["kind"] == kind)
            .collect()
    }

    /// A manager opening the same store and settings, as the next window
    /// start does, while this one is left as it is.
    fn next_window(&self) -> Arc<Manager> {
        test_support::reopened_manager(
            &self.data,
            &self.settings,
            self.models.clone(),
            factory(self.control.path().to_path_buf(), self.starts.clone()),
            Arc::new(|_| {}),
            true,
        )
    }

    async fn recorded(&self) -> Vec<Value> {
        let began = Instant::now();
        loop {
            let records = self.attempts();
            if !records.is_empty() {
                return records;
            }
            assert!(began.elapsed() < SETTLE, "restore attempt was not recorded");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    fn relocated(&self, previous: &Provider, current: &Provider, occupant: &TcpListener) {
        assert_eq!(self.starts(), 1);
        assert_ne!(current.identity(), previous.identity());
        let selected = crate::selection::read(&self.settings).unwrap();
        assert_eq!(selected.provider.identity(), current.identity());
        assert_eq!(
            selected.binding.unwrap().provider_identity,
            current.identity()
        );
        let intent = self.intent();
        assert_eq!(intent["providerIdentity"], current.identity());
        assert_eq!(intent["port"], port(current));
        assert_eq!(self.swap_calls.load(Ordering::SeqCst), 1);
        assert_eq!(self.writes.load(Ordering::SeqCst), 1);
        assert!(
            matches!(occupant.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock),
            "wildcard occupant must accept no restore connection"
        );
    }
}

#[tokio::test]
async fn restore_gate_skips_without_changing_state_and_has_a_live_control() {
    for case in [
        "off",
        "unmanaged",
        "missing_model",
        "uncatalogued_model",
        "missing_runtime",
        "unverified_runtime",
        "unsupported",
        "stopped",
    ] {
        let mut fixture = Restoring::new();
        fixture.seed();
        match case {
            "unmanaged" => fs::write(&fixture.settings, b"{}").unwrap(),
            "missing_model" => {
                let directory = fixture.manager.snapshot().models[0]
                    .installed_directory
                    .clone()
                    .unwrap();
                fs::remove_file(Path::new(&directory).join(&fixture.models[0].files[0].path))
                    .unwrap();
            }
            "uncatalogued_model" => {
                fixture.models.clear();
                fixture.reopen_with_runtime(true);
            }
            "missing_runtime" => {
                fs::remove_dir_all(fixture.data.join("managed-judgment/runtime")).unwrap()
            }
            "unverified_runtime" => fixture.reopen_with_runtime(false),
            "unsupported" => {
                let directory = fixture.manager.snapshot().models[0]
                    .installed_directory
                    .clone()
                    .unwrap();
                fixture.models[0].engine = crate::EngineKind::SystemOneServer;
                fs::write(
                    Path::new(&directory).join("receipt.json"),
                    serde_json::to_vec(
                        &json!({"fingerprint": fixture.models[0].fingerprint().unwrap()}),
                    )
                    .unwrap(),
                )
                .unwrap();
                fixture.reopen_with_runtime(true);
                fixture.seed();
            }
            "stopped" => {
                let (server, _) = fixture.press().await;
                fixture.manager.stop(&server).unwrap();
                fixture.reopen().await;
                fixture.starts.store(0, Ordering::SeqCst);
            }
            _ => {}
        }
        let before = state_of(fixture.manager.snapshot());
        let after = fixture.restore(case != "off").unwrap();
        assert_eq!(state_of(after), before);
        fixture.clock.advance(HOUR);
        tokio::task::yield_now().await;
        let after = fixture.manager.snapshot();
        assert_eq!(fixture.starts(), 0, "excluded restore started for {case}");
        assert!(
            after.operation.is_none(),
            "excluded restore reserved for {case}"
        );
        let reason = reason_of(&after);
        assert_eq!(
            state_of(after),
            before,
            "excluded restore changed state for {case}"
        );
        // The status says why the start restored nothing; settings that
        // choose no managed model have nothing to say.
        let wanted = match case {
            "off" => json!("jev_off"),
            "unmanaged" => Value::Null,
            "missing_model" => json!("model_missing"),
            "uncatalogued_model" => json!("unknown_model"),
            "missing_runtime" => json!("runtime_missing"),
            "unverified_runtime" => json!("engine_unverified"),
            "unsupported" => json!("engine_not_supported"),
            _ => json!("no_intent"),
        };
        assert_eq!(reason, wanted, "why the start restored nothing, for {case}");
    }
    let fixture = Restoring::new();
    fixture.seed();
    let starting = fixture.restore(true).unwrap();
    assert!(
        starting.operation.is_some(),
        "valid intent must reserve a restore operation"
    );
    let ready = crate::tests::settled_snapshot(&fixture.manager).await;
    assert_eq!(fixture.starts(), 1, "valid intent must start one runtime");
    assert_eq!(ready.server.state, "ready");
    assert!(ready.server.checked);
}

#[tokio::test]
async fn invalid_intents_skip_and_a_valid_intent_still_restores() {
    for case in [
        "missing",
        "corrupt",
        "oversized",
        "unknown_field",
        "version",
        "model",
        "revision",
        "fingerprint",
        "identity",
        "port",
    ] {
        let fixture = Restoring::new();
        fixture.seed();
        let mut intent = fixture.intent();
        match case {
            "missing" => fs::remove_file(fixture.intent_path()).unwrap(),
            "corrupt" => fs::write(fixture.intent_path(), b"{").unwrap(),
            "oversized" => {
                let mut bytes = serde_json::to_vec(&intent).unwrap();
                bytes.resize(1025, b' ');
                fs::write(fixture.intent_path(), bytes).unwrap();
            }
            _ => {
                match case {
                    "unknown_field" => intent["unexpected"] = json!(true),
                    "version" => intent["version"] = json!(2),
                    "model" => intent["modelId"] = json!(fixture.models[1].id),
                    "revision" => {
                        intent["revision"] = json!(&fixture.models[0].files[0].sha256[..40])
                    }
                    "fingerprint" => {
                        intent["manifestFingerprint"] =
                            json!(fixture.models[1].fingerprint().unwrap())
                    }
                    "identity" => {
                        intent["providerIdentity"] =
                            json!(Provider::from_root(&json!({})).unwrap().identity())
                    }
                    "port" => intent["port"] = json!(0),
                    _ => unreachable!(),
                }
                fs::write(fixture.intent_path(), serde_json::to_vec(&intent).unwrap()).unwrap();
            }
        }
        let before = state_of(fixture.manager.snapshot());
        let after = fixture.restore(true).unwrap();
        assert_eq!(state_of(after), before);
        fixture.clock.advance(HOUR);
        tokio::task::yield_now().await;
        let after = fixture.manager.snapshot();
        assert_eq!(
            fixture.starts(),
            0,
            "invalid {case} intent started a runtime"
        );
        assert!(
            after.operation.is_none(),
            "invalid {case} intent reserved an operation"
        );
        let reason = reason_of(&after);
        assert_eq!(state_of(after), before);
        let wanted = match case {
            "model" | "revision" | "identity" => "selection_changed",
            "fingerprint" => "model_revision_changed",
            _ => "no_intent",
        };
        assert_eq!(
            reason, wanted,
            "why an invalid {case} intent restored nothing"
        );
    }
    let fixture = Restoring::new();
    fixture.seed();
    let starting = fixture.restore(true).unwrap();
    assert!(
        starting.operation.is_some(),
        "valid intent after malformed controls must reserve an operation"
    );
    let ready = crate::tests::settled_snapshot(&fixture.manager).await;
    assert_eq!(
        fixture.starts(),
        1,
        "valid intent after malformed controls must restore"
    );
    assert!(ready.server.checked);
}

#[tokio::test]
async fn shutdown_restores_once_on_the_same_port_without_rewriting_settings() {
    let mut fixture = Restoring::new();
    let (_, provider) = fixture.press().await;
    fixture.keep_intent(&provider);
    let settings = fs::read(&fixture.settings).unwrap();
    let intent = fs::read(fixture.intent_path()).unwrap();
    fixture.reopen().await;
    assert_eq!(fs::read(fixture.intent_path()).unwrap(), intent);
    let starting = fixture.restore(true).unwrap();
    assert!(
        starting.operation.is_some(),
        "shutdown intent must reserve one restore operation"
    );
    let ready = crate::tests::settled_snapshot(&fixture.manager).await;
    assert_eq!(
        fixture.starts(),
        2,
        "shutdown intent must restore exactly one runtime"
    );
    assert_eq!(ready.server.state, "ready");
    assert!(ready.server.checked);
    assert_eq!(
        ready.applied_model_id.as_deref(),
        Some(fixture.models[0].id.as_str())
    );
    assert_eq!(
        ready.server.provider.as_ref().unwrap().identity(),
        provider.identity()
    );
    assert_eq!(
        port(ready.server.provider.as_ref().unwrap()),
        port(&provider)
    );
    assert_eq!(fs::read(&fixture.settings).unwrap(), settings);
    assert_eq!(fs::read(fixture.intent_path()).unwrap(), intent);
    assert_eq!(fixture.writes.load(Ordering::SeqCst), 0);
    assert!(
        fs::read_to_string(fixture.control.path().join("requests-2"))
            .is_ok_and(|requests| !requests.is_empty())
    );
    let server = ready.server.id.clone();
    let operation = ready.operation.as_ref().unwrap().id.clone();
    fixture.restore(true).unwrap();
    let repeated = crate::tests::settled_snapshot(&fixture.manager).await;
    assert_eq!(fixture.starts(), 2);
    assert_eq!(repeated.server.id, server);
    assert_eq!(repeated.operation.unwrap().id, operation);
    assert_eq!(fixture.recorded().await.len(), 1);
}

#[tokio::test]
async fn an_occupied_port_is_replaced_without_contacting_its_listener() {
    let mut fixture = Restoring::new();
    let (_, previous) = fixture.press().await;
    fixture.keep_intent(&previous);
    fixture.reopen().await;
    let occupant = TcpListener::bind((Ipv4Addr::LOCALHOST, port(&previous))).unwrap();
    occupant.set_nonblocking(true).unwrap();
    let starting = fixture.restore(true).unwrap();
    assert!(
        starting.operation.is_some(),
        "occupied-port intent must reserve one restore operation"
    );
    let ready = crate::tests::settled_snapshot(&fixture.manager).await;
    assert_eq!(
        fixture.starts(),
        2,
        "occupied-port restore must start on a fresh owned listener"
    );
    assert_eq!(ready.server.state, "ready");
    assert!(ready.server.checked);
    let current = ready.server.provider.unwrap();
    assert_ne!(port(&current), port(&previous));
    assert_ne!(current.identity(), previous.identity());
    let selected = crate::selection::read(&fixture.settings).unwrap();
    assert_eq!(selected.provider.identity(), current.identity());
    assert_eq!(
        selected.binding.unwrap().provider_identity,
        current.identity()
    );
    let intent = fixture.intent();
    assert_eq!(intent["providerIdentity"], current.identity());
    assert_eq!(intent["port"], port(&current));
    assert_eq!(fixture.writes.load(Ordering::SeqCst), 1);
    assert!(
        matches!(occupant.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock),
        "restore contacted the occupied listener"
    );
}

#[tokio::test]
async fn an_ipv4_wildcard_occupant_forces_a_fresh_restore_port() {
    let mut fixture = Restoring::new();
    fixture.seed();
    fixture.reopen_with_runtime(true);
    let previous = Provider::read(&fixture.settings).unwrap();
    let occupant = TcpListener::bind((Ipv4Addr::UNSPECIFIED, port(&previous))).unwrap();
    occupant.set_nonblocking(true).unwrap();
    fixture.restore(true).unwrap();
    let ready = crate::tests::settled_snapshot(&fixture.manager).await;
    assert_eq!(ready.server.state, "ready");
    assert!(ready.server.checked);
    let current = ready.server.provider.unwrap();
    assert_ne!(
        port(&current),
        port(&previous),
        "IPv4 wildcard occupant must force a fresh restore port"
    );
    fixture.relocated(&previous, &current, &occupant);
}

#[tokio::test]
async fn an_ipv6_wildcard_occupant_forces_a_fresh_restore_port() {
    let mut fixture = Restoring::new();
    fixture.seed();
    fixture.reopen_with_runtime(true);
    let previous = Provider::read(&fixture.settings).unwrap();
    let occupant = TcpListener::bind((Ipv6Addr::UNSPECIFIED, port(&previous))).unwrap();
    occupant.set_nonblocking(true).unwrap();
    fixture.restore(true).unwrap();
    let ready = crate::tests::settled_snapshot(&fixture.manager).await;
    assert_eq!(ready.server.state, "ready");
    assert!(ready.server.checked);
    let current = ready.server.provider.unwrap();
    assert_ne!(
        port(&current),
        port(&previous),
        "IPv6 wildcard occupant must force a fresh restore port"
    );
    fixture.relocated(&previous, &current, &occupant);
}

#[tokio::test]
async fn an_owned_time_wait_keeps_the_restore_port_without_settings_writes() {
    use tokio::io::AsyncReadExt;

    let mut fixture = Restoring::new();
    fixture.seed();
    fixture.reopen_with_runtime(true);
    let previous = Provider::read(&fixture.settings).unwrap();
    let settings = fs::read(&fixture.settings).unwrap();
    let intent = fs::read(fixture.intent_path()).unwrap();
    let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, port(&previous)))
        .await
        .unwrap();
    let mut client = tokio::net::TcpStream::connect(listener.local_addr().unwrap())
        .await
        .unwrap();
    let (connection, _) = listener.accept().await.unwrap();
    drop(connection);
    assert_eq!(client.read(&mut [0_u8; 1]).await.unwrap(), 0);
    drop(client);
    drop(listener);
    let probe = tokio::net::TcpSocket::new_v4().unwrap();
    assert!(!probe.reuseaddr().unwrap());
    assert!(
        matches!(probe.bind((Ipv4Addr::LOCALHOST, port(&previous)).into()), Err(error) if error.kind() == std::io::ErrorKind::AddrInUse),
        "TIME_WAIT guard must leave the recorded server port in use"
    );
    fixture.restore(true).unwrap();
    let ready = crate::tests::settled_snapshot(&fixture.manager).await;
    assert_eq!(fixture.starts(), 1);
    assert_eq!(ready.server.state, "ready");
    assert!(ready.server.checked);
    assert_eq!(
        port(ready.server.provider.as_ref().unwrap()),
        port(&previous),
        "owned TIME_WAIT must keep the recorded restore port"
    );
    assert_eq!(fs::read(&fixture.settings).unwrap(), settings);
    assert_eq!(fs::read(fixture.intent_path()).unwrap(), intent);
    assert_eq!(fixture.swap_calls.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.writes.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_person_cancelled_check_clears_intent_and_cannot_restore() {
    let mut fixture = Restoring::new();
    let (server, _) = fixture.press().await;
    assert!(fixture.intent_path().is_file());
    let operation = fixture.checking(&server).await;
    assert_eq!(fixture.manager.stop(&server).err(), Some(Error::Busy));
    fixture.manager.cancel(&operation).unwrap();
    let stopped = crate::tests::settled_snapshot(&fixture.manager).await;
    assert_eq!(stopped.server.state, "stopped");
    assert_eq!(stopped.operation.unwrap().state, "cancelled");
    fixture.reopen().await;
    assert!(
        !fixture.intent_path().exists(),
        "person-cancelled check must remove its run intent before restart"
    );
    fixture.restore(true).unwrap();
    fixture.clock.advance(HOUR);
    let skipped = fixture.manager.snapshot();
    assert!(skipped.operation.is_none());
    assert_eq!(skipped.server.state, "stopped");
    assert_eq!(fixture.starts(), 1);
}

#[tokio::test]
async fn a_check_ended_by_shutdown_keeps_its_intent_for_restore() {
    let mut fixture = Restoring::new();
    let (server, _) = fixture.press().await;
    let intent = fs::read(fixture.intent_path()).unwrap();
    fixture.checking(&server).await;
    fixture.manager.shutdown();
    let stopped = crate::tests::settled_snapshot(&fixture.manager).await;
    assert_eq!(stopped.server.state, "stopped");
    assert_eq!(stopped.operation.unwrap().state, "cancelled");
    assert_eq!(
        fs::read(fixture.intent_path()).unwrap(),
        intent,
        "shutdown during a check must preserve the run intent"
    );
    fixture.reopen().await;
    fixture.restore(true).unwrap();
    let restored = crate::tests::settled_snapshot(&fixture.manager).await;
    assert_eq!(restored.server.state, "ready");
    assert!(restored.server.checked);
    assert_eq!(fixture.starts(), 2);
}

#[tokio::test]
async fn a_person_cancelled_check_reports_intent_removal_errors() {
    let fixture = Restoring::new();
    let (server, _) = fixture.press().await;
    let operation = fixture.checking(&server).await;
    fs::remove_file(fixture.intent_path()).unwrap();
    fs::create_dir(fixture.intent_path()).unwrap();
    assert_eq!(
        fixture.manager.cancel(&operation).err(),
        Some(Error::UnsafePath),
        "person-cancelled check must report run intent removal errors"
    );
    let stopped = crate::tests::settled_snapshot(&fixture.manager).await;
    assert_eq!(stopped.server.state, "stopped");
    assert!(fixture.intent_path().is_dir());
    assert_eq!(fixture.starts(), 1);
}

#[tokio::test]
async fn a_cancel_after_check_completion_keeps_the_run_intent() {
    let fixture = Restoring::new();
    let (server, _) = fixture.press().await;
    let intent = fs::read(fixture.intent_path()).unwrap();
    fixture.manager.check(&server).unwrap();
    let checked = crate::tests::settled_snapshot(&fixture.manager).await;
    assert_eq!(checked.operation.as_ref().unwrap().state, "done");
    fixture
        .manager
        .cancel(&checked.operation.unwrap().id)
        .unwrap();
    let unchanged = fixture.manager.snapshot();
    assert_eq!(unchanged.server.state, "ready");
    assert!(unchanged.server.checked);
    assert_eq!(fs::read(fixture.intent_path()).unwrap(), intent);
    assert_eq!(fixture.starts(), 1);
}

#[tokio::test]
async fn apply_persists_stop_clears_and_unapplied_start_leaves_intent_alone() {
    use std::os::unix::fs::PermissionsExt;

    let mut fixture = Restoring::new();
    let first = fixture.start(&fixture.models[0]).await;
    assert!(
        !fixture.intent_path().exists(),
        "unapplied start wrote an intent"
    );
    let provider = fixture.apply(first.server.id.as_deref().unwrap()).await;
    assert!(
        fixture.intent_path().is_file(),
        "apply must create a private run intent"
    );
    let intent = fixture.intent();
    assert_eq!(intent["version"], 1);
    assert_eq!(intent["modelId"], fixture.models[0].id);
    assert_eq!(intent["revision"], fixture.models[0].revision);
    assert_eq!(
        intent["manifestFingerprint"],
        fixture.models[0].fingerprint().unwrap()
    );
    assert_eq!(intent["providerIdentity"], provider.identity());
    assert_eq!(intent["port"], port(&provider));
    assert!(intent["atMs"].as_u64().is_some_and(|value| value > 0));
    assert_eq!(
        fs::metadata(fixture.intent_path())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    if let Some(destination) = std::env::var_os("T62479_INTENT_EXAMPLE") {
        fs::copy(fixture.intent_path(), destination).unwrap();
    }
    let bytes = fs::read(fixture.intent_path()).unwrap();
    fixture.reopen().await;
    assert_eq!(fs::read(fixture.intent_path()).unwrap(), bytes);
    fixture.start(&fixture.models[1]).await;
    assert_eq!(
        fs::read(fixture.intent_path()).unwrap(),
        bytes,
        "unapplied start cleared or rewrote the previous intent"
    );
    fixture.reopen().await;
    fixture.restore(true).unwrap();
    let restored = crate::tests::settled_snapshot(&fixture.manager).await;
    assert_eq!(fixture.starts(), 3);
    assert_eq!(
        restored.server.model_id.as_deref(),
        Some(fixture.models[0].id.as_str())
    );
    fixture
        .manager
        .stop(restored.server.id.as_deref().unwrap())
        .unwrap();
    assert!(
        !fixture.intent_path().exists(),
        "person stop kept the intent"
    );
    fixture.reopen().await;
    let before = state_of(fixture.manager.snapshot());
    fixture.restore(true).unwrap();
    assert_eq!(fixture.starts(), 3);
    let after = fixture.manager.snapshot();
    assert_eq!(
        reason_of(&after),
        "no_intent",
        "why the start after the window's stop restored nothing"
    );
    assert_eq!(state_of(after), before);
}

#[tokio::test]
async fn cancel_stop_selection_changes_and_shutdown_prevent_late_activation() {
    for case in ["cancel", "stop", "provider", "binding", "shutdown"] {
        let fixture = Restoring::new();
        fixture.seed();
        fixture.tell("hold", 1);
        let starting = fixture.restore(true).unwrap();
        assert!(
            starting
                .operation
                .as_ref()
                .is_some_and(|operation| operation.kind == "restore"),
            "restore must expose a cancellable operation"
        );
        let operation = starting.operation.as_ref().unwrap().id.clone();
        fixture.started(1).await;
        match case {
            "cancel" => {
                fixture.manager.cancel(&operation).unwrap();
            }
            "stop" => {
                fixture
                    .manager
                    .stop(starting.server.id.as_deref().unwrap_or_default())
                    .unwrap();
            }
            "provider" => {
                fs::write(&fixture.settings, b"{}").unwrap();
                fixture.manager.notify_settings_changed();
            }
            "binding" => {
                let mut root: Value =
                    serde_json::from_slice(&fs::read(&fixture.settings).unwrap()).unwrap();
                root["smart"][crate::MANAGED_MODEL_KEY]["revision"] =
                    json!(&fixture.models[0].files[0].sha256[..40]);
                fs::write(&fixture.settings, serde_json::to_vec(&root).unwrap()).unwrap();
                fixture.manager.notify_settings_changed();
            }
            "shutdown" => fixture.manager.shutdown(),
            _ => unreachable!(),
        }
        let settings = fs::read(&fixture.settings).unwrap();
        fixture.tell("go", 1);
        let finished = crate::tests::settled_snapshot(&fixture.manager).await;
        assert_eq!(fixture.starts(), 1);
        assert_ne!(
            finished.server.state, "ready",
            "{case} installed a late server"
        );
        assert!(
            finished.server.provider.is_none(),
            "{case} left an installed server"
        );
        assert!(!finished.server.checked);
        assert_eq!(fs::read(&fixture.settings).unwrap(), settings);
        assert_eq!(fixture.writes.load(Ordering::SeqCst), 0);
        assert_eq!(
            fixture.intent_path().exists(),
            case == "shutdown",
            "wrong intent persistence for {case}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_restores_share_one_operation_and_block_manual_start() {
    let fixture = Restoring::new();
    fixture.seed();
    fixture.tell("hold", 1);
    let runtime = tokio::runtime::Handle::current();
    let barrier = std::sync::Barrier::new(3);
    let (first, second) = std::thread::scope(|threads| {
        let first = threads.spawn(|| {
            let _entered = runtime.enter();
            barrier.wait();
            fixture.restore(true).unwrap()
        });
        let second = threads.spawn(|| {
            let _entered = runtime.enter();
            barrier.wait();
            fixture.restore(true).unwrap()
        });
        barrier.wait();
        (first.join().unwrap(), second.join().unwrap())
    });
    assert!(
        first
            .operation
            .as_ref()
            .is_some_and(|operation| operation.kind == "restore"),
        "concurrent restores must reserve one operation"
    );
    assert_eq!(first.operation.unwrap().id, second.operation.unwrap().id);
    assert!(matches!(
        fixture
            .manager
            .start(&fixture.models[0].id, &fixture.models[0].revision),
        Err(Error::Busy)
    ));
    fixture.started(1).await;
    assert_eq!(fixture.starts(), 1);
    fixture.tell("go", 1);
    let ready = crate::tests::settled_snapshot(&fixture.manager).await;
    assert_eq!(ready.server.state, "ready");
    assert!(ready.server.checked);
    let server = ready.server.id;
    fixture.restore(true).unwrap();
    assert_eq!(fixture.manager.snapshot().server.id, server);
    assert_eq!(fixture.starts(), 1);
    assert_eq!(fixture.recorded().await.len(), 1);
}

#[tokio::test]
async fn a_failed_restore_keeps_its_reason_and_is_tried_again_at_each_start_up_to_its_limit() {
    let mut fixture = Restoring::new();
    fixture.seed();
    for number in 1..=3 {
        fixture.tell("fail", number);
    }
    let starting = fixture.restore(true).unwrap();
    assert!(
        starting
            .operation
            .as_ref()
            .is_some_and(|operation| operation.kind == "restore"),
        "failed restore must be recorded as an operation"
    );
    let failed = crate::tests::settled_snapshot(&fixture.manager).await;
    let operation = failed.operation.as_ref().unwrap();
    assert_eq!(operation.state, "failed");
    let reason = operation.error.expect("failed restore reason");
    assert_eq!(failed.server.state, "failed");
    assert_eq!(failed.server.error, Some(reason));
    assert!(!failed.server.checked);
    assert_eq!(fixture.starts(), 1);
    assert!(
        fixture.intent_path().is_file(),
        "a restore that did not come up lost the press it restores"
    );
    let attempts = fixture.recorded().await;
    assert_eq!(attempts.len(), 1);
    assert_eq!(attempts[0]["result"], reason);
    fixture.clock.advance(HOUR);
    fixture.restore(true).unwrap();
    assert_eq!(
        fixture.starts(),
        1,
        "failed restore retried within the same start"
    );
    // The next window starts try the press's model again, each once.
    for start in 2..=3 {
        fixture.reopen().await;
        fixture.restore(true).unwrap();
        let failed = crate::tests::settled_snapshot(&fixture.manager).await;
        assert_eq!(
            fixture.starts(),
            start,
            "window start {start} did not try the press's model again"
        );
        assert_eq!(failed.server.state, "failed");
    }
    // A model that failed three starts running is left to the person.
    fixture.reopen().await;
    fixture.restore(true).unwrap();
    assert_eq!(
        fixture.starts(),
        3,
        "a fourth window start tried a model that failed three starts running"
    );
    let stopped = fixture.manager.snapshot();
    assert!(stopped.operation.is_none());
    assert_eq!(reason_of(&stopped), "model_restore_failed");
    let attempts = fixture.attempts();
    assert_eq!(
        attempts
            .last()
            .map(|attempt| (attempt["result"].clone(), attempt["reason"].clone())),
        Some((json!("skipped"), json!("model_restore_failed"))),
        "{attempts:?}"
    );
    // The person's press brings it back and starts the count again.
    fixture.press().await;
    assert_eq!(fixture.starts(), 4);
    assert!(
        fixture.intent_path().is_file(),
        "person apply did not reopen restoration"
    );
    fixture.reopen().await;
    fixture.restore(true).unwrap();
    let restored = crate::tests::settled_snapshot(&fixture.manager).await;
    assert_eq!(fixture.starts(), 5);
    assert_eq!(restored.server.state, "ready");
}

#[tokio::test]
async fn a_changed_port_restore_cannot_overwrite_newer_settings_without_a_notification() {
    for when in ["loading", "swap"] {
        let fixture = Restoring::new();
        fixture.seed();
        let recorded_port = u16::try_from(fixture.intent()["port"].as_u64().unwrap()).unwrap();
        let occupant = TcpListener::bind((Ipv4Addr::LOCALHOST, recorded_port)).unwrap();
        occupant.set_nonblocking(true).unwrap();
        fixture.tell("hold", 1);
        let starting = fixture.restore(true).unwrap();
        assert!(
            starting
                .operation
                .as_ref()
                .is_some_and(|operation| operation.kind == "restore"),
            "changed-port restore must expose its operation"
        );
        fixture.started(1).await;
        let owned_port = test_support::held_port(&fixture.manager, &fixture.models[0].id).unwrap();
        assert_ne!(owned_port, recorded_port);
        let newer = br#"{"preserved":"newer"}"#.to_vec();
        if when == "loading" {
            fs::write(&fixture.settings, &newer).unwrap();
        } else {
            *fixture.before_swap.lock().unwrap() = Some(newer.clone());
        }
        fixture.tell("go", 1);
        let refused = crate::tests::settled_snapshot(&fixture.manager).await;
        assert_eq!(refused.operation.as_ref().unwrap().state, "failed");
        assert_eq!(
            refused.operation.as_ref().unwrap().error,
            Some(Error::SettingsChanged.token())
        );
        assert_eq!(refused.server.state, "failed");
        assert!(refused.server.provider.is_none());
        assert!(!refused.server.checked);
        assert_eq!(fs::read(&fixture.settings).unwrap(), newer);
        assert!(!fixture.intent_path().exists());
        assert_eq!(fixture.writes.load(Ordering::SeqCst), 0);
        assert_eq!(
            fixture.swap_calls.load(Ordering::SeqCst),
            usize::from(when == "swap")
        );
        assert_eq!(test_support::held_listener_count(&fixture.manager), 0);
        let began = Instant::now();
        loop {
            if TcpListener::bind((Ipv4Addr::LOCALHOST, owned_port)).is_ok() {
                break;
            }
            assert!(
                began.elapsed() < SETTLE,
                "refused restore kept its listener"
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(
            fixture.recorded().await[0]["result"],
            Error::SettingsChanged.token()
        );
        assert!(
            matches!(occupant.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock)
        );
        fixture.restore(true).unwrap();
        assert_eq!(fixture.starts(), 1);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_runtime_exit_after_the_check_fails_restoration() {
    let fixture = Restoring::new();
    fixture.seed();
    let guarded_manager = fixture.manager.clone();
    let settings = fixture.settings.clone();
    let ending = fixture.control.path().join("end-1");
    fixture
        .manager
        .restore_guarded(
            true,
            (
                move |provider, expected, binding| save(&settings, provider, expected, binding),
                move |action: &mut dyn FnMut() -> crate::Result<()>| {
                    fs::write(&ending, b"code 3").map_err(|_| Error::Storage)?;
                    let began = Instant::now();
                    while guarded_manager.snapshot().server.state != "failed" {
                        assert!(began.elapsed() < SETTLE, "checked runtime did not end");
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    action()
                },
            ),
        )
        .unwrap();
    let failed = crate::tests::settled_snapshot(&fixture.manager).await;
    assert_eq!(
        failed.operation.as_ref().unwrap().state,
        "failed",
        "post-check runtime exit must fail restoration"
    );
    assert_eq!(failed.server.state, "failed");
    assert_eq!(
        failed.operation.as_ref().unwrap().error,
        Some(Error::RuntimeFailed.token())
    );
    assert!(
        fixture.intent_path().is_file(),
        "a restore whose runtime ended lost the press it restores"
    );
    assert_eq!(fixture.starts(), 1);
}

#[tokio::test]
async fn every_stop_this_side_makes_is_kept_with_what_made_it() {
    for by in ["press", "cancel", "switch", "app_exit"] {
        let fixture = Restoring::new();
        let (server, provider) = fixture.press().await;
        match by {
            "press" => {
                fixture.manager.stop(&server).unwrap();
            }
            "cancel" => {
                let operation = fixture.checking(&server).await;
                fixture.manager.cancel(&operation).unwrap();
                crate::tests::settled_snapshot(&fixture.manager).await;
            }
            "switch" => {
                let target = fixture.models[1].clone();
                let settings = fixture.settings.clone();
                let instance = fixture.manager.snapshot().instance_id;
                fixture
                    .manager
                    .switch_model(
                        &target.id,
                        &target.revision,
                        &server,
                        &provider.identity(),
                        &instance,
                        move |provider, expected, binding| {
                            save(&settings, provider, expected, binding)
                        },
                    )
                    .unwrap();
                let switched = crate::tests::settled_snapshot(&fixture.manager).await;
                assert_eq!(
                    switched.applied_model_id.as_deref(),
                    Some(target.id.as_str()),
                    "{:?}",
                    switched
                        .operation
                        .as_ref()
                        .and_then(|operation| operation.error)
                );
            }
            _ => fixture.manager.shutdown(),
        }
        let stops = fixture.kept("stop");
        assert_eq!(stops.len(), 1, "{by}: {stops:?}");
        assert_eq!(
            (
                &stops[0]["by"],
                &stops[0]["serverId"],
                &stops[0]["modelId"],
                &stops[0]["cause"]
            ),
            (
                &json!(by),
                &json!(server),
                &json!(fixture.models[0].id),
                &json!("server_stopped")
            ),
            "{by}"
        );
        assert!(
            stops[0]["atMs"].as_u64().is_some_and(|at| at > 0),
            "{by}: {stops:?}"
        );
    }
}

#[tokio::test]
async fn an_end_no_window_saw_is_kept_when_the_next_window_opens_the_store() {
    let fixture = Restoring::new();
    let (server, _) = fixture.press().await;
    let applied = fixture.kept("apply");
    assert_eq!(
        applied.len(),
        1,
        "the press's apply was not kept: {applied:?}"
    );
    assert_eq!(
        (&applied[0]["by"], &applied[0]["serverId"]),
        (&json!("press"), &json!(server))
    );
    // The window that runs it dies without its own end; the next window finds
    // the server it last saw come up with no end after it, and keeps that end
    // as one nobody saw.
    fixture.next_window().shutdown();
    let unseen = fixture.kept("unseen");
    assert_eq!(unseen.len(), 1, "{unseen:?}");
    assert_eq!(
        (&unseen[0]["sinceMs"], &unseen[0]["modelId"]),
        (&applied[0]["atMs"], &json!(fixture.models[0].id))
    );
    // The window after that finds the end kept and adds none.
    fixture.next_window().shutdown();
    assert_eq!(fixture.kept("unseen").len(), 1);
}

#[tokio::test]
async fn a_server_an_older_window_restored_and_never_saw_end_is_kept_as_unseen() {
    // The record as the window kept it on 10-21: its last line a ready
    // restore, and no end after it.
    let fixture = Restoring::new();
    let restored = json!({
        "atMs": 1_000_u64,
        "kind": "restore",
        "modelId": fixture.models[0].id,
        "generation": 0,
        "port": 9,
        "result": "ready",
    });
    fs::write(
        fixture.data.join("managed-judgment/server-exits.jsonl"),
        format!("{restored}\n"),
    )
    .unwrap();
    let opened = fixture.next_window();
    let unseen = fixture.kept("unseen");
    assert_eq!(unseen.len(), 1, "{unseen:?}");
    assert_eq!(unseen[0]["sinceMs"], 1000);
    assert!(
        unseen[0]["atMs"].as_u64().is_some_and(|at| at > 1000),
        "{unseen:?}"
    );
    let status = serde_json::to_value(opened.snapshot()).unwrap();
    let ended = &status["supervision"]["lastEnd"];
    assert_eq!(
        (&ended["kind"], &ended["sinceMs"]),
        (&json!("unseen"), &json!(1000)),
        "{status}"
    );
    opened.shutdown();
}

#[tokio::test]
async fn every_start_that_restores_nothing_keeps_why() {
    for (case, why) in [
        ("stopped", Some("no_intent")),
        ("off", Some("jev_off")),
        ("missing_runtime", Some("runtime_missing")),
        ("uncatalogued_model", Some("unknown_model")),
        ("unmanaged", None),
    ] {
        let mut fixture = Restoring::new();
        fixture.seed();
        match case {
            "stopped" => {
                let (server, _) = fixture.press().await;
                fixture.manager.stop(&server).unwrap();
                fixture.reopen().await;
            }
            "missing_runtime" => {
                fs::remove_dir_all(fixture.data.join("managed-judgment/runtime")).unwrap();
            }
            "uncatalogued_model" => {
                fixture.models.clear();
                fixture.reopen_with_runtime(true);
            }
            "unmanaged" => fs::write(&fixture.settings, b"{}").unwrap(),
            _ => {}
        }
        let starts = fixture.starts();
        fixture.restore(case != "off").unwrap();
        assert_eq!(fixture.starts(), starts, "{case} started a runtime");
        let kept = fixture.attempts();
        let said = kept
            .last()
            .map(|record| (record["result"].clone(), record["reason"].clone()));
        assert_eq!(
            said,
            why.map(|why| (json!("skipped"), json!(why))),
            "what the start kept for {case}: {kept:?}"
        );
    }
}
