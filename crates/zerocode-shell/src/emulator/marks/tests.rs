use super::*;
use serde_json::json;
use zerocode_core::computer_use::MARK_PIN_TOLERANCE_POINTS;

fn device(name: &str) -> Device {
    Device {
        address: name.into(),
        identity: name.into(),
    }
}

fn ios_tree() -> Value {
    serde_json::from_str(include_str!("fixtures/ios.json")).unwrap()
}

/// Captured 2026-09-21 from an iPhone simulator (iOS 26.5, 402x874 points)
/// through the helper's own `ax` request: the home screen, and Settings right
/// after `simctl launch com.apple.Preferences`. Times, battery and the
/// sign-in prompt are all they carry.
fn ios_home_tree() -> Value {
    serde_json::from_str(include_str!("fixtures/ios-home.json")).unwrap()
}

fn ios_settings_tree() -> Value {
    serde_json::from_str(include_str!("fixtures/ios-settings.json")).unwrap()
}

/// A screen whose accessibility graph is a DAG, as the exporter hands it over:
/// `settings.airdrop` is pinned above the list *and* listed inside it, so two
/// parents reference the one subview. The walk seats it under the first parent
/// that reaches it — `settings.list` therefore exports one child where it
/// declared two, and nothing at all is missing.
fn ios_dag_tree() -> Value {
    serde_json::from_str(include_str!("fixtures/ios-dag.json")).unwrap()
}

/// The same tree as the exporter gave it before it was asked about centres.
fn without_hit_tests(mut tree: Value) -> Value {
    fn strip(node: &mut Value) {
        if let Some(object) = node.as_object_mut() {
            object.remove(HIT_AT_CENTRE_KEY);
        }
        let children = match node {
            Value::Array(roots) => roots.iter_mut().collect::<Vec<_>>(),
            Value::Object(object) => object
                .get_mut("children")
                .and_then(Value::as_array_mut)
                .map(|children| children.iter_mut().collect())
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        for child in children {
            strip(child);
        }
    }
    strip(&mut tree);
    tree
}

fn marked_words<'a>(plan: &MarkPlan, faces: &'a [ElementFace]) -> Vec<(&'a str, &'a str)> {
    plan.marks
        .iter()
        .map(|placed| {
            let face = faces
                .iter()
                .find(|face| face.index == placed.element_index)
                .expect("a numbered face");
            (face.role.as_str(), face.words())
        })
        .collect()
}

#[test]
fn ios_settings_numbers_what_the_exporter_found_under_each_centre() {
    // Jev opened Settings from home (mark 10) and the next `marks` answered
    // 0 items three times over. The tree was whole — a heading, the sign-in
    // card, 「일반」, the toolbar's search field and its dictation button —
    // but with no z-order proof the numbering refused every centre another
    // candidate held (the floating search field holds 「일반」's), and the
    // sign-in card is over half the screen. The exporter now says what is on
    // top at each centre: the field and its button answer to themselves,
    // 「일반」 does not.
    let snapshot = Snapshot::ios(&ios_settings_tree()).unwrap();
    let plan = numbered(&snapshot.faces, snapshot.screen);
    assert_eq!(
        marked_words(&plan, &snapshot.faces),
        [("AXTextField", "검색"), ("AXButton", "받아쓰기")]
    );
    let general = snapshot
        .faces
        .iter()
        .find(|face| face.words() == "일반")
        .expect("the row under the search field");
    assert_eq!(general.seen().area(), 0.0);
    assert!(general.local().area() > 0.0);
    let field = snapshot
        .faces
        .iter()
        .find(|face| face.role == "AXTextField")
        .expect("the search field");
    // Window-local, like the frame it answers for.
    assert_eq!(field.seen(), field.local());
    // The tree as the exporter gave it before: no answer at any centre,
    // every claimed centre refused, and the screen unmarkable.
    let before = Snapshot::ios(&without_hit_tests(ios_settings_tree())).unwrap();
    assert!(before.faces.iter().all(|face| face.visible.is_none()));
    assert!(numbered(&before.faces, before.screen).marks.is_empty());
}

