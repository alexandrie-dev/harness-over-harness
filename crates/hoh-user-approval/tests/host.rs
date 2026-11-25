use crate::{test_support::*, *};
use futures_util::future::BoxFuture;
use hoh_error::BoxError;
use hoh_protocol::{ApprovalOutcome as ProtocolOutcome, HostApprovalDecision, HostApprovalRequest};
use hoh_timeout::{AbortController, AbortSignal};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

type Responder = Arc<
    dyn Fn(Value, Option<AbortSignal>) -> BoxFuture<'static, Result<Value, BoxError>> + Send + Sync,
>;

struct Peer {
    requests: Mutex<Vec<Value>>,
    notifications: Mutex<Vec<(&'static str, Value)>>,
    signals: Mutex<Vec<AbortSignal>>,
    responder: Responder,
}

impl Peer {
    fn new(
        responder: impl Fn(Value, Option<AbortSignal>) -> BoxFuture<'static, Result<Value, BoxError>>
        + Send
        + Sync
        + 'static,
    ) -> Arc<Self> {
        Arc::new(Self {
            requests: Mutex::new(Vec::new()),
            notifications: Mutex::new(Vec::new()),
            signals: Mutex::new(Vec::new()),
            responder: Arc::new(responder),
        })
    }
}

impl HostApprovalPeer for Peer {
    fn request(
        &self,
        method: &'static str,
        params: Value,
        signal: Option<AbortSignal>,
    ) -> BoxFuture<'static, Result<Value, BoxError>> {
        assert_eq!(method, "approval/request");
        lock(&self.requests).push(params.clone());
        if let Some(signal) = &signal {
            lock(&self.signals).push(signal.clone());
        }
        (self.responder)(params, signal)
    }

    fn notify(&self, method: &'static str, params: Value) -> Result<(), BoxError> {
        lock(&self.notifications).push((method, params));
        Ok(())
    }
}

fn key_pair(byte: u8) -> ([u8; 32], String) {
    let key = [byte; 32];
    let hex = hoh_protocol::approval::hex(&key);
    (key, hex)
}

fn signed(key: [u8; 32], outcome: ProtocolOutcome) -> Responder {
    Arc::new(move |params, _| {
        let request: HostApprovalRequest = serde_json::from_value(params).unwrap();
        let text = serde_json::to_value(&outcome)
            .unwrap()
            .as_str()
            .unwrap()
            .to_owned();
        let decision = HostApprovalDecision {
            approval_id: request.approval_id.clone(),
            outcome: outcome.clone(),
            mac: hoh_protocol::approval::mac(&key, &request, &text),
        };
        Box::pin(async move { Ok(serde_json::to_value(decision)?) })
    })
}

