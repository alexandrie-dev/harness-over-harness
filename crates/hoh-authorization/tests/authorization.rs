use std::sync::{
    Arc, Mutex, MutexGuard,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

use hoh_authorization::{
    Authorization, AuthorizationDeclinedError, AuthorizationEntry, AuthorizationError,
    AuthorizationFlow, AuthorizationInteraction, AuthorizationMethod, AuthorizationNotice,
    AuthorizationOutcome, AuthorizationPlugin, AuthorizationPrompt, AuthorizationRequest,
    AuthorizationService, AuthorizationSettled, AuthorizationSettlement, AuthorizationStatus,
};
use hoh_credentials::{
    CredentialFuture, CredentialInfo, CredentialKey, CredentialMutator, CredentialProvider,
    CredentialRecord, CredentialRecordEntry, CredentialRecordInfo, CredentialRef, Credentials,
    GrantRecord, ResolvedCredential, credential_key,
};
use hoh_error::{BoxError, HarnessError, SharedError};
use hoh_kernel::{Context, Kernel, Plugin};
use hoh_timeout::{AbortController, AbortError, is_abort_error};
use serde_json::json;
use tokio::sync::{Notify, oneshot};

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn key() -> CredentialKey {
    credential_key("llm-pi-ai", "openai-codex").unwrap()
}

fn other_key() -> CredentialKey {
    credential_key("llm-pi-ai", "anthropic").unwrap()
}

fn grant(token: &str) -> CredentialRecord {
    CredentialRecord::Grant(GrantRecord {
        payload: json!({ "token": token }),
    })
}

struct WriteGate {
    admitted: Notify,
    release: Notify,
}

#[derive(Default)]
struct MemoryState {
    records: std::collections::HashMap<CredentialKey, CredentialRecord>,
    gate: Option<Arc<WriteGate>>,
}

struct MemoryCredentials {
    ctx: Context,
    state: Mutex<MemoryState>,
}

impl MemoryCredentials {
    fn new(ctx: Context) -> Self {
        Self {
            ctx,
            state: Mutex::new(MemoryState::default()),
        }
    }

    fn gate(&self, gate: Arc<WriteGate>) {
        lock(&self.state).gate = Some(gate);
    }
}

impl CredentialProvider for MemoryCredentials {
    fn resolve<'a>(
        &'a self,
        _reference: &'a CredentialRef,
    ) -> CredentialFuture<'a, Option<ResolvedCredential>> {
        Box::pin(async { Ok(None) })
    }

    fn describe<'a>(
        &'a self,
        _reference: &'a CredentialRef,
    ) -> CredentialFuture<'a, CredentialInfo> {
        Box::pin(async {
            Ok(CredentialInfo {
                configured: false,
                source: None,
                writable: true,
                masked: None,
            })
        })
    }

    fn set<'a>(
        &'a self,
        _reference: &'a CredentialRef,
        _value: &'a str,
    ) -> CredentialFuture<'a, ()> {
        Box::pin(async { Ok(()) })
    }

    fn unset<'a>(&'a self, _reference: &'a CredentialRef) -> CredentialFuture<'a, ()> {
        Box::pin(async { Ok(()) })
    }

    fn read_record<'a>(
        &'a self,
        key: &'a CredentialKey,
    ) -> CredentialFuture<'a, Option<CredentialRecord>> {
        Box::pin(async move { Ok(lock(&self.state).records.get(key).cloned()) })
    }

    fn describe_record<'a>(
        &'a self,
        key: &'a CredentialKey,
    ) -> CredentialFuture<'a, CredentialRecordInfo> {
        Box::pin(async move {
            let stored = lock(&self.state).records.get(key).cloned();
            Ok(CredentialRecordInfo {
                configured: stored.is_some(),
                kind: stored.as_ref().map(CredentialRecord::kind),
                writable: true,
            })
        })
    }

    fn list_records(&self) -> CredentialFuture<'_, Vec<CredentialRecordEntry>> {
        Box::pin(async move {
            Ok(lock(&self.state)
                .records
                .iter()
                .map(|(key, record)| CredentialRecordEntry {
                    key: key.clone(),
                    kind: record.kind(),
                })
                .collect())
        })
    }

    fn modify_record<'a>(
        &'a self,
        key: &'a CredentialKey,
        mutate: CredentialMutator,
    ) -> CredentialFuture<'a, Option<CredentialRecord>> {
        Box::pin(async move {
            let gate = lock(&self.state).gate.clone();
            if let Some(gate) = gate {
                gate.admitted.notify_one();
                gate.release.notified().await;
            }
            let current = lock(&self.state).records.get(key).cloned();
            let next = mutate(current.clone()).await?;
            let Some(next) = next else {
                return Ok(current);
            };
            lock(&self.state).records.insert(key.clone(), next.clone());
            self.notify_record_updated(&self.ctx, key);
            Ok(Some(next))
        })
    }

    fn delete_record<'a>(&'a self, key: &'a CredentialKey) -> CredentialFuture<'a, ()> {
        Box::pin(async move {
            if lock(&self.state).records.remove(key).is_some() {
                self.notify_record_updated(&self.ctx, key);
            }
            Ok(())
        })
    }
}

