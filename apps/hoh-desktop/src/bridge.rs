//! The only window -> engine boundary. No raw RPC, initialization or shutdown from JS.
use crate::engine::Engine;
use serde_json::Value;
use std::sync::Arc;

pub const WINDOW_METHODS: &[&str] = &[
    "session/prompt",
    "session/query",
    "session/resume",
    "session/wait",
    "session/list",
    "session/events",
    "session/cancel",
    "session/delete",
    "session/working-directory/get",
    "session/working-directory/set",
    "session/working-directory/status",
    "session/working-directory/rebind",
    "hoh/employee/list",
    "hoh/employee/get",
    "hoh/employee/create",
    "hoh/employee/update",
    "hoh/employee/delete",
    "hoh/employee/open",
    "hoh/employee/memory/list",
    "hoh/employee/memory/put",
    "hoh/employee/memory/delete",
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
    "hoh/model/probe",
    "hoh/model/key/set",
    "hoh/plugins/list",
    "hoh/settings/get",
    "hoh/settings/set",
    "hoh/settings/rules/set",
    "hoh/security/summary",
    "hoh/executor/access/get",
    "hoh/executor/access/set",
    "hoh/executor/trust/get",
    "hoh/executor/trust/set",
    "hoh/executor/discover",
    "hoh/executor/limits",
    "hoh/executor/select",
    // HOH-41: `/`, `$` and `@` of the T3 composer. Read-only, and only for a folder the engine
    // knows (the engine checks it); like every method here, only from the main window.
    "hoh/executor/commands",
    "hoh/workspace/entries",
    // The person's "Add project": privileged, confirmed by that action in the main window.
    "hoh/workspace/add",
    "hoh/executor/title/generate",
];
pub fn check_main(label: &str, method: &str, params: &Value) -> Result<(), String> {
    if label != "main" {
        return Err("Команда доступна только главному окну".into());
    }
    if !WINDOW_METHODS.contains(&method) {
        return Err("Метод не разрешён окну".into());
    }
    let invalid = || "Некорректные параметры запроса".to_owned();
    // The engine consumes effort as a window extension before decoding the public SDK request.
    // Validate it here, then check the unchanged SDK fields. Forward the original request.
    if method == "session/prompt"
        && let Some(effort) = params.get("effort")
    {
        if !effort
            .as_str()
            .is_some_and(|effort| !effort.trim().is_empty())
        {
            return Err(invalid());
        }
        let mut sdk_params = params.clone();
        sdk_params
            .as_object_mut()
            .ok_or_else(invalid)?
            .remove("effort");
        return hoh_protocol::validate_params(method, &sdk_params).map_err(|_| invalid());
    }
    hoh_protocol::validate_params(method, params).map_err(|_| invalid())
}
pub fn needs_approval(method: &str, params: &Value) -> bool {
    hoh_protocol::PRIVILEGED_METHODS.contains(&method)
        && hoh_protocol::requires_host_approval(method, params)
}
pub fn request(
    engine: &Arc<Engine>,
    label: &str,
    method: &str,
    params: Value,
) -> Result<Value, String> {
    check_main(label, method, &params)?;
    if needs_approval(method, &params) {
        return Err("Нужно подтверждение в отдельном окне".into());
    }
    engine.request(method, params)
}

/// The Tauri adapter supplies the native preflight. The original RPC is not
/// handed to the transport until that callback has returned successfully.
pub fn request_with_approval(
    engine: &Arc<Engine>,
    label: &str,
    method: &str,
    params: Value,
    approve: impl FnOnce(&Value) -> Result<(), String>,
) -> Result<Value, String> {
    request_with_approval_armed(engine, label, method, params, approve, None)
}

/// Same, and `on_send` is told the id of the outgoing request (only when a confirmation was needed
/// and given), so that confirmation can be bound to that request.
pub fn request_with_approval_armed(
    engine: &Arc<Engine>,
    label: &str,
    method: &str,
    params: Value,
    approve: impl FnOnce(&Value) -> Result<(), String>,
    on_send: Option<crate::engine::OnSend>,
) -> Result<Value, String> {
    check_main(label, method, &params)?;
    let confirmed = needs_approval(method, &params);
    if confirmed {
        approve(&params)?;
    }
    engine.request_with(method, params, if confirmed { on_send } else { None })
}