#[tokio::test]
async fn host_mode_emits_typed_pending_request_and_resolved_for_tool_approval() {
    let h = Harness::mounted().await;
    let agent = h.open_agent();
    let (key, key_hex) = key_pair(0x42);
    let responder = signed(key, ProtocolOutcome::AllowedOnce);
    let peer = Peer::new(move |params, signal| responder(params, signal));
    h.service()
        .install_host_mode(&key_hex, peer.clone())
        .unwrap();

    let mut request = ApprovalRequest::new(agent.clone(), "bash");
    request.call_id = Some("call-1".into());
    request.reason = Some("команда меняет файлы".into());
    request.display_reason = Some(DisplayReason::new("Allow the requested command?"));
    assert_eq!(
        h.service().request(request).await.unwrap(),
        ApprovalOutcome::AllowedOnce
    );

    let requests = lock(&peer.requests).clone();
    assert_eq!(requests.len(), 1);
    let host: HostApprovalRequest = serde_json::from_value(requests[0].clone()).unwrap();
    assert_eq!(host.session_id, agent.session().id().as_str());
    assert_eq!(host.tool_name, "bash");
    assert_eq!(host.call_id.as_deref(), Some("call-1"));
    assert_eq!(host.reason.as_deref(), Some("команда меняет файлы"));
    assert_eq!(host.details, None);
    assert!(
        requests[0].get("details").is_none(),
        "no details, no key: {}",
        requests[0]
    );
    assert_eq!(host.risk, hoh_protocol::Risk::High);
    assert_eq!(host.human_text, "Allow the requested command?");
    assert_eq!(
        host.action,
        json!({"method":"approval/request","params":{"sessionId":agent.session().id(),"toolName":"bash","callId":"call-1","reason":"команда меняет файлы"}})
    );
    assert_eq!(
        host.action_hash,
        hoh_protocol::approval::action_hash(&host.action)
    );
    hoh_protocol::approval::validate_host_request(&host).unwrap();

    let notifications = lock(&peer.notifications).clone();
    assert_eq!(notifications.len(), 2);
    assert_eq!(notifications[0].0, "approval.pending");
    assert_eq!(notifications[0].1["approvalId"], host.approval_id);
    assert_eq!(notifications[0].1["humanText"], host.human_text);
    assert_eq!(notifications[1].0, "approval.resolved");
    assert_eq!(notifications[1].1["approvalId"], host.approval_id);
    assert_eq!(notifications[1].1["outcome"], "allowed-once");
    assert_pair(agent.session(), ApprovalOutcome::AllowedOnce);
    h.close().await;
}

/// `details` rides next to `reason` under that name, stays out of the signed action and out of the
/// session journal, and a decision signed for the request still verifies.
#[tokio::test]
async fn details_reach_the_host_outside_the_action_and_the_journal() {
    let h = Harness::mounted().await;
    let agent = h.open_agent();
    let (key, key_hex) = key_pair(0x42);
    let responder = signed(key, ProtocolOutcome::AllowedOnce);
    let peer = Peer::new(move |params, signal| responder(params, signal));
    h.service()
        .install_host_mode(&key_hex, peer.clone())
        .unwrap();

    let mut request = ApprovalRequest::new(agent.clone(), "bash");
    request.call_id = Some("call-1".into());
    request.reason = Some("Удалить файл old.txt".into());
    request.details = Some("Команда: rm old.txt".into());
    request.display_reason = Some(DisplayReason::new("Удалить файл"));
    assert_eq!(
        h.service().request(request).await.unwrap(),
        ApprovalOutcome::AllowedOnce
    );

    let requests = lock(&peer.requests).clone();
    assert_eq!(requests[0]["details"], "Команда: rm old.txt");
    assert_eq!(requests[0]["reason"], "Удалить файл old.txt");
    let host: HostApprovalRequest = serde_json::from_value(requests[0].clone()).unwrap();
    assert_eq!(host.details.as_deref(), Some("Команда: rm old.txt"));
    assert_eq!(
        host.action,
        json!({"method":"approval/request","params":{"sessionId":agent.session().id(),"toolName":"bash","callId":"call-1","reason":"Удалить файл old.txt"}}),
        "details are not part of the signed action"
    );
    hoh_protocol::approval::validate_host_request(&host).unwrap();
    let journal: Vec<_> = audit(agent.session())
        .iter()
        .map(|event| event.data().to_string())
        .collect();
    assert!(
        journal.iter().all(|line| !line.contains("rm old.txt")),
        "the command is not journaled: {journal:?}"
    );
    assert_pair(agent.session(), ApprovalOutcome::AllowedOnce);
    h.close().await;
}

