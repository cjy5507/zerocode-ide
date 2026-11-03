use super::*;

fn sized(kind: &str, label: &str, x: f64, y: f64, w: f64, h: f64) -> Value {
    json!({ "type": kind, "label": label, "rect": { "x": x, "y": y, "width": w, "height": h } })
}

fn teams_json() -> String {
    json!({ "account-a": [
        { "teamID": "TEAMPERSON", "teamName": "Synthetic Person", "teamType": "Individual", "isFreeProvisioningTeam": 0 },
        { "teamID": "TEAMCOMPNY", "teamName": "Synthetic Company", "teamType": "Company", "isFreeProvisioningTeam": 0 },
    ], "account-b": [
        { "teamID": "TEAMPERSON", "teamName": "Synthetic Person", "teamType": "Individual", "isFreeProvisioningTeam": 0 },
    ]})
    .to_string()
}

/// A phone's id is not its hardware id, and `--device` alone says which road a
/// verb takes: no simulator UDID or Android serial starts with the prefix.
#[test]
fn a_phones_id_hides_its_hardware_id_and_alone_names_the_road() {
    let handle = phone_handle("00008150-SYNTHETIC");
    assert!(is_real_phone(&handle));
    assert!(!handle.contains("00008150"));
    assert_eq!(handle, phone_handle("00008150-SYNTHETIC"));
    assert_ne!(handle, phone_handle("00008150-SYNTHETIC2"));
    assert_eq!(handle.len(), REAL_PHONE_PREFIX.len() + 12);
    for other in [
        "79356ECF-8AE5-4316-BC49-76702AE8DD03",
        "emulator-5554",
        "pixel",
    ] {
        assert!(!is_real_phone(other));
    }
}

/// 해시 불일치: a moved tag, other commit or other files stop before any patch,
/// and the sentence says nothing was built.
#[test]
fn a_fetch_with_any_other_hash_stops_before_the_patch() {
    assert_eq!(check_fetched(WDA_COMMIT, WDA_COMMIT, WDA_TREE), Ok(()));
    let other = "0000000000000000000000000000000000000000";
    for (tag, head, tree) in [
        (other, WDA_COMMIT, WDA_TREE),
        (WDA_COMMIT, other, WDA_TREE),
        (WDA_COMMIT, WDA_COMMIT, other),
    ] {
        let why = check_fetched(tag, head, tree).unwrap_err();
        assert!(why.starts_with(PHONE_SOURCE_MISMATCH), "{why}");
        assert!(why.contains("Nothing was patched or built"), "{why}");
    }
}

/// 패치 불일치: nothing is built, and the person is told.
#[test]
fn a_patch_that_does_not_fit_says_nothing_was_built_and_to_tell_the_person() {
    let said = patch_mismatch("does not apply");
    assert!(said.starts_with(PHONE_PATCH_MISMATCH));
    assert!(said.contains("Nothing was built"));
    assert!(said.contains("Tell the person"));
}

/// Only the four things that change the product rebuild it.
#[test]
fn only_the_patch_the_team_and_the_phones_os_rebuild() {
    let stamp = BuildStamp::new("patch", "TEAM", "26.5");
    assert_eq!(stamp, BuildStamp::new("patch", "TEAM", "26.5"));
    assert_ne!(stamp, BuildStamp::new("patch2", "TEAM", "26.5"));
    assert_ne!(stamp, BuildStamp::new("patch", "TEAM2", "26.5"));
    assert_ne!(stamp, BuildStamp::new("patch", "TEAM", "26.6"));
}

/// 팀이 여럿이면 물으며, a company team is not taken by being there.
#[test]
fn several_teams_and_no_name_is_a_question_and_a_named_one_is_the_only_answer() {
    let teams = parse_teams(&teams_json());
    assert_eq!(teams.len(), 2, "a team two accounts share is one team");
    let why = choose_team(&teams, None).unwrap_err();
    assert!(why.starts_with(PHONE_TEAM_REQUIRED), "{why}");
    assert!(why.contains("Synthetic Company") && why.contains("Synthetic Person"));
    assert_eq!(
        choose_team(&teams, Some("TEAMCOMPNY")).unwrap().kind,
        "Company"
    );
    assert!(choose_team(&teams, Some("NOSUCHTEAM")).is_err());
    let one = &teams[..1];
    assert_eq!(choose_team(one, None).unwrap().id, "TEAMPERSON");
    assert!(choose_team(&[], None).is_err());
    assert!(parse_teams("not json").is_empty());
}