#[test]
fn ios_home_numbers_the_apps_and_leaves_the_covered_widget_faces_out() {
    let snapshot = Snapshot::ios(&ios_home_tree()).unwrap();
    let plan = numbered(&snapshot.faces, snapshot.screen);
    let marked = marked_words(&plan, &snapshot.faces);
    for app in [
        "사진",
        "미리 알림",
        "News",
        "건강",
        "지갑",
        "설정",
        "Safari",
        "메시지",
    ] {
        assert!(
            marked.contains(&("AXButton", app)),
            "{app} lost its number: {marked:?}"
        );
    }
    assert!(marked.contains(&("AXSlider", "검색")), "{marked:?}");
    // The widget stack's two faces answered to something else on top.
    let covered: Vec<&ElementFace> = snapshot
        .faces
        .iter()
        .filter(|face| face.seen().area() == 0.0)
        .collect();
    assert_eq!(covered.len(), 2, "{covered:?}");
    assert!(covered.iter().all(|face| face.height > 190.0));
    assert!(plan.marks.iter().all(|placed| {
        covered
            .iter()
            .all(|face| face.index != placed.element_index)
    }));
}

#[test]
fn ios_ax_fields_keep_their_evidence_and_leave_unknowns_empty() {
    let faces = faces(EmulatorPlatform::Ios, &ios_tree());
    let face = &faces[1];
    assert_eq!(face.index, 1);
    assert_eq!(face.role, "AXButton");
    assert_eq!(face.name.as_deref(), Some("일반"));
    assert_eq!(face.placeholder, None);
    assert_eq!(face.traits, Vec::<String>::new());
    assert_eq!(face.actions, Vec::<String>::new());
    assert_eq!(face.x, 20.0);
    assert_eq!(face.y, 80.0);
    assert_eq!(face.width, 120.0);
    assert_eq!(face.height, 44.0);
    assert!(face.signature.contains("settings.general"));
    assert_eq!(face.visible, None);
    assert_eq!(face.context.as_deref(), Some("설정"));
}

#[test]
fn android_compound_rows_preserve_their_descendant_words_and_pin_them() {
    // Android Settings exposes the press on an unnamed LinearLayout and
    // its actual title/summary on non-clickable TextView descendants.
    let row = |top, title, summary| {
        json!({
            "className": "android.widget.LinearLayout", "enabled": true,
            "clickable": true, "packageName": "com.android.settings",
            "bounds": {"left": 0, "top": top, "right": 1080, "bottom": top + 231},
            "children": [{"children": [
                {"className": "android.widget.TextView", "text": title,
                 "resourceId": "android:id/title", "clickable": false},
                {"className": "android.widget.TextView", "text": summary,
                 "resourceId": "android:id/summary", "clickable": false},
                {"clickable": true, "children": [{"text": "Separate action"}]}
            ]}]
        })
    };
    let mut tree = json!({"children": [
        row(770, "Network & internet", "Mobile, Wi-Fi, hotspot"),
        row(1001, "Connected devices", "Bluetooth, pairing")
    ]});
    let screen = Rect::new(0.0, 0.0, 1080.0, 2400.0);
    let original = Snapshot::new(EmulatorPlatform::Android, &tree, screen).unwrap();
    assert_eq!(
        original.faces[0].name.as_deref(),
        Some("Network & internet Mobile, Wi-Fi, hotspot")
    );
    let table = Table {
        platform: EmulatorPlatform::Android,
        device: device("phone"),
        plan: numbered(&original.faces, screen),
        screen,
        made: Instant::now(),
    };
    assert_eq!(table.plan.marks.len(), 2);
    let request = table.request(table.platform, &table.device, 1).unwrap();
    tree["children"][0]["children"][0]["children"][0]["text"] = json!("Apps");
    let changed = faces(EmulatorPlatform::Android, &tree);
    let error = request
        .perform(&changed, screen, |_, _| {
            panic!("a changed compound row was pressed")
        })
        .unwrap_err();
    assert_eq!(error.code, error_code::PIN_BROKEN);
}

