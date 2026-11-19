//! Every scenario shipped in `scenarios/` must call the window profile's tools with the arguments
//! their schemas require: the real engine answers `MALFORMED_RESPONSE` for a call that lacks one.

use hoh_llm_openai_mock::Scenario;
use serde_json::Value;

/// Required argument names of the tools of the window profile (`required: true` in the schemas of
/// `hoh-tool-bash` and `hoh-tool-fs`). A tool missing here fails the test: add its schema first.
fn required_arguments(tool: &str) -> Option<&'static [&'static str]> {
    match tool {
        "bash" => Some(&["command", "description"]),
        "read" => Some(&["file_path"]),
        "write" => Some(&["file_path", "content"]),
        _ => None,
    }
}

#[test]
fn every_scripted_tool_call_carries_the_fields_its_schema_requires() {
    let directory = concat!(env!("CARGO_MANIFEST_DIR"), "/scenarios");
    let mut checked = 0;
    let mut problems = Vec::new();
    for entry in std::fs::read_dir(directory).expect("scenarios directory") {
        let path = entry.expect("directory entry").path();
        if path.extension().is_none_or(|extension| extension != "json") {
            continue;
        }
        let scenario = Scenario::from_path(&path).expect("scenario loads");
        for (index, response) in scenario.responses.iter().enumerate() {
            for call in &response.tool_calls {
                checked += 1;
                let place = format!(
                    "{} response {index} call {} ({})",
                    path.display(),
                    call.id,
                    call.name
                );
                let Some(required) = required_arguments(&call.name) else {
                    problems.push(format!("{place}: no schema known for this tool"));
                    continue;
                };
                let Some(arguments) = call.arguments.as_object() else {
                    problems.push(format!("{place}: arguments are not an object"));
                    continue;
                };
                for field in required {
                    match arguments.get(*field) {
                        Some(Value::String(text)) if !text.trim().is_empty() => {}
                        other => problems.push(format!(
                            "{place}: required `{field}` is missing or blank ({other:?})"
                        )),
                    }
                }
                // "Required with sandbox_permissions" (`hoh-tool-bash` schema).
                if arguments.contains_key("sandbox_permissions")
                    && !arguments.contains_key("justification")
                {
                    problems.push(format!(
                        "{place}: sandbox_permissions without justification"
                    ));
                }
            }
        }
    }
    assert!(checked > 0, "no tool calls found in {directory}");
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}
