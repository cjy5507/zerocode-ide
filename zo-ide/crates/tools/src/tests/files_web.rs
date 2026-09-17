//! File tools, glob/grep, notebooks, skills, and web fetch/search.
//!
//! Split out of the old flat `tests.rs` (4,133 lines) by domain;
//! shared fixtures live in the parent module.

use super::*;

#[test]
fn web_fetch_returns_page_body_with_metadata_header() {
    let _local = allow_local_web();
    let server = TestServer::spawn(Arc::new(|request_line: &str| {
        assert!(request_line.starts_with("GET /page "));
        HttpResponse::html(
            200,
            "OK",
            "<html><head><title>Doc Title</title></head><body><h1>Test Page</h1><p>Hello <b>world</b> from local server.</p><ul><li>alpha</li><li>beta</li></ul></body></html>",
        )
    }));

    // WebFetch now returns the page body as plain-text markdown with a
    // Title/URL/Status header — not a JSON envelope, and no LLM summary.
    let result = run_tool(
        "WebFetch",
        &json!({
            "url": format!("http://{}/page", server.addr()),
            "prompt": "Summarize this page"
        }),
    )
    .expect("WebFetch should succeed");

    assert!(
        serde_json::from_str::<serde_json::Value>(&result).is_err(),
        "result is plain text, not a JSON envelope"
    );
    assert!(result.contains("Title: Doc Title"), "header title: {result}");
    assert!(
        result.contains(&format!("URL: http://{}/page", server.addr())),
        "header url: {result}"
    );
    assert!(result.contains("Status: 200 OK · text/html"), "status: {result}");
    assert!(result.contains("# Test Page"), "h1 → markdown heading: {result}");
    assert!(result.contains("Hello world from local server"));
    assert!(result.contains("- alpha") && result.contains("- beta"), "lists: {result}");

    // The magic "title"/"summary" prompt branches were removed: the title lives
    // in the header for EVERY prompt, so a title-style prompt behaves the same.
    let titled = run_tool(
        "WebFetch",
        &json!({
            "url": format!("http://{}/page", server.addr()),
            "prompt": "What is the page title?"
        }),
    )
    .expect("WebFetch title query should succeed");
    assert!(titled.contains("Title: Doc Title"));
    assert!(titled.contains("# Test Page"), "same body regardless of prompt");
}

#[test]
fn web_fetch_supports_plain_text_and_rejects_invalid_url() {
    let _local = allow_local_web();
    let server = TestServer::spawn(Arc::new(|request_line: &str| {
        assert!(request_line.starts_with("GET /plain "));
        HttpResponse::text(200, "OK", "plain text response")
    }));

    let result = run_tool(
        "WebFetch",
        &json!({
            "url": format!("http://{}/plain", server.addr()),
            "prompt": "Show me the content"
        }),
    )
    .expect("WebFetch should succeed for text content");

    // Non-HTML content: no title line, body returned verbatim under the header.
    assert!(result.contains(&format!("URL: http://{}/plain", server.addr())));
    assert!(result.contains("Status: 200 OK · text/plain"));
    assert!(result.contains("plain text response"));
    assert!(!result.contains("Title:"), "no title for plain text: {result}");

    let error = run_tool(
        "WebFetch",
        &json!({
            "url": "not a url",
            "prompt": "Summarize"
        }),
    )
    .expect_err("invalid URL should fail");
    let msg = error.to_string();
    assert!(msg.contains("relative URL without a base") || msg.contains("invalid"));
}

#[test]
fn web_fetch_caps_response_body_and_stays_recoverable() {
    let _local = allow_local_web();
    // Route the recovery artifact to a temp store so the large-body digest does
    // not write into the repo's `.zo/artifacts` (the env_lock held by
    // `allow_local_web` serializes this against other env-touching tests).
    let artifact_dir = temp_path("webfetch-large-artifacts");
    std::env::set_var("ZO_ARTIFACT_STORE", &artifact_dir);

    let server = TestServer::spawn(Arc::new(|request_line: &str| {
        assert!(request_line.starts_with("GET /large "));
        // A long readable body (well over the 256 KiB read cap and the 30k
        // model-facing cap) so the truncation/artifact seam engages.
        let body = "word ".repeat(120_000);
        HttpResponse::text(200, "OK", &body)
    }));

    let result = run_tool(
        "WebFetch",
        &json!({
            "url": format!("http://{}/large", server.addr()),
            "prompt": "Summarize"
        }),
    )
    .expect("WebFetch should succeed for large content");
    std::env::remove_var("ZO_ARTIFACT_STORE");

    // The read is capped and the model-facing view is the head+tail digest with
    // a recovery handle; the full body is preserved as an artifact on disk.
    assert!(result.chars().count() <= 40_000, "digest is bounded: {} chars", result.chars().count());
    assert!(result.contains("URL: http://"));
    assert!(result.contains("retrieve_tool_output"), "recovery handle present");
    assert!(result.contains("page body middle elided"), "head+tail digest marker");
    assert!(
        std::fs::read_dir(&artifact_dir)
            .map(|mut entries| entries.next().is_some())
            .unwrap_or(false),
        "the full body was persisted as a recoverable artifact"
    );
    let _ = std::fs::remove_dir_all(&artifact_dir);
}

#[test]
fn web_search_extracts_and_filters_results() {
    let server = TestServer::spawn(Arc::new(|request_line: &str| {
        assert!(request_line.contains("GET /search?q=rust+web+search "));
        HttpResponse::html(
            200,
            "OK",
            r#"
                <html><body>
                  <a class="result__a" href="https://docs.rs/reqwest">Reqwest docs</a>
                  <a class="result__a" href="https://example.com/blocked">Blocked result</a>
                </body></html>
                "#,
        )
    }));

    std::env::set_var(
        "ZO_WEB_SEARCH_BASE_URL",
        format!("http://{}/search", server.addr()),
    );
    let result = run_tool(
        "WebSearch",
        &json!({
            "query": "rust web search",
            "allowed_domains": ["https://DOCS.rs/"],
            "blocked_domains": ["HTTPS://EXAMPLE.COM"]
        }),
    )
    .expect("WebSearch should succeed");
    std::env::remove_var("ZO_WEB_SEARCH_BASE_URL");

    let output: serde_json::Value = serde_json::from_str(&result).expect("valid json");
    assert_eq!(output["query"], "rust web search");
    let results = output["results"].as_array().expect("results array");
    let search_result = results
        .iter()
        .find(|item| item.get("content").is_some())
        .expect("search result block present");
    let content = search_result["content"].as_array().expect("content array");
    assert_eq!(content.len(), 1);
    assert_eq!(content[0]["title"], "Reqwest docs");
    assert_eq!(content[0]["url"], "https://docs.rs/reqwest");
}

#[test]
fn web_search_treats_url_query_as_direct_fetch() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    std::env::remove_var("ZO_WEB_SEARCH_BASE_URL");
    // The direct fetch targets the loopback TestServer, so permit local access
    // (the SSRF guard blocks loopback by default). Held under the same env_lock.
    std::env::set_var("ZO_WEB_ALLOW_LOCAL", "1");
    let server = TestServer::spawn(Arc::new(|request_line: &str| {
        assert!(
            request_line.starts_with("GET /direct-page "),
            "URL queries must fetch the target URL directly, not the search backend: {request_line}"
        );
        HttpResponse::html(
            200,
            "OK",
            r"<html><head><title>Direct Product Docs</title></head><body>Official docs</body></html>",
        )
    }));

    let direct_url = format!("http://{}/direct-page", server.addr());
    let result = run_tool(
        "WebSearch",
        &json!({
            "query": direct_url,
        }),
    )
    .expect("WebSearch should fetch URL queries directly");

    let output: serde_json::Value = serde_json::from_str(&result).expect("valid json");
    let results = output["results"].as_array().expect("results array");
    let content = results[1]["content"].as_array().expect("content array");
    assert_eq!(content.len(), 1);
    assert_eq!(content[0]["title"], "Direct Product Docs");
    assert_eq!(content[0]["url"], direct_url);
    assert!(results[0].as_str().unwrap().contains("Direct URL result"));
    std::env::remove_var("ZO_WEB_ALLOW_LOCAL");
}

