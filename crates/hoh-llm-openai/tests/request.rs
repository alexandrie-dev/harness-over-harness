mod support;
use hoh_llm::{ContentBlock, ImageBlock, LlmAdapter, ReasoningEffortId};
use hoh_llm_openai::{ServerKind, ToolChoice};
use serde_json::{Value, json};
use support::*;

async fn sent(server: &Server, build: impl FnOnce(&mut hoh_llm::GenerateOptions)) -> Value {
    sent_with(server, |_| {}, build).await
}

async fn sent_with(
    server: &Server,
    edit: impl FnOnce(&mut hoh_llm_openai::OpenAiConfig),
    build: impl FnOnce(&mut hoh_llm::GenerateOptions),
) -> Value {
    let adapter = adapter(server, edit);
    let mut opts = options(vec![user("hello")]);
    build(&mut opts);
    let _ = run(&adapter, opts).await;
    server.last_body()
}

fn ok_server_reply() -> Vec<(&'static str, Vec<Reply>)> {
    let text = format!(
        "{}{}{}",
        delta(json!({"content": "ok"})),
        finish("stop"),
        done()
    );
    vec![(CHAT, vec![Reply::sse_text(&text)])]
}

#[tokio::test]
async fn basic_request_shape_and_headers() {
    let server = serve(ok_server_reply()).await;
    let body = sent_with(
        &server,
        |c| c.api_key = Some("sk-local".into()),
        |o| {
            o.system = Some("You are helpful.".into());
            o.temperature = Some(0.25);
            o.max_tokens = Some(256);
            o.stop = Some(vec!["END".into()]);
        },
    )
    .await;
    assert_eq!(
        body,
        json!({
            "model": "qwen", "stream": true, "stream_options": {"include_usage": true},
            "messages": [
                {"role": "system", "content": "You are helpful."},
                {"role": "user", "content": "hello"}
            ],
            "temperature": 0.25, "max_tokens": 256, "stop": ["END"]
        })
    );
    let request = &server.captured()[0];
    assert_eq!(request.path, "/v1/chat/completions");
    assert_eq!(request.headers["authorization"], "Bearer sk-local");
    assert_eq!(request.headers["accept"], "text/event-stream");
    assert!(request.headers["user-agent"].contains('/'));
}

#[tokio::test]
async fn no_authorization_header_without_a_key() {
    let server = serve(ok_server_reply()).await;
    sent(&server, |_| {}).await;
    assert!(!server.captured()[0].headers.contains_key("authorization"));
}

#[tokio::test]
async fn tools_are_function_declarations_with_tool_choice() {
    let server = serve(ok_server_reply()).await;
    let body = sent(&server, |o| {
        o.tools = Some(vec![tool("weather", &["city"])])
    })
    .await;
    assert_eq!(
        body["tools"],
        json!([{"type": "function", "function": {
            "name": "weather", "description": "Tool weather.",
            "parameters": {"type": "object", "properties": {"city": {"type": "string"}}, "required": ["city"]}}}])
    );
    assert_eq!(body["tool_choice"], "auto");

    let body = sent_with(
        &server,
        |c| c.tool_choice = ToolChoice::Function("weather".into()),
        |o| {
            o.tools = Some(vec![tool("weather", &["city"])]);
        },
    )
    .await;
    assert_eq!(
        body["tool_choice"],
        json!({"type": "function", "function": {"name": "weather"}})
    );

    let body = sent_with(
        &server,
        |c| c.tool_choice = ToolChoice::Required,
        |o| {
            o.tools = Some(vec![tool("weather", &[])]);
        },
    )
    .await;
    assert_eq!(body["tool_choice"], "required");

    // No tools: neither `tools` nor `tool_choice`.
    let body = sent(&server, |o| o.tools = Some(vec![])).await;
    assert!(body.get("tools").is_none() && body.get("tool_choice").is_none());
}

