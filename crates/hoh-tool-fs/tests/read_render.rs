//! Tests for the line-windowing module: offset/limit windows, byte caps, per-line truncation, CRLF
//! stripping, offset-past-EOF rejection, and the capped line buffer for newline-free giant lines —
//! all over a stream of decoded text chunks (so one code path serves whole-file and streamed reads).

use futures_util::stream;
use hoh_error::{BoxError, HarnessError};
use hoh_tool_fs::read_render::{
    FileTextLine, READ_MAX_BYTES, READ_MAX_LINE_LENGTH, ReadWindow, WindowResult, build_window,
    lang_from_path, read_meta_from_meta,
};
use hoh_util_code_language::language_for_path;
use serde_json::{Value, json};

const READ_ALL: ReadWindow = ReadWindow {
    offset: 1.0,
    limit: 2000,
    max_line_length: READ_MAX_LINE_LENGTH,
    max_bytes: READ_MAX_BYTES,
};

fn window(offset: u64, limit: u64) -> ReadWindow {
    ReadWindow {
        offset: offset as f64,
        limit,
        ..READ_ALL
    }
}

/// Yield `text` as one chunk (whole-file read shape).
async fn whole(text: &str, request: &ReadWindow) -> Result<WindowResult, BoxError> {
    build_window(stream::iter([Ok(text.to_owned())]), request, "f").await
}

/// Yield `text` split into fixed-size chunks (streamed read shape).
async fn chunked(text: &str, size: usize, request: &ReadWindow) -> Result<WindowResult, BoxError> {
    let chars: Vec<char> = text.chars().collect();
    let chunks: Vec<Result<String, BoxError>> = chars
        .chunks(size)
        .map(|chunk| Ok(chunk.iter().collect::<String>()))
        .collect();
    build_window(stream::iter(chunks), request, "f").await
}

fn line(number: u64, text: &str) -> FileTextLine {
    FileTextLine {
        number,
        text: text.into(),
    }
}

fn texts(result: &WindowResult) -> Vec<&str> {
    result.lines.iter().map(|line| line.text.as_str()).collect()
}

#[tokio::test]
async fn numbers_lines_and_reports_total_for_a_whole_file_read() {
    let result = whole("one\ntwo\nthree", &READ_ALL).await.unwrap();
    assert_eq!(
        result.lines,
        [line(1, "one"), line(2, "two"), line(3, "three")]
    );
    assert_eq!(result.total_lines, 3);
    assert!(!result.truncated_by_bytes);
}

#[tokio::test]
async fn applies_offset_and_limit() {
    let result = whole("one\ntwo\nthree\nfour", &window(2, 2)).await.unwrap();
    let numbers: Vec<u64> = result.lines.iter().map(|line| line.number).collect();
    assert_eq!(numbers, [2, 3]);
    assert_eq!(result.total_lines, 4);
}

#[tokio::test]
async fn strips_crlf() {
    let result = whole("one\r\ntwo\r\n", &READ_ALL).await.unwrap();
    assert_eq!(texts(&result), ["one", "two"]);
}

#[tokio::test]
async fn truncates_an_over_long_line() {
    let result = whole(&"x".repeat(3000), &READ_ALL).await.unwrap();
    assert!(result.lines[0].text.contains(&format!(
        "... (line truncated to {READ_MAX_LINE_LENGTH} chars)"
    )));
}

#[tokio::test]
async fn caps_output_bytes_and_reports_truncated_by_bytes() {
    let big = vec!["y".repeat(100); 2000].join("\n");
    let result = whole(&big, &READ_ALL).await.unwrap();
    assert!(result.truncated_by_bytes);
}

#[tokio::test]
async fn reads_an_empty_file_at_offset_1_as_zero_lines() {
    let result = whole("", &READ_ALL).await.unwrap();
    assert_eq!(result.lines, []);
    assert_eq!(result.total_lines, 0);
}

#[tokio::test]
async fn rejects_an_offset_past_eof() {
    let error = whole("one\ntwo", &window(9, 1)).await.unwrap_err();
    assert_eq!(
        HarnessError::of(error.as_ref()).unwrap().code(),
        "FS_NOT_FOUND"
    );
}

#[tokio::test]
async fn flushes_a_final_line_with_no_trailing_newline() {
    let result = whole("one\ntwo", &READ_ALL).await.unwrap();
    assert_eq!(texts(&result), ["one", "two"]);
}

#[tokio::test]
async fn handles_a_trailing_newline_without_a_dangling_empty_line() {
    let result = whole("one\ntwo\n", &READ_ALL).await.unwrap();
    assert_eq!(texts(&result), ["one", "two"]);
    assert_eq!(result.total_lines, 2);
}

