//! Port of `packages/util/values/tests/partial-json.spec.ts`.
//!
//! Spy-based checks of the source (`JSON.parse`, `String.prototype.indexOf`, `Array.prototype.join`
//! call counts) are replaced as described in `PORT_NOTES.md`.

use hoh_util_values::{LengthReadOptions, PartialArguments, deep_equal_json};
use serde_json::{Value, json};

/// Complete, valid objects exercising strings, escapes, scalars, nested values, and whitespace.
const CLOSED_TEXTS: [&str; 7] = [
    r#"{"file_path":"src/a.ts","content":"line 1\nline \"2\"\t\u4e2d\ud83d\ude00","n":-1.5e3,"t":true}"#,
    r#" { "skip" : "a\"}{b\\" , "todos" : [ { "content" : "x}" } , [ [ ] ] ] , "file_path" : "y" , "content" : "\u00E9" } "#,
    r#"{"n":1 ,"content":"","m":null}"#,
    r#"{"o":{"k":[1,2,{"s":"\u00e9]}"}]},"n":-0.5e1}"#,
    r#"{"file\u005fpath":"x","content":"a\\b\/c"}"#,
    r#"{"__proto__":{"polluted":true},"constructor":"c","a":1}"#,
    "{}",
];

/// Invalid structure or value contents, inspected by the full-reader oracle.
const INVALID_TEXTS: [&str; 7] = [
    r#"{"file_path":"ab\q"}"#,
    r#"{"file_path":"ab\u12G4"}"#,
    "{\"file_path\":\"ab\nc\"}",
    "{\"file_path\":\"ab\tc\"}",
    r#"{"a":[1,{"b":tru}]}"#,
    r#"{"n":1.}"#,
    r#"{"a":"b" x"#,
];

fn all_texts() -> Vec<&'static str> {
    CLOSED_TEXTS
        .iter()
        .chain(INVALID_TEXTS.iter())
        .copied()
        .collect()
}

/// Cut `text` (ASCII) into consecutive slices whose sizes come from `next_size`.
fn slices(text: &str, mut next_size: impl FnMut() -> usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut index = 0;
    while index < text.len() {
        let size = next_size();
        let end = (index + size).min(text.len());
        out.push(text[index..end].to_owned());
        index += size;
    }
    out
}

/// Deterministic slice sizes in 1..3 (mulberry32), so a failing split is reproducible from its seed.
fn seeded_sizes(seed: u32) -> impl FnMut() -> usize {
    let mut state = seed;
    move || {
        state = state.wrapping_add(0x6d2b_79f5);
        let mut t = state;
        t = (t ^ (t >> 15)).wrapping_mul(t | 1);
        t ^= t.wrapping_add((t ^ (t >> 7)).wrapping_mul(t | 61));
        (((t ^ (t >> 14)) % 3) + 1) as usize
    }
}

#[derive(Debug, Clone)]
struct FieldView {
    has: bool,
    complete: bool,
    length: Option<usize>,
    prefix: Option<Vec<u16>>,
    exceeds: bool,
    text: Option<Vec<u16>>,
    value: Option<Value>,
}

impl PartialEq for FieldView {
    fn eq(&self, other: &Self) -> bool {
        let values = match (&self.value, &other.value) {
            (None, None) => true,
            (Some(a), Some(b)) => deep_equal_json(a, b),
            _ => false,
        };
        values
            && self.has == other.has
            && self.complete == other.complete
            && self.length == other.length
            && self.prefix == other.prefix
            && self.exceeds == other.exceeds
            && self.text == other.text
    }
}

#[derive(Debug, Clone, PartialEq)]
struct ViewSnapshot {
    invalid: bool,
    closed: bool,
    keys: Vec<String>,
    fields: Vec<(String, FieldView)>,
}

fn len(view: &mut PartialArguments, key: &str) -> Option<usize> {
    view.string_length(key, LengthReadOptions::default())
}

fn len_step(view: &mut PartialArguments, key: &str, step: f64) -> Option<usize> {
    view.string_length(
        key,
        LengthReadOptions {
            step: Some(step),
            offset: None,
        },
    )
}

/// Read every answer for `keys` (default: the view's own keys); on a streaming view this also registers the reads.
fn snapshot_with(view: &mut PartialArguments, keys: Option<&[String]>) -> ViewSnapshot {
    let keys: Vec<String> = keys.map_or_else(|| view.keys(), <[String]>::to_vec);
    let fields = keys
        .iter()
        .map(|key| {
            let length = len(view, key);
            let prefix = view.text_prefix_utf16(key, 4);
            let exceeds = view.string_exceeds(key, 4);
            let text = view.text_utf16(key);
            let value = view.value(key);
            let field = FieldView {
                has: view.has(key),
                complete: view.complete(key),
                length,
                prefix,
                exceeds,
                text,
                value,
            };
            (key.clone(), field)
        })
        .collect();
    let invalid = view.invalid();
    let closed = view.closed();
    ViewSnapshot {
        invalid,
        closed,
        keys: view.keys(),
        fields,
    }
}

fn snapshot_of(view: &mut PartialArguments) -> ViewSnapshot {
    snapshot_with(view, None)
}

fn streamed(chunks: &[&str]) -> PartialArguments {
    let mut view = PartialArguments::new();
    for chunk in chunks {
        view.append(chunk).unwrap();
    }
    view
}

fn streamed_owned(chunks: &[String]) -> PartialArguments {
    let mut view = PartialArguments::new();
    for chunk in chunks {
        view.append(chunk).unwrap();
    }
    view
}

fn append(view: &mut PartialArguments, fragment: &str) {
    view.append(fragment).unwrap();
}

fn append_and_refresh(view: &mut PartialArguments, fragment: &str) -> bool {
    view.append(fragment).unwrap();
    view.refresh()
}

fn assert_sealed_append_fails(view: &mut PartialArguments, fragment: &str) {
    let error = view
        .append(fragment)
        .expect_err("a sealed view rejects appends");
    assert!(error.to_string().contains("cannot append to a sealed view"));
}

