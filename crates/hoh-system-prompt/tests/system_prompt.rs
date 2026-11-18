mod common;

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

use common::*;
use hoh_error::BoxError;
use hoh_kernel::{Cfg, Context, Kernel, NoConfig, Plugin};
use hoh_llm::{ContextSnapshotSection, ToolSchema};
use hoh_system_prompt::{
    AssembleContext, AssembledSection, Config, PERSONA_PREFIX_SECTION, PromptAssembly,
    PromptContext, PromptContextOrderName, PromptPresentation, PromptSection,
    PromptSectionOrderName as S, PromptText, SystemPrompt, SystemPromptAssemble,
    SystemPromptChange, SystemPromptKey, ToolProviderResult, Variables, join_context_sections,
    render_context_sections, render_context_snapshot, render_prompt,
};
use indexmap::IndexMap;
use serde_json::{Value, json};

/// (placement, the name the source's tests use for it as section name and text).
const SECTION_ORDER_NAMES: [(S, &str); 33] = [
    (S::HarnessIdentity, "HARNESS_IDENTITY"),
    (S::DeploymentPersonaPrefix, "DEPLOYMENT_PERSONA_PREFIX"),
    (S::PlanPolicy, "PLAN_POLICY"),
    (S::TeamPolicy, "TEAM_POLICY"),
    (S::PtcOnly, "PTC_ONLY"),
    (S::FileReference, "FILE_REFERENCE"),
    (S::ToolBash, "TOOL_BASH"),
    (S::ToolPwsh, "TOOL_PWSH"),
    (S::ToolRead, "TOOL_READ"),
    (S::ToolWrite, "TOOL_WRITE"),
    (S::ToolEdit, "TOOL_EDIT"),
    (S::ToolApplyPatch, "TOOL_APPLY_PATCH"),
    (S::ToolGlob, "TOOL_GLOB"),
    (S::ToolGrep, "TOOL_GREP"),
    (S::ToolJobs, "TOOL_JOBS"),
    (S::ToolPty, "TOOL_PTY"),
    (S::ToolPtyPanel, "TOOL_PTY_PANEL"),
    (S::ToolWebSearch, "TOOL_WEB_SEARCH"),
    (S::ToolWebFetch, "TOOL_WEB_FETCH"),
    (S::ToolLsp, "TOOL_LSP"),
    (S::ToolSessionQuery, "TOOL_SESSION_QUERY"),
    (S::ToolGoal, "TOOL_GOAL"),
    (S::ToolSchedule, "TOOL_SCHEDULE"),
    (S::ToolWorkflow, "TOOL_WORKFLOW"),
    (S::ToolRalph, "TOOL_RALPH"),
    (S::ToolSubagent, "TOOL_SUBAGENT"),
    (S::ToolReport, "TOOL_REPORT"),
    (S::ToolsSdk, "TOOLS_SDK"),
    (S::DeliverableFileReferences, "DELIVERABLE_FILE_REFERENCES"),
    (S::StructuredOutput, "STRUCTURED_OUTPUT"),
    (S::HarnessSource, "HARNESS_SOURCE"),
    (S::WebSurface, "WEB_SURFACE"),
    (S::DeploymentPersonaSuffix, "DEPLOYMENT_PERSONA_SUFFIX"),
];

const CONTEXT_ORDER_NAMES: [PromptContextOrderName; 6] = [
    PromptContextOrderName::WorkingDirectory,
    PromptContextOrderName::SandboxPolicy,
    PromptContextOrderName::ApprovalPolicy,
    PromptContextOrderName::SubagentDelegation,
    PromptContextOrderName::HarnessSource,
    PromptContextOrderName::WebSurface,
];

const SNAPSHOT_HEAD: &str =
    "Current runtime context. This snapshot supersedes earlier runtime-context snapshots.";

fn plain() -> AssembleContext {
    AssembleContext::default()
}

fn rendered(assembly: &PromptAssembly) -> String {
    render_prompt(assembly).unwrap()
}

type Provider = Box<dyn Fn() -> String + Send + Sync>;
fn live(current: &Arc<Mutex<String>>) -> Provider {
    let current = current.clone();
    Box::new(move || current.lock().unwrap().clone())
}

/// A listener that adds a late context to the assembly, then delegates.
fn push_context_listener(h: &Harness, item: AssembledContextSpec) {
    h.ctx
        .on_waterfall::<SystemPromptAssemble, PromptPresentation, _>(
            move |event, next| async move {
                event.update(|assembly| {
                    assembly.contexts.push(hoh_system_prompt::AssembledContext {
                        name: item.0.into(),
                        text: item.1.into(),
                        interpolate: None,
                    })
                });
                next.run().await
            },
        )
        .unwrap();
}
type AssembledContextSpec = (&'static str, &'static str);

fn presentation_section(name: &str, text: &str) -> AssembledSection {
    AssembledSection {
        name: name.into(),
        text: text.into(),
        interpolate: None,
    }
}

fn only_sections(sections: Vec<AssembledSection>) -> PromptAssembly {
    PromptAssembly {
        sections,
        ..PromptAssembly::default()
    }
}

fn with_variables(
    sections: Vec<AssembledSection>,
    variables: &[(&str, Option<&str>)],
) -> PromptAssembly {
    PromptAssembly {
        sections,
        variables: variables
            .iter()
            .map(|(name, value)| ((*name).to_owned(), value.map(str::to_owned)))
            .collect::<Variables>(),
        ..PromptAssembly::default()
    }
}

struct Who(String);
fn who(name: &str) -> AssembleContext {
    AssembleContext::default().with_extension(Who(name.into()))
}

/// A plugin whose contributions belong to its own fiber (the source's anonymous plugin with
/// `inject: ['systemPrompt']`).
struct Contributor;
impl Plugin for Contributor {
    const NAME: &'static str = "Contributor";
    type Config = NoConfig;
    type Deps = ();
    async fn apply(ctx: Context, _deps: (), _config: Cfg<NoConfig>) -> Result<(), BoxError> {
        let prompt = ctx.get::<SystemPromptKey>().unwrap();
        prompt.section(&ctx, section("scoped", 0.0, "scoped section"))?;
        prompt.context(&ctx, context("scoped-context", 0.0, "scoped context"))?;
        prompt.tools(&ctx, schemas(&["scoped-tool"]))?;
        prompt.variable(&ctx, "scoped_var", |_| Ok(Some("v".to_owned())))?;
        Ok(())
    }
}

struct ToolContributor;
impl Plugin for ToolContributor {
    const NAME: &'static str = "ToolContributor";
    type Config = NoConfig;
    type Deps = ();
    async fn apply(ctx: Context, _deps: (), _config: Cfg<NoConfig>) -> Result<(), BoxError> {
        ctx.get::<SystemPromptKey>()
            .unwrap()
            .tools(&ctx, schemas(&["fiber-tool"]))?;
        Ok(())
    }
}

