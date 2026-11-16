//! `js_json_parse`, `js_json_stringify` and `js_json_stringify_pretty` against the fixtures written by
//! `tests/fixtures/gen-js-json.mjs` (Node v20.20.2), plus cases of their own.

use hoh_util_values::{
    JS_JSON_MAX_DEPTH, JsonParseError, js_json_parse, js_json_stringify, js_json_stringify_pretty,
};
use serde_json::{Map, Number, Value, json};

/// A fixture line may hold an escaped lone surrogate (V8 quotes one UTF-16 unit of the source in its
/// message), which `serde_json` refuses; it is read as U+FFFD, the documented difference.
fn replace_lone_surrogates(line: &str) -> String {
    let chars: Vec<char> = line.chars().collect();
    let unit_at = |at: usize| -> Option<u32> {
        if chars.get(at) == Some(&'\\') && chars.get(at + 1) == Some(&'u') {
            let digits: String = chars.get(at + 2..at + 6)?.iter().collect();
            u32::from_str_radix(&digits, 16).ok()
        } else {
            None
        }
    };
    let mut out = String::new();
    let mut at = 0;
    while at < chars.len() {
        if chars[at] == '\\' && chars.get(at + 1) == Some(&'u') {
            let unit = unit_at(at);
            let is_pair = matches!(unit, Some(0xd800..=0xdbff))
                && matches!(unit_at(at + 6), Some(0xdc00..=0xdfff));
            match unit {
                Some(0xd800..=0xdfff) if !is_pair => out.push_str("\\ufffd"),
                Some(_) if is_pair => {
                    out.extend(&chars[at..at + 12]);
                    at += 6;
                }
                _ => out.extend(&chars[at..at + 6]),
            }
            at += 6;
        } else if chars[at] == '\\' {
            out.extend(&chars[at..(at + 2).min(chars.len())]);
            at += 2;
        } else {
            out.push(chars[at]);
            at += 1;
        }
    }
    out
}

fn fixture_lines(name: &str) -> Vec<Vec<Value>> {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    let content = std::fs::read_to_string(&path).unwrap();
    let mut lines = content.lines();
    let header: Value = serde_json::from_str(lines.next().unwrap()).unwrap();
    assert_eq!(header, json!({"node": "v20.20.2"}));
    lines
        .map(|line| {
            match serde_json::from_str::<Value>(line)
                .or_else(|_| serde_json::from_str(&replace_lone_surrogates(line)))
            {
                Ok(Value::Array(items)) => items,
                other => panic!("bad fixture line {line:?}: {other:?}"),
            }
        })
        .collect()
}

fn text(value: &Value) -> &str {
    value.as_str().unwrap()
}

#[test]
fn roundtrip_fixture() {
    let lines = fixture_lines("js_json_roundtrip.jsonl");
    for line in &lines {
        let (input, compact, pretty) = (text(&line[0]), text(&line[1]), text(&line[2]));
        let parsed = js_json_parse(input)
            .unwrap_or_else(|error| panic!("{input:?} failed to parse: {error}"));
        assert_eq!(js_json_stringify(&parsed), compact, "compact of {input:?}");
        assert_eq!(
            js_json_stringify_pretty(&parsed, 2),
            pretty,
            "pretty of {input:?}"
        );
    }
    assert_eq!(lines.len(), 2088);
    println!("round trip lines checked: {}", lines.len());
}

#[test]
fn parse_error_fixture() {
    let lines = fixture_lines("js_json_parse_errors.jsonl");
    for line in &lines {
        let (input, message) = (text(&line[0]), text(&line[1]));
        match js_json_parse(input) {
            Ok(value) => panic!("{input:?} parsed to {value} instead of failing with {message:?}"),
            Err(error) => assert_eq!(error.message(), message, "message for {input:?}"),
        }
    }
    assert_eq!(lines.len(), 1799);
    println!("parse error lines checked: {}", lines.len());
}

