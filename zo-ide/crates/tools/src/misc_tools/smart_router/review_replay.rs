//! Explicit, bounded replay of frozen review-study requests through the real Jev door.
//! This research entrypoint never acts on a judgment or writes a promotion row.

use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use api::{SystemOneClient, SystemOneConfig};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use zerocode_core::jev::{self, learning::{Case, store::{Store, Recorded}}, promote};

use super::jev_gate::{self, JevDoor};
use super::settings::merged_settings_root_from;

const MAX_PLAN_BYTES: u64 = 64 * 1024 * 1024;
const MAX_REQUESTS: usize = 300;
const DEFAULT_REQUESTS: usize = 32;
const DEFAULT_SPEND: f64 = 4.0;
const FRAMING_RESERVE: u64 = 8_192;
const WALL: Duration = Duration::from_secs(30);

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Plan {
    schema_version: u32,
    candidate_sha256: String,
    phase: String,
    seat: String,
    rubric_version: u32,
    model: String,
    requests: Vec<ReplayInput>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReplayInput {
    base_case_id: String,
    workspace: String,
    #[serde(default)]
    origin_group: Option<String>,
    request: Value,
}

fn private_file(path: &Path) -> Result<File, String> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path).map_err(|error| error.to_string())
}

fn write_line(file: &mut File, value: &Value) -> Result<(), String> {
    serde_json::to_writer(&mut *file, value).map_err(|error| error.to_string())?;
    file.write_all(b"\n").map_err(|error| error.to_string())?;
    file.sync_all().map_err(|error| error.to_string())
}

fn read_plan(path: &Path) -> Result<Plan, String> {
    let mut bytes = Vec::new();
    File::open(path).map_err(|error| error.to_string())?.take(MAX_PLAN_BYTES + 1)
        .read_to_end(&mut bytes).map_err(|error| error.to_string())?;
    if bytes.len() as u64 > MAX_PLAN_BYTES { return Err("review plan exceeds its byte limit".into()); }
    serde_json::from_slice(&bytes).map_err(|error| error.to_string())
}

fn allowed(workspace: &Path, seat: &jev::JevUse) -> bool {
    let home = runtime::default_config_home();
    let root: Value = fs::read_to_string(home.join("settings.json")).ok()
        .and_then(|text| serde_json::from_str(&text).ok()).unwrap_or(Value::Null);
    promote::label_drafts_wanted(&root)
        && merged_settings_root_from(&runtime::ConfigLoader::default_for(workspace))
            .is_some_and(|merged| seat.mode_in(&merged).asks())
}

fn has_credential(value: &Value) -> bool {
    match value {
        Value::String(text) => text.lines().any(zerocode_core::credential::may_carry_a_credential),
        Value::Array(values) => values.iter().any(has_credential),
        Value::Object(values) => values.iter().any(|(key, value)|
            zerocode_core::credential::may_carry_a_credential(key) || has_credential(value)),
        _ => false,
    }
}

fn preflight(plan: &Plan, cap: usize, spend: f64) -> Result<&'static jev::JevUse, String> {
    let seat = jev::jev_use(&plan.seat).ok_or("unknown Jev seat")?;
    if plan.schema_version != 1 || !matches!(plan.phase.as_str(), "development" | "heldout")
        || plan.candidate_sha256.len() != 64 || !plan.candidate_sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
        || plan.rubric_version != seat.rubric_version.saturating_add(1)
        || plan.model.trim().is_empty() || !(1..=MAX_REQUESTS).contains(&cap)
        || plan.requests.is_empty() || plan.requests.len() > cap
        || !spend.is_finite() || spend <= 0.0 || spend > DEFAULT_SPEND {
        return Err("invalid, oversized or stale review plan".into());
    }
    let mut ids = HashSet::new();
    let mut inputs = HashSet::new();
    for row in &plan.requests {
        let mut canonical = row.request.clone();
        canonical.sort_all_objects();
        let fingerprint: [u8; 32] = Sha256::digest(canonical.to_string().as_bytes()).into();
        let input = (jev::door::resolved_path(Path::new(&row.workspace)), row.origin_group.clone(), fingerprint);
        if row.base_case_id.len() != 64 || !row.base_case_id.bytes().all(|byte| byte.is_ascii_hexdigit())
            || row.origin_group.as_deref().is_some_and(|origin| origin.len() != 64
                || !origin.bytes().all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()))
            || !ids.insert(&row.base_case_id) || !inputs.insert(input) || !Path::new(&row.workspace).is_absolute()
            || row.request.get("state").is_none()
            || row.request.get("model").and_then(Value::as_str).is_none()
            || !row.request.get("questions").and_then(Value::as_object)
                .is_some_and(|questions| (1..=jev::learning::MAX_BATCH).contains(&questions.len()))
            || row.request.to_string().len() > jev::learning::MAX_CASE_BYTES
            || has_credential(&row.request) {
            return Err("invalid, duplicate or uncleared review request".into());
        }
    }
    Ok(seat)
}

