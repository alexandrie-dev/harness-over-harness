//! Native-only approval state. JavaScript sees a redacted view and never the key or MAC.
use hoh_protocol::{ApprovalOutcome, HostApprovalDecision, HostApprovalRequest, Risk};
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

pub const ALLOW_DELAY: Duration = Duration::from_secs(1);
const MAX_LIFETIME_MS: u64 = 300_000;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalView {
    pub approval_id: String,
    pub risk: Risk,
    pub human_text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub expires_at: u64,
}

pub struct OpenApproval {
    pub label: String,
    pub view: ApprovalView,
}

pub enum Resolution {
    Preflight {
        reply: mpsc::SyncSender<bool>,
        allowed: bool,
    },
    Engine {
        generation: u64,
        id: Value,
        decision: Result<HostApprovalDecision, String>,
    },
}

pub struct DecisionOutcome {
    pub resolution: Resolution,
    pub command_result: Result<(), String>,
}

enum Destination {
    Preflight(mpsc::SyncSender<bool>),
    Engine { generation: u64, id: Value },
}

struct Pending {
    label: String,
    request: HostApprovalRequest,
    shown_at: Option<Instant>,
    destination: Destination,
}

pub struct Broker {
    key: [u8; 32],
    next: AtomicU64,
    pending: Mutex<Option<Pending>>,
}

impl Broker {
    pub fn new(key: [u8; 32]) -> Result<Self, String> {
        hoh_protocol::approval::key_from_hex(&hoh_protocol::approval::hex(&key))?;
        Ok(Self {
            key,
            next: AtomicU64::new(0),
            pending: Mutex::new(None),
        })
    }

    pub fn begin_preflight(
        &self,
        method: &str,
        params: &Value,
        now: u64,
    ) -> Result<(OpenApproval, mpsc::Receiver<bool>), String> {
        if !hoh_protocol::requires_host_approval(method, params) {
            return Err("Действие не требует подтверждения".into());
        }
        let sequence = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        let action = redacted_action(method, params)?;
        let request = HostApprovalRequest {
            session_id: "native".into(),
            tool_name: method.into(),
            call_id: None,
            reason: None,
            approval_id: format!("native-preflight-{sequence}"),
            risk: Risk::High,
            human_text: preflight_text(method, params)?,
            action_hash: hoh_protocol::approval::action_hash(&action),
            expires_at: now.saturating_add(MAX_LIFETIME_MS),
            action,
        };
        let (reply, receiver) = mpsc::sync_channel(1);
        let open = self.insert(request, Destination::Preflight(reply))?;
        Ok((open, receiver))
    }

    pub fn begin_engine(
        &self,
        generation: u64,
        id: Value,
        method: &str,
        params: Value,
        now: u64,
    ) -> Result<OpenApproval, String> {
        if method != "approval/request" {
            return Err("Обратный метод не поддерживается".into());
        }
        hoh_protocol::validate_params_host(method, &params)
            .map_err(|_| "Движок прислал некорректное подтверждение".to_owned())?;
        let request: HostApprovalRequest = serde_json::from_value(params)
            .map_err(|_| "Движок прислал некорректное подтверждение".to_owned())?;
        validate_time(&request, now)?;
        self.insert(request, Destination::Engine { generation, id })
    }

    fn insert(
        &self,
        request: HostApprovalRequest,
        destination: Destination,
    ) -> Result<OpenApproval, String> {
        hoh_protocol::approval::validate_host_request(&request)
            .map_err(|_| "Некорректное подтверждение".to_owned())?;
        let sequence = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        let label = format!("approval-{sequence}");
        let view = ApprovalView {
            approval_id: request.approval_id.clone(),
            risk: request.risk.clone(),
            human_text: request.human_text.clone(),
            reason: request.reason.clone(),
            expires_at: request.expires_at,
        };
        let mut slot = self
            .pending
            .lock()
            .map_err(|_| "Подтверждение недоступно")?;
        if slot.is_some() {
            return Err("Другое подтверждение уже открыто".into());
        }
        *slot = Some(Pending {
            label: label.clone(),
            request,
            shown_at: None,
            destination,
        });
        Ok(OpenApproval { label, view })
    }