struct Harness {
    kernel: Kernel,
    ctx: Context,
    credentials: Arc<MemoryCredentials>,
    service: AuthorizationService,
}

async fn harness() -> Harness {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let credentials = Arc::new(MemoryCredentials::new(ctx.clone()));
    ctx.provide::<Credentials>(credentials.clone())
        .await
        .unwrap();
    let service = AuthorizationService::new(ctx.clone(), ctx.get::<Credentials>().unwrap());
    Harness {
        kernel,
        ctx,
        credentials,
        service,
    }
}

#[derive(Default)]
struct Surface {
    notices: Mutex<Vec<AuthorizationNotice>>,
    prompts: Mutex<Vec<serde_json::Value>>,
    answer: Mutex<Option<Result<String, HarnessError>>>,
    fail_notice: AtomicBool,
}

impl Surface {
    fn answering(answer: &str) -> Arc<Self> {
        Arc::new(Self {
            answer: Mutex::new(Some(Ok(answer.into()))),
            ..Self::default()
        })
    }

    fn declining() -> Arc<Self> {
        Arc::new(Self {
            answer: Mutex::new(Some(Err(AuthorizationDeclinedError::new()))),
            ..Self::default()
        })
    }
}

impl AuthorizationInteraction for Surface {
    fn notify(&self, notice: AuthorizationNotice) -> Result<(), BoxError> {
        if self.fail_notice.load(Ordering::Acquire) {
            return Err("page connection closed".into());
        }
        lock(&self.notices).push(notice);
        Ok(())
    }

    fn prompt(
        &self,
        prompt: AuthorizationPrompt,
    ) -> hoh_authorization::AuthorizationFuture<String> {
        lock(&self.prompts).push(serde_json::to_value(&prompt).unwrap());
        let answer = lock(&self.answer)
            .take()
            .unwrap_or_else(|| Ok("typed".into()));
        Box::pin(async move { answer.map_err(Into::into) })
    }
}

fn method(id: &str, label: &str) -> AuthorizationMethod {
    AuthorizationMethod {
        id: id.into(),
        label: label.into(),
    }
}

fn request(interaction: Arc<dyn AuthorizationInteraction>) -> AuthorizationRequest {
    AuthorizationRequest {
        key: key(),
        method: None,
        interaction,
        signal: None,
    }
}

fn committing_flow(
    credentials: Arc<MemoryCredentials>,
    before: impl Fn(
        hoh_authorization::AuthorizationSession,
    ) -> hoh_authorization::AuthorizationFuture<()>
    + Send
    + Sync
    + 'static,
) -> AuthorizationFlow {
    AuthorizationFlow::new(
        key(),
        "ChatGPT (Codex)",
        method("oauth", "Sign in with ChatGPT"),
        [method("api-key", "Paste a key")],
        move |session| {
            let credentials = credentials.clone();
            let before = before(session.clone());
            async move {
                before.await?;
                credentials
                    .modify_record(
                        &key(),
                        Box::new(|_| Box::pin(async { Ok(Some(grant("granted"))) })),
                    )
                    .await?;
                Ok(())
            }
        },
    )
}

fn ready(_: hoh_authorization::AuthorizationSession) -> hoh_authorization::AuthorizationFuture<()> {
    Box::pin(async { Ok(()) })
}

fn assert_authorization_error(error: &BoxError, message: &str, code: &str) {
    let error = HarnessError::of(error.as_ref()).expect("HarnessError");
    assert_eq!(error.name(), AuthorizationError::NAME);
    assert_eq!(error.message(), message);
    assert_eq!(error.code(), code);
}

#[tokio::test]
async fn plugin_publishes_the_authorization_service_with_the_source_name() {
    assert_eq!(AuthorizationPlugin::NAME, "AuthorizationService");
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let credentials = Arc::new(MemoryCredentials::new(ctx.clone()));
    ctx.provide::<Credentials>(credentials).await.unwrap();
    ctx.plugin::<AuthorizationPlugin>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    assert!(ctx.get::<Authorization>().is_some());
    kernel.shutdown().await;
}

