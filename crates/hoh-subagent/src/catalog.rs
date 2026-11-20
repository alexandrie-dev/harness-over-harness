//! Parent-owned durable subagent catalog events and projection.

use std::{collections::HashSet, sync::Arc, time::SystemTime};

use hoh_chunked_list::{ChunkedList, append_chunked_list, iterate_chunked_list};
use hoh_error::BoxError;
use hoh_session::{Session, SessionEvent, SessionHeader, SessionId, SessionLogOffset};
use hoh_session_projection::{ProjectionDefinition, ProjectionWire};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{names::is_agent_name, projection_types::SubagentCatalogEntry};

pub const SUBAGENT_CATALOG_VERSION: u64 = 3;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SubagentCatalogEvent {
    pub version: u64,
    pub child_id: SessionId,
    pub child_created_at: i64,
    pub mode: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl SubagentCatalogEvent {
    fn validate(self) -> Result<Self, BoxError> {
        let valid = match (self.version, self.mode.as_str()) {
            (0 | 1, "one-shot") => self.name.is_none(),
            (0 | 1, "continuable") => self.label.is_some() && self.name.is_none(),
            (1, "unknown") => self.name.is_none(),
            (2, "external") => self.name.is_none(),
            (3, "continuable") => self.label.is_some(),
            (3, "external") => true,
            _ => false,
        };
        if !(0..=9_007_199_254_740_991).contains(&self.child_created_at)
            || !valid
            || self.version == 3 && !self.name.as_deref().is_some_and(is_agent_name)
        {
            return Err("invalid subagent/catalog event data".into());
        }
        Ok(self)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SubagentCatalogState {
    pub inherited_event_count: SessionLogOffset,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head: Option<Arc<ChunkedList<SubagentCatalogEvent>>>,
}

pub fn subagent_catalog_entries(state: &SubagentCatalogState) -> Vec<SubagentCatalogEntry> {
    iterate_chunked_list(state.head.as_ref())
        .map(|data| SubagentCatalogEntry {
            id: data.child_id.clone(),
            created_at: data.child_created_at,
            mode: data.mode.clone(),
            label: data.label.clone(),
            name: data.name.clone(),
        })
        .collect()
}

pub fn subagent_catalog_projection_definition() -> ProjectionDefinition<SubagentCatalogState> {
    ProjectionDefinition {
        key: "subagentCatalog".into(),
        state_schema: Arc::new(|value| {
            let value = value.ok_or_else(|| -> BoxError {
                "subagentCatalog projection state is required".into()
            })?;
            let state: SubagentCatalogState = serde_json::from_value(value.clone())?;
            for event in iterate_chunked_list(state.head.as_ref()) {
                event.clone().validate()?;
            }
            Ok(Arc::new(state))
        }),
        init: Arc::new(|_: &SessionHeader, inherited_event_count| {
            Ok(Arc::new(SubagentCatalogState {
                inherited_event_count,
                head: None,
            }))
        }),
        apply: Arc::new(|state, event: &SessionEvent| {
            if event.event_type() != "subagent/catalog"
                || u64::from(event.seq()) < u64::from(state.inherited_event_count)
            {
                return Ok(state.clone());
            }
            let data: SubagentCatalogEvent =
                serde_json::from_value::<SubagentCatalogEvent>(event.data().clone())?.validate()?;
            Ok(Arc::new(SubagentCatalogState {
                inherited_event_count: state.inherited_event_count,
                head: Some(append_chunked_list(state.head.as_ref(), data)),
            }))
        }),
        state_version: 6,
        wire: Some(ProjectionWire::new(
            Arc::new(|value| {
                let value = value.ok_or_else(|| -> BoxError {
                    "subagentCatalog projection view is required".into()
                })?;
                let entries: Vec<SubagentCatalogEntry> = serde_json::from_value(value.clone())?;
                for entry in entries {
                    let valid_shape = match entry.mode.as_str() {
                        "one-shot" | "unknown" => entry.name.is_none(),
                        "continuable" => entry.label.is_some(),
                        "external" => true,
                        _ => false,
                    };
                    if !(0..=9_007_199_254_740_991).contains(&entry.created_at)
                        || !valid_shape
                        || entry
                            .name
                            .as_deref()
                            .is_some_and(|name| !is_agent_name(name))
                    {
                        return Err("invalid subagentCatalog projection view".into());
                    }
                }
                Ok(Arc::new(value.clone()))
            }),
            Arc::new(|state| {
                Ok(Some(Arc::new(serde_json::to_value(
                    subagent_catalog_entries(state),
                )?)))
            }),
        )),
    }
}

pub fn establish_catalog_child(
    parent: &Session,
    child: &SessionHeader,
    label: &str,
    name: &str,
) -> Result<(), BoxError> {
    parent.append(
        "subagent/catalog",
        json!({
            "version": SUBAGENT_CATALOG_VERSION,
            "childId": child.id,
            "childCreatedAt": child.created_at,
            "mode": "continuable",
            "label": label,
            "name": name,
        }),
        None,
    )?;
    Ok(())
}

pub fn establish_external_catalog_child(
    parent: &Session,
    child_id: &SessionId,
    label: &str,
    name: &str,
) -> Result<(), BoxError> {
    let child_created_at = SystemTime::UNIX_EPOCH.elapsed()?.as_millis() as i64;
    parent.append(
        "subagent/catalog",
        json!({
            "version": SUBAGENT_CATALOG_VERSION,
            "childId": child_id,
            "childCreatedAt": child_created_at,
            "mode": "external",
            "label": label,
            "name": name,
        }),
        None,
    )?;
    Ok(())
}

pub fn catalog_names(entries: &[SubagentCatalogEntry]) -> HashSet<String> {
    entries
        .iter()
        .filter_map(|entry| entry.name.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn header() -> SessionHeader {
        SessionHeader::new("parent".into(), 0)
    }
    fn event(seq: u64, data: serde_json::Value) -> SessionEvent {
        SessionEvent::from_value(
            json!({ "seq": seq, "time": seq, "type": "subagent/catalog", "data": data }),
        )
    }

    #[test]
    fn catalog_keeps_order_and_skips_inherited_prefix() {
        let definition = subagent_catalog_projection_definition();
        let mut state = (definition.init)(&header(), SessionLogOffset::new(1).unwrap()).unwrap();
        state = (definition.apply)(&state, &event(0, json!({
            "version": 3, "childId": "old", "childCreatedAt": 0, "mode": "external", "name": "old"
        }))).unwrap();
        for (seq, id, name) in [(1, "one", "agent-1"), (2, "two", "agent-2")] {
            state = (definition.apply)(&state, &event(seq, json!({
                "version": 3, "childId": id, "childCreatedAt": seq, "mode": "external", "name": name
            }))).unwrap();
        }
        let ids = subagent_catalog_entries(&state)
            .into_iter()
            .map(|entry| entry.id.to_string())
            .collect::<Vec<_>>();
        assert_eq!(ids, ["one", "two"]);
    }

    #[test]
    fn current_catalog_rejects_missing_reserved_or_future_names() {
        let definition = subagent_catalog_projection_definition();
        let state = (definition.init)(&header(), SessionLogOffset::default()).unwrap();
        for data in [
            json!({ "version": 3, "childId": "x", "childCreatedAt": 1, "mode": "external" }),
            json!({ "version": 3, "childId": "x", "childCreatedAt": 1, "mode": "future", "name": "agent-1" }),
            json!({ "version": 3, "childId": "x", "childCreatedAt": 1, "mode": "external", "name": "lead" }),
            json!({ "version": 2, "childId": "x", "childCreatedAt": -1, "mode": "external" }),
            json!({ "version": 2, "childId": "x", "childCreatedAt": 9_007_199_254_740_992_u64, "mode": "external" }),
        ] {
            assert!((definition.apply)(&state, &event(0, data)).is_err());
        }
    }

    #[test]
    fn catalog_names_ignores_unnamed_history() {
        let names = catalog_names(&[
            SubagentCatalogEntry {
                id: "a".into(),
                created_at: 0,
                mode: "one-shot".into(),
                label: None,
                name: None,
            },
            SubagentCatalogEntry {
                id: "b".into(),
                created_at: 0,
                mode: "external".into(),
                label: None,
                name: Some("agent-1".into()),
            },
        ]);
        assert_eq!(names, HashSet::from([String::from("agent-1")]));
    }
}
