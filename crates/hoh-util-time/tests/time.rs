//! Port of `tests/time.spec.ts`.

use hoh_util_time::canonical_client_time_zone;

#[test]
fn canonical_client_time_zone_accepts_utc_and_area_location_names_unchanged() {
    assert_eq!(canonical_client_time_zone("UTC").as_deref(), Some("UTC"));
    assert_eq!(
        canonical_client_time_zone("Asia/Shanghai").as_deref(),
        Some("Asia/Shanghai")
    );
    assert_eq!(
        canonical_client_time_zone("Europe/London").as_deref(),
        Some("Europe/London")
    );
}

#[test]
fn canonical_client_time_zone_answers_the_canonical_name_rather_than_the_alias_asked_for() {
    // A durable record is compared against the zone a later reader derives, so an alias must not survive the boundary.
    let canonical = canonical_client_time_zone("Asia/Chongqing");
    assert_ne!(canonical.as_deref(), Some("Asia/Chongqing"));
    assert_eq!(
        canonical_client_time_zone(canonical.as_deref().unwrap_or("")),
        canonical
    );
}

#[test]
fn canonical_client_time_zone_refuses_blank_padded_abbreviated_and_single_segment_names() {
    for value in ["", " ", " UTC", "UTC ", "CST", "GMT+8", "Asia", "utc"] {
        assert_eq!(canonical_client_time_zone(value), None, "{value:?}");
    }
}

#[test]
fn canonical_client_time_zone_refuses_a_well_formed_name_the_platform_does_not_support() {
    assert_eq!(canonical_client_time_zone("Not/A_Real_Zone"), None);
}
