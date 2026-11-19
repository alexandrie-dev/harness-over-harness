use hoh_loader::resolve_telemetry_patch;
#[test]
fn preserves_the_configured_telemetry_mode_when_the_hard_disable_switch_is_unset_or_empty() {
    assert!(resolve_telemetry_patch(None, true).is_none());
    assert!(resolve_telemetry_patch(Some(""), true).is_none());
}
#[test]
fn disables_on_any_non_empty_value_including_falsy_looking_ones() {
    for value in ["1", "0", "false", "no"] {
        let patch = resolve_telemetry_patch(Some(value), true).unwrap();
        assert_eq!(patch.id, Some("session-telemetry-otel".into()));
        assert_eq!(patch.set.len(), 1);
        assert_eq!(patch.set["disabled"], true);
        assert!(patch.insert.is_none());
    }
}
#[test]
fn is_trivially_satisfied_by_a_composition_without_the_telemetry_row() {
    assert!(resolve_telemetry_patch(Some("1"), false).is_none());
    assert!(resolve_telemetry_patch(None, false).is_none());
}
