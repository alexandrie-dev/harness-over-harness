mod common;
use common::*;
#[path = "../../hoh-session-format-v3-to-v4/tests/common/corpus_contract.rs"]
mod contract;
#[test]
fn restores_recorded_parent_child_corpus_through_installed_session_validation() {
    contract::corpus_contract(
        |path| {
            let (header, rows) = contract::fixture_records(path);
            let mut reader = historical_session_format_catalog()
                .unwrap()
                .create_restore(&header, policy(SessionFormatValidation::Current))
                .unwrap();
            for row in rows {
                reader.decode_row(row).unwrap();
            }
            reader.finish().unwrap()
        },
        |parent, children| {
            let facts = children
                .iter()
                .map(|child| historical_child_catalog_source(child).map(Value::Object))
                .collect::<R<Vec<_>>>()?;
            let catalog = create_session_format_catalog_with_children(facts)?;
            let mut physical = obj(json!({"type":"session"}));
            physical.extend(parent.header.clone());
            let mut reader = catalog
                .create_restore(&physical.into(), policy(SessionFormatValidation::Current))?;
            for event in &parent.events {
                reader.decode_row(event.clone().into())?;
            }
            reader.finish()
        },
    );
}
