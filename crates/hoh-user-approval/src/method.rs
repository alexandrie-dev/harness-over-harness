//! Human text and signed action of a privileged JSON-RPC method (`hoh_protocol::PRIVILEGED_METHODS`)
//! asked through the host approval path. Texts are ported from `hoh-window-stub::engine::approval_text`:
//! they are built from the typed structure of the call, never from model prose.
use hoh_protocol::{HostApprovalRequest, Risk};
use serde_json::{Value, json};

/// Session id of an approval for a method that is not bound to a session.
pub(crate) const HOST_SESSION: &str = "host";

#[cfg(test)]
mod trust_tests {
    use super::*;

    #[test]
    fn folder_trust_confirmation_names_both_executors() {
        for executor in ["claude", "codex"] {
            for trust in [true, false] {
                let text = method_human_text(
                    "hoh/executor/trust/set",
                    &json!({
                        "executor": executor, "cwd": "/work/project", "trust": trust
                    }),
                )
                .unwrap();
                assert!(
                    text.contains("«/work/project» (Claude and Codex)"),
                    "{text}"
                );
                assert!(text.contains("project settings"), "{text}");
            }
        }
    }
}

/// Text shown to the user for one privileged method call; `Err` refuses malformed calls.
pub fn method_human_text(method: &str, params: &Value) -> Result<String, String> {
    match method {
        "hoh/executor/trust/set" => {
            let request: hoh_protocol::ExecutorTrustSetParams =
                serde_json::from_value(params.clone()).map_err(|_| "invalid folder trust")?;
            Ok(if request.trust {
                format!(
                    "Trust project settings in {} (Claude and Codex)? These settings can run programs.",
                    quoted(&request.cwd)
                )
            } else {
                format!(
                    "Revoke trust for project settings in {} (Claude and Codex)?",
                    quoted(&request.cwd)
                )
            })
        }
        "session/working-directory/set" => {
            let request: hoh_protocol::SessionWorkingDirectorySetParams =
                serde_json::from_value(params.clone()).map_err(|_| "invalid working directory")?;
            Ok(format!("Allow work in folder {}?", quoted(&request.path)))
        }
        "session/working-directory/rebind" => {
            let request: hoh_protocol::SessionWorkingDirectoryRebindParams =
                serde_json::from_value(params.clone()).map_err(|_| "invalid working directory")?;
            Ok(format!(
                "The folder for this chat has changed: {}. Rebind this chat to this folder?",
                quoted(&request.path)
            ))
        }
        "hoh/settings/set" => {
            let request: hoh_protocol::SettingsSetParams =
                serde_json::from_value(params.clone()).map_err(|_| "invalid settings")?;
            let mut changes = Vec::new();
            if let Some(model) = request.model {
                changes.push(format!(
                    "model — {}, provider — {}",
                    quoted(&model.model),
                    quoted(&model.base_url)
                ));
            }
            if let Some(security) = request.security {
                changes.push(format!("permissions: {}", permissions_text(&security)));
            }
            if let Some(view) = request.view {
                changes.push(format!(
                    "theme — {}, text size — {}",
                    theme_text(&view.theme),
                    view.font_size
                ));
            }
            if changes.is_empty() {
                return Err("settings update has no changes".into());
            }
            Ok(format!("Change settings: {}?", changes.join("; ")))
        }
        "hoh/employee/create" => {
            let request: hoh_protocol::EmployeeCreateParams =
                serde_json::from_value(params.clone()).map_err(|_| "invalid employee draft")?;
            Ok(format!(
                "Create employee {} with role {}; rules — {}; assigned tasks — {}; permissions: {}?",
                quoted(&request.draft.name),
                quoted(&request.draft.role),
                quoted_list(&request.draft.rules),
                quoted_list(&request.draft.task_ids),
                permissions_text(&request.draft.permissions)
            ))
        }
        "hoh/employee/update" => {
            let request: hoh_protocol::EmployeeUpdateParams =
                serde_json::from_value(params.clone()).map_err(|_| "invalid employee update")?;
            let mut changes = Vec::new();
            if let Some(name) = request.patch.name {
                changes.push(format!("name — {}", quoted(&name)));
            }
            if let Some(role) = request.patch.role {
                changes.push(format!("role — {}", quoted(&role)));
            }
            if let Some(rules) = request.patch.rules {
                changes.push(format!("rules — {}", quoted_list(&rules)));
            }
            if let Some(task_ids) = request.patch.task_ids {
                changes.push(format!("assigned tasks — {}", quoted_list(&task_ids)));
            }
            if let Some(permissions) = request.patch.permissions {
                changes.push(format!("permissions: {}", permissions_text(&permissions)));
            }
            if changes.is_empty() {
                return Err("privileged employee update has no rights change".into());
            }
            Ok(format!(
                "Change employee {}: {}?",
                quoted(&request.id),
                changes.join("; ")
            ))
        }
        "hoh/settings/rules/set" => {
            let rules: hoh_protocol::SettingsRulesSetParams =
                serde_json::from_value(params.clone()).map_err(|_| "invalid setup rules")?;
            let internet = match rules.internet {
                hoh_protocol::SetupInternetMode::Ask => "with permission requested",
                hoh_protocol::SetupInternetMode::Public => "to public websites",
            };
            Ok(format!(
                "Allow work in folder {} and internet access {internet}?",
                quoted(&rules.folder)
            ))
        }
        "hoh/model/key/set" => {
            let address = params["baseUrl"]
                .as_str()
                .filter(|value| !value.is_empty())
                .ok_or("model provider address missing")?;
            Ok(format!(
                "Save the API key for provider {}?",
                quoted(address)
            ))
        }
        "hoh/model/probe" => {
            let probe: hoh_protocol::ModelProbeParams =
                serde_json::from_value(params.clone()).map_err(|_| "invalid model probe")?;
            Ok(format!(
                "Allow testing model {} at provider {}?",
                quoted(&probe.model),
                quoted(&probe.base_url)
            ))
        }
        // HOH-18 (review 232500, item 1): the user's own choice of the executor's access and of
        // who answers new chats; the text names the executor and what the choice allows.
        "hoh/executor/access/set" => {
            let request: hoh_protocol::ExecutorAccessSetParams =
                serde_json::from_value(params.clone()).map_err(|_| "invalid executor access")?;
            let name = executor_name(&request.executor);
            let runtime = match (&request.mode, request.runtime_mode.as_ref()) {
                (
                    hoh_protocol::ExecutorAccessMode::Full,
                    None | Some(hoh_protocol::ExecutorRuntimeMode::Full),
                ) => hoh_protocol::ExecutorRuntimeMode::Full,
                (
                    hoh_protocol::ExecutorAccessMode::Confirm,
                    None | Some(hoh_protocol::ExecutorRuntimeMode::Confirm),
                ) => hoh_protocol::ExecutorRuntimeMode::Confirm,
                (
                    hoh_protocol::ExecutorAccessMode::Confirm,
                    Some(hoh_protocol::ExecutorRuntimeMode::AutoAcceptEdits),
                ) => hoh_protocol::ExecutorRuntimeMode::AutoAcceptEdits,
                (
                    hoh_protocol::ExecutorAccessMode::Confirm,
                    Some(hoh_protocol::ExecutorRuntimeMode::Auto),
                ) => hoh_protocol::ExecutorRuntimeMode::Auto,
                _ => return Err("executor access mode does not match runtimeMode".into()),
            };
            Ok(match runtime {
                hoh_protocol::ExecutorRuntimeMode::Full => format!(
                    "Enable Full access for {name}? The assistant can delete and change files in any folder you can access, run programs, and access the internet without asking. These actions cannot be undone."
                ),
                hoh_protocol::ExecutorRuntimeMode::Confirm => {
                    format!("Return {name} to Supervised mode?")
                }
                hoh_protocol::ExecutorRuntimeMode::AutoAcceptEdits => {
                    if request.executor == "claude" {
                        format!(
                            "Enable Accept edits for {name}? The assistant edits files in the conversation folder without asking; commands and network access require your approval."
                        )
                    } else {
                        format!(
                            "Enable Accept edits for {name}? The assistant automatically accepts file changes in the working folder but asks before other actions."
                        )
                    }
                }
                hoh_protocol::ExecutorRuntimeMode::Auto => {
                    if request.executor == "claude" {
                        format!(
                            "Enable Auto for {name}? The assistant decides which actions are safe and performs them without asking; other actions require your approval."
                        )
                    } else {
                        format!(
                            "Enable Auto for {name}? The assistant automatically accepts file changes in the working folder and reviews other permission requests automatically."
                        )
                    }
                }
            })
        }
        "hoh/executor/select" => {
            let request: hoh_protocol::ExecutorSelectParams =
                serde_json::from_value(params.clone()).map_err(|_| "invalid executor choice")?;
            Ok(match request.executor {
                Some(executor) => format!(
                    "Use {} (model {}) to respond?",
                    executor_name(&executor),
                    request.model
                ),
                None => "Use the model on this computer to respond?".to_owned(),
            })
        }
        // HOH-41: the person adds a folder as a project; `/`, `$` and `@` then answer for it.
        "hoh/workspace/add" => {
            let request: hoh_protocol::WorkspaceAddParams =
                serde_json::from_value(params.clone()).map_err(|_| "invalid project folder")?;
            Ok(format!("Add folder {} as a project?", quoted(&request.cwd)))
        }
        _ => Ok(format!("Allow action {method}?")),
    }
}