#[test]
fn android_landscape_marks_and_pins_share_the_logical_input_coordinates() {
    // Unmodified CLI response from t-4836's 2400x1080 Display & touch
    // reproduction; the old wm-size frame (1080x2400) issued only one mark.
    let response: Value =
        serde_json::from_str(include_str!("fixtures/android-landscape.json")).unwrap();
    let tree = &response["result"];
    let physical = Rect::new(0.0, 0.0, 1080.0, 2400.0);
    let screen = Rect::new(0.0, 0.0, 2400.0, 1080.0);
    let snapshot = Snapshot::new(EmulatorPlatform::Android, tree, screen).unwrap();
    assert_eq!(numbered(&snapshot.faces, physical).marks.len(), 1);
    let table = Table {
        platform: EmulatorPlatform::Android,
        device: device("fixture"),
        plan: numbered(&snapshot.faces, screen),
        screen,
        made: Instant::now(),
    };
    let items = shared::items(&table.plan);
    // The existing overlap policy withholds the switch whose centre is
    // inside its clickable parent row: seven clickable nodes, six marks.
    assert_eq!(items.len(), 6);
    assert!(
        items
            .iter()
            .any(|item| item["label"] == "Display size and text")
    );
    for item in items {
        let mark = item["mark"].as_u64().unwrap() as usize;
        let request = table.request(table.platform, &table.device, mark).unwrap();
        let mut taps = Vec::new();
        request
            .perform(&snapshot.faces, screen, |x, y| {
                taps.push(super::super::android::device_point(x, y, (2400, 1080)));
                Ok(())
            })
            .unwrap();
        assert_eq!(
            taps,
            vec![(
                item["centerX"].as_f64().unwrap().round() as u32,
                item["centerY"].as_f64().unwrap().round() as u32
            )]
        );
        let error = request
            .perform(&snapshot.faces, physical, |_, _| {
                panic!("a pin from a different display geometry was pressed")
            })
            .unwrap_err();
        assert_eq!(error.code, error_code::PIN_BROKEN);
    }
}

#[test]
fn mobile_pin_refuses_changed_tree_before_the_tap() {
    let original = faces(EmulatorPlatform::Ios, &ios_tree());
    let face = &original[1];
    let screen = Rect::new(0.0, 0.0, 390.0, 844.0);
    let request = PinnedTap {
        index: face.index,
        pin: Pin {
            signature: face.signature.clone(),
            name: face.words().into(),
            context: face.context.clone().unwrap_or_default(),
            frame: face.local(),
            tolerance: MARK_PIN_TOLERANCE_POINTS,
        },
        screen,
        platform: EmulatorPlatform::Ios,
        made: Instant::now(),
        identity: "phone".into(),
    };
    let mut changed = original.clone();
    changed[1].name = Some("삭제".into());
    let mut taps = 0;
    let error = request
        .perform(&changed, screen, |_, _| {
            taps += 1;
            Ok(())
        })
        .unwrap_err();
    assert_eq!(error.code, "pin_broken");
    assert_eq!(taps, 0);
    request
        .perform(&original, screen, |x, y| {
            assert!((x - 80.0 / 390.0).abs() < f64::EPSILON);
            assert!((y - 102.0 / 844.0).abs() < f64::EPSILON);
            taps += 1;
            Ok(())
        })
        .unwrap();
    assert_eq!(taps, 1);
}

