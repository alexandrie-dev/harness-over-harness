use futures_util::{FutureExt, future::BoxFuture};
use hoh_error::{BoxError, HarnessError, SharedError};
use hoh_kernel::{Context, Disposer, Kernel, Level};
use hoh_scope::{CreateScopeOptions, ScopeKey, bind_scope_parent, create_scope, scope_of};
use hoh_skill::*;
use hoh_timeout::AbortController;
use serde_json::{Value, json};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::sync::Notify;

type List = dyn Fn(usize, Arc<SkillLookupOptions>) -> BoxFuture<'static, Result<SkillProviderOutput, BoxError>>
    + Send
    + Sync;
type Get = dyn Fn(
        SkillCandidate,
        Arc<SkillLookupOptions>,
    ) -> BoxFuture<'static, Result<Option<SkillDefinition>, BoxError>>
    + Send
    + Sync;
struct Memory {
    name: String,
    candidates: Mutex<Vec<SkillCandidate>>,
    calls: AtomicUsize,
    get_calls: AtomicUsize,
    list: Option<Arc<List>>,
    get: Option<Arc<Get>>,
}
impl Memory {
    fn new(name: &str, candidates: Vec<SkillCandidate>) -> Self {
        Self {
            name: name.into(),
            candidates: Mutex::new(candidates),
            calls: AtomicUsize::new(0),
            get_calls: AtomicUsize::new(0),
            list: None,
            get: None,
        }
    }
    fn list_with(
        mut self,
        list: impl Fn(
            usize,
            Arc<SkillLookupOptions>,
        ) -> BoxFuture<'static, Result<SkillProviderOutput, BoxError>>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        self.list = Some(Arc::new(list));
        self
    }
    fn get_with(
        mut self,
        get: impl Fn(
            SkillCandidate,
            Arc<SkillLookupOptions>,
        ) -> BoxFuture<'static, Result<Option<SkillDefinition>, BoxError>>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        self.get = Some(Arc::new(get));
        self
    }
    fn replace(&self, candidates: Vec<SkillCandidate>) {
        *self.candidates.lock().unwrap() = candidates;
    }
    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}
impl SkillProvider for Memory {
    fn name(&self) -> &str {
        &self.name
    }
    fn list(
        &self,
        options: Arc<SkillLookupOptions>,
    ) -> BoxFuture<'static, Result<SkillProviderOutput, BoxError>> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        if let Some(list) = &self.list {
            return list(call, options);
        }
        futures_util::future::ready(Ok(self.candidates.lock().unwrap().clone().into())).boxed()
    }
    fn get(
        &self,
        candidate: SkillCandidate,
        options: Arc<SkillLookupOptions>,
    ) -> BoxFuture<'static, Result<Option<SkillDefinition>, BoxError>> {
        self.get_calls.fetch_add(1, Ordering::SeqCst);
        if let Some(get) = &self.get {
            return get(candidate, options);
        }
        let content = candidate
            .get("locator")
            .and_then(|v| v.get("content"))
            .cloned()
            .unwrap_or(json!("Body."));
        futures_util::future::ready(Ok(Some(
            SkillDefinition::from(&candidate).with_field("content", content),
        )))
        .boxed()
    }
}
fn skill(name: &str, description: &str, rank: f64) -> SkillCandidate {
    json!({"name":name,"description":description,"invocation":{"modelInvocable":true,"userInvocable":true},"provider":"memory","source":"memory","rank":rank,"locator":{"content":format!("{name} body.")}}).into()
}
fn registration(name: &str, description: &str) -> SkillRegistration {
    json!({"name":name,"description":description,"source":"runtime","content":format!("{name} body.")}).into()
}
fn setup() -> (Kernel, Context, Arc<SkillRegistry>) {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let registry = Arc::new(SkillRegistry::new(ctx.clone(), Config::default()).unwrap());
    (kernel, ctx, registry)
}
fn options() -> Arc<SkillViewOptions> {
    Arc::new(SkillViewOptions::default())
}
fn scoped(ctx: &Context) -> Arc<SkillViewOptions> {
    Arc::new(SkillViewOptions {
        scope: scope_of(ctx),
        ..Default::default()
    })
}
fn add(registry: &SkillRegistry, ctx: &Context, provider: Arc<Memory>) -> Disposer {
    registry.register_provider(ctx, |_| Ok(provider)).unwrap()
}
fn controlled(
    registry: &SkillRegistry,
    ctx: &Context,
    provider: Arc<Memory>,
) -> (Disposer, SkillProviderControl) {
    let mut control = None;
    let disposer = registry
        .register_provider(ctx, |given| {
            control = Some(given);
            Ok(provider)
        })
        .unwrap();
    (disposer, control.unwrap())
}
fn names(skills: &[SkillSummary]) -> Vec<&str> {
    skills.iter().map(|s| s.text("name").unwrap()).collect()
}
fn warnings(ctx: &Context) -> Vec<String> {
    ctx.logger()
        .buffer()
        .into_iter()
        .filter(|r| r.level == Level::Warn)
        .map(|r| r.message)
        .collect()
}
fn invalid_error(error: &BoxError, expected: &str) {
    let error = HarnessError::of(error.as_ref()).unwrap();
    assert_eq!(error.name(), "SkillError");
    assert_eq!(error.code(), "SKILL_INVALID_DEFINITION");
    assert!(error.message().contains(expected), "{error}");
}

#[tokio::test(start_paused = true)]
async fn registers_providers_resolves_duplicates_first_wins_and_disposes_providers() {
    let (_kernel, ctx, registry) = setup();
    let memory = Arc::new(Memory::new(
        "memory",
        vec![
            skill("z-skill", "Z skill", 20.0),
            skill("a-skill", "A skill", 10.0),
            skill("shadowed", "Lower priority", 20.0),
        ],
    ));
    let dispose = add(&registry, &ctx, memory);
    add(
        &registry,
        &ctx,
        Arc::new(Memory::new(
            "override",
            vec![
                skill("shadowed", "Higher priority", 5.0)
                    .with_field("provider", json!("override"))
                    .with_field("locator", json!({"content":"Override body."})),
            ],
        )),
    );
    let listed = registry.list(options()).await.unwrap();
    assert_eq!(
        listed
            .iter()
            .map(|skill| (
                skill.text("name").unwrap(),
                skill.text("description").unwrap(),
                skill.text("provider").unwrap(),
            ))
            .collect::<Vec<_>>(),
        [
            ("a-skill", "A skill", "memory"),
            ("shadowed", "Higher priority", "override"),
            ("z-skill", "Z skill", "memory"),
        ]
    );
    assert_eq!(
        registry
            .get("shadowed", options())
            .await
            .unwrap()
            .unwrap()
            .text("content"),
        Some("Override body.")
    );
    add(
        &registry,
        &ctx,
        Arc::new(Memory::new(
            "same-rank",
            vec![
                skill("same-rank-skill", "Same rank", 10.0)
                    .with_field("provider", json!("same-rank")),
            ],
        )),
    );
    assert_eq!(
        registry
            .list(options())
            .await
            .unwrap()
            .iter()
            .find(|s| s.text("name") == Some("same-rank-skill"))
            .unwrap()
            .text("provider"),
        Some("same-rank")
    );
    assert!(
        registry
            .register_provider(&ctx, |_| Ok(Arc::new(Memory::new("memory", vec![]))))
            .err()
            .unwrap()
            .to_string()
            .contains("already registered")
    );
    let mut rejected = None;
    assert!(
        registry
            .register_provider(&ctx, |control| {
                rejected = Some(control.signal);
                Ok(Arc::new(Memory::new("runtime", vec![])))
            })
            .err()
            .unwrap()
            .to_string()
            .contains("reserved")
    );
    assert!(rejected.unwrap().aborted());
    let failure = SharedError::new(NativeError::Error("factory failed".into()));
    let mut failed = None;
    let error = registry
        .register_provider(&ctx, |control| {
            failed = Some(control.signal);
            Err(failure.clone().into())
        })
        .err()
        .unwrap();
    assert!(SharedError::of(error.as_ref()).unwrap().ptr_eq(&failure));
    assert!(failed.unwrap().reason().unwrap().ptr_eq(&failure));
    let closed = ctx.child("closed").unwrap();
    closed.fiber().dispose().await;
    let mut signal = None;
    let error = registry
        .register_provider(&closed, |control| {
            signal = Some(control.signal);
            Ok(Arc::new(Memory::new("effect-provider", vec![])))
        })
        .err()
        .unwrap();
    assert!(
        signal
            .unwrap()
            .reason()
            .unwrap()
            .ptr_eq(SharedError::of(error.as_ref()).unwrap())
    );
    dispose.dispose().await;
    assert_eq!(
        names(&registry.list(options()).await.unwrap()),
        ["same-rank-skill", "shadowed"]
    );
}

