use futures_util::StreamExt;
use hoh_credentials::{
    CredentialFuture, CredentialInfo, CredentialKey, CredentialMutator, CredentialProvider,
    CredentialRecord, CredentialRecordEntry, CredentialRecordInfo, CredentialRef, Credentials,
    ResolvedCredential,
};
use hoh_kernel::{Context, Kernel};
use hoh_llm::{
    ContentBlock, GenerateOptions, Llm, LlmCallConfig, LlmRuntime, MessageSource, NewUserMessage,
    StreamChunk, ToolSchema, create_user_message,
};
use hoh_model_host::{
    MODEL_API_KEY_REF, ModelHost, ModelHostConfig, ModelHostKey, ModelHostPlugin,
};
use hoh_protocol::SECRETARY_PROPOSE_METHOD;
use hoh_sdk_jsonrpc_server::{HohRpcMethodRegistry, HohRpcMethods};
use serde_json::json;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
use support::{Reply, serve};

mod support;

#[derive(Default)]
struct MemoryCredentials {
    values: Mutex<BTreeMap<String, String>>,
    fail_set: Mutex<bool>,
}

impl CredentialProvider for MemoryCredentials {
    fn resolve<'a>(
        &'a self,
        reference: &'a CredentialRef,
    ) -> CredentialFuture<'a, Option<ResolvedCredential>> {
        let value = self.values.lock().unwrap().get(reference.as_str()).cloned();
        Box::pin(async move {
            Ok(value.map(|value| ResolvedCredential {
                value,
                source: "test".into(),
            }))
        })
    }
    fn describe<'a>(
        &'a self,
        reference: &'a CredentialRef,
    ) -> CredentialFuture<'a, CredentialInfo> {
        let configured = self.values.lock().unwrap().contains_key(reference.as_str());
        Box::pin(async move {
            Ok(CredentialInfo {
                configured,
                source: configured.then(|| "test".into()),
                writable: true,
                masked: None,
            })
        })
    }
    fn set<'a>(&'a self, reference: &'a CredentialRef, value: &'a str) -> CredentialFuture<'a, ()> {
        let fail = *self.fail_set.lock().unwrap();
        Box::pin(async move {
            if fail {
                return Err(format!("provider echoed {value}").into());
            }
            self.values
                .lock()
                .unwrap()
                .insert(reference.as_str().into(), value.into());
            Ok(())
        })
    }
    fn unset<'a>(&'a self, reference: &'a CredentialRef) -> CredentialFuture<'a, ()> {
        Box::pin(async move {
            self.values.lock().unwrap().remove(reference.as_str());
            Ok(())
        })
    }
    fn read_record<'a>(
        &'a self,
        _: &'a CredentialKey,
    ) -> CredentialFuture<'a, Option<CredentialRecord>> {
        Box::pin(async { Ok(None) })
    }
    fn describe_record<'a>(
        &'a self,
        _: &'a CredentialKey,
    ) -> CredentialFuture<'a, CredentialRecordInfo> {
        Box::pin(async {
            Ok(CredentialRecordInfo {
                configured: false,
                kind: None,
                writable: false,
            })
        })
    }
    fn list_records(&self) -> CredentialFuture<'_, Vec<CredentialRecordEntry>> {
        Box::pin(async { Ok(vec![]) })
    }
    fn modify_record<'a>(
        &'a self,
        _: &'a CredentialKey,
        _: CredentialMutator,
    ) -> CredentialFuture<'a, Option<CredentialRecord>> {
        Box::pin(async { Ok(None) })
    }
    fn delete_record<'a>(&'a self, _: &'a CredentialKey) -> CredentialFuture<'a, ()> {
        Box::pin(async { Ok(()) })
    }
}

async fn boot() -> (Kernel, Context, Arc<MemoryCredentials>) {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let credentials = Arc::new(MemoryCredentials::default());
    let service: Arc<dyn CredentialProvider> = credentials.clone();
    ctx.provide::<Credentials>(service).await.unwrap();
    ctx.provide::<Llm>(Arc::new(LlmRuntime::with_context(&ctx)))
        .await
        .unwrap();
    ctx.provide::<HohRpcMethods>(Arc::new(HohRpcMethodRegistry::default()))
        .await
        .unwrap();
    (kernel, ctx, credentials)
}

async fn plugin(ctx: &Context, config: serde_json::Value) {
    ctx.plugin::<ModelHostPlugin>(config)
        .unwrap()
        .settled()
        .await
        .unwrap();
}