#[tokio::test]
async fn registry_preserves_order_rejects_duplicates_and_disposes_flows() {
    let h = harness().await;
    let first = h
        .service
        .register_flow(&h.ctx, committing_flow(h.credentials.clone(), ready))
        .unwrap();
    let other = AuthorizationFlow::new(
        other_key(),
        "Anthropic",
        method("api-key", "Paste a key"),
        [],
        |_| async { Ok(()) },
    );
    let second = h.service.register_flow(&h.ctx, other).unwrap();
    assert_eq!(
        h.service.list(),
        vec![
            AuthorizationEntry {
                key: key(),
                label: "ChatGPT (Codex)".into(),
                methods: vec![
                    method("oauth", "Sign in with ChatGPT"),
                    method("api-key", "Paste a key")
                ],
                in_flight: false,
            },
            AuthorizationEntry {
                key: other_key(),
                label: "Anthropic".into(),
                methods: vec![method("api-key", "Paste a key")],
                in_flight: false,
            },
        ]
    );
    let error = match h
        .service
        .register_flow(&h.ctx, committing_flow(h.credentials.clone(), ready))
    {
        Ok(_) => panic!("duplicate flow was accepted"),
        Err(error) => error,
    };
    assert_authorization_error(
        &error,
        "an authorization flow for \"llm-pi-ai/openai-codex\" is already registered",
        "DUPLICATE_FLOW",
    );
    first.dispose().await;
    assert!(h.service.describe(&key()).is_none());
    assert!(h.service.describe(&other_key()).is_some());
    second.dispose().await;
    h.kernel.shutdown().await;
}

#[tokio::test]
async fn successful_attempt_uses_methods_carries_interaction_and_settles() {
    let h = harness().await;
    let seen = Arc::new(Mutex::new(Vec::new()));
    let seen_run = seen.clone();
    h.service
        .register_flow(
            &h.ctx,
            committing_flow(h.credentials.clone(), move |session| {
                lock(&seen_run).push(session.method().to_owned());
                session.notify(AuthorizationNotice {
                    message: "Continue in your browser".into(),
                    url: Some("https://auth.example/start".into()),
                    code: None,
                });
                Box::pin(async move {
                    assert_eq!(
                        session
                            .prompt(AuthorizationPrompt::Text {
                                message: "Paste the code".into(),
                                placeholder: None,
                                signal: None,
                            })
                            .await?,
                        "code-123"
                    );
                    Ok(())
                })
            }),
        )
        .unwrap();
    let settlements = Arc::new(Mutex::new(Vec::new()));
    let recorded = settlements.clone();
    h.ctx
        .on_contained::<AuthorizationSettled>(move |event| {
            lock(&recorded).push(event.clone());
            Ok(())
        })
        .unwrap();
    let surface = Surface::answering("code-123");
    assert_eq!(
        h.service.begin(request(surface.clone())).await.unwrap(),
        AuthorizationOutcome {
            status: AuthorizationStatus::Authorized
        }
    );
    let mut named = request(Surface::answering("code-123"));
    named.method = Some("api-key".into());
    assert_eq!(
        h.service.begin(named).await.unwrap().status,
        AuthorizationStatus::Authorized
    );
    assert_eq!(&*lock(&seen), &["oauth", "api-key"]);
    assert_eq!(
        *lock(&surface.notices),
        vec![AuthorizationNotice {
            message: "Continue in your browser".into(),
            url: Some("https://auth.example/start".into()),
            code: None,
        }]
    );
    assert_eq!(
        lock(&surface.prompts)[0],
        json!({"kind":"text","message":"Paste the code"})
    );
    assert_eq!(
        lock(&settlements)[0].settlement,
        AuthorizationSettlement::Authorized
    );
    assert_eq!(
        h.credentials.read_record(&key()).await.unwrap(),
        Some(grant("granted"))
    );
    h.kernel.shutdown().await;
}

