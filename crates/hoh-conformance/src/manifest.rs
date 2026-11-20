//! `snapshot.yml` validation, transcribed from `session-snapshot/src/manifest.ts:201-432`
//! (`parseSnapshotManifest`). Messages are the TypeScript ones, prefixed like the source with
//! `session-snapshot: <path>: `.
use crate::{Result, yaml};
use regex::Regex;
use serde_json::{Map, Value};

const PROFILES: &[&str] = &["headless", "sdk", "sdk-minimal", "acp", "web"];
const COVERAGE: &[&str] = &[
    "multi-hop",
    "packed-row",
    "retry-failure",
    "shipped-profile",
    "adjacent-migration",
    "retired-tools",
];
const MAX_SAFE: f64 = 9_007_199_254_740_991.0;

/// Parse and validate one manifest text (`yaml.load(..., JSON_SCHEMA)` without JavaScript tags).
/// Snapshot execution mode for [`writes_current_session_fixtures`] (`SnapshotSessionWriteMode`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WriteMode {
    Replay,
    Record,
    Refresh,
}

/// `writesCurrentSessionFixtures`: only a write-capable run of a scenario that owns its Session
/// fixtures (no `session` reference, no retained `sessionFormat`) tracks the current writer.
pub fn writes_current_session_fixtures(manifest: &Value, mode: WriteMode) -> bool {
    mode != WriteMode::Replay
        && manifest.get("session").is_none()
        && manifest.get("sessionFormat").is_none()
}

pub fn parse(source: &str, path: &str) -> Result<Value> {
    let parsed = yaml::parse(source, false)
        .map_err(|e| format!("session-snapshot: {path}: invalid YAML: {e}"))?;
    validate(&parsed).map_err(|e| format!("session-snapshot: {path}: {e}"))?;
    Ok(parsed)
}

fn record<'a>(value: &'a Value, label: &str) -> Result<&'a Map<String, Value>> {
    value
        .as_object()
        .ok_or_else(|| format!("{label} must be a mapping"))
}

fn exact_keys(value: &Map<String, Value>, allowed: &[&str], label: &str) -> Result<()> {
    let mut unknown: Vec<&str> = value
        .keys()
        .map(String::as_str)
        .filter(|k| !allowed.contains(k))
        .collect();
    unknown.sort_unstable();
    if unknown.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "{label} has unknown field(s): {}",
            unknown.join(", ")
        ))
    }
}

fn name_ok(value: &Value) -> bool {
    value.as_str().is_some_and(|s| {
        !s.is_empty()
            && s.split('-').all(|part| {
                !part.is_empty()
                    && part
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
            })
    })
}

fn name(value: &Value, label: &str) -> Result<()> {
    if name_ok(value) {
        Ok(())
    } else {
        Err(format!("{label} must be a lower-kebab-case name"))
    }
}

fn scenario_source(value: &Value, label: &str) -> Result<()> {
    if value.as_str().is_some_and(|s| {
        s.split('/')
            .all(|part| name_ok(&Value::String(part.into())))
    }) {
        Ok(())
    } else {
        Err(format!(
            "{label} must be a lower-kebab-case name or corpus-relative path"
        ))
    }
}

fn integer(value: &Value) -> Option<f64> {
    value.as_f64().filter(|n| n.fract() == 0.0)
}

fn positive_indexes(value: &Value, label: &str) -> Result<()> {
    let items = value.as_array();
    let valid = items.is_some_and(|items| {
        let numbers: Vec<_> = items.iter().map(integer).collect();
        numbers.iter().all(|n| n.is_some_and(|n| n >= 1.0))
            && numbers
                .iter()
                .enumerate()
                .all(|(i, n)| numbers[..i].iter().all(|p| p != n))
    });
    if valid {
        Ok(())
    } else {
        Err(format!(
            "{label} must be an array of unique positive integers"
        ))
    }
}

fn is_posix_relative(path: &str) -> bool {
    !path.starts_with('/') && !path.contains('\\') && !path.contains('\0')
}

