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
