use hoh_llm_openai_mock::{
    MockOptions, MockServer, RequestRecord, Scenario, ScenarioResponse, ScenarioToolCall,
    ScenarioUsage,
};
use serde_json::{Value, json};

fn post_json(client: &reqwest::Client, url: &str, body: Value) -> reqwest::RequestBuilder {
    client
        .post(url)
        .header("content-type", "application/json")
        .body(body.to_string())
}

fn scenario() -> Scenario {
    Scenario {
        model: "window-mock".to_owned(),
        responses: vec![
            ScenarioResponse {
                text: vec!["Готовлю ".to_owned(), "ответ.".to_owned()],
                tool_calls: vec![],
                usage: ScenarioUsage {
                    prompt_tokens: 4,
                    completion_tokens: 2,
                },
            },
            ScenarioResponse {
                text: vec!["Запускаю проверку.".to_owned()],
                tool_calls: vec![ScenarioToolCall {
                    id: "call-bash".to_owned(),
                    name: "bash".to_owned(),
                    arguments: json!({"command":"pwd"}),
                }],
                usage: ScenarioUsage {
                    prompt_tokens: 7,
                    completion_tokens: 5,
                },
            },
        ],
    }
}

/// `error.message` of a JSON error response.
async fn error_message(response: reqwest::Response) -> String {
    let text = response.text().await.expect("error body");
    let value: Value = serde_json::from_str(&text).expect("JSON error body");
    value["error"]["message"]
        .as_str()
        .expect("error message")
        .to_owned()
}

/// A failed bind is a failed test: a run that skipped its checks must not look green.
async fn start(options: MockOptions) -> MockServer {
    MockServer::start(options).await.expect("start mock")
}

#[tokio::test]
async fn models_and_probe_do_not_consume_stream_scenario() {
    let mut server = start(MockOptions::new(scenario())).await;
    let client = reqwest::Client::new();
    let models = client
        .get(format!("{}/models", server.base_url))
        .send()
        .await
        .expect("models request")
        .text()
        .await
        .expect("models body");
    let models: Value = serde_json::from_str(&models).expect("models JSON");
    assert_eq!(models["data"][0]["id"], "window-mock");

    let probe = post_json(
        &client,
        &format!("{}/chat/completions", server.base_url),
        json!({"model":"window-mock","stream":false,"messages":[]}),
    )
    .send()
    .await
    .expect("probe request")
    .text()
    .await
    .expect("probe body");
    let probe: Value = serde_json::from_str(&probe).expect("probe JSON");
    assert_eq!(probe["choices"][0]["message"]["content"], "pong");

    let text = post_json(
        &client,
        &format!("{}/chat/completions", server.base_url),
        json!({"model":"window-mock","stream":true,"messages":[]}),
    )
    .send()
    .await
    .expect("stream request")
    .text()
    .await
    .expect("stream text");
    assert!(text.contains("Готовлю "));
    assert!(text.contains("ответ."));
    assert!(text.contains("\"finish_reason\":\"stop\""));
    assert!(text.contains("\"prompt_tokens\":4"));
    assert!(text.ends_with("data: [DONE]\n\n"));
    server.close().await;
}