pub fn validate(manifest: &Value) -> Result<()> {
    let root = record(manifest, "manifest")?;
    exact_keys(
        root,
        &[
            "version",
            "scenario",
            "profile",
            "composition",
            "recording",
            "header",
            "replay",
            "platform",
            "permission",
            "environment",
            "workspace",
            "input",
            "session",
            "sessionFormat",
        ],
        "manifest",
    )?;
    if root.get("version").and_then(Value::as_f64) != Some(1.0) {
        return Err("manifest.version must equal 1".into());
    }
    if let Some(scenario) = root.get("scenario") {
        name(scenario, "manifest.scenario")?;
    }
    if !root
        .get("profile")
        .and_then(Value::as_str)
        .is_some_and(|p| PROFILES.contains(&p))
    {
        return Err("manifest.profile must be headless, sdk, sdk-minimal, acp, or web".into());
    }
    if let Some(composition) = root.get("composition") {
        name(composition, "manifest.composition")?;
    }
    if let Some(recording) = root.get("recording")
        && !recording
            .as_str()
            .is_some_and(|r| ["live", "authored"].contains(&r))
    {
        return Err("manifest.recording must be live or authored".into());
    }
    if let Some(header) = root.get("header") {
        let value = record(header, "manifest.header")?;
        exact_keys(
            value,
            &[
                "class",
                "pin",
                "systemPromptSource",
                "toolSchemasSource",
                "childSystemPrompts",
                "childToolSchemas",
                "changes",
                "promptChanges",
            ],
            "manifest.header",
        )?;
        if value.get("pin").is_some_and(|p| *p != Value::Bool(true)) {
            return Err("manifest.header.pin must equal true when present".into());
        }
        for field in ["changes", "promptChanges"] {
            if let Some(count) = value.get(field)
                && !integer(count).is_some_and(|n| n >= 0.0)
            {
                return Err(format!(
                    "manifest.header.{field} must be a non-negative integer"
                ));
            }
        }
        name(
            value.get("class").unwrap_or(&Value::Null),
            "manifest.header.class",
        )?;
        for field in ["systemPromptSource", "toolSchemasSource"] {
            if let Some(source) = value.get(field) {
                scenario_source(source, &format!("manifest.header.{field}"))?;
            }
        }
        for field in ["childSystemPrompts", "childToolSchemas"] {
            if let Some(indexes) = value.get(field) {
                positive_indexes(indexes, &format!("manifest.header.{field}"))?;
            }
        }
    }
    if let Some(replay) = root.get("replay") {
        let value = record(replay, "manifest.replay")?;
        exact_keys(value, &["override"], "manifest.replay")?;
        if value.get("override") != Some(&Value::Bool(true)) {
            return Err("manifest.replay.override must equal true".into());
        }
    }
    if let Some(platform) = root.get("platform")
        && !platform
            .as_str()
            .is_some_and(|p| ["posix", "pwsh"].contains(&p))
    {
        return Err("manifest.platform must be posix or pwsh".into());
    }
    if let Some(permission) = root.get("permission")
        && !permission
            .as_str()
            .is_some_and(|p| ["read-only", "workspace-write", "danger-full-access"].contains(&p))
    {
        return Err(
            "manifest.permission must be read-only, workspace-write, or danger-full-access".into(),
        );
    }
    if let Some(environment) = root.get("environment") {
        let value = record(environment, "manifest.environment")?;
        let variable = Regex::new("^[A-Z][A-Z0-9_]*$").map_err(|e| e.to_string())?;
        if value
            .iter()
            .any(|(k, v)| !variable.is_match(k) || !v.is_string())
        {
            return Err(
                "manifest.environment must map uppercase environment names to strings".into(),
            );
        }
    }
    if let Some(workspace) = root.get("workspace") {
        let value = record(workspace, "manifest.workspace")?;
        exact_keys(value, &["setup", "final", "parent"], "manifest.workspace")?;
        if value.get("final").is_some_and(|f| *f != Value::Bool(true)) {
            return Err("manifest.workspace.final must equal true when present".into());
        }
        if value.get("parent").is_some_and(|p| p != "outside-temp") {
            return Err("manifest.workspace.parent must equal outside-temp".into());
        }
        if let Some(setup) = value.get("setup") {
            name(setup, "manifest.workspace.setup")?;
        }
        if value.is_empty() {
            return Err("manifest.workspace must not be empty".into());
        }
    }
    if let Some(input) = root.get("input") {
        validate_input(input)?;
    }
    let mut owns_session = false;
    if let Some(session) = root.get("session") {
        let value = record(session, "manifest.session")?;
        exact_keys(value, &["source"], "manifest.session")?;
        let source = value
            .get("source")
            .and_then(Value::as_str)
            .filter(|s| !hoh_util_values::js_trim(s).is_empty())
            .ok_or("manifest.session.source must be a non-empty string")?;
        if !is_posix_relative(source) {
            return Err("manifest.session.source must be a relative POSIX path".into());
        }
        owns_session = true;
    }
    if let Some(format) = root.get("sessionFormat") {
        let value = record(format, "manifest.sessionFormat")?;
        exact_keys(value, &["version", "coverage"], "manifest.sessionFormat")?;
        let version = value.get("version").and_then(integer);
        if !version.is_some_and(|n| (0.0..=MAX_SAFE).contains(&n) && !n.is_sign_negative()) {
            return Err(
                "manifest.sessionFormat.version must be a non-negative safe integer".into(),
            );
        }
        let coverage = value.get("coverage").and_then(Value::as_array);
        let valid = coverage.is_some_and(|items| {
            !items.is_empty()
                && items
                    .iter()
                    .all(|c| c.as_str().is_some_and(|c| COVERAGE.contains(&c)))
                && items
                    .iter()
                    .enumerate()
                    .all(|(i, c)| items[..i].iter().all(|p| p != c))
        });
        if !valid {
            return Err("manifest.sessionFormat.coverage must be a non-empty array of unique supported coverage names".into());
        }
        if owns_session {
            return Err(
                "manifest.sessionFormat is only valid when the scenario owns its Session fixtures"
                    .into(),
            );
        }
    }
    Ok(())
}

