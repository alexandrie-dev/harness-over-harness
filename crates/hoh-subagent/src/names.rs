//! Model-facing names and synchronous sibling reservations.

use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex, MutexGuard},
};

use hoh_agent::Agent;
use hoh_error::BoxError;
use hoh_util_values::js_json_stringify;
use serde_json::Value;

use crate::error::{SubagentError, SubagentErrorCode};

pub const ROOT_AGENT_NAME: &str = "lead";

pub fn is_agent_name(value: &str) -> bool {
    let bytes = value.as_bytes();
    (1..=32).contains(&bytes.len())
        && bytes[0].is_ascii_lowercase()
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
        && value != ROOT_AGENT_NAME
}

pub fn join_agent_path(parent_path: Option<&str>, name: Option<&str>) -> Option<String> {
    let (parent_path, name) = (parent_path?, name?);
    Some(if parent_path.is_empty() {
        name.to_owned()
    } else {
        format!("{parent_path}/{name}")
    })
}

pub fn agent_address(path: Option<&str>, id: &str) -> String {
    match path {
        Some("") => ROOT_AGENT_NAME.into(),
        Some(path) => path.into(),
        None => id.into(),
    }
}

pub fn parent_agent_path(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(parent, _)| parent)
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn agent_key(agent: &Arc<dyn Agent>) -> usize {
    Arc::as_ptr(agent) as *const () as usize
}

#[derive(Clone, Default)]
pub struct SiblingNameReservations {
    pending: Arc<Mutex<HashMap<usize, HashSet<String>>>>,
}

pub struct SiblingNameReservation {
    name: String,
    owner: usize,
    pending: Arc<Mutex<HashMap<usize, HashSet<String>>>>,
    released: bool,
}

impl SiblingNameReservation {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn release(&mut self) {
        if self.released {
            return;
        }
        self.released = true;
        let mut pending = lock(&self.pending);
        if let Some(names) = pending.get_mut(&self.owner) {
            names.remove(&self.name);
            if names.is_empty() {
                pending.remove(&self.owner);
            }
        }
    }
}

impl SiblingNameReservations {
    pub fn reserve(
        &self,
        parent: &Arc<dyn Agent>,
        committed: &HashSet<String>,
        requested: Option<&str>,
    ) -> Result<SiblingNameReservation, BoxError> {
        let owner = agent_key(parent);
        let mut store = lock(&self.pending);
        let pending = store.entry(owner).or_default();
        let taken = |name: &str| committed.contains(name) || pending.contains(name);
        let name = match requested {
            None => {
                let mut ordinal = 1_u64;
                loop {
                    let candidate = format!("agent-{ordinal}");
                    if !taken(&candidate) {
                        break candidate;
                    }
                    ordinal += 1;
                }
            }
            Some(requested) if !is_agent_name(requested) => {
                let quoted = js_json_stringify(&Value::String(requested.into()));
                return Err(Box::new(SubagentError::error(
                    format!(
                        "agent name {quoted} is invalid; use 1-32 lower-case letters, digits, or hyphens starting with a letter, other than \"{ROOT_AGENT_NAME}\""
                    ),
                    SubagentErrorCode::InvalidAgentName,
                )));
            }
            Some(requested) if taken(requested) => {
                let quoted = js_json_stringify(&Value::String(requested.into()));
                return Err(Box::new(SubagentError::error(
                    format!(
                        "agent name {quoted} is already used by another subagent you created; choose another name"
                    ),
                    SubagentErrorCode::DuplicateAgentName,
                )));
            }
            Some(requested) => requested.into(),
        };
        pending.insert(name.clone());
        Ok(SiblingNameReservation {
            name,
            owner,
            pending: self.pending.clone(),
            released: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepted_agent_names_match_source_grammar() {
        for value in [
            "a",
            "agent-1",
            "z9",
            "a-b-c",
            &format!("a{}", "0".repeat(31)),
        ] {
            assert!(is_agent_name(value), "{value}");
        }
    }

    #[test]
    fn rejected_agent_names_include_reserved_and_unicode() {
        for value in [
            "",
            "lead",
            "A",
            "1a",
            "a_b",
            "é",
            "a/child",
            &format!("a{}", "0".repeat(32)),
        ] {
            assert!(!is_agent_name(value), "{value}");
        }
    }

    #[test]
    fn path_join_propagates_unnamed_history() {
        assert_eq!(join_agent_path(Some(""), Some("a")), Some("a".into()));
        assert_eq!(join_agent_path(Some("a"), Some("b")), Some("a/b".into()));
        assert_eq!(join_agent_path(None, Some("b")), None);
        assert_eq!(join_agent_path(Some("a"), None), None);
    }

    #[test]
    fn addresses_and_parent_paths_match_model_vocabulary() {
        assert_eq!(agent_address(Some(""), "root-id"), "lead");
        assert_eq!(agent_address(Some("a/b"), "id"), "a/b");
        assert_eq!(agent_address(None, "id"), "id");
        assert_eq!(parent_agent_path("a"), "");
        assert_eq!(parent_agent_path("a/b/c"), "a/b");
    }
}
