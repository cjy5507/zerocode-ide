use fixture::inclusive_count;

#[test]
fn inclusive_end() {
    assert_eq!(inclusive_count(3, 3), 1);
}

#[test]
fn inclusive_range() {
    assert_eq!(inclusive_count(3, 5), 3);
}

#[test]
fn reversed_range_is_empty() {
    assert_eq!(inclusive_count(9, 2), 0);
}
