mod support;
use hoh_loader::*;
use serde_json::json;
use std::path::Path;
use support::*;
#[test]
fn reads_and_writes_js_scalars_as_expression_nodes() {
    let parsed = entries("- id: a\n  name: example\n  disabled: !!js process.env.OFF\n");
    assert_eq!(parsed[0].disabled, json!({"__jsExpr":"process.env.OFF"}));
    let dump = render_dump(&compose(&parsed, &[], &mut |_| {}));
    assert_eq!(
        dump,
        "# == a · base\n- id: a\n  name: example\n  disabled: !!js \"process.env.OFF\"\n"
    );
    assert_eq!(entries(&dump), parsed);
}
#[test]
fn reads_binary_integers_as_numbers_and_dates_as_strings() {
    let row = entries(
        "- id: a\n  name: example\n  config: [0b101, 2001-01-01, ~, True, FALSE, yes, 0o12, 0x1f, 1.2e3, '01', <<]\n",
    );
    assert_eq!(
        row[0].config,
        json!([
            5,
            "2001-01-01",
            null,
            true,
            false,
            "yes",
            10,
            31,
            1200,
            "01",
            "<<"
        ])
    );
    assert!(parse_entries("", Path::new("/empty.yml")).is_err());
}
#[test]
fn writes_repeated_references_in_full() {
    let rows = entries(
        "- id: a\n  name: example\n  config: &shared {value: 1}\n- id: b\n  name: example\n  config: *shared\n",
    );
    let dump = render_dump(&compose(&rows, &[], &mut |_| {}));
    assert_eq!(
        dump,
        "# == a · base\n- id: a\n  name: example\n  config:\n    value: 1\n# == b · base\n- id: b\n  name: example\n  config:\n    value: 1\n"
    );
    assert_eq!(entries(&dump), rows);
}
#[test]
fn locates_parse_errors() {
    let err = parse_entries("- id: a\n  name: [\n", Path::new("/bad.yml")).unwrap_err();
    match err {
        LoadError::Yaml {
            file, line, column, ..
        } => {
            assert_eq!(file, Path::new("/bad.yml"));
            assert_eq!((line, column), (3, 1));
        }
        _ => panic!("{err}"),
    }
}
#[test]
fn rejects_duplicate_non_string_keys_unknown_row_keys_and_cyclic_aliases() {
    for text in [
        "- id: a\n  name: one\n  name: two\n",
        "- id: a\n  name: one\n  config: {true: no}",
        "- id: a\n  name: one\n  typo: x",
        "- id: a\n  name: one\n  config: &a [*a]",
        "---\n[]\n---\n[]",
        "- id: a\n  name: one\n  config: !!timestamp 2020-01-01",
    ] {
        assert!(
            parse_entries(text, Path::new("/bad.yml")).is_err(),
            "{text}"
        );
    }
}
#[test]
fn validates_all_row_field_shapes() {
    for field in [
        "id: 1",
        "name: true",
        "disabled: yes",
        "group: x",
        "inject: [1]",
        "isolate: {s: false}",
        "intercept: {s: 1}",
        "group: true\n  config: [42]",
    ] {
        let text = format!("- name: p\n  {field}\n");
        assert!(
            parse_entries(&text, Path::new("/bad.yml")).is_err(),
            "{field}"
        );
    }
    assert!(
        entries("- name: p\n  isolate: {a: true, b: label}\n  intercept: {s: {a: 1}}\n")[0]
            .isolate
            .is_some()
    );
}
#[test]
fn merge_keys_remain_literal_and_quoted_scalars_remain_strings() {
    assert_eq!(
        entries("- name: p\n  config: {<<: {x: 1}, quoted: 'true', date: 2020-01-01}\n")[0].config,
        json!({"<<":{"x":1},"quoted":"true","date":"2020-01-01"})
    );
}
#[test]
fn assigns_eight_hexadecimal_ids_unique_across_nested_groups() {
    let raw = entries(
        "- name: cordis:group\n  group: true\n  config:\n    - name: child\n- name: p\n- id: 'a:b'\n  name: p\n",
    );
    assert!(raw[0].id.is_empty() && raw[1].id.is_empty());
    let rows = compose(&raw, &[], &mut |_| {}).entries;
    let child = rows[0].config[0]["id"].as_str().unwrap();
    for id in [&rows[0].id, &rows[1].id, child] {
        assert_eq!(id.len(), 8);
        assert!(id.bytes().all(|b| b.is_ascii_hexdigit()));
    }
    assert_ne!(rows[0].id, rows[1].id);
    assert_ne!(child, rows[1].id);
    assert_eq!(rows[2].id, "a:b");
}
#[test]
fn strings_and_expression_nodes_round_trip_without_changing_types() {
    let row = entries(
        r##"- id: a
  name: p
  config:
    a: "null"
    b: "True"
    c: "0b101"
    d: "1e2"
    e: "hello\nworld"
    f: "# : [] {} ☃"
    g: !!js "process.cwd() + '/a'"
    h: []
    i: {}
"##,
    );
    let dump = render_dump(&compose(&row, &[], &mut |_| {}));
    assert_eq!(entries(&dump), row);
}
#[test]
fn invalid_expression_has_file_and_scalar_position() {
    let error = parse_entries(
        "- name: p\n  config:\n    x: !!js process.exit()\n",
        Path::new("/bad.yml"),
    )
    .unwrap_err();
    match error {
        LoadError::Expr {
            file,
            line,
            column,
            reason,
            ..
        } => {
            assert_eq!(file, Path::new("/bad.yml"));
            assert_eq!(line, 3);
            assert!(column >= 8);
            assert!(reason.position > 0);
        }
        _ => panic!("{error}"),
    }
}
#[test]
fn non_finite_numbers_are_reported_instead_of_becoming_json_null() {
    for n in [".inf", "-.Inf", ".NaN"] {
        assert!(
            parse_entries(
                &format!("- name: p\n  config: {n}\n"),
                Path::new("/bad.yml")
            )
            .is_err()
        );
    }
}

