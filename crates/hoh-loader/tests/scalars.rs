mod support;
use serde_json::{Value, json};
use support::entries;
// Source paths are relative to refs/js-yaml/lib/type; each row records its resolver/constructor line.
#[test]
fn json_schema_432_plain_scalar_table() {
    let cases = [
        ("", Value::Null, "null.js:6"),
        ("~", Value::Null, "null.js:10"),
        ("null", Value::Null, "null.js:11"),
        ("Null", Value::Null, "null.js:11"),
        ("NULL", Value::Null, "null.js:11"),
        ("nUlL", json!("nUlL"), "null.js:11"),
        ("nil", json!("nil"), "null.js:11"),
        ("true", json!(true), "bool.js:10"),
        ("True", json!(true), "bool.js:10"),
        ("TRUE", json!(true), "bool.js:10"),
        ("false", json!(false), "bool.js:11"),
        ("False", json!(false), "bool.js:11"),
        ("FALSE", json!(false), "bool.js:11"),
        ("tRuE", json!("tRuE"), "bool.js:10"),
        ("yes", json!("yes"), "bool.js:10"),
        ("on", json!("on"), "bool.js:10"),
        ("0", json!(0), "int.js:104"),
        ("-0", json!(0), "int.js:104"),
        ("+0", json!(0), "int.js:104"),
        ("1", json!(1), "int.js:80"),
        ("+1", json!(1), "int.js:32"),
        ("-12", json!(-12), "int.js:32"),
        ("01", json!(1), "int.js:80"),
        ("00", json!(0), "int.js:80"),
        ("08", json!(8), "int.js:80"),
        ("0b101", json!(5), "int.js:43"),
        ("+0b101", json!(5), "int.js:43"),
        ("-0b101", json!(-5), "int.js:107"),
        ("0o17", json!(15), "int.js:66"),
        ("-0o17", json!(-15), "int.js:109"),
        ("0x1f", json!(31), "int.js:55"),
        ("+0xF", json!(15), "int.js:55"),
        ("-0xF", json!(-15), "int.js:108"),
        ("0x", json!("0x"), "int.js:63"),
        ("0o", json!("0o"), "int.js:74"),
        ("0b", json!("0b"), "int.js:52"),
        ("0B10", json!("0B10"), "int.js:43"),
        ("0XFF", json!("0XFF"), "int.js:55"),
        ("0O17", json!("0O17"), "int.js:66"),
        ("0b2", json!("0b2"), "int.js:49"),
        ("0o8", json!("0o8"), "int.js:71"),
        ("0xG", json!("0xG"), "int.js:60"),
        ("1_", json!("1_"), "int.js:81"),
        ("0_1", json!("0_1"), "int.js:81"),
        ("0b_", json!("0b_"), "int.js:49"),
        ("0xF_", json!("0xF_"), "int.js:60"),
        ("1__2", json!("1__2"), "int.js:81"),
        ("0o7_1", json!("0o7_1"), "int.js:71"),
        (".1_", json!(".1_"), "float.js:11"),
        ("1_.0", json!("1_.0"), "float.js:8"),
        ("1e_2", json!("1e_2"), "float.js:8"),
        (".", json!("."), "float.js:11"),
        ("+.", json!("+."), "float.js:8"),
        ("-.", json!("-."), "float.js:8"),
        (".e2", json!(".e2"), "float.js:11"),
        ("-.5", json!("-.5"), "float.js:8"),
        ("+.5", json!("+.5"), "float.js:8"),
        (".2", json!(0.2), "float.js:11"),
        (".2e2", json!(20), "float.js:11"),
        (".2e-2", json!(0.002), "float.js:11"),
        ("1.", json!(1), "float.js:8"),
        ("1.2", json!(1.2), "float.js:8"),
        ("-1.2", json!(-1.2), "float.js:51"),
        ("1.e2", json!(100), "float.js:8"),
        ("1e2", json!(100), "float.js:8"),
        ("1E+2", json!(100), "float.js:8"),
        ("1e", json!("1e"), "float.js:8"),
        ("1e+", json!("1e+"), "float.js:8"),
        ("1e999", json!("1e999"), "float.js:31"),
        ("-1e999", json!("-1e999"), "float.js:31"),
        ("1e-999", json!(0), "float.js:31"),
        ("-.nan", json!("-.nan"), "float.js:15"),
        ("+.nan", json!("+.nan"), "float.js:15"),
        ("Infinity", json!("Infinity"), "float.js:13"),
        ("NaN", json!("NaN"), "float.js:15"),
        ("1:20", json!("1:20"), "float.js:8"),
        ("2001-01-01", json!("2001-01-01"), "float.js:8"),
        ("9007199254740993", json!(9007199254740992u64), "int.js:112"),
        (
            "0x20000000000000801",
            json!(36893488147419103232.0_f64),
            "int.js:108",
        ),
    ];
    assert!(cases.len() >= 60);
    for (raw, want, source) in cases {
        let got = entries(&format!("- id: a\n  name: p\n  config: {raw}\n"))
            .remove(0)
            .config;
        assert_eq!(
            std::mem::discriminant(&got),
            std::mem::discriminant(&want),
            "kind: {raw}, {source}"
        );
        assert_eq!(got, want, "value: {raw}, {source}");
    }
    // int.js:52/63/74/89 reject non-finite integers; float.js:31 rejects overflowing decimals.
    for raw in [
        "9".repeat(400),
        format!("0x{}", "F".repeat(300)),
        format!("0b1{}", "0".repeat(1100)),
    ] {
        assert_eq!(
            entries(&format!("- name: p\n  config: {raw}\n"))[0].config,
            json!(raw)
        );
    }
    // int.js:104 discards the sign only when the unsigned spelling is exactly '0'.
    for raw in ["-00", "-0.0", "-0x0", "-0o0", "-0b0"] {
        let value = entries(&format!("- name: p\n  config: {raw}\n"))[0]
            .config
            .as_f64()
            .unwrap();
        assert_eq!(value, 0.0);
        assert!(value.is_sign_negative(), "{raw}");
    }
}
#[test]
fn explicit_tags_resolve_plain_and_quoted_text_identically() {
    for (tag, raw, want) in [
        ("int", "01", json!(1)),
        ("int", "-0", json!(0)),
        ("float", "1", json!(1)),
        ("float", ".2e2", json!(20)),
        ("str", "true", json!("true")),
        ("bool", "TRUE", json!(true)),
        ("null", "~", Value::Null),
    ] {
        for scalar in [raw.to_owned(), serde_json::to_string(raw).unwrap()] {
            assert_eq!(
                entries(&format!("- name: p\n  config: !!{tag} {scalar}\n"))[0].config,
                want,
                "{tag} {scalar}"
            );
        }
    }
}

