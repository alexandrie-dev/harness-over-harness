use hoh_util_crypto::{bytes_to_base64, random_uuid};
use std::collections::HashSet;
fn shape(value: &str) -> bool {
    regex::Regex::new("^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$")
        .unwrap()
        .is_match(value)
}
#[test]
fn emits_rfc_9562_v4_ids() {
    for _ in 0..64 {
        assert!(shape(random_uuid().unwrap().as_ref()));
    }
}
#[test]
fn emits_distinct_ids_across_calls() {
    assert_eq!(
        (0..64)
            .map(|_| random_uuid().unwrap())
            .collect::<HashSet<_>>()
            .len(),
        64
    );
}
#[test]
fn mints_without_platform_random_uuid() {
    // Rust has no ambient randomUUID API; the production function uses getrandom directly.
    assert!(shape(random_uuid().unwrap().as_ref()));
}
#[test]
fn encodes_empty_binary_and_multi_chunk_arrays() {
    assert_eq!(bytes_to_base64(&[]), "");
    assert_eq!(bytes_to_base64(&[0, 127, 128, 255]), "AH+A/w==");
    assert_eq!(bytes_to_base64(&vec![65; 0x8001]), "QUFB".repeat(10923));
}