#[tokio::test]
async fn emits_tool_calls_and_returns_conflict_when_exhausted() {
    let one = Scenario {
        model: "window-mock".to_owned(),
        responses: vec![scenario().responses.remove(1)],
    };
    let mut server = start(MockOptions::new(one)).await;
    let client = reqwest::Client::new();
    let endpoint = format!("{}/chat/completions", server.base_url);
    let first = post_json(&client, &endpoint, json!({"stream":true}))
        .send()
        .await
        .expect("first response")
        .text()
        .await
        .expect("first body");
    assert!(first.contains("\"name\":\"bash\""));
    assert!(first.contains(r#"{\"command\":\"pwd\"}"#));
    assert!(first.contains("\"finish_reason\":\"tool_calls\""));

    let exhausted = post_json(&client, &endpoint, json!({"stream":true}))
        .send()
        .await
        .expect("exhausted response");
    assert_eq!(exhausted.status(), reqwest::StatusCode::CONFLICT);
    server.close().await;
}

#[tokio::test]
async fn journal_is_jsonl_and_omits_authorization() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let journal = directory.path().join("requests.jsonl");
    let mut options = MockOptions::new(scenario());
    options.journal_path = Some(journal.clone());
    options.api_key = Some("do-not-record".to_owned());
    let mut server = start(options).await;
    let client = reqwest::Client::new();
    client
        .get(format!("{}/models", server.base_url))
        .bearer_auth("do-not-record")
        .send()
        .await
        .expect("models request");
    client
        .get(format!("{}/models", server.base_url))
        .bearer_auth("wrong")
        .send()
        .await
        .expect("models request with another key");
    let records = server.requests().await;
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].path, "/v1/models");
    assert!(records[0].authorized);
    assert!(!records[1].authorized);
    server.close().await;

    let file = std::fs::read_to_string(journal).expect("journal file");
    assert!(!file.contains("do-not-record"));
    let written = file
        .lines()
        .map(|line| serde_json::from_str::<RequestRecord>(line).expect("journal record"))
        .collect::<Vec<_>>();
    assert_eq!(written, records);
}

#[test]
fn loads_required_window_actions_from_file() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("scenario.json");
    std::fs::write(
        &path,
        serde_json::to_vec(&json!({
            "model":"window-mock",
            "responses":[
                {"text":["поток ","текста"],"usage":{"prompt_tokens":1,"completion_tokens":2}},
                {"tool_calls":[{"id":"b","name":"bash","arguments":{"command":"pwd"}}]},
                {"tool_calls":[{"id":"r","name":"read","arguments":{"path":"note.txt"}}]},
                {"tool_calls":[{"id":"w","name":"write","arguments":{"path":"note.txt","content":"ok"}}]},
                {"tool_calls":[{"id":"d","name":"bash","arguments":{"command":"rm -rf ./generated"}}]}
            ]
        }))
        .expect("serialize scenario"),
    )
    .expect("write scenario");
    let scenario = Scenario::from_path(path).expect("load scenario");
    let names = scenario
        .responses
        .iter()
        .flat_map(|response| response.tool_calls.iter().map(|call| call.name.as_str()))
        .collect::<Vec<_>>();
    assert_eq!(names, ["bash", "read", "write", "bash"]);
}

#[tokio::test]
async fn configured_delay_comes_after_the_opening_chunk_and_before_the_text() {
    let mut options = MockOptions::new(scenario());
    options.delays_ms.insert(0, 400);
    let mut server = start(options).await;
    let client = reqwest::Client::new();
    let url = format!("{}/chat/completions", server.base_url);
    let started = std::time::Instant::now();
    let mut response = post_json(&client, &url, json!({"stream":true,"messages":[]}))
        .send()
        .await
        .expect("delayed completion");
    let first = response
        .chunk()
        .await
        .expect("first chunk")
        .expect("opening chunk");
    let first = String::from_utf8_lossy(&first).into_owned();
    assert!(first.contains("\"role\":\"assistant\""), "{first}");
    assert!(
        !first.contains("Готовлю "),
        "the text must come after the pause: {first}"
    );
    // The pause sits between the opening chunk and the text: the opening chunk is already here,
    // and the text arrives no sooner than the delay after it.
    let opening_at = std::time::Instant::now();
    let mut rest = String::new();
    let mut text_at = None;
    while let Some(chunk) = response.chunk().await.expect("next chunk") {
        rest.push_str(&String::from_utf8_lossy(&chunk));
        if text_at.is_none() && rest.contains("Готовлю ") {
            text_at = Some(std::time::Instant::now());
        }
    }
    let text_at = text_at.unwrap_or_else(|| panic!("the text never arrived: {rest}"));
    assert!(
        text_at.duration_since(opening_at) >= std::time::Duration::from_millis(300),
        "the text came {:?} after the opening chunk",
        text_at.duration_since(opening_at)
    );
    assert!(started.elapsed() >= std::time::Duration::from_millis(400));

    // An index without a delay answers at once, and the second request still sees the scripted
    // second response.
    let body = post_json(&client, &url, json!({"stream":true,"messages":[]}))
        .send()
        .await
        .expect("undelayed completion")
        .text()
        .await
        .expect("undelayed body");
    assert!(body.contains("Запускаю проверку."), "{body}");
    server.close().await;
}