#[test]
fn explicit_numeric_tags_use_their_own_resolvers() {
    for value in ["!!int 1.2", "!!bool yes", "!!null 'nil'"] {
        assert!(
            parse_entries(
                &format!("- name: p\n  config: {value}\n"),
                Path::new("/bad.yml")
            )
            .is_err(),
            "{value}"
        );
    }
    assert_eq!(
        entries(
            "- name: p\n  config: [!!int '01', !!float '1.0', !!str true, !!bool 'TRUE', !!float 1]"
        )[0]
        .config,
        json!([1, 1, "true", true, 1])
    );
}
#[test]
fn expression_sentinel_requires_text_and_does_not_reserve_ordinary_object_properties() {
    assert!(parse_entries("- name: p\n  config: {__jsExpr: 1}", Path::new("/bad.yml")).is_err());
    let rows = entries("- name: p\n  config: {__jsExpr: 1, other: 2}");
    assert_eq!(rows[0].config, json!({"__jsExpr":1,"other":2}));
}

#[test]
fn writer_preserves_negative_zero_and_mapping_order() {
    let rows = entries("- id: a\n  name: p\n  config: {z: -0.0, a: 1, middle: 2}\n");
    let dump = render_dump(&compose(&rows, &[], &mut |_| {}));
    assert_eq!(entries(&dump), rows);
    assert!(dump.find("z: -0").unwrap() < dump.find("a: 1").unwrap());
    assert!(dump.find("a: 1").unwrap() < dump.find("middle: 2").unwrap());
}

#[test]
fn alias_expansion_has_a_node_budget_and_expanded_depth_budget() {
    for (doubling, expected) in [
        (true, "expanded YAML node limit exceeded (100000)"),
        (false, "expanded YAML nesting limit exceeded"),
    ] {
        let mut source = String::from("- id: a\n  name: p\n  config:\n    a0: &a0 [0]\n");
        for i in 1..=140 {
            let prev = i - 1;
            source.push_str(&if doubling {
                format!("    a{i}: &a{i} [*a{prev}, *a{prev}]\n")
            } else {
                format!("    a{i}: &a{i} [*a{prev}]\n")
            });
        }
        let err = parse_entries(&source, Path::new("/alias.yml")).unwrap_err();
        match err {
            LoadError::Yaml {
                file,
                line,
                column,
                message,
            } => {
                assert_eq!(file, Path::new("/alias.yml"));
                assert!(line > 4 && line < 145);
                assert!(column > 0);
                assert_eq!(message, expected);
            }
            _ => panic!("{err}"),
        }
    }
}
#[test]
fn empty_comments_and_null_are_shape_errors_at_all_array_readers() {
    // app-boot/index.ts:594/663 and include/index.ts:299-303 all reject !Array.isArray(undefined/null).
    for source in ["", "# only comments\n", "null\n", "~\n", "---\n"] {
        for error in [
            parse_entries(source, Path::new("/empty.yml")).unwrap_err(),
            parse_patches(source, Path::new("/empty.yml")).unwrap_err(),
        ] {
            match error {
                LoadError::Shape {
                    file,
                    line,
                    column,
                    message,
                } => {
                    assert_eq!(file, Path::new("/empty.yml"));
                    assert!(line >= 1 && column >= 1);
                    assert_eq!(message, "config file must be a top-level YAML array");
                }
                _ => panic!("{error}"),
            }
        }
    }
}
#[test]
fn non_printable_keys_and_values_round_trip_with_yaml_escapes() {
    for (ch, escaped) in [
        ('\u{7f}', "\\x7F"),
        ('\u{85}', "\\N"),
        ('\u{2028}', "\\L"),
        ('\u{2029}', "\\P"),
        ('\u{feff}', "\\uFEFF"),
    ] {
        let mut rows = entries("- id: a\n  name: p\n");
        let key = format!("key{ch}suffix");
        let value = format!("value{ch}suffix");
        rows[0].config = json!({key.clone():value});
        let dump = render_dump(&compose(&rows, &[], &mut |_| {}));
        assert_eq!(dump.matches(escaped).count(), 2, "{dump}");
        assert!(!dump.contains(ch));
        assert_eq!(entries(&dump), rows, "{escaped}");
    }
}
#[test]
fn deep_groups_are_read_without_materializing_every_ancestor_config() {
    let mut text = String::from("- id: g0\n  name: cordis:group\n  group: true\n  config:\n");
    for depth in 1..=30 {
        let pad = " ".repeat(depth * 4);
        text.push_str(&format!(
            "{pad}- id: g{depth}\n{pad}  name: cordis:group\n{pad}  group: true\n{pad}  config:\n"
        ));
    }
    text.push_str(&format!(
        "{}- id: leaf\n{}  name: p\n{}  config: {{payload: x}}\n",
        " ".repeat(124),
        " ".repeat(124),
        " ".repeat(124)
    ));
    let rows = entries(&text);
    let mut value = &rows[0].config;
    for _ in 0..30 {
        value = &value[0]["config"];
    }
    assert_eq!(value[0]["id"], json!("leaf"));
}
