//! Skill-activated model-facing Office render, conversion, and recalculation tools.

pub mod content;
pub mod error;
pub mod formats;
pub mod shim;

mod operations;

use std::{
    collections::HashSet,
    sync::{Arc, Mutex, MutexGuard},
};

use content::{OfficeFileValue, OfficeRenderValue, render_file_content, render_images_content};
use formats::{
    OfficeConvertArgs, OfficeOperationName, OfficeRecalculateArgs, check_conversion_arguments,
    check_parameter_names, check_render_arguments, input_family, parse_render_arguments,
};
use hoh_agent::{AgentDisposed, Agents};
use hoh_error::BoxError;
use hoh_fs::Fs;
use hoh_kernel::{Cfg, Context, Inject, Plugin, Service};
use hoh_scope::ScopeKey;
use hoh_session::SessionEventEmitted;
use hoh_tools::{
    DefineToolOptions, DefineToolOutput, FileLocation, GenericCallView, ToolCallView,
    ToolRegistrationOptions, Tools, ToolsResult, define_tool,
};
use hoh_working_directory::WorkingDirectoryKey;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub use error::{OfficeToolError, OfficeToolErrorCode};

pub const NAME: &str = "tool-office";
const OFFICE_TOOL_NAMES: [&str; 3] = ["office_render", "office_convert", "office_recalculate"];

fn default_activation_skills() -> Vec<String> {
    ["office", "office-docx", "office-xlsx", "office-pptx"]
        .into_iter()
        .map(str::to_owned)
        .collect()
}

const fn default_max_images() -> u64 {
    20
}
const fn default_max_image_pixels() -> u64 {
    8_388_608
}
const fn default_max_image_dimension() -> u64 {
    8_192
}
const fn default_max_image_bytes() -> u64 {
    32 * 1_024 * 1_024
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    #[serde(default = "default_activation_skills")]
    pub activation_skills: Vec<String>,
    #[serde(default = "default_max_images")]
    pub max_images: u64,
    #[serde(default = "default_max_image_pixels")]
    pub max_image_pixels: u64,
    #[serde(default = "default_max_image_dimension")]
    pub max_image_dimension: u64,
    #[serde(default = "default_max_image_bytes")]
    pub max_image_bytes: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            activation_skills: default_activation_skills(),
            max_images: default_max_images(),
            max_image_pixels: default_max_image_pixels(),
            max_image_dimension: default_max_image_dimension(),
            max_image_bytes: default_max_image_bytes(),
        }
    }
}

impl Config {
    pub fn validate(&self) -> Result<(), BoxError> {
        if self.activation_skills.is_empty()
            || self.activation_skills.iter().any(|skill| {
                skill.is_empty()
                    || skill.split('-').any(|part| {
                        part.is_empty()
                            || !part
                                .bytes()
                                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
                    })
            })
        {
            return Err(
                "tool-office: activationSkills must be a nonempty list of kebab-case names".into(),
            );
        }
        if !(1..=10_000).contains(&self.max_images) {
            return Err("tool-office: maxImages must be from 1 to 10000".into());
        }
        if !(1..=16_777_216).contains(&self.max_image_pixels) {
            return Err("tool-office: maxImagePixels must be from 1 to 16777216".into());
        }
        if !(1..=16_777_216).contains(&self.max_image_dimension) {
            return Err("tool-office: maxImageDimension must be from 1 to 16777216".into());
        }
        if self.max_image_bytes == 0 || self.max_image_bytes > 9_007_199_254_740_991 {
            return Err("tool-office: maxImageBytes must be a positive safe integer".into());
        }
        Ok(())
    }
}

#[derive(Inject)]
pub struct Deps {
    tools: Service<Tools>,
    fs: Service<Fs>,
    working_directory: Service<WorkingDirectoryKey>,
    agents: Service<Agents>,
}

#[derive(Clone)]
pub(crate) struct Env {
    ctx: Context,
    fs: Service<Fs>,
    working_directory: Service<WorkingDirectoryKey>,
    agents: Service<Agents>,
}

