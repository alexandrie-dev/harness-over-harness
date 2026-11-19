use hoh_error::{BoxError, HarnessError};
use hoh_timeout::AbortController;
use hoh_web::{WebSearchProvider, WebSearchRequest};
use hoh_web_search_deepseek::{
    DEEPSEEK_PROVIDER_ID, DeepSeekSearchLlmRequest, DeepSeekSearchProvider,
    DeepSeekSearchProviderOptions, citation_snippets, map_anthropic_response,
    types::AnthropicResponse,
};
use serde_json::json;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

fn options() -> DeepSeekSearchProviderOptions {
    DeepSeekSearchProviderOptions {
        resolve_account_token: None,
        reject_account_token: None,
        api_key: Some("ds-key".into()),
        resolve_api_key: None,
        api_key_env: None,
        base_url: "https://api.deepseek.test/anthropic/v1".into(),
        model: "deepseek-chat".into(),
        api_version: "2023-06-01".into(),
        max_tokens: 4096,
        max_uses: 5,
        record_request: None,
    }
}

fn provider(options: DeepSeekSearchProviderOptions) -> DeepSeekSearchProvider {
    DeepSeekSearchProvider::new(move || options.clone())
}

fn code(error: &BoxError) -> &str {
    HarnessError::of(error.as_ref())
        .expect("HarnessError")
        .code()
}

#[test]
fn citation_snippets_maps_first_non_empty_excerpt_per_url() {
    let blocks = vec![
        json!({"type":"text","citations":[
            {"url":"https://a.test","cited_text":"first"},
            {"url":"https://a.test","cited_text":"second"},
            {"url":"","cited_text":"empty"},
            {"cited_text":"orphan"}
        ]}),
        json!({"type":"text","citations":[{"url":"https://b.test","cited_text":"b text"}]}),
    ];
    let snippets = citation_snippets(&blocks).unwrap();
    assert_eq!(
        snippets.get("https://a.test").map(String::as_str),
        Some("first")
    );
    assert_eq!(
        snippets.get("https://b.test").map(String::as_str),
        Some("b text")
    );
    assert_eq!(snippets.len(), 2);
}

#[test]
fn maps_structured_blocks_citations_optional_fields_and_page_age() {
    let result = map_anthropic_response(AnthropicResponse {
        content: Some(vec![
            json!({"type":"text","text":"prose","citations":[{"url":"https://a.test","cited_text":"excerpt"}]}),
            json!({"type":"web_search_tool_result","content":[
                {"type":"web_search_result","url":"https://a.test","title":"A","page_age":"2026-02-02"},
                {"type":"web_search_result","url":"https://b.test","title":"","page_age":""}
            ]}),
        ]),
    })
    .unwrap();
    assert_eq!(result.content, None);
    assert!(!result.truncated);
    assert_eq!(result.sources[0].url, "https://a.test");
    assert_eq!(result.sources[0].title.as_deref(), Some("A"));
    assert_eq!(result.sources[0].snippet.as_deref(), Some("excerpt"));
    assert_eq!(
        result.sources[0].published_at.as_deref(),
        Some("2026-02-02")
    );
    assert_eq!(result.sources[1].title, None);
    assert_eq!(result.sources[1].published_at, None);
}

#[test]
fn dedupes_first_url_and_skips_non_results_and_empty_urls() {
    let result = map_anthropic_response(AnthropicResponse {
        content: Some(vec![
            json!({"type":"web_search_tool_result","content":[
                {"type":"web_search_result_error","url":"https://error.test"},
                {"type":"web_search_result","url":""},
                {"type":"web_search_result","url":"https://a.test","title":"first"}
            ]}),
            json!({"type":"web_search_tool_result","content":[
                {"type":"web_search_result","url":"https://a.test","title":"second"}
            ]}),
        ]),
    })
    .unwrap();
    assert_eq!(result.sources.len(), 1);
    assert_eq!(result.sources[0].title.as_deref(), Some("first"));
}

