//! What the door lets through, what it refuses and by which word, and what
//! is left of the words it lets through.

use serde_json::json;

use super::super::{
    BROWSER, JEV_USES, ROUTING, ROUTING_TASK_CHAR_CAP, STALL, STALL_SCREEN_BYTE_CAP,
    STALL_TRANSCRIPT_BYTE_CAP,
};
use super::*;
use crate::screen_action::{ActionLook, Errand, Where, ask};

const APP: &str = "/work/app";

fn settings(daily_requests: Option<u64>) -> JevSettings {
    JevSettings {
        enabled: true,
        workspaces: vec![APP.to_string()],
        daily_requests,
        model: crate::jev::DEFAULT_MODEL.to_string(),
    }
}

fn asking<'a>(settings: &'a JevSettings, workspace: Option<&'a str>, sent: u64) -> Asking<'a> {
    Asking {
        key: true,
        settings,
        workspace,
        sent_today: sent,
    }
}

fn routing_body(task: &str) -> Value {
    json!({ "state": task, "model": "jev-latest", "questions": {} })
}

#[test]
fn the_door_asks_in_order_and_names_each_refusal() {
    let consented = settings(Some(2));
    let off = JevSettings {
        enabled: false,
        ..consented.clone()
    };
    let body = routing_body("rename a variable");
    let refused = |asking: &Asking<'_>| may_send(&ROUTING, asking, body.clone()).err();

    // Every question fails at once: the first one names the refusal.
    let nothing = Asking {
        key: false,
        settings: &off,
        workspace: Some("/elsewhere"),
        sent_today: 9,
    };
    assert_eq!(refused(&nothing), Some(Refused::NoKey));
    assert_eq!(
        refused(&Asking {
            key: true,
            ..nothing
        }),
        Some(Refused::Off)
    );
    assert_eq!(
        refused(&asking(&consented, Some("/elsewhere"), 0)),
        Some(Refused::NotConsented)
    );
    assert_eq!(
        refused(&asking(&consented, None, 0)),
        Some(Refused::NotConsented),
        "a workspace nobody could name consents to nothing"
    );
    assert_eq!(
        refused(&asking(&consented, Some(APP), 2)),
        Some(Refused::Budget)
    );
    assert_eq!(refused(&asking(&consented, Some(APP), 1)), None);

    let tokens: Vec<&str> = Refused::ALL.iter().map(|refused| refused.token()).collect();
    assert_eq!(tokens, ["no_key", "off", "not_consented", "budget"]);
    assert_eq!(Refused::Off.token(), JevMode::Off.key());
    for refused in Refused::ALL {
        assert_eq!(Refused::from_token(refused.token()), Some(refused));
    }
    assert_eq!(Refused::from_token("timeout"), None);
}

#[test]
fn a_folder_under_a_consented_root_passes_and_one_beside_it_does_not() {
    let consented = settings(None);
    for inside in [APP, "/work/app/", "/work/app/crates/tools"] {
        assert!(consented.consents(inside), "{inside}");
    }
    for beside in ["/work/apple", "/work", "/other/app", ""] {
        assert!(!consented.consents(beside), "{beside:?}");
    }
    let windows = JevSettings {
        workspaces: vec![r"C:\work\app".to_string()],
        ..consented
    };
    assert!(windows.consents(r"C:\work\app\src"));
    assert!(!windows.consents(r"C:\work\apple"));
}

#[test]
fn an_unregistered_git_pointer_cannot_borrow_another_checkouts_consent() {
    let dir = tempfile::tempdir().expect("machine");
    let parent = dir.path().join("parent");
    let stranger = dir.path().join("stranger");
    std::fs::create_dir_all(parent.join(".git")).expect("parent");
    std::fs::create_dir_all(&stranger).expect("stranger");
    std::fs::write(
        stranger.join(".git"),
        format!("gitdir: {}\n", parent.join(".git").display()),
    )
    .expect("untrusted pointer");
    let consented = JevSettings {
        workspaces: vec![resolved_path(&parent)],
        ..settings(None)
    };
    assert!(!consented.consents(&resolved_path(&stranger)));
}

#[cfg(unix)]
#[test]
fn a_symlink_to_a_registered_git_file_does_not_register_its_own_folder() {
    let dir = tempfile::tempdir().expect("machine");
    let parent = dir.path().join("parent");
    let own = parent.join(".git/worktrees/registered");
    let registered = dir.path().join("registered");
    let stranger = dir.path().join("stranger");
    for path in [&own, &registered, &stranger] {
        std::fs::create_dir_all(path).expect("folder");
    }
    std::fs::write(
        registered.join(".git"),
        format!("gitdir: {}\n", own.display()),
    )
    .expect("pointer");
    std::fs::write(own.join("commondir"), "../..").expect("shared");
    std::fs::write(
        own.join("gitdir"),
        registered.join(".git").to_string_lossy().as_bytes(),
    )
    .expect("backlink");
    std::os::unix::fs::symlink(registered.join(".git"), stranger.join(".git"))
        .expect("borrowed pointer");
    let consented = JevSettings {
        workspaces: vec![resolved_path(&parent)],
        ..settings(None)
    };
    assert!(!consented.consents(&resolved_path(&stranger)));
}

