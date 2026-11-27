//! Window-facing methods that the SDK protocol lacks: `session/list`, `session/events`,
//! `session/cancel`, plus dispatch of `hoh/*` methods registered by sibling plugins.
use super::{HarnessSdkJsonRpcServer, SessionRecord, lock};
use crate::{
    HohPrivilegedApprover, HohRpcMethods,
    error::{HohMethodNotFound, error, type_error},
};
use hoh_agent::{
    Agent, AgentOptions, AgentSessionMeta, AgentStatus, Agents, CancelOptions, CreateAgentOptions,
    EXECUTOR_BINDING_RECORD, Live,
};
use hoh_error::BoxError;
use hoh_protocol::{
    SessionEventsParams, SessionEventsResult, SessionForkForEditParams, SessionForkForEditResult,
    SessionForkSideChatParams, SessionForkSideChatResult, SessionListParams, SessionListResult,
    SessionSideChat, SessionSummary, SessionWaitParams,
};
use hoh_session::{
    AgentCancelCause, MAX_SAFE_INTEGER, SessionEvent, SessionId, SessionLogOffset, build_fork_seed,
};
use hoh_session_persistence::{SessionAccess, SessionPersistenceService};
use hoh_working_directory::{WORKING_DIRECTORY_CHANGE, WorkingDirectoryKey};
use serde_json::{Value, json};
use std::{collections::HashSet, sync::Arc};

const MAX_EVENTS_PER_PAGE: u32 = 1000;
const DEFAULT_EMPLOYEE_ID: &str = "default";
const TITLE_CHARS: usize = 80;
const SIDE_CHAT_RECORD: &str = "plugin:hoh-window/side-chat";

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

fn side_chat_seed_end(events: &[Arc<SessionEvent>]) -> Option<usize> {
    let cut = events.iter().rposition(|event| {
        event.event_type() == "session/end-seed" && event.data()["inherited"] == true
    })?;
    events[cut + 1..]
        .iter()
        .any(|event| event.event_type() == SIDE_CHAT_RECORD && event.ignorable())
        .then_some(cut)
}

fn summary_of(id: &SessionId, header: &Value, events: &[Arc<SessionEvent>]) -> SessionSummary {
    let updated_at = events
        .last()
        .map(|event| event.time())
        .or_else(|| header["createdAt"].as_i64())
        .unwrap_or_default();
    let side_parent = header["parentSession"].as_str();
    let (title, side_chat) =
        if let Some((parent, cut)) = side_parent.zip(side_chat_seed_end(events)) {
            let title = title_of(&Value::Null, &events[cut + 1..]);
            let title = if title.is_empty() {
                "Side chat".to_owned()
            } else {
                title
            };
            (
                title,
                Some(SessionSideChat {
                    parent_session_id: parent.to_owned(),
                    seed_end_seq: u64::from(events[cut].seq()),
                }),
            )
        } else {
            (title_of(header, events), None)
        };
    SessionSummary {
        id: id.to_string(),
        title,
        employee_id: header["employeeId"]
            .as_str()
            .unwrap_or(DEFAULT_EMPLOYEE_ID)
            .to_owned(),
        updated_at: u64::try_from(updated_at).unwrap_or_default(),
        side_chat,
    }
}

fn edit_boundary<'a>(
    events: &'a [Arc<SessionEvent>],
    message_id: &str,
) -> Result<(&'a SessionEvent, Vec<Value>), BoxError> {
    let user_message = |event: &Arc<SessionEvent>| {
        event.event_type() == "user/message" && event.data()["source"]["kind"] == "user"
    };
    let index = events
        .iter()
        .position(|event| user_message(event) && event.data()["id"] == message_id)
        .ok_or_else(|| error("This message is no longer available to edit."))?;
    let start = events[..index]
        .iter()
        .rposition(|event| matches!(event.event_type(), "turn/start" | "turn/end"))
        .filter(|start| {
            events[*start].event_type() == "turn/start"
                && !events[*start + 1..index].iter().any(user_message)
        })
        .ok_or_else(|| error("This message does not start a turn."))?;
    let prefix = start
        .checked_sub(1)
        .and_then(|index| events.get(index))
        .ok_or_else(|| error("The history before this message is unavailable."))?;
    let attachments = events[index].data()["content"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|block| block["type"] == "image")
        .cloned()
        .collect();
    Ok((prefix, attachments))
}

enum ForkBoundary {
    Edit(String),
    SideChat,
}

fn latest_stable_boundary(events: &[Arc<SessionEvent>]) -> Option<&SessionEvent> {
    let end = events
        .iter()
        .rposition(|event| event.event_type() == "turn/end");
    let before_first_turn = events
        .iter()
        .position(|event| event.event_type() == "turn/start")
        .and_then(|index| index.checked_sub(1));
    end.or(before_first_turn)
        .and_then(|index| events.get(index))
        .map(Arc::as_ref)
}

