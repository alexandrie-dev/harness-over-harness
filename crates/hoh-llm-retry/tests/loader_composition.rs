use hoh_kernel::{PluginDef, ServiceKey};
use hoh_llm_retry::LlmRetryPlugin;

#[test]
fn loads_provider_supplied_policy_executor_with_the_shipping_plugin_contract() {
    let definition = PluginDef::of::<LlmRetryPlugin>();
    assert_eq!(definition.name(), "llm-retry");
    assert_eq!(
        definition
            .deps()
            .iter()
            .map(|dependency| dependency.name)
            .collect::<Vec<_>>(),
        [
            hoh_agent::Agents::NAME,
            hoh_session_projection::SessionProjections::NAME,
        ]
    );
    assert_eq!(definition.schema()["type"], "object");
}
