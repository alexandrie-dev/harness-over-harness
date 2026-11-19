//! Historical direct-child descriptors decoded by their frozen format owners.
use crate::{
    Result,
    error::error,
    format::JsonlCompression,
    generation::{JsonlGenerationSourceChangedError, JsonlPhysicalIdentity},
};
use hoh_session::{SessionHeader, SessionId};
use hoh_timeout::AbortSignal;
use serde_json::{Value, json};
pub struct CatalogSource {
    pub header: SessionHeader,
    pub path: String,
}
fn same_identity_value(left: Option<&Value>, right: Option<&Value>) -> bool {
    match (left, right) {
        (Some(Value::Number(a)), Some(Value::Number(b))) => a.as_f64() == b.as_f64(),
        _ => left == right,
    }
}
pub struct CatalogFailure {
    pub path: String,
    pub error: hoh_error::BoxError,
}
pub struct PreparedCatalogFacts {
    pub facts: Vec<Value>,
    pub failures: Vec<CatalogFailure>,
    witnesses: Vec<(String, JsonlPhysicalIdentity)>,
}
impl PreparedCatalogFacts {
    pub async fn validate(&self) -> Result<()> {
        for (path, expected) in &self.witnesses {
            let actual = crate::io::FileSystem::default().identity(path).await?;
            if actual != *expected {
                return Err(Box::new(JsonlGenerationSourceChangedError {
                    path: path.clone(),
                }));
            }
        }
        Ok(())
    }
}
pub async fn prepare_catalog_facts(
    parent_id: &SessionId,
    sources: &[CatalogSource],
    compression: JsonlCompression,
    signal: &AbortSignal,
) -> Result<PreparedCatalogFacts> {
    let mut result = PreparedCatalogFacts {
        facts: vec![],
        failures: vec![],
        witnesses: vec![],
    };
    for source in sources {
        signal.throw_if_aborted()?;
        let filename = hoh_util_workspace_path::node_path::basename(&source.path);
        if crate::format::parse_generation_log_filename(&filename, compression).is_none() {
            return Err(Box::new(
                hoh_session_format::SessionFormatError::unsupported_migration(format!(
                    "unrecognized historical child generation {}",
                    source.path
                )),
            ));
        }
        let identity = crate::io::FileSystem::default()
            .identity(&source.path)
            .await?;
        result.witnesses.push((source.path.clone(), identity));
        let version = crate::format::parse_generation_log_filename(&filename, compression)
            .ok_or_else(|| error("invalid generation"))?;
        let catalog = if version <= 3 {
            hoh_session_format_catalog::historical_session_format_catalog()?
        } else {
            hoh_session_format_catalog::session_format_catalog()?
        };
        let decoded = crate::generation::read_decoded_jsonl_source(
            &source.path,
            version,
            compression,
            &crate::catalog_adapter::CatalogAdapter(catalog),
            Some(signal),
        )
        .await;
        let unavailable = json!({"childId":source.header.id,"childCreatedAt":source.header.created_at,"descriptorCount":0,"descriptor":null,"sourcePath":source.path});
        let (artifact, identity) = match decoded {
            Ok(value) => value,
            Err(error) => {
                signal.throw_if_aborted()?;
                result.failures.push(CatalogFailure {
                    path: source.path.clone(),
                    error,
                });
                result.facts.push(unavailable);
                continue;
            }
        };
        if let Some(witness) = result.witnesses.last_mut() {
            witness.1 = identity;
        }
        let expected = serde_json::to_value(&source.header)?;
        let header = &artifact.header;
        if header.get("id") != expected.get("id")
            || !same_identity_value(header.get("createdAt"), expected.get("createdAt"))
            || header.get("parentSession").and_then(Value::as_str) != Some(parent_id.as_str())
            || header.get("origin").and_then(Value::as_str) != Some("subagent")
            || ["cwd", "isSeeded", "delegationDepth", "agentPreset"]
                .iter()
                .any(|key| !same_identity_value(header.get(*key), expected.get(*key)))
        {
            return Err(Box::new(JsonlGenerationSourceChangedError {
                path: source.path.clone(),
            }));
        }
        match hoh_session_format_v3_to_v4::historical_child_catalog_source(&artifact) {
            Ok(mut fact) => {
                fact.insert("sourcePath".into(), source.path.clone().into());
                result.facts.push(fact.into());
            }
            Err(error) => {
                result.failures.push(CatalogFailure {
                    path: source.path.clone(),
                    error: Box::new(error),
                });
                result.facts.push(unavailable);
            }
        }
    }
    Ok(result)
}
