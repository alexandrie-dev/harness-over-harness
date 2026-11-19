use hoh_error::HarnessError;
use hoh_web::{WebSearchProvider, WebSearchRequest};
use hoh_web_search_deepseek::{DeepSeekSearchProvider, DeepSeekSearchProviderOptions};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
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

fn provider(base_url: String) -> DeepSeekSearchProvider {
    DeepSeekSearchProvider::new(move || DeepSeekSearchProviderOptions {
        resolve_account_token: None,
        reject_account_token: None,
        api_key: Some("redirect-test-key".into()),
        resolve_api_key: None,
        api_key_env: None,
        base_url: base_url.clone(),
        model: "deepseek-chat".into(),
        api_version: "2023-06-01".into(),
        max_tokens: 32,
        max_uses: 1,
        record_request: None,
    })
}

#[tokio::test]
#[ignore = "requires loopback bind, which the writer sandbox forbids"]
async fn rejects_redirect_statuses_before_contacting_location() {
    for status in [301, 302, 303, 307, 308] {
        let target = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target_address = target.local_addr().unwrap();
        let target_requests = Arc::new(AtomicUsize::new(0));
        let requests = target_requests.clone();
        let target_task = tokio::spawn(async move {
            if tokio::time::timeout(std::time::Duration::from_millis(200), target.accept())
                .await
                .is_ok()
            {
                requests.fetch_add(1, Ordering::SeqCst);
            }
        });

        let redirect = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let redirect_address = redirect.local_addr().unwrap();
        let redirect_task = tokio::spawn(async move {
            let (mut stream, _) = redirect.accept().await.unwrap();
            stream
                .write_all(
                    format!(
                        "HTTP/1.1 {status} Redirect\r\nLocation: http://{target_address}/collect\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
        });

        let error = provider(format!("http://{redirect_address}/{status}"))
            .search(
                WebSearchRequest {
                    query: "private redirect query".into(),
                    max_results: None,
                },
                None,
            )
            .await
            .unwrap_err();
        assert_eq!(
            HarnessError::of(error.as_ref()).unwrap().code(),
            "WEB_PROVIDER_ERROR"
        );
        redirect_task.await.unwrap();
        target_task.await.unwrap();
        assert_eq!(target_requests.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
#[ignore = "requires loopback bind, which the writer sandbox forbids"]
async fn default_redirect_following_forwards_custom_credential_and_post_body() {
    let target = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let target_address = target.local_addr().unwrap();
    let target_task = tokio::spawn(async move {
        let (mut stream, _) = target.accept().await.unwrap();
        let request = read_http_request(&mut stream).await;
        stream
            .write_all(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        request
    });

    let redirect = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let redirect_address = redirect.local_addr().unwrap();
    let redirect_task = tokio::spawn(async move {
        let (mut stream, _) = redirect.accept().await.unwrap();
        let mut request = [0_u8; 4096];
        let _ = stream.read(&mut request).await.unwrap();
        stream
            .write_all(
                format!(
                    "HTTP/1.1 307 Temporary Redirect\r\nLocation: http://{target_address}/collect\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                )
                .as_bytes(),
            )
            .await
            .unwrap();
    });

    let body = r#"{"query":"private redirect query"}"#;
    let response = reqwest::Client::new()
        .post(format!("http://{redirect_address}/307"))
        .header("x-api-key", "redirect-test-key")
        .header("authorization", "Bearer redirect-test-key")
        .header("content-type", "application/json")
        .body(body)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::NO_CONTENT);
    redirect_task.await.unwrap();
    let received = target_task.await.unwrap();
    assert!(received.starts_with("POST /collect HTTP/1.1"));
    assert!(
        received
            .to_ascii_lowercase()
            .contains("x-api-key: redirect-test-key")
    );
    assert!(received.ends_with(body));
}
