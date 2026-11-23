use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use serde_json::Value;

use crate::{
    ActivityStatus, BackgroundJobStats, BackgroundKind, McpServerStats, McpServerStatus,
    PlanSnapshot, PlanSource, ProviderKind, SessionStatsSnapshot, SkillInvocation, StatsEvent,
    SubagentKind, SubagentStats, TokenUsage,
};

const CODEX_INVISIBLE_NOTE: &str = "внутренние шаги не видны";

#[derive(Clone, Debug, PartialEq)]
struct ToolCallState {
    name: String,
    input: Value,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SessionStats {
    snapshot: SessionStatsSnapshot,
    seen_turns: HashSet<String>,
    seen_results: HashSet<String>,
    seen_tool_calls: HashSet<String>,
    tools: HashMap<String, ToolCallState>,
    background: BTreeMap<String, BackgroundJobStats>,
    subagents: BTreeMap<String, SubagentStats>,
    task_to_tool: HashMap<String, String>,
    claude_ledgers: HashMap<String, TokenUsage>,
    claude_ledger_total: Option<TokenUsage>,
    claude_cost_ledgers: HashMap<String, f64>,
    claude_cost_total: Option<f64>,
    codex_baseline: Option<TokenUsage>,
    codex_active_turn: Option<String>,
    codex_turn_usage: HashMap<String, TokenUsage>,
}

impl SessionStats {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn snapshot(&self) -> &SessionStatsSnapshot {
        &self.snapshot
    }

    #[must_use]
    pub fn into_snapshot(self) -> SessionStatsSnapshot {
        self.snapshot
    }

    /// Applies one normalized event. Returns true only when projection state changed.
    pub fn apply(&mut self, event: StatsEvent) -> bool {
        let before = self.clone();
        self.apply_inner(event);
        self.refresh_collections();
        if *self == before {
            return false;
        }
        self.snapshot.revision = before.snapshot.revision.saturating_add(1);
        true
    }

    /// Folds one Claude stream-json object and returns the number of state changes it caused.
    pub fn apply_claude_stream_value(&mut self, value: &Value) -> usize {
        let mut events = Vec::new();
        match value.get("type").and_then(Value::as_str) {
            Some("system") => self.claude_system_events(value, &mut events),
            Some("assistant") => self.claude_assistant_events(value, &mut events),
            Some("user") => self.claude_user_events(value, &mut events),
            Some("result") => self.claude_result_event(value, &mut events),
            Some("control_response") => self.claude_control_events(value, &mut events),
            _ => {}
        }
        events
            .into_iter()
            .map(|event| usize::from(self.apply(event)))
            .sum()
    }

    /// Folds one Codex app-server notification. Unknown methods are safe no-ops.
    pub fn apply_codex_notification(&mut self, value: &Value) -> usize {
        let method = value.get("method").and_then(Value::as_str);
        let params = value.get("params").unwrap_or(value);
        let event = match method {
            Some("turn/started") => string_at(params, &["turn", "id"])
                .or_else(|| string_field(params, "turnId"))
                .map(|id| StatsEvent::CodexTurnStarted { id }),
            Some("thread/tokenUsage/updated") => (|| {
                let usage = params.get("tokenUsage")?;
                let turn_id = string_field(params, "turnId")?;
                let total = parse_codex_usage(usage.get("total")?)?;
                let last = parse_codex_usage(usage.get("last")?)?;
                Some(StatsEvent::CodexTokenUsageUpdated {
                    turn_id,
                    total,
                    last,
                })
            })(),
            Some("turn/completed") => string_at(params, &["turn", "id"])
                .or_else(|| string_field(params, "turnId"))
                .map(|id| StatsEvent::CodexTurnFinished { id }),
            _ => None,
        };
        event.map_or(0, |event| usize::from(self.apply(event)))
    }