#[tokio::test(start_paused = true)]
async fn retains_required_user_context_when_optional_context_is_suppressed() {
    let h = mount(json!({"includeRuntimeContext": false})).await;
    h.prompt
        .context(
            &h.ctx,
            PromptContext {
                required: true,
                ..context("directory", 0.0, "Directory: /project")
            },
        )
        .unwrap();
    h.prompt
        .context(
            &h.ctx,
            PromptContext {
                text: PromptText::provider(|_| Err("suppressed provider ran".into())),
                ..context("optional", 1.0, "")
            },
        )
        .unwrap();
    push_context_listener(&h, ("unregistered", "should stay suppressed"));
    let assembly = h.prompt.assemble(plain()).await.unwrap();
    assert!(
        render_context_snapshot(&assembly)
            .unwrap()
            .contains("Directory: /project")
    );
    assert_eq!(
        assembly.contexts,
        [assembled("directory", "Directory: /project")]
    );
    assert!(!rendered(&assembly).contains("Directory: /project"));
    h.kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn refreshes_registered_context_without_reassembling_accepted_sections_schemas_variables_or_expert_additions()
 {
    let h = mount(json!({})).await;
    let current = Arc::new(Mutex::new("before".to_owned()));
    let assemblies = Arc::new(AtomicUsize::new(0));
    let tools = Arc::new(AtomicUsize::new(0));
    let variables = Arc::new(AtomicUsize::new(0));
    h.prompt
        .context(
            &h.ctx,
            PromptContext {
                text: text_of({
                    let read = live(&current);
                    move || read()
                }),
                interpolate: Some(false),
                ..context("live", 0.0, "")
            },
        )
        .unwrap();
    let (counter, read) = (variables.clone(), current.clone());
    h.prompt
        .variable(&h.ctx, "fixed", move |_| {
            counter.fetch_add(1, Ordering::SeqCst);
            Ok(Some(read.lock().unwrap().clone()))
        })
        .unwrap();
    let (counter, read) = (tools.clone(), current.clone());
    h.prompt
        .tools(&h.ctx, move |_| {
            counter.fetch_add(1, Ordering::SeqCst);
            Ok(ToolProviderResult {
                schemas: vec![ToolSchema {
                    description: read.lock().unwrap().clone(),
                    defer_loading: Some(true),
                    ..tool("accepted_tool", "")
                }],
                known_names: None,
            })
        })
        .unwrap();
    let counter = assemblies.clone();
    h.ctx
        .on_waterfall::<SystemPromptAssemble, PromptPresentation, _>(move |_event, next| {
            let counter = counter.clone();
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                let accepted = next.run().await?;
                accepted.update(|a| {
                    a.contexts.push(hoh_system_prompt::AssembledContext {
                        name: "expert".into(),
                        text: "accepted only".into(),
                        interpolate: None,
                    })
                });
                Ok(accepted)
            }
        })
        .unwrap();
    let accepted = h.prompt.assemble(plain()).await.unwrap();
    *current.lock().unwrap() = "after".into();
    let refreshed = h.prompt.refresh_context(&accepted, &plain()).unwrap();
    assert_eq!(
        refreshed.contexts,
        [
            hoh_system_prompt::AssembledContext {
                name: "live".into(),
                text: "after".into(),
                interpolate: Some(false)
            },
            assembled("expert", "accepted only"),
        ]
    );
    assert_eq!(accepted.contexts[0].text, "before");
    assert_eq!(refreshed.sections, accepted.sections);
    assert_eq!(refreshed.tools, accepted.tools);
    assert_eq!(refreshed.tools[0].description, "before");
    assert_eq!(refreshed.variables, accepted.variables);
    assert_eq!(
        (
            assemblies.load(Ordering::SeqCst),
            tools.load(Ordering::SeqCst),
            variables.load(Ordering::SeqCst)
        ),
        (1, 1, 1)
    );
    h.kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn restores_missing_required_contexts_before_a_later_registered_context() {
    for later in [false, true] {
        let h = mount(json!({})).await;
        h.prompt
            .context(
                &h.ctx,
                PromptContext {
                    required: true,
                    ..context("required", 0.0, "fresh")
                },
            )
            .unwrap();
        h.prompt
            .context(&h.ctx, context("later", 10.0, "last"))
            .unwrap();
        let mut accepted = h.prompt.assemble(plain()).await.unwrap();
        accepted.contexts = if later {
            vec![assembled("expert", "extra"), accepted.contexts[1].clone()]
        } else {
            vec![]
        };
        let expected = if later {
            vec![
                assembled("expert", "extra"),
                assembled("required", "fresh"),
                assembled("later", "last"),
            ]
        } else {
            vec![assembled("required", "fresh")]
        };
        assert_eq!(
            h.prompt
                .refresh_context(&accepted, &plain())
                .unwrap()
                .contexts,
            expected,
            "later={later}"
        );
        h.kernel.shutdown().await;
    }
}

#[tokio::test(start_paused = true)]
async fn applies_current_suppression_without_evaluating_optional_providers_or_keeping_expert_additions()
 {
    let h = mount(json!({})).await;
    let suppressed = Arc::new(AtomicBool::new(false));
    h.prompt
        .context(
            &h.ctx,
            PromptContext {
                text: PromptText::provider(|_| Ok("current".into())),
                required: true,
                ..context("required", 0.0, "")
            },
        )
        .unwrap();
    let flag = suppressed.clone();
    h.prompt
        .context(
            &h.ctx,
            PromptContext {
                text: PromptText::provider(move |_| {
                    if flag.load(Ordering::SeqCst) {
                        return Err("suppressed context was evaluated".into());
                    }
                    Ok("optional".into())
                }),
                ..context("optional", 10.0, "")
            },
        )
        .unwrap();
    let mut accepted = h.prompt.assemble(plain()).await.unwrap();
    accepted
        .contexts
        .push(assembled("expert", "accepted optional"));
    let restore = h.prompt.suppress_runtime_context(&h.ctx).unwrap();
    suppressed.store(true, Ordering::SeqCst);
    assert_eq!(
        h.prompt
            .refresh_context(&accepted, &plain())
            .unwrap()
            .contexts,
        [assembled("required", "current")]
    );
    restore.dispose().await;
    h.kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn preserves_literal_context_in_both_the_joined_snapshot_and_attributed_sections() {
    let h = mount(json!({})).await;
    h.prompt
        .variable(&h.ctx, "project", |_| Ok(Some("expanded".into())))
        .unwrap();
    let literal = "/workspace/{{project}}/{{unregistered}}/{{malformed-name}}";
    h.prompt
        .context(
            &h.ctx,
            PromptContext {
                interpolate: Some(false),
                ..context("literal", 0.0, literal)
            },
        )
        .unwrap();
    h.prompt
        .context(
            &h.ctx,
            PromptContext {
                interpolate: Some(true),
                ..context("template", 1.0, "{{project}}")
            },
        )
        .unwrap();
    h.prompt
        .context(&h.ctx, context("default", 2.0, "{{project}}"))
        .unwrap();
    let assembly = h.prompt.assemble(plain()).await.unwrap();
    let item = |name: &str, text: &str, interpolate| hoh_system_prompt::AssembledContext {
        name: name.into(),
        text: text.into(),
        interpolate,
    };
    assert_eq!(
        assembly.contexts,
        [
            item("literal", literal, Some(false)),
            item("template", "{{project}}", Some(true)),
            item("default", "{{project}}", None),
        ]
    );
    let section = |name: &str, text: &str| ContextSnapshotSection {
        name: name.into(),
        text: text.into(),
    };
    assert_eq!(
        render_context_sections(&assembly).unwrap(),
        [
            section("literal", literal),
            section("template", "expanded"),
            section("default", "expanded")
        ]
    );
    assert_eq!(
        render_context_snapshot(&assembly).unwrap(),
        format!("{SNAPSHOT_HEAD}\n\n{literal}\n\nexpanded\n\nexpanded")
    );
    h.kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn keeps_repository_section_placements_unique_integral_and_at_least_ten_apart() {
    let h = mount(json!({})).await;
    let orders: Vec<f64> = SECTION_ORDER_NAMES
        .iter()
        .map(|(name, _)| SystemPrompt::get_section_order(*name))
        .collect();
    assert!(orders.iter().all(|order| order.fract() == 0.0));
    let mut sorted = orders.clone();
    sorted.sort_by(f64::total_cmp);
    sorted.dedup();
    assert_eq!(sorted.len(), orders.len());
    assert!(sorted.windows(2).all(|pair| pair[1] - pair[0] >= 10.0));
    h.kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn keeps_reusable_instructions_identical_across_local_environments() {
    let h = mount(json!({"personaPrefix": "Model {{model}}.", "personaSuffix": "In {{cwd}} on {{platform}}."})).await;
    #[derive(Clone)]
    struct Env {
        model: &'static str,
        cwd: &'static str,
        platform: &'static str,
        source: &'static str,
        url: &'static str,
    }
    let environment = Arc::new(Mutex::new(Env {
        model: "model-a",
        cwd: "/alice/project",
        platform: "darwin",
        source: "/alice/dsh",
        url: "http://127.0.0.1:3080",
    }));
    for (key, read) in [
        ("model", (|e: &Env| e.model) as fn(&Env) -> &'static str),
        ("cwd", |e| e.cwd),
        ("platform", |e| e.platform),
    ] {
        let environment = environment.clone();
        h.prompt
            .variable(&h.ctx, key, move |_| {
                Ok(Some(read(&environment.lock().unwrap()).to_owned()))
            })
            .unwrap();
    }
    let reusable: Vec<&(S, &str)> = SECTION_ORDER_NAMES
        .iter()
        .filter(|(name, _)| {
            ![
                S::HarnessIdentity,
                S::DeploymentPersonaPrefix,
                S::HarnessSource,
                S::WebSurface,
                S::DeploymentPersonaSuffix,
            ]
            .contains(name)
        })
        .collect();
    for (name, label) in reusable.iter().rev() {
        h.prompt
            .section(
                &h.ctx,
                section(label, SystemPrompt::get_section_order(*name), label),
            )
            .unwrap();
    }
    let read = environment.clone();
    h.prompt
        .section(
            &h.ctx,
            PromptSection {
                text: text_of(move || read.lock().unwrap().source.to_owned()),
                ..section(
                    "source",
                    SystemPrompt::get_section_order(S::HarnessSource),
                    "",
                )
            },
        )
        .unwrap();
    let read = environment.clone();
    h.prompt
        .section(
            &h.ctx,
            PromptSection {
                text: text_of(move || read.lock().unwrap().url.to_owned()),
                ..section("web", SystemPrompt::get_section_order(S::WebSurface), "")
            },
        )
        .unwrap();
    let first = rendered(&h.prompt.assemble(plain()).await.unwrap());
    *environment.lock().unwrap() = Env {
        model: "model-a",
        cwd: "C:/bob/project",
        platform: "win32",
        source: "C:/bob/dsh",
        url: "http://127.0.0.1:4080",
    };
    let second = rendered(&h.prompt.assemble(plain()).await.unwrap());
    let labels: Vec<&str> = reusable.iter().map(|(_, label)| *label).collect();
    let prefix = format!(
        "{IDENTITY}\n\nModel model-a.\n\n{}\n\n",
        labels.join("\n\n")
    );
    assert_eq!(
        first,
        format!("{prefix}/alice/dsh\n\nhttp://127.0.0.1:3080\n\nIn /alice/project on darwin.")
    );
    assert_eq!(
        second,
        format!("{prefix}C:/bob/dsh\n\nhttp://127.0.0.1:4080\n\nIn C:/bob/project on win32.")
    );
    environment.lock().unwrap().model = "model-b";
    assert_eq!(
        rendered(&h.prompt.assemble(plain()).await.unwrap()),
        second.replace("Model model-a.", "Model model-b.")
    );
    h.kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn keeps_repository_context_placements_unique_and_integral() {
    let orders: Vec<f64> = CONTEXT_ORDER_NAMES
        .iter()
        .map(|n| SystemPrompt::get_context_order(*n))
        .collect();
    assert!(orders.iter().all(|order| order.fract() == 0.0));
    let mut sorted = orders.clone();
    sorted.sort_by(f64::total_cmp);
    sorted.dedup();
    assert_eq!(sorted.len(), orders.len());
}

#[tokio::test(start_paused = true)]
async fn built_in_renders_the_environment_after_guidance_and_reports_its_strict_interpolation_errors()
 {
    let h =
        mount(json!({"personaPrefix": "Model {{model}}.", "personaSuffix": "Workspace {{cwd}}."}))
            .await;
    h.prompt
        .variable(&h.ctx, "model", |_| Ok(Some("m".into())))
        .unwrap();
    h.prompt
        .section(&h.ctx, section("guidance", 100.0, "Use tools."))
        .unwrap();
    let unresolved = h.prompt.assemble(plain()).await.unwrap();
    let error = render_prompt(&unresolved).unwrap_err();
    assert!(
        error.to_string().contains(
            "unknown prompt variable \"{{cwd}}\" in section \"deployment:persona-suffix\""
        ),
        "{error}"
    );
    h.prompt
        .variable(&h.ctx, "cwd", |_| Ok(Some("/work".into())))
        .unwrap();
    assert_eq!(
        rendered(&h.prompt.assemble(plain()).await.unwrap()),
        format!("{IDENTITY}\n\nModel m.\n\nUse tools.\n\nWorkspace /work.")
    );
    h.kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn built_in_registers_the_harness_identity_and_the_configured_deployment_persona() {
    let h = mount(json!({"personaPrefix": "You are DeepSeek Harness."})).await;
    let assembly = h.prompt.assemble(plain()).await.unwrap();
    assert_eq!(
        section_names(&assembly),
        [
            "harness:identity",
            "deployment:persona-prefix",
            "deployment:persona-suffix"
        ]
    );
    assert_eq!(
        rendered(&assembly),
        format!("{IDENTITY}\n\nYou are DeepSeek Harness.")
    );
    // The names are reserved by the plugin — one owner per section.
    let error = h
        .prompt
        .section(&h.ctx, section(PERSONA_PREFIX_SECTION, 0.0, "imposter"))
        .err()
        .unwrap();
    assert!(
        error
            .to_string()
            .contains("prompt section \"deployment:persona-prefix\" is already registered"),
        "{error}"
    );
    h.kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn built_in_renders_no_persona_section_for_a_persona_less_deployment() {
    let h = mount(Value::Null).await;
    assert_eq!(
        rendered(&h.prompt.assemble(plain()).await.unwrap()),
        IDENTITY
    );
    h.kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn built_in_can_omit_the_harness_identity_for_a_deployment_that_owns_the_complete_persona() {
    let h = mount(json!({
        "includeHarnessIdentity": false,
        "personaPrefix": "You are a helpful software engineer assistant."
    }))
    .await;
    let assembly = h.prompt.assemble(plain()).await.unwrap();
    assert_eq!(
        section_names(&assembly),
        ["deployment:persona-prefix", "deployment:persona-suffix"]
    );
    assert_eq!(
        rendered(&assembly),
        "You are a helpful software engineer assistant."
    );
    h.kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn built_in_can_suppress_runtime_context_without_evaluating_providers_or_accepting_waterfall_additions()
 {
    let h = mount(json!({"includeRuntimeContext": false})).await;
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    h.prompt
        .context(
            &h.ctx,
            PromptContext {
                text: PromptText::provider(move |_| {
                    Ok(format!(
                        "policy {}",
                        counter.fetch_add(1, Ordering::SeqCst) + 1
                    ))
                }),
                ..context("policy", 0.0, "")
            },
        )
        .unwrap();
    push_context_listener(&h, ("late", "late context"));
    let assembly = h.prompt.assemble(plain()).await.unwrap();
    assert_eq!(assembly.contexts, Vec::new());
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    h.kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn built_in_tolerates_a_schema_bypassing_direct_construction() {
    // `ctx.plugin` validates and defaults the config first; a direct construction skips
    // the schema, so the defaults of `Config` are what fires.
    let kernel = Kernel::start();
    let service = SystemPrompt::new(kernel.root(), &Config::default()).unwrap();
    assert_eq!(
        rendered(&service.assemble(plain()).await.unwrap()),
        IDENTITY
    );
    kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn assembles_sections_in_order_with_context_resolved_text_and_collected_tools() {
    let h = mount(json!({"personaPrefix": "You are DeepSeek Harness."})).await;
    h.prompt
        .section(
            &h.ctx,
            PromptSection {
                text: text_of(|| "cwd: /tmp".into()),
                ..section("cwd", 20.0, "")
            },
        )
        .unwrap();
    h.prompt
        .section(&h.ctx, section("rules", 10.0, "Be precise."))
        .unwrap();
    h.prompt
        .context(
            &h.ctx,
            PromptContext {
                text: text_of(|| "context 2".into()),
                ..context("later", 20.0, "")
            },
        )
        .unwrap();
    h.prompt
        .context(&h.ctx, context("earlier", 10.0, "context 1"))
        .unwrap();
    h.prompt
        .tools(&h.ctx, |_| {
            Ok(ToolProviderResult {
                schemas: vec![ToolSchema {
                    name: "echo".into(),
                    description: "echo back".into(),
                    parameters: Default::default(),
                    defer_loading: None,
                }],
                known_names: None,
            })
        })
        .unwrap();

    let assembly = h.prompt.assemble(plain()).await.unwrap();
    assert_eq!(
        section_names(&assembly),
        [
            "harness:identity",
            "deployment:persona-prefix",
            "rules",
            "cwd",
            "deployment:persona-suffix"
        ]
    );
    let texts: Vec<&str> = assembly.sections.iter().map(|s| s.text.as_str()).collect();
    assert_eq!(
        texts,
        [
            IDENTITY,
            "You are DeepSeek Harness.",
            "Be precise.",
            "cwd: /tmp",
            ""
        ]
    );
    assert_eq!(
        assembly.contexts,
        [
            assembled("earlier", "context 1"),
            assembled("later", "context 2")
        ]
    );
    assert_eq!(
        assembly.tools,
        [ToolSchema {
            name: "echo".into(),
            description: "echo back".into(),
            parameters: Default::default(),
            defer_loading: None
        }]
    );
    assert!(assembly.variables.is_empty());
    assert_eq!(
        rendered(&assembly),
        format!("{IDENTITY}\n\nYou are DeepSeek Harness.\n\nBe precise.\n\ncwd: /tmp")
    );
    assert_eq!(
        render_context_snapshot(&assembly).unwrap(),
        format!("{SNAPSHOT_HEAD}\n\ncontext 1\n\ncontext 2")
    );
    h.kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn breaks_equal_section_orders_by_code_unit_name_regardless_of_registration_order() {
    for names in [["äther", "zeta"], ["zeta", "äther"]] {
        let h = mount(json!({})).await;
        for name in names {
            h.prompt.section(&h.ctx, section(name, 10.0, name)).unwrap();
        }
        let assembly = h.prompt.assemble(plain()).await.unwrap();
        let got: Vec<String> = contributed(&assembly).into_iter().map(|s| s.name).collect();
        assert_eq!(got, ["zeta", "äther"]);
        h.kernel.shutdown().await;
    }
}

#[tokio::test(start_paused = true)]
async fn orders_names_by_utf16_code_unit_not_by_code_point() {
    // U+FF5E (BMP) sorts after U+1F600 (surrogate pair D83D…) in UTF-16, before it by code point.
    let h = mount(json!({})).await;
    for name in ["\u{ff5e}", "\u{1f600}"] {
        h.prompt.section(&h.ctx, section(name, 10.0, name)).unwrap();
    }
    let assembly = h.prompt.assemble(plain()).await.unwrap();
    let got: Vec<String> = contributed(&assembly).into_iter().map(|s| s.name).collect();
    assert_eq!(got, ["\u{1f600}", "\u{ff5e}"]);
    h.kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn resolves_section_text_providers_against_the_assemble_context_at_each_assemble_call() {
    // The context is how per-agent sections work; this test smuggles a marker through an extension field.
    let h = mount(json!({})).await;
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    h.prompt
        .section(
            &h.ctx,
            PromptSection {
                text: PromptText::provider(move |context| {
                    let n = counter.fetch_add(1, Ordering::SeqCst) + 1;
                    let who = context
                        .extension::<Who>()
                        .map_or("nobody", |w| w.0.as_str());
                    Ok(format!("call {n} for {who}"))
                }),
                ..section("dynamic", 0.0, "")
            },
        )
        .unwrap();
    assert_eq!(
        contributed(&h.prompt.assemble(who("alice")).await.unwrap())[0].text,
        "call 1 for alice"
    );
    assert_eq!(
        contributed(&h.prompt.assemble(plain()).await.unwrap())[0].text,
        "call 2 for nobody"
    );
    h.kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn removes_contributions_when_the_contributing_fiber_is_disposed() {
    let h = mount(json!({})).await;
    let fiber = h.ctx.plugin::<Contributor>(Value::Null).unwrap();
    fiber.settled().await.unwrap();

    let before = h.prompt.assemble(plain()).await.unwrap();
    assert_eq!(contributed(&before).len(), 1);
    assert_eq!(before.contexts.len(), 1);
    assert_eq!(
        before.variables,
        IndexMap::from([("scoped_var".to_owned(), Some("v".to_owned()))])
    );
    fiber.dispose().await;
    let assembly = h.prompt.assemble(plain()).await.unwrap();
    assert_eq!(contributed(&assembly).len(), 0);
    assert_eq!(assembly.contexts.len(), 0);
    // The built-ins belong to the service fiber, so they survive the plugin's disposal.
    assert_eq!(section_names(&assembly), BUILT_IN);
    assert_eq!(assembly.tools.len(), 0);
    assert!(assembly.variables.is_empty());
    h.kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn rejects_a_duplicate_section_name() {
    let h = mount(json!({})).await;
    h.prompt
        .section(&h.ctx, section("dup", 0.0, "first"))
        .unwrap();
    let error = h
        .prompt
        .section(&h.ctx, section("dup", 1.0, "second"))
        .err()
        .unwrap();
    assert!(
        error
            .to_string()
            .contains("prompt section \"dup\" is already registered"),
        "{error}"
    );
    // The failed registration leaked nothing; the original stays intact.
    let assembly = h.prompt.assemble(plain()).await.unwrap();
    let texts: Vec<String> = contributed(&assembly).into_iter().map(|s| s.text).collect();
    assert_eq!(texts, ["first"]);
    h.kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn rejects_a_non_finite_section_order() {
    let h = mount(json!({})).await;
    for order in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let error = h
            .prompt
            .section(&h.ctx, section("bad-order", order, "x"))
            .err()
            .unwrap();
        assert!(
            error.to_string().contains("order must be a finite number"),
            "{error}"
        );
    }
    assert_eq!(
        contributed(&h.prompt.assemble(plain()).await.unwrap()),
        Vec::new()
    );
    h.kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn rejects_duplicate_and_non_finite_context_registrations_without_leaking() {
    let h = mount(json!({})).await;
    h.prompt
        .context(&h.ctx, context("policy", 1.0, "first"))
        .unwrap();
    let error = h
        .prompt
        .context(&h.ctx, context("policy", 2.0, "second"))
        .err()
        .unwrap();
    assert!(
        error
            .to_string()
            .contains("prompt context \"policy\" is already registered"),
        "{error}"
    );
    let error = h
        .prompt
        .context(&h.ctx, context("bad", f64::NAN, "x"))
        .err()
        .unwrap();
    assert!(
        error
            .to_string()
            .contains("prompt context \"bad\" order must be a finite number"),
        "{error}"
    );
    assert_eq!(
        h.prompt.assemble(plain()).await.unwrap().contexts,
        [assembled("policy", "first")]
    );
    h.kernel.shutdown().await;
}

/// A `system-prompt/change` listener that panics once, as the source's listener throws once.
fn throw_once(h: &Harness) -> hoh_kernel::Disposer {
    let threw = Arc::new(AtomicBool::new(false));
    h.ctx
        .on::<SystemPromptChange>(move |_| {
            if !threw.swap(true, Ordering::SeqCst) {
                panic!("boom change listener");
            }
        })
        .unwrap()
}

#[tokio::test(start_paused = true)]
async fn rolls_back_a_section_when_a_change_listener_throws() {
    let h = mount(json!({})).await;
    let off = throw_once(&h);

    let error = h
        .prompt
        .section(&h.ctx, section("p", 0.0, "persona"))
        .err()
        .unwrap();
    assert!(
        error.to_string().contains("boom change listener"),
        "{error}"
    );
    assert_eq!(
        contributed(&h.prompt.assemble(plain()).await.unwrap()).len(),
        0
    ); // nothing leaked

    // Subsequent listener-free register contributes exactly once.
    off.dispose().await;
    h.prompt
        .section(&h.ctx, section("p", 0.0, "persona"))
        .unwrap();
    let names: Vec<String> = contributed(&h.prompt.assemble(plain()).await.unwrap())
        .into_iter()
        .map(|s| s.name)
        .collect();
    assert_eq!(names, ["p"]);
    h.kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn rolls_back_a_tool_provider_when_a_change_listener_throws() {
    let h = mount(json!({})).await;
    let off = throw_once(&h);

    let error = h.prompt.tools(&h.ctx, schemas(&["t"])).err().unwrap();
    assert!(
        error.to_string().contains("boom change listener"),
        "{error}"
    );
    assert_eq!(h.prompt.assemble(plain()).await.unwrap().tools.len(), 0);

    off.dispose().await;
    h.prompt.tools(&h.ctx, schemas(&["t"])).unwrap();
    assert_eq!(names(&h.prompt.assemble(plain()).await.unwrap()), ["t"]);
    h.kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn snapshots_tool_provider_membership_before_evaluating_an_assembly() {
    let h = mount(json!({})).await;
    let added = Arc::new(AtomicBool::new(false));
    let (prompt, ctx) = (h.prompt.clone(), h.ctx.clone());
    h.prompt
        .tools(&h.ctx, move |_| {
            if !added.swap(true, Ordering::SeqCst) {
                prompt.tools(&ctx, schemas(&["late"])).unwrap();
            }
            Ok(ToolProviderResult {
                schemas: vec![tool("first", "")],
                known_names: None,
            })
        })
        .unwrap();

    assert_eq!(names(&h.prompt.assemble(plain()).await.unwrap()), ["first"]);
    assert_eq!(
        names(&h.prompt.assemble(plain()).await.unwrap()),
        ["first", "late"]
    );
    h.kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn rolls_back_a_variable_when_a_change_listener_throws() {
    let h = mount(json!({})).await;
    let off = throw_once(&h);

    let error = h
        .prompt
        .variable(&h.ctx, "v", |_| Ok(Some("x".into())))
        .err()
        .unwrap();
    assert!(
        error.to_string().contains("boom change listener"),
        "{error}"
    );
    assert!(
        h.prompt
            .assemble(plain())
            .await
            .unwrap()
            .variables
            .is_empty()
    );

    off.dispose().await;
    h.prompt
        .variable(&h.ctx, "v", |_| Ok(Some("x".into())))
        .unwrap();
    assert_eq!(
        h.prompt.assemble(plain()).await.unwrap().variables,
        IndexMap::from([("v".to_owned(), Some("x".to_owned()))])
    );
    h.kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn composes_multiple_assemble_waterfall_listeners_in_order_with_the_context() {
    let h = mount(json!({})).await;
    h.prompt
        .section(&h.ctx, section("base", 10.0, "base"))
        .unwrap();

    // Listener A appends a section, then delegates.
    let contexts: Arc<Mutex<Vec<Arc<AssembleContext>>>> = Arc::default();
    let record = contexts.clone();
    h.ctx
        .on_waterfall::<SystemPromptAssemble, PromptPresentation, _>(move |event, next| {
            let record = record.clone();
            async move {
                record.lock().unwrap().push(event.context.clone());
                event.update(|a| a.sections.push(presentation_section("from-a", "a")));
                next.run().await
            }
        })
        .unwrap();
    // Listener B (registered later, runs after A) sees A's contribution.
    let seen: Arc<Mutex<Vec<Vec<String>>>> = Arc::default();
    let record = seen.clone();
    h.ctx
        .on_waterfall::<SystemPromptAssemble, PromptPresentation, _>(move |event, next| {
            let record = record.clone();
            async move {
                record.lock().unwrap().push(
                    event
                        .assembly()
                        .snapshot()
                        .sections
                        .iter()
                        .map(|s| s.name.clone())
                        .collect(),
                );
                next.run().await
            }
        })
        .unwrap();

    let passed = Arc::new(AssembleContext::default());
    let assembly = h.prompt.assemble(passed.clone()).await.unwrap();
    let expected = [
        "harness:identity",
        "deployment:persona-prefix",
        "base",
        "deployment:persona-suffix",
        "from-a",
    ];
    assert_eq!(*seen.lock().unwrap(), [expected]);
    assert_eq!(section_names(&assembly), expected);
    // The caller's context reaches listeners.
    assert!(Arc::ptr_eq(&contexts.lock().unwrap()[0], &passed));
    h.kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn lets_a_waterfall_listener_short_circuit_by_not_calling_next() {
    let h = mount(json!({})).await;
    h.prompt
        .section(&h.ctx, section("real", 0.0, "real"))
        .unwrap();
    h.ctx
        .on_waterfall::<SystemPromptAssemble, PromptPresentation, _>(|_event, _next| async move {
            Ok(PromptPresentation::default())
        })
        .unwrap();
    assert_eq!(h.prompt.assemble(plain()).await.unwrap().sections.len(), 0);
    h.kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn restores_one_complete_section_after_the_assembly_waterfall() {
    let h = mount(json!({})).await;
    h.prompt
        .section(
            &h.ctx,
            PromptSection {
                complete: true,
                ..section("complete", 10.0, "Exact prompt.")
            },
        )
        .unwrap();
    h.prompt
        .section(&h.ctx, section("extra", 20.0, "extra"))
        .unwrap();
    h.ctx
        .listen()
        .prepend()
        .on_waterfall::<SystemPromptAssemble, PromptPresentation, _>(|event, next| async move {
            if !event
                .assembly()
                .snapshot()
                .sections
                .iter()
                .any(|s| s.name == "complete")
            {
                return Err("complete section missing before waterfall".into());
            }
            event.update(|a| {
                a.sections
                    .iter_mut()
                    .find(|s| s.name == "complete")
                    .unwrap()
                    .text = "mutated".into();
                a.sections.push(presentation_section("late", "late"));
            });
            next.run().await
        })
        .unwrap();

    assert_eq!(
        h.prompt.assemble(plain()).await.unwrap().sections,
        [presentation_section("complete", "Exact prompt.")]
    );
    h.kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn rejects_multiple_effective_complete_sections() {
    let h = mount(json!({})).await;
    h.prompt
        .section(
            &h.ctx,
            PromptSection {
                complete: true,
                ..section("first", 10.0, "first")
            },
        )
        .unwrap();
    h.prompt
        .section(
            &h.ctx,
            PromptSection {
                complete: true,
                ..section("second", 20.0, "second")
            },
        )
        .unwrap();
    let error = h.prompt.assemble(plain()).await.unwrap_err();
    assert!(
        error
            .to_string()
            .contains("multiple complete prompt sections are active: \"first\", \"second\""),
        "{error}"
    );
    h.kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn assembles_snapshots_so_one_step_mutations_do_not_leak_into_future_assemblies() {
    let h = mount(json!({})).await;
    h.prompt
        .section(&h.ctx, section("base", 10.0, "base"))
        .unwrap();
    h.prompt
        .tools(&h.ctx, |_| {
            Ok(ToolProviderResult {
                schemas: vec![tool("t", "tool")],
                known_names: None,
            })
        })
        .unwrap();

    let mut first = h.prompt.assemble(plain()).await.unwrap();
    first.sections[0].name = "mutated".into();
    first.sections[0].text = "mutated".into();
    first.contexts.push(assembled("mutated", "mutated"));
    first.tools[0].description = "mutated".into();
    first.tools[0]
        .parameters
        .get_mut("properties")
        .unwrap()
        .as_object_mut()
        .unwrap()
        .insert("leak".into(), json!({"type": "string"}));

    let second = h.prompt.assemble(plain()).await.unwrap();
    assert_eq!(
        section_names(&second),
        [
            "harness:identity",
            "deployment:persona-prefix",
            "base",
            "deployment:persona-suffix"
        ]
    );
    assert_eq!(second.sections[0].text, IDENTITY);
    assert_eq!(second.contexts, Vec::new());
    assert_eq!(second.tools, [tool("t", "tool")]);
    h.kernel.shutdown().await;
}

#[test]
fn filters_out_empty_section_text_from_render_prompt() {
    let result = rendered(&only_sections(vec![
        presentation_section("empty", ""),
        presentation_section("real", "content"),
    ]));
    assert_eq!(result, "content");
}

#[tokio::test(start_paused = true)]
async fn filters_empty_context_interpolates_variables_and_returns_empty_without_active_context() {
    let h = mount(json!({})).await;
    h.prompt.context(&h.ctx, context("empty", 0.0, "")).unwrap();
    assert_eq!(
        render_context_snapshot(&h.prompt.assemble(plain()).await.unwrap()).unwrap(),
        ""
    );
    h.prompt
        .variable(&h.ctx, "mode", |_| Ok(Some("read-only".into())))
        .unwrap();
    h.prompt
        .context(&h.ctx, context("policy", 1.0, "Mode: {{mode}}."))
        .unwrap();
    assert_eq!(
        render_context_snapshot(&h.prompt.assemble(plain()).await.unwrap()).unwrap(),
        format!("{SNAPSHOT_HEAD}\n\nMode: read-only.")
    );
    h.kernel.shutdown().await;
}

#[test]
fn attributes_context_interpolation_failures_to_the_contributing_context() {
    let assembly = PromptAssembly {
        contexts: vec![assembled("policy", "Mode: {{missing}}.")],
        ..PromptAssembly::default()
    };
    let error = render_context_snapshot(&assembly).unwrap_err();
    assert!(
        error.to_string().contains(
            "unknown prompt variable \"{{missing}}\" in context \"policy\"; registered variables: (none)"
        ),
        "{error}"
    );
}

#[test]
fn joins_no_text_into_an_empty_snapshot() {
    assert_eq!(join_context_sections(&[]), "");
}

#[tokio::test(start_paused = true)]
async fn emits_system_prompt_change_when_a_tool_provider_is_registered_and_disposed() {
    let h = mount(json!({})).await;
    let count = Arc::new(AtomicUsize::new(0));
    let counter = count.clone();
    h.ctx
        .on::<SystemPromptChange>(move |_| {
            counter.fetch_add(1, Ordering::SeqCst);
        })
        .unwrap();

    let dispose = h
        .prompt
        .tools(&h.ctx, |_| Ok(ToolProviderResult::default()))
        .unwrap();
    // registration emits change
    assert_eq!(count.load(Ordering::SeqCst), 1);
    dispose.dispose().await;
    // disposal emits change again
    assert_eq!(count.load(Ordering::SeqCst), 2);
    h.kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn emits_system_prompt_change_when_a_context_is_registered_and_disposed() {
    let h = mount(json!({})).await;
    let count = Arc::new(AtomicUsize::new(0));
    let counter = count.clone();
    h.ctx
        .on::<SystemPromptChange>(move |_| {
            counter.fetch_add(1, Ordering::SeqCst);
        })
        .unwrap();
    let dispose = h
        .prompt
        .context(&h.ctx, context("policy", 0.0, "current"))
        .unwrap();
    assert_eq!(count.load(Ordering::SeqCst), 1);
    dispose.dispose().await;
    assert_eq!(count.load(Ordering::SeqCst), 2);
    h.kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn cleans_up_tool_providers_on_fiber_dispose() {
    let h = mount(json!({})).await;
    let fiber = h.ctx.plugin::<ToolContributor>(Value::Null).unwrap();
    fiber.settled().await.unwrap();

    assert_eq!(h.prompt.assemble(plain()).await.unwrap().tools.len(), 1);
    fiber.dispose().await;
    assert_eq!(h.prompt.assemble(plain()).await.unwrap().tools.len(), 0);
    h.kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn removes_section_when_returned_disposer_is_called_directly() {
    let h = mount(json!({})).await;
    let dispose = h
        .prompt
        .section(&h.ctx, section("direct", 0.0, "direct section"))
        .unwrap();
    assert_eq!(
        contributed(&h.prompt.assemble(plain()).await.unwrap()).len(),
        1
    );
    dispose.dispose().await;
    assert_eq!(
        contributed(&h.prompt.assemble(plain()).await.unwrap()).len(),
        0
    );
    h.kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn removes_tool_provider_when_returned_disposer_is_called_directly() {
    let h = mount(json!({})).await;
    let dispose = h.prompt.tools(&h.ctx, schemas(&["direct-tool"])).unwrap();
    assert_eq!(h.prompt.assemble(plain()).await.unwrap().tools.len(), 1);
    dispose.dispose().await;
    assert_eq!(h.prompt.assemble(plain()).await.unwrap().tools.len(), 0);
    h.kernel.shutdown().await;
}

mod prompt_variables {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn resolves_each_variable_against_the_assemble_context_and_emits_change_on_register_and_unregister()
     {
        let h = mount(json!({})).await;
        let count = Arc::new(AtomicUsize::new(0));
        let counter = count.clone();
        h.ctx
            .on::<SystemPromptChange>(move |_| {
                counter.fetch_add(1, Ordering::SeqCst);
            })
            .unwrap();

        let dispose = h
            .prompt
            .variable(&h.ctx, "who", |context| {
                Ok(context.extension::<Who>().map(|w| w.0.clone()))
            })
            .unwrap();
        assert_eq!(count.load(Ordering::SeqCst), 1);

        assert_eq!(
            h.prompt.assemble(who("alice")).await.unwrap().variables,
            IndexMap::from([("who".to_owned(), Some("alice".to_owned()))])
        );
        // A provider returning nothing records "registered but no value here".
        assert_eq!(
            h.prompt.assemble(plain()).await.unwrap().variables,
            IndexMap::from([("who".to_owned(), None)])
        );

        dispose.dispose().await;
        assert_eq!(count.load(Ordering::SeqCst), 2);
        assert!(
            h.prompt
                .assemble(plain())
                .await
                .unwrap()
                .variables
                .is_empty()
        );
        h.kernel.shutdown().await;
    }

    #[tokio::test(start_paused = true)]
    async fn live_iterates_variables_registered_by_an_earlier_provider() {
        let h = mount(json!({})).await;
        let added = Arc::new(AtomicBool::new(false));
        let (prompt, ctx) = (h.prompt.clone(), h.ctx.clone());
        h.prompt
            .variable(&h.ctx, "first", move |_| {
                if !added.swap(true, Ordering::SeqCst) {
                    prompt
                        .variable(&ctx, "late", |_| Ok(Some("second value".into())))
                        .unwrap();
                }
                Ok(Some("first value".into()))
            })
            .unwrap();
        assert_eq!(
            h.prompt.assemble(plain()).await.unwrap().variables,
            IndexMap::from([
                ("first".to_owned(), Some("first value".to_owned())),
                ("late".to_owned(), Some("second value".to_owned())),
            ])
        );
        h.kernel.shutdown().await;
    }

    #[tokio::test(start_paused = true)]
    async fn rejects_a_duplicate_variable_name_and_an_unreferenceable_name() {
        let h = mount(json!({})).await;
        h.prompt
            .variable(&h.ctx, "model", |_| Ok(Some("m1".into())))
            .unwrap();
        let error = h
            .prompt
            .variable(&h.ctx, "model", |_| Ok(Some("m2".into())))
            .err()
            .unwrap();
        assert!(
            error
                .to_string()
                .contains("prompt variable \"model\" is already registered"),
            "{error}"
        );
        assert!(
            error.to_string().contains("for a per-agent value"),
            "{error}"
        );
        let error = h
            .prompt
            .variable(&h.ctx, "Not Valid", |_| Ok(Some("x".into())))
            .err()
            .unwrap();
        assert_eq!(
            error.to_string(),
            "invalid prompt variable name \"Not Valid\" (must match /^[a-z][a-z0-9_]*$/)"
        );
        // Neither failed registration leaked.
        assert_eq!(
            h.prompt.assemble(plain()).await.unwrap().variables,
            IndexMap::from([("model".to_owned(), Some("m1".to_owned()))])
        );
        h.kernel.shutdown().await;
    }

    #[tokio::test(start_paused = true)]
    async fn interpolates_name_references_in_section_text_at_render_the_persona_included() {
        let h = mount(json!({"personaPrefix": "You run on {{model}} in {{cwd}}."})).await;
        h.prompt
            .variable(&h.ctx, "model", |_| Ok(Some("deepseek-v4".into())))
            .unwrap();
        h.prompt
            .variable(&h.ctx, "cwd", |_| Ok(Some("/work".into())))
            .unwrap();
        assert_eq!(
            rendered(&h.prompt.assemble(plain()).await.unwrap()),
            format!("{IDENTITY}\n\nYou run on deepseek-v4 in /work.")
        );
        h.kernel.shutdown().await;
    }

    #[tokio::test(start_paused = true)]
    async fn preserves_literal_section_text_with_complete_and_dynamic() {
        for (complete, dynamic) in [(false, false), (false, true), (true, false), (true, true)] {
            let h =
                mount(json!({"includeHarnessIdentity": false, "personaPrefix": "{{model}}"})).await;
            h.prompt
                .variable(&h.ctx, "model", |_| Ok(Some("actual-model".into())))
                .unwrap();
            let text = "{{item}} {{model}} {{ model }} {{nested{{item}}}}";
            h.prompt
                .section(
                    &h.ctx,
                    PromptSection {
                        text: if dynamic {
                            text_of(move || text.to_owned())
                        } else {
                            text.into()
                        },
                        interpolate: Some(false),
                        complete,
                        ..section("literal", 1.0, "")
                    },
                )
                .unwrap();
            let expected = if complete {
                text.to_owned()
            } else {
                format!("actual-model\n\n{text}")
            };
            assert_eq!(
                rendered(&h.prompt.assemble(plain()).await.unwrap()),
                expected,
                "complete={complete} dynamic={dynamic}"
            );
            h.kernel.shutdown().await;
        }
    }

    #[tokio::test(start_paused = true)]
    async fn lets_a_waterfall_listener_add_or_override_variables_before_render() {
        let h = mount(json!({})).await;
        h.prompt
            .section(&h.ctx, section("s", 0.0, "{{extra}}"))
            .unwrap();
        h.ctx
            .on_waterfall::<SystemPromptAssemble, PromptPresentation, _>(|event, next| async move {
                event.update(|a| a.variables.insert("extra", Some("from-waterfall".into())));
                next.run().await
            })
            .unwrap();
        assert_eq!(
            rendered(&h.prompt.assemble(plain()).await.unwrap()),
            format!("{IDENTITY}\n\nfrom-waterfall")
        );
        h.kernel.shutdown().await;
    }

    #[tokio::test(start_paused = true)]
    async fn throws_on_a_reference_to_an_unregistered_variable_listing_what_exists() {
        let h = mount(json!({})).await;
        h.prompt
            .section(&h.ctx, section("persona", 0.0, "on {{modle}}"))
            .unwrap();
        h.prompt
            .variable(&h.ctx, "model", |_| Ok(Some("m".into())))
            .unwrap();
        let error = render_prompt(&h.prompt.assemble(plain()).await.unwrap()).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("unknown prompt variable \"{{modle}}\" in section \"persona\"; registered variables: model"),
            "{error}"
        );
        h.kernel.shutdown().await;
    }

    #[test]
    fn names_none_when_no_variables_are_registered_at_all() {
        let error =
            render_prompt(&only_sections(vec![presentation_section("s", "{{x}}")])).unwrap_err();
        assert!(
            error.to_string().contains(
                "unknown prompt variable \"{{x}}\" in section \"s\"; registered variables: (none)"
            ),
            "{error}"
        );
    }

    #[test]
    fn throws_when_a_referenced_variable_has_no_value_for_this_assembly() {
        let assembly = with_variables(
            vec![presentation_section("persona", "in {{cwd}}")],
            &[("cwd", None)],
        );
        let error = render_prompt(&assembly).unwrap_err();
        assert!(
            error.to_string().contains(
                "prompt variable \"{{cwd}}\" has no value for this assembly (section \"persona\")"
            ),
            "{error}"
        );
    }

    #[test]
    fn throws_on_a_malformed_complete_reference_e_g_inner_spaces() {
        let assembly = with_variables(
            vec![presentation_section("s", "on {{ model }}")],
            &[("model", Some("m"))],
        );
        let error = render_prompt(&assembly).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("malformed prompt variable reference \"{{ model }}\" in section \"s\""),
            "{error}"
        );
        assert!(
            error
                .to_string()
                .ends_with("(variable names match /^[a-z][a-z0-9_]*$/)"),
            "{error}"
        );
    }

    #[test]
    fn leaves_a_lone_open_braces_verbatim_only_when_no_close_braces_follow_anywhere_after_it() {
        let text = rendered(&only_sections(vec![presentation_section(
            "s",
            "shell ${X:-{{fallback} stays",
        )]));
        assert_eq!(text, "shell ${X:-{{fallback} stays");
    }

    #[test]
    fn throws_on_a_mangled_reference_with_a_close_braces_still_following() {
        for (label, text) in [
            ("extra outer braces", "{{{model}}}"),
            (
                "nested brace inside a would-be group",
                "x {{a{b}} y {{model}}",
            ),
        ] {
            let assembly = with_variables(
                vec![presentation_section("s", text)],
                &[("model", Some("m"))],
            );
            let error = render_prompt(&assembly).unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("malformed prompt variable reference at"),
                "{label}: {error}"
            );
        }
    }

    #[test]
    fn cuts_the_malformed_excerpt_at_sixteen_utf16_units() {
        let assembly = with_variables(
            vec![presentation_section("s", "{{aé{b}} 😀 and more text")],
            &[],
        );
        let error = render_prompt(&assembly).unwrap_err();
        assert_eq!(
            error.to_string(),
            "malformed prompt variable reference at \"{{aé{b}} 😀 and …\" in section \"s\" (references are complete simple {{name}} groups)"
        );
    }

    #[test]
    fn rejects_constructor_as_unknown_prototype_properties_are_not_variables() {
        let assembly = with_variables(
            vec![presentation_section("s", "on {{constructor}}")],
            &[("model", Some("m"))],
        );
        let error = render_prompt(&assembly).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("unknown prompt variable \"{{constructor}}\""),
            "{error}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_variable_named_like_a_prototype_property_works_once_actually_registered() {
        let h = mount(json!({})).await;
        h.prompt
            .section(&h.ctx, section("s", 0.0, "{{constructor}}"))
            .unwrap();
        h.prompt
            .variable(&h.ctx, "constructor", |_| Ok(Some("own-value".into())))
            .unwrap();
        assert_eq!(
            rendered(&h.prompt.assemble(plain()).await.unwrap()),
            format!("{IDENTITY}\n\nown-value")
        );
        h.kernel.shutdown().await;
    }

    #[test]
    fn never_re_scans_substituted_values() {
        let assembly = with_variables(
            vec![presentation_section("s", "v = {{model}}!")],
            &[("model", Some("literal {{sneaky}} inside"))],
        );
        assert_eq!(rendered(&assembly), "v = literal {{sneaky}} inside!");
    }
}

#[tokio::test(start_paused = true)]
async fn publishes_the_config_schema_with_the_source_defaults() {
    let schema = hoh_kernel::PluginDef::of::<SystemPrompt>().schema().clone();
    let properties = &schema["properties"];
    assert_eq!(properties["includeHarnessIdentity"]["default"], json!(true));
    assert_eq!(properties["includeRuntimeContext"]["default"], json!(true));
    assert_eq!(properties["personaPrefix"]["default"], json!(""));
    assert_eq!(properties["personaSuffix"]["default"], json!(""));
    assert!(properties["toolOrder"].get("default").is_none());
    assert_eq!(
        hoh_kernel::PluginDef::of::<SystemPrompt>().name(),
        "SystemPrompt"
    );
}
