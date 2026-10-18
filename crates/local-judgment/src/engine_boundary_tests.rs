use super::*;
use std::future::Future;
use std::task::{Context, Poll, Waker};

const CLOSED_INPUT: &str = r#"
import os,sys,time
root=sys.argv[1]
os.close(0)
with open(os.path.join(root,'ready'),'w') as marker:
 marker.write('ready')
while True:
 time.sleep(1)
"#;

const UNUSED_INPUT: &str = r#"
import os,sys,time
root=sys.argv[1]
sys.stdin.readline()
with open(os.path.join(root,'received'),'w') as marker:
 marker.write('received')
while True:
 time.sleep(1)
"#;

async fn fixture(program: &str, root: &Path) -> (Running, HttpState) {
    let engine = Engine::from_command(
        Command::new("python3")
            .args(["-I", "-S", "-B", "-u", "-c", program])
            .arg(root),
    );
    let failed: Arc<dyn Fn() + Send + Sync> = Arc::new(|| {});
    let running = serve_limited(
        &crate::tests::fixture_model(),
        engine.clone(),
        failed.clone(),
        Limits {
            request: Duration::from_secs(5),
            queue_wait: Duration::from_secs(5),
            waiting: 1,
        },
    )
    .await
    .unwrap();
    let state = HttpState {
        engine,
        provider: running.provider.clone(),
        admission: running.admission.clone(),
        failed,
        declared: None,
    };
    (running, state)
}

async fn until(what: &str, done: impl Fn() -> bool) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !done() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect(what);
}

#[tokio::test]
async fn an_io_failure_closes_admission_before_its_supervisor_is_polled_again() {
    let root = tempfile::tempdir().unwrap();
    let (_running, state) = fixture(CLOSED_INPUT, root.path()).await;
    until("the fake runtime closes its input", || {
        root.path().join("ready").exists()
    })
    .await;
    let ticket = state.admission.enter().unwrap();
    let turn = state.admission.turn.clone().acquire_owned().await.unwrap();
    let mut exchange = Box::pin(exchange_in_turn(
        state.clone(),
        turn,
        ticket,
        b"{}".to_vec(),
        None,
        None,
    ));
    {
        let _io = lock(&state.engine.io);
        let mut context = Context::from_waker(Waker::noop());
        assert!(matches!(
            exchange.as_mut().poll(&mut context),
            Poll::Pending
        ));
    }
    until(
        "the blocking exchange either closes or releases its turn",
        || state.admission.turn.is_closed() || state.admission.turn.available_permits() > 0,
    )
    .await;
    assert!(
        state.admission.turn.is_closed(),
        "a queued request can acquire the failed runtime before its supervisor runs"
    );
    assert_eq!(exchange.await, Err(Error::RuntimeFailed));
    assert!(!state.engine.alive());
}