mod caps_are_per_request {
    use super::*;

    #[tokio::test]
    async fn truncates_lines_at_a_custom_max_line_length_and_names_it_in_the_suffix() {
        let request = ReadWindow {
            offset: 1.0,
            limit: 10,
            max_line_length: 5,
            max_bytes: READ_MAX_BYTES,
        };
        let result = whole("abcdefghij", &request).await.unwrap();
        assert_eq!(result.lines[0].text, "abcde... (line truncated to 5 chars)");
    }

    #[tokio::test]
    async fn caps_output_at_a_custom_max_bytes() {
        let request = ReadWindow {
            offset: 1.0,
            limit: 10,
            max_line_length: 2000,
            max_bytes: 9,
        };
        let result = whole("aaaa\nbbbb\ncccc", &request).await.unwrap();
        assert_eq!(texts(&result), ["aaaa", "bbbb"]);
        assert_eq!(result.total_lines, 3);
        assert!(result.truncated_by_bytes);
    }
}

mod chunked_input {
    use super::*;

    #[tokio::test]
    async fn windows_identically_when_text_arrives_in_small_chunks() {
        let result = chunked("one\ntwo\nthree", 2, &window(2, 1)).await.unwrap();
        assert_eq!(result.lines, [line(2, "two")]);
        assert_eq!(result.total_lines, 3);
    }

    #[tokio::test]
    async fn caps_a_newline_free_giant_line_split_across_chunks() {
        let result = chunked(&"z".repeat(5000), 256, &READ_ALL).await.unwrap();
        assert!(result.lines[0].text.contains(&format!(
            "... (line truncated to {READ_MAX_LINE_LENGTH} chars)"
        )));
    }

    #[tokio::test]
    async fn caps_output_bytes_mid_stream() {
        let big = vec!["y".repeat(100); 2000].join("\n");
        let result = chunked(&big, 512, &READ_ALL).await.unwrap();
        assert_eq!(result.total_lines, 2000);
        assert!(result.truncated_by_bytes);
    }

    #[tokio::test]
    async fn flushes_a_final_newline_terminated_line_across_a_chunk_boundary() {
        let result = chunked("one\ntwo\n", 3, &READ_ALL).await.unwrap();
        assert_eq!(texts(&result), ["one", "two"]);
    }
}

/// The short `lang` values a recorded read persisted, transcribed verbatim. Changing any value
/// changes replay-visible output for an already-recorded session, so this table is the test's
/// independent expectation rather than a second read of the implementation's own data.
const PERSISTED_READ_LANG_BY_EXTENSION: [(&str, &str); 43] = [
    ("ts", "ts"),
    ("tsx", "tsx"),
    ("mts", "ts"),
    ("cts", "ts"),
    ("js", "js"),
    ("jsx", "jsx"),
    ("mjs", "js"),
    ("cjs", "js"),
    ("json", "json"),
    ("jsonc", "json"),
    ("py", "py"),
    ("rb", "rb"),
    ("go", "go"),
    ("rs", "rs"),
    ("java", "java"),
    ("c", "c"),
    ("h", "c"),
    ("cc", "cpp"),
    ("cpp", "cpp"),
    ("hpp", "cpp"),
    ("cxx", "cpp"),
    ("cs", "cs"),
    ("kt", "kotlin"),
    ("swift", "swift"),
    ("php", "php"),
    ("sh", "sh"),
    ("bash", "sh"),
    ("zsh", "sh"),
    ("yaml", "yaml"),
    ("yml", "yaml"),
    ("toml", "toml"),
    ("ini", "ini"),
    ("md", "md"),
    ("markdown", "md"),
    ("mdx", "mdx"),
    ("html", "html"),
    ("htm", "html"),
    ("css", "css"),
    ("scss", "scss"),
    ("less", "less"),
    ("sql", "sql"),
    ("xml", "xml"),
    ("lua", "lua"),
];

#[test]
fn lang_keeps_every_already_persisted_suffix_byte_identical() {
    assert_eq!(PERSISTED_READ_LANG_BY_EXTENSION.len(), 43);
    for (extension, hint) in PERSISTED_READ_LANG_BY_EXTENSION {
        assert_eq!(
            lang_from_path(&format!("file.{extension}")),
            Some(hint),
            "{extension}"
        );
    }
}

