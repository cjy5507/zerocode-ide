use super::*;
use serde_json::json;
use zerocode_core::computer_use_protocol::marks::ELEMENTS_KEY;

fn frame() -> Value {
    json!({
        "snapshot": {
            "id": "fixture",
            "app": {"name": "Fixture", "pid": 4321, "bundleId": "test.fixture"},
            "window": {"id": 77, "x": 100, "y": 50, "width": 400, "height": 300},
            "treeText": "[1] AXButton Continue",
            "elementCount": 1,
            ELEMENTS_KEY: [{
                "index": 1, "role": "AXButton", "name": "Continue",
                "actions": ["AXPress"], "traits": [],
                "x": 20, "y": 30, "width": 120, "height": 40,
                "signature": "button/continue", "context": "form"
            }]
        }
    })
}

fn params(viewer: &str) -> Map<String, Value> {
    json!({"app": "Fixture", "marks": true, "noScreenshot": true, "viewer": viewer})
        .as_object()
        .unwrap()
        .clone()
}

fn with_picture(mut answer: Value) -> Value {
    use base64::Engine as _;
    let image = RgbaImage::new(400, 300, vec![255; 400 * 300 * 4]).unwrap();
    answer["screenshot"] = json!({
        "data": base64::engine::general_purpose::STANDARD.encode(image.encode().unwrap()),
        "width": 400, "height": 300, "scale": 1.0
    });
    answer
}

#[test]
fn structured_marks_need_one_tree_and_no_pixels() {
    let mut calls = Vec::new();
    let answer = observe_with(
        &params("structured-one"),
        &super::super::eye::Memory::new(),
        &mut |method, request| {
            calls.push(method.to_owned());
            assert_eq!(request["noScreenshot"], true);
            assert_eq!(request[ELEMENT_FRAMES_KEY], true);
            Ok(frame())
        },
        &mut |_| panic!("structured observation does not sleep"),
    )
    .unwrap();
    assert_eq!(calls, ["getAppState"]);
    assert!(answer["screenshot"].is_null());
    assert_eq!(answer["tree"]["text"], "[1] AXButton Continue");
    assert_eq!(answer["marks"]["perception"], "accessibility");
    let pin = super::super::marks::pinned_click(&json!({
        "mark": 1, "look": answer["marks"]["lookId"], "background": true
    }))
    .unwrap();
    assert_eq!(pin.params["background"], true);
    assert_eq!(pin.params["windowId"], 77);
    assert_eq!(pin.params["elementIndex"], 1);
    assert_eq!(pin.params["elementSignature"], "button/continue");
}

#[test]
fn structured_marks_revoke_the_pixel_diff_baseline() {
    let request = params("structured-baseline");
    let resolved = resolved_key(&request, &frame()).unwrap();
    keep(
        resolved.clone(),
        LastFrame {
            png: vec![1, 2, 3],
            frame: ShotFrame::UNIT,
            acts: 0,
            before_act: None,
        },
    );
    let answer = structured_look(&request, Map::new(), &mut |_, _| Ok(frame()))
        .unwrap()
        .unwrap();
    assert!(answer["changed"].is_null());
    assert!(take_last(&resolved).is_none());
}

#[test]
fn an_empty_tree_falls_back_to_a_real_captured_frame() {
    let mut empty = frame();
    empty["snapshot"][ELEMENTS_KEY] = json!([]);
    let captured = with_picture(empty.clone());
    let mut calls = Vec::new();
    let answer = observe_with(
        &params("structured-empty"),
        &super::super::eye::Memory::new(),
        &mut |method, request| {
            calls.push(method.to_owned());
            match method {
                "getAppState" if request["noScreenshot"] == true => Ok(empty.clone()),
                "getAppState" => Ok(captured.clone()),
                "readText" => {
                    assert_eq!(
                        request["capturedFrame"]["screenshot"],
                        captured["screenshot"]
                    );
                    Ok(json!({"capturedFrame": true, "pixelTextPins": true, "lines": []}))
                }
                _ => panic!("unexpected {method}"),
            }
        },
        &mut |_| {},
    )
    .unwrap();
    assert_eq!(calls, ["getAppState", "getAppState", "readText"]);
    assert!(answer["screenshot"].is_null());
    assert_eq!(answer["marks"]["items"], json!([]));
}

