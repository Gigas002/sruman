use super::*;

#[test]
fn dates_are_local_minutes() {
    let text = date(1_700_000_000);
    // `2023-11-14 22:13` in UTC; the zone moves the clock, not the shape.
    assert_eq!(text.len(), 16, "{text}");
    assert!(text.starts_with("2023-11-1"), "{text}");
    assert_eq!(&text[10..11], " ");
    // Out of range: the raw number.
    assert_eq!(date(u64::MAX), u64::MAX.to_string());
}