fn sse(fields: serde_json::Value, finish: Option<&str>) -> String {
    let body = json!({"id":"c1","object":"chat.completion.chunk",
        "choices":[{"index":0,"delta":fields,"finish_reason":finish}]});
    format!("data: {body}\n\n")
}

fn user_options(provider: &str, model: &str) -> GenerateOptions {
    GenerateOptions {
        provider: provider.into(),
        model: model.into(),
        messages: vec![create_user_message(NewUserMessage {
            content: vec![ContentBlock::text("hi")],
            source: MessageSource::new("user"),
        }).into()],
        tools: Some(vec![ToolSchema {
            name: "bash".into(),
            description: "Run a command.".into(),
            parameters: json!({"type":"object","properties":{"command":{"type":"string"}},"required":["command"]})
                .as_object().unwrap().clone(),
            defer_loading: None,
        }]),
        ..Default::default()
    }
}

async fn run(ctx: &Context, options: GenerateOptions) -> Vec<StreamChunk> {
    let llm = ctx.get::<Llm>().unwrap();
    let mut stream = llm.stream(ctx, options);
    let mut chunks = Vec::new();
    while let Some(item) = stream.next().await {
        chunks.push(item.unwrap());
    }
    chunks
}

const MODELS: &str = r#"{"object":"list","data":[{"id":"qwen"},{"id":"other"}]}"#;
const COMPLETION: &str = r#"{"id":"c","choices":[{"index":0,"message":{"role":"assistant","content":"p"},"finish_reason":"length"}]}"#;

#[tokio::test]
async fn plugin_registers_openai_route_and_window_methods_in_the_table() {
    let (kernel, ctx, _) = boot().await;
    plugin(
        &ctx,
        json!({"provider":"window-openai","baseUrl":"http://127.0.0.1:8080/v1","model":"local"}),
    )
    .await;
    let llm = ctx.get::<Llm>().unwrap();
    assert_eq!(llm.list_providers()[0].id, "window-openai");
    assert_eq!(
        llm.resolve_model_info("window-openai", "local", None)
            .await
            .unwrap()
            .model
            .id,
        "local"
    );
    assert!(ctx.get::<ModelHostKey>().is_some());
    // The table refuses a second owner, which proves every method is already registered.
    let table = ctx.get::<HohRpcMethods>().unwrap();
    for method in [
        "hoh/model/probe",
        SECRETARY_PROPOSE_METHOD,
        "hoh/model/key/set",
        "hoh/settings/get",
        "hoh/settings/set",
        "hoh/settings/rules/set",
    ] {
        let error = table
            .register(&ctx, method, |_| async { Ok(json!({})) })
            .err()
            .expect("method must already be registered");
        assert!(
            error.to_string().contains("already registered"),
            "{method}: {error}"
        );
    }
    kernel.shutdown().await;
}

#[tokio::test]
async fn secretary_sends_bounded_text_only_to_the_configured_model() {
    let answer = r#"{"reply":"Готово","changes":[]}"#;
    let server = serve(vec![(
        "POST /v1/chat/completions",
        Reply::sse(&[
            sse(json!({"reasoning_content":"private reasoning"}), None),
            sse(json!({"content":answer}), None),
            sse(json!({}), Some("stop")),
            "data: [DONE]\n\n".into(),
        ]),
    )])
    .await;
    let (kernel, ctx, _) = boot().await;
    plugin(
        &ctx,
        json!({"baseUrl":server.base_url,"model":"workspace-model"}),
    )
    .await;
    let host = ctx.get::<ModelHostKey>().unwrap();
    host.call(
        "hoh/model/key/set",
        json!({"baseUrl":server.base_url,"apiKey":"test-key"}),
    )
    .await
    .unwrap();
    let params = json!({
        "message":"Перенеси задачу",
        "instructions":"Reply with JSON",
        "context":{"title":"<script>untrusted title</script>"}
    });
    let response = host
        .call(SECRETARY_PROPOSE_METHOD, params.clone())
        .await
        .unwrap();
    assert_eq!(response, json!({"text":answer}));
    let requests = server.captured();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].authorization.as_deref(),
        Some("Bearer test-key")
    );
    let sent: serde_json::Value = serde_json::from_str(&requests[0].body).unwrap();
    assert_eq!(sent["model"], "workspace-model");
    assert_eq!(sent["max_tokens"], 8_192);
    assert!(sent.get("tools").is_none());
    assert_eq!(sent["messages"].as_array().unwrap().len(), 2);
    assert_eq!(sent["messages"][0]["role"], "system");
    assert!(
        sent["messages"][0]["content"]
            .as_str()
            .unwrap()
            .contains("untrusted snapshot")
    );
    assert_eq!(sent["messages"][1]["role"], "user");
    let input: serde_json::Value =
        serde_json::from_str(sent["messages"][1]["content"].as_str().unwrap()).unwrap();
    assert_eq!(input, params);

    for invalid in [
        json!({"message":"я".repeat(2_049),"instructions":"","context":{}}),
        json!({"message":"hi","instructions":"x".repeat(12 * 1024 + 1),"context":{}}),
        json!({"message":"hi","instructions":"","context":{"description":"x".repeat(128 * 1024)}}),
        json!({"message":"hi","instructions":"","context":[]}),
        json!({"message":"hi","instructions":"","context":{},"model":"override"}),
    ] {
        assert!(host.call(SECRETARY_PROPOSE_METHOD, invalid).await.is_err());
    }
    assert_eq!(server.captured().len(), 1);
    kernel.shutdown().await;
}

