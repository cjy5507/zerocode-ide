use super::*;

/// The window measured on 09-30: the window server's, at the cursor's level,
/// 23×22 — and the pointer at its largest.
#[test]
fn the_window_servers_pointer_sized_window_at_the_cursors_level_is_the_pointer() {
    assert!(is_pointer_picture(
        CURSOR_WINDOW_LAYER,
        WINDOW_SERVER_OWNER,
        23.0,
        22.0
    ));
    assert!(is_pointer_picture(
        CURSOR_WINDOW_LAYER,
        WINDOW_SERVER_OWNER,
        POINTER_PICTURE_MAX_SIDE_PT,
        POINTER_PICTURE_MAX_SIDE_PT
    ));
}

/// One fact short is not the pointer: another level, another owner, or a
/// side bigger than any pointer.
#[test]
fn a_window_short_of_any_of_the_three_is_not_the_pointer() {
    assert!(!is_pointer_picture(25, WINDOW_SERVER_OWNER, 23.0, 22.0));
    assert!(!is_pointer_picture(
        CURSOR_WINDOW_LAYER,
        "Other",
        23.0,
        22.0
    ));
    assert!(!is_pointer_picture(
        CURSOR_WINDOW_LAYER,
        WINDOW_SERVER_OWNER,
        300.0,
        22.0
    ));
    assert!(!is_pointer_picture(
        CURSOR_WINDOW_LAYER,
        WINDOW_SERVER_OWNER,
        23.0,
        200.0
    ));
}
