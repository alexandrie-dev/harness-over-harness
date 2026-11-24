use hoh_protocol::*;
use serde_json::Value;
pub fn typed_result(method: &str, value: Value) -> Value {
    match method {
        "hoh/model/key/set" | "hoh/settings/rules/set" => {
            serde_json::to_value(serde_json::from_value::<Empty>(value).unwrap()).unwrap()
        }
        "initialize" => {
            serde_json::to_value(serde_json::from_value::<InitializeResult>(value).unwrap())
                .unwrap()
        }
        "hoh/executor/discover" => {
            serde_json::to_value(serde_json::from_value::<ExecutorDiscoverResult>(value).unwrap())
                .unwrap()
        }
        "hoh/executor/select" => {
            serde_json::to_value(serde_json::from_value::<ExecutorSelection>(value).unwrap())
                .unwrap()
        }
        "hoh/executor/access/get" => {
            serde_json::to_value(serde_json::from_value::<ExecutorAccessGetResult>(value).unwrap())
                .unwrap()
        }
        "hoh/executor/access/set" => {
            serde_json::to_value(serde_json::from_value::<ExecutorAccessSetResult>(value).unwrap())
                .unwrap()
        }
        "hoh/executor/commands" => {
            serde_json::to_value(serde_json::from_value::<ExecutorCommandsResult>(value).unwrap())
                .unwrap()
        }
        "hoh/workspace/add" => {
            serde_json::to_value(serde_json::from_value::<WorkspaceAddResult>(value).unwrap())
                .unwrap()
        }
        "hoh/workspace/entries" => {
            serde_json::to_value(serde_json::from_value::<WorkspaceEntriesResult>(value).unwrap())
                .unwrap()
        }
        "hoh/executor/title/generate" => serde_json::to_value(
            serde_json::from_value::<ExecutorTitleGenerateResult>(value).unwrap(),
        )
        .unwrap(),
        "session/prompt" => {
            serde_json::to_value(serde_json::from_value::<SessionPromptResult>(value).unwrap())
                .unwrap()
        }
        "session/query" => {
            serde_json::to_value(serde_json::from_value::<SessionQueryResult>(value).unwrap())
                .unwrap()
        }
        "session/resume" => {
            serde_json::to_value(serde_json::from_value::<Empty>(value).unwrap()).unwrap()
        }
        "session/wait" => {
            serde_json::to_value(serde_json::from_value::<Empty>(value).unwrap()).unwrap()
        }
        "session/working-directory/get" => serde_json::to_value(
            serde_json::from_value::<SessionWorkingDirectoryResult>(value).unwrap(),
        )
        .unwrap(),
        "session/working-directory/set" => serde_json::to_value(
            serde_json::from_value::<SessionWorkingDirectoryResult>(value).unwrap(),
        )
        .unwrap(),
        "shutdown" => {
            serde_json::to_value(serde_json::from_value::<Empty>(value).unwrap()).unwrap()
        }
        "session/list" => {
            serde_json::to_value(serde_json::from_value::<SessionListResult>(value).unwrap())
                .unwrap()
        }
        "session/events" => {
            serde_json::to_value(serde_json::from_value::<SessionEventsResult>(value).unwrap())
                .unwrap()
        }
        "session/cancel" => {
            serde_json::to_value(serde_json::from_value::<Empty>(value).unwrap()).unwrap()
        }
        "hoh/employee/list" => {
            serde_json::to_value(serde_json::from_value::<Vec<Employee>>(value).unwrap()).unwrap()
        }
        "hoh/employee/get" => {
            serde_json::to_value(serde_json::from_value::<Employee>(value).unwrap()).unwrap()
        }
        "hoh/employee/create" => {
            serde_json::to_value(serde_json::from_value::<IdRev>(value).unwrap()).unwrap()
        }
        "hoh/employee/update" => {
            serde_json::to_value(serde_json::from_value::<IdRev>(value).unwrap()).unwrap()
        }
        "hoh/employee/delete" => {
            serde_json::to_value(serde_json::from_value::<IdRev>(value).unwrap()).unwrap()
        }
        "hoh/employee/memory/list" => {
            serde_json::to_value(serde_json::from_value::<Vec<MemoryItem>>(value).unwrap()).unwrap()
        }
        "hoh/employee/memory/put" => {
            serde_json::to_value(serde_json::from_value::<MemoryItem>(value).unwrap()).unwrap()
        }
        "hoh/employee/memory/delete" => {
            serde_json::to_value(serde_json::from_value::<Empty>(value).unwrap()).unwrap()
        }
        "hoh/employee/open" => {
            serde_json::to_value(serde_json::from_value::<SessionIdResult>(value).unwrap()).unwrap()
        }
        "hoh/task/draft/start" => {
            serde_json::to_value(serde_json::from_value::<TaskDraftStartResult>(value).unwrap())
                .unwrap()
        }
        "hoh/task/draft/decide" => {
            serde_json::to_value(serde_json::from_value::<TaskDraftDecideResult>(value).unwrap())
                .unwrap()
        }
        "hoh/task/list" => {
            serde_json::to_value(serde_json::from_value::<Vec<Task>>(value).unwrap()).unwrap()
        }
        "hoh/task/get" => {
            serde_json::to_value(serde_json::from_value::<Task>(value).unwrap()).unwrap()
        }
        "hoh/task/versions" => {
            serde_json::to_value(serde_json::from_value::<Vec<TaskVersion>>(value).unwrap())
                .unwrap()
        }
        "hoh/task/rollback" => {
            serde_json::to_value(serde_json::from_value::<Task>(value).unwrap()).unwrap()
        }
        "hoh/task/delete" => {
            serde_json::to_value(serde_json::from_value::<Empty>(value).unwrap()).unwrap()
        }
        "hoh/task/run" => {
            serde_json::to_value(serde_json::from_value::<TaskRunResult>(value).unwrap()).unwrap()
        }
        "hoh/task/knowledge/get" => {
            serde_json::to_value(serde_json::from_value::<TaskKnowledge>(value).unwrap()).unwrap()
        }
        "hoh/task/knowledge/edit" => {
            serde_json::to_value(serde_json::from_value::<TaskDraftStartResult>(value).unwrap())
                .unwrap()
        }
        "hoh/button/list" => {
            serde_json::to_value(serde_json::from_value::<Vec<Button>>(value).unwrap()).unwrap()
        }
        "hoh/button/create" => {
            serde_json::to_value(serde_json::from_value::<Button>(value).unwrap()).unwrap()
        }
        "hoh/button/update" => {
            serde_json::to_value(serde_json::from_value::<Button>(value).unwrap()).unwrap()
        }
        "hoh/button/delete" => {
            serde_json::to_value(serde_json::from_value::<Empty>(value).unwrap()).unwrap()
        }
        "hoh/schedule/list" => {
            serde_json::to_value(serde_json::from_value::<Vec<Schedule>>(value).unwrap()).unwrap()
        }
        "hoh/schedule/create" => {
            serde_json::to_value(serde_json::from_value::<Schedule>(value).unwrap()).unwrap()
        }
        "hoh/schedule/update" => {
            serde_json::to_value(serde_json::from_value::<Schedule>(value).unwrap()).unwrap()
        }
        "hoh/schedule/delete" => {
            serde_json::to_value(serde_json::from_value::<Empty>(value).unwrap()).unwrap()
        }
        "hoh/schedule/run-now" => {
            serde_json::to_value(serde_json::from_value::<TaskRunResult>(value).unwrap()).unwrap()
        }
        "hoh/schedule/history" => {
            serde_json::to_value(serde_json::from_value::<Vec<ScheduleRun>>(value).unwrap())
                .unwrap()
        }
        "hoh/artifact/list" => {
            serde_json::to_value(serde_json::from_value::<Vec<Artifact>>(value).unwrap()).unwrap()
        }
        "hoh/artifact/read" => {
            serde_json::to_value(serde_json::from_value::<ArtifactReadResult>(value).unwrap())
                .unwrap()
        }
        "hoh/artifact/render" => {
            serde_json::to_value(serde_json::from_value::<ArtifactRenderResult>(value).unwrap())
                .unwrap()
        }
        "hoh/model/probe" => {
            serde_json::to_value(serde_json::from_value::<ModelProbeResult>(value).unwrap())
                .unwrap()
        }
        "hoh/settings/get" => {
            serde_json::to_value(serde_json::from_value::<Settings>(value).unwrap()).unwrap()
        }
        "hoh/settings/set" => {
            serde_json::to_value(serde_json::from_value::<Settings>(value).unwrap()).unwrap()
        }
        "hoh/security/summary" => {
            serde_json::to_value(serde_json::from_value::<SecuritySummary>(value).unwrap()).unwrap()
        }
        "approval/request" => {
            serde_json::to_value(serde_json::from_value::<ApprovalRequestResult>(value).unwrap())
                .unwrap()
        }
        "user-question/ask" => {
            serde_json::to_value(serde_json::from_value::<SdkUserQuestionResult>(value).unwrap())
                .unwrap()
        }
        _ => panic!("unknown method"),
    }
}