#[cfg(unix)]
#[test]
fn a_symlink_below_a_consented_root_does_not_consent_to_its_outside_target() {
    let dir = tempfile::tempdir().expect("machine");
    let parent = dir.path().join("parent");
    let stranger = dir.path().join("stranger");
    std::fs::create_dir_all(&parent).expect("parent");
    std::fs::create_dir_all(&stranger).expect("stranger");
    std::os::unix::fs::symlink(&stranger, parent.join("link")).expect("link");
    let consented = JevSettings {
        workspaces: vec![resolved_path(&parent)],
        ..settings(None)
    };
    let link = Path::new(&resolved_path(&parent)).join("link");
    assert!(!consented.consents(&link.to_string_lossy()));
}

#[test]
fn a_worktree_of_a_consented_checkout_is_consented_and_a_strangers_is_not() {
    let dir = tempfile::tempdir().expect("a machine to cut checkouts on");
    // 동의한 체크아웃 하나, 동의하지 않은 체크아웃 하나 — 같은 기계, 같은 무늬.
    let mine = dir.path().join("app");
    let theirs = dir.path().join("stranger");
    let cut_from = |checkout: &std::path::Path, name: &str| {
        let git_dir = checkout.join(".git").join("worktrees").join(name);
        std::fs::create_dir_all(&git_dir).expect("the worktree's own git directory");
        let at = dir.path().join("workspaces").join(name);
        std::fs::create_dir_all(&at).expect("the checkout");
        std::fs::write(at.join(".git"), format!("gitdir: {}\n", git_dir.display()))
            .expect("the pointer git writes");
        std::fs::write(git_dir.join("commondir"), "../..\n").expect("the shared pointer");
        std::fs::write(
            git_dir.join("gitdir"),
            at.join(".git").to_string_lossy().as_bytes(),
        )
        .expect("registered backlink");
        at
    };
    std::fs::create_dir_all(mine.join("crates")).expect("a folder inside the checkout");
    let ours = cut_from(&mine, "t-5805");
    let strangers = cut_from(&theirs, "side");

    let settings = JevSettings {
        enabled: true,
        workspaces: vec![resolved_path(&mine)],
        daily_requests: None,
        model: crate::jev::DEFAULT_MODEL.to_string(),
    };
    // 동의한 체크아웃, 그 아래 폴더, 거기서 잘라 낸 워크트리, 그 아래 폴더.
    for inside in [
        mine.clone(),
        mine.join("crates"),
        ours.clone(),
        ours.join("ui"),
    ] {
        assert!(
            settings.consents(&resolved_path(&inside)),
            "{}",
            inside.display()
        );
    }
    // 남의 저장소도, 남의 워크트리도, 둘을 담은 폴더도 아니다.
    for outside in [theirs.clone(), strangers, dir.path().to_path_buf()] {
        assert!(
            !settings.consents(&resolved_path(&outside)),
            "{}",
            outside.display()
        );
    }
    // 그리고 문은 같은 답을 낸다 — 규칙은 한 함수이고, 문은 그것을 부른다.
    let refused = |workspace: &str| {
        may_send(
            &ROUTING,
            &Asking {
                key: true,
                settings: &settings,
                workspace: Some(workspace),
                sent_today: 0,
            },
            json!({}),
        )
        .err()
    };
    assert_eq!(refused(&resolved_path(&ours)), None);
    assert_eq!(
        refused(&resolved_path(&theirs)),
        Some(Refused::NotConsented)
    );
}

#[test]
fn settings_of_the_wrong_shape_never_widen_what_is_sent() {
    let read = |jev: Value| JevSettings::from_root(&json!({ "smart": { "jev": jev } }));

    let absent = JevSettings::from_root(&json!({ "smart": { "decisionShadow": "shadow" } }));
    assert_eq!(
        absent,
        JevSettings {
            enabled: true,
            workspaces: Vec::new(),
            daily_requests: None,
            model: crate::jev::DEFAULT_MODEL.to_string(),
        },
        "unset, Jev is on and nothing is consented or capped"
    );
    assert!(
        !read(json!(true)).enabled,
        "a `jev` that is not an object is off"
    );
    assert!(!read(json!({ "enabled": "yes" })).enabled);
    assert!(!read(json!({ "enabled": false })).enabled);
    assert!(read(json!({ "enabled": true })).enabled);

    assert!(
        read(json!({ "workspaces": "/work/app" }))
            .workspaces
            .is_empty()
    );
    assert_eq!(
        read(json!({ "workspaces": [" /work/app ", "", 3, "/b"] })).workspaces,
        ["/work/app", "/b"]
    );

    assert_eq!(read(json!({ "dailyRequests": null })).daily_requests, None);
    assert_eq!(read(json!({ "dailyRequests": 5 })).daily_requests, Some(5));
    for wrong in [json!("5"), json!(-1), json!(2.5)] {
        assert_eq!(
            read(json!({ "dailyRequests": wrong })).daily_requests,
            Some(0),
            "a budget of the wrong shape sends nothing: {wrong}"
        );
    }
}

