/// Inclusive end of a range, including a singleton.
pub fn inclusive_count(start: u32, end: u32) -> u32 {
    end.saturating_sub(start)
}

#[cfg(test)]
mod tests {
    #[test]
    fn inclusive_end() { assert_eq!(super::inclusive_count(3, 3), 1); }
    #[test]
    fn reversed_range_is_empty() { assert_eq!(super::inclusive_count(9, 2), 0); }
}