    fn apply_inner(&mut self, event: StatsEvent) {
        match event {
            StatsEvent::UserPrompt { text } => {
                let text = text.trim();
                if self.snapshot.goal.is_none() && !text.is_empty() {
                    self.snapshot.goal = Some(text.to_owned());
                }
            }
            StatsEvent::TurnStarted { id, at_ms } => {
                self.record_turn(&id);
                if let Some(at_ms) = at_ms {
                    self.snapshot.started_at_ms.get_or_insert(at_ms);
                    self.snapshot.updated_at_ms = Some(at_ms);
                }
            }
            StatsEvent::TurnFinished { id, at_ms } => {
                self.record_turn(&id);
                if let Some(at_ms) = at_ms {
                    self.snapshot.updated_at_ms = Some(at_ms);
                    if let Some(started) = self.snapshot.started_at_ms {
                        self.snapshot.runtime_ms =
                            u64::try_from(at_ms.saturating_sub(started)).ok();
                    }
                }
            }
            StatsEvent::RuntimeMetadata {
                provider,
                model,
                effort,
                mode,
            } => {
                if let Some(provider) = provider {
                    self.snapshot.provider = Some(provider);
                    if provider == ProviderKind::Codex {
                        self.snapshot.cost_usd = None;
                    }
                }
                if model.is_some() {
                    self.snapshot.model = model;
                }
                if effort.is_some() {
                    self.snapshot.effort = effort;
                }
                if mode.is_some() {
                    self.snapshot.mode = mode;
                }
            }
            StatsEvent::ContextUpdated(context) => self.snapshot.context = Some(context),
            StatsEvent::AvailableSkills(names) => {
                self.snapshot.skills.available = Some(deduplicate(names));
            }
            StatsEvent::McpServers(servers) => self.snapshot.mcp_servers = Some(servers),
            StatsEvent::ToolCall {
                id,
                name,
                input,
                parent_tool_use_id,
            } => self.record_tool_call(id, name, input, parent_tool_use_id),
            StatsEvent::ToolResult { id, is_error } => self.record_tool_result(&id, is_error),
            StatsEvent::SystemTask {
                id,
                tool_use_id,
                label,
                status,
            } => self.record_system_task(id, tool_use_id, label, status),
            StatsEvent::BackgroundTasksSnapshot { active_task_ids } => {
                self.record_background_tasks_snapshot(&active_task_ids);
            }
            StatsEvent::ClaudeResult {
                id,
                ledger_id,
                usage,
                model_usage,
                total_cost_usd,
                duration_ms,
            } => {
                if !self.seen_results.insert(id.clone()) {
                    return;
                }
                self.snapshot.provider = Some(ProviderKind::Claude);
                self.record_turn(&id);
                if usage.is_known() {
                    self.snapshot.tokens.turn = Some(usage.clone());
                    self.snapshot.tokens.conversation =
                        Some(match self.snapshot.tokens.conversation.as_ref() {
                            Some(total) => strict_usage_sum(total, &usage),
                            None => usage,
                        });
                }
                let ledger_id = ledger_id.unwrap_or_else(|| "default".to_owned());
                // `modelUsage` is cumulative for one CLI process and includes subagents and
                // compaction. Accumulate only its growth. A lower counter means a provider/process
                // reset, so the current ledger starts a new segment instead of replacing history.
                if let Some(model_usage) = model_usage.filter(TokenUsage::is_known) {
                    let delta = self
                        .claude_ledgers
                        .get(&ledger_id)
                        .and_then(|previous| claude_ledger_delta(previous, &model_usage))
                        .unwrap_or_else(|| model_usage.clone());
                    self.claude_ledger_total = Some(match self.claude_ledger_total.as_ref() {
                        Some(total) => strict_usage_sum(total, &delta),
                        None => delta,
                    });
                    self.claude_ledgers.insert(ledger_id.clone(), model_usage);
                    self.snapshot.tokens.provider_total = self.claude_ledger_total.clone();
                }
                if let Some(total_cost_usd) = total_cost_usd {
                    let delta = self
                        .claude_cost_ledgers
                        .get(&ledger_id)
                        .and_then(|previous| {
                            (total_cost_usd >= *previous).then_some(total_cost_usd - *previous)
                        })
                        .unwrap_or(total_cost_usd);
                    let total = self.claude_cost_total.unwrap_or(0.0) + delta;
                    if total.is_finite() {
                        self.claude_cost_total = Some(total);
                        self.snapshot.cost_usd = Some(total);
                    }
                    self.claude_cost_ledgers.insert(ledger_id, total_cost_usd);
                }
                if let Some(duration_ms) = duration_ms {
                    self.snapshot.runtime_ms = Some(
                        self.snapshot
                            .runtime_ms
                            .unwrap_or(0)
                            .saturating_add(duration_ms),
                    );
                }
            }
            StatsEvent::CodexTurnStarted { id } => {
                self.snapshot.provider = Some(ProviderKind::Codex);
                self.snapshot.cost_usd = None;
                self.record_turn(&id);
                self.codex_active_turn = Some(id.clone());
                self.codex_turn_usage.entry(id).or_default();
            }
            StatsEvent::CodexTokenUsageUpdated {
                turn_id,
                total,
                last,
            } => {
                self.snapshot.provider = Some(ProviderKind::Codex);
                self.snapshot.cost_usd = None;
                self.snapshot.tokens.conversation = Some(total.clone());
                self.snapshot.tokens.provider_total = Some(total.clone());
                if self.codex_active_turn.as_deref() == Some(turn_id.as_str()) {
                    let delta = self
                        .codex_baseline
                        .as_ref()
                        .and_then(|baseline| usage_delta(baseline, &total))
                        .unwrap_or(last);
                    let accumulated = self.codex_turn_usage.entry(turn_id).or_default();
                    *accumulated = if accumulated.is_known() {
                        strict_usage_sum(accumulated, &delta)
                    } else {
                        delta
                    };
                }
                // As in T3, even a late update for a non-active turn advances the baseline.
                self.codex_baseline = Some(total);
            }
            StatsEvent::CodexTurnFinished { id } => {
                self.snapshot.provider = Some(ProviderKind::Codex);
                self.snapshot.cost_usd = None;
                self.record_turn(&id);
                if let Some(usage) = self.codex_turn_usage.remove(&id)
                    && usage.is_known()
                {
                    self.snapshot.tokens.turn = Some(usage);
                }
                if self.codex_active_turn.as_deref() == Some(id.as_str()) {
                    self.codex_active_turn = None;
                }
            }
        }
    }

