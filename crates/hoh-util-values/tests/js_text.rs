use hoh_util_values::{JS_WHITE_SPACE_SET, is_js_white_space, js_trim, js_trim_end, js_trim_start};
use regex::Regex;
use serde_json::Value;
use std::collections::BTreeSet;

fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/js_white_space.json")).unwrap()
}

#[test]
fn every_unicode_scalar_and_regex_class_agree_with_node() {
    let fixture = fixture();
    let expected: BTreeSet<u32> = fixture["whiteSpace"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| u32::try_from(v.as_u64().unwrap()).unwrap())
        .collect();
    assert_eq!(expected.len(), 25);
    let positive = Regex::new(&format!("^[{JS_WHITE_SPACE_SET}]+$")).unwrap();
    let negative = Regex::new(&format!("^[^{JS_WHITE_SPACE_SET}]$")).unwrap();
    let mut buffer = [0; 4];
    for ch in (0..=0x10ffff).filter_map(char::from_u32) {
        let wanted = expected.contains(&u32::from(ch));
        assert_eq!(is_js_white_space(ch), wanted, "U+{:04X}", u32::from(ch));
        let text = ch.encode_utf8(&mut buffer);
        assert_eq!(
            positive.is_match(text),
            wanted,
            "positive U+{:04X}",
            u32::from(ch)
        );
        assert_eq!(
            negative.is_match(text),
            !wanted,
            "negative U+{:04X}",
            u32::from(ch)
        );
    }
}

#[test]
fn every_trim_fixture_agrees_with_node() {
    for row in fixture()["trims"].as_array().unwrap() {
        let input = row["input"].as_str().unwrap();
        assert_eq!(js_trim(input), row["trim"].as_str().unwrap(), "{row}");
        assert_eq!(
            js_trim_start(input),
            row["trimStart"].as_str().unwrap(),
            "{row}"
        );
        assert_eq!(
            js_trim_end(input),
            row["trimEnd"].as_str().unwrap(),
            "{row}"
        );
    }
}
