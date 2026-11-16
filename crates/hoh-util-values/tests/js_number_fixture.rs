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

fn check_fixture(fixture: &str) -> usize {
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
    checked
}

#[test]
fn every_line_of_both_number_fixtures_matches_node() {
    let main = check_fixture(include_str!("fixtures/js_number_strings.tsv"));
    let ties = check_fixture(include_str!("fixtures/js_number_strings_ties.tsv"));
    println!("number fixture lines checked: {main} and {ties} (ties)");
    assert_eq!(ties, 26_708);
    assert!(main > 5000);
}

#[test]
fn halfway_cases_take_the_even_digit() {
    assert_eq!(js_number_string(HALFWAY), "1000000000000000.2");
    assert_eq!(js_number_string(-HALFWAY), "-1000000000000000.2");
}

/// 1000000000000000.25, written as bits because a decimal literal of it trips an excess-precision lint.
const HALFWAY: f64 = f64::from_bits(0x430c_6bf5_2634_0002);