/// The every-folder word consents to every folder a program can name, and to
/// nothing a program cannot: a relative path or one that climbs is refused as
/// it always was. It is a word, not a path, so it is never resolved against
/// the asking program's own directory.
#[test]
fn every_folder_consents_to_every_named_workspace_and_is_never_resolved() {
    let everywhere = JevSettings {
        workspaces: vec![EVERY_WORKSPACE.to_string()],
        ..settings(None)
    };
    assert!(everywhere.everywhere());
    assert_eq!(everywhere.folders().count(), 0, "the word is not a folder");
    for named in [APP, "/elsewhere/entirely", "/"] {
        assert!(everywhere.consents(named), "{named}");
    }
    for unnamed in ["work/app", "", "/work/../etc"] {
        assert!(!everywhere.consents(unnamed), "{unnamed:?}");
    }
    assert_eq!(
        everywhere.clone().resolved().workspaces,
        [EVERY_WORKSPACE],
        "the word is kept as written"
    );

    let named = settings(None);
    assert!(!named.everywhere());
    assert_eq!(named.folders().collect::<Vec<_>>(), [APP]);
    assert!(!named.consents("/elsewhere/entirely"));
}

/// The settings file of a person who set every seat by hand (2026-09-23: the
/// twenty seat words at `auto` or `on`, four folders consented), with
/// everything a switch must not move beside it. `enabled` is the door's
/// switch as written: `Some(true)` as the brief shaped it, `None` as this
/// machine's own file holds it — never written, with a key of the door's
/// neighbours (`labelDrafts`) under the same object.
fn a_persons_file(enabled: Option<bool>) -> Value {
    let mut smart = serde_json::Map::new();
    for (at, row) in JEV_USES.iter().enumerate() {
        let word = if at % 3 == 0 || !row.modes.contains(&JevMode::Auto) {
            JevMode::On
        } else {
            JevMode::Auto
        };
        smart.insert(row.setting.to_string(), json!(word.key()));
    }
    let mut jev = json!({
        WORKSPACES_SETTING: ["/work/a", "/work/b", "/work/c", "/work/d"],
        DAILY_REQUESTS_SETTING: 500,
        "labelDrafts": true,
    });
    if let Some(enabled) = enabled {
        jev[ENABLED_SETTING] = json!(enabled);
    }
    smart.insert(JEV_SETTINGS_KEY.to_string(), jev);
    smart.insert(crate::jev::MODEL_SETTING.to_string(), json!("jev-1.13.0"));
    smart.insert(crate::jev::CLASSIFIER_SETTING.to_string(), json!("probed"));
    smart.insert("plan".to_string(), json!({ "minEdge": 3 }));
    json!({ "model": "fable", "providers": [], SMART_SETTINGS_KEY: smart })
}

/// One press on, from the file a person who set every seat by hand keeps:
/// every seat stands at its row's recommendation, every folder is consented,
/// and nothing else of the file moved. One press off: the switch alone
/// moves, every seat nobody wrote a word for is off, and the door refuses
/// every one of the twenty with `off` — nothing is sent.
#[test]
fn one_press_on_hands_every_seat_its_recommendation_and_one_press_off_sends_nothing() {
    for enabled in [Some(true), None] {
        one_press_on_and_one_off(enabled);
    }
}

