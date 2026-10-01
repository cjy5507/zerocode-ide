use super::*;

fn window(id: u64, pid: i64, layer: i64, rect: (f64, f64, f64, f64)) -> DesktopWindow {
    DesktopWindow {
        id,
        pid,
        app: format!("app-{pid}"),
        rect: Rect::new(rect.0, rect.1, rect.2, rect.3),
        own: false,
        layer,
        alpha: 1.0,
        overlay: false,
    }
}

const SCREEN: Rect = Rect::new(0.0, 0.0, 1_000.0, 800.0);

/// Another app's window over the target's right half: it names whose it is,
/// its layer and its bounds from the target's corner — and nothing it shows —
/// and says how much of the place it hides.
#[test]
fn a_window_in_front_is_told_by_whose_it_is_its_layer_and_where() {
    let windows = [
        window(7, 20, 3, (250.0, 100.0, 300.0, 200.0)),
        window(5, 10, 0, (100.0, 100.0, 300.0, 200.0)),
    ];
    let cover = cover_of(&windows, 5, None).expect("the target is listed");
    assert_eq!(cover.coverers.len(), 1);
    let over = &cover.coverers[0];
    assert_eq!((over.id, over.owner, over.layer), (7, Owner::OtherApp, 3));
    assert_eq!(over.app, "app-20");
    assert_eq!(over.bounds, Rect::new(150.0, 0.0, 300.0, 200.0));
    assert_eq!(cover.hidden_permille, 500);
    assert!(cover.centre_hidden, "the centre (250, 200) is under it");
    assert!(cover.blocks_a_press());
}

/// A place given from the window's corner follows the window: the same
/// scene read after the window moved reads the same.
#[test]
fn a_place_in_the_window_follows_the_window() {
    let local = Some(Rect::new(10.0, 10.0, 20.0, 20.0));
    let at = |x: f64| {
        let windows = [
            window(9, 30, 0, (x, 100.0, 50.0, 50.0)),
            window(5, 10, 0, (x, 100.0, 300.0, 200.0)),
        ];
        cover_of(&windows, 5, local).expect("listed")
    };
    let (here, there) = (at(100.0), at(400.0));
    assert_eq!(here.spot, Rect::new(110.0, 110.0, 20.0, 20.0));
    assert_eq!(there.spot, Rect::new(410.0, 110.0, 20.0, 20.0));
    assert_eq!(here.coverers[0].bounds, there.coverers[0].bounds);
    assert_eq!(here.hidden_permille, 1_000);
}

/// Behind the target, beside the place, an overlay and a window seen through:
/// none of them covers it. The target's own panel and ZeroCode's window do,
/// each named for whose it is.
#[test]
fn only_what_stands_in_front_over_the_place_covers_it() {
    let mut overlay = window(1, 40, 20, (0.0, 0.0, 1_000.0, 800.0));
    overlay.overlay = true;
    let mut clear = window(2, 41, 3, (100.0, 100.0, 50.0, 50.0));
    clear.alpha = 0.0;
    let mut own = window(3, 42, 0, (380.0, 280.0, 40.0, 40.0));
    own.own = true;
    let windows = [
        overlay,
        clear,
        own,
        window(4, 10, 3, (100.0, 100.0, 40.0, 40.0)),
        window(6, 43, 0, (900.0, 700.0, 50.0, 50.0)),
        window(5, 10, 0, (100.0, 100.0, 300.0, 200.0)),
        window(8, 44, 0, (100.0, 100.0, 300.0, 200.0)),
    ];
    let cover = cover_of(&windows, 5, None).expect("listed");
    let seen: Vec<(u64, Owner)> = cover
        .coverers
        .iter()
        .map(|over| (over.id, over.owner))
        .collect();
    assert_eq!(seen, [(3, Owner::ZeroCode), (4, Owner::SameApp)]);
    assert_eq!(
        cover.in_front.len(),
        3,
        "window 6 is kept clear of by a move"
    );
    assert!(!cover.centre_hidden);
    assert!(cover.hides_any());
}

/// A sliver hidden is not nothing hidden: the share rounds up.
#[test]
fn a_sliver_over_the_place_counts() {
    let windows = [
        window(9, 30, 20, (0.0, 297.0, 1_000.0, 3.0)),
        window(5, 10, 0, (100.0, 100.0, 300.0, 200.0)),
    ];
    let cover = cover_of(&windows, 5, None).expect("listed");
    assert_eq!(cover.hidden_permille, 15);
    assert!(!cover.blocks_a_press());
    assert_eq!(cover_of(&windows, 99, None), None, "a window not listed");
}