#[tokio::test(start_paused = true)]
async fn returns_an_invocation_neutral_catalog_and_resolves_model_and_user_policy_independently() {
    let (_kernel, ctx, registry) = setup();
    for (name, policy) in [
        ("both", None),
        ("model-only", Some((true, false))),
        ("user-only", Some((false, true))),
        ("trusted-only", Some((false, false))),
    ] {
        let mut value = registration(name, name);
        if let Some((m, u)) = policy {
            value = value.with_field("invocation", json!({"modelInvocable":m,"userInvocable":u}));
        }
        registry.register(&ctx, value).unwrap();
    }
    let listed = registry.list(options()).await.unwrap();
    assert_eq!(
        names(&listed),
        ["both", "model-only", "trusted-only", "user-only"]
    );
    assert_eq!(
        listed
            .iter()
            .find(|skill| skill.text("name") == Some("both"))
            .unwrap()
            .get("invocation"),
        Some(&json!({"modelInvocable":true,"userInvocable":true}))
    );
    assert_eq!(
        names(
            &listed
                .iter()
                .filter(|s| is_model_invocable(s).unwrap())
                .cloned()
                .collect::<Vec<_>>()
        ),
        ["both", "model-only"]
    );
    assert_eq!(
        names(
            &listed
                .iter()
                .filter(|s| is_user_invocable(s).unwrap())
                .cloned()
                .collect::<Vec<_>>()
        ),
        ["both", "user-only"]
    );
    assert_eq!(
        registry
            .get("trusted-only", options())
            .await
            .unwrap()
            .unwrap()
            .text("content"),
        Some("trusted-only body.")
    );
    assert_eq!(
        registry
            .get("both", options())
            .await
            .unwrap()
            .unwrap()
            .get("invocation"),
        Some(&json!({"modelInvocable":true,"userInvocable":true}))
    );
}

#[tokio::test(start_paused = true)]
async fn validates_parsed_candidate_fields() {
    for (field, value, expected) in [
        (
            "description",
            json!({"value":"object-description"}),
            "non-string description",
        ),
        (
            "invocation",
            json!({"modelInvocable":"false","userInvocable":true}),
            "non-boolean invocation.modelInvocable",
        ),
    ] {
        let (_kernel, ctx, registry) = setup();
        add(
            &registry,
            &ctx,
            Arc::new(Memory::new(
                "memory",
                vec![skill("bad-candidate", "Bad", 1.0).with_field(field, value)],
            )),
        );
        invalid_error(&registry.list(options()).await.unwrap_err(), expected);
    }
}

#[tokio::test(start_paused = true)]
async fn rejects_malformed_provider_results_and_every_malformed_candidate_scalar() {
    for output in [
        Value::Null,
        json!(1),
        json!({}),
        json!({"candidates":[],"complete":"yes"}),
    ] {
        let (_kernel, ctx, registry) = setup();
        add(
            &registry,
            &ctx,
            Arc::new(Memory::new("memory", vec![]).list_with(move |_, _| {
                futures_util::future::ready(Ok(SkillProviderOutput::Raw(output.clone()))).boxed()
            })),
        );
        for _ in 0..2 {
            invalid_error(
                &registry.list(options()).await.unwrap_err(),
                "list() must return an array or { candidates, complete } observation",
            );
        }
    }
    for (field, value, expected) in [
        (
            "name",
            json!({"value":"candidate"}),
            "non-string skill name",
        ),
        ("whenToUse", json!(1), "non-string whenToUse"),
        ("source", json!({}), "non-string source"),
        ("rank", json!("1"), "invalid rank"),
        ("provider", json!({}), "non-string provider"),
        ("path", json!(1), "non-string path"),
    ] {
        let (_kernel, ctx, registry) = setup();
        add(
            &registry,
            &ctx,
            Arc::new(Memory::new(
                "memory",
                vec![skill("candidate", "Candidate", 1.0).with_field(field, value)],
            )),
        );
        for _ in 0..2 {
            invalid_error(&registry.list(options()).await.unwrap_err(), expected);
        }
    }
}

#[tokio::test(start_paused = true)]
async fn borrows_the_exact_lookup_options_through_discovery_and_loading() {
    let (_kernel, ctx, registry) = setup();
    let options = Arc::new(SkillViewOptions {
        cwd: Some("/workspace/a".into()),
        ..Default::default()
    });
    let candidate = skill("skill-a", "Skill A", 1.0);
    let listed = Arc::new(Mutex::new(None));
    let loaded = Arc::new(Mutex::new(None));
    let l = listed.clone();
    let g = loaded.clone();
    let c = candidate.clone();
    add(
        &registry,
        &ctx,
        Arc::new(
            Memory::new("memory", vec![])
                .list_with(move |_, o| {
                    *l.lock().unwrap() = Some(o);
                    futures_util::future::ready(Ok(vec![c.clone()].into())).boxed()
                })
                .get_with(move |received, o| {
                    assert!(received.ptr_eq(&candidate));
                    *g.lock().unwrap() = Some(o);
                    futures_util::future::ready(Ok(Some(
                        SkillDefinition::from(&received)
                            .with_field("content", json!("Skill A body.")),
                    )))
                    .boxed()
                }),
        ),
    );
    assert_eq!(
        names(&registry.list(options.clone()).await.unwrap()),
        ["skill-a"]
    );
    assert_eq!(
        registry
            .get("skill-a", options.clone())
            .await
            .unwrap()
            .unwrap()
            .text("content"),
        Some("Skill A body.")
    );
    assert!(Arc::ptr_eq(
        listed.lock().unwrap().as_ref().unwrap(),
        &options
    ));
    assert!(Arc::ptr_eq(
        loaded.lock().unwrap().as_ref().unwrap(),
        &options
    ));
}

#[tokio::test(start_paused = true)]
async fn rechecks_cancellation_after_cached_discovery_before_provider_loading() {
    let (_kernel, ctx, registry) = setup();
    let provider = Arc::new(Memory::new(
        "memory",
        vec![skill("cached-skill", "Cached", 1.0)],
    ));
    add(&registry, &ctx, provider.clone());
    registry.list(options()).await.unwrap();
    let controller = AbortController::new();
    let reason = SharedError::new(NativeError::Error(
        "cancelled after cached discovery".into(),
    ));
    let lookup = Arc::new(SkillViewOptions {
        signal: Some(controller.signal()),
        ..Default::default()
    });
    let pending = registry.get("cached-skill", lookup);
    controller.abort_with(reason.clone());
    let error = pending.await.unwrap_err();
    assert!(SharedError::of(error.as_ref()).unwrap().ptr_eq(&reason));
    assert_eq!(provider.get_calls.load(Ordering::SeqCst), 0);
}

struct AbortAfterListing {
    controller: Arc<AbortController>,
    reason: SharedError,
    name_calls: AtomicUsize,
    get_calls: AtomicUsize,
}
impl SkillProvider for AbortAfterListing {
    fn name(&self) -> &str {
        if self.name_calls.fetch_add(1, Ordering::SeqCst) == 1 {
            self.controller.abort_with(self.reason.clone());
        }
        "abort-after-listing"
    }

    fn list(
        &self,
        _options: Arc<SkillLookupOptions>,
    ) -> BoxFuture<'static, Result<SkillProviderOutput, BoxError>> {
        futures_util::future::ready(Ok(vec![
            skill("listed-before-abort", "Listed before abort", 1.0)
                .with_field("provider", json!("abort-after-listing")),
        ]
        .into()))
        .boxed()
    }

    fn get(
        &self,
        _candidate: SkillCandidate,
        _options: Arc<SkillLookupOptions>,
    ) -> BoxFuture<'static, Result<Option<SkillDefinition>, BoxError>> {
        self.get_calls.fetch_add(1, Ordering::SeqCst);
        futures_util::future::ready(Ok(None)).boxed()
    }
}