impl HarnessSdkJsonRpcServer {
    fn live_agent(&self, id: &SessionId) -> Option<Arc<dyn Agent>> {
        let record = lock(&self.inner.state).sessions.get(id).cloned()?;
        Some(record.handle.agent.clone())
    }

    pub(super) async fn session_fork_for_edit(&self, params: Value) -> Result<Value, BoxError> {
        self.assert_initialized()?;
        let params: SessionForkForEditParams = params_of("session/fork-for-edit", params)?;
        let (session_id, _, attachments) = self
            .fork_session(
                params.session_id,
                params.target_session_id,
                ForkBoundary::Edit(params.message_id),
            )
            .await?;
        Ok(serde_json::to_value(SessionForkForEditResult {
            session_id,
            attachments,
        })?)
    }

    pub(super) async fn session_fork_side_chat(&self, params: Value) -> Result<Value, BoxError> {
        self.assert_initialized()?;
        let params: SessionForkSideChatParams = params_of("session/fork-side-chat", params)?;
        let (session_id, seed_end_seq, _) = self
            .fork_session(
                params.session_id,
                params.target_session_id,
                ForkBoundary::SideChat,
            )
            .await?;
        Ok(serde_json::to_value(SessionForkSideChatResult {
            session_id,
            seed_end_seq,
        })?)
    }

