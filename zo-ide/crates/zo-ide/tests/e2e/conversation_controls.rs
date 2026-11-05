use super::*;
use serde_json::{Value, json};
use zerocode_core::conversation_control::{REQUEST_METHOD, RESULT_METHOD};

async fn control(client: &mut Client, session: &str, action: Value) -> Value {
    let started = Instant::now();
    let queued = client.call(REQUEST_METHOD, json!({"session":session,"action":action})).await.expect("queue control");
    let ticket = queued["ticket"].as_u64().expect("ticket");
    loop {
        let reply = client.call(RESULT_METHOD, json!({"ticket":ticket})).await.expect("read control result");
        if reply["pending"] == false { return reply["result"].clone(); }
        assert!(started.elapsed() < TEST_TIMEOUT, "control remained pending");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn conversation_controls_reach_the_real_owner_and_survive_a_fork() {
    let layout = Layout::new();
    let service = ScriptedAnthropicService::text("original answer kept").await.unwrap();
    let address = layout.root.path().join("control.addr");
    let address_text = address.to_string_lossy().to_string();
    let mut run = PtyRun::spawn_with_env(
        &layout.cwd, &layout.home, &layout.sessions, &layout.state,
        service.base_url(), &interactive_args(),
        &[("ZO_EVENTS_ADDR_FILE", &address_text), ("ZO_SERVE_TOKEN", HELPER_STOP_TOKEN)],
    ).unwrap();
    run.wait_for("directory:", TEST_TIMEOUT);
    let mut client = connect_to_addr_file(&address, HELPER_STOP_TOKEN).await;
    let info = client.call(method::INFO, json!({})).await.unwrap();
    let parent = info["id"].as_str().unwrap().to_string();
    let (frames, watcher) = watch_frames(&address, &parent).await;
    run.send(b"keep original prompt\r").unwrap();
    run.wait_for("original answer kept", TEST_TIMEOUT);
    run.wait_for_screen("/find search", 40, TEST_TIMEOUT);
    let original_path = wait_for_transcript(&layout, "original answer kept", TEST_TIMEOUT);
    wait_for_turn_ends(&frames, 1).await;
    let original = core_types::Session::load_from_path(&original_path).unwrap();
    let local = original.messages.iter().position(core_types::session::ConversationMessage::is_user_prompt).unwrap();
    let message_index = original.first_message_index() + u32::try_from(local).unwrap();

    run.send(b"/find original\r").unwrap();
    run.wait_for_screen("saved matches", 40, TEST_TIMEOUT);
    run.send(b"\x14").unwrap();
    run.send(b"/rewind\r").unwrap();
    run.wait_for_screen("Return to an earlier turn", 40, TEST_TIMEOUT);
    run.send(b"\r").unwrap();
    run.wait_for_screen("Conversation and files", 40, TEST_TIMEOUT);
    run.send(b"\x03").unwrap();

    let absent = layout.root.path().join("missing.jsonl");
    run.send(format!("/resume {}\r", absent.display()).as_bytes()).unwrap();
    run.wait_for("Could not open saved conversation", TEST_TIMEOUT);
    let preview = control(&mut client, &parent, json!({"kind":"preview","message_index":message_index})).await;
    assert_eq!(preview["ok"], true, "failed resume must keep the original owner: {preview}");
    let revision = preview["value"]["revision"].as_str().unwrap();
    let forked = control(&mut client, &parent, json!({"kind":"fork","message_index":message_index,"revision":revision})).await;
    assert_eq!(forked["ok"], true, "{forked}");
    assert_eq!(forked["value"]["kind"], "forked");
    let child_path = forked["value"]["path"].as_str().unwrap();
    let child = core_types::Session::load_from_path(child_path).unwrap();
    assert_eq!(child.fork.as_ref().unwrap().parent_session_id, parent);

    // The process channel stays put while the saved conversation changes.
    // The new owner must be active before the fork ticket reports completion.
    let child_preview = control(&mut client, &child.session_id, json!({"kind":"preview","message_index":message_index})).await;
    assert_eq!(child_preview["ok"], true, "{child_preview}");
    let stale = control(&mut client, &parent, json!({"kind":"preview","message_index":message_index})).await;
    assert_eq!(stale["ok"], false);
    let restored = control(&mut client, &child.session_id, json!({"kind":"restore","message_index":message_index,
        "revision":child_preview["value"]["revision"],"scope":"conversation"})).await;
    assert_eq!(restored["value"]["kind"], "restored", "{restored}");
    let saved = core_types::Session::load_from_path(child_path).unwrap();
    assert!(!saved.messages.iter().any(core_types::session::ConversationMessage::is_user_prompt));
    assert!(core_types::Session::load_from_path(&original_path).unwrap().messages.iter().any(core_types::session::ConversationMessage::is_user_prompt));
    watcher.abort();
    let _ = run.finish();
    assert_eq!(service.request_bodies().await.len(), 1, "controls must never become model prompts");
}
