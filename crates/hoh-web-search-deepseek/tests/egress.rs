use hoh_error::HarnessError;
use hoh_kernel::Kernel;
use hoh_launch_environment::{
    LaunchEnvironment, LaunchEnvironmentLayerInput, LaunchEnvironmentSource,
    create_launch_environment_snapshot,
};
use hoh_web::{Web, WebRuntime, WebSearchProvider, WebSearchRequest};
use hoh_web_search_deepseek::{
    DEEPSEEK_PROVIDER_ID, DeepSeekSearchProvider, DeepSeekSearchProviderOptions, WebSearchDeepSeek,
};
use indexmap::IndexMap;
use std::sync::Arc;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

async fn read_http_request(stream: &mut TcpStream) -> String {
    let mut request = Vec::new();
    loop {
        let mut chunk = [0_u8; 4096];
        let length = stream.read(&mut chunk).await.unwrap();
        if length == 0 {
            break;
        }
        request.extend_from_slice(&chunk[..length]);
        let Some(header_end) = request.windows(4).position(|window| window == b"\r\n\r\n") else {
            continue;
        };
        let headers = String::from_utf8_lossy(&request[..header_end]);
        let content_length = headers
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().ok())
                    .flatten()
            })
            .unwrap_or(0);
        if request.len() >= header_end + 4 + content_length {
            break;
        }
    }
    String::from_utf8(request).unwrap()
}

#[tokio::test]
#[ignore = "requires loopback bind, which the writer sandbox forbids"]
async fn uses_the_launch_snapshot_proxy_for_search_egress() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy = format!("http://{}", listener.local_addr().unwrap());
    let proxy_task = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let request = read_http_request(&mut stream).await;
        let body = b"{\"content\":[{\"type\":\"web_search_tool_result\",\"content\":[{\"type\":\"web_search_result\",\"url\":\"https://result.test\"}]}]}";
        stream
            .write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    String::from_utf8_lossy(body)
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        request
    });

    let kernel = Kernel::start();
    let ctx = kernel.root();
    let snapshot = create_launch_environment_snapshot(&[LaunchEnvironmentLayerInput {
        source: LaunchEnvironmentSource::ProjectEnv,
        path: Some(".env".into()),
        values: IndexMap::from([("HTTP_PROXY".into(), proxy)]),
    }]);
    ctx.provide::<LaunchEnvironment>(Arc::new(snapshot))
        .await
        .unwrap();
    let web = ctx
        .plugin::<WebRuntime>(serde_json::json!({"searchProvider":DEEPSEEK_PROVIDER_ID}))
        .unwrap();
    web.settled().await.unwrap();
    let provider = ctx
        .plugin::<WebSearchDeepSeek>(serde_json::json!({
            "apiKey":"proxy-key",
            "baseURL":"http://deepseek.invalid/anthropic/v1"
        }))
        .unwrap();
    provider.settled().await.unwrap();
    let result = ctx
        .get::<Web>()
        .unwrap()
        .search(
            WebSearchRequest {
                query: "proxy query".into(),
                max_results: None,
            },
            None,
        )
        .await
        .unwrap();
    assert_eq!(result.sources[0].url, "https://result.test");
    let request = proxy_task.await.unwrap();
    assert!(request.starts_with("POST http://deepseek.invalid/anthropic/v1/messages HTTP/1.1"));
    assert!(
        request
            .to_ascii_lowercase()
            .contains("x-api-key: proxy-key")
    );
    let lower = request.to_ascii_lowercase();
    for header in [
        "authorization: bearer proxy-key",
        "anthropic-version: 2023-06-01",
        "content-type: application/json",
        "accept: application/json",
        "user-agent: deepseek-harness/0.0.1",
    ] {
        assert!(lower.contains(header), "missing header {header}");
    }
    let (_, body) = request.split_once("\r\n\r\n").unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(body).unwrap(),
        serde_json::json!({
            "model":"deepseek-v4-flash",
            "max_tokens":4096,
            "messages":[{"role":"user","content":[{
                "type":"text","text":"Perform a web search for the query: proxy query"
            }]}],
            "tools":[{"type":"web_search_20250305","name":"web_search","max_uses":5}],
            "thinking":{"type":"disabled"}
        })
    );
    kernel.shutdown().await;
}

#[tokio::test]
#[ignore = "requires loopback bind, which the writer sandbox forbids"]
async fn account_401_sends_only_the_account_token_and_rejects_the_sent_value() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let request = read_http_request(&mut stream).await;
        let body = r#"{"error":"invalid token"}"#;
        stream
            .write_all(
                format!(
                    "HTTP/1.1 401 Unauthorized\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        request
    });
    let rejected = Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured = rejected.clone();
    let provider = DeepSeekSearchProvider::new(move || DeepSeekSearchProviderOptions {
        resolve_account_token: Some(Arc::new(|_| {
            Box::pin(async { Ok(Some("account-token".into())) })
        })),
        reject_account_token: Some(Arc::new({
            let captured = captured.clone();
            move |token| {
                captured.lock().unwrap().push(token);
                Box::pin(async { Ok(()) })
            }
        })),
        api_key: Some("must-not-be-sent".into()),
        resolve_api_key: None,
        api_key_env: None,
        base_url: format!("http://{address}/anthropic/v1"),
        model: "deepseek-chat".into(),
        api_version: "2023-06-01".into(),
        max_tokens: 32,
        max_uses: 1,
        record_request: None,
    });
    let error = provider
        .search(
            WebSearchRequest {
                query: "q".into(),
                max_results: None,
            },
            None,
        )
        .await
        .unwrap_err();
    assert!(
        HarnessError::of(error.as_ref())
            .unwrap()
            .message()
            .contains("DeepSeek rejected the account sign-in")
    );
    assert_eq!(*rejected.lock().unwrap(), vec!["account-token"]);
    let request = server.await.unwrap().to_ascii_lowercase();
    assert!(request.contains("x-dsh-auth-token: account-token"));
    assert!(!request.contains("x-api-key:"));
    assert!(!request.contains("authorization:"));
}