/// 동의 문구: the card says it is a modified build, by whose signature, what it
/// lets agents do, what it never does, and how it ends.
#[test]
fn the_consent_card_says_it_is_a_patched_build_and_whose_signature_it_carries() {
    let team = &parse_teams(&teams_json())[0];
    let said = consent_sentence(team, "Synthetic Phone");
    for words in [
        WDA_TAG,
        "patch",
        "modified build",
        "Synthetic Person",
        "Synthetic Phone",
        "without the Mac's pointer",
        "never unlock",
        "password",
        "payment",
        "phone-remove",
        "Settings",
    ] {
        assert!(
            said.contains(words),
            "the card never says `{words}`: {said}"
        );
    }
}

/// 동의 없음: the refusal names the road, and the road is a card.
#[test]
fn no_consent_points_to_the_card_and_no_switch_an_agent_could_turn() {
    let said = not_allowed_sentence("phone-abc");
    assert!(said.starts_with(PHONE_NOT_ALLOWED));
    assert!(said.contains("phone-install --device phone-abc"));
    assert!(said.contains("card to the person"));
}

/// 잠금: the refusal is exact about which road is open now.
#[test]
fn a_locked_phone_says_exactly_which_road_is_open_now() {
    let all = locked_sentence(LockedRoads {
        mirroring_installed: true,
        mirroring_running: true,
        shared_desktop: true,
    });
    assert!(all.starts_with(PHONE_LOCKED));
    assert!(all.contains("nothing was pressed"));
    assert!(all.contains("never unlocks"));
    assert!(all.contains("com.apple.ScreenContinuity"));
    assert!(all.contains("may borrow the Mac's pointer"));

    let background = locked_sentence(LockedRoads {
        mirroring_installed: true,
        mirroring_running: false,
        shared_desktop: false,
    });
    assert!(background.contains("not running"));
    assert!(background.contains("refused until the person sets the shared desktop"));

    let none = locked_sentence(LockedRoads {
        mirroring_installed: false,
        mirroring_running: false,
        shared_desktop: true,
    });
    assert!(none.contains("no road is open now"));
    assert!(!none.contains("borrow"));
}

/// 숨은 요소: the size-zero copy of an icon leaves the tree; its sized twin stays,
/// and a sized child keeps a size-zero parent's place in the lineage.
#[test]
fn a_size_zero_element_leaves_the_tree_and_a_sized_one_under_it_stays() {
    let source = json!({
        "type": "Application", "label": "Synthetic",
        "rect": { "x": 0, "y": 0, "width": 402, "height": 874 },
        "children": [
            sized("Icon", "Weather", 0.0, 0.0, 0.0, 0.0),
            sized("Icon", "Weather", 27.0, 150.0, 60.0, 60.0),
            { "type": "Other", "rect": { "x": 0, "y": 0, "width": 0, "height": 0 },
              "children": [ sized("Button", "Inside", 10.0, 10.0, 44.0, 44.0) ] },
            { "type": "Other", "rect": { "x": 0, "y": 0, "width": 0, "height": 0 },
              "children": [ sized("Icon", "Gone", 0.0, 0.0, 0.0, 0.0) ] },
        ],
    });
    let tree = tree_from_source(&source, Some(SPRINGBOARD));
    let root = &tree[0];
    assert_eq!(root["AXUniqueId"], SPRINGBOARD);
    assert_eq!(root["frame"]["width"], 402.0);
    let children = root["children"].as_array().unwrap();
    assert_eq!(children.len(), 2, "{children:?}");
    assert_eq!(children[0]["type"], "Button", "an icon presses as a button");
    assert_eq!(children[0]["AXLabel"], "Weather");
    assert_eq!(children[0]["frame"]["x"], 27.0);
    assert_eq!(children[1]["children"][0]["AXLabel"], "Inside");
}

/// 비밀번호 칸: the tree's own type says so, with no label to trust; its value is
/// never carried; and a hidden copy does not count as a field on the screen.
#[test]
fn a_secure_field_is_known_by_its_type_and_its_value_is_never_carried() {
    assert!(is_secure_type("SecureTextField"));
    assert!(is_secure_type("XCUIElementTypeSecureTextField"));
    assert!(!is_secure_type("TextField"));
    let mut field = sized("SecureTextField", "Anything", 10.0, 10.0, 200.0, 44.0);
    field["value"] = json!("hunter2");
    let tree = tree_from_source(&field, None);
    assert_eq!(tree[0]["type"], "SecureTextField");
    assert!(tree[0].get("AXValue").is_none());
    assert!(has_visible_secure_field(&json!({
        "type": "Application", "rect": { "x": 0, "y": 0, "width": 1, "height": 1 },
        "children": [field],
    })));
    assert!(!has_visible_secure_field(&json!({
        "type": "Application",
        "children": [sized("SecureTextField", "Hidden", 0.0, 0.0, 0.0, 0.0)],
    })));
}