#[tokio::test]
async fn validation_order_and_exact_errors_match_the_source() {
    let h = harness().await;
    let settlements = Arc::new(AtomicUsize::new(0));
    let recorded = settlements.clone();
    h.ctx
        .on_contained::<AuthorizationSettled>(move |_| {
            recorded.fetch_add(1, Ordering::AcqRel);
            Ok(())
        })
        .unwrap();
    let controller = AbortController::new();
    controller.abort();
    let mut missing = request(Surface::answering("unused"));
    missing.signal = Some(controller.signal());
    let error = h.service.begin(missing).await.unwrap_err();
    assert_authorization_error(
        &error,
        "no authorization flow is registered for \"llm-pi-ai/openai-codex\"",
        "NO_FLOW",
    );
    let ran = Arc::new(AtomicBool::new(false));
    let ran_flow = ran.clone();
    h.service
        .register_flow(
            &h.ctx,
            committing_flow(h.credentials.clone(), move |_| {
                ran_flow.store(true, Ordering::Release);
                Box::pin(async { Ok(()) })
            }),
        )
        .unwrap();
    let mut unknown = request(Surface::answering("unused"));
    unknown.method = Some("device".into());
    unknown.signal = Some(controller.signal());
    let error = h.service.begin(unknown).await.unwrap_err();
    assert_authorization_error(
        &error,
        "authorization flow for \"llm-pi-ai/openai-codex\" offers no method \"device\"",
        "UNKNOWN_METHOD",
    );
    let pre_aborted = request(Surface::answering("unused"));
    let mut pre_aborted = pre_aborted;
    pre_aborted.signal = Some(controller.signal());
    assert_eq!(
        h.service.begin(pre_aborted).await.unwrap().status,
        AuthorizationStatus::Cancelled
    );
    assert!(!ran.load(Ordering::Acquire));
    assert!(!h.service.describe(&key()).unwrap().in_flight);
    assert_eq!(settlements.load(Ordering::Acquire), 0);
    h.kernel.shutdown().await;
}

#[tokio::test]
async fn concurrent_attempt_is_refused_then_key_is_released() {
    let h = harness().await;
    h.service.cancel(&key());
    h.service.cancel(&other_key());
    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let first = Arc::new(AtomicBool::new(true));
    let started_run = started.clone();
    let release_run = release.clone();
    let first_run = first.clone();
    h.service
        .register_flow(
            &h.ctx,
            committing_flow(h.credentials.clone(), move |_| {
                let started = started_run.clone();
                let release = release_run.clone();
                let is_first = first_run.swap(false, Ordering::AcqRel);
                Box::pin(async move {
                    if is_first {
                        started.notify_one();
                        release.notified().await;
                    }
                    Ok(())
                })
            }),
        )
        .unwrap();
    let service = h.service.clone();
    let running =
        tokio::spawn(async move { service.begin(request(Surface::answering("unused"))).await });
    started.notified().await;
    assert!(h.service.describe(&key()).unwrap().in_flight);
    h.service.cancel(&other_key());
    assert!(h.service.describe(&key()).unwrap().in_flight);
    let error = h
        .service
        .begin(request(Surface::answering("unused")))
        .await
        .unwrap_err();
    assert_authorization_error(
        &error,
        "an authorization attempt for \"llm-pi-ai/openai-codex\" is already running",
        "ALREADY_IN_FLIGHT",
    );
    release.notify_one();
    assert_eq!(
        running.await.unwrap().unwrap().status,
        AuthorizationStatus::Authorized
    );
    assert!(!h.service.describe(&key()).unwrap().in_flight);
    assert_eq!(
        h.service
            .begin(request(Surface::answering("unused")))
            .await
            .unwrap()
            .status,
        AuthorizationStatus::Authorized
    );
    h.kernel.shutdown().await;
}

#[tokio::test]
async fn dropping_the_awaiting_caller_does_not_drop_attempt_cleanup() {
    let h = harness().await;
    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let first = Arc::new(AtomicBool::new(true));
    let started_run = started.clone();
    let release_run = release.clone();
    let first_run = first.clone();
    h.service
        .register_flow(
            &h.ctx,
            committing_flow(h.credentials.clone(), move |_| {
                let started = started_run.clone();
                let release = release_run.clone();
                let is_first = first_run.swap(false, Ordering::AcqRel);
                Box::pin(async move {
                    if is_first {
                        started.notify_one();
                        release.notified().await;
                    }
                    Ok(())
                })
            }),
        )
        .unwrap();
    let settled = Arc::new(Notify::new());
    let settlement = Arc::new(Mutex::new(None));
    let settled_listener = settled.clone();
    let settlement_listener = settlement.clone();
    h.ctx
        .on_contained::<AuthorizationSettled>(move |event| {
            *lock(&settlement_listener) = Some(event.settlement);
            settled_listener.notify_one();
            Ok(())
        })
        .unwrap();

    let service = h.service.clone();
    let waiter =
        tokio::spawn(async move { service.begin(request(Surface::answering("unused"))).await });
    started.notified().await;
    waiter.abort();
    assert!(waiter.await.unwrap_err().is_cancelled());
    assert!(h.service.describe(&key()).unwrap().in_flight);

    release.notify_one();
    settled.notified().await;
    assert_eq!(
        *lock(&settlement),
        Some(AuthorizationSettlement::Authorized)
    );
    assert!(!h.service.describe(&key()).unwrap().in_flight);
    assert_eq!(
        h.service
            .begin(request(Surface::answering("unused")))
            .await
            .unwrap()
            .status,
        AuthorizationStatus::Authorized
    );
    h.kernel.shutdown().await;
}

