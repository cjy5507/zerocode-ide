use super::*;

struct Golden {
    expected: String,
    plan: serde_json::Value,
    /// The plan's canonical bytes exactly as the shared file holds them.
    wire: Vec<u8>,
}

fn golden(name: &str) -> Golden {
    #[derive(Deserialize)]
    struct Row {
        expected: String,
        plan: serde_json::Value,
    }
    let path = format!(
        "{}/fixtures/reflex-contract/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let raw = std::fs::read(path).unwrap();
    let row: Row = serde_json::from_slice(&raw).unwrap();
    // Every file is canonical JSON and a final newline, so the plan's wire is
    // the file's own bytes inside the envelope: the expected bytes never pass
    // through a serializer whose map order the build decides.
    let prefix = format!("{{\"expected\":\"{}\",\"plan\":", row.expected);
    let wire = raw
        .strip_prefix(prefix.as_bytes())
        .and_then(|rest| rest.strip_suffix(b"}\n"))
        .unwrap_or_else(|| panic!("{name} is not written canonical"))
        .to_vec();
    Golden {
        expected: row.expected,
        plan: row.plan,
        wire,
    }
}

/// With serde_json's `preserve_order` a `Map` keeps insertion order; without
/// it the map sorts. Cargo turns the feature on for a whole workspace build.
fn map_keeps_insertion_order() -> bool {
    let probe: serde_json::Map<String, serde_json::Value> = [("b", 0), ("a", 0)]
        .into_iter()
        .map(|(key, value)| (key.to_string(), value.into()))
        .collect();
    probe.keys().next().map(String::as_str) == Some("b")
}

/// The same value with every object's keys inserted in reverse order, at
/// every depth; arrays keep their order.
fn reversed(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => serde_json::Value::Object(
            map.iter()
                .rev()
                .map(|(key, item)| (key.clone(), reversed(item)))
                .collect(),
        ),
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.iter().map(reversed).collect())
        }
        leaf => leaf.clone(),
    }
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
        let got = match decode_wire(&row.wire) {
            Ok(validated) => {
                if wire_bytes(validated.plan()) != row.wire {
                    mismatches.push(format!("{name}: re-encoded wire differs"));
                }
                "ok".to_string()
            }
            Err(err) => format!("{err:?}").to_lowercase(),
        };
        if got != row.expected {
            mismatches.push(format!("{name}: expected {}, got {got}", row.expected));
        }
    }
    assert!(mismatches.is_empty(), "{mismatches:#?}");
    // R1's 32 cases under v2, the six v2 adds, the identifier bound's two
    // sides (t-9205), and a detector's pick with the R4 bench's plan (t-10242).
    assert_eq!(cases.len(), 49);
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
    assert_eq!(manifest["wire_negative"].as_array().unwrap().len(), 11);
}

#[test]
fn map_insertion_order_never_reaches_the_wire_or_the_hash() {
    let keeps_order = map_keeps_insertion_order();
    for name in ["valid_basic", "valid_nested_predicate"] {
        let row = golden(name);
        let turned = reversed(&row.plan);
        assert_eq!(canonical_json(&row.plan), row.wire, "{name}");
        assert_eq!(canonical_json(&turned), row.wire, "{name}");
        let typed: ReflexPlan = serde_json::from_value(turned.clone()).unwrap();
        assert_eq!(wire_bytes(&typed), row.wire, "{name}");
        assert_eq!(plan_hash(&typed), typed.plan_hash, "{name}");
        // Written as the map holds it: reordered when the build keeps insertion
        // order, and then refused rather than normalized and run.
        let as_held = serde_json::to_vec(&turned).unwrap();
        if keeps_order {
            assert_ne!(as_held, row.wire, "{name}");
            assert_eq!(decode_wire(&as_held).unwrap_err(), ReflexError::Wire);
        } else {
            assert_eq!(as_held, row.wire, "{name}");
        }
    }
    // Inserted out of order at both depths; integers at both ends stay exact.
    let exact = serde_json::json!({"z": u64::MAX, "a": [i64::MIN, {"y": 0, "b": -1}]});
    assert_eq!(
        canonical_json(&exact),
        br#"{"a":[-9223372036854775808,{"b":-1,"y":0}],"z":18446744073709551615}"#
    );
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

/// A display stream stamps a frame with the time the display shows it, which
/// can run ahead of its delivery (t-10127): the frame is in hand at the
/// earlier of the two, and a delivery never stands in for an unknown capture.
#[test]
fn a_frame_is_observed_no_later_than_its_delivery() {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../../fixtures/reflex-contract/lease_cases.json"
    ))
    .unwrap();
    let base: FrameFacts = serde_json::from_value(fixture["frame"].clone()).unwrap();
    let observed = |captured: Option<u64>, delivered: Option<u64>| {
        FrameFacts {
            captured_host_ns: captured,
            delivered_host_ns: delivered,
            ..base.clone()
        }
        .observed_host_ns()
    };
    for (captured, delivered, expected, why) in [
        (Some(103), Some(100), Some(100), "ahead of its delivery"),
        (Some(90), Some(91), Some(90), "captured, then delivered"),
        (Some(103), None, Some(103), "no delivery to bound it"),
        (None, Some(100), None, "the delivery never stands in"),
    ] {
        assert_eq!(observed(captured, delivered), expected, "{why}");
    }
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