#[test]
fn web_search_backend_connection_errors_are_actionable_and_compact() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind unused port");
    let addr = listener.local_addr().expect("local addr");
    drop(listener);
    std::env::set_var("ZO_WEB_SEARCH_BASE_URL", format!("http://{addr}/search"));

    let error = run_tool(
        "WebSearch",
        &json!({
            "query": "query that should not be printed inside the backend URL",
        }),
    )
    .expect_err("closed local port should fail immediately");
    std::env::remove_var("ZO_WEB_SEARCH_BASE_URL");

    let message = error.to_string();
    assert!(
        message.contains("web search backend request failed"),
        "operation label should be clear: {message}"
    );
    assert!(
        message.contains("network connection failed") || message.contains("request failed"),
        "failure class should be clear: {message}"
    );
    assert!(
        message.contains("use WebFetch for known URLs"),
        "recovery hint should be present: {message}"
    );
    assert!(
        !message.contains("?q=query+that+should"),
        "backend URL query should not be dumped into the transcript: {message}"
    );
}

#[test]
fn web_search_handles_generic_links_and_invalid_base_url() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let server = TestServer::spawn(Arc::new(|request_line: &str| {
        assert!(request_line.contains("GET /fallback?q=generic+links "));
        HttpResponse::html(
            200,
            "OK",
            r#"
                <html><body>
                  <a href="https://example.com/one">Example One</a>
                  <a href="https://example.com/one">Duplicate Example One</a>
                  <a href="https://docs.rs/tokio">Tokio Docs</a>
                </body></html>
                "#,
        )
    }));

    std::env::set_var(
        "ZO_WEB_SEARCH_BASE_URL",
        format!("http://{}/fallback", server.addr()),
    );
    let result = run_tool(
        "WebSearch",
        &json!({
            "query": "generic links"
        }),
    )
    .expect("WebSearch fallback parsing should succeed");
    std::env::remove_var("ZO_WEB_SEARCH_BASE_URL");

    let output: serde_json::Value = serde_json::from_str(&result).expect("valid json");
    let results = output["results"].as_array().expect("results array");
    let search_result = results
        .iter()
        .find(|item| item.get("content").is_some())
        .expect("search result block present");
    let content = search_result["content"].as_array().expect("content array");
    assert_eq!(content.len(), 2);
    assert_eq!(content[0]["url"], "https://example.com/one");
    assert_eq!(content[1]["url"], "https://docs.rs/tokio");

    std::env::set_var("ZO_WEB_SEARCH_BASE_URL", "://bad-base-url");
    let error = run_tool("WebSearch", &json!({ "query": "generic links" }))
        .expect_err("invalid base URL should fail");
    std::env::remove_var("ZO_WEB_SEARCH_BASE_URL");
    let msg = error.to_string();
    assert!(msg.contains("relative URL without a base") || msg.contains("empty host"));
}

#[test]
fn todo_write_persists_and_returns_previous_state() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let path = temp_path("todos.json");
    std::env::set_var("ZO_TODO_STORE", &path);

    let first = run_tool(
        "TodoWrite",
        &json!({
            "todos": [
                {"stepId": "build", "content": "Add tool", "activeForm": "Adding tool", "status": "in_progress"},
                {"stepId": "test", "content": "Run tests", "activeForm": "Running tests", "status": "pending"}
            ]
        }),
    )
    .expect("TodoWrite should succeed");
    let first_output: serde_json::Value = serde_json::from_str(&first).expect("valid json");
    assert_eq!(first_output["oldTodos"].as_array().expect("array").len(), 0);
    assert_eq!(first_output["newTodos"][0]["stepId"], "build");

    let second = run_tool(
        "TodoWrite",
        &json!({
            "todos": [
                {"stepId": "build", "content": "Add tool", "activeForm": "Adding tool", "status": "completed"},
                {"stepId": "test", "content": "Run tests", "activeForm": "Running tests", "status": "completed"},
                {"stepId": "verify", "content": "Verify", "activeForm": "Verifying", "status": "completed"}
            ]
        }),
    )
    .expect("TodoWrite should succeed");
    std::env::remove_var("ZO_TODO_STORE");
    let _ = std::fs::remove_file(path);

    let second_output: serde_json::Value = serde_json::from_str(&second).expect("valid json");
    assert_eq!(
        second_output["oldTodos"].as_array().expect("array").len(),
        2
    );
    assert_eq!(
        second_output["newTodos"].as_array().expect("array").len(),
        3
    );
    assert_eq!(second_output["oldTodos"][0]["stepId"], "build");
    assert!(second_output["verificationNudgeNeeded"].is_null());
}

/// A plan keeps the order the model wrote it in, finished steps included —
/// the way codex's `update_plan` and Claude Code's todo list do. Sorted by
/// status, a finished step sank to the bottom and every `Updated Plan` cell
/// read as a different plan from the one before it.
#[test]
fn todo_write_keeps_the_models_order() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let path = temp_path("todos-order.json");
    std::env::set_var("ZO_TODO_STORE", &path);

    let ids = |output: &serde_json::Value, key: &str| -> Vec<String> {
        output[key]
            .as_array()
            .expect("array")
            .iter()
            .map(|todo| todo["stepId"].as_str().expect("stepId").to_string())
            .collect()
    };
    let first = run_tool(
        "TodoWrite",
        &json!({
            "todos": [
                {"stepId": "survey", "content": "Survey", "activeForm": "Surveying", "status": "completed"},
                {"stepId": "tokens", "content": "Tokens", "activeForm": "Defining tokens", "status": "in_progress"},
                {"stepId": "render", "content": "Render", "activeForm": "Rendering", "status": "pending"}
            ]
        }),
    )
    .expect("TodoWrite should succeed");
    let first: serde_json::Value = serde_json::from_str(&first).expect("valid json");
    assert_eq!(ids(&first, "newTodos"), ["survey", "tokens", "render"]);

    let second = run_tool(
        "TodoWrite",
        &json!({
            "todos": [
                {"stepId": "survey", "content": "Survey", "activeForm": "Surveying", "status": "completed"},
                {"stepId": "tokens", "content": "Tokens", "activeForm": "Defining tokens", "status": "completed"},
                {"stepId": "render", "content": "Render", "activeForm": "Rendering", "status": "in_progress"}
            ]
        }),
    )
    .expect("TodoWrite should succeed");
    std::env::remove_var("ZO_TODO_STORE");
    let _ = std::fs::remove_file(path);
    let second: serde_json::Value = serde_json::from_str(&second).expect("valid json");
    assert_eq!(
        ids(&second, "oldTodos"),
        ["survey", "tokens", "render"],
        "the store keeps the order too"
    );
    assert_eq!(ids(&second, "newTodos"), ["survey", "tokens", "render"]);
}