#[tokio::test]
async fn caller_and_local_cancellation_settle_even_for_uncooperative_flows() {
    for caller in [false, true] {
        let h = harness().await;
        let started = Arc::new(Notify::new());
        let never = Arc::new(Notify::new());
        let finished = Arc::new(Notify::new());
        let started_run = started.clone();
        let never_run = never.clone();
        let finished_run = finished.clone();
        h.service
            .register_flow(
                &h.ctx,
                AuthorizationFlow::new(
                    key(),
                    "Account",
                    method("browser", "Browser"),
                    [],
                    move |_| {
                        let started = started_run.clone();
                        let never = never_run.clone();
                        let finished = finished_run.clone();
                        async move {
                            started.notify_one();
                            never.notified().await;
                            finished.notify_one();
                            Err("gave up long after the human left".into())
                        }
                    },
                ),
            )
            .unwrap();
        let controller = AbortController::new();
        let mut authorization = request(Surface::answering("unused"));
        if caller {
            authorization.signal = Some(controller.signal());
        }
        let service = h.service.clone();
        let running = tokio::spawn(async move { service.begin(authorization).await });
        started.notified().await;
        if caller {
            controller.abort_with(AbortError::new("caller withdrew"));
        } else {
            h.service.cancel(&key());
        }
        assert_eq!(
            running.await.unwrap().unwrap().status,
            AuthorizationStatus::Cancelled
        );
        assert!(!h.service.describe(&key()).unwrap().in_flight);
        never.notify_one();
        finished.notified().await;
        assert!(!h.service.describe(&key()).unwrap().in_flight);
        h.kernel.shutdown().await;
    }
}

#[tokio::test]
async fn caller_abort_reason_reaches_the_flow_unchanged() {
    let h = harness().await;
    let received = Arc::new(Mutex::new(None));
    let received_run = received.clone();
    let started = Arc::new(Notify::new());
    let started_run = started.clone();
    h.service
        .register_flow(
            &h.ctx,
            AuthorizationFlow::new(
                key(),
                "Account",
                method("browser", "Browser"),
                [],
                move |session| {
                    let received = received_run.clone();
                    let started = started_run.clone();
                    async move {
                        started.notify_one();
                        *lock(&received) = Some(session.signal().cancelled().await);
                        Err("withdrawn".into())
                    }
                },
            ),
        )
        .unwrap();
    let controller = AbortController::new();
    let reason = SharedError::new(AbortError::new("caller withdrew"));
    let mut authorization = request(Surface::answering("unused"));
    authorization.signal = Some(controller.signal());
    let service = h.service.clone();
    let running = tokio::spawn(async move { service.begin(authorization).await });
    started.notified().await;
    controller.abort_with(reason.clone());
    assert_eq!(
        running.await.unwrap().unwrap().status,
        AuthorizationStatus::Cancelled
    );
    assert!(lock(&received).as_ref().unwrap().ptr_eq(&reason));
    h.kernel.shutdown().await;
}

#[tokio::test]
async fn flow_failure_propagates_and_settles_failed_after_releasing_the_key() {
    let h = harness().await;
    h.service
        .register_flow(
            &h.ctx,
            AuthorizationFlow::new(
                key(),
                "Account",
                method("browser", "Browser"),
                [],
                |_| async { Err("the token endpoint said no".into()) },
            ),
        )
        .unwrap();
    let settlements = Arc::new(Mutex::new(Vec::new()));
    let recorded = settlements.clone();
    let service = h.service.clone();
    h.ctx
        .on_contained::<AuthorizationSettled>(move |event| {
            lock(&recorded).push((
                event.clone(),
                service.describe(&event.key).unwrap().in_flight,
            ));
            Ok(())
        })
        .unwrap();

    assert_eq!(
        h.service
            .begin(request(Surface::answering("unused")))
            .await
            .unwrap_err()
            .to_string(),
        "the token endpoint said no"
    );
    assert_eq!(
        *lock(&settlements),
        vec![(
            AuthorizationSettled {
                key: key(),
                settlement: AuthorizationSettlement::Failed,
            },
            false,
        )]
    );
    assert!(!h.service.describe(&key()).unwrap().in_flight);
    h.kernel.shutdown().await;
}

