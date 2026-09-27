//! The rules on the words people's commands hold (t-10916): the window reads
//! every shell command an agent pane runs with them, so a command no test
//! spelled is one they meet.

use super::*;

/// A value after `KEY=` is read where it stands in the command: whitespace
/// before it is skipped in the command itself, not only in a trimmed copy —
/// so the command after it is the first one, and a word of letters wider
/// than a byte after the space is never cut in two (a transcript on this
/// machine held `…= (규제 준수 현황 cards, …)`, and the rules panicked on it).
#[test]
fn an_assignments_value_is_read_where_it_stands_in_the_command() {
    assert_eq!(extract_first_command("KEY=value ls"), "ls");
    assert_eq!(extract_first_command("KEY=\"a b\" ls -la"), "ls");
    assert_eq!(extract_first_command("KEY= value ls"), "ls");
    assert_eq!(extract_first_command("KEY= \"a b\" ls"), "ls");
    let wide = "SUMMARY= (규제 준수 현황 cards, 망분리 · 개인정보, KPI bands). cat notes.md";
    assert_eq!(extract_first_command(wide), "준수");
    // Read through whole by the reader the window's pane guard asks — its
    // verdict is the table's (a name it does not list is not a write).
    let _ = proven_read_only(wide);
}