    pub fn current(&self, label: &str, now: u64, instant: Instant) -> Result<ApprovalView, String> {
        let mut slot = self
            .pending
            .lock()
            .map_err(|_| "Подтверждение недоступно")?;
        let pending = slot.as_mut().ok_or("Нет ожидающего подтверждения")?;
        if pending.label != label {
            return Err("Это окно не может открыть подтверждение".into());
        }
        validate_time(&pending.request, now)?;
        pending.shown_at.get_or_insert(instant);
        Ok(ApprovalView {
            approval_id: pending.request.approval_id.clone(),
            risk: pending.request.risk.clone(),
            human_text: pending.request.human_text.clone(),
            reason: pending.request.reason.clone(),
            expires_at: pending.request.expires_at,
        })
    }

    pub fn decide(
        &self,
        label: &str,
        approval_id: &str,
        outcome: ApprovalOutcome,
        now: u64,
        instant: Instant,
    ) -> Result<DecisionOutcome, String> {
        if !matches!(
            outcome,
            ApprovalOutcome::AllowedOnce | ApprovalOutcome::Rejected
        ) {
            return Err("Недопустимый ответ окна".into());
        }
        {
            let slot = self
                .pending
                .lock()
                .map_err(|_| "Подтверждение недоступно")?;
            let pending = slot.as_ref().ok_or("Нет ожидающего подтверждения")?;
            if pending.label != label {
                return Err("Это окно не может ответить на подтверждение".into());
            }
            if pending.request.approval_id != approval_id {
                return Err("Подтверждение уже сменилось".into());
            }
            if outcome == ApprovalOutcome::AllowedOnce
                && pending.shown_at.is_none_or(|shown_at| {
                    instant
                        .checked_duration_since(shown_at)
                        .is_none_or(|elapsed| elapsed < ALLOW_DELAY)
                })
            {
                return Err("Сначала прочитайте подтверждение".into());
            }
        }
        let request = {
            let slot = self
                .pending
                .lock()
                .map_err(|_| "Подтверждение недоступно")?;
            slot.as_ref()
                .map(|pending| pending.request.clone())
                .ok_or("Нет ожидающего подтверждения")?
        };
        let outcome_text = serde_json::to_value(&outcome)
            .expect("approval outcome serializes")
            .as_str()
            .expect("approval outcome is a string")
            .to_owned();
        let decision = HostApprovalDecision {
            approval_id: request.approval_id.clone(),
            outcome,
            mac: hoh_protocol::approval::mac(&self.key, &request, &outcome_text),
        };
        self.submit_signed(label, approval_id, decision, now)
    }

    pub fn submit_signed(
        &self,
        label: &str,
        approval_id: &str,
        decision: HostApprovalDecision,
        now: u64,
    ) -> Result<DecisionOutcome, String> {
        let pending = self.take_exact(label, approval_id)?;
        match hoh_protocol::approval::verify_decision(&self.key, &pending.request, &decision, now) {
            Ok(()) => Ok(DecisionOutcome {
                resolution: resolve(
                    pending.destination,
                    Ok(decision.clone()),
                    decision.outcome == ApprovalOutcome::AllowedOnce,
                ),
                command_result: Ok(()),
            }),
            Err(_) => Ok(DecisionOutcome {
                resolution: resolve(
                    pending.destination,
                    Err("Подтверждение отклонено".into()),
                    false,
                ),
                command_result: Err("Подтверждение недействительно".into()),
            }),
        }
    }

    pub fn reject(&self, label: &str, now: u64) -> Option<Resolution> {
        let pending = self.take_label(label)?;
        let decision = HostApprovalDecision {
            approval_id: pending.request.approval_id.clone(),
            outcome: ApprovalOutcome::Rejected,
            mac: hoh_protocol::approval::mac(&self.key, &pending.request, "rejected"),
        };
        let verified =
            hoh_protocol::approval::verify_decision(&self.key, &pending.request, &decision, now)
                .map(|()| decision)
                .map_err(|_| "Подтверждение истекло".into());
        Some(resolve(pending.destination, verified, false))
    }

