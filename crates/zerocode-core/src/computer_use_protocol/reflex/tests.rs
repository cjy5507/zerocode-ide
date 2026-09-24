use super::*;

#[derive(Deserialize)]
struct Golden {
    expected: String,
    plan: serde_json::Value,
}

fn golden(name: &str) -> Golden {
    let path = format!(
        "{}/fixtures/reflex-contract/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

#[test]
fn shared_golden_runs_through_the_real_validator() {
    let manifest: serde_json::Value = serde_json::from_str(include_str!(
        "../../../fixtures/reflex-contract/manifest.json"
    ))
    .unwrap();
    let cases = manifest["semantic"].as_array().unwrap();
    // Every case is judged before the assertion, so a failure names them all.
    let mut mismatches = Vec::new();
    for name in cases {
        let name = name.as_str().unwrap();
        let row = golden(name);
        let bytes = serde_json::to_vec(&row.plan).unwrap();
        let got = match decode_wire(&bytes) {
            Ok(_) => "ok".to_string(),
            Err(err) => format!("{err:?}").to_lowercase(),
        };
        if got != row.expected {
            mismatches.push(format!("{name}: expected {}, got {got}", row.expected));
        }
    }
    assert!(mismatches.is_empty(), "{mismatches:#?}");
    assert_eq!(cases.len(), 31);
    for name in manifest["wire_negative"].as_array().unwrap() {
        let name = name.as_str().unwrap();
        let path = format!(
            "{}/fixtures/reflex-contract/{name}.txt",
            env!("CARGO_MANIFEST_DIR")
        );
        assert_eq!(
            decode_wire(&std::fs::read(path).unwrap()).unwrap_err(),
            ReflexError::Wire,
            "{name}"
        );
    }
    assert_eq!(manifest["wire_negative"].as_array().unwrap().len(), 9);
}

#[test]
fn shared_lease_cases_exercise_permits_and_frame_cursor() {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../../fixtures/reflex-contract/lease_cases.json"
    ))
    .unwrap();
    // A case replaces top-level fields of the shared base frame or lease.
    let patched = |base: &serde_json::Value, patch: &serde_json::Value| {
        let mut value = base.clone();
        value
            .as_object_mut()
            .unwrap()
            .extend(patch.as_object().unwrap().clone());
        value
    };
    let mut mismatches = Vec::new();
    for case in fixture["cases"].as_array().unwrap() {
        let frame: FrameFacts =
            serde_json::from_value(patched(&fixture["frame"], &case["frame"])).unwrap();
        let lease: ActionLease =
            serde_json::from_value(patched(&fixture["lease"], &case["lease"])).unwrap();
        let input: LeaseInput = serde_json::from_value(case["input"].clone()).unwrap();
        let got = lease.permits(&frame, case["now_host_ns"].as_u64().unwrap(), input);
        if got != case["expected"].as_bool().unwrap() {
            mismatches.push(format!(
                "permits {}: got {got}",
                case["name"].as_str().unwrap()
            ));
        }
    }
    for case in fixture["cursor_cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let frames = case["frames"].as_array().unwrap();
        let expected: Vec<bool> = case["expected"]
            .as_array()
            .unwrap()
            .iter()
            .map(|verdict| verdict.as_bool().unwrap())
            .collect();
        assert_eq!(frames.len(), expected.len(), "{name}");
        let mut cursor = FrameCursor::default();
        let got: Vec<bool> = frames
            .iter()
            .map(|patch| {
                let frame: FrameFacts =
                    serde_json::from_value(patched(&fixture["frame"], patch)).unwrap();
                cursor.observe(&frame)
            })
            .collect();
        if got != expected {
            mismatches.push(format!(
                "observe {name}: got {got:?}, expected {expected:?}"
            ));
        }
    }
    assert!(mismatches.is_empty(), "{mismatches:#?}");
}

#[test]
fn unknown_features_cannot_authorize_actions() {
    let unknown = None;
    for predicate in [
        Predicate::Not {
            child: Box::new(Predicate::Known),
        },
        Predicate::Any {
            children: vec![Predicate::Eq { value: 1 }],
        },
        Predicate::All {
            children: vec![Predicate::Eq { value: 1 }],
        },
    ] {
        assert_eq!(predicate.evaluate(unknown), Truth::Unknown);
    }
    assert_eq!(
        Predicate::Any {
            children: vec![Predicate::Eq { value: 1 }, Predicate::Eq { value: 2 }]
        }
        .evaluate(Some(2)),
        Truth::True
    );
    assert_eq!(
        Predicate::All {
            children: vec![Predicate::Eq { value: 1 }, Predicate::Eq { value: 2 }]
        }
        .evaluate(Some(1)),
        Truth::False
    );
}