#[tokio::test]
async fn history_with_tool_calls_and_results() {
    let server = serve(ok_server_reply()).await;
    let body = sent(&server, |o| {
        o.messages = vec![
            user("weather in Paris?"),
            assistant(vec![
                ContentBlock::reasoning("not replayed"),
                ContentBlock::text("Checking."),
                call("call_1", "weather", "{\"city\":\"Paris\"}"),
                call("call_2", "weather", "not json"),
            ]),
            tool_result("call_1", "sunny"),
            tool_result("call_2", "error"),
            user("thanks"),
        ];
    })
    .await;
    assert_eq!(
        body["messages"],
        json!([
            {"role": "user", "content": "weather in Paris?"},
            {"role": "assistant", "content": "Checking.", "tool_calls": [
                {"id": "call_1", "type": "function", "function": {"name": "weather", "arguments": "{\"city\":\"Paris\"}"}},
                {"id": "call_2", "type": "function", "function": {"name": "weather", "arguments": "{}"}}
            ]},
            {"role": "tool", "tool_call_id": "call_1", "content": "sunny"},
            {"role": "tool", "tool_call_id": "call_2", "content": "error"},
            {"role": "user", "content": "thanks"}
        ])
    );
}

#[tokio::test]
async fn system_text_is_merged_into_one_leading_message() {
    let server = serve(ok_server_reply()).await;
    let system = hoh_llm::create_system_message("Extra rule.");
    let body = sent(&server, |o| {
        o.system = Some("Base.".into());
        o.messages = vec![user("a"), system.into(), user("b")];
    })
    .await;
    assert_eq!(
        body["messages"][0],
        json!({"role": "system", "content": "Base.\n\nExtra rule."})
    );
    assert_eq!(body["messages"].as_array().unwrap().len(), 3);
}

#[tokio::test]
async fn invalid_histories_are_rejected_before_sending() {
    let server = serve(ok_server_reply()).await;
    let adapter = adapter(&server, |_| {});
    let cases: Vec<Vec<hoh_llm::RequestMessage>> = vec![
        // result without a call
        vec![user("a"), tool_result("x", "r")],
        // call without a result
        vec![user("a"), assistant(vec![call("c", "t", "{}")])],
        // user message between call and result
        vec![
            user("a"),
            assistant(vec![call("c", "t", "{}")]),
            user("b"),
            tool_result("c", "r"),
        ],
        // duplicate id
        vec![
            user("a"),
            assistant(vec![call("c", "t", "{}"), call("c", "t", "{}")]),
            tool_result("c", "r"),
        ],
    ];
    for messages in cases {
        let run = run(&adapter, options(messages)).await;
        assert_eq!(run.code(), "INVALID_REQUEST");
    }
    assert!(server.captured().is_empty(), "nothing may reach the server");
}

#[tokio::test]
async fn reasoning_efforts_per_server() {
    let server = serve(ok_server_reply()).await;
    let effort = |id: &'static str| {
        move |o: &mut hoh_llm::GenerateOptions| {
            o.reasoning_effort = Some(ReasoningEffortId::from(id))
        }
    };
    let body = sent_with(&server, |c| c.server = ServerKind::Vllm, effort("off")).await;
    assert_eq!(
        body["chat_template_kwargs"],
        json!({"enable_thinking": false})
    );
    let body = sent_with(&server, |c| c.server = ServerKind::LlamaCpp, effort("on")).await;
    assert_eq!(
        body["chat_template_kwargs"],
        json!({"enable_thinking": true})
    );
    let body = sent_with(&server, |c| c.server = ServerKind::Ollama, effort("off")).await;
    assert_eq!(body["reasoning_effort"], "none");
    let body = sent_with(&server, |c| c.server = ServerKind::Ollama, effort("high")).await;
    assert_eq!(body["reasoning_effort"], "high");
    let body = sent(&server, |_| {}).await;
    assert!(body.get("chat_template_kwargs").is_none() && body.get("reasoning_effort").is_none());

    // An unknown effort, or one for a server without a switch, is refused before sending.
    let before = server.captured().len();
    let run = run(&adapter(&server, |c| c.server = ServerKind::Generic), {
        let mut o = options(vec![user("x")]);
        o.reasoning_effort = Some("on".into());
        o
    })
    .await;
    assert_eq!(run.code(), "UNSUPPORTED_REASONING_EFFORT");
    assert_eq!(server.captured().len(), before);
}