fn one_press_on_and_one_off(enabled: Option<bool>) {
    let before = a_persons_file(enabled);
    let mut root = before.clone();
    // Before the press, each seat stands at the word written for it, and the
    // door sends from the four folders and nowhere else.
    for row in &JEV_USES {
        assert_eq!(
            row.mode_in(&root),
            row.mode_of(root[SMART_SETTINGS_KEY].get(row.setting)),
            "{}",
            row.id
        );
    }
    let door = JevSettings::from_root(&root);
    assert!(door.enabled && !door.everywhere(), "{enabled:?}");
    assert!(door.consents("/work/c/src") && !door.consents("/a/folder/nobody/named"));
    let smart = |root: &mut Value| {
        root.get_mut(SMART_SETTINGS_KEY)
            .and_then(Value::as_object_mut)
            .expect("smart")
            .clone()
    };
    let write = |root: &mut Value, smart: serde_json::Map<String, Value>| {
        root[SMART_SETTINGS_KEY] = Value::Object(smart);
    };

    let mut on = smart(&mut root);
    switch_on(&mut on).expect("an object");
    write(&mut root, on);
    for row in &JEV_USES {
        assert_eq!(row.mode_in(&root), row.recommended, "{}", row.id);
        assert!(
            root[SMART_SETTINGS_KEY].get(row.setting).is_none(),
            "{} kept its own word",
            row.id
        );
    }
    let door = JevSettings::from_root(&root);
    assert!(door.enabled && door.everywhere());
    assert!(door.consents("/a/folder/nobody/named"));
    for kept in ["model", "providers"] {
        assert_eq!(root[kept], before[kept], "{kept} moved");
    }
    for kept in [
        crate::jev::MODEL_SETTING,
        crate::jev::CLASSIFIER_SETTING,
        "plan",
    ] {
        assert_eq!(
            root[SMART_SETTINGS_KEY][kept], before[SMART_SETTINGS_KEY][kept],
            "smart.{kept} moved"
        );
    }
    for kept in [DAILY_REQUESTS_SETTING, "labelDrafts"] {
        assert_eq!(
            root[SMART_SETTINGS_KEY][JEV_SETTINGS_KEY][kept],
            before[SMART_SETTINGS_KEY][JEV_SETTINGS_KEY][kept],
            "smart.jev.{kept} moved"
        );
    }
    let again = root.clone();
    let mut twice = smart(&mut root);
    switch_on(&mut twice).expect("an object");
    write(&mut root, twice);
    assert_eq!(root, again, "a second press on changes nothing");

    let mut off = smart(&mut root);
    switch_off(&mut off).expect("an object");
    write(&mut root, off);
    let mut expected = again.clone();
    expected[SMART_SETTINGS_KEY][JEV_SETTINGS_KEY][ENABLED_SETTING] = json!(false);
    assert_eq!(root, expected, "off moves the switch and nothing else");
    let door = JevSettings::from_root(&root);
    for row in &JEV_USES {
        assert_eq!(row.mode_in(&root), JevMode::Off, "{}", row.id);
        let asked = Asking {
            key: true,
            settings: &door,
            workspace: Some(APP),
            sent_today: 0,
        };
        assert_eq!(
            may_send(row, &asked, json!({})).err(),
            Some(Refused::Off),
            "{} was let through with Jev off",
            row.id
        );
    }
}

/// A `smart.jev` of another shape is the person's to fix: neither press
/// overwrites it.
#[test]
fn a_press_refuses_a_jev_of_another_shape() {
    let mut smart = serde_json::Map::new();
    smart.insert(JEV_SETTINGS_KEY.to_string(), json!("on please"));
    smart.insert(ROUTING.setting.to_string(), json!("shadow"));
    let before = smart.clone();
    assert_eq!(switch_on(&mut smart), Err(NotAnObject(JEV_SETTINGS_KEY)));
    assert_eq!(switch_off(&mut smart), Err(NotAnObject(JEV_SETTINGS_KEY)));
    assert_eq!(smart, before, "a refused press wrote nothing");
}

#[test]
fn every_line_that_may_carry_a_credential_is_withheld_whole_and_counted() {
    let task = "Rename the session cache\n\
                export OPENAI_API_KEY=sk-proj-abc123\n\
                keep this line\n\
                curl -H 'Authorization: Bearer abc.def' https://api.test";
    let (kept, withheld) = clear_text(task, Cap::Uncut);
    assert_eq!(withheld, 2);
    assert_eq!(
        kept.lines().collect::<Vec<_>>(),
        [
            "Rename the session cache",
            WITHHELD_LINE,
            "keep this line",
            WITHHELD_LINE
        ]
    );
    for secret in ["sk-proj-abc123", "abc.def", "OPENAI_API_KEY"] {
        assert!(!kept.contains(secret), "{secret} left the door: {kept}");
    }
    let (plain, none) = clear_text("nothing to hide\nhere", Cap::Uncut);
    assert_eq!((plain.as_str(), none), ("nothing to hide\nhere", 0));
}

