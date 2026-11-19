//! Unified JSON-value schema DSL, compilation, and tool definition helpers.
//!
//! The author-facing specs are plain JSON values (the source's TypeScript inference has no
//! counterpart): a value schema is an object with `type` or `oneOf`, a parameter schema is a map of
//! property name to value schema where `required: true` marks a required property.

use std::sync::Arc;

use hoh_error::{BoxError, HarnessError};
use hoh_llm::{ContentBlock, ToolSchema};
use hoh_util_values::{JsonValue, is_json_value};
use serde_json::{Map, Value};

use crate::contracts::{
    ConcurrencyFn, ContentHook, PresentCallFn, PresentResultFn, ToolDefinition, ToolExecution,
    ToolOutputDefinition,
};
use crate::errors::ToolsError;
use crate::json_schema::{
    JsonSchemaError, assert_supported_json_schema, js_entries, js_keys, validate_json_schema_value,
};
use crate::presentation::{ToolCallView, ToolResultView};
use crate::shim::BoxFuture;

const ANNOTATION_KEYS: [&str; 4] = ["description", "title", "default", "examples"];

fn author_error(message: String) -> HarnessError {
    JsonSchemaError::new(vec![message])
}

/// Copy own annotation fields for validation by the raw-schema boundary.
fn copy_annotations(source: &Map<String, Value>, target: &mut Map<String, Value>) {
    for key in ANNOTATION_KEYS {
        if let Some(value) = source.get(key) {
            target.insert(key.to_owned(), value.clone());
        }
    }
}

/// Reject author-only keys outside one node's declared vocabulary.
fn assert_author_keys(
    source: &Map<String, Value>,
    path: &str,
    allowed: &[&str],
) -> Result<(), HarnessError> {
    for key in js_keys(source) {
        if !allowed.contains(&key.as_str()) {
            return Err(author_error(format!(
                "{path}.{key} is not supported by the value schema DSL"
            )));
        }
    }
    Ok(())
}

/// Compiled form of one implicit property map.
struct CompiledPropertyMap {
    properties: Map<String, Value>,
    required: Option<Vec<String>>,
}

fn compile_property_map(input: &Value, path: &str) -> Result<CompiledPropertyMap, HarnessError> {
    let Some(map) = input.as_object() else {
        return Err(author_error(format!(
            "{path} must be an object of value schemas"
        )));
    };
    let mut properties = Map::new();
    let mut required = Vec::new();
    for (key, property) in js_entries(map) {
        let property_path = format!("{path}.{key}");
        let Some(property_map) = property.as_object() else {
            return Err(author_error(format!(
                "{property_path} must be a value schema object"
            )));
        };
        if let Some(flag) = property_map.get("required") {
            if flag != &Value::Bool(true) {
                return Err(author_error(format!(
                    "{property_path}.required must be true when present"
                )));
            }
            required.push(key.clone());
        }
        let node = compile_value(property, &property_path, true)?;
        properties.insert(key.clone(), node);
    }
    Ok(CompiledPropertyMap {
        properties,
        required: (!required.is_empty()).then_some(required),
    })
}