#[test]
fn a_plain_tree_only_look_needs_no_picture_or_marks() {
    let mut request = params("structured-plain");
    request.remove("marks");
    let answer = observe_with(
        &request,
        &super::super::eye::Memory::new(),
        &mut |method, request| {
            assert_eq!(method, "getAppState");
            assert_eq!(request["noScreenshot"], true);
            assert!(request.get(ELEMENT_FRAMES_KEY).is_none());
            Ok(frame())
        },
        &mut |_| {},
    )
    .unwrap();
    assert!(answer["marks"].is_null());
    assert!(answer["screenshot"].is_null());
    assert_eq!(answer["tree"]["text"], "[1] AXButton Continue");
}

#[test]
fn a_requested_pixel_diff_or_ocr_never_takes_the_structured_shortcut() {
    for option in ["diff", "ocr"] {
        let mut request = params(&format!("structured-{option}"));
        request.insert(option.into(), true.into());
        let captured = with_picture(frame());
        let mut reads = 0;
        observe_with(
            &request,
            &super::super::eye::Memory::new(),
            &mut |method, request| {
                if method == "getAppState" {
                    reads += 1;
                    assert_ne!(request["noScreenshot"], true);
                    Ok(captured.clone())
                } else {
                    assert_eq!(method, "readText");
                    Ok(json!({"capturedFrame": true, "pixelTextPins": true, "lines": []}))
                }
            },
            &mut |_| {},
        )
        .unwrap();
        assert_eq!(reads, 1);
    }
}

#[test]
fn inaccessible_or_invalid_geometry_cannot_create_a_structured_pin() {
    for bounds in [(0.0, 300.0), (-2.0, 300.0), (1e20, 300.0)] {
        let mut answer = frame();
        answer["snapshot"]["window"]["width"] = bounds.0.into();
        answer["snapshot"]["window"]["height"] = bounds.1.into();
        assert!(
            super::super::marks::structured_marks(&answer, "invalid")
                .unwrap()
                .is_none()
        );
    }
    let mut answer = frame();
    answer["snapshot"]
        .as_object_mut()
        .unwrap()
        .remove(ELEMENTS_KEY);
    assert!(super::super::marks::structured_marks(&answer, "missing").is_err());
}

#[test]
fn unnamed_controls_alone_do_not_claim_to_describe_app_content() {
    let mut answer = frame();
    answer["snapshot"][ELEMENTS_KEY][0]["name"] = Value::Null;
    assert!(
        super::super::marks::structured_marks(&answer, "unlabelled")
            .unwrap()
            .is_none()
    );
    answer["snapshot"][ELEMENTS_KEY][0]["role"] = "AXTextField".into();
    answer["snapshot"][ELEMENTS_KEY][0]["plainInput"] = "".into();
    assert!(
        super::super::marks::structured_marks(&answer, "field")
            .unwrap()
            .is_some()
    );
}

#[test]
fn structured_fields_follow_the_latest_value_not_cached_pixels() {
    let mut answer = frame();
    answer["snapshot"][ELEMENTS_KEY][0]["role"] = "AXTextField".into();
    answer["snapshot"][ELEMENTS_KEY][0]["plainInput"] = "".into();
    let first = super::super::marks::structured_marks(&answer, "field")
        .unwrap()
        .unwrap();
    answer["snapshot"][ELEMENTS_KEY][0]["plainInput"] = "changed".into();
    let next = super::super::marks::structured_marks(&answer, "field")
        .unwrap()
        .unwrap();
    assert_ne!(first.answer["lookId"], next.answer["lookId"]);
    assert_eq!(next.answer["fields"][0]["value"], "changed");
    let pin = super::super::marks::pinned_click(&json!({
        "mark": 1, "look": next.answer["lookId"], "value": "replacement"
    }))
    .unwrap();
    assert_eq!(pin.params["expectedPlainValue"], "changed");
}
