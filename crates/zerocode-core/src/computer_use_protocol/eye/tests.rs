use serde_json::json;

use super::*;

const ACT: Mark = Mark {
    seq: 10,
    at_ms: 100_000,
};

fn window(id: u64, own: bool, rect: (f64, f64, f64, f64)) -> DesktopWindow {
    DesktopWindow {
        id,
        pid: i64::try_from(id).unwrap(),
        app: format!("App {id}"),
        rect: Rect::new(rect.0, rect.1, rect.2, rect.3),
        own,
        layer: 0,
        alpha: 1.0,
        overlay: false,
    }
}

fn change(seq: u64, at_ms: i64, rect: (f64, f64, f64, f64)) -> Change {
    Change {
        seq,
        at_ms,
        rects: vec![Rect::new(rect.0, rect.1, rect.2, rect.3)],
    }
}

/// ZeroCode's pane on the left, Mail on the right, a menu over the pane's
/// lower corner.
fn desk() -> Vec<DesktopWindow> {
    let mut menu = window(3, false, (300.0, 500.0, 200.0, 200.0));
    menu.layer = 101;
    vec![
        menu,
        window(1, true, (0.0, 0.0, 700.0, 900.0)),
        window(2, false, (700.0, 0.0, 800.0, 900.0)),
    ]
}

#[test]
fn a_repaint_is_zerocodes_own_only_where_its_window_is_the_one_in_front() {
    let desk = desk();
    assert!(
        owned(Rect::new(10.0, 10.0, 100.0, 20.0), &desk),
        "the pane's own line"
    );
    assert!(
        !owned(Rect::new(350.0, 550.0, 20.0, 20.0), &desk),
        "the menu stands in front of the pane there"
    );
    assert!(
        !owned(Rect::new(650.0, 10.0, 100.0, 20.0), &desk),
        "half of it is Mail's"
    );
    assert!(
        !owned(Rect::new(10.0, 950.0, 20.0, 20.0), &desk),
        "the bare desktop is nobody's"
    );
    let mut clear = desk.clone();
    clear[0].alpha = 0.0;
    assert!(
        owned(Rect::new(350.0, 550.0, 20.0, 20.0), &clear),
        "a window that cannot be seen hides nothing"
    );
}

#[test]
fn zerocodes_region_is_its_windows_less_whatever_stands_in_front_of_them() {
    let region = own_region(&desk());
    let shown: f64 = region.iter().map(Rect::area).sum();
    assert!(
        (shown - (700.0 * 900.0 - 200.0 * 200.0)).abs() < 1e-9,
        "the pane less the menu over it: {region:?}"
    );
    assert!(
        region
            .iter()
            .all(|piece| !piece.intersects(&Rect::new(300.0, 500.0, 200.0, 200.0)))
    );
    // Two of ZeroCode's windows overlapping: the region counts the overlap once.
    let popout = window(4, true, (600.0, 100.0, 300.0, 300.0));
    let mut two = vec![popout];
    two.extend(desk());
    let shown: f64 = own_region(&two).iter().map(Rect::area).sum();
    assert!((shown - (700.0 * 900.0 - 200.0 * 200.0 + 200.0 * 300.0)).abs() < 1e-9);
}

/// This Mac on macOS 26 (2026-09-12): the menu bar, the Dock's window over
/// the whole screen at the Dock's layer, then ZeroCode filling the rest.
/// The helper calls the Dock's window an overlay; were it taken for a window
/// in front, nothing would ever be ZeroCode's and its pane's repaints would
/// hold every look back.
#[test]
fn an_overlay_in_front_leaves_zerocodes_window_its_own() {
    let mut menu_bar = window(9, false, (0.0, 0.0, 1512.0, 33.0));
    menu_bar.layer = 24;
    let mut dock = window(11, false, (0.0, 0.0, 1512.0, 982.0));
    dock.layer = 20;
    dock.overlay = true;
    let desk = vec![
        menu_bar,
        dock.clone(),
        window(1, true, (0.0, 33.0, 1512.0, 879.0)),
        window(2, false, (0.0, 33.0, 1512.0, 884.0)),
    ];
    assert_eq!(own_region(&desk), vec![Rect::new(0.0, 33.0, 1512.0, 879.0)]);
    assert!(
        owned(Rect::new(800.0, 100.0, 300.0, 20.0), &desk),
        "a line in the pane"
    );
    assert!(
        !owned(Rect::new(800.0, 910.0, 300.0, 20.0), &desk),
        "what shows below ZeroCode is not"
    );
    let mut taken = desk.clone();
    taken[1].overlay = false;
    assert!(own_region(&taken).is_empty());
}

