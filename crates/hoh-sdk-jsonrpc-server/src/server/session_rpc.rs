//! Window-facing methods that the SDK protocol lacks: `session/list`, `session/events`,
//! `session/cancel`, plus dispatch of `hoh/*` methods registered by sibling plugins.
use super::{HarnessSdkJsonRpcServer, lock};
use crate::{
    HohPrivilegedApprover, HohRpcMethods,
    error::{HohMethodNotFound, error, type_error},
};
use hoh_agent::{Agent, AgentStatus, CancelOptions};
use hoh_error::BoxError;
use hoh_protocol::{
    SessionEventsParams, SessionEventsResult, SessionListParams, SessionListResult, SessionSummary,
    SessionWaitParams,
};
use hoh_session::{AgentCancelCause, SessionEvent, SessionId};
use hoh_session_persistence::{SessionAccess, SessionPersistenceService};
use serde_json::{Value, json};
use std::sync::Arc;

const MAX_EVENTS_PER_PAGE: u32 = 1000;
const DEFAULT_EMPLOYEE_ID: &str = "default";
const TITLE_CHARS: usize = 80;

fn params_of<T: serde::de::DeserializeOwned>(method: &str, params: Value) -> Result<T, BoxError> {
    serde_json::from_value(params).map_err(|e| type_error(format!("{method}: {e}")))
}

fn is_subagent(header: &Value) -> bool {
    header["origin"] == "subagent"
}

/// Title for the session list: the logged title, else a header title, else the first
/// human message, so a session that never got a generated title is still recognisable.
fn title_of(header: &Value, events: &[Arc<SessionEvent>]) -> String {
    // Latest `session/title` event (owner: hoh-session-title; read here without its crate).
    if let Some(title) = events
        .iter()
        .rev()
        .find(|e| e.event_type() == "session/title")
        .and_then(|e| e.data()["title"].as_str())
    {
        return title.to_owned();
    }
    if let Some(title) = header["title"].as_str() {
        return title.to_owned();
    }
    events
        .iter()
        .filter(|e| e.event_type() == "user/message" && e.data()["source"]["kind"] == "user")
        .find_map(|e| {
            let text = e.data()["content"]
                .as_array()?
                .iter()
                .filter(|block| block["type"] == "text")
                .filter_map(|block| block["text"].as_str())
                .collect::<Vec<_>>()
                .join("\n");
            let line = text.lines().map(str::trim).find(|line| !line.is_empty())?;
            Some(line.chars().take(TITLE_CHARS).collect::<String>())
        })
        .unwrap_or_default()
}

fn summary_of(id: &SessionId, header: &Value, events: &[Arc<SessionEvent>]) -> SessionSummary {
    let updated_at = events
        .last()
        .map(|event| event.time())
        .or_else(|| header["createdAt"].as_i64())
        .unwrap_or_default();
    SessionSummary {
        id: id.to_string(),
        title: title_of(header, events),
        employee_id: header["employeeId"]
            .as_str()
            .unwrap_or(DEFAULT_EMPLOYEE_ID)
            .to_owned(),
        updated_at: u64::try_from(updated_at).unwrap_or_default(),
    }
}

impl HarnessSdkJsonRpcServer {
    fn live_agent(&self, id: &SessionId) -> Option<Arc<dyn Agent>> {
        let record = lock(&self.inner.state).sessions.get(id).cloned()?;
        Some(record.handle.agent.clone())
    }

    /// Full log of a session: the live one this server owns, else the stored one, read-only.
    async fn session_log(&self, id: &SessionId) -> Result<Arc<[Arc<SessionEvent>]>, BoxError> {
        if let Some(agent) = self.live_agent(id) {
            return Ok(agent.session().snapshot_events(None, None));
        }
        let unknown = || error(format!("unknown SDK session: {id}"));
        let persistence = self
            .inner
            .ctx
            .get::<SessionPersistenceService>()
            .ok_or_else(unknown)?;
        persistence.stat(id, None).await?.ok_or_else(unknown)?;
        let handle = persistence.open(id, SessionAccess::Read, None).await?;
        let read = handle.read(None, None, None).await;
        let closed = handle.close().await;
        let events = read?.events;
        closed?;
        Ok(events.into())
    }