fn price(rate: api::SystemOneRate) -> Value {
    json!({"inputUsdPerMillion":rate.input,"outputUsdPerMillion":rate.output,
        "announced":format!("{:04}-{:02}-{:02}", rate.announced.year, rate.announced.month, rate.announced.day)})
}

/// A fresh output home is a one-shot purchase boundary. A crashed or uncertain
/// run keeps its reservation and cannot silently send the same plan again.
fn replay(plan: &Plan, output: &Path, client: &SystemOneClient, cap: usize, spend_cap: f64) -> Result<Value, String> {
    replay_priced(plan, output, client, cap, spend_cap, api::systemone_rate)
}

fn replay_priced(plan: &Plan, output: &Path, client: &SystemOneClient, cap: usize, spend_cap: f64,
    price_of: impl Fn(&str) -> Option<api::SystemOneRate>) -> Result<Value, String> {
    let seat = preflight(plan, cap, spend_cap)?;
    let rate = price_of(&plan.model).ok_or("the planned model has no known price")?;
    let parent = output.parent().ok_or("output home needs a parent directory")?.canonicalize().map_err(|error| error.to_string())?;
    let checkout = Path::new(env!("CARGO_MANIFEST_DIR")).ancestors().nth(3).ok_or("checkout root unavailable")?
        .canonicalize().map_err(|error| error.to_string())?;
    if parent.starts_with(checkout) { return Err("study evidence must stay outside the public checkout".into()); }
    for row in &plan.requests {
        let workspace = Path::new(&row.workspace);
        if !allowed(workspace, seat) || !JevDoor::open(workspace).permits_application_now() {
            return Err("existing Jev mode, consent or review-retention setting withholds the replay".into());
        }
    }
    let directory = fs::DirBuilder::new();
    #[cfg(unix)]
    let directory = {
        use std::os::unix::fs::DirBuilderExt;
        let mut private = directory;
        private.mode(0o700);
        private
    };
    directory.create(output).map_err(|error| format!("output home must be new; keep existing receipts: {error}"))?;
    let mut journal = private_file(&output.join("replay.jsonl"))?;
    write_line(&mut journal, &json!({"kind":"plan","candidateSha256":plan.candidate_sha256,
        "phase":plan.phase,"seat":plan.seat,"rubricVersion":plan.rubric_version,"model":plan.model,
        "requestCap":cap,"spendCapUsd":spend_cap,"productionChanged":false,
        "costBasis":"estimated_api_usage","price":price(rate)}))?;
    let mut spent = 0.0;
    let mut wire_requests = 0;
    let mut saved = 0;
    for row in &plan.requests {
        let workspace = Path::new(&row.workspace);
        let door = JevDoor::open(workspace);
        if !allowed(workspace, seat) { return Err("review settings changed; replay stopped".into()); }
        if row.request["model"].as_str() != Some(door.model()) {
            return Err("model pin changed since the captured request".into());
        }
        let preview = door.clear(seat, true, row.request.clone()).map_err(|refusal| refusal.token().to_string())?;
        let reserved_tokens = u64::try_from(preview.bytes().len()).unwrap_or(u64::MAX).saturating_add(FRAMING_RESERVE);
        if spent + rate.cost_usd(reserved_tokens, reserved_tokens) > spend_cap {
            return Err("review spend reservation would exceed the run cap".into());
        }
        let cleared = door.pass(seat, true, row.request.clone()).map_err(|refusal| refusal.token().to_string())?;
        let proof = cleared.clone();
        write_line(&mut journal, &json!({"kind":"reserved","baseCaseId":row.base_case_id,
            "requestFingerprint":jev::fingerprint_of(&String::from_utf8_lossy(proof.bytes())),"inputTokensReserved":reserved_tokens,"outputTokensReserved":reserved_tokens}))?;
        let call = api::sync_bridge::run_blocking(jev_gate::send_once(cleared, client, WALL));
        wire_requests += call.requests;
        let elapsed = jev_gate::millis(call.elapsed);
        let response = match call.outcome {
            Ok(response) => response,
            Err(failure) => {
                write_line(&mut journal, &json!({"kind":"result","baseCaseId":row.base_case_id,
                    "outcome":failure.ledger_token(),"requests":call.requests,"retries":call.retries,
                    "elapsedMs":elapsed,"costUnknown":call.requests>0}))?;
                return Err("replay failed; an uncertain bill is not retried".into());
            }
        };
        let actual_rate = price_of(&response.model);
        let cost = actual_rate.map(|rate| rate.cost_usd(response.usage.input_tokens, response.usage.output_tokens));
        spent += cost.unwrap_or(0.0);
        let over = response.usage.input_tokens > reserved_tokens || response.usage.output_tokens > reserved_tokens || spent > spend_cap;
        write_line(&mut journal, &json!({"kind":"result","baseCaseId":row.base_case_id,
            "outcome":"answered","requests":call.requests,"retries":call.retries,"elapsedMs":elapsed,
            "inputTokens":response.usage.input_tokens,"outputTokens":response.usage.output_tokens,"costUsd":cost,"costUnknown":cost.is_none(),
            "budgetExceeded":over,"model":response.model,"price":actual_rate.map(price)}))?;
        if over || cost.is_none() || response.model != plan.model { return Err("model or spend bound changed; replay stopped".into()); }
        if !allowed(workspace, seat) || !door.permits_application_now() {
            return Err("review permission changed before evidence was retained".into());
        }
        // Keep the bytes actually sent, including the day's last permitted
        // request. Rechecking the spent budget must not erase its receipt.
        let mut trial = *seat;
        trial.rubric_version = plan.rubric_version;
        let at = i64::try_from(super::decision_shadow::unix_millis()).unwrap_or(i64::MAX);
        let case = Case::from_cleared(&trial, &proof, &json!({"model":response.model,"answers":response.answers}), at)
            .and_then(|case| case.with_workspace(workspace))
            .and_then(|case| case.with_origin_group(row.origin_group.clone())).map_err(|error| error.to_string())?;
        match Store::at(output).record(&case).map_err(|error| error.to_string())? {
            Recorded::Saved | Recorded::Existing => saved += 1,
            Recorded::Full => return Err("study evidence store is full".into()),
        }
        write_line(&mut journal, &json!({"kind":"retained","baseCaseId":row.base_case_id,"caseId":case.id}))?;
    }
    let result = json!({"requests":wire_requests,"cases":saved,"costUsd":spent,"productionChanged":false});
    write_line(&mut journal, &json!({"kind":"complete","summary":result}))?;
    Ok(result)
}