#[tokio::test]
async fn a_prepared_frame_at_the_pipe_limit_is_refused_without_stopping_the_runtime() {
    let root = tempfile::tempdir().unwrap();
    let (running, state) = fixture(UNUSED_INPUT, root.path()).await;
    let mut request = json!({
        "model": state.provider.model,
        "state": "",
        "questions": {"only": {"type": "noul", "instructions": "fixture"}},
    });
    let padding = MAX_FRAME - serde_json::to_vec(&request).unwrap().len();
    request["state"] = json!("x".repeat(padding));
    let body = serde_json::to_vec(&request).unwrap();
    assert_eq!(body.len(), MAX_FRAME);
    request["model"] = json!(state.provider.identity());
    let prepared = state
        .provider
        .prepare(serde_json::to_vec(&request).unwrap())
        .unwrap();
    assert_eq!(prepared.body.len(), MAX_FRAME);
    let mut headers = HeaderMap::new();
    headers.insert(
        axum::http::header::HOST,
        state
            .provider
            .base_url
            .trim_start_matches("http://")
            .parse()
            .unwrap(),
    );
    let response = predict(State(state.clone()), headers, Bytes::from(body)).await;
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert!(state.engine.alive());
    assert_eq!(running.admitted(), 0);
    assert!(!root.path().join("received").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cancelled_deadline_owner_stops_inference_before_its_release_signal() {
    use std::os::unix::net::UnixListener;

    let root = tempfile::tempdir().unwrap();
    let listener = UnixListener::bind(root.path().join("release.sock")).unwrap();
    let program = r#"import json,socket,sys
from pathlib import Path
frame=json.loads(sys.stdin.readline())
request=frame.get('request',frame)
release=socket.socket(socket.AF_UNIX)
release.connect(str(Path(sys.argv[1])/'release.sock'))
release.recv(1)
print(json.dumps({'model':request['model'],'answers':{'only':{'type':'noul','noul':0.5}},'usage':{'input_tokens':1,'output_tokens':0}}),flush=True)
sys.stdin.read()
"#;
    let (_running, state) = fixture(program, root.path()).await;
    let mut headers = HeaderMap::new();
    headers.insert(
        axum::http::header::HOST,
        state
            .provider
            .base_url
            .trim_start_matches("http://")
            .parse()
            .unwrap(),
    );
    headers.insert(
        judgment_provider::DEADLINE_HEADER,
        zerocode_core::jev::BROWSER
            .apply_deadline_ms
            .unwrap()
            .to_string()
            .parse()
            .unwrap(),
    );
    let body = serde_json::to_vec(&json!({
        "model": state.provider.model,
        "state": "fixture",
        "questions": {"only": {"type": "noul", "instructions": "fixture"}},
    }))
    .unwrap();
    let called = state.clone();
    let owner =
        tokio::spawn(async move { predict(State(called), headers, Bytes::from(body)).await });
    let (mut release, _) = tokio::time::timeout(
        Duration::from_secs(5),
        tokio::task::spawn_blocking(move || listener.accept()),
    )
    .await
    .unwrap()
    .unwrap()
    .unwrap();
    owner.abort();
    assert!(owner.await.unwrap_err().is_cancelled());
    let stopped_before_release = !state.engine.alive();
    let closed_before_release = state.admission.turn.is_closed();
    let _ = release.write_all(b"release");
    state.engine.stop();
    assert!(
        stopped_before_release && closed_before_release,
        "a cancelled deadline owner left native inference running before its release signal"
    );
}

#[test]
fn withheld_lines_count_alike_whether_they_end_in_lf_or_crlf_or_blank_pieces_stand_between() {
    let said = ["judging: the harbor plan", "still judging", "done"];
    let lf = format!("{}\n", said.join("\n"));
    let crlf = format!("{}\r\n", said.join("\r\n"));
    // A vertical tab and an ideographic space are blank as str::trim judges,
    // though an ASCII scan would count them.
    let padded = format!("\n{}\n \t\n", said.join("\n   \n\u{b}\u{3000}\r\n\n"));
    let counts: Vec<u32> = [lf, crlf, padded]
        .into_iter()
        .map(|written| {
            let tail = Tail::default();
            tail.withhold();
            let (reader, mut writer) = std::io::pipe().unwrap();
            writer.write_all(written.as_bytes()).unwrap();
            drop(writer);
            drain(ChildStderr::from(std::os::fd::OwnedFd::from(reader)), &tail);
            let (kept, withheld) = tail.settled();
            assert_eq!(kept, Vec::<String>::new(), "{written:?} left text kept");
            withheld
        })
        .collect();
    assert_eq!(
        counts,
        [3, 3, 3],
        "the same three lines counted differently"
    );
}

/// The owned bridge over a stand-in for the laya runtime and the libraries
/// the bridge imports: an agent that loads its folder's
/// `rl_agent_config.json` and builds every question head through
/// `laya.common.build_head`, encoding the instruction and then each option as
/// laya does, one token per word, and cutting the instruction to what the
/// options leave of the config's head budget.
const STAND_IN_LAYA: &str = r#"import json,os,sys,types
class Words:
 mask_token,cls_token_id,sep_token_id,mask_token_id='[MASK]',1,2,3
 def __call__(self,text,add_special_tokens=True,truncation=False,max_length=None):
  ids=[10+len(word) for word in text.split()]
  return {'input_ids':ids[:max_length] if truncation else ids}
def build_head(tok,q,head_max_len,option_order=None):
 instruction=tok('%s question: %s'%(q['t'],q['ins']),add_special_tokens=False)['input_ids']
 options=[[tok.mask_token_id]+tok(' '+text,add_special_tokens=False,truncation=True,max_length=48)['input_ids'] for text in ('false','true')]
 ids=[tok.cls_token_id]+instruction[:head_max_len-sum(map(len,options))]+[tok.sep_token_id]
 markers=[]
 for option in options:
  markers.append(len(ids))
  ids.extend(option)
 return ids+[tok.sep_token_id],markers,{}
common=types.ModuleType('laya.common')
common.build_head=build_head
class Agent:
 def __init__(self,directory,device,compile):
  with open(os.path.join(directory,'rl_agent_config.json')) as config:
   self.cfg=json.load(config)
  self.tok=Words()
 @staticmethod
 def _to_internal(question):
  return {'t':question['type'],'ins':question['instructions']}
 def predict(self,state,questions):
  for question in questions.values():
   common.build_head(self.tok,self._to_internal(question),self.cfg['head_max_len'])
  usage={'input_tokens':1,'output_tokens':0,'truncated':False,'truncated_questions':[]}
  return {'answers':{name:{'type':'noul','noul':0.5} for name in questions},'usage':usage}
laya=types.ModuleType('laya')
laya.Agent,laya.common=Agent,common
torch=types.ModuleType('torch')
torch.set_num_threads=torch.set_num_interop_threads=lambda count:None
torch.jit=types.SimpleNamespace()
transformers=types.ModuleType('transformers')
transformers.AutoConfig=types.SimpleNamespace(from_pretrained=None)
transformers.AutoTokenizer=types.SimpleNamespace(from_pretrained=None)
sys.modules.update({'laya':laya,'laya.common':common,'torch':torch,'transformers':transformers})
exec(compile(sys.argv[1],'bridge','exec'),{'__name__':'__main__'})
"#;

/// A runtime that reads its start frame and answers with `ready.json`, the
/// start frame's model and runtime written where it says `<model>` and
/// `<runtime>`.
const READY_AS_TOLD: &str = r#"import json,sys
start=json.loads(sys.stdin.readline())
told=open('ready.json').read().replace('<model>',start.get('modelId','')).replace('<runtime>',start.get('runtime',''))
print(told.strip(),flush=True)
sys.stdin.read()
"#;

/// A runtime folder `Engine::spawn` starts the way it starts the managed
/// one: its executable runs `program` under the python3 these tests run and
/// hands it the bridge source the server passes; `id` tells its profile, and
/// so its fingerprint, apart from another one's.
fn runtime_folder(program: &str, id: &str) -> (tempfile::TempDir, RuntimeProfile) {
    use std::os::unix::fs::PermissionsExt;
    let folder = tempfile::tempdir().unwrap();
    let python = Command::new("python3")
        .args(["-I", "-c", "import sys;print(sys.executable)"])
        .output()
        .unwrap()
        .stdout;
    let python = String::from_utf8(python).unwrap();
    std::fs::write(folder.path().join("program.py"), program).unwrap();
    std::fs::create_dir(folder.path().join("model")).unwrap();
    let executable = folder.path().join("python");
    std::fs::write(
        &executable,
        format!(
            "#!/bin/sh\nexec '{}' -I -S -B -u program.py \"$4\"\n",
            python.trim()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
    let profile = serde_json::from_value(json!({
        "id": id,
        "pythonVersion": "3",
        "engine": "laya",
        "python": {"path": "python.tar.gz", "bytes": 1, "sha256": "0".repeat(64), "url": "https://example.com/python"},
        "executable": "python",
        "sitePackages": "site-packages",
        "wheels": [],
    }))
    .unwrap();
    (folder, profile)
}

/// Starts `model` in the runtime of `folder` as the manager does: spawned,
/// ready, then served.
async fn started(folder: &Path, profile: &RuntimeProfile, model: &Model) -> Running {
    let engine = Engine::spawn(
        folder,
        profile,
        Some((folder.join("model").as_path(), model)),
    )
    .unwrap();
    let process = engine.clone();
    tokio::time::timeout(
        START_TIMEOUT,
        tokio::task::spawn_blocking(move || process.ready()),
    )
    .await
    .expect("the runtime is ready in time")
    .unwrap()
    .unwrap();
    serve(model, engine, Arc::new(|| {})).await.unwrap()
}

/// The owned bridge over the stand-in laya runtime, serving `model` with
/// `config` as its `rl_agent_config.json`, from a runtime profile `id` names.
async fn bridged(
    model: &Model,
    id: &str,
    config: &Value,
) -> (tempfile::TempDir, RuntimeProfile, Running) {
    let (folder, profile) = runtime_folder(STAND_IN_LAYA, id);
    std::fs::write(
        folder.path().join("model/rl_agent_config.json"),
        config.to_string(),
    )
    .unwrap();
    let running = started(folder.path(), &profile, model).await;
    (folder, profile, running)
}

/// A runtime that says it is ready with `ready`, serving `model`.
async fn told(model: &Model, ready: &Value) -> (tempfile::TempDir, RuntimeProfile, Running) {
    let (folder, profile) = runtime_folder(READY_AS_TOLD, "fixture-runtime");
    std::fs::write(folder.path().join("ready.json"), ready.to_string()).unwrap();
    let running = started(folder.path(), &profile, model).await;
    (folder, profile, running)
}

fn client() -> reqwest::Client {
    reqwest::Client::builder().no_proxy().build().unwrap()
}

async fn listed(running: &Running) -> Value {
    client()
        .get(running.provider.api_url("models"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_bridge_refuses_a_head_its_runtime_cuts_and_answers_one_that_fits() {
    let model = crate::tests::fixture_model();
    let request = json!({
        "model": model.id,
        "state": "fixture",
        "questions": {
            "goal": {"type": "noul", "instructions": "word ".repeat(260)},
            "other": {"type": "noul", "instructions": "Is the page ready?"},
        },
    });
    for (config, cut) in [
        (json!({"max_len": 1024, "head_max_len": 256}), true),
        (json!({"max_len": 2048, "head_max_len": 300}), false),
    ] {
        let (_folder, _profile, running) = bridged(&model, "fixture-runtime", &config).await;
        let response = client()
            .post(running.provider.request_url())
            .json(&request)
            .send()
            .await
            .unwrap();
        if cut {
            assert_eq!(
                response.status(),
                reqwest::StatusCode::BAD_GATEWAY,
                "a head the runtime cut under {config} was answered"
            );
            let body: Value = response.json().await.unwrap();
            assert_eq!(
                body,
                json!({"error": "native_protocol_failed", "refusal": "provider_context_truncated"})
            );
        } else {
            assert_eq!(
                response.status(),
                reqwest::StatusCode::OK,
                "a head that fits under {config} was refused"
            );
        }
        assert!(running.engine.alive());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn models_carry_the_limits_each_runtime_declared_from_the_config_it_loaded() {
    let mut next = crate::tests::fixture_model();
    next.id = "fixture-model-next".to_owned();
    let mut generations = Vec::new();
    for (model, id, max_len, head_max_len) in [
        (crate::tests::fixture_model(), "fixture-runtime", 1024, 256),
        (next, "fixture-runtime-next", 2048, 300),
        (crate::tests::fixture_model(), "fixture-runtime", 1024, 256),
    ] {
        let config = json!({"max_len": max_len, "head_max_len": head_max_len});
        let (_folder, profile, running) = bridged(&model, id, &config).await;
        assert_eq!(
            listed(&running).await["data"],
            json!([{
                "id": model.id,
                "limits": {
                    "model": model.id,
                    "runtime": profile.fingerprint().unwrap(),
                    "generation": running.id,
                    "tokens": {"max_len": max_len, "head_max_len": head_max_len},
                },
            }]),
            "the server declared other limits than the runtime it started loaded"
        );
        generations.push(running.id.clone());
    }
    generations.sort();
    generations.dedup();
    assert_eq!(generations.len(), 3, "two starts declared one generation");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_ready_frame_declares_limits_only_for_its_own_model_and_runtime() {
    let model = crate::tests::fixture_model();
    let id = model.id.as_str();
    let named =
        |tokens: Value| json!({"model": "<model>", "runtime": "<runtime>", "tokens": tokens});
    for tokens in [
        json!({"max_len": 1024, "head_max_len": 256}),
        json!({"max_len": 1024}),
    ] {
        let ready = json!({"ready": true, "limits": named(tokens.clone())});
        let (_folder, profile, running) = told(&model, &ready).await;
        assert_eq!(
            listed(&running).await["data"],
            json!([{
                "id": id,
                "limits": {
                    "model": id,
                    "runtime": profile.fingerprint().unwrap(),
                    "generation": running.id,
                    "tokens": tokens,
                },
            }]),
            "a usable declaration for the served model and runtime was dropped"
        );
    }
    for limits in [
        None,
        Some(named(json!({"max_len": 0, "head_max_len": 256}))),
        Some(named(json!({"max_len": 1024, "head_max_len": 0}))),
        Some(named(json!({"max_len": -1024}))),
        Some(named(json!({"max_len": true}))),
        Some(named(json!({"max_len": "1024"}))),
        Some(named(json!({"max_len": 1024.5}))),
        Some(named(json!({"max_len": 1024, "head_max_len": null}))),
        Some(named(json!({"head_max_len": 256}))),
        Some(named(json!(1024))),
        Some(json!({"model": "", "runtime": "<runtime>", "tokens": {"max_len": 1024}})),
        Some(json!({"model": "<model>", "runtime": "", "tokens": {"max_len": 1024}})),
        Some(
            json!({"model": "another-model", "runtime": "<runtime>", "tokens": {"max_len": 1024}}),
        ),
        Some(
            json!({"model": "<model>", "runtime": "another-runtime", "tokens": {"max_len": 1024}}),
        ),
        Some(json!({"model": "<model>", "tokens": {"max_len": 1024}})),
        Some(json!({"runtime": "<runtime>", "tokens": {"max_len": 1024}})),
        Some(json!(1024)),
    ] {
        let mut ready = json!({"ready": true});
        if let Some(limits) = limits {
            ready["limits"] = limits;
        }
        let (_folder, _profile, running) = told(&model, &ready).await;
        assert_eq!(
            listed(&running).await["data"],
            json!([{"id": id}]),
            "{ready} declared a limit"
        );
    }
}

/// Names the managed-judgment folder whose manifest-pinned laya runtime and
/// model the contract test below runs. No other test reads it.
const PINNED_ROOT: &str = "LOCAL_JUDGMENT_PINNED_ROOT";

fn compiled_goal_head_shapes() -> Vec<Value> {
    use zerocode_core::screen_action::{
        ActionLook, Beside, Errand, MAX_ACTION_CANDIDATES, MORE_CONTROLS, Navigation, Where,
        ask_with, mark_of,
    };

    let navigation_sets: [(&str, &[Navigation]); 5] = [
        ("none", &[]),
        ("wait", &[Navigation::Wait]),
        ("page", &Navigation::PAGE),
        ("desktop", &Navigation::DESKTOP),
        ("all", &Navigation::ALL),
    ];
    let mut shapes = Vec::new();
    for (legend, label) in [
        ("short", "Open workspace"),
        (
            "long",
            "Open the project workspace and review the latest completed test results, including the detailed status for each selected task",
        ),
    ] {
        let items: Vec<Value> = (1..=MAX_ACTION_CANDIDATES + 1)
            .map(|mark| {
                json!({
                    "mark": mark, "role": "button", "label": format!("{label} {mark}"),
                    "centerX": 100 + mark * 20, "centerY": 200 + mark * 30,
                })
            })
            .collect();
        let look = ActionLook {
            goal: "Open the task workspace, then confirm test completion.",
            errand: Errand::Goal,
            at: Where::Page {
                host: "example.test",
                path: "/start",
            },
            tried: &[],
            items: &items,
            pressed: &[],
            shows: &[],
        };
        let crowded = ask_with(
            &look,
            &Beside {
                navigation: &Navigation::ALL,
                ..Beside::default()
            },
        )
        .unwrap();
        for (navigation_name, navigation) in navigation_sets {
            for count in 0..=MAX_ACTION_CANDIDATES {
                for more in [false, true] {
                    let available = count.max(1);
                    let mut asked = ask_with(
                        &ActionLook {
                            items: &items[..available],
                            ..look
                        },
                        &Beside {
                            navigation,
                            ..Beside::default()
                        },
                    )
                    .unwrap();
                    if more {
                        asked.questions["action"]["instructions"] =
                            crowded.questions["action"]["instructions"].clone();
                        asked.questions["action"]["criteria"][MORE_CONTROLS] =
                            crowded.questions["action"]["criteria"][MORE_CONTROLS].clone();
                    }
                    asked.questions["action"]["criteria"]
                        .as_object_mut()
                        .unwrap()
                        .retain(|option, _| mark_of(option).is_none_or(|mark| mark <= count));
                    let candidates = asked.state["candidates"].as_array_mut().unwrap();
                    candidates.truncate(count);
                    shapes.push(json!({
                        "shape": {"legend": legend, "navigation": navigation_name, "candidates": count, "more": more},
                        "request": {"state": asked.state, "questions": asked.questions},
                    }));
                }
            }
        }
    }
    shapes
}

fn compiled_recorded_goal_heads() -> Vec<Value> {
    use zerocode_core::screen_action::{ActionLook, Beside, Errand, Navigation, Where, ask_with};

    let Some(root) = std::env::var_os("LOCAL_JUDGMENT_RECORDED_TRIAL") else {
        return Vec::new();
    };
    let root = Path::new(&root);
    let fixture: Value =
        serde_json::from_slice(&std::fs::read(root.join("fixture.json")).unwrap()).unwrap();
    let marks: Value =
        serde_json::from_slice(&std::fs::read(root.join("marks.txt")).unwrap()).unwrap();
    let address = url::Url::parse(fixture["url"].as_str().unwrap()).unwrap();
    [false, true]
        .into_iter()
        .map(|navigation| {
            let asked = ask_with(
                &ActionLook {
                    goal: fixture["goal"].as_str().unwrap(),
                    errand: Errand::Goal,
                    at: Where::Page {
                        host: address.host_str().unwrap(),
                        path: address.path(),
                    },
                    tried: &[],
                    items: marks["items"].as_array().unwrap(),
                    pressed: &[],
                    shows: &[],
                },
                &Beside {
                    navigation: if navigation { &Navigation::PAGE } else { &[] },
                    ..Beside::default()
                },
            )
            .unwrap();
            json!({
                "shape": {"legend": "recorded", "navigation": if navigation { "page" } else { "none" },
                          "candidates": asked.marks().len(), "more": false},
                "request": {"state": asked.state, "questions": asked.questions},
            })
        })
        .collect()
}

#[test]
fn compiled_goal_shapes_fit_a_loaded_stand_in_budget() {
    let payload = serde_json::to_vec(&compiled_goal_head_shapes()).unwrap();
    let mut process = Command::new("python3")
        .args([
            "-I",
            "-S",
            "-B",
            "-c",
            include_str!("../tests/bridge_boundary.py"),
        ])
        .arg(serde_json::to_string(include_str!("bridge.py")).unwrap())
        .arg("--compiled")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = process.stdin.take().unwrap();
    let writing = std::thread::spawn(move || input.write_all(&payload));
    let output = process.wait_with_output().unwrap();
    writing.join().unwrap().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[ignore = "contract: runs recorded compiled actions with the manifest-pinned model in PINNED_ROOT"]
fn compiled_recorded_actions_run_on_the_manifest_pinned_model() {
    let root = std::env::var_os(PINNED_ROOT)
        .unwrap_or_else(|| panic!("did not run: {PINNED_ROOT} names no folder"));
    let profile = crate::catalog::runtime().unwrap().unwrap();
    let model = crate::catalog::catalog()
        .unwrap()
        .models
        .into_iter()
        .find(|model| model.engine == EngineKind::Laya)
        .unwrap();
    let runtime = Path::new(&root).join("runtime").join(&profile.id);
    let folder = Path::new(&root).join("models").join(&model.id);
    for (place, fingerprint) in [
        (&runtime, profile.fingerprint().unwrap()),
        (&folder, model.fingerprint().unwrap()),
    ] {
        let receipt: Value =
            serde_json::from_slice(&std::fs::read(place.join("receipt.json")).unwrap()).unwrap();
        assert_eq!(
            receipt["fingerprint"], fingerprint,
            "did not run: the clone is not manifest-pinned"
        );
    }
    let mut shapes = Vec::new();
    for mut fixture in compiled_recorded_goal_heads() {
        fixture["shape"]["questions"] = json!("all");
        shapes.push(fixture.clone());
        fixture["shape"]["questions"] = json!("action");
        fixture["request"]["questions"]
            .as_object_mut()
            .unwrap()
            .retain(|name, _| name == "action");
        shapes.push(fixture);
    }
    assert_eq!(
        shapes.len(),
        4,
        "did not run: no recorded trial was compiled"
    );
    if std::env::var("LOCAL_JUDGMENT_INFERENCE_EXTREMES").is_ok_and(|value| value == "1") {
        shapes.extend(compiled_goal_head_shapes().into_iter().filter(|fixture| {
            fixture["shape"]["navigation"] == "all"
                && fixture["shape"]["candidates"] == zerocode_core::jev::SCREEN_CANDIDATE_CAP
                && fixture["shape"]["more"] == true
        }));
        assert_eq!(shapes.len(), 6);
    }
    let packages: serde_json::Map<String, Value> = profile
        .wheels
        .iter()
        .filter_map(|wheel| Some((wheel.package.clone()?, json!(wheel.version.as_ref()?))))
        .collect();
    let initial = json!({
        "engine": "laya", "modelDirectory": folder, "modelId": model.id,
        "runtime": profile.fingerprint().unwrap(), "packages": packages,
    });
    let source = std::env::var_os("LOCAL_JUDGMENT_BRIDGE_SOURCE")
        .map(|path| std::fs::read_to_string(path).unwrap())
        .unwrap_or_else(|| include_str!("bridge.py").to_string());
    let payload = serde_json::to_vec(&shapes).unwrap();
    let cache = runtime.join("cache");
    std::fs::create_dir_all(&cache).unwrap();
    let mut process = Command::new(child(&runtime, &profile.executable).unwrap())
        .args(["-I", "-B", "-c", include_str!("../tests/infer_compiled.py")])
        .arg(source)
        .arg(initial.to_string())
        .env_clear()
        .env("HOME", &cache)
        .env("HF_HOME", &cache)
        .env("TMPDIR", &cache)
        .env("HF_HUB_OFFLINE", "1")
        .env("TRANSFORMERS_OFFLINE", "1")
        .env("OMP_NUM_THREADS", "1")
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env(
            "LOCAL_JUDGMENT_BUILD_RUN",
            std::env::var_os("LOCAL_JUDGMENT_BUILD_RUN").unwrap_or_default(),
        )
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = process.stdin.take().unwrap();
    let writing = std::thread::spawn(move || input.write_all(&payload));
    let output = process.wait_with_output().unwrap();
    writing.join().unwrap().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let measured: Value = serde_json::from_slice(&output.stdout).unwrap();
    if let Some(path) = std::env::var_os("LOCAL_JUDGMENT_INFERENCE_REPORT") {
        std::fs::write(path, &output.stdout).unwrap();
    }
    let provider = Provider {
        kind: ProviderKind::Local,
        protocol: Protocol::SystemOne,
        base_url: "http://127.0.0.1".to_string(),
        model: model.id,
        requires_auth: false,
    };
    let expected_refusal =
        std::env::var("LOCAL_JUDGMENT_EXPECT_REFUSAL").is_ok_and(|value| value == "1");
    let rows = measured["rows"].as_array().unwrap();
    assert_eq!(rows.len(), shapes.len() * 3);
    for (index, row) in rows.iter().enumerate() {
        let mut request = shapes[index / 3]["request"].clone();
        request["model"] = json!(provider.identity());
        let prepared = provider
            .prepare(serde_json::to_vec(&request).unwrap())
            .unwrap();
        let decoded = provider.decode(&prepared, serde_json::to_vec(&row["result"]).unwrap());
        if expected_refusal && row["shape"]["navigation"] != "none" {
            assert_eq!(decoded, Err(judgment_provider::Refusal::Truncated), "{row}");
        } else {
            let decoded: Value = serde_json::from_str(&decoded.unwrap()).unwrap();
            assert_eq!(decoded["usage"], row["result"]["usage"]);
            assert!(
                row["result"]["usage"]["truncated_questions"]
                    .as_array()
                    .is_none_or(Vec::is_empty)
            );
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "contract: checks a client deadline against the manifest-pinned model in PINNED_ROOT"]
async fn a_screen_client_deadline_does_not_leave_inference_ahead_of_the_next_request() {
    let root = std::env::var_os(PINNED_ROOT)
        .unwrap_or_else(|| panic!("did not run: {PINNED_ROOT} names no folder"));
    let profile = crate::catalog::runtime().unwrap().unwrap();
    let model = crate::catalog::catalog()
        .unwrap()
        .models
        .into_iter()
        .find(|model| model.engine == EngineKind::Laya)
        .unwrap();
    let runtime = Path::new(&root).join("runtime").join(&profile.id);
    let folder = Path::new(&root).join("models").join(&model.id);
    let engine = Engine::spawn(&runtime, &profile, Some((folder.as_path(), &model))).unwrap();
    let process = engine.clone();
    tokio::time::timeout(
        START_TIMEOUT,
        tokio::task::spawn_blocking(move || process.ready()),
    )
    .await
    .unwrap()
    .unwrap()
    .unwrap();
    let running = serve(&model, engine.clone(), Arc::new(|| {}))
        .await
        .unwrap();
    let mut crowded = compiled_goal_head_shapes()
        .into_iter()
        .find(|fixture| {
            fixture["shape"]["legend"] == "long"
                && fixture["shape"]["navigation"] == "all"
                && fixture["shape"]["candidates"] == zerocode_core::jev::SCREEN_CANDIDATE_CAP
                && fixture["shape"]["more"] == true
        })
        .unwrap()["request"]
        .clone();
    crowded["model"] = json!(model.id);
    let deadline = Duration::from_millis(zerocode_core::jev::SCREEN_APPLY_DEADLINE_MS);
    let context = || {
        let mut jobs = std::env::var_os("LOCAL_JUDGMENT_BUILD_RUN")
            .and_then(|path| std::fs::read_dir(path).ok())
            .into_iter()
            .flatten()
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        jobs.sort();
        json!({
            "at_ms": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis(),
            "running_jobs": jobs,
        })
    };
    let first_context = context();
    let began = Instant::now();
    let first = client()
        .post(running.provider.request_url())
        .timeout(deadline)
        .header(
            judgment_provider::DEADLINE_HEADER,
            deadline.as_millis().to_string(),
        )
        .json(&crowded)
        .send()
        .await;
    let first_elapsed = began.elapsed();
    let first_timed_out = first.as_ref().is_err_and(reqwest::Error::is_timeout);
    let first_status = first
        .as_ref()
        .ok()
        .map(|response| response.status().as_u16());
    let first_body = match first {
        Ok(response) => response.json::<Value>().await.unwrap(),
        Err(_) => Value::Null,
    };
    let admitted_after_first = running.admitted();
    let mut following = compiled_recorded_goal_heads()
        .into_iter()
        .find(|fixture| fixture["shape"]["navigation"] == "none")
        .unwrap()["request"]
        .clone();
    following["model"] = json!(model.id);
    let following_context = context();
    let began = Instant::now();
    let response = client()
        .post(running.provider.request_url())
        .timeout(REQUEST_TIMEOUT)
        .header(
            judgment_provider::DEADLINE_HEADER,
            deadline.as_millis().to_string(),
        )
        .json(&following)
        .send()
        .await
        .unwrap();
    let following_elapsed = began.elapsed();
    let following_status = response.status();
    let following_body = response.json::<Value>().await.unwrap();
    let report = json!({
        "first_timed_out": first_timed_out,
        "first_context": first_context,
        "first_status": first_status,
        "first_body": first_body,
        "first_elapsed_ms": first_elapsed.as_secs_f64() * 1000.0,
        "admitted_after_first": admitted_after_first,
        "following_elapsed_ms": following_elapsed.as_secs_f64() * 1000.0,
        "following_context": following_context,
        "following_status": following_status.as_u16(),
        "following_body": following_body,
        "engine_alive_after_following": engine.alive(),
    });
    if let Some(path) = std::env::var_os("LOCAL_JUDGMENT_CANCELLATION_REPORT") {
        std::fs::write(path, serde_json::to_vec(&report).unwrap()).unwrap();
    }
    assert_eq!(following_status, reqwest::StatusCode::OK, "{report}");
    assert!(
        !first_timed_out || admitted_after_first == 0,
        "a client deadline left native inference ahead of the next request: {report}"
    );
    assert!(
        following_elapsed <= deadline,
        "a client deadline left native inference ahead of the next request: {report}"
    );
}

#[test]
#[ignore = "contract: measures compiled questions with the manifest-pinned tokenizer in PINNED_ROOT"]
fn compiled_goal_heads_fit_the_manifest_pinned_tokenizer() {
    let root = std::env::var_os(PINNED_ROOT)
        .unwrap_or_else(|| panic!("did not run: {PINNED_ROOT} names no folder"));
    let profile = crate::catalog::runtime().unwrap().unwrap();
    let model = crate::catalog::catalog()
        .unwrap()
        .models
        .into_iter()
        .find(|model| model.engine == EngineKind::Laya)
        .unwrap();
    let runtime = Path::new(&root).join("runtime").join(&profile.id);
    let folder = Path::new(&root).join("models").join(&model.id);
    for (place, fingerprint) in [
        (&runtime, profile.fingerprint().unwrap()),
        (&folder, model.fingerprint().unwrap()),
    ] {
        let receipt: Value =
            serde_json::from_slice(&std::fs::read(place.join("receipt.json")).unwrap()).unwrap();
        assert_eq!(
            receipt["fingerprint"], fingerprint,
            "did not run: the clone is not manifest-pinned"
        );
    }
    let mut shapes = compiled_goal_head_shapes();
    assert_eq!(
        shapes.len(),
        5 * (zerocode_core::jev::SCREEN_CANDIDATE_CAP + 1) * 2 * 2
    );
    shapes.extend(compiled_recorded_goal_heads());
    let payload = serde_json::to_vec(&shapes).unwrap();
    if let Some(path) = std::env::var_os("LOCAL_JUDGMENT_COMPILED_REQUESTS") {
        std::fs::write(path, &payload).unwrap();
    }
    let cache = runtime.join("cache");
    std::fs::create_dir_all(&cache).unwrap();
    let mut process = Command::new(child(&runtime, &profile.executable).unwrap())
        .args(["-I", "-B", "-c", include_str!("../tests/compiled_heads.py")])
        .arg(include_str!("bridge.py"))
        .arg(&folder)
        .env_clear()
        .env("HOME", &cache)
        .env("HF_HOME", &cache)
        .env("TMPDIR", &cache)
        .env("HF_HUB_OFFLINE", "1")
        .env("TRANSFORMERS_OFFLINE", "1")
        .env("OMP_NUM_THREADS", "1")
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = process.stdin.take().unwrap();
    let writing = std::thread::spawn(move || input.write_all(&payload));
    let output = process.wait_with_output().unwrap();
    writing.join().unwrap().unwrap();
    assert!(
        output.status.success(),
        "the loaded tokenizer did not measure the compiled requests: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    if let Some(path) = std::env::var_os("LOCAL_JUDGMENT_HEAD_REPORT") {
        std::fs::write(path, &output.stdout).unwrap();
    }
    let measured: Value = serde_json::from_slice(&output.stdout).unwrap();
    let rows = measured["rows"].as_array().unwrap();
    assert_eq!(rows.len(), shapes.len());
    let failed: Vec<&Value> = rows.iter().filter(|row| row["pass"] != true).collect();
    assert!(
        failed.is_empty(),
        "{} compiled request shapes submit cut heads or lose options against the loaded head budget of {} tokens; first: {}",
        failed.len(),
        measured["head_max_len"],
        failed.first().unwrap_or(&&Value::Null)
    );
}

/// Runs in the pinned runtime with the loaded agent's tokenizer and head
/// budget. It grows a goal one sentence at a time until the instruction
/// carrying it, the first text the runtime's own `build_head` encodes, is
/// longer than the budget in whole tokens; each sentence adds a token, so a
/// goal still inside the budget after one sentence more than the budget has
/// tokens means the test did not run. It measures that question and the same
/// question without the goal with and without the bridge's measure around
/// `build_head`, exits with a failure if the two differ, and prints both
/// questions, the goal's size and whether each head was whole. The questions'
/// keys are sorted, so the server hands them to the bridge in that order.
const PINNED_HEADS: &str = r#"import ast,json,sys
source=ast.parse(sys.argv[1])
kept=[node for node in source.body if isinstance(node,(ast.Import,ast.ImportFrom,ast.FunctionDef,ast.ClassDef))]
bridge={}
exec(compile(ast.Module(body=kept,type_ignores=[]),'bridge','exec'),bridge)
from laya import Agent,common
agent=Agent(sys.argv[2],device='cpu',compile=False)
budget=agent.cfg['head_max_len']
class Counted:
 def __init__(self,tok):
  self.tok,self.counts=tok,[]
 def __getattr__(self,name):
  return getattr(self.tok,name)
 def __call__(self,text,**kwargs):
  uncut={key:value for key,value in kwargs.items() if key not in('truncation','max_length')}
  self.counts.append(len(self.tok(text,**uncut)['input_ids']))
  return self.tok(text,**kwargs)
def sorted_keys(question):
 return json.loads(json.dumps(question,sort_keys=True))
asked={'type':'noul','instructions':{'question':'Is the goal done?'}}
def with_goal(goal):
 return sorted_keys(dict(asked,instructions=dict(asked['instructions'],callerContext={'goal':goal})))
whole=sorted_keys(asked)
sentence='작업 공간을 열고 결과를 확인한 다음 완료 상태를 확인하세요.'
for sentences in range(1,budget+2):
 cut=with_goal(' '.join([sentence]*sentences))
 counted=Counted(agent.tok)
 common.build_head(counted,Agent._to_internal(cut),budget)
 if counted.counts[0]>budget:
  break
else:
 sys.exit('did not run: a goal of %d sentences never made the instruction longer than the loaded head budget of %d tokens'%(sentences,budget))
verdicts=[]
for question in (cut,whole):
 internal=Agent._to_internal(question)
 plain=common.build_head(agent.tok,internal,budget)
 head,measured=bridge['measured_head'](common.build_head,agent.tok,internal,budget)
 if head!=plain:
  sys.exit('the measure changed what build_head computes')
 verdicts.append(measured)
print(json.dumps({'sentences':sentences,'tokens':counted.counts[0],'head_max_len':budget,'questions':[cut,whole],'whole':verdicts}))
"#;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "contract: runs the manifest-pinned laya runtime and model in the folder PINNED_ROOT names"]
async fn the_manifest_pinned_runtime_refuses_a_cut_head_and_declares_the_config_it_loaded() {
    let root = std::env::var_os(PINNED_ROOT)
        .unwrap_or_else(|| panic!("did not run: {PINNED_ROOT} names no folder"));
    let profile = crate::catalog::runtime()
        .unwrap()
        .unwrap_or_else(|| panic!("did not run: the manifest pins no runtime for this platform"));
    let model = crate::catalog::catalog()
        .unwrap()
        .models
        .into_iter()
        .find(|model| model.engine == EngineKind::Laya)
        .unwrap_or_else(|| panic!("did not run: the manifest pins no laya model"));
    let runtime = Path::new(&root).join("runtime").join(&profile.id);
    let folder = Path::new(&root).join("models").join(&model.id);
    for (place, fingerprint) in [
        (&runtime, profile.fingerprint().unwrap()),
        (&folder, model.fingerprint().unwrap()),
    ] {
        let receipt = std::fs::read(place.join("receipt.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
        assert!(
            receipt
                .as_ref()
                .and_then(|receipt| receipt["fingerprint"].as_str())
                == Some(fingerprint.as_str()),
            "did not run: {} does not hold the manifest-pinned build",
            place.display()
        );
    }
    // Both questions come from the pinned runtime: a goal grown one sentence
    // at a time until the loaded tokenizer makes the instruction carrying it
    // longer than the loaded head budget, and the same question without it.
    let home = tempfile::tempdir().unwrap();
    let heads = Command::new(child(&runtime, &profile.executable).unwrap())
        .args(["-I", "-B", "-c", PINNED_HEADS])
        .arg(include_str!("bridge.py"))
        .arg(&folder)
        .env_clear()
        .env("HOME", home.path())
        .env("HF_HUB_OFFLINE", "1")
        .env("TRANSFORMERS_OFFLINE", "1")
        .env("TOKENIZERS_PARALLELISM", "false")
        .output()
        .unwrap();
    assert!(
        heads.status.success(),
        "the pinned runtime's heads were not measured: {}",
        String::from_utf8_lossy(&heads.stderr)
            .lines()
            .last()
            .unwrap_or_default()
    );
    let measured: Value = serde_json::from_slice(&heads.stdout).unwrap();
    assert_eq!(
        measured["whole"],
        json!([false, true]),
        "the pinned runtime's heads were measured otherwise: a goal of {} sentences made the instruction {} tokens against a head budget of {}",
        measured["sentences"],
        measured["tokens"],
        measured["head_max_len"]
    );
    let (cut, whole) = (&measured["questions"][0], &measured["questions"][1]);
    let engine = Engine::spawn(&runtime, &profile, Some((folder.as_path(), &model))).unwrap();
    let process = engine.clone();
    tokio::time::timeout(
        START_TIMEOUT,
        tokio::task::spawn_blocking(move || process.ready()),
    )
    .await
    .expect("the pinned runtime is ready in time")
    .unwrap()
    .unwrap();
    let running = serve(&model, engine, Arc::new(|| {})).await.unwrap();
    for (question, status) in [
        (cut, reqwest::StatusCode::BAD_GATEWAY),
        (whole, reqwest::StatusCode::OK),
    ] {
        let response = client()
            .post(running.provider.request_url())
            .json(&json!({"model": model.id, "state": "fixture", "questions": {"goal": question}}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), status);
        if status == reqwest::StatusCode::BAD_GATEWAY {
            let body: Value = response.json().await.unwrap();
            assert_eq!(body["refusal"], "provider_context_truncated");
        }
    }
    let config: Value =
        serde_json::from_slice(&std::fs::read(folder.join("rl_agent_config.json")).unwrap())
            .unwrap();
    assert_eq!(
        listed(&running).await["data"][0]["limits"],
        json!({
            "model": model.id,
            "runtime": profile.fingerprint().unwrap(),
            "generation": running.id,
            "tokens": {"max_len": config["max_len"], "head_max_len": config["head_max_len"]},
        }),
        "the pinned runtime declared other limits than its model folder holds"
    );
}

const ECHO_DEADLINE_INPUT: &str = r#"import json,sys
from pathlib import Path
for line in sys.stdin:
 frame=json.loads(line)
 Path(sys.argv[1],'received.json').write_text(json.dumps(frame))
 request=frame.get('request',frame)
 answers={name:{'type':'noul','noul':0.5} for name in request['questions']}
 print(json.dumps({'model':request['model'],'answers':answers,'usage':{'input_tokens':1,'output_tokens':0}}),flush=True)
"#;

#[tokio::test]
async fn an_invalid_or_ambiguous_deadline_header_reaches_no_native_request() {
    let invalid = vec![
        vec![String::new()],
        vec!["not-a-duration".to_string()],
        vec!["-1".to_string()],
        vec!["+1".to_string()],
        vec!["1,2".to_string()],
        vec![format!("{}0", u64::MAX)],
        vec!["1".to_string(), "2".to_string()],
    ];
    for values in invalid {
        let root = tempfile::tempdir().unwrap();
        let (_running, state) = fixture(ECHO_DEADLINE_INPUT, root.path()).await;
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::HOST,
            state
                .provider
                .base_url
                .trim_start_matches("http://")
                .parse()
                .unwrap(),
        );
        for value in &values {
            headers.append(judgment_provider::DEADLINE_HEADER, value.parse().unwrap());
        }
        let request = json!({
            "model": state.provider.model, "state": "fixture",
            "questions": {"only": {"type": "noul", "instructions": "fixture"}},
        });
        let response = predict(
            State(state.clone()),
            headers,
            Bytes::from(serde_json::to_vec(&request).unwrap()),
        )
        .await;
        assert_eq!(
            response.status(),
            StatusCode::BAD_REQUEST,
            "an invalid deadline header reached native inference: {values:?}"
        );
        assert!(state.engine.alive());
        assert!(!root.path().join("received.json").exists());
    }
}

#[tokio::test]
async fn a_request_without_a_deadline_header_keeps_the_legacy_native_frame() {
    let root = tempfile::tempdir().unwrap();
    let (_running, state) = fixture(ECHO_DEADLINE_INPUT, root.path()).await;
    let mut headers = HeaderMap::new();
    headers.insert(
        axum::http::header::HOST,
        state
            .provider
            .base_url
            .trim_start_matches("http://")
            .parse()
            .unwrap(),
    );
    let request = json!({
        "model": state.provider.model, "state": "fixture",
        "questions": {"only": {"type": "noul", "instructions": "fixture"}},
    });
    let response = predict(
        State(state.clone()),
        headers,
        Bytes::from(serde_json::to_vec(&request).unwrap()),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let native: Value =
        serde_json::from_slice(&std::fs::read(root.path().join("received.json")).unwrap()).unwrap();
    assert_eq!(native, request);
    assert!(state.engine.alive());
}

async fn deadline_runtime(
    root: &Path,
    config: &Value,
) -> (tempfile::TempDir, Running, tokio::net::UnixStream) {
    let listener = tokio::net::UnixListener::bind(root.join("release.sock")).unwrap();
    let definitions = serde_json::to_string(include_str!("../tests/bridge_boundary.py")).unwrap();
    let address = serde_json::to_string(&root.join("release.sock")).unwrap();
    let program = format!(
        r#"import json,socket,sys,types
saved=sys.argv[:]
definitions={definitions}
namespace={{}}
sys.argv=['boundary',json.dumps(saved[1])]
exec(definitions.rsplit('unittest.main(',1)[0],namespace)
sys.argv=saved
release=socket.socket(socket.AF_UNIX)
release.connect({address})
def gate():
 release.sendall(b'E')
 if release.recv(1)!=b'G':
  raise ValueError('release_signal')
clock=namespace['VirtualClock']()
namespace['bridge']['time'].perf_counter=clock.read
laya=namespace['timed_laya'](clock,gate)
torch=types.ModuleType('torch')
torch.set_num_threads=torch.set_num_interop_threads=lambda count:None
torch.jit=types.SimpleNamespace()
transformers=types.ModuleType('transformers')
transformers.AutoConfig=types.SimpleNamespace(from_pretrained=None)
transformers.AutoTokenizer=types.SimpleNamespace(from_pretrained=None)
sys.modules.update({{'laya':laya,'torch':torch,'transformers':transformers}})
exec(compile(saved[1],'bridge','exec'),{{'__name__':'__main__'}})
"#
    );
    let (folder, profile) = runtime_folder(&program, "deadline-fixture");
    std::fs::write(
        folder.path().join("model/rl_agent_config.json"),
        config.to_string(),
    )
    .unwrap();
    let running = started(folder.path(), &profile, &crate::tests::fixture_model()).await;
    let (release, _) = listener.accept().await.unwrap();
    (folder, running, release)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_deadline_shorter_than_measured_cost_reaches_no_inference_and_legacy_still_runs() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let root = tempfile::tempdir().unwrap();
    let (_runtime, running, mut release) = deadline_runtime(
        root.path(),
        &json!({"max_len": 512, "head_max_len": 40, "fixed_seconds": 10.0, "seconds_per_token": 1.0}),
    ).await;
    let request = json!({
        "model": running.provider.model, "state": {"controlled": true},
        "questions": {"only": {"type": "noul", "instructions": "fixture", "criteria": {"false": "False", "true": "True"}}},
    });
    let budget = zerocode_core::jev::BROWSER.apply_deadline_ms.unwrap();
    let mut asking = Box::pin(
        client()
            .post(running.provider.request_url())
            .header(judgment_provider::DEADLINE_HEADER, budget.to_string())
            .json(&request)
            .send(),
    );
    let (response, inferred) = tokio::select! {
        response = &mut asking => (response.unwrap(), false),
        entered = release.read_u8() => {
            assert_eq!(entered.unwrap(), b'E');
            release.write_all(b"G").await.unwrap();
            (asking.as_mut().await.unwrap(), true)
        }
    };
    assert!(
        !inferred,
        "a deadline shorter than measured cost entered native inference"
    );
    assert_eq!(response.status(), reqwest::StatusCode::BAD_GATEWAY);
    let refusal: Value = response.json().await.unwrap();
    assert_eq!(refusal["refusal"], "provider_deadline_budget");
    assert_eq!(refusal["usage"]["model_calls"], 0);
    assert!(refusal["usage"]["estimated_ms"].as_f64().unwrap() > budget as f64);
    assert!(running.engine.alive());
    let mut legacy = Box::pin(
        client()
            .post(running.provider.request_url())
            .json(&request)
            .send(),
    );
    tokio::select! {
        response = &mut legacy => panic!("a legacy request skipped its native release signal: {:?}", response.unwrap().status()),
        entered = release.read_u8() => assert_eq!(entered.unwrap(), b'E'),
    }
    release.write_all(b"G").await.unwrap();
    assert_eq!(
        legacy.as_mut().await.unwrap().status(),
        reqwest::StatusCode::OK
    );
    assert!(running.engine.alive());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_expired_budget_stops_between_native_batches_and_leaves_the_next_request_ready() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let root = tempfile::tempdir().unwrap();
    let (_runtime, running, mut release) = deadline_runtime(
        root.path(),
        &json!({"max_len": 512, "head_max_len": 40, "fixed_seconds": 0.001, "seconds_per_token": 0.001}),
    ).await;
    let budget = zerocode_core::jev::BROWSER.apply_deadline_ms.unwrap();
    let criteria: serde_json::Map<String, Value> = (0..20)
        .map(|index| (format!("option-{index}"), json!(format!("control {index}"))))
        .collect();
    let request = json!({
        "model": running.provider.model,
        "state": {"controlled": true, "advance_seconds": budget as f64 / 1000.0 * 2.0},
        "questions": {"choice": {"type": "choice", "instructions": "Choose the next visible control for this goal.", "criteria": criteria}},
    });
    let mut asking = Box::pin(
        client()
            .post(running.provider.request_url())
            .header(judgment_provider::DEADLINE_HEADER, budget.to_string())
            .json(&request)
            .send(),
    );
    tokio::select! {
        response = &mut asking => panic!("the first native batch never reached its release signal: {:?}", response.unwrap().status()),
        entered = release.read_u8() => assert_eq!(entered.unwrap(), b'E'),
    }
    release.write_all(b"G").await.unwrap();
    let (response, another_batch) = tokio::select! {
        response = &mut asking => (response.unwrap(), false),
        entered = release.read_u8() => {
            assert_eq!(entered.unwrap(), b'E');
            release.write_all(b"G").await.unwrap();
            (asking.as_mut().await.unwrap(), true)
        }
    };
    assert!(
        !another_batch,
        "an expired budget entered another native batch"
    );
    assert_eq!(response.status(), reqwest::StatusCode::BAD_GATEWAY);
    let refused: Value = response.json().await.unwrap();
    assert_eq!(refused["refusal"], "provider_deadline_budget");
    assert_eq!(refused["usage"]["model_calls"], 1);
    let following = json!({
        "model": running.provider.model, "state": "short",
        "questions": {"only": {"type": "noul", "instructions": "fixture", "criteria": {"false": "False", "true": "True"}}},
    });
    let response = client()
        .post(running.provider.request_url())
        .header(judgment_provider::DEADLINE_HEADER, budget.to_string())
        .json(&following)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    assert!(running.engine.alive());
}

fn observation_context() -> Value {
    let mut jobs = std::env::var_os("LOCAL_JUDGMENT_BUILD_RUN")
        .and_then(|path| std::fs::read_dir(path).ok())
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    jobs.sort();
    json!({
        "at_ms": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis(),
        "running_jobs": jobs,
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "contract: runs every compiled screen shape against the pinned model with its caller deadline"]
async fn every_compiled_screen_shape_finishes_or_refuses_before_its_caller_deadline() {
    let root = std::env::var_os(PINNED_ROOT)
        .unwrap_or_else(|| panic!("did not run: {PINNED_ROOT} names no folder"));
    let profile = crate::catalog::runtime().unwrap().unwrap();
    let model = crate::catalog::catalog()
        .unwrap()
        .models
        .into_iter()
        .find(|model| model.engine == EngineKind::Laya)
        .unwrap();
    let runtime = Path::new(&root).join("runtime").join(&profile.id);
    let folder = Path::new(&root).join("models").join(&model.id);
    for (place, fingerprint) in [
        (&runtime, profile.fingerprint().unwrap()),
        (&folder, model.fingerprint().unwrap()),
    ] {
        let receipt: Value =
            serde_json::from_slice(&std::fs::read(place.join("receipt.json")).unwrap()).unwrap();
        assert_eq!(
            receipt["fingerprint"], fingerprint,
            "did not run: the clone is not manifest-pinned"
        );
    }
    let startup_context = observation_context();
    let began = Instant::now();
    let engine = Engine::spawn(&runtime, &profile, Some((folder.as_path(), &model))).unwrap();
    let process = engine.clone();
    tokio::time::timeout(
        START_TIMEOUT,
        tokio::task::spawn_blocking(move || process.ready()),
    )
    .await
    .unwrap()
    .unwrap()
    .unwrap();
    let running = serve(&model, engine, Arc::new(|| {})).await.unwrap();
    let startup_ms = began.elapsed().as_secs_f64() * 1000.0;
    let mut fixtures = compiled_goal_head_shapes();
    for mut fixture in compiled_recorded_goal_heads() {
        fixtures.push(fixture.clone());
        fixture["shape"]["questions"] = json!("action");
        fixture["request"]["questions"]
            .as_object_mut()
            .unwrap()
            .retain(|name, _| name == "action");
        fixtures.push(fixture);
    }
    let budget_ms = zerocode_core::jev::BROWSER.apply_deadline_ms.unwrap();
    let deadline = Duration::from_millis(budget_ms);
    let client = client();
    let mut rows = Vec::new();
    for fixture in fixtures {
        let mut request = fixture["request"].clone();
        request["model"] = json!(model.id);
        let context = observation_context();
        let began = Instant::now();
        let response = client
            .post(running.provider.request_url())
            .header(judgment_provider::DEADLINE_HEADER, budget_ms.to_string())
            .timeout(deadline)
            .json(&request)
            .send()
            .await;
        let (status, body, timed_out) = match response {
            Ok(response) => {
                let status = response.status().as_u16();
                let body = response.json::<Value>().await.unwrap_or(Value::Null);
                (Some(status), body, false)
            }
            Err(error) => (None, Value::Null, error.is_timeout()),
        };
        let elapsed = began.elapsed();
        let outcome = if status == Some(200) {
            body["usage"]["truncated"] != true
                && body["usage"]["state_tokens_dropped"]
                    .as_u64()
                    .is_none_or(|dropped| dropped == 0)
                && body["usage"]["truncated_questions"]
                    .as_array()
                    .is_none_or(Vec::is_empty)
                && body["answers"].as_object().is_some_and(|answers| {
                    answers.len() == request["questions"].as_object().unwrap().len()
                })
        } else {
            status == Some(502)
                && body["refusal"] == "provider_deadline_budget"
                && body["usage"]["model_calls"] == 0
        };
        rows.push(json!({
            "shape": fixture["shape"], "context": context, "finished_context": observation_context(),
            "elapsed_ms": elapsed.as_secs_f64() * 1000.0, "status": status, "body": body,
            "timed_out": timed_out, "pass": outcome && !timed_out && elapsed <= deadline,
        }));
    }
    let report = json!({"budget_ms":budget_ms,"startup_ms":startup_ms,"startup_context":startup_context,"rows":rows});
    if let Some(path) = std::env::var_os("LOCAL_JUDGMENT_DEADLINE_REPORT") {
        std::fs::write(path, serde_json::to_vec(&report).unwrap()).unwrap();
    }
    let failed: Vec<&Value> = report["rows"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["pass"] != true)
        .collect();
    assert!(
        failed.is_empty(),
        "{} compiled screen shapes missed their caller deadline or lacked a named preflight refusal; first: {}",
        failed.len(),
        failed.first().copied().unwrap_or(&Value::Null)
    );
}

async fn snapshot_matching(
    snapshots: &mut tokio::sync::mpsc::UnboundedReceiver<crate::Snapshot>,
    predicate: impl Fn(&crate::Snapshot) -> bool,
) -> crate::Snapshot {
    tokio::time::timeout(START_TIMEOUT, async {
        loop {
            let snapshot = snapshots
                .recv()
                .await
                .expect("the manager still reports its state");
            if predicate(&snapshot) {
                return snapshot;
            }
        }
    })
    .await
    .expect("the manager reaches the requested state before its startup bound")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "contract: records native deadline restart cost and the persisted exit in PINNED_ROOT"]
async fn a_deadline_stop_records_its_exit_and_the_cost_of_the_next_successful_question() {
    let root = std::env::var_os(PINNED_ROOT)
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| panic!("did not run: {PINNED_ROOT} names no folder"));
    let profile = crate::catalog::runtime().unwrap().unwrap();
    let model = crate::catalog::catalog()
        .unwrap()
        .models
        .into_iter()
        .find(|model| model.engine == EngineKind::Laya)
        .unwrap();
    let managed = root.join("managed-judgment");
    std::fs::create_dir(&managed).unwrap();
    std::fs::rename(root.join("runtime"), managed.join("runtime")).unwrap();
    std::fs::rename(root.join("models"), managed.join("models")).unwrap();
    let runtime = managed.join("runtime").join(&profile.id);
    let folder = managed.join("models").join(&model.id);
    let settings = root.join("fixture-settings.json");
    std::fs::write(&settings, b"{}").unwrap();
    let (notices, mut snapshots) = tokio::sync::mpsc::unbounded_channel();
    let manager = crate::Manager::new(&root, &settings, move |snapshot| {
        let _ = notices.send(snapshot);
    })
    .unwrap();
    let spawned = Arc::new(std::sync::Mutex::new(Vec::new()));
    let capture = spawned.clone();
    crate::manager::test_support::replace_engine_factory(
        &manager,
        Arc::new(move |model| {
            let engine = Engine::spawn(&runtime, &profile, Some((folder.as_path(), model)))?;
            capture.lock().unwrap().push(engine.clone());
            Ok(engine)
        }),
    );
    let first_context = observation_context();
    let began = Instant::now();
    manager.start(&model.id, &model.revision).unwrap();
    let ready = snapshot_matching(&mut snapshots, |snapshot| {
        snapshot.server.state == "ready"
            && snapshot
                .operation
                .as_ref()
                .is_some_and(|operation| operation.state == "done")
    })
    .await;
    let first_start = began.elapsed();
    let first_id = ready.server.id.unwrap();
    manager.check(&first_id).unwrap();
    snapshot_matching(&mut snapshots, |snapshot| {
        snapshot.server.checked
            && snapshot
                .operation
                .as_ref()
                .is_some_and(|operation| operation.state == "done")
    })
    .await;
    let expected = Provider::read(&settings).unwrap().identity();
    manager
        .apply(&first_id, &expected, |provider, expected, binding| {
            assert_eq!(Provider::read(&settings).unwrap().identity(), expected);
            std::fs::write(
                &settings,
                serde_json::to_vec(&json!({"smart":{
                    "judgmentProvider": provider, (crate::MANAGED_MODEL_KEY): binding,
                }}))
                .unwrap(),
            )
            .map_err(|_| Error::Storage)
        })
        .unwrap();
    let configured = std::fs::read(&settings).unwrap();
    let engine = spawned.lock().unwrap()[0].clone();
    let stopped_context = observation_context();
    let stopped = Instant::now();
    engine.stop_for(Stop::Deadline(Some("deadline-contract".to_string())));
    let recovering =
        snapshot_matching(&mut snapshots, |snapshot| snapshot.server.exit.is_some()).await;
    let provider = recovering.server.provider.clone().unwrap();
    let mut request = compiled_recorded_goal_heads()
        .into_iter()
        .find(|fixture| fixture["shape"]["navigation"] == "none")
        .unwrap()["request"]
        .clone();
    request["model"] = json!(model.id);
    let deadline = zerocode_core::jev::BROWSER.apply_deadline_ms.unwrap();
    let refused_context = observation_context();
    let asking = Instant::now();
    let refused = client()
        .post(provider.request_url())
        .header(judgment_provider::DEADLINE_HEADER, deadline.to_string())
        .json(&request)
        .send()
        .await
        .unwrap();
    let refused_ms = asking.elapsed().as_secs_f64() * 1000.0;
    let refused_status = refused.status();
    let refused_body: Value = refused.json().await.unwrap();
    let restored = snapshot_matching(&mut snapshots, |snapshot| {
        snapshot.server.state == "ready" && snapshot.server.id.as_deref() != Some(&first_id)
    })
    .await;
    let restore_ms = stopped.elapsed().as_secs_f64() * 1000.0;
    let provider = restored.server.provider.clone().unwrap();
    let response_context = observation_context();
    let asking = Instant::now();
    let response = client()
        .post(provider.request_url())
        .header(judgment_provider::DEADLINE_HEADER, deadline.to_string())
        .json(&request)
        .send()
        .await
        .unwrap();
    let response_ms = asking.elapsed().as_secs_f64() * 1000.0;
    let response_status = response.status();
    let response_body: Value = response.json().await.unwrap();
    let successful_ms = stopped.elapsed().as_secs_f64() * 1000.0;
    let journal = managed.join("server-exits.jsonl");
    let record = tokio::time::timeout(START_TIMEOUT, async {
        loop {
            let text = tokio::fs::read_to_string(&journal)
                .await
                .unwrap_or_default();
            if let Some(record) = text
                .lines()
                .filter_map(|line| serde_json::from_str::<Value>(line).ok())
                .find(|record| record["serverId"] == first_id && record.get("kind").is_none())
            {
                break record;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let report = json!({
        "first_context":first_context,"stopped_context":stopped_context,"refused_context":refused_context,"response_context":response_context,"finished_context":observation_context(),
        "first_start_ready_ms": first_start.as_secs_f64() * 1000.0,
        "immediate_following_ms": refused_ms, "immediate_following_status": refused_status.as_u16(),
        "immediate_following_body": refused_body, "restart_ready_ms": restore_ms,
        "successful_following_ms_including_restart": successful_ms,
        "successful_request_ms": response_ms, "successful_request_status": response_status.as_u16(),
        "successful_response": response_body, "exit_record": record,
        "engine_loads": spawned.lock().unwrap().len(),
    });
    if let Some(path) = std::env::var_os("LOCAL_JUDGMENT_RESTART_REPORT") {
        std::fs::write(path, serde_json::to_vec(&report).unwrap()).unwrap();
    }
    assert_eq!(
        refused_status,
        reqwest::StatusCode::SERVICE_UNAVAILABLE,
        "{report}"
    );
    assert_eq!(response_status, reqwest::StatusCode::OK, "{report}");
    assert_eq!(record["cause"], Error::Timeout.token());
    assert_eq!(record["recovery"], "restart");
    assert_eq!(std::fs::read(&settings).unwrap(), configured);
    assert_eq!(spawned.lock().unwrap().len(), 2);
    manager
        .stop(restored.server.id.as_deref().unwrap())
        .unwrap();
}
