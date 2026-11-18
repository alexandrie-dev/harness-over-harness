mod common;

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

use common::*;
use hoh_kernel::Disposer;
use hoh_scope::{CreateScopeOptions, Scope, ScopeKey, create_scope, scope_of};
use hoh_system_prompt::{
    AssembleContext, AssembledContext, PromptContext, PromptPresentation, PromptText,
    SystemPromptAssemble, TOOL_ORDER_REST, ToolProviderResult, render_context_snapshot,
    render_prompt,
};
use serde_json::json;

async fn mint(h: &Harness) -> Scope {
    let scope = create_scope(&h.ctx, ScopeKey::new(), CreateScopeOptions::default()).unwrap();
    scope.ctx.fiber().settled().await.unwrap();
    scope
}

/// The key a test scope was minted with.
fn key_of(scope: &Scope) -> ScopeKey {
    scope_of(&scope.ctx).unwrap()
}

fn scoped(scope: &Scope) -> AssembleContext {
    AssembleContext::new(Some(key_of(scope)))
}

fn text(assembly: &hoh_system_prompt::PromptAssembly) -> String {
    render_prompt(assembly).unwrap()
}

#[tokio::test(start_paused = true)]
async fn a_scoped_persona_shadows_deployment_persona_prefix_for_that_scope_only() {
    let h = mount(json!({"personaPrefix": "You are the deployment."})).await;
    let scope = mint(&h).await;
    h.prompt
        .section(
            &scope.ctx,
            section("deployment:persona-prefix", 0.0, "You run tests."),
        )
        .unwrap();

    let scoped_text = text(&h.prompt.assemble(scoped(&scope)).await.unwrap());
    let global = text(&h.prompt.assemble(AssembleContext::default()).await.unwrap());
    assert!(scoped_text.contains("You run tests."));
    assert!(!scoped_text.contains("You are the deployment."));
    assert!(global.contains("You are the deployment."));
    assert!(!global.contains("You run tests."));
    h.kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn scoped_only_sections_join_that_scope_alone_and_disposal_removes_them() {
    let h = mount(json!({})).await;
    let scope = mint(&h).await;
    h.prompt
        .section(&scope.ctx, section("child:extra", 50.0, "Extra guidance."))
        .unwrap();

    assert!(text(&h.prompt.assemble(scoped(&scope)).await.unwrap()).contains("Extra guidance."));
    assert!(
        !text(&h.prompt.assemble(AssembleContext::default()).await.unwrap())
            .contains("Extra guidance.")
    );
    let key = key_of(&scope);
    scope.dispose().await;
    let after = h
        .prompt
        .assemble(AssembleContext::new(Some(key)))
        .await
        .unwrap();
    assert!(!text(&after).contains("Extra guidance."));
    h.kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn duplicate_names_fail_per_layer_naming_agent_ctx_for_the_global_case() {
    let h = mount(json!({})).await;
    let scope = mint(&h).await;
    h.prompt.section(&h.ctx, section("x", 1.0, "a")).unwrap();
    let error = h
        .prompt
        .section(&h.ctx, section("x", 1.0, "b"))
        .err()
        .unwrap();
    assert!(error.to_string().contains("agent.ctx"), "{error}");
    h.prompt
        .section(&scope.ctx, section("y", 1.0, "a"))
        .unwrap();
    let error = h
        .prompt
        .section(&scope.ctx, section("y", 1.0, "b"))
        .err()
        .unwrap();
    assert!(
        error
            .to_string()
            .contains("already registered in this scope"),
        "{error}"
    );
    h.kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn shadows_a_global_section_before_evaluating_either_text_provider() {
    let h = mount(json!({})).await;
    let scope = mint(&h).await;
    let global_calls = Arc::new(AtomicUsize::new(0));
    let scoped_calls = Arc::new(AtomicUsize::new(0));
    let g = global_calls.clone();
    let s = scoped_calls.clone();
    let mut global = section("shared", 1.0, "");
    global.text = PromptText::provider(move |_| {
        g.fetch_add(1, Ordering::SeqCst);
        Ok("global text".into())
    });
    let mut local = section("shared", 1.0, "");
    local.text = PromptText::provider(move |_| {
        s.fetch_add(1, Ordering::SeqCst);
        Ok("scoped text".into())
    });
    h.prompt.section(&h.ctx, global).unwrap();
    h.prompt.section(&scope.ctx, local).unwrap();

    let assembly = h.prompt.assemble(scoped(&scope)).await.unwrap();

    let shared = assembly
        .sections
        .iter()
        .find(|s| s.name == "shared")
        .unwrap();
    assert_eq!(shared.text, "scoped text");
    assert_eq!(global_calls.load(Ordering::SeqCst), 0);
    assert_eq!(scoped_calls.load(Ordering::SeqCst), 1);
    h.kernel.shutdown().await;
}

fn value(
    text: &'static str,
) -> impl Fn(&AssembleContext) -> Result<Option<String>, hoh_error::BoxError> + Send + Sync + 'static
{
    move |_| Ok(Some(text.to_owned()))
}

#[tokio::test(start_paused = true)]
async fn a_scoped_variable_shadows_its_global_name_twin_for_that_scope() {
    let h = mount(json!({"personaPrefix": "Mode: {{mode}}."})).await;
    let scope = mint(&h).await;
    h.prompt.variable(&h.ctx, "mode", value("normal")).unwrap();
    h.prompt
        .variable(&scope.ctx, "mode", value("strict"))
        .unwrap();

    assert!(text(&h.prompt.assemble(scoped(&scope)).await.unwrap()).contains("Mode: strict."));
    assert!(
        text(&h.prompt.assemble(AssembleContext::default()).await.unwrap())
            .contains("Mode: normal.")
    );
    h.kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn same_layer_duplicates_fail_and_the_scoped_layer_cleans_up_on_dispose() {
    let h = mount(json!({})).await;
    let scope = mint(&h).await;
    h.prompt.variable(&scope.ctx, "v", value("1")).unwrap();
    let error = h
        .prompt
        .variable(&scope.ctx, "v", value("2"))
        .err()
        .unwrap();
    assert!(
        error
            .to_string()
            .contains("already registered in this scope"),
        "{error}"
    );
    scope.dispose().await;
    // A scope minted afterwards starts clean.
    let again = mint(&h).await;
    h.prompt.variable(&again.ctx, "v", value("3")).unwrap();
    h.kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn defers_a_scoped_variable_that_replaces_the_last_provider_in_its_generation() {
    let h = mount(json!({"personaPrefix": "Mode: {{mode}}."})).await;
    let scope = mint(&h).await;
    let key = key_of(&scope);
    let calls: Arc<Mutex<Vec<&'static str>>> = Arc::default();
    h.prompt
        .section(&scope.ctx, section("scope:sibling", 1.0, "Scoped."))
        .unwrap();
    let slot: Arc<Mutex<Option<Disposer>>> = Arc::default();
    let provider = {
        let (calls, slot, prompt, scope_ctx) = (
            calls.clone(),
            slot.clone(),
            h.prompt.clone(),
            scope.ctx.clone(),
        );
        move |_: &AssembleContext| {
            calls.lock().unwrap().push("first");
            // `dispose()` removes the registration at once; dropping the future leaves it removed.
            drop(slot.lock().unwrap().take().unwrap().dispose());
            let calls = calls.clone();
            prompt
                .variable(&scope_ctx, "mode", move |_| {
                    calls.lock().unwrap().push("replacement");
                    Ok(Some("replacement".to_owned()))
                })
                .unwrap();
            Ok(Some("first".to_owned()))
        }
    };
    *slot.lock().unwrap() = Some(h.prompt.variable(&scope.ctx, "mode", provider).unwrap());

    let context = AssembleContext::new(Some(key));
    assert!(text(&h.prompt.assemble(context.clone()).await.unwrap()).contains("Mode: first."));
    assert_eq!(*calls.lock().unwrap(), ["first"]);
    assert!(text(&h.prompt.assemble(context).await.unwrap()).contains("Mode: replacement."));
    assert_eq!(*calls.lock().unwrap(), ["first", "replacement"]);
    h.kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn shadows_a_global_context_for_one_scope_and_cleans_up_with_that_scope() {
    let h = mount(json!({})).await;
    let scope = mint(&h).await;
    h.prompt
        .context(&h.ctx, context("policy", 1.0, "global policy"))
        .unwrap();
    h.prompt
        .context(&scope.ctx, context("policy", 1.0, "scoped policy"))
        .unwrap();
    let error = h
        .prompt
        .context(&scope.ctx, context("policy", 2.0, "duplicate"))
        .err()
        .unwrap();
    assert_eq!(
        error.to_string(),
        "prompt context \"policy\" is already registered in this scope"
    );

    let scoped_snapshot =
        render_context_snapshot(&h.prompt.assemble(scoped(&scope)).await.unwrap()).unwrap();
    assert!(scoped_snapshot.contains("scoped policy"));
    let global_snapshot =
        render_context_snapshot(&h.prompt.assemble(AssembleContext::default()).await.unwrap())
            .unwrap();
    assert!(global_snapshot.contains("global policy"));

    let key = key_of(&scope);
    scope.dispose().await;
    let after = h
        .prompt
        .assemble(AssembleContext::new(Some(key)))
        .await
        .unwrap();
    assert!(
        render_context_snapshot(&after)
            .unwrap()
            .contains("global policy")
    );
    h.kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn suppresses_all_context_for_one_scope_and_restores_it_when_disposed() {
    let h = mount(json!({})).await;
    let scope = mint(&h).await;
    h.prompt
        .context(&h.ctx, context("policy", 1.0, "global policy"))
        .unwrap();
    let dispose = h.prompt.suppress_runtime_context(&scope.ctx).unwrap();

    let suppressed = h.prompt.assemble(scoped(&scope)).await.unwrap();
    assert_eq!(suppressed.contexts, Vec::<AssembledContext>::new());
    let global = h.prompt.assemble(AssembleContext::default()).await.unwrap();
    assert!(
        render_context_snapshot(&global)
            .unwrap()
            .contains("global policy")
    );

    dispose.dispose().await;
    let restored = h.prompt.assemble(scoped(&scope)).await.unwrap();
    assert!(
        render_context_snapshot(&restored)
            .unwrap()
            .contains("global policy")
    );
    h.kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn scoped_providers_are_consulted_only_for_their_scope() {
    let h = mount(json!({})).await;
    let scope = mint(&h).await;
    h.prompt.tools(&h.ctx, schemas(&["global_tool"])).unwrap();
    h.prompt
        .tools(&scope.ctx, schemas(&["scoped_tool"]))
        .unwrap();

    let in_scope = h.prompt.assemble(scoped(&scope)).await.unwrap();
    let global = h.prompt.assemble(AssembleContext::default()).await.unwrap();
    assert_eq!(names(&in_scope), ["global_tool", "scoped_tool"]);
    assert_eq!(names(&global), ["global_tool"]);
    h.kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn disposing_a_scoped_tool_provider_empties_its_layer_without_residue() {
    let h = mount(json!({})).await;
    let scope = mint(&h).await;
    let dispose = h
        .prompt
        .tools(&scope.ctx, schemas(&["scoped_tool"]))
        .unwrap();
    dispose.dispose().await;
    let after = h.prompt.assemble(scoped(&scope)).await.unwrap();
    assert_eq!(names(&after), Vec::<String>::new());
    // Re-registering through the same scope starts a fresh layer.
    h.prompt.tools(&scope.ctx, schemas(&["again"])).unwrap();
    let again = h.prompt.assemble(scoped(&scope)).await.unwrap();
    assert_eq!(names(&again), ["again"]);
    h.kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn a_tool_order_entry_restricted_away_for_a_scope_is_a_normal_absence_while_a_typo_still_fails()
 {
    let h = mount(json!({"toolOrder": ["bash", TOOL_ORDER_REST]})).await;
    // A provider mimicking the registry's restriction split: bash exists
    // (known names) but is masked for this assembly (schemas).
    h.prompt
        .tools(&h.ctx, |_| {
            Ok(ToolProviderResult {
                schemas: vec![tool("read", "read")],
                known_names: Some(vec!["read".into(), "bash".into()]),
            })
        })
        .unwrap();
    let assembly = h.prompt.assemble(AssembleContext::default()).await.unwrap();
    assert_eq!(names(&assembly), ["read"]);

    let bad = mount(json!({"toolOrder": ["basj", TOOL_ORDER_REST]})).await;
    bad.prompt
        .tools(&bad.ctx, |_| {
            Ok(ToolProviderResult {
                schemas: vec![tool("read", "read")],
                known_names: Some(vec!["read".into(), "bash".into()]),
            })
        })
        .unwrap();
    let error = bad
        .prompt
        .assemble(AssembleContext::default())
        .await
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("toolOrder lists unregistered tool \"basj\"; known tools: bash, read"),
        "{error}"
    );
    h.kernel.shutdown().await;
    bad.kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn refreshes_through_the_same_scoped_providers_and_suppression_as_assembly() {
    let h = mount(json!({})).await;
    let scope = mint(&h).await;
    let current = Arc::new(Mutex::new("before".to_owned()));
    h.prompt
        .context(&h.ctx, context("fact", 0.0, "global"))
        .unwrap();
    let live = current.clone();
    h.prompt
        .context(
            &scope.ctx,
            PromptContext {
                text: text_of(move || live.lock().unwrap().clone()),
                required: true,
                ..context("fact", 0.0, "")
            },
        )
        .unwrap();
    h.prompt
        .context(&scope.ctx, context("optional", 10.0, "optional"))
        .unwrap();
    let context = scoped(&scope);
    let accepted = h.prompt.assemble(context.clone()).await.unwrap();
    *current.lock().unwrap() = "after".into();
    let restore = h.prompt.suppress_runtime_context(&scope.ctx).unwrap();
    assert_eq!(
        h.prompt
            .refresh_context(&accepted, &context)
            .unwrap()
            .contexts,
        [assembled("fact", "after")]
    );
    restore.dispose().await;
    let global = h.prompt.assemble(AssembleContext::default()).await.unwrap();
    assert_eq!(
        h.prompt
            .refresh_context(&global, &AssembleContext::default())
            .unwrap()
            .contexts,
        [assembled("fact", "global")]
    );
    h.kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn an_agent_ctx_assemble_listener_shapes_only_its_own_scopes_assemblies() {
    let h = mount(json!({})).await;
    let scope = mint(&h).await;
    let shaped: Arc<Mutex<Vec<Option<ScopeKey>>>> = Arc::default();
    let seen = shaped.clone();
    scope
        .ctx
        .on_waterfall::<SystemPromptAssemble, PromptPresentation, _>(move |event, next| {
            let seen = seen.clone();
            async move {
                seen.lock().unwrap().push(event.context.scope.clone());
                let result = next.run().await?;
                result.update(|a| {
                    a.sections.push(hoh_system_prompt::AssembledSection {
                        name: "listener:extra".into(),
                        text: "listener text".into(),
                        interpolate: None,
                    })
                });
                Ok(result)
            }
        })
        .unwrap();

    let in_scope = h.prompt.assemble(scoped(&scope)).await.unwrap();
    let global = h.prompt.assemble(AssembleContext::default()).await.unwrap();
    assert!(in_scope.sections.iter().any(|s| s.name == "listener:extra"));
    assert!(!global.sections.iter().any(|s| s.name == "listener:extra"));
    assert_eq!(shaped.lock().unwrap().len(), 1);
    h.kernel.shutdown().await;
}