    async fn fork_session(
        &self,
        source_id: String,
        target_id: String,
        boundary: ForkBoundary,
    ) -> Result<(String, u64, Vec<Value>), BoxError> {
        let source_id = SessionId::from(source_id);
        let id = SessionId::from(target_id);
        if id.as_ref().is_empty() || id == source_id {
            return Err(type_error("A fork requires a distinct target session id."));
        }
        let source = self
            .live_agent(&source_id)
            .ok_or_else(|| error("Open the source chat before branching."))?;
        let side_chat = matches!(boundary, ForkBoundary::SideChat);
        if !side_chat && source.status() != AgentStatus::Idle {
            return Err(error("Stop the current turn before editing a message."));
        }
        let session = source.session();
        let source_header = session.header();
        let events = session.snapshot_events(None, None);
        if side_chat && (is_subagent(&source_header) || side_chat_seed_end(&events).is_some()) {
            return Err(error(
                "Side chats must start from a top-level conversation.",
            ));
        }
        let selection = source
            .model()
            .current()
            .filter(|selection| {
                matches!(
                    selection.provider.as_str(),
                    "claude" | "codex" | "pi" | "kimi"
                )
            })
            .ok_or_else(|| error("Branching requires a Claude, Codex, Pi or Kimi conversation."))?;
        let (prefix, attachments) = match boundary {
            ForkBoundary::Edit(message_id) => edit_boundary(&events, &message_id)?,
            ForkBoundary::SideChat => (
                latest_stable_boundary(&events)
                    .ok_or_else(|| error("Start the first message before opening a side chat."))?,
                Vec::new(),
            ),
        };
        let inherited_event_count = SessionLogOffset::new(u64::from(prefix.seq()) + 1)?;
        let mut seed = build_fork_seed(&events, prefix.seq())?;
        // Even if creation is interrupted before the new binding is written, this log
        // must never resume a native session belonging to the original conversation.
        for event in &mut seed {
            if event.event_type() == EXECUTOR_BINDING_RECORD {
                let mut value = event.as_value().clone();
                value["data"]["nativeSessionId"] = Value::Null;
                if let Some(data) = value["data"].as_object_mut() {
                    data.remove("replaces");
                }
                *event = Arc::new(SessionEvent::from_value(value));
            }
        }
        if side_chat {
            // Keep the Window relationship with the persisted seed and its existing header.
            seed.push(Arc::new(SessionEvent::from_value(json!({
                "type": SIDE_CHAT_RECORD,
                "seq": SessionLogOffset::new(seed.len() as u64)?,
                "time": prefix.time(),
                "data": {},
                "ignorable": true,
            }))));
        }
        let directory = self
            .inner
            .ctx
            .get::<WorkingDirectoryKey>()
            .ok_or_else(|| error("workingDirectory service is unavailable"))?;
        let (cwd, [dev, ino]) = directory.verify_for_read(session)?;
        let agents = self
            .inner
            .ctx
            .get::<Agents>()
            .ok_or_else(|| error("agents service is unavailable"))?;
        let options = CreateAgentOptions {
            session_id: Some(id.clone()),
            meta: Some(AgentSessionMeta {
                cwd: Some(cwd.clone()),
                parent_session: Some(source_id.clone()),
                is_seeded: Some(true),
                ..Default::default()
            }),
            inherited_event_count: Some(inherited_event_count),
            seed: Some(seed),
            agent_options: Some(AgentOptions {
                provider: Some(Live::Value(selection.provider)),
                model: Some(Live::Value(selection.model)),
                ..Default::default()
            }),
            setup: Some(Arc::new(move |_ctx, agent| {
                let data = json!({"cwd": cwd, "identity": {"path": cwd, "dev": dev, "ino": ino}});
                Box::pin(async move {
                    agent
                        .session()
                        .append(WORKING_DIRECTORY_CHANGE, data, None)?;
                    Ok(None)
                })
            })),
            ..Default::default()
        };
        let creation = {
            let mut state = lock(&self.inner.state);
            if state.shutting_down || state.deleting.contains(&source_id) {
                return Err(error("The source chat is closing."));
            }
            if state.sessions.contains_key(&id)
                || state.creations.contains_key(&id)
                || state.deleting.contains(&id)
                || state.deleted.contains(&id)
            {
                return Err(error("The target chat already exists."));
            }
            let this = self.clone();
            let target_id = id.clone();
            self.reserve_session_creation(&mut state, id.clone(), async move {
                let handle = agents.create(&this.inner.ctx, options).await?;
                let record = Arc::new(SessionRecord {
                    handle,
                    failure: Default::default(),
                });
                lock(&this.inner.state)
                    .sessions
                    .insert(target_id, record.clone());
                Ok(record)
            })
        };
        creation.await?;
        Ok((id.to_string(), u64::from(prefix.seq()) + 1, attachments))
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
        let tail = params.tail.unwrap_or_default();
        if tail && params.after_seq.is_some() {
            return Err(type_error(
                "session/events tail cannot be combined with afterSeq",
            ));
        }
        let id = SessionId::from(params.session_id);
        let limit = params.limit as usize;
        let log = if let Some(agent) = self.live_agent(&id) {
            agent.session().snapshot_events(None, None)
        } else {
            let unknown = || error(format!("unknown SDK session: {id}"));
            let persistence = self
                .inner
                .ctx
                .get::<SessionPersistenceService>()
                .ok_or_else(unknown)?;
            let snapshot = persistence.stat(&id, None).await?.ok_or_else(unknown)?;
            let offset = if tail {
                snapshot
                    .event_count
                    .map(|count| count.saturating_sub(limit as u64))
            } else {
                Some((after.saturating_add(1) as u64).min(MAX_SAFE_INTEGER))
            };
            let handle = persistence.open(&id, SessionAccess::Read, None).await?;
            let read = handle
                .read(offset, (!tail).then_some(limit as u64), None)
                .await;
            let closed = handle.close().await;
            let events = read?.events;
            closed?;
            events.into()
        };
        let start = if tail {
            log.len().saturating_sub(limit)
        } else {
            log.partition_point(|event| {
                i64::try_from(u64::from(event.seq())).is_ok_and(|seq| seq <= after)
            })
        };
        let events = log[start..]
            .iter()
            .take(limit)
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
        let mut seen = HashSet::new();
        if let Some(persistence) = self.inner.ctx.get::<SessionPersistenceService>() {
            for snapshot in persistence.list(None).await? {
                let header = serde_json::to_value(&snapshot.header)?;
                let id = snapshot.header.id.clone();
                if is_subagent(&header) {
                    continue;
                }
                seen.insert(id.clone());
                if let Some(agent) = self.live_agent(&id) {
                    let events = agent.session().snapshot_events(None, None);
                    sessions.push(summary_of(&id, &header, &events));
                    continue;
                }
                let cached = lock(&self.inner.stored_summaries)
                    .get(&id)
                    .filter(|(revision, _)| *revision == snapshot.revision)
                    .map(|(_, summary)| summary.clone());
                if let Some(summary) = cached {
                    sessions.push(summary);
                    continue;
                }
                let read = async {
                    let handle = persistence.open(&id, SessionAccess::Read, None).await?;
                    let read = handle.read(None, None, None).await;
                    let closed = handle.close().await;
                    closed?;
                    Ok::<_, BoxError>(read?.events)
                }
                .await;
                let summary = match read {
                    Ok(events) => {
                        let summary = summary_of(&id, &header, &events);
                        lock(&self.inner.stored_summaries)
                            .insert(id, (snapshot.revision, summary.clone()));
                        summary
                    }
                    Err(e) => {
                        // A damaged log must not hide other sessions or poison a cached title.
                        self.inner
                            .ctx
                            .logger()
                            .warn(format!("session/list: cannot read session {id}: {e}"));
                        summary_of(&id, &header, &[])
                    }
                };
                sessions.push(summary);
            }
        }
        lock(&self.inner.stored_summaries).retain(|id, _| seen.contains(id));
        let live = lock(&self.inner.state)
            .sessions
            .iter()
            .filter(|(id, _)| !seen.contains(*id))
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
                        "CHAT_DELETE_BUSY: Chat is running or waiting for a response. Stop it before deleting.",
                    ));
                }
                agent.reserve_deletion().map_err(|failure| {
                    if failure.to_string() == "Chat is running or waiting for a response. Stop it before deleting." {
                        error("CHAT_DELETE_BUSY: Chat is running or waiting for a response. Stop it before deleting.")
                    } else { failure }
                })?;
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
            return if method.starts_with("hoh/") || method == "session/working-directory/rebind" {
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