#[test]
fn todo_write_rejects_invalid_payloads_and_sets_verification_nudge() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let path = temp_path("todos-errors.json");
    std::env::set_var("ZO_TODO_STORE", &path);

    let empty =
        run_tool("TodoWrite", &json!({ "todos": [] })).expect_err("empty todos should fail");
    assert!(empty.to_string().contains("todos must not be empty"));

    // Multiple in_progress items are now allowed for parallel workflows
    let _multi_active = run_tool(
        "TodoWrite",
        &json!({
            "todos": [
                {"content": "One", "activeForm": "Doing one", "status": "in_progress"},
                {"content": "Two", "activeForm": "Doing two", "status": "in_progress"}
            ]
        }),
    )
    .expect("multiple in-progress todos should succeed");

    let blank_content = run_tool(
        "TodoWrite",
        &json!({
            "todos": [
                {"content": "   ", "activeForm": "Doing it", "status": "pending"}
            ]
        }),
    )
    .expect_err("blank content should fail");
    assert!(blank_content
        .to_string()
        .contains("todo content must not be empty"));

    let nudge = run_tool(
        "TodoWrite",
        &json!({
            "todos": [
                {"content": "Write tests", "activeForm": "Writing tests", "status": "completed"},
                {"content": "Fix errors", "activeForm": "Fixing errors", "status": "completed"},
                {"content": "Ship branch", "activeForm": "Shipping branch", "status": "completed"}
            ]
        }),
    )
    .expect("completed todos should succeed");
    std::env::remove_var("ZO_TODO_STORE");
    let _ = fs::remove_file(path);

    let output: serde_json::Value = serde_json::from_str(&nudge).expect("valid json");
    assert_eq!(output["verificationNudgeNeeded"], true);
}

#[test]
fn skill_loads_local_skill_prompt() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let cwd = temp_path("skills-cwd");
    fs::create_dir_all(&cwd).expect("cwd should exist");
    let original_cwd = std::env::current_dir().expect("cwd");
    std::env::set_current_dir(&cwd).expect("enter temp cwd");
    let skill_dir = cwd.join(".zo").join("skills").join("help");
    fs::create_dir_all(&skill_dir).expect("skill dir should exist");
    fs::write(
        skill_dir.join("SKILL.md"),
        "# help\n\nGuide on using oh-my-codex plugin\n",
    )
    .expect("skill file should exist");

    let result = run_tool_isolated(
        "Skill",
        &json!({
            "skill": "help",
            "args": "overview"
        }),
    )
    .expect("Skill should succeed");

    let output: serde_json::Value = serde_json::from_str(&result).expect("valid json");
    assert_eq!(output["skill"], "help");
    assert!(output["path"]
        .as_str()
        .expect("path")
        .ends_with("/help/SKILL.md"));
    assert!(output["prompt"]
        .as_str()
        .expect("prompt")
        .contains("Guide on using oh-my-codex plugin"));

    let dollar_result = run_tool_isolated(
        "Skill",
        &json!({
            "skill": "$help"
        }),
    )
    .expect("Skill should accept $skill invocation form");
    let dollar_output: serde_json::Value =
        serde_json::from_str(&dollar_result).expect("valid json");
    assert_eq!(dollar_output["skill"], "$help");
    assert!(dollar_output["path"]
        .as_str()
        .expect("path")
        .ends_with("/help/SKILL.md"));

    std::env::set_current_dir(original_cwd).expect("restore cwd");
    cleanup_temp_cwd(&cwd, "temp cwd should clean up");
}

#[test]
fn skill_loads_zo_global_but_not_non_zo_global_prompt() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let cwd = temp_path("skills-global-cwd");
    let root = temp_path("skills-global-home");
    let zo_global_skill = root.join("zo-global").join("skills").join("global-help");
    let non_zo_skill = root
        .join("home")
        .join(".other-tool")
        .join("skills")
        .join("omc-reference");
    fs::create_dir_all(&cwd).expect("cwd should exist");
    fs::create_dir_all(&zo_global_skill).expect("zo global skill dir should exist");
    fs::create_dir_all(&non_zo_skill).expect("non-zo skill dir should exist");
    fs::write(
        zo_global_skill.join("SKILL.md"),
        "---\nname: global-help\ndescription: Zo global guidance\n---\n\nGLOBAL BODY\n",
    )
    .expect("zo global skill file should exist");
    fs::write(
        non_zo_skill.join("SKILL.md"),
        "---\nname: omc-reference\ndescription: Non-Zo guidance\n---\n\nOMC BODY\n",
    )
    .expect("non-zo skill file should exist");

    let original_cwd = std::env::current_dir().expect("cwd");
    let original_home = std::env::var("HOME").ok();
    let original_zo_config_home = std::env::var("ZO_CONFIG_HOME").ok();
    let original_zo_home = std::env::var("ZO_HOME").ok();
    std::env::set_current_dir(&cwd).expect("enter temp cwd");
    std::env::set_var("HOME", root.join("home"));
    std::env::set_var("ZO_CONFIG_HOME", root.join("zo-global"));
    std::env::set_var("ZO_HOME", root.join("missing-zo-home"));

    let result = run_tool_isolated("Skill", &json!({ "skill": "global-help" }))
        .expect("Zo global Skill should succeed");
    let output: serde_json::Value = serde_json::from_str(&result).expect("valid json");
    assert!(output["path"]
        .as_str()
        .expect("path")
        .contains("/zo-global/skills/global-help/SKILL.md"));
    assert!(output["prompt"]
        .as_str()
        .expect("prompt")
        .contains("GLOBAL BODY"));

    let error = run_tool_isolated("Skill", &json!({ "skill": "omc-reference" }))
        .expect_err("non-Zo global skills must not load");
    assert!(matches!(
        error,
        ToolError::NotFound(message) if message.contains("omc-reference")
    ));

    std::env::set_current_dir(original_cwd).expect("restore cwd");
    match original_home {
        Some(value) => std::env::set_var("HOME", value),
        None => std::env::remove_var("HOME"),
    }
    match original_zo_config_home {
        Some(value) => std::env::set_var("ZO_CONFIG_HOME", value),
        None => std::env::remove_var("ZO_CONFIG_HOME"),
    }
    match original_zo_home {
        Some(value) => std::env::set_var("ZO_HOME", value),
        None => std::env::remove_var("ZO_HOME"),
    }
    cleanup_temp_cwd(&cwd, "temp cwd should clean up");
    fs::remove_dir_all(root).expect("temp root should clean up");
}

/// Tear down a temp dir that served as the process-wide cwd. Other test
/// threads write cwd-relative paths (dispatch's artifact offload lands
/// `.zo/artifacts/<sha>` in whatever cwd is current at that instant), so a
/// plain `remove_dir_all` can race a concurrent write between readdir and
/// rmdir (`DirectoryNotEmpty`). Retrying absorbs the transient write; a
/// persistent failure still panics so real leaks stay visible.
fn cleanup_temp_cwd(path: &std::path::Path, what: &str) {
    for _ in 0..5 {
        match fs::remove_dir_all(path) {
            Ok(()) => return,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
            Err(_) => std::thread::sleep(std::time::Duration::from_millis(25)),
        }
    }
    fs::remove_dir_all(path).unwrap_or_else(|error| panic!("{what}: {error:?}"));
}

#[test]
fn skill_rejects_proposed_skill_prompt() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let cwd = temp_path("proposed-skills-cwd");
    fs::create_dir_all(&cwd).expect("cwd should exist");
    let original_cwd = std::env::current_dir().expect("cwd");
    std::env::set_current_dir(&cwd).expect("enter temp cwd");
    let skill_dir = cwd.join(".zo").join("skills").join("draft");
    fs::create_dir_all(&skill_dir).expect("skill dir should exist");
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: draft\nstate: proposed\ndescription: Not ready\n---\n\nDraft body\n",
    )
    .expect("skill file should exist");

    let error = run_tool_isolated(
        "Skill",
        &json!({
            "skill": "draft"
        }),
    )
    .expect_err("proposed skills must not load");

    assert!(matches!(
        error,
        ToolError::InvalidInput(message)
            if message.contains("proposed") && message.contains("approved")
    ));

    std::env::set_current_dir(original_cwd).expect("restore cwd");
    cleanup_temp_cwd(&cwd, "temp cwd should clean up");
}

