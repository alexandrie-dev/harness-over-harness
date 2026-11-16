use hoh_credentials::*;
use indexmap::IndexMap;
use serde_json::{Value, json};

#[test]
fn key_helpers_preserve_segments_and_exact_type_error_messages() {
    let key = credential_key("llm-pi-ai", "openai-codex").unwrap();
    assert_eq!(key.as_str(), "llm-pi-ai/openai-codex");
    assert_eq!(parse_credential_key(key.as_str()).unwrap(), key);
    assert_eq!(credential_key_scope(&key), "llm-pi-ai");
    assert_eq!(credential_key_id(&key), "openai-codex");
    for invalid in ["", "a", "a/b/c", "/", "a/", "/b"] {
        let error = parse_credential_key(invalid).unwrap_err();
        let parts: Vec<_> = invalid.split('/').collect();
        let expected = if parts.len() != 2 {
            format!("credential key \"{invalid}\" must be \"<scope>/<id>\"")
        } else {
            let segment = if is_credential_key_segment(parts[0]) {
                parts[1]
            } else {
                parts[0]
            };
            format!("credential key segment \"{segment}\" must match /^[a-z][a-z0-9-]*$/")
        };
        assert_eq!(error.to_string(), expected);
    }
    assert_eq!(
        credential_key("Bad", "Bad2").unwrap_err().to_string(),
        "credential key segment \"Bad\" must match /^[a-z][a-z0-9-]*$/"
    );
    assert_eq!(
        credential_ref("a-b").unwrap_err().to_string(),
        "credential ref \"a-b\" must match /^[A-Za-z_][A-Za-z0-9_]*$/"
    );
}

#[test]
fn grammars_remain_disjoint_and_ascii_only() {
    for valid in ["A", "_", "a9", "__", "A_B_C"] {
        assert!(is_credential_ref_name(valid));
        assert!(credential_ref(valid).is_ok());
    }
    for invalid in [
        "\n",
        "a\n",
        "a\r",
        "a\r\n",
        "a\u{2028}",
        "é",
        "变量",
        "a/b",
        "a\0",
    ] {
        assert!(!is_credential_ref_name(invalid));
        assert!(!is_credential_key_segment(invalid));
    }
    for value in ["a-", "a--b", "z9"] {
        assert!(is_credential_key_segment(value));
    }
    let key = credential_key("a", "b").unwrap();
    assert!(!is_credential_ref_name(key.as_str()));
}

#[test]
fn record_union_preserves_tags_fields_opaque_payload_and_order() {
    let mut env = IndexMap::new();
    env.insert("Z".into(), "z".into());
    env.insert("A".into(), "a".into());
    let api = ApiKeyRecord {
        key: Some("secret".into()),
        env: Some(env),
    };
    let record = CredentialRecord::ApiKey(api.clone());
    let value = serde_json::to_value(&record).unwrap();
    assert_eq!(
        value,
        json!({"kind":"api-key","key":"secret","env":{"Z":"z","A":"a"}})
    );
    assert_eq!(
        value.as_object().unwrap().keys().collect::<Vec<_>>(),
        ["kind", "key", "env"]
    );
    assert_eq!(
        value["env"].as_object().unwrap().keys().collect::<Vec<_>>(),
        ["Z", "A"]
    );
    assert_eq!(
        serde_json::from_value::<CredentialRecord>(value.clone()).unwrap(),
        record
    );
    assert_eq!(serde_json::from_value::<ApiKeyRecord>(value).unwrap(), api);
    assert_eq!(
        serde_json::to_value(CredentialRecord::ApiKey(ApiKeyRecord::default())).unwrap(),
        json!({"kind":"api-key"})
    );
    for payload in [
        Value::Null,
        json!([1,"secret",{"opaque":true}]),
        json!({"z":2,"a":1}),
    ] {
        let grant = GrantRecord {
            payload: payload.clone(),
        };
        let record = CredentialRecord::Grant(grant.clone());
        let value = serde_json::to_value(&record).unwrap();
        assert_eq!(value, json!({"kind":"grant","payload":payload}));
        assert_eq!(
            value.as_object().unwrap().keys().collect::<Vec<_>>(),
            ["kind", "payload"]
        );
        assert_eq!(
            serde_json::from_value::<CredentialRecord>(value.clone()).unwrap(),
            record
        );
        assert_eq!(serde_json::from_value::<GrantRecord>(value).unwrap(), grant);
    }
    assert!(serde_json::from_value::<CredentialRecord>(json!({"kind":"unknown"})).is_err());
}

#[test]
fn safe_views_omit_absent_fields_and_never_add_values() {
    let info = CredentialInfo {
        configured: false,
        source: None,
        writable: true,
        masked: None,
    };
    let value = serde_json::to_value(info).unwrap();
    assert_eq!(value, json!({"configured":false,"writable":true}));
    let info = CredentialInfo {
        configured: true,
        source: Some("file".into()),
        writable: false,
        masked: Some("***".into()),
    };
    let value = serde_json::to_value(info).unwrap();
    assert_eq!(
        value.as_object().unwrap().keys().collect::<Vec<_>>(),
        ["configured", "source", "writable", "masked"]
    );
    let info = CredentialRecordInfo {
        configured: false,
        kind: None,
        writable: true,
    };
    assert_eq!(
        serde_json::to_value(info).unwrap(),
        json!({"configured":false,"writable":true})
    );
    let info = CredentialRecordInfo {
        configured: true,
        kind: Some(CredentialRecordKind::Grant),
        writable: true,
    };
    assert_eq!(
        serde_json::to_value(info)
            .unwrap()
            .as_object()
            .unwrap()
            .keys()
            .collect::<Vec<_>>(),
        ["configured", "kind", "writable"]
    );
    assert_eq!(
        serde_json::to_value(credential_ref("KEY").unwrap()).unwrap(),
        json!("KEY")
    );
    assert_eq!(
        serde_json::to_value(credential_key("a", "b").unwrap()).unwrap(),
        json!("a/b")
    );
}
