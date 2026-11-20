//! Shared composition inputs for in-process child agents.

use hoh_agent::{Agent, AgentOptions, AgentSessionMeta, Live};
use hoh_agent_preset_registry::AgentPresets;
use hoh_error::{BoxError, HarnessError};
use hoh_kernel::Context;
use hoh_permission_presets::PermissionPresets;
use hoh_sandbox::SandboxMode;
use hoh_sandbox_policy::SandboxPolicy;
use hoh_session::Session;
use hoh_system_prompt::{
    PromptContext, PromptContextOrderName, PromptSection, PromptSectionOrderName, SystemPrompt,
    SystemPromptKey,
};
use hoh_tools::ToolRestriction;
use hoh_tools::Tools;
use hoh_user_approval::Approval;
use serde_json::json;

use crate::{
    depth::delegation_depth_of,
    error::{SubagentError, SubagentErrorCode},
};

pub const SUBAGENT_DELEGATION_CONTEXT: &str = "You are a delegated subagent: your permission scope was fixed when you were started and cannot be widened from inside this session — operations that require approval are rejected automatically. When the task needs access beyond that scope, do not retry the denied operation; state the limitation in your reply so the delegating agent can handle it.";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SubagentDepthErrorDetail {
    pub attempted_depth: u64,
    pub max_depth: u64,
}

/// Constructor namespace for the source's `SubagentDepthError` subclass.
pub struct SubagentDepthError;

impl SubagentDepthError {
    pub const NAME: &'static str = "SubagentDepthError";

    #[allow(clippy::new_ret_no_self)]
    pub fn new(attempted_depth: u64, max_depth: u64) -> HarnessError {
        HarnessError::named(
            Self::NAME,
            format!("subagent depth {attempted_depth} exceeds maxDepth {max_depth}"),
            SubagentErrorCode::DepthExceeded.as_str(),
        )
        .with_detail(SubagentDepthErrorDetail {
            attempted_depth,
            max_depth,
        })
    }
}

#[derive(Clone, Debug, Default)]
pub struct ChildComposition {
    pub agent_preset: Option<String>,
    pub persona: Option<String>,
    pub tool_filter: Option<ToolRestriction>,
}

