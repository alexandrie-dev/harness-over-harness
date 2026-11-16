use hoh_brand::{brand_number, brand_string, branded_number, branded_string};
branded_number!(EventOrdinal, u64);
branded_string!(SessionId);

#[test]
fn brands_a_number_without_changing_its_runtime_value() {
    let value: EventOrdinal = brand_number(7);
    assert_eq!(u64::from(value), 7);
    assert_eq!(serde_json::to_string(&value).unwrap(), "7");
}

#[test]
fn string_brand_preserves_wire_value() {
    let id: SessionId = brand_string("session-1");
    assert_eq!(id.as_ref(), "session-1");
    assert_eq!(serde_json::to_string(&id).unwrap(), "\"session-1\"");
    assert_eq!(
        serde_json::from_str::<SessionId>("\"session-1\"").unwrap(),
        id
    );
}

#[test]
fn string_brand_supports_borrowed_lookup_and_comparison() {
    let id: SessionId = brand_string("x");
    assert_eq!(id.as_str(), "x");
    assert!(id == "x");
    assert!(id.eq("x"));
    assert!(!id.eq("y"));
    let map = std::collections::HashMap::from([(id, 7)]);
    assert_eq!(map.get("x"), Some(&7));
    assert_eq!(map.get("missing"), None);
}
#[test]
fn integer_brands_are_ordered_hashable_map_keys() {
    let a: EventOrdinal = brand_number(7);
    let b: EventOrdinal = brand_number(2);
    let mut values = [a, b];
    values.sort();
    assert_eq!(values, [b, a]);
    let map = std::collections::HashMap::from([(a, "seven"), (b, "two")]);
    assert_eq!(map.get(&brand_number::<EventOrdinal>(7)), Some(&"seven"));
    let ordered = std::collections::BTreeSet::from([a, b]);
    assert_eq!(ordered.into_iter().collect::<Vec<_>>(), [b, a]);
}
mod llm {
    pub use hoh_brand::ids::SessionId;
}
mod session {
    pub use hoh_brand::ids::SessionId;
}
#[test]
fn two_modules_share_one_session_id_type() {
    let id: llm::SessionId = brand_string("session-1");
    let same: session::SessionId = id.clone();
    assert_eq!(id, same);
    assert_eq!(serde_json::to_string(&same).unwrap(), "\"session-1\"");
}
#[test]
fn all_shared_brands_are_transparent() {
    use hoh_brand::ids::*;
    let ids = [
        serde_json::to_value(brand_string::<ApprovalRequestId>("x")).unwrap(),
        serde_json::to_value(brand_string::<SessionRequestId>("x")).unwrap(),
        serde_json::to_value(brand_string::<FileUploadReceiptId>("x")).unwrap(),
        serde_json::to_value(brand_string::<SessionTitleProviderId>("x")).unwrap(),
    ];
    for id in ids {
        assert_eq!(id, serde_json::json!("x"));
    }
}