fn u(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

fn from_parsed(text: &str) -> PartialArguments {
    PartialArguments::from_object(&serde_json::from_str::<Value>(text).unwrap())
}

fn s(text: &str) -> Option<String> {
    Some(text.to_owned())
}

// ---- sealed views ----

#[test]
fn reports_sealing_independently_of_object_closure_and_preserves_append_rejection() {
    let mut view = streamed(&[r#"{"a":"value"}"#]);
    assert!(view.closed());
    assert!(!view.is_sealed());
    append(&mut view, " ");
    assert!(view.settle(r#"{"a":"value"} "#).is_none());
    assert!(view.is_sealed());
    assert_sealed_append_fails(&mut view, "");
    assert!(PartialArguments::empty().is_sealed());
    assert!(PartialArguments::from_text(r#"{"a":"unfinished"#).is_sealed());
    let mut partial = streamed(&[r#"{"a":"old"#]);
    let settled = partial.settle(r#"{"a":"new"}"#).expect("a new view");
    assert!(settled.is_sealed());
    assert!(!partial.is_sealed());
}

#[test]
fn reads_sealing_without_scanning_or_observing_streamed_text() {
    // `is_sealed` takes `&self`, so it cannot scan or register a read.
    let mut view = streamed(&[r#"{"a":"value"#]);
    assert!(!view.is_sealed());
    append(&mut view, r#" grows"}"#);
    assert!(!view.refresh());
}

#[test]
fn empty_answers_every_reader_with_absence_and_refuses_appends() {
    let mut empty = PartialArguments::empty();
    assert!(empty.keys().is_empty());
    assert!(!empty.has("x"));
    assert!(!empty.complete("x"));
    assert_eq!(len(&mut empty, "x"), None);
    assert_eq!(empty.text("x"), None);
    assert_eq!(empty.value("x"), None);
    assert!(!empty.invalid());
    assert_sealed_append_fails(&mut empty, "{");
}

#[test]
fn reports_every_sealed_view_as_closed_whatever_its_text_says() {
    assert!(PartialArguments::empty().closed());
    assert!(PartialArguments::from_object(&json!({"a": 1})).closed());
    assert!(PartialArguments::from_object(&Value::Null).closed());
    let mut texts = all_texts();
    texts.extend(["", r#"{"a":1"#]);
    for text in texts {
        assert!(PartialArguments::from_text(text).closed(), "{text:?}");
    }
}

#[test]
fn never_answers_with_inherited_object_prototype_members() {
    for mut view in [
        PartialArguments::empty(),
        PartialArguments::from_object(&json!({"a": 1})),
    ] {
        for key in ["constructor", "__proto__", "toString", "hasOwnProperty"] {
            assert!(!view.has(key));
            assert!(!view.complete(key));
            assert_eq!(view.value(key), None);
            assert_eq!(view.text(key), None);
            assert_eq!(len(&mut view, key), None);
        }
    }
}

#[test]
fn from_object_reads_strings_and_other_values_straight_from_the_parsed_object() {
    let mut view = PartialArguments::from_object(
        &json!({"file_path": "src/a.ts", "empty": "", "n": 12.5, "z": null, "o": {"k": [1]}, "a": [1, "x"]}),
    );
    assert_eq!(view.keys(), ["file_path", "empty", "n", "z", "o", "a"]);
    assert_eq!(len(&mut view, "file_path"), Some(8));
    assert_eq!(view.text("file_path"), s("src/a.ts"));
    assert_eq!(view.value("file_path"), None);
    assert_eq!(len(&mut view, "empty"), Some(0));
    assert_eq!(view.text("empty"), s(""));
    assert_eq!(view.value("n"), Some(json!(12.5)));
    assert_eq!(view.value("z"), Some(Value::Null));
    assert_eq!(view.value("o"), Some(json!({"k": [1]})));
    assert_eq!(view.value("a"), Some(json!([1, "x"])));
    assert_eq!(len(&mut view, "n"), None);
    assert_eq!(view.text("n"), None);
    for key in view.keys() {
        assert!(view.has(&key));
        assert!(view.complete(&key));
    }
    assert!(!view.has("missing"));
    assert!(!view.complete("missing"));
    assert_eq!(view.value("missing"), None);
    assert!(!view.invalid());
    assert_sealed_append_fails(&mut view, "{");
}

#[test]
fn from_object_treats_anything_but_an_object_as_a_call_without_fields() {
    // The source also passes `undefined`; a Rust caller has no such value.
    for input in [
        Value::Null,
        json!("str"),
        json!(42),
        json!(true),
        json!([1, 2]),
    ] {
        let mut view = PartialArguments::from_object(&input);
        assert!(view.keys().is_empty());
        assert!(!view.has("0"));
        assert_eq!(view.value("0"), None);
    }
}

#[test]
fn from_text_answers_exactly_like_from_object_over_the_parsed_text_and_refuses_appends() {
    for text in CLOSED_TEXTS {
        let mut from_text = PartialArguments::from_text(text);
        assert_eq!(
            snapshot_of(&mut from_text),
            snapshot_of(&mut from_parsed(text)),
            "{text:?}"
        );
        assert_sealed_append_fails(&mut from_text, " ");
    }
}

#[test]
fn from_text_reports_invalid_contents_when_read_and_retains_independent_fields() {
    for text in INVALID_TEXTS {
        assert!(
            snapshot_of(&mut PartialArguments::from_text(text)).invalid,
            "{text:?}"
        );
    }
    let mut view = PartialArguments::from_text(r#"{"n":1,"b":tru}"#);
    assert!(!view.invalid());
    assert_eq!(view.value("n"), Some(json!(1)));
    assert!(view.has("b"));
    assert!(view.complete("b"));
    assert_eq!(view.value("b"), None);
    assert!(view.invalid());
    assert!(!view.complete("b"));
}

#[test]
fn scans_nothing_until_a_reader_asks() {
    let mut sealed = PartialArguments::from_text(r#"{"todos":[1],"n":2}"#);
    let mut streaming = PartialArguments::new();
    assert!(!append_and_refresh(&mut streaming, r#"{"todos":[1],"#));
    assert!(!append_and_refresh(&mut streaming, r#""n":2}"#));
    assert_eq!(sealed.json_parse_count() + streaming.json_parse_count(), 0);

    assert_eq!(sealed.keys(), ["todos", "n"]);
    assert!(sealed.has("todos"));
    assert!(sealed.complete("todos"));
    assert!(!sealed.invalid());
    assert_eq!(streaming.keys(), ["todos", "n"]);
    assert!(streaming.has("todos"));
    assert!(streaming.complete("todos"));
    assert_eq!(sealed.json_parse_count() + streaming.json_parse_count(), 0);

    assert_eq!(streaming.value("todos"), Some(json!([1])));
    assert_eq!(streaming.json_parse_count(), 1);
    assert_eq!(streaming.value("todos"), Some(json!([1])));
    assert_eq!(streaming.json_parse_count(), 1);
    assert_eq!(streaming.value("n"), Some(json!(2)));
    assert_eq!(streaming.json_parse_count(), 2);
}

// ---- settlement ----

#[test]
fn seals_matching_chunks_without_joining_and_retains_previously_read_values() {
    let mut view = PartialArguments::new();
    append(&mut view, r#"{"command":"a\u4e"#);
    assert_eq!(view.text("command"), s("a"));
    assert_eq!(len(&mut view, "command"), Some(1));
    assert_eq!(view.text_prefix("command", 2), s("a"));
    assert!(!view.complete("command"));
    append(&mut view, r#"2d","todos":[1]}"#);
    let todos = view.value("todos");
    assert_eq!(todos, Some(json!([1])));
    let final_text = r#"{"command":"a\u4e2d","todos":[1]}"#;
    // `settle` compares the fragments against the text in place; there is no join to count.
    assert!(view.settle(final_text).is_none());
    assert!(view == PartialArguments::from_text(final_text));
    assert_eq!(view.text("command"), s("a中"));
    assert_eq!(len(&mut view, "command"), Some(2));
    assert_eq!(view.text_prefix("command", 2), s("a中"));
    assert!(view.complete("command"));
    assert_eq!(view.value("todos"), todos);
    assert!(view.closed());
    assert!(!view.invalid());
    assert!(view.refresh());
    assert!(!view.refresh());
    assert!(view.settle(final_text).is_none());
    assert_sealed_append_fails(&mut view, " ");
}

#[test]
fn leaves_matching_unread_containers_unparsed_until_requested_after_settlement() {
    let mut view = streamed(&[r#"{"items":"#, "[1,2]}"]);
    assert!(view.settle(r#"{"items":[1,2]}"#).is_none());
    assert_eq!(view.keys(), ["items"]);
    assert!(view.has("items"));
    assert!(view.complete("items"));
    assert_eq!(view.json_parse_count(), 0);
    assert_eq!(view.value("items"), Some(json!([1, 2])));
    assert_eq!(view.json_parse_count(), 1);
}

#[test]
fn uses_final_text_after_conflicting_or_missing_or_duplicate_deltas_without_changing_the_streaming_view()
 {
    let cases: [(&str, &[&str], &str, &str, &str); 3] = [
        (
            "same-length conflicting text",
            &["x\"}"],
            r#"{"a":"y"}"#,
            "x",
            "y",
        ),
        ("missing deltas", &["x"], r#"{"a":"xy"}"#, "x", "xy"),
        (
            "duplicate deltas",
            &["x", "x\"}"],
            r#"{"a":"x"}"#,
            "xx",
            "x",
        ),
    ];
    for (label, tail, final_text, before, after) in cases {
        let mut view = PartialArguments::new();
        append(&mut view, r#"{"a":""#);
        assert_eq!(view.text("a"), s(""), "{label}");
        assert_eq!(view.text_prefix("a", 1), s(""), "{label}");
        for chunk in tail {
            append(&mut view, chunk);
        }
        assert_eq!(view.text("a"), s(before), "{label}");
        let mut chunks = vec![r#"{"a":""#];
        chunks.extend(tail.iter().copied());
        let original = streamed(&chunks);
        let mut settled = view.settle(final_text).expect(label);
        assert!(
            settled == PartialArguments::from_text(final_text),
            "{label}"
        );
        assert_eq!(settled.text("a"), s(after), "{label}");
        assert_eq!(len(&mut settled, "a"), Some(after.len()), "{label}");
        assert!(settled.closed(), "{label}");
        assert!(!settled.refresh(), "{label}");
        assert!(view == original, "{label}");
        assert_eq!(view.text("a"), s(before), "{label}");
        assert!(view.refresh(), "{label}");
        assert!(!view.refresh(), "{label}");
        assert!(view.append(" ").is_ok(), "{label}");
        assert_sealed_append_fails(&mut settled, " ");
    }
}

#[test]
fn does_not_carry_an_invalid_reader_cache_into_corrected_authoritative_text() {
    let mut view = streamed(&[r#"{"a":"ab\q"}"#]);
    assert_eq!(view.text("a"), s("ab"));
    assert!(view.invalid());
    let mut settled = view.settle(r#"{"a":"ab\n"}"#).expect("a new view");
    assert_eq!(settled.text("a"), s("ab\n"));
    assert!(settled.complete("a"));
    assert!(!settled.invalid());
    assert_eq!(view.text("a"), s("ab"));
    assert!(view.invalid());
}

#[test]
fn seals_empty_text_and_uses_authoritative_text_for_an_object_backed_view() {
    let mut empty = streamed(&["", ""]);
    assert!(empty.settle("").is_none());
    assert!(empty.closed());
    assert!(empty.keys().is_empty());
    assert!(empty == PartialArguments::from_text(""));
    assert_sealed_append_fails(&mut empty, "{}");
    let mut object = PartialArguments::from_object(&json!({"a": "old"}));
    let mut settled = object.settle(r#"{"a":"new"}"#).expect("a new view");
    assert_eq!(settled.text("a"), s("new"));
    assert_eq!(object.text("a"), s("old"));
}

// ---- structural equality ----

#[test]
fn compares_two_views_by_their_source_alone_however_far_each_has_been_read() {
    for text in all_texts() {
        let mut read = PartialArguments::from_text(text);
        snapshot_of(&mut read);
        let mut fresh = PartialArguments::from_text(text);
        assert!(read == fresh, "{text:?}");
        assert!(fresh == read, "{text:?}");

        let mut eager = PartialArguments::new();
        let keys = fresh.keys();
        snapshot_with(&mut eager, Some(&keys));
        append(&mut eager, text);
        let mut untouched = PartialArguments::new();
        append(&mut untouched, text);
        assert!(eager == untouched, "{text:?}");
    }
    let object = json!({"file_path": "src/a.ts", "n": 1});
    let mut read_object = PartialArguments::from_object(&object);
    snapshot_of(&mut read_object);
    assert!(read_object == PartialArguments::from_object(&object));
}

#[test]
fn distinguishes_a_streaming_view_from_a_sealed_view_of_the_same_text_even_though_every_reader_agrees()
 {
    let text = r#"{"file_path":"src/a.ts","content":"a\nb","n":1,"todos":[{"content":"x"}]}"#;
    let mut sealed = PartialArguments::from_text(text);
    let mut streaming = streamed_owned(&slices(text, seeded_sizes(3)));
    assert!(streaming != sealed);
    let answers = snapshot_of(&mut streaming);
    let expected = ViewSnapshot {
        closed: sealed.closed(),
        ..answers.clone()
    };
    assert_eq!(expected, snapshot_of(&mut sealed));
    assert!(answers.closed);
}

#[test]
fn distinguishes_views_of_different_sources() {
    assert!(PartialArguments::from_text(r#"{"a":1}"#) != PartialArguments::from_text(r#"{"a":2}"#));
    assert!(
        PartialArguments::from_text(r#"{"a":1}"#) != PartialArguments::from_text(r#"{"a":1} "#)
    );
    assert!(
        PartialArguments::from_object(&json!({"a": 1}))
            != PartialArguments::from_object(&json!({"a": 2}))
    );
    assert!(PartialArguments::from_text("{}") != PartialArguments::empty());
}

// ---- streaming readers ----

#[test]
fn distinguishes_an_open_string_a_closed_string_a_pending_value_a_closed_value_and_an_absent_key() {
    let mut view = PartialArguments::new();
    append(&mut view, r#"{"file_path":"ab"#);
    assert_eq!(view.keys(), ["file_path"]);
    assert!(view.has("file_path"));
    assert!(!view.complete("file_path"));
    assert_eq!(len(&mut view, "file_path"), Some(2));
    assert_eq!(view.text("file_path"), s("ab"));
    assert_eq!(view.value("file_path"), None);

    append(&mut view, r#"","content":"xy"#);
    assert!(view.complete("file_path"));
    assert_eq!(view.text("file_path"), s("ab"));
    assert!(!view.complete("content"));
    assert_eq!(len(&mut view, "content"), Some(2));
    assert_eq!(view.text("content"), s("xy"));

    append(&mut view, r#"","n":12"#);
    assert_eq!(view.keys(), ["file_path", "content", "n"]);
    assert!(view.has("n"));
    assert!(!view.complete("n"));
    assert_eq!(view.value("n"), None);
    assert_eq!(len(&mut view, "n"), None);
    assert_eq!(view.text("n"), None);

    append(&mut view, "}");
    assert!(view.complete("n"));
    assert_eq!(view.value("n"), Some(json!(12)));
    assert!(!view.has("missing"));
    assert!(!view.complete("missing"));
    assert_eq!(len(&mut view, "missing"), None);
    assert_eq!(view.text("missing"), None);
    assert_eq!(view.value("missing"), None);
    assert!(!view.invalid());
}

#[test]
fn reports_every_top_level_field_of_a_finished_object() {
    let mut view = streamed(&[
        r#"{"file_path":"src/a.ts","content":"hello","n":1,"o":{"file_path":"inner"},"a":[1,2]}"#,
    ]);
    assert_eq!(view.keys(), ["file_path", "content", "n", "o", "a"]);
    assert_eq!(view.text("file_path"), s("src/a.ts"));
    assert_eq!(len(&mut view, "file_path"), Some(8));
    assert_eq!(len(&mut view, "content"), Some(5));
    assert_eq!(view.value("n"), Some(json!(1)));
    assert_eq!(view.value("o"), Some(json!({"file_path": "inner"})));
    assert_eq!(view.value("a"), Some(json!([1, 2])));
    assert_eq!(view.text("o"), None);
    assert_eq!(view.value("file_path"), None);
    for key in view.keys() {
        assert!(view.complete(&key));
    }
    assert!(!view.invalid());
}

#[test]
fn treats_proto_and_object_prototype_member_names_as_ordinary_keys() {
    let mut flat = streamed(&[r#"{"__proto__":"x","constructor":"c","a":1}"#]);
    assert_eq!(flat.keys(), ["__proto__", "constructor", "a"]);
    assert!(flat.has("__proto__"));
    assert_eq!(flat.text("__proto__"), s("x"));
    assert_eq!(flat.text("constructor"), s("c"));
    assert_eq!(flat.value("a"), Some(json!(1)));

    let mut nested = streamed(&[r#"{"__proto__":{"polluted":true}}"#]);
    assert_eq!(nested.keys(), ["__proto__"]);
    assert_eq!(nested.value("__proto__"), Some(json!({"polluted": true})));
    let mut parsed = from_parsed(r#"{"__proto__":{"polluted":true}}"#);
    assert_eq!(parsed.keys(), ["__proto__"]);
    assert_eq!(parsed.value("__proto__"), Some(json!({"polluted": true})));
}

#[test]
fn reports_numbers_and_literals_once_a_comma_whitespace_or_the_closing_brace_ends_them() {
    let mut by_comma = streamed(&[r#"{"n":12.5,"b":true,"z":null,"file_path":"p"}"#]);
    assert_eq!(by_comma.value("n"), Some(json!(12.5)));
    assert_eq!(by_comma.value("b"), Some(json!(true)));
    assert_eq!(by_comma.value("z"), Some(Value::Null));
    assert!(by_comma.has("z"));
    assert!(by_comma.complete("z"));
    assert_eq!(by_comma.text("file_path"), s("p"));

    let mut by_brace = streamed(&[r#"{"file_path":"p","n":-1e3}"#]);
    assert_eq!(by_brace.value("n"), Some(json!(-1000)));
    assert!(!by_brace.invalid());

    let mut by_whitespace = streamed(&["{\"n\":1 ,\"file_path\":\"p\",\"m\":false\n}"]);
    assert_eq!(by_whitespace.value("n"), Some(json!(1)));
    assert_eq!(by_whitespace.value("m"), Some(json!(false)));
    assert!(!by_whitespace.invalid());
    assert_eq!(streamed(&[r#"{"n":1 }"#]).value("n"), Some(json!(1)));
}

#[test]
fn holds_a_number_or_literal_open_until_its_delimiter_arrives() {
    let mut view = PartialArguments::new();
    append(&mut view, r#"{"n":12"#);
    assert!(view.has("n"));
    assert!(!view.complete("n"));
    assert_eq!(view.value("n"), None);
    append(&mut view, ".5");
    assert_eq!(view.value("n"), None);
    append(&mut view, ",");
    assert!(view.complete("n"));
    assert_eq!(view.value("n"), Some(json!(12.5)));
    append(&mut view, r#""t":tru"#);
    assert_eq!(view.value("t"), None);
    append(&mut view, "e}");
    assert_eq!(view.value("t"), Some(json!(true)));
    assert!(!view.invalid());
}

#[test]
fn parses_a_nested_value_when_its_matching_bracket_closes_ignoring_brackets_and_escapes_inside_nested_strings()
 {
    let todos =
        json!([{"content": "x}"}, {"content": "]\"\\"}, [[]], {"deep": {"a": [1, {"b": "}"}]}}]);
    let mut view = PartialArguments::new();
    append(
        &mut view,
        r#"{"todos":[{"content":"x}"},{"content":"]\"\\"},[[]],{"deep":{"a":[1,{"b":"}"}]}}"#,
    );
    assert!(view.has("todos"));
    assert!(!view.complete("todos"));
    assert_eq!(view.value("todos"), None);
    assert!(!view.has("content"));
    append(&mut view, "]");
    assert!(view.complete("todos"));
    assert_eq!(view.value("todos"), Some(todos));
    append(&mut view, r#","file_path":"y"}"#);
    assert_eq!(view.keys(), ["todos", "file_path"]);
    assert_eq!(view.text("file_path"), s("y"));
    assert!(!view.invalid());
}

#[test]
fn reports_a_string_whose_quotes_backslashes_and_braces_are_escaped_by_decoded_length_and_text() {
    let mut view = streamed(&[r#"{"skip":"a\"}{b\\","file_path":"x"}"#]);
    assert_eq!(len(&mut view, "skip"), Some(6));
    assert_eq!(view.text("skip"), s("a\"}{b\\"));
    assert_eq!(view.text("file_path"), s("x"));
    assert!(!view.invalid());
}

// ---- escapes ----

#[test]
fn keeps_a_split_escape_inside_its_string_field() {
    let cases = [
        (
            "odd backslash run",
            r#"{"command":"a\\"#,
            r#"\"b","later":"ok"}"#,
            "a\\\"b",
        ),
        (
            "even backslash run",
            r#"{"command":"a\\"#,
            r#"\\","later":"ok"}"#,
            "a\\\\",
        ),
        (
            "Unicode quote",
            r#"{"command":"a\u00"#,
            "22b\",\"later\":\"ok\"}",
            "a\"b",
        ),
    ];
    for (label, head, tail, expected) in cases {
        let mut view = PartialArguments::new();
        append(&mut view, head);
        assert_eq!(view.keys(), ["command"], "{label}");
        assert!(!view.complete("command"), "{label}");
        assert!(!view.has("later"), "{label}");
        view.text("command");
        len(&mut view, "command");
        append(&mut view, tail);
        assert_eq!(view.keys(), ["command", "later"], "{label}");
        assert!(view.complete("command"), "{label}");
        assert_eq!(view.text("command"), s(expected), "{label}");
        assert_eq!(len(&mut view, "command"), Some(expected.len()), "{label}");
        assert_eq!(view.text("later"), s("ok"), "{label}");
        assert!(!view.invalid(), "{label}");
    }
}

#[test]
fn keeps_escaped_nested_quotes_inside_an_unread_container_across_one_character_chunks() {
    let text = r#"{"items":["a\\\"b","a\\\\","\u0022"],"later":"ok"}"#;
    let mut view = PartialArguments::new();
    for chunk in text.chars() {
        append(&mut view, &chunk.to_string());
        view.keys();
    }
    assert_eq!(view.keys(), ["items", "later"]);
    assert!(view.complete("items"));
    assert_eq!(view.text("later"), s("ok"));
    assert_eq!(view.value("items"), Some(json!(["a\\\"b", "a\\\\", "\""])));
    assert!(!view.invalid());
}

#[test]
fn decodes_every_simple_escape() {
    let mut view = streamed(&[r#"{"file_path":"\"\\\/\b\f\n\r\t"}"#]);
    assert_eq!(len(&mut view, "file_path"), Some(8));
    assert_eq!(view.text("file_path"), s("\"\\/\u{8}\u{c}\n\r\t"));
}

#[test]
fn decodes_u_escapes_in_either_hex_case() {
    let mut view = streamed(&[r#"{"file_path":"\u4E2D\u00e9"}"#]);
    assert_eq!(len(&mut view, "file_path"), Some(2));
    assert_eq!(view.text("file_path"), s("中é"));
}

#[test]
fn decodes_a_u_escape_split_across_appends_whether_the_text_was_read_before_or_only_after() {
    let chunkings: [&[&str]; 2] = [
        &[r#"{"file_path":"\u4e"#, "2d\"}"],
        &["{\"file_path\":\"\\", "u", "4", "e2", "d\"}"],
    ];
    for chunks in chunkings {
        let mut lazy = streamed(chunks);
        assert_eq!(lazy.text("file_path"), s("中"));
        assert_eq!(len(&mut lazy, "file_path"), Some(1));

        let mut eager = PartialArguments::new();
        assert_eq!(eager.text("file_path"), None);
        for chunk in chunks {
            append_and_refresh(&mut eager, chunk);
        }
        assert_eq!(eager.text("file_path"), s("中"));
        assert_eq!(len(&mut eager, "file_path"), Some(1));
    }
}

#[test]
fn decodes_a_surrogate_pair_of_two_u_escapes_into_one_code_point_counted_as_two_utf16_units() {
    let mut view = PartialArguments::new();
    append(&mut view, r#"{"file_path":"\ud83d"#);
    assert_eq!(view.text_utf16("file_path"), Some(vec![0xd83d]));
    assert_eq!(len(&mut view, "file_path"), Some(1));
    append(&mut view, r#"\ude00"}"#);
    assert_eq!(view.text("file_path"), s("😀"));
    assert_eq!(len(&mut view, "file_path"), Some(2));
    let mut other = PartialArguments::from_text(r#"{"content":"\u00e9\ud83d\ude00"}"#);
    assert_eq!(len(&mut other, "content"), Some(3));
}

// ---- lazy text reads ----

#[test]
fn decodes_the_streamed_prefix_on_first_read_including_complete_escapes_then_keeps_it_current() {
    let mut view = PartialArguments::new();
    append(&mut view, r#"{"file_path":"a\n\u4e2d\"x"#);
    assert_eq!(view.text("file_path"), s("a\n中\"x"));
    assert_eq!(len(&mut view, "file_path"), Some(5));
    append(&mut view, "y\"}");
    assert_eq!(view.text("file_path"), s("a\n中\"xy"));
    assert!(view.complete("file_path"));
}

#[test]
fn drops_an_escape_still_in_flight_from_the_first_read_and_completes_it_later() {
    let cases = [
        ("{\"file_path\":\"a\\", "n\"}", "a\n"),
        ("{\"file_path\":\"a\\u", "4e2d\"}", "a中"),
        (r#"{"file_path":"a\u4e"#, "2d\"}", "a中"),
    ];
    for (head, tail, decoded) in cases {
        let mut view = PartialArguments::new();
        append(&mut view, head);
        assert_eq!(view.text("file_path"), s("a"), "{head:?}");
        assert_eq!(len(&mut view, "file_path"), Some(1), "{head:?}");
        append(&mut view, tail);
        assert_eq!(view.text("file_path"), s(decoded), "{head:?}");
        assert_eq!(
            len(&mut view, "file_path"),
            Some(u(decoded).len()),
            "{head:?}"
        );
        assert!(view.complete("file_path"), "{head:?}");
    }
}

#[test]
fn decodes_a_closed_string_in_one_step_on_first_read() {
    let mut view = PartialArguments::from_text(r#"{"file_path":"a\n\u4e2d\ud83d\ude00"}"#);
    assert_eq!(view.text("file_path"), s("a\n中😀"));
    assert_eq!(view.text("file_path"), s("a\n中😀"));
    assert_eq!(len(&mut view, "file_path"), Some(5));
}

#[test]
fn returns_the_prefix_before_an_invalid_simple_escape_whether_the_text_is_first_read_before_at_or_after_the_failure()
 {
    let text = r#"{"file_path":"ab\q"}"#;
    let mut lazy = PartialArguments::from_text(text);
    assert_eq!(lazy.text("file_path"), s("ab"));
    assert_eq!(len(&mut lazy, "file_path"), Some(2));
    assert!(lazy.invalid());
    assert!(!lazy.complete("file_path"));

    let mut read_before_key = PartialArguments::new();
    assert_eq!(read_before_key.text("file_path"), None);
    assert!(append_and_refresh(&mut read_before_key, text));
    assert_eq!(read_before_key.text("file_path"), s("ab"));
    assert_eq!(len(&mut read_before_key, "file_path"), Some(2));

    let mut read_mid_string = PartialArguments::new();
    append(&mut read_mid_string, r#"{"file_path":"a"#);
    assert_eq!(read_mid_string.text("file_path"), s("a"));
    assert!(append_and_refresh(&mut read_mid_string, r#"b\q"}"#));
    assert_eq!(read_mid_string.text("file_path"), s("ab"));
    assert_eq!(len(&mut read_mid_string, "file_path"), Some(2));
    assert!(read_mid_string.invalid());
}

#[test]
fn returns_the_prefix_before_an_invalid_unicode_escape_matching_a_read_made_before_the_failure() {
    let mut lazy = PartialArguments::from_text(r#"{"file_path":"ab\u12G4"}"#);
    assert!(!lazy.invalid());
    assert_eq!(lazy.text("file_path"), s("ab"));
    assert!(lazy.invalid());
    assert_eq!(len(&mut lazy, "file_path"), Some(2));
    assert!(!lazy.complete("file_path"));

    let mut eager = PartialArguments::new();
    assert_eq!(eager.text("file_path"), None);
    append(&mut eager, r#"{"file_path":"ab\u12G4"}"#);
    assert!(!eager.invalid());
    assert_eq!(eager.text("file_path"), s("ab"));
    assert!(eager.invalid());
    assert_eq!(len(&mut eager, "file_path"), Some(2));
}

// ---- keys and whitespace ----

#[test]
fn matches_an_escaped_key_against_its_decoded_name() {
    let mut view = streamed(&[r#"{"file\u005fpath":"x","a\"b":"v"}"#]);
    assert_eq!(view.keys(), ["file_path", "a\"b"]);
    assert_eq!(view.text("file_path"), s("x"));
    assert_eq!(view.text("a\"b"), s("v"));
}

#[test]
fn accepts_json_whitespace_around_every_structural_token() {
    let mut view = streamed(&[
        " \n{\t\"file_path\"\r:\n\"x\"\t,\r\"content\" : \"y\" ,\n\"n\" : 1 ,\t\"o\" : { \"k\" : [ 1 ] } \n}\r\n",
    ]);
    assert_eq!(view.keys(), ["file_path", "content", "n", "o"]);
    assert_eq!(view.text("file_path"), s("x"));
    assert_eq!(view.text("content"), s("y"));
    assert_eq!(view.value("n"), Some(json!(1)));
    assert_eq!(view.value("o"), Some(json!({"k": [1]})));
    assert!(!view.invalid());
}

#[test]
fn accepts_an_empty_object_and_trailing_whitespace_after_the_close_but_nothing_else_after_it() {
    for text in ["{}", "{ \n}", "{\"a\":1} \n\t\r", ""] {
        let mut view = PartialArguments::from_text(text);
        assert!(!view.invalid(), "{text:?}");
        assert!(!view.has("b"), "{text:?}");
    }
    assert!(PartialArguments::from_text("{} x").invalid());
    assert!(PartialArguments::from_text(r#"{"file_path":"x"}}"#).invalid());
    assert_eq!(
        PartialArguments::from_text(r#"{"file_path":"x"}}"#).text("file_path"),
        s("x")
    );
}

#[test]
fn reports_a_streaming_object_closed_when_its_outer_closing_brace_arrives() {
    let mut view = PartialArguments::new();
    assert!(!view.closed());
    append(&mut view, r#"{"a":1,"b":"x"#);
    assert!(!view.closed());
    append(&mut view, "\"}");
    assert!(view.closed());
    append(&mut view, " \n");
    assert!(view.closed());
    assert!(!view.invalid());
    assert!(streamed(&["{}"]).closed());

    let mut broken = PartialArguments::new();
    append(&mut broken, r#"{"a":tru"#);
    assert!(!broken.closed());
    append(&mut broken, "}");
    assert!(broken.closed());
    assert!(!broken.invalid());
    assert_eq!(broken.value("a"), None);
    assert!(broken.invalid());
}

#[test]
fn lets_a_repeated_key_replace_the_earlier_entry_without_repeating_the_key() {
    let mut view = PartialArguments::new();
    append(&mut view, r#"{"file_path":"a","#);
    assert_eq!(view.text("file_path"), s("a"));
    append(&mut view, r#""file_path":""#);
    assert_eq!(view.keys(), ["file_path"]);
    assert_eq!(view.text("file_path"), s(""));
    assert!(!view.complete("file_path"));
    append(&mut view, r#"bc","#);
    assert_eq!(view.text("file_path"), s("bc"));

    append(&mut view, r#""file_path":[1],"#);
    assert_eq!(view.value("file_path"), Some(json!([1])));
    assert_eq!(view.text("file_path"), None);
    assert_eq!(len(&mut view, "file_path"), None);
    append(&mut view, r#""file_path":"d"}"#);
    assert_eq!(view.text("file_path"), s("d"));
    assert_eq!(view.value("file_path"), None);
    assert_eq!(view.keys(), ["file_path"]);
    assert!(!view.invalid());
}

// ---- fragmentation ----

#[test]
fn answers_the_same_whether_scanned_eagerly_per_character_per_seeded_slice_or_lazily_as_a_whole() {
    for text in all_texts() {
        let expected = snapshot_of(&mut PartialArguments::from_text(text));
        let mut chunkings: Vec<Vec<String>> = vec![text.chars().map(String::from).collect()];
        for seed in 1..=8 {
            chunkings.push(slices(text, seeded_sizes(seed)));
        }
        for chunks in chunkings {
            let mut eager = PartialArguments::new();
            snapshot_with(&mut eager, Some(&expected.keys));
            for chunk in &chunks {
                append_and_refresh(&mut eager, chunk);
            }
            assert_eq!(
                snapshot_with(&mut eager, Some(&expected.keys)),
                expected,
                "{text:?} {chunks:?}"
            );
            assert_eq!(
                snapshot_of(&mut streamed_owned(&chunks)),
                expected,
                "{text:?} {chunks:?}"
            );
        }
    }
}

#[test]
fn keeps_text_and_length_consistent_when_the_first_read_lands_at_any_character_boundary() {
    for text in all_texts() {
        let expected = snapshot_of(&mut PartialArguments::from_text(text));
        for cut in 0..=text.len() {
            let mut view = PartialArguments::new();
            append(&mut view, &text[..cut]);
            let midway = snapshot_with(&mut view, Some(&expected.keys));
            for (_, field) in &midway.fields {
                if let Some(field_text) = &field.text {
                    assert_eq!(Some(field_text.len()), field.length, "{text:?} cut {cut}");
                    let head: Vec<u16> = field_text.iter().take(4).copied().collect();
                    assert_eq!(field.prefix.as_ref(), Some(&head), "{text:?} cut {cut}");
                    assert_eq!(field.exceeds, field_text.len() > 4, "{text:?} cut {cut}");
                }
            }
            append(&mut view, &text[cut..]);
            assert_eq!(
                snapshot_with(&mut view, Some(&expected.keys)),
                expected,
                "{text:?} cut {cut}"
            );
        }
    }
}

// ---- change detection ----

#[test]
fn publishes_a_completion_error_discovered_by_a_later_content_read_during_sealed_refresh() {
    let mut view = streamed(&[r#"{"a":"ab""#]);
    assert!(view.complete("a"));
    assert_eq!(view.text("a"), s("ab"));
    append(&mut view, r#","a":"ab\q"}"#);
    view.settle(r#"{"a":"ab","a":"ab\q"}"#);
    assert!(view.refresh());
    assert!(!view.complete("a"));
    assert_eq!(view.text("a"), s("ab"));
    assert!(!view.refresh());
}

#[test]
fn reports_no_change_while_nothing_has_been_read_then_answers_fully_on_the_first_read() {
    let mut view = PartialArguments::new();
    assert!(!append_and_refresh(
        &mut view,
        r#"{"file_path":"src/a.ts","n":1,"#
    ));
    assert!(!append_and_refresh(&mut view, r#""todos":[1]}"#));
    assert_eq!(view.keys(), ["file_path", "n", "todos"]);
    assert_eq!(view.text("file_path"), s("src/a.ts"));
    assert_eq!(view.value("todos"), Some(json!([1])));
}

#[test]
fn reports_a_length_read_only_when_its_rounded_up_step_moves() {
    let mut view = PartialArguments::new();
    append(&mut view, r#"{"content":""#);
    assert_eq!(len_step(&mut view, "content", 1024.0), Some(0));
    assert!(append_and_refresh(&mut view, "a"));
    assert!(!append_and_refresh(&mut view, &"a".repeat(1022)));
    assert!(!append_and_refresh(&mut view, "a"));
    assert_eq!(len_step(&mut view, "content", 1024.0), Some(1024));
    assert!(append_and_refresh(&mut view, "a"));
    assert!(!append_and_refresh(&mut view, &"a".repeat(1023)));
    assert!(append_and_refresh(&mut view, "a"));
    assert_eq!(len_step(&mut view, "content", 1024.0), Some(2049));
    assert!(!append_and_refresh(&mut view, "\"}"));
}

#[test]
fn treats_a_missing_zero_or_fractional_step_as_its_floor_of_at_least_one() {
    let mut per_character = PartialArguments::new();
    append(&mut per_character, r#"{"c":""#);
    len(&mut per_character, "c");
    per_character.string_length(
        "c",
        LengthReadOptions {
            step: None,
            offset: None,
        },
    );
    len_step(&mut per_character, "c", 0.0);
    for c in ["a", "b", "c"] {
        assert!(append_and_refresh(&mut per_character, c));
    }

    let mut by_two = PartialArguments::new();
    append(&mut by_two, r#"{"c":""#);
    len_step(&mut by_two, "c", 2.9);
    assert!(append_and_refresh(&mut by_two, "a"));
    assert!(!append_and_refresh(&mut by_two, "b"));
    assert!(append_and_refresh(&mut by_two, "c"));
    assert!(!append_and_refresh(&mut by_two, "d"));
}

#[test]
fn reports_a_text_read_on_every_decoded_character_and_nothing_else() {
    let mut view = PartialArguments::new();
    append(&mut view, r#"{"description":""#);
    assert_eq!(view.text("description"), s(""));
    for c in ["h", "e", "l", "l", "o"] {
        assert!(append_and_refresh(&mut view, c));
    }
    assert!(!append_and_refresh(&mut view, "\\"));
    assert!(append_and_refresh(&mut view, "n"));
    assert!(!append_and_refresh(&mut view, "\""));
    assert!(!append_and_refresh(&mut view, r#","n":1}"#));
    assert_eq!(view.text("description"), s("hello\n"));
}

#[test]
fn stops_reporting_a_bounded_text_prefix_after_its_decoded_limit() {
    let mut view = PartialArguments::new();
    append(&mut view, r#"{"command":""#);
    assert_eq!(view.text_prefix("command", 4), s(""));
    assert!(append_and_refresh(&mut view, "ab"));
    assert!(append_and_refresh(&mut view, "cd"));
    assert_eq!(view.text_prefix("command", 4), s("abcd"));
    assert!(!append_and_refresh(&mut view, "ef"));
    assert_eq!(view.text_prefix("command", 4), s("abcd"));
    assert!(view.string_exceeds("command", 4));
    assert_eq!(view.text("command"), s("abcdef"));
}

#[test]
fn evaluates_deferred_fragments_together_at_the_publication_point() {
    let mut view = PartialArguments::new();
    append(&mut view, r#"{"content":""#);
    assert_eq!(len_step(&mut view, "content", 4.0), Some(0));
    append(&mut view, "ab");
    append(&mut view, "cd");
    assert!(view.refresh());
    assert_eq!(len_step(&mut view, "content", 4.0), Some(4));
    assert!(!view.refresh());
    append(&mut view, "ef");
    assert!(view.refresh());
    append(&mut view, "gh");
    assert!(!view.refresh());
    assert_eq!(len_step(&mut view, "content", 4.0), Some(8));
}

#[test]
fn bounds_decoded_prefixes_across_json_escapes_and_parsed_objects() {
    assert_eq!(
        PartialArguments::from_text(r#"{"command":"\u4e2d tail"}"#).text_prefix("command", 1),
        s("中")
    );
    assert_eq!(PartialArguments::empty().text_prefix("missing", 4), None);
    assert_eq!(
        PartialArguments::from_text(r#"{"command":"a\n\u4e2d😀z"}"#)
            .text_prefix_utf16("command", 4),
        Some(vec![u16::from(b'a'), 0x0a, 0x4e2d, 0xd83d])
    );
    let object = json!({"command": "abcdef"});
    assert_eq!(
        PartialArguments::from_object(&object).text_prefix("command", 3),
        s("abc")
    );
    assert!(PartialArguments::from_object(&object).string_exceeds("command", 3));
    assert!(!PartialArguments::from_object(&object).string_exceeds("command", 6));
    let number = json!({"command": 1});
    assert_eq!(
        PartialArguments::from_object(&number).text_prefix("command", 3),
        None
    );
    assert!(!PartialArguments::from_object(&number).string_exceeds("command", 3));
}

#[test]
fn tracks_bounded_reads_through_split_unicode_escapes_and_repeated_fields() {
    let mut view = PartialArguments::new();
    fn observe(view: &mut PartialArguments) -> (Option<Vec<u16>>, bool) {
        (
            view.text_prefix_utf16("command", 1),
            view.string_exceeds("command", 1),
        )
    }
    assert_eq!(observe(&mut view), (None, false));
    let rows: [(&str, Option<Vec<u16>>, bool, bool); 10] = [
        (r#"{"command":"\ud83"#, Some(vec![]), false, true),
        ("d", Some(vec![0xd83d]), false, true),
        (r#"\ude"#, Some(vec![0xd83d]), false, false),
        ("00", Some(vec![0xd83d]), true, true),
        ("x\"", Some(vec![0xd83d]), true, false),
        (r#","command":""#, Some(vec![]), false, true),
        (r#"\u4e"#, Some(vec![]), false, false),
        ("2d", Some(vec![0x4e2d]), false, true),
        ("x\"", Some(vec![0x4e2d]), true, true),
        (r#","command":0}"#, None, false, true),
    ];
    for (fragment, prefix, exceeds, changed) in rows {
        append(&mut view, fragment);
        assert_eq!(observe(&mut view), (prefix, exceeds), "{fragment:?}");
        assert_eq!(view.refresh(), changed, "{fragment:?}");
        assert!(!view.refresh(), "{fragment:?}");
    }
}

#[test]
fn reports_a_has_read_when_the_field_begins_at_a_string_quote_or_the_first_character_of_another_value()
 {
    let mut view = PartialArguments::new();
    assert!(!view.has("x"));
    assert!(!view.has("y"));
    assert!(!append_and_refresh(&mut view, r#"{"a":1,"x""#));
    assert!(!append_and_refresh(&mut view, ":"));
    assert!(append_and_refresh(&mut view, "\""));
    assert!(!append_and_refresh(&mut view, r#"v","y":"#));
    assert!(append_and_refresh(&mut view, "1"));
    assert!(!append_and_refresh(&mut view, "}"));
}

#[test]
fn reports_a_complete_read_when_the_string_closes_or_the_value_lands() {
    let mut view = PartialArguments::new();
    append(&mut view, r#"{"file_path":"a"#);
    assert!(!view.complete("file_path"));
    assert!(!view.complete("todos"));
    assert!(!append_and_refresh(&mut view, "b"));
    assert!(append_and_refresh(&mut view, "\""));
    assert!(!append_and_refresh(&mut view, r#","todos":[1,"#));
    assert!(append_and_refresh(&mut view, "2]"));
    assert!(!append_and_refresh(&mut view, "}"));
}

#[test]
fn reports_a_value_read_only_when_the_value_lands_not_while_its_nested_text_streams() {
    let mut view = PartialArguments::new();
    append(&mut view, r#"{"todos":"#);
    assert_eq!(view.value("todos"), None);
    assert!(!append_and_refresh(&mut view, r#"[{"content":"x""#));
    assert!(!append_and_refresh(&mut view, "}"));
    assert!(append_and_refresh(&mut view, "]"));
    assert_eq!(view.value("todos"), Some(json!([{"content": "x"}])));
    assert!(!append_and_refresh(&mut view, "}"));
}

#[test]
fn reports_empty_keys_and_new_keys_not_repeated_keys() {
    let mut view = PartialArguments::new();
    assert!(view.keys().is_empty());
    assert!(append_and_refresh(&mut view, r#"{"":"x""#));
    assert!(!append_and_refresh(&mut view, r#","":"y""#));
    assert!(!append_and_refresh(&mut view, r#","b""#));
    assert!(append_and_refresh(&mut view, ":1"));
    assert_eq!(view.keys(), ["", "b"]);
}

#[test]
fn reports_a_closed_read_when_the_outer_object_closes_or_indexing_fails() {
    let mut view = PartialArguments::new();
    assert!(!view.closed());
    assert!(!append_and_refresh(&mut view, r#"{"a":1,"b":"x""#));
    assert!(append_and_refresh(&mut view, "}"));
    assert!(!append_and_refresh(&mut view, " "));

    let mut broken = PartialArguments::new();
    assert!(!broken.closed());
    assert!(!append_and_refresh(&mut broken, r#"{"a":"#));
    assert!(append_and_refresh(&mut broken, "]"));
    assert!(broken.closed());
    assert!(broken.invalid());
    assert!(!append_and_refresh(&mut broken, "}"));
}

#[test]
fn keeps_an_observed_text_change_pending_until_refresh() {
    let mut view = PartialArguments::new();
    append(&mut view, r#"{"a":""#);
    assert_eq!(view.text("a"), s(""));
    append(&mut view, "xy");
    assert_eq!(view.text("a"), s("xy"));
    assert!(view.refresh());
    assert!(!view.refresh());
    append(&mut view, "z");
    assert_eq!(view.text("a"), s("xyz"));
    assert!(view.refresh());
    assert!(!view.refresh());
}

#[test]
fn keeps_changes_pending_while_several_readers_inspect_the_same_field() {
    let mut view = PartialArguments::new();
    append(&mut view, r#"{"command":"ab"#);
    fn observe(view: &mut PartialArguments) -> (Option<String>, Option<String>, bool) {
        (
            view.text("command"),
            view.text_prefix("command", 3),
            view.string_exceeds("command", 3),
        )
    }
    assert_eq!(observe(&mut view), (s("ab"), s("ab"), false));
    append(&mut view, "cd");
    assert_eq!(observe(&mut view), (s("abcd"), s("abc"), true));
    assert_eq!(len_step(&mut view, "command", 4.0), Some(4));
    assert!(view.refresh());
    assert!(!view.refresh());
    append(&mut view, "e");
    assert_eq!(observe(&mut view), (s("abcde"), s("abc"), true));
    assert_eq!(len_step(&mut view, "command", 4.0), Some(5));
    assert!(view.refresh());
    assert!(!view.refresh());
}

#[test]
fn keeps_length_observations_independent_by_step_and_offset_until_refresh() {
    let mut view = PartialArguments::new();
    append(&mut view, r#"{"content":"a"#);
    fn observe(view: &mut PartialArguments) -> [Option<usize>; 3] {
        [
            len_step(view, "content", 4.0),
            view.string_length(
                "content",
                LengthReadOptions {
                    step: Some(4.0),
                    offset: Some(2.0),
                },
            ),
            len_step(view, "content", 8.0),
        ]
    }
    assert_eq!(observe(&mut view), [Some(1), Some(1), Some(1)]);
    let rows = [
        ("b", 2, false),
        ("c", 3, true),
        ("d", 4, false),
        ("e", 5, true),
        ("fgh", 8, true),
        ("i", 9, true),
    ];
    for (fragment, length, changed) in rows {
        append(&mut view, fragment);
        assert_eq!(observe(&mut view), [Some(length); 3], "{fragment:?}");
        assert_eq!(view.refresh(), changed, "{fragment:?}");
        assert!(!view.refresh(), "{fragment:?}");
    }
}

#[test]
fn ignores_text_after_structural_failure_and_retains_an_invalid_string_prefix() {
    let mut view = PartialArguments::new();
    assert!(!view.has("file_path"));
    assert!(!append_and_refresh(&mut view, r#"{"n":1,"b":}"#));
    assert!(view.invalid());
    assert_eq!(view.value("n"), Some(json!(1)));
    assert!(!append_and_refresh(&mut view, r#","file_path":"x"}"#));
    assert!(!view.has("file_path"));
    assert!(view.invalid());

    let mut mid_string = PartialArguments::new();
    append(&mut mid_string, r#"{"file_path":"ab"#);
    assert_eq!(mid_string.text("file_path"), s("ab"));
    assert!(!append_and_refresh(&mut mid_string, r#"\q"}"#));
    assert!(mid_string.invalid());
    assert_eq!(mid_string.text("file_path"), s("ab"));
    assert_eq!(len(&mut mid_string, "file_path"), Some(2));
    assert!(!mid_string.complete("file_path"));
}

#[test]
fn tracks_a_write_call_streamed_in_3_character_slices_exactly_as_an_independent_oracle_predicts() {
    let content = [
        "import { a } from \"./a.ts\"",
        "",
        "export const x = `${a}\\n`\t// \"quoted\" and C:\\path",
        "bell \u{7} 中文 😀",
        "",
    ]
    .join("\n");
    let text = serde_json::to_string(
        &json!({"file_path": "src/a.ts", "content": content, "overwrite": true}),
    )
    .unwrap();
    // The control character is written as a \uXXXX escape, so the stream exercises one.
    assert!(text.contains("\\u0007"));
    let content_length = content.encode_utf16().count();

    #[derive(Debug, PartialEq)]
    struct Observed {
        path: Option<String>,
        path_done: bool,
        length: Option<usize>,
        steps: Option<usize>,
        overwrite: Option<Value>,
    }
    fn observe(view: &mut PartialArguments) -> Observed {
        let length = len_step(view, "content", 64.0);
        Observed {
            path: view.text("file_path"),
            path_done: view.complete("file_path"),
            length,
            steps: length.map(|length| length.div_ceil(64)),
            overwrite: view.value("overwrite"),
        }
    }

    let mut view = PartialArguments::new();
    let mut before = observe(&mut view);
    assert_eq!(
        before,
        Observed {
            path: None,
            path_done: false,
            length: None,
            steps: None,
            overwrite: None
        }
    );

    // The source (spec line 1086) cuts every three UTF-16 units. A fragment is a `&str`, so a cut that
    // would fall inside the surrogate pair of an emoji moves to the nearest representable cut, after the pair.
    let mut chunks: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut units = 0;
    for character in text.chars() {
        current.push(character);
        units += character.len_utf16();
        if units >= 3 {
            chunks.push(std::mem::take(&mut current));
            units = 0;
        }
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    let mut reported = 0;
    let mut previous_length = 0;
    for chunk in &chunks {
        append(&mut view, chunk);
        let changed = view.refresh();
        let after = observe(&mut view);
        let expect_changed = after.path != before.path
            || after.path_done != before.path_done
            || after.steps != before.steps
            || after.overwrite != before.overwrite;
        assert_eq!(changed, expect_changed, "{chunk:?}");
        if changed {
            reported += 1;
        }
        if let Some(length) = after.length {
            assert!(after.path_done);
            assert!(length >= previous_length);
            previous_length = length;
        }
        before = after;
    }
    assert!(reported > 0);
    assert!(reported < chunks.len() / 2);
    assert_eq!(
        before,
        Observed {
            path: s("src/a.ts"),
            path_done: true,
            length: Some(content_length),
            steps: Some(content_length.div_ceil(64)),
            overwrite: Some(json!(true)),
        }
    );
    assert!(view.complete("content"));
    assert_eq!(view.keys(), ["file_path", "content", "overwrite"]);
    assert!(!view.invalid());
}

// ---- invalid text ----

#[test]
fn freezes_as_invalid_and_ignores_later_text() {
    let cases = [
        ("x", "a first character other than {"),
        ("{1", "a key that is not a string"),
        (r#"{"a":1,}"#, "a trailing comma before the closing brace"),
        (r#"{"a":1, 2"#, "a key that is not a string after a comma"),
        (r#"{"a\q":1}"#, "a key with an invalid escape"),
        (r#"{"a" 1"#, "a missing colon"),
        (r#"{"a":}"#, "a value starting with }"),
        (r#"{"a":,"#, "a value starting with ,"),
        (r#"{"a"::"#, "a value starting with :"),
        (r#"{"a":]"#, "a value starting with ]"),
        (r#"{"a":"b" x"#, "text other than , or } after a value"),
        (r#"{"file_path":"x"}}"#, "a second closing brace"),
        (r#"{"a":[}"#, "mismatched nested brackets"),
        (r#"{"a":{"b":[]]"#, "mismatched nested object brackets"),
        (r#"{"a\u12G4":1}"#, "a non-hex digit in a key escape"),
        ("{\"a\nb\":1}", "an unescaped control character in a key"),
    ];
    for (text, label) in cases {
        assert!(PartialArguments::from_text(text).invalid(), "{label}");

        let mut view = PartialArguments::new();
        assert!(!view.has("later"), "{label}");
        append(&mut view, text);
        assert!(view.invalid(), "{label}");
        assert!(view.closed(), "{label}");
        assert!(
            !append_and_refresh(&mut view, r#","later":"x"}"#),
            "{label}"
        );
        assert!(!view.has("later"), "{label}");
    }
}

#[test]
fn indexes_later_keys_before_validating_the_malformed_value() {
    let cases: [(&str, &str, Option<&str>); 8] = [
        (
            r#"{"file_path":"ab\q","later":"x"}"#,
            "file_path",
            Some("ab"),
        ),
        (
            r#"{"file_path":"ab\u12G4","later":"x"}"#,
            "file_path",
            Some("ab"),
        ),
        (
            "{\"file_path\":\"ab\nc\",\"later\":\"x\"}",
            "file_path",
            Some("ab"),
        ),
        (r#"{"a":tru,"later":"x"}"#, "a", None),
        (r#"{"a":1.,"later":"x"}"#, "a", None),
        (r#"{"a":1. ,"later":"x"}"#, "a", None),
        (r#"{"a":[tru],"later":"x"}"#, "a", None),
        (r#"{"a":{"b":"\q"},"later":"x"}"#, "a", None),
    ];
    for (text, key, prefix) in cases {
        let mut view = PartialArguments::from_text(text);
        assert_eq!(view.keys(), [key, "later"], "{text:?}");
        assert!(view.has(key), "{text:?}");
        assert!(view.complete(key), "{text:?}");
        assert!(view.has("later"), "{text:?}");
        assert!(!view.invalid(), "{text:?}");
        assert_eq!(view.text("later"), s("x"), "{text:?}");
        assert!(!view.invalid(), "{text:?}");
        match prefix {
            None => assert_eq!(view.value(key), None, "{text:?}"),
            Some(prefix) => {
                assert_eq!(view.text(key), s(prefix), "{text:?}");
                assert_eq!(len(&mut view, key), Some(prefix.len()), "{text:?}");
            }
        }
        assert!(view.invalid(), "{text:?}");
        assert!(!view.complete(key), "{text:?}");
        assert_eq!(view.text("later"), s("x"), "{text:?}");
    }
}

#[test]
fn keeps_the_outer_object_open_after_reading_invalid_contents() {
    let cases = [
        (r#"{"bad":"ab\q","#, Some("ab")),
        (r#"{"bad":[tru],"#, None),
    ];
    for (head, prefix) in cases {
        let mut view = streamed(&[head]);
        assert!(!view.closed(), "{head:?}");
        assert!(!view.has("later"), "{head:?}");
        assert!(!view.invalid(), "{head:?}");
        match prefix {
            None => assert_eq!(view.value("bad"), None, "{head:?}"),
            Some(prefix) => assert_eq!(view.text("bad"), s(prefix), "{head:?}"),
        }
        assert!(view.invalid(), "{head:?}");
        assert!(!view.closed(), "{head:?}");
        assert!(!view.refresh(), "{head:?}");

        append(&mut view, "\"later\":\"ok\"");
        assert!(view.refresh(), "{head:?}");
        assert_eq!(view.keys(), ["bad", "later"], "{head:?}");
        assert_eq!(view.text("later"), s("ok"), "{head:?}");
        assert!(view.invalid(), "{head:?}");
        assert!(!view.closed(), "{head:?}");
        append(&mut view, "}");
        assert!(view.refresh(), "{head:?}");
        assert!(view.closed(), "{head:?}");
        assert!(!view.refresh(), "{head:?}");
    }
}

#[test]
fn rejects_an_incomplete_unicode_escape_when_its_closed_string_is_first_read() {
    let mut view = PartialArguments::from_text(r#"{"a":"\u12"}"#);
    assert!(view.complete("a"));
    assert!(!view.invalid());
    assert_eq!(view.text("a"), s(""));
    assert!(view.invalid());
    assert!(!view.complete("a"));
}

#[test]
fn indexes_past_a_nested_unescaped_newline_and_reports_it_when_the_container_is_read() {
    let mut view = PartialArguments::from_text("{\"a\":{\"b\":\"x\ny\"},\"later\":1}");
    assert_eq!(view.keys(), ["a", "later"]);
    assert!(view.complete("a"));
    assert!(!view.invalid());
    assert_eq!(view.value("a"), None);
    assert!(view.invalid());
    assert!(!view.complete("a"));
    assert_eq!(view.value("later"), Some(json!(1)));
}

#[test]
fn discovers_malformed_escapes_from_a_length_only_read() {
    let mut view = PartialArguments::from_text(r#"{"content":"ab\q","later":1}"#);
    assert!(view.complete("content"));
    assert!(!view.invalid());
    assert_eq!(len(&mut view, "content"), Some(2));
    assert!(view.invalid());
    assert!(!view.complete("content"));
    assert_eq!(view.value("later"), Some(json!(1)));
}

#[test]
fn caches_a_malformed_container_result_without_parsing_unrelated_containers() {
    let mut view = PartialArguments::from_text(r#"{"bad":[tru],"good":[1]}"#);
    assert_eq!(view.keys(), ["bad", "good"]);
    assert!(view.has("bad"));
    assert!(view.complete("bad"));
    assert!(view.complete("good"));
    assert!(!view.invalid());
    assert_eq!(view.json_parse_count(), 0);
    assert_eq!(view.value("bad"), None);
    assert_eq!(view.json_parse_count(), 1);
    assert_eq!(view.value("bad"), None);
    assert!(!view.complete("bad"));
    assert!(view.invalid());
    assert_eq!(view.json_parse_count(), 1);
    let good = view.value("good");
    assert_eq!(good, Some(json!([1])));
    assert_eq!(view.value("good"), good);
    assert_eq!(view.json_parse_count(), 2);
}

#[test]
fn stops_at_the_first_invalid_character_of_a_fragment_and_ignores_the_rest_of_it() {
    let mut view = PartialArguments::from_text(r#"{"a" 1,"file_path":"x"}"#);
    assert!(view.invalid());
    assert!(view.keys().is_empty());
    assert!(!view.has("file_path"));
}

#[test]
fn orders_integer_like_keys_ascending_before_the_other_keys_as_javascript_does() {
    let mut object = serde_json::Map::new();
    for key in ["b", "2", "1", "a", "01"] {
        object.insert(key.to_owned(), json!(0));
    }
    let mut view = PartialArguments::from_object(&json!({"o": Value::Object(object)}));
    assert_eq!(view.keys(), ["o"]);
    let nested = view.value("o").unwrap();
    let keys: Vec<&String> = nested.as_object().unwrap().keys().collect();
    assert_eq!(keys, ["1", "2", "b", "a", "01"]);

    let mut top = serde_json::Map::new();
    top.insert("2".to_owned(), json!(0));
    top.insert("1".to_owned(), json!(0));
    assert_eq!(
        PartialArguments::from_object(&Value::Object(top)).keys(),
        ["1", "2"]
    );

    let mut parsed = streamed(&[r#"{"o":{"2":0,"1":0}}"#]);
    assert_eq!(
        serde_json::to_string(&parsed.value("o").unwrap()).unwrap(),
        r#"{"1":0,"2":0}"#
    );
}

#[test]
fn keeps_a_nan_step_as_nan_so_its_length_read_never_reports_a_change() {
    let mut view = PartialArguments::new();
    append(&mut view, r#"{"x":""#);
    assert_eq!(len_step(&mut view, "x", f64::NAN), Some(0));
    assert!(!append_and_refresh(&mut view, "a"));
}
