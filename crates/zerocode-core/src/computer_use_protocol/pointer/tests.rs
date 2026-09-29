use super::*;

/// The windows read off this Mac's window list on 09-30, written as read —
/// never through the constants, so a constant that is wrong fails here: the
/// owner "Window Server", level 2147483630, 23×22 (a 1080×1920 display
/// beside a 1920×1080 one) and 28×40 (right after the built-in display
/// closed); and 28×40 at the largest pointer size, four times: 112×160.
#[test]
fn the_pointer_as_read_off_the_window_list_is_the_pointer() {
    for (width, height) in [(23.0, 22.0), (28.0, 40.0), (112.0, 160.0)] {
        assert!(
            is_pointer_picture(2_147_483_630, "Window Server", width, height),
            "{width}×{height}"
        );
    }
}

/// One fact short is not the pointer: another level, another owner — the
/// window server's process name "WindowServer" is not the name the window
/// list gives its windows — or a side bigger than any pointer.
#[test]
fn a_window_short_of_any_of_the_three_is_not_the_pointer() {
    for (layer, owner, width, height) in [
        (25, "Window Server", 23.0, 22.0),
        (2_147_483_630, "Other", 23.0, 22.0),
        (2_147_483_630, "WindowServer", 23.0, 22.0),
        (2_147_483_630, "Window Server", 300.0, 22.0),
        (2_147_483_630, "Window Server", 23.0, 200.0),
    ] {
        assert!(
            !is_pointer_picture(layer, owner, width, height),
            "{layer} {owner} {width}×{height}"
        );
    }
}

/// The one table of rows read on this Mac and of guards
/// (`fixtures/pointer-picture/examples.json`, w-12356 ef5b7702): the judgment
/// answers each as the table does — the reflex bench answers the same table
/// by the window server's process.
#[test]
fn every_example_of_the_shared_table_is_answered_as_the_table_says() {
    let table: serde_json::Value = serde_json::from_str(include_str!(
        "../../../fixtures/pointer-picture/examples.json"
    ))
    .expect("the table reads");
    let examples = table["examples"].as_array().expect("examples");
    assert!(examples.len() >= 7, "{}", examples.len());
    for example in examples {
        let answered = is_pointer_picture(
            example["layer"].as_i64().expect("layer"),
            example["owner"].as_str().expect("owner"),
            example["width"].as_f64().expect("width"),
            example["height"].as_f64().expect("height"),
        );
        assert_eq!(
            Some(answered),
            example["pointer"].as_bool(),
            "{}",
            example["what"]
        );
    }
}