#[test]
fn a_wait_counts_neither_zerocodes_pane_nor_what_was_already_moving() {
    let desk = desk();
    let spinner = (900.0, 40.0, 16.0, 16.0);
    let changes = vec![
        change(8, ACT.at_ms - 300, spinner),
        change(9, ACT.at_ms - 1_000, (900.0, 400.0, 50.0, 50.0)),
        change(10, ACT.at_ms - 10, spinner),
        change(11, ACT.at_ms + 20, (10.0, 10.0, 300.0, 18.0)),
        change(12, ACT.at_ms + 30, spinner),
        change(13, ACT.at_ms + 40, (800.0, 200.0, 300.0, 40.0)),
    ];
    let ignore = Ignore::new(&changes, ACT, &desk);
    let counted: Vec<u64> = ignore
        .after(&changes, ACT)
        .iter()
        .map(|change| change.seq)
        .collect();
    assert_eq!(
        counted,
        [13],
        "the pane's line (11) is ZeroCode's, the spinner (12) was moving before the act"
    );
    let old = Ignore::new(&changes, ACT, &desk);
    assert!(
        old.counts(&changes[1]),
        "a repaint a second before the act is not the screen's motion now"
    );
    let reading = Ignore::new(&changes, ACT, &desk).within(Rect::new(0.0, 300.0, 1500.0, 600.0));
    assert!(
        !reading.counts(&changes[5]),
        "an OCR wait over the lower part does not read the top again"
    );
}

#[test]
fn the_look_after_an_act_waits_for_its_paint_to_finish_and_no_longer() {
    let desk = desk();
    let at = |ms: i64| ACT.at_ms + ms;
    // Nothing counted: the table's settle since the act, then look.
    assert_eq!(settle(&[], ACT, &desk, at(0)), Settle::Wait);
    let settle_ms = i64::try_from(COMPUTER_SETTLE_MS).unwrap();
    assert_eq!(settle(&[], ACT, &desk, at(settle_ms - 1)), Settle::Wait);
    assert_eq!(
        settle(&[], ACT, &desk, at(settle_ms)),
        Settle::Look { settled: true }
    );
    // A sheet slides in for 90 ms: look once it has been still for the quiet.
    let sheet: Vec<Change> = (0..4)
        .map(|frame| {
            change(
                11 + frame,
                at(30 + 30 * i64::try_from(frame).unwrap()),
                (800.0, 100.0, 400.0, 300.0),
            )
        })
        .collect();
    let quiet = i64::try_from(EYE_QUIET_MS).unwrap();
    assert_eq!(settle(&sheet, ACT, &desk, at(130)), Settle::Wait);
    assert_eq!(
        settle(&sheet, ACT, &desk, at(120 + quiet)),
        Settle::Look { settled: true },
        "the last frame at 120 ms, still since"
    );
    // A video that never stops: the cap, and the look says so.
    let video: Vec<Change> = (0..40)
        .map(|frame| {
            change(
                11 + frame,
                at(33 * i64::try_from(frame).unwrap()),
                (800.0, 400.0, 320.0, 180.0),
            )
        })
        .collect();
    let cap = i64::try_from(EYE_SETTLE_MAX_MS).unwrap();
    assert_eq!(settle(&video, ACT, &desk, at(cap - 1)), Settle::Wait);
    assert_eq!(
        settle(&video, ACT, &desk, at(cap)),
        Settle::Look { settled: false }
    );
    // ZeroCode's pane scrolling the agent's words the whole time holds nothing back.
    let pane: Vec<Change> = (0..20)
        .map(|frame| {
            change(
                11 + frame,
                at(20 * i64::try_from(frame).unwrap()),
                (10.0, 800.0, 600.0, 18.0),
            )
        })
        .collect();
    assert_eq!(
        settle(&pane, ACT, &desk, at(settle_ms)),
        Settle::Look { settled: true }
    );
    // An act long past: whatever the screen does now, look at once — still
    // moving is said, stopped long ago is settled.
    let playing: Vec<Change> = (0..152)
        .map(|frame| {
            change(
                11 + frame,
                at(33 * i64::try_from(frame).unwrap()),
                (800.0, 400.0, 320.0, 180.0),
            )
        })
        .collect();
    assert_eq!(
        settle(&playing, ACT, &desk, at(5_000)),
        Settle::Look { settled: false }
    );
    assert_eq!(
        settle(&video, ACT, &desk, at(5_000)),
        Settle::Look { settled: true }
    );
    // A repaint before the act is not its doing.
    let before = [change(9, at(-2_000), (800.0, 100.0, 400.0, 300.0))];
    assert_eq!(settle(&before, ACT, &desk, at(10)), Settle::Wait);
}