    pub fn cancel(&self) -> Option<(String, Resolution)> {
        let pending = self.pending.lock().ok()?.take()?;
        let label = pending.label;
        Some((
            label,
            resolve(
                pending.destination,
                Err("Подтверждение отменено".into()),
                false,
            ),
        ))
    }

    fn take_exact(&self, label: &str, approval_id: &str) -> Result<Pending, String> {
        let mut slot = self
            .pending
            .lock()
            .map_err(|_| "Подтверждение недоступно")?;
        let pending = slot.as_ref().ok_or("Нет ожидающего подтверждения")?;
        if pending.label != label {
            return Err("Это окно не может ответить на подтверждение".into());
        }
        if pending.request.approval_id != approval_id {
            return Err("Подтверждение уже сменилось".into());
        }
        slot.take()
            .ok_or_else(|| "Нет ожидающего подтверждения".into())
    }

    fn take_label(&self, label: &str) -> Option<Pending> {
        let mut slot = self.pending.lock().ok()?;
        if slot.as_ref()?.label != label {
            return None;
        }
        slot.take()
    }
}

fn resolve(
    destination: Destination,
    decision: Result<HostApprovalDecision, String>,
    allowed: bool,
) -> Resolution {
    match destination {
        Destination::Preflight(reply) => Resolution::Preflight { reply, allowed },
        Destination::Engine { generation, id } => Resolution::Engine {
            generation,
            id,
            decision,
        },
    }
}

fn validate_time(request: &HostApprovalRequest, now: u64) -> Result<(), String> {
    if now >= request.expires_at {
        return Err("Подтверждение истекло".into());
    }
    if request.expires_at - now > MAX_LIFETIME_MS {
        return Err("Срок подтверждения слишком велик".into());
    }
    Ok(())
}

fn redacted_action(method: &str, params: &Value) -> Result<Value, String> {
    let params = if method == "hoh/model/key/set" {
        let api_key = params.get("apiKey").ok_or("Нет ключа модели")?;
        let base_url = params
            .get("baseUrl")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or("Нет адреса модели")?;
        json!({
            "keyHash": hoh_protocol::approval::action_hash(&json!({"apiKey": api_key})),
            "baseUrl": base_url,
        })
    } else {
        params.clone()
    };
    Ok(json!({"method": method, "params": params}))
}