#[tokio::test]
async fn unknown_paths_and_bad_bodies_get_json_errors_and_are_still_journalled() {
    let mut server = start(MockOptions::new(scenario())).await;
    let client = reqwest::Client::new();
    let url = format!("{}/chat/completions", server.base_url);

    let missing = client
        .get(format!("http://127.0.0.1:{}/v1/nothing", server.port))
        .send()
        .await
        .expect("unknown path");
    assert_eq!(missing.status(), 404);
    assert_eq!(error_message(missing).await, "route not found");

    let empty = client.post(&url).send().await.expect("no body");
    assert_eq!(empty.status(), 400);
    assert_eq!(error_message(empty).await, "missing JSON body");

    let garbage = client
        .post(&url)
        .header("content-type", "application/json")
        .body("{not json")
        .send()
        .await
        .expect("garbage body");
    assert_eq!(garbage.status(), 400);
    let message = error_message(garbage).await;
    assert!(message.starts_with("request body is not JSON"), "{message}");

    // Only the first two are journalled: a body that cannot be parsed never becomes a request.
    let records = server.requests().await;
    assert_eq!(
        records
            .iter()
            .map(|record| (
                record.sequence,
                record.method.as_str(),
                record.path.as_str()
            ))
            .collect::<Vec<_>>(),
        [
            (1, "GET", "/v1/nothing"),
            (2, "POST", "/v1/chat/completions")
        ]
    );
    server.close().await;
}

#[tokio::test]
async fn the_journal_keeps_sequence_and_the_request_body() {
    let mut server = start(MockOptions::new(scenario())).await;
    let client = reqwest::Client::new();
    let url = format!("{}/chat/completions", server.base_url);
    let sent =
        json!({"model":"window-mock","stream":false,"messages":[{"role":"user","content":"ping"}]});
    post_json(&client, &url, sent.clone())
        .send()
        .await
        .expect("probe");
    client
        .get(format!("{}/models", server.base_url))
        .send()
        .await
        .expect("models");
    let records = server.requests().await;
    assert_eq!(records.len(), 2);
    assert_eq!((records[0].sequence, records[1].sequence), (1, 2));
    assert_eq!(records[0].body.as_ref(), Some(&sent));
    assert_eq!(records[1].body, None);
    server.close().await;
}

#[tokio::test]
async fn malformed_tool_arguments_can_be_scripted_as_a_string() {
    let mut options = MockOptions::new(Scenario {
        model: "window-mock".to_owned(),
        responses: vec![ScenarioResponse {
            text: vec![],
            tool_calls: vec![ScenarioToolCall {
                id: "bad".to_owned(),
                name: "bash".to_owned(),
                arguments: json!("{\"command\": "),
            }],
            usage: ScenarioUsage::default(),
        }],
    });
    options.port = 0;
    let mut server = start(options).await;
    let body = post_json(
        &reqwest::Client::new(),
        &format!("{}/chat/completions", server.base_url),
        json!({"stream":true,"messages":[]}),
    )
    .send()
    .await
    .expect("completion")
    .text()
    .await
    .expect("body");
    let call = body
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .filter_map(|data| serde_json::from_str::<Value>(data).ok())
        .find_map(|chunk| {
            chunk["choices"][0]["delta"]["tool_calls"][0]
                .as_object()
                .cloned()
        })
        .expect("a tool call chunk");
    assert_eq!(call["function"]["arguments"], "{\"command\": ");
    server.close().await;
}

#[tokio::test]
async fn an_ipv6_host_is_advertised_in_brackets() {
    let mut options = MockOptions::new(scenario());
    options.host = "::1".to_owned();
    let mut server = start(options).await;
    assert!(
        server.base_url.starts_with("http://[::1]:"),
        "{}",
        server.base_url
    );
    let models = reqwest::get(format!("{}/models", server.base_url))
        .await
        .expect("models over IPv6");
    assert_eq!(models.status(), 200);
    server.close().await;
}
