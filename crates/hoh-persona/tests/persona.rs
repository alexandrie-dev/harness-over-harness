use hoh_kernel::{Context, Kernel, Service};
use hoh_persona::{PERSONA_PREFIX_SECTION, PERSONA_SUFFIX_SECTION, Persona};
use hoh_scope::{CreateScopeOptions, Scope, ScopeKey, create_scope, scope_of};
use hoh_system_prompt::{
    AssembleContext, AssembledContext, AssembledSection, PromptContext, PromptPresentation,
    PromptSection, SystemPrompt, SystemPromptAssemble, SystemPromptKey, render_prompt,
};
use serde_json::{Value, json};

struct Harness {
    kernel: Kernel,
    ctx: Context,
    prompt: Service<SystemPromptKey>,
}

async fn mount(config: Value) -> Harness {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    ctx.plugin::<SystemPrompt>(config)
        .unwrap()
        .settled()
        .await
        .unwrap();
    let prompt = ctx.get::<SystemPromptKey>().unwrap();
    Harness {
        kernel,
        ctx,
        prompt,
    }
}

/// `harness(deploymentPersona)` of the source.
async fn harness(deployment_persona: &str) -> Harness {
    mount(json!({ "personaPrefix": deployment_persona })).await
}

/// A fresh agent scope (`createScope(ctx, key)`), minted and settled.
async fn new_scope(h: &Harness) -> (Scope, ScopeKey) {
    let scope = create_scope(&h.ctx, ScopeKey::new(), CreateScopeOptions::default()).unwrap();
    scope.ctx.fiber().settled().await.unwrap();
    let key = scope_of(&scope.ctx).unwrap();
    (scope, key)
}

fn scoped(key: &ScopeKey) -> AssembleContext {
    AssembleContext::new(Some(key.clone()))
}

fn global() -> AssembleContext {
    AssembleContext::default()
}

/// The rendered text of the persona slot as one scope sees it.
async fn persona_text(h: &Harness, scope: Option<&ScopeKey>) -> Option<String> {
    let assembly = h
        .prompt
        .assemble(scope.map_or_else(global, scoped))
        .await
        .unwrap();
    assembly
        .sections
        .iter()
        .find(|section| section.name == PERSONA_PREFIX_SECTION)
        .map(|section| section.text.clone())
}

fn rendered(assembly: &hoh_system_prompt::PromptAssembly) -> String {
    render_prompt(assembly).unwrap()
}

fn section(name: &str, order: f64, text: &str) -> PromptSection {
    PromptSection {
        name: name.into(),
        order,
        text: text.into(),
        ..PromptSection::default()
    }
}