#[test]
fn the_cut_lands_on_the_cap_and_a_withheld_line_past_it_is_not_counted() {
    // The secret line starts past ten characters: none of it is sent.
    let (kept, withheld) = clear_text("0123456789\npassword: hunter2", Cap::Chars(10));
    assert_eq!((kept.as_str(), withheld), ("0123456789", 0));

    // A withheld line that starts exactly where the cut falls is not sent.
    let (kept, withheld) = clear_text("01234\npassword: hunter2", Cap::Chars(6));
    assert_eq!((kept.as_str(), withheld), ("01234\n", 0));
    // One character further, its mark begins to be sent, and it counts.
    let (kept, withheld) = clear_text("01234\npassword: hunter2", Cap::Chars(7));
    assert_eq!((kept.as_str(), withheld), ("01234\n[", 1));

    // Characters are characters, and bytes end on a character's boundary.
    assert_eq!(cut("가나다라", Cap::Chars(2)), "가나");
    let clipped = cut("가나다", Cap::Bytes(4));
    assert_eq!(clipped, format!("가{CUT_MARK}"));
    assert_eq!(
        cut(&clipped, Cap::Bytes(4)),
        clipped,
        "a cut text cuts to itself"
    );
    assert_eq!(
        cut("가나", Cap::Bytes(6)),
        "가나",
        "a text that fits is not marked"
    );

    // Reading stops past the cap and the answer is the same as clearing the
    // whole text and cutting it.
    let long: String = (0..2_000)
        .map(|line| format!("line {line} of the brief\n"))
        .chain(std::iter::once("token=sk-live-000".to_string()))
        .collect();
    let (kept, withheld) = clear_text(&long, Cap::Chars(ROUTING_TASK_CHAR_CAP));
    let (whole, _) = clear_text(&long, Cap::Uncut);
    assert_eq!(kept, cut(&whole, Cap::Chars(ROUTING_TASK_CHAR_CAP)));
    assert_eq!(withheld, 0, "the secret sat past the cap");
    let (kept, withheld) = clear_text(&long, Cap::Bytes(64));
    assert_eq!(kept, cut(&whole, Cap::Bytes(64)));
    assert_eq!(withheld, 0);
}

/// With nothing to withhold, the routing state is the task cut exactly as the
/// chat probe's prompt cuts it — the door changes no byte of an honest task.
#[test]
fn an_honest_task_leaves_the_door_as_the_probe_cuts_it() {
    let task: String = "가".repeat(ROUTING_TASK_CHAR_CAP + 7);
    let cleared = may_send(
        &ROUTING,
        &asking(&settings(None), Some(APP), 0),
        routing_body(&task),
    )
    .expect("consented");
    let body: Value = serde_json::from_slice(cleared.bytes()).expect("json");
    assert_eq!(
        body["state"],
        json!(task.chars().take(ROUTING_TASK_CHAR_CAP).collect::<String>())
    );
    assert_eq!(cleared.withheld_lines(), 0);
    assert_eq!(
        body["model"], "jev-latest",
        "the product's own words pass as they were"
    );
}

/// A stopped walk's question puts page words in its state and in the options'
/// descriptions. Every one the browser row names reaches the door, and no
/// credential in any of them reaches the wire.
#[test]
fn a_stopped_walks_question_leaves_no_credential_from_any_field_the_row_names() {
    let items = [
        json!({ "mark": 1, "role": "button", "label": "Save", "centerX": 10.0, "centerY": 20.0 }),
        json!({ "mark": 2, "role": "textbox", "label": "token=sk-live-123", "centerX": 30.0, "centerY": 20.0 }),
    ];
    let asked = ask(&ActionLook {
        goal: "sign in\npassword: hunter2",
        errand: Errand::Clear {
            stopped: "step_failed",
            step: "type",
            refusal: "Bearer abc.def was refused",
        },
        at: Where::Page {
            host: "app.local",
            path: "/login",
        },
        tried: &[],
        items: &items,
        pressed: &[],
        shows: &[],
    })
    .expect("a screen with controls asks");
    let mut body =
        json!({ "state": asked.state, "model": "jev-latest", "questions": asked.questions });

    for sent in BROWSER.sends {
        let path: Vec<&str> = sent.at.split('/').skip(1).collect();
        let mut reached = 0;
        visit(&mut body, &path, &mut |_| reached += 1);
        assert!(reached > 0, "{} names nothing in a real question", sent.at);
    }

    let cleared =
        may_send(&BROWSER, &asking(&settings(None), Some(APP), 0), body).expect("consented");
    let sent = String::from_utf8(cleared.bytes().to_vec()).expect("utf-8");
    for secret in ["hunter2", "abc.def", "sk-live-123"] {
        assert!(!sent.contains(secret), "{secret} left the door: {sent}");
    }
    // The goal's line, the refusal, and the label twice — as a candidate and
    // as its option's description.
    assert_eq!(cleared.withheld_lines(), 4);
    let body: Value = serde_json::from_slice(cleared.bytes()).expect("json");
    assert!(
        body["questions"]["action"]["criteria"]
            .as_object()
            .is_some_and(|options| options.contains_key("mark:2")),
        "an option's name is the product's word and stays answerable"
    );
}

