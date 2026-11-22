use crate::*;
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
#[serde(tag = "method", content = "params")]
pub enum Request {
    #[serde(rename = "initialize")]
    Initialize(InitializeParams),
    #[serde(rename = "session/prompt")]
    SessionPrompt(SessionPromptParams),
    #[serde(rename = "session/query")]
    SessionQuery(SessionQueryParams),
    #[serde(rename = "session/resume")]
    SessionResume(SessionResumeParams),
    #[serde(rename = "session/wait")]
    SessionWait(SessionWaitParams),
    #[serde(rename = "session/working-directory/get")]
    WorkingDirectoryGet(SessionWorkingDirectoryParams),
    #[serde(rename = "session/working-directory/set")]
    WorkingDirectorySet(SessionWorkingDirectorySetParams),
    #[serde(rename = "shutdown")]
    Shutdown,
    #[serde(rename = "session/list")]
    SessionList(SessionListParams),
    #[serde(rename = "session/events")]
    SessionEvents(SessionEventsParams),
    #[serde(rename = "session/cancel")]
    SessionCancel(SessionResumeParams),
    #[serde(rename = "hoh/employee/list")]
    EmployeeList(Empty),
    #[serde(rename = "hoh/employee/get")]
    EmployeeGet(IdParams),
    #[serde(rename = "hoh/employee/create")]
    EmployeeCreate(EmployeeCreateParams),
    #[serde(rename = "hoh/employee/update")]
    EmployeeUpdate(EmployeeUpdateParams),
    #[serde(rename = "hoh/employee/delete")]
    EmployeeDelete(IdParams),
    #[serde(rename = "hoh/employee/memory/list")]
    EmployeeMemoryList(IdParams),
    #[serde(rename = "hoh/employee/memory/put")]
    EmployeeMemoryPut(MemoryPutParams),
    #[serde(rename = "hoh/employee/memory/delete")]
    EmployeeMemoryDelete(MemoryDeleteParams),
    #[serde(rename = "hoh/employee/open")]
    EmployeeOpen(IdParams),
    #[serde(rename = "hoh/task/draft/start")]
    TaskDraftStart(TaskDraftStartParams),
    #[serde(rename = "hoh/task/draft/decide")]
    TaskDraftDecide(TaskDraftDecideParams),
    #[serde(rename = "hoh/task/list")]
    TaskList(Empty),
    #[serde(rename = "hoh/task/get")]
    TaskGet(IdParams),
    #[serde(rename = "hoh/task/versions")]
    TaskVersions(IdParams),
    #[serde(rename = "hoh/task/rollback")]
    TaskRollback(TaskRollbackParams),
    #[serde(rename = "hoh/task/delete")]
    TaskDelete(IdParams),
    #[serde(rename = "hoh/task/run")]
    TaskRun(TaskRunParams),
    #[serde(rename = "hoh/task/knowledge/get")]
    TaskKnowledgeGet(TaskIdParams),
    #[serde(rename = "hoh/task/knowledge/edit")]
    TaskKnowledgeEdit(TaskKnowledgeEditParams),
    #[serde(rename = "hoh/button/list")]
    ButtonList(Empty),
    #[serde(rename = "hoh/button/create")]
    ButtonCreate(ButtonCreateParams),
    #[serde(rename = "hoh/button/update")]
    ButtonUpdate(ButtonUpdateParams),
    #[serde(rename = "hoh/button/delete")]
    ButtonDelete(IdParams),
    #[serde(rename = "hoh/schedule/list")]
    ScheduleList(Empty),
    #[serde(rename = "hoh/schedule/create")]
    ScheduleCreate(ScheduleCreateParams),
    #[serde(rename = "hoh/schedule/update")]
    ScheduleUpdate(ScheduleUpdateParams),
    #[serde(rename = "hoh/schedule/delete")]
    ScheduleDelete(IdParams),
    #[serde(rename = "hoh/schedule/run-now")]
    ScheduleRunNow(IdParams),
    #[serde(rename = "hoh/schedule/history")]
    ScheduleHistory(IdParams),
    #[serde(rename = "hoh/artifact/list")]
    ArtifactList(ArtifactListParams),
    #[serde(rename = "hoh/artifact/read")]
    ArtifactRead(ArtifactReadParams),
    #[serde(rename = "hoh/artifact/render")]
    ArtifactRender(ArtifactRenderParams),
    #[serde(rename = "hoh/model/key/set")]
    ModelKeySet(ModelKeySetParams),
    #[serde(rename = "hoh/settings/rules/set")]
    SettingsRulesSet(SettingsRulesSetParams),
    #[serde(rename = "hoh/model/probe")]
    ModelProbe(ModelProbeParams),
    #[serde(rename = "hoh/settings/get")]
    SettingsGet(Empty),
    #[serde(rename = "hoh/settings/set")]
    SettingsSet(SettingsSetParams),
    #[serde(rename = "hoh/security/summary")]
    SecuritySummary(Empty),
    #[serde(rename = "hoh/executor/discover")]
    ExecutorDiscover(ExecutorParams),
    #[serde(rename = "hoh/executor/select")]
    ExecutorSelect(ExecutorSelectParams),
    #[serde(rename = "hoh/executor/access/get")]
    ExecutorAccessGet(ExecutorAccessGetParams),
    #[serde(rename = "hoh/executor/access/set")]
    ExecutorAccessSet(ExecutorAccessSetParams),
    #[serde(rename = "approval/request")]
    ApprovalRequest(ApprovalRequestParams),
    #[serde(rename = "user-question/ask")]
    UserQuestionAsk(SdkUserQuestionRequest),
}
impl Request {
    pub fn method(&self) -> &'static str {
        match self {
            Self::Initialize(_) => "initialize",
            Self::SessionPrompt(_) => "session/prompt",
            Self::SessionQuery(_) => "session/query",
            Self::SessionResume(_) => "session/resume",
            Self::SessionWait(_) => "session/wait",
            Self::WorkingDirectoryGet(_) => "session/working-directory/get",
            Self::WorkingDirectorySet(_) => "session/working-directory/set",
            Self::Shutdown => "shutdown",
            Self::SessionList(_) => "session/list",
            Self::SessionEvents(_) => "session/events",
            Self::SessionCancel(_) => "session/cancel",
            Self::EmployeeList(_) => "hoh/employee/list",
            Self::EmployeeGet(_) => "hoh/employee/get",
            Self::EmployeeCreate(_) => "hoh/employee/create",
            Self::EmployeeUpdate(_) => "hoh/employee/update",
            Self::EmployeeDelete(_) => "hoh/employee/delete",
            Self::EmployeeMemoryList(_) => "hoh/employee/memory/list",
            Self::EmployeeMemoryPut(_) => "hoh/employee/memory/put",
            Self::EmployeeMemoryDelete(_) => "hoh/employee/memory/delete",
            Self::EmployeeOpen(_) => "hoh/employee/open",
            Self::TaskDraftStart(_) => "hoh/task/draft/start",
            Self::TaskDraftDecide(_) => "hoh/task/draft/decide",
            Self::TaskList(_) => "hoh/task/list",
            Self::TaskGet(_) => "hoh/task/get",
            Self::TaskVersions(_) => "hoh/task/versions",
            Self::TaskRollback(_) => "hoh/task/rollback",
            Self::TaskDelete(_) => "hoh/task/delete",
            Self::TaskRun(_) => "hoh/task/run",
            Self::TaskKnowledgeGet(_) => "hoh/task/knowledge/get",
            Self::TaskKnowledgeEdit(_) => "hoh/task/knowledge/edit",
            Self::ButtonList(_) => "hoh/button/list",
            Self::ButtonCreate(_) => "hoh/button/create",
            Self::ButtonUpdate(_) => "hoh/button/update",
            Self::ButtonDelete(_) => "hoh/button/delete",
            Self::ScheduleList(_) => "hoh/schedule/list",
            Self::ScheduleCreate(_) => "hoh/schedule/create",
            Self::ScheduleUpdate(_) => "hoh/schedule/update",
            Self::ScheduleDelete(_) => "hoh/schedule/delete",
            Self::ScheduleRunNow(_) => "hoh/schedule/run-now",
            Self::ScheduleHistory(_) => "hoh/schedule/history",
            Self::ArtifactList(_) => "hoh/artifact/list",
            Self::ArtifactRead(_) => "hoh/artifact/read",
            Self::ArtifactRender(_) => "hoh/artifact/render",
            Self::ModelKeySet(_) => "hoh/model/key/set",
            Self::SettingsRulesSet(_) => "hoh/settings/rules/set",
            Self::ModelProbe(_) => "hoh/model/probe",
            Self::SettingsGet(_) => "hoh/settings/get",
            Self::SettingsSet(_) => "hoh/settings/set",
            Self::SecuritySummary(_) => "hoh/security/summary",
            Self::ExecutorDiscover(_) => "hoh/executor/discover",
            Self::ExecutorSelect(_) => "hoh/executor/select",
            Self::ExecutorAccessGet(_) => "hoh/executor/access/get",
            Self::ExecutorAccessSet(_) => "hoh/executor/access/set",
            Self::ApprovalRequest(_) => "approval/request",
            Self::UserQuestionAsk(_) => "user-question/ask",
        }
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "method", content = "params")]
pub enum Notification {
    #[serde(rename = "user-question.cancelled")]
    UserQuestionCancelled(SdkUserQuestionCancelled),
    #[serde(rename = "session.event")]
    SessionEvent(SessionEventNotification),
    #[serde(rename = "session.assistant-stream")]
    SessionAssistantStream(SessionAssistantStreamNotification),
    #[serde(rename = "session.status")]
    SessionStatus(SessionStatusNotification),
    #[serde(rename = "session.query.completed")]
    SessionQueryCompleted(SessionQueryCompleted),
    #[serde(rename = "subagent.started")]
    SubagentStarted(SubagentStartedNotification),
    #[serde(rename = "subagent.finished")]
    SubagentFinished(SubagentFinishedNotification),
    #[serde(rename = "employee.changed")]
    EmployeeChanged(EmployeeChanged),
    #[serde(rename = "task.changed")]
    TaskChanged(TaskChanged),
    #[serde(rename = "task.draft.state")]
    TaskDraftState(TaskDraftState),
    #[serde(rename = "task.example")]
    TaskExample(TaskExample),
    #[serde(rename = "schedule.fired")]
    ScheduleFired(ScheduleFired),
    #[serde(rename = "schedule.changed")]
    ScheduleChanged(ScheduleChanged),
    #[serde(rename = "artifact.presented")]
    ArtifactPresented(ArtifactPresented),
    #[serde(rename = "approval.pending")]
    ApprovalPending(ApprovalPending),
    #[serde(rename = "approval.resolved")]
    ApprovalResolved(ApprovalResolved),
    #[serde(rename = "security.egress")]
    SecurityEgress(SecurityEgress),
}
pub const REQUEST_METHODS: &[&str] = &[
    "initialize",
    "session/prompt",
    "session/query",
    "session/resume",
    "session/wait",
    "session/working-directory/get",
    "session/working-directory/set",
    "shutdown",
    "session/list",
    "session/events",
    "session/cancel",
    "hoh/employee/list",
    "hoh/employee/get",
    "hoh/employee/create",
    "hoh/employee/update",
    "hoh/employee/delete",
    "hoh/employee/memory/list",
    "hoh/employee/memory/put",
    "hoh/employee/memory/delete",
    "hoh/employee/open",
    "hoh/task/draft/start",
    "hoh/task/draft/decide",
    "hoh/task/list",
    "hoh/task/get",
    "hoh/task/versions",
    "hoh/task/rollback",
    "hoh/task/delete",
    "hoh/task/run",
    "hoh/task/knowledge/get",
    "hoh/task/knowledge/edit",
    "hoh/button/list",
    "hoh/button/create",
    "hoh/button/update",
    "hoh/button/delete",
    "hoh/schedule/list",
    "hoh/schedule/create",
    "hoh/schedule/update",
    "hoh/schedule/delete",
    "hoh/schedule/run-now",
    "hoh/schedule/history",
    "hoh/artifact/list",
    "hoh/artifact/read",
    "hoh/artifact/render",
    "hoh/model/key/set",
    "hoh/settings/rules/set",
    "hoh/model/probe",
    "hoh/settings/get",
    "hoh/settings/set",
    "hoh/security/summary",
    "hoh/executor/discover",
    "hoh/executor/select",
    "hoh/executor/access/get",
    "hoh/executor/access/set",
    "approval/request",
    "user-question/ask",
];
pub const NOTIFICATION_METHODS: &[&str] = &[
    "user-question.cancelled",
    "session.event",
    "session.assistant-stream",
    "session.status",
    "session.query.completed",
    "subagent.started",
    "subagent.finished",
    "employee.changed",
    "task.changed",
    "task.draft.state",
    "task.example",
    "schedule.fired",
    "schedule.changed",
    "artifact.presented",
    "approval.pending",
    "approval.resolved",
    "security.egress",
];
fn admit<T: serde::de::DeserializeOwned>(value: &Value) -> Result<(), String> {
    serde_json::from_value::<T>(value.clone())
        .map(|_| ())
        .map_err(|e| e.to_string())
}
pub fn validate_params(method: &str, value: &Value) -> Result<(), String> {
    match method {
        "initialize" => admit::<InitializeParams>(value),
        "session/prompt" => admit::<SessionPromptParams>(value),
        "session/query" => admit::<SessionQueryParams>(value),
        "session/resume" => admit::<SessionResumeParams>(value),
        "session/wait" => admit::<SessionWaitParams>(value),
        "session/working-directory/get" => admit::<SessionWorkingDirectoryParams>(value),
        "session/working-directory/set" => admit::<SessionWorkingDirectorySetParams>(value),
        "shutdown" => admit::<Empty>(value),
        "session/list" => admit::<SessionListParams>(value),
        "session/events" => admit::<SessionEventsParams>(value),
        "session/cancel" => admit::<SessionResumeParams>(value),
        "hoh/employee/list" => admit::<Empty>(value),
        "hoh/employee/get" => admit::<IdParams>(value),
        "hoh/employee/create" => admit::<EmployeeCreateParams>(value),
        "hoh/employee/update" => admit::<EmployeeUpdateParams>(value),
        "hoh/employee/delete" => admit::<IdParams>(value),
        "hoh/employee/memory/list" => admit::<IdParams>(value),
        "hoh/employee/memory/put" => admit::<MemoryPutParams>(value),
        "hoh/employee/memory/delete" => admit::<MemoryDeleteParams>(value),
        "hoh/employee/open" => admit::<IdParams>(value),
        "hoh/task/draft/start" => admit::<TaskDraftStartParams>(value),
        "hoh/task/draft/decide" => admit::<TaskDraftDecideParams>(value),
        "hoh/task/list" => admit::<Empty>(value),
        "hoh/task/get" => admit::<IdParams>(value),
        "hoh/task/versions" => admit::<IdParams>(value),
        "hoh/task/rollback" => admit::<TaskRollbackParams>(value),
        "hoh/task/delete" => admit::<IdParams>(value),
        "hoh/task/run" => admit::<TaskRunParams>(value),
        "hoh/task/knowledge/get" => admit::<TaskIdParams>(value),
        "hoh/task/knowledge/edit" => admit::<TaskKnowledgeEditParams>(value),
        "hoh/button/list" => admit::<Empty>(value),
        "hoh/button/create" => admit::<ButtonCreateParams>(value),
        "hoh/button/update" => admit::<ButtonUpdateParams>(value),
        "hoh/button/delete" => admit::<IdParams>(value),
        "hoh/schedule/list" => admit::<Empty>(value),
        "hoh/schedule/create" => admit::<ScheduleCreateParams>(value),
        "hoh/schedule/update" => admit::<ScheduleUpdateParams>(value),
        "hoh/schedule/delete" => admit::<IdParams>(value),
        "hoh/schedule/run-now" => admit::<IdParams>(value),
        "hoh/schedule/history" => admit::<IdParams>(value),
        "hoh/artifact/list" => admit::<ArtifactListParams>(value),
        "hoh/artifact/read" => admit::<ArtifactReadParams>(value),
        "hoh/artifact/render" => admit::<ArtifactRenderParams>(value),
        "hoh/model/key/set" => admit::<ModelKeySetParams>(value),
        "hoh/settings/rules/set" => admit::<SettingsRulesSetParams>(value),
        "hoh/model/probe" => admit::<ModelProbeParams>(value),
        "hoh/settings/get" => admit::<Empty>(value),
        "hoh/settings/set" => admit::<SettingsSetParams>(value),
        "hoh/security/summary" => admit::<Empty>(value),
        "hoh/executor/discover" => admit::<ExecutorParams>(value),
        "hoh/executor/select" => admit::<ExecutorSelectParams>(value),
        "hoh/executor/access/get" => admit::<ExecutorAccessGetParams>(value),
        "hoh/executor/access/set" => admit::<ExecutorAccessSetParams>(value),
        "approval/request" => admit::<ApprovalRequestParams>(value),
        "user-question/ask" => admit::<SdkUserQuestionRequest>(value),
        _ => Err(format!("unknown method: {method}")),
    }
}
pub fn validate_result(method: &str, value: &Value) -> Result<(), String> {
    if method == "user-question/ask" && value.get("cancelled").is_some() {
        return if value.as_object().is_some_and(|v| v.len() == 1) && value["cancelled"] == true {
            Ok(())
        } else {
            Err("cancellation requires only {cancelled:true}".into())
        };
    }
    match method {
        "initialize" => admit::<InitializeResult>(value),
        "session/prompt" => admit::<SessionPromptResult>(value),
        "session/query" => admit::<SessionQueryResult>(value),
        "session/resume" => admit::<Empty>(value),
        "session/wait" => admit::<Empty>(value),
        "session/working-directory/get" => admit::<SessionWorkingDirectoryResult>(value),
        "session/working-directory/set" => admit::<SessionWorkingDirectoryResult>(value),
        "shutdown" => admit::<Empty>(value),
        "session/list" => admit::<SessionListResult>(value),
        "session/events" => admit::<SessionEventsResult>(value),
        "session/cancel" => admit::<Empty>(value),
        "hoh/employee/list" => admit::<Vec<Employee>>(value),
        "hoh/employee/get" => admit::<Employee>(value),
        "hoh/employee/create" => admit::<IdRev>(value),
        "hoh/employee/update" => admit::<IdRev>(value),
        "hoh/employee/delete" => admit::<IdRev>(value),
        "hoh/employee/memory/list" => admit::<Vec<MemoryItem>>(value),
        "hoh/employee/memory/put" => admit::<MemoryItem>(value),
        "hoh/employee/memory/delete" => admit::<Empty>(value),
        "hoh/employee/open" => admit::<SessionIdResult>(value),
        "hoh/task/draft/start" => admit::<TaskDraftStartResult>(value),
        "hoh/task/draft/decide" => admit::<TaskDraftDecideResult>(value),
        "hoh/task/list" => admit::<Vec<Task>>(value),
        "hoh/task/get" => admit::<Task>(value),
        "hoh/task/versions" => admit::<Vec<TaskVersion>>(value),
        "hoh/task/rollback" => admit::<Task>(value),
        "hoh/task/delete" => admit::<Empty>(value),
        "hoh/task/run" => admit::<TaskRunResult>(value),
        "hoh/task/knowledge/get" => admit::<TaskKnowledge>(value),
        "hoh/task/knowledge/edit" => admit::<TaskDraftStartResult>(value),
        "hoh/button/list" => admit::<Vec<Button>>(value),
        "hoh/button/create" => admit::<Button>(value),
        "hoh/button/update" => admit::<Button>(value),
        "hoh/button/delete" => admit::<Empty>(value),
        "hoh/schedule/list" => admit::<Vec<Schedule>>(value),
        "hoh/schedule/create" => admit::<Schedule>(value),
        "hoh/schedule/update" => admit::<Schedule>(value),
        "hoh/schedule/delete" => admit::<Empty>(value),
        "hoh/schedule/run-now" => admit::<TaskRunResult>(value),
        "hoh/schedule/history" => admit::<Vec<ScheduleRun>>(value),
        "hoh/artifact/list" => admit::<Vec<Artifact>>(value),
        "hoh/artifact/read" => admit::<ArtifactReadResult>(value),
        "hoh/artifact/render" => admit::<ArtifactRenderResult>(value),
        "hoh/model/key/set" | "hoh/settings/rules/set" => admit::<Empty>(value),
        "hoh/model/probe" => admit::<ModelProbeResult>(value),
        "hoh/settings/get" => admit::<Settings>(value),
        "hoh/settings/set" => admit::<Settings>(value),
        "hoh/security/summary" => admit::<SecuritySummary>(value),
        "hoh/executor/discover" => admit::<ExecutorDiscoverResult>(value),
        "hoh/executor/select" => admit::<ExecutorSelection>(value),
        "hoh/executor/access/get" => admit::<ExecutorAccessGetResult>(value),
        "hoh/executor/access/set" => admit::<ExecutorAccessSetResult>(value),
        "approval/request" => admit::<ApprovalRequestResult>(value),
        "user-question/ask" => admit::<SdkUserQuestionResult>(value),
        _ => Err(format!("unknown method: {method}")),
    }
}
pub fn validate_notification(method: &str, value: &Value) -> Result<(), String> {
    match method {
        "user-question.cancelled" => admit::<SdkUserQuestionCancelled>(value),
        "session.event" => admit::<SessionEventNotification>(value),
        "session.assistant-stream" => admit::<SessionAssistantStreamNotification>(value),
        "session.status" => admit::<SessionStatusNotification>(value),
        "session.query.completed" => admit::<SessionQueryCompleted>(value),
        "subagent.started" => admit::<SubagentStartedNotification>(value),
        "subagent.finished" => admit::<SubagentFinishedNotification>(value),
        "employee.changed" => admit::<EmployeeChanged>(value),
        "task.changed" => admit::<TaskChanged>(value),
        "task.draft.state" => admit::<TaskDraftState>(value),
        "task.example" => admit::<TaskExample>(value),
        "schedule.fired" => admit::<ScheduleFired>(value),
        "schedule.changed" => admit::<ScheduleChanged>(value),
        "artifact.presented" => admit::<ArtifactPresented>(value),
        "approval.pending" => admit::<ApprovalPending>(value),
        "approval.resolved" => admit::<ApprovalResolved>(value),
        "security.egress" => admit::<SecurityEgress>(value),
        _ => Err(format!("unknown notification: {method}")),
    }
}
/// Every endpoint has its own correlated result schema, rather than an ambiguous union.
pub fn schemas() -> Vec<(String, Value)> {
    let mut schemas = vec![
        (
            "HostApprovalRequest".into(),
            serde_json::to_value(schema_for!(HostApprovalRequest)).unwrap(),
        ),
        (
            "HostApprovalDecision".into(),
            serde_json::to_value(schema_for!(HostApprovalDecision)).unwrap(),
        ),
        (
            "Request".into(),
            serde_json::to_value(schema_for!(Request)).unwrap(),
        ),
        (
            "Notification".into(),
            serde_json::to_value(schema_for!(Notification)).unwrap(),
        ),
        (
            "RequestFrame".into(),
            serde_json::to_value(schema_for!(crate::framing::RequestFrame)).unwrap(),
        ),
        (
            "NotificationFrame".into(),
            serde_json::to_value(schema_for!(crate::framing::NotificationFrame)).unwrap(),
        ),
        (
            "ResponseFrame".into(),
            serde_json::to_value(schema_for!(crate::framing::ResponseFrame)).unwrap(),
        ),
    ];
    schemas.push((
        "InitializeParams".into(),
        serde_json::to_value(schema_for!(InitializeParams)).unwrap(),
    ));
    {
        let mut schema = serde_json::to_value(schema_for!(InitializeResult)).unwrap();
        schema
            .as_object_mut()
            .unwrap()
            .insert("x-method".into(), Value::String("initialize".into()));
        schemas.push(("InitializeResult".into(), schema));
    }
    schemas.push((
        "SessionPromptParams".into(),
        serde_json::to_value(schema_for!(SessionPromptParams)).unwrap(),
    ));
    {
        let mut schema = serde_json::to_value(schema_for!(SessionPromptResult)).unwrap();
        schema
            .as_object_mut()
            .unwrap()
            .insert("x-method".into(), Value::String("session/prompt".into()));
        schemas.push(("SessionPromptResult".into(), schema));
    }
    schemas.push((
        "SessionQueryParams".into(),
        serde_json::to_value(schema_for!(SessionQueryParams)).unwrap(),
    ));
    {
        let mut schema = serde_json::to_value(schema_for!(SessionQueryResult)).unwrap();
        schema
            .as_object_mut()
            .unwrap()
            .insert("x-method".into(), Value::String("session/query".into()));
        schemas.push(("SessionQueryResult".into(), schema));
    }
    schemas.push((
        "SessionResumeParams".into(),
        serde_json::to_value(schema_for!(SessionResumeParams)).unwrap(),
    ));
    {
        let mut schema = serde_json::to_value(schema_for!(Empty)).unwrap();
        schema
            .as_object_mut()
            .unwrap()
            .insert("x-method".into(), Value::String("session/resume".into()));
        schemas.push(("SessionResumeResult".into(), schema));
    }
    schemas.push((
        "SessionWaitParams".into(),
        serde_json::to_value(schema_for!(SessionWaitParams)).unwrap(),
    ));
    {
        let mut schema = serde_json::to_value(schema_for!(Empty)).unwrap();
        schema
            .as_object_mut()
            .unwrap()
            .insert("x-method".into(), Value::String("session/wait".into()));
        schemas.push(("SessionWaitResult".into(), schema));
    }
    schemas.push((
        "WorkingDirectoryGetParams".into(),
        serde_json::to_value(schema_for!(SessionWorkingDirectoryParams)).unwrap(),
    ));
    {
        let mut schema = serde_json::to_value(schema_for!(SessionWorkingDirectoryResult)).unwrap();
        schema.as_object_mut().unwrap().insert(
            "x-method".into(),
            Value::String("session/working-directory/get".into()),
        );
        schemas.push(("WorkingDirectoryGetResult".into(), schema));
    }
    schemas.push((
        "WorkingDirectorySetParams".into(),
        serde_json::to_value(schema_for!(SessionWorkingDirectorySetParams)).unwrap(),
    ));
    {
        let mut schema = serde_json::to_value(schema_for!(SessionWorkingDirectoryResult)).unwrap();
        schema.as_object_mut().unwrap().insert(
            "x-method".into(),
            Value::String("session/working-directory/set".into()),
        );
        schemas.push(("WorkingDirectorySetResult".into(), schema));
    }
    schemas.push((
        "ShutdownParams".into(),
        serde_json::to_value(schema_for!(Empty)).unwrap(),
    ));
    {
        let mut schema = serde_json::to_value(schema_for!(Empty)).unwrap();
        schema
            .as_object_mut()
            .unwrap()
            .insert("x-method".into(), Value::String("shutdown".into()));
        schemas.push(("ShutdownResult".into(), schema));
    }
    schemas.push((
        "SessionListParams".into(),
        serde_json::to_value(schema_for!(SessionListParams)).unwrap(),
    ));
    {
        let mut schema = serde_json::to_value(schema_for!(SessionListResult)).unwrap();
        schema
            .as_object_mut()
            .unwrap()
            .insert("x-method".into(), Value::String("session/list".into()));
        schemas.push(("SessionListResult".into(), schema));
    }
    schemas.push((
        "SessionEventsParams".into(),
        serde_json::to_value(schema_for!(SessionEventsParams)).unwrap(),
    ));
    {
        let mut schema = serde_json::to_value(schema_for!(SessionEventsResult)).unwrap();
        schema
            .as_object_mut()
            .unwrap()
            .insert("x-method".into(), Value::String("session/events".into()));
        schemas.push(("SessionEventsResult".into(), schema));
    }
    schemas.push((
        "SessionCancelParams".into(),
        serde_json::to_value(schema_for!(SessionResumeParams)).unwrap(),
    ));
    {
        let mut schema = serde_json::to_value(schema_for!(Empty)).unwrap();
        schema
            .as_object_mut()
            .unwrap()
            .insert("x-method".into(), Value::String("session/cancel".into()));
        schemas.push(("SessionCancelResult".into(), schema));
    }
    schemas.push((
        "EmployeeListParams".into(),
        serde_json::to_value(schema_for!(Empty)).unwrap(),
    ));
    {
        let mut schema = serde_json::to_value(schema_for!(Vec<Employee>)).unwrap();
        schema
            .as_object_mut()
            .unwrap()
            .insert("x-method".into(), Value::String("hoh/employee/list".into()));
        schemas.push(("EmployeeListResult".into(), schema));
    }
    schemas.push((
        "EmployeeGetParams".into(),
        serde_json::to_value(schema_for!(IdParams)).unwrap(),
    ));
    {
        let mut schema = serde_json::to_value(schema_for!(Employee)).unwrap();
        schema
            .as_object_mut()
            .unwrap()
            .insert("x-method".into(), Value::String("hoh/employee/get".into()));
        schemas.push(("EmployeeGetResult".into(), schema));
    }
    schemas.push((
        "EmployeeCreateParams".into(),
        serde_json::to_value(schema_for!(EmployeeCreateParams)).unwrap(),
    ));
    {
        let mut schema = serde_json::to_value(schema_for!(IdRev)).unwrap();
        schema.as_object_mut().unwrap().insert(
            "x-method".into(),
            Value::String("hoh/employee/create".into()),
        );
        schemas.push(("EmployeeCreateResult".into(), schema));
    }
    schemas.push((
        "EmployeeUpdateParams".into(),
        serde_json::to_value(schema_for!(EmployeeUpdateParams)).unwrap(),
    ));
    {
        let mut schema = serde_json::to_value(schema_for!(IdRev)).unwrap();
        schema.as_object_mut().unwrap().insert(
            "x-method".into(),
            Value::String("hoh/employee/update".into()),
        );
        schemas.push(("EmployeeUpdateResult".into(), schema));
    }
    schemas.push((
        "EmployeeDeleteParams".into(),
        serde_json::to_value(schema_for!(IdParams)).unwrap(),
    ));
    {
        let mut schema = serde_json::to_value(schema_for!(IdRev)).unwrap();
        schema.as_object_mut().unwrap().insert(
            "x-method".into(),
            Value::String("hoh/employee/delete".into()),
        );
        schemas.push(("EmployeeDeleteResult".into(), schema));
    }
    schemas.push((
        "EmployeeMemoryListParams".into(),
        serde_json::to_value(schema_for!(IdParams)).unwrap(),
    ));
    {
        let mut schema = serde_json::to_value(schema_for!(Vec<MemoryItem>)).unwrap();
        schema.as_object_mut().unwrap().insert(
            "x-method".into(),
            Value::String("hoh/employee/memory/list".into()),
        );
        schemas.push(("EmployeeMemoryListResult".into(), schema));
    }
    schemas.push((
        "EmployeeMemoryPutParams".into(),
        serde_json::to_value(schema_for!(MemoryPutParams)).unwrap(),
    ));
    {
        let mut schema = serde_json::to_value(schema_for!(MemoryItem)).unwrap();
        schema.as_object_mut().unwrap().insert(
            "x-method".into(),
            Value::String("hoh/employee/memory/put".into()),
        );
        schemas.push(("EmployeeMemoryPutResult".into(), schema));
    }
    schemas.push((
        "EmployeeMemoryDeleteParams".into(),
        serde_json::to_value(schema_for!(MemoryDeleteParams)).unwrap(),
    ));
    {
        let mut schema = serde_json::to_value(schema_for!(Empty)).unwrap();
        schema.as_object_mut().unwrap().insert(
            "x-method".into(),
            Value::String("hoh/employee/memory/delete".into()),
        );
        schemas.push(("EmployeeMemoryDeleteResult".into(), schema));
    }
    schemas.push((
        "EmployeeOpenParams".into(),
        serde_json::to_value(schema_for!(IdParams)).unwrap(),
    ));
    {
        let mut schema = serde_json::to_value(schema_for!(SessionIdResult)).unwrap();
        schema
            .as_object_mut()
            .unwrap()
            .insert("x-method".into(), Value::String("hoh/employee/open".into()));
        schemas.push(("EmployeeOpenResult".into(), schema));
    }
    schemas.push((
        "TaskDraftStartParams".into(),
        serde_json::to_value(schema_for!(TaskDraftStartParams)).unwrap(),
    ));
    {
        let mut schema = serde_json::to_value(schema_for!(TaskDraftStartResult)).unwrap();
        schema.as_object_mut().unwrap().insert(
            "x-method".into(),
            Value::String("hoh/task/draft/start".into()),
        );
        schemas.push(("TaskDraftStartResult".into(), schema));
    }
    schemas.push((
        "TaskDraftDecideParams".into(),
        serde_json::to_value(schema_for!(TaskDraftDecideParams)).unwrap(),
    ));
    {
        let mut schema = serde_json::to_value(schema_for!(TaskDraftDecideResult)).unwrap();
        schema.as_object_mut().unwrap().insert(
            "x-method".into(),
            Value::String("hoh/task/draft/decide".into()),
        );
        schemas.push(("TaskDraftDecideResult".into(), schema));
    }
    schemas.push((
        "TaskListParams".into(),
        serde_json::to_value(schema_for!(Empty)).unwrap(),
    ));
    {
        let mut schema = serde_json::to_value(schema_for!(Vec<Task>)).unwrap();
        schema
            .as_object_mut()
            .unwrap()
            .insert("x-method".into(), Value::String("hoh/task/list".into()));
        schemas.push(("TaskListResult".into(), schema));
    }
    schemas.push((
        "TaskGetParams".into(),
        serde_json::to_value(schema_for!(IdParams)).unwrap(),
    ));
    {
        let mut schema = serde_json::to_value(schema_for!(Task)).unwrap();
        schema
            .as_object_mut()
            .unwrap()
            .insert("x-method".into(), Value::String("hoh/task/get".into()));
        schemas.push(("TaskGetResult".into(), schema));
    }
    schemas.push((
        "TaskVersionsParams".into(),
        serde_json::to_value(schema_for!(IdParams)).unwrap(),
    ));
    {
        let mut schema = serde_json::to_value(schema_for!(Vec<TaskVersion>)).unwrap();
        schema
            .as_object_mut()
            .unwrap()
            .insert("x-method".into(), Value::String("hoh/task/versions".into()));
        schemas.push(("TaskVersionsResult".into(), schema));
    }
    schemas.push((
        "TaskRollbackParams".into(),
        serde_json::to_value(schema_for!(TaskRollbackParams)).unwrap(),
    ));
    {
        let mut schema = serde_json::to_value(schema_for!(Task)).unwrap();
        schema
            .as_object_mut()
            .unwrap()
            .insert("x-method".into(), Value::String("hoh/task/rollback".into()));
        schemas.push(("TaskRollbackResult".into(), schema));
    }
    schemas.push((
        "TaskDeleteParams".into(),
        serde_json::to_value(schema_for!(IdParams)).unwrap(),
    ));
    {
        let mut schema = serde_json::to_value(schema_for!(Empty)).unwrap();
        schema
            .as_object_mut()
            .unwrap()
            .insert("x-method".into(), Value::String("hoh/task/delete".into()));
        schemas.push(("TaskDeleteResult".into(), schema));
    }
    schemas.push((
        "TaskRunParams".into(),
        serde_json::to_value(schema_for!(TaskRunParams)).unwrap(),
    ));
    {
        let mut schema = serde_json::to_value(schema_for!(TaskRunResult)).unwrap();
        schema
            .as_object_mut()
            .unwrap()
            .insert("x-method".into(), Value::String("hoh/task/run".into()));
        schemas.push(("TaskRunResult".into(), schema));
    }
    schemas.push((
        "TaskKnowledgeGetParams".into(),
        serde_json::to_value(schema_for!(TaskIdParams)).unwrap(),
    ));
    {
        let mut schema = serde_json::to_value(schema_for!(TaskKnowledge)).unwrap();
        schema.as_object_mut().unwrap().insert(
            "x-method".into(),
            Value::String("hoh/task/knowledge/get".into()),
        );
        schemas.push(("TaskKnowledgeGetResult".into(), schema));
    }
    schemas.push((
        "TaskKnowledgeEditParams".into(),
        serde_json::to_value(schema_for!(TaskKnowledgeEditParams)).unwrap(),
    ));
    {
        let mut schema = serde_json::to_value(schema_for!(TaskDraftStartResult)).unwrap();
        schema.as_object_mut().unwrap().insert(
            "x-method".into(),
            Value::String("hoh/task/knowledge/edit".into()),
        );
        schemas.push(("TaskKnowledgeEditResult".into(), schema));
    }
    schemas.push((
        "ButtonListParams".into(),
        serde_json::to_value(schema_for!(Empty)).unwrap(),
    ));
    {
        let mut schema = serde_json::to_value(schema_for!(Vec<Button>)).unwrap();
        schema
            .as_object_mut()
            .unwrap()
            .insert("x-method".into(), Value::String("hoh/button/list".into()));
        schemas.push(("ButtonListResult".into(), schema));
    }
    schemas.push((
        "ButtonCreateParams".into(),
        serde_json::to_value(schema_for!(ButtonCreateParams)).unwrap(),
    ));
    {
        let mut schema = serde_json::to_value(schema_for!(Button)).unwrap();
        schema
            .as_object_mut()
            .unwrap()
            .insert("x-method".into(), Value::String("hoh/button/create".into()));
        schemas.push(("ButtonCreateResult".into(), schema));
    }
    schemas.push((
        "ButtonUpdateParams".into(),
        serde_json::to_value(schema_for!(ButtonUpdateParams)).unwrap(),
    ));
    {
        let mut schema = serde_json::to_value(schema_for!(Button)).unwrap();
        schema
            .as_object_mut()
            .unwrap()
            .insert("x-method".into(), Value::String("hoh/button/update".into()));
        schemas.push(("ButtonUpdateResult".into(), schema));
    }
    schemas.push((
        "ButtonDeleteParams".into(),
        serde_json::to_value(schema_for!(IdParams)).unwrap(),
    ));
    {
        let mut schema = serde_json::to_value(schema_for!(Empty)).unwrap();
        schema
            .as_object_mut()
            .unwrap()
            .insert("x-method".into(), Value::String("hoh/button/delete".into()));
        schemas.push(("ButtonDeleteResult".into(), schema));
    }
    schemas.push((
        "ScheduleListParams".into(),
        serde_json::to_value(schema_for!(Empty)).unwrap(),
    ));
    {
        let mut schema = serde_json::to_value(schema_for!(Vec<Schedule>)).unwrap();
        schema
            .as_object_mut()
            .unwrap()
            .insert("x-method".into(), Value::String("hoh/schedule/list".into()));
        schemas.push(("ScheduleListResult".into(), schema));
    }
    schemas.push((
        "ScheduleCreateParams".into(),
        serde_json::to_value(schema_for!(ScheduleCreateParams)).unwrap(),
    ));
    {
        let mut schema = serde_json::to_value(schema_for!(Schedule)).unwrap();
        schema.as_object_mut().unwrap().insert(
            "x-method".into(),
            Value::String("hoh/schedule/create".into()),
        );
        schemas.push(("ScheduleCreateResult".into(), schema));
    }
    schemas.push((
        "ScheduleUpdateParams".into(),
        serde_json::to_value(schema_for!(ScheduleUpdateParams)).unwrap(),
    ));
    {
        let mut schema = serde_json::to_value(schema_for!(Schedule)).unwrap();
        schema.as_object_mut().unwrap().insert(
            "x-method".into(),
            Value::String("hoh/schedule/update".into()),
        );
        schemas.push(("ScheduleUpdateResult".into(), schema));
    }
    schemas.push((
        "ScheduleDeleteParams".into(),
        serde_json::to_value(schema_for!(IdParams)).unwrap(),
    ));
    {
        let mut schema = serde_json::to_value(schema_for!(Empty)).unwrap();
        schema.as_object_mut().unwrap().insert(
            "x-method".into(),
            Value::String("hoh/schedule/delete".into()),
        );
        schemas.push(("ScheduleDeleteResult".into(), schema));
    }
    schemas.push((
        "ScheduleRunNowParams".into(),
        serde_json::to_value(schema_for!(IdParams)).unwrap(),
    ));
    {
        let mut schema = serde_json::to_value(schema_for!(TaskRunResult)).unwrap();
        schema.as_object_mut().unwrap().insert(
            "x-method".into(),
            Value::String("hoh/schedule/run-now".into()),
        );
        schemas.push(("ScheduleRunNowResult".into(), schema));
    }
    schemas.push((
        "ScheduleHistoryParams".into(),
        serde_json::to_value(schema_for!(IdParams)).unwrap(),
    ));
    {
        let mut schema = serde_json::to_value(schema_for!(Vec<ScheduleRun>)).unwrap();
        schema.as_object_mut().unwrap().insert(
            "x-method".into(),
            Value::String("hoh/schedule/history".into()),
        );
        schemas.push(("ScheduleHistoryResult".into(), schema));
    }
    schemas.push((
        "ArtifactListParams".into(),
        serde_json::to_value(schema_for!(ArtifactListParams)).unwrap(),
    ));
    {
        let mut schema = serde_json::to_value(schema_for!(Vec<Artifact>)).unwrap();
        schema
            .as_object_mut()
            .unwrap()
            .insert("x-method".into(), Value::String("hoh/artifact/list".into()));
        schemas.push(("ArtifactListResult".into(), schema));
    }
    schemas.push((
        "ArtifactReadParams".into(),
        serde_json::to_value(schema_for!(ArtifactReadParams)).unwrap(),
    ));
    {
        let mut schema = serde_json::to_value(schema_for!(ArtifactReadResult)).unwrap();
        schema
            .as_object_mut()
            .unwrap()
            .insert("x-method".into(), Value::String("hoh/artifact/read".into()));
        schemas.push(("ArtifactReadResult".into(), schema));
    }
    schemas.push((
        "ArtifactRenderParams".into(),
        serde_json::to_value(schema_for!(ArtifactRenderParams)).unwrap(),
    ));
    {
        let mut schema = serde_json::to_value(schema_for!(ArtifactRenderResult)).unwrap();
        schema.as_object_mut().unwrap().insert(
            "x-method".into(),
            Value::String("hoh/artifact/render".into()),
        );
        schemas.push(("ArtifactRenderResult".into(), schema));
    }
    schemas.push((
        "ModelProbeParams".into(),
        serde_json::to_value(schema_for!(ModelProbeParams)).unwrap(),
    ));
    {
        let mut schema = serde_json::to_value(schema_for!(ModelProbeResult)).unwrap();
        schema
            .as_object_mut()
            .unwrap()
            .insert("x-method".into(), Value::String("hoh/model/probe".into()));
        schemas.push(("ModelProbeResult".into(), schema));
    }
    schemas.push((
        "SettingsGetParams".into(),
        serde_json::to_value(schema_for!(Empty)).unwrap(),
    ));
    {
        let mut schema = serde_json::to_value(schema_for!(Settings)).unwrap();
        schema
            .as_object_mut()
            .unwrap()
            .insert("x-method".into(), Value::String("hoh/settings/get".into()));
        schemas.push(("SettingsGetResult".into(), schema));
    }
    schemas.push((
        "SettingsSetParams".into(),
        serde_json::to_value(schema_for!(SettingsSetParams)).unwrap(),
    ));
    {
        let mut schema = serde_json::to_value(schema_for!(Settings)).unwrap();
        schema
            .as_object_mut()
            .unwrap()
            .insert("x-method".into(), Value::String("hoh/settings/set".into()));
        schemas.push(("SettingsSetResult".into(), schema));
    }
    schemas.push((
        "SecuritySummaryParams".into(),
        serde_json::to_value(schema_for!(Empty)).unwrap(),
    ));
    {
        let mut schema = serde_json::to_value(schema_for!(SecuritySummary)).unwrap();
        schema.as_object_mut().unwrap().insert(
            "x-method".into(),
            Value::String("hoh/security/summary".into()),
        );
        schemas.push(("SecuritySummaryResult".into(), schema));
    }
    schemas.push((
        "ApprovalRequestParams".into(),
        serde_json::to_value(schema_for!(ApprovalRequestParams)).unwrap(),
    ));
    {
        let mut schema = serde_json::to_value(schema_for!(ApprovalRequestResult)).unwrap();
        schema
            .as_object_mut()
            .unwrap()
            .insert("x-method".into(), Value::String("approval/request".into()));
        schemas.push(("ApprovalRequestResult".into(), schema));
    }
    schemas.push((
        "UserQuestionAskParams".into(),
        serde_json::to_value(schema_for!(SdkUserQuestionRequest)).unwrap(),
    ));
    {
        let mut schema = serde_json::to_value(schema_for!(SdkUserQuestionResult)).unwrap();
        schema
            .as_object_mut()
            .unwrap()
            .insert("x-method".into(), Value::String("user-question/ask".into()));
        schemas.push(("UserQuestionAskResult".into(), schema));
    }
    schemas.push((
        "ModelKeySetParams".into(),
        serde_json::to_value(schema_for!(ModelKeySetParams)).unwrap(),
    ));
    schemas.push((
        "SettingsRulesSetParams".into(),
        serde_json::to_value(schema_for!(SettingsRulesSetParams)).unwrap(),
    ));
    for (name, method) in [
        ("ModelKeySetResult", "hoh/model/key/set"),
        ("SettingsRulesSetResult", "hoh/settings/rules/set"),
    ] {
        let mut schema = serde_json::to_value(schema_for!(Empty)).unwrap();
        schema
            .as_object_mut()
            .unwrap()
            .insert("x-method".into(), Value::String(method.into()));
        schemas.push((name.into(), schema));
    }
    for (name, method, params, result) in [
        (
            "ExecutorDiscover",
            "hoh/executor/discover",
            schema_for!(ExecutorParams),
            schema_for!(ExecutorDiscoverResult),
        ),
        (
            "ExecutorSelect",
            "hoh/executor/select",
            schema_for!(ExecutorSelectParams),
            schema_for!(ExecutorSelection),
        ),
        (
            "ExecutorAccessGet",
            "hoh/executor/access/get",
            schema_for!(ExecutorAccessGetParams),
            schema_for!(ExecutorAccessGetResult),
        ),
        (
            "ExecutorAccessSet",
            "hoh/executor/access/set",
            schema_for!(ExecutorAccessSetParams),
            schema_for!(ExecutorAccessSetResult),
        ),
    ] {
        schemas.push((
            format!("{name}Params"),
            serde_json::to_value(params).unwrap(),
        ));
        let mut schema = serde_json::to_value(result).unwrap();
        schema
            .as_object_mut()
            .unwrap()
            .insert("x-method".into(), Value::String(method.into()));
        schemas.push((format!("{name}Result"), schema));
    }
    schemas
}