    fn record_turn(&mut self, id: &str) {
        if self.seen_turns.insert(id.to_owned()) {
            self.snapshot.turn_count = Some(self.snapshot.turn_count.unwrap_or(0) + 1);
        }
    }

    fn record_tool_call(
        &mut self,
        id: String,
        name: String,
        input: Value,
        parent_tool_use_id: Option<String>,
    ) {
        if !self.seen_tool_calls.insert(id.clone()) {
            return;
        }
        self.snapshot.tool_call_count = Some(self.snapshot.tool_call_count.unwrap_or(0) + 1);
        self.tools.insert(
            id.clone(),
            ToolCallState {
                name: name.clone(),
                input: input.clone(),
            },
        );

        match name.as_str() {
            "TodoWrite" if parent_tool_use_id.is_none() => {
                if let Some(text) = todo_plan(&input) {
                    self.snapshot.plan = Some(PlanSnapshot {
                        source: PlanSource::TodoWrite,
                        text,
                    });
                }
            }
            "ExitPlanMode" if parent_tool_use_id.is_none() => {
                if let Some(text) = input
                    .get("plan")
                    .or_else(|| input.get("text"))
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|text| !text.is_empty())
                {
                    self.snapshot.plan = Some(PlanSnapshot {
                        source: PlanSource::ExitPlanMode,
                        text: text.to_owned(),
                    });
                }
            }
            "Skill" => {
                if let Some(skill) = input
                    .get("skill")
                    .or_else(|| input.get("name"))
                    .and_then(Value::as_str)
                {
                    let invoked = self.snapshot.skills.invoked.get_or_insert_with(Vec::new);
                    invoked.push(SkillInvocation {
                        name: skill.to_owned(),
                        call_id: id.clone(),
                    });
                }
            }
            "Agent" | "Task" => {
                let subagent_type = input
                    .get("subagent_type")
                    .or_else(|| input.get("name"))
                    .and_then(Value::as_str)
                    .unwrap_or("Claude");
                let description = input
                    .get("description")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                self.subagents.insert(
                    id.clone(),
                    SubagentStats {
                        call_id: id.clone(),
                        kind: SubagentKind::Claude,
                        name: subagent_type.to_owned(),
                        description,
                        status: ActivityStatus::Running,
                        note: None,
                    },
                );
                if input
                    .get("run_in_background")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                {
                    self.background.insert(
                        id.clone(),
                        BackgroundJobStats {
                            id: id.clone(),
                            kind: BackgroundKind::Agent,
                            label: subagent_type.to_owned(),
                            status: ActivityStatus::Running,
                        },
                    );
                }
            }
            _ => {}
        }

