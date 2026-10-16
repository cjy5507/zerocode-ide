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
        .args(["-I", "-c", PINNED_HEADS])
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
