//! Durable direct-child and recursive descendant discovery.

use std::collections::HashSet;

use hoh_error::{BoxError, HarnessError};
use hoh_kernel::Context;
use hoh_session::{SessionId, Sessions};
use hoh_session_query::{
    SessionObservationOptions, SessionQuery, SessionQueryError, SessionQueryErrorCode,
};
use hoh_timeout::AbortSignal;

use crate::{
    control_types::{SubagentActivity, SubagentDiagnosticReason, SubagentListEntry},
    error::{SubagentError, SubagentErrorCode},
    names::{is_agent_name, join_agent_path},
    projection_types::{SubagentCatalogEntry, SubagentCatalogEntryFields},
};

pub use crate::control_types::SubagentDescendantListEntry;

/// Read one parent's durable catalog through the live-preferred query service.
pub async fn list_children(
    ctx: &Context,
    parent_session_id: &SessionId,
    signal: Option<AbortSignal>,
) -> Result<Vec<SubagentCatalogEntry>, BoxError> {
    let query = ctx.get::<SessionQuery>().ok_or_else(|| {
        subagent_error(
            "listing subagents requires the sessionQuery service (load @deepseek-ai/dsh-session-query)",
            SubagentErrorCode::SubagentControlQueryUnavailable,
        )
    })?;
    let observation = query
        .observe_session(
            parent_session_id,
            SessionObservationOptions {
                signal,
                ..SessionObservationOptions::default()
            },
        )
        .await?;
    let value = observation
        .projections()
        .and_then(|snapshot| snapshot.values.get("subagentCatalog"))
        .ok_or_else(|| {
            subagent_error(
                "listing subagents requires the registered subagentCatalog projection",
                SubagentErrorCode::SubagentControlProjectionsUnavailable,
            )
        })?;
    serde_json::from_value((**value).clone()).map_err(Into::into)
}

/// Walk reachable catalogs in stable pre-order without loading agents.
pub async fn list_descendants(
    ctx: &Context,
    root_session_id: &SessionId,
    signal: Option<AbortSignal>,
) -> Result<Vec<SubagentDescendantListEntry>, BoxError> {
    let sessions = ctx.get::<Sessions>().ok_or_else(|| {
        subagent_error(
            "listing subagents requires the session store (load @deepseek-ai/dsh-session)",
            SubagentErrorCode::SubagentControlSessionStoreUnavailable,
        )
    })?;
    assert_not_cancelled(signal.as_ref())?;
    let root = list_children(ctx, root_session_id, signal.clone()).await?;
    assert_not_cancelled(signal.as_ref())?;

    struct Position {
        entry: SubagentCatalogEntry,
        parent_id: SessionId,
        depth: u64,
        parent_path: Option<String>,
    }
    let mut stack = root
        .into_iter()
        .rev()
        .map(|entry| Position {
            entry,
            parent_id: root_session_id.clone(),
            depth: 1,
            parent_path: Some(String::new()),
        })
        .collect::<Vec<_>>();
    let mut visited = HashSet::from([root_session_id.clone()]);
    let mut result = Vec::new();
    while let Some(position) = stack.pop() {
        if !visited.insert(position.entry.id.clone()) {
            continue;
        }
        let path = join_agent_path(
            position.parent_path.as_deref(),
            position.entry.name.as_deref(),
        );
        let mut children = Vec::new();
        if position.entry.mode != "external" {
            assert_not_cancelled(signal.as_ref())?;
            match list_children(ctx, &position.entry.id, signal.clone()).await {
                Ok(found) => children = found,
                Err(error) => {
                    assert_not_cancelled(signal.as_ref())?;
                    if HarnessError::of(error.as_ref())
                        .is_some_and(|error| error.is_named(SubagentError::NAME))
                    {
                        return Err(error);
                    }
                    let reason = query_failure_reason(&error);
                    result.push(SubagentDescendantListEntry {
                        entry: SubagentListEntry::Diagnostic {
                            id: position.entry.id,
                            reason,
                        },
                        parent_id: position.parent_id,
                        depth: position.depth,
                        path,
                    });
                    continue;
                }
            }
            assert_not_cancelled(signal.as_ref())?;
        }

        if position.entry.mode == "unknown" {
            result.push(SubagentDescendantListEntry {
                entry: SubagentListEntry::Diagnostic {
                    id: position.entry.id.clone(),
                    reason: SubagentDiagnosticReason::Unsupported,
                },
                parent_id: position.parent_id,
                depth: position.depth,
                path: path.clone(),
            });
        } else {
            result.push(SubagentDescendantListEntry {
                entry: SubagentListEntry::Child {
                    id: position.entry.id.clone(),
                    activity: if sessions.get(&position.entry.id).is_some() {
                        SubagentActivity::Running
                    } else {
                        SubagentActivity::Inactive
                    },
                    fields: SubagentCatalogEntryFields {
                        mode: position.entry.mode,
                        label: position.entry.label,
                        name: position.entry.name,
                    },
                    has_children: !children.is_empty(),
                },
                parent_id: position.parent_id,
                depth: position.depth,
                path: path.clone(),
            });
        }
        for child in children.into_iter().rev() {
            stack.push(Position {
                entry: child,
                parent_id: position.entry.id.clone(),
                depth: position.depth + 1,
                parent_path: path.clone(),
            });
        }
    }
    Ok(result)
}