        if (name == "Bash" && explicit_codex_exec(&input)) || is_codex_mcp_tool(&name) {
            let description = if name == "Bash" {
                input
                    .get("command")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            } else {
                Some(name.clone())
            };
            self.subagents.insert(
                id.clone(),
                SubagentStats {
                    call_id: id.clone(),
                    kind: SubagentKind::Codex,
                    name: "Codex".to_owned(),
                    description,
                    status: ActivityStatus::Running,
                    note: Some(CODEX_INVISIBLE_NOTE.to_owned()),
                },
            );
        }

        let background_kind = match name.as_str() {
            "ScheduleWakeup" => Some(BackgroundKind::ScheduleWakeup),
            "CronCreate" => Some(BackgroundKind::Cron),
            "Monitor" => Some(BackgroundKind::Monitor),
            "Bash"
                if input
                    .get("run_in_background")
                    .and_then(Value::as_bool)
                    .unwrap_or(false) =>
            {
                Some(BackgroundKind::Bash)
            }
            _ => None,
        };
        if let Some(kind) = background_kind {
            let label = input
                .get("description")
                .or_else(|| input.get("command"))
                .or_else(|| input.get("prompt"))
                .and_then(Value::as_str)
                .unwrap_or(name.as_str())
                .to_owned();
            self.background.insert(
                id.clone(),
                BackgroundJobStats {
                    id,
                    kind,
                    label,
                    status: ActivityStatus::Running,
                },
            );
        }
    }

    fn record_tool_result(&mut self, id: &str, is_error: Option<bool>) {
        let failed = is_error == Some(true);
        let has_system_task = self.task_to_tool.values().any(|tool| tool == id);
        if let Some(subagent) = self.subagents.get_mut(id) {
            subagent.status = if failed {
                ActivityStatus::Failed
            } else if has_system_task {
                ActivityStatus::Running
            } else {
                ActivityStatus::Finished
            };
        }
        if let Some(job) = self.background.get_mut(id) {
            job.status = if failed {
                ActivityStatus::Failed
            } else if job.kind == BackgroundKind::Monitor {
                ActivityStatus::Finished
            } else {
                // A launcher result is not proof that the launched work finished.
                ActivityStatus::Unknown
            };
        }
    }

    fn record_system_task(
        &mut self,
        id: String,
        tool_use_id: Option<String>,
        label: Option<String>,
        status: ActivityStatus,
    ) {
        let previous_tool_id = self.task_to_tool.get(&id).cloned();
        let tool_id = tool_use_id
            .or_else(|| self.task_to_tool.get(&id).cloned())
            .unwrap_or_else(|| id.clone());
        if previous_tool_id.as_deref() == Some(id.as_str()) && tool_id != id {
            // `background_tasks_changed` can announce a task before `task_started` provides the
            // launcher call id. Drop that provisional row when the stable tool reference arrives.
            self.background.remove(&id);
        }
        self.task_to_tool.insert(id.clone(), tool_id.clone());
        if let Some(subagent) = self.subagents.get_mut(&tool_id) {
            subagent.status = status;
        }
        if let Some(job) = self.background.get_mut(&tool_id) {
            job.status = status;
            if let Some(label) = label {
                job.label = label;
            }
            return;
        }
        let kind = self
            .tools
            .get(&tool_id)
            .map(|tool| match tool.name.as_str() {
                "Bash" => BackgroundKind::Bash,
                "Agent" | "Task" => BackgroundKind::Agent,
                "ScheduleWakeup" => BackgroundKind::ScheduleWakeup,
                "CronCreate" => BackgroundKind::Cron,
                "Monitor" => BackgroundKind::Monitor,
                _ => BackgroundKind::Other,
            })
            .unwrap_or(BackgroundKind::Other);
        self.background.insert(
            tool_id.clone(),
            BackgroundJobStats {
                id: tool_id,
                kind,
                label: label.unwrap_or(id),
                status,
            },
        );
    }

    fn record_background_tasks_snapshot(&mut self, active_task_ids: &[String]) {
        let active: HashSet<&str> = active_task_ids.iter().map(String::as_str).collect();
        for (task_id, tool_id) in &self.task_to_tool {
            if active.contains(task_id.as_str()) {
                continue;
            }
            if let Some(job) = self.background.get_mut(tool_id)
                && job.status == ActivityStatus::Running
            {
                job.status = ActivityStatus::Unknown;
            }
            if let Some(subagent) = self.subagents.get_mut(tool_id)
                && subagent.status == ActivityStatus::Running
            {
                subagent.status = ActivityStatus::Unknown;
            }
        }
    }