#[test]
fn a_stream_replaced_after_the_snapshot_cannot_finish_its_old_marked_tap() {
    use crate::emulator::session::{SessionKey, SessionRegistry, StartClaim};

    let (table, snapshot) = table();
    let request = table.request(table.platform, &table.device, 1).unwrap();
    let registry = Box::leak(Box::<SessionRegistry>::default());
    let descriptor = |stream: &str| crate::emulator::EmulatorStream {
        stream: stream.into(),
        udid: table.device.address.clone(),
        name: table.device.identity.clone(),
        platform: crate::emulator::EmulatorPlatform::Ios,
        interactive: true,
        reused: false,
    };
    let key = SessionKey::frames(
        crate::emulator::EmulatorPlatform::Ios,
        &table.device.address,
    );
    let StartClaim::Acquired(lease) = registry.claim(key.clone()).unwrap() else {
        panic!("expected a new stream");
    };
    let old = registry.new_control(&descriptor("old"));
    lease.activate(descriptor("old"), old.clone());
    let input = old.input().unwrap();
    // The backend has just captured `snapshot` under this gate. Replacing
    // the stream must revoke that proof even when the exported face is equal.
    assert!(registry.stop("old"));
    let StartClaim::Acquired(lease) = registry.claim(key).unwrap() else {
        panic!("expected the replacement stream");
    };
    let current = registry.new_control(&descriptor("new"));
    lease.activate(descriptor("new"), current.clone());
    let mut taps = 0;
    let error = request
        .perform_in(&input, &snapshot, table.screen, |_, _| {
            taps += 1;
            Ok(())
        })
        .unwrap_err();
    assert_eq!(error.code, error_code::PIN_BROKEN);
    assert_eq!(taps, 0);
    drop(input);
    let next = current.input().unwrap();
    request
        .perform_in(&next, &snapshot, table.screen, |_, _| {
            taps += 1;
            Ok(())
        })
        .unwrap();
    assert_eq!(taps, 1);
}

#[test]
fn ios_value_is_not_invented_as_a_placeholder_and_changes_break_identity() {
    let mut tree = ios_tree();
    tree[0]["children"][0]["AXLabel"] = Value::Null;
    tree[0]["children"][0]["AXValue"] = json!("켜짐");
    let before = faces(EmulatorPlatform::Ios, &tree);
    assert_eq!(before[1].name.as_deref(), Some("켜짐"));
    assert_eq!(before[1].placeholder, None);
    tree[0]["children"][0]["AXValue"] = json!("꺼짐");
    assert_ne!(
        before[1].signature,
        faces(EmulatorPlatform::Ios, &tree)[1].signature
    );
}

fn table() -> (Table, Vec<ElementFace>) {
    let snapshot = Snapshot::ios(&ios_tree()).unwrap();
    (
        Table {
            platform: EmulatorPlatform::Ios,
            device: device("phone"),
            plan: numbered(&snapshot.faces, snapshot.screen),
            screen: snapshot.screen,
            made: Instant::now(),
        },
        snapshot.faces,
    )
}

