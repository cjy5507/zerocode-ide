/// Inclusive end of a range, including a singleton.
pub fn inclusive_count(start: u32, end: u32) -> u32 {
    end.saturating_sub(start)
}
