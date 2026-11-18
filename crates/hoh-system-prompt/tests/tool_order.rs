mod common;

use common::*;
use hoh_kernel::Kernel;
use hoh_system_prompt::{
    AssembleContext, Config, PromptPresentation, SystemPrompt, SystemPromptAssemble,
    TOOL_ORDER_REST, ToolProviderResult,
};
use serde_json::{Value, json};

fn order(entries: &[&str]) -> Value {
    json!({ "toolOrder": entries })
}

#[test]
fn exports_the_rest_entry_as_unlisted_tools() {
    // The ONE place the public constant's value is pinned.
    assert_eq!(TOOL_ORDER_REST, "<unlisted-tools>");
}

#[tokio::test(start_paused = true)]
async fn assembles_tools_in_lexicographic_name_order_when_no_tool_order_is_configured() {
    let h = mount(json!({})).await;
    h.prompt
        .tools(&h.ctx, schemas(&["charlie", "alpha"]))
        .unwrap();
    h.prompt.tools(&h.ctx, schemas(&["bravo"])).unwrap();
    let assembly = h.prompt.assemble(AssembleContext::default()).await.unwrap();
    assert_eq!(names(&assembly), ["alpha", "bravo", "charlie"]);
    h.kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn assembles_the_same_order_regardless_of_provider_registration_order() {
    let forward = mount(json!({})).await;
    forward
        .prompt
        .tools(&forward.ctx, schemas(&["alpha"]))
        .unwrap();
    forward
        .prompt
        .tools(&forward.ctx, schemas(&["zulu"]))
        .unwrap();
    let backward = mount(json!({})).await;
    backward
        .prompt
        .tools(&backward.ctx, schemas(&["zulu"]))
        .unwrap();
    backward
        .prompt
        .tools(&backward.ctx, schemas(&["alpha"]))
        .unwrap();
    let a = forward
        .prompt
        .assemble(AssembleContext::default())
        .await
        .unwrap();
    let b = backward
        .prompt
        .assemble(AssembleContext::default())
        .await
        .unwrap();
    assert_eq!(names(&a), ["alpha", "zulu"]);
    assert_eq!(names(&b), ["alpha", "zulu"]);
    forward.kernel.shutdown().await;
    backward.kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn applies_a_configured_tool_order_listed_positions_rest_at_the_rest_entry_lexicographically()
{
    let h = mount(order(&["todo_write", TOOL_ORDER_REST, "bash"])).await;
    h.prompt
        .tools(&h.ctx, schemas(&["bash", "echo_b", "todo_write", "echo_a"]))
        .unwrap();
    let assembly = h.prompt.assemble(AssembleContext::default()).await.unwrap();
    assert_eq!(names(&assembly), ["todo_write", "echo_a", "echo_b", "bash"]);
    h.kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn rejects_the_assembly_when_tool_order_names_a_tool_that_is_not_registered() {
    let h = mount(order(&["todo_write", "ghost", TOOL_ORDER_REST, "wraith"])).await;
    h.prompt
        .tools(&h.ctx, schemas(&["bash", "todo_write"]))
        .unwrap();
    let error = h
        .prompt
        .assemble(AssembleContext::default())
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains(
            "toolOrder lists unregistered tools \"ghost\", \"wraith\"; known tools: bash, todo_write"
        ),
        "{error}"
    );
    h.kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn names_the_single_unregistered_tool_when_no_tools_are_registered_at_all() {
    let h = mount(order(&["ghost", TOOL_ORDER_REST])).await;
    let error = h
        .prompt
        .assemble(AssembleContext::default())
        .await
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("toolOrder lists unregistered tool \"ghost\"; known tools: (none)"),
        "{error}"
    );
    h.kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn rejects_a_provider_tool_named_like_the_reserved_rest_entry() {
    for (case, config) in [
        ("without an explicit toolOrder", json!({})),
        (
            "with only the rest entry configured",
            order(&[TOOL_ORDER_REST]),
        ),
    ] {
        let h = mount(config).await;
        h.prompt.tools(&h.ctx, schemas(&[TOOL_ORDER_REST])).unwrap();
        let error = h
            .prompt
            .assemble(AssembleContext::default())
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains(&format!(
                "tool provider returned reserved tool name \"{TOOL_ORDER_REST}\""
            )),
            "{case}: {error}"
        );
        h.kernel.shutdown().await;
    }
}

#[tokio::test(start_paused = true)]
async fn keeps_collection_order_between_tools_that_share_a_name_stable_sort() {
    let h = mount(json!({})).await;
    h.prompt
        .tools(&h.ctx, |_| {
            Ok(ToolProviderResult {
                schemas: vec![
                    tool("dup", "first"),
                    tool("anchor", "anchor"),
                    tool("dup", "second"),
                ],
                known_names: None,
            })
        })
        .unwrap();
    let assembly = h.prompt.assemble(AssembleContext::default()).await.unwrap();
    let descriptions: Vec<_> = assembly
        .tools
        .iter()
        .map(|t| t.description.clone())
        .collect();
    assert_eq!(descriptions, ["anchor", "first", "second"]);
    h.kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn keeps_ordered_provider_declarations_outside_presentation_transforms() {
    let h = mount(json!({})).await;
    h.prompt.tools(&h.ctx, schemas(&["zulu", "alpha"])).unwrap();
    // The presentation carries no `tools` (the source asserts `not.toHaveProperty('tools')`;
    // here the type has no such field), and a listener cannot replace the declarations.
    h.ctx
        .on_waterfall::<SystemPromptAssemble, PromptPresentation, _>(|_event, next| async move {
            next.run().await
        })
        .unwrap();
    let assembly = h.prompt.assemble(AssembleContext::default()).await.unwrap();
    assert_eq!(names(&assembly), ["alpha", "zulu"]);
    h.kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn rejects_a_tool_order_without_the_rest_entry_at_load() {
    for (case, config) in [
        ("an empty list", order(&[])),
        (
            "a list without the rest entry",
            order(&["bash", "todo_write"]),
        ),
    ] {
        let kernel = Kernel::start();
        let fiber = kernel.root().plugin::<SystemPrompt>(config).unwrap();
        let error = fiber.settled().await.unwrap_err();
        assert!(
            error.to_string().contains(&format!(
                "must contain the \"{TOOL_ORDER_REST}\" rest entry"
            )),
            "{case}: {error}"
        );
        kernel.shutdown().await;
    }
}

#[tokio::test(start_paused = true)]
async fn rejects_duplicates_in_the_tool_order_at_load() {
    for (case, config) in [
        (
            "a duplicate tool name",
            order(&["bash", "bash", TOOL_ORDER_REST]),
        ),
        (
            "a duplicate rest entry",
            order(&[TOOL_ORDER_REST, "bash", TOOL_ORDER_REST]),
        ),
    ] {
        let kernel = Kernel::start();
        let fiber = kernel.root().plugin::<SystemPrompt>(config).unwrap();
        let error = fiber.settled().await.unwrap_err();
        assert!(
            error.to_string().contains("more than once"),
            "{case}: {error}"
        );
        kernel.shutdown().await;
    }
}

#[tokio::test(start_paused = true)]
async fn throws_from_direct_construction_too() {
    let kernel = Kernel::start();
    let config = Config {
        tool_order: Some(Some(vec!["bash".into()])),
        ..Config::default()
    };
    let error = SystemPrompt::new(kernel.root(), &config).err().unwrap();
    assert!(error.to_string().contains("rest entry"), "{error}");
    kernel.shutdown().await;
}
