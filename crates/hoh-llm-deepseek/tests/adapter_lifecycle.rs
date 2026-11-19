use futures_util::{StreamExt, future::BoxFuture};
use hoh_error::BoxError;
use hoh_llm::{BuiltinLlmFailureCode as Code, GenerateOptions, LlmAdapter, LlmError, LlmModelInfo};
use hoh_llm_deepseek::{
    DeepSeekAdapter, DeepSeekCatalogModel, DeepSeekConnectionOptions, DeepSeekRequestAuth, Options,
    resolve_adapter_options,
};
use hoh_timeout::{AbortController, AbortSignal};
use std::sync::{Arc, Mutex};

fn connection(base_url: &str, max_tokens: u64) -> DeepSeekConnectionOptions {
    resolve_adapter_options(&Options {
        base_url: Some(base_url.into()),
        max_tokens: Some(max_tokens),
        models: Some(vec![DeepSeekCatalogModel {
            id: "remote".into(),
            context_window: Some(max_tokens * 10),
            ..Default::default()
        }]),
        ..Default::default()
    })
    .unwrap()
}

fn error(message: &str, code: Code) -> BoxError {
    LlmError::new(message, code)
        .expect("test diagnostic is valid")
        .into()
}

#[tokio::test]
async fn discovery_is_provider_owned_and_omission_exposes_no_entries() {
    let adapter = DeepSeekAdapter::new(connection("https://wire.example", 10)).unwrap();
    assert!(adapter.list_models("route").await.unwrap().is_empty());

    let adapter = adapter.with_model_discovery(Arc::new(|provider| {
        Box::pin(async move {
            Ok(vec![LlmModelInfo {
                provider,
                id: "listed".into(),
                name: "Listed remotely".into(),
                ..Default::default()
            }])
        })
    }));
    let models = adapter.list_models("route").await.unwrap();
    assert_eq!(models.len(), 1);
    assert_eq!(models[0].provider, "route");
    assert_eq!(models[0].id, "listed");
}

#[test]
fn sync_operations_read_the_current_options_once_per_operation() {
    let current = Arc::new(Mutex::new(connection("https://first.example", 10)));
    let calls = Arc::new(Mutex::new(0_u64));
    let adapter = DeepSeekAdapter::new(connection("https://unused.example", 1))
        .unwrap()
        .with_options_resolver(Arc::new({
            let current = current.clone();
            let calls = calls.clone();
            move || {
                *calls.lock().unwrap() += 1;
                Ok(current.lock().unwrap().clone())
            }
        }));

    let first = adapter.provider_retry_policy("route").unwrap().unwrap();
    *current.lock().unwrap() = connection("https://second.example", 20);
    let second = adapter.provider_retry_policy("route").unwrap().unwrap();

    assert_eq!(*calls.lock().unwrap(), 2);
    assert_eq!(first, connection("https://first.example", 10).retry_policy);
    assert_eq!(
        second,
        connection("https://second.example", 20).retry_policy
    );
}

#[tokio::test]
async fn async_connection_receives_signal_and_prepared_call_freezes_snapshot() {
    let current = Arc::new(Mutex::new(connection("https://first.example", 10)));
    let signals = Arc::new(Mutex::new(Vec::new()));
    let connect = Arc::new({
        let current = current.clone();
        let signals = signals.clone();
        move |signal: Option<AbortSignal>| -> BoxFuture<'static, Result<DeepSeekConnectionOptions, BoxError>> {
            signals.lock().unwrap().push(signal.is_some());
            let snapshot = current.lock().unwrap().clone();
            Box::pin(async move { Ok(snapshot) })
        }
    });
    let adapter = Arc::new(
        DeepSeekAdapter::new(connection("https://unused.example", 1))
            .unwrap()
            .with_connection_resolver(connect),
    );
    let controller = AbortController::new();
    let prepared = adapter
        .clone()
        .prepare_call("route".into(), "remote".into(), Some(controller.signal()))
        .await
        .unwrap();

    *current.lock().unwrap() = connection("https://second.example", 20);
    assert_eq!(prepared.model.default_max_tokens, Some(10));
    assert_eq!(
        prepared
            .model
            .context
            .as_ref()
            .map(|value| value.context_window),
        Some(100)
    );
    let stream = (prepared.dispatch)(GenerateOptions {
        provider: "route".into(),
        model: "remote".into(),
        ..Default::default()
    });
    assert!(stream.is_ok());
    assert_eq!(*signals.lock().unwrap(), [true]);

    let resolved = adapter
        .resolve_model("route", "remote", None)
        .await
        .unwrap();
    assert_eq!(resolved.default_max_tokens, Some(20));
    assert_eq!(*signals.lock().unwrap(), [true, false]);
}

#[tokio::test]
async fn admission_refuses_direct_and_prepared_calls_before_transport() {
    let admitted = Arc::new(Mutex::new(Vec::new()));
    let adapter = Arc::new(
        DeepSeekAdapter::new(connection("http://127.0.0.1:9", 10))
            .unwrap()
            .with_model_admission(Arc::new({
                let admitted = admitted.clone();
                move |connection, model| {
                    admitted
                        .lock()
                        .unwrap()
                        .push((connection.base_url.clone(), model.to_owned()));
                    if model == "blocked" {
                        Err(error("blocked by fixture", Code::ModelUnsupported))
                    } else {
                        Ok(())
                    }
                }
            })),
    );

    let direct = adapter.stream(GenerateOptions {
        provider: "route".into(),
        model: "blocked".into(),
        ..Default::default()
    });
    let direct_error = match direct {
        Ok(_) => panic!("blocked direct call was admitted"),
        Err(error) => error,
    };
    assert_eq!(direct_error.to_string(), "blocked by fixture");

    let prepared = adapter
        .clone()
        .prepare_call("route".into(), "blocked".into(), None)
        .await;
    let prepared_error = match prepared {
        Ok(_) => panic!("blocked prepared call was admitted"),
        Err(error) => error,
    };
    assert_eq!(prepared_error.to_string(), "blocked by fixture");
    assert_eq!(admitted.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn request_auth_is_resolved_from_the_snapshot_and_maps_request_failures() {
    let mut snapshot = connection("https://wire.example", 10);
    snapshot.base_url = "not a valid URL".into();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let adapter = DeepSeekAdapter::new(snapshot)
        .unwrap()
        .with_auth_resolver(Arc::new({
            let seen = seen.clone();
            move |connection| {
                seen.lock().unwrap().push(connection.base_url);
                Box::pin(async {
                    Ok(DeepSeekRequestAuth {
                        headers: Default::default(),
                        on_request_error: Some(Arc::new(|original| {
                            Box::pin(async move {
                                assert_eq!(original.code(), "INVALID_CONFIG");
                                Ok(LlmError::new("mapped by credential", Code::Auth)
                                    .expect("test diagnostic is valid"))
                            })
                        })),
                    })
                }) as BoxFuture<'static, Result<DeepSeekRequestAuth, BoxError>>
            }
        }));
    let mut stream = adapter
        .stream(GenerateOptions {
            provider: "route".into(),
            model: "remote".into(),
            ..Default::default()
        })
        .unwrap();

    let failure = stream
        .next()
        .await
        .expect("stream reports its request failure")
        .unwrap_err();
    assert_eq!(failure.to_string(), "mapped by credential");
    assert_eq!(*seen.lock().unwrap(), ["not a valid URL"]);
}