#[tokio::test(start_paused = true)]
async fn rechecks_cancellation_after_provider_listing_before_returning_the_catalog() {
    let (_kernel, ctx, registry) = setup();
    let controller = Arc::new(AbortController::new());
    let reason = SharedError::new(NativeError::Error(
        "cancelled after provider listing".into(),
    ));
    let provider = Arc::new(AbortAfterListing {
        controller: controller.clone(),
        reason: reason.clone(),
        name_calls: AtomicUsize::new(0),
        get_calls: AtomicUsize::new(0),
    });
    registry
        .register_provider(&ctx, |_| Ok(provider.clone()))
        .unwrap();
    assert_eq!(provider.name_calls.load(Ordering::SeqCst), 1);

    let error = registry
        .list(Arc::new(SkillViewOptions {
            signal: Some(controller.signal()),
            ..Default::default()
        }))
        .await
        .unwrap_err();

    assert!(SharedError::of(error.as_ref()).unwrap().ptr_eq(&reason));
    assert_eq!(provider.get_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test(start_paused = true)]
async fn stops_waiting_for_cached_provider_loading_when_a_hostile_abort_reason_fires() {
    let (_kernel, ctx, registry) = setup();
    let started = Arc::new(Notify::new());
    let held = Arc::new(Notify::new());
    let observed = Arc::new(Mutex::new(None));
    let s = started.clone();
    let h = held.clone();
    let o = observed.clone();
    add(
        &registry,
        &ctx,
        Arc::new(
            Memory::new("memory", vec![skill("held-skill", "Held", 1.0)]).get_with(
                move |c, options| {
                    let s = s.clone();
                    let h = h.clone();
                    *o.lock().unwrap() = Some(options);
                    async move {
                        s.notify_one();
                        h.notified().await;
                        Ok(Some(
                            SkillDefinition::from(&c).with_field("content", json!("Held body.")),
                        ))
                    }
                    .boxed()
                },
            ),
        ),
    );
    registry.list(options()).await.unwrap();
    let controller = AbortController::new();
    let reason = SharedError::new(SkillThrownValue::Unrenderable);
    let lookup = Arc::new(SkillViewOptions {
        signal: Some(controller.signal()),
        ..Default::default()
    });
    let r = registry.clone();
    let l = lookup.clone();
    let pending = tokio::spawn(async move { r.get("held-skill", l).await });
    started.notified().await;
    controller.abort_with(reason.clone());
    let error = tokio::time::timeout(Duration::from_secs(1), pending)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert_eq!(error.to_string(), "[unrenderable thrown value]");
    assert!(Arc::ptr_eq(
        observed.lock().unwrap().as_ref().unwrap(),
        &lookup
    ));
    held.notify_one();
}

#[tokio::test(start_paused = true)]
async fn borrows_cached_candidates_and_loaded_definitions_from_the_provider() {
    let (_kernel, ctx, registry) = setup();
    let locator = Arc::new(AtomicUsize::new(42));
    let candidate = skill("stable-skill", "Stable description", 1.0)
        .with_field("whenToUse", json!("When stability matters."))
        .with_field("path", json!("/skills/stable/SKILL.md"))
        .with_field(
            "resourceBase",
            json!({"kind":"opaque","description":"candidate resources"}),
        )
        .with_locator(locator.clone());
    let definition = SkillDefinition::from(&candidate)
        .with_field(
            "resourceBase",
            json!({"kind":"opaque","description":"definition resources"}),
        )
        .with_field("content", json!("Stable body."));
    let c = candidate.clone();
    let d = definition.clone();
    let l = locator.clone();
    let provider = Arc::new(Memory::new("memory", vec![candidate.clone()]).get_with(
        move |received, _| {
            assert!(received.ptr_eq(&c));
            assert!(Arc::ptr_eq(&received.locator::<AtomicUsize>().unwrap(), &l));
            futures_util::future::ready(Ok(Some(d.clone()))).boxed()
        },
    ));
    add(&registry, &ctx, provider.clone());
    let listed = registry.list(options()).await.unwrap();
    for field in ["path", "resourceBase", "invocation"] {
        assert!(Arc::ptr_eq(
            &listed[0].shared_field(field).unwrap(),
            &candidate.shared_field(field).unwrap()
        ));
    }
    assert!(
        registry
            .get("stable-skill", options())
            .await
            .unwrap()
            .unwrap()
            .ptr_eq(&definition)
    );
    assert_eq!(provider.calls(), 1);
}

#[tokio::test(start_paused = true)]
async fn preserves_readonly_runtime_resource_identities_while_adding_the_default_provider() {
    let (_kernel, ctx, registry) = setup();
    let registration = registration("runtime-skill", "Runtime")
        .with_field(
            "resourceBase",
            json!({"kind":"opaque","description":"runtime resources"}),
        )
        .with_field("metadata", json!({"owner":"runtime"}))
        .with_field(
            "invocation",
            json!({"modelInvocable":true,"userInvocable":true}),
        );
    registry.register(&ctx, registration.clone()).unwrap();
    registry
        .register(
            &ctx,
            crate::registration("z-runtime", "Second runtime skill"),
        )
        .unwrap();
    let listed = registry.list(options()).await.unwrap();
    let loaded = registry
        .get("runtime-skill", options())
        .await
        .unwrap()
        .unwrap();
    for field in ["resourceBase", "invocation"] {
        assert!(Arc::ptr_eq(
            &listed[0].shared_field(field).unwrap(),
            &registration.shared_field(field).unwrap()
        ));
    }
    for field in ["resourceBase", "metadata", "invocation"] {
        assert!(Arc::ptr_eq(
            &loaded.shared_field(field).unwrap(),
            &registration.shared_field(field).unwrap()
        ));
    }
    assert_eq!(loaded.text("provider"), Some("runtime"));
}

#[tokio::test(start_paused = true)]
async fn rejects_every_malformed_scalar_in_provider_loaded_definitions() {
    for (field, value, expected) in [
        (
            "name",
            json!({"value":"loaded"}),
            "loaded skill name must be a string",
        ),
        ("name", json!("Bad_Name"), "loaded skill has invalid name"),
        ("description", json!({}), "description must be a string"),
        ("description", json!(""), "requires a description"),
        ("invocation", Value::Null, "non-object invocation policy"),
        (
            "invocation",
            json!({"modelInvocable":"false","userInvocable":true}),
            "invocation.modelInvocable",
        ),
        (
            "invocation",
            json!({"modelInvocable":true,"userInvocable":"true"}),
            "invocation.userInvocable",
        ),
        (
            "invocation",
            json!({"userInvocable":true}),
            "invocation.modelInvocable",
        ),
        (
            "invocation",
            json!({"modelInvocable":true}),
            "invocation.userInvocable",
        ),
        ("whenToUse", json!(1), "whenToUse must be a string"),
        ("source", json!({}), "source must be a string"),
        ("provider", json!({}), "provider must be a string"),
        ("content", json!({}), "content must be a string"),
        ("path", json!(1), "path must be a string"),
    ] {
        let (_kernel, ctx, registry) = setup();
        add(
            &registry,
            &ctx,
            Arc::new(
                Memory::new("memory", vec![skill("definition", "Candidate", 1.0)]).get_with(
                    move |c, _| {
                        futures_util::future::ready(Ok(Some(
                            SkillDefinition::from(&c)
                                .with_field("content", json!("Definition body."))
                                .with_field(field, value.clone()),
                        )))
                        .boxed()
                    },
                ),
            ),
        );
        for _ in 0..2 {
            invalid_error(
                &registry.get("definition", options()).await.unwrap_err(),
                expected,
            );
        }
    }
}

#[tokio::test(start_paused = true)]
async fn validates_provider_candidates_and_invalid_registry_caps() {
    let (_kernel, ctx, registry) = setup();
    assert!(registry.list(options()).await.unwrap().is_empty());
    for (candidate, expected) in [
        (skill("Bad_Name", "bad", 1.0), "invalid skill name"),
        (skill("empty-description", "", 1.0), "without a description"),
        (skill("bad-rank", "Bad rank", f64::NAN), "invalid rank"),
        (
            skill("wrong-provider", "Wrong provider", 1.0)
                .with_field("provider", json!("different")),
            "for provider \"different\"",
        ),
    ] {
        let registry = SkillRegistry::new(ctx.clone(), Config::default()).unwrap();
        add(
            &registry,
            &ctx,
            Arc::new(Memory::new("memory", vec![candidate])),
        );
        invalid_error(&registry.list(options()).await.unwrap_err(), expected);
    }
    let error = SkillRegistry::new(
        ctx,
        Config {
            collect_cache_max_entries: 1.5,
        },
    )
    .err()
    .unwrap();
    assert_eq!(
        error.to_string(),
        "skill: collectCacheMaxEntries must be an integer greater than or equal to 1"
    );
    assert!(HarnessError::of(error.as_ref()).is_none());
}

#[tokio::test(start_paused = true)]
async fn sorts_model_visible_summaries_without_locale_sensitive_collation() {
    let (_kernel, ctx, registry) = setup();
    add(
        &registry,
        &ctx,
        Arc::new(Memory::new(
            "memory",
            vec![
                skill("z-skill", "Z skill", 10.0),
                skill("a-skill", "A skill", 10.0),
                skill("a-skill", "Duplicate A skill", 20.0),
            ],
        )),
    );
    assert_eq!(
        names(&registry.list(options()).await.unwrap()),
        ["a-skill", "z-skill"]
    );
}

#[tokio::test(start_paused = true)]
async fn caches_provider_discovery_skips_failing_providers_and_invalidates_on_runtime_skills() {
    let (_kernel, ctx, _) = setup();
    let registry = SkillRegistry::new(
        ctx.clone(),
        Config {
            collect_cache_max_entries: 1.0,
        },
    )
    .unwrap();
    let provider = Arc::new(Memory::new(
        "memory",
        vec![skill("first-skill", "First", 10.0)],
    ));
    add(&registry, &ctx, provider.clone());
    assert_eq!(
        names(&registry.list(options()).await.unwrap()),
        ["first-skill"]
    );
    provider.replace(vec![skill("second-skill", "Second", 10.0)]);
    assert_eq!(
        names(&registry.list(options()).await.unwrap()),
        ["first-skill"]
    );
    let runtime = registry
        .register(
            &ctx,
            registration("runtime-skill", "Runtime")
                .with_field("path", json!("memory://runtime-skill"))
                .with_field("metadata", json!({"owner":"tests"})),
        )
        .unwrap()
        .unwrap();
    assert_eq!(
        names(&registry.list(options()).await.unwrap()),
        ["runtime-skill", "second-skill"]
    );
    let loaded = registry
        .get("runtime-skill", options())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(loaded.text("path"), Some("memory://runtime-skill"));
    assert_eq!(loaded.get("metadata"), Some(&json!({"owner":"tests"})));
    runtime.dispose().await;
    for cwd in [
        "/tmp/first-cache-key",
        "/tmp/second-cache-key",
        "/tmp/first-cache-key",
    ] {
        registry
            .list(Arc::new(SkillViewOptions {
                cwd: Some(cwd.into()),
                ..Default::default()
            }))
            .await
            .unwrap();
    }
    assert_eq!(provider.calls(), 5); // initial, invalidation, and FIFO eviction misses
    let fail = Arc::new(AtomicBool::new(true));
    let f = fail.clone();
    let flaky = Arc::new(Memory::new("flaky", vec![]).list_with(move |_, _| {
        futures_util::future::ready(if f.load(Ordering::SeqCst) {
            Err(NativeError::Error("transient discovery failure".into()).into())
        } else {
            Ok(
                vec![skill("flaky-skill", "Flaky", 10.0).with_field("provider", json!("flaky"))]
                    .into(),
            )
        })
        .boxed()
    }));
    add(&registry, &ctx, flaky.clone());
    let snapshot = registry.snapshot(options()).await.unwrap();
    assert!(!snapshot.complete);
    assert_eq!(names(&snapshot.skills), ["second-skill"]);
    assert_eq!(flaky.calls(), 1);
    registry.list(options()).await.unwrap();
    assert_eq!(flaky.calls(), 2);
    fail.store(false, Ordering::SeqCst);
    for _ in 0..2 {
        assert_eq!(
            names(&registry.list(options()).await.unwrap()),
            ["flaky-skill", "second-skill"]
        );
    }
    assert_eq!(flaky.calls(), 3);
    assert!(
        warnings(&ctx)
            .iter()
            .any(|w| w == "skill provider \"flaky\" skipped: Error: transient discovery failure")
    );
}

#[tokio::test(start_paused = true)]
async fn keeps_candidates_from_incomplete_provider_observations_loadable_without_caching_them() {
    let (_kernel, ctx, registry) = setup();
    let provider = Arc::new(Memory::new("memory", vec![]).list_with(|_, _| {
        futures_util::future::ready(Ok(SkillProviderOutput::Observation(
            SkillProviderObservation {
                candidates: vec![skill("available-skill", "Available", 10.0)],
                complete: false,
            },
        )))
        .boxed()
    }));
    add(&registry, &ctx, provider.clone());
    let snapshot = registry.snapshot(options()).await.unwrap();
    assert!(!snapshot.complete);
    assert_eq!(names(&snapshot.skills), ["available-skill"]);
    assert_eq!(
        registry
            .get("available-skill", options())
            .await
            .unwrap()
            .unwrap()
            .text("content"),
        Some("available-skill body.")
    );
    assert_eq!(
        names(&registry.list(options()).await.unwrap()),
        ["available-skill"]
    );
    assert_eq!(provider.calls(), 3);
}

#[tokio::test(start_paused = true)]
async fn invalidates_only_the_exact_registered_provider_and_ignores_its_late_callbacks() {
    let (_kernel, ctx, registry) = setup();
    let provider = Arc::new(Memory::new(
        "memory",
        vec![skill("first-skill", "First", 10.0)],
    ));
    let (dispose, control) = controlled(&registry, &ctx, provider.clone());
    assert!(registry.snapshot(options()).await.unwrap().complete);
    provider.replace(vec![skill("second-skill", "Second", 10.0)]);
    assert_eq!(
        names(&registry.list(options()).await.unwrap()),
        ["first-skill"]
    );
    control.invalidate();
    assert_eq!(
        names(&registry.list(options()).await.unwrap()),
        ["second-skill"]
    );
    dispose.dispose().await;
    assert!(control.signal.aborted());
    assert_eq!(
        control.signal.reason().unwrap().to_string(),
        "skill provider \"memory\" disposed"
    );
    let replacement = Arc::new(Memory::new(
        "memory",
        vec![skill("replacement-skill", "Replacement", 10.0)],
    ));
    add(&registry, &ctx, replacement.clone());
    for _ in 0..2 {
        assert_eq!(
            names(&registry.list(options()).await.unwrap()),
            ["replacement-skill"]
        );
        control.invalidate();
    }
    assert_eq!(replacement.calls(), 1);
}

#[tokio::test(start_paused = true)]
async fn emits_catalog_invalidations_for_live_provider_and_runtime_mutations() {
    let (_kernel, ctx, registry) = setup();
    let count = Arc::new(AtomicUsize::new(0));
    let c = count.clone();
    ctx.on_contained::<SkillsChange>(move |_| {
        c.fetch_add(1, Ordering::SeqCst);
        Ok(())
    })
    .unwrap();
    let (provider, control) = controlled(&registry, &ctx, Arc::new(Memory::new("memory", vec![])));
    assert_eq!(count.load(Ordering::SeqCst), 1);
    control.invalidate();
    assert_eq!(count.load(Ordering::SeqCst), 2);
    let runtime = registry
        .register(&ctx, registration("runtime-skill", "Runtime"))
        .unwrap()
        .unwrap();
    assert_eq!(count.load(Ordering::SeqCst), 3);
    runtime.dispose().await;
    assert_eq!(count.load(Ordering::SeqCst), 4);
    provider.dispose().await;
    assert_eq!(count.load(Ordering::SeqCst), 5);
    control.invalidate();
    assert_eq!(count.load(Ordering::SeqCst), 5);
}

#[tokio::test(start_paused = true)]
async fn contains_synchronous_and_asynchronous_catalog_observer_failures() {
    let (_kernel, ctx, registry) = setup();
    let throwing = ctx
        .on_contained::<SkillsChange>(|_| Err(NativeError::Error("observer threw".into()).into()))
        .unwrap();
    let rejecting = ctx
        .on_contained_async::<SkillsChange, _>(|_| async {
            Err(NativeError::Error("observer rejected".into()).into())
        })
        .unwrap();
    let observed = Arc::new(AtomicUsize::new(0));
    let o = observed.clone();
    let observer = ctx
        .on_contained::<SkillsChange>(move |_| {
            o.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
        .unwrap();
    add(&registry, &ctx, Arc::new(Memory::new("memory", vec![])));
    tokio::task::yield_now().await;
    assert_eq!(observed.load(Ordering::SeqCst), 1);
    assert_eq!(
        warnings(&ctx),
        [
            "skills/change listener threw: Error: observer threw",
            "skills/change listener rejected: Error: observer rejected"
        ]
    );
    throwing.dispose().await;
    rejecting.dispose().await;
    observer.dispose().await;
}

#[tokio::test(start_paused = true)]
async fn retries_an_in_flight_catalog_invalidated_by_its_provider() {
    let (_kernel, ctx, registry) = setup();
    let started = Arc::new(Notify::new());
    let gate = Arc::new(Notify::new());
    let s = started.clone();
    let g = gate.clone();
    let provider = Arc::new(Memory::new("memory", vec![]).list_with(move |call, _| {
        let s = s.clone();
        let g = g.clone();
        async move {
            if call == 1 {
                s.notify_one();
                g.notified().await;
                Ok(vec![skill("stale-skill", "Stale", 10.0)].into())
            } else {
                Ok(vec![skill("fresh-skill", "Fresh", 10.0)].into())
            }
        }
        .boxed()
    }));
    let (_dispose, control) = controlled(&registry, &ctx, provider.clone());
    let r = registry.clone();
    let pending = tokio::spawn(async move { r.list(options()).await });
    started.notified().await;
    control.invalidate();
    gate.notify_one();
    assert_eq!(names(&pending.await.unwrap().unwrap()), ["fresh-skill"]);
    assert_eq!(provider.calls(), 2);
}

#[tokio::test(start_paused = true)]
async fn bounds_repeated_in_flight_invalidation_and_leaves_the_result_uncached() {
    let (_kernel, ctx, registry) = setup();
    let provider = Arc::new(Mutex::new(None));
    let p = provider.clone();
    registry
        .register_provider(&ctx, move |control| {
            let value = Arc::new(Memory::new("self-invalidating", vec![]).list_with(
                move |call, _| {
                    control.invalidate();
                    futures_util::future::ready(Ok(vec![
                        skill("bounded-skill", &format!("Attempt {call}"), 10.0)
                            .with_field("provider", json!("self-invalidating")),
                    ]
                    .into()))
                    .boxed()
                },
            ));
            *p.lock().unwrap() = Some(value.clone());
            Ok(value)
        })
        .unwrap();
    let first = registry.snapshot(options()).await.unwrap();
    assert_eq!(
        serde_json::to_value(&first).unwrap(),
        json!({
            "skills": [{
                "name": "bounded-skill",
                "description": "Attempt 2",
                "invocation": {"modelInvocable": true, "userInvocable": true},
                "source": "memory",
                "provider": "self-invalidating"
            }],
            "complete": false
        })
    );
    assert_eq!(provider.lock().unwrap().as_ref().unwrap().calls(), 2);
    assert_eq!(
        registry.snapshot(options()).await.unwrap().skills[0].text("description"),
        Some("Attempt 4")
    );
    assert_eq!(provider.lock().unwrap().as_ref().unwrap().calls(), 4);
}

#[tokio::test(start_paused = true)]
async fn invalidates_a_provider_whose_loaded_definition_changed_identity() {
    let (_kernel, ctx, registry) = setup();
    let provider = Arc::new(
        Memory::new("memory", vec![skill("old-name", "Old name", 1.0)]).get_with(|c, _| {
            futures_util::future::ready(Ok(Some(
                SkillDefinition::from(&c)
                    .with_field("name", json!("new-name"))
                    .with_field("content", json!("Fresh body.")),
            )))
            .boxed()
        }),
    );
    add(&registry, &ctx, provider.clone());
    assert!(registry.get("old-name", options()).await.unwrap().is_none());
    registry.list(options()).await.unwrap();
    assert_eq!(provider.calls(), 2);
}

#[tokio::test(start_paused = true)]
async fn returns_undefined_when_a_discovered_candidate_disappears_before_loading() {
    let (_kernel, ctx, registry) = setup();
    add(
        &registry,
        &ctx,
        Arc::new(
            Memory::new("memory", vec![skill("vanished-skill", "Vanished", 10.0)])
                .get_with(|_, _| futures_util::future::ready(Ok(None)).boxed()),
        ),
    );
    assert!(
        registry
            .get("vanished-skill", options())
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test(start_paused = true)]
async fn propagates_a_load_failure_raced_against_an_armed_abort_signal() {
    let (_kernel, ctx, registry) = setup();
    let failure = SharedError::new(NativeError::Error("load failed".into()));
    let f = failure.clone();
    add(
        &registry,
        &ctx,
        Arc::new(
            Memory::new("memory", vec![skill("failing-skill", "Failing", 10.0)])
                .get_with(move |_, _| futures_util::future::ready(Err(f.clone().into())).boxed()),
        ),
    );
    let error = registry
        .get(
            "failing-skill",
            Arc::new(SkillViewOptions {
                signal: Some(AbortController::new().signal()),
                ..Default::default()
            }),
        )
        .await
        .unwrap_err();
    assert!(SharedError::of(error.as_ref()).unwrap().ptr_eq(&failure));
}

#[derive(Debug)]
struct Hostile;
impl std::fmt::Display for Hostile {
    fn fmt(&self, _: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        panic!("provider failure coercion failed")
    }
}
impl std::error::Error for Hostile {}
#[tokio::test(start_paused = true)]
async fn contains_a_provider_rejection_whose_string_coercion_throws() {
    let (_kernel, ctx, registry) = setup();
    add(
        &registry,
        &ctx,
        Arc::new(Memory::new("hostile-failure", vec![]).list_with(|_, _| {
            futures_util::future::ready(Err(Box::new(Hostile) as BoxError)).boxed()
        })),
    );
    assert!(registry.list(options()).await.unwrap().is_empty());
    assert_eq!(
        warnings(&ctx),
        ["skill provider \"hostile-failure\" skipped: [unrenderable thrown value]"]
    );
}

#[tokio::test(start_paused = true)]
async fn renders_an_error_with_an_empty_message_like_javascript_string() {
    let (_kernel, ctx, registry) = setup();
    add(
        &registry,
        &ctx,
        Arc::new(Memory::new("empty-failure", vec![]).list_with(|_, _| {
            futures_util::future::ready(
                Err(Box::new(NativeError::Error(String::new())) as BoxError),
            )
            .boxed()
        })),
    );

    assert!(registry.list(options()).await.unwrap().is_empty());
    assert_eq!(
        warnings(&ctx),
        ["skill provider \"empty-failure\" skipped: Error"]
    );
}

#[tokio::test(start_paused = true)]
async fn abandons_an_in_flight_catalog_when_provider_registrations_change() {
    let (_kernel, ctx, registry) = setup();
    let started = Arc::new(Notify::new());
    let gate = Arc::new(Notify::new());
    let s = started.clone();
    let g = gate.clone();
    let dispose = add(
        &registry,
        &ctx,
        Arc::new(Memory::new("memory", vec![]).list_with(move |_, _| {
            let s = s.clone();
            let g = g.clone();
            async move {
                s.notify_one();
                g.notified().await;
                Ok(vec![skill("stale-skill", "Stale", 10.0)].into())
            }
            .boxed()
        })),
    );
    let r = registry.clone();
    let pending = tokio::spawn(async move { r.list(options()).await });
    started.notified().await;
    dispose.dispose().await;
    gate.notify_one();
    assert!(pending.await.unwrap().unwrap().is_empty());
}

#[tokio::test(start_paused = true)]
async fn stops_waiting_for_discovery_when_its_lookup_signal_aborts() {
    let (_kernel, ctx, registry) = setup();
    let started = Arc::new(Notify::new());
    let held = Arc::new(Notify::new());
    let seen = Arc::new(Mutex::new(None));
    let s = started.clone();
    let h = held.clone();
    let o = seen.clone();
    add(
        &registry,
        &ctx,
        Arc::new(
            Memory::new("uncooperative", vec![]).list_with(move |_, options| {
                let s = s.clone();
                let h = h.clone();
                *o.lock().unwrap() = Some(options);
                async move {
                    s.notify_one();
                    h.notified().await;
                    Ok(vec![].into())
                }
                .boxed()
            }),
        ),
    );
    let controller = AbortController::new();
    let lookup = Arc::new(SkillViewOptions {
        signal: Some(controller.signal()),
        ..Default::default()
    });
    let r = registry.clone();
    let l = lookup.clone();
    let pending = tokio::spawn(async move { r.list(l).await });
    started.notified().await;
    controller.abort_with(SkillThrownValue::Json(json!("discovery cancelled")));
    let error = tokio::time::timeout(Duration::from_secs(1), pending)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert_eq!(error.to_string(), "discovery cancelled");
    assert!(Arc::ptr_eq(seen.lock().unwrap().as_ref().unwrap(), &lookup));
    held.notify_one();
}

#[tokio::test(start_paused = true)]
async fn rejects_invalid_runtime_skill_registrations_and_ignores_duplicates() {
    let (_kernel, ctx, registry) = setup();
    for (value, expected) in [
        (registration("Bad_Name", "Bad"), "invalid skill name"),
        (registration("no-description", ""), "requires a description"),
        (
            registration("bad-invocation", "Bad invocation").with_field("invocation", json!([])),
            "non-object invocation policy",
        ),
    ] {
        invalid_error(&registry.register(&ctx, value).err().unwrap(), expected);
    }
    assert!(
        registry
            .get("missing-skill", options())
            .await
            .unwrap()
            .is_none()
    );
    assert!(registry.get("Bad_Name", options()).await.unwrap().is_none());
    let first = registry
        .register(&ctx, registration("same-skill", "First"))
        .unwrap()
        .unwrap();
    assert!(
        registry
            .register(&ctx, registration("same-skill", "Second"))
            .unwrap()
            .is_none()
    );
    assert_eq!(
        registry
            .get("same-skill", options())
            .await
            .unwrap()
            .unwrap()
            .text("description"),
        Some("First")
    );
    first.dispose().await;
    assert!(
        registry
            .get("same-skill", options())
            .await
            .unwrap()
            .is_none()
    );
}

#[test]
fn renders_a_directory_based_skill_with_the_shared_wrapper() {
    let text=render_skill_content(&json!({"name":"demo-skill","provider":"memory","resourceBase":{"kind":"directory","path":"/tmp/demo"},"content":"Do the thing."}).into()).unwrap();
    assert_eq!(text,["<skill_content name=\"demo-skill\">","<skill_resources>","Base directory for this skill: /tmp/demo","Resolve relative paths mentioned by this skill against the base directory before using them. Load referenced resources only as needed.","</skill_resources>","","<skill_instructions>","Do the thing.","</skill_instructions>","</skill_content>"].join("\n"));
}
#[test]
fn renders_url_and_opaque_resource_hints() {
    let url=render_skill_content(&json!({"name":"url-skill","provider":"memory","resourceBase":{"kind":"url","url":"https://example.test/base/"},"content":"Body."}).into()).unwrap();
    assert!(url.contains("Base URL for this skill: https://example.test/base/"));
    assert!(url.contains(
        "Resolve relative URLs mentioned by this skill against the base URL before using them."
    ));
    let opaque=render_skill_content(&json!({"name":"opaque-skill","provider":"memory","resourceBase":{"kind":"opaque","description":"archive <bundle>"},"content":"Body."}).into()).unwrap();
    assert!(opaque.contains("Resources for this skill: archive &lt;bundle&gt;"));
}
#[test]
fn falls_back_to_the_provider_hint_without_a_resource_base() {
    let text = render_skill_content(
        &json!({"name":"provider-skill","provider":"remote <hub>","content":"Body."}).into(),
    )
    .unwrap();
    assert!(
        text.contains("Resources for this skill are managed by provider \"remote &lt;hub&gt;\".")
    );
}
#[test]
fn escapes_hostile_attribute_names_and_keeps_the_body_verbatim() {
    let text=render_skill_content(&json!({"name":"x\"&<y","provider":"memory","resourceBase":{"kind":"directory","path":"/tmp"},"content":"Keep </skill_content> and <tags> as-is."}).into()).unwrap();
    assert!(text.contains("<skill_content name=\"x&quot;&amp;&lt;y\">"));
    assert!(text.contains("Keep </skill_content> and <tags> as-is."));
}

#[tokio::test(start_paused = true)]
async fn files_a_scoped_provider_into_its_layer_and_merges_it_into_that_scope_view_only() {
    let (_kernel, ctx, registry) = setup();
    add(
        &registry,
        &ctx,
        Arc::new(Memory::new(
            "memory",
            vec![skill("global-skill", "Global", 100.0)],
        )),
    );
    let preset = create_scope(&ctx, ScopeKey::new(), CreateScopeOptions::default()).unwrap();
    add(
        &registry,
        &preset.ctx,
        Arc::new(Memory::new(
            "preset-local",
            vec![
                skill("preset-skill", "Preset", 300.0)
                    .with_field("provider", json!("preset-local"))
                    .with_field("locator", json!({"content":"Preset body."})),
            ],
        )),
    );
    assert_eq!(
        names(&registry.list(options()).await.unwrap()),
        ["global-skill"]
    );
    assert_eq!(
        names(&registry.list(scoped(&preset.ctx)).await.unwrap()),
        ["global-skill", "preset-skill"]
    );
    assert_eq!(
        registry
            .get("preset-skill", scoped(&preset.ctx))
            .await
            .unwrap()
            .unwrap()
            .text("content"),
        Some("Preset body.")
    );
    assert!(
        registry
            .get("preset-skill", options())
            .await
            .unwrap()
            .is_none()
    );
    preset.dispose().await;
}
#[tokio::test(start_paused = true)]
async fn lets_the_nearest_layer_win_a_duplicate_name_regardless_of_rank() {
    let (_kernel, ctx, registry) = setup();
    add(
        &registry,
        &ctx,
        Arc::new(Memory::new(
            "memory",
            vec![skill("shared-name", "Global wins ranks", 10.0)],
        )),
    );
    let preset = create_scope(&ctx, ScopeKey::new(), CreateScopeOptions::default()).unwrap();
    add(
        &registry,
        &preset.ctx,
        Arc::new(Memory::new(
            "preset-local",
            vec![
                skill("shared-name", "Preset shadow", 900.0)
                    .with_field("provider", json!("preset-local"))
                    .with_field("locator", json!({"content":"Preset shadow body."})),
            ],
        )),
    );
    let listed = registry.list(scoped(&preset.ctx)).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].text("description"), Some("Preset shadow"));
    assert_eq!(
        registry
            .get("shared-name", scoped(&preset.ctx))
            .await
            .unwrap()
            .unwrap()
            .text("content"),
        Some("Preset shadow body.")
    );
    assert_eq!(
        registry.list(options()).await.unwrap()[0].text("description"),
        Some("Global wins ranks")
    );
    preset.dispose().await;
}
#[tokio::test(start_paused = true)]
async fn resolves_the_scope_chain_so_an_agent_key_inherits_its_preset_layer_and_recompose_follows_the_new_parent()
 {
    let (_kernel, ctx, registry) = setup();
    let a = create_scope(&ctx, ScopeKey::new(), CreateScopeOptions::default()).unwrap();
    let b = create_scope(&ctx, ScopeKey::new(), CreateScopeOptions::default()).unwrap();
    for (scope, label) in [(&a, "a"), (&b, "b")] {
        registry
            .register(
                &scope.ctx,
                registration(&format!("skill-{label}"), &format!("Skill {label}")),
            )
            .unwrap();
    }
    let key = ScopeKey::new();
    let binding = bind_scope_parent(&key, scope_of(&a.ctx).unwrap()).unwrap();
    let lookup = Arc::new(SkillViewOptions {
        scope: Some(key),
        ..Default::default()
    });
    assert_eq!(
        names(&registry.list(lookup.clone()).await.unwrap()),
        ["skill-a"]
    );
    binding.rebind(scope_of(&b.ctx).unwrap()).unwrap();
    assert_eq!(names(&registry.list(lookup).await.unwrap()), ["skill-b"]);
    a.dispose().await;
    b.dispose().await;
}
#[tokio::test(start_paused = true)]
async fn scopes_provider_name_uniqueness_per_layer_and_reports_scoped_duplicates_distinctly() {
    let (_kernel, ctx, registry) = setup();
    add(&registry, &ctx, Arc::new(Memory::new("memory", vec![])));
    let a = create_scope(&ctx, ScopeKey::new(), CreateScopeOptions::default()).unwrap();
    let b = create_scope(&ctx, ScopeKey::new(), CreateScopeOptions::default()).unwrap();
    for (scope, label) in [(&a, "a"), (&b, "b")] {
        add(
            &registry,
            &scope.ctx,
            Arc::new(Memory::new(
                "memory",
                vec![skill(&format!("{label}-only"), label, 100.0)],
            )),
        );
    }
    let error = registry
        .register_provider(&a.ctx, |_| Ok(Arc::new(Memory::new("memory", vec![]))))
        .err()
        .unwrap();
    assert_eq!(
        error.to_string(),
        "a skill provider named \"memory\" is already registered in this scope"
    );
    assert!(HarnessError::of(error.as_ref()).is_none());
    assert_eq!(
        names(&registry.list(scoped(&a.ctx)).await.unwrap()),
        ["a-only"]
    );
    assert_eq!(
        names(&registry.list(scoped(&b.ctx)).await.unwrap()),
        ["b-only"]
    );
    a.dispose().await;
    b.dispose().await;
}
#[tokio::test(start_paused = true)]
async fn keeps_runtime_duplicate_handling_per_layer_and_shadows_a_global_runtime_name() {
    let (_kernel, ctx, registry) = setup();
    registry
        .register(
            &ctx,
            registration("told-twice", "Global runtime")
                .with_field("content", json!("Global body.")),
        )
        .unwrap();
    let preset = create_scope(&ctx, ScopeKey::new(), CreateScopeOptions::default()).unwrap();
    let shadow = registry
        .register(
            &preset.ctx,
            registration("told-twice", "Preset runtime")
                .with_field("content", json!("Preset body.")),
        )
        .unwrap()
        .unwrap();
    assert!(warnings(&ctx).is_empty());
    assert!(
        registry
            .register(&preset.ctx, registration("told-twice", "Ignored"))
            .unwrap()
            .is_none()
    );
    assert_eq!(
        warnings(&ctx),
        ["runtime skill \"told-twice\" ignored because it is already registered"]
    );
    assert_eq!(
        registry
            .get("told-twice", scoped(&preset.ctx))
            .await
            .unwrap()
            .unwrap()
            .text("content"),
        Some("Preset body.")
    );
    assert_eq!(
        registry
            .get("told-twice", options())
            .await
            .unwrap()
            .unwrap()
            .text("content"),
        Some("Global body.")
    );
    shadow.dispose().await;
    assert_eq!(
        registry
            .get("told-twice", scoped(&preset.ctx))
            .await
            .unwrap()
            .unwrap()
            .text("content"),
        Some("Global body.")
    );
    preset.dispose().await;
}
#[tokio::test(start_paused = true)]
async fn drops_a_disposed_scoped_registration_from_its_scope_view_and_notifies_change() {
    let (_kernel, ctx, registry) = setup();
    let changes = Arc::new(AtomicUsize::new(0));
    let c = changes.clone();
    ctx.on_contained::<SkillsChange>(move |_| {
        c.fetch_add(1, Ordering::SeqCst);
        Ok(())
    })
    .unwrap();
    let preset = create_scope(&ctx, ScopeKey::new(), CreateScopeOptions::default()).unwrap();
    let lookup = scoped(&preset.ctx);
    add(
        &registry,
        &preset.ctx,
        Arc::new(Memory::new(
            "memory",
            vec![skill("scoped-skill", "Scoped", 100.0)],
        )),
    );
    assert_eq!(
        names(&registry.list(lookup.clone()).await.unwrap()),
        ["scoped-skill"]
    );
    let notified = changes.load(Ordering::SeqCst);
    preset.dispose().await;
    assert!(changes.load(Ordering::SeqCst) > notified);
    assert!(registry.list(lookup).await.unwrap().is_empty());
}
#[tokio::test(start_paused = true)]
async fn invalidates_through_a_scoped_provider_control_only_while_its_exact_registration_is_live() {
    let (_kernel, ctx, registry) = setup();
    let preset = create_scope(&ctx, ScopeKey::new(), CreateScopeOptions::default()).unwrap();
    let provider = Arc::new(Memory::new(
        "memory",
        vec![skill("watched", "Watched", 100.0)],
    ));
    let (dispose, control) = controlled(&registry, &preset.ctx, provider.clone());
    let lookup = scoped(&preset.ctx);
    assert_eq!(
        names(&registry.list(lookup.clone()).await.unwrap()),
        ["watched"]
    );
    provider.replace(vec![skill("replaced", "Replaced", 100.0)]);
    control.invalidate();
    assert_eq!(
        names(&registry.list(lookup.clone()).await.unwrap()),
        ["replaced"]
    );
    dispose.dispose().await;
    provider.replace(vec![skill("ignored", "Ignored", 100.0)]);
    control.invalidate();
    assert!(registry.list(lookup).await.unwrap().is_empty());
    preset.dispose().await;
}

#[tokio::test(start_paused = true)]
async fn plugin_publishes_skills_with_the_source_name_and_config_default() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let fiber = ctx.plugin::<SkillRegistry>(json!({})).unwrap();
    fiber.settled().await.unwrap();
    assert_eq!(fiber.name().as_ref(), "SkillRegistry");
    assert_eq!(fiber.raw_config().as_ref(), &json!({}));
    assert_eq!(Config::default().collect_cache_max_entries, 128.0);
    assert_eq!(
        serde_json::to_string(&Config::default()).unwrap(),
        r#"{"collectCacheMaxEntries":128}"#
    );
    let registry = ctx.get::<Skills>().unwrap();
    assert!(registry.list(options()).await.unwrap().is_empty());
    registry
        .register(&ctx, registration("published", "Published"))
        .unwrap();
    assert!(
        registry
            .get("published", options())
            .await
            .unwrap()
            .is_some()
    );
    let invalid = ctx
        .plugin::<SkillRegistry>(json!({"collectCacheMaxEntries":1.5}))
        .unwrap();
    assert!(
        invalid
            .settled()
            .await
            .unwrap_err()
            .to_string()
            .contains("collectCacheMaxEntries")
    );
    kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn keeps_the_typed_boundary_for_malformed_runtime_metadata() {
    let missing_invocation = SkillSummary::from_value(json!({"name":"missing-invocation"}));
    for error in [
        is_model_invocable(&missing_invocation).unwrap_err(),
        is_user_invocable(&missing_invocation).unwrap_err(),
    ] {
        assert!(matches!(
            error.downcast_ref::<NativeError>(),
            Some(NativeError::Type(_))
        ));
    }

    let (_kernel, ctx, registry) = setup();
    let missing_source = SkillRegistration::from_value(json!({
        "name": "missing-source",
        "description": "Accepted by the source validator",
        "content": "Body."
    }));
    registry.register(&ctx, missing_source).unwrap();
    assert!(
        registry
            .list(options())
            .await
            .unwrap()
            .iter()
            .find(|skill| skill.text("name") == Some("missing-source"))
            .unwrap()
            .get("source")
            .is_none()
    );

    let numeric_name = SkillRegistration::from_value(json!({
        "name": 123,
        "description": "Outside the typed contract",
        "source": "runtime",
        "content": "Body."
    }));
    let error = match registry.register(&ctx, numeric_name) {
        Ok(_) => panic!("numeric skill name unexpectedly registered"),
        Err(error) => error,
    };
    invalid_error(&error, "invalid skill name \"123\"");
}

#[tokio::test(start_paused = true)]
async fn summary_and_invocation_source_keep_source_wire_order_and_omission() {
    let (_kernel, ctx, registry) = setup();
    add(
        &registry,
        &ctx,
        Arc::new(Memory::new(
            "memory",
            vec![
                skill("demo-skill", "Description", 1.0)
                    .with_field("path", json!("/skills/demo/SKILL.md"))
                    .with_field("whenToUse", json!("When needed."))
                    .with_field(
                        "resourceBase",
                        json!({"kind":"directory","path":"/skills/demo"}),
                    )
                    .with_field("metadata", json!({"hidden":true})),
            ],
        )),
    );
    let listed = registry.list(options()).await.unwrap();
    assert_eq!(
        hoh_util_values::js_json_stringify(&serde_json::to_value(&listed).unwrap()),
        r#"[{"name":"demo-skill","path":"/skills/demo/SKILL.md","description":"Description","whenToUse":"When needed.","invocation":{"modelInvocable":true,"userInvocable":true},"source":"memory","provider":"memory","resourceBase":{"kind":"directory","path":"/skills/demo"}}]"#
    );
    let source = SkillInvocationSource {
        name: "demo-skill".into(),
    };
    assert_eq!(
        hoh_util_values::js_json_stringify(&serde_json::to_value(&source).unwrap()),
        r#"{"kind":"skill-invocation","name":"demo-skill","form":"instructions"}"#
    );
    let decoded: SkillInvocationSource =
        serde_json::from_value(serde_json::to_value(source).unwrap()).unwrap();
    assert_eq!(decoded.name, "demo-skill");
    let minimal = SkillRegistry::new(ctx.clone(), Config::default()).unwrap();
    minimal
        .register(&ctx, registration("minimal", "Minimal"))
        .unwrap();
    assert_eq!(
        hoh_util_values::js_json_stringify(
            &serde_json::to_value(minimal.list(options()).await.unwrap()).unwrap()
        ),
        r#"[{"name":"minimal","description":"Minimal","invocation":{"modelInvocable":true,"userInvocable":true},"source":"runtime","provider":"runtime"}]"#
    );
}

#[tokio::test(start_paused = true)]
async fn rank_ties_runtime_precedence_and_local_order_follow_the_source() {
    let (_kernel, ctx, registry) = setup();
    let candidates = vec![
        skill("local-tie", "First local", 20.5),
        skill("local-tie", "Second local", 20.5),
        skill("provider-tie", "First provider", 20.5),
        skill("runtime-tie", "Provider rank 250", 250.0),
        skill("project-wins", "Project", 100.0),
        skill("runtime-wins", "User", 300.0),
    ];
    add(&registry, &ctx, Arc::new(Memory::new("memory", candidates)));
    add(
        &registry,
        &ctx,
        Arc::new(Memory::new(
            "later",
            vec![
                skill("provider-tie", "Later provider", 20.5)
                    .with_field("provider", json!("later")),
            ],
        )),
    );
    for name in ["runtime-tie", "project-wins", "runtime-wins"] {
        registry
            .register(&ctx, registration(name, "Runtime"))
            .unwrap();
    }
    for (name, expected) in [
        ("local-tie", "First local"),
        ("provider-tie", "First provider"),
        ("runtime-tie", "Runtime"),
        ("project-wins", "Project"),
        ("runtime-wins", "Runtime"),
    ] {
        assert_eq!(
            registry
                .get(name, options())
                .await
                .unwrap()
                .unwrap()
                .text("description"),
            Some(expected)
        );
    }
    assert!(warnings(&ctx).contains(&"skill \"local-tie\" from memory ignored because a higher-priority skill already exists".into()));
}

#[tokio::test(start_paused = true)]
async fn catalog_change_ignores_the_registering_context_receiver_filter() {
    let (_kernel, ctx, registry) = setup();
    let count = Arc::new(AtomicUsize::new(0));
    let c = count.clone();
    ctx.on_contained::<SkillsChange>(move |_| {
        c.fetch_add(1, Ordering::SeqCst);
        Ok(())
    })
    .unwrap();
    let filtered = ctx.with_filter(|_| false);
    add(
        &registry,
        &filtered,
        Arc::new(Memory::new("memory", vec![])),
    );
    registry
        .register(&filtered, registration("runtime-skill", "Runtime"))
        .unwrap();
    assert_eq!(count.load(Ordering::SeqCst), 2);
    assert_eq!(
        names(&registry.list(options()).await.unwrap()),
        ["runtime-skill"]
    );
}

#[test]
fn skill_error_codes_name_and_cause_are_shared_harness_errors() {
    let codes = [
        SkillErrorCode::SkillNotFound,
        SkillErrorCode::SkillNotModelInvocable,
        SkillErrorCode::SkillInvalidDefinition,
        SkillErrorCode::SkillReadFailed,
    ];
    for (code, text) in codes.into_iter().zip(SKILL_ERROR_CODES) {
        let error: BoxError = SkillError::new("source message", code)
            .with_cause(NativeError::Error("cause".into()))
            .into();
        let harness = HarnessError::of(error.as_ref()).unwrap();
        assert_eq!(harness.name(), "SkillError");
        assert_eq!(harness.code(), text);
        assert_eq!(harness.message(), "source message");
        assert_eq!(harness.cause().unwrap().to_string(), "cause");
    }
}

#[test]
fn raw_renderer_preserves_source_coercion_order_and_native_failures() {
    let definition =
        SkillDefinition::from_value(json!({"name":"demo","provider":"memory","content":"Body."}));
    for (base, expected) in [
        (
            Value::Null,
            "Cannot read properties of null (reading 'kind')",
        ),
        (
            json!({"kind":"directory"}),
            "Cannot read properties of undefined (reading 'replaceAll')",
        ),
        (
            json!({"kind":"opaque","description":1}),
            "value.replaceAll is not a function",
        ),
        (
            json!({"kind":"future"}),
            "unreachable variant in SkillResourceBase.kind: {\"kind\":\"future\"}",
        ),
    ] {
        let error =
            render_skill_content(&definition.clone().with_field("resourceBase", base)).unwrap_err();
        assert_eq!(error.to_string(), expected);
        assert!(HarnessError::of(error.as_ref()).is_none());
    }
    assert!(
        render_skill_content(&definition.clone().with_field("content", Value::Null))
            .unwrap()
            .contains("<skill_instructions>\n\n</skill_instructions>")
    );
    assert_eq!(escape_text("&<>\"'"), "&amp;&lt;&gt;\"'");
}

#[tokio::test(start_paused = true)]
async fn null_provider_entries_keep_native_type_errors_outside_harness_routing() {
    let (_kernel, ctx, registry) = setup();
    add(
        &registry,
        &ctx,
        Arc::new(Memory::new("memory", vec![Value::Null.into()])),
    );
    let error = registry.list(options()).await.unwrap_err();
    assert_eq!(
        error.to_string(),
        "Cannot read properties of null (reading 'name')"
    );
    assert!(HarnessError::of(error.as_ref()).is_none());
    let other = SkillRegistry::new(ctx.clone(), Config::default()).unwrap();
    add(
        &other,
        &ctx,
        Arc::new(
            Memory::new("memory", vec![skill("null-body", "Null body", 1.0)])
                .get_with(|_, _| futures_util::future::ready(Ok(Some(Value::Null.into()))).boxed()),
        ),
    );
    let error = other.get("null-body", options()).await.unwrap_err();
    assert_eq!(
        error.to_string(),
        "Cannot read properties of null (reading 'name')"
    );
    assert!(HarnessError::of(error.as_ref()).is_none());
}

#[test]
fn skill_names_match_the_entire_ascii_grammar_without_trimming() {
    for name in ["a", "0", "skill-1", "a0-b1-c2"] {
        assert!(is_skill_name(name), "{name:?}");
    }
    for name in [
        "",
        "Bad_Name",
        "a--b",
        "-a",
        "a-",
        "a b",
        "a\n",
        "a\r",
        "a\u{2028}",
        "a\u{2029}",
        "\u{feff}a",
        "a\u{85}",
        "навык",
    ] {
        assert!(!is_skill_name(name), "{name:?}");
    }
}
