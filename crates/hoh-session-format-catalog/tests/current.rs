mod common;
use common::*;
use hoh_session_format_catalog::current::*;
#[test]
fn rejects_version_skew_before_entering_current_session_validation() {
    let header = h(0, "installed-current");
    let message = format!(
        "installed Session format is v{}, got v0",
        hoh_session::SESSION_FORMAT_VERSION
    );
    assert_eq!(
        validate_installed_current_session_header(&header)
            .unwrap_err()
            .to_string(),
        message
    );
    assert_eq!(
        validate_installed_current_session_artifact(&artifact(vec![], header, 0))
            .unwrap_err()
            .to_string(),
        message
    );
}
#[test]
fn accepts_a_current_header_whose_numeric_version_is_an_integral_float() {
    let mut header = h(4, "installed-current");
    header.insert("version".into(), Value::from(4.0));
    let header_result = validate_installed_current_session_header(&header);
    let artifact_result = validate_installed_current_session_artifact(&artifact(vec![], header, 0));
    header_result.unwrap();
    artifact_result.unwrap();
}

#[test]
fn accepts_only_current_request_header_reasons_and_the_true_starts_series_marker() {
    for (reason, marker, error) in [
        ("fallback", None, Some("reason")),
        ("initial", Some(false), Some("startsSeries")),
        ("series", Some(true), None),
    ] {
        let mut r = e(
            "request/header",
            1,
            json!({"header":{"config":{"provider":"mock","model":"mock"}},"reason":reason}),
        );
        if let Some(marker) = marker {
            r["data"]["startsSeries"] = marker.into();
        }
        let a = artifact(
            vec![ev("turn/start", json!({"turn":1})), r],
            h(4, "installed-current"),
            0,
        );
        let result = validate_installed_current_session_artifact(&a);
        if let Some(message) = error {
            assert!(result.unwrap_err().to_string().contains(message));
        } else {
            result.unwrap();
        }
    }
}
