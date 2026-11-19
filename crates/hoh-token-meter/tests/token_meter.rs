use hoh_llm::Message;
use hoh_token_meter::{client, estimate};
use serde_json::json;

#[test]
fn public_client_and_estimate_entrypoints_match_the_package_exports() {
    let message: Message = serde_json::from_value(json!({
        "role":"user","content":[{"type":"text","text":"entrypoint"}],
        "source":{"kind":"user"},"id":"u"
    }))
    .unwrap();
    assert_eq!(estimate::estimate_message(&message), 11);
    let _: Option<client::TurnTokenUsage> = None;
}