/// An executor's name as a word in a sentence: `claude` → `Claude`.
fn executor_name(executor: &str) -> String {
    let mut chars = executor.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

fn quoted(value: &str) -> String {
    format!("«{value}»")
}

fn quoted_list(values: &[String]) -> String {
    if values.is_empty() {
        "none".into()
    } else {
        values
            .iter()
            .map(|value| quoted(value))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

fn permissions_text(permissions: &hoh_protocol::Permissions) -> String {
    let internet = match &permissions.internet {
        hoh_protocol::InternetMode::Ask => "with permission requested",
        hoh_protocol::InternetMode::Public => "public websites",
        hoh_protocol::InternetMode::Deny => "denied",
    };
    format!(
        "internet — {internet}; websites — {}; folders — {}; file changes — {}; running programs — {}; program network access — {}; actions on your behalf — {}",
        quoted_list(&permissions.domains),
        quoted_list(&permissions.folders),
        allowed(permissions.write_files),
        allowed(permissions.programs),
        quoted_list(&permissions.program_network),
        allowed(permissions.act_as_user)
    )
}

fn allowed(value: bool) -> &'static str {
    if value { "allowed" } else { "denied" }
}

fn theme_text(theme: &hoh_protocol::Theme) -> &'static str {
    match theme {
        hoh_protocol::Theme::Light => "light",
        hoh_protocol::Theme::Dark => "dark",
        hoh_protocol::Theme::System => "system",
    }
}

/// The action the user signs. The model credential never leaves the engine: only its hash does.
pub(crate) fn signed_action(method: &str, params: &Value) -> Result<Value, String> {
    let params = if method == "hoh/model/key/set" {
        let typed: hoh_protocol::ModelKeySetParams =
            serde_json::from_value(params.clone()).map_err(|_| "invalid model credential")?;
        if typed.api_key.expose().is_empty() || typed.api_key.expose().len() > 8192 {
            return Err("model credential must contain 1..8192 bytes".into());
        }
        json!({
            "keyHash": hoh_protocol::approval::action_hash(&json!({"apiKey": typed.api_key.expose()})),
            "baseUrl": typed.base_url,
        })
    } else {
        params.clone()
    };
    Ok(json!({"method": method, "params": params}))
}

pub(crate) fn build_method_request(
    method: &str,
    params: &Value,
    approval_id: String,
    expires_at: u64,
) -> Result<HostApprovalRequest, String> {
    let action = signed_action(method, params)?;
    let human_text = method_human_text(method, params)?;
    let session_id = params
        .get("sessionId")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .unwrap_or(HOST_SESSION)
        .to_owned();
    let request = HostApprovalRequest {
        session_id,
        tool_name: method.to_owned(),
        call_id: None,
        reason: None,
        details: None,
        approval_id,
        risk: Risk::High,
        human_text,
        action_hash: hoh_protocol::approval::action_hash(&action),
        expires_at,
        action,
    };
    hoh_protocol::approval::validate_host_request(&request)?;
    Ok(request)
}