/// The shortest move that clears the place of every window in front and
/// keeps the whole window on one screen: never onto another window in front,
/// never off the screen.
#[test]
fn a_clear_place_leaves_every_window_in_front_and_stays_on_a_screen() {
    let windows = [
        window(7, 20, 3, (300.0, 100.0, 200.0, 400.0)),
        window(8, 21, 3, (0.0, 0.0, 1_000.0, 90.0)),
        window(5, 10, 0, (200.0, 150.0, 300.0, 200.0)),
    ];
    let cover = cover_of(&windows, 5, None).expect("listed");
    let [x, y] = clear_place(&cover, &[SCREEN]).expect("a place exists");
    let moved = Rect::new(x, y, cover.window.width, cover.window.height);
    assert!(SCREEN.contains_rect(&moved), "{moved:?}");
    assert!(cover.in_front.iter().all(|over| !over.intersects(&moved)));
    // Left of the panel is 200 away, right of it 300, below it 350.
    assert_eq!([x, y], [0.0, 150.0]);
}

/// A place no move can clear on any screen has none.
#[test]
fn no_clear_place_when_the_screen_is_covered() {
    let windows = [
        window(7, 20, 3, (0.0, 0.0, 1_000.0, 800.0)),
        window(5, 10, 0, (200.0, 150.0, 300.0, 200.0)),
    ];
    let cover = cover_of(&windows, 5, None).expect("listed");
    assert_eq!(clear_place(&cover, &[SCREEN]), None);
}

/// The screens of a desk that is not one screen at the origin: the main one,
/// one left of it reaching above its top edge, one above it, one to its
/// right — all in the window list's global top-left points, whatever each
/// screen's scale (a 3008×1692-point screen is a Retina one at scale 2).
const MAIN: Rect = Rect::new(0.0, 0.0, 1_920.0, 1_080.0);
const LEFT: Rect = Rect::new(-3_008.0, -300.0, 3_008.0, 1_692.0);
const ABOVE: Rect = Rect::new(0.0, -1_692.0, 3_008.0, 1_692.0);
const RIGHT: Rect = Rect::new(1_920.0, 0.0, 1_512.0, 982.0);

/// A window on a screen left of the main one, whose origin is negative on
/// both axes: the shortest clear move keeps it whole on that screen.
#[test]
fn a_clear_place_on_a_screen_left_of_the_main_one_stays_on_it() {
    let windows = [
        window(7, 20, 0, (-1_760.0, 320.0, 120.0, 160.0)),
        window(5, 10, 0, (-2_000.0, 200.0, 600.0, 400.0)),
    ];
    let cover = cover_of(&windows, 5, None).expect("listed");
    assert!(cover.hides_any());
    let [x, y] = clear_place(&cover, &[MAIN, LEFT]).expect("a place exists");
    assert_eq!([x, y], [-2_000.0, -80.0], "up past the cover, 280 points");
    assert!(LEFT.contains_rect(&Rect::new(x, y, 600.0, 400.0)));
}

/// A window standing across two screens is moved whole onto one — never
/// left across the seam.
#[test]
fn a_window_across_two_screens_is_moved_whole_onto_one() {
    let windows = [
        window(7, 20, 0, (1_800.0, 150.0, 200.0, 200.0)),
        window(5, 10, 0, (1_700.0, 100.0, 500.0, 300.0)),
    ];
    let cover = cover_of(&windows, 5, None).expect("listed");
    let [x, y] = clear_place(&cover, &[MAIN, RIGHT]).expect("a place exists");
    let moved = Rect::new(x, y, 500.0, 300.0);
    assert_eq!([x, y], [2_000.0, 100.0]);
    assert!(RIGHT.contains_rect(&moved) && !MAIN.intersects(&moved));
    assert!(cover.in_front.iter().all(|over| !over.intersects(&moved)));
}

/// A place in a window on a screen above the main one, at negative y: the
/// place follows the window, and the shortest move clears the place alone —
/// the window may stay under the cover elsewhere.
#[test]
fn a_place_on_a_screen_above_the_main_one_clears_with_the_shortest_move() {
    let windows = [
        window(7, 20, 0, (600.0, -700.0, 300.0, 300.0)),
        window(5, 10, 0, (400.0, -900.0, 700.0, 500.0)),
    ];
    let cover = cover_of(&windows, 5, Some(Rect::new(250.0, 250.0, 40.0, 20.0))).expect("listed");
    assert_eq!(cover.spot, Rect::new(650.0, -650.0, 40.0, 20.0));
    assert!(cover.blocks_a_press());
    let [x, y] = clear_place(&cover, &[MAIN, ABOVE]).expect("a place exists");
    assert_eq!([x, y], [400.0, -970.0], "up 70 points");
}

