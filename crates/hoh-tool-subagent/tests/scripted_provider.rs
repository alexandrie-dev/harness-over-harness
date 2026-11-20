use hoh_subagent::{SubagentCapabilities, SubagentProvider};

struct ScriptedProvider;

impl SubagentProvider for ScriptedProvider {
    fn name(&self) -> &str {
        "scripted"
    }

    fn capabilities(&self) -> SubagentCapabilities {
        SubagentCapabilities {
            depth_limit: true,
            ..Default::default()
        }
    }

    fn inherits_parent_context(&self) -> bool {
        false
    }

    fn supports_start(&self) -> bool {
        true
    }
}

#[test]
fn scripted_provider_contract_has_stable_name_and_capabilities() {
    let provider = ScriptedProvider;
    assert_eq!(provider.name(), "scripted");
    assert!(provider.supports_start());
    assert!(provider.capabilities().depth_limit);
    assert!(!provider.inherits_parent_context());
}
