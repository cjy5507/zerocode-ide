//! Opt-in, fixed-response workflow measurements. These measure the host and
//! prompt envelope; they do not estimate real model accuracy or actual bills.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use runtime::{ProjectContext, SkillIndexEntry, SkillsIndexRoad, SystemPromptBuilder};
use runtime::skill_rank::SkillSuggestionSeat;
use serde_json::{json, Value};
use zerocode_core::jev::{JevMode, SKILLS, SKILL_SUGGESTION};

use super::jev_mock::{machine_words, Mock};
use super::skill_search::{skill_suggestion_path, SkillSuggestionJudge};
use super::super::skill_tools::{execute_skill_load, execute_skill_search_for_session, SkillLoadInput, SkillSearchInput};

fn description(index: usize) -> String {
    format!("Fixture topic {index}: {}", "A documented procedure for the named fixture task and its checks. ".repeat(8))
}

fn catalog(count: usize) -> Vec<SkillIndexEntry> {
    (0..count).map(|index| SkillIndexEntry::new(format!("fixture-{index:03}"),
        Some(description(index)), PathBuf::from(format!("/work/skills/fixture-{index:03}/SKILL.md")))).collect()
}

fn overflow_boundary() -> usize {
    (2..=256).find(|count| SkillsIndexRoad::decide(JevMode::Off, false, &catalog(*count)) == SkillsIndexRoad::Tools)
        .expect("fixture catalog must cross the real prompt budget")
}

#[derive(Default)]
struct WireCounts { explicit: usize, wide: usize, narrow: usize, bytes: usize }

fn replay(body: &str, counts: &Mutex<WireCounts>) -> String {
    let request: Value = serde_json::from_str(body).unwrap();
    let task = request["state"]["task"].as_str().unwrap();
    let wanted = task.split("fixture-").nth(1).and_then(|tail| tail.get(..3)).and_then(|n| n.parse::<usize>().ok());
    let questions = request["questions"].as_object().unwrap();
    let mut counted = counts.lock().unwrap();
    counted.bytes += body.len();
    if questions.contains_key("acts_on_system") { counted.wide += 1; }
    else if questions.contains_key("which") { counted.narrow += 1; }
    else { counted.explicit += 1; }
    drop(counted);
    let answers: serde_json::Map<String, Value> = questions.iter().map(|(id, question)| {
        let answer = match question["type"].as_str().unwrap() {
            "choice" => {
                let options = question["criteria"].as_object().unwrap();
                let selected = wanted.map(|index| format!("s{index}")).filter(|key| options.contains_key(key))
                    .unwrap_or_else(|| "__no_skill__".into());
                let probabilities: serde_json::Map<String, Value> = options.keys()
                    .map(|key| (key.clone(), json!(if *key == selected { 1.0 } else { 0.0 }))).collect();
                json!({"type":"choice","choice":selected,"confidence":1.0,"probabilities":probabilities})
            }
            "noul" => json!({"type":"noul","noul": if (id == "prose_suffices") == wanted.is_none() { 1.0 } else { 0.0 }}),
            "score" => {
                let top = wanted.is_some_and(|index| *id == format!("s{index}"));
                let legend: serde_json::Map<String, Value> = zerocode_core::jev::SKILL_LEVELS.iter()
                    .enumerate().map(|(index, text)| (index.to_string(), json!(text))).collect();
                json!({"type":"score","score":if top { 2.0 } else { 0.0 },"confidence":1.0,
                    "legend":legend,"probabilities":{"0":if top {0.0} else {1.0},"1":0.0,"2":if top {1.0} else {0.0}}})
            }
            other => panic!("unexpected fixture question {other}"),
        };
        (id.clone(), answer)
    }).collect();
    json!({"model":"jev-fixture-replay","answers":answers,"usage":{"input_tokens":0,"output_tokens":0}}).to_string()
}

fn estimated_tokens(bytes: usize) -> usize { bytes.div_ceil(4) }

