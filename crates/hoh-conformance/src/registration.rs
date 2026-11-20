//! The registration contract of `suite.ts` (`defineAcpSnapshotSuite`): one pinning scenario per header
//! class, valid sidecar sources and counts, plus the checks on shared and committed sidecar content.
use crate::Result;
use std::collections::{BTreeMap, HashMap};

/// The declaration fields of a suite scenario that registration validates.
#[derive(Debug, Clone, Default)]
pub struct Declaration {
    pub name: String,
    pub pins_header: bool,
    /// `None` is the `default` class.
    pub header_class: Option<String>,
    pub system_prompt_source: Option<String>,
    pub tool_schemas_source: Option<String>,
    pub expected_header_changes: Option<u64>,
    pub expected_prompt_changes: Option<u64>,
}

impl Declaration {
    fn class(&self) -> &str {
        self.header_class.as_deref().unwrap_or("default")
    }
}

/// A scenario's generated claim on a shared snapshot file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharedSnapshotClaim {
    pub scenario: String,
    pub content: String,
}

fn source_for<'a>(
    pinning: &'a Declaration,
    by_name: &HashMap<&str, &'a Declaration>,
    prompt: bool,
    label: &str,
) -> Result<&'a Declaration> {
    let own = if prompt {
        &pinning.system_prompt_source
    } else {
        &pinning.tool_schemas_source
    };
    let source_name = own.as_deref().unwrap_or(&pinning.name);
    let source = by_name.get(source_name).copied().ok_or_else(|| {
        format!(
            "acp-snapshot: {} names unknown {label} source \"{source_name}\"",
            pinning.name
        )
    })?;
    if !source.pins_header {
        return Err(format!(
            "acp-snapshot: {} names non-pinning {label} source \"{source_name}\"",
            pinning.name
        ));
    }
    let source_own = if prompt {
        &source.system_prompt_source
    } else {
        &source.tool_schemas_source
    };
    if source_own.as_deref().is_some_and(|s| s != source.name) {
        return Err(format!(
            "acp-snapshot: {} names {label} source \"{source_name}\", which does not own its sidecar",
            pinning.name
        ));
    }
    let (count_field, mine, theirs) = if prompt {
        (
            "expectedPromptChanges",
            pinning.expected_prompt_changes,
            source.expected_prompt_changes,
        )
    } else {
        (
            "expectedHeaderChanges",
            pinning.expected_header_changes,
            source.expected_header_changes,
        )
    };
    if theirs.unwrap_or(0) != mine.unwrap_or(0) {
        return Err(format!(
            "acp-snapshot: {} and {source_name} declare different {count_field} counts for shared {label}",
            pinning.name
        ));
    }
    Ok(source)
}

/// Validate a scenario table the way `defineAcpSnapshotSuite` does at registration.
pub fn validate_registration(scenarios: &[Declaration]) -> Result<()> {
    let mut by_name: HashMap<&str, &Declaration> = HashMap::new();
    for scenario in scenarios {
        if by_name.insert(&scenario.name, scenario).is_some() {
            return Err(format!(
                "acp-snapshot: duplicate scenario name \"{}\"",
                scenario.name
            ));
        }
        let declared = [
            (
                "systemPromptSource",
                scenario.system_prompt_source.is_some(),
            ),
            ("toolSchemasSource", scenario.tool_schemas_source.is_some()),
            (
                "expectedHeaderChanges",
                scenario.expected_header_changes.is_some(),
            ),
            (
                "expectedPromptChanges",
                scenario.expected_prompt_changes.is_some(),
            ),
        ];
        for (field, present) in declared {
            if present && !scenario.pins_header {
                return Err(format!(
                    "acp-snapshot: {}.{field} is only valid on a header-pinning scenario",
                    scenario.name
                ));
            }
        }
    }
    let mut pinning: BTreeMap<&str, &Declaration> = BTreeMap::new();
    for scenario in scenarios.iter().filter(|s| s.pins_header) {
        if let Some(existing) = pinning.insert(scenario.class(), scenario) {
            return Err(format!(
                "acp-snapshot: header class \"{}\" pinned by both {} and {}",
                scenario.class(),
                existing.name,
                scenario.name
            ));
        }
    }
    for scenario in scenarios {
        if !pinning.contains_key(scenario.class()) {
            return Err(format!(
                "acp-snapshot: no scenario pins the request-header content of class \"{}\" (needed by {})",
                scenario.class(),
                scenario.name
            ));
        }
    }
    for pin in scenarios.iter().filter(|s| s.pins_header) {
        source_for(pin, &by_name, true, "system-prompt snapshot")?;
        source_for(pin, &by_name, false, "tool-schema snapshot")?;
    }
    Ok(())
}

/// Record one scenario's generated content for a shared snapshot; a later claimant must match it.
pub fn claim_shared_snapshot(
    claims: &mut HashMap<String, SharedSnapshotClaim>,
    source: &str,
    scenario: &str,
    content: &str,
) -> Result<()> {
    match claims.get(source) {
        Some(previous) if previous.content != content => Err(format!(
            "acp-snapshot: shared snapshot {source} diverged between {} and {scenario}",
            previous.scenario
        )),
        Some(_) => Ok(()),
        None => {
            claims.insert(
                source.to_owned(),
                SharedSnapshotClaim {
                    scenario: scenario.to_owned(),
                    content: content.to_owned(),
                },
            );
            Ok(())
        }
    }
}

/// Reject byte-identical committed snapshots stored under different paths.
pub fn assert_unique_snapshot_contents(kind: &str, snapshots: &[(&str, &str)]) -> Result<()> {
    let mut first: HashMap<&str, &str> = HashMap::new();
    for (path, content) in snapshots {
        if let Some(previous) = first.get(content) {
            return Err(format!(
                "acp-snapshot: identical {kind} snapshots appear in {previous} and {path}; reuse one source"
            ));
        }
        first.insert(content, path);
    }
    Ok(())
}