fn preflight_text(method: &str, params: &Value) -> Result<String, String> {
    match method {
        "session/working-directory/set" => {
            let request: hoh_protocol::SessionWorkingDirectorySetParams =
                serde_json::from_value(params.clone()).map_err(|_| "Некорректная рабочая папка")?;
            Ok(format!(
                "Разрешить работу в папке {}?",
                quoted(&request.path)
            ))
        }
        "hoh/settings/set" => {
            let request: hoh_protocol::SettingsSetParams =
                serde_json::from_value(params.clone()).map_err(|_| "Некорректные настройки")?;
            let mut changes = Vec::new();
            if let Some(model) = request.model {
                changes.push(format!(
                    "модель — {}, провайдер — {}",
                    quoted(&model.model),
                    quoted(&model.base_url)
                ));
            }
            if let Some(security) = request.security {
                changes.push(format!("права: {}", permissions_text(&security)));
            }
            if let Some(view) = request.view {
                changes.push(format!(
                    "вид — {}, размер текста — {}",
                    theme_text(&view.theme),
                    view.font_size
                ));
            }
            if changes.is_empty() {
                return Err("Настройки не содержат изменений".into());
            }
            Ok(format!("Изменить настройки: {}?", changes.join("; ")))
        }
        "hoh/employee/create" => {
            let request: hoh_protocol::EmployeeCreateParams =
                serde_json::from_value(params.clone())
                    .map_err(|_| "Некорректный новый сотрудник")?;
            Ok(format!(
                "Создать сотрудника {} с ролью {}; правила — {}; назначенные задачи — {}; права: {}?",
                quoted(&request.draft.name),
                quoted(&request.draft.role),
                quoted_list(&request.draft.rules),
                quoted_list(&request.draft.task_ids),
                permissions_text(&request.draft.permissions)
            ))
        }
        "hoh/employee/update" => {
            let request: hoh_protocol::EmployeeUpdateParams =
                serde_json::from_value(params.clone())
                    .map_err(|_| "Некорректное изменение сотрудника")?;
            let mut changes = Vec::new();
            if let Some(name) = request.patch.name {
                changes.push(format!("имя — {}", quoted(&name)));
            }
            if let Some(role) = request.patch.role {
                changes.push(format!("роль — {}", quoted(&role)));
            }
            if let Some(rules) = request.patch.rules {
                changes.push(format!("правила — {}", quoted_list(&rules)));
            }
            if let Some(task_ids) = request.patch.task_ids {
                changes.push(format!("назначенные задачи — {}", quoted_list(&task_ids)));
            }
            if let Some(permissions) = request.patch.permissions {
                changes.push(format!("права: {}", permissions_text(&permissions)));
            }
            if changes.is_empty() {
                return Err("Изменение сотрудника не содержит новых прав или задач".into());
            }
            Ok(format!(
                "Изменить сотрудника {}: {}?",
                quoted(&request.id),
                changes.join("; ")
            ))
        }
        "hoh/task/draft/decide" => {
            let request: hoh_protocol::TaskDraftDecideParams =
                serde_json::from_value(params.clone())
                    .map_err(|_| "Некорректное решение по черновику")?;
            Ok(format!(
                "Создать задачу из черновика {}?",
                quoted(&request.draft_id)
            ))
        }
        "hoh/task/rollback" => {
            let request: hoh_protocol::TaskRollbackParams =
                serde_json::from_value(params.clone()).map_err(|_| "Некорректный откат задачи")?;
            Ok(format!(
                "Откатить задачу {} к версии {}?",
                quoted(&request.id),
                request.version
            ))
        }
        "hoh/settings/rules/set" => {
            let rules: hoh_protocol::SettingsRulesSetParams =
                serde_json::from_value(params.clone())
                    .map_err(|_| "Некорректные правила доступа")?;
            let internet = match rules.internet {
                hoh_protocol::SetupInternetMode::Ask => "с запросом разрешения",
                hoh_protocol::SetupInternetMode::Public => "к общедоступным сайтам",
            };
            Ok(format!(
                "Разрешить работу в папке {} и доступ в интернет {internet}?",
                quoted(&rules.folder)
            ))
        }
        "hoh/model/key/set" => {
            let address = params["baseUrl"]
                .as_str()
                .filter(|value| !value.is_empty())
                .ok_or("Нет адреса провайдера модели")?;
            Ok(format!(
                "Сохранить API-ключ для провайдера {}?",
                quoted(address)
            ))
        }
        "hoh/model/probe" => {
            let probe: hoh_protocol::ModelProbeParams = serde_json::from_value(params.clone())
                .map_err(|_| "Некорректная проверка модели")?;
            Ok(format!(
                "Разрешить проверку модели {} у провайдера {}?",
                quoted(&probe.model),
                quoted(&probe.base_url)
            ))
        }
        _ => Err("Нет безопасного описания привилегированного действия".into()),
    }
}

fn quoted(value: &str) -> String {
    format!("«{value}»")
}

fn quoted_list(values: &[String]) -> String {
    if values.is_empty() {
        "нет".into()
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
        hoh_protocol::InternetMode::Ask => "с запросом разрешения",
        hoh_protocol::InternetMode::Public => "общедоступные сайты",
        hoh_protocol::InternetMode::Deny => "запрещён",
    };
    format!(
        "интернет — {internet}; сайты — {}; папки — {}; изменение файлов — {}; запуск программ — {}; сеть для программ — {}; действия от вашего имени — {}",
        quoted_list(&permissions.domains),
        quoted_list(&permissions.folders),
        allowed(permissions.write_files),
        allowed(permissions.programs),
        quoted_list(&permissions.program_network),
        allowed(permissions.act_as_user)
    )
}

fn allowed(value: bool) -> &'static str {
    if value {
        "разрешено"
    } else {
        "запрещено"
    }
}

