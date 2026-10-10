use std::collections::BTreeMap;

use serde_json::{Value, json};
use zerocode_core::computer_use::MARK_PIN_TOLERANCE_POINTS;
use zerocode_core::computer_use_protocol::{
    marks::{ElementFace, is_mark_candidate},
    render::Rect,
    words::{self, ReadLine},
};

const MIN_CONFIDENCE: f64 = 0.8;

pub(super) fn add_faces(
    faces: &mut Vec<ElementFace>,
    reading: &Value,
    window: Rect,
) -> BTreeMap<usize, ReadLine> {
    let lines = reading
        .get("lines")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|line| {
            line.get("confidence")
                .and_then(Value::as_f64)
                .is_some_and(|confidence| (MIN_CONFIDENCE..=1.0).contains(&confidence))
        })
        .filter_map(ReadLine::from_value)
        .collect::<Vec<_>>();
    let lines = words::reading_order(lines);
    let mut pixels = BTreeMap::new();
    let mut index = faces
        .iter()
        .map(|face| face.index)
        .max()
        .unwrap_or_default();
    for line in lines {
        let rect = Rect::new(line.x, line.y, line.width, line.height);
        if line.text.trim().is_empty()
            || ![line.x, line.y, line.width, line.height]
                .into_iter()
                .all(f64::is_finite)
            || line.width <= 0.0
            || line.height <= 0.0
            || !window.contains_point(line.x, line.y)
            || !window.contains_point(rect.max_x(), rect.max_y())
            || faces.iter().any(|face| {
                is_mark_candidate(face, window)
                    && face
                        .local()
                        .contains_point(line.center().0 - window.x, line.center().1 - window.y)
            })
        {
            continue;
        }
        let Some(next) = index.checked_add(1) else {
            break;
        };
        index = next;
        faces.push(ElementFace {
            index,
            role: "OCRText".into(),
            name: Some(line.text.clone()),
            placeholder: None,
            plain_input: None,
            traits: Vec::new(),
            actions: Vec::new(),
            x: line.x - window.x,
            y: line.y - window.y,
            width: line.width,
            height: line.height,
            signature: "ocr".into(),
            visible: None,
            context: None,
        });
        pixels.insert(index, line);
    }
    pixels
}

pub(super) fn pin(line: &ReadLine, window: Rect) -> Value {
    json!({
        "text": line.text,
        "frame": { "x": line.x, "y": line.y, "width": line.width, "height": line.height },
        "window": { "x": window.x, "y": window.y, "width": window.width, "height": window.height },
        "tolerance": MARK_PIN_TOLERANCE_POINTS,
        "minimumConfidence": MIN_CONFIDENCE,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pixels_offer_only_confident_in_window_regions_and_never_fields() {
        let mut faces = Vec::new();
        let reading = json!({"lines": [
            {"text": "Next", "confidence": 0.95, "x": 120, "y": 100, "width": 60, "height": 20},
            {"text": "Uncertain", "confidence": 0.3, "x": 120, "y": 130, "width": 60, "height": 20},
            {"text": "Outside", "confidence": 0.95, "x": 10, "y": 10, "width": 60, "height": 20},
            {"text": "Invalid", "confidence": 0.95, "x": 120, "y": 150, "width": -60, "height": 20},
            {"text": "Unknown confidence", "x": 120, "y": 170, "width": 60, "height": 20}
        ]});
        let pixels = add_faces(&mut faces, &reading, Rect::new(100.0, 50.0, 300.0, 300.0));
        assert_eq!(pixels.len(), 1);
        assert_eq!(faces[0].role, "OCRText");
        assert_eq!(faces[0].x, 20.0);
        assert!(faces[0].plain_input.is_none());
    }
}