#[test]
fn lang_gives_every_other_suffix_the_language_short_id() {
    // A suffix with no persisted value has none to preserve, so the projection applies the
    // language's short name instead of the canonical grammar id.
    for (path, hint) in [
        ("build.ps1", "ps1"),
        ("table.csv", "csv"),
        ("deploy.bat", "bat"),
        (".env", "env"),
        ("server.log", "log"),
        ("message.proto", "proto"),
        ("infra.tf", "tf"),
        ("paper.tex", "tex"),
        ("model.jl", "jl"),
        ("top.v", "v"),
        ("build.gradle", "gradle"),
        // A later suffix of an already-known language follows that language's short name.
        ("app.conf", "ini"),
        ("task.rake", "rb"),
        ("events.jsonl", "json"),
        ("page.xhtml", "html"),
        ("logo.svg", "xml"),
        ("notebook.ipynb", "json"),
    ] {
        assert_eq!(lang_from_path(path), Some(hint), "{path}");
    }
}

#[test]
fn lang_maps_a_known_extension_to_its_persisted_short_hint_case_insensitively() {
    for (path, hint) in [
        ("src/a.ts", "ts"),
        ("src/a.TSX", "tsx"),
        ("/abs/module.mjs", "js"),
        ("conf.yml", "yaml"),
        ("README.md", "md"),
    ] {
        assert_eq!(lang_from_path(path), Some(hint), "{path}");
    }
}

#[test]
fn lang_keeps_the_canonical_ids_the_client_surfaces_use_out_of_the_persisted_hint() {
    // The Client reads the shared table directly; only this projection owns the persisted value,
    // so the two intentionally differ for the suffixes a recorded session already holds.
    assert_eq!(language_for_path("src/a.ts"), Some("typescript"));
    assert_eq!(language_for_path("README.md"), Some("markdown"));
    assert_ne!(lang_from_path("src/a.ts"), language_for_path("src/a.ts"));
}

#[test]
fn lang_reads_the_extension_after_the_last_path_segment_and_last_dot() {
    assert_eq!(lang_from_path("a.py.bak"), None);
    assert_eq!(lang_from_path("archive.tar.gz"), None);
    assert_eq!(lang_from_path("/dir.py/plain"), None);
    assert_eq!(lang_from_path("C:\\src\\main.rs"), Some("rs"));
}

#[test]
fn lang_is_none_for_a_dotfile_an_extensionless_name_and_an_unknown_extension() {
    assert_eq!(lang_from_path(".gitignore"), None);
    assert_eq!(lang_from_path("/etc/hosts"), None);
    assert_eq!(lang_from_path("data.unknownext"), None);
    assert_eq!(lang_from_path("trailingdot."), None);
}

#[test]
fn lang_is_none_for_an_extension_that_is_an_object_prototype_key() {
    // Own-property lookup only: these must not resolve to the inherited member.
    for path in [
        "foo.constructor",
        "foo.__proto__",
        "foo.toString",
        "foo.hasOwnProperty",
    ] {
        assert_eq!(lang_from_path(path), None, "{path}");
    }
}

fn good() -> Value {
    json!({ "path": "/abs/a.ts", "offset": 1, "lines": [{ "number": 1, "text": "x" }], "totalLines": 1, "lang": "ts" })
}

fn with(key: &str, value: Value) -> Value {
    let mut meta = good();
    meta[key] = value;
    meta
}

fn reject(meta: &Value) {
    assert_eq!(read_meta_from_meta(Some(meta)), None, "{meta}");
}

fn roundtrip(meta: &Value) {
    let narrowed = read_meta_from_meta(Some(meta)).expect("well-formed meta narrows");
    assert_eq!(narrowed.path, meta["path"].as_str().unwrap());
    assert_eq!(narrowed.offset, meta["offset"].as_u64().unwrap());
    assert_eq!(narrowed.total_lines, meta["totalLines"].as_u64().unwrap());
    assert_eq!(
        narrowed.lang.as_deref(),
        meta.get("lang").and_then(Value::as_str)
    );
    let expected: Vec<FileTextLine> = meta["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|line| FileTextLine {
            number: line["number"].as_u64().unwrap(),
            text: line["text"].as_str().unwrap().into(),
        })
        .collect();
    assert_eq!(narrowed.lines, expected);
}

#[test]
fn meta_narrows_a_well_formed_read_meta_with_and_without_a_lang_hint() {
    roundtrip(&good());
    roundtrip(&json!({ "path": "/abs/a", "offset": 1, "lines": [], "totalLines": 0 }));
}

#[test]
fn meta_narrows_an_empty_window_at_a_positive_offset() {
    roundtrip(&json!({ "path": "/abs/a", "offset": 5, "lines": [], "totalLines": 9 }));
}

