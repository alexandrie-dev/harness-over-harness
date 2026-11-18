use hoh_error::{BoxError, HarnessError, SharedError};
use hoh_kernel::{Cfg, Context, Fiber, Inject, Kernel, Plugin, Service};
use hoh_package_manifest::LocalizedText;
use hoh_timeout::{AbortController, AbortSignal};
use hoh_web::*;
use serde_json::{Value, json};
use std::sync::{
    Arc, Mutex, MutexGuard,
    atomic::{AtomicUsize, Ordering},
};

fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value.lock().unwrap()
}
fn info(id: &str) -> WebProviderInfo {
    WebProviderInfo {
        id: id.into(),
        title: None,
        description: None,
    }
}
fn source(url: &str) -> WebSearchSource {
    WebSearchSource {
        url: url.into(),
        title: None,
        snippet: None,
        summary: None,
        published_at: None,
    }
}
fn search_result(marker: &str) -> WebSearchResult {
    WebSearchResult {
        content: Some(marker.into()),
        sources: vec![],
        truncated: false,
    }
}
fn fetch_result(marker: &str) -> WebFetchResult {
    WebFetchResult {
        url: "https://example.com".into(),
        status_code: 200,
        body: WebFetchBody::Text {
            content: marker.into(),
        },
        truncated: false,
    }
}
fn request() -> WebSearchRequest {
    WebSearchRequest {
        query: "q".into(),
        max_results: None,
    }
}
fn fetch_request() -> WebFetchRequest {
    WebFetchRequest {
        url: "https://example.com".into(),
    }
}

#[derive(Clone)]
struct Script {
    info: WebProviderInfo,
    usable: bool,
    ready: bool,
    asked: Arc<Mutex<Vec<Option<String>>>>,
    availability_calls: Arc<AtomicUsize>,
    availability_error: Option<SharedError>,
    execution_error: Option<SharedError>,
    readiness_error: Option<SharedError>,
    searches: Arc<Mutex<Vec<WebSearchRequest>>>,
    signals: Arc<Mutex<Vec<Option<usize>>>>,
    search_result: WebSearchResult,
    fetch_result: WebFetchResult,
}
impl Script {
    fn new(id: &str, usable: bool) -> Self {
        Self {
            info: info(id),
            usable,
            ready: true,
            asked: Arc::default(),
            availability_calls: Arc::default(),
            availability_error: None,
            execution_error: None,
            readiness_error: None,
            searches: Arc::default(),
            signals: Arc::default(),
            search_result: search_result(id),
            fetch_result: fetch_result(id),
        }
    }
    fn check(&self) -> Result<bool, BoxError> {
        self.availability_calls.fetch_add(1, Ordering::SeqCst);
        if let Some(error) = &self.availability_error {
            return Err(error.clone().into());
        }
        Ok(self.usable)
    }
    fn readiness<'a>(&'a self, route: Option<&'a str>) -> WebFuture<'a, bool> {
        Box::pin(async move {
            lock(&self.asked).push(route.map(str::to_owned));
            if let Some(error) = &self.readiness_error {
                return Err(error.clone().into());
            }
            Ok(self.ready)
        })
    }
    fn record_signal(&self, signal: Option<&AbortSignal>) {
        lock(&self.signals).push(signal.map(|s| std::ptr::from_ref(s) as usize));
    }
}
impl WebSearchProvider for Script {
    fn info(&self) -> WebProviderInfo {
        self.info.clone()
    }
    fn available(&self) -> Result<bool, BoxError> {
        self.check()
    }
    fn ready<'a>(&'a self, route: Option<&'a str>) -> WebFuture<'a, bool> {
        self.readiness(route)
    }
    fn search<'a>(
        &'a self,
        request: WebSearchRequest,
        signal: Option<&'a AbortSignal>,
    ) -> WebFuture<'a, WebSearchResult> {
        Box::pin(async move {
            lock(&self.searches).push(request);
            self.record_signal(signal);
            if let Some(error) = &self.execution_error {
                return Err(error.clone().into());
            }
            Ok(self.search_result.clone())
        })
    }
}
impl WebFetchProvider for Script {
    fn info(&self) -> WebProviderInfo {
        self.info.clone()
    }
    fn available(&self) -> Result<bool, BoxError> {
        self.check()
    }
    fn ready<'a>(&'a self, route: Option<&'a str>) -> WebFuture<'a, bool> {
        self.readiness(route)
    }
    fn fetch<'a>(
        &'a self,
        _: WebFetchRequest,
        signal: Option<&'a AbortSignal>,
    ) -> WebFuture<'a, WebFetchResult> {
        Box::pin(async move {
            self.record_signal(signal);
            if let Some(error) = &self.execution_error {
                return Err(error.clone().into());
            }
            Ok(self.fetch_result.clone())
        })
    }
}
struct Plain(Script);
impl WebSearchProvider for Plain {
    fn info(&self) -> WebProviderInfo {
        self.0.info.clone()
    }
    fn available(&self) -> Result<bool, BoxError> {
        self.0.check()
    }
    fn search<'a>(
        &'a self,
        request: WebSearchRequest,
        signal: Option<&'a AbortSignal>,
    ) -> WebFuture<'a, WebSearchResult> {
        WebSearchProvider::search(&self.0, request, signal)
    }
}
impl WebFetchProvider for Plain {
    fn info(&self) -> WebProviderInfo {
        self.0.info.clone()
    }
    fn available(&self) -> Result<bool, BoxError> {
        self.0.check()
    }
    fn fetch<'a>(
        &'a self,
        request: WebFetchRequest,
        signal: Option<&'a AbortSignal>,
    ) -> WebFuture<'a, WebFetchResult> {
        WebFetchProvider::fetch(&self.0, request, signal)
    }
}