fn compile_value(input: &Value, path: &str, allow_required: bool) -> Result<Value, HarnessError> {
    let Some(map) = input.as_object() else {
        return Err(author_error(format!(
            "{path} must be a value schema object"
        )));
    };
    let mut author_keys: Vec<&str> = ANNOTATION_KEYS.to_vec();
    if allow_required {
        author_keys.push("required");
    }
    let mut node = Map::new();

    if map.contains_key("oneOf") {
        let mut allowed = author_keys.clone();
        allowed.extend(["oneOf", "type"]);
        assert_author_keys(map, path, &allowed)?;
        if map.contains_key("type") {
            return Err(author_error(format!(
                "{path} cannot declare both type and oneOf"
            )));
        }
        let Some(branches) = map["oneOf"].as_array() else {
            return Err(author_error(format!(
                "{path}.oneOf must be an array of at least two value schemas"
            )));
        };
        let mut compiled = Vec::with_capacity(branches.len());
        for (index, branch) in branches.iter().enumerate() {
            compiled.push(compile_value(
                branch,
                &format!("{path}.oneOf[{index}]"),
                false,
            )?);
        }
        node.insert("oneOf".into(), Value::Array(compiled));
        copy_annotations(map, &mut node);
        return Ok(Value::Object(node));
    }

    let input_type = map.get("type").and_then(Value::as_str);
    match input_type {
        Some("json") => {
            let mut allowed = author_keys.clone();
            allowed.push("type");
            assert_author_keys(map, path, &allowed)?;
            copy_annotations(map, &mut node);
        }
        Some("object") => {
            let mut allowed = author_keys.clone();
            allowed.extend(["type", "properties", "additionalProperties"]);
            assert_author_keys(map, path, &allowed)?;
            let Some(open) = map.get("additionalProperties").and_then(Value::as_bool) else {
                return Err(author_error(format!(
                    "{path}.additionalProperties must be explicitly true or false"
                )));
            };
            node.insert("type".into(), "object".into());
            copy_annotations(map, &mut node);
            node.insert("additionalProperties".into(), Value::Bool(open));
            if let Some(properties) = map.get("properties") {
                let compiled = compile_property_map(properties, &format!("{path}.properties"))?;
                node.insert("properties".into(), Value::Object(compiled.properties));
                if let Some(required) = compiled.required {
                    node.insert(
                        "required".into(),
                        Value::Array(required.into_iter().map(Value::String).collect()),
                    );
                }
            }
        }
        Some("array") => {
            let mut allowed = author_keys.clone();
            allowed.extend(["type", "items"]);
            assert_author_keys(map, path, &allowed)?;
            node.insert("type".into(), "array".into());
            copy_annotations(map, &mut node);
            if let Some(items) = map.get("items") {
                node.insert(
                    "items".into(),
                    compile_value(items, &format!("{path}.items"), false)?,
                );
            }
        }
        Some(scalar @ ("string" | "number" | "integer" | "boolean" | "null")) => {
            let mut allowed = author_keys.clone();
            allowed.extend(["type", "enum", "const"]);
            assert_author_keys(map, path, &allowed)?;
            node.insert("type".into(), scalar.into());
            copy_annotations(map, &mut node);
            if let Some(entries) = map.get("enum") {
                let Some(entries) = entries.as_array() else {
                    return Err(author_error(format!(
                        "{path}.enum must be a non-empty array of scalar values"
                    )));
                };
                node.insert("enum".into(), Value::Array(entries.clone()));
            }
            if let Some(constant) = map.get("const") {
                node.insert("const".into(), constant.clone());
            }
        }
        _ => {
            return Err(author_error(format!(
                "{path}.type must be string/number/integer/boolean/null/array/object/json, or use oneOf"
            )));
        }
    }
    Ok(Value::Object(node))
}

/// Compile one author-facing value schema to the enforced raw JSON Schema subset. The
/// author-only `json` node becomes an annotation-only schema.
pub fn value_schema_spec_to_json_schema(spec: &Value) -> Result<Value, HarnessError> {
    let schema = compile_value(spec, "schema", false)?;
    assert_supported_json_schema(&schema)?;
    Ok(schema)
}

/// Compile the implicit open parameter object into raw JSON Schema: an object-rooted schema with
/// no implicit-root openness override.
pub fn parameter_schema_spec_to_json_schema(
    spec: &Value,
) -> Result<Map<String, Value>, HarnessError> {
    let compiled = compile_property_map(spec, "parameters")?;
    let mut schema = Map::new();
    schema.insert("type".into(), "object".into());
    schema.insert("properties".into(), Value::Object(compiled.properties));
    if let Some(required) = compiled.required {
        schema.insert(
            "required".into(),
            Value::Array(required.into_iter().map(Value::String).collect()),
        );
    }
    assert_supported_json_schema(&Value::Object(schema.clone()))?;
    Ok(schema)
}