#[tokio::test]
async fn failures_and_missing_or_deleted_commits_have_exact_taxonomy() {
    let h = harness().await;
    h.service
        .register_flow(
            &h.ctx,
            AuthorizationFlow::new(
                key(),
                "Forgetful",
                method("oauth", "Sign in"),
                [],
                |_| async { Ok(()) },
            ),
        )
        .unwrap();
    let error = h
        .service
        .begin(request(Surface::answering("unused")))
        .await
        .unwrap_err();
    assert_authorization_error(
        &error,
        "authorization flow for \"llm-pi-ai/openai-codex\" resolved without committing a credential record in this attempt",
        "NOT_COMMITTED",
    );
    h.kernel.shutdown().await;

    let h = harness().await;
    h.credentials
        .modify_record(
            &key(),
            Box::new(|_| Box::pin(async { Ok(Some(grant("stale"))) })),
        )
        .await
        .unwrap();
    let credentials = h.credentials.clone();
    h.service
        .register_flow(
            &h.ctx,
            AuthorizationFlow::new(
                key(),
                "Wrong key",
                method("oauth", "Sign in"),
                [],
                move |_| {
                    let credentials = credentials.clone();
                    async move {
                        credentials
                            .modify_record(
                                &other_key(),
                                Box::new(|_| Box::pin(async { Ok(Some(grant("other"))) })),
                            )
                            .await?;
                        Ok(())
                    }
                },
            ),
        )
        .unwrap();
    let error = h
        .service
        .begin(request(Surface::answering("unused")))
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "authorization flow for \"llm-pi-ai/openai-codex\" resolved without committing a credential record in this attempt"
    );
    assert_eq!(
        h.credentials.read_record(&key()).await.unwrap(),
        Some(grant("stale"))
    );
    h.kernel.shutdown().await;

    let h = harness().await;
    h.credentials
        .modify_record(
            &key(),
            Box::new(|_| Box::pin(async { Ok(Some(grant("stale"))) })),
        )
        .await
        .unwrap();
    let credentials = h.credentials.clone();
    h.service
        .register_flow(
            &h.ctx,
            AuthorizationFlow::new(
                key(),
                "Destructive",
                method("oauth", "Sign in"),
                [],
                move |_| {
                    let credentials = credentials.clone();
                    async move { credentials.delete_record(&key()).await }
                },
            ),
        )
        .unwrap();
    let error = h
        .service
        .begin(request(Surface::answering("unused")))
        .await
        .unwrap_err();
    assert_authorization_error(
        &error,
        "authorization flow for \"llm-pi-ai/openai-codex\" deleted its credential record instead of committing one",
        "NOT_COMMITTED",
    );
    h.kernel.shutdown().await;
}

#[tokio::test]
async fn declined_prompt_is_cancelled_even_when_rewrapped_but_other_failure_escapes() {
    for rewrap in [false, true] {
        let h = harness().await;
        let settlements = Arc::new(Mutex::new(Vec::new()));
        let recorded = settlements.clone();
        h.ctx
            .on_contained::<AuthorizationSettled>(move |event| {
                lock(&recorded).push(event.clone());
                Ok(())
            })
            .unwrap();
        h.service
            .register_flow(
                &h.ctx,
                committing_flow(h.credentials.clone(), move |session| {
                    Box::pin(async move {
                        let result = session
                            .prompt(AuthorizationPrompt::Text {
                                message: "Paste the code".into(),
                                placeholder: None,
                                signal: None,
                            })
                            .await;
                        if rewrap && result.is_err() {
                            return Err("sign-in aborted".into());
                        }
                        result.map(|_| ())
                    })
                }),
            )
            .unwrap();
        assert_eq!(
            h.service
                .begin(request(Surface::declining()))
                .await
                .unwrap()
                .status,
            AuthorizationStatus::Cancelled
        );
        assert_eq!(
            *lock(&settlements),
            vec![AuthorizationSettled {
                key: key(),
                settlement: AuthorizationSettlement::Cancelled,
            }]
        );
        h.kernel.shutdown().await;
    }

    let h = harness().await;
    let settlements = Arc::new(Mutex::new(Vec::new()));
    let recorded = settlements.clone();
    h.ctx
        .on_contained::<AuthorizationSettled>(move |event| {
            lock(&recorded).push(event.clone());
            Ok(())
        })
        .unwrap();
    h.service
        .register_flow(
            &h.ctx,
            committing_flow(h.credentials.clone(), |session| {
                Box::pin(async move {
                    session
                        .prompt(AuthorizationPrompt::Text {
                            message: "Paste the code".into(),
                            placeholder: None,
                            signal: None,
                        })
                        .await?;
                    Ok(())
                })
            }),
        )
        .unwrap();
    let broken = Arc::new(Surface {
        answer: Mutex::new(Some(Err(HarnessError::named(
            "TransportError",
            "the transport dropped",
            "DROPPED",
        )))),
        ..Surface::default()
    });
    assert_eq!(
        h.service
            .begin(request(broken))
            .await
            .unwrap_err()
            .to_string(),
        "the transport dropped"
    );
    assert_eq!(
        *lock(&settlements),
        vec![AuthorizationSettled {
            key: key(),
            settlement: AuthorizationSettlement::Failed,
        }]
    );
    assert!(!h.service.describe(&key()).unwrap().in_flight);
    h.kernel.shutdown().await;
}