/// The bash tool's own escalation ask carries the model's justification. When its grant is gone
/// (expired or never made) it reaches the host: there the engine words it in Russian, the model's
/// words are only in `details`, and the session journal keeps the ask as the tool made it.
#[tokio::test]
async fn an_escalation_ask_reaching_the_host_is_worded_by_the_engine_and_the_justification_is_in_details()
 {
    let h = Harness::mounted().await;
    let agent = h.open_agent();
    let (key, key_hex) = key_pair(0x42);
    let responder = signed(key, ProtocolOutcome::AllowedOnce);
    let peer = Peer::new(move |params, signal| responder(params, signal));
    h.service()
        .install_host_mode(&key_hex, peer.clone())
        .unwrap();

    let justification = "нужно выполнить RM \"old.txt\" (rm old.txt) и ещё: двоеточие";
    let original = format!("escalate sandbox to danger-full-access: {justification}");
    let ask = |tool: &str, call: &str| {
        let mut request = ApprovalRequest::new(agent.clone(), tool);
        request.call_id = Some(call.into());
        request.reason = Some(original.clone());
        let mut display = DisplayReason::new(format!(
            "Allow this operation with danger-full-access permissions: {justification}"
        ));
        display.insert("ru", "Разрешить: русский текст инструмента");
        request.display_reason = Some(display);
        request
    };
    // Expired grant.
    h.service()
        .pre_approve_escalation(agent.session(), &"c1".into(), "bash", &original);
    h.service()
        .age_escalation_grants(std::time::Duration::from_secs(301));
    assert_eq!(
        h.service().request(ask("bash", "c1")).await.unwrap(),
        ApprovalOutcome::AllowedOnce
    );
    // No grant at all, another tool.
    assert_eq!(
        h.service().request(ask("write", "c2")).await.unwrap(),
        ApprovalOutcome::AllowedOnce
    );

    let requests = lock(&peer.requests).clone();
    assert_eq!(requests.len(), 2);
    for (value, what) in [(&requests[0], "the command"), (&requests[1], "the action")] {
        let host: HostApprovalRequest = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(
            host.human_text,
            format!("The assistant asks to run {what} outside its folder")
        );
        assert_eq!(
            host.reason.as_deref(),
            Some(
                format!(
                    "{}. See details for the assistant’s justification",
                    host.human_text
                )
                .as_str()
            )
        );
        assert_eq!(
            host.details.as_deref(),
            Some(format!("Assistant justification: {justification}").as_str())
        );
        let window_text = format!("{} {}", host.human_text, host.reason.as_deref().unwrap());
        for word in ["old.txt", "RM", "rm", "двоеточие", "выполнить RM", "Allow"]
        {
            assert!(!window_text.contains(word), "{word:?} in {window_text}");
        }
        assert_eq!(
            host.action["params"]["reason"].as_str(),
            host.reason.as_deref(),
            "the signed action carries the same reason that is shown"
        );
        hoh_protocol::approval::validate_host_request(&host).unwrap();
    }
    // The engine's own journal keeps the ask as the tool made it.
    let asked: Vec<_> = audit(agent.session())
        .iter()
        .filter(|event| event.event_type() == "approval/asked")
        .map(|event| event.data()["reason"].clone())
        .collect();
    assert_eq!(asked, vec![json!(original), json!(original)]);
    h.close().await;
}

/// Other modes get their own Russian words; an overlong justification still fits the protocol.
#[tokio::test]
async fn escalation_wording_covers_other_modes_and_an_overlong_justification() {
    let h = Harness::mounted().await;
    let agent = h.open_agent();
    let (key, key_hex) = key_pair(0x42);
    let responder = signed(key, ProtocolOutcome::AllowedOnce);
    let peer = Peer::new(move |params, signal| responder(params, signal));
    h.service()
        .install_host_mode(&key_hex, peer.clone())
        .unwrap();
    for (call, mode) in [("c1", "workspace-write"), ("c2", "something-new")] {
        let mut request = ApprovalRequest::new(agent.clone(), "bash");
        request.call_id = Some(call.into());
        request.reason = Some(format!("escalate sandbox to {mode}: {}", "😀".repeat(6000)));
        assert_eq!(
            h.service().request(request).await.unwrap(),
            ApprovalOutcome::AllowedOnce
        );
    }
    let requests = lock(&peer.requests).clone();
    let first: HostApprovalRequest = serde_json::from_value(requests[0].clone()).unwrap();
    let second: HostApprovalRequest = serde_json::from_value(requests[1].clone()).unwrap();
    assert_eq!(
        first.human_text,
        "The assistant asks to run the command with write access to its working folder"
    );
    assert_eq!(
        second.human_text,
        "The assistant asks to run the command with additional permissions"
    );
    for host in [&first, &second] {
        let details = host.details.as_deref().unwrap();
        assert!(details.encode_utf16().count() <= 8192);
        hoh_protocol::approval::validate_host_request(host).unwrap();
    }
    h.close().await;
}