/// The `violations` list of a `ToolArgsError`, attached with `HarnessError::with_detail`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolArgsViolations(pub Vec<String>);

/// Invalid model-generated arguments for a typed tool.
pub struct ToolArgsError;

impl ToolArgsError {
    pub const NAME: &'static str = "ToolArgsError";
    pub const CODE: &'static str = "INVALID_ARGS";

    pub fn new(violations: Vec<String>) -> HarnessError {
        HarnessError::named(
            Self::NAME,
            format!("invalid arguments: {}", violations.join("; ")),
            Self::CODE,
        )
        .with_detail(ToolArgsViolations(violations))
    }

    pub fn violations(error: &HarnessError) -> Option<&[String]> {
        error
            .detail::<ToolArgsViolations>()
            .map(|detail| detail.0.as_slice())
    }
}

/// Validate model-generated arguments against an implicit parameter schema. Returns
/// path-qualified violations; empty means valid.
pub fn validate_args(spec: &Value, args: &Value) -> Result<Vec<String>, HarnessError> {
    let schema = Value::Object(parameter_schema_spec_to_json_schema(spec)?);
    Ok(validate_json_schema_value(&schema, args, ""))
}

/// The output declaration of [`DefineToolOptions`].
pub struct DefineToolOutput {
    /// Schema enforced against every successful body or policy-replaced value (author spec).
    pub schema: Value,
    /// Pure Native/model rendering of one validated canonical value.
    pub render: Arc<dyn Fn(&Value, &Value) -> Vec<ContentBlock> + Send + Sync>,
    /// Pure replayable presentation metadata for native and nested calls.
    pub presentation_meta: Option<Arc<dyn Fn(&Value, &Value) -> Value + Send + Sync>>,
}

type DefineExecute = Arc<
    dyn Fn(Value, Arc<ToolExecution>) -> BoxFuture<'static, Result<Value, BoxError>> + Send + Sync,
>;

/// Options for [`define_tool`]. Arguments reach every callback as the validated JSON object.
pub struct DefineToolOptions {
    /// Tool name (must be unique).
    pub name: String,
    /// Human-readable description sent to the model.
    pub description: String,
    /// Per-property parameter schema compiled to an implicit open object root (author spec).
    pub parameters: Value,
    pub output: DefineToolOutput,
    /// Requests deferred loading of the tool definition.
    pub defer_loading: bool,
    /// Optional positive cooperative timeout budget in milliseconds.
    pub timeout_ms: Option<f64>,
    /// Pure classifier for sibling overlap, called with validated arguments.
    pub is_concurrency_safe: Option<Arc<dyn Fn(&Value) -> bool + Send + Sync>>,
    /// Execute the tool after argument validation.
    pub execute: DefineExecute,
    pub project_content: Option<ContentHook>,
    pub finalize_content: Option<ContentHook>,
    /// Pure pending-state presenter over validated arguments.
    pub present_call: Option<PresentCallFn>,
    /// Pure completed-state presenter over validated arguments.
    pub present_result: Option<PresentResultFn>,
}

impl DefineToolOptions {
    /// Options with the mandatory members; every optional member is absent.
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        parameters: Value,
        output: DefineToolOutput,
        execute: impl Fn(Value, Arc<ToolExecution>) -> BoxFuture<'static, Result<Value, BoxError>>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            parameters,
            output,
            defer_loading: false,
            timeout_ms: None,
            is_concurrency_safe: None,
            execute: Arc::new(execute),
            project_content: None,
            finalize_content: None,
            present_call: None,
            present_result: None,
        }
    }
}

