use hoh_llm::*;
#[test]
fn accepts_a_printable_ascii_key_unchanged() {
    assert_eq!(
        normalize_api_key("sk-0123456789abcdef"),
        ApiKeyCheck::Ok {
            value: "sk-0123456789abcdef".into()
        }
    );
}
#[test]
fn trims_surrounding_whitespace_before_judging() {
    assert_eq!(
        normalize_api_key("  sk-abc\t\n"),
        ApiKeyCheck::Ok {
            value: "sk-abc".into()
        }
    );
    assert_eq!(
        normalize_api_key("\u{feff}sk-abc\u{feff}"),
        ApiKeyCheck::Ok {
            value: "sk-abc".into()
        }
    );
}
#[test]
fn rejects_empty_values_as_empty() {
    for raw in ["", "   ", "\t"] {
        assert_eq!(
            normalize_api_key(raw),
            ApiKeyCheck::Rejected {
                reason: ApiKeyRejection::Empty
            }
        );
    }
}
#[test]
fn rejects_illegal_characters() {
    for raw in [
        "sk-😀abc",
        "sk-你好",
        "sk-abc，",
        "sk-abc def",
        "sk-abc\x01",
        "sk-café",
    ] {
        assert_eq!(
            normalize_api_key(raw),
            ApiKeyCheck::Rejected {
                reason: ApiKeyRejection::IllegalCharacters
            }
        );
    }
}
#[test]
fn accepts_the_printable_ascii_boundary_characters() {
    assert_eq!(
        serde_json::to_value(normalize_api_key("!~")).unwrap(),
        serde_json::json!({"ok":true,"value":"!~"})
    );
    assert_eq!(
        serde_json::to_value(normalize_api_key("")).unwrap(),
        serde_json::json!({"ok":false,"reason":"empty"})
    );
    assert_eq!(
        normalize_api_key("!~"),
        ApiKeyCheck::Ok { value: "!~".into() }
    );
}
#[test]
fn publishes_a_code_distinct_from_a_missing_credential() {
    assert_eq!(INVALID_CREDENTIAL_CODE, "INVALID_CREDENTIAL");
    assert_ne!(INVALID_CREDENTIAL_CODE, "MISSING_CREDENTIAL");
}
#[test]
fn returns_the_trimmed_key_when_it_is_usable() {
    assert_eq!(
        assert_usable_api_key(" sk-abc ", "llm-deepseek", "DEEPSEEK_API_KEY").unwrap(),
        "sk-abc"
    );
}
#[test]
fn refuses_a_blank_stored_credential_naming_the_reference() {
    assert!(
        assert_usable_api_key(" ", "llm-deepseek", "DEEPSEEK_API_KEY")
            .unwrap_err()
            .message()
            .contains("llm-deepseek: the API key resolved from DEEPSEEK_API_KEY is blank")
    );
}
#[test]
fn refuses_an_unusable_stored_credential_with_the_invalid_credential_code() {
    let e = assert_usable_api_key("sk-😀", "llm-pi-ai", "ACME_API_KEY").unwrap_err();
    assert_eq!(e.code(), INVALID_CREDENTIAL_CODE);
    assert!(e.message().contains("llm-pi-ai"));
    assert!(e.message().contains("ACME_API_KEY"));
}
#[test]
fn never_echoes_the_key_it_refuses() {
    assert!(
        !assert_usable_api_key("sk-😀supersecret", "llm-deepseek", "DEEPSEEK_API_KEY")
            .unwrap_err()
            .message()
            .contains("supersecret")
    );
}