fn wait_for_suggestion(cwd: &Path, previous: usize) {
    let until = Instant::now() + Duration::from_secs(5);
    while super::jev_summary::read_rows(&skill_suggestion_path(cwd)).len() <= previous {
        assert!(Instant::now() < until, "detached suggestion did not settle");
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn session(cwd: &Path, needed_percent: usize, counts: &Mutex<WireCounts>) -> Value {
    let setup = Instant::now();
    let context = ProjectContext::discover(cwd.to_path_buf(), "2026-10-03").unwrap();
    let road = context.skills_index_road;
    let installed = context.skills_index.len();
    let prompt = SystemPromptBuilder::new().with_project_context(context).render();
    let judge = SkillSuggestionJudge::at(cwd);
    let setup_us = setup.elapsed().as_micros();
    let mut history: Vec<Value> = Vec::new();
    let (mut prompt_bytes, mut model_requests, mut tool_calls, mut loaded, mut expected_loads) = (0, 0, 0, 0, 0);
    let mut foreground_us = 0;
    let mut drain_us = 0;
    let before = {
        let c = counts.lock().unwrap(); (c.explicit, c.wide, c.narrow, c.bytes)
    };
    for turn in 0..12 {
        let needs_skill = (turn * 100 / 12) < needed_percent;
        let wanted = format!("fixture-{:03}", turn % installed);
        let task = if needs_skill { format!("Turn {turn}: Use {wanted} for its documented fixture task.") }
            else { format!("Turn {turn}: Explain a general idea; no installed procedure is needed.") };
        let rows_before = super::jev_summary::read_rows(&skill_suggestion_path(cwd)).len();
        let began = Instant::now();
        let hint = api::sync_bridge::run_blocking(judge.suggest(task.clone()));
        history.push(json!({"role":"user","content":task,"hint":hint}));
        prompt_bytes += prompt.len() + serde_json::to_vec(&history).unwrap().len();
        model_requests += 1;
        if needs_skill {
            expected_loads += 1;
            let (name, output) = match road {
                SkillsIndexRoad::Index => ("skill_load", serde_json::to_value(execute_skill_load(
                    &SkillLoadInput { names: vec![wanted.clone()] }, cwd).unwrap()).unwrap()),
                SkillsIndexRoad::Tools => ("skill_search", serde_json::to_value(execute_skill_search_for_session(
                    &SkillSearchInput { task: task.clone(), max_skills: Some(1) }, cwd, Some("impact-session")).unwrap()).unwrap()),
            };
            tool_calls += 1;
            if output["skills"].as_array().unwrap().iter().any(|skill|
                skill["skill"] == wanted && skill["prompt"].as_str().unwrap().contains(&format!("BODY:{wanted}"))) { loaded += 1; }
            history.push(json!({"role":"tool","name":name,"output":output}));
            prompt_bytes += prompt.len() + serde_json::to_vec(&history).unwrap().len();
            model_requests += 1;
        }
        history.push(json!({"role":"assistant","content":"Fixture turn complete."}));
        foreground_us += began.elapsed().as_micros();
        let drain = Instant::now();
        if SKILL_SUGGESTION.mode_in(&super::settings::merged_settings_root(cwd).unwrap()).asks()
            && installed <= zerocode_core::jev::SKILL_SUGGESTION_CATALOG_CAP {
            wait_for_suggestion(cwd, rows_before);
        }
        judge.finish(&[]);
        drain_us += drain.elapsed().as_micros();
    }
    let after = counts.lock().unwrap();
    json!({"turns":12,"neededPercent":needed_percent,"installed":installed,
        "road":if road == SkillsIndexRoad::Index {"index"} else {"tools"},
        "initialPromptBytes":prompt.len(),"initialPromptEstimatedTokens":estimated_tokens(prompt.len()),
        "sessionPromptBytes":prompt_bytes,"sessionPromptEstimatedTokens":estimated_tokens(prompt_bytes),
        "scriptedFrontierRequests":model_requests,"toolCalls":tool_calls,"expectedLoads":expected_loads,"successfulLoads":loaded,
        "setupMicros":setup_us,"foregroundMicros":foreground_us,"backgroundDrainMicros":drain_us,
        "jevExplicitRequests":after.explicit-before.0,"jevWideRequests":after.wide-before.1,
        "jevNarrowRequests":after.narrow-before.2,"jevRequestBytes":after.bytes-before.3,
        "providerReportedUsage":null,"modelWaitMicros":null,"modelOutput":"fixed local replay; not model accuracy"})
}

#[test]
#[ignore = "explicit fixed-response workflow measurement; no real model or user state"]
fn measure_skill_workflow_impact() {
    let mode = std::env::var("JEV_IMPACT_MODE").expect("off/shadow/on");
    assert!(["off", "shadow", "on"].contains(&mode.as_str()));
    let size = std::env::var("JEV_IMPACT_SIZE").expect("below/near/above");
    let phase = std::env::var("JEV_IMPACT_PHASE").expect("cold/warm");
    let needed = std::env::var("JEV_IMPACT_NEEDED_PERCENT").unwrap().parse::<usize>().unwrap();
    assert!([0,25,100].contains(&needed));
    let boundary = overflow_boundary();
    let count = match size.as_str() { "below" => (boundary/2).max(2), "near" => boundary-1, "above" => boundary+1, _ => panic!("size") };
    let counts = Arc::new(Mutex::new(WireCounts::default()));
    let recording = Arc::clone(&counts);
    let endpoint = Mock::answering(move |body| (200, replay(body, &recording)));
    machine_words(&[(SKILLS.setting, &mode), (SKILL_SUGGESTION.setting, &mode)], &endpoint.base_url, |cwd| {
        for entry in catalog(count) {
            let dir = cwd.join(".zo/skills").join(&entry.name);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("SKILL.md"), format!("---\nname: {}\ndescription: {}\n---\nBODY:{}\nFollow the fixture procedure and verify its result.\n",
                entry.name, entry.description.unwrap(), entry.name)).unwrap();
        }
        if phase == "warm" { let _ = session(cwd, needed, &counts); }
        else { assert_eq!(phase, "cold"); }
        let mut measured = session(cwd, needed, &counts);
        measured["mode"] = json!(mode); measured["size"] = json!(size); measured["phase"] = json!(phase);
        measured["overflowBoundary"] = json!(boundary);
        println!("JEV_IMPACT_JSON={measured}");
    });
}