#[tokio::test]
async fn secretary_refuses_a_truncated_model_response() {
    let server = serve(vec![(
        "POST /v1/chat/completions",
        Reply::sse(&[
            sse(json!({"content":"partial response"}), Some("length")),
            "data: [DONE]\n\n".into(),
        ]),
    )])
    .await;
    let (kernel, ctx, _) = boot().await;
    let host = ModelHost::new(
        ctx,
        ModelHostConfig {
            base_url: server.base_url,
            ..Default::default()
        },
    )
    .unwrap();
    let error = host
        .call(
            SECRETARY_PROPOSE_METHOD,
            json!({"message":"hi","instructions":"","context":{}}),
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("incomplete response"));
    kernel.shutdown().await;
}

#[tokio::test]
async fn key_is_normalized_stored_only_in_credentials_and_provider_errors_are_redacted() {
    let (kernel, ctx, credentials) = boot().await;
    let host = ModelHost::new(ctx, ModelHostConfig::default()).unwrap();
    host.call(
        "hoh/model/key/set",
        json!({"apiKey":"  sk-window-secret  ","baseUrl":"https://api.example/v1"}),
    )
    .await
    .unwrap();
    assert_eq!(
        credentials
            .values
            .lock()
            .unwrap()
            .get(MODEL_API_KEY_REF)
            .unwrap(),
        "sk-window-secret"
    );
    let rendered = format!(
        "{host:?} {}",
        serde_json::to_string(&host.settings()).unwrap()
    );
    assert!(!rendered.contains("sk-window-secret"));

    *credentials.fail_set.lock().unwrap() = true;
    let secret = "sk-must-not-escape";
    let error = host
        .call(
            "hoh/model/key/set",
            json!({"apiKey":secret,"baseUrl":"https://api.example/v1"}),
        )
        .await
        .unwrap_err();
    assert!(!error.to_string().contains(secret));
    kernel.shutdown().await;
}

#[tokio::test]
async fn settings_commit_only_valid_model_fields_and_reject_non_model_shim_inputs() {
    let (kernel, ctx, _) = boot().await;
    let host = ModelHost::new(ctx, ModelHostConfig::default()).unwrap();
    let before = host.settings();
    for params in [
        json!({"model":{"baseUrl":"not a url","model":"m"}}),
        json!({"model":{"baseUrl":"https://api.example/v1","model":""}}),
        json!({"view":{"theme":"dark","fontSize":18}}),
    ] {
        assert!(host.call("hoh/settings/set", params).await.is_err());
        assert_eq!(host.settings(), before);
    }
    let result = host
        .call(
            "hoh/settings/set",
            json!({"model":{"baseUrl":"https://api.example/v1/","model":"qwen"}}),
        )
        .await
        .unwrap();
    assert_eq!(
        result["model"],
        json!({"baseUrl":"https://api.example/v1","model":"qwen"})
    );
    assert_eq!(
        host.call("hoh/settings/get", json!({})).await.unwrap(),
        result
    );
    assert!(hoh_protocol::validate_result("hoh/settings/get", &result).is_ok());
    kernel.shutdown().await;
}

#[tokio::test]
async fn invalid_probe_is_typed_without_network_and_disposed_host_refuses_calls() {
    let (kernel, ctx, _) = boot().await;
    let host = ModelHost::new(ctx.clone(), ModelHostConfig::default()).unwrap();
    let result = host
        .call("hoh/model/probe", json!({"baseUrl":"bad","model":"m"}))
        .await
        .unwrap();
    assert_eq!(
        result,
        json!({"ok":false,"latencyMs":0,"toolCalls":false,"models":[]})
    );
    ctx.fiber().dispose().await;
    assert!(host.call("hoh/settings/get", json!({})).await.is_err());
    kernel.shutdown().await;
}

