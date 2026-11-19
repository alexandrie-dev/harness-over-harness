use crate::types::DeepSeekCatalogModel;

pub const OFFICIAL_CONTEXT_WINDOW: u64 = 1_048_576;

pub fn default_models() -> Vec<DeepSeekCatalogModel> {
    vec![
        DeepSeekCatalogModel {
            id: "deepseek-flash".into(),
            name: Some("DeepSeek-V4.1-Flash".into()),
            context_window: Some(OFFICIAL_CONTEXT_WINDOW),
            input_modalities: Some(vec!["text".into(), "image".into()]),
            system_prompt_update: Some("in-history".into()),
            tool_update: Some("addition-only".into()),
            ..Default::default()
        },
        DeepSeekCatalogModel {
            id: "deepseek-v4-pro".into(),
            name: Some("DeepSeek-V4-Pro".into()),
            description: Some("Stronger agentic coding, knowledge, and difficult reasoning; suited to complex or quality-critical tasks at higher cost.".into()),
            context_window: Some(OFFICIAL_CONTEXT_WINDOW),
            ..Default::default()
        },
    ]
}