/// The pointer's own picture, as the window server lists it when it draws
/// the cursor as a window (a rotated display, 09-30): a 23×22 window of the
/// window server's at the cursor's level (kCGCursorWindowLevel), in front of
/// every other, wherever the pointer rests.
fn pointer_picture(x: f64, y: f64) -> DesktopWindow {
    let mut pointer = window(99, 399, 2_147_483_630, (x, y, 23.0, 22.0));
    pointer.app = "Window Server".to_string();
    pointer
}

/// The pointer resting on the target's window hides nothing a hand presses:
/// the cursor is not a cover. With it over the place's centre and another
/// app's ordinary window over the right edge, only that window covers the
/// place, the centre shows, and today's rule brings the target forward
/// rather than leaving "another app's window above ordinary ones" to the
/// person (t-12979, measured 09-30 00:4x on a 1920x1080 and a 1080x1920
/// display: the cursor stood first on the list at layer 2147483630).
#[test]
fn the_pointer_resting_on_the_target_is_not_a_cover() {
    let windows = [
        pointer_picture(240.0, 190.0),
        window(7, 20, 0, (330.0, 100.0, 70.0, 200.0)),
        window(5, 10, 0, (100.0, 100.0, 300.0, 200.0)),
    ];
    let cover = cover_of(&windows, 5, None).expect("listed");
    let over: Vec<u64> = cover.coverers.iter().map(|over| over.id).collect();
    assert_eq!(over, [7], "the cursor is not one of what covers the place");
    assert!(
        !cover.centre_hidden,
        "the cursor over the centre hides nothing to press"
    );
    assert_eq!(
        cover.hidden_permille, 233,
        "only the other app's window hides the place"
    );
    assert_eq!(
        crate::jev::cover::todays_rule(&cover),
        Ok(vec![
            crate::jev::cover::Move::RaiseTarget,
            crate::jev::cover::Move::MoveTarget
        ]),
        "the target is brought forward, not the person asked"
    );

    let alone = [
        pointer_picture(240.0, 190.0),
        window(5, 10, 0, (100.0, 100.0, 300.0, 200.0)),
    ];
    assert_eq!(
        in_front(&alone, 1).count(),
        0,
        "nothing stands in front of the target but the pointer"
    );
}

/// Only the three together are the pointer: a window of the window server's
/// at the cursor's level bigger than any pointer, a pointer-sized window of
/// another app at that level, and a pointer-sized window of the window
/// server's below it each still cover the place.
#[test]
fn a_window_that_is_not_all_three_of_the_pointer_still_covers() {
    let mut big = pointer_picture(100.0, 100.0);
    big.id = 97;
    big.rect = Rect::new(100.0, 100.0, 300.0, 200.0);
    let mut lower = pointer_picture(240.0, 190.0);
    lower.id = 96;
    lower.layer = 25;
    let windows = [
        big,
        window(98, 20, 2_147_483_630, (240.0, 190.0, 23.0, 22.0)),
        lower,
        window(5, 10, 0, (100.0, 100.0, 300.0, 200.0)),
    ];
    let cover = cover_of(&windows, 5, None).expect("listed");
    let over: Vec<u64> = cover.coverers.iter().map(|over| over.id).collect();
    assert_eq!(over, [97, 98, 96]);
    assert!(cover.centre_hidden);
}

/// 09-30 (m-16152): where WindowServer draws the pointer itself, the window
/// list holds it as a window at the cursor level, 28 × 40 points right under
/// the pointer. A person's pointer resting on the place — or a press at its
/// centre with the pointer already there — does not cover it: the pointer's
/// own image hides nothing, and no ladder can move it away. Another window at
/// that level, of another app or larger than a cursor, still covers.
#[test]
fn the_pointers_own_image_covers_nothing() {
    let image = DesktopWindow {
        id: 4,
        pid: 399,
        app: "Window Server".to_string(),
        rect: Rect::new(236.0, 180.0, 28.0, 40.0),
        own: false,
        layer: 2_147_483_630,
        alpha: 1.0,
        overlay: false,
    };
    let target = window(5, 10, 0, (100.0, 100.0, 300.0, 200.0));
    let place = Some(Rect::new(130.0, 75.0, 40.0, 50.0));
    let cover = cover_of(&[image.clone(), target.clone()], 5, place).expect("the target is listed");
    assert!(
        cover.coverers.is_empty() && cover.hidden_permille == 0 && !cover.blocks_a_press(),
        "the pointer's own image covers the place: {cover:?}"
    );
    let another_app = DesktopWindow {
        pid: 50,
        app: "app-50".to_string(),
        ..image.clone()
    };
    let larger = DesktopWindow {
        rect: Rect::new(100.0, 100.0, 300.0, 200.0),
        ..image
    };
    for over in [another_app, larger] {
        let cover = cover_of(&[over, target.clone()], 5, place).expect("listed");
        assert!(
            cover.hides_any(),
            "a window at the cursor level that is not the pointer's own covers"
        );
    }
}