#[test]
#[ignore = "explicit paid research: reads ZEROCODE_JEV_REVIEW_PLAN and writes a new ZEROCODE_JEV_REVIEW_OUTPUT home"]
fn replay_frozen_review_requests() {
    let input = PathBuf::from(std::env::var("ZEROCODE_JEV_REVIEW_PLAN").expect("a frozen review request plan"));
    let output = PathBuf::from(std::env::var("ZEROCODE_JEV_REVIEW_OUTPUT").expect("a new private output home"));
    let cap = std::env::var("ZEROCODE_JEV_REVIEW_CAP").ok().map_or(Ok(DEFAULT_REQUESTS), |word| word.parse()).expect("a request cap");
    let spend = std::env::var("ZEROCODE_JEV_REVIEW_SPEND_USD").ok().map_or(Ok(DEFAULT_SPEND), |word| word.parse()).expect("a spend cap");
    let plan = read_plan(&input).expect("a bounded review plan");
    let client = SystemOneConfig::from_env().expect("the existing configured Jev key").into_client();
    println!("{}", replay(&plan, &output, &client, cap, spend).expect("the replay's receipts explain a refusal"));
}

fn fixture(workspace: &Path) -> Plan {
    Plan { schema_version: 1, candidate_sha256: "a".repeat(64), phase: "heldout".into(),
        seat: jev::AGENT_TOOL.id.into(), rubric_version: jev::AGENT_TOOL.rubric_version + 1,
        model: "jev-1.13.0".into(), requests: vec![ReplayInput { base_case_id: "b".repeat(64),
            origin_group: Some("e".repeat(64)),
            workspace: workspace.to_string_lossy().into_owned(),
            request: json!({"model":jev::DEFAULT_MODEL,"state":{"context":"a public test case"},
                "questions":{"q":{"type":"noul","instructions":"Is this useful?",
                    "criteria":{"yes":"Useful","no":"Not useful"}}}}) }] }
}