#[tokio::test]
async fn unknown_method_and_bad_params_are_errors() {
    let (kernel, ctx, _) = boot().await;
    let host = ModelHost::new(ctx, ModelHostConfig::default()).unwrap();
    assert!(host.call("hoh/employee/list", json!({})).await.is_err());
    assert!(
        host.call("hoh/model/probe", json!({"baseUrl":"http://x"}))
            .await
            .is_err()
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn probe_reports_models_and_tools_and_sends_the_stored_key() {
    let server = serve(vec![
        ("GET /v1/models", Reply::json(MODELS)),
        ("POST /v1/chat/completions", Reply::json(COMPLETION)),
    ])
    .await;
    let (kernel, ctx, _) = boot().await;
    let host = ModelHost::new(ctx, ModelHostConfig::default()).unwrap();
    host.call(
        "hoh/model/key/set",
        json!({"apiKey":"sk-probe","baseUrl":server.base_url}),
    )
    .await
    .unwrap();
    let result = host
        .call(
            "hoh/model/probe",
            json!({"baseUrl":server.base_url,"model":"qwen"}),
        )
        .await
        .unwrap();
    assert!(hoh_protocol::validate_result("hoh/model/probe", &result).is_ok());
    assert_eq!(result["ok"], true);
    assert_eq!(result["toolCalls"], true);
    assert_eq!(result["models"], json!(["qwen", "other"]));
    assert!(result["latencyMs"].is_u64());
    let lines: Vec<_> = server
        .captured()
        .into_iter()
        .map(|r| r.request_line)
        .collect();
    assert_eq!(lines[0], "GET /v1/models");
    assert!(lines.iter().any(|l| l == "POST /v1/chat/completions"));
    assert!(
        server
            .captured()
            .iter()
            .all(|r| r.authorization.as_deref() == Some("Bearer sk-probe"))
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn probe_of_a_dead_address_is_a_typed_failure() {
    let (kernel, ctx, _) = boot().await;
    let host = ModelHost::new(ctx, ModelHostConfig::default()).unwrap();
    let closed = {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        format!("http://{}/v1", listener.local_addr().unwrap())
    };
    let result = host
        .call("hoh/model/probe", json!({"baseUrl":closed,"model":"m"}))
        .await
        .unwrap();
    assert_eq!(result["ok"], false);
    assert_eq!(result["models"], json!([]));
    kernel.shutdown().await;
}

#[tokio::test]
async fn model_streams_text_and_tool_calls_with_the_stored_key_and_follows_settings() {
    let first = serve(vec![(
        "POST /v1/chat/completions",
        Reply::sse(&[
            sse(json!({"content":"Hello"}), None),
            sse(
                json!({"tool_calls":[{"index":0,"id":"call_1","type":"function",
            "function":{"name":"bash","arguments":"{\"command\":\"ls\"}"}}]}),
                None,
            ),
            sse(json!({}), Some("tool_calls")),
            "data: [DONE]\n\n".into(),
        ]),
    )])
    .await;
    let second = serve(vec![(
        "POST /v1/chat/completions",
        Reply::sse(&[
            sse(json!({"content":"Second"}), None),
            sse(json!({}), Some("stop")),
            "data: [DONE]\n\n".into(),
        ]),
    )])
    .await;
    let (kernel, ctx, _) = boot().await;
    plugin(
        &ctx,
        json!({"provider":"window-openai","baseUrl":first.base_url,"model":"qwen"}),
    )
    .await;
    let host = ctx.get::<ModelHostKey>().unwrap();

    // No key stored yet: the request goes out without an Authorization header.
    let chunks = run(&ctx, user_options("window-openai", "qwen")).await;
    let text: String = chunks
        .iter()
        .filter_map(|c| match c {
            StreamChunk::TextDelta { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(text, "Hello");
    assert!(
        chunks
            .iter()
            .any(|c| matches!(c, StreamChunk::ToolCallDelta { name: Some(n), .. } if n == "bash"))
    );
    assert!(
        matches!(chunks.last(), Some(StreamChunk::Finish { reason, .. }) if reason.kind == "tool-calls")
    );
    assert_eq!(first.captured()[0].authorization, None);
    let sent: serde_json::Value = serde_json::from_str(&first.captured()[0].body).unwrap();
    assert_eq!(sent["model"], "qwen");
    assert_eq!(sent["stream"], true);
    assert_eq!(sent["tools"][0]["function"]["name"], "bash");

    // Key stored later is picked up without a restart; settings/set redirects the model.
    host.call(
        "hoh/model/key/set",
        json!({"apiKey":"sk-live","baseUrl":second.base_url}),
    )
    .await
    .unwrap();
    host.call(
        "hoh/settings/set",
        json!({"model":{"baseUrl":second.base_url,"model":"other"}}),
    )
    .await
    .unwrap();
    let chunks = run(&ctx, user_options("window-openai", "other")).await;
    assert!(
        chunks
            .iter()
            .any(|c| matches!(c, StreamChunk::TextDelta { text, .. } if text == "Second"))
    );
    assert_eq!(first.captured().len(), 1);
    assert_eq!(
        second.captured()[0].authorization.as_deref(),
        Some("Bearer sk-live")
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn probe_with_empty_model_lists_models_like_the_window_first_run() {
    let server = serve(vec![
        ("GET /v1/models", Reply::json(MODELS)),
        ("POST /v1/chat/completions", Reply::json(COMPLETION)),
    ])
    .await;
    let (kernel, ctx, _) = boot().await;
    let host = ModelHost::new(ctx, ModelHostConfig::default()).unwrap();
    let result = host
        .call(
            "hoh/model/probe",
            json!({"baseUrl":server.base_url,"model":""}),
        )
        .await
        .unwrap();
    assert_eq!(result["ok"], true);
    assert_eq!(result["models"], json!(["qwen", "other"]));
    let completion = server
        .captured()
        .into_iter()
        .find(|r| r.request_line == "POST /v1/chat/completions")
        .unwrap();
    let sent: serde_json::Value = serde_json::from_str(&completion.body).unwrap();
    assert_eq!(sent["model"], "qwen");
    let list_requests = server
        .captured()
        .iter()
        .filter(|r| r.request_line == "GET /v1/models")
        .count();
    assert_eq!(list_requests, 1, "the model list must be requested once");
    kernel.shutdown().await;
}

#[tokio::test]
async fn probe_with_empty_model_and_no_model_list_uses_the_configured_model() {
    let server = serve(vec![("POST /v1/chat/completions", Reply::json(COMPLETION))]).await;
    let (kernel, ctx, _) = boot().await;
    let host = ModelHost::new(ctx, ModelHostConfig::default()).unwrap();
    let result = host
        .call(
            "hoh/model/probe",
            json!({"baseUrl":server.base_url,"model":"  "}),
        )
        .await
        .unwrap();
    assert_eq!(result["ok"], true);
    assert_eq!(result["models"], json!([]));
    let completion = server
        .captured()
        .into_iter()
        .find(|r| r.request_line.starts_with("POST"))
        .unwrap();
    let sent: serde_json::Value = serde_json::from_str(&completion.body).unwrap();
    assert_eq!(sent["model"], "local");
    kernel.shutdown().await;
}

#[tokio::test]
async fn unloading_the_plugin_aborts_a_probe_in_flight() {
    let server = serve(vec![
        ("GET /v1/models", Reply::json(MODELS)),
        ("POST /v1/chat/completions", Reply::stalled()),
    ])
    .await;
    let (kernel, ctx, _) = boot().await;
    let host = ModelHost::new(ctx.clone(), ModelHostConfig::default()).unwrap();
    let url = server.base_url.clone();
    let probe = tokio::spawn(async move {
        host.call("hoh/model/probe", json!({"baseUrl":url,"model":"qwen"}))
            .await
    });
    let waited = std::time::Instant::now();
    while !server
        .captured()
        .iter()
        .any(|r| r.request_line.starts_with("POST"))
    {
        assert!(
            waited.elapsed().as_secs() < 5,
            "probe never reached the completion"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    ctx.fiber().dispose().await;
    let outcome = tokio::time::timeout(std::time::Duration::from_secs(3), probe)
        .await
        .expect("probe must end quickly after unload")
        .unwrap();
    assert!(outcome.is_err());
    let waited = std::time::Instant::now();
    while server
        .closed_stalled
        .load(std::sync::atomic::Ordering::SeqCst)
        == 0
    {
        assert!(
            waited.elapsed().as_secs() < 3,
            "connection stayed open after unload"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    kernel.shutdown().await;
}

#[tokio::test]
async fn adapter_is_reused_so_the_learned_limit_field_survives_between_turns() {
    let ok = Reply::sse(&[
        sse(json!({"content":"ok"}), None),
        sse(json!({}), Some("stop")),
        "data: [DONE]\n\n".into(),
    ])
    .rejecting("\"max_tokens\"");
    let server = serve(vec![("POST /v1/chat/completions", ok)]).await;
    let (kernel, ctx, _) = boot().await;
    plugin(
        &ctx,
        json!({"provider":"window-openai","baseUrl":server.base_url,"model":"qwen"}),
    )
    .await;
    let posts = || {
        server
            .captured()
            .iter()
            .filter(|r| r.request_line.starts_with("POST"))
            .count()
    };
    for turn in 0..2 {
        let mut options = user_options("window-openai", "qwen");
        options.max_tokens = Some(64);
        let chunks = run(&ctx, options).await;
        assert!(
            matches!(chunks.last(), Some(StreamChunk::Finish { reason, .. }) if reason.kind == "stop"),
            "turn {turn}"
        );
    }
    // Turn 1: refused spelling, then the accepted one. Turn 2 starts with the accepted one.
    assert_eq!(posts(), 3);
    kernel.shutdown().await;
}

#[tokio::test]
async fn saved_model_name_goes_on_the_wire_instead_of_the_session_placeholder() {
    let reply = Reply::sse(&[
        sse(json!({"content":"ok"}), None),
        sse(json!({}), Some("stop")),
        "data: [DONE]\n\n".into(),
    ]);
    let server = serve(vec![("POST /v1/chat/completions", reply)]).await;
    let (kernel, ctx, _) = boot().await;
    plugin(
        &ctx,
        json!({"provider":"window-openai","baseUrl":server.base_url,"model":"window-mock"}),
    )
    .await;
    // The window's `initialize` announces the placeholder "local"; the session keeps that route.
    run(&ctx, user_options("window-openai", "local")).await;
    let sent: serde_json::Value = serde_json::from_str(&server.captured()[0].body).unwrap();
    assert_eq!(sent["model"], "window-mock");
    // A later `settings/set` changes the name on the wire for the same session route.
    let host = ctx.get::<ModelHostKey>().unwrap();
    host.call(
        "hoh/settings/set",
        json!({"model":{"baseUrl":server.base_url,"model":"qwen"}}),
    )
    .await
    .unwrap();
    run(&ctx, user_options("window-openai", "local")).await;
    let sent: serde_json::Value = serde_json::from_str(&server.captured()[1].body).unwrap();
    assert_eq!(sent["model"], "qwen");
    kernel.shutdown().await;
}

#[tokio::test]
async fn call_prepared_for_one_address_keeps_that_addresss_model_name_when_settings_change() {
    let reply = || {
        Reply::sse(&[
            sse(json!({"content":"ok"}), None),
            sse(json!({}), Some("stop")),
            "data: [DONE]\n\n".into(),
        ])
    };
    let a = serve(vec![("POST /v1/chat/completions", reply())]).await;
    let b = serve(vec![("POST /v1/chat/completions", reply())]).await;
    let (kernel, ctx, _) = boot().await;
    plugin(
        &ctx,
        json!({"provider":"window-openai","baseUrl":a.base_url,"model":"m1"}),
    )
    .await;
    let llm = ctx.get::<Llm>().unwrap();
    let prepared = llm
        .prepare_call(
            &ctx,
            LlmCallConfig {
                provider: "window-openai".into(),
                model: "local".into(),
                ..Default::default()
            },
            None,
            None,
        )
        .await
        .unwrap();
    // Settings change between the preparation of the step and its dispatch.
    ctx.get::<ModelHostKey>()
        .unwrap()
        .call(
            "hoh/settings/set",
            json!({"model":{"baseUrl":b.base_url,"model":"m2"}}),
        )
        .await
        .unwrap();
    let mut stream = prepared
        .stream(user_options("window-openai", "local"))
        .unwrap();
    while let Some(item) = stream.next().await {
        item.unwrap();
    }
    let sent: serde_json::Value = serde_json::from_str(&a.captured()[0].body).unwrap();
    assert_eq!(sent["model"], "m1");
    assert!(b.captured().is_empty());
    // The next step is prepared from the new settings.
    run(&ctx, user_options("window-openai", "local")).await;
    let sent: serde_json::Value = serde_json::from_str(&b.captured()[0].body).unwrap();
    assert_eq!(sent["model"], "m2");
    kernel.shutdown().await;
}

#[tokio::test]
async fn subscribers_are_told_about_real_model_and_rules_changes_only() {
    let (kernel, ctx, _) = boot().await;
    let host = ModelHost::new(ctx, ModelHostConfig::default()).unwrap();
    let mut changes = host.subscribe();
    let first = changes.borrow_and_update().clone();
    assert_eq!(first.rules, None);
    assert_eq!(first.model, host.settings().model);

    host.call(
        "hoh/settings/set",
        json!({"model":{"baseUrl":"https://api.example/v1","model":"m1"}}),
    )
    .await
    .unwrap();
    changes.changed().await.unwrap();
    let after_model = changes.borrow_and_update().clone();
    assert_eq!(after_model.model.model, "m1");
    assert_eq!(after_model.rules, None);

    host.call(
        "hoh/settings/rules/set",
        json!({"internet":"public","folder":"/a "}),
    )
    .await
    .unwrap();
    changes.changed().await.unwrap();
    let after_rules = changes.borrow_and_update().clone();
    assert_eq!(after_rules.model, after_model.model);
    assert_eq!(
        serde_json::to_value(after_rules.rules.as_ref().unwrap()).unwrap(),
        json!({"internet":"public","folder":"/a "})
    );
    assert_eq!(after_rules.rules, host.rules());

    // The same values again, a rejected update and a key change are not changes.
    host.call(
        "hoh/settings/rules/set",
        json!({"internet":"public","folder":"/a "}),
    )
    .await
    .unwrap();
    host.call(
        "hoh/settings/set",
        json!({"model":{"baseUrl":"https://api.example/v1","model":"m1"}}),
    )
    .await
    .unwrap();
    assert!(
        host.call(
            "hoh/settings/rules/set",
            json!({"internet":"ask","folder":" "})
        )
        .await
        .is_err()
    );
    host.call(
        "hoh/model/key/set",
        json!({"apiKey":"sk-x","baseUrl":"https://api.example/v1"}),
    )
    .await
    .unwrap();
    assert!(!changes.has_changed().unwrap());
    kernel.shutdown().await;
}

#[tokio::test]
async fn key_stored_between_turns_is_used_by_the_next_turn_on_the_same_address() {
    let reply = Reply::sse(&[
        sse(json!({"content":"ok"}), None),
        sse(json!({}), Some("stop")),
        "data: [DONE]\n\n".into(),
    ]);
    let server = serve(vec![("POST /v1/chat/completions", reply)]).await;
    let (kernel, ctx, _) = boot().await;
    plugin(
        &ctx,
        json!({"provider":"window-openai","baseUrl":server.base_url,"model":"qwen"}),
    )
    .await;
    let host = ctx.get::<ModelHostKey>().unwrap();
    run(&ctx, user_options("window-openai", "qwen")).await;
    host.call(
        "hoh/model/key/set",
        json!({"apiKey":"sk-late","baseUrl":server.base_url}),
    )
    .await
    .unwrap();
    run(&ctx, user_options("window-openai", "qwen")).await;
    let seen: Vec<_> = server
        .captured()
        .into_iter()
        .map(|r| r.authorization)
        .collect();
    assert_eq!(seen, [None, Some("Bearer sk-late".to_owned())]);
    kernel.shutdown().await;
}

#[tokio::test]
async fn parameter_errors_never_quote_the_input() {
    let (kernel, ctx, _) = boot().await;
    let host = ModelHost::new(ctx, ModelHostConfig::default()).unwrap();
    for method in ["hoh/model/key/set", "hoh/model/probe", "hoh/settings/set"] {
        let error = host.call(method, json!("sk-secret")).await.unwrap_err();
        assert!(
            !error.to_string().contains("sk-secret"),
            "{method}: {error}"
        );
    }
    let error = host
        .call("hoh/model/key/set", json!({"apiKey":"sk-secret"}))
        .await
        .unwrap_err();
    assert!(!error.to_string().contains("sk-secret"));
    kernel.shutdown().await;
}

#[tokio::test]
async fn empty_settings_update_returns_current_settings_and_blank_key_is_a_clear_error() {
    let (kernel, ctx, credentials) = boot().await;
    let host = ModelHost::new(ctx, ModelHostConfig::default()).unwrap();
    let current = host.call("hoh/settings/get", json!({})).await.unwrap();
    assert_eq!(
        host.call("hoh/settings/set", json!({})).await.unwrap(),
        current
    );
    let error = host
        .call(
            "hoh/model/key/set",
            json!({"apiKey":"   ","baseUrl":"https://api.example/v1"}),
        )
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "model credential must contain 1..8192 bytes"
    );
    let error = host
        .call(
            "hoh/model/key/set",
            json!({"apiKey":"sk-a\u{7f}b","baseUrl":"https://api.example/v1"}),
        )
        .await
        .unwrap_err();
    assert!(!error.to_string().contains("sk-a") && !error.to_string().contains("Models page"));
    assert!(credentials.values.lock().unwrap().is_empty());
    kernel.shutdown().await;
}

#[tokio::test]
async fn first_run_rules_are_stored_read_back_by_settings_get_and_replayable() {
    let (kernel, ctx, _) = boot().await;
    let host = ModelHost::new(ctx.clone(), ModelHostConfig::default()).unwrap();
    assert_eq!(host.rules(), None);
    let params = json!({"internet":"public","folder":"/home/u/Work"});
    assert!(hoh_protocol::validate_params("hoh/settings/rules/set", &params).is_ok());
    let result = host.call("hoh/settings/rules/set", params).await.unwrap();
    assert!(hoh_protocol::validate_result("hoh/settings/rules/set", &result).is_ok());
    let settings = host.call("hoh/settings/get", json!({})).await.unwrap();
    assert!(hoh_protocol::validate_result("hoh/settings/get", &settings).is_ok());
    assert_eq!(settings["security"]["internet"], "public");
    assert_eq!(settings["security"]["folders"], json!(["/home/u/Work"]));
    // The model part is untouched and a later model change keeps the rules.
    host.call(
        "hoh/settings/set",
        json!({"model":{"baseUrl":"https://api.example/v1","model":"m"}}),
    )
    .await
    .unwrap();
    assert_eq!(host.settings().security.folders, ["/home/u/Work"]);
    let saved = host.rules().unwrap();
    assert_eq!(
        serde_json::to_value(&saved).unwrap(),
        json!({"internet":"public","folder":"/home/u/Work"})
    );

    // A restarted host starts empty and takes the saved rules back through the same entry point.
    let restarted = ModelHost::new(ctx, ModelHostConfig::default()).unwrap();
    assert_eq!(restarted.rules(), None);
    restarted.set_rules(saved.clone()).unwrap();
    assert_eq!(restarted.rules(), Some(saved));
    kernel.shutdown().await;
}

#[tokio::test]
async fn invalid_rules_change_nothing() {
    let (kernel, ctx, _) = boot().await;
    let host = ModelHost::new(ctx, ModelHostConfig::default()).unwrap();
    host.call(
        "hoh/settings/rules/set",
        json!({"internet":"ask","folder":"/a"}),
    )
    .await
    .unwrap();
    for params in [
        json!({"internet":"ask","folder":"  "}),
        json!({"internet":"deny","folder":"/b"}),
        json!({"internet":"ask"}),
        json!({"internet":"ask","folder":"a".repeat(4097)}),
        json!({"internet":"ask","folder":"/a\u{0}b"}),
    ] {
        assert!(host.call("hoh/settings/rules/set", params).await.is_err());
    }
    assert_eq!(host.rules().unwrap().folder, "/a");
    // The longest allowed folder is accepted.
    let longest = "a".repeat(4096);
    host.call(
        "hoh/settings/rules/set",
        json!({"internet":"ask","folder":longest}),
    )
    .await
    .unwrap();
    assert_eq!(host.rules().unwrap().folder, longest);
    kernel.shutdown().await;
}

#[tokio::test]
async fn rules_folder_is_stored_exactly_as_the_user_allowed_it() {
    let (kernel, ctx, _) = boot().await;
    let host = ModelHost::new(ctx, ModelHostConfig::default()).unwrap();
    host.call(
        "hoh/settings/rules/set",
        json!({"internet":"ask","folder":"/a "}),
    )
    .await
    .unwrap();
    let settings = host.call("hoh/settings/get", json!({})).await.unwrap();
    assert_eq!(settings["security"]["folders"], json!(["/a "]));
    assert_eq!(host.rules().unwrap().folder, "/a ");
    kernel.shutdown().await;
}

#[test]
fn config_has_no_key_field() {
    let config = ModelHostConfig::default();
    assert!(!format!("{config:?}").contains("apiKey"));
    let value = serde_json::to_value(config).unwrap();
    assert!(
        value
            .as_object()
            .unwrap()
            .keys()
            .all(|k| !k.to_lowercase().contains("key"))
    );
    assert!(serde_json::from_value::<ModelHostConfig>(json!({"apiKey":"x"})).is_err());
}
