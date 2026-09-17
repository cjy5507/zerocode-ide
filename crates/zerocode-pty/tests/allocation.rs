use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use zerocode_pty::{Cell, MIN_SCROLLBACK_LINES, Terminal};

struct CountingAllocator;

static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);
static LIVE_BYTES: AtomicUsize = AtomicUsize::new(0);

#[global_allocator]
static GLOBAL: CountingAllocator = CountingAllocator;

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
            LIVE_BYTES.fetch_add(layout.size(), Ordering::Relaxed);
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) };
        LIVE_BYTES.fetch_sub(layout.size(), Ordering::Relaxed);
    }
}

/// Pin both sides of the grid's allocation contract: plain ASCII needs no
/// temporary grapheme buffer, and shrinking history releases whole row
/// buffers rather than retaining hidden capacity.
#[test]
fn hot_path_allocation_contracts() {
    let input = vec![b'x'; 512];
    let mut terminal = Terminal::new(4, input.len());
    ALLOCATIONS.store(0, Ordering::Relaxed);

    terminal.feed(&input);

    assert_eq!(
        ALLOCATIONS.load(Ordering::Relaxed),
        0,
        "plain ASCII feed allocated after terminal setup"
    );
    drop(terminal);

    // SGR parameters live in vte's fixed Params array. Walking them must not
    // rebuild that array as heap Vecs for every style transition.
    let input = b"\x1b[38;5;196mred\x1b[0m".repeat(64);
    let mut terminal = Terminal::new(4, 512);
    ALLOCATIONS.store(0, Ordering::Relaxed);
    terminal.feed(&input);
    assert_eq!(
        ALLOCATIONS.load(Ordering::Relaxed),
        0,
        "SGR feed allocated after terminal setup"
    );
    drop(terminal);

    // Erasing the display clears cells in their existing row buffers. A
    // repaint must not free and reallocate every row in the grid.
    let input = b"\x1b[2J\x1b[H".repeat(64);
    let mut terminal = Terminal::new(24, 80);
    ALLOCATIONS.store(0, Ordering::Relaxed);
    terminal.feed(&input);
    assert_eq!(
        ALLOCATIONS.load(Ordering::Relaxed),
        0,
        "full-screen clears replaced row allocations"
    );
    drop(terminal);

    // A mixed boundary used to allocate both the pending grapheme String and
    // a second cluster-probe String. The in-place append/rollback path needs
    // one reusable buffer for the whole stream.
    let input = "a가".repeat(128);
    let mut terminal = Terminal::new(4, 512);
    ALLOCATIONS.store(0, Ordering::Relaxed);
    terminal.feed(input.as_bytes());
    assert_eq!(
        ALLOCATIONS.load(Ordering::Relaxed),
        1,
        "mixed Unicode feed did not keep one reusable grapheme buffer"
    );
    drop(terminal);

    // Inspecting a stored wide cluster during overwrite fits in Cell's fixed
    // scalar ceiling and therefore needs no temporary heap String.
    let mut terminal = Terminal::new(4, 80);
    terminal.feed("☂️".as_bytes());
    ALLOCATIONS.store(0, Ordering::Relaxed);
    terminal.feed(b"\rX");
    assert_eq!(
        ALLOCATIONS.load(Ordering::Relaxed),
        0,
        "wide-cluster overwrite allocated a width probe"
    );
    drop(terminal);

    // Lowering the configured cap must release the oldest row buffers, not
    // only shorten a logical length while retaining their allocations. Since
    // t-1956, a history row stores only through its last non-default cell, so
    // the releasable bill follows the written width rather than `COLS`.
    const COLS: usize = 80;
    const FILLED_LINES: usize = MIN_SCROLLBACK_LINES * 2;
    const ROW_TEXT: &str = "one retained scrollback row";
    let input = format!("{ROW_TEXT}\r\n")
        .repeat(FILLED_LINES + 64)
        .into_bytes();
    let before = LIVE_BYTES.load(Ordering::Relaxed);
    let mut terminal = Terminal::new(24, COLS);
    terminal.grid_mut().set_scrollback_cap(FILLED_LINES);
    terminal.feed(&input);
    assert_eq!(terminal.grid().scrollback_len(), FILLED_LINES);
    let filled = LIVE_BYTES.load(Ordering::Relaxed);
    let full_width_storage = FILLED_LINES * COLS * size_of::<Cell>();
    assert!(
        filled.saturating_sub(before) < full_width_storage,
        "short rows cost as much as full-width history: before={before}, filled={filled}, \
         full-width bill={full_width_storage}"
    );

    terminal.grid_mut().set_scrollback_cap(MIN_SCROLLBACK_LINES);

    assert_eq!(terminal.grid().scrollback_len(), MIN_SCROLLBACK_LINES);
    let shrunk = LIVE_BYTES.load(Ordering::Relaxed);
    let released = filled.saturating_sub(shrunk);
    let written_width = ROW_TEXT.chars().count();
    let row_storage = (FILLED_LINES - MIN_SCROLLBACK_LINES) * written_width * size_of::<Cell>();
    assert!(
        released >= row_storage,
        "cap shrink released {released} bytes, expected at least {row_storage}; \
         live bytes before grid={before}, filled={filled}, shrunk={shrunk}"
    );
}
