use std::fs;
use std::net::{Ipv4Addr, TcpListener};
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

struct Restoring {
    _home: tempfile::TempDir,
    control: tempfile::TempDir,
    settings: PathBuf,
    data: PathBuf,
    manager: Arc<Manager>,
    models: Vec<Model>,
    starts: Arc<AtomicUsize>,
    writes: Arc<AtomicUsize>,
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
        let writer = self.transaction.clone();
        let guard = self.transaction.clone();
        self.manager.restore_guarded(
            allowed,
            (
                move |provider, expected, binding| {
                    let _writing = writer.try_lock().map_err(|_| Error::Busy)?;
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
        fs::read_to_string(self.data.join("managed-judgment/server-exits.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .filter(|record| record["kind"] == "restore")
            .collect()
    }
}

#[tokio::test]
async fn restore_gate_skips_without_changing_state_and_has_a_live_control() {
    for case in [
        "off",
        "unmanaged",
        "missing_model",
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
        let before = serde_json::to_value(fixture.manager.snapshot()).unwrap();
        fixture.restore(case != "off").unwrap();
        let after = crate::tests::settled_snapshot(&fixture.manager).await;
        assert_eq!(fixture.starts(), 0, "excluded restore started for {case}");
        assert!(
            after.operation.is_none(),
            "excluded restore reserved for {case}"
        );
        assert_eq!(
            serde_json::to_value(after).unwrap(),
            before,
            "excluded restore changed state for {case}"
        );
    }
    let fixture = Restoring::new();
    fixture.seed();
    fixture.restore(true).unwrap();
    let ready = crate::tests::settled_snapshot(&fixture.manager).await;
    assert_eq!(fixture.starts(), 1, "valid intent must start one runtime");
    assert_eq!(ready.server.state, "ready");
    assert!(ready.server.checked);
}
