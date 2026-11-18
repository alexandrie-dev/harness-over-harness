use crate::helpers::*;
use hoh_session_format::{SessionFormatArtifact, SessionFormatJsonObject};
use serde_json::Value;
pub fn historical_child_catalog_source(
    artifact: &SessionFormatArtifact,
) -> Result<SessionFormatJsonObject> {
    let h = &artifact.header;
    if field(h, "origin") != "subagent" || !h.contains_key("parentSession") {
        return Err(refusal(
            "catalog migration requires a subagent child with a direct parent",
        ));
    }
    let descriptors: Vec<_> = artifact
        .events
        .iter()
        .filter(|e| {
            kind(e) == "subagent/descriptor"
                && e.get("seq")
                    .and_then(Value::as_f64)
                    .is_some_and(|n| n >= artifact.inherited_event_count as f64)
        })
        .collect();
    let mut source = SessionFormatJsonObject::new();
    source.insert("childId".into(), field(h, "id").clone());
    source.insert("childCreatedAt".into(), field(h, "createdAt").clone());
    source.insert("descriptorCount".into(), descriptors.len().into());
    source.insert(
        "descriptor".into(),
        descriptors
            .first()
            .map(|e| field(e, "data").clone())
            .unwrap_or(Value::Null),
    );
    child_catalog_fact(&source)?;
    Ok(source)
}
pub fn child_catalog_source(value: &Value) -> Result<&SessionFormatJsonObject> {
    let source = value
        .as_object()
        .filter(|s| field(s, "childId").is_string() && s.contains_key("descriptor"))
        .ok_or_else(|| {
            refusal("catalog migration requires historical child identity and descriptor evidence")
        })?;
    count(source.get("childCreatedAt"), "catalog child creation time")?;
    count(source.get("descriptorCount"), "child descriptor count")?;
    Ok(source)
}
pub fn child_catalog_fact(
    source: &SessionFormatJsonObject,
) -> Result<Option<SessionFormatJsonObject>> {
    let Some(descriptor) = field(source, "descriptor")
        .as_object()
        .filter(|d| (1..=3).any(|n| number_eq(d.get("version"), n)))
    else {
        return Ok(None);
    };
    if !number_eq(source.get("descriptorCount"), 1) {
        return Ok(None);
    }
    if !field(descriptor, "provider").is_string() {
        return Err(refusal(format!(
            "{} has an invalid subagent descriptor provider",
            child_catalog_subject(source)
        )));
    }
    let v1 = number_eq(descriptor.get("version"), 1);
    if !v1
        && !matches!(
            field(descriptor, "mode").as_str(),
            Some("continuable" | "one-shot")
        )
    {
        return Err(refusal(format!(
            "{} has an invalid subagent descriptor mode",
            child_catalog_subject(source)
        )));
    }
    let mut result = SessionFormatJsonObject::new();
    result.insert("version".into(), 0.into());
    result.insert("childId".into(), field(source, "childId").clone());
    result.insert(
        "childCreatedAt".into(),
        field(source, "childCreatedAt").clone(),
    );
    result.insert(
        "mode".into(),
        if v1 {
            "continuable".into()
        } else {
            field(descriptor, "mode").clone()
        },
    );
    if let Some(label) = descriptor.get("label") {
        result.insert("label".into(), label.clone());
    }
    catalog_fact_labeled(
        &Value::Object(result.clone()),
        &child_catalog_subject(source),
    )?;
    Ok(Some(result))
}
pub fn catalog_fact(value: &Value) -> Result<&SessionFormatJsonObject> {
    catalog_fact_labeled(value, "subagent/catalog")
}
pub fn catalog_fact_labeled<'a>(
    value: &'a Value,
    subject: &str,
) -> Result<&'a SessionFormatJsonObject> {
    let valid = value
        .as_object()
        .filter(|v| {
            (0..=2).any(|n| number_eq(v.get("version"), n))
                && field(v, "childId").is_string()
                && matches!(
                    field(v, "mode").as_str(),
                    Some("continuable" | "one-shot" | "external" | "unknown")
                )
                && (number_eq(v.get("version"), 2) == (field(v, "mode") == "external"))
                && !(number_eq(v.get("version"), 0) && field(v, "mode") == "unknown")
                && (field(v, "mode") != "continuable" || field(v, "label").is_string())
                && v.get("label").is_none_or(Value::is_string)
        })
        .ok_or_else(|| {
            error(format!(
                "{subject} requires a supported versioned catalog fact"
            ))
        })?;
    count(valid.get("childCreatedAt"), "catalog child creation time")?;
    Ok(valid)
}
pub fn child_catalog_subject(source: &SessionFormatJsonObject) -> String {
    format!(
        "Session {}{}",
        text(source.get("childId")),
        field(source, "sourcePath")
            .as_str()
            .map(|p| format!(" (raw log: {p})"))
            .unwrap_or_default()
    )
}
