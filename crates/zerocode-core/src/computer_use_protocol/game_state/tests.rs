use serde_json::{Value, json};

use super::*;

fn fixture() -> Value {
    serde_json::from_str(include_str!("../../../fixtures/game-state/spec_cases.json")).unwrap()
}

/// The shared base spec with a case's top-level fields replaced; a null
/// removes the field.
fn patched(base: &Value, patch: &Value) -> Value {
    let mut value = base.clone();
    let fields = value.as_object_mut().unwrap();
    for (key, item) in patch.as_object().unwrap() {
        if item.is_null() {
            fields.remove(key);
        } else {
            fields.insert(key.clone(), item.clone());
        }
    }
    value
}

#[test]
fn shared_spec_cases_run_through_the_real_validator() {
    let fixture = fixture();
    let limits: PerceptionLimits =
        serde_json::from_str(include_str!("../../../fixtures/game-state/limits.json")).unwrap();
    let cases = fixture["cases"].as_array().unwrap();
    // Every case is judged before the assertion, so a failure names them all.
    let mut mismatches = Vec::new();
    for case in cases {
        let name = case["name"].as_str().unwrap();
        let roi: Roi =
            serde_json::from_value(case.get("roi").unwrap_or(&fixture["roi"]).clone()).unwrap();
        let (got, cost) =
            match serde_json::from_value::<ColorSpec>(patched(&fixture["spec"], &case["patch"])) {
                Err(_) => ("wire", None),
                Ok(spec) => match validate_color(&spec, &roi, &limits) {
                    Ok(cost) => ("ok", Some(cost)),
                    Err(err) => (err.code(), None),
                },
            };
        if got != case["expected"].as_str().unwrap() || cost != case["cost"].as_u64() {
            mismatches.push(format!("{name}: got {got} {cost:?}"));
        }
    }
    assert!(mismatches.is_empty(), "{mismatches:#?}");
    assert_eq!(cases.len(), 61);
}

#[test]
fn frame_roi_follows_only_a_uniform_rescale() {
    let fixture = fixture();
    let base: ColorSpec = serde_json::from_value(fixture["spec"].clone()).unwrap();
    let cases = fixture["frame_roi"].as_array().unwrap();
    for case in cases {
        let name = case["name"].as_str().unwrap();
        let roi: Roi = serde_json::from_value(case["roi"].clone()).unwrap();
        let reference: PixelExtent = serde_json::from_value(case["reference"].clone()).unwrap();
        let frame: PixelExtent = serde_json::from_value(case["frame"].clone()).unwrap();
        let spec = ColorSpec {
            reference_width: reference.width,
            reference_height: reference.height,
            ..base.clone()
        };
        let got = frame_roi(&roi, &spec, &frame).map_or(
            Value::Null,
            |(roi, scale)| json!({"roi": roi, "scale": scale}),
        );
        assert_eq!(got, case["expected"], "{name}");
    }
    assert_eq!(cases.len(), 9);
}

#[test]
fn perception_limits_come_from_one_table() {
    let wire = include_bytes!("../../../fixtures/game-state/limits.json");
    assert_eq!(
        serde_json::from_slice::<PerceptionLimits>(wire).unwrap(),
        LIMITS
    );
    assert_eq!(limits_wire(&LIMITS), wire.strip_suffix(b"\n").unwrap());
}
