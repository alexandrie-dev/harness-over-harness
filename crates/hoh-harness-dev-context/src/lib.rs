//! Model context for the Harness checkout and the Web GUI serving it.

mod shim;

use hoh_agent::AssembleAgent;
use hoh_error::BoxError;
use hoh_kernel::{Cfg, Context, Inject, NoConfig, Plugin, Service};
use hoh_session::SessionEvent;
use hoh_session_projection::{ProjectionDefinition, ProjectionHandle, SessionProjections};
use hoh_shell_env::InstallAnchorKey;
use hoh_system_prompt::{
    AssembleContext, PromptContext, PromptContextOrderName, PromptSection, PromptSectionOrderName,
    PromptText, SystemPrompt, SystemPromptKey,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{path::MAIN_SEPARATOR, sync::Arc};

pub use shim::{WebApp, WebAppApi};

/// Stable Cordis plugin name.
pub const NAME: &str = "harness-dev-context";

/// Prompt registries, projection registry, and the Web GUI service.
pub const INJECT: [&str; 3] = ["systemPrompt", "sessionProjections", "webApp"];

const HARNESS_SOURCE: &str = "harness:source";
const WEB_SURFACE: &str = "app:web-surface";
const LEGACY_PROMPT: &str = "harnessDevLegacyPrompt";

const HARNESS_SOURCE_PROMPT: &str = "The runtime context names the DeepSeek Harness implementation checkout. The checkout location and current working directory are separate values and may differ; never infer the working directory from the checkout path. Use pwd to determine the current working directory. Use this checkout only to inspect or extend DSH itself.";

const WEB_SURFACE_PROMPT: &str = "You are interacting with the user through the DeepSeek Harness Web GUI whose URL the runtime context names. When the user refers to \"this page\", \"this GUI\", or \"this app\" without naming another target, they mean this GUI. The browser provides no implicit DOM, route, or screenshot context. The client-plugin HMR receiver is active, but client-plugin changes reload without a refresh only while `pnpm run dev:web` is also running from this same checkout to rebuild their bundles; verify that watcher before promising automatic updates. Every other change — the apps/web shell and plain packages — requires rebuilding the affected Web artifacts and verifying this existing URL after a page refresh. Starting another server does not update this GUI. The apps/web Vite entry builds the shell but is not a standalone application because only dsh web injects window.__DSH_BOOT__. Do not start a replacement server unless the user asks; if one is needed, use a managed background job and verify its exact URL.";

const LEGACY_SOURCE_PREFIX: &str = "The DeepSeek Harness implementation checkout is at ";
const LEGACY_SOURCE_SUFFIX: &str = ". The checkout location and current working directory are separate values and may differ; never infer the working directory from this path. Use pwd to determine the current working directory. Use this checkout only to inspect or extend DSH itself.";
const LEGACY_WEB_PREFIX: &str =
    "You are interacting with the user through the DeepSeek Harness Web GUI at ";
const LEGACY_WEB_SUFFIX: &str = ". When the user refers to \"this page\", \"this GUI\", or \"this app\" without naming another target, they mean this GUI. The browser provides no implicit DOM, route, or screenshot context. The client-plugin HMR receiver is active, but client-plugin changes reload without a refresh only while `pnpm run dev:web` is also running from this same checkout to rebuild their bundles; verify that watcher before promising automatic updates. Every other change — the apps/web shell and plain packages — requires rebuilding the affected Web artifacts and verifying this existing URL after a page refresh. Starting another server does not update this GUI. The apps/web Vite entry builds the shell but is not a standalone application because only dsh web injects window.__DSH_BOOT__. Do not start a replacement server unless the user asks; if one is needed, use a managed background job and verify its exact URL.";

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
struct LegacyPrompt {
    source: Option<String>,
    web: Option<String>,
}

fn nullable_string(value: &Value, key: &str) -> Result<Option<String>, BoxError> {
    match value.get(key) {
        Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => Ok(Some(text.clone())),
        _ => Err(format!(
            "harness-dev-context projection state field {key} must be a string or null"
        )
        .into()),
    }
}

fn state_schema(value: Option<&Value>) -> Result<Arc<LegacyPrompt>, BoxError> {
    let value = value
        .and_then(Value::as_object)
        .ok_or_else(|| BoxError::from("harness-dev-context projection state must be an object"))?;
    let value = Value::Object(value.clone());
    Ok(Arc::new(LegacyPrompt {
        source: nullable_string(&value, "source")?,
        web: nullable_string(&value, "web")?,
    }))
}

fn legacy_paragraph(prompt: &str, prefix: &str, suffix: &str) -> Option<String> {
    let start = prompt.find(prefix)?;
    let suffix_start = start + prefix.len();
    let end = suffix_start + prompt[suffix_start..].find(suffix)? + suffix.len();
    let paragraph = &prompt[start..end];
    (!paragraph.contains('\n')).then(|| paragraph.to_owned())
}

fn fold_legacy_prompt(
    state: &Arc<LegacyPrompt>,
    event: &SessionEvent,
) -> Result<Arc<LegacyPrompt>, BoxError> {
    if event.event_type() != "system/message" {
        return Ok(state.clone());
    }
    let Some(content) = event
        .data()
        .get("message")
        .and_then(|message| message.get("content"))
        .and_then(Value::as_array)
    else {
        return Ok(state.clone());
    };
    let [block] = content.as_slice() else {
        return Ok(state.clone());
    };
    if block.get("type").and_then(Value::as_str) != Some("text") {
        return Ok(state.clone());
    }
    let Some(text) = block.get("text").and_then(Value::as_str) else {
        return Ok(state.clone());
    };
    if text.is_empty() {
        return Ok(state.clone());
    }
    let next = LegacyPrompt {
        source: legacy_paragraph(text, LEGACY_SOURCE_PREFIX, LEGACY_SOURCE_SUFFIX),
        web: legacy_paragraph(text, LEGACY_WEB_PREFIX, LEGACY_WEB_SUFFIX),
    };
    Ok(if **state == next {
        state.clone()
    } else {
        Arc::new(next)
    })
}

fn projection_definition() -> ProjectionDefinition<LegacyPrompt> {
    ProjectionDefinition {
        key: LEGACY_PROMPT.into(),
        state_version: 1,
        state_schema: Arc::new(state_schema),
        init: Arc::new(|_, _| Ok(Arc::new(LegacyPrompt::default()))),
        apply: Arc::new(fold_legacy_prompt),
        wire: None,
    }
}

fn admitted(
    handle: &ProjectionHandle<LegacyPrompt>,
    context: &AssembleContext,
) -> Result<Arc<LegacyPrompt>, BoxError> {
    match context.extension::<AssembleAgent>() {
        Some(agent) => handle.state_of(agent.0.session()),
        None => Ok(Arc::new(LegacyPrompt::default())),
    }
}

fn source_root(ctx: &Context) -> String {
    let root = ctx
        .get::<InstallAnchorKey>()
        .and_then(|anchor| anchor.0.parent().map(std::path::Path::to_path_buf))
        .or_else(|| {
            std::env::current_exe()
                .ok()
                .and_then(|executable| executable.parent().map(std::path::Path::to_path_buf))
        })
        .unwrap_or_default();
    let mut root = root.as_os_str().to_string_lossy().into_owned();
    if !root.ends_with(MAIN_SEPARATOR) {
        root.push(MAIN_SEPARATOR);
    }
    root
}

fn json_string(value: &str) -> String {
    hoh_util_values::js_json_stringify(&Value::String(value.into()))
}

/// Services declared by the source plugin.
#[derive(Inject)]
pub struct Deps {
    system_prompt: Service<SystemPromptKey>,
    session_projections: Service<SessionProjections>,
    web_app: Service<WebApp>,
}

/// Registers stable prompt guidance and process-specific runtime context.
pub struct HarnessDevContext;

impl Plugin for HarnessDevContext {
    const NAME: &'static str = NAME;
    type Config = NoConfig;
    type Deps = Deps;

    async fn apply(ctx: Context, deps: Deps, _config: Cfg<Self::Config>) -> Result<(), BoxError> {
        let handle = deps
            .session_projections
            .register(&ctx, projection_definition())
            .await?;
        let source_root = source_root(&ctx);

        let source_handle = handle.clone();
        deps.system_prompt.section(
            &ctx,
            PromptSection {
                name: HARNESS_SOURCE.into(),
                order: SystemPrompt::get_section_order(PromptSectionOrderName::HarnessSource),
                interpolate: Some(false),
                text: PromptText::provider(move |context| {
                    Ok(admitted(&source_handle, context)?
                        .source
                        .clone()
                        .unwrap_or_else(|| HARNESS_SOURCE_PROMPT.into()))
                }),
                ..PromptSection::default()
            },
        )?;
        deps.system_prompt.context(
            &ctx,
            PromptContext {
                name: HARNESS_SOURCE.into(),
                order: SystemPrompt::get_context_order(PromptContextOrderName::HarnessSource),
                interpolate: Some(false),
                text: format!(
                    "Current DeepSeek Harness implementation checkout: {}.",
                    json_string(&source_root)
                )
                .into(),
                ..PromptContext::default()
            },
        )?;

        deps.system_prompt.section(
            &ctx,
            PromptSection {
                name: WEB_SURFACE.into(),
                order: SystemPrompt::get_section_order(PromptSectionOrderName::WebSurface),
                interpolate: Some(false),
                text: PromptText::provider(move |context| {
                    Ok(admitted(&handle, context)?
                        .web
                        .clone()
                        .unwrap_or_else(|| WEB_SURFACE_PROMPT.into()))
                }),
                ..PromptSection::default()
            },
        )?;
        let web_app = deps.web_app.clone();
        deps.system_prompt.context(
            &ctx,
            PromptContext {
                name: WEB_SURFACE.into(),
                order: SystemPrompt::get_context_order(PromptContextOrderName::WebSurface),
                interpolate: Some(false),
                text: PromptText::provider(move |_| {
                    Ok(format!(
                        "Current DeepSeek Harness Web GUI URL: {}.",
                        json_string(&web_app.get_advertised_url())
                    ))
                }),
                ..PromptContext::default()
            },
        )?;
        Ok(())
    }
}