/// Resolve a slash-separated name by reading only catalogs along its path.
pub async fn resolve_agent_path(
    ctx: &Context,
    root_session_id: &SessionId,
    name: &str,
    signal: AbortSignal,
) -> Result<Option<SessionId>, BoxError> {
    let segments = name.split('/').collect::<Vec<_>>();
    if !segments.iter().all(|segment| is_agent_name(segment)) {
        return Ok(None);
    }
    let mut current = root_session_id.clone();
    for (index, segment) in segments.iter().enumerate() {
        let children = list_children(ctx, &current, Some(signal.clone())).await?;
        let Some(child) = children
            .into_iter()
            .find(|entry| entry.name.as_deref() == Some(*segment))
        else {
            return Ok(None);
        };
        if index + 1 < segments.len() && child.mode != "continuable" && child.mode != "one-shot" {
            return Ok(None);
        }
        current = child.id;
    }
    Ok(Some(current))
}

fn query_failure_reason(error: &BoxError) -> SubagentDiagnosticReason {
    match HarnessError::of(error.as_ref())
        .filter(|error| error.is_named(SessionQueryError::NAME))
        .map(HarnessError::code)
    {
        Some(code)
            if code == SessionQueryErrorCode::CorruptSession.as_str()
                || code == SessionQueryErrorCode::SourceConflict.as_str() =>
        {
            SubagentDiagnosticReason::Corrupt
        }
        _ => SubagentDiagnosticReason::Unavailable,
    }
}

fn assert_not_cancelled(signal: Option<&AbortSignal>) -> Result<(), BoxError> {
    if signal.is_some_and(AbortSignal::aborted) {
        return Err(subagent_error(
            "subagent listing was cancelled",
            SubagentErrorCode::Cancelled,
        ));
    }
    Ok(())
}

fn subagent_error(message: impl Into<String>, code: SubagentErrorCode) -> BoxError {
    Box::new(SubagentError::error(message, code))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_paths_do_not_trigger_catalog_reads() {
        for value in ["", "lead", "Agent", "a//b", "a_b", "a/lead"] {
            assert!(!value.split('/').all(is_agent_name), "{value}");
        }
    }

    #[test]
    fn query_failures_are_classified_like_the_source() {
        let corrupt: BoxError = Box::new(SessionQueryError::new(
            "bad",
            SessionQueryErrorCode::CorruptSession,
        ));
        let conflict: BoxError = Box::new(SessionQueryError::new(
            "bad",
            SessionQueryErrorCode::SourceConflict,
        ));
        let missing: BoxError = Box::new(SessionQueryError::new(
            "bad",
            SessionQueryErrorCode::SessionNotFound,
        ));
        assert_eq!(
            query_failure_reason(&corrupt),
            SubagentDiagnosticReason::Corrupt
        );
        assert_eq!(
            query_failure_reason(&conflict),
            SubagentDiagnosticReason::Corrupt
        );
        assert_eq!(
            query_failure_reason(&missing),
            SubagentDiagnosticReason::Unavailable
        );
    }
}
