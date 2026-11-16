//! Differential check of `js_number_string` against a real JavaScript runtime.
//!
//! `fixtures/js_number_strings.tsv` was produced by Node 20 (`String(value)` for each
//! double, deterministic generator); each line is the IEEE-754 bits in hex, a tab, and
//! the JavaScript text.

use hoh_util_values::js_number_string;

#[test]
fn matches_node_for_every_fixture_value() {
    let fixture = include_str!("fixtures/js_number_strings.tsv");
    let mut checked = 0usize;
    let mut mismatches = Vec::new();
    for line in fixture.lines() {
        let (bits, expected) = line.split_once('\t').expect("fixture line has a tab");
        let value = f64::from_bits(u64::from_str_radix(bits, 16).expect("hex bits"));
        let actual = js_number_string(value);
        if actual != expected {
            mismatches.push(format!("{bits}: expected {expected}, got {actual}"));
        }
        checked += 1;
    }
    assert!(checked > 5000, "fixture is unexpectedly small: {checked}");
    assert!(
        mismatches.is_empty(),
        "{} of {checked} values differ from Node:\n{}",
        mismatches.len(),
        mismatches
            .iter()
            .take(20)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}