#[test]
fn error_displays_the_message() {
    let error: JsonParseError = js_json_parse("[1,]").unwrap_err();
    assert_eq!(error.to_string(), error.message());
}

#[test]
fn built_values_use_javascript_key_order() {
    let mut map = Map::new();
    for key in [
        "b",
        "2",
        "a",
        "10",
        "01",
        "-1",
        "1.5",
        "4294967295",
        "1",
        "4294967294",
    ] {
        map.insert(key.to_owned(), json!(0));
    }
    assert_eq!(
        js_json_stringify(&Value::Object(map)),
        r#"{"1":0,"2":0,"10":0,"4294967294":0,"b":0,"a":0,"01":0,"-1":0,"1.5":0,"4294967295":0}"#
    );
    let nested = json!({"z": {"2": 1, "1": 2}});
    assert_eq!(js_json_stringify(&nested), r#"{"z":{"1":2,"2":1}}"#);
}

#[test]
fn indent_is_clamped_to_ten() {
    let value = json!({"a": [1, {}], "b": []});
    assert_eq!(
        js_json_stringify_pretty(&value, 0),
        js_json_stringify(&value)
    );
    assert_eq!(
        js_json_stringify_pretty(&value, 1),
        "{\n \"a\": [\n  1,\n  {}\n ],\n \"b\": []\n}"
    );
    assert_eq!(
        js_json_stringify_pretty(&value, 4),
        "{\n    \"a\": [\n        1,\n        {}\n    ],\n    \"b\": []\n}"
    );
    let ten = " ".repeat(10);
    let expected = format!("{{\n{ten}\"a\": 1\n}}");
    assert_eq!(js_json_stringify_pretty(&json!({"a": 1}), 10), expected);
    assert_eq!(js_json_stringify_pretty(&json!({"a": 1}), 11), expected);
    assert_eq!(js_json_stringify_pretty(&json!([]), 4), "[]");
    assert_eq!(js_json_stringify_pretty(&json!(5), 4), "5");
}

#[test]
fn escaped_lone_surrogate_becomes_the_replacement_character() {
    assert_eq!(js_json_parse(r#""\ud800""#).unwrap(), json!("\u{fffd}"));
    assert_eq!(js_json_parse(r#""\ude00x""#).unwrap(), json!("\u{fffd}x"));
    assert_eq!(js_json_parse(r#""😀""#).unwrap(), json!("😀"));
}

#[test]
fn infinity_literal_is_stored_as_null() {
    assert_eq!(js_json_parse("1e400").unwrap(), Value::Null);
    assert_eq!(js_json_parse("-1e400").unwrap(), Value::Null);
    assert_eq!(
        js_json_stringify(&js_json_parse("[1e400]").unwrap()),
        "[null]"
    );
}

#[test]
fn negative_zero() {
    let parsed = js_json_parse("-0").unwrap();
    assert_eq!(parsed.as_f64().map(f64::is_sign_negative), Some(true));
    assert_eq!(parsed.as_i64(), None);
    assert_eq!(js_json_stringify(&parsed), "0");
    assert_eq!(
        js_json_stringify(&Value::Number(Number::from_f64(-0.0).unwrap())),
        "0"
    );
    assert_eq!(js_json_parse("0").unwrap().as_i64(), Some(0));
}

#[test]
fn integers_around_two_to_the_53() {
    assert_eq!(
        js_json_stringify(&json!(9_007_199_254_740_992_i64)),
        "9007199254740992"
    );
    assert_eq!(
        js_json_stringify(&json!(9_007_199_254_740_993_i64)),
        "9007199254740992"
    );
    assert_eq!(
        js_json_stringify(&json!(9_007_199_254_740_995_i64)),
        "9007199254740996"
    );
    assert_eq!(
        js_json_stringify(&json!(-9_007_199_254_740_993_i64)),
        "-9007199254740992"
    );
    assert_eq!(js_json_stringify(&json!(i64::MIN)), "-9223372036854776000");
    assert_eq!(js_json_stringify(&json!(u64::MAX)), "18446744073709552000");
    assert_eq!(
        js_json_parse("9007199254740992").unwrap().as_i64(),
        Some(9_007_199_254_740_992)
    );
    // The double nearest to 2^53+1 is 2^53, which is stored as an integer.
    assert_eq!(
        js_json_parse("9007199254740993").unwrap().as_i64(),
        Some(9_007_199_254_740_992)
    );
    assert_eq!(js_json_parse("9007199254740994").unwrap().as_i64(), None);
    assert_eq!(
        js_json_stringify(&js_json_parse("9007199254740993").unwrap()),
        "9007199254740992"
    );
    assert_eq!(
        js_json_stringify(&js_json_parse("1e20").unwrap()),
        "100000000000000000000"
    );
    assert_eq!(js_json_stringify(&js_json_parse("1.0").unwrap()), "1");
}

#[test]
fn escapes_follow_json_stringify() {
    let value = json!("a\"b\\c\u{8}\u{c}\n\r\t\u{1}\u{1f}\u{7f}\u{2028}\u{2029}/é😀");
    assert_eq!(
        js_json_stringify(&value),
        "\"a\\\"b\\\\c\\b\\f\\n\\r\\t\\u0001\\u001f\u{7f}\u{2028}\u{2029}/é😀\""
    );
}

#[test]
fn duplicate_key_keeps_first_position_and_last_value() {
    let parsed = js_json_parse(r#"{"a":1,"b":2,"a":3}"#).unwrap();
    assert_eq!(js_json_stringify(&parsed), r#"{"a":3,"b":2}"#);
}

/// Take a value apart without recursion.
fn dismantle(value: Value) {
    let mut work = vec![value];
    while let Some(item) = work.pop() {
        match item {
            Value::Array(items) => work.extend(items),
            Value::Object(map) => work.extend(map.into_iter().map(|(_, item)| item)),
            _ => {}
        }
    }
}

#[test]
fn deep_nesting_needs_no_deep_call_stack() {
    let handle = std::thread::Builder::new()
        .stack_size(256 * 1024)
        .spawn(|| {
            let depth = 4000;
            let text = format!("{}{}", "[".repeat(depth), "]".repeat(depth));
            let value = js_json_parse(&text).unwrap();
            assert_eq!(js_json_stringify(&value), text);
            assert_eq!(
                js_json_stringify_pretty(&value, 1).lines().count(),
                2 * depth - 1
            );
            dismantle(value);
            let objects = format!("{}1{}", "{\"a\":".repeat(depth), "}".repeat(depth));
            let value = js_json_parse(&objects).unwrap();
            assert_eq!(js_json_stringify(&value), objects);
            dismantle(value);
            // A failure deep inside must also release the partial value without recursion.
            let broken = format!("{}x", "[[".repeat(depth));
            assert!(js_json_parse(&broken).is_err());
            let deep_then_error = format!("[{}{},", "[".repeat(depth), "]".repeat(depth));
            assert!(js_json_parse(&deep_then_error).is_err());
        })
        .unwrap();
    handle.join().unwrap();
}

#[test]
fn depth_limit_is_4096_levels() {
    let handle = std::thread::Builder::new()
        .stack_size(256 * 1024)
        .spawn(|| {
            assert_eq!(JS_JSON_MAX_DEPTH, 4096);
            let nested = |depth: usize| format!("{}{}", "[".repeat(depth), "]".repeat(depth));
            let value = js_json_parse(&nested(4096)).unwrap();
            dismantle(value);
            let error = js_json_parse(&nested(4097)).unwrap_err();
            assert_eq!(error.message(), "Maximum call stack size exceeded");
            let error = js_json_parse(&format!("{}1", "{\"a\":".repeat(4097))).unwrap_err();
            assert_eq!(error.message(), "Maximum call stack size exceeded");
        })
        .unwrap();
    handle.join().unwrap();
}