/// Live reflex is claimed where a live frame provider feeds a run — the
/// macOS desktop's eye — and nowhere else: the Windows desktop and the iOS
/// device have none a run reads, so their rows claim nothing; and no surface
/// claims an instant pointer (`instant_stays_refused_until_a_helper_reads_it`).
#[test]
fn windows_reflex_is_unsupported_without_a_live_frame_provider() {
    for (surface, live_reflex) in [
        (Surface::MacosDesktop, true),
        (Surface::IosDevice, false),
        (Surface::WindowsDesktop, false),
    ] {
        assert_eq!(
            capability(surface),
            ReflexCapability {
                schema_version: VERSION,
                live_reflex,
                instant_pointer: false,
            },
            "{surface:?}"
        );
    }
}

/// The capability table is one file both sides read: the window compiles
/// it in and sends its bytes with a start, the helper decodes those bytes
/// (`ReflexContract.decodeCapabilities`, the Swift half of this test). What
/// a surface claims is the file's row, and nothing at all under a table
/// written for another contract or run-policy version.
#[test]
fn swift_and_rust_read_one_capability_table() {
    let file = include_str!("../../../fixtures/reflex-contract/capability.json");
    assert_eq!(capability_wire(), file.trim_end().as_bytes());
    assert!(file.ends_with('\n') && !file.trim_end().contains('\n'));
    let table = decode_capability(capability_wire()).unwrap();
    assert_eq!(table, capability_table());
    assert_eq!(
        canonical_json(&serde_json::to_value(table).unwrap()),
        capability_wire()
    );
    assert_eq!(table.contract, VERSION);
    assert_eq!(table.run_policy, RUN_POLICY_VERSION);
    let row = |value: &serde_json::Value, surface: &str| SurfaceCapability {
        live_reflex: value["surfaces"][surface]["live_reflex"].as_bool().unwrap(),
        instant_pointer: value["surfaces"][surface]["instant_pointer"]
            .as_bool()
            .unwrap(),
    };
    let raw: serde_json::Value = serde_json::from_str(file).unwrap();
    for (surface, name) in [
        (Surface::MacosDesktop, "macos_desktop"),
        (Surface::IosDevice, "ios_device"),
        (Surface::WindowsDesktop, "windows_desktop"),
    ] {
        let claimed = row(&raw, name);
        assert_eq!(table.surface(surface), claimed, "{name}");
        assert_eq!(
            capability(surface),
            ReflexCapability {
                schema_version: VERSION,
                live_reflex: claimed.live_reflex,
                instant_pointer: claimed.instant_pointer,
            },
            "{name}"
        );
    }
    // A table that claims everything, written for another contract or run
    // policy, claims nothing here.
    let everything = SurfaceCapability {
        live_reflex: true,
        instant_pointer: true,
    };
    let generous = CapabilityTable {
        surfaces: CapabilitySurfaces {
            ios_device: everything,
            macos_desktop: everything,
            windows_desktop: everything,
        },
        ..table
    };
    assert!(capability_in(&generous, Surface::WindowsDesktop).live_reflex);
    for stale in [
        CapabilityTable {
            contract: VERSION + 1,
            ..generous
        },
        CapabilityTable {
            run_policy: RUN_POLICY_VERSION + 1,
            ..generous
        },
    ] {
        for surface in [
            Surface::MacosDesktop,
            Surface::IosDevice,
            Surface::WindowsDesktop,
        ] {
            let claimed = capability_in(&stale, surface);
            assert!(!claimed.live_reflex && !claimed.instant_pointer);
        }
    }
    // Only the canonical bytes with every field known are read.
    let text = std::str::from_utf8(capability_wire()).unwrap();
    for bad in [
        format!(" {text}"),
        text.replacen("{\"contract\"", "{\"a_claim\":true,\"contract\"", 1),
        text.replacen("\"instant_pointer\":false,", "", 1),
        text.replacen("\"contract\":2,", "", 1),
    ] {
        assert_eq!(
            decode_capability(bad.as_bytes()).unwrap_err(),
            ReflexError::Wire,
            "{bad}"
        );
    }
}

