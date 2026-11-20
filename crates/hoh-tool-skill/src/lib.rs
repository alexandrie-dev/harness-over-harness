//! Model-facing skill loader and durable catalog.

mod catalog;
mod gesture;

use std::sync::Arc;

use catalog::{
    catalog_history, catalog_message_in, catalog_source_entries, digest_catalog_entries,
    render_catalog_message, render_catalog_update,
};
use gesture::invoked_skill_names;
use hoh_agent::{AgentPreStep, Agents, PreStepDecision};
use hoh_error::BoxError;
use hoh_kernel::{Cfg, Context, Inject, Plugin, Service};
use hoh_llm::{ContentBlock, NewUserMessage, create_user_message};
use hoh_scope::scope_of;
use hoh_skill::{
    NativeError, SkillDefinition, SkillError, SkillErrorCode, SkillInvocationSource,
    SkillLookupOptions, Skills, is_model_invocable, is_skill_name, is_user_invocable,
    render_skill_content,
};
use hoh_tools::{
    DefineToolOptions, DefineToolOutput, GenericCallView, ToolArgsError, ToolCallKind,
    ToolCallView, ToolRegistrationOptions, Tools, define_tool,
};
use hoh_working_directory::WorkingDirectoryKey;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

pub const NAME: &str = "tool-skill";
pub const INJECT: [&str; 4] = ["agents", "tools", "skills", "workingDirectory"];
const DEFAULT_CATALOG_DESCRIPTION_MAX_LENGTH: f64 = 500.0;