#[test]
fn a_new_frame_keeps_a_lease_but_an_epoch_change_revokes_it() {
    let frame = FrameFacts {
        run_id: "owned".into(),
        display_id: "fixture".into(),
        region: Roi {
            x: 0,
            y: 0,
            width: 32,
            height: 32,
            space: CoordinateSpace::Pixel,
        },
        pixel_extent: PixelExtent {
            width: 32,
            height: 32,
        },
        point_transform: PointTransform {
            origin_x: 0,
            origin_y: 0,
            points_per_pixel: Scale {
                numerator: 1,
                denominator: 1,
            },
        },
        orientation: Orientation::Up,
        color_space: ColorSpace::Srgb,
        status: FrameStatus::Ready,
        dirty: true,
        capture_gap: 0,
        delivered_host_ns: Some(91),
        capture_seq: 10,
        repaint_seq: 7,
        stream_epoch: 1,
        owner_epoch: 1,
        geometry_epoch: 1,
        plan_epoch: 1,
        clock_domain: 1,
        captured_host_ns: Some(90),
    };
    let lease = ActionLease {
        run_id: "owned".into(),
        action_id: "click1".into(),
        target_id: "ball".into(),
        target_roi: Roi {
            x: 0,
            y: 0,
            width: 32,
            height: 32,
            space: CoordinateSpace::Pixel,
        },
        allowed_inputs: BTreeSet::from([LeaseInput::PointerMove]),
        owner_epoch: 1,
        stream_epoch: 1,
        geometry_epoch: 1,
        plan_epoch: 1,
        clock_domain: 1,
        source_capture_seq: 9,
        issued_host_ns: 80,
        valid_until_host_ns: 120,
        target_proof_until_host_ns: 110,
        max_children: 2,
        used_children: 0,
    };
    assert!(lease.permits(&frame, 100, LeaseInput::PointerMove));
    assert!(lease.permits(
        &FrameFacts {
            capture_seq: 11,
            ..frame.clone()
        },
        100,
        LeaseInput::PointerMove
    ));
    assert!(!lease.permits(
        &FrameFacts {
            owner_epoch: 2,
            ..frame.clone()
        },
        100,
        LeaseInput::PointerMove
    ));
    assert!(!lease.permits(
        &FrameFacts {
            capture_seq: 8,
            ..frame.clone()
        },
        100,
        LeaseInput::PointerMove
    ));
    assert!(!lease.permits(&frame, 110, LeaseInput::PointerMove));
    assert!(!lease.permits(
        &FrameFacts {
            clock_domain: 2,
            ..frame.clone()
        },
        100,
        LeaseInput::PointerMove
    ));
    assert!(!lease.permits(&frame, 100, LeaseInput::LeftClick));
}

#[test]
fn windows_reflex_is_unsupported_without_a_live_frame_provider() {
    for surface in [
        Surface::MacosDesktop,
        Surface::IosDevice,
        Surface::WindowsDesktop,
    ] {
        assert_eq!(
            capability(surface),
            ReflexCapability {
                schema_version: VERSION,
                live_reflex: false
            }
        );
    }
}

#[test]
fn canonical_wire_is_strict_and_preserves_the_validated_plan() {
    let row = golden("valid_basic");
    let original = serde_json::to_vec(&row.plan).unwrap();
    let typed: ReflexPlan = serde_json::from_value(row.plan).unwrap();
    let bytes = wire_bytes(&typed);
    assert_eq!(bytes, original);
    assert_eq!(decode_wire(&bytes).unwrap().plan(), &typed);
    let duplicate = String::from_utf8(bytes.clone())
        .unwrap()
        .replace("\"version\":1", "\"version\":1,\"version\":1");
    assert_eq!(
        decode_wire(duplicate.as_bytes()).unwrap_err(),
        ReflexError::Wire
    );
    for bad in [
        bytes
            .iter()
            .copied()
            .chain(b" ".iter().copied())
            .collect::<Vec<_>>(),
        String::from_utf8(bytes.clone())
            .unwrap()
            .replace("\"x\":0", "\"x\":NaN")
            .into_bytes(),
        String::from_utf8(bytes.clone())
            .unwrap()
            .replace("\"x\":0", "\"x\":-0")
            .into_bytes(),
    ] {
        assert_eq!(decode_wire(&bad).unwrap_err(), ReflexError::Wire);
    }
    assert_eq!(
        serde_json::from_str::<ReflexLimits>(include_str!(
            "../../../fixtures/reflex-contract/limits.json"
        ))
        .unwrap(),
        LIMITS
    );
}

#[test]
fn duplicate_or_regressed_capture_is_not_fresh_even_if_repaint_is_unchanged() {
    let base = FrameFacts {
        run_id: "owned".into(),
        display_id: "fixture".into(),
        region: Roi {
            x: 0,
            y: 0,
            width: 32,
            height: 32,
            space: CoordinateSpace::Pixel,
        },
        pixel_extent: PixelExtent {
            width: 32,
            height: 32,
        },
        point_transform: PointTransform {
            origin_x: 0,
            origin_y: 0,
            points_per_pixel: Scale {
                numerator: 1,
                denominator: 1,
            },
        },
        orientation: Orientation::Up,
        color_space: ColorSpace::Srgb,
        status: FrameStatus::Ready,
        dirty: true,
        capture_gap: 0,
        delivered_host_ns: Some(91),
        capture_seq: 1,
        repaint_seq: 1,
        stream_epoch: 1,
        owner_epoch: 1,
        geometry_epoch: 1,
        plan_epoch: 1,
        clock_domain: 1,
        captured_host_ns: Some(1),
    };
    let mut cursor = FrameCursor::default();
    assert!(cursor.observe(&base));
    assert!(!cursor.observe(&base));
    assert!(cursor.observe(&FrameFacts {
        capture_seq: 2,
        ..base.clone()
    }));
    assert!(!cursor.observe(&FrameFacts {
        capture_seq: 3,
        repaint_seq: 0,
        ..base.clone()
    }));
    assert!(cursor.observe(&FrameFacts {
        stream_epoch: 2,
        ..base
    }));
}