/// `--instant` asks the helper's verbs for an instant pointer, which is the
/// table's `instant_pointer` column and nothing else: a surface that runs
/// live reflex plans has not thereby learnt to glide its verbs' pointer
/// instantly (the helper does not read the flag), so the flag stays refused
/// until a helper reads it and its row says so.
#[test]
fn instant_stays_refused_until_a_helper_reads_it() {
    let runs_plans = ReflexCapability {
        schema_version: VERSION,
        live_reflex: true,
        instant_pointer: false,
    };
    assert!(
        instant_pointer_refusal(runs_plans)
            .is_some_and(|why| why.contains("unsupported_capability"))
    );
    assert_eq!(
        instant_pointer_refusal(ReflexCapability {
            live_reflex: false,
            instant_pointer: true,
            ..runs_plans
        }),
        None
    );
    assert!(!capability(Surface::MacosDesktop).instant_pointer);
    let words = |parts: &[&str]| {
        parts
            .iter()
            .map(|part| (*part).to_string())
            .collect::<Vec<_>>()
    };
    for command in [
        words(&["mouse-move", "--x", "1", "--y", "2", "--instant"]),
        words(&["mouse-click", "--x", "1", "--y", "2", "--instant"]),
    ] {
        assert_eq!(
            crate::computer_use::parse_command(&command).unwrap_err(),
            instant_pointer_refusal(capability(Surface::MacosDesktop)).unwrap()
        );
    }
    assert!(
        crate::computer_use::parse_command(&words(&["mouse-move", "--x", "1", "--y", "2"])).is_ok()
    );
}

/// A run's policy is decoded the same way on both sides, from the shared
/// cases: canonical bytes, an integer version this contract names, its three
/// fields exactly, and a length the table allows.
#[test]
fn shared_run_policy_cases_run_through_the_real_decoder() {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../../fixtures/reflex-contract/run_policy_cases.json"
    ))
    .unwrap();
    let mut mismatches = Vec::new();
    for case in fixture["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let wire = case["wire"].as_str().unwrap().as_bytes();
        let got = match decode_run_policy(wire) {
            Ok(policy) => {
                if policy.wire() != wire {
                    mismatches.push(format!("{name}: re-encoded wire differs"));
                }
                "ok".to_string()
            }
            Err(err) => format!("{err:?}").to_lowercase(),
        };
        if got != case["expected"].as_str().unwrap() {
            mismatches.push(format!("{name}: got {got}"));
        }
    }
    assert!(mismatches.is_empty(), "{mismatches:#?}");
    assert_eq!(fixture["cases"].as_array().unwrap().len(), 18);
}

/// The seconds a start asks for become the policy's nanoseconds at the
/// window's door: none, too many to count, or more than the table's longest
/// run are refused there, before anything reaches a helper.
#[test]
fn a_run_policy_names_its_seconds_within_the_table() {
    let longest = LIMITS.max_run_ns / 1_000_000_000;
    assert_eq!(longest, 120);
    let policy = RunPolicy::for_seconds(longest, true).unwrap();
    assert_eq!(
        policy,
        RunPolicy {
            version: RUN_POLICY_VERSION,
            run_ns: LIMITS.max_run_ns,
            renew: true
        }
    );
    assert_eq!(decode_run_policy(&policy.wire()), Ok(policy));
    for seconds in [0, longest + 1, u64::MAX / 1_000_000_000 + 1, u64::MAX] {
        assert_eq!(
            RunPolicy::for_seconds(seconds, false),
            Err(ReflexError::Budget),
            "{seconds}"
        );
    }
    assert_eq!(
        RunPolicy::for_seconds(60, false).unwrap().run_ns,
        60_000_000_000
    );
}

