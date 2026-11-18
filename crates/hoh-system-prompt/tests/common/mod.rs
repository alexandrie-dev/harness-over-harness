#![allow(dead_code)]

use std::sync::Arc;

use hoh_error::BoxError;
use hoh_kernel::{Context, Kernel, Service};
use hoh_llm::ToolSchema;
use hoh_system_prompt::{
    AssembledSection, PromptAssembly, PromptContext, PromptSection, PromptText, SystemPrompt,
    SystemPromptKey, ToolProviderResult,
};
use serde_json::{Value, json};

pub const IDENTITY: &str = "You are an AI agent powered by DeepSeek Harness.";

/// Every assembly carries the plugin's own built-ins; mechanics tests strip them.
pub const BUILT_IN: [&str; 3] = [
    "harness:identity",
    "deployment:persona-prefix",
    "deployment:persona-suffix",
];

pub struct Harness {
    pub kernel: Kernel,
    pub ctx: Context,
    pub prompt: Service<SystemPromptKey>,
}

pub async fn mount(config: Value) -> Harness {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let fiber = ctx.plugin::<SystemPrompt>(config).unwrap();
    fiber.settled().await.unwrap();
    let prompt = ctx.get::<SystemPromptKey>().unwrap();
    Harness {
        kernel,
        ctx,
        prompt,
    }
}

pub fn section(name: &str, order: f64, text: &str) -> PromptSection {
    PromptSection {
        name: name.into(),
        order,
        text: text.into(),
        ..PromptSection::default()
    }
}

pub fn context(name: &str, order: f64, text: &str) -> PromptContext {
    PromptContext {
        name: name.into(),
        order,
        text: text.into(),
        ..PromptContext::default()
    }
}

pub fn text_of(f: impl Fn() -> String + Send + Sync + 'static) -> PromptText {
    PromptText::provider(move |_| Ok(f()))
}

pub fn tool(name: &str, description: &str) -> ToolSchema {
    ToolSchema {
        name: name.into(),
        description: description.into(),
        parameters: json!({"type": "object", "properties": {}})
            .as_object()
            .unwrap()
            .clone(),
        defer_loading: None,
    }
}

pub fn schemas(
    names: &[&str],
) -> impl Fn(&hoh_system_prompt::AssembleContext) -> Result<ToolProviderResult, BoxError>
+ Send
+ Sync
+ 'static {
    let tools: Vec<ToolSchema> = names.iter().map(|name| tool(name, name)).collect();
    move |_| {
        Ok(ToolProviderResult {
            schemas: tools.clone(),
            known_names: None,
        })
    }
}

pub fn names(assembly: &PromptAssembly) -> Vec<String> {
    assembly
        .tools
        .iter()
        .map(|tool| tool.name.clone())
        .collect()
}

pub fn contributed(assembly: &PromptAssembly) -> Vec<AssembledSection> {
    assembly
        .sections
        .iter()
        .filter(|section| !BUILT_IN.contains(&section.name.as_str()))
        .cloned()
        .collect()
}

pub fn section_names(assembly: &PromptAssembly) -> Vec<String> {
    assembly.sections.iter().map(|s| s.name.clone()).collect()
}

pub fn assembled(name: &str, text: &str) -> hoh_system_prompt::AssembledContext {
    hoh_system_prompt::AssembledContext {
        name: name.into(),
        text: text.into(),
        interpolate: None,
    }
}

pub fn arc<T>(value: T) -> Arc<T> {
    Arc::new(value)
}