#[test]
fn meta_is_none_for_absent_non_object_or_array_meta() {
    assert_eq!(read_meta_from_meta(None), None);
    assert_eq!(read_meta_from_meta(Some(&Value::Null)), None);
    assert_eq!(read_meta_from_meta(Some(&json!("nope"))), None);
    assert_eq!(read_meta_from_meta(Some(&json!([good()]))), None);
}

#[test]
fn meta_is_none_when_a_field_is_missing_or_the_wrong_type() {
    reject(&with("path", json!(5)));
    reject(&with("offset", json!("1")));
    reject(&with("totalLines", json!("1")));
    reject(&with("lines", json!("nope")));
    reject(&with("lines", json!([{ "number": "1", "text": "x" }])));
    reject(&with("lines", json!([{ "number": 1 }])));
    reject(&with("lines", json!([null])));
    reject(&with("lang", json!(5)));
}

#[test]
fn meta_rejects_an_offset_that_is_not_a_one_based_integer() {
    reject(&with("offset", json!(0)));
    reject(&with("offset", json!(1.5)));
    // NaN and Infinity are not JSON values; a JSON null is the nearest hand-edited form.
    reject(&with("offset", Value::Null));
}

#[test]
fn meta_rejects_a_first_line_number_below_offset() {
    let mut meta = good();
    meta["offset"] = json!(2);
    meta["totalLines"] = json!(2);
    reject(&meta);
}

#[test]
fn meta_rejects_a_line_number_that_is_not_a_one_based_integer() {
    for (number, total) in [(json!(0), 1), (json!(1.5), 2), (Value::Null, 1)] {
        let mut meta = good();
        meta["lines"] = json!([{ "number": number, "text": "x" }]);
        meta["totalLines"] = json!(total);
        reject(&meta);
    }
}

#[test]
fn meta_rejects_a_total_lines_that_is_not_a_non_negative_integer() {
    reject(&with("totalLines", json!(-1)));
    reject(&with("totalLines", json!(1.5)));
    reject(&with("totalLines", Value::Null));
}

#[test]
fn meta_rejects_lines_that_do_not_strictly_increase_or_exceed_total_lines() {
    let base = |lines: Value, total: u64| json!({ "path": "/abs/a", "offset": 1, "lang": "ts", "lines": lines, "totalLines": total });
    // Duplicate line numbers.
    reject(&base(
        json!([{ "number": 1, "text": "a" }, { "number": 1, "text": "b" }]),
        2,
    ));
    // Out-of-order line numbers.
    reject(&base(
        json!([{ "number": 2, "text": "b" }, { "number": 1, "text": "a" }]),
        2,
    ));
    // A line number past totalLines.
    reject(&base(json!([{ "number": 3, "text": "c" }]), 2));
}

mod utf16_boundary {
    use super::*;

    fn narrow() -> ReadWindow {
        ReadWindow {
            max_line_length: 5,
            ..READ_ALL
        }
    }

    /// `line.substring(0, 5)` of "abcd😀tail" keeps the high surrogate of the emoji; encoded as UTF-8
    /// for the model it is U+FFFD (three bytes), exactly what JavaScript's `Buffer.from` produces.
    #[tokio::test]
    async fn a_cut_inside_an_emoji_keeps_what_substring_keeps() {
        let expected = "abcd\u{FFFD}... (line truncated to 5 chars)";
        let whole_read = whole("abcd😀tail", &narrow()).await.unwrap();
        assert_eq!(texts(&whole_read), [expected]);
        assert_eq!(whole_read.lines[0].text.len(), 4 + 3 + 31);
        let streamed = chunked("abcd😀tail", 2, &narrow()).await.unwrap();
        assert_eq!(texts(&streamed), [expected]);
    }

    #[tokio::test]
    async fn a_cut_between_two_emoji_is_clean() {
        let result = whole("a😀😀b", &narrow()).await.unwrap();
        assert_eq!(texts(&result), ["a😀😀... (line truncated to 5 chars)"]);
    }

    #[tokio::test]
    async fn a_cut_just_before_an_emoji_is_clean() {
        let result = whole("abcde😀", &narrow()).await.unwrap();
        assert_eq!(texts(&result), ["abcde... (line truncated to 5 chars)"]);
    }
}

#[tokio::test]
async fn an_offset_past_the_end_names_the_unrounded_number() {
    let request = ReadWindow {
        offset: 1e20,
        ..READ_ALL
    };
    let error = whole("one\ntwo", &request).await.unwrap_err();
    let harness = HarnessError::of(error.as_ref()).expect("a filesystem error");
    assert_eq!(harness.code(), "FS_NOT_FOUND");
    assert_eq!(
        harness.message(),
        "offset 100000000000000000000 is out of range for \"f\" (2 lines)"
    );
}