#[test]
fn skill_distill_writes_proposed_skill_without_auto_loading_it() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let cwd = temp_path("skill-distill");
    fs::create_dir_all(&cwd).expect("cwd should exist");
    let original_cwd = std::env::current_dir().expect("cwd");
    std::env::set_current_dir(&cwd).expect("enter temp cwd");

    let result = run_tool_isolated(
        "SkillDistill",
        &json!({
            "slug": "review-loop",
            "description": "Capture the review loop",
            "body": "1. Inspect the diff.\n2. Run focused tests.",
            "name": "review-loop"
        }),
    )
    .expect("SkillDistill should write a proposed draft");
    let output: serde_json::Value = serde_json::from_str(&result).expect("json output");
    assert_eq!(output["slug"], "review-loop");
    assert_eq!(output["state"], "proposed");

    let skill_path = cwd
        .join(".zo")
        .join("skills")
        .join("review-loop")
        .join("SKILL.md");
    let contents = fs::read_to_string(&skill_path).expect("skill draft should exist");
    assert!(contents.contains("state: proposed"));
    assert!(contents.contains("Capture the review loop"));
    assert!(contents.contains("Run focused tests."));

    let error = run_tool_isolated("Skill", &json!({ "skill": "review-loop" }))
        .expect_err("proposed skill must not auto-load");
    assert!(matches!(
        error,
        ToolError::InvalidInput(message)
            if message.contains("proposed") && message.contains("approved")
    ));

    std::env::set_current_dir(original_cwd).expect("restore cwd");
    cleanup_temp_cwd(&cwd, "cleanup temp cwd");
}

#[test]
fn skill_distill_rejects_existing_skill_path() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let cwd = temp_path("skill-distill-existing");
    let skill_dir = cwd.join(".zo").join("skills").join("review-loop");
    fs::create_dir_all(&skill_dir).expect("skill dir should exist");
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: review-loop\nstate: proposed\n---\n\nExisting draft\n",
    )
    .expect("existing skill should exist");
    let original_cwd = std::env::current_dir().expect("cwd");
    std::env::set_current_dir(&cwd).expect("enter temp cwd");

    let error = run_tool_isolated(
        "SkillDistill",
        &json!({
            "slug": "review-loop",
            "description": "Capture the review loop",
            "body": "Replacement"
        }),
    )
    .expect_err("SkillDistill should not overwrite existing drafts");
    assert!(matches!(
        error,
        ToolError::InvalidInput(message) if message.contains("already exists")
    ));

    std::env::set_current_dir(original_cwd).expect("restore cwd");
    cleanup_temp_cwd(&cwd, "cleanup temp cwd");
}

#[test]
fn skill_distill_rejects_lexically_duplicate_skill() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let cwd = temp_path("skill-distill-duplicate");
    let skill_dir = cwd.join(".zo").join("skills").join("existing-review");
    fs::create_dir_all(&skill_dir).expect("skill dir should exist");
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: existing-review\ndescription: Inspect pull request diff and run focused tests\n---\n\n1. Inspect the pull request diff.\n2. Run focused tests.\n3. Summarize findings.\n",
    )
    .expect("existing skill should exist");
    let original_cwd = std::env::current_dir().expect("cwd");
    std::env::set_current_dir(&cwd).expect("enter temp cwd");

    let error = run_tool_isolated(
        "SkillDistill",
        &json!({
            "slug": "code-review-flow",
            "description": "Inspect pull request diff and run focused tests",
            "body": "Inspect the pull request diff, run focused tests, then summarize findings."
        }),
    )
    .expect_err("SkillDistill should reject duplicate skill content");
    assert!(matches!(
        error,
        ToolError::InvalidInput(message)
            if message.contains("similar skill already exists")
                && message.contains("existing-review")
    ));

    std::env::set_current_dir(original_cwd).expect("restore cwd");
    cleanup_temp_cwd(&cwd, "cleanup temp cwd");
}

#[test]
fn skill_distill_new_draft_starts_at_version_one() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let cwd = temp_path("skill-distill-v1");
    fs::create_dir_all(&cwd).expect("cwd should exist");
    let original_cwd = std::env::current_dir().expect("cwd");
    std::env::set_current_dir(&cwd).expect("enter temp cwd");

    let result = run_tool_isolated(
        "SkillDistill",
        &json!({
            "slug": "deploy-flow",
            "description": "Capture the deploy flow",
            "body": "1. Build.\n2. Ship."
        }),
    )
    .expect("new draft should write");
    let output: serde_json::Value = serde_json::from_str(&result).expect("json output");
    assert_eq!(output["version"], 1);

    let contents = fs::read_to_string(
        cwd.join(".zo")
            .join("skills")
            .join("deploy-flow")
            .join("SKILL.md"),
    )
    .expect("draft exists");
    assert!(contents.contains("version: 1"));

    std::env::set_current_dir(original_cwd).expect("restore cwd");
    cleanup_temp_cwd(&cwd, "cleanup temp cwd");
}

#[test]
fn skill_distill_update_bumps_version_and_rewrites_body() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let cwd = temp_path("skill-distill-evolve");
    let skill_dir = cwd.join(".zo").join("skills").join("review-loop");
    fs::create_dir_all(&skill_dir).expect("skill dir should exist");
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: review-loop\ndescription: old\nversion: 2\nstate: proposed\n---\n\nOld body\n",
    )
    .expect("existing draft");
    let original_cwd = std::env::current_dir().expect("cwd");
    std::env::set_current_dir(&cwd).expect("enter temp cwd");

    // Without `update` the existing draft is protected; the error points at it.
    let blocked = run_tool_isolated(
        "SkillDistill",
        &json!({ "slug": "review-loop", "description": "new", "body": "New body" }),
    )
    .expect_err("must refuse to overwrite without update");
    assert!(matches!(
        blocked,
        ToolError::InvalidInput(message) if message.contains("update: true")
    ));

    // With `update` the version bumps (2 → 3), the body is rewritten, and the
    // evolved draft stays proposed for re-review.
    let result = run_tool_isolated(
        "SkillDistill",
        &json!({
            "slug": "review-loop",
            "description": "Capture the evolved review loop",
            "body": "1. Inspect the diff.\n2. Run focused tests.\n3. Summarize.",
            "update": true
        }),
    )
    .expect("re-distill should update");
    let output: serde_json::Value = serde_json::from_str(&result).expect("json output");
    assert_eq!(output["version"], 3);
    assert_eq!(output["state"], "proposed");

    let contents = fs::read_to_string(skill_dir.join("SKILL.md")).expect("evolved draft");
    assert!(contents.contains("version: 3"));
    assert!(contents.contains("Summarize."));
    assert!(!contents.contains("Old body"), "body should be rewritten");
    assert!(contents.contains("state: proposed"));

    std::env::set_current_dir(original_cwd).expect("restore cwd");
    cleanup_temp_cwd(&cwd, "cleanup temp cwd");
}