#[test]
fn canonical_wire_is_strict_and_preserves_the_validated_plan() {
    let row = golden("valid_basic");
    let typed: ReflexPlan = serde_json::from_value(row.plan).unwrap();
    let bytes = wire_bytes(&typed);
    assert_eq!(bytes, row.wire);
    assert_eq!(decode_wire(&bytes).unwrap().plan(), &typed);
    let duplicate = String::from_utf8(bytes.clone())
        .unwrap()
        .replace("\"version\":2", "\"version\":2,\"version\":2");
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

#[test]
fn a_colour_detector_carries_its_spec_inside_the_plan_hash() {
    let row = golden("valid_basic");
    let typed: ReflexPlan = serde_json::from_value(row.plan).unwrap();
    assert!(validate(typed.clone()).is_ok());
    // Without its spec a colour detector has nothing to read the ROI with.
    let mut bare = typed.clone();
    bare.detectors[0].color = None;
    bare.plan_hash = plan_hash(&bare);
    assert_eq!(validate(bare).unwrap_err(), ReflexError::Perception);
    // The spec is covered by the hash: the same digest over another spec is refused.
    let mut other = typed.clone();
    if let Some(color) = other.detectors[0].color.as_mut() {
        color.confirm += 1;
    }
    assert_eq!(validate(other.clone()).unwrap_err(), ReflexError::Hash);
    other.plan_hash = plan_hash(&other);
    assert!(validate(other).is_ok());
    // A version-1 document is refused, whatever it holds.
    let mut old = typed;
    old.version = 1;
    old.plan_hash = plan_hash(&old);
    assert_eq!(validate(old).unwrap_err(), ReflexError::Version);
}

/// A detector whose pick is `first` leaves the word out of the wire, so a plan
/// written before the word existed keeps its bytes and its hash (t-10223 R8):
/// `pick_omitted` and the plan the R4 bench writes (`valid_bench_plan`, hashed
/// by the v1.1.28 core) come back byte for byte under the digest they carry.
/// Each word written out is another plan under another digest, and without the
/// word each is `pick_omitted` exactly.
#[test]
fn a_plan_without_a_pick_keeps_the_wire_and_the_hash_it_had() {
    for name in ["pick_omitted", "valid_bench_plan"] {
        let row = golden(name);
        let validated = decode_wire(&row.wire).unwrap_or_else(|err| panic!("{name}: {err:?}"));
        assert_eq!(wire_bytes(validated.plan()), row.wire, "{name}");
        assert_eq!(
            plan_hash(validated.plan()),
            row.plan["plan_hash"].as_str().unwrap(),
            "{name}"
        );
        assert!(!String::from_utf8(row.wire).unwrap().contains("\"pick\""));
    }
    let omitted = golden("pick_omitted");
    for word in ["nearest", "largest", "newest", "oldest"] {
        let row = golden(&format!("pick_{word}"));
        let validated = decode_wire(&row.wire).unwrap_or_else(|err| panic!("{word}: {err:?}"));
        assert_eq!(wire_bytes(validated.plan()), row.wire, "{word}");
        assert_ne!(
            validated.plan().plan_hash,
            omitted.plan["plan_hash"].as_str().unwrap(),
            "{word}"
        );
        let mut without = row.plan.clone();
        without["detectors"][0]
            .as_object_mut()
            .unwrap()
            .remove("pick");
        let mut typed: ReflexPlan = serde_json::from_value(without).unwrap();
        typed.plan_hash = plan_hash(&typed);
        assert_eq!(wire_bytes(&typed), omitted.wire, "{word}");
    }
}

/// A Flow document names a detector's pick inside its `detector:` line — the
/// detector's own JSON, never a line of its own — and writes it back the same
/// way; a word the contract does not know is refused where the line is read.
#[test]
fn a_flow_detector_line_carries_its_pick() {
    let omitted: ReflexPlan = serde_json::from_value(golden("pick_omitted").plan).unwrap();
    let nearest = golden("pick_nearest");
    let text = omitted
        .written_sections()
        .replace("\"patches\":1,", "\"patches\":1,\"pick\":\"nearest\",")
        .replace(
            &omitted.plan_hash,
            nearest.plan["plan_hash"].as_str().unwrap(),
        );
    let read = read_sections(&text)
        .unwrap_or_else(|err| panic!("{err}"))
        .unwrap();
    assert_eq!(wire_bytes(read.plan()), nearest.wire);
    let written = read.plan().written_sections();
    assert!(written.contains("\"pick\":\"nearest\""));
    assert_eq!(
        read_sections(&written).unwrap().unwrap().plan(),
        read.plan()
    );
    let unknown = text.replace("\"pick\":\"nearest\"", "\"pick\":\"nearby\"");
    assert!(
        read_sections(&unknown)
            .unwrap_err()
            .starts_with("detector:")
    );
}

/// `Pick::ALL` is the one table of a pick's words: each is its serde name,
/// `first` is the default and the one the wire leaves out, and every scene of
/// `pick_cases.json` names each word once for each of its frames — the scenes
/// Swift reads through its kernel.
#[test]
fn a_picks_words_are_one_table() {
    for pick in Pick::ALL {
        assert_eq!(serde_json::to_value(pick).unwrap(), pick.word());
        assert_eq!(
            serde_json::from_value::<Pick>(pick.word().into()).unwrap(),
            pick
        );
        assert_eq!(pick.is_first(), pick == Pick::default());
    }
    assert_eq!(Pick::default(), Pick::First);
    let words: BTreeSet<&str> = Pick::ALL.iter().map(|pick| pick.word()).collect();
    assert_eq!(words.len(), Pick::ALL.len());
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../../fixtures/reflex-contract/pick_cases.json"
    ))
    .unwrap();
    let cases = fixture["cases"].as_array().unwrap();
    for case in cases {
        let name = case["name"].as_str().unwrap();
        let frames = case["frames"].as_array().unwrap().len();
        let expected = case["expected"].as_object().unwrap();
        assert_eq!(
            expected.keys().map(String::as_str).collect::<BTreeSet<_>>(),
            words,
            "{name}"
        );
        for (word, targets) in expected {
            assert_eq!(targets.as_array().unwrap().len(), frames, "{name} {word}");
        }
    }
    assert_eq!(cases.len(), 4);
}