#[tokio::test]
async fn notice_and_settled_listener_failures_are_contained() {
    let h = harness().await;
    h.service
        .register_flow(
            &h.ctx,
            committing_flow(h.credentials.clone(), |session| {
                session.notify(AuthorizationNotice {
                    message: "Continue in your browser".into(),
                    url: None,
                    code: None,
                });
                Box::pin(async { Ok(()) })
            }),
        )
        .unwrap();
    h.ctx
        .on_contained::<AuthorizationSettled>(|_| Err("watcher boom".into()))
        .unwrap();
    let second = Arc::new(Mutex::new(Vec::new()));
    let second_listener = second.clone();
    let service = h.service.clone();
    h.ctx
        .on_contained::<AuthorizationSettled>(move |event| {
            lock(&second_listener).push((
                event.clone(),
                service.describe(&event.key).unwrap().in_flight,
            ));
            Ok(())
        })
        .unwrap();
    h.ctx
        .on_contained_async::<AuthorizationSettled, _>(|_| async {
            Err("async watcher boom".into())
        })
        .unwrap();
    let surface = Surface::answering("unused");
    surface.fail_notice.store(true, Ordering::Release);
    assert_eq!(
        h.service.begin(request(surface)).await.unwrap().status,
        AuthorizationStatus::Authorized
    );
    assert_eq!(
        *lock(&second),
        vec![(
            AuthorizationSettled {
                key: key(),
                settlement: AuthorizationSettlement::Authorized,
            },
            false,
        )]
    );
    tokio::task::yield_now().await;
    h.kernel.shutdown().await;
}

#[tokio::test]
async fn disposing_a_flow_withdraws_its_running_attempt() {
    let h = harness().await;
    let started = Arc::new(Notify::new());
    let started_run = started.clone();
    let disposer = h
        .service
        .register_flow(
            &h.ctx,
            AuthorizationFlow::new(
                key(),
                "Account",
                method("browser", "Browser"),
                [],
                move |session| {
                    let started = started_run.clone();
                    async move {
                        started.notify_one();
                        session.signal().cancelled().await;
                        Err("withdrawn".into())
                    }
                },
            ),
        )
        .unwrap();
    let service = h.service.clone();
    let running =
        tokio::spawn(async move { service.begin(request(Surface::answering("unused"))).await });
    started.notified().await;
    disposer.dispose().await;
    assert_eq!(
        running.await.unwrap().unwrap().status,
        AuthorizationStatus::Cancelled
    );
    assert!(h.service.describe(&key()).is_none());
    h.kernel.shutdown().await;
}

#[tokio::test]
async fn late_commit_is_rejected_after_cancellation_and_during_a_new_attempt() {
    let h = harness().await;
    let first_sender = Arc::new(Mutex::new(None));
    let second_sender = Arc::new(Mutex::new(None));
    let release = Arc::new(Notify::new());
    let attempts = Arc::new(AtomicUsize::new(0));
    let first_run = first_sender.clone();
    let second_run = second_sender.clone();
    let release_run = release.clone();
    let attempts_run = attempts.clone();
    let (first_tx, first_rx) = oneshot::channel();
    let (second_tx, second_rx) = oneshot::channel();
    *lock(&first_sender) = Some(first_tx);
    *lock(&second_sender) = Some(second_tx);
    h.service
        .register_flow(
            &h.ctx,
            AuthorizationFlow::new(
                key(),
                "Account",
                method("browser", "Browser"),
                [],
                move |session| {
                    let first = first_run.clone();
                    let second = second_run.clone();
                    let release = release_run.clone();
                    let attempt = attempts_run.fetch_add(1, Ordering::AcqRel) + 1;
                    async move {
                        if attempt == 1 {
                            let _ = lock(&first).take().unwrap().send(session.clone());
                        } else {
                            let _ = lock(&second).take().unwrap().send(session.clone());
                            release.notified().await;
                        }
                        session.commit(grant(&format!("grant-{attempt}"))).await
                    }
                },
            ),
        )
        .unwrap();
    assert_eq!(
        h.service
            .begin(request(Surface::answering("unused")))
            .await
            .unwrap()
            .status,
        AuthorizationStatus::Authorized
    );
    let settled = first_rx.await.unwrap();
    let error = settled.commit(grant("late")).await.unwrap_err();
    assert_authorization_error(
        &error,
        "authorization attempt is no longer active",
        "CANCELLED",
    );
    let service = h.service.clone();
    let running =
        tokio::spawn(async move { service.begin(request(Surface::answering("unused"))).await });
    let _current = second_rx.await.unwrap();
    let error = settled.commit(grant("late")).await.unwrap_err();
    assert_authorization_error(
        &error,
        "authorization attempt is no longer active",
        "CANCELLED",
    );
    assert_eq!(
        h.credentials.read_record(&key()).await.unwrap(),
        Some(grant("grant-1"))
    );
    release.notify_one();
    assert_eq!(
        running.await.unwrap().unwrap().status,
        AuthorizationStatus::Authorized
    );
    assert_eq!(
        h.credentials.read_record(&key()).await.unwrap(),
        Some(grant("grant-2"))
    );
    h.kernel.shutdown().await;
}