#[test]
fn skill_review_approves_proposed_skill_for_loading() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let cwd = temp_path("skill-review-approve");
    let skill_dir = cwd.join(".zo").join("skills").join("review-loop");
    fs::create_dir_all(&skill_dir).expect("skill dir should exist");
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: review-loop\ndescription: Review loop\nstate: proposed\n---\n\nReview body\n",
    )
    .expect("proposed skill should exist");
    let original_cwd = std::env::current_dir().expect("cwd");
    std::env::set_current_dir(&cwd).expect("enter temp cwd");

    let result = run_tool_isolated(
        "SkillReview",
        &json!({
            "slug": "review-loop",
            "action": "approve"
        }),
    )
    .expect("SkillReview should approve proposed skill");
    let output: serde_json::Value = serde_json::from_str(&result).expect("json output");
    assert_eq!(output["state"], "active");

    let contents = fs::read_to_string(skill_dir.join("SKILL.md")).expect("approved skill");
    assert!(contents.contains("state: active"));
    assert!(!contents.contains("state: proposed"));

    let loaded = run_tool_isolated("Skill", &json!({ "skill": "review-loop" }))
        .expect("approved skill should load");
    let loaded: serde_json::Value = serde_json::from_str(&loaded).expect("json output");
    assert!(loaded["prompt"]
        .as_str()
        .expect("prompt")
        .contains("Review body"));

    std::env::set_current_dir(original_cwd).expect("restore cwd");
    cleanup_temp_cwd(&cwd, "cleanup temp cwd");
}

#[test]
fn skill_review_discards_proposed_skill_cleanly() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let cwd = temp_path("skill-review-discard");
    let skill_dir = cwd.join(".zo").join("skills").join("review-loop");
    fs::create_dir_all(&skill_dir).expect("skill dir should exist");
    let skill_path = skill_dir.join("SKILL.md");
    fs::write(
        &skill_path,
        "---\nname: review-loop\ndescription: Review loop\nstate: proposed\n---\n\nReview body\n",
    )
    .expect("proposed skill should exist");
    let original_cwd = std::env::current_dir().expect("cwd");
    std::env::set_current_dir(&cwd).expect("enter temp cwd");

    let result = run_tool_isolated(
        "SkillReview",
        &json!({
            "slug": "review-loop",
            "action": "discard"
        }),
    )
    .expect("SkillReview should discard proposed skill");
    let output: serde_json::Value = serde_json::from_str(&result).expect("json output");
    assert_eq!(output["state"], "discarded");
    assert!(!skill_path.exists());
    assert!(!skill_dir.exists());

    let error = run_tool_isolated("Skill", &json!({ "skill": "review-loop" }))
        .expect_err("discarded skill should not load");
    assert!(matches!(error, ToolError::NotFound(_)));

    std::env::set_current_dir(original_cwd).expect("restore cwd");
    cleanup_temp_cwd(&cwd, "cleanup temp cwd");
}

// --- Provider skill catalog (enabled Claude/Codex bundled skills) ---

/// RAII env var override restored on drop, so a provider-catalog test never
/// leaks a home override into the shared test process (and never inspects the
/// developer's real `~/.claude` / `~/.codex`).
struct EnvVarGuard {
    key: &'static str,
    previous: Option<std::ffi::OsString>,
}

impl EnvVarGuard {
    fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
        let previous = std::env::var_os(key);
        std::env::set_var(key, value);
        Self { key, previous }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        match &self.previous {
            Some(value) => std::env::set_var(self.key, value),
            None => std::env::remove_var(self.key),
        }
    }
}

/// Point every home-resolution env var at hermetic temp roots so the catalog
/// only ever sees the fixture, never the developer's real home. Returns the
/// guards (drop-restores) plus the two provider home roots.
fn isolated_provider_homes(root: &std::path::Path) -> (Vec<EnvVarGuard>, PathBuf, PathBuf) {
    let claude_home = root.join("claude-home");
    let codex_home = root.join("codex-home");
    let guards = vec![
        EnvVarGuard::set("HOME", root.join("home")),
        EnvVarGuard::set("ZO_CONFIG_HOME", root.join("zo-config-missing")),
        EnvVarGuard::set("ZO_HOME", root.join("zo-home-missing")),
        EnvVarGuard::set("ZO_CLAUDE_HOME", &claude_home),
        EnvVarGuard::set("ZO_CODEX_HOME", &codex_home),
        // Borrowed provider catalogs are off by default
        // (`runtime::skill_sources::PROVIDER_SKILLS_ENV`). Every caller of this
        // helper exists to exercise that path, so turning it on belongs here
        // rather than repeated in each test — and a test that forgets it would
        // read as "the provider skill was rejected" when it was never scanned.
        EnvVarGuard::set(runtime::skill_sources::PROVIDER_SKILLS_ENV, "1"),
    ];
    (guards, claude_home, codex_home)
}

fn write_skill_file(dir: &std::path::Path, name: &str, body: &str) {
    fs::create_dir_all(dir).expect("skill dir should exist");
    fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: {name} guidance\n---\n\n{body}\n"),
    )
    .expect("skill file should exist");
}

#[test]
fn skill_catalog_discovers_enabled_claude_plugin_skill() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let root = temp_path("skill-catalog-claude");
    let cwd = root.join("project");
    fs::create_dir_all(&cwd).expect("cwd should exist");
    let (_env, claude_home, _codex_home) = isolated_provider_homes(&root);

    let install_path = claude_home.join("cache").join("acme");
    fs::create_dir_all(claude_home.join("plugins")).expect("plugins dir");
    fs::write(
        claude_home.join("settings.json"),
        r#"{"enabledPlugins": {"acme@marketplace": true}}"#,
    )
    .expect("settings.json");
    fs::write(
        claude_home.join("plugins").join("installed_plugins.json"),
        format!(
            r#"{{"acme@marketplace": {{"installPath": "{}"}}}}"#,
            install_path.display()
        ),
    )
    .expect("installed_plugins.json");
    write_skill_file(
        &install_path.join("skills").join("claude-helper"),
        "claude-helper",
        "CLAUDE PLUGIN BODY",
    );

    let original_cwd = std::env::current_dir().expect("cwd");
    std::env::set_current_dir(&cwd).expect("enter temp cwd");

    let result = run_tool_isolated("Skill", &json!({ "skill": "claude-helper" }))
        .expect("enabled Claude plugin skill should load");
    let output: serde_json::Value = serde_json::from_str(&result).expect("valid json");
    assert!(output["path"]
        .as_str()
        .expect("path")
        .ends_with("/claude-helper/SKILL.md"));
    assert!(output["prompt"]
        .as_str()
        .expect("prompt")
        .contains("CLAUDE PLUGIN BODY"));
    assert!(output["origin"]
        .as_str()
        .expect("origin field must be present")
        .contains("claude"));

    std::env::set_current_dir(original_cwd).expect("restore cwd");
    cleanup_temp_cwd(&root, "temp root should clean up");
}

#[test]
fn skill_catalog_discovers_enabled_codex_plugin_skill() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let root = temp_path("skill-catalog-codex");
    let cwd = root.join("project");
    fs::create_dir_all(&cwd).expect("cwd should exist");
    let (_env, _claude_home, codex_home) = isolated_provider_homes(&root);

    fs::create_dir_all(&codex_home).expect("codex home");
    fs::write(
        codex_home.join("config.toml"),
        "[plugins.\"beta\"]\nenabled = true\n",
    )
    .expect("config.toml");
    write_skill_file(
        &codex_home
            .join("plugins")
            .join("beta")
            .join("skills")
            .join("codex-helper"),
        "codex-helper",
        "CODEX PLUGIN BODY",
    );

    let original_cwd = std::env::current_dir().expect("cwd");
    std::env::set_current_dir(&cwd).expect("enter temp cwd");

    let result = run_tool_isolated("Skill", &json!({ "skill": "codex-helper" }))
        .expect("enabled Codex plugin skill should load");
    let output: serde_json::Value = serde_json::from_str(&result).expect("valid json");
    assert!(output["prompt"]
        .as_str()
        .expect("prompt")
        .contains("CODEX PLUGIN BODY"));
    assert!(output["origin"]
        .as_str()
        .expect("origin field must be present")
        .contains("codex"));

    std::env::set_current_dir(original_cwd).expect("restore cwd");
    cleanup_temp_cwd(&root, "temp root should clean up");
}