/// A plain field's value and an element that says it is disabled still read.
#[test]
fn a_value_and_a_disabled_control_still_read() {
    let mut field = sized("TextField", "To", 1.0, 2.0, 3.0, 4.0);
    field["value"] = json!("Synthetic");
    field["isEnabled"] = json!("0");
    let tree = tree_from_source(&field, None);
    assert_eq!(tree[0]["AXValue"], "Synthetic");
    assert_eq!(tree[0]["enabled"], false);
    assert_eq!(
        tree_from_source(&sized("Button", "Go", 1.0, 2.0, 3.0, 4.0), None)[0]["enabled"],
        true
    );
    assert_eq!(
        tree_from_source(&json!({ "type": "Other" }), None),
        json!([])
    );
}

#[test]
fn a_tap_and_a_swipe_are_one_fingers_w3c_actions() {
    let tap = tap_actions(113.0, 190.0);
    let steps = tap["actions"][0]["actions"].as_array().unwrap();
    assert_eq!(steps[0]["x"], 113.0);
    assert_eq!(steps[1]["type"], "pointerDown");
    assert_eq!(steps[3]["type"], "pointerUp");
    let swipe = swipe_actions((1.0, 2.0), (3.0, 4.0), 300);
    assert_eq!(swipe["actions"][0]["actions"][2]["duration"], 300);
    assert_eq!(swipe["actions"][0]["parameters"]["pointerType"], "touch");
}

#[test]
fn devicectl_gives_the_tunnel_address_and_nothing_for_other_hardware() {
    let json = json!({ "result": { "devices": [
        { "hardwareProperties": { "platform": "iOS", "udid": "U1" },
          "deviceProperties": { "name": "Synthetic Phone", "osVersionNumber": "26.5", "developerModeStatus": "enabled" },
          "connectionProperties": { "tunnelIPAddress": "fd00::1", "tunnelState": "connected" } },
        { "hardwareProperties": { "platform": "iOS", "udid": "U2" },
          "deviceProperties": { "name": "Away" }, "connectionProperties": { "tunnelState": "unavailable" } },
        { "hardwareProperties": { "platform": "watchOS", "udid": "U3" } },
    ]}})
    .to_string();
    let phones = phones_in_devicectl(&json);
    assert_eq!(phones.len(), 2);
    assert_eq!(phones[0].handle, phone_handle("U1"));
    assert_eq!(phones[0].address.as_deref(), Some("fd00::1"));
    assert_eq!(phones[0].os, "26.5");
    assert_eq!(phones[1].address, None);
    assert!(phones_in_devicectl("{").is_empty());
    assert_eq!(runner_base("fd00::1", WDA_PORT), "http://[fd00::1]:8100");
    assert_eq!(runner_base("127.0.0.1", 8123), "http://127.0.0.1:8123");
}

/// What the window cannot read is said as unknown, and is not called good.
#[test]
fn the_readiness_answer_keeps_bad_and_unknown_apart() {
    let answer = check_answer(vec![
        CheckItem::new("connected", Some(true), "yes"),
        CheckItem::new("phoneAutoLock", None, "unreadable"),
    ]);
    assert_eq!(answer["ready"], true);
    assert_eq!(answer["unknown"], json!(["phoneAutoLock"]));
    let bad = check_answer(vec![CheckItem::new("consent", Some(false), "no")]);
    assert_eq!(bad["ready"], false);
    assert_eq!(bad["bad"], json!(["consent"]));
}

#[test]
fn the_macs_sleep_and_a_profiles_end_are_read_from_their_tools() {
    let pmset =
        " standby  1\n sleep                0 (sleep prevented by x)\n displaysleep         10\n";
    assert_eq!(sleep_minutes(pmset, "sleep"), Some(0));
    assert_eq!(sleep_minutes(pmset, "displaysleep"), Some(10));
    assert_eq!(sleep_minutes(pmset, "nosuch"), None);
    let profile = "<key>ExpirationDate</key>\n\t<date>2027-01-02T03:04:05Z</date>\n";
    assert_eq!(
        profile_expiry(profile).as_deref(),
        Some("2027-01-02T03:04:05Z")
    );
    assert_eq!(profile_expiry("<plist/>"), None);
}