/// A quiet worker's question puts its screen and its transcript's tail in the
/// state. Both fields the stall row names reach real text, no credential on
/// either reaches the wire, and what is sent fits the row's caps.
#[test]
fn a_stalled_panes_question_leaves_no_credential_and_fits_its_caps() {
    let screen =
        "cargo test\nexport OPENAI_API_KEY=sk-proj-123\n✻ Worked for 27m 0s · done 2:11 AM\n❯";
    let transcript = vec![
        r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t","content":"curl -H 'Authorization: Bearer abc.def' https://api.test"}]}}"#
            .to_string(),
    ];
    let asked = crate::stall_cause::ask(&crate::stall_cause::StallLook {
        agent: "claude",
        quiet_ms: 200_000,
        screen,
        transcript: &transcript,
    })
    .expect("a screen and a record ask");
    let mut body =
        json!({ "state": asked.state, "model": "jev-latest", "questions": asked.questions });
    for sent in STALL.sends {
        let path: Vec<&str> = sent.at.split('/').skip(1).collect();
        let mut reached = 0;
        visit(&mut body, &path, &mut |_| reached += 1);
        assert_eq!(reached, 1, "{} names nothing in a real question", sent.at);
    }

    let cleared =
        may_send(&STALL, &asking(&settings(None), Some(APP), 0), body).expect("consented");
    let sent = String::from_utf8(cleared.bytes().to_vec()).expect("utf-8");
    for secret in ["sk-proj-123", "abc.def"] {
        assert!(!sent.contains(secret), "{secret} left the door: {sent}");
    }
    assert_eq!(
        cleared.withheld_lines(),
        2,
        "the screen's line and the result's"
    );
    let body: Value = serde_json::from_slice(cleared.bytes()).expect("json");
    assert!(
        body["state"]["screen"]
            .as_str()
            .is_some_and(|text| text.len() <= STALL_SCREEN_BYTE_CAP)
    );
    assert!(
        body["state"]["transcript"]
            .as_str()
            .is_some_and(|text| text.len() <= STALL_TRANSCRIPT_BYTE_CAP)
    );
    assert!(
        body["questions"]["cause"]["criteria"]
            .as_object()
            .is_some_and(|options| options.contains_key("finished_without_report")),
        "the product's own options pass as they were"
    );
}

/// A list's cap runs before the texts under it are read, so no line past the
/// cap is cleared, counted or sent.
#[test]
fn a_rows_list_caps_come_before_the_texts_under_them() {
    for row in &JEV_USES {
        for (index, list) in row.sends.iter().enumerate() {
            if !matches!(list.cap, Cap::Items(_)) {
                continue;
            }
            let under = format!("{}/", list.at);
            for earlier in &row.sends[..index] {
                assert!(
                    !earlier.at.starts_with(&under),
                    "{}: {} is read before its list is cut",
                    row.id,
                    earlier.at
                );
            }
        }
    }
}

#[test]
fn a_key_check_asks_no_consent_but_keeps_the_switch_and_the_budget() {
    let body =
        json!({ "state": "Rename one local variable.", "model": "jev-latest", "questions": {} });
    let capped = settings(Some(1));
    assert!(may_check_key(&asking(&capped, None, 0), &body).is_ok());
    assert_eq!(
        may_check_key(&asking(&capped, None, 1), &body).err(),
        Some(Refused::Budget)
    );
    let off = JevSettings {
        enabled: false,
        ..capped.clone()
    };
    assert_eq!(
        may_check_key(&asking(&off, None, 0), &body).err(),
        Some(Refused::Off)
    );
    let keyless = Asking {
        key: false,
        ..asking(&capped, None, 0)
    };
    assert_eq!(may_check_key(&keyless, &body).err(), Some(Refused::NoKey));
}

/// Every request the door lets through names the model the person pinned
/// (`smart.jevModel`, t-6187) — whatever the asking program wrote into it,
/// in both programs, because both pass this door — and the vendor's alias
/// when nobody pinned one, so an unpinned request leaves the door exactly
/// as it came. A pin that is not one word is no pin: a slip never sends a
/// model nobody named.
#[test]
fn a_cleared_request_names_the_model_the_person_pinned() {
    let named = |settings: &Value| {
        let door = JevSettings::from_root(settings);
        let cleared = may_send(
            &ROUTING,
            &asking(&door, Some(APP), 0),
            routing_body("rename a variable"),
        )
        .expect("cleared");
        let checked =
            may_check_key(&asking(&door, None, 0), &routing_body("a key check")).expect("checked");
        let model = |bytes: &[u8]| {
            serde_json::from_slice::<Value>(bytes).expect("a body")["model"]
                .as_str()
                .map(str::to_string)
        };
        (model(cleared.bytes()), model(checked.bytes()), cleared)
    };
    let pinned = json!({"smart": {"jevModel": " jev-1.13.0 ", "jev": {"workspaces": [APP]}}});
    let (sent, checked, _) = named(&pinned);
    assert_eq!(sent.as_deref(), Some("jev-1.13.0"));
    assert_eq!(
        checked.as_deref(),
        Some("jev-1.13.0"),
        "a key check asks the pinned model too"
    );

    let unpinned = json!({"smart": {"jev": {"workspaces": [APP]}}});
    let (sent, _, cleared) = named(&unpinned);
    assert_eq!(sent.as_deref(), Some("jev-latest"));
    assert_eq!(
        cleared.bytes(),
        routing_body("rename a variable").to_string().as_bytes(),
        "an unpinned request is the body the program built, to the byte"
    );

    for slip in [
        json!(""),
        json!("   "),
        json!("jev 1.13"),
        json!(113),
        json!(null),
    ] {
        let (sent, _, _) =
            named(&json!({"smart": {"jevModel": slip, "jev": {"workspaces": [APP]}}}));
        assert_eq!(
            sent.as_deref(),
            Some("jev-latest"),
            "{slip} pinned something"
        );
    }
}