fn review_settings(change: impl FnOnce(&mut Value)) {
    let path = runtime::default_config_home().join("settings.json");
    let mut root: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    root["smart"]["jev"]["labelDrafts"] = json!(true);
    change(&mut root);
    fs::write(path, root.to_string()).unwrap();
}

fn reply() -> String {
    json!({"model":"jev-1.13.0","answers":{"q":{"type":"noul","noul":0.5}},
        "usage":{"input_tokens":10,"output_tokens":0}}).to_string()
}

#[test]
fn a_replay_keeps_the_last_daily_request_and_records_no_correctness_or_production_row() {
    use super::jev_mock::{machine, Mock};
    let mock = Mock::serving(200, reply());
    machine(&jev::AGENT_TOOL, "on", &mock.base_url, |workspace| {
        review_settings(|root| root["smart"]["jev"]["dailyRequests"] = json!(1));
        let parent = tempfile::tempdir().unwrap();
        let output = parent.path().join("candidate");
        let client = SystemOneClient::new(&mock.base_url, "test-key");
        let plan = fixture(workspace);
        let summary = replay(&plan, &output, &client, 1, DEFAULT_SPEND).unwrap();
        assert_eq!(summary["requests"], 1);
        assert_eq!(mock.requests().len(), 1);
        let snapshot = Store::at(&output).snapshot().unwrap();
        assert_eq!(snapshot.cases.len(), 1);
        assert!(snapshot.outcomes.is_empty(), "the wire cannot invent a correctness label");
        assert_eq!(snapshot.cases[0].workspace.as_deref(), Some(plan.requests[0].workspace.as_str()));
        assert_eq!(snapshot.cases[0].rubric_version, plan.rubric_version);
        assert!(Store::at(&runtime::default_config_home()).snapshot().unwrap().cases.is_empty());
        assert!(!super::shadow_ledger::shadow_ledger_path(workspace, jev::AGENT_TOOL.ledger).exists());
        let receipt = fs::read_to_string(output.join("replay.jsonl")).unwrap();
        assert!(receipt.contains("estimated_api_usage") && receipt.contains("announced"));
    });
}

#[test]
fn existing_off_consent_and_retention_settings_refuse_replays_before_the_wire() {
    use super::jev_mock::{machine, Mock};
    for setting in ["global", "seat", "consent", "retention"] {
        let mock = Mock::serving(200, reply());
        machine(&jev::AGENT_TOOL, "on", &mock.base_url, |workspace| {
            review_settings(|root| match setting {
                "global" => root["smart"]["jev"]["enabled"] = json!(false),
                "seat" => root["smart"][jev::AGENT_TOOL.setting] = json!("off"),
                "consent" => root["smart"]["jev"]["workspaces"] = json!([]),
                _ => root["smart"]["jev"]["labelDrafts"] = json!(false),
            });
            let parent = tempfile::tempdir().unwrap();
            let output = parent.path().join("candidate");
            let client = SystemOneClient::new(&mock.base_url, "test-key");
            assert!(replay(&fixture(workspace), &output, &client, 1, DEFAULT_SPEND).is_err(), "{setting}");
            assert!(mock.requests().is_empty());
            assert!(!output.exists(), "{setting}");
        });
    }
}

