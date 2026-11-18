mod common;
use common::*;
use hoh_kernel::{Context, Kernel};
use hoh_llm::*;
use serde_json::json;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
fn fresh() -> (Kernel, Context, LlmRuntime) {
    let k = Kernel::start();
    let c = k.root();
    (k, c, LlmRuntime::new())
}
fn entry(provider: &str) -> LlmConfigurableProvider {
    LlmConfigurableProvider {
        provider: provider.into(),
        display_name: "OpenAI".into(),
        settings_ns: "llm-pi-ai".into(),
        settings_path: vec!["providers".into(), "openai".into()],
        ..Default::default()
    }
}
#[tokio::test]
async fn fires_at_both_adapter_registration_commit_points_with_the_registry_already_readable() {
    let (k, c, r) = fresh();
    let seen = Arc::new(Mutex::new(vec![]));
    let s = seen.clone();
    let rr = r.clone();
    c.on_contained::<AdaptersUpdated>(move |_| {
        s.lock().unwrap().push(
            rr.list_providers()
                .into_iter()
                .map(|p| p.id)
                .collect::<Vec<_>>(),
        );
        Ok(())
    })
    .unwrap();
    let h = r
        .register_adapter(
            &c,
            vec!["a".into(), "b".into()],
            Arc::new(RecordingAdapter::default()),
        )
        .unwrap();
    assert_eq!(*seen.lock().unwrap(), vec![vec!["a", "b"]]);
    h.dispose();
    assert_eq!(*seen.lock().unwrap(), vec![vec!["a", "b"], vec![]]);
    k.shutdown().await;
}
#[tokio::test]
async fn contains_a_throwing_listener_without_vetoing_registration_or_starving_later_listeners() {
    let (k, c, r) = fresh();
    let calls = Arc::new(AtomicUsize::new(0));
    c.on_contained::<AdaptersUpdated>(
        |_| Err(ValueError::Message("broken observer".into()).into()),
    )
    .unwrap();
    let count = calls.clone();
    c.on_contained::<AdaptersUpdated>(move |_| {
        count.fetch_add(1, Ordering::SeqCst);
        Ok(())
    })
    .unwrap();
    r.register_adapter(&c, vec!["a".into()], Arc::new(RecordingAdapter::default()))
        .unwrap();
    assert_eq!(r.list_providers()[0].id, "a");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(
        c.logger()
            .buffer()
            .iter()
            .any(|r| r.message == "llm: an llm/adapters-updated listener failed")
    );
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn contains_an_async_listener_rejection_instead_of_leaving_it_unhandled() {
    let (k, c, r) = fresh();
    c.on_contained_async::<AdaptersUpdated, _>(|_| async {
        Err(ValueError::Message("async observer".into()).into())
    })
    .unwrap();
    r.register_adapter(&c, vec!["a".into()], Arc::new(RecordingAdapter::default()))
        .unwrap();
    assert_eq!(r.list_providers()[0].id, "a");
    assert!(
        c.logger()
            .buffer()
            .iter()
            .any(|r| r.message == "llm: an llm/adapters-updated listener failed")
    );
    k.shutdown().await;
}
#[tokio::test]
async fn replaces_a_route_set_in_one_event_never_publishing_an_empty_registry_between_the_two() {
    let (k, c, r) = fresh();
    let h = r
        .register_adapter(&c, vec!["a".into()], Arc::new(RecordingAdapter::default()))
        .unwrap();
    let seen = Arc::new(Mutex::new(vec![]));
    let s = seen.clone();
    let rr = r.clone();
    c.on_contained::<AdaptersUpdated>(move |_| {
        s.lock().unwrap().push(rr.list_providers());
        Ok(())
    })
    .unwrap();
    h.replace(vec!["a".into()]).unwrap();
    assert_eq!(seen.lock().unwrap().len(), 1);
    assert_eq!(seen.lock().unwrap()[0][0].id, "a");
    k.shutdown().await;
}
#[tokio::test]
async fn registers_entries_lists_detached_copies_in_order_and_fires_the_topology_event() {
    let (k, c, r) = fresh();
    let calls = Arc::new(AtomicUsize::new(0));
    let cc = calls.clone();
    c.on_contained::<AdaptersUpdated>(move |_| {
        cc.fetch_add(1, Ordering::SeqCst);
        Ok(())
    })
    .unwrap();
    r.register_configurable_providers(&c, vec![entry("deepseek-official"), entry("openai")])
        .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let mut listed = r.list_configurable_providers();
    assert_eq!(
        listed
            .iter()
            .map(|e| e.provider.as_str())
            .collect::<Vec<_>>(),
        ["deepseek-official", "openai"]
    );
    listed[0].display_name = "mutated".into();
    listed[1].settings_path.push("mutated".into());
    assert_eq!(r.list_configurable_providers()[0].display_name, "OpenAI");
    assert_eq!(r.list_configurable_providers()[1].settings_path.len(), 2);
    k.shutdown().await;
}
#[tokio::test]
async fn detaches_stored_entries_from_caller_owned_objects() {
    let (k, c, r) = fresh();
    let mut source = entry("openai");
    source.default_base_url = Some("https://api.openai.com/v1".into());
    source.base_url_env = Some(vec!["ENDPOINT".into()]);
    source.native_auth = Some(LlmNativeAuth {
        env: vec!["OPENAI_API_KEY".into()],
        oauth: false,
    });
    r.register_configurable_providers(&c, vec![source.clone()])
        .unwrap();
    source
        .native_auth
        .as_mut()
        .unwrap()
        .env
        .push("mutated".into());
    source.base_url_env.as_mut().unwrap().push("mutated".into());
    let mut listed = r.list_configurable_providers();
    assert_eq!(
        listed[0].native_auth.as_ref().unwrap().env,
        ["OPENAI_API_KEY"]
    );
    listed[0].native_auth.as_mut().unwrap().env.clear();
    assert_eq!(
        r.list_configurable_providers()[0]
            .native_auth
            .as_ref()
            .unwrap()
            .env,
        ["OPENAI_API_KEY"]
    );
    assert_eq!(
        r.list_configurable_providers()[0]
            .base_url_env
            .as_ref()
            .unwrap(),
        &["ENDPOINT"]
    );
    k.shutdown().await;
}
#[tokio::test]
async fn withdraws_every_entry_when_the_registration_disposes() {
    let (k, c, r) = fresh();
    let h = r
        .register_configurable_providers(&c, vec![entry("openai")])
        .unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let cc = calls.clone();
    c.on_contained::<AdaptersUpdated>(move |_| {
        cc.fetch_add(1, Ordering::SeqCst);
        Ok(())
    })
    .unwrap();
    h.dispose();
    assert!(r.list_configurable_providers().is_empty());
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    k.shutdown().await;
}
#[tokio::test]
async fn withdraws_entries_when_the_contributing_fiber_disposes() {
    let (k, c, r) = fresh();
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    c.on_contained::<AdaptersUpdated>(move |_| {
        count.fetch_add(1, Ordering::SeqCst);
        Ok(())
    })
    .unwrap();
    let child = c.child("directory owner").unwrap();
    r.register_configurable_providers(&child, vec![entry("openai")])
        .unwrap();
    assert_eq!(r.list_configurable_providers().len(), 1);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    child.fiber().dispose().await;
    assert!(r.list_configurable_providers().is_empty());
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    k.shutdown().await;
}
#[tokio::test]
async fn rejects_an_empty_registration() {
    let (k, c, r) = fresh();
    let e = r.register_configurable_providers(&c, vec![]).err().unwrap();
    assert!(e.to_string().contains("at least one provider"));
    assert_eq!(
        HarnessError::of(e.as_ref()).unwrap().code(),
        "INVALID_DIRECTORY"
    );
    k.shutdown().await;
}
#[tokio::test]
async fn rejects_invalid_entries_all_or_nothing() {
    let (k, c, r) = fresh();
    for invalid in [
        LlmConfigurableProvider {
            provider: "".into(),
            ..entry("openai")
        },
        LlmConfigurableProvider {
            display_name: "".into(),
            ..entry("openai")
        },
        LlmConfigurableProvider {
            settings_ns: "".into(),
            ..entry("openai")
        },
        LlmConfigurableProvider {
            settings_path: vec!["providers".into(), "".into()],
            ..entry("openai")
        },
    ] {
        assert!(
            r.register_configurable_providers(&c, vec![entry("valid-first"), invalid])
                .is_err()
        );
        assert!(r.list_configurable_providers().is_empty());
    }
    k.shutdown().await;
}
#[tokio::test]
async fn replaces_its_entries_atomically_keeping_the_old_set_when_a_candidate_collides() {
    let (k, c, r) = fresh();
    let h = r
        .register_configurable_providers(&c, vec![entry("openai"), entry("second")])
        .unwrap();
    r.register_configurable_providers(&c, vec![entry("owned-elsewhere")])
        .unwrap();
    assert!(h.replace(vec![entry("owned-elsewhere")]).is_err());
    assert_eq!(r.list_configurable_providers().len(), 3);
    let mut renamed = entry("openai");
    renamed.display_name = "Renamed".into();
    h.replace(vec![renamed]).unwrap();
    assert_eq!(r.list_configurable_providers().len(), 2);
    assert_eq!(r.list_configurable_providers()[1].display_name, "Renamed");
    h.replace(vec![]).unwrap();
    assert_eq!(
        r.list_configurable_providers()[0].provider,
        "owned-elsewhere"
    );
    h.dispose();
    assert!(
        h.replace(vec![entry("openai")])
            .unwrap_err()
            .to_string()
            .contains("was disposed")
    );
    k.shutdown().await;
}
#[tokio::test]
async fn rejects_duplicates_within_one_registration_and_across_registrations() {
    let (k, c, r) = fresh();
    assert!(
        r.register_configurable_providers(&c, vec![entry("openai"), entry("openai")])
            .is_err()
    );
    r.register_configurable_providers(&c, vec![entry("openai")])
        .unwrap();
    assert!(
        r.register_configurable_providers(&c, vec![entry("openai"), entry("unseen")])
            .is_err()
    );
    assert_eq!(r.list_configurable_providers().len(), 1);
    k.shutdown().await;
}
#[tokio::test]
async fn offers_one_interrogation_per_settings_namespace_and_disposes_with_its_fiber() {
    let (k, c, r) = fresh();
    let h = r
        .register_model_discovery(
            &c,
            "llm-example".into(),
            Arc::new(|request, signal| {
                Box::pin(async move {
                    assert_eq!(
                        request.base_url.as_deref(),
                        Some("https://gateway.example/v1")
                    );
                    assert!(signal.is_none());
                    Ok(vec![decode(json!({"id":"from-endpoint"}))])
                })
            }),
        )
        .unwrap();
    let request = LlmModelDiscoveryRequest {
        base_url: Some("https://gateway.example/v1".into()),
        ..Default::default()
    };
    assert_eq!(
        r.discover_models("llm-example", request.clone(), None)
            .await
            .unwrap()[0]
            .id,
        "from-endpoint"
    );
    drop(h.dispose());
    assert!(
        r.discover_models("llm-example", request, None)
            .await
            .unwrap_err()
            .to_string()
            .contains("no model discovery is registered")
    );
    k.shutdown().await;
}
#[tokio::test]
async fn rejects_an_unnamed_namespace_and_a_second_registration_of_the_same_one() {
    let (k, c, r) = fresh();
    let d: ModelDiscovery = Arc::new(|_, _| Box::pin(async { Ok(vec![]) }));
    assert!(
        r.register_model_discovery(&c, "".into(), d.clone())
            .is_err()
    );
    r.register_model_discovery(&c, "llm-example".into(), d.clone())
        .unwrap();
    assert!(
        r.register_model_discovery(&c, "llm-example".into(), d)
            .is_err()
    );
    assert!(
        r.discover_models(
            "llm-example",
            LlmModelDiscoveryRequest {
                provider: Some("known".into()),
                ..Default::default()
            },
            None
        )
        .await
        .unwrap()
        .is_empty()
    );
    k.shutdown().await;
}
#[tokio::test]
async fn preserves_discovered_input_types_without_inventing_missing_metadata() {
    let (k, c, r) = fresh();
    r.register_model_discovery(&c,"llm-example".into(),Arc::new(|_,_|Box::pin(async{Ok(decode(json!([{"id":"keep","name":"Keep","contextWindow":1024,"maxTokens":256,"inputModalities":["text","image"]},{"id":""},{"id":"keep"},{"id":"bare"}])))}))).unwrap();
    let models = r
        .discover_models(
            "llm-example",
            LlmModelDiscoveryRequest {
                provider: Some("known".into()),
                ..Default::default()
            },
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        value(models),
        json!([{"id":"keep","name":"Keep","contextWindow":1024,"maxTokens":256,"inputModalities":["text","image"]},{"id":"bare"}])
    );
    k.shutdown().await;
}
#[tokio::test]
async fn carries_cancellation_into_remote_discovery_and_maps_provider_failures() {
    let (k, c, r) = fresh();
    r.register_model_discovery(
        &c,
        "llm-example".into(),
        Arc::new(|_, signal| {
            Box::pin(async move {
                assert!(signal.is_some());
                Err(ValueError::Message("endpoint offline".into()).into())
            })
        }),
    )
    .unwrap();
    let e = r
        .remote_discover_models(
            "llm-example",
            LlmModelDiscoveryRequest {
                base_url: Some("https://gateway.example/v1".into()),
                ..Default::default()
            },
            hoh_timeout::AbortController::new().signal(),
        )
        .await
        .unwrap_err();
    assert_eq!(e.name(), "RemoteError");
    assert_eq!(e.code(), "llm/model-discovery-rejected");
    assert_eq!(e.message(), "endpoint offline");
    let d = e.detail::<ModelDiscoveryRejectedDetails>().unwrap();
    assert_eq!(d.settings_ns, "llm-example");
    assert_eq!(
        value(d),
        json!({"settingsNs":"llm-example","baseURL":"https://gateway.example/v1"})
    );
    assert_eq!(d.base_url.as_deref(), Some("https://gateway.example/v1"));
    let e = r
        .remote_discover_models(
            "llm-example",
            LlmModelDiscoveryRequest {
                provider: Some("known-route".into()),
                ..Default::default()
            },
            hoh_timeout::AbortController::new().signal(),
        )
        .await
        .unwrap_err();
    assert!(
        e.detail::<ModelDiscoveryRejectedDetails>()
            .unwrap()
            .base_url
            .is_none()
    );
    k.shutdown().await;
}
#[tokio::test]
async fn refuses_a_namespace_nothing_serves_and_a_draft_with_no_endpoint() {
    let (k, c, r) = fresh();
    r.register_model_discovery(
        &c,
        "llm-example".into(),
        Arc::new(|_, _| Box::pin(async { Ok(vec![]) })),
    )
    .unwrap();
    let e = r
        .discover_models("llm-absent", LlmModelDiscoveryRequest::default(), None)
        .await
        .unwrap_err();
    assert_eq!(HarnessError::of(e.as_ref()).unwrap().code(), "NO_DISCOVERY");
    for req in [
        LlmModelDiscoveryRequest::default(),
        LlmModelDiscoveryRequest {
            provider: Some("".into()),
            base_url: Some("".into()),
            ..Default::default()
        },
    ] {
        let e = r
            .discover_models("llm-example", req, None)
            .await
            .unwrap_err();
        assert_eq!(
            HarnessError::of(e.as_ref()).unwrap().code(),
            "INVALID_DISCOVERY"
        );
    }
    assert!(
        r.discover_models(
            "llm-example",
            LlmModelDiscoveryRequest {
                provider: Some("known-route".into()),
                ..Default::default()
            },
            None
        )
        .await
        .unwrap()
        .is_empty()
    );
    k.shutdown().await;
}
struct Pricing;
impl LlmImageRequestPricing for Pricing {
    fn price_images(&self, _: &[ImageBlock]) -> Vec<LlmImageRequestPrice> {
        vec![]
    }
}
struct PricingAdapter(Arc<dyn LlmImageRequestPricing>);
impl LlmAdapter for PricingAdapter {
    fn stream(&self, _: GenerateOptions) -> Result<LlmStream, hoh_error::BoxError> {
        Ok(LlmStream::chunks([]))
    }
    fn image_request_pricing(&self, p: &str, m: &str) -> Option<Arc<dyn LlmImageRequestPricing>> {
        (p == "a" && m == "vision").then(|| self.0.clone())
    }
}
#[tokio::test]
async fn resolves_the_owning_adapter_declaration_and_degrades_everywhere_else_to_undefined() {
    let (k, c, r) = fresh();
    let pricing: Arc<dyn LlmImageRequestPricing> = Arc::new(Pricing);
    let h = r
        .register_adapter(
            &c,
            vec!["a".into()],
            Arc::new(PricingAdapter(pricing.clone())),
        )
        .unwrap();
    r.register_adapter(
        &c,
        vec!["plain".into()],
        Arc::new(RecordingAdapter::default()),
    )
    .unwrap();
    assert!(Arc::ptr_eq(
        &r.image_request_pricing("a", "vision").unwrap(),
        &pricing
    ));
    for (p, m) in [("a", "other"), ("plain", "vision"), ("missing", "vision")] {
        assert!(r.image_request_pricing(p, m).is_none());
    }
    h.dispose();
    assert!(r.image_request_pricing("a", "vision").is_none());
    k.shutdown().await;
}
