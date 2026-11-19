//! Scope-aware filesystem guidance: the prompt text follows the tools visible to the agent's scope.

mod support;

use hoh_scope::{CreateScopeOptions, create_scope};
use hoh_system_prompt::{AssembleContext, PromptAssembly, SystemPromptKey, render_prompt};
use hoh_tools::{ToolRegistrationOptions, ToolRestriction};
use serde_json::json;
use support::{TestAgent, setup};

const READ: &str = "Use the read tool — not shell commands like cat, sed, head, or tail — to inspect text files. Use offset and limit to read a line range or continue a large file.";
const WRITE: &str =
    "Read an existing file before overwriting it with write and prefer edit for targeted changes.";
const EDIT: &str =
    "Read a file before editing it unless you just created or edited it in this session.";

fn original(name: &str) -> &'static str {
    match name {
        "read" => READ,
        "write" => WRITE,
        _ => EDIT,
    }
}

/// Preserve the default persona and exact section separators in the oracle.
fn with_persona(sections: &[&str]) -> String {
    let mut all = vec!["You are an AI agent powered by DeepSeek Harness."];
    all.extend_from_slice(sections);
    all.join("\n\n")
}

fn rendered(assembly: &PromptAssembly) -> String {
    render_prompt(assembly).unwrap()
}

#[tokio::test]
async fn preserves_exact_text_for_visible_tools_for_every_subset() {
    for mask in 0..8_u32 {
        let setup = setup(json!({})).await;
        let agent = TestAgent::new();
        let scope = create_scope(
            &setup.ctx,
            agent.key.clone(),
            CreateScopeOptions { parent: None },
        )
        .unwrap();
        let prompt = setup.ctx.get::<SystemPromptKey>().unwrap();
        let names = ["read", "write", "edit"];
        let allow: Vec<&str> = names
            .iter()
            .enumerate()
            .filter(|(index, _)| mask & (1 << index) != 0)
            .map(|(_, name)| *name)
            .collect();
        let baseline = with_persona(&names.map(original));
        assert_eq!(
            rendered(&prompt.assemble(AssembleContext::default()).await.unwrap()),
            baseline,
            "mask {mask}"
        );

        let release = setup
            .tools
            .restrict(
                &scope.ctx,
                ToolRestriction {
                    allow: Some(allow.iter().map(|n| (*n).to_owned()).collect()),
                    deny: None,
                },
            )
            .unwrap();
        let assembly = prompt
            .assemble(AssembleContext::new(Some(agent.key.clone())))
            .await
            .unwrap();
        let mut visible: Vec<String> = assembly
            .tools
            .iter()
            .map(|tool| tool.name.clone())
            .collect();
        visible.sort();
        let mut expected_names: Vec<String> = allow.iter().map(|n| (*n).to_owned()).collect();
        expected_names.sort();
        assert_eq!(visible, expected_names, "mask {mask}");
        let expected: Vec<String> = allow
            .iter()
            .map(|name| {
                if *name == "write" && !allow.contains(&"edit") {
                    WRITE.replace(" and prefer edit for targeted changes", "")
                } else {
                    original(name).to_owned()
                }
            })
            .collect();
        let expected: Vec<&str> = expected.iter().map(String::as_str).collect();
        assert_eq!(rendered(&assembly), with_persona(&expected), "mask {mask}");
        assert_eq!(
            rendered(&prompt.assemble(AssembleContext::default()).await.unwrap()),
            baseline,
            "mask {mask}"
        );
        release.dispose().await;
        assert_eq!(
            rendered(
                &prompt
                    .assemble(AssembleContext::new(Some(agent.key.clone())))
                    .await
                    .unwrap()
            ),
            baseline,
            "mask {mask}"
        );
        scope.dispose().await;
        setup.kernel.shutdown().await;
    }
}

#[tokio::test]
async fn honors_deny_filters_and_the_existing_exemption_for_own_scope_tools() {
    let setup = setup(json!({})).await;
    let agent = TestAgent::new();
    let scope = create_scope(
        &setup.ctx,
        agent.key.clone(),
        CreateScopeOptions { parent: None },
    )
    .unwrap();
    let prompt = setup.ctx.get::<SystemPromptKey>().unwrap();
    let write = setup.tools.get("write", None, None).unwrap();
    setup
        .tools
        .restrict(
            &scope.ctx,
            ToolRestriction {
                allow: None,
                deny: Some(vec!["write".into(), "edit".into()]),
            },
        )
        .unwrap();
    let assembly = prompt
        .assemble(AssembleContext::new(Some(agent.key.clone())))
        .await
        .unwrap();
    assert_eq!(rendered(&assembly), with_persona(&[READ]));
    let denied = support::call(
        &setup.ctx,
        "write",
        json!({ "file_path": "/blocked", "content": "blocked" }),
        Some(agent.clone() as std::sync::Arc<dyn hoh_tools::Agent>),
    )
    .await;
    assert!(denied.is_error());
    assert!(denied.text().contains("unknown tool \"write\""));

    setup
        .tools
        .register(
            &scope.ctx,
            (*write).clone(),
            ToolRegistrationOptions::default(),
        )
        .unwrap();
    let assembly = prompt
        .assemble(AssembleContext::new(Some(agent.key.clone())))
        .await
        .unwrap();
    let mut visible: Vec<String> = assembly
        .tools
        .iter()
        .map(|tool| tool.name.clone())
        .collect();
    visible.sort();
    assert_eq!(visible, ["read", "write"]);
    assert_eq!(
        rendered(&assembly),
        with_persona(&[
            READ,
            &WRITE.replace(" and prefer edit for targeted changes", "")
        ])
    );
    scope.dispose().await;
    setup.kernel.shutdown().await;
}