/// Define a first-party tool with strict execution validation. Replay-only presenters validate
/// softly and fall back to generic rendering for obsolete logged arguments.
///
/// A bad `timeout_ms` fails with a plain [`ToolsError::Error`] (a bare `Error` in the source, not a
/// `HarnessError`); an unsupported schema fails with the `JsonSchemaError` of the compiler.
pub fn define_tool(options: DefineToolOptions) -> Result<ToolDefinition, BoxError> {
    if let Some(timeout_ms) = options.timeout_ms
        && (!timeout_ms.is_finite() || timeout_ms <= 0.0)
    {
        return Err(Box::new(ToolsError::Error(format!(
            "defineTool({}): timeoutMs must be a positive finite number",
            options.name
        ))));
    }
    let parameters = parameter_schema_spec_to_json_schema(&options.parameters)?;
    let output_schema = value_schema_spec_to_json_schema(&options.output.schema)?;
    let parameters_schema = Arc::new(Value::Object(parameters.clone()));
    let validate = {
        let schema = parameters_schema.clone();
        Arc::new(move |args: &Value| validate_json_schema_value(&schema, args, ""))
    };

    let user_render = options.output.render;
    let user_meta = options.output.presentation_meta;
    let user_execute = options.execute;
    let output = ToolOutputDefinition {
        schema: output_schema,
        render: Arc::new(move |args, value| Ok(user_render(args, value))),
        presentation_meta: user_meta.map(|meta| -> crate::contracts::PresentationMetaFn {
            Arc::new(move |args, value| Ok(meta(args, value)))
        }),
    };
    let execute_validate = validate.clone();
    let execute: crate::contracts::ExecuteFn = Arc::new(move |args, exec| {
        let violations = execute_validate(&args);
        if !violations.is_empty() {
            let error: BoxError = Box::new(ToolArgsError::new(violations));
            return Box::pin(async move { Err(error) });
        }
        user_execute(args, exec)
    });
    let mut tool = ToolDefinition::new(
        options.name,
        options.description,
        parameters,
        output,
        execute,
    );
    tool.defer_loading = options.defer_loading.then_some(true);
    tool.timeout_ms = options.timeout_ms;
    tool.project_content = options.project_content;
    tool.finalize_content = options.finalize_content;
    // Presentation is display-only and may run on REPLAY of arbitrary logged args (possibly from an
    // older schema), so it must never fail: validate softly and fall back to a generic presentation.
    if let Some(present) = options.present_call {
        let validate = validate.clone();
        tool.present_call = Some(Arc::new(move |args: &Value| -> Option<ToolCallView> {
            validate(args).is_empty().then(|| present(args)).flatten()
        }));
    }
    if let Some(present) = options.present_result {
        let validate = validate.clone();
        tool.present_result = Some(Arc::new(
            move |args: &Value, result: &crate::contracts::ToolResult| -> Option<ToolResultView> {
                validate(args)
                    .is_empty()
                    .then(|| present(args, result))
                    .flatten()
            },
        ));
    }
    if let Some(classify) = options.is_concurrency_safe {
        let validate = validate.clone();
        let classifier: ConcurrencyFn =
            Arc::new(move |args: &Value| validate(args).is_empty() && classify(args));
        tool.is_concurrency_safe = Some(classifier);
    }
    Ok(tool)
}

/// Omit execution and presentation callbacks from a tool definition.
/// `detach_parameters` copies the parameters for a caller-owned inspection result and requires
/// them to be lossless JSON.
pub fn project_tool_schema(
    definition: &ToolDefinition,
    detach_parameters: bool,
) -> Result<ToolSchema, ToolsError> {
    if detach_parameters && !is_json_value(&JsonValue::Object(definition.parameters.clone())) {
        return Err(ToolsError::Error(format!(
            "tool \"{}\" parameters must be lossless JSON before schema projection",
            definition.name
        )));
    }
    Ok(ToolSchema {
        name: definition.name.clone(),
        description: definition.description.clone(),
        parameters: definition.parameters.clone(),
        defer_loading: (definition.defer_loading == Some(true)).then_some(true),
    })
}