#[test]
fn a_watch_ends_on_a_counted_change_or_on_stillness() {
    let desk = desk();
    let start = ACT;
    let at = |ms: i64| start.at_ms + ms;
    let quiet = i64::try_from(EYE_QUIET_MS).unwrap();
    assert_eq!(
        watched(Until::Change, &[], start, &desk, at(5_000)),
        Watched::Wait
    );
    let changes = vec![
        change(11, at(40), (10.0, 10.0, 300.0, 18.0)),
        change(12, at(700), (800.0, 200.0, 300.0, 40.0)),
        change(13, at(730), (800.0, 240.0, 300.0, 40.0)),
    ];
    assert_eq!(
        watched(Until::Change, &changes, start, &desk, at(731)),
        Watched::Changed {
            first_ms: at(700),
            rects: vec![
                Rect::new(800.0, 200.0, 300.0, 40.0),
                Rect::new(800.0, 240.0, 300.0, 40.0)
            ],
        },
        "ZeroCode's pane at 40 ms is not a change"
    );
    assert_eq!(
        watched(Until::Quiet, &[], start, &desk, at(quiet)),
        Watched::Quiet {
            last_ms: start.at_ms,
            rects: Vec::new()
        },
        "a still screen is quiet at once"
    );
    assert_eq!(
        watched(Until::Quiet, &changes, start, &desk, at(730 + quiet - 1)),
        Watched::Wait
    );
    assert!(matches!(
        watched(Until::Quiet, &changes, start, &desk, at(730 + quiet)),
        Watched::Quiet { last_ms, .. } if last_ms == at(730)
    ));
    assert_eq!(Until::from_word(None), Some(Until::Change));
    assert_eq!(Until::from_word(Some("quiet")), Some(Until::Quiet));
    assert_eq!(Until::from_word(Some("still")), None);
    for word in crate::computer_use::WATCH_UNTIL {
        assert!(Until::from_word(Some(word)).is_some(), "{word}");
    }
}