#[derive(Deserialize)]
#[serde(remote = "Request", tag = "method", content = "params")]
#[allow(dead_code)]
enum RequestDecode {
    #[serde(rename = "initialize")]
    Initialize(InitializeParams),
    #[serde(rename = "session/prompt")]
    SessionPrompt(SessionPromptParams),
    #[serde(rename = "session/query")]
    SessionQuery(SessionQueryParams),
    #[serde(rename = "session/resume")]
    SessionResume(SessionResumeParams),
    #[serde(rename = "session/wait")]
    SessionWait(SessionWaitParams),
    #[serde(rename = "session/working-directory/get")]
    WorkingDirectoryGet(SessionWorkingDirectoryParams),
    #[serde(rename = "session/working-directory/set")]
    WorkingDirectorySet(SessionWorkingDirectorySetParams),
    #[serde(rename = "shutdown")]
    Shutdown,
    #[serde(rename = "session/list")]
    SessionList(SessionListParams),
    #[serde(rename = "session/events")]
    SessionEvents(SessionEventsParams),
    #[serde(rename = "session/cancel")]
    SessionCancel(SessionResumeParams),
    #[serde(rename = "hoh/employee/list")]
    EmployeeList(Empty),
    #[serde(rename = "hoh/employee/get")]
    EmployeeGet(IdParams),
    #[serde(rename = "hoh/employee/create")]
    EmployeeCreate(EmployeeCreateParams),
    #[serde(rename = "hoh/employee/update")]
    EmployeeUpdate(EmployeeUpdateParams),
    #[serde(rename = "hoh/employee/delete")]
    EmployeeDelete(IdParams),
    #[serde(rename = "hoh/employee/memory/list")]
    EmployeeMemoryList(IdParams),
    #[serde(rename = "hoh/employee/memory/put")]
    EmployeeMemoryPut(MemoryPutParams),
    #[serde(rename = "hoh/employee/memory/delete")]
    EmployeeMemoryDelete(MemoryDeleteParams),
    #[serde(rename = "hoh/employee/open")]
    EmployeeOpen(IdParams),
    #[serde(rename = "hoh/task/draft/start")]
    TaskDraftStart(TaskDraftStartParams),
    #[serde(rename = "hoh/task/draft/decide")]
    TaskDraftDecide(TaskDraftDecideParams),
    #[serde(rename = "hoh/task/list")]
    TaskList(Empty),
    #[serde(rename = "hoh/task/get")]
    TaskGet(IdParams),
    #[serde(rename = "hoh/task/versions")]
    TaskVersions(IdParams),
    #[serde(rename = "hoh/task/rollback")]
    TaskRollback(TaskRollbackParams),
    #[serde(rename = "hoh/task/delete")]
    TaskDelete(IdParams),
    #[serde(rename = "hoh/task/run")]
    TaskRun(TaskRunParams),
    #[serde(rename = "hoh/task/knowledge/get")]
    TaskKnowledgeGet(TaskIdParams),
    #[serde(rename = "hoh/task/knowledge/edit")]
    TaskKnowledgeEdit(TaskKnowledgeEditParams),
    #[serde(rename = "hoh/button/list")]
    ButtonList(Empty),
    #[serde(rename = "hoh/button/create")]
    ButtonCreate(ButtonCreateParams),
    #[serde(rename = "hoh/button/update")]
    ButtonUpdate(ButtonUpdateParams),
    #[serde(rename = "hoh/button/delete")]
    ButtonDelete(IdParams),
    #[serde(rename = "hoh/schedule/list")]
    ScheduleList(Empty),
    #[serde(rename = "hoh/schedule/create")]
    ScheduleCreate(ScheduleCreateParams),
    #[serde(rename = "hoh/schedule/update")]
    ScheduleUpdate(ScheduleUpdateParams),
    #[serde(rename = "hoh/schedule/delete")]
    ScheduleDelete(IdParams),
    #[serde(rename = "hoh/schedule/run-now")]
    ScheduleRunNow(IdParams),
    #[serde(rename = "hoh/schedule/history")]
    ScheduleHistory(IdParams),
    #[serde(rename = "hoh/artifact/list")]
    ArtifactList(ArtifactListParams),
    #[serde(rename = "hoh/artifact/read")]
    ArtifactRead(ArtifactReadParams),
    #[serde(rename = "hoh/artifact/render")]
    ArtifactRender(ArtifactRenderParams),
    #[serde(rename = "hoh/model/key/set")]
    ModelKeySet(ModelKeySetParams),
    #[serde(rename = "hoh/settings/rules/set")]
    SettingsRulesSet(SettingsRulesSetParams),
    #[serde(rename = "hoh/model/probe")]
    ModelProbe(ModelProbeParams),
    #[serde(rename = "hoh/settings/get")]
    SettingsGet(Empty),
    #[serde(rename = "hoh/settings/set")]
    SettingsSet(SettingsSetParams),
    #[serde(rename = "hoh/security/summary")]
    SecuritySummary(Empty),
    #[serde(rename = "hoh/executor/discover")]
    ExecutorDiscover(ExecutorParams),
    #[serde(rename = "hoh/executor/select")]
    ExecutorSelect(ExecutorSelectParams),
    #[serde(rename = "hoh/executor/access/get")]
    ExecutorAccessGet(ExecutorAccessGetParams),
    #[serde(rename = "hoh/executor/access/set")]
    ExecutorAccessSet(ExecutorAccessSetParams),
    #[serde(rename = "approval/request")]
    ApprovalRequest(ApprovalRequestParams),
    #[serde(rename = "user-question/ask")]
    UserQuestionAsk(SdkUserQuestionRequest),
}
impl<'de> Deserialize<'de> for Request {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let mut value = Value::deserialize(d)?;
        let method = value
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        if method == "shutdown" {
            if let Some(object) = value.as_object_mut() {
                object.remove("params");
            }
        } else if matches!(
            method.as_str(),
            "session/list"
                | "hoh/employee/list"
                | "hoh/task/list"
                | "hoh/button/list"
                | "hoh/schedule/list"
                | "hoh/settings/get"
                | "hoh/settings/set"
                | "hoh/security/summary"
        ) && value.get("params").is_none()
        {
            value["params"] = serde_json::json!({});
        }
        RequestDecode::deserialize(value).map_err(serde::de::Error::custom)
    }
}
// The exported filter and engine predicate are generated from one method table.
macro_rules! privileged_methods {
    ($($method:literal => $predicate:expr),* $(,)?) => {
        pub const PRIVILEGED_METHODS: &[&str] = &[$($method),*];
        pub fn requires_host_approval(method:&str,params:&Value)->bool {
            match method { $($method => ($predicate)(params)),*, _=>false }
        }
    };
}
privileged_methods! {
    "hoh/employee/create" => |_:&Value| true,
    "hoh/employee/update" => |p:&Value| p.get("patch").is_some_and(|patch|patch.get("permissions").is_some() || patch.get("taskIds").is_some()),
    "hoh/task/draft/decide" => |p:&Value| p["decision"]=="yes",
    "hoh/task/rollback" => |_:&Value| true,
    "hoh/settings/set" => |p:&Value| p.get("security").is_some() || p.get("model").is_some(),
    "session/working-directory/set" => |_:&Value| true,
    "hoh/model/probe" => |p:&Value| p.get("baseUrl").is_some(),
    "hoh/model/key/set" => |_:&Value| true,
    "hoh/settings/rules/set" => |_:&Value| true,
    "hoh/executor/access/set" => |_:&Value| true,
    "hoh/executor/select" => |_:&Value| true,
}
pub fn validate_params_host(method: &str, value: &Value) -> Result<(), String> {
    if method == "approval/request" {
        let request = serde_json::from_value::<HostApprovalRequest>(value.clone())
            .map_err(|e| e.to_string())?;
        crate::approval::validate_host_request(&request)
    } else {
        validate_params(method, value)
    }
}
pub fn validate_result_host(method: &str, value: &Value) -> Result<(), String> {
    if method == "approval/request" {
        let decision = serde_json::from_value::<HostApprovalDecision>(value.clone())
            .map_err(|e| e.to_string())?;
        crate::approval::validate_host_decision(&decision)
    } else {
        validate_result(method, value)
    }
}