#[tokio::test]
async fn second_install_is_rejected_and_replay_cannot_authorize_a_new_request() {
    let h = Harness::mounted().await;
    let agent = h.open_agent();
    let (key, key_hex) = key_pair(0x33);
    let replay = Arc::new(Mutex::new(None::<Value>));
    let saved = replay.clone();
    let peer = Peer::new(move |params, _| {
        let request: HostApprovalRequest = serde_json::from_value(params).unwrap();
        let mut saved = lock(&saved);
        let answer = saved.get_or_insert_with(|| {
            serde_json::to_value(HostApprovalDecision {
                approval_id: request.approval_id.clone(),
                outcome: ProtocolOutcome::AllowedOnce,
                mac: hoh_protocol::approval::mac(&key, &request, "allowed-once"),
            })
            .unwrap()
        });
        let answer = answer.clone();
        Box::pin(async move { Ok(answer) })
    });
    h.service()
        .install_host_mode(&key_hex, peer.clone())
        .unwrap();
    let (_, other_key) = key_pair(0x55);
    assert_eq!(
        h.service()
            .install_host_mode(&other_key, peer.clone())
            .unwrap_err()
            .to_string(),
        "host approval mode is already configured"
    );
    assert!(h.service().host_mode_enabled());

    assert_eq!(
        h.service()
            .request(ApprovalRequest::new(agent.clone(), "bash"))
            .await
            .unwrap(),
        ApprovalOutcome::AllowedOnce
    );
    assert_eq!(
        h.service()
            .request(ApprovalRequest::new(agent.clone(), "write"))
            .await
            .unwrap(),
        ApprovalOutcome::Unavailable
    );
    let notifications = lock(&peer.notifications).clone();
    assert_eq!(notifications[1].1["outcome"], "allowed-once");
    assert_eq!(notifications[3].1["outcome"], "unavailable");
    h.close().await;
}

#[tokio::test]
async fn invalid_mac_fails_closed_and_is_resolved_as_unavailable() {
    let h = Harness::mounted().await;
    let agent = h.open_agent();
    let (_, key_hex) = key_pair(0x22);
    let peer = Peer::new(|params, _| {
        let request: HostApprovalRequest = serde_json::from_value(params).unwrap();
        Box::pin(async move {
            Ok(json!({
                "approvalId":request.approval_id,
                "outcome":"allowed-once",
                "mac":"00".repeat(32)
            }))
        })
    });
    h.service()
        .install_host_mode(&key_hex, peer.clone())
        .unwrap();
    assert_eq!(
        h.service()
            .request(ApprovalRequest::new(agent.clone(), "bash"))
            .await
            .unwrap(),
        ApprovalOutcome::Unavailable
    );
    assert_eq!(
        lock(&peer.notifications).last().unwrap().1["outcome"],
        "unavailable"
    );
    assert_pair(agent.session(), ApprovalOutcome::Unavailable);
    h.close().await;
}