#[test]
fn the_door_counts_only_what_it_lets_through_and_the_last_place_goes_once() {
    let home = tempfile::tempdir().expect("a config home");
    let requests = count::requests_path(home.path(), "2026-09-17");
    let capped = settings(Some(2));
    let body = routing_body("rename a variable");
    let routing = |asking: &Asking<'_>| may_send(&ROUTING, asking, body.clone());

    assert!(pass(routing, true, &capped, Some(APP), &requests).is_ok());
    assert_eq!(
        pass(routing, true, &capped, Some("/elsewhere"), &requests).err(),
        Some(Refused::NotConsented)
    );
    assert_eq!(
        count::sent(&requests),
        1,
        "a refused request is not counted"
    );
    assert!(pass(routing, true, &capped, Some(APP), &requests).is_ok());
    assert_eq!(
        pass(routing, true, &capped, Some(APP), &requests).err(),
        Some(Refused::Budget)
    );
    assert_eq!(count::sent(&requests), 2);

    // Two programs ask at once with one place left: the one counted second is
    // refused after the door let it through.
    let racing = count::requests_path(home.path(), "2026-09-18");
    count::count_one(&racing).expect("an earlier request");
    let raced = pass(
        |asking: &Asking<'_>| {
            let cleared = may_send(&ROUTING, asking, body.clone());
            count::count_one(&racing).expect("the other program's request");
            cleared
        },
        true,
        &capped,
        Some(APP),
        &racing,
    );
    assert_eq!(raced.err(), Some(Refused::Budget));

    // A count that cannot be kept refuses only where a budget needs keeping.
    let blocked = home.path().join("a-file");
    std::fs::write(&blocked, "").expect("a file where the directory should be");
    let unkept = count::requests_path(&blocked, "2026-09-17");
    assert_eq!(
        pass(routing, true, &capped, Some(APP), &unkept).err(),
        Some(Refused::Budget)
    );
    assert!(pass(routing, true, &settings(None), Some(APP), &unkept).is_ok());
}

/// A judgment's second request is the person's money spent twice, so it takes
/// a place in the day exactly as the first did — and a day with no place left
/// refuses it, which is what stops the hedge from sending it.
#[test]
fn a_hedge_takes_the_days_place_before_it_may_leave() {
    let home = tempfile::tempdir().expect("a config home");
    let requests = count::requests_path(home.path(), "2026-09-17");
    let capped = settings(Some(3));
    let body = routing_body("rename a variable");
    let routing = |asking: &Asking<'_>| may_send(&ROUTING, asking, body.clone());

    assert!(pass(routing, true, &capped, Some(APP), &requests).is_ok());
    assert!(
        count_a_hedge(&capped, &requests).is_ok(),
        "the day had room for the second copy"
    );
    assert_eq!(count::sent(&requests), 2, "one judgment, two places");

    // The third place goes to a judgment; there is then nothing left for its
    // hedge, and the refusal is what keeps the second copy off the wire.
    assert!(pass(routing, true, &capped, Some(APP), &requests).is_ok());
    assert_eq!(
        count_a_hedge(&capped, &requests).err(),
        Some(Refused::Budget)
    );
    assert_eq!(
        count::sent(&requests),
        4,
        "a refused place is still spent, never unspent"
    );

    // Uncapped, a hedge is counted and never refused — the same as a first
    // request under no budget.
    let open = count::requests_path(home.path(), "2026-09-19");
    assert!(count_a_hedge(&settings(None), &open).is_ok());
    assert_eq!(count::sent(&open), 1);

    // A count that cannot be kept refuses only where a budget needs keeping.
    let blocked = home.path().join("another-file");
    std::fs::write(&blocked, "").expect("a file where the directory should be");
    let unkept = count::requests_path(&blocked, "2026-09-17");
    assert_eq!(count_a_hedge(&capped, &unkept).err(), Some(Refused::Budget));
    assert!(count_a_hedge(&settings(None), &unkept).is_ok());
}