    fn refresh_collections(&mut self) {
        if !self.background.is_empty() {
            self.snapshot.background_jobs = Some(self.background.values().cloned().collect());
        }
        if !self.subagents.is_empty() {
            self.snapshot.subagents = Some(self.subagents.values().cloned().collect());
        }
    }

    fn claude_system_events(&self, value: &Value, events: &mut Vec<StatsEvent>) {
        match value.get("subtype").and_then(Value::as_str) {
            Some("init") => {
                events.push(StatsEvent::RuntimeMetadata {
                    provider: Some(ProviderKind::Claude),
                    model: string_field(value, "model"),
                    effort: string_field(value, "effort"),
                    mode: string_field(value, "permissionMode"),
                });
                let mut skills = strings_field(value, "skills");
                skills.extend(strings_field(value, "slash_commands"));
                if value.get("skills").is_some() || value.get("slash_commands").is_some() {
                    events.push(StatsEvent::AvailableSkills(deduplicate(skills)));
                }
                if let Some(servers) = value.get("mcp_servers").and_then(Value::as_array) {
                    events.push(StatsEvent::McpServers(
                        servers.iter().filter_map(parse_mcp_server).collect(),
                    ));
                }
            }
            Some("task_started") => {
                if let Some(id) = string_field(value, "task_id") {
                    events.push(StatsEvent::SystemTask {
                        id,
                        tool_use_id: string_field(value, "tool_use_id"),
                        label: string_field(value, "description"),
                        status: ActivityStatus::Running,
                    });
                }
            }
            Some("task_updated") => {
                if let Some(id) = string_field(value, "task_id") {
                    let status = value
                        .get("patch")
                        .and_then(|patch| patch.get("status"))
                        .and_then(Value::as_str)
                        .map(activity_status)
                        .unwrap_or(ActivityStatus::Unknown);
                    events.push(StatsEvent::SystemTask {
                        id,
                        tool_use_id: None,
                        label: None,
                        status,
                    });
                }
            }
            Some("task_notification") => {
                if let Some(id) = string_field(value, "task_id") {
                    events.push(StatsEvent::SystemTask {
                        id,
                        tool_use_id: string_field(value, "tool_use_id"),
                        label: string_field(value, "summary"),
                        status: value
                            .get("status")
                            .and_then(Value::as_str)
                            .map(activity_status)
                            .unwrap_or(ActivityStatus::Unknown),
                    });
                }
            }
            Some("background_tasks_changed") => {
                if let Some(tasks) = value.get("tasks").and_then(Value::as_array) {
                    events.push(StatsEvent::BackgroundTasksSnapshot {
                        active_task_ids: tasks
                            .iter()
                            .filter_map(|task| string_field(task, "task_id"))
                            .collect(),
                    });
                    for task in tasks {
                        if let Some(id) = string_field(task, "task_id") {
                            events.push(StatsEvent::SystemTask {
                                id,
                                tool_use_id: string_field(task, "tool_use_id"),
                                label: string_field(task, "description"),
                                status: task
                                    .get("status")
                                    .and_then(Value::as_str)
                                    .map(activity_status)
                                    .unwrap_or(ActivityStatus::Running),
                            });
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn claude_assistant_events(&self, value: &Value, events: &mut Vec<StatsEvent>) {
        if let Some(model) = string_at(value, &["message", "model"]) {
            events.push(StatsEvent::RuntimeMetadata {
                provider: Some(ProviderKind::Claude),
                model: Some(model),
                effort: None,
                mode: None,
            });
        }
        let parent = string_field(value, "parent_tool_use_id");
        let Some(content) = value
            .get("message")
            .and_then(|message| message.get("content"))
            .and_then(Value::as_array)
        else {
            return;
        };
        for block in content {
            if block.get("type").and_then(Value::as_str) != Some("tool_use") {
                continue;
            }
            let (Some(id), Some(name)) = (string_field(block, "id"), string_field(block, "name"))
            else {
                continue;
            };
            events.push(StatsEvent::ToolCall {
                id,
                name,
                input: block.get("input").cloned().unwrap_or(Value::Null),
                parent_tool_use_id: parent.clone(),
            });
        }
    }

    fn claude_user_events(&self, value: &Value, events: &mut Vec<StatsEvent>) {
        let is_root = value.get("parent_tool_use_id").is_none_or(Value::is_null);
        let Some(content) = value
            .get("message")
            .and_then(|message| message.get("content"))
        else {
            return;
        };
        if let Some(text) = content.as_str() {
            if is_root && !text.starts_with("<system-reminder>") {
                events.push(StatsEvent::UserPrompt {
                    text: text.to_owned(),
                });
            }
            return;
        }
        let Some(blocks) = content.as_array() else {
            return;
        };
        let has_tool_result = blocks
            .iter()
            .any(|block| block.get("type").and_then(Value::as_str) == Some("tool_result"));
        for block in blocks {
            match block.get("type").and_then(Value::as_str) {
                Some("tool_result") => {
                    if let Some(id) = string_field(block, "tool_use_id") {
                        events.push(StatsEvent::ToolResult {
                            id,
                            is_error: block.get("is_error").and_then(Value::as_bool),
                        });
                    }
                }
                Some("text") if is_root && !has_tool_result => {
                    if let Some(text) = block.get("text").and_then(Value::as_str)
                        && !text.starts_with("<system-reminder>")
                    {
                        events.push(StatsEvent::UserPrompt {
                            text: text.to_owned(),
                        });
                    }
                }
                _ => {}
            }
        }
    }

    fn claude_result_event(&self, value: &Value, events: &mut Vec<StatsEvent>) {
        let usage = value
            .get("usage")
            .map(parse_claude_usage)
            .unwrap_or_default();
        let model_usage = value.get("modelUsage").and_then(parse_claude_model_usage);
        let id = string_field(value, "uuid").unwrap_or_else(|| {
            format!(
                "{}:{}:{}",
                value
                    .get("session_id")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown"),
                value
                    .get("result_index")
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
                value
                    .get("duration_ms")
                    .and_then(Value::as_u64)
                    .unwrap_or(0)
            )
        });
        events.push(StatsEvent::ClaudeResult {
            id,
            ledger_id: string_field(value, "session_id"),
            usage,
            model_usage,
            total_cost_usd: finite_nonnegative_f64(value.get("total_cost_usd")),
            duration_ms: value.get("duration_ms").and_then(Value::as_u64),
        });
    }

    fn claude_control_events(&self, value: &Value, events: &mut Vec<StatsEvent>) {
        let response = value.get("response").unwrap_or(value);
        let mut names = strings_field(response, "skills");
        names.extend(strings_field(response, "commands"));
        if !names.is_empty() {
            events.push(StatsEvent::AvailableSkills(deduplicate(names)));
        }
    }
}

impl TokenUsage {
    fn is_known(&self) -> bool {
        self.input_tokens.is_some()
            || self.output_tokens.is_some()
            || self.cache_read_tokens.is_some()
            || self.cache_write_tokens.is_some()
            || self.reasoning_tokens.is_some()
    }
}

fn strict_usage_sum(left: &TokenUsage, right: &TokenUsage) -> TokenUsage {
    fn sum(left: Option<u64>, right: Option<u64>) -> Option<u64> {
        left.zip(right)
            .and_then(|(left, right)| left.checked_add(right))
    }
    TokenUsage {
        input_tokens: sum(left.input_tokens, right.input_tokens),
        output_tokens: sum(left.output_tokens, right.output_tokens),
        cache_read_tokens: sum(left.cache_read_tokens, right.cache_read_tokens),
        cache_write_tokens: sum(left.cache_write_tokens, right.cache_write_tokens),
        reasoning_tokens: sum(left.reasoning_tokens, right.reasoning_tokens),
    }
}

fn usage_delta(previous: &TokenUsage, current: &TokenUsage) -> Option<TokenUsage> {
    fn required_delta(previous: Option<u64>, current: Option<u64>) -> Option<u64> {
        current?.checked_sub(previous?)
    }
    Some(TokenUsage {
        input_tokens: Some(required_delta(previous.input_tokens, current.input_tokens)?),
        output_tokens: Some(required_delta(
            previous.output_tokens,
            current.output_tokens,
        )?),
        cache_read_tokens: Some(required_delta(
            previous.cache_read_tokens,
            current.cache_read_tokens,
        )?),
        // Codex added cache-write accounting after the other cumulative fields. As in T3,
        // absence on either side makes only this field unknown; it must not force every other
        // counter to fall back to `last` and be counted twice.
        cache_write_tokens: previous
            .cache_write_tokens
            .zip(current.cache_write_tokens)
            .and_then(|(previous, current)| current.checked_sub(previous)),
        reasoning_tokens: Some(required_delta(
            previous.reasoning_tokens,
            current.reasoning_tokens,
        )?),
    })
}

fn claude_ledger_delta(previous: &TokenUsage, current: &TokenUsage) -> Option<TokenUsage> {
    let pairs = [
        (previous.input_tokens, current.input_tokens),
        (previous.output_tokens, current.output_tokens),
        (previous.cache_read_tokens, current.cache_read_tokens),
        (previous.cache_write_tokens, current.cache_write_tokens),
        (previous.reasoning_tokens, current.reasoning_tokens),
    ];
    if pairs
        .iter()
        .any(|(previous, current)| previous.zip(*current).is_some_and(|(a, b)| b < a))
    {
        return None;
    }
    let delta = |previous: Option<u64>, current: Option<u64>| {
        previous
            .zip(current)
            .and_then(|(previous, current)| current.checked_sub(previous))
    };
    Some(TokenUsage {
        input_tokens: delta(previous.input_tokens, current.input_tokens),
        output_tokens: delta(previous.output_tokens, current.output_tokens),
        cache_read_tokens: delta(previous.cache_read_tokens, current.cache_read_tokens),
        cache_write_tokens: delta(previous.cache_write_tokens, current.cache_write_tokens),
        reasoning_tokens: delta(previous.reasoning_tokens, current.reasoning_tokens),
    })
}

fn parse_claude_usage(value: &Value) -> TokenUsage {
    TokenUsage {
        input_tokens: u64_field(value, "input_tokens"),
        output_tokens: u64_field(value, "output_tokens"),
        cache_read_tokens: u64_field(value, "cache_read_input_tokens"),
        cache_write_tokens: u64_field(value, "cache_creation_input_tokens"),
        reasoning_tokens: value
            .get("output_tokens_details")
            .and_then(|details| u64_field(details, "thinking_tokens")),
    }
}

fn parse_claude_model_usage(value: &Value) -> Option<TokenUsage> {
    let entries = value.as_object()?;
    if entries.is_empty() {
        return None;
    }
    let mut total: Option<TokenUsage> = None;
    for entry in entries.values() {
        let usage = TokenUsage {
            input_tokens: u64_field(entry, "inputTokens"),
            output_tokens: u64_field(entry, "outputTokens"),
            cache_read_tokens: u64_field(entry, "cacheReadInputTokens"),
            cache_write_tokens: u64_field(entry, "cacheCreationInputTokens"),
            reasoning_tokens: u64_field(entry, "thinkingTokens"),
        };
        total = Some(match total {
            Some(ref aggregate) => strict_usage_sum(aggregate, &usage),
            None => usage,
        });
    }
    total.filter(TokenUsage::is_known)
}

fn parse_codex_usage(value: &Value) -> Option<TokenUsage> {
    let usage = TokenUsage {
        input_tokens: u64_field(value, "inputTokens"),
        output_tokens: u64_field(value, "outputTokens"),
        cache_read_tokens: u64_field(value, "cachedInputTokens"),
        cache_write_tokens: u64_field(value, "cacheWriteInputTokens"),
        reasoning_tokens: u64_field(value, "reasoningOutputTokens"),
    };
    usage.is_known().then_some(usage)
}

fn parse_mcp_server(value: &Value) -> Option<McpServerStats> {
    Some(McpServerStats {
        name: string_field(value, "name")?,
        status: value
            .get("status")
            .and_then(Value::as_str)
            .map(mcp_status)
            .unwrap_or(McpServerStatus::Unknown),
    })
}

fn mcp_status(status: &str) -> McpServerStatus {
    match status {
        "connected" => McpServerStatus::Connected,
        "pending" | "starting" => McpServerStatus::Pending,
        "needs-auth" | "needs_auth" => McpServerStatus::NeedsAuth,
        "failed" | "error" | "disconnected" => McpServerStatus::Failed,
        _ => McpServerStatus::Unknown,
    }
}

fn activity_status(status: &str) -> ActivityStatus {
    match status {
        "running" | "started" | "pending" => ActivityStatus::Running,
        "completed" | "finished" | "success" | "succeeded" => ActivityStatus::Finished,
        "failed" | "error" | "cancelled" | "canceled" => ActivityStatus::Failed,
        _ => ActivityStatus::Unknown,
    }
}

fn todo_plan(input: &Value) -> Option<String> {
    let todos = input.get("todos")?.as_array()?;
    let lines: Vec<String> = todos
        .iter()
        .filter_map(|todo| {
            let content = todo.get("content")?.as_str()?.trim();
            if content.is_empty() {
                return None;
            }
            let status = todo
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            Some(format!("• [{status}] {content}"))
        })
        .collect();
    (!lines.is_empty()).then(|| lines.join("\n"))
}

fn deduplicate(values: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    values
        .into_iter()
        .filter(|value| !value.is_empty() && seen.insert(value.clone()))
        .collect()
}

fn strings_field(value: &Value, field: &str) -> Vec<String> {
    value
        .get(field)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .or_else(|| string_field(value, "name"))
        })
        .collect()
}

fn string_field(value: &Value, field: &str) -> Option<String> {
    value.get(field)?.as_str().map(str::to_owned)
}

fn string_at(value: &Value, path: &[&str]) -> Option<String> {
    let mut current = value;
    for field in path {
        current = current.get(field)?;
    }
    current.as_str().map(str::to_owned)
}

fn u64_field(value: &Value, field: &str) -> Option<u64> {
    value.get(field)?.as_u64()
}

fn finite_nonnegative_f64(value: Option<&Value>) -> Option<f64> {
    let value = value?.as_f64()?;
    (value.is_finite() && value >= 0.0).then_some(value)
}

/// Recognizes only an explicit argv whose executable basename is `codex` and subcommand is
/// `exec`. Shell operators, substitutions, variables, and wrapper commands are rejected.
#[must_use]
pub fn explicit_codex_exec(input: &Value) -> bool {
    let Some(command) = input.get("command") else {
        return false;
    };
    let argv = if let Some(values) = command.as_array() {
        let Some(values) = values.iter().map(Value::as_str).collect::<Option<Vec<_>>>() else {
            return false;
        };
        values.into_iter().map(str::to_owned).collect()
    } else if let Some(command) = command.as_str() {
        let Some(argv) = conservative_shell_argv(command) else {
            return false;
        };
        argv
    } else {
        return false;
    };
    argv.len() >= 2
        && Path::new(&argv[0])
            .file_name()
            .and_then(|name| name.to_str())
            == Some("codex")
        && argv[1] == "exec"
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Quote {
    None,
    Single,
    Double,
}

fn conservative_shell_argv(command: &str) -> Option<Vec<String>> {
    let mut argv = Vec::new();
    let mut current = String::new();
    let mut quote = Quote::None;
    let mut escaped = false;
    for character in command.chars() {
        if escaped {
            current.push(character);
            escaped = false;
            continue;
        }
        match quote {
            Quote::Single => {
                if character == '\'' {
                    quote = Quote::None;
                } else {
                    current.push(character);
                }
            }
            Quote::Double => match character {
                '"' => quote = Quote::None,
                '\\' => escaped = true,
                '$' | '`' => return None,
                _ => current.push(character),
            },
            Quote::None => match character {
                '\'' => quote = Quote::Single,
                '"' => quote = Quote::Double,
                '\\' => escaped = true,
                character if character.is_whitespace() => {
                    if !current.is_empty() {
                        argv.push(std::mem::take(&mut current));
                    }
                }
                ';' | '|' | '&' | '<' | '>' | '$' | '`' | '\n' | '\r' => return None,
                _ => current.push(character),
            },
        }
    }
    if escaped || quote != Quote::None {
        return None;
    }
    if !current.is_empty() {
        argv.push(current);
    }
    (!argv.is_empty()).then_some(argv)
}

/// Claude MCP tool names use `mcp__<server>__<tool>`; only the exact server segment `codex`
/// is accepted so a similarly named server cannot be mislabeled.
#[must_use]
pub fn is_codex_mcp_tool(name: &str) -> bool {
    let mut parts = name.split("__");
    parts.next() == Some("mcp")
        && parts
            .next()
            .is_some_and(|server| server.eq_ignore_ascii_case("codex"))
        && parts.next().is_some()
}
