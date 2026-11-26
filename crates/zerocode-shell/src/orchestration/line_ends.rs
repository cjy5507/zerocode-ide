//! A build line job's end the line could not tell, told by the window
//! (t-131644). Red: nothing is told yet.

use zerocode_core::line::Line;

/// One pass over `line`: nothing yet.
pub(super) fn sweep_line(_line: &Line, _now_ms: i64) {}