#[derive(Clone, Default)]
struct Environment(Option<String>, Option<String>);
struct TestWeb;
impl Plugin for TestWeb {
    const NAME: &'static str = "WebRuntime";
    type Config = WebRuntimeConfig;
    type Deps = ();
    async fn apply(ctx: Context, _: (), config: Cfg<Self::Config>) -> Result<(), BoxError> {
        let env = ctx.tag::<Environment>().unwrap();
        let web = WebRuntime::with_environment(&ctx, config, env.0.clone(), env.1.clone());
        ctx.provide::<Web>(Arc::new(web)).await?;
        Ok(())
    }
}
struct Mounted {
    kernel: Kernel,
    ctx: Context,
    web: Service<Web>,
    fiber: Fiber,
}
async fn mount(config: Value) -> Mounted {
    mount_env(config, Environment::default()).await
}
async fn mount_env(config: Value, env: Environment) -> Mounted {
    let kernel = Kernel::start();
    let ctx = kernel.root().with_tag(env);
    let fiber = ctx.plugin::<TestWeb>(config).unwrap();
    fiber.settled().await.unwrap();
    let web = ctx.get::<Web>().unwrap();
    Mounted {
        kernel,
        ctx,
        web,
        fiber,
    }
}
impl Mounted {
    fn search(&self, script: Script) -> hoh_kernel::Disposer {
        self.web
            .register_search_provider(&self.ctx, Arc::new(script))
            .unwrap()
    }
    fn fetch(&self, script: Script) -> hoh_kernel::Disposer {
        self.web
            .register_fetch_provider(&self.ctx, Arc::new(script))
            .unwrap()
    }
    async fn marker(&self) -> String {
        self.web
            .search(request(), None)
            .await
            .unwrap()
            .content
            .unwrap()
    }
    async fn close(self) {
        self.kernel.shutdown().await;
    }
}
fn assert_error(error: BoxError, code: &str, message: &str) {
    let error = HarnessError::of(error.as_ref()).unwrap();
    assert_eq!(error.name(), "WebError");
    assert_eq!(error.code(), code);
    assert_eq!(error.message(), message);
}
#[derive(Inject)]
struct WebDeps {
    web: Service<Web>,
}
struct Contributor;
impl Plugin for Contributor {
    const NAME: &'static str = "contributor";
    type Config = hoh_kernel::NoConfig;
    type Deps = WebDeps;
    async fn apply(ctx: Context, deps: WebDeps, _: Cfg<Self::Config>) -> Result<(), BoxError> {
        let mut script = Script::new("custom", true);
        script.info.title = Some("Custom".into());
        deps.web.register_search_provider(&ctx, Arc::new(script))?;
        Ok(())
    }
}