#[tokio::test(start_paused = true)]
async fn shadows_and_interpolates_the_environment_per_scope_restoring_both_defaults_on_disposal() {
    let h =
        mount(json!({"personaPrefix": "Deployment.", "personaSuffix": "Global workspace."})).await;
    h.prompt
        .variable(&h.ctx, "cwd", |_| Ok(Some("/local".into())))
        .unwrap();
    h.prompt
        .section(&h.ctx, section("guidance", 100.0, "Use tools."))
        .unwrap();
    let (scope, key) = new_scope(&h).await;
    let fiber = scope
        .ctx
        .plugin::<Persona>(json!({"prefix": "Preset.", "suffix": "Workspace {{cwd}}."}))
        .unwrap();
    fiber.settled().await.unwrap();

    let assembly = h.prompt.assemble(scoped(&key)).await.unwrap();
    assert_eq!(
        assembly
            .sections
            .iter()
            .find(|section| section.name == PERSONA_SUFFIX_SECTION)
            .map(|section| section.text.as_str()),
        Some("Workspace {{cwd}}.")
    );
    assert_eq!(
        rendered(&assembly),
        "You are an AI agent powered by DeepSeek Harness.\n\nPreset.\n\nUse tools.\n\nWorkspace /local."
    );
    assert!(rendered(&h.prompt.assemble(global()).await.unwrap()).contains("Global workspace."));
    fiber.dispose().await;
    let restored = rendered(&h.prompt.assemble(scoped(&key)).await.unwrap());
    assert!(restored.contains("Deployment."));
    assert!(restored.contains("Global workspace."));
    assert!(!restored.contains("Workspace /local."));
    h.kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn shadows_the_default_environment_with_an_omitted_or_empty_value() {
    for environment in [json!({}), json!({"suffix": ""})] {
        let h = mount(json!({"personaSuffix": "Global workspace."})).await;
        let (scope, key) = new_scope(&h).await;
        let mut config = json!({"prefix": "Preset."});
        config
            .as_object_mut()
            .unwrap()
            .extend(environment.as_object().unwrap().clone());
        scope
            .ctx
            .plugin::<Persona>(config)
            .unwrap()
            .settled()
            .await
            .unwrap();
        let assembly = h.prompt.assemble(scoped(&key)).await.unwrap();
        assert!(!rendered(&assembly).contains("Global workspace."));
        assert_eq!(
            assembly
                .sections
                .iter()
                .find(|section| section.name == PERSONA_SUFFIX_SECTION)
                .map(|section| section.text.as_str()),
            Some("")
        );
        h.kernel.shutdown().await;
    }
}

#[tokio::test(start_paused = true)]
async fn rejects_an_unscoped_mount_which_would_collide_with_the_registry_default() {
    let h = harness("deployment identity").await;

    let error = h
        .ctx
        .plugin::<Persona>(json!({"prefix": "composition identity"}))
        .unwrap()
        .settled()
        .await
        .unwrap_err();

    assert!(
        error
            .to_string()
            .contains("\"deployment:persona-prefix\" is already registered"),
        "{error}"
    );
    h.kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn shadows_the_deployment_default_for_one_scope_only() {
    let h = harness("deployment identity").await;
    let (scope, key) = new_scope(&h).await;

    scope
        .ctx
        .plugin::<Persona>(json!({"prefix": "preset identity"}))
        .unwrap()
        .settled()
        .await
        .unwrap();

    assert_eq!(
        persona_text(&h, Some(&key)).await.as_deref(),
        Some("preset identity")
    );
    assert_eq!(
        persona_text(&h, None).await.as_deref(),
        Some("deployment identity")
    );
    h.kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn gives_two_scopes_independent_personas() {
    let h = harness("").await;
    let (first, first_key) = new_scope(&h).await;
    let (second, second_key) = new_scope(&h).await;

    first
        .ctx
        .plugin::<Persona>(json!({"prefix": "first identity"}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    second
        .ctx
        .plugin::<Persona>(json!({"prefix": "second identity"}))
        .unwrap()
        .settled()
        .await
        .unwrap();

    assert_eq!(
        persona_text(&h, Some(&first_key)).await.as_deref(),
        Some("first identity")
    );
    assert_eq!(
        persona_text(&h, Some(&second_key)).await.as_deref(),
        Some("second identity")
    );
    h.kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn shadows_the_deployment_persona_away_entirely_when_its_text_is_empty() {
    let h = harness("deployment identity").await;
    let (scope, key) = new_scope(&h).await;

    scope
        .ctx
        .plugin::<Persona>(json!({"prefix": ""}))
        .unwrap()
        .settled()
        .await
        .unwrap();

    // The slot is still occupied, so the deployment persona is gone for this
    // agent; an empty section is dropped when the prompt renders.
    assert_eq!(persona_text(&h, Some(&key)).await.as_deref(), Some(""));
    assert_eq!(
        persona_text(&h, None).await.as_deref(),
        Some("deployment identity")
    );
    h.kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn restores_the_shadowed_default_when_its_fiber_unloads() {
    let h = harness("deployment identity").await;
    let (scope, key) = new_scope(&h).await;
    let fiber = scope
        .ctx
        .plugin::<Persona>(json!({"prefix": "preset identity"}))
        .unwrap();
    fiber.settled().await.unwrap();
    assert_eq!(
        persona_text(&h, Some(&key)).await.as_deref(),
        Some("preset identity")
    );

    fiber.dispose().await;

    assert_eq!(
        persona_text(&h, Some(&key)).await.as_deref(),
        Some("deployment identity")
    );
    h.kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn interpolates_prompt_variables_strictly_like_any_other_section() {
    let h = harness("").await;
    let (scope, key) = new_scope(&h).await;
    h.prompt
        .variable(&h.ctx, "model", |_| Ok(Some("deepseek-v4-pro".into())))
        .unwrap();

    scope
        .ctx
        .plugin::<Persona>(json!({"prefix": "You run on {{model}}."}))
        .unwrap()
        .settled()
        .await
        .unwrap();

    // `assemble()` keeps section text uninterpolated; `render_prompt()` is the
    // stage that resolves `{{…}}` against the assembly's variables.
    assert_eq!(
        persona_text(&h, Some(&key)).await.as_deref(),
        Some("You run on {{model}}.")
    );
    assert!(
        rendered(&h.prompt.assemble(scoped(&key)).await.unwrap())
            .contains("You run on deepseek-v4-pro.")
    );
    h.kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn makes_a_complete_persona_the_exact_prompt_after_every_other_contribution() {
    let h = harness("deployment identity").await;
    let (scope, key) = new_scope(&h).await;
    h.prompt
        .section(&h.ctx, section("global:extra", 100.0, "global guidance"))
        .unwrap();

    scope
        .ctx
        .plugin::<Persona>(
            json!({"prefix": "Only this.", "suffix": "{{unknown}}", "complete": true}),
        )
        .unwrap()
        .settled()
        .await
        .unwrap();
    scope
        .ctx
        .listen()
        .prepend()
        .on_waterfall::<SystemPromptAssemble, PromptPresentation, _>(|event, next| async move {
            event.update(|assembly| {
                assembly.sections.push(AssembledSection {
                    name: "late:extra".into(),
                    text: "late guidance".into(),
                    interpolate: None,
                });
            });
            next.run().await
        })
        .unwrap();

    let assembly = h.prompt.assemble(scoped(&key)).await.unwrap();
    assert_eq!(
        assembly.sections,
        vec![AssembledSection {
            name: PERSONA_PREFIX_SECTION.into(),
            text: "Only this.".into(),
            interpolate: None,
        }]
    );
    assert_eq!(rendered(&assembly), "Only this.");
    h.kernel.shutdown().await;
}

fn policy() -> AssembledContext {
    AssembledContext {
        name: "policy".into(),
        text: "global policy".into(),
        interpolate: None,
    }
}

fn policy_context() -> PromptContext {
    PromptContext {
        name: "policy".into(),
        order: 1.0,
        text: "global policy".into(),
        ..PromptContext::default()
    }
}

#[tokio::test(start_paused = true)]
async fn can_suppress_runtime_context_for_its_scope_without_changing_the_global_assembly() {
    let h = harness("deployment identity").await;
    let (scope, key) = new_scope(&h).await;
    h.prompt.context(&h.ctx, policy_context()).unwrap();

    let fiber = scope
        .ctx
        .plugin::<Persona>(json!({"prefix": "Only this.", "includeRuntimeContext": false}))
        .unwrap();
    fiber.settled().await.unwrap();
    let suppressed = h.prompt.assemble(scoped(&key)).await.unwrap();
    assert_eq!(suppressed.contexts, vec![]);
    let global = h.prompt.assemble(global()).await.unwrap();
    assert_eq!(global.contexts, vec![policy()]);

    fiber.dispose().await;
    assert_eq!(
        h.prompt.assemble(scoped(&key)).await.unwrap().contexts,
        vec![policy()]
    );
    h.kernel.shutdown().await;
}

/// The source calls `apply` with a config that bypasses the schema defaults. A Rust `Config`
/// has no absent fields, so the equivalent is a config that names only `prefix`: the defaults
/// of the schema fill the rest, and runtime context stays.
#[tokio::test(start_paused = true)]
async fn keeps_runtime_context_by_default_when_apply_bypasses_schema_defaults() {
    let h = harness("deployment identity").await;
    let (scope, key) = new_scope(&h).await;
    h.prompt.context(&h.ctx, policy_context()).unwrap();

    scope
        .ctx
        .plugin::<Persona>(json!({"prefix": "Scoped identity."}))
        .unwrap()
        .settled()
        .await
        .unwrap();

    assert_eq!(
        h.prompt.assemble(scoped(&key)).await.unwrap().contexts,
        vec![policy()]
    );
    h.kernel.shutdown().await;
}