#[test]
fn the_reflex_table_the_window_sends_is_the_one_table() {
    let file = include_str!("../../../fixtures/reflex-contract/limits.json");
    assert_eq!(limits_wire(), file.trim_end().as_bytes());
    let longest_glide = LIMITS.max_pointer_duration_ms * 1_000_000 / LIMITS.pointer_tick_ns;
    assert!(longest_glide + 2 <= LIMITS.max_expanded_actions);
}

#[test]
fn shared_observation_cases_run_through_admissible_and_aim() {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../../fixtures/reflex-contract/observation_cases.json"
    ))
    .unwrap();
    // A case replaces top-level fields; a null removes an optional one.
    let patched = |base: &serde_json::Value, patch: &serde_json::Value| {
        let mut value = base.clone();
        let object = value.as_object_mut().unwrap();
        for (key, item) in patch.as_object().unwrap() {
            if item.is_null() && key != "captured_host_ns" {
                object.remove(key);
            } else {
                object.insert(key.clone(), item.clone());
            }
        }
        value
    };
    let samples = fixture["samples"].as_u64().unwrap();
    let mut mismatches = Vec::new();
    for case in fixture["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let detector: Detector =
            serde_json::from_value(patched(&fixture["detector"], &case["detector"])).unwrap();
        let frame: FrameFacts =
            serde_json::from_value(patched(&fixture["frame"], &case["frame"])).unwrap();
        let observation: Observation =
            serde_json::from_value(patched(&fixture["observation"], &case["observation"])).unwrap();
        // Each observation's own canonical wire decodes back to itself.
        let wire = canonical_json(&serde_json::to_value(&observation).unwrap());
        assert_eq!(
            serde_json::from_slice::<Observation>(&wire).unwrap(),
            observation,
            "{name}"
        );
        let got = observation.admissible(&detector, &frame, samples);
        if got != case["expected"].as_bool().unwrap() {
            mismatches.push(format!("admissible {name}: got {got}"));
        }
    }
    for case in fixture["aim_cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let target: Target =
            serde_json::from_value(patched(&fixture["target"], &case["target"])).unwrap();
        let got = target.aim(
            case["at_host_ns"].as_u64().unwrap(),
            case["captured_host_ns"].as_u64().unwrap(),
            case["max_age_ns"].as_u64().unwrap(),
        );
        let expected = case["expected"]
            .as_array()
            .map(|point| (point[0].as_i64().unwrap(), point[1].as_i64().unwrap()));
        if got != expected {
            mismatches.push(format!("aim {name}: got {got:?}"));
        }
    }
    assert!(mismatches.is_empty(), "{mismatches:#?}");
    assert_eq!(fixture["cases"].as_array().unwrap().len(), 34);
    assert_eq!(fixture["aim_cases"].as_array().unwrap().len(), 10);
}