#[test]
fn skill_catalog_hides_disabled_or_cache_only_provider_skill() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let root = temp_path("skill-catalog-disabled");
    let cwd = root.join("project");
    fs::create_dir_all(&cwd).expect("cwd should exist");
    let (_env, claude_home, _codex_home) = isolated_provider_homes(&root);

    // One enabled plugin (its skill must load) and one cache-only plugin that is
    // NOT enabled in settings.json (its skill must stay invisible). Discovering
    // the enabled one is what fails on HEAD, so this test is red pre-feature.
    let live_install = claude_home.join("cache").join("live");
    let ghost_install = claude_home.join("cache").join("ghost");
    fs::create_dir_all(claude_home.join("plugins")).expect("plugins dir");
    fs::write(
        claude_home.join("settings.json"),
        r#"{"enabledPlugins": {"live@mkt": true, "ghost@mkt": false}}"#,
    )
    .expect("settings.json");
    fs::write(
        claude_home.join("plugins").join("installed_plugins.json"),
        format!(
            r#"{{"live@mkt": {{"installPath": "{}"}}, "ghost@mkt": {{"installPath": "{}"}}}}"#,
            live_install.display(),
            ghost_install.display()
        ),
    )
    .expect("installed_plugins.json");
    write_skill_file(
        &live_install.join("skills").join("live-skill"),
        "live-skill",
        "LIVE BODY",
    );
    write_skill_file(
        &ghost_install.join("skills").join("ghost-skill"),
        "ghost-skill",
        "GHOST BODY",
    );

    let original_cwd = std::env::current_dir().expect("cwd");
    std::env::set_current_dir(&cwd).expect("enter temp cwd");

    run_tool_isolated("Skill", &json!({ "skill": "live-skill" }))
        .expect("enabled plugin skill should load");
    let ghost = run_tool_isolated("Skill", &json!({ "skill": "ghost-skill" }))
        .expect_err("disabled/cache-only plugin skill must not load");
    assert!(matches!(
        ghost,
        ToolError::NotFound(message) if message.contains("ghost-skill")
    ));

    std::env::set_current_dir(original_cwd).expect("restore cwd");
    cleanup_temp_cwd(&root, "temp root should clean up");
}

#[cfg(unix)]
#[test]
fn skill_catalog_rejects_symlink_escape_from_provider_root() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let root = temp_path("skill-catalog-escape");
    let cwd = root.join("project");
    fs::create_dir_all(&cwd).expect("cwd should exist");
    let (_env, claude_home, _codex_home) = isolated_provider_homes(&root);

    let install_path = claude_home.join("cache").join("acme");
    let skills_root = install_path.join("skills");
    fs::create_dir_all(claude_home.join("plugins")).expect("plugins dir");
    fs::write(
        claude_home.join("settings.json"),
        r#"{"enabledPlugins": {"acme@mkt": true}}"#,
    )
    .expect("settings.json");
    fs::write(
        claude_home.join("plugins").join("installed_plugins.json"),
        format!(
            r#"{{"acme@mkt": {{"installPath": "{}"}}}}"#,
            install_path.display()
        ),
    )
    .expect("installed_plugins.json");
    // A legit sibling that must load (this is what fails on HEAD → red).
    write_skill_file(&skills_root.join("safe-skill"), "safe-skill", "SAFE BODY");

    // A skill directory that is a symlink escaping the trusted plugin root.
    let outside = root.join("outside");
    write_skill_file(&outside.join("escape"), "escape", "SECRET BODY");
    std::os::unix::fs::symlink(outside.join("escape"), skills_root.join("escape"))
        .expect("create escaping symlink");

    let original_cwd = std::env::current_dir().expect("cwd");
    std::env::set_current_dir(&cwd).expect("enter temp cwd");

    run_tool_isolated("Skill", &json!({ "skill": "safe-skill" }))
        .expect("in-tree plugin skill should load");
    let escaped = run_tool_isolated("Skill", &json!({ "skill": "escape" }))
        .expect_err("symlink escaping the provider root must be rejected");
    assert!(matches!(escaped, ToolError::NotFound(_)));

    std::env::set_current_dir(original_cwd).expect("restore cwd");
    cleanup_temp_cwd(&root, "temp root should clean up");
}

#[test]
fn skill_catalog_zo_skill_wins_provider_name_collision() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let root = temp_path("skill-catalog-collision");
    let cwd = root.join("project");
    fs::create_dir_all(&cwd).expect("cwd should exist");
    let (_env, claude_home, _codex_home) = isolated_provider_homes(&root);

    // Same skill name in the project's Zo root and an enabled Claude plugin.
    write_skill_file(
        &cwd.join(".zo").join("skills").join("shared-helper"),
        "shared-helper",
        "ZO BODY",
    );
    let install_path = claude_home.join("cache").join("acme");
    fs::create_dir_all(claude_home.join("plugins")).expect("plugins dir");
    fs::write(
        claude_home.join("settings.json"),
        r#"{"enabledPlugins": {"acme@mkt": true}}"#,
    )
    .expect("settings.json");
    fs::write(
        claude_home.join("plugins").join("installed_plugins.json"),
        format!(
            r#"{{"acme@mkt": {{"installPath": "{}"}}}}"#,
            install_path.display()
        ),
    )
    .expect("installed_plugins.json");
    write_skill_file(
        &install_path.join("skills").join("shared-helper"),
        "shared-helper",
        "CLAUDE BODY",
    );

    let original_cwd = std::env::current_dir().expect("cwd");
    std::env::set_current_dir(&cwd).expect("enter temp cwd");

    let result = run_tool_isolated("Skill", &json!({ "skill": "shared-helper" }))
        .expect("Zo skill should win the collision");
    let output: serde_json::Value = serde_json::from_str(&result).expect("valid json");
    assert_eq!(
        output["origin"].as_str().expect("origin field must be present"),
        "project-zo",
        "project Zo skill must win over an enabled provider skill of the same name"
    );
    assert!(output["prompt"].as_str().expect("prompt").contains("ZO BODY"));

    std::env::set_current_dir(original_cwd).expect("restore cwd");
    cleanup_temp_cwd(&root, "temp root should clean up");
}