#[test]
fn a_day_file_counts_one_byte_per_request_and_forgets_earlier_days() {
    let home = tempfile::tempdir().expect("a config home");
    let yesterday = count::requests_path(home.path(), "2026-09-16");
    assert_eq!(count::sent(&yesterday), 0);
    assert_eq!(count::count_one(&yesterday).expect("count"), 1);
    assert_eq!(count::count_one(&yesterday).expect("count"), 2);
    let today = count::requests_path(home.path(), "2026-09-17");
    assert_eq!(count::count_one(&today).expect("count"), 1);
    assert!(
        !yesterday.exists(),
        "the first request of a day clears earlier days"
    );
    assert_eq!(count::sent(&today), 1);

    // The day is the person's: 15:00 UTC is already tomorrow in Seoul.
    const SEOUL_MINUTES: i32 = 9 * 60;
    let afternoon_utc = 15 * 60 * 60 * 1_000;
    assert_eq!(count::day_of(afternoon_utc, 0), "1970-01-01");
    assert_eq!(count::day_of(afternoon_utc, SEOUL_MINUTES), "1970-01-02");
}

#[test]
fn a_row_without_the_doors_count_predates_the_door() {
    assert!(predates_the_door(
        r#"{"at":1,"task":"0000000000000001","outcome":"answered"}"#
    ));
    assert!(!predates_the_door(
        r#"{"at":2,"outcome":"not_consented","requests":0,"redactedLines":0}"#
    ));
    assert!(!predates_the_door("{\"at\": "), "a torn line says nothing");
    assert!(!predates_the_door("[1, 2]"));
}

/// The memo sits between the door's four questions and its count: a hit
/// under a seat that may apply it is a request that passed and did not
/// leave — no place taken — while under a seat that only compares the same
/// hit is counted like any request, because the request still goes. Every
/// refusal is asked before the memo is.
#[test]
fn a_memo_hit_passes_the_door_and_takes_no_place_only_when_it_may_answer() {
    let home = tempfile::tempdir().expect("a config home");
    let requests = count::requests_path(home.path(), "2026-09-22");
    let memo_file = home.path().join(count::REQUESTS_DIR).join(memo::MEMO_FILE);
    let capped = settings(Some(3));
    let body = routing_body("rename a variable");
    let routing = |asking: &Asking<'_>| may_send(&ROUTING, asking, body.clone());
    let memo = |applying: bool| Memo {
        path: &memo_file,
        seat: &ROUTING,
        applying,
    };

    // A miss under an applying seat: looked up, nothing held, counted.
    let passed = pass_remembering(
        routing,
        true,
        &capped,
        Some(APP),
        &requests,
        Some(memo(true)),
    )
    .expect("the door lets it through");
    let memoed = passed.memo.clone().expect("a memo was asked");
    assert_eq!(memoed.recalled, None);
    assert!(!memoed.answered);
    assert_eq!(count::sent(&requests), 1, "a miss leaves, and is counted");
    assert_eq!(memoed.key, memo::key_of(&ROUTING, passed.cleared.bytes()));

    // The wire's answer, remembered under the cleared bytes' key.
    memo::remember(&memo_file, &ROUTING, &memoed.key, "{\"answers\":{}}", 1).expect("kept");

    // A hit under a seat that only compares: the request still goes, so it
    // still takes its place.
    let passed = pass_remembering(
        routing,
        true,
        &capped,
        Some(APP),
        &requests,
        Some(memo(false)),
    )
    .expect("the door lets it through");
    let memoed = passed.memo.expect("a memo was asked");
    assert!(memoed.recalled.is_some());
    assert!(!memoed.answered);
    assert_eq!(count::sent(&requests), 2);

    // A hit under an applying seat: passed, not counted.
    let passed = pass_remembering(
        routing,
        true,
        &capped,
        Some(APP),
        &requests,
        Some(memo(true)),
    )
    .expect("the door lets it through");
    let memoed = passed.memo.expect("a memo was asked");
    assert!(memoed.answered);
    assert_eq!(
        memoed.recalled.map(|r| r.answer),
        Some("{\"answers\":{}}".to_string())
    );
    assert_eq!(
        count::sent(&requests),
        2,
        "a hit that answers takes no place"
    );

    // The four questions come first: a workspace nobody consented to is
    // refused with the memo full, and so is a day with no place left.
    assert_eq!(
        pass_remembering(
            routing,
            true,
            &capped,
            Some("/elsewhere"),
            &requests,
            Some(memo(true))
        )
        .err(),
        Some(Refused::NotConsented)
    );
    assert_eq!(
        pass_remembering(
            routing,
            false,
            &capped,
            Some(APP),
            &requests,
            Some(memo(true))
        )
        .err(),
        Some(Refused::NoKey)
    );
    count::count_one(&requests).expect("the day's last place");
    assert_eq!(
        pass_remembering(
            routing,
            true,
            &capped,
            Some(APP),
            &requests,
            Some(memo(true))
        )
        .err(),
        Some(Refused::Budget),
        "the budget is asked before the memo"
    );

    // No memo asked: `pass` as it always was.
    let open = settings(None);
    assert_eq!(
        pass_remembering(routing, true, &open, Some(APP), &requests, None)
            .expect("through")
            .memo,
        None
    );
}