fn default_description_max_length() -> f64 {
    DEFAULT_CATALOG_DESCRIPTION_MAX_LENGTH
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    #[serde(default = "default_description_max_length")]
    pub catalog_description_max_length: f64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            catalog_description_max_length: default_description_max_length(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillCatalogEntry {
    pub name: String,
    pub description: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillCatalogSource {
    pub kind: String,
    pub form: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub update: Option<bool>,
    pub entries: Vec<SkillCatalogEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub incomplete: Option<bool>,
}

impl SkillCatalogSource {
    pub const KIND: &'static str = "skill-catalog";
    pub const FORM: &'static str = "catalog";
}

fn assert_positive_integer(name: &str, value: f64, minimum: f64) -> Result<(), BoxError> {
    if !value.is_finite() || value.fract() != 0.0 || value < minimum {
        return Err(NativeError::Error(format!(
            "tool-skill: {name} must be an integer greater than or equal to {minimum:.0}"
        ))
        .into());
    }
    Ok(())
}

#[derive(Inject)]
pub struct Deps {
    _agents: Service<Agents>,
    tools: Service<Tools>,
    skills: Service<Skills>,
    working_directory: Service<WorkingDirectoryKey>,
}

pub struct ToolSkillPlugin;

impl Plugin for ToolSkillPlugin {
    const NAME: &'static str = NAME;
    type Config = Config;
    type Deps = Deps;

    async fn apply(ctx: Context, deps: Deps, config: Cfg<Config>) -> Result<(), BoxError> {
        let max_length = config.get().catalog_description_max_length;
        assert_positive_integer("catalogDescriptionMaxLength", max_length, 3.0)?;

        let tool_skills = deps.skills.clone();
        let tool_working_directory = deps.working_directory.clone();
        let mut tool_options = DefineToolOptions::new(
            "skill",
            "Load the full instructions for a skill. Call it before acting on a task that names or clearly matches a skill in the session skill catalog.",
            json!({
                "name": {
                    "type": "string",
                    "required": true,
                    "description": "The exact skill name from the available skills list."
                }
            }),
            DefineToolOutput {
                schema: json!({
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {
                        "name": {"type":"string", "required":true},
                        "provider": {"type":"string", "required":true},
                        "resourceBase": {
                            "oneOf": [
                                {
                                    "type":"object",
                                    "additionalProperties":false,
                                    "properties": {
                                        "kind":{"type":"string", "required":true, "const":"directory"},
                                        "path":{"type":"string", "required":true}
                                    }
                                },
                                {
                                    "type":"object",
                                    "additionalProperties":false,
                                    "properties": {
                                        "kind":{"type":"string", "required":true, "const":"url"},
                                        "url":{"type":"string", "required":true}
                                    }
                                },
                                {
                                    "type":"object",
                                    "additionalProperties":false,
                                    "properties": {
                                        "kind":{"type":"string", "required":true, "const":"opaque"},
                                        "description":{"type":"string", "required":true}
                                    }
                                }
                            ]
                        },
                        "content": {"type":"string", "required":true}
                    }
                }),
                render: Arc::new(|_, value| {
                    let skill = SkillDefinition::from_value(value.clone());
                    vec![ContentBlock::text(
                        render_skill_content(&skill)
                            .expect("the validated skill tool output must render"),
                    )]
                }),
                presentation_meta: None,
            },
            move |args, exec| {
                let skills = tool_skills.clone();
                let working_directory = tool_working_directory.clone();
                Box::pin(async move {
                    let name = args
                        .get("name")
                        .and_then(Value::as_str)
                        .expect("validated skill arguments contain name")
                        .to_owned();
                    if !is_skill_name(&name) {
                        return Err(Box::new(ToolArgsError::new(vec![format!(
                            "invalid skill name \"{name}\""
                        )])) as BoxError);
                    }
                    let signal = exec.signal();
                    let cwd = match &exec.agent {
                        Some(agent) => Some(working_directory.ensure(agent, Some(&signal)).await?),
                        None => None,
                    };
                    signal.throw_if_aborted()?;
                    let scope = exec.agent.as_ref().map(|agent| agent.scope_key());
                    let lookup = Arc::new(SkillLookupOptions {
                        cwd,
                        signal: Some(signal.clone()),
                        scope,
                    });
                    let summary = skills
                        .list(lookup.clone())
                        .await?
                        .into_iter()
                        .find(|skill| skill.text("name") == Some(name.as_str()));
                    signal.throw_if_aborted()?;
                    let Some(summary) = summary else {
                        return Err(SkillError::new(
                            format!("skill \"{name}\" is unknown or no longer available"),
                            SkillErrorCode::SkillNotFound,
                        )
                        .into());
                    };
                    if !is_model_invocable(&summary)? {
                        return Err(SkillError::new(
                            format!("skill \"{name}\" is not available for model invocation"),
                            SkillErrorCode::SkillNotModelInvocable,
                        )
                        .into());
                    }
                    let skill = skills.get(&name, lookup).await?;
                    signal.throw_if_aborted()?;
                    let Some(skill) = skill else {
                        return Err(SkillError::new(
                            format!("skill \"{name}\" is unknown or no longer available"),
                            SkillErrorCode::SkillNotFound,
                        )
                        .into());
                    };
                    if !is_model_invocable(&skill)? {
                        return Err(SkillError::new(
                            format!("skill \"{name}\" is not available for model invocation"),
                            SkillErrorCode::SkillNotModelInvocable,
                        )
                        .into());
                    }
                    let mut output = Map::new();
                    output.insert(
                        "name".into(),
                        Value::String(skill.text("name").unwrap_or_default().into()),
                    );
                    output.insert(
                        "provider".into(),
                        Value::String(skill.text("provider").unwrap_or_default().into()),
                    );
                    if let Some(resource_base) = skill.get("resourceBase") {
                        output.insert("resourceBase".into(), resource_base.clone());
                    }
                    output.insert(
                        "content".into(),
                        Value::String(skill.text("content").unwrap_or_default().into()),
                    );
                    Ok(Value::Object(output))
                })
            },
        );
        tool_options.present_call = Some(Arc::new(|args| {
            let name = args.get("name")?.as_str()?;
            Some(ToolCallView::Generic(GenericCallView {
                title: format!("Load skill {name}"),
                kind: Some(ToolCallKind::Read),
                raw_input: Some(Value::String(name.into())),
                content: None,
                locations: None,
            }))
        }));
        let skill_tool = define_tool(tool_options)?;
        deps.tools
            .register(&ctx, skill_tool, ToolRegistrationOptions::default())?;
        let registration_scope = scope_of(&ctx);
        let registered_tool = deps
            .tools
            .get("skill", registration_scope.as_ref(), None)
            .ok_or_else(|| NativeError::Error("tool-skill: registration disappeared".into()))?;

        let invocation_skills = deps.skills.clone();
        let invocation_working_directory = deps.working_directory.clone();
        ctx.on_waterfall::<AgentPreStep, PreStepDecision, _>(move |event, next| {
            let skills = invocation_skills.clone();
            let working_directory = invocation_working_directory.clone();
            async move {
                let decision = next.run().await?;
                let PreStepDecision::Enter {
                    mut messages,
                    starts_request_series,
                } = decision
                else {
                    return Ok(PreStepDecision::Reject);
                };
                let names = invoked_skill_names(&event.messages);
                if names.is_empty() {
                    return Ok(PreStepDecision::Enter {
                        messages,
                        starts_request_series,
                    });
                }
                event.signal.throw_if_aborted()?;
                let cwd = working_directory
                    .ensure(&event.agent, Some(&event.signal))
                    .await?;
                event.signal.throw_if_aborted()?;
                let lookup = Arc::new(SkillLookupOptions {
                    cwd: Some(cwd),
                    signal: Some(event.signal.clone()),
                    scope: Some(event.agent.scope_key()),
                });
                for name in names {
                    let skill = skills.get(&name, lookup.clone()).await?;
                    event.signal.throw_if_aborted()?;
                    let Some(skill) = skill else {
                        continue;
                    };
                    if !is_user_invocable(&skill)? {
                        continue;
                    }
                    messages.push(create_user_message(NewUserMessage {
                        content: vec![ContentBlock::text(render_skill_content(&skill)?)],
                        source: SkillInvocationSource { name }.into(),
                    }));
                }
                Ok(PreStepDecision::Enter {
                    messages,
                    starts_request_series,
                })
            }
        })?;

        let catalog_tools = deps.tools;
        let catalog_skills = deps.skills;
        let catalog_working_directory = deps.working_directory;
        ctx.on_waterfall::<AgentPreStep, PreStepDecision, _>(move |event, next| {
            let tools = catalog_tools.clone();
            let skills = catalog_skills.clone();
            let working_directory = catalog_working_directory.clone();
            let registered_tool = registered_tool.clone();
            async move {
                let decision = next.run().await?;
                let PreStepDecision::Enter {
                    mut messages,
                    starts_request_series,
                } = decision
                else {
                    return Ok(PreStepDecision::Reject);
                };
                event.signal.throw_if_aborted()?;
                let scope = event.agent.scope_key();
                let visible = tools
                    .get("skill", Some(&scope), None)
                    .is_some_and(|tool| Arc::ptr_eq(&tool, &registered_tool));
                let snapshot = if visible {
                    let cwd = working_directory
                        .ensure(&event.agent, Some(&event.signal))
                        .await?;
                    event.signal.throw_if_aborted()?;
                    skills
                        .snapshot(Arc::new(SkillLookupOptions {
                            cwd: Some(cwd),
                            signal: Some(event.signal.clone()),
                            scope: Some(scope),
                        }))
                        .await?
                } else {
                    hoh_skill::SkillCatalogSnapshot {
                        skills: vec![],
                        complete: true,
                    }
                };
                event.signal.throw_if_aborted()?;
                let model_skills = snapshot
                    .skills
                    .into_iter()
                    .filter_map(|skill| match is_model_invocable(&skill) {
                        Ok(true) => Some(Ok(skill)),
                        Ok(false) => None,
                        Err(error) => Some(Err(error)),
                    })
                    .collect::<Result<Vec<_>, BoxError>>()?;
                let entries = catalog_source_entries(&model_skills, max_length);
                let incomplete = !snapshot.complete;
                let digest = digest_catalog_entries(&entries, incomplete);
                let history = catalog_history(event.agent.as_ref())?;
                let existing = catalog_message_in(&messages);
                if history.visible_digest.as_deref() == Some(digest.as_str()) {
                    if let Some(existing) = existing {
                        messages.retain(|message| message.id != existing.message_id);
                    }
                    return Ok(PreStepDecision::Enter {
                        messages,
                        starts_request_series,
                    });
                }
                if let Some(existing) = &existing
                    && digest_catalog_entries(&existing.entries, existing.incomplete) == digest
                {
                    return Ok(PreStepDecision::Enter {
                        messages,
                        starts_request_series,
                    });
                }
                if !history.published && model_skills.is_empty() && !incomplete {
                    if let Some(existing) = existing {
                        messages.retain(|message| message.id != existing.message_id);
                    }
                    return Ok(PreStepDecision::Enter {
                        messages,
                        starts_request_series,
                    });
                }
                let catalog = if history.published {
                    render_catalog_update(entries, incomplete)
                } else {
                    render_catalog_message(entries, incomplete)
                };
                if let Some(existing) = existing {
                    for message in &mut messages {
                        if message.id == existing.message_id {
                            *message = catalog.clone();
                        }
                    }
                } else {
                    messages.push(catalog);
                }
                Ok(PreStepDecision::Enter {
                    messages,
                    starts_request_series,
                })
            }
        })?;
        Ok(())
    }
}