#[test]
fn notebook_edit_replaces_inserts_and_deletes_cells() {
    let path = temp_path("notebook.ipynb");
    std::fs::write(
            &path,
            r#"{
  "cells": [
    {"cell_type": "code", "id": "cell-a", "metadata": {}, "source": ["print(1)\n"], "outputs": [], "execution_count": null}
  ],
  "metadata": {"kernelspec": {"language": "python"}},
  "nbformat": 4,
  "nbformat_minor": 5
}"#,
        )
        .expect("write notebook");

    let replaced = run_tool(
        "NotebookEdit",
        &json!({
            "notebook_path": path.display().to_string(),
            "cell_id": "cell-a",
            "new_source": "print(2)\n",
            "edit_mode": "replace"
        }),
    )
    .expect("NotebookEdit replace should succeed");
    let replaced_output: serde_json::Value = serde_json::from_str(&replaced).expect("json");
    assert_eq!(replaced_output["cell_id"], "cell-a");
    assert_eq!(replaced_output["cell_type"], "code");

    let inserted = run_tool(
        "NotebookEdit",
        &json!({
            "notebook_path": path.display().to_string(),
            "cell_id": "cell-a",
            "new_source": "# heading\n",
            "cell_type": "markdown",
            "edit_mode": "insert"
        }),
    )
    .expect("NotebookEdit insert should succeed");
    let inserted_output: serde_json::Value = serde_json::from_str(&inserted).expect("json");
    assert_eq!(inserted_output["cell_type"], "markdown");
    let appended = run_tool(
        "NotebookEdit",
        &json!({
            "notebook_path": path.display().to_string(),
            "new_source": "print(3)\n",
            "edit_mode": "insert"
        }),
    )
    .expect("NotebookEdit append should succeed");
    let appended_output: serde_json::Value = serde_json::from_str(&appended).expect("json");
    assert_eq!(appended_output["cell_type"], "code");

    let deleted = run_tool(
        "NotebookEdit",
        &json!({
            "notebook_path": path.display().to_string(),
            "cell_id": "cell-a",
            "edit_mode": "delete"
        }),
    )
    .expect("NotebookEdit delete should succeed without new_source");
    let deleted_output: serde_json::Value = serde_json::from_str(&deleted).expect("json");
    assert!(deleted_output["cell_type"].is_null());
    assert_eq!(deleted_output["new_source"], "");

    let final_notebook: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("read notebook"))
            .expect("valid notebook json");
    let cells = final_notebook["cells"].as_array().expect("cells array");
    assert_eq!(cells.len(), 2);
    assert_eq!(cells[0]["cell_type"], "markdown");
    assert!(cells[0].get("outputs").is_none());
    assert_eq!(cells[1]["cell_type"], "code");
    assert_eq!(cells[1]["source"][0], "print(3)\n");
    let _ = std::fs::remove_file(path);
}

#[test]
fn notebook_edit_rejects_invalid_inputs() {
    let text_path = temp_path("notebook.txt");
    fs::write(&text_path, "not a notebook").expect("write text file");
    let wrong_extension = run_tool(
        "NotebookEdit",
        &json!({
            "notebook_path": text_path.display().to_string(),
            "new_source": "print(1)\n"
        }),
    )
    .expect_err("non-ipynb file should fail");
    assert!(wrong_extension.to_string().contains("Jupyter notebook"));
    let _ = fs::remove_file(&text_path);

    let empty_notebook = temp_path("empty.ipynb");
    fs::write(
            &empty_notebook,
            r#"{"cells":[],"metadata":{"kernelspec":{"language":"python"}},"nbformat":4,"nbformat_minor":5}"#,
        )
        .expect("write empty notebook");

    let missing_source = run_tool(
        "NotebookEdit",
        &json!({
            "notebook_path": empty_notebook.display().to_string(),
            "edit_mode": "insert"
        }),
    )
    .expect_err("insert without source should fail");
    assert!(missing_source
        .to_string()
        .contains("new_source is required"));

    let missing_cell = run_tool(
        "NotebookEdit",
        &json!({
            "notebook_path": empty_notebook.display().to_string(),
            "edit_mode": "delete"
        }),
    )
    .expect_err("delete on empty notebook should fail");
    assert!(missing_cell
        .to_string()
        .contains("Notebook has no cells to edit"));
    let _ = fs::remove_file(empty_notebook);
}

#[test]
fn file_tools_cover_read_write_and_edit_behaviors() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let root = temp_path("fs-suite");
    fs::create_dir_all(&root).expect("create root");
    let original_dir = std::env::current_dir().expect("cwd");
    std::env::set_current_dir(&root).expect("set cwd");

    let write_create = run_tool(
        "write_file",
        &json!({ "path": "nested/demo.txt", "content": "alpha\nbeta\nalpha\n" }),
    )
    .expect("write create should succeed");
    let write_create_output: serde_json::Value = serde_json::from_str(&write_create).expect("json");
    assert_eq!(write_create_output["type"], "create");
    assert!(root.join("nested/demo.txt").exists());

    let write_update = run_tool(
        "write_file",
        &json!({ "path": "nested/demo.txt", "content": "alpha\nbeta\ngamma\n" }),
    )
    .expect("write update should succeed");
    let write_update_output: serde_json::Value = serde_json::from_str(&write_update).expect("json");
    assert_eq!(write_update_output["type"], "update");
    // `original_file` is deliberately `#[serde(skip)]` (emitting it would re-send the
    // whole pre-write file to the model on every write and re-bill as cache_read).
    // The proof that the prior content was read is the emitted `structuredPatch`,
    // which diffs the original (`alpha\nbeta\nalpha\n`) against the new content.
    assert_eq!(
        write_update_output["structuredPatch"][0]["lines"],
        json!([" alpha", " beta", "-alpha", "+gamma"])
    );

    let read_full = run_tool("read_file", &json!({ "path": "nested/demo.txt" }))
        .expect("read full should succeed");
    let read_full_output: serde_json::Value = serde_json::from_str(&read_full).expect("json");
    assert_eq!(read_full_output["file"]["content"], "alpha\nbeta\ngamma");
    assert_eq!(read_full_output["file"]["startLine"], 1);

    let read_slice = run_tool(
        "read_file",
        &json!({ "path": "nested/demo.txt", "offset": 1, "limit": 1 }),
    )
    .expect("read slice should succeed");
    let read_slice_output: serde_json::Value = serde_json::from_str(&read_slice).expect("json");
    assert_eq!(read_slice_output["file"]["content"], "beta");
    assert_eq!(read_slice_output["file"]["startLine"], 2);

    let read_past_end = run_tool(
        "read_file",
        &json!({ "path": "nested/demo.txt", "offset": 50 }),
    )
    .expect("read past EOF should succeed");
    let read_past_end_output: serde_json::Value =
        serde_json::from_str(&read_past_end).expect("json");
    assert_eq!(read_past_end_output["file"]["content"], "");
    assert_eq!(read_past_end_output["file"]["startLine"], 4);

    let read_error = run_tool("read_file", &json!({ "path": "missing.txt" }))
        .expect_err("missing file should fail");
    assert!(!read_error.to_string().is_empty());

    let edit_once = run_tool(
        "edit_file",
        &json!({ "path": "nested/demo.txt", "old_string": "alpha", "new_string": "omega" }),
    )
    .expect("single edit should succeed");
    let edit_once_output: serde_json::Value = serde_json::from_str(&edit_once).expect("json");
    assert_eq!(edit_once_output["replaceAll"], false);
    assert_eq!(
        fs::read_to_string(root.join("nested/demo.txt")).expect("read file"),
        "omega\nbeta\ngamma\n"
    );

    run_tool(
        "write_file",
        &json!({ "path": "nested/demo.txt", "content": "alpha\nbeta\nalpha\n" }),
    )
    .expect("reset file");
    let edit_all = run_tool(
        "edit_file",
        &json!({
            "path": "nested/demo.txt",
            "old_string": "alpha",
            "new_string": "omega",
            "replace_all": true
        }),
    )
    .expect("replace all should succeed");
    let edit_all_output: serde_json::Value = serde_json::from_str(&edit_all).expect("json");
    assert_eq!(edit_all_output["replaceAll"], true);
    assert_eq!(
        fs::read_to_string(root.join("nested/demo.txt")).expect("read file"),
        "omega\nbeta\nomega\n"
    );

    let edit_same = run_tool(
        "edit_file",
        &json!({ "path": "nested/demo.txt", "old_string": "omega", "new_string": "omega" }),
    )
    .expect_err("identical old/new should fail");
    assert!(edit_same.to_string().contains("must differ"));

    let edit_missing = run_tool(
        "edit_file",
        &json!({ "path": "nested/demo.txt", "old_string": "missing", "new_string": "omega" }),
    )
    .expect_err("missing substring should fail");
    assert!(edit_missing.to_string().contains("old_string not found"));

    std::env::set_current_dir(&original_dir).expect("restore cwd");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn multi_edit_applies_edits_sequentially_against_one_buffer() {
    let path = temp_path("multi-edit-sequential.txt");
    fs::write(&path, "alpha\ntail\n").expect("seed multi-edit file");
    let ctx = ToolContext::new();
    execute_tool(&ctx, "read_file", &json!({ "path": path }))
        .expect("read should seed freshness guard");

    execute_tool(
        &ctx,
        "MultiEdit",
        &json!({
            "path": path,
            "edits": [
                { "old_string": "alpha", "new_string": "beta" },
                { "old_string": "beta", "new_string": "gamma" }
            ]
        }),
    )
    .expect("later edit should see earlier edit output");

    assert_eq!(fs::read_to_string(&path).expect("read result"), "gamma\ntail\n");
    let _ = fs::remove_file(path);
}

