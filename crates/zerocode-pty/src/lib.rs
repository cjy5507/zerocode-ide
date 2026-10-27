//! Terminal hosting for ZeroCode.
//!
//! ZeroCode is a shell IDE: agents run as real processes inside its terminal,
//! not as a linked-in library. This crate is that terminal — it spawns a child
//! in a pty, owns the VT state machine in Rust, and hands the view layer a grid
//! of cells rather than a stream of escape sequences (ADR 0002).
//!
//! It is also where the independence contract lives. [`ZoBinary::discover`]
//! answers whether `zo` exists on this machine; when it does not, the lane
//! surfaces fold away and the IDE — files, editor, diff, source control,
//! terminal — keeps working.

pub mod answer;
pub mod briefing;
pub mod colors;
pub mod echo_shapes;
pub mod grid;
pub mod input;
pub mod lane;
mod packed_row;
#[cfg(target_os = "macos")]
mod posix_launch;
pub mod readers;
pub mod ready;
pub mod recording;
pub mod serialize;
pub mod zo;

pub use colors::{
    ANSI_COLOR_COUNT, Rgb, TerminalColors, format_x_color_rgb_spec, parse_x_color_spec,
    reads_as_dark, relative_luminance,
};
pub use echo_shapes::{EchoMatch, EchoShape, ExpectedEchoes, echo_shapes, locate_echo};
pub use grid::{
    BadPattern, Cell, CellStyle, Color, DEFAULT_SCROLLBACK_LINES, DeviceIdentity, FoldMeta,
    FoldRole, GlyphDrawn, GridDelta, GridNews, GridRow, MAX_SCROLLBACK_LINES, MIN_SCROLLBACK_LINES,
    MouseTracking, RowFold, TERMINAL_NAME, TERMINAL_VERSION, TermHit, TermPoint, Terminal,
    TerminalGrid, drawn,
};
pub use input::{
    KeyPress, MouseEvent, MouseKind, Words, encode_focus, encode_key, encode_mouse, encode_paste,
    encode_typed, sanitize_paste,
};
pub use lane::{
    DECLARED_TERMINAL, GroupPrograms, INHERITED_SESSION_MARKERS, INHERITED_TERMINAL_IDENTITY,
    PTY_OUTPUT_BYTE_BUDGET, PtyError, PtyLane, Pumped, ask_for_pty,
};
#[cfg(target_os = "macos")]
pub use posix_launch::{
    CTTY_DYLD_WRAP, CTTY_EXEC_ENV, CTTY_LOG_ENV, CTTY_PROBE_ENV, CTTY_PROBE_LIMIT, launch_through,
    launch_through_within, serve_ctty_exec,
};
pub use readers::{
    FrameReaders, Look, READER_SHARE_FRAMES, READER_SHARE_SCREENS, RENOTIFY_AFTER, ShareOf,
};
pub use ready::{
    EnterAgain, Observed, Outcome as DeliveryOutcome, PromptDelivery, Readiness, ReadySignal,
    State as ReadyState, Step as DeliveryStep,
};
pub use serialize::{
    POST_REPLAY_MODE_RESET, SCROLLBACK_BUFFER_BYTE_LIMIT, replay_payload, serialize_tail,
};
pub use zo::{ZO_EVENTS_ADDR_FILE_ENV, ZO_EXECUTABLE, ZoBinary};
