use crate::{ReplayError, Result, error, json, shim};
use hoh_session::{SessionEvent, SessionLogOffset};
use serde_json::{Value, json};
#[derive(Debug)]
pub struct ParsedSessionHeader {
    pub id: String,
    pub created_at: i64,
    pub inherited_event_count: SessionLogOffset,
}
pub(crate) struct ParsedFixture {
    pub artifact: shim::Artifact,
    pub source_header: Value,
}
pub fn parse_session_log(text: &str) -> Result<Vec<SessionEvent>> {
    Ok(parse_fixture(text)?.artifact.events)
}
pub fn parse_session_header(text: &str) -> Result<ParsedSessionHeader> {
    let p = parse_fixture(text)?;
    Ok(ParsedSessionHeader {
        id: p.artifact.header["id"].as_str().unwrap_or_default().into(),
        created_at: p.artifact.header["createdAt"].as_i64().unwrap_or(0),
        inherited_event_count: p.artifact.inherited_event_count,
    })
}
pub(crate) fn parse_fixture(text: &str) -> Result<ParsedFixture> {
    let mut restore = None;
    let mut source_header = Value::Null;
    let mut header_line = 1;
    let mut body_kind = None;
    let mut next = 0_u64;
    let mut event_lines = vec![];
    for (index, line) in text.split('\n').enumerate() {
        if hoh_util_values::js_trim(line).is_empty() {
            continue;
        }
        let line_number = index + 1;
        let mut row = hoh_util_values::js_json_parse(line).map_err(|cause| {
            Box::new(ReplayError::Format {
                message: format!("session snapshot line {line_number} contains invalid JSON"),
                cause: Box::new(cause),
            }) as hoh_error::BoxError
        })?;
        if !row.is_object() {
            return Err(error(format!(
                "session snapshot line {line_number} must be a JSON object"
            )));
        }
        if restore.is_none() {
            source_header = row.clone();
            header_line = line_number;
            if row["version"] == 0 && row.get("delegationDepth").is_none() {
                row["delegationDepth"] = json!(0);
            }
            if let Some(cwd) = row["cwd"]
                .as_str()
                .filter(|s| *s == "{{cwd}}" || s.starts_with("{{cwd}}/"))
            {
                row["cwd"] = json!(cwd.replacen("{{cwd}}", "/dsh-snapshot-cwd", 1));
            }
            restore = Some(shim::Restore::new(row).map_err(|e| located(e, line_number))?);
            continue;
        }
        normalize_row(&mut row);
        let packed = ["text-chunks", "reasoning-chunks", "tool-call-chunks"]
            .contains(&row["type"].as_str().unwrap_or(""));
        let (seq, time) = if packed {
            ("seq0", "time0")
        } else {
            ("seq", "time")
        };
        let complete = row.get(seq).is_some();
        if complete != row.get(time).is_some() {
            return Err(error(format!(
                "session snapshot line {line_number} must contain both {seq} and {time}, or neither"
            )));
        }
        if body_kind.is_some_and(|previous| previous != complete) {
            return Err(error(format!(
                "session snapshot line {line_number} cannot mix projected and complete body rows"
            )));
        }
        body_kind = Some(complete);
        if !complete {
            row[seq] = json!(next);
            row[time] = json!(0);
        }
        let cardinality = if packed {
            row["data"][if row["type"] == "tool-call-chunks" {
                "args"
            } else {
                "texts"
            }]
            .as_array()
            .map_or(1, |a| a.len().max(1))
        } else {
            1
        };
        next += cardinality as u64;
        event_lines.extend(std::iter::repeat_n(line_number, cardinality));
        // The header branch establishes a restore before any body row.
        restore
            .as_mut()
            .expect("header precedes body")
            .decode_row(row)
            .map_err(|e| located(e, line_number))?;
    }
    let restore =
        restore.ok_or_else(|| error("session snapshot must start with a session header"))?;
    let artifact = restore.finish().map_err(|e| {
        let detail = e.to_string();
        let line = regex::Regex::new(
            r"(?:Session event | at seq |inherited Session cut |seed event at index )([0-9]+)",
        )
        .ok()
        .and_then(|r| r.captures(&detail).and_then(|c| c[1].parse::<usize>().ok()))
        .and_then(|n| event_lines.get(n))
        .copied()
        .unwrap_or(header_line);
        located(e, line)
    })?;
    Ok(ParsedFixture {
        artifact,
        source_header,
    })
}
fn located(e: hoh_error::BoxError, line: usize) -> hoh_error::BoxError {
    let message = format!("session snapshot line {line}: {e}");
    if matches!(
        e.downcast_ref::<ReplayError>(),
        Some(ReplayError::UnsupportedMigration { .. })
    ) {
        Box::new(ReplayError::UnsupportedMigration {
            message,
            cause: Some(e),
        })
    } else {
        Box::new(ReplayError::Format { message, cause: e })
    }
}
fn normalize_row(row: &mut Value) {
    if row["type"] != "request/header" {
        return;
    }
    let Some(header) = row
        .pointer_mut("/data/header")
        .and_then(Value::as_object_mut)
    else {
        return;
    };
    if header.get("tools").is_some_and(|v| v == "{{tools}}") {
        header.shift_remove("tools");
    } else if let Some(tools) = header.get_mut("tools")
        && let Some(names) = tools.as_array().filter(|a| {
            !a.is_empty() && a.iter().all(|v| v.as_str().is_some_and(|s| !s.is_empty()))
        })
    {
        *tools = json!(
            names
                .iter()
                .map(|v| json!({"name":v,"description":"","parameters":{}}))
                .collect::<Vec<_>>()
        );
    }
}
pub fn prepare_session_snapshot_fixture_for_comparison(text: &str) -> Result<String> {
    let p = parse_fixture(text)?;
    let catalog = hoh_session_format_catalog::session_format_catalog()?;
    let mut logical_header = p.artifact.header;
    // The temporary restore shim retains the physical tag; catalog metadata excludes it.
    let logical_header = logical_header
        .as_object_mut()
        .ok_or_else(|| error("restored Session header must be an object"))?;
    logical_header.shift_remove("type");
    let mut header = Value::Object(
        catalog.encode_current_header(logical_header, p.artifact.inherited_event_count.get())?,
    );
    if p.source_header["cwd"]
        .as_str()
        .is_some_and(|s| s == "{{cwd}}" || s.starts_with("{{cwd}}/"))
    {
        header["cwd"] = p.source_header["cwd"].clone();
    }
    let source: Vec<_> = text
        .lines()
        .filter(|s| !hoh_util_values::js_trim(s).is_empty())
        .skip(1)
        .map(hoh_util_values::js_json_parse)
        .collect::<std::result::Result<_, _>>()?;
    let mut requests = source.iter().filter(|r| r["type"] == "request/header");
    let mut lines = vec![json(&header)];
    for event in p.artifact.events {
        let logical_event = event
            .as_value()
            .as_object()
            .ok_or_else(|| error("restored Session event must be an object"))?;
        let mut value = Value::Object(catalog.encode_current_event(logical_event)?);
        if event.event_type() == "request/header"
            && let Some(source) = requests.next()
            && source["data"]["header"]["tools"] == "{{tools}}"
        {
            value["data"]["header"]["tools"] = json!("{{tools}}");
        }
        lines.push(json(&value));
    }
    Ok(lines.join("\n") + if text.ends_with('\n') { "\n" } else { "" })
}