#[test]
fn the_helpers_answer_is_read_whole_or_not_at_all() {
    let mut answer = json!({
        SEQ_KEY: 14, NOW_KEY: 100_500, STREAMING_KEY: true, WHOLE_KEY: false,
        ACT_KEY: { SEQ_KEY: 10, AT_KEY: 100_000 },
        CHANGES_KEY: [
            { SEQ_KEY: 13, AT_KEY: 100_400, RECTS_KEY: [[1.0, 2.0, 3.0, 4.0], [5, 6, 7, 8]] },
            { SEQ_KEY: 14, AT_KEY: 100_450, RECTS_KEY: [[1.0, 2.0, 3.0]] },
        ],
    });
    assert!(
        Changes::from_answer(&answer).is_none(),
        "a broken repaint cannot silently disappear"
    );
    answer[CHANGES_KEY].as_array_mut().unwrap().pop();
    let read = Changes::from_answer(&answer).expect("a complete answer");
    assert_eq!(
        (read.seq, read.now_ms, read.streaming, read.whole),
        (14, 100_500, true, false)
    );
    assert_eq!(read.act, Some(ACT));
    assert_eq!(read.changes.len(), 1, "every returned repaint is valid");
    assert_eq!(
        read.changes[0].rects,
        [Rect::new(1.0, 2.0, 3.0, 4.0), Rect::new(5.0, 6.0, 7.0, 8.0)]
    );
    let plain = Changes::from_answer(&json!({ SEQ_KEY: 0, NOW_KEY: 1 })).unwrap();
    assert_eq!(
        (plain.act, plain.whole, plain.streaming, plain.changes.len()),
        (None, true, false, 0)
    );
    assert!(Changes::from_answer(&json!({ NOW_KEY: 1 })).is_none());
    let absent = Changes::from_answer(
        &json!({ SEQ_KEY: 0, NOW_KEY: 1, ACT_KEY: null, STREAM_ID_KEY: null }),
    )
    .unwrap();
    assert!(absent.act.is_none() && absent.stream_id.is_none());
}

#[test]
fn a_cursor_detects_lost_history_and_replaced_streams_even_after_their_sequences_catch_up() {
    let initial = json!({ SEQ_KEY: 14, NOW_KEY: 1_000, STREAMING_KEY: true, WHOLE_KEY: true, STREAM_ID_KEY: "first" });
    let cursor = Changes::from_answer(&initial).unwrap().cursor();
    let next = json!({ SEQ_KEY: 15, NOW_KEY: 1_050, STREAMING_KEY: true, WHOLE_KEY: true, STREAM_ID_KEY: "first" });
    assert!(Changes::from_answer(&next).unwrap().continues(&cursor));
    for (key, value) in [
        (WHOLE_KEY, json!(false)),
        (STREAMING_KEY, json!(false)),
        (SEQ_KEY, json!(1)),
        (NOW_KEY, json!(900)),
        (STREAM_ID_KEY, json!("second")),
    ] {
        let mut changed = next.clone();
        changed[key] = value;
        assert!(
            !Changes::from_answer(&changed).unwrap().continues(&cursor),
            "{key}"
        );
    }
    let mut legacy = initial.clone();
    legacy.as_object_mut().unwrap().remove(STREAM_ID_KEY);
    let legacy = Changes::from_answer(&legacy).unwrap();
    assert!(
        legacy.continues(&legacy.cursor()),
        "old helpers retain sequence/time checks"
    );
    assert!(
        !legacy.continues(&cursor),
        "an identity cannot disappear within a known stream"
    );
}

#[test]
fn malformed_stream_metadata_and_change_lists_are_not_empty_intervals() {
    for (key, value) in [
        (STREAM_ID_KEY, json!("")),
        (STREAM_ID_KEY, json!(7)),
        (CHANGES_KEY, json!({})),
        (CHANGES_KEY, json!([null])),
        (WHOLE_KEY, json!("false")),
        (STREAMING_KEY, json!(1)),
        (ACT_KEY, json!({})),
        (
            CHANGES_KEY,
            json!([{ SEQ_KEY: 1, AT_KEY: 10, RECTS_KEY: [[0, 0, -1, 5]] }]),
        ),
    ] {
        let mut answer = json!({ SEQ_KEY: 1, NOW_KEY: 10 });
        answer[key] = value;
        assert!(Changes::from_answer(&answer).is_none(), "{answer}");
    }
}