#[test]
fn accepts_empty_result_block_but_rejects_absent_or_malformed_result_blocks() {
    assert!(
        map_anthropic_response(AnthropicResponse {
            content: Some(vec![
                json!({"type":"web_search_tool_result","content":null}),
                json!({"type":"web_search_tool_result"})
            ]),
        })
        .unwrap()
        .sources
        .is_empty()
    );
    for response in [
        AnthropicResponse { content: None },
        AnthropicResponse {
            content: Some(vec![json!({"type":"text","text":"prose"})]),
        },
    ] {
        let error = map_anthropic_response(response).unwrap_err();
        assert_eq!(code(&error), "WEB_PROVIDER_ERROR");
    }
    let error = map_anthropic_response(AnthropicResponse {
        content: Some(vec![json!({"type":"web_search_tool_result","content":{}})]),
    })
    .unwrap_err();
    assert!(HarnessError::of(error.as_ref()).is_none());

    let result = map_anthropic_response(AnthropicResponse {
        content: Some(vec![json!({
            "type":"web_search_tool_result",
            "content":"abc"
        })]),
    })
    .unwrap();
    assert!(result.sources.is_empty());

    let error = map_anthropic_response(AnthropicResponse {
        content: Some(vec![json!({
            "type":"web_search_tool_result",
            "content":[null]
        })]),
    })
    .unwrap_err();
    assert!(HarnessError::of(error.as_ref()).is_none());

    for content in [
        vec![json!(null), json!({"type":"web_search_tool_result"})],
        vec![
            json!({"type":"text","citations":[null]}),
            json!({"type":"web_search_tool_result"}),
        ],
    ] {
        let error = map_anthropic_response(AnthropicResponse {
            content: Some(content),
        })
        .unwrap_err();
        assert!(HarnessError::of(error.as_ref()).is_none());
        assert!(
            error
                .to_string()
                .starts_with("TypeError: Cannot read properties of null")
        );
    }
}

#[test]
fn availability_requires_auth_parseable_url_and_positive_limits() {
    assert!(provider(options()).available().unwrap());
    let mut value = options();
    value.api_key = Some(String::new());
    assert!(!provider(value).available().unwrap());
    let mut value = options();
    value.api_key = None;
    value.resolve_account_token = Some(Arc::new(|_| Box::pin(async { Ok(None) })));
    assert!(provider(value).available().unwrap());
    for mutate in [0_u8, 1, 2] {
        let mut value = options();
        match mutate {
            0 => value.base_url = "not a url".into(),
            1 => value.max_tokens = 0,
            _ => value.max_uses = 0,
        }
        assert!(!provider(value).available().unwrap());
    }
}

#[test]
fn provider_metadata_matches_picker_contract() {
    let info = provider(options()).info();
    assert_eq!(info.id, DEEPSEEK_PROVIDER_ID);
    assert_eq!(info.title, None);
    let text = serde_json::to_value(info.description).unwrap();
    assert_eq!(
        text["en"],
        "Slower, and every search uses extra model tokens. Requires a DeepSeek account or API key."
    );
    assert_eq!(
        text["zh"],
        "速度较慢，每次搜索都会额外消耗模型 token。需要 DeepSeek 账号或 API Key。"
    );
}

#[tokio::test]
async fn missing_key_uses_default_reference_and_never_dispatches() {
    let mut value = options();
    value.api_key = None;
    let error = provider(value)
        .search(
            WebSearchRequest {
                query: "q".into(),
                max_results: None,
            },
            None,
        )
        .await
        .unwrap_err();
    let error = HarnessError::of(error.as_ref()).unwrap();
    assert_eq!(error.code(), "WEB_PROVIDER_CREDENTIAL_MISSING");
    assert!(error.message().contains("\"DEEPSEEK_API_KEY\""));
}