#[tokio::test]
async fn model_info_reflects_config() {
    let server = serve(vec![]).await;
    let adapter = adapter(&server, |c| {
        c.server = ServerKind::Vllm;
        c.model.context_window = Some(32768);
        c.model.default_max_tokens = Some(4096);
        c.model.image_input = true;
        c.provider_name = "Local Qwen".into();
    });
    let info = adapter.resolve_model("local", "qwen", None).await.unwrap();
    assert_eq!(info.context.unwrap().context_window, 32768);
    assert_eq!(info.default_max_tokens, Some(4096));
    assert_eq!(info.model.input_modalities.unwrap(), ["text", "image"]);
    let efforts: Vec<_> = info
        .reasoning
        .unwrap()
        .efforts
        .into_iter()
        .map(|e| e.id.as_str().to_owned())
        .collect();
    assert_eq!(efforts, ["off", "on"]);
    assert_eq!(adapter.provider_info("local").unwrap().name, "Local Qwen");

    let plain = adapter_for_plain(&server).await;
    assert!(plain.reasoning.is_none());
    assert_eq!(plain.model.input_modalities.unwrap(), ["text"]);
}

async fn adapter_for_plain(server: &Server) -> hoh_llm::LlmResolvedModelInfo {
    adapter(server, |_| {})
        .resolve_model("local", "qwen", None)
        .await
        .unwrap()
}

#[tokio::test]
async fn model_list_comes_from_models_endpoint() {
    let server = serve(vec![(
        "GET /v1/models",
        vec![Reply::json(
            200,
            r#"{"object":"list","data":[{"id":"qwen3.6-35b-a3b"},{"id":"other"}]}"#,
        )],
    )])
    .await;
    let models = adapter(&server, |_| {}).list_models("local").await.unwrap();
    let ids: Vec<_> = models.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids, ["qwen3.6-35b-a3b", "other"]);
}

#[tokio::test]
async fn offloaded_images_become_text_and_other_images_need_a_store() {
    let server = serve(ok_server_reply()).await;
    let reference = serde_json::from_value(json!({
        "attachmentId": "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        "mediaType": "image/png", "bytes": 10, "width": 4, "height": 4, "name": "a.png"
    }))
    .unwrap();
    let offloaded: ContentBlock = ImageBlock {
        attachment: reference,
        offloaded: Some(true),
    }
    .into();
    let body = sent(&server, |o| {
        o.messages = vec![
            hoh_llm::create_user_message(hoh_llm::NewUserMessage {
                content: vec![ContentBlock::text("see"), offloaded],
                source: hoh_llm::MessageSource::new("user"),
            })
            .into(),
        ];
    })
    .await;
    let parts = body["messages"][0]["content"].as_array().unwrap();
    assert_eq!(parts[0], json!({"type": "text", "text": "see"}));
    assert!(
        parts[1]["text"]
            .as_str()
            .unwrap()
            .contains("image omitted to fit request image limits")
    );

    let live: ContentBlock = ImageBlock {
        attachment: serde_json::from_value(json!({
            "attachmentId": "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            "mediaType": "image/png", "bytes": 10, "width": 4, "height": 4
        })).unwrap(),
        offloaded: None,
    }.into();
    let run = run(
        &adapter(&server, |_| {}),
        options(vec![
            hoh_llm::create_user_message(hoh_llm::NewUserMessage {
                content: vec![live],
                source: hoh_llm::MessageSource::new("user"),
            })
            .into(),
        ]),
    )
    .await;
    assert_eq!(run.code(), "UNSUPPORTED_CONTENT");
}