fn theme_text(theme: &hoh_protocol::Theme) -> &'static str {
    match theme {
        hoh_protocol::Theme::Light => "светлая тема",
        hoh_protocol::Theme::Dark => "тёмная тема",
        hoh_protocol::Theme::System => "как в системе",
    }
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(id: &str, now: u64) -> Value {
        let action = json!({"method":"hoh/model/probe","params":{"baseUrl":"https://example.test","model":"m"}});
        json!({
            "sessionId":"s", "toolName":"hoh/model/probe", "approvalId":id,
            "risk":"high", "humanText":"Разрешить проверку модели?",
            "actionHash":hoh_protocol::approval::action_hash(&action),
            "expiresAt":now+5_000, "action":action
        })
    }

    #[test]
    fn bad_mac_is_rejected_and_consumes_the_request() {
        let now = 10_000;
        let broker = Broker::new([0x42; 32]).unwrap();
        let open = broker
            .begin_engine(1, json!(1), "approval/request", request("a", now), now)
            .unwrap();
        let decision = HostApprovalDecision {
            approval_id: "a".into(),
            outcome: ApprovalOutcome::AllowedOnce,
            mac: "00".repeat(32),
        };
        let result = broker
            .submit_signed(&open.label, "a", decision, now)
            .unwrap();
        assert!(result.command_result.is_err());
        assert!(broker.current(&open.label, now, Instant::now()).is_err());
    }

    #[test]
    fn main_window_cannot_read_or_decide() {
        let now = 10_000;
        let broker = Broker::new([0x42; 32]).unwrap();
        let open = broker
            .begin_engine(1, json!(1), "approval/request", request("a", now), now)
            .unwrap();
        assert!(broker.current("main", now, Instant::now()).is_err());
        assert!(
            broker
                .decide("main", "a", ApprovalOutcome::Rejected, now, Instant::now(),)
                .is_err()
        );
        assert!(broker.current(&open.label, now, Instant::now()).is_ok());
    }

    #[test]
    fn stale_window_cannot_decide_reused_id_after_restart() {
        let now = 10_000;
        let broker = Broker::new([0x42; 32]).unwrap();
        let old = broker
            .begin_engine(1, json!(1), "approval/request", request("same", now), now)
            .unwrap();
        assert!(broker.reject(&old.label, now).is_some());
        let new = broker
            .begin_engine(2, json!(2), "approval/request", request("same", now), now)
            .unwrap();
        assert_ne!(old.label, new.label);
        assert!(
            broker
                .decide(
                    &old.label,
                    "same",
                    ApprovalOutcome::Rejected,
                    now,
                    Instant::now(),
                )
                .is_err()
        );
        assert!(broker.current(&new.label, now, Instant::now()).is_ok());
    }

    #[test]
    fn allow_delay_starts_at_first_current_and_is_not_reset() {
        let now = 10_000;
        let shown = Instant::now();
        let broker = Broker::new([0x42; 32]).unwrap();
        let open = broker
            .begin_engine(1, json!(1), "approval/request", request("a", now), now)
            .unwrap();
        assert!(
            broker
                .decide(
                    &open.label,
                    "a",
                    ApprovalOutcome::AllowedOnce,
                    now,
                    shown + Duration::from_secs(10),
                )
                .is_err()
        );
        broker.current(&open.label, now, shown).unwrap();
        broker
            .current(&open.label, now, shown + Duration::from_millis(500))
            .unwrap();
        assert!(
            broker
                .decide(
                    &open.label,
                    "a",
                    ApprovalOutcome::AllowedOnce,
                    now,
                    shown + ALLOW_DELAY - Duration::from_millis(1),
                )
                .is_err()
        );
        assert!(
            broker
                .decide(
                    &open.label,
                    "a",
                    ApprovalOutcome::AllowedOnce,
                    now,
                    shown + ALLOW_DELAY,
                )
                .unwrap()
                .command_result
                .is_ok()
        );
    }

    #[test]
    fn rejection_is_immediate_before_current() {
        let now = 10_000;
        let broker = Broker::new([0x42; 32]).unwrap();
        let open = broker
            .begin_engine(1, json!(1), "approval/request", request("a", now), now)
            .unwrap();
        assert!(
            broker
                .decide(
                    &open.label,
                    "a",
                    ApprovalOutcome::Rejected,
                    now,
                    Instant::now(),
                )
                .unwrap()
                .command_result
                .is_ok()
        );
    }

    #[test]
    fn second_engine_request_is_rejected_without_replacing_the_first() {
        let now = 10_000;
        let broker = Broker::new([0x42; 32]).unwrap();
        let first = broker
            .begin_engine(1, json!(1), "approval/request", request("a", now), now)
            .unwrap();
        assert!(
            broker
                .begin_engine(1, json!(2), "approval/request", request("b", now), now)
                .is_err()
        );
        assert!(broker.current(&first.label, now, Instant::now()).is_ok());
    }

    #[test]
    fn model_key_preflight_view_contains_no_secret_or_signing_fields() {
        let broker = Broker::new([0x42; 32]).unwrap();
        let (open, _receiver) = broker
            .begin_preflight(
                "hoh/model/key/set",
                &json!({
                    "apiKey":"private-test-key",
                    "baseUrl":"https://api.example.test/v1"
                }),
                10_000,
            )
            .unwrap();
        let encoded = serde_json::to_string(&open.view).unwrap();
        assert!(!encoded.contains("private-test-key"));
        for forbidden in ["apiKey", "keyHash", "action", "mac"] {
            assert!(!encoded.contains(forbidden), "view leaked {forbidden}");
        }
        let stored = broker.pending.lock().unwrap();
        let action = serde_json::to_string(&stored.as_ref().unwrap().request.action).unwrap();
        assert!(!action.contains("private-test-key"));
        assert!(!action.contains("apiKey"));
        assert!(action.contains("keyHash"));
        assert!(action.contains("https://api.example.test/v1"));
    }

    fn permissions() -> Value {
        json!({
            "internet":"ask", "domains":["example.test"], "folders":["/work"],
            "writeFiles":true, "programs":true, "programNetwork":["example.test"],
            "actAsUser":false
        })
    }

    #[test]
    fn every_privileged_method_has_a_concrete_parameterized_text() {
        let samples = vec![
            (
                "hoh/employee/create",
                json!({"draft":{"name":"Алиса","role":"аналитик","rules":["rule-a"],"taskIds":["task-7"],"permissions":permissions()}}),
                "Алиса",
            ),
            (
                "hoh/employee/update",
                json!({"id":"employee-7","patch":{"taskIds":["task-9"],"permissions":permissions()},"rev":1}),
                "employee-7",
            ),
            (
                "hoh/task/draft/decide",
                json!({"draftId":"draft-3","decision":"yes"}),
                "draft-3",
            ),
            (
                "hoh/task/rollback",
                json!({"id":"task-4","version":7}),
                "task-4",
            ),
            (
                "hoh/settings/set",
                json!({"model":{"baseUrl":"https://model.example.test","model":"model-x"}}),
                "model-x",
            ),
            (
                "session/working-directory/set",
                json!({"sessionId":"session-1","path":"/work/project"}),
                "/work/project",
            ),
            (
                "hoh/model/probe",
                json!({"baseUrl":"https://probe.example.test","model":"probe-model"}),
                "probe-model",
            ),
            (
                "hoh/model/key/set",
                json!({"apiKey":"never-show-this","baseUrl":"https://key.example.test"}),
                "https://key.example.test",
            ),
            (
                "hoh/settings/rules/set",
                json!({"internet":"public","folder":"/rules/work"}),
                "/rules/work",
            ),
        ];
        assert_eq!(samples.len(), hoh_protocol::PRIVILEGED_METHODS.len());
        let broker = Broker::new([0x42; 32]).unwrap();
        for (method, params, key) in samples {
            assert!(hoh_protocol::PRIVILEGED_METHODS.contains(&method));
            let (open, _receiver) = broker.begin_preflight(method, &params, 10_000).unwrap();
            assert!(
                open.view.human_text.contains(key),
                "{method}: {}",
                open.view.human_text
            );
            assert_ne!(
                open.view.human_text,
                format!("Разрешить действие {method}?")
            );
            assert!(!open.view.human_text.contains("never-show-this"));
            assert!(broker.reject(&open.label, 10_000).is_some());
        }
    }
}
