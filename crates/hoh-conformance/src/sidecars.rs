//! Scenario-level helpers of `session-snapshot/src/suite.ts` that need no running agent: scenario
//! skipping, stdout variants, fixture context, header/prompt/schema extraction and the sidecar formats.
use crate::{
    Result,
    normalize::{Context, Normalizer},
    records,
};
use serde_json::{Value, json};

/// The fields of a suite `Scenario` that the helpers below read.
#[derive(Debug, Clone, Default)]
pub struct SnapshotScenario {
    pub name: String,
    pub recorded: bool,
    /// An explicit retained Session generation (`sessionFormat`).
    pub retained_generation: bool,
    pub posix_only: bool,
    pub pwsh_only: bool,
    pub pins_native_windows_stdout: bool,
}

/// `scenarioSkipped`: record mode skips authored scenarios and retained generations, `posixOnly`
/// skips on Windows, `pwshOnly` skips without a usable `pwsh`.
pub fn scenario_skipped(
    scenario: &SnapshotScenario,
    recording: bool,
    platform: &str,
    has_pwsh: Option<bool>,
) -> bool {
    if recording && (!scenario.recorded || scenario.retained_generation) {
        return true;
    }
    if scenario.posix_only && platform == "win32" {
        return true;
    }
    scenario.pwsh_only && has_pwsh != Some(true)
}

/// `stdoutExpectedVariants`: the shared golden first, then the Windows-native sidecar when declared.
pub fn stdout_expected_variants(
    scenario: &SnapshotScenario,
    platform: &str,
) -> Vec<(&'static str, &'static str)> {
    let canonical = ("stdout.expected.jsonl", "canonical");
    if platform != "win32" || !scenario.pins_native_windows_stdout {
        return vec![canonical];
    }
    vec![canonical, ("stdout.expected.windows.jsonl", "native")]
}

/// `fixtureContext`: the fixture's own volatile values; a missing cwd is an impossible sentinel.
pub fn fixture_context(fixture: &str) -> Result<Context> {
    let first = fixture
        .split('\n')
        .find(|line| !hoh_util_values::js_trim(line).is_empty())
        .unwrap_or("{}");
    let header = hoh_util_values::js_json_parse(first).map_err(|e| e.to_string())?;
    Ok(Context {
        cwd: header["cwd"].as_str().unwrap_or("\0no-cwd\0").to_owned(),
        aliases: Vec::new(),
        session_ids: header["id"]
            .as_str()
            .map(|id| vec![id.to_owned()])
            .unwrap_or_default(),
    })
}

/// `normalizedHeaders`: the `data.header` of every `request/header`, normalized, in log order.
pub fn normalized_headers(log: &str, ctx: &Context) -> Result<Vec<Value>> {
    Ok(Normalizer::new()?
        .legacy_log(log, ctx)?
        .into_iter()
        .filter(|r| r["type"] == "request/header")
        .map(|r| r["data"]["header"].clone())
        .collect())
}

/// `normalizedSystemPrompts`: the prompt text of every `system/message` (empty content reads as
/// `''`; a record without a text block is omitted).
pub fn normalized_system_prompts(log: &str, ctx: &Context) -> Result<Vec<String>> {
    Ok(Normalizer::new()?
        .legacy_log(log, ctx)?
        .iter()
        .filter(|r| r["type"] == "system/message")
        .filter_map(|r| {
            let content = r["data"]["message"]["content"].as_array()?;
            if content.is_empty() {
                return Some(String::new());
            }
            content[0]["text"].as_str().map(str::to_owned)
        })
        .collect())
}

/// `normalizedToolSchemas`: the array-valued tool catalog of each header (absent tools → `[]`).
pub fn normalized_tool_schemas(log: &str, ctx: &Context) -> Result<Vec<Vec<Value>>> {
    Ok(normalized_headers(log, ctx)?
        .iter()
        .filter(|header| header.is_object())
        .filter_map(|header| match header.get("tools") {
            None => Some(Vec::new()),
            Some(Value::Array(tools)) => Some(tools.clone()),
            Some(_) => None,
        })
        .collect())
}

/// `formatSystemPromptSnapshot`.
pub fn format_system_prompt_snapshot(prompt: &str, changes: &[&str]) -> String {
    let terminated = |text: &str| {
        if text.ends_with('\n') {
            text.to_owned()
        } else {
            format!("{text}\n")
        }
    };
    let mut snapshot = terminated(prompt);
    for (index, change) in changes.iter().enumerate() {
        snapshot.push_str(&format!(
            "\n<!-- system/message change {} -->\n\n",
            index + 1
        ));
        snapshot.push_str(&terminated(change));
    }
    snapshot
}

/// `parseSystemPromptSnapshot`: the initial prompt and every later change.
pub fn parse_system_prompt_snapshot(snapshot: &str) -> Result<(String, Vec<String>)> {
    let mut parts = crate::compare::prompt_parts(snapshot)?.into_iter();
    let initial = parts.next().unwrap_or_default();
    Ok((initial, parts.collect()))
}

/// `assertChildSystemPromptSnapshot`.
pub fn assert_child_system_prompt_snapshot(
    sidecar: &str,
    class_pin: &str,
    label: &str,
) -> Result<()> {
    if hoh_util_values::js_trim(sidecar).is_empty() {
        return Err(format!("{label} must pin a non-empty prompt"));
    }
    if !sidecar.ends_with('\n') {
        return Err(format!("{label} must end in a newline"));
    }
    if sidecar == class_pin {
        return Err(format!("{label} must differ from its class pin"));
    }
    Ok(())
}

/// `headerChangeCount`: request headers whose reason is `change`.
pub fn header_change_count(log: &str) -> Result<usize> {
    Ok(records(log)?
        .iter()
        .filter(|r| r["type"] == "request/header" && r["data"]["reason"] == "change")
        .count())
}

/// `formatToolSchemasSnapshot`: pretty JSON ending in one newline.
pub fn format_tool_schemas_snapshot(initial: &[Value], changes: &[Vec<Value>]) -> String {
    let value = json!({"initial": initial, "changes": changes});
    format!(
        "{}\n",
        serde_json::to_string_pretty(&value).unwrap_or_default()
    )
}

/// `parseToolSchemasSnapshot`.
pub fn parse_tool_schemas_snapshot(snapshot: &str) -> Result<(Vec<Value>, Vec<Vec<Value>>)> {
    let parsed = hoh_util_values::js_json_parse(snapshot).map_err(|e| e.to_string())?;
    if !parsed.is_object() {
        return Err("acp-snapshot: tool-schema snapshot must be an object".into());
    }
    let shape =
        "acp-snapshot: tool-schema snapshot must carry array-valued initial and changes fields";
    let initial = parsed["initial"].as_array().ok_or(shape)?.clone();
    let changes = parsed["changes"]
        .as_array()
        .ok_or(shape)?
        .iter()
        .map(|set| set.as_array().cloned().ok_or(shape))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok((initial, changes))
}
