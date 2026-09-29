"""The pointer's own picture on a window list (t-12979): the core's
`is_pointer_picture` (crates/zerocode-core/src/computer_use_protocol/
pointer.rs) for a row as the helper's `list-all-windows --all-layers`
answers it — its layer, its owner's name and its size, all three. The three
values are the core's, written again here; the one shared table
(crates/zerocode-core/fixtures/pointer-picture/examples.json) and the core's
source hold both to one answer (test_pointer_picture.py)."""

CURSOR_WINDOW_LAYER = 2_147_483_630
WINDOW_SERVER_OWNER = "Window Server"
POINTER_PICTURE_MAX_SIDE_PT = 160


def is_pointer_picture(row):
    """Whether a listed window row is the pointer's own picture."""
    return (row.get("layer") == CURSOR_WINDOW_LAYER and (row.get("app") or {}).get("name") == WINDOW_SERVER_OWNER
            and row["width"] <= POINTER_PICTURE_MAX_SIDE_PT and row["height"] <= POINTER_PICTURE_MAX_SIDE_PT)


def front_at(windows, x, y):
    """The window a press at (x, y) lands on: the frontmost that is seen,
    neither an overlay nor the pointer's own picture, and holds the point;
    None when none does."""
    return next((w for w in windows if not w.get("overlay") and not is_pointer_picture(w) and w.get("alpha", 1) > 0
                 and w["x"] <= x < w["x"] + w["width"] and w["y"] <= y < w["y"] + w["height"]), None)
