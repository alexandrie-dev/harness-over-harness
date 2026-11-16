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