    pub(super) async fn session_events(&self, params: Value) -> Result<Value, BoxError> {
        self.assert_initialized()?;
        let params: SessionEventsParams = params_of("session/events", params)?;
        let after = params.after_seq.unwrap_or(-1);
        if !(1..=MAX_EVENTS_PER_PAGE).contains(&params.limit) || after < -1 {
            return Err(type_error(
                "session/events requires limit 1..1000 and afterSeq >= -1",
            ));
        }
        let log = self
            .session_log(&SessionId::from(params.session_id))
            .await?;
        let events = log
            .iter()
            .filter(|event| i64::try_from(u64::from(event.seq())).is_ok_and(|seq| seq > after))
            .take(params.limit as usize)
            .map(|event| serde_json::from_value(serde_json::to_value(event)?))
            .collect::<Result<Vec<_>, serde_json::Error>>()?;
        let next_seq = events
            .last()
            .and_then(|event: &hoh_protocol::SessionEvent| i64::try_from(event.seq).ok())
            .unwrap_or(after);
        Ok(serde_json::to_value(SessionEventsResult {
            events,
            next_seq,
        })?)
    }

    pub(super) async fn session_list(&self, params: Value) -> Result<Value, BoxError> {
        self.assert_initialized()?;
        let params: SessionListParams = params_of("session/list", params)?;
        let mut sessions = Vec::new();
        let mut seen = Vec::new();
        if let Some(persistence) = self.inner.ctx.get::<SessionPersistenceService>() {
            for snapshot in persistence.list(None).await? {
                let header = serde_json::to_value(&snapshot.header)?;
                let id = snapshot.header.id.clone();
                if is_subagent(&header) {
                    continue;
                }
                // One unreadable stored log must not hide the other sessions.
                let events = self.session_log(&id).await.unwrap_or_else(|e| {
                    self.inner
                        .ctx
                        .logger()
                        .warn(format!("session/list: cannot read session {id}: {e}"));
                    Arc::from(Vec::new())
                });
                sessions.push(summary_of(&id, &header, &events));
                seen.push(id);
            }
        }
        let live = lock(&self.inner.state)
            .sessions
            .iter()
            .filter(|(id, _)| !seen.contains(id))
            .map(|(id, record)| (id.clone(), record.handle.agent.clone()))
            .collect::<Vec<_>>();
        for (id, agent) in live {
            let header = agent.session().header();
            if !is_subagent(&header) {
                let events = agent.session().snapshot_events(None, None);
                sessions.push(summary_of(&id, &header, &events));
            }
        }
        if let Some(employee_id) = &params.employee_id {
            sessions.retain(|session| &session.employee_id == employee_id);
        }
        sessions.sort_by(|a, b| {
            b.updated_at
                .cmp(&a.updated_at)
                .then_with(|| a.id.cmp(&b.id))
        });
        Ok(serde_json::to_value(SessionListResult { sessions })?)
    }