#[derive(Clone)]
struct Activation {
    skills: Arc<HashSet<String>>,
    marked: Arc<Mutex<HashSet<ScopeKey>>>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl Activation {
    fn new(skills: Vec<String>) -> Self {
        Self {
            skills: Arc::new(skills.into_iter().collect()),
            marked: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    fn mark(&self, scope: ScopeKey) {
        lock(&self.marked).insert(scope);
    }

    fn clear(&self, scope: &ScopeKey) {
        lock(&self.marked).remove(scope);
    }

    fn active(&self, agent: &Arc<dyn hoh_tools::Agent>) -> bool {
        let session = agent.session();
        if lock(&self.marked).contains(&agent.scope_key()) {
            return true;
        }
        session
            .request_header()
            .as_ref()
            .and_then(|header| header.get("tools"))
            .and_then(Value::as_array)
            .is_some_and(|tools| {
                tools.iter().any(|tool| {
                    tool.get("name")
                        .and_then(Value::as_str)
                        .is_some_and(|name| OFFICE_TOOL_NAMES.contains(&name))
                })
            })
    }
}

fn call_view(title: String, kind: hoh_tools::ToolCallKind, path: String) -> ToolCallView {
    ToolCallView::Generic(GenericCallView {
        title,
        kind: Some(kind),
        raw_input: None,
        content: None,
        locations: Some(vec![FileLocation { path, line: None }]),
    })
}

fn render_output() -> DefineToolOutput {
    DefineToolOutput {
        schema: json!({
            "type": "object", "additionalProperties": false,
            "properties": {
                "operation": { "type": "string", "const": "render", "required": true },
                "path": { "type": "string", "required": true, "description": "Canonical absolute input path." },
                "pageCount": { "type": "integer", "required": true, "description": "Pages or slides of a document, or visible worksheets." },
                "dpi": { "type": "integer", "required": true },
                "images": { "type": "array", "required": true, "items": {
                    "type": "object", "additionalProperties": false,
                    "properties": {
                        "page": { "type": "integer" }, "sheet": { "type": "string" },
                        "range": { "type": "string" },
                        "attachmentId": { "type": "string", "required": true },
                        "mediaType": { "type": "string", "enum": ["image/png", "image/jpeg", "image/webp"], "required": true },
                        "bytes": { "type": "integer", "required": true },
                        "width": { "type": "integer", "required": true },
                        "height": { "type": "integer", "required": true },
                        "originalDimensions": { "type": "object", "additionalProperties": false,
                            "properties": { "width": { "type": "integer", "required": true }, "height": { "type": "integer", "required": true } }
                        }
                    }
                } },
                "missingFonts": { "type": "array", "required": true, "items": { "type": "string" } }
            }
        }),
        render: Arc::new(|_, value| {
            render_images_content(
                &serde_json::from_value::<OfficeRenderValue>(value.clone())
                    .expect("validated Office render value"),
            )
        }),
        presentation_meta: Some(Arc::new(|_, value| json!({ "path": value["path"] }))),
    }
}

fn file_output(operation: &'static str, formats: &'static [&'static str]) -> DefineToolOutput {
    DefineToolOutput {
        schema: json!({
            "type": "object", "additionalProperties": false,
            "properties": {
                "operation": { "type": "string", "const": operation, "required": true },
                "path": { "type": "string", "required": true, "description": "Canonical absolute input path." },
                "file": { "type": "string", "required": true, "description": "Absolute path of the temporary output file." },
                "format": { "type": "string", "enum": formats, "required": true },
                "bytes": { "type": "integer", "required": true },
                "pageCount": { "type": "integer", "description": "Page count the converter reports for the output." },
                "missingFonts": { "type": "array", "required": true, "items": { "type": "string" } }
            }
        }),
        render: Arc::new(|_, value| {
            render_file_content(
                &serde_json::from_value::<OfficeFileValue>(value.clone())
                    .expect("validated Office file value"),
            )
        }),
        presentation_meta: Some(Arc::new(
            |_, value| json!({ "path": value["path"], "file": value["file"] }),
        )),
    }
}

fn when(activation: Activation) -> ToolRegistrationOptions {
    ToolRegistrationOptions::when_sync(move |context| activation.active(&context.agent))
}

fn render_definition(
    env: Arc<Env>,
    config: Arc<Config>,
) -> Result<hoh_tools::ToolDefinition, BoxError> {
    let mut options = DefineToolOptions::new(
        "office_render",
        "Render document pages, slides, or worksheet areas as images you can view.",
        json!({
            "input": { "type": "string", "required": true, "description": "Path to doc, docx, odt, xls, xlsx, ods, ppt, pptx, odp, or pdf; not modified." },
            "pages": { "type": "array", "items": { "type": "integer" }, "description": "Distinct one-based pages or slides; omit for all. Spreadsheets use sheet and range instead." },
            "sheet": { "type": "string", "description": "Exact worksheet name; omit to render every visible sheet." },
            "range": { "type": "string", "description": "A1 range, such as A1:H40; requires sheet. Omit for the used area." },
            "dpi": { "type": "integer", "description": "Resolution from 24 to 600; default 144." }
        }),
        render_output(),
        move |raw, exec| {
            let env = env.clone();
            let config = config.clone();
            Box::pin(async move {
                let object = raw.as_object().expect("validated tool arguments");
                check_parameter_names("office_render", object)?;
                let args = parse_render_arguments(raw)?;
                let family = input_family(OfficeOperationName::Render, &args.input)?;
                check_render_arguments(&args, family)?;
                Ok(serde_json::to_value(
                    operations::render(&env, &config, args, exec).await?,
                )?)
            })
        },
    );
    options.is_concurrency_safe = Some(Arc::new(|_| true));
    options.present_call = Some(Arc::new(|args| {
        let input = args["input"].as_str()?.to_owned();
        Some(call_view(
            format!("Render {input}"),
            hoh_tools::ToolCallKind::Read,
            input,
        ))
    }));
    define_tool(options)
}

fn convert_definition(env: Arc<Env>) -> Result<hoh_tools::ToolDefinition, BoxError> {
    let mut options = DefineToolOptions::new(
        "office_convert",
        "Convert a document to another format. Return a temporary file to copy to its destination with the shell.",
        json!({
            "input": { "type": "string", "required": true, "description": "Path to doc, docx, odt, xls, xlsx, ods, ppt, pptx, or odp; not modified." },
            "format": { "type": "string", "required": true, "enum": formats::OUTPUT_FORMATS,
                "description": "Word: pdf, docx, odt, txt. Excel: pdf, xlsx, ods, csv. PowerPoint: pdf, pptx, odp." },
            "sheet": { "type": "string", "description": "Exact worksheet name for CSV only; required when the workbook has several sheets." }
        }),
        file_output("convert", formats::OUTPUT_FORMATS),
        move |raw, exec| {
            let env = env.clone();
            Box::pin(async move {
                let object = raw.as_object().expect("validated tool arguments");
                check_parameter_names("office_convert", object)?;
                let args: OfficeConvertArgs = serde_json::from_value(raw)?;
                let family = input_family(OfficeOperationName::Convert, &args.input)?;
                check_conversion_arguments(&args, family)?;
                Ok(serde_json::to_value(
                    operations::convert(&env, args, exec).await?,
                )?)
            })
        },
    );
    options.is_concurrency_safe = Some(Arc::new(|_| false));
    options.present_call = Some(Arc::new(|args| {
        let input = args["input"].as_str()?.to_owned();
        let format = args["format"].as_str()?;
        Some(call_view(
            format!("Convert {input} to {format}"),
            hoh_tools::ToolCallKind::Other,
            input,
        ))
    }));
    define_tool(options)
}

fn recalculate_definition(env: Arc<Env>) -> Result<hoh_tools::ToolDefinition, BoxError> {
    let mut options = DefineToolOptions::new(
        "office_recalculate",
        "Recalculate spreadsheet formulas. Return a temporary workbook to copy to its destination with the shell.",
        json!({
            "input": { "type": "string", "required": true, "description": "Path to an xls, xlsx, or ods workbook; not modified." },
            "format": { "type": "string", "required": true, "enum": formats::RECALCULATION_FORMATS, "description": "Output workbook format." }
        }),
        file_output("recalculate", formats::RECALCULATION_FORMATS),
        move |raw, exec| {
            let env = env.clone();
            Box::pin(async move {
                let object = raw.as_object().expect("validated tool arguments");
                check_parameter_names("office_recalculate", object)?;
                let args: OfficeRecalculateArgs = serde_json::from_value(raw)?;
                input_family(OfficeOperationName::Recalculate, &args.input)?;
                Ok(serde_json::to_value(
                    operations::recalculate(&env, args, exec).await?,
                )?)
            })
        },
    );
    options.is_concurrency_safe = Some(Arc::new(|_| false));
    options.present_call = Some(Arc::new(|args| {
        let input = args["input"].as_str()?.to_owned();
        Some(call_view(
            format!("Recalculate {input}"),
            hoh_tools::ToolCallKind::Other,
            input,
        ))
    }));
    define_tool(options)
}

pub struct ToolOffice;

impl Plugin for ToolOffice {
    const NAME: &'static str = NAME;
    type Config = Config;
    type Deps = Deps;

    async fn apply(ctx: Context, deps: Deps, config: Cfg<Config>) -> Result<(), BoxError> {
        let config = config.get();
        config.validate()?;
        let activation = Activation::new(config.activation_skills.clone());

        let result_activation = activation.clone();
        ctx.on_contained::<ToolsResult>(move |event| {
            if event.exec.name != "skill" || event.result.is_error() {
                return Ok(());
            }
            let Some(agent) = &event.exec.agent else {
                return Ok(());
            };
            let Some(name) = event.exec.arguments.get("name").and_then(Value::as_str) else {
                return Ok(());
            };
            if result_activation.skills.contains(name) {
                result_activation.mark(agent.scope_key());
            }
            Ok(())
        })?;

        let invocation_activation = activation.clone();
        let invocation_agents = deps.agents.clone();
        ctx.on_contained::<SessionEventEmitted>(move |event| {
            if event.event.event_type() != "user/message" {
                return Ok(());
            }
            let source = &event.event.data()["source"];
            let Some(name) = source.get("name").and_then(Value::as_str) else {
                return Ok(());
            };
            if source.get("kind").and_then(Value::as_str) != Some("skill-invocation")
                || !invocation_activation.skills.contains(name)
            {
                return Ok(());
            }
            if let Some(agent) = invocation_agents
                .get(event.session.id())
                .filter(|agent| agent.session() == &event.session)
            {
                invocation_activation.mark(agent.scope_key());
            }
            Ok(())
        })?;

        let disposal_activation = activation.clone();
        ctx.on_contained::<AgentDisposed>(move |event| {
            disposal_activation.clear(&event.agent.scope_key());
            Ok(())
        })?;

        let env = Arc::new(Env {
            ctx: ctx.clone(),
            fs: deps.fs,
            working_directory: deps.working_directory,
            agents: deps.agents,
        });
        deps.tools.register(
            &ctx,
            render_definition(env.clone(), config)?,
            when(activation.clone()),
        )?;
        deps.tools.register(
            &ctx,
            convert_definition(env.clone())?,
            when(activation.clone()),
        )?;
        deps.tools
            .register(&ctx, recalculate_definition(env)?, when(activation))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_defaults_match_the_source() {
        let config: Config = serde_json::from_value(json!({})).unwrap();
        assert_eq!(config.activation_skills, default_activation_skills());
        assert_eq!(config.max_images, 20);
        assert_eq!(config.max_image_pixels, 8_388_608);
        assert_eq!(config.max_image_dimension, 8_192);
        assert_eq!(config.max_image_bytes, 32 * 1_024 * 1_024);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn config_rejects_invalid_skill_names() {
        for skills in [
            vec![],
            vec![""],
            vec!["Office"],
            vec!["has space"],
            vec!["two--parts"],
        ] {
            assert!(
                Config {
                    activation_skills: skills.into_iter().map(str::to_owned).collect(),
                    ..Config::default()
                }
                .validate()
                .is_err()
            );
        }
    }

    #[test]
    fn config_deserialization_rejects_non_string_activation_skills() {
        for raw in [
            json!({ "activationSkills": [1] }),
            json!({ "activationSkills": [null] }),
            json!({ "activationSkills": "office" }),
        ] {
            assert!(serde_json::from_value::<Config>(raw).is_err());
        }
        let config: Config = serde_json::from_value(json!({ "activationSkills": ["office"] }))
            .expect("a list of strings is valid typed config");
        assert!(config.validate().is_ok());
    }

    #[test]
    fn config_rejects_bounds_outside_the_source_schema() {
        let mut config = Config {
            max_images: 0,
            ..Config::default()
        };
        assert!(config.validate().is_err());
        config = Config::default();
        config.max_image_pixels = 16_777_217;
        assert!(config.validate().is_err());
        config = Config::default();
        config.max_image_dimension = 0;
        assert!(config.validate().is_err());
        config = Config::default();
        config.max_image_bytes = 9_007_199_254_740_992;
        assert!(config.validate().is_err());
    }

    #[test]
    fn output_schemas_are_operation_specific_without_one_of() {
        let render = render_output().schema;
        assert_eq!(render["properties"]["operation"]["const"], "render");
        assert!(render.get("oneOf").is_none());
        let converted = file_output("convert", formats::OUTPUT_FORMATS).schema;
        assert_eq!(
            converted["properties"]["format"]["enum"],
            json!(formats::OUTPUT_FORMATS)
        );
        let recalculated = file_output("recalculate", formats::RECALCULATION_FORMATS).schema;
        assert_eq!(
            recalculated["properties"]["operation"]["const"],
            "recalculate"
        );
    }
}