/// 겹친 요소: a row and a control inside it are one line of parents, so both keep
/// their centre; two controls outside each other's line over one point do not, and
/// a keyboard over a button takes the button's number away.
#[test]
fn only_something_outside_its_own_line_takes_a_centre_away() {
    let source = json!({
        "type": "Application", "rect": { "x": 0, "y": 0, "width": 400, "height": 800 },
        "children": [
            { "type": "Cell", "label": "Row", "rect": { "x": 0, "y": 100, "width": 400, "height": 80 },
              "children": [
                { "type": "Cell", "label": "Row", "rect": { "x": 0, "y": 100, "width": 400, "height": 80 },
                  "children": [ sized("Button", "Photo", 20.0, 120.0, 40.0, 40.0) ] },
              ] },
            sized("Button", "Under", 100.0, 300.0, 100.0, 100.0),
            sized("Button", "Over", 110.0, 310.0, 100.0, 100.0),
            sized("Button", "Hidden", 0.0, 600.0, 400.0, 40.0),
            { "type": "Keyboard", "rect": { "x": 0, "y": 500, "width": 400, "height": 300 },
              "children": [ sized("Button", "Return", 300.0, 700.0, 90.0, 50.0) ] },
            sized("Button", "Below", 100.0, 900.0, 100.0, 40.0),
        ],
    });
    let tree = tree_from_source(&source, None);
    let children = tree[0]["children"].as_array().unwrap();
    let row = &children[0];
    assert_eq!(row["hit_at_centre"], true, "{row}");
    assert_eq!(
        row["children"].as_array().unwrap().len(),
        1,
        "one row, not two"
    );
    assert_eq!(row["children"][0]["AXLabel"], "Photo");
    assert_eq!(row["children"][0]["hit_at_centre"], true);
    let by_name = |name: &str| {
        children
            .iter()
            .find(|child| child["AXLabel"] == name)
            .unwrap_or_else(|| panic!("{name}"))["hit_at_centre"]
            .clone()
    };
    assert_eq!(by_name("Under"), false, "another control holds its centre");
    assert_eq!(by_name("Over"), false, "and it holds this one's");
    assert_eq!(by_name("Hidden"), false, "the keyboard lies over it");
    assert_eq!(by_name("Below"), false, "off the screen");
    let keyboard = children
        .iter()
        .find(|child| child["type"] == "Keyboard")
        .unwrap();
    assert_eq!(keyboard["children"][0]["hit_at_centre"], true);
}

/// A long conversation's rows scroll under the navigation bar (the Messages
/// bench, 2026-11-03: its back button had no number in 16 of 20 rounds). The bar
/// lies over them: its back button keeps its number, a row behind the bar loses
/// its own there, and a keyboard or an alert still lies over the bar.
#[test]
fn a_bar_lies_over_the_rows_scrolled_under_it_and_an_overlay_over_the_bar() {
    let source = json!({
        "type": "Application", "rect": { "x": 0, "y": 0, "width": 402, "height": 874 },
        "children": [
            { "type": "NavigationBar", "rect": { "x": 0, "y": 61, "width": 402, "height": 97 },
              "children": [
                sized("Button", "Back", 16.0, 61.0, 44.0, 45.0),
                sized("Button", "+1 (888) 555-1212", 107.0, 116.0, 188.0, 34.0),
              ] },
            { "type": "Table", "rect": { "x": 0, "y": 0, "width": 402, "height": 874 },
              "children": [
                sized("Cell", "Under the bar", 0.0, 77.0, 402.0, 27.0),
                sized("Cell", "In the open", 0.0, 300.0, 402.0, 60.0),
              ] },
            { "type": "Alert", "rect": { "x": 0, "y": 100, "width": 402, "height": 200 },
              "children": [ sized("Button", "OK", 150.0, 200.0, 100.0, 40.0) ] },
        ],
    });
    let tree = tree_from_source(&source, None);
    let mut found = std::collections::HashMap::new();
    let mut pending = vec![&tree[0]];
    while let Some(node) = pending.pop() {
        if let Some(label) = node["AXLabel"].as_str() {
            found.insert(label.to_string(), node["hit_at_centre"].clone());
        }
        pending.extend(node["children"].as_array().into_iter().flatten());
    }
    assert_eq!(
        found["Back"], true,
        "the bar's button over a row: {found:?}"
    );
    assert_eq!(
        found["Under the bar"], false,
        "the bar lies over its centre"
    );
    assert_eq!(found["In the open"], true);
    assert_eq!(
        found["+1 (888) 555-1212"], false,
        "the alert lies over the bar"
    );
    assert_eq!(found["OK"], true);
}