    pub(super) async fn session_delete(&self, params: Value) -> Result<Value, BoxError> {
        self.assert_initialized()?;
        let params: SessionWaitParams = params_of("session/delete", params)?;
        if params.session_id.is_empty() {
            return Err(type_error("session/delete requires a non-empty sessionId"));
        }
        let id = SessionId::from(params.session_id);
        let persistence = self
            .inner
            .ctx
            .get::<SessionPersistenceService>()
            .ok_or_else(|| error("Session storage is unavailable"))?;
        let record = {
            let mut state = lock(&self.inner.state);
            if state.deleted.contains(&id) {
                return Ok(json!({}));
            }
            if state.deleting.contains(&id)
                || state.prompt_admissions.contains_key(&id)
                || state
                    .activities
                    .values()
                    .any(|activity| activity.session_id() == &id)
                || state.creations.contains_key(&id)
                || state.scheduled.contains_key(&id)
            {
                return Err(error(
                    "Chat is busy. Wait for the current operation to finish.",
                ));
            }
            if let Some(record) = state.sessions.get(&id) {
                let agent = &record.handle.agent;
                let mut turn_open = false;
                let mut approvals = std::collections::HashSet::new();
                for event in agent.session().snapshot_events(None, None).iter() {
                    match event.event_type() {
                        "turn/start" => turn_open = true,
                        "turn/end" => {
                            turn_open = false;
                            approvals.clear();
                        }
                        "approval/asked" => {
                            if let Some(id) = event.data()["id"].as_str() {
                                approvals.insert(id.to_owned());
                            }
                        }
                        "approval/decided" => {
                            if let Some(id) = event.data()["id"].as_str() {
                                approvals.remove(id);
                            }
                        }
                        _ => {}
                    }
                }
                if agent.status() != AgentStatus::Idle
                    || turn_open
                    || !approvals.is_empty()
                    || !agent.inbox().next_turn().is_empty()
                    || !agent.inbox().next_step().is_empty()
                {
                    return Err(error(
                        "Chat is running or waiting for a response. Stop it before deleting.",
                    ));
                }
                agent.reserve_deletion()?;
            }
            state.deleting.insert(id.clone());
            state.sessions.shift_remove(&id)
        };
        // Drain buffered user events while the reserved agent is still alive. A failed flush
        // leaves the agent and its writer intact, so a later retry can persist the tail.
        if let Err(failure) = persistence.prepare_delete(&id).await {
            let mut state = lock(&self.inner.state);
            if let Some(record) = record {
                record.handle.agent.release_deletion();
                state.sessions.insert(id.clone(), record);
            }
            state.deleting.remove(&id);
            return Err(failure);
        }
        let result = async {
            if let Some(record) = record {
                record.handle.dispose().await?;
            }
            persistence.trash(&id).await?;
            Ok(json!({}))
        }
        .await;
        {
            let mut state = lock(&self.inner.state);
            state.deleting.remove(&id);
            if result.is_ok() {
                state.deleted.insert(id);
            }
        }
        result
    }

    pub(super) fn session_cancel(&self, params: Value) -> Result<Value, BoxError> {
        self.assert_initialized()?;
        // Same wire shape as session/wait: exactly `{sessionId}`.
        let params: SessionWaitParams = params_of("session/cancel", params)?;
        let id = SessionId::from(params.session_id);
        let agent = self
            .live_agent(&id)
            .ok_or_else(|| error(format!("unknown SDK session: {id}")))?;
        agent.cancel(AgentCancelCause::User, CancelOptions::default())?;
        Ok(json!({}))
    }

    /// Host mode asks the user before a privileged call runs. Outside host mode no mac can be
    /// checked, so the new `hoh/*` surface refuses such calls; the legacy SDK method keeps
    /// its old behaviour.
    pub(super) async fn approve_privileged(
        &self,
        method: &str,
        params: &Value,
    ) -> Result<(), BoxError> {
        if !hoh_protocol::requires_host_approval(method, params) {
            return Ok(());
        }
        if !lock(&self.inner.state).host_approvals {
            return if method.starts_with("hoh/") {
                Err(error(format!("{method} requires host approvals")))
            } else {
                Ok(())
            };
        }
        let approver = self
            .inner
            .ctx
            .get::<HohPrivilegedApprover>()
            .ok_or_else(|| {
                error(format!(
                    "{method} requires host approval, none is available"
                ))
            })?;
        approver.approve(method, params).await
    }

    /// Dispatch to the plugin that registered `method` in [`HohRpcMethods`].
    pub(super) async fn hoh_method(&self, method: &str, params: Value) -> Result<Value, BoxError> {
        self.assert_initialized()?;
        let registry = self.inner.ctx.get::<HohRpcMethods>();
        if !registry
            .as_ref()
            .is_some_and(|registry| registry.is_registered(method))
        {
            return Err(Box::new(HohMethodNotFound(method.to_owned())));
        }
        self.approve_privileged(method, &params).await?;
        match registry.and_then(|registry| registry.invoke(method, params)) {
            Some(call) => call.await,
            None => Err(Box::new(HohMethodNotFound(method.to_owned()))),
        }
    }
}