#[tokio::test]
async fn commit_from_a_withdrawn_orphan_is_rejected_without_writing_a_record() {
    let h = harness().await;
    let session_sender = Arc::new(Mutex::new(None));
    let sender_run = session_sender.clone();
    let (session_tx, session_rx) = oneshot::channel();
    *lock(&session_sender) = Some(session_tx);
    h.service
        .register_flow(
            &h.ctx,
            AuthorizationFlow::new(
                key(),
                "Account",
                method("browser", "Browser"),
                [],
                move |session| {
                    let sender = sender_run.clone();
                    async move {
                        let _ = lock(&sender).take().unwrap().send(session);
                        std::future::pending::<Result<(), BoxError>>().await
                    }
                },
            ),
        )
        .unwrap();
    let service = h.service.clone();
    let running =
        tokio::spawn(async move { service.begin(request(Surface::answering("unused"))).await });
    let withdrawn_session = session_rx.await.unwrap();
    h.service.cancel(&key());
    assert_eq!(
        running.await.unwrap().unwrap().status,
        AuthorizationStatus::Cancelled
    );

    let error = withdrawn_session.commit(grant("late")).await.unwrap_err();
    assert!(is_abort_error(error.as_ref()));
    assert_eq!(error.to_string(), "This operation was aborted");
    assert_eq!(h.credentials.read_record(&key()).await.unwrap(), None);
    assert!(!h.service.describe(&key()).unwrap().in_flight);
    h.kernel.shutdown().await;
}

#[tokio::test]
async fn admitted_commit_finishes_during_local_or_caller_cancellation() {
    for caller in [false, true] {
        let h = harness().await;
        let gate = Arc::new(WriteGate {
            admitted: Notify::new(),
            release: Notify::new(),
        });
        h.credentials.gate(gate.clone());
        h.service
            .register_flow(
                &h.ctx,
                AuthorizationFlow::new(
                    key(),
                    "Account",
                    method("browser", "Browser"),
                    [],
                    |session| async move { session.commit(grant("saved")).await },
                ),
            )
            .unwrap();
        let controller = AbortController::new();
        let mut authorization = request(Surface::answering("unused"));
        if caller {
            authorization.signal = Some(controller.signal());
        }
        let service = h.service.clone();
        let running = tokio::spawn(async move { service.begin(authorization).await });
        gate.admitted.notified().await;
        if caller {
            controller.abort();
        } else {
            h.service.cancel(&key());
        }
        gate.release.notify_one();
        assert_eq!(
            running.await.unwrap().unwrap().status,
            AuthorizationStatus::Authorized
        );
        assert_eq!(
            h.credentials.read_record(&key()).await.unwrap(),
            Some(grant("saved"))
        );
        h.kernel.shutdown().await;
    }
}

#[test]
fn wire_types_and_declined_error_are_exact() {
    assert_eq!(
        serde_json::to_value(AuthorizationNotice {
            message: "Continue".into(),
            url: Some("https://example.test".into()),
            code: None,
        })
        .unwrap(),
        json!({"message":"Continue","url":"https://example.test"})
    );
    assert_eq!(
        serde_json::to_value(AuthorizationOutcome {
            status: AuthorizationStatus::Cancelled
        })
        .unwrap(),
        json!({"status":"cancelled"})
    );
    let error = AuthorizationDeclinedError::new();
    assert_eq!(error.name(), AuthorizationDeclinedError::NAME);
    assert_eq!(error.code(), "DECLINED");
    assert_eq!(error.message(), "the authorization prompt was declined");
}
