use super::*;

#[test]
#[should_panic(expected = "--size 必须在 1..=4096")]
fn oversized_size_is_rejected() {
    size_of(&["--size".to_string(), "100000".to_string()], 256);
}

#[test]
fn size_defaults_and_in_range_still_work() {
    assert_eq!(size_of(&[], 256), 256);
    assert_eq!(size_of(&["--size".to_string(), "64".to_string()], 256), 64);
    assert_eq!(
        size_of(&["--size".to_string(), "4096".to_string()], 256),
        4096
    );
}

#[test]
#[should_panic(expected = "--size 必须在 1..=4096")]
fn size_one_above_the_cap_is_rejected() {
    size_of(&["--size".to_string(), "4097".to_string()], 256);
}