pub async fn apply_child_composition(
    child_ctx: &Context,
    parent: Option<&dyn Agent>,
    composition: &ChildComposition,
) -> Result<(), BoxError> {
    if let Some(preset) = composition.agent_preset.as_deref() {
        let presets = child_ctx.get::<AgentPresets>().ok_or_else(|| -> BoxError {
            Box::new(SubagentError::error(
                "selecting a child agent preset requires the agent-presets service",
                SubagentErrorCode::PresetUnavailable,
            ))
        })?;
        presets.mount(child_ctx, Some(preset)).await?;
    } else if let Some(parent) = parent
        && let Some(presets) = child_ctx.get::<AgentPresets>()
    {
        presets.compose_from(child_ctx, parent.ctx())?;
    }

    let system_prompt = child_ctx
        .get::<SystemPromptKey>()
        .ok_or_else(|| "child composition requires the systemPrompt service".to_string())?;
    system_prompt.context(
        child_ctx,
        PromptContext {
            name: "subagent:delegation".into(),
            // Registered per child, after every plugin's contexts of the same order in the source;
            // the tool plugins start concurrently here, so their tie is pinned by fractions (< 0.75).
            order: SystemPrompt::get_context_order(PromptContextOrderName::SubagentDelegation)
                + 0.75,
            text: SUBAGENT_DELEGATION_CONTEXT.into(),
            ..PromptContext::default()
        },
    )?;
    if let Some(persona) = &composition.persona {
        system_prompt.section(
            child_ctx,
            PromptSection {
                name: "deployment:persona-prefix".into(),
                order: SystemPrompt::get_section_order(
                    PromptSectionOrderName::DeploymentPersonaPrefix,
                ),
                text: persona.clone().into(),
                ..PromptSection::default()
            },
        )?;
    }
    if let Some(filter) = &composition.tool_filter {
        child_ctx
            .get::<Tools>()
            .ok_or_else(|| "child composition requires the tools service".to_string())?
            .restrict(child_ctx, filter.clone())?;
    }
    Ok(())
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DelegatedPolicyOverrides {
    pub permission_preset: Option<String>,
    pub sandbox_mode: Option<SandboxMode>,
    pub approval_policy: Option<&'static str>,
}

pub fn resolve_child_depth(parent: &dyn Agent, max_depth: Option<u64>) -> Result<u64, BoxError> {
    let child_depth = delegation_depth_of(parent)
        .checked_add(1)
        .ok_or_else(|| -> BoxError {
            Box::new(SubagentError::error(
                "subagent child depth exceeds the safe-integer range",
                SubagentErrorCode::InvalidDepth,
            ))
        })?;
    if child_depth > 9_007_199_254_740_991 {
        return Err(Box::new(SubagentError::error(
            "subagent child depth exceeds the safe-integer range",
            SubagentErrorCode::InvalidDepth,
        )));
    }
    if let Some(max_depth) = max_depth
        && child_depth > max_depth
    {
        return Err(Box::new(SubagentError::error(
            format!("subagent depth {child_depth} exceeds maxDepth {max_depth}"),
            SubagentErrorCode::DepthExceeded,
        )));
    }
    Ok(child_depth)
}

pub fn parent_agent_options_for_delegation(parent: &dyn Agent) -> AgentOptions {
    let created = parent.options();
    let Some(header) = parent.session().request_header() else {
        return created.clone();
    };
    let config = &header["config"];
    AgentOptions {
        provider: config
            .get("provider")
            .and_then(serde_json::Value::as_str)
            .map(|value| Live::Value(value.into())),
        model: config
            .get("model")
            .and_then(serde_json::Value::as_str)
            .map(|value| Live::Value(value.into())),
        reasoning_effort: config
            .get("reasoningEffort")
            .and_then(serde_json::Value::as_str)
            .map(|value| Live::Value(value.into())),
        max_tokens: created.max_tokens.clone(),
    }
}

pub fn resolve_child_agent_options(
    parent: &dyn Agent,
    requested: Option<&AgentOptions>,
    _child_depth: u64,
) -> AgentOptions {
    let inherited = parent_agent_options_for_delegation(parent);
    let Some(requested) = requested else {
        return inherited;
    };
    let parent_provider = inherited.provider();
    let parent_model = inherited.model();
    let provider = requested.provider.clone().or(inherited.provider);
    let model = requested.model.clone().or(inherited.model);
    let route_changed = provider.as_ref().and_then(Live::get) != parent_provider
        || model.as_ref().and_then(Live::get) != parent_model;
    AgentOptions {
        provider,
        model,
        reasoning_effort: if route_changed && requested.reasoning_effort.is_none() {
            None
        } else {
            requested
                .reasoning_effort
                .clone()
                .or(inherited.reasoning_effort)
        },
        max_tokens: requested.max_tokens.clone().or(inherited.max_tokens),
    }
}

pub fn child_session_meta(
    parent: &dyn Agent,
    child_depth: u64,
    is_seeded: bool,
    selected_preset: Option<String>,
) -> AgentSessionMeta {
    let header = parent.session().header();
    let agent_preset = selected_preset.or_else(|| {
        parent
            .ctx()
            .get::<AgentPresets>()
            .and_then(|presets| presets.composed_preset(parent.ctx()))
    });
    AgentSessionMeta {
        cwd: header
            .get("cwd")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
        parent_session: Some(parent.id().clone()),
        is_seeded: Some(is_seeded),
        origin: Some("subagent".into()),
        delegation_depth: Some(child_depth),
        agent_preset,
    }
}

pub fn capture_delegated_policy_overrides(
    ctx: &Context,
    parent: &dyn Agent,
) -> Result<DelegatedPolicyOverrides, BoxError> {
    let permission_preset = ctx
        .get::<PermissionPresets>()
        .map(|presets| presets.current(parent.session()))
        .transpose()?
        .filter(|preset| matches!(preset.as_str(), "auto" | "danger-full-access"));
    let sandbox_mode = ctx
        .get::<SandboxPolicy>()
        .map(|policy| policy.override_of(parent.session()))
        .transpose()?
        .flatten();
    Ok(DelegatedPolicyOverrides {
        permission_preset,
        sandbox_mode,
        approval_policy: ctx.get::<Approval>().map(|_| "never"),
    })
}

pub fn append_delegated_policy_overrides(
    child_session: &Session,
    overrides: &DelegatedPolicyOverrides,
) -> Result<(), BoxError> {
    if let Some(mode) = overrides.sandbox_mode {
        child_session.append(
            "sandbox/mode",
            json!({"mode": mode, "source": "delegation"}),
            None,
        )?;
    }
    if let Some(policy) = overrides.approval_policy {
        child_session.append(
            "approval/policy",
            json!({"policy": policy, "source": "delegation"}),
            None,
        )?;
    }
    if let Some(preset) = &overrides.permission_preset {
        child_session.append("permission/preset", json!({"preset": preset}), None)?;
    }
    Ok(())
}