#[test]
fn mobile_marks_stop_before_a_persons_guarded_step() {
    for kind in zerocode_core::computer_use::ConfirmKind::ALL {
        let (mut held, mut faces) = table();
        faces[1].name = Some(kind.words()[0].into());
        held.plan = numbered(&faces, held.screen);
        let pin = held
            .request(EmulatorPlatform::Ios, &device("phone"), 1)
            .unwrap();
        let mut taps = 0;
        let error = pin
            .perform_with_policy(
                &faces,
                held.screen,
                crate::computer_use::confirm::Policy::default(),
                |_, _| {
                    taps += 1;
                    Ok(())
                },
            )
            .unwrap_err();
        assert_eq!(error.code, error_code::CONFIRMATION_REQUIRED);
        assert_eq!(taps, 0);
        pin.perform_with_policy(
            &faces,
            held.screen,
            crate::computer_use::confirm::Policy {
                payment: false,
                transfer: false,
                delete: false,
            },
            |_, _| {
                taps += 1;
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(taps, 1);
    }
}

#[test]
fn mobile_pin_rechecks_identity_geometry_eligibility_and_uniqueness() {
    let (table, original) = table();
    let request = table.request(table.platform, &table.device, 1).unwrap();
    let mut variants = Vec::new();
    for field in [
        "signature",
        "name",
        "context",
        "x",
        "y",
        "width",
        "height",
        "disabled",
        "gone",
        "duplicate",
    ] {
        let mut changed = original.clone();
        let face = &mut changed[1];
        match field {
            "signature" => face.signature.push('!'),
            "name" => face.name = Some("다른 항목".into()),
            "context" => face.context = Some("다른 행".into()),
            "x" => face.x += MARK_PIN_TOLERANCE_POINTS + 1.0,
            "y" => face.y += MARK_PIN_TOLERANCE_POINTS + 1.0,
            "width" => face.width += MARK_PIN_TOLERANCE_POINTS + 1.0,
            "height" => face.height += MARK_PIN_TOLERANCE_POINTS + 1.0,
            "disabled" => face.traits.push("disabled".into()),
            "gone" => {
                changed.pop();
            }
            "duplicate" => {
                let mut duplicate = face.clone();
                duplicate.index += 1;
                duplicate.y += 80.0;
                changed.push(duplicate);
            }
            _ => unreachable!(),
        }
        variants.push((field, changed));
    }
    for (field, changed) in variants {
        let error = request
            .perform(&changed, table.screen, |_, _| {
                panic!("pressed after {field}")
            })
            .unwrap_err();
        assert_eq!(error.code, error_code::PIN_BROKEN, "{field}");
    }
}

#[test]
fn tolerance_uses_the_fresh_center_and_screen_changes_fail_closed() {
    let (table, mut current) = table();
    let request = table.request(table.platform, &table.device, 1).unwrap();
    current[1].x += MARK_PIN_TOLERANCE_POINTS;
    request
        .perform(&current, table.screen, |x, _| {
            assert!((x - current[1].local().mid_x() / table.screen.width).abs() < f64::EPSILON);
            Ok(())
        })
        .unwrap();
    let rotated = Rect::new(0.0, 0.0, table.screen.height, table.screen.width);
    assert_eq!(
        request
            .perform(&current, rotated, |_, _| panic!("rotated device pressed"))
            .unwrap_err()
            .code,
        error_code::PIN_BROKEN
    );
}

#[test]
fn looks_are_bound_to_the_device_platform_age_and_their_own_numbers() {
    let (mut table, _) = table();
    assert!(
        table
            .request(EmulatorPlatform::Android, &table.device, 1)
            .is_err()
    );
    assert!(
        table
            .request(table.platform, &device("another-phone"), 1)
            .is_err()
    );
    assert!(table.request(table.platform, &table.device, 0).is_err());
    assert!(table.request(table.platform, &table.device, 2).is_err());
    table.made -= cache::MAX_AGE + std::time::Duration::from_secs(1);
    assert!(table.request(table.platform, &table.device, 1).is_err());
}

#[test]
fn an_android_serial_reused_by_another_avd_cannot_inherit_a_look() {
    let devices = |avd| json!([{"avd": avd, "serial": "emulator-5554", "booted": true}]);
    let resolve = crate::agent_tools_runtime::android_mark_target;
    let before = resolve(&devices("first-avd"), "first-avd").unwrap();
    let after = resolve(&devices("second-avd"), "emulator-5554").unwrap();
    assert_eq!(before.address, after.address);
    let (mut table, _) = table();
    table.platform = EmulatorPlatform::Android;
    table.device = before;
    let error = table
        .request(table.platform, &after, 1)
        .err()
        .expect("a different AVD cannot inherit the old look");
    assert_eq!(error.code, error_code::PIN_BROKEN);
    assert!(resolve(&devices("second-avd"), "first-avd").is_err());
    assert!(resolve(&devices(""), "emulator-5554").is_err());
    let moved = resolve(
        &json!([{"avd": "first-avd", "serial": "emulator-5556", "booted": true}]),
        "first-avd",
    )
    .unwrap();
    assert!(
        table.request(table.platform, &moved, 1).is_err(),
        "stable AVD identity must not discard the observed transport binding"
    );
}

#[test]
fn a_device_replaced_after_resolution_cannot_reach_the_marked_tap() {
    let (table, faces) = table();
    let request = table.request(table.platform, &table.device, 1).unwrap();
    let mut taps = 0;
    let result = request.on_device("another-device", || {
        request.perform(&faces, table.screen, |_, _| {
            taps += 1;
            Ok(())
        })
    });
    assert_eq!(result.unwrap_err().code, error_code::PIN_BROKEN);
    assert_eq!(taps, 0);
}

/// A subview two parents share costs the screen nothing.
///
/// The exporter seats such an element once, so the parent that reached it
/// second shows fewer children than it declared. Counting that as a cut
/// subtree made `Snapshot` refuse a screen that was exported whole (t-5445);
/// the exporter's own rule is pinned in `tests/source_contracts`. Here: the
/// tree is taken, the shared cell carries exactly one number, and the same
/// tree marked truncated is still refused — that word still means
/// observations the export does not have.
#[test]
fn a_subview_two_parents_share_is_numbered_once_and_never_refuses_the_screen() {
    let snapshot = Snapshot::ios(&ios_dag_tree()).unwrap();
    let plan = numbered(&snapshot.faces, snapshot.screen);
    assert_eq!(
        marked_words(&plan, &snapshot.faces),
        [
            ("AXButton", "AirDrop"),
            ("AXButton", "기내 모드"),
            ("AXButton", "일반")
        ]
    );
    let shared: Vec<&ElementFace> = snapshot
        .faces
        .iter()
        .filter(|face| face.words() == "AirDrop")
        .collect();
    assert_eq!(
        shared.len(),
        1,
        "the shared cell is seated twice: {shared:?}"
    );
    // What the exporter used to say about that second parent, and what the
    // word must go on meaning: a subtree it really could not walk.
    let mut cut = ios_dag_tree();
    cut[0]["children"][1]["truncated"] = json!(true);
    assert!(Snapshot::ios(&cut).unwrap_err().contains("truncated"));
}

#[test]
fn malformed_frames_and_truncated_trees_cannot_supply_a_mark() {
    let mut tree = ios_tree();
    tree[0]["children"][0]["truncated"] = json!(true);
    assert!(Snapshot::ios(&tree).unwrap_err().contains("truncated"));
    tree[0]["truncated"] = json!(true);
    assert!(Snapshot::ios(&tree).is_err());
    assert!(Snapshot::ios(&json!([])).is_err());
    for width in [0.0, -1.0, f64::INFINITY, f64::NAN] {
        assert!(
            Snapshot::new(
                EmulatorPlatform::Ios,
                &ios_tree(),
                Rect::new(0.0, 0.0, width, 844.0)
            )
            .is_err()
        );
    }
}

#[test]
fn marks_share_the_wire_words_and_the_model_only_reads_the_legend() {
    let (table, _) = table();
    let answer = table.answer("L1");
    assert_eq!(answer[LOOK_ID_KEY], "L1");
    assert_eq!(answer[ITEMS_KEY][0]["mark"], 1);
    assert!(answer[LEGEND_KEY].as_str().unwrap().contains("일반"));
    let text = crate::agent_tools_runtime::emulator_pretty(
        zerocode_core::computer_use::EmulatorMethod::Marks,
        &answer,
    );
    for key in shared::TOOL_ONLY_KEYS {
        assert!(!text.contains(key), "{key}: {text}");
    }
    assert!(text.contains("L1") && text.contains("일반"));
}

#[test]
fn disabled_or_unknown_ios_nodes_never_acquire_a_number() {
    let mut tree = ios_tree();
    tree[0]["children"][0]["enabled"] = json!(false);
    let snapshot = Snapshot::ios(&tree).unwrap();
    assert!(numbered(&snapshot.faces, snapshot.screen).marks.is_empty());
    tree[0]["children"][0]["enabled"] = json!(true);
    tree[0]["children"][0]["type"] = json!("Group");
    let snapshot = Snapshot::ios(&tree).unwrap();
    assert!(numbered(&snapshot.faces, snapshot.screen).marks.is_empty());
}

#[test]
fn android_mark_center_round_trips_through_the_existing_tap_conversion() {
    let (mut table, faces) = table();
    table.platform = EmulatorPlatform::Android;
    let request = table.request(table.platform, &table.device, 1).unwrap();
    request
        .perform(&faces, table.screen, |x, y| {
            assert_eq!(
                crate::emulator::android::device_point(x, y, (390, 844)),
                (80, 102)
            );
            Ok(())
        })
        .unwrap();
}

#[test]
fn a_request_that_expires_during_the_tree_read_never_taps() {
    let (table, faces) = table();
    let mut request = table.request(table.platform, &table.device, 1).unwrap();
    request.made -= cache::MAX_AGE + std::time::Duration::from_secs(1);
    assert_eq!(
        request
            .perform(&faces, table.screen, |_, _| panic!("expired look pressed"))
            .unwrap_err()
            .code,
        error_code::PIN_BROKEN
    );
}

#[test]
fn a_replaced_parent_with_the_same_label_cannot_inherit_the_childs_pin() {
    let (table, _) = table();
    let request = table.request(table.platform, &table.device, 1).unwrap();
    let mut tree = ios_tree();
    tree[0]["AXUniqueId"] = json!("another.application");
    let fresh = Snapshot::ios(&tree).unwrap();
    let mut taps = 0;
    let error = request
        .perform(&fresh.faces, fresh.screen, |_, _| {
            taps += 1;
            Ok(())
        })
        .unwrap_err();
    assert_eq!(error.code, error_code::PIN_BROKEN);
    assert_eq!(taps, 0);
}

#[test]
fn an_overlapping_search_field_cannot_leave_a_number_on_the_row_under_it() {
    // Observed on the iOS Settings screen with large accessibility text:
    // General's centre was inside the floating search field's AX rectangle.
    // The tree has no z-order or clipping proof, so neither centre is safe.
    let (table, _) = table();
    let request = table.request(table.platform, &table.device, 1).unwrap();
    let mut tree = ios_tree();
    tree[0]["children"].as_array_mut().unwrap().push(json!({
        "AXLabel": "검색", "AXUniqueId": "search", "type": "TextField",
        "enabled": true,
        "frame": {"x": 10, "y": 75, "width": 200, "height": 44},
        "children": []
    }));
    let current = Snapshot::ios(&tree).unwrap();
    assert!(numbered(&current.faces, current.screen).marks.is_empty());
    let mut taps = 0;
    let error = request
        .perform(&current.faces, current.screen, |_, _| {
            taps += 1;
            Ok(())
        })
        .unwrap_err();
    assert_eq!(error.code, error_code::PIN_BROKEN);
    assert_eq!(taps, 0);
}

mod cli_answers {
    use crate::agent_tools_runtime::emulator_answer;
    use serde_json::json;
    use zerocode_core::computer_use::EmulatorMethod;
    use zerocode_core::computer_use_protocol::marks::{ITEMS_KEY, LEGEND_KEY, LOOK_ID_KEY};

    #[test]
    fn marks_json_is_parseable_and_carries_the_external_content_flag() {
        let answer = emulator_answer(
            EmulatorMethod::Marks,
            true,
            Ok(json!({
                LOOK_ID_KEY: "L1", ITEMS_KEY: [], LEGEND_KEY: "1 button 설정 @80,102",
            })),
        );
        assert_eq!(answer.exit_code, 0);
        let value: serde_json::Value = serde_json::from_str(&answer.stdout).unwrap();
        assert_eq!(value["result"][zerocode_core::untrusted::JSON_FLAG], true);
        assert_eq!(value["result"][LOOK_ID_KEY], "L1");
    }

    #[test]
    fn a_mobile_pin_refusal_keeps_its_code_on_the_cli_wire() {
        let answer = emulator_answer(
            EmulatorMethod::Click,
            true,
            Err(zerocode_core::computer_use_protocol::ProviderError::new(
                zerocode_core::computer_use_protocol::error_code::PIN_BROKEN,
                "changed",
            )),
        );
        assert_eq!(answer.exit_code, 1);
        let value: serde_json::Value = serde_json::from_str(&answer.stderr).unwrap();
        assert_eq!(value["error"]["code"], "pin_broken");
    }
}