#[test]
fn an_insufficient_replay_spend_cap_does_not_take_the_daily_slot() {
    use super::jev_mock::{machine, Mock};
    let mock = Mock::serving(200, reply());
    machine(&jev::AGENT_TOOL, "on", &mock.base_url, |workspace| {
        review_settings(|root| root["smart"]["jev"]["dailyRequests"] = json!(1));
        let parent = tempfile::tempdir().unwrap();
        let client = SystemOneClient::new(&mock.base_url, "test-key");
        let plan = fixture(workspace);
        assert!(replay(&plan, &parent.path().join("too-small"), &client, 1, f64::EPSILON).is_err());
        assert!(mock.requests().is_empty());
        assert!(replay(&plan, &parent.path().join("allowed"), &client, 1, DEFAULT_SPEND).is_ok());
        assert_eq!(mock.requests().len(), 1);
    });
}

#[test]
fn a_nonzero_output_price_is_reserved_before_purchase_and_counted_from_usage() {
    use super::jev_mock::{machine, Mock};
    let mut response: Value = serde_json::from_str(&reply()).unwrap();
    response["usage"]["output_tokens"] = json!(20);
    let mock = Mock::serving(200, response.to_string());
    machine(&jev::AGENT_TOOL, "on", &mock.base_url, |workspace| {
        review_settings(|root| root["smart"]["jev"]["dailyRequests"] = json!(1));
        let parent = tempfile::tempdir().unwrap();
        let client = SystemOneClient::new(&mock.base_url, "test-key");
        let plan = fixture(workspace);
        let rate = api::SystemOneRate { output: 4.0, ..api::systemone_rate(&plan.model).unwrap() };
        let preview = JevDoor::open(workspace).clear(&jev::AGENT_TOOL, true, plan.requests[0].request.clone()).unwrap();
        let only_input = rate.input_cost_usd(u64::try_from(preview.bytes().len()).unwrap() + FRAMING_RESERVE);
        assert!(replay_priced(&plan, &parent.path().join("insufficient"), &client, 1,
            only_input + 0.000_001, |_| Some(rate)).is_err());
        assert!(mock.requests().is_empty());
        let result = replay_priced(&plan, &parent.path().join("allowed"), &client, 1,
            DEFAULT_SPEND, |_| Some(rate)).unwrap();
        assert_eq!(mock.requests().len(), 1);
        let wanted = rate.input_cost_usd(10) + 20.0 * 4.0 / 1_000_000.0;
        assert!((result["costUsd"].as_f64().unwrap() - wanted).abs() < 1e-12);
    });
}

#[test]
fn an_uncertain_replay_bill_is_kept_and_never_retried_in_the_same_run() {
    use super::jev_mock::{machine, Mock};
    let mock = Mock::serving(503, "{}".into());
    machine(&jev::AGENT_TOOL, "on", &mock.base_url, |workspace| {
        review_settings(|_| {});
        let parent = tempfile::tempdir().unwrap();
        let output = parent.path().join("candidate");
        let client = SystemOneClient::new(&mock.base_url, "test-key");
        let plan = fixture(workspace);
        assert!(replay(&plan, &output, &client, 1, DEFAULT_SPEND).is_err());
        assert_eq!(mock.requests().len(), 1);
        assert!(fs::read_to_string(output.join("replay.jsonl")).unwrap().contains("\"costUnknown\":true"));
        assert!(replay(&plan, &output, &client, 1, DEFAULT_SPEND).is_err());
        assert_eq!(mock.requests().len(), 1, "there is no hidden retry after an uncertain purchase");
    });
}

#[test]
fn duplicate_inputs_and_uncleared_instruction_words_are_rejected_before_purchase() {
    let workspace = tempfile::tempdir().unwrap();
    let mut plan = fixture(workspace.path());
    plan.requests.push(ReplayInput { base_case_id: "c".repeat(64), workspace: plan.requests[0].workspace.clone(),
        origin_group: plan.requests[0].origin_group.clone(),
        request: plan.requests[0].request.clone() });
    assert!(preflight(&plan, 2, DEFAULT_SPEND).is_err());
    plan.requests.pop();
    plan.requests[0].request["questions"]["q"]["instructions"] = json!("Authorization: Bearer sample-private-value");
    assert!(preflight(&plan, 1, DEFAULT_SPEND).is_err());
}