/// A run's log tail becomes one of four sentences; a locked phone is told apart,
/// and nothing of the log itself goes out.
#[test]
fn a_runners_end_is_read_into_a_sentence_and_not_its_log() {
    assert_eq!(
        runner_failure("Failed to initialize for UI testing: LocalAuthentication -4"),
        RunnerFailure::Locked
    );
    assert_eq!(
        runner_failure("Untrusted Developer on device 0000"),
        RunnerFailure::Signature
    );
    assert_eq!(
        runner_failure("Developer Mode disabled"),
        RunnerFailure::DeveloperMode
    );
    assert_eq!(runner_failure("** TEST FAILED **"), RunnerFailure::Other);
    for failure in [
        RunnerFailure::Locked,
        RunnerFailure::Signature,
        RunnerFailure::DeveloperMode,
        RunnerFailure::Other,
    ] {
        assert!(!runner_failure_said(failure).is_empty());
    }
}

#[test]
fn a_dates_days_count_from_1970() {
    assert_eq!(iso_days("1970-01-01T00:00:00Z"), Some(0));
    assert_eq!(iso_days("2026-11-03"), Some(20_760));
    assert_eq!(iso_days("nonsense"), None);
}

/// 패치: the file in this repository is one hunk of one function of the pinned
/// source, carries its reason in numbers, and asks nothing to be downloaded.
#[test]
fn the_patch_is_one_hunk_of_one_file_and_says_why_in_numbers() {
    let patch = include_str!("../../../../zerocode-shell/src/emulator/wda/tap-no-wait.patch");
    assert_eq!(patch.matches("diff --git").count(), 1);
    assert_eq!(patch.matches("@@ -").count(), 1);
    assert!(patch.contains("+++ b/WebDriverAgentLib/Utilities/FBXCTestDaemonsProxy.m"));
    assert!(patch.contains("synthesizeEventWithRecord"));
    assert!(patch.contains("6 s") && patch.contains("0.8 s"));
    for word in ["http://", "https://", "curl", "system("] {
        assert!(!patch.contains(word), "{word}");
    }
}

/// 검수 t-91811 P3-4: the base is the address the runner itself printed, an IPv6
/// one without brackets included; a line with no port names no runner.
#[test]
fn the_runner_is_reached_where_it_said_it_listens() {
    assert_eq!(
        announced_base("t= ServerURLHere->http://fd00::1:8100<-ServerURLHere").as_deref(),
        Some("http://[fd00::1]:8100")
    );
    assert_eq!(
        announced_base("ServerURLHere->http://192.0.2.7:8101<-ServerURLHere\n").as_deref(),
        Some("http://192.0.2.7:8101")
    );
    assert_eq!(
        announced_base("ServerURLHere->http://[fd00::1]:8100<-ServerURLHere").as_deref(),
        Some("http://[fd00::1]:8100")
    );
    assert_eq!(announced_base("ServerURLHere->x"), None);
    assert_eq!(
        announced_base("ServerURLHere->http://fd00::1:port<-ServerURLHere"),
        None
    );
    assert_eq!(announced_base("no mark at all"), None);
}

/// 검수 t-91811 P2-5: a refusal tells the agent to pass it on and stop. The
/// mirroring road is the person's to ask for, never one to take on its own.
#[test]
fn a_refusal_never_sends_the_agent_to_mirroring_on_its_own() {
    let sentences = [
        not_allowed_sentence("phone-abc"),
        locked_sentence(LockedRoads {
            mirroring_installed: true,
            mirroring_running: true,
            shared_desktop: true,
        }),
    ];
    for said in &sentences {
        assert!(said.contains("stop"), "{said}");
        assert!(said.contains("only"), "{said}");
        assert!(said.contains("the person asks for it"), "{said}");
        for wrong in ["take the mirroring road", "use iPhone Mirroring or"] {
            assert!(!said.contains(wrong), "`{wrong}` in: {said}");
        }
    }
}

/// 검수 t-91811 P2-1·P3-6: the card says what lands in the person's Apple account,
/// that it is about this phone, and how to stop an agent pressing while they hold
/// the phone; the remove card says what stays.
#[test]
fn the_cards_say_what_lands_in_the_apple_account_and_how_to_stop() {
    let team = &parse_teams(&teams_json())[0];
    let said = consent_sentence(team, "Synthetic Phone");
    for words in [
        "App ID",
        "provisioning profile",
        "Apple account",
        "lock it",
        "this phone",
    ] {
        assert!(
            said.contains(words),
            "the card never says `{words}`: {said}"
        );
    }
    let removed = remove_sentence("Synthetic Phone");
    assert!(removed.contains("App ID"), "{removed}");
    assert!(removed.contains("Apple account"), "{removed}");
}