#[tokio::test]
async fn account_token_precedes_api_key_resolution() {
    let account_calls = Arc::new(AtomicUsize::new(0));
    let api_calls = Arc::new(AtomicUsize::new(0));
    let mut value = options();
    value.api_key = None;
    let calls = account_calls.clone();
    value.resolve_account_token = Some(Arc::new(move |endpoint| {
        calls.fetch_add(1, Ordering::SeqCst);
        assert_eq!(endpoint, "https://api.deepseek.test/anthropic/v1/messages");
        Box::pin(async { Ok(Some("account-token".into())) })
    }));
    let calls = api_calls.clone();
    value.resolve_api_key = Some(Arc::new(move || {
        calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Ok(Some("api-key".into())) })
    }));
    value.record_request = Some(Arc::new(|_| {
        Err(std::io::Error::other("stop before dispatch").into())
    }));
    assert!(
        provider(value)
            .search(
                WebSearchRequest {
                    query: "q".into(),
                    max_results: None
                },
                None
            )
            .await
            .is_err()
    );
    assert_eq!(account_calls.load(Ordering::SeqCst), 1);
    assert_eq!(api_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn unreadable_account_grant_is_terminal_before_api_key_fallback() {
    let api_calls = Arc::new(AtomicUsize::new(0));
    let recorded = Arc::new(AtomicUsize::new(0));
    let mut value = options();
    value.resolve_account_token = Some(Arc::new(|_| {
        Box::pin(async { Err(std::io::Error::other("account storage unreadable").into()) })
    }));
    let calls = api_calls.clone();
    value.resolve_api_key = Some(Arc::new(move || {
        calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Ok(Some("fallback-key".into())) })
    }));
    let calls = recorded.clone();
    value.record_request = Some(Arc::new(move |_| {
        calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }));
    let error = provider(value)
        .search(
            WebSearchRequest {
                query: "q".into(),
                max_results: None,
            },
            None,
        )
        .await
        .unwrap_err();
    let error = HarnessError::of(error.as_ref()).unwrap();
    assert_eq!(error.code(), "WEB_PROVIDER_ERROR");
    assert!(error.message().contains("account storage unreadable"));
    assert_eq!(api_calls.load(Ordering::SeqCst), 0);
    assert_eq!(recorded.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn absent_account_token_falls_back_to_resolved_api_key() {
    let api_calls = Arc::new(AtomicUsize::new(0));
    let mut value = options();
    value.api_key = None;
    value.resolve_account_token = Some(Arc::new(|_| Box::pin(async { Ok(None) })));
    let calls = api_calls.clone();
    value.resolve_api_key = Some(Arc::new(move || {
        calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Ok(Some("api-key".into())) })
    }));
    value.record_request = Some(Arc::new(|_| {
        Err(std::io::Error::other("stop before dispatch").into())
    }));
    assert!(
        provider(value)
            .search(
                WebSearchRequest {
                    query: "q".into(),
                    max_results: None
                },
                None
            )
            .await
            .is_err()
    );
    assert_eq!(api_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn api_key_is_resolved_again_for_every_search() {
    let calls = Arc::new(AtomicUsize::new(0));
    let mut value = options();
    value.api_key = None;
    let count = calls.clone();
    value.resolve_api_key = Some(Arc::new(move || {
        let revision = count.fetch_add(1, Ordering::SeqCst) + 1;
        Box::pin(async move { Ok(Some(format!("key-{revision}"))) })
    }));
    value.record_request = Some(Arc::new(|_| {
        Err(std::io::Error::other("stop before dispatch").into())
    }));
    let search = provider(value);
    for query in ["first", "second"] {
        assert!(
            search
                .search(
                    WebSearchRequest {
                        query: query.into(),
                        max_results: None,
                    },
                    None,
                )
                .await
                .is_err()
        );
    }
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn resolver_failure_is_provider_error_and_record_is_not_started() {
    let recorded = Arc::new(AtomicUsize::new(0));
    let mut value = options();
    value.api_key = None;
    value.resolve_api_key = Some(Arc::new(|| {
        Box::pin(async { Err(std::io::Error::other("credential backend failed").into()) })
    }));
    let count = recorded.clone();
    value.record_request = Some(Arc::new(move |_| {
        count.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }));
    let error = provider(value)
        .search(
            WebSearchRequest {
                query: "q".into(),
                max_results: None,
            },
            None,
        )
        .await
        .unwrap_err();
    let error = HarnessError::of(error.as_ref()).unwrap();
    assert_eq!(error.code(), "WEB_PROVIDER_ERROR");
    assert_eq!(
        error.message(),
        "DeepSeek search credential resolution failed: Error: credential backend failed"
    );
    assert_eq!(recorded.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn pre_aborted_search_starts_no_resolver_or_record() {
    let resolved = Arc::new(AtomicUsize::new(0));
    let recorded = Arc::new(AtomicUsize::new(0));
    let mut value = options();
    value.api_key = None;
    let calls = resolved.clone();
    value.resolve_api_key = Some(Arc::new(move || {
        calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Ok(Some("key".into())) })
    }));
    let calls = recorded.clone();
    value.record_request = Some(Arc::new(move |_| {
        calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }));
    let controller = AbortController::new();
    controller.abort_with(std::io::Error::other("caller stopped"));
    let error = provider(value)
        .search(
            WebSearchRequest {
                query: "q".into(),
                max_results: None,
            },
            Some(&controller.signal()),
        )
        .await
        .unwrap_err();
    assert_eq!(code(&error), "WEB_ABORTED");
    assert_eq!(resolved.load(Ordering::SeqCst), 0);
    assert_eq!(recorded.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn pre_aborted_search_still_takes_the_source_options_snapshot() {
    let snapshots = Arc::new(AtomicUsize::new(0));
    let calls = snapshots.clone();
    let search = DeepSeekSearchProvider::new(move || {
        calls.fetch_add(1, Ordering::SeqCst);
        options()
    });
    let controller = AbortController::new();
    controller.abort_with(std::io::Error::other("caller stopped"));
    let error = search
        .search(
            WebSearchRequest {
                query: "q".into(),
                max_results: None,
            },
            Some(&controller.signal()),
        )
        .await
        .unwrap_err();
    assert_eq!(code(&error), "WEB_ABORTED");
    assert_eq!(snapshots.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn cancellation_interrupts_an_uncooperative_resolver() {
    let mut value = options();
    value.api_key = None;
    value.resolve_api_key = Some(Arc::new(|| {
        Box::pin(async {
            futures_util::future::pending::<Result<Option<String>, BoxError>>().await
        })
    }));
    let controller = AbortController::new();
    let signal = controller.signal();
    let search = provider(value);
    let operation = search.search(
        WebSearchRequest {
            query: "q".into(),
            max_results: None,
        },
        Some(&signal),
    );
    controller.abort_with(std::io::Error::other("deadline"));
    let error = operation.await.unwrap_err();
    assert_eq!(code(&error), "WEB_ABORTED");
}

#[tokio::test]
async fn cancelled_credential_resolution_remains_observed_until_completion() {
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let (completed_tx, completed_rx) = tokio::sync::oneshot::channel();
    let started_tx = Arc::new(Mutex::new(Some(started_tx)));
    let release_rx = Arc::new(Mutex::new(Some(release_rx)));
    let completed_tx = Arc::new(Mutex::new(Some(completed_tx)));
    let mut value = options();
    value.api_key = None;
    value.resolve_api_key = Some(Arc::new(move || {
        let started = started_tx.lock().unwrap().take();
        let release = release_rx.lock().unwrap().take();
        let completed = completed_tx.lock().unwrap().take();
        Box::pin(async move {
            if let Some(started) = started {
                let _ = started.send(());
            }
            if let Some(release) = release {
                let _ = release.await;
            }
            if let Some(completed) = completed {
                let _ = completed.send(());
            }
            Ok(Some("late-key".into()))
        })
    }));
    let controller = AbortController::new();
    let signal = controller.signal();
    let running = tokio::spawn(async move {
        provider(value)
            .search(
                WebSearchRequest {
                    query: "q".into(),
                    max_results: None,
                },
                Some(&signal),
            )
            .await
    });
    started_rx.await.unwrap();
    controller.abort_with(std::io::Error::other("caller stopped"));
    let error = running.await.unwrap().unwrap_err();
    assert_eq!(code(&error), "WEB_ABORTED");
    release_tx.send(()).unwrap();
    completed_rx.await.unwrap();
}

#[tokio::test]
async fn records_exact_secret_free_request_before_dispatch() {
    let recorded = Arc::new(Mutex::new(None::<DeepSeekSearchLlmRequest>));
    let mut value = options();
    let output = recorded.clone();
    value.record_request = Some(Arc::new(move |request| {
        *output.lock().unwrap() = Some(request);
        Err(std::io::Error::other("stop before dispatch").into())
    }));
    assert!(
        provider(value)
            .search(
                WebSearchRequest {
                    query: "hello".into(),
                    max_results: Some(1)
                },
                None
            )
            .await
            .is_err()
    );
    let request = recorded.lock().unwrap().clone().unwrap();
    assert_eq!(
        request.endpoint,
        "https://api.deepseek.test/anthropic/v1/messages"
    );
    assert_eq!(request.api_version, "2023-06-01");
    assert_eq!(request.body.model, "deepseek-chat");
    assert_eq!(request.body.max_tokens, 4096);
    assert_eq!(
        request.body.messages[0].content[0].text,
        "Perform a web search for the query: hello"
    );
    assert_eq!(request.body.tools[0].r#type, "web_search_20250305");
    assert_eq!(request.body.tools[0].name, "web_search");
    assert_eq!(request.body.tools[0].max_uses, 5);
    assert_eq!(request.body.thinking.unwrap().r#type, "disabled");
}

#[tokio::test]
async fn invalid_header_is_wrapped_after_the_request_is_recorded() {
    let recorded = Arc::new(AtomicUsize::new(0));
    let mut value = options();
    value.api_version = "invalid\nversion".into();
    let calls = recorded.clone();
    value.record_request = Some(Arc::new(move |_| {
        calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }));
    let error = provider(value)
        .search(
            WebSearchRequest {
                query: "q".into(),
                max_results: None,
            },
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(code(&error), "WEB_PROVIDER_ERROR");
    assert!(
        HarnessError::of(error.as_ref())
            .unwrap()
            .message()
            .contains("Search endpoint configuration is separate from chat.")
    );
    assert_eq!(recorded.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn one_search_uses_one_options_snapshot() {
    let current = Arc::new(Mutex::new((
        "https://before.test/v1".to_owned(),
        "before".to_owned(),
    )));
    let recorded = Arc::new(Mutex::new(None::<DeepSeekSearchLlmRequest>));
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let started_tx = Arc::new(Mutex::new(Some(started_tx)));
    let release_rx = Arc::new(Mutex::new(Some(release_rx)));
    let current_for_options = current.clone();
    let recorded_for_options = recorded.clone();
    let provider = Arc::new(DeepSeekSearchProvider::new(move || {
        let (base_url, model) = current_for_options.lock().unwrap().clone();
        let output = recorded_for_options.clone();
        let started = started_tx.clone();
        let release = release_rx.clone();
        DeepSeekSearchProviderOptions {
            api_key: None,
            resolve_api_key: Some(Arc::new(move || {
                let started = started.lock().unwrap().take();
                let release = release.lock().unwrap().take();
                Box::pin(async move {
                    if let Some(started) = started {
                        let _ = started.send(());
                    }
                    if let Some(release) = release {
                        let _ = release.await;
                    }
                    Ok(Some("key-from-before".into()))
                })
            })),
            base_url,
            model,
            record_request: Some(Arc::new(move |request| {
                *output.lock().unwrap() = Some(request);
                Err(std::io::Error::other("stop before dispatch").into())
            })),
            ..options()
        }
    }));
    let running = provider.clone();
    let search = tokio::spawn(async move {
        running
            .search(
                WebSearchRequest {
                    query: "q".into(),
                    max_results: None,
                },
                None,
            )
            .await
    });
    started_rx.await.unwrap();
    *current.lock().unwrap() = ("https://after.test/v1".into(), "after".into());
    release_tx.send(()).unwrap();
    assert!(search.await.unwrap().is_err());
    let request = recorded.lock().unwrap().clone().unwrap();
    assert_eq!(request.endpoint, "https://before.test/v1/messages");
    assert_eq!(request.body.model, "before");
}
