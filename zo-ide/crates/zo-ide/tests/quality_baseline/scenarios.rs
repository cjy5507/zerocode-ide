use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};
use serde_json::{json, Value};
use tempfile::TempDir;
use zerocode_harness::Client;
use crate::{baseline::{self, Fields}, harness::PtyRun, scripted::ScriptedAnthropicService};

fn command(args: &[&str], cwd: &Path) {
    let output = Command::new(args[0]).args(&args[1..]).current_dir(cwd).output().unwrap();
    assert!(output.status.success(), "{args:?}: {}", String::from_utf8_lossy(&output.stderr));
}
fn evidence(root: &Path, name: &str, data: &Value) {
    fs::write(root.join("evidence").join(name), serde_json::to_vec_pretty(data).unwrap()).unwrap();
}
fn timeout() -> Duration { Duration::from_millis(baseline::table().limits.timeout_ms) }
fn pty(root: &Path, url: &str, resume: Option<&str>) -> PtyRun {
    let mut args = vec!["--model", "claude-sonnet-4-6", "--permission-mode", "danger-full-access"];
    if let Some(id) = resume { args.extend(["--resume", id]); }
    let address = root.join("address").to_string_lossy().into_owned();
    let probe = root.join("evidence/paint").to_string_lossy().into_owned();
    PtyRun::spawn_with_env(&root.join("repo"), &root.join("home"), &root.join("sessions"), &root.join("state"), url, &args,
        &[("ZO_PROBE_PAINT", &probe), ("ZO_CLAUDE_HOME", ""), ("ZO_CODEX_HOME", ""), ("ZO_EVENTS_ADDR_FILE", &address), ("ZO_SERVE_TOKEN", "baseline-canary")]).unwrap()
}
async fn capabilities(root: &Path) -> Value {
    let deadline = Instant::now() + timeout();
    loop {
        if let Ok(address) = fs::read_to_string(root.join("address")) {
            if let Ok(mut client) = Client::connect(address.trim(), Some("baseline-canary".into())).await {
                if let Ok(value) = client.call("session.capabilities", json!({})).await { return value; }
            }
        }
        assert!(Instant::now() < deadline, "capability endpoint timeout");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}
fn bytes(root: &Path) -> u64 {
    fs::read_dir(root).unwrap().filter_map(Result::ok).map(|e| {
        let p = e.path(); if p.is_dir() { bytes(&p) } else { e.metadata().unwrap().len() }
    }).sum()
}
fn transcript_with(root: &Path, needle: &str) -> Option<PathBuf> {
    for entry in fs::read_dir(root).ok()?.flatten() {
        let path = entry.path();
        if path.is_dir() { if let Some(p) = transcript_with(&path, needle) { return Some(p); } }
        else if path.extension().is_some_and(|s| s == "jsonl") && fs::read_to_string(&path).is_ok_and(|s| s.contains(needle)) { return Some(path); }
    }
    None
}
async fn scenario(task: &str, repeat: u64, root: &Path) -> Fields {
    let fixture = baseline::fixture();
    command(&["python3", fixture.join("fixture.py").to_str().unwrap(), "prepare", root.to_str().unwrap()], &fixture);
    for dir in ["home", "sessions", "state"] { fs::create_dir_all(root.join(dir)).unwrap(); }
    fs::write(root.join("home/settings.json"), r#"{"smart":{"autoClassifier":"off"}}"#).unwrap();
    let patch = fixture.join("answers").join(format!("{task}.patch"));
    let service = ScriptedAnthropicService::baseline(task, root.to_str().unwrap(), patch.to_str().unwrap()).await.unwrap();
    let prompt = fs::read_to_string(fixture.join("tasks").join(task).join("prompt.txt")).unwrap();
    let disk_before = bytes(root);
    let start = Instant::now();
    let mut previous_usage = [0; 3];
    let mut resume_id = None;
    if task == "Q3" {
        let parked = ScriptedAnthropicService::parked_tool(baseline::table().limits.interrupt_park_seconds, "unused interrupted final").await.unwrap();
        let mut interrupted = pty(root, parked.base_url(), None);
        interrupted.wait_for("directory:", timeout());
        let cap = capabilities(root).await;
        resume_id = cap["identity"]["session_id"].as_str().map(str::to_owned);
        interrupted.send(format!("{}\r", prompt.trim()).as_bytes()).unwrap();
        let deadline = Instant::now() + timeout();
        while transcript_with(&root.join("sessions"), "toolu_parked_e2e").is_none() {
            assert!(Instant::now() < deadline, "interrupted call was never persisted");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        nix::sys::signal::kill(nix::unistd::Pid::from_raw(i32::try_from(interrupted.pid()).unwrap()), nix::sys::signal::Signal::SIGTERM).unwrap();
        let _ = interrupted.finish();
        previous_usage = parked.usage().await;
        fs::remove_file(root.join("address")).ok();
        // The renderer persists an interrupted pane, is destroyed, and restores it
        // through spawnStoredLeaf. Only its resume request authorizes this nudge.
        let window = fixture.join("../../ui/tests/baseline.mjs");
        command(&["node", window.to_str().unwrap(), "--restore", root.to_str().unwrap(), resume_id.as_deref().unwrap()], &fixture);
        let restore: Value = serde_json::from_slice(&fs::read(root.join("evidence/window-resume.json")).unwrap()).unwrap();
        assert_eq!(restore["interrupted"], true);
        assert_eq!(restore["id"], resume_id.as_deref().unwrap());
    }
    let mut run = pty(root, service.base_url(), resume_id.as_deref());
    run.wait_for("directory:", timeout());
    let cap = capabilities(root).await;
    let offset = run.output_len();
    run.measure_submission();
    let input = if task == "Q3" { "Continue the interrupted task from the saved conversation." } else { prompt.trim() };
    run.send(format!("{input}\r").as_bytes()).unwrap();
    run.wait_for_after("BASELINE_DONE", offset, timeout());
    let measurements = measurements(root, run.pid()).await;
    let first_visible = run.first_visible_ms();
    let interventions = run.interventions();
    let rendered = run.finish();
    fs::write(root.join("evidence/terminal.ansi"), rendered).unwrap();
    let requests = service.request_bodies().await;
    // Requests are hermetic and contain no credentials; judges inspect tool envelopes.
    evidence(root, "requests.json", &json!(requests.iter().map(|r| serde_json::from_str::<Value>(r).unwrap()).collect::<Vec<_>>()));
    if task == "Q3" {
        evidence(root, "resume.json", &json!({"interrupted":true,"resumed":resume_id.is_some(),"session_id":resume_id,"original_prompt":prompt.trim(),"resumed_messages":baseline::messages(&requests[0])}));
    }
    if task == "Q2" {
        let mut receipts = vec![];
        let mut parent = vec![];
        for request in &requests {
            let messages = baseline::messages(request);
            for message in messages.as_array().unwrap() {
                for block in message["content"].as_array().into_iter().flatten() {
                    if block["type"] == "tool_result" {
                        let id = block["tool_use_id"].as_str().unwrap_or_default();
                        if ["BASELINE_CHILD_ALPHA", "BASELINE_CHILD_BETA"].contains(&id) && block["is_error"] != true {
                            let Some(text) = block["content"][0]["text"].as_str() else { continue; };
                            let Ok(receipt) = serde_json::from_str::<Value>(text) else { continue; };
                            let Some(agent_id) = receipt["agentId"].as_str() else { continue; };
                            if parent.iter().any(|s| s == agent_id) { continue; }
                            // The runtime's terminal status, never the assistant's claim.
                            parent.push(agent_id.to_owned());
                            let changed_file = if id.ends_with("ALPHA") { "js/alpha.mjs" } else { "js/beta.mjs" };
                            receipts.push(json!({"id":agent_id,"status":receipt["status"],"path":changed_file}));
                        }
                    }
                }
            }
        }
        evidence(root, "delegation.json", &json!({"receipts":receipts,"parent_tool_results":parent}));
    }
    let verdict = Command::new(fixture.join("tasks").join(task).join("verify.sh")).arg(root).output().unwrap();
    let mut row = Fields::new(task, repeat);
    row.elapsed_ms = u64::try_from(start.elapsed().saturating_sub(measurements.3).as_millis()).unwrap();
    row.stream = measurements.0;
    row.rss_peak_mb = Some(measurements.1);
    row.host.mem = Some(measurements.2);
    row.first_visible_ms = first_visible;
    row.interventions = interventions;
    row.verified = verdict.status.success();
    row.reason = (!row.verified).then(|| "judge_failed".into());
    row.disk_delta_mb = (f64::from(u32::try_from(bytes(root)).unwrap()) - f64::from(u32::try_from(disk_before).unwrap())) / (1024.0 * 1024.0);
    row.protocol.events = serde_json::from_value(cap["protocol"].clone()).ok();
    row.protocol.capabilities = cap["support"].as_object().unwrap().iter().filter(|(_, v)| v["supported"] == true).map(|(k, _)| k.clone()).collect();
    row.build.git_sha = cap["process"]["build"]["git_sha"].as_str().map(str::to_owned);
    row.build.dirty = cap["process"]["build"]["dirty"].as_bool();
    row.model.requested = cap["launch"]["requested"]["model"].as_str().unwrap().into();
    row.model.effective = cap["launch"]["effective"]["wire_model"].as_str().unwrap().into();
    row.model.effort = cap["launch"]["effective"]["wire_effort"].as_str().map(str::to_owned);
    let usage = service.usage().await;
    row.tokens.r#in = usage[0] + previous_usage[0]; row.tokens.out = usage[1] + previous_usage[1]; row.tokens.cached = usage[2] + previous_usage[2];
    row
}
pub async fn run() {
    let temp = TempDir::new().unwrap();
    let output = std::env::var_os("BASELINE_OUT").map_or_else(|| temp.path().to_path_buf(), PathBuf::from);
    fs::create_dir_all(&output).unwrap();
    let repeats = std::env::var("BASELINE_REPEAT").ok().map_or(baseline::table().limits.default_repeat, |s| s.parse().unwrap());
    let selected = std::env::var("BASELINE_TASK").ok().filter(|s| !s.is_empty());
    let mut verdicts = std::collections::BTreeMap::new();
    let mut failed = vec![];
    for repeat in 1..=repeats {
        for task in ["Q1", "Q2", "Q3", "Q4", "Q6"] {
            if selected.as_ref().is_some_and(|s| s != task) { continue; }
            let root = output.join(format!("{task}-{repeat}"));
            let row = scenario(task, repeat, &root).await;
            let line = row.json();
            writeln!(OpenOptions::new().create(true).append(true).open(output.join("runs.jsonl")).unwrap(), "{line}").unwrap();
            if let Some(previous) = verdicts.insert(task, (row.verified, row.interventions)) { assert_eq!(previous, (row.verified, row.interventions), "verdict changed for {task}"); }
            if !row.verified { failed.push(format!("{task}-{repeat}")); }
        }
    }
    assert!(failed.is_empty(), "failed judges: {failed:?}; evidence {}", output.display());
}

async fn measurements(root: &Path, pid: u32) -> (baseline::Stream, f64, u64, Duration) {
    let limits = baseline::table().limits;
    let path = root.join("evidence/paint.frames.jsonl");
    let deadline = Instant::now() + timeout();
    let marks = loop {
        let marks: Vec<Value> = fs::read_to_string(&path).unwrap_or_default().lines()
            .filter_map(|line| serde_json::from_str(line).ok()).collect();
        if marks.last().is_some_and(|mark| mark["mark"] == "turn_end" && mark["pid"] == pid) { break marks; }
        assert!(Instant::now() < deadline, "turn-end paint mark missing: {}", path.display());
        tokio::time::sleep(Duration::from_millis(limits.probe_poll_ms)).await;
    };
    let start = marks.iter().rposition(|mark| mark["mark"] == "turn_start" && mark["pid"] == pid).unwrap();
    let frames: Vec<_> = marks[start..].iter().filter(|mark| mark["mark"] == "frame").collect();
    assert!(!frames.is_empty(), "turn emitted no measured frames");
    let queue_max = frames.iter().map(|mark| mark["queue"].as_u64().unwrap()).max();
    let mut paint: Vec<_> = frames.iter().map(|mark| mark["paint_ms"].as_f64().unwrap()).collect();
    assert!(paint.iter().all(|ms| ms.is_finite() && *ms >= 0.0));
    paint.sort_by(f64::total_cmp);
    let rank = (paint.len() * usize::try_from(limits.paint_percentile).unwrap()).div_ceil(100) - 1;
    let heap_path = root.join("evidence/heap.json");
    let sampling = Instant::now();
    command(&["python3", baseline::fixture().join("heap.py").to_str().unwrap(), &pid.to_string(), heap_path.to_str().unwrap()], root);
    let heap: Value = serde_json::from_slice(&fs::read(heap_path).unwrap()).unwrap();
    let live = heap["live_bytes"].as_f64().unwrap();
    assert!(live > 0.0 && !heap["classes"].as_array().unwrap().is_empty());
    (baseline::Stream { queue_max, paint_p95_ms: Some(paint[rank]) }, live / f64::from(limits.bytes_per_mb), heap["host_mem"].as_u64().unwrap(), sampling.elapsed())
}