fn validate_input(input: &Value) -> Result<()> {
    let value = record(input, "manifest.input")?;
    exact_keys(value, &["task", "attachments"], "manifest.input")?;
    if let Some(task) = value.get("task")
        && !task
            .as_str()
            .is_some_and(|t| !hoh_util_values::js_trim(t).is_empty())
    {
        return Err("manifest.input.task must be a non-empty string when present".into());
    }
    if let Some(attachments) = value.get("attachments") {
        let items = attachments
            .as_array()
            .filter(|a| !a.is_empty())
            .ok_or("manifest.input.attachments must be a non-empty array")?;
        let mut ids = Vec::new();
        for (index, item) in items.iter().enumerate() {
            let label = format!("manifest.input.attachments[{index}]");
            let attachment = record(item, &label)?;
            exact_keys(attachment, &["id", "mediaType", "data"], &label)?;
            let id = attachment
                .get("id")
                .and_then(Value::as_str)
                .filter(|id| id.starts_with("sha256:"))
                .ok_or_else(|| format!("{label}.id must start with sha256:"))?;
            if !attachment
                .get("mediaType")
                .and_then(Value::as_str)
                .is_some_and(|m| m.contains('/'))
            {
                return Err(format!("{label}.mediaType must be a MIME type"));
            }
            if !attachment
                .get("data")
                .and_then(Value::as_str)
                .is_some_and(|d| !d.is_empty())
            {
                return Err(format!("{label}.data must be non-empty base64"));
            }
            ids.push(id);
        }
        if ids.iter().enumerate().any(|(i, id)| ids[..i].contains(id)) {
            return Err("manifest.input.attachments must have unique ids".into());
        }
    }
    if value.get("task").is_none() && value.get("attachments").is_none() {
        return Err("manifest.input must declare task or attachments".into());
    }
    Ok(())
}