#[tokio::test(start_paused = true)]
async fn five_minute_expiry_signals_transport_cancels_once_and_ignores_late_answer() {
    let h = Harness::mounted().await;
    let agent = h.open_agent();
    let (_, key_hex) = key_pair(0x11);
    let peer = Peer::new(|_, _| Box::pin(std::future::pending()));
    h.service()
        .install_host_mode(&key_hex, peer.clone())
        .unwrap();
    let pending = h
        .service()
        .request(ApprovalRequest::new(agent.clone(), "write"));
    tokio::task::yield_now().await;
    tokio::time::advance(std::time::Duration::from_secs(300)).await;
    assert_eq!(pending.await.unwrap(), ApprovalOutcome::Cancelled);
    assert!(lock(&peer.signals)[0].aborted());
    assert_eq!(
        lock(&peer.notifications).last().unwrap().1["outcome"],
        "cancelled"
    );
    tokio::time::advance(std::time::Duration::from_secs(300)).await;
    tokio::task::yield_now().await;
    assert_eq!(lock(&peer.notifications).len(), 2);
    assert_pair(agent.session(), ApprovalOutcome::Cancelled);
    h.close().await;
}

#[tokio::test]
async fn abort_signals_transport_and_bash_and_fs_share_the_same_host_path() {
    let h = Harness::mounted().await;
    let agent = h.open_agent();
    let (key, key_hex) = key_pair(0x66);
    let responder = signed(key, ProtocolOutcome::Rejected);
    let peer = Peer::new(move |params, signal| responder(params, signal));
    h.service()
        .install_host_mode(&key_hex, peer.clone())
        .unwrap();
    for tool in ["bash", "write"] {
        let mut request = ApprovalRequest::new(agent.clone(), tool);
        request.reason = Some("MODEL-CONTROLLED REASON".into());
        assert_eq!(
            h.service().request(request).await.unwrap(),
            ApprovalOutcome::Rejected
        );
    }
    let requests = lock(&peer.requests).clone();
    assert_eq!(
        requests
            .iter()
            .map(|request| request["toolName"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["bash", "write"]
    );
    assert_eq!(requests[0]["humanText"], "Allow tool action bash?");
    assert_eq!(requests[1]["humanText"], "Allow tool action write?");
    assert_ne!(requests[0]["humanText"], requests[0]["reason"]);
    h.close().await;

    let h = Harness::mounted().await;
    let agent = h.open_agent();
    let (_, key_hex) = key_pair(0x77);
    let peer = Peer::new(|_, _| Box::pin(std::future::pending()));
    h.service()
        .install_host_mode(&key_hex, peer.clone())
        .unwrap();
    let controller = AbortController::new();
    let mut request = ApprovalRequest::new(agent.clone(), "bash");
    request.signal = Some(controller.signal());
    let pending = h.service().request(request);
    tokio::task::yield_now().await;
    controller.abort();
    assert_eq!(pending.await.unwrap(), ApprovalOutcome::Cancelled);
    assert!(lock(&peer.signals)[0].aborted());
    assert_eq!(
        lock(&peer.notifications).last().unwrap().1["outcome"],
        "cancelled"
    );
    h.close().await;
}

#[tokio::test]
async fn privileged_method_is_asked_with_typed_text_and_runs_only_after_a_verified_allow() {
    let h = Harness::mounted().await;
    let (key, key_hex) = key_pair(0x12);
    let responder = signed(key, ProtocolOutcome::AllowedOnce);
    let peer = Peer::new(move |params, signal| responder(params, signal));
    h.service()
        .install_host_mode(&key_hex, peer.clone())
        .unwrap();
    let params = json!({"sessionId":"s-1","path":"/work/project"});
    h.service()
        .approve_privileged("session/working-directory/set", &params)
        .await
        .unwrap();
    let requests = lock(&peer.requests).clone();
    let host: HostApprovalRequest = serde_json::from_value(requests[0].clone()).unwrap();
    assert_eq!(host.session_id, "s-1");
    assert_eq!(host.tool_name, "session/working-directory/set");
    assert_eq!(host.human_text, "Allow work in folder «/work/project»?");
    assert_eq!(
        host.action,
        json!({"method":"session/working-directory/set","params":params})
    );
    assert_eq!(host.risk, hoh_protocol::Risk::High);
    hoh_protocol::approval::validate_host_request(&host).unwrap();
    let notifications = lock(&peer.notifications).clone();
    assert_eq!(notifications[0].0, "approval.pending");
    assert_eq!(notifications[1].1["outcome"], "allowed-once");

    // A call not bound to a session uses the host session, and the text names provider and model.
    h.service()
        .approve_privileged(
            "hoh/model/probe",
            &json!({"baseUrl":"https://api.example.com/v1","model":"m-1"}),
        )
        .await
        .unwrap();
    let requests = lock(&peer.requests).clone();
    assert_eq!(requests[1]["sessionId"], "host");
    assert_eq!(
        requests[1]["humanText"],
        "Allow testing model «m-1» at provider «https://api.example.com/v1»?"
    );
    h.close().await;
}

/// Review 232500, item 1: the user's choices of an executor's access and of who answers new chats
/// are asked in words that name the executor and what the choice allows, never as the bare
/// method name.
#[tokio::test]
async fn executor_choices_are_asked_in_words_that_name_the_executor_and_the_right() {
    let h = Harness::mounted().await;
    let (key, key_hex) = key_pair(0x15);
    let responder = signed(key, ProtocolOutcome::AllowedOnce);
    let peer = Peer::new(move |params, signal| responder(params, signal));
    h.service()
        .install_host_mode(&key_hex, peer.clone())
        .unwrap();
    let cases = [
        (
            "hoh/executor/access/set",
            json!({"executor":"claude","mode":"full"}),
            "Enable Full access for Claude? The assistant can delete and change files in any folder you can access, run programs, and access the internet without asking. These actions cannot be undone.",
        ),
        (
            "hoh/executor/access/set",
            json!({"executor":"claude","mode":"confirm"}),
            "Return Claude to Supervised mode?",
        ),
        (
            "hoh/executor/access/set",
            json!({"executor":"codex","mode":"confirm","runtimeMode":"auto-accept-edits"}),
            "Enable Accept edits for Codex? The assistant automatically accepts file changes in the working folder but asks before other actions.",
        ),
        (
            "hoh/executor/access/set",
            json!({"executor":"codex","mode":"confirm","runtimeMode":"auto"}),
            "Enable Auto for Codex? The assistant automatically accepts file changes in the working folder and reviews other permission requests automatically.",
        ),
        (
            "hoh/executor/select",
            json!({"executor":"claude","model":"claude-sonnet-4-5"}),
            "Use Claude (model claude-sonnet-4-5) to respond?",
        ),
        (
            "hoh/executor/select",
            json!({"executor":null,"model":"local-model"}),
            "Use the model on this computer to respond?",
        ),
    ];
    for (index, (method, params, text)) in cases.iter().enumerate() {
        assert_eq!(
            crate::method_human_text(method, params).as_deref(),
            Ok(*text)
        );
        h.service()
            .approve_privileged(method, params)
            .await
            .unwrap();
        let requests = lock(&peer.requests).clone();
        let host: HostApprovalRequest = serde_json::from_value(requests[index].clone()).unwrap();
        assert_eq!(host.human_text, *text);
        assert_eq!(host.tool_name, *method);
        assert_eq!(host.session_id, "host");
        assert_eq!(host.risk, hoh_protocol::Risk::High);
        assert_eq!(host.action, json!({"method": method, "params": params}));
        hoh_protocol::approval::validate_host_request(&host).unwrap();
    }
    // A malformed choice is refused before anything is asked.
    let asked = lock(&peer.requests).len();
    assert!(
        h.service()
            .approve_privileged(
                "hoh/executor/access/set",
                &json!({"executor":"claude","mode":"everything"})
            )
            .await
            .is_err()
    );
    assert!(
        h.service()
            .approve_privileged(
                "hoh/executor/access/set",
                &json!({"executor":"codex","mode":"full","runtimeMode":"auto"})
            )
            .await
            .is_err()
    );
    assert_eq!(lock(&peer.requests).len(), asked);
    h.close().await;
}

#[tokio::test]
async fn model_key_never_travels_in_the_approval_and_a_rejection_refuses_the_call() {
    let h = Harness::mounted().await;
    let (key, key_hex) = key_pair(0x13);
    let responder = signed(key, ProtocolOutcome::Rejected);
    let peer = Peer::new(move |params, signal| responder(params, signal));
    h.service()
        .install_host_mode(&key_hex, peer.clone())
        .unwrap();
    let secret = "sk-very-secret-value";
    let error = h
        .service()
        .approve_privileged(
            "hoh/model/key/set",
            &json!({"apiKey":secret,"baseUrl":"https://api.example.com/v1"}),
        )
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "hoh/model/key/set was not approved: rejected"
    );
    let sent = format!(
        "{:?}{:?}",
        lock(&peer.requests).clone(),
        lock(&peer.notifications).clone()
    );
    assert!(!sent.contains(secret));
    let requests = lock(&peer.requests).clone();
    assert_eq!(
        requests[0]["action"]["params"]["keyHash"],
        hoh_protocol::approval::action_hash(&json!({"apiKey":secret}))
    );
    assert_eq!(
        requests[0]["humanText"],
        "Save the API key for provider «https://api.example.com/v1»?"
    );
    h.close().await;
}

#[tokio::test]
async fn privileged_method_is_refused_without_host_mode_on_bad_params_and_on_a_bad_mac() {
    let h = Harness::mounted().await;
    assert!(
        h.service()
            .approve_privileged(
                "session/working-directory/set",
                &json!({"sessionId":"s","path":"/x"})
            )
            .await
            .is_err()
    );

    let (_, key_hex) = key_pair(0x14);
    let peer = Peer::new(|params, _| {
        let request: HostApprovalRequest = serde_json::from_value(params).unwrap();
        Box::pin(async move {
            Ok(
                json!({"approvalId":request.approval_id,"outcome":"allowed-once","mac":"00".repeat(32)}),
            )
        })
    });
    h.service()
        .install_host_mode(&key_hex, peer.clone())
        .unwrap();
    assert!(
        h.service()
            .approve_privileged("session/working-directory/set", &json!({"path":7}))
            .await
            .is_err()
    );
    assert!(lock(&peer.requests).is_empty());
    let error = h
        .service()
        .approve_privileged(
            "session/working-directory/set",
            &json!({"sessionId":"s","path":"/x"}),
        )
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "session/working-directory/set was not approved: unavailable"
    );
    h.close().await;
}