#[test]
fn explicit_null_empty_and_negative_zero_tags_keep_the_resolver_distinctions() {
    assert_eq!(
        entries("- name: p\n  config: !!null\n")[0].config,
        Value::Null
    );
    for raw in [
        "!!null ''",
        "!!int ''",
        "!!bool ''",
        "!!float ''",
        "!!int 0x",
        "!!float -.5",
    ] {
        assert!(
            hoh_loader::parse_entries(
                &format!("- name: p\n  config: {raw}\n"),
                std::path::Path::new("/tags.yml")
            )
            .is_err(),
            "{raw}"
        );
    }
    let rows = entries("- name: p\n  config: [!!int '-0', !!float '-0', !!float '-0.0']\n");
    assert!(!rows[0].config[0].as_f64().unwrap().is_sign_negative());
    for index in [1, 2] {
        assert!(rows[0].config[index].as_f64().unwrap().is_sign_negative());
    }
}
#[test]
fn radix_integer_rounding_uses_guard_and_sticky_bits_only_once() {
    let value = entries(
        "- name: p\n  config: [0x20000000000001000, 0x20000000000001001, 0x20000000000003000]\n",
    )[0]
    .config
    .clone();
    // 2^65 has an ulp of 8192. Ties round to even; the last bit after the midpoint must survive.
    assert_eq!(value[0].as_f64().unwrap(), 2.0f64.powi(65));
    assert_eq!(value[1].as_f64().unwrap(), 2.0f64.powi(65) + 8192.0);
    assert_eq!(value[2].as_f64().unwrap(), 2.0f64.powi(65) + 16384.0);
}
