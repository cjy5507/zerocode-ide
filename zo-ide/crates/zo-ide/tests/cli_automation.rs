//! Scripted-provider process regressions. These test CLI contracts, not model quality.
#[allow(dead_code)]
#[path = "e2e/scripted.rs"]
mod scripted;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};
use std::time::Duration;
use core_types::{ContentBlock, ConversationMessage, MessageRole, Session};
use serde_json::{json, Value};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use scripted::ScriptedAnthropicService;

struct Fixture { root: tempfile::TempDir, cwd: PathBuf, home: PathBuf, store: PathBuf }
impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let cwd = root.path().join("workspace");
        let home = root.path().join("config");
        let store = root.path().join("store");
        for path in [&cwd, &home, &store.join("sessions")] { fs::create_dir_all(path).unwrap(); }
        fs::write(home.join("settings.json"), json!({"smart":{"autoClassifier":"off","orchestration":"model"}}).to_string()).unwrap();
        Self { root, cwd: cwd.canonicalize().unwrap(), home, store }
    }
    fn command(&self, args: &[&str], service: Option<&ScriptedAnthropicService>) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_zo"));
        command.env_clear();
        for name in ["PATH", "SystemRoot", "WINDIR", "COMSPEC", "PATHEXT"] {
            if let Some(value) = std::env::var_os(name) { command.env(name, value); }
        }
        command.args(args).current_dir(&self.cwd)
            .env("HOME", &self.home).env("USERPROFILE", &self.home)
            .env("ZO_CONFIG_HOME", &self.home).env("ZO_SESSION_ROOT", &self.store)
            .env("ZO_STATE_DIR", self.store.join("state"))
            .env("ZO_DISABLE_MODEL_DISCOVERY", "1").env("ZO_DISABLE_KEYCHAIN", "1")
            .env("ZO_DISABLE_EXTERNAL_CREDENTIALS", "1").env("NO_COLOR", "1")
            .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
        if let Some(service) = service {
            command.env("ANTHROPIC_API_KEY", "test-dummy-key").env("ANTHROPIC_BASE_URL", service.base_url());
        }
        command
    }
    async fn run(&self, args: &[&str], stdin: &str, service: Option<&ScriptedAnthropicService>) -> Output {
        let mut command = self.command(args, service);
        let mut child = command.spawn().unwrap();
        child.stdin.take().unwrap().write_all(stdin.as_bytes()).await.unwrap();
        tokio::time::timeout(Duration::from_secs(90), child.wait_with_output()).await.expect("CLI timed out").unwrap()
    }
    fn seed(&self, id: &str, text: &str) -> PathBuf {
        let path = self.store.join("sessions").join(format!("{id}.jsonl"));
        let mut session = Session::new(); session.session_id = id.into();
        session.push_user_text(text).unwrap(); session.save_to_path(&path).unwrap(); path
    }
}
fn diagnostic(output: &Output) -> String { format!("{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr)) }
fn success(output: &Output) { assert!(output.status.success(), "{}", diagnostic(output)); }
fn failed(output: &Output, needle: &str) { assert!(!output.status.success(), "{}", diagnostic(output)); assert!(diagnostic(output).contains(needle), "{}", diagnostic(output)); }
fn events(output: &Output) -> Vec<Value> { String::from_utf8_lossy(&output.stdout).lines().map(|line| serde_json::from_str(line).unwrap()).collect() }
const OPTIONS: &[&str] = &["--plain", "--model", "claude-sonnet-4-6", "--permission-mode", "workspace-write"];

#[tokio::test(flavor="multi_thread", worker_threads=2)]
async fn malformed_and_missing_input_fails_before_provider_startup() {
    let f = Fixture::new();
    for (args, needle) in [
        (vec!["--prompt-file", "missing.txt"], "cannot read prompt file"),
        (vec!["--prompt-file", "missing.txt", "prompt"], "choose positional"),
        (vec!["--max-turns", "0", "prompt"], "positive integer"),
        (vec!["--last-message", "--json"], "needs a file path"),
        (vec!["--continue", "--resume", "other"], "choose one --resume"),
        (vec!["--max-output-tokens", "NaN"], "positive integer"),
        (vec!["review", "--base", "bad", "--uncommitted"], "choose exactly one"),
        (vec!["review", "--commit", "--bad"], "requires a value"),
        (vec!["review", "--uncommitted", "--permission-mode", "danger-full-access"], "read-only"),
        (vec!["--add-dir", "missing-dir", "prompt"], "cannot open --add-dir"),
    ] { failed(&f.run(&args, "", None).await, needle); }
    assert!(fs::read_dir(f.store.join("sessions")).unwrap().next().is_none());
}

#[tokio::test(flavor="multi_thread", worker_threads=2)]
async fn positional_and_file_prompts_are_one_shot_and_stdin_stays_available() {
    let f = Fixture::new(); let service = ScriptedAnthropicService::text("fixture answer").await.unwrap();
    let mut args = OPTIONS.to_vec(); args.extend(["positional", "question"]);
    success(&f.run(&args, "", Some(&service)).await);
    let prompt = f.cwd.join("prompt.txt"); fs::write(&prompt, "first line\nsecond line").unwrap();
    let mut args = OPTIONS.to_vec(); args.extend(["--prompt-file", prompt.to_str().unwrap()]);
    success(&f.run(&args, "", Some(&service)).await);
    let bodies = service.request_bodies().await.join("\n");
    assert!(bodies.contains("positional question") && bodies.contains("first line\\nsecond line"));
    let mut args = OPTIONS.to_vec(); args.push("named input");
    failed(&f.run(&args, "conflicting stdin", Some(&service)).await, "choose one prompt source");
    success(&f.run(OPTIONS, "stdin question\n/exit\n", Some(&service)).await);
}

#[tokio::test(flavor="multi_thread", worker_threads=2)]
async fn schema_validates_final_file_and_reports_mismatch_as_failure() {
    let f = Fixture::new(); let schema = f.cwd.join("schema.json"); let output = f.cwd.join("answer.json");
    fs::write(&schema, json!({"type":"object","properties":{"count":{"type":"integer","minimum":1}},"required":["count"],"additionalProperties":false}).to_string()).unwrap();
    let service = ScriptedAnthropicService::text(r#"{"count":2}"#).await.unwrap();
    let args = ["--json","--model","claude-sonnet-4-6","--permission-mode","read-only","--output-schema",schema.to_str().unwrap(),"--last-message",output.to_str().unwrap(),"return data"];
    let result = f.run(&args, "", Some(&service)).await; success(&result);
    assert_eq!(serde_json::from_slice::<Value>(&fs::read(&output).unwrap()).unwrap(), json!({"count":2}));
    let service = ScriptedAnthropicService::text(r#"{"count":"wrong"}"#).await.unwrap();
    fs::remove_file(&output).unwrap();
    let result = f.run(&args, "", Some(&service)).await; failed(&result, "does not match output schema");
    assert!(!output.exists());
    assert!(events(&result).iter().any(|event| event["type"] == "result" && event["status"] == "failed"));
    assert!(!events(&result).iter().any(|event| event["type"] == "result" && event["status"] == "succeeded"));
}

#[tokio::test(flavor="multi_thread", worker_threads=2)]
async fn invalid_and_external_schemas_never_open_a_session() {
    let f = Fixture::new(); let schema = f.cwd.join("schema.json"); let output = f.cwd.join("answer.json");
    for body in [r#"{"type":"bogus"}"#, r#"{"$ref":"file:///tmp/never-load.json"}"#, r#"{"$ref":"https://example.invalid/never-load.json"}"#] {
        fs::write(&schema, body).unwrap();
        let result = f.run(&["--output-schema",schema.to_str().unwrap(),"--last-message",output.to_str().unwrap(),"prompt"], "", None).await;
        failed(&result, "invalid output schema");
    }
    assert!(fs::read_dir(f.store.join("sessions")).unwrap().next().is_none());
}

#[tokio::test(flavor="multi_thread", worker_threads=2)]
async fn exact_final_token_limit_succeeds_and_overrun_or_next_iteration_fails() {
    let f = Fixture::new(); let service = ScriptedAnthropicService::text("fixture answer").await.unwrap();
    let mut args = OPTIONS.to_vec(); args.extend(["--max-output-tokens","8","plain question"]);
    success(&f.run(&args,"",Some(&service)).await);
    args[OPTIONS.len()+1] = "7";
    failed(&f.run(&args,"",Some(&service)).await,"budget exhausted");
    let service = ScriptedAnthropicService::tool_calls(&[("read_file", json!({"path":"fixture.txt"}))], "unreachable final answer").await.unwrap();
    fs::write(f.cwd.join("fixture.txt"),"fixture").unwrap();
    let mut args = OPTIONS.to_vec(); args.extend(["--json","--max-turns","1","read fixture"]);
    let result=f.run(&args,"",Some(&service)).await; failed(&result,"budget exhausted");
    assert_eq!(service.request_bodies().await.len(),1);
    assert!(!diagnostic(&result).contains("unreachable final answer"));
    assert!(events(&result).iter().any(|event|event["type"]=="result" && event["status"]=="failed"));
}

#[tokio::test(flavor="multi_thread", worker_threads=2)]
async fn old_sessions_are_searchable_with_query_preserved_across_pages() {
    let f=Fixture::new();
    for n in 0..260u64 {
        let path=f.seed(&format!("session-{n:02}"),&format!("eligible query {n}"));
        fs::OpenOptions::new().write(true).open(path).unwrap().set_modified(std::time::UNIX_EPOCH+Duration::from_secs(n+1)).unwrap();
    }
    f.seed("blank","   ");
    let result=f.run(&["session","list","--query","query 0","--limit","1","--json"],"",None).await; success(&result);
    let rows:Value=serde_json::from_slice(&result.stdout).unwrap(); assert_eq!(rows[0]["id"],"session-00");
    let result=f.run(&["session","list","--query","eligible","--offset","255","--limit","2","--json"],"",None).await; success(&result);
    let rows:Value=serde_json::from_slice(&result.stdout).unwrap(); assert_eq!(rows.as_array().unwrap().len(),2); assert_eq!(rows[0]["id"],"session-04");
}

#[tokio::test(flavor="multi_thread", worker_threads=2)]
async fn fork_is_independent_and_a_fresh_process_resumes_paired_tools_and_dotted_name() {
    let f=Fixture::new(); let parent=f.seed("parent-session","original question");
    let mut source=Session::load_from_path(&parent).unwrap();
    let mut call=ConversationMessage::user_text(""); call.role=MessageRole::Assistant;
    call.blocks=vec![ContentBlock::ToolUse{id:"read-before-fork".into(),name:"read_file".into(),input:r#"{"path":"fixture.txt"}"#.into()}];
    source.push_message(call).unwrap();
    let mut result=ConversationMessage::user_text(""); result.blocks=vec![ContentBlock::ToolResult{tool_use_id:"read-before-fork".into(),tool_name:"read_file".into(),output:"parent reading".into(),is_error:false,images:vec![]}]; source.push_message(result).unwrap(); source.save_to_path(&parent).unwrap();
    let before=fs::read(&parent).unwrap();
    let fork=f.run(&["session","fork","parent-session","--name","branch.v2"],"",None).await; success(&fork);
    let child=PathBuf::from(String::from_utf8(fork.stdout).unwrap().trim());
    assert_ne!(parent,child); assert_eq!(fs::read(&parent).unwrap(),before);
    let loaded=Session::load_from_path(&child).unwrap(); assert_eq!(loaded.fork.as_ref().unwrap().parent_session_id,"parent-session"); assert_eq!(loaded.messages,source.messages); drop(loaded);
    let renamed=f.run(&["session","rename",child.to_str().unwrap(),"branch.v3"],"",None).await; success(&renamed);
    let service=ScriptedAnthropicService::text("child answer").await.unwrap();
    let mut args=OPTIONS.to_vec(); args.extend(["--resume","branch.v3","continue child"]); success(&f.run(&args,"",Some(&service)).await);
    let requests=service.request_bodies().await.join("\n"); assert!(requests.contains("read-before-fork") && requests.contains("parent reading")); assert_eq!(fs::read(&parent).unwrap(),before);
    let export=f.cwd.join("copy.jsonl"); success(&f.run(&["session","export",child.to_str().unwrap(),export.to_str().unwrap()],"",None).await);
    failed(&f.run(&["session","export",child.to_str().unwrap(),export.to_str().unwrap()],"",None).await,"exists");
    assert_eq!(fs::read(&parent).unwrap(),before);
}

#[tokio::test(flavor="multi_thread", worker_threads=2)]
async fn additional_roots_allow_only_the_named_directory_and_keep_deny_rules() {
    let f=Fixture::new(); let extra=f.root.path().join("extra"); fs::create_dir(&extra).unwrap(); let target=extra.join("allowed.txt"); fs::write(&target,"named root contents").unwrap();
    let service=ScriptedAnthropicService::tool_calls(&[("read_file",json!({"path":target}))],"read complete").await.unwrap();
    let mut args=OPTIONS.to_vec(); args.extend(["--json","--add-dir",extra.to_str().unwrap(),"read outside"]);
    let result=f.run(&args,"",Some(&service)).await; success(&result);
    assert!(events(&result).iter().any(|event|event["type"]=="tool_result" && event["is_error"]==false && event["body"].as_str().unwrap_or("").contains("named root contents")));
    fs::write(f.home.join("settings.json"),json!({"smart":{"autoClassifier":"off","orchestration":"model"},"permissions":{"deny":["read_file(*)"]}}).to_string()).unwrap();
    let service=ScriptedAnthropicService::tool_calls(&[("read_file",json!({"path":target}))],"read refused").await.unwrap();
    let result=f.run(&args,"",Some(&service)).await; success(&result);
    assert!(!events(&result).iter().any(|event|event["type"]=="tool_result" && event["is_error"]==false && event["body"].as_str().unwrap_or("").contains("named root contents")));
}

fn git(cwd:&Path,args:&[&str]) { assert!(std::process::Command::new("git").current_dir(cwd).args(args).status().unwrap().success()); }
#[tokio::test(flavor="multi_thread", worker_threads=2)]
async fn review_rejects_ambiguous_refs_and_does_not_run_fsmonitor_or_edits() {
    let f=Fixture::new(); git(&f.cwd,&["init","-q"]); git(&f.cwd,&["config","user.email","dev@example.test"]); git(&f.cwd,&["config","user.name","dev"]);
    fs::write(f.cwd.join("fixture.txt"),"original").unwrap(); git(&f.cwd,&["add","fixture.txt"]); git(&f.cwd,&["commit","-qm","initial"]);
    git(&f.cwd,&["branch","ambiguous"]); git(&f.cwd,&["tag","ambiguous"]);
    failed(&f.run(&["review","--base","ambiguous"],"",None).await,"ambiguous");
    #[cfg(unix)] {
        use std::os::unix::fs::PermissionsExt;
        let monitor=f.cwd.join("fsmonitor.sh");
        fs::write(&monitor,"#!/bin/sh\nprintf ran > fsmonitor-ran\n").unwrap();
        fs::set_permissions(&monitor,fs::Permissions::from_mode(0o755)).unwrap();
        git(&f.cwd,&["config","core.fsmonitor",monitor.to_str().unwrap()]);
        let filter=f.cwd.join("filter.sh");
        fs::write(&filter,"#!/bin/sh\nprintf ran > clean-filter-ran\ncat\n").unwrap();
        fs::set_permissions(&filter,fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(f.cwd.join(".gitattributes"),"*.txt filter=fixture\n").unwrap();
        git(&f.cwd,&["config","filter.fixture.clean",filter.to_str().unwrap()]);
        git(&f.cwd,&["config","filter.fixture.required","true"]);

    }
    fs::write(f.cwd.join("fixture.txt"),"changed").unwrap();
    let service=ScriptedAnthropicService::tool_calls(&[("write_file",json!({"path":"fixture.txt","content":"forbidden"})),("bash",json!({"command":"touch forbidden.txt"}))],"review findings").await.unwrap();
    let result=f.run(&["review","--uncommitted","--model","claude-sonnet-4-6","--json"],"",Some(&service)).await; success(&result);
    assert_eq!(fs::read_to_string(f.cwd.join("fixture.txt")).unwrap(),"changed"); assert!(!f.cwd.join("forbidden.txt").exists()); assert!(!f.cwd.join("fsmonitor-ran").exists()); assert!(!f.cwd.join("clean-filter-ran").exists());
    let requests=service.request_bodies().await; let request:Value=serde_json::from_str(&requests[0]).unwrap();
    assert!(!request["tools"].as_array().unwrap().iter().any(|tool|tool["name"]=="write_file" || tool["name"]=="Agent"));
}

#[tokio::test(flavor="multi_thread", worker_threads=2)]
async fn discovery_keeps_builtin_precedence_and_never_executes_inline_shell() {
    let f=Fixture::new(); let commands=f.cwd.join(".zo/commands"); fs::create_dir_all(&commands).unwrap();
    fs::write(commands.join("help.md"),"shadow").unwrap(); fs::write(commands.join("custom.md"),"---\ndescription: test discovery\nallowed-tools: Bash\n---\n!touch discovery-ran\ncustom prompt").unwrap();
    let result=f.run(&["commands","--json"],"",None).await; success(&result);
    let rows:Value=serde_json::from_slice(&result.stdout).unwrap(); let rows=rows.as_array().unwrap();
    assert_eq!(rows.iter().filter(|row|row["name"]=="/help").count(),1); assert!(rows.iter().any(|row|row["name"]=="/custom" && row["source"]=="prompt")); assert!(!f.cwd.join("discovery-ran").exists());
    success(&f.run(&["hooks","list","--json"],"",None).await);
    success(&f.run(&["plugin","list","--json"],"",None).await);
    success(&f.run(&["mcp","diagnose","--json"],"",None).await);
    failed(&f.run(&["plugin","enable","missing-plugin"],"",None).await,"not installed");
}

#[cfg(unix)]
#[tokio::test(flavor="multi_thread", worker_threads=2)]
async fn export_refuses_symlinks_and_preserves_the_original_writer_lease() {
    let f=Fixture::new(); let parent=f.seed("leased-session","original");
    let mut owner=Session::load_from_path(&parent).unwrap(); owner.push_user_text("held by this process").unwrap();
    let before=fs::read(&parent).unwrap();
    let link=f.cwd.join("linked.jsonl"); std::os::unix::fs::symlink(&parent,&link).unwrap();
    failed(&f.run(&["session","export",parent.to_str().unwrap(),link.to_str().unwrap()],"",None).await,"exists");
    failed(&f.run(&["session","rename",parent.to_str().unwrap(),"other name"],"",None).await,"writer");
    assert!(core_types::session::writer_lease_holder(&parent).is_some());
    assert_eq!(fs::read(&parent).unwrap(),before);
    success(&f.run(&["session","fork",parent.to_str().unwrap(),"--name","independent"],"",None).await);
    assert!(core_types::session::writer_lease_holder(&parent).is_some());
    assert_eq!(fs::read(&parent).unwrap(),before);
}

#[tokio::test(flavor="multi_thread", worker_threads=2)]
async fn relative_extra_roots_keep_the_resolution_origin_for_nested_and_traversal_paths() {
    let f=Fixture::new(); let nested=f.cwd.join("nested"); fs::create_dir(&nested).unwrap(); fs::write(nested.join("a.txt"),"nested content").unwrap();
    let sibling=f.root.path().join("sibling/deep"); fs::create_dir_all(&sibling).unwrap(); fs::write(sibling.join("b.txt"),"sibling content").unwrap();
    let service=ScriptedAnthropicService::tool_calls(&[("read_file",json!({"path":"nested/a.txt"})),("read_file",json!({"path":"../sibling/deep/b.txt"}))],"read complete").await.unwrap();
    let mut args=OPTIONS.to_vec(); args.extend(["--json","--add-dir",nested.to_str().unwrap(),"--add-dir",sibling.to_str().unwrap(),"read both"]);
    let result=f.run(&args,"",Some(&service)).await; success(&result);
    let results:Vec<_>=events(&result).into_iter().filter(|event|event["type"]=="tool_result").collect();
    assert!(results.iter().any(|event|event["is_error"]==false && event["body"].as_str().unwrap_or("").contains("nested content")),"{}",diagnostic(&result));
    assert!(results.iter().any(|event|event["is_error"]==false && event["body"].as_str().unwrap_or("").contains("sibling content")),"{}",diagnostic(&result));
}

#[tokio::test(flavor="multi_thread", worker_threads=2)]
async fn provider_failure_is_a_failed_result_and_nonzero_exit() {
    use tokio::io::AsyncReadExt;
    let f=Fixture::new(); let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address=listener.local_addr().unwrap();
    let server=tokio::spawn(async move {
        loop { let (mut stream,_)=listener.accept().await.unwrap(); tokio::spawn(async move {
            let mut data=vec![0;65536]; let _=stream.read(&mut data).await;
            let body=r#"{"type":"error","error":{"type":"invalid_request_error","message":"scripted invalid provider request"}}"#;
            let response=format!("HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len());
            let _=stream.write_all(response.as_bytes()).await;
        }); }
    });
    // The endpoint is passed through a local wrapper because the shared fixture
    // deliberately exposes only valid scripted provider responses.
    let mut command=Command::new(env!("CARGO_BIN_EXE_zo"));
    command.env_clear().env("PATH",std::env::var_os("PATH").unwrap_or_default())
        .env("HOME",&f.home).env("ZO_CONFIG_HOME",&f.home).env("ZO_SESSION_ROOT",&f.store)
        .env("ZO_DISABLE_KEYCHAIN","1").env("ZO_DISABLE_EXTERNAL_CREDENTIALS","1").env("ZO_DISABLE_MODEL_DISCOVERY","1")
        .env("ANTHROPIC_API_KEY","test-dummy-key").env("ANTHROPIC_BASE_URL",format!("http://{address}"))
        .current_dir(&f.cwd).args(["--json","--model","claude-sonnet-4-6","--permission-mode","read-only","plain question"])
        .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    let result=tokio::time::timeout(Duration::from_secs(90),command.output()).await.unwrap().unwrap(); server.abort();
    failed(&result,"scripted invalid provider request");
    assert!(events(&result).iter().any(|event|event["type"]=="result" && event["status"]=="failed"));
}

#[cfg(unix)]
#[tokio::test(flavor="multi_thread", worker_threads=2)]
async fn plugin_management_and_discovery_use_enabled_provenance_without_execution() {
    use std::os::unix::fs::PermissionsExt;
    let f=Fixture::new(); let plugin=f.root.path().join("plugin-source"); fs::create_dir(&plugin).unwrap();
    let marker=f.cwd.join("plugin-invoked");
    let quoted=format!("'{}'",marker.to_string_lossy().replace('\'', "'\\''"));
    let script=plugin.join("command.sh"); fs::write(&script,format!("#!/bin/sh\nprintf ran > {quoted}\nprintf plugin-prompt-output\n")).unwrap(); fs::set_permissions(&script,fs::Permissions::from_mode(0o755)).unwrap();
    fs::write(plugin.join("plugin.json"),json!({"name":"fixture-plugin","version":"1.0.0","description":"Fixture command provider","commands":[{"name":"plugin-command","description":"Fixture command","command":"./command.sh"},{"name":"help","description":"Shadow fixture","command":"./command.sh"}]}).to_string()).unwrap();
    let installed=f.run(&["plugin","install",plugin.to_str().unwrap(),"--json"],"",None).await; success(&installed);
    let metadata:Value=serde_json::from_slice(&installed.stdout).unwrap(); let installed=PathBuf::from(metadata["path"].as_str().unwrap()); assert!(installed.is_dir());
    let catalog=f.run(&["commands","--json"],"",None).await; success(&catalog); let rows:Value=serde_json::from_slice(&catalog.stdout).unwrap();
    assert!(rows.as_array().unwrap().iter().any(|row|row["name"]=="/plugin-command" && row["source"]=="plugin" && row["plugin"]=="fixture-plugin@external"));
    assert_eq!(rows.as_array().unwrap().iter().filter(|row|row["name"]=="/help").count(),1); assert!(!marker.exists());
    let service=ScriptedAnthropicService::text("fixture final").await.unwrap();
    success(&f.run(OPTIONS,"/plugin-command argument\n/exit\n",Some(&service)).await);
    assert!(marker.exists()); assert!(service.request_bodies().await.join("\n").contains("plugin-prompt-output"));
    success(&f.run(&["plugin","disable","fixture-plugin@external"],"",None).await);
    let catalog=f.run(&["commands","--json"],"",None).await; success(&catalog); assert!(!String::from_utf8_lossy(&catalog.stdout).contains("/plugin-command"));
    success(&f.run(&["plugin","enable","fixture-plugin@external"],"",None).await);
    success(&f.run(&["plugin","get","fixture-plugin@external","--json"],"",None).await);
}

#[tokio::test(flavor="multi_thread", worker_threads=2)]
async fn worktree_start_uses_the_requested_checkout_and_records_it() {
    let f=Fixture::new(); git(&f.cwd,&["init","-q"]); git(&f.cwd,&["config","user.email","dev@example.test"]); git(&f.cwd,&["config","user.name","dev"]);
    fs::write(f.cwd.join("fixture.txt"),"tracked").unwrap(); git(&f.cwd,&["add","fixture.txt"]); git(&f.cwd,&["commit","-qm","fixture"]);
    let target=f.root.path().join("automation-tree"); let service=ScriptedAnthropicService::text("fixture answer").await.unwrap();
    let mut args=OPTIONS.to_vec(); args.extend(["--worktree",target.to_str().unwrap(),"plain question"]);
    success(&f.run(&args,"",Some(&service)).await);
    assert!(target.join(".git").is_file());
    let result=f.run(&["session","list","--json"],"",None).await; success(&result);
    let rows:Value=serde_json::from_slice(&result.stdout).unwrap(); assert_eq!(rows[0]["cwd"],target.canonicalize().unwrap().to_str().unwrap());
    git(&f.cwd,&["worktree","remove",target.to_str().unwrap()]);
}

#[cfg(unix)]
#[tokio::test(flavor="multi_thread", worker_threads=2)]
async fn extra_root_symlinks_and_canonical_deny_rules_cannot_escape_permissions() {
    let f=Fixture::new(); let extra=f.root.path().join("extra"); let outside=f.root.path().join("outside");
    fs::create_dir(&extra).unwrap(); fs::create_dir(&outside).unwrap(); fs::write(outside.join("secret.txt"),"outside secret").unwrap();
    std::os::unix::fs::symlink(&outside,extra.join("link")).unwrap();
    fs::write(extra.join("denied.txt"),"denied contents").unwrap();
    let denied=extra.join("denied.txt").canonicalize().unwrap();
    fs::write(f.home.join("settings.json"),json!({"smart":{"autoClassifier":"off","orchestration":"model"},"permissions":{"deny":[format!("read_file({})",denied.display())]}}).to_string()).unwrap();
    let service=ScriptedAnthropicService::tool_calls(&[
        ("read_file",json!({"path":"../extra/link/secret.txt"})),
        ("read_file",json!({"path":"../extra/denied.txt"})),
        ("read_file",json!({"path":denied})),
        ("write_file",json!({"path":"../extra/link/new.txt","content":"forbidden"})),
    ],"permissions checked").await.unwrap();
    let mut args=OPTIONS.to_vec(); args.extend(["--json","--add-dir",extra.to_str().unwrap(),"inspect paths"]);
    let result=f.run(&args,"",Some(&service)).await; success(&result);
    let results:Vec<_>=events(&result).into_iter().filter(|event|event["type"]=="tool_result").collect();
    assert_eq!(results.len(),4,"{}",diagnostic(&result));
    assert!(results.iter().all(|event|event["is_error"]==true),"{}",diagnostic(&result));
    assert!(!outside.join("new.txt").exists());
}

#[tokio::test(flavor="multi_thread", worker_threads=2)]
async fn cross_directory_fork_records_the_destination_workspace() {
    let f=Fixture::new(); let source=f.seed("source-other-workspace","original");
    fs::write(source.with_extension("cwd"),json!({"cwd":f.cwd}).to_string()).unwrap();
    let destination=f.root.path().join("destination"); fs::create_dir(&destination).unwrap();
    let result=f.run(&["session","fork",source.to_str().unwrap(),"--cwd",destination.to_str().unwrap()],"",None).await; success(&result);
    let path=PathBuf::from(String::from_utf8(result.stdout).unwrap().trim());
    let metadata:Value=serde_json::from_slice(&fs::read(path.with_extension("cwd")).unwrap()).unwrap();
    assert_eq!(metadata["cwd"],destination.canonicalize().unwrap().to_str().unwrap());
    let service=ScriptedAnthropicService::text("continued in destination").await.unwrap();
    let mut args=OPTIONS.to_vec(); args.extend(["--cwd",destination.to_str().unwrap(),"--resume",path.to_str().unwrap(),"plain question"]);
    success(&f.run(&args,"",Some(&service)).await);
}

#[tokio::test(flavor="multi_thread", worker_threads=2)]
async fn management_help_works_without_valid_configuration_or_a_provider() {
    let f=Fixture::new(); fs::write(f.home.join("settings.json"),"{broken").unwrap();
    for command in ["session","plugin","hooks"] {
        let result=f.run(&[command,"--help"],"",None).await; success(&result);
        assert!(String::from_utf8_lossy(&result.stdout).contains(&format!("zo {command}")));
    }
    assert!(fs::read_dir(f.store.join("sessions")).unwrap().next().is_none());
    assert!(!f.run(&["plugin","enable","missing"],"",None).await.status.success());
}

#[tokio::test(flavor="multi_thread", worker_threads=2)]
async fn persistent_plain_session_reports_last_error_and_recovers_the_next_prompt() {
    use tokio::io::AsyncBufReadExt;
    for recover in [false, true] {
        let f=Fixture::new(); let service=ScriptedAnthropicService::error_then_text("RECOVERED_SECOND_REPLY").await.unwrap();
        let mut args=OPTIONS.to_vec(); args.push("--json");
        let mut child=f.command(&args,Some(&service)).spawn().unwrap();
        let mut input=child.stdin.take().unwrap();
        let mut output=tokio::io::BufReader::new(child.stdout.take().unwrap()).lines();
        input.write_all(b"first persistent prompt\n").await.unwrap();
        tokio::time::timeout(Duration::from_secs(20),async {
            loop {
                let line=output.next_line().await.unwrap().expect("stream closed before first error");
                if line.contains("turn ended:") && line.contains("scripted first-turn failure") { break; }
            }
        }).await.expect("first turn never reported its failure");
        if recover {
            input.write_all(b"second distinct persistent prompt\n").await.unwrap();
            let mut seen=Vec::new();
            let recovered=tokio::time::timeout(Duration::from_secs(10),async {
                loop {
                    let line=output.next_line().await.unwrap().expect("stream closed before recovery");
                    let found=line.contains("RECOVERED_SECOND_REPLY"); seen.push(line);
                    if found { break; }
                }
            }).await;
            assert!(recovered.is_ok(),"second prompt did not recover: {seen:?}");
        }
        // EOF finishes the current turn without cancelling a valid answer.
        drop(input);
        let status=tokio::time::timeout(Duration::from_secs(20),child.wait()).await.unwrap().unwrap();
        assert_eq!(status.success(),recover,"the exit must describe the last completed turn");
        let bodies=service.request_bodies().await;
        assert_eq!(bodies.len(),if recover {2} else {1});
        if recover { assert!(bodies[1].contains("second distinct persistent prompt")); }
    }
}