#[test]
fn multi_edit_without_prior_read_is_rejected() {
    let path = temp_path("multi-edit-unread.txt");
    fs::write(&path, "alpha\n").expect("seed unread file");
    let ctx = ToolContext::new();

    let error = execute_tool(
        &ctx,
        "MultiEdit",
        &json!({
            "path": path,
            "edits": [{ "old_string": "alpha", "new_string": "beta" }]
        }),
    )
    .expect_err("MultiEdit must share the read-before-write guard");

    let message = error.to_string();
    assert!(
        message.contains("has not been read in this conversation")
            && message.contains("read_file"),
        "{message}"
    );
    assert_eq!(fs::read_to_string(&path).expect("read unchanged file"), "alpha\n");
    let _ = fs::remove_file(path);
}

#[test]
fn multi_edit_duplicate_edit_reports_index_and_writes_nothing() {
    let path = temp_path("multi-edit-duplicate-failure.txt");
    let original = "alpha\ntail\n";
    fs::write(&path, original).expect("seed multi-edit file");
    let ctx = ToolContext::new();
    execute_tool(&ctx, "read_file", &json!({ "path": path }))
        .expect("read should seed freshness guard");

    let error = execute_tool(
        &ctx,
        "MultiEdit",
        &json!({
            "path": path,
            "edits": [
                { "old_string": "alpha", "new_string": "beta" },
                { "old_string": "alpha", "new_string": "never-written" }
            ]
        }),
    )
    .expect_err("a failing later edit must reject the whole call");

    assert!(error.to_string().contains("edit[1] failed: old_string not found"));
    assert_eq!(fs::read_to_string(&path).expect("read unchanged file"), original);
    let _ = fs::remove_file(path);
}

#[test]
fn multi_edit_ambiguous_edit_reports_index_and_writes_nothing() {
    let path = temp_path("multi-edit-atomic-ambiguous.txt");
    let original = "alpha\nduplicate\nduplicate\n";
    fs::write(&path, original).expect("seed multi-edit file");
    let ctx = ToolContext::new();
    execute_tool(&ctx, "read_file", &json!({ "path": path }))
        .expect("read should seed freshness guard");

    let error = execute_tool(
        &ctx,
        "MultiEdit",
        &json!({
            "path": path,
            "edits": [
                { "old_string": "alpha", "new_string": "beta" },
                { "old_string": "duplicate", "new_string": "ambiguous" }
            ]
        }),
    )
    .expect_err("an ambiguous edit must reject the whole call");

    assert!(error.to_string().contains("edit[1] failed: old_string is not unique"));
    assert_eq!(fs::read_to_string(&path).expect("read unchanged file"), original);
    let _ = fs::remove_file(path);
}

#[test]
fn multi_edit_tolerant_match_preserves_crlf_line_endings() {
    let path = temp_path("multi-edit-crlf.txt");
    fs::write(&path, b"alpha\r\nbeta\r\ntail\r\n").expect("seed CRLF file");
    let ctx = ToolContext::new();
    execute_tool(&ctx, "read_file", &json!({ "path": path }))
        .expect("read should seed freshness guard");

    execute_tool(
        &ctx,
        "MultiEdit",
        &json!({
            "path": path,
            "edits": [
                { "old_string": "alpha\nbeta", "new_string": "ALPHA\nBETA" },
                { "old_string": "tail", "new_string": "done\nmore" }
            ]
        }),
    )
    .expect("CRLF-tolerant multi-edit should succeed");

    assert_eq!(
        fs::read(&path).expect("read CRLF result"),
        b"ALPHA\r\nBETA\r\ndone\r\nmore\r\n"
    );
    let _ = fs::remove_file(path);
}

#[test]
fn glob_and_grep_tools_cover_success_and_errors() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let root = temp_path("search-suite");
    fs::create_dir_all(root.join("nested")).expect("create root");
    let original_dir = std::env::current_dir().expect("cwd");
    std::env::set_current_dir(&root).expect("set cwd");

    fs::write(
        root.join("nested/lib.rs"),
        "fn main() {}\nlet alpha = 1;\nlet alpha = 2;\n",
    )
    .expect("write rust file");
    fs::write(
        root.join("nested/notes.txt"),
        "alpha\nbeta\nliteral (open\n",
    )
    .expect("write txt file");

    let globbed =
        run_tool("glob_search", &json!({ "pattern": "nested/*.rs" })).expect("glob should succeed");
    let globbed_output: serde_json::Value = serde_json::from_str(&globbed).expect("json");
    assert_eq!(globbed_output["numFiles"], 1);
    assert!(globbed_output["filenames"][0]
        .as_str()
        .expect("filename")
        .ends_with("nested/lib.rs"));

    let glob_error =
        run_tool("glob_search", &json!({ "pattern": "[" })).expect_err("invalid glob should fail");
    assert!(!glob_error.to_string().is_empty());

    let grep_content = run_tool(
        "grep_search",
        &json!({
            "pattern": "alpha",
            "path": "nested",
            "glob": "*.rs",
            "output_mode": "content",
            "-n": true,
            "head_limit": 1,
            "offset": 1
        }),
    )
    .expect("grep content should succeed");
    let grep_content_output: serde_json::Value = serde_json::from_str(&grep_content).expect("json");
    // The offset window keeps one content row from nested/lib.rs, so the
    // filename list names that file (it used to be paged by the same line
    // offset and came back empty — desynced from `content`).
    assert_eq!(grep_content_output["numFiles"], 1);
    assert!(grep_content_output["filenames"][0]
        .as_str()
        .expect("filename")
        .ends_with("nested/lib.rs"));
    assert!(grep_content_output["appliedLimit"].is_null());
    assert_eq!(grep_content_output["appliedOffset"], 1);
    assert!(grep_content_output["content"]
        .as_str()
        .expect("content")
        .contains("let alpha = 2;"));

    let grep_count = run_tool(
        "grep_search",
        &json!({ "pattern": "alpha", "path": "nested", "output_mode": "count" }),
    )
    .expect("grep count should succeed");
    let grep_count_output: serde_json::Value = serde_json::from_str(&grep_count).expect("json");
    assert_eq!(grep_count_output["numMatches"], 3);

    let grep_literal = run_tool(
        "grep_search",
        &json!({ "pattern": "(open", "path": "nested" }),
    )
    .expect("invalid regex should fall back to literal search");
    let grep_literal_output: serde_json::Value = serde_json::from_str(&grep_literal).expect("json");
    assert_eq!(grep_literal_output["numFiles"], 1);
    assert!(grep_literal_output["filenames"][0]
        .as_str()
        .expect("filename")
        .ends_with("nested/notes.txt"));

    std::env::set_current_dir(&original_dir).expect("restore cwd");
    let _ = fs::remove_dir_all(root);
}