#[tokio::test]
async fn host_mode_survives_the_replacement_of_the_service() {
    let mut h = Harness::context().await;
    let approval = h.load::<ApprovalService>(json!({})).await;
    let agent = h.open_agent();
    let (key, key_hex) = key_pair(0x21);
    let responder = signed(key, ProtocolOutcome::AllowedOnce);
    let peer = Peer::new(move |params, signal| responder(params, signal));
    h.service()
        .install_host_mode(&key_hex, peer.clone())
        .unwrap();

    // The projection registry leaves and returns: the service is withdrawn and built again.
    h.registry_owner.take().unwrap().dispose().await;
    assert!(h.ctx.get::<Approval>().is_none());
    h.registry_owner = Some(
        h.load::<hoh_session_projection::SessionProjectionRegistry>(json!({}))
            .await,
    );
    approval.settled().await.unwrap();

    // The replacement still asks the window with a signed request, for tools and for methods.
    assert!(h.service().host_mode_enabled());
    assert_eq!(
        h.service()
            .request(ApprovalRequest::new(agent.clone(), "bash"))
            .await
            .unwrap(),
        ApprovalOutcome::AllowedOnce
    );
    assert_eq!(lock(&peer.requests).len(), 1);
    h.service()
        .approve_privileged(
            "session/working-directory/set",
            &json!({"sessionId":"s","path":"/x"}),
        )
        .await
        .unwrap();
    assert_eq!(lock(&peer.requests).len(), 2);
    // The key is still set once: a second install is refused on the replacement too.
    assert!(h.service().install_host_mode(&key_hex, peer).is_err());
    h.close().await;
}