#[tokio::test(start_paused = true)]
async fn registers_a_search_provider_and_unregisters_it_via_the_returned_disposer() {
    let m = mount(json!({})).await;
    let dispose = m.search(Script::new("exa", true));
    assert_eq!(m.marker().await, "exa");
    // Calling dispose starts synchronous cleanup even when its future is discarded.
    drop(dispose.dispose());
    assert_error(
        m.web.search(request(), None).await.unwrap_err(),
        "WEB_PROVIDER_UNAVAILABLE",
        "no usable web provider is registered",
    );
    m.close().await;
}
#[tokio::test(start_paused = true)]
async fn throws_web_duplicate_provider_on_a_duplicate_search_id() {
    let m = mount(json!({})).await;
    m.search(Script::new("exa", true));
    let error = m
        .web
        .register_search_provider(&m.ctx, Arc::new(Script::new("exa", true)))
        .err()
        .unwrap();
    assert_error(
        error,
        "WEB_DUPLICATE_PROVIDER",
        "a web provider with id \"exa\" is already registered",
    );
    m.close().await;
}
#[tokio::test(start_paused = true)]
async fn keeps_search_and_fetch_id_namespaces_independent() {
    let m = mount(json!({})).await;
    m.search(Script::new("shared", true));
    m.fetch(Script::new("shared", true));
    assert_eq!(m.web.list_providers().search[0].id, "shared");
    assert_eq!(m.web.list_providers().fetch[0].id, "shared");
    m.close().await;
}
#[tokio::test(start_paused = true)]
async fn disposes_provider_registrations_when_the_contributing_fiber_is_disposed_hmr_safety() {
    let m = mount(json!({})).await;
    let fiber = m.ctx.plugin::<Contributor>(json!({})).unwrap();
    fiber.settled().await.unwrap();
    assert_eq!(m.marker().await, "custom");
    fiber.dispose().await;
    assert_error(
        m.web.search(request(), None).await.unwrap_err(),
        "WEB_PROVIDER_UNAVAILABLE",
        "no usable web provider is registered",
    );
    m.close().await;
}
#[tokio::test(start_paused = true)]
async fn lists_registered_providers_per_capability_in_registration_order_including_unavailable_ones()
 {
    let m = mount(json!({})).await;
    assert_eq!(
        serde_json::to_value(m.web.list_providers()).unwrap(),
        json!({"search":[],"fetch":[]})
    );
    let mut b = Script::new("b", true);
    b.info.title = Some("B search".into());
    m.search(b);
    let a = m.search(Script::new("a", false));
    let mut http = Script::new("http", true);
    http.info.title = Some("Direct fetch".into());
    http.info.description = Some(
        serde_json::from_value::<LocalizedText>(
            json!({"en":"Fetches from this machine.","zh":"本机获取"}),
        )
        .unwrap(),
    );
    m.fetch(http);
    assert_eq!(
        serde_json::to_value(m.web.list_providers()).unwrap(),
        json!({"search":[{"id":"b","title":"B search"},{"id":"a"}],"fetch":[{"id":"http","title":"Direct fetch","description":{"en":"Fetches from this machine.","zh":"本机获取"}}]})
    );
    drop(a.dispose());
    assert_eq!(
        serde_json::to_value(m.web.list_providers().search).unwrap(),
        json!([{"id":"b","title":"B search"}])
    );
    m.close().await;
}
#[tokio::test(start_paused = true)]
async fn emits_web_providers_changed_once_after_each_registration_and_each_disposal() {
    let m = mount(json!({})).await;
    let observed = Arc::new(Mutex::new(Vec::<Vec<String>>::new()));
    let seen = observed.clone();
    let web = m.web.clone();
    m.ctx
        .on_contained::<WebProvidersChanged>(move |_| {
            lock(&seen).push(
                web.list_providers()
                    .fetch
                    .into_iter()
                    .map(|p| p.id)
                    .collect(),
            );
            Ok(())
        })
        .unwrap();
    let dispose = m.fetch(Script::new("http", true));
    assert_eq!(*lock(&observed), vec![vec!["http".to_string()]]);
    drop(dispose.dispose());
    assert_eq!(*lock(&observed), vec![vec!["http".to_string()], vec![]]);
    m.close().await;
}
#[tokio::test(start_paused = true)]
async fn does_not_emit_web_providers_changed_for_a_rejected_duplicate_registration() {
    let m = mount(json!({})).await;
    m.search(Script::new("exa", true));
    let changes = Arc::new(AtomicUsize::new(0));
    let seen = changes.clone();
    m.ctx
        .on_contained::<WebProvidersChanged>(move |_| {
            seen.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
        .unwrap();
    assert!(
        m.web
            .register_search_provider(&m.ctx, Arc::new(Script::new("exa", true)))
            .is_err()
    );
    assert_eq!(changes.load(Ordering::SeqCst), 0);
    m.close().await;
}
#[tokio::test(start_paused = true)]
async fn contains_a_throwing_or_rejecting_listener_so_later_listeners_still_run_and_registration_commits()
 {
    let m = mount(json!({})).await;
    m.ctx
        .on_contained::<WebProvidersChanged>(|_| {
            Err(std::io::Error::other("sync listener failure").into())
        })
        .unwrap();
    m.ctx
        .on_contained_async::<WebProvidersChanged, _>(|_| async {
            Err(std::io::Error::other("async listener failure").into())
        })
        .unwrap();
    let later = Arc::new(AtomicUsize::new(0));
    let seen = later.clone();
    m.ctx
        .on_contained::<WebProvidersChanged>(move |_| {
            seen.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
        .unwrap();
    m.fetch(Script::new("http", true));
    tokio::task::yield_now().await;
    assert_eq!(later.load(Ordering::SeqCst), 1);
    assert_eq!(m.web.list_providers().fetch, vec![info("http")]);
    let records = m.ctx.logger().buffer();
    assert_eq!(
        records
            .iter()
            .map(|r| r.message.as_str())
            .collect::<Vec<_>>(),
        vec![
            "web: a web/providers-changed listener failed",
            "sync listener failure",
            "web: a web/providers-changed listener failed",
            "async listener failure"
        ]
    );
    assert!(
        records
            .iter()
            .all(|r| r.fiber == m.fiber.uid() && r.name.as_ref() == "web")
    );
    m.close().await;
}
#[tokio::test(start_paused = true)]
async fn removes_a_contributing_fibers_providers_from_the_list_and_announces_it_hmr_safety() {
    let m = mount(json!({})).await;
    let changes = Arc::new(AtomicUsize::new(0));
    let seen = changes.clone();
    m.ctx
        .on_contained::<WebProvidersChanged>(move |_| {
            seen.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
        .unwrap();
    let fiber = m.ctx.plugin::<Contributor>(json!({})).unwrap();
    fiber.settled().await.unwrap();
    assert_eq!(
        serde_json::to_value(m.web.list_providers().search).unwrap(),
        json!([{"id":"custom","title":"Custom"}])
    );
    assert_eq!(changes.load(Ordering::SeqCst), 1);
    fiber.dispose().await;
    assert!(m.web.list_providers().search.is_empty());
    assert_eq!(changes.load(Ordering::SeqCst), 2);
    m.close().await;
}

#[tokio::test(start_paused = true)]
async fn throws_web_provider_unavailable_when_nothing_is_registered() {
    let m = mount(json!({})).await;
    assert_error(
        m.web.search(request(), None).await.unwrap_err(),
        "WEB_PROVIDER_UNAVAILABLE",
        "no usable web provider is registered",
    );
    m.close().await;
}
#[tokio::test(start_paused = true)]
async fn throws_web_provider_unavailable_when_providers_exist_but_none_are_usable() {
    let m = mount(json!({})).await;
    m.search(Script::new("exa", false));
    assert_error(
        m.web.search(request(), None).await.unwrap_err(),
        "WEB_PROVIDER_UNAVAILABLE",
        "no usable web provider is registered",
    );
    m.close().await;
}
#[tokio::test(start_paused = true)]
async fn throws_web_provider_configured_missing_for_an_unregistered_configured_id() {
    let m = mount(json!({"searchProvider":"perplexity"})).await;
    m.search(Script::new("exa", true));
    assert_error(
        m.web.search(request(), None).await.unwrap_err(),
        "WEB_PROVIDER_CONFIGURED_MISSING",
        "configured web provider \"perplexity\" is not registered",
    );
    m.close().await;
}
#[tokio::test(start_paused = true)]
async fn throws_web_provider_configured_unavailable_for_an_unusable_configured_id() {
    let m = mount(json!({"searchProvider":"exa"})).await;
    m.search(Script::new("exa", false));
    assert_error(
        m.web.search(request(), None).await.unwrap_err(),
        "WEB_PROVIDER_CONFIGURED_UNAVAILABLE",
        "configured web provider \"exa\" is registered but unavailable",
    );
    m.close().await;
}
#[tokio::test(start_paused = true)]
async fn throws_web_provider_ambiguous_rather_than_picking_by_order() {
    let m = mount(json!({})).await;
    m.search(Script::new("exa", true));
    m.search(Script::new("perplexity", true));
    assert_error(
        m.web.search(request(), None).await.unwrap_err(),
        "WEB_PROVIDER_AMBIGUOUS",
        "multiple usable web providers are registered (exa, perplexity); configure one explicitly",
    );
    m.close().await;
}
#[tokio::test(start_paused = true)]
async fn runs_the_configured_provider_even_when_another_usable_provider_is_registered() {
    let m = mount(json!({"searchProvider":"perplexity"})).await;
    m.search(Script::new("exa", true));
    m.search(Script::new("perplexity", true));
    assert_eq!(m.marker().await, "perplexity");
    m.close().await;
}
#[tokio::test(start_paused = true)]
async fn ignores_unusable_providers_when_auto_selecting() {
    let m = mount(json!({})).await;
    m.search(Script::new("exa", true));
    m.search(Script::new("perplexity", false));
    assert_eq!(m.marker().await, "exa");
    m.close().await;
}
#[tokio::test(start_paused = true)]
async fn does_not_let_registration_order_change_auto_selection() {
    for ids in [["exa", "perplexity"], ["perplexity", "exa"]] {
        let m = mount(json!({})).await;
        for id in ids {
            m.search(Script::new(id, id == "perplexity"));
        }
        assert_eq!(m.marker().await, "perplexity");
        m.close().await;
    }
}
#[tokio::test(start_paused = true)]
async fn runs_the_selected_provider_and_returns_its_result() {
    let m = mount(json!({})).await;
    let mut script = Script::new("exa", true);
    script.search_result = search_result("answer");
    script.search_result.sources = vec![source("https://a")];
    m.search(script);
    let result = m.web.search(request(), None).await.unwrap();
    assert_eq!(result.content.as_deref(), Some("answer"));
    assert_eq!(result.sources, vec![source("https://a")]);
    m.close().await;
}
#[tokio::test(start_paused = true)]
async fn propagates_the_abort_signal_to_the_provider() {
    let m = mount(json!({})).await;
    let script = Script::new("exa", true);
    let seen = script.signals.clone();
    m.search(script);
    let controller = AbortController::new();
    let signal = controller.signal();
    m.web.search(request(), Some(&signal)).await.unwrap();
    assert_eq!(
        *lock(&seen),
        vec![Some(std::ptr::from_ref(&signal) as usize)]
    );
    m.close().await;
}

#[tokio::test(start_paused = true)]
async fn answers_from_the_selected_provider_and_passes_it_the_request_route() {
    let m = mount(json!({"searchProvider":"deepseek"})).await;
    let mut p = Script::new("deepseek", true);
    p.ready = false;
    let asked = p.asked.clone();
    m.search(p);
    assert!(!m.web.search_ready(Some("deepseek-official")).await.unwrap());
    assert_eq!(*lock(&asked), vec![Some("deepseek-official".into())]);
    m.close().await;
}
#[tokio::test(start_paused = true)]
async fn does_not_ask_a_provider_that_search_would_not_select() {
    let m = mount(json!({"searchProvider":"exa"})).await;
    m.search(Script::new("exa", true));
    let mut p = Script::new("deepseek", true);
    p.ready = false;
    let asked = p.asked.clone();
    m.search(p);
    assert!(m.web.search_ready(Some("deepseek-official")).await.unwrap());
    assert!(lock(&asked).is_empty());
    m.close().await;
}
#[tokio::test(start_paused = true)]
async fn asks_the_single_usable_provider_when_none_is_configured() {
    let m = mount(json!({})).await;
    let mut p = Script::new("usable", true);
    p.ready = false;
    let asked = p.asked.clone();
    m.search(p);
    m.search(Script::new("unusable", false));
    assert!(!m.web.search_ready(None).await.unwrap());
    assert_eq!(*lock(&asked), vec![None]);
    m.close().await;
}
#[tokio::test(start_paused = true)]
async fn treats_a_selected_search_provider_without_ready_as_ready() {
    let m = mount(json!({})).await;
    m.web
        .register_search_provider(&m.ctx, Arc::new(Plain(Script::new("plain", true))))
        .unwrap();
    assert!(m.web.search_ready(Some("deepseek-official")).await.unwrap());
    m.close().await;
}
#[tokio::test(start_paused = true)]
async fn answers_true_when_selection_fails_leaving_search_to_report_the_failure() {
    let cases = [
        (json!({}), vec![]),
        (json!({"searchProvider":"missing"}), vec![("other", true)]),
        (json!({"searchProvider":"down"}), vec![("down", false)]),
        (json!({}), vec![("a", true), ("b", true)]),
    ];
    for (config, providers) in cases {
        let m = mount(config).await;
        for (id, usable) in providers {
            let mut p = Script::new(id, usable);
            p.ready = false;
            m.search(p);
        }
        assert!(m.web.search_ready(Some("deepseek-official")).await.unwrap());
        assert!(
            HarnessError::of(m.web.search(request(), None).await.unwrap_err().as_ref())
                .unwrap()
                .is_named(WebError::NAME)
        );
        m.close().await;
    }
}

async fn capped(max: Option<u64>, count: usize) -> WebSearchResult {
    let m = mount(json!({})).await;
    let mut p = Script::new("exa", true);
    p.search_result.sources = (1..=count)
        .map(|n| source(&format!("https://{n}")))
        .collect();
    let seen = p.searches.clone();
    m.search(p);
    let mut req = request();
    req.max_results = max;
    let result = m.web.search(req.clone(), None).await.unwrap();
    assert_eq!(*lock(&seen), vec![req]);
    m.close().await;
    result
}
#[tokio::test(start_paused = true)]
async fn truncates_sources_and_sets_truncated_when_a_provider_over_returns() {
    let result = capped(Some(2), 3).await;
    assert_eq!(
        result.sources,
        vec![source("https://1"), source("https://2")]
    );
    assert!(result.truncated);
}
#[tokio::test(start_paused = true)]
async fn leaves_truncated_false_when_within_the_bound() {
    let result = capped(Some(8), 1).await;
    assert_eq!(result.sources.len(), 1);
    assert!(!result.truncated);
}
#[tokio::test(start_paused = true)]
async fn does_not_bound_when_max_results_is_omitted() {
    let result = capped(None, 2).await;
    assert_eq!(result.sources.len(), 2);
    assert!(!result.truncated);
}
#[tokio::test(start_paused = true)]
async fn resolves_and_runs_the_fetch_provider_independently_of_search() {
    let m = mount(json!({})).await;
    m.fetch(Script::new("http", true));
    assert_eq!(
        m.web.fetch(fetch_request(), None).await.unwrap(),
        fetch_result("http")
    );
    m.close().await;
}
#[tokio::test(start_paused = true)]
async fn throws_web_provider_unavailable_for_fetch_when_no_fetch_provider_is_registered() {
    let m = mount(json!({})).await;
    m.search(Script::new("exa", true));
    assert_error(
        m.web.fetch(fetch_request(), None).await.unwrap_err(),
        "WEB_PROVIDER_UNAVAILABLE",
        "no usable web provider is registered",
    );
    m.close().await;
}
#[tokio::test(start_paused = true)]
async fn answers_from_the_provider_fetch_selects_and_passes_it_the_request_route() {
    let m = mount(json!({"fetchProvider":"deepseek"})).await;
    let mut p = Script::new("deepseek", true);
    p.ready = false;
    let asked = p.asked.clone();
    m.fetch(p);
    let http = Script::new("http", true);
    let http_asked = http.asked.clone();
    m.fetch(http);
    assert!(!m.web.fetch_ready(Some("deepseek-official")).await.unwrap());
    assert_eq!(*lock(&asked), vec![Some("deepseek-official".into())]);
    assert!(lock(&http_asked).is_empty());
    m.close().await;
}
#[tokio::test(start_paused = true)]
async fn treats_a_selected_fetch_provider_without_ready_as_ready() {
    let m = mount(json!({})).await;
    m.web
        .register_fetch_provider(&m.ctx, Arc::new(Plain(Script::new("http", true))))
        .unwrap();
    assert!(m.web.fetch_ready(Some("deepseek-official")).await.unwrap());
    m.close().await;
}
#[tokio::test(start_paused = true)]
async fn answers_true_when_no_provider_qualifies_leaving_fetch_to_report_the_failure() {
    let m = mount(json!({"fetchProvider":"down"})).await;
    let mut p = Script::new("down", false);
    p.ready = false;
    m.fetch(p);
    assert!(m.web.fetch_ready(Some("deepseek-official")).await.unwrap());
    assert_error(
        m.web.fetch(fetch_request(), None).await.unwrap_err(),
        "WEB_PROVIDER_CONFIGURED_UNAVAILABLE",
        "configured web provider \"down\" is registered but unavailable",
    );
    m.close().await;
}
#[test]
fn is_a_harness_error_carrying_its_code() {
    let error = WebError::new("boom", "WEB_INVALID_URL");
    assert_eq!(error.code(), "WEB_INVALID_URL");
    assert_eq!(error.name(), "WebError");
}
#[tokio::test(start_paused = true)]
async fn applies_a_saved_search_and_fetch_selection_to_the_next_operation_without_remounting() {
    let m = mount(json!({"searchProvider":"first","fetchProvider":"first"})).await;
    for id in ["first", "second"] {
        m.search(Script::new(id, true));
        m.fetch(Script::new(id, true));
    }
    let identity = m.fiber.uid();
    assert_eq!(m.marker().await, "first");
    assert_eq!(
        m.web.fetch(fetch_request(), None).await.unwrap(),
        fetch_result("first")
    );
    assert!(
        m.fiber
            .commit_volatile(json!({"searchProvider":"second","fetchProvider":"second"}))
            .await
            .unwrap()
    );
    assert_eq!(m.fiber.uid(), identity);
    assert!(std::ptr::eq::<WebRuntime>(
        &*m.web,
        &*m.ctx.get::<Web>().unwrap()
    ));
    assert_eq!(m.marker().await, "second");
    assert_eq!(
        m.web.fetch(fetch_request(), None).await.unwrap(),
        fetch_result("second")
    );
    assert_eq!(m.ctx.get::<Web>().unwrap().list_providers().search.len(), 2);
    m.close().await;
}

#[test]
fn json_vocabulary_preserves_field_names_order_optional_fields_and_closed_body_union() {
    let source_json = r#"{"url":"https://a","title":"A","snippet":"short","summary":"long","publishedAt":"2026-01-02T03:04:05.678Z"}"#;
    let item: WebSearchSource = serde_json::from_str(source_json).unwrap();
    assert_eq!(serde_json::to_string(&item).unwrap(), source_json);
    assert_eq!(
        serde_json::to_string(&source("https://a")).unwrap(),
        r#"{"url":"https://a"}"#
    );
    assert_eq!(
        serde_json::to_string(&request()).unwrap(),
        r#"{"query":"q"}"#
    );
    let req: WebSearchRequest = serde_json::from_str(r#"{"query":"q","maxResults":2}"#).unwrap();
    assert_eq!(req.max_results, Some(2));
    assert_eq!(
        serde_json::to_string(&req).unwrap(),
        r#"{"query":"q","maxResults":2}"#
    );
    let result = WebSearchResult {
        content: Some("answer".into()),
        sources: vec![item],
        truncated: false,
    };
    assert_eq!(
        serde_json::to_string(&result).unwrap(),
        format!(r#"{{"content":"answer","sources":[{source_json}],"truncated":false}}"#)
    );
    let empty: WebSearchResult =
        serde_json::from_str(r#"{"sources":[],"truncated":true}"#).unwrap();
    assert_eq!(empty.content, None);
    assert_eq!(
        serde_json::to_string(&empty).unwrap(),
        r#"{"sources":[],"truncated":true}"#
    );
    for kind in ["html", "text"] {
        let json = format!(
            r#"{{"url":"https://a","statusCode":404,"body":{{"kind":"{kind}","content":"<404>"}},"truncated":true}}"#
        );
        let result: WebFetchResult = serde_json::from_str(&json).unwrap();
        assert_eq!(serde_json::to_string(&result).unwrap(), json);
        match result.body {
            WebFetchBody::Html { content } | WebFetchBody::Text { content } => {
                assert_eq!(content, "<404>")
            }
        }
    }
    assert!(serde_json::from_str::<WebFetchBody>(r#"{"kind":"binary","content":"x"}"#).is_err());
    assert_eq!(
        serde_json::to_string(&fetch_request()).unwrap(),
        r#"{"url":"https://example.com"}"#
    );
    let list: WebProviderList = serde_json::from_str(r#"{"search":[{"id":"b","title":"B","description":{"zh":"中","en":"English"}}],"fetch":[{"id":"f","description":"plain"}]}"#).unwrap();
    assert_eq!(
        serde_json::to_string(&list).unwrap(),
        r#"{"search":[{"id":"b","title":"B","description":{"zh":"中","en":"English"}}],"fetch":[{"id":"f","description":"plain"}]}"#
    );
}

#[test]
fn configuration_has_the_source_keys_defaults_and_volatile_metadata() {
    let config: WebRuntimeConfig = serde_json::from_value(json!({})).unwrap();
    assert_eq!(config.search_provider.0, None);
    assert_eq!(config.fetch_provider.0, None);
    assert_eq!(serde_json::to_value(config).unwrap(), json!({}));
    let schema = hoh_kernel::PluginDef::of::<WebRuntime>().schema().clone();
    assert_eq!(schema["default"], json!({}));
    assert_eq!(
        schema["properties"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["searchProvider", "fetchProvider"]
    );
    for key in ["searchProvider", "fetchProvider"] {
        assert_eq!(schema["properties"][key]["x-volatile"], true);
    }
    assert_eq!(<WebRuntime as Plugin>::NAME, "WebRuntime");
    assert_eq!(<Web as hoh_kernel::ServiceKey>::NAME, "web");
    assert_eq!(
        <WebProvidersChanged as hoh_kernel::Event>::NAME,
        "web/providers-changed"
    );
    assert!(hoh_kernel::config::validate(&schema, &json!({"searchProvider":4})).is_err());
}

#[tokio::test(start_paused = true)]
async fn environment_is_a_captured_fallback_and_empty_ids_are_explicit_selections() {
    let m = mount_env(
        json!({"searchProvider":"config"}),
        Environment(Some("env".into()), Some("fetch-env".into())),
    )
    .await;
    for id in ["config", "env", ""] {
        m.search(Script::new(id, true));
    }
    m.fetch(Script::new("fetch-env", true));
    assert_eq!(m.marker().await, "config");
    assert_eq!(
        m.web.fetch(fetch_request(), None).await.unwrap(),
        fetch_result("fetch-env")
    );
    m.fiber.commit_volatile(json!({})).await.unwrap();
    assert_eq!(m.marker().await, "env");
    m.fiber
        .commit_volatile(json!({"searchProvider":""}))
        .await
        .unwrap();
    assert_eq!(m.marker().await, "");
    // A new derived caller environment does not change the captured values.
    let caller = m.ctx.with_tag(Environment(Some("later".into()), None));
    m.web
        .register_search_provider(&caller, Arc::new(Script::new("later", true)))
        .unwrap();
    m.fiber.commit_volatile(json!({})).await.unwrap();
    assert_eq!(m.marker().await, "env");
    m.close().await;
    let m = mount_env(json!({}), Environment(Some("".into()), None)).await;
    m.search(Script::new("other", true));
    assert_error(
        m.web.search(request(), None).await.unwrap_err(),
        "WEB_PROVIDER_CONFIGURED_MISSING",
        "configured web provider \"\" is not registered",
    );
    m.close().await;
}

#[tokio::test(start_paused = true)]
async fn fetch_resolution_preserves_order_errors_and_checks_only_the_pinned_provider() {
    let m = mount(json!({"fetchProvider":"missing"})).await;
    assert_error(
        m.web.fetch(fetch_request(), None).await.unwrap_err(),
        "WEB_PROVIDER_CONFIGURED_MISSING",
        "configured web provider \"missing\" is not registered",
    );
    let a = Script::new("a", true);
    let a_calls = a.availability_calls.clone();
    let b = Script::new("b", true);
    let b_calls = b.availability_calls.clone();
    m.fetch(a);
    m.fetch(b);
    m.fiber
        .commit_volatile(json!({"fetchProvider":"b"}))
        .await
        .unwrap();
    assert_eq!(
        m.web.fetch(fetch_request(), None).await.unwrap(),
        fetch_result("b")
    );
    assert_eq!(a_calls.load(Ordering::SeqCst), 0);
    assert_eq!(b_calls.load(Ordering::SeqCst), 1);
    m.fiber.commit_volatile(json!({})).await.unwrap();
    assert!(m.web.fetch_ready(None).await.unwrap());
    assert_error(
        m.web.fetch(fetch_request(), None).await.unwrap_err(),
        "WEB_PROVIDER_AMBIGUOUS",
        "multiple usable web providers are registered (a, b); configure one explicitly",
    );
    assert_eq!(a_calls.load(Ordering::SeqCst), 2);
    assert_eq!(b_calls.load(Ordering::SeqCst), 3);
    m.close().await;
}

#[tokio::test(start_paused = true)]
async fn fetch_duplicate_disposal_and_reregistration_keep_the_other_capability() {
    let m = mount(json!({})).await;
    m.search(Script::new("shared", true));
    let d = m.fetch(Script::new("shared", true));
    assert_error(
        m.web
            .register_fetch_provider(&m.ctx, Arc::new(Script::new("shared", true)))
            .err()
            .unwrap(),
        "WEB_DUPLICATE_PROVIDER",
        "a web provider with id \"shared\" is already registered",
    );
    drop(d.dispose());
    m.fetch(Script::new("shared", true));
    assert_eq!(m.marker().await, "shared");
    assert_eq!(
        m.web.fetch(fetch_request(), None).await.unwrap(),
        fetch_result("shared")
    );
    m.close().await;
}

#[tokio::test(start_paused = true)]
async fn arbitrary_availability_errors_are_selection_failures_but_ready_errors_propagate() {
    for search in [true, false] {
        let m = mount(json!({})).await;
        let availability = SharedError::new(std::io::Error::other("availability"));
        let mut p = Script::new("p", true);
        p.availability_error = Some(availability.clone());
        let dispose = if search { m.search(p) } else { m.fetch(p) };
        let ready = if search {
            m.web.search_ready(None).await
        } else {
            m.web.fetch_ready(None).await
        };
        assert!(ready.unwrap());
        let error = if search {
            m.web.search(request(), None).await.unwrap_err()
        } else {
            m.web.fetch(fetch_request(), None).await.unwrap_err()
        };
        assert!(
            error
                .downcast_ref::<SharedError>()
                .unwrap()
                .ptr_eq(&availability)
        );
        drop(dispose.dispose());
        let readiness = SharedError::new(std::io::Error::other("ready"));
        let mut p = Script::new("p", true);
        p.readiness_error = Some(readiness.clone());
        if search {
            m.search(p);
        } else {
            m.fetch(p);
        }
        let error = if search {
            m.web.search_ready(None).await.unwrap_err()
        } else {
            m.web.fetch_ready(None).await.unwrap_err()
        };
        assert!(
            error
                .downcast_ref::<SharedError>()
                .unwrap()
                .ptr_eq(&readiness)
        );
        m.close().await;
    }
}

#[tokio::test(start_paused = true)]
async fn provider_failures_and_abort_reasons_pass_through_without_wrapping() {
    let m = mount(json!({})).await;
    let controller = AbortController::new();
    let signal = controller.signal();
    let cause = SharedError::new(std::io::Error::other("cause"));
    let error = SharedError::new(
        WebError::new("provider failed", "PROVIDER_CUSTOM_CODE").with_cause(cause.clone()),
    );
    controller.abort_with(error.clone());
    let mut p = Script::new("p", true);
    p.execution_error = Some(error.clone());
    let seen = p.signals.clone();
    m.search(p.clone());
    m.fetch(p);
    for failure in [
        m.web.search(request(), Some(&signal)).await.unwrap_err(),
        m.web
            .fetch(fetch_request(), Some(&signal))
            .await
            .unwrap_err(),
    ] {
        let shared = failure.downcast_ref::<SharedError>().unwrap();
        assert!(shared.ptr_eq(&signal.reason().unwrap()));
        let harness = HarnessError::of(failure.as_ref()).unwrap();
        assert_eq!(harness.code(), "PROVIDER_CUSTOM_CODE");
        assert!(
            harness
                .cause()
                .unwrap()
                .downcast_ref::<SharedError>()
                .unwrap()
                .ptr_eq(&cause)
        );
    }
    assert_eq!(
        *lock(&seen),
        vec![Some(std::ptr::from_ref(&signal) as usize); 2]
    );
    m.close().await;
}

#[tokio::test(start_paused = true)]
async fn non_2xx_fetch_and_preexisting_truncation_are_unchanged() {
    let m = mount(json!({})).await;
    let mut p = Script::new("p", true);
    p.fetch_result.status_code = 503;
    p.fetch_result.body = WebFetchBody::Html {
        content: "<h1>unavailable</h1>".into(),
    };
    p.fetch_result.truncated = true;
    p.search_result.truncated = true;
    let expected_fetch = p.fetch_result.clone();
    let expected_search = p.search_result.clone();
    m.search(p.clone());
    m.fetch(p);
    assert_eq!(
        m.web.fetch(fetch_request(), None).await.unwrap(),
        expected_fetch
    );
    let mut req = request();
    req.max_results = Some(10);
    assert_eq!(m.web.search(req, None).await.unwrap(), expected_search);
    m.close().await;
}

#[tokio::test(start_paused = true)]
async fn zero_equal_and_large_source_caps_preserve_the_source_rules() {
    let result = capped(Some(0), 2).await;
    assert!(result.sources.is_empty());
    assert!(result.truncated);
    let result = capped(Some(2), 2).await;
    assert_eq!(result.sources.len(), 2);
    assert!(!result.truncated);
    let result = capped(Some(u64::MAX), 2).await;
    assert_eq!(result.sources.len(), 2);
    assert!(!result.truncated);
}

#[tokio::test(start_paused = true)]
async fn listing_is_detached_does_not_check_availability_and_ignores_registrant_filters() {
    let m = mount(json!({})).await;
    let script = Script::new("p", false);
    let calls = script.availability_calls.clone();
    let count = Arc::new(AtomicUsize::new(0));
    let seen = count.clone();
    m.ctx
        .on_contained::<WebProvidersChanged>(move |_| {
            seen.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
        .unwrap();
    let caller = m.ctx.with_filter(|_| false);
    m.web
        .register_search_provider(&caller, Arc::new(script))
        .unwrap();
    let mut snapshot = m.web.list_providers();
    snapshot.search[0].id = "changed".into();
    snapshot.search[0].title = Some("changed".into());
    assert_eq!(m.web.list_providers().search, vec![info("p")]);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(count.load(Ordering::SeqCst), 1);
    m.close().await;
}

#[tokio::test(start_paused = true)]
async fn concurrent_duplicate_registration_commits_once_and_cleanup_removes_it() {
    let m = mount(json!({})).await;
    let barrier = Arc::new(std::sync::Barrier::new(8));
    let count = Arc::new(AtomicUsize::new(0));
    let seen = count.clone();
    m.ctx
        .on_contained::<WebProvidersChanged>(move |_| {
            seen.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
        .unwrap();
    let handles = std::thread::scope(|scope| {
        let tasks: Vec<_> = (0..8)
            .map(|_| {
                let web = m.web.clone();
                let ctx = m.ctx.clone();
                let barrier = barrier.clone();
                scope.spawn(move || {
                    barrier.wait();
                    web.register_search_provider(&ctx, Arc::new(Script::new("p", true)))
                })
            })
            .collect();
        tasks
            .into_iter()
            .map(|t| t.join().unwrap())
            .collect::<Vec<_>>()
    });
    let mut successful = Vec::new();
    for result in handles {
        match result {
            Ok(d) => successful.push(d),
            Err(error) => assert_error(
                error,
                "WEB_DUPLICATE_PROVIDER",
                "a web provider with id \"p\" is already registered",
            ),
        }
    }
    assert_eq!(successful.len(), 1);
    assert_eq!(count.load(Ordering::SeqCst), 1);
    drop(successful.pop().unwrap().dispose());
    assert!(m.web.list_providers().search.is_empty());
    assert_eq!(count.load(Ordering::SeqCst), 2);
    m.close().await;
}

#[tokio::test(start_paused = true)]
async fn source_plugin_provides_and_withdraws_the_web_service() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let fiber = ctx
        .plugin::<WebRuntime>(json!({"searchProvider":"p","fetchProvider":"p"}))
        .unwrap();
    fiber.settled().await.unwrap();
    {
        let web = ctx.get::<Web>().unwrap();
        web.register_search_provider(&ctx, Arc::new(Script::new("p", true)))
            .unwrap();
        web.register_fetch_provider(&ctx, Arc::new(Script::new("p", true)))
            .unwrap();
        assert_eq!(
            web.search(request(), None).await.unwrap(),
            search_result("p")
        );
        assert_eq!(
            web.fetch(fetch_request(), None).await.unwrap(),
            fetch_result("p")
        );
    }
    fiber.dispose().await;
    assert!(ctx.get::<Web>().is_none());
    kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn omitted_root_configuration_uses_the_schemastery_object_default() {
    let m = mount(Value::Null).await;
    assert_eq!(m.fiber.effective_config().as_deref(), Some(&json!({})));
    m.search(Script::new("p", true));
    assert_eq!(m.marker().await, "p");
    m.close().await;
}

#[tokio::test(start_paused = true)]
async fn inactive_callers_cannot_register_but_duplicates_keep_the_source_error_priority() {
    let m = mount(json!({})).await;
    m.search(Script::new("p", true));
    let child = m.ctx.child("closed registrant").unwrap();
    child.fiber().dispose().await;
    assert_error(
        m.web
            .register_search_provider(&child, Arc::new(Script::new("p", true)))
            .err()
            .unwrap(),
        "WEB_DUPLICATE_PROVIDER",
        "a web provider with id \"p\" is already registered",
    );
    let error = m
        .web
        .register_search_provider(&child, Arc::new(Script::new("new", true)))
        .err()
        .unwrap();
    assert!(matches!(
        error.downcast_ref::<hoh_kernel::KernelError>(),
        Some(hoh_kernel::KernelError::Inactive)
    ));
    assert_eq!(m.web.list_providers().search, vec![info("p")]);
    m.close().await;
}

#[tokio::test(start_paused = true)]
async fn the_seam_forwards_an_already_aborted_signal_without_cancelling_a_successful_provider() {
    let m = mount(json!({})).await;
    let p = Script::new("p", true);
    let seen = p.signals.clone();
    m.search(p.clone());
    m.fetch(p);
    let controller = AbortController::new();
    controller.abort();
    let signal = controller.signal();
    assert_eq!(
        m.web.search(request(), Some(&signal)).await.unwrap(),
        search_result("p")
    );
    assert_eq!(
        m.web.fetch(fetch_request(), Some(&signal)).await.unwrap(),
        fetch_result("p")
    );
    assert_eq!(
        *lock(&seen),
        vec![Some(std::ptr::from_ref(&signal) as usize); 2]
    );
    m.close().await;
}
